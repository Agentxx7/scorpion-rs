//! Machine-enforceable architecture guardrails for Scorpion.
//!
//! These tests scan the `spider` crate source tree and assert that canonical
//! architecture invariants hold. They are not unit tests of behavior — they
//! are architecture tests that prevent new code from silently violating the
//! canonical ownership and security seams documented in
//! `SCORPION_ARCHITECTURE.md`.
//!
//! Run with: `cargo test -p spider --test architecture_guardrails`

use std::fs;
use std::path::Path;

/// One Rust source file in the crate, with its contents.
struct SourceFile {
    relative_path: String,
    contents: String,
}

/// Recursively collect all `.rs` files under `spider/src`.
fn scan_spider_src() -> Vec<SourceFile> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_dir = manifest_dir.join("src");
    let mut files = Vec::new();
    collect_rust_files(&src_dir, &src_dir, &mut files);
    files
}

fn collect_rust_files(dir: &Path, base: &Path, out: &mut Vec<SourceFile>) {
    for entry in fs::read_dir(dir).expect("failed to read src directory") {
        let entry = entry.expect("failed to read directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, base, out);
        } else if path.extension().map_or(false, |ext| ext == "rs") {
            let contents = fs::read_to_string(&path).expect("failed to read source file");
            let relative_path = path
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.push(SourceFile {
                relative_path,
                contents,
            });
        }
    }
}

/// Assert that a pattern only appears in explicitly allowed files.
/// Returns the list of violating files for diagnostics.
fn assert_pattern_only_in_files(pattern: &str, allowed_files: &[&str], description: &str) {
    let files = scan_spider_src();
    let violations = find_pattern_violations(&files, pattern, allowed_files);
    assert!(
        violations.is_empty(),
        "architecture guardrail violated: {description}\n  pattern: {pattern:?}\n  allowed files: {allowed_files:?}\n  violating files: {violations:?}"
    );
}

/// Assert that a pattern appears in at least the expected file.
fn assert_pattern_exists_in_file(pattern: &str, expected_file: &str, description: &str) {
    let files = scan_spider_src();
    let found = files
        .iter()
        .any(|file| file.relative_path == expected_file && file.contents.contains(pattern));
    assert!(
        found,
        "canonical path missing: {description}\n  expected pattern: {pattern:?}\n  expected file: {expected_file}"
    );
}

/// Assert that a module is declared in `spider/src/features/mod.rs`.
fn assert_feature_module_declared(module_name: &str) {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mod_rs = manifest_dir.join("src/features/mod.rs");
    let contents = fs::read_to_string(&mod_rs).expect("failed to read features/mod.rs");
    let declaration = format!("pub mod {module_name};");
    assert!(
        contents.contains(&declaration),
        "canonical module not declared: {declaration} in features/mod.rs"
    );
}

/// Assert that a `#[cfg(...)]` gate exists around a module declaration.
fn assert_feature_module_gated(module_name: &str, cfg_gate: &str) {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mod_rs = manifest_dir.join("src/features/mod.rs");
    let contents = fs::read_to_string(&mod_rs).expect("failed to read features/mod.rs");
    let declaration = format!("pub mod {module_name};");
    let decl_index = contents
        .find(&declaration)
        .expect("module declaration not found");
    // Search backwards from the declaration for the cfg gate.
    let preceding = &contents[..decl_index];
    let has_gate = preceding
        .lines()
        .rev()
        .take(10)
        .any(|line| line.contains(cfg_gate));
    assert!(
        has_gate,
        "feature gate {cfg_gate:?} not found before {declaration} in features/mod.rs"
    );
}

// ---------------------------------------------------------------------------
// NO PARALLEL HTTP STACK
// ---------------------------------------------------------------------------

#[test]
fn no_new_reqwest_client_new_outside_canonical_paths() {
    assert_pattern_only_in_files(
        "reqwest::Client::new()",
        &[
            "features/transport.rs",
            "website.rs",
            "utils/mod.rs",
            "features/search_providers/bing.rs",
            "features/search_providers/brave.rs",
            "features/search_providers/searxng.rs",
            "features/search_providers/serper.rs",
            "features/search_providers/tavily.rs",
            "features/automation.rs",
            "features/solvers.rs",
            // Grandfathered exception: pre-existing raw client in a provider
            // adapter. Classification is UNKNOWN (outside canonical seam,
            // unaudited for SSRF/redirect/provenance). Frozen: must not be
            // extended. This allowlist entry is a mechanical exception, not
            // architectural approval.
            "features/github_source_provider.rs",
            "features/hugging_face_source_provider.rs",
        ],
        "reqwest::Client::new() must only be constructed in canonical transport or explicitly allowed upstream/provider paths",
    );
}

