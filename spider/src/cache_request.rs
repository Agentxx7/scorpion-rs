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

/// A response carrying `Set-Cookie` fails closed for cache persistence: no
/// canonical policy in this codebase proves a `Set-Cookie`-bearing response
/// safe to persist (see `cacheable_request`, which already rejects the
/// request-side equivalent — `Authorization`/`Cookie`/`Proxy-Authorization`/
/// `Set-Cookie` on the *request* — but has no visibility into headers the
/// origin only sends back on the *response*). `CachePolicy::is_storable`
/// (consulted by `http-cache`'s `should_cache_response` on the real write
/// path, `HttpCache::run` -> `remote_fetch` -> `Middleware::policy`) treats a
/// `Cache-Control: no-store` response directive as an absolute veto ahead of
/// every other storability rule, so forcing it here — before the response
/// parts are ever handed to `CachePolicy::new`/`new_options` — is sufficient
/// to keep a `Set-Cookie`-bearing response out of local persistence without
/// touching request-side classification, cache identity, or any other
/// storability rule for ordinary, cookie-free responses.
fn fail_closed_on_set_cookie(mut parts: http::response::Parts) -> http::response::Parts {
    if parts.headers.contains_key(reqwest::header::SET_COOKIE) {
        parts.headers.insert(
            http::header::CACHE_CONTROL,
            http::header::HeaderValue::from_static("no-store"),
        );
    }
    parts
}

