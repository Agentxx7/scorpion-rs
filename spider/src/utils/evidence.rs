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

use crate::features::identity::EvidenceId;
use crate::features::transport::{self, TransportPolicy};
use crate::page::Page;
#[cfg(not(feature = "wreq"))]
use crate::website::Website;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
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
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct EvidenceBundle {
    /// This bundle's durable identity in the evidence ledger. `None` for a
    /// bundle fresh from [`build_evidence`] that has not (yet) been
    /// recorded — see [`record_evidence`] (behind the `disk` feature).
    /// Identity is assigned, never derived from fetch content; it is not
    /// itself provenance. Present unconditionally (identity carries no
    /// feature gate of its own — see `features/identity.rs`) so
    /// `EvidenceBundle`'s shape does not change across builds depending
    /// on whether the `disk` feature happens to be enabled.
    pub id: Option<EvidenceId>,
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
    /// Which backend observed or reconstructed this response —
    /// `"reqwest"`, `"wreq"`, `"cache_layer"`,
    /// `"noncanonical_fetch_engine"`, `"noncanonical_remote_fetcher"`, or
    /// `"upstream_compatibility"`. Read directly from
    /// `Page::backend_provenance()` (`spider_transport::BackendProvenance`,
    /// the same canonical provenance type the transport/cache execution
    /// seams stamp); `None` for a page no audited path stamped.
    pub backend_provenance: Option<String>,
    /// Neutral origin of the response representation this bundle was
    /// built from — `"network"`, `"reconstructed_cache"`, or `"synthetic"`.
    /// Read directly from `Page::response_origin()`
    /// (`spider_transport::ResponseOrigin`); `None` for a page no audited
    /// path stamped.
    pub response_origin: Option<String>,
    /// Every observed value of each closed-allowlist, audit-relevant
    /// response header (see [`AUDIT_RESPONSE_HEADER_ALLOWLIST`] and
    /// [`audit_response_headers`]), keyed by lowercase header name in
    /// deterministic (sorted) order, values preserved as raw bytes in
    /// observed order — never collapsed, never lossily re-encoded.
    /// Additive field: `None` on evidence recorded before it existed, and
    /// on any page whose response carried none of the allowlisted
    /// headers. This is never a general response-header capture — in
    /// particular `Set-Cookie` is deliberately never in the allowlist
    /// (see that constant's own doc comment).
    pub response_headers: Option<std::collections::BTreeMap<String, Vec<Vec<u8>>>>,
}

/// The exact, closed set of response headers Scorpion's audit
/// architecture observes. Deliberately small — only headers required or
/// clearly useful for the deterministic audit rules this and successor
/// frontiers implement (starting with `SEO_CANONICAL_MISSING`) — never a
/// blanket capture of the full response `HeaderMap`.
///
/// `Set-Cookie` is deliberately never included: cookie values may carry
/// session identifiers, authentication state, or other bearer-equivalent
/// material, and persisting them raw would reopen credential-persistence
/// risk merely to support passive security analysis. A future
/// passive-security analyzer frontier may add a value-redacted cookie
/// *attribute* representation (`Secure`/`HttpOnly`/`SameSite` presence,
/// never the cookie value) — that is a deliberately separate design
/// decision, not made here.
///
/// `x-generator` was added by
/// `SCORPION_AUDIT_OBSERVED_TECHNOLOGY_MARKERS_001` for the same reason
/// `server`/`x-powered-by` were already here: it is a response header the
/// remote system uses to *voluntarily self-declare* a technology-identifying
/// value (e.g. a CMS setting `X-Generator: Drupal 9`), carries no
/// session/credential/bearer material of its own (unlike `Set-Cookie`,
/// `Authorization`, or `Cookie` — none of which are, or will ever be,
/// allowlisted here), and is read through the exact same closed-allowlist,
/// raw-bytes, never-fabricated capture this function already provides —
/// broadening the allowlist by one more self-declared, non-credential
/// header name preserves every existing privacy/security guarantee this
/// module makes.
pub const AUDIT_RESPONSE_HEADER_ALLOWLIST: &[&str] = &[
    "content-language",
    "x-robots-tag",
    "strict-transport-security",
    "content-security-policy",
    "content-security-policy-report-only",
    "x-frame-options",
    "x-content-type-options",
    "referrer-policy",
    "permissions-policy",
    "access-control-allow-origin",
    "access-control-allow-credentials",
    "access-control-allow-methods",
    "access-control-allow-headers",
    "server",
    "x-powered-by",
    "x-generator",
];

