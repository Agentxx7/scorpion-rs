use rmcp::schemars;
use serde::{Deserialize, Serialize};
use spider::features::sitemap::{SitemapDiscoveryResult, SitemapParseFailure, SitemapReference};
use spider::features::source::SourceItem;

/// Input for `spider_sitemap_read`.
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
pub struct SitemapReadParams {
    /// Direct standard sitemap URL.
    pub url: String,
    /// Return only the first N candidates in source order.
    pub limit: Option<usize>,
    /// Transport for acquiring the sitemap document itself. Omit for
    /// Default (normal networking). Candidate URLs the sitemap lists are
    /// never fetched, so this never applies to them.
    pub transport: Option<crate::transport::TransportParam>,
}

#[derive(Clone, Debug, Serialize)]
struct SitemapParseError {
    code: String,
    message: String,
}

#[derive(Clone, Debug, Serialize)]
struct SitemapReadResult {
    sitemap_url: String,
    sitemap_type: Option<String>,
    result_count: usize,
    entries: Vec<SourceItem>,
    child_sitemaps: Vec<SitemapReference>,
    evidence: crate::evidence::EvidenceBundle,
    parse_error: Option<SitemapParseError>,
}

/// Fetch and parse one standard sitemap while preserving retrieval evidence.
pub async fn run(params: SitemapReadParams) -> Result<String, String> {
    let policy = crate::transport::resolve(params.transport)?;
    let page = super::fetch_document(&params.url, policy).await?;
    let bytes = page
        .get_bytes()
        .ok_or_else(|| "Sitemap page arrived without a retained representation".to_string())?;
    let raw_xml = std::str::from_utf8(bytes).ok().map(str::to_string);
    let evidence = crate::evidence::build_evidence(&page, raw_xml, false, false);
    let parsed = spider::features::sitemap::parse(bytes, &params.url).await;
    let result = shape_result(params.url, params.limit, evidence, parsed);
    serde_json::to_string_pretty(&result).map_err(|error| error.to_string())
}

