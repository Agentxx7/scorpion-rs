//! One-shot resource acquisition and evidence/provenance construction.
//!
//! This is the canonical seam both `spider_mcp` and `spider_cli` call into
//! for "fetch exactly one resource and produce truthful retrieval evidence
//! for it" — the shared acquisition/evidence layer beneath Scorpion's
//! feed/sitemap/news-sitemap/robots-sitemap discovery adapters and any
//! evidence-first single-resource fetch. Relocated from `spider_mcp` (where
//! it originated) so a second, independently-drifting implementation is
//! never written for the CLI. This is plumbing, not a new capability: every
//! field `EvidenceBundle` can populate is sourced from data `Page` already
//! captures — nothing here changes crawling, fetching, or rendering
//! behavior.

use crate::features::transport::{self, TransportPolicy};
use crate::page::Page;
use crate::website::Website;

#[cfg(feature = "serde")]
use serde::Serialize;
use sha2::{Digest, Sha256};

/// SHA-256 of exactly the supplied bytes, encoded as lowercase hexadecimal.
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Extract a page's captured screenshot bytes, if any. `Page::screenshot_bytes`
/// only exists at all behind the `chrome` feature, so this is `None`
/// unconditionally without it.
#[cfg(feature = "chrome")]
pub fn page_screenshot_bytes(page: &Page) -> Option<&[u8]> {
    page.screenshot_bytes.as_deref()
}

/// No screenshot bytes are ever available without the `chrome` feature.
#[cfg(not(feature = "chrome"))]
pub fn page_screenshot_bytes(_page: &Page) -> Option<&[u8]> {
    None
}

/// Truthful retrieval evidence for one fetched resource. Every field is
/// `Option`, populated only when the underlying data was actually observed
/// during a fetch — never fabricated or guessed.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct EvidenceBundle {
    /// The URL that was actually requested.
    pub requested_url: Option<String>,
    /// The URL after following any redirects. Equal to `requested_url` when
    /// no redirect occurred — this field is populated whenever a page was
    /// fetched, never omitted just because the two happen to match, so its
    /// presence always reflects ground truth rather than leaving
    /// redirect-vs-no-redirect ambiguous.
    pub final_url: Option<String>,
    /// Unix epoch milliseconds when the live HTTP- or Chrome-produced
    /// representation backing this page finished materializing. `None` when
    /// that canonical completion time was not captured (including cache and
    /// error paths). This is not a server timestamp or request-start time.
    pub retrieved_at: Option<u64>,
    /// Spider's effective/crawler status after existing operational
    /// reclassification and retry policy.
    pub status_code: Option<u16>,
    /// HTTP status actually observed from a response or trusted relay. This
    /// remains independent of Spider's effective/crawler `status_code`.
    pub observed_status_code: Option<u16>,
    /// The response's `Content-Type` header, verbatim, when present.
    pub content_type: Option<String>,
    /// MIME type detected directly from the retained non-browser HTTP
    /// response bytes. Independent of the declared `content_type`; `None`
    /// when bytes are absent, unrecognized, or produced by a browser path.
    pub detected_content_type: Option<String>,
    /// SHA-256 of the exact HTTP content-decoded response-body bytes retained
    /// by `Page` on the non-browser HTTP scrape path. This is not a hash of
    /// transport/wire bytes and no character normalization is applied. Always
    /// `None` for browser/headless fetches because their `Page` bytes represent
    /// Chromium's rendered DOM rather than an HTTP response body.
    pub response_body_hash: Option<String>,
    /// SHA-256 of `content.as_bytes()` exactly as returned in this bundle.
    pub transformed_content_hash: Option<String>,
    /// Textual content in the requested format (markdown/text/raw/xml).
    /// `None` when the request was for a screenshot instead — see
    /// `screenshot`.
    pub content: Option<String>,
    /// Links discovered on the page, when link collection was enabled.
    pub links: Option<Vec<String>>,
    /// Which engine/site surfaced this evidence — populated only for
    /// search-derived evidence (e.g. "youtube"). `None` for a direct fetch:
    /// a URL fetch has no "source" distinct from the URL itself.
    pub source: Option<String>,
    /// Which search provider produced this evidence — populated only for
    /// search-derived evidence (e.g. "searxng"). `None` for a direct fetch.
    pub provider: Option<String>,
    /// The search query that led to this evidence — populated only for
    /// search-derived evidence. `None` for a direct fetch.
    pub query: Option<String>,
    /// Base64-encoded screenshot, when a screenshot was requested and
    /// captured. Kept distinct from `content` — image bytes are not
    /// textual content.
    pub screenshot: Option<String>,
    /// SHA-256 of the original captured PNG bytes, never its base64 encoding.
    pub screenshot_hash: Option<String>,
    /// Reserved for future structured metadata. Always `None` today —
    /// nothing currently populates it honestly. `serde_json::Value` is
    /// available unconditionally here because the `evidence` feature
    /// requires `serde` (which pulls in `serde_json`).
    pub metadata: Option<serde_json::Value>,
    /// Which transport actually performed this acquisition — `"default"`
    /// or `"tor"`. Populated by [`build_evidence`] from the `Page`-carried
    /// provenance stamp; `None` for a page no audited acquisition path
    /// stamped, which never claims a transport it didn't observe. Never
    /// claims the configured SOCKS endpoint is genuinely Tor — only that
    /// the Tor transport policy was the one selected for this fetch.
    pub transport: Option<String>,
    /// How target-hostname DNS resolution was performed: `"proxy"` when
    /// resolution happened proxy-side (Tor/SOCKS5h — the local process
    /// never resolved the target host), `None` when unspecified/ordinary
    /// local resolution applies (`default` transport, or a page carrying
    /// no provenance stamp).
    pub dns: Option<String>,
}

