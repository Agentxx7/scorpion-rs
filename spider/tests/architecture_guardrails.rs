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
        "pub struct EvidenceRef",
        "pub enum EvidenceLedgerError",
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

// --- SECTION: SCORPION_CANONICAL_CACHE_IDENTITY_AND_LEGACY_STACK_CONVERGENCE_001 ---
//
// Before this frontier, four legacy Chrome-hybrid cache read/write paths in
// `spider/src/utils/mod.rs` (`cache_chrome_response`,
// `cache_chrome_response_from_cdp_body`, `cache_http_response_skip_browser`,
// and both `get_cached_url_base` variants) bound the live `Authorization`
// token out of `CacheOptions::Authorized` / `SkipBrowserAuthorized` and
// threaded it straight into `create_cache_key_raw`'s plaintext key —
// persisted into the same `CACACHE_MANAGER` the canonical HTTP executor
// (`cache_request::CanonicalCacheExecutor`) reads from, and in the
// `chrome_remote_cache` case uploaded verbatim to the remote cache server
// inside a `DumpJob`. `CanonicalCacheExecutor`'s own `cacheable_request()`
// never partitions by credential — it refuses to cache anything carrying
// Authorization/Cookie/Proxy-Authorization at all. These paths now
// converge onto that same fail-closed rule instead of embedding the
// credential in the cache identity.

#[test]
fn legacy_chrome_cache_paths_never_embed_credentials_in_cache_identity() {
    let utils = fs::read_to_string(workspace_root().join("spider/src/utils/mod.rs")).unwrap();

    // The exact historical patterns that embedded a live Authorization
    // token in a legacy cache key or a remote-cache `DumpJob`. None of
    // these forms may reappear.
    for forbidden in [
        "SkipBrowserAuthorized(token)) => Some(token)",
        "auth_opt.map(|token| token.as_ref())",
        "auth_opt.map(|x| x.as_str())",
        "create_cache_key_raw(target_url, Some(method), auth_opt, namespace)",
    ] {
        assert!(
            !utils.contains(forbidden),
            "credential re-entered legacy cache identity via {forbidden:?}"
        );
    }

    // Each entry point that persists into (or reads from) the hybrid
    // CACACHE_MANAGER / remote cache server must still exist...
    for owner_fn in [
        "pub async fn cache_chrome_response(",
        "pub async fn cache_http_response_skip_browser(",
        "async fn cache_chrome_response_from_cdp_body(",
        "pub async fn get_cached_url_base(",
    ] {
        assert!(
            utils.contains(owner_fn),
            "missing legacy chrome cache entry point {owner_fn}"
        );
    }

    // ...and must still fail closed (bail before building any key or
    // remote payload) on an authenticated `CacheOptions` variant, mirroring
    // `cacheable_request()` in cache_request.rs.
    assert!(
        utils.matches("CacheOptions::Authorized(_)").count() >= 5,
        "expected a fail-closed auth guard in each legacy chrome cache read/write path"
    );
    assert!(
        utils.matches("SkipBrowserAuthorized(_)").count() >= 5,
        "expected a fail-closed auth guard in each legacy chrome cache read/write path"
    );
}

// --- SECTION: SCORPION_CANONICAL_EXECUTION_RAW_HTTP_ESCAPE_CONVERGENCE_001 ---
//
// `utils::fetch_page_html`'s chrome-error recovery fallback (both the
// `fs`+`chrome` and `not(fs)`+`chrome` builds) used to unconditionally
// escape through a raw `client.get(url).send()` / `fetch_page_html_raw`
// call on any chrome navigation error — bypassing `ResolvedExecutor`
// entirely (no Tor/SSRF policy, no truthful BackendProvenance/
// ResponseOrigin, and silently swallowing invalid proxy configuration via
// `configure_http_client_builder`'s `if let Ok(proxy) = ...` skip) even
// when `ExecutionMode::Canonical`/`CanonicalWreq` was active — despite a
// comment in `Website::setup()` claiming "Canonical Page execution never
// consults it". Both sites now check
// `crate::website::current_canonical_executor()` first and route through
// `fetch_page_html_with_executor` whenever a canonical executor is in
// scope, keeping the raw-client recovery path for genuinely noncanonical
// execution only. `CANONICAL_EXECUTOR_SCOPE` is established at every
// chrome-touching top-level crawl entry point (`crawl`,
// `crawl_sitemap_chrome`, `crawl_smart`).

/// Slice `haystack` starting at the byte offset of `anchor`, up to (and
/// including) the first occurrence of `needle` found after it. Panics
/// with a descriptive message if either is missing — used to prove one
/// specific, textually-identified fallback site is gated, without being
/// confused by unrelated occurrences of the same short substrings
/// elsewhere in a 12k-line file.
fn assert_anchor_precedes(haystack: &str, anchor: &str, needle: &str, max_gap: usize) {
    let anchor_pos = haystack
        .find(anchor)
        .unwrap_or_else(|| panic!("expected anchor present: {anchor:?}"));
    let after_anchor = &haystack[anchor_pos..];
    let needle_pos = after_anchor
        .find(needle)
        .unwrap_or_else(|| panic!("expected {needle:?} after anchor {anchor:?}"));
    assert!(
        needle_pos < max_gap,
        "expected {needle:?} within {max_gap} bytes after anchor {anchor:?}, found at {needle_pos}"
    );
}

#[test]
fn chrome_error_recovery_fetch_checks_canonical_executor_before_raw_client() {
    let utils = fs::read_to_string(workspace_root().join("spider/src/utils/mod.rs")).unwrap();

    // Both fallback sites must route through the canonical executor
    // seam — exactly once each, in the fs+chrome and not(fs)+chrome
    // builds of `fetch_page_html`.
    let route_marker =
        "Some(executor) => fetch_page_html_with_executor(target_url, &executor).await";
    assert_eq!(
        utils.matches(route_marker).count(),
        2,
        "expected the chrome-error recovery fetch to route through \
         fetch_page_html_with_executor in both the fs+chrome and \
         not(fs)+chrome builds of fetch_page_html"
    );

    // fs+chrome site (`fetch_page_html`): its own comment (unique text,
    // distinct from the not(fs)+chrome site's) must be immediately
    // followed by a canonical-executor check, itself immediately followed
    // by the raw `client.get(...).send()` escape it guards.
    let fs_chrome_anchor = "// Chrome-error recovery fetch. Canonical execution (a\n";
    assert_anchor_precedes(
        &utils,
        fs_chrome_anchor,
        "current_canonical_executor()",
        800,
    );
    assert_anchor_precedes(
        &utils,
        fs_chrome_anchor,
        "client.get(target_url).send().await",
        1_500,
    );

    // not(fs)+chrome site (`fetch_page_html_base`): same shape, guarding
    // `fetch_page_html_raw(target_url, client).await` instead. This is
    // NOT the same raw-fallback text as the legitimate, always-unguarded
    // not(fs)+not(chrome) build (chrome isn't even compiled there, so no
    // executor could ever exist) — the anchor disambiguates which call
    // site is under test.
    let not_fs_chrome_anchor = "// Chrome-error recovery fetch. Canonical execution (a resolved\n";
    assert_anchor_precedes(
        &utils,
        not_fs_chrome_anchor,
        "current_canonical_executor()",
        800,
    );
    assert_anchor_precedes(
        &utils,
        not_fs_chrome_anchor,
        "fetch_page_html_raw(target_url, client).await",
        1_000,
    );
}

#[test]
fn chrome_html_direct_api_has_no_internal_caller() {
    // `_fetch_page_html_chrome` (and its public wrappers
    // `fetch_page_html_chrome` / `fetch_page_html_chrome_seeded`) keep the
    // legacy unconditional raw-client chrome-error fallback — acceptable
    // ONLY because nothing in this workspace's `Website`/`Page` canonical
    // crawl graph calls them. If that ever changes, the new caller
    // reintroduces exactly the raw-HTTP-escape-from-canonical-execution
    // bug this frontier closed for `fetch_page_html`/`fetch_page_html_base`.
    let utils = fs::read_to_string(workspace_root().join("spider/src/utils/mod.rs")).unwrap();
    assert!(
        utils.contains("UPSTREAM_COMPATIBILITY_BOUNDARY: this (and its public wrappers"),
        "_fetch_page_html_chrome must stay explicitly classified as an \
         upstream-compatibility, direct-API-only boundary"
    );

    for file in ["website.rs", "page.rs"] {
        let source = fs::read_to_string(workspace_root().join("spider/src").join(file)).unwrap();
        for callee in ["fetch_page_html_chrome(", "fetch_page_html_chrome_seeded("] {
            assert!(
                !source.contains(callee),
                "{file} must not call {callee} — Website's canonical crawl entry points use \
                 fetch_page_html/fetch_page_html_base instead, which route their chrome-error \
                 recovery fetch through current_canonical_executor(); wiring this direct-API \
                 function into Website would bypass that convergence"
            );
        }
    }
}

#[test]
fn canonical_executor_scope_established_at_every_chrome_crawl_entry_point() {
    let website = fs::read_to_string(workspace_root().join("spider/src/website.rs")).unwrap();

    assert!(
        website.contains("tokio::task_local! {")
            && website.contains("pub(crate) static CANONICAL_EXECUTOR_SCOPE:"),
        "CANONICAL_EXECUTOR_SCOPE task-local must be declared in website.rs"
    );
    assert!(
        website.contains("pub(crate) fn current_canonical_executor()"),
        "current_canonical_executor() reader must be declared in website.rs"
    );

    // One `.scope(canonical_executor, ...)` establishment per chrome-touching
    // top-level crawl entry point: `crawl`, `crawl_sitemap_chrome`,
    // `crawl_smart`. A regression here silently reopens the raw-client
    // escape for whichever entry point stops establishing the scope.
    let establishments = website
        .matches("let canonical_executor = self.resolved_executor.clone();")
        .count();
    assert!(
        establishments >= 3,
        "expected CANONICAL_EXECUTOR_SCOPE to be established at crawl, \
         crawl_sitemap_chrome, and crawl_smart; found {establishments} site(s)"
    );

    // The historical false claim must not reappear verbatim.
    assert!(
        !website.contains("// signatures. Canonical Page execution never consults it."),
        "the false 'Canonical Page execution never consults it' comment must not reappear \
         now that the chrome-error fallback genuinely can reach this client"
    );
}

// --- SECTION: SCORPION_PERSISTED_DOMAIN_IDENTITY_001 ---
//
// Track 1 of the frozen roadmap: identity for persisted domain objects.
// `EvidenceId` (SCORPION.md §3) and `WatchId` (SCORPION_SDD.md §5.2) are
// defined exactly once, in `spider/src/features/identity.rs` — identity
// only: explicit type, deterministic serialization, validating parse,
// value equality/hash/ordering. No persistence, no state/lifecycle, no
// domain object, no interface-local shadow type. WATCH/MONITOR's actual
// state model (`WatchDefinition`/`WatchState`/`Snapshot`/`Transition`)
// remains BLOCKED per SCORPION_SDD.md §5.2 — only identity exists.

#[test]
fn evidence_id_and_watch_id_are_defined_exactly_once() {
    assert_pattern_only_in_files(
        "struct EvidenceId",
        &["features/identity.rs"],
        "EvidenceId must only be defined in the canonical identity module",
    );
    assert_pattern_only_in_files(
        "struct WatchId",
        &["features/identity.rs"],
        "WatchId must only be defined in the canonical identity module",
    );
}

#[test]
fn identity_module_declared_unconditionally_in_features_mod() {
    let features_mod =
        fs::read_to_string(workspace_root().join("spider/src/features/mod.rs")).unwrap();
    let decl_index = features_mod
        .find("pub mod identity;")
        .expect("identity module not declared in features/mod.rs");
    let preceding = &features_mod[..decl_index];
    // Walk backward past this declaration's own doc comment (if any) to the
    // line immediately above it — that is the only line that could gate
    // *this* declaration. Looking further back would hit the previous,
    // unrelated module's own gate/doc comment instead.
    let gated = preceding
        .lines()
        .rev()
        .find(|line| !line.trim_start().starts_with("///"))
        .is_some_and(|line| line.trim_start().starts_with("#[cfg("));
    assert!(
        !gated,
        "identity module must be declared unconditionally — persisted-domain \
         identity must not depend on optional cargo features"
    );
}

