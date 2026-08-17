//! The canonical Watch model: `WatchDefinition` and `WatchState`, using
//! the existing `WatchId`.
//!
//! Track 7 of the roadmap — the frontier `SCORPION_SDD.md` §5.2 named as
//! the one that must "establish canonical ownership" before WATCH/MONITOR
//! can stop being **BLOCKED**. It realizes exactly the identity and
//! model links of that locked chain —
//!
//! ```text
//! WatchId → WatchDefinition → WatchState → Snapshot → Transition → ...
//! ```
//!
//! — and no further: no scheduler decides *when* a watch is checked, no
//! change-detection product decides *what changed*, no notification
//! system acts on either. Those remain later, separate frontiers.
//!
//! # Target-model reuse decision
//!
//! Rule #2 requires preferring existing target concepts over inventing
//! `WatchTarget`/`WatchSpec`. [`crate::features::discovery_target::DiscoveryTarget`]
//! (`{ url, kind, discovered_via }`) already names exactly "a pointer —
//! a URL that should be acquired later" — precisely what a
//! [`WatchDefinition`] needs to describe. Its own module doc already
//! distinguishes it from `SourceItem` ("a *content candidate*... never a
//! pointer"), which rules `SourceItem` out on its own terms: a watch
//! target is a pointer, not a content candidate. `DiscoveryTargetKind::Requested`
//! ("a caller/request-supplied URL, declared directly — no containing
//! document") already names exactly what a caller defining a new watch
//! supplies. No field `WatchDefinition` needs is missing from
//! `DiscoveryTarget`, so no new target type is introduced —
//! [`WatchDefinition`] wraps a `DiscoveryTarget` directly.
//!
//! # Definition vs. state
//!
//! - [`WatchDefinition`] describes *what* is watched: `{ target:
//!   DiscoveryTarget }`. It owns no execution history and no mutable
//!   lifecycle state — there is no field on it that a transition could
//!   ever apply to, and [`define_watch`] is the only way to create one
//!   (immutable thereafter; persisted once, through
//!   [`DomainPersistence::append_history`], never revised).
//! - [`WatchState`] is the canonical *current lifecycle state* for a
//!   `WatchId`, built entirely on
//!   [`crate::features::domain_state`]'s unmodified contract:
//!   `CurrentState`/`Transition`/`HistoryEntry`/`HistoryLog`. It carries
//!   no target/URL of its own — that would duplicate `WatchDefinition`,
//!   blurring exactly the separation rule #3 requires.
//!
//! # Lifecycle vocabulary (source-justified, not invented for symmetry)
//!
//! Two states only: [`WatchState::Active`] and [`WatchState::Stopped`]
//! (terminal). This is the minimum any watch can have *at all* once
//! scheduling, change detection, health, and notification are all
//! correctly out of scope (this frontier's own DO-NOT list) — a watch
//! that could never be turned off would not have a lifecycle worth
//! representing with Track 2's contract in the first place. Two
//! transitions:
//!
//! - [`ObserveEvidence`] — realizes the locked chain's "Snapshot" step
//!   exactly as `features/domain_state.rs`'s own doc comment already
//!   prescribed for whichever frontier realized it: *"a future watch
//!   frontier realizing §5.2's 'Snapshot' step should define it as a
//!   watch-specific input type to its own `Transition<WatchState>`
//!   impl."* The "input" here is simply an [`EvidenceRef`] — this
//!   frontier does not need a separate named wrapper type (which would
//!   risk re-treading the already-reconciled bare `Observation`/
//!   `Snapshot` names) since the transition struct itself carries the
//!   one field that matters. Only valid while `Active`; updates which
//!   evidence is currently associated with the watch. This is *not*
//!   execution history — it is the single current pointer, exactly the
//!   same shape as `AuthSessionState::Paused`'s single
//!   `BrowserContinuityToken` — the full history of every observation
//!   lives in `HistoryLog`/`DomainPersistence`'s append-only records
//!   instead, once each superseded `WatchState` is captured.
//! - [`StopWatch`] — `Active → Stopped`, preserving whatever evidence was
//!   last observed. Terminal: no transition in this module leads back
//!   out of `Stopped` (a caller who wants to watch again defines a new
//!   `WatchId`, exactly the precedent `AuthSessionState::Invalidated`
//!   already set).
//!
//! # Persistence
//!
//! Both halves reuse [`crate::features::domain_persistence::DomainPersistence`]
//! — never a second persistence mechanism:
//!
//! - [`define_watch`] persists the `WatchDefinition` via
//!   `append_history` at a namespaced key (`"<id>#definition"`, fixed
//!   revision `1`) — immutable, write-once, exactly like Track 4's
//!   evidence ledger — and, separately, the initial `WatchState::Active`
//!   via `write_current` (`expected_revision: None`, a genuine first
//!   write) under the identity's own plain key. These are two separate
//!   `DomainPersistence` calls (Track 3 exposes no cross-call
//!   transaction), the same accepted characteristic
//!   `apply_session_transition` (Track 5) already has for its own
//!   two-call current-state-plus-history sequence.
//! - [`apply_watch_transition`] mirrors
//!   `auth_session::apply_session_transition` exactly: read the current
//!   `(revision, state)`, run it through `CurrentState::apply` (Track 2,
//!   unmodified), write the new current state via `write_current`
//!   (compare-and-swap against the revision just read — a concurrent
//!   writer racing this call is rejected, never silently lost), and
//!   append the just-superseded state via `append_history` (immutable).
//!   No blind overwrite exists anywhere in this path.
//!
//! [`EvidenceRef`]: crate::utils::evidence::EvidenceRef
//! [`DomainPersistence::append_history`]: crate::features::domain_persistence::DomainPersistence::append_history

