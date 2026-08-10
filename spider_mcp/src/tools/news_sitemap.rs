//! One-fetch Google News Sitemap discovery with retrieval evidence.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use spider::features::news_sitemap::{
    NewsSitemapDiscoveryResult, NewsSitemapEntry, NewsSitemapParseFailure,
};

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
pub struct NewsSitemapReadParams {
    /// Direct News Sitemap URL.
    pub url: String,
    /// Return only the first N URL entries in source order.
    pub limit: Option<usize>,
    /// Transport for acquiring the News Sitemap document itself. Omit for
    /// Default (normal networking). Discovered article/media URLs are
    /// never fetched, so this never applies to them.
    pub transport: Option<crate::transport::TransportParam>,
}

#[derive(Clone, Debug, Serialize)]
struct NewsSitemapParseError {
    code: String,
    message: String,
}

#[derive(Clone, Debug, Serialize)]
struct NewsSitemapReadResult {
    sitemap_url: String,
    sitemap_type: Option<String>,
    result_count: usize,
    entries: Vec<NewsSitemapEntry>,
    evidence: crate::evidence::EvidenceBundle,
    parse_error: Option<NewsSitemapParseError>,
}

pub async fn run(params: NewsSitemapReadParams) -> Result<String, String> {
    let policy = crate::transport::resolve(params.transport)?;
    let page = super::fetch_document(&params.url, policy).await?;
    let bytes = page
        .get_bytes()
        .ok_or_else(|| "News Sitemap page arrived without a retained representation".to_string())?;
    let raw_xml = std::str::from_utf8(bytes).ok().map(str::to_string);
    let evidence = crate::evidence::build_evidence(&page, raw_xml, false, false);
    let parsed = spider::features::news_sitemap::parse(bytes, &params.url).await;
    let result = shape_result(params.url, params.limit, evidence, parsed);
    serde_json::to_string_pretty(&result).map_err(|error| error.to_string())
}

