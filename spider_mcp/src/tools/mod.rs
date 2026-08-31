pub mod audit;
pub mod crawl;
pub mod crawl_status;
pub mod evidence_read;
#[cfg(feature = "feed")]
pub mod feed;
pub mod links;
pub mod media_search;
pub mod news_search;
#[cfg(feature = "news_sitemap")]
pub mod news_sitemap;
#[cfg(feature = "robots_sitemap")]
pub mod robots_sitemap;
pub mod scrape;
pub mod search;
#[cfg(feature = "sitemap")]
pub mod sitemap;
pub mod transform;

use spider::features::chrome_common::{WaitForDelay, WaitForIdleNetwork, WaitForSelector};
use spider::website::Website;
#[cfg(feature = "chrome_screenshot")]
use spider_transformations::transformation::content::ReturnFormat;
use std::time::Duration;

/// Fetch exactly one page through Spider's ordinary non-browser HTTP path,
/// honoring the given transport policy — the single seam every
/// source-reader tool (feed/sitemap/news_sitemap/robots_sitemap) funnels
/// through (Section L: `TransportRequest -> TransportPolicy ->
/// AcquisitionOptions -> fetch_single_page_with_options`). No second Tor
/// client is ever constructed here.
#[cfg(any(
    feature = "feed",
    feature = "sitemap",
    feature = "news_sitemap",
    feature = "robots_sitemap"
))]
pub(crate) async fn fetch_document(
    url: &str,
    transport: spider::features::transport::TransportPolicy,
) -> Result<spider::page::Page, String> {
    spider::utils::evidence::fetch_single_page_with_options(
        url,
        spider::utils::evidence::AcquisitionOptions { transport },
    )
    .await
    .map(spider::utils::evidence::TransportAcquisition::into_page)
}

/// Process-wide cap on *concurrently executing* `spawn_chrome_capable`
/// workers (real Chrome/browser process launches — `spider_scrape`/
/// `spider_crawl`/`spider_links` with `headless: true` or
/// `return_format: "screenshot"`), shared by every tool call this server
/// process handles. `rmcp`'s own request dispatch (`serve_inner` in
/// `rmcp::service`) spawns a fresh Tokio task per incoming `tools/call`
/// and never blocks reading the next request on a prior one completing —
/// confirmed by reading its source — so nothing upstream of this module
/// already limits how many of these heavy, real browser-process launches
/// can be in flight at once; without this, N simultaneous headless
/// requests would launch N simultaneous Chrome processes with no bound.
///
/// No existing configuration already owns this resource:
/// `Configuration::concurrency_limit` bounds *page-fetch* parallelism
/// *within* one already-launched crawl (defaulting to system CPU cores),
/// not how many separate browser *processes* this one server process may
/// have running at once — a different, far heavier resource axis (RAM and
/// OS-process count, not page-fetch throughput), so it is deliberately not
/// reused here.
///
/// `4` is a conservative, hand-chosen internal default, not scaled to CPU
/// core count (a real headless Chrome launch is RAM/process-count bound,
/// not CPU-parallelism bound — confirmed live: one launch spawns ~10 OS
/// processes: the browser itself, 2 crashpad handlers, 2 zygotes, a GPU
/// process, network/storage utility processes, and multiple renderers).
/// Not user-configurable — no existing architecture in this crate exposes
/// a public concurrency-tuning surface, so none is added here either.
#[cfg(feature = "chrome")]
static CHROME_EXECUTION_PERMITS: std::sync::LazyLock<tokio::sync::Semaphore> =
    std::sync::LazyLock::new(|| tokio::sync::Semaphore::new(4));

