//! Canonical content/transform lineage: `source input → transformation →
//! output`.
//!
//! Track 6 of the roadmap. Records the *fact* that a transformation
//! happened — which input bytes, which transformation, which output
//! bytes — durably, immutably, and (per this frontier's requirement)
//! without inventing a generic "content ID" or shadowing the existing,
//! unrelated `Fingerprint` name. This module performs no content
//! transformation itself; it has no HTML/markdown/text conversion logic
//! of any kind. It only records what a transformation elsewhere (the
//! real [`transform_content_input`]-shaped pipeline) already did.
//!
//! [`transform_content_input`]: https://docs.rs/spider_transformations/latest/spider_transformations/transformation/content/fn.transform_content_input.html
//!
//! # Fingerprint naming reconciliation (required before any new name)
//!
//! `spider::configuration::Fingerprint` (re-exported from the
//! `spider_fingerprint` crate) already exists and means something
//! entirely different: which *browser anti-detection stealth spoofing*
//! profile a Chrome automation session uses (`Basic` / `NativeGPU` /
//! `None` — WebGL/GPU spoofing configuration). It has no notion of
//! content, transformation, or provenance — it configures how a browser
//! *presents itself* to a remote site, not what happened to fetched
//! content afterward. This module does not redefine it, shadow it, or
//! import it, and never will; the two concepts share only an English
//! word, not a domain. Source evidence does not support treating them as
//! the same concept (this frontier's rule #7), so the content/transform
//! identity type introduced here is named [`TransformLineageId`] —
//! domain-qualified, never bare `Fingerprint`.
//!
//! # Why a new identity type, and why it is not in `features/identity.rs`
//!
//! `EvidenceId`/`WatchId`/`AuthSessionId` are all randomly minted — 16
//! bytes of entropy naming a *thing that was created*, where two
//! separately created things must never collide. A transform-lineage
//! fact is different in kind: this frontier explicitly requires "same
//! input + same transformation identity produces stable lineage
//! identity" — the *opposite* of random minting. [`TransformLineageId`]
//! is therefore **content-addressed**: a deterministic SHA-256 (via the
//! existing, reused [`crate::utils::evidence::sha256_hex`]) of the
//! `(input hash, transformation identity, output hash)` triple. The same
//! triple always produces the same id; a different triple never
//! collides with a different one (short of an actual SHA-256 collision).
//! This is a materially different construction than
//! `features/identity.rs`'s three types, so it lives in its own module
//! rather than blending a second minting strategy into that one's
//! tightly-scoped "3 identity types, one entropy source" contract.
//!
//! # The three-link chain
//!
//! - **Source input** — named by its SHA-256 hash (reusing
//!   [`crate::utils::evidence::sha256_hex`] exactly), plus, when the
//!   input is already durable evidence, an [`crate::utils::evidence::EvidenceRef`]
//!   pointing at it — a reference, never a duplicate of the evidence
//!   payload (Track 4's own stated design intent for `EvidenceRef`,
//!   realized here for the first time).
//! - **Transformation** — named by [`TransformationIdentity`], a SHA-256
//!   of a caller-supplied, deterministic description of the
//!   transformation actually applied (e.g. a real
//!   `spider_transformations::TransformConfig`'s `Debug` output). This
//!   module has no dependency on any specific transformation library —
//!   `spider_transformations` is a dependency of the interface crates
//!   (`spider_cli`/`spider_mcp`), not of canonical `spider` — so it
//!   accepts the description as an opaque string rather than importing
//!   a concrete config type, exactly the same "neutral seam, caller
//!   supplies the concrete specifics" pattern
//!   [`crate::features::domain_persistence`] already established.
//! - **Output** — named by its SHA-256 hash, the same way the input is.
//!
//! [`record_lineage`] requires real input and output bytes — there is no
//! code path that accepts a caller-supplied hash string instead, so it
//! is structurally impossible to fabricate a lineage record for content
//! that was never actually hashed (rule #6).
//!
//! # Persistence
//!
//! Through [`crate::features::domain_persistence::DomainPersistence`]'s
//! append-only historical semantics only (`append_history`) — never
//! `write_current`. A lineage fact has no "current state" to replace,
//! exactly like Track 4's evidence ledger and unlike Track 5's
//! authenticated sessions. Every record uses the fixed revision `1`
//! (the only record its content-addressed id will ever need). Because
//! the id is a deterministic function of the record's own content,
//! re-recording an *identical* fact — the same input, transformation,
//! and output, presumably from re-running the same transformation again
//! — is not a conflict: [`record_lineage`] treats
//! `PersistenceError::HistoryAlreadyExists` as success (returning the
//! same id), since a collision on a content-addressed key can only mean
//! the fact was already recorded, never that two different facts were
//! forced together. A genuinely *different* input, transformation, or
//! output always hashes to a different id, so different facts never
//! silently collapse into one record.

