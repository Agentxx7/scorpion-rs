//! Canonical change detection: `ChangeResult` (the truthful outcome of
//! comparing two durable evidence records for the same watch) and
//! `ChangeEvent` (the durable, immutable historical record of a detected
//! comparison).
//!
//! Track 9 of the roadmap — the successor boundary Track 7's own closure
//! deferred alongside scheduling: *"no `ChangeResult`/`ChangeEvent`... no
//! health, no notification system... those remain later, separate
//! frontiers."* This module realizes exactly that, and no further: no
//! health, no notifications, no generic event framework, no `Job`/
//! `Operation`, no second `Evidence`/`Watch` model, no new acquisition
//! path, and no scheduler of its own — [`crate::features::watch_schedule`]
//! (Track 8) remains the sole scheduler/execution owner; this module only
//! ever *reads* the evidence a watch's own history already contains.
//!
//! # Same-watch evidence only
//!
//! [`detect_and_record_change`] never trusts a caller-supplied pairing of
//! [`EvidenceRef`]s blindly: it reads `watch`'s own `WatchState` current
//! value and every superseded historical value (Track 7's
//! `HistoryLog`/`DomainPersistence::read_history`, reused unmodified —
//! the exact same append-only records
//! [`crate::features::watch::apply_watch_transition`] already produces)
//! and confirms both the previous and current evidence actually appear as
//! a `last_evidence` value somewhere in that specific watch's own
//! history. An `EvidenceRef` that never belonged to `watch` is rejected —
//! [`ChangeDetectionError::EvidenceNotAssociatedWithWatch`] — before any
//! comparison is attempted, so a caller can never mix evidence from two
//! unrelated watches into one `ChangeEvent`.
//!
//! # Truthful comparison — never reduced to "unchanged"
//!
//! [`compute_change_result`] is a pure function of two already-resolved
//! [`EvidenceBundle`]s. It never invents a new hash: it reuses exactly
//! the SHA-256 fields [`crate::utils::evidence::build_evidence`] already
//! computed (via the same [`sha256_hex`] every other hashing seam in this
//! crate uses) — `response_body_hash` (the raw acquired bytes, preferred
//! whenever present) or, only when that is absent,
//! `transformed_content_hash` (the extracted/transformed text). A
//! comparison is only ever made between two hashes computed on the exact
//! same basis — comparing a raw-body hash against a transformed-content
//! hash would not be a truthful signal, so that case (and the case where
//! neither bundle carries any usable hash at all — e.g. two
//! browser-rendered evidence records with no body and no content
//! captured) produces [`ChangeResult::Uncomparable`], never a silent
//! [`ChangeResult::Unchanged`]. Evidence that cannot even be *resolved*
//! (an [`EvidenceRef`] naming a record that was never durably written) is
//! rejected before comparison is attempted at all —
//! [`ChangeDetectionError::PreviousEvidenceUnresolvable`]/
//! [`ChangeDetectionError::CurrentEvidenceUnresolvable`] — never silently
//! treated as "no change."
//!
//! # Lineage/fingerprint reuse (rule #6 — no second fingerprint architecture)
//!
//! `TransformLineageId`/`TransformLineageRecord` (Track 6) model a
//! materially different fact — `source input → transformation → output`
//! — and are not reused as a *type* here: a change comparison has no
//! transformation step and no output distinct from the evidence being
//! compared, so forcing that shape on this concept would not be
//! source-justified. What genuinely *is* reused from Track 6 is (a) the
//! same [`sha256_hex`] hashing primitive every hash field in this module
//! ultimately traces back to — no new hashing/fingerprinting logic is
//! introduced anywhere — and (b) Track 6's own established
//! content-addressed, idempotent-duplicate-append persistence pattern
//! (see "Persistence" below), applied to a structurally identical kind of
//! fact: an immutable observation about two already-durable inputs, not
//! a lifecycle state. `spider::configuration::Fingerprint` (browser
//! anti-detection stealth spoofing) remains untouched, unimported, and
//! unreferenced, exactly as Track 6 left it.
//!
//! # Persistence
//!
//! [`ChangeEventId`] is content-addressed — deterministic SHA-256 of
//! `(watch, previous_evidence, current_evidence)`, exactly
//! `TransformLineageId`'s own construction pattern — because the
//! resulting [`ChangeResult`] is itself a pure function of that same
//! triple's already-durable inputs: comparing the same two evidence
//! records for the same watch again can only ever reproduce the same
//! fact. [`ChangeEvent`] is persisted through
//! [`DomainPersistence::append_history`] only (never `write_current` — a
//! change-detection fact has no current state to replace), at fixed
//! revision `1`, exactly like Track 6's lineage ledger. Recording an
//! identical `(watch, previous, current)` fact twice is therefore
//! idempotent, not a conflict:
//! `Err(PersistenceError::HistoryAlreadyExists) => Ok(event)`, reusing
//! Track 6's own precedent verbatim rather than re-deriving it. There is
//! no `write_current`, no raw SQL, and no second persistence mechanism
//! anywhere in this module.
//!
//! # Computation vs. persistence
//!
//! [`compute_change_result`] never touches [`DomainPersistence`] — it is
//! pure, synchronous, and callable in isolation (e.g. for a future
//! dry-run surface that never persists anything).
//! [`detect_and_record_change`] is the only function in this module that
//! performs I/O; it resolves both `EvidenceRef`s, calls
//! `compute_change_result`, and persists the resulting [`ChangeEvent`] —
//! three separable steps, never fused into one un-inspectable operation.

