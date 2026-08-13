//! Architecture acceptance for the offline Qwen3-VL CPU/F32 runtime.

use std::{fs, path::PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn runtime() -> String {
    fs::read_to_string(root().join("spider/src/features/qwen3_vl_runtime.rs")).unwrap()
}

#[test]
fn runtime_is_canonical_installation_only_and_offline() {
    let source = runtime();
    assert!(source.contains("installation.reverify()?"));
    assert!(source.contains(".artifact_path(Path::new(name))"));
    assert!(!source.contains("hf_hub"));
    assert!(!source.contains("reqwest::"));
    assert!(!source.contains("wreq::"));
    assert!(!source.contains("download("));
}

#[test]
fn exact_manifest_and_runtime_identity_are_pinned() {
    let source = runtime();
    for required in [
        "model.safetensors",
        "config.json",
        "tokenizer.json",
        "tokenizer_config.json",
        "chat_template.json",
        "preprocessor_config.json",
        "89644892e4d85e24eaac8bacfd4f463576704203",
        "candle-0.11.0-cpu-f32",
        "EXPECTED_TENSORS: usize = 625",
    ] {
        assert!(source.contains(required), "missing {required}");
    }
}

#[test]
fn processor_and_template_contract_are_explicit() {
    let source = runtime();
    for required in [
        "smart_resize",
        "image_grid_thw",
        "TEMPORAL_PATCH_SIZE",
        "MERGE_SIZE",
        "merged_visual_tokens",
        "build_prompt_tokens",
        "vision token framing",
        "pinned_processor_shape_matches_reference_fixture",
    ] {
        assert!(source.contains(required), "missing {required}");
    }
}

#[test]
fn generation_uses_fresh_session_and_bounded_deterministic_decode() {
    let source = runtime();
    assert!(source.contains(".begin_request()"));
    assert!(source.contains("maximum_generated_tokens"));
    assert!(source.contains("greedy_token"));
    assert!(source.contains("token == EOS_TOKEN_ID"));
    assert!(!source.contains("Cuda"));
}

#[test]
fn cpu_resource_contract_is_fail_closed() {
    let source = runtime();
    assert!(source.contains("QWEN3_VL_MINIMUM_RAM_BYTES: u64 = 13_253_615_616"));
    assert!(source.contains("ResourceLimitExceeded"));
    assert!(source.contains("LocalModelDevice::Cpu"));
    assert!(!source.contains("fallbacks"));
}

#[test]
fn lifecycle_has_real_offline_regression_proof() {
    let source = runtime();
    assert!(source.contains("real_offline_generation_unload_and_reinitialize"));
    assert!(source.contains("runtime.unload()"));
    assert!(source.contains("for _ in 0..2"));
}
