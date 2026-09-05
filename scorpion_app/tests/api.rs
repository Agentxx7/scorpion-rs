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
        "OPENAI_COMPAT_TIMEOUT_SECS",
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

/// Spawn `scorpion-api` with an exact, explicit environment — every
/// provider/research-relevant variable is cleared first, then only the
/// given overrides are applied. Used to reproduce configuration classes
/// `api_server`/`api_server_with_research` cannot express (an operator
/// selecting a real-but-not-compiled-in provider, or an unrecognized
/// selector) without depending on process-global env var mutation.
fn api_server_with_env(env: &[(&str, &str)]) -> (Child, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let mut command = Command::new(env!("CARGO_BIN_EXE_scorpion-api"));
    command.env("SCORPION_API_BIND", address.to_string());
    for name in [
        "RESEARCH_EVIDENCE_DB",
        "SEARCH_PROVIDER",
        "SEARXNG_BASE_URL",
        "BRAVE_API_KEY",
        "SERPER_API_KEY",
        "TAVILY_API_KEY",
        "OPENAI_COMPAT_BASE_URL",
        "OPENAI_COMPAT_MODEL",
        "OPENAI_COMPAT_API_KEY",
        "OPENAI_COMPAT_TIMEOUT_SECS",
    ] {
        command.env_remove(name);
    }
    for (key, value) in env {
        command.env(key, value);
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
fn api_search_preserves_provider_score_including_values_above_one() {
    // Canonical truth: score is optional, provider-owned, unbounded (values
    // above 1.0 are valid), and Scorpion performs no normalization,
    // clamping, or filtering on it. `/api/search` must pass it through
    // unchanged, in provider order, and preserve absence as null.
    let provider = fake_searxng(
        r#"{"query":"rust","number_of_results":2,"results":[
            {"title":"High Score Result","url":"https://high.example","content":"first","score":1.5},
            {"title":"No Score Result","url":"https://none.example","content":"second"}
        ]}"#,
        "200 OK",
    );
    let (mut child, base) = api_server(Some(provider));
    let response = post(&base, r#"{"query":"rust"}"#);
    child.kill().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("\"score\":1.5"), "{response}");
    assert!(response.contains("\"score\":null"), "{response}");

    // Result ordering remains provider/canonical position order, unrelated
    // to score value.
    let high = response
        .find("High Score Result")
        .expect("first result present");
    let none = response
        .find("No Score Result")
        .expect("second result present");
    assert!(high < none, "provider order must be preserved: {response}");
}