#[test]
fn identity_module_has_no_persistence_or_lifecycle_implementation() {
    let identity =
        fs::read_to_string(workspace_root().join("spider/src/features/identity.rs")).unwrap();
    for forbidden in [
        "struct WatchState",
        "enum WatchState",
        "struct WatchDefinition",
        "enum WatchDefinition",
        "struct Snapshot",
        "struct Transition",
        "enum Transition",
        "sqlx::",
        "cacache::",
        "CACACHE_MANAGER",
        "tokio::fs::",
        "std::fs::File",
        "std::fs::write",
        "reqwest::",
        "chromiumoxide::",
    ] {
        assert!(
            !identity.contains(forbidden),
            "identity module must stay identity-only: found forbidden pattern {forbidden:?}"
        );
    }
}

#[test]
fn identity_types_have_deterministic_serialization_and_validation() {
    let identity =
        fs::read_to_string(workspace_root().join("spider/src/features/identity.rs")).unwrap();
    for marker in [
        "impl fmt::Display for EvidenceId",
        "impl FromStr for EvidenceId",
        "impl fmt::Display for WatchId",
        "impl FromStr for WatchId",
        "pub const PREFIX",
        "IdentityParseError",
    ] {
        assert!(
            identity.contains(marker),
            "identity module missing expected canonical marker: {marker:?}"
        );
    }
    // Distinct wire prefixes are the hard type boundary between the two
    // identity kinds — an EvidenceId string must never be a valid WatchId
    // string, and vice versa.
    assert!(identity.contains("\"evid_\""));
    assert!(identity.contains("\"watch_\""));
}

#[test]
fn no_shadow_ids_in_cli_or_mcp() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    for crate_dir in ["spider_cli/src", "spider_mcp/src"] {
        let dir = manifest_dir.parent().unwrap().join(crate_dir);
        if !dir.exists() {
            continue;
        }
        let mut files = Vec::new();
        collect_rust_files(&dir, &dir, &mut files);
        for file in files {
            for shadow in [
                "struct EvidenceId",
                "enum EvidenceId",
                "struct WatchId",
                "enum WatchId",
                "type EvidenceId",
                "type WatchId",
            ] {
                assert!(
                    !file.contents.contains(shadow),
                    "{crate_dir}/{} must not define its own {shadow:?} — EvidenceId/WatchId \
                     are owned exclusively by spider::features::identity",
                    file.relative_path
                );
            }
        }
    }
}

// --- SECTION: SCORPION_PERSISTED_DOMAIN_SEMANTICS_001 ---
//
// Track 2 of the frozen roadmap: canonical state/transition semantics for
// persisted domain objects, built on Track 1's identity. `CurrentState`,
// `HistoryEntry`, `HistoryLog`, and `Transition` are defined exactly once,
// in `spider/src/features/domain_state.rs` — semantics only: the
// transition contract (current state + explicit transition → new current
// state), "one current state per identity," "historical records are
// immutable/append-only," and the persistence/domain-decision ownership
// boundary. No database, no `WatchDefinition`/`WatchState` product model,
// no `AuthSessionId`, no scheduling, no `ChangeResult`/`ChangeEvent`, no
// health semantics — those remain later, separate frontier work. This
// frontier also reconciled the `Observation`/`Snapshot` bare-name naming
// collisions (`Fingerprint` deliberately deferred to Track 6).

#[test]
fn domain_state_types_are_defined_exactly_once() {
    for (pattern, description) in [
        (
            "struct CurrentState",
            "CurrentState must only be defined in the canonical domain_state module",
        ),
        (
            "struct HistoryEntry",
            "HistoryEntry must only be defined in the canonical domain_state module",
        ),
        (
            "struct HistoryLog",
            "HistoryLog must only be defined in the canonical domain_state module",
        ),
        (
            "trait Transition",
            "Transition must only be defined in the canonical domain_state module",
        ),
    ] {
        assert_pattern_only_in_files(pattern, &["features/domain_state.rs"], description);
    }
}

#[test]
fn domain_state_module_declared_unconditionally_in_features_mod() {
    let features_mod =
        fs::read_to_string(workspace_root().join("spider/src/features/mod.rs")).unwrap();
    let decl_index = features_mod
        .find("pub mod domain_state;")
        .expect("domain_state module not declared in features/mod.rs");
    let preceding = &features_mod[..decl_index];
    let gated = preceding
        .lines()
        .rev()
        .find(|line| !line.trim_start().starts_with("///"))
        .is_some_and(|line| line.trim_start().starts_with("#[cfg("));
    assert!(
        !gated,
        "domain_state module must be declared unconditionally — persisted-domain \
         state/transition semantics must not depend on optional cargo features"
    );
}

#[test]
fn domain_state_module_has_no_persistence_or_product_model_implementation() {
    let domain_state =
        fs::read_to_string(workspace_root().join("spider/src/features/domain_state.rs")).unwrap();
    for forbidden in [
        "struct WatchState",
        "enum WatchState",
        "struct WatchDefinition",
        "enum WatchDefinition",
        "struct AuthSessionId",
        "struct ChangeResult",
        "enum ChangeResult",
        "struct ChangeEvent",
        "enum ChangeEvent",
        "fn schedule",
        "struct Scheduler",
        "sqlx::",
        "cacache::",
        "CACACHE_MANAGER",
        "tokio::fs::",
        "std::fs::File",
        "std::fs::write",
        "reqwest::",
        "chromiumoxide::",
    ] {
        assert!(
            !domain_state.contains(forbidden),
            "domain_state module must stay semantics-only: found forbidden pattern {forbidden:?}"
        );
    }
}

#[test]
fn history_log_is_structurally_append_only() {
    let domain_state =
        fs::read_to_string(workspace_root().join("spider/src/features/domain_state.rs")).unwrap();
    // HistoryLog's only mutating method is `append`. None of these may
    // exist anywhere in the module — a `remove`/`clear`/mutable-access
    // method on HistoryLog (or a public field/IndexMut on HistoryEntry)
    // would let a caller alter or discard a historical record after the
    // fact, breaking the immutable/append-only invariant.
    for forbidden in [
        "fn remove(",
        "fn clear(",
        "fn get_mut(",
        "IndexMut<",
        "impl std::ops::IndexMut",
        "pub state: S",
        "pub identity: Id",
        "pub recorded_at",
    ] {
        assert!(
            !domain_state.contains(forbidden),
            "HistoryLog/HistoryEntry must stay append-only/immutable: found {forbidden:?}"
        );
    }
    assert!(
        domain_state.contains("fn append(&mut self, entry: HistoryEntry<Id, S>)"),
        "HistoryLog must expose its append operation with the expected signature"
    );
}

#[test]
fn transition_contract_matches_current_state_plus_transition_to_new_current_state() {
    let domain_state =
        fs::read_to_string(workspace_root().join("spider/src/features/domain_state.rs")).unwrap();
    // The canonical contract: Transition::apply is a pure fn(&S) -> Result<S, _>
    // (no storage handle, no identity, no I/O capability in the signature),
    // and CurrentState::apply is the only way to run one, producing exactly
    // one new CurrentState plus the HistoryEntry it superseded.
    assert!(domain_state.contains("fn apply(&self, current: &S) -> Result<S, Self::Rejection>"));
    assert!(domain_state.contains("pub fn apply<T: Transition<S>>("));
    assert!(domain_state.contains("Result<Applied<Id, S>, (Self, T::Rejection)>"));
}

#[test]
fn no_bare_observation_or_snapshot_types_introduced() {
    // Reconciled: `Observation` is owned by spider_agent_types::PageObservation
    // (a different crate/domain); `Snapshot` already has two qualified,
    // unrelated owners (VitalsSnapshot, BrowserChallengeSnapshot) plus a
    // locked informal use in SCORPION_SDD.md §5.2. Neither bare name may be
    // (re)introduced as a new canonical type anywhere in spider/src.
    for pattern in [
        "struct Observation",
        "enum Observation",
        "struct Snapshot",
        "enum Snapshot",
    ] {
        assert_pattern_only_in_files(
            pattern,
            &[],
            "bare Observation/Snapshot types are reconciled away — see \
             features/domain_state.rs's \"Naming reconciliation\" doc section",
        );
    }
}

#[test]
fn no_shadow_domain_state_types_in_cli_or_mcp() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    for crate_dir in ["spider_cli/src", "spider_mcp/src"] {
        let dir = manifest_dir.parent().unwrap().join(crate_dir);
        if !dir.exists() {
            continue;
        }
        let mut files = Vec::new();
        collect_rust_files(&dir, &dir, &mut files);
        for file in files {
            for shadow in [
                "struct CurrentState",
                "struct HistoryEntry",
                "struct HistoryLog",
                "trait Transition",
                "struct WatchState",
                "struct WatchDefinition",
            ] {
                assert!(
                    !file.contents.contains(shadow),
                    "{crate_dir}/{} must not define its own {shadow:?} — persisted-domain \
                     state/transition semantics are owned exclusively by \
                     spider::features::domain_state",
                    file.relative_path
                );
            }
        }
    }
}

// --- SECTION: SCORPION_CANONICAL_PERSISTENCE_MECHANISM_001 ---
//
// Track 3 of the frozen roadmap: one canonical persistence seam for the
// domain state Track 2 defines. `DomainPersistence` is defined exactly
// once, in `spider/src/features/domain_persistence.rs`, gated behind the
// existing `disk` feature (reusing the crate's sqlx/SQLite dependency
// rather than introducing a second persistence stack). It stores opaque
// identity-keyed bytes only: current-state writes are compare-and-swap
// (no unconditional overwrite method exists), and historical-record
// appends fail closed on a duplicate `(identity, revision)` key (the
// database's own primary-key constraint enforces this). The module never
// imports `EvidenceId`/`WatchId`/`CurrentState`/`Transition` and defines
// no domain product model, no lifecycle status, no scheduling, no event
// sourcing.

#[test]
fn domain_persistence_type_is_defined_exactly_once() {
    for (pattern, description) in [
        (
            "struct DomainPersistence",
            "DomainPersistence must only be defined in the canonical domain_persistence module",
        ),
        (
            "enum PersistenceError",
            "PersistenceError must only be defined in the canonical domain_persistence module",
        ),
    ] {
        assert_pattern_only_in_files(pattern, &["features/domain_persistence.rs"], description);
    }
}

#[test]
fn domain_persistence_module_gated_behind_disk_feature() {
    let features_mod =
        fs::read_to_string(workspace_root().join("spider/src/features/mod.rs")).unwrap();
    let decl_index = features_mod
        .find("pub mod domain_persistence;")
        .expect("domain_persistence module not declared in features/mod.rs");
    let preceding = &features_mod[..decl_index];
    let gate_line = preceding
        .lines()
        .rev()
        .find(|line| !line.trim_start().starts_with("///"))
        .expect("expected a line before the module declaration");
    assert_eq!(
        gate_line.trim(),
        "#[cfg(feature = \"disk\")]",
        "domain_persistence must be gated behind the existing disk feature — \
         it must not introduce a second, always-on storage stack"
    );
}

#[test]
fn domain_persistence_never_imports_domain_state_or_identity_types() {
    let domain_persistence =
        fs::read_to_string(workspace_root().join("spider/src/features/domain_persistence.rs"))
            .unwrap();
    // A storage mechanism that had to import a concrete domain type to
    // compile would already be deciding something about that domain —
    // this module takes only opaque `&str`/`&[u8]`. Checked as actual
    // `use` import lines, not bare prose mentions (the module's own doc
    // comments legitimately *name* EvidenceId/WatchId/CurrentState/
    // Transition in English, including as intra-doc links, to explain
    // the decoupling).
    for forbidden in [
        "use crate::features::identity",
        "use crate::features::domain_state",
        "use super::identity",
        "use super::domain_state",
    ] {
        assert!(
            !domain_persistence.contains(forbidden),
            "domain_persistence must stay decoupled from Track 1/2 Rust types: \
             found {forbidden:?}"
        );
    }
}

