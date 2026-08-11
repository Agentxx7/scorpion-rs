#![cfg(all(feature = "evidence", feature = "transport_tor", feature = "sitemap"))]

//! Deterministic, network-free proof that the canonical **multi-page** HTTP
//! Tor crawl (`Website::with_transport(TransportPolicy::Tor(..))` +
//! `website.crawl()` / `website.crawl_raw()`) is fail-closed end to end:
//! every request — seed, discovered links, robots.txt, sitemap, nested
//! sitemap, redirects, retries, and every concurrent worker — travels
//! through the one audited SOCKS5h client, with proxy-side target DNS,
//! transport-pinned redirects, and truthful `Page`/evidence provenance.
//!
//! No public Tor dependency: a hand-rolled local SOCKS5 fixture records
//! every CONNECT (domain/IP ATYP, host, port) and splices to local HTTP
//! fixtures serving small, deterministic multi-page sites.

use spider::features::transport::{TorTransportConfig, TransportPolicy};
use spider::website::Website;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
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
    Splice,
    Fail,
}

/// SOCKS5 fixture supporting multiple simultaneous CONNECTs (each
/// connection is handled on its own spawned task), full CONNECT
/// recording (domain/IP ATYP distinction, host, port), and a
/// single-splice-target model — sufficient for same-domain multi-page
/// crawls, where every request in a crawl targets the same origin.
#[derive(Clone)]
struct SocksFixture {
    addr: SocketAddr,
    connect_count: Arc<AtomicUsize>,
    recorded: Arc<Mutex<Vec<RecordedTarget>>>,
    splice_to: Option<SocketAddr>,
    behavior: SocksBehavior,
    /// When set, every CONNECT strictly *after* this many successful
    /// splices fails instead — lets a test succeed on the seed/sitemap
    /// fetches and then force a genuine tunnel failure on a specific
    /// later request (e.g. the sitemap-derived page fetch), without the
    /// SOCKS layer being able to distinguish targets by HTTP path.
    fail_after: Option<usize>,
}

impl SocksFixture {
    async fn start_failing() -> Self {
        Self::start(None, SocksBehavior::Fail, None).await
    }

    async fn start_splicing(splice_to: SocketAddr) -> Self {
        Self::start(Some(splice_to), SocksBehavior::Splice, None).await
    }

    /// Splices successfully for the first `fail_after` CONNECTs, then
    /// fails every one after that.
    async fn start_splicing_fail_after(splice_to: SocketAddr, fail_after: usize) -> Self {
        Self::start(Some(splice_to), SocksBehavior::Splice, Some(fail_after)).await
    }

    async fn start(
        splice_to: Option<SocketAddr>,
        behavior: SocksBehavior,
        fail_after: Option<usize>,
    ) -> Self {
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
            fail_after,
        };

