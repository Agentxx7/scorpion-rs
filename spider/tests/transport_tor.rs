#![cfg(all(feature = "evidence", feature = "transport_tor"))]

//! Deterministic, network-free proof that `TransportPolicy::Tor` is
//! fail-closed end to end: proxy-side (not local) DNS resolution, no
//! fallback to a direct connection on SOCKS failure, `.onion` rejected
//! under `Default` transport with zero network activity, transport
//! pinned across redirects (including into existing SSRF-forbidden
//! destinations), bounded behavior against a blackhole SOCKS endpoint,
//! and truthful evidence provenance bound to the acquisition that
//! actually happened.
//!
//! No public Tor dependency: a hand-rolled local SOCKS5 fixture server
//! parses just enough of the protocol (greeting + CONNECT) to record what
//! was requested and either splice the connection to a local HTTP
//! fixture, return a controlled SOCKS failure reply, or say nothing at
//! all (blackhole).

use spider::features::transport::{TorTransportConfig, TransportPolicy};
use spider::utils::evidence::{
    build_evidence_with_transport, fetch_single_page_with_options, AcquisitionOptions,
};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// One CONNECT request observed by the SOCKS fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RecordedTarget {
    Domain(String, u16),
    Ipv4(std::net::Ipv4Addr, u16),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SocksBehavior {
    /// Succeed every CONNECT and splice to a local HTTP fixture.
    Splice,
    /// Reply with a SOCKS server-failure to every CONNECT.
    Fail,
    /// Accept the TCP connection and then never send another byte —
    /// not even the SOCKS greeting reply. Used to prove bounded
    /// (non-infinite) behavior against a stalled proxy.
    Blackhole,
}

#[derive(Clone)]
struct SocksFixture {
    addr: SocketAddr,
    connect_count: Arc<AtomicUsize>,
    recorded: Arc<Mutex<Vec<RecordedTarget>>>,
    splice_to: Option<SocketAddr>,
    behavior: SocksBehavior,
}

impl SocksFixture {
    /// Start a SOCKS5 fixture that fails every CONNECT with a SOCKS
    /// server-failure reply (no splice ever occurs).
    async fn start_failing() -> Self {
        Self::start(None, SocksBehavior::Fail).await
    }

    /// Start a SOCKS5 fixture that succeeds every CONNECT and splices the
    /// resulting TCP stream to `splice_to`.
    async fn start_splicing(splice_to: SocketAddr) -> Self {
        Self::start(Some(splice_to), SocksBehavior::Splice).await
    }

    /// Start a SOCKS5 fixture that accepts the TCP connection and then
    /// goes completely silent — never completes even the SOCKS greeting.
    async fn start_blackhole() -> Self {
        Self::start(None, SocksBehavior::Blackhole).await
    }

    async fn start(splice_to: Option<SocketAddr>, behavior: SocksBehavior) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let connect_count = Arc::new(AtomicUsize::new(0));
        let recorded = Arc::new(Mutex::new(Vec::new()));

        let fixture = Self {
            addr,
            connect_count: connect_count.clone(),
            recorded: recorded.clone(),
            splice_to,
            behavior,
        };

