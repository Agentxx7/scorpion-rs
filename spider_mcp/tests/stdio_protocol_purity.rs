//! SCORPION_CANONICAL_MCP_STDIO_PROTOCOL_PURITY_001: closes the exact
//! arrow `spider-mcp shipping process -> stdio transport -> stdout byte
//! stream -> JSON-RPC/MCP consumer`.
//!
//! Every other tests/*.rs suite in this crate proves *response semantics*
//! (a given request produces a given result/error shape) by reading
//! exactly as many stdout lines as it expects and parsing each one. That
//! is real evidence those specific lines are valid JSON, but it is not
//! the same claim as "stdout contains only protocol bytes": a test that
//! only ever reads N lines for N expected responses cannot see a stray
//! line before/after them, and every existing test discards stderr
//! entirely (`Stdio::null()`), so it proves nothing about which channel
//! diagnostics actually land on.
//!
//! This suite instead captures the *entire* stdout stream produced across
//! a real session -- every line consumed while driving the interaction,
//! plus everything written after the last request, read until the real
//! spider-mcp process actually exits -- and requires every single line to
//! be a genuine JSON-RPC 2.0 envelope. Stderr is captured independently
//! (never discarded, never required to be empty -- the contract is
//! stdout purity, not diagnostic silence).

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Every non-empty line the server ever writes to stdout, across the
/// entire session, must satisfy this shape: a real JSON-RPC 2.0 envelope
/// -- a response (`jsonrpc` + `id` + `result` or `error`) or a
/// notification (`jsonrpc` + `method`, no `id`) -- never plain text, a
/// startup banner, a log line, or garbage mixed into a protocol line.
/// Whole-line contamination and garbage sharing a line with real JSON are
/// both already rejected by `serde_json::from_str` on the full trimmed
/// line (it accepts no leading or trailing non-JSON bytes); a
/// well-formed-but-wrong-shape JSON line (e.g. a JSON-structured log
/// line some dependency might emit by default) is rejected by the
/// `jsonrpc == "2.0"` and id/result/error-or-method shape checks below.
fn assert_line_is_pure_jsonrpc(line: &str, context: &str) {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() {
        return;
    }
    let value: Value = serde_json::from_str(trimmed).unwrap_or_else(|error| {
        panic!(
            "{context}: a stdout line was not valid JSON at all -- this is exactly the \
             contamination this test exists to catch (a banner, a log line, or garbage \
             sharing a line with real JSON would each fail to parse): error={error}; \
             line={trimmed:?}"
        )
    });
    assert_eq!(
        value["jsonrpc"], "2.0",
        "{context}: stdout line parsed as JSON but was not a JSON-RPC 2.0 envelope: {value:?}"
    );
    let has_id = value.get("id").is_some();
    let is_response = has_id && (value.get("result").is_some() || value.get("error").is_some());
    let is_notification = !has_id && value.get("method").is_some();
    assert!(
        is_response || is_notification,
        "{context}: stdout line was JSON-RPC-shaped but neither a real response (id + \
         result/error) nor a real notification (method, no id): {value:?}"
    );
}

struct McpClient {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    /// Every stdout line already consumed via `response()`, carried
    /// forward so the final purity check in `finish()` covers the
    /// *entire* stream, not only whatever happened to be read after the
    /// interaction ended.
    stdout_audit: Vec<String>,
    stderr_rx: mpsc::Receiver<Vec<u8>>,
}

impl McpClient {
    fn spawn_with_args(args: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_spider-mcp"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spider-mcp must spawn");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut stderr = child.stderr.take().unwrap();