use crate::features::discovery_target::DiscoveryTarget;
use crate::features::domain_persistence::{DomainPersistence, PersistenceError};
use crate::features::domain_state::{Applied, CurrentState, Transition};
use crate::features::identity::WatchId;
use crate::utils::evidence::EvidenceRef;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::SystemTime;

/// What is watched. Immutable once created — see this module's doc
/// comment for why it wraps the existing `DiscoveryTarget` rather than a
/// new `WatchTarget`/`WatchSpec` type, and why it owns no execution
/// history or mutable lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchDefinition {
    /// The pointer this watch observes.
    pub target: DiscoveryTarget,
}

/// The canonical current lifecycle state for one `WatchId`. See this
/// module's doc comment for why exactly these two variants, and only
/// these two, are source-justified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WatchState {
    /// Currently in effect.
    Active {
        /// The most recently observed evidence for this watch, if any
        /// has been recorded yet. A single current pointer, not a
        /// history — every superseded value is preserved separately by
        /// `HistoryLog`/`DomainPersistence`'s append-only records.
        last_evidence: Option<EvidenceRef>,
    },
    /// Permanently stopped. Terminal: no transition in this module leads
    /// out of this state.
    Stopped {
        /// Whatever evidence was last observed before this watch was
        /// stopped, preserved rather than discarded.
        last_evidence: Option<EvidenceRef>,
    },
}

impl WatchState {
    /// The most recently observed evidence, regardless of whether the
    /// watch is still active.
    pub fn last_evidence(&self) -> Option<EvidenceRef> {
        match self {
            WatchState::Active { last_evidence } | WatchState::Stopped { last_evidence } => {
                *last_evidence
            }
        }
    }
}

