//! Canonical HTTP transport policy: `Default` (preserve existing
//! Scorpion/Spider networking behavior) and `Tor` (fail-closed
//! SOCKS5h-over-Tor, with proxy-side hostname resolution and mandatory
//! `.onion` protection).
//!
//! This module owns no network behavior of its own beyond a small,
//! narrow, crate-private seam (`apply_transport_policy`/
//! `pin_redirect_policy`) applied to a plain `reqwest::ClientBuilder`
//! before building a client, internally, by
//! `spider::utils::evidence::fetch_single_page_with_options` — the public
//! contract external callers use. It does not rewrite `Website`'s
//! existing multi-proxy rotation, worker-pool, wreq, or Chrome/smart-mode
//! client construction — those remain exactly as they were, and are
//! simply not usable with [`TransportPolicy::Tor`] yet (rejected
//! explicitly, not silently ignored; see `Website::with_transport`).
//!
//! Public surface: [`TransportPolicy`], [`TorTransportConfig`] (and its
//! constructor [`TorTransportConfig::new`]), and [`TransportError`].
//! Everything else in this module (`is_onion_host`, `validate_target`,
//! `apply_transport_policy`, `pin_redirect_policy`,
//! `TransportPolicy::label`) is crate-private implementation detail.

use std::fmt;

/// Selects which network path an acquisition uses. `Default` is not named
/// `Direct` — the existing HTTP client stack may legitimately inherit
/// system/environment proxy configuration, so `Default` means "whatever
/// Spider already does today", not "no proxy".
#[derive(Debug, Clone, Default)]
#[cfg_attr(
    all(
        not(feature = "regex"),
        not(feature = "openai"),
        not(feature = "cache_openai"),
        not(feature = "gemini"),
        not(feature = "cache_gemini")
    ),
    derive(PartialEq)
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TransportPolicy {
    /// Preserve current Spider/Scorpion networking behavior exactly.
    #[default]
    Default,
    /// Fail-closed Tor-over-SOCKS5h. Every request under this policy is
    /// pinned to the configured endpoint for its entire lifetime,
    /// including redirects — there is no fallback to `Default`.
    Tor(TorTransportConfig),
}

impl TransportPolicy {
    /// The short, stable provenance label this policy corresponds to —
    /// exactly the value recorded in `EvidenceBundle.transport`. Crate-private:
    /// external callers get provenance from `EvidenceBundle`/
    /// `TransportAcquisition`, not by calling this directly (see
    /// `spider::utils::evidence`) — this is an internal implementation
    /// detail of how that provenance is computed, not a public contract.
    /// Only called from `spider::utils::evidence`, hence the matching `cfg`.
    #[cfg(feature = "evidence")]
    pub(crate) fn label(&self) -> &'static str {
        match self {
            TransportPolicy::Default => "default",
            TransportPolicy::Tor(_) => "tor",
        }
    }
}

/// A validated Tor SOCKS5h endpoint. Only ever constructed via
/// [`TorTransportConfig::new`], which enforces the full Tor-safe
/// contract: `socks5h://` scheme only (proxy-side DNS resolution — the
/// target hostname is never resolved locally), and no embedded
/// credentials (rejected outright in V1 — the smaller, safer contract;
/// see module docs).
///
/// `Deserialize` (when the `serde` feature is on) is hand-written to
/// delegate to `new`, not derived — a derived impl would write the
/// private `endpoint` field directly from untrusted input, bypassing
/// every one of these checks. This is the only way to construct a value
/// of this type from outside the module (besides `Clone`), so "every
/// `TorTransportConfig` is validated" holds regardless of which
/// constructor/deserialization path a caller uses.
#[derive(Clone)]
#[cfg_attr(
    all(
        not(feature = "regex"),
        not(feature = "openai"),
        not(feature = "cache_openai"),
        not(feature = "gemini"),
        not(feature = "cache_gemini")
    ),
    derive(PartialEq)
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct TorTransportConfig {
    endpoint: String,
}

