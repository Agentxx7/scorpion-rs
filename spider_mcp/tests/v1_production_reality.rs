//! Bounded, opt-in V1 production-reality acceptance.
//!
//! This test intentionally uses real public network fixtures. It is ignored by
//! ordinary test runs and must be invoked by the dedicated release-proof
//! workflow with `SCORPION_V1_PRODUCTION_REALITY=1`.

use serde_json::{json, Value};
use spider::utils::evidence::{build_evidence, page_provenance};
use spider::website::Website;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HTML_URL: &str = "https://httpbin.org/html";
const BYTES_URL: &str = "https://httpbin.org/bytes/64";
const REDIRECT_URL: &str = "https://httpbin.org/redirect-to?url=https%3A%2F%2Fhttpbin.org%2Fhtml";
const NOT_FOUND_URL: &str = "https://httpbin.org/status/404";
const SERVER_ERROR_URL: &str = "https://httpbin.org/status/500";
const NXDOMAIN_URL: &str = "https://scorpion-v1-proof-nonexistent.invalid/";
const BAD_CERT_URL: &str = "https://expired.badssl.com/";
const CHROME_JS_URL: &str = "https://www.crawler-test.com/javascript/render_only_after_js";
const EXPECTED_CASES: usize = 17;

fn infra_fail(message: impl std::fmt::Display) -> ! {
    panic!("INFRA_BLOCKED: {message}")
}

fn product_fail(message: impl std::fmt::Display) -> ! {
    panic!("PRODUCT_FAIL: {message}")
}

fn pass(cases: &mut usize, name: &str) {
    *cases += 1;
    println!("V1_PRODUCTION_REALITY {name}=PASS");
}

fn preflight(url: &str, expected_status: u16) {
    let output = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--location",
            "--max-time",
            "20",
            "--output",
            "/dev/null",
            "--write-out",
            "%{http_code}",
            url,
        ])
        .output()
        .unwrap_or_else(|error| infra_fail(format!("curl unavailable: {error}")));
    if !output.status.success() {
        infra_fail(format!(
            "fixture {url} unavailable: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let actual = String::from_utf8_lossy(&output.stdout);
    if actual.trim() != expected_status.to_string() {
        infra_fail(format!(
            "fixture {url} returned {}, expected {expected_status}",
            actual.trim()
        ));
    }
}

fn preflight_infrastructure(cases: &mut usize) {
    for host in ["httpbin.org:443", "www.crawler-test.com:443"] {
        if host
            .to_socket_addrs()
            .unwrap_or_else(|error| {
                infra_fail(format!("DNS resolution failed for {host}: {error}"))
            })
            .next()
            .is_none()
        {
            infra_fail(format!("DNS resolution returned no addresses for {host}"));
        }
    }
    preflight(HTML_URL, 200);
    preflight(BYTES_URL, 200);
    preflight(REDIRECT_URL, 200);
    preflight(NOT_FOUND_URL, 404);
    preflight(SERVER_ERROR_URL, 500);
    preflight(CHROME_JS_URL, 200);

    let bad_cert = Command::new("curl")
        .args(["--silent", "--show-error", "--max-time", "15", BAD_CERT_URL])
        .output()
        .unwrap_or_else(|error| infra_fail(format!("curl unavailable: {error}")));
    if bad_cert.status.success() {
        infra_fail("expired.badssl.com unexpectedly presented a trusted certificate");
    }
    pass(cases, "INFRASTRUCTURE_PREFLIGHT");
}

async fn website_page(url: &str, scrape: bool) -> spider::page::Page {
    let mut website = Website::new(url);
    website
        .with_limit(1)
        .with_request_timeout(Some(Duration::from_secs(30)))
        .with_crawl_timeout(Some(Duration::from_secs(60)));
    if scrape {
        website.scrape().await;
        let pages = website.get_pages().unwrap_or_else(|| {
            product_fail("Website::scrape did not initialize its canonical page collector")
        });
        if pages.len() != 1 {
            product_fail(format!(
                "Website::scrape collected {} pages, expected exactly one",
                pages.len()
            ));
        }
        return pages[0].clone();
    } else {
        let mut receiver = website.subscribe(8);
        let collector = tokio::spawn(async move {
            let mut pages = Vec::new();
            while let Ok(page) = receiver.recv().await {
                pages.push(page);
            }
            pages
        });
        website.crawl().await;
        website.unsubscribe();
        let pages = collector
            .await
            .unwrap_or_else(|error| product_fail(format!("Website collector failed: {error}")));
        if pages.len() != 1 {
            product_fail(format!(
                "Website::crawl observed {} pages, expected exactly one",
                pages.len()
            ));
        }
        return pages.into_iter().next().unwrap();
    }
}

fn assert_network_page(page: &spider::page::Page, label: &str) {
    if page.status_code.as_u16() != 200
        || page.observed_status_code.map(|s| s.as_u16()) != Some(200)
    {
        product_fail(format!(
            "{label} status={} observed={:?}",
            page.status_code, page.observed_status_code
        ));
    }
    if page.get_html_bytes_u8().is_empty() || !page.get_html().contains("Moby-Dick") {
        product_fail(format!("{label} returned empty or unexpected content"));
    }
    let provenance = page_provenance(page);
    let evidence = build_evidence(page, Some(page.get_html().to_string()), false, false);
    let truthful_backend_origin = matches!(
        (
            provenance.backend_provenance.as_deref(),
            provenance.response_origin.as_deref()
        ),
        (Some("reqwest"), Some("network")) | (None, None)
    );
    if provenance.transport.as_deref() != Some("default")
        || !truthful_backend_origin
        || evidence.transport != provenance.transport
        || evidence.backend_provenance != provenance.backend_provenance
        || evidence.response_origin != provenance.response_origin
        || evidence.observed_status_code != provenance.observed_status_code
        || evidence.response_body_hash.is_none()
    {
        product_fail(format!(
            "{label} Page/provenance/evidence disagreement: {provenance:?} {evidence:?}"
        ));
    }
}

fn scorpion() -> PathBuf {
    let configured = std::env::var_os("SCORPION_V1_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/debug/scorpion"));
    if configured.is_absolute() {
        configured
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(configured)
    }
}

fn run_scorpion(args: &[&str]) -> Output {
    Command::new(scorpion())
        .args(args)
        .output()
        .unwrap_or_else(|error| product_fail(format!("scorpion did not run: {error}")))
}

fn json_stdout(output: &Output, label: &str) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        product_fail(format!(
            "{label} stdout was not JSON: {error}; stdout={:?}; stderr={:?}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    })
}

fn assert_success(output: &Output, label: &str) {
    if !output.status.success() {
        product_fail(format!("{label} failed: {output:?}"));
    }
}

fn temporary_directory() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "scorpion-v1-production-reality-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn find_files(root: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            find_files(&path, files);
        } else {
            files.push(path);
        }
    }
}

