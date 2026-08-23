//! Real MCP stdio proof for `spider_crawl`'s background-session
//! (`limit` above `INLINE_LIMIT`) failure semantics, and the
//! `spider_crawl_status` terminal-state read path that reports it.
//!
//! SCORPION_MCP_BACKGROUND_CRAWL_FAILURE_SEMANTICS_001.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

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
        Self {
            stdin: child.stdin.take().unwrap(),
            stdout: BufReader::new(child.stdout.take().unwrap()),
            child,
            next_id: 1,
        }
    }

    fn send(&mut self, value: &Value) {
        use std::io::Write;
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
                "clientInfo": { "name": "crawl-status-failure-test", "version": "1.0" }
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

fn poll_status(client: &mut McpClient, crawl_id: &str) -> Value {
    for _ in 0..300 {
        let response = client.call("spider_crawl_status", json!({ "crawl_id": crawl_id }));
        assert_eq!(
            response["result"]["isError"], false,
            "spider_crawl_status itself must never be a tool error for a known crawl_id: {response:?}"
        );
        let payload = content_payload(&response);
        if payload["status"] != "running" {
            return payload;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("crawl {crawl_id} never left running within the poll budget");
}

/// The exact operator-observed defect and its repair: a background crawl
/// (limit above INLINE_LIMIT = 10) against a verified-unused localhost
/// port must reach terminal status "failed", not "complete", despite
/// producing one synthetic diagnostic page with a zero observed HTTP
/// response.
#[test]
fn background_crawl_refused_acquisition_is_failed_and_server_remains_usable() {
    let unused = TcpListener::bind("127.0.0.1:0").unwrap();
    let refused_url = format!("http://{}/", unused.local_addr().unwrap());
    drop(unused);

    let mut client = McpClient::spawn();
    client.initialize();

    let started = client.call(
        "spider_crawl",
        json!({ "url": refused_url, "limit": 11, "headless": false }),
    );
    assert_eq!(started["result"]["isError"], false, "{started:?}");
    let start_payload = content_payload(&started);
    let crawl_id = start_payload["crawl_id"]
        .as_str()
        .expect("crawl_id must be returned truthfully even for a doomed crawl")
        .to_string();
    assert_eq!(start_payload["status"], "running");

    let summary = poll_status(&mut client, &crawl_id);
    assert_eq!(summary["status"], "failed", "{summary:?}");
    assert_eq!(summary["page_count"], 1, "{summary:?}");
    let page = &summary["pages"][0];
    assert_eq!(page["provenance"]["observed_status_code"], Value::Null);
    assert_eq!(page["provenance"]["response_origin"], Value::Null);
    // No typed transport error exists for an ordinary connection refusal
    // — must not be fabricated.
    assert_eq!(summary["error"], Value::Null);
    assert_eq!(summary["error_code"], Value::Null);

    assert!(!client.list_tools()["result"]["tools"]
        .as_array()
        .unwrap()
        .is_empty());
    let followup = client.call("spider_crawl_status", json!({ "crawl_id": crawl_id }));
    assert_eq!(followup["result"]["isError"], false);
}

/// Positive control: a real background crawl against a real local HTTP
/// server reaches "complete" with a real observed HTTP response —
/// confirms the repair did not turn genuine success into failure.
#[test]
fn background_crawl_real_success_is_complete() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let body = b"<html><body>ok</body></html>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(body);
            });
        }
    });
    let url = format!("http://{addr}/");

    let mut client = McpClient::spawn();
    client.initialize();

    let started = client.call(
        "spider_crawl",
        json!({ "url": url, "limit": 11, "headless": false, "respect_robots_txt": false }),
    );
    assert_eq!(started["result"]["isError"], false, "{started:?}");
    let crawl_id = content_payload(&started)["crawl_id"]
        .as_str()
        .unwrap()
        .to_string();

    let summary = poll_status(&mut client, &crawl_id);
    assert_eq!(summary["status"], "complete", "{summary:?}");
    let page_count = summary["page_count"].as_u64().unwrap();
    assert!(page_count >= 1, "{summary:?}");
    assert_ne!(
        summary["pages"][0]["provenance"]["observed_status_code"],
        Value::Null
    );
}