#[test]
fn default_console_ui_hides_raw_search_score_without_replacement_metric() {
    let (mut child, base) = api_server(None);
    let response = get(&base, "/");
    child.kill().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");

    // The opaque raw provider score fragment must be gone from the
    // default human-facing rendering path.
    assert!(!response.contains("score ${result.score}"), "{response}");
    assert!(!response.contains("result.score"), "{response}");

    // No synthetic replacement metric was introduced in its place.
    for forbidden in [
        "confidence",
        "relevance",
        "match %",
        "matchScore",
        "quality",
    ] {
        assert!(
            !response.to_lowercase().contains(&forbidden.to_lowercase()),
            "unexpected synthetic metric `{forbidden}` in: {response}"
        );
    }

    // Truthful discovery presentation remains intact.
    assert!(response.contains("result.title"), "{response}");
    assert!(response.contains("result.url"), "{response}");
    assert!(response.contains("result.snippet"), "{response}");
    assert!(response.contains("result.date"), "{response}");
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

// ---------------------------------------------------------------------
// F-4: canonical provider-configuration error distinctions must survive
// the scorpion_app boundary — this shipping build compiles only
// `search_searxng` (scorpion_app/Cargo.toml), so selecting Brave/Serper/
// Tavily or an unrecognized provider name is a real, distinct
// configuration class, never a "not configured" or "upstream failure"
// misclassification.
// ---------------------------------------------------------------------

const UNSUPPORTED_KEY: &str = "sk-should-never-appear-in-any-response";
const UNSUPPORTED_OPENAI_KEY: &str = "never-render-this-secret-either";
const UNSUPPORTED_DB_PATH: &str = "/tmp/scorpion-web-research-unsupported-provider.sqlite";
const UNSUPPORTED_OPENAI_URL: &str = "http://127.0.0.1:11434/v1";

#[test]
fn unsupported_search_provider_is_neither_not_configured_nor_upstream_failure() {
    let (mut child, base) = api_server_with_env(&[
        ("SEARCH_PROVIDER", "brave"),
        ("BRAVE_API_KEY", UNSUPPORTED_KEY),
    ]);
    let response = post(&base, r#"{"query":"rust"}"#);
    child.kill().unwrap();

    // Static configuration/build failure: 503, never 502 (upstream failure
    // — the provider was never executable in this build) and never a bare
    // "provider_not_configured" (the key IS present; the build just does
    // not compile Brave).
    assert!(
        response.starts_with("HTTP/1.1 503 Service Unavailable"),
        "{response}"
    );
    assert!(
        response.contains("\"code\":\"provider_unsupported\""),
        "{response}"
    );
    assert!(!response.contains("provider_not_configured"), "{response}");
    assert!(!response.contains("provider_unavailable"), "{response}");
    assert!(!response.contains(UNSUPPORTED_KEY), "{response}");
}

#[test]
fn unsupported_serper_and_tavily_search_providers_are_also_unsupported_not_missing() {
    for (provider, key_var) in [("serper", "SERPER_API_KEY"), ("tavily", "TAVILY_API_KEY")] {
        let (mut child, base) =
            api_server_with_env(&[("SEARCH_PROVIDER", provider), (key_var, UNSUPPORTED_KEY)]);
        let response = post(&base, r#"{"query":"rust"}"#);
        child.kill().unwrap();
        assert!(
            response.starts_with("HTTP/1.1 503 Service Unavailable"),
            "{provider}: {response}"
        );
        assert!(
            response.contains("\"code\":\"provider_unsupported\""),
            "{provider}: {response}"
        );
        assert!(
            !response.contains("provider_not_configured"),
            "{provider}: {response}"
        );
        assert!(
            !response.contains("provider_unavailable"),
            "{provider}: {response}"
        );
    }
}

#[test]
fn unknown_search_provider_selector_is_a_truthful_static_configuration_error() {
    let (mut child, base) = api_server_with_env(&[("SEARCH_PROVIDER", "scorpion-does-not-exist")]);
    let response = post(&base, r#"{"query":"rust"}"#);
    child.kill().unwrap();
    assert!(
        response.starts_with("HTTP/1.1 503 Service Unavailable"),
        "{response}"
    );
    assert!(
        response.contains("\"code\":\"invalid_provider_configuration\""),
        "{response}"
    );
    assert!(!response.contains("provider_unavailable"), "{response}");
}

#[test]
fn valid_searxng_search_configuration_is_unaffected() {
    let provider = fake_searxng(r#"{"query":"rust","results":[]}"#, "200 OK");
    let (mut child, base) = api_server(Some(provider));
    let response = post(&base, r#"{"query":"rust"}"#);
    child.kill().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
}

/// The primary F-4 adversarial acceptance case: for the exact same static
/// configuration (Brave selected, key present, SearXNG-only shipping
/// build), Search, Research availability (GET /), and Research submission
/// (POST /api/research) must all agree on the root configuration class —
/// none of them may say "not configured" or "unavailable".
#[test]
fn unsupported_research_provider_agrees_across_ui_availability_and_execution() {
    let env: &[(&str, &str)] = &[
        ("SEARCH_PROVIDER", "brave"),
        ("BRAVE_API_KEY", UNSUPPORTED_KEY),
        ("RESEARCH_EVIDENCE_DB", UNSUPPORTED_DB_PATH),
        ("OPENAI_COMPAT_BASE_URL", UNSUPPORTED_OPENAI_URL),
        ("OPENAI_COMPAT_MODEL", "operator-model"),
        ("OPENAI_COMPAT_API_KEY", UNSUPPORTED_OPENAI_KEY),
    ];

    let (mut child, base) = api_server_with_env(env);
    let index = get(&base, "/");
    child.kill().unwrap();
    assert!(index.starts_with("HTTP/1.1 200 OK"), "{index}");
    // Truthful: Research is disabled...
    assert!(
        index.contains("id=\"research-button\" type=\"submit\" disabled"),
        "{index}"
    );
    // ...but not falsely claimed as simply "not configured" (a key IS
    // present) nor claimed as available.
    assert!(
        !index.contains(">Research is not configured.</p>"),
        "{index}"
    );
    assert!(!index.contains(">Research is configured.</p>"), "{index}");
    assert!(index.contains("not supported by this build"), "{index}");
    assert!(!index.contains(UNSUPPORTED_KEY), "{index}");
    assert!(!index.contains(UNSUPPORTED_OPENAI_KEY), "{index}");
    assert!(!index.contains(UNSUPPORTED_DB_PATH), "{index}");

    let (mut child, base) = api_server_with_env(env);
    let submit = post_path(&base, "/api/research", r#"{"topic":"rust"}"#);
    child.kill().unwrap();
    // Same root configuration class as the UI/availability projection:
    // static configuration failure (503), the unsupported-provider code —
    // never the runtime-failure class (research_unavailable/502) and
    // never research_not_configured.
    assert!(
        submit.starts_with("HTTP/1.1 503 Service Unavailable"),
        "{submit}"
    );
    assert!(
        submit.contains("\"code\":\"research_provider_unsupported\""),
        "{submit}"
    );
    assert!(!submit.contains("research_not_configured"), "{submit}");
    assert!(!submit.contains("research_unavailable"), "{submit}");
    assert!(!submit.contains(UNSUPPORTED_KEY), "{submit}");
    assert!(!submit.contains(UNSUPPORTED_OPENAI_KEY), "{submit}");
    assert!(!submit.contains(UNSUPPORTED_DB_PATH), "{submit}");
}

#[test]
fn invalid_research_provider_selector_agrees_across_ui_and_execution() {
    let env: &[(&str, &str)] = &[
        ("SEARCH_PROVIDER", "scorpion-does-not-exist"),
        ("RESEARCH_EVIDENCE_DB", UNSUPPORTED_DB_PATH),
        ("OPENAI_COMPAT_BASE_URL", UNSUPPORTED_OPENAI_URL),
        ("OPENAI_COMPAT_MODEL", "operator-model"),
        ("OPENAI_COMPAT_API_KEY", UNSUPPORTED_OPENAI_KEY),
    ];

    let (mut child, base) = api_server_with_env(env);
    let index = get(&base, "/");
    child.kill().unwrap();
    assert!(
        index.contains("id=\"research-button\" type=\"submit\" disabled"),
        "{index}"
    );
    assert!(
        !index.contains(">Research is not configured.</p>"),
        "{index}"
    );
    assert!(!index.contains(">Research is configured.</p>"), "{index}");
    assert!(index.contains("invalid"), "{index}");

    let (mut child, base) = api_server_with_env(env);
    let submit = post_path(&base, "/api/research", r#"{"topic":"rust"}"#);
    child.kill().unwrap();
    assert!(
        submit.starts_with("HTTP/1.1 503 Service Unavailable"),
        "{submit}"
    );
    assert!(
        submit.contains("\"code\":\"research_provider_configuration_invalid\""),
        "{submit}"
    );
    assert!(!submit.contains("research_not_configured"), "{submit}");
    assert!(!submit.contains("research_unavailable"), "{submit}");
}

// SCORPION_WEB_RESEARCH_CONFIGURABLE_LLM_TIMEOUT_001: a present-but-
// malformed OPENAI_COMPAT_TIMEOUT_SECS is a static configuration failure,
// same class as an invalid provider selector above — never "not
// configured" (the setting is present) and never the runtime-failure
// class. UI availability and POST /api/research must agree.
#[test]
fn invalid_openai_timeout_agrees_across_ui_and_execution() {
    let env: &[(&str, &str)] = &[
        ("SEARXNG_BASE_URL", "http://127.0.0.1:8080"),
        ("RESEARCH_EVIDENCE_DB", UNSUPPORTED_DB_PATH),
        ("OPENAI_COMPAT_BASE_URL", UNSUPPORTED_OPENAI_URL),
        ("OPENAI_COMPAT_MODEL", "operator-model"),
        ("OPENAI_COMPAT_API_KEY", UNSUPPORTED_OPENAI_KEY),
        ("OPENAI_COMPAT_TIMEOUT_SECS", "not-a-number"),
    ];

    let (mut child, base) = api_server_with_env(env);
    let index = get(&base, "/");
    child.kill().unwrap();
    assert!(
        index.contains("id=\"research-button\" type=\"submit\" disabled"),
        "{index}"
    );
    assert!(
        !index.contains(">Research is not configured.</p>"),
        "{index}"
    );
    assert!(!index.contains(">Research is configured.</p>"), "{index}");
    assert!(index.contains("invalid"), "{index}");
    assert!(!index.contains("not-a-number"), "{index}");

    let (mut child, base) = api_server_with_env(env);
    let submit = post_path(&base, "/api/research", r#"{"topic":"rust"}"#);
    child.kill().unwrap();
    assert!(
        submit.starts_with("HTTP/1.1 503 Service Unavailable"),
        "{submit}"
    );
    assert!(
        submit.contains("\"code\":\"research_configuration_invalid\""),
        "{submit}"
    );
    assert!(!submit.contains("research_not_configured"), "{submit}");
    assert!(!submit.contains("research_unavailable"), "{submit}");
    assert!(!submit.contains("not-a-number"), "{submit}");
    assert!(!submit.contains(UNSUPPORTED_OPENAI_KEY), "{submit}");
    assert!(!submit.contains(UNSUPPORTED_DB_PATH), "{submit}");
}

// A valid OPENAI_COMPAT_TIMEOUT_SECS must not disable Research — the same
// static configuration otherwise accepted by
// `valid_research_configuration_remains_enabled` below, plus an explicit
// operator timeout, must still report Research as configured.
#[test]
fn valid_openai_timeout_does_not_disable_research() {
    let (mut child, base) = api_server_with_env(&[
        ("SEARXNG_BASE_URL", "http://127.0.0.1:8080"),
        (
            "RESEARCH_EVIDENCE_DB",
            "/tmp/scorpion-web-research-timeout.sqlite",
        ),
        ("OPENAI_COMPAT_BASE_URL", "http://127.0.0.1:11434/v1"),
        ("OPENAI_COMPAT_MODEL", "operator-model"),
        ("OPENAI_COMPAT_API_KEY", "never-render-this-secret"),
        ("OPENAI_COMPAT_TIMEOUT_SECS", "300"),
    ]);
    let index = get(&base, "/");
    child.kill().unwrap();
    assert!(index.contains(">Research is configured.</p>"), "{index}");
    assert!(
        !index.contains("id=\"research-button\" type=\"submit\" disabled"),
        "{index}"
    );
}

/// SCORPION_WEB_RESEARCH_CONFIGURABLE_LLM_TIMEOUT_001, Phase 5 boundary
/// proof: an explicitly configured OPENAI_COMPAT_TIMEOUT_SECS is actually
/// applied to Research's real Agent HTTP client, through the existing
/// canonical execution path — not merely parsed and discarded. A local
/// OpenAI-compatible fixture never responds within the deliberately tiny
/// configured timeout; Research must reach a terminal state quickly (well
/// under the old 60-second default) rather than hanging until the fixture
/// eventually would respond. Uses the real `POST /api/research` /
/// `GET /api/research/{id}` routes and the real `claim_durable_research`
/// execution path — no independently-created reqwest client, no second
/// Agent/LLM implementation.
#[test]
fn configured_openai_timeout_is_actually_applied_to_research_execution() {
    // A trivial real page for acquisition/extraction to succeed against,
    // so execution genuinely reaches the synthesis step this test targets.
    let page_body = "<html><body><h1>Rust</h1><p>A systems programming language focused on safety and performance.</p></body></html>";
    let page_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let page_addr = page_listener.local_addr().unwrap();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = page_listener.accept() {
            let mut buf = [0_u8; 4096];
            let _ = stream.read(&mut buf);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page_body}",
                page_body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    let page_url = format!("http://{page_addr}/");

    let searxng_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let searxng_addr = searxng_listener.local_addr().unwrap();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = searxng_listener.accept() {
            let mut buf = [0_u8; 4096];
            let _ = stream.read(&mut buf);
            let body = format!(
                r#"{{"query":"rust","results":[{{"title":"Rust","url":"{page_url}","content":"Rust is a systems programming language."}}]}}"#
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    let searxng_url = format!("http://{searxng_addr}");

    // Deliberately never responds within this test's bound: accepts the
    // connection, reads the request, then sleeps well past the configured
    // 1-second timeout before ever writing a response.
    let openai_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let openai_addr = openai_listener.local_addr().unwrap();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = openai_listener.accept() {
            let mut buf = [0_u8; 8192];
            let _ = stream.read(&mut buf);
            thread::sleep(Duration::from_secs(20));
            let body = r#"{"choices":[{"message":{"content":"too late to matter"}}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    let openai_url = format!("http://{openai_addr}/v1");

    let database = std::env::temp_dir().join(format!(
        "scorpion-app-research-timeout-boundary-{}.sqlite3",
        std::process::id()
    ));
    let database_str = database.to_string_lossy().to_string();

    let (mut child, base) = api_server_with_env(&[
        ("SEARXNG_BASE_URL", &searxng_url),
        ("RESEARCH_EVIDENCE_DB", &database_str),
        ("OPENAI_COMPAT_BASE_URL", &openai_url),
        ("OPENAI_COMPAT_MODEL", "test-model"),
        ("OPENAI_COMPAT_API_KEY", "test-key"),
        ("OPENAI_COMPAT_TIMEOUT_SECS", "1"),
    ]);
    let submit = post_path(&base, "/api/research", r#"{"topic":"rust programming"}"#);
    assert!(submit.starts_with("HTTP/1.1 202 Accepted"), "{submit}");
    let research_id = submit
        .rsplit("\"research_id\":\"")
        .next()
        .and_then(|rest| rest.split('"').next())
        .expect("research_id in submit response")
        .to_string();

    // The old default is 60s; this bound proves the 1-second override was
    // genuinely applied, not silently ignored -- generous enough to absorb
    // real search/acquisition/extraction work, but far short of 60s.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut terminal_state: Option<String> = None;
    while std::time::Instant::now() < deadline {
        let status = get(&base, &format!("/api/research/{research_id}"));
        if let Some(state) = status
            .rsplit("\"state\":\"")
            .next()
            .and_then(|rest| rest.split('"').next())
        {
            if state != "claimed" {
                terminal_state = Some(state.to_string());
                break;
            }
        }
        thread::sleep(Duration::from_millis(200));
    }
    child.kill().unwrap();
    let _ = std::fs::remove_file(&database);
    let _ = std::fs::remove_file(format!("{database_str}-shm"));
    let _ = std::fs::remove_file(format!("{database_str}-wal"));

    let terminal_state = terminal_state.unwrap_or_else(|| {
        panic!(
            "research did not reach a terminal state within 30s -- the configured 1s \
             OPENAI_COMPAT_TIMEOUT_SECS was not applied (it would otherwise have hung \
             toward the old 60s default or the fixture's 20s delay)"
        )
    });
    // `research_session.rs` documents extraction itself as reaching the LLM
    // stage (a per-source extraction call, distinct from the final
    // synthesis call) -- both use the same configured OpenAI-compatible
    // client, so a uniformly-too-short 1s timeout cuts off extraction's own
    // call before synthesis is ever attempted, landing on
    // `completed_no_extractions` rather than `completed_synthesis_failed`.
    // Either is valid proof for what this test asserts: the terminal state
    // was reached quickly (the deadline loop above already proved that),
    // not that the timeout happened to cut off one specific pipeline stage
    // over another.
    assert!(
        matches!(
            terminal_state.as_str(),
            "completed_no_extractions" | "completed_synthesis_failed"
        ),
        "expected a terminal state reachable only once the LLM-calling stage(s) genuinely \
         timed out at ~1s, got {terminal_state:?}"
    );
}

#[test]
fn valid_research_configuration_remains_enabled() {
    let (mut child, base) = api_server_with_research(None, true);
    let index = get(&base, "/");
    child.kill().unwrap();
    assert!(index.contains(">Research is configured.</p>"), "{index}");
    assert!(
        !index.contains("id=\"research-button\" type=\"submit\" disabled"),
        "{index}"
    );
}

// ---------------------------------------------------------------------
// F-5: raw internal provider/runtime error strings must never cross the
// public application boundary. `SEARXNG_BASE_URL` below is a real
// operator-configuration value carrying credentials, a hostname, a path,
// and a query token — the canonical SearxngProvider embeds this exact
// string verbatim in its internal `ProviderError` for an unsupported
// scheme (`spider_search/src/providers/searxng.rs:search_endpoint`),
// which previously reached the public API/UI via
// `SearchError::Provider(error.to_string())`. The invalid `ftp://` scheme
// makes this fail deterministically and instantly, with no real network
// I/O — no external service health required.
// ---------------------------------------------------------------------
const SENTINEL_OPERATOR_URL: &str = "ftp://apikey_API_KEY_SENTINEL_91CA:secret@\
    OPERATOR_URL_SENTINEL_8B16.example.invalid:9443\
    /PATH_SENTINEL_F043?token=QUERY_SENTINEL_77E2";

#[test]
fn provider_runtime_failure_never_leaks_operator_configuration_sentinels() {
    let (mut child, base) = api_server(Some(SENTINEL_OPERATOR_URL.to_string()));
    let response = post(&base, r#"{"query":"rust"}"#);
    child.kill().unwrap();

    // Truthful, stable classification preserved: a runtime provider
    // failure remains 502/provider_unavailable — unchanged by this
    // correction, and distinct from every F-4 static configuration class.
    assert!(
        response.starts_with("HTTP/1.1 502 Bad Gateway"),
        "{response}"
    );
    assert!(
        response.contains("\"code\":\"provider_unavailable\""),
        "{response}"
    );
    assert!(!response.contains("provider_not_configured"), "{response}");
    assert!(!response.contains("provider_unsupported"), "{response}");

    // None of the sentinels embedded in the operator-configured URL —
    // API key/userinfo, hostname, path, or query token — may appear
    // anywhere in the public response the Web Console also renders
    // verbatim (payload.error.message).
    for sentinel in [
        "API_KEY_SENTINEL_91CA",
        "apikey_API_KEY_SENTINEL_91CA:secret",
        "OPERATOR_URL_SENTINEL_8B16",
        "PATH_SENTINEL_F043",
        "QUERY_SENTINEL_77E2",
        "9443",
        "ftp://",
    ] {
        assert!(
            !response.contains(sentinel),
            "leaked `{sentinel}` in: {response}"
        );
    }

    // The public message is the fixed, sanitized, deterministic string —
    // the same one the Web Console will display unmodified.
    assert!(
        response.contains("\"message\":\"search provider failed: search provider is unavailable\""),
        "{response}"
    );
}

// ---------------------------------------------------------------------
// SCORPION_WEB_CONSOLE_CANONICAL_EVIDENCE_INSPECTION_001
//
// GET /api/evidence/{evidence_ref} acceptance matrix (Phase 20) against
// the real, compiled `scorpion-api` binary. Evidence is seeded directly
// through the canonical `spider::utils::evidence::record_evidence` seam
// against a real on-disk SQLite file — never through the HTTP API itself
// (there is no write path here to seed through) — then resolved purely
// over HTTP, proving the same canonical `EvidenceBundle` a durable write
// produced is exactly what the read boundary returns.
// ---------------------------------------------------------------------

fn evidence_db_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "scorpion-api-evidence-route-test-{}-{name}.sqlite3",
        std::process::id()
    ))
}

/// Record one representative `EvidenceBundle` directly through the
/// canonical seam (not through HTTP) and return its assigned `EvidenceId`
/// string. Uses a brand-new tokio runtime since `tests/api.rs` itself is
/// not an async test file.
fn seed_evidence(path: &std::path::Path) -> String {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let store = spider::features::domain_persistence::DomainPersistence::open(path)
            .await
            .unwrap();
        let mut response_headers = std::collections::BTreeMap::new();
        response_headers.insert("server".to_string(), vec![b"nginx/fixture".to_vec()]);
        let bundle = spider::utils::evidence::EvidenceBundle {
            requested_url: Some("https://example.test/evidence-route".to_string()),
            final_url: Some("https://example.test/evidence-route".to_string()),
            retrieved_at: Some(1_700_000_000_000),
            status_code: Some(200),
            observed_status_code: Some(200),
            content_type: Some("text/html; charset=utf-8".to_string()),
            content: Some("<html><body>hello evidence route</body></html>".to_string()),
            transport: Some("default".to_string()),
            backend_provenance: Some("reqwest".to_string()),
            response_origin: Some("network".to_string()),
            response_headers: Some(response_headers),
            ..Default::default()
        };
        let recorded = spider::utils::evidence::record_evidence(&store, bundle)
            .await
            .unwrap();
        recorded.id.unwrap().to_string()
    })
}

fn evidence_env(db_path: &std::path::Path) -> Vec<(&'static str, String)> {
    vec![("SCORPION_DOMAIN_DB", db_path.to_string_lossy().to_string())]
}

#[test]
fn valid_evidence_ref_returns_the_exact_canonical_bundle() {
    let path = evidence_db_path("valid");
    let _ = std::fs::remove_file(&path);
    let evidence_ref = seed_evidence(&path);

    let env = evidence_env(&path);
    let env_refs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (mut child, base) = api_server_with_env(&env_refs);
    let response = get(&base, &format!("/api/evidence/{evidence_ref}"));
    child.kill().unwrap();

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(
        response.contains(&format!("\"id\":\"{evidence_ref}\"")),
        "{response}"
    );
    assert!(response.contains("evidence-route"));
    assert!(response.contains("hello evidence route"));
    assert!(response.contains("\"backend_provenance\":\"reqwest\""));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn malformed_evidence_ref_is_400() {
    let path = evidence_db_path("malformed");
    let _ = std::fs::remove_file(&path);
    let _ = seed_evidence(&path);

    let env = evidence_env(&path);
    let env_refs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (mut child, base) = api_server_with_env(&env_refs);
    let response = get(&base, "/api/evidence/not-a-real-evidence-ref");
    child.kill().unwrap();

    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request"),
        "{response}"
    );
    assert!(response.contains("\"code\":\"invalid_evidence_reference\""));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn valid_but_absent_evidence_ref_is_404() {
    let path = evidence_db_path("absent");
    let _ = std::fs::remove_file(&path);
    let _ = seed_evidence(&path);

    let env = evidence_env(&path);
    let env_refs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (mut child, base) = api_server_with_env(&env_refs);
    let response = get(&base, "/api/evidence/evid_0123456789abcdef0123456789abcdef");
    child.kill().unwrap();

    assert!(response.starts_with("HTTP/1.1 404 Not Found"), "{response}");
    assert!(response.contains("\"code\":\"evidence_not_found\""));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn evidence_store_not_configured_is_deterministic_503() {
    let (mut child, base) = api_server(None);
    let response = get(&base, "/api/evidence/evid_0123456789abcdef0123456789abcdef");
    child.kill().unwrap();

    assert!(
        response.starts_with("HTTP/1.1 503 Service Unavailable"),
        "{response}"
    );
    assert!(response.contains("\"code\":\"evidence_store_not_configured\""));
    // No filesystem/persistence diagnostic leakage.
    assert!(!response.contains(".sqlite"));
}

#[test]
fn evidence_store_unavailable_is_deterministic_and_does_not_leak_the_path() {
    let dir = std::env::temp_dir().join(format!(
        "scorpion-api-evidence-route-unopenable-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();

    let dir_string = dir.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &dir_string)]);
    let response = get(&base, "/api/evidence/evid_0123456789abcdef0123456789abcdef");
    child.kill().unwrap();

    assert!(
        response.starts_with("HTTP/1.1 503 Service Unavailable"),
        "{response}"
    );
    assert!(response.contains("\"code\":\"evidence_store_unavailable\""));
    assert!(!response.contains(&dir_string));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn two_sequential_reads_return_identical_results_and_never_mutate_history() {
    let path = evidence_db_path("sequential");
    let _ = std::fs::remove_file(&path);
    let evidence_ref = seed_evidence(&path);

    let history_before = {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let store = spider::features::domain_persistence::DomainPersistence::open(&path)
                .await
                .unwrap();
            store.read_history(&evidence_ref).await.unwrap()
        })
    };

    let env = evidence_env(&path);
    let env_refs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (mut child, base) = api_server_with_env(&env_refs);
    let first = get(&base, &format!("/api/evidence/{evidence_ref}"));
    let second = get(&base, &format!("/api/evidence/{evidence_ref}"));
    child.kill().unwrap();

    assert!(first.starts_with("HTTP/1.1 200 OK"), "{first}");
    assert_eq!(first, second, "repeated reads must return identical output");

    let history_after = {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let store = spider::features::domain_persistence::DomainPersistence::open(&path)
                .await
                .unwrap();
            store.read_history(&evidence_ref).await.unwrap()
        })
    };
    assert_eq!(
        history_before, history_after,
        "reading evidence over HTTP must never mutate persisted history"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn concurrent_evidence_reads_both_succeed() {
    let path = evidence_db_path("concurrent");
    let _ = std::fs::remove_file(&path);
    let evidence_ref = seed_evidence(&path);

    let env = evidence_env(&path);
    let env_refs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (mut child, base) = api_server_with_env(&env_refs);

    let base_a = base.clone();
    let ref_a = evidence_ref.clone();
    let handle_a = thread::spawn(move || get(&base_a, &format!("/api/evidence/{ref_a}")));
    let base_b = base.clone();
    let ref_b = evidence_ref.clone();
    let handle_b = thread::spawn(move || get(&base_b, &format!("/api/evidence/{ref_b}")));

    let response_a = handle_a.join().unwrap();
    let response_b = handle_b.join().unwrap();
    child.kill().unwrap();

    assert!(response_a.starts_with("HTTP/1.1 200 OK"), "{response_a}");
    assert!(response_b.starts_with("HTTP/1.1 200 OK"), "{response_b}");

    let _ = std::fs::remove_file(&path);
}

/// The one canonical route that names durable evidence must never be
/// confused with an unrelated 404. This mirrors the F-4/F-5 discipline
/// already established for search/research error classes: none of the
/// evidence error codes should ever collapse into the generic
/// `not_found` route-miss response the un-matched-route fallback uses.
#[test]
fn evidence_route_error_codes_never_collide_with_the_generic_route_not_found_code() {
    let (mut child, base) = api_server(None);
    let response = get(&base, "/api/evidence/evid_0123456789abcdef0123456789abcdef");
    child.kill().unwrap();
    assert!(!response.contains("\"code\":\"not_found\""), "{response}");
}

#[test]
fn console_serves_the_evidence_inspector_section() {
    let (mut child, base) = api_server(None);
    let response = get(&base, "/");
    child.kill().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("id=\"evidence-heading\""));
    assert!(response.contains("id=\"evidence-form\""));
    assert!(response.contains("id=\"evidence-ref\""));
    assert!(response.contains("id=\"evidence-button\" type=\"submit\">Inspect evidence</button>"));
    assert!(response.contains("fetch(`/api/evidence/"));
    // The evidence UI must never require Search/Research configuration —
    // it is always rendered, independent of ResearchAvailability (Phase 17).
    assert!(response.contains("Evidence Inspector"));
}

/// Phase 12/21 XSS-firewall proof, HTTP half: hostile stored evidence
/// content survives the read path completely unmodified — the server
/// never strips, escapes, or otherwise interprets it as markup. JSON
/// string encoding is not HTML-escaping (the literal bytes
/// `<script>...`/`<img onerror=...>` appear verbatim inside the quoted
/// JSON string value); the guarantee that the browser never executes
/// them comes from the structural proof that the console's own
/// rendering script contains no unsafe DOM-injection primitive at all
/// (`spider/tests/architecture_guardrails.rs`'s
/// `web_console_never_uses_unsafe_dom_injection_primitives`) — together
/// these two proofs cover both ends of the inert-rendering contract.
#[test]
fn hostile_stored_content_survives_the_read_path_completely_unmodified() {
    let path = evidence_db_path("hostile");
    let _ = std::fs::remove_file(&path);

    const HOSTILE: &str = "<script>alert(\"scorpion-xss\")</script><img src=x onerror=alert(1)>";
    let evidence_ref = {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let store = spider::features::domain_persistence::DomainPersistence::open(&path)
                .await
                .unwrap();
            let bundle = spider::utils::evidence::EvidenceBundle {
                requested_url: Some("https://example.test/hostile".to_string()),
                status_code: Some(200),
                content: Some(HOSTILE.to_string()),
                ..Default::default()
            };
            let recorded = spider::utils::evidence::record_evidence(&store, bundle)
                .await
                .unwrap();
            recorded.id.unwrap().to_string()
        })
    };

    let env = evidence_env(&path);
    let env_refs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (mut child, base) = api_server_with_env(&env_refs);
    let response = get(&base, &format!("/api/evidence/{evidence_ref}"));
    child.kill().unwrap();

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    // Verbatim survival — not stripped, not neutered, not re-encoded.
    assert!(
        response.contains("<script>alert(\\\"scorpion-xss\\\")</script>")
            || response.contains(HOSTILE),
        "{response}"
    );
    assert!(response.contains("onerror=alert(1)"), "{response}");

    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------
// SCORPION_WEB_CONSOLE_CANONICAL_PAGE_AUDIT_EXECUTION_001
//
// POST /api/audit acceptance matrix (Phase 17) against the real,
// compiled `scorpion-api` binary. Unlike the evidence-route tests
// above, audit execution genuinely acquires its own target through a
// real local fixture — there is no separate seeding step.
// ---------------------------------------------------------------------

fn audit_db_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "scorpion-api-audit-route-test-{}-{name}.sqlite3",
        std::process::id()
    ))
}

/// A tiny blocking local HTTP fixture for audit acceptance tests —
/// counts every accepted request, and can serve a fixed status/
/// content-type/body/extra-headers combination.
struct AuditRouteFixture {
    addr: std::net::SocketAddr,
    hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl AuditRouteFixture {
    fn start(
        status: &'static str,
        content_type: &'static str,
        body: &'static str,
        extra_headers: &'static str,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hits_thread = hits.clone();
        thread::spawn(move || loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buf = [0_u8; 4096];
                    let _ = stream.read(&mut buf);
                    hits_thread.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
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
        self.hits.load(std::sync::atomic::Ordering::SeqCst)
    }
}

const AUDIT_MINIMAL_HTML: &str =
    "<html><head><title>t</title></head><body><h1>hi</h1></body></html>";

fn post_audit(base: &str, url: &str) -> String {
    post_path(base, "/api/audit", &format!("{{\"url\":\"{url}\"}}"))
}

#[test]
fn valid_html_audit_returns_200_with_canonical_findings_and_ref() {
    let path = audit_db_path("valid");
    let _ = std::fs::remove_file(&path);
    let fixture = AuditRouteFixture::start("200 OK", "text/html", AUDIT_MINIMAL_HTML, "");

    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);
    let response = post_audit(&base, &fixture.url());
    child.kill().unwrap();

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("\"outcome\":\"evaluated\""));
    assert!(response.contains("\"evidence_ref\":\"evid_"));
    assert!(response.contains("\"findings\":["));
    assert!(response.contains("\"technology_markers\":["));
    assert_eq!(fixture.hit_count(), 1);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn empty_audit_url_is_400() {
    let path = audit_db_path("empty-url");
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);
    let response = post_audit(&base, "");
    child.kill().unwrap();
    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request"),
        "{response}"
    );
    assert!(response.contains("\"code\":\"invalid_request\""));
}

#[test]
fn whitespace_audit_url_is_400() {
    let path = audit_db_path("ws-url");
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);
    let response = post_audit(&base, "   ");
    child.kill().unwrap();
    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request"),
        "{response}"
    );
}

#[test]
fn malformed_audit_url_fails_deterministically() {
    let path = audit_db_path("malformed-url");
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);
    let response = post_audit(&base, "not a url");
    child.kill().unwrap();
    // Rejected by the canonical engine *before* any acquisition — a
    // client-correctable 400 with its own stable code, never a 502
    // acquisition failure (which now means a real pre-response
    // acquisition breakdown only) and never a fabricated 200.
    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request"),
        "{response}"
    );
    assert!(response.contains("\"code\":\"invalid_audit_target\""));
}

