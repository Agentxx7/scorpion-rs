#![cfg(all(feature = "evidence", feature = "transport_tor", feature = "sitemap"))]

//! T11 (crawl-level counterpart to `transport_tor_env_proxy.rs`'s one-shot
//! proof): a **multi-page** Tor crawl must ignore `HTTP_PROXY` /
//! `HTTPS_PROXY` / `ALL_PROXY` for every request it makes — seed and
//! discovered children alike — not just a single one-shot fetch.
//!
//! Deliberately kept in its own test binary/process, for the same reason
//! as `transport_tor_env_proxy.rs`: `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY`
//! are process-global, and a second test in the same process building an
//! ordinary (non-Tor) client while these vars are set could spuriously
//! inherit the hostile proxy and fail for an unrelated reason. A separate
//! binary is a separate OS process with its own environment, so no other
//! test can observe these mutations.

use spider::features::transport::{TorTransportConfig, TransportPolicy};
use spider::website::Website;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A "hostile" proxy stand-in: just counts connections. If Tor crawl
/// acquisition ever honored `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY`, any of
/// the crawl's requests would route here instead of the real SOCKS
/// fixture.
struct HostileProxyFixture {
    addr: SocketAddr,
    hits: Arc<AtomicUsize>,
}

impl HostileProxyFixture {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_clone = hits.clone();
        tokio::spawn(async move {
            loop {
                let (_stream, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                hits_clone.fetch_add(1, Ordering::SeqCst);
            }
        });
        Self { addr, hits }
    }
}

/// Minimal SOCKS5 fixture: accepts the greeting/CONNECT, records a hit
/// count, and splices to a local HTTP fixture. Trimmed copy of the shared
/// pattern used across this frontier's other test binaries — kept local
/// so this stays a fully independent process with no shared-module
/// coupling.
struct SocksFixture {
    addr: SocketAddr,
    connect_count: Arc<AtomicUsize>,
}

impl SocksFixture {
    async fn start(splice_to: SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let connect_count = Arc::new(AtomicUsize::new(0));
        let connect_count_clone = connect_count.clone();

        tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                let connect_count = connect_count_clone.clone();
                tokio::spawn(async move {
                    let _ = serve_one(stream, splice_to, connect_count).await;
                });
            }
        });

        Self {
            addr,
            connect_count,
        }
    }
}

async fn serve_one(
    mut stream: TcpStream,
    splice_to: SocketAddr,
    connect_count: Arc<AtomicUsize>,
) -> std::io::Result<()> {
    let mut header = [0_u8; 2];
    stream.read_exact(&mut header).await?;
    let nmethods = header[1] as usize;
    let mut methods = vec![0_u8; nmethods];
    stream.read_exact(&mut methods).await?;
    stream.write_all(&[0x05, 0x00]).await?;

    let mut req_head = [0_u8; 4];
    stream.read_exact(&mut req_head).await?;
    match req_head[3] {
        0x01 => {
            let mut addr = [0_u8; 4];
            stream.read_exact(&mut addr).await?;
        }
        0x03 => {
            let mut len_buf = [0_u8; 1];
            stream.read_exact(&mut len_buf).await?;
            let mut name = vec![0_u8; len_buf[0] as usize];
            stream.read_exact(&mut name).await?;
        }
        0x04 => {
            let mut addr = [0_u8; 16];
            stream.read_exact(&mut addr).await?;
        }
        _ => return Ok(()),
    }
    let mut port_buf = [0_u8; 2];
    stream.read_exact(&mut port_buf).await?;

    connect_count.fetch_add(1, Ordering::SeqCst);

    stream
        .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await?;

    let mut upstream = TcpStream::connect(splice_to).await?;
    let (mut ri, mut wi) = stream.split();
    let (mut ro, mut wo) = upstream.split();
    let client_to_up = tokio::io::copy(&mut ri, &mut wo);
    let up_to_client = tokio::io::copy(&mut ro, &mut wi);
    let _ = tokio::try_join!(client_to_up, up_to_client);
    Ok(())
}

/// Tiny multi-page local HTTP fixture, matching `transport_tor_crawl.rs`'s
/// route-table shape (trimmed to what this test needs).
struct HttpFixture {
    addr: SocketAddr,
    hits: Arc<Mutex<HashMap<String, usize>>>,
}

impl HttpFixture {
    async fn start(routes: HashMap<&'static str, &'static str>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(Mutex::new(HashMap::new()));
        let hits_clone = hits.clone();
        let routes: HashMap<String, String> = routes
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        tokio::spawn(async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                let hits = hits_clone.clone();
                let routes = routes.clone();
                tokio::spawn(async move {
                    let mut buf = [0_u8; 8192];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..n]);
                    let path = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .to_string();
                    *hits.lock().unwrap().entry(path.clone()).or_insert(0) += 1;

                    let response = match routes.get(&path) {
                        Some(body) => format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        ),
                        None => {
                            let body = "not found";
                            format!(
                                "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            )
                        }
                    };
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });

        Self { addr, hits }
    }

    fn total_hits(&self) -> usize {
        self.hits.lock().unwrap().values().sum()
    }
}

/// RAII guard: sets the six hostile-proxy env vars on construction,
/// restores their prior state (removed, since none of them are expected
/// to be set in a clean test environment) on drop — including on panic.
struct HostileProxyEnv;

const PROXY_VARS: [&str; 6] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];

impl HostileProxyEnv {
    fn set(hostile_addr: SocketAddr) -> Self {
        let value = format!("http://{hostile_addr}");
        for var in PROXY_VARS {
            std::env::set_var(var, &value);
        }
        Self
    }
}

impl Drop for HostileProxyEnv {
    fn drop(&mut self) {
        for var in PROXY_VARS {
            std::env::remove_var(var);
        }
    }
}

/// T11: every request in a **multi-page** Tor crawl (seed + discovered
/// children) must contact only the explicit SOCKS5h endpoint, never a
/// proxy inherited from `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` — proving
/// the crawl-level `CrawlHttpContext`'s `no_proxy()` call genuinely
/// disables environment/system proxy inheritance for every worker, not
/// merely for a single one-shot request.
#[tokio::test]
async fn tor_crawl_ignores_hostile_proxy_environment() {
    let mut routes = HashMap::new();
    routes.insert("/", r#"<html><body><a href="/page2">two</a></body></html>"#);
    routes.insert("/page2", "<html><body>two</body></html>");

    let http = HttpFixture::start(routes).await;
    let hostile = HostileProxyFixture::start().await;
    let socks = SocksFixture::start(http.addr).await;

    let _env_guard = HostileProxyEnv::set(hostile.addr);

    let seed_host = "hostile-env-proxy-crawl-test.invalid";
    let url = format!("http://{seed_host}:{}/", http.addr.port());

    let mut website = Website::new(&url);
    website.with_transport(TransportPolicy::Tor(
        TorTransportConfig::new(&format!("socks5h://{}", socks.addr)).unwrap(),
    ));
    website.with_ignore_sitemap(true);
    website.with_limit(10);
    website.crawl_raw().await;

    assert_eq!(website.get_links().len(), 2, "seed + /page2");
    assert_eq!(
        socks.connect_count.load(Ordering::SeqCst),
        2,
        "both requests must reach the explicit SOCKS endpoint"
    );
    assert_eq!(
        hostile.hits.load(Ordering::SeqCst),
        0,
        "the hostile HTTP_PROXY/HTTPS_PROXY/ALL_PROXY environment must never be contacted"
    );
    assert_eq!(http.total_hits(), 2);
}
