//! `spider_media_search` — exposes SearXNG's category-restricted video and
//! image search (SCORPION.md §9.3 `VideoResult`/`ImageResult`) through MCP.
//! Same posture as `spider_search`: request/response translation only, no
//! bespoke search engine, and — per SCORPION.md §9.9/§12 — no automatic
//! fetch/crawl/browser invocation of any discovered URL. This tool returns
//! candidates only; retrieval remains a separate, deliberate step.

use rmcp::schemars;
use serde::Deserialize;

#[derive(Deserialize, schemars::JsonSchema)]
pub struct MediaSearchParams {
    /// The search query.
    #[cfg_attr(not(feature = "search_searxng"), allow(dead_code))]
    pub query: String,
    /// Media type to search for. Currently supported: "video", "image".
    #[cfg_attr(not(feature = "search_searxng"), allow(dead_code))]
    pub media_type: String,
    /// Search provider to use. Currently supported: "searxng".
    pub provider: String,
    /// Base URL of a self-hosted SearXNG instance (e.g. "http://localhost:8080").
    /// Required when provider = "searxng" — Scorpion does not assume or
    /// default to any public SearXNG instance.
    #[cfg_attr(not(feature = "search_searxng"), allow(dead_code))]
    pub base_url: Option<String>,
    /// Maximum number of results to return.
    #[cfg_attr(not(feature = "search_searxng"), allow(dead_code))]
    pub limit: Option<usize>,
    /// Language code (e.g. "en").
    #[cfg_attr(not(feature = "search_searxng"), allow(dead_code))]
    pub language: Option<String>,
}

/// Map MCP request params into Spider's `SearchOptions`. Pure — no network —
/// so directly unit-testable. Identical mapping to `spider_search`'s
/// `build_search_options`, kept local to avoid coupling the two tools'
/// evolution together.
#[cfg(feature = "search_searxng")]
fn build_search_options(params: &MediaSearchParams) -> spider::features::search::SearchOptions {
    let mut options = spider::features::search::SearchOptions::new();
    if let Some(limit) = params.limit {
        options = options.with_limit(limit);
    }
    if let Some(ref language) = params.language {
        options = options.with_language(language.clone());
    }
    options
}

/// Render `SearxngVideoResult`s into the deterministic MCP JSON shape. Pure
/// — no network. Absent optional fields serialize as JSON `null` (via
/// `Option`'s `Serialize` impl) rather than being fabricated or omitted.
#[cfg(feature = "search_searxng")]
fn render_video_results(
    provider: &str,
    query: &str,
    results: &[spider::features::search_providers::SearxngVideoResult],
) -> serde_json::Value {
    let results_json: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            serde_json::json!({
                "title": r.title,
                "url": r.url,
                "thumbnail_url": r.thumbnail_url,
                "description": r.description,
                "creator_or_channel": r.creator_or_channel,
                "published_at": r.published_at,
                "duration": r.duration,
                "source": r.source,
            })
        })
        .collect();

    serde_json::json!({
        "query": query,
        "media_type": "video",
        "provider": provider,
        "result_count": results.len(),
        "results": results_json,
    })
}

/// Render `SearxngImageResult`s into the deterministic MCP JSON shape. Same
/// null-not-fabricated posture as [`render_video_results`].
#[cfg(feature = "search_searxng")]
fn render_image_results(
    provider: &str,
    query: &str,
    results: &[spider::features::search_providers::SearxngImageResult],
) -> serde_json::Value {
    let results_json: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            serde_json::json!({
                "title": r.title,
                "image_url": r.image_url,
                "thumbnail_url": r.thumbnail_url,
                "source_page": r.source_page,
                "width": r.width,
                "height": r.height,
                "mime_type": r.mime_type,
                "description": r.description,
            })
        })
        .collect();

    serde_json::json!({
        "query": query,
        "media_type": "image",
        "provider": provider,
        "result_count": results.len(),
        "results": results_json,
    })
}