#[test]
fn non_http_scheme_audit_target_is_400() {
    let path = audit_db_path("ftp-target");
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);
    let response = post_audit(&base, "ftp://example.com/");
    child.kill().unwrap();
    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request"),
        "{response}"
    );
    assert!(response.contains("\"code\":\"invalid_audit_target\""));
}

/// BUG-1 regression: an unobserved target (valid HTTP URL, nothing
/// listening) completes truthfully as HTTP 200 with an explicit
/// `target_unobserved` outcome and zero findings — never the
/// indistinguishable "evaluated with zero findings" shape.
#[test]
fn unobserved_audit_target_is_200_with_target_unobserved_outcome() {
    let path = audit_db_path("unobserved");
    let _ = std::fs::remove_file(&path);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);
    let response = post_audit(&base, &format!("http://{addr}/"));
    child.kill().unwrap();

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(
        response.contains("\"outcome\":\"target_unobserved\""),
        "{response}"
    );
    assert!(response.contains("\"findings\":[]"), "{response}");
    assert!(response.contains("\"technology_markers\":[]"), "{response}");
    assert!(response.contains("\"evidence_ref\":\"evid_"), "{response}");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn missing_domain_store_config_fails_closed_but_server_still_starts() {
    let (mut child, base) = api_server(None);
    let index = get(&base, "/");
    assert!(
        index.starts_with("HTTP/1.1 200 OK"),
        "server must still start: {index}"
    );
    let response = post_audit(&base, "http://127.0.0.1:1/");
    child.kill().unwrap();
    assert!(
        response.starts_with("HTTP/1.1 503 Service Unavailable"),
        "{response}"
    );
    assert!(response.contains("\"code\":\"audit_store_not_configured\""));
}

