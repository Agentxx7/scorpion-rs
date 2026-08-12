//! Backend-neutral crawler execution facts. Retry decisions deliberately do
//! not live here; Spider consumes these facts and owns crawler policy.

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use std::error::Error;
use std::fmt;
use std::pin::Pin;
use std::sync::Arc;

use crate::transport::AcquisitionTransport;

/// Backend that observed or reconstructed an execution result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendProvenance {
    Reqwest,
    Wreq,
    /// Response or failure produced by the canonical cache layer. This does
    /// not identify a network backend and never implies middleware-owned
    /// transport execution.
    CacheLayer,
    NoncanonicalFetchEngine,
    NoncanonicalRemoteFetcher,
    UpstreamCompatibility,
}

/// Where a response representation came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseOrigin {
    Network,
    ReconstructedCache,
    Synthetic,
}

/// Transport facts used by crawler policy without exposing backend types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrawlerFailureKind {
    Timeout,
    Dns,
    TlsHandshake,
    ProxyTunnel,
    ConnectionRefused,
    ConnectionAborted,
    ConnectionReset,
    ConnectionUnreachable,
    Connection,
    Request,
    BodyStream,
    Decode,
    HttpStatus,
    ProtocolRetryable,
    ProtocolPermanent,
    Other,
}

type SharedSource = Arc<dyn Error + Send + Sync + 'static>;

/// One backend-neutral failure with an optional type-erased causal source.
pub struct CrawlerFailure {
    kind: CrawlerFailureKind,
    backend: BackendProvenance,
    observed_status: Option<http::StatusCode>,
    source: Option<SharedSource>,
}

impl CrawlerFailure {
    pub fn new(kind: CrawlerFailureKind, backend: BackendProvenance) -> Self {
        Self {
            kind,
            backend,
            observed_status: None,
            source: None,
        }
    }

    pub fn with_source<E>(kind: CrawlerFailureKind, backend: BackendProvenance, source: E) -> Self
    where
        E: Error + Send + Sync + 'static,
    {
        Self {
            kind,
            backend,
            observed_status: None,
            source: Some(Arc::new(source)),
        }
    }

    pub fn with_status(mut self, status: http::StatusCode) -> Self {
        self.observed_status = Some(status);
        self
    }

    pub fn kind(&self) -> CrawlerFailureKind {
        self.kind
    }
    pub fn backend(&self) -> BackendProvenance {
        self.backend
    }
    pub fn observed_status(&self) -> Option<http::StatusCode> {
        self.observed_status
    }
    pub fn source_ref(&self) -> Option<&(dyn Error + Send + Sync + 'static)> {
        self.source.as_deref()
    }
}

impl fmt::Debug for CrawlerFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CrawlerFailure")
            .field("kind", &self.kind)
            .field("backend", &self.backend)
            .field("observed_status", &self.observed_status)
            .field("has_source", &self.source.is_some())
            .finish()
    }
}

impl fmt::Display for CrawlerFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "crawler transport {:?} failure via {:?}",
            self.kind, self.backend
        )?;
        if let Some(source) = &self.source {
            write!(formatter, ": {source}")?;
        }
        Ok(())
    }
}

impl Error for CrawlerFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

pub type CrawlerBodyStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, CrawlerFailure>> + Send + 'static>>;

/// Backend-neutral streaming response. Constructing this value never reads a
/// body byte.
pub struct CrawlerResponse {
    pub status: http::StatusCode,
    pub headers: http::HeaderMap,
    pub final_url: url::Url,
    pub origin: ResponseOrigin,
    pub backend: BackendProvenance,
    pub transport: AcquisitionTransport,
    pub body: CrawlerBodyStream,
}

impl fmt::Debug for CrawlerResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CrawlerResponse")
            .field("status", &self.status)
            .field("final_url", &self.final_url)
            .field("origin", &self.origin)
            .field("backend", &self.backend)
            .field("transport", &self.transport)
            .finish_non_exhaustive()
    }
}

/// Translate a reqwest error at the backend boundary. Typed predicates and
/// typed `io::ErrorKind` values take precedence; bounded phrase inspection is
/// used only for facts reqwest does not expose as typed predicates.
pub fn from_reqwest_error(error: reqwest::Error) -> CrawlerFailure {
    let status = error.status();
    let kind = classify_reqwest_error(&error);
    let mut failure = CrawlerFailure::with_source(kind, BackendProvenance::Reqwest, error);
    if let Some(status) = status {
        failure = failure.with_status(status);
    }
    failure
}