/// Extract every value `headers` actually carries for each
/// [`AUDIT_RESPONSE_HEADER_ALLOWLIST`] name, losslessly — HTTP header
/// values are not guaranteed to be valid UTF-8, so each value is kept as
/// raw bytes rather than risking a lossy `String` conversion — in the
/// exact order `HeaderMap` yields them for that name (multiple values for
/// one name, e.g. repeated `Content-Security-Policy` headers, are
/// preserved individually, never comma-joined). Header names are the
/// allowlist's own lowercase spelling (`HeaderMap` lookup is already
/// case-insensitive). A name with zero observed values is omitted
/// entirely from the result — absence is never fabricated as an empty
/// list — and the returned `BTreeMap` keeps header-name ordering
/// deterministic regardless of the order headers were observed in.
///
/// Truthful for every acquisition path: this reads only what `headers`
/// actually contains, so a Chrome/CDP-derived `HeaderMap` (whatever
/// fidelity that path actually captures) is never embellished with
/// duplicate or fabricated values the browser path did not provide.
pub fn audit_response_headers(
    headers: &reqwest::header::HeaderMap,
) -> std::collections::BTreeMap<String, Vec<Vec<u8>>> {
    let mut observed = std::collections::BTreeMap::new();
    for &name in AUDIT_RESPONSE_HEADER_ALLOWLIST {
        let values: Vec<Vec<u8>> = headers
            .get_all(name)
            .iter()
            .map(|value| value.as_bytes().to_vec())
            .collect();
        if !values.is_empty() {
            observed.insert(name.to_string(), values);
        }
    }
    observed
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
    let response_headers = page
        .headers
        .as_ref()
        .map(audit_response_headers)
        .filter(|observed| !observed.is_empty());
    let links = page.page_links.as_ref().map(|links| {
        links
            .iter()
            .map(|link| link.inner().to_string())
            .collect::<Vec<_>>()
    });
    let provenance = page_provenance(page);

    EvidenceBundle {
        id: None,
        requested_url: Some(page.get_url().to_string()),
        final_url: Some(page.get_url_final().to_string()),
        retrieved_at: page.get_retrieved_at(),
        status_code: Some(page.status_code.as_u16()),
        observed_status_code: provenance.observed_status_code,
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
        transport: provenance.transport,
        dns: provenance.dns,
        backend_provenance: provenance.backend_provenance,
        response_origin: provenance.response_origin,
        response_headers,
    }
}

/// The subset of [`EvidenceBundle`]'s provenance fields derivable
/// directly from a [`Page`], without any content/hashing work — the
/// exact same facts and label conventions [`build_evidence`] itself
/// uses (this function *is* that shared derivation; `build_evidence`
/// calls it rather than duplicating it), factored out so a caller that
/// wants only truthful acquisition provenance — not a full
/// content-hashing `EvidenceBundle` — can get it cheaply. `None` for any
/// field the acquisition path never stamped; unknown is never guessed
/// or fabricated. Never a second provenance model: every field name and
/// label string here is identical to `EvidenceBundle`'s own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct PageProvenance {
    /// Which audited transport actually acquired this page — `"default"`
    /// or `"tor"`. See [`EvidenceBundle::transport`].
    pub transport: Option<String>,
    /// How target-hostname DNS resolution was performed. See
    /// [`EvidenceBundle::dns`].
    pub dns: Option<String>,
    /// Which backend observed or reconstructed this response. See
    /// [`EvidenceBundle::backend_provenance`].
    pub backend_provenance: Option<String>,
    /// Neutral origin of this response representation. See
    /// [`EvidenceBundle::response_origin`].
    pub response_origin: Option<String>,
    /// HTTP status actually observed from a response or trusted relay,
    /// independent of Spider's effective/crawler `status_code` — a
    /// truthful signal that a page was reclassified (retry policy,
    /// operational recovery) even when the final status looks ordinary.
    /// See [`EvidenceBundle::observed_status_code`].
    pub observed_status_code: Option<u16>,
}

/// Derive [`PageProvenance`] for `page`. See that type's own doc comment.
pub fn page_provenance(page: &Page) -> PageProvenance {
    let observed_transport = page.transport();
    PageProvenance {
        transport: observed_transport.map(|transport| transport.label().to_string()),
        dns: match observed_transport {
            Some(transport::AcquisitionTransport::Tor) => Some("proxy".to_string()),
            _ => None,
        },
        backend_provenance: page
            .backend_provenance()
            .map(|backend| backend_provenance_label(backend).to_string()),
        response_origin: page
            .response_origin()
            .map(|origin| response_origin_label(origin).to_string()),
        observed_status_code: page.observed_status_code.map(|status| status.as_u16()),
    }
}

/// Stringify [`spider_transport::BackendProvenance`] for
/// [`EvidenceBundle::backend_provenance`]. Truthful presentation of an
/// already-observed fact, not a new provenance source — mirrors
/// `AcquisitionTransport::label()`'s existing style.
fn backend_provenance_label(backend: spider_transport::BackendProvenance) -> &'static str {
    match backend {
        spider_transport::BackendProvenance::Reqwest => "reqwest",
        spider_transport::BackendProvenance::Wreq => "wreq",
        spider_transport::BackendProvenance::CacheLayer => "cache_layer",
        spider_transport::BackendProvenance::NoncanonicalFetchEngine => "noncanonical_fetch_engine",
        spider_transport::BackendProvenance::NoncanonicalRemoteFetcher => {
            "noncanonical_remote_fetcher"
        }
        spider_transport::BackendProvenance::UpstreamCompatibility => "upstream_compatibility",
    }
}