#[test]
fn unavailable_audit_store_is_sanitized_and_does_not_leak_the_path() {
    let dir = std::env::temp_dir().join(format!(
        "scorpion-api-audit-route-unopenable-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let dir_string = dir.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &dir_string)]);
    let response = post_audit(&base, "http://127.0.0.1:1/");
    child.kill().unwrap();
    assert!(
        response.starts_with("HTTP/1.1 503 Service Unavailable"),
        "{response}"
    );
    assert!(response.contains("\"code\":\"audit_store_unavailable\""));
    assert!(!response.contains(&dir_string));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn declared_text_plain_yields_no_html_only_seo_findings_or_generator_marker() {
    let path = audit_db_path("text-plain");
    let _ = std::fs::remove_file(&path);
    let fixture = AuditRouteFixture::start(
        "200 OK",
        "text/plain",
        "<html><head><title>t</title><meta name=\"generator\" content=\"WordPress\"></head><body><h1>hi</h1></body></html>",
        "",
    );
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);
    let response = post_audit(&base, &fixture.url());
    child.kill().unwrap();

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    // No SEO findings that require HTML representation (title/canonical/
    // h1/etc.) may fire against a declared text/plain body.
    assert!(!response.contains("seo.title.missing"));
    assert!(!response.contains("seo.canonical.missing"));
    assert!(!response.contains("HtmlMetaGenerator"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn non_2xx_html_response_follows_canonical_audit_applicability() {
    let path = audit_db_path("404-html");
    let _ = std::fs::remove_file(&path);
    let fixture = AuditRouteFixture::start("404 Not Found", "text/html", AUDIT_MINIMAL_HTML, "");
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);
    let response = post_audit(&base, &fixture.url());
    child.kill().unwrap();

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    // 2xx-gated SEO applicability rules (e.g. canonical-missing) must not
    // fire against a non-2xx response — matches canonical audit_page's
    // own applicability semantics exactly (never re-derived here).
    assert!(!response.contains("seo.canonical.missing"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn wp_content_url_alone_infers_zero_technology() {
    let path = audit_db_path("wp-content-url");
    let _ = std::fs::remove_file(&path);
    let fixture = AuditRouteFixture::start("200 OK", "text/html", AUDIT_MINIMAL_HTML, "");
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);
    // The request path itself contains "/wp-content/" but the response
    // carries no technology-identifying header/meta value.
    let target = format!("{}wp-content/", fixture.url());
    let response = post_audit(&base, &target);
    child.kill().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("\"technology_markers\":[]"), "{response}");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn php_url_alone_infers_zero_technology() {
    let path = audit_db_path("php-url");
    let _ = std::fs::remove_file(&path);
    let fixture = AuditRouteFixture::start("200 OK", "text/html", AUDIT_MINIMAL_HTML, "");
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);
    let target = format!("{}index.php", fixture.url());
    let response = post_audit(&base, &target);
    child.kill().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("\"technology_markers\":[]"), "{response}");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn set_cookie_never_leaks_into_the_public_audit_response() {
    let path = audit_db_path("set-cookie");
    let _ = std::fs::remove_file(&path);
    let fixture = AuditRouteFixture::start(
        "200 OK",
        "text/html",
        AUDIT_MINIMAL_HTML,
        "Set-Cookie: session=SUPER_SECRET_SENTINEL; Secure; HttpOnly\r\n",
    );
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);
    let response = post_audit(&base, &fixture.url());
    child.kill().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(!response.to_lowercase().contains("set-cookie"));
    assert!(!response.contains("SUPER_SECRET_SENTINEL"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn one_web_audit_causes_exactly_one_target_hit() {
    let path = audit_db_path("one-hit");
    let _ = std::fs::remove_file(&path);
    let fixture = AuditRouteFixture::start("200 OK", "text/html", AUDIT_MINIMAL_HTML, "");
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);
    let response = post_audit(&base, &fixture.url());
    child.kill().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert_eq!(fixture.hit_count(), 1);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn console_serves_the_page_audit_section() {
    let (mut child, base) = api_server(None);
    let response = get(&base, "/");
    child.kill().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("id=\"audit-heading\""));
    assert!(response.contains("id=\"audit-form\""));
    assert!(response.contains("id=\"audit-url\""));
    assert!(response.contains("id=\"audit-button\" type=\"submit\">Run audit</button>"));
    assert!(response.contains("fetch('/api/audit'"));
    assert!(response.contains("Page Audit"));
}

/// Phase 18 (partial, HTTP-only half): a Web-created audit's own
/// `evidence_ref` resolves through the Web Console's own
/// `GET /api/evidence/{ref}` — proving the Web audit boundary wrote
/// through the identical canonical store the Evidence Inspector already
/// reads from, with no translation. The MCP half of Phase 18 (spawning a
/// real spider-mcp process) lives in `cross_interface_evidence.rs`.
#[test]
fn web_created_audit_evidence_resolves_through_the_web_evidence_route() {
    let path = audit_db_path("web-audit-then-web-evidence");
    let _ = std::fs::remove_file(&path);
    let fixture = AuditRouteFixture::start("200 OK", "text/html", AUDIT_MINIMAL_HTML, "");
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);

    let audit_response = post_audit(&base, &fixture.url());
    assert!(
        audit_response.starts_with("HTTP/1.1 200 OK"),
        "{audit_response}"
    );
    let audit_body = audit_response.split("\r\n\r\n").nth(1).unwrap();
    let evidence_ref = audit_body
        .split("\"evidence_ref\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("audit response must contain evidence_ref");

    let evidence_response = get(&base, &format!("/api/evidence/{evidence_ref}"));
    child.kill().unwrap();
    assert!(
        evidence_response.starts_with("HTTP/1.1 200 OK"),
        "{evidence_response}"
    );
    assert!(evidence_response.contains(&format!("\"id\":\"{evidence_ref}\"")));
    assert!(evidence_response.contains(&fixture.addr.to_string()));

    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------
// SCORPION_RESEARCH_EVIDENCE_NAVIGATION_AND_EXPORT_UX_001 — Phase 11 C/D/E:
// `GET /api/evidence/{ref}/content` and `GET /api/evidence/{ref}/export`
// against the real, compiled `scorpion-api` binary. Evidence is seeded
// directly through the canonical `record_evidence` seam, exactly like the
// existing `/api/evidence/{ref}` acceptance matrix above.
// ---------------------------------------------------------------------

fn split_head_and_body(response: &str) -> (&str, &str) {
    let mut parts = response.splitn(2, "\r\n\r\n");
    let head = parts.next().unwrap_or_default();
    let body = parts.next().unwrap_or_default();
    (head, body)
}

#[test]
fn download_captured_content_returns_exact_stored_bytes_with_safe_headers() {
    let path = evidence_db_path("download-content");
    let _ = std::fs::remove_file(&path);
    let evidence_ref = seed_evidence(&path);

    let env = evidence_env(&path);
    let env_refs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (mut child, base) = api_server_with_env(&env_refs);
    let response = get(&base, &format!("/api/evidence/{evidence_ref}/content"));
    child.kill().unwrap();

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    let (head, body) = split_head_and_body(&response);
    assert!(
        head.contains("Content-Type: text/plain; charset=utf-8"),
        "{head}"
    );
    assert!(
        head.contains(&format!(
            "Content-Disposition: attachment; filename=\"{evidence_ref}-content.txt\""
        )),
        "{head}"
    );
    assert!(head.contains("X-Content-Type-Options: nosniff"), "{head}");
    // Exactly the stored content, unmodified — no HTML cleaning, no
    // extraction, no re-fetch.
    assert_eq!(body, "<html><body>hello evidence route</body></html>");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn download_canonical_json_export_resolves_back_to_the_same_evidence_id() {
    let path = evidence_db_path("download-export");
    let _ = std::fs::remove_file(&path);
    let evidence_ref = seed_evidence(&path);

    let env = evidence_env(&path);
    let env_refs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (mut child, base) = api_server_with_env(&env_refs);
    let response = get(&base, &format!("/api/evidence/{evidence_ref}/export"));
    child.kill().unwrap();

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    let (head, body) = split_head_and_body(&response);
    assert!(head.contains("Content-Type: application/json"), "{head}");
    assert!(
        head.contains(&format!(
            "Content-Disposition: attachment; filename=\"{evidence_ref}-evidence.json\""
        )),
        "{head}"
    );
    assert!(head.contains("X-Content-Type-Options: nosniff"), "{head}");
    // Same canonical EvidenceBundle value the plain read route returns —
    // same identity, same fields, no reconstruction.
    assert!(body.contains(&format!("\"id\":\"{evidence_ref}\"")));
    assert!(body.contains("hello evidence route"));
    assert!(body.contains("\"backend_provenance\":\"reqwest\""));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn download_captured_content_absent_is_a_truthful_404_never_a_fabricated_empty_file() {
    let path = evidence_db_path("download-content-absent");
    let _ = std::fs::remove_file(&path);
    let evidence_ref = {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let store = spider::features::domain_persistence::DomainPersistence::open(&path)
                .await
                .unwrap();
            let bundle = spider::utils::evidence::EvidenceBundle {
                requested_url: Some("https://example.test/no-content".to_string()),
                status_code: Some(200),
                content: None,
                ..Default::default()
            };
            let recorded = spider::utils::evidence::record_evidence(&store, bundle)
                .await
                .unwrap();
            recorded.id.unwrap().to_string()
        })
    };

    let env = evidence_env(&path);
    let env_refs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (mut child, base) = api_server_with_env(&env_refs);
    let response = get(&base, &format!("/api/evidence/{evidence_ref}/content"));
    child.kill().unwrap();

    assert!(response.starts_with("HTTP/1.1 404 Not Found"), "{response}");
    assert!(response.contains("\"code\":\"evidence_content_absent\""));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn download_content_and_export_reuse_the_same_evidence_error_classes() {
    let path = evidence_db_path("download-errors");
    let _ = std::fs::remove_file(&path);
    let _ = seed_evidence(&path);

    let env = evidence_env(&path);
    let env_refs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (mut child, base) = api_server_with_env(&env_refs);

    for suffix in ["content", "export"] {
        let malformed = get(&base, &format!("/api/evidence/not-a-real-ref/{suffix}"));
        assert!(
            malformed.starts_with("HTTP/1.1 400 Bad Request"),
            "{suffix}: {malformed}"
        );
        assert!(malformed.contains("\"code\":\"invalid_evidence_reference\""));

        let absent = get(
            &base,
            &format!("/api/evidence/evid_0123456789abcdef0123456789abcdef/{suffix}"),
        );
        assert!(
            absent.starts_with("HTTP/1.1 404 Not Found"),
            "{suffix}: {absent}"
        );
        assert!(absent.contains("\"code\":\"evidence_not_found\""));
    }
    child.kill().unwrap();

    let _ = std::fs::remove_file(&path);
}

#[test]
fn downloads_never_leak_filesystem_paths_or_store_diagnostics() {
    let dir = std::env::temp_dir().join(format!(
        "scorpion-api-evidence-download-unopenable-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let dir_string = dir.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &dir_string)]);

    for suffix in ["content", "export"] {
        let response = get(
            &base,
            &format!("/api/evidence/evid_0123456789abcdef0123456789abcdef/{suffix}"),
        );
        assert!(
            response.starts_with("HTTP/1.1 503 Service Unavailable"),
            "{suffix}: {response}"
        );
        assert!(response.contains("\"code\":\"evidence_store_unavailable\""));
        assert!(!response.contains(&dir_string), "{suffix}: {response}");
    }
    child.kill().unwrap();

    let _ = std::fs::remove_dir_all(&dir);
}

/// Phase 12 UI-safety proof: the Evidence Inspector's rendering script
/// only ever assigns a live-source `href` after passing it through
/// `safeHttpUrl` (which parses the URL and checks its real `protocol`),
/// and download links point at this crate's own deterministic
/// `/api/evidence/{id}/...` routes — never at a raw, unvalidated,
/// target-controlled string.
#[test]
fn console_evidence_actions_use_safe_url_construction_and_deterministic_download_routes() {
    let (mut child, base) = api_server(None);
    let response = get(&base, "/");
    child.kill().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("function safeHttpUrl(candidate)"));
    assert!(response.contains("parsed.protocol === 'http:' || parsed.protocol === 'https:'"));
    assert!(response.contains("openLiveSource.href = liveSourceUrl"));
    assert!(response.contains("openLiveSource.rel = 'noopener noreferrer'"));
    assert!(response.contains("openLiveSource.target = '_blank'"));
    assert!(response.contains("`/api/evidence/${encodeURIComponent(bundle.id)}/content`"));
    assert!(response.contains("`/api/evidence/${encodeURIComponent(bundle.id)}/export`"));
    // Collapsible sections default closed — no `open` attribute anywhere
    // on a `<details>` element this console renders.
    assert!(response.contains("document.createElement('details')"));
    assert!(!response.contains("details.open = true"));
    assert!(!response.contains(".setAttribute('open'"));
}

/// Phase 12/3: the citation-link rendering path resolves only through the
/// canonical `citations` projection (never `evidence_ids` array position)
/// and never renders synthesis text via an unsafe DOM-injection
/// primitive — reusing the same structural proof style as
/// `spider/tests/architecture_guardrails.rs`'s
/// `web_console_never_uses_unsafe_dom_injection_primitives`, scoped to
/// this new citation-rendering code.
#[test]
fn console_citation_rendering_uses_the_canonical_projection_and_only_inert_text_nodes() {
    let (mut child, base) = api_server(None);
    let response = get(&base, "/");
    child.kill().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("payload.citations"));
    assert!(response.contains("citationsBySourceNumber.get(sourceNumber)"));
    assert!(response
        .contains("citationsBySourceNumber.set(citation.source_number, citation.evidence_ref)"));
    // Every citation piece — the literal marker text and the surrounding
    // synthesis text — goes through the same createTextNode-based `text`
    // helper; there is no innerHTML/outerHTML/insertAdjacentHTML anywhere
    // in the file (already proven repository-wide by
    // `web_console_never_uses_unsafe_dom_injection_primitives`).
    assert!(response.contains("citationButton.appendChild(text(match[0]))"));
}

// ---------------------------------------------------------------------
// SCORPION_CANONICAL_WEB_FETCH_SURFACE_001
//
// POST /api/fetch acceptance matrix against the real, compiled
// `scorpion-api` binary — mirrors the POST /api/audit matrix above
// (real local fixture, real acquisition, real durable evidence) since
// both boundaries share the same "one canonical seam, one interface
// adapter" shape.
// ---------------------------------------------------------------------

fn fetch_db_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "scorpion-api-fetch-route-test-{}-{name}.sqlite3",
        std::process::id()
    ))
}

const FETCH_MINIMAL_HTML: &str =
    "<html><head><title>t</title></head><body><h1>hi</h1></body></html>";

fn post_fetch(base: &str, url: &str) -> String {
    post_path(base, "/api/fetch", &format!("{{\"url\":\"{url}\"}}"))
}

#[test]
fn valid_fetch_returns_200_with_evidence_ref_and_exactly_one_acquisition() {
    let path = fetch_db_path("valid");
    let _ = std::fs::remove_file(&path);
    let fixture = AuditRouteFixture::start("200 OK", "text/html", FETCH_MINIMAL_HTML, "");

    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);
    let response = post_fetch(&base, &fixture.url());
    child.kill().unwrap();

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("\"evidence_ref\":\"evid_"));
    assert!(response.contains("\"status_code\":200"));
    assert!(response.contains("\"observed_status_code\":200"));
    assert!(response.contains("\"content_type\":\"text/html\""));
    assert_eq!(
        fixture.hit_count(),
        1,
        "exactly one target acquisition, never a crawl"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn empty_fetch_url_is_400() {
    let path = fetch_db_path("empty-url");
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);
    let response = post_fetch(&base, "");
    child.kill().unwrap();
    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request"),
        "{response}"
    );
    assert!(response.contains("\"code\":\"invalid_request\""));
}

#[test]
fn non_http_scheme_fetch_target_is_400() {
    let path = fetch_db_path("non-http-scheme");
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);
    let response = post_fetch(&base, "ftp://example.test/");
    child.kill().unwrap();
    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request"),
        "{response}"
    );
    assert!(response.contains("\"code\":\"invalid_fetch_target\""));
}

#[test]
fn unavailable_fetch_store_is_sanitized_and_does_not_leak_the_path() {
    let dir = std::env::temp_dir().join(format!(
        "scorpion-api-fetch-route-unopenable-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);
    let response = post_fetch(&base, "http://example.test/");
    child.kill().unwrap();
    assert!(
        response.starts_with("HTTP/1.1 503 Service Unavailable"),
        "{response}"
    );
    assert!(response.contains("\"code\":\"fetch_store_unavailable\""));
    assert!(!response.contains(&db));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn set_cookie_never_leaks_into_the_public_fetch_response() {
    let path = fetch_db_path("set-cookie");
    let _ = std::fs::remove_file(&path);
    let fixture = AuditRouteFixture::start(
        "200 OK",
        "text/html",
        FETCH_MINIMAL_HTML,
        "Set-Cookie: session=SUPER_SECRET_SENTINEL; Secure; HttpOnly\r\n",
    );
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);
    let response = post_fetch(&base, &fixture.url());
    child.kill().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(!response.to_lowercase().contains("set-cookie"));
    assert!(!response.contains("SUPER_SECRET_SENTINEL"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn console_serves_the_fetch_section() {
    let (mut child, base) = api_server(None);
    let response = get(&base, "/");
    child.kill().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("id=\"fetch-heading\""));
    assert!(response.contains("id=\"fetch-form\""));
    assert!(response.contains("id=\"fetch-url\""));
    assert!(response.contains("id=\"fetch-button\" type=\"submit\">Fetch</button>"));
    assert!(response.contains("fetch('/api/fetch'"));
    assert!(response.contains("<h2 id=\"fetch-heading\">Fetch</h2>"));
}

/// A Web-created fetch's own `evidence_ref` resolves through the Web
/// Console's own `GET /api/evidence/{ref}` — the same proof
/// `web_created_audit_evidence_resolves_through_the_web_evidence_route`
/// already establishes for Audit, now for Fetch: one shared canonical
/// store, no translation, no second evidence path.
#[test]
fn web_created_fetch_evidence_resolves_through_the_web_evidence_route() {
    let path = fetch_db_path("web-fetch-then-web-evidence");
    let _ = std::fs::remove_file(&path);
    let fixture = AuditRouteFixture::start("200 OK", "text/html", FETCH_MINIMAL_HTML, "");
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);

    let fetch_response = post_fetch(&base, &fixture.url());
    assert!(
        fetch_response.starts_with("HTTP/1.1 200 OK"),
        "{fetch_response}"
    );
    let fetch_body = fetch_response.split("\r\n\r\n").nth(1).unwrap();
    let evidence_ref = fetch_body
        .split("\"evidence_ref\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("fetch response must contain evidence_ref");

    let evidence_response = get(&base, &format!("/api/evidence/{evidence_ref}"));
    child.kill().unwrap();
    assert!(
        evidence_response.starts_with("HTTP/1.1 200 OK"),
        "{evidence_response}"
    );
    assert!(evidence_response.contains(&format!("\"id\":\"{evidence_ref}\"")));
    assert!(evidence_response.contains(&fixture.addr.to_string()));

    let _ = std::fs::remove_file(&path);
}

/// The Fetch capability never crawls or follows links — proven the same
/// way `one_web_audit_causes_exactly_one_target_hit` proves it for
/// Audit: exactly one target hit, even when the fixture's own page body
/// contains outbound links a crawler would otherwise discover.
#[test]
fn one_web_fetch_causes_exactly_one_target_hit_even_with_outbound_links_in_the_body() {
    let path = fetch_db_path("one-hit");
    let _ = std::fs::remove_file(&path);
    let fixture = AuditRouteFixture::start(
        "200 OK",
        "text/html",
        "<html><body><a href=\"/other\">other</a><a href=\"/another\">another</a></body></html>",
        "",
    );
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);
    let response = post_fetch(&base, &fixture.url());
    child.kill().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert_eq!(fixture.hit_count(), 1);

    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------
// SCORPION_IAM_CALLBACK_INSPECTOR_WEB_UI_001
//
// The IAM Callback Inspector Web Console section against the real,
// compiled `scorpion-api` binary. This section exposes only the three
// already-shipped `scorpion_app::iam` routes (`POST /api/iam/traces`,
// `GET /api/iam/traces/{id}`, `GET`/`POST /iam/callback/{id}`, proven by
// their own acceptance work) through a rendering layer — no new IAM
// protocol semantics, no new backend route.
// ---------------------------------------------------------------------

fn iam_db_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "scorpion-api-iam-ui-route-test-{}-{name}.sqlite3",
        std::process::id()
    ))
}

fn post_iam_trace(base: &str) -> String {
    let url = base.strip_prefix("http://").unwrap();
    let mut stream = TcpStream::connect(url).unwrap();
    write!(
        stream,
        "POST /api/iam/traces HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn post_iam_callback_form(base: &str, trace_id: &str, form_body: &str) -> String {
    let url = base.strip_prefix("http://").unwrap();
    let mut stream = TcpStream::connect(url).unwrap();
    write!(
        stream,
        "POST /iam/callback/{trace_id} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        form_body.len(),
        form_body
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn extract_json_string_field(body: &str, field: &str) -> Option<String> {
    let marker = format!("\"{field}\":\"");
    body.split(&marker)
        .nth(1)?
        .split('"')
        .next()
        .map(str::to_string)
}

fn make_jwt(header_json: &str, payload_json: &str, signature_segment: &str) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let header_b64 = URL_SAFE_NO_PAD.encode(header_json.as_bytes());
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
    format!("{header_b64}.{payload_b64}.{signature_segment}")
}

fn encode_saml(xml: &str) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine};
    STANDARD.encode(xml.as_bytes())
}

fn url_encode(value: &str) -> String {
    spider::url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

const IAM_SYNTHETIC_SAML_XML: &str = r#"<samlp:Response xmlns:samlp="urn:oasis:names:tc:SAML:2.0:protocol" xmlns:saml="urn:oasis:names:tc:SAML:2.0:assertion" ID="_response-ui-test" InResponseTo="_authnrequest-ui-test" Version="2.0" Destination="https://sp.example.invalid/acs">
  <saml:Issuer>https://idp.example.invalid</saml:Issuer>
  <samlp:Status><samlp:StatusCode Value="urn:oasis:names:tc:SAML:2.0:status:Success"/></samlp:Status>
  <saml:Assertion ID="_assertion-ui-test" IssueInstant="2026-01-01T00:00:00Z" Version="2.0">
    <saml:Issuer>https://idp.example.invalid</saml:Issuer>
    <saml:Subject>
      <saml:NameID Format="urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress">test-user@example.invalid</saml:NameID>
    </saml:Subject>
    <saml:Conditions NotBefore="2026-01-01T00:00:00Z" NotOnOrAfter="2026-01-01T01:00:00Z">
      <saml:AudienceRestriction><saml:Audience>urn:scorpion:test</saml:Audience></saml:AudienceRestriction>
    </saml:Conditions>
    <saml:AttributeStatement>
      <saml:Attribute Name="test-attribute"><saml:AttributeValue>harmless-test-value</saml:AttributeValue></saml:Attribute>
    </saml:AttributeStatement>
  </saml:Assertion>
</samlp:Response>"#;

// 1. IAM section renders.
#[test]
fn console_serves_the_iam_callback_inspector_section() {
    let (mut child, base) = api_server(None);
    let response = get(&base, "/");
    child.kill().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("id=\"iam-heading\""));
    assert!(response.contains("IAM Callback Inspector"));
    assert!(response.contains("id=\"iam-new-trace-button\""));
    assert!(response.contains("id=\"iam-refresh-button\""));
    assert!(response.contains("id=\"iam-trace-info\""));
    assert!(response.contains("id=\"iam-timeline\""));
}

// 2. New Trace calls canonical route. 17. No new IAM backend route
// introduced unnecessarily — exactly the three already-shipped routes
// are referenced by this console, and main.rs itself declares no more.
#[test]
fn iam_console_new_trace_calls_the_canonical_create_route_and_no_new_route_exists() {
    let (mut child, base) = api_server(None);
    let response = get(&base, "/");
    child.kill().unwrap();
    assert!(response.contains("fetch('/api/iam/traces', { method: 'POST' })"));
    assert!(response.contains("fetch(`/api/iam/traces/${encodeURIComponent("));

    let main_source =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs")).unwrap();
    assert_eq!(
        main_source.matches("path == \"/api/iam/traces\"").count(),
        1
    );
    assert_eq!(
        main_source
            .matches("path.starts_with(\"/api/iam/traces/\")")
            .count(),
        1
    );
    assert_eq!(
        main_source
            .matches("path.starts_with(\"/iam/callback/\")")
            .count(),
        1
    );
}

// 3. Returned callback URI displayed exactly — as inert text, never
// reconstructed or re-derived from the trace id client-side.
#[test]
fn iam_console_renders_the_callback_uri_verbatim_as_inert_text() {
    let (mut child, base) = api_server(None);
    let response = get(&base, "/");
    child.kill().unwrap();
    assert!(response.contains("iamCallbackUri = payload.callback_uri"));
    assert!(response.contains("uriCode.appendChild(text(iamCallbackUri))"));
}

// 4/5. AwaitingCallback and Received both shown correctly, end to end
// against the real trace-state machine.
#[test]
fn iam_awaiting_and_received_states_round_trip_through_the_real_server() {
    let path = iam_db_path("state-round-trip");
    let _ = std::fs::remove_file(&path);
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);

    let create_response = post_iam_trace(&base);
    assert!(
        create_response.starts_with("HTTP/1.1 201 Created"),
        "{create_response}"
    );
    let trace_id = extract_json_string_field(&create_response, "trace_id")
        .expect("create response must contain trace_id");

    let readback_before = get(&base, &format!("/api/iam/traces/{trace_id}"));
    assert!(
        readback_before.contains("\"state\":\"awaiting_callback\""),
        "{readback_before}"
    );

    let callback_response = post_iam_callback_form(&base, &trace_id, "code=abc123&state=xyz");
    assert!(
        callback_response.starts_with("HTTP/1.1 200 OK"),
        "{callback_response}"
    );

    let readback_after = get(&base, &format!("/api/iam/traces/{trace_id}"));
    child.kill().unwrap();
    assert!(
        readback_after.contains("\"state\":\"received\""),
        "{readback_after}"
    );

    let _ = std::fs::remove_file(&path);
}

// The console must render AWAITING CALLBACK / RECEIVED with genuinely
// distinct badge classes — never the same tone for both.
#[test]
fn iam_console_distinguishes_awaiting_and_received_state_badges() {
    let (mut child, base) = api_server(None);
    let response = get(&base, "/");
    child.kill().unwrap();
    assert!(response.contains("received ? 'iam-badge-observed' : 'iam-badge-not_validated'"));
    assert!(response.contains("received ? 'RECEIVED' : 'AWAITING CALLBACK'"));
}

// 6. A generic Observed fact is both produced by the real server and
// rendered through the Observed badge path.
#[test]
fn iam_generic_observed_fact_is_persisted_and_console_renders_observed_badge() {
    let path = iam_db_path("generic-observed");
    let _ = std::fs::remove_file(&path);
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);

    let trace_id = extract_json_string_field(&post_iam_trace(&base), "trace_id").unwrap();
    // `foo` is not on the sensitive-name list (unlike `code`, `state`,
    // `id_token`, etc.), so this stays Generic protocol and Observed —
    // a genuine plain-Observed generic fact.
    post_iam_callback_form(&base, &trace_id, "foo=bar123");
    let readback = get(&base, &format!("/api/iam/traces/{trace_id}"));
    child.kill().unwrap();
    assert!(readback.contains("\"name\":\"foo\""), "{readback}");
    assert!(
        readback.contains("\"observed\":{\"value\":\"bar123\"}"),
        "{readback}"
    );

    let console =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs")).unwrap();
    assert!(console.contains("case 'observed': return 'OBSERVED';"));
    assert!(console.contains(".iam-badge-observed { background:"));

    let _ = std::fs::remove_file(&path);
}