#[test]
fn domain_persistence_decides_no_domain_semantics() {
    let domain_persistence =
        fs::read_to_string(workspace_root().join("spider/src/features/domain_persistence.rs"))
            .unwrap();
    for forbidden in [
        "trait Transition",
        "struct WatchState",
        "enum WatchState",
        "struct WatchDefinition",
        "enum WatchDefinition",
        "struct AuthSessionId",
        "struct ChangeResult",
        "enum ChangeResult",
        "struct ChangeEvent",
        "enum ChangeEvent",
        "struct Fingerprint",
        "struct Lineage",
        "fn schedule",
        "struct Scheduler",
        "EventSourc",
        "struct Job",
        "struct Operation",
        "reqwest::",
        "chromiumoxide::",
    ] {
        assert!(
            !domain_persistence.contains(forbidden),
            "domain_persistence must stay a pure storage mechanism: \
             found forbidden pattern {forbidden:?}"
        );
    }
}

#[test]
fn current_state_writes_are_compare_and_swap_with_no_unconditional_overwrite() {
    let domain_persistence =
        fs::read_to_string(workspace_root().join("spider/src/features/domain_persistence.rs"))
            .unwrap();
    assert!(
        domain_persistence.contains("pub async fn write_current("),
        "expected the canonical current-state write method"
    );
    assert!(
        domain_persistence.contains("expected_revision: Option<u64>"),
        "write_current must require the caller's expected prior revision — \
         no blind overwrite"
    );
    assert!(
        domain_persistence.contains("if actual != expected_revision {")
            && domain_persistence.contains("CurrentStateConflict"),
        "write_current must fail closed when the expected revision does not match"
    );
    // No second, unconditional write path.
    for forbidden in [
        "pub async fn set_current(",
        "pub async fn overwrite_current(",
        "pub async fn force_write_current(",
        "pub async fn put_current(",
    ] {
        assert!(
            !domain_persistence.contains(forbidden),
            "found an unconditional current-state overwrite method: {forbidden:?} — \
             current state must only ever be written through compare-and-swap"
        );
    }
}

#[test]
fn historical_append_fails_closed_on_duplicate_key() {
    let domain_persistence =
        fs::read_to_string(workspace_root().join("spider/src/features/domain_persistence.rs"))
            .unwrap();
    assert!(
        domain_persistence.contains("pub async fn append_history("),
        "expected the canonical historical-append method"
    );
    assert!(
        domain_persistence.contains("PRIMARY KEY (identity, revision)"),
        "the history table must enforce uniqueness by (identity, revision) at the \
         database level, not merely in application logic"
    );
    assert!(
        domain_persistence.contains("is_unique_violation()")
            && domain_persistence.contains("HistoryAlreadyExists"),
        "a duplicate-key insert must be mapped to a fail-closed HistoryAlreadyExists error"
    );
    // No update/delete/replace path for history at all.
    for forbidden in [
        "UPDATE scorpion_domain_history",
        "DELETE FROM scorpion_domain_history",
        "INSERT OR REPLACE INTO scorpion_domain_history",
        "INSERT OR IGNORE INTO scorpion_domain_history",
    ] {
        assert!(
            !domain_persistence.contains(forbidden),
            "found a mutating/silently-replacing history query: {forbidden:?} — \
             historical records must be plain INSERT-or-fail"
        );
    }
}

#[test]
fn domain_persistence_reuses_existing_sqlite_stack_not_a_second_architecture() {
    let domain_persistence =
        fs::read_to_string(workspace_root().join("spider/src/features/domain_persistence.rs"))
            .unwrap();
    assert!(
        domain_persistence.contains("use sqlx::sqlite::"),
        "domain_persistence must reuse the crate's existing sqlx/SQLite dependency"
    );
    // It owns its own tables/pool rather than reusing disk.rs's
    // DatabaseHandler/resources/signatures schema (a different, upstream
    // crawl-resume mechanism with non-transition-aware semantics) — but it
    // must not define a second DatabaseHandler-shaped type either.
    assert!(!domain_persistence.contains("struct DatabaseHandler"));
    let cargo_toml = fs::read_to_string(workspace_root().join("spider/Cargo.toml")).unwrap();
    assert_eq!(
        cargo_toml.matches("dep:sqlx").count(),
        1,
        "expected exactly one sqlx dependency declaration — no second database crate introduced"
    );
}

#[test]
fn no_shadow_persistence_seam_in_cli_or_mcp() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    for crate_dir in ["spider_cli/src", "spider_mcp/src"] {
        let dir = manifest_dir.parent().unwrap().join(crate_dir);
        if !dir.exists() {
            continue;
        }
        let mut files = Vec::new();
        collect_rust_files(&dir, &dir, &mut files);
        for file in files {
            for shadow in [
                "struct DomainPersistence",
                "enum PersistenceError",
                "scorpion_domain_current_state",
                "scorpion_domain_history",
            ] {
                assert!(
                    !file.contents.contains(shadow),
                    "{crate_dir}/{} must not define its own {shadow:?} — the canonical \
                     persistence seam is owned exclusively by \
                     spider::features::domain_persistence",
                    file.relative_path
                );
            }
        }
    }
}

// --- SECTION: SCORPION_DURABLE_EVIDENCE_LEDGER_001 ---
//
// Track 4 of the frozen roadmap: EvidenceId becomes the canonical durable
// identity for evidence records, and EvidenceBundle (the existing
// canonical evidence model in `utils/evidence.rs` — never a second one)
// gains an `id` field plus the `backend_provenance`/`response_origin`
// provenance fields it was missing. `record_evidence`/`read_evidence`
// persist a bundle through `DomainPersistence`'s append-only historical
// semantics only — never its current-state compare-and-swap path, because
// evidence has no "current state" to replace. Every ledger write uses the
// fixed revision `1`, so Track 3's existing `(identity, revision)`
// uniqueness constraint is exactly what makes a duplicate `EvidenceId`
// write fail closed — this frontier adds no new conflict logic of its
// own. `EvidenceRef` is a pure `EvidenceId` wrapper: it never carries
// evidence content.

#[test]
fn evidence_ledger_types_are_defined_exactly_once() {
    for (pattern, description) in [
        (
            "struct EvidenceRef",
            "EvidenceRef must only be defined in the canonical evidence module",
        ),
        (
            "enum EvidenceLedgerError",
            "EvidenceLedgerError must only be defined in the canonical evidence module",
        ),
        (
            "fn record_evidence(",
            "record_evidence must only be defined in the canonical evidence module",
        ),
        (
            "fn read_evidence(",
            "read_evidence must only be defined in the canonical evidence module",
        ),
    ] {
        assert_pattern_only_in_files(pattern, &["utils/evidence.rs"], description);
    }
}

#[test]
fn evidence_ledger_writes_are_append_only_never_current_state() {
    let evidence =
        fs::read_to_string(workspace_root().join("spider/src/utils/evidence.rs")).unwrap();
    // Evidence has no "current state" to replace — Track 2's CAS
    // current-state semantics belong to future, genuinely stateful
    // capabilities (WatchState, ...), not to an immutable evidence record.
    assert!(
        evidence.contains(".append_history("),
        "record_evidence must persist through DomainPersistence::append_history"
    );
    assert!(
        !evidence.contains(".write_current("),
        "evidence must never be persisted through DomainPersistence::write_current — \
         it has no current state to compare-and-swap"
    );
    // Every evidence record is the one and only record for its EvidenceId
    // — the fixed revision `1`, never a counter that could imply
    // multiple revisions of "the same" evidence.
    assert!(evidence.contains("append_history(&id.to_string(), 1,"));
}

#[test]
fn evidence_ledger_never_defines_its_own_conflict_or_lifecycle_logic() {
    let evidence =
        fs::read_to_string(workspace_root().join("spider/src/utils/evidence.rs")).unwrap();
    // Duplicate-write and conflict handling must be inherited from
    // Track 3's PersistenceError, not reimplemented here.
    for forbidden in [
        "struct WatchState",
        "enum WatchState",
        "struct WatchDefinition",
        "enum WatchDefinition",
        "struct ChangeResult",
        "enum ChangeResult",
        "struct ChangeEvent",
        "enum ChangeEvent",
        "struct AuthSessionId",
        "struct Fingerprint",
        "struct Lineage",
        "fn schedule",
        "struct Scheduler",
        "EventSourc",
        "trait Transition",
    ] {
        assert!(
            !evidence.contains(forbidden),
            "evidence.rs must stay a ledger over the existing domain model: \
             found forbidden pattern {forbidden:?}"
        );
    }
}

#[test]
fn evidence_bundle_provenance_is_read_from_page_never_fabricated() {
    let evidence =
        fs::read_to_string(workspace_root().join("spider/src/utils/evidence.rs")).unwrap();
    // backend_provenance/response_origin must be populated by reading the
    // same canonical, already-audited Page accessors transport/dns
    // already use — never a literal/synthesized value.
    assert!(evidence.contains(".backend_provenance()"));
    assert!(evidence.contains(".response_origin()"));
    assert!(evidence.contains("page.transport()"));
}

#[test]
fn no_shadow_evidence_model_in_cli_or_mcp() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    for crate_dir in ["spider_cli/src", "spider_mcp/src"] {
        let dir = manifest_dir.parent().unwrap().join(crate_dir);
        if !dir.exists() {
            continue;
        }
        let mut files = Vec::new();
        collect_rust_files(&dir, &dir, &mut files);
        for file in files {
            for shadow in [
                "struct EvidenceBundle",
                "struct EvidenceRef",
                "enum EvidenceLedgerError",
                "fn record_evidence(",
                "fn read_evidence(",
            ] {
                assert!(
                    !file.contents.contains(shadow),
                    "{crate_dir}/{} must not define its own {shadow:?} — the canonical \
                     evidence ledger is owned exclusively by spider::utils::evidence",
                    file.relative_path
                );
            }
        }
    }
}

// --- SECTION: SCORPION_AUTHENTICATED_SESSION_LIFECYCLE_001 ---
//
// Track 5 of the frozen roadmap: AuthSessionId (identity.rs) and the
// authenticated-session lifecycle (auth_session.rs: AuthSessionState,
// PauseSession/ResumeSession/InvalidateSession transitions, persistence
// through DomainPersistence) — the first capability to use Track 2's
// full current-state + historical-record contract (not append-only-only,
// like Track 4's evidence ledger). No bare SessionId is introduced; no
// existing "session" meaning (chromiumoxide CDP SessionId, frame_context,
// spider_mcp CrawlSession) is redefined; no credential material can enter
// identity or lifecycle state.

#[test]
fn auth_session_id_is_defined_exactly_once() {
    assert_pattern_only_in_files(
        "struct AuthSessionId",
        &["features/identity.rs"],
        "AuthSessionId must only be defined in the canonical identity module",
    );
}

#[test]
fn no_bare_session_id_is_introduced() {
    // The frontier explicitly forbids a generic/bare SessionId — only the
    // domain-specific AuthSessionId. `chromiumoxide`'s own CDP SessionId
    // is vendored/upstream, not defined anywhere in spider/src, so this
    // scan can safely forbid the bare name outright.
    assert_pattern_only_in_files(
        "struct SessionId",
        &[],
        "no bare SessionId may be introduced — only the domain-specific AuthSessionId",
    );
}

#[test]
fn auth_session_lifecycle_types_are_defined_exactly_once() {
    for (pattern, description) in [
        (
            "enum AuthSessionState",
            "AuthSessionState must only be defined in the canonical auth_session module",
        ),
        (
            "enum AuthenticationProfile",
            "AuthenticationProfile must only be defined in the canonical auth_session module",
        ),
        (
            "struct BrowserContinuityToken",
            "BrowserContinuityToken must only be defined in the canonical auth_session module",
        ),
        (
            "enum AuthSessionTransitionRejected",
            "AuthSessionTransitionRejected must only be defined in the canonical auth_session module",
        ),
        (
            "struct PauseSession",
            "PauseSession must only be defined in the canonical auth_session module",
        ),
        (
            "struct ResumeSession",
            "ResumeSession must only be defined in the canonical auth_session module",
        ),
        (
            "struct InvalidateSession",
            "InvalidateSession must only be defined in the canonical auth_session module",
        ),
        (
            "enum AuthSessionError",
            "AuthSessionError must only be defined in the canonical auth_session module",
        ),
    ] {
        assert_pattern_only_in_files(pattern, &["features/auth_session.rs"], description);
    }
}