#[test]
fn no_new_reqwest_client_builder_outside_canonical_paths() {
    assert_pattern_only_in_files(
        "reqwest::Client::builder()",
        &[
            "features/transport.rs",
            "website.rs",
            "utils/mod.rs",
            // Grandfathered exception: pre-existing test-only usages inside
            // #[cfg(test)] blocks. Classification is UPSTREAM_COMPAT.
            "page.rs",
            "features/search_providers/bing.rs",
            "features/search_providers/brave.rs",
            "features/search_providers/searxng.rs",
            "features/search_providers/serper.rs",
            "features/search_providers/tavily.rs",
            "features/automation.rs",
            "features/solvers.rs",
            // Grandfathered exception: pre-existing raw client in a provider
            // adapter. Classification is UNKNOWN (outside canonical seam,
            // unaudited for SSRF/redirect/provenance). Frozen: must not be
            // extended. This allowlist entry is a mechanical exception, not
            // architectural approval.
            "features/github_source_provider.rs",
            "features/hugging_face_source_provider.rs",
        ],
        "reqwest::Client::builder() must only be used in canonical transport or explicitly allowed upstream/provider paths",
    );
}

#[test]
fn no_new_reqwest_clientbuilder_new_outside_canonical_paths() {
    assert_pattern_only_in_files(
        "reqwest::ClientBuilder::new()",
        &[
            "features/transport.rs",
            "website.rs",
            "utils/mod.rs",
            "features/search_providers/bing.rs",
            "features/search_providers/brave.rs",
            "features/search_providers/searxng.rs",
            "features/search_providers/serper.rs",
            "features/search_providers/tavily.rs",
            "features/automation.rs",
            "features/solvers.rs",
            // Grandfathered exception: pre-existing raw client in a provider
            // adapter. Classification is UNKNOWN (outside canonical seam,
            // unaudited for SSRF/redirect/provenance). Frozen: must not be
            // extended. This allowlist entry is a mechanical exception, not
            // architectural approval.
            "features/github_source_provider.rs",
            "features/hugging_face_source_provider.rs",
        ],
        "reqwest::ClientBuilder::new() must only be used in canonical transport or explicitly allowed upstream/provider paths",
    );
}

// ---------------------------------------------------------------------------
// NO PARALLEL TOR STACK
// ---------------------------------------------------------------------------

#[test]
fn no_new_tor_client_construction() {
    assert_pattern_only_in_files(
        "fn build_tor_client",
        &["features/transport.rs"],
        "build_tor_client must only be defined in the canonical transport module",
    );
}

#[test]
fn no_new_tor_transport_config_construction() {
    assert_pattern_only_in_files(
        "fn TorTransportConfig::new",
        &["features/transport.rs"],
        "TorTransportConfig::new must only be defined in the canonical transport module",
    );
}

#[test]
fn no_new_apply_transport_policy() {
    assert_pattern_only_in_files(
        "fn apply_transport_policy",
        &["features/transport.rs"],
        "apply_transport_policy must only be defined in the canonical transport module",
    );
}

// ---------------------------------------------------------------------------
// NO DUPLICATE SECURITY PRIMITIVES
// ---------------------------------------------------------------------------

#[test]
fn no_duplicate_onion_suffix_matching() {
    assert_pattern_only_in_files(
        "ends_with(\".onion\")",
        &[
            "features/transport.rs",
            // onion_seed.rs only mentions the pattern in a doc comment while
            // explicitly delegating to the canonical classifier; it does not
            // reimplement suffix matching.
            "features/onion_seed.rs",
        ],
        ".onion suffix matching must only be implemented in the canonical transport module",
    );
}

#[test]
fn no_duplicate_is_onion_url() {
    assert_pattern_only_in_files(
        "fn is_onion_url",
        &["features/transport.rs"],
        "is_onion_url must only be defined in the canonical transport module",
    );
}

#[test]
fn no_duplicate_validate_target() {
    assert_pattern_only_in_files(
        "fn validate_target",
        &["features/transport.rs"],
        "validate_target must only be defined in the canonical transport module",
    );
}

#[test]
fn no_duplicate_pin_redirect_policy() {
    assert_pattern_only_in_files(
        "fn pin_redirect_policy",
        &["features/transport.rs"],
        "pin_redirect_policy must only be defined in the canonical transport module",
    );
}