// 7. A Redacted fact's digest is persisted and the console only ever
// reads the digest field — never a plaintext value off the Redacted
// variant (which structurally has no such field).
#[test]
fn iam_redacted_fact_carries_only_a_digest_and_console_never_reads_a_plaintext_field() {
    let path = iam_db_path("redacted-digest");
    let _ = std::fs::remove_file(&path);
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);

    let trace_id = extract_json_string_field(&post_iam_trace(&base), "trace_id").unwrap();
    post_iam_callback_form(&base, &trace_id, "access_token=super-secret-value");
    let readback = get(&base, &format!("/api/iam/traces/{trace_id}"));
    child.kill().unwrap();
    assert!(readback.contains("\"name\":\"access_token\""), "{readback}");
    assert!(
        readback.contains("\"redacted\":{\"sha256_digest\":"),
        "{readback}"
    );
    assert!(!readback.contains("super-secret-value"), "{readback}");

    let console =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs")).unwrap();
    assert!(console.contains("case 'redacted': return `digest: ${status.redacted.sha256_digest}`;"));
    assert!(!console.contains("status.redacted.value"));

    let _ = std::fs::remove_file(&path);
}

// 8. NOT_VALIDATED is visually distinct from OBSERVED — different CSS
// classes, different colors, never sharing a "success" tone.
#[test]
fn iam_console_not_validated_badge_is_visually_distinct_from_observed() {
    let (mut child, base) = api_server(None);
    let response = get(&base, "/");
    child.kill().unwrap();
    assert!(response.contains(".iam-badge-observed { background: #e3ecff; color: #1447b8; }"));
    assert!(response.contains(".iam-badge-not_validated { background: #fff2d6; color: #8a5a00; }"));
    assert_ne!(response.matches(".iam-badge-observed {").next(), None);
    // Distinct color values (never the same background/foreground pair).
    assert!(!response.contains(".iam-badge-not_validated { background: #e3ecff; color: #1447b8; }"));
}

