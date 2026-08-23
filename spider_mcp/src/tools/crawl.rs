use rmcp::schemars;
use serde::Deserialize;
use serde_json::json;
use spider::tokio;
use spider::website::Website;
use spider_transformations::transformation::content::{
    transform_content_input, ReturnFormat, TransformConfig, TransformInput,
};
use std::sync::Arc;
use std::time::Instant;

use crate::state::{CrawlPageResult, CrawlSession, CrawlSessionStatus, SharedState};

#[derive(Deserialize, schemars::JsonSchema)]
pub struct CrawlParams {
    /// The starting URL to crawl
    pub url: String,
    /// Maximum number of pages to crawl (default: 25)
    pub limit: Option<u32>,
    /// Maximum crawl depth (default: 25)
    pub depth: Option<usize>,
    /// Output format: raw, markdown, text, xml, or screenshot (default: markdown).
    /// "screenshot" returns each page's base64-encoded image and implies
    /// Chrome rendering even if `headless` is unset.
    pub return_format: Option<String>,
    /// Honor robots.txt (default: true)
    pub respect_robots_txt: Option<bool>,
    /// Include subdomains (default: false)
    pub subdomains: Option<bool>,
    /// Use Chrome for JavaScript rendering
    pub headless: Option<bool>,
    /// Polite crawl delay in milliseconds
    pub delay_ms: Option<u64>,
    /// URL patterns to skip
    pub blacklist_urls: Option<Vec<String>>,
    /// Only crawl matching URL patterns
    pub whitelist_urls: Option<Vec<String>>,
    /// Additional domains to follow
    pub external_domains: Option<Vec<String>>,
    /// Proxy URL
    pub proxy: Option<String>,
    /// Custom User-Agent string
    pub user_agent: Option<String>,
    /// Transport for this crawl. Omit for Default (normal networking).
    pub transport: Option<crate::transport::TransportParam>,
    /// Explicit CAPTCHA provider to route a detected browser challenge
    /// through (requires headless=true): "local-language-model",
    /// "external-gemini", "openai-vision", or "paligemma-local". Omit for
    /// no provider routing (default) — a detected challenge then resolves
    /// to a typed not-configured outcome without constructing any provider
    /// or registry. Selecting a provider this server was not compiled with
    /// is not rejected here; the canonical router still truthfully reports
    /// provider-unavailable at resolution time. No fallback to a different
    /// provider.
    pub captcha_provider: Option<String>,
}

/// Threshold: crawls at or below this limit run inline; above run in background.
const INLINE_LIMIT: u32 = 10;

fn serialize_inline_pages(
    pages: Vec<serde_json::Value>,
    observed_response_count: usize,
) -> Result<String, String> {
    let diagnostic =
        serde_json::to_string_pretty(&json!({ "pages": pages })).map_err(|e| e.to_string())?;
    if observed_response_count == 0 {
        Err(diagnostic)
    } else {
        Ok(diagnostic)
    }
}

