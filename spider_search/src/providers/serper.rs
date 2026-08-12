//! Serper.dev search provider implementation.
//!
//! Serper provides high-quality Google SERP API access.

use super::{SearchError, SearchOptions, SearchProvider, SearchResult, SearchResults};
use async_trait::async_trait;

/// Default Serper API endpoint.
const DEFAULT_API_URL: &str = "https://google.serper.dev/search";

/// Serper.dev search provider.
///
/// Provides access to Google search results via the Serper API.
///
/// # Example
/// ```ignore
/// use spider_search::{SearchOptions, SearchProvider, SerperProvider};
///
/// let provider = SerperProvider::new("your-api-key");
/// let results = provider.search("rust web crawler", &SearchOptions::default()).await?;
/// ```
#[derive(Debug, Clone)]
pub struct SerperProvider {
    api_key: String,
    api_url: Option<String>,
}

impl SerperProvider {
    /// Create a new Serper provider with the given API key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            api_url: None,
        }
    }

    /// Use a custom API endpoint.
    ///
    /// This is useful for self-hosted or alternative Serper-compatible APIs.
    pub fn with_api_url(mut self, url: impl Into<String>) -> Self {
        self.api_url = Some(url.into());
        self
    }

    /// Get the API endpoint URL.
    fn endpoint(&self) -> &str {
        self.api_url.as_deref().unwrap_or(DEFAULT_API_URL)
    }
}

#[async_trait]
impl SearchProvider for SerperProvider {
    async fn search(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<SearchResults, SearchError> {
        // Build request body
        let mut body = serde_json::json!({
            "q": query
        });

        if let Some(limit) = options.limit {
            body["num"] = serde_json::json!(limit.min(100));
        }

        if let Some(ref country) = options.country {
            body["gl"] = serde_json::json!(country);
        }

        if let Some(ref language) = options.language {
            body["hl"] = serde_json::json!(language);
        }

        // Build the query with site filter if present
        let query_with_filter = if let Some(ref sites) = options.site_filter {
            let site_query = sites
                .iter()
                .map(|s| format!("site:{}", s))
                .collect::<Vec<_>>()
                .join(" OR ");
            format!("{} ({})", query, site_query)
        } else {
            query.to_string()
        };
        body["q"] = serde_json::json!(query_with_filter);

        let headers = super::headers(&[("X-API-KEY", &self.api_key)])?;
        let body = serde_json::to_vec(&body)
            .map_err(|error| SearchError::ProviderError(error.to_string()))?;
        let response =
            super::execute(reqwest::Method::POST, self.endpoint(), &headers, Some(body)).await?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(SearchError::AuthenticationFailed);
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(SearchError::RateLimited);
        }
        if !status.is_success() {
            return Err(SearchError::ProviderError(format!(
                "HTTP {} from Serper API",
                status
            )));
        }

        // Parse response
        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| SearchError::ProviderError(format!("Failed to parse response: {}", e)))?;

        // Extract organic results
        let mut results = SearchResults::new(query);

        if let Some(organic) = json.get("organic").and_then(|v| v.as_array()) {
            for (i, item) in organic.iter().enumerate() {
                let title = item
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let url = item
                    .get("link")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();

                if url.is_empty() {
                    continue;
                }

                let mut result = SearchResult::new(title, url, i + 1);

                if let Some(snippet) = item.get("snippet").and_then(|v| v.as_str()) {
                    result = result.with_snippet(snippet);
                }

                if let Some(date) = item.get("date").and_then(|v| v.as_str()) {
                    result = result.with_date(date);
                }

                results.push(result);
            }
        }

        // Extract total results if available
        if let Some(info) = json.get("searchInformation") {
            if let Some(total) = info.get("totalResults").and_then(|v| v.as_str()) {
                results.total_results = total.parse().ok();
            }
        }

        // Store raw metadata
        results.metadata = Some(json);

        Ok(results)
    }

    fn provider_name(&self) -> &'static str {
        "serper"
    }

    fn is_configured(&self) -> bool {
        !self.api_key.is_empty() || self.api_url.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serper_provider_new() {
        let provider = SerperProvider::new("test-key");
        assert_eq!(provider.endpoint(), DEFAULT_API_URL);
    }

    #[test]
    fn test_serper_provider_custom_url() {
        let provider =
            SerperProvider::new("test-key").with_api_url("https://custom.api.com/search");
        assert_eq!(provider.endpoint(), "https://custom.api.com/search");
    }
}
