//! Canonical, purely observational health for the complete watch
//! pipeline: `WatchDefinition → Scheduling → canonical acquisition →
//! EvidenceRef → WatchState → ChangeResult/ChangeEvent`.
//!
//! Track 10 of the roadmap. Health reports what is actually durably
//! known — it never infers success from configuration alone (a
//! `WatchSchedule` existing proves cadence was validated, not that a run
//! ever executed; a `WatchDefinition` existing proves a target was
//! declared, not that anything was ever fetched). Every
//! [`HealthStatus`]/[`ChangeDetectionReadiness`] value returned by
//! [`assess_watch_health`] is derived by reading durable state Tracks
//! 3/4/7/8/9 already produced — this module owns none of it.
//!
//! # Ownership boundary — observational only
//!
//! [`assess_watch_health`] calls only read functions:
//! [`crate::features::watch::read_watch_definition`],
//! [`crate::features::watch::read_current_watch_state`],
//! [`DomainPersistence::read_history`],
//! [`crate::features::watch_schedule::read_watch_schedule`],
//! [`crate::utils::evidence::EvidenceRef::resolve`], and
//! [`crate::features::change_detection::read_change_event`]. It never
//! calls `apply_watch_transition`, `execute_scheduled_watch_run`,
//! `define_watch_schedule`, or `detect_and_record_change` — this module
//! does not own scheduling, `WatchState` transitions, change
//! computation, acquisition, or retries; it only observes what those
//! canonical owners have already durably recorded.
//!
//! # Health vocabulary (source-justified per dimension)
//!
//! [`HealthStatus`] has four variants —
//! [`HealthStatus::Unknown`]/[`HealthStatus::Healthy`]/
//! [`HealthStatus::Degraded`]/[`HealthStatus::Failed`] — but not every
//! pipeline dimension can *truthfully* reach every variant purely
//! observationally, and this module never forces an unreachable state
//! "for symmetry":
//!
//! - **Scheduling** — `Unknown` (no [`crate::features::watch_schedule::WatchSchedule`]
//!   recorded yet) or `Healthy` (one is recorded; Track 8's own
//!   `define_watch_schedule` already validates cadence syntax fail-closed
//!   at definition time, so a recorded schedule is never itself
//!   malformed). `Degraded`/`Failed` are not reachable here: there is no
//!   durable, purely-observational signal for "the schedule looks wrong"
//!   beyond the validation Track 8 already performs before persisting it.
//! - **Watch execution** — `Unknown` (no `ObserveEvidence` transition has
//!   ever occurred for this watch) or `Healthy` (at least one has).
//!   `Degraded`/`Failed` are deliberately not derived from Track 8's
//!   internal, private per-run claim bookkeeping
//!   (`features::watch_schedule`'s `ScheduledRunRecord`) — reaching into
//!   that would blur exactly the ownership boundary this module must
//!   respect; a failed execution attempt that never completes a
//!   transition simply leaves this dimension at `Unknown`, which is
//!   truthful (execution has not yet succeeded), not a fabricated
//!   "Failed".
//! - **Evidence production** — the one dimension where all four
//!   variants are genuinely reachable: `Unknown` (no evidence ever
//!   observed), `Healthy` (the current evidence and every historical
//!   evidence value resolve through the durable ledger),
//!   `Degraded` (the current evidence resolves, but at least one
//!   superseded historical value does not — a real, discoverable
//!   integrity gap in the watch's own past, not a guess), `Failed` (the
//!   *current* evidence does not resolve at all — the watch's own
//!   `WatchState` claims evidence that the durable ledger cannot
//!   produce).
//! - **Change detection** — see "Type-level ready vs. production
//!   exercised" below; this dimension is reported as
//!   [`ChangeDetectionReadiness`], not a bare `HealthStatus`, precisely
//!   because collapsing it into one status would risk exactly the
//!   conflation rule #4 forbids.
//!
//! # Type-level ready vs. production exercised (rule #4)
//!
//! Change-detection *logic* (Track 9) is closed and always valid the
//! moment this module is compiled in — that is a static, type-level
//! fact, true regardless of any particular watch's history. Whether a
//! *real* comparison has ever actually been recorded for a *specific*
//! watch is a separate, per-watch, runtime fact. [`ChangeDetectionReadiness`]
//! makes the two impossible to conflate by construction — they are
//! different enum variants, not different values of the same field:
//!
//! - [`ChangeDetectionReadiness::TypeLevelReady`] — the capability
//!   exists and would work, but either fewer than two evidence
//!   observations exist for this watch (nothing to compare yet) or two
//!   or more exist and no [`crate::features::change_detection::ChangeEvent`]
//!   has actually been durably recorded for the most recent consecutive
//!   pair.
//! - [`ChangeDetectionReadiness::ProductionExercised`] — a real
//!   `ChangeEvent` was found, durably recorded, for the most recent
//!   consecutive evidence pair. Only reachable by finding that record —
//!   never inferred from "a schedule exists" or "evidence exists" alone.
//!
//! Checking "was this exact pair ever compared" reuses
//! [`crate::features::change_detection::ChangeEventId::derive`] (made
//! `pub(crate)` by this frontier so this read-only check never needs to
//! recompute or duplicate the comparison itself — see that module's own
//! doc for the reasoning) plus
//! [`crate::features::change_detection::read_change_event`] — this
//! module never calls `detect_and_record_change`, so it can never
//! *cause* production exercise to become true; it can only observe
//! whether it already is.
//!
//! # No duplication (rule #5)
//!
//! [`WatchHealthReport`] never embeds a `WatchState`, an
//! `EvidenceBundle`, or a `ChangeEvent`. It references the most recent
//! recorded comparison, when one exists, by [`ChangeEventId`] only — a
//! caller who needs the full record reads it back through
//! [`crate::features::change_detection::read_change_event`], exactly the
//! same "hold a reference, resolve on demand" discipline every prior
//! track in this roadmap has used for `EvidenceRef`.
//!
//! # Provider-health reconciliation (rule #2)
//!
//! [`crate::features::source_provider::ProviderDescriptor`] was
//! inspected: it is pure declarative metadata (`id`, `display_name`,
//! `capabilities`) with no runtime or health state of its own, and
//! nothing else in this crate defines any `*Health`/`*Status` type for
//! providers or sources. There is therefore no existing type this
//! module's `HealthStatus` could meaningfully extend or wrap without
//! forcing an unrelated shape onto watch-pipeline health; `HealthStatus`
//! is not routed through `ProviderDescriptor` for that reason. What *is*
//! honored is the same design discipline `ProviderDescriptor` itself
//! follows — pure, declarative, execution-free — and no second
//! provider-health architecture is introduced: `ProviderDescriptor`/
//! `ProviderCapabilities` remain untouched, unmodified, and the sole
//! capability-declaration vocabulary for source providers.