pub async fn run(params: CrawlParams, state: Arc<SharedState>) -> Result<String, String> {
    let url = if params.url.starts_with("http") {
        params.url.clone()
    } else {
        format!("https://{}", params.url)
    };

    let limit = params.limit.unwrap_or(25);
    let mut website = Website::new(&url);
    super::apply_spider_cloud(&mut website);

    website
        .with_respect_robots_txt(params.respect_robots_txt.unwrap_or(true))
        .with_subdomains(params.subdomains.unwrap_or(false))
        .with_limit(limit);

    if let Some(depth) = params.depth {
        website.with_depth(depth);
    }
    if let Some(delay) = params.delay_ms {
        website.with_delay(delay);
    }
    if let Some(agent) = &params.user_agent {
        website.with_user_agent(Some(agent));
    }
    if let Some(proxy) = &params.proxy {
        if !proxy.is_empty() {
            website.with_proxies(Some(vec![proxy.clone()]));
        }
    }
    if let Some(blacklist) = &params.blacklist_urls {
        website.with_blacklist_url(Some(
            blacklist
                .iter()
                .map(|s| s.as_str().into())
                .collect::<Vec<spider::compact_str::CompactString>>(),
        ));
    }
    if let Some(whitelist) = &params.whitelist_urls {
        website.with_whitelist_url(Some(
            whitelist
                .iter()
                .map(|s| s.as_str().into())
                .collect::<Vec<spider::compact_str::CompactString>>(),
        ));
    }
    if let Some(domains) = params.external_domains.clone() {
        website.with_external_domains(Some(domains.into_iter()));
    }

    // Explicit CAPTCHA-provider selection
    // (SCORPION_CANONICAL_CAPTCHA_PROVIDER_PUBLIC_SELECTION_BINDING_001):
    // parsed through the same canonical, closed vocabulary the CLI reuses
    // (`CaptchaProviderId::from_str_exact`) — no MCP-owned provider string
    // list. Sets only `Configuration::captcha_provider`; no registry, no
    // provider construction, no browser action happens here. That all
    // remains owned by `spider`'s canonical router, reached only if a real
    // browser challenge is later detected during this same crawl.
    #[cfg(feature = "chrome")]
    if let Some(provider) = &params.captcha_provider {
        website.configuration.captcha_provider = Some(
            spider::features::captcha::CaptchaProviderId::from_str_exact(provider).ok_or_else(
                || {
                    format!(
                        "unknown captcha_provider {provider:?}; expected one of {:?}",
                        spider::features::captcha::CaptchaProviderId::KNOWN
                            .iter()
                            .map(|id| id.as_str())
                            .collect::<Vec<_>>()
                    )
                },
            )?,
        );
    }
    #[cfg(not(feature = "chrome"))]
    if params.captcha_provider.is_some() {
        return Err(
            "captcha_provider requires this server to be built with the `chrome` feature"
                .to_string(),
        );
    }

    website.configuration.return_page_links = true;

    let format_str = params.return_format.as_deref().unwrap_or("markdown");
    let return_format = ReturnFormat::from_str(format_str);
    let wants_screenshot = super::apply_screenshot_options(&mut website, format_str);

    let transport_policy = crate::transport::resolve(params.transport)?;
    let tor_requested = matches!(
        transport_policy,
        spider::features::transport::TransportPolicy::Tor(_)
    );
    let use_headless = params.headless.unwrap_or(false) || wants_screenshot;
    // Fail closed before any target networking, browser launch included
    // (Section I/M): Tor crawling is HTTP-only.
    if tor_requested && use_headless {
        return Err(
            "transport.mode=\"tor\" cannot be combined with headless=true or \
             return_format=\"screenshot\" — Tor crawling is HTTP-only"
                .to_string(),
        );
    }
    if tor_requested {
        website.with_transport(transport_policy);
    }

    let mut website = website.build().map_err(|_| "Invalid URL".to_string())?;
    let mut rx = website.subscribe(16);

    // Captured (not fire-and-forget) so a Tor preflight rejection can
    // never surface as an empty successful crawl — inline (Section H) or
    // background (Section N) — and `.abort_handle()` still lets an inline
    // request dropped by the client (or a session evicted) cancel the
    // crawl exactly as before.
    let crawl_join = super::spawn_crawl_task(website, use_headless).await;
    let crawl_abort = crawl_join.abort_handle();

    if limit <= INLINE_LIMIT {
        // Cancel the crawl if this request future is dropped (client disconnect).
        // No-op on success: the drain loop below only ends once the crawl has
        // finished and closed the channel.
        let _abort_on_drop = crate::state::AbortOnDrop(crawl_abort);

        let transform_conf = TransformConfig {
            return_format,
            ..Default::default()
        };

        let mut pages = Vec::new();
        let mut observed_response_count = 0usize;

        while let Ok(page) = rx.recv().await {
            observed_response_count += usize::from(page.observed_status_code.is_some());
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
            let links: Vec<String> = page
                .page_links
                .as_ref()
                .map(|s| s.iter().map(|l| l.inner().to_string()).collect())
                .unwrap_or_default();

            #[cfg_attr(not(feature = "chrome"), allow(unused_mut))]
            let mut page_json = json!({
                "url": page.get_url(),
                "status_code": page.status_code.as_u16(),
                "content": content,
                "links": links,
                "provenance": spider::utils::evidence::page_provenance(&page),
            });
            // Public CAPTCHA-outcome observability
            // (SCORPION_CANONICAL_CAPTCHA_PROVIDER_PUBLIC_SELECTION_BINDING_001):
            // reports the same canonical, read-only summary the router
            // itself produced — never reconstructs detection, never builds
            // a registry, never calls a provider. Present only when the
            // caller explicitly selected a provider.
            #[cfg(feature = "chrome")]
            if params.captcha_provider.is_some() {
                if let Some(observation) = page.detected_browser_challenge() {
                    page_json["captcha_observation"] = json!(format!("{observation:?}"));
                }
            }
            pages.push(page_json);
        }

        if pages.is_empty() {
            if let Ok(Some(transport_error)) = crawl_join.await {
                return Err(transport_error.to_string());
            }
        }

        serialize_inline_pages(pages, observed_response_count)
    } else {
        let crawl_id = uuid::Uuid::new_v4().to_string();
        // Reclaim finished sessions before adding a new one so the server's
        // memory stays bounded over its lifetime.
        state.evict_stale_sessions();
        state.sessions.insert(
            crawl_id.clone(),
            CrawlSession {
                status: CrawlSessionStatus::Running,
                pages: Vec::new(),
                started_at: Instant::now(),
                abort: Some(crawl_abort),
                error: None,
                error_code: None,
            },
        );

        let state2 = state.clone();
        let id2 = crawl_id.clone();

        tokio::spawn(async move {
            let transform_conf = TransformConfig {
                return_format,
                ..Default::default()
            };

            // Set on a fail-closed screenshot rejection so the loop's normal
            // exit (channel closed) doesn't overwrite Failed with Complete.
            let mut failed = false;
            // Section (background crawl failure semantics): counts only
            // pages with a real observed HTTP response
            // (`page.observed_status_code.is_some()`) — the same signal
            // the inline path's `observed_response_count` already uses
            // (see `serialize_inline_pages` above). This is a purely
            // internal gating variable; it is never the externally
            // reported `page_count` field (`spider_crawl_status`
            // independently computes that from `session.pages.len()`,
            // which still retains every received page regardless of
            // observed status). A prior version counted every received
            // page here, so a refused acquisition's one synthetic
            // diagnostic page (no real response, `observed_status_code:
            // null`) satisfied this as "non-zero" and bypassed the
            // zero-page failure path entirely, reaching `Complete` with
            // zero observed HTTP responses.
            let mut observed_response_count: usize = 0;

            while let Ok(page) = rx.recv().await {
                observed_response_count += usize::from(page.observed_status_code.is_some());
                let input = TransformInput {
                    url: page.get_url_parsed_ref().as_ref(),
                    content: page.get_html_bytes_u8(),
                    screenshot_bytes: super::page_screenshot_bytes(&page),
                    encoding: None,
                    selector_config: None,
                    ignore_tags: None,
                };
                let content = transform_content_input(input, &transform_conf);
                let content = match super::screenshot_content_or_error(wants_screenshot, content) {
                    Ok(content) => content,
                    Err(_) => {
                        // Same fail-closed rule as the inline path: a
                        // screenshot request that produced no bytes must not
                        // surface as a silent success — mark the session
                        // failed and stop, rather than completing with a
                        // page missing its requested screenshot.
                        if let Some(mut session) = state2.sessions.get_mut(&id2) {
                            session.status = CrawlSessionStatus::Failed;
                        }
                        failed = true;
                        break;
                    }
                };
                let links: Vec<String> = page
                    .page_links
                    .as_ref()
                    .map(|s| s.iter().map(|l| l.inner().to_string()).collect())
                    .unwrap_or_default();

                let provenance = spider::utils::evidence::page_provenance(&page);
                if let Some(mut session) = state2.sessions.get_mut(&id2) {
                    session.pages.push(CrawlPageResult {
                        url: page.get_url().to_string(),
                        content,
                        status_code: page.status_code.as_u16(),
                        links,
                        provenance,
                    });
                }
            }

            if !failed {
                if observed_response_count > 0 {
                    if let Some(mut session) = state2.sessions.get_mut(&id2) {
                        session.status = CrawlSessionStatus::Complete;
                    }
                } else {
                    // Zero observed HTTP responses — a genuine total
                    // acquisition failure (the background sibling of the
                    // inline path's zero-observed-response guard in
                    // `serialize_inline_pages`), regardless of how many
                    // raw/synthetic pages were retained for diagnostics.
                    // `crawl_join` only ever carries a typed
                    // `TransportError` for executor/Tor preflight
                    // resolution failures (audited: `last_transport_error`
                    // is set exclusively by `resolve_canonical_executor`
                    // failures, never by an ordinary per-request network
                    // failure such as a refused connection) — it is
                    // awaited here only to capture that class when it
                    // genuinely applies; its absence must not be read as
                    // success. It is safe to await unconditionally here:
                    // the spawned crawl task has already finished (or is
                    // finishing) by the time this channel closes, exactly
                    // as before this change.
                    let transport_error = crawl_join.await.ok().flatten();
                    if let Some(mut session) = state2.sessions.get_mut(&id2) {
                        session.status = CrawlSessionStatus::Failed;
                        if let Some(transport_error) = transport_error {
                            session.error = Some(transport_error.to_string());
                            session.error_code =
                                Some(crate::transport::error_code(&transport_error).to_string());
                        }
                    }
                }
            }
        });

        serde_json::to_string_pretty(&json!({
            "crawl_id": crawl_id,
            "status": "running",
            "message": format!("Crawl started. Use spider_crawl_status tool or read resource crawl://{crawl_id}/summary to check progress."),
        }))
        .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_total_failure_preserves_pages_in_error_payload() {
        let failed_page = json!({
            "status_code": 521,
            "content": "",
            "provenance": {
                "observed_status_code": null,
                "response_origin": null
            }
        });

        let output = serialize_inline_pages(vec![failed_page.clone()], 0).unwrap_err();
        let diagnostic: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(diagnostic["pages"], json!([failed_page]));
    }

    #[test]
    fn inline_partial_success_remains_successful() {
        let observed = json!({ "provenance": { "observed_status_code": 200 } });
        let failed = json!({ "provenance": { "observed_status_code": null } });

        let output = serialize_inline_pages(vec![observed, failed], 1).unwrap();
        let result: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(result["pages"].as_array().unwrap().len(), 2);
    }

    fn tor_crawl_params(
        url: String,
        limit: Option<u32>,
        transport: Option<crate::transport::TransportParam>,
    ) -> CrawlParams {
        CrawlParams {
            url,
            limit,
            depth: None,
            return_format: Some("raw".to_string()),
            respect_robots_txt: Some(false),
            subdomains: None,
            headless: Some(false),
            delay_ms: None,
            blacklist_urls: None,
            whitelist_urls: None,
            external_domains: None,
            proxy: None,
            user_agent: None,
            transport,
            captcha_provider: None,
        }
    }

    async fn await_terminal(state: &SharedState, crawl_id: &str) -> CrawlSessionStatus {
        for _ in 0..200 {
            if let Some(session) = state.sessions.get(crawl_id) {
                if session.status != CrawlSessionStatus::Running {
                    return session.status.clone();
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("session {crawl_id} never left Running within the poll budget");
    }

    /// V5: an inline Tor crawl (limit <= INLINE_LIMIT) reaches the seed
    /// exclusively via SOCKS.
    #[cfg(feature = "transport_tor")]
    #[tokio::test]
    async fn tor_inline_crawl_reaches_target_only_via_socks() {
        let http =
            crate::test_support::HttpFixture::start("<html><body>tor inline crawl</body></html>");
        let socks = crate::test_support::SocksFixture::start(
            Some(http.addr),
            crate::test_support::SocksBehavior::Splice,
        );
        let url = format!(
            "http://crawl-tor-inline-mcp-test.invalid:{}/",
            http.addr.port()
        );
        let state = Arc::new(SharedState::new());

        let output = run(
            tor_crawl_params(
                url,
                Some(1),
                Some(crate::transport::TransportParam {
                    mode: Some(crate::transport::TransportModeParam::Tor),
                    proxy: Some(format!("socks5h://{}", socks.addr)),
                }),
            ),
            state,
        )
        .await
        .unwrap();

        assert_eq!(http.hit_count(), 1);
        assert_eq!(socks.connect_count(), 1);
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["pages"].as_array().unwrap().len(), 1);
    }

    /// I (blocker-fix frontier): a genuine INLINE `spider_crawl` (limit <=
    /// INLINE_LIMIT) Tor preflight rejection surfaces as a public-tool
    /// `Err`, never `Ok` with `{"pages":[]}`. Legacy proxy list + Tor is an
    /// audited-incompatible combination (`Website::tor_crawl_preflight`),
    /// the same genuine condition the background-path sibling test below
    /// uses — this is its inline-path counterpart, since the inline and
    /// background branches propagate `last_transport_error()` through
    /// entirely different code (an immediate `Err` return vs. a stored
    /// `CrawlSession`).
    #[tokio::test]
    async fn tor_inline_crawl_preflight_rejection_is_err_not_empty_pages() {
        let http = crate::test_support::HttpFixture::start("<html></html>");
        let socks = crate::test_support::SocksFixture::start(
            Some(http.addr),
            crate::test_support::SocksBehavior::Splice,
        );
        let url = format!(
            "http://crawl-tor-inline-reject-mcp-test.invalid:{}/",
            http.addr.port()
        );
        let state = Arc::new(SharedState::new());

        let mut params = tor_crawl_params(
            url,
            Some(1),
            Some(crate::transport::TransportParam {
                mode: Some(crate::transport::TransportModeParam::Tor),
                proxy: Some(format!("socks5h://{}", socks.addr)),
            }),
        );
        params.proxy = Some("http://127.0.0.1:9".to_string());

        let result = run(params, state).await;
        assert!(
            result.is_err(),
            "a rejected inline Tor preflight must be a tool Err, never Ok(\"{{\\\"pages\\\":[]}}\")"
        );
        assert_eq!(
            http.hit_count(),
            0,
            "the target must never be reached once the preflight is incompatible"
        );
        assert_eq!(socks.connect_count(), 0);
    }

    /// V6: a background Tor crawl (limit > INLINE_LIMIT) reaches the seed
    /// exclusively via SOCKS and completes successfully.
    #[cfg(feature = "transport_tor")]
    #[tokio::test]
    async fn tor_background_crawl_reaches_target_only_via_socks() {
        let http = crate::test_support::HttpFixture::start(
            "<html><body>tor background crawl</body></html>",
        );
        let socks = crate::test_support::SocksFixture::start(
            Some(http.addr),
            crate::test_support::SocksBehavior::Splice,
        );
        let url = format!("http://crawl-tor-bg-mcp-test.invalid:{}/", http.addr.port());
        let url_seed = url.clone();
        let state = Arc::new(SharedState::new());

        let output = run(
            tor_crawl_params(
                url,
                Some(INLINE_LIMIT + 1),
                Some(crate::transport::TransportParam {
                    mode: Some(crate::transport::TransportModeParam::Tor),
                    proxy: Some(format!("socks5h://{}", socks.addr)),
                }),
            ),
            state.clone(),
        )
        .await
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        let crawl_id = value["crawl_id"].as_str().unwrap().to_string();

        let status = await_terminal(&state, &crawl_id).await;
        assert_eq!(status, CrawlSessionStatus::Complete);
        {
            let session = state.sessions.get(&crawl_id).unwrap();
            assert_eq!(session.pages.len(), 1);
            assert_eq!(session.pages[0].url, url_seed);
            assert!(session.error.is_none());
        }
        // `crawl_raw` unconditionally probes `/sitemap.xml` after the
        // ordinary crawl pass (`ignore_sitemap` defaults to `false`; this
        // MCP tool does not set it, matching pre-Tor behavior byte for
        // byte) — a second, unrelated hit alongside the seed page. The
        // fixture's body isn't valid sitemap XML, so it yields zero extra
        // pages; both hits still went exclusively through SOCKS, which is
        // what this test exists to prove.
        assert_eq!(
            http.hit_paths(),
            vec!["/".to_string(), "/sitemap.xml".to_string()]
        );
        assert_eq!(socks.connect_count(), 2);
    }

    /// V15: a background crawl whose transport preflight is rejected
    /// (Tor combined with the legacy proxy list) surfaces as an explicit
    /// `Failed` status with `error`/`error_code` — never a silent, empty
    /// `Complete` (Section N: the exact defect this frontier's fix
    /// targets).
    #[tokio::test]
    async fn tor_background_crawl_preflight_rejection_is_failed_not_empty_complete() {
        let http = crate::test_support::HttpFixture::start("<html></html>");
        let socks = crate::test_support::SocksFixture::start(
            Some(http.addr),
            crate::test_support::SocksBehavior::Splice,
        );
        let url = format!(
            "http://crawl-tor-bg-reject-mcp-test.invalid:{}/",
            http.addr.port()
        );
        let state = Arc::new(SharedState::new());

        let mut params = tor_crawl_params(
            url,
            Some(INLINE_LIMIT + 1),
            Some(crate::transport::TransportParam {
                mode: Some(crate::transport::TransportModeParam::Tor),
                proxy: Some(format!("socks5h://{}", socks.addr)),
            }),
        );
        // Legacy proxy list + Tor is an audited-incompatible combination
        // (`Website::tor_crawl_preflight`) — a genuine preflight
        // rejection, not a synthetic test-only condition.
        params.proxy = Some("http://127.0.0.1:9".to_string());

        let output = run(params, state.clone()).await.unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        let crawl_id = value["crawl_id"].as_str().unwrap().to_string();

        let status = await_terminal(&state, &crawl_id).await;
        assert_eq!(status, CrawlSessionStatus::Failed);
        {
            let session = state.sessions.get(&crawl_id).unwrap();
            assert!(session.pages.is_empty());
            assert!(
                session.error.is_some(),
                "Failed session must carry an error message"
            );
            assert!(
                session.error_code.is_some(),
                "Failed session must carry a machine-readable error_code"
            );
        }
        assert_eq!(
            http.hit_count(),
            0,
            "a rejected preflight must never reach the target"
        );
        assert_eq!(socks.connect_count(), 0);
    }

    /// V11: an explicit Tor request combined with an active Spider Cloud
    /// configuration is rejected before any target networking.
    /// Constructed directly on `Website` (not via the `SPIDER_API_KEY` env
    /// var `apply_spider_cloud` reads) to avoid cross-test env-var
    /// pollution in this shared test binary — this exercises the same
    /// `Website::tor_crawl_preflight` check the tool's `run()` relies on.
    #[cfg(feature = "spider_cloud")]
    #[tokio::test]
    async fn tor_crawl_spider_cloud_conflict_rejected() {
        let http = crate::test_support::HttpFixture::start("<html></html>");
        let socks = crate::test_support::SocksFixture::start(
            Some(http.addr),
            crate::test_support::SocksBehavior::Splice,
        );
        let url = format!(
            "http://crawl-tor-cloud-conflict-mcp-test.invalid:{}/",
            http.addr.port()
        );

        let mut website = spider::website::Website::new(&url);
        website.configuration.spider_cloud = Some(Box::new(
            spider::configuration::SpiderCloudConfig::new("sk-test-not-a-real-key"),
        ));
        website.with_transport(spider::features::transport::TransportPolicy::Tor(
            spider::features::transport::TorTransportConfig::new(&format!(
                "socks5h://{}",
                socks.addr
            ))
            .unwrap(),
        ));
        website.crawl_raw().await;

        assert!(website.last_transport_error().is_some());
        assert_eq!(http.hit_count(), 0);
        assert_eq!(socks.connect_count(), 0);
    }

    /// Default background crawl (transport omitted) is unaffected.
    #[tokio::test]
    async fn default_background_crawl_unchanged() {
        let http = crate::test_support::HttpFixture::start("<html><body>default</body></html>");
        let url = format!("http://{}/", http.addr);
        let state = Arc::new(SharedState::new());

        let output = run(
            tor_crawl_params(url, Some(INLINE_LIMIT + 1), None),
            state.clone(),
        )
        .await
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        let crawl_id = value["crawl_id"].as_str().unwrap().to_string();

        let status = await_terminal(&state, &crawl_id).await;
        assert_eq!(status, CrawlSessionStatus::Complete);
        let session = state.sessions.get(&crawl_id).unwrap();
        assert_eq!(session.pages.len(), 1);
        assert!(session.error.is_none());
        assert!(session.error_code.is_none());
    }

    /// SCORPION_MCP_BACKGROUND_CRAWL_FAILURE_SEMANTICS_001, the exact
    /// operator-observed defect: a refused acquisition (a genuinely
    /// unreachable target, not a synthetic status code or string match)
    /// against a background crawl (limit > INLINE_LIMIT) previously
    /// reached `Complete` because the terminal-state gate counted the one
    /// synthetic diagnostic `Page` spider still emits for a connection
    /// failure, rather than counting only pages with a real observed HTTP
    /// response. Proves: zero observed responses -> `Failed`; the
    /// synthetic page remains retained as diagnostic evidence with
    /// `observed_status_code: null`/`response_origin: null`; the
    /// externally-reported `page_count` (`spider_crawl_status`, exercised
    /// by the sibling integration test) still truthfully counts it; no
    /// `error`/`error_code` is fabricated, since this ordinary connection
    /// refusal never populates a typed `TransportError` (matches the
    /// already-established null-fields contract for a non-transport
    /// `Failed` session).
    #[tokio::test]
    async fn background_crawl_refused_acquisition_reports_failed_with_zero_observed_responses() {
        let unused = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let refused_addr = unused.local_addr().unwrap();
        drop(unused);
        let url = format!("http://{refused_addr}/");
        let state = Arc::new(SharedState::new());

        let output = run(
            tor_crawl_params(url, Some(INLINE_LIMIT + 1), None),
            state.clone(),
        )
        .await
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        let crawl_id = value["crawl_id"].as_str().unwrap().to_string();
        assert_eq!(value["status"], "running");

        let status = await_terminal(&state, &crawl_id).await;
        assert_eq!(
            status,
            CrawlSessionStatus::Failed,
            "zero observed HTTP responses must never reach Complete"
        );
        let session = state.sessions.get(&crawl_id).unwrap();
        // page_count (spider_crawl_status's session.pages.len()) remains
        // truthful: the one synthetic diagnostic page is still retained,
        // it just no longer counts toward success.
        assert_eq!(session.pages.len(), 1);
        assert!(session.pages[0].provenance.observed_status_code.is_none());
        assert!(session.pages[0].provenance.response_origin.is_none());
        // No typed transport error exists for an ordinary connection
        // refusal — must not be fabricated.
        assert!(session.error.is_none());
        assert!(session.error_code.is_none());
    }

    // Mixed background crawl semantics (>=1 observed response alongside
    // >=1 synthetic zero-observed-response page must still reach
    // `Complete`) are intentionally not exercised by a dedicated live
    // two-hop test here: constructing a genuine same-crawl mix requires
    // the crawler to actually follow a cross-origin link to a refused
    // target within one run, and doing so reliably depends on spider's
    // own link-discovery/external-domain-following internals — out of
    // this MCP-layer repair's authorized surface to modify or debug
    // (`Website` is explicitly unchanged). The terminal-state decision
    // this frontier changed is a single boolean check,
    // `observed_response_count > 0`, evaluated once after the receive
    // loop fully drains; it is structurally independent of how many
    // additional pages (observed or synthetic) also arrived, since every
    // received page is unconditionally pushed into `session.pages`
    // regardless of the branch taken (unchanged by this repair). Both
    // sides of that boolean are already proven by real, live crawls:
    // `default_background_crawl_unchanged`/the Tor background success
    // test (observed_response_count > 0 -> Complete) and
    // `background_crawl_refused_acquisition_reports_failed_with_zero_observed_responses`
    // (observed_response_count == 0 -> Failed) above. A mix is the same
    // code path exercised with a larger page set; no additional branch
    // exists for it to hit.
}
