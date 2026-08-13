//! Acceptance contract for the canonical provider-neutral CAPTCHA capability.

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
fn neutral_model_contains_every_required_type_and_task() {
    let core = read("spider/src/features/captcha.rs");
    for required in [
        "CaptchaChallenge",
        "CaptchaChallengeKind",
        "ImageGridSelection",
        "HorizontalOffset",
        "PointSelection",
        "CaptchaVisualInput",
        "CaptchaSolveRequest",
        "CaptchaSolution",
        "CaptchaSolveOutcome",
        "CaptchaSolveFailure",
        "CaptchaSolveProvenance",
        "CaptchaProviderId",
        "CaptchaProviderCapabilities",
    ] {
        assert!(
            core.contains(required),
            "missing canonical type: {required}"
        );
    }
}

#[test]
fn every_failure_is_explicit_and_transport_facts_are_retained() {
    let core = read("spider/src/features/captcha.rs");
    for failure in [
        "InvalidChallenge",
        "UnsupportedChallenge",
        "ProviderUnavailable",
        "CredentialUnavailable",
        "DeadlineExceeded",
        "Transport(CrawlerFailure)",
        "ProviderRejected",
        "InvalidProviderResponse",
        "Inconclusive",
        "LocalExecutionFailure",
        "Cancelled",
    ] {
        assert!(core.contains(failure), "missing failure: {failure}");
    }
}

#[test]
fn capability_validation_precedes_provider_execution() {
    let core = read("spider/src/features/captcha.rs");
    let dispatch = core.split("pub async fn solve_captcha").nth(1).unwrap();
    let validation = dispatch.find("supported_kinds").unwrap();
    let execution = dispatch.find("provider.solve(request).await").unwrap();
    assert!(validation < execution);
    assert!(dispatch.contains("request.selected_provider != capabilities.provider"));
}

#[test]
fn core_has_no_browser_provider_protocol_or_network_authority() {
    let core = read("spider/src/features/captcha.rs");
    for forbidden in [
        "chromiumoxide",
        "reqwest::Client",
        "wreq::Client",
        "ClientBuilder",
        "CrawlerRequest",
        "CanonicalExecutor",
        "GEMINI_API_KEY",
        "CdpError",
    ] {
        assert!(
            !core.contains(forbidden),
            "core leaked authority: {forbidden}"
        );
    }
}

#[test]
fn local_and_external_providers_are_distinct_and_share_one_contract() {
    let solvers = read("spider/src/features/solvers.rs");
    assert!(solvers.contains("impl CaptchaProvider for LocalLanguageModelProvider"));
    assert!(solvers.contains("impl CaptchaProvider for ExternalGeminiProvider"));
    assert!(solvers.contains("CaptchaProviderId::LOCAL_LANGUAGE_MODEL"));
    assert!(solvers.contains("CaptchaProviderId::EXTERNAL_GEMINI"));
    let local = solvers
        .split("struct LocalLanguageModelProvider")
        .nth(1)
        .unwrap()
        .split("struct ExternalGeminiProvider")
        .next()
        .unwrap();
    assert!(!local.contains("GEMINI_EXECUTOR"));
    assert!(!local.contains("CrawlerRequest"));
}

#[test]
fn remote_assets_are_materialized_before_provider_dispatch() {
    let solvers = read("spider/src/features/solvers.rs");
    assert!(solvers.contains("async fn materialize_remote_challenge"));
    assert!(solvers.contains(".execute(CrawlerRequest::get(url))"));
    assert!(solvers.contains("materialize_remote_challenge(remote)"));
}

#[test]
fn external_provider_preserves_canonical_transport_and_provenance() {
    let solvers = read("spider/src/features/solvers.rs");
    assert!(solvers.contains("SecretRequestHeaders::new()"));
    assert!(solvers.contains(".execute(request)"));
    assert!(solvers.contains("CaptchaSolveFailure::Transport"));
    assert!(solvers.contains("CaptchaSolveProvenance::external"));
}

#[test]
fn canonical_dispatch_contains_no_provider_substitution() {
    let core = read("spider/src/features/captcha.rs");
    let dispatch = core
        .split("pub async fn solve_captcha")
        .nth(1)
        .unwrap()
        .split("fn solution_matches")
        .next()
        .unwrap();
    for forbidden in ["fallback", "retry", "race", "EXTERNAL_GEMINI"] {
        assert!(!dispatch.contains(forbidden));
    }
}

#[test]
fn browser_interaction_remains_outside_canonical_core() {
    let core = read("spider/src/features/captcha.rs");
    for forbidden in ["click_smooth", "click_and_drag", "outer_html", "Page"] {
        assert!(!core.contains(forbidden));
    }
}
