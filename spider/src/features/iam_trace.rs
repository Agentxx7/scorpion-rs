//! Canonical IAM Callback Inspector trace / observation / redaction model.
//!
//! `SCORPION_CANONICAL_IAM_TRACE_AND_OBSERVATION_MODEL_001`, realizing the
//! model half of the owner-decided IAM Callback Inspector
//! (`SCORPION_ARCHITECTURE.md` §3.8). Identity
//! ([`crate::features::identity::IamTraceId`]) lives in
//! `features/identity.rs`; the transition contract this module uses
//! (current state / historical record / transition) is
//! [`crate::features::domain_state`]'s, unmodified — this is the second
//! capability to use it, after `features/auth_session.rs`, which this
//! module's own state-machine shape directly follows.
//!
//! This module owns **model only**: no network, no HTTP parsing, no
//! callback listener, no JWT decoding, no SAML XML parsing, and no
//! persistence I/O. Those are later, separate frontiers building on top
//! of what is defined here — see "Not implemented here" below.
//!
//! # Why a new identity, not reuse of an existing one
//!
//! One operator-created troubleshooting trace may accumulate multiple
//! callback observations over its lifetime (a redirect that never
//! returns, then a retry; a front-channel and a back-channel delivery of
//! the same flow). Those observations need a stable identity independent
//! of any single one of them — the same relationship
//! [`crate::features::identity::ResearchId`] already has to the many
//! `EvidenceId`s one research invocation produces. No `IamObservationId`
//! exists: one trace's observations are distinguished by persistence
//! revision (see "Persistence shape" below), exactly how
//! `spider::utils::evidence::record_evidence` already distinguishes
//! per-identity records without a second identity type.
//!
//! # Lifecycle vocabulary (minimum truthful, not maximum plausible)
//!
//! Two states — [`IamTraceState::AwaitingCallback`] and
//! [`IamTraceState::Received`] — not three or four. "Created" and
//! "AwaitingCallback" collapse into one: nothing meaningful happens
//! between minting an [`IamTraceId`](crate::features::identity::IamTraceId)
//! and the trace being ready to receive a callback, so inventing a
//! separate `Created` state would be a distinction without a behavioral
//! difference. No `TimedOut`/`Retrying`/`Closed` state exists — the owner
//! decision for this frontier explicitly forbids adding them "merely
//! because they may be useful later." [`ReceiveCallback`] is the one
//! transition: it fires exactly once, on the *first* observation a trace
//! receives (`AwaitingCallback` -> `Received`); every observation after
//! the first is recorded by appending a new history revision under the
//! same trace (a later receiver frontier's job), not by transitioning
//! current state again — a trace that already recorded its first
//! observation rejects a second [`ReceiveCallback`]
//! ([`IamTraceTransitionRejected::AlreadyReceived`]), which is this
//! module's one real, falsifiable "invalid transition" case, not an
//! invented one.
//!
//! Trace-creation-time correlation inputs (an operator-supplied expected
//! `state`/`nonce` to compare an observation against) are deliberately
//! **not** state carried by [`IamTraceState`] — they are inputs to a
//! future comparison operation a receiver frontier performs when
//! producing an [`IamFact`], not part of this trace's own coarse
//! lifecycle. Keeping them out of `IamTraceState` keeps this frontier's
//! two states genuinely bare and avoids inventing payload fields this
//! frontier has no receiver to populate truthfully.
//!
//! # Fact truth vocabulary (structural, not conventional)
//!
//! [`IamFactStatus`] has exactly four variants —
//! [`Observed`](IamFactStatus::Observed),
//! [`Validated`](IamFactStatus::Validated),
//! [`NotValidated`](IamFactStatus::NotValidated),
//! [`Redacted`](IamFactStatus::Redacted) — matching the owner decision's
//! canonical distinctions exactly. They are structurally, not just
//! conventionally, distinguishable: only [`IamFactStatus::Redacted`] has
//! no plaintext-carrying field at all (its only payload is
//! `sha256_digest: String`) — there is no shared "value" field a caller
//! could populate on a `Redacted` fact and no code path in this module
//! that ever does. A decoded value can never "automatically become"
//! [`IamFactStatus::Validated`]: this module defines no function that
//! constructs that variant at all — only [`redact`] constructs
//! [`IamFactStatus::Redacted`] (from a digest, having already discarded
//! the plaintext), and every other variant is constructed directly by a
//! caller that already decided the label — the *decision* of whether a
//! comparison actually matched belongs to a future receiver/validation
//! frontier, not to this model.
//!
//! # Sensitive-value handling
//!
//! [`redact`] is the one way this module ever produces
//! [`IamFactStatus::Redacted`]: it takes an owned `String`, computes its
//! SHA-256 hex digest, and returns only the digest — the original value
//! is dropped at the end of that function's stack frame and appears
//! nowhere in the returned [`IamFactStatus`]. The digest is a one-way,
//! non-reversible encoding (SHA-256, not base64/hex-of-plaintext or any
//! other reversible transform), deterministic (the same input always
//! produces the same digest, proven by `redact_is_deterministic_for_the_
//! same_input`) and collision-resistant enough that two different
//! observed secrets produce different digests in practice (proven by
//! `different_secrets_produce_different_digests`). This module defines
//! the redaction *vocabulary and helper* only — deciding *which* observed
//! parameter/header names are sensitive (`code`, `access_token`,
//! `refresh_token`, `id_token`, `client_secret`, `cookie`,
//! `authorization`, `password`, …) is HTTP-name-detection policy for the
//! future callback-receiver frontier, not this one; this module never
//! inspects a name string to decide redaction on its own.
//!
//! # Not implemented here
//!
//! Per this frontier's explicit scope, none of the following exist in
//! this module, and nothing here should be read as authorizing them:
//!
//! - Any network listener, HTTP route, or request parsing.
//! - JWT decoding or SAML XML parsing of any kind.
//! - Actual signature/cryptographic verification — V1 may only ever
//!   reach [`IamFactStatus::NotValidated`] for a signature-presence fact;
//!   [`IamFactStatus::Validated`] requires a future, separately
//!   authorized verification frontier.
//! - Any `DomainPersistence` read/write call — see "Persistence shape"
//!   below for the *design*, none of it wired to a store here.
//! - A Web UI, an API route, or any owner-facing surface.
//! - `IamObservationId` or any second IAM identity type.
//!
//! # Persistence shape (design only — nothing here opens a store)
//!
//! A trace's own coarse lifecycle is genuinely "current state" —
//! [`IamTraceState`] is designed to be driven through
//! [`crate::features::domain_state::CurrentState`]/
//! [`crate::features::domain_state::Transition::apply`] exactly as
//! `features/auth_session.rs`'s `AuthSessionState` already is, then
//! persisted via `DomainPersistence::write_current` keyed by
//! `IamTraceId::to_string()` — compare-and-swap, one row per trace,
//! reusing the existing mechanism unmodified.
//!
//! [`IamCallbackObservation`] is immutable once observed and a trace may
//! accumulate more than one — the same shape
//! `spider::utils::evidence::record_evidence` already persists evidence
//! in: a future receiver frontier calls
//! `DomainPersistence::append_history(&trace_id.to_string(), revision,
//! &serde_json::to_vec(&observation)?, SystemTime::now())` directly, with
//! `revision` counting 1, 2, 3, … per additional observation under the
//! same trace — not through `domain_state::HistoryEntry` (which only a
//! `CurrentState::apply` call can construct, and an observation is not a
//! superseded lifecycle state). This mirrors `record_evidence`'s own
//! `append_history(&id.to_string(), 1, &payload, …)` call exactly, generalized
//! to more than one revision per identity. No second database, no second
//! persistence mechanism, no second env var.

