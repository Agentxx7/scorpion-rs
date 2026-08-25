//! Independent guard for the closure verifier itself.
//! This test intentionally lives outside `closure_harness.rs`: removing or
//! gutting the verifier must fail the required CI target.

use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

const CLOSURE_HARNESS_SHA256: &str =
    "46be0569cb904d97afe92ae9106c96db20221a494b2c27c0aca6ff249babb180";

#[test]
fn closure_harness_contains_required_independent_checks() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let source = fs::read_to_string(root.join("spider/tests/closure_harness.rs")).unwrap();
    let digest = format!("{:x}", Sha256::digest(source.as_bytes()));
    assert_eq!(
        digest, CLOSURE_HARNESS_SHA256,
        "closure_harness.rs changed without updating its independent integrity review"
    );
    for marker in [
        "ast_contains_production_definition",
        "ast_function_calls",
        "manifest_enables_feature",
        "executable_test_command",
        "closed_stage_commit_is_a_real_commit_reachable_from_history",
        "structural_parser_rejects_known_adversarial_fixtures",
        "live_network_registry_matches_detected_reality",
        "proof_class_records_are_typed_bound_and_non_substitutable",
        "closed_requires_every_declared_proof_class_independently",
        "assert_commit_binds_relevant_files",
        "ledger_toml_unchanged_except_attestation_fields",
    ] {
        assert!(
            source.contains(marker),
            "closure harness integrity marker missing: {marker}"
        );
    }
}