use crate::features::domain_persistence::{DomainPersistence, PersistenceError};
use crate::utils::evidence::{sha256_hex, EvidenceRef};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::SystemTime;

/// Deterministic identity of one transformation configuration/description.
/// A SHA-256 of the caller-supplied `description` — this module has no
/// opinion on what that description contains beyond "it deterministically
/// identifies the transformation that was applied."
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TransformationIdentity(String);

impl TransformationIdentity {
    /// Derive the identity of a transformation from its description.
    /// Equal descriptions always produce equal identities.
    pub fn of(description: &str) -> Self {
        Self(sha256_hex(description.as_bytes()))
    }

    /// Borrow the underlying digest.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TransformationIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Content-addressed identity of one transform-lineage record. See this
/// module's doc comment for why this is deterministic rather than
/// randomly minted.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TransformLineageId(String);

impl TransformLineageId {
    /// Wire-format prefix.
    pub const PREFIX: &'static str = "lineage_";

    /// Deterministically derive the identity of the lineage fact
    /// `(input_hash, transformation, output_hash)`. Never reads
    /// `recorded_at` or any other incidental metadata — only the triple
    /// that actually defines what happened is part of the identity, so
    /// running the exact same transformation on the exact same input
    /// again (at a different time) reproduces the exact same id.
    fn derive(
        input_hash: &str,
        transformation: &TransformationIdentity,
        output_hash: &str,
    ) -> Self {
        let joined = format!("lineage-v1|{input_hash}|{transformation}|{output_hash}");
        Self(format!("{}{}", Self::PREFIX, sha256_hex(joined.as_bytes())))
    }

    /// Borrow the wire-format string (`lineage_<sha256 hex>`).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TransformLineageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One immutable record of `source input → transformation → output`.
///
/// The only way to obtain one is [`record_lineage`]'s return path or
/// [`read_lineage`] reading one back — there is no public constructor
/// that lets a caller assemble a record (and therefore its id) from
/// hashes it did not actually compute from real bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransformLineageRecord {
    input_hash: String,
    input_evidence: Option<EvidenceRef>,
    transformation: TransformationIdentity,
    output_hash: String,
    recorded_at: SystemTime,
}

impl TransformLineageRecord {
    /// SHA-256 of the exact input bytes the transformation consumed.
    pub fn input_hash(&self) -> &str {
        &self.input_hash
    }

    /// The durable evidence record the input came from, when the caller
    /// supplied one. `None` never implies the input was fabricated —
    /// only that it is not (yet, or ever) recorded as durable evidence
    /// separately from this lineage fact.
    pub fn input_evidence(&self) -> Option<EvidenceRef> {
        self.input_evidence
    }

    /// The identity of the transformation that was applied.
    pub fn transformation(&self) -> &TransformationIdentity {
        &self.transformation
    }

    /// SHA-256 of the exact output bytes the transformation produced.
    pub fn output_hash(&self) -> &str {
        &self.output_hash
    }

    /// When this lineage fact was recorded.
    pub fn recorded_at(&self) -> SystemTime {
        self.recorded_at
    }

    /// This record's content-addressed identity.
    pub fn id(&self) -> TransformLineageId {
        TransformLineageId::derive(&self.input_hash, &self.transformation, &self.output_hash)
    }
}

/// Failure recording or reading a lineage fact. Storage-shaped only.
#[derive(Debug)]
pub enum TransformLineageError {
    /// A backend/persistence failure unrelated to lineage identity
    /// (`HistoryAlreadyExists` for a matching content-addressed id is
    /// not surfaced here — see [`record_lineage`]).
    Persistence(PersistenceError),
    /// The record could not be encoded/decoded.
    Serialization(serde_json::Error),
}

impl fmt::Display for TransformLineageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransformLineageError::Persistence(error) => write!(f, "transform lineage: {error}"),
            TransformLineageError::Serialization(error) => {
                write!(f, "transform lineage: record serialization failed: {error}")
            }
        }
    }
}

