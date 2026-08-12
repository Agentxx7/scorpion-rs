//! `spider_transport` — the single canonical owner of Scorpion's network
//! transport semantics.
//!
//! This neutral leaf crate owns:
//!
//! - the transport policy vocabulary ([`TransportPolicy`],
//!   [`TorTransportConfig`], [`TransportRequest`], [`TransportMode`]) and
//!   [`TransportError`];
//! - target validation ([`validate_target`]) and `.onion` classification
//!   ([`is_onion_url`], [`is_onion_host`]);
//! - redirect security (`pin_redirect_policy` transport pinning) and SSRF
//!   redirect screening ([`is_ssrf_redirect`] — the canonical primitive,
//!   extracted below `Website`, which delegates to it);
//! - canonical client construction (`build_tor_client`,
//!   `build_streaming_client`) and the canonical streaming execution seam
//!   ([`execute_streaming_request`]);
//! - acquisition transport provenance ([`AcquisitionTransport`],
//!   [`ACQUISITION_TRANSPORT_SCOPE`] task-local and helpers);
//! - crawl transport boundaries ([`CrawlBoundary`],
//!   [`crawl_boundary_allows`]);
//! - ephemeral secret request headers ([`SecretRequestHeaders`],
//!   [`SecretHeaderError`]).
//!
//! Dependency direction is strictly one-way: this crate depends on nothing
//! in the workspace; consumers (`spider`, and future canonical crates such
//! as `spider_search`) depend on it. Client construction is parameterized on
//! an explicit `user_agent: &str` supplied by the consumer, so the leaf
//! never reaches up into a consumer's configuration.

pub mod secret_request_headers;
pub mod transport;

pub use secret_request_headers::{SecretHeaderError, SecretRequestHeaders};
pub use transport::{
    acquisition_transport_for, crawl_boundary_allows, current_acquisition_transport, is_onion_host,
    is_onion_url, is_ssrf_redirect, target_dns_suppressed, validate_target, AcquisitionTransport,
    CrawlBoundary, TorTransportConfig, TransportError, TransportMode, TransportPolicy,
    TransportRequest, ACQUISITION_TRANSPORT_SCOPE,
};

#[cfg(feature = "tor")]
pub use transport::build_tor_client;

pub use transport::execute_streaming_request;
