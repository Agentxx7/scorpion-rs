//! Thin compatibility façade over canonical search providers.

#[cfg(feature = "search_bing")]
pub use spider_search::BingProvider;
#[cfg(feature = "search_brave")]
pub use spider_search::BraveProvider;
#[cfg(feature = "search_serper")]
pub use spider_search::SerperProvider;
#[cfg(feature = "search_tavily")]
pub use spider_search::TavilyProvider;
#[cfg(feature = "search_searxng")]
pub use spider_search::{
    ImageResult as SearxngImageResult, NewsResult as SearxngNewsResult, SearxngProvider,
    VideoResult as SearxngVideoResult,
};

pub use super::search::{SearchError, SearchOptions, SearchProvider, SearchResult, SearchResults};
