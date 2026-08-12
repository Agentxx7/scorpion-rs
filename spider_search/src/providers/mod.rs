//! Search provider implementations.
//!
//! This module contains implementations of the [`super::search::SearchProvider`] trait
//! for various web search APIs.

#[cfg(feature = "search_bing")]
mod bing;
#[cfg(feature = "search_brave")]
mod brave;
#[cfg(feature = "search_searxng")]
mod searxng;
#[cfg(feature = "search_serper")]
mod serper;
#[cfg(feature = "search_tavily")]
mod tavily;

#[cfg(feature = "search_bing")]
pub use bing::BingProvider;
#[cfg(feature = "search_brave")]
pub use brave::BraveProvider;
#[cfg(feature = "search_searxng")]
pub use searxng::{ImageResult, NewsResult, SearxngProvider, VideoResult};
#[cfg(feature = "search_serper")]
pub use serper::SerperProvider;
#[cfg(feature = "search_tavily")]
pub use tavily::TavilyProvider;

pub use super::search::{SearchError, SearchOptions, SearchProvider, SearchResult, SearchResults};

#[cfg(any(
    feature = "search_bing",
    feature = "search_brave",
    feature = "search_searxng",
    feature = "search_serper",
    feature = "search_tavily"
))]
pub(crate) async fn execute(
    method: reqwest::Method,
    endpoint: &str,
    headers: &spider_transport::SecretRequestHeaders,
    body: Option<Vec<u8>>,
) -> Result<reqwest::Response, SearchError> {
    let url =
        url::Url::parse(endpoint).map_err(|error| SearchError::InvalidQuery(error.to_string()))?;
    let content_type = body.as_ref().map(|_| "application/json");
    spider_transport::execute_request(
        &url,
        method,
        &spider_transport::TransportPolicy::Default,
        headers,
        body,
        content_type,
        "spider_search",
    )
    .await
    .map_err(|error| SearchError::RequestFailed(error.to_string()))
}

#[cfg(any(
    feature = "search_bing",
    feature = "search_brave",
    feature = "search_searxng",
    feature = "search_serper"
))]
pub(crate) fn headers(
    values: &[(&str, &str)],
) -> Result<spider_transport::SecretRequestHeaders, SearchError> {
    let mut headers = spider_transport::SecretRequestHeaders::new();
    for (name, value) in values {
        headers
            .try_insert(name, value)
            .map_err(|_| SearchError::AuthenticationFailed)?;
    }
    Ok(headers)
}

#[cfg(any(
    feature = "search_bing",
    feature = "search_brave",
    feature = "search_searxng"
))]
pub(crate) fn with_query(endpoint: &str, params: &[(&str, String)]) -> Result<String, SearchError> {
    let mut url =
        url::Url::parse(endpoint).map_err(|error| SearchError::InvalidQuery(error.to_string()))?;
    url.query_pairs_mut()
        .extend_pairs(params.iter().map(|(key, value)| (*key, value.as_str())));
    Ok(url.into())
}