/// Resolve `params.provider`/`params.media_type` into a live category
/// search, or an explicit error. Never fetches, crawls, or opens any
/// discovered URL — this tool returns candidates only (SCORPION.md §9.9).
#[cfg(feature = "search_searxng")]
pub async fn run(params: MediaSearchParams) -> Result<String, String> {
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
            "provider=\"searxng\" requires a non-empty base_url pointing at a \
             self-hosted SearXNG instance — no public instance is assumed."
                .to_string()
        })?;

    let options = build_search_options(&params);
    let provider = resolve_searxng_provider(Some(base_url)).map_err(|e| e.to_string())?;

    let output = match params.media_type.as_str() {
        "video" => {
            let results = provider
                .search_videos(&params.query, &options)
                .await
                .map_err(|e| e.to_string())?;
            render_video_results(&params.provider, &params.query, &results)
        }
        "image" => {
            let results = provider
                .search_images(&params.query, &options)
                .await
                .map_err(|e| e.to_string())?;
            render_image_results(&params.provider, &params.query, &results)
        }
        other => {
            return Err(format!(
                "Unknown or unsupported media_type \"{other}\". Currently supported: \"video\", \"image\"."
            ));
        }
    };

    serde_json::to_string_pretty(&output).map_err(|e| e.to_string())
}

/// No-op degrade when `search_searxng` isn't compiled in — explicit error,
/// matching `spider_search`'s and `apply_screenshot_options`'s degrade
/// pattern, never a silent success.
#[cfg(not(feature = "search_searxng"))]
pub async fn run(params: MediaSearchParams) -> Result<String, String> {
    Err(format!(
        "Media search provider \"{}\" is not available: this build was compiled without the \
         search_searxng feature.",
        params.provider
    ))
}

#[cfg(all(test, feature = "search_searxng"))]
mod tests {
    use super::*;
    use spider::features::search_providers::{SearxngImageResult, SearxngVideoResult};

    fn params(media_type: &str, provider: &str, base_url: Option<&str>) -> MediaSearchParams {
        MediaSearchParams {
            query: "dark sci-fi".to_string(),
            media_type: media_type.to_string(),
            provider: provider.to_string(),
            base_url: base_url.map(|s| s.to_string()),
            limit: None,
            language: None,
        }
    }

    /// 2. Video request maps correctly.
    #[test]
    fn maps_limit_and_language_into_search_options() {
        let mut p = params("video", "searxng", Some("http://localhost:8080"));
        p.limit = Some(5);
        p.language = Some("en".to_string());

        let options = build_search_options(&p);
        assert_eq!(options.limit, Some(5));
        assert_eq!(options.language.as_deref(), Some("en"));
    }

    #[test]
    fn omitted_optional_params_stay_none_in_options() {
        let p = params("video", "searxng", Some("http://localhost:8080"));
        let options = build_search_options(&p);
        assert_eq!(options.limit, None);
        assert_eq!(options.language, None);
    }

    /// 2/4. Video request renders correctly, ordering preserved.
    #[test]
    fn render_video_results_preserves_order_and_fields() {
        let results = vec![
            SearxngVideoResult {
                title: "First".to_string(),
                url: "https://a.example/v".to_string(),
                thumbnail_url: Some("https://a.example/thumb.jpg".to_string()),
                description: Some("d1".to_string()),
                creator_or_channel: Some("Channel A".to_string()),
                published_at: Some("2024-01-01".to_string()),
                duration: Some("1:00".to_string()),
                source: Some("youtube".to_string()),
            },
            SearxngVideoResult {
                title: "Second".to_string(),
                url: "https://b.example/v".to_string(),
                ..Default::default()
            },
        ];

        let v = render_video_results("searxng", "dark sci-fi", &results);
        assert_eq!(v["query"], "dark sci-fi");
        assert_eq!(v["media_type"], "video");
        assert_eq!(v["provider"], "searxng");
        assert_eq!(v["result_count"], 2);
        assert_eq!(v["results"][0]["title"], "First");
        assert_eq!(v["results"][0]["url"], "https://a.example/v");
        assert_eq!(v["results"][0]["creator_or_channel"], "Channel A");
        assert_eq!(v["results"][0]["source"], "youtube");
        assert_eq!(v["results"][1]["title"], "Second");
    }

