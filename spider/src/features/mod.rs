/// Chrome utils
#[cfg(feature = "chrome")]
pub mod chrome;
#[cfg(feature = "chrome")]
/// Chrome launch args.
pub(crate) mod chrome_args;
/// Common modules for Chrome
pub mod chrome_common;
#[cfg(feature = "real_browser")]
/// Viewport
pub mod chrome_viewport;

/// WebDriver utils
#[cfg(feature = "webdriver")]
pub mod webdriver;
#[cfg(feature = "webdriver")]
/// WebDriver launch args.
pub(crate) mod webdriver_args;
/// Common modules for WebDriver
pub mod webdriver_common;

/// Decentralized header handling
#[cfg(feature = "decentralized_headers")]
pub mod decentralized_headers;
/// Disk options
pub mod disk;
/// URL globbing
#[cfg(feature = "glob")]
pub mod glob;
/// OpenAI
#[cfg(feature = "openai")]
pub mod openai;
/// Common modules for OpenAI
pub mod openai_common;

/// Gemini
#[cfg(feature = "gemini")]
pub mod gemini;
/// Common modules for Gemini
pub mod gemini_common;

/// Solve all.
pub mod solvers;

/// RSS and Atom feed parsing and normalization.
#[cfg(feature = "feed")]
pub mod feed;
/// Standard sitemap urlset and sitemapindex parsing and normalization.
#[cfg(feature = "sitemap")]
pub mod sitemap;
/// Google News Sitemap parsing and normalization.
#[cfg(feature = "news_sitemap")]
pub mod news_sitemap;
/// robots.txt `Sitemap:` directive discovery.
#[cfg(feature = "robots_sitemap")]
pub mod robots_sitemap;
/// Generic source-discovery vocabulary.
pub mod source;

/// Canonical HTTP transport policy (`Default` / Tor-over-SOCKS5h), with
/// fail-closed `.onion` protection and transport-pinned redirects.
pub mod transport;

#[cfg(all(not(feature = "simd"), any(feature = "openai", feature = "gemini")))]
pub(crate) use serde_json;
#[cfg(all(feature = "simd", any(feature = "openai", feature = "gemini")))]
pub(crate) use sonic_rs as serde_json;

/// Automation scripts.
pub mod automation;

/// Web search integration.
#[cfg(feature = "search")]
pub mod search;
/// Search provider implementations.
#[cfg(feature = "search")]
pub mod search_providers;
