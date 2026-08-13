//! Shared acceptance contract for canonical Wreq transport convergence.

use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn read(path: &str) -> String {
    fs::read_to_string(root().join(path)).unwrap()
}

#[test]
fn leaf_owns_the_resolved_wreq_executor_and_neutral_contract() {
    let transport = read("spider_transport/src/transport.rs");
    let outcome = read("spider_transport/src/crawler_outcome.rs");
    assert!(transport.contains("pub struct ResolvedWreqExecutor"));
    assert!(transport.contains("pub async fn execute"));
    assert!(transport.contains("validate_target(&request.url"));
    assert!(transport.contains("request.secret_headers.apply_to"));
    assert!(transport.contains("TransportError::InvalidProxy"));
    assert!(transport.contains("BackendProvenance::Wreq"));
    assert!(outcome.contains("BackendProvenance::Wreq"));
    assert!(outcome.contains("ResponseOrigin::Network"));
    assert!(!transport.contains("pub fn client(&self)"));
}

#[test]
fn website_resolves_canonical_wreq_before_execution() {
    let website = read("spider/src/website.rs");
    assert!(website.contains("ExecutionMode::CanonicalWreq"));
    assert!(website.contains("resolve_wreq_executor"));
    assert!(website.contains("resolved_executor"));
}

#[test]
fn canonical_wreq_uses_page_request_only_entrypoint() {
    let page = read("spider/src/page.rs");
    assert!(page.contains("new_page_with_executor"));
    assert!(page.contains("fetch_page_html_with_executor"));
    assert!(page.contains("UPSTREAM_COMPATIBILITY_BOUNDARY"));
}

#[test]
fn canonical_website_does_not_select_raw_wreq_transport() {
    let website = read("spider/src/website.rs");
    let canonical = website.split("fn resolve_wreq_executor").nth(1).unwrap();
    let canonical = canonical
        .split("pub fn configure_base_client")
        .next()
        .unwrap();
    assert!(!canonical.contains("ClientBuilder::new"));
    assert!(!canonical.contains(".send()"));
    assert!(!canonical.contains("if let Ok(proxy)"));
}

#[test]
fn tls_dns_interface_cookie_and_emulation_are_resolved_in_leaf() {
    let transport = read("spider_transport/src/transport.rs");
    for required in [
        "cert_verification(!config.accept_invalid_certs)",
        "verify_hostname(!config.accept_invalid_certs)",
        "dns_resolver",
        "network_interface",
        "local_address",
        "cookie_jar",
        "emulation",
    ] {
        assert!(transport.contains(required), "missing {required}");
    }
}

#[test]
fn invalid_proxy_and_redirect_security_are_canonical() {
    let transport = read("spider_transport/src/transport.rs");
    assert!(transport.contains("canonical_redirect_decision"));
    assert!(transport.contains("wreq::Proxy::all(endpoint)"));
    assert!(!transport.contains("filter_map(|proxy|"));
}

#[test]
fn compatibility_tor_cache_and_gemini_boundaries_remain() {
    let page = read("spider/src/page.rs");
    let website = read("spider/src/website.rs");
    let lib = read("spider/src/lib.rs");
    let evidence = read("spider/src/utils/evidence.rs");
    let gemini = read("spider/src/features/solvers.rs");
    assert!(page.contains("UPSTREAM_COMPATIBILITY_BOUNDARY"));
    assert!(website.contains("UPSTREAM_COMPATIBILITY_BOUNDARY"));
    assert!(lib.contains("cache_request + wreq is explicitly rejected"));
    assert!(evidence.contains("requires a build without the wreq feature"));
    assert!(gemini.contains("static ref GEMINI_EXECUTOR: CanonicalExecutor"));
    assert!(!gemini.contains("CAPABILITY_LOCAL_NONCANONICAL_WREQ"));
}

#[test]
fn synthetic_reintroduction_is_detected() {
    fn violation(source: &str) -> bool {
        [
            "Website { wreq_client:",
            "ClientBuilder::new().send()",
            "if let Ok(proxy)",
            "canonical_page.error = wreq_error",
        ]
        .iter()
        .any(|pattern| source.contains(pattern))
    }
    for fixture in [
        "Website { wreq_client: client }",
        "ClientBuilder::new().send()",
        "if let Ok(proxy) { direct() }",
        "canonical_page.error = wreq_error",
    ] {
        assert!(violation(fixture));
    }
    assert!(!violation("executor.execute(request).await"));
}

#[test]
fn sdd_preserves_closed_baseline_and_design_boundary() {
    let sdd = read("docs/frontier/WREQ_SECURITY_AND_TRANSPORT_CONVERGENCE_SDD.md");
    assert!(sdd.contains("588b90581eef9e518eb3092ec9b7ada9d986fb65"));
    assert!(sdd.contains("TWO_BRANCH_NOT_APPLICABLE"));
    assert!(sdd.contains("CAPABILITY_LOCAL_NONCANONICAL_WREQ"));
}

#[cfg(feature = "wreq")]
fn wreq_config() -> spider_transport::WreqTransportConfiguration {
    spider_transport::WreqTransportConfiguration {
        policy: spider_transport::TransportPolicy::Default,
        user_agent: "wreq-convergence-test".into(),
        default_headers: wreq::header::HeaderMap::new(),
        proxies: Vec::new(),
        request_timeout: std::time::Duration::from_secs(5),
        connect_timeout: std::time::Duration::from_secs(5),
        read_timeout: std::time::Duration::from_secs(5),
        accept_invalid_certs: false,
        local_address: None,
        network_interface: None,
        dns_resolver: None,
        cookie_jar: Some(std::sync::Arc::new(wreq::cookie::Jar::default())),
        emulation: None,
        redirect_limit: 10,
        redirect_mode: spider_transport::WreqRedirectMode::Follow,
    }
}

