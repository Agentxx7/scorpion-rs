//! Canonical scheduling semantics for [`crate::features::watch::WatchDefinition`]
//! and the execution path for one scheduled watch run.
//!
//! Track 8 of the roadmap — the frontier the closed Track 7
//! (`SCORPION_CANONICAL_WATCH_MODEL_001`) left as its own explicit
//! successor boundary: *"a scheduler deciding when a watch is checked...
//! remain later, separate frontiers."* This module is that frontier, and
//! no further: it does not implement change detection
//! (`ChangeResult`/`ChangeEvent`), health, notifications, or a generic
//! `Job`/`Operation` model. It does not run a background scheduling
//! daemon — it defines what happens *when a scheduled trigger fires*,
//! not what decides *when that trigger fires*.
//!
//! # Cadence ownership (source-justified, not `CronType`)
//!
//! `spider::website::CronType` is a *what-to-run* selector
//! (`Crawl`/`Scrape`) for `Website`'s own existing cron integration — it
//! carries no cadence syntax at all, so it cannot be reused here. The
//! actual cadence primitive `Website`'s cron feature already depends on
//! is `async_job::Schedule` (`Website::schedule()`, gated behind the
//! `cron` feature, parses `Configuration::cron_str` via
//! `cron_str.parse::<async_job::Schedule>()`). [`WatchSchedule`] reuses
//! that exact primitive to validate cadence syntax — it does not
//! reimplement cron parsing, and it does not adopt `async_job::Job`/
//! `async_job::Runner` (a `Website`-owned, always-running scheduler
//! daemon abstraction): a `WatchSchedule` is validated, durable data, not
//! a running process. This is the "adapt an existing primitive cleanly
//! without inventing a second scheduler abstraction" rule applied
//! precisely: adopt the *parser*, not the *daemon*.
//!
//! # Scheduling never owns WatchState
//!
//! This module persists exactly one thing of its own — [`WatchSchedule`]
//! (cadence only) — and otherwise **delegates entirely** to Track 7's
//! existing seam: [`execute_scheduled_watch_run`] calls
//! [`crate::features::watch::read_watch_definition`] to read (never
//! redefine) the watch's target, and
//! [`crate::features::watch::apply_watch_transition`] with Track 7's own
//! [`crate::features::watch::ObserveEvidence`] to record the outcome.
//! `WatchState`'s variants, transitions, and persistence rules are not
//! touched, extended, or shadowed here.
//!
//! # Execution path
//!
//! ```text
//! WatchDefinition (Track 7, read-only)
//!       │
//!       ▼
//! scheduled trigger (id, scheduled_at — supplied by the caller; this
//!                     module does not decide when to fire)
//!       │
//!       ▼
//! canonical acquisition (features::acquisition_binding::bind + execute
//!                         — the same seam CLI/MCP fetch already uses;
//!                         no second fetch/crawl/transport architecture)
//!       │
//!       ▼
//! durable EvidenceRef (utils::evidence::build_evidence + record_evidence
//!                       — Track 4's ledger, unmodified)
//!       │
//!       ▼
//! WatchState transition (features::watch::apply_watch_transition with
//!                         ObserveEvidence — Track 7's own seam, unmodified)
//! ```
//!
//! # Idempotency
//!
//! "The same scheduled run" is identified by `(WatchId, scheduled_at)`
//! — a caller retrying an execution attempt for the exact tick that
//! already ran must not duplicate the fetch, the durable evidence
//! record, or the `WatchState` transition. This is enforced by claiming
//! the run's identity *before* any side effect: [`execute_scheduled_watch_run`]
//! first writes a `Claimed` marker via
//! [`DomainPersistence::write_current`]'s compare-and-swap
//! (`expected_revision: None` — a genuine first write for this exact
//! run). Only the caller that wins that claim performs acquisition,
//! records evidence, and applies the watch transition; it then finalizes
//! the same run identity to `Completed { evidence }` via a second
//! compare-and-swap. A caller that loses the claim (the run identity was
//! already written) never touches acquisition, evidence, or `WatchState`
//! at all — it reads what is already there: a `Completed` record is
//! replayed (the already-produced `EvidenceRef` is returned, no new work
//! performed); a `Claimed`-but-not-yet-`Completed` record (a concurrent
//! or crashed prior attempt) is rejected as
//! [`WatchExecutionError::RunAlreadyInProgress`] — fail closed rather
//! than guessing whether it is safe to duplicate the in-flight work.
//!
//! # Persistence
//!
//! Reuses [`DomainPersistence`] exclusively — no second persistence
//! mechanism. `WatchSchedule` is immutable, write-once, persisted via
//! `append_history` at a namespaced key (`"<id>#schedule"`), exactly
//! like Track 7's own `WatchDefinition`. Each scheduled run's claim/
//! completion record is persisted via `write_current`'s compare-and-swap
//! at its own namespaced key (`"<id>#run#<unix_seconds>"`) — durable
//! scheduling state is kept to exactly what idempotency actually
//! requires; no generic `Job`/`Task`/`Operation` table, no execution
//! history log beyond the one current claim/completion record per run
//! identity (Track 7's own `HistoryLog`/`append_history` already
//! preserves every superseded `WatchState`, so this module does not
//! duplicate that).
//!
//! [`DomainPersistence::write_current`]: crate::features::domain_persistence::DomainPersistence::write_current

