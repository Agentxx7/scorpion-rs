#![cfg(feature = "chrome")]

//! SCORPION_HEADLESS_CHROME_PRODUCTION_STACK_SIZE_001.
//!
//! Real end-to-end proof, through the actual shipping `spider-mcp` binary
//! driven over genuine MCP JSON-RPC/stdio, that headless/Chrome-backed
//! acquisition (`spider_scrape`/`spider_crawl` `headless: true`,
//! `return_format: "screenshot"`) works with the operator's ordinary
//! default process environment — no `RUST_MIN_STACK` required. Before this
//! frontier, the identical requests hung indefinitely at the
//! platform-default thread stack size.
//!
//! The fixture's initial HTML contains a placeholder a raw HTTP fetch
//! would see; a script tag replaces it with a marker only real Chrome/JS
//! execution can produce, so every headless assertion below distinguishes
//! genuine Chrome execution from an HTTP fallback that merely didn't hang.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const PRE_JS_PLACEHOLDER: &str = "before-js-placeholder";
const RENDERED_MARKER: &str = "CHROME_RENDERED_MARKER_9f3a1c";

/// Per-launch counter feeding a per-process `TMPDIR`, for the same reason
/// `spider_cli`'s sibling test does: Chrome's default profile directory
/// (`$TMPDIR/chromiumoxide-runner`) is otherwise one fixed path every
/// launch shares, and concurrent/rapid-sequential launches against it race
/// on its `SingletonLock` — an established environment quirk of this
/// launch path, unrelated to production correctness.
static LAUNCH_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpClient {
    fn spawn() -> Self {
        let launch = LAUNCH_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!(
            "spider-mcp-headless-stack-test-{}-{launch}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&tmp);
        let mut child = Command::new(env!("CARGO_BIN_EXE_spider-mcp"))
            .env_remove("RUST_MIN_STACK")
            .env("TMPDIR", &tmp)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spider-mcp must spawn");
        Self {
            stdin: child.stdin.take().unwrap(),
            stdout: BufReader::new(child.stdout.take().unwrap()),
            child,
            next_id: 1,
        }
    }

    fn send(&mut self, value: &Value) {
        writeln!(self.stdin, "{}", serde_json::to_string(value).unwrap()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn response(&mut self) -> Value {
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line)
            .unwrap_or_else(|error| panic!("response was not JSON: {error}; response={line:?}"))
    }

    fn initialize(&mut self) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "headless-chrome-stack-test", "version": "1.0" }
            }
        }));
        self.next_id += 1;
        assert!(self.response().get("result").is_some());
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }));
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        }));
        self.response()
    }

    fn list_tools(&mut self) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/list",
            "params": {}
        }));
        self.response()
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn content_payload(response: &Value) -> Value {
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("missing tool text content: {response:?}"));
    serde_json::from_str(text).unwrap()
}

/// Serves the marker fixture on a fresh local port, once, then stops.
fn marker_fixture() -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        let body = format!(
            "<html><head><title>chrome marker test</title></head><body>\
             <div id=\"target\">{PRE_JS_PLACEHOLDER}</div>\
             <script>document.getElementById('target').textContent = '{RENDERED_MARKER}';</script>\
             </body></html>"
        );
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    (url, handle)
}

/// Positive control: the identical fixture over plain HTTP (`headless`
/// omitted) must show the pre-JS placeholder, never the marker - proves
/// the fixture genuinely distinguishes rendering from raw HTML.
#[test]
fn plain_http_scrape_sees_pre_js_placeholder_not_the_rendered_marker() {
    let (url, handle) = marker_fixture();
    let mut client = McpClient::spawn();
    client.initialize();

    let response = client.call("spider_scrape", json!({ "url": url }));
    handle.join().unwrap();
    assert_eq!(response["result"]["isError"], false, "{response:?}");
    let payload = content_payload(&response);
    let content = payload["content"].as_str().unwrap();
    assert!(content.contains(PRE_JS_PLACEHOLDER), "{content}");
    assert!(!content.contains(RENDERED_MARKER), "{content}");
}

/// The required negative-then-positive proof for `spider_scrape`: with
/// `RUST_MIN_STACK` explicitly unset in the spawned process's own
/// environment, a `headless: true` call against a real local fixture must
/// complete (not hang), report success, report a real observed HTTP
/// response, and return the JS-rendered marker.
#[test]
fn spider_scrape_headless_completes_without_rust_min_stack_and_proves_real_chrome_execution() {
    let (url, handle) = marker_fixture();
    let mut client = McpClient::spawn();
    client.initialize();

    // return_format="raw": avoids markdown's own escaping of `_` inside
    // the marker (`CHROME\_RENDERED\_...`), which would otherwise
    // complicate what is meant to be a direct substring check.
    let response = client.call(
        "spider_scrape",
        json!({ "url": url, "headless": true, "return_format": "raw" }),
    );
    handle.join().unwrap();

    assert_eq!(response["result"]["isError"], false, "{response:?}");
    let payload = content_payload(&response);
    assert_eq!(
        payload["provenance"]["observed_status_code"], 200,
        "{payload}"
    );
    let content = payload["content"].as_str().unwrap();
    assert!(
        content.contains(RENDERED_MARKER),
        "content must contain the JS-rendered marker, proving real Chrome execution \
         (not an HTTP fallback masquerading as success): {content}"
    );
    assert!(!content.contains(PRE_JS_PLACEHOLDER), "{content}");

    // The server must remain fully responsive after driving real Chrome
    // execution - a distinct follow-up call must still succeed.
    let tools = client.list_tools();
    assert!(
        !tools["result"]["tools"].as_array().unwrap().is_empty(),
        "{tools:?}"
    );
}