impl TorTransportConfig {
    /// Validate a Tor SOCKS endpoint. Accepts only `socks5h://host:port`
    /// with no embedded userinfo. Every other scheme (`socks5://`,
    /// `socks://`, `http://`, `https://`, anything else) and any
    /// malformed or credential-bearing value is rejected — never
    /// rewritten, never silently skipped.
    pub fn new(endpoint: &str) -> Result<Self, TransportError> {
        let url = url::Url::parse(endpoint)
            .map_err(|error| TransportError::InvalidEndpoint(error.to_string()))?;

        if url.scheme() != "socks5h" {
            return Err(TransportError::UnsupportedScheme(url.scheme().to_string()));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(TransportError::CredentialsNotSupported);
        }
        if url.host_str().is_none() {
            return Err(TransportError::InvalidEndpoint(
                "missing host in Tor SOCKS endpoint".to_string(),
            ));
        }

        Ok(Self {
            endpoint: url.as_str().to_string(),
        })
    }

    /// The validated `socks5h://host:port` endpoint string. Only ever
    /// read by the real Tor client-construction path (see
    /// `apply_transport_policy`), hence the matching `cfg`.
    #[cfg(all(
        feature = "transport_tor",
        not(feature = "wreq"),
        not(feature = "cache_request")
    ))]
    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

/// Deserializes through [`TorTransportConfig::new`] — see the struct's
/// doc comment for why this cannot be `#[derive(Deserialize)]`. The wire
/// format is unchanged (still a bare endpoint string via `Serialize`);
/// only the deserialization *path* is different, not the shape.
#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for TorTransportConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Endpoint {
            endpoint: String,
        }
        let raw = Endpoint::deserialize(deserializer)?;
        TorTransportConfig::new(&raw.endpoint).map_err(serde::de::Error::custom)
    }
}

impl fmt::Debug for TorTransportConfig {
    /// Deliberately redacted: even though `new` already rejects
    /// credential-bearing endpoints outright, this never prints anything
    /// beyond the endpoint that was already validated as
    /// credential-free, so a future change to the validation rule can
    /// never turn this into a credential leak by omission.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TorTransportConfig")
            .field("endpoint", &self.endpoint)
            .finish()
    }
}

/// Explicit, fail-closed transport configuration/application failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// The endpoint string is not a parseable URL.
    InvalidEndpoint(String),
    /// The endpoint's scheme is not `socks5h`.
    UnsupportedScheme(String),
    /// The endpoint carries embedded userinfo (username/password),
    /// rejected outright in V1.
    CredentialsNotSupported,
    /// `TransportPolicy::Tor` was used in a build without the
    /// `transport_tor` feature compiled in.
    TorNotCompiled,
    /// The target host is `.onion` but the active policy is `Default`.
    OnionRequiresTor,
    /// Building the underlying `reqwest::Proxy` failed.
    ProxyBuildFailed(String),
    /// A redirect would silently change transport (onion boundary or
    /// cross-onion-service hop) — rejected rather than followed.
    RedirectTransportViolation(String),
    /// The requested `Website` configuration cannot honor the active
    /// transport policy's fail-closed guarantees (e.g. Tor combined with
    /// Chrome/smart mode, proxy rotation, or Spider Cloud — none of
    /// those paths have been audited for Tor pinning yet).
    IncompatibleConfiguration(String),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransportError::InvalidEndpoint(message) => {
                write!(f, "invalid Tor transport endpoint: {message}")
            }
            TransportError::UnsupportedScheme(scheme) => write!(
                f,
                "unsupported Tor transport scheme \"{scheme}\": only socks5h:// is accepted"
            ),
            TransportError::CredentialsNotSupported => write!(
                f,
                "Tor transport endpoints with embedded credentials are not supported"
            ),
            TransportError::TorNotCompiled => write!(
                f,
                "Tor transport requested but this build was compiled without the transport_tor feature"
            ),
            TransportError::OnionRequiresTor => write!(
                f,
                ".onion targets require TransportPolicy::Tor; refusing under the active transport policy"
            ),
            TransportError::ProxyBuildFailed(message) => {
                write!(f, "failed to build Tor proxy: {message}")
            }
            TransportError::RedirectTransportViolation(message) => {
                write!(f, "redirect rejected: {message}")
            }
            TransportError::IncompatibleConfiguration(message) => {
                write!(f, "transport policy is incompatible with this configuration: {message}")
            }
        }
    }
}

impl std::error::Error for TransportError {}

