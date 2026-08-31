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

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;
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

/// `true` only for a raw `SQLITE_BUSY` (extended result code `5`) —
/// "database is locked" — surfaced through this module's own
/// [`PersistenceError::Backend`] wrapping. Never matches any other error
/// (a genuine I/O failure, a malformed path, corruption); [`DomainPersistence::open`]'s
/// retry loop must fail fast on anything else.
fn is_sqlite_busy(error: &PersistenceError) -> bool {
    let PersistenceError::Backend(sqlx::Error::Database(db_error)) = error else {
        return false;
    };
    db_error.code().as_deref() == Some("5")
}

impl DomainPersistence {
    /// Open (creating if absent) a SQLite-backed persistence store at
    /// `path`, creating its two tables if they do not already exist.
    ///
    /// Retries a bounded number of times, with a short backoff, on a raw
    /// `SQLITE_BUSY` ("database is locked") specifically — proven
    /// necessary, not merely theoretical, by
    /// `SCORPION_MCP_CANONICAL_PAGE_AUDIT_SHIPPING_001`'s own
    /// concurrent-`open()`-against-a-fresh-file tests: converting a
    /// brand-new file to WAL journal mode is a one-time operation SQLite
    /// performs during connection setup, and two connections racing to
    /// perform that conversion on the *same not-yet-existing* file can
    /// both observe `SQLITE_BUSY` there — before this module's own
    /// `BEGIN IMMEDIATE`-guarded table creation ever runs, so that fix
    /// alone does not cover this earlier, narrower race. This retry is
    /// scoped to file-backed opens only (never `open_in_memory`, which
    /// creates a private, never-shared, never-racing database) and stops
    /// retrying immediately on any other error.
    pub async fn open(path: &Path) -> Result<Self, PersistenceError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
            .map_err(PersistenceError::Backend)?
            .create_if_missing(true);