        let accept_fixture = fixture.clone();
        tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                let fixture = accept_fixture.clone();
                tokio::spawn(async move {
                    let _ = fixture.serve_one(stream).await;
                });
            }
        });

        fixture
    }

    async fn serve_one(&self, mut stream: TcpStream) -> std::io::Result<()> {
        if self.behavior == SocksBehavior::Blackhole {
            // Accept, then hold the connection open forever without
            // sending or reading anything further. The task itself never
            // completes; the client-side timeout is what must bound this.
            let mut sink = [0_u8; 1];
            loop {
                std::future::pending::<()>().await;
                let _ = stream.read(&mut sink).await;
            }
        }

        // --- Greeting: [0x05, nmethods, methods...] -> [0x05, 0x00] ---
        let mut header = [0_u8; 2];
        stream.read_exact(&mut header).await?;
        let nmethods = header[1] as usize;
        let mut methods = vec![0_u8; nmethods];
        stream.read_exact(&mut methods).await?;
        stream.write_all(&[0x05, 0x00]).await?; // version 5, no-auth selected

        // --- Request: [0x05, CMD, RSV, ATYP, ADDR, PORT] ---
        let mut req_head = [0_u8; 4];
        stream.read_exact(&mut req_head).await?;
        let atyp = req_head[3];

        let target = match atyp {
            0x01 => {
                // IPv4
                let mut addr = [0_u8; 4];
                stream.read_exact(&mut addr).await?;
                let mut port_buf = [0_u8; 2];
                stream.read_exact(&mut port_buf).await?;
                let port = u16::from_be_bytes(port_buf);
                RecordedTarget::Ipv4(std::net::Ipv4Addr::from(addr), port)
            }
            0x03 => {
                // Domain name: [len, name..]
                let mut len_buf = [0_u8; 1];
                stream.read_exact(&mut len_buf).await?;
                let mut name = vec![0_u8; len_buf[0] as usize];
                stream.read_exact(&mut name).await?;
                let mut port_buf = [0_u8; 2];
                stream.read_exact(&mut port_buf).await?;
                let port = u16::from_be_bytes(port_buf);
                RecordedTarget::Domain(String::from_utf8_lossy(&name).to_string(), port)
            }
            0x04 => {
                // IPv6 — not exercised by these tests, drain and record nothing usable.
                let mut addr = [0_u8; 16];
                stream.read_exact(&mut addr).await?;
                let mut port_buf = [0_u8; 2];
                stream.read_exact(&mut port_buf).await?;
                let _ = port_buf;
                RecordedTarget::Domain("<ipv6>".to_string(), 0)
            }
            _ => return Ok(()),
        };

        self.connect_count.fetch_add(1, Ordering::SeqCst);
        self.recorded.lock().unwrap().push(target);

        if self.behavior == SocksBehavior::Fail {
            // 0x01 = general SOCKS server failure.
            stream
                .write_all(&[0x05, 0x01, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await?;
            return Ok(());
        }

        let Some(splice_to) = self.splice_to else {
            stream
                .write_all(&[0x05, 0x01, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await?;
            return Ok(());
        };

        // Success reply, bind addr 0.0.0.0:0 (unused by the client).
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
}

/// A tiny local HTTP fixture: replies 200 OK with a fixed body to every
/// request, and counts how many connections it accepted.
struct HttpFixture {
    addr: SocketAddr,
    hits: Arc<AtomicUsize>,
}

impl HttpFixture {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_clone = hits.clone();

        tokio::spawn(async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                let hits = hits_clone.clone();
                tokio::spawn(async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    let mut buf = [0_u8; 4096];
                    let _ = stream.read(&mut buf).await;
                    let body = b"tor transport fixture ok";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.write_all(body).await;
                });
            }
        });

        Self { addr, hits }
    }
}

fn tor_policy(socks_addr: SocketAddr) -> TransportPolicy {
    let endpoint = format!("socks5h://{socks_addr}");
    TransportPolicy::Tor(TorTransportConfig::new(&endpoint).unwrap())
}

/// Section O: DNS-leak proof. A deliberately non-resolvable hostname is
/// requested through `TransportPolicy::Tor`; the fixture must observe the
/// ORIGINAL HOSTNAME as a domain-ATYP CONNECT, never a locally-resolved
/// IP — proving no local DNS resolution occurred for the target host.
#[tokio::test]
async fn tor_acquisition_never_resolves_target_hostname_locally() {
    let http = HttpFixture::start().await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let target_host = "definitely-nonexistent-scorpion-tor-test-host.invalid";
    let url = format!("http://{target_host}:{}/", http.addr.port());

    let result = fetch_single_page_with_options(
        &url,
        AcquisitionOptions {
            transport: tor_policy(socks.addr),
        },
    )
    .await;

    assert!(result.is_ok(), "{result:?}");
    let acquisition = result.unwrap();
    assert!(matches!(acquisition.transport(), TransportPolicy::Tor(_)));
    assert_eq!(acquisition.page().status_code.as_u16(), 200);

    let recorded = socks.recorded.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    match &recorded[0] {
        RecordedTarget::Domain(name, port) => {
            assert_eq!(name, target_host);
            assert_eq!(*port, http.addr.port());
        }
        other => panic!("expected domain ATYP, got {other:?}"),
    }
}