// 9/10. OIDC/JWT facts render, with signature verification always
// NOT_VALIDATED — proven end to end against the real server with a
// synthetic id_token.
#[test]
fn iam_oidc_jwt_facts_render_with_signature_verification_not_validated() {
    let path = iam_db_path("oidc-jwt");
    let _ = std::fs::remove_file(&path);
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);

    let jwt = make_jwt(
        r#"{"alg":"RS256","typ":"JWT","kid":"key-1"}"#,
        r#"{"iss":"https://idp.example.invalid","aud":"scorpion-test-client","exp":4102444800,"iat":1700000000,"nonce":"nonce-value-123"}"#,
        "fake-signature",
    );
    let form_body = format!("id_token={}&state=xyz", url_encode(&jwt));
    let trace_id = extract_json_string_field(&post_iam_trace(&base), "trace_id").unwrap();
    post_iam_callback_form(&base, &trace_id, &form_body);
    let readback = get(&base, &format!("/api/iam/traces/{trace_id}"));
    child.kill().unwrap();

    assert!(readback.contains("\"protocol\":\"oidc\""), "{readback}");
    assert!(readback.contains("\"jwt.iss\""), "{readback}");
    assert!(
        readback.contains("\"https://idp.example.invalid\""),
        "{readback}"
    );
    assert!(readback.contains("\"jwt.aud\""), "{readback}");
    assert!(readback.contains("\"jwt.nonce\""), "{readback}");
    assert!(
        readback.contains(
            "{\"name\":\"jwt.signature.verification\",\"status\":{\"not_validated\":{\"value\":\"not_attempted\"}}}"
        ),
        "{readback}"
    );

    let console =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs")).unwrap();
    assert!(console.contains("'jwt.iss': 'Issuer'"));
    assert!(console.contains("'jwt.signature.verification': 'Signature verification'"));

    let _ = std::fs::remove_file(&path);
}

