//! Canonical, neutral runtime binding to the one shared [`DomainPersistence`]
//! store every Scorpion interface may open.
//!
//! [`crate::features::domain_persistence`] is deliberately mechanism-only —
//! it never resolves a database *path* from operator configuration, and it
//! never decides which interface's env var/CLI flag wins. Before this
//! module, that resolution existed in exactly one place —
//! `spider_cli::research`'s `RESEARCH_EVIDENCE_DB` — genuinely scoped to
//! Research (its own module doc comment says "durable canonical
//! **research**," its owning struct is `ResearchService`/`RunParams`), not
//! a documented cross-interface contract. `SCORPION_MCP_CANONICAL_PAGE_AUDIT_SHIPPING_001`
//! needed a real neutral binding and found none — this module is that
//! prerequisite (`SCORPION_CANONICAL_SHARED_DOMAIN_PERSISTENCE_RUNTIME_BINDING_001`).
//!
//! # Shared-store safety (proven, not assumed)
//!
//! Before this binding was defined, this frontier proved from real,
//! multi-handle tests (in [`crate::features::domain_persistence`],
//! [`crate::utils::evidence`], and `crate::features::audit`) that:
//!
//! 1. every current canonical identity/derived-record-identity type's wire
//!    prefix is pairwise distinct (`spider/tests/architecture_guardrails.rs`'s
//!    `every_canonical_identity_prefix_is_pairwise_distinct`) — two
//!    different identity types can never collide in
//!    `DomainPersistence`'s flat, per-domain-namespace-free `TEXT` primary
//!    key;
//! 2. two independently constructed `DomainPersistence` values opened
//!    against the same real file observe each other's writes immediately;
//! 3. evidence recorded through one handle resolves through another;
//! 4. a `Finding` recorded through one handle reads back through another;
//! 5. near-concurrent use from two handles against the same file does not
//!    deadlock or corrupt data — proven only after this frontier hardened
//!    [`DomainPersistence::open`]'s connection configuration (WAL journal
//!    mode, an explicit busy timeout, and `BEGIN IMMEDIATE` for
//!    [`DomainPersistence::write_current`]) against a real "database is
//!    locked" failure the unhardened configuration produced under
//!    contention.
//!
//! One shared SQLite file is therefore safe for multiple interfaces to
//! open independently — this module exists to make that the *only*
//! sanctioned way to obtain the path, not to invent a second persistence
//! mechanism.
//!
//! # `RESEARCH_EVIDENCE_DB` — explicit reconciliation, not silent aliasing
//!
//! [`DOMAIN_DATABASE_ENV`] is the new canonical, neutral variable. The
//! pre-existing [`LEGACY_RESEARCH_DATABASE_ENV`] remains fully honored as
//! an explicit, tested, lower-priority fallback — an operator's existing
//! `RESEARCH_EVIDENCE_DB`-only deployment keeps working unmodified, with
//! no code change and no silent behavior shift, because
//! [`resolve_domain_database_path`] checks [`DOMAIN_DATABASE_ENV`] first
//! and falls back to [`LEGACY_RESEARCH_DATABASE_ENV`] only when the new
//! variable is unset. This relationship is a real, tested code path
//! (`spider_cli::research` and `scorpion_app` are wired through this exact
//! function — see their own call sites), not documentation asserting a
//! coincidence. A fresh deployment, or any interface beyond Research
//! (canonical audit's own shipping frontier, and a future Web Console),
//! should set [`DOMAIN_DATABASE_ENV`].

use crate::features::domain_persistence::{DomainPersistence, PersistenceError};
use std::path::PathBuf;

/// The canonical, neutral environment variable naming the one shared
/// Scorpion domain database path. Every capability that needs a durable
/// [`DomainPersistence`] handle resolves its database path through
/// [`resolve_domain_database_path`]/[`open_shared_domain_store`], never
/// through a capability-local variable of its own.
pub const DOMAIN_DATABASE_ENV: &str = "SCORPION_DOMAIN_DB";

/// Legacy, Research-originated variable name, honored as an explicit
/// lower-priority fallback — see this module's own doc comment,
/// "`RESEARCH_EVIDENCE_DB` — explicit reconciliation, not silent
/// aliasing."
pub const LEGACY_RESEARCH_DATABASE_ENV: &str = "RESEARCH_EVIDENCE_DB";

/// Resolve the one shared domain database path, in explicit priority
/// order: `explicit` (e.g. a caller's own CLI flag), then
/// [`DOMAIN_DATABASE_ENV`], then [`LEGACY_RESEARCH_DATABASE_ENV`].
/// `lookup` is injected so callers (and this module's own tests) can
/// resolve deterministically without touching real process environment.
/// `Err` names every variable that was checked and found unset/empty —
/// never a bare "not configured" with no indication which surface an
/// operator should set.
pub fn resolve_domain_database_path(
    explicit: Option<PathBuf>,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<PathBuf, String> {
    fn nonempty(value: String) -> Option<String> {
        (!value.trim().is_empty()).then_some(value)
    }

    explicit
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| {
            lookup(DOMAIN_DATABASE_ENV)
                .and_then(nonempty)
                .map(PathBuf::from)
        })
        .or_else(|| {
            lookup(LEGACY_RESEARCH_DATABASE_ENV)
                .and_then(nonempty)
                .map(PathBuf::from)
        })
        .ok_or_else(|| {
            format!(
                "missing required domain database configuration: set \
                 {DOMAIN_DATABASE_ENV} (preferred) or {LEGACY_RESEARCH_DATABASE_ENV}"
            )
        })
}