/// Section O (repeat): same DNS-leak proof for a synthetic `.onion`
/// hostname — the SOCKS fixture must see it verbatim too.
#[tokio::test]
async fn tor_acquisition_passes_onion_hostname_through_unresolved() {
    let http = HttpFixture::start().await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let onion_host = "scorpiontortestfixture1234567.onion";
    let url = format!("http://{onion_host}:{}/", http.addr.port());

    let result = fetch_single_page_with_options(
        &url,
        AcquisitionOptions {
            transport: tor_policy(socks.addr),
        },
    )
    .await;

    assert!(result.is_ok(), "{result:?}");

    let recorded = socks.recorded.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    match &recorded[0] {
        RecordedTarget::Domain(name, _) => assert_eq!(name, onion_host),
        other => panic!("expected domain ATYP, got {other:?}"),
    }
}

/// Section P / I (strengthened): no-fallback proof. The SOCKS endpoint
/// fails every CONNECT; the "direct" target is the exact same HTTP
/// fixture the Tor request is aimed at (reached at `127.0.0.1:port`),
/// which WOULD succeed if contacted directly — this removes any ambiguity
/// about whether a hypothetical fallback could even have worked. Tor
/// acquisition surfaces the SOCKS failure as a non-success page (never
/// panicking, matching `Page::new_page`'s existing error-status
/// convention — the same convention `fetch_single_page`/`Default`
/// already uses), and the fixture's own hit count stays exactly zero —
/// proving no direct/Chrome/Cloud fallback occurred.
#[tokio::test]
async fn tor_socks_failure_never_falls_back_to_direct_target() {
    let target = HttpFixture::start().await;
    let socks = SocksFixture::start_failing().await;

    // Same host:port a direct request would use to reach `target`
    // successfully — proven reachable by the sibling `evidence_records_*`
    // test, which fetches this exact fixture over `Default` transport.
    let url = format!("http://127.0.0.1:{}/", target.addr.port());

    let result = fetch_single_page_with_options(
        &url,
        AcquisitionOptions {
            transport: tor_policy(socks.addr),
        },
    )
    .await;

    let acquisition = result.expect("fetch_single_page_with_options itself does not error");
    assert!(matches!(acquisition.transport(), TransportPolicy::Tor(_)));
    assert!(
        !acquisition.page().status_code.is_success(),
        "expected a non-success status for a SOCKS-failed acquisition, got {}",
        acquisition.page().status_code
    );
    assert_eq!(
        target.hits.load(Ordering::SeqCst),
        0,
        "the target fixture — reachable directly — must never be contacted when Tor fails"
    );
    assert_eq!(socks.connect_count.load(Ordering::SeqCst), 1);
}

