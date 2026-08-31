//! SCORPION_MCP_CANONICAL_PAGE_AUDIT_SHIPPING_001 — production-reality
//! acceptance.
//!
//! Real end-to-end proof, through the actual shipping `spider-mcp` binary
//! driven over genuine MCP JSON-RPC/stdio (never a standalone `run()`
//! helper called in-process), that `spider_audit_page` is compiled into
//! the default/production build, is advertised by `tools/list`, reaches
//! the real production handler on `tools/call`, performs exactly one
//! acquisition against a real local HTTP fixture, and durably records
//! evidence a second, independently spawned process can resolve — the
//! exact shape a future human-facing interface (Web Console) will rely
//! on. No external network is used; no `#[ignore]`/opt-in env var is
//! needed to run this file.

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
    /// Spawn the real, default-feature `spider-mcp` binary with
    /// `SCORPION_DOMAIN_DB` pointed at `db_path` — the same canonical,
    /// neutral environment variable an operator sets, never a
    /// test-only bypass.
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
                "clientInfo": { "name": "audit-page-production-reality-test", "version": "1.0" }
            }
        }));
        self.next_id += 1;
        assert!(
            self.response().get("result").is_some(),
            "initialize must return a result"
        );
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }));
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
    /// Serves the same fixed HTML fixture for every request, always with
    /// `Server`/`X-Powered-By` response headers — real bytes over a real
    /// TCP socket, counted per request.
    fn start() -> Self {
        const BODY: &str = "<html><head><meta name=\"generator\" content=\"fixture-generator\"></head><body><img src=\"/x.png\"></body></html>";
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
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nServer: nginx/fixture\r\nX-Powered-By: fixture-runtime\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{BODY}",
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
        "spider-mcp-audit-page-production-reality-{}-{name}.sqlite3",
        std::process::id()
    ))
}

#[test]
fn spider_audit_page_is_advertised_by_tools_list() {
    let path = db_path("tools-list");
    let _ = std::fs::remove_file(&path);
    let mut client = McpClient::spawn(&path);
    client.initialize();

    let tools = client.list_tools();
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .expect("tools/list must return a tools array")
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"spider_audit_page"),
        "spider_audit_page must be advertised by the production default build's tools/list: {names:?}"
    );

    let _ = std::fs::remove_file(&path);
}

/// The full Phase 20 production-reality proof: real binary, real MCP
/// JSON-RPC, real local fixture, real durable evidence, real
/// independently-spawned second process resolving it, real second
/// sequential request keeping the same server healthy.
#[test]
fn real_binary_serves_a_real_spider_audit_page_request_end_to_end() {
    let path = db_path("end-to-end");
    let _ = std::fs::remove_file(&path);
    let fixture = AuditFixture::start();

    let mut client = McpClient::spawn(&path);
    client.initialize();

    let response = client.call("spider_audit_page", json!({ "url": fixture.url() }));
    assert_eq!(
        response["result"]["isError"], false,
        "first spider_audit_page call must succeed: {response:?}"
    );
    assert_eq!(
        fixture.hit_count(),
        1,
        "exactly one target acquisition for one spider_audit_page call"
    );

    let payload = content_payload(&response);
    let evidence_ref = payload["evidence_ref"]
        .as_str()
        .expect("response must contain a string evidence_ref")
        .to_string();
    assert!(evidence_ref.starts_with("evid_"));

    let findings = payload["findings"].as_array().unwrap();
    assert!(!findings.is_empty());
    assert!(findings
        .iter()
        .any(|f| f["rule_id"] == "seo.canonical.missing" && f["rule_version"] == 2));
    for finding in findings {
        assert_eq!(finding["evidence"][0]["id"], evidence_ref);
    }

    let markers = payload["technology_markers"].as_array().unwrap();
    assert!(markers.iter().any(
        |m| m["source"] == json!({"ResponseHeader": "server"}) && m["value"] == "nginx/fixture"
    ));
    assert!(markers.iter().any(|m| m["source"] == "HtmlMetaGenerator"));

    // Second sequential request: the server (and the shared store) remain
    // healthy — a second independent acquisition, a second independent
    // EvidenceRef.
    let second_response = client.call("spider_audit_page", json!({ "url": fixture.url() }));
    assert_eq!(second_response["result"]["isError"], false);
    assert_eq!(fixture.hit_count(), 2);
    let second_payload = content_payload(&second_response);
    let second_evidence_ref = second_payload["evidence_ref"].as_str().unwrap();
    assert_ne!(second_evidence_ref, evidence_ref);

    // `Drop::drop` kills and waits on the child — the `spider-mcp`
    // process that durably wrote both `EvidenceRef`s is confirmed fully
    // exited *before* the resolution below runs.
    drop(client);

    // Cross-process resolution: this test binary is a process wholly
    // distinct from the now-exited `spider-mcp` binary that wrote the
    // evidence. Opening `SCORPION_DOMAIN_DB`'s same file fresh here and
    // resolving both returned `EvidenceRef`s is exactly the
    // human-in-the-loop shape a future Web Console will rely on: the
    // writer process's own identity is irrelevant, only the shared
    // durable file matters.
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let store = spider::features::domain_persistence::DomainPersistence::open(&path)
            .await
            .expect("the shared store the spider-mcp process wrote must reopen cleanly");
        for reference in [&evidence_ref, second_evidence_ref] {
            let id: spider::features::identity::EvidenceId = reference
                .parse()
                .expect("evidence_ref must parse as a canonical EvidenceId");
            let resolved = spider::utils::evidence::EvidenceRef::new(id)
                .resolve(&store)
                .await
                .unwrap();
            assert!(
                resolved.is_some(),
                "{reference} must resolve from a process wholly distinct from the one that wrote it"
            );
        }
    });

    let _ = std::fs::remove_file(&path);
}
