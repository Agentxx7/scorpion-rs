//! `spider_search` — exposes Scorpion's existing `SearchProvider`
//! abstraction (`spider::features::search`) through MCP, starting with the
//! self-hosted SearXNG provider. No bespoke search engine here: this is
//! request/response translation only, over the exact same
//! `SearchProvider`/`SearchOptions`/`SearchResults` types the `spider` crate
//! already defines and tests.
//!
//! Does not automatically fetch or crawl any result URL — the tool returns
//! enough information (title/url/snippet/date/score) for a downstream agent
//! to decide what to do next.

use rmcp::schemars;
use serde::Deserialize;

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    /// The search query.
    #[cfg_attr(not(feature = "search_searxng"), allow(dead_code))]
    pub query: String,
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

/// Map MCP request params into Spider's `SearchOptions`. Pure — no network,
/// no provider construction — so the mapping itself is directly
/// unit-testable.
#[cfg(feature = "search_searxng")]
fn build_search_options(params: &SearchParams) -> spider::features::search::SearchOptions {
    let mut options = spider::features::search::SearchOptions::new();
    if let Some(limit) = params.limit {
        options = options.with_limit(limit);
    }
    if let Some(ref language) = params.language {
        options = options.with_language(language.clone());
    }
    options
}

/// Render Spider's canonical `SearchResults` into the deterministic MCP JSON
/// shape. Pure — testable directly with a hand-built `SearchResults`, no
/// network required. `snippet`/`date`/`score` serialize as JSON `null` (via
/// `Option`'s normal `Serialize` impl) rather than being fabricated or
/// omitted, so a downstream agent can tell "absent" from "empty string".
#[cfg(feature = "search_searxng")]
fn render_results(
    provider: &str,
    results: &spider::features::search::SearchResults,
) -> serde_json::Value {
    let results_json: Vec<serde_json::Value> = results
        .results
        .iter()
        .map(|r| {
            serde_json::json!({
                "position": r.position,
                "title": r.title,
                "url": r.url,
                "snippet": r.snippet,
                "date": r.date,
                "score": r.score,
            })
        })
        .collect();

    serde_json::json!({
        "query": results.query,
        "provider": provider,
        "result_count": results.results.len(),
        "results": results_json,
    })
}

/// Resolve `params.provider` into a live search, or an explicit error.
/// `SearxngProvider` never touches `Website`/Spider Cloud configuration —
/// there is no `apply_spider_cloud` call anywhere on this path, and none is
/// possible: the provider is a plain HTTP client wrapper unrelated to the
/// crawler's `Website` type.
#[cfg(feature = "search_searxng")]
pub async fn run(params: SearchParams) -> Result<String, String> {
    use spider::features::search::SearchProvider;
    use spider::features::search_providers::SearxngProvider;

    let options = build_search_options(&params);

    let results = match params.provider.as_str() {
        "searxng" => {
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

            let provider = SearxngProvider::new(base_url);
            provider
                .search(&params.query, &options)
                .await
                .map_err(|e| e.to_string())?
        }
        other => {
            return Err(format!(
                "Unknown or unsupported search provider \"{other}\". Currently supported: \"searxng\"."
            ));
        }
    };

    let output = render_results(&params.provider, &results);
    serde_json::to_string_pretty(&output).map_err(|e| e.to_string())
}

/// No-op degrade when the `search_searxng` feature isn't compiled in —
/// explicit error, matching how `apply_screenshot_options` degrades without
/// `chrome_screenshot`, never a silent success.
#[cfg(not(feature = "search_searxng"))]
pub async fn run(params: SearchParams) -> Result<String, String> {
    Err(format!(
        "Search provider \"{}\" is not available: this build was compiled without the \
         search_searxng feature.",
        params.provider
    ))
}

#[cfg(all(test, feature = "search_searxng"))]
mod tests {
    use super::*;
    use spider::features::search::{SearchResult, SearchResults};

    fn params(provider: &str, base_url: Option<&str>) -> SearchParams {
        SearchParams {
            query: "rust web crawler".to_string(),
            provider: provider.to_string(),
            base_url: base_url.map(|s| s.to_string()),
            limit: None,
            language: None,
        }
    }

    /// 2. SearXNG request maps MCP params into SearchOptions correctly.
    #[test]
    fn maps_limit_and_language_into_search_options() {
        let mut p = params("searxng", Some("http://localhost:8080"));
        p.limit = Some(5);
        p.language = Some("en".to_string());

        let options = build_search_options(&p);
        assert_eq!(options.limit, Some(5));
        assert_eq!(options.language.as_deref(), Some("en"));
    }

    #[test]
    fn omitted_optional_params_stay_none_in_options() {
        let p = params("searxng", Some("http://localhost:8080"));
        let options = build_search_options(&p);
        assert_eq!(options.limit, None);
        assert_eq!(options.language, None);
    }

    /// 3. Result ordering is preserved.
    #[test]
    fn render_results_preserves_order_and_fields() {
        let mut results = SearchResults::new("rust");
        results.push(SearchResult::new("First", "https://a.example/", 1).with_snippet("s1"));
        results.push(SearchResult::new("Second", "https://b.example/", 2));
        results.push(SearchResult::new("Third", "https://c.example/", 3));

        let v = render_results("searxng", &results);
        assert_eq!(v["query"], "rust");
        assert_eq!(v["provider"], "searxng");
        assert_eq!(v["result_count"], 3);
        assert_eq!(v["results"][0]["title"], "First");
        assert_eq!(v["results"][0]["position"], 1);
        assert_eq!(v["results"][0]["snippet"], "s1");
        assert_eq!(v["results"][1]["title"], "Second");
        assert_eq!(v["results"][1]["position"], 2);
        assert_eq!(v["results"][2]["title"], "Third");
        assert_eq!(v["results"][2]["position"], 3);
    }

    /// 4. Absent optional fields remain absent/null rather than fabricated.
    #[test]
    fn render_results_absent_optional_fields_are_null_not_fabricated() {
        let mut results = SearchResults::new("q");
        results.push(SearchResult::new("Bare", "https://bare.example/", 1));

        let v = render_results("searxng", &results);
        assert!(v["results"][0]["snippet"].is_null());
        assert!(v["results"][0]["date"].is_null());
        assert!(v["results"][0]["score"].is_null());
    }

    /// 5. Unknown provider fails explicitly.
    #[tokio::test]
    async fn unknown_provider_fails_explicitly() {
        let p = params("not-a-real-provider", None);
        let result = run(p).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not-a-real-provider"));
    }

    /// 6. Missing/invalid SearXNG base_url fails explicitly.
    #[tokio::test]
    async fn searxng_missing_base_url_fails_explicitly() {
        let p = params("searxng", None);
        let result = run(p).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_lowercase().contains("base_url"));
    }

    #[tokio::test]
    async fn searxng_blank_base_url_fails_explicitly() {
        let p = params("searxng", Some("   "));
        let result = run(p).await;
        assert!(result.is_err());
    }

    /// 7. Provider/network failure is not converted into empty success —
    /// an unreachable base_url must surface as Err, not Ok("...empty...").
    #[tokio::test]
    async fn unreachable_base_url_fails_explicitly_not_empty_success() {
        // Port chosen to be almost certainly unbound in any test environment.
        let p = params("searxng", Some("http://127.0.0.1:1"));
        let result = run(p).await;
        assert!(
            result.is_err(),
            "a real connection failure must surface as an explicit error"
        );
    }
}
