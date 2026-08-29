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
use async_trait::async_trait;

/// SearXNG metasearch provider.
///
/// Talks to a self-hosted (or otherwise operator-supplied) SearXNG
/// instance's `/search?format=json` endpoint. No API key, no hardcoded
/// public instance, no fallback to another provider.
///
/// # Example
/// ```ignore
/// use spider_search::{SearchOptions, SearchProvider, SearxngProvider};
///
/// let provider = SearxngProvider::new("http://localhost:8080");
/// let results = provider.search("rust web crawler", &SearchOptions::default()).await?;
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

    /// Perform the actual HTTP GET against the configured instance's
    /// `/search` endpoint, with `extra_params` appended beyond the always-
    /// present `q`/`format=json`, returning the raw parsed JSON. Shared by
    /// text, video, and image search so the request/status/parse handling
    /// exists exactly once — text search's behavior is unchanged by this:
    /// same params, same status handling, same error mapping.
    async fn fetch_json(
        &self,
        query: &str,
        extra_params: &[(&str, String)],
    ) -> Result<serde_json::Value, SearchError> {
        let endpoint = self.search_endpoint()?.to_string();

        let mut params = vec![("q", query.to_string()), ("format", "json".to_string())];
        params.extend(extra_params.iter().cloned());

        let endpoint = super::with_query(&endpoint, &params)?;
        let headers = super::headers(&[("Accept", "application/json")])?;
        let response = super::execute(reqwest::Method::GET, &endpoint, &headers, None).await?;

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
        Self::parse_json_body(&bytes)
    }

    /// Map an already-parsed SearXNG JSON response into Spider's canonical
    /// [`SearchResults`]. Pure and deterministic — no network — so result
    /// mapping (field presence, ordering, `limit` truncation) is directly
    /// unit-testable.
    fn map_results(query: &str, json: serde_json::Value, limit: Option<usize>) -> SearchResults {
        let mut results = SearchResults::new(query);

        // SearXNG reports upstream engine failures in the machine-readable
        // `unresponsive_engines` array. Preserve only the provider-neutral
        // availability signal; raw failure details remain in the existing
        // transient metadata field and are not used for terminal semantics.
        results.backend_failure = json
            .get("unresponsive_engines")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|engines| !engines.is_empty());

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

#[async_trait]
impl SearchProvider for SearxngProvider {
    async fn search(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<SearchResults, SearchError> {
        // Standard SearXNG JSON search API params. `format=json` requires
        // the instance to have the JSON output format enabled (a normal,
        // documented SearXNG setting — not a bespoke auth scheme).
        // `options.limit` has no server-side equivalent, so it is applied
        // client-side in `map_results` instead of guessing at a nonexistent
        // param.
        let mut extra_params = Vec::new();
        if let Some(ref language) = options.language {
            extra_params.push(("language", language.clone()));
        }

        let json = self.fetch_json(query, &extra_params).await?;
        Ok(Self::map_results(query, json, options.limit))
    }

    fn provider_name(&self) -> &'static str {
        "searxng"
    }

    fn is_configured(&self) -> bool {
        !self.base_url.is_empty()
    }
}

/// A discovered video result — the SearXNG-backed instance of the
/// `VideoResult` concept locked in `SCORPION.md` §9.3. Only fields SearXNG
/// actually supplied in the response are populated; nothing here is
/// fabricated. `provenance` is intentionally omitted from this struct —
/// SCORPION.md §3 has not yet locked a representation for it, and adding an
/// always-`None` placeholder field would invent one prematurely.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VideoResult {
    /// Result title.
    pub title: String,
    /// Video page/watch URL.
    pub url: String,
    /// Thumbnail image URL, if the source provided one.
    pub thumbnail_url: Option<String>,
    /// Snippet/description, if provided.
    pub description: Option<String>,
    /// Uploader, channel, or author, if provided.
    pub creator_or_channel: Option<String>,
    /// Publish date as reported by the source (opaque string — not
    /// reparsed/reformatted, to avoid inventing a value the source didn't
    /// actually state in that form).
    pub published_at: Option<String>,
    /// Duration as reported by the source (opaque string).
    pub duration: Option<String>,
    /// Which underlying engine/site surfaced this result (e.g. "youtube").
    pub source: Option<String>,
}