        // Drain stderr on its own thread from the moment the process
        // starts. Required to avoid a real pipe deadlock: if the child
        // ever fills stderr's OS pipe buffer while this test is blocked
        // reading stdout (or vice versa), both sides stall forever --
        // exactly why every other test in this crate discards stderr
        // with `Stdio::null()` instead of piping it. Capturing it
        // properly, without that risk, is the whole point of this suite.
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf);
            let _ = tx.send(buf);
        });

        Self {
            child,
            stdin: Some(stdin),
            stdout,
            next_id: 1,
            stdout_audit: Vec::new(),
            stderr_rx: rx,
        }
    }

    fn spawn() -> Self {
        Self::spawn_with_args(&[])
    }

    fn send(&mut self, value: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin already closed");
        writeln!(stdin, "{}", serde_json::to_string(value).unwrap()).unwrap();
        stdin.flush().unwrap();
    }

    fn response(&mut self) -> Value {
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        self.stdout_audit.push(line.clone());
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
                "clientInfo": { "name": "stdio-protocol-purity-test", "version": "1.0" }
            }
        }));
        self.next_id += 1;
        assert!(self.response().get("result").is_some());
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }));
    }

    fn scrape(&mut self, url: &str) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": "spider_scrape",
                "arguments": { "url": url, "return_format": "raw", "evidence": false }
            }
        }));
        self.response()
    }

    /// Closes stdin -- the server's real, documented shutdown signal
    /// (EOF on stdin) -- then waits for the process to actually exit,
    /// bounded so a wrong assumption about shutdown behavior can never
    /// hang the suite (falls back to a hard kill), drains every
    /// remaining stdout byte, and collects the stderr gathered on the
    /// background thread. Returns the complete stdout line audit for the
    /// whole session and the complete stderr bytes.
    fn finish(mut self) -> (Vec<String>, Vec<u8>) {
        drop(self.stdin.take());

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(20));
                }
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }

        let mut remaining = String::new();
        let _ = self.stdout.read_to_string(&mut remaining);
        for line in remaining.split_inclusive('\n') {
            if !line.is_empty() {
                self.stdout_audit.push(line.to_string());
            }
        }

        let stderr_bytes = self
            .stderr_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_default();

        (std::mem::take(&mut self.stdout_audit), stderr_bytes)
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn content_text(response: &Value) -> &str {
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("missing tool text content: {response:?}"))
}

fn local_http(body: &'static [u8]) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
    });
    (format!("http://{addr}/"), handle)
}

/// A real refused-connection URL, the same deterministic failure class
/// `scrape_failure_semantics.rs` already establishes as this crate's
/// canonical acquisition-failure fixture.
fn refused_url() -> String {
    let unused = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", unused.local_addr().unwrap());
    drop(unused);
    url
}

#[test]
fn stdout_is_protocol_pure_across_success_failure_and_shutdown() {
    let mut client = McpClient::spawn();
    client.initialize();

    let (success_url, server) = local_http(b"<html><body>protocol purity fixture</body></html>");
    let succeeded = client.scrape(&success_url);
    assert_eq!(succeeded["result"]["isError"], false, "{succeeded:?}");
    assert!(content_text(&succeeded).contains("protocol purity fixture"));
    server.join().unwrap();

    let failed = client.scrape(&refused_url());
    assert_eq!(failed["result"]["isError"], true, "{failed:?}");

    let (stdout_lines, _stderr_bytes) = client.finish();
    assert!(
        stdout_lines.len() >= 3,
        "expected at least the initialize response and two scrape responses on stdout, got {}: {stdout_lines:?}",
        stdout_lines.len()
    );
    for line in &stdout_lines {
        assert_line_is_pure_jsonrpc(line, "default logging session");
    }
}

#[test]
fn stdout_remains_protocol_pure_with_verbose_logging_enabled() {
    // `--log-level debug` is the real, supported mechanism (spider_mcp's
    // own main.rs clap flag) -- not RUN_LIVE_TESTS, not RUST_LOG (main.rs
    // builds its env_logger via `Builder::new().parse_filters(&cli.log_level)`,
    // which reads only the CLI flag, never the environment).
    let mut client = McpClient::spawn_with_args(&["--log-level", "debug"]);
    client.initialize();

    let (success_url, server) = local_http(b"<html><body>verbose logging fixture</body></html>");
    let succeeded = client.scrape(&success_url);
    assert_eq!(succeeded["result"]["isError"], false, "{succeeded:?}");
    server.join().unwrap();

    let failed = client.scrape(&refused_url());
    assert_eq!(failed["result"]["isError"], true, "{failed:?}");

    let (stdout_lines, _stderr_bytes) = client.finish();
    assert!(
        stdout_lines.len() >= 3,
        "expected at least the initialize response and two scrape responses on stdout, got {}: {stdout_lines:?}",
        stdout_lines.len()
    );
    for line in &stdout_lines {
        assert_line_is_pure_jsonrpc(line, "--log-level debug session");
    }
}
