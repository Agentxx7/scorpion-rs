//! Canonical transport-request conversion for the CLI — the ONE place CLI
//! flags become `spider::features::transport::TransportPolicy`, reused by
//! both the one-shot discovery commands (fetch/feed/sitemap/news-sitemap/
//! robots-sitemap) and the crawl/scrape/download path. Neither path
//! constructs a `TransportPolicy` any other way.

use crate::options::sub_command::TransportModeArg;
use spider::features::transport::{TransportPolicy, TransportRequest};

/// Convert `--transport`/`--tor-proxy` into the canonical `TransportPolicy`.
/// Fails closed (returns `Err`) before any target networking on every
/// malformed combination — see `TransportRequest::into_policy`'s doc
/// comment for the exact matrix (mode=default+proxy, mode=tor+no proxy,
/// and any malformed Tor endpoint are all rejected here, never silently
/// coerced).
pub fn resolve(mode: TransportModeArg, proxy: Option<String>) -> Result<TransportPolicy, String> {
    TransportRequest {
        mode: mode.into(),
        proxy,
    }
    .into_policy()
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mode_no_proxy_resolves_to_default() {
        let policy = resolve(TransportModeArg::Default, None).unwrap();
        assert!(matches!(policy, TransportPolicy::Default));
    }

    #[test]
    fn default_mode_with_proxy_is_rejected() {
        assert!(resolve(
            TransportModeArg::Default,
            Some("socks5h://127.0.0.1:9050".into())
        )
        .is_err());
    }

    #[test]
    fn tor_mode_without_proxy_is_rejected() {
        assert!(resolve(TransportModeArg::Tor, None).is_err());
    }

    #[test]
    fn tor_mode_with_valid_proxy_resolves_to_tor() {
        let policy = resolve(
            TransportModeArg::Tor,
            Some("socks5h://127.0.0.1:9050".into()),
        )
        .unwrap();
        assert!(matches!(policy, TransportPolicy::Tor(_)));
    }

    #[test]
    fn tor_mode_with_malformed_proxy_is_rejected() {
        assert!(resolve(TransportModeArg::Tor, Some("http://127.0.0.1:9050".into())).is_err());
    }

    #[test]
    fn tor_mode_with_missing_port_is_rejected() {
        assert!(resolve(TransportModeArg::Tor, Some("socks5h://127.0.0.1".into())).is_err());
    }

    #[test]
    fn tor_mode_with_path_is_rejected() {
        assert!(resolve(
            TransportModeArg::Tor,
            Some("socks5h://127.0.0.1:9050/path".into())
        )
        .is_err());
    }

    #[test]
    fn tor_mode_with_credentials_is_rejected() {
        assert!(resolve(
            TransportModeArg::Tor,
            Some("socks5h://user:pass@127.0.0.1:9050".into())
        )
        .is_err());
    }
}