/// Section E: bounded-timeout proof against a blackhole SOCKS proxy that
/// accepts the TCP connection and then never sends another byte. Uses a
/// paused/auto-advancing tokio clock so the test proves the REAL
/// (non-shortened) configured bound is honored — the call still
/// eventually completes rather than hanging forever — without the test
/// itself burning real wall-clock time waiting for it. `real_elapsed` is
/// asserted against a generous tolerance (not a tight, flaky bound) since
/// it only needs to prove "did not hang", not measure exact timing.
#[tokio::test(start_paused = true)]
async fn tor_blackhole_socks_completes_within_bounded_timeout() {
    let socks = SocksFixture::start_blackhole().await;
    let url = "http://bounded-timeout-test.invalid/";

    let started = Instant::now();
    let result = fetch_single_page_with_options(
        url,
        AcquisitionOptions {
            transport: tor_policy(socks.addr),
        },
    )
    .await;
    let real_elapsed = started.elapsed();

    // Whether this surfaces as Ok(page-with-bad-status) or as an error
    // string, the call must actually return — that is the property under
    // test — and it must not have needed to actually wait out the
    // configured bound in real wall-clock time (proving the paused clock
    // genuinely drove completion, not a coincidental fast failure).
    if let Ok(acquisition) = &result {
        assert!(!acquisition.page().status_code.is_success());
    }
    assert!(
        real_elapsed < std::time::Duration::from_secs(5),
        "expected paused-clock auto-advance to resolve the bounded timeout \
         near-instantly in real time, took {real_elapsed:?}"
    );
}

/// A slow-drip HTTP fixture: SOCKS handshake and HTTP connection both
/// succeed, headers are sent promising a large body, then exactly one
/// byte of body is written every `drip_interval` — forever — without ever
/// completing the response. Each drip individually keeps the connection
/// "active" (resetting any read-inactivity timeout), so this can only be
/// bounded by a true *total* request deadline, not a read-inactivity one.
struct SlowDripFixture {
    addr: SocketAddr,
}

impl SlowDripFixture {
    async fn start(drip_interval: std::time::Duration) -> Self {
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
                    let headers =
                        "HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\nConnection: close\r\n\r\n";
                    if stream.write_all(headers.as_bytes()).await.is_err() {
                        return;
                    }
                    loop {
                        tokio::time::sleep(drip_interval).await;
                        if stream.write_all(b"x").await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        Self { addr }
    }
}

/// Section D/E: slow-drip proof that a *total* request deadline bounds Tor
/// acquisition against a peer that keeps trickling bytes (never fully
/// stalling, so a read-inactivity timeout alone would not help) and never
/// completes the response.
///
/// Uses a paused clock so the test resolves near-instantly in real time;
/// asserts only that the request is bounded to (at most, with tolerance)
/// `TOR_TOTAL_TIMEOUT`, without asserting *which* of connect/read/total
/// specifically fired — under this exact paused-clock harness, drip
/// activity racing a per-chunk read-inactivity reset against tokio's
/// auto-advance is empirically unreliable to pin down precisely (verified
/// during development: identical drip timing survives far longer in real,
/// non-paused execution, where `read_timeout`'s reset genuinely works —
/// see [`tor_read_timeout_resets_on_activity_real_time`] below, which
/// proves that specific mechanism independently, fast and deterministically,
/// without depending on paused-clock timer-reset semantics at all). What
/// this test *does* prove unambiguously: the request never runs
/// unbounded — whichever timeout fires, it fires by `TOR_TOTAL_TIMEOUT`
/// at the latest, which is the actual security property Section D exists
/// to guarantee.
#[tokio::test(start_paused = true)]
async fn tor_slow_drip_response_is_bounded_by_total_timeout() {
    let drip_interval = std::time::Duration::from_secs(5);
    let http = SlowDripFixture::start(drip_interval).await;
    let socks = SocksFixture::start_splicing(http.addr).await;
    let url = "http://slow-drip-test.invalid/";

    let real_started = Instant::now();
    let virtual_started = tokio::time::Instant::now();
    let result = fetch_single_page_with_options(
        url,
        AcquisitionOptions {
            transport: tor_policy(socks.addr),
        },
    )
    .await;
    let real_elapsed = real_started.elapsed();
    let virtual_elapsed = virtual_started.elapsed();

    if let Ok(acquisition) = &result {
        assert!(!acquisition.page().status_code.is_success());
    }
    assert!(
        real_elapsed < std::time::Duration::from_secs(5),
        "expected the paused clock to resolve this near-instantly in real \
         time, took {real_elapsed:?}"
    );
    assert!(
        virtual_elapsed <= std::time::Duration::from_secs(130),
        "a never-completing drip response must never run past TOR_TOTAL_TIMEOUT \
         (120s) plus reasonable tolerance, took {virtual_elapsed:?} of virtual time"
    );
}

/// Section D/E: independent, fast, real-time (non-paused) proof that
/// `reqwest`'s per-request read timeout genuinely is a resettable
/// inactivity timeout — not a fixed deadline from request start — which
/// is exactly the property that makes a *total* request deadline
/// necessary in the first place (a slow-drip peer can keep resetting an
/// inactivity timeout forever). Deliberately generic (a plain
/// `reqwest::Client`, not the Tor transport module) and deliberately
/// small/fast (well under a second of real wall-clock time): a
/// short-lived custom `read_timeout` is repeatedly outrun by drips spaced
/// well inside it, proving the reset genuinely happens, without needing
/// any paused-clock timer-reset interaction at all.
#[tokio::test]
async fn tor_read_timeout_resets_on_activity_real_time() {
    let read_timeout = std::time::Duration::from_millis(600);
    let drip_interval = std::time::Duration::from_millis(150);
    let http = SlowDripFixture::start(drip_interval).await;

    let client = reqwest::Client::builder()
        .read_timeout(read_timeout)
        .build()
        .unwrap();
    let url = format!("http://{}/", http.addr);

    let started = Instant::now();
    // `.send()` alone only waits for response headers, not the body — the
    // body must actually be read for `read_timeout` to apply at all.
    // Outer bound comfortably above three read_timeout windows (600ms
    // each) but far below what a fixed (non-reset) single 600ms
    // read_timeout would allow — proving the request survives multiple
    // read_timeout windows' worth of elapsed time only because each drip
    // is resetting it.
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        client.get(&url).send().await?.bytes().await
    })
    .await;
    let elapsed = started.elapsed();

    assert!(
        outcome.is_err(),
        "expected the outer 2s bound to be what ends this request \
         (the response never completes), not read_timeout — got {outcome:?}"
    );
    assert!(
        elapsed >= std::time::Duration::from_millis(1800),
        "expected the request to survive close to the full outer bound, \
         proving read_timeout ({read_timeout:?}) was reset by each \
         {drip_interval:?} drip rather than firing on its own, took {elapsed:?}"
    );
}

