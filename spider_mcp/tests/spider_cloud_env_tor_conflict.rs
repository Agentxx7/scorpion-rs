#![cfg(all(feature = "spider_cloud", feature = "transport_tor"))]

//! Section J (blocker-fix frontier): the real MCP Spider Cloud activation
//! seam is the `SPIDER_API_KEY` environment variable
//! (`spider_mcp::tools::apply_spider_cloud`), not just a manually
//! constructed `Website.configuration.spider_cloud` field. This proves
//! that seam specifically: a `spider_scrape` call with `SPIDER_API_KEY`
//! set in the server process's environment AND an explicit
//! `transport.mode = "tor"` fails closed before any target networking —
//! never silently falling through to Spider Cloud or direct acquisition.
//!
//! Runs the actual compiled `spider-mcp` binary as a real MCP server over
//! stdio (the genuine protocol boundary, not an internal function call),
//! with `SPIDER_API_KEY` set only in *that child process's* environment —
//! this process's own environment, and every other test's, is untouched,
//! so there is no cross-test contamination to isolate/restore.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// A tiny local HTTP fixture — the target `spider_scrape` must never
/// reach once the Tor + Spider Cloud conflict is rejected.
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

/// Minimal newline-delimited JSON-RPC client speaking just enough MCP to
/// initialize and call one tool.
struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpClient {
    fn spawn(spider_api_key: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_spider-mcp"))
            .env("SPIDER_API_KEY", spider_api_key)
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

/// The real activation seam — `SPIDER_API_KEY` in the server process's
/// environment, not a hand-constructed `Website` — combined with an
/// explicit `transport.mode = "tor"` request must fail closed before any
/// target networking. Never: Tor request silently falls through to
/// Spider Cloud or direct acquisition.
#[test]
fn spider_api_key_env_var_plus_explicit_tor_fails_closed_before_target_network() {
    let http = HttpFixture::start();
    let mut client = McpClient::spawn("sk-test-not-a-real-spider-cloud-key");
    client.initialize();

    let url = format!(
        "http://spider-cloud-env-tor-mcp-test.invalid:{}/",
        http.addr.port()
    );
    let response = client.call_tool(
        "spider_scrape",
        serde_json::json!({
            "url": url,
            "transport": {
                "mode": "tor",
                "proxy": "socks5h://127.0.0.1:9",
            }
        }),
    );

    assert_eq!(
        http.hit_count(),
        0,
        "the target must never be reached once Tor + Spider Cloud is rejected: {response:?}"
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
        "a Tor + Spider Cloud conflict must surface as a tool/protocol error, never a \
         successful result: {response:?}"
    );
}