struct McpClient {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpClient {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_spider-mcp"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| product_fail(format!("spider-mcp did not start: {error}")));
        Self {
            stdin: Some(child.stdin.take().unwrap()),
            stdout: BufReader::new(child.stdout.take().unwrap()),
            child,
            next_id: 1,
        }
    }

    fn send(&mut self, value: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{}", serde_json::to_string(&value).unwrap()).unwrap();
        stdin.flush().unwrap();
    }

    fn response(&mut self) -> Value {
        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .unwrap_or_else(|error| product_fail(format!("MCP stdout read failed: {error}")));
        serde_json::from_str(&line).unwrap_or_else(|error| {
            product_fail(format!(
                "MCP stdout protocol impurity: {error}; line={line:?}"
            ))
        })
    }

    fn initialize(&mut self) {
        self.send(json!({
            "jsonrpc": "2.0", "id": self.next_id, "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                       "clientInfo": {"name": "v1-production-reality", "version": "1"}}
        }));
        self.next_id += 1;
        if self.response().get("result").is_none() {
            product_fail("MCP initialize did not return a result");
        }
        self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    }

    fn scrape(&mut self, url: &str) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": "spider_scrape",
                       "arguments": {"url": url, "return_format": "raw", "evidence": true}}
        }));
        self.response()
    }

    fn shutdown_cleanly(mut self) {
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) if status.success() => return,
                Ok(Some(status)) => product_fail(format!("MCP exited nonzero: {status}")),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                Ok(None) => {
                    let _ = self.child.kill();
                    product_fail("MCP did not shut down after stdin EOF");
                }
                Err(error) => product_fail(format!("MCP wait failed: {error}")),
            }
        }
    }
}

