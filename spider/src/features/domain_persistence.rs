//! Canonical persistence seam for Scorpion domain state.
//!
//! Track 3 of the roadmap, built to store — never decide — the state
//! Track 2 ([`crate::features::domain_state`]) defines and the identities
//! Track 1 ([`crate::features::identity`]) names. This module imports
//! neither: it never sees `EvidenceId`/`WatchId`, `CurrentState`, or
//! `Transition`. Every identity it stores is an opaque `&str` (any
//! identity's canonical `Display` form); every state it stores is an
//! opaque `&[u8]` (whatever bytes the caller already decided, via a real
//! [`crate::features::domain_state::Transition`], represent the new
//! state). That is deliberate — a storage mechanism that had to import a
//! concrete domain type to compile would already be deciding something
//! about that domain.
//!
//! # Ownership boundary
//!
//! - **This module stores canonical domain state.** It does not decide
//!   whether a transition is valid (there is no `Transition` parameter
//!   anywhere in this file — see [`DomainPersistence::write_current`]'s
//!   signature), it does not invent lifecycle state (there is no status
//!   enum, no "active"/"closed" concept, nothing beyond an opaque
//!   revision counter), and it does not own domain semantics (no field of
//!   any state is ever inspected — everything is a `BLOB`).
//! - **Domain code (Track 2) decides transitions before calling here.**
//!   The expected call shape is: compute `Applied` via
//!   `CurrentState::apply`, serialize `applied.current.state()` and
//!   `applied.superseded.state()` to bytes some other way (this module
//!   does not pick or perform a serialization format), then call
//!   [`DomainPersistence::write_current`] and
//!   [`DomainPersistence::append_history`] to record the result. This
//!   module never calls back into Track 2 and never will — that would
//!   invert the ownership boundary `SCORPION_SDD.md` §5.2 draws
//!   ("persistence stores state but does not decide valid domain
//!   transitions").
//!
//! # Two operations, two failure-closed guarantees
//!
//! 1. **Current state** — [`DomainPersistence::write_current`] is the
//!    *only* way to change what is stored as current for an identity, and
//!    it is compare-and-swap, not overwrite: the caller must state the
//!    revision it believes is currently stored (`None` for "no row must
//!    exist yet"), and the write is rejected — nothing touched — if that
//!    does not match what is actually stored. There is no second,
//!    unconditional "just set it" method anywhere in this type.
//! 2. **Historical record** — [`DomainPersistence::append_history`] can
//!    only add a record; a `(identity, revision)` pair that has already
//!    been recorded is rejected outright (the database's own primary-key
//!    constraint enforces this, not application logic that could be
//!    bypassed), and the pre-existing record is left byte-for-byte
//!    untouched.
//!
//! # Storage technology
//!
//! Reuses the crate's existing `sqlx`/SQLite dependency (already present
//! for Spider's own crawl-resume database, `features/disk.rs`, behind the
//! same `disk` feature) rather than introducing a second persistence
//! stack. This module owns its own two tables
//! (`scorpion_domain_current_state`, `scorpion_domain_history`) and its
//! own connection pool — it does not share `disk.rs`'s `DatabaseHandler`
//! or its `resources`/`signatures` tables, which are Spider's upstream
//! crawl-resume mechanism, not canonical Scorpion domain state, and carry
//! unrelated (non-transition-aware, freely overwritable) semantics.
//!
//! The pool is deliberately capped at one connection: SQLite allows only
//! one writer at a time regardless, and a single connection makes the
//! read-then-conditionally-write sequence inside [`DomainPersistence::write_current`]
//! trivially race-free — no other query from this seam can interleave
//! between the read and the write, because there is only ever one
//! connection to run either on.
//!
//! # Not implemented here
//!
//! Per this frontier's explicit scope: no Evidence Ledger product
//! semantics, no authenticated-session lifecycle, no `WatchDefinition`/
//! `WatchState`, no Fingerprint/Lineage, no scheduling, no change
//! detection, no health, no event sourcing, and no generic Job/Operation
//! persistence. This module is a mechanism two future capabilities will
//! call into, not a capability itself.

use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::path::Path;
use std::str::FromStr;
use std::time::SystemTime;

