//! SCORPION_CANONICAL_CHROME_CACHE_PROVENANCE_POPULATION_001: closes the
//! addressable half of the provenance gap the preceding frontier
//! (SCORPION_CANONICAL_SMART_CHROME_FAILURE_STATUS_TRUTHFULNESS_001)
//! discovered but explicitly declined to fix without first auditing the
//! architecture: `Page::response_origin()`/`Page::backend_provenance()`
//! staying `None` for both successful and failed Chrome acquisitions.
//!
//! Preflight audit (see this frontier's own final report for the full
//! evidence) established that this gap has TWO structurally different
//! answers, not one:
//!
//! - **Real Chrome/CDP navigation** (success or failure alike):
//!   `spider_transport::BackendProvenance`'s variants (`Reqwest`, `Wreq`,
//!   `CacheLayer`, `NoncanonicalFetchEngine`, `NoncanonicalRemoteFetcher`,
//!   `UpstreamCompatibility`) are exhaustively about the HTTP-transport
//!   execution seam (`CrawlerResponse`/`CrawlerFailure`, only ever
//!   constructed from a genuine `reqwest`/`wreq` response or error).
//!   Chrome navigation never constructs either type — it drives a real
//!   browser over CDP, an entirely different acquisition mechanism this
//!   enum was never designed to describe. Forcing Chrome into any
//!   existing variant would be a false claim. **MODEL_INSUFFICIENT** —
//!   `None`/`None` remains the truthful answer here, unchanged by this
//!   frontier, and asserted below as the correct (not merely
//!   unaddressed) outcome.
//!
//! - **Scorpion's own canonical cache hit** (the `skip_browser`/
//!   `get_cached_url` early-return, and `Website::try_cache_
//!   shortcircuit`'s own use of the identical
//!   `build_cached_html_page_response` constructor): this is the exact
//!   same `CACACHE_MANAGER`-backed disk/mem cache `cache_request.rs`'s
//!   own `reconstruct_response` already truthfully stamps
//!   `ResponseOrigin::ReconstructedCache` / `BackendProvenance::
//!   CacheLayer` for. **MODEL_SUFFICIENT** — the already-known,
//!   already-modeled fact was simply never propagated at this call site.
//!   Fixed: `build_cached_html_page_response` now stamps both fields.
//!
//! Every test below drives the real, public `Website::scrape()`/`crawl()`
//! against real loopback fixtures — real headless Chrome where Chrome
//! truth is being proven, no mock browser, no public network, no
//! secrets, no `RUN_LIVE_TESTS`.
//!
//!   cargo test -p spider --test chrome_cache_provenance_population --features chrome -- --test-threads=1
//!   cargo test -p spider --test chrome_cache_provenance_population --features cache_chrome_hybrid -- --test-threads=1

#![cfg(feature = "chrome")]
#![recursion_limit = "512"]

use spider::page::Page;
use spider::reqwest::StatusCode;
use spider::tokio;
use spider::website::Website;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

const BOUND: Duration = Duration::from_secs(60);

async fn bounded<F: std::future::Future>(fut: F) -> F::Output {
    tokio::time::timeout(BOUND, fut)
        .await
        .expect("operation exceeded the bounded timeout — possible hang/regression")
}

/// See `smart_chrome_failure_status_truthfulness.rs`'s identical helper
/// for the full rationale (large-stack isolated runtime for chrome
/// futures).
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

fn refused_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    drop(listener);
    format!("http://{addr}/")
}

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
// (K) Real Chrome 200 — provenance stays None/None truthfully. Not a
// regression: a genuine, evidence-backed statement that the current
// canonical model cannot describe real Chrome navigation, so it
// correctly declines to guess rather than lying.
// ---------------------------------------------------------------------
#[test]
fn real_chrome_200_provenance_stays_truthfully_unknown() {
    run_isolated(async move {
        let (url, server) = one_shot_http("<html><body>chrome provenance fixture</body></html>");
        let mut website = Website::new(&url);
        website.with_limit(1);
        bounded(website.scrape()).await;
        server.join().expect("fixture server thread");

        let page = only_page(&website);

        assert_eq!(page.status_code, StatusCode::OK);
        assert_eq!(page.observed_status_code, Some(StatusCode::OK));
        assert_eq!(
            page.response_origin(),
            None,
            "no BackendProvenance/ResponseOrigin variant can truthfully represent a real \
             CDP/Chrome navigation today -- None is the correct, non-fabricated answer, \
             got {:?}",
            page.response_origin()
        );
        assert_eq!(
            page.backend_provenance(),
            None,
            "same as response_origin -- got {:?}",
            page.backend_provenance()
        );
    });
}

