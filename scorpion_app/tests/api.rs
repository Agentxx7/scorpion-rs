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