// 11/12. SAML facts render, with signature verification always
// NOT_VALIDATED — proven end to end against the real server with a
// synthetic SAMLResponse.
#[test]
fn iam_saml_facts_render_with_signature_verification_not_validated() {
    let path = iam_db_path("saml-facts");
    let _ = std::fs::remove_file(&path);
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);

    let saml_b64 = encode_saml(IAM_SYNTHETIC_SAML_XML);
    let form_body = format!("SAMLResponse={}", url_encode(&saml_b64));
    let trace_id = extract_json_string_field(&post_iam_trace(&base), "trace_id").unwrap();
    post_iam_callback_form(&base, &trace_id, &form_body);
    let readback = get(&base, &format!("/api/iam/traces/{trace_id}"));
    child.kill().unwrap();

    assert!(readback.contains("\"protocol\":\"saml\""), "{readback}");
    assert!(readback.contains("\"saml.response.id\""), "{readback}");
    assert!(readback.contains("\"_response-ui-test\""), "{readback}");
    assert!(readback.contains("\"saml.name_id\""), "{readback}");
    assert!(
        readback.contains("\"test-user@example.invalid\""),
        "{readback}"
    );
    assert!(
        readback.contains("\"saml.attribute.value[test-attribute]\""),
        "{readback}"
    );
    assert!(
        readback.contains(
            "{\"name\":\"saml.signature.verification\",\"status\":{\"not_validated\":{\"value\":\"not_attempted\"}}}"
        ),
        "{readback}"
    );

    let console =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs")).unwrap();
    assert!(console.contains("'saml.name_id': 'NameID'"));
    assert!(console.contains("'saml.signature.verification': 'Signature verification'"));

    let _ = std::fs::remove_file(&path);
}

// 13. Multiple observations on the same trace render in revision order
// — proven by asserting the server's own JSON emits revision 1 before
// revision 2 (the console renders `payload.observations` as received,
// with no client-side re-sort — see `renderIamTimeline`).
#[test]
fn iam_multiple_observations_are_returned_in_revision_order() {
    let path = iam_db_path("multi-observation-order");
    let _ = std::fs::remove_file(&path);
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);

    let trace_id = extract_json_string_field(&post_iam_trace(&base), "trace_id").unwrap();
    post_iam_callback_form(&base, &trace_id, "foo=first-callback");
    post_iam_callback_form(&base, &trace_id, "foo=second-callback");
    let readback = get(&base, &format!("/api/iam/traces/{trace_id}"));
    child.kill().unwrap();

    let first_index = readback
        .find("first-callback")
        .expect("first callback must be present");
    let second_index = readback
        .find("second-callback")
        .expect("second callback must be present");
    assert!(first_index < second_index, "{readback}");
    let revision_1 = readback
        .find("\"revision\":1")
        .expect("revision 1 must be present");
    let revision_2 = readback
        .find("\"revision\":2")
        .expect("revision 2 must be present");
    assert!(revision_1 < revision_2, "{readback}");

    let console =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs")).unwrap();
    assert!(console.contains(
        "for (const view of observations) list.appendChild(renderIamObservation(view));"
    ));

    let _ = std::fs::remove_file(&path);
}

// 14/15. No raw id_token or SAMLResponse reconstruction anywhere in the
// console's JavaScript — no base64/JWT/XML decoding primitive exists in
// the file at all; the console only ever renders the already-decoded
// facts the server computed.
#[test]
fn iam_console_never_reconstructs_raw_id_token_or_saml_response_client_side() {
    let (mut child, base) = api_server(None);
    let response = get(&base, "/");
    child.kill().unwrap();
    assert!(!response.contains("atob("));
    assert!(!response.contains("btoa("));
    assert!(!response.contains("DOMParser"));
    assert!(!response.contains("jwt_decode"));
    assert!(!response.contains(".split('.')"));
}

// 16. Existing Fetch/Audit/Evidence/Research sections remain present
// and functional on the same page as the new IAM section.
#[test]
fn iam_section_coexists_with_every_pre_existing_console_section() {
    let (mut child, base) = api_server(None);
    let response = get(&base, "/");
    child.kill().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("id=\"research-heading\""));
    assert!(response.contains("id=\"evidence-heading\""));
    assert!(response.contains("id=\"audit-heading\""));
    assert!(response.contains("id=\"fetch-heading\""));
    assert!(response.contains("id=\"iam-heading\""));
    assert!(response.contains("fetch('/api/search'"));
    assert!(response.contains("fetch('/api/research'"));
    assert!(response.contains("fetch('/api/audit'"));
    assert!(response.contains("fetch('/api/fetch'"));
    assert!(response.contains("fetch('/api/iam/traces'"));
}

// Malformed-input error handling: a truthful readback error for an
// unknown trace id (trace not found), never a fabricated empty-success
// render.
#[test]
fn iam_readback_of_an_unknown_trace_returns_a_truthful_not_found_error() {
    let path = iam_db_path("unknown-trace");
    let _ = std::fs::remove_file(&path);
    let db = path.to_string_lossy().to_string();
    let (mut child, base) = api_server_with_env(&[("SCORPION_DOMAIN_DB", &db)]);
    // A syntactically well-formed (correct prefix, correct hex length)
    // but never-minted trace id — must resolve to trace_not_found, not
    // invalid_trace_id.
    let response = get(
        &base,
        "/api/iam/traces/iam_trace_00000000000000000000000000000000",
    );
    child.kill().unwrap();
    assert!(response.starts_with("HTTP/1.1 404 Not Found"), "{response}");
    assert!(
        response.contains("\"code\":\"trace_not_found\""),
        "{response}"
    );

    let console =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs")).unwrap();
    assert!(console
        .contains("iamShowError(payload?.error?.message || 'Trace readback is unavailable.')"));

    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------
// SCORPION_WINDOWS_INSTALLABLE_APPLICATION_001
//
// scorpion-launcher end-to-end, against the real compiled binaries.
// The launcher's shipped/meaningful behavior is Windows-only, but every
// piece of it was deliberately written with a cross-platform fallback
// (see scorpion_launcher.rs's own module doc) specifically so its real,
// compiled orchestration logic — not just its pure helper functions —
// could be exercised here on the platform this repository's CI actually
// runs on. This is not a substitute for the real Windows acceptance run
// (SCORPION_WINDOWS_INSTALLABLE_APPLICATION_001's own owner-acceptance
// procedure); it proves the orchestration logic itself is correct.
//
// These tests use the launcher's real fixed bind (127.0.0.1:8787) since
// that is the actual, non-configurable contract under test — no other
// test in this suite binds that exact address (every other helper here
// uses TcpListener::bind("127.0.0.1:0") for an OS-assigned ephemeral
// port precisely to avoid this collision), so this remains safe to run
// alongside the rest of the suite.
// ---------------------------------------------------------------------

fn launcher_home_dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "scorpion-launcher-test-home-{}-{name}",
        std::process::id()
    ))
}