use crate::features::acquisition_binding::{self, AcquisitionBindingError};
use crate::features::domain_persistence::{DomainPersistence, PersistenceError};
use crate::features::identity::WatchId;
use crate::features::transport::TransportRequest;
use crate::features::watch::{self, ObserveEvidence, WatchError};
use crate::utils::evidence::{build_evidence, record_evidence, EvidenceLedgerError, EvidenceRef};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::SystemTime;

/// A watch's durable cadence — cron syntax only, validated (not
/// reinterpreted) via the same primitive `Website`'s own cron feature
/// already depends on. Carries no lifecycle/target state of its own; see
/// this module's doc comment for why it never owns `WatchState`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchSchedule {
    /// Cadence syntax, `async_job::Schedule`-parseable (6-field cron:
    /// seconds minutes hours day-of-month month day-of-week).
    pub cron_str: String,
}

/// Why a [`WatchSchedule`] could not be defined or read.
#[derive(Debug)]
pub enum WatchScheduleError {
    /// `cron_str` did not parse as valid cadence syntax — fail closed,
    /// nothing was persisted. Carries the parser's own diagnostic
    /// (cadence syntax only; never secret material).
    InvalidCadence(String),
    /// A backend/persistence failure.
    Persistence(PersistenceError),
    /// The schedule could not be encoded/decoded.
    Serialization(serde_json::Error),
}

impl fmt::Display for WatchScheduleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WatchScheduleError::InvalidCadence(detail) => {
                write!(f, "invalid watch schedule cadence: {detail}")
            }
            WatchScheduleError::Persistence(error) => write!(f, "watch schedule ledger: {error}"),
            WatchScheduleError::Serialization(error) => {
                write!(f, "watch schedule ledger: serialization failed: {error}")
            }
        }
    }
}

impl std::error::Error for WatchScheduleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WatchScheduleError::Persistence(error) => Some(error),
            WatchScheduleError::Serialization(error) => Some(error),
            WatchScheduleError::InvalidCadence(_) => None,
        }
    }
}

/// Why a scheduled watch run could not be executed. Storage/domain-shaped
/// only; every acquisition/evidence/transition failure delegates to its
/// existing canonical error type rather than re-deriving one.
#[derive(Debug)]
pub enum WatchExecutionError {
    /// No [`WatchSchedule`] is recorded for this `WatchId` — a scheduled
    /// trigger fired for a watch that was never scheduled.
    NoSchedule,
    /// No [`crate::features::watch::WatchDefinition`] is recorded for
    /// this `WatchId`.
    WatchNotFound,
    /// This exact `(WatchId, scheduled_at)` run was already claimed by
    /// another execution attempt that has not yet completed (or crashed
    /// mid-flight). Fail closed rather than risk duplicating the
    /// in-flight fetch/evidence/transition.
    RunAlreadyInProgress,
    /// Failure reading the watch's schedule.
    Schedule(WatchScheduleError),
    /// Failure reading the watch's definition.
    Watch(WatchError),
    /// The target could not be bound to acquisition intent.
    Acquisition(AcquisitionBindingError),
    /// The canonical acquisition seam itself failed before a page could
    /// be evaluated.
    Fetch(String),
    /// Failure durably recording the acquired evidence.
    Evidence(EvidenceLedgerError),
    /// Failure applying the resulting `WatchState` transition.
    Transition(WatchError),
    /// A backend/persistence failure unrelated to the above.
    Persistence(PersistenceError),
    /// The run's claim/completion record could not be encoded/decoded.
    Serialization(serde_json::Error),
}