/// Build retrieval evidence for one fetched page. Content and screenshot
/// remain mutually exclusive; byte-derived fields are never claimed for a
/// browser-produced representation.
///
/// `transport`/`dns` are read directly from `page.transport()` — the
/// private, `pub(crate)`-only-writable stamp only actual network-acquisition
/// code sets (see `crate::features::transport::AcquisitionTransport` and
/// `Page::transport`). This is the single canonical provenance path
/// (Section H/I of the blocker-fix frontier): a page truthfully acquired
/// over `Tor` reports `transport = "tor"`, `dns = "proxy"`; over `Default`,
/// `transport = "default"`, `dns = null`; a page this frontier's
/// acquisition scope never stamped (cache hit, non-network, or an
/// unaudited path) reports `transport = null`, `dns = null` — never
/// fabricated, and never reconstructed from a caller-supplied
/// `TransportPolicy`/`Website.configuration`.
pub fn build_evidence(
    page: &Page,
    content: Option<String>,
    wants_screenshot: bool,
    used_browser: bool,
) -> EvidenceBundle {
    let response_body_hash = (!used_browser)
        .then(|| page.get_bytes().map(sha256_hex))
        .flatten();
    let detected_content_type = if used_browser {
        None
    } else {
        page.get_bytes()
            .and_then(infer::get)
            .map(|kind| kind.mime_type().to_string())
    };
    let screenshot_bytes = page_screenshot_bytes(page);
    let transformed_content_hash = if wants_screenshot {
        None
    } else {
        content.as_deref().map(|text| sha256_hex(text.as_bytes()))
    };
    let screenshot_hash = wants_screenshot
        .then_some(screenshot_bytes)
        .flatten()
        .map(sha256_hex);
    let content_type = page
        .headers
        .as_ref()
        .and_then(|headers| headers.get("content-type"))
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let links = page.page_links.as_ref().map(|links| {
        links
            .iter()
            .map(|link| link.inner().to_string())
            .collect::<Vec<_>>()
    });
    let observed_transport = page.transport();

    EvidenceBundle {
        requested_url: Some(page.get_url().to_string()),
        final_url: Some(page.get_url_final().to_string()),
        retrieved_at: page.get_retrieved_at(),
        status_code: Some(page.status_code.as_u16()),
        observed_status_code: page.observed_status_code.map(|status| status.as_u16()),
        content_type,
        detected_content_type,
        response_body_hash,
        transformed_content_hash,
        content: (!wants_screenshot).then_some(content.clone()).flatten(),
        links,
        source: None,
        provider: None,
        query: None,
        screenshot: wants_screenshot.then_some(content).flatten(),
        screenshot_hash,
        metadata: None,
        transport: observed_transport.map(|transport| transport.label().to_string()),
        dns: match observed_transport {
            Some(transport::AcquisitionTransport::Tor) => Some("proxy".to_string()),
            _ => None,
        },
    }
}

/// Fetch exactly one page through Spider's ordinary non-browser HTTP path:
/// one target URL, no Chrome/browser fallback, no discovered-link
/// traversal. The canonical one-shot acquisition primitive shared by every
/// discovery adapter (feed/sitemap/news-sitemap/robots-sitemap) and any
/// evidence-first single-resource fetch, in both the MCP server and the
/// CLI.
pub async fn fetch_single_page(url: &str) -> Result<Page, String> {
    let mut website = Website::new(url);
    website.with_limit(1);
    let mut website = website.build().map_err(|_| "Invalid URL".to_string())?;
    let mut receiver = website.subscribe(1);
    tokio::spawn(async move {
        website.crawl_raw().await;
        website.unsubscribe();
    });
    receiver
        .recv()
        .await
        .map_err(|_| "Retrieval completed without producing a page".to_string())
}

