//! Acceptance contract for the canonical OpenAI vision CAPTCHA adapter.

use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn read(path: &str) -> String {
    fs::read_to_string(root().join(path)).unwrap()
}

#[test]
fn provider_has_distinct_identity_and_conservative_capabilities() {
    let core = read("spider/src/features/captcha.rs");
    let solvers = read("spider/src/features/solvers.rs");
    assert!(core.contains("pub const OPENAI_VISION"));
    assert!(solvers.contains("provider: CaptchaProviderId::OPENAI_VISION"));
    assert!(solvers.contains("supported_media_types: &[\"image/jpeg\", \"image/png\"]"));
    assert!(solvers.contains("maximum_inputs: 16"));
    assert!(solvers.contains("requires_credentials: true"));
}

#[test]
fn all_existing_neutral_challenge_kinds_are_advertised() {
    let solvers = read("spider/src/features/solvers.rs");
    let caps = solvers
        .split("static OPENAI_VISION_CAPABILITIES")
        .nth(1)
        .unwrap()
        .split("static LOCAL_LANGUAGE_MODEL_CAPABILITIES")
        .next()
        .unwrap();
    for kind in ["ImageGridSelection", "HorizontalOffset", "PointSelection"] {
        assert!(caps.contains(kind));
    }
}

#[test]
fn model_and_credentials_are_explicit_and_caller_supplied() {
    let solvers = read("spider/src/features/solvers.rs");
    assert!(solvers.contains("pub struct OpenAiVisionCaptchaProvider"));
    assert!(solvers.contains("pub fn new(model: impl Into<String>, api_key: impl Into<String>)"));
    let provider = solvers
        .split("pub struct OpenAiVisionCaptchaProvider")
        .nth(1)
        .unwrap()
        .split("impl CaptchaProvider for OpenAiVisionCaptchaProvider")
        .next()
        .unwrap();
    assert!(!provider.contains("OPENAI_API_KEY"));
}

#[test]
fn provider_uses_only_canonical_transport_and_secret_headers() {
    let solvers = read("spider/src/features/solvers.rs");
    assert!(solvers.contains("OPENAI_VISION_EXECUTOR.execute(crawler_request)"));
    assert!(solvers.contains("SecretRequestHeaders::new()"));
    assert!(solvers.contains("try_insert(\"authorization\""));
    for forbidden in [
        "async_openai::Client",
        "reqwest::Client::new()",
        "reqwest::Client::builder()",
        "wreq::Client::new()",
    ] {
        let adapter = solvers
            .split("pub struct OpenAiVisionCaptchaProvider")
            .nth(1)
            .unwrap();
        assert!(!adapter.contains(forbidden), "raw transport: {forbidden}");
    }
}

#[test]
fn endpoint_is_fixed_and_credentials_never_enter_url_or_payload() {
    let solvers = read("spider/src/features/solvers.rs");
    assert!(solvers.contains("https://api.openai.com/v1/responses"));
    assert!(!solvers.contains("api.openai.com/v1/responses?"));
    assert!(!solvers.contains("\"api_key\":"));
}

#[test]
fn response_parsing_is_strict_for_ids_and_coordinates() {
    let solvers = read("spider/src/features/solvers.rs");
    assert!(solvers.matches("#[serde(deny_unknown_fields)]").count() >= 3);
    assert!(solvers.contains("!valid_ids.contains(id.as_str())"));
    assert!(solvers.contains("!observed.insert(id.as_str())"));
    assert!(solvers.contains("!parsed.x.is_finite()"));
    assert!(solvers.contains("!parsed.y.is_finite()"));
}

#[test]
fn outcomes_use_existing_neutral_failure_and_provenance_seams() {
    let solvers = read("spider/src/features/solvers.rs");
    assert!(solvers.contains("CaptchaSolveFailure::Transport(failure)"));
    assert!(solvers.contains("CaptchaSolveFailure::ProviderRejected"));
    assert!(solvers.contains("CaptchaSolveFailure::InvalidProviderResponse"));
    assert!(solvers.contains("CaptchaSolveProvenance::external("));
}

#[test]
fn provider_does_not_change_registry_or_routing_policy() {
    let core = read("spider/src/features/captcha.rs");
    let registry = core
        .split("pub struct CaptchaProviderRegistry")
        .nth(1)
        .unwrap();
    assert!(!registry.contains("OPENAI_VISION"));
    assert!(!registry.contains("OpenAiVisionCaptchaProvider"));
}