#[test]
fn auth_session_id_never_collides_with_existing_session_meanings() {
    // Reconciliation proof: the three pre-existing "session" concepts
    // must remain completely untouched by this frontier — no reference
    // to AuthSessionId/AuthSessionState anywhere near them.
    let frame_context =
        fs::read_to_string(workspace_root().join("spider/src/features/frame_context.rs")).unwrap();
    assert!(
        !frame_context.contains("AuthSessionId") && !frame_context.contains("AuthSessionState"),
        "frame_context.rs (chromiumoxide CDP SessionId identity chain) must remain \
         untouched by the authenticated-session lifecycle"
    );

    let mcp_state_path = workspace_root().join("spider_mcp/src/state.rs");
    if mcp_state_path.exists() {
        let mcp_state = fs::read_to_string(&mcp_state_path).unwrap();
        assert!(
            !mcp_state.contains("AuthSessionId") && !mcp_state.contains("AuthSessionState"),
            "spider_mcp::CrawlSession (async tool-call progress tracking) must remain \
             untouched by the authenticated-session lifecycle"
        );
    }

    // And the reconciliation must be documented, not merely true by
    // accident.
    let auth_session =
        fs::read_to_string(workspace_root().join("spider/src/features/auth_session.rs")).unwrap();
    for must_mention in [
        "chromiumoxide::cdp::browser_protocol::target::SessionId",
        "CrawlSession",
        "frame_context.rs",
    ] {
        assert!(
            auth_session.contains(must_mention),
            "auth_session.rs must document its reconciliation against {must_mention:?}"
        );
    }
}

#[test]
fn auth_session_lifecycle_uses_domain_state_transition_contract() {
    let auth_session =
        fs::read_to_string(workspace_root().join("spider/src/features/auth_session.rs")).unwrap();
    assert!(auth_session.contains("use crate::features::domain_state::Transition"));
    // Exactly the three transitions the lifecycle vocabulary justifies —
    // no bare "Resumed" state, no invented transitions.
    assert_eq!(
        auth_session
            .matches("impl Transition<AuthSessionState> for")
            .count(),
        3,
        "expected exactly 3 Transition<AuthSessionState> impls: pause, resume, invalidate"
    );
    assert!(auth_session.contains("impl Transition<AuthSessionState> for PauseSession"));
    assert!(auth_session.contains("impl Transition<AuthSessionState> for ResumeSession"));
    assert!(auth_session.contains("impl Transition<AuthSessionState> for InvalidateSession"));
}

#[test]
fn auth_session_persistence_uses_both_domain_persistence_primitives() {
    let auth_session =
        fs::read_to_string(workspace_root().join("spider/src/features/auth_session.rs")).unwrap();
    // Unlike Track 4's evidence ledger (append-only only), an
    // authenticated session has genuine current state — this module must
    // use both of Track 3's primitives: compare-and-swap for the current
    // lifecycle state, and append-only history for each superseded one.
    assert!(auth_session.contains(".write_current("));
    assert!(auth_session.contains(".append_history("));
    // No unconditional overwrite / no direct SQL / no second persistence
    // mechanism of its own.
    for forbidden in ["set_current(", "overwrite_current(", "sqlx::query"] {
        assert!(
            !auth_session.contains(forbidden),
            "auth_session.rs must not construct its own persistence mechanism: \
             found {forbidden:?}"
        );
    }
}

#[test]
fn auth_session_pause_resume_requires_matching_continuity_token() {
    let auth_session =
        fs::read_to_string(workspace_root().join("spider/src/features/auth_session.rs")).unwrap();
    // The truthfulness proof for "pause/resume preserves the same
    // authenticated browser session, never silently re-authenticates":
    // resume must compare the presented token against the one recorded
    // at pause time and reject on mismatch.
    assert!(auth_session.contains("if *continuity == self.continuity"));
    assert!(auth_session.contains("AuthSessionTransitionRejected::ContinuityMismatch"));
}

#[test]
fn auth_session_credential_types_never_appear_in_identity_or_lifecycle() {
    let identity =
        fs::read_to_string(workspace_root().join("spider/src/features/identity.rs")).unwrap();
    let auth_session =
        fs::read_to_string(workspace_root().join("spider/src/features/auth_session.rs")).unwrap();
    // AuthSessionId must share EvidenceId/WatchId's exact structural
    // shape — 16 opaque bytes, nothing else — which is what makes it
    // structurally incapable of holding a cookie/token/credential.
    assert!(identity.contains("pub struct AuthSessionId([u8; ID_BYTES]);"));
    // Real credential-carrying types already named elsewhere in this
    // codebase (features/secret_request_headers.rs, reqwest's header
    // vocabulary, the cookie jar) must never be imported or referenced
    // as an actual type here — checked as code forms (an import or a
    // field-type annotation), not bare prose words, since this module's
    // own doc comments legitimately *discuss* cookies/tokens/credentials
    // in English to explain why none of them can enter.
    for forbidden in [
        "use reqwest::header",
        ": HeaderValue",
        ": HeaderMap",
        ": SecretRequestHeaders",
        "use cookie::",
        "cookie::Jar",
    ] {
        assert!(
            !identity.contains(forbidden) && !auth_session.contains(forbidden),
            "found a real credential-carrying type reference: {forbidden:?} — \
             AuthSessionId/AuthSessionState must never be able to hold secret material"
        );
    }
}

#[test]
fn auth_session_never_implements_out_of_scope_capabilities() {
    let auth_session =
        fs::read_to_string(workspace_root().join("spider/src/features/auth_session.rs")).unwrap();
    for forbidden in [
        "struct WatchState",
        "enum WatchState",
        "struct WatchDefinition",
        "enum WatchDefinition",
        "struct ChangeResult",
        "enum ChangeResult",
        "struct ChangeEvent",
        "enum ChangeEvent",
        "struct Fingerprint",
        "struct Lineage",
        "fn schedule",
        "struct Scheduler",
        "EventSourc",
        "chromiumoxide::Browser",
        "chromiumoxide::Page",
    ] {
        assert!(
            !auth_session.contains(forbidden),
            "auth_session.rs must stay scoped to identity/lifecycle/persistence: \
             found forbidden pattern {forbidden:?}"
        );
    }
}

#[test]
fn no_shadow_auth_session_model_in_cli_or_mcp() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    for crate_dir in ["spider_cli/src", "spider_mcp/src"] {
        let dir = manifest_dir.parent().unwrap().join(crate_dir);
        if !dir.exists() {
            continue;
        }
        let mut files = Vec::new();
        collect_rust_files(&dir, &dir, &mut files);
        for file in files {
            for shadow in [
                "struct AuthSessionId",
                "enum AuthSessionState",
                "enum AuthenticationProfile",
                "struct BrowserContinuityToken",
                "struct PauseSession",
                "struct ResumeSession",
                "struct InvalidateSession",
                "enum AuthSessionError",
            ] {
                assert!(
                    !file.contents.contains(shadow),
                    "{crate_dir}/{} must not define its own {shadow:?} — the canonical \
                     authenticated-session lifecycle is owned exclusively by \
                     spider::features::identity / spider::features::auth_session",
                    file.relative_path
                );
            }
        }
    }
}

// --- SECTION: SCORPION_FINGERPRINT_AND_TRANSFORM_LINEAGE_001 ---
//
// Track 6 of the frozen roadmap: content/transform lineage
// (`TransformLineageId`, `TransformationIdentity`,
// `TransformLineageRecord`, in `features/transform_lineage.rs`) without
// redefining or shadowing the existing, unrelated
// `spider::configuration::Fingerprint` (browser anti-detection stealth
// profile, re-exported from the `spider_fingerprint` crate).
// `TransformLineageId` is content-addressed — deterministic from
// (input hash, transformation identity, output hash) — a materially
// different construction than `features/identity.rs`'s three
// randomly-minted identity types, which is why it lives in its own
// module rather than that one.

#[test]
fn configuration_fingerprint_ownership_remains_intact() {
    let configuration =
        fs::read_to_string(workspace_root().join("spider/src/configuration.rs")).unwrap();
    assert!(
        configuration.contains("pub use spider_fingerprint::Fingerprint;"),
        "configuration::Fingerprint's existing re-export must remain exactly as it was — \
         this frontier must not touch it"
    );
}

#[test]
fn no_bare_fingerprint_type_is_defined_anywhere_in_spider() {
    // `Fingerprint` is only ever a re-export from `spider_fingerprint` —
    // it must never be *defined* (shadowed/redefined) anywhere in this
    // crate, including by this frontier's own new module.
    for pattern in ["struct Fingerprint", "enum Fingerprint"] {
        assert_pattern_only_in_files(
            pattern,
            &[],
            "Fingerprint must never be defined in spider/src — it is owned entirely \
             by the spider_fingerprint crate and only re-exported from configuration.rs",
        );
    }
}

#[test]
fn transform_lineage_never_imports_or_references_configuration_fingerprint() {
    let transform_lineage =
        fs::read_to_string(workspace_root().join("spider/src/features/transform_lineage.rs"))
            .unwrap();
    // Checked as actual code forms (imports), not prose — the module's
    // own doc comment legitimately *names* `configuration::Fingerprint`
    // in English as part of the required reconciliation.
    for forbidden in [
        "use crate::configuration::Fingerprint",
        "use spider_fingerprint",
    ] {
        assert!(
            !transform_lineage.contains(forbidden),
            "transform_lineage.rs must never import configuration::Fingerprint \
             (or the spider_fingerprint crate) — found {forbidden:?}"
        );
    }
}

#[test]
fn transform_lineage_types_are_defined_exactly_once() {
    for (pattern, description) in [
        (
            "struct TransformLineageId",
            "TransformLineageId must only be defined in the canonical transform_lineage module",
        ),
        (
            "struct TransformationIdentity",
            "TransformationIdentity must only be defined in the canonical transform_lineage module",
        ),
        (
            "struct TransformLineageRecord",
            "TransformLineageRecord must only be defined in the canonical transform_lineage module",
        ),
        (
            "enum TransformLineageError",
            "TransformLineageError must only be defined in the canonical transform_lineage module",
        ),
    ] {
        assert_pattern_only_in_files(pattern, &["features/transform_lineage.rs"], description);
    }
}

#[test]
fn transform_lineage_module_gated_behind_evidence_and_disk() {
    let features_mod =
        fs::read_to_string(workspace_root().join("spider/src/features/mod.rs")).unwrap();
    let decl_index = features_mod
        .find("pub mod transform_lineage;")
        .expect("transform_lineage module not declared in features/mod.rs");
    let preceding = &features_mod[..decl_index];
    let gate_line = preceding
        .lines()
        .rev()
        .find(|line| !line.trim_start().starts_with("///"))
        .expect("expected a line before the module declaration");
    assert_eq!(
        gate_line.trim(),
        "#[cfg(all(feature = \"evidence\", feature = \"disk\"))]",
        "transform_lineage must be gated behind evidence (sha256_hex) and disk \
         (DomainPersistence/EvidenceRef) — it must not introduce a third, independent \
         storage or hashing stack"
    );
}

#[test]
fn transform_lineage_identity_is_content_addressed_not_randomly_minted() {
    let transform_lineage =
        fs::read_to_string(workspace_root().join("spider/src/features/transform_lineage.rs"))
            .unwrap();
    // The determinism proof: the id is a pure function of
    // (input_hash, transformation, output_hash) — never of recorded_at
    // or any random source. No `random_bytes`/`AtomicU64` counter
    // (identity.rs's random-minting machinery) is reachable here.
    assert!(transform_lineage.contains("fn derive("));
    assert!(transform_lineage
        .contains("format!(\"lineage-v1|{input_hash}|{transformation}|{output_hash}\")"));
    for forbidden in ["random_bytes", "AtomicU64", "fastrand", "rand::"] {
        assert!(
            !transform_lineage.contains(forbidden),
            "transform_lineage.rs's identity must be purely content-addressed, \
             never randomly minted: found {forbidden:?}"
        );
    }
}