/// Spawn the future that drives one `Website`'s crawl/scrape acquisition.
/// `use_headless` may route into real Chrome/CDP execution, whose call
/// chain has empirically overflowed the default tokio worker-thread stack
/// (confirmed live: a real `spider_scrape`/`spider_crawl` call with
/// `headless: true` hangs indefinitely at that default, and completes
/// normally once the process's minimum thread stack is raised) — route
/// that case through `spider::features::chrome::spawn_chrome_capable`,
/// which runs it on a dedicated, adequately-stacked thread instead, while
/// returning a real `tokio::task::JoinHandle` so every existing caller's
/// `.await`/`.abort_handle()`/panic-propagation contract (already relied
/// on by crawl.rs/scrape.rs/links.rs's transport-error handling) is
/// unchanged.
///
/// When `use_headless` is true, a [`CHROME_EXECUTION_PERMITS`] permit is
/// acquired *before* the dedicated worker is spawned (backpressure — an
/// (N+1)th concurrent headless request waits here rather than launching an
/// (N+1)th Chrome process) and held for the permit's own lifetime inside
/// the spawned future itself, so it is released exactly when that future
/// finishes for any reason — normal completion, an internal Chrome
/// failure (`website.crawl()` never itself returns `Err`, so this is not
/// a distinct case), a panic (caught by `spawn_chrome_capable`'s own
/// `JoinError` propagation, which still drops the future — and therefore
/// the held permit — during unwind), or the returned `JoinHandle` being
/// aborted/dropped by a caller (`AbortOnDrop`, an evicted session, or an
/// inline request future itself being dropped) — never released early,
/// never leaked permanently.
///
/// Ordinary HTTP-only crawls (`use_headless == false`, or any build
/// without the `chrome` feature) never touch the semaphore at all and
/// keep using plain `tokio::spawn`, unaffected — this exists to pay the
/// backpressure/dedicated-thread cost only when Chrome is actually
/// involved.
pub(crate) async fn spawn_crawl_task(
    mut website: Website,
    use_headless: bool,
) -> tokio::task::JoinHandle<Option<spider::features::transport::TransportError>> {
    #[cfg(feature = "chrome")]
    let permit = if use_headless {
        Some(
            CHROME_EXECUTION_PERMITS
                .acquire()
                .await
                .expect("CHROME_EXECUTION_PERMITS is never closed"),
        )
    } else {
        None
    };

    let body = async move {
        #[cfg(feature = "chrome")]
        let _permit = permit; // held for this future's entire lifetime
        #[cfg(feature = "chrome")]
        {
            if use_headless {
                website.crawl().await;
            } else {
                website.crawl_raw().await;
            }
        }
        #[cfg(not(feature = "chrome"))]
        {
            let _ = use_headless;
            website.crawl().await;
        }
        website.last_transport_error().cloned()
    };
    #[cfg(feature = "chrome")]
    if use_headless {
        return spider::features::chrome::spawn_chrome_capable(body);
    }
    tokio::spawn(body)
}

/// Apply common wait-for options to a Website builder.
pub fn apply_wait_options(
    website: &mut Website,
    wait_for: &Option<String>,
    wait_for_delay_ms: Option<u64>,
    wait_for_idle_network: Option<bool>,
) {
    if let Some(selector) = wait_for {
        website.with_wait_for_selector(Some(WaitForSelector::new(
            Some(Duration::from_secs(60)),
            selector.clone(),
        )));
    }
    if let Some(ms) = wait_for_delay_ms {
        website.with_wait_for_delay(Some(WaitForDelay::new(Some(Duration::from_millis(ms)))));
    }
    if wait_for_idle_network == Some(true) {
        website.with_wait_for_idle_network(Some(WaitForIdleNetwork::new(Some(
            Duration::from_millis(500),
        ))));
    }
}

/// Apply Spider Cloud API key if SPIDER_API_KEY env var is set.
pub fn apply_spider_cloud(website: &mut Website) {
    if let Ok(api_key) = std::env::var("SPIDER_API_KEY") {
        if !api_key.is_empty() {
            website.with_spider_cloud(&api_key);
        }
    }
}

/// If `return_format` requests a screenshot, configure Chrome to capture one
/// via Spider's existing `Website::with_screenshot`/`ScreenShotConfig` — no
/// bespoke screenshot engine here, just wiring an opt-in signal to Spider's
/// own capability. Returns whether screenshot capture was requested, so
/// callers can also force headless (Chrome) rendering: a screenshot cannot
/// come from the HTTP-only crawl path.
///
/// No-op (always returns `false`) when the `chrome_screenshot` feature isn't
/// compiled in, matching how the rest of this crate degrades without
/// `chrome` (see `headless` handling in `scrape.rs`/`crawl.rs`).
#[cfg(feature = "chrome_screenshot")]
pub fn apply_screenshot_options(website: &mut Website, return_format: &str) -> bool {
    let wants_screenshot = ReturnFormat::from_str(return_format) == ReturnFormat::Screenshot;
    if wants_screenshot {
        website.with_screenshot(Some(spider::configuration::ScreenShotConfig::new(
            Default::default(),
            true,
            false,
            None,
        )));
    }
    wants_screenshot
}

#[cfg(not(feature = "chrome_screenshot"))]
pub fn apply_screenshot_options(_website: &mut Website, _return_format: &str) -> bool {
    false
}

/// Extract a page's captured screenshot bytes, if any. `Page::screenshot_bytes`
/// only exists at all behind the `chrome` feature, so this is `None`
/// unconditionally without it. The canonical implementation now lives in
/// `spider::utils::evidence`; re-exported here so every existing
/// crate-local call site (`crawl.rs`, `scrape.rs`) keeps working unchanged.
pub use spider::utils::evidence::page_screenshot_bytes;

/// Fail-closed screenshot semantics, shared by `scrape` and `crawl` so both
/// tools apply the identical rule: a caller who explicitly asked for
/// `return_format = "screenshot"` must never get back a silent, empty-content
/// success when no browser-generated screenshot bytes were actually captured
/// (Chrome unavailable, render failed, etc.) — that must surface as an
/// explicit error instead. Non-screenshot requests, and screenshot requests
/// that did produce content, pass through unchanged.
///
pub fn screenshot_content_or_error(
    wants_screenshot: bool,
    content: String,
) -> Result<String, String> {
    if wants_screenshot && content.is_empty() {
        Err(
            "Screenshot capture was requested (return_format=\"screenshot\") but no \
             browser-generated screenshot was available for this page."
                .to_string(),
        )
    } else {
        Ok(content)
    }
}