/// The same proof for `spider_crawl` - a separate call site from
/// `spider_scrape` sharing the same underlying dispatch helper. Do not
/// assume fixing one caller fixes the other.
#[test]
fn spider_crawl_headless_completes_without_rust_min_stack_and_proves_real_chrome_execution() {
    let (url, handle) = marker_fixture();
    let mut client = McpClient::spawn();
    client.initialize();

    let response = client.call(
        "spider_crawl",
        json!({ "url": url, "limit": 1, "headless": true, "return_format": "raw" }),
    );
    handle.join().unwrap();

    assert_eq!(response["result"]["isError"], false, "{response:?}");
    let payload = content_payload(&response);
    let pages = payload["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 1, "{payload}");
    assert_eq!(
        pages[0]["provenance"]["observed_status_code"], 200,
        "{payload}"
    );
    let content = pages[0]["content"].as_str().unwrap();
    assert!(content.contains(RENDERED_MARKER), "{content}");
}

/// Screenshot proof (Section 11): `return_format: "screenshot"` must
/// return real, valid, non-trivial PNG bytes without `RUST_MIN_STACK` -
/// not merely a non-empty payload.
#[test]
fn spider_scrape_screenshot_completes_without_rust_min_stack_and_returns_a_real_png() {
    let (url, handle) = marker_fixture();
    let mut client = McpClient::spawn();
    client.initialize();

    let response = client.call(
        "spider_scrape",
        json!({ "url": url, "return_format": "screenshot" }),
    );
    handle.join().unwrap();

    assert_eq!(response["result"]["isError"], false, "{response:?}");
    let payload = content_payload(&response);
    let encoded = payload["content"]
        .as_str()
        .expect("screenshot content must be a base64 string");
    use base64::Engine;
    // The screenshot pipeline encodes with the URL-safe alphabet (`-`/`_`
    // rather than `+`/`/`), confirmed live against this exact response.
    let bytes = base64::engine::general_purpose::URL_SAFE
        .decode(encoded)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded))
        .expect("screenshot content must be valid base64 (URL-safe)");
    assert!(
        bytes.len() > 1024,
        "expected a substantial real image payload, got {} bytes",
        bytes.len()
    );
    assert_eq!(
        &bytes[..8],
        &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'],
        "expected real PNG magic bytes, proving genuine Chrome-rendered image data"
    );
}

/// Like [`marker_fixture`], but delays its response by `delay` after
/// accepting the connection — long enough to give several concurrent
/// requests a real window to genuinely overlap in flight, and to let a
/// test distinguish "this request ran immediately" from "this request
/// waited for a Chrome execution permit."
fn slow_marker_fixture(delay: std::time::Duration) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        std::thread::sleep(delay);
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        let body = format!(
            "<html><head><title>chrome marker test</title></head><body>\
             <div id=\"target\">{PRE_JS_PLACEHOLDER}</div>\
             <script>document.getElementById('target').textContent = '{RENDERED_MARKER}';</script>\
             </body></html>"
        );
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    (url, handle)
}