#[test]
fn transform_lineage_persists_append_only_and_reuses_domain_persistence() {
    let transform_lineage =
        fs::read_to_string(workspace_root().join("spider/src/features/transform_lineage.rs"))
            .unwrap();
    assert!(transform_lineage.contains(".append_history("));
    assert!(!transform_lineage.contains(".write_current("));
    // A duplicate content-addressed key is treated as success (the
    // identical fact was already recorded), not surfaced as an error —
    // and no direct SQL / second persistence mechanism exists here.
    assert!(transform_lineage.contains("Err(PersistenceError::HistoryAlreadyExists) => Ok(id)"));
    for forbidden in ["sqlx::query", "struct DomainPersistence"] {
        assert!(
            !transform_lineage.contains(forbidden),
            "transform_lineage.rs must not construct its own persistence mechanism: \
             found {forbidden:?}"
        );
    }
}

#[test]
fn transform_lineage_reuses_evidence_ref_and_sha256_hex_without_duplication() {
    let transform_lineage =
        fs::read_to_string(workspace_root().join("spider/src/features/transform_lineage.rs"))
            .unwrap();
    assert!(transform_lineage.contains("use crate::utils::evidence::{sha256_hex, EvidenceRef}"));
    // EvidenceRef is stored by value (Copy, 16-byte reference) — never
    // alongside a duplicated evidence payload/content field.
    assert!(transform_lineage.contains("input_evidence: Option<EvidenceRef>"));
    for forbidden in [
        "struct EvidenceBundle",
        "struct EvidenceRef",
        "fn sha256_hex",
    ] {
        assert!(
            !transform_lineage.contains(forbidden),
            "transform_lineage.rs must reuse EvidenceRef/sha256_hex, never redefine \
             them: found {forbidden:?}"
        );
    }
}

#[test]
fn transform_lineage_never_implements_out_of_scope_capabilities() {
    let transform_lineage =
        fs::read_to_string(workspace_root().join("spider/src/features/transform_lineage.rs"))
            .unwrap();
    for forbidden in [
        "struct WatchState",
        "enum WatchState",
        "struct WatchDefinition",
        "enum WatchDefinition",
        "struct ChangeResult",
        "enum ChangeResult",
        "struct ChangeEvent",
        "enum ChangeEvent",
        "fn schedule",
        "struct Scheduler",
        "EventSourc",
        "struct AuthSessionId",
        "struct EvidenceBundle",
    ] {
        assert!(
            !transform_lineage.contains(forbidden),
            "transform_lineage.rs must stay scoped to lineage identity/persistence: \
             found forbidden pattern {forbidden:?}"
        );
    }
}

#[test]
fn no_shadow_transform_lineage_model_in_cli_or_mcp() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    for crate_dir in ["spider_cli/src", "spider_mcp/src"] {
        let dir = manifest_dir.parent().unwrap().join(crate_dir);
        if !dir.exists() {
            continue;
        }
        let mut files = Vec::new();
        collect_rust_files(&dir, &dir, &mut files);
        for file in files {
            for shadow in [
                "struct TransformLineageId",
                "struct TransformationIdentity",
                "struct TransformLineageRecord",
                "enum TransformLineageError",
            ] {
                assert!(
                    !file.contents.contains(shadow),
                    "{crate_dir}/{} must not define its own {shadow:?} — the canonical \
                     transform lineage model is owned exclusively by \
                     spider::features::transform_lineage",
                    file.relative_path
                );
            }
        }
    }
}

// --- SECTION: SCORPION_CANONICAL_WATCH_MODEL_001 ---
//
// Track 7 of the frozen roadmap: the canonical Watch model
// (`WatchDefinition`/`WatchState`, in `features/watch.rs`), reusing the
// existing `WatchId` (identity.rs) and the existing `DiscoveryTarget`
// (discovery_target.rs) rather than inventing `WatchTarget`/`WatchSpec`.
// `WatchState` is built on Track 2's unmodified `CurrentState`/
// `Transition` contract and persisted through Track 3's
// `DomainPersistence` (compare-and-swap current state, append-only
// history) — no scheduler, no `ChangeResult`/`ChangeEvent`, no health,
// no notifications, no generic `Job` model.

#[test]
fn watch_definition_and_watch_state_are_defined_exactly_once() {
    for (pattern, description) in [
        (
            "struct WatchDefinition",
            "WatchDefinition must only be defined in the canonical watch module",
        ),
        (
            "enum WatchState",
            "WatchState must only be defined in the canonical watch module",
        ),
        (
            "enum WatchTransitionRejected",
            "WatchTransitionRejected must only be defined in the canonical watch module",
        ),
        (
            "enum WatchError",
            "WatchError must only be defined in the canonical watch module",
        ),
        (
            "struct ObserveEvidence",
            "ObserveEvidence must only be defined in the canonical watch module",
        ),
        (
            "struct StopWatch",
            "StopWatch must only be defined in the canonical watch module",
        ),
    ] {
        assert_pattern_only_in_files(pattern, &["features/watch.rs"], description);
    }
}

#[test]
fn watch_id_is_reused_not_duplicated() {
    // WatchId must remain defined exactly once, in identity.rs — this
    // frontier reuses it, it does not redefine or shadow it.
    assert_pattern_only_in_files(
        "struct WatchId",
        &["features/identity.rs"],
        "WatchId must only be defined in the canonical identity module — Track 7 reuses it, \
         it does not redefine it",
    );
    let watch = fs::read_to_string(workspace_root().join("spider/src/features/watch.rs")).unwrap();
    assert!(
        watch.contains("use crate::features::identity::WatchId"),
        "watch.rs must import the existing WatchId rather than defining its own"
    );
}

#[test]
fn watch_definition_and_watch_state_remain_separate() {
    let watch = fs::read_to_string(workspace_root().join("spider/src/features/watch.rs")).unwrap();
    // WatchDefinition owns the target; WatchState owns lifecycle only —
    // it must never carry a target/DiscoveryTarget field of its own,
    // which would blur the definition/state separation this frontier
    // requires.
    assert!(watch.contains("pub struct WatchDefinition {\n    /// The pointer this watch observes.\n    pub target: DiscoveryTarget,\n}"));
    let state_block_start = watch
        .find("pub enum WatchState {")
        .expect("WatchState must be defined");
    let state_block_end = watch[state_block_start..]
        .find("\nimpl WatchState")
        .map(|offset| state_block_start + offset)
        .expect("expected an impl WatchState block after the enum");
    let state_block = &watch[state_block_start..state_block_end];
    assert!(
        !state_block.contains("DiscoveryTarget") && !state_block.contains("target:"),
        "WatchState must not carry a target/DiscoveryTarget field — that belongs to \
         WatchDefinition alone"
    );
}

#[test]
fn watch_reuses_discovery_target_not_a_new_watch_target_type() {
    for forbidden in [
        "struct WatchTarget",
        "enum WatchTarget",
        "struct WatchSpec",
        "enum WatchSpec",
    ] {
        assert_pattern_only_in_files(
            forbidden,
            &[],
            "Track 7 must reuse the existing DiscoveryTarget — no WatchTarget/WatchSpec may \
             be introduced anywhere in spider/src",
        );
    }
    let watch = fs::read_to_string(workspace_root().join("spider/src/features/watch.rs")).unwrap();
    assert!(watch.contains("use crate::features::discovery_target::DiscoveryTarget"));
}

#[test]
fn watch_state_uses_domain_state_transition_contract() {
    let watch = fs::read_to_string(workspace_root().join("spider/src/features/watch.rs")).unwrap();
    assert!(
        watch.contains("use crate::features::domain_state::{Applied, CurrentState, Transition};")
    );
    // Exactly the two transitions the lifecycle vocabulary justifies —
    // no bare "Paused"/"Resumed" state, no invented transitions.
    assert_eq!(
        watch.matches("impl Transition<WatchState> for").count(),
        2,
        "expected exactly 2 Transition<WatchState> impls: ObserveEvidence, StopWatch"
    );
    assert!(watch.contains("impl Transition<WatchState> for ObserveEvidence"));
    assert!(watch.contains("impl Transition<WatchState> for StopWatch"));
}

#[test]
fn watch_persists_via_both_domain_persistence_primitives_with_cas() {
    let watch = fs::read_to_string(workspace_root().join("spider/src/features/watch.rs")).unwrap();
    assert!(watch.contains(
        "use crate::features::domain_persistence::{DomainPersistence, PersistenceError};"
    ));
    assert!(watch.contains(".write_current("));
    assert!(watch.contains(".append_history("));
    // The current-state write is compare-and-swap, not a blind
    // overwrite: it passes the just-read revision as the expected
    // revision, and a conflict surfaces as a genuine error rather than
    // being silently dropped.
    assert!(watch.contains(".write_current(&id.to_string(), Some(revision), &new_payload)"));
    assert!(watch.contains(
        "PersistenceError::CurrentStateConflict { .. } => WatchError::ConcurrentModification"
    ));
    for forbidden in [
        "set_current(",
        "overwrite_current(",
        "sqlx::query",
        "struct DomainPersistence",
    ] {
        assert!(
            !watch.contains(forbidden),
            "watch.rs must not construct its own persistence mechanism: found {forbidden:?}"
        );
    }
}

#[test]
fn watch_evidence_ref_is_reused_by_reference_not_duplicated() {
    let watch = fs::read_to_string(workspace_root().join("spider/src/features/watch.rs")).unwrap();
    assert!(watch.contains("use crate::utils::evidence::EvidenceRef"));
    // Stored as a plain Option<EvidenceRef> pointer — never a field
    // holding evidence content or a redefinition of the evidence types
    // themselves.
    assert!(watch.contains("last_evidence: Option<EvidenceRef>"));
    for forbidden in [
        "struct EvidenceBundle",
        "struct EvidenceRef",
        "struct EvidenceId",
    ] {
        assert!(
            !watch.contains(forbidden),
            "watch.rs must reuse EvidenceRef by reference, never redefine the evidence \
             types: found {forbidden:?}"
        );
    }
}

#[test]
fn watch_never_implements_out_of_scope_capabilities() {
    let watch = fs::read_to_string(workspace_root().join("spider/src/features/watch.rs")).unwrap();
    for forbidden in [
        "struct WatchTarget",
        "struct WatchSpec",
        "struct ChangeResult",
        "enum ChangeResult",
        "struct ChangeEvent",
        "enum ChangeEvent",
        "fn schedule",
        "struct Scheduler",
        "EventSourc",
        "struct Job",
        "enum Job",
        "struct Health",
        "enum Health",
        "struct Notification",
        "enum Notification",
        "struct AuthSessionId",
        "struct EvidenceBundle",
        "cron_str",
    ] {
        assert!(
            !watch.contains(forbidden),
            "watch.rs must stay scoped to WatchDefinition/WatchState/identity/persistence: \
             found forbidden pattern {forbidden:?}"
        );
    }
}

#[test]
fn no_shadow_watch_model_in_cli_or_mcp() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    for crate_dir in ["spider_cli/src", "spider_mcp/src"] {
        let dir = manifest_dir.parent().unwrap().join(crate_dir);
        if !dir.exists() {
            continue;
        }
        let mut files = Vec::new();
        collect_rust_files(&dir, &dir, &mut files);
        for file in files {
            for shadow in [
                "struct WatchDefinition",
                "enum WatchState",
                "struct WatchTarget",
                "struct WatchSpec",
                "enum WatchTransitionRejected",
                "struct ObserveEvidence",
                "struct StopWatch",
            ] {
                assert!(
                    !file.contents.contains(shadow),
                    "{crate_dir}/{} must not define its own {shadow:?} — the canonical \
                     Watch model is owned exclusively by spider::features::watch",
                    file.relative_path
                );
            }
        }
    }
}

