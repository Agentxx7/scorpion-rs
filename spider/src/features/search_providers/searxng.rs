//! SearXNG search provider implementation.
//!
//! [SearXNG](https://docs.searxng.org/) is a free, self-hosted metasearch
//! engine. Unlike the other providers in this module, SearXNG is not a
//! commercial API with a single official endpoint — there is no "the"
//! SearXNG instance, and Scorpion does not ship or default to any public
//! one. Callers must always supply the base URL of a SearXNG deployment
//! they control (self-hosted or otherwise trusted), and no API key is
//! required by the standard SearXNG JSON search endpoint.

use super::{SearchError, SearchOptions, SearchProvider, SearchResult, SearchResults};

/// SearXNG metasearch provider.
///
/// Talks to a self-hosted (or otherwise operator-supplied) SearXNG
/// instance's `/search?format=json` endpoint. No API key, no hardcoded
/// public instance, no fallback to another provider.
///
/// # Example
/// ```ignore
/// use spider::features::search_providers::SearxngProvider;
/// use spider::features::search::{SearchOptions, SearchProvider};
///
/// let provider = SearxngProvider::new("http://localhost:8080");
/// let results = provider.search("rust web crawler", &SearchOptions::default(), None).await?;
/// ```
#[derive(Debug, Clone)]
pub struct SearxngProvider {
    base_url: String,
}

impl SearxngProvider {
    /// Create a new SearXNG provider pointed at the given instance base URL
    /// (e.g. `"http://localhost:8080"` or `"https://searxng.internal/"`).
    /// The URL is validated lazily on first [`SearchProvider::search`] call
    /// rather than here, matching this crate's other infallible provider
    /// constructors — but it is always validated before any request is
    /// made, and a bad URL fails explicitly rather than silently falling
    /// back to any default.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
        }
    }

    /// Resolve the configured base URL's `/search` endpoint. Explicit
    /// `Err` — never a silent fallback to any other host or service — for
    /// an unparsable URL or a non-http(s) scheme.
    fn search_endpoint(&self) -> Result<url::Url, SearchError> {
        let mut base = self.base_url.clone();
        if !base.ends_with('/') {
            // `Url::join` replaces the last path segment unless the base
            // ends with `/`, which would silently truncate a mounted-path
            // deployment (e.g. `.../searxng` -> `.../search`). Normalizing
            // first keeps `join("search")` additive in every case.
            base.push('/');
        }

        let base = url::Url::parse(&base).map_err(|e| {
            SearchError::ProviderError(format!(
                "Invalid SearXNG base URL \"{}\": {e}",
                self.base_url
            ))
        })?;

        if base.scheme() != "http" && base.scheme() != "https" {
            return Err(SearchError::ProviderError(format!(
                "Invalid SearXNG base URL \"{}\": scheme must be http or https",
                self.base_url
            )));
        }

        base.join("search").map_err(|e| {
            SearchError::ProviderError(format!(
                "Invalid SearXNG base URL \"{}\": {e}",
                self.base_url
            ))
        })
    }

    /// Deserialize a raw response body. Split out from [`Self::search`] so
    /// malformed/non-JSON bodies (an HTML error page, an instance with the
    /// JSON format disabled, a truncated response, etc.) can be proven to
    /// fail explicitly with a deterministic, offline unit test rather than
    /// only being exercised over a real HTTP round trip.
    fn parse_json_body(bytes: &[u8]) -> Result<serde_json::Value, SearchError> {
        serde_json::from_slice(bytes)
            .map_err(|e| SearchError::ProviderError(format!("Failed to parse response: {e}")))
    }

    /// Map an already-parsed SearXNG JSON response into Spider's canonical
    /// [`SearchResults`]. Pure and deterministic — no network — so result
    /// mapping (field presence, ordering, `limit` truncation) is directly
    /// unit-testable.
    fn map_results(query: &str, json: serde_json::Value, limit: Option<usize>) -> SearchResults {
        let mut results = SearchResults::new(query);

        if let Some(items) = json.get("results").and_then(|v| v.as_array()) {
            for (i, item) in items.iter().enumerate() {
                let title = item
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let url = item.get("url").and_then(|v| v.as_str()).unwrap_or_default();

                if url.is_empty() {
                    continue;
                }

                let mut result = SearchResult::new(title, url, i + 1);

                // SearXNG's snippet/description field is named "content",
                // not "snippet" — mapped only when present and non-empty so
                // we never fabricate a snippet the source didn't provide.
                if let Some(snippet) = item.get("content").and_then(|v| v.as_str()) {
                    if !snippet.is_empty() {
                        result = result.with_snippet(snippet);
                    }
                }

                if let Some(date) = item.get("publishedDate").and_then(|v| v.as_str()) {
                    if !date.is_empty() {
                        result = result.with_date(date);
                    }
                }

                if let Some(score) = item.get("score").and_then(|v| v.as_f64()) {
                    result = result.with_score(score as f32);
                }

                results.push(result);
            }
        }

        if let Some(limit) = limit {
            results.results.truncate(limit);
        }

        if let Some(total) = json.get("number_of_results").and_then(|v| v.as_u64()) {
            results.total_results = Some(total);
        }

        // Store raw metadata (answers/infoboxes/suggestions/etc. SearXNG
        // returns alongside `results`), same convention as the other
        // providers in this module.
        results.metadata = Some(json);

        results
    }
}