/// Why a [`WatchState`] transition did not apply. With exactly two
/// states (see this module's doc comment for why no third is
/// source-justified), "not `Active`" always means `Stopped`, and
/// `Stopped` is always terminal — so there is only one way any
/// transition can be rejected; a separate `InvalidFromState`-shaped
/// variant would be dead code no transition here could ever construct,
/// which is exactly the "invented for symmetry" this frontier's rule #8
/// forbids. The transition it came from left the watch's current state
/// completely unchanged (see
/// [`CurrentState::apply`](crate::features::domain_state::CurrentState::apply)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchTransitionRejected {
    /// The watch is [`WatchState::Stopped`] — terminal; no transition
    /// exists out of it.
    Terminal,
}

impl fmt::Display for WatchTransitionRejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WatchTransitionRejected::Terminal => {
                write!(f, "watch is stopped; no transition applies")
            }
        }
    }
}

impl std::error::Error for WatchTransitionRejected {}

/// Record that new evidence was observed for an active watch — the
/// locked chain's "Snapshot" step, realized as a transition input. Only
/// applies from [`WatchState::Active`].
#[derive(Debug, Clone, Copy)]
pub struct ObserveEvidence {
    /// The evidence being associated with the watch.
    pub evidence: EvidenceRef,
}

impl Transition<WatchState> for ObserveEvidence {
    type Rejection = WatchTransitionRejected;

    fn apply(&self, current: &WatchState) -> Result<WatchState, Self::Rejection> {
        match current {
            WatchState::Active { .. } => Ok(WatchState::Active {
                last_evidence: Some(self.evidence),
            }),
            WatchState::Stopped { .. } => Err(WatchTransitionRejected::Terminal),
        }
    }
}

/// Permanently stop a watch. Applies from [`WatchState::Active`]; rejected
/// (`Terminal`) if already stopped — not idempotent-by-silent-success.
#[derive(Debug, Clone, Copy, Default)]
pub struct StopWatch;

impl Transition<WatchState> for StopWatch {
    type Rejection = WatchTransitionRejected;

    fn apply(&self, current: &WatchState) -> Result<WatchState, Self::Rejection> {
        match current {
            WatchState::Active { last_evidence } => Ok(WatchState::Stopped {
                last_evidence: *last_evidence,
            }),
            WatchState::Stopped { .. } => Err(WatchTransitionRejected::Terminal),
        }
    }
}

/// Failure defining, reading, or transitioning a durable watch.
/// Storage/domain-shaped only.
#[derive(Debug)]
pub enum WatchError {
    /// No watch definition/state is recorded for the given `WatchId`.
    NotFound,
    /// The attempted transition does not apply to the watch's current
    /// state. The watch's persisted state is unchanged.
    TransitionRejected(WatchTransitionRejected),
    /// Another writer changed this watch's persisted state between this
    /// call's read and write — a genuine concurrent-modification race
    /// (Track 3's compare-and-swap fail-closed behavior), not an invalid
    /// domain transition. The watch's persisted state is unchanged;
    /// retry with a fresh read.
    ConcurrentModification,
    /// A backend/persistence failure unrelated to the above.
    Persistence(PersistenceError),
    /// The definition or state could not be encoded/decoded.
    Serialization(serde_json::Error),
}

impl fmt::Display for WatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WatchError::NotFound => write!(f, "no watch recorded for this identity"),
            WatchError::TransitionRejected(rejection) => write!(f, "{rejection}"),
            WatchError::ConcurrentModification => {
                write!(
                    f,
                    "watch state changed concurrently; retry with a fresh read"
                )
            }
            WatchError::Persistence(error) => write!(f, "watch ledger: {error}"),
            WatchError::Serialization(error) => {
                write!(f, "watch ledger: serialization failed: {error}")
            }
        }
    }
}

impl std::error::Error for WatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WatchError::TransitionRejected(rejection) => Some(rejection),
            WatchError::Persistence(error) => Some(error),
            WatchError::Serialization(error) => Some(error),
            WatchError::NotFound | WatchError::ConcurrentModification => None,
        }
    }
}

