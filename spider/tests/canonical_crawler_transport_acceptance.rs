//! Architecture acceptance contract for canonical crawler execution.

use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

fn read(path: &str) -> String {
    fs::read_to_string(root().join(path)).unwrap_or_else(|error| panic!("{path}: {error}"))
}

#[test]
fn leaf_owns_the_persistent_request_only_executor() {
    let transport = read("spider_transport/src/transport.rs");
    for required in [
        "pub struct ResolvedExecutor",
        "pub struct CrawlerTransportConfiguration",
        "pub struct CrawlerRequest",
        "pub enum ExecutionMode",
        "clients: Arc<[reqwest::Client]>",
        "SecretRequestHeaders",
    ] {
        assert!(transport.contains(required), "missing {required}");
    }
    assert!(!transport.contains("pub fn client("));
    assert!(!transport.contains("pub fn clients("));
    assert!(!transport.contains("use spider::"));
}

#[test]
fn website_state_and_execution_are_executor_owned() {
    let website = read("spider/src/website.rs");
    assert!(website.contains("resolved_executor: Option<Arc<CanonicalExecutor>>"));
    assert!(website.contains("resolve_execution_mode"));
    assert!(!website.contains("struct ClientRotator"));
    assert!(website.contains("struct NoncanonicalClientRotator"));
    assert!(website.contains("ExecutionMode::Canonical | ExecutionMode::CanonicalWreq"));
    assert!(!website.contains("self.client.take()"));
    assert!(website.contains("EXPLICIT_UPSTREAM_COMPATIBILITY_BOUNDARY"));
}

#[test]
fn canonical_website_never_calls_raw_page_compatibility_apis() {
    let website = read("spider/src/website.rs");
    for forbidden in [
        "Page::new_page(",
        "Page::new_page_streaming(",
        "Page::new_page_with_cache(",
        "fetch_page_html_raw_conditional(",
        ".get(sitemap_url.as_str()).send()",
        ".get(domain.as_str()).send()",
    ] {
        assert!(
            !website.contains(forbidden),
            "canonical Website contains {forbidden}"
        );
    }
    assert_eq!(website.matches("set_http_client(").count(), 1);
    assert_eq!(website.matches("get_client(").count(), 1);
}

#[test]
fn page_has_one_request_only_canonical_entry_point() {
    let page = read("spider/src/page.rs");
    assert!(page.contains("new_page_with_executor"));
    assert!(page.contains("fetch_page_html_with_executor"));
    assert!(!page.contains("CrawlerExecutionError"));
}

#[test]
fn canonical_callers_do_not_use_raw_page_compatibility() {
    for canonical in [
        "spider/src/website.rs",
        "spider/src/utils/evidence.rs",
        "spider/src/features/artifact_download_execution.rs",
    ] {
        let source = read(canonical);
        assert!(
            !source.contains("Page::new_page("),
            "raw Page API in {canonical}"
        );
        assert!(
            !source.contains("Page::new_page_streaming("),
            "raw streaming Page API in {canonical}"
        );
    }
    assert!(read("spider_worker/src/main.rs").contains("Page::new_page_streaming("));
}

#[test]
fn canonical_outcomes_have_one_neutral_owner() {
    let outcome = read("spider_transport/src/crawler_outcome.rs");
    let transport = read("spider_transport/src/transport.rs");
    for required in [
        "pub enum CrawlerFailureKind",
        "pub struct CrawlerFailure",
        "pub struct CrawlerResponse",
        "pub type CrawlerBodyStream",
        "pub enum BackendProvenance",
        "pub enum ResponseOrigin",
    ] {
        assert!(outcome.contains(required), "missing {required}");
        assert!(!transport.contains(required), "duplicate {required}");
    }
    assert!(!transport.contains("CrawlerExecutionError"));
}

#[test]
fn proxy_rotation_and_security_are_transport_owned() {
    let transport = read("spider_transport/src/transport.rs");
    for required in [
        "next_client.fetch_add",
        "TransportError::InvalidProxy",
        "validate_target(&request.url",
        "pin_redirect_policy(",
        "ssrf_screened_base_policy(",
        "crawler proxy rotation cannot be combined with Tor",
    ] {
        assert!(transport.contains(required), "missing {required}");
    }
}

#[test]
fn noncanonical_modes_are_explicit_before_execution() {
    let transport = read("spider_transport/src/transport.rs");
    let website = read("spider/src/website.rs");
    for mode in [
        "NoncanonicalHttpFetchEngine",
        "NoncanonicalRemoteFetcher",
        "UpstreamCompatibility",
    ] {
        assert!(transport.contains(mode), "missing {mode}");
    }
    assert!(website.contains("resolve_execution_mode"));
}

#[test]
fn cache_miss_contract_requires_no_raw_client_lending() {
    let sdd = read("docs/frontier/CANONICAL_CRAWLER_TRANSPORT_EXECUTION_SEAM_SDD.md");
    assert!(sdd.contains("future cache miss"));
    assert!(sdd.contains("ResolvedExecutor::execute"));
    assert!(sdd.contains("does not converge wreq or caching"));
}