/// Open the one shared canonical [`DomainPersistence`] store at the path
/// [`resolve_domain_database_path`] resolves — the single seam an
/// interface should call rather than independently constructing its own
/// `DomainPersistence::open(...)` from a locally invented path or
/// variable name.
pub async fn open_shared_domain_store(
    explicit: Option<PathBuf>,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<DomainPersistence, DomainRuntimeError> {
    let path = resolve_domain_database_path(explicit, lookup)
        .map_err(DomainRuntimeError::NotConfigured)?;
    DomainPersistence::open(&path)
        .await
        .map_err(DomainRuntimeError::Persistence)
}

/// Why [`open_shared_domain_store`] could not produce a handle.
#[derive(Debug)]
pub enum DomainRuntimeError {
    /// No database path could be resolved — see the message for exactly
    /// which variables were checked.
    NotConfigured(String),
    /// The path resolved, but opening the store itself failed.
    Persistence(PersistenceError),
}

impl std::fmt::Display for DomainRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured(message) => write!(f, "{message}"),
            Self::Persistence(error) => write!(f, "opening shared domain store: {error}"),
        }
    }
}

impl std::error::Error for DomainRuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Persistence(error) => Some(error),
            Self::NotConfigured(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn explicit_path_wins_over_every_environment_variable() {
        let resolved = resolve_domain_database_path(
            Some(PathBuf::from("explicit.sqlite")),
            &lookup(&[
                (DOMAIN_DATABASE_ENV, "neutral.sqlite"),
                (LEGACY_RESEARCH_DATABASE_ENV, "legacy.sqlite"),
            ]),
        )
        .unwrap();
        assert_eq!(resolved, PathBuf::from("explicit.sqlite"));
    }

    #[test]
    fn neutral_variable_wins_over_legacy_research_variable() {
        let resolved = resolve_domain_database_path(
            None,
            &lookup(&[
                (DOMAIN_DATABASE_ENV, "neutral.sqlite"),
                (LEGACY_RESEARCH_DATABASE_ENV, "legacy.sqlite"),
            ]),
        )
        .unwrap();
        assert_eq!(resolved, PathBuf::from("neutral.sqlite"));
    }

    // Explicit reconciliation, proven: an operator with only the
    // pre-existing RESEARCH_EVIDENCE_DB set (every real deployment prior
    // to this frontier) keeps working unmodified.
    #[test]
    fn legacy_research_variable_is_honored_when_the_neutral_variable_is_unset() {
        let resolved = resolve_domain_database_path(
            None,
            &lookup(&[(LEGACY_RESEARCH_DATABASE_ENV, "legacy.sqlite")]),
        )
        .unwrap();
        assert_eq!(resolved, PathBuf::from("legacy.sqlite"));
    }

    #[test]
    fn empty_explicit_path_falls_through_to_environment() {
        let resolved = resolve_domain_database_path(
            Some(PathBuf::new()),
            &lookup(&[(DOMAIN_DATABASE_ENV, "neutral.sqlite")]),
        )
        .unwrap();
        assert_eq!(resolved, PathBuf::from("neutral.sqlite"));
    }

    #[test]
    fn nothing_configured_fails_closed_naming_both_variables() {
        let error = resolve_domain_database_path(None, &lookup(&[])).unwrap_err();
        assert!(error.contains(DOMAIN_DATABASE_ENV));
        assert!(error.contains(LEGACY_RESEARCH_DATABASE_ENV));
    }

    #[test]
    fn whitespace_only_values_are_treated_as_unset() {
        let resolved = resolve_domain_database_path(
            None,
            &lookup(&[
                (DOMAIN_DATABASE_ENV, "   "),
                (LEGACY_RESEARCH_DATABASE_ENV, "legacy.sqlite"),
            ]),
        )
        .unwrap();
        assert_eq!(resolved, PathBuf::from("legacy.sqlite"));
    }

    #[tokio::test]
    async fn open_shared_domain_store_opens_a_real_handle() {
        let path = std::env::temp_dir().join(format!(
            "scorpion-domain-runtime-test-{}-{}.sqlite3",
            std::process::id(),
            crate::features::identity::EvidenceId::new()
        ));
        let _ = std::fs::remove_file(&path);

        let store = open_shared_domain_store(Some(path.clone()), &|_| None)
            .await
            .unwrap();
        store
            .write_current("watch_domain_runtime_test", None, b"ok")
            .await
            .unwrap();
        drop(store);
        assert!(path.exists());

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn open_shared_domain_store_fails_closed_when_unconfigured() {
        let result = open_shared_domain_store(None, &|_| None).await;
        assert!(matches!(result, Err(DomainRuntimeError::NotConfigured(_))));
    }
}
