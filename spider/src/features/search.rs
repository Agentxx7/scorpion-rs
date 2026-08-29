//! Thin compatibility façade over the canonical `spider_search` capability.

#[cfg(feature = "search_searxng")]
pub use spider_search::resolve_searxng_provider;
pub use spider_search::{
    resolve_search_provider, SearchError, SearchOptions, SearchProvider, SearchProviderConfigError,
    SearchProviderKind, SearchResult, SearchResults, TimeRange,
};