// --- SECTION: SCORPION_SCHEDULING_AND_WATCH_EXECUTION_001 ---
//
// Track 8 of the frozen roadmap: canonical scheduling semantics for
// WatchDefinition (`WatchSchedule`, in `features/watch_schedule.rs`) and
// the execution path for one scheduled watch run
// (`execute_scheduled_watch_run`). Cadence syntax reuses the existing
// `async_job::Schedule` primitive `Website`'s own cron feature already
// depends on — never `website::CronType` (a what-to-run selector, not
// cadence syntax) and never `async_job::Job`/`async_job::Runner` (a
// separate, `Website`-owned scheduler daemon abstraction). Execution
// reuses `acquisition_binding` for acquisition and `utils::evidence` for
// the durable `EvidenceRef`; `WatchState` remains owned exclusively by
// Track 7 — this module only ever calls into
// `features::watch::apply_watch_transition`, never redefining or
// mutating `WatchState` itself.

#[test]
fn watch_schedule_and_execution_types_are_defined_exactly_once() {
    for (pattern, description) in [
        (
            "struct WatchSchedule",
            "WatchSchedule must only be defined in the canonical watch_schedule module",
        ),
        (
            "enum WatchScheduleError",
            "WatchScheduleError must only be defined in the canonical watch_schedule module",
        ),
        (
            "enum WatchExecutionError",
            "WatchExecutionError must only be defined in the canonical watch_schedule module",
        ),
        (
            "enum ScheduledRunRecord",
            "ScheduledRunRecord must only be defined in the canonical watch_schedule module",
        ),
    ] {
        assert_pattern_only_in_files(pattern, &["features/watch_schedule.rs"], description);
    }
}

#[test]
fn watch_schedule_module_gated_behind_evidence_disk_and_cron() {
    let features_mod =
        fs::read_to_string(workspace_root().join("spider/src/features/mod.rs")).unwrap();
    let decl_index = features_mod
        .find("pub mod watch_schedule;")
        .expect("watch_schedule module not declared in features/mod.rs");
    let preceding = &features_mod[..decl_index];
    let gate_line = preceding
        .lines()
        .rev()
        .find(|line| !line.trim_start().starts_with("///"))
        .expect("expected a line before the module declaration");
    assert_eq!(
        gate_line.trim(),
        "#[cfg(all(feature = \"evidence\", feature = \"disk\", feature = \"cron\"))]",
        "watch_schedule must be gated behind evidence+disk (like watch) plus cron (for the \
         cadence primitive) — it must not introduce a second cadence-parsing stack"
    );
}

#[test]
fn watch_schedule_reuses_async_job_schedule_not_website_crontype() {
    let watch_schedule =
        fs::read_to_string(workspace_root().join("spider/src/features/watch_schedule.rs")).unwrap();
    assert!(watch_schedule.contains("cron_str.parse::<async_job::Schedule>()"));
    // Never the Website-owned scheduler daemon abstraction (Job/Runner),
    // and never website::CronType (what-to-run, not cadence syntax).
    // Checked as actual code forms, not prose — this module's own doc
    // comment legitimately *names* both in English to explain why
    // neither is reused.
    for forbidden in [
        "impl Job for",
        "async_job::Runner::new",
        "Runner::new(",
        "async_job::async_trait",
        "use crate::website::CronType",
        "CronType::Crawl",
        "CronType::Scrape",
        ": CronType",
    ] {
        assert!(
            !watch_schedule.contains(forbidden),
            "watch_schedule.rs must adapt async_job::Schedule's parser only — never adopt \
             the async_job::Job/Runner scheduler daemon, and never reuse website::CronType \
             (a what-to-run selector, not cadence syntax): found {forbidden:?}"
        );
    }
}

#[test]
fn watch_execution_reuses_watch_definition_and_never_redefines_watch_state() {
    let watch_schedule =
        fs::read_to_string(workspace_root().join("spider/src/features/watch_schedule.rs")).unwrap();
    assert!(
        watch_schedule.contains("use crate::features::watch::{self, ObserveEvidence, WatchError};")
    );
    assert!(watch_schedule.contains("watch::read_watch_definition("));
    assert!(watch_schedule.contains("watch::apply_watch_transition("));
    for forbidden in [
        "struct WatchState",
        "enum WatchState",
        "struct WatchDefinition",
    ] {
        assert!(
            !watch_schedule.contains(forbidden),
            "watch_schedule.rs must reuse WatchDefinition/WatchState by reference, never \
             redefine them — WatchState remains owned exclusively by Track 7: found \
             {forbidden:?}"
        );
    }
}

#[test]
fn watch_execution_reuses_canonical_acquisition_binding() {
    let watch_schedule =
        fs::read_to_string(workspace_root().join("spider/src/features/watch_schedule.rs")).unwrap();
    assert!(watch_schedule
        .contains("use crate::features::acquisition_binding::{self, AcquisitionBindingError};"));
    assert!(watch_schedule.contains("acquisition_binding::bind("));
    assert!(watch_schedule.contains("acquisition_binding::execute("));
    for forbidden in [
        "reqwest::Client::new",
        "reqwest::Client::builder",
        "Website::new",
        ".crawl()",
        ".scrape()",
        "TorTransportConfig::new",
    ] {
        assert!(
            !watch_schedule.contains(forbidden),
            "watch_schedule.rs must reuse the existing acquisition_binding seam — no second \
             fetch/crawl/transport architecture: found {forbidden:?}"
        );
    }
}

#[test]
fn watch_execution_produces_durable_evidence_via_canonical_ledger() {
    let watch_schedule =
        fs::read_to_string(workspace_root().join("spider/src/features/watch_schedule.rs")).unwrap();
    assert!(watch_schedule.contains(
        "use crate::utils::evidence::{build_evidence, record_evidence, EvidenceLedgerError, EvidenceRef};"
    ));
    assert!(watch_schedule.contains("build_evidence(page, content, false, false)"));
    assert!(watch_schedule.contains("record_evidence(store, bundle)"));
    for forbidden in [
        "struct EvidenceBundle",
        "struct EvidenceRef",
        "fn build_evidence",
        "fn record_evidence",
    ] {
        assert!(
            !watch_schedule.contains(forbidden),
            "watch_schedule.rs must reuse the existing evidence ledger, never redefine it: \
             found {forbidden:?}"
        );
    }
}

#[test]
fn watch_execution_claims_run_identity_before_any_side_effect() {
    let watch_schedule =
        fs::read_to_string(workspace_root().join("spider/src/features/watch_schedule.rs")).unwrap();
    let claim_index = watch_schedule
        .find(".write_current(&run_key, None, &claim_payload)")
        .expect("expected an initial CAS claim of the run identity");
    let acquisition_index = watch_schedule
        .find("acquisition_binding::bind(")
        .expect("expected a canonical acquisition call");
    let finalize_index = watch_schedule
        .find(".write_current(&run_key, Some(claim_revision), &completed_payload)")
        .expect("expected a CAS finalize of the run identity");
    assert!(
        claim_index < acquisition_index,
        "the run identity must be claimed (compare-and-swap) before acquisition begins — \
         otherwise a concurrent retry could duplicate the fetch/evidence/transition"
    );
    assert!(
        acquisition_index < finalize_index,
        "the run identity must only be finalized after acquisition/evidence/transition \
         complete"
    );
    assert!(watch_schedule.contains("PersistenceError::CurrentStateConflict { .. }) =>"));
    assert!(watch_schedule.contains("WatchExecutionError::RunAlreadyInProgress"));
}

#[test]
fn watch_schedule_never_implements_out_of_scope_capabilities() {
    let watch_schedule =
        fs::read_to_string(workspace_root().join("spider/src/features/watch_schedule.rs")).unwrap();
    for forbidden in [
        "struct ChangeResult",
        "enum ChangeResult",
        "struct ChangeEvent",
        "enum ChangeEvent",
        "struct Health",
        "enum Health",
        "struct Notification",
        "enum Notification",
        "struct Job",
        "enum Job",
        "struct Operation",
        "enum Operation",
        "struct Scheduler",
        "EventSourc",
        "struct WatchTarget",
        "struct WatchSpec",
    ] {
        assert!(
            !watch_schedule.contains(forbidden),
            "watch_schedule.rs must stay scoped to scheduling/execution: found forbidden \
             pattern {forbidden:?}"
        );
    }
}

#[test]
fn no_shadow_watch_schedule_model_in_cli_or_mcp() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    for crate_dir in ["spider_cli/src", "spider_mcp/src"] {
        let dir = manifest_dir.parent().unwrap().join(crate_dir);
        if !dir.exists() {
            continue;
        }
        let mut files = Vec::new();
        collect_rust_files(&dir, &dir, &mut files);
        for file in files {
            for shadow in [
                "struct WatchSchedule",
                "enum WatchScheduleError",
                "enum WatchExecutionError",
                "enum ScheduledRunRecord",
                "fn execute_scheduled_watch_run",
            ] {
                assert!(
                    !file.contents.contains(shadow),
                    "{crate_dir}/{} must not define its own {shadow:?} — the canonical \
                     scheduling/execution model is owned exclusively by \
                     spider::features::watch_schedule",
                    file.relative_path
                );
            }
        }
    }
}

// --- SECTION: SCORPION_CANONICAL_CHANGE_DETECTION_001 ---
//
// Track 9 of the frozen roadmap: canonical change detection
// (`ChangeResult`/`ChangeEvent`, in `features/change_detection.rs`).
// Compares only evidence a watch's own history already associates with
// it; never reduces an uncomparable pair to "unchanged"; reuses
// `EvidenceBundle`'s existing sha256_hex-derived hash fields and Track
// 6's content-addressed idempotent-append persistence pattern rather
// than inventing a second fingerprint/hashing architecture; persists
// through Track 3's append-only history only. Track 8
// (`features/watch_schedule.rs`) remains the sole scheduler/execution
// owner — this module never defines scheduling of its own.

#[test]
fn change_result_and_change_event_types_are_defined_exactly_once() {
    for (pattern, description) in [
        (
            "enum ChangeResult",
            "ChangeResult must only be defined in the canonical change_detection module",
        ),
        (
            "struct ChangeEvent",
            "ChangeEvent must only be defined in the canonical change_detection module",
        ),
        (
            "struct ChangeEventId",
            "ChangeEventId must only be defined in the canonical change_detection module",
        ),
        (
            "enum ChangeDetectionError",
            "ChangeDetectionError must only be defined in the canonical change_detection module",
        ),
        (
            "enum ComparisonBasis",
            "ComparisonBasis must only be defined in the canonical change_detection module",
        ),
        (
            "enum UncomparableReason",
            "UncomparableReason must only be defined in the canonical change_detection module",
        ),
    ] {
        assert_pattern_only_in_files(pattern, &["features/change_detection.rs"], description);
    }
}

#[test]
fn change_detection_module_gated_behind_evidence_and_disk() {
    let features_mod =
        fs::read_to_string(workspace_root().join("spider/src/features/mod.rs")).unwrap();
    let decl_index = features_mod
        .find("pub mod change_detection;")
        .expect("change_detection module not declared in features/mod.rs");
    let preceding = &features_mod[..decl_index];
    let gate_line = preceding
        .lines()
        .rev()
        .find(|line| !line.trim_start().starts_with("///"))
        .expect("expected a line before the module declaration");
    assert_eq!(
        gate_line.trim(),
        "#[cfg(all(feature = \"evidence\", feature = \"disk\"))]",
        "change_detection must be gated behind evidence+disk, like watch.rs (which it reads) \
         — it must not introduce a third persistence/hashing stack"
    );
}

#[test]
fn change_detection_only_compares_same_watch_evidence() {
    let change_detection =
        fs::read_to_string(workspace_root().join("spider/src/features/change_detection.rs"))
            .unwrap();
    assert!(change_detection.contains("fn watch_evidence_refs("));
    assert!(change_detection.contains("fn ensure_evidence_belongs_to_watch("));
    assert!(change_detection.contains("ChangeDetectionError::EvidenceNotAssociatedWithWatch"));
    // Both refs are validated against the watch's own history before any
    // comparison is attempted.
    let ensure_calls = change_detection
        .matches("ensure_evidence_belongs_to_watch(store, watch,")
        .count();
    assert_eq!(
        ensure_calls, 2,
        "both previous_evidence and current_evidence must be validated against watch's own \
         history before any comparison is attempted"
    );
}

