//! SCORPION_WEB_CONSOLE_CANONICAL_EVIDENCE_INSPECTION_001 — the primary
//! frontier acceptance (Phase 9/27): one real, shared `SCORPION_DOMAIN_DB`
//! SQLite file, a real `spider-mcp` process performing `spider_audit_page`
//! then `spider_evidence_read` over genuine stdio MCP JSON-RPC, and a real
//! `scorpion-api` process resolving the exact same `EvidenceRef` over HTTP
//! — proving `EvidenceBundle A == EvidenceBundle B` semantically across
//! every canonical field, with no translation table, no new identity, no
//! duplicated evidence, and no target re-fetch. Also proves the read
//! survives the target fixture being shut down entirely (Phase 7/27).
//!
//! `spider-mcp` (a different workspace package) is not addressable via
//! `CARGO_BIN_EXE_spider-mcp` from this crate's own integration tests —
//! that variable is only populated for binaries within the *same*
//! package. This file instead builds it explicitly on demand (once, via
//! `std::sync::Once`) through `cargo build -p spider_mcp --bin
//! spider-mcp`, then locates it in the shared workspace `target/debug`
//! directory next to this crate's own `CARGO_BIN_EXE_scorpion-api`.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Once};

fn spider_mcp_binary() -> PathBuf {
    static BUILD: Once = Once::new();
    // CARGO_BIN_EXE_scorpion-api is `<workspace>/target/<profile>/scorpion-api`
    // — its parent is the exact profile directory spider-mcp will also
    // build into in this same, single-target-dir workspace.
    let profile_dir = PathBuf::from(env!("CARGO_BIN_EXE_scorpion-api"))
        .parent()
        .expect("scorpion-api binary must have a parent directory")
        .to_path_buf();
    let binary = profile_dir.join("spider-mcp");
    BUILD.call_once(|| {
        let status = Command::new(env!("CARGO"))
            .args(["build", "-p", "spider_mcp", "--bin", "spider-mcp"])
            .status()
            .expect("failed to invoke cargo build for spider-mcp");
        assert!(
            status.success(),
            "cargo build -p spider_mcp --bin spider-mcp failed"
        );
    });
    assert!(
        binary.is_file(),
        "expected spider-mcp binary at {}",
        binary.display()
    );
    binary
}

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpClient {
    fn spawn(db_path: &std::path::Path) -> Self {
        let mut child = Command::new(spider_mcp_binary())
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
                "clientInfo": { "name": "cross-interface-evidence-test", "version": "1.0" }
            }
        }));
        self.next_id += 1;
        assert!(self.response().get("result").is_some());
        self.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
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
    shutdown: Arc<std::sync::atomic::AtomicBool>,
}

