//! Acceptance contract for explicit wreq authority and compatibility isolation.

use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn read(path: &str) -> String {
    fs::read_to_string(root().join(path)).unwrap_or_else(|error| panic!("{path}: {error}"))
}

#[test]
fn website_resolves_wreq_before_execution() {
    let website = read("spider/src/website.rs");
    let transport = read("spider_transport/src/transport.rs");
    assert!(transport.contains("NoncanonicalWreq"));
    assert!(website.contains("ExecutionMode::NoncanonicalWreq"));
    assert!(website.contains("fn prepare_execution"));
    assert!(website.contains("crawl_sitemap_chrome"));
}

#[test]
fn wreq_never_receives_canonical_executor_identity() {
    let website = read("spider/src/website.rs");
    assert!(website.contains("#[cfg(feature = \"wreq\")]"));
    assert!(!website.contains("ExecutionMode::CanonicalWreq"));
    assert!(website.contains("mode != ExecutionMode::Canonical"));
}

#[test]
fn wreq_success_and_failure_provenance_are_truthful() {
    let page = read("spider/src/page.rs");
    let utils = read("spider/src/utils/mod.rs");
    assert!(page.contains("BackendProvenance::Wreq"));
    assert!(utils.contains("BackendProvenance::Wreq"));
    assert!(utils.contains("AcquisitionOrigin::Network"));
    assert!(!page.contains("Arc<wreq::Error>"));
}

#[test]
fn raw_wreq_apis_are_compatibility_boundaries() {
    let page = read("spider/src/page.rs");
    let website = read("spider/src/website.rs");
    assert!(page.contains("UPSTREAM_COMPATIBILITY_BOUNDARY"));
    assert!(website.contains("UPSTREAM_COMPATIBILITY_BOUNDARY"));
    for api in [
        "pub type Client = wreq::Client",
        "pub type ClientBuilder = wreq::ClientBuilder",
    ] {
        assert!(read("spider/src/client.rs").contains(api));
    }
}

#[test]
fn canonical_capabilities_reject_or_exclude_wreq() {
    let evidence = read("spider/src/utils/evidence.rs");
    let artifact = read("spider/src/features/mod.rs");
    let github = read("spider/src/features/github_source_provider.rs");
    let hugging_face = read("spider/src/features/hugging_face_source_provider.rs");
    assert!(evidence.contains("canonical evidence acquisition is unavailable under wreq"));
    assert!(artifact.contains("all(feature = \"evidence\", not(feature = \"wreq\"))"));
    assert!(github.contains("unavailable under wreq"));
    assert!(hugging_face.contains("unavailable under wreq"));
}

#[test]
fn cache_and_tor_remain_rejected() {
    let lib = read("spider/src/lib.rs");
    let evidence = read("spider/src/utils/evidence.rs");
    assert!(lib.contains("cache_request + wreq is explicitly rejected"));
    assert!(evidence.contains("Tor transport requires a build without the wreq feature"));
}

#[test]
fn canonical_products_do_not_select_wreq() {
    for manifest in [
        "spider_cli/Cargo.toml",
        "spider_mcp/Cargo.toml",
        "spider_agent/Cargo.toml",
    ] {
        let contents = read(manifest);
        assert!(
            !contents.contains("spider/wreq"),
            "canonical product selects wreq: {manifest}"
        );
    }
}

#[test]
fn gemini_is_separately_noncanonical() {
    let solvers = read("spider/src/features/solvers.rs");
    assert!(solvers.contains("CAPABILITY_LOCAL_NONCANONICAL_WREQ"));
}

#[test]
fn negative_scanner_detects_wreq_authority_violations() {
    fn violation(source: &str) -> bool {
        [
            "ExecutionMode::CanonicalWreq",
            "canonical_wreq_client",
            "ResolvedExecutor::from_wreq",
            "canonical_secret_headers.apply_to(wreq_request)",
        ]
        .iter()
        .any(|pattern| source.contains(pattern))
    }
    for fixture in [
        "ExecutionMode::CanonicalWreq",
        "let client = canonical_wreq_client();",
        "ResolvedExecutor::from_wreq(client)",
        "canonical_secret_headers.apply_to(wreq_request)",
    ] {
        assert!(violation(fixture), "negative fixture escaped: {fixture}");
    }
    assert!(!violation("ExecutionMode::NoncanonicalWreq"));
}

#[cfg(feature = "wreq")]
#[tokio::test]
async fn live_wreq_page_reports_network_backend() {
    use spider_transport::{BackendProvenance, ResponseOrigin};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await
            .unwrap();
    });

    let client = spider::Client::new();
    let page = spider::page::Page::new_page(&format!("http://{address}/"), &client).await;
    assert_eq!(page.backend_provenance(), Some(BackendProvenance::Wreq));
    assert_eq!(page.response_origin(), Some(ResponseOrigin::Network));
}

#[cfg(feature = "wreq")]
#[tokio::test]
async fn ordinary_website_wreq_execution_is_live_and_provenanced() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        observed.fetch_add(1, Ordering::SeqCst);
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 28\r\nConnection: close\r\n\r\n<html><body>ok</body></html>")
            .await
            .unwrap();
    });

    let mut website = spider::website::Website::new(&format!("http://{address}/"));
    website.with_limit(1);
    website.crawl_raw().await;
    assert_eq!(requests.load(Ordering::SeqCst), 1);
}

#[cfg(all(feature = "wreq", feature = "evidence"))]
#[tokio::test]
async fn canonical_evidence_rejects_wreq_before_network() {
    let error = spider::utils::evidence::fetch_single_page("http://127.0.0.1:9/")
        .await
        .unwrap_err();
    assert!(error.contains("unavailable under wreq"));
}

#[cfg(all(feature = "wreq", feature = "transport_tor"))]
#[tokio::test]
async fn tor_wreq_rejects_without_fallback() {
    use spider::features::transport::{TorTransportConfig, TransportPolicy};
    use spider::utils::evidence::{fetch_single_page_with_options, AcquisitionOptions};

    let policy = TransportPolicy::Tor(
        TorTransportConfig::new("socks5h://127.0.0.1:9050").expect("valid Tor endpoint"),
    );
    let error = fetch_single_page_with_options(
        "http://127.0.0.1:9/",
        AcquisitionOptions { transport: policy },
    )
    .await
    .unwrap_err();
    assert!(error.contains("requires a build without the wreq feature"));
}
