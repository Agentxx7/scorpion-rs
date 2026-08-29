//! Canonical runtime selection of exactly one search provider.

use crate::SearchProvider;
use std::fmt;
use std::str::FromStr;

/// Provider identities accepted by the runtime selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchProviderKind {
    /// Operator-managed SearXNG instance.
    Searxng,
    /// Brave Search API.
    Brave,
    /// Serper API.
    Serper,
    /// Tavily Search API.
    Tavily,
    /// Retired Bing API (kept for compatibility, never selectable).
    Bing,
}

impl SearchProviderKind {
    /// Canonical lowercase identifier.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Searxng => "searxng",
            Self::Brave => "brave",
            Self::Serper => "serper",
            Self::Tavily => "tavily",
            Self::Bing => "bing",
        }
    }
}

impl FromStr for SearchProviderKind {
    type Err = SearchProviderConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "searxng" => Ok(Self::Searxng),
            "brave" => Ok(Self::Brave),
            "serper" => Ok(Self::Serper),
            "tavily" => Ok(Self::Tavily),
            "bing" => Ok(Self::Bing),
            other => Err(SearchProviderConfigError::UnknownProvider(
                other.to_string(),
            )),
        }
    }
}

/// Sanitized provider configuration error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchProviderConfigError {
    /// Selector was empty or unknown.
    UnknownProvider(String),
    /// Required non-secret configuration is missing.
    MissingConfiguration(&'static str),
    /// Provider exists in source but is not enabled in this build.
    UnsupportedProvider(&'static str),
}

impl fmt::Display for SearchProviderConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownProvider(name) => write!(f, "unknown search provider \"{name}\""),
            Self::MissingConfiguration(name) => write!(f, "search provider requires {name}"),
            Self::UnsupportedProvider(name) => {
                write!(f, "search provider \"{name}\" is not enabled")
            }
        }
    }
}

impl std::error::Error for SearchProviderConfigError {}

/// Resolve one provider from deployment configuration. An absent selector
/// preserves legacy SearXNG behavior when a SearXNG URL is present.
pub fn resolve_search_provider(
    selector: Option<&str>,
    searxng_url: Option<&str>,
    brave_key: Option<&str>,
    serper_key: Option<&str>,
    tavily_key: Option<&str>,
) -> Result<(SearchProviderKind, Box<dyn SearchProvider>), SearchProviderConfigError> {
    let kind = match selector.map(str::trim).filter(|v| !v.is_empty()) {
        Some(value) => value.parse()?,
        None => SearchProviderKind::Searxng,
    };
    fn nonempty(value: Option<&str>) -> Option<&str> {
        value.map(str::trim).filter(|v| !v.is_empty())
    }
    match kind {
        SearchProviderKind::Searxng => {
            let url = nonempty(searxng_url).ok_or(
                SearchProviderConfigError::MissingConfiguration("SEARXNG_BASE_URL"),
            )?;
            #[cfg(feature = "search_searxng")]
            {
                Ok((kind, Box::new(crate::SearxngProvider::new(url))))
            }
            #[cfg(not(feature = "search_searxng"))]
            {
                let _ = url;
                Err(SearchProviderConfigError::UnsupportedProvider("searxng"))
            }
        }
        SearchProviderKind::Brave => {
            let key = nonempty(brave_key).ok_or(
                SearchProviderConfigError::MissingConfiguration("BRAVE_API_KEY"),
            )?;
            #[cfg(feature = "search_brave")]
            {
                Ok((kind, Box::new(crate::BraveProvider::new(key))))
            }
            #[cfg(not(feature = "search_brave"))]
            {
                let _ = key;
                Err(SearchProviderConfigError::UnsupportedProvider("brave"))
            }
        }
        SearchProviderKind::Serper => {
            let key = nonempty(serper_key).ok_or(
                SearchProviderConfigError::MissingConfiguration("SERPER_API_KEY"),
            )?;
            #[cfg(feature = "search_serper")]
            {
                Ok((kind, Box::new(crate::SerperProvider::new(key))))
            }
            #[cfg(not(feature = "search_serper"))]
            {
                let _ = key;
                Err(SearchProviderConfigError::UnsupportedProvider("serper"))
            }
        }
        SearchProviderKind::Tavily => {
            let key = nonempty(tavily_key).ok_or(
                SearchProviderConfigError::MissingConfiguration("TAVILY_API_KEY"),
            )?;
            #[cfg(feature = "search_tavily")]
            {
                Ok((kind, Box::new(crate::TavilyProvider::new(key))))
            }
            #[cfg(not(feature = "search_tavily"))]
            {
                let _ = key;
                Err(SearchProviderConfigError::UnsupportedProvider("tavily"))
            }
        }
        SearchProviderKind::Bing => Err(SearchProviderConfigError::UnsupportedProvider("bing")),
    }
}

/// Resolve the legacy SearXNG extensions while retaining shared selection
/// ownership for the ordinary provider trait.
#[cfg(feature = "search_searxng")]
pub fn resolve_searxng_provider(
    base_url: Option<&str>,
) -> Result<crate::SearxngProvider, SearchProviderConfigError> {
    let url = base_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(SearchProviderConfigError::MissingConfiguration(
            "SEARXNG_BASE_URL",
        ))?;
    Ok(crate::SearxngProvider::new(url))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_selector_preserves_searxng_legacy() {
        let (kind, provider) =
            resolve_search_provider(None, Some("http://localhost:8080"), None, None, None).unwrap();
        assert_eq!(kind, SearchProviderKind::Searxng);
        assert_eq!(provider.provider_name(), "searxng");
    }

    #[test]
    fn explicit_provider_requires_its_own_secret() {
        let err = resolve_search_provider(
            Some("brave"),
            Some("http://localhost:8080"),
            None,
            None,
            None,
        );
        assert!(matches!(
            err,
            Err(SearchProviderConfigError::MissingConfiguration(
                "BRAVE_API_KEY"
            ))
        ));
    }

    #[test]
    fn unknown_and_retired_providers_fail_closed() {
        assert!(matches!(
            resolve_search_provider(Some("unknown"), None, None, None, None),
            Err(SearchProviderConfigError::UnknownProvider(_))
        ));
        assert!(matches!(
            resolve_search_provider(Some("bing"), None, Some("secret"), None, None),
            Err(SearchProviderConfigError::UnsupportedProvider("bing"))
        ));
    }
}
