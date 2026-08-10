use rmcp::schemars;
use serde::{Deserialize, Serialize};
use spider::features::robots_sitemap::{
    RobotsSitemapDiscoveryResult, RobotsSitemapParseFailure, RobotsSitemapReference,
};

/// Input for `spider_robots_sitemap_read`.
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
pub struct RobotsSitemapReadParams {
    /// Direct robots.txt URL.
    pub url: String,
    /// Return only the first N declared sitemap URLs in source order.
    pub limit: Option<usize>,
    /// Transport for acquiring the robots.txt document itself. Omit for
    /// Default (normal networking). Declared Sitemap: URLs are never
    /// fetched, so this never applies to them.
    pub transport: Option<crate::transport::TransportParam>,
}

#[derive(Clone, Debug, Serialize)]
struct RobotsSitemapParseError {
    code: String,
    message: String,
}

#[derive(Clone, Debug, Serialize)]
struct RobotsSitemapReadResult {
    robots_url: String,
    result_count: usize,
    sitemaps: Vec<RobotsSitemapReference>,
    evidence: crate::evidence::EvidenceBundle,
    parse_error: Option<RobotsSitemapParseError>,
}

/// Fetch exactly one robots.txt document while preserving retrieval
/// evidence, and return declared `Sitemap:` URLs without fetching any of
/// them, any child sitemap, or any article/media URL.
pub async fn run(params: RobotsSitemapReadParams) -> Result<String, String> {
    let policy = crate::transport::resolve(params.transport)?;
    let page = super::fetch_document(&params.url, policy).await?;
    let bytes = page
        .get_bytes()
        .ok_or_else(|| "Robots page arrived without a retained representation".to_string())?;
    let raw_text = std::str::from_utf8(bytes).ok().map(str::to_string);
    let evidence = crate::evidence::build_evidence(&page, raw_text, false, false);
    let parsed = spider::features::robots_sitemap::parse(bytes).await;
    let result = shape_result(params.url, params.limit, evidence, parsed);
    serde_json::to_string_pretty(&result).map_err(|error| error.to_string())
}