impl Middleware for CanonicalCacheAdapter<'_> {
    fn is_method_get_head(&self) -> bool {
        self.identity_method == reqwest::Method::GET
            || self.identity_method == reqwest::Method::HEAD
    }

    fn policy(&self, response: &HttpResponse) -> http_cache::Result<CachePolicy> {
        Ok(CachePolicy::new(
            &self.parts()?,
            &fail_closed_on_set_cookie(response.parts()?),
        ))
    }

    fn policy_with_options(
        &self,
        response: &HttpResponse,
        options: SemanticsOptions,
    ) -> http_cache::Result<CachePolicy> {
        Ok(CachePolicy::new_options(
            &self.parts()?,
            &fail_closed_on_set_cookie(response.parts()?),
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
        mut headers,
        final_url,
        transport,
        mut body,
        ..
    } = response;
    // A 304 Not Modified response carries no body of its own — `http-cache`'s
    // revalidation merge (`CachePolicy::after_response`, invoked directly on
    // the already-stored policy, never through `Middleware::policy`) folds
    // its headers into the persisted entry and then unconditionally
    // re-persists via `CacheManager::put`, with no `is_storable`/
    // `should_cache_response` check in that branch at all — so the
    // `fail_closed_on_set_cookie` veto in `policy`/`policy_with_options`
    // never gets a chance to run for this specific write. When the 304
    // omits (or mismatches) validators, `after_response` folds the raw 304
    // headers in verbatim, which would let an origin smuggle a fresh
    // `Set-Cookie` straight into the unconditional re-persist. Since the
    // originally-cached entry is (by the same invariant enforced in
    // `policy`/`policy_with_options`) guaranteed to have never carried
    // `Set-Cookie` itself, and a 304 has no body a caller could need
    // alongside a fresh cookie, strip `Set-Cookie` from 304 responses here
    // — before they ever reach that merge — so there is nothing left to
    // smuggle in. Ordinary (200) responses are untouched: their
    // `Set-Cookie` still reaches the caller and is still kept out of the
    // cache by the `policy`/`policy_with_options` veto.
    if status.as_u16() == 304 {
        headers.remove(reqwest::header::SET_COOKIE);
    }
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
        fixture_with_response(
            b"HTTP/1.1 200 OK\r\nCache-Control: public, max-age=3600\r\nContent-Length: 6\r\nConnection: close\r\n\r\ncached",
        )
        .await
    }

    async fn fixture_with_response(response: &'static [u8]) -> (url::Url, Arc<AtomicUsize>) {
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
                    let _ = stream.write_all(response).await;
                });
            }
        });
        (
            url::Url::parse(&format!("http://{address}/cache-test")).unwrap(),
            requests,
        )
    }

    /// Serves `responses` in order, one per accepted connection; once
    /// exhausted, every further connection repeats the last response.
    async fn fixture_sequenced(
        responses: &'static [&'static [u8]],
    ) -> (url::Url, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = requests.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let index = observed.fetch_add(1, Ordering::SeqCst);
                let response = responses[index.min(responses.len() - 1)];
                tokio::spawn(async move {
                    let mut request = [0_u8; 4096];
                    let _ = stream.read(&mut request).await;
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
    async fn set_cookie_response_is_never_persisted_or_served_from_cache() {
        // Requirement: "A response carrying Set-Cookie must fail closed for
        // cache persistence unless there is already an explicit canonical
        // policy that proves the response safe to persist." No such policy
        // exists, so an otherwise-cacheable (Cache-Control: public,
        // max-age=3600) response that also carries Set-Cookie must never be
        // served from the local cache — every request in this test must
        // reach the network, first request and every subsequent lookup
        // alike.
        let (url, requests) = fixture_with_response(
            b"HTTP/1.1 200 OK\r\nCache-Control: public, max-age=3600\r\nSet-Cookie: session=must-never-be-cached\r\nContent-Length: 6\r\nConnection: close\r\n\r\ncached",
        )
        .await;
        let executor = ResolvedExecutor::resolve(Default::default()).unwrap();
        let cache = CanonicalCacheExecutor::new(&executor, Some("set-cookie-proof"));

        for iteration in 0..3 {
            let response = cache
                .execute(CrawlerRequest::get(url.clone()), true)
                .await
                .unwrap_or_else(|error| panic!("{error}: {:?}", error.source_ref()));
            // (a) the first request's Set-Cookie-bearing response is not
            // persisted, and (b) no subsequent lookup can retrieve it from
            // persistent cache — both proven by every iteration, including
            // the first, always originating from the network.
            assert_eq!(
                response.origin,
                ResponseOrigin::Network,
                "iteration {iteration} unexpectedly served from cache"
            );
            // Wire-truth boundary check: the synthetic `Cache-Control:
            // no-store` that fail_closed_on_set_cookie injects is
            // policy-local — built from a fresh `response.parts()?` clone,
            // never mutating `response`/`cond_res` itself — so the actual
            // Set-Cookie value the origin sent must still reach the
            // caller on every one of these Network-origin exchanges. Only
            // its persistence is suppressed, never its truthful return.
            assert_eq!(
                response
                    .headers
                    .get(reqwest::header::SET_COOKIE)
                    .map(|value| value.to_str().unwrap()),
                Some("session=must-never-be-cached"),
                "iteration {iteration}: the real Set-Cookie value must still reach the \
                 caller — only its persistence is suppressed, not its truthful return"
            );
            assert_eq!(consume(response).await, b"cached");
        }
        assert_eq!(requests.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn set_cookie_on_a_304_revalidation_response_is_never_persisted() {
        // A deeper variant of the same invariant, found during this fix's
        // own adversarial verification: `http-cache`'s 304/revalidation
        // merge (`CachePolicy::after_response`) runs entirely outside the
        // `Middleware::policy` hook and unconditionally re-persists its
        // result — so a validator-less (or validator-mismatched) 304 could
        // smuggle a fresh Set-Cookie into the cache even though the
        // fresh-write path above is fixed. First response is a normal,
        // Set-Cookie-free 200 with an ETag and `max-age=0` (storable, but
        // immediately stale, forcing revalidation on the very next
        // lookup). The revalidation response is a bare `304` — no ETag
        // echoed back, so http-cache's own validator-matching logic
        // resolves to "does not match" — carrying `Set-Cookie` and a fresh
        // `max-age=3600`. A third request must then be served from cache
        // (proving the revalidated entry is otherwise cacheable/fresh, not
        // that caching broke entirely) with no `Set-Cookie` header on it,
        // and must not have touched the network again.
        let (url, requests) = fixture_sequenced(&[
            b"HTTP/1.1 200 OK\r\nETag: \"v1\"\r\nCache-Control: max-age=0\r\nContent-Length: 6\r\nConnection: close\r\n\r\ncached",
            b"HTTP/1.1 304 Not Modified\r\nSet-Cookie: session=must-never-be-cached\r\nCache-Control: max-age=3600\r\nConnection: close\r\n\r\n",
        ])
        .await;
        let executor = ResolvedExecutor::resolve(Default::default()).unwrap();
        let cache = CanonicalCacheExecutor::new(&executor, Some("set-cookie-304-proof"));

        let first = cache
            .execute(CrawlerRequest::get(url.clone()), true)
            .await
            .unwrap_or_else(|error| panic!("{error}: {:?}", error.source_ref()));
        assert_eq!(first.origin, ResponseOrigin::Network);
        assert!(!first.headers.contains_key(reqwest::header::SET_COOKIE));
        assert_eq!(consume(first).await, b"cached");

        // Every further call revalidates (the persisted entry is stale at
        // max-age=0, and — a quirk of `after_response` carrying the raw
        // 304 status into the merged policy when validators don't match —
        // status 304 is not among http-cache-semantics's
        // `UNDERSTOOD_STATUSES`, so the merged entry is never considered
        // storable/fresh either, forcing revalidation again next time).
        // That repetition is exactly what makes this a strong regression
        // check: the bare 304 carrying Set-Cookie is replayed on every
        // iteration, and it must never once leak into the returned
        // response or the re-persisted entry.
        for iteration in 0..3 {
            let response = cache
                .execute(CrawlerRequest::get(url.clone()), true)
                .await
                .unwrap_or_else(|error| panic!("{error}: {:?}", error.source_ref()));
            assert!(
                !response.headers.contains_key(reqwest::header::SET_COOKIE),
                "iteration {iteration}: a Set-Cookie header from a 304 revalidation \
                 response must never reach the re-persisted cache entry or the caller"
            );
            // Wire-truth boundary check: Scorpion's own provenance tagging
            // must never claim this exchange was a fresh, first-hand
            // `Network` observation of the 304's real headers — a 304
            // revalidation is, and is labeled, a cache reconstruction
            // (`x-cache: HIT`, set by http-cache itself). The Set-Cookie
            // suppression above only ever applies to an exchange already
            // tagged as cache-origin, never to a response Scorpion presents
            // as an untouched live network capture.
            assert_eq!(
                response.origin,
                ResponseOrigin::ReconstructedCache,
                "iteration {iteration}: a 304-revalidated exchange must be tagged as a \
                 cache reconstruction, not a live Network observation — this is the \
                 basis for stripping Set-Cookie here without falsifying wire truth"
            );
            assert_eq!(consume(response).await, b"cached");
        }

        assert_eq!(requests.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn response_without_set_cookie_still_caches_normally() {
        // Non-regression: an ordinary, credential-free, Set-Cookie-free
        // cacheable response must continue to be served from the local
        // cache on repeat lookups, unaffected by the Set-Cookie fail-closed
        // guard.
        let (url, requests) = fixture().await;
        let executor = ResolvedExecutor::resolve(Default::default()).unwrap();
        let cache = CanonicalCacheExecutor::new(&executor, Some("no-set-cookie-proof"));

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
    async fn request_side_cookie_and_proxy_authorization_headers_still_bypass_cache() {
        // Non-regression: existing request-side Authorization/Cookie/
        // Proxy-Authorization rejection in `cacheable_request` remains
        // intact after the response-side Set-Cookie fix — a plain (not
        // `secret_headers`) Cookie or Proxy-Authorization request header
        // must still force every request to the network, never touching the
        // cache.
        use reqwest::header::{COOKIE, PROXY_AUTHORIZATION};

        let (url, requests) = fixture().await;
        let executor = ResolvedExecutor::resolve(Default::default()).unwrap();
        let cache = CanonicalCacheExecutor::new(&executor, Some("request-header-proof"));

        for _ in 0..2 {
            let mut request = CrawlerRequest::get(url.clone());
            request
                .headers
                .insert(COOKIE, "session=must-never-be-cached".parse().unwrap());
            let response = cache.execute(request, true).await.unwrap();
            assert_eq!(response.origin, ResponseOrigin::Network);
            let _ = consume(response).await;
        }

        for _ in 0..2 {
            let mut request = CrawlerRequest::get(url.clone());
            request.headers.insert(
                PROXY_AUTHORIZATION,
                "Basic must-never-be-cached".parse().unwrap(),
            );
            let response = cache.execute(request, true).await.unwrap();
            assert_eq!(response.origin, ResponseOrigin::Network);
            let _ = consume(response).await;
        }

        assert_eq!(requests.load(Ordering::SeqCst), 4);
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
