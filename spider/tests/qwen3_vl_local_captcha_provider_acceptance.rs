//! Acceptance contract for the executable-unqualified local Qwen3-VL provider.

use std::{fs, path::PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn provider() -> String {
    fs::read_to_string(root().join("src/features/qwen3_vl_captcha.rs")).unwrap()
}

#[test]
fn provider_has_stable_identity_and_explicit_registry_proof() {
    let source = provider();
    assert!(source.contains("CaptchaProviderId::QWEN3_VL_LOCAL"));
    assert!(source.contains("registry.resolve(CaptchaProviderId::QWEN3_VL_LOCAL)"));
}

#[test]
fn all_shapes_are_executable_but_empirically_unqualified() {
    let source = provider();
    for kind in ["ImageGridSelection", "HorizontalOffset", "PointSelection"] {
        assert!(source.contains(kind));
    }
    assert!(source.contains("CaptchaCapabilityQualification::ExecutableUnqualified"));
    assert!(!source.contains("EmpiricallyQualified"));
}

#[test]
fn provider_uses_only_canonical_runtime_and_structured_generation() {
    let source = provider();
    assert!(source.contains("Qwen3VlCpuRuntime"));
    assert!(source.contains("generate_structured("));
    for forbidden in [
        "Qwen3VLModel",
        "VarBuilder",
        "process_image(",
        "constrained_token(",
    ] {
        assert!(!source.contains(forbidden));
    }
}

#[test]
fn materialized_grid_supplies_identity_geometry_and_empty_policy() {
    let source = provider();
    assert!(source.contains(".image_grid()"));
    assert!(source.contains("cell.choice_id()"));
    assert!(source.contains("cell.geometry()"));
    assert!(source.contains("grid.empty_selection_valid()"));
}

#[test]
fn strict_parsing_rejects_unknown_duplicate_and_malformed_results() {
    let source = provider();
    assert!(source.contains("serde(deny_unknown_fields)"));
    assert!(source.contains("!known.contains"));
    assert!(source.contains("!observed.insert"));
    assert!(source.contains("InvalidProviderResponse"));
}

#[test]
fn coordinates_are_finite_and_original_space_bounded() {
    let source = provider();
    assert!(source.contains("metadata.original_dimensions"));
    assert!(source.contains("output.x.is_finite()"));
    assert!(source.contains("output.y.is_finite()"));
}

#[test]
fn local_provenance_is_complete_and_transport_neutral() {
    let source = provider();
    for fact in [
        "model_revision",
        "runtime_identity",
        "processor_identity",
        "challenge_kind",
        "prompt_grammar_identity",
        "elapsed",
        "succeeded",
    ] {
        assert!(source.contains(fact));
    }
    assert!(!source.contains("BackendProvenance"));
}

#[test]
fn real_offline_provider_chain_is_a_required_ignored_host_test() {
    let source = provider();
    assert!(source.contains("real_provider_registry_runtime_and_strict_outcome"));
    assert!(source.contains("QWEN3_VL_MINIMUM_RAM_BYTES - 1"));
    assert!(source.contains("provider.unload()"));
}