/// Options for the transport-aware one-shot acquisition seam. `transport`
/// selects `Default` (existing behavior) or `Tor` (fail-closed SOCKS5h).
/// `Default::default()` is `TransportPolicy::Default`, matching
/// [`fetch_single_page`]'s existing behavior exactly.
#[derive(Debug, Clone, Default)]
pub struct AcquisitionOptions {
    /// The transport policy this acquisition must use.
    pub transport: TransportPolicy,
}

/// A fetched `Page` bound to the transport policy that *actually*
/// performed the fetch. The only way to obtain one is
/// [`fetch_single_page_with_options`]'s return value — there is no public
/// constructor — so a caller can never mint a `TransportAcquisition`
/// claiming `Tor` for a `Page` that was really fetched over `Default`
/// transport (or vice versa). This is what makes evidence provenance
/// trustworthy: [`build_evidence`] reads `.transport()` off the page
/// carried by this type instead of accepting an unrelated,
/// independently-suppliable policy argument from the caller.
#[derive(Debug)]
pub struct TransportAcquisition {
    page: Page,
    transport: TransportPolicy,
}

impl TransportAcquisition {
    /// The fetched page.
    pub fn page(&self) -> &Page {
        &self.page
    }

    /// The transport policy that actually performed this acquisition.
    pub fn transport(&self) -> &TransportPolicy {
        &self.transport
    }

    /// Consume this acquisition, discarding the transport binding and
    /// returning the bare `Page` — for callers that only need the page
    /// and don't need (or have already extracted) transport provenance.
    pub fn into_page(self) -> Page {
        self.page
    }
}

/// Fetch exactly one page honoring the given transport policy — the
/// options-aware, backward-compatible superset of [`fetch_single_page`].
///
/// `AcquisitionOptions { transport: TransportPolicy::Default }` delegates
/// to the exact same `Website`-based path `fetch_single_page` already
/// uses (byte-for-byte the same call sequence) — zero behavior change for
/// every existing caller.
///
/// `TransportPolicy::Tor` never touches `Website`/`configure_http_client`:
/// Tor traffic is issued through a small, dedicated `reqwest::Client`
/// built solely from the validated SOCKS5h endpoint (see
/// [`crate::features::transport`]), so it can never inherit unaudited
/// proxy-rotation, Spider Cloud, `wreq`, or Chrome/smart-mode behavior.
/// `.onion` targets are rejected before any DNS lookup or network
/// activity when the active policy is `Default`.
///
/// # Failure contract
///
/// `Err` is returned only for failures that occur *before* a request is
/// ever attempted: an unparseable `url`, a `.onion` target under
/// `Default` transport, or a transport *configuration* problem (an
/// invalid/unsupported Tor endpoint, `transport_tor` not compiled in, or
/// an incompatible build combination). None of these ever reach the
/// network.
///
/// Once a request is actually attempted, this matches [`fetch_single_page`]'s
/// established contract exactly: a network-level failure (connection
/// refused, SOCKS failure, TLS failure, timeout, DNS failure, non-2xx
/// response, …) is `Ok(TransportAcquisition)` whose `.page()` carries a
/// non-success status — never a fabricated success, but also never a
/// hard `Err` for something that reached the wire. This is intentional,
/// not an oversight: `Page` (not `Result`) has always been Spider's
/// vocabulary for "a request was attempted and this is what came back",
/// and Tor acquisition reuses that vocabulary rather than inventing a
/// second one. Callers that need to distinguish "the Tor circuit itself
/// failed" from "the origin returned an error" should inspect
/// `.page().status_code`.
///
/// There is no fallback in either case: a Tor failure — at any layer —
/// never causes a retry over `Default` transport.
pub async fn fetch_single_page_with_options(
    url: &str,
    options: AcquisitionOptions,
) -> Result<TransportAcquisition, String> {
    let parsed = url::Url::parse(url).map_err(|_| "Invalid URL".to_string())?;
    transport::validate_target(&parsed, &options.transport).map_err(|error| error.to_string())?;

    match &options.transport {
        TransportPolicy::Default => fetch_single_page(url)
            .await
            .map(|page| TransportAcquisition {
                page,
                transport: options.transport,
            }),
        TransportPolicy::Tor(_) => {
            let page = fetch_via_tor(url, &options.transport).await?;
            Ok(TransportAcquisition {
                page,
                transport: options.transport,
            })
        }
    }
}