/// The launcher's `resolve_app_data_dir` branches on `cfg!(windows)` at
/// *runtime* — Windows reads `LOCALAPPDATA`, every other platform (this
/// repository's own Linux dev/CI environment) reads `HOME`. Spawning the
/// launcher with **both** env vars pointed at the same isolated `home`
/// directory makes it deterministically test-isolated on whichever
/// platform this test suite actually executes on — the real Windows
/// run of this exact test (`SCORPION_WINDOWS_INSTALLABLE_APPLICATION_001`'s
/// own CI) is what caught the very real bug of only setting `HOME`.
fn set_launcher_home_env(command: &mut Command, home: &std::path::Path) {
    command.env("HOME", home).env("LOCALAPPDATA", home);
}

/// Mirrors `resolve_app_data_dir`'s own path-joining exactly, for
/// whichever platform this test is actually executing on.
fn expected_app_data_dir(home: &std::path::Path) -> std::path::PathBuf {
    if cfg!(windows) {
        home.join("Scorpion")
    } else {
        home.join(".local/share/Scorpion")
    }
}

fn probe_health(timeout: Duration) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&"127.0.0.1:8787".parse().unwrap(), timeout)
    else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let _ =
        stream.write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response.starts_with("HTTP/1.1 200") && response.contains("\"status\":\"ok\"")
}

fn wait_for_health(timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if probe_health(Duration::from_millis(300)) {
            return true;
        }
        thread::sleep(Duration::from_millis(200));
    }
    false
}

/// Guarantees the `scorpion-api` this test's launcher spawns is reaped
/// even if an assertion panics partway through — a `Drop` impl runs
/// during unwinding too, unlike a plain cleanup step at the end of the
/// function (which a mid-test panic would skip, leaking a process still
/// holding 127.0.0.1:8787 for every later test run).
struct KillSpawnedApiOnDrop(std::path::PathBuf);

impl Drop for KillSpawnedApiOnDrop {
    fn drop(&mut self) {
        kill_process_with_env_containing(&domain_db_env_needle(&self.0));
    }
}

#[test]
fn launcher_end_to_end_spawns_configures_and_becomes_healthy() {
    assert!(
        !probe_health(Duration::from_millis(200)),
        "127.0.0.1:8787 must be free before this test runs — something is already listening on it"
    );

    let home = launcher_home_dir("spawn-fresh");
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let _kill_guard = KillSpawnedApiOnDrop(home.clone());

    let mut launcher_command = Command::new(env!("CARGO_BIN_EXE_scorpion-launcher"));
    set_launcher_home_env(&mut launcher_command, &home);
    let mut child = launcher_command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("scorpion-launcher must spawn");

    // 4/5: launcher waits for real /health readiness before doing
    // anything else observable — proven by the readiness itself
    // becoming true within a bounded wait.
    let became_healthy = wait_for_health(Duration::from_secs(20));
    let _ = child.wait();
    assert!(
        became_healthy,
        "scorpion-api must become healthy after the launcher spawns it"
    );

    // Log files exist and the launcher's own log recorded a real
    // startup line (2/3: exact loopback bind, never 0.0.0.0).
    let app_data_dir = expected_app_data_dir(&home);
    let launcher_log = std::fs::read_to_string(app_data_dir.join("logs/launcher.log"))
        .expect("launcher.log must exist");
    assert!(launcher_log.contains("127.0.0.1:8787"));
    assert!(!launcher_log.contains("0.0.0.0"));
    assert!(launcher_log.contains("scorpion-api is healthy"));

    let api_log_path = app_data_dir.join("logs/scorpion-api.log");
    let api_log = std::fs::read_to_string(&api_log_path).expect("scorpion-api.log must exist");
    assert!(api_log.contains("scorpion-api listening on 127.0.0.1:8787"));

    // 14/15/16/17: exercise a real generic + a real SAML callback
    // through the launcher-configured instance, then confirm the log
    // never contains the raw secret material.
    let trace_id =
        extract_json_string_field(&post_iam_trace("http://127.0.0.1:8787"), "trace_id").unwrap();
    post_iam_callback_form(
        "http://127.0.0.1:8787",
        &trace_id,
        "access_token=must-never-appear-in-any-log-file",
    );
    let readback = get(
        "http://127.0.0.1:8787",
        &format!("/api/iam/traces/{trace_id}"),
    );
    assert!(readback.contains("\"redacted\""), "{readback}");
    assert!(!readback.contains("must-never-appear-in-any-log-file"));

    // 9: no manual SCORPION_DOMAIN_DB setup was needed — the launcher
    // configured it internally. The store is opened lazily (on first
    // real domain-persistence use, not merely on startup — /health
    // itself never touches it), so the sqlite file's existence is
    // checked here, after the IAM trace/callback above actually used
    // it, not right after startup.
    let db_path = app_data_dir.join("scorpion.sqlite3");
    assert!(
        db_path.is_file(),
        "expected canonical DB at {}",
        db_path.display()
    );

    let api_log_after = std::fs::read_to_string(&api_log_path).unwrap();
    assert!(!api_log_after.contains("must-never-appear-in-any-log-file"));

    // 18/19: the existing Web Console still renders, and /health stays
    // truthful, through the launcher-managed instance.
    let console = get("http://127.0.0.1:8787", "/");
    assert!(console.starts_with("HTTP/1.1 200 OK"));
    assert!(console.contains("id=\"iam-heading\""));
    let health = get("http://127.0.0.1:8787", "/health");
    assert!(health.contains("\"status\":\"ok\""));

    // 12: no development absolute path leaked into any log.
    assert!(!launcher_log.contains("/home/jonny"));
    assert!(!api_log_after.contains("jonnyfrez"));

    // 8: duplicate launch is deterministic — a second launcher run must
    // detect the already-healthy instance and must NOT start a second
    // scorpion-api (proven by: it exits quickly, and the DB/log files
    // are not duplicated/reset).
    let second_home = launcher_home_dir("spawn-fresh-second-launch");
    let _ = std::fs::remove_dir_all(&second_home);
    std::fs::create_dir_all(&second_home).unwrap();
    let start = std::time::Instant::now();
    let mut second_launcher_command = Command::new(env!("CARGO_BIN_EXE_scorpion-launcher"));
    set_launcher_home_env(&mut second_launcher_command, &second_home);
    let status = second_launcher_command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("second scorpion-launcher run must spawn");
    let elapsed = start.elapsed();
    assert!(
        status.success(),
        "duplicate launch must succeed (reuse), not fail"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "duplicate launch must reuse the existing instance quickly, not re-spawn/re-wait: took {elapsed:?}"
    );
    // The second launcher's own (isolated) app-data dir must never have
    // gotten a scorpion.sqlite3 — proof no second server/config was
    // created.
    assert!(!expected_app_data_dir(&second_home)
        .join("scorpion.sqlite3")
        .is_file());

    // Clean up the one real scorpion-api this test spawned. It was
    // spawned by the launcher (not by this test directly), so there is
    // no `Child` handle to it here, and its distinguishing marker
    // (SCORPION_DOMAIN_DB=<home>/...) lives in its *environment*, not
    // its argv — `pkill -f` only searches argv, so it would silently
    // match nothing (or worse, match an unrelated concurrently-running
    // test's scorpion-api by bare name). Find the real PID by scanning
    // /proc/*/environ for this test's own unique db path instead — this
    // is a Linux-only cleanup mechanism (there is no /proc on Windows);
    // `kill_process_with_env_containing` no-ops harmlessly there, since
    // each CI run is a fresh, disposable VM anyway.
    kill_process_with_env_containing(&domain_db_env_needle(&home));
    thread::sleep(Duration::from_millis(300));
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&second_home);
}

fn domain_db_env_needle(home: &std::path::Path) -> String {
    format!(
        "SCORPION_DOMAIN_DB={}",
        expected_app_data_dir(home)
            .join("scorpion.sqlite3")
            .display()
    )
}

/// Best-effort test-only cleanup: kill any process whose `/proc/<pid>/environ`
/// contains `needle` — used only to reap the one `scorpion-api` this test's
/// own launcher spawned (identified by its unique, test-local
/// `SCORPION_DOMAIN_DB` path), never anything matched by name alone.
fn kill_process_with_env_containing(needle: &str) {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        let Ok(environ) = std::fs::read(entry.path().join("environ")) else {
            continue;
        };
        if environ
            .split(|&b| b == 0)
            .any(|var| var == needle.as_bytes())
        {
            let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
        }
    }
}

// 3: structural guardrail — 0.0.0.0 never appears as a bind literal
// anywhere in the launcher or the server entrypoint.
#[test]
fn no_wildcard_bind_literal_exists_in_launcher_or_server_source() {
    // Scoped to actual production code — excludes doc-comment prose
    // (this module's own doc comments truthfully explain "never
    // 0.0.0.0" in several places) and the test module (whose own
    // negative assertions, e.g. `!SCORPION_BIND.starts_with("0.0.0.0")`,
    // legitimately reference the literal too) — both of which would
    // otherwise self-defeat a bare whole-file substring check.
    fn has_wildcard_bind_in_code(source: &str) -> bool {
        let production_source = source.split("#[cfg(test)]").next().unwrap_or(source);
        production_source.lines().any(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with("//") && line.contains("0.0.0.0")
        })
    }
    let launcher_source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/bin/scorpion_launcher.rs"
    ))
    .unwrap();
    let server_source =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs")).unwrap();
    assert!(!has_wildcard_bind_in_code(&launcher_source));
    assert!(!has_wildcard_bind_in_code(&server_source));
}

// 13: install-directory read-only safety — the launcher never writes
// anywhere under its own resolved install directory; every mutable
// path it computes (db/log paths) lives only under the app-data
// directory.
#[test]
fn launcher_source_never_writes_under_the_install_directory() {
    let launcher_source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/bin/scorpion_launcher.rs"
    ))
    .unwrap();
    assert!(!launcher_source.contains("install_dir.join(\"scorpion.sqlite3\")"));
    assert!(!launcher_source.contains("install_dir.join(\"logs\")"));
    // The only writes performed (`create_dir_all`, log file opens, the
    // spawned child's own DB) are all rooted at `app_data_dir`, never
    // `install_dir` — `install_dir` is read-only-used solely to locate
    // the sibling scorpion-api executable.
    assert!(launcher_source.contains("install_dir.join(scorpion_api_exe_name())"));
}
