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
    /// status_code, content_type, content, links, screenshot, and integrity
    /// hashes) instead of the normal {url, status_code, content, links} shape.
    /// `response_body_hash` is available only for the non-browser HTTP path;
    /// browser/headless pages contain rendered DOM bytes instead. Default
    /// false — normal output is unaffected either way.
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
    used_browser: bool,
) -> crate::evidence::EvidenceBundle {
    // Only the non-browser path retains HTTP content-decoded response bytes.
    // Browser `Page` bytes are rendered DOM and must not be mislabeled.
    let response_body_hash = (!used_browser)
        .then(|| page.get_bytes().map(crate::evidence::sha256_hex))
        .flatten();
    let screenshot_bytes = super::page_screenshot_bytes(page);
    let transformed_content_hash =
        (!wants_screenshot).then(|| crate::evidence::sha256_hex(content.as_bytes()));
    let screenshot_hash = wants_screenshot
        .then_some(screenshot_bytes)
        .flatten()
        .map(crate::evidence::sha256_hex);
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
        response_body_hash,
        transformed_content_hash,
        content: if wants_screenshot {
            None
        } else {
            Some(content.clone())
        },
        links,
        source: None,
        provider: None,
        query: None,
        screenshot: if wants_screenshot {
            Some(content)
        } else {
            None
        },
        screenshot_hash,
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
    let used_browser = cfg!(feature = "chrome") && use_headless;

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
            let evidence = build_evidence(&page, content, wants_screenshot, used_browser);
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

#[cfg(all(test, feature = "chrome"))]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use spider::client::StatusCode;
    use spider::utils::PageResponse;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn page(
        body: Option<&[u8]>,
        screenshot: Option<&[u8]>,
        signature: Option<u64>,
    ) -> spider::page::Page {
        spider::page::build(
            "http://127.0.0.1/evidence",
            PageResponse {
                content: body.map(<[u8]>::to_vec),
                screenshot_bytes: screenshot.map(<[u8]>::to_vec),
                status_code: StatusCode::OK,
                signature,
                ..Default::default()
            },
        )
    }

    #[test]
    fn evidence_hashes_exact_page_and_transformed_bytes() {
        let body = b"<p>\xc3\xa5</p>\r\n";
        let transformed = "# Exact\n\ntext  \n";
        let evidence = build_evidence(
            &page(Some(body), None, Some(u64::MAX)),
            transformed.into(),
            false,
            false,
        );

        assert_eq!(
            evidence.response_body_hash,
            Some(format!("{:x}", Sha256::digest(body)))
        );
        assert_eq!(
            evidence.transformed_content_hash,
            Some(format!("{:x}", Sha256::digest(transformed.as_bytes())))
        );
        assert_eq!(evidence.content.as_deref(), Some(transformed));
        assert!(evidence.screenshot.is_none());
        assert!(evidence.screenshot_hash.is_none());
    }

    #[test]
    fn screenshot_hash_uses_original_png_bytes_not_base64_text() {
        let png = b"\x89PNG\r\n\x1a\nexact-png-bytes";
        let base64_output = "iVBORw0KGgpleGFjdC1wbmctYnl0ZXM=";
        let evidence = build_evidence(
            &page(Some(b"<html></html>"), Some(png), Some(1)),
            base64_output.into(),
            true,
            true,
        );

        let original_hash = format!("{:x}", Sha256::digest(png));
        let base64_hash = format!("{:x}", Sha256::digest(base64_output.as_bytes()));
        assert_eq!(
            evidence.screenshot_hash.as_deref(),
            Some(original_hash.as_str())
        );
        assert_ne!(
            evidence.screenshot_hash.as_deref(),
            Some(base64_hash.as_str())
        );
        assert_eq!(evidence.screenshot.as_deref(), Some(base64_output));
        assert!(evidence.content.is_none());
        assert!(evidence.transformed_content_hash.is_none());
    }

    #[test]
    fn absent_screenshot_has_null_hash() {
        let evidence = build_evidence(&page(Some(b"body"), None, None), "".into(), true, true);
        assert!(evidence.screenshot_hash.is_none());
        assert!(serde_json::to_value(evidence).unwrap()["screenshot_hash"].is_null());
    }

    #[test]
    fn present_empty_http_body_hashes_as_empty_sha256() {
        let evidence = build_evidence(&page(Some(b""), None, None), "text".into(), false, false);
        assert_eq!(
            evidence.response_body_hash.as_deref(),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
    }

    #[test]
    fn absent_http_body_has_null_hash() {
        let evidence = build_evidence(&page(None, None, None), "text".into(), false, false);
        assert!(evidence.response_body_hash.is_none());
    }

    #[test]
    fn browser_dom_never_populates_response_body_hash() {
        let evidence = build_evidence(
            &page(Some(b"<html>rendered DOM</html>"), None, None),
            "rendered".into(),
            false,
            true,
        );
        assert!(evidence.response_body_hash.is_none());
        assert!(evidence.transformed_content_hash.is_some());
    }

    #[test]
    fn present_empty_transformed_content_and_screenshot_are_hashed() {
        let text = build_evidence(&page(Some(b"body"), None, None), "".into(), false, false);
        assert_eq!(
            text.transformed_content_hash.as_deref(),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );

        let screenshot =
            build_evidence(&page(Some(b"dom"), Some(b""), None), "".into(), true, true);
        assert_eq!(
            screenshot.screenshot_hash.as_deref(),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
        assert!(screenshot.transformed_content_hash.is_none());
        assert!(screenshot.response_body_hash.is_none());
    }

    fn localhost_server(
        body: &'static [u8],
    ) -> (String, Arc<AtomicBool>, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut request = [0_u8; 2048];
                        let _ = stream.read(&mut request);
                        let headers = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        stream.write_all(headers.as_bytes()).unwrap();
                        stream.write_all(body).unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(error) => panic!("localhost server failed: {error}"),
                }
            }
        });
        (address, stop, handle)
    }

    fn decode_base64_independently(input: &str) -> Vec<u8> {
        fn value(byte: u8) -> Option<u8> {
            match byte {
                b'A'..=b'Z' => Some(byte - b'A'),
                b'a'..=b'z' => Some(byte - b'a' + 26),
                b'0'..=b'9' => Some(byte - b'0' + 52),
                b'+' | b'-' => Some(62),
                b'/' | b'_' => Some(63),
                _ => None,
            }
        }

        let mut output = Vec::with_capacity(input.len() / 4 * 3);
        let mut accumulator = 0_u32;
        let mut bits = 0_u8;
        for byte in input.bytes() {
            if byte == b'=' {
                break;
            }
            let Some(value) = value(byte) else {
                continue;
            };
            accumulator = (accumulator << 6) | u32::from(value);
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                output.push((accumulator >> bits) as u8);
                accumulator &= (1_u32 << bits) - 1;
            }
        }
        output
    }

    #[tokio::test]
    #[ignore = "localhost socket acceptance; run explicitly where loopback bind is permitted"]
    async fn localhost_scrape_acceptance_and_legacy_shape() {
        static BODY: &[u8] = b"<!doctype html><html><body>known evidence body</body></html>";
        let (url, stop, handle) = localhost_server(BODY);

        let evidence_json = run(ScrapeParams {
            url: url.clone(),
            return_format: Some("raw".into()),
            headless: Some(false),
            wait_for: None,
            wait_for_delay_ms: None,
            wait_for_idle_network: None,
            user_agent: None,
            cookie: None,
            proxy: None,
            evidence: Some(true),
        })
        .await
        .unwrap();
        let evidence: serde_json::Value = serde_json::from_str(&evidence_json).unwrap();
        assert_eq!(
            evidence["response_body_hash"],
            format!("{:x}", Sha256::digest(BODY))
        );
        assert_eq!(evidence["content_type"], "text/html; charset=utf-8");
        let returned = evidence["content"].as_str().unwrap();
        assert_eq!(
            evidence["transformed_content_hash"],
            format!("{:x}", Sha256::digest(returned.as_bytes()))
        );

        let legacy_json = run(ScrapeParams {
            url,
            return_format: Some("raw".into()),
            headless: Some(false),
            wait_for: None,
            wait_for_delay_ms: None,
            wait_for_idle_network: None,
            user_agent: None,
            cookie: None,
            proxy: None,
            evidence: Some(false),
        })
        .await
        .unwrap();
        let legacy: serde_json::Value = serde_json::from_str(&legacy_json).unwrap();
        let mut keys = legacy
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        assert_eq!(keys, ["content", "links", "status_code", "url"]);

        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires a real local Chromium process and loopback socket"]
    async fn localhost_chromium_screenshot_hash_acceptance() {
        static BODY: &[u8] = b"<!doctype html><html><body><h1>Chromium evidence</h1></body></html>";
        let (url, stop, handle) = localhost_server(BODY);

        let result = run(ScrapeParams {
            url,
            return_format: Some("screenshot".into()),
            headless: None,
            wait_for: None,
            wait_for_delay_ms: None,
            wait_for_idle_network: None,
            user_agent: None,
            cookie: None,
            proxy: None,
            evidence: Some(true),
        })
        .await
        .unwrap();
        let evidence: serde_json::Value = serde_json::from_str(&result).unwrap();
        let png = decode_base64_independently(evidence["screenshot"].as_str().unwrap());
        assert!(png.len() > 8);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(
            evidence["screenshot_hash"],
            format!("{:x}", Sha256::digest(&png))
        );
        assert!(evidence["response_body_hash"].is_null());
        assert!(evidence["transformed_content_hash"].is_null());

        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
    }
}