use crate::features::change_detection::{self, ChangeDetectionError, ChangeEventId, ChangeResult};
use crate::features::domain_persistence::{DomainPersistence, PersistenceError};
use crate::features::identity::WatchId;
use crate::features::watch::{self, WatchError, WatchState};
use crate::features::watch_schedule::{self, WatchScheduleError};
use crate::utils::evidence::{EvidenceLedgerError, EvidenceRef};
use std::fmt;

/// Truthful, per-dimension operational status. Not every dimension can
/// reach every variant purely observationally — see this module's doc
/// comment for exactly which variants each dimension can truthfully
/// produce, and why the rest are never forced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Not yet exercised — no durable fact exists to assess this
    /// dimension at all. Never a guess; never defaulted to `Healthy`.
    Unknown,
    /// Verified working, from durable, already-recorded fact.
    Healthy,
    /// Working, but a real, discoverable gap exists (e.g. a historical
    /// evidence record has gone missing while the current one is
    /// intact).
    Degraded,
    /// The most recent durable fact for this dimension is itself
    /// unusable (e.g. the current evidence does not resolve).
    Failed,
}

/// Change-detection readiness — kept structurally distinct from
/// [`HealthStatus`] so type-level readiness and production exercise can
/// never be reported as the same thing. See this module's doc comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeDetectionReadiness {
    /// The comparison logic exists and would work, but no real
    /// [`crate::features::change_detection::ChangeEvent`] has actually
    /// been durably recorded for this watch's most recent consecutive
    /// evidence pair (including the case where fewer than two evidence
    /// observations exist at all).
    TypeLevelReady,
    /// A real comparison was found, durably recorded, for this watch's
    /// most recent consecutive evidence pair.
    ProductionExercised {
        /// `Healthy` when the recorded comparison produced a definite
        /// verdict (`Changed`/`Unchanged`); `Degraded` when it produced
        /// `Uncomparable` — the pipeline ran, but evidence quality could
        /// not support a definite answer. Never `Unknown` (a record was
        /// found) or `Failed` (nothing in `ChangeResult` represents a
        /// comparison failure distinct from `Uncomparable`).
        status: HealthStatus,
        /// The exact recorded comparison this readiness was derived
        /// from — a reference only, never a duplicated `ChangeEvent`.
        most_recent_change_event: ChangeEventId,
    },
}