impl std::error::Error for TransformLineageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TransformLineageError::Persistence(error) => Some(error),
            TransformLineageError::Serialization(error) => Some(error),
        }
    }
}

/// Record that `output_bytes` were produced from `input_bytes` by the
/// transformation `transformation_description` names — durably,
/// immutably, through [`DomainPersistence::append_history`].
///
/// `input_bytes`/`output_bytes` are hashed here, from the real bytes
/// given — there is no way to call this with a pre-computed hash string
/// instead, so a lineage record can never be fabricated for content that
/// was never actually observed (this frontier's rule #6).
///
/// `input_evidence`, when supplied, must be the [`EvidenceRef`] for the
/// durable evidence record `input_bytes` actually came from — this
/// function does not verify that relationship (it has no I/O capability
/// to do so without a redundant read); callers should only supply a ref
/// they know is correct.
///
/// Returns the record's [`TransformLineageId`] on success — including
/// when an identical fact (the same input/transformation/output triple)
/// was already recorded: since the id is a deterministic function of
/// that triple, a collision can only mean "already recorded," never a
/// forced merge of two different facts (see this module's doc comment).
pub async fn record_lineage(
    store: &DomainPersistence,
    input_bytes: &[u8],
    input_evidence: Option<EvidenceRef>,
    transformation_description: &str,
    output_bytes: &[u8],
) -> Result<TransformLineageId, TransformLineageError> {
    let record = TransformLineageRecord {
        input_hash: sha256_hex(input_bytes),
        input_evidence,
        transformation: TransformationIdentity::of(transformation_description),
        output_hash: sha256_hex(output_bytes),
        recorded_at: SystemTime::now(),
    };
    let id = record.id();

    let payload = serde_json::to_vec(&record).map_err(TransformLineageError::Serialization)?;

    match store
        .append_history(id.as_str(), 1, &payload, record.recorded_at)
        .await
    {
        Ok(()) => Ok(id),
        Err(PersistenceError::HistoryAlreadyExists) => Ok(id),
        Err(other) => Err(TransformLineageError::Persistence(other)),
    }
}

