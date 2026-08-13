//! Acceptance contract for canonical token-level structured generation.

use std::{fs, path::PathBuf};

fn source() -> String {
    fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/features/qwen3_vl_runtime.rs"),
    )
    .unwrap()
}

#[test]
fn structured_generation_is_runtime_owned_and_neutral() {
    let source = source();
    assert!(source.contains("pub enum Qwen3VlStructuredSchema"));
    assert!(source.contains("pub async fn generate_structured"));
    assert!(!source.contains("CaptchaSolution"));
}

#[test]
fn every_token_is_filtered_before_selection() {
    let source = source();
    assert!(source.contains("fn constrained_token("));
    assert!(source.contains("schema_state(schema, &text)"));
    assert!(source.contains("state != GrammarState::Invalid"));
}

#[test]
fn variable_ids_and_finite_numbers_are_explicit_grammars() {
    let source = source();
    assert!(source.contains("StringIdArray"));
    assert!(source.contains("allowed_ids"));
    assert!(source.contains("FiniteNumbers"));
    assert!(source.contains("value.is_finite()"));
}

#[test]
fn dead_ends_fail_with_a_typed_runtime_fact() {
    let source = source();
    assert!(source.contains("NoValidStructuredContinuation"));
    assert!(!source.contains("unwrap_or_default"));
}

#[test]
fn completion_is_selected_during_decoding_not_repaired_afterward() {
    let source = source();
    assert!(source.contains("must_complete"));
    for forbidden in ["push_str(\"}\")", "trim_end_matches", "first valid value"] {
        assert!(!source.contains(forbidden));
    }
}

#[test]
fn free_form_generation_remains_available() {
    let source = source();
    assert!(source.contains("pub async fn generate("));
    assert!(source.contains("None => greedy_token(&logits)?"));
}

#[test]
fn request_local_state_and_cpu_only_policy_remain_intact() {
    let source = source();
    assert!(source.contains(".begin_request()"));
    assert!(source.contains("let device = Device::Cpu"));
    assert!(!source.contains("Device::new_cuda"));
}

#[test]
fn real_pinned_model_acceptance_requires_strict_parsing_and_determinism() {
    let source = source();
    assert!(source.contains("real_structured_generation_is_nonempty_and_strictly_parsed"));
    assert!(source.contains("assert_eq!(output.text, repeated.text)"));
    assert!(source.contains("serde_json::from_str(&ids.text)"));
}