#[cfg(test)]
mod tests {
    use spider_transformations::transformation::content::{
        transform_content_input, ReturnFormat, TransformConfig, TransformInput,
    };

    /// SCORPION_HEADLESS_CHROME_PRODUCTION_STACK_SIZE_001 (concurrency
    /// closure). A panic inside a `spawn_chrome_capable` future — exactly
    /// the shape `spawn_crawl_task` uses, holding a
    /// `CHROME_EXECUTION_PERMITS` permit for the future's own lifetime —
    /// must not leak that permit. Proves the RAII release survives the
    /// panic-driven unwind through `spawn_chrome_capable`'s dedicated
    /// worker thread, using the real production semaphore, not a
    /// throwaway one.
    #[cfg(feature = "chrome")]
    #[tokio::test]
    async fn chrome_execution_permit_releases_on_panic() {
        let before = super::CHROME_EXECUTION_PERMITS.available_permits();
        let permit = super::CHROME_EXECUTION_PERMITS.acquire().await.unwrap();
        assert_eq!(
            super::CHROME_EXECUTION_PERMITS.available_permits(),
            before - 1
        );

        let handle = spider::features::chrome::spawn_chrome_capable(async move {
            let _permit = permit; // held for this future's lifetime, exactly like spawn_crawl_task's body
            panic!("intentional failure for chrome_execution_permit_releases_on_panic");
        });
        let result = handle.await;
        assert!(result.is_err());
        assert!(result.unwrap_err().is_panic());

        assert_eq!(
            super::CHROME_EXECUTION_PERMITS.available_permits(),
            before,
            "the permit must be released, not leaked, after the panic propagates"
        );
    }

    /// RED: proves the MCP surface's screenshot pipeline is broken today.
    /// `spider_mcp` calls into `spider_transformations::transform_content_input`
    /// for every return format, including `ReturnFormat::Screenshot` — but that
    /// crate's `screenshot` feature (which is what turns real screenshot bytes
    /// into the base64 `content` string) is never enabled anywhere in this
    /// workspace, so even a correctly populated `screenshot_bytes` input
    /// silently comes back as an empty string. This test exercises exactly
    /// that call with real bytes present and asserts non-empty output — no
    /// network, no Chrome, fully deterministic. Gated on `chrome_screenshot`
    /// so it only runs (and only need pass) in builds that claim screenshot
    /// support.
    #[cfg(feature = "chrome_screenshot")]
    #[test]
    fn screenshot_return_format_produces_base64_content() {
        let dummy_bytes = b"not a real png, just bytes to round-trip";
        let input = TransformInput {
            url: None,
            content: b"<html></html>",
            screenshot_bytes: Some(dummy_bytes),
            encoding: None,
            selector_config: None,
            ignore_tags: None,
        };
        let conf = TransformConfig {
            return_format: ReturnFormat::Screenshot,
            ..Default::default()
        };

        let out = transform_content_input(input, &conf);

        assert!(
            !out.is_empty(),
            "return_format=screenshot with real screenshot bytes present must not \
             silently return empty content — the spider_transformations `screenshot` \
             feature must be enabled for the `chrome_screenshot` build to work"
        );
    }

    // -------------------------------------------------------------------
    // SCORPION_MCP_SCREENSHOT_OUTPUT_001A — fail-closed screenshot semantics.
    // Pure, offline, no feature gate needed: the helper never touches
    // Website/Page/Chrome types.
    // -------------------------------------------------------------------

    /// CURRENT-BEHAVIOR PROOF + RED: today, a screenshot request with no
    /// captured bytes silently "succeeds" with empty content instead of
    /// erroring. This is exactly the shape `scrape::run`/`crawl::run` hit
    /// when `Page.screenshot_bytes == None` (Chrome unavailable, render
    /// failed, etc.) — `transform_content_input` returns `""`, and nothing
    /// today rejects that before it becomes a 200-shaped Ok result.
    #[test]
    fn screenshot_without_bytes_must_error_not_silently_succeed() {
        let result = super::screenshot_content_or_error(true, String::new());
        assert!(
            result.is_err(),
            "a screenshot request that produced no bytes must return an explicit \
             error, not Ok(empty) — got {result:?}"
        );
        let msg = result.unwrap_err();
        assert!(
            msg.to_lowercase().contains("screenshot"),
            "error message should clearly reference screenshot capture: {msg}"
        );
    }

    /// A screenshot request that DID produce content must still succeed
    /// unchanged — the fail-closed check must not become a false positive.
    #[test]
    fn screenshot_with_bytes_still_succeeds() {
        let result = super::screenshot_content_or_error(true, "some-base64==".to_string());
        assert_eq!(result, Ok("some-base64==".to_string()));
    }

    /// Empty content for a *non*-screenshot format (e.g. a genuinely blank
    /// page) is not a screenshot failure and must pass through unchanged.
    #[test]
    fn non_screenshot_request_with_empty_content_is_unaffected() {
        let result = super::screenshot_content_or_error(false, String::new());
        assert_eq!(result, Ok(String::new()));
    }
}