impl fmt::Display for WatchExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WatchExecutionError::NoSchedule => {
                write!(f, "no schedule recorded for this watch")
            }
            WatchExecutionError::WatchNotFound => {
                write!(f, "no watch recorded for this identity")
            }
            WatchExecutionError::RunAlreadyInProgress => write!(
                f,
                "this scheduled run is already claimed by another execution attempt"
            ),
            WatchExecutionError::Schedule(error) => write!(f, "{error}"),
            WatchExecutionError::Watch(error) => write!(f, "{error}"),
            WatchExecutionError::Acquisition(error) => write!(f, "{error}"),
            WatchExecutionError::Fetch(error) => write!(f, "watch acquisition failed: {error}"),
            WatchExecutionError::Evidence(error) => write!(f, "{error}"),
            WatchExecutionError::Transition(error) => write!(f, "{error}"),
            WatchExecutionError::Persistence(error) => write!(f, "watch execution ledger: {error}"),
            WatchExecutionError::Serialization(error) => {
                write!(f, "watch execution ledger: serialization failed: {error}")
            }
        }
    }
}

impl std::error::Error for WatchExecutionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WatchExecutionError::Schedule(error) => Some(error),
            WatchExecutionError::Watch(error) => Some(error),
            WatchExecutionError::Acquisition(error) => Some(error),
            WatchExecutionError::Evidence(error) => Some(error),
            WatchExecutionError::Transition(error) => Some(error),
            WatchExecutionError::Persistence(error) => Some(error),
            WatchExecutionError::Serialization(error) => Some(error),
            WatchExecutionError::NoSchedule
            | WatchExecutionError::WatchNotFound
            | WatchExecutionError::RunAlreadyInProgress
            | WatchExecutionError::Fetch(_) => None,
        }
    }
}

/// One scheduled run's durable claim/completion record. Never exposed —
/// purely this module's own idempotency bookkeeping, distinct from both
/// `WatchState`'s historical records (Track 7) and the durable evidence
/// ledger (Track 4).
#[derive(Debug, Clone, Serialize, Deserialize)]
enum ScheduledRunRecord {
    /// A run identity has been claimed; acquisition/evidence/transition
    /// have not (yet, or ever) completed for it.
    Claimed,
    /// The run completed; this is the evidence it produced.
    Completed {
        /// The durable evidence this run produced.
        evidence: EvidenceRef,
    },
}

/// The persistence key a [`WatchSchedule`] uses — namespaced distinctly
/// from `WatchDefinition`'s own `"<id>#definition"` key, `WatchState`'s
/// plain `id.to_string()` key, and every run's own `"<id>#run#..."` key.
fn schedule_key(id: WatchId) -> String {
    format!("{id}#schedule")
}

/// The persistence key one scheduled run's claim/completion record uses —
/// deterministic from `(id, scheduled_at)` so two execution attempts for
/// the exact same logical run always address the exact same record.
fn run_key(id: WatchId, scheduled_at: SystemTime) -> String {
    let scheduled_at_unix_seconds = scheduled_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    format!("{id}#run#{scheduled_at_unix_seconds}")
}