#[cfg(feature = "wreq")]
#[test]
fn invalid_proxy_resolution_fails_closed() {
    let mut config = wreq_config();
    config.proxies.push("not a proxy URL".into());
    let error = spider_transport::ResolvedWreqExecutor::resolve(config).unwrap_err();
    assert!(matches!(
        error,
        spider_transport::TransportError::InvalidProxy(_)
    ));
}

#[cfg(feature = "wreq")]
#[tokio::test]
async fn website_invalid_proxy_stops_before_target_network() {
    use spider::configuration::{ProxyIgnore, RequestProxy};
    let mut website = spider::website::Website::new("http://127.0.0.1:9/");
    website.configuration.proxies = Some(vec![RequestProxy {
        addr: "not a proxy URL".into(),
        ignore: ProxyIgnore::No,
    }]);
    website.crawl_raw().await;
    assert!(website.last_transport_error().is_some());
    assert_eq!(*website.get_status(), spider::website::CrawlStatus::Invalid);
}

#[cfg(feature = "wreq")]
#[tokio::test]
async fn onion_target_is_rejected_before_network() {
    let executor = spider_transport::ResolvedWreqExecutor::resolve(wreq_config()).unwrap();
    let request = spider_transport::CrawlerRequest::get(
        url::Url::parse("http://canonical-validation-test.onion/").unwrap(),
    );
    let failure = executor.execute(request).await.unwrap_err();
    assert_eq!(failure.backend(), spider_transport::BackendProvenance::Wreq);
    assert_eq!(
        failure.kind(),
        spider_transport::CrawlerFailureKind::Request
    );
}

#[cfg(feature = "wreq")]
#[tokio::test]
async fn redirect_ssrf_is_blocked_by_canonical_policy() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 1024];
        let _ = socket.read(&mut request).await;
        socket
            .write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
    });
    let executor = spider_transport::ResolvedWreqExecutor::resolve(wreq_config()).unwrap();
    let request = spider_transport::CrawlerRequest::get(
        url::Url::parse(&format!("http://{address}/redirect")).unwrap(),
    );
    let failure = executor.execute(request).await.unwrap_err();
    assert_eq!(failure.backend(), spider_transport::BackendProvenance::Wreq);
}

#[cfg(feature = "wreq")]
#[tokio::test]
async fn secret_headers_reach_wreq_wire_without_entering_provenance() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = vec![0; 4096];
        let read = socket.read(&mut bytes).await.unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes[..read]).into_owned()
    });
    let executor = spider_transport::ResolvedWreqExecutor::resolve(wreq_config()).unwrap();
    let mut request = spider_transport::CrawlerRequest::get(
        url::Url::parse(&format!("http://{address}/")).unwrap(),
    );
    request
        .secret_headers
        .try_insert("authorization", "Bearer wreq-secret")
        .unwrap();
    let response = executor.execute(request).await.unwrap();
    assert_eq!(response.backend, spider_transport::BackendProvenance::Wreq);
    assert_eq!(response.origin, spider_transport::ResponseOrigin::Network);
    let wire = server.await.unwrap();
    assert!(wire
        .to_ascii_lowercase()
        .contains("authorization: bearer wreq-secret"));
}

#[cfg(feature = "wreq")]
#[tokio::test]
async fn persistent_executor_preserves_cookie_session() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut second = String::new();
        for attempt in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = vec![0; 4096];
            let read = socket.read(&mut bytes).await.unwrap();
            if attempt == 1 {
                second = String::from_utf8_lossy(&bytes[..read]).into_owned();
            }
            let response = if attempt == 0 {
                b"HTTP/1.1 200 OK\r\nSet-Cookie: session=canonical; Path=/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".as_slice()
            } else {
                b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".as_slice()
            };
            socket.write_all(response).await.unwrap();
        }
        second
    });
    let executor = spider_transport::ResolvedWreqExecutor::resolve(wreq_config()).unwrap();
    let url = url::Url::parse(&format!("http://{address}/")).unwrap();
    executor
        .execute(spider_transport::CrawlerRequest::get(url.clone()))
        .await
        .unwrap();
    executor
        .execute(spider_transport::CrawlerRequest::get(url))
        .await
        .unwrap();
    let second = server.await.unwrap().to_ascii_lowercase();
    assert!(second.contains("cookie: session=canonical"));
}

#[cfg(feature = "wreq")]
#[tokio::test]
async fn proxy_rotation_is_private_and_round_robin() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    async fn proxy(label: &'static str) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 2048];
            let _ = socket.read(&mut request).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\nX-Proxy: {label}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        (address, task)
    }
    let (first, first_task) = proxy("first").await;
    let (second, second_task) = proxy("second").await;
    let mut config = wreq_config();
    config.proxies = vec![format!("http://{first}"), format!("http://{second}")];
    let executor = spider_transport::ResolvedWreqExecutor::resolve(config).unwrap();
    let url = url::Url::parse("http://example.invalid/resource").unwrap();
    let first_response = executor
        .execute(spider_transport::CrawlerRequest::get(url.clone()))
        .await
        .unwrap();
    let second_response = executor
        .execute(spider_transport::CrawlerRequest::get(url))
        .await
        .unwrap();
    assert_eq!(first_response.headers["x-proxy"], "first");
    assert_eq!(second_response.headers["x-proxy"], "second");
    first_task.await.unwrap();
    second_task.await.unwrap();
}
