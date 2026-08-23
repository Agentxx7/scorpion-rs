#![cfg(feature = "chrome")]

//! SCORPION_HEADLESS_CHROME_PRODUCTION_STACK_SIZE_001.
//!
//! Real end-to-end proof, through the actual shipping `scorpion` binary,
//! that headless/Chrome-backed acquisition works with the operator's
//! ordinary default process environment — no `RUST_MIN_STACK` required.
//! Before this frontier, the identical command reproducibly aborted with
//! `fatal runtime error: stack overflow` at the platform-default thread
//! stack size.
//!
//! The fixture's initial HTML contains a placeholder a raw HTTP fetch
//! would see; a script tag replaces it with a marker only real Chrome/JS
//! execution can produce. This lets every test below distinguish genuine
//! Chrome execution from an HTTP fallback that merely didn't crash.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;

const PRE_JS_PLACEHOLDER: &str = "before-js-placeholder";
const RENDERED_MARKER: &str = "CHROME_RENDERED_MARKER_9f3a1c";

/// Unique per-launch counter feeding a per-process `TMPDIR`. Chrome's
/// default profile directory (absent an explicit `--user-data-dir`, which
/// this CLI subprocess has no flag for) is `$TMPDIR/chromiumoxide-runner`
/// — a *fixed* path every launch would otherwise share. Concurrent or
/// rapid-sequential launches against that one shared profile race on its
/// `SingletonLock` and can hang indefinitely (an established, documented
/// environment quirk of this exact launch path, unrelated to production
/// correctness) — giving each launch its own `TMPDIR` avoids that
/// collision entirely without needing any product code change.
static LAUNCH_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn scorpion() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_scorpion"));
    // The whole point of this test is proving the shipping binary needs no
    // special stack configuration - explicitly remove it even if the
    // ambient test environment happens to have it set, so this test can
    // never be accidentally rescued by inherited configuration.
    command.env_remove("RUST_MIN_STACK");
    let launch = LAUNCH_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp = std::env::temp_dir().join(format!(
        "scorpion-headless-stack-test-{}-{launch}",
        std::process::id()
    ));
    let _ = std::fs::create_dir_all(&tmp);
    command.env("TMPDIR", &tmp);
    command
}

/// Serves the marker fixture on a fresh local port, once, then stops.
fn marker_fixture() -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        let body = format!(
            "<html><head><title>chrome marker test</title></head><body>\
             <div id=\"target\">{PRE_JS_PLACEHOLDER}</div>\
             <script>document.getElementById('target').textContent = '{RENDERED_MARKER}';</script>\
             </body></html>"
        );
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    (url, handle)
}

/// Positive control: the identical fixture over plain HTTP (no
/// `--headless`) must show the pre-JS placeholder, never the marker -
/// proves the fixture genuinely distinguishes rendering from raw HTML.
#[test]
fn plain_http_scrape_sees_pre_js_placeholder_not_the_rendered_marker() {
    let (url, handle) = marker_fixture();
    let output = scorpion().args(["--url", &url, "scrape"]).output().unwrap();
    handle.join().unwrap();

    assert!(output.status.success(), "{:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(PRE_JS_PLACEHOLDER), "{stdout}");
    assert!(!stdout.contains(RENDERED_MARKER), "{stdout}");
}

/// The required negative-then-positive proof: with `RUST_MIN_STACK`
/// explicitly unset, `scorpion --headless scrape` against a real local
/// fixture must not abort, must exit successfully, must report a real
/// observed HTTP response, and must return the JS-rendered marker - proof
/// that real Chrome executed rather than the request hanging, crashing,
/// or silently falling back to HTTP.
#[test]
fn headless_scrape_completes_without_rust_min_stack_and_proves_real_chrome_execution() {
    let (url, handle) = marker_fixture();
    // --output-html: raw rendered HTML, not markdown - avoids markdown's
    // own escaping of `_` inside the marker (`CHROME\_RENDERED\_...`)
    // complicating what is otherwise a direct substring check.
    let output = scorpion()
        .args(["--url", &url, "--headless", "scrape", "--output-html"])
        .output()
        .unwrap();
    handle.join().unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "headless scrape must not abort/fail without RUST_MIN_STACK: \
         status={:?} stdout={stdout} stderr={stderr}",
        output.status
    );
    assert!(
        !stderr.contains("stack overflow"),
        "must never hit the known stack-overflow failure mode: {stderr}"
    );
    let value: serde_json::Value = serde_json::from_str(&stdout).expect(&stdout);
    assert_eq!(value["provenance"]["observed_status_code"], 200, "{value}");
    assert!(
        value["content"].as_str().unwrap().contains(RENDERED_MARKER),
        "content must contain the JS-rendered marker, proving real Chrome execution \
         (not an HTTP fallback masquerading as success): {value}"
    );
    assert!(
        !value["content"]
            .as_str()
            .unwrap()
            .contains(PRE_JS_PLACEHOLDER),
        "content must not still show the pre-JS placeholder: {value}"
    );
}

