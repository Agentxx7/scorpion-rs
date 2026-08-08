use rmcp::schemars;
use serde::Deserialize;
use serde_json::json;
use spider::tokio;
use spider::website::Website;
use spider_transformations::transformation::content::{
    transform_content_input, ReturnFormat, TransformConfig, TransformInput,
};

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ScrapeParams {
    /// The URL to scrape
    pub url: String,
    /// Output format: raw, markdown, text, xml, or screenshot (default: markdown).
    /// "screenshot" returns a base64-encoded image and implies Chrome
    /// rendering even if `headless` is unset.
    pub return_format: Option<String>,
    /// Use Chrome for JavaScript rendering (requires chrome feature)
    pub headless: Option<bool>,
    /// CSS selector to wait for before extraction
    pub wait_for: Option<String>,
    /// Wait N milliseconds after page load
    pub wait_for_delay_ms: Option<u64>,
    /// Wait for network to become idle
    pub wait_for_idle_network: Option<bool>,
    /// Custom User-Agent string
    pub user_agent: Option<String>,
    /// Cookie string (e.g. "key=val; key2=val2")
    pub cookie: Option<String>,
    /// Proxy URL
    pub proxy: Option<String>,
    /// Opt-in: return an EvidenceBundle (requested/final URL, retrieved_at,
    /// status_code, content_type, content, links, screenshot) instead of
    /// the normal {url, status_code, content, links} shape. Default false —
    /// normal output is unaffected either way.
    pub evidence: Option<bool>,
}

/// Build the evidence-mode result for one fetched page. Reuses the same
/// `content`/`wants_screenshot` values `run()` already computed — no
/// duplicate text-extraction or screenshot logic. Content and screenshot
/// are kept mutually exclusive (never both `Some`): whichever the caller's
/// `return_format` actually produced is the one that's real.
fn build_evidence(
    page: &spider::page::Page,
    content: String,
    wants_screenshot: bool,
) -> crate::evidence::EvidenceBundle {
    let content_type = page
        .headers
        .as_ref()
        .and_then(|h| h.get("content-type"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let links = page.page_links.as_ref().map(|s| {
        s.iter()
            .map(|l| l.inner().to_string())
            .collect::<Vec<String>>()
    });

    crate::evidence::EvidenceBundle {
        requested_url: Some(page.get_url().to_string()),
        final_url: Some(page.get_url_final().to_string()),
        // Always `None`: no canonical retrieval wall-clock timestamp exists
        // anywhere in the reachable scrape path today. `Page` only carries
        // a private, monotonic `Instant` (elapsed-time measurement, not a
        // point in time, and inaccessible from this crate regardless). An
        // MCP-side `SystemTime::now()` taken here would only mark when
        // this function got scheduled to run — not when the network
        // fetch actually completed — and was reverted for exactly that
        // reason (SCORPION_EVIDENCE_BUNDLE_001A). Populating this field
        // honestly requires Spider core to capture the timestamp at fetch
        // completion, which is out of scope here.
        retrieved_at: None,
        status_code: Some(page.status_code.as_u16()),
        content_type,
        content: if wants_screenshot { None } else { Some(content.clone()) },
        links,
        source: None,
        provider: None,
        query: None,
        screenshot: if wants_screenshot { Some(content) } else { None },
        metadata: None,
    }
}

pub async fn run(params: ScrapeParams) -> Result<String, String> {
    let url = if params.url.starts_with("http") {
        params.url.clone()
    } else {
        format!("https://{}", params.url)
    };

    let mut website = Website::new(&url);
    super::apply_spider_cloud(&mut website);

    if let Some(agent) = &params.user_agent {
        website.with_user_agent(Some(agent));
    }
    if let Some(cookie) = &params.cookie {
        website.with_cookies(cookie);
    }
    if let Some(proxy) = &params.proxy {
        if !proxy.is_empty() {
            website.with_proxies(Some(vec![proxy.clone()]));
        }
    }

    super::apply_wait_options(
        &mut website,
        &params.wait_for,
        params.wait_for_delay_ms,
        params.wait_for_idle_network,
    );

    website.configuration.return_page_links = true;
    website.with_limit(1);

    let format_str = params.return_format.as_deref().unwrap_or("markdown");
    let wants_screenshot = super::apply_screenshot_options(&mut website, format_str);

    let mut website = website.build().map_err(|_| "Invalid URL".to_string())?;

    let mut rx = website.subscribe(0);

    let use_headless = params.headless.unwrap_or(false) || wants_screenshot;

    tokio::spawn(async move {
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
    });

    let transform_conf = TransformConfig {
        return_format: ReturnFormat::from_str(format_str),
        ..Default::default()
    };

    let mut results = Vec::new();

    while let Ok(page) = rx.recv().await {
        let input = TransformInput {
            url: page.get_url_parsed_ref().as_ref(),
            content: page.get_html_bytes_u8(),
            screenshot_bytes: super::page_screenshot_bytes(&page),
            encoding: None,
            selector_config: None,
            ignore_tags: None,
        };
        let content = transform_content_input(input, &transform_conf);
        let content = super::screenshot_content_or_error(wants_screenshot, content)?;

        let result = if params.evidence.unwrap_or(false) {
            let evidence = build_evidence(&page, content, wants_screenshot);
            serde_json::to_value(&evidence).map_err(|e| e.to_string())?
        } else {
            let links: Vec<String> = page
                .page_links
                .as_ref()
                .map(|s| s.iter().map(|l| l.inner().to_string()).collect())
                .unwrap_or_default();

            json!({
                "url": page.get_url(),
                "status_code": page.status_code.as_u16(),
                "content": content,
                "links": links,
            })
        };

        results.push(result);
    }

    if results.is_empty() {
        return Err(format!("No content returned for {}", params.url));
    }

    if results.len() == 1 {
        serde_json::to_string_pretty(&results[0]).map_err(|e| e.to_string())
    } else {
        serde_json::to_string_pretty(&results).map_err(|e| e.to_string())
    }
}
