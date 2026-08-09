pub mod crawl;
pub mod crawl_status;
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

/// Fetch exactly one page through Spider's ordinary non-browser HTTP path.
#[cfg(any(
    feature = "feed",
    feature = "sitemap",
    feature = "news_sitemap",
    feature = "robots_sitemap"
))]
pub(crate) async fn fetch_single_page(url: &str) -> Result<spider::page::Page, String> {
    let mut website = Website::new(url);
    website.with_limit(1);
    let mut website = website.build().map_err(|_| "Invalid URL".to_string())?;
    let mut receiver = website.subscribe(1);
    tokio::spawn(async move {
        website.crawl_raw().await;
        website.unsubscribe();
    });
    receiver
        .recv()
        .await
        .map_err(|_| "Feed retrieval completed without producing a page".to_string())
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
/// unconditionally without it.
#[cfg(feature = "chrome")]
pub fn page_screenshot_bytes(page: &spider::page::Page) -> Option<&[u8]> {
    page.screenshot_bytes.as_deref()
}

#[cfg(not(feature = "chrome"))]
pub fn page_screenshot_bytes(_page: &spider::page::Page) -> Option<&[u8]> {
    None
}

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