/// The same proof for `--headless crawl` (a separate call site from
/// `scrape`) - do not assume fixing one caller fixes the others. `crawl`'s
/// own stdout (`-o`) is URLs only, never page content, so this checks the
/// process-level contract crawl actually makes: it does not crash, and it
/// exits successfully only when a real HTTP response was observed
/// (the acquisition-failure-semantics contract established separately) -
/// combined with the marker-content proof on `scrape`/`download` below,
/// which exercise the identical `spawn_crawl_task`/Chrome dispatch this
/// call site shares.
#[test]
fn headless_crawl_completes_without_rust_min_stack() {
    let (url, handle) = marker_fixture();
    let output = scorpion()
        .args(["--url", &url, "--limit", "1", "--headless", "crawl", "-o"])
        .output()
        .unwrap();
    handle.join().unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "headless crawl must not abort/fail without RUST_MIN_STACK: \
         status={:?} stdout={stdout} stderr={stderr}",
        output.status
    );
    assert!(!stderr.contains("stack overflow"), "{stderr}");
    assert!(stdout.contains(&url), "{stdout}");
}

/// The same proof for `--headless download` - materializes real bytes to
/// disk, so (unlike `crawl`) the marker check applies directly to the
/// downloaded file content, proving real Chrome execution reached this
/// call site too.
#[test]
fn headless_download_completes_without_rust_min_stack_and_proves_real_chrome_execution() {
    let (url, handle) = marker_fixture();
    let dest = std::env::temp_dir().join(format!(
        "scorpion-headless-download-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let output = scorpion()
        .args([
            "--url",
            &url,
            "--limit",
            "1",
            "--headless",
            "download",
            "--target-destination",
            dest.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    handle.join().unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "headless download must not abort/fail without RUST_MIN_STACK: \
         status={:?} stderr={stderr}",
        output.status
    );
    assert!(!stderr.contains("stack overflow"), "{stderr}");

    let downloaded = find_downloaded_file(&dest);
    let content = std::fs::read_to_string(&downloaded).unwrap();
    assert!(
        content.contains(RENDERED_MARKER),
        "downloaded file must contain the JS-rendered marker: {content}"
    );
    let _ = std::fs::remove_dir_all(&dest);
}

fn find_downloaded_file(root: &std::path::Path) -> std::path::PathBuf {
    for entry in walkdir(root) {
        if entry.is_file() {
            return entry;
        }
    }
    panic!("no downloaded file found under {root:?}");
}

fn walkdir(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walkdir(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}

/// Two sequential headless requests in independent processes must each
/// succeed - proves the fix does not depend on leftover state from a
/// single run (e.g. an accidentally-reused enlarged main-thread stack).
#[test]
fn sequential_headless_requests_each_succeed_independently() {
    for _ in 0..2 {
        let (url, handle) = marker_fixture();
        let output = scorpion()
            .args(["--url", &url, "--headless", "scrape", "--output-html"])
            .output()
            .unwrap();
        handle.join().unwrap();
        assert!(output.status.success(), "{:?}", output.status);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains(RENDERED_MARKER), "{stdout}");
    }
}