/// The complete, purely observational health report for one watch.
/// Every field is independently derived — see this module's doc comment
/// for exactly what each one means and why.
#[derive(Debug, Clone)]
pub struct WatchHealthReport {
    watch: WatchId,
    scheduling: HealthStatus,
    execution: HealthStatus,
    evidence: HealthStatus,
    change_detection: ChangeDetectionReadiness,
}

impl WatchHealthReport {
    /// The watch this report describes.
    pub fn watch(&self) -> WatchId {
        self.watch
    }

    /// Whether a valid cadence is durably recorded for this watch.
    pub fn scheduling(&self) -> HealthStatus {
        self.scheduling
    }

    /// Whether canonical acquisition has ever successfully completed and
    /// transitioned this watch's state at least once.
    pub fn execution(&self) -> HealthStatus {
        self.execution
    }

    /// Whether this watch's current (and historical) evidence durably
    /// resolves.
    pub fn evidence(&self) -> HealthStatus {
        self.evidence
    }

    /// Type-level-ready vs. production-exercised change detection —
    /// never conflated.
    pub fn change_detection(&self) -> &ChangeDetectionReadiness {
        &self.change_detection
    }
}

/// Why a watch's health could not be assessed.
#[derive(Debug)]
pub enum WatchHealthError {
    /// No `WatchDefinition` is recorded for this identity — there is
    /// nothing to assess.
    WatchNotFound,
    /// Failure reading watch state/history.
    Watch(WatchError),
    /// Failure reading the watch's schedule.
    Schedule(WatchScheduleError),
    /// Failure resolving an `EvidenceRef`.
    Evidence(EvidenceLedgerError),
    /// Failure reading a durable change event.
    ChangeDetection(ChangeDetectionError),
    /// A backend/persistence failure unrelated to the above.
    Persistence(PersistenceError),
    /// A watch-history entry could not be decoded.
    Serialization(serde_json::Error),
}

impl fmt::Display for WatchHealthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WatchHealthError::WatchNotFound => {
                write!(f, "no watch recorded for this identity")
            }
            WatchHealthError::Watch(error) => write!(f, "{error}"),
            WatchHealthError::Schedule(error) => write!(f, "{error}"),
            WatchHealthError::Evidence(error) => write!(f, "{error}"),
            WatchHealthError::ChangeDetection(error) => write!(f, "{error}"),
            WatchHealthError::Persistence(error) => write!(f, "watch health: {error}"),
            WatchHealthError::Serialization(error) => {
                write!(f, "watch health: watch history decode failed: {error}")
            }
        }
    }
}

impl std::error::Error for WatchHealthError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WatchHealthError::Watch(error) => Some(error),
            WatchHealthError::Schedule(error) => Some(error),
            WatchHealthError::Evidence(error) => Some(error),
            WatchHealthError::ChangeDetection(error) => Some(error),
            WatchHealthError::Persistence(error) => Some(error),
            WatchHealthError::Serialization(error) => Some(error),
            WatchHealthError::WatchNotFound => None,
        }
    }
}

