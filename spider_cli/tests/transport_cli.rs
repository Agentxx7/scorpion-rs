//! Process-level proof of the CLI Tor transport surface — spawns the real
//! compiled `scorpion` binary (`CARGO_BIN_EXE_scorpion`) against
//! deterministic local HTTP + SOCKS5 fixtures, matching the established
//! `search_cli.rs` convention (`Command`, blocking `std::net` fixtures, no
//! public network/Tor dependency).

#![cfg(all(feature = "fetch", feature = "feed", feature = "transport_tor"))]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn scorpion() -> Command {
    Command::new(env!("CARGO_BIN_EXE_scorpion"))
}

/// A tiny local HTTP fixture: serves one fixed 200 response body for every
/// request, records every path hit.
struct HttpFixture {
    addr: std::net::SocketAddr,
    hits: Arc<Mutex<Vec<String>>>,
}

impl HttpFixture {
    fn start(body: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(Mutex::new(Vec::new()));
        let hits_thread = hits.clone();
        std::thread::spawn(move || loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buf = [0_u8; 4096];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..n]);
                    let path = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .to_string();
                    hits_thread.lock().unwrap().push(path);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        });
        Self { addr, hits }
    }

    fn hit_count(&self) -> usize {
        self.hits.lock().unwrap().len()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SocksBehavior {
    Splice,
    Fail,
}

/// A minimal blocking SOCKS5 fixture: handles the greeting/CONNECT,
/// records connect count, and either splices to a target or always fails
/// (for the "SOCKS failure -> zero direct hits" proof).
struct SocksFixture {
    addr: std::net::SocketAddr,
    connect_count: Arc<AtomicUsize>,
}

impl SocksFixture {
    fn start(splice_to: Option<std::net::SocketAddr>, behavior: SocksBehavior) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let connect_count = Arc::new(AtomicUsize::new(0));
        let connect_count_thread = connect_count.clone();
        std::thread::spawn(move || loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    let connect_count = connect_count_thread.clone();
                    std::thread::spawn(move || {
                        let _ = serve_one(stream, splice_to, behavior, connect_count);
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        });
        Self {
            addr,
            connect_count,
        }
    }

    fn connect_count(&self) -> usize {
        self.connect_count.load(Ordering::SeqCst)
    }
}

fn serve_one(
    mut stream: TcpStream,
    splice_to: Option<std::net::SocketAddr>,
    behavior: SocksBehavior,
    connect_count: Arc<AtomicUsize>,
) -> std::io::Result<()> {
    let mut header = [0_u8; 2];
    stream.read_exact(&mut header)?;
    let nmethods = header[1] as usize;
    let mut methods = vec![0_u8; nmethods];
    stream.read_exact(&mut methods)?;
    stream.write_all(&[0x05, 0x00])?;

    let mut req_head = [0_u8; 4];
    stream.read_exact(&mut req_head)?;
    match req_head[3] {
        0x01 => {
            let mut addr = [0_u8; 4];
            stream.read_exact(&mut addr)?;
        }
        0x03 => {
            let mut len_buf = [0_u8; 1];
            stream.read_exact(&mut len_buf)?;
            let mut name = vec![0_u8; len_buf[0] as usize];
            stream.read_exact(&mut name)?;
        }
        0x04 => {
            let mut addr = [0_u8; 16];
            stream.read_exact(&mut addr)?;
        }
        _ => return Ok(()),
    }
    let mut port_buf = [0_u8; 2];
    stream.read_exact(&mut port_buf)?;

    connect_count.fetch_add(1, Ordering::SeqCst);

    if behavior == SocksBehavior::Fail {
        stream.write_all(&[0x05, 0x01, 0x00, 0x01, 0, 0, 0, 0, 0, 0])?;
        return Ok(());
    }

    let Some(splice_to) = splice_to else {
        stream.write_all(&[0x05, 0x01, 0x00, 0x01, 0, 0, 0, 0, 0, 0])?;
        return Ok(());
    };

    stream.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])?;

    let mut upstream = TcpStream::connect(splice_to)?;
    let mut client_reader = stream.try_clone()?;
    let mut upstream_writer = upstream.try_clone()?;
    let up = std::thread::spawn(move || {
        let _ = std::io::copy(&mut client_reader, &mut upstream_writer);
    });
    let _ = std::io::copy(&mut upstream, &mut stream);
    let _ = up.join();
    Ok(())
}

