//! SCORPION_CANONICAL_SMART_CHROME_FAILURE_STATUS_TRUTHFULNESS_001: closes
//! the exact arrow `target connection refused -> chrome/smart acquisition
//! attempts real navigation -> Chrome renders its own internal
//! chrome-error://chromewebdata interstitial (a genuine, successful CDP
//! load of Chrome's OWN resource, not the target's) -> Page::status_code
//! becomes a literal 200 OK -> caller sees a truthful-looking successful
//! HTTP status for a connection that was actually refused`.
//!
//! Root cause (confirmed via a real local headless-Chrome reproduction,
//! not inferred): `spider::page::is_chrome_error_page` — the function
//! `build()` already relies on to reclassify a rendered Chrome error
//! interstitial's misleading `200` into a truthful failure status —
//! required the rendered template to end with the *exact* byte sequence
//! `};</script></html>` (no `</body>` tag), on the documented assumption
//! that real Chrome error pages never close `<body>` before `</html>`.
//! That assumption is false for this environment's real Chromium output:
//! the actual rendered `ERR_CONNECTION_REFUSED` interstitial ends
//! `};</script></body></html>`. The exact-suffix check silently failed,
//! `build()`'s reclassification block never ran, and the interstitial's
//! own genuinely-observed `200` (correct for loading Chrome's internal
//! resource, wrong for describing the target's origin response) passed
//! through untouched as both `Page::status_code` and
//! `Page::observed_status_code`.
//!
//! The fix (`is_chrome_error_page`) accepts both the bare and the
//! `</body>`-qualified tail as the same real template, and `build()`'s
//! reclassification block now also resets `observed_status_code` to
//! `None` when it fires — no genuine origin HTTP response was ever
//! observed merely because Chrome's own internal error page loaded
//! successfully.
//!
//! Every test below drives the real, public `Website::scrape()` /
//! `scrape_smart()` against a real local headless Chrome instance (this
//! repository's own already-established deterministic Chrome CI
//! environment — no mock browser, no live public network, no secrets,
//! no `RUN_LIVE_TESTS`) with a loopback target that is either a genuine
//! local 200 fixture or a real refused connection.
//!
//! Run serialized (`--test-threads=1`), matching this repo's established
//! convention for Chrome-launch resource contention (see
//! `spider_mcp`/`spider_cli`'s own `headless_chrome_production_stack.rs`
//! suites).
//!
//!   cargo test -p spider --test smart_chrome_failure_status_truthfulness --features chrome -- --test-threads=1
//!   cargo test -p spider --test smart_chrome_failure_status_truthfulness --features smart -- --test-threads=1

#![cfg(feature = "chrome")]
// See `scrape_collector_drain.rs`'s identical accommodation: the chrome-
// capable crawl future's type is large enough that the compiler's
// default query-depth limit is exceeded just computing its layout. Purely
// mechanical compile-time accommodation, no bearing on correctness.
#![recursion_limit = "512"]

use spider::page::Page;
use spider::reqwest::StatusCode;
use spider::tokio;
use spider::website::Website;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

/// Mirrors crate-private `ADDRESS_UNREACHABLE_ERROR` (526) — the status
/// `chrome_permanent_failure_status("ERR_CONNECTION_REFUSED")` maps to.
/// Kept in lockstep here rather than expanding the public surface for a
/// test-only assertion, same convention as `dns_no_retry.rs`'s own
/// mirrored constants.
const ADDRESS_UNREACHABLE_ERROR_U16: u16 = 526;

const BOUND: Duration = Duration::from_secs(60);

async fn bounded<F: std::future::Future>(fut: F) -> F::Output {
    tokio::time::timeout(BOUND, fut)
        .await
        .expect("operation exceeded the bounded timeout — possible hang/regression")
}

/// Run an async test body on a dedicated large-stack `current_thread`
/// runtime. Chrome-enabled futures in this crate are large enough to
/// overflow the default 2 MiB thread stack (established precedent: see
/// `spider/tests/dns_no_retry.rs`'s own `block_on_isolated`, and this
/// same session's `scrape_collector_drain.rs`).
fn run_isolated<F>(body: F) -> F::Output
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build isolated current_thread runtime");
            rt.block_on(body)
        })
        .expect("spawn isolated test thread")
        .join()
        .expect("isolated test thread panicked")
}