        let accept_fixture = fixture.clone();
        tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                let fixture = accept_fixture.clone();
                // Genuinely concurrent: each accepted connection gets its
                // own task, so simultaneous CONNECTs from concurrent
                // crawl workers are all serviced in parallel, not
                // serialized behind a single accept loop.
                tokio::spawn(async move {
                    let _ = fixture.serve_one(stream).await;
                });
            }
        });

        fixture
    }

    async fn serve_one(&self, mut stream: TcpStream) -> std::io::Result<()> {
        let mut header = [0_u8; 2];
        stream.read_exact(&mut header).await?;
        let nmethods = header[1] as usize;
        let mut methods = vec![0_u8; nmethods];
        stream.read_exact(&mut methods).await?;
        stream.write_all(&[0x05, 0x00]).await?;

        let mut req_head = [0_u8; 4];
        stream.read_exact(&mut req_head).await?;
        let atyp = req_head[3];

        let target = match atyp {
            0x01 => {
                let mut addr = [0_u8; 4];
                stream.read_exact(&mut addr).await?;
                let mut port_buf = [0_u8; 2];
                stream.read_exact(&mut port_buf).await?;
                let port = u16::from_be_bytes(port_buf);
                RecordedTarget::Ipv4(std::net::Ipv4Addr::from(addr), port)
            }
            0x03 => {
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
                let mut addr = [0_u8; 16];
                stream.read_exact(&mut addr).await?;
                let mut port_buf = [0_u8; 2];
                stream.read_exact(&mut port_buf).await?;
                let _ = port_buf;
                RecordedTarget::Domain("<ipv6>".to_string(), 0)
            }
            _ => return Ok(()),
        };

        let connect_number = self.connect_count.fetch_add(1, Ordering::SeqCst) + 1;
        self.recorded.lock().unwrap().push(target);

        let force_fail = self.fail_after.is_some_and(|limit| connect_number > limit);

        if self.behavior == SocksBehavior::Fail || force_fail {
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

/// A tiny multi-page local HTTP fixture: serves different HTML per path
/// from an in-memory route table, records every hit (path -> count), and
/// always closes the connection after one response (`Connection: close`)
/// so `reqwest`'s connection pool never masks a missing/extra request.
struct HttpFixture {
    addr: SocketAddr,
    hits: Arc<Mutex<HashMap<String, usize>>>,
}

impl HttpFixture {
    /// `routes` maps an exact request path (e.g. `"/"`, `"/page2"`) to the
    /// HTML body served for it; any unmapped path gets 404.
    async fn start(routes: HashMap<&'static str, &'static str>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        Self::start_on(listener, routes).await
    }

    /// Same as [`Self::start`], but binds to an already-created listener
    /// — for self-referential fixtures whose route bodies need to embed
    /// the fixture's own (otherwise-not-yet-known) port.
    async fn start_on(listener: TcpListener, routes: HashMap<&'static str, &'static str>) -> Self {
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
                    let hit_number = {
                        let mut hits = hits.lock().unwrap();
                        let count = hits.entry(path.clone()).or_insert(0);
                        *count += 1;
                        *count
                    };

                    let response = match routes.get(&path) {
                        // A body prefixed `REDIRECT:` is a genuine 302
                        // with the rest of the string as `Location` — not
                        // a fake "just navigate to another 200 page"
                        // stand-in, so redirect tests exercise the real
                        // `reqwest` redirect-following/pinning path.
                        Some(body) if body.starts_with("REDIRECT:") => {
                            let location = &body["REDIRECT:".len()..];
                            format!(
                                "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            )
                        }
                        // A body prefixed `FLAKY_ONCE:` serves a `503` on
                        // the *first* hit to this path (forcing the
                        // crawl's ordinary numeric retry path) and the
                        // remainder of the string as a normal `200` body
                        // on every hit after that — a deterministic
                        // "flaky then succeeds" target with no timing
                        // dependency.
                        Some(body) if body.starts_with("FLAKY_ONCE:") => {
                            if hit_number == 1 {
                                let msg = "temporarily unavailable";
                                format!(
                                    "HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{msg}",
                                    msg.len()
                                )
                            } else {
                                let real_body = &body["FLAKY_ONCE:".len()..];
                                format!(
                                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{real_body}",
                                    real_body.len()
                                )
                            }
                        }
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

    fn hit_count(&self, path: &str) -> usize {
        self.hits.lock().unwrap().get(path).copied().unwrap_or(0)
    }

    fn total_hits(&self) -> usize {
        self.hits.lock().unwrap().values().sum()
    }
}

fn tor_policy(socks_addr: SocketAddr) -> TransportPolicy {
    TransportPolicy::Tor(TorTransportConfig::new(&format!("socks5h://{socks_addr}")).unwrap())
}

/// Standard 3-page site: `/` links to `/page2` and `/page3`; both leaves
/// link back to `/` (a cycle, proving visited-tracking works) and to each
/// other, so a full crawl discovers exactly `{ "/", "/page2", "/page3" }`.
fn three_page_site() -> HashMap<&'static str, &'static str> {
    let mut routes = HashMap::new();
    routes.insert(
        "/",
        r#"<html><body><a href="/page2">two</a><a href="/page3">three</a></body></html>"#,
    );
    routes.insert(
        "/page2",
        r#"<html><body><a href="/">home</a><a href="/page3">three</a></body></html>"#,
    );
    routes.insert(
        "/page3",
        r#"<html><body><a href="/">home</a><a href="/page2">two</a></body></html>"#,
    );
    routes
}

/// T6: a clearnet multi-page crawl under Tor discovers every page and
/// every single request — seed and both discovered children — travels
/// through SOCKS as a domain-ATYP CONNECT to the exact seed host, proving
/// the fixed `CrawlHttpContext`/client serves the whole crawl, not just
/// the first request.
#[tokio::test]
async fn tor_clearnet_multi_page_crawl_stays_through_socks() {
    let http = HttpFixture::start(three_page_site()).await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let seed_host = "clearnet-crawl-test.invalid";
    let url = format!("http://{seed_host}:{}/", http.addr.port());

    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_ignore_sitemap(true);
    website.with_limit(10);
    website.crawl_raw().await;

    let visited = website.get_links();
    assert_eq!(
        visited.len(),
        3,
        "expected all 3 pages discovered, got {visited:?}"
    );

    assert_eq!(http.hit_count("/"), 1);
    assert_eq!(http.hit_count("/page2"), 1);
    assert_eq!(http.hit_count("/page3"), 1);

    let recorded = socks.recorded.lock().unwrap();
    assert_eq!(
        recorded.len(),
        3,
        "every one of the 3 fetches must have gone through SOCKS, got {recorded:?}"
    );
    for target in recorded.iter() {
        match target {
            RecordedTarget::Domain(host, port) => {
                assert_eq!(host, seed_host);
                assert_eq!(*port, http.addr.port());
            }
            other => panic!("expected domain ATYP for every request, got {other:?}"),
        }
    }
}

/// T4/T5: every target CONNECT during a multi-page crawl uses domain
/// ATYP (never IP), and the seed hostname is deliberately unresolvable
/// locally — proving proxy-side DNS resolution for the whole crawl, not
/// just a single one-shot fetch.
#[tokio::test]
async fn tor_crawl_never_resolves_target_hostname_locally() {
    let http = HttpFixture::start(three_page_site()).await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let seed_host = "definitely-nonexistent-scorpion-crawl-host.invalid";
    let url = format!("http://{seed_host}:{}/", http.addr.port());

    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_ignore_sitemap(true);
    website.with_limit(10);
    website.crawl_raw().await;

    assert_eq!(website.get_links().len(), 3);
    let recorded = socks.recorded.lock().unwrap();
    assert!(recorded
        .iter()
        .all(|t| matches!(t, RecordedTarget::Domain(host, _) if host == seed_host)));
}

/// T10: SOCKS CONNECT failure for every request causes zero contact with
/// the (directly reachable) target fixture — no direct/Default fallback
/// for multi-page crawling either, matching the one-shot guarantee.
#[tokio::test]
async fn tor_crawl_socks_failure_causes_zero_direct_target_hits() {
    let http = HttpFixture::start(three_page_site()).await;
    let socks = SocksFixture::start_failing().await;

    // Same host:port a direct request would use to reach `http`
    // successfully (proven reachable by the sibling multi-page test
    // above), removing ambiguity about whether a fallback could work.
    let url = format!("http://127.0.0.1:{}/", http.addr.port());

    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_ignore_sitemap(true);
    website.with_limit(10);
    website.crawl_raw().await;

    assert_eq!(
        http.total_hits(),
        0,
        "the directly-reachable target fixture must never be contacted when SOCKS fails"
    );
    assert!(socks.connect_count.load(Ordering::SeqCst) >= 1);
}

/// T12 / Section K: concurrent workers all use SOCKS, with a
/// **deterministic** (not timing-based) proof of genuine temporal
/// overlap. Every leaf request blocks on a shared `tokio::sync::Barrier`
/// sized 2 before responding — the barrier can only release once two
/// requests are *simultaneously* in flight, so a passing test is
/// structural proof of concurrency, not an inference from wall-clock
/// timing. Active/max-concurrent counts are tracked independently
/// (incremented before the barrier wait, decremented after) so the
/// assertion doesn't rely on the barrier size alone.
#[tokio::test]
async fn tor_crawl_concurrent_workers_all_use_socks() {
    use std::sync::atomic::AtomicI64;
    use tokio::sync::Barrier;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http_addr = listener.local_addr().unwrap();

    let active = Arc::new(AtomicI64::new(0));
    let max_active = Arc::new(AtomicI64::new(0));
    let max_active_outer = max_active.clone();
    // Releases once 2 leaf requests are simultaneously blocked on it —
    // the deterministic overlap primitive Section K asks for in place of
    // a timing-only test.
    let barrier = Arc::new(Barrier::new(2));

    let mut home_links = String::new();
    for i in 1..=8 {
        home_links.push_str(&format!(r#"<a href="/leaf{i}">leaf{i}</a>"#));
    }
    let home_body = format!("<html><body>{home_links}</body></html>");

    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            let home_body = home_body.clone();
            let active = active.clone();
            let max_active = max_active.clone();
            let barrier = barrier.clone();
            tokio::spawn(async move {
                let mut buf = [0_u8; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string();

                if path == "/" {
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{home_body}",
                        home_body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    return;
                }

                // Leaf path: track concurrency, then block until a second
                // leaf request is simultaneously in flight.
                let now_active = active.fetch_add(1, Ordering::SeqCst) + 1;
                max_active.fetch_max(now_active, Ordering::SeqCst);
                barrier.wait().await;
                active.fetch_sub(1, Ordering::SeqCst);

                let body = "<html><body>leaf</body></html>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });

    let socks = SocksFixture::start_splicing(http_addr).await;

    let seed_host = "concurrent-crawl-test.invalid";
    let url = format!("http://{seed_host}:{}/", http_addr.port());

    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_ignore_sitemap(true);
    website.with_limit(20);

    tokio::time::timeout(std::time::Duration::from_secs(20), website.crawl_raw())
        .await
        .expect("crawl must not hang — the barrier releases as soon as 2 leaves overlap");

    assert_eq!(website.get_links().len(), 9, "seed + 8 leaves");
    assert_eq!(socks.connect_count.load(Ordering::SeqCst), 9);
    assert!(
        max_active_outer.load(Ordering::SeqCst) >= 2,
        "expected at least 2 leaf requests simultaneously in flight, deterministically proven by the barrier"
    );
    let recorded = socks.recorded.lock().unwrap();
    assert!(recorded
        .iter()
        .all(|t| matches!(t, RecordedTarget::Domain(host, _) if host == seed_host)));
}

/// T1/T2/T3: an onion seed crawls entirely over SOCKS — the seed page and
/// a same-onion child page (`/a`, `/b` on the *same* onion host) are both
/// fetched, every CONNECT uses domain ATYP against the exact onion
/// hostname, and no local DNS is ever needed for a synthetic `.onion`
/// address that could never resolve in real DNS.
#[tokio::test]
async fn tor_onion_seed_multi_page_crawl_stays_through_socks() {
    let mut routes = HashMap::new();
    routes.insert(
        "/",
        r#"<html><body><a href="/a">a</a><a href="/b">b</a></body></html>"#,
    );
    routes.insert("/a", r#"<html><body>page a</body></html>"#);
    routes.insert("/b", r#"<html><body>page b</body></html>"#);

    let http = HttpFixture::start(routes).await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let onion_host = "scorpiontestfixtureonion1234567.onion";
    let url = format!("http://{onion_host}:{}/", http.addr.port());

    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_ignore_sitemap(true);
    website.with_limit(10);
    website.crawl_raw().await;

    let visited = website.get_links();
    assert_eq!(visited.len(), 3, "seed + /a + /b, got {visited:?}");
    assert_eq!(http.hit_count("/"), 1);
    assert_eq!(http.hit_count("/a"), 1);
    assert_eq!(http.hit_count("/b"), 1);

    let recorded = socks.recorded.lock().unwrap();
    assert_eq!(recorded.len(), 3);
    for target in recorded.iter() {
        match target {
            RecordedTarget::Domain(host, _) => assert_eq!(host, onion_host),
            other => panic!("expected domain ATYP for every request, got {other:?}"),
        }
    }
}

/// T7/T8: an onion seed whose page links to a *different* onion host and
/// to a clearnet host — neither forbidden candidate is ever admitted to
/// the crawl frontier, so the SOCKS fixture never sees a CONNECT for
/// either of them; only the seed's own onion host is ever contacted.
#[tokio::test]
async fn tor_onion_seed_never_follows_cross_onion_or_clearnet_links() {
    let mut routes = HashMap::new();
    routes.insert(
        "/",
        r#"<html><body>
            <a href="http://different-onion-fixture1234567.onion/evil">different onion</a>
            <a href="http://clearnet-escape-fixture.invalid/evil">clearnet escape</a>
        </body></html>"#,
    );

    let http = HttpFixture::start(routes).await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let onion_host = "scorpiontestfixtureonion7654321.onion";
    let url = format!("http://{onion_host}:{}/", http.addr.port());

    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_ignore_sitemap(true);
    website.with_limit(10);
    website.crawl_raw().await;

    let visited = website.get_links();
    assert_eq!(
        visited.len(),
        1,
        "only the seed may be visited — neither forbidden link may be followed, got {visited:?}"
    );

    let recorded = socks.recorded.lock().unwrap();
    assert_eq!(
        recorded.len(),
        1,
        "the SOCKS fixture must never see a CONNECT for either forbidden candidate"
    );
    assert!(matches!(
        &recorded[0],
        RecordedTarget::Domain(host, _) if host == onion_host
    ));
}

/// T9: a clearnet Tor seed whose page links to an onion host — the onion
/// candidate is never admitted (clearnet-pinned crawl rejects onion
/// candidates), so it never reaches the SOCKS fixture.
#[tokio::test]
async fn tor_clearnet_seed_never_follows_onion_links() {
    let mut routes = HashMap::new();
    routes.insert(
        "/",
        r#"<html><body><a href="http://escape-fixture1234567890.onion/evil">onion escape</a></body></html>"#,
    );

    let http = HttpFixture::start(routes).await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let seed_host = "clearnet-seed-no-onion-test.invalid";
    let url = format!("http://{seed_host}:{}/", http.addr.port());

    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_ignore_sitemap(true);
    website.with_limit(10);
    website.crawl_raw().await;

    assert_eq!(website.get_links().len(), 1, "only the seed");
    let recorded = socks.recorded.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert!(matches!(
        &recorded[0],
        RecordedTarget::Domain(host, _) if host == seed_host
    ));
}

/// T13: robots.txt is fetched through the exact same SOCKS route as
/// every other request in the crawl — same client, same redirect policy,
/// same proxy-side DNS — not a separate target client.
#[tokio::test]
async fn tor_crawl_fetches_robots_txt_through_socks() {
    let mut routes = HashMap::new();
    routes.insert("/robots.txt", "User-agent: *\nAllow: /\n");
    routes.insert("/", r#"<html><body><a href="/page2">two</a></body></html>"#);
    routes.insert("/page2", "<html><body>two</body></html>");

    let http = HttpFixture::start(routes).await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let seed_host = "robots-crawl-test.invalid";
    let url = format!("http://{seed_host}:{}/", http.addr.port());

    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_ignore_sitemap(true);
    website.configuration.respect_robots_txt = true;
    website.with_limit(10);
    website.crawl_raw().await;

    assert_eq!(
        http.hit_count("/robots.txt"),
        1,
        "robots.txt must actually be fetched"
    );
    assert_eq!(website.get_links().len(), 2, "seed + /page2");

    let recorded = socks.recorded.lock().unwrap();
    // seed + robots.txt + /page2 == 3 CONNECTs, all to the same onion-free
    // clearnet seed host — proving robots.txt travelled the same route.
    assert_eq!(recorded.len(), 3, "{recorded:?}");
    assert!(recorded
        .iter()
        .all(|t| matches!(t, RecordedTarget::Domain(host, _) if host == seed_host)));
}

/// T14/T15: `sitemap.xml` (and a page URL it discovers) are fetched
/// through the same SOCKS route — sitemap-derived URLs pass the same
/// transport-boundary validation as any other discovered link.
#[tokio::test]
async fn tor_crawl_fetches_sitemap_and_discovered_page_through_socks() {
    // The sitemap body is self-referential (its `<loc>` must carry the
    // fixture's own real port), so bind first, then build routes from the
    // now-known port — mirroring the established `start_..._with` pattern
    // for self-referential fixtures elsewhere in this frontier's tests.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture_addr = listener.local_addr().unwrap();

    let mut routes = HashMap::new();
    routes.insert("/", "<html><body>home, no links</body></html>");
    let sitemap_body: &'static str = Box::leak(
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>http://sitemap-crawl-test.invalid:{}/from-sitemap</loc></url>
</urlset>"#,
            fixture_addr.port()
        )
        .into_boxed_str(),
    );
    routes.insert("/sitemap.xml", sitemap_body);
    routes.insert("/from-sitemap", "<html><body>from sitemap</body></html>");

    let http = HttpFixture::start_on(listener, routes).await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let seed_host = "sitemap-crawl-test.invalid";
    let url = format!("http://{seed_host}:{}/", http.addr.port());

    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_limit(10);
    website.crawl_raw().await;

    assert_eq!(
        http.hit_count("/sitemap.xml"),
        1,
        "sitemap.xml must be fetched"
    );
    assert_eq!(
        http.hit_count("/from-sitemap"),
        1,
        "the sitemap-discovered page must be fetched"
    );

    let recorded = socks.recorded.lock().unwrap();
    assert!(recorded
        .iter()
        .all(|t| matches!(t, RecordedTarget::Domain(host, _) if host == seed_host)));
}

/// T16: a genuine 302 redirect encountered while fetching the seed (same
/// clearnet host, so redirect pinning follows it — see the one-shot
/// redirect matrix in `transport_tor.rs` for the full pin/reject matrix
/// at the client-policy level, which this dedicated Tor crawl client
/// shares verbatim) is followed, and both the redirecting request and its
/// target are fetched through the same SOCKS route, alongside an
/// ordinarily-discovered second page.
#[tokio::test]
async fn tor_crawl_follows_redirect_through_socks() {
    let seed_host = "redirect-crawl-test.invalid";

    let mut routes = HashMap::new();
    routes.insert("/", "REDIRECT:/final");
    routes.insert(
        "/final",
        r#"<html><body><a href="/next">next</a></body></html>"#,
    );
    routes.insert("/next", "<html><body>next page</body></html>");
    let http = HttpFixture::start(routes).await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let url = format!("http://{seed_host}:{}/", http.addr.port());
    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_ignore_sitemap(true);
    website.with_limit(10);
    website.crawl_raw().await;

    assert_eq!(http.hit_count("/"), 1, "the redirecting request");
    assert_eq!(http.hit_count("/final"), 1, "the redirect target");
    assert_eq!(http.hit_count("/next"), 1, "discovered from the final page");

    let recorded = socks.recorded.lock().unwrap();
    assert!(!recorded.is_empty());
    assert!(recorded
        .iter()
        .all(|t| matches!(t, RecordedTarget::Domain(host, _) if host == seed_host)));
}

/// T19/T20: every network-acquired `Page` from a Tor crawl carries
/// truthful transport provenance, and evidence built from it reports
/// `"tor"` — proving `Page::transport()`/`build_evidence`
/// are correctly wired for the multi-page path, not just the one-shot
/// seam.
#[tokio::test]
async fn tor_crawl_pages_carry_truthful_transport_provenance() {
    use spider::features::transport::AcquisitionTransport;
    use spider::utils::evidence::build_evidence;

    let http = HttpFixture::start(three_page_site()).await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let seed_host = "provenance-crawl-test.invalid";
    let url = format!("http://{seed_host}:{}/", http.addr.port());

    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_ignore_sitemap(true);
    website.with_limit(10);

    let mut rx = website.subscribe(16);
    // Bounded by the known page count (3), not by channel closure:
    // `unsubscribe()` doesn't necessarily close the broadcast channel out
    // from under an already-created receiver, so "loop until `recv`
    // errors" is not a safe termination condition here.
    let pages = tokio::spawn(async move {
        let mut collected = Vec::new();
        while collected.len() < 3 {
            match rx.recv().await {
                Ok(page) => collected.push(page),
                Err(_) => break,
            }
        }
        collected
    });
    website.crawl_raw().await;
    website.unsubscribe();
    let pages = tokio::time::timeout(std::time::Duration::from_secs(5), pages)
        .await
        .expect("page collector must finish once 3 pages have arrived")
        .unwrap();

    assert_eq!(pages.len(), 3, "seed + 2 children");
    for page in &pages {
        assert_eq!(
            page.transport(),
            Some(AcquisitionTransport::Tor),
            "page for {} must be stamped Tor",
            page.get_url()
        );
        let evidence = build_evidence(page, None, false, false);
        // Section H/I (blocker-fix frontier): `build_evidence` now reads
        // `Page.transport()` directly as the single canonical provenance
        // source — a Tor-acquired page's evidence must say so too,
        // without the caller needing to construct a `TransportAcquisition`
        // or otherwise reconstruct the policy.
        assert_eq!(evidence.transport.as_deref(), Some("tor"));
        assert_eq!(evidence.dns.as_deref(), Some("proxy"));
    }
}

/// T25: a blackhole SOCKS endpoint (accepts the TCP connection, never
/// completes even the greeting) bounds a crawl exactly like it bounds a
/// one-shot fetch — the crawl completes (doesn't hang) within the
/// connect timeout, with zero pages produced.
#[tokio::test(start_paused = true)]
async fn tor_crawl_terminates_under_blackhole_socks() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            tokio::spawn(async move {
                let mut sink = [0_u8; 1];
                loop {
                    std::future::pending::<()>().await;
                    let _ = stream.read(&mut sink).await;
                }
            });
        }
    });

    let url = "http://blackhole-crawl-test.invalid/";
    let mut website = Website::new(url);
    website.with_transport(tor_policy(addr));
    website.with_ignore_sitemap(true);
    website.with_limit(10);

    let real_started = std::time::Instant::now();
    website.crawl_raw().await;
    let real_elapsed = real_started.elapsed();

    assert!(
        real_elapsed < std::time::Duration::from_secs(5),
        "expected the paused clock to resolve this near-instantly in real \
         time, took {real_elapsed:?}"
    );
    assert!(website.get_links().is_empty() || website.get_links().len() <= 1);
}

/// T17: a redirect to an SSRF-forbidden destination during a Tor crawl is
/// rejected — the existing SSRF redirect guard remains active for the
/// multi-page path (reused verbatim from the one-shot client, not
/// duplicated), so the forbidden target is never actually fetched.
#[tokio::test]
async fn tor_crawl_rejects_redirect_to_ssrf_forbidden_destination() {
    let seed_host = "ssrf-redirect-crawl-test.invalid";
    let mut routes = HashMap::new();
    routes.insert("/", "REDIRECT:http://169.254.169.254/latest/meta-data/");
    let http = HttpFixture::start(routes).await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let url = format!("http://{seed_host}:{}/", http.addr.port());
    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_ignore_sitemap(true);
    website.with_limit(10);
    website.crawl_raw().await;

    assert_eq!(
        http.hit_count("/"),
        1,
        "the redirecting request itself is fetched"
    );
    // The SSRF-forbidden redirect target must never be dialed — the
    // client-level redirect policy rejects it before any second CONNECT.
    assert_eq!(socks.connect_count.load(Ordering::SeqCst), 1);
}

/// T18: `crawl_smart` still fails closed under a *fully valid, compatible*
/// Tor configuration — confirming this isn't merely a side effect of
/// `tor_crawl_preflight` rejecting something else. Smart/Chrome escalation
/// remains categorically unaudited for Tor, independent of how clean the
/// rest of the configuration is.
#[cfg(feature = "smart")]
#[tokio::test]
async fn tor_crawl_smart_fails_closed_even_with_valid_tor_config() {
    let http = HttpFixture::start(three_page_site()).await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let url = format!(
        "http://smart-fallback-crawl-test.invalid:{}/",
        http.addr.port()
    );
    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_ignore_sitemap(true);
    website.with_limit(10);
    website.crawl_smart().await;

    assert_eq!(website.get_status(), &spider::website::CrawlStatus::Invalid);
    assert_eq!(
        http.total_hits(),
        0,
        "crawl_smart must never reach the target, valid Tor config or not"
    );
    assert_eq!(socks.connect_count.load(Ordering::SeqCst), 0);
}

/// T22: a URL rewritten by `on_link_find_callback` to a forbidden
/// candidate (a different onion host) is rejected immediately before
/// acquisition — the admission-time check only saw the *original* queued
/// link, so this specifically proves the second, pre-fetch check (not
/// just admission) is doing real work.
#[tokio::test]
async fn tor_crawl_rejects_callback_rewritten_forbidden_url() {
    let mut routes = HashMap::new();
    routes.insert("/", r#"<html><body><a href="/page2">two</a></body></html>"#);
    routes.insert("/page2", "<html><body>two</body></html>");
    let http = HttpFixture::start(routes).await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let onion_host = "callback-rewrite-source1234567.onion";
    let forbidden_host = "callback-rewrite-target7654321.onion";
    let url = format!("http://{onion_host}:{}/", http.addr.port());

    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_ignore_sitemap(true);
    website.with_limit(10);
    let forbidden_url = format!("http://{forbidden_host}/rewritten");
    // `on_link_find_callback` rewrites every discovered link to a
    // *different* onion host — a candidate the admission-time check never
    // saw (it only validated the original `/page2` link).
    website.on_link_find_callback = Some(std::sync::Arc::new(move |_link, html| {
        (
            spider::CaseInsensitiveString::from(forbidden_url.as_str()),
            html,
        )
    }));
    website.crawl_raw().await;

    let recorded = socks.recorded.lock().unwrap();
    assert!(
        recorded
            .iter()
            .all(|t| matches!(t, RecordedTarget::Domain(host, _) if host == onion_host)),
        "the rewritten forbidden host must never reach SOCKS: {recorded:?}"
    );
}

/// T23: a URL restored into `extra_links` (the queued/pending-link path,
/// distinct from links discovered by parsing a fetched page) that
/// violates the transport boundary is rejected immediately before fetch,
/// the same as any other candidate.
#[tokio::test]
async fn tor_crawl_rejects_queued_extra_link_forbidden_url() {
    let http = HttpFixture::start(three_page_site()).await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let seed_host = "extra-links-crawl-test.invalid";
    let url = format!("http://{seed_host}:{}/", http.addr.port());

    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_ignore_sitemap(true);
    website.with_limit(10);
    // Queue a forbidden (onion) candidate directly, bypassing normal page
    // discovery entirely — the pre-acquisition boundary check must still
    // catch it.
    website.set_extra_links(
        std::iter::once(spider::CaseInsensitiveString::from(
            "http://queued-forbidden-target1234.onion/",
        ))
        .collect(),
    );
    website.crawl_raw().await;

    let recorded = socks.recorded.lock().unwrap();
    assert!(
        recorded
            .iter()
            .all(|t| matches!(t, RecordedTarget::Domain(host, _) if host == seed_host)),
        "the queued forbidden onion candidate must never reach SOCKS: {recorded:?}"
    );
}

/// T24: a target that fails once (`503`, retryable) and then succeeds
/// forces the crawl's ordinary numeric retry/backoff path to actually
/// fire — both the failing attempt and the succeeding retry must travel
/// through the exact same SOCKS route (same client, same transport
/// boundary), never rotating proxy/client or falling back to Default.
#[tokio::test]
async fn tor_crawl_retries_stay_socks_only() {
    let mut routes = HashMap::new();
    routes.insert("/", "FLAKY_ONCE:<html><body>recovered</body></html>");
    let http = HttpFixture::start(routes).await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let seed_host = "retry-crawl-test.invalid";
    let url = format!("http://{seed_host}:{}/", http.addr.port());

    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_ignore_sitemap(true);
    website.with_limit(10);
    website.with_retry(1);
    website.crawl_raw().await;

    assert_eq!(
        http.hit_count("/"),
        2,
        "expected exactly one failing attempt plus one successful retry"
    );
    assert_eq!(website.get_links().len(), 1, "the seed, once recovered");

    let recorded = socks.recorded.lock().unwrap();
    assert_eq!(
        recorded.len(),
        2,
        "both the failing attempt and the retry must travel through SOCKS: {recorded:?}"
    );
    assert!(
        recorded
            .iter()
            .all(|t| matches!(t, RecordedTarget::Domain(host, _) if host == seed_host)),
        "every retry attempt must use the same SOCKS route: {recorded:?}"
    );
}

/// Section C regression
/// (`SCORPION_CANONICAL_MULTI_PAGE_HTTP_TOR_CRAWL_BLOCKER_FIX_001`): a
/// Tor **sitemap-derived** page fetch that encounters a genuine SOCKS
/// tunnel failure must never fall through to a real local DNS lookup for
/// the target hostname. Unlike the crawl-level SOCKS-failure test above
/// (which proves "zero direct target hits"), this specifically forces the
/// `confirm_tunnel_failure_with_local_dns` → `host_resolves_locally`
/// branch and proves it via a call-count seam
/// (`host_resolves_locally_call_count_for_test`) rather than accepting
/// "the SOCKS fixture still saw a domain-ATYP CONNECT" as sufficient
/// proof on its own — a *successful* request proves proxy-side DNS, but
/// says nothing about what happens on the *failure* path, which is a
/// structurally different code path (`build_error_page_response`).
#[tokio::test]
async fn tor_crawl_sitemap_failure_never_triggers_local_dns_lookup() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture_addr = listener.local_addr().unwrap();
    let seed_host = "dns-failure-sitemap-test.invalid";

    let mut routes = HashMap::new();
    routes.insert("/", "<html><body>home, no links</body></html>");
    let sitemap_body: &'static str = Box::leak(
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>http://{seed_host}:{}/from-sitemap</loc></url>
</urlset>"#,
            fixture_addr.port()
        )
        .into_boxed_str(),
    );
    routes.insert("/sitemap.xml", sitemap_body);
    routes.insert(
        "/from-sitemap",
        "<html><body>must never actually be reached</body></html>",
    );

    let http = HttpFixture::start_on(listener, routes).await;
    // Connect #1 = seed, #2 = sitemap.xml both succeed; #3 = the
    // sitemap-derived page fetch fails — a genuine SOCKS tunnel failure
    // for a request the Section A/B task-scope fix now correctly reaches
    // from inside a spawned sitemap worker task.
    let socks = SocksFixture::start_splicing_fail_after(http.addr, 2).await;

    let before = spider::page::host_resolves_locally_call_count_for_test();

    let url = format!("http://{seed_host}:{}/", http.addr.port());
    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_limit(10);
    website.crawl_raw().await;

    let after = spider::page::host_resolves_locally_call_count_for_test();

    assert_eq!(http.hit_count("/sitemap.xml"), 1);
    assert_eq!(
        http.hit_count("/from-sitemap"),
        0,
        "the failed fetch must never actually reach the target"
    );
    assert_eq!(
        socks.connect_count.load(Ordering::SeqCst),
        3,
        "expected seed + sitemap.xml + one failed sitemap-page attempt"
    );
    let recorded = socks.recorded.lock().unwrap();
    assert!(
        matches!(&recorded[2], RecordedTarget::Domain(host, _) if host == seed_host),
        "the SOCKS fixture must still observe the unresolved hostname on the failing CONNECT: {recorded:?}"
    );
    assert_eq!(
        after, before,
        "a Tor tunnel failure on a sitemap-derived fetch must never trigger a real local DNS lookup"
    );
}

/// E1 (Section E): a Tor onion-seed sitemap's same-onion page is allowed
/// through SOCKS.
#[tokio::test]
async fn tor_onion_sitemap_same_onion_page_allowed_through_socks() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture_addr = listener.local_addr().unwrap();
    let onion_host = "sitemap-boundary-onion-e1-abcde.onion";

    let mut routes = HashMap::new();
    routes.insert("/", "<html><body>onion sitemap seed</body></html>");
    let sitemap_body: &'static str = Box::leak(
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>http://{onion_host}:{}/from-onion-sitemap</loc></url>
</urlset>"#,
            fixture_addr.port()
        )
        .into_boxed_str(),
    );
    routes.insert("/sitemap.xml", sitemap_body);
    routes.insert(
        "/from-onion-sitemap",
        "<html><body>onion sitemap page</body></html>",
    );

    let http = HttpFixture::start_on(listener, routes).await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let url = format!("http://{onion_host}:{}/", http.addr.port());
    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_limit(10);
    website.crawl_raw().await;

    assert_eq!(http.hit_count("/sitemap.xml"), 1);
    assert_eq!(
        http.hit_count("/from-onion-sitemap"),
        1,
        "same-onion sitemap page must be fetched"
    );

    let recorded = socks.recorded.lock().unwrap();
    assert!(recorded
        .iter()
        .all(|t| matches!(t, RecordedTarget::Domain(host, _) if host == onion_host)));
}

/// E2 (Section E): a Tor onion-seed sitemap's clearnet page is rejected —
/// zero SOCKS CONNECT for the forbidden target, and zero direct hit
/// against a fixture that would have proven contact if it had happened.
#[tokio::test]
async fn tor_onion_sitemap_clearnet_page_rejected() {
    let mut forbidden_routes = HashMap::new();
    forbidden_routes.insert("/evil", "<html><body>should never be reached</body></html>");
    let forbidden = HttpFixture::start(forbidden_routes).await;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let onion_host = "sitemap-boundary-onion-e2-fghij.onion";

    let mut routes = HashMap::new();
    routes.insert("/", "<html><body>onion sitemap seed</body></html>");
    let sitemap_body: &'static str = Box::leak(
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>http://clearnet-forbidden-target.invalid:{}/evil</loc></url>
</urlset>"#,
            forbidden.addr.port()
        )
        .into_boxed_str(),
    );
    routes.insert("/sitemap.xml", sitemap_body);

    let http = HttpFixture::start_on(listener, routes).await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let url = format!("http://{onion_host}:{}/", http.addr.port());
    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_limit(10);
    website.crawl_raw().await;

    assert_eq!(http.hit_count("/sitemap.xml"), 1);

    let recorded = socks.recorded.lock().unwrap();
    assert!(
        recorded
            .iter()
            .all(|t| matches!(t, RecordedTarget::Domain(host, _) if host == onion_host)),
        "the forbidden clearnet target must never reach SOCKS: {recorded:?}"
    );
    assert_eq!(
        forbidden.total_hits(),
        0,
        "the forbidden clearnet target must never be contacted directly either"
    );
}