    /// 5 (video). Absent optional fields remain null, not fabricated.
    #[test]
    fn render_video_results_absent_optional_fields_are_null() {
        let results = vec![SearxngVideoResult {
            title: "Bare".to_string(),
            url: "https://bare.example/v".to_string(),
            ..Default::default()
        }];
        let v = render_video_results("searxng", "q", &results);
        assert!(v["results"][0]["thumbnail_url"].is_null());
        assert!(v["results"][0]["description"].is_null());
        assert!(v["results"][0]["creator_or_channel"].is_null());
        assert!(v["results"][0]["published_at"].is_null());
        assert!(v["results"][0]["duration"].is_null());
        assert!(v["results"][0]["source"].is_null());
    }

    /// 3/4. Image request renders correctly, ordering preserved.
    #[test]
    fn render_image_results_preserves_order_and_fields() {
        let results = vec![
            SearxngImageResult {
                title: Some("A Cat".to_string()),
                image_url: "https://a.example/cat.png".to_string(),
                thumbnail_url: Some("https://a.example/cat_t.png".to_string()),
                source_page: Some("https://a.example/page".to_string()),
                width: Some(1920),
                height: Some(1080),
                mime_type: Some("image/png".to_string()),
                description: Some("d1".to_string()),
            },
            SearxngImageResult {
                image_url: "https://b.example/second.jpg".to_string(),
                ..Default::default()
            },
        ];

        let v = render_image_results("searxng", "dark sci-fi", &results);
        assert_eq!(v["media_type"], "image");
        assert_eq!(v["result_count"], 2);
        assert_eq!(v["results"][0]["title"], "A Cat");
        assert_eq!(v["results"][0]["image_url"], "https://a.example/cat.png");
        assert_eq!(v["results"][0]["width"], 1920);
        assert_eq!(v["results"][0]["height"], 1080);
        assert_eq!(v["results"][0]["mime_type"], "image/png");
        assert_eq!(v["results"][1]["image_url"], "https://b.example/second.jpg");
    }

    /// 5 (image). Absent optional fields remain null, not fabricated.
    #[test]
    fn render_image_results_absent_optional_fields_are_null() {
        let results = vec![SearxngImageResult {
            image_url: "https://bare.example/i.png".to_string(),
            ..Default::default()
        }];
        let v = render_image_results("searxng", "q", &results);
        assert!(v["results"][0]["title"].is_null());
        assert!(v["results"][0]["thumbnail_url"].is_null());
        assert!(v["results"][0]["source_page"].is_null());
        assert!(v["results"][0]["width"].is_null());
        assert!(v["results"][0]["height"].is_null());
        assert!(v["results"][0]["mime_type"].is_null());
        assert!(v["results"][0]["description"].is_null());
    }

    /// 6. Unknown media_type fails explicitly.
    #[tokio::test]
    async fn unknown_media_type_fails_explicitly() {
        let p = params("audio", "searxng", Some("http://localhost:8080"));
        let result = run(p).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("audio"));
    }

    /// 7. Unknown provider fails explicitly.
    #[tokio::test]
    async fn unknown_provider_fails_explicitly() {
        let p = params("video", "not-a-real-provider", None);
        let result = run(p).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not-a-real-provider"));
    }

    /// 8. Missing/blank base_url fails explicitly.
    #[tokio::test]
    async fn searxng_missing_base_url_fails_explicitly() {
        let p = params("video", "searxng", None);
        let result = run(p).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_lowercase().contains("base_url"));
    }

    #[tokio::test]
    async fn searxng_blank_base_url_fails_explicitly() {
        let p = params("image", "searxng", Some("   "));
        let result = run(p).await;
        assert!(result.is_err());
    }

    /// 9. Provider/network failure is not converted into empty success.
    #[tokio::test]
    async fn unreachable_base_url_fails_explicitly_not_empty_success() {
        // Port chosen to be almost certainly unbound in any test environment.
        let p = params("video", "searxng", Some("http://127.0.0.1:1"));
        let result = run(p).await;
        assert!(
            result.is_err(),
            "a real connection failure must surface as an explicit error"
        );
    }
}