#[test]
fn no_duplicate_ssrf_screened_base_policy() {
    assert_pattern_only_in_files(
        "fn ssrf_screened_base_policy",
        &["features/transport.rs"],
        "ssrf_screened_base_policy must only be defined in the canonical transport module",
    );
}

#[test]
fn no_duplicate_execute_streaming_request() {
    assert_pattern_only_in_files(
        "fn execute_streaming_request",
        &["features/transport.rs"],
        "execute_streaming_request must only be defined in the canonical transport module",
    );
}

#[test]
fn no_duplicate_fetch_single_page_with_options() {
    assert_pattern_only_in_files(
        "fn fetch_single_page_with_options",
        &["utils/evidence.rs"],
        "fetch_single_page_with_options must only be defined in the canonical evidence module",
    );
}

// ---------------------------------------------------------------------------
// NO SHADOW MODELS
// ---------------------------------------------------------------------------

#[test]
fn no_shadow_artifact_reference() {
    assert_pattern_only_in_files(
        "pub struct ArtifactReference",
        &["features/artifact_reference.rs"],
        "ArtifactReference must only be defined in the canonical artifact_reference module",
    );
}

#[test]
fn no_shadow_artifact_download_binding() {
    assert_pattern_only_in_files(
        "pub struct ArtifactDownloadBinding",
        &["features/artifact_download_binding.rs"],
        "ArtifactDownloadBinding must only be defined in the canonical artifact_download_binding module",
    );
}

#[test]
fn no_shadow_artifact_download_execution_error() {
    assert_pattern_only_in_files(
        "pub enum ArtifactDownloadExecutionError",
        &["features/artifact_download_execution.rs"],
        "ArtifactDownloadExecutionError must only be defined in the canonical artifact_download_execution module",
    );
}

#[test]
fn no_shadow_acquired_artifact() {
    assert_pattern_only_in_files(
        "pub struct AcquiredArtifact",
        &["features/artifact_download_execution.rs"],
        "AcquiredArtifact must only be defined in the canonical artifact_download_execution module",
    );
}

// ---------------------------------------------------------------------------
// CANONICAL PATH EXISTENCE
// ---------------------------------------------------------------------------

#[test]
fn canonical_transport_module_exists() {
    assert_feature_module_declared("transport");
}

#[test]
fn canonical_evidence_module_exists() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let utils_mod = manifest_dir.join("src/utils/mod.rs");
    let contents = fs::read_to_string(&utils_mod).expect("failed to read utils/mod.rs");
    assert!(
        contents.contains("pub mod evidence"),
        "utils/evidence module must be declared"
    );
}

#[test]
fn canonical_acquisition_binding_module_exists() {
    assert_feature_module_declared("acquisition_binding");
}

#[test]
fn canonical_artifact_reference_module_exists() {
    assert_feature_module_declared("artifact_reference");
}

#[test]
fn canonical_artifact_download_binding_module_exists() {
    assert_feature_module_declared("artifact_download_binding");
}

#[test]
fn canonical_artifact_download_execution_module_exists() {
    assert_feature_module_declared("artifact_download_execution");
}

#[test]
fn canonical_secret_request_headers_module_exists() {
    assert_feature_module_declared("secret_request_headers");
}

#[test]
fn canonical_onion_seed_module_exists() {
    assert_feature_module_declared("onion_seed");
}

#[test]
fn canonical_discovery_target_module_exists() {
    assert_feature_module_declared("discovery_target");
}

#[test]
fn canonical_research_scope_module_exists() {
    assert_feature_module_declared("research_scope");
}

#[test]
fn canonical_source_module_exists() {
    assert_feature_module_declared("source");
}

#[test]
fn canonical_source_provider_module_exists() {
    assert_feature_module_declared("source_provider");
}

#[test]
fn canonical_transport_seam_exists() {
    assert_pattern_exists_in_file(
        "pub async fn execute_streaming_request",
        "features/transport.rs",
        "execute_streaming_request is the canonical streaming transport seam",
    );
}

#[test]
fn canonical_tor_client_builder_exists() {
    assert_pattern_exists_in_file(
        "pub(crate) fn build_tor_client",
        "features/transport.rs",
        "build_tor_client is the canonical Tor client construction seam",
    );
}

#[test]
fn canonical_evidence_seam_exists() {
    assert_pattern_exists_in_file(
        "pub async fn fetch_single_page_with_options",
        "utils/evidence.rs",
        "fetch_single_page_with_options is the canonical one-shot acquisition seam",
    );
}