fn shape_result(
    robots_url: String,
    limit: Option<usize>,
    evidence: crate::evidence::EvidenceBundle,
    parsed: Result<RobotsSitemapDiscoveryResult, RobotsSitemapParseFailure>,
) -> RobotsSitemapReadResult {
    match parsed {
        Ok(mut discovery) => {
            if let Some(limit) = limit {
                discovery.sitemaps.truncate(limit);
            }
            RobotsSitemapReadResult {
                robots_url,
                result_count: discovery.sitemaps.len(),
                sitemaps: discovery.sitemaps,
                evidence,
                parse_error: None,
            }
        }
        Err(error) => RobotsSitemapReadResult {
            robots_url,
            result_count: 0,
            sitemaps: Vec::new(),
            evidence,
            parse_error: Some(RobotsSitemapParseError {
                code: "robots_sitemap_parse_failed".into(),
                message: error.to_string(),
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    type LocalhostHandle = (
        String,
        Arc<Mutex<Vec<String>>>,
        Arc<AtomicBool>,
        std::thread::JoinHandle<()>,
    );

    fn localhost(status_line: &'static str, body: Vec<u8>) -> LocalhostHandle {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/robots.txt", listener.local_addr().unwrap());
        let paths = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_paths = paths.clone();
        let thread_stop = stop.clone();
        let handle = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut request = [0_u8; 2048];
                        let count = stream.read(&mut request).unwrap_or(0);
                        let request = String::from_utf8_lossy(&request[..count]);
                        let path = request
                            .lines()
                            .next()
                            .and_then(|line| line.split_whitespace().nth(1))
                            .unwrap_or("")
                            .to_string();
                        thread_paths.lock().unwrap().push(path);
                        write!(
                            stream,
                            "{status_line}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .unwrap();
                        stream.write_all(&body).unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("localhost server failed: {error}"),
                }
            }
        });
        (url, paths, stop, handle)
    }

    async fn accepted(
        status_line: &'static str,
        body: Vec<u8>,
        limit: Option<usize>,
    ) -> (serde_json::Value, Vec<String>, Vec<u8>) {
        let (url, paths, stop, handle) = localhost(status_line, body.clone());
        let output = run(RobotsSitemapReadParams {
            url,
            limit,
            transport: None,
        })
        .await
        .unwrap();
        let value = serde_json::from_str(&output).unwrap();
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        let log = paths.lock().unwrap().clone();
        (value, log, body)
    }

    const ROBOTS_BODY: &str = "User-agent: *\r\nDisallow: /private\r\n\r\nSitemap: {base}/sitemap.xml\r\nSitemap: {base}/news.xml\r\nSitemap: {base}/sitemap.xml\r\n";

    #[tokio::test]
    async fn localhost_is_one_fetch_exact_evidence_and_preserves_order_and_duplicates() {
        // The declared sitemap URLs point at this same listener, but must
        // never actually be requested.
        let (probe_url, _paths, stop, handle) = localhost("HTTP/1.1 200 OK", Vec::new());
        let base = probe_url.trim_end_matches("/robots.txt");
        let body = ROBOTS_BODY.replace("{base}", base).into_bytes();
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();

        let (value, paths, served) = accepted("HTTP/1.1 200 OK", body, None).await;
        assert_eq!(paths, ["/robots.txt"]);
        assert_eq!(value["result_count"], 3);
        assert_eq!(
            value["sitemaps"],
            serde_json::json!([
                {"url": format!("{base}/sitemap.xml")},
                {"url": format!("{base}/news.xml")},
                {"url": format!("{base}/sitemap.xml")},
            ])
        );
        assert_eq!(value["evidence"]["observed_status_code"], 200);
        assert!(value["evidence"]["retrieved_at"].as_u64().is_some());
        assert_eq!(
            value["evidence"]["content"],
            std::str::from_utf8(&served).unwrap()
        );
        assert_eq!(
            value["evidence"]["response_body_hash"],
            format!("{:x}", Sha256::digest(&served))
        );
        assert!(value["parse_error"].is_null());
    }

    #[tokio::test]
    async fn no_requests_are_made_to_any_declared_sitemap_or_private_path() {
        let (probe_url, _paths, stop, handle) = localhost("HTTP/1.1 200 OK", Vec::new());
        let base = probe_url.trim_end_matches("/robots.txt");
        let body = ROBOTS_BODY.replace("{base}", base).into_bytes();
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();

        let (_value, paths, _served) = accepted("HTTP/1.1 200 OK", body, None).await;
        assert_eq!(paths.len(), 1);
        assert!(!paths.iter().any(|p| p.contains("/sitemap.xml")
            || p.contains("/news.xml")
            || p.contains("/private")));
    }

    #[tokio::test]
    async fn limit_truncates_after_successful_parse() {
        let body = b"Sitemap: https://example.test/a.xml\nSitemap: https://example.test/b.xml\nSitemap: https://example.test/c.xml\n".to_vec();
        let (value, paths, _served) = accepted("HTTP/1.1 200 OK", body, Some(2)).await;
        assert_eq!(paths, ["/robots.txt"]);
        assert_eq!(value["result_count"], 2);
        assert_eq!(value["sitemaps"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn empty_robots_txt_succeeds_with_no_sitemaps() {
        let (value, paths, _served) = accepted("HTTP/1.1 200 OK", Vec::new(), None).await;
        assert_eq!(paths, ["/robots.txt"]);
        assert_eq!(value["result_count"], 0);
        assert_eq!(value["sitemaps"], serde_json::json!([]));
        assert!(value["parse_error"].is_null());
        assert_eq!(value["evidence"]["observed_status_code"], 200);
    }

    #[tokio::test]
    async fn http_error_response_preserves_truthful_evidence_and_scans_returned_body_only() {
        // A 404 error page's body happens to contain no Sitemap: lines —
        // the empty `sitemaps` result must not be mistaken for a genuine
        // "site declares no sitemaps" answer; the caller must observe the
        // true 404 status via evidence.
        let body = b"<html><body>Not Found</body></html>".to_vec();
        let (value, paths, served) = accepted("HTTP/1.1 404 Not Found", body, None).await;
        assert_eq!(paths, ["/robots.txt"]);
        assert_eq!(value["result_count"], 0);
        assert_eq!(value["sitemaps"], serde_json::json!([]));
        assert_eq!(value["evidence"]["observed_status_code"], 404);
        assert_eq!(
            value["evidence"]["content"],
            std::str::from_utf8(&served).unwrap()
        );
        assert_eq!(
            value["evidence"]["response_body_hash"],
            format!("{:x}", Sha256::digest(&served))
        );
    }

    #[tokio::test]
    async fn http_error_response_with_declared_sitemap_still_scans_it() {
        // Whatever body actually came back is scanned truthfully, even on
        // a non-2xx status — retrieval success and discovery result are
        // reported independently via evidence, not conflated.
        let body = b"Sitemap: https://example.test/sitemap.xml\n".to_vec();
        let (value, _paths, _served) =
            accepted("HTTP/1.1 500 Internal Server Error", body, None).await;
        assert_eq!(value["result_count"], 1);
        assert_eq!(value["evidence"]["observed_status_code"], 500);
    }

    #[tokio::test]
    async fn network_failure_is_an_explicit_error() {
        // Port chosen to be almost certainly unbound in any test environment.
        let result = run(RobotsSitemapReadParams {
            url: "http://127.0.0.1:1/robots.txt".to_string(),
            limit: None,
            transport: None,
        })
        .await;
        assert!(result.is_err());
    }

    /// H: a Tor `spider_robots_sitemap_read` acquisition reaches the
    /// robots.txt document exclusively via SOCKS, and (Section T) never
    /// fetches the declared Sitemap: URLs.
    #[cfg(feature = "transport_tor")]
    #[tokio::test]
    async fn tor_robots_sitemap_read_reaches_document_only_via_socks_and_never_fetches_declared() {
        let body = b"Sitemap: http://robots-tor-mcp-test.invalid/sitemap.xml\nSitemap: http://robots-tor-mcp-test.invalid/news.xml\n";
        let http = crate::test_support::HttpFixture::start(std::str::from_utf8(body).unwrap());
        let socks = crate::test_support::SocksFixture::start(
            Some(http.addr),
            crate::test_support::SocksBehavior::Splice,
        );
        let url = format!(
            "http://robots-tor-mcp-test.invalid:{}/robots.txt",
            http.addr.port()
        );

        let output = run(RobotsSitemapReadParams {
            url,
            limit: None,
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
        assert_eq!(value["result_count"], 2);
    }

    #[test]
    fn input_schema_has_exactly_url_limit_and_transport() {
        let value = serde_json::to_value(schemars::schema_for!(RobotsSitemapReadParams)).unwrap();
        let properties = value["properties"].as_object().unwrap();
        assert_eq!(
            properties.keys().collect::<Vec<_>>(),
            ["limit", "transport", "url"]
        );
    }

    #[test]
    fn pure_result_shaping_distinguishes_valid_empty_limited_and_parse_failure() {
        let discovery = |count: usize| RobotsSitemapDiscoveryResult {
            sitemaps: (0..count)
                .map(|index| RobotsSitemapReference {
                    url: format!("https://example.test/{index}.xml"),
                })
                .collect(),
        };

        let valid = shape_result("x".into(), None, Default::default(), Ok(discovery(2)));
        assert_eq!(valid.result_count, 2);
        assert!(valid.parse_error.is_none());

        let empty = shape_result("x".into(), None, Default::default(), Ok(discovery(0)));
        assert_eq!(empty.result_count, 0);
        assert!(empty.parse_error.is_none());

        let limited = shape_result("x".into(), Some(1), Default::default(), Ok(discovery(3)));
        assert_eq!(limited.result_count, 1);
        assert_eq!(limited.sitemaps[0].url, "https://example.test/0.xml");

        let failed = shape_result(
            "x".into(),
            None,
            crate::evidence::EvidenceBundle {
                observed_status_code: Some(200),
                ..Default::default()
            },
            Err(RobotsSitemapParseFailure::Panicked("boom".into())),
        );
        assert_eq!(failed.result_count, 0);
        assert!(failed.sitemaps.is_empty());
        assert_eq!(failed.evidence.observed_status_code, Some(200));
        assert_eq!(
            failed.parse_error.unwrap().code,
            "robots_sitemap_parse_failed"
        );
    }
}
