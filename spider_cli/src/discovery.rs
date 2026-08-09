//! Canonical CLI commands over Scorpion's discovery/evidence core.
//!
//! Every command here calls exactly the same core seam
//! (`spider::utils::evidence::{fetch_single_page, build_evidence}`,
//! `spider::features::{feed,sitemap,news_sitemap,robots_sitemap}::parse`)
//! that `spider_mcp`'s own tools call — no acquisition/evidence/parsing
//! logic is duplicated here, only the JSON presentation shape is
//! CLI-local, and it is kept as close to each MCP tool's own field names
//! as practical rather than inventing a separate normalized contract.
//!
//! Each `run_*` function returns the JSON string to print — it never
//! writes to stdout itself — so the shaping logic is directly testable
//! without capturing process output, matching the pattern already
//! established by every `spider_mcp` tool.

#[cfg(any(
    feature = "feed",
    feature = "sitemap",
    feature = "news_sitemap",
    feature = "robots_sitemap"
))]
use serde::Serialize;
#[cfg(any(
    feature = "feed",
    feature = "sitemap",
    feature = "news_sitemap",
    feature = "robots_sitemap"
))]
use spider::utils::evidence::EvidenceBundle;
use spider::utils::evidence::{build_evidence, fetch_single_page};

/// `spider fetch <url>` — exactly one evidence-first resource acquisition.
/// No crawl following, no browser, no content transformation, no
/// discovery. The output losslessly exposes the canonical `EvidenceBundle`.
#[cfg(feature = "fetch")]
pub async fn run_fetch(url: &str) -> Result<String, String> {
    let page = fetch_single_page(url).await?;
    let content = page
        .get_bytes()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .map(str::to_string);
    let evidence = build_evidence(&page, content, false, false);
    serde_json::to_string_pretty(&evidence).map_err(|error| error.to_string())
}

#[cfg(all(test, feature = "fetch"))]
mod fetch_tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    fn localhost(
        body: &'static [u8],
    ) -> (
        String,
        Arc<AtomicUsize>,
        Arc<AtomicBool>,
        std::thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/resource", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let requests_thread = requests.clone();
        let stop_thread = stop.clone();
        let handle = std::thread::spawn(move || {
            while !stop_thread.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        requests_thread.fetch_add(1, Ordering::Relaxed);
                        let mut request = [0_u8; 2048];
                        let _ = stream.read(&mut request);
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                        stream.write_all(body).unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("localhost server failed: {error}"),
                }
            }
        });
        (url, requests, stop, handle)
    }

    #[tokio::test]
    async fn fetch_is_one_request_valid_json_with_truthful_evidence() {
        const BODY: &[u8] = b"hello from scorpion fetch";
        let (url, requests, stop, handle) = localhost(BODY);
        let output = run_fetch(&url).await.unwrap();
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();

        assert_eq!(requests.load(Ordering::Relaxed), 1);
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["observed_status_code"], 200);
        assert!(value["retrieved_at"].as_u64().is_some());
        assert_eq!(value["content"], std::str::from_utf8(BODY).unwrap());
        assert_eq!(
            value["response_body_hash"],
            format!("{:x}", Sha256::digest(BODY))
        );
        assert_eq!(value["requested_url"], url);
    }

    #[tokio::test]
    async fn fetch_invalid_url_is_explicit_error() {
        let result = run_fetch("not a url").await;
        assert!(result.is_err());
    }

    /// A connection-level failure (DNS/connect refused) is NOT a
    /// `fetch_single_page` `Err` — Spider represents it as a successful
    /// `Page` whose internal status reflects the failure. Empirically
    /// confirmed: `status_code` is reclassified (503), `observed_status_code`
    /// stays `None` because no real wire response was ever received, and no
    /// bytes/hash/content are present. This is the same "retrieval status
    /// != process failure" distinction every other command preserves — the
    /// CLI process succeeds and reports truthful, absent evidence, it does
    /// not fabricate an error exit for a condition Spider itself can
    /// truthfully represent as data.
    #[tokio::test]
    async fn fetch_connection_failure_is_truthful_evidence_not_a_process_error() {
        // Port chosen to be almost certainly unbound in any test environment.
        let output = run_fetch("http://127.0.0.1:1/resource")
            .await
            .expect("a connection failure is representable as evidence, not an Err");
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["observed_status_code"], serde_json::Value::Null);
        assert_eq!(value["response_body_hash"], serde_json::Value::Null);
        assert_eq!(value["content"], serde_json::Value::Null);
    }
}