/// E3 (Section E): a Tor onion-seed sitemap's different-onion page is
/// rejected — zero SOCKS CONNECT for the forbidden onion target.
#[tokio::test]
async fn tor_onion_sitemap_different_onion_page_rejected() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture_addr = listener.local_addr().unwrap();
    let onion_host = "sitemap-boundary-onion-e3-klmno.onion";
    let forbidden_onion_host = "sitemap-boundary-onion-e3-forbid.onion";

    let mut routes = HashMap::new();
    routes.insert("/", "<html><body>onion sitemap seed</body></html>");
    let sitemap_body: &'static str = Box::leak(
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>http://{forbidden_onion_host}:{}/evil</loc></url>
</urlset>"#,
            fixture_addr.port()
        )
        .into_boxed_str(),
    );
    routes.insert("/sitemap.xml", sitemap_body);

    let http = HttpFixture::start_on(listener, routes).await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let url = format!("http://{onion_host}:{}/", http.addr.port());
    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_limit(10);
    website.crawl_raw().await;

    assert_eq!(http.hit_count("/sitemap.xml"), 1);

    let recorded = socks.recorded.lock().unwrap();
    assert!(
        recorded
            .iter()
            .all(|t| matches!(t, RecordedTarget::Domain(host, _) if host == onion_host)),
        "the different-onion forbidden target must never reach SOCKS: {recorded:?}"
    );
}

