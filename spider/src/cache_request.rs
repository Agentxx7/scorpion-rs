//! Cache semantics above the canonical crawler transport executor.

use http_cache::{
    CacheMode, HttpCache, HttpCacheOptions, HttpHeaders, HttpResponse, HttpVersion, Middleware,
};
use http_cache_semantics::{CacheOptions as SemanticsOptions, CachePolicy};
use spider_transport::{
    AcquisitionTransport, BackendProvenance, CrawlerBodyStream, CrawlerFailure, CrawlerFailureKind,
    CrawlerRequest, CrawlerResponse, ResolvedExecutor, ResponseOrigin,
};
use std::error::Error;
use std::fmt;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;
use tokio_stream::StreamExt;

#[cfg(all(feature = "cache", not(feature = "cache_mem")))]
type RequestCacheManager = http_cache::CACacheManager;
#[cfg(any(not(feature = "cache"), feature = "cache_mem"))]
type RequestCacheManager = http_cache::MokaManager;

#[cfg(all(feature = "cache", not(feature = "cache_mem")))]
fn make_manager() -> RequestCacheManager {
    http_cache::CACacheManager::new(std::env::temp_dir().join("spider-http-cache"), false)
}

#[cfg(any(not(feature = "cache"), feature = "cache_mem"))]
fn make_manager() -> RequestCacheManager {
    http_cache::MokaManager::default()
}

/// Compatibility name retained for hybrid-cache helpers. It is a cache
/// manager only and has no client or network execution capability.
pub static CACACHE_MANAGER: LazyLock<RequestCacheManager> = LazyLock::new(make_manager);

/// Cache-only request identity. It deliberately contains no headers or body.
struct CacheRequestIdentity<'a> {
    namespace: Option<&'a str>,
    method: &'a str,
    url: &'a str,
}

impl CacheRequestIdentity<'_> {
    fn key(&self) -> String {
        format!(
            "scorpion-cache-v1\n{}\n{}\n{}",
            self.namespace.unwrap_or_default(),
            self.method,
            self.url
        )
    }
}

#[derive(Debug)]
struct CacheExecutionError(String);

impl fmt::Display for CacheExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for CacheExecutionError {}

#[derive(Debug)]
struct TransportFailureMarker;

impl fmt::Display for TransportFailureMarker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("canonical transport execution failed")
    }
}

impl Error for TransportFailureMarker {}

/// Cache policy executor. Its only network authority is the borrowed
/// `ResolvedExecutor`; it owns no transport client or transport policy.
pub(crate) struct CanonicalCacheExecutor<'a> {
    executor: &'a ResolvedExecutor,
    namespace: Option<&'a str>,
}

impl<'a> CanonicalCacheExecutor<'a> {
    pub(crate) fn new(executor: &'a ResolvedExecutor, namespace: Option<&'a str>) -> Self {
        Self {
            executor,
            namespace,
        }
    }

    pub(crate) async fn execute(
        &self,
        request: CrawlerRequest,
        allow_cache: bool,
    ) -> Result<CrawlerResponse, CrawlerFailure> {
        if !allow_cache || !cacheable_request(&request) {
            return self.executor.execute(request).await;
        }

        let namespace = self.namespace.map(ToOwned::to_owned);
        let mut options = HttpCacheOptions::default();
        options.cache_key = Some(Arc::new(move |parts| {
            CacheRequestIdentity {
                namespace: namespace.as_deref(),
                method: parts.method.as_str(),
                url: &parts.uri.to_string(),
            }
            .key()
        }));
        let cache = HttpCache {
            mode: CacheMode::Default,
            manager: CACACHE_MANAGER.clone(),
            options,
        };
        let transport_failure = Arc::new(Mutex::new(None));
        let adapter = CanonicalCacheAdapter {
            executor: self.executor,
            identity_url: request.url.clone(),
            identity_method: request.method.clone(),
            identity_headers: request.headers.clone(),
            request: Some(request),
            transport_failure: transport_failure.clone(),
        };
        match cache.run(adapter).await {
            Ok(response) => reconstruct_response(response),
            Err(error) => {
                if let Some(failure) = transport_failure.lock().expect("failure lock").take() {
                    Err(failure)
                } else {
                    Err(CrawlerFailure::with_source(
                        CrawlerFailureKind::Other,
                        BackendProvenance::CacheLayer,
                        CacheExecutionError(error.to_string()),
                    ))
                }
            }
        }
    }
}

