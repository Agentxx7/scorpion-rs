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
    /// Output format: raw, markdown, text, xml, screenshot, or auto (default: markdown).
    /// Auto routes confidently identified HTML to Markdown, preserves JSON/XML
    /// as text, and rejects binary formats for which text extraction is unsupported.
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
    /// `response_body_hash` and byte-derived `detected_content_type` are
    /// available only for the non-browser HTTP path; browser/headless pages
    /// contain rendered DOM bytes instead. Default false — normal output is
    /// unaffected either way.
    pub evidence: Option<bool>,
}

/// Build the evidence-mode result for one fetched page. `content` is absent
/// when retrieval succeeded but auto extraction did not. Content and
/// screenshot remain mutually exclusive; hashes exist only for the actual
/// representation returned in the corresponding field.
fn build_evidence(
    page: &spider::page::Page,
    content: Option<String>,
    wants_screenshot: bool,
    used_browser: bool,
) -> crate::evidence::EvidenceBundle {
    // Only the non-browser path retains HTTP content-decoded response bytes.
    // Browser `Page` bytes are rendered DOM and must not be mislabeled.
    let response_body_hash = (!used_browser)
        .then(|| page.get_bytes().map(crate::evidence::sha256_hex))
        .flatten();
    let detected_content_type = if used_browser {
        None
    } else {
        page.get_bytes()
            .and_then(infer::get)
            .map(|kind| kind.mime_type().to_string())
    };
    let screenshot_bytes = super::page_screenshot_bytes(page);
    let transformed_content_hash = if wants_screenshot {
        None
    } else {
        content
            .as_deref()
            .map(|text| crate::evidence::sha256_hex(text.as_bytes()))
    };
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
        retrieved_at: page.get_retrieved_at(),
        status_code: Some(page.status_code.as_u16()),
        content_type,
        detected_content_type,
        response_body_hash,
        transformed_content_hash,
        content: (!wants_screenshot).then_some(content.clone()).flatten(),
        links,
        source: None,
        provider: None,
        query: None,
        screenshot: wants_screenshot.then_some(content).flatten(),
        screenshot_hash,
        metadata: None,
    }
}

#[derive(Debug, PartialEq)]
enum AutoRoute {
    Markdown,
    Json(String),
    Xml(String),
    Text(String),
}

