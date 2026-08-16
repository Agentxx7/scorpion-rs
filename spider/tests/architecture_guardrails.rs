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

fn workspace_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("spider crate must be inside workspace")
        .to_path_buf()
}

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
        } else if path.extension().is_some_and(|ext| ext == "rs") {
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
            "features/automation.rs",
            "features/solvers.rs",
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
            "features/automation.rs",
            "features/solvers.rs",
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
            "features/automation.rs",
            "features/solvers.rs",
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
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../spider_transport/src/transport.rs");
    assert!(fs::read_to_string(path)
        .unwrap()
        .contains("pub fn is_onion_url"));
}

#[test]
fn canonical_target_validator_exists() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../spider_transport/src/transport.rs");
    assert!(fs::read_to_string(path)
        .unwrap()
        .contains("pub fn validate_target"));
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
fn artifact_download_execution_is_not_gated_by_cache_request() {
    let features = fs::read_to_string(workspace_root().join("spider/src/features/mod.rs")).unwrap();
    let module = features
        .find("pub mod artifact_download_execution;")
        .unwrap();
    let gate = &features[module.saturating_sub(240)..module];
    assert!(!gate.contains("not(feature = \"cache_request\")"));
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
    let transport_rs = manifest_dir.join("../spider_transport/src/transport.rs");
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
    let transport_rs = manifest_dir.join("../spider_transport/src/transport.rs");
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

// ---------------------------------------------------------------------------
// Helpers for dependency/absence guards
// ---------------------------------------------------------------------------

/// Read one spider/src file by relative path.
fn read_src_file(relative_path: &str) -> String {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(manifest_dir.join("src").join(relative_path))
        .unwrap_or_else(|_| panic!("failed to read src/{relative_path}"))
}

/// All files (relative paths) whose contents contain `pattern`.
fn find_files_containing(files: &[SourceFile], pattern: &str) -> Vec<String> {
    files
        .iter()
        .filter(|file| file.contents.contains(pattern))
        .map(|file| file.relative_path.clone())
        .collect()
}

/// Assert that a pattern exists in no spider/src file at all.
fn assert_pattern_absent_everywhere(pattern: &str, description: &str) {
    let files = scan_spider_src();
    let found = find_files_containing(&files, pattern);
    assert!(
        found.is_empty(),
        "rejected/removed pattern reintroduced: {description}\n  pattern: {pattern:?}\n  found in: {found:?}"
    );
}

/// Assert that a canonical source file contains none of the forbidden
/// upstream/legacy call patterns.
fn assert_file_lacks_legacy_calls(relative_path: &str, forbidden: &[&str]) {
    let contents = read_src_file(relative_path);
    for pattern in forbidden {
        assert!(
            !contents.contains(pattern),
            "canonical -> legacy direct dependency: {relative_path} must not directly call {pattern:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// REJECTED MEANS GONE
// ---------------------------------------------------------------------------

#[test]
fn rejected_build_evidence_with_transport_is_gone() {
    // `build_evidence_with_transport` was a superseded compatibility shim over
    // the canonical `build_evidence` (which reads `Page::transport()`
    // directly). It was removed in the architecture-convergence frontier;
    // REJECTED means removed — this test detects any reintroduction.
    assert_pattern_absent_everywhere(
        "fn build_evidence_with_transport",
        "build_evidence_with_transport was removed; it must not be reintroduced as shim, helper, or fallback",
    );
}

// ---------------------------------------------------------------------------
// NO CANONICAL -> LEGACY DIRECT DEPENDENCY
// ---------------------------------------------------------------------------
//
// Dependency model (SCORPION_SDD.md §3) — three distinct categories:
//
// 1. CANONICAL DIRECT DEPENDENCY — allowed only on canonical seams.
// 2. TRANSITIVE UPSTREAM IMPLEMENTATION — permitted only behind an
//    explicitly approved boundary primitive (e.g. `fetch_single_page_with_options`
//    may route Default acquisition through `Website`/`crawl_raw`, whose
//    upstream internals execute underneath the seam). These tests do NOT
//    forbid that: upstream machinery may run transitively as the boundary
//    primitive's own implementation.
// 3. CANONICAL -> LEGACY/UPSTREAM DIRECT ALTERNATE EXECUTION — forbidden
//    and enforced here: no canonical module may *directly call* upstream
//    machinery, select it as an alternate path, or fall back to it.

/// Upstream/legacy execution call patterns that canonical Scorpion modules
/// must never invoke directly. Call-shape patterns (with parentheses) are
/// used so doc comments referencing these names do not false-positive.
const LEGACY_EXECUTION_CALLS: &[&str] = &[
    "configure_base_client(",
    "fetch_page_html(",
    "fetch_page_html_raw(",
    "fetch_page_html_with_fallback(",
    "setup_redirect_policy(",
    "setup_strict_policy(",
    "replacen(\"socks://\", \"http://\"",
];

#[test]
fn canonical_modules_do_not_call_legacy_execution_paths() {
    for canonical in [
        "features/transport.rs",
        "utils/evidence.rs",
        "features/artifact_download_execution.rs",
        "features/github_source_provider.rs",
        "features/hugging_face_source_provider.rs",
        "features/acquisition_binding.rs",
        "features/research_scope.rs",
        "features/discovery_target.rs",
    ] {
        assert_file_lacks_legacy_calls(canonical, LEGACY_EXECUTION_CALLS);
    }
}

#[test]
fn canonical_transport_does_not_rewrite_socks_scheme() {
    let contents = read_src_file("features/transport.rs");
    assert!(
        !contents.contains("replacen(\"socks://\", \"http://\""),
        "canonical transport must never silently rewrite socks:// to http://"
    );
}

// ---------------------------------------------------------------------------
// PROVIDER CANONICAL TRANSPORT USE (single execution graph)
// ---------------------------------------------------------------------------

#[test]
fn providers_use_canonical_transport_seam() {
    for provider in [
        "features/github_source_provider.rs",
        "features/hugging_face_source_provider.rs",
    ] {
        let contents = read_src_file(provider);
        assert!(
            contents.contains("execute_streaming_request"),
            "{provider} must execute through the canonical transport seam"
        );
        for forbidden in ["Website::new", "Website {", "Page::new", "reqwest::Client"] {
            assert!(
                !contents.contains(forbidden),
                "{provider} must not bypass canonical transport via {forbidden:?}"
            );
        }
    }
}

#[test]
fn canonical_search_owner_uses_transport_without_raw_clients() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    let providers = workspace.join("spider_search/src/providers");
    let mut files = Vec::new();
    collect_rust_files(&providers, &providers, &mut files);
    assert!(!files.is_empty(), "canonical search providers must exist");

    for file in files {
        for forbidden in [
            "reqwest::Client",
            "ClientBuilder",
            "Client::new",
            "Client::builder",
            "wreq::Client",
            ".send()",
            "client: Option",
            "client: &",
        ] {
            assert!(
                !file.contents.contains(forbidden),
                "search provider bypass {forbidden:?} in {}",
                file.relative_path
            );
        }
    }

    let manifest = fs::read_to_string(workspace.join("spider_search/Cargo.toml")).unwrap();
    assert!(manifest.contains("spider_transport"));
    assert!(!manifest.contains("path = \"../spider\""));
    assert!(!manifest.contains("path = \"../spider_agent\""));
}

#[test]
fn spider_search_facades_have_no_implementation() {
    for file in ["features/search.rs", "features/search_providers/mod.rs"] {
        let contents = read_src_file(file);
        for forbidden in [
            "pub trait SearchProvider",
            "pub struct SearchOptions",
            "pub struct SearchResult",
            "pub enum SearchError",
            "impl SearchProvider for",
            "reqwest::Client",
            ".send()",
        ] {
            assert!(
                !contents.contains(forbidden),
                "search implementation {forbidden:?} in façade {file}"
            );
        }
    }
}

#[test]
fn canonical_search_seam_and_models_are_unique() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    let mut graph = Vec::new();
    for crate_name in [
        "spider",
        "spider_agent",
        "spider_agent_types",
        "spider_search",
    ] {
        let source = workspace.join(crate_name).join("src");
        let mut files = Vec::new();
        collect_rust_files(&source, &source, &mut files);
        graph.extend(files.into_iter().map(|mut file| {
            file.relative_path = format!("{crate_name}/src/{}", file.relative_path);
            file
        }));
    }

    for pattern in [
        "pub trait SearchProvider",
        "pub struct SearchOptions",
        "pub enum TimeRange",
        "pub struct SearchResults",
        "pub enum SearchError",
    ] {
        let owners: Vec<_> = graph
            .iter()
            .filter(|file| file.contents.contains(pattern))
            .map(|file| file.relative_path.as_str())
            .collect();
        assert_eq!(
            owners,
            ["spider_search/src/search.rs"],
            "unexpected owners for {pattern}"
        );
    }
}

// ---------------------------------------------------------------------------
// SINGLE EXECUTION GRAPH: canonical capability seams are unique
// ---------------------------------------------------------------------------

#[test]
fn research_discover_seam_is_unique() {
    assert_pattern_only_in_files(
        "pub async fn discover(",
        &["features/research_scope.rs"],
        "research discover() must be the single canonical research seam",
    );
}

#[test]
fn discovery_plan_seam_is_unique() {
    assert_pattern_only_in_files(
        "pub fn plan(",
        &["features/discovery_target.rs"],
        "discovery plan() must be the single canonical discovery seam",
    );
}

#[test]
fn evidence_build_seam_is_unique() {
    assert_pattern_only_in_files(
        "pub fn build_evidence(",
        &["utils/evidence.rs"],
        "build_evidence() must be the single canonical evidence seam",
    );
}

#[test]
fn evidence_bundle_model_is_unique() {
    assert_pattern_only_in_files(
        "pub struct EvidenceBundle",
        &["utils/evidence.rs"],
        "EvidenceBundle must only be defined in the canonical evidence module",
    );
}

// ---------------------------------------------------------------------------
// THIN INTERFACES: no shadow canonical models in interface crates
// ---------------------------------------------------------------------------

#[test]
fn interfaces_define_no_shadow_domain_models() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let shadow_patterns = [
        "pub struct EvidenceBundle",
        "pub struct ArtifactReference",
        "pub struct ArtifactDownloadBinding",
        "pub enum TransportPolicy",
        "pub struct AcquiredArtifact",
    ];
    for interface_src in ["spider_cli/src", "spider_mcp/src"] {
        let dir = manifest_dir.parent().unwrap().join(interface_src);
        if !dir.exists() {
            continue;
        }
        let mut files = Vec::new();
        collect_rust_files(&dir, &dir, &mut files);
        for file in files {
            for pattern in shadow_patterns {
                assert!(
                    !file.contents.contains(pattern),
                    "thin interface violation: {interface_src}/{} defines shadow model {pattern:?}",
                    file.relative_path
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// NEGATIVE GUARDRAIL PROOFS (synthetic — never touch production source)
// ---------------------------------------------------------------------------

/// Each entry: (violation class, pattern scanned, allowlist, synthetic
/// violating file). Proves the scanner detects every violation class the
/// real guardrails above protect against.
#[test]
fn scanner_detects_every_violation_class() {
    let cases: &[(&str, &str, &[&str], &str)] = &[
        (
            "unauthorized HTTP client construction",
            "reqwest::Client::new()",
            &["features/transport.rs"],
            "features/new_provider.rs",
        ),
        (
            "unauthorized Tor builder",
            "fn build_tor_client",
            &["features/transport.rs"],
            "features/evil_tor.rs",
        ),
        (
            "duplicate onion classifier",
            "fn is_onion_url",
            &["features/transport.rs"],
            "features/evil_onion.rs",
        ),
        (
            "duplicate target validator",
            "fn validate_target",
            &["features/transport.rs"],
            "features/evil_validate.rs",
        ),
        (
            "duplicate canonical model",
            "pub struct ArtifactReference",
            &["features/artifact_reference.rs"],
            "features/evil_model.rs",
        ),
        (
            "interface-owned canonical execution (shadow model)",
            "pub struct EvidenceBundle",
            &["utils/evidence.rs"],
            "interface/evil_evidence.rs",
        ),
        (
            "canonical -> legacy dependency",
            "configure_base_client(",
            &[],
            "features/evil_legacy.rs",
        ),
        (
            "silent fallback to upstream alternate",
            "fetch_page_html_with_fallback(",
            &[],
            "features/evil_fallback.rs",
        ),
        (
            "reintroduced REJECTED implementation",
            "fn build_evidence_with_transport",
            &[],
            "features/evil_rejected.rs",
        ),
        (
            "unauthorized alternate execution seam",
            "fn execute_streaming_request",
            &["features/transport.rs"],
            "features/evil_seam.rs",
        ),
    ];

    for (class, pattern, allowed, violating_path) in cases {
        let mut synthetic = Vec::new();
        // An allowed file legitimately containing the pattern (only when the
        // guardrail has an allowlist; an empty allowlist means "nowhere").
        if let Some(allowed_path) = allowed.first() {
            synthetic.push(SourceFile {
                relative_path: allowed_path.to_string(),
                contents: format!("{pattern} {{ /* canonical */ }}"),
            });
        }
        // A violating file containing the same pattern.
        synthetic.push(SourceFile {
            relative_path: violating_path.to_string(),
            contents: format!("{pattern} {{ /* violation */ }}"),
        });
        let violations = find_pattern_violations(&synthetic, pattern, allowed);
        assert_eq!(
            violations,
            vec![violating_path.to_string()],
            "scanner must detect violation class: {class}"
        );
    }
}

/// Provider bypass detection is two-sided: a provider must both avoid
/// forbidden constructions AND positively call the canonical seam. Prove the
/// positive-requirement half fails when the seam call is absent.
#[test]
fn scanner_detects_provider_bypass_of_canonical_transport() {
    let bypassing = SourceFile {
        relative_path: "features/evil_provider.rs".to_string(),
        contents: "let client = reqwest::Client::new(); client.get(url).send()".to_string(),
    };
    assert!(
        !bypassing.contents.contains("execute_streaming_request"),
        "synthetic bypassing provider must lack the canonical seam call"
    );
    // The real guardrail (`providers_use_canonical_transport_seam`) asserts
    // presence of the seam call; this proves the discriminating condition
    // actually distinguishes a bypassing file.
    let conforming = SourceFile {
        relative_path: "features/good_provider.rs".to_string(),
        contents: "crate::features::transport::execute_streaming_request(&endpoint, &self.transport, &self.headers)".to_string(),
    };
    assert!(
        conforming.contents.contains("execute_streaming_request"),
        "synthetic conforming provider must contain the canonical seam call"
    );
}

#[test]
fn automation_proxy_resolution_cannot_authorize_direct_fallback() {
    let engine = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../spider_agent/src/automation/engine.rs"),
    )
    .expect("failed to read spider_agent automation engine");
    let browser = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../spider_agent/src/automation/browser.rs"),
    )
    .expect("failed to read spider_agent automation browser");

    for forbidden in [
        "if let Ok(proxy) = reqwest::Proxy::all",
        "builder.build().ok()",
    ] {
        assert!(
            !engine.contains(forbidden),
            "automation proxy errors must not be discarded via {forbidden:?}"
        );
    }
    assert!(
        browser.contains("engine.with_proxies(cfgs.proxies.as_deref())?;"),
        "automation construction must propagate explicit proxy resolution failure"
    );

    let synthetic = "if let Ok(proxy) = reqwest::Proxy::all(url) { builder = builder.proxy(proxy); } builder.build().ok()";
    assert!(
        synthetic.contains("if let Ok(proxy) = reqwest::Proxy::all")
            && synthetic.contains("builder.build().ok()"),
        "guardrail conditions must detect swallowed parse and build failures"
    );
}

fn worker_boundary_violation(kind: &str, contents: &str) -> bool {
    match kind {
        "reverse_dependency" => contents.contains("path = \"../spider_worker\""),
        "canonical_selection" => {
            contents.contains("spider_worker")
                || contents.contains("SPIDER_WORKER")
                || contents.contains("worker_connection")
        }
        "canonical_ownership" => [
            "TransportPolicy",
            "trait SearchProvider",
            "struct SearchOptions",
            "enum SearchError",
            "trait SourceProvider",
            "struct EvidenceBundle",
            "struct ArtifactReference",
            "struct AgentConfig",
            "struct WatchState",
            "pub struct Job",
        ]
        .iter()
        .any(|pattern| contents.contains(pattern)),
        "ssrf_import" => contents.contains("spider_worker::target_host_blocked"),
        _ => false,
    }
}

#[test]
fn spider_worker_is_a_terminal_upstream_compatibility_boundary() {
    let root = workspace_root();
    let worker_manifest = fs::read_to_string(root.join("spider_worker/Cargo.toml")).unwrap();
    assert!(worker_manifest.contains("[dependencies.spider]"));
    assert!(!root.join("spider_worker/src/lib.rs").exists());

    for entry in fs::read_dir(&root).unwrap().flatten() {
        let manifest = entry.path().join("Cargo.toml");
        if !manifest.exists() || entry.file_name() == "spider_worker" {
            continue;
        }
        let contents = fs::read_to_string(&manifest).unwrap();
        assert!(
            !worker_boundary_violation("reverse_dependency", &contents),
            "workspace capability must not depend on spider_worker: {}",
            manifest.display()
        );
    }
}

#[test]
fn canonical_capabilities_cannot_select_worker_execution() {
    for canonical in [
        "features/transport.rs",
        "utils/evidence.rs",
        "features/artifact_download_execution.rs",
        "features/github_source_provider.rs",
        "features/hugging_face_source_provider.rs",
        "features/acquisition_binding.rs",
        "features/research_scope.rs",
        "features/discovery_target.rs",
        "features/search.rs",
    ] {
        let contents =
            fs::read_to_string(workspace_root().join("spider/src").join(canonical)).unwrap();
        assert!(
            !worker_boundary_violation("canonical_selection", &contents),
            "canonical capability selects worker protocol: {canonical}"
        );
        assert!(
            !worker_boundary_violation("ssrf_import", &contents),
            "canonical capability imports worker-local SSRF defense: {canonical}"
        );
    }
}

#[test]
fn worker_defines_no_canonical_ownership_and_uses_exact_compat_primitives() {
    let worker = fs::read_to_string(workspace_root().join("spider_worker/src/main.rs")).unwrap();
    assert!(!worker_boundary_violation("canonical_ownership", &worker));
    assert!(worker.contains("fn target_host_blocked("));
    assert!(!worker.contains("pub fn target_host_blocked("));
    assert!(!worker.contains("spider_transport::"));
    for primitive in [
        "configure_http_client()",
        "Page::new_page_streaming(",
        "fetch_page_html_raw(",
    ] {
        assert_eq!(
            worker.matches(primitive).count(),
            1,
            "compatibility primitive drift: {primitive}"
        );
    }
}

#[test]
fn decentralized_worker_remains_rejected_under_tor() {
    let website = fs::read_to_string(workspace_root().join("spider/src/website.rs")).unwrap();
    assert!(website.contains("if cfg!(feature = \"decentralized\")"));
    assert!(website.contains("decentralized crawling is not audited for Tor"));
}

#[test]
fn scanner_detects_worker_boundary_violation_classes() {
    for (kind, synthetic) in [
        (
            "reverse_dependency",
            "spider_worker = { path = \"../spider_worker\" }",
        ),
        (
            "canonical_selection",
            "fallback_to_spider_worker(worker_url)",
        ),
        ("canonical_ownership", "pub trait SearchProvider {}"),
        ("ssrf_import", "use spider_worker::target_host_blocked;"),
    ] {
        assert!(
            worker_boundary_violation(kind, synthetic),
            "scanner missed {kind}"
        );
    }
}

fn canonical_crawler_error_boundary_violation(contents: &str) -> bool {
    [
        "Arc<reqwest::Error>",
        "Arc<wreq::Error>",
        "reqwest_middleware::Error>",
        "pub error_for_status: Option<Result<",
    ]
    .iter()
    .any(|pattern| contents.contains(pattern))
}

#[test]
fn crawler_response_error_seam_has_one_neutral_owner() {
    let root = workspace_root();
    let transport = fs::read_to_string(root.join("spider_transport/src/crawler_outcome.rs"))
        .expect("canonical crawler outcome module");
    assert_eq!(transport.matches("pub enum CrawlerFailureKind").count(), 1);
    assert_eq!(transport.matches("pub struct CrawlerFailure").count(), 1);
    assert_eq!(transport.matches("pub struct CrawlerResponse").count(), 1);

    let page = fs::read_to_string(root.join("spider/src/page.rs")).expect("Page source");
    let utils = fs::read_to_string(root.join("spider/src/utils/mod.rs")).expect("crawler utils");
    assert!(!canonical_crawler_error_boundary_violation(&page));
    assert!(!canonical_crawler_error_boundary_violation(&utils));
    assert!(page.contains("Arc<spider_transport::CrawlerFailure>"));
    assert!(utils.contains("pub failure: Option<spider_transport::CrawlerFailure>"));
}

#[test]
fn crawler_retry_policy_stays_above_transport_facts() {
    let root = workspace_root();
    let transport = fs::read_to_string(root.join("spider_transport/src/crawler_outcome.rs"))
        .expect("canonical crawler outcome module");
    for crawler_policy in ["should_retry", "backoff", "is_retryable_status"] {
        assert!(
            !transport.contains(crawler_policy),
            "crawler retry policy leaked into transport facts: {crawler_policy}"
        );
    }

    let page = fs::read_to_string(root.join("spider/src/page.rs")).expect("Page source");
    assert!(page.contains("fn get_error_status_base("));
    assert!(page.contains("is_retryable_status(pre_classified_status)"));
}

#[test]
fn scanner_detects_backend_error_leakage_into_canonical_page_contract() {
    for synthetic in [
        "pub error_status: Option<Arc<reqwest::Error>>",
        "pub error_status: Option<Arc<wreq::Error>>",
        "pub error_for_status: Option<Result<Response, reqwest_middleware::Error>>",
    ] {
        assert!(
            canonical_crawler_error_boundary_violation(synthetic),
            "scanner missed backend error leakage: {synthetic}"
        );
    }
    assert!(!canonical_crawler_error_boundary_violation(
        "pub failure: Option<spider_transport::CrawlerFailure>"
    ));
}

#[test]
fn canonical_crawler_transport_execution_is_executor_owned() {
    let root = workspace_root();
    let website = fs::read_to_string(root.join("spider/src/website.rs")).unwrap();
    let page = fs::read_to_string(root.join("spider/src/page.rs")).unwrap();
    let transport = fs::read_to_string(root.join("spider_transport/src/transport.rs")).unwrap();
    assert!(website.contains("resolved_executor: Option<Arc<CanonicalExecutor>>"));
    assert!(website.contains("prepare_execution"));
    assert!(!website.contains("Page::new_page_streaming("));
    assert!(!website.contains("Page::new_page_with_cache("));
    assert!(!website.contains("fetch_page_html_raw_conditional("));
    assert!(!website.contains("struct ClientRotator"));
    assert!(website.contains("struct NoncanonicalClientRotator"));
    assert!(page.contains("new_page_streaming_for_mode"));
    assert!(transport.contains("next_client.fetch_add"));
    assert!(!transport.contains("pub fn client("));
    assert!(!transport.contains("CrawlerExecutionError"));
}

#[test]
fn scanner_rejects_synthetic_raw_client_reintroductions() {
    fn rejects(source: &str) -> bool {
        [
            "Page::new_page(",
            "Page::new_page_streaming(",
            "Page::new_page_with_cache(",
            "fetch_page_html_raw_conditional(",
            "set_http_client(",
            "get_client(",
            "struct ClientRotator",
        ]
        .iter()
        .any(|forbidden| source.contains(forbidden))
    }
    for fixture in [
        "Page::new_page(url, &client).await;",
        "Page::new_page_streaming(url, client, false).await;",
        "website.set_http_client(client);",
        "let client = website.get_client();",
        "struct ClientRotator { clients: Vec<Client> }",
    ] {
        assert!(rejects(fixture), "negative fixture escaped: {fixture}");
    }
    assert!(!rejects("executor.execute(CrawlerRequest::get(url)).await"));
}

fn cache_transport_ownership_violation(source: &str) -> bool {
    [
        "reqwest::ClientBuilder",
        "reqwest_middleware::ClientBuilder",
        "ClientWithMiddleware",
        "RequestBuilder.send",
        ".redirect(",
        "Proxy::all",
        "danger_accept_invalid_certs",
        "secret_headers.serialize",
        "origin = ResponseOrigin::Network",
    ]
    .iter()
    .any(|pattern| source.contains(pattern))
}

#[test]
fn cache_request_is_policy_above_the_canonical_executor() {
    let root = workspace_root();
    let cache = fs::read_to_string(root.join("spider/src/cache_request.rs")).unwrap();
    let website = fs::read_to_string(root.join("spider/src/website.rs")).unwrap();
    let page = fs::read_to_string(root.join("spider/src/page.rs")).unwrap();
    let manifest = fs::read_to_string(root.join("spider/Cargo.toml")).unwrap();
    assert!(cache.contains("executor.execute(request)"));
    assert!(cache.contains("ResponseOrigin::ReconstructedCache"));
    assert!(cache.contains("BackendProvenance::CacheLayer"));
    assert!(!cache.contains("BackendProvenance::CacheMiddleware"));
    assert!(cache.contains("request.secret_headers.is_empty()"));
    assert!(!cache_transport_ownership_violation(&cache));
    for removed in [
        "reqwest_middleware",
        "ClientWithMiddleware",
        "CACHE_WRAPPED_TRANSPORT_AC",
        "HttpCache {",
    ] {
        assert!(
            !website.contains(removed),
            "historical Website cache transport: {removed}"
        );
        assert!(
            !page.contains(removed),
            "historical Page cache transport: {removed}"
        );
    }
    for removed in [
        "reqwest-middleware =",
        "spider-http-cache-reqwest",
        "http-global-cache",
    ] {
        assert!(
            !manifest.contains(removed),
            "old cache transport dependency: {removed}"
        );
    }
}

#[test]
fn cache_transport_negative_fixtures_are_rejected() {
    for fixture in [
        "reqwest::ClientBuilder::new().redirect(policy)",
        "let client: ClientWithMiddleware = build();",
        "RequestBuilder.send().await",
        "Proxy::all(cache_proxy)",
        "secret_headers.serialize(metadata)",
        "cache_hit.origin = ResponseOrigin::Network",
    ] {
        assert!(
            cache_transport_ownership_violation(fixture),
            "cache transport fixture escaped: {fixture}"
        );
    }
    assert!(!cache_transport_ownership_violation(
        "executor.execute(request).await"
    ));
}

fn wreq_authority_violation(source: &str) -> bool {
    [
        "Website { wreq_client:",
        "ClientBuilder::new().send()",
        "canonical_page.error = wreq_error",
        "if let Ok(proxy) { direct() }",
    ]
    .iter()
    .any(|pattern| source.contains(pattern))
}

#[test]
fn wreq_execution_authority_is_canonical_and_compatibility_isolated() {
    let root = workspace_root();
    let transport = fs::read_to_string(root.join("spider_transport/src/transport.rs")).unwrap();
    let website = fs::read_to_string(root.join("spider/src/website.rs")).unwrap();
    let page = fs::read_to_string(root.join("spider/src/page.rs")).unwrap();
    let evidence = fs::read_to_string(root.join("spider/src/utils/evidence.rs")).unwrap();
    let cache = fs::read_to_string(root.join("spider/src/cache_request.rs")).unwrap();
    let solvers = fs::read_to_string(root.join("spider/src/features/solvers.rs")).unwrap();
    assert!(transport.contains("NoncanonicalWreq"));
    assert!(transport.contains("CanonicalWreq"));
    assert!(website.contains("ExecutionMode::CanonicalWreq"));
    assert!(transport.contains("ResolvedWreqExecutor"));
    assert!(transport.contains("canonical_redirect_decision"));
    assert!(transport.contains("validate_target(&request.url"));
    assert!(transport.contains("request.secret_headers.apply_to"));
    assert!(transport.contains("wreq::Proxy::all(endpoint)"));
    let resolution = website
        .split("fn resolve_wreq_executor")
        .nth(1)
        .expect("canonical Wreq resolver")
        .split("pub fn configure_base_client")
        .next()
        .expect("resolver boundary");
    assert!(!resolution.contains("ClientBuilder::new"));
    assert!(!resolution.contains(".send()"));
    assert!(!resolution.contains("if let Ok(proxy)"));
    assert!(page.contains("UPSTREAM_COMPATIBILITY_BOUNDARY"));
    assert!(evidence.contains("canonical evidence acquisition is unavailable under wreq"));
    assert!(!evidence.contains("wreq::Client"));
    assert!(!cache.contains("wreq::Client"));
    assert!(solvers.contains("static ref GEMINI_EXECUTOR: CanonicalExecutor"));
    assert!(!solvers.contains("GEMINI_CLIENT"));
    assert!(!solvers.contains("generateContent?key="));
    for manifest in [
        "spider_cli/Cargo.toml",
        "spider_mcp/Cargo.toml",
        "spider_agent/Cargo.toml",
    ] {
        let contents = fs::read_to_string(root.join(manifest)).unwrap();
        assert!(
            !contents.contains("spider/wreq"),
            "canonical product selects wreq: {manifest}"
        );
    }
    assert!(!canonical_crawler_error_boundary_violation(&page));
}

#[test]
fn scanner_rejects_synthetic_canonical_wreq_claims() {
    for fixture in [
        "Website { wreq_client: client }",
        "ClientBuilder::new().send()",
        "canonical_page.error = wreq_error",
        "if let Ok(proxy) { direct() }",
    ] {
        assert!(
            wreq_authority_violation(fixture),
            "wreq fixture escaped: {fixture}"
        );
    }
    assert!(!wreq_authority_violation(
        "ResolvedWreqExecutor::execute(request).await"
    ));
}

fn gemini_solver_transport_violation(source: &str) -> bool {
    [
        "GEMINI_CLIENT",
        "reqwest::ClientBuilder::new()",
        "wreq::ClientBuilder::new()",
        "generateContent?key=",
        ".get(tile.img_src).send()",
    ]
    .iter()
    .any(|pattern| source.contains(pattern))
}

#[test]
fn gemini_solver_uses_only_canonical_transport_authority() {
    let root = workspace_root();
    let solvers = fs::read_to_string(root.join("spider/src/features/solvers.rs")).unwrap();
    assert!(solvers.contains("static ref GEMINI_EXECUTOR: CanonicalExecutor"));
    assert!(solvers.contains("SecretRequestHeaders::new()"));
    assert!(solvers.contains("CrawlerRequest::get"));
    assert!(!gemini_solver_transport_violation(&solvers));
}

#[test]
fn scanner_rejects_synthetic_raw_gemini_transport() {
    for fixture in [
        "static GEMINI_CLIENT: reqwest::Client",
        "wreq::ClientBuilder::new()",
        "generateContent?key=secret",
        ".get(tile.img_src).send()",
    ] {
        assert!(gemini_solver_transport_violation(fixture));
    }
    assert!(!gemini_solver_transport_violation(
        "GEMINI_EXECUTOR.execute(request).await"
    ));
}

fn captcha_provider_authority_violation(source: &str) -> bool {
    [
        "impl CaptchaProvider for RawProvider { reqwest::Client",
        "impl CaptchaProvider for RawProvider { wreq::Client",
        "impl CaptchaProvider for OpenAiVisionCaptchaProvider { async_openai::Client",
        "CaptchaSolveRequest { client:",
        "provider.solve(request).or_else(fallback_provider)",
        "provider_id = BackendProvenance::Reqwest",
        "CaptchaSolveFailure::LocalExecutionFailure(cdp_error)",
        "registry.first()",
        "registry.iter().find(|provider| provider.capabilities().locality",
        "execute_explicit_attempt(request).or_else",
        "attempts.clear()",
    ]
    .iter()
    .any(|pattern| source.contains(pattern))
}

#[test]
fn canonical_captcha_capability_separates_provider_transport_and_browser_authority() {
    let root = workspace_root();
    let core = fs::read_to_string(root.join("spider/src/features/captcha.rs")).unwrap();
    let solvers = fs::read_to_string(root.join("spider/src/features/solvers.rs")).unwrap();
    assert!(core.contains("pub trait CaptchaProvider"));
    assert!(core.contains("request.selected_provider != capabilities.provider"));
    assert!(core.contains("provider.solve(request).await"));
    assert!(core.contains("pub struct CaptchaProviderRegistry<'a>"));
    assert!(core.contains("DuplicateProvider(CaptchaProviderId)"));
    assert!(core.contains("pub struct CaptchaRouteAttempts"));
    assert!(!core.contains("reqwest::Client"));
    assert!(!core.contains("wreq::Client"));
    assert!(!core.contains("CdpError"));
    assert!(solvers.contains("impl CaptchaProvider for LocalLanguageModelProvider"));
    assert!(solvers.contains("impl CaptchaProvider for ExternalGeminiProvider"));
    assert!(solvers.contains("impl CaptchaProvider for OpenAiVisionCaptchaProvider"));
    assert!(solvers.contains("OPENAI_VISION_EXECUTOR.execute(crawler_request)"));
    let openai_adapter = solvers
        .split("pub struct OpenAiVisionCaptchaProvider")
        .nth(1)
        .expect("OpenAI vision provider exists");
    for forbidden in [
        "async_openai::Client",
        "reqwest::Client::new()",
        "wreq::Client::new()",
        "OPENAI_API_KEY",
    ] {
        assert!(!openai_adapter.contains(forbidden));
    }
}

#[test]
fn canonical_captcha_full_grid_has_one_validated_provider_neutral_owner() {
    let source =
        fs::read_to_string(workspace_root().join("spider/src/features/captcha.rs")).unwrap();
    assert_eq!(
        source.matches("pub struct CaptchaImageGridInput").count(),
        1
    );
    assert!(source.contains("MaterializedFullGrid(Box<CaptchaImageGridInput>)"));
    assert!(source.contains("pub fn image_grid(&self) -> Option<&CaptchaImageGridInput>"));
    for forbidden in [
        "Qwen3",
        "Gemini",
        "OpenAi",
        "compose_tiles",
        "infer_grid_layout",
    ] {
        let grid = source
            .split("pub struct CaptchaImageGridInput")
            .nth(1)
            .unwrap()
            .split("pub enum CaptchaVisualInput")
            .next()
            .unwrap();
        assert!(!grid.contains(forbidden), "grid model contains {forbidden}");
    }
}

#[test]
fn scanner_rejects_synthetic_provider_owned_grid_inference() {
    for fixture in [
        "let rows = (visuals.len() as f64).sqrt()",
        "let id = index.to_string()",
        "compose_tiles(visuals)",
        "infer_grid_layout(image)",
    ] {
        assert!(
            fixture.contains("visuals.len")
                || fixture.contains("index.to_string")
                || fixture.contains("compose_tiles")
                || fixture.contains("infer_grid_layout"),
            "negative fixture was not detected"
        );
    }
}

#[test]
fn scanner_rejects_synthetic_captcha_provider_authority_leaks() {
    for fixture in [
        "impl CaptchaProvider for RawProvider { reqwest::Client",
        "impl CaptchaProvider for RawProvider { wreq::Client",
        "impl CaptchaProvider for OpenAiVisionCaptchaProvider { async_openai::Client",
        "CaptchaSolveRequest { client: raw_client }",
        "provider.solve(request).or_else(fallback_provider)",
        "provider_id = BackendProvenance::Reqwest",
        "CaptchaSolveFailure::LocalExecutionFailure(cdp_error)",
        "registry.first()",
        "registry.iter().find(|provider| provider.capabilities().locality",
        "execute_explicit_attempt(request).or_else",
        "attempts.clear()",
    ] {
        assert!(captcha_provider_authority_violation(fixture));
    }
    assert!(!captcha_provider_authority_violation(
        "solve_captcha(&selected_provider, &request).await"
    ));
}

fn local_model_contract_violation(source: &str) -> bool {
    [
        "impl LocalModelRuntime { reqwest::Client",
        "impl LocalModelRuntime { wreq::Client",
        "LocalModelRuntime::download(",
        "revision: \"latest\"",
        "unwrap_or(LocalModelDevice::Cpu)",
        "activate_without_integrity_check",
        "LocalModelArtifact { sha256: None",
    ]
    .iter()
    .any(|pattern| source.contains(pattern))
}

#[test]
fn canonical_local_model_contract_is_transport_and_runtime_neutral() {
    let root = workspace_root();
    let model = fs::read_to_string(root.join("spider/src/features/local_model.rs")).unwrap();
    assert!(model.contains("pub struct LocalModelManifest"));
    assert!(model.contains("pub struct LocalModelInstallation"));
    assert!(model.contains("pub struct InstalledModelIdentity"));
    assert!(model.contains("pub fn activate("));
    assert!(model.contains("verify_file(staging, artifact)?"));
    assert!(model.contains("std::fs::rename(staging, active)"));
    assert!(model.contains("pub fn preflight_device("));
    assert!(model.contains("pub fn require_qualification("));
    assert!(!local_model_contract_violation(&model));
    for forbidden in ["reqwest::", "wreq::", "ClientBuilder", "candle_"] {
        assert!(!model.contains(forbidden), "local model owns {forbidden}");
    }
}

#[test]
fn scanner_rejects_synthetic_local_model_contract_violations() {
    for fixture in [
        "impl LocalModelRuntime { reqwest::Client",
        "impl LocalModelRuntime { wreq::Client",
        "LocalModelRuntime::download(model)",
        "revision: \"latest\"",
        "device.unwrap_or(LocalModelDevice::Cpu)",
        "activate_without_integrity_check(staging)",
        "LocalModelArtifact { sha256: None }",
    ] {
        assert!(
            local_model_contract_violation(fixture),
            "fixture escaped: {fixture}"
        );
    }
    assert!(!local_model_contract_violation(
        "manifest.activate(staging, active)?"
    ));
}

fn qwen_generation_state_violation(source: &str) -> bool {
    [
        "model: Mutex<Qwen3VLModel>",
        "Arc<Qwen3VLModel>",
        "recycle_session",
        "session_pool",
        "return_model",
        "hf_hub",
        "reqwest::Client",
    ]
    .iter()
    .any(|pattern| source.contains(pattern))
}

#[test]
fn qwen_generation_state_is_request_local_and_transport_free() {
    let root = workspace_root();
    let source =
        fs::read_to_string(root.join("spider/src/features/qwen3_vl_generation.rs")).unwrap();
    assert!(source.contains("weights: VarBuilder<'static>"));
    assert!(source.contains("Qwen3VLModel::new(&self.config, self.weights.clone())"));
    assert!(source.contains("_serialized_permit: tokio::sync::OwnedMutexGuard<()>"));
    assert!(source.contains("lock_owned().await"));
    assert!(!qwen_generation_state_violation(&source));
}

#[test]
fn qwen_cpu_runtime_is_installation_only_and_transport_free() {
    let source =
        fs::read_to_string(workspace_root().join("spider/src/features/qwen3_vl_runtime.rs"))
            .unwrap();
    assert!(source.contains("installation.reverify()?"));
    assert!(source.contains("Qwen3VlGenerationFactory"));
    assert!(source.contains("Device::Cpu"));
    for forbidden in ["reqwest::", "wreq::", "hf_hub", "Client::new", ".send()"] {
        assert!(!source.contains(forbidden), "runtime contains {forbidden}");
    }
}

#[test]
fn scanner_rejects_synthetic_qwen_runtime_network_and_fallbacks() {
    for fixture in [
        "reqwest::Client::new()",
        "wreq::Client::new()",
        "hf_hub::api::sync::Api::new()",
        "if cpu_fails { Device::new_cuda(0) }",
    ] {
        assert!(
            fixture.contains("Client::new")
                || fixture.contains("hf_hub")
                || fixture.contains("new_cuda"),
            "negative fixture was not detected"
        );
    }
}

#[test]
fn scanner_rejects_synthetic_qwen_generation_state_leaks() {
    for fixture in [
        "model: Mutex<Qwen3VLModel>",
        "Arc<Qwen3VLModel>",
        "recycle_session(session)",
        "session_pool.push(model)",
        "return_model(model)",
        "hf_hub::api::Api::new()",
        "reqwest::Client::new()",
    ] {
        assert!(qwen_generation_state_violation(fixture));
    }
    assert!(!qwen_generation_state_violation(
        "factory.begin_request().await?"
    ));
}

fn qwen_structured_generation_violation(source: &str) -> bool {
    [
        "push_str(\"}\")",
        "trim_end_matches",
        "extract_first_valid",
        "unwrap_or_default",
        "CaptchaSolution",
        "reqwest::Client",
        "Device::new_cuda",
    ]
    .iter()
    .any(|pattern| source.contains(pattern))
}

#[test]
fn qwen_structured_generation_is_token_constrained_and_runtime_owned() {
    let source =
        fs::read_to_string(workspace_root().join("spider/src/features/qwen3_vl_runtime.rs"))
            .unwrap();
    assert!(source.contains("fn constrained_token("));
    assert!(source.contains("NoValidStructuredContinuation"));
    assert!(source.contains("schema_state(schema, &text)"));
    assert!(!qwen_structured_generation_violation(&source));
}

#[test]
fn scanner_rejects_synthetic_qwen_structured_output_repairs() {
    for fixture in [
        "output.push_str(\"}\")",
        "output.trim_end_matches(',')",
        "extract_first_valid(output)",
        "parse(output).unwrap_or_default()",
        "fn decode(value: CaptchaSolution)",
        "reqwest::Client::new()",
        "Device::new_cuda(0)",
    ] {
        assert!(qwen_structured_generation_violation(fixture));
    }
}

fn qwen_captcha_provider_violation(source: &str) -> bool {
    [
        "Qwen3VLModel::new",
        "VarBuilder::from_",
        "fn constrained_token(",
        "reqwest::Client",
        "wreq::Client",
        "Device::new_cuda",
        "EmpiricallyQualified",
        "fallback_provider",
    ]
    .iter()
    .any(|pattern| source.contains(pattern))
}

#[test]
fn qwen_captcha_provider_is_runtime_owned_and_unqualified() {
    let source =
        fs::read_to_string(workspace_root().join("spider/src/features/qwen3_vl_captcha.rs"))
            .unwrap();
    assert!(source.contains("Qwen3VlCpuRuntime"));
    assert!(source.contains("generate_structured("));
    assert!(source.contains("ExecutableUnqualified"));
    assert!(!qwen_captcha_provider_violation(&source));
}

#[test]
fn scanner_rejects_synthetic_qwen_captcha_authority_leaks() {
    for fixture in [
        "Qwen3VLModel::new(config, weights)",
        "VarBuilder::from_mmaped_safetensors(files)",
        "fn constrained_token(logits: Tensor)",
        "reqwest::Client::new()",
        "wreq::Client::new()",
        "Device::new_cuda(0)",
        "qualification = EmpiricallyQualified",
        "fallback_provider.solve(request)",
    ] {
        assert!(qwen_captcha_provider_violation(fixture));
    }
}

fn captcha_corpus_governance_violation(source: &str) -> bool {
    [
        "impl From<CaptchaCorpusDraft> for FrozenCaptchaCorpus",
        "pub draft: CaptchaCorpusDraft",
        "minimum_cases: 199",
        "annotations.len() < 1",
        "qualification_split.unsealed()",
        "provider_output.relabel",
        "reqwest::Client",
        "Qwen3VlGenerationFactory",
    ]
    .iter()
    .any(|pattern| source.contains(pattern))
}

#[test]
fn canonical_captcha_corpus_protocol_is_provider_and_transport_neutral() {
    let root = workspace_root();
    let source =
        fs::read_to_string(root.join("spider/src/features/captcha_evaluation_corpus.rs")).unwrap();
    assert!(source.contains("pub struct FrozenCaptchaCorpus"));
    assert!(source.contains("const MINIMUM_CASES: usize = 200"));
    assert!(source.contains("pub fn freeze("));
    assert!(source.contains("qualification_seal_record"));
    assert!(!captcha_corpus_governance_violation(&source));
}

#[test]
fn scanner_rejects_synthetic_captcha_corpus_governance_bypasses() {
    for fixture in [
        "impl From<CaptchaCorpusDraft> for FrozenCaptchaCorpus",
        "pub draft: CaptchaCorpusDraft",
        "minimum_cases: 199",
        "annotations.len() < 1",
        "qualification_split.unsealed()",
        "provider_output.relabel(case)",
        "impl CorpusLoader { reqwest::Client }",
        "evaluate(Qwen3VlGenerationFactory, draft)",
    ] {
        assert!(captcha_corpus_governance_violation(fixture));
    }
}

fn browser_challenge_authority_violation(source: &str) -> bool {
    [
        "find_element(",
        "find_elements(",
        "query_selector(",
        ".clamp(",
        "nearest_element",
        "fallback_click",
        "retry_action",
        "let _ = page.click",
        "this.click()",
        "provider.solve",
        "CaptchaSolveRequest",
    ]
    .iter()
    .any(|pattern| source.contains(pattern))
}

#[test]
fn canonical_browser_challenge_seam_owns_only_snapshot_and_exact_action() {
    let source =
        fs::read_to_string(workspace_root().join("spider/src/features/browser_challenge.rs"))
            .unwrap();
    assert!(source.contains("pub struct BrowserChallengeSnapshot"));
    assert!(source.contains("self.revalidate(page).await?"));
    assert!(source.contains("backend_node_id"));
    assert!(source.contains("BrowserChallengeFailure::UnsupportedContext"));
    assert!(!browser_challenge_authority_violation(&source));
}

#[test]
fn vendored_chromey_exposes_one_session_routed_oopif_handle() {
    let root = workspace_root();
    let browser = fs::read_to_string(root.join("vendor/chromey/src/browser.rs")).unwrap();
    let handler = fs::read_to_string(root.join("vendor/chromey/src/handler/mod.rs")).unwrap();
    let target = fs::read_to_string(root.join("vendor/chromey/src/handler/target.rs")).unwrap();

    assert!(browser.contains("pub struct AttachedTargetSession"));
    assert!(browser.contains("pub async fn attached_session"));
    assert!(browser.contains("CommandMessage::with_session"));
    assert!(handler.contains("AttachedSessionState"));
    assert!(target.contains("TargetEvent::AttachedToTarget"));
    assert!(!browser.contains("WebSocketStream"));
    assert!(!browser.contains("selector"));
}

#[test]
fn captcha_layers_do_not_own_chromium_target_session_lifecycle() {
    let root = workspace_root();
    for relative in [
        "spider/src/features/captcha.rs",
        "spider/src/features/captcha",
        "spider/src/features/solvers.rs",
        "spider/src/features/solvers",
    ] {
        let path = root.join(relative);
        if path.is_file() {
            let source = fs::read_to_string(path).unwrap();
            assert!(!source.contains("AttachedTargetSession"));
            assert!(!source.contains("attached_session("));
            assert!(!source.contains("Target.attachedToTarget"));
        }
    }
}

#[test]
fn scanner_rejects_synthetic_browser_challenge_identity_and_action_bypasses() {
    for fixture in [
        "page.find_element(selector).await?.click().await?",
        "page.find_elements(selector).await?[index].click().await?",
        "document.query_selector(selector)",
        "x.clamp(0.0, width)",
        "nearest_element(point)",
        "fallback_click(target)",
        "retry_action(action)",
        "let _ = page.click(point).await",
        "element.call_js_fn(\"function(){this.click()}\", false)",
        "provider.solve(CaptchaSolveRequest::new(challenge))",
    ] {
        assert!(browser_challenge_authority_violation(fixture));
    }
}

// ---------------------------------------------------------------------------
// CANONICAL FRAME-CONTEXT IDENTITY SEAM
// ---------------------------------------------------------------------------
// SCORPION_CANONICAL_CHROMIUM_OOPIF_TARGET_SESSION_AND_FRAME_CONTEXT_001
//
// FrameId -> TargetId -> SessionId -> ExecutionContextId -> frame DOM
// identity -> frame owner -> lifecycle -> revalidation.

#[test]
fn canonical_frame_context_module_exists() {
    assert_feature_module_declared("frame_context");
}

#[test]
fn frame_context_struct_and_failure_enum_are_unique() {
    assert_pattern_only_in_files(
        "pub struct FrameContext {",
        &["features/frame_context.rs"],
        "FrameContext must only be defined in the canonical frame_context module",
    );
    assert_pattern_only_in_files(
        "pub enum FrameContextFailure",
        &["features/frame_context.rs"],
        "FrameContextFailure must only be defined in the canonical frame_context module",
    );
}

#[test]
fn canonical_frame_context_seam_owns_identity_chain_and_revalidation() {
    let source =
        fs::read_to_string(workspace_root().join("spider/src/features/frame_context.rs")).unwrap();
    assert!(source.contains("pub struct FrameContext"));
    assert!(source.contains("pub async fn resolve_top_level"));
    assert!(source.contains("pub async fn resolve_child"));
    assert!(source.contains("pub async fn revalidate"));
    assert!(source.contains("pub async fn resolve_dom_identity"));
    assert!(source.contains("pub async fn revalidate_dom_identity"));
    assert!(source.contains("GetFrameOwnerParams"));
    assert!(source.contains("FrameContextFailure::FrameTargetAssociationAmbiguous"));
    assert!(source.contains("FrameContextFailure::FrameOwnerChanged"));
    assert!(source.contains("FrameContextFailure::ExecutionContextChanged"));
}

#[test]
fn only_frame_context_calls_raw_attached_session_api() {
    // This is the frontier's core seam guarantee: chromey owns raw
    // TargetId<->SessionId attachment; this module owns turning that into
    // canonical frame identity. Nothing else in the crate may reach past it.
    assert_pattern_only_in_files(
        "browser.attached_session(",
        &["features/frame_context.rs"],
        "raw chromey Browser::attached_session must only be called from the canonical frame-context seam",
    );
    assert_pattern_only_in_files(
        "AttachedTargetSession",
        &["features/frame_context.rs"],
        "the raw chromey attached-session handle must not be named or stored outside the canonical frame-context seam",
    );
}

#[test]
fn frame_context_has_no_second_cdp_transport_stack() {
    let source =
        fs::read_to_string(workspace_root().join("spider/src/features/frame_context.rs")).unwrap();
    for forbidden in [
        "WebSocketStream",
        "tokio_tungstenite",
        "Connection::connect",
        "Browser::launch",
        "Browser::connect",
    ] {
        assert!(
            !source.contains(forbidden),
            "frame_context.rs must not implement or open a second CDP transport stack: found {forbidden:?}"
        );
    }
}

#[test]
fn frame_context_never_resolves_identity_by_selector() {
    let source =
        fs::read_to_string(workspace_root().join("spider/src/features/frame_context.rs")).unwrap();
    for forbidden in ["QuerySelectorParams", "find_element("] {
        assert!(
            !source.contains(forbidden),
            "frame_context.rs must never resolve canonical identity via a selector: found {forbidden:?}"
        );
    }
    assert!(source.contains("nothing is inferred from a selector"));
}

#[test]
fn frame_context_owns_no_captcha_or_provider_vocabulary() {
    let source =
        fs::read_to_string(workspace_root().join("spider/src/features/frame_context.rs")).unwrap();
    for forbidden in [
        "CaptchaSolveRequest",
        "provider.solve(",
        "Turnstile",
        "SpiderCloudMode",
        "Qwen3",
        "GeminiConfig",
        "GPTConfigs",
    ] {
        assert!(
            !source.contains(forbidden),
            "frame_context.rs must not own CAPTCHA/provider vocabulary: found {forbidden:?}"
        );
    }
}

#[test]
fn captcha_and_browser_challenge_cannot_reconstruct_frame_identity() {
    let root = workspace_root();
    for relative in [
        "spider/src/features/captcha.rs",
        "spider/src/features/captcha",
        "spider/src/features/captcha_evaluation_corpus.rs",
        "spider/src/features/solvers.rs",
        "spider/src/features/solvers",
        "spider/src/features/browser_challenge.rs",
        "spider/src/features/qwen3_vl_captcha.rs",
        "spider/src/features/captcha_browser.rs",
    ] {
        let path = root.join(relative);
        let mut files = Vec::new();
        if path.is_file() {
            let contents = fs::read_to_string(&path).unwrap();
            files.push(SourceFile {
                relative_path: relative.to_string(),
                contents,
            });
        } else if path.is_dir() {
            collect_rust_files(&path, &path, &mut files);
        }
        for file in files {
            for forbidden in [
                "AttachedTargetSession",
                "attached_session(",
                "GetFrameOwnerParams",
                "GetFrameTreeParams",
                "Target.attachedToTarget",
            ] {
                assert!(
                    !file.contents.contains(forbidden),
                    "{relative}/{} must not reconstruct frame identity via {forbidden:?}; it must consume the canonical FrameContext seam instead",
                    file.relative_path
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// FRAME-AWARE BROWSER CHALLENGE SNAPSHOT/ACTION SEAM
// ---------------------------------------------------------------------------
// SCORPION_CANONICAL_BROWSER_FRAME_CONTEXT_SNAPSHOT_AND_ACTION_001
//
// Composes the existing canonical browser-challenge snapshot/action
// primitive with the existing canonical FrameContext identity seam — never
// a second snapshot/action stack, never a second frame-identity
// reconstruction, never a CAPTCHA/provider path that reaches a browser or
// frame handle directly.

#[test]
fn frame_aware_browser_challenge_seam_composes_canonical_frame_context() {
    let source =
        fs::read_to_string(workspace_root().join("spider/src/features/browser_challenge.rs"))
            .unwrap();
    assert!(source.contains("pub async fn capture_in_frame"));
    assert!(source.contains("pub async fn revalidate_in_frame"));
    assert!(source.contains("pub async fn apply_in_frame"));
    assert!(source.contains("use crate::features::frame_context::"));
    assert!(source.contains("FrameContext"));
    assert!(source.contains("BrowserChallengeFailure::FrameDetached"));
    assert!(source.contains("BrowserChallengeFailure::FrameNavigated"));
    assert!(source.contains("BrowserChallengeFailure::TargetReplaced"));
    assert!(source.contains("BrowserChallengeFailure::SessionChanged"));
    assert!(source.contains("BrowserChallengeFailure::ExecutionContextChanged"));
    assert!(source.contains("BrowserChallengeFailure::FrameOwnerChanged"));
    assert!(source.contains("BrowserChallengeFailure::FrameGeometryUnavailable"));
    assert!(source.contains("BrowserChallengeFailure::FrameTransformAmbiguous"));
    assert!(!browser_challenge_authority_violation(&source));
}

#[test]
fn frame_aware_browser_challenge_actions_are_the_only_frame_aware_action_stack() {
    // The frontier's core "never a second snapshot/action stack" guarantee:
    // these three entry points may only be defined where the canonical
    // browser-challenge primitive already lives.
    assert_pattern_only_in_files(
        "pub async fn capture_in_frame",
        &["features/browser_challenge.rs"],
        "capture_in_frame must only be defined in the canonical browser-challenge module",
    );
    assert_pattern_only_in_files(
        "pub async fn revalidate_in_frame",
        &["features/browser_challenge.rs"],
        "revalidate_in_frame must only be defined in the canonical browser-challenge module",
    );
    assert_pattern_only_in_files(
        "pub async fn apply_in_frame",
        &["features/browser_challenge.rs"],
        "apply_in_frame must only be defined in the canonical browser-challenge module",
    );
}

#[test]
fn captcha_layers_cannot_own_frame_transforms_or_reach_browser_frame_handles() {
    // Scoped to the canonical CaptchaProvider seam only (`captcha.rs`'s
    // trait/request/outcome vocabulary, its corpus, and the local Qwen
    // provider): `solvers.rs` is the separate, pre-existing legacy solving
    // pipeline (out of scope for this frontier — see
    // SCORPION_CANONICAL_BROWSER_FRAME_CONTEXT_SNAPSHOT_AND_ACTION_001's
    // OUT OF SCOPE list) that already holds a direct `Page` reference for
    // its own, unrelated token-injection flow; this guardrail only proves
    // the *canonical provider* contract stays image-in/outcome-out.
    let root = workspace_root();
    for relative in [
        "spider/src/features/captcha.rs",
        "spider/src/features/captcha",
        "spider/src/features/captcha_evaluation_corpus.rs",
        "spider/src/features/qwen3_vl_captcha.rs",
    ] {
        let path = root.join(relative);
        let mut files = Vec::new();
        if path.is_file() {
            let contents = fs::read_to_string(&path).unwrap();
            files.push(SourceFile {
                relative_path: relative.to_string(),
                contents,
            });
        } else if path.is_dir() {
            collect_rust_files(&path, &path, &mut files);
        }
        for file in files {
            for forbidden in [
                // No provider/CAPTCHA code may hold a browser or frame
                // handle directly.
                "chromiumoxide::Page",
                "chromiumoxide::browser::Browser",
                "frame_context::FrameContext",
                // No provider/CAPTCHA code may reimplement the frame-owner
                // transform composition the browser-challenge seam owns.
                "FrameOwnerOffset",
                "resolve_frame_owner_offset",
                "capture_in_frame",
                "apply_in_frame",
            ] {
                assert!(
                    !file.contents.contains(forbidden),
                    "{relative}/{} must not own frame transforms or reach a browser/frame handle via {forbidden:?}; CAPTCHA reasoning stays image-in/outcome-out",
                    file.relative_path
                );
            }
        }
    }
}

#[test]
fn scanner_rejects_synthetic_frame_aware_browser_challenge_bypasses() {
    for fixture in [
        "page.find_element(selector).await?.click_in_frame(point).await?",
        "document.query_selector(selector)",
        "x.clamp(0.0, frame_width)",
        "nearest_element(point)",
        "fallback_click(target)",
        "retry_action(action)",
        "let _ = page.click(point).await",
        "provider.solve(CaptchaSolveRequest::new(challenge))",
    ] {
        assert!(browser_challenge_authority_violation(fixture));
    }
}

// ---------------------------------------------------------------------------
// CANONICAL CAPTCHA BROWSER EXECUTION BINDING
// ---------------------------------------------------------------------------
// SCORPION_CANONICAL_CAPTCHA_BROWSER_EXECUTION_BINDING_001
//
// Thin composition of one immutable BrowserChallengeSnapshot, one normalized
// CaptchaSolveRequest, one explicitly selected provider attempt and the
// exact browser action already owned by the snapshot seam.

fn captcha_browser_binding_violation(source: &str) -> bool {
    [
        "find_element(",
        "find_elements(",
        "click_smooth(",
        "click_and_drag_smooth(",
        "call_js_fn(",
        "Qwen3VlCpuRuntime",
        "generate_structured(",
        "fallback_provider",
        "retry_challenge",
        ".clamp(",
    ]
    .iter()
    .any(|pattern| source.contains(pattern))
}

#[test]
fn captcha_browser_binding_composes_canonical_seams_without_new_authority() {
    let source =
        fs::read_to_string(workspace_root().join("spider/src/features/captcha_browser.rs"))
            .unwrap();
    assert!(source.contains("BrowserChallengeSnapshot"));
    assert!(source.contains("CaptchaRouteAttempts"));
    assert!(source.contains("CaptchaSolveRequest"));
    assert!(source.contains("snapshot.revalidate(page).await"));
    assert!(source.contains("snapshot.apply(page, action).await"));
    assert!(!captcha_browser_binding_violation(&source));
}

#[test]
fn scanner_rejects_synthetic_captcha_browser_authority_leaks() {
    for fixture in [
        "page.find_element(selector).await?",
        "page.find_elements(selector).await?",
        "page.click_smooth(point).await?",
        "page.click_and_drag_smooth(from, to).await?",
        "element.call_js_fn(script, false).await?",
        "Qwen3VlCpuRuntime::initialize(installation, ram)",
        "runtime.generate_structured(request).await?",
        "fallback_provider.solve(request).await",
        "retry_challenge(challenge).await",
        "x.clamp(0.0, width)",
    ] {
        assert!(captcha_browser_binding_violation(fixture));
    }
}

#[test]
fn captcha_browser_binding_composes_frame_aware_seam_without_duplicating_routing() {
    // SCORPION_CANONICAL_BROWSER_FRAME_CONTEXT_SNAPSHOT_AND_ACTION_001's
    // successor: the frame-aware entry point must compose
    // revalidate_in_frame/apply_in_frame (never a second frame-aware action
    // stack, never a raw frame-identity reconstruction), and must share
    // materialization/action-selection with the top-level entry point
    // rather than duplicating provider-routing logic.
    let source =
        fs::read_to_string(workspace_root().join("spider/src/features/captcha_browser.rs"))
            .unwrap();
    assert!(source.contains("pub async fn execute_browser_captcha_attempt_in_frame"));
    assert!(source.contains("snapshot.revalidate_in_frame(page, top_level, frame).await"));
    assert!(source.contains(".apply_in_frame(page, top_level, frame, action)"));
    assert!(source.contains("use crate::features::frame_context::FrameContext"));
    assert_eq!(source.matches("fn materialize_request(").count(), 1);
    assert_eq!(source.matches("fn actions_for_solution(").count(), 1);
    assert_eq!(
        source.matches("async fn solve_and_select_actions(").count(),
        1
    );
    assert!(!captcha_browser_binding_violation(&source));
    for forbidden in [
        "AttachedTargetSession",
        "attached_session(",
        "GetFrameOwnerParams",
        "GetFrameTreeParams",
        "Target.attachedToTarget",
        "FrameOwnerOffset",
        "resolve_frame_owner_offset",
    ] {
        assert!(
            !source.contains(forbidden),
            "captcha_browser.rs must not reconstruct frame identity or transforms via {forbidden:?}"
        );
    }
}