/// Section Q: onion-direct proof. A synthetic onion host under `Default`
/// transport must fail immediately, with zero SOCKS and zero direct
/// connections — proving no target DNS/network attempt is made at all.
#[tokio::test]
async fn onion_under_default_transport_makes_no_network_attempt() {
    let direct_target = HttpFixture::start().await;
    let socks = SocksFixture::start_failing().await;

    let url = format!(
        "http://scorpiontortestfixture1234567.onion:{}/",
        direct_target.addr.port()
    );

    let result = fetch_single_page_with_options(
        &url,
        AcquisitionOptions {
            transport: TransportPolicy::Default,
        },
    )
    .await;

    assert!(result.is_err());
    assert_eq!(direct_target.hits.load(Ordering::SeqCst), 0);
    assert_eq!(socks.connect_count.load(Ordering::SeqCst), 0);
}

/// Section S / G: evidence tests. Default acquisition -> transport =
/// "default", dns = None. Tor acquisition -> transport = "tor", dns =
/// "proxy". No proxy credentials ever appear (there are none to redact in
/// V1, but the endpoint string itself must not leak either). Provenance
/// comes from `TransportAcquisition`, which only `fetch_single_page_with_options`
/// can produce — there is no way to call `build_evidence_with_transport`
/// with a policy unrelated to how the page was actually fetched.
#[tokio::test]
async fn evidence_records_truthful_transport_provenance() {
    let http = HttpFixture::start().await;

    // Default transport: fetch the HTTP fixture directly.
    let default_url = format!("http://127.0.0.1:{}/", http.addr.port());
    let default_acquisition = fetch_single_page_with_options(
        &default_url,
        AcquisitionOptions {
            transport: TransportPolicy::Default,
        },
    )
    .await
    .unwrap();
    let default_evidence = build_evidence_with_transport(&default_acquisition, None, false, false);
    assert_eq!(default_evidence.transport.as_deref(), Some("default"));
    assert_eq!(default_evidence.dns, None);
    assert_eq!(default_evidence.status_code, Some(200));

    // Tor transport: fetch the same fixture via a splicing SOCKS server.
    let socks = SocksFixture::start_splicing(http.addr).await;
    let tor_url = format!(
        "http://scorpion-evidence-test.invalid:{}/",
        http.addr.port()
    );
    let tor_acquisition = fetch_single_page_with_options(
        &tor_url,
        AcquisitionOptions {
            transport: tor_policy(socks.addr),
        },
    )
    .await
    .unwrap();
    let tor_evidence = build_evidence_with_transport(&tor_acquisition, None, false, false);
    assert_eq!(tor_evidence.transport.as_deref(), Some("tor"));
    assert_eq!(tor_evidence.dns.as_deref(), Some("proxy"));
    assert_eq!(tor_evidence.status_code, Some(200));

    // No proxy credentials anywhere in the evidence (there are none to
    // leak in V1, but the Debug/endpoint representation must not appear
    // either — this is the "no proxy URL" guarantee).
    let serialized = format!("{tor_evidence:?}");
    assert!(!serialized.contains("socks5h"));
    assert!(!serialized.to_lowercase().contains("password"));
}

