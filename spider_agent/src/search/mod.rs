//! Thin compatibility façade over the canonical `spider_search` capability.

#[cfg(feature = "search_searxng")]
pub use spider_search::resolve_searxng_provider;
pub use spider_search::{
    resolve_search_provider, SearchProvider, SearchProviderConfigError, SearchProviderKind,
    SearchResult, SearchResults,
};

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
