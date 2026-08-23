use rmcp::schemars;
use serde::Deserialize;
use serde_json::json;
use spider::tokio;
use spider::website::Website;

#[derive(Deserialize, schemars::JsonSchema)]
pub struct LinksParams {
    /// The URL to extract links from
    pub url: String,
    /// Use Chrome for JavaScript rendering
    pub headless: Option<bool>,
    /// Include subdomain links (default: false)
    pub subdomains: Option<bool>,
    /// Transport for this acquisition. Omit for Default (normal
    /// networking).
    pub transport: Option<crate::transport::TransportParam>,
}

pub async fn run(params: LinksParams) -> Result<String, String> {
    let url = if params.url.starts_with("http") {
        params.url.clone()
    } else {
        format!("https://{}", params.url)
    };

    let mut website = Website::new(&url);
    super::apply_spider_cloud(&mut website);

    let transport_policy = crate::transport::resolve(params.transport)?;
    let tor_requested = matches!(
        transport_policy,
        spider::features::transport::TransportPolicy::Tor(_)
    );
    let use_headless = params.headless.unwrap_or(false);
    // Fail closed before any target networking (Section I/L): Tor
    // crawling is HTTP-only, never browser/headless.
    if tor_requested && use_headless {
        return Err(
            "transport.mode=\"tor\" cannot be combined with headless=true — Tor crawling is \
             HTTP-only"
                .to_string(),
        );
    }
    if tor_requested {
        website.with_transport(transport_policy);
    }

    website
        .with_subdomains(params.subdomains.unwrap_or(false))
        .with_limit(1);

    website.configuration.return_page_links = true;

    let mut website = website.build().map_err(|_| "Invalid URL".to_string())?;

    let mut rx = website.subscribe(0);

    // Captured so a Tor preflight rejection surfaces as a specific error
    // rather than the generic "No response" message below (Section H/N).
    let crawl_task = tokio::spawn(async move {
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
    });

    if let Ok(page) = rx.recv().await {
        let response_observed = page.observed_status_code.is_some();
        let links: Vec<String> = page
            .page_links
            .as_ref()
            .map(|s| s.iter().map(|l| l.inner().to_string()).collect())
            .unwrap_or_default();
        let count = links.len();

        let diagnostic = serde_json::to_string_pretty(&json!({
            "url": page.get_url(),
            "links": links,
            "count": count,
        }))
        .map_err(|e| e.to_string())?;
        if response_observed {
            Ok(diagnostic)
        } else {
            Err(diagnostic)
        }
    } else if let Ok(Some(transport_error)) = crawl_task.await {
        Err(transport_error.to_string())
    } else {
        Err(format!("No response for {}", params.url))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// V3: a Tor `spider_links` acquisition reaches the target
    /// exclusively via SOCKS.
    #[cfg(feature = "transport_tor")]
    #[tokio::test]
    async fn tor_links_reaches_target_only_via_socks() {
        let http = crate::test_support::HttpFixture::start(
            r#"<html><body><a href="/a">a</a><a href="/b">b</a></body></html>"#,
        );
        let socks = crate::test_support::SocksFixture::start(
            Some(http.addr),
            crate::test_support::SocksBehavior::Splice,
        );
        let url = format!("http://links-tor-mcp-test.invalid:{}/", http.addr.port());

        let output = run(LinksParams {
            url,
            headless: Some(false),
            subdomains: None,
            transport: Some(crate::transport::TransportParam {
                mode: Some(crate::transport::TransportModeParam::Tor),
                proxy: Some(format!("socks5h://{}", socks.addr)),
            }),
        })
        .await
        .unwrap();

        assert_eq!(http.hit_count(), 1);
        assert_eq!(socks.connect_count(), 1);
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["count"], 2);
    }

    /// V10 (links half): `transport.mode="tor"` combined with
    /// `headless=true` is rejected before any browser launch or target
    /// networking.
    #[tokio::test]
    async fn tor_links_headless_rejected_before_launch() {
        let http = crate::test_support::HttpFixture::start("<html></html>");
        let socks = crate::test_support::SocksFixture::start(
            Some(http.addr),
            crate::test_support::SocksBehavior::Splice,
        );
        let url = format!("http://{}/", http.addr);

        let result = run(LinksParams {
            url,
            headless: Some(true),
            subdomains: None,
            transport: Some(crate::transport::TransportParam {
                mode: Some(crate::transport::TransportModeParam::Tor),
                proxy: Some(format!("socks5h://{}", socks.addr)),
            }),
        })
        .await;

        assert!(result.is_err());
        assert_eq!(http.hit_count(), 0);
        assert_eq!(socks.connect_count(), 0);
    }

    /// Default `spider_links` (transport omitted) is unaffected.
    #[tokio::test]
    async fn default_links_unchanged() {
        let http = crate::test_support::HttpFixture::start(
            r#"<html><body><a href="/x">x</a></body></html>"#,
        );
        let url = format!("http://{}/", http.addr);

        let output = run(LinksParams {
            url,
            headless: Some(false),
            subdomains: None,
            transport: None,
        })
        .await
        .unwrap();

        assert_eq!(http.hit_count(), 1);
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["count"], 1);
    }

    #[tokio::test]
    async fn observed_page_with_zero_links_remains_successful() {
        let http = crate::test_support::HttpFixture::start("<html><body>No links</body></html>");
        let url = format!("http://{}/", http.addr);

        let output = run(LinksParams {
            url,
            headless: Some(false),
            subdomains: None,
            transport: None,
        })
        .await
        .unwrap();

        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["count"], 0);
        assert_eq!(value["links"], json!([]));
    }

    #[tokio::test]
    async fn external_domain_filtering_remains_unchanged() {
        let http = crate::test_support::HttpFixture::start(
            r#"<html><body><a href="/same">same</a><a href="https://example.invalid/external">external</a></body></html>"#,
        );
        let url = format!("http://{}/", http.addr);

        let output = run(LinksParams {
            url: url.clone(),
            headless: Some(false),
            subdomains: None,
            transport: None,
        })
        .await
        .unwrap();

        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["count"], 1);
        assert_eq!(value["links"], json!([format!("{url}same")]));
    }
}