/// The persistence key `WatchDefinition` uses — deliberately distinct
/// from the plain `id.to_string()` key `WatchState` uses (below), so the
/// two never collide in `DomainPersistence`'s shared history table.
fn definition_key(id: WatchId) -> String {
    format!("{id}#definition")
}

/// Mint a fresh `WatchId`, durably record `target` as its
/// `WatchDefinition` (immutable, write-once), and initialize its
/// `WatchState` to `Active` with no evidence observed yet.
pub async fn define_watch(
    store: &DomainPersistence,
    target: DiscoveryTarget,
) -> Result<(WatchId, WatchDefinition), WatchError> {
    let id = WatchId::new();
    let definition = WatchDefinition { target };

    let definition_payload = serde_json::to_vec(&definition).map_err(WatchError::Serialization)?;
    store
        .append_history(
            &definition_key(id),
            1,
            &definition_payload,
            SystemTime::now(),
        )
        .await
        .map_err(WatchError::Persistence)?;

    let initial_state = WatchState::Active {
        last_evidence: None,
    };
    let state_payload = serde_json::to_vec(&initial_state).map_err(WatchError::Serialization)?;
    store
        .write_current(&id.to_string(), None, &state_payload)
        .await
        .map_err(WatchError::Persistence)?;

    Ok((id, definition))
}

/// Read the durable definition of `id`. `Ok(None)` if no watch was ever
/// defined for this identity.
pub async fn read_watch_definition(
    store: &DomainPersistence,
    id: WatchId,
) -> Result<Option<WatchDefinition>, WatchError> {
    let history = store
        .read_history(&definition_key(id))
        .await
        .map_err(WatchError::Persistence)?;

    match history.into_iter().next() {
        Some((_revision, payload, _recorded_at)) => {
            let definition = serde_json::from_slice(&payload).map_err(WatchError::Serialization)?;
            Ok(Some(definition))
        }
        None => Ok(None),
    }
}

/// Read the current durable state of `id`, together with its storage
/// revision. `Ok(None)` if no watch is recorded.
pub async fn read_current_watch_state(
    store: &DomainPersistence,
    id: WatchId,
) -> Result<Option<(u64, WatchState)>, WatchError> {
    match store
        .read_current(&id.to_string())
        .await
        .map_err(WatchError::Persistence)?
    {
        Some((revision, payload)) => {
            let state = serde_json::from_slice(&payload).map_err(WatchError::Serialization)?;
            Ok(Some((revision, state)))
        }
        None => Ok(None),
    }
}

