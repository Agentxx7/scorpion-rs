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
//! constructor [`TorTransportConfig::new`]), [`TransportError`],
//! [`is_onion_url`] — the canonical `.onion` URL classifier —
//! [`validate_target`] — the canonical fail-closed URL/transport
//! compatibility guard — and [`execute_streaming_request`] — the
//! canonical streaming request/response seam (status, final URL,
//! headers, and an unconsumed async body stream, without collecting the
//! body or constructing a `Page`). All of these are pure
//! or, for [`execute_streaming_request`], stop the instant response
//! metadata is established — no body byte is read on this module's
//! behalf. Everything else in this module (`is_onion_host`,
//! `apply_transport_policy`, `pin_redirect_policy`,
//! `TransportPolicy::label`) is crate-private implementation detail.

use std::fmt;

/// Selects which network path an acquisition uses. `Default` is not named
/// `Direct` — the existing HTTP client stack may legitimately inherit
/// system/environment proxy configuration, so `Default` means "whatever
/// Spider already does today", not "no proxy".
#[derive(Debug, Clone, Default, PartialEq)]
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
#[derive(Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct TorTransportConfig {
    endpoint: String,
}

impl TorTransportConfig {
    /// Validate a Tor SOCKS endpoint. Accepts **exactly** an authority
    /// endpoint — `socks5h://HOST:PORT` — and nothing else:
    ///
    /// - scheme must be `socks5h` (never `socks5://`, `socks://`,
    ///   `http(s)://`, or anything else)
    /// - a host is required
    /// - a port is required *explicitly* — there is no implicit/default
    ///   Tor port, so `socks5h://127.0.0.1` (no port) is rejected exactly
    ///   like a missing host
    /// - no userinfo/credentials (`user:pass@`, `user@`)
    /// - no path, query, or fragment — `socks5h://127.0.0.1:9050/`,
    ///   `?x=1`, or `#frag` are all rejected; this is an authority-only
    ///   endpoint, not a request URL
    ///
    /// Every malformed or out-of-grammar value is rejected outright —
    /// never rewritten, never silently normalized, never silently
    /// skipped. There is no environment-derived or implicit default Tor
    /// endpoint anywhere in this crate; a caller must always supply one
    /// explicitly.
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
        if url.port().is_none() {
            return Err(TransportError::InvalidEndpoint(
                "missing explicit port in Tor SOCKS endpoint — there is no default Tor port"
                    .to_string(),
            ));
        }
        if !url.path().is_empty() {
            return Err(TransportError::InvalidEndpoint(
                "Tor SOCKS endpoint must not carry a path".to_string(),
            ));
        }
        if url.query().is_some() {
            return Err(TransportError::InvalidEndpoint(
                "Tor SOCKS endpoint must not carry a query string".to_string(),
            ));
        }
        if url.fragment().is_some() {
            return Err(TransportError::InvalidEndpoint(
                "Tor SOCKS endpoint must not carry a fragment".to_string(),
            ));
        }

        Ok(Self {
            endpoint: url.as_str().to_string(),
        })
    }

    /// The validated `socks5h://host:port` endpoint string. Only ever
    /// read by the real Tor client-construction path (see
    /// `apply_transport_policy`), hence the matching `cfg`.
    #[cfg(feature = "tor")]
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
    /// A built client failed to execute the request — connect/TLS/network
    /// failure, or a redirect hop that [`pin_redirect_policy`]/
    /// [`ssrf_screened_base_policy`] rejected. `reqwest` itself does not
    /// distinguish a policy-rejected redirect from any other mid-request
    /// failure at the type level (both surface through the same
    /// `reqwest::Error`), so this variant does not invent a distinction
    /// the underlying library doesn't make; the message text carries
    /// whichever reason actually applied. Always established *before* any
    /// response body byte is read — see [`execute_streaming_request`].
    RequestExecutionFailed(String),
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
            TransportError::RequestExecutionFailed(message) => {
                write!(f, "transport request execution failed: {message}")
            }
        }
    }
}

impl std::error::Error for TransportError {}

/// Stable, wire/user-facing transport intent — deliberately **not** the
/// same shape as [`TransportPolicy`]. `TransportPolicy` is Spider's
/// internal execution vocabulary (and `TorTransportConfig` inside it
/// enforces validation at construction, not at rest); `TransportRequest`
/// is the small, stable DTO a public surface (Scorpion CLI, MCP) parses
/// *its own* wire format into, then converts here exactly once via
/// [`TransportRequest::into_policy`]. Neither the CLI nor MCP should ever
/// serialize/deserialize a raw `TransportPolicy` as their public contract
/// — this type is the seam that keeps that from happening, and the one
/// place both surfaces' request-validation logic actually lives, so it
/// can never independently drift between them.
///
/// `Default::default()` is `TransportMode::Default` with no proxy — the
/// same backward-compatible "existing behavior" starting point every
/// other transport-aware default in this crate uses.
#[derive(Debug, Clone, Default)]
pub struct TransportRequest {
    /// Which transport family this request selects.
    pub mode: TransportMode,
    /// The Tor SOCKS5h endpoint, required when `mode` is
    /// [`TransportMode::Tor`] and meaningless (must be `None`) when
    /// `mode` is [`TransportMode::Default`] — see [`into_policy`](Self::into_policy)
    /// for the exact validation matrix.
    pub proxy: Option<String>,
}

/// The transport family half of a [`TransportRequest`]. Intentionally a
/// closed two-variant enum: an unrecognized mode string is a public
/// surface's own parsing/deserialization concern (reject before this type
/// is ever constructed), not something this type represents as a third
/// "unknown" state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransportMode {
    /// Preserve current Spider/Scorpion networking behavior exactly.
    #[default]
    Default,
    /// Fail-closed Tor-over-SOCKS5h — see [`TransportPolicy::Tor`].
    Tor,
}

