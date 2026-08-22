//! Thin compatibility façade over the canonical `spider_search` capability.

pub use spider_search::{SearchProvider, SearchResult, SearchResults};

#[cfg(feature = "search_bing")]
pub use spider_search::BingProvider;
#[cfg(feature = "search_brave")]
pub use spider_search::BraveProvider;
#[cfg(feature = "search_searxng")]
pub use spider_search::SearxngProvider;
#[cfg(feature = "search_serper")]
pub use spider_search::SerperProvider;
#[cfg(feature = "search_tavily")]
pub use spider_search::TavilyProvider;