use crate::features::domain_state::Transition;
use crate::features::identity::IamTraceId;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// The durable, current lifecycle state of one IAM Callback Inspector
/// trace. See this module's doc comment for why exactly these two states
/// exist and no more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum IamTraceState {
    /// Created; no callback has been observed for this trace yet.
    AwaitingCallback,
    /// At least one callback observation has been recorded for this
    /// trace. Not terminal for the trace's *observations* (more may
    /// still be appended, see the module doc's "Persistence shape") —
    /// terminal only for the [`ReceiveCallback`] transition itself,
    /// which fires exactly once.
    Received,
}

impl IamTraceState {
    /// A short, stable label for this variant — used only for error
    /// reporting (never persisted, never a lifecycle decision input).
    fn kind(&self) -> &'static str {
        match self {
            IamTraceState::AwaitingCallback => "awaiting_callback",
            IamTraceState::Received => "received",
        }
    }
}

/// Why an [`IamTraceState`] transition did not apply. The transition it
/// came from left the trace's current state completely unchanged (see
/// [`crate::features::domain_state::CurrentState::apply`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IamTraceTransitionRejected {
    /// [`ReceiveCallback`] was applied to a trace already in
    /// [`IamTraceState::Received`] — this transition marks only the
    /// *first* observation; it does not reapply for subsequent ones.
    AlreadyReceived,
}