/// E4 (Section E): a nested same-onion sitemap is allowed through SOCKS —
/// both the nested sitemap fetch itself and the page it discovers.
#[tokio::test]
async fn tor_onion_nested_sitemap_same_onion_allowed_through_socks() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture_addr = listener.local_addr().unwrap();
    let onion_host = "sitemap-boundary-onion-e4-pqrst.onion";

    let mut routes = HashMap::new();
    routes.insert("/", "<html><body>onion nested-sitemap seed</body></html>");
    let root_sitemap_body: &'static str = Box::leak(
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <sitemap><loc>http://{onion_host}:{}/nested-sitemap.xml</loc></sitemap>
</sitemapindex>"#,
            fixture_addr.port()
        )
        .into_boxed_str(),
    );
    routes.insert("/sitemap.xml", root_sitemap_body);
    let nested_sitemap_body: &'static str = Box::leak(
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>http://{onion_host}:{}/from-nested-sitemap</loc></url>
</urlset>"#,
            fixture_addr.port()
        )
        .into_boxed_str(),
    );
    routes.insert("/nested-sitemap.xml", nested_sitemap_body);
    routes.insert(
        "/from-nested-sitemap",
        "<html><body>nested sitemap page</body></html>",
    );

    let http = HttpFixture::start_on(listener, routes).await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let url = format!("http://{onion_host}:{}/", http.addr.port());
    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_limit(10);
    website.crawl_raw().await;

    assert_eq!(http.hit_count("/sitemap.xml"), 1);
    assert_eq!(
        http.hit_count("/nested-sitemap.xml"),
        1,
        "the nested sitemap must be fetched"
    );
    assert_eq!(
        http.hit_count("/from-nested-sitemap"),
        1,
        "the page discovered from the nested sitemap must be fetched"
    );

    let recorded = socks.recorded.lock().unwrap();
    assert!(recorded
        .iter()
        .all(|t| matches!(t, RecordedTarget::Domain(host, _) if host == onion_host)));
}

