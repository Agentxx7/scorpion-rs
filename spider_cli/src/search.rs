//! CLI adapter for Spider's canonical SearXNG search provider.
//!
//! This module only translates CLI parameters and canonical provider results
//! to JSON. It owns no HTTP-provider implementation and never fetches any
//! result URL.

use crate::options::sub_command::SearchCategory;
use spider::features::search::{resolve_search_provider, resolve_searxng_provider, SearchOptions};

#[derive(Clone, Debug)]
pub struct SearchParams {
    pub query: String,
    pub provider: String,
    pub base_url: Option<String>,
    pub category: SearchCategory,
    pub limit: Option<usize>,
    pub language: Option<String>,
}

fn build_search_options(params: &SearchParams) -> SearchOptions {
    let mut options = SearchOptions::new();
    if let Some(limit) = params.limit {
        options = options.with_limit(limit);
    }
    if let Some(ref language) = params.language {
        options = options.with_language(language.clone());
    }
    options
}

fn base_url(params: &SearchParams) -> Result<&str, String> {
    if params.provider != "searxng" {
        return Err(format!(
            "Unknown or unsupported search provider \"{}\". Currently supported: \"searxng\".",
            params.provider
        ));
    }

    params
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "provider=\"searxng\" requires a non-empty base_url pointing at a self-hosted \
             SearXNG instance — no public instance is assumed."
                .to_string()
        })
}