/// Stringify [`spider_transport::ResponseOrigin`] for
/// [`EvidenceBundle::response_origin`]. Truthful presentation of an
/// already-observed fact, not a new provenance source.
fn response_origin_label(origin: spider_transport::ResponseOrigin) -> &'static str {
    match origin {
        spider_transport::ResponseOrigin::Network => "network",
        spider_transport::ResponseOrigin::ReconstructedCache => "reconstructed_cache",
        spider_transport::ResponseOrigin::Synthetic => "synthetic",
    }
}

// ---------------------------------------------------------------------------
// Canonical content classification (SCORPION_CANONICAL_PUBLIC_SURFACE_
// OWNERSHIP_CONVERGENCE_001). Was previously reimplemented independently
// inside spider_mcp's scrape tool (`route_auto_http`/`declared_mime`) —
// provider/interface-neutral "what kind of content is this" classification
// is exactly what `build_evidence` above already partially derives
// (`detected_content_type` via `infer::get`); these two primitives make
// that reconciliation canonical too, so no interface need reimplement it.
// Deliberately scoped to only the two genuinely provider/interface-neutral,
// self-contained pieces: normalizing a declared header, and categorizing a
// byte-signature match. The remaining decision — what to do when *no*
// byte-signature was found at all (declared-header fallback branching, the
// safely-textual-bytes heuristic, which output format/error message to
// produce) — stays caller/interface-owned: it is tool-contract policy
// (e.g. MCP's own `return_format="auto"` semantics), not a universal fact
// about the bytes, and a different interface could reasonably choose
// differently.
// ---------------------------------------------------------------------------

/// Normalize a declared `Content-Type` header value down to its base MIME
/// type: strips parameters (e.g. `; charset=utf-8`), trims whitespace,
/// lowercases. `None` for an absent or empty header. Never rewrites or
/// second-guesses the header's own value — purely mechanical extraction of
/// the base type token.
pub fn declared_mime(content_type: Option<&str>) -> Option<String> {
    content_type
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

/// Coarse category for a positive byte-signature match (via `infer`).
/// `None` from [`classify_detected_content`] means no signature was found
/// at all — not a claim that the content is one of these categories by
/// elimination; the caller decides its own fallback policy for that case
/// (e.g. consulting the declared header, or [`declared_mime`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectedContentClass {
    /// Byte-signature identifies HTML.
    Html,
    /// Byte-signature identifies XML.
    Xml,
    /// Byte-signature identifies PDF.
    Pdf,
    /// Byte-signature identifies image content.
    Image,
    /// Byte-signature identifies audio or video content.
    AudioVideo,
    /// A byte-signature was found but names a binary format with no more
    /// specific category above.
    UnclassifiedBinary,
}

/// Classify `bytes` by byte-signature alone (via `infer::get`) —
/// independent of any declared header, and independent of any interface's
/// chosen output representation. Pure classification only: never decodes,
/// transforms, or extracts the bytes, and never dictates an output format
/// or error message.
pub fn classify_detected_content(bytes: &[u8]) -> Option<DetectedContentClass> {
    infer::get(bytes).map(|kind| match kind.mime_type() {
        "text/html" => DetectedContentClass::Html,
        "text/xml" | "application/xml" => DetectedContentClass::Xml,
        "application/pdf" => DetectedContentClass::Pdf,
        mime if mime.starts_with("image/") => DetectedContentClass::Image,
        mime if mime.starts_with("video/") || mime.starts_with("audio/") => {
            DetectedContentClass::AudioVideo
        }
        _ => DetectedContentClass::UnclassifiedBinary,
    })
}

// ---------------------------------------------------------------------------
// Durable evidence ledger (Track 4:
// SCORPION_DURABLE_EVIDENCE_LEDGER_001). Extends this module's existing
// EvidenceBundle/build_evidence/sha256_hex ownership rather than starting a
// second evidence model — see this file's own module doc comment.
// ---------------------------------------------------------------------------

/// Failure recording or reading durable evidence. Storage-shaped only —
/// wraps [`crate::features::domain_persistence::PersistenceError`]
/// unchanged plus a serialization failure, inventing no evidence-domain
/// error vocabulary of its own.
#[cfg(feature = "disk")]
#[derive(Debug)]
pub enum EvidenceLedgerError {
    /// A duplicate `EvidenceId` (or any other persistence-layer conflict
    /// or backend failure). See
    /// [`crate::features::domain_persistence::PersistenceError`] for the
    /// specific reason.
    Persistence(crate::features::domain_persistence::PersistenceError),
    /// The bundle could not be encoded/decoded. The evidence content
    /// itself was never in question; this is strictly a serialization
    /// failure.
    Serialization(serde_json::Error),
}

#[cfg(feature = "disk")]
impl std::fmt::Display for EvidenceLedgerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvidenceLedgerError::Persistence(error) => write!(f, "evidence ledger: {error}"),
            EvidenceLedgerError::Serialization(error) => {
                write!(f, "evidence ledger: bundle serialization failed: {error}")
            }
        }
    }
}

#[cfg(feature = "disk")]
impl std::error::Error for EvidenceLedgerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EvidenceLedgerError::Persistence(error) => Some(error),
            EvidenceLedgerError::Serialization(error) => Some(error),
        }
    }
}

