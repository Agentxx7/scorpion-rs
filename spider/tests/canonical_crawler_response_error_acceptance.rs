use spider_transport::{BackendProvenance, CrawlerFailure, CrawlerFailureKind, ResponseOrigin};

#[test]
fn neutral_failure_vocabulary_covers_retry_facts() {
    let variants = [
        CrawlerFailureKind::Timeout,
        CrawlerFailureKind::Dns,
        CrawlerFailureKind::TlsHandshake,
        CrawlerFailureKind::ProxyTunnel,
        CrawlerFailureKind::ConnectionRefused,
        CrawlerFailureKind::ConnectionAborted,
        CrawlerFailureKind::ConnectionReset,
        CrawlerFailureKind::ConnectionUnreachable,
        CrawlerFailureKind::Connection,
        CrawlerFailureKind::Request,
        CrawlerFailureKind::BodyStream,
        CrawlerFailureKind::Decode,
        CrawlerFailureKind::HttpStatus,
        CrawlerFailureKind::ProtocolRetryable,
        CrawlerFailureKind::ProtocolPermanent,
        CrawlerFailureKind::Other,
    ];
    assert_eq!(variants.len(), 16);
}

#[test]
fn source_chain_is_preserved_without_backend_contract() {
    let source = std::io::Error::new(std::io::ErrorKind::TimedOut, "sentinel-source");
    let failure = CrawlerFailure::with_source(
        CrawlerFailureKind::Timeout,
        BackendProvenance::Reqwest,
        source,
    );
    assert!(std::error::Error::source(&failure).is_some());
    assert!(failure.to_string().contains("sentinel-source"));
    assert!(!format!("{failure:?}").contains("sentinel-source"));
}

#[test]
fn reconstructed_cache_never_claims_network_origin() {
    assert_ne!(ResponseOrigin::ReconstructedCache, ResponseOrigin::Network);
}

#[test]
fn page_error_details_are_neutral() {
    let page = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/page.rs"))
        .expect("page source");
    assert!(page.contains("Arc<spider_transport::CrawlerFailure>"));
    assert!(!page.contains("Arc<reqwest::Error>"));
}

#[test]
fn page_response_does_not_expose_backend_error_result() {
    let utils = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/utils/mod.rs"))
        .expect("utils source");
    assert!(utils.contains("pub failure: Option<spider_transport::CrawlerFailure>"));
    assert!(!utils.contains("pub error_for_status: Option<Result<Response, RequestError>>"));
}

#[test]
fn retry_policy_remains_spider_owned() {
    let transport = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../spider_transport/src/crawler_outcome.rs"
    ))
    .expect("neutral seam");
    assert!(!transport.contains("should_retry"));
    assert!(!transport.contains("backoff"));
    assert!(!transport.contains("is_retryable_status"));
}

#[test]
fn backend_provenance_keeps_compatibility_identity_visible() {
    assert_ne!(BackendProvenance::Reqwest, BackendProvenance::Wreq);
    assert_ne!(
        BackendProvenance::Reqwest,
        BackendProvenance::CacheMiddleware
    );
}