use crate::features::domain_persistence::{DomainPersistence, PersistenceError};
use crate::features::identity::WatchId;
use crate::features::watch::{self, WatchError, WatchState};
use crate::utils::evidence::{sha256_hex, EvidenceBundle, EvidenceLedgerError, EvidenceRef};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::SystemTime;

/// Which of `EvidenceBundle`'s hash fields a [`ChangeResult`] compared.
/// Two hashes are only ever compared when both bundles produced this
/// exact same basis — see this module's doc comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComparisonBasis {
    /// `EvidenceBundle::response_body_hash` — the raw acquired response
    /// bytes. Preferred whenever present.
    ResponseBodyHash,
    /// `EvidenceBundle::transformed_content_hash` — the extracted/
    /// transformed text content. Used only when `response_body_hash` is
    /// absent from *both* bundles being compared.
    TransformedContentHash,
}

/// Why two evidence records could not be truthfully compared at all. With
/// exactly one code path that resolves both bundles before calling
/// [`compute_change_result`], there is exactly one source-justified
/// reason a *pure comparison* can fail to produce a definite answer —
/// see this module's doc comment for why evidence that cannot be
/// resolved at all is a separate, earlier failure
/// ([`ChangeDetectionError`]), not folded into this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UncomparableReason {
    /// Neither bundle carries a hash on a shared, comparable basis (e.g.
    /// both are browser-rendered evidence with no captured body or
    /// content), or the two bundles' available hashes use different
    /// bases (comparing a raw-body hash against a transformed-content
    /// hash would not be truthful).
    NoConsistentContentSignal,
}

/// The truthful result of comparing one watch's previous durable evidence
/// against its current durable evidence. Never reduces an uncomparable
/// pair to [`ChangeResult::Unchanged`] — see this module's doc comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeResult {
    /// Both bundles produced an equal hash on the same [`ComparisonBasis`].
    Unchanged {
        /// Which hash field was compared.
        basis: ComparisonBasis,
    },
    /// Both bundles produced a hash on the same [`ComparisonBasis`], and
    /// the hashes differ.
    Changed {
        /// Which hash field was compared.
        basis: ComparisonBasis,
        /// The previous evidence's hash on `basis`.
        previous_hash: String,
        /// The current evidence's hash on `basis`.
        current_hash: String,
    },
    /// Source truth does not support a definite comparison.
    Uncomparable {
        /// Why.
        reason: UncomparableReason,
    },
}

/// Pick the hash `compute_change_result` should compare a bundle on:
/// `response_body_hash` preferred, `transformed_content_hash` only as a
/// fallback when the former is absent. Never invents a hash — both
/// fields are read exactly as [`crate::utils::evidence::build_evidence`]
/// already computed them via [`sha256_hex`].
fn comparable_hash(bundle: &EvidenceBundle) -> Option<(ComparisonBasis, &str)> {
    bundle
        .response_body_hash
        .as_deref()
        .map(|hash| (ComparisonBasis::ResponseBodyHash, hash))
        .or_else(|| {
            bundle
                .transformed_content_hash
                .as_deref()
                .map(|hash| (ComparisonBasis::TransformedContentHash, hash))
        })
}

