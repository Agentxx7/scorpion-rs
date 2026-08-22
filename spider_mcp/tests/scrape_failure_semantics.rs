//! Real MCP stdio proof for `spider_scrape` acquisition-failure semantics.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;

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
                "clientInfo": { "name": "scrape-failure-test", "version": "1.0" }
            }
        }));
        self.next_id += 1;
        assert!(self.response().get("result").is_some());
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }));
    }

    fn scrape(&mut self, url: &str, evidence: bool) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": "spider_scrape",
                "arguments": { "url": url, "return_format": "raw", "evidence": evidence }
            }
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

#[test]
fn refused_scrape_is_a_tool_error_and_server_remains_usable() {
    let unused = TcpListener::bind("127.0.0.1:0").unwrap();
    let refused_url = format!("http://{}/", unused.local_addr().unwrap());
    drop(unused);

    let mut client = McpClient::spawn();
    client.initialize();

    let failed = client.scrape(&refused_url, false);
    assert_eq!(failed["result"]["isError"], true, "{failed:?}");
    let normal: Value = serde_json::from_str(content_text(&failed)).unwrap();
    assert_eq!(normal["status_code"], 521);
    assert_eq!(normal["content"], "");
    assert_eq!(normal["provenance"]["observed_status_code"], Value::Null);
    assert_eq!(normal["provenance"]["response_origin"], Value::Null);

    let failed_with_evidence = client.scrape(&refused_url, true);
    assert_eq!(
        failed_with_evidence["result"]["isError"], true,
        "{failed_with_evidence:?}"
    );
    let evidence: Value = serde_json::from_str(content_text(&failed_with_evidence)).unwrap();
    assert_eq!(evidence["status_code"], 521);
    assert_eq!(evidence["observed_status_code"], Value::Null);
    assert_eq!(evidence["response_origin"], Value::Null);
    assert_eq!(evidence["retrieved_at"], Value::Null);
    assert_eq!(evidence["response_body_hash"], Value::Null);

    let (success_url, server) = local_http(b"<html><body>MCP remains usable</body></html>");
    let succeeded = client.scrape(&success_url, true);
    assert_eq!(succeeded["result"]["isError"], false, "{succeeded:?}");
    let evidence: Value = serde_json::from_str(content_text(&succeeded)).unwrap();
    assert_eq!(evidence["observed_status_code"], 200);
    assert!(evidence["content"]
        .as_str()
        .unwrap()
        .contains("MCP remains usable"));
    server.join().unwrap();
}