        const MAX_ATTEMPTS: u32 = 5;
        let mut attempt = 0_u32;
        loop {
            attempt += 1;
            match Self::open_with_options(options.clone()).await {
                Ok(store) => return Ok(store),
                Err(error) if attempt < MAX_ATTEMPTS && is_sqlite_busy(&error) => {
                    tokio::time::sleep(Duration::from_millis(20 * u64::from(attempt))).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Open a private, in-memory persistence store — every table lives
    /// only for the lifetime of the returned value. Intended for tests
    /// and any caller that explicitly wants no durability.
    pub async fn open_in_memory() -> Result<Self, PersistenceError> {
        Self::open_with_options(SqliteConnectOptions::new().in_memory(true)).await
    }

    async fn open_with_options(options: SqliteConnectOptions) -> Result<Self, PersistenceError> {
        // WAL journal mode + an explicit busy timeout: SQLite's default
        // rollback-journal mode returns `SQLITE_BUSY` ("database is
        // locked") immediately on writer contention between two separate
        // connections to the same file — proven empirically by
        // `SCORPION_CANONICAL_SHARED_DOMAIN_PERSISTENCE_RUNTIME_BINDING_001`'s
        // own multi-handle concurrency tests before this fix. WAL mode is
        // the standard SQLite-recommended configuration for exactly this
        // shape (multiple independently opened connections/processes
        // against one file): readers never block a writer and vice versa,
        // and a genuine writer/writer conflict then waits (retrying
        // internally) up to `busy_timeout` instead of failing instantly.
        // This is configuration of the existing single mechanism, not a
        // second one — no new dependency, no new table, no new pool
        // beyond the existing one-per-`DomainPersistence` cap this
        // module's own compare-and-swap race-freedom already relies on.
        // Harmless (and effectively a no-op journal-mode-wise) for
        // `open_in_memory()` — SQLite always uses its own `MEMORY`
        // journal mode for `:memory:` regardless of what is requested.
        let options = options
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));

        // Capped at one connection deliberately — see the module-level
        // "Storage technology" doc section for why this is what makes
        // write_current's compare-and-swap race-free.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(PersistenceError::Backend)?;

        // `BEGIN IMMEDIATE`, matching `write_current`'s own fix and for
        // exactly the same reason: two independently constructed
        // `DomainPersistence` values racing to `open()` the *same
        // not-yet-existing* file each get their own connection, and each
        // connection issuing these two `CREATE TABLE IF NOT EXISTS`
        // statements as separate, ordinary (deferred) auto-commit
        // statements let two concurrent openers race on first table
        // creation — proven empirically by
        // `SCORPION_MCP_CANONICAL_PAGE_AUDIT_SHIPPING_001`'s own
        // concurrent-`spider_audit_page`-calls-against-a-fresh-store test
        // (a scenario the prerequisite frontier's own concurrency tests
        // never exercised: those opened both handles sequentially, before
        // testing concurrent *writes* — never concurrent *first opens*).
        // Taking the write lock immediately, before either statement
        // runs, makes a second concurrent opener wait and retry via
        // `busy_timeout` instead of failing instantly, exactly like
        // `write_current`.
        let mut tx = pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(PersistenceError::Backend)?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS scorpion_domain_current_state (
                identity TEXT PRIMARY KEY,
                revision INTEGER NOT NULL,
                state BLOB NOT NULL
            )",
        )
        .execute(&mut *tx)
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
        .execute(&mut *tx)
        .await
        .map_err(PersistenceError::Backend)?;

        tx.commit().await.map_err(PersistenceError::Backend)?;

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
        // `BEGIN IMMEDIATE`, not a plain (deferred) `BEGIN`: this
        // transaction reads before it writes, and a deferred transaction
        // only acquires SQLite's write lock at the *first write*, not at
        // `BEGIN` — so two concurrent deferred transactions from two
        // separate connections (exactly the shape two independently
        // opened `DomainPersistence` handles against one file produce)
        // can each take the initial read lock, then both try to upgrade
        // to a write lock at the same time. That specific upgrade
        // conflict is a real SQLite deadlock, not an ordinary busy-wait —
        // `busy_timeout` does not resolve it. `BEGIN IMMEDIATE` takes the
        // write lock up front, so a second concurrent writer instead
        // blocks (and correctly retries via `busy_timeout`) before ever
        // starting its own read — proven by
        // `concurrent_use_from_two_handles_against_the_same_file_does_not_deadlock_or_corrupt`.
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(PersistenceError::Backend)?;

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

    // --- SCORPION_CANONICAL_SHARED_DOMAIN_PERSISTENCE_RUNTIME_BINDING_001 ---
    //
    // Prerequisite reconnaissance for MCP audit shipping: prove two
    // independently constructed `DomainPersistence` values (never the same
    // Rust value, never sharing a `SqlitePool`) can safely observe the same
    // real on-disk SQLite file, and that ordinary near-concurrent use does
    // not reveal an architectural blocker. `open_in_memory()` is
    // deliberately never used in this section — an in-memory store is
    // process-local and cannot be shared across two handles at all, which
    // is exactly the property under test.

    fn shared_binding_test_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "scorpion-shared-binding-test-{}-{name}-{}.sqlite3",
            std::process::id(),
            // A monotonically increasing per-process counter, not just the
            // PID, so multiple tests in this same file never race on one
            // path even though `cargo test` runs them concurrently by
            // default.
            {
                static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            }
        ))
    }

    /// Gate 2: two independently opened `DomainPersistence` instances,
    /// against the same real file, are not two separate stores — a write
    /// through one is visible through the other for both `write_current`
    /// and `append_history`.
    #[tokio::test]
    async fn two_independently_opened_handles_against_the_same_file_see_each_others_writes() {
        let path = shared_binding_test_path("cross-handle-visibility");
        let _ = std::fs::remove_file(&path);

        let handle_a = DomainPersistence::open(&path).await.unwrap();
        let handle_b = DomainPersistence::open(&path).await.unwrap();

        // write_current through A, read through B.
        handle_a
            .write_current("watch_shared_test", None, b"from-a")
            .await
            .unwrap();
        let (revision, state) = handle_b
            .read_current("watch_shared_test")
            .await
            .unwrap()
            .expect("handle_b must see handle_a's write_current");
        assert_eq!(revision, 1);
        assert_eq!(state, b"from-a");

        // append_history through B, read through A.
        let now = SystemTime::now();
        handle_b
            .append_history("evid_shared_test", 1, b"from-b", now)
            .await
            .unwrap();
        let history = handle_a.read_history("evid_shared_test").await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].1, b"from-b");

        // A second write_current on B (compare-and-swap against the
        // revision A produced) round-trips correctly too — the CAS
        // invariant holds across handles, not just within one.
        handle_b
            .write_current("watch_shared_test", Some(1), b"from-b-cas")
            .await
            .unwrap();
        let (revision, state) = handle_a
            .read_current("watch_shared_test")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(revision, 2);
        assert_eq!(state, b"from-b-cas");

        let _ = std::fs::remove_file(&path);
    }

    /// Gate 5: near-concurrent use from two independently opened handles
    /// against the same file does not deadlock, corrupt data, or lose a
    /// write — proven both for disjoint identities (no lock contention
    /// expected) and for the same identity (real SQLite writer
    /// contention, resolved by each handle's single-connection pool
    /// serializing its own queries and SQLite's own file locking
    /// serializing the two pools against each other).
    #[tokio::test]
    async fn concurrent_use_from_two_handles_against_the_same_file_does_not_deadlock_or_corrupt() {
        let path = shared_binding_test_path("concurrent-use");
        let _ = std::fs::remove_file(&path);

        let handle_a = DomainPersistence::open(&path).await.unwrap();
        let handle_b = DomainPersistence::open(&path).await.unwrap();

        // Disjoint identities, concurrently, from two separate handles.
        let (result_a, result_b) = tokio::join!(
            handle_a.write_current("watch_concurrent_a", None, b"a-value"),
            handle_b.write_current("watch_concurrent_b", None, b"b-value"),
        );
        result_a.unwrap();
        result_b.unwrap();
        assert_eq!(
            handle_a
                .read_current("watch_concurrent_a")
                .await
                .unwrap()
                .unwrap()
                .1,
            b"a-value"
        );
        assert_eq!(
            handle_b
                .read_current("watch_concurrent_b")
                .await
                .unwrap()
                .unwrap()
                .1,
            b"b-value"
        );

        // The same identity, from two handles, truly concurrently. SQLite
        // serializes real writer contention at the file level; neither
        // side may silently lose its write, corrupt the row, or hang.
        // Exactly one of the two "first write" (`None`) attempts must
        // succeed and the other must see a genuine
        // `CurrentStateConflict` — never both succeeding (which would mean
        // a lost update) and never both failing (which would mean the row
        // was never written at all).
        let (first, second) = tokio::join!(
            handle_a.write_current("watch_contended", None, b"from-a"),
            handle_b.write_current("watch_contended", None, b"from-b"),
        );
        let outcomes = [first, second];
        let successes = outcomes.iter().filter(|result| result.is_ok()).count();
        assert_eq!(
            successes, 1,
            "exactly one concurrent first-write must win; got {outcomes:?}"
        );
        let (revision, state) = handle_a
            .read_current("watch_contended")
            .await
            .unwrap()
            .expect("the winning write must be durably visible");
        assert_eq!(revision, 1);
        assert!(state == b"from-a" || state == b"from-b");

        // append_history to the same identity/revision from both handles
        // concurrently: exactly one must win (HistoryAlreadyExists for the
        // loser), and the winning record is never overwritten.
        let now = SystemTime::now();
        let (first, second) = tokio::join!(
            handle_a.append_history("evid_contended", 1, b"from-a", now),
            handle_b.append_history("evid_contended", 1, b"from-b", now),
        );
        let outcomes = [&first, &second];
        let successes = outcomes.iter().filter(|result| result.is_ok()).count();
        assert_eq!(
            successes, 1,
            "exactly one concurrent history append must win"
        );
        let history = handle_b.read_history("evid_contended").await.unwrap();
        assert_eq!(history.len(), 1);

        let _ = std::fs::remove_file(&path);
    }

    /// Gate 2 (reopen variant): a `DomainPersistence` opened, dropped, and
    /// reopened by a second, independent process-equivalent handle against
    /// the same path sees everything the first handle durably wrote — the
    /// scenario a short-lived MCP tool call (open → one write → drop)
    /// followed by a later CLI/Web Console read actually looks like.
    #[tokio::test]
    async fn a_handle_opened_after_the_first_is_dropped_sees_everything_the_first_wrote() {
        let path = shared_binding_test_path("reopen-after-drop");
        let _ = std::fs::remove_file(&path);

        {
            let handle = DomainPersistence::open(&path).await.unwrap();
            handle
                .write_current("watch_reopen", None, b"durable")
                .await
                .unwrap();
            handle
                .append_history("evid_reopen", 1, b"durable-history", SystemTime::now())
                .await
                .unwrap();
        } // handle dropped — its pool and connection are gone.

        let reopened = DomainPersistence::open(&path).await.unwrap();
        let (revision, state) = reopened
            .read_current("watch_reopen")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(revision, 1);
        assert_eq!(state, b"durable");
        let history = reopened.read_history("evid_reopen").await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].1, b"durable-history");

        let _ = std::fs::remove_file(&path);
    }

    /// `SCORPION_MCP_CANONICAL_PAGE_AUDIT_SHIPPING_001`: two concurrent
    /// `DomainPersistence::open()` calls against the *same not-yet-existing*
    /// file — the scenario two `spider_audit_page` MCP calls arriving at
    /// nearly the same moment against a fresh `SCORPION_DOMAIN_DB` produce.
    /// Distinct from, and initially not covered by, the concurrent-*write*
    /// proof above (which opens both handles sequentially first). Without
    /// `open_with_options`'s own `BEGIN IMMEDIATE` fix, this genuinely
    /// failed with a real "database is locked" error — both connections
    /// racing to run `CREATE TABLE IF NOT EXISTS` as separate deferred
    /// auto-commit statements.
    #[tokio::test]
    async fn concurrent_first_open_of_the_same_not_yet_existing_file_does_not_fail() {
        let path = shared_binding_test_path("concurrent-first-open");
        let _ = std::fs::remove_file(&path);
        assert!(!path.exists());

        let (first, second) = tokio::join!(
            DomainPersistence::open(&path),
            DomainPersistence::open(&path)
        );
        let first = first.unwrap();
        let second = second.unwrap();

        first
            .write_current("watch_concurrent_open", None, b"from-first")
            .await
            .unwrap();
        let (revision, state) = second
            .read_current("watch_concurrent_open")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(revision, 1);
        assert_eq!(state, b"from-first");

        let _ = std::fs::remove_file(&path);
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