/// Failure from this seam. Every variant is storage-shaped — none names a
/// domain concept (no "invalid transition", no lifecycle status).
#[derive(Debug)]
pub enum PersistenceError {
    /// [`DomainPersistence::write_current`]'s `expected_revision` did not
    /// match what is actually stored. Carries the actual stored revision
    /// (`None` if no row exists at all) so the caller can decide what to
    /// do next; the write this error is returned from never happened.
    CurrentStateConflict {
        /// The revision actually stored, if a row exists at all.
        actual: Option<u64>,
    },
    /// [`DomainPersistence::append_history`] was called for an
    /// `(identity, revision)` pair that already has a historical record.
    /// The write did not happen; the existing record is untouched.
    HistoryAlreadyExists,
    /// The underlying storage engine failed for a reason unrelated to the
    /// conflict semantics above (I/O, corruption, connection loss, ...).
    Backend(sqlx::Error),
}

impl std::fmt::Display for PersistenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PersistenceError::CurrentStateConflict { actual } => write!(
                f,
                "current-state write rejected: expected revision did not match \
                 actual stored revision {actual:?}"
            ),
            PersistenceError::HistoryAlreadyExists => write!(
                f,
                "historical record already exists for this identity/revision; \
                 historical records are never replaced"
            ),
            PersistenceError::Backend(error) => write!(f, "persistence backend error: {error}"),
        }
    }
}

impl std::error::Error for PersistenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PersistenceError::Backend(error) => Some(error),
            _ => None,
        }
    }
}

/// The canonical persistence seam: one SQLite-backed store for current
/// domain state (compare-and-swap only) and historical records
/// (append-only, fails closed on a duplicate key).
///
/// Every method takes the identity as a plain `&str` — this type never
/// imports, constructs, or inspects an `EvidenceId`/`WatchId` or any other
/// identity type. Callers pass `identity.to_string()` (every canonical
/// identity type formats deterministically via `Display`).
pub struct DomainPersistence {
    pool: SqlitePool,
}