/// A real refused-connection URL: bind a loopback listener to claim a
/// free port, then drop it immediately so the port is guaranteed to
/// refuse the next connection attempt.
fn refused_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    drop(listener);
    format!("http://{addr}/")
}

/// A one-shot, single-request loopback HTTP server returning a fixed 200
/// OK body — a genuine local success fixture for the "real 200 stays
/// truthful" regressions.
fn one_shot_http(body: &'static str) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let handle = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0_u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(body.as_bytes());
        }
    });
    (format!("http://{addr}/"), handle)
}

fn only_page(website: &Website) -> Page {
    let pages = website.get_pages().expect("scrape must set self.pages");
    assert_eq!(pages.len(), 1, "expected exactly 1 collected page");
    pages[0].clone()
}

// ---------------------------------------------------------------------
// (1) Refused connection + chrome: must NOT be falsely observed as 200.
// ---------------------------------------------------------------------
#[test]
fn refused_connection_chrome_is_not_falsely_observed_as_200() {
    run_isolated(async move {
        let url = refused_url();
        let mut website = Website::new(&url);
        website.with_limit(1);
        bounded(website.scrape()).await;

        let page = only_page(&website);

        // The truthfulness invariant: a connection refusal must never
        // become a caller-visible truthful HTTP 200. Chrome's own
        // internal chrome-error interstitial DID genuinely load (that's
        // real, useful content -- see the fallback-content assertions
        // below) but that is not the same fact as "the target's origin
        // HTTP response was 200".
        assert_ne!(
            page.status_code,
            StatusCode::OK,
            "a refused connection must never surface as a truthful 200 OK; got {}",
            page.status_code
        );
        assert_eq!(
            page.status_code.as_u16(),
            ADDRESS_UNREACHABLE_ERROR_U16,
            "ERR_CONNECTION_REFUSED must classify to 526 (reachable-target-refused), got {}",
            page.status_code
        );

        // No genuine origin HTTP response was ever observed -- Chrome
        // observed a real 200 for loading its OWN internal error
        // resource, not the target's. observed_status_code's whole
        // purpose ("status observed from an actual CDP response, if one
        // existed") must not be satisfied by that misattribution.
        assert_eq!(
            page.observed_status_code, None,
            "no real origin HTTP response was ever observed for a refused connection; \
             observed_status_code must be None, got {:?}",
            page.observed_status_code
        );

        // 526 is a permanent, non-retryable classification.
        assert!(
            !page.should_retry,
            "526 (address unreachable) must not be marked retryable"
        );
        assert!(!page.needs_retry(), "needs_retry() must be false for 526");
    });
}

// ---------------------------------------------------------------------
// (2) Real local HTTP 200 + chrome: genuine success remains truthful.
// ---------------------------------------------------------------------
//
// Fixing the failure-status defect must not turn a real success into a
// failure -- proves the correction is scoped to the actual chrome-error
// interstitial pattern, not a blanket distrust of chrome-observed 200s.
#[test]
fn real_local_200_chrome_remains_truthful_success() {
    run_isolated(async move {
        let (url, server) = one_shot_http(
            "<html><body>real chrome success fixture, well over five hundred bytes long \
             so it cannot possibly be misclassified by any length-gated detector: \
             padding padding padding padding padding padding padding padding padding \
             padding padding padding padding padding padding padding padding padding \
             padding padding padding padding padding padding padding padding padding.\
             </body></html>",
        );
        let mut website = Website::new(&url);
        website.with_limit(1);
        bounded(website.scrape()).await;
        server.join().expect("fixture server thread");

        let page = only_page(&website);

        assert_eq!(
            page.status_code,
            StatusCode::OK,
            "a genuine real local 200 response must remain a truthful 200, got {}",
            page.status_code
        );
        assert_eq!(
            page.observed_status_code,
            Some(StatusCode::OK),
            "a genuine real local 200 must be truthfully recorded as observed, got {:?}",
            page.observed_status_code
        );
        assert!(
            page.get_html().contains("real chrome success fixture"),
            "genuine success content must still be surfaced"
        );
    });
}

