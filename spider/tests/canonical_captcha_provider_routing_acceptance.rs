//! Acceptance contract for canonical CAPTCHA provider registration and routing.

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
fn runtime_registry_has_explicit_resolution_and_duplicate_rejection() {
    let core = read("spider/src/features/captcha.rs");
    assert!(core.contains("pub struct CaptchaProviderRegistry<'a>"));
    assert!(core.contains("DuplicateProvider(CaptchaProviderId)"));
    assert!(core.contains("pub fn resolve(&self, id: CaptchaProviderId)"));
    assert!(core.contains("self.providers.get(&id).copied()"));
}

#[test]
fn registry_exposes_capabilities_and_provider_owned_availability() {
    let core = read("spider/src/features/captcha.rs");
    assert!(core.contains("pub fn capabilities("));
    assert!(core.contains("fn availability(&self) -> CaptchaProviderAvailability"));
    assert!(core.contains("CredentialUnavailable"));
}

#[test]
fn request_still_selects_exactly_one_provider() {
    let core = read("spider/src/features/captcha.rs");
    let request = core.split("pub struct CaptchaSolveRequest").nth(1).unwrap();
    assert!(request.contains("pub selected_provider: CaptchaProviderId"));
    assert!(!request.contains("preferred_providers"));
}

#[test]
fn attempt_ledger_preserves_unmodified_outcomes() {
    let core = read("spider/src/features/captcha.rs");
    assert!(core.contains("pub struct CaptchaRouteAttempt"));
    assert!(core.contains("pub outcome: CaptchaSolveOutcome"));
    assert!(core.contains("self.attempts.push(CaptchaRouteAttempt"));
    assert!(core.contains("pub fn attempts(&self) -> &[CaptchaRouteAttempt]"));
}

#[test]
fn registry_and_ledger_have_no_implicit_routing_policy() {
    let core = read("spider/src/features/captcha.rs");
    let registry = core
        .split("pub struct CaptchaProviderRegistry")
        .nth(1)
        .unwrap()
        .split("fn unavailable_provenance")
        .next()
        .unwrap();
    for forbidden in [
        "sort",
        "fallback_provider",
        "retry_provider",
        "GEMINI_API_KEY",
    ] {
        assert!(
            !registry.contains(forbidden),
            "implicit routing: {forbidden}"
        );
    }
}

#[test]
fn all_three_legacy_routes_use_explicit_attempts() {
    let solvers = read("spider/src/features/solvers.rs");
    for function in [
        "solve_horizontal_offset_with_legacy_routing",
        "solve_enterprise_with_browser_gemini",
        "solve_point_with_legacy_routing",
    ] {
        let body = solvers.split(function).nth(1).unwrap();
        let body = body.split("\n}\n").next().unwrap();
        assert!(
            body.contains("CaptchaProviderRegistry::new()"),
            "{function}"
        );
        assert!(body.contains("execute_explicit_attempt"), "{function}");
    }
}

#[test]
fn substitution_trigger_and_credential_ownership_are_preserved() {
    let solvers = read("spider/src/features/solvers.rs");
    assert!(
        solvers
            .matches("CaptchaSolveFailure::ProviderUnavailable")
            .count()
            >= 3
    );
    assert!(solvers.matches("std::env::var(\"GEMINI_API_KEY\")").count() >= 3);
    let core = read("spider/src/features/captcha.rs");
    assert!(!core.contains("GEMINI_API_KEY"));
}

#[test]
fn canonical_transport_and_provider_identity_remain_separate() {
    let core = read("spider/src/features/captcha.rs");
    assert!(core.contains("transport_backend: Option<BackendProvenance>"));
    assert!(core.contains("pub provider: CaptchaProviderId"));
    assert!(!core.contains("CanonicalExecutor"));
}
