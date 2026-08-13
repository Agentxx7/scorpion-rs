#![cfg(feature = "chrome")]

use std::{fs, path::PathBuf};

fn source() -> String {
    fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/features/browser_challenge.rs"),
    )
    .unwrap()
}

#[test]
fn snapshot_records_every_authoritative_coordinate_fact() {
    let source = source();
    for fact in [
        "captured_pixel_width",
        "captured_pixel_height",
        "viewport_width",
        "viewport_height",
        "device_pixel_ratio",
        "capture_scale",
        "capture_clip",
        "scroll_x",
        "scroll_y",
    ] {
        assert!(source.contains(fact), "missing {fact}");
    }
}

#[test]
fn target_identity_uses_remote_dom_identity_not_selector_order() {
    let source = source();
    assert!(source.contains("backend_node_id"));
    assert!(source.contains("node_id"));
    assert!(source.contains("remote DOM objects"));
    assert!(!source.contains("query_selector"));
}

#[test]
fn revalidation_is_mandatory_before_every_action() {
    let source = source();
    let apply = source.split("pub async fn apply(").nth(1).unwrap();
    assert!(apply.contains("self.revalidate(page).await?"));
    assert!(apply.contains("BrowserActionFailed"));
}

#[test]
fn top_level_only_context_is_explicit_and_fail_closed() {
    let source = source();
    assert!(source.contains(".frames()"));
    assert!(source.contains("UnsupportedContext"));
    assert!(source.contains("window===top"));
}

#[test]
fn exact_actions_have_no_fallback_retry_substitution_or_clamping() {
    let source = source();
    for forbidden in [
        "clamp(",
        "find_element",
        "find_elements",
        "evaluate(\"document",
        "retry",
        "fallback",
    ] {
        assert!(!source.contains(forbidden), "forbidden {forbidden}");
    }
}

#[test]
fn mutation_geometry_bounds_and_action_errors_are_typed() {
    let source = source();
    for failure in [
        "TargetStale",
        "ChallengeMutated",
        "GeometryChanged",
        "TransformAmbiguous",
        "PointOutOfBounds",
        "DragOutOfBounds",
        "BrowserActionFailed",
        "RevalidationFailed",
    ] {
        assert!(source.contains(failure), "missing {failure}");
    }
}