#[test]
fn change_result_never_reduces_uncomparable_evidence_to_unchanged() {
    let change_detection =
        fs::read_to_string(workspace_root().join("spider/src/features/change_detection.rs"))
            .unwrap();
    assert!(change_detection.contains("ChangeResult::Uncomparable"));
    assert!(change_detection.contains("ChangeDetectionError::PreviousEvidenceUnresolvable"));
    assert!(change_detection.contains("ChangeDetectionError::CurrentEvidenceUnresolvable"));
    // The uncomparable branch is reached whenever the two bases differ or
    // either side has no usable hash — never silently defaulted to
    // Unchanged.
    assert!(change_detection.contains("if previous_basis == current_basis"));
}

#[test]
fn change_detection_reuses_evidence_bundle_hashes_not_a_new_fingerprint() {
    let change_detection =
        fs::read_to_string(workspace_root().join("spider/src/features/change_detection.rs"))
            .unwrap();
    assert!(change_detection.contains(".response_body_hash"));
    assert!(change_detection.contains(".transformed_content_hash"));
    assert!(change_detection
        .contains("use crate::utils::evidence::{sha256_hex, EvidenceBundle, EvidenceLedgerError, EvidenceRef};"));
    for forbidden in [
        "struct EvidenceBundle",
        "fn sha256_hex",
        "struct Fingerprint",
        "spider_fingerprint",
        "use crate::configuration::Fingerprint",
    ] {
        assert!(
            !change_detection.contains(forbidden),
            "change_detection.rs must reuse EvidenceBundle's existing hash fields and \
             sha256_hex, never redefine them or introduce a second fingerprint \
             architecture: found {forbidden:?}"
        );
    }
}

#[test]
fn change_event_persists_append_only_and_is_content_addressed_idempotent() {
    let change_detection =
        fs::read_to_string(workspace_root().join("spider/src/features/change_detection.rs"))
            .unwrap();
    assert!(change_detection.contains(".append_history("));
    assert!(!change_detection.contains(".write_current("));
    // Content-addressed from (watch, previous_evidence, current_evidence)
    // only — never from the computed result or a timestamp — so a
    // duplicate recording of the identical fact is idempotent, not a
    // conflict, mirroring Track 6's own precedent verbatim.
    assert!(change_detection.contains("change-v1|{watch}|"));
    assert!(change_detection.contains("Err(PersistenceError::HistoryAlreadyExists) => Ok(event)"));
    for forbidden in ["sqlx::query", "struct DomainPersistence"] {
        assert!(
            !change_detection.contains(forbidden),
            "change_detection.rs must not construct its own persistence mechanism: found \
             {forbidden:?}"
        );
    }
}

#[test]
fn change_result_computation_is_separate_from_change_event_persistence() {
    let change_detection =
        fs::read_to_string(workspace_root().join("spider/src/features/change_detection.rs"))
            .unwrap();
    // compute_change_result is a plain (non-async) function — it cannot
    // perform any DomainPersistence I/O.
    assert!(change_detection.contains(
        "pub fn compute_change_result(previous: &EvidenceBundle, current: &EvidenceBundle) -> ChangeResult {"
    ));
    assert!(change_detection
        .contains("pub async fn detect_and_record_change(\n    store: &DomainPersistence,"));
}

#[test]
fn track_8_remains_sole_scheduler_and_is_only_read_by_change_detection() {
    let change_detection =
        fs::read_to_string(workspace_root().join("spider/src/features/change_detection.rs"))
            .unwrap();
    for forbidden in [
        "struct WatchSchedule",
        "enum WatchSchedule",
        "fn execute_scheduled_watch_run",
        "fn define_watch_schedule",
        "async_job::Schedule",
        "struct WatchState",
        "enum WatchState",
        "struct WatchDefinition",
        "enum WatchDefinition",
    ] {
        assert!(
            !change_detection.contains(forbidden),
            "change_detection.rs must never define scheduling or redefine Watch types — \
             Track 8 remains the sole scheduler/execution owner and Track 7 remains the \
             sole Watch owner: found {forbidden:?}"
        );
    }
}

#[test]
fn change_detection_never_implements_out_of_scope_capabilities() {
    let change_detection =
        fs::read_to_string(workspace_root().join("spider/src/features/change_detection.rs"))
            .unwrap();
    for forbidden in [
        "struct Health",
        "enum Health",
        "struct Notification",
        "enum Notification",
        "struct Job",
        "enum Job",
        "struct Operation",
        "enum Operation",
        "struct Scheduler",
        "EventSourc",
        "struct GenericEvent",
        "trait Event",
    ] {
        assert!(
            !change_detection.contains(forbidden),
            "change_detection.rs must stay scoped to ChangeResult/ChangeEvent computation \
             and persistence: found forbidden pattern {forbidden:?}"
        );
    }
}

#[test]
fn no_shadow_change_detection_model_in_cli_or_mcp() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    for crate_dir in ["spider_cli/src", "spider_mcp/src"] {
        let dir = manifest_dir.parent().unwrap().join(crate_dir);
        if !dir.exists() {
            continue;
        }
        let mut files = Vec::new();
        collect_rust_files(&dir, &dir, &mut files);
        for file in files {
            for shadow in [
                "enum ChangeResult",
                "struct ChangeEvent",
                "struct ChangeEventId",
                "enum ChangeDetectionError",
                "fn detect_and_record_change",
            ] {
                assert!(
                    !file.contents.contains(shadow),
                    "{crate_dir}/{} must not define its own {shadow:?} — the canonical \
                     change detection model is owned exclusively by \
                     spider::features::change_detection",
                    file.relative_path
                );
            }
        }
    }
}

// --- SECTION: SCORPION_HEALTH_AND_OPERATIONAL_RECONCILIATION_001 ---
//
// Track 10 of the frozen roadmap: canonical, purely observational health
// for the complete watch pipeline (`HealthStatus`/
// `ChangeDetectionReadiness`, in `features/watch_health.rs`). Reads only
// — never calls apply_watch_transition/execute_scheduled_watch_run/
// define_watch_schedule/detect_and_record_change. Keeps type-level
// change-detection readiness structurally distinct from real production
// exercise. No second provider-health architecture; `ProviderDescriptor`
// remains untouched.

#[test]
fn health_types_are_defined_exactly_once() {
    for (pattern, description) in [
        (
            "enum HealthStatus",
            "HealthStatus must only be defined in the canonical watch_health module",
        ),
        (
            "enum ChangeDetectionReadiness",
            "ChangeDetectionReadiness must only be defined in the canonical watch_health module",
        ),
        (
            "struct WatchHealthReport",
            "WatchHealthReport must only be defined in the canonical watch_health module",
        ),
        (
            "enum WatchHealthError",
            "WatchHealthError must only be defined in the canonical watch_health module",
        ),
    ] {
        assert_pattern_only_in_files(pattern, &["features/watch_health.rs"], description);
    }
}

#[test]
fn watch_health_module_gated_behind_evidence_disk_and_cron() {
    let features_mod =
        fs::read_to_string(workspace_root().join("spider/src/features/mod.rs")).unwrap();
    let decl_index = features_mod
        .find("pub mod watch_health;")
        .expect("watch_health module not declared in features/mod.rs");
    let preceding = &features_mod[..decl_index];
    let gate_line = preceding
        .lines()
        .rev()
        .find(|line| !line.trim_start().starts_with("///"))
        .expect("expected a line before the module declaration");
    assert_eq!(
        gate_line.trim(),
        "#[cfg(all(feature = \"evidence\", feature = \"disk\", feature = \"cron\"))]",
        "watch_health must be gated exactly like watch_schedule (which it reads)"
    );
}

#[test]
fn watch_health_is_observational_only_never_a_write_owner() {
    let full_source =
        fs::read_to_string(workspace_root().join("spider/src/features/watch_health.rs")).unwrap();
    // Scan production code only — `#[cfg(test)] mod tests` legitimately
    // calls these same functions to set up fixtures (e.g. defining a
    // watch and driving a real scheduled run to exercise health against
    // real data), which is not this module itself owning them.
    let watch_health = full_source
        .split_once("#[cfg(test)]")
        .map(|(production, _)| production)
        .expect("expected a #[cfg(test)] module marker");
    // Every canonical write/mutation entry point across Tracks 7-9 must
    // never be called from this module — checked as real call forms
    // (with the opening paren), not bare prose, since this module's own
    // doc comment legitimately *names* every one of them in English to
    // explain why they are absent.
    for forbidden_call in [
        "apply_watch_transition(",
        "execute_scheduled_watch_run(",
        "define_watch_schedule(",
        "define_watch(",
        "detect_and_record_change(",
        "record_evidence(",
        "append_history(",
        "write_current(",
    ] {
        assert!(
            !watch_health.contains(forbidden_call),
            "watch_health.rs must be purely observational — it must never call a canonical \
             write/mutation entry point: found {forbidden_call:?}"
        );
    }
    // And it does call the canonical read entry points it depends on.
    for required_call in [
        "watch::read_watch_definition(",
        "watch::read_current_watch_state(",
        "watch_schedule::read_watch_schedule(",
        "change_detection::read_change_event(",
        ".resolve(store)",
        ".read_history(",
    ] {
        assert!(
            watch_health.contains(required_call),
            "watch_health.rs must derive health from the canonical read seams: missing \
             {required_call:?}"
        );
    }
}

#[test]
fn change_detection_readiness_cannot_conflate_type_level_and_production_exercise() {
    let watch_health =
        fs::read_to_string(workspace_root().join("spider/src/features/watch_health.rs")).unwrap();
    // The two states are separate enum variants, not two values of one
    // bare HealthStatus field — structurally impossible to conflate.
    assert!(watch_health.contains("enum ChangeDetectionReadiness"));
    assert!(watch_health.contains("TypeLevelReady,"));
    assert!(watch_health.contains("ProductionExercised {"));
    // ProductionExercised is only ever constructed inside the branch that
    // actually found a durable ChangeEvent — never inferred from evidence
    // or schedule presence alone.
    let match_index = watch_health
        .find("match change_detection::read_change_event(store, &id)")
        .expect("expected a read_change_event match");
    let some_arm_index = watch_health[match_index..]
        .find("Some(event) => {")
        .expect("expected a Some(event) arm after the read_change_event match");
    let production_exercised_index = watch_health[match_index..]
        .find("Ok(ChangeDetectionReadiness::ProductionExercised {")
        .expect("expected a ProductionExercised construction site");
    assert!(
        some_arm_index < production_exercised_index,
        "ProductionExercised must only be constructed inside the Some(event) arm of the \
         read_change_event match — never before a real ChangeEvent was actually found"
    );
}

#[test]
fn watch_health_never_duplicates_watch_state_evidence_bundle_or_change_event() {
    let watch_health =
        fs::read_to_string(workspace_root().join("spider/src/features/watch_health.rs")).unwrap();
    for forbidden in [
        "struct WatchState",
        "enum WatchState",
        "struct EvidenceBundle",
        "struct ChangeEvent",
    ] {
        assert!(
            !watch_health.contains(forbidden),
            "watch_health.rs must reference WatchState/EvidenceBundle/ChangeEvent by reading \
             them, never redefine or embed a duplicate: found {forbidden:?}"
        );
    }
    // WatchHealthReport references the most recent comparison by
    // ChangeEventId only, never a full ChangeEvent field.
    assert!(watch_health.contains("most_recent_change_event: ChangeEventId,"));
    assert!(!watch_health.contains("most_recent_change_event: ChangeEvent,"));
}

