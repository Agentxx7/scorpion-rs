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
    assert!(website.contains("resolved_executor: Option<Arc<ResolvedExecutor>>"));
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