// ---------------------------------------------------------------------
// (L) Real Chrome refused connection — provenance stays None/None
// truthfully, and the status-truthfulness contract from the preceding
// frontier remains intact.
// ---------------------------------------------------------------------
#[test]
fn chrome_refused_connection_provenance_stays_truthfully_unknown() {
    run_isolated(async move {
        let url = refused_url();
        let mut website = Website::new(&url);
        website.with_limit(1);
        bounded(website.scrape()).await;

        let page = only_page(&website);

        assert_eq!(page.status_code.as_u16(), 526);
        assert_eq!(page.observed_status_code, None);
        assert_eq!(
            page.response_origin(),
            None,
            "a refused connection must not manufacture ANY provenance claim, truthful or \
             otherwise -- got {:?}",
            page.response_origin()
        );
        assert_eq!(page.backend_provenance(), None);
        // Fallback content remains available, per the preceding frontier's
        // own contract -- untouched here.
        assert!(!page.get_html().is_empty());
    });
}

// ---------------------------------------------------------------------
// (N) Cache miss -> real network: provenance must be Network/Reqwest,
// never mislabeled Cache merely because caching is enabled.
// ---------------------------------------------------------------------
#[cfg(feature = "cache_chrome_hybrid")]
#[test]
fn cache_enabled_miss_still_reports_truthful_network_origin() {
    run_isolated(async move {
        let (url, server) = one_shot_http("<html><body>cache miss fixture</body></html>");
        let mut website = Website::new(&url);
        website
            .with_limit(1)
            .with_caching(true)
            .with_cache_skip_browser(true);
        // First request against a fresh (never-cached) URL: a genuine
        // miss must fall through to real Chrome navigation, not be
        // mislabeled as a cache hit merely because caching is enabled.
        bounded(website.scrape()).await;
        server.join().expect("fixture server thread");

        let page = only_page(&website);
        assert_eq!(page.status_code, StatusCode::OK);
        // This is real Chrome navigation (the miss path), so the same
        // MODEL_INSUFFICIENT truth from (K) applies -- must NOT be
        // mislabeled ReconstructedCache/CacheLayer merely because
        // caching was enabled.
        assert_ne!(
            page.response_origin(),
            Some(spider_transport::ResponseOrigin::ReconstructedCache),
            "a genuine cache MISS must never be mislabeled as a cache hit"
        );
        assert_ne!(
            page.backend_provenance(),
            Some(spider_transport::BackendProvenance::CacheLayer),
            "a genuine cache MISS must never be mislabeled as a cache hit"
        );
    });
}

// ---------------------------------------------------------------------
// (M) Cache hit truthfulness: real round-trip through the actual
// production skip_browser/get_cached_url path (not a fabricated cache
// object) -- second request against the SAME URL, after the first
// request's real network fetch populated Scorpion's own canonical
// cache, must report ReconstructedCache/CacheLayer.
//
// (Deterministic disk-cache-fixture coverage of the identical
// `build_cached_html_page_response` constructor, via
// `Website::try_cache_shortcircuit`, already lives in
// `spider/src/website.rs`'s own `test_cache_shortcircuit_single_page`/
// `test_cache_shortcircuit_single_page_mem` -- extended by this same
// frontier with response_origin/backend_provenance assertions. This
// test proves the SAME fix from the other reachable direction: a real
// two-request round trip through actual Chrome+cache production code,
// not a pre-seeded fixture.)
// ---------------------------------------------------------------------
#[cfg(feature = "cache_chrome_hybrid")]
#[test]
fn real_round_trip_cache_hit_reports_truthful_reconstructed_cache_origin() {
    run_isolated(async move {
        let (url, server) =
            one_shot_http("<html><body>real round-trip cache fixture</body></html>");

        // Request 1: genuine miss -> real network fetch, populates
        // Scorpion's own canonical CACACHE_MANAGER-backed cache.
        let mut w1 = Website::new(&url);
        w1.with_limit(1)
            .with_caching(true)
            .with_cache_skip_browser(true);
        bounded(w1.scrape()).await;
        server.join().expect("fixture server thread");

        // Request 2: fresh Website instance, same URL/namespace -> a
        // real cache hit through the actual production skip_browser
        // early return this frontier fixed.
        let mut w2 = Website::new(&url);
        w2.with_limit(1)
            .with_caching(true)
            .with_cache_skip_browser(true);
        bounded(w2.scrape()).await;

        let page = only_page(&w2);
        assert!(
            page.get_html().contains("real round-trip cache fixture"),
            "cache hit must still return the real cached content"
        );
        assert_eq!(page.status_code, StatusCode::OK);
        assert_eq!(
            page.observed_status_code, None,
            "no real HTTP response was observed on the cache-hit request itself"
        );
        assert_eq!(
            page.response_origin(),
            Some(spider_transport::ResponseOrigin::ReconstructedCache),
            "a genuine cache hit must truthfully report ReconstructedCache, got {:?}",
            page.response_origin()
        );
        assert_eq!(
            page.backend_provenance(),
            Some(spider_transport::BackendProvenance::CacheLayer),
            "a genuine cache hit must truthfully report CacheLayer, got {:?}",
            page.backend_provenance()
        );
    });
}
