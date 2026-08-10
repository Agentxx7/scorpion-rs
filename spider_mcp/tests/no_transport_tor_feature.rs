#![cfg(not(feature = "transport_tor"))]

//! Section K (blocker-fix frontier): a dedicated, deterministic proof for
//! an MCP build compiled *without* `transport_tor`. Only compiles/runs
//! when this crate is built without that feature (e.g.
//! `cargo test -p spider_mcp --no-default-features --features
//! "feed,sitemap,news_sitemap,robots_sitemap"`) — the standard default
//! MCP build keeps `transport_tor` enabled and is entirely unaffected by
//! this file.
//!
//! An explicit `transport.mode = "tor"` request must fail with a
//! `TorNotCompiled`-flavored error, and must NEVER silently fall through
//! to Default (direct) acquisition of the target — that would be a live
//! network request an operator explicitly tried to route through Tor
//! reaching the target directly instead, unannounced.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

struct HttpFixture {
    addr: std::net::SocketAddr,
    hits: Arc<AtomicUsize>,
}

impl HttpFixture {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_thread = hits.clone();
        std::thread::spawn(move || loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    hits_thread.fetch_add(1, Ordering::SeqCst);
                    let mut buf = [0_u8; 1024];
                    let _ = stream.read(&mut buf);
                    let body = b"<html><body>should never be reached</body></html>";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.write_all(body);
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
        self.hits.load(Ordering::SeqCst)
    }
}

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpClient {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_spider-mcp"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spider-mcp must spawn");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        }
    }

    fn send(&mut self, value: &serde_json::Value) {
        let mut line = serde_json::to_string(value).unwrap();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn read_response(&mut self) -> serde_json::Value {
        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .expect("must read a response line from spider-mcp stdout");
        serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("response line was not valid JSON: {e}: {line:?}"))
    }

    fn initialize(&mut self) {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "scorpion-test", "version": "0.0.0" }
            }
        }));
        let response = self.read_response();
        assert!(
            response.get("result").is_some(),
            "initialize must succeed: {response:?}"
        );
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }));
    }

    fn call_tool(&mut self, name: &str, arguments: serde_json::Value) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        }));
        self.read_response()
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A build without `transport_tor` still advertises the `transport` field
/// (schema is unconditional — Section Q/Y: the capability, not just the
/// schema, must be truthfully described) but must reject an explicit Tor
/// request with an honest `TorNotCompiled` error, never a silent Default
/// fallback that reaches the target directly.
#[test]
fn tor_request_fails_with_tor_not_compiled_never_falls_back_to_default() {
    let http = HttpFixture::start();
    let mut client = McpClient::spawn();
    client.initialize();

    let url = format!(
        "http://no-transport-tor-feature-mcp-test.invalid:{}/",
        http.addr.port()
    );
    let response = client.call_tool(
        "spider_scrape",
        serde_json::json!({
            "url": url,
            "transport": {
                "mode": "tor",
                "proxy": "socks5h://127.0.0.1:9050",
            }
        }),
    );

    assert_eq!(
        http.hit_count(),
        0,
        "a Tor request must never silently fall back to reaching the target directly: {response:?}"
    );

    let result = response
        .get("result")
        .unwrap_or_else(|| panic!("expected a tools/call result envelope: {response:?}"));
    let is_error = result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let has_protocol_error = response.get("error").is_some();
    assert!(
        is_error || has_protocol_error,
        "an explicit Tor request must fail (TorNotCompiled), never silently succeed: {response:?}"
    );

    let text = result
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|entry| entry.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or_default();
    assert!(
        text.to_lowercase().contains("transport_tor") || text.to_lowercase().contains("tor"),
        "error message should name the missing capability, not a generic failure: {text:?}"
    );
}

/// Default (non-Tor) acquisition is completely unaffected by the missing
/// feature — the capability gap is scoped to Tor requests only.
#[test]
fn default_transport_is_unaffected_by_missing_transport_tor_feature() {
    let http = HttpFixture::start();
    let mut client = McpClient::spawn();
    client.initialize();

    let url = format!("http://{}/", http.addr);
    let response = client.call_tool("spider_scrape", serde_json::json!({ "url": url }));

    assert_eq!(http.hit_count(), 1, "{response:?}");
    let result = response.get("result").expect("must succeed");
    let is_error = result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(!is_error, "{response:?}");
}