/// Assign `bundle` a durable [`EvidenceId`] (minting a fresh one via
/// [`EvidenceId::new`] unless the caller already set `bundle.id`) and
/// append it to the durable evidence ledger through
/// [`crate::features::domain_persistence::DomainPersistence`]'s
/// append-only historical semantics — never its current-state
/// compare-and-swap semantics, because evidence has no "current state" to
/// replace: it is immutable and historical from the moment it is
/// recorded. Every write uses the fixed revision `1` (the one and only
/// record an `EvidenceId` will ever have), so [`DomainPersistence::append_history`]'s
/// existing `(identity, revision)` uniqueness constraint is exactly what
/// makes a duplicate `EvidenceId` write fail closed here — nothing in
/// this function decides that; it inherits it unchanged from Track 3.
///
/// On success, returns `bundle` with `id` populated (the identity that
/// was actually written), ready to be named by an [`EvidenceRef`].
///
/// This function neither fabricates nor alters any provenance field:
/// `bundle`'s existing fields (as built by [`build_evidence`]) are
/// persisted exactly as given, byte-for-byte, via `serde_json`.
///
/// [`DomainPersistence::append_history`]: crate::features::domain_persistence::DomainPersistence::append_history
#[cfg(feature = "disk")]
pub async fn record_evidence(
    store: &crate::features::domain_persistence::DomainPersistence,
    mut bundle: EvidenceBundle,
) -> Result<EvidenceBundle, EvidenceLedgerError> {
    let id = bundle.id.unwrap_or_default();
    bundle.id = Some(id);

    let payload = serde_json::to_vec(&bundle).map_err(EvidenceLedgerError::Serialization)?;

    store
        .append_history(&id.to_string(), 1, &payload, std::time::SystemTime::now())
        .await
        .map_err(EvidenceLedgerError::Persistence)?;

    Ok(bundle)
}

/// Read back the durable evidence record for `id`, exactly as
/// [`record_evidence`] wrote it — no reconstruction, no re-derivation
/// from any other source. `Ok(None)` when nothing has ever been recorded
/// for this identity.
#[cfg(feature = "disk")]
pub async fn read_evidence(
    store: &crate::features::domain_persistence::DomainPersistence,
    id: EvidenceId,
) -> Result<Option<EvidenceBundle>, EvidenceLedgerError> {
    let history = store
        .read_history(&id.to_string())
        .await
        .map_err(EvidenceLedgerError::Persistence)?;

    match history.into_iter().next() {
        Some((_revision, payload, _recorded_at)) => {
            let bundle: EvidenceBundle =
                serde_json::from_slice(&payload).map_err(EvidenceLedgerError::Serialization)?;
            Ok(Some(bundle))
        }
        None => Ok(None),
    }
}

/// A neutral reference to one durable evidence record — names it without
/// carrying its payload, so later Watch/Change/Lineage frontiers can hold
/// this cheaply (16 bytes, `Copy`) and resolve it back to the full
/// [`EvidenceBundle`] via [`EvidenceRef::resolve`] only when they actually
/// need the content. This is a pure identity wrapper: it stores nothing
/// about evidence content, decides no domain semantics, and is not a
/// second evidence model.
#[cfg(feature = "disk")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct EvidenceRef {
    id: EvidenceId,
}

#[cfg(feature = "disk")]
impl EvidenceRef {
    /// Reference the evidence record named by `id`. Does not verify that
    /// record exists — construction is a pure value operation, exactly
    /// like [`EvidenceId`] itself; use [`EvidenceRef::resolve`] to find
    /// out.
    pub fn new(id: EvidenceId) -> Self {
        Self { id }
    }

    /// The identity this reference names.
    pub fn id(&self) -> EvidenceId {
        self.id
    }

    /// Resolve this reference to the full durable evidence record,
    /// through the same [`read_evidence`] every other caller uses.
    pub async fn resolve(
        &self,
        store: &crate::features::domain_persistence::DomainPersistence,
    ) -> Result<Option<EvidenceBundle>, EvidenceLedgerError> {
        read_evidence(store, self.id).await
    }
}

#[cfg(feature = "disk")]
impl From<EvidenceId> for EvidenceRef {
    fn from(id: EvidenceId) -> Self {
        Self::new(id)
    }
}

