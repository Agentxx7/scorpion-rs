//! Architectural acceptance contract for Gemini solver transport convergence.

use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn source() -> String {
    fs::read_to_string(root().join("spider/src/features/solvers.rs")).unwrap()
}

fn direct_gemini_transport_violation(source: &str) -> bool {
    [
        "GEMINI_CLIENT",
        "reqwest::ClientBuilder",
        "wreq::ClientBuilder",
        "generateContent?key=",
        ".post(&url).send()",
        ".get(tile.img_src).send()",
    ]
    .iter()
    .any(|pattern| source.contains(pattern))
}

#[test]
fn one_persistent_feature_selected_executor_owns_all_solver_networking() {
    let source = source();
    assert!(source.contains("static ref GEMINI_EXECUTOR: CanonicalExecutor"));
    assert!(source.contains("fn resolve_gemini_executor() -> CanonicalExecutor"));
    assert!(!direct_gemini_transport_violation(&source));
}

#[test]
fn challenge_get_and_all_post_families_use_crawler_request() {
    let source = source();
    assert!(source.contains("async fn materialize_remote_challenge"));
    assert!(source.contains("fn gemini_post_request"));
    assert!(source.contains("CrawlerRequest::get"));
    assert!(source.contains("execute_external_gemini_json"));
}

#[test]
fn api_credentials_are_ephemeral_secret_headers_never_url_queries() {
    let source = source();
    assert!(source.contains("SecretRequestHeaders::new()"));
    assert!(source.contains("try_insert(\"x-goog-api-key\", api_key)"));
    assert!(source.contains("name.eq_ignore_ascii_case(\"key\")"));
    assert!(!source.contains("?key={}"));
    assert!(!source.contains("generateContent?key="));
}

#[test]
fn neutral_response_and_failure_seam_is_consumed() {
    let source = source();
    for required in [
        "CrawlerBodyStream",
        "CrawlerFailure",
        "CrawlerFailureKind::HttpStatus",
        "response.backend",
        "collect_gemini_body",
        "CaptchaSolveFailure::Transport",
    ] {
        assert!(
            source.contains(required),
            "missing neutral seam use: {required}"
        );
    }
}

#[test]
fn solver_policy_remains_above_transport_execution() {
    let source = source();
    assert!(source.contains(".acquire_many(permits)"));
    assert!(source.contains(".acquire()"));
    assert!(source.contains("tokio::time::timeout(request.deadline"));
    assert!(source.contains("per_operation"));
}

#[test]
fn canonical_executor_supplies_validation_redirect_and_backend_provenance() {
    let transport = fs::read_to_string(root().join("spider_transport/src/transport.rs")).unwrap();
    assert!(transport.contains("validate_target(&request.url"));
    assert!(transport.contains("canonical_redirect_decision"));
    assert!(transport.contains("BackendProvenance::Reqwest"));
    assert!(transport.contains("BackendProvenance::Wreq"));
}

#[test]
fn synthetic_raw_transport_reintroduction_is_detected() {
    for fixture in [
        "static GEMINI_CLIENT: reqwest::Client",
        "wreq::ClientBuilder::new()",
        "generateContent?key=secret",
        "GEMINI_CLIENT.get(tile.img_src).send()",
    ] {
        assert!(direct_gemini_transport_violation(fixture));
    }
    assert!(!direct_gemini_transport_violation(
        "GEMINI_EXECUTOR.execute(CrawlerRequest::get(url)).await"
    ));
}