/// Pure comparison of two already-resolved [`EvidenceBundle`]s. Performs
/// no I/O and decides no persistence — see this module's doc comment
/// ("Computation vs. persistence").
pub fn compute_change_result(previous: &EvidenceBundle, current: &EvidenceBundle) -> ChangeResult {
    match (comparable_hash(previous), comparable_hash(current)) {
        (Some((previous_basis, previous_hash)), Some((current_basis, current_hash)))
            if previous_basis == current_basis =>
        {
            if previous_hash == current_hash {
                ChangeResult::Unchanged {
                    basis: previous_basis,
                }
            } else {
                ChangeResult::Changed {
                    basis: previous_basis,
                    previous_hash: previous_hash.to_string(),
                    current_hash: current_hash.to_string(),
                }
            }
        }
        _ => ChangeResult::Uncomparable {
            reason: UncomparableReason::NoConsistentContentSignal,
        },
    }
}

/// Content-addressed identity of one [`ChangeEvent`] — deterministic from
/// `(watch, previous_evidence, current_evidence)`. See this module's doc
/// comment for why this mirrors `TransformLineageId`'s own construction.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChangeEventId(String);

impl ChangeEventId {
    /// Wire-format prefix.
    pub const PREFIX: &'static str = "change_";

    /// `pub(crate)` (not module-private) so a read-only, purely
    /// observational caller — namely Track 10's `watch_health` — can
    /// check "was this exact evidence pair ever actually compared"
    /// (via [`read_change_event`]) without recomputing a comparison
    /// itself, which would mean owning change computation rather than
    /// only reading its already-durable result. This exposes no new
    /// behavior: the formula is unchanged, and no crate-external caller
    /// gains access to it.
    pub(crate) fn derive(
        watch: WatchId,
        previous_evidence: EvidenceRef,
        current_evidence: EvidenceRef,
    ) -> Self {
        let joined = format!(
            "change-v1|{watch}|{}|{}",
            previous_evidence.id(),
            current_evidence.id()
        );
        Self(format!("{}{}", Self::PREFIX, sha256_hex(joined.as_bytes())))
    }

    /// Borrow the wire-format string (`change_<sha256 hex>`).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ChangeEventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One immutable historical record of a detected comparison. References
/// `watch`/`previous_evidence`/`current_evidence` by identity only —
/// never a copy of any evidence payload. The only way to obtain one is
/// [`detect_and_record_change`]'s return path or [`read_change_event`]
/// reading one back — there is no public constructor that lets a caller
/// assemble a record (and therefore its id) from a `ChangeResult` it did
/// not actually compute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeEvent {
    watch: WatchId,
    previous_evidence: EvidenceRef,
    current_evidence: EvidenceRef,
    result: ChangeResult,
    detected_at: SystemTime,
}

impl ChangeEvent {
    /// The watch this comparison belongs to.
    pub fn watch(&self) -> WatchId {
        self.watch
    }

    /// The prior evidence compared.
    pub fn previous_evidence(&self) -> EvidenceRef {
        self.previous_evidence
    }

    /// The current evidence compared.
    pub fn current_evidence(&self) -> EvidenceRef {
        self.current_evidence
    }

    /// The truthful comparison outcome.
    pub fn result(&self) -> &ChangeResult {
        &self.result
    }

    /// When this comparison was recorded.
    pub fn detected_at(&self) -> SystemTime {
        self.detected_at
    }

    /// This record's content-addressed identity.
    pub fn id(&self) -> ChangeEventId {
        ChangeEventId::derive(self.watch, self.previous_evidence, self.current_evidence)
    }
}

/// Why a change could not be detected or recorded.
#[derive(Debug)]
pub enum ChangeDetectionError {
    /// `previous_evidence` does not name any `last_evidence` value ever
    /// recorded in `watch`'s own history.
    EvidenceNotAssociatedWithWatch,
    /// `previous_evidence` names no durable evidence record at all.
    PreviousEvidenceUnresolvable,
    /// `current_evidence` names no durable evidence record at all.
    CurrentEvidenceUnresolvable,
    /// Failure reading `watch`'s own state/history.
    Watch(WatchError),
    /// Failure resolving an `EvidenceRef`.
    Evidence(EvidenceLedgerError),
    /// A backend/persistence failure unrelated to the above.
    Persistence(PersistenceError),
    /// The event (or a watch history entry read while validating
    /// association) could not be encoded/decoded.
    Serialization(serde_json::Error),
}