fn mcp_evidence(response: &Value) -> Value {
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| product_fail(format!("MCP response lacks text content: {response:?}")));
    serde_json::from_str(text)
        .unwrap_or_else(|error| product_fail(format!("MCP evidence was not JSON: {error}")))
}

#[tokio::test]
#[ignore = "real external V1 acceptance; run only through the release-proof workflow"]
async fn v1_production_reality_acceptance() {
    if std::env::var("SCORPION_V1_PRODUCTION_REALITY").as_deref() != Ok("1") {
        product_fail("SCORPION_V1_PRODUCTION_REALITY=1 is required; no skip is permitted");
    }
    let mut cases = 0usize;
    preflight_infrastructure(&mut cases);

    let crawl_page = website_page(HTML_URL, false).await;
    assert_network_page(&crawl_page, "Website::crawl");
    pass(&mut cases, "WEBSITE_CRAWL");

    let scrape_page = website_page(HTML_URL, true).await;
    assert_network_page(&scrape_page, "Website::scrape");
    pass(&mut cases, "WEBSITE_SCRAPE");
    pass(&mut cases, "PROVENANCE_EVIDENCE_AGREEMENT");

    let crawl = run_scorpion(&[
        "--url",
        HTML_URL,
        "--limit",
        "1",
        "--http",
        "crawl",
        "--output-links",
    ]);
    assert_success(&crawl, "CLI crawl");
    if !String::from_utf8_lossy(&crawl.stdout).contains("https://httpbin.org/html") {
        product_fail("CLI crawl did not emit the acquired URL");
    }
    pass(&mut cases, "CLI_CRAWL");

    let scrape = run_scorpion(&[
        "--url",
        HTML_URL,
        "--limit",
        "1",
        "--http",
        "scrape",
        "--output-html",
    ]);
    assert_success(&scrape, "CLI scrape");
    let scrape_json = json_stdout(&scrape, "CLI scrape");
    if scrape_json["status_code"] != 200
        || scrape_json["provenance"]["observed_status_code"] != 200
        || scrape_json["provenance"]["transport"] != "default"
        || scrape_json["provenance"]["backend_provenance"] != "reqwest"
        || !scrape_json["content"]
            .as_str()
            .unwrap_or_default()
            .contains("Moby-Dick")
    {
        product_fail(format!("CLI scrape evidence mismatch: {scrape_json:?}"));
    }
    pass(&mut cases, "CLI_SCRAPE");

    let download_root = temporary_directory();
    let download = run_scorpion(&[
        "--url",
        BYTES_URL,
        "--limit",
        "1",
        "--http",
        "download",
        "--target-destination",
        download_root.to_str().unwrap(),
    ]);
    assert_success(&download, "CLI download");
    let mut files = Vec::new();
    find_files(&download_root, &mut files);
    if files.len() != 1 || fs::metadata(&files[0]).unwrap().len() != 64 {
        product_fail(format!("CLI download materialization mismatch: {files:?}"));
    }
    fs::remove_dir_all(download_root).unwrap();
    pass(&mut cases, "CLI_DOWNLOAD");

    let fetch = run_scorpion(&["fetch", HTML_URL]);
    assert_success(&fetch, "CLI fetch");
    let fetch_json = json_stdout(&fetch, "CLI fetch");
    if fetch_json["status_code"] != 200
        || fetch_json["observed_status_code"] != 200
        || fetch_json["transport"] != "default"
        || fetch_json["backend_provenance"] != "reqwest"
        || fetch_json["response_origin"] != "network"
        || fetch_json["response_body_hash"]
            .as_str()
            .unwrap_or_default()
            .len()
            != 64
    {
        product_fail(format!("CLI fetch evidence mismatch: {fetch_json:?}"));
    }
    pass(&mut cases, "CLI_FETCH");

    let redirect = run_scorpion(&["fetch", REDIRECT_URL]);
    assert_success(&redirect, "CLI redirect fetch");
    let redirect_json = json_stdout(&redirect, "CLI redirect fetch");
    if redirect_json["final_url"] != HTML_URL || redirect_json["observed_status_code"] != 200 {
        product_fail(format!("redirect evidence mismatch: {redirect_json:?}"));
    }
    pass(&mut cases, "REDIRECT");

    let not_found = run_scorpion(&["fetch", NOT_FOUND_URL]);
    assert_success(&not_found, "CLI observed 404 acquisition");
    let not_found_json = json_stdout(&not_found, "CLI observed 404 acquisition");
    if not_found_json["status_code"] != 404 || not_found_json["observed_status_code"] != 404 {
        product_fail(format!(
            "404 was not surfaced truthfully: {not_found_json:?}"
        ));
    }
    pass(&mut cases, "REMOTE_4XX");

    let server_error = run_scorpion(&[
        "--url",
        SERVER_ERROR_URL,
        "--limit",
        "1",
        "--http",
        "scrape",
        "--output-html",
    ]);
    assert_success(&server_error, "CLI observed 500 acquisition");
    let server_error_json = json_stdout(&server_error, "CLI observed 500 acquisition");
    if server_error_json["status_code"] != 500
        || server_error_json["provenance"]["observed_status_code"] != 500
        || !String::from_utf8_lossy(&server_error.stderr).contains("server error")
    {
        product_fail(format!(
            "500 was not surfaced truthfully: {server_error_json:?}"
        ));
    }
    pass(&mut cases, "REMOTE_5XX");

    for (name, url) in [
        ("REMOTE_DNS_FAILURE", NXDOMAIN_URL),
        ("TLS_INVALID_CERTIFICATE", BAD_CERT_URL),
    ] {
        let failed = run_scorpion(&["fetch", url]);
        let failed_json = json_stdout(&failed, name);
        if failed.status.code() != Some(2)
            || !failed_json["observed_status_code"].is_null()
            || !failed_json["retrieved_at"].is_null()
            || !failed_json["response_origin"].is_null()
            || !failed_json["response_body_hash"].is_null()
        {
            product_fail(format!(
                "{name} became false success: {failed:?} {failed_json:?}"
            ));
        }
        pass(&mut cases, name);
    }

    let chrome = run_scorpion(&[
        "--url",
        CHROME_JS_URL,
        "--limit",
        "1",
        "--headless",
        "scrape",
        "--output-html",
    ]);
    assert_success(&chrome, "external Chrome scrape");
    let chrome_json = json_stdout(&chrome, "external Chrome scrape");
    if chrome_json["status_code"] != 200
        || chrome_json["provenance"]["observed_status_code"] != 200
        || !chrome_json["provenance"]["backend_provenance"].is_null()
        || !chrome_json["content"]
            .as_str()
            .unwrap_or_default()
            .contains("It's working!")
    {
        product_fail(format!("external Chrome proof mismatch: {chrome_json:?}"));
    }
    pass(&mut cases, "EXTERNAL_CHROME");

    let mut mcp = McpClient::spawn();
    mcp.initialize();
    let mcp_success = mcp.scrape(HTML_URL);
    if mcp_success["result"]["isError"] != false {
        product_fail(format!("MCP HTTPS acquisition failed: {mcp_success:?}"));
    }
    let mcp_success_evidence = mcp_evidence(&mcp_success);
    if mcp_success_evidence["observed_status_code"] != 200
        || mcp_success_evidence["transport"] != "default"
        || mcp_success_evidence["backend_provenance"] != "reqwest"
        || !mcp_success_evidence["content"]
            .as_str()
            .unwrap_or_default()
            .contains("Moby-Dick")
    {
        product_fail(format!(
            "MCP HTTPS evidence mismatch: {mcp_success_evidence:?}"
        ));
    }
    pass(&mut cases, "MCP_HTTPS_ACQUISITION");

    let mcp_failure = mcp.scrape(NXDOMAIN_URL);
    if mcp_failure["result"]["isError"] != true {
        product_fail(format!(
            "MCP NXDOMAIN became false success: {mcp_failure:?}"
        ));
    }
    let mcp_failure_evidence = mcp_evidence(&mcp_failure);
    if !mcp_failure_evidence["observed_status_code"].is_null()
        || !mcp_failure_evidence["retrieved_at"].is_null()
        || !mcp_failure_evidence["response_origin"].is_null()
    {
        product_fail(format!(
            "MCP failure evidence mismatch: {mcp_failure_evidence:?}"
        ));
    }
    pass(&mut cases, "MCP_FAILURE");
    mcp.shutdown_cleanly();
    pass(&mut cases, "MCP_CLEAN_SHUTDOWN");

    if cases != EXPECTED_CASES {
        product_fail(format!(
            "zero-proof guard: executed {cases}, expected {EXPECTED_CASES}"
        ));
    }
    println!("V1_PRODUCTION_REALITY_CASES={cases}");
}