pub(crate) async fn fetch_page_html_with_cache_executor(
    target_url: &str,
    executor: &ResolvedExecutor,
    cache_options: Option<crate::utils::CacheOptions>,
    namespace: Option<&str>,
) -> crate::utils::PageResponse {
    let url = match url::Url::parse(target_url) {
        Ok(url) => url,
        Err(error) => {
            return crate::utils::PageResponse {
                failure: Some(CrawlerFailure::with_source(
                    CrawlerFailureKind::Request,
                    BackendProvenance::Reqwest,
                    error,
                )),
                ..Default::default()
            };
        }
    };
    let allow_cache = matches!(
        cache_options,
        Some(crate::utils::CacheOptions::Yes | crate::utils::CacheOptions::SkipBrowser)
    );
    match CanonicalCacheExecutor::new(executor, namespace)
        .execute(CrawlerRequest::get(url), allow_cache)
        .await
    {
        Ok(response) => {
            crate::utils::page_response_from_crawler_response(target_url, response).await
        }
        Err(failure) => crate::utils::PageResponse {
            failure: Some(failure),
            ..Default::default()
        },
    }
}

fn cacheable_request(request: &CrawlerRequest) -> bool {
    use reqwest::header::{AUTHORIZATION, COOKIE, PROXY_AUTHORIZATION, SET_COOKIE};
    (request.method == reqwest::Method::GET || request.method == reqwest::Method::HEAD)
        && request.body.is_none()
        && request.secret_headers.is_empty()
        && !request.headers.contains_key(AUTHORIZATION)
        && !request.headers.contains_key(PROXY_AUTHORIZATION)
        && !request.headers.contains_key(COOKIE)
        && !request.headers.contains_key(SET_COOKIE)
}

struct CanonicalCacheAdapter<'a> {
    executor: &'a ResolvedExecutor,
    identity_url: url::Url,
    identity_method: reqwest::Method,
    identity_headers: reqwest::header::HeaderMap,
    request: Option<CrawlerRequest>,
    transport_failure: Arc<Mutex<Option<CrawlerFailure>>>,
}

impl Middleware for CanonicalCacheAdapter<'_> {
    fn is_method_get_head(&self) -> bool {
        self.identity_method == reqwest::Method::GET
            || self.identity_method == reqwest::Method::HEAD
    }

    fn policy(&self, response: &HttpResponse) -> http_cache::Result<CachePolicy> {
        Ok(CachePolicy::new(&self.parts()?, &response.parts()?))
    }

    fn policy_with_options(
        &self,
        response: &HttpResponse,
        options: SemanticsOptions,
    ) -> http_cache::Result<CachePolicy> {
        Ok(CachePolicy::new_options(
            &self.parts()?,
            &response.parts()?,
            SystemTime::now(),
            options,
        ))
    }

    fn update_headers(&mut self, parts: &http::request::Parts) -> http_cache::Result<()> {
        self.identity_headers.extend(parts.headers.clone());
        if let Some(request) = &mut self.request {
            request.headers.extend(parts.headers.clone());
        }
        Ok(())
    }

    fn force_no_cache(&mut self) -> http_cache::Result<()> {
        self.identity_headers.insert(
            reqwest::header::CACHE_CONTROL,
            reqwest::header::HeaderValue::from_static("no-cache"),
        );
        if let Some(request) = &mut self.request {
            request.headers.insert(
                reqwest::header::CACHE_CONTROL,
                reqwest::header::HeaderValue::from_static("no-cache"),
            );
        }
        Ok(())
    }

    fn parts(&self) -> http_cache::Result<http::request::Parts> {
        let mut builder = http::Request::builder()
            .method(self.identity_method.as_str())
            .uri(self.identity_url.as_str());
        for (name, value) in &self.identity_headers {
            builder = builder.header(name, value);
        }
        Ok(builder
            .body(())
            .map_err(|error| Box::new(error) as http_cache::BoxError)?
            .into_parts()
            .0)
    }

    fn url(&self) -> http_cache::Result<http_cache::Url> {
        http_cache::Url::parse(self.identity_url.as_str())
            .map_err(|error| Box::new(error) as http_cache::BoxError)
    }

    fn method(&self) -> http_cache::Result<String> {
        Ok(self.identity_method.as_str().to_owned())
    }

    async fn remote_fetch(&mut self) -> http_cache::Result<HttpResponse> {
        let request = self.request.take().ok_or_else(|| {
            Box::new(CacheExecutionError("cache request already consumed".into()))
                as http_cache::BoxError
        })?;
        match self.executor.execute(request).await {
            Ok(response) => materialize_network_response(response).await,
            Err(failure) => {
                *self.transport_failure.lock().expect("failure lock") = Some(failure);
                Err(Box::new(TransportFailureMarker))
            }
        }
    }
}

