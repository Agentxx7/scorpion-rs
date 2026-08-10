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

/// `DiscoveryTarget`: the smallest canonical planning boundary for
/// discovery pointers (sitemap index child sitemaps, robots.txt-declared
/// sitemaps, caller/request-supplied URLs) — URLs to acquire *later*,
/// never `SourceItem` content candidates and never something already
/// fetched. Zero acquisition; terminates in targets. Always available —
/// the module itself has no feature gate, though its sitemap/
/// robots_sitemap `PlanningInput` variants are individually gated behind
/// their respective existing features.
pub mod discovery_target;
/// RSS and Atom feed parsing and normalization.
#[cfg(feature = "feed")]
pub mod feed;
/// Google News Sitemap parsing and normalization.
#[cfg(feature = "news_sitemap")]
pub mod news_sitemap;
/// Manual/request-supplied onion seed URL discovery (classification and
/// `SourceItem` normalization only — zero target acquisition). Available
/// unconditionally, independent of the `transport_tor` feature: this is
/// URL classification, not Tor networking.
pub mod onion_seed;
/// `ResearchScope`: the smallest canonical declarative discovery-scope
/// boundary (onion seeds / already-produced candidates only — never
/// fetched document bytes), plus the `discover` orchestration seam that
/// normalizes a `ResearchScope` together with already-acquired
/// `DiscoveryMaterial` (fetched feed/sitemap/news-sitemap document
/// bytes) into ordered `SourceItem` candidates. Zero acquisition;
/// terminates in candidates. Always available — the module itself has
/// no feature gate, though its feed/sitemap/news_sitemap
/// `DiscoveryMaterial` variants are individually gated behind their
/// respective existing features.
pub mod research_scope;
/// robots.txt `Sitemap:` directive discovery.
#[cfg(feature = "robots_sitemap")]
pub mod robots_sitemap;
/// Standard sitemap urlset and sitemapindex parsing and normalization.
#[cfg(feature = "sitemap")]
pub mod sitemap;
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