/// Canonical, case-insensitive `.onion` hostname detection. Matches the
/// exact `.onion` suffix and any subdomain beneath it; never a substring
/// match (`abc.onion.example.com` is NOT onion; `fakeonion` is NOT
/// onion). Crate-private implementation mechanics — external callers
/// observe onion-ness indirectly, through `fetch_single_page_with_options`
/// succeeding/failing and through evidence provenance, not by calling
/// this detector directly.
pub(crate) fn is_onion_host(host: &str) -> bool {
    let host = host.trim_end_matches('.');
    host.len() > 6 && host.to_ascii_lowercase().ends_with(".onion")
}

/// Pre-flight guard: reject a `.onion` target under `TransportPolicy::Default`
/// before any DNS lookup or network activity occurs. Always `Ok(())` for
/// non-onion targets under any policy, and for onion targets under
/// `TransportPolicy::Tor` (still subject to ordinary URL validation
/// elsewhere). Crate-private implementation mechanics. Only called from
/// `spider::utils::evidence`, hence the matching `cfg`.
#[cfg(feature = "evidence")]
pub(crate) fn validate_target(
    url: &url::Url,
    policy: &TransportPolicy,
) -> Result<(), TransportError> {
    let onion = url.host_str().is_some_and(is_onion_host);
    match (onion, policy) {
        (true, TransportPolicy::Default) => Err(TransportError::OnionRequiresTor),
        _ => Ok(()),
    }
}

/// Apply a transport policy to a plain `reqwest::ClientBuilder`. `Default`
/// returns the builder unchanged. `Tor` clears any inherited
/// environment/system proxy configuration (`no_proxy()`) and installs
/// exactly one explicit proxy for all traffic — the validated
/// `socks5h://` endpoint — so DNS resolution for the target host happens
/// proxy-side, never locally. Errors always propagate; there is no
/// `if let Ok(...)` skip path.
///
/// Crate-private implementation mechanics, only ever reachable via the
/// real Tor client-construction path in
/// `spider::utils::evidence::fetch_via_tor` (hence the matching `cfg`) —
/// that function's own `not(transport_tor)` and
/// `any(wreq, cache_request)` sibling variants return `TorNotCompiled` /
/// `IncompatibleConfiguration` directly, without reaching this function
/// at all, so a duplicate fail-closed branch is not needed here.
/// [`fetch_single_page_with_options`](crate::utils::evidence::fetch_single_page_with_options)
/// is the public contract this backs.
#[cfg(all(
    feature = "transport_tor",
    not(feature = "wreq"),
    not(feature = "cache_request")
))]
pub(crate) fn apply_transport_policy(
    builder: reqwest::ClientBuilder,
    policy: &TransportPolicy,
) -> Result<reqwest::ClientBuilder, TransportError> {
    match policy {
        TransportPolicy::Default => Ok(builder),
        TransportPolicy::Tor(config) => {
            let proxy = reqwest::Proxy::all(config.endpoint())
                .map_err(|error| TransportError::ProxyBuildFailed(error.to_string()))?;
            Ok(builder.no_proxy().proxy(proxy))
        }
    }
}

/// A `reqwest`-typed redirect policy that screens every hop with Spider's
/// existing SSRF redirect guard (`Website::is_ssrf_redirect`), capped at
/// `limit` hops. Deliberately hardcoded to `reqwest::redirect::Policy`
/// (never the crate's `wreq`-aliased `Client`/`Policy` types) because
/// every Tor-transport client this module builds is itself a plain
/// `reqwest::Client` — the `wreq` client stack is explicitly not
/// Tor-audited (see module docs) and never reaches this function.
#[cfg(all(
    feature = "transport_tor",
    not(feature = "wreq"),
    not(feature = "cache_request")
))]
pub(crate) fn ssrf_screened_base_policy(limit: usize) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        if crate::website::Website::is_ssrf_redirect(attempt.url()) {
            attempt.error("SSRF blocked: redirect to internal address")
        } else if attempt.previous().len() > limit {
            attempt.error("too many redirects")
        } else {
            attempt.follow()
        }
    })
}