fn classify_reqwest_error(error: &reqwest::Error) -> CrawlerFailureKind {
    if error.is_timeout() {
        return CrawlerFailureKind::Timeout;
    }
    if error.is_decode() {
        return CrawlerFailureKind::Decode;
    }
    if error.is_body() {
        return CrawlerFailureKind::BodyStream;
    }
    if error.status().is_some() {
        return CrawlerFailureKind::HttpStatus;
    }
    if error.is_connect() {
        let mut current = error.source();
        let mut depth = 0;
        while let Some(source) = current {
            if depth >= 8 {
                break;
            }
            if let Some(io) = source.downcast_ref::<std::io::Error>() {
                return match io.kind() {
                    std::io::ErrorKind::TimedOut => CrawlerFailureKind::Timeout,
                    std::io::ErrorKind::NotFound => CrawlerFailureKind::Dns,
                    std::io::ErrorKind::ConnectionRefused => CrawlerFailureKind::ConnectionRefused,
                    std::io::ErrorKind::ConnectionAborted => CrawlerFailureKind::ConnectionAborted,
                    std::io::ErrorKind::ConnectionReset => CrawlerFailureKind::ConnectionReset,
                    std::io::ErrorKind::HostUnreachable
                    | std::io::ErrorKind::NetworkUnreachable => {
                        CrawlerFailureKind::ConnectionUnreachable
                    }
                    _ => CrawlerFailureKind::Connection,
                };
            }
            let text = source.to_string().to_ascii_lowercase();
            if text.contains("dns error") || text.contains("failed to lookup address") {
                return CrawlerFailureKind::Dns;
            }
            if text.contains("handshake failure") || text.contains("certificate") {
                return CrawlerFailureKind::TlsHandshake;
            }
            if text.contains("tunnel")
                || text.contains("socks proxy")
                || text.contains("socks error")
            {
                return CrawlerFailureKind::ProxyTunnel;
            }
            current = source.source();
            depth += 1;
        }
        return CrawlerFailureKind::Connection;
    }
    if error.is_request() || error.is_builder() {
        return CrawlerFailureKind::Request;
    }
    CrawlerFailureKind::Other
}

/// Convert a live reqwest response without materializing its body.
pub fn from_reqwest_response(
    response: reqwest::Response,
    transport: AcquisitionTransport,
) -> CrawlerResponse {
    let status = response.status();
    let headers = response.headers().clone();
    let final_url = response.url().clone();
    let body = response
        .bytes_stream()
        .map(|item| item.map_err(from_reqwest_error));
    CrawlerResponse {
        status,
        headers,
        final_url,
        origin: ResponseOrigin::Network,
        backend: BackendProvenance::Reqwest,
        transport,
        body: Box::pin(body),
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[test]
    fn reqwest_request_error_retains_typed_source() {
        let error = reqwest::Client::new()
            .get("http://[")
            .build()
            .expect_err("malformed URL must fail");
        let failure = from_reqwest_error(error);
        assert_eq!(failure.kind(), CrawlerFailureKind::Request);
        assert_eq!(failure.backend(), BackendProvenance::Reqwest);
        assert!(failure.source_ref().is_some());
    }

    #[tokio::test]
    async fn reqwest_response_translation_keeps_metadata_and_streams_body() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket
                .write_all(b"HTTP/1.1 201 Created\r\nx-seam: neutral\r\ncontent-length: 4\r\nconnection: close\r\n\r\nbody")
                .await
                .unwrap();
        });

        let url = format!("http://{address}/stream");
        let response = reqwest::get(&url).await.unwrap();
        let mut neutral = from_reqwest_response(response, AcquisitionTransport::Default);
        assert_eq!(neutral.status, http::StatusCode::CREATED);
        assert_eq!(neutral.final_url.as_str(), url);
        assert_eq!(neutral.headers["x-seam"], "neutral");
        assert_eq!(neutral.origin, ResponseOrigin::Network);
        assert_eq!(neutral.backend, BackendProvenance::Reqwest);

        let first = neutral.body.next().await.unwrap().unwrap();
        assert_eq!(&first[..], b"body");
        assert!(neutral.body.next().await.is_none());
        server.await.unwrap();
    }
}