impl SearchProvider for SearxngProvider {
    async fn search(
        &self,
        query: &str,
        options: &SearchOptions,
        client: Option<&reqwest::Client>,
    ) -> Result<SearchResults, SearchError> {
        let endpoint = self.search_endpoint()?.to_string();

        // Standard SearXNG JSON search API params. `format=json` requires
        // the instance to have the JSON output format enabled (a normal,
        // documented SearXNG setting — not a bespoke auth scheme). Only
        // params SearXNG's documented API actually supports are sent;
        // `options.limit` has no server-side equivalent, so it is applied
        // client-side in `map_results` instead of guessing at a nonexistent
        // param.
        let mut params = vec![("q", query.to_string()), ("format", "json".to_string())];

        if let Some(ref language) = options.language {
            params.push(("language", language.clone()));
        }

        // reqwest's `.query()` percent-encodes each pair via the `url`
        // crate's `form_urlencoded` — the same mechanism every other
        // provider in this module relies on for safe query construction.
        let response = if let Some(c) = client {
            c.get(&endpoint)
                .header("Accept", "application/json")
                .query(&params)
                .send()
                .await
        } else {
            let c = reqwest::ClientBuilder::new()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| SearchError::RequestFailed(e.to_string()))?;

            c.get(&endpoint)
                .header("Accept", "application/json")
                .query(&params)
                .send()
                .await
        };