/// Every `EvidenceRef` this watch has ever observed, oldest first,
/// ending with the current value (if any). Read-only: every superseded
/// historical `WatchState` (`DomainPersistence::read_history`, Track
/// 7's own append-only records) followed by the current `WatchState`
/// (`watch::read_current_watch_state`) — never a new index, never a
/// second history of its own.
async fn ordered_watch_evidence(
    store: &DomainPersistence,
    watch_id: WatchId,
) -> Result<Vec<EvidenceRef>, WatchHealthError> {
    let mut ordered = Vec::new();

    let history = store
        .read_history(&watch_id.to_string())
        .await
        .map_err(WatchHealthError::Persistence)?;
    for (_revision, payload, _recorded_at) in history {
        let state: WatchState =
            serde_json::from_slice(&payload).map_err(WatchHealthError::Serialization)?;
        ordered.extend(state.last_evidence());
    }

    if let Some((_revision, state)) = watch::read_current_watch_state(store, watch_id)
        .await
        .map_err(WatchHealthError::Watch)?
    {
        ordered.extend(state.last_evidence());
    }

    Ok(ordered)
}

fn assess_execution(ordered_evidence: &[EvidenceRef]) -> HealthStatus {
    if ordered_evidence.is_empty() {
        HealthStatus::Unknown
    } else {
        HealthStatus::Healthy
    }
}

async fn assess_scheduling(
    store: &DomainPersistence,
    watch_id: WatchId,
) -> Result<HealthStatus, WatchHealthError> {
    match watch_schedule::read_watch_schedule(store, watch_id)
        .await
        .map_err(WatchHealthError::Schedule)?
    {
        Some(_) => Ok(HealthStatus::Healthy),
        None => Ok(HealthStatus::Unknown),
    }
}

async fn assess_evidence(
    store: &DomainPersistence,
    ordered_evidence: &[EvidenceRef],
) -> Result<HealthStatus, WatchHealthError> {
    let Some((current, historical)) = ordered_evidence.split_last() else {
        return Ok(HealthStatus::Unknown);
    };

    let current_resolves = current
        .resolve(store)
        .await
        .map_err(WatchHealthError::Evidence)?
        .is_some();
    if !current_resolves {
        return Ok(HealthStatus::Failed);
    }

    for evidence in historical {
        let resolves = evidence
            .resolve(store)
            .await
            .map_err(WatchHealthError::Evidence)?
            .is_some();
        if !resolves {
            return Ok(HealthStatus::Degraded);
        }
    }

    Ok(HealthStatus::Healthy)
}

async fn assess_change_detection(
    store: &DomainPersistence,
    watch_id: WatchId,
    ordered_evidence: &[EvidenceRef],
) -> Result<ChangeDetectionReadiness, WatchHealthError> {
    if ordered_evidence.len() < 2 {
        return Ok(ChangeDetectionReadiness::TypeLevelReady);
    }
    let previous_evidence = ordered_evidence[ordered_evidence.len() - 2];
    let current_evidence = ordered_evidence[ordered_evidence.len() - 1];
    let id = ChangeEventId::derive(watch_id, previous_evidence, current_evidence);

    match change_detection::read_change_event(store, &id)
        .await
        .map_err(WatchHealthError::ChangeDetection)?
    {
        Some(event) => {
            let status = match event.result() {
                ChangeResult::Changed { .. } | ChangeResult::Unchanged { .. } => {
                    HealthStatus::Healthy
                }
                ChangeResult::Uncomparable { .. } => HealthStatus::Degraded,
            };
            Ok(ChangeDetectionReadiness::ProductionExercised {
                status,
                most_recent_change_event: id,
            })
        }
        None => Ok(ChangeDetectionReadiness::TypeLevelReady),
    }
}

