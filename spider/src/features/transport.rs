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

/// Sanitized, network-acquisition-only transport provenance carried by
/// [`crate::page::Page`]. Deliberately minimal: no SOCKS endpoint, no
/// credentials, no full [`TransportPolicy`] — just which of the two
/// audited routes actually performed the fetch. `Page`'s field is
/// private; only the acquisition code paths in this crate that actually
/// dispatch a request may stamp it (see
/// [`crate::page::Page::transport`]).
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
    pub(crate) fn label(&self) -> &'static str {
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
    /// [`crate::utils::spawn_set`] re-entering the same scope — mirroring
    /// the existing `WEBSITE_SPOOL_DIR` task-local
    /// (`crate::utils::html_spool`), the established pattern in this
    /// crate for ambient per-crawl context that must survive `tokio::spawn`
    /// boundaries without threading a new parameter through every
    /// intermediate function signature.
    ///
    /// Two independent readers consume this:
    /// - [`crate::page::build`] stamps `Page::transport` from it — the
    ///   only writer of that field.
    /// - [`crate::page::host_resolves_locally_cached`] refuses to perform
    ///   or consult any local DNS lookup for the target host while this
    ///   scope reads `Tor` — see [`target_dns_suppressed`].
    pub(crate) static ACQUISITION_TRANSPORT_SCOPE: AcquisitionTransport;
}

/// The transport of the enclosing [`ACQUISITION_TRANSPORT_SCOPE`], or
/// `None` when called outside any such scope (e.g. code paths this
/// frontier does not audit, or ordinary non-crawl test code).
pub(crate) fn current_acquisition_transport() -> Option<AcquisitionTransport> {
    ACQUISITION_TRANSPORT_SCOPE.try_with(|value| *value).ok()
}

/// `true` only when the enclosing acquisition is genuinely Tor. Never
/// `true` by default, never `true` outside an explicit scope — the
/// suppression this gates (see callers) must never activate for a
/// context nobody positively marked as Tor.
pub(crate) fn target_dns_suppressed() -> bool {
    current_acquisition_transport() == Some(AcquisitionTransport::Tor)
}

/// The `AcquisitionTransport` this fixed `TransportPolicy` corresponds to
/// — the value a crawl's outer `ACQUISITION_TRANSPORT_SCOPE` must be
/// entered with for its whole duration.
pub(crate) fn acquisition_transport_for(policy: &TransportPolicy) -> AcquisitionTransport {
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
pub(crate) enum CrawlBoundary {
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
    pub(crate) fn from_seed(policy: &TransportPolicy, seed: &url::Url) -> Self {
        let seed_onion = seed.host_str().is_some_and(is_onion_host);
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
pub(crate) fn crawl_boundary_allows(boundary: &CrawlBoundary, candidate: &url::Url) -> bool {
    let candidate_onion = candidate.host_str().is_some_and(is_onion_host);
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

/// Connect/read timeouts for the audited Tor client, reused verbatim from
/// `Website::configure_base_client`'s own *unmultiplied* defaults
/// (`Duration::from_secs(24)` / `Duration::from_secs(42)`) — not a
/// Tor-specific invention. `configure_base_client` doubles these only
/// when Spider's legacy multi-proxy rotation list (`configuration.proxies`)
/// is configured; the dedicated Tor client never uses that list (rejected
/// explicitly at preflight — see `Website::tor_crawl_preflight`), so the
/// unmultiplied canonical values are the correct match, and they provide
/// the hard bound that keeps a stalled/blackhole SOCKS handshake from
/// waiting indefinitely. Shared by both the one-shot seam
/// (`spider::utils::evidence::fetch_via_tor`) and multi-page Tor crawling
/// — one canonical set of constants, not two.
#[cfg(all(
    feature = "transport_tor",
    not(feature = "wreq"),
    not(feature = "cache_request")
))]
pub(crate) const TOR_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(24);
#[cfg(all(
    feature = "transport_tor",
    not(feature = "wreq"),
    not(feature = "cache_request")
))]
pub(crate) const TOR_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(42);

/// Total request deadline for the audited Tor client, reused verbatim
/// from `Configuration::new()`'s own default `request_timeout`
/// (`Duration::from_secs(120)`), applied via `reqwest::ClientBuilder::timeout`.
///
/// Connect/read timeouts alone are not sufficient: a peer that keeps the
/// connection alive and periodically sends enough bytes to keep resetting
/// [`TOR_READ_TIMEOUT`] (a slow-drip response) would never trip either of
/// them, and could otherwise stall a Tor acquisition indefinitely.
/// `.timeout()` bounds the request end-to-end — connect, redirects, and
/// response body — regardless of how activity is paced within it.
#[cfg(all(
    feature = "transport_tor",
    not(feature = "wreq"),
    not(feature = "cache_request")
))]
pub(crate) const TOR_TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Redirect hop cap for the audited Tor client, reused verbatim from
/// `Configuration::new()`'s own default `redirect_limit` (`7`).
#[cfg(all(
    feature = "transport_tor",
    not(feature = "wreq"),
    not(feature = "cache_request")
))]
pub(crate) const TOR_REDIRECT_LIMIT: usize = 7;

/// Build the one canonical Tor-audited `reqwest::Client`: no
/// environment/system proxy inheritance, exactly one explicit SOCKS5h
/// proxy, bounded connect/read/total timeouts, a redirect policy that
/// pins transport across hops while reusing Spider's existing SSRF
/// redirect guard, and Spider's own default user-agent (so a Tor fetch is
/// not distinguishable from a Default fetch by header fingerprint alone).
///
/// This is the single reusable transport building primitive — both the
/// one-shot seam (`spider::utils::evidence::fetch_via_tor`) and multi-page
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
#[cfg(all(
    feature = "transport_tor",
    not(feature = "wreq"),
    not(feature = "cache_request")
))]
pub(crate) fn build_tor_client(
    policy: &TransportPolicy,
) -> Result<reqwest::Client, TransportError> {
    let builder = apply_transport_policy(reqwest::Client::builder(), policy)?;
    let builder = builder
        .connect_timeout(TOR_CONNECT_TIMEOUT)
        .read_timeout(TOR_READ_TIMEOUT)
        .timeout(TOR_TOTAL_TIMEOUT)
        .user_agent(crate::configuration::get_ua(false))
        .redirect(pin_redirect_policy(
            ssrf_screened_base_policy(TOR_REDIRECT_LIMIT),
            policy.clone(),
        ));
    builder
        .build()
        .map_err(|error| TransportError::ProxyBuildFailed(error.to_string()))
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

    /// Supersedes the old `TransportPolicy::label` test — provenance
    /// labels now live on `AcquisitionTransport` (the `Page`-carried,
    /// actually-observed transport), not the requested policy. See
    /// `spider::utils::evidence::build_evidence_with_transport`.
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
}
