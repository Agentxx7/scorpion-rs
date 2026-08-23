//! Acceptance contract for canonical CAPTCHA corpus governance.

use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn source() -> String {
    fs::read_to_string(root().join("spider/src/features/captcha_evaluation_corpus.rs")).unwrap()
}

#[test]
fn provider_neutral_corpus_vocabulary_exists_once() {
    let source = source();
    for model in [
        "CorpusId",
        "CorpusVersion",
        "CorpusSourceProvenance",
        "CorpusAsset",
        "IndependentAnnotation",
        "CorpusAdjudication",
        "CaptchaCorpusCase",
        "CaptchaCorpusDraft",
        "FrozenCaptchaCorpus",
    ] {
        assert_eq!(
            source.matches(&format!("pub struct {model}")).count(),
            1,
            "{model}"
        );
    }
}

#[test]
fn all_three_challenge_families_have_distinct_truth_shapes() {
    let source = source();
    for shape in ["ImageGrid", "HorizontalOffset", "PointSelection"] {
        assert!(source.contains(shape), "missing {shape}");
    }
    assert!(source.contains("initial_piece_x: Option<f64>"));
    assert!(source.contains("displacement_px: f64"));
    assert!(source.contains("accepted_region: Option<[f64; 4]>"));
}

#[test]
fn freeze_requires_size_rights_independence_and_adjudication() {
    let source = source();
    assert!(source.contains("const MINIMUM_CASES: usize = 200"));
    assert!(source.contains("case.independent_annotations.len() < 2"));
    assert!(source.contains("annotators.contains(case.adjudication.adjudicator"));
    assert!(source.contains("disagreement != case.adjudication.disagreement_recorded"));
    assert!(source.contains("rights_review_record"));
}

#[test]
fn development_and_sealed_qualification_splits_are_explicit() {
    let source = source();
    assert!(source.contains("Development"));
    assert!(source.contains("Qualification"));
    assert!(source.contains("qualification_seal_record"));
    assert!(source.contains("assigned.len() != cases.len()"));
}

#[test]
fn complete_identity_binds_every_governed_record() {
    let source = source();
    for bound in [
        "source_revision",
        "provenance_sha256",
        "asset.sha256",
        "annotation.annotation_sha256",
        "adjudication.adjudication_sha256",
        "attestation.attestation_sha256",
        "threshold_policy_id",
    ] {
        assert!(source.contains(bound), "identity omits {bound}");
    }
    assert!(source.contains("format!(\"{:x}\", digest.finalize())"));
}

#[test]
fn drafts_cannot_masquerade_as_frozen_evaluation_material() {
    let source = source();
    assert!(source.contains("pub fn freeze("));
    assert!(source.contains("pub struct FrozenCaptchaCorpus"));
    assert!(source.contains("draft: CaptchaCorpusDraft"));
    assert!(!source.contains("impl From<CaptchaCorpusDraft> for FrozenCaptchaCorpus"));
    assert!(!source.contains("pub draft: CaptchaCorpusDraft"));
}

#[test]
fn corpus_governance_has_no_provider_or_transport_execution() {
    let source = source();
    for forbidden in [
        "CaptchaProviderId",
        "CaptchaSolveOutcome",
        "PaliGemma",
        "OpenAi",
        "Gemini",
        "reqwest::",
        "wreq::",
        "CanonicalExecutor",
        "CrawlerRequest",
    ] {
        assert!(!source.contains(forbidden), "authority leak: {forbidden}");
    }
}