impl TransportRequest {
    /// Convert this request into the canonical [`TransportPolicy`] Spider
    /// actually executes against — the one and only place this
    /// conversion happens, reused verbatim by both the CLI and MCP
    /// adapters. Fails closed, before any target networking, on every
    /// malformed combination:
    ///
    /// | `mode`    | `proxy`    | Result                                   |
    /// |-----------|------------|-------------------------------------------|
    /// | `Default` | `None`     | `Ok(TransportPolicy::Default)`             |
    /// | `Default` | `Some(_)`  | `Err` — a proxy without `mode = tor` is a request-shape error, not silently ignored |
    /// | `Tor`     | `Some(ep)` | `Ok(TransportPolicy::Tor(..))` if `ep` validates (see [`TorTransportConfig::new`]), else that same `Err` |
    /// | `Tor`     | `None`     | `Err` — Tor requires an explicit endpoint; there is no implicit/default Tor proxy |
    pub fn into_policy(self) -> Result<TransportPolicy, TransportError> {
        match (self.mode, self.proxy) {
            (TransportMode::Default, None) => Ok(TransportPolicy::Default),
            (TransportMode::Default, Some(_)) => Err(TransportError::IncompatibleConfiguration(
                "a proxy endpoint was supplied but mode is \"default\" — set mode to \"tor\" \
                 to use it, or omit the proxy field"
                    .to_string(),
            )),
            (TransportMode::Tor, Some(endpoint)) => {
                TorTransportConfig::new(&endpoint).map(TransportPolicy::Tor)
            }
            (TransportMode::Tor, None) => Err(TransportError::IncompatibleConfiguration(
                "mode is \"tor\" but no proxy endpoint was supplied — Tor requires an explicit \
                 socks5h://HOST:PORT endpoint; there is no implicit or environment-derived default"
                    .to_string(),
            )),
        }
    }
}

/// Canonical, case-insensitive `.onion` hostname detection. Matches the
/// exact `.onion` suffix and any subdomain beneath it; never a substring
/// match (`abc.onion.example.com` is NOT onion; `fakeonion` is NOT
/// onion). Crate-private: operates on a bare host string, not a full URL
/// — [`is_onion_url`] is the public, URL-level wrapper acquisition-time
/// (`validate_target`, `pin_redirect_policy`, `CrawlBoundary`) and
/// discovery-time (`spider::features::onion_seed`) callers actually use.
pub fn is_onion_host(host: &str) -> bool {
    let host = host.trim_end_matches('.');
    host.len() > 6 && host.to_ascii_lowercase().ends_with(".onion")
}

/// Canonical, public `.onion` URL classifier. `true` exactly when `url`
/// has a host and that host is `.onion` per [`is_onion_host`] — the same
/// detection Tor acquisition's fail-closed `.onion` protection
/// (`validate_target`), redirect pinning (`pin_redirect_policy`), and
/// crawl-boundary checks (`CrawlBoundary`) already rely on internally,
/// exposed here as the one canonical seam so discovery code
/// (`spider::features::onion_seed`) never reimplements `.ends_with(".onion")`
/// or equivalent matching on its own.
///
/// Pure classification, no network activity: this does not confirm the
/// hidden service is reachable, does not validate Onion v2/v3 address
/// structure beyond what `url::Url` itself requires to parse a host, and
/// makes no cryptographic claim about the address. Userinfo
/// (`user:pass@`) never participates — only `url.host_str()` is
/// inspected — and path/query/fragment never affect the result.
pub fn is_onion_url(url: &url::Url) -> bool {
    url.host_str().is_some_and(is_onion_host)
}

/// Canonical pre-flight URL/transport compatibility guard. Rejects a
/// `.onion` target under [`TransportPolicy::Default`] before any DNS lookup
/// or network activity occurs. Always `Ok(())` for non-onion targets under
/// either policy, and for onion targets under [`TransportPolicy::Tor`].
/// There is no fallback, policy upgrade, or coercion.
///
/// The caller supplies an already-parsed [`url::Url`]. This seam deliberately
/// does **not** define general target-scheme policy: it neither parses malformed
/// input nor rejects schemes such as `ftp`, `file`, or `data`. Callers remain
/// responsible for any operation-specific scheme restrictions before or after
/// this compatibility check.
///
/// Pure validation only: no DNS, sockets, filesystem access, environment
/// lookup, authentication, or request execution. Available without the
/// `evidence` or `transport_tor` features.
pub fn validate_target(url: &url::Url, policy: &TransportPolicy) -> Result<(), TransportError> {
    let onion = is_onion_url(url);
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
/// `fetch_via_tor` (hence the matching `cfg`) —
/// that function's own `not(transport_tor)` and
/// `any(wreq, cache_request)` sibling variants return `TorNotCompiled` /
/// `IncompatibleConfiguration` directly, without reaching this function
/// at all, so a duplicate fail-closed branch is not needed here.
/// `fetch_single_page_with_options`
/// is the public contract this backs.
#[cfg(feature = "tor")]
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

/// The canonical SSRF redirect classifier: `true` when `url` points into
/// internal/link-local/loopback/metadata address space (or carries a
/// non-http(s) scheme or no host) and a redirect to it must therefore be
/// blocked.
///
/// This is the single owner of the SSRF internal-address classification
/// logic, extracted below `Website` (whose redirect policies delegate to
/// it) so every canonical consumer shares exactly one implementation.
/// Onion transport requirements are deliberately NOT folded in here —
/// `.onion` handling is context-dependent and enforced by callers that
/// know the active [`TransportPolicy`] (see [`validate_target`] and
/// `pin_redirect_policy`).
///
/// Pure classification on the already-parsed `url::Url`: no DNS, no
/// network, no allocation per hop.
pub fn is_ssrf_redirect(url: &url::Url) -> bool {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    fn is_internal_v4(v4: Ipv4Addr) -> bool {
        v4.is_loopback()
            || v4.is_private()
            || v4.is_link_local()
            || v4.is_unspecified()
            || v4.is_broadcast()
            // RFC 1122 reserves all of 0.0.0.0/8 as "this network";
            // `is_unspecified` only matches 0.0.0.0 itself.
            || v4.octets()[0] == 0
    }

    fn is_internal_v6(v6: Ipv6Addr) -> bool {
        v6.is_loopback()
            || v6.is_unspecified()
            // fc00::/7 is the IPv6 side of RFC 1918, and where the
            // cloud metadata service answers (fd00:ec2::254).
            || v6.is_unique_local()
            // fe80::/10 is the IPv6 side of 169.254.0.0/16.
            || v6.is_unicast_link_local()
            || v6.to_ipv4_mapped().is_some_and(is_internal_v4)
    }

    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return true;
    }
    let host = match url.host_str() {
        Some(h) => h,
        None => return true,
    };
    if host == "localhost"
        || host == "0.0.0.0"
        || host.ends_with(".localhost")
        || host == "[::1]"
        || host == "[::0]"
    {
        return true;
    }
    if host == "169.254.169.254" || host == "metadata.google.internal" || host == "metadata.goog" {
        return true;
    }
    // `url` serializes IPv6 hosts with brackets; strip one pair so
    // bracketed / IPv4-mapped literals can't bypass the parse.
    let ip_host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    match ip_host.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => is_internal_v4(v4),
        Ok(IpAddr::V6(v6)) => is_internal_v6(v6),
        _ => false,
    }
}

