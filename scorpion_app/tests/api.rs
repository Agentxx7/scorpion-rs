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
    api_server_with_research(searxng, false)
}

fn api_server_with_research(searxng: Option<String>, research_configured: bool) -> (Child, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let mut command = Command::new(env!("CARGO_BIN_EXE_scorpion-api"));
    command.env("SCORPION_API_BIND", address.to_string());
    for name in [
        "RESEARCH_EVIDENCE_DB",
        "SEARCH_PROVIDER",
        "BRAVE_API_KEY",
        "SERPER_API_KEY",
        "TAVILY_API_KEY",
        "OPENAI_COMPAT_BASE_URL",
        "OPENAI_COMPAT_MODEL",
        "OPENAI_COMPAT_API_KEY",
    ] {
        command.env_remove(name);
    }
    if let Some(url) = searxng {
        command.env("SEARXNG_BASE_URL", url);
    } else {
        command.env_remove("SEARXNG_BASE_URL");
    }
    if research_configured {
        command
            .env("RESEARCH_EVIDENCE_DB", "/tmp/scorpion-web-research.sqlite")
            .env("SEARXNG_BASE_URL", "http://127.0.0.1:8080")
            .env("OPENAI_COMPAT_BASE_URL", "http://127.0.0.1:11434/v1")
            .env("OPENAI_COMPAT_MODEL", "operator-model")
            .env("OPENAI_COMPAT_API_KEY", "never-render-this-secret");
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
    assert!(response.contains("\"code\":\"research_not_configured\""));
    assert!(response.contains("\"message\":\"research is not configured\""));

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
    assert!(response.contains(">Research is not configured.</p>"));
    assert!(response.contains("id=\"research-button\" type=\"submit\" disabled"));
    assert!(response.contains("fetch('/api/research'"));
    assert!(response.contains("terminalResearchStates"));
    assert!(response.contains("completed_synthesis_insufficient"));
    assert!(response.contains("EvidenceIds:"));
    assert!(!response.contains("SEARXNG_BASE_URL"));
    assert!(!response.contains("OPENAI_COMPAT_API_KEY"));
}

#[test]
fn configured_research_is_actionable_without_rendering_configuration() {
    let (mut child, base) = api_server_with_research(None, true);
    let response = get(&base, "/");
    child.kill().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains(">Research is configured.</p>"));
    assert!(response.contains("id=\"research-button\" type=\"submit\">"));
    assert!(!response.contains("id=\"research-button\" type=\"submit\" disabled"));
    assert!(!response.contains("never-render-this-secret"));
    assert!(!response.contains("/tmp/scorpion-web-research.sqlite"));
    assert!(!response.contains("127.0.0.1:11434"));
    assert!(!response.contains("operator-model"));
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