impl fmt::Display for ChangeDetectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChangeDetectionError::EvidenceNotAssociatedWithWatch => {
                write!(f, "the supplied evidence was never recorded for this watch")
            }
            ChangeDetectionError::PreviousEvidenceUnresolvable => {
                write!(f, "the previous evidence could not be resolved")
            }
            ChangeDetectionError::CurrentEvidenceUnresolvable => {
                write!(f, "the current evidence could not be resolved")
            }
            ChangeDetectionError::Watch(error) => write!(f, "{error}"),
            ChangeDetectionError::Evidence(error) => write!(f, "{error}"),
            ChangeDetectionError::Persistence(error) => {
                write!(f, "change detection ledger: {error}")
            }
            ChangeDetectionError::Serialization(error) => {
                write!(f, "change detection ledger: serialization failed: {error}")
            }
        }
    }
}

impl std::error::Error for ChangeDetectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ChangeDetectionError::Watch(error) => Some(error),
            ChangeDetectionError::Evidence(error) => Some(error),
            ChangeDetectionError::Persistence(error) => Some(error),
            ChangeDetectionError::Serialization(error) => Some(error),
            ChangeDetectionError::EvidenceNotAssociatedWithWatch
            | ChangeDetectionError::PreviousEvidenceUnresolvable
            | ChangeDetectionError::CurrentEvidenceUnresolvable => None,
        }
    }
}

/// Every `EvidenceRef` ever associated with `watch` as a `last_evidence`
/// value — its current state plus every superseded historical state
/// (Track 7's own `HistoryLog`/`DomainPersistence::read_history`, reused
/// unmodified, at exactly the plain `watch.to_string()` key
/// `apply_watch_transition` already writes to).
async fn watch_evidence_refs(
    store: &DomainPersistence,
    watch: WatchId,
) -> Result<Vec<EvidenceRef>, ChangeDetectionError> {
    let mut refs = Vec::new();

    if let Some((_revision, state)) = self::watch::read_current_watch_state(store, watch)
        .await
        .map_err(ChangeDetectionError::Watch)?
    {
        refs.extend(state.last_evidence());
    }

    let history = store
        .read_history(&watch.to_string())
        .await
        .map_err(ChangeDetectionError::Persistence)?;
    for (_revision, payload, _recorded_at) in history {
        let state: WatchState =
            serde_json::from_slice(&payload).map_err(ChangeDetectionError::Serialization)?;
        refs.extend(state.last_evidence());
    }

    Ok(refs)
}

async fn ensure_evidence_belongs_to_watch(
    store: &DomainPersistence,
    watch: WatchId,
    evidence: EvidenceRef,
) -> Result<(), ChangeDetectionError> {
    let refs = watch_evidence_refs(store, watch).await?;
    if refs.contains(&evidence) {
        Ok(())
    } else {
        Err(ChangeDetectionError::EvidenceNotAssociatedWithWatch)
    }
}

/// Detect and durably record the change between `previous_evidence` and
/// `current_evidence`, both of which must belong to `watch`'s own
/// history. See this module's doc comment for the full same-watch,
/// truthful-comparison, and idempotent-persistence contract.
pub async fn detect_and_record_change(
    store: &DomainPersistence,
    watch: WatchId,
    previous_evidence: EvidenceRef,
    current_evidence: EvidenceRef,
) -> Result<ChangeEvent, ChangeDetectionError> {
    ensure_evidence_belongs_to_watch(store, watch, previous_evidence).await?;
    ensure_evidence_belongs_to_watch(store, watch, current_evidence).await?;

    let previous_bundle = previous_evidence
        .resolve(store)
        .await
        .map_err(ChangeDetectionError::Evidence)?
        .ok_or(ChangeDetectionError::PreviousEvidenceUnresolvable)?;
    let current_bundle = current_evidence
        .resolve(store)
        .await
        .map_err(ChangeDetectionError::Evidence)?
        .ok_or(ChangeDetectionError::CurrentEvidenceUnresolvable)?;

    let result = compute_change_result(&previous_bundle, &current_bundle);

    let event = ChangeEvent {
        watch,
        previous_evidence,
        current_evidence,
        result,
        detected_at: SystemTime::now(),
    };
    let id = event.id();
    let payload = serde_json::to_vec(&event).map_err(ChangeDetectionError::Serialization)?;

    match store
        .append_history(id.as_str(), 1, &payload, event.detected_at)
        .await
    {
        Ok(()) => Ok(event),
        Err(PersistenceError::HistoryAlreadyExists) => Ok(event),
        Err(other) => Err(ChangeDetectionError::Persistence(other)),
    }
}

