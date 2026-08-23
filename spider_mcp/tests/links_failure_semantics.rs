//! Real MCP stdio proof for `spider_links` acquisition-failure semantics.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

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
                "clientInfo": { "name": "links-failure-test", "version": "1.0" }
            }
        }));
        self.next_id += 1;
        assert!(self.response().get("result").is_some());
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }));
    }

    fn links(&mut self, url: &str) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": "spider_links",
                "arguments": { "url": url, "headless": false, "subdomains": false }
            }
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

#[test]
fn refused_links_is_a_tool_error_and_server_remains_usable() {
    let unused = TcpListener::bind("127.0.0.1:0").unwrap();
    let refused_url = format!("http://{}/", unused.local_addr().unwrap());
    drop(unused);

    let mut client = McpClient::spawn();
    client.initialize();

    let failed = client.links(&refused_url);
    assert_eq!(failed["result"]["isError"], true, "{failed:?}");
    let diagnostic = content_payload(&failed);
    assert_eq!(diagnostic["url"], refused_url);
    assert_eq!(diagnostic["count"], 0);
    assert_eq!(diagnostic["links"], json!([]));

    assert!(!client.list_tools()["result"]["tools"]
        .as_array()
        .unwrap()
        .is_empty());
}