/// Fetch one page through the one canonical audited Tor client (see
/// [`transport::build_tor_client`] — the same primitive multi-page Tor
/// crawling uses; there is exactly one Tor client implementation in this
/// crate). Runs the fetch inside an [`transport::ACQUISITION_TRANSPORT_SCOPE`]
/// of `Tor`, so [`crate::page::build`] stamps `Page::transport` truthfully
/// and [`crate::page::host_resolves_locally_cached`] refuses any local
/// lookup for the target host.
#[cfg(all(
    feature = "transport_tor",
    not(feature = "wreq"),
    not(feature = "cache_request")
))]
async fn fetch_via_tor(url: &str, policy: &TransportPolicy) -> Result<Page, String> {
    let executor = spider_transport::ResolvedExecutor::resolve(
        spider_transport::CrawlerTransportConfiguration {
            policy: policy.clone(),
            user_agent: crate::configuration::get_ua(false).to_string(),
            ..Default::default()
        },
    )
    .map_err(|error| error.to_string())?;
    Ok(transport::ACQUISITION_TRANSPORT_SCOPE
        .scope(
            transport::AcquisitionTransport::Tor,
            Page::new_page_with_executor(url, &executor),
        )
        .await)
}

/// `transport_tor` is compiled, but so is `wreq` and/or `cache_request` —
/// neither alternate client stack is Tor-audited (see [`fetch_via_tor`]
/// above), so the combination is rejected explicitly rather than silently
/// using an unaudited client.
#[cfg(all(
    feature = "transport_tor",
    any(feature = "wreq", feature = "cache_request")
))]
async fn fetch_via_tor(_url: &str, _policy: &TransportPolicy) -> Result<Page, String> {
    let message = "Tor transport requires a build without the wreq or cache_request features — \
         neither alternate client stack is audited for Tor-safe (fail-closed) behavior"
        .to_string();
    Err(crate::features::transport::TransportError::IncompatibleConfiguration(message).to_string())
}

#[cfg(not(feature = "transport_tor"))]
async fn fetch_via_tor(_url: &str, _policy: &TransportPolicy) -> Result<Page, String> {
    Err(crate::features::transport::TransportError::TorNotCompiled.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only actually-known fields are populated; the rest remain absent
    /// (default `None`), never fabricated.
    #[test]
    fn only_known_fields_are_populated_rest_stay_none() {
        let bundle = EvidenceBundle {
            requested_url: Some("https://example.test/".to_string()),
            status_code: Some(200),
            ..Default::default()
        };
        assert_eq!(
            bundle.requested_url.as_deref(),
            Some("https://example.test/")
        );
        assert_eq!(bundle.status_code, Some(200));
        assert_eq!(bundle.final_url, None);
        assert_eq!(bundle.retrieved_at, None);
        assert_eq!(bundle.observed_status_code, None);
        assert_eq!(bundle.content_type, None);
        assert_eq!(bundle.detected_content_type, None);
        assert_eq!(bundle.response_body_hash, None);
        assert_eq!(bundle.transformed_content_hash, None);
        assert_eq!(bundle.content, None);
        assert_eq!(bundle.links, None);
        assert_eq!(bundle.source, None);
        assert_eq!(bundle.provider, None);
        assert_eq!(bundle.query, None);
        assert_eq!(bundle.screenshot, None);
        assert_eq!(bundle.screenshot_hash, None);
        assert_eq!(bundle.metadata, None);
    }

    /// requested_url and final_url are independent fields — a redirect
    /// changing one must never overwrite or conflate the other.
    #[test]
    fn requested_and_final_url_remain_independent() {
        let bundle = EvidenceBundle {
            requested_url: Some("https://example.test/".to_string()),
            final_url: Some("https://example.test/final".to_string()),
            ..Default::default()
        };
        assert_ne!(bundle.requested_url, bundle.final_url);
    }

    #[test]
    fn sha256_is_deterministic_and_byte_sensitive() {
        let bytes = b"scorpion evidence";
        assert_eq!(sha256_hex(bytes), sha256_hex(bytes));
        assert_ne!(sha256_hex(bytes), sha256_hex(b"scorpion evidencf"));
    }

    #[test]
    fn sha256_matches_known_vector_and_is_lowercase_hex() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[tokio::test]
    async fn fetch_single_page_rejects_invalid_url() {
        let result = fetch_single_page("not a url").await;
        assert!(result.is_err());
    }

    /// Without the `transport_tor` feature, requesting `TransportPolicy::Tor`
    /// through the public one-shot seam fails closed with an explicit `Err`
    /// before any network activity — never silently falls back to `Default`.
    /// This is the public-contract-level proof that replaces the old
    /// `transport::apply_transport_policy`-level unit test, which no longer
    /// exists in this configuration (see that function's `cfg`).
    #[cfg(not(feature = "transport_tor"))]
    #[tokio::test]
    async fn tor_acquisition_fails_closed_without_transport_tor_feature() {
        let config =
            crate::features::transport::TorTransportConfig::new("socks5h://127.0.0.1:9050")
                .unwrap();
        let result = fetch_single_page_with_options(
            "http://example.test/",
            AcquisitionOptions {
                transport: TransportPolicy::Tor(config),
            },
        )
        .await;
        assert!(result.is_err());
    }
}