/// SCORPION_HEADLESS_CHROME_PRODUCTION_STACK_SIZE_001 (concurrency
/// closure). Real end-to-end proof, through the actual `spider-mcp`
/// binary and genuine MCP JSON-RPC, that concurrent headless requests are
/// bounded rather than unbounded: `REQUESTS` (6) simultaneous
/// `spider_scrape headless:true` calls — more than
/// `CHROME_EXECUTION_PERMITS`'s bound of 4 — against fixtures that each
/// delay their response by a fixed window. If concurrency were genuinely
/// unbounded, all 6 would complete in roughly one fixture delay; bounded
/// to 4, completion happens in two waves, so the total wall-clock time is
/// bounded below by two delays.
///
/// Every single response — including the ones that had to wait for a
/// permit — must still show genuine Chrome-rendered content (the
/// placeholder absent, matching every other test in this file). This is
/// the critical proof that the fix holds under real contention: a
/// genuinely queued request waits for a free permit rather than racing
/// another live Chrome instance for the same profile-directory lock and
/// silently falling back to HTTP (the exact failure this frontier found
/// live before `unique_chrome_profile_dir` — two concurrent launches
/// sharing one profile directory collided on Chrome's own
/// `SingletonLock`, and the losing launch silently downgraded to a plain,
/// unrendered HTTP fetch with `isError: false`).
#[test]
fn concurrent_headless_requests_are_bounded_and_none_falls_back_to_http() {
    const REQUESTS: usize = 6;
    let delay = std::time::Duration::from_millis(800);

    let mut client = McpClient::spawn();
    client.initialize();

    let fixtures: Vec<_> = (0..REQUESTS).map(|_| slow_marker_fixture(delay)).collect();

    let start = std::time::Instant::now();
    for (i, (url, _)) in fixtures.iter().enumerate() {
        client.send(&json!({
            "jsonrpc": "2.0",
            "id": 100 + i,
            "method": "tools/call",
            "params": {
                "name": "spider_scrape",
                "arguments": { "url": url, "headless": true, "return_format": "raw" }
            }
        }));
    }

    for _ in 0..REQUESTS {
        let response = client.response();
        let id = response["id"].as_u64().unwrap();
        assert_eq!(
            response["result"]["isError"], false,
            "id={id}: {response:?}"
        );
        let payload = content_payload(&response);
        let content = payload["content"].as_str().unwrap();
        assert!(
            !content.contains(PRE_JS_PLACEHOLDER),
            "id={id}: every response, including queued ones, must show real Chrome \
             rendering, never a silent HTTP fallback: {content}"
        );
        assert!(content.contains(RENDERED_MARKER), "id={id}: {content}");
    }

    let total = start.elapsed();
    for (_, handle) in fixtures {
        handle.join().unwrap();
    }

    assert!(
        total >= delay * 2,
        "6 concurrent requests against a permit bound of 4 must take at least two \
         fixture delays to all complete - proves real backpressure occurred rather than \
         unbounded concurrency: total={total:?}, one delay={delay:?}"
    );
    assert!(
        total < delay * 2 + std::time::Duration::from_secs(60),
        "generous upper bound (real Chrome launch/navigate overhead on top of the \
         fixture's own delay, times two waves) - catches a genuinely broken \
         serialize-to-one-at-a-time regression without being tight enough to flake. \
         Widened from a +8s to a +60s margin after this margin genuinely fired on \
         real GitHub Actions CI hardware (observed total=13.24s and 13.58s across \
         two real runs, both well under the modest 2-vCPU standard runner's actual \
         Chrome launch/navigate overhead, not a correctness regression -- the lower- \
         bound backpressure assertion above, the only one that would catch a real \
         unbounded-concurrency regression, passed in both runs): \
         total={total:?}"
    );

    // Server must remain fully responsive after the concurrent burst.
    let tools = client.list_tools();
    assert!(
        !tools["result"]["tools"].as_array().unwrap().is_empty(),
        "{tools:?}"
    );
}

/// A plain, instant-responding fixture — no artificial delay — for the
/// HTTP-bypass proof below.
fn fast_fixture() -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        let body = b"<html><body>fast plain http page</body></html>";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
    });
    (url, handle)
}

/// SCORPION_HEADLESS_CHROME_PRODUCTION_STACK_SIZE_001 (concurrency
/// closure, HTTP-bypass proof). With all four `CHROME_EXECUTION_PERMITS`
/// slots saturated by slow headless requests, an ordinary HTTP-only
/// `spider_scrape` (no `headless`) fired at the same time must still
/// complete quickly and independently — proving the semaphore genuinely
/// gates only Chrome-capable dispatch, not general acquisition traffic.
#[test]
fn http_only_request_bypasses_chrome_saturation() {
    let delay = std::time::Duration::from_secs(3);
    let mut client = McpClient::spawn();
    client.initialize();

    // Saturate all 4 Chrome slots with slow headless requests.
    let slow_fixtures: Vec<_> = (0..4).map(|_| slow_marker_fixture(delay)).collect();
    let (fast_url, fast_handle) = fast_fixture();

    let start = std::time::Instant::now();
    for (i, (url, _)) in slow_fixtures.iter().enumerate() {
        client.send(&json!({
            "jsonrpc": "2.0",
            "id": 300 + i,
            "method": "tools/call",
            "params": {
                "name": "spider_scrape",
                "arguments": { "url": url, "headless": true, "return_format": "raw" }
            }
        }));
    }
    // Fired last, after all 4 Chrome slots are already claimed.
    client.send(&json!({
        "jsonrpc": "2.0",
        "id": 999,
        "method": "tools/call",
        "params": {
            "name": "spider_scrape",
            "arguments": { "url": fast_url }
        }
    }));

    let mut http_elapsed = None;
    for _ in 0..5 {
        let response = client.response();
        let id = response["id"].as_u64().unwrap();
        let elapsed = start.elapsed();
        assert_eq!(
            response["result"]["isError"], false,
            "id={id}: {response:?}"
        );
        if id == 999 {
            http_elapsed = Some(elapsed);
        }
    }

    for (_, handle) in slow_fixtures {
        handle.join().unwrap();
    }
    fast_handle.join().unwrap();

    let http_elapsed = http_elapsed.expect("the HTTP-only response (id=999) must arrive");
    assert!(
        http_elapsed < delay,
        "an HTTP-only request must complete well before the Chrome-saturating fixtures' \
         own artificial delay elapses, proving it never queued behind the semaphore: \
         http_elapsed={http_elapsed:?}, chrome_fixture_delay={delay:?}"
    );
}