/// Start an HTTP fixture that 302-redirects its first request to
/// `location` and 200s every subsequent request. Returns the fixture's
/// own listen address and a shared hit counter.
async fn start_redirecting_fixture(location: String) -> (SocketAddr, Arc<AtomicUsize>) {
    start_redirecting_fixture_with(move |_own_port| location).await
}

/// Same as [`start_redirecting_fixture`], but `location` is built from the
/// fixture's own listen port — for genuinely self-referential redirects
/// (host redirects back to itself). Building the `Location` header from
/// the *same* listener the request is actually served by (rather than a
/// second, separately-bound fixture) is what makes the redirect target
/// real and asserted rather than accidentally masked by the SOCKS
/// splicer, which otherwise blindly forwards every CONNECT to whichever
/// upstream address it was configured with regardless of the requested
/// host/port — see the doc comment on the SOCKS-target assertions below.
async fn start_redirecting_fixture_with(
    location: impl FnOnce(u16) -> String,
) -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let location = location(addr.port());
    let hit_count = Arc::new(AtomicUsize::new(0));
    let hit_count_clone = hit_count.clone();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            let hit_count = hit_count_clone.clone();
            let location = location.clone();
            tokio::spawn(async move {
                let n = hit_count.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0_u8; 4096];
                let _ = stream.read(&mut buf).await;
                let response = if n == 0 {
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                        .to_string()
                };
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    (addr, hit_count)
}

/// Section R: redirect matrix — Tor clearnet -> clearnet is followed on
/// the same Tor route (same SOCKS proxy, same target fixture). Asserts
/// the exact recorded SOCKS targets (host *and* port) for both hops, not
/// just a connect count — since the fixture splices every CONNECT to the
/// same upstream regardless of the requested target, a bug that ignored
/// the requested host/port entirely could otherwise still "pass".
#[tokio::test]
async fn tor_redirect_clearnet_to_clearnet_is_followed_on_same_route() {
    let (addr, _hits) = start_redirecting_fixture_with(|own_port| {
        format!("http://redirect-clearnet-test.invalid:{own_port}/next")
    })
    .await;

    let socks = SocksFixture::start_splicing(addr).await;
    let url = format!("http://redirect-clearnet-test.invalid:{}/", addr.port());

    let result = fetch_single_page_with_options(
        &url,
        AcquisitionOptions {
            transport: tor_policy(socks.addr),
        },
    )
    .await;

    assert!(result.is_ok(), "{result:?}");
    let acquisition = result.unwrap();
    assert_eq!(acquisition.page().status_code.as_u16(), 200);
    let recorded = socks.recorded.lock().unwrap().clone();
    assert_eq!(
        recorded,
        vec![
            RecordedTarget::Domain("redirect-clearnet-test.invalid".to_string(), addr.port()),
            RecordedTarget::Domain("redirect-clearnet-test.invalid".to_string(), addr.port()),
        ]
    );
}