/// Read back the change event named by `id`, exactly as
/// [`detect_and_record_change`] wrote it. `Ok(None)` if nothing has ever
/// been recorded for this identity.
pub async fn read_change_event(
    store: &DomainPersistence,
    id: &ChangeEventId,
) -> Result<Option<ChangeEvent>, ChangeDetectionError> {
    let history = store
        .read_history(id.as_str())
        .await
        .map_err(ChangeDetectionError::Persistence)?;

    match history.into_iter().next() {
        Some((_revision, payload, _recorded_at)) => {
            let event =
                serde_json::from_slice(&payload).map_err(ChangeDetectionError::Serialization)?;
            Ok(Some(event))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::discovery_target::{DiscoveryTarget, DiscoveryTargetKind};
    use crate::features::identity::EvidenceId;
    use crate::features::watch::{apply_watch_transition, define_watch, ObserveEvidence};
    use crate::utils::evidence::record_evidence;

    fn requested_target(url: &str) -> DiscoveryTarget {
        DiscoveryTarget {
            url: url.to_string(),
            kind: DiscoveryTargetKind::Requested,
            discovered_via: None,
        }
    }

    fn bundle_with_body_hash(hash: &str) -> EvidenceBundle {
        EvidenceBundle {
            response_body_hash: Some(hash.to_string()),
            ..Default::default()
        }
    }

    async fn record(store: &DomainPersistence, bundle: EvidenceBundle) -> EvidenceRef {
        let recorded = record_evidence(store, bundle).await.unwrap();
        EvidenceRef::new(recorded.id.unwrap())
    }

    // --- compute_change_result: pure comparison ---

    #[test]
    fn equal_response_body_hashes_are_unchanged() {
        let previous = bundle_with_body_hash("abc");
        let current = bundle_with_body_hash("abc");
        assert_eq!(
            compute_change_result(&previous, &current),
            ChangeResult::Unchanged {
                basis: ComparisonBasis::ResponseBodyHash
            }
        );
    }

    #[test]
    fn different_response_body_hashes_are_changed() {
        let previous = bundle_with_body_hash("abc");
        let current = bundle_with_body_hash("xyz");
        assert_eq!(
            compute_change_result(&previous, &current),
            ChangeResult::Changed {
                basis: ComparisonBasis::ResponseBodyHash,
                previous_hash: "abc".to_string(),
                current_hash: "xyz".to_string(),
            }
        );
    }

    #[test]
    fn falls_back_to_transformed_content_hash_when_body_hash_absent_on_both() {
        let previous = EvidenceBundle {
            transformed_content_hash: Some("t1".to_string()),
            ..Default::default()
        };
        let current = EvidenceBundle {
            transformed_content_hash: Some("t2".to_string()),
            ..Default::default()
        };
        assert_eq!(
            compute_change_result(&previous, &current),
            ChangeResult::Changed {
                basis: ComparisonBasis::TransformedContentHash,
                previous_hash: "t1".to_string(),
                current_hash: "t2".to_string(),
            }
        );
    }

    #[test]
    fn mismatched_basis_is_uncomparable_never_unchanged() {
        // One bundle only has a body hash, the other only a transformed
        // content hash — comparing across bases would not be truthful.
        let previous = bundle_with_body_hash("abc");
        let current = EvidenceBundle {
            transformed_content_hash: Some("abc".to_string()),
            ..Default::default()
        };
        assert_eq!(
            compute_change_result(&previous, &current),
            ChangeResult::Uncomparable {
                reason: UncomparableReason::NoConsistentContentSignal
            }
        );
    }

    #[test]
    fn no_usable_hash_on_either_side_is_uncomparable_never_unchanged() {
        let previous = EvidenceBundle::default();
        let current = EvidenceBundle::default();
        assert_eq!(
            compute_change_result(&previous, &current),
            ChangeResult::Uncomparable {
                reason: UncomparableReason::NoConsistentContentSignal
            }
        );
    }

    // --- detect_and_record_change: association, persistence, idempotency ---

    #[tokio::test]
    async fn same_watch_evidence_produces_a_truthful_change_event() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target("https://example.test/"))
            .await
            .unwrap();

        let previous = record(&store, bundle_with_body_hash("v1")).await;
        apply_watch_transition(&store, id, &ObserveEvidence { evidence: previous })
            .await
            .unwrap();
        let current = record(&store, bundle_with_body_hash("v2")).await;
        apply_watch_transition(&store, id, &ObserveEvidence { evidence: current })
            .await
            .unwrap();

        let event = detect_and_record_change(&store, id, previous, current)
            .await
            .unwrap();
        assert_eq!(event.watch(), id);
        assert_eq!(event.previous_evidence(), previous);
        assert_eq!(event.current_evidence(), current);
        assert_eq!(
            *event.result(),
            ChangeResult::Changed {
                basis: ComparisonBasis::ResponseBodyHash,
                previous_hash: "v1".to_string(),
                current_hash: "v2".to_string(),
            }
        );

        let read_back = read_change_event(&store, &event.id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(read_back, event);
    }

    #[tokio::test]
    async fn evidence_from_an_unrelated_watch_is_rejected() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (watch_a, _) = define_watch(&store, requested_target("https://a.test/"))
            .await
            .unwrap();
        let (watch_b, _) = define_watch(&store, requested_target("https://b.test/"))
            .await
            .unwrap();

        let evidence_a = record(&store, bundle_with_body_hash("a")).await;
        apply_watch_transition(
            &store,
            watch_a,
            &ObserveEvidence {
                evidence: evidence_a,
            },
        )
        .await
        .unwrap();
        let evidence_b = record(&store, bundle_with_body_hash("b")).await;
        apply_watch_transition(
            &store,
            watch_b,
            &ObserveEvidence {
                evidence: evidence_b,
            },
        )
        .await
        .unwrap();

        // evidence_b was never recorded against watch_a.
        let error = detect_and_record_change(&store, watch_a, evidence_a, evidence_b)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            ChangeDetectionError::EvidenceNotAssociatedWithWatch
        ));
    }

    #[tokio::test]
    async fn unresolvable_evidence_is_rejected_not_treated_as_unchanged() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target("https://example.test/"))
            .await
            .unwrap();

        // A ref that was never actually recorded via record_evidence — it
        // cannot even be a `last_evidence` value, so this also proves the
        // association check itself fails closed for phantom refs.
        let phantom = EvidenceRef::new(EvidenceId::new());
        let error = detect_and_record_change(&store, id, phantom, phantom)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            ChangeDetectionError::EvidenceNotAssociatedWithWatch
        ));
    }

    #[tokio::test]
    async fn recording_the_identical_comparison_twice_is_idempotent_not_a_conflict() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target("https://example.test/"))
            .await
            .unwrap();
        let previous = record(&store, bundle_with_body_hash("v1")).await;
        apply_watch_transition(&store, id, &ObserveEvidence { evidence: previous })
            .await
            .unwrap();
        let current = record(&store, bundle_with_body_hash("v2")).await;
        apply_watch_transition(&store, id, &ObserveEvidence { evidence: current })
            .await
            .unwrap();

        let first = detect_and_record_change(&store, id, previous, current)
            .await
            .unwrap();
        let second = detect_and_record_change(&store, id, previous, current)
            .await
            .unwrap();

        assert_eq!(first.id(), second.id());
        assert_eq!(
            store.read_history(first.id().as_str()).await.unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn different_facts_never_silently_collapse() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target("https://example.test/"))
            .await
            .unwrap();
        let v1 = record(&store, bundle_with_body_hash("v1")).await;
        apply_watch_transition(&store, id, &ObserveEvidence { evidence: v1 })
            .await
            .unwrap();
        let v2 = record(&store, bundle_with_body_hash("v2")).await;
        apply_watch_transition(&store, id, &ObserveEvidence { evidence: v2 })
            .await
            .unwrap();
        let v3 = record(&store, bundle_with_body_hash("v3")).await;
        apply_watch_transition(&store, id, &ObserveEvidence { evidence: v3 })
            .await
            .unwrap();

        let event_a = detect_and_record_change(&store, id, v1, v2).await.unwrap();
        let event_b = detect_and_record_change(&store, id, v2, v3).await.unwrap();

        assert_ne!(event_a.id(), event_b.id());
        assert!(read_change_event(&store, &event_a.id())
            .await
            .unwrap()
            .is_some());
        assert!(read_change_event(&store, &event_b.id())
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn change_event_history_is_append_only() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target("https://example.test/"))
            .await
            .unwrap();
        let previous = record(&store, bundle_with_body_hash("v1")).await;
        apply_watch_transition(&store, id, &ObserveEvidence { evidence: previous })
            .await
            .unwrap();
        let current = record(&store, bundle_with_body_hash("v2")).await;
        apply_watch_transition(&store, id, &ObserveEvidence { evidence: current })
            .await
            .unwrap();

        let event = detect_and_record_change(&store, id, previous, current)
            .await
            .unwrap();
        let first_read = read_change_event(&store, &event.id())
            .await
            .unwrap()
            .unwrap();
        let second_read = read_change_event(&store, &event.id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first_read, second_read);
    }

    #[tokio::test]
    async fn read_change_event_of_unknown_id_is_none() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (id, _) = define_watch(&store, requested_target("https://example.test/"))
            .await
            .unwrap();
        let bogus = ChangeEventId::derive(
            id,
            EvidenceRef::new(EvidenceId::new()),
            EvidenceRef::new(EvidenceId::new()),
        );
        assert!(read_change_event(&store, &bogus).await.unwrap().is_none());
    }

    // --- Real Track 8 production-path verification: change detection
    // driven entirely by real scheduled-watch-run evidence, not
    // hand-built bundles. Proves Track 9 never becomes a second
    // scheduler/execution owner — it only ever consumes the evidence
    // Track 8's own execution path already produced.
    #[cfg(feature = "cron")]
    mod production_path {
        use super::*;
        use crate::features::transport::{TransportMode, TransportRequest};
        use crate::features::watch_schedule::{define_watch_schedule, execute_scheduled_watch_run};
        use std::net::SocketAddr;
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
        use std::sync::Arc;
        use std::time::Duration;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        /// Serves a different body on each successive request, so two
        /// real scheduled runs against the same fixture produce two
        /// genuinely different durable evidence records.
        struct SequencedHttpFixture {
            addr: SocketAddr,
        }

        impl SequencedHttpFixture {
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

        fn default_transport() -> TransportRequest {
            TransportRequest {
                mode: TransportMode::Default,
                proxy: None,
            }
        }

        #[tokio::test]
        async fn change_detection_over_two_real_track_8_scheduled_runs() {
            let fixture = SequencedHttpFixture::start(&[b"version one", b"version two"]).await;
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let (id, _) = define_watch(
                &store,
                requested_target(&format!("http://{}/", fixture.addr)),
            )
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

            let event = detect_and_record_change(&store, id, previous, current)
                .await
                .unwrap();

            assert!(matches!(event.result(), ChangeResult::Changed { .. }));
            if let ChangeResult::Changed {
                previous_hash,
                current_hash,
                ..
            } = event.result()
            {
                assert_eq!(previous_hash, &sha256_hex(b"version one"));
                assert_eq!(current_hash, &sha256_hex(b"version two"));
            }
        }

        #[tokio::test]
        async fn no_change_across_two_real_track_8_scheduled_runs_of_identical_content() {
            let fixture = SequencedHttpFixture::start(&[b"stable body"]).await;
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let (id, _) = define_watch(
                &store,
                requested_target(&format!("http://{}/", fixture.addr)),
            )
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

            let event = detect_and_record_change(&store, id, previous, current)
                .await
                .unwrap();

            assert_eq!(
                *event.result(),
                ChangeResult::Unchanged {
                    basis: ComparisonBasis::ResponseBodyHash
                }
            );
        }
    }
}