pub async fn run(params: SearchParams) -> Result<String, String> {
    if !matches!(params.category, SearchCategory::Web) {
        let provider =
            resolve_searxng_provider(Some(base_url(&params)?)).map_err(|e| e.to_string())?;
        let options = build_search_options(&params);
        let output = match params.category {
            SearchCategory::News => {
                let results = provider
                    .search_news(&params.query, &options)
                    .await
                    .map_err(|e| e.to_string())?;
                serde_json::json!({"query": params.query, "category": "news", "provider": params.provider, "result_count": results.len(), "results": results})
            }
            SearchCategory::Image => {
                let results = provider
                    .search_images(&params.query, &options)
                    .await
                    .map_err(|e| e.to_string())?;
                serde_json::json!({"query": params.query, "category": "image", "provider": params.provider, "result_count": results.len(), "results": results})
            }
            SearchCategory::Video => {
                let results = provider
                    .search_videos(&params.query, &options)
                    .await
                    .map_err(|e| e.to_string())?;
                serde_json::json!({"query": params.query, "category": "video", "provider": params.provider, "result_count": results.len(), "results": results})
            }
            SearchCategory::Web => unreachable!(),
        };
        return serde_json::to_string_pretty(&output).map_err(|e| e.to_string());
    }
    let provider = resolve_search_provider(
        Some(&params.provider),
        Some(base_url(&params)?),
        None,
        None,
        None,
    )
    .map_err(|error| error.to_string())?
    .1;
    let options = build_search_options(&params);

    let output = match params.category {
        SearchCategory::Web => {
            let results = provider
                .search(&params.query, &options)
                .await
                .map_err(|error| error.to_string())?;
            let items = results
                .results
                .iter()
                .map(|result| {
                    serde_json::json!({
                        "position": result.position,
                        "title": result.title,
                        "url": result.url,
                        "snippet": result.snippet,
                        "date": result.date,
                        "score": result.score,
                    })
                })
                .collect::<Vec<_>>();
            serde_json::json!({
                "query": results.query,
                "category": "web",
                "provider": params.provider,
                "result_count": items.len(),
                "results": items,
                "total_results": results.total_results,
                "metadata": results.metadata,
            })
        }
        SearchCategory::News | SearchCategory::Image | SearchCategory::Video => unreachable!(),
    };

    serde_json::to_string_pretty(&output).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn params(category: SearchCategory) -> SearchParams {
        SearchParams {
            query: "openai research".into(),
            provider: "searxng".into(),
            base_url: Some("http://127.0.0.1:1".into()),
            category,
            limit: None,
            language: None,
        }
    }

    #[test]
    fn maps_limit_and_languages_without_translation() {
        let mut input = params(SearchCategory::Web);
        input.limit = Some(3);
        input.language = Some("sv".into());
        let options = build_search_options(&input);
        assert_eq!(options.limit, Some(3));
        assert_eq!(options.language.as_deref(), Some("sv"));

        input.language = Some("en".into());
        assert_eq!(build_search_options(&input).language.as_deref(), Some("en"));
    }

    #[tokio::test]
    async fn configuration_and_connection_errors_are_explicit() {
        let mut input = params(SearchCategory::Web);
        input.base_url = None;
        assert!(run(input).await.unwrap_err().contains("base_url"));

        let mut input = params(SearchCategory::Web);
        input.provider = "unsupported".into();
        assert!(run(input).await.unwrap_err().contains("unsupported"));

        let mut input = params(SearchCategory::Web);
        input.base_url = Some("not a url".into());
        assert!(run(input)
            .await
            .unwrap_err()
            .contains("Invalid SearXNG base URL"));

        assert!(run(params(SearchCategory::Web)).await.is_err());
    }

    async fn serve_responses(
        bodies: Vec<(&'static str, &'static str)>,
    ) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server_requests = requests.clone();
        let server = tokio::spawn(async move {
            for (status, body) in bodies {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = vec![0; 8192];
                let count = stream.read(&mut bytes).await.unwrap();
                let request = String::from_utf8_lossy(&bytes[..count]);
                server_requests
                    .lock()
                    .unwrap()
                    .push(request.lines().next().unwrap().to_string());
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        (format!("http://{address}"), requests, server)
    }

    #[tokio::test]
    async fn all_categories_use_one_canonical_request_and_never_fetch_results() {
        const BODY: &str = r#"{"results":[
            {"title":"First","url":"http://127.0.0.1/result-a","content":"Snippet","publishedDate":"opaque","author":"Author","thumbnail":"http://127.0.0.1/thumb.jpg","engine":"engine-a","score":0.75,"img_src":"http://127.0.0.1/image.jpg","resolution":"640 x 480","img_format":"png","length":"1:23"},
            {"title":"Second","url":"http://127.0.0.1/result-b","img_src":"http://127.0.0.1/image-b.jpg"}
        ]}"#;
        let (base_url, requests, server) = serve_responses(vec![
            ("200 OK", BODY),
            ("200 OK", BODY),
            ("200 OK", BODY),
            ("200 OK", BODY),
        ])
        .await;

        for (category, expected, language) in [
            (SearchCategory::Web, "web", "en"),
            (SearchCategory::News, "news", "sv"),
            (SearchCategory::Image, "image", "en"),
            (SearchCategory::Video, "video", "sv"),
        ] {
            let mut input = params(category);
            input.base_url = Some(base_url.clone());
            input.limit = Some(1);
            input.language = Some(language.into());
            let output: serde_json::Value =
                serde_json::from_str(&run(input).await.unwrap()).unwrap();
            assert_eq!(output["category"], expected);
            assert_eq!(output["provider"], "searxng");
            assert_eq!(output["result_count"], 1);
            assert_eq!(output["results"].as_array().unwrap().len(), 1);
            match category {
                SearchCategory::Web => {
                    assert_eq!(output["results"][0]["position"], 1);
                    assert_eq!(output["results"][0]["snippet"], "Snippet");
                }
                SearchCategory::News => {
                    assert_eq!(output["results"][0]["snippet"], "Snippet");
                    assert_eq!(output["results"][0]["engine"], "engine-a");
                }
                SearchCategory::Image => {
                    assert_eq!(
                        output["results"][0]["image_url"],
                        "http://127.0.0.1/image.jpg"
                    );
                    assert_eq!(output["results"][0]["width"], 640);
                    assert_eq!(output["results"][0]["height"], 480);
                }
                SearchCategory::Video => {
                    assert_eq!(output["results"][0]["description"], "Snippet");
                    assert_eq!(output["results"][0]["duration"], "1:23");
                }
            }
        }

        server.await.unwrap();
        let log = requests.lock().unwrap();
        assert_eq!(log.len(), 4, "one request per search operation: {log:?}");
        for request in log.iter() {
            assert!(request.starts_with("GET /search?"), "{request}");
            assert!(request.contains("q=openai+research"), "{request}");
            assert!(request.contains("format=json"), "{request}");
            assert!(!request.contains("limit="), "{request}");
            assert!(!request.contains("result-a"), "{request}");
            assert!(!request.contains("thumb.jpg"), "{request}");
        }
        assert!(!log[0].contains("categories="), "{}", log[0]);
        assert!(log[0].contains("language=en"), "{}", log[0]);
        assert!(log[1].contains("categories=news"), "{}", log[1]);
        assert!(log[1].contains("language=sv"), "{}", log[1]);
        assert!(log[2].contains("categories=images"), "{}", log[2]);
        assert!(log[3].contains("categories=videos"), "{}", log[3]);
    }

    #[tokio::test]
    async fn zero_results_are_valid_json_success() {
        let (base_url, _requests, server) =
            serve_responses(vec![("200 OK", r#"{"results":[]}"#)]).await;
        let mut input = params(SearchCategory::Web);
        input.base_url = Some(base_url);
        let output: serde_json::Value = serde_json::from_str(&run(input).await.unwrap()).unwrap();
        server.await.unwrap();
        assert_eq!(output["result_count"], 0);
        assert_eq!(output["results"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn provider_http_and_malformed_response_errors_are_explicit() {
        let (base_url, _requests, server) = serve_responses(vec![
            ("500 Internal Server Error", "provider failed"),
            ("200 OK", "not json"),
        ])
        .await;

        let mut http_error = params(SearchCategory::Web);
        http_error.base_url = Some(base_url.clone());
        assert!(run(http_error).await.unwrap_err().contains("HTTP 500"));

        let mut malformed = params(SearchCategory::Web);
        malformed.base_url = Some(base_url);
        assert!(run(malformed).await.unwrap_err().contains("parse response"));
        server.await.unwrap();
    }
}