impl std::fmt::Display for IamTraceTransitionRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IamTraceTransitionRejected::AlreadyReceived => write!(
                f,
                "receive_callback does not apply from state {}: the first observation was \
                 already recorded",
                IamTraceState::Received.kind()
            ),
        }
    }
}

impl std::error::Error for IamTraceTransitionRejected {}

/// Mark a trace's *first* callback observation as received. Only applies
/// from [`IamTraceState::AwaitingCallback`]; rejected
/// ([`IamTraceTransitionRejected::AlreadyReceived`]) if the trace already
/// recorded one. Subsequent observations under the same trace are
/// recorded as additional persistence revisions (see this module's
/// "Persistence shape" doc), not by applying this transition again.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReceiveCallback;

impl Transition<IamTraceState> for ReceiveCallback {
    type Rejection = IamTraceTransitionRejected;

    fn apply(&self, current: &IamTraceState) -> Result<IamTraceState, Self::Rejection> {
        match current {
            IamTraceState::AwaitingCallback => Ok(IamTraceState::Received),
            IamTraceState::Received => Err(IamTraceTransitionRejected::AlreadyReceived),
        }
    }
}

/// Provider/protocol-neutral classification of one callback observation.
/// A classification, never a parser — this frontier decodes nothing; a
/// future receiver frontier assigns this value after inspecting the
/// request shape (content type, parameter names present, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum IamProtocolClassification {
    /// No protocol-specific shape was recognized — a plain HTTP GET/POST
    /// with arbitrary query/form/JSON content.
    Generic,
    /// Recognized as an OAuth2 authorization-code-style callback.
    OAuth2,
    /// Recognized as an OpenID Connect callback (OAuth2 plus an ID
    /// token).
    Oidc,
    /// Recognized as a SAML 2.0 Response/Assertion delivery.
    Saml,
}

/// Canonical truth-state of one observed fact. Exactly the four
/// distinctions the owner decision requires, structurally
/// distinguishable — see this module's doc comment for the full
/// rationale. Every variant name mirrors the owner decision's own
/// vocabulary (`OBSERVED`/`VALIDATED`/`NOT_VALIDATED`/`REDACTED`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum IamFactStatus {
    /// Seen in the callback, retained in full; no comparison or
    /// verification against any expected value was attempted.
    Observed {
        /// The observed value, verbatim.
        value: String,
    },
    /// Compared against an expected value, or cryptographically
    /// verified, and confirmed correct/authentic. No function in this
    /// module ever constructs this variant — see the module doc's "Fact
    /// truth vocabulary" section.
    Validated {
        /// The validated value, verbatim.
        value: String,
    },
    /// A comparison or verification was attempted but did not succeed,
    /// or no verification material was available to attempt one at all
    /// (e.g. a JWT/SAML signature in V1, which this project excludes
    /// cryptographic verification for — see the module doc). Still
    /// retained in full: this variant is not itself secret-shaped, and
    /// the operator needs to see exactly what failed or went unchecked.
    NotValidated {
        /// The unvalidated value, verbatim.
        value: String,
    },
    /// The observed value was flagged sensitive and is never retained in
    /// plaintext — only a one-way digest, for correlation. Structurally
    /// incapable of carrying the original value: this is the variant's
    /// only field.
    Redacted {
        /// Hex-encoded SHA-256 digest of the original value. The
        /// original value itself is not retained anywhere.
        sha256_digest: String,
    },
}

