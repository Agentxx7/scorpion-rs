//! Acceptance contract for immutable local model installation and runtime.

use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn contract() -> String {
    fs::read_to_string(root().join("spider/src/features/local_model.rs")).unwrap()
}

#[test]
fn required_provider_neutral_model_exists_once() {
    let source = contract();
    for model in [
        "LocalModelManifest",
        "LocalModelIdentity",
        "LocalModelArtifact",
        "LocalModelInstallation",
        "InstalledModelIdentity",
        "LocalModelRuntimeRequirements",
        "LocalModelDevicePolicy",
        "LocalModelRuntimeState",
        "LocalModelQualification",
    ] {
        assert_eq!(
            source.matches(&format!("pub struct {model}")).count()
                + source.matches(&format!("pub enum {model}")).count(),
            1,
            "{model}"
        );
    }
}

#[test]
fn manifest_requires_complete_size_digest_and_pinned_revision() {
    let source = contract();
    assert!(source.contains("pub size_bytes: u64"));
    assert!(source.contains("pub sha256: String"));
    assert!(source.contains("resolved_revision.as_deref()"));
    assert!(source.contains("observed != expected"));
    assert!(source.contains("immutable_revision"));
}

#[test]
fn activation_verifies_staging_then_atomically_renames_directory() {
    let source = contract();
    let activation = source.split("pub fn activate(").nth(1).unwrap();
    assert!(activation.contains("verify_file(staging, artifact)?"));
    assert!(activation.contains("std::fs::rename(staging, active)"));
    assert!(activation.find("verify_file").unwrap() < activation.find("rename(staging").unwrap());
}

#[test]
fn durable_identity_is_rechecked_before_runtime_consumption() {
    let source = contract();
    assert!(source.contains(".scorpion-local-model-identity-v1"));
    assert!(source.contains("manifest_sha256"));
    assert!(source.contains("pub fn open_installation("));
    assert!(source.contains("marker != identity_record(&identity)"));
}

#[test]
fn runtime_has_no_network_or_inference_dependency() {
    let source = contract();
    for forbidden in [
        "reqwest::",
        "wreq::",
        "CanonicalExecutor",
        "candle_",
        "onnxruntime",
        "tch::",
        "download_url()",
    ] {
        assert!(
            !source.contains(forbidden),
            "forbidden runtime authority: {forbidden}"
        );
    }
}

#[test]
fn device_selection_and_fallback_are_explicit_and_fail_closed() {
    let source = contract();
    assert!(source.contains("pub primary: LocalModelDevice"));
    assert!(source.contains("pub fallbacks: Vec<LocalModelDevice>"));
    assert!(source.contains("DeviceUnavailable"));
    assert!(source.contains("ResourceLimitExceeded"));
    assert!(!source.contains("unwrap_or(LocalModelDevice::Cpu)"));
}

#[test]
fn persistent_runtime_lifecycle_requires_verified_installation() {
    let source = contract();
    assert!(source.contains("pub struct LocalModelRuntimeLifecycle"));
    assert!(source.contains("installation: &LocalModelInstallation"));
    assert!(source.contains("installation.reverify()?"));
    for state in [
        "Uninitialized",
        "Initializing",
        "Ready",
        "Failed",
        "Unloaded",
    ] {
        assert!(source.contains(state));
    }
}

#[test]
fn every_required_failure_is_explicit() {
    let source = contract();
    for failure in [
        "ModelNotInstalled",
        "InstallationInvalid",
        "IntegrityFailure",
        "RevisionMismatch",
        "RuntimeUnavailable",
        "DeviceUnavailable",
        "ResourceLimitExceeded",
        "InitializationFailure",
        "QualificationMissing",
    ] {
        assert!(source.contains(failure), "missing {failure}");
    }
}

#[test]
fn captcha_kinds_are_qualified_independently_for_exact_runtime_contract() {
    let source = contract();
    assert!(source.contains("pub challenge_kind: CaptchaChallengeKind"));
    assert!(source.contains("pub evaluation_sha256: String"));
    assert!(source.contains("pub runtime: String"));
    assert!(source.contains("pub preprocessing: String"));
    assert!(source.contains("require_qualification"));
}

#[test]
fn existing_artifact_reference_is_reused_without_transport_duplication() {
    let source = contract();
    assert!(source.contains("pub reference: ArtifactReference"));
    assert!(source.contains("pub fn from_acquired("));
    assert!(source.contains("acquired.bytes_written"));
    assert!(source.contains("acquired.sha256_hex"));
    assert!(!source.contains("CrawlerRequest"));
    assert!(!source.contains("execute_streaming_request"));
    assert!(!source.contains("ClientBuilder"));
}