fn shape_result(
    sitemap_url: String,
    limit: Option<usize>,
    evidence: crate::evidence::EvidenceBundle,
    parsed: Result<NewsSitemapDiscoveryResult, NewsSitemapParseFailure>,
) -> NewsSitemapReadResult {
    match parsed {
        Ok(mut sitemap) => {
            if let Some(limit) = limit {
                sitemap.entries.truncate(limit);
            }
            NewsSitemapReadResult {
                sitemap_url,
                sitemap_type: Some(sitemap.sitemap_type),
                result_count: sitemap.entries.len(),
                entries: sitemap.entries,
                evidence,
                parse_error: None,
            }
        }
        Err(error) => {
            let code = match error {
                NewsSitemapParseFailure::NotSitemap(_) => "not_sitemap",
                NewsSitemapParseFailure::Parse(_) | NewsSitemapParseFailure::Panicked(_) => {
                    "news_sitemap_parse_failed"
                }
            };
            NewsSitemapReadResult {
                sitemap_url,
                sitemap_type: None,
                result_count: 0,
                entries: Vec::new(),
                evidence,
                parse_error: Some(NewsSitemapParseError {
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
        let url = format!("{base}/news-sitemap.xml");
        let body = build_body(&base);
        let served = body.clone();
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
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", served.len()).unwrap();
                        stream.write_all(&served).unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("localhost server failed: {error}"),
                }
            }
        });
        (url, body, paths, stop, handle)
    }

    async fn accepted<F>(
        build_body: F,
        limit: Option<usize>,
    ) -> (serde_json::Value, Vec<String>, Vec<u8>)
    where
        F: FnOnce(&str) -> Vec<u8>,
    {
        let (url, body, paths, stop, handle) = localhost(build_body);
        let output = run(NewsSitemapReadParams {
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

    #[tokio::test]
    async fn localhost_is_one_fetch_exact_evidence_and_preserves_mixed_duplicates() {
        let (value, paths, body) = accepted(|base| format!(r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:n="http://www.google.com/schemas/sitemap-news/0.9"><url><loc>{base}/article-a</loc><n:news><n:publication><n:name>Paper</n:name><n:language>en</n:language></n:publication><n:publication_date>2026-08-09T10:30+02:00</n:publication_date><n:title>A1</n:title></n:news></url><url><loc>{base}/article-b</loc><n:news><n:title>B</n:title></n:news></url><url><loc>{base}/ordinary-c</loc></url><url><loc>{base}/article-a</loc><n:news><n:title>A2</n:title></n:news></url></urlset>"#).into_bytes(), None).await;
        assert_eq!(paths, ["/news-sitemap.xml"]);
        assert_eq!(value["result_count"], 4);
        assert_eq!(value["entries"][0]["item"]["title"], "A1");
        assert_eq!(
            value["entries"][1]["news"]["publication_name"],
            serde_json::Value::Null
        );
        assert_eq!(value["entries"][2]["news"], serde_json::Value::Null);
        assert_eq!(
            value["entries"][0]["item"]["url"],
            value["entries"][3]["item"]["url"]
        );
        assert_eq!(value["entries"][3]["item"]["title"], "A2");
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
    async fn malformed_keeps_exact_evidence_and_leaks_no_partial_entries() {
        let malformed = b"<urlset><url><loc>http://example.test/a</loc></url><url>".to_vec();
        let (value, paths, body) = accepted(move |_| malformed, Some(0)).await;
        assert_eq!(paths, ["/news-sitemap.xml"]);
        assert_eq!(value["entries"], serde_json::json!([]));
        assert_eq!(value["result_count"], 0);
        assert_eq!(value["parse_error"]["code"], "news_sitemap_parse_failed");
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
    }

    /// H: a Tor `spider_news_sitemap_read` acquisition reaches the News
    /// Sitemap document exclusively via SOCKS, and (Section T) never
    /// fetches the article URLs it lists.
    #[cfg(feature = "transport_tor")]
    #[tokio::test]
    async fn tor_news_sitemap_read_reaches_document_only_via_socks_and_never_fetches_articles() {
        let body = br#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:n="http://www.google.com/schemas/sitemap-news/0.9"><url><loc>http://news-sitemap-tor-mcp-test.invalid/article-a</loc><n:news><n:title>A</n:title></n:news></url></urlset>"#;
        let http = crate::test_support::HttpFixture::start(std::str::from_utf8(body).unwrap());
        let socks = crate::test_support::SocksFixture::start(
            Some(http.addr),
            crate::test_support::SocksBehavior::Splice,
        );
        let url = format!(
            "http://news-sitemap-tor-mcp-test.invalid:{}/news-sitemap.xml",
            http.addr.port()
        );

        let output = run(NewsSitemapReadParams {
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
        assert_eq!(value["result_count"], 1);
    }

    #[test]
    fn input_schema_has_exactly_url_limit_and_transport() {
        let value = serde_json::to_value(schemars::schema_for!(NewsSitemapReadParams)).unwrap();
        let properties = value["properties"].as_object().unwrap();
        assert_eq!(
            properties.keys().collect::<Vec<_>>(),
            ["limit", "transport", "url"]
        );
    }

    #[test]
    fn limit_shapes_only_after_successful_parse() {
        let discovery = NewsSitemapDiscoveryResult {
            sitemap_type: "urlset".into(),
            entries: vec![Default::default(), Default::default()],
        };
        let zero = shape_result("x".into(), Some(0), Default::default(), Ok(discovery));
        assert_eq!(zero.result_count, 0);
        assert!(zero.entries.is_empty());
        assert!(zero.parse_error.is_none());

        let index = shape_result(
            "x".into(),
            None,
            Default::default(),
            Ok(NewsSitemapDiscoveryResult {
                sitemap_type: "sitemapindex".into(),
                entries: Vec::new(),
            }),
        );
        assert_eq!(index.sitemap_type.as_deref(), Some("sitemapindex"));
        assert_eq!(index.result_count, 0);
        assert!(index.entries.is_empty());
        assert!(index.parse_error.is_none());

        let not_sitemap = shape_result(
            "x".into(),
            None,
            Default::default(),
            Err(NewsSitemapParseFailure::NotSitemap("wrong root".into())),
        );
        assert_eq!(not_sitemap.parse_error.unwrap().code, "not_sitemap");
    }
}