/// A discovered image result — the SearXNG-backed instance of the
/// `ImageResult` concept locked in `SCORPION.md` §9.3. Distinct from a
/// Scorpion-captured `PAGE_SCREENSHOT`: this is a result Scorpion *found*,
/// not evidence Scorpion *produced*. `provenance` is intentionally omitted
/// for the same reason as [`VideoResult`].
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ImageResult {
    /// Result title, if provided.
    pub title: Option<String>,
    /// Direct URL to the image file itself.
    pub image_url: String,
    /// Thumbnail/preview URL, if the source provided one.
    pub thumbnail_url: Option<String>,
    /// The page the image was found on (distinct from `image_url`, the
    /// image file itself).
    pub source_page: Option<String>,
    /// Pixel width, only when the source reported a parseable resolution.
    pub width: Option<u32>,
    /// Pixel height, only when the source reported a parseable resolution.
    pub height: Option<u32>,
    /// MIME type, only derived from a recognized `img_format` value —
    /// never guessed for an unrecognized one.
    pub mime_type: Option<String>,
    /// Snippet/description, if provided.
    pub description: Option<String>,
}

/// A news discovery candidate returned by SearXNG. This is provider output
/// only: constructing it never fetches the article or thumbnail URL.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NewsResult {
    /// Result headline.
    pub title: String,
    /// Article URL.
    pub url: String,
    /// Result snippet from SearXNG's raw `content` field.
    pub snippet: Option<String>,
    /// Raw `publishedDate` string, preserved verbatim.
    pub published_at: Option<String>,
    /// Raw result author, when supplied.
    pub author: Option<String>,
    /// Raw result thumbnail URL, when supplied.
    pub thumbnail_url: Option<String>,
    /// Raw SearXNG result `engine`; this is not a publisher or outlet.
    pub engine: Option<String>,
    /// Raw SearXNG result score.
    pub score: Option<f32>,
}

/// Map a SearXNG `img_format` value (e.g. `"png"`, `"jpeg"`) to its
/// standard MIME type. Unrecognized formats deliberately map to `None`
/// rather than guessing.
fn image_format_to_mime(fmt: &str) -> Option<&'static str> {
    match fmt.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpeg" | "jpg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg+xml" | "svg" => Some("image/svg+xml"),
        "bmp" => Some("image/bmp"),
        "x-icon" | "ico" => Some("image/x-icon"),
        "tiff" => Some("image/tiff"),
        _ => None,
    }
}

/// Parse a SearXNG `resolution` string (documented shape: `"{width} x
/// {height}"`) into `(width, height)`. Any other shape yields `None` rather
/// than a guessed value.
fn parse_resolution(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x').or_else(|| s.split_once('×'))?;
    let w: u32 = w.trim().parse().ok()?;
    let h: u32 = h.trim().parse().ok()?;
    Some((w, h))
}