/// A `reqwest`-typed redirect policy that screens every hop with the
/// canonical SSRF redirect guard ([`is_ssrf_redirect`]), capped at
/// `limit` hops. Deliberately hardcoded to `reqwest::redirect::Policy`
/// (never the crate's `wreq`-aliased `Client`/`Policy` types) because
/// every client this module builds (Tor *or* the streaming Default
/// client — see [`build_streaming_client`]) is itself a plain
/// `reqwest::Client` — the `wreq` client stack is explicitly not
/// audited by this module (see module docs) and never reaches this
/// function. Contains no Tor-specific logic (it never reads
/// `TorTransportConfig`), so — unlike [`apply_transport_policy`] and
/// [`build_tor_client`], which actually construct the Tor proxy and stay
/// gated behind `tor` — this is available whenever the plain
/// `reqwest` client stack is in use, Tor-capable build or not.
pub(crate) fn ssrf_screened_base_policy(limit: usize) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        if is_ssrf_redirect(attempt.url()) {
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
/// the canonical SSRF redirect guard ([`is_ssrf_redirect`])
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
/// `fetch_single_page_with_options`'s Tor path and by
/// [`build_streaming_client`]'s Default path; not a public contract.
/// Contains no Tor-specific logic of its own beyond pattern-matching on
/// [`TransportPolicy`] itself (never reads `TorTransportConfig`'s
/// endpoint), so this is available under the same `cfg` as
/// [`ssrf_screened_base_policy`] — not narrowed to `transport_tor`.
pub(crate) fn pin_redirect_policy(
    base: reqwest::redirect::Policy,
    policy: TransportPolicy,
) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        let next_onion = is_onion_url(attempt.url());

        match &policy {
            TransportPolicy::Default => {
                if next_onion {
                    return attempt.error(TransportError::OnionRequiresTor.to_string());
                }
                base.redirect(attempt)
            }
            TransportPolicy::Tor(_) => {
                let original = attempt.previous().first().unwrap_or_else(|| attempt.url());
                let original_onion = is_onion_url(original);

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

/// Sanitized, network-acquisition-only transport provenance carried by
/// `Page`. Deliberately minimal: no SOCKS endpoint, no
/// credentials, no full [`TransportPolicy`] — just which of the two
/// audited routes actually performed the fetch. `Page`'s field is
/// private; only the acquisition code paths in this crate that actually
/// dispatch a request may stamp it (see
/// `Page::transport`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquisitionTransport {
    /// Fetched over Spider's existing default networking behavior.
    Default,
    /// Fetched over the audited fail-closed Tor SOCKS5h transport.
    Tor,
}

impl AcquisitionTransport {
    /// The short, stable provenance label — exactly the value recorded in
    /// `EvidenceBundle.transport`.
    pub fn label(&self) -> &'static str {
        match self {
            AcquisitionTransport::Default => "default",
            AcquisitionTransport::Tor => "tor",
        }
    }
}

tokio::task_local! {
    /// Ambient signal for "the page(s) built for the rest of this async
    /// task tree were acquired over this transport". Established once per
    /// acquisition context (one-shot Tor/Default fetch, or one multi-page
    /// crawl) and propagated into spawned worker tasks by
    /// `utils::spawn_set` re-entering the same scope — mirroring
    /// the existing `WEBSITE_SPOOL_DIR` task-local
    /// (`utils::html_spool`), the established pattern in this
    /// crate for ambient per-crawl context that must survive `tokio::spawn`
    /// boundaries without threading a new parameter through every
    /// intermediate function signature.
    ///
    /// Two independent readers consume this:
    /// - `page::build` stamps `Page::transport` from it — the
    ///   only writer of that field.
    /// - `page::host_resolves_locally_cached` refuses to perform
    ///   or consult any local DNS lookup for the target host while this
    ///   scope reads `Tor` — see [`target_dns_suppressed`].
    pub static ACQUISITION_TRANSPORT_SCOPE: AcquisitionTransport;
}

/// The transport of the enclosing [`ACQUISITION_TRANSPORT_SCOPE`], or
/// `None` when called outside any such scope (e.g. code paths this
/// frontier does not audit, or ordinary non-crawl test code).
pub fn current_acquisition_transport() -> Option<AcquisitionTransport> {
    ACQUISITION_TRANSPORT_SCOPE.try_with(|value| *value).ok()
}

/// `true` only when the enclosing acquisition is genuinely Tor. Never
/// `true` by default, never `true` outside an explicit scope — the
/// suppression this gates (see callers) must never activate for a
/// context nobody positively marked as Tor.
pub fn target_dns_suppressed() -> bool {
    current_acquisition_transport() == Some(AcquisitionTransport::Tor)
}

/// The `AcquisitionTransport` this fixed `TransportPolicy` corresponds to
/// — the value a crawl's outer `ACQUISITION_TRANSPORT_SCOPE` must be
/// entered with for its whole duration.
pub fn acquisition_transport_for(policy: &TransportPolicy) -> AcquisitionTransport {
    match policy {
        TransportPolicy::Default => AcquisitionTransport::Default,
        TransportPolicy::Tor(_) => AcquisitionTransport::Tor,
    }
}

/// The seed-derived boundary a candidate URL must satisfy before it may
/// be admitted to the crawl frontier or fetched — see module docs on
/// [`crawl_boundary_allows`] for the exact matrix. Deliberately not
/// merged into `Website::is_allowed`: this is a transport-security
/// concern (can this request happen at all, over this pinned transport),
/// not a crawl-policy concern (depth/budget/robots/allow-deny lists).
#[derive(Debug, Clone)]
pub enum CrawlBoundary {
    /// Default transport, or Tor pinned to a clearnet seed: clearnet
    /// candidates are allowed (subject to existing crawl policy
    /// elsewhere); `.onion` candidates are always rejected. Whether this
    /// specific clearnet crawl runs over Tor or Default doesn't change
    /// the boundary decision itself (onion-ness is what's being pinned,
    /// not which non-onion route carries the traffic) — the transport is
    /// already fixed and enforced elsewhere (`tor_crawl_preflight`,
    /// `pin_redirect_policy`), so it isn't duplicated here.
    Clearnet,
    /// Tor pinned to an onion seed: only the exact same onion hostname
    /// (case-insensitive) is allowed; every clearnet candidate and every
    /// other onion service is rejected. Lowercased once at construction
    /// so every comparison is a cheap, already-normalized `==`.
    Onion { host: String },
}

impl CrawlBoundary {
    /// Derive the boundary for a crawl from its transport policy and seed
    /// URL. `.onion` seeds under `Default` transport are never
    /// constructible here — that combination is already rejected at
    /// preflight before a boundary is ever derived (see
    /// `Website::tor_crawl_preflight`).
    pub fn from_seed(policy: &TransportPolicy, seed: &url::Url) -> Self {
        let seed_onion = is_onion_url(seed);
        match (policy, seed_onion) {
            (TransportPolicy::Tor(_), true) => CrawlBoundary::Onion {
                host: seed.host_str().unwrap_or_default().to_ascii_lowercase(),
            },
            (TransportPolicy::Tor(_), false) | (TransportPolicy::Default, _) => {
                CrawlBoundary::Clearnet
            }
        }
    }
}

/// Ports never define onion-service identity (a `.onion` address has no
/// notion of "the same service on a different port" — the hostname alone
/// is the identity), so this compares hosts only, case-insensitively,
/// normalized the same way [`CrawlBoundary::from_seed`] and the redirect
/// pinning in [`pin_redirect_policy`] already normalize `.onion` hosts.
pub fn crawl_boundary_allows(boundary: &CrawlBoundary, candidate: &url::Url) -> bool {
    let candidate_onion = is_onion_url(candidate);
    match boundary {
        CrawlBoundary::Clearnet => !candidate_onion,
        CrawlBoundary::Onion { host } => {
            candidate_onion
                && candidate
                    .host_str()
                    .is_some_and(|h| h.to_ascii_lowercase() == *host)
        }
    }
}

/// Connect/read timeouts for this module's audited clients, reused
/// verbatim from `Website::configure_base_client`'s own *unmultiplied*
/// defaults (`Duration::from_secs(24)` / `Duration::from_secs(42)`) — not
/// a Tor-specific invention (see doc history on this constant), which is
/// exactly why [`build_streaming_client`]'s Default path reuses the same
/// values rather than inventing a second baseline. `configure_base_client`
/// doubles these only when Spider's legacy multi-proxy rotation list
/// (`configuration.proxies`) is configured; neither the dedicated Tor
/// client nor the streaming Default client ever uses that list (rejected
/// explicitly at preflight — see `Website::tor_crawl_preflight`), so the
/// unmultiplied canonical values are the correct match, and they provide
/// the hard bound that keeps a stalled/blackhole handshake from waiting
/// indefinitely. Shared by the one-shot Tor seam
/// (`fetch_via_tor`), multi-page Tor crawling,
/// and [`build_streaming_client`] — one canonical set of constants, not
/// several.
pub(crate) const TOR_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(24);
pub(crate) const TOR_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(42);

/// Total request deadline for this module's audited clients, reused
/// verbatim from `Configuration::new()`'s own default `request_timeout`
/// (`Duration::from_secs(120)`), applied via `reqwest::ClientBuilder::timeout`.
///
/// Connect/read timeouts alone are not sufficient: a peer that keeps the
/// connection alive and periodically sends enough bytes to keep resetting
/// [`TOR_READ_TIMEOUT`] (a slow-drip response) would never trip either of
/// them, and could otherwise stall an acquisition indefinitely.
/// `.timeout()` bounds the request end-to-end — connect, redirects, and
/// response body — regardless of how activity is paced within it. Also
/// used by [`build_streaming_client`]'s Default path.
pub(crate) const TOR_TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Redirect hop cap for this module's audited clients, reused verbatim
/// from `Configuration::new()`'s own default `redirect_limit` (`7`). Also
/// used by [`build_streaming_client`]'s Default path.
pub(crate) const TOR_REDIRECT_LIMIT: usize = 7;

/// Build the one canonical Tor-audited `reqwest::Client`: no
/// environment/system proxy inheritance, exactly one explicit SOCKS5h
/// proxy, bounded connect/read/total timeouts, a redirect policy that
/// pins transport across hops while reusing Spider's existing SSRF
/// redirect guard, and Spider's own default user-agent (so a Tor fetch is
/// not distinguishable from a Default fetch by header fingerprint alone).
///
/// This is the single reusable transport building primitive — both the
/// one-shot seam (`fetch_via_tor`) and multi-page
/// Tor crawling (`Website::tor_crawl_preflight`) call this instead of
/// each independently constructing a Tor client; there is exactly one Tor
/// client implementation in this crate.
///
/// Hardcoded to return a plain `reqwest::Client` — never the crate's
/// aliased `Client` type, which resolves to `wreq::Client` or
/// `reqwest_middleware::ClientWithMiddleware` under the `wreq`/
/// `cache_request` features respectively. Neither of those alternate
/// stacks has been audited for the fail-closed guarantees this function
/// requires (no environment proxy inheritance, no target DNS
/// pre-resolution, transport-pinned redirects), so that combination is
/// rejected explicitly at the (matching) `cfg` boundary rather than
/// silently used — callers outside this `cfg` must reject Tor themselves
/// (see `Website::tor_crawl_preflight` and `evidence::fetch_via_tor`'s
/// sibling variants).
#[cfg(feature = "tor")]
pub fn build_tor_client(
    policy: &TransportPolicy,
    user_agent: &str,
) -> Result<reqwest::Client, TransportError> {
    let builder = apply_transport_policy(reqwest::Client::builder(), policy)?;
    let builder = builder
        .connect_timeout(TOR_CONNECT_TIMEOUT)
        .read_timeout(TOR_READ_TIMEOUT)
        .timeout(TOR_TOTAL_TIMEOUT)
        .user_agent(user_agent)
        .redirect(pin_redirect_policy(
            ssrf_screened_base_policy(TOR_REDIRECT_LIMIT),
            policy.clone(),
        ));
    builder
        .build()
        .map_err(|error| TransportError::ProxyBuildFailed(error.to_string()))
}

/// Build the one canonical **streaming-capable** `reqwest::Client` for a
/// given [`TransportPolicy`] — `Default` or `Tor`. This is the transport
/// primitive behind [`execute_streaming_request`]; it never issues a
/// request itself, only constructs the client.
///
/// - `Default`: a plain `reqwest::Client` with no proxy, the same
///   SSRF-screened, transport-pinned redirect policy, connect/read/total
///   timeouts, and user-agent as the audited Tor client (see
///   [`TOR_CONNECT_TIMEOUT`] and neighbors — reused verbatim, not
///   Tor-specific values; see their doc comments). This is deliberately
///   **not** `Website::configure_base_client`'s full behavior (legacy
///   proxy rotation, per-crawl header/DNS-cache/Spider-Cloud
///   configuration) — this module owns a small, narrow, independently
///   audited client, the same architectural choice already made for Tor
///   (see module docs); it does not rewrite or reach into `Website`'s own
///   client construction, and constructs no `Website` value.
/// - `Tor`: when `transport_tor` is compiled in, delegates entirely to
///   the existing, already-audited [`build_tor_client`] — there is
///   exactly one Tor client implementation in this crate, not two. When
///   `transport_tor` is *not* compiled in, fails closed with
///   [`TransportError::TorNotCompiled`] — the same honest failure
///   `fetch_via_tor`'s own `not(transport_tor)`
///   sibling variant already returns — Default execution is unaffected.
#[cfg(feature = "tor")]
pub(crate) fn build_streaming_client(
    policy: &TransportPolicy,
    user_agent: &str,
) -> Result<reqwest::Client, TransportError> {
    match policy {
        TransportPolicy::Default => build_default_streaming_client(user_agent),
        TransportPolicy::Tor(_) => build_tor_client(policy, user_agent),
    }
}

/// See [`build_streaming_client`] above (the `transport_tor`-compiled
/// variant). This sibling exists for builds *without* `transport_tor`:
/// `Default` streaming execution does not need Tor at all, so it must
/// keep working; only `Tor` fails closed here.
#[cfg(not(feature = "tor"))]
pub(crate) fn build_streaming_client(
    policy: &TransportPolicy,
    user_agent: &str,
) -> Result<reqwest::Client, TransportError> {
    match policy {
        TransportPolicy::Default => build_default_streaming_client(user_agent),
        TransportPolicy::Tor(_) => Err(TransportError::TorNotCompiled),
    }
}

/// The `Default`-policy half of [`build_streaming_client`], factored out
/// so both the `transport_tor`-on and `transport_tor`-off variants build
/// an identical Default client from one definition rather than two
/// copies that could silently drift apart.
fn build_default_streaming_client(user_agent: &str) -> Result<reqwest::Client, TransportError> {
    reqwest::Client::builder()
        .connect_timeout(TOR_CONNECT_TIMEOUT)
        .read_timeout(TOR_READ_TIMEOUT)
        .timeout(TOR_TOTAL_TIMEOUT)
        .user_agent(user_agent)
        .redirect(pin_redirect_policy(
            ssrf_screened_base_policy(TOR_REDIRECT_LIMIT),
            TransportPolicy::Default,
        ))
        .build()
        .map_err(|error| TransportError::ProxyBuildFailed(error.to_string()))
}

/// Execute one canonical, streaming, **non-body-consuming** HTTP GET
/// against `url` under `policy`, applying `headers`, and return the
/// moment response status/final URL/headers are established — before any
/// response body byte is read.
///
/// This is the smallest transport-owned request/response seam:
///
/// ```text
/// url + TransportPolicy + SecretRequestHeaders
///       │
///       ▼
/// validate_target (fail-closed .onion/Default rejection)
///       │
///       ▼
/// build_streaming_client (Default or Tor, same audited primitives
///                          fetch_via_tor/build_tor_client already use)
///       │
///       ▼
/// apply SecretRequestHeaders::apply_to
///       │
///       ▼
/// client.get(url).send().await   <- stops here; body untouched
///       │
///       ▼
/// Ok(reqwest::Response)  <- .status()/.url()/.headers() available now;
///                            .bytes_stream()/.chunk() left for the
///                            caller to consume, on their own schedule
/// ```
///
/// Deliberately returns the plain `reqwest::Response` itself rather than
/// a crate-defined wrapper: status, final URL (post-redirect), headers,
/// and an unconsumed `impl Stream<Item = Result<bytes::Bytes,
/// reqwest::Error>>` (via `.bytes_stream()`, the `stream` reqwest feature
/// already enabled crate-wide) are all already exactly what
/// `reqwest::Response` exposes without consuming the body — inventing a
/// second type here would just re-export the same accessors under a new
/// name. This function never calls `.bytes()`, `.text()`, `.chunk()`, or
/// `.bytes_stream()` itself, never constructs a `Page`,
/// and never constructs a `Website`.
///
/// Mid-stream body failures are **not** represented in this function's
/// `Result` — they cannot be: the body has not been read yet when this
/// function returns `Ok`. They surface later, truthfully, as `Err` items
/// yielded by the caller's own consumption of `.bytes_stream()` (a
/// `reqwest::Error` per chunk) — exactly `reqwest::Response`'s existing,
/// well-defined contract; nothing about that contract is reimplemented or
/// weakened here.
///
/// Fails closed exactly like every other acquisition seam in this crate:
/// `.onion` targets under `Default` are rejected by `validate_target`
/// before any client is built; `Tor` under a build without
/// `transport_tor` fails with [`TransportError::TorNotCompiled`] before
/// any network activity; a redirect that would change transport or trip
/// the SSRF guard is rejected mid-request by the exact same
/// `pin_redirect_policy`/`ssrf_screened_base_policy` the audited Tor
/// client already uses.
pub async fn execute_streaming_request(
    url: &url::Url,
    policy: &TransportPolicy,
    headers: &crate::secret_request_headers::SecretRequestHeaders,
    user_agent: &str,
) -> Result<reqwest::Response, TransportError> {
    validate_target(url, policy)?;
    let client = build_streaming_client(policy, user_agent)?;

    let mut request_headers = reqwest::header::HeaderMap::new();
    headers.apply_to(&mut request_headers);

    let request = client.get(url.clone()).headers(request_headers);

    ACQUISITION_TRANSPORT_SCOPE
        .scope(acquisition_transport_for(policy), request.send())
        .await
        .map_err(|error| TransportError::RequestExecutionFailed(error.to_string()))
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

    /// The public URL-level wrapper agrees with the crate-private
    /// host-level detector exactly, and ignores userinfo/path/query/
    /// fragment — only `host_str()` participates.
    #[test]
    fn is_onion_url_matches_is_onion_host_and_ignores_userinfo_path_query_fragment() {
        assert!(is_onion_url(&url::Url::parse("http://abc.onion/").unwrap()));
        assert!(is_onion_url(
            &url::Url::parse("http://ABC.ONION/path?q=1#frag").unwrap()
        ));
        assert!(is_onion_url(
            &url::Url::parse("http://user:pass@abc.onion/").unwrap()
        ));
        assert!(!is_onion_url(
            &url::Url::parse("http://abc.onion.example.com/").unwrap()
        ));
        assert!(!is_onion_url(
            &url::Url::parse("https://example.com/").unwrap()
        ));
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

    /// Section C (public Tor transport surface frontier): the accepted
    /// grammar is exactly `socks5h://HOST:PORT` — every named valid shape
    /// (IPv4, hostname, bracketed IPv6) accepted; every named invalid
    /// shape (missing port, path, query, fragment) rejected.
    #[test]
    fn tor_endpoint_grammar_accepts_only_bare_authority() {
        for valid in [
            "socks5h://127.0.0.1:9050",
            "socks5h://localhost:9050",
            "socks5h://[::1]:9050",
        ] {
            assert!(
                TorTransportConfig::new(valid).is_ok(),
                "{valid} must be accepted"
            );
        }

        assert!(matches!(
            TorTransportConfig::new("socks5h://127.0.0.1").unwrap_err(),
            TransportError::InvalidEndpoint(_)
        ));
        assert!(matches!(
            TorTransportConfig::new("socks5h://127.0.0.1:9050/").unwrap_err(),
            TransportError::InvalidEndpoint(_)
        ));
        assert!(matches!(
            TorTransportConfig::new("socks5h://127.0.0.1:9050/path").unwrap_err(),
            TransportError::InvalidEndpoint(_)
        ));
        assert!(matches!(
            TorTransportConfig::new("socks5h://127.0.0.1:9050?x=1").unwrap_err(),
            TransportError::InvalidEndpoint(_)
        ));
        assert!(matches!(
            TorTransportConfig::new("socks5h://127.0.0.1:9050#frag").unwrap_err(),
            TransportError::InvalidEndpoint(_)
        ));
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

    #[test]
    fn tor_policy_permits_both_onion_and_clearnet_targets() {
        let config = TorTransportConfig::new("socks5h://127.0.0.1:9050").unwrap();
        let policy = TransportPolicy::Tor(config);
        let clearnet = url::Url::parse("https://example.test/").unwrap();
        let onion = url::Url::parse("http://abc.onion/").unwrap();
        assert!(validate_target(&clearnet, &policy).is_ok());
        assert!(validate_target(&onion, &policy).is_ok());
    }

    /// Target compatibility is intentionally not a general URL-scheme policy.
    /// Preserve that established boundary when exposing the seam publicly.
    #[test]
    fn target_validation_does_not_invent_scheme_restrictions() {
        let policy = TransportPolicy::Default;
        for target in ["ftp://example.test/file", "file:///tmp/file", "data:,value"] {
            let parsed = url::Url::parse(target).unwrap();
            assert!(validate_target(&parsed, &policy).is_ok(), "{target}");
        }
    }

    /// Supersedes the old `TransportPolicy::label` test — provenance
    /// labels now live on `AcquisitionTransport` (the `Page`-carried,
    /// actually-observed transport), not the requested policy. See
    /// `spider::utils::evidence::build_evidence`.
    #[test]
    fn acquisition_transport_labels_are_the_locked_provenance_values() {
        assert_eq!(AcquisitionTransport::Default.label(), "default");
        assert_eq!(AcquisitionTransport::Tor.label(), "tor");
    }

    // The "fails closed without transport_tor" contract is proven at the
    // `fetch_single_page_with_options` level instead
    // (`spider::utils::evidence`'s own test module) — `apply_transport_policy`
    // itself no longer exists in that configuration (see its `cfg`), so
    // there is nothing to call here without the feature.
    #[cfg(feature = "tor")]
    #[test]
    fn tor_application_succeeds_with_transport_tor_feature() {
        let config = TorTransportConfig::new("socks5h://127.0.0.1:9050").unwrap();
        let result =
            apply_transport_policy(reqwest::ClientBuilder::new(), &TransportPolicy::Tor(config));
        assert!(result.is_ok());
    }

    /// Section B/D (public Tor transport surface frontier): the shared
    /// `TransportRequest -> TransportPolicy` validation matrix, exactly as
    /// documented on `TransportRequest::into_policy`.
    mod transport_request {
        use super::*;

        #[test]
        fn default_mode_no_proxy_is_default_policy() {
            let policy = TransportRequest {
                mode: TransportMode::Default,
                proxy: None,
            }
            .into_policy()
            .unwrap();
            assert!(matches!(policy, TransportPolicy::Default));
        }

        #[test]
        fn default_mode_with_proxy_is_rejected() {
            let result = TransportRequest {
                mode: TransportMode::Default,
                proxy: Some("socks5h://127.0.0.1:9050".to_string()),
            }
            .into_policy();
            assert!(matches!(
                result,
                Err(TransportError::IncompatibleConfiguration(_))
            ));
        }

        #[test]
        fn tor_mode_with_valid_proxy_is_tor_policy() {
            let policy = TransportRequest {
                mode: TransportMode::Tor,
                proxy: Some("socks5h://127.0.0.1:9050".to_string()),
            }
            .into_policy()
            .unwrap();
            assert!(matches!(policy, TransportPolicy::Tor(_)));
        }

        #[test]
        fn tor_mode_without_proxy_is_rejected() {
            let result = TransportRequest {
                mode: TransportMode::Tor,
                proxy: None,
            }
            .into_policy();
            assert!(matches!(
                result,
                Err(TransportError::IncompatibleConfiguration(_))
            ));
        }

        /// An invalid endpoint under `mode = tor` surfaces
        /// `TorTransportConfig::new`'s own error verbatim — the request
        /// layer doesn't wrap/obscure it.
        #[test]
        fn tor_mode_with_malformed_proxy_surfaces_endpoint_error() {
            let result = TransportRequest {
                mode: TransportMode::Tor,
                proxy: Some("http://127.0.0.1:9050".to_string()),
            }
            .into_policy();
            assert_eq!(
                result.unwrap_err(),
                TransportError::UnsupportedScheme("http".into())
            );
        }

        #[test]
        fn default_is_default_mode_no_proxy() {
            let request = TransportRequest::default();
            assert_eq!(request.mode, TransportMode::Default);
            assert_eq!(request.proxy, None);
            assert!(matches!(
                request.into_policy().unwrap(),
                TransportPolicy::Default
            ));
        }
    }

    /// `execute_streaming_request` / `build_streaming_client` — real,
    /// local, deterministic network fixtures. Matches the established
    /// blocking-free `tokio::net::TcpListener` fixture convention already
    /// used by `spider/tests/transport_tor.rs` and
    /// `acquisition_binding.rs`'s own test module. No public
    /// network/Tor dependency, no internet-dependent test.
    mod streaming_request {
        use super::*;
        use crate::secret_request_headers::SecretRequestHeaders;
        use std::net::SocketAddr;
        use std::sync::{Arc, Mutex};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        use tokio_stream::StreamExt;

        struct HttpFixture {
            addr: SocketAddr,
            last_request: Arc<Mutex<Vec<u8>>>,
        }

        impl HttpFixture {
            async fn start(
                status: &'static str,
                extra_headers: &'static str,
                body: &'static [u8],
            ) -> Self {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap();
                let last_request = Arc::new(Mutex::new(Vec::new()));
                let last_request_clone = last_request.clone();
                tokio::spawn(async move {
                    loop {
                        let (mut stream, _) = match listener.accept().await {
                            Ok(pair) => pair,
                            Err(_) => break,
                        };
                        let last_request = last_request_clone.clone();
                        tokio::spawn(async move {
                            let mut buf = [0_u8; 8192];
                            if let Ok(n) = stream.read(&mut buf).await {
                                *last_request.lock().unwrap() = buf[..n].to_vec();
                            }
                            let response = format!(
                                "HTTP/1.1 {status}\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let _ = stream.write_all(response.as_bytes()).await;
                            let _ = stream.write_all(body).await;
                        });
                    }
                });
                Self { addr, last_request }
            }

            fn url(&self) -> url::Url {
                url::Url::parse(&format!("http://{}/", self.addr)).unwrap()
            }

            fn last_request_text(&self) -> String {
                String::from_utf8_lossy(&self.last_request.lock().unwrap()).to_string()
            }
        }

        /// Section G/N: status, final URL, and headers are all readable
        /// immediately — before the caller ever reads a body byte — and
        /// the body is still fully available afterward, on the caller's
        /// own schedule.
        #[tokio::test]
        async fn default_policy_exposes_status_final_url_and_headers_before_body_is_read() {
            let fixture = HttpFixture::start(
                "200 OK",
                "X-Fixture: streaming-frontier\r\n",
                b"hello streaming world",
            )
            .await;
            let url = fixture.url();
            let headers = SecretRequestHeaders::new();

            let response = execute_streaming_request(
                &url,
                &TransportPolicy::Default,
                &headers,
                "spider_transport-test",
            )
            .await
            .unwrap();

            assert_eq!(response.status(), reqwest::StatusCode::OK);
            assert_eq!(response.url().host_str(), url.host_str());
            assert_eq!(
                response.headers().get("x-fixture").unwrap(),
                "streaming-frontier"
            );

            // The body is consumed only now, explicitly, by the test --
            // never inside `execute_streaming_request` itself.
            let bytes = response.bytes().await.unwrap();
            assert_eq!(&bytes[..], b"hello streaming world");
        }

        /// Section M: `SecretRequestHeaders` are actually applied to the
        /// outgoing request, not merely accepted and dropped.
        #[tokio::test]
        async fn secret_headers_are_applied_to_the_outgoing_request() {
            let fixture = HttpFixture::start("200 OK", "", b"ok").await;
            let url = fixture.url();
            let mut headers = SecretRequestHeaders::new();
            headers
                .try_insert("x-secret-sentinel", "streaming-secret-value")
                .unwrap();

            let response = execute_streaming_request(
                &url,
                &TransportPolicy::Default,
                &headers,
                "spider_transport-test",
            )
            .await
            .unwrap();
            assert!(response.status().is_success());

            let request_text = fixture.last_request_text().to_ascii_lowercase();
            assert!(request_text.contains("x-secret-sentinel: streaming-secret-value"));
        }

        /// Section E: fail-closed `.onion`/`Default` rejection happens
        /// before any client is built or any network activity occurs --
        /// reusing `validate_target` verbatim, not a second check.
        #[tokio::test]
        async fn onion_target_under_default_policy_is_rejected_before_any_network_activity() {
            let onion_url = url::Url::parse("http://exampleexampleexampleexamp.onion/").unwrap();
            let headers = SecretRequestHeaders::new();

            let error = execute_streaming_request(
                &onion_url,
                &TransportPolicy::Default,
                &headers,
                "spider_transport-test",
            )
            .await
            .unwrap_err();
            assert_eq!(error, TransportError::OnionRequiresTor);
        }

        /// Section E/N: a redirect that would silently change transport
        /// (clearnet -> onion under `Default`) is rejected mid-request by
        /// the same `pin_redirect_policy` the audited Tor client already
        /// uses, and surfaces truthfully as `RequestExecutionFailed` --
        /// not as a followed redirect, not as a panic, not silently
        /// ignored.
        #[tokio::test]
        async fn default_policy_rejects_a_redirect_to_an_onion_host() {
            let fixture = HttpFixture::start(
                "302 Found",
                "Location: http://exampleexampleexampleexamp.onion/\r\n",
                b"",
            )
            .await;
            let url = fixture.url();
            let headers = SecretRequestHeaders::new();

            let error = execute_streaming_request(
                &url,
                &TransportPolicy::Default,
                &headers,
                "spider_transport-test",
            )
            .await
            .unwrap_err();
            assert!(matches!(error, TransportError::RequestExecutionFailed(_)));
        }

        /// Section O: the response body is genuinely streamed chunk by
        /// chunk via the caller's own `.bytes_stream()` consumption, not
        /// pre-collected into a `Vec<u8>` by this seam.
        #[tokio::test]
        async fn body_is_available_only_via_the_caller_consuming_the_stream() {
            let payload = b"streaming payload assembled chunk by chunk for the frontier proof";
            let fixture = HttpFixture::start("200 OK", "", payload).await;
            let url = fixture.url();
            let headers = SecretRequestHeaders::new();

            let response = execute_streaming_request(
                &url,
                &TransportPolicy::Default,
                &headers,
                "spider_transport-test",
            )
            .await
            .unwrap();

            let mut stream = response.bytes_stream();
            let mut collected = Vec::new();
            while let Some(chunk) = stream.next().await {
                collected.extend_from_slice(&chunk.unwrap());
            }
            assert_eq!(collected, payload);
        }

        /// Section N: a body failure that only manifests *after* status
        /// and headers were already returned `Ok` (a truncated response)
        /// surfaces truthfully as a stream error to whatever code
        /// actually consumes the stream -- `execute_streaming_request`
        /// does not, and structurally cannot, mask it, since the body was
        /// never touched before returning.
        #[tokio::test]
        async fn mid_stream_body_truncation_surfaces_as_a_stream_error_after_status_was_ok() {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                if let Ok((mut stream, _)) = listener.accept().await {
                    let mut buf = [0_u8; 4096];
                    let _ = stream.read(&mut buf).await;
                    // Advertise more bytes than are actually sent, then
                    // close -- proves mid-stream failures surface
                    // truthfully even though `execute_streaming_request`
                    // already returned `Ok`.
                    let _ = stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\nshort",
                        )
                        .await;
                }
            });

            let url = url::Url::parse(&format!("http://{addr}/")).unwrap();
            let headers = SecretRequestHeaders::new();
            let response = execute_streaming_request(
                &url,
                &TransportPolicy::Default,
                &headers,
                "spider_transport-test",
            )
            .await
            .unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::OK);

            let mut stream = response.bytes_stream();
            let mut saw_error = false;
            while let Some(chunk) = stream.next().await {
                if chunk.is_err() {
                    saw_error = true;
                    break;
                }
            }
            assert!(saw_error, "a truncated body must surface as a stream error");
        }

        /// Section L (Default/Tor parity, `transport_tor` compiled in):
        /// `build_streaming_client` builds successfully for both
        /// policies, `Tor` delegating entirely to the existing
        /// `build_tor_client` -- there is exactly one Tor client
        /// implementation in this crate, not two.
        #[cfg(feature = "tor")]
        #[test]
        fn build_streaming_client_succeeds_for_both_policies_when_transport_tor_is_compiled() {
            assert!(
                build_streaming_client(&TransportPolicy::Default, "spider_transport-test").is_ok()
            );
            let config = TorTransportConfig::new("socks5h://127.0.0.1:9050").unwrap();
            assert!(
                build_streaming_client(&TransportPolicy::Tor(config), "spider_transport-test")
                    .is_ok()
            );
        }

        /// Section L/U (Default/Tor parity, `transport_tor` NOT
        /// compiled): `Default` streaming execution keeps working --
        /// this seam never needed Tor for that -- while `Tor` fails
        /// closed with `TorNotCompiled`, never silently falling back to
        /// Default.
        #[cfg(not(feature = "tor"))]
        #[test]
        fn build_streaming_client_supports_default_and_fails_closed_for_tor_without_the_feature() {
            assert!(
                build_streaming_client(&TransportPolicy::Default, "spider_transport-test").is_ok()
            );
            let config = TorTransportConfig::new("socks5h://127.0.0.1:9050").unwrap();
            assert_eq!(
                build_streaming_client(&TransportPolicy::Tor(config), "spider_transport-test")
                    .unwrap_err(),
                TransportError::TorNotCompiled
            );
        }

        /// Section U: `Tor` under `execute_streaming_request` itself
        /// (not just `build_streaming_client`) fails closed the same way,
        /// before any network activity, when `transport_tor` is not
        /// compiled in.
        #[cfg(not(feature = "tor"))]
        #[tokio::test]
        async fn tor_policy_without_transport_tor_feature_fails_closed_before_any_network_activity()
        {
            let config = TorTransportConfig::new("socks5h://127.0.0.1:9050").unwrap();
            let policy = TransportPolicy::Tor(config);
            let url = url::Url::parse("https://example.test/").unwrap();
            let headers = SecretRequestHeaders::new();

            let error = execute_streaming_request(&url, &policy, &headers, "spider_transport-test")
                .await
                .unwrap_err();
            assert_eq!(error, TransportError::TorNotCompiled);
        }

        /// Section H: request-execution failures against a target that
        /// refuses the TCP connection outright surface as
        /// `RequestExecutionFailed`, not a panic and not a silently
        /// empty/default response.
        #[tokio::test]
        async fn connection_refused_surfaces_as_request_execution_failed() {
            // Bind, read the local address, then drop the listener so the
            // port is refusing connections -- deterministic, no sleep.
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            drop(listener);

            let url = url::Url::parse(&format!("http://{addr}/")).unwrap();
            let headers = SecretRequestHeaders::new();
            let error = execute_streaming_request(
                &url,
                &TransportPolicy::Default,
                &headers,
                "spider_transport-test",
            )
            .await
            .unwrap_err();
            assert!(matches!(error, TransportError::RequestExecutionFailed(_)));
        }
    }
}
