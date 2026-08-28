//! Thin server-side application boundary for a future Scorpion Web Console.
//!
//! This crate owns request/response DTOs and HTTP mapping only. Search,
//! provider requests, parsing, and result normalization remain owned by the
//! canonical `spider_search` capability through Spider's public façade.

use serde::{Deserialize, Serialize};
use spider::features::search::{SearchOptions, SearchProvider};
use spider::features::search_providers::SearxngProvider;

/// Stable application request for the first Web Console capability.
#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct SearchRequest {
    /// User-entered search query.
    pub query: String,
    /// Maximum number of returned candidates. The provider applies this
    /// client-side after canonical result mapping.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Stable application response; provider-specific metadata is intentionally
/// not exposed here.
#[derive(Debug, Serialize, PartialEq)]
pub struct SearchResponse {
    pub query: String,
    pub result_count: usize,
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct SearchResult {
    pub position: usize,
    pub title: String,
    pub url: String,
    pub snippet: Option<String>,
    pub date: Option<String>,
    pub score: Option<f32>,
}

/// Errors exposed by the application boundary. Secrets and filesystem paths
/// are never carried in these messages.
#[derive(Debug, PartialEq, Eq)]
pub enum SearchError {
    InvalidRequest(String),
    ProviderNotConfigured,
    Provider(String),
    Internal(String),
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(message) => write!(f, "invalid request: {message}"),
            Self::ProviderNotConfigured => f.write_str("search provider is not configured"),
            Self::Provider(message) => write!(f, "search provider failed: {message}"),
            Self::Internal(message) => write!(f, "internal application error: {message}"),
        }
    }
}

/// Execute one canonical SearXNG search using server-owned configuration.
pub async fn search(
    request: SearchRequest,
    searxng_base_url: Option<&str>,
) -> Result<SearchResponse, SearchError> {
    let query = request.query.trim();
    if query.is_empty() {
        return Err(SearchError::InvalidRequest(
            "query must not be empty".into(),
        ));
    }
    if request.limit == Some(0) {
        return Err(SearchError::InvalidRequest(
            "limit must be greater than zero".into(),
        ));
    }
    let base_url = searxng_base_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(SearchError::ProviderNotConfigured)?;

    let provider = SearxngProvider::new(base_url);
    let options = request.limit.map_or_else(SearchOptions::new, |limit| {
        SearchOptions::new().with_limit(limit)
    });
    let results = provider
        .search(query, &options)
        .await
        .map_err(|error| SearchError::Provider(error.to_string()))?;

    Ok(SearchResponse {
        query: results.query,
        result_count: results.results.len(),
        results: results
            .results
            .into_iter()
            .map(|result| SearchResult {
                position: result.position,
                title: result.title,
                url: result.url,
                snippet: result.snippet,
                date: result.date,
                score: result.score,
            })
            .collect(),
    })
}

/// HTTP status and public JSON error body for an application error.
pub fn error_status(error: &SearchError) -> u16 {
    match error {
        SearchError::InvalidRequest(_) => 400,
        SearchError::ProviderNotConfigured => 503,
        SearchError::Provider(_) => 502,
        SearchError::Internal(_) => 500,
    }
}

/// Serialize a public error without leaking provider configuration.
pub fn error_json(error: &SearchError) -> String {
    serde_json::json!({
        "error": {
            "code": match error {
                SearchError::InvalidRequest(_) => "invalid_request",
                SearchError::ProviderNotConfigured => "provider_not_configured",
                SearchError::Provider(_) => "provider_unavailable",
                SearchError::Internal(_) => "internal_error",
            },
            "message": error.to_string(),
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_response_excludes_provider_metadata() {
        let response = SearchResponse {
            query: "rust".into(),
            result_count: 1,
            results: vec![SearchResult {
                position: 1,
                title: "Rust".into(),
                url: "https://www.rust-lang.org".into(),
                snippet: Some("language".into()),
                date: None,
                score: Some(1.0),
            }],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(!json.contains("metadata"));
        assert!(!json.contains("api_key"));
    }

    #[tokio::test]
    async fn request_validation_and_configuration_fail_closed() {
        let empty = search(
            SearchRequest {
                query: " ".into(),
                limit: None,
            },
            Some("http://127.0.0.1:1"),
        )
        .await
        .unwrap_err();
        assert_eq!(
            empty,
            SearchError::InvalidRequest("query must not be empty".into())
        );

        let missing = search(
            SearchRequest {
                query: "rust".into(),
                limit: None,
            },
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(missing, SearchError::ProviderNotConfigured);
    }
}