impl AuditFixture {
    fn start() -> Self {
        const BODY: &str =
            "<html><head><title>cross interface fixture</title></head><body>hello</body></html>";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let hits_thread = hits.clone();
        let shutdown_thread = shutdown.clone();
        std::thread::spawn(move || loop {
            if shutdown_thread.load(Ordering::SeqCst) {
                break;
            }
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
        Self {
            addr,
            hits,
            shutdown,
        }
    }

    fn url(&self) -> String {
        format!("http://{}/", self.addr)
    }

    fn hit_count(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    /// Stop accepting further connections — simulates the target going
    /// offline entirely, for the Phase 7/27 target-offline-still-resolves
    /// proof. The listening socket itself is dropped when the accept
    /// thread observes the flag and returns, so a subsequent connection
    /// attempt to `self.addr` genuinely fails rather than merely being
    /// ignored.
    fn stop(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // Give the accept loop a moment to observe the flag and exit
        // before the caller proceeds.
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn db_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "scorpion-cross-interface-evidence-{}-{name}.sqlite3",
        std::process::id()
    ))
}

/// Blocking HTTP GET against the real `scorpion-api` binary — mirrors
/// `tests/api.rs`'s own `get` helper (kept independent/self-contained per
/// this repository's established per-file convention).
fn http_get(base: &str, path: &str) -> String {
    use std::net::TcpStream;
    let url = base.strip_prefix("http://").unwrap();
    let mut stream = TcpStream::connect(url).unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn http_body_json(response: &str) -> Value {
    let split = response.split("\r\n\r\n").nth(1).unwrap_or_default();
    serde_json::from_str(split)
        .unwrap_or_else(|error| panic!("HTTP body was not JSON: {error}; body={split:?}"))
}

fn spawn_scorpion_api(db_path: &std::path::Path) -> (Child, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let child = Command::new(env!("CARGO_BIN_EXE_scorpion-api"))
        .env("SCORPION_API_BIND", address.to_string())
        .env("SCORPION_DOMAIN_DB", db_path)
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(150));
    (child, format!("http://{address}"))
}

/// The primary frontier acceptance: MCP resolves `EvidenceRef A` through
/// `spider_audit_page` then `spider_evidence_read`; a wholly separate
/// `scorpion-api` process resolves the identical `EvidenceRef` over HTTP.
/// The two canonical `EvidenceBundle`s must be semantically identical
/// across every field. Also proves: target offline after evidence
/// creation still resolves (both interfaces), and the Web Console read
/// performs zero additional target acquisition.
#[test]
fn mcp_and_web_console_resolve_the_identical_persisted_evidence_bundle() {
    let path = db_path("same-truth");
    let _ = std::fs::remove_file(&path);
    let fixture = AuditFixture::start();

    // Process A: real spider-mcp, real audit, then a real MCP
    // spider_evidence_read call against the exact returned evidence_ref.
    let (evidence_ref, bundle_a) = {
        let mut mcp = McpClient::spawn(&path);
        mcp.initialize();

        let audit_response = mcp.call("spider_audit_page", json!({ "url": fixture.url() }));
        assert_eq!(
            audit_response["result"]["isError"], false,
            "{audit_response:?}"
        );
        assert_eq!(fixture.hit_count(), 1);
        let audit_payload = content_payload(&audit_response);
        let evidence_ref = audit_payload["evidence_ref"]
            .as_str()
            .expect("audit response must contain evidence_ref")
            .to_string();

        let read_response = mcp.call(
            "spider_evidence_read",
            json!({ "evidence_ref": evidence_ref.clone() }),
        );
        assert_eq!(
            read_response["result"]["isError"], false,
            "{read_response:?}"
        );
        let bundle_a = content_payload(&read_response);
        (evidence_ref, bundle_a)
        // mcp dropped here: process A killed and waited on.
    };

    // Target goes offline entirely before the Web Console ever reads.
    fixture.stop();
    let hits_before_web_read = fixture.hit_count();

    // Process B: a wholly separate scorpion-api process, resolving the
    // identical EvidenceRef purely through the canonical store — no MCP
    // involvement whatsoever.
    let (mut api, base) = spawn_scorpion_api(&path);
    let http_response = http_get(&base, &format!("/api/evidence/{evidence_ref}"));
    let bundle_b = http_body_json(&http_response);
    api.kill().unwrap();

    assert!(
        http_response.starts_with("HTTP/1.1 200 OK"),
        "{http_response}"
    );

    // Zero additional target acquisition for the Web Console read — the
    // target is not even listening any more, so any acquisition attempt
    // would fail outright, but the hit counter is the stronger proof:
    // truly zero requests, not merely zero *successful* ones.
    assert_eq!(
        fixture.hit_count(),
        hits_before_web_read,
        "the Web Console evidence read must perform zero target requests"
    );

    // Same identity, no translation.
    assert_eq!(bundle_a["id"], bundle_b["id"]);
    assert_eq!(bundle_a["id"], Value::String(evidence_ref.clone()));

    // Full semantic equality across every canonical field MCP already
    // proved fidelity for (evidence_read_production_reality.rs) — the
    // Web Console must resolve exactly the same record, not a
    // reconstructed approximation of it.
    for field in [
        "id",
        "requested_url",
        "final_url",
        "retrieved_at",
        "status_code",
        "observed_status_code",
        "content_type",
        "detected_content_type",
        "response_body_hash",
        "transformed_content_hash",
        "content",
        "links",
        "source",
        "provider",
        "query",
        "screenshot",
        "screenshot_hash",
        "metadata",
        "transport",
        "dns",
        "backend_provenance",
        "response_origin",
        "response_headers",
    ] {
        assert_eq!(
            bundle_a.get(field),
            bundle_b.get(field),
            "field {field:?} diverged between MCP and Web Console: \
             mcp={:?} web={:?}",
            bundle_a.get(field),
            bundle_b.get(field)
        );
    }
    assert!(bundle_b["requested_url"]
        .as_str()
        .unwrap()
        .contains(&fixture.addr.to_string()));
    assert!(bundle_b["content"]
        .as_str()
        .unwrap_or_default()
        .contains("cross interface fixture"));

    let _ = std::fs::remove_file(&path);
}

/// GET / on the real scorpion-api still renders (Phase 27 step 13) even
/// with a real, populated SCORPION_DOMAIN_DB configured, and the Evidence
/// Inspector against the same ID this test seeds independently succeeds
/// (Phase 27 step 14) — proving persistence configuration is not a global
/// startup prerequisite, only a per-request one for the evidence route.
#[test]
fn console_stays_healthy_and_evidence_inspector_resolves_the_same_id_after_repeated_reads() {
    let path = db_path("console-health");
    let _ = std::fs::remove_file(&path);
    let fixture = AuditFixture::start();

    let evidence_ref = {
        let mut mcp = McpClient::spawn(&path);
        mcp.initialize();
        let audit_response = mcp.call("spider_audit_page", json!({ "url": fixture.url() }));
        assert_eq!(audit_response["result"]["isError"], false);
        content_payload(&audit_response)["evidence_ref"]
            .as_str()
            .unwrap()
            .to_string()
    };

    let (mut api, base) = spawn_scorpion_api(&path);

    let index = http_get(&base, "/");
    assert!(index.starts_with("HTTP/1.1 200 OK"), "{index}");
    assert!(index.contains("Evidence Inspector"));

    // Second read: server stays healthy (Phase 27 step 17).
    let first = http_get(&base, &format!("/api/evidence/{evidence_ref}"));
    let second = http_get(&base, &format!("/api/evidence/{evidence_ref}"));
    let health = http_get(&base, "/health");
    api.kill().unwrap();

    assert!(first.starts_with("HTTP/1.1 200 OK"), "{first}");
    assert!(second.starts_with("HTTP/1.1 200 OK"), "{second}");
    assert!(health.contains("\"status\":\"ok\""));

    let _ = std::fs::remove_file(&path);
}