/// Fetch exactly one page through Spider's ordinary non-browser HTTP path:
/// one target URL, no Chrome/browser fallback, no discovered-link
/// traversal. The canonical one-shot acquisition primitive shared by every
/// discovery adapter (feed/sitemap/news-sitemap/robots-sitemap) and any
/// evidence-first single-resource fetch, in both the MCP server and the
/// CLI.
pub async fn fetch_single_page(url: &str) -> Result<Page, String> {
    #[cfg(feature = "wreq")]
    {
        let _ = url;
        Err("canonical evidence acquisition is unavailable under wreq".to_string())
    }
    #[cfg(not(feature = "wreq"))]
    {
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
#[cfg(all(feature = "transport_tor", not(feature = "wreq")))]
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

/// `transport_tor` is compiled with `wreq`, whose alternate client stack is
/// not Tor-audited (see [`fetch_via_tor`]
/// above), so the combination is rejected explicitly rather than silently
/// using an unaudited client.
#[cfg(all(feature = "transport_tor", feature = "wreq"))]
async fn fetch_via_tor(_url: &str, _policy: &TransportPolicy) -> Result<Page, String> {
    let message = "Tor transport requires a build without the wreq feature — \
         that alternate client stack is not audited for Tor-safe (fail-closed) behavior"
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
        assert_eq!(bundle.id, None);
        assert_eq!(bundle.transport, None);
        assert_eq!(bundle.dns, None);
        assert_eq!(bundle.backend_provenance, None);
        assert_eq!(bundle.response_origin, None);
        assert_eq!(bundle.response_headers, None);
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

    mod page_provenance_tests {
        use super::*;
        use crate::features::transport::AcquisitionTransport;
        use crate::page::build;
        use crate::utils::PageResponse;

        #[test]
        fn unstamped_page_reports_all_provenance_as_none() {
            let page = Page::default();
            let provenance = page_provenance(&page);
            assert_eq!(provenance, PageProvenance::default());
        }

        #[test]
        fn default_transport_reports_default_label_and_no_dns_proxy_fact() {
            let mut page = Page::default();
            page.transport = Some(AcquisitionTransport::Default);
            let provenance = page_provenance(&page);
            assert_eq!(provenance.transport.as_deref(), Some("default"));
            assert_eq!(provenance.dns, None);
        }

        #[test]
        fn tor_transport_reports_tor_label_and_proxy_dns_fact() {
            let mut page = Page::default();
            page.transport = Some(AcquisitionTransport::Tor);
            let provenance = page_provenance(&page);
            assert_eq!(provenance.transport.as_deref(), Some("tor"));
            assert_eq!(provenance.dns.as_deref(), Some("proxy"));
        }

        #[test]
        fn backend_and_response_origin_are_read_through_not_fabricated() {
            let mut page = Page::default();
            page.backend = Some(spider_transport::BackendProvenance::Wreq);
            page.response_origin = Some(spider_transport::ResponseOrigin::ReconstructedCache);
            let provenance = page_provenance(&page);
            assert_eq!(provenance.backend_provenance.as_deref(), Some("wreq"));
            assert_eq!(
                provenance.response_origin.as_deref(),
                Some("reconstructed_cache")
            );
        }

        #[test]
        fn observed_status_code_survives_effective_reclassification() {
            let page = build(
                "https://example.test/",
                PageResponse {
                    content: Some(Vec::new()),
                    status_code: reqwest::StatusCode::OK,
                    observed_status_code: Some(reqwest::StatusCode::OK),
                    ..Default::default()
                },
            );
            // Truthful even when Spider's own effective status differs —
            // this is exactly the signal EvidenceBundle::observed_status_code
            // already preserves; page_provenance must not drop it.
            let provenance = page_provenance(&page);
            assert_eq!(provenance.observed_status_code, Some(200));
        }

        #[test]
        fn build_evidence_and_page_provenance_agree_exactly() {
            let mut page = Page::default();
            page.transport = Some(AcquisitionTransport::Tor);
            page.backend = Some(spider_transport::BackendProvenance::CacheLayer);
            page.response_origin = Some(spider_transport::ResponseOrigin::Synthetic);

            let provenance = page_provenance(&page);
            let bundle = build_evidence(&page, None, false, false);

            assert_eq!(bundle.transport, provenance.transport);
            assert_eq!(bundle.dns, provenance.dns);
            assert_eq!(bundle.backend_provenance, provenance.backend_provenance);
            assert_eq!(bundle.response_origin, provenance.response_origin);
            assert_eq!(bundle.observed_status_code, provenance.observed_status_code);
        }
    }

    mod audit_response_header_tests {
        use super::*;
        use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

        fn header(name: &'static str, value: &str) -> (HeaderName, HeaderValue) {
            (
                HeaderName::from_static(name),
                HeaderValue::from_str(value).unwrap(),
            )
        }

        // A/B/C: individually allowlisted headers survive.
        #[test]
        fn hsts_header_survives_into_audit_evidence() {
            let mut headers = HeaderMap::new();
            let (name, value) = header("strict-transport-security", "max-age=63072000");
            headers.insert(name, value);
            let observed = audit_response_headers(&headers);
            assert_eq!(
                observed.get("strict-transport-security"),
                Some(&vec![b"max-age=63072000".to_vec()])
            );
        }

        #[test]
        fn csp_header_survives_into_audit_evidence() {
            let mut headers = HeaderMap::new();
            let (name, value) = header("content-security-policy", "default-src 'self'");
            headers.insert(name, value);
            let observed = audit_response_headers(&headers);
            assert_eq!(
                observed.get("content-security-policy"),
                Some(&vec![b"default-src 'self'".to_vec()])
            );
        }

        #[test]
        fn x_robots_tag_header_survives_into_audit_evidence() {
            let mut headers = HeaderMap::new();
            let (name, value) = header("x-robots-tag", "noindex");
            headers.insert(name, value);
            let observed = audit_response_headers(&headers);
            assert_eq!(
                observed.get("x-robots-tag"),
                Some(&vec![b"noindex".to_vec()])
            );
        }

        // D: server/x-powered-by survive when present.
        #[test]
        fn server_and_x_powered_by_survive_when_present() {
            let mut headers = HeaderMap::new();
            let (n1, v1) = header("server", "nginx");
            let (n2, v2) = header("x-powered-by", "Express");
            headers.insert(n1, v1);
            headers.insert(n2, v2);
            let observed = audit_response_headers(&headers);
            assert_eq!(observed.get("server"), Some(&vec![b"nginx".to_vec()]));
            assert_eq!(
                observed.get("x-powered-by"),
                Some(&vec![b"Express".to_vec()])
            );
        }

        // D2: x-generator survives when present (added by
        // SCORPION_AUDIT_OBSERVED_TECHNOLOGY_MARKERS_001 — see this
        // constant's own doc comment for why).
        #[test]
        fn x_generator_survives_when_present() {
            let mut headers = HeaderMap::new();
            let (name, value) = header("x-generator", "Drupal 9 (https://www.drupal.org)");
            headers.insert(name, value);
            let observed = audit_response_headers(&headers);
            assert_eq!(
                observed.get("x-generator"),
                Some(&vec![b"Drupal 9 (https://www.drupal.org)".to_vec()])
            );
        }

        // E: multiple values for one retained header preserve every value.
        #[test]
        fn multiple_values_for_one_header_are_all_preserved() {
            let mut headers = HeaderMap::new();
            let (name, v1) = header("content-security-policy", "default-src 'self'");
            let (_, v2) = header("content-security-policy", "report-uri /csp-report");
            headers.append(name, v1);
            headers.append(HeaderName::from_static("content-security-policy"), v2);
            let observed = audit_response_headers(&headers);
            assert_eq!(
                observed.get("content-security-policy"),
                Some(&vec![
                    b"default-src 'self'".to_vec(),
                    b"report-uri /csp-report".to_vec(),
                ])
            );
        }

        // F: header-name ordering is deterministic (sorted), independent of
        // insertion order.
        #[test]
        fn header_name_ordering_is_deterministic() {
            let mut headers = HeaderMap::new();
            let (n1, v1) = header("x-powered-by", "Express");
            let (n2, v2) = header("server", "nginx");
            let (n3, v3) = header("x-frame-options", "DENY");
            headers.insert(n1, v1);
            headers.insert(n2, v2);
            headers.insert(n3, v3);
            let observed = audit_response_headers(&headers);
            let names: Vec<&str> = observed.keys().map(String::as_str).collect();
            assert_eq!(names, vec!["server", "x-frame-options", "x-powered-by"]);
        }

        // G: old EvidenceBundle JSON without the new field still
        // deserializes, and the new field is truthfully absent.
        #[test]
        fn old_evidence_bundle_json_without_header_field_still_deserializes() {
            let old_json = r#"{
                "requested_url": "https://example.test/",
                "status_code": 200
            }"#;
            let bundle: EvidenceBundle = serde_json::from_str(old_json).unwrap();
            assert_eq!(
                bundle.requested_url.as_deref(),
                Some("https://example.test/")
            );
            assert_eq!(bundle.response_headers, None);
        }

        // H: missing headers remain absent, never fabricated.
        #[test]
        fn absent_headers_are_never_fabricated() {
            let headers = HeaderMap::new();
            let observed = audit_response_headers(&headers);
            assert!(observed.is_empty());

            // Even with unrelated headers present, no allowlisted name is
            // fabricated into the result.
            let mut headers = HeaderMap::new();
            let (name, value) = header("x-custom-app-header", "irrelevant");
            headers.insert(name, value);
            let observed = audit_response_headers(&headers);
            assert!(observed.is_empty());
        }

        // I: the Set-Cookie raw value sentinel is never persisted in the
        // new header evidence field — proven both at the extraction
        // function and through the full build_evidence path.
        #[test]
        fn set_cookie_raw_value_is_never_persisted_in_audit_header_evidence() {
            const SENTINEL: &str = "SUPER_SECRET_SENTINEL";
            let mut headers = HeaderMap::new();
            let (name, value) = header(
                "set-cookie",
                &format!("session={SENTINEL}; Secure; HttpOnly"),
            );
            headers.insert(name, value);
            let observed = audit_response_headers(&headers);
            assert!(observed.is_empty(), "set-cookie must never be allowlisted");
            for values in observed.values() {
                for value in values {
                    assert!(!value
                        .windows(SENTINEL.len())
                        .any(|w| w == SENTINEL.as_bytes()));
                }
            }

            // End-to-end through build_evidence: the sentinel must not
            // appear anywhere in the resulting EvidenceBundle, serialized
            // or not.
            let page = crate::page::build(
                "https://example.test/",
                crate::utils::PageResponse {
                    content: Some(b"<html></html>".to_vec()),
                    status_code: reqwest::StatusCode::OK,
                    headers: Some(headers),
                    ..Default::default()
                },
            );
            let bundle = build_evidence(&page, Some("raw".to_string()), false, false);
            assert_eq!(bundle.response_headers, None);
            let serialized = serde_json::to_string(&bundle).unwrap();
            assert!(!serialized.contains(SENTINEL));
        }

        // I2 (SCORPION_AUDIT_OBSERVED_TECHNOLOGY_MARKERS_001): the
        // allowlist itself — not just runtime behavior for one sentinel
        // header — structurally excludes every credential/session-bearing
        // header name, and stays a small, fixed, closed list. Added
        // alongside the `x-generator` extension to prove that extension
        // preserved this invariant rather than merely asserting it in
        // prose.
        #[test]
        fn allowlist_structurally_excludes_credential_bearing_headers_and_stays_bounded() {
            for forbidden in [
                "authorization",
                "proxy-authorization",
                "cookie",
                "set-cookie",
            ] {
                assert!(
                    !AUDIT_RESPONSE_HEADER_ALLOWLIST.contains(&forbidden),
                    "AUDIT_RESPONSE_HEADER_ALLOWLIST must never contain \
                     {forbidden:?} — that would reopen credential/session \
                     persistence risk"
                );
            }
            // Every entry is lowercase (HeaderMap lookup is
            // case-insensitive, but the allowlist's own spelling must
            // stay canonical) and the list is small/closed — a bulk
            // capture-everything allowlist is exactly what this
            // constant's own doc comment says it must never become.
            for name in AUDIT_RESPONSE_HEADER_ALLOWLIST {
                assert_eq!(*name, name.to_lowercase());
            }
            assert!(
                AUDIT_RESPONSE_HEADER_ALLOWLIST.len() <= 20,
                "AUDIT_RESPONSE_HEADER_ALLOWLIST grew unexpectedly large \
                 ({} entries) — this is a deliberately small, closed \
                 allowlist, never a general response-header capture",
                AUDIT_RESPONSE_HEADER_ALLOWLIST.len()
            );
        }

        // J: Chrome-limited (or any path's) fidelity is never embellished
        // beyond what Page actually carries — only the headers genuinely
        // present are ever returned, nothing else fabricated to "fill
        // out" the allowlist.
        #[test]
        fn fidelity_never_exceeds_what_page_actually_carries() {
            let mut headers = HeaderMap::new();
            let (name, value) = header("x-content-type-options", "nosniff");
            headers.insert(name, value);
            let page = crate::page::build(
                "https://example.test/",
                crate::utils::PageResponse {
                    content: Some(b"<html></html>".to_vec()),
                    status_code: reqwest::StatusCode::OK,
                    headers: Some(headers),
                    ..Default::default()
                },
            );
            let bundle = build_evidence(&page, Some("raw".to_string()), false, false);
            let observed = bundle.response_headers.expect("one header observed");
            assert_eq!(observed.len(), 1);
            assert_eq!(
                observed.get("x-content-type-options"),
                Some(&vec![b"nosniff".to_vec()])
            );
        }

        // Byte-fidelity: a non-UTF-8 header value is preserved exactly,
        // never dropped or lossily converted.
        #[test]
        fn non_utf8_header_value_is_preserved_losslessly() {
            let mut headers = HeaderMap::new();
            let raw_bytes = vec![0x78, 0xFF, 0xFE, 0x79]; // not valid UTF-8
            headers.insert(
                HeaderName::from_static("server"),
                HeaderValue::from_bytes(&raw_bytes).unwrap(),
            );
            let observed = audit_response_headers(&headers);
            assert_eq!(observed.get("server"), Some(&vec![raw_bytes]));
        }
    }

    mod content_classification {
        use super::*;

        #[test]
        fn detected_html_classifies_as_html() {
            assert_eq!(
                classify_detected_content(b"<!DOCTYPE html><html><body>Hello</body></html>"),
                Some(DetectedContentClass::Html)
            );
        }

        #[test]
        fn detected_xml_classifies_as_xml() {
            let xml = b"<?xml version=\"1.0\"?><root><item>value</item></root>";
            assert_eq!(
                classify_detected_content(xml),
                Some(DetectedContentClass::Xml)
            );
        }

        #[test]
        fn detected_pdf_classifies_as_pdf() {
            assert_eq!(
                classify_detected_content(b"%PDF-1.7\nbody"),
                Some(DetectedContentClass::Pdf)
            );
        }

        #[test]
        fn detected_image_and_audio_video_classify_distinctly() {
            assert_eq!(
                classify_detected_content(b"\x89PNG\r\n\x1a\nbytes"),
                Some(DetectedContentClass::Image)
            );
            assert_eq!(
                classify_detected_content(b"\x00\x00\x00\x18ftypmp42\x00\x00\x00\x00mp42isom"),
                Some(DetectedContentClass::AudioVideo)
            );
        }

        #[test]
        fn detected_but_unmapped_signature_is_unclassified_binary() {
            // A known signature (zip) with no dedicated category falls
            // through the detected-binary catch-all.
            assert_eq!(
                classify_detected_content(b"PK\x03\x04\x14\x00\x00\x00\x00\x00known-archive"),
                Some(DetectedContentClass::UnclassifiedBinary)
            );
        }

        #[test]
        fn no_signature_is_none_never_a_guessed_category() {
            assert_eq!(classify_detected_content(b"mystery bytes"), None);
        }

        #[test]
        fn declared_mime_strips_parameters_trims_and_lowercases() {
            assert_eq!(
                declared_mime(Some(" Application/JSON; charset=utf-8 ")),
                Some("application/json".to_string())
            );
            assert_eq!(declared_mime(Some("")), None);
            assert_eq!(declared_mime(None), None);
        }
    }

    #[cfg(feature = "disk")]
    mod ledger {
        use super::*;
        use crate::features::domain_persistence::{DomainPersistence, PersistenceError};

        fn sample_bundle() -> EvidenceBundle {
            EvidenceBundle {
                requested_url: Some("https://example.test/".to_string()),
                final_url: Some("https://example.test/".to_string()),
                retrieved_at: Some(1_700_000_000_000),
                status_code: Some(200),
                observed_status_code: Some(200),
                transport: Some("default".to_string()),
                backend_provenance: Some("reqwest".to_string()),
                response_origin: Some("network".to_string()),
                content: Some("hello".to_string()),
                ..Default::default()
            }
        }

        #[tokio::test]
        async fn record_evidence_assigns_id_and_reads_back_truthfully() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let bundle = sample_bundle();

            let recorded = record_evidence(&store, bundle.clone()).await.unwrap();
            let id = recorded.id.expect("record_evidence must assign an id");

            let read_back = read_evidence(&store, id).await.unwrap().unwrap();
            assert_eq!(read_back.id, Some(id));
            assert_eq!(read_back.requested_url, bundle.requested_url);
            assert_eq!(read_back.final_url, bundle.final_url);
            assert_eq!(read_back.retrieved_at, bundle.retrieved_at);
            assert_eq!(read_back.status_code, bundle.status_code);
            assert_eq!(read_back.observed_status_code, bundle.observed_status_code);
            assert_eq!(read_back.transport, bundle.transport);
            assert_eq!(read_back.backend_provenance, bundle.backend_provenance);
            assert_eq!(read_back.response_origin, bundle.response_origin);
            assert_eq!(read_back.content, bundle.content);
        }

        #[tokio::test]
        async fn record_evidence_never_fabricates_absent_provenance() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            // A bundle with no provenance captured at all (e.g. a page no
            // audited acquisition path stamped) must read back with every
            // provenance field still absent — never invented at write or
            // read time.
            let bundle = EvidenceBundle {
                requested_url: Some("https://example.test/".to_string()),
                ..Default::default()
            };
            let recorded = record_evidence(&store, bundle).await.unwrap();
            let id = recorded.id.unwrap();
            let read_back = read_evidence(&store, id).await.unwrap().unwrap();
            assert_eq!(read_back.transport, None);
            assert_eq!(read_back.dns, None);
            assert_eq!(read_back.backend_provenance, None);
            assert_eq!(read_back.response_origin, None);
            assert_eq!(read_back.source, None);
            assert_eq!(read_back.provider, None);
            assert_eq!(read_back.query, None);
        }

        #[tokio::test]
        async fn read_evidence_of_unknown_id_is_none() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            assert!(read_evidence(&store, EvidenceId::new())
                .await
                .unwrap()
                .is_none());
        }

        #[tokio::test]
        async fn duplicate_evidence_id_write_fails_closed_and_leaves_original_untouched() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let first = record_evidence(&store, sample_bundle()).await.unwrap();
            let id = first.id.unwrap();

            // Attempt to record a second, different bundle under the same
            // already-written EvidenceId.
            let mut second_attempt = sample_bundle();
            second_attempt.id = Some(id);
            second_attempt.content = Some("attempted-overwrite".to_string());

            let error = record_evidence(&store, second_attempt).await.unwrap_err();
            assert!(matches!(
                error,
                EvidenceLedgerError::Persistence(PersistenceError::HistoryAlreadyExists)
            ));

            // The original record is completely untouched.
            let read_back = read_evidence(&store, id).await.unwrap().unwrap();
            assert_eq!(read_back.content, first.content);
            assert_ne!(read_back.content, Some("attempted-overwrite".to_string()));
        }

        #[tokio::test]
        async fn distinct_bundles_get_distinct_ids_and_do_not_collide() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let a = record_evidence(&store, sample_bundle()).await.unwrap();
            let b = record_evidence(&store, sample_bundle()).await.unwrap();
            assert_ne!(a.id, b.id);
            assert!(read_evidence(&store, a.id.unwrap())
                .await
                .unwrap()
                .is_some());
            assert!(read_evidence(&store, b.id.unwrap())
                .await
                .unwrap()
                .is_some());
        }

        #[tokio::test]
        async fn evidence_ref_resolves_to_the_intended_evidence_id() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let recorded = record_evidence(&store, sample_bundle()).await.unwrap();
            let id = recorded.id.unwrap();

            let evidence_ref = EvidenceRef::new(id);
            assert_eq!(evidence_ref.id(), id);

            let resolved = evidence_ref.resolve(&store).await.unwrap().unwrap();
            assert_eq!(resolved.id, Some(id));
            assert_eq!(resolved.requested_url, recorded.requested_url);
        }

        #[tokio::test]
        async fn evidence_ref_from_id_matches_new() {
            let id = EvidenceId::new();
            assert_eq!(EvidenceRef::from(id), EvidenceRef::new(id));
        }

        #[tokio::test]
        async fn evidence_ref_of_unrecorded_id_resolves_to_none() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let evidence_ref = EvidenceRef::new(EvidenceId::new());
            assert!(evidence_ref.resolve(&store).await.unwrap().is_none());
        }
    }
}