async fn materialize_network_response(
    response: CrawlerResponse,
) -> http_cache::Result<HttpResponse> {
    let CrawlerResponse {
        status,
        headers,
        final_url,
        transport,
        mut body,
        ..
    } = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = body.next().await {
        bytes.extend_from_slice(&chunk.map_err(|error| Box::new(error) as http_cache::BoxError)?);
    }
    Ok(HttpResponse {
        body: bytes,
        headers: HttpHeaders::from(&headers),
        status: status.as_u16(),
        url: final_url,
        version: HttpVersion::Http11,
        metadata: Some(vec![match transport {
            AcquisitionTransport::Default => 0,
            AcquisitionTransport::Tor => 1,
        }]),
    })
}

fn reconstruct_response(response: HttpResponse) -> Result<CrawlerResponse, CrawlerFailure> {
    let origin = if response
        .headers
        .get("x-cache")
        .is_some_and(|value| value.eq_ignore_ascii_case("HIT"))
    {
        ResponseOrigin::ReconstructedCache
    } else {
        ResponseOrigin::Network
    };
    let backend = if origin == ResponseOrigin::ReconstructedCache {
        BackendProvenance::CacheLayer
    } else {
        BackendProvenance::Reqwest
    };
    let transport = match response.metadata.as_deref() {
        Some([1, ..]) => AcquisitionTransport::Tor,
        _ => AcquisitionTransport::Default,
    };
    let status = http::StatusCode::from_u16(response.status).map_err(|error| {
        CrawlerFailure::with_source(
            CrawlerFailureKind::ProtocolPermanent,
            BackendProvenance::CacheLayer,
            error,
        )
    })?;
    let mut headers = http::HeaderMap::new();
    for (name, value) in response.headers {
        if let (Ok(name), Ok(value)) = (
            http::header::HeaderName::from_bytes(name.as_bytes()),
            http::header::HeaderValue::from_str(&value),
        ) {
            headers.append(name, value);
        }
    }
    let body: CrawlerBodyStream =
        Box::pin(tokio_stream::once(Ok(bytes::Bytes::from(response.body))));
    Ok(CrawlerResponse {
        status,
        headers,
        final_url: response.url,
        origin,
        backend,
        transport,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn fixture() -> (url::Url, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = requests.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                observed.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut request = [0_u8; 4096];
                    let _ = stream.read(&mut request).await;
                    let response = b"HTTP/1.1 200 OK\r\nCache-Control: public, max-age=3600\r\nContent-Length: 6\r\nConnection: close\r\n\r\ncached";
                    let _ = stream.write_all(response).await;
                });
            }
        });
        (
            url::Url::parse(&format!("http://{address}/cache-test")).unwrap(),
            requests,
        )
    }

    async fn consume(mut response: CrawlerResponse) -> Vec<u8> {
        let mut bytes = Vec::new();
        while let Some(chunk) = response.body.next().await {
            bytes.extend_from_slice(&chunk.unwrap());
        }
        bytes
    }

    #[tokio::test]
    async fn miss_uses_executor_and_hit_performs_no_network() {
        let (url, requests) = fixture().await;
        let executor = ResolvedExecutor::resolve(Default::default()).unwrap();
        let cache = CanonicalCacheExecutor::new(&executor, Some("hit-proof"));

        let first = cache
            .execute(CrawlerRequest::get(url.clone()), true)
            .await
            .unwrap_or_else(|error| panic!("{error}: {:?}", error.source_ref()));
        assert_eq!(first.origin, ResponseOrigin::Network);
        assert_eq!(consume(first).await, b"cached");
        let second = cache.execute(CrawlerRequest::get(url), true).await.unwrap();
        assert_eq!(second.origin, ResponseOrigin::ReconstructedCache);
        assert_eq!(second.backend, BackendProvenance::CacheLayer);
        assert_eq!(consume(second).await, b"cached");
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn secret_headers_bypass_lookup_and_persistence() {
        let (url, requests) = fixture().await;
        let executor = ResolvedExecutor::resolve(Default::default()).unwrap();
        let cache = CanonicalCacheExecutor::new(&executor, Some("secret-proof"));
        for _ in 0..2 {
            let mut request = CrawlerRequest::get(url.clone());
            request
                .secret_headers
                .try_insert("authorization", "Bearer must-never-be-cached")
                .unwrap();
            let response = cache.execute(request, true).await.unwrap();
            assert_eq!(response.origin, ResponseOrigin::Network);
            assert_eq!(consume(response).await, b"cached");
        }
        assert_eq!(requests.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn disabled_cache_always_uses_executor() {
        let (url, requests) = fixture().await;
        let executor = ResolvedExecutor::resolve(Default::default()).unwrap();
        let cache = CanonicalCacheExecutor::new(&executor, Some("disabled-proof"));
        for _ in 0..2 {
            let response = cache
                .execute(CrawlerRequest::get(url.clone()), false)
                .await
                .unwrap();
            assert_eq!(response.origin, ResponseOrigin::Network);
            let _ = consume(response).await;
        }
        assert_eq!(requests.load(Ordering::SeqCst), 2);
    }
}