/// Assess the complete, purely observational health of `watch` across
/// scheduling, execution, evidence production, and change detection. See
/// this module's doc comment for the full contract. Fails closed
/// (`WatchNotFound`) if `watch` was never defined at all.
pub async fn assess_watch_health(
    store: &DomainPersistence,
    watch_id: WatchId,
) -> Result<WatchHealthReport, WatchHealthError> {
    watch::read_watch_definition(store, watch_id)
        .await
        .map_err(WatchHealthError::Watch)?
        .ok_or(WatchHealthError::WatchNotFound)?;

    let scheduling = assess_scheduling(store, watch_id).await?;
    let ordered_evidence = ordered_watch_evidence(store, watch_id).await?;
    let execution = assess_execution(&ordered_evidence);
    let evidence = assess_evidence(store, &ordered_evidence).await?;
    let change_detection = assess_change_detection(store, watch_id, &ordered_evidence).await?;

    Ok(WatchHealthReport {
        watch: watch_id,
        scheduling,
        execution,
        evidence,
        change_detection,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::change_detection::detect_and_record_change;
    use crate::features::discovery_target::{DiscoveryTarget, DiscoveryTargetKind};
    use crate::features::identity::EvidenceId;
    use crate::features::transport::{TransportMode, TransportRequest};
    use crate::features::watch::{apply_watch_transition, define_watch, ObserveEvidence};
    use crate::features::watch_schedule::{define_watch_schedule, execute_scheduled_watch_run};
    use crate::utils::evidence::{record_evidence, EvidenceBundle};
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};
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

    async fn record_bundle(store: &DomainPersistence, hash: &str) -> EvidenceRef {
        let bundle = EvidenceBundle {
            response_body_hash: Some(hash.to_string()),
            ..Default::default()
        };
        let recorded = record_evidence(store, bundle).await.unwrap();
        EvidenceRef::new(recorded.id.unwrap())
    }

    struct HttpFixture {
        addr: SocketAddr,
    }

    impl HttpFixture {
        async fn start(bodies: &'static [&'static [u8]]) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let index = Arc::new(AtomicUsize::new(0));
            tokio::spawn(async move {
                loop {
                    let (mut stream, _) = match listener.accept().await {
                        Ok(pair) => pair,
                        Err(_) => break,
                    };
                    let index = index.clone();
                    tokio::spawn(async move {
                        let mut buf = [0_u8; 4096];
                        let _ = stream.read(&mut buf).await;
                        let served = index.fetch_add(1, AtomicOrdering::SeqCst) % bodies.len();
                        let body = bodies[served];
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.write_all(body).await;
                    });
                }
            });
            Self { addr }
        }
    }

    #[tokio::test]
    async fn assess_watch_health_on_unknown_id_fails_closed() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let error = assess_watch_health(&store, WatchId::new())
            .await
            .unwrap_err();
        assert!(matches!(error, WatchHealthError::WatchNotFound));
    }

    #[tokio::test]
    async fn everything_unknown_for_a_freshly_defined_watch() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target("https://example.test/"))
            .await
            .unwrap();

        let report = assess_watch_health(&store, id).await.unwrap();
        assert_eq!(report.watch(), id);
        assert_eq!(report.scheduling(), HealthStatus::Unknown);
        assert_eq!(report.execution(), HealthStatus::Unknown);
        assert_eq!(report.evidence(), HealthStatus::Unknown);
        assert_eq!(
            *report.change_detection(),
            ChangeDetectionReadiness::TypeLevelReady
        );
    }

    #[tokio::test]
    async fn scheduling_is_healthy_once_a_schedule_is_recorded() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target("https://example.test/"))
            .await
            .unwrap();
        define_watch_schedule(&store, id, "0 * * * * *")
            .await
            .unwrap();

        let report = assess_watch_health(&store, id).await.unwrap();
        assert_eq!(report.scheduling(), HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn execution_and_evidence_are_healthy_after_one_real_scheduled_run() {
        let http = HttpFixture::start(&[b"body"]).await;
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target(&format!("http://{}/", http.addr)))
            .await
            .unwrap();
        define_watch_schedule(&store, id, "0 * * * * *")
            .await
            .unwrap();

        execute_scheduled_watch_run(&store, id, SystemTime::now(), default_transport())
            .await
            .unwrap();

        let report = assess_watch_health(&store, id).await.unwrap();
        assert_eq!(report.execution(), HealthStatus::Healthy);
        assert_eq!(report.evidence(), HealthStatus::Healthy);
        // Only one observation exists — not enough for a comparable pair
        // yet, so change detection remains type-level ready, not
        // production exercised.
        assert_eq!(
            *report.change_detection(),
            ChangeDetectionReadiness::TypeLevelReady
        );
    }

    #[tokio::test]
    async fn change_detection_stays_type_level_ready_until_actually_recorded() {
        let http = HttpFixture::start(&[b"v1", b"v2"]).await;
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target(&format!("http://{}/", http.addr)))
            .await
            .unwrap();
        define_watch_schedule(&store, id, "0 * * * * *")
            .await
            .unwrap();

        let first_tick = SystemTime::now();
        let second_tick = first_tick + Duration::from_secs(60);
        let previous = execute_scheduled_watch_run(&store, id, first_tick, default_transport())
            .await
            .unwrap();
        let current = execute_scheduled_watch_run(&store, id, second_tick, default_transport())
            .await
            .unwrap();

        // Two real evidence observations exist, but no ChangeEvent has
        // actually been recorded yet — production exercise must not be
        // inferred just because the data to exercise it is present.
        let report = assess_watch_health(&store, id).await.unwrap();
        assert_eq!(
            *report.change_detection(),
            ChangeDetectionReadiness::TypeLevelReady
        );

        // Now a real comparison is actually recorded.
        detect_and_record_change(&store, id, previous, current)
            .await
            .unwrap();

        let report = assess_watch_health(&store, id).await.unwrap();
        assert!(matches!(
            *report.change_detection(),
            ChangeDetectionReadiness::ProductionExercised {
                status: HealthStatus::Healthy,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn production_exercised_is_degraded_when_recorded_comparison_is_uncomparable() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target("https://example.test/"))
            .await
            .unwrap();

        // Bundles with no usable hash — Uncomparable.
        let previous_recorded = record_evidence(&store, EvidenceBundle::default())
            .await
            .unwrap();
        let previous = EvidenceRef::new(previous_recorded.id.unwrap());
        apply_watch_transition(&store, id, &ObserveEvidence { evidence: previous })
            .await
            .unwrap();
        let current_recorded = record_evidence(&store, EvidenceBundle::default())
            .await
            .unwrap();
        let current = EvidenceRef::new(current_recorded.id.unwrap());
        apply_watch_transition(&store, id, &ObserveEvidence { evidence: current })
            .await
            .unwrap();

        detect_and_record_change(&store, id, previous, current)
            .await
            .unwrap();

        let report = assess_watch_health(&store, id).await.unwrap();
        assert!(matches!(
            *report.change_detection(),
            ChangeDetectionReadiness::ProductionExercised {
                status: HealthStatus::Degraded,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn evidence_is_failed_when_current_evidence_does_not_resolve() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target("https://example.test/"))
            .await
            .unwrap();

        // A phantom ref — never actually written via record_evidence.
        let phantom = EvidenceRef::new(EvidenceId::new());
        apply_watch_transition(&store, id, &ObserveEvidence { evidence: phantom })
            .await
            .unwrap();

        let report = assess_watch_health(&store, id).await.unwrap();
        assert_eq!(report.evidence(), HealthStatus::Failed);
        // Execution itself is still truthfully "Healthy": a real
        // transition did occur — evidence integrity is a separate
        // concern from whether execution ran at all.
        assert_eq!(report.execution(), HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn evidence_is_degraded_when_only_a_historical_value_is_unresolvable() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target("https://example.test/"))
            .await
            .unwrap();

        let phantom = EvidenceRef::new(EvidenceId::new());
        apply_watch_transition(&store, id, &ObserveEvidence { evidence: phantom })
            .await
            .unwrap();
        let real = record_bundle(&store, "v2").await;
        apply_watch_transition(&store, id, &ObserveEvidence { evidence: real })
            .await
            .unwrap();

        let report = assess_watch_health(&store, id).await.unwrap();
        assert_eq!(report.evidence(), HealthStatus::Degraded);
    }
}