/// T1: Default clearnet crawl is unaffected by the new flags (omitted).
#[test]
fn default_clearnet_unchanged() {
    let http = HttpFixture::start("<html><body>hi</body></html>");
    let url = format!("http://{}/", http.addr);
    let output = scorpion()
        .args(["fetch", &url])
        .output()
        .expect("scorpion must run");
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(http.hit_count(), 1);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["observed_status_code"], 200);
    assert!(value["content"]
        .as_str()
        .is_some_and(|body| !body.is_empty()));
    assert!(value["response_body_hash"].as_str().is_some());
    assert!(value["retrieved_at"].as_u64().is_some());
    assert_eq!(value["response_origin"], "network");
}

/// An attempted fetch with no HTTP response preserves its structured
/// synthetic failure evidence, but the shipping process must fail so shell
/// automation cannot mistake acquisition failure for success.
#[test]
fn refused_fetch_prints_truthful_evidence_and_exits_nonzero() {
    let unused = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = unused.local_addr().unwrap();
    drop(unused);

    let output = scorpion()
        .args(["fetch", &format!("http://{addr}/")])
        .output()
        .expect("scorpion must run");

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        output.stderr.is_empty(),
        "structured failure stays on stdout"
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["status_code"], 521);
    assert_eq!(value["observed_status_code"], serde_json::Value::Null);
    assert_eq!(value["content"], serde_json::Value::Null);
    assert_eq!(value["response_body_hash"], serde_json::Value::Null);
    assert_eq!(value["transformed_content_hash"], serde_json::Value::Null);
    assert_eq!(value["response_origin"], serde_json::Value::Null);
    assert_eq!(value["retrieved_at"], serde_json::Value::Null);
    assert_eq!(value["backend_provenance"], "reqwest");
}