#[test]
fn canonical_artifact_execution_seam_exists() {
    assert_pattern_exists_in_file(
        "pub async fn execute",
        "features/artifact_download_execution.rs",
        "execute is the canonical artifact download execution seam",
    );
}

#[test]
fn canonical_onion_classifier_exists() {
    assert_pattern_exists_in_file(
        "pub fn is_onion_url",
        "features/transport.rs",
        "is_onion_url is the canonical onion URL classifier",
    );
}

#[test]
fn canonical_target_validator_exists() {
    assert_pattern_exists_in_file(
        "pub fn validate_target",
        "features/transport.rs",
        "validate_target is the canonical target validation seam",
    );
}

// ---------------------------------------------------------------------------
// FEATURE GATE ARCHITECTURE
// ---------------------------------------------------------------------------

#[test]
fn artifact_download_execution_gated_correctly() {
    assert_feature_module_gated("artifact_download_execution", "not(feature = \"wreq\")");
}

#[test]
fn artifact_download_execution_gated_on_evidence() {
    assert_feature_module_gated("artifact_download_execution", "feature = \"evidence\"");
}

#[test]
fn artifact_download_execution_gated_not_cache_request() {
    assert_feature_module_gated(
        "artifact_download_execution",
        "not(feature = \"cache_request\")",
    );
}

#[test]
fn acquisition_binding_gated_on_evidence() {
    assert_feature_module_gated("acquisition_binding", "feature = \"evidence\"");
}

#[test]
fn evidence_module_gated_on_evidence_feature() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let utils_mod = manifest_dir.join("src/utils/mod.rs");
    let contents = fs::read_to_string(&utils_mod).expect("failed to read utils/mod.rs");
    assert!(
        contents.contains("#[cfg(feature = \"evidence\")]")
            && contents.contains("pub mod evidence"),
        "utils/evidence module must be gated on the evidence feature"
    );
}

// ---------------------------------------------------------------------------
// THIN INTERFACE: interface crates must not construct independent clients
// ---------------------------------------------------------------------------

#[test]
fn spider_cli_does_not_construct_reqwest_client() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cli_dir = manifest_dir.parent().unwrap().join("spider_cli/src");
    if !cli_dir.exists() {
        return;
    }
    let mut files = Vec::new();
    collect_rust_files(&cli_dir, &cli_dir, &mut files);
    for file in files {
        // oauth.rs is an authentication flow client, not an acquisition
        // transport path; it is allowed by exception.
        if file.relative_path == "oauth.rs" {
            continue;
        }
        assert!(
            !file.contents.contains("reqwest::Client::new()")
                && !file.contents.contains("reqwest::Client::builder()")
                && !file.contents.contains("reqwest::ClientBuilder::new()"),
            "spider_cli must not construct its own HTTP clients: found in {}",
            file.relative_path
        );
    }
}

#[test]
fn spider_mcp_does_not_construct_reqwest_client() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mcp_dir = manifest_dir.parent().unwrap().join("spider_mcp/src");
    if !mcp_dir.exists() {
        return;
    }
    let mut files = Vec::new();
    collect_rust_files(&mcp_dir, &mcp_dir, &mut files);
    for file in files {
        assert!(
            !file.contents.contains("reqwest::Client::new()")
                && !file.contents.contains("reqwest::Client::builder()")
                && !file.contents.contains("reqwest::ClientBuilder::new()"),
            "spider_mcp must not construct its own HTTP clients: found in {}",
            file.relative_path
        );
    }
}

// ---------------------------------------------------------------------------
// PROVIDER ISOLATION
// ---------------------------------------------------------------------------

#[test]
fn neutral_transport_is_provider_independent() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let transport_rs = manifest_dir.join("src/features/transport.rs");
    let contents = fs::read_to_string(&transport_rs).expect("failed to read transport.rs");
    assert!(
        !contents.contains("github") && !contents.contains("hugging_face"),
        "canonical transport must not contain provider-specific logic"
    );
}

#[test]
fn neutral_artifact_execution_is_provider_independent() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let execution_rs = manifest_dir.join("src/features/artifact_download_execution.rs");
    let contents =
        fs::read_to_string(&execution_rs).expect("failed to read artifact_download_execution.rs");
    assert!(
        !contents.contains("github") && !contents.contains("hugging_face"),
        "canonical artifact execution must not contain provider-specific logic"
    );
}

// ---------------------------------------------------------------------------
// FAIL CLOSED: rejecting siblings must exist
// ---------------------------------------------------------------------------

