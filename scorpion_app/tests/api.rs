use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command};
use std::thread;
use std::time::Duration;

fn fake_searxng(body: &'static str, status: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    format!("http://{address}")
}

fn api_server(searxng: Option<String>) -> (Child, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let mut command = Command::new(env!("CARGO_BIN_EXE_scorpion-api"));
    command.env("SCORPION_API_BIND", address.to_string());
    if let Some(url) = searxng {
        command.env("SEARXNG_BASE_URL", url);
    } else {
        command.env_remove("SEARXNG_BASE_URL");
    }
    let child = command.spawn().unwrap();
    thread::sleep(Duration::from_millis(100));
    (child, format!("http://{address}"))
}

fn post(base: &str, body: &str) -> String {
    post_path(base, "/api/search", body)
}

fn post_path(base: &str, path: &str, body: &str) -> String {
    let url = base.strip_prefix("http://").unwrap();
    let mut stream = TcpStream::connect(url).unwrap();
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(), body
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn research_routes_fail_closed_without_operator_configuration() {
    let (mut child, base) = api_server(None);
    let response = post_path(&base, "/api/research", r#"{"topic":"rust"}"#);
    child.kill().unwrap();
    assert!(
        response.starts_with("HTTP/1.1 503 Service Unavailable"),
        "{response}"
    );

    let (mut child, base) = api_server(None);
    let response = get(&base, "/api/research/not-a-research-id");
    child.kill().unwrap();
    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request"),
        "{response}"
    );
}

fn get(base: &str, path: &str) -> String {
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

#[test]
fn real_server_serves_search_only_console_from_same_origin() {
    let (mut child, base) = api_server(None);
    let response = get(&base, "/");
    child.kill().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("<h1>Scorpion</h1>"));
    assert!(response.contains("fetch('/api/search'"));
    assert!(response.contains("Searching…"));
    assert!(response.contains("No results found."));
    assert!(response.contains("id=\"research-form\""));
    assert!(response.contains("fetch('/api/research'"));
    assert!(response.contains("terminalResearchStates"));
    assert!(response.contains("completed_synthesis_insufficient"));
    assert!(response.contains("EvidenceIds:"));
    assert!(!response.contains("SEARXNG_BASE_URL"));
    assert!(!response.contains("OPENAI_COMPAT_API_KEY"));
}

#[test]
fn real_server_delegates_to_canonical_search_and_filters_metadata() {
    let provider = fake_searxng(
        r#"{"query":"rust","number_of_results":1,"results":[{"title":"Rust","url":"https://rust-lang.org","content":"language","score":0.8}],"answers":["internal"]}"#,
        "200 OK",
    );
    let (mut child, base) = api_server(Some(provider));
    let response = post(&base, r#"{"query":"rust","limit":1}"#);
    child.kill().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("\"result_count\":1"));
    assert!(response.contains("https://rust-lang.org"));
    assert!(!response.contains("answers"));
}

#[test]
fn backend_failure_with_empty_results_is_not_reported_as_success() {
    let provider = fake_searxng(
        r#"{"query":"rust","results":[],"unresponsive_engines":[["duckduckgo","CAPTCHA"]]}"#,
        "200 OK",
    );
    let (mut child, base) = api_server(Some(provider));
    let response = post(&base, r#"{"query":"rust"}"#);
    child.kill().unwrap();
    assert!(
        response.starts_with("HTTP/1.1 502 Bad Gateway"),
        "{response}"
    );
    assert!(response.contains("provider_unavailable"));
}

#[test]
fn genuine_empty_results_remain_successful() {
    let provider = fake_searxng(r#"{"query":"rust","results":[]}"#, "200 OK");
    let (mut child, base) = api_server(Some(provider));
    let response = post(&base, r#"{"query":"rust"}"#);
    child.kill().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("\"result_count\":0"));
}

#[test]
fn malformed_request_and_missing_provider_fail_without_200() {
    let (mut child, base) = api_server(None);
    let response = post(&base, "not-json");
    child.kill().unwrap();
    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request"),
        "{response}"
    );

    let (mut child, base) = api_server(None);
    let response = post(&base, r#"{"query":"rust"}"#);
    child.kill().unwrap();
    assert!(
        response.starts_with("HTTP/1.1 503 Service Unavailable"),
        "{response}"
    );
}