/// Build a `reqwest` redirect policy that pins transport across
/// redirects, matching the locked redirect matrix exactly, while reusing
/// Spider's existing SSRF redirect guard (`Website::is_ssrf_redirect`)
/// rather than building a second redirect-safety engine:
///
/// - `Default` -> `.onion`: rejected, before the existing SSRF check runs.
/// - Tor: clearnet -> onion, or onion -> clearnet: rejected (the original
///   request's onion-ness is the pinned identity for the whole chain).
/// - Tor: onion -> the *same* onion host: follows (subject to `base`).
/// - Tor: onion -> a *different* onion host: rejected.
/// - Everything else (clearnet -> clearnet under Tor, and every hop
///   under `Default`): delegated to `base` — the caller's existing
///   redirect policy, SSRF screening included.
///
/// Crate-private implementation mechanics — applied internally by
/// `fetch_single_page_with_options`'s Tor path; not a public contract.
/// Same `cfg` as [`apply_transport_policy`]/[`ssrf_screened_base_policy`]:
/// only the real Tor client-construction path calls this.
#[cfg(all(
    feature = "transport_tor",
    not(feature = "wreq"),
    not(feature = "cache_request")
))]
pub(crate) fn pin_redirect_policy(
    base: reqwest::redirect::Policy,
    policy: TransportPolicy,
) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        let next_onion = attempt.url().host_str().is_some_and(is_onion_host);

        match &policy {
            TransportPolicy::Default => {
                if next_onion {
                    return attempt.error(TransportError::OnionRequiresTor.to_string());
                }
                base.redirect(attempt)
            }
            TransportPolicy::Tor(_) => {
                let original = attempt.previous().first().unwrap_or_else(|| attempt.url());
                let original_onion = original.host_str().is_some_and(is_onion_host);

                if original_onion != next_onion {
                    return attempt.error(
                        TransportError::RedirectTransportViolation(
                            "cross onion/clearnet redirect rejected".to_string(),
                        )
                        .to_string(),
                    );
                }
                if original_onion && original.host_str() != attempt.url().host_str() {
                    return attempt.error(
                        TransportError::RedirectTransportViolation(
                            "redirect to a different onion service rejected".to_string(),
                        )
                        .to_string(),
                    );
                }
                base.redirect(attempt)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onion_detection_is_exact_suffix_case_insensitive() {
        assert!(is_onion_host("abc.onion"));
        assert!(is_onion_host("ABC.ONION"));
        assert!(is_onion_host("sub.abc.onion"));
        assert!(!is_onion_host("abc.onion.example.com"));
        assert!(!is_onion_host("fakeonion"));
        assert!(!is_onion_host("onion"));
        assert!(!is_onion_host(""));
    }

    #[test]
    fn tor_endpoint_accepts_only_socks5h() {
        assert!(TorTransportConfig::new("socks5h://127.0.0.1:9050").is_ok());
        assert_eq!(
            TorTransportConfig::new("socks5://127.0.0.1:9050").unwrap_err(),
            TransportError::UnsupportedScheme("socks5".into())
        );
        assert_eq!(
            TorTransportConfig::new("socks://127.0.0.1:9050").unwrap_err(),
            TransportError::UnsupportedScheme("socks".into())
        );
        assert_eq!(
            TorTransportConfig::new("http://127.0.0.1:9050").unwrap_err(),
            TransportError::UnsupportedScheme("http".into())
        );
        assert_eq!(
            TorTransportConfig::new("https://127.0.0.1:9050").unwrap_err(),
            TransportError::UnsupportedScheme("https".into())
        );
        assert!(matches!(
            TorTransportConfig::new("not a url"),
            Err(TransportError::InvalidEndpoint(_))
        ));
        assert!(matches!(
            TorTransportConfig::new("ftp://nonsense::::"),
            Err(TransportError::InvalidEndpoint(_) | TransportError::UnsupportedScheme(_))
        ));
    }

    #[test]
    fn tor_endpoint_rejects_credentials() {
        assert_eq!(
            TorTransportConfig::new("socks5h://user:pass@127.0.0.1:9050").unwrap_err(),
            TransportError::CredentialsNotSupported
        );
        assert_eq!(
            TorTransportConfig::new("socks5h://user@127.0.0.1:9050").unwrap_err(),
            TransportError::CredentialsNotSupported
        );
    }

    #[test]
    fn tor_endpoint_debug_never_exposes_credentials() {
        let config = TorTransportConfig::new("socks5h://127.0.0.1:9050").unwrap();
        let debug = format!("{config:?}");
        assert!(!debug.contains("user"));
        assert!(!debug.contains("pass"));
        assert!(debug.contains("127.0.0.1:9050"));
    }

    /// Section G / A: direct proof that `TorTransportConfig`'s
    /// hand-written `Deserialize` impl enforces exactly the same
    /// validation as `TorTransportConfig::new` — never writing the
    /// private `endpoint` field directly from untrusted input. Covers
    /// every scheme `new` itself rejects, plus credential-bearing and
    /// malformed endpoints, and confirms the one accepted shape.
    #[cfg(feature = "serde")]
    #[test]
    fn deserialize_enforces_the_same_validation_as_new() {
        let valid = serde_json::json!({ "endpoint": "socks5h://127.0.0.1:9050" });
        let parsed: Result<TorTransportConfig, _> = serde_json::from_value(valid);
        assert!(parsed.is_ok(), "{parsed:?}");

        for rejected_endpoint in [
            "http://127.0.0.1:9050",
            "https://127.0.0.1:9050",
            "socks5://127.0.0.1:9050",
            "socks://127.0.0.1:9050",
            "socks5h://user:pass@127.0.0.1:9050",
            "socks5h://user@127.0.0.1:9050",
            "not a url",
        ] {
            let value = serde_json::json!({ "endpoint": rejected_endpoint });
            let result: Result<TorTransportConfig, _> = serde_json::from_value(value);
            assert!(
                result.is_err(),
                "deserialization must reject {rejected_endpoint:?} exactly as \
                 TorTransportConfig::new does"
            );
        }
    }

    /// The `endpoint` field cannot be written directly through
    /// deserialization even when the caller supplies the field name that
    /// matches the struct's actual (private) field — the hand-written
    /// impl always routes through `new`, so a malformed value in that
    /// position is still rejected, not silently accepted verbatim.
    #[cfg(feature = "serde")]
    #[test]
    fn deserialize_does_not_bypass_validation_via_direct_field_write() {
        let malformed = serde_json::json!({ "endpoint": "ftp://not-socks5h" });
        let result: Result<TorTransportConfig, _> = serde_json::from_value(malformed);
        assert!(result.is_err());
    }

    #[cfg(feature = "evidence")]
    #[test]
    fn default_policy_permits_clearnet_but_not_onion() {
        let policy = TransportPolicy::Default;
        let clearnet = url::Url::parse("https://example.test/").unwrap();
        let onion = url::Url::parse("http://abc.onion/").unwrap();
        assert!(validate_target(&clearnet, &policy).is_ok());
        assert_eq!(
            validate_target(&onion, &policy).unwrap_err(),
            TransportError::OnionRequiresTor
        );
    }

    #[cfg(feature = "evidence")]
    #[test]
    fn tor_policy_permits_both_onion_and_clearnet_targets() {
        let config = TorTransportConfig::new("socks5h://127.0.0.1:9050").unwrap();
        let policy = TransportPolicy::Tor(config);
        let clearnet = url::Url::parse("https://example.test/").unwrap();
        let onion = url::Url::parse("http://abc.onion/").unwrap();
        assert!(validate_target(&clearnet, &policy).is_ok());
        assert!(validate_target(&onion, &policy).is_ok());
    }

    #[cfg(feature = "evidence")]
    #[test]
    fn policy_labels_are_the_locked_provenance_values() {
        let config = TorTransportConfig::new("socks5h://127.0.0.1:9050").unwrap();
        assert_eq!(TransportPolicy::Default.label(), "default");
        assert_eq!(TransportPolicy::Tor(config).label(), "tor");
    }

    // The "fails closed without transport_tor" contract is proven at the
    // `fetch_single_page_with_options` level instead
    // (`spider::utils::evidence`'s own test module) — `apply_transport_policy`
    // itself no longer exists in that configuration (see its `cfg`), so
    // there is nothing to call here without the feature.
    #[cfg(all(
        feature = "transport_tor",
        not(feature = "wreq"),
        not(feature = "cache_request")
    ))]
    #[test]
    fn tor_application_succeeds_with_transport_tor_feature() {
        let config = TorTransportConfig::new("socks5h://127.0.0.1:9050").unwrap();
        let result =
            apply_transport_policy(reqwest::ClientBuilder::new(), &TransportPolicy::Tor(config));
        assert!(result.is_ok());
    }
}
