//! Canonical neutral web-search capability for the Scorpion workspace.
//!
//! This crate is the only owner of the search seam, models, errors, and
//! provider implementations. Higher-level crates expose re-export façades.

#![warn(missing_docs)]

mod search;

#[cfg(feature = "search")]
pub mod providers;

pub use search::{
    SearchError, SearchOptions, SearchProvider, SearchResult, SearchResults, TimeRange,
};

#[cfg(feature = "search_bing")]
pub use providers::BingProvider;
#[cfg(feature = "search_brave")]
pub use providers::BraveProvider;
#[cfg(feature = "search_serper")]
pub use providers::SerperProvider;
#[cfg(feature = "search_tavily")]
pub use providers::TavilyProvider;
#[cfg(feature = "search_searxng")]
pub use providers::{ImageResult, NewsResult, SearxngProvider, VideoResult};