fn shape_result(
    sitemap_url: String,
    limit: Option<usize>,
    evidence: crate::evidence::EvidenceBundle,
    parsed: Result<SitemapDiscoveryResult, SitemapParseFailure>,
) -> SitemapReadResult {
    match parsed {
        Ok(mut sitemap) => {
            if let Some(limit) = limit {
                if sitemap.sitemap_type == "urlset" {
                    sitemap.entries.truncate(limit);
                } else {
                    sitemap.child_sitemaps.truncate(limit);
                }
            }
            let result_count = if sitemap.sitemap_type == "urlset" {
                sitemap.entries.len()
            } else {
                sitemap.child_sitemaps.len()
            };
            SitemapReadResult {
                sitemap_url,
                sitemap_type: Some(sitemap.sitemap_type),
                result_count,
                entries: sitemap.entries,
                child_sitemaps: sitemap.child_sitemaps,
                evidence,
                parse_error: None,
            }
        }
        Err(error) => {
            let code = match error {
                SitemapParseFailure::NotSitemap(_) => "not_sitemap",
                SitemapParseFailure::Parse(_) | SitemapParseFailure::Panicked(_) => {
                    "sitemap_parse_failed"
                }
            };
            SitemapReadResult {
                sitemap_url,
                sitemap_type: None,
                result_count: 0,
                entries: Vec::new(),
                child_sitemaps: Vec::new(),
                evidence,
                parse_error: Some(SitemapParseError {
                    code: code.into(),
                    message: error.to_string(),
                }),
            }
        }
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

    fn localhost<F>(
        path: &str,
        build_body: F,
    ) -> (
        String,
        Vec<u8>,
        Arc<Mutex<Vec<String>>>,
        Arc<AtomicBool>,
        std::thread::JoinHandle<()>,
    )
    where
        F: FnOnce(&str) -> Vec<u8>,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let url = format!("{base}{path}");
        let body = build_body(&base);
        let served_body = body.clone();
        let paths = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let paths_thread = paths.clone();
        let stop_thread = stop.clone();
        let handle = std::thread::spawn(move || {
            while !stop_thread.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut request = [0_u8; 2048];
                        let read = stream.read(&mut request).unwrap_or(0);
                        let request = String::from_utf8_lossy(&request[..read]);
                        let path = request
                            .lines()
                            .next()
                            .and_then(|line| line.split_whitespace().nth(1))
                            .unwrap_or("")
                            .to_string();
                        paths_thread.lock().unwrap().push(path);
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", served_body.len()).unwrap();
                        stream.write_all(&served_body).unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("localhost server failed: {error}"),
                }
            }
        });
        (url, body, paths, stop, handle)
    }

    async fn accepted<F>(
        path: &str,
        build_body: F,
        limit: Option<usize>,
    ) -> (serde_json::Value, Vec<String>, Vec<u8>)
    where
        F: FnOnce(&str) -> Vec<u8>,
    {
        let (url, body, paths, stop, handle) = localhost(path, build_body);
        let output = run(SitemapReadParams {
            url,
            limit,
            transport: None,
        })
        .await
        .unwrap();
        let value = serde_json::from_str(&output).unwrap();
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        let requested_paths = paths.lock().unwrap().clone();
        (value, requested_paths, body)
    }

    #[tokio::test]
    async fn localhost_urlset_is_one_fetch_exact_evidence_and_no_candidate_fetches() {
        let (value, paths, body) = accepted(
            "/sitemap.xml",
            |base| {
                format!(r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><url><loc>{base}/article-a</loc><lastmod>2024-01-01</lastmod></url><url><loc>{base}/article-b</loc></url><url><loc>{base}/image.jpg</loc></url><url><loc>{base}/article-a</loc></url></urlset>"#).into_bytes()
            },
            None,
        ).await;
        assert_eq!(paths, ["/sitemap.xml"]);
        assert_eq!(value["sitemap_type"], "urlset");
        assert_eq!(value["result_count"], 4);
        assert_eq!(value["entries"][0]["url"], value["entries"][3]["url"]);
        assert!(value["entries"][0]["updated_at"].as_u64().is_some());
        assert_eq!(value["entries"][1]["updated_at"], serde_json::Value::Null);
        assert_eq!(value["evidence"]["observed_status_code"], 200);
        assert!(value["evidence"]["retrieved_at"].as_u64().is_some());
        assert_eq!(
            value["evidence"]["content"],
            std::str::from_utf8(&body).unwrap()
        );
        assert_eq!(
            value["evidence"]["response_body_hash"],
            format!("{:x}", Sha256::digest(&body))
        );
        assert!(value["parse_error"].is_null());
    }

    #[tokio::test]
    async fn localhost_sitemapindex_is_one_fetch_without_recursion() {
        let (value, paths, body) = accepted(
            "/sitemap-index.xml",
            |base| format!(r#"<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><sitemap><loc>{base}/child-a.xml</loc><lastmod>2024-02-01</lastmod></sitemap><sitemap><loc>{base}/child-b.xml</loc></sitemap></sitemapindex>"#).into_bytes(),
            None,
        ).await;
        assert_eq!(paths, ["/sitemap-index.xml"]);
        assert_eq!(value["sitemap_type"], "sitemapindex");
        assert_eq!(value["entries"], serde_json::json!([]));
        assert_eq!(value["child_sitemaps"].as_array().unwrap().len(), 2);
        assert!(value["child_sitemaps"][0]["updated_at"].as_u64().is_some());
        assert_eq!(
            value["evidence"]["response_body_hash"],
            format!("{:x}", Sha256::digest(&body))
        );
    }

    #[tokio::test]
    async fn malformed_sitemap_preserves_retrieval_evidence_without_partial_results() {
        let malformed = b"<urlset><url><loc>http://example.test/a</loc></url><url>".to_vec();
        let expected = malformed.clone();
        let (value, paths, body) = accepted("/broken.xml", move |_| malformed, None).await;
        assert_eq!(paths, ["/broken.xml"]);
        assert_eq!(body, expected);
        assert_eq!(value["result_count"], 0);
        assert_eq!(value["entries"], serde_json::json!([]));
        assert_eq!(value["child_sitemaps"], serde_json::json!([]));
        assert_eq!(value["parse_error"]["code"], "sitemap_parse_failed");
        assert!(!value["parse_error"]["message"].as_str().unwrap().is_empty());
        assert_eq!(value["evidence"]["observed_status_code"], 200);
        assert!(value["evidence"]["retrieved_at"].as_u64().is_some());
        assert_eq!(
            value["evidence"]["response_body_hash"],
            format!("{:x}", Sha256::digest(&body))
        );
    }

    /// H: a Tor `spider_sitemap_read` acquisition reaches the sitemap
    /// document exclusively via SOCKS, and (Section T) never fetches the
    /// candidate URLs it lists — discovery stays separate from
    /// acquisition under Tor too.
    #[cfg(feature = "transport_tor")]
    #[tokio::test]
    async fn tor_sitemap_read_reaches_document_only_via_socks_and_never_fetches_candidates() {
        let body = br#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><url><loc>http://sitemap-tor-mcp-test.invalid/article-a</loc></url><url><loc>http://sitemap-tor-mcp-test.invalid/article-b</loc></url></urlset>"#;
        let http = crate::test_support::HttpFixture::start(std::str::from_utf8(body).unwrap());
        let socks = crate::test_support::SocksFixture::start(
            Some(http.addr),
            crate::test_support::SocksBehavior::Splice,
        );
        let url = format!(
            "http://sitemap-tor-mcp-test.invalid:{}/sitemap.xml",
            http.addr.port()
        );

        let output = run(SitemapReadParams {
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
        let schema = schemars::schema_for!(SitemapReadParams);
        let value = serde_json::to_value(schema).unwrap();
        let properties = value["properties"].as_object().unwrap();
        assert_eq!(properties.len(), 3);
        assert!(properties.contains_key("url"));
        assert!(properties.contains_key("limit"));
        assert!(properties.contains_key("transport"));
        for forbidden in [
            "query",
            "source_hint",
            "language",
            "recursive",
            "browser",
            "screenshot",
        ] {
            assert!(!properties.contains_key(forbidden));
        }
    }

    fn discovery(sitemap_type: &str, count: usize) -> SitemapDiscoveryResult {
        if sitemap_type == "urlset" {
            SitemapDiscoveryResult {
                sitemap_type: sitemap_type.into(),
                entries: (0..count)
                    .map(|index| SourceItem {
                        source_type: "sitemap".into(),
                        url: Some(format!("https://example.test/{index}")),
                        discovered_via: Some("https://example.test/sitemap.xml".into()),
                        ..Default::default()
                    })
                    .collect(),
                child_sitemaps: Vec::new(),
            }
        } else {
            SitemapDiscoveryResult {
                sitemap_type: sitemap_type.into(),
                entries: Vec::new(),
                child_sitemaps: (0..count)
                    .map(|index| SitemapReference {
                        url: format!("https://example.test/{index}.xml"),
                        updated_at: None,
                    })
                    .collect(),
            }
        }
    }

    #[test]
    fn pure_shaping_handles_both_types_limits_zero_empty_and_failure_evidence() {
        for (kind, count) in [("urlset", 3), ("sitemapindex", 3)] {
            let limited = shape_result(
                "https://example.test/sitemap.xml".into(),
                Some(1),
                Default::default(),
                Ok(discovery(kind, count)),
            );
            assert_eq!(limited.result_count, 1);
            assert_eq!(limited.sitemap_type.as_deref(), Some(kind));

            let zero = shape_result(
                "https://example.test/sitemap.xml".into(),
                Some(0),
                Default::default(),
                Ok(discovery(kind, count)),
            );
            assert_eq!(zero.result_count, 0);
            assert!(zero.entries.is_empty() && zero.child_sitemaps.is_empty());

            let empty = shape_result(
                "https://example.test/sitemap.xml".into(),
                None,
                Default::default(),
                Ok(discovery(kind, 0)),
            );
            assert_eq!(empty.result_count, 0);
            assert!(empty.parse_error.is_none());
        }

        let failed = shape_result(
            "https://example.test/sitemap.xml".into(),
            None,
            crate::evidence::EvidenceBundle {
                observed_status_code: Some(200),
                ..Default::default()
            },
            Err(SitemapParseFailure::NotSitemap("not a sitemap".into())),
        );
        assert_eq!(failed.result_count, 0);
        assert!(failed.entries.is_empty() && failed.child_sitemaps.is_empty());
        assert_eq!(failed.evidence.observed_status_code, Some(200));
        assert_eq!(failed.parse_error.unwrap().code, "not_sitemap");
    }
}