/// E5 (Section E): a page discovered from a nested sitemap that violates
/// the transport boundary is rejected before acquisition — the nested
/// sitemap fetch itself still succeeds (it's the same onion host), but
/// the forbidden page it names never reaches SOCKS.
#[tokio::test]
async fn tor_onion_nested_sitemap_forbidden_target_rejected() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture_addr = listener.local_addr().unwrap();
    let onion_host = "sitemap-boundary-onion-e5-uvwxy.onion";

    let mut routes = HashMap::new();
    routes.insert("/", "<html><body>onion nested-sitemap seed</body></html>");
    let root_sitemap_body: &'static str = Box::leak(
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <sitemap><loc>http://{onion_host}:{}/nested-sitemap.xml</loc></sitemap>
</sitemapindex>"#,
            fixture_addr.port()
        )
        .into_boxed_str(),
    );
    routes.insert("/sitemap.xml", root_sitemap_body);
    let nested_sitemap_body = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>http://nested-sitemap-clearnet-forbidden.invalid/evil</loc></url>
</urlset>"#;
    routes.insert("/nested-sitemap.xml", nested_sitemap_body);

    let http = HttpFixture::start_on(listener, routes).await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let url = format!("http://{onion_host}:{}/", http.addr.port());
    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_limit(10);
    website.crawl_raw().await;

    assert_eq!(
        http.hit_count("/nested-sitemap.xml"),
        1,
        "the nested sitemap itself must still be fetched"
    );

    let recorded = socks.recorded.lock().unwrap();
    assert!(
        recorded
            .iter()
            .all(|t| matches!(t, RecordedTarget::Domain(host, _) if host == onion_host)),
        "the forbidden target discovered from the nested sitemap must never reach SOCKS: {recorded:?}"
    );
}