/// Durably record `cron_str` as `id`'s schedule — immutable, write-once,
/// exactly like Track 7's own `WatchDefinition`. Fails closed
/// (`InvalidCadence`, nothing persisted) if `cron_str` does not parse as
/// valid cadence syntax.
pub async fn define_watch_schedule(
    store: &DomainPersistence,
    id: WatchId,
    cron_str: &str,
) -> Result<WatchSchedule, WatchScheduleError> {
    cron_str
        .parse::<async_job::Schedule>()
        .map_err(|error| WatchScheduleError::InvalidCadence(format!("{error:?}")))?;

    let schedule = WatchSchedule {
        cron_str: cron_str.to_string(),
    };
    let payload = serde_json::to_vec(&schedule).map_err(WatchScheduleError::Serialization)?;
    store
        .append_history(&schedule_key(id), 1, &payload, SystemTime::now())
        .await
        .map_err(WatchScheduleError::Persistence)?;

    Ok(schedule)
}

/// Read the durable schedule of `id`. `Ok(None)` if no schedule was ever
/// defined for this identity.
pub async fn read_watch_schedule(
    store: &DomainPersistence,
    id: WatchId,
) -> Result<Option<WatchSchedule>, WatchScheduleError> {
    let history = store
        .read_history(&schedule_key(id))
        .await
        .map_err(WatchScheduleError::Persistence)?;

    match history.into_iter().next() {
        Some((_revision, payload, _recorded_at)) => {
            let schedule =
                serde_json::from_slice(&payload).map_err(WatchScheduleError::Serialization)?;
            Ok(Some(schedule))
        }
        None => Ok(None),
    }
}