        let response = response.map_err(|e| SearchError::RequestFailed(e.to_string()))?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(SearchError::AuthenticationFailed);
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(SearchError::RateLimited);
        }
        if !status.is_success() {
            return Err(SearchError::ProviderError(format!(
                "HTTP {} from SearXNG instance",
                status
            )));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| SearchError::RequestFailed(e.to_string()))?;
        let json = Self::parse_json_body(&bytes)?;

        Ok(Self::map_results(query, json, options.limit))
    }

    fn provider_name(&self) -> &'static str {
        "searxng"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_searxng_provider_new() {
        let provider = SearxngProvider::new("http://localhost:8080");
        assert_eq!(provider.base_url, "http://localhost:8080");
    }

    // --- URL construction (Phase 3 #5, #6) ---

    #[test]
    fn test_searxng_endpoint_no_trailing_slash() {
        let provider = SearxngProvider::new("http://localhost:8080");
        assert_eq!(
            provider.search_endpoint().unwrap().as_str(),
            "http://localhost:8080/search"
        );
    }

    #[test]
    fn test_searxng_endpoint_with_trailing_slash() {
        let provider = SearxngProvider::new("http://localhost:8080/");
        assert_eq!(
            provider.search_endpoint().unwrap().as_str(),
            "http://localhost:8080/search"
        );
    }

    #[test]
    fn test_searxng_endpoint_preserves_mounted_path() {
        // A SearXNG instance mounted under a sub-path must not have that
        // path silently dropped by naive `Url::join` usage.
        let provider = SearxngProvider::new("http://localhost:8080/searxng");
        assert_eq!(
            provider.search_endpoint().unwrap().as_str(),
            "http://localhost:8080/searxng/search"
        );
    }

    #[test]
    fn test_searxng_invalid_base_url_fails_explicitly() {
        let provider = SearxngProvider::new("not a url");
        let err = provider.search_endpoint().unwrap_err();
        assert!(matches!(err, SearchError::ProviderError(_)));
    }

    #[test]
    fn test_searxng_non_http_scheme_rejected() {
        // Must fail rather than silently proceeding to some other service.
        let provider = SearxngProvider::new("ftp://localhost:8080");
        let err = provider.search_endpoint().unwrap_err();
        assert!(matches!(err, SearchError::ProviderError(_)));
    }

    #[test]
    fn test_searxng_query_is_safely_percent_encoded() {
        // Build (not send) a real request via the same client/param
        // machinery `search()` uses, and inspect the resulting URL —
        // proves safe encoding without any network access.
        let provider = SearxngProvider::new("http://localhost:8080");
        let endpoint = provider.search_endpoint().unwrap().to_string();
        let params = vec![
            ("q", "rust web crawler & \"quotes\"".to_string()),
            ("format", "json".to_string()),
        ];
        let client = reqwest::Client::new();
        let request = client
            .get(&endpoint)
            .query(&params)
            .build()
            .expect("request must build");
        let built_url = request.url().as_str();
        assert!(built_url.starts_with("http://localhost:8080/search?"));
        // Raw '&', '"', and spaces must not appear unescaped in the query string.
        let query_part = request.url().query().unwrap();
        assert!(!query_part.contains(' '));
        assert!(!query_part.contains('"'));
        assert!(query_part.contains("q=rust"));
    }

    // --- malformed response (Phase 3 #4) ---

    #[test]
    fn test_malformed_response_fails_explicitly() {
        let err = SearxngProvider::parse_json_body(b"not valid json {{{").unwrap_err();
        assert!(matches!(err, SearchError::ProviderError(_)));
    }

    #[test]
    fn test_empty_body_fails_explicitly() {
        let err = SearxngProvider::parse_json_body(b"").unwrap_err();
        assert!(matches!(err, SearchError::ProviderError(_)));
    }

    // --- result mapping (Phase 3 #1, #2, #3) ---

    #[test]
    fn test_map_results_basic_shape() {
        let body = json!({
            "query": "rust",
            "number_of_results": 2,
            "results": [
                {
                    "title": "Rust Programming Language",
                    "url": "https://rust-lang.org/",
                    "content": "A language empowering everyone.",
                    "publishedDate": "2024-01-01",
                    "score": 1.5
                },
                {
                    "title": "Rust on Wikipedia",
                    "url": "https://en.wikipedia.org/wiki/Rust_(programming_language)",
                    "content": "Rust is a multi-paradigm language.",
                    "score": 0.9
                }
            ]
        });

        let results = SearxngProvider::map_results("rust", body, None);

        assert_eq!(results.query, "rust");
        assert_eq!(results.len(), 2);
        assert_eq!(results.total_results, Some(2));

        assert_eq!(results.results[0].title, "Rust Programming Language");
        assert_eq!(results.results[0].url, "https://rust-lang.org/");
        assert_eq!(
            results.results[0].snippet.as_deref(),
            Some("A language empowering everyone.")
        );
        assert_eq!(results.results[0].date.as_deref(), Some("2024-01-01"));
        assert_eq!(results.results[0].score, Some(1.5));
    }

    /// 2. Multiple results preserve deterministic ordering.
    #[test]
    fn test_map_results_preserves_order_and_position() {
        let body = json!({
            "results": [
                {"title": "First", "url": "https://a.example/"},
                {"title": "Second", "url": "https://b.example/"},
                {"title": "Third", "url": "https://c.example/"}
            ]
        });

        let results = SearxngProvider::map_results("q", body, None);

        assert_eq!(results.len(), 3);
        assert_eq!(results.results[0].title, "First");
        assert_eq!(results.results[0].position, 1);
        assert_eq!(results.results[1].title, "Second");
        assert_eq!(results.results[1].position, 2);
        assert_eq!(results.results[2].title, "Third");
        assert_eq!(results.results[2].position, 3);
    }

    /// 3. Missing optional fields do not fabricate content.
    #[test]
    fn test_map_results_missing_optional_fields_stay_none() {
        let body = json!({
            "results": [
                {"title": "Bare Result", "url": "https://bare.example/"}
            ]
        });

        let results = SearxngProvider::map_results("q", body, None);

        assert_eq!(results.len(), 1);
        let r = &results.results[0];
        assert_eq!(r.title, "Bare Result");
        assert_eq!(r.snippet, None, "no content field must not fabricate a snippet");
        assert_eq!(r.date, None, "no publishedDate field must not fabricate a date");
        assert_eq!(r.score, None, "no score field must not fabricate a score");
    }

    #[test]
    fn test_map_results_empty_content_not_mapped_as_snippet() {
        let body = json!({
            "results": [
                {"title": "Empty Content", "url": "https://empty.example/", "content": ""}
            ]
        });

        let results = SearxngProvider::map_results("q", body, None);
        assert_eq!(results.results[0].snippet, None);
    }

    #[test]
    fn test_map_results_skips_items_without_url() {
        let body = json!({
            "results": [
                {"title": "No URL"},
                {"title": "Has URL", "url": "https://real.example/"}
            ]
        });

        let results = SearxngProvider::map_results("q", body, None);
        assert_eq!(results.len(), 1);
        assert_eq!(results.results[0].url, "https://real.example/");
    }

    #[test]
    fn test_map_results_respects_limit_truncation() {
        let body = json!({
            "results": [
                {"title": "One", "url": "https://1.example/"},
                {"title": "Two", "url": "https://2.example/"},
                {"title": "Three", "url": "https://3.example/"}
            ]
        });

        let results = SearxngProvider::map_results("q", body, Some(2));
        assert_eq!(results.len(), 2);
        assert_eq!(results.results[0].title, "One");
        assert_eq!(results.results[1].title, "Two");
    }

    #[test]
    fn test_map_results_no_results_array_is_empty_not_error() {
        let body = json!({"query": "q"});
        let results = SearxngProvider::map_results("q", body, None);
        assert!(results.is_empty());
    }
}