#[test]
fn no_second_provider_health_architecture() {
    for pattern in [
        "struct ProviderHealth",
        "enum ProviderHealth",
        "struct SourceHealth",
        "enum SourceHealth",
    ] {
        assert_pattern_only_in_files(
            pattern,
            &[],
            "no second provider/source-health architecture may be introduced anywhere in \
             spider/src — ProviderDescriptor/ProviderCapabilities remain the sole \
             capability-declaration vocabulary for source providers",
        );
    }
    let source_provider =
        fs::read_to_string(workspace_root().join("spider/src/features/source_provider.rs"))
            .unwrap();
    assert!(source_provider.contains("pub struct ProviderDescriptor {"));
    assert!(!source_provider.contains("Health"));
}

#[test]
fn watch_health_never_implements_out_of_scope_capabilities() {
    let watch_health =
        fs::read_to_string(workspace_root().join("spider/src/features/watch_health.rs")).unwrap();
    for forbidden in [
        "struct Notification",
        "enum Notification",
        "struct Job",
        "enum Job",
        "struct Operation",
        "enum Operation",
        "struct Scheduler",
        "struct WatchTarget",
        "struct WatchSpec",
        "EventSourc",
        "struct MonitoringFramework",
    ] {
        assert!(
            !watch_health.contains(forbidden),
            "watch_health.rs must stay scoped to observational health assessment: found \
             forbidden pattern {forbidden:?}"
        );
    }
}

#[test]
fn no_shadow_watch_health_model_in_cli_or_mcp() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    for crate_dir in ["spider_cli/src", "spider_mcp/src"] {
        let dir = manifest_dir.parent().unwrap().join(crate_dir);
        if !dir.exists() {
            continue;
        }
        let mut files = Vec::new();
        collect_rust_files(&dir, &dir, &mut files);
        for file in files {
            for shadow in [
                "enum HealthStatus",
                "enum ChangeDetectionReadiness",
                "struct WatchHealthReport",
                "enum WatchHealthError",
                "fn assess_watch_health",
            ] {
                assert!(
                    !file.contents.contains(shadow),
                    "{crate_dir}/{} must not define its own {shadow:?} — the canonical \
                     health model is owned exclusively by spider::features::watch_health",
                    file.relative_path
                );
            }
        }
    }
}

// --- SECTION: SCORPION_CANONICAL_PRODUCTION_CAPTCHA_EXECUTION_CONVERGENCE_001 ---
//
// Audit of the live production CAPTCHA path (utils/mod.rs's solver gate,
// under `real_browser`) against the already-closed canonical CAPTCHA
// architecture (`features/captcha.rs`'s `CaptchaProviderRegistry`/
// `CaptchaRouteAttempts`, `features/captcha_browser.rs`'s
// `execute_browser_captcha_attempt`). Finding: `cf_handle`/
// `imperva_handle` invoke no CAPTCHA provider at all (pure DOM
// click/drag/wait heuristics — genuinely out of scope for provider
// convergence); `recaptcha_handle`/`geetest_handle`/`lemin_handle`
// already dispatch through the canonical registry/route-attempts seam,
// but their surrounding browser image-capture/action-application layer
// remains bespoke rather than `BrowserChallengeSnapshot`/
// `execute_browser_captcha_attempt` — deferred (not this frontier)
// because it would change the production pixel source and cannot be
// validated against real challenge pages here. One confirmed bug fixed:
// `solve_enterprise_with_browser_gemini` no longer returns a fabricated
// `Ok(Vec::new())` "nothing to click" success when the local provider is
// unavailable and no `GEMINI_API_KEY` is set — it now fails closed with a
// truthful error. See
// `docs/frontier/CANONICAL_PRODUCTION_CAPTCHA_EXECUTION_CONVERGENCE_SDD.md`.

#[test]
fn production_solver_gate_reaches_every_vendor_handler() {
    // Reachability proof: the live production fetch path (not a test, not
    // an example) is the one and only caller that gates on real-time
    // detection and dispatches to each vendor handler.
    let utils_mod = fs::read_to_string(workspace_root().join("spider/src/utils/mod.rs")).unwrap();
    for handler in [
        "crate::features::solvers::cf_handle(",
        "crate::features::solvers::imperva_handle(",
        "crate::features::solvers::recaptcha_handle(",
        "crate::features::solvers::geetest_handle(",
        "crate::features::solvers::lemin_handle(",
    ] {
        assert!(
            utils_mod.contains(handler),
            "production solver gate (spider/src/utils/mod.rs) must reach {handler:?} — if it \
             no longer does, this frontier's reachability audit is stale and must be redone"
        );
    }
}

#[test]
fn dom_heuristic_handlers_are_explicitly_classified_and_invoke_no_provider() {
    let solvers =
        fs::read_to_string(workspace_root().join("spider/src/features/solvers.rs")).unwrap();
    for handler_doc_anchor in ["pub async fn cf_handle(", "pub async fn imperva_handle("] {
        let index = solvers
            .find(handler_doc_anchor)
            .unwrap_or_else(|| panic!("expected to find {handler_doc_anchor:?}"));
        let preceding = &solvers[..index];
        let doc_start = preceding
            .rfind("/// Handle")
            .expect("expected a doc comment block starting with '/// Handle'");
        let doc_block = &solvers[doc_start..index];
        assert!(
            doc_block.contains("LEGACY_DOM_HEURISTIC"),
            "{handler_doc_anchor} must carry an explicit LEGACY_DOM_HEURISTIC classification \
             — it invokes no CAPTCHA provider and must never be presented as canonical \
             provider dispatch"
        );
        // Checked as an actual dispatch call form, not a bare type-name
        // mention — the classification doc legitimately *names*
        // CaptchaProviderRegistry in English to explain why this handler
        // does not use it.
        assert!(
            !doc_block.contains("CaptchaProviderRegistry::new()")
                && !doc_block.contains(".execute_explicit_attempt("),
            "{handler_doc_anchor}'s own classification must not claim provider dispatch"
        );
    }
}

#[test]
fn provider_dispatch_handlers_are_explicitly_classified_and_reuse_canonical_dispatch() {
    let solvers =
        fs::read_to_string(workspace_root().join("spider/src/features/solvers.rs")).unwrap();
    for (handler_doc_anchor, dispatch_fn) in [
        (
            "pub async fn recaptcha_handle(",
            "solve_enterprise_with_browser_gemini",
        ),
        (
            "pub async fn lemin_handle(",
            "solve_point_with_legacy_routing",
        ),
        (
            "pub async fn geetest_handle(",
            "solve_horizontal_offset_with_legacy_routing",
        ),
    ] {
        let index = solvers
            .find(handler_doc_anchor)
            .unwrap_or_else(|| panic!("expected to find {handler_doc_anchor:?}"));
        let preceding = &solvers[..index];
        let doc_start = preceding
            .rfind("/// CANONICAL_PROVIDER_DISPATCH_LEGACY_BINDING")
            .or_else(|| preceding.rfind("/// Lemin solve handler."))
            .expect("expected a classification doc comment immediately before this handler");
        let doc_block = &solvers[doc_start..index];
        assert!(
            doc_block.contains("CANONICAL_PROVIDER_DISPATCH_LEGACY_BINDING"),
            "{handler_doc_anchor} must carry an explicit CANONICAL_PROVIDER_DISPATCH_LEGACY_BINDING \
             classification — it does reach the canonical provider dispatch seam, but browser \
             binding around it remains bespoke"
        );
        assert!(doc_block.contains(dispatch_fn));
    }

    // Each named dispatch function genuinely uses the canonical registry
    // and route-attempts ledger — not a raw/ad-hoc provider call.
    for dispatch_fn_signature in [
        "async fn solve_enterprise_with_browser_gemini(",
        "async fn solve_point_with_legacy_routing(",
        "async fn solve_horizontal_offset_with_legacy_routing(",
    ] {
        let index = solvers
            .find(dispatch_fn_signature)
            .unwrap_or_else(|| panic!("expected to find {dispatch_fn_signature:?}"));
        // Scan a bounded window after the signature rather than to EOF or
        // the next `fn`, so this stays robust to unrelated edits.
        let window = &solvers[index..(index + 3000).min(solvers.len())];
        assert!(
            window.contains("CaptchaProviderRegistry::new()"),
            "{dispatch_fn_signature} must construct a real CaptchaProviderRegistry"
        );
        assert!(
            window.contains("CaptchaRouteAttempts::new()")
                && window.contains(".execute_explicit_attempt("),
            "{dispatch_fn_signature} must dispatch through CaptchaRouteAttempts::execute_explicit_attempt \
             — the canonical capability-prevalidated dispatch primitive"
        );
    }
}

#[test]
fn provider_unavailable_never_becomes_a_fabricated_empty_success() {
    let solvers =
        fs::read_to_string(workspace_root().join("spider/src/features/solvers.rs")).unwrap();
    // The specific fixed bug must never reappear: a provider-unavailable
    // branch silently returning a fabricated "nothing to click" success.
    assert!(
        !solvers.contains("Err(_) => return Ok(Vec::new())"),
        "provider-unavailable must never be converted into a fabricated empty-selection \
         success — CaptchaSolution::SelectedChoices(vec![]) is a legitimate answer only when \
         a provider actually examined the challenge, never a stand-in for 'no provider ran'"
    );
    // solve_enterprise_with_browser_gemini specifically now fails closed.
    let anchor = solvers
        .find("async fn solve_enterprise_with_browser_gemini(")
        .expect("expected to find solve_enterprise_with_browser_gemini");
    let window = &solvers[anchor..(anchor + 4000).min(solvers.len())];
    assert!(window.contains("return Err(CdpError::msg("));
    assert!(window.contains("local CAPTCHA provider unavailable"));
}

#[test]
fn canonical_captcha_execution_seam_is_not_reimplemented_in_solvers() {
    // The composed materialize -> route -> revalidate -> apply seam
    // remains defined exactly once, in captcha_browser.rs — solvers.rs
    // must never grow its own duplicate.
    assert_pattern_only_in_files(
        "async fn execute_browser_captcha_attempt(",
        &["features/captcha_browser.rs"],
        "execute_browser_captcha_attempt must only be defined in the canonical \
         captcha_browser module",
    );
    assert_pattern_only_in_files(
        "async fn execute_browser_captcha_attempt_in_frame(",
        &["features/captcha_browser.rs"],
        "execute_browser_captcha_attempt_in_frame must only be defined in the canonical \
         captcha_browser module",
    );
    // Checked as actual code forms (a real capture/import/struct-literal
    // use), not prose — this frontier's own classification doc comments
    // legitimately *name* BrowserChallengeSnapshot in English to explain
    // why solvers.rs's browser binding does not yet use it.
    let solvers =
        fs::read_to_string(workspace_root().join("spider/src/features/solvers.rs")).unwrap();
    for forbidden in [
        "BrowserChallengeSnapshot::capture",
        "use crate::features::browser_challenge::BrowserChallengeSnapshot",
        "BrowserChallengeSnapshot {",
    ] {
        assert!(
            !solvers.contains(forbidden),
            "solvers.rs must not reimplement the canonical snapshot capture/revalidate/apply \
             seam — it may only reuse CaptchaProviderRegistry/CaptchaRouteAttempts dispatch: \
             found {forbidden:?}"
        );
    }
}

#[test]
fn no_shadow_captcha_dispatch_in_cli_or_mcp() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    for crate_dir in ["spider_cli/src", "spider_mcp/src"] {
        let dir = manifest_dir.parent().unwrap().join(crate_dir);
        if !dir.exists() {
            continue;
        }
        let mut files = Vec::new();
        collect_rust_files(&dir, &dir, &mut files);
        for file in files {
            for shadow in [
                "CaptchaProviderRegistry::new()",
                "solvers::cf_handle",
                "solvers::imperva_handle",
                "solvers::recaptcha_handle",
                "solvers::geetest_handle",
                "solvers::lemin_handle",
            ] {
                assert!(
                    !file.contents.contains(shadow),
                    "{crate_dir}/{} must not construct its own CAPTCHA dispatch or call \
                     production solver-gate internals directly — production CAPTCHA \
                     execution is owned exclusively by the canonical seam plus the \
                     classified solvers.rs handlers",
                    file.relative_path
                );
            }
        }
    }
}