/// Compute a redacted fact from a sensitive plaintext value. The only way
/// this module ever produces [`IamFactStatus::Redacted`] — see the module
/// doc's "Sensitive-value handling" section for the full non-reversibility
/// and determinism proof. `value` is consumed and dropped at the end of
/// this function; it does not escape into the returned [`IamFactStatus`].
pub fn redact(value: String) -> IamFactStatus {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(value.as_bytes());
    IamFactStatus::Redacted {
        sha256_digest: format!("{digest:x}"),
    }
}

/// One named fact extracted from a callback observation — a parameter, a
/// header, or a decoded claim. Provider/protocol-neutral: this type
/// carries no OAuth/OIDC/SAML-specific field of its own. Protocol-specific
/// meaning (this is a `nonce`, this is a SAML `NameID`, …) is carried
/// entirely in `name`, a plain string a future receiver/decoder frontier
/// chooses — this model imposes no fixed vocabulary of names.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct IamFact {
    /// The fact's name, e.g. `"state"`, `"code"`, `"NameID"`, `"exp"`.
    /// Freeform — this model defines no closed vocabulary of names.
    pub name: String,
    /// This fact's truth-state and, depending on the variant, its value
    /// or digest.
    pub status: IamFactStatus,
}

impl IamFact {
    /// Construct an [`IamFactStatus::Observed`] fact.
    pub fn observed(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: IamFactStatus::Observed {
                value: value.into(),
            },
        }
    }

    /// Construct an [`IamFactStatus::NotValidated`] fact.
    pub fn not_validated(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: IamFactStatus::NotValidated {
                value: value.into(),
            },
        }
    }

    /// Construct a [`IamFactStatus::Redacted`] fact via [`redact`] — the
    /// original `value` never enters the returned [`IamFact`].
    pub fn redacted(name: impl Into<String>, value: String) -> Self {
        Self {
            name: name.into(),
            status: redact(value),
        }
    }
}

/// One immutable, provider/protocol-neutral record of a single callback
/// delivery observed for one [`IamTraceId`]. See the module doc's
/// "Persistence shape" for how a future receiver frontier durably records
/// more than one of these per trace.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct IamCallbackObservation {
    /// The trace this observation belongs to. Every observation is
    /// associated with exactly one trace — this field is that
    /// association, never inferred from ordering or storage position.
    pub trace: IamTraceId,
    /// The HTTP method the callback arrived as (e.g. `"GET"`, `"POST"`).
    /// Populated by a future receiver frontier; this model only defines
    /// the field.
    pub method: String,
    /// The callback target/path the request was delivered to.
    pub target: String,
    /// Unix epoch milliseconds when this observation was recorded.
    pub observed_at_unix_ms: u64,
    /// This observation's protocol classification.
    pub protocol: IamProtocolClassification,
    /// Every fact extracted from this observation, each independently
    /// labeled per the truth vocabulary above.
    pub facts: Vec<IamFact>,
}