/// SearXNG's `length` field is a Python `timedelta` server-side; its JSON
/// wire shape isn't pinned down by the docs, so this accepts either a
/// string or a number rather than assuming one — either way, it passes
/// through the raw value verbatim instead of inventing formatting.
fn json_value_to_display_string(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

impl SearxngProvider {
    /// Search only SearXNG's news category. The response is discovery data;
    /// article and thumbnail URLs are never opened by this method.
    pub async fn search_news(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<Vec<NewsResult>, SearchError> {
        let mut extra_params = vec![("categories", "news".to_string())];
        if let Some(ref language) = options.language {
            extra_params.push(("language", language.clone()));
        }

        let json = self.fetch_json(query, &extra_params).await?;
        Ok(Self::map_news_results(json, options.limit))
    }

    /// Search SearXNG's `videos` category (`categories=videos`, confirmed
    /// against SearXNG's own `settings.yml` — see module docs). Returns
    /// Spider's canonical `VideoResult` shape, ordering preserved from the
    /// source response, truncated to `options.limit` client-side (SearXNG
    /// has no server-side result-count param).
    pub async fn search_videos(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<Vec<VideoResult>, SearchError> {
        let mut extra_params = vec![("categories", "videos".to_string())];
        if let Some(ref language) = options.language {
            extra_params.push(("language", language.clone()));
        }

        let json = self.fetch_json(query, &extra_params).await?;
        Ok(Self::map_video_results(json, options.limit))
    }

    /// Search SearXNG's `images` category (`categories=images`, confirmed
    /// against SearXNG's own `settings.yml`). Returns Spider's canonical
    /// `ImageResult` shape.
    pub async fn search_images(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<Vec<ImageResult>, SearchError> {
        let mut extra_params = vec![("categories", "images".to_string())];
        if let Some(ref language) = options.language {
            extra_params.push(("language", language.clone()));
        }

        let json = self.fetch_json(query, &extra_params).await?;
        Ok(Self::map_image_results(json, options.limit))
    }

    /// Pure, deterministic mapping from a `categories=videos` JSON response
    /// into `VideoResult`s. No network — directly unit-testable.
    fn map_video_results(json: serde_json::Value, limit: Option<usize>) -> Vec<VideoResult> {
        let mut out = Vec::new();

        if let Some(items) = json.get("results").and_then(|v| v.as_array()) {
            for item in items {
                let title = item
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let url = item.get("url").and_then(|v| v.as_str()).unwrap_or_default();

                if url.is_empty() {
                    continue;
                }

                out.push(VideoResult {
                    title: title.to_string(),
                    url: url.to_string(),
                    thumbnail_url: item
                        .get("thumbnail")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                    description: item
                        .get("content")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                    creator_or_channel: item
                        .get("author")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                    published_at: item
                        .get("publishedDate")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                    duration: item.get("length").and_then(json_value_to_display_string),
                    source: item
                        .get("engine")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                });
            }
        }

        if let Some(limit) = limit {
            out.truncate(limit);
        }

        out
    }

    /// Pure, deterministic mapping from a `categories=images` JSON response
    /// into `ImageResult`s. No network — directly unit-testable.
    fn map_image_results(json: serde_json::Value, limit: Option<usize>) -> Vec<ImageResult> {
        let mut out = Vec::new();

        if let Some(items) = json.get("results").and_then(|v| v.as_array()) {
            for item in items {
                let image_url = item
                    .get("img_src")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();

                if image_url.is_empty() {
                    continue;
                }

                let (width, height) = item
                    .get("resolution")
                    .and_then(|v| v.as_str())
                    .and_then(parse_resolution)
                    .unzip();

                out.push(ImageResult {
                    title: item
                        .get("title")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                    image_url: image_url.to_string(),
                    thumbnail_url: item
                        .get("thumbnail_src")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                    source_page: item
                        .get("url")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                    width,
                    height,
                    mime_type: item
                        .get("img_format")
                        .and_then(|v| v.as_str())
                        .and_then(image_format_to_mime)
                        .map(str::to_string),
                    description: item
                        .get("content")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                });
            }
        }

        if let Some(limit) = limit {
            out.truncate(limit);
        }

        out
    }

    /// Pure mapping for a SearXNG `categories=news` response.
    fn map_news_results(json: serde_json::Value, limit: Option<usize>) -> Vec<NewsResult> {
        let mut out = Vec::new();
        if let Some(items) = json.get("results").and_then(|v| v.as_array()) {
            for item in items {
                let url = item.get("url").and_then(|v| v.as_str()).unwrap_or_default();
                if url.is_empty() {
                    continue;
                }
                let optional_string = |name| {
                    item.get(name)
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                };
                out.push(NewsResult {
                    title: item
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    url: url.to_string(),
                    snippet: optional_string("content"),
                    published_at: optional_string("publishedDate"),
                    author: optional_string("author"),
                    thumbnail_url: optional_string("thumbnail"),
                    engine: optional_string("engine"),
                    score: item
                        .get("score")
                        .and_then(|v| v.as_f64())
                        .map(|score| score as f32),
                });
            }
        }
        if let Some(limit) = limit {
            out.truncate(limit);
        }
        out
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
        let built = crate::providers::with_query(&endpoint, &params).expect("URL must build");
        let request_url = url::Url::parse(&built).expect("URL must parse");
        let built_url = request_url.as_str();
        assert!(built_url.starts_with("http://localhost:8080/search?"));
        // Raw '&', '"', and spaces must not appear unescaped in the query string.
        let query_part = request_url.query().unwrap();
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
        assert!(!results.backend_failure);
    }

    #[test]
    fn test_map_results_preserves_backend_failure_signal() {
        let body = json!({
            "results": [],
            "unresponsive_engines": [["duckduckgo", "CAPTCHA"]]
        });
        let results = SearxngProvider::map_results("rust", body, None);
        assert!(results.is_empty());
        assert!(results.backend_failure);
    }

    #[test]
    fn test_map_results_keeps_partial_results_usable_when_engine_fails() {
        let body = json!({
            "results": [{"title": "Rust", "url": "https://rust-lang.org/"}],
            "unresponsive_engines": [["brave", "TooManyRequests"]]
        });
        let results = SearxngProvider::map_results("rust", body, None);
        assert_eq!(results.len(), 1);
        assert!(results.backend_failure);
    }

    #[test]
    fn test_backend_failure_contract_matrix() {
        // Exercise a bounded matrix of empty/non-empty results and absent,
        // empty, single, and multiple failure metadata representations.
        let failure_shapes = [
            None,
            Some(json!([])),
            Some(json!([["engine", "timeout"]])),
            Some(json!([["a", "captcha"], ["b", "rate_limit"]])),
            Some(json!("not-an-array")),
        ];
        for has_result in [false, true] {
            for (index, failures) in failure_shapes.iter().enumerate() {
                let mut body = json!({
                    "results": if has_result {
                        json!([{"title": "Result", "url": "https://example.test"}])
                    } else {
                        json!([])
                    }
                });
                if let Some(value) = failures {
                    body["unresponsive_engines"] = value.clone();
                }
                let mapped = SearxngProvider::map_results(
                    &format!("matrix-{has_result}-{index}"),
                    body,
                    None,
                );
                let expected_failure = matches!(index, 2 | 3);
                assert_eq!(mapped.backend_failure, expected_failure);
                assert_eq!(mapped.is_empty(), !has_result);
            }
        }
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
        assert_eq!(
            r.snippet, None,
            "no content field must not fabricate a snippet"
        );
        assert_eq!(
            r.date, None,
            "no publishedDate field must not fabricate a date"
        );
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

    // --- video results (SCORPION_MEDIA_DISCOVERY_001) ---

    #[test]
    fn test_map_video_results_basic_shape_and_order() {
        let body = json!({
            "results": [
                {
                    "title": "Rust in 100 Seconds",
                    "url": "https://www.youtube.com/watch?v=abc123",
                    "thumbnail": "https://img.youtube.com/vi/abc123/hq.jpg",
                    "content": "A quick tour of Rust.",
                    "author": "Fireship",
                    "publishedDate": "2023-05-01T00:00:00",
                    "length": "0:02:20",
                    "engine": "youtube"
                },
                {
                    "title": "Second Video",
                    "url": "https://example.test/v2"
                }
            ]
        });

        let results = SearxngProvider::map_video_results(body, None);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust in 100 Seconds");
        assert_eq!(results[0].url, "https://www.youtube.com/watch?v=abc123");
        assert_eq!(
            results[0].thumbnail_url.as_deref(),
            Some("https://img.youtube.com/vi/abc123/hq.jpg")
        );
        assert_eq!(
            results[0].description.as_deref(),
            Some("A quick tour of Rust.")
        );
        assert_eq!(results[0].creator_or_channel.as_deref(), Some("Fireship"));
        assert_eq!(
            results[0].published_at.as_deref(),
            Some("2023-05-01T00:00:00")
        );
        assert_eq!(results[0].duration.as_deref(), Some("0:02:20"));
        assert_eq!(results[0].source.as_deref(), Some("youtube"));

        // Order preserved, and a second, sparse item follows.
        assert_eq!(results[1].title, "Second Video");
    }

    #[test]
    fn test_map_video_results_missing_optional_fields_stay_none() {
        let body = json!({
            "results": [{"title": "Bare", "url": "https://bare.example/v"}]
        });
        let results = SearxngProvider::map_video_results(body, None);
        assert_eq!(results.len(), 1);
        let r = &results[0];
        assert_eq!(r.thumbnail_url, None);
        assert_eq!(r.description, None);
        assert_eq!(r.creator_or_channel, None);
        assert_eq!(r.published_at, None);
        assert_eq!(r.duration, None);
        assert_eq!(r.source, None);
    }

    #[test]
    fn test_map_video_results_skips_items_without_url() {
        let body = json!({
            "results": [
                {"title": "No URL"},
                {"title": "Has URL", "url": "https://real.example/v"}
            ]
        });
        let results = SearxngProvider::map_video_results(body, None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://real.example/v");
    }

    #[test]
    fn test_map_video_results_respects_limit_truncation() {
        let body = json!({
            "results": [
                {"title": "One", "url": "https://1.example/v"},
                {"title": "Two", "url": "https://2.example/v"},
                {"title": "Three", "url": "https://3.example/v"}
            ]
        });
        let results = SearxngProvider::map_video_results(body, Some(2));
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "One");
        assert_eq!(results[1].title, "Two");
    }

    #[test]
    fn test_map_video_results_length_accepts_numeric_value() {
        let body = json!({
            "results": [{"title": "T", "url": "https://e.example/v", "length": 140}]
        });
        let results = SearxngProvider::map_video_results(body, None);
        assert_eq!(results[0].duration.as_deref(), Some("140"));
    }

    // --- image results (SCORPION_MEDIA_DISCOVERY_001) ---

    #[test]
    fn test_map_image_results_basic_shape_and_order() {
        let body = json!({
            "results": [
                {
                    "title": "A Cat",
                    "url": "https://example.test/page-with-cat",
                    "img_src": "https://example.test/cat.png",
                    "thumbnail_src": "https://example.test/cat_thumb.png",
                    "resolution": "1920 x 1080",
                    "img_format": "png",
                    "content": "A photo of a cat."
                },
                {
                    "img_src": "https://example.test/second.jpg"
                }
            ]
        });

        let results = SearxngProvider::map_image_results(body, None);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title.as_deref(), Some("A Cat"));
        assert_eq!(results[0].image_url, "https://example.test/cat.png");
        assert_eq!(
            results[0].thumbnail_url.as_deref(),
            Some("https://example.test/cat_thumb.png")
        );
        assert_eq!(
            results[0].source_page.as_deref(),
            Some("https://example.test/page-with-cat")
        );
        assert_eq!(results[0].width, Some(1920));
        assert_eq!(results[0].height, Some(1080));
        assert_eq!(results[0].mime_type.as_deref(), Some("image/png"));
        assert_eq!(results[0].description.as_deref(), Some("A photo of a cat."));

        // Order preserved.
        assert_eq!(results[1].image_url, "https://example.test/second.jpg");
    }

    #[test]
    fn test_map_image_results_missing_optional_fields_stay_none() {
        let body = json!({
            "results": [{"img_src": "https://bare.example/i.png"}]
        });
        let results = SearxngProvider::map_image_results(body, None);
        assert_eq!(results.len(), 1);
        let r = &results[0];
        assert_eq!(r.title, None);
        assert_eq!(r.thumbnail_url, None);
        assert_eq!(r.source_page, None);
        assert_eq!(r.width, None);
        assert_eq!(r.height, None);
        assert_eq!(r.mime_type, None);
        assert_eq!(r.description, None);
    }

    #[test]
    fn test_map_image_results_skips_items_without_img_src() {
        let body = json!({
            "results": [
                {"title": "No image URL"},
                {"img_src": "https://real.example/i.png"}
            ]
        });
        let results = SearxngProvider::map_image_results(body, None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].image_url, "https://real.example/i.png");
    }

    #[test]
    fn test_map_image_results_respects_limit_truncation() {
        let body = json!({
            "results": [
                {"img_src": "https://1.example/i.png"},
                {"img_src": "https://2.example/i.png"},
                {"img_src": "https://3.example/i.png"}
            ]
        });
        let results = SearxngProvider::map_image_results(body, Some(2));
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_map_image_results_unrecognized_format_not_guessed() {
        let body = json!({
            "results": [{"img_src": "https://e.example/i.xyz", "img_format": "xyz"}]
        });
        let results = SearxngProvider::map_image_results(body, None);
        assert_eq!(results[0].mime_type, None);
    }

    #[test]
    fn test_map_image_results_unparseable_resolution_not_guessed() {
        let body = json!({
            "results": [{"img_src": "https://e.example/i.png", "resolution": "huge"}]
        });
        let results = SearxngProvider::map_image_results(body, None);
        assert_eq!(results[0].width, None);
        assert_eq!(results[0].height, None);
    }

    #[test]
    fn test_parse_resolution_handles_expected_shape() {
        assert_eq!(parse_resolution("1920 x 1080"), Some((1920, 1080)));
        assert_eq!(parse_resolution("640x480"), Some((640, 480)));
        assert_eq!(parse_resolution("not a resolution"), None);
    }

    #[test]
    fn test_image_format_to_mime_known_and_unknown() {
        assert_eq!(image_format_to_mime("png"), Some("image/png"));
        assert_eq!(image_format_to_mime("JPEG"), Some("image/jpeg"));
        assert_eq!(image_format_to_mime("made-up-format"), None);
    }

    // --- news results (SCORPION_SEARXNG_NEWS_DISCOVERY_001) ---

    #[test]
    fn test_map_news_results_preserves_fields_order_and_opaque_date() {
        let body = json!({"results": [
            {
                "title": "First",
                "url": "https://example.test/article-a",
                "content": "Summary A",
                "publishedDate": "yesterday at 09:17 + mystery-zone",
                "author": "Author A",
                "thumbnail": "https://example.test/thumb.jpg",
                "engine": "engine-a",
                "score": 1.25
            },
            {"title": "Second", "url": "https://example.test/article-b"}
        ]});

        let results = SearxngProvider::map_news_results(body, None);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "First");
        assert_eq!(results[0].url, "https://example.test/article-a");
        assert_eq!(results[0].snippet.as_deref(), Some("Summary A"));
        assert_eq!(
            results[0].published_at.as_deref(),
            Some("yesterday at 09:17 + mystery-zone")
        );
        assert_eq!(results[0].author.as_deref(), Some("Author A"));
        assert_eq!(
            results[0].thumbnail_url.as_deref(),
            Some("https://example.test/thumb.jpg")
        );
        assert_eq!(results[0].engine.as_deref(), Some("engine-a"));
        let score: Option<f32> = results[0].score;
        assert_eq!(score, Some(1.25_f32));
        assert_eq!(results[1].title, "Second");
        assert_eq!(results[1].snippet, None);
        assert_eq!(results[1].published_at, None);
        assert_eq!(results[1].author, None);
        assert_eq!(results[1].thumbnail_url, None);
        assert_eq!(results[1].engine, None);
        assert_eq!(results[1].score, None);
    }

    #[test]
    fn test_map_news_results_limit_zero_and_skips_missing_url() {
        let body = json!({"results": [
            {"title": "No URL"},
            {"title": "One", "url": "https://1.example/"},
            {"title": "Two", "url": "https://2.example/"}
        ]});
        assert!(SearxngProvider::map_news_results(body.clone(), Some(0)).is_empty());
        let limited = SearxngProvider::map_news_results(body, Some(1));
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].title, "One");
    }
}