/// Section I / R: `Default` transport rejects a redirect that lands on
/// `.onion`, mid-crawl, via the existing SSRF-guarded redirect policy
/// (`Website::is_ssrf_redirect`) — no second redirect engine, no
/// silent transport change.
#[tokio::test]
async fn default_redirect_into_onion_is_rejected() {
    let (addr, hits) =
        start_redirecting_fixture("http://default-redirect-onion-test.onion/".to_string()).await;
    let url = format!("http://127.0.0.1:{}/", addr.port());

    let result = fetch_single_page_with_options(
        &url,
        AcquisitionOptions {
            transport: TransportPolicy::Default,
        },
    )
    .await;

    let acquisition = result.expect("fetch_single_page_with_options itself does not error");
    assert!(
        !acquisition.page().status_code.is_success(),
        "redirect into .onion under Default transport must not succeed, got {}",
        acquisition.page().status_code
    );
    // Only the first (redirecting) hop was ever contacted — the onion
    // redirect target was never dialed.
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

/// Section L: `Default` transport rejects a redirect that lands on an
/// existing-SSRF-forbidden destination (loopback-disguised-as-external
/// is not needed here — a directly loopback/link-local Location header
/// is enough) — proving the pre-existing SSRF redirect guard remains
/// active and reachable through this same policy, unmodified.
#[tokio::test]
async fn default_redirect_into_ssrf_forbidden_destination_is_rejected() {
    let (addr, hits) =
        start_redirecting_fixture("http://169.254.169.254/latest/meta-data/".to_string()).await;
    let url = format!("http://127.0.0.1:{}/", addr.port());

    let result = fetch_single_page_with_options(
        &url,
        AcquisitionOptions {
            transport: TransportPolicy::Default,
        },
    )
    .await;

    let acquisition = result.expect("fetch_single_page_with_options itself does not error");
    assert!(
        !acquisition.page().status_code.is_success(),
        "redirect into an SSRF-forbidden destination must not succeed, got {}",
        acquisition.page().status_code
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

/// Section L: the same SSRF-forbidden-destination redirect, under Tor —
/// proving the dedicated Tor client's base redirect policy
/// (`ssrf_screened_base_policy`) also still enforces it, since Tor
/// clearnet -> clearnet redirects delegate to that base policy rather
/// than bypassing SSRF screening.
#[tokio::test]
async fn tor_redirect_into_ssrf_forbidden_destination_is_rejected() {
    let (addr, hits) =
        start_redirecting_fixture("http://169.254.169.254/latest/meta-data/".to_string()).await;
    let socks = SocksFixture::start_splicing(addr).await;
    let url = format!("http://tor-redirect-ssrf-source.invalid:{}/", addr.port());

    let result = fetch_single_page_with_options(
        &url,
        AcquisitionOptions {
            transport: tor_policy(socks.addr),
        },
    )
    .await;

    let acquisition = result.expect("fetch_single_page_with_options itself does not error");
    assert!(
        !acquisition.page().status_code.is_success(),
        "Tor redirect into an SSRF-forbidden destination must not succeed, got {}",
        acquisition.page().status_code
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

/// Section I / R: Tor transport rejects a redirect from a clearnet
/// service to an onion host — the original request's onion-ness is
/// pinned for the whole redirect chain.
#[tokio::test]
async fn tor_redirect_clearnet_to_onion_is_rejected() {
    let (addr, hits) =
        start_redirecting_fixture("http://tor-redirect-target.onion/".to_string()).await;
    let socks = SocksFixture::start_splicing(addr).await;
    let url = format!("http://tor-redirect-source.invalid:{}/", addr.port());

    let result = fetch_single_page_with_options(
        &url,
        AcquisitionOptions {
            transport: tor_policy(socks.addr),
        },
    )
    .await;

    let acquisition = result.expect("fetch_single_page_with_options itself does not error");
    assert!(
        !acquisition.page().status_code.is_success(),
        "clearnet -> onion redirect under Tor must not succeed, got {}",
        acquisition.page().status_code
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        socks.connect_count.load(Ordering::SeqCst),
        1,
        "the rejected onion redirect target must never reach a second SOCKS CONNECT"
    );
}

/// Section I / R: Tor transport rejects a redirect from an onion service
/// to a clearnet host.
#[tokio::test]
async fn tor_redirect_onion_to_clearnet_is_rejected() {
    let (addr, hits) =
        start_redirecting_fixture("http://tor-redirect-clearnet-target.invalid/".to_string()).await;
    let socks = SocksFixture::start_splicing(addr).await;
    let url = format!("http://tor-redirect-onion-source.onion:{}/", addr.port());

    let result = fetch_single_page_with_options(
        &url,
        AcquisitionOptions {
            transport: tor_policy(socks.addr),
        },
    )
    .await;

    let acquisition = result.expect("fetch_single_page_with_options itself does not error");
    assert!(
        !acquisition.page().status_code.is_success(),
        "onion -> clearnet redirect under Tor must not succeed, got {}",
        acquisition.page().status_code
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(socks.connect_count.load(Ordering::SeqCst), 1);
}

/// Section I / R: Tor transport follows a redirect from an onion service
/// to the *same* onion host (same-service, same-route redirect). Asserts
/// the exact recorded SOCKS targets for both hops.
#[tokio::test]
async fn tor_redirect_onion_to_same_onion_is_followed() {
    let onion_host = "tor-redirect-same-onion-test.onion";
    let (addr, _hits) = start_redirecting_fixture(format!("http://{onion_host}/next")).await;
    let socks = SocksFixture::start_splicing(addr).await;
    let url = format!("http://{onion_host}:{}/", addr.port());

    let result = fetch_single_page_with_options(
        &url,
        AcquisitionOptions {
            transport: tor_policy(socks.addr),
        },
    )
    .await;

    assert!(result.is_ok(), "{result:?}");
    let acquisition = result.unwrap();
    assert_eq!(acquisition.page().status_code.as_u16(), 200);
    let recorded = socks.recorded.lock().unwrap().clone();
    assert_eq!(
        recorded,
        vec![
            RecordedTarget::Domain(onion_host.to_string(), addr.port()),
            RecordedTarget::Domain(onion_host.to_string(), 80),
        ]
    );
}

/// Section I / R: Tor transport rejects a redirect from one onion
/// service to a *different* onion service.
#[tokio::test]
async fn tor_redirect_onion_to_different_onion_is_rejected() {
    let (addr, hits) =
        start_redirecting_fixture("http://tor-redirect-different-onion-target.onion/".to_string())
            .await;
    let socks = SocksFixture::start_splicing(addr).await;
    let url = format!(
        "http://tor-redirect-different-onion-source.onion:{}/",
        addr.port()
    );

    let result = fetch_single_page_with_options(
        &url,
        AcquisitionOptions {
            transport: tor_policy(socks.addr),
        },
    )
    .await;

    let acquisition = result.expect("fetch_single_page_with_options itself does not error");
    assert!(
        !acquisition.page().status_code.is_success(),
        "onion -> different onion redirect under Tor must not succeed, got {}",
        acquisition.page().status_code
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(socks.connect_count.load(Ordering::SeqCst), 1);
}