/// Execute one scheduled run of `id`, triggered for `scheduled_at`, over
/// `transport_request`. See this module's doc comment for the full
/// execution path and idempotency contract. Fails closed if `id` has no
/// recorded schedule or no recorded definition; performs no acquisition,
/// evidence recording, or `WatchState` transition in either case.
pub async fn execute_scheduled_watch_run(
    store: &DomainPersistence,
    id: WatchId,
    scheduled_at: SystemTime,
    transport_request: TransportRequest,
) -> Result<EvidenceRef, WatchExecutionError> {
    read_watch_schedule(store, id)
        .await
        .map_err(WatchExecutionError::Schedule)?
        .ok_or(WatchExecutionError::NoSchedule)?;

    let definition = watch::read_watch_definition(store, id)
        .await
        .map_err(WatchExecutionError::Watch)?
        .ok_or(WatchExecutionError::WatchNotFound)?;

    let run_key = run_key(id, scheduled_at);
    let claim_payload = serde_json::to_vec(&ScheduledRunRecord::Claimed)
        .map_err(WatchExecutionError::Serialization)?;
    let claim_revision = match store.write_current(&run_key, None, &claim_payload).await {
        Ok(revision) => revision,
        Err(PersistenceError::CurrentStateConflict { .. }) => {
            let (_, existing) = store
                .read_current(&run_key)
                .await
                .map_err(WatchExecutionError::Persistence)?
                .expect("a CurrentStateConflict on this key implies a row already exists");
            let record: ScheduledRunRecord =
                serde_json::from_slice(&existing).map_err(WatchExecutionError::Serialization)?;
            return match record {
                ScheduledRunRecord::Completed { evidence } => Ok(evidence),
                ScheduledRunRecord::Claimed => Err(WatchExecutionError::RunAlreadyInProgress),
            };
        }
        Err(other) => return Err(WatchExecutionError::Persistence(other)),
    };

    // Canonical acquisition only — the same bind/execute seam
    // CLI/MCP fetch already uses. No new fetch/crawl/transport path.
    let binding = acquisition_binding::bind(&definition.target, transport_request)
        .map_err(WatchExecutionError::Acquisition)?;
    let acquisition = acquisition_binding::execute(binding)
        .await
        .map_err(WatchExecutionError::Fetch)?;

    let page = acquisition.page();
    let content = page
        .get_bytes()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .map(str::to_string);
    let bundle = build_evidence(page, content, false, false);
    let recorded = record_evidence(store, bundle)
        .await
        .map_err(WatchExecutionError::Evidence)?;
    let evidence_ref = EvidenceRef::new(
        recorded
            .id
            .expect("record_evidence always populates id on success"),
    );

    // WatchState transition — Track 7's own seam, unmodified.
    watch::apply_watch_transition(
        store,
        id,
        &ObserveEvidence {
            evidence: evidence_ref,
        },
    )
    .await
    .map_err(WatchExecutionError::Transition)?;

    let completed_payload = serde_json::to_vec(&ScheduledRunRecord::Completed {
        evidence: evidence_ref,
    })
    .map_err(WatchExecutionError::Serialization)?;
    store
        .write_current(&run_key, Some(claim_revision), &completed_payload)
        .await
        .map_err(WatchExecutionError::Persistence)?;

    Ok(evidence_ref)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::discovery_target::{DiscoveryTarget, DiscoveryTargetKind};
    use crate::features::transport::TransportMode;
    use crate::features::watch::{define_watch, read_current_watch_state, WatchState};
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn requested_target(url: &str) -> DiscoveryTarget {
        DiscoveryTarget {
            url: url.to_string(),
            kind: DiscoveryTargetKind::Requested,
            discovered_via: None,
        }
    }

    fn default_transport() -> TransportRequest {
        TransportRequest {
            mode: TransportMode::Default,
            proxy: None,
        }
    }

    struct HttpFixture {
        addr: SocketAddr,
        hits: Arc<AtomicUsize>,
    }

    impl HttpFixture {
        async fn start(body: &'static [u8]) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let hits = Arc::new(AtomicUsize::new(0));
            let hits_clone = hits.clone();
            tokio::spawn(async move {
                loop {
                    let (mut stream, _) = match listener.accept().await {
                        Ok(pair) => pair,
                        Err(_) => break,
                    };
                    let hits = hits_clone.clone();
                    tokio::spawn(async move {
                        hits.fetch_add(1, AtomicOrdering::SeqCst);
                        let mut buf = [0_u8; 4096];
                        let _ = stream.read(&mut buf).await;
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.write_all(body).await;
                    });
                }
            });
            Self { addr, hits }
        }

        fn hit_count(&self) -> usize {
            self.hits.load(AtomicOrdering::SeqCst)
        }
    }

    #[tokio::test]
    async fn define_watch_schedule_persists_and_reads_back_truthfully() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target("https://example.test/"))
            .await
            .unwrap();

        let schedule = define_watch_schedule(&store, id, "0 * * * * *")
            .await
            .unwrap();
        assert_eq!(schedule.cron_str, "0 * * * * *");

        let read_back = read_watch_schedule(&store, id).await.unwrap().unwrap();
        assert_eq!(read_back, schedule);
    }

    #[tokio::test]
    async fn invalid_cadence_is_rejected_fail_closed_nothing_persisted() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target("https://example.test/"))
            .await
            .unwrap();

        let error = define_watch_schedule(&store, id, "not a cron string")
            .await
            .unwrap_err();
        assert!(matches!(error, WatchScheduleError::InvalidCadence(_)));
        assert!(read_watch_schedule(&store, id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn schedule_definition_and_state_keys_do_not_collide() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target("https://example.test/"))
            .await
            .unwrap();
        define_watch_schedule(&store, id, "0 * * * * *")
            .await
            .unwrap();

        assert!(read_watch_schedule(&store, id).await.unwrap().is_some());
        assert!(watch::read_watch_definition(&store, id)
            .await
            .unwrap()
            .is_some());
        assert!(read_current_watch_state(&store, id)
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn execute_without_schedule_fails_closed() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target("https://example.test/"))
            .await
            .unwrap();

        let error = execute_scheduled_watch_run(&store, id, SystemTime::now(), default_transport())
            .await
            .unwrap_err();
        assert!(matches!(error, WatchExecutionError::NoSchedule));
    }

    #[tokio::test]
    async fn execute_without_watch_definition_fails_closed() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let id = WatchId::new();
        define_watch_schedule(&store, id, "0 * * * * *")
            .await
            .unwrap();

        let error = execute_scheduled_watch_run(&store, id, SystemTime::now(), default_transport())
            .await
            .unwrap_err();
        assert!(matches!(error, WatchExecutionError::WatchNotFound));
    }

    #[tokio::test]
    async fn end_to_end_execution_reuses_canonical_acquisition_and_transitions_watch() {
        let http = HttpFixture::start(b"scheduled watch fixture body").await;
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target(&format!("http://{}/", http.addr)))
            .await
            .unwrap();
        define_watch_schedule(&store, id, "0 * * * * *")
            .await
            .unwrap();

        let evidence_ref =
            execute_scheduled_watch_run(&store, id, SystemTime::now(), default_transport())
                .await
                .unwrap();

        assert_eq!(http.hit_count(), 1);

        let bundle = crate::utils::evidence::read_evidence(&store, evidence_ref.id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(bundle.content.unwrap(), "scheduled watch fixture body");

        let (_, state) = read_current_watch_state(&store, id).await.unwrap().unwrap();
        assert!(matches!(state, WatchState::Active { .. }));
        assert_eq!(state.last_evidence(), Some(evidence_ref));
    }

    #[tokio::test]
    async fn retry_of_the_same_scheduled_run_is_idempotent_and_does_not_refetch() {
        let http = HttpFixture::start(b"idempotent fixture body").await;
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target(&format!("http://{}/", http.addr)))
            .await
            .unwrap();
        define_watch_schedule(&store, id, "0 * * * * *")
            .await
            .unwrap();
        let scheduled_at = SystemTime::now();

        let first = execute_scheduled_watch_run(&store, id, scheduled_at, default_transport())
            .await
            .unwrap();
        assert_eq!(http.hit_count(), 1);
        let history_len_after_first = store.read_history(&id.to_string()).await.unwrap().len();

        let second = execute_scheduled_watch_run(&store, id, scheduled_at, default_transport())
            .await
            .unwrap();

        assert_eq!(
            second, first,
            "a retry of the same run must replay the same evidence"
        );
        assert_eq!(http.hit_count(), 1, "a retry must never refetch");
        assert_eq!(
            store.read_history(&id.to_string()).await.unwrap().len(),
            history_len_after_first,
            "a retry must never apply a second WatchState transition"
        );
    }

    #[tokio::test]
    async fn different_scheduled_tick_executes_independently_not_suppressed() {
        let http = HttpFixture::start(b"distinct tick fixture body").await;
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target(&format!("http://{}/", http.addr)))
            .await
            .unwrap();
        define_watch_schedule(&store, id, "0 * * * * *")
            .await
            .unwrap();
        let first_tick = SystemTime::now();
        let second_tick = first_tick + Duration::from_secs(60);

        let first = execute_scheduled_watch_run(&store, id, first_tick, default_transport())
            .await
            .unwrap();
        let second = execute_scheduled_watch_run(&store, id, second_tick, default_transport())
            .await
            .unwrap();

        assert_ne!(
            first, second,
            "distinct logical runs produce distinct evidence"
        );
        assert_eq!(
            http.hit_count(),
            2,
            "idempotency must be scoped to one exact run, not global"
        );
    }

    #[tokio::test]
    async fn a_claimed_but_incomplete_run_fails_closed_without_duplicating_work() {
        let http = HttpFixture::start(b"never reached").await;
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target(&format!("http://{}/", http.addr)))
            .await
            .unwrap();
        define_watch_schedule(&store, id, "0 * * * * *")
            .await
            .unwrap();
        let scheduled_at = SystemTime::now();

        // Simulate a prior execution attempt that claimed this exact run
        // and then crashed before completing it.
        let claim_payload = serde_json::to_vec(&ScheduledRunRecord::Claimed).unwrap();
        store
            .write_current(&run_key(id, scheduled_at), None, &claim_payload)
            .await
            .unwrap();

        let error = execute_scheduled_watch_run(&store, id, scheduled_at, default_transport())
            .await
            .unwrap_err();

        assert!(matches!(error, WatchExecutionError::RunAlreadyInProgress));
        assert_eq!(
            http.hit_count(),
            0,
            "a fail-closed rejection must never fetch"
        );
        let (revision, state) = read_current_watch_state(&store, id).await.unwrap().unwrap();
        assert_eq!(revision, 1, "no WatchState transition occurred");
        assert!(matches!(
            state,
            WatchState::Active {
                last_evidence: None
            }
        ));
    }
}
