#![cfg(all(feature = "evidence", feature = "transport_tor"))]

//! Section J: hostile-environment-proxy regression. Proves Tor
//! acquisition's `no_proxy()` call genuinely disables environment/system
//! proxy inheritance, rather than merely happening to not need it in a
//! clean test environment.
//!
//! Deliberately kept in its own test binary/process, separate from
//! `transport_tor.rs`'s other (concurrent, in-process) tests: `HTTP_PROXY`
//! / `HTTPS_PROXY` / `ALL_PROXY` are process-global, and `cargo test`
//! runs the `#[test]` functions *within* one binary concurrently on
//! multiple threads. A second test in the same process building an
//! ordinary (non-Tor) `reqwest::Client` while these vars were set could
//! spuriously inherit the hostile proxy and fail for an unrelated reason.
//! A separate binary is a separate OS process with its own environment,
//! so no other test can observe these mutations — no extra lock needed.

use spider::features::transport::{TorTransportConfig, TransportPolicy};
use spider::utils::evidence::{fetch_single_page_with_options, AcquisitionOptions};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A "hostile" proxy stand-in: just counts connections. If Tor
/// acquisition ever honored `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY`, the
/// request would route here instead of the real SOCKS fixture.
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

/// Minimal SOCKS5 fixture: accepts the greeting/CONNECT, records it, and
/// splices to a local HTTP fixture. Trimmed copy of `transport_tor.rs`'s
/// `SocksFixture` — kept local so this stays a fully independent test
/// binary with no shared-module coupling.
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

struct HttpFixture {
    addr: SocketAddr,
}

impl HttpFixture {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                tokio::spawn(async move {
                    let mut buf = [0_u8; 4096];
                    let _ = stream.read(&mut buf).await;
                    let body = b"ok";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.write_all(body).await;
                });
            }
        });
        Self { addr }
    }
}

/// RAII guard: sets the six hostile-proxy env vars on construction,
/// restores their prior state (removed, since none of them are expected
/// to be set in a clean test environment) on drop — including on panic,
/// so a test failure never leaks a poisoned `HTTP_PROXY` into whatever
/// process runs next in this same binary (there is only one test here,
/// but this is cheap insurance either way).
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

/// Tor acquisition must contact only the explicit SOCKS5h endpoint, never
/// a proxy inherited from `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` (upper or
/// lowercase) — `apply_transport_policy`'s `no_proxy()` call must
/// genuinely disable environment/system proxy inheritance, not merely
/// happen to not need it.
#[tokio::test]
async fn tor_acquisition_ignores_hostile_proxy_environment() {
    let http = HttpFixture::start().await;
    let hostile = HostileProxyFixture::start().await;
    let socks = SocksFixture::start(http.addr).await;

    let _env_guard = HostileProxyEnv::set(hostile.addr);

    let endpoint = format!("socks5h://{}", socks.addr);
    let policy = TransportPolicy::Tor(TorTransportConfig::new(&endpoint).unwrap());
    let url = "http://hostile-env-proxy-test.invalid/";

    let result =
        fetch_single_page_with_options(url, AcquisitionOptions { transport: policy }).await;

    assert!(result.is_ok(), "{result:?}");
    let acquisition = result.unwrap();
    assert_eq!(acquisition.page().status_code.as_u16(), 200);
    assert_eq!(
        socks.connect_count.load(Ordering::SeqCst),
        1,
        "the request must reach the explicit SOCKS endpoint"
    );
    assert_eq!(
        hostile.hits.load(Ordering::SeqCst),
        0,
        "the hostile HTTP_PROXY/HTTPS_PROXY/ALL_PROXY environment must never be contacted"
    );
}
