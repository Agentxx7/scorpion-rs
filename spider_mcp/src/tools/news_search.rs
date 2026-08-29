//! `spider_news_search` exposes SearXNG news discovery through MCP. It only
//! returns candidates and never fetches article or thumbnail URLs.

use rmcp::schemars;
use serde::Deserialize;

#[derive(Deserialize, schemars::JsonSchema)]
pub struct NewsSearchParams {
    /// The news query.
    #[cfg_attr(not(feature = "search_searxng"), allow(dead_code))]
    pub query: String,
    /// Search provider to use. Currently supported: "searxng".
    pub provider: String,
    /// Base URL of the caller's self-hosted SearXNG instance.
    #[cfg_attr(not(feature = "search_searxng"), allow(dead_code))]
    pub base_url: Option<String>,
    /// Maximum number of results to return.
    #[cfg_attr(not(feature = "search_searxng"), allow(dead_code))]
    pub limit: Option<usize>,
    /// Language code passed through to SearXNG (for example, "en").
    #[cfg_attr(not(feature = "search_searxng"), allow(dead_code))]
    pub language: Option<String>,
}

#[cfg(feature = "search_searxng")]
fn build_search_options(params: &NewsSearchParams) -> spider::features::search::SearchOptions {
    let mut options = spider::features::search::SearchOptions::new();
    if let Some(limit) = params.limit {
        options = options.with_limit(limit);
    }
    if let Some(ref language) = params.language {
        options = options.with_language(language.clone());
    }
    options
}

#[cfg(feature = "search_searxng")]
fn render_results(
    provider: &str,
    query: &str,
    results: &[spider::features::search_providers::SearxngNewsResult],
) -> serde_json::Value {
    let results = results
        .iter()
        .map(|result| {
            serde_json::json!({
                "title": result.title,
                "url": result.url,
                "snippet": result.snippet,
                "published_at": result.published_at,
                "author": result.author,
                "thumbnail_url": result.thumbnail_url,
                "engine": result.engine,
                "score": result.score,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "query": query,
        "provider": provider,
        "result_count": results.len(),
        "results": results,
    })
}

#[cfg(feature = "search_searxng")]
pub async fn run(params: NewsSearchParams) -> Result<String, String> {
    use spider::features::search::resolve_searxng_provider;

    if params.provider != "searxng" {
        return Err(format!(
            "Unknown or unsupported search provider \"{}\". Currently supported: \"searxng\".",
            params.provider
        ));
    }
    let base_url = params
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            "provider=\"searxng\" requires a non-empty base_url pointing at a self-hosted \
             SearXNG instance — no public instance is assumed."
                .to_string()
        })?;
    let provider = resolve_searxng_provider(Some(base_url)).map_err(|e| e.to_string())?;
    let results = provider
        .search_news(&params.query, &build_search_options(&params))
        .await
        .map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&render_results(&params.provider, &params.query, &results))
        .map_err(|error| error.to_string())
}

#[cfg(not(feature = "search_searxng"))]
pub async fn run(params: NewsSearchParams) -> Result<String, String> {
    Err(format!(
        "News search provider \"{}\" is not available: this build was compiled without the \
         search_searxng feature.",
        params.provider
    ))
}

#[cfg(all(test, feature = "search_searxng"))]
mod tests {
    use super::*;
    use spider::features::search_providers::SearxngNewsResult;

    fn params(provider: &str, base_url: Option<String>) -> NewsSearchParams {
        NewsSearchParams {
            query: "openai".to_string(),
            provider: provider.to_string(),
            base_url,
            limit: None,
            language: None,
        }
    }

    #[test]
    fn output_preserves_all_fields_and_nulls() {
        let results = vec![
            SearxngNewsResult {
                title: "Headline".into(),
                url: "https://example.test/a".into(),
                snippet: Some("Summary".into()),
                published_at: Some("not a normalized date".into()),
                author: Some("Writer".into()),
                thumbnail_url: Some("https://example.test/t.jpg".into()),
                engine: Some("raw-engine".into()),
                score: Some(0.75_f32),
            },
            SearxngNewsResult {
                title: "Bare".into(),
                url: "https://example.test/b".into(),
                ..Default::default()
            },
        ];
        let output = render_results("searxng", "openai", &results);
        assert_eq!(output["provider"], "searxng");
        assert_eq!(output["results"][0]["snippet"], "Summary");
        assert!(output["results"][0].get("description").is_none());
        assert_eq!(output["results"][0]["engine"], "raw-engine");
        assert_eq!(
            output["results"][0]["published_at"],
            "not a normalized date"
        );
        assert!(output["results"][1]["author"].is_null());
        assert!(output["results"][1]["thumbnail_url"].is_null());
        assert!(output["results"][0]["score"].is_number());
        assert_eq!(output["results"][0]["score"].as_f64(), Some(0.75));
        assert!(output["results"][1]["score"].is_null());
    }

    #[tokio::test]
    async fn errors_are_explicit() {
        assert!(run(params("unknown", None))
            .await
            .unwrap_err()
            .contains("unknown"));
        assert!(run(params("searxng", None))
            .await
            .unwrap_err()
            .contains("base_url"));
        assert!(run(params("searxng", Some("http://127.0.0.1:1".into())))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn actual_tool_path_requests_only_search_with_news_query_and_language() {
        use std::sync::{Arc, Mutex};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let server_requests = requests.clone();
        let server = tokio::spawn(async move {
            while let Ok(Ok((mut stream, _))) =
                tokio::time::timeout(std::time::Duration::from_millis(500), listener.accept()).await
            {
                let mut bytes = vec![0; 8192];
                let count = stream.read(&mut bytes).await.unwrap();
                let request = String::from_utf8_lossy(&bytes[..count]);
                server_requests
                    .lock()
                    .unwrap()
                    .push(request.lines().next().unwrap().to_string());
                let body = format!(
                    r#"{{"results":[
                    {{"title":"A","url":"http://{address}/article-a","thumbnail":"http://{address}/thumb.jpg","publishedDate":"opaque date","engine":"engine-a"}},
                    {{"title":"B","url":"http://{address}/article-b"}}
                ]}}"#
                );
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let mut input = params("searxng", Some(format!("http://{address}")));
        input.language = Some("en".into());
        let output: serde_json::Value = serde_json::from_str(&run(input).await.unwrap()).unwrap();
        assert_eq!(output["result_count"], 2);
        server.await.unwrap();
        let log = requests.lock().unwrap();
        assert_eq!(log.len(), 1, "only /search may be requested: {log:?}");
        let request_line = &log[0];
        assert!(request_line.starts_with("GET /search?"), "{request_line}");
        assert!(request_line.contains("q=openai"), "{request_line}");
        assert!(request_line.contains("format=json"), "{request_line}");
        assert!(request_line.contains("categories=news"), "{request_line}");
        assert!(request_line.contains("language=en"), "{request_line}");
        assert!(!log.iter().any(|line| line.contains("/article-a")
            || line.contains("/article-b")
            || line.contains("/thumb.jpg")));
    }
}