/// T2: Default transport rejects an onion target before any network
/// activity — confirmed via `fetch`'s explicit-error contract.
#[test]
fn default_onion_rejected() {
    let output = scorpion()
        .args(["fetch", "http://scorpiontestfixtureonion1234567.onion/"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.to_lowercase().contains("onion") || stderr.to_lowercase().contains("tor"),
        "{stderr}"
    );
}

/// T3: Tor onion target reaches the target exclusively via SOCKS.
#[test]
fn tor_onion_via_socks() {
    let http = HttpFixture::start("<html><body>onion fixture</body></html>");
    let socks = SocksFixture::start(Some(http.addr), SocksBehavior::Splice);
    let onion_host = "scorpiontestfixtureonion7654321.onion";
    let url = format!("http://{onion_host}:{}/", http.addr.port());
    let output = scorpion()
        .args([
            "fetch",
            &url,
            "--transport",
            "tor",
            "--tor-proxy",
            &format!("socks5h://{}", socks.addr),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(socks.connect_count(), 1);
    assert_eq!(http.hit_count(), 1);
}

/// T4: Tor clearnet target reaches the target exclusively via SOCKS.
#[test]
fn tor_clearnet_via_socks() {
    let http = HttpFixture::start("<html><body>clearnet fixture</body></html>");
    let socks = SocksFixture::start(Some(http.addr), SocksBehavior::Splice);
    let url = format!("http://clearnet-cli-test.invalid:{}/", http.addr.port());
    let output = scorpion()
        .args([
            "fetch",
            &url,
            "--transport",
            "tor",
            "--tor-proxy",
            &format!("socks5h://{}", socks.addr),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(socks.connect_count(), 1);
    assert_eq!(http.hit_count(), 1);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["transport"], "tor");
    assert_eq!(value["dns"], "proxy");
}

/// T5: `--transport tor` without `--tor-proxy` is rejected.
#[test]
fn missing_tor_proxy_rejected() {
    let output = scorpion()
        .args(["fetch", "http://example.test/", "--transport", "tor"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

/// T6: `--tor-proxy` with the default transport is rejected.
#[test]
fn tor_proxy_with_default_transport_rejected() {
    let output = scorpion()
        .args([
            "fetch",
            "http://example.test/",
            "--tor-proxy",
            "socks5h://127.0.0.1:9050",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

/// T7: `socks5://` (not `socks5h://`) is rejected.
#[test]
fn socks5_scheme_rejected() {
    let output = scorpion()
        .args([
            "fetch",
            "http://example.test/",
            "--transport",
            "tor",
            "--tor-proxy",
            "socks5://127.0.0.1:9050",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

/// T8: a missing explicit port is rejected.
#[test]
fn missing_port_rejected() {
    let output = scorpion()
        .args([
            "fetch",
            "http://example.test/",
            "--transport",
            "tor",
            "--tor-proxy",
            "socks5h://127.0.0.1",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

/// T9: path/query/fragment on the endpoint are all rejected.
#[test]
fn path_query_fragment_rejected() {
    for endpoint in [
        "socks5h://127.0.0.1:9050/path",
        "socks5h://127.0.0.1:9050?x=1",
        "socks5h://127.0.0.1:9050#frag",
    ] {
        let output = scorpion()
            .args([
                "fetch",
                "http://example.test/",
                "--transport",
                "tor",
                "--tor-proxy",
                endpoint,
            ])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{endpoint} must be rejected");
        assert!(output.stdout.is_empty());
    }
}

/// T10: a credential-bearing endpoint is rejected.
#[test]
fn credential_endpoint_rejected() {
    let output = scorpion()
        .args([
            "fetch",
            "http://example.test/",
            "--transport",
            "tor",
            "--tor-proxy",
            "socks5h://user:pass@127.0.0.1:9050",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

/// T11: a SOCKS-layer failure causes zero direct hits on a target that
/// would have responded if contacted directly, bypassing Tor.
#[test]
fn socks_failure_causes_zero_direct_target_hits() {
    let http = HttpFixture::start("<html><body>should never be reached</body></html>");
    let socks = SocksFixture::start(None, SocksBehavior::Fail);
    let url = format!(
        "http://socks-failure-cli-test.invalid:{}/",
        http.addr.port()
    );
    let _output = scorpion()
        .args([
            "fetch",
            &url,
            "--transport",
            "tor",
            "--tor-proxy",
            &format!("socks5h://{}", socks.addr),
        ])
        .output()
        .unwrap();
    assert_eq!(http.hit_count(), 0);
    assert!(socks.connect_count() >= 1);
}

/// T12: `--transport tor --headless` is rejected before any browser
/// launch or target networking.
#[test]
fn tor_headless_rejected_before_browser() {
    let http = HttpFixture::start("<html><body>x</body></html>");
    let socks = SocksFixture::start(Some(http.addr), SocksBehavior::Splice);
    let url = format!("http://{}/", http.addr);
    let output = scorpion()
        .args([
            "--url",
            &url,
            "--headless",
            "crawl",
            "--transport",
            "tor",
            "--tor-proxy",
            &format!("socks5h://{}", socks.addr),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(http.hit_count(), 0);
    assert_eq!(socks.connect_count(), 0);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.to_lowercase().contains("headless"), "{stderr}");
}

/// T13: `--transport tor` combined with the legacy `--proxy-url` is
/// rejected before any target networking.
#[test]
fn tor_legacy_proxy_rejected() {
    let http = HttpFixture::start("<html><body>x</body></html>");
    let socks = SocksFixture::start(Some(http.addr), SocksBehavior::Splice);
    let url = format!("http://{}/", http.addr);
    let output = scorpion()
        .args([
            "--url",
            &url,
            "--proxy-url",
            "http://127.0.0.1:9",
            "crawl",
            "--transport",
            "tor",
            "--tor-proxy",
            &format!("socks5h://{}", socks.addr),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(http.hit_count(), 0);
    assert_eq!(socks.connect_count(), 0);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.to_lowercase().contains("proxy-url"), "{stderr}");
}

/// T14: `--transport tor` combined with a Spider Cloud key is rejected
/// before any target networking.
#[cfg(feature = "spider_cloud")]
#[test]
fn tor_spider_cloud_conflict_rejected() {
    let http = HttpFixture::start("<html><body>x</body></html>");
    let socks = SocksFixture::start(Some(http.addr), SocksBehavior::Splice);
    let url = format!("http://{}/", http.addr);
    let output = scorpion()
        .args([
            "--url",
            &url,
            "--spider-cloud-key",
            "sk-test-not-a-real-key",
            "crawl",
            "--transport",
            "tor",
            "--tor-proxy",
            &format!("socks5h://{}", socks.addr),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(http.hit_count(), 0);
    assert_eq!(socks.connect_count(), 0);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.to_lowercase().contains("spider cloud"), "{stderr}");
}

/// T15: `fetch` evidence for a Tor-acquired page reports truthful
/// transport/dns provenance (also proven inline by `tor_clearnet_via_socks`
/// above; kept here as its own named scenario for direct traceability).
#[test]
fn fetch_evidence_says_tor_and_proxy_dns() {
    let http = HttpFixture::start("<html><body>evidence fixture</body></html>");
    let socks = SocksFixture::start(Some(http.addr), SocksBehavior::Splice);
    let url = format!("http://evidence-cli-test.invalid:{}/", http.addr.port());
    let output = scorpion()
        .args([
            "fetch",
            &url,
            "--transport",
            "tor",
            "--tor-proxy",
            &format!("socks5h://{}", socks.addr),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["transport"], "tor");
    assert_eq!(value["dns"], "proxy");
}

/// T16: `fetch` evidence for a Default-acquired page reports "default"
/// (never fabricated, never left implicitly Tor).
#[test]
fn fetch_evidence_says_default() {
    let http = HttpFixture::start("<html><body>default evidence fixture</body></html>");
    let url = format!("http://{}/", http.addr);
    let output = scorpion().args(["fetch", &url]).output().unwrap();
    assert!(output.status.success(), "{:?}", output);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["transport"], "default");
    assert_eq!(value["dns"], serde_json::Value::Null);
}

/// `spider scrape` surfaces the same truthful acquisition provenance
/// `spider fetch` does — via the canonical `page_provenance` seam, not a
/// second, independently-reimplemented notion of it.
#[test]
fn scrape_output_surfaces_page_provenance() {
    let http = HttpFixture::start("<html><body>scrape provenance fixture</body></html>");
    let url = format!("http://{}/", http.addr);
    let output = scorpion().args(["--url", &url, "scrape"]).output().unwrap();
    assert!(output.status.success(), "{:?}", output);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["provenance"]["transport"], "default");
    assert_eq!(value["provenance"]["dns"], serde_json::Value::Null);
}

/// T17: a genuine Tor crawl preflight rejection (Tor + legacy proxy, in
/// this case) is a nonzero exit with a stderr message — never a
/// successful command with empty stdout.
#[test]
fn crawl_preflight_rejection_is_nonzero_not_empty_success() {
    let http = HttpFixture::start("<html><body>x</body></html>");
    let socks = SocksFixture::start(Some(http.addr), SocksBehavior::Splice);
    let url = format!("http://{}/", http.addr);
    let output = scorpion()
        .args([
            "--url",
            &url,
            "--proxy-url",
            "http://127.0.0.1:9",
            "crawl",
            "--output-links",
            "--transport",
            "tor",
            "--tor-proxy",
            &format!("socks5h://{}", socks.addr),
        ])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "a rejected Tor preflight must never look like a successful command"
    );
    assert!(
        output.stdout.is_empty(),
        "must never be a successful command with empty output"
    );
    assert!(!output.stderr.is_empty());
}

/// T18: `feed` acquires only the feed document itself over Tor — the
/// entry URLs it discovers are never fetched (through SOCKS or
/// otherwise).
#[test]
fn feed_candidate_urls_remain_unfetched_under_tor() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let paths = Arc::new(Mutex::new(Vec::new()));
    let paths_thread = paths.clone();
    let rss = format!(
        r#"<rss version="2.0"><channel><title>T</title><item><guid>one</guid><link>http://{addr}/article-one</link><title>One</title></item></channel></rss>"#
    );
    std::thread::spawn(move || loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let mut buf = [0_u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string();
                paths_thread.lock().unwrap().push(path);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{rss}",
                    rss.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => break,
        }
    });

    let socks = SocksFixture::start(Some(addr), SocksBehavior::Splice);
    let url = format!("http://feed-cli-tor-test.invalid:{}/feed.xml", addr.port());
    let output = scorpion()
        .args([
            "feed",
            &url,
            "--transport",
            "tor",
            "--tor-proxy",
            &format!("socks5h://{}", socks.addr),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    let hits = paths.lock().unwrap().clone();
    assert_eq!(
        hits,
        ["/feed.xml"],
        "only the feed document itself must be fetched: {hits:?}"
    );
    assert!(!hits.iter().any(|p| p.contains("article-one")));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["result_count"], 1);
}

// -----------------------------------------------------------------
// Blocker fix: `--transport`/`--tor-proxy` are scoped to acquisition
// commands only (crawl, scrape, download, fetch, feed, sitemap,
// news-sitemap, robots-sitemap) — `search` and `mcp` must reject them at
// the parser level, in every position: after the subcommand, and before
// it (there is no top-level `Cli::transport` left to catch the latter).
// -----------------------------------------------------------------

/// T19: `search` rejects `--transport` placed after the subcommand.
#[cfg(feature = "search_searxng")]
#[test]
fn search_rejects_transport_flag_after_subcommand() {
    let output = scorpion()
        .args([
            "search",
            "test query",
            "--provider",
            "searxng",
            "--base-url",
            "http://127.0.0.1:1",
            "--transport",
            "tor",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--transport"), "{stderr}");
}

/// T20: `search` rejects `--transport` placed before the subcommand —
/// there is no top-level `Cli::transport` left for it to bind to.
#[cfg(feature = "search_searxng")]
#[test]
fn search_rejects_transport_flag_before_subcommand() {
    let output = scorpion()
        .args([
            "--transport",
            "tor",
            "search",
            "test query",
            "--provider",
            "searxng",
            "--base-url",
            "http://127.0.0.1:1",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--transport"), "{stderr}");
}

/// T21: `mcp` rejects `--transport` placed after the subcommand.
#[cfg(feature = "mcp")]
#[test]
fn mcp_rejects_transport_flag_after_subcommand() {
    let output = scorpion()
        .args(["mcp", "--transport", "tor"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--transport"), "{stderr}");
}

/// T22: `mcp` rejects `--transport` placed before the subcommand.
#[cfg(feature = "mcp")]
#[test]
fn mcp_rejects_transport_flag_before_subcommand() {
    let output = scorpion()
        .args(["--transport", "tor", "mcp"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--transport"), "{stderr}");
}

/// T23: the same is true for `--tor-proxy` alone (not just `--transport`)
/// on both `search` and `mcp`, in both positions.
#[cfg(feature = "search_searxng")]
#[test]
fn search_rejects_tor_proxy_flag_in_either_position() {
    for args in [
        vec![
            "search",
            "q",
            "--provider",
            "searxng",
            "--base-url",
            "http://127.0.0.1:1",
            "--tor-proxy",
            "socks5h://127.0.0.1:9050",
        ],
        vec![
            "--tor-proxy",
            "socks5h://127.0.0.1:9050",
            "search",
            "q",
            "--provider",
            "searxng",
            "--base-url",
            "http://127.0.0.1:1",
        ],
    ] {
        let output = scorpion().args(&args).output().unwrap();
        assert!(!output.status.success(), "{args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("--tor-proxy"), "{args:?}: {stderr}");
    }
}

/// T24: required valid-form proof — `crawl`/`scrape`/`download` all accept
/// `--transport`/`--tor-proxy` placed after the subcommand name (the
/// flattened `TransportArgs` on each variant), matching the acquisition
/// commands that already worked this way (fetch/feed/sitemap/etc).
#[test]
fn crawl_scrape_download_accept_transport_flags_after_subcommand() {
    for subcommand in ["crawl", "scrape", "download"] {
        let http = HttpFixture::start("<html><body>x</body></html>");
        let socks = SocksFixture::start(Some(http.addr), SocksBehavior::Splice);
        let url = format!("http://{}/", http.addr);
        let mut args = vec![
            "--url".to_string(),
            url,
            subcommand.to_string(),
            "--transport".to_string(),
            "tor".to_string(),
            "--tor-proxy".to_string(),
            format!("socks5h://{}", socks.addr),
        ];
        // `download` writes files under a target destination — point it at
        // a scratch dir under the OS temp dir so this test never leaves
        // files behind in the crate's working directory.
        let download_dir = std::env::temp_dir().join(format!(
            "scorpion-transport-cli-test-download-{}",
            socks.addr.port()
        ));
        if subcommand == "download" {
            args.push("--target-destination".to_string());
            args.push(download_dir.to_string_lossy().into_owned());
        }
        let output = scorpion().args(&args).output().unwrap();
        assert!(
            output.status.success(),
            "{subcommand}: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
        // `crawl`/`download` (via `crawl_raw`) also auto-probe
        // `/sitemap.xml` after the seed page — a pre-existing, unrelated
        // crawl behavior (`ignore_sitemap` defaults to `false`), not
        // something this frontier changes. `scrape` does not crawl-follow,
        // so it stays at exactly one hit. Either way, every hit went
        // through SOCKS — that's what this test proves.
        assert!(socks.connect_count() >= 1, "{subcommand}");
        assert_eq!(socks.connect_count(), http.hit_count(), "{subcommand}");
        if subcommand == "download" {
            let _ = std::fs::remove_dir_all(&download_dir);
        }
    }
}