#[cfg(feature = "feed")]
mod feed_cmd {
    use super::*;
    use spider::features::feed::{self, FeedDiscoveryResult, FeedParseFailure};
    use spider::features::source::SourceItem;

    #[derive(Serialize)]
    struct FeedParseError {
        code: String,
        message: String,
    }

    #[derive(Serialize)]
    struct FeedResult {
        feed_url: String,
        feed_type: Option<String>,
        feed_title: Option<String>,
        feed_authors: Vec<String>,
        language: Option<String>,
        result_count: usize,
        entries: Vec<SourceItem>,
        evidence: EvidenceBundle,
        parse_error: Option<FeedParseError>,
    }

    pub async fn run(url: &str, limit: Option<usize>) -> Result<String, String> {
        let page = fetch_single_page(url).await?;
        let bytes = page
            .get_bytes()
            .ok_or_else(|| "Feed page arrived without a retained representation".to_string())?;
        let raw_xml = std::str::from_utf8(bytes).ok().map(str::to_string);
        let evidence = build_evidence(&page, raw_xml, false, false);
        let parsed = feed::parse(bytes, url).await;
        let result = shape(url.to_string(), limit, evidence, parsed);
        serde_json::to_string_pretty(&result).map_err(|error| error.to_string())
    }

    fn shape(
        feed_url: String,
        limit: Option<usize>,
        evidence: EvidenceBundle,
        parsed: Result<FeedDiscoveryResult, FeedParseFailure>,
    ) -> FeedResult {
        match parsed {
            Ok(mut discovery) => {
                if let Some(limit) = limit {
                    discovery.entries.truncate(limit);
                }
                FeedResult {
                    feed_url,
                    feed_type: Some(discovery.feed_type),
                    feed_title: discovery.feed_title,
                    feed_authors: discovery.feed_authors,
                    language: discovery.language,
                    result_count: discovery.entries.len(),
                    entries: discovery.entries,
                    evidence,
                    parse_error: None,
                }
            }
            Err(error) => {
                let code = match error {
                    FeedParseFailure::NotFeed(_) => "not_feed",
                    FeedParseFailure::Parse(_) | FeedParseFailure::Panicked(_) => {
                        "feed_parse_failed"
                    }
                };
                FeedResult {
                    feed_url,
                    feed_type: None,
                    feed_title: None,
                    feed_authors: Vec::new(),
                    language: None,
                    result_count: 0,
                    entries: Vec::new(),
                    evidence,
                    parse_error: Some(FeedParseError {
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

        const RSS: &str = r#"<rss version="2.0"><channel><title>Local RSS</title><item><guid>one</guid><link>http://127.0.0.1/article-one</link><title>One</title></item><item><guid>two</guid><link>http://127.0.0.1/article-two</link><title>Two</title></item></channel></rss>"#;

        type LocalhostHandle = (
            String,
            Arc<Mutex<Vec<String>>>,
            Arc<AtomicBool>,
            std::thread::JoinHandle<()>,
        );

        fn localhost(body: &'static [u8]) -> LocalhostHandle {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = format!("http://{}/feed.xml", listener.local_addr().unwrap());
            let paths = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let paths_thread = paths.clone();
            let stop_thread = stop.clone();
            let handle = std::thread::spawn(move || {
                while !stop_thread.load(Ordering::Relaxed) {
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
                            paths_thread.lock().unwrap().push(path);
                            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                            stream.write_all(body).unwrap();
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("localhost server failed: {error}"),
                    }
                }
            });
            (url, paths, stop, handle)
        }

        async fn accepted(
            body: &'static [u8],
            limit: Option<usize>,
        ) -> (serde_json::Value, Vec<String>, &'static [u8]) {
            let (url, paths, stop, handle) = localhost(body);
            let output = run(&url, limit).await.unwrap();
            let value: serde_json::Value = serde_json::from_str(&output).unwrap();
            stop.store(true, Ordering::Relaxed);
            handle.join().unwrap();
            let log = paths.lock().unwrap().clone();
            (value, log, body)
        }

        #[tokio::test]
        async fn feed_is_one_fetch_exact_evidence_and_order() {
            let (value, paths, body) = accepted(RSS.as_bytes(), None).await;
            assert_eq!(paths, ["/feed.xml"]);
            assert_eq!(value["feed_type"], "rss2");
            assert_eq!(value["result_count"], 2);
            assert_eq!(value["entries"][0]["source_item_id"], "one");
            assert_eq!(value["entries"][1]["source_item_id"], "two");
            assert_eq!(value["evidence"]["observed_status_code"], 200);
            assert!(value["evidence"]["retrieved_at"].as_u64().is_some());
            assert_eq!(
                value["evidence"]["content"],
                std::str::from_utf8(body).unwrap()
            );
            assert_eq!(
                value["evidence"]["response_body_hash"],
                format!("{:x}", Sha256::digest(body))
            );
            assert!(value["parse_error"].is_null());
        }

        #[tokio::test]
        async fn feed_limit_truncates_after_parse_without_extra_fetch() {
            let (value, paths, _body) = accepted(RSS.as_bytes(), Some(1)).await;
            assert_eq!(paths, ["/feed.xml"]);
            assert_eq!(value["result_count"], 1);
            assert_eq!(value["entries"][0]["source_item_id"], "one");
        }

        #[tokio::test]
        async fn feed_never_fetches_discovered_article_urls() {
            let (_value, paths, _body) = accepted(RSS.as_bytes(), None).await;
            assert_eq!(paths.len(), 1);
            assert!(!paths
                .iter()
                .any(|p| p.contains("article-one") || p.contains("article-two")));
        }

        #[tokio::test]
        async fn feed_malformed_body_preserves_evidence_with_parse_error() {
            let malformed: &'static [u8] = b"<rss><channel><item>";
            let (value, paths, _body) = accepted(malformed, None).await;
            assert_eq!(paths, ["/feed.xml"]);
            assert_eq!(value["result_count"], 0);
            assert_eq!(value["parse_error"]["code"], "feed_parse_failed");
            assert_eq!(value["evidence"]["observed_status_code"], 200);
        }

        /// HTTP 404/500 with a returned body: the CLI process still
        /// succeeds, JSON is still valid, and evidence reports the true
        /// non-2xx status — retrieval status is never conflated with
        /// process failure.
        #[tokio::test]
        async fn feed_http_error_response_preserves_truthful_evidence() {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = format!("http://{}/feed.xml", listener.local_addr().unwrap());
            let stop = Arc::new(AtomicBool::new(false));
            let stop_thread = stop.clone();
            let body = b"<html><body>Not Found</body></html>";
            let handle = std::thread::spawn(move || {
                while !stop_thread.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let mut request = [0_u8; 2048];
                            let _ = stream.read(&mut request);
                            write!(stream, "HTTP/1.1 404 Not Found\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                            stream.write_all(body).unwrap();
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("localhost server failed: {error}"),
                    }
                }
            });

            let output = run(&url, None).await.unwrap();
            stop.store(true, Ordering::Relaxed);
            handle.join().unwrap();

            let value: serde_json::Value = serde_json::from_str(&output).unwrap();
            assert_eq!(value["evidence"]["observed_status_code"], 404);
            assert_eq!(
                value["evidence"]["response_body_hash"],
                format!("{:x}", Sha256::digest(body))
            );
        }

        /// A syntactically invalid URL is the one case that fails before
        /// any retrieval evidence can exist — a genuine process/input
        /// error, non-zero-exit at the `main.rs` dispatch layer.
        #[tokio::test]
        async fn feed_invalid_url_is_explicit_error() {
            let result = run("not a url", None).await;
            assert!(result.is_err());
        }
    }
}
#[cfg(feature = "feed")]
pub use feed_cmd::run as run_feed;

#[cfg(feature = "sitemap")]
mod sitemap_cmd {
    use super::*;
    use spider::features::sitemap::{
        self, SitemapDiscoveryResult, SitemapParseFailure, SitemapReference,
    };
    use spider::features::source::SourceItem;

    #[derive(Serialize)]
    struct SitemapParseError {
        code: String,
        message: String,
    }

    #[derive(Serialize)]
    struct SitemapResult {
        sitemap_url: String,
        sitemap_type: Option<String>,
        result_count: usize,
        entries: Vec<SourceItem>,
        child_sitemaps: Vec<SitemapReference>,
        evidence: EvidenceBundle,
        parse_error: Option<SitemapParseError>,
    }

    pub async fn run(url: &str, limit: Option<usize>) -> Result<String, String> {
        let page = fetch_single_page(url).await?;
        let bytes = page
            .get_bytes()
            .ok_or_else(|| "Sitemap page arrived without a retained representation".to_string())?;
        let raw_xml = std::str::from_utf8(bytes).ok().map(str::to_string);
        let evidence = build_evidence(&page, raw_xml, false, false);
        let parsed = sitemap::parse(bytes, url).await;
        let result = shape(url.to_string(), limit, evidence, parsed);
        serde_json::to_string_pretty(&result).map_err(|error| error.to_string())
    }

    fn shape(
        sitemap_url: String,
        limit: Option<usize>,
        evidence: EvidenceBundle,
        parsed: Result<SitemapDiscoveryResult, SitemapParseFailure>,
    ) -> SitemapResult {
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
                SitemapResult {
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
                SitemapResult {
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

        type LocalhostHandle = (
            String,
            Arc<Mutex<Vec<String>>>,
            Arc<AtomicBool>,
            std::thread::JoinHandle<()>,
        );

        fn localhost(body: &'static [u8]) -> LocalhostHandle {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = format!("http://{}/sitemap.xml", listener.local_addr().unwrap());
            let paths = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let paths_thread = paths.clone();
            let stop_thread = stop.clone();
            let handle = std::thread::spawn(move || {
                while !stop_thread.load(Ordering::Relaxed) {
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
                            paths_thread.lock().unwrap().push(path);
                            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                            stream.write_all(body).unwrap();
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("localhost server failed: {error}"),
                    }
                }
            });
            (url, paths, stop, handle)
        }

        async fn accepted(
            body: &'static [u8],
            limit: Option<usize>,
        ) -> (serde_json::Value, Vec<String>, &'static [u8]) {
            let (url, paths, stop, handle) = localhost(body);
            let output = run(&url, limit).await.unwrap();
            let value: serde_json::Value = serde_json::from_str(&output).unwrap();
            stop.store(true, Ordering::Relaxed);
            handle.join().unwrap();
            let log = paths.lock().unwrap().clone();
            (value, log, body)
        }

        #[tokio::test]
        async fn sitemap_is_one_fetch_exact_evidence_and_no_candidate_fetches() {
            const URLSET: &[u8] = br#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><url><loc>http://example.test/article-a</loc><lastmod>2024-01-01</lastmod></url><url><loc>http://example.test/article-b</loc></url></urlset>"#;
            let (value, paths, body) = accepted(URLSET, None).await;
            assert_eq!(paths, ["/sitemap.xml"]);
            assert_eq!(value["sitemap_type"], "urlset");
            assert_eq!(value["result_count"], 2);
            assert!(value["entries"][0]["updated_at"].as_u64().is_some());
            assert_eq!(value["evidence"]["observed_status_code"], 200);
            assert!(value["evidence"]["retrieved_at"].as_u64().is_some());
            assert_eq!(
                value["evidence"]["response_body_hash"],
                format!("{:x}", Sha256::digest(body))
            );
            assert!(value["parse_error"].is_null());
            assert!(!paths
                .iter()
                .any(|p| p.contains("article-a") || p.contains("article-b")));
        }

        #[tokio::test]
        async fn sitemap_limit_truncates_entries_without_extra_fetch() {
            const URLSET: &[u8] = br#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><url><loc>http://example.test/a</loc></url><url><loc>http://example.test/b</loc></url></urlset>"#;
            let (value, paths, _body) = accepted(URLSET, Some(1)).await;
            assert_eq!(paths, ["/sitemap.xml"]);
            assert_eq!(value["result_count"], 1);
        }

        #[tokio::test]
        async fn sitemap_malformed_body_preserves_evidence_with_parse_error() {
            let malformed: &'static [u8] =
                b"<urlset><url><loc>http://example.test/a</loc></url><url>";
            let (value, paths, _body) = accepted(malformed, None).await;
            assert_eq!(paths, ["/sitemap.xml"]);
            assert_eq!(value["result_count"], 0);
            assert_eq!(value["parse_error"]["code"], "sitemap_parse_failed");
            assert_eq!(value["evidence"]["observed_status_code"], 200);
        }
    }
}
#[cfg(feature = "sitemap")]
pub use sitemap_cmd::run as run_sitemap;

#[cfg(feature = "news_sitemap")]
mod news_sitemap_cmd {
    use super::*;
    use spider::features::news_sitemap::{
        self, NewsSitemapDiscoveryResult, NewsSitemapEntry, NewsSitemapParseFailure,
    };

    #[derive(Serialize)]
    struct NewsSitemapParseError {
        code: String,
        message: String,
    }

    #[derive(Serialize)]
    struct NewsSitemapResult {
        sitemap_url: String,
        sitemap_type: Option<String>,
        result_count: usize,
        entries: Vec<NewsSitemapEntry>,
        evidence: EvidenceBundle,
        parse_error: Option<NewsSitemapParseError>,
    }

    pub async fn run(url: &str, limit: Option<usize>) -> Result<String, String> {
        let page = fetch_single_page(url).await?;
        let bytes = page.get_bytes().ok_or_else(|| {
            "News Sitemap page arrived without a retained representation".to_string()
        })?;
        let raw_xml = std::str::from_utf8(bytes).ok().map(str::to_string);
        let evidence = build_evidence(&page, raw_xml, false, false);
        let parsed = news_sitemap::parse(bytes, url).await;
        let result = shape(url.to_string(), limit, evidence, parsed);
        serde_json::to_string_pretty(&result).map_err(|error| error.to_string())
    }

    fn shape(
        sitemap_url: String,
        limit: Option<usize>,
        evidence: EvidenceBundle,
        parsed: Result<NewsSitemapDiscoveryResult, NewsSitemapParseFailure>,
    ) -> NewsSitemapResult {
        match parsed {
            Ok(mut sitemap) => {
                if let Some(limit) = limit {
                    sitemap.entries.truncate(limit);
                }
                NewsSitemapResult {
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
                NewsSitemapResult {
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

        type LocalhostHandle = (
            String,
            Arc<Mutex<Vec<String>>>,
            Arc<AtomicBool>,
            std::thread::JoinHandle<()>,
        );

        fn localhost(body: &'static [u8]) -> LocalhostHandle {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = format!("http://{}/news-sitemap.xml", listener.local_addr().unwrap());
            let paths = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let paths_thread = paths.clone();
            let stop_thread = stop.clone();
            let handle = std::thread::spawn(move || {
                while !stop_thread.load(Ordering::Relaxed) {
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
                            paths_thread.lock().unwrap().push(path);
                            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                            stream.write_all(body).unwrap();
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("localhost server failed: {error}"),
                    }
                }
            });
            (url, paths, stop, handle)
        }

        async fn accepted(
            body: &'static [u8],
            limit: Option<usize>,
        ) -> (serde_json::Value, Vec<String>, &'static [u8]) {
            let (url, paths, stop, handle) = localhost(body);
            let output = run(&url, limit).await.unwrap();
            let value: serde_json::Value = serde_json::from_str(&output).unwrap();
            stop.store(true, Ordering::Relaxed);
            handle.join().unwrap();
            let log = paths.lock().unwrap().clone();
            (value, log, body)
        }

        #[tokio::test]
        async fn news_sitemap_is_one_fetch_with_truthful_news_entries() {
            const DOC: &[u8] = br#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:news="http://www.google.com/schemas/sitemap-news/0.9"><url><loc>http://example.test/article-a</loc><news:news><news:publication><news:name>Paper</news:name><news:language>en</news:language></news:publication><news:publication_date>2026-08-09T10:30:00Z</news:publication_date><news:title>Title A</news:title></news:news></url><url><loc>http://example.test/article-b</loc></url></urlset>"#;
            let (value, paths, body) = accepted(DOC, None).await;
            assert_eq!(paths, ["/news-sitemap.xml"]);
            assert_eq!(value["sitemap_type"], "urlset");
            assert_eq!(value["result_count"], 2);
            assert_eq!(value["entries"][0]["item"]["title"], "Title A");
            assert_eq!(value["entries"][0]["news"]["publication_name"], "Paper");
            assert_eq!(value["entries"][1]["news"], serde_json::Value::Null);
            assert_eq!(value["evidence"]["observed_status_code"], 200);
            assert_eq!(
                value["evidence"]["response_body_hash"],
                format!("{:x}", Sha256::digest(body))
            );
            assert!(value["parse_error"].is_null());
            assert!(!paths
                .iter()
                .any(|p| p.contains("article-a") || p.contains("article-b")));
        }

        #[tokio::test]
        async fn news_sitemap_limit_truncates_without_extra_fetch() {
            const DOC: &[u8] = br#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><url><loc>http://example.test/a</loc></url><url><loc>http://example.test/b</loc></url></urlset>"#;
            let (value, paths, _body) = accepted(DOC, Some(1)).await;
            assert_eq!(paths, ["/news-sitemap.xml"]);
            assert_eq!(value["result_count"], 1);
        }

        #[tokio::test]
        async fn news_sitemap_malformed_body_preserves_evidence_with_parse_error() {
            let malformed: &'static [u8] =
                b"<urlset><url><loc>http://example.test/a</loc></url><url>";
            let (value, paths, _body) = accepted(malformed, None).await;
            assert_eq!(paths, ["/news-sitemap.xml"]);
            assert_eq!(value["result_count"], 0);
            assert_eq!(value["parse_error"]["code"], "news_sitemap_parse_failed");
            assert_eq!(value["evidence"]["observed_status_code"], 200);
        }
    }
}
#[cfg(feature = "news_sitemap")]
pub use news_sitemap_cmd::run as run_news_sitemap;

#[cfg(feature = "robots_sitemap")]
mod robots_sitemap_cmd {
    use super::*;
    use spider::features::robots_sitemap::{
        self, RobotsSitemapDiscoveryResult, RobotsSitemapParseFailure, RobotsSitemapReference,
    };

    #[derive(Serialize)]
    struct RobotsSitemapParseError {
        code: String,
        message: String,
    }

    #[derive(Serialize)]
    struct RobotsSitemapResult {
        robots_url: String,
        result_count: usize,
        sitemaps: Vec<RobotsSitemapReference>,
        evidence: EvidenceBundle,
        parse_error: Option<RobotsSitemapParseError>,
    }

    pub async fn run(url: &str, limit: Option<usize>) -> Result<String, String> {
        let page = fetch_single_page(url).await?;
        let bytes = page
            .get_bytes()
            .ok_or_else(|| "Robots page arrived without a retained representation".to_string())?;
        let raw_text = std::str::from_utf8(bytes).ok().map(str::to_string);
        let evidence = build_evidence(&page, raw_text, false, false);
        let parsed = robots_sitemap::parse(bytes).await;
        let result = shape(url.to_string(), limit, evidence, parsed);
        serde_json::to_string_pretty(&result).map_err(|error| error.to_string())
    }

    fn shape(
        robots_url: String,
        limit: Option<usize>,
        evidence: EvidenceBundle,
        parsed: Result<RobotsSitemapDiscoveryResult, RobotsSitemapParseFailure>,
    ) -> RobotsSitemapResult {
        match parsed {
            Ok(mut discovery) => {
                if let Some(limit) = limit {
                    discovery.sitemaps.truncate(limit);
                }
                RobotsSitemapResult {
                    robots_url,
                    result_count: discovery.sitemaps.len(),
                    sitemaps: discovery.sitemaps,
                    evidence,
                    parse_error: None,
                }
            }
            Err(error) => {
                let code = match error {
                    RobotsSitemapParseFailure::Panicked(_) => "robots_sitemap_parse_failed",
                };
                RobotsSitemapResult {
                    robots_url,
                    result_count: 0,
                    sitemaps: Vec::new(),
                    evidence,
                    parse_error: Some(RobotsSitemapParseError {
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

        type LocalhostHandle = (
            String,
            Arc<Mutex<Vec<String>>>,
            Arc<AtomicBool>,
            std::thread::JoinHandle<()>,
        );

        fn localhost(body: String) -> LocalhostHandle {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = format!("http://{}/robots.txt", listener.local_addr().unwrap());
            let paths = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let paths_thread = paths.clone();
            let stop_thread = stop.clone();
            let handle = std::thread::spawn(move || {
                let body = body.into_bytes();
                while !stop_thread.load(Ordering::Relaxed) {
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
                            paths_thread.lock().unwrap().push(path);
                            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                            stream.write_all(&body).unwrap();
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("localhost server failed: {error}"),
                    }
                }
            });
            (url, paths, stop, handle)
        }

        async fn accepted(
            body: String,
            limit: Option<usize>,
        ) -> (serde_json::Value, Vec<String>, String) {
            let (url, paths, stop, handle) = localhost(body.clone());
            let output = run(&url, limit).await.unwrap();
            let value: serde_json::Value = serde_json::from_str(&output).unwrap();
            stop.store(true, Ordering::Relaxed);
            handle.join().unwrap();
            let log = paths.lock().unwrap().clone();
            (value, log, body)
        }

        #[tokio::test]
        async fn robots_sitemap_is_one_fetch_preserves_order_and_duplicates() {
            let body = "User-agent: *\r\nDisallow: /private\r\n\r\nSitemap: http://example.test/sitemap.xml\r\nSitemap: http://example.test/news.xml\r\nSitemap: http://example.test/sitemap.xml\r\n".to_string();
            let (value, paths, served) = accepted(body, None).await;
            assert_eq!(paths, ["/robots.txt"]);
            assert_eq!(value["result_count"], 3);
            assert_eq!(
                value["sitemaps"],
                serde_json::json!([
                    {"url": "http://example.test/sitemap.xml"},
                    {"url": "http://example.test/news.xml"},
                    {"url": "http://example.test/sitemap.xml"},
                ])
            );
            assert_eq!(value["evidence"]["observed_status_code"], 200);
            assert!(value["evidence"]["retrieved_at"].as_u64().is_some());
            assert_eq!(
                value["evidence"]["response_body_hash"],
                format!("{:x}", Sha256::digest(served.as_bytes()))
            );
            assert!(value["parse_error"].is_null());
            assert!(!paths.iter().any(|p| p.contains("sitemap.xml")
                || p.contains("news.xml")
                || p.contains("private")));
        }

        #[tokio::test]
        async fn robots_sitemap_limit_truncates_without_extra_fetch() {
            let body = "Sitemap: http://example.test/a.xml\nSitemap: http://example.test/b.xml\n"
                .to_string();
            let (value, paths, _served) = accepted(body, Some(1)).await;
            assert_eq!(paths, ["/robots.txt"]);
            assert_eq!(value["result_count"], 1);
            assert_eq!(value["sitemaps"][0]["url"], "http://example.test/a.xml");
        }

        #[tokio::test]
        async fn robots_sitemap_empty_body_succeeds_with_no_sitemaps() {
            let (value, paths, _served) = accepted(String::new(), None).await;
            assert_eq!(paths, ["/robots.txt"]);
            assert_eq!(value["result_count"], 0);
            assert!(value["parse_error"].is_null());
        }
    }
}
#[cfg(feature = "robots_sitemap")]
pub use robots_sitemap_cmd::run as run_robots_sitemap;
