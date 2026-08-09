#![cfg(feature = "search_searxng")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;

fn spider() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spider"))
}

#[test]
fn missing_configuration_and_provider_failure_use_stderr_and_nonzero_exit() {
    let missing = spider()
        .args(["search", "query", "--provider", "searxng"])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(missing.stdout.is_empty());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("base_url"));

    let unreachable = spider()
        .args([
            "search",
            "query",
            "--provider",
            "searxng",
            "--base-url",
            "http://127.0.0.1:1",
        ])
        .output()
        .unwrap();
    assert!(!unreachable.status.success());
    assert!(unreachable.stdout.is_empty());
    assert!(String::from_utf8_lossy(&unreachable.stderr).contains("Search request failed"));
}

#[test]
fn zero_results_are_json_stdout_with_one_provider_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let count = stream.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..count]).to_string();
        let body = r#"{"results":[]}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        request
    });

    let output = spider()
        .args([
            "search",
            "zero result query",
            "--provider",
            "searxng",
            "--base-url",
            &format!("http://{address}"),
            "--language",
            "sv",
            "--limit",
            "0",
        ])
        .output()
        .unwrap();

    assert!(output.status.success(), "{:?}", output.status);
    assert!(output.stderr.is_empty());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["result_count"], 0);
    assert_eq!(json["results"], serde_json::json!([]));

    let request = server.join().unwrap();
    assert!(request.starts_with("GET /search?"), "{request}");
    assert!(request.contains("q=zero+result+query"), "{request}");
    assert!(request.contains("format=json"), "{request}");
    assert!(request.contains("language=sv"), "{request}");
    assert!(!request.contains("limit="), "{request}");
}
