#![cfg(feature = "chrome")]

use std::{fs, path::PathBuf};

fn source() -> String {
    fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/features/captcha_browser.rs"),
    )
    .unwrap()
}

#[test]
fn thin_binding_composes_only_canonical_owners() {
    let source = source();
    for required in [
        "BrowserChallengeSnapshot",
        "CaptchaImageGridInput::new",
        "CaptchaSolveRequest",
        "CaptchaProviderRegistry",
        "CaptchaRouteAttempts",
        "snapshot.revalidate(page).await",
        "snapshot.apply(page, action).await",
    ] {
        assert!(source.contains(required), "missing {required}");
    }
    for forbidden in [
        "PaligemmaCpuRuntime",
        "generate_structured",
        "find_element(",
        "find_elements(",
        "click_smooth(",
        "click_and_drag_smooth(",
        "call_js_fn(",
        "retry",
        "fallback",
        "clamp(",
    ] {
        assert!(!source.contains(forbidden), "forbidden {forbidden}");
    }
}

#[test]
fn grid_identity_geometry_and_empty_semantics_are_preserved() {
    let source = source();
    assert!(source.contains("cells.len() != snapshot.targets.len()"));
    assert!(source.contains(".get(&cell.choice_id)"));
    assert!(source.contains("browser_rect_to_image(target.geometry)"));
    assert!(source.contains("empty_selection_valid"));
    assert!(source.contains("UnknownChoiceIdentity"));
    assert!(source.contains("EmptySelectionNotAllowed"));
}

#[test]
fn point_and_drag_delegate_to_the_authoritative_transform() {
    let source = source();
    assert!(source.contains(".image_to_browser(x, y)"));
    assert!(source.contains(".horizontal_drag_from_target(handle_target_id, offset)"));
    assert!(!source.contains("capture_scale *"));
    assert!(!source.contains("captured_pixel_width as f64 /"));
}

#[test]
fn provider_failure_and_revalidation_precede_all_actions() {
    let source = source();
    let provider_failure = source.find("ProviderFailure").unwrap();
    let revalidate = source.rfind("snapshot.revalidate(page).await").unwrap();
    let apply = source.rfind("snapshot.apply(page, action).await").unwrap();
    assert!(provider_failure < revalidate);
    assert!(revalidate < apply);
    assert!(source.contains("actions_applied: 0"));
}

#[test]
fn result_never_claims_that_action_dispatch_solved_the_challenge() {
    let source = source();
    assert!(source.contains("NotObservedByBinding"));
    assert!(!source.contains("CaptchaSolved"));
    assert!(!source.contains("ProgressionObserved"));
}

#[test]
fn one_attempt_has_no_routing_retry_or_provider_substitution() {
    let source = source();
    assert!(source.contains("selected_provider: attempt.selected_provider"));
    assert_eq!(source.matches("execute_explicit_attempt(").count(), 1);
    assert!(!source.contains("EXTERNAL_GEMINI"));
    assert!(!source.contains("OPENAI_VISION"));
    assert!(!source.contains("LOCAL_LANGUAGE_MODEL"));
}

#[test]
fn browser_handles_never_enter_the_solver_request() {
    let source = source();
    let request = source.split("Ok(CaptchaSolveRequest").nth(1).unwrap();
    let request = request.split("fn actions_for_solution").next().unwrap();
    assert!(!request.contains("page:"));
    assert!(!request.contains("snapshot:"));
    assert!(!request.contains("Element"));
    assert!(request.contains("CaptchaChallenge"));
    assert!(request.contains("visuals: vec![visual]"));
}

#[test]
fn every_required_failure_class_is_typed() {
    let source = source();
    for required in [
        "Materialization",
        "ProviderFailure",
        "SolutionKindMismatch",
        "UnknownChoiceIdentity",
        "EmptySelectionNotAllowed",
        "SolutionOutOfBounds",
        "Browser(BrowserChallengeFailure)",
    ] {
        assert!(source.contains(required), "missing {required}");
    }
}