impl IamCallbackObservation {
    /// Construct a new observation for `trace`. `observed_at_unix_ms`
    /// must be supplied by the caller (this model performs no I/O,
    /// including reading the system clock) — a future receiver frontier
    /// supplies the real time it observed the request.
    pub fn new(
        trace: IamTraceId,
        method: impl Into<String>,
        target: impl Into<String>,
        observed_at_unix_ms: u64,
        protocol: IamProtocolClassification,
        facts: Vec<IamFact>,
    ) -> Self {
        Self {
            trace,
            method: method.into(),
            target: target.into(),
            observed_at_unix_ms,
            protocol,
            facts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::domain_state::CurrentState;

    // ---------------------------------------------------------------
    // 3/4: transition semantics
    // ---------------------------------------------------------------

    #[test]
    fn allowed_transition_succeeds() {
        let current = CurrentState::new(IamTraceId::new(), IamTraceState::AwaitingCallback);
        let applied = current.apply(&ReceiveCallback).expect("must succeed");
        assert_eq!(applied.current.state(), &IamTraceState::Received);
    }

    #[test]
    fn invalid_transition_fails_closed_and_leaves_state_unchanged() {
        let current = CurrentState::new(IamTraceId::new(), IamTraceState::Received);
        let (unchanged, rejection) = current
            .apply(&ReceiveCallback)
            .expect_err("must be rejected");
        assert_eq!(rejection, IamTraceTransitionRejected::AlreadyReceived);
        assert_eq!(unchanged.state(), &IamTraceState::Received);
    }

    // ---------------------------------------------------------------
    // 5: observation <-> trace association
    // ---------------------------------------------------------------

    #[test]
    fn observation_is_associated_with_exactly_one_trace() {
        let trace = IamTraceId::new();
        let other = IamTraceId::new();
        let observation = IamCallbackObservation::new(
            trace,
            "GET",
            "/iam/callback/abc",
            1_700_000_000_000,
            IamProtocolClassification::Generic,
            vec![],
        );
        assert_eq!(observation.trace, trace);
        assert_ne!(observation.trace, other);
    }

    // ---------------------------------------------------------------
    // 6/7: OBSERVED != VALIDATED, NOT_VALIDATED stays explicit
    // ---------------------------------------------------------------

    #[test]
    fn observed_is_not_validated() {
        let fact = IamFact::observed("state", "xyz123");
        assert_ne!(
            fact.status,
            IamFactStatus::Validated {
                value: "xyz123".to_string()
            }
        );
        assert!(matches!(fact.status, IamFactStatus::Observed { .. }));
    }

    #[test]
    fn not_validated_remains_explicit_and_distinct_from_validated() {
        let fact = IamFact::not_validated("jwt_signature", "present");
        assert!(matches!(fact.status, IamFactStatus::NotValidated { .. }));
        assert_ne!(
            fact.status,
            IamFactStatus::Validated {
                value: "present".to_string()
            }
        );
    }

    // ---------------------------------------------------------------
    // 8/9/10/11: redaction and digest properties
    // ---------------------------------------------------------------

    #[test]
    fn redacted_fact_cannot_expose_original_plaintext_through_serialization_or_debug() {
        const SECRET: &str = "super-secret-authorization-code-sentinel";
        let fact = IamFact::redacted("code", SECRET.to_string());

        // Structural: the Redacted variant has no field capable of
        // holding SECRET at all.
        match &fact.status {
            IamFactStatus::Redacted { sha256_digest } => {
                assert_ne!(sha256_digest, SECRET);
                assert!(!sha256_digest.contains(SECRET));
            }
            other => panic!("expected Redacted, got {other:?}"),
        }

        // Debug output never leaks the secret.
        let debug = format!("{fact:?}");
        assert!(!debug.contains(SECRET));

        // Serialized durable form never leaks the secret.
        #[cfg(feature = "serde")]
        {
            let json = serde_json::to_string(&fact).unwrap();
            assert!(!json.contains(SECRET));
        }
    }

    #[test]
    fn redact_is_deterministic_for_the_same_input() {
        let a = redact("same-value".to_string());
        let b = redact("same-value".to_string());
        assert_eq!(a, b);
    }

    #[test]
    fn different_secrets_produce_different_digests() {
        let a = redact("secret-one".to_string());
        let b = redact("secret-two".to_string());
        assert_ne!(a, b);
    }

    #[test]
    fn redact_never_returns_a_reversible_encoding() {
        // The digest must not simply be the plaintext re-encoded
        // (hex/base64 of the original bytes) — it must be a genuine
        // one-way hash, verified against a known SHA-256 test vector.
        let IamFactStatus::Redacted { sha256_digest } = redact("abc".to_string()) else {
            panic!("redact must return Redacted");
        };
        assert_eq!(
            sha256_digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn observation_with_redacted_fact_round_trips_and_stays_redacted() {
        let trace = IamTraceId::new();
        let observation = IamCallbackObservation::new(
            trace,
            "POST",
            "/iam/callback/abc",
            1_700_000_000_000,
            IamProtocolClassification::OAuth2,
            vec![
                IamFact::observed("state", "opaque-correlation-value"),
                IamFact::redacted("code", "authorization-code-sentinel".to_string()),
            ],
        );
        let json = serde_json::to_string(&observation).unwrap();
        assert!(!json.contains("authorization-code-sentinel"));
        let restored: IamCallbackObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, observation);
        assert!(matches!(
            restored.facts[1].status,
            IamFactStatus::Redacted { .. }
        ));
    }
}