/// E6 (Section E): a Tor clearnet-seed sitemap's onion target is
/// rejected.
#[tokio::test]
async fn tor_clearnet_sitemap_onion_target_rejected() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let seed_host = "sitemap-boundary-clearnet-e6.invalid";

    let mut routes = HashMap::new();
    routes.insert("/", "<html><body>clearnet sitemap seed</body></html>");
    let sitemap_body = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>http://clearnet-to-onion-forbidden1234567.onion/evil</loc></url>
</urlset>"#;
    routes.insert("/sitemap.xml", sitemap_body);

    let http = HttpFixture::start_on(listener, routes).await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let url = format!("http://{seed_host}:{}/", http.addr.port());
    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_limit(10);
    website.crawl_raw().await;

    assert_eq!(http.hit_count("/sitemap.xml"), 1);

    let recorded = socks.recorded.lock().unwrap();
    assert!(
        recorded
            .iter()
            .all(|t| matches!(t, RecordedTarget::Domain(host, _) if host == seed_host)),
        "the forbidden onion target must never reach SOCKS from a clearnet-pinned crawl: {recorded:?}"
    );
}

/// F (Section F): after the Section A/B task-scope fix, every
/// network-acquired `Page` from a sitemap-derived and nested-sitemap-derived
/// fetch carries truthful Tor provenance — not just ordinary
/// worker-fetched pages (already covered by
/// `tor_crawl_pages_carry_truthful_transport_provenance`).
#[tokio::test]
async fn tor_crawl_sitemap_derived_pages_carry_truthful_transport_provenance() {
    use spider::features::transport::AcquisitionTransport;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture_addr = listener.local_addr().unwrap();
    let seed_host = "provenance-sitemap-crawl-test.invalid";

    let mut routes = HashMap::new();
    routes.insert("/", "<html><body>seed, no links</body></html>");
    let root_sitemap_body: &'static str = Box::leak(
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <sitemap><loc>http://{seed_host}:{}/nested-sitemap.xml</loc></sitemap>
</sitemapindex>"#,
            fixture_addr.port()
        )
        .into_boxed_str(),
    );
    routes.insert("/sitemap.xml", root_sitemap_body);
    let nested_sitemap_body: &'static str = Box::leak(
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>http://{seed_host}:{}/from-nested-sitemap</loc></url>
</urlset>"#,
            fixture_addr.port()
        )
        .into_boxed_str(),
    );
    routes.insert("/nested-sitemap.xml", nested_sitemap_body);
    routes.insert(
        "/from-nested-sitemap",
        "<html><body>nested sitemap page</body></html>",
    );

    let http = HttpFixture::start_on(listener, routes).await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let url = format!("http://{seed_host}:{}/", http.addr.port());
    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_limit(10);

    let mut rx = website.subscribe(16);
    // Bounded by the known page count (seed + nested-sitemap-derived page
    // == 2), matching the established pattern used by the ordinary-path
    // provenance test — see its comment for why "loop until closed" is
    // unsafe here.
    let pages = tokio::spawn(async move {
        let mut collected = Vec::new();
        while collected.len() < 2 {
            match rx.recv().await {
                Ok(page) => collected.push(page),
                Err(_) => break,
            }
        }
        collected
    });
    website.crawl_raw().await;
    website.unsubscribe();
    let pages = tokio::time::timeout(std::time::Duration::from_secs(5), pages)
        .await
        .expect("page collector must finish once 2 pages have arrived")
        .unwrap();

    assert_eq!(pages.len(), 2, "seed + nested-sitemap-derived page");
    assert_eq!(http.hit_count("/nested-sitemap.xml"), 1);
    for page in &pages {
        assert_eq!(
            page.transport(),
            Some(AcquisitionTransport::Tor),
            "page for {} must be stamped Tor",
            page.get_url()
        );
    }
}