impl DomainPersistence {
    /// Open (creating if absent) a SQLite-backed persistence store at
    /// `path`, creating its two tables if they do not already exist.
    pub async fn open(path: &Path) -> Result<Self, PersistenceError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
            .map_err(PersistenceError::Backend)?
            .create_if_missing(true);
        Self::open_with_options(options).await
    }

    /// Open a private, in-memory persistence store — every table lives
    /// only for the lifetime of the returned value. Intended for tests
    /// and any caller that explicitly wants no durability.
    pub async fn open_in_memory() -> Result<Self, PersistenceError> {
        Self::open_with_options(SqliteConnectOptions::new().in_memory(true)).await
    }

    async fn open_with_options(options: SqliteConnectOptions) -> Result<Self, PersistenceError> {
        // Capped at one connection deliberately — see the module-level
        // "Storage technology" doc section for why this is what makes
        // write_current's compare-and-swap race-free.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(PersistenceError::Backend)?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS scorpion_domain_current_state (
                identity TEXT PRIMARY KEY,
                revision INTEGER NOT NULL,
                state BLOB NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .map_err(PersistenceError::Backend)?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS scorpion_domain_history (
                identity TEXT NOT NULL,
                revision INTEGER NOT NULL,
                state BLOB NOT NULL,
                recorded_at_unix_ms INTEGER NOT NULL,
                PRIMARY KEY (identity, revision)
            )",
        )
        .execute(&pool)
        .await
        .map_err(PersistenceError::Backend)?;

        Ok(Self { pool })
    }

    /// Read the current stored revision and state bytes for `identity`,
    /// or `None` if nothing has ever been written for it.
    pub async fn read_current(
        &self,
        identity: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, PersistenceError> {
        let row = sqlx::query(
            "SELECT revision, state FROM scorpion_domain_current_state WHERE identity = ?",
        )
        .bind(identity)
        .fetch_optional(&self.pool)
        .await
        .map_err(PersistenceError::Backend)?;

        Ok(row.map(|row| {
            let revision: i64 = row.get("revision");
            let state: Vec<u8> = row.get("state");
            (revision as u64, state)
        }))
    }

    /// Replace the current state for `identity` — but only if the
    /// revision actually stored right now equals `expected_revision`
    /// exactly. Pass `None` to mean "no row must exist yet" (first
    /// write); pass `Some(revision)` (as returned by a prior successful
    /// call, or read via [`Self::read_current`]) to mean "only replace it
    /// if it is still exactly that revision."
    ///
    /// This is the *only* method that changes current state. There is no
    /// unconditional-overwrite counterpart: a caller that does not know
    /// (or does not check) the expected prior revision cannot write
    /// current state through this seam at all.
    ///
    /// On success, returns the new revision (always `expected_revision`
    /// interpreted-as-0 `+ 1`, i.e. `1` for a first write). On conflict,
    /// returns [`PersistenceError::CurrentStateConflict`] with the
    /// actually-stored revision, and nothing was written.
    pub async fn write_current(
        &self,
        identity: &str,
        expected_revision: Option<u64>,
        new_state: &[u8],
    ) -> Result<u64, PersistenceError> {
        let mut tx = self.pool.begin().await.map_err(PersistenceError::Backend)?;

        let actual: Option<i64> = sqlx::query_scalar(
            "SELECT revision FROM scorpion_domain_current_state WHERE identity = ?",
        )
        .bind(identity)
        .fetch_optional(&mut *tx)
        .await
        .map_err(PersistenceError::Backend)?;
        let actual = actual.map(|revision| revision as u64);

        if actual != expected_revision {
            // Dropping `tx` without committing rolls it back — nothing
            // written. Fail closed: an unexpected prior state is never
            // silently blind-overwritten.
            return Err(PersistenceError::CurrentStateConflict { actual });
        }

        let new_revision = expected_revision.unwrap_or(0) + 1;

        if expected_revision.is_some() {
            sqlx::query(
                "UPDATE scorpion_domain_current_state SET revision = ?, state = ? \
                 WHERE identity = ?",
            )
            .bind(new_revision as i64)
            .bind(new_state)
            .bind(identity)
            .execute(&mut *tx)
            .await
            .map_err(PersistenceError::Backend)?;
        } else {
            sqlx::query(
                "INSERT INTO scorpion_domain_current_state (identity, revision, state) \
                 VALUES (?, ?, ?)",
            )
            .bind(identity)
            .bind(new_revision as i64)
            .bind(new_state)
            .execute(&mut *tx)
            .await
            .map_err(PersistenceError::Backend)?;
        }

        tx.commit().await.map_err(PersistenceError::Backend)?;
        Ok(new_revision)
    }

    /// Append one immutable historical record for `identity` at
    /// `revision`. Fails closed — returns
    /// [`PersistenceError::HistoryAlreadyExists`] and writes nothing — if
    /// a record already exists for this exact `(identity, revision)`
    /// pair. There is no method anywhere in this type that can modify or
    /// remove a historical record once appended.
    pub async fn append_history(
        &self,
        identity: &str,
        revision: u64,
        state: &[u8],
        recorded_at: SystemTime,
    ) -> Result<(), PersistenceError> {
        let recorded_at_unix_ms = recorded_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);

        let result = sqlx::query(
            "INSERT INTO scorpion_domain_history \
             (identity, revision, state, recorded_at_unix_ms) VALUES (?, ?, ?, ?)",
        )
        .bind(identity)
        .bind(revision as i64)
        .bind(state)
        .bind(recorded_at_unix_ms)
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(db_error)) if db_error.is_unique_violation() => {
                Err(PersistenceError::HistoryAlreadyExists)
            }
            Err(error) => Err(PersistenceError::Backend(error)),
        }
    }

    /// Read every historical record for `identity`, oldest (lowest
    /// revision) first.
    pub async fn read_history(
        &self,
        identity: &str,
    ) -> Result<Vec<(u64, Vec<u8>, SystemTime)>, PersistenceError> {
        let rows = sqlx::query(
            "SELECT revision, state, recorded_at_unix_ms FROM scorpion_domain_history \
             WHERE identity = ? ORDER BY revision ASC",
        )
        .bind(identity)
        .fetch_all(&self.pool)
        .await
        .map_err(PersistenceError::Backend)?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let revision: i64 = row.get("revision");
                let state: Vec<u8> = row.get("state");
                let recorded_at_unix_ms: i64 = row.get("recorded_at_unix_ms");
                let recorded_at = SystemTime::UNIX_EPOCH
                    + std::time::Duration::from_millis(recorded_at_unix_ms.max(0) as u64);
                (revision as u64, state, recorded_at)
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn first_write_requires_none_and_yields_revision_one() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let revision = store
            .write_current("watch_abc", None, b"initial")
            .await
            .unwrap();
        assert_eq!(revision, 1);
        let (stored_revision, stored_state) =
            store.read_current("watch_abc").await.unwrap().unwrap();
        assert_eq!(stored_revision, 1);
        assert_eq!(stored_state, b"initial");
    }

    #[tokio::test]
    async fn second_write_with_none_fails_closed_no_blind_overwrite() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        store
            .write_current("watch_abc", None, b"initial")
            .await
            .unwrap();

        // Attempting a second "first write" (None) must be rejected —
        // there is no way to blind-overwrite an existing current state.
        let error = store
            .write_current("watch_abc", None, b"blind-overwrite")
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            PersistenceError::CurrentStateConflict { actual: Some(1) }
        ));

        // Nothing was touched.
        let (revision, state) = store.read_current("watch_abc").await.unwrap().unwrap();
        assert_eq!(revision, 1);
        assert_eq!(state, b"initial");
    }

    #[tokio::test]
    async fn stale_expected_revision_fails_closed() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        store.write_current("watch_abc", None, b"v1").await.unwrap();
        store
            .write_current("watch_abc", Some(1), b"v2")
            .await
            .unwrap();

        // A caller that still thinks revision 1 is current (stale read)
        // must be rejected now that revision 2 is stored.
        let error = store
            .write_current("watch_abc", Some(1), b"v3-based-on-stale-read")
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            PersistenceError::CurrentStateConflict { actual: Some(2) }
        ));

        let (revision, state) = store.read_current("watch_abc").await.unwrap().unwrap();
        assert_eq!(revision, 2);
        assert_eq!(state, b"v2");
    }

    #[tokio::test]
    async fn correct_expected_revision_succeeds_and_advances() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let r1 = store.write_current("watch_abc", None, b"v1").await.unwrap();
        let r2 = store
            .write_current("watch_abc", Some(r1), b"v2")
            .await
            .unwrap();
        assert_eq!(r2, 2);
        let (revision, state) = store.read_current("watch_abc").await.unwrap().unwrap();
        assert_eq!(revision, 2);
        assert_eq!(state, b"v2");
    }

    #[tokio::test]
    async fn read_current_of_unknown_identity_is_none() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        assert!(store.read_current("never-written").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn append_history_then_duplicate_fails_closed_and_leaves_original_untouched() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let now = SystemTime::now();
        store
            .append_history("evid_abc", 1, b"superseded-v1", now)
            .await
            .unwrap();

        let error = store
            .append_history("evid_abc", 1, b"attempted-overwrite", now)
            .await
            .unwrap_err();
        assert!(matches!(error, PersistenceError::HistoryAlreadyExists));

        let history = store.read_history("evid_abc").await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].0, 1);
        assert_eq!(history[0].1, b"superseded-v1");
    }

    #[tokio::test]
    async fn history_is_append_only_and_ordered() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let t0 = SystemTime::now();
        store
            .append_history("evid_abc", 1, b"first", t0)
            .await
            .unwrap();
        store
            .append_history("evid_abc", 2, b"second", t0)
            .await
            .unwrap();
        store
            .append_history("evid_abc", 3, b"third", t0)
            .await
            .unwrap();

        let history = store.read_history("evid_abc").await.unwrap();
        let revisions: Vec<u64> = history.iter().map(|(revision, _, _)| *revision).collect();
        assert_eq!(revisions, vec![1, 2, 3]);
        let states: Vec<&[u8]> = history
            .iter()
            .map(|(_, state, _)| state.as_slice())
            .collect();
        assert_eq!(
            states,
            vec![
                b"first".as_slice(),
                b"second".as_slice(),
                b"third".as_slice()
            ]
        );
    }

    #[tokio::test]
    async fn distinct_identities_do_not_interfere() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        store.write_current("evid_a", None, b"a1").await.unwrap();
        store.write_current("evid_b", None, b"b1").await.unwrap();

        let (rev_a, state_a) = store.read_current("evid_a").await.unwrap().unwrap();
        let (rev_b, state_b) = store.read_current("evid_b").await.unwrap().unwrap();
        assert_eq!((rev_a, state_a), (1, b"a1".to_vec()));
        assert_eq!((rev_b, state_b), (1, b"b1".to_vec()));
    }

    #[tokio::test]
    async fn open_creates_parent_directory_and_persists_to_a_real_file() {
        let dir = std::env::temp_dir().join(format!(
            "scorpion-domain-persistence-test-{}",
            std::process::id()
        ));
        let db_path = dir.join("nested").join("domain.sqlite3");
        let _ = std::fs::remove_dir_all(&dir);

        {
            let store = DomainPersistence::open(&db_path).await.unwrap();
            store.write_current("evid_a", None, b"a1").await.unwrap();
        }

        assert!(db_path.exists());

        // Reopening the same file sees the previously written state.
        let store = DomainPersistence::open(&db_path).await.unwrap();
        let (revision, state) = store.read_current("evid_a").await.unwrap().unwrap();
        assert_eq!(revision, 1);
        assert_eq!(state, b"a1");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
