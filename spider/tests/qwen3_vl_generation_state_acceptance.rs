//! Architecture acceptance for request-isolated Candle Qwen3-VL state.

use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn source() -> String {
    fs::read_to_string(root().join("spider/src/features/qwen3_vl_generation.rs")).unwrap()
}

#[test]
fn design_b_reuses_only_immutable_builder_backend() {
    let source = source();
    assert!(source.contains("weights: VarBuilder<'static>"));
    assert!(source.contains("self.weights.clone()"));
    assert!(source.contains("Qwen3VLModel::new(&self.config"));
    assert!(source.contains("lock_owned().await"));
    assert!(!source.contains("model: Mutex<Qwen3VLModel>"));
}

#[test]
fn each_request_owns_one_fresh_non_clonable_model() {
    let source = source();
    assert!(source.contains("pub struct Qwen3VlGenerationSession"));
    assert!(source.contains("model: Qwen3VLModel"));
    assert!(source.contains("_serialized_permit: tokio::sync::OwnedMutexGuard<()>"));
    assert!(!source.contains("impl Clone for Qwen3VlGenerationSession"));
    assert!(!source.contains("Arc<Qwen3VLModel>"));
}

#[test]
fn session_has_no_cache_escape_or_return_to_factory() {
    let source = source();
    for forbidden in [
        "reset_kv_cache",
        "pub fn cache",
        "return_model",
        "recycle_session",
        "session_pool",
    ] {
        assert!(!source.contains(forbidden), "state escape: {forbidden}");
    }
}

#[test]
fn construction_has_no_network_or_artifact_resolution() {
    let source = source();
    for forbidden in ["reqwest", "wreq", "hf_hub", "download", "ArtifactReference"] {
        assert!(
            !source.contains(forbidden),
            "network ownership: {forbidden}"
        );
    }
}

#[test]
fn cleanup_is_infallible_discard_for_every_termination_path() {
    let source = source();
    assert!(source.contains("Drop is infallible state discard"));
    assert!(source.contains("cancellation_and_deadline_drop_request_state_and_permit"));
    assert!(!source.contains("cleanup().unwrap()"));
}

#[test]
fn actual_candle_model_proves_a_then_b_equals_fresh_b() {
    let source = source();
    assert!(source.contains("actual_candle_qwen_request_b_matches_fresh_session_b"));
    assert!(source.contains("let _ = text_forward(&request_a, 2, 1)"));
    assert!(source.contains("assert_eq!(output_b, fresh_output_b)"));
}

#[test]
fn unload_consumes_factory_without_cache_access() {
    let source = source();
    assert!(source.contains("pub fn unload(self)"));
    assert!(!source.contains("kv_cache.lock"));
}