/// Read back the lineage record named by `id`, exactly as
/// [`record_lineage`] wrote it. `Ok(None)` if nothing has ever been
/// recorded for this identity.
pub async fn read_lineage(
    store: &DomainPersistence,
    id: &TransformLineageId,
) -> Result<Option<TransformLineageRecord>, TransformLineageError> {
    let history = store
        .read_history(id.as_str())
        .await
        .map_err(TransformLineageError::Persistence)?;

    match history.into_iter().next() {
        Some((_revision, payload, _recorded_at)) => {
            let record =
                serde_json::from_slice(&payload).map_err(TransformLineageError::Serialization)?;
            Ok(Some(record))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::identity::EvidenceId;

    #[test]
    fn same_input_and_transformation_produce_stable_lineage_identity() {
        let a = TransformLineageRecord {
            input_hash: sha256_hex(b"raw html"),
            input_evidence: None,
            transformation: TransformationIdentity::of("markdown:readability=true"),
            output_hash: sha256_hex(b"# markdown"),
            recorded_at: SystemTime::UNIX_EPOCH,
        };
        let b = TransformLineageRecord {
            // Different recorded_at — identity must not depend on it.
            recorded_at: SystemTime::now(),
            ..a.clone()
        };
        assert_eq!(a.id(), b.id());
    }

    #[test]
    fn different_input_does_not_collapse_identity() {
        let base = TransformationIdentity::of("markdown:readability=true");
        let a =
            TransformLineageId::derive(&sha256_hex(b"input a"), &base, &sha256_hex(b"same output"));
        let b =
            TransformLineageId::derive(&sha256_hex(b"input b"), &base, &sha256_hex(b"same output"));
        assert_ne!(a, b);
    }

    #[test]
    fn different_transformation_does_not_collapse_identity() {
        let input_hash = sha256_hex(b"same input");
        let output_hash = sha256_hex(b"same output");
        let a = TransformLineageId::derive(
            &input_hash,
            &TransformationIdentity::of("markdown"),
            &output_hash,
        );
        let b = TransformLineageId::derive(
            &input_hash,
            &TransformationIdentity::of("text"),
            &output_hash,
        );
        assert_ne!(a, b);
    }

    #[test]
    fn different_output_does_not_collapse_identity() {
        let input_hash = sha256_hex(b"same input");
        let transformation = TransformationIdentity::of("markdown");
        let a = TransformLineageId::derive(&input_hash, &transformation, &sha256_hex(b"output 1"));
        let b = TransformLineageId::derive(&input_hash, &transformation, &sha256_hex(b"output 2"));
        assert_ne!(a, b);
    }

    #[test]
    fn lineage_id_has_the_expected_wire_prefix() {
        let id = TransformLineageId::derive(
            &sha256_hex(b"x"),
            &TransformationIdentity::of("y"),
            &sha256_hex(b"z"),
        );
        assert!(id.as_str().starts_with(TransformLineageId::PREFIX));
        assert_eq!(id.to_string(), id.as_str());
    }

    mod ledger {
        use super::*;

        #[tokio::test]
        async fn record_then_read_back_truthfully() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let id = record_lineage(
                &store,
                b"<html>raw</html>",
                None,
                "markdown:readability=true",
                b"# raw",
            )
            .await
            .unwrap();

            let record = read_lineage(&store, &id).await.unwrap().unwrap();
            assert_eq!(record.input_hash(), sha256_hex(b"<html>raw</html>"));
            assert_eq!(record.output_hash(), sha256_hex(b"# raw"));
            assert_eq!(
                *record.transformation(),
                TransformationIdentity::of("markdown:readability=true")
            );
            assert_eq!(record.id(), id);
        }

        #[tokio::test]
        async fn recording_the_identical_fact_twice_is_idempotent_not_a_conflict() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let first = record_lineage(&store, b"input", None, "markdown", b"output")
                .await
                .unwrap();
            let second = record_lineage(&store, b"input", None, "markdown", b"output")
                .await
                .unwrap();
            assert_eq!(first, second);

            // Only one historical record exists — the second call did not
            // append a duplicate.
            let history = store.read_history(first.as_str()).await.unwrap();
            assert_eq!(history.len(), 1);
        }

        #[tokio::test]
        async fn different_facts_never_silently_collapse() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let a = record_lineage(&store, b"input a", None, "markdown", b"output")
                .await
                .unwrap();
            let b = record_lineage(&store, b"input b", None, "markdown", b"output")
                .await
                .unwrap();
            assert_ne!(a, b);
            assert!(read_lineage(&store, &a).await.unwrap().is_some());
            assert!(read_lineage(&store, &b).await.unwrap().is_some());
        }

        #[tokio::test]
        async fn read_lineage_of_unknown_id_is_none() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let bogus = TransformLineageId::derive(
                &sha256_hex(b"never recorded"),
                &TransformationIdentity::of("markdown"),
                &sha256_hex(b"never recorded either"),
            );
            assert!(read_lineage(&store, &bogus).await.unwrap().is_none());
        }

        #[tokio::test]
        async fn evidence_ref_is_stored_by_reference_not_duplicated() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let evidence_id = EvidenceId::new();
            let evidence_ref = EvidenceRef::new(evidence_id);

            let id = record_lineage(
                &store,
                b"fetched html",
                Some(evidence_ref),
                "markdown",
                b"# fetched",
            )
            .await
            .unwrap();

            let record = read_lineage(&store, &id).await.unwrap().unwrap();
            // The lineage record carries only the reference (16 bytes of
            // identity) — not a copy of any evidence payload/content.
            assert_eq!(record.input_evidence(), Some(evidence_ref));
            assert_eq!(record.input_evidence().unwrap().id(), evidence_id);
        }

        #[tokio::test]
        async fn history_is_append_only() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let id = record_lineage(&store, b"input", None, "markdown", b"output")
                .await
                .unwrap();
            // No update/delete surface exists on DomainPersistence at all
            // (Track 3's own guarantee) — reading twice returns the same,
            // untouched record.
            let first_read = read_lineage(&store, &id).await.unwrap().unwrap();
            let second_read = read_lineage(&store, &id).await.unwrap().unwrap();
            assert_eq!(first_read, second_read);
        }
    }
}