// ---------------------------------------------------------------------
// (3) Fallback-content semantics: the refused-connection page still
// carries real, useful content (Chrome's own rendered interstitial),
// truthfully -- "produced content via fallback" and "observed a real
// origin response" are kept as two independently truthful facts, not
// conflated into one.
// ---------------------------------------------------------------------
#[test]
fn refused_connection_chrome_preserves_fallback_content_truthfully() {
    run_isolated(async move {
        let url = refused_url();
        let mut website = Website::new(&url);
        website.with_limit(1);
        bounded(website.scrape()).await;

        let page = only_page(&website);

        // Existing, preserved product behavior: the interstitial's own
        // content is kept for debugging (see build()'s own comment,
        // untouched by this fix) -- this is real, useful acquisition
        // behavior this frontier must not remove.
        assert!(
            !page.get_html().is_empty(),
            "Chrome's own rendered interstitial content should still be preserved"
        );
        assert!(
            spider::page::is_chrome_error_page(page.get_html().as_bytes()),
            "the preserved content must be structurally recognized as the real Chrome \
             error interstitial (this is the exact detector this frontier fixed)"
        );
        assert_eq!(
            spider::page::extract_chrome_error_code(page.get_html().as_bytes()),
            Some("ERR_CONNECTION_REFUSED"),
            "the real underlying net::ERR_* code must still be extractable from the \
             preserved content"
        );

        // ...while the status facts remain truthful (re-asserted here
        // for locality with the content assertions above).
        assert_ne!(page.status_code, StatusCode::OK);
        assert_eq!(page.observed_status_code, None);
    });
}

// ---------------------------------------------------------------------
// (4) Website::scrape_smart() evidence — the exact public path the
// original anomaly was observed through.
// ---------------------------------------------------------------------
#[cfg(feature = "smart")]
#[test]
fn refused_connection_scrape_smart_is_not_falsely_observed_as_200() {
    run_isolated(async move {
        let url = refused_url();
        let mut website = Website::new(&url);
        website.with_limit(1);
        bounded(website.scrape_smart()).await;

        let page = only_page(&website);

        assert_ne!(
            page.status_code,
            StatusCode::OK,
            "scrape_smart(): a refused connection must never surface as truthful 200 OK, \
             got {}",
            page.status_code
        );
        assert_eq!(
            page.observed_status_code, None,
            "scrape_smart(): observed_status_code must be None for a refused connection, \
             got {:?}",
            page.observed_status_code
        );
        assert!(!page.should_retry);
    });
}

#[cfg(feature = "smart")]
#[test]
fn real_local_200_scrape_smart_remains_truthful_success() {
    run_isolated(async move {
        let (url, server) = one_shot_http("<html><body>smart real success fixture</body></html>");
        let mut website = Website::new(&url);
        website.with_limit(1);
        bounded(website.scrape_smart()).await;
        server.join().expect("fixture server thread");

        let page = only_page(&website);

        assert_eq!(page.status_code, StatusCode::OK);
        assert_eq!(page.observed_status_code, Some(StatusCode::OK));
        assert!(page.get_html().contains("smart real success fixture"));
    });
}

// ---------------------------------------------------------------------
// (5) Non-chrome failure agreement: the plain-HTTP path's own
// established contract (521 / observed_status_code == None) is
// untouched by this fix -- re-verified here (not merely assumed) inside
// the same suite so the two contracts' divergence (521 plain-HTTP vs.
// 526 chrome/smart -- a pre-existing, deliberate distinction already
// encoded in `chrome_permanent_failure_status`'s own doc comment, not
// something this frontier introduced or is asked to unify) is visible
// side by side.
// ---------------------------------------------------------------------
#[test]
fn refused_connection_plain_http_path_unaffected_by_this_fix() {
    run_isolated(async move {
        let url = refused_url();
        let mut website = Website::new(&url);
        website.with_limit(1);
        // crawl_raw() / scrape_raw() sidestep any chrome upgrade even
        // when chrome is compiled in, exercising the same plain HTTP
        // `_crawl_establish` path the default-feature build already
        // covers in scrape_collector_drain.rs.
        bounded(website.scrape_raw()).await;

        let page = only_page(&website);

        assert_eq!(
            page.status_code.as_u16(),
            521,
            "the plain-HTTP path's own established contract (521, \
             CONNECTION_REFUSED_ERROR) must remain unchanged by this fix, got {}",
            page.status_code
        );
        assert_eq!(
            page.observed_status_code, None,
            "the plain-HTTP path's own established observed_status_code contract \
             must remain unchanged, got {:?}",
            page.observed_status_code
        );
    });
}