#[test]
fn build_streaming_client_has_rejecting_sibling() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let transport_rs = manifest_dir.join("src/features/transport.rs");
    let contents = fs::read_to_string(&transport_rs).expect("failed to read transport.rs");
    assert!(
        contents.contains("TransportError::TorNotCompiled"),
        "build_streaming_client must fail closed when transport_tor is not compiled"
    );
}

#[test]
fn fetch_via_tor_has_rejecting_sibling() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let evidence_rs = manifest_dir.join("src/utils/evidence.rs");
    let contents = fs::read_to_string(&evidence_rs).expect("failed to read evidence.rs");
    assert!(
        contents.contains("IncompatibleConfiguration") || contents.contains("TorNotCompiled"),
        "fetch_via_tor must fail closed for unsupported configurations"
    );
}

// ---------------------------------------------------------------------------
// NEGATIVE GUARDRAIL: the scanner itself catches violations
// ---------------------------------------------------------------------------

/// Core violation-detection logic, extracted so it can be tested with
/// synthetic content without touching repository source files.
fn find_pattern_violations(
    files: &[SourceFile],
    pattern: &str,
    allowed_files: &[&str],
) -> Vec<String> {
    files
        .iter()
        .filter(|file| {
            !allowed_files
                .iter()
                .any(|allowed| file.relative_path == *allowed)
        })
        .filter(|file| file.contents.contains(pattern))
        .map(|file| file.relative_path.clone())
        .collect()
}

#[test]
fn scanner_catches_forbidden_pattern() {
    // This test proves the guardrail machinery detects an actual violation.
    // It feeds the scanner synthetic content containing a forbidden
    // construction and proves detection. It does not modify repository
    // source files.
    let synthetic = vec![
        SourceFile {
            relative_path: "features/transport.rs".to_string(),
            contents: "pub fn build_tor_client() {}".to_string(),
        },
        SourceFile {
            relative_path: "features/evil.rs".to_string(),
            contents: "fn build_tor_client() { /* parallel implementation */ }".to_string(),
        },
    ];

    let violations = find_pattern_violations(
        &synthetic,
        "fn build_tor_client",
        &["features/transport.rs"],
    );
    assert_eq!(
        violations,
        vec!["features/evil.rs"],
        "scanner must detect forbidden pattern outside allowed files"
    );

    // Prove the scanner does not flag the same pattern in the allowed file.
    let clean = vec![SourceFile {
        relative_path: "features/transport.rs".to_string(),
        contents: "fn build_tor_client() {}".to_string(),
    }];
    let violations =
        find_pattern_violations(&clean, "fn build_tor_client", &["features/transport.rs"]);
    assert!(
        violations.is_empty(),
        "scanner must not flag pattern in allowed files"
    );

    // Prove a pattern that does not exist is not reported as a violation.
    let empty = vec![SourceFile {
        relative_path: "features/transport.rs".to_string(),
        contents: "pub fn is_onion_url() {}".to_string(),
    }];
    let violations =
        find_pattern_violations(&empty, "fn build_tor_client", &["features/transport.rs"]);
    assert!(
        violations.is_empty(),
        "scanner must not report violations for absent patterns"
    );
}

// ---------------------------------------------------------------------------
// NO SILENT FALLBACK: canonical paths do not fall through to alternates
// ---------------------------------------------------------------------------

#[test]
fn artifact_download_execution_has_no_website_dependency() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let execution_rs = manifest_dir.join("src/features/artifact_download_execution.rs");
    let contents =
        fs::read_to_string(&execution_rs).expect("failed to read artifact_download_execution.rs");
    assert!(
        !contents.contains("Website::new") && !contents.contains("Website {"),
        "artifact download execution must not construct or depend on Website"
    );
}

#[test]
fn artifact_download_execution_has_no_page_dependency() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let execution_rs = manifest_dir.join("src/features/artifact_download_execution.rs");
    let contents =
        fs::read_to_string(&execution_rs).expect("failed to read artifact_download_execution.rs");
    assert!(
        !contents.contains("Page::new") && !contents.contains("Page {"),
        "artifact download execution must not construct or depend on Page"
    );
}

#[test]
fn artifact_download_execution_uses_only_streaming_seam() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let execution_rs = manifest_dir.join("src/features/artifact_download_execution.rs");
    let contents =
        fs::read_to_string(&execution_rs).expect("failed to read artifact_download_execution.rs");
    assert!(
        contents.contains("execute_streaming_request"),
        "artifact download execution must use the canonical streaming transport seam"
    );
    assert!(
        !contents.contains("fetch_single_page")
            && !contents.contains("fetch_single_page_with_options"),
        "artifact download execution must not use the evidence fetch seam"
    );
}