/// Apply `transition` to `id`'s current durable state — the canonical
/// `current state + explicit transition → new current state` contract
/// (Track 2, unmodified), persisted through both of Track 3's
/// primitives: `write_current` (compare-and-swap) for the new current
/// state, `append_history` (immutable) for the just-superseded one.
///
/// On an invalid transition, nothing is written — the watch's persisted
/// state is exactly what it was before this call.
pub async fn apply_watch_transition<T>(
    store: &DomainPersistence,
    id: WatchId,
    transition: &T,
) -> Result<WatchState, WatchError>
where
    T: Transition<WatchState, Rejection = WatchTransitionRejected>,
{
    let (revision, current_state) = read_current_watch_state(store, id)
        .await?
        .ok_or(WatchError::NotFound)?;

    let current = CurrentState::new(id, current_state);
    let Applied {
        current: new_current,
        superseded,
    } = match current.apply(transition) {
        Ok(applied) => applied,
        Err((_unchanged, rejection)) => {
            return Err(WatchError::TransitionRejected(rejection));
        }
    };

    let new_payload = serde_json::to_vec(new_current.state()).map_err(WatchError::Serialization)?;
    store
        .write_current(&id.to_string(), Some(revision), &new_payload)
        .await
        .map_err(|error| match error {
            PersistenceError::CurrentStateConflict { .. } => WatchError::ConcurrentModification,
            other => WatchError::Persistence(other),
        })?;

    let superseded_payload =
        serde_json::to_vec(superseded.state()).map_err(WatchError::Serialization)?;
    store
        .append_history(
            &id.to_string(),
            revision,
            &superseded_payload,
            superseded.recorded_at(),
        )
        .await
        .map_err(WatchError::Persistence)?;

    Ok(new_current.into_parts().1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::discovery_target::DiscoveryTargetKind;
    use crate::features::identity::EvidenceId;

    fn requested_target(url: &str) -> DiscoveryTarget {
        DiscoveryTarget {
            url: url.to_string(),
            kind: DiscoveryTargetKind::Requested,
            discovered_via: None,
        }
    }

    fn active(last_evidence: Option<EvidenceRef>) -> WatchState {
        WatchState::Active { last_evidence }
    }

    #[test]
    fn observe_evidence_updates_current_pointer_only() {
        let evidence_ref = EvidenceRef::new(EvidenceId::new());
        let current = CurrentState::new(WatchId::new(), active(None));
        let applied = current
            .apply(&ObserveEvidence {
                evidence: evidence_ref,
            })
            .expect("observe from Active must succeed");
        assert_eq!(applied.current.state().last_evidence(), Some(evidence_ref));
        assert!(matches!(
            applied.superseded.state(),
            WatchState::Active { .. }
        ));
        assert_eq!(applied.superseded.state().last_evidence(), None);
    }

    #[test]
    fn stop_watch_preserves_last_evidence() {
        let evidence_ref = EvidenceRef::new(EvidenceId::new());
        let current = CurrentState::new(WatchId::new(), active(Some(evidence_ref)));
        let applied = current
            .apply(&StopWatch)
            .expect("stop from Active must succeed");
        assert!(matches!(
            applied.current.state(),
            WatchState::Stopped { .. }
        ));
        assert_eq!(applied.current.state().last_evidence(), Some(evidence_ref));
    }

    #[test]
    fn stopped_is_terminal_no_transition_leaves_it() {
        let current = CurrentState::new(WatchId::new(), active(None));
        let stopped = current.apply(&StopWatch).unwrap();

        let (unchanged, rejection) = stopped.current.clone().apply(&StopWatch).unwrap_err();
        assert_eq!(rejection, WatchTransitionRejected::Terminal);
        assert!(matches!(unchanged.state(), WatchState::Stopped { .. }));

        let (_, rejection) = stopped
            .current
            .apply(&ObserveEvidence {
                evidence: EvidenceRef::new(EvidenceId::new()),
            })
            .unwrap_err();
        assert_eq!(rejection, WatchTransitionRejected::Terminal);
    }

    #[tokio::test]
    async fn define_watch_persists_definition_and_initial_active_state() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, definition) = define_watch(&store, requested_target("https://example.test/"))
            .await
            .unwrap();
        assert_eq!(definition.target.url, "https://example.test/");

        let read_definition = read_watch_definition(&store, id).await.unwrap().unwrap();
        assert_eq!(read_definition, definition);

        let (revision, state) = read_current_watch_state(&store, id).await.unwrap().unwrap();
        assert_eq!(revision, 1);
        assert!(matches!(
            state,
            WatchState::Active {
                last_evidence: None
            }
        ));
    }

    #[tokio::test]
    async fn watch_definition_is_immutable_defining_twice_yields_two_ids() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id_a, _) = define_watch(&store, requested_target("https://a.test/"))
            .await
            .unwrap();
        let (id_b, _) = define_watch(&store, requested_target("https://b.test/"))
            .await
            .unwrap();
        assert_ne!(id_a, id_b);

        let def_a = read_watch_definition(&store, id_a).await.unwrap().unwrap();
        let def_b = read_watch_definition(&store, id_b).await.unwrap().unwrap();
        assert_eq!(def_a.target.url, "https://a.test/");
        assert_eq!(def_b.target.url, "https://b.test/");
    }

    #[tokio::test]
    async fn full_observe_then_stop_lifecycle_persists_and_reads_back_truthfully() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target("https://example.test/"))
            .await
            .unwrap();

        let evidence_ref = EvidenceRef::new(EvidenceId::new());
        let observed = apply_watch_transition(
            &store,
            id,
            &ObserveEvidence {
                evidence: evidence_ref,
            },
        )
        .await
        .unwrap();
        assert_eq!(observed.last_evidence(), Some(evidence_ref));

        let stopped = apply_watch_transition(&store, id, &StopWatch)
            .await
            .unwrap();
        assert!(matches!(stopped, WatchState::Stopped { .. }));
        assert_eq!(stopped.last_evidence(), Some(evidence_ref));

        let (_, current) = read_current_watch_state(&store, id).await.unwrap().unwrap();
        assert!(matches!(current, WatchState::Stopped { .. }));

        // Every superseded state was appended to history, in order:
        // Active(None) superseded by observe, Active(Some) superseded by
        // stop.
        let history = store.read_history(&id.to_string()).await.unwrap();
        assert_eq!(history.len(), 2);
        let first: WatchState = serde_json::from_slice(&history[0].1).unwrap();
        let second: WatchState = serde_json::from_slice(&history[1].1).unwrap();
        assert_eq!(
            first,
            WatchState::Active {
                last_evidence: None
            }
        );
        assert_eq!(
            second,
            WatchState::Active {
                last_evidence: Some(evidence_ref)
            }
        );
    }

    #[tokio::test]
    async fn invalid_transition_persists_nothing_and_leaves_current_state_unchanged() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target("https://example.test/"))
            .await
            .unwrap();
        apply_watch_transition(&store, id, &StopWatch)
            .await
            .unwrap();

        let error = apply_watch_transition(
            &store,
            id,
            &ObserveEvidence {
                evidence: EvidenceRef::new(EvidenceId::new()),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            WatchError::TransitionRejected(WatchTransitionRejected::Terminal)
        ));

        // Still revision 2 (from the successful stop only) — the
        // rejected observe wrote nothing.
        let (revision, state) = read_current_watch_state(&store, id).await.unwrap().unwrap();
        assert_eq!(revision, 2);
        assert!(matches!(state, WatchState::Stopped { .. }));
        assert_eq!(store.read_history(&id.to_string()).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn apply_watch_transition_on_unknown_id_is_not_found() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let error = apply_watch_transition(&store, WatchId::new(), &StopWatch)
            .await
            .unwrap_err();
        assert!(matches!(error, WatchError::NotFound));
    }

    #[tokio::test]
    async fn read_watch_definition_of_unknown_id_is_none() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        assert!(read_watch_definition(&store, WatchId::new())
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn definition_and_state_keys_do_not_collide() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target("https://example.test/"))
            .await
            .unwrap();
        // Both a definition history entry (namespaced key) and a current
        // state (plain key) exist independently for the same WatchId.
        assert!(read_watch_definition(&store, id).await.unwrap().is_some());
        assert!(read_current_watch_state(&store, id)
            .await
            .unwrap()
            .is_some());
        // The plain-key history (superseded WatchStates) starts empty —
        // defining a watch does not itself append a historical state
        // transition, only the initial current-state write.
        assert!(store
            .read_history(&id.to_string())
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn distinct_watches_do_not_interfere() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (a, _) = define_watch(&store, requested_target("https://a.test/"))
            .await
            .unwrap();
        let (b, _) = define_watch(&store, requested_target("https://b.test/"))
            .await
            .unwrap();

        apply_watch_transition(&store, a, &StopWatch).await.unwrap();

        let (_, state_a) = read_current_watch_state(&store, a).await.unwrap().unwrap();
        let (_, state_b) = read_current_watch_state(&store, b).await.unwrap().unwrap();
        assert!(matches!(state_a, WatchState::Stopped { .. }));
        assert!(matches!(state_b, WatchState::Active { .. }));
    }
}
