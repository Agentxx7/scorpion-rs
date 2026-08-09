use rmcp::schemars;
use serde::{Deserialize, Serialize};
use spider::features::feed::{FeedDiscoveryResult, FeedParseFailure};
use spider::features::source::SourceItem;

/// Input for `spider_feed_read`.
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
pub struct FeedReadParams {
    /// Direct RSS or Atom feed URL.
    pub url: String,
    /// Return only the first N entries in provider order.
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
struct FeedParseError {
    code: String,
    message: String,
}

#[derive(Clone, Debug, Serialize)]
struct FeedReadResult {
    feed_url: String,
    feed_type: Option<String>,
    feed_title: Option<String>,
    feed_authors: Vec<String>,
    language: Option<String>,
    result_count: usize,
    entries: Vec<SourceItem>,
    evidence: crate::evidence::EvidenceBundle,
    parse_error: Option<FeedParseError>,
}

/// Fetch and parse one RSS or Atom feed while preserving retrieval evidence.
pub async fn run(params: FeedReadParams) -> Result<String, String> {
    let page = super::fetch_single_page(&params.url).await?;
    let bytes = page
        .get_bytes()
        .ok_or_else(|| "Feed page arrived without a retained representation".to_string())?;
    let raw_xml = std::str::from_utf8(bytes).ok().map(str::to_string);
    let evidence = crate::evidence::build_evidence(&page, raw_xml, false, false);
    let parsed = spider::features::feed::parse(bytes, &params.url).await;
    let result = shape_result(params.url, params.limit, evidence, parsed);
    serde_json::to_string_pretty(&result).map_err(|error| error.to_string())
}

fn shape_result(
    feed_url: String,
    limit: Option<usize>,
    evidence: crate::evidence::EvidenceBundle,
    parsed: Result<FeedDiscoveryResult, FeedParseFailure>,
) -> FeedReadResult {
    match parsed {
        Ok(mut feed) => {
            if let Some(limit) = limit {
                feed.entries.truncate(limit);
            }
            FeedReadResult {
                feed_url,
                feed_type: Some(feed.feed_type),
                feed_title: feed.feed_title,
                feed_authors: feed.feed_authors,
                language: feed.language,
                result_count: feed.entries.len(),
                entries: feed.entries,
                evidence,
                parse_error: None,
            }
        }
        Err(error) => {
            let code = match error {
                FeedParseFailure::NotFeed(_) => "not_feed",
                FeedParseFailure::Parse(_) | FeedParseFailure::Panicked(_) => "feed_parse_failed",
            };
            FeedReadResult {
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    const RSS: &str = r#"<rss version="2.0"><channel><title>Local RSS</title><item><guid>one</guid><link>http://127.0.0.1/article-one</link><title>One</title><enclosure url="http://127.0.0.1/media.mp3" type="audio/mpeg"/></item><item><guid>two</guid><link>http://127.0.0.1/article-two</link><title>Two</title></item></channel></rss>"#;
    const ATOM: &str = r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>Local Atom</title><id>feed</id><updated>2024-01-01T00:00:00Z</updated><entry><title>One</title><id>one</id><link rel="self" href="http://127.0.0.1/self"/><link rel="alternate" href="http://127.0.0.1/article"/><link rel="enclosure" href="http://127.0.0.1/media.mp4" type="video/mp4"/><author><name>First</name></author><author><name>Second</name></author><published>2024-01-01T00:00:00Z</published><updated>2024-01-02T00:00:00Z</updated></entry></feed>"#;

    fn localhost(
        body: &[u8],
    ) -> (
        String,
        Arc<AtomicUsize>,
        Arc<AtomicBool>,
        std::thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/feed", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let requests_thread = requests.clone();
        let stop_thread = stop.clone();
        let body = body.to_vec();
        let handle = std::thread::spawn(move || {
            while !stop_thread.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        requests_thread.fetch_add(1, Ordering::Relaxed);
                        let mut request = [0_u8; 2048];
                        let _ = stream.read(&mut request);
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                        stream.write_all(&body).unwrap();
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

    async fn accepted(body: &[u8], limit: Option<usize>) -> (serde_json::Value, usize) {
        let (url, requests, stop, handle) = localhost(body);
        let output = run(FeedReadParams { url, limit }).await.unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        (value, requests.load(Ordering::Relaxed))
    }

    #[tokio::test]
    async fn localhost_rss_is_one_fetch_exact_evidence_and_provider_order() {
        let (value, requests) = accepted(RSS.as_bytes(), Some(2)).await;
        assert_eq!(requests, 1);
        assert_eq!(value["feed_type"], "rss2");
        assert_eq!(value["result_count"], 2);
        assert_eq!(value["entries"][0]["source_item_id"], "one");
        assert_eq!(value["entries"][1]["source_item_id"], "two");
        assert_eq!(
            value["entries"][0]["media_references"][0]["url"],
            "http://127.0.0.1/media.mp3"
        );
        assert_eq!(value["evidence"]["observed_status_code"], 200);
        assert!(value["evidence"]["retrieved_at"].as_u64().is_some());
        assert_eq!(value["evidence"]["content"], RSS);
        assert_eq!(
            value["evidence"]["response_body_hash"],
            format!("{:x}", Sha256::digest(RSS.as_bytes()))
        );
        assert!(value["parse_error"].is_null());
    }

    #[tokio::test]
    async fn localhost_atom_selects_alternate_and_does_not_fetch_discovered_urls() {
        let (value, requests) = accepted(ATOM.as_bytes(), None).await;
        assert_eq!(requests, 1);
        assert_eq!(value["feed_type"], "atom");
        assert_eq!(value["entries"][0]["url"], "http://127.0.0.1/article");
        assert_eq!(
            value["entries"][0]["authors"],
            serde_json::json!(["First", "Second"])
        );
        assert_ne!(
            value["entries"][0]["published_at"],
            value["entries"][0]["updated_at"]
        );
        assert_eq!(
            value["entries"][0]["media_references"][0]["url"],
            "http://127.0.0.1/media.mp4"
        );
        assert_eq!(
            value["evidence"]["response_body_hash"],
            format!("{:x}", Sha256::digest(ATOM.as_bytes()))
        );
    }

    #[tokio::test]
    async fn malformed_feed_preserves_successful_retrieval_evidence() {
        let malformed = b"<rss><channel><item>";
        let (value, requests) = accepted(malformed, None).await;
        assert_eq!(requests, 1);
        assert_eq!(value["result_count"], 0);
        assert_eq!(value["entries"], serde_json::json!([]));
        assert_eq!(value["parse_error"]["code"], "feed_parse_failed");
        assert!(!value["parse_error"]["message"].as_str().unwrap().is_empty());
        assert_eq!(
            value["evidence"]["content"],
            std::str::from_utf8(malformed).unwrap()
        );
        assert_eq!(value["evidence"]["observed_status_code"], 200);
    }

    #[test]
    fn input_schema_has_only_url_and_limit() {
        let schema = schemars::schema_for!(FeedReadParams);
        let value = serde_json::to_value(schema).unwrap();
        let properties = &value["properties"];
        assert!(properties.get("url").is_some());
        assert!(properties.get("limit").is_some());
        assert!(properties.get("query").is_none());
        assert!(properties.get("source_hint").is_none());
        assert!(properties.get("language").is_none());
    }

    fn discovery(entries: usize) -> FeedDiscoveryResult {
        FeedDiscoveryResult {
            feed_type: "rss2".into(),
            feed_title: Some("Feed".into()),
            feed_authors: Vec::new(),
            language: None,
            entries: (0..entries)
                .map(|index| SourceItem {
                    source_type: "feed".into(),
                    source_item_id: Some(index.to_string()),
                    discovered_via: "https://example.test/feed".into(),
                    ..Default::default()
                })
                .collect(),
        }
    }

    #[test]
    fn pure_result_shaping_distinguishes_valid_empty_limited_and_parse_failure() {
        let valid = shape_result(
            "https://example.test/feed".into(),
            None,
            Default::default(),
            Ok(discovery(2)),
        );
        assert_eq!(valid.result_count, 2);
        assert!(valid.parse_error.is_none());

        let empty = shape_result(
            "https://example.test/feed".into(),
            None,
            Default::default(),
            Ok(discovery(0)),
        );
        assert_eq!(empty.result_count, 0);
        assert!(empty.parse_error.is_none());

        let limited = shape_result(
            "https://example.test/feed".into(),
            Some(1),
            Default::default(),
            Ok(discovery(3)),
        );
        assert_eq!(limited.result_count, 1);
        assert_eq!(limited.entries[0].source_item_id.as_deref(), Some("0"));

        let failed = shape_result(
            "https://example.test/feed".into(),
            None,
            crate::evidence::EvidenceBundle {
                observed_status_code: Some(200),
                ..Default::default()
            },
            Err(FeedParseFailure::NotFeed("not a feed".into())),
        );
        assert_eq!(failed.result_count, 0);
        assert!(failed.entries.is_empty());
        assert_eq!(failed.evidence.observed_status_code, Some(200));
        assert_eq!(failed.parse_error.unwrap().code, "not_feed");
    }
}