fn declared_mime(content_type: Option<&str>) -> Option<String> {
    content_type
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

fn auto_error(
    content_type: Option<&str>,
    detected_content_type: Option<&str>,
    reason: &str,
) -> String {
    serde_json::to_string_pretty(&json!({
        "error": "auto_extraction_unsupported",
        "message": format!("Extraction unsupported in auto mode: {reason}"),
        "content_type": content_type,
        "detected_content_type": detected_content_type,
    }))
    .expect("auto error JSON contains only serializable values")
}

/// Decide how `return_format="auto"` handles the original HTTP bytes.
/// Byte-signature MIME is authoritative when available; the declared header
/// remains an independent fallback signal and is never rewritten.
fn route_auto_http(bytes: &[u8], content_type: Option<&str>) -> Result<AutoRoute, String> {
    let detected = infer::get(bytes).map(|kind| kind.mime_type());
    let declared = declared_mime(content_type);

    if let Some(mime) = detected {
        return match mime {
            "text/html" => Ok(AutoRoute::Markdown),
            "text/xml" | "application/xml" => std::str::from_utf8(bytes)
                .map(|text| AutoRoute::Xml(text.to_owned()))
                .map_err(|_| auto_error(content_type, detected, "detected XML is not valid UTF-8")),
            "application/pdf" => Err(auto_error(
                content_type,
                detected,
                "PDF text extraction is not available",
            )),
            mime if mime.starts_with("image/") => Err(auto_error(
                content_type,
                detected,
                "image text extraction is not available",
            )),
            mime if mime.starts_with("video/") || mime.starts_with("audio/") => Err(auto_error(
                content_type,
                detected,
                "audio/video text extraction is not available",
            )),
            _ => Err(auto_error(
                content_type,
                detected,
                "the detected binary format has no auto extractor",
            )),
        };
    }

    let parsed_json = serde_json::from_slice::<serde_json::Value>(bytes).ok();
    let declared_json = declared
        .as_deref()
        .is_some_and(|mime| mime == "application/json" || mime.ends_with("+json"));
    if declared_json || parsed_json.is_some() {
        let value = parsed_json.ok_or_else(|| {
            auto_error(
                content_type,
                None,
                "the declared JSON response could not be parsed",
            )
        })?;
        return serde_json::to_string_pretty(&value)
            .map(AutoRoute::Json)
            .map_err(|error| {
                auto_error(
                    content_type,
                    None,
                    &format!("JSON serialization failed: {error}"),
                )
            });
    }

    let text = std::str::from_utf8(bytes).map_err(|_| {
        auto_error(
            content_type,
            None,
            "content type is undetermined and the bytes are not valid UTF-8",
        )
    })?;

    match declared.as_deref() {
        Some("text/html") | Some("application/xhtml+xml") => Ok(AutoRoute::Markdown),
        Some("text/xml") | Some("application/xml") => Ok(AutoRoute::Xml(text.to_owned())),
        Some(mime) if mime.ends_with("+xml") => Ok(AutoRoute::Xml(text.to_owned())),
        Some(mime) if mime.starts_with("text/") => Ok(AutoRoute::Text(text.to_owned())),
        Some(mime)
            if mime.starts_with("image/")
                || mime.starts_with("video/")
                || mime.starts_with("audio/")
                || mime == "application/pdf"
                || mime == "application/octet-stream" =>
        {
            Err(auto_error(
                content_type,
                None,
                "the declared binary format has no auto text extractor",
            ))
        }
        _ if text
            .chars()
            .all(|ch| !ch.is_control() || ch.is_ascii_whitespace()) =>
        {
            Ok(AutoRoute::Text(text.to_owned()))
        }
        _ => Err(auto_error(
            content_type,
            None,
            "content type is undetermined and the bytes are not safely textual",
        )),
    }
}

fn page_content_type(page: &spider::page::Page) -> Option<&str> {
    page.headers
        .as_ref()
        .and_then(|headers| headers.get("content-type"))
        .and_then(|value| value.to_str().ok())
}

fn transform_page(page: &spider::page::Page, return_format: ReturnFormat) -> String {
    transform_content_input(
        TransformInput {
            url: page.get_url_parsed_ref().as_ref(),
            content: page.get_html_bytes_u8(),
            screenshot_bytes: super::page_screenshot_bytes(page),
            encoding: None,
            selector_config: None,
            ignore_tags: None,
        },
        &TransformConfig {
            return_format,
            ..Default::default()
        },
    )
}

fn auto_content(page: &spider::page::Page, used_browser: bool) -> Result<String, String> {
    // The browser path intentionally operates on Chromium's rendered DOM. It
    // does not claim to identify or retain the original resource MIME/bytes.
    if used_browser {
        return Ok(transform_page(page, ReturnFormat::Markdown));
    }

    let bytes = page.get_bytes().ok_or_else(|| {
        auto_error(
            page_content_type(page),
            None,
            "original HTTP response bytes are unavailable",
        )
    })?;
    match route_auto_http(bytes, page_content_type(page))? {
        AutoRoute::Markdown => Ok(transform_page(page, ReturnFormat::Markdown)),
        AutoRoute::Json(text) | AutoRoute::Xml(text) | AutoRoute::Text(text) => Ok(text),
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
    let wants_auto = format_str == "auto";
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

    let mut results = Vec::new();

    while let Ok(page) = rx.recv().await {
        let content_result = if wants_auto {
            auto_content(&page, used_browser)
        } else {
            Ok(transform_page(&page, ReturnFormat::from_str(format_str)))
        };

        let result = if params.evidence.unwrap_or(false) {
            // Retrieval evidence survives an auto extraction failure. With no
            // extraction-error field in EvidenceBundle, `content = None` and
            // `transformed_content_hash = None` state the outcome honestly.
            let content = match content_result {
                Ok(content) => Some(super::screenshot_content_or_error(
                    wants_screenshot,
                    content,
                )?),
                Err(_) if wants_auto => None,
                Err(error) => return Err(error),
            };
            let evidence = build_evidence(&page, content, wants_screenshot, used_browser);
            serde_json::to_value(&evidence).map_err(|e| e.to_string())?
        } else {
            let content = super::screenshot_content_or_error(wants_screenshot, content_result?)?;
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

#[cfg(test)]
mod auto_router_tests {
    use super::*;

    fn error_value(result: Result<AutoRoute, String>) -> serde_json::Value {
        serde_json::from_str(&result.unwrap_err()).unwrap()
    }

    #[test]
    fn auto_routes_confident_html_to_markdown_path() {
        assert_eq!(
            route_auto_http(b"<!DOCTYPE html><html><body>Hello</body></html>", None).unwrap(),
            AutoRoute::Markdown
        );
    }

    #[test]
    fn auto_formats_json_without_html_transformation() {
        assert_eq!(
            route_auto_http(br#"{"b":2,"a":1}"#, Some("application/json; charset=utf-8")).unwrap(),
            AutoRoute::Json("{\n  \"a\": 1,\n  \"b\": 2\n}".into())
        );
        assert!(matches!(
            route_auto_http(br#"[1,true]"#, None).unwrap(),
            AutoRoute::Json(_)
        ));
    }

    #[test]
    fn declared_invalid_json_is_an_extraction_error() {
        let error = error_value(route_auto_http(
            b"this is not JSON",
            Some("application/json"),
        ));
        assert_eq!(error["error"], "auto_extraction_unsupported");
        assert_eq!(error["content_type"], "application/json");
        assert!(error["message"]
            .as_str()
            .unwrap()
            .contains("could not be parsed"));
    }

    #[test]
    fn auto_preserves_xml_instead_of_using_html_to_xml() {
        let xml = "<?xml version=\"1.0\"?><root><item>value</item></root>";
        assert_eq!(
            route_auto_http(xml.as_bytes(), None).unwrap(),
            AutoRoute::Xml(xml.into())
        );
    }

    #[test]
    fn auto_rejects_pdf_png_mp4_and_unknown_binary() {
        let cases: &[(&[u8], Option<&str>, Option<&str>)] = &[
            (
                b"%PDF-1.7\nbody",
                Some("text/html"),
                Some("application/pdf"),
            ),
            (b"\x89PNG\r\n\x1a\nbytes", None, Some("image/png")),
            (
                b"\x00\x00\x00\x18ftypmp42\x00\x00\x00\x00mp42isom",
                None,
                Some("video/mp4"),
            ),
            (b"\xff\x00\x81\x02unknown", None, None),
        ];

        for (bytes, declared, detected) in cases {
            let error = error_value(route_auto_http(bytes, *declared));
            assert_eq!(error["error"], "auto_extraction_unsupported");
            assert!(error["message"]
                .as_str()
                .unwrap()
                .contains("unsupported in auto mode"));
            assert_eq!(error["content_type"].as_str(), *declared);
            assert_eq!(error["detected_content_type"].as_str(), *detected);
        }
    }

    #[test]
    fn auto_rejects_infer_known_zip_through_binary_catchall() {
        let zip = b"PK\x03\x04\x14\x00\x00\x00\x00\x00known-archive";
        assert_eq!(
            infer::get(zip).map(|kind| kind.mime_type()),
            Some("application/zip")
        );
        let error = error_value(route_auto_http(zip, Some("text/html")));
        assert_eq!(error["error"], "auto_extraction_unsupported");
        assert_eq!(error["content_type"], "text/html");
        assert_eq!(error["detected_content_type"], "application/zip");
        assert!(error["message"]
            .as_str()
            .unwrap()
            .contains("detected binary format"));
    }

    #[test]
    fn byte_signature_takes_precedence_without_mutating_declared_signal() {
        let error = error_value(route_auto_http(
            b"%PDF-1.7\nbody",
            Some("application/json; charset=utf-8"),
        ));
        assert_eq!(error["content_type"], "application/json; charset=utf-8");
        assert_eq!(error["detected_content_type"], "application/pdf");
    }

    #[test]
    fn conservative_unknown_utf8_is_text_but_controls_are_undetermined() {
        assert_eq!(
            route_auto_http("plain å text".as_bytes(), None).unwrap(),
            AutoRoute::Text("plain å text".into())
        );
        let error = error_value(route_auto_http(b"valid utf8\0but binary", None));
        assert!(error["message"].as_str().unwrap().contains("undetermined"));
    }

    #[test]
    fn historical_return_formats_and_unknown_fallback_are_unchanged() {
        assert_eq!(ReturnFormat::from_str("raw"), ReturnFormat::Raw);
        assert_eq!(ReturnFormat::from_str("markdown"), ReturnFormat::Markdown);
        assert_eq!(ReturnFormat::from_str("text"), ReturnFormat::Text);
        assert_eq!(ReturnFormat::from_str("xml"), ReturnFormat::XML);
        assert_eq!(
            ReturnFormat::from_str("screenshot"),
            ReturnFormat::Screenshot
        );
        assert_eq!(
            ReturnFormat::from_str("historical-unknown"),
            ReturnFormat::Raw
        );
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

    fn page_with_content_type(body: &[u8], content_type: Option<&str>) -> spider::page::Page {
        let headers = content_type.map(|value| {
            let mut headers = spider::client::header::HeaderMap::new();
            headers.insert(
                spider::client::header::CONTENT_TYPE,
                spider::client::header::HeaderValue::from_bytes(value.as_bytes()).unwrap(),
            );
            headers
        });
        spider::page::build(
            "http://127.0.0.1/evidence",
            PageResponse {
                content: Some(body.to_vec()),
                headers,
                status_code: StatusCode::OK,
                ..Default::default()
            },
        )
    }

    fn detected_type(body: &[u8]) -> Option<String> {
        build_evidence(
            &page_with_content_type(body, None),
            Some("unchanged transformed content".into()),
            false,
            false,
        )
        .detected_content_type
    }

    #[test]
    fn auto_html_uses_existing_markdown_transformer() {
        let page = page_with_content_type(
            b"<!doctype html><html><body><h1>Auto title</h1><p>Body</p></body></html>",
            Some("text/html; charset=utf-8"),
        );
        let content = auto_content(&page, false).unwrap();
        assert!(content.contains("# Auto title"));
        assert!(content.contains("Body"));
        assert!(!content.contains("<h1>"));
    }

    #[test]
    fn auto_json_and_xml_bypass_html_transformer() {
        let json = page_with_content_type(br#"{"z":2,"a":1}"#, Some("application/json"));
        assert_eq!(
            auto_content(&json, false).unwrap(),
            "{\n  \"a\": 1,\n  \"z\": 2\n}"
        );

        let xml_text = "<?xml version=\"1.0\"?><root><item>unchanged</item></root>";
        let xml = page_with_content_type(xml_text.as_bytes(), Some("application/xml"));
        assert_eq!(auto_content(&xml, false).unwrap(), xml_text);
    }

    #[test]
    fn browser_auto_is_bounded_to_rendered_dom_markdown() {
        let page = page_with_content_type(
            b"<!doctype html><html><body><h1>Rendered DOM</h1></body></html>",
            Some("application/pdf"),
        );
        let content = auto_content(&page, true).unwrap();
        assert!(content.contains("# Rendered DOM"));

        let evidence = build_evidence(&page, Some(content), false, true);
        assert!(evidence.detected_content_type.is_none());
        assert!(evidence.response_body_hash.is_none());
        assert_eq!(evidence.content_type.as_deref(), Some("application/pdf"));
        assert!(evidence.transformed_content_hash.is_some());
    }

    #[test]
    fn unsupported_auto_returns_structured_extraction_error() {
        let page = page_with_content_type(b"%PDF-1.7\nbody", Some("application/pdf"));
        let error: serde_json::Value =
            serde_json::from_str(&auto_content(&page, false).unwrap_err()).unwrap();
        assert_eq!(error["content_type"], "application/pdf");
        assert_eq!(error["detected_content_type"], "application/pdf");
        assert_eq!(error["error"], "auto_extraction_unsupported");
    }

    #[test]
    fn detects_known_byte_signatures_and_leaves_unknown_null() {
        assert_eq!(
            detected_type(b"\x89PNG\r\n\x1a\nbytes").as_deref(),
            Some("image/png")
        );
        assert_eq!(
            detected_type(b"%PDF-1.7\nbody").as_deref(),
            Some("application/pdf")
        );
        assert_eq!(
            detected_type(b"<!DOCTYPE HTML><html><body>x</body></html>").as_deref(),
            Some("text/html")
        );
        assert_eq!(
            detected_type(b"<?xml version=\"1.0\"?><root/>").as_deref(),
            Some("text/xml")
        );
        assert_eq!(
            detected_type(b"scorpion bytes with no known signature"),
            None
        );
    }

    #[test]
    fn declared_and_detected_content_types_remain_independent() {
        let agreeing = build_evidence(
            &page_with_content_type(b"\x89PNG\r\n\x1a\nbytes", Some("image/png")),
            Some("text".into()),
            false,
            false,
        );
        assert_eq!(agreeing.content_type.as_deref(), Some("image/png"));
        assert_eq!(agreeing.detected_content_type.as_deref(), Some("image/png"));

        let disagreeing = build_evidence(
            &page_with_content_type(b"%PDF-1.7\nbody", Some("text/html")),
            Some("text".into()),
            false,
            false,
        );
        assert_eq!(disagreeing.content_type.as_deref(), Some("text/html"));
        assert_eq!(
            disagreeing.detected_content_type.as_deref(),
            Some("application/pdf")
        );

        let missing = build_evidence(
            &page_with_content_type(b"\x89PNG\r\n\x1a\nbytes", None),
            Some("text".into()),
            false,
            false,
        );
        assert_eq!(missing.content_type, None);
        assert_eq!(missing.detected_content_type.as_deref(), Some("image/png"));

        for body in [
            b"\x89PNG\r\n\x1a\nbytes".as_slice(),
            b"%PDF-1.7\nbody".as_slice(),
        ] {
            let evidence = build_evidence(
                &page_with_content_type(body, Some("application/octet-stream")),
                Some("text".into()),
                false,
                false,
            );
            assert_eq!(
                evidence.content_type.as_deref(),
                Some("application/octet-stream")
            );
            assert!(matches!(
                evidence.detected_content_type.as_deref(),
                Some("image/png" | "application/pdf")
            ));
        }
    }

    #[test]
    fn evidence_hashes_exact_page_and_transformed_bytes() {
        let body = b"<p>\xc3\xa5</p>\r\n";
        let transformed = "# Exact\n\ntext  \n";
        let evidence = build_evidence(
            &page(Some(body), None, Some(u64::MAX)),
            Some(transformed.into()),
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
            Some(base64_output.into()),
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
        assert!(evidence.detected_content_type.is_none());
        assert!(evidence.transformed_content_hash.is_none());
    }

    #[test]
    fn absent_screenshot_has_null_hash() {
        let evidence = build_evidence(
            &page(Some(b"body"), None, None),
            Some("".into()),
            true,
            true,
        );
        assert!(evidence.screenshot_hash.is_none());
        assert!(serde_json::to_value(evidence).unwrap()["screenshot_hash"].is_null());
    }

    #[test]
    fn present_empty_http_body_hashes_as_empty_sha256() {
        let evidence = build_evidence(
            &page(Some(b""), None, None),
            Some("text".into()),
            false,
            false,
        );
        assert_eq!(
            evidence.response_body_hash.as_deref(),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
    }

    #[test]
    fn absent_http_body_has_null_hash() {
        let evidence = build_evidence(&page(None, None, None), Some("text".into()), false, false);
        assert!(evidence.response_body_hash.is_none());
        assert!(evidence.detected_content_type.is_none());
    }

    #[test]
    fn browser_dom_never_populates_response_body_hash() {
        let evidence = build_evidence(
            &page(Some(b"<html>rendered DOM</html>"), None, None),
            Some("rendered".into()),
            false,
            true,
        );
        assert!(evidence.response_body_hash.is_none());
        assert!(evidence.detected_content_type.is_none());
        assert!(evidence.transformed_content_hash.is_some());
    }

    #[test]
    fn present_empty_transformed_content_and_screenshot_are_hashed() {
        let text = build_evidence(
            &page(Some(b"body"), None, None),
            Some("".into()),
            false,
            false,
        );
        assert_eq!(
            text.transformed_content_hash.as_deref(),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );

        let screenshot = build_evidence(
            &page(Some(b"dom"), Some(b""), None),
            Some("".into()),
            true,
            true,
        );
        assert_eq!(
            screenshot.screenshot_hash.as_deref(),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
        assert!(screenshot.transformed_content_hash.is_none());
        assert!(screenshot.response_body_hash.is_none());
    }

    fn localhost_server(
        body: &'static [u8],
        content_type: &'static str,
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
                            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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

    fn local_unix_epoch_millis() -> u64 {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test wall clock must be after Unix epoch")
            .as_millis();
        u64::try_from(millis).expect("test wall clock milliseconds must fit u64")
    }

    #[tokio::test]
    #[ignore = "localhost socket acceptance; run explicitly where loopback bind is permitted"]
    async fn localhost_scrape_acceptance_and_legacy_shape() {
        static BODY: &[u8] = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n%%EOF\n";
        let (url, stop, handle) = localhost_server(BODY, "text/html; charset=utf-8");

        let before = local_unix_epoch_millis();
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
        let after = local_unix_epoch_millis();
        let evidence: serde_json::Value = serde_json::from_str(&evidence_json).unwrap();
        let retrieved_at = evidence["retrieved_at"].as_u64().unwrap();
        assert!(before <= retrieved_at && retrieved_at <= after);
        assert_eq!(
            evidence["response_body_hash"],
            format!("{:x}", Sha256::digest(BODY))
        );
        assert_eq!(evidence["content_type"], "text/html; charset=utf-8");
        assert_eq!(evidence["detected_content_type"], "application/pdf");
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

    #[tokio::test]
    #[ignore = "localhost socket acceptance; run explicitly where loopback bind is permitted"]
    async fn localhost_auto_html_acceptance() {
        static BODY: &[u8] =
            b"<!doctype html><html><body><h1>Auto localhost</h1><p>Safe route</p></body></html>";
        let (url, stop, handle) = localhost_server(BODY, "text/html; charset=utf-8");

        let result = run(ScrapeParams {
            url,
            return_format: Some("auto".into()),
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
        let evidence: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(evidence["content"]
            .as_str()
            .unwrap()
            .contains("# Auto localhost"));
        assert_eq!(evidence["content_type"], "text/html; charset=utf-8");
        assert_eq!(evidence["detected_content_type"], "text/html");
        assert_eq!(
            evidence["response_body_hash"],
            format!("{:x}", Sha256::digest(BODY))
        );
        assert_eq!(
            evidence["transformed_content_hash"],
            format!(
                "{:x}",
                Sha256::digest(evidence["content"].as_str().unwrap().as_bytes())
            )
        );
        assert!(evidence["retrieved_at"].as_u64().is_some());

        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
    }

    fn localhost_auto_params(url: String, evidence: bool) -> ScrapeParams {
        ScrapeParams {
            url,
            return_format: Some("auto".into()),
            headless: Some(false),
            wait_for: None,
            wait_for_delay_ms: None,
            wait_for_idle_network: None,
            user_agent: None,
            cookie: None,
            proxy: None,
            evidence: Some(evidence),
        }
    }

    #[tokio::test]
    #[ignore = "localhost socket acceptance; run explicitly where loopback bind is permitted"]
    async fn localhost_auto_pdf_evidence_and_error_contract() {
        static BODY: &[u8] = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n%%EOF\n";
        let (url, stop, handle) = localhost_server(BODY, "text/html; charset=utf-8");

        let evidence_json = run(localhost_auto_params(url.clone(), true)).await.unwrap();
        let evidence: serde_json::Value = serde_json::from_str(&evidence_json).unwrap();
        assert!(evidence["content"].is_null());
        assert!(evidence["transformed_content_hash"].is_null());
        assert_eq!(evidence["content_type"], "text/html; charset=utf-8");
        assert_eq!(evidence["detected_content_type"], "application/pdf");
        assert_eq!(
            evidence["response_body_hash"],
            format!("{:x}", Sha256::digest(BODY))
        );
        assert_eq!(evidence["status_code"], 200);
        assert_eq!(evidence["requested_url"], url);
        assert!(evidence["final_url"].as_str().unwrap().starts_with(&url));
        assert!(evidence["retrieved_at"].as_u64().is_some());

        let error = run(localhost_auto_params(url, false)).await.unwrap_err();
        let error: serde_json::Value = serde_json::from_str(&error).unwrap();
        assert_eq!(error["error"], "auto_extraction_unsupported");
        assert_eq!(error["content_type"], "text/html; charset=utf-8");
        assert_eq!(error["detected_content_type"], "application/pdf");

        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
    }

    #[tokio::test]
    #[ignore = "localhost socket acceptance; run explicitly where loopback bind is permitted"]
    async fn localhost_auto_invalid_json_evidence_and_error_contract() {
        static BODY: &[u8] = b"definitely not valid JSON";
        let (url, stop, handle) = localhost_server(BODY, "application/json");

        let evidence_json = run(localhost_auto_params(url.clone(), true)).await.unwrap();
        let evidence: serde_json::Value = serde_json::from_str(&evidence_json).unwrap();
        assert!(evidence["content"].is_null());
        assert!(evidence["transformed_content_hash"].is_null());
        assert_eq!(evidence["content_type"], "application/json");
        assert!(evidence["detected_content_type"].is_null());
        assert_eq!(
            evidence["response_body_hash"],
            format!("{:x}", Sha256::digest(BODY))
        );
        assert_eq!(evidence["status_code"], 200);
        assert!(evidence["retrieved_at"].as_u64().is_some());

        let error = run(localhost_auto_params(url, false)).await.unwrap_err();
        let error: serde_json::Value = serde_json::from_str(&error).unwrap();
        assert_eq!(error["error"], "auto_extraction_unsupported");
        assert_eq!(error["content_type"], "application/json");
        assert!(error["detected_content_type"].is_null());
        assert!(error["message"]
            .as_str()
            .unwrap()
            .contains("could not be parsed"));

        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires a real local Chromium process and loopback socket"]
    async fn localhost_chromium_screenshot_hash_acceptance() {
        static BODY: &[u8] = b"<!doctype html><html><body><h1>Chromium evidence</h1></body></html>";
        let (url, stop, handle) = localhost_server(BODY, "text/html; charset=utf-8");

        let before = local_unix_epoch_millis();
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
        let after = local_unix_epoch_millis();
        let evidence: serde_json::Value = serde_json::from_str(&result).unwrap();
        let retrieved_at = evidence["retrieved_at"].as_u64().unwrap();
        assert!(before <= retrieved_at && retrieved_at <= after);
        let png = decode_base64_independently(evidence["screenshot"].as_str().unwrap());
        assert!(png.len() > 8);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(
            evidence["screenshot_hash"],
            format!("{:x}", Sha256::digest(&png))
        );
        assert!(evidence["response_body_hash"].is_null());
        assert!(evidence["detected_content_type"].is_null());
        assert!(evidence["transformed_content_hash"].is_null());

        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
    }
}