/// L (Section L): a sitemap-derived page fetch that fails once (`503`,
/// retryable) and then succeeds forces `sitemap_parse_crawl`'s own
/// (separate) numeric retry loop to fire — both attempts must travel
/// through the same SOCKS route, the same transport scope, and the
/// resulting `Page` must still carry truthful Tor provenance. This is a
/// distinct code path from the ordinary worker retry loop
/// (`tor_crawl_retries_stay_socks_only`), so proving one does not prove
/// the other.
#[tokio::test]
async fn tor_crawl_sitemap_derived_page_retries_stay_socks_only() {
    use spider::features::transport::AcquisitionTransport;

    let seed_host = "sitemap-retry-crawl-test.invalid";

    let mut routes = HashMap::new();
    routes.insert("/", "<html><body>home, no links</body></html>");
    let sitemap_body: &'static str = Box::leak(
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>http://{seed_host}:PORT/from-sitemap</loc></url>
</urlset>"#
        )
        .into_boxed_str(),
    );
    routes.insert(
        "/from-sitemap",
        "FLAKY_ONCE:<html><body>recovered from sitemap</body></html>",
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture_addr = listener.local_addr().unwrap();
    let sitemap_body: &'static str = Box::leak(
        sitemap_body
            .replace("PORT", &fixture_addr.port().to_string())
            .into_boxed_str(),
    );
    routes.insert("/sitemap.xml", sitemap_body);

    let http = HttpFixture::start_on(listener, routes).await;
    let socks = SocksFixture::start_splicing(http.addr).await;

    let url = format!("http://{seed_host}:{}/", http.addr.port());
    let mut website = Website::new(&url);
    website.with_transport(tor_policy(socks.addr));
    website.with_limit(10);
    website.with_retry(1);

    let mut rx = website.subscribe(16);
    let pages = tokio::spawn(async move {
        let mut collected = Vec::new();
        while collected.len() < 2 {
            match rx.recv().await {
                Ok(page) => collected.push(page),
                Err(_) => break,
            }
        }
        collected
    });
    website.crawl_raw().await;
    website.unsubscribe();
    let pages = tokio::time::timeout(std::time::Duration::from_secs(5), pages)
        .await
        .expect("page collector must finish once 2 pages have arrived")
        .unwrap();

    assert_eq!(
        http.hit_count("/from-sitemap"),
        2,
        "expected exactly one failing attempt plus one successful retry"
    );
    assert_eq!(pages.len(), 2, "seed + sitemap-derived page (after retry)");

    let recorded = socks.recorded.lock().unwrap();
    // seed(1) + sitemap.xml(1) + sitemap-page attempt(1) + sitemap-page
    // retry(1) == 4 CONNECTs, all to the same host.
    assert_eq!(recorded.len(), 4, "{recorded:?}");
    assert!(
        recorded
            .iter()
            .all(|t| matches!(t, RecordedTarget::Domain(host, _) if host == seed_host)),
        "every retry attempt must use the same SOCKS route: {recorded:?}"
    );

    for page in &pages {
        assert_eq!(
            page.transport(),
            Some(AcquisitionTransport::Tor),
            "page for {} must be stamped Tor even after a retry",
            page.get_url()
        );
    }
}
