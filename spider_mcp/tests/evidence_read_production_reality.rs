//! SCORPION_MCP_CANONICAL_EVIDENCE_READ_001 — production-reality
//! acceptance.
//!
//! Real end-to-end proof, through the actual shipping `spider-mcp`
//! binary driven over genuine MCP JSON-RPC/stdio, that `spider_evidence_read`
//! is compiled into the default build, is advertised by `tools/list`,
//! composes with `spider_audit_page` with no translation of the returned
//! `evidence_ref`, performs zero additional target acquisition, and
//! durably resolves across a completely separate, independently spawned
//! server process — the exact human-in-the-loop shape a future Web
//! Console will rely on. No external network is used; no
//! `#[ignore]`/opt-in env var is needed to run this file.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpClient {
    fn spawn(db_path: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_spider-mcp"))
            .env("SCORPION_DOMAIN_DB", db_path)
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
                "clientInfo": { "name": "evidence-read-production-reality-test", "version": "1.0" }
            }
        }));
        self.next_id += 1;
        assert!(self.response().get("result").is_some());
        self.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    }

    fn list_tools(&mut self) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": "tools/list", "params": {} }));
        self.response()
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
    serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("tool text content was not JSON: {error}; text={text:?}"))
}

struct AuditFixture {
    addr: std::net::SocketAddr,
    hits: Arc<AtomicUsize>,
}

impl AuditFixture {
    fn start() -> Self {
        const BODY: &str =
            "<html><head><title>evidence read fixture</title></head><body>hello</body></html>";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_thread = hits.clone();
        std::thread::spawn(move || loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buf = [0_u8; 4096];
                    let _ = stream.read(&mut buf);
                    hits_thread.fetch_add(1, Ordering::SeqCst);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{BODY}",
                        BODY.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(_) => break,
            }
        });
        Self { addr, hits }
    }

    fn url(&self) -> String {
        format!("http://{}/", self.addr)
    }

    fn hit_count(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

fn db_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "spider-mcp-evidence-read-production-reality-{}-{name}.sqlite3",
        std::process::id()
    ))
}

#[test]
fn spider_evidence_read_is_advertised_by_tools_list() {
    let path = db_path("tools-list");
    let _ = std::fs::remove_file(&path);
    let mut client = McpClient::spawn(&path);
    client.initialize();

    let tools = client.list_tools();
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"spider_audit_page"));
    assert!(
        names.contains(&"spider_evidence_read"),
        "spider_evidence_read must be advertised by the production default build: {names:?}"
    );

    let _ = std::fs::remove_file(&path);
}

/// Phase 17: real AI workflow, entirely through MCP — tools/list ->
/// spider_audit_page -> extract evidence_ref -> spider_evidence_read
/// using that exact string -> canonical EvidenceBundle. No shell
/// manipulation, no direct database access between the two calls.
/// Also proves Phase 9/10: zero additional target acquisition for the
/// evidence read.
#[test]
fn audit_then_evidence_read_composes_end_to_end_with_zero_extra_acquisition() {
    let path = db_path("composition");
    let _ = std::fs::remove_file(&path);
    let fixture = AuditFixture::start();

    let mut client = McpClient::spawn(&path);
    client.initialize();

    let audit_response = client.call("spider_audit_page", json!({ "url": fixture.url() }));
    assert_eq!(audit_response["result"]["isError"], false);
    assert_eq!(fixture.hit_count(), 1);
    let audit_payload = content_payload(&audit_response);
    let evidence_ref = audit_payload["evidence_ref"]
        .as_str()
        .expect("audit response must contain evidence_ref")
        .to_string();

    // The exact string, passed through with no translation.
    let read_response = client.call(
        "spider_evidence_read",
        json!({ "evidence_ref": evidence_ref }),
    );
    assert_eq!(
        read_response["result"]["isError"], false,
        "spider_evidence_read must succeed for the exact evidence_ref spider_audit_page returned: {read_response:?}"
    );

    // Zero additional target acquisition for the read.
    assert_eq!(
        fixture.hit_count(),
        1,
        "spider_evidence_read must perform zero target requests"
    );

    let bundle = content_payload(&read_response);
    assert_eq!(bundle["id"], evidence_ref);
    assert_eq!(bundle["requested_url"], fixture.url());
    assert_eq!(bundle["status_code"], 200);
    assert_eq!(bundle["observed_status_code"], 200);
    assert_eq!(bundle["transport"], "default");
    assert_eq!(bundle["backend_provenance"], "reqwest");
    assert_eq!(bundle["response_origin"], "network");
    assert!(bundle["content"]
        .as_str()
        .unwrap_or_default()
        .contains("evidence read fixture"));
    assert!(bundle["response_body_hash"].as_str().is_some());

    let _ = std::fs::remove_file(&path);
}

#[test]
fn evidence_read_negative_matrix_over_real_mcp_protocol() {
    let path = db_path("negative-matrix");
    let _ = std::fs::remove_file(&path);
    let mut client = McpClient::spawn(&path);
    client.initialize();

    let empty = client.call("spider_evidence_read", json!({ "evidence_ref": "" }));
    assert_eq!(empty["result"]["isError"], true);
    assert_eq!(content_payload(&empty)["error"], "invalid_request");

    let malformed = client.call(
        "spider_evidence_read",
        json!({ "evidence_ref": "not-a-real-evidence-ref" }),
    );
    assert_eq!(malformed["result"]["isError"], true);
    assert_eq!(content_payload(&malformed)["error"], "invalid_request");

    let absent = client.call(
        "spider_evidence_read",
        json!({ "evidence_ref": "evid_0123456789abcdef0123456789abcdef" }),
    );
    assert_eq!(absent["result"]["isError"], true);
    assert_eq!(content_payload(&absent)["error"], "evidence_not_found");

    let _ = std::fs::remove_file(&path);
}

/// Phase 18: cross-process acceptance — the strongest human-in-the-loop
/// precursor proof. Process A acquires and exits; Process B, a wholly
/// separate `spider-mcp` invocation, resolves the exact same
/// `EvidenceRef` through nothing but the shared `SCORPION_DOMAIN_DB`
/// file — never MCP `SharedState`, process memory, or server lifetime.
#[test]
fn evidence_resolves_across_two_independently_spawned_server_processes() {
    let path = db_path("cross-process");
    let _ = std::fs::remove_file(&path);
    let fixture = AuditFixture::start();

    let evidence_ref = {
        let mut process_a = McpClient::spawn(&path);
        process_a.initialize();
        let response = process_a.call("spider_audit_page", json!({ "url": fixture.url() }));
        assert_eq!(response["result"]["isError"], false);
        content_payload(&response)["evidence_ref"]
            .as_str()
            .unwrap()
            .to_string()
        // process_a dropped here: killed and waited on.
    };

    let mut process_b = McpClient::spawn(&path);
    process_b.initialize();
    let response = process_b.call(
        "spider_evidence_read",
        json!({ "evidence_ref": evidence_ref.clone() }),
    );
    assert_eq!(
        response["result"]["isError"], false,
        "a wholly separate spider-mcp process must resolve evidence process A durably recorded: {response:?}"
    );
    let bundle = content_payload(&response);
    assert_eq!(bundle["id"], evidence_ref);
    assert_eq!(bundle["requested_url"], fixture.url());

    let _ = std::fs::remove_file(&path);
}
