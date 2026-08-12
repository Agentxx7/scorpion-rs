//! Canonical HTTP transport policy: `Default` (preserve existing
//! Scorpion/Spider networking behavior) and `Tor` (fail-closed
//! SOCKS5h-over-Tor, with proxy-side hostname resolution and mandatory
//! `.onion` protection).
//!
//! This module is a thin re-export façade: every type, security primitive,
//! and execution seam is owned by the canonical `spider_transport` leaf
//! crate. The only items defined here are thin delegating wrappers that
//! inject Spider's own default user-agent (`configuration::get_ua`) into
//! the leaf's parameterized client-construction seams, preserving the
//! exact pre-extraction signatures. No client construction, validation,
//! redirect, or Tor logic lives in this file.

pub use spider_transport::{
    is_onion_url, validate_target, AcquisitionTransport, TorTransportConfig, TransportError,
    TransportMode, TransportPolicy, TransportRequest,
};

pub(crate) use spider_transport::{
    acquisition_transport_for, crawl_boundary_allows, current_acquisition_transport, is_onion_host,
    is_ssrf_redirect, target_dns_suppressed, CrawlBoundary, ACQUISITION_TRANSPORT_SCOPE,
};

/// Build the one canonical Tor-audited `reqwest::Client`. Thin wrapper over
/// [`spider_transport::build_tor_client`] supplying Spider's default
/// user-agent. Only exists with `transport_tor` compiled in and without the
/// `wreq`/`cache_request` alternate stacks (fail-closed siblings live in
/// `Website::tor_crawl_preflight` / `evidence::fetch_via_tor`).
#[cfg(all(
    feature = "transport_tor",
    not(feature = "wreq"),
    not(feature = "cache_request")
))]
pub(crate) fn build_tor_client(
    policy: &TransportPolicy,
) -> Result<reqwest::Client, TransportError> {
    spider_transport::build_tor_client(policy, crate::configuration::get_ua(false))
}

/// Execute one canonical, streaming, non-body-consuming HTTP GET. Thin
/// wrapper over [`spider_transport::execute_streaming_request`] supplying
/// Spider's default user-agent; the signature and semantics are exactly the
/// pre-extraction public seam.
#[cfg(all(not(feature = "wreq"), not(feature = "cache_request")))]
pub async fn execute_streaming_request(
    url: &url::Url,
    policy: &TransportPolicy,
    headers: &crate::features::secret_request_headers::SecretRequestHeaders,
) -> Result<reqwest::Response, TransportError> {
    spider_transport::execute_streaming_request(
        url,
        policy,
        headers,
        crate::configuration::get_ua(false),
    )
    .await
}
