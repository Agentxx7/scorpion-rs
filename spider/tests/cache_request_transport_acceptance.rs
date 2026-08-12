//! Shared acceptance contract for cache_request transport convergence.

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
fn canonical_cache_graph_has_one_network_executor() {
    let cache = read("spider/src/cache_request.rs");
    assert!(cache.contains("pub(crate) struct CanonicalCacheExecutor"));
    assert!(cache.contains("executor.execute(request)"));
    for forbidden in [
        "reqwest::Client",
        "ClientWithMiddleware",
        "reqwest_middleware",
        ".send()",
        "redirect(",
        "Proxy::",
        "danger_accept_invalid_certs",
        "dns_resolver",
        "local_address",
    ] {
        assert!(
            !cache.contains(forbidden),
            "cache owns transport primitive {forbidden}"
        );
    }
}

#[test]
fn canonical_website_remains_executor_based_with_cache_request() {
    let website = read("spider/src/website.rs");
    assert!(website.contains("resolved_executor: Option<Arc<ResolvedExecutor>>"));
    // Four historical implementations remain syntactically retained only
    // behind an impossible cfg while compatibility history is unwound. They
    // cannot compile, and the dependencies required to revive them are gone.
    let compact: String = website
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    assert_eq!(
        compact
            .matches("feature=\"cache_request\",not(feature=\"cache_request\")")
            .count(),
        4
    );
    assert_eq!(
        website.matches("reqwest_middleware::ClientBuilder").count(),
        4
    );
    assert_eq!(website.matches("HttpCache {").count(), 3);
    assert!(!website.contains("cache_request builds are explicitly noncanonical"));
}

#[test]
fn old_parallel_transport_dependencies_are_removed() {
    let manifest = read("spider/Cargo.toml");
    for forbidden in [
        "reqwest-middleware =",
        "spider-http-cache-reqwest",
        "http-global-cache",
    ] {
        assert!(
            !manifest.contains(forbidden),
            "old cache transport dependency {forbidden}"
        );
    }
    let client = read("spider/src/client.rs");
    assert!(!client.contains("ClientWithMiddleware"));
}

#[test]
fn secret_material_is_never_cache_identity_or_metadata() {
    let cache = read("spider/src/cache_request.rs");
    for required in [
        "request.secret_headers.is_empty()",
        "AUTHORIZATION",
        "PROXY_AUTHORIZATION",
        "COOKIE",
        "CacheRequestIdentity",
    ] {
        assert!(
            cache.contains(required),
            "missing secret safety rule {required}"
        );
    }
    assert!(!cache.contains("secret_headers.apply_to"));
    assert!(!cache.contains("Authorized(token)"));
}

#[test]
fn response_origin_and_failure_provenance_are_truthful() {
    let cache = read("spider/src/cache_request.rs");
    assert!(cache.contains("ResponseOrigin::ReconstructedCache"));
    assert!(cache.contains("ResponseOrigin::Network"));
    assert!(cache.contains("BackendProvenance::CacheLayer"));
    assert!(!cache.contains("BackendProvenance::CacheMiddleware"));
    assert!(cache.contains("CrawlerFailure"));
    assert!(!cache.contains("reqwest::Error"));
}

#[test]
fn cache_materialization_is_explicit_and_artifacts_are_unchanged() {
    let cache = read("spider/src/cache_request.rs");
    assert!(cache.contains("materialize_network_response"));
    assert!(cache.contains("CrawlerBodyStream"));
    let artifact = read("spider/src/features/artifact_download_execution.rs");
    assert!(!artifact.contains("CanonicalCacheExecutor"));
}

#[test]
fn tor_and_invalid_proxy_semantics_stay_below_cache() {
    let cache = read("spider/src/cache_request.rs");
    assert!(!cache.contains("TransportPolicy::Default"));
    assert!(!cache.contains("InvalidProxy"));
    assert!(cache.contains("executor.execute(request)"));
}

#[test]
fn synthetic_negative_scanner_rejects_parallel_transport_reintroduction() {
    fn violation(source: &str) -> bool {
        [
            "ClientWithMiddleware",
            "reqwest_middleware::ClientBuilder",
            "reqwest::ClientBuilder",
            "RequestBuilder.send",
            "cache_hit.origin = ResponseOrigin::Network",
            "secret_headers.serialize",
        ]
        .iter()
        .any(|pattern| source.contains(pattern))
    }
    for fixture in [
        "let client: ClientWithMiddleware = build();",
        "reqwest::ClientBuilder::new().redirect(policy)",
        "cache_hit.origin = ResponseOrigin::Network;",
        "secret_headers.serialize(writer);",
    ] {
        assert!(violation(fixture), "negative fixture escaped: {fixture}");
    }
    assert!(!violation("executor.execute(request).await"));
}

#[test]
fn cache_miss_api_requires_no_client_lending() {
    let cache = read("spider/src/cache_request.rs");
    assert!(cache.contains("ResolvedExecutor"));
    assert!(cache.contains("CrawlerRequest"));
    assert!(!cache.contains("pub fn client"));
}
