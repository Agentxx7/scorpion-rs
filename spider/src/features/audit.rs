//! Canonical audit fact/finding contract — the smallest architecture that
//! proves:
//!
//! ```text
//! one actually acquired Page
//!   -> truthful persisted Evidence
//!   -> transient deterministic PageFacts
//!   -> deterministic audit rule
//!   -> evidence-linked Finding
//!   -> durable Finding readback
//! ```
//!
//! `SCORPION_AUDIT_FACTS_AND_FINDING_CONTRACT_FRONTIER_001` established
//! this contract with exactly one rule.
//! `SCORPION_AUDIT_DETERMINISTIC_PAGE_ANALYZERS_001` turned it into the
//! first real deterministic page-audit engine: eleven production page
//! rules (seven SEO, four passive security) evaluated purely over one
//! [`EvidencedPageFacts`] value by [`analyze_page`], executed by the
//! single generic seam [`audit_page`].
//! `SCORPION_AUDIT_OBSERVED_TECHNOLOGY_MARKERS_001` added a second,
//! parallel deterministic projection over that same
//! [`EvidencedPageFacts`] value — [`extract_technology_markers`] — for
//! technology-identifying values the page directly exposed. A marker is
//! not a `Finding` (see "Technology boundary" below); [`audit_page`]
//! still performs exactly one acquisition either way.
//!
//! # Fetch once, evidence once, parse once, analyze many
//!
//! [`audit_page`] performs exactly one acquisition
//! ([`fetch_single_page`]) and exactly one evidence recording
//! ([`EvidencedPageFacts::record`]) per audited page — never once per
//! rule, and never a second acquisition for technology-marker
//! extraction either. [`PageFacts::from_page`] performs exactly one
//! HTML fact extraction pass ([`extract_html_facts`]) covering every
//! authorized DOM fact — including `<meta name="generator">` capture —
//! not one `lol_html` parse per rule and not a second parse for
//! markers. Every `Finding` [`analyze_page`] returns and every
//! [`ObservedTechnologyMarker`] [`extract_technology_markers`] returns,
//! for one page, therefore carries the exact same [`EvidenceRef`] —
//! proven in this module's own `exact_page_evidence_binding`,
//! `same_evidence_across_rules`, and `technology_markers` test modules.
//!
//! # Applicability
//!
//! Reproduced empirically before this frontier's mutation: the original
//! single rule had no content-type or status applicability check at
//! all, and produced a false-positive `SEO_CANONICAL_MISSING` Finding
//! for a `200 text/plain` response, a `200 application/json` response,
//! and a `404 text/html` error document. Every HTML DOM SEO rule in this
//! module now reuses one shared predicate,
//! [`page_content_seo_applicable`]: the final response must be a
//! successful 2xx *and* the observed representation must be
//! HTML/XHTML ([`DocumentRepresentation::Html`], derived only from the
//! declared `Content-Type` — never body sniffing, filename extensions,
//! search metadata, or AI). Passive security/transport rules state their
//! own applicability explicitly where it differs (see
//! [`security_https_missing`], [`security_hsts_missing`]).
//!
//! # Header observation vs. header absence
//!
//! [`PageFacts::response_headers`] is `Option<BTreeMap<..>>`, not a bare
//! `BTreeMap`: `None` means header observation itself was unavailable
//! (`Page.headers` was `None`), `Some(map)` — possibly empty — means
//! headers *were* observed and `map` names every allowlisted header
//! actually present. A security-header-absence rule
//! ([`security_hsts_missing`], [`security_csp_missing`],
//! [`security_x_content_type_options_missing`]) requires `Some(_)`
//! before it can produce a Finding at all; when observation itself is
//! unknown, no Finding is produced — absence is never inferred from an
//! observation surface that cannot prove it.
//!
//! # Truth chain
//!
//! `ACQUIRED -> OBSERVED -> EVIDENCED -> DERIVED FINDING`. There is no
//! fourth canonical stage named "ANALYZED": [`Finding`] is a derived
//! record that *references* evidence by [`EvidenceRef`] — it is never
//! itself `Evidence`, and it can never be recorded, read back, or
//! serialized as an [`EvidenceBundle`].
//!
//! # Discovery/evidence boundary
//!
//! Discovery may select a URL. Only acquisition can establish page
//! evidence. There is no path from `SearchResult`/`SearchResults`/
//! `SearchProvider` into [`PageFacts`], [`EvidencedPageFacts`], or
//! [`Finding`] anywhere in this module — none of those types are
//! imported here at all (enforced by
//! `audit_module_never_imports_website_or_search_provider_types` in
//! `architecture_guardrails.rs`). [`EvidencedPageFacts`] is obtained
//! exclusively through [`EvidencedPageFacts::record`], which receives one
//! `&Page` and derives *both* its evidence recording and its facts from
//! that single value — never by independently pairing a caller-supplied
//! `PageFacts` with a caller-supplied `EvidenceRef` (an earlier revision
//! exposed exactly that pairing as `EvidencedPageFacts::new`; adversarial
//! reproduction proved it let one page's facts be durably linked to a
//! different, validly-resolving page's evidence —
//! `SCORPION_AUDIT_EXACT_PAGE_EVIDENCE_BINDING_CORRECTION_001` fixed this
//! by removing that constructor rather than merely discouraging it), and
//! never by index, completion ordering, or URL-string equality.
//! [`record_finding`] additionally verifies every [`EvidenceRef`] a
//! [`Finding`] carries actually resolves in [`DomainPersistence`] before
//! persisting anything — fail-closed, never accepting an identity-shaped
//! string as proof evidence exists — though that check alone was never
//! sufficient to prevent a mismatched pairing, which is why construction
//! authority lives in [`EvidencedPageFacts::record`] instead.
//!
//! # Finding identity and persistence
//!
//! [`FindingId`] is content-addressed — deterministic SHA-256 over every
//! semantic field that makes two findings meaningfully different
//! (`rule_id`, `rule_version`, `category`, `severity`, `target`,
//! `observed_condition`, `expected_condition`, and the sorted/deduplicated
//! evidence identities) — exactly [`ChangeEventId`]'s and
//! `TransformLineageId`'s own construction pattern, reused verbatim. No
//! wall-clock time participates in identity. [`Finding`] is persisted
//! through [`DomainPersistence::append_history`] only, at fixed revision
//! `1`, exactly like the change-detection and transform-lineage ledgers —
//! recording an identical fact twice is idempotent
//! (`PersistenceError::HistoryAlreadyExists => Ok(finding)`), never a
//! conflict. There is no `write_current` and no second persistence
//! backend anywhere in this module.
//!
//! [`FindingId`] deliberately does **not** live in `features/identity.rs`
//! — this repository's own established precedent
//! ([`ChangeEventId`](crate::features::change_detection::ChangeEventId))
//! is that content-addressed *derived-record* ids live with their domain
//! module, not in the canonical random-mint identity registry.
//!
//! # Scope firewall
//!
//! Exactly eleven production page rules exist — see [`PAGE_RULES`]. Not
//! implemented here: canonical conflict/normalization/loops, hreflang,
//! sitemap/robots.txt/X-Robots-Tag analysis, structured data, OpenGraph
//! scoring, title/description length or keyword recommendations,
//! duplicate titles/descriptions across pages, orphan detection,
//! broken-link graphs; CORS, cookie attribute evaluation, mixed-content
//! graphs, insecure form actions, CSP/XFO/Referrer-Policy/
//! Permissions-Policy *quality* evaluation, certificate analysis, any
//! active probe or exploit technique; any technology/CMS/framework/WAF
//! *fingerprinting*, *inference*, or CVE mapping (permanently out of
//! scope — see "Technology boundary" below; this is distinct from, and
//! never satisfied by, the directly-observed technology markers that
//! section describes); any site-wide analytics (duplicate-title
//! aggregation, orphan detection, canonical loops, redirect graphs,
//! sitemap drift); any network/Nmap capability (`NetworkObservation`,
//! port scanning, service detection, process execution, target
//! admission policy); any CLI/API/MCP/Web Console surface; and no AI
//! (summarization, severity generation, report generation, technology
//! classification). Severity here is fixed rule policy, never observed
//! evidence and never AI-generated narrative — see [`FindingSeverity`].
//! A header-absence or scheme Finding states a deterministic policy
//! check, never a vulnerability, exploitability, or CVE claim.
//!
//! # Technology boundary
//!
//! `SCORPION_AUDIT_OBSERVED_TECHNOLOGY_MARKERS_001` added deterministic
//! extraction of technology-identifying values the remote page
//! *explicitly* exposed — never an inference. This is a genuinely
//! different contract from [`Finding`], not a variant of it: the
//! `Finding` contract is predicate-shaped (observed condition vs.
//! expected condition), and an observation-only fact like `Server:
//! nginx/1.24` has no truthful "expected" counterpart to contort it
//! into, so it is never forced into `FindingCondition`/`Finding`. See
//! [`ObservedTechnologyMarker`], [`TechnologyMarkerSource`], and
//! [`extract_technology_markers`].
//!
//! What counts as an observation here is narrow and closed:
//! [`MARKER_HEADER_NAMES`]'s three response headers (`Server`,
//! `X-Powered-By`, and `X-Generator`, all already
//! [`AUDIT_RESPONSE_HEADER_ALLOWLIST`] members before this frontier
//! except the last, added by it — see that constant's own doc comment)
//! and `<meta name="generator">`'s literal `content` value. Permanently
//! *not* a marker source, no matter how suggestive: a `/wp-content/`
//! path, a `.php` URL, a framework-shaped script `src`, DOM structure,
//! favicon/asset hashing, TLS/JA3 fingerprinting, timing, a regex
//! signature catalog, or any Wappalyzer/BuiltWith-style probabilistic
//! database — none of those are the remote system *declaring* a value,
//! only Scorpion *guessing* one, and Scorpion does not guess. There are
//! no active probes, no hidden-endpoint requests, and no version
//! probing: every marker comes from the exact same one acquisition
//! [`audit_page`] already performed for [`analyze_page`] — see
//! [`extract_technology_markers`]'s own doc comment for the full
//! same-evidence, single-acquisition, parse-once proof. Markers are
//! never persisted as their own durable, independently-identified
//! record in this frontier — see the "Observed technology markers"
//! section directly above [`MARKER_HEADER_NAMES`] in this file for why.

use crate::features::domain_persistence::{DomainPersistence, PersistenceError};
use crate::page::Page;
use crate::utils::evidence::{
    audit_response_headers, build_evidence, fetch_single_page, page_provenance, record_evidence,
    sha256_hex, EvidenceBundle, EvidenceLedgerError, EvidenceRef,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::time::SystemTime;

// Stable semantic identities and deterministic predicate versions for
// every production page rule. Naming follows `<category>.<check>`
// (Phase 21 of the authorizing frontier). A rule's *version* is
// independent of crate/package version — a future behavior change to a
// predicate must bump only that rule's own constant, never
// `Cargo.toml`'s version, and never silently.
//
// `SEO_CANONICAL_MISSING_RULE_VERSION` is `2`, not `1`: the original
// `SCORPION_AUDIT_FACTS_AND_FINDING_CONTRACT_FRONTIER_001` predicate had
// no applicability check at all (reproduced empirically: it false-
// positived on `200 text/plain`, `200 application/json`, and
// `404 text/html`). Historical `version = 1` Finding payloads remain
// fully readable — `Finding`'s shape, `FindingId::derive`'s formula, and
// `FindingCondition::CanonicalLinkCount`/`CanonicalLinkCountAtLeast`'s
// `identity_repr()` are all byte-identical to before; only the *value*
// fed into `rule_version` at execution time changed, which the identity
// formula already treats as ordinary data — see
// `historical_compatibility` in this module's tests.
/// Rule id: zero `<link rel="canonical">` observations.
pub const SEO_CANONICAL_MISSING_RULE_ID: &str = "seo.canonical.missing";
/// Deterministic predicate version for
/// [`SEO_CANONICAL_MISSING_RULE_ID`]. `2`, not `1` — see the module-level
/// comment directly above.
pub const SEO_CANONICAL_MISSING_RULE_VERSION: u32 = 2;

/// Rule id: missing/empty `<title>`.
pub const SEO_TITLE_MISSING_RULE_ID: &str = "seo.title.missing";
/// Deterministic predicate version for [`SEO_TITLE_MISSING_RULE_ID`].
pub const SEO_TITLE_MISSING_RULE_VERSION: u32 = 1;

/// Rule id: missing/empty `<meta name="description">`.
pub const SEO_META_DESCRIPTION_MISSING_RULE_ID: &str = "seo.meta_description.missing";
/// Deterministic predicate version for
/// [`SEO_META_DESCRIPTION_MISSING_RULE_ID`].
pub const SEO_META_DESCRIPTION_MISSING_RULE_VERSION: u32 = 1;

/// Rule id: zero `<h1>` elements.
pub const SEO_H1_MISSING_RULE_ID: &str = "seo.h1.missing";
/// Deterministic predicate version for [`SEO_H1_MISSING_RULE_ID`].
pub const SEO_H1_MISSING_RULE_VERSION: u32 = 1;

/// Rule id: more than one `<h1>` element.
pub const SEO_H1_MULTIPLE_RULE_ID: &str = "seo.h1.multiple";
/// Deterministic predicate version for [`SEO_H1_MULTIPLE_RULE_ID`].
pub const SEO_H1_MULTIPLE_RULE_VERSION: u32 = 1;

/// Rule id: missing/empty `<html lang="...">`.
pub const SEO_HTML_LANG_MISSING_RULE_ID: &str = "seo.html_lang.missing";
/// Deterministic predicate version for [`SEO_HTML_LANG_MISSING_RULE_ID`].
pub const SEO_HTML_LANG_MISSING_RULE_VERSION: u32 = 1;

/// Rule id: one or more `<img>` elements missing an `alt` attribute.
pub const SEO_IMAGE_ALT_MISSING_RULE_ID: &str = "seo.image_alt.missing";
/// Deterministic predicate version for [`SEO_IMAGE_ALT_MISSING_RULE_ID`].
pub const SEO_IMAGE_ALT_MISSING_RULE_VERSION: u32 = 1;

/// Rule id: final URL scheme is `http`, not `https`.
pub const SECURITY_HTTPS_MISSING_RULE_ID: &str = "security.https.missing";
/// Deterministic predicate version for [`SECURITY_HTTPS_MISSING_RULE_ID`].
pub const SECURITY_HTTPS_MISSING_RULE_VERSION: u32 = 1;

/// Rule id: `Strict-Transport-Security` absent on an observed `https` response.
pub const SECURITY_HSTS_MISSING_RULE_ID: &str = "security.hsts.missing";
/// Deterministic predicate version for [`SECURITY_HSTS_MISSING_RULE_ID`].
pub const SECURITY_HSTS_MISSING_RULE_VERSION: u32 = 1;

/// Rule id: `Content-Security-Policy` absent (report-only does not satisfy).
pub const SECURITY_CSP_MISSING_RULE_ID: &str = "security.csp.missing";
/// Deterministic predicate version for [`SECURITY_CSP_MISSING_RULE_ID`].
pub const SECURITY_CSP_MISSING_RULE_VERSION: u32 = 1;

/// Rule id: `X-Content-Type-Options` absent (value not yet evaluated).
pub const SECURITY_X_CONTENT_TYPE_OPTIONS_MISSING_RULE_ID: &str =
    "security.x_content_type_options.missing";
/// Deterministic predicate version for
/// [`SECURITY_X_CONTENT_TYPE_OPTIONS_MISSING_RULE_ID`].
pub const SECURITY_X_CONTENT_TYPE_OPTIONS_MISSING_RULE_VERSION: u32 = 1;

/// Truthful classification of what kind of document representation was
/// observed — derived *only* from the declared `Content-Type` (parameters
/// like `; charset=utf-8` ignored, matched case-insensitively). Never
/// inferred from body sniffing (`bytes.starts_with(b"<html")`), a
/// filename extension, search/discovery metadata, or AI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentRepresentation {
    /// Declared `Content-Type` is `text/html` or `application/xhtml+xml`.
    Html,
    /// A `Content-Type` was observed and it is not HTML/XHTML.
    NonHtml,
    /// No `Content-Type` was observed, or it could not be parsed — there
    /// is insufficient information to classify. Never silently promoted
    /// to `Html`.
    Unknown,
}

/// The declared `Content-Type` header value, verbatim, when present.
fn declared_content_type(page: &Page) -> Option<String> {
    page.headers
        .as_ref()
        .and_then(|headers| headers.get("content-type"))
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// Classify `content_type` into a [`DocumentRepresentation`]. Parameters
/// (`; charset=...`) are stripped before comparison; the base media type
/// is matched case-insensitively.
fn classify_representation(content_type: Option<&str>) -> DocumentRepresentation {
    let Some(raw) = content_type else {
        return DocumentRepresentation::Unknown;
    };
    let base = raw
        .split(';')
        .next()
        .unwrap_or(raw)
        .trim()
        .to_ascii_lowercase();
    match base.as_str() {
        "text/html" | "application/xhtml+xml" => DocumentRepresentation::Html,
        "" => DocumentRepresentation::Unknown,
        _ => DocumentRepresentation::NonHtml,
    }
}

/// True iff a page-content HTML DOM SEO rule may apply to `facts`: the
/// final response was a successful 2xx *and* the observed representation
/// is HTML/XHTML. Every HTML DOM SEO rule in this module reuses this one
/// predicate — never duplicated independently per rule. Passive
/// security/transport rules state their own, different applicability
/// explicitly (see [`security_https_missing`], [`security_hsts_missing`]).
fn page_content_seo_applicable(facts: &PageFacts) -> bool {
    is_success_2xx(facts.effective_status) && facts.representation == DocumentRepresentation::Html
}

fn is_success_2xx(status: u16) -> bool {
    (200..300).contains(&status)
}

/// One-pass, deterministic extraction of every authorized HTML DOM fact
/// this frontier's rules — and, since
/// `SCORPION_AUDIT_OBSERVED_TECHNOLOGY_MARKERS_001`,
/// [`extract_html_generator_technology_markers`] — need. Only ever
/// computed by [`PageFacts::from_page`] when the page's own
/// [`DocumentRepresentation`] is [`DocumentRepresentation::Html`] —
/// parsing non-HTML/unknown-representation bytes as HTML would not be
/// truthful.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HtmlPageFacts {
    canonical_links: Vec<String>,
    title_present: bool,
    meta_description_present: bool,
    h1_count: usize,
    html_lang_present: bool,
    image_count: usize,
    images_missing_alt: usize,
    /// Every `<meta name="generator" content="...">` observation's raw,
    /// unnormalized `content` attribute value, in document order. An
    /// element with the attribute present but empty contributes `""`;
    /// an element with the attribute entirely absent contributes nothing.
    /// This is a truthful capture, not a technology marker itself — see
    /// [`extract_html_generator_technology_markers`] for the
    /// deterministic normalization (trim, drop-if-empty) applied when
    /// turning this into an [`ObservedTechnologyMarker`].
    meta_generators: Vec<String>,
}

impl HtmlPageFacts {
    /// Every `<link rel="canonical" href="...">` observation, in
    /// document order. Search title/snippet/provider score, discovery
    /// metadata, an HTTP `Link` header, and an OpenGraph URL are never
    /// treated as an HTML canonical element.
    pub fn canonical_links(&self) -> &[String] {
        &self.canonical_links
    }

    /// `true` only when a real `<title>` observation contains
    /// non-whitespace text.
    pub fn title_present(&self) -> bool {
        self.title_present
    }

    /// `true` only when a real `<meta name="description" content="...">`
    /// observation's `content` contains non-whitespace text.
    pub fn meta_description_present(&self) -> bool {
        self.meta_description_present
    }

    /// The exact number of `<h1>` elements observed. Wording/content
    /// quality is never evaluated.
    pub fn h1_count(&self) -> usize {
        self.h1_count
    }

    /// `true` only when `<html lang="...">` carries a non-empty,
    /// non-whitespace value.
    pub fn html_lang_present(&self) -> bool {
        self.html_lang_present
    }

    /// The total number of `<img>` elements observed.
    pub fn image_count(&self) -> usize {
        self.image_count
    }

    /// The number of `<img>` elements whose `alt` attribute is entirely
    /// absent. `<img alt="">` (a valid, intentional decorative-image
    /// marker) never counts here — only a genuinely missing attribute
    /// does.
    pub fn images_missing_alt(&self) -> usize {
        self.images_missing_alt
    }

    /// Every `<meta name="generator">` observation's raw `content`
    /// attribute value, in document order — truthful capture, not yet a
    /// normalized technology marker. See this field's own doc comment.
    pub fn meta_generators(&self) -> &[String] {
        &self.meta_generators
    }
}

/// Truthfully extract every authorized HTML DOM fact from `html` in one
/// deterministic pass. Reuses this crate's existing `lol_html`
/// infrastructure (the same synchronous, side-effecting `element!`/
/// `text!` handler pattern `crate::utils::clean_html_base` and
/// `crate::page::metadata_handlers` already use) — no new HTML parser
/// dependency, and this module deliberately does not reuse
/// `crate::page::metadata_handlers`/`Page::metadata` itself: that
/// function is coupled to the Chrome/streaming link-extraction pipeline
/// (different lifetimes, different composition point), and empirical
/// testing (`fetch_single_page` against a real fixture server) showed
/// `Page::metadata` population depends on that pipeline's own link-
/// extraction configuration rather than being unconditionally guaranteed
/// on every acquisition this seam might run under — so this module owns
/// one small, fully self-contained, independently testable extraction
/// pass instead of taking on that hidden dependency.
fn extract_html_facts(html: &str) -> HtmlPageFacts {
    use lol_html::{element, rewrite_str, text, RewriteStrSettings};
    use std::cell::Cell;
    use std::cell::RefCell;

    let canonical_links: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let title_present = Cell::new(false);
    let meta_description_present = Cell::new(false);
    let h1_count = Cell::new(0_usize);
    let html_lang_present = Cell::new(false);
    let image_count = Cell::new(0_usize);
    let images_missing_alt = Cell::new(0_usize);
    let meta_generators: RefCell<Vec<String>> = RefCell::new(Vec::new());

    // catch_unwind guards against lol_html's internal panic on malformed
    // encodings, exactly like `clean_html_base` — a page whose HTML
    // cannot be safely rewritten yields every fact at its truthful
    // zero/absent default rather than propagating a panic into the audit
    // seam.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rewrite_str(
            html,
            RewriteStrSettings {
                element_content_handlers: vec![
                    element!("link[rel=\"canonical\"]", |el| {
                        if let Some(href) = el.get_attribute("href") {
                            canonical_links.borrow_mut().push(href);
                        }
                        Ok(())
                    }),
                    element!("html", |el| {
                        if let Some(lang) = el.get_attribute("lang") {
                            if !lang.trim().is_empty() {
                                html_lang_present.set(true);
                            }
                        }
                        Ok(())
                    }),
                    text!("title", |chunk| {
                        if !chunk.as_str().trim().is_empty() {
                            title_present.set(true);
                        }
                        Ok(())
                    }),
                    element!("meta[name=\"description\"]", |el| {
                        if let Some(content) = el.get_attribute("content") {
                            if !content.trim().is_empty() {
                                meta_description_present.set(true);
                            }
                        }
                        Ok(())
                    }),
                    // Directly observed, not inferred: the remote
                    // document itself declared this value in a
                    // `<meta name="generator">` element. Raw capture
                    // only — normalization/emptiness policy lives in
                    // `extract_html_generator_technology_markers`, not
                    // here (this pass stays a truthful, unopinionated
                    // fact extractor, matching every other handler
                    // above).
                    element!("meta[name=\"generator\"]", |el| {
                        if let Some(content) = el.get_attribute("content") {
                            meta_generators.borrow_mut().push(content);
                        }
                        Ok(())
                    }),
                    element!("h1", |_el| {
                        h1_count.set(h1_count.get() + 1);
                        Ok(())
                    }),
                    element!("img", |el| {
                        image_count.set(image_count.get() + 1);
                        if el.get_attribute("alt").is_none() {
                            images_missing_alt.set(images_missing_alt.get() + 1);
                        }
                        Ok(())
                    }),
                ],
                ..RewriteStrSettings::default()
            },
        )
    }));

    HtmlPageFacts {
        canonical_links: canonical_links.into_inner(),
        title_present: title_present.get(),
        meta_description_present: meta_description_present.get(),
        h1_count: h1_count.get(),
        html_lang_present: html_lang_present.get(),
        image_count: image_count.get(),
        images_missing_alt: images_missing_alt.get(),
        meta_generators: meta_generators.into_inner(),
    }
}

/// Transient, deterministic, network-free projection of one acquired
/// [`Page`]'s audit-relevant facts. Never persisted itself — see this
/// module's doc comment: `Evidence` is the acquired page, `PageFacts` is
/// the deterministic projection, `Finding` is the deterministic rule
/// output. The only constructor is [`PageFacts::from_page`]; there is no
/// way to build one from a `SearchResult` or any other discovery-only
/// type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageFacts {
    requested_url: String,
    final_url: String,
    effective_status: u16,
    observed_status: Option<u16>,
    content_type: Option<String>,
    representation: DocumentRepresentation,
    /// `None` = header observation itself was unavailable
    /// (`Page.headers` was `None`). `Some(map)` — possibly empty — =
    /// headers were observed; `map` names every allowlisted header
    /// actually present. See this module's doc comment.
    response_headers: Option<BTreeMap<String, Vec<Vec<u8>>>>,
    /// `Some` only when `representation == Html`.
    html: Option<HtmlPageFacts>,
}

impl PageFacts {
    /// Derive facts from `page`, and only `page` — no network, no
    /// persistence, no AI, no call into `Website`/`reqwest`/any search
    /// provider.
    pub fn from_page(page: &Page) -> Self {
        let provenance = page_provenance(page);
        let content_type = declared_content_type(page);
        let representation = classify_representation(content_type.as_deref());
        let response_headers = page.headers.as_ref().map(audit_response_headers);
        let html = (representation == DocumentRepresentation::Html)
            .then(|| extract_html_facts(&page.get_html()));
        Self {
            requested_url: page.get_url().to_string(),
            final_url: page.get_url_final().to_string(),
            effective_status: page.status_code.as_u16(),
            observed_status: provenance.observed_status_code,
            content_type,
            representation,
            response_headers,
            html,
        }
    }

    /// The URL that was actually requested.
    pub fn requested_url(&self) -> &str {
        &self.requested_url
    }

    /// The URL after following any redirects.
    pub fn final_url(&self) -> &str {
        &self.final_url
    }

    /// Spider's effective/crawler status after existing operational
    /// reclassification and retry policy.
    pub fn effective_status(&self) -> u16 {
        self.effective_status
    }

    /// HTTP status actually observed from a response or trusted relay,
    /// independent of `effective_status`. `None` when the acquisition
    /// path never stamped it.
    pub fn observed_status(&self) -> Option<u16> {
        self.observed_status
    }

    /// The declared `Content-Type` header value, verbatim, when observed.
    pub fn content_type(&self) -> Option<&str> {
        self.content_type.as_deref()
    }

    /// The truthful HTML/non-HTML/unknown classification — see
    /// [`DocumentRepresentation`].
    pub fn representation(&self) -> DocumentRepresentation {
        self.representation
    }

    /// Every `<link rel="canonical" href="...">` observation, in
    /// document order — `&[]` when [`Self::representation`] is not
    /// [`DocumentRepresentation::Html`]. Convenience accessor; see
    /// [`Self::html`] for the full HTML fact set.
    pub fn canonical_links(&self) -> &[String] {
        self.html
            .as_ref()
            .map(HtmlPageFacts::canonical_links)
            .unwrap_or(&[])
    }

    /// Every authorized HTML DOM fact — `None` when
    /// [`Self::representation`] is not [`DocumentRepresentation::Html`].
    pub fn html(&self) -> Option<&HtmlPageFacts> {
        self.html.as_ref()
    }

    /// Every observed value of each closed-allowlist audit-relevant
    /// response header — see
    /// [`audit_response_headers`](crate::utils::evidence::audit_response_headers).
    /// `None` means header observation itself was unavailable, never
    /// that headers were observed to be absent — see this module's doc
    /// comment.
    pub fn response_headers(&self) -> Option<&BTreeMap<String, Vec<Vec<u8>>>> {
        self.response_headers.as_ref()
    }
}

/// [`PageFacts`] bound to the exact [`EvidenceRef`] naming the durable
/// recording of the same acquired [`Page`] the facts were derived from.
///
/// There is exactly one way to obtain one: [`EvidencedPageFacts::record`],
/// which receives a `&Page` and, from that single value, both builds and
/// durably records its evidence *and* derives its facts — never two
/// independently supplied halves. There is no public constructor that
/// accepts a caller-supplied `PageFacts` and `EvidenceRef` pair (a prior
/// version of this type had exactly that — `EvidencedPageFacts::new(facts,
/// evidence_ref)` — which let a caller pair one page's facts with a
/// *different, unrelated but validly-resolving* page's evidence; that
/// constructor has been removed, not merely discouraged). This is what
/// makes it structurally impossible — not just conventionally
/// discouraged — to analyze Page A and link the resulting `Finding` to
/// Evidence B: association authority lives here, not in caller
/// discipline or in [`record_finding`]'s evidence-resolution check
/// (which only proves the referenced evidence *exists*, never that it
/// came from the same acquisition as the facts).
#[derive(Debug, Clone)]
pub struct EvidencedPageFacts {
    facts: PageFacts,
    evidence_ref: EvidenceRef,
}

impl EvidencedPageFacts {
    /// The sole production association seam: build and durably record
    /// `page`'s own evidence, derive [`PageFacts`] from that *exact same*
    /// `page` value, and pair them. No network activity — `page` must
    /// already be acquired (see [`fetch_single_page`]); this performs no
    /// second fetch, ever.
    pub async fn record(store: &DomainPersistence, page: &Page) -> Result<Self, AuditError> {
        let content = page.get_html();
        let bundle: EvidenceBundle = build_evidence(page, Some(content), false, false);
        let recorded = record_evidence(store, bundle)
            .await
            .map_err(AuditError::EvidenceRecording)?;
        let evidence_ref = EvidenceRef::new(
            recorded
                .id
                .expect("record_evidence always assigns an id on success"),
        );
        let facts = PageFacts::from_page(page);
        Ok(Self {
            facts,
            evidence_ref,
        })
    }

    /// The deterministic facts.
    pub fn facts(&self) -> &PageFacts {
        &self.facts
    }

    /// The evidence these facts are bound to.
    pub fn evidence_ref(&self) -> EvidenceRef {
        self.evidence_ref
    }
}

/// Which product area a [`Finding`] belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FindingCategory {
    /// Search-engine-optimization observations.
    Seo,
    /// Passive security/transport observations — deterministic policy
    /// checks, never vulnerability, exploitability, or CVE claims.
    Security,
}

impl FindingCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::Seo => "seo",
            Self::Security => "security",
        }
    }
}

/// Fixed rule policy for how significant a `Finding` is — never derived
/// from observed evidence, and never AI-generated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FindingSeverity {
    /// Informational only — no remediation implied.
    Info,
    /// Minor issue.
    Low,
    /// Moderate issue — `SEO_CANONICAL_MISSING`'s fixed policy severity.
    Medium,
    /// Significant issue.
    High,
    /// Severe issue.
    Critical,
}

impl FindingSeverity {
    fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

/// A URL scheme a [`FindingCondition::Scheme`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UrlScheme {
    /// `http`.
    Http,
    /// `https`.
    Https,
}

impl UrlScheme {
    fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }
}

/// A structurally typed observed or expected fact a rule's predicate
/// compares — never a free-form narrative string like `"SEO is bad"`.
/// `CanonicalLinkCount`/`CanonicalLinkCountAtLeast` are load-bearing for
/// historical Finding identity — see this module's rule-constant doc
/// comments — and must never change shape or `identity_repr()` wording.
/// `Present(bool)` is deliberately reused across every simple presence/
/// absence rule (title, meta description, `html[lang]`, and each
/// security-header rule) rather than minted once per rule: it is a
/// closed, typed representation on its own, and its meaning is always
/// unambiguous alongside the `Finding`'s own `rule_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FindingCondition {
    /// The exact number of `<link rel="canonical">` observations found.
    CanonicalLinkCount(usize),
    /// The minimum number of `<link rel="canonical">` observations
    /// required.
    CanonicalLinkCountAtLeast(usize),
    /// Whether the fact this condition describes was present.
    Present(bool),
    /// The exact number of `<h1>` elements observed.
    HeadingCount(usize),
    /// The minimum number of `<h1>` elements required.
    HeadingCountAtLeast(usize),
    /// The maximum number of `<h1>` elements allowed.
    HeadingCountAtMost(usize),
    /// The exact number of `<img>` elements missing an `alt` attribute.
    ImagesMissingAlt(usize),
    /// The URL scheme actually observed or expected.
    Scheme(UrlScheme),
}

impl FindingCondition {
    /// Stable, explicit wire representation used only for
    /// [`FindingId`] derivation — never `{:?}` (`Debug` formatting is
    /// not a serialization contract). The `CanonicalLinkCount`/
    /// `CanonicalLinkCountAtLeast` arms are byte-identical to this
    /// module's original single-rule frontier — changing their wording
    /// would silently change every historical Finding's identity.
    fn identity_repr(&self) -> String {
        match self {
            Self::CanonicalLinkCount(n) => format!("canonical_link_count={n}"),
            Self::CanonicalLinkCountAtLeast(n) => format!("canonical_link_count>={n}"),
            Self::Present(present) => format!("present={present}"),
            Self::HeadingCount(n) => format!("heading_count={n}"),
            Self::HeadingCountAtLeast(n) => format!("heading_count>={n}"),
            Self::HeadingCountAtMost(n) => format!("heading_count<={n}"),
            Self::ImagesMissingAlt(n) => format!("images_missing_alt={n}"),
            Self::Scheme(scheme) => format!("scheme={}", scheme.as_str()),
        }
    }
}

/// Content-addressed identity of one [`Finding`] — deterministic SHA-256
/// over every semantic field that makes two findings meaningfully
/// different. See this module's doc comment for why this mirrors
/// `ChangeEventId`'s own construction pattern.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FindingId(String);

impl FindingId {
    /// Wire-format prefix.
    pub const PREFIX: &'static str = "finding_";

    fn derive(
        rule_id: &str,
        rule_version: u32,
        category: FindingCategory,
        severity: FindingSeverity,
        target: &str,
        observed_condition: &FindingCondition,
        expected_condition: &FindingCondition,
        evidence: &[EvidenceRef],
    ) -> Self {
        let evidence_ids = evidence
            .iter()
            .map(|reference| reference.id().to_string())
            .collect::<Vec<_>>()
            .join(",");
        let joined = format!(
            "finding-v1|{rule_id}|{rule_version}|{}|{}|{target}|{}|{}|{evidence_ids}",
            category.as_str(),
            severity.as_str(),
            observed_condition.identity_repr(),
            expected_condition.identity_repr(),
        );
        Self(format!("{}{}", Self::PREFIX, sha256_hex(joined.as_bytes())))
    }

    /// Borrow the wire-format string (`finding_<sha256 hex>`).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FindingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One immutable, evidence-linked derived record. Never `Evidence`
/// itself, and never constructible with an empty or unresolved evidence
/// set — see [`Finding::new`] and [`record_finding`]. The only way to
/// obtain one is a rule function (e.g. [`seo_canonical_missing`]) or
/// [`read_finding`] reading one back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    rule_id: String,
    rule_version: u32,
    category: FindingCategory,
    severity: FindingSeverity,
    target: String,
    observed_condition: FindingCondition,
    expected_condition: FindingCondition,
    evidence: Vec<EvidenceRef>,
}

impl Finding {
    /// Construct a finding. `evidence` is sorted and deduplicated by
    /// identity before storage, so evidence-input ordering never affects
    /// [`Finding::id`]. Fails closed on an empty evidence set — an
    /// absence claim like `SEO_CANONICAL_MISSING` must always name the
    /// durable page material it was checked against.
    fn new(
        rule_id: &str,
        rule_version: u32,
        category: FindingCategory,
        severity: FindingSeverity,
        target: impl Into<String>,
        observed_condition: FindingCondition,
        expected_condition: FindingCondition,
        mut evidence: Vec<EvidenceRef>,
    ) -> Result<Self, AuditError> {
        if evidence.is_empty() {
            return Err(AuditError::EmptyEvidence);
        }
        evidence.sort_by_key(|reference| reference.id());
        evidence.dedup();
        Ok(Self {
            rule_id: rule_id.to_string(),
            rule_version,
            category,
            severity,
            target: target.into(),
            observed_condition,
            expected_condition,
            evidence,
        })
    }

    /// This finding's content-addressed identity.
    pub fn id(&self) -> FindingId {
        FindingId::derive(
            &self.rule_id,
            self.rule_version,
            self.category,
            self.severity,
            &self.target,
            &self.observed_condition,
            &self.expected_condition,
            &self.evidence,
        )
    }

    /// Stable semantic identity of the rule that produced this finding.
    pub fn rule_id(&self) -> &str {
        &self.rule_id
    }

    /// The rule predicate version that produced this finding.
    pub fn rule_version(&self) -> u32 {
        self.rule_version
    }

    /// Which product area this finding belongs to.
    pub fn category(&self) -> FindingCategory {
        self.category
    }

    /// Fixed rule policy — never observed evidence.
    pub fn severity(&self) -> FindingSeverity {
        self.severity
    }

    /// What this finding is about (the audited page's final URL).
    pub fn target(&self) -> &str {
        &self.target
    }

    /// The structurally typed fact actually observed.
    pub fn observed_condition(&self) -> &FindingCondition {
        &self.observed_condition
    }

    /// The structurally typed fact the rule expected.
    pub fn expected_condition(&self) -> &FindingCondition {
        &self.expected_condition
    }

    /// Every evidence record this finding is linked to (sorted,
    /// deduplicated).
    pub fn evidence(&self) -> &[EvidenceRef] {
        &self.evidence
    }
}

/// Why a finding could not be derived, resolved, or persisted.
#[derive(Debug)]
pub enum AuditError {
    /// [`fetch_single_page`] failed before producing a `Page`.
    Acquisition(String),
    /// Building/recording the acquired page's evidence failed.
    EvidenceRecording(EvidenceLedgerError),
    /// A `Finding` was constructed with zero `EvidenceRef`s — an
    /// absence claim must always name the evidence it was checked
    /// against.
    EmptyEvidence,
    /// A `Finding`'s `EvidenceRef` does not resolve in the canonical
    /// evidence ledger — recording fails closed rather than accepting
    /// an identity-shaped string as proof evidence exists.
    EvidenceUnresolvable(EvidenceRef),
    /// Resolving an `EvidenceRef` itself failed.
    Evidence(EvidenceLedgerError),
    /// A backend/persistence failure unrelated to the above.
    Persistence(PersistenceError),
    /// The finding could not be encoded/decoded.
    Serialization(serde_json::Error),
}

impl fmt::Display for AuditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Acquisition(message) => write!(f, "audit acquisition failed: {message}"),
            Self::EvidenceRecording(error) => write!(f, "audit evidence recording: {error}"),
            Self::EmptyEvidence => f.write_str("a finding must carry at least one EvidenceRef"),
            Self::EvidenceUnresolvable(reference) => {
                write!(f, "finding evidence {reference:?} does not resolve")
            }
            Self::Evidence(error) => write!(f, "audit evidence: {error}"),
            Self::Persistence(error) => write!(f, "finding ledger: {error}"),
            Self::Serialization(error) => {
                write!(f, "finding ledger: serialization failed: {error}")
            }
        }
    }
}

impl std::error::Error for AuditError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::EvidenceRecording(error) | Self::Evidence(error) => Some(error),
            Self::Persistence(error) => Some(error),
            Self::Serialization(error) => Some(error),
            Self::Acquisition(_) | Self::EmptyEvidence | Self::EvidenceUnresolvable(_) => None,
        }
    }
}

/// Build a single-evidence `Finding`. Every production rule in this
/// module links its `Finding` to exactly the one `EvidenceRef`
/// `evidenced` carries — never a second, independently-obtained
/// reference.
fn single_evidence_finding(
    rule_id: &str,
    rule_version: u32,
    category: FindingCategory,
    severity: FindingSeverity,
    target: String,
    observed: FindingCondition,
    expected: FindingCondition,
    evidenced: &EvidencedPageFacts,
) -> Finding {
    Finding::new(
        rule_id,
        rule_version,
        category,
        severity,
        target,
        observed,
        expected,
        vec![evidenced.evidence_ref],
    )
    .expect("EvidencedPageFacts always carries exactly one EvidenceRef")
}

/// `Some(Finding)` when `evidenced`'s page is a successful HTML document
/// (see [`page_content_seo_applicable`]) carrying zero
/// `<link rel="canonical">` observations, `None` otherwise. No severity
/// gradient, no conflict/loop detection, no normalization policy — those
/// belong to successor rules. Version `2`: see
/// [`SEO_CANONICAL_MISSING_RULE_VERSION`]'s own doc comment for why this
/// changed from the original, applicability-free version `1`.
pub fn seo_canonical_missing(evidenced: &EvidencedPageFacts) -> Option<Finding> {
    let facts = evidenced.facts();
    if !page_content_seo_applicable(facts) {
        return None;
    }
    if !facts.html()?.canonical_links().is_empty() {
        return None;
    }
    Some(single_evidence_finding(
        SEO_CANONICAL_MISSING_RULE_ID,
        SEO_CANONICAL_MISSING_RULE_VERSION,
        FindingCategory::Seo,
        FindingSeverity::Medium,
        facts.final_url().to_string(),
        FindingCondition::CanonicalLinkCount(0),
        FindingCondition::CanonicalLinkCountAtLeast(1),
        evidenced,
    ))
}

/// `Some(Finding)` when `evidenced`'s page is a successful HTML document
/// with no non-whitespace `<title>` text, `None` otherwise.
pub fn seo_title_missing(evidenced: &EvidencedPageFacts) -> Option<Finding> {
    let facts = evidenced.facts();
    if !page_content_seo_applicable(facts) {
        return None;
    }
    if facts.html()?.title_present() {
        return None;
    }
    Some(single_evidence_finding(
        SEO_TITLE_MISSING_RULE_ID,
        SEO_TITLE_MISSING_RULE_VERSION,
        FindingCategory::Seo,
        FindingSeverity::Medium,
        facts.final_url().to_string(),
        FindingCondition::Present(false),
        FindingCondition::Present(true),
        evidenced,
    ))
}

/// `Some(Finding)` when `evidenced`'s page is a successful HTML document
/// with no non-whitespace `<meta name="description">` content, `None`
/// otherwise.
pub fn seo_meta_description_missing(evidenced: &EvidencedPageFacts) -> Option<Finding> {
    let facts = evidenced.facts();
    if !page_content_seo_applicable(facts) {
        return None;
    }
    if facts.html()?.meta_description_present() {
        return None;
    }
    Some(single_evidence_finding(
        SEO_META_DESCRIPTION_MISSING_RULE_ID,
        SEO_META_DESCRIPTION_MISSING_RULE_VERSION,
        FindingCategory::Seo,
        FindingSeverity::Low,
        facts.final_url().to_string(),
        FindingCondition::Present(false),
        FindingCondition::Present(true),
        evidenced,
    ))
}

/// `Some(Finding)` when `evidenced`'s page is a successful HTML document
/// with zero `<h1>` elements, `None` otherwise.
pub fn seo_h1_missing(evidenced: &EvidencedPageFacts) -> Option<Finding> {
    let facts = evidenced.facts();
    if !page_content_seo_applicable(facts) {
        return None;
    }
    if facts.html()?.h1_count() != 0 {
        return None;
    }
    Some(single_evidence_finding(
        SEO_H1_MISSING_RULE_ID,
        SEO_H1_MISSING_RULE_VERSION,
        FindingCategory::Seo,
        FindingSeverity::Low,
        facts.final_url().to_string(),
        FindingCondition::HeadingCount(0),
        FindingCondition::HeadingCountAtLeast(1),
        evidenced,
    ))
}

/// `Some(Finding)` when `evidenced`'s page is a successful HTML document
/// with more than one `<h1>` element, `None` otherwise. This is rule
/// policy, not a claimed security defect or indexing failure.
pub fn seo_h1_multiple(evidenced: &EvidencedPageFacts) -> Option<Finding> {
    let facts = evidenced.facts();
    if !page_content_seo_applicable(facts) {
        return None;
    }
    let h1_count = facts.html()?.h1_count();
    if h1_count <= 1 {
        return None;
    }
    Some(single_evidence_finding(
        SEO_H1_MULTIPLE_RULE_ID,
        SEO_H1_MULTIPLE_RULE_VERSION,
        FindingCategory::Seo,
        FindingSeverity::Low,
        facts.final_url().to_string(),
        FindingCondition::HeadingCount(h1_count),
        FindingCondition::HeadingCountAtMost(1),
        evidenced,
    ))
}

/// `Some(Finding)` when `evidenced`'s page is a successful HTML document
/// with no non-empty `<html lang="...">` value, `None` otherwise.
pub fn seo_html_lang_missing(evidenced: &EvidencedPageFacts) -> Option<Finding> {
    let facts = evidenced.facts();
    if !page_content_seo_applicable(facts) {
        return None;
    }
    if facts.html()?.html_lang_present() {
        return None;
    }
    Some(single_evidence_finding(
        SEO_HTML_LANG_MISSING_RULE_ID,
        SEO_HTML_LANG_MISSING_RULE_VERSION,
        FindingCategory::Seo,
        FindingSeverity::Low,
        facts.final_url().to_string(),
        FindingCondition::Present(false),
        FindingCondition::Present(true),
        evidenced,
    ))
}

/// `Some(Finding)` when `evidenced`'s page is a successful HTML document
/// with one or more `<img>` elements entirely missing their `alt`
/// attribute, `None` otherwise. `<img alt="">` (intentionally
/// decorative) never counts. Produces exactly one page-level Finding
/// carrying the observed count — never one Finding per image.
pub fn seo_image_alt_missing(evidenced: &EvidencedPageFacts) -> Option<Finding> {
    let facts = evidenced.facts();
    if !page_content_seo_applicable(facts) {
        return None;
    }
    let missing = facts.html()?.images_missing_alt();
    if missing == 0 {
        return None;
    }
    Some(single_evidence_finding(
        SEO_IMAGE_ALT_MISSING_RULE_ID,
        SEO_IMAGE_ALT_MISSING_RULE_VERSION,
        FindingCategory::Seo,
        FindingSeverity::Low,
        facts.final_url().to_string(),
        FindingCondition::ImagesMissingAlt(missing),
        FindingCondition::ImagesMissingAlt(0),
        evidenced,
    ))
}

/// The scheme `url` actually uses, when it parses as `http`/`https`;
/// `None` for anything else (unparseable, or a non-HTTP(S) scheme) — no
/// guessing.
fn url_scheme(url: &str) -> Option<UrlScheme> {
    let parsed = url::Url::parse(url).ok()?;
    match parsed.scheme() {
        "http" => Some(UrlScheme::Http),
        "https" => Some(UrlScheme::Https),
        _ => None,
    }
}

/// `Some(Finding)` when `evidenced`'s final URL scheme is `http`, `None`
/// otherwise. Applicability differs from the HTML DOM SEO rules above:
/// this applies to any successfully acquired page with an observed final
/// URL (the seam already guarantees a `Page` was acquired to reach the
/// analyzer at all) — status code and representation are irrelevant. No
/// active probe, no second request: purely a read of the already-
/// acquired final URL.
pub fn security_https_missing(evidenced: &EvidencedPageFacts) -> Option<Finding> {
    let facts = evidenced.facts();
    let scheme = url_scheme(facts.final_url())?;
    if scheme == UrlScheme::Https {
        return None;
    }
    Some(single_evidence_finding(
        SECURITY_HTTPS_MISSING_RULE_ID,
        SECURITY_HTTPS_MISSING_RULE_VERSION,
        FindingCategory::Security,
        FindingSeverity::High,
        facts.final_url().to_string(),
        FindingCondition::Scheme(UrlScheme::Http),
        FindingCondition::Scheme(UrlScheme::Https),
        evidenced,
    ))
}

/// `Some(Finding)` when `evidenced`'s final URL is `https`, response
/// headers were actually observed, and `Strict-Transport-Security` is
/// absent from that observed set. When header observation itself is
/// unknown (`response_headers()` is `None`), no Finding is produced —
/// absence is never inferred from an observation surface that cannot
/// prove it.
pub fn security_hsts_missing(evidenced: &EvidencedPageFacts) -> Option<Finding> {
    let facts = evidenced.facts();
    if url_scheme(facts.final_url()) != Some(UrlScheme::Https) {
        return None;
    }
    let headers = facts.response_headers()?;
    if headers.contains_key("strict-transport-security") {
        return None;
    }
    Some(single_evidence_finding(
        SECURITY_HSTS_MISSING_RULE_ID,
        SECURITY_HSTS_MISSING_RULE_VERSION,
        FindingCategory::Security,
        FindingSeverity::Medium,
        facts.final_url().to_string(),
        FindingCondition::Present(false),
        FindingCondition::Present(true),
        evidenced,
    ))
}

/// `Some(Finding)` when `evidenced`'s page is a successful HTML document,
/// response headers were actually observed, and `Content-Security-Policy`
/// is absent. `Content-Security-Policy-Report-Only` does **not** satisfy
/// enforcement CSP presence for this rule. CSP directive quality is not
/// evaluated — a separate future rule.
pub fn security_csp_missing(evidenced: &EvidencedPageFacts) -> Option<Finding> {
    let facts = evidenced.facts();
    if !page_content_seo_applicable(facts) {
        return None;
    }
    let headers = facts.response_headers()?;
    if headers.contains_key("content-security-policy") {
        return None;
    }
    Some(single_evidence_finding(
        SECURITY_CSP_MISSING_RULE_ID,
        SECURITY_CSP_MISSING_RULE_VERSION,
        FindingCategory::Security,
        FindingSeverity::Medium,
        facts.final_url().to_string(),
        FindingCondition::Present(false),
        FindingCondition::Present(true),
        evidenced,
    ))
}

/// `Some(Finding)` when `evidenced`'s page is a successful HTML document,
/// response headers were actually observed, and
/// `X-Content-Type-Options` is absent. Its value is not yet evaluated
/// (whether it equals `nosniff`) — a separate future rule.
pub fn security_x_content_type_options_missing(evidenced: &EvidencedPageFacts) -> Option<Finding> {
    let facts = evidenced.facts();
    if !page_content_seo_applicable(facts) {
        return None;
    }
    let headers = facts.response_headers()?;
    if headers.contains_key("x-content-type-options") {
        return None;
    }
    Some(single_evidence_finding(
        SECURITY_X_CONTENT_TYPE_OPTIONS_MISSING_RULE_ID,
        SECURITY_X_CONTENT_TYPE_OPTIONS_MISSING_RULE_VERSION,
        FindingCategory::Security,
        FindingSeverity::Low,
        facts.final_url().to_string(),
        FindingCondition::Present(false),
        FindingCondition::Present(true),
        evidenced,
    ))
}

/// Every authorized production page rule, in stable, explicit
/// declaration order — never `HashMap`/iteration-order-dependent. The
/// sole rule registry: [`analyze_page`] and this module's own tests both
/// enumerate exactly this list, never a separately maintained one.
pub const PAGE_RULES: &[fn(&EvidencedPageFacts) -> Option<Finding>] = &[
    seo_canonical_missing,
    seo_title_missing,
    seo_meta_description_missing,
    seo_h1_missing,
    seo_h1_multiple,
    seo_html_lang_missing,
    seo_image_alt_missing,
    security_https_missing,
    security_hsts_missing,
    security_csp_missing,
    security_x_content_type_options_missing,
];

/// Run every authorized production page rule over `evidenced`, purely
/// and network-free, in [`PAGE_RULES`]'s deterministic order. No rule
/// may fetch, record evidence, create another `Page`, call `Website`/
/// `reqwest`/any search provider, or mutate persistence — every rule
/// here is a pure function of already-derived facts.
pub fn analyze_page(evidenced: &EvidencedPageFacts) -> Vec<Finding> {
    PAGE_RULES
        .iter()
        .filter_map(|rule| rule(evidenced))
        .collect()
}

// ---------------------------------------------------------------------
// Observed technology markers — SCORPION_AUDIT_OBSERVED_TECHNOLOGY_MARKERS_001
// ---------------------------------------------------------------------
//
// A technology marker is not a `Finding`: a `Finding` states that a
// deterministic rule evaluated observed facts against an expected
// condition; a marker states only that the remote page *itself*
// explicitly exposed a technology-identifying value in the same
// acquisition an audit already evidenced. There is no "expected"
// counterpart to a `Server: nginx` observation, so it is never forced
// into `FindingCondition`/`Finding`'s predicate shape — see this
// module's "Technology boundary" doc section above.
//
// Deliberately *not* introduced here: a `TechnologyMarkerId` or any
// other content-addressed identity, and no durable persistence path
// (no `append_history`/`write_current` call for markers). Both
// `PageFacts` and `HtmlPageFacts` are themselves "[n]ever persisted
// itself" (see their own doc comments) — they are pure, deterministic,
// re-derivable projections of the one thing that *is* durably recorded:
// the page's `Evidence` (which already stores `response_headers` and
// raw HTML `content`). `extract_technology_markers` is exactly one more
// such pure projection: a future consumer (MCP/Web Console, out of
// scope here — see the shipping-surface firewall below) can reproduce
// today's exact marker sequence at any time by reading the same
// `Evidence` back and calling this same deterministic function again —
// inventing marker identity/versioning now, before any real consumer
// exists, would be exactly the kind of premature, unauthorized-here
// shipping-surface design this frontier's own scope firewall forbids.

/// Response header names this frontier treats as technology-identifying
/// when the remote server sets them — a fixed, closed, deliberately
/// small subset of [`AUDIT_RESPONSE_HEADER_ALLOWLIST`] (never a
/// signature/fingerprint database, and never grown to "detect" a
/// specific product): each name here is a header a system may use to
/// *voluntarily self-declare* what serves it, not a name any other
/// system probes for. Order here is also
/// [`extract_response_header_technology_markers`]'s emission order —
/// stable and explicit, never `HashMap`-order-dependent.
pub const MARKER_HEADER_NAMES: &[&str] = &["server", "x-powered-by", "x-generator"];

/// Where an [`ObservedTechnologyMarker`]'s value was directly observed —
/// never an inferred, guessed, or probabilistic source. See this
/// module's non-inference boundary
/// (`SCORPION_AUDIT_OBSERVED_TECHNOLOGY_MARKERS_001`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TechnologyMarkerSource {
    /// One response header's own literal value, named by its lowercase
    /// [`AUDIT_RESPONSE_HEADER_ALLOWLIST`] spelling (always one of
    /// [`MARKER_HEADER_NAMES`] in production output).
    ResponseHeader(String),
    /// `<meta name="generator" content="...">`'s own literal `content`
    /// value.
    HtmlMetaGenerator,
}

/// A technology-identifying value the remote page *explicitly* exposed
/// in the same acquisition an audit already evidenced — the observation
/// itself, never an interpretation of it. Scorpion records that a
/// `Server` header's value was `"nginx"`, or that a
/// `<meta name="generator">` value was `"WordPress 6.4"`; Scorpion never
/// records a conclusion like `"technology = nginx"` or
/// `"cms = WordPress"`. Interpreting an observed marker into a named
/// technology/product/vendor claim belongs to a future consumer (an AI
/// via MCP, or a human reviewing canonical evidence through Web
/// Console) — never to this module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedTechnologyMarker {
    source: TechnologyMarkerSource,
    value: String,
}

impl ObservedTechnologyMarker {
    /// Where this value was observed.
    pub fn source(&self) -> &TechnologyMarkerSource {
        &self.source
    }

    /// The literal, deterministically-normalized (trimmed, never
    /// case-folded, never parsed/split) value the remote page exposed.
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// Deterministic technology markers from `facts`'s observed response
/// headers. Requires `facts.response_headers()` to be `Some(_)` —
/// header observation itself being unavailable ([`None`]) yields zero
/// markers, exactly like every header-absence security rule in this
/// module: absence of *observation* is never treated as absence of
/// *value*. Unlike [`page_content_seo_applicable`]-gated SEO rules, this
/// is **not** gated on 2xx status or HTML representation — a header a
/// server sets is a header a server sets, regardless of what status
/// code or body it served alongside it (an HTML 404 error page that
/// still answers `Server: nginx` really did expose that value).
///
/// For each of [`MARKER_HEADER_NAMES`], in that fixed order, every raw
/// byte value actually observed for that header name is considered in
/// observed order. A value is skipped (never a fabricated/lossy
/// substitute) when it is not valid UTF-8 — HTTP header values are not
/// guaranteed to be text, and Scorpion fails closed for that one value
/// rather than synthesizing a guess from raw bytes — or when, after
/// trimming surrounding whitespace, it is empty (an empty value
/// identifies no technology). Multiple distinct values for one header
/// name, and exact repeated identical values, are **all** retained as
/// separate markers — mirroring [`audit_response_headers`]'s own
/// never-collapse, never-comma-join semantics; no value is silently
/// deduplicated away.
pub fn extract_response_header_technology_markers(
    facts: &PageFacts,
) -> Vec<ObservedTechnologyMarker> {
    let Some(headers) = facts.response_headers() else {
        return Vec::new();
    };
    let mut markers = Vec::new();
    for &name in MARKER_HEADER_NAMES {
        let Some(values) = headers.get(name) else {
            continue;
        };
        for raw in values {
            let Ok(text) = std::str::from_utf8(raw) else {
                continue;
            };
            let trimmed = text.trim();
            if trimmed.is_empty() {
                continue;
            }
            markers.push(ObservedTechnologyMarker {
                source: TechnologyMarkerSource::ResponseHeader(name.to_string()),
                value: trimmed.to_string(),
            });
        }
    }
    markers
}

/// Deterministic technology markers from `facts`'s observed
/// `<meta name="generator">` elements. Requires
/// `facts.representation() == `[`DocumentRepresentation::Html`] — a
/// non-HTML/unknown-representation body was never parsed as HTML in the
/// first place (see [`PageFacts::from_page`]), so there is nothing to
/// read. **Not** additionally gated on 2xx status, deliberately unlike
/// [`page_content_seo_applicable`]: technology markers are observations,
/// not SEO findings, and an HTML error page can truthfully expose a
/// generator tag exactly as any other HTML page can (see this module's
/// own applicability tests).
///
/// Reuses [`HtmlPageFacts::meta_generators`] — the exact same single
/// [`extract_html_facts`] parse pass every other HTML fact in this
/// module already shares; this function performs no second HTML parse.
/// Each raw captured value is trimmed and, if empty after trimming,
/// skipped (an empty `content` attribute identifies no technology) —
/// otherwise kept verbatim (never case-folded, never parsed apart into
/// a name/version split). Multiple `<meta name="generator">` elements,
/// including exact repeated identical values, all produce separate
/// markers in document order — never deduplicated.
pub fn extract_html_generator_technology_markers(
    facts: &PageFacts,
) -> Vec<ObservedTechnologyMarker> {
    let Some(html) = facts.html() else {
        return Vec::new();
    };
    html.meta_generators()
        .iter()
        .filter_map(|raw| {
            let trimmed = raw.trim();
            (!trimmed.is_empty()).then(|| ObservedTechnologyMarker {
                source: TechnologyMarkerSource::HtmlMetaGenerator,
                value: trimmed.to_string(),
            })
        })
        .collect()
}

/// Every deterministic technology marker `evidenced`'s page directly
/// exposed — response-header markers first (in [`MARKER_HEADER_NAMES`]
/// order), then HTML `<meta name="generator">` markers (in document
/// order); both orderings are fixed and explicit, never
/// iteration-order-dependent. A pure function of already-derived facts:
/// no network activity, no second acquisition
/// ([`fetch_single_page`] is never called here), no second evidence
/// recording, no persistence, no AI. Every marker this returns
/// describes the exact same acquisition as `evidenced`'s own
/// [`EvidencedPageFacts::evidence_ref`] — this function does not carry
/// that reference itself only because it has no independent identity to
/// attach it to; [`audit_page`]'s caller-facing [`PageAuditResult`]
/// pairs every marker it returns with that one shared `EvidenceRef`.
pub fn extract_technology_markers(evidenced: &EvidencedPageFacts) -> Vec<ObservedTechnologyMarker> {
    let facts = evidenced.facts();
    let mut markers = extract_response_header_technology_markers(facts);
    markers.extend(extract_html_generator_technology_markers(facts));
    markers
}

/// Durably record `finding`, first verifying every `EvidenceRef` it
/// carries actually resolves in `store` — fail-closed: no finding is
/// persisted if any evidence reference is unresolvable. Recording an
/// identical (by content-addressed identity) finding twice is
/// idempotent, not a conflict — the same [`ChangeEvent`](crate::features::change_detection::ChangeEvent)
/// precedent, reused verbatim.
pub async fn record_finding(
    store: &DomainPersistence,
    finding: Finding,
) -> Result<Finding, AuditError> {
    for reference in &finding.evidence {
        let resolved = reference
            .resolve(store)
            .await
            .map_err(AuditError::Evidence)?;
        if resolved.is_none() {
            return Err(AuditError::EvidenceUnresolvable(*reference));
        }
    }

    let id = finding.id();
    let payload = serde_json::to_vec(&finding).map_err(AuditError::Serialization)?;

    match store
        .append_history(id.as_str(), 1, &payload, SystemTime::now())
        .await
    {
        Ok(()) => Ok(finding),
        Err(PersistenceError::HistoryAlreadyExists) => Ok(finding),
        Err(other) => Err(AuditError::Persistence(other)),
    }
}

/// Read back the finding named by `id`, exactly as [`record_finding`]
/// wrote it — no reconstruction, no re-derivation. `Ok(None)` when
/// nothing has ever been recorded for this identity. Fails closed
/// (`AuditError::Serialization`) on a corrupted/invalid stored payload.
pub async fn read_finding(
    store: &DomainPersistence,
    id: &FindingId,
) -> Result<Option<Finding>, AuditError> {
    let history = store
        .read_history(id.as_str())
        .await
        .map_err(AuditError::Persistence)?;

    match history.into_iter().next() {
        Some((_revision, payload, _recorded_at)) => {
            let finding = serde_json::from_slice(&payload).map_err(AuditError::Serialization)?;
            Ok(Some(finding))
        }
        None => Ok(None),
    }
}

/// The smallest internal result proving one page audit's architecture:
/// the exact [`EvidenceRef`] every returned [`Finding`] *and* every
/// returned [`ObservedTechnologyMarker`] is linked to, the deterministic
/// Finding list, and the deterministic technology-marker list. Not a
/// shipping DTO — no API, UI, MCP, or CLI schema is authorized in this
/// frontier.
#[derive(Debug, Clone)]
pub struct PageAuditResult {
    evidence_ref: EvidenceRef,
    findings: Vec<Finding>,
    technology_markers: Vec<ObservedTechnologyMarker>,
}

impl PageAuditResult {
    /// The evidence every `Finding` in [`Self::findings`] and every
    /// marker in [`Self::technology_markers`] is linked to — always the
    /// exact evidence recorded for the one `Page` this audit acquired.
    pub fn evidence_ref(&self) -> EvidenceRef {
        self.evidence_ref
    }

    /// Every persisted `Finding` produced by this audit, in
    /// [`PAGE_RULES`]'s deterministic order.
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    /// Every directly observed technology marker produced by this
    /// audit, in [`extract_technology_markers`]'s deterministic order.
    /// Never persisted independently — see the "Observed technology
    /// markers" section of this module's doc comment for why.
    pub fn technology_markers(&self) -> &[ObservedTechnologyMarker] {
        &self.technology_markers
    }
}

/// The internal, generic canonical audit execution seam — the *only*
/// production acquisition entrypoint in this module, reused by every
/// current and future page rule and by technology-marker extraction
/// (never one acquisition entrypoint per rule, and never a second
/// acquisition entrypoint for markers):
/// acquire exactly one page ([`fetch_single_page`], the same one-shot
/// primitive every other evidence-first caller uses) -> record its
/// evidence and derive [`PageFacts`] from that *exact same* `Page`
/// ([`EvidencedPageFacts::record`]) -> run every rule in [`PAGE_RULES`]
/// ([`analyze_page`]) -> persist each resulting [`Finding`]
/// ([`record_finding`]) -> derive every technology marker from that
/// *same* [`EvidencedPageFacts`] value ([`extract_technology_markers`]).
/// Exactly one acquisition, exactly one evidence recording, per audited
/// page — every returned `Finding` and every returned
/// `ObservedTechnologyMarker` therefore shares
/// [`PageAuditResult::evidence_ref`]. This is not a CLI/API/MCP/Web
/// Console surface — see this module's doc comment.
pub async fn audit_page(
    store: &DomainPersistence,
    url: &str,
) -> Result<PageAuditResult, AuditError> {
    let page = fetch_single_page(url)
        .await
        .map_err(AuditError::Acquisition)?;

    let evidenced = EvidencedPageFacts::record(store, &page).await?;
    let evidence_ref = evidenced.evidence_ref();

    let mut findings = Vec::new();
    for finding in analyze_page(&evidenced) {
        findings.push(record_finding(store, finding).await?);
    }

    let technology_markers = extract_technology_markers(&evidenced);

    Ok(PageAuditResult {
        evidence_ref,
        findings,
        technology_markers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::build;
    use crate::utils::PageResponse;

    /// A page with an explicit `Content-Type`, status, and (optionally)
    /// extra response headers — the general-purpose fixture builder every
    /// applicability/header test below uses.
    fn page_with(
        url: &str,
        body: &str,
        content_type: &str,
        status: reqwest::StatusCode,
        extra_headers: &[(&'static str, &str)],
    ) -> Page {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_str(content_type).unwrap(),
        );
        for (name, value) in extra_headers {
            headers.insert(
                reqwest::header::HeaderName::from_static(name),
                reqwest::header::HeaderValue::from_str(value).unwrap(),
            );
        }
        build(
            url,
            PageResponse {
                content: Some(body.as_bytes().to_vec()),
                status_code: status,
                headers: Some(headers),
                ..Default::default()
            },
        )
    }

    /// A page whose response never captured any headers at all —
    /// `PageFacts::response_headers()` must be `None`, not `Some(empty)`.
    fn page_with_no_headers(url: &str, html: &str) -> Page {
        build(
            url,
            PageResponse {
                content: Some(html.as_bytes().to_vec()),
                status_code: reqwest::StatusCode::OK,
                headers: None,
                ..Default::default()
            },
        )
    }

    /// A successful (`200 text/html`) page — the default shape almost
    /// every rule/extraction test below builds on.
    fn page_with_html(url: &str, html: &str) -> Page {
        page_with(url, html, "text/html", reqwest::StatusCode::OK, &[])
    }

    async fn record(store: &DomainPersistence, page: &Page) -> EvidenceRef {
        let bundle = build_evidence(page, Some(page.get_html()), false, false);
        let recorded = record_evidence(store, bundle).await.unwrap();
        EvidenceRef::new(recorded.id.unwrap())
    }

    /// `SCORPION_AUDIT_EXACT_PAGE_EVIDENCE_BINDING_CORRECTION_001`: prior
    /// to this correction, `EvidencedPageFacts::new(facts, evidence_ref)`
    /// was a public constructor accepting independently-supplied halves.
    /// Adversarial reproduction confirmed empirically (before the fix)
    /// that pairing Page A's facts with genuinely-resolvable Page B's
    /// evidence produced a `Finding` claiming `canonical_link_count=0`
    /// durably linked to evidence that actually contained a canonical
    /// link — `record_finding` only proves evidence *exists*, never that
    /// it came from the same acquisition as the facts. That constructor
    /// no longer exists; this module now provides only
    /// [`EvidencedPageFacts::record`], which cannot be given two
    /// independent halves at all. The tests below prove the replacement
    /// seam is both safe (same-page binding, Phase 9/10) and that safety
    /// does not depend on the mismatched evidence being unresolvable
    /// (Phase 11 — both pages' evidence coexist in the same store).
    mod exact_page_evidence_binding {
        use super::*;

        fn page_a() -> Page {
            page_with_html(
                "https://a.example/",
                "<html><head><title>A</title></head><body>A</body></html>",
            )
        }

        fn page_b() -> Page {
            page_with_html(
                "https://b.example/",
                r#"<html><head><link rel="canonical" href="https://b.example/"><title>B</title></head><body>B</body></html>"#,
            )
        }

        // Phase 9: positive same-page binding proof for Page A.
        #[tokio::test]
        async fn page_a_finding_references_page_a_evidence_and_only_page_a() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            // Phase 11 setup: Page B's evidence coexists in the same
            // store, fully valid and resolvable, before Page A is ever
            // touched.
            let _evidence_b = record(&store, &page_b()).await;

            let page_a = page_a();
            let evidenced_a = EvidencedPageFacts::record(&store, &page_a).await.unwrap();
            assert_eq!(evidenced_a.facts().requested_url(), page_a.get_url());

            let finding =
                seo_canonical_missing(&evidenced_a).expect("Page A has no canonical link");
            let persisted = record_finding(&store, finding).await.unwrap();

            let resolved = persisted.evidence()[0]
                .resolve(&store)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(resolved.requested_url.as_deref(), Some(page_a.get_url()));
            let content = resolved.content.unwrap();
            assert!(
                extract_html_facts(&content).canonical_links().is_empty(),
                "resolved evidence must genuinely be Page A's own content"
            );
        }

        // Phase 10: positive proof for Page B — zero findings.
        #[tokio::test]
        async fn page_b_produces_no_finding() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let page_b = page_b();
            let evidenced_b = EvidencedPageFacts::record(&store, &page_b).await.unwrap();
            assert!(seo_canonical_missing(&evidenced_b).is_none());
        }

        // Phase 11: the cross-page adversarial case. Both pages' evidence
        // are valid and resolvable in the same store — resolvability
        // alone must not be what prevents a mismatched pairing. The
        // supported production API (`EvidencedPageFacts::record`) simply
        // has no way to accept Page B's evidence while deriving facts
        // from Page A: it takes exactly one `&Page` and produces both
        // halves from it. Calling it twice, on two different pages,
        // necessarily produces two independently and correctly bound
        // values.
        #[tokio::test]
        async fn evidence_from_two_pages_in_the_same_store_never_cross_binds() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let page_a = page_a();
            let page_b = page_b();

            let evidenced_a = EvidencedPageFacts::record(&store, &page_a).await.unwrap();
            let evidenced_b = EvidencedPageFacts::record(&store, &page_b).await.unwrap();

            assert_ne!(evidenced_a.evidence_ref(), evidenced_b.evidence_ref());

            let bundle_a = evidenced_a
                .evidence_ref()
                .resolve(&store)
                .await
                .unwrap()
                .unwrap();
            let bundle_b = evidenced_b
                .evidence_ref()
                .resolve(&store)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(bundle_a.requested_url.as_deref(), Some(page_a.get_url()));
            assert_eq!(bundle_b.requested_url.as_deref(), Some(page_b.get_url()));

            // seo_canonical_missing on each evidenced value still agrees
            // with that exact page's own content, never the other's.
            assert!(seo_canonical_missing(&evidenced_a).is_some());
            assert!(seo_canonical_missing(&evidenced_b).is_none());
        }
    }

    mod html_fact_extraction {
        use super::*;

        #[test]
        fn extracts_a_present_canonical_link() {
            let html = r#"<html><head><link rel="canonical" href="https://example.test/product"></head></html>"#;
            assert_eq!(
                extract_html_facts(html).canonical_links(),
                &["https://example.test/product".to_string()]
            );
        }

        #[test]
        fn no_canonical_link_yields_empty() {
            let html = "<html><head><title>Example</title></head><body>hello</body></html>";
            assert!(extract_html_facts(html).canonical_links().is_empty());
        }

        #[test]
        fn discovery_shaped_fields_are_never_mistaken_for_a_canonical_element() {
            // A page whose HTML carries only OpenGraph/meta-shaped markup
            // (never a real <link rel="canonical">) must yield zero
            // observations, never a substitution.
            let html = r#"<html><head>
                <meta property="og:url" content="https://example.test/og">
                <meta name="title" content="Example">
                <link rel="alternate" href="https://example.test/rss">
            </head></html>"#;
            assert!(extract_html_facts(html).canonical_links().is_empty());
        }

        #[test]
        fn title_present_only_with_non_whitespace_text() {
            assert!(
                extract_html_facts("<html><head><title>Example</title></head></html>")
                    .title_present()
            );
            assert!(
                !extract_html_facts("<html><head><title>   </title></head></html>").title_present()
            );
            assert!(!extract_html_facts("<html><head></head></html>").title_present());
        }

        #[test]
        fn meta_description_present_only_with_non_whitespace_content() {
            let present = r#"<html><head><meta name="description" content="A real description"></head></html>"#;
            assert!(extract_html_facts(present).meta_description_present());
            let empty = r#"<html><head><meta name="description" content="   "></head></html>"#;
            assert!(!extract_html_facts(empty).meta_description_present());
            let absent = "<html><head></head></html>";
            assert!(!extract_html_facts(absent).meta_description_present());
        }

        #[test]
        fn h1_count_is_exact() {
            assert_eq!(
                extract_html_facts("<html><body></body></html>").h1_count(),
                0
            );
            assert_eq!(
                extract_html_facts("<html><body><h1>One</h1></body></html>").h1_count(),
                1
            );
            assert_eq!(
                extract_html_facts("<html><body><h1>A</h1><h1>B</h1></body></html>").h1_count(),
                2
            );
        }

        #[test]
        fn html_lang_present_only_with_non_empty_value() {
            assert!(
                extract_html_facts(r#"<html lang="en"><body></body></html>"#).html_lang_present()
            );
            assert!(
                !extract_html_facts(r#"<html lang="  "><body></body></html>"#).html_lang_present()
            );
            assert!(!extract_html_facts("<html><body></body></html>").html_lang_present());
        }

        #[test]
        fn image_alt_absent_attribute_counts_but_empty_alt_does_not() {
            let facts = extract_html_facts(
                r#"<html><body>
                    <img src="/a.png">
                    <img src="/b.png" alt="">
                    <img src="/c.png" alt="a real description">
                </body></html>"#,
            );
            assert_eq!(facts.image_count(), 3);
            // Only the first image (no `alt` attribute at all) counts.
            assert_eq!(facts.images_missing_alt(), 1);
        }

        // SCORPION_AUDIT_OBSERVED_TECHNOLOGY_MARKERS_001: meta-generator
        // capture reuses this same single parse pass — raw, untrimmed,
        // unfiltered values only; normalization is the marker layer's
        // job, not this one's.
        #[test]
        fn meta_generator_raw_capture_matches_every_other_fact_in_the_same_pass() {
            assert!(extract_html_facts("<html><head></head></html>")
                .meta_generators()
                .is_empty());
            assert_eq!(
                extract_html_facts(
                    r#"<html><head><meta name="generator" content="WordPress 6.4"></head></html>"#
                )
                .meta_generators(),
                &["WordPress 6.4".to_string()]
            );
            // Present-but-empty content is captured raw (not filtered
            // here) — the marker layer decides that policy.
            assert_eq!(
                extract_html_facts(
                    r#"<html><head><meta name="generator" content=""></head></html>"#
                )
                .meta_generators(),
                &["".to_string()]
            );
            // Attribute entirely absent contributes nothing.
            assert!(
                extract_html_facts(r#"<html><head><meta name="generator"></head></html>"#)
                    .meta_generators()
                    .is_empty()
            );
            // Multiple elements, document order, duplicates retained.
            assert_eq!(
                extract_html_facts(
                    r#"<html><head>
                        <meta name="generator" content="WordPress 6.4">
                        <meta name="generator" content="WordPress 6.4">
                        <meta name="generator" content="Elementor 3.2">
                    </head></html>"#
                )
                .meta_generators(),
                &[
                    "WordPress 6.4".to_string(),
                    "WordPress 6.4".to_string(),
                    "Elementor 3.2".to_string(),
                ]
            );
            // A discovery/OpenGraph-shaped generator-adjacent meta name
            // never substitutes — only the literal `name="generator"`.
            assert!(extract_html_facts(
                r#"<html><head><meta name="og:generator" content="Something"></head></html>"#
            )
            .meta_generators()
            .is_empty());
        }
    }

    mod page_facts_tests {
        use super::*;

        #[test]
        fn from_page_reflects_only_the_supplied_page() {
            let page = page_with_html(
                "https://example.test/",
                r#"<html><head><link rel="canonical" href="https://example.test/c"></head></html>"#,
            );
            let facts = PageFacts::from_page(&page);
            assert_eq!(facts.requested_url(), "https://example.test/");
            assert_eq!(facts.final_url(), "https://example.test/");
            assert_eq!(facts.effective_status(), 200);
            assert_eq!(
                facts.canonical_links(),
                &["https://example.test/c".to_string()]
            );
            assert_eq!(facts.content_type(), Some("text/html"));
            assert_eq!(facts.representation(), DocumentRepresentation::Html);
            assert!(facts.html().is_some());
        }

        #[test]
        fn header_observation_unavailable_is_none_not_empty_map() {
            let page = page_with_no_headers("https://example.test/", "<html></html>");
            let facts = PageFacts::from_page(&page);
            assert_eq!(facts.response_headers(), None);
        }

        #[test]
        fn header_observation_available_but_no_allowlisted_headers_is_some_empty() {
            let page = page_with_html("https://example.test/", "<html></html>");
            let facts = PageFacts::from_page(&page);
            // page_with_html sets only Content-Type, which is not on the
            // audit allowlist, so the observed set is legitimately empty
            // — but it must be `Some(empty)`, distinguishable from
            // `None` (observation unavailable).
            assert_eq!(facts.response_headers(), Some(&BTreeMap::new()));
        }

        #[test]
        fn representation_classification_matrix() {
            for (content_type, expected) in [
                ("text/html", DocumentRepresentation::Html),
                ("text/html; charset=utf-8", DocumentRepresentation::Html),
                ("TEXT/HTML", DocumentRepresentation::Html),
                ("application/xhtml+xml", DocumentRepresentation::Html),
                ("text/plain", DocumentRepresentation::NonHtml),
                ("application/json", DocumentRepresentation::NonHtml),
                ("image/png", DocumentRepresentation::NonHtml),
            ] {
                let page = page_with(
                    "https://example.test/",
                    "irrelevant",
                    content_type,
                    reqwest::StatusCode::OK,
                    &[],
                );
                assert_eq!(
                    PageFacts::from_page(&page).representation(),
                    expected,
                    "content_type={content_type}"
                );
            }
        }

        #[test]
        fn missing_content_type_is_unknown_never_html() {
            let page = page_with_no_headers("https://example.test/", "<html></html>");
            assert_eq!(
                PageFacts::from_page(&page).representation(),
                DocumentRepresentation::Unknown
            );
        }

        #[test]
        fn non_html_representation_carries_no_html_facts() {
            let page = page_with(
                "https://example.test/",
                "hello world",
                "text/plain",
                reqwest::StatusCode::OK,
                &[],
            );
            assert!(PageFacts::from_page(&page).html().is_none());
        }
    }

    mod rule_proof {
        use super::*;

        // Phase 15 Case A.
        #[tokio::test]
        async fn missing_canonical_link_produces_exactly_one_finding() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let page = page_with_html(
                "https://example.test/",
                "<html><head><title>Example</title></head><body>hello</body></html>",
            );
            let evidenced = EvidencedPageFacts::record(&store, &page).await.unwrap();
            let evidence_ref = evidenced.evidence_ref();

            let finding = seo_canonical_missing(&evidenced).expect("must find a violation");
            assert_eq!(finding.rule_id(), SEO_CANONICAL_MISSING_RULE_ID);
            assert_eq!(finding.rule_version(), SEO_CANONICAL_MISSING_RULE_VERSION);
            assert_eq!(finding.category(), FindingCategory::Seo);
            assert_eq!(
                finding.observed_condition(),
                &FindingCondition::CanonicalLinkCount(0)
            );
            assert_eq!(
                finding.expected_condition(),
                &FindingCondition::CanonicalLinkCountAtLeast(1)
            );
            assert_eq!(finding.evidence(), &[evidence_ref]);
        }

        // Phase 15 Case B.
        #[tokio::test]
        async fn present_canonical_link_produces_zero_findings() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let page = page_with_html(
                "https://example.test/product",
                r#"<html><head><link rel="canonical" href="https://example.test/product"></head></html>"#,
            );
            let evidenced = EvidencedPageFacts::record(&store, &page).await.unwrap();

            assert!(seo_canonical_missing(&evidenced).is_none());
        }
    }

    // Phase 17/18: non-HTML and non-2xx-status negative matrices — no
    // HTML DOM SEO rule may ever fire outside its stated applicability.
    mod applicability_negative_matrix {
        use super::*;

        async fn evidenced(page: &Page) -> EvidencedPageFacts {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            EvidencedPageFacts::record(&store, page).await.unwrap()
        }

        fn html_seo_rules() -> Vec<fn(&EvidencedPageFacts) -> Option<Finding>> {
            vec![
                seo_canonical_missing,
                seo_title_missing,
                seo_meta_description_missing,
                seo_h1_missing,
                seo_h1_multiple,
                seo_html_lang_missing,
                seo_image_alt_missing,
            ]
        }

        async fn assert_no_html_seo_findings(page: &Page) {
            let evidenced = evidenced(page).await;
            for rule in html_seo_rules() {
                assert!(
                    rule(&evidenced).is_none(),
                    "no HTML DOM SEO rule may fire for {:?} ({:?}/{})",
                    page.get_url(),
                    evidenced.facts().representation(),
                    evidenced.facts().effective_status()
                );
            }
        }

        // A: 200 text/plain.
        #[tokio::test]
        async fn text_plain_produces_no_html_seo_findings() {
            let page = page_with(
                "https://example.test/",
                "hello world",
                "text/plain",
                reqwest::StatusCode::OK,
                &[],
            );
            assert_no_html_seo_findings(&page).await;
        }

        // B: 200 application/json.
        #[tokio::test]
        async fn json_produces_no_html_seo_findings() {
            let page = page_with(
                "https://example.test/",
                r#"{"hello":"world"}"#,
                "application/json",
                reqwest::StatusCode::OK,
                &[],
            );
            assert_no_html_seo_findings(&page).await;
        }

        // C: 200 image/png.
        #[tokio::test]
        async fn image_produces_no_html_seo_findings() {
            let page = page_with(
                "https://example.test/x.png",
                "not really png bytes",
                "image/png",
                reqwest::StatusCode::OK,
                &[],
            );
            assert_no_html_seo_findings(&page).await;
        }

        // D: unknown/missing Content-Type.
        #[tokio::test]
        async fn missing_content_type_produces_no_html_seo_findings() {
            let page = page_with_no_headers(
                "https://example.test/",
                "<html><body>no content-type header at all</body></html>",
            );
            assert_no_html_seo_findings(&page).await;
        }

        // 404/500 text/html — an error document is never treated as a
        // normal SEO content page merely because it contains HTML.
        #[tokio::test]
        async fn error_status_html_documents_produce_no_html_seo_findings() {
            for status in [
                reqwest::StatusCode::NOT_FOUND,
                reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            ] {
                let page = page_with(
                    "https://example.test/missing",
                    "<html><body>not found</body></html>",
                    "text/html",
                    status,
                    &[],
                );
                assert_no_html_seo_findings(&page).await;
            }
        }

        // HTTPS/HSTS transport rules have their own, different
        // applicability and may still legitimately apply even when HTML
        // SEO rules do not.
        #[tokio::test]
        async fn transport_rules_remain_governed_by_their_own_applicability() {
            let page = page_with(
                "http://example.test/",
                "hello world",
                "text/plain",
                reqwest::StatusCode::OK,
                &[],
            );
            let evidenced = evidenced(&page).await;
            // No HTML SEO finding for this non-HTML page...
            for rule in html_seo_rules() {
                assert!(rule(&evidenced).is_none());
            }
            // ...but the scheme-only https rule still legitimately
            // applies regardless of representation.
            assert!(security_https_missing(&evidenced).is_some());
        }
    }

    // Phase 19: header-observation fidelity across every security rule.
    mod header_fidelity_matrix {
        use super::*;

        async fn evidenced(page: &Page) -> EvidencedPageFacts {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            EvidencedPageFacts::record(&store, page).await.unwrap()
        }

        // A: Page.headers == None -> no HSTS/CSP/XCTO absence Finding.
        #[tokio::test]
        async fn headers_unavailable_produces_no_header_absence_findings() {
            let page = page_with_no_headers("https://example.test/", "<html></html>");
            let evidenced = evidenced(&page).await;
            assert_eq!(evidenced.facts().response_headers(), None);
            assert!(security_hsts_missing(&evidenced).is_none());
            assert!(security_csp_missing(&evidenced).is_none());
            assert!(security_x_content_type_options_missing(&evidenced).is_none());
        }

        // B: Page.headers == Some(empty relevant set) on an applicable
        // page -> legitimate absence Findings.
        #[tokio::test]
        async fn headers_observed_empty_on_applicable_page_produces_absence_findings() {
            let page = page_with(
                "https://example.test/",
                "<html></html>",
                "text/html",
                reqwest::StatusCode::OK,
                &[],
            );
            let evidenced = evidenced(&page).await;
            assert_eq!(evidenced.facts().response_headers(), Some(&BTreeMap::new()));
            assert!(security_csp_missing(&evidenced).is_some());
            assert!(security_x_content_type_options_missing(&evidenced).is_some());
        }

        // C: HSTS present -> no security.hsts.missing.
        #[tokio::test]
        async fn hsts_present_suppresses_the_rule() {
            let page = page_with(
                "https://example.test/",
                "<html></html>",
                "text/html",
                reqwest::StatusCode::OK,
                &[("strict-transport-security", "max-age=63072000")],
            );
            let evidenced = evidenced(&page).await;
            assert!(security_hsts_missing(&evidenced).is_none());
        }

        // D: CSP present -> no security.csp.missing.
        #[tokio::test]
        async fn csp_present_suppresses_the_rule() {
            let page = page_with(
                "https://example.test/",
                "<html></html>",
                "text/html",
                reqwest::StatusCode::OK,
                &[("content-security-policy", "default-src 'self'")],
            );
            let evidenced = evidenced(&page).await;
            assert!(security_csp_missing(&evidenced).is_none());
        }

        // E: only CSP-Report-Only present -> security.csp.missing still
        // produced.
        #[tokio::test]
        async fn csp_report_only_alone_does_not_satisfy_enforcement_csp() {
            let page = page_with(
                "https://example.test/",
                "<html></html>",
                "text/html",
                reqwest::StatusCode::OK,
                &[("content-security-policy-report-only", "default-src 'self'")],
            );
            let evidenced = evidenced(&page).await;
            assert!(security_csp_missing(&evidenced).is_some());
        }

        // F: X-Content-Type-Options present -> no missing rule.
        #[tokio::test]
        async fn x_content_type_options_present_suppresses_the_rule() {
            let page = page_with(
                "https://example.test/",
                "<html></html>",
                "text/html",
                reqwest::StatusCode::OK,
                &[("x-content-type-options", "nosniff")],
            );
            let evidenced = evidenced(&page).await;
            assert!(security_x_content_type_options_missing(&evidenced).is_none());
        }

        // G: raw Set-Cookie values remain outside the audit header
        // evidence model entirely (F-5/earlier frontier's own guarantee,
        // reconfirmed at the PageFacts layer this frontier added).
        #[tokio::test]
        async fn set_cookie_raw_value_never_enters_page_facts() {
            const SENTINEL: &str = "SUPER_SECRET_SESSION_SENTINEL";
            let page = page_with(
                "https://example.test/",
                "<html></html>",
                "text/html",
                reqwest::StatusCode::OK,
                &[(
                    "set-cookie",
                    &format!("session={SENTINEL}; Secure; HttpOnly"),
                )],
            );
            let facts = PageFacts::from_page(&page);
            let headers = facts.response_headers().expect("headers observed");
            assert!(!headers.contains_key("set-cookie"));
            for values in headers.values() {
                for value in values {
                    assert!(!value
                        .windows(SENTINEL.len())
                        .any(|w| w == SENTINEL.as_bytes()));
                }
            }
        }

        // H: multi-value retained-header semantics remain unchanged —
        // every observed value for one allowlisted header name survives.
        #[tokio::test]
        async fn multi_value_header_semantics_are_preserved_through_page_facts() {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                reqwest::header::CONTENT_TYPE,
                reqwest::header::HeaderValue::from_static("text/html"),
            );
            headers.append(
                reqwest::header::HeaderName::from_static("content-security-policy"),
                reqwest::header::HeaderValue::from_static("default-src 'self'"),
            );
            headers.append(
                reqwest::header::HeaderName::from_static("content-security-policy"),
                reqwest::header::HeaderValue::from_static("report-uri /csp-report"),
            );
            let page = build(
                "https://example.test/",
                PageResponse {
                    content: Some(b"<html></html>".to_vec()),
                    status_code: reqwest::StatusCode::OK,
                    headers: Some(headers),
                    ..Default::default()
                },
            );
            let facts = PageFacts::from_page(&page);
            let observed = facts.response_headers().unwrap();
            assert_eq!(
                observed.get("content-security-policy"),
                Some(&vec![
                    b"default-src 'self'".to_vec(),
                    b"report-uri /csp-report".to_vec(),
                ])
            );
        }
    }

    // Phase 20: deterministic SEO fixture matrix.
    mod seo_fixture_matrix {
        use super::*;

        async fn evidenced(page: &Page) -> EvidencedPageFacts {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            EvidencedPageFacts::record(&store, page).await.unwrap()
        }

        fn seo_rule_ids(findings: &[Finding]) -> Vec<&str> {
            let mut ids: Vec<&str> = findings.iter().map(Finding::rule_id).collect();
            ids.sort_unstable();
            ids
        }

        #[tokio::test]
        async fn healthy_page_produces_zero_seo_findings() {
            let html = r#"<html lang="en"><head>
                <title>A Healthy Page</title>
                <meta name="description" content="A genuinely useful description.">
                <link rel="canonical" href="https://example.test/healthy">
            </head><body>
                <h1>Welcome</h1>
                <img src="/a.png" alt="a real photo">
            </body></html>"#;
            let page = page_with_html("https://example.test/healthy", html);
            let evidenced = evidenced(&page).await;
            let findings: Vec<Finding> = analyze_page(&evidenced)
                .into_iter()
                .filter(|f| f.category() == FindingCategory::Seo)
                .collect();
            assert!(findings.is_empty(), "unexpected SEO findings: {findings:?}");
        }

        #[tokio::test]
        async fn broken_page_produces_every_applicable_seo_finding() {
            let html = "<html><head></head><body><img src=\"/a.png\"></body></html>";
            let page = page_with_html("https://example.test/broken", html);
            let evidenced = evidenced(&page).await;
            let findings = analyze_page(&evidenced);
            let ids = seo_rule_ids(&findings);
            assert!(ids.contains(&SEO_CANONICAL_MISSING_RULE_ID));
            assert!(ids.contains(&SEO_TITLE_MISSING_RULE_ID));
            assert!(ids.contains(&SEO_META_DESCRIPTION_MISSING_RULE_ID));
            assert!(ids.contains(&SEO_H1_MISSING_RULE_ID));
            assert!(ids.contains(&SEO_HTML_LANG_MISSING_RULE_ID));
            assert!(ids.contains(&SEO_IMAGE_ALT_MISSING_RULE_ID));
            assert!(!ids.contains(&SEO_H1_MULTIPLE_RULE_ID));
        }

        #[tokio::test]
        async fn multi_h1_page_triggers_only_h1_multiple_among_heading_rules() {
            let html = r#"<html lang="en"><head><title>T</title>
                <meta name="description" content="d">
                <link rel="canonical" href="https://example.test/multi">
            </head><body><h1>One</h1><h1>Two</h1></body></html>"#;
            let page = page_with_html("https://example.test/multi", html);
            let evidenced = evidenced(&page).await;
            assert!(seo_h1_multiple(&evidenced).is_some());
            assert!(seo_h1_missing(&evidenced).is_none());
        }

        #[tokio::test]
        async fn decorative_image_with_empty_alt_is_not_flagged() {
            let html = r#"<html lang="en"><head><title>T</title>
                <meta name="description" content="d">
                <link rel="canonical" href="https://example.test/decorative">
            </head><body><h1>H</h1><img src="/x.png" alt=""></body></html>"#;
            let page = page_with_html("https://example.test/decorative", html);
            let evidenced = evidenced(&page).await;
            assert!(seo_image_alt_missing(&evidenced).is_none());
        }

        #[tokio::test]
        async fn image_without_alt_is_flagged_with_exact_count() {
            let html = r#"<html lang="en"><head><title>T</title>
                <meta name="description" content="d">
                <link rel="canonical" href="https://example.test/noalt">
            </head><body><h1>H</h1><img src="/x.png"></body></html>"#;
            let page = page_with_html("https://example.test/noalt", html);
            let evidenced = evidenced(&page).await;
            let finding = seo_image_alt_missing(&evidenced).expect("must flag");
            assert_eq!(
                finding.observed_condition(),
                &FindingCondition::ImagesMissingAlt(1)
            );
        }
    }

    // Phase 10/16: passive security rule matrix.
    mod security_rule_matrix {
        use super::*;

        async fn evidenced(page: &Page) -> EvidencedPageFacts {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            EvidencedPageFacts::record(&store, page).await.unwrap()
        }

        #[tokio::test]
        async fn http_scheme_triggers_https_missing() {
            let page = page_with(
                "http://example.test/",
                "<html></html>",
                "text/html",
                reqwest::StatusCode::OK,
                &[],
            );
            let evidenced = evidenced(&page).await;
            let finding = security_https_missing(&evidenced).expect("must flag http scheme");
            assert_eq!(finding.category(), FindingCategory::Security);
            assert_eq!(finding.severity(), FindingSeverity::High);
            assert_eq!(
                finding.observed_condition(),
                &FindingCondition::Scheme(UrlScheme::Http)
            );
        }

        #[tokio::test]
        async fn https_scheme_does_not_trigger_https_missing() {
            let page = page_with(
                "https://example.test/",
                "<html></html>",
                "text/html",
                reqwest::StatusCode::OK,
                &[],
            );
            let evidenced = evidenced(&page).await;
            assert!(security_https_missing(&evidenced).is_none());
        }

        #[tokio::test]
        async fn hsts_rule_never_applies_to_http_scheme() {
            let page = page_with(
                "http://example.test/",
                "<html></html>",
                "text/html",
                reqwest::StatusCode::OK,
                &[],
            );
            let evidenced = evidenced(&page).await;
            // Headers were observed (Some(empty)) but scheme is http —
            // HSTS applicability requires https specifically.
            assert!(security_hsts_missing(&evidenced).is_none());
        }

        #[tokio::test]
        async fn hsts_missing_on_https_with_observed_headers() {
            let page = page_with(
                "https://example.test/",
                "<html></html>",
                "text/html",
                reqwest::StatusCode::OK,
                &[],
            );
            let evidenced = evidenced(&page).await;
            let finding = security_hsts_missing(&evidenced).expect("must flag");
            assert_eq!(finding.severity(), FindingSeverity::Medium);
        }
    }

    // Phase 13/14/16: the generic analyzer and same-evidence invariant
    // across multiple simultaneously-triggered rules.
    mod same_evidence_across_rules {
        use super::*;

        #[tokio::test]
        async fn multiple_rules_on_one_page_all_share_the_same_evidence_ref() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            // A page with unrelated, already-valid evidence in the same
            // store beforehand — the same cross-page discipline the
            // prior binding-correction frontier established.
            let unrelated = page_with_html(
                "https://unrelated.example/",
                r#"<html lang="en"><head><title>T</title>
                    <meta name="description" content="d">
                    <link rel="canonical" href="https://unrelated.example/">
                </head><body><h1>H</h1></body></html>"#,
            );
            let _unrelated_evidence = record(&store, &unrelated).await;

            let html = "<html><head></head><body><img src=\"/a.png\"></body></html>";
            let page = page_with_html("https://example.test/broken", html);
            let evidenced = EvidencedPageFacts::record(&store, &page).await.unwrap();
            let findings = analyze_page(&evidenced);
            assert!(findings.len() >= 3, "fixture should trigger several rules");

            let expected_ref = evidenced.evidence_ref();
            for finding in &findings {
                assert_eq!(finding.evidence(), &[expected_ref]);
            }

            // Independently resolve and confirm it is genuinely this
            // exact page's evidence, not the unrelated one.
            let bundle = expected_ref.resolve(&store).await.unwrap().unwrap();
            assert_eq!(
                bundle.requested_url.as_deref(),
                Some("https://example.test/broken")
            );
        }

        #[tokio::test]
        async fn audit_page_persists_every_finding_under_the_same_evidence_ref() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            std::thread::spawn(move || {
                use std::io::{Read, Write};
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut buf = [0_u8; 4096];
                    let _ = stream.read(&mut buf);
                    let body = "<html><head></head><body><img src=\"/a.png\"></body></html>";
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(body.as_bytes());
                }
            });
            let url = format!("http://{addr}/");

            let result = audit_page(&store, &url).await.unwrap();
            assert!(result.findings().len() >= 3);
            for finding in result.findings() {
                assert_eq!(finding.evidence(), &[result.evidence_ref()]);
                // Every persisted finding reads back identically.
                let read_back = read_finding(&store, &finding.id()).await.unwrap().unwrap();
                assert_eq!(&read_back, finding);
            }
        }
    }

    // Phase 21: determinism — repeated analysis of one already-recorded
    // EvidencedPageFacts produces identical output every time.
    mod determinism {
        use super::*;

        #[tokio::test]
        async fn repeated_analysis_is_byte_identical() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let html = "<html><head></head><body><img src=\"/a.png\"></body></html>";
            let page = page_with_html("https://example.test/broken", html);
            let evidenced = EvidencedPageFacts::record(&store, &page).await.unwrap();

            let first = analyze_page(&evidenced);
            let second = analyze_page(&evidenced);

            assert_eq!(first.len(), second.len());
            for (a, b) in first.iter().zip(second.iter()) {
                assert_eq!(a, b);
                assert_eq!(a.id(), b.id());
                assert_eq!(a.rule_id(), b.rule_id());
                assert_eq!(a.rule_version(), b.rule_version());
                assert_eq!(
                    serde_json::to_string(a).unwrap(),
                    serde_json::to_string(b).unwrap()
                );
            }
            // Ordering itself is identical (PAGE_RULES declaration order,
            // never HashMap iteration order).
            let first_ids: Vec<&str> = first.iter().map(Finding::rule_id).collect();
            let second_ids: Vec<&str> = second.iter().map(Finding::rule_id).collect();
            assert_eq!(first_ids, second_ids);
        }
    }

    // Phase 13/28: the generic rule registry itself.
    mod generic_analyzer {
        use super::*;

        #[test]
        fn exactly_eleven_production_rules_with_unique_ids() {
            assert_eq!(PAGE_RULES.len(), 11);
        }

        #[tokio::test]
        async fn production_rule_ids_are_unique() {
            // Trigger every rule at once (an empty, http, plain page)
            // and confirm the rule_ids on any findings produced are
            // pairwise distinct — a structural uniqueness proof over the
            // real registry, not a hardcoded literal list.
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let page = page_with(
                "http://example.test/",
                "<html><head></head><body><img src=\"/a.png\"></body></html>",
                "text/html",
                reqwest::StatusCode::OK,
                &[],
            );
            let evidenced = EvidencedPageFacts::record(&store, &page).await.unwrap();
            let findings = analyze_page(&evidenced);
            let mut ids: Vec<&str> = findings.iter().map(Finding::rule_id).collect();
            let before = ids.len();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(
                ids.len(),
                before,
                "duplicate rule_id among produced findings"
            );
        }

        #[tokio::test]
        async fn ordering_is_declaration_order_not_hashmap_dependent() {
            // PAGE_RULES is a plain slice — iteration order is exactly
            // declaration order by construction, never HashMap iteration
            // order. Proven behaviorally (function-pointer equality
            // comparison is not reliable — addresses are not guaranteed
            // unique): a fixture that triggers every rule must yield
            // findings in exactly this declared rule_id sequence, on
            // every repeated run.
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let page = page_with(
                "http://example.test/",
                "<html><head></head><body><img src=\"/a.png\"></body></html>",
                "text/html",
                reqwest::StatusCode::OK,
                &[],
            );
            let evidenced = EvidencedPageFacts::record(&store, &page).await.unwrap();

            let expected_order = [
                SEO_CANONICAL_MISSING_RULE_ID,
                SEO_TITLE_MISSING_RULE_ID,
                SEO_META_DESCRIPTION_MISSING_RULE_ID,
                SEO_H1_MISSING_RULE_ID,
                SEO_HTML_LANG_MISSING_RULE_ID,
                SEO_IMAGE_ALT_MISSING_RULE_ID,
                SECURITY_HTTPS_MISSING_RULE_ID,
                SECURITY_CSP_MISSING_RULE_ID,
                SECURITY_X_CONTENT_TYPE_OPTIONS_MISSING_RULE_ID,
            ];

            for _ in 0..3 {
                let findings = analyze_page(&evidenced);
                let ids: Vec<&str> = findings.iter().map(Finding::rule_id).collect();
                assert_eq!(ids, expected_order.to_vec());
            }
        }
    }

    // Phase 23: historical v1 canonical-missing compatibility. A golden,
    // hand-verified FindingId for a fixed input using the pre-frontier
    // `rule_version = 1` — this is a literal regression test: any
    // accidental change to `FindingId::derive`'s formula or
    // `FindingCondition::CanonicalLinkCount`/`CanonicalLinkCountAtLeast`'s
    // `identity_repr()` wording would change this hardcoded value.
    mod historical_compatibility {
        use super::*;

        #[test]
        fn old_v1_canonical_missing_payload_still_deserializes_and_reproduces_its_id() {
            // Exactly the shape SCORPION_AUDIT_FACTS_AND_FINDING_CONTRACT_
            // FRONTIER_001's original code would have serialized, with a
            // fixed, deterministic EvidenceId wire string standing in for
            // whatever real evidence a historical record referenced.
            let historical_json = r#"{
                "rule_id": "seo.canonical.missing",
                "rule_version": 1,
                "category": "Seo",
                "severity": "Medium",
                "target": "https://example.test/",
                "observed_condition": { "CanonicalLinkCount": 0 },
                "expected_condition": { "CanonicalLinkCountAtLeast": 1 },
                "evidence": [{ "id": "evid_0123456789abcdef0123456789abcdef" }]
            }"#;
            let finding: Finding = serde_json::from_str(historical_json)
                .expect("a genuine v1 payload must still deserialize");
            assert_eq!(finding.rule_version(), 1);
            assert_eq!(
                finding.id().as_str(),
                "finding_dea5b498b30162f5294a044db3bf73a04404021970973c1456a9269c1373048e",
                "the identity formula for historical v1 canonical-missing \
                 Findings must never change"
            );
        }

        #[tokio::test]
        async fn new_execution_uses_version_2_and_yields_a_different_identity() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let page = page_with_html(
                "https://example.test/",
                "<html><head><title>T</title></head></html>",
            );
            let evidenced = EvidencedPageFacts::record(&store, &page).await.unwrap();
            let finding = seo_canonical_missing(&evidenced).unwrap();
            assert_eq!(finding.rule_version(), 2);
            assert_ne!(finding.rule_version(), 1);
        }
    }

    mod finding_identity_tests {
        use super::*;

        fn sample_finding(evidence: Vec<EvidenceRef>) -> Finding {
            Finding::new(
                SEO_CANONICAL_MISSING_RULE_ID,
                SEO_CANONICAL_MISSING_RULE_VERSION,
                FindingCategory::Seo,
                FindingSeverity::Medium,
                "https://example.test/",
                FindingCondition::CanonicalLinkCount(0),
                FindingCondition::CanonicalLinkCountAtLeast(1),
                evidence,
            )
            .unwrap()
        }

        fn evidence_ref_n(n: u8) -> EvidenceRef {
            // Deterministic-enough distinct EvidenceRefs for identity
            // tests: EvidenceId has no public byte constructor, so mint
            // via the canonical `new()` and rely on distinct values,
            // asserted only for inequality, never a specific byte
            // pattern.
            let _ = n;
            EvidenceRef::new(crate::features::identity::EvidenceId::new())
        }

        // 1 & 2: same semantic input -> same FindingId and same
        // serialized semantic payload.
        #[test]
        fn same_semantic_input_yields_same_id_and_payload() {
            let reference = evidence_ref_n(1);
            let a = sample_finding(vec![reference]);
            let b = sample_finding(vec![reference]);
            assert_eq!(a.id(), b.id());
            assert_eq!(
                serde_json::to_string(&a).unwrap(),
                serde_json::to_string(&b).unwrap()
            );
        }

        // 3: evidence input ordering does not alter FindingId.
        #[test]
        fn evidence_ordering_does_not_alter_identity() {
            let r1 = evidence_ref_n(1);
            let r2 = evidence_ref_n(2);
            let forward = sample_finding(vec![r1, r2]);
            let backward = sample_finding(vec![r2, r1]);
            assert_eq!(forward.id(), backward.id());
        }

        // 4: duplicate EvidenceRefs are canonicalized deterministically.
        #[test]
        fn duplicate_evidence_refs_are_canonicalized() {
            let r1 = evidence_ref_n(1);
            let with_duplicate = sample_finding(vec![r1, r1, r1]);
            let without_duplicate = sample_finding(vec![r1]);
            assert_eq!(with_duplicate.evidence(), &[r1]);
            assert_eq!(with_duplicate.id(), without_duplicate.id());
        }

        // 5: empty EvidenceRefs fail closed.
        #[test]
        fn empty_evidence_fails_closed() {
            let result = Finding::new(
                SEO_CANONICAL_MISSING_RULE_ID,
                SEO_CANONICAL_MISSING_RULE_VERSION,
                FindingCategory::Seo,
                FindingSeverity::Medium,
                "https://example.test/",
                FindingCondition::CanonicalLinkCount(0),
                FindingCondition::CanonicalLinkCountAtLeast(1),
                vec![],
            );
            assert!(matches!(result, Err(AuditError::EmptyEvidence)));
        }

        // 10: severity remains separate from observed fact.
        #[test]
        fn severity_is_distinct_from_observed_condition() {
            let finding = sample_finding(vec![evidence_ref_n(1)]);
            assert_eq!(finding.severity(), FindingSeverity::Medium);
            assert_ne!(
                finding.observed_condition(),
                &FindingCondition::CanonicalLinkCount(1)
            );
        }

        // 11: rule_version participates in identity.
        #[test]
        fn rule_version_participates_in_identity() {
            let reference = evidence_ref_n(1);
            let v1 = sample_finding(vec![reference]);
            let v2 = Finding::new(
                SEO_CANONICAL_MISSING_RULE_ID,
                SEO_CANONICAL_MISSING_RULE_VERSION + 1,
                FindingCategory::Seo,
                FindingSeverity::Medium,
                "https://example.test/",
                FindingCondition::CanonicalLinkCount(0),
                FindingCondition::CanonicalLinkCountAtLeast(1),
                vec![reference],
            )
            .unwrap();
            assert_ne!(v1.id(), v2.id());
        }

        // 12: target participates in identity.
        #[test]
        fn target_participates_in_identity() {
            let reference = evidence_ref_n(1);
            let a = sample_finding(vec![reference]);
            let b = Finding::new(
                SEO_CANONICAL_MISSING_RULE_ID,
                SEO_CANONICAL_MISSING_RULE_VERSION,
                FindingCategory::Seo,
                FindingSeverity::Medium,
                "https://example.test/other",
                FindingCondition::CanonicalLinkCount(0),
                FindingCondition::CanonicalLinkCountAtLeast(1),
                vec![reference],
            )
            .unwrap();
            assert_ne!(a.id(), b.id());
        }

        // 13: different evidence produces different Finding identity.
        #[test]
        fn different_evidence_produces_different_identity() {
            let a = sample_finding(vec![evidence_ref_n(1)]);
            let b = sample_finding(vec![evidence_ref_n(2)]);
            assert_ne!(a.id(), b.id());
        }
    }

    mod persistence_tests {
        use super::*;

        #[tokio::test]
        async fn recording_verifies_and_round_trips() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let page = page_with_html(
                "https://example.test/",
                "<html><head></head><body>hello</body></html>",
            );
            let evidenced = EvidencedPageFacts::record(&store, &page).await.unwrap();
            let finding = seo_canonical_missing(&evidenced).unwrap();
            let id = finding.id();

            let persisted = record_finding(&store, finding.clone()).await.unwrap();
            assert_eq!(persisted, finding);

            // 8: persisted Finding reads back exactly.
            let read_back = read_finding(&store, &id).await.unwrap().unwrap();
            assert_eq!(read_back, finding);
        }

        // 7: successful duplicate persistence is idempotent.
        #[tokio::test]
        async fn duplicate_persistence_is_idempotent() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let page = page_with_html(
                "https://example.test/",
                "<html><head></head><body>hello</body></html>",
            );
            let evidenced = EvidencedPageFacts::record(&store, &page).await.unwrap();
            let finding = seo_canonical_missing(&evidenced).unwrap();

            record_finding(&store, finding.clone()).await.unwrap();
            let second = record_finding(&store, finding.clone()).await;
            assert!(second.is_ok());
        }

        // 6 / Phase 18: unresolved EvidenceRef fails closed, nothing
        // persisted.
        #[tokio::test]
        async fn unresolvable_evidence_ref_fails_closed() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let bogus_ref = EvidenceRef::new(crate::features::identity::EvidenceId::new());
            let finding = Finding::new(
                SEO_CANONICAL_MISSING_RULE_ID,
                SEO_CANONICAL_MISSING_RULE_VERSION,
                FindingCategory::Seo,
                FindingSeverity::Medium,
                "https://example.test/",
                FindingCondition::CanonicalLinkCount(0),
                FindingCondition::CanonicalLinkCountAtLeast(1),
                vec![bogus_ref],
            )
            .unwrap();
            let id = finding.id();

            let result = record_finding(&store, finding).await;
            assert!(matches!(result, Err(AuditError::EvidenceUnresolvable(_))));

            // Fail-closed proof: nothing was persisted for this identity.
            assert!(read_finding(&store, &id).await.unwrap().is_none());
        }

        // 9: corruption/invalid payload fails closed on readback.
        #[tokio::test]
        async fn corrupted_payload_fails_closed_on_readback() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let id = FindingId(format!("{}deadbeef", FindingId::PREFIX));
            store
                .append_history(id.as_str(), 1, b"not valid json", SystemTime::now())
                .await
                .unwrap();

            let result = read_finding(&store, &id).await;
            assert!(matches!(result, Err(AuditError::Serialization(_))));
        }

        // 14: a Finding cannot be serialized as/decoded from an
        // EvidenceBundle, and never enters the evidence ledger as
        // retrieval evidence.
        #[tokio::test]
        async fn finding_never_enters_the_evidence_ledger_as_retrieval_evidence() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let page = page_with_html(
                "https://example.test/",
                "<html><head></head><body>hello</body></html>",
            );
            let evidenced = EvidencedPageFacts::record(&store, &page).await.unwrap();
            let finding = seo_canonical_missing(&evidenced).unwrap();
            let payload = serde_json::to_vec(&finding).unwrap();

            // The finding's own serialized shape is not genuine
            // retrieval evidence — even in the degenerate case where
            // EvidenceBundle's all-`Option` fields let it parse at all
            // (every field unrecognized -> None), it must carry none of
            // the acquired page's actual evidence content.
            let as_bundle: Result<EvidenceBundle, _> = serde_json::from_slice(&payload);
            if let Ok(bundle) = as_bundle {
                assert_eq!(bundle.requested_url, None);
                assert_eq!(bundle.content, None);
                assert_eq!(bundle.response_body_hash, None);
            }
        }

        // SCORPION_CANONICAL_SHARED_DOMAIN_PERSISTENCE_RUNTIME_BINDING_001
        // Gate 4: a Finding recorded through one `DomainPersistence`
        // handle is readable, byte-for-byte, through a second,
        // independently opened handle against the same real file — the
        // same cross-handle durability `audit_page` will rely on once a
        // production interface (MCP, and later a Web Console) opens its
        // own handle against the shared canonical store rather than the
        // exact handle that wrote it.
        #[tokio::test]
        async fn finding_recorded_through_one_handle_is_readable_through_a_second_handle_on_the_same_file(
        ) {
            let path = std::env::temp_dir().join(format!(
                "scorpion-shared-binding-finding-test-{}-{}.sqlite3",
                std::process::id(),
                crate::features::identity::EvidenceId::new()
            ));
            let _ = std::fs::remove_file(&path);

            let writer = DomainPersistence::open(&path).await.unwrap();
            let page = page_with_html(
                "https://example.test/",
                "<html><head></head><body>hello</body></html>",
            );
            let evidenced = EvidencedPageFacts::record(&writer, &page).await.unwrap();
            let finding = seo_canonical_missing(&evidenced).unwrap();
            let id = finding.id();
            record_finding(&writer, finding.clone()).await.unwrap();
            drop(writer);

            let reader = DomainPersistence::open(&path).await.unwrap();
            let read_back = read_finding(&reader, &id)
                .await
                .unwrap()
                .expect("a Finding recorded by one handle must read back through another");
            assert_eq!(read_back, finding);

            let _ = std::fs::remove_file(&path);
        }
    }

    // Phase 16: full local-fixture end-to-end proof.
    mod end_to_end {
        use super::*;
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        fn fixture_server(html: &'static str) -> String {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut buf = [0_u8; 4096];
                    let _ = stream.read(&mut buf);
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        html.len()
                    );
                    let _ = stream.write_all(html.as_bytes());
                }
            });
            format!("http://{addr}/")
        }

        #[tokio::test]
        async fn full_chain_from_real_acquisition_to_finding_readback() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let url = fixture_server(
                "<html><head><title>No canonical here</title></head><body>hello</body></html>",
            );

            // 1-2: acquire one HTML page, prove Page received (implicit
            // in a successful, non-erroring seam call). This minimal
            // fixture legitimately triggers several rules at once (no
            // canonical, no meta description, no h1, no html lang, http
            // scheme, no CSP, no X-Content-Type-Options) — only title is
            // genuinely present.
            let result = audit_page(&store, &url).await.unwrap();
            assert!(!result.findings().is_empty());

            let finding = result
                .findings()
                .iter()
                .find(|f| f.rule_id() == SEO_CANONICAL_MISSING_RULE_ID)
                .expect("this fixture has no canonical link")
                .clone();

            // 9: read Finding back by FindingId.
            let id = finding.id();
            let read_back = read_finding(&store, &id).await.unwrap().unwrap();
            assert_eq!(read_back, finding);

            // 16 / same-evidence invariant: every Finding from this one
            // page audit shares the exact same EvidenceRef as the audit
            // result itself.
            for f in result.findings() {
                assert_eq!(f.evidence(), &[result.evidence_ref()]);
            }

            // 10-12: resolve the shared EvidenceRef, independently
            // confirm it names the acquired page and that the persisted
            // page material lacks a canonical link.
            let bundle = result
                .evidence_ref()
                .resolve(&store)
                .await
                .unwrap()
                .expect("evidence must resolve");
            assert_eq!(bundle.requested_url.as_deref(), Some(url.as_str()));
            let content = bundle.content.expect("acquired page content persisted");
            assert!(extract_html_facts(&content).canonical_links().is_empty());
        }

        #[tokio::test]
        async fn full_chain_with_a_canonical_link_produces_no_canonical_finding() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let url = fixture_server(
                r#"<html><head><link rel="canonical" href="https://example.test/product"></head></html>"#,
            );

            let result = audit_page(&store, &url).await.unwrap();
            assert!(result
                .findings()
                .iter()
                .all(|f| f.rule_id() != SEO_CANONICAL_MISSING_RULE_ID));
        }
    }

    // SCORPION_AUDIT_OBSERVED_TECHNOLOGY_MARKERS_001: deterministic
    // technology-marker extraction — header observations, HTML
    // observations, same-evidence/single-acquisition proof, and the
    // non-inference boundary.
    mod technology_markers {
        use super::*;

        async fn evidenced(page: &Page) -> EvidencedPageFacts {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            EvidencedPageFacts::record(&store, page).await.unwrap()
        }

        fn header_value(bytes: &[u8]) -> reqwest::header::HeaderValue {
            reqwest::header::HeaderValue::from_bytes(bytes).unwrap()
        }

        // ---- HEADER OBSERVATIONS ----

        #[tokio::test]
        async fn server_header_observed_produces_exact_marker() {
            let page = page_with(
                "https://example.test/",
                "hello",
                "text/plain",
                reqwest::StatusCode::OK,
                &[("server", "nginx/1.24.0")],
            );
            let markers = extract_technology_markers(&evidenced(&page).await);
            assert_eq!(
                markers,
                vec![ObservedTechnologyMarker {
                    source: TechnologyMarkerSource::ResponseHeader("server".to_string()),
                    value: "nginx/1.24.0".to_string(),
                }]
            );
        }

        #[tokio::test]
        async fn x_powered_by_header_observed_produces_exact_marker() {
            let page = page_with(
                "https://example.test/",
                "hello",
                "text/plain",
                reqwest::StatusCode::OK,
                &[("x-powered-by", "Express")],
            );
            let markers = extract_technology_markers(&evidenced(&page).await);
            assert_eq!(
                markers,
                vec![ObservedTechnologyMarker {
                    source: TechnologyMarkerSource::ResponseHeader("x-powered-by".to_string()),
                    value: "Express".to_string(),
                }]
            );
        }

        #[tokio::test]
        async fn x_generator_header_observed_produces_exact_marker() {
            let page = page_with(
                "https://example.test/",
                "hello",
                "text/plain",
                reqwest::StatusCode::OK,
                &[("x-generator", "Drupal 9")],
            );
            let markers = extract_technology_markers(&evidenced(&page).await);
            assert_eq!(
                markers,
                vec![ObservedTechnologyMarker {
                    source: TechnologyMarkerSource::ResponseHeader("x-generator".to_string()),
                    value: "Drupal 9".to_string(),
                }]
            );
        }

        // Deterministic ordering across distinct marker headers:
        // MARKER_HEADER_NAMES order (server, x-powered-by, x-generator),
        // never insertion/HashMap order.
        #[tokio::test]
        async fn multiple_distinct_header_markers_follow_declared_order() {
            let page = page_with(
                "https://example.test/",
                "hello",
                "text/plain",
                reqwest::StatusCode::OK,
                &[
                    ("x-generator", "Drupal 9"),
                    ("x-powered-by", "Express"),
                    ("server", "nginx"),
                ],
            );
            let markers = extract_technology_markers(&evidenced(&page).await);
            let sources: Vec<&str> = markers
                .iter()
                .map(|m| match m.source() {
                    TechnologyMarkerSource::ResponseHeader(name) => name.as_str(),
                    TechnologyMarkerSource::HtmlMetaGenerator => "meta",
                })
                .collect();
            assert_eq!(sources, vec!["server", "x-powered-by", "x-generator"]);
        }

        // Repeated identical header values are retained, never
        // silently deduplicated.
        #[tokio::test]
        async fn repeated_identical_header_values_are_retained_not_deduplicated() {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                reqwest::header::CONTENT_TYPE,
                reqwest::header::HeaderValue::from_static("text/plain"),
            );
            headers.append(
                reqwest::header::HeaderName::from_static("server"),
                reqwest::header::HeaderValue::from_static("nginx"),
            );
            headers.append(
                reqwest::header::HeaderName::from_static("server"),
                reqwest::header::HeaderValue::from_static("nginx"),
            );
            let page = build(
                "https://example.test/",
                PageResponse {
                    content: Some(b"hello".to_vec()),
                    status_code: reqwest::StatusCode::OK,
                    headers: Some(headers),
                    ..Default::default()
                },
            );
            let markers = extract_technology_markers(&evidenced(&page).await);
            assert_eq!(
                markers,
                vec![
                    ObservedTechnologyMarker {
                        source: TechnologyMarkerSource::ResponseHeader("server".to_string()),
                        value: "nginx".to_string(),
                    },
                    ObservedTechnologyMarker {
                        source: TechnologyMarkerSource::ResponseHeader("server".to_string()),
                        value: "nginx".to_string(),
                    },
                ]
            );
        }

        // Header observation itself unavailable -> zero header-derived
        // markers, exactly like every header-absence security rule.
        #[tokio::test]
        async fn headers_unavailable_produces_no_header_markers() {
            let page = page_with_no_headers("https://example.test/", "hello");
            let evidenced = evidenced(&page).await;
            assert_eq!(evidenced.facts().response_headers(), None);
            assert!(extract_response_header_technology_markers(evidenced.facts()).is_empty());
        }

        // An observed-but-empty marker header value produces no marker
        // (an empty value identifies no technology) — documented
        // deterministic behavior.
        #[tokio::test]
        async fn observed_empty_header_value_produces_no_marker() {
            let page = page_with(
                "https://example.test/",
                "hello",
                "text/plain",
                reqwest::StatusCode::OK,
                &[("server", "   ")],
            );
            let markers = extract_technology_markers(&evidenced(&page).await);
            assert!(markers.is_empty());
        }

        // Non-UTF-8 header bytes fail closed for that one value rather
        // than a lossy/fabricated substitute — other, valid values for
        // the same header still survive.
        #[tokio::test]
        async fn non_utf8_header_value_fails_closed_for_that_marker() {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                reqwest::header::CONTENT_TYPE,
                reqwest::header::HeaderValue::from_static("text/plain"),
            );
            headers.append(
                reqwest::header::HeaderName::from_static("server"),
                header_value(&[0x78, 0xFF, 0xFE, 0x79]),
            );
            headers.append(
                reqwest::header::HeaderName::from_static("server"),
                reqwest::header::HeaderValue::from_static("nginx"),
            );
            let page = build(
                "https://example.test/",
                PageResponse {
                    content: Some(b"hello".to_vec()),
                    status_code: reqwest::StatusCode::OK,
                    headers: Some(headers),
                    ..Default::default()
                },
            );
            let markers = extract_technology_markers(&evidenced(&page).await);
            assert_eq!(
                markers,
                vec![ObservedTechnologyMarker {
                    source: TechnologyMarkerSource::ResponseHeader("server".to_string()),
                    value: "nginx".to_string(),
                }]
            );
        }

        // Set-Cookie is not in AUDIT_RESPONSE_HEADER_ALLOWLIST at all, so
        // it structurally cannot become a marker; reconfirmed here at
        // the marker layer.
        #[tokio::test]
        async fn set_cookie_never_becomes_a_marker() {
            let page = page_with(
                "https://example.test/",
                "hello",
                "text/plain",
                reqwest::StatusCode::OK,
                &[("set-cookie", "session=SECRET; Secure; HttpOnly")],
            );
            let markers = extract_technology_markers(&evidenced(&page).await);
            assert!(markers.is_empty());
        }

        // Authorization/Cookie/Proxy-Authorization never become markers
        // even if a (misconfigured) response echoed them — defense in
        // depth on top of the closed MARKER_HEADER_NAMES allowlist.
        #[tokio::test]
        async fn credential_bearing_headers_never_become_markers() {
            let page = page_with(
                "https://example.test/",
                "hello",
                "text/plain",
                reqwest::StatusCode::OK,
                &[
                    ("authorization", "Bearer SUPER_SECRET_TOKEN"),
                    ("cookie", "session=SUPER_SECRET_TOKEN"),
                    ("proxy-authorization", "Basic SUPER_SECRET_TOKEN"),
                ],
            );
            let markers = extract_technology_markers(&evidenced(&page).await);
            assert!(markers.is_empty());
            for marker in &markers {
                assert!(!marker.value().contains("SUPER_SECRET_TOKEN"));
            }
        }

        // ---- HTML OBSERVATIONS ----

        #[tokio::test]
        async fn meta_generator_observed_produces_marker() {
            let page = page_with_html(
                "https://example.test/",
                r#"<html><head><meta name="generator" content="WordPress 6.4"></head></html>"#,
            );
            let markers = extract_technology_markers(&evidenced(&page).await);
            assert_eq!(
                markers,
                vec![ObservedTechnologyMarker {
                    source: TechnologyMarkerSource::HtmlMetaGenerator,
                    value: "WordPress 6.4".to_string(),
                }]
            );
        }

        #[tokio::test]
        async fn multiple_meta_generators_preserve_document_order_and_duplicates() {
            let page = page_with_html(
                "https://example.test/",
                r#"<html><head>
                    <meta name="generator" content="WordPress 6.4">
                    <meta name="generator" content="WordPress 6.4">
                    <meta name="generator" content="Elementor 3.2">
                </head></html>"#,
            );
            let markers = extract_technology_markers(&evidenced(&page).await);
            let values: Vec<&str> = markers
                .iter()
                .map(ObservedTechnologyMarker::value)
                .collect();
            assert_eq!(
                values,
                vec!["WordPress 6.4", "WordPress 6.4", "Elementor 3.2"]
            );
        }

        #[tokio::test]
        async fn empty_generator_content_produces_no_marker() {
            let page = page_with_html(
                "https://example.test/",
                r#"<html><head><meta name="generator" content="   "></head></html>"#,
            );
            let markers = extract_technology_markers(&evidenced(&page).await);
            assert!(markers.is_empty());
        }

        // An HTML error page can truthfully expose a generator tag —
        // technology markers are not gated on 2xx status, deliberately
        // unlike page_content_seo_applicable.
        #[tokio::test]
        async fn meta_generator_on_html_error_page_still_produces_a_marker() {
            let page = page_with(
                "https://example.test/missing",
                r#"<html><head><meta name="generator" content="WordPress 6.4"></head></html>"#,
                "text/html",
                reqwest::StatusCode::NOT_FOUND,
                &[],
            );
            let markers = extract_technology_markers(&evidenced(&page).await);
            assert_eq!(
                markers,
                vec![ObservedTechnologyMarker {
                    source: TechnologyMarkerSource::HtmlMetaGenerator,
                    value: "WordPress 6.4".to_string(),
                }]
            );
        }

        #[tokio::test]
        async fn text_plain_containing_meta_generator_markup_produces_no_html_marker() {
            let page = page_with(
                "https://example.test/",
                r#"<meta name="generator" content="WordPress 6.4">"#,
                "text/plain",
                reqwest::StatusCode::OK,
                &[],
            );
            let markers = extract_technology_markers(&evidenced(&page).await);
            assert!(markers.is_empty());
        }

        #[tokio::test]
        async fn json_containing_html_like_string_produces_no_html_marker() {
            let page = page_with(
                "https://example.test/",
                r#"{"note":"<meta name=\"generator\" content=\"WordPress 6.4\">"}"#,
                "application/json",
                reqwest::StatusCode::OK,
                &[],
            );
            let markers = extract_technology_markers(&evidenced(&page).await);
            assert!(markers.is_empty());
        }

        #[tokio::test]
        async fn image_representation_produces_no_html_marker() {
            let page = page_with(
                "https://example.test/x.png",
                "not really png bytes",
                "image/png",
                reqwest::StatusCode::OK,
                &[],
            );
            let markers = extract_technology_markers(&evidenced(&page).await);
            assert!(markers.is_empty());
        }

        // ---- EVIDENCE / SINGLE-ACQUISITION / DETERMINISM ----

        #[tokio::test]
        async fn markers_and_findings_from_one_audit_page_share_the_same_evidence_ref() {
            use std::io::Write;
            use std::net::TcpListener;

            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let html =
                r#"<html><head><meta name="generator" content="WordPress 6.4"></head></html>"#;
            std::thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut buf = [0u8; 1024];
                    let _ = std::io::Read::read(&mut stream, &mut buf);
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nServer: nginx\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        html.len()
                    );
                    let _ = stream.write_all(html.as_bytes());
                }
            });
            let url = format!("http://{addr}/");

            let store = DomainPersistence::open_in_memory().await.unwrap();
            let result = audit_page(&store, &url).await.unwrap();

            assert!(!result.findings().is_empty());
            assert!(!result.technology_markers().is_empty());
            for f in result.findings() {
                assert_eq!(f.evidence(), &[result.evidence_ref()]);
            }
            // Technology markers do not themselves carry an EvidenceRef
            // (no independent identity — see this module's doc comment),
            // but every one of them came from the exact same
            // PageAuditResult, which itself names exactly one shared
            // EvidenceRef for both findings and markers.
            assert!(result
                .technology_markers()
                .iter()
                .any(|m| m.source() == &TechnologyMarkerSource::HtmlMetaGenerator));
            assert!(result.technology_markers().iter().any(
                |m| m.source() == &TechnologyMarkerSource::ResponseHeader("server".to_string())
            ));
        }

        // Deterministic repeated analysis: calling extract_technology_markers
        // twice on the same EvidencedPageFacts yields an identical sequence.
        #[tokio::test]
        async fn repeated_analysis_produces_an_identical_marker_sequence() {
            let page = page_with(
                "https://example.test/",
                r#"<html><head><meta name="generator" content="WordPress 6.4"></head></html>"#,
                "text/html",
                reqwest::StatusCode::OK,
                &[("server", "nginx"), ("x-powered-by", "PHP/8.2")],
            );
            let evidenced = evidenced(&page).await;
            let first = extract_technology_markers(&evidenced);
            let second = extract_technology_markers(&evidenced);
            assert_eq!(first, second);
            assert_eq!(first.len(), 3);
        }

        // extract_technology_markers is a pure function of an already
        // evidenced page: no acquisition, no persistence handle. This
        // is a compile-time proof (the call below would not type-check
        // if the signature required &DomainPersistence or async), not
        // merely a runtime assertion.
        #[tokio::test]
        async fn extract_technology_markers_takes_only_evidenced_page_facts() {
            let page = page_with_html("https://example.test/", "<html></html>");
            let evidenced = evidenced(&page).await;
            let _markers: Vec<ObservedTechnologyMarker> = extract_technology_markers(&evidenced);
        }

        // ---- NON-INFERENCE BOUNDARY ----

        #[tokio::test]
        async fn wp_content_path_alone_produces_no_marker() {
            let page = page_with(
                "https://example.test/wp-content/uploads/x.png",
                "<html><body>no header or meta markers at all</body></html>",
                "text/html",
                reqwest::StatusCode::OK,
                &[],
            );
            let markers = extract_technology_markers(&evidenced(&page).await);
            assert!(markers.is_empty());
        }

        #[tokio::test]
        async fn php_url_alone_produces_no_marker() {
            let page = page_with(
                "https://example.test/index.php?p=1",
                "<html><body>no header or meta markers at all</body></html>",
                "text/html",
                reqwest::StatusCode::OK,
                &[],
            );
            let markers = extract_technology_markers(&evidenced(&page).await);
            assert!(markers.is_empty());
        }

        #[tokio::test]
        async fn framework_like_script_path_alone_produces_no_marker() {
            let page = page_with(
                "https://example.test/",
                r#"<html><body>
                    <script src="/_next/static/chunks/main.js"></script>
                    <script src="/wp-includes/js/wp-embed.min.js"></script>
                </body></html>"#,
                "text/html",
                reqwest::StatusCode::OK,
                &[],
            );
            let markers = extract_technology_markers(&evidenced(&page).await);
            assert!(markers.is_empty());
        }
    }

    // Phase 17: discovery negative proof.
    mod discovery_negative_proof {
        use super::*;

        #[tokio::test]
        async fn evidenced_page_facts_construction_requires_a_real_acquired_page_not_a_search_result(
        ) {
            // There is no `From<SearchResult>` / `From<SearchResults>`
            // for `PageFacts` or `EvidencedPageFacts` anywhere in this
            // module, and — since
            // SCORPION_AUDIT_EXACT_PAGE_EVIDENCE_BINDING_CORRECTION_001 —
            // no public constructor accepts a `PageFacts`/`EvidenceRef`
            // pair at all. The sole production seam,
            // `EvidencedPageFacts::record`, takes `&DomainPersistence`
            // and `&Page` — a `SearchResult`/`SearchResults` value has
            // neither shape and cannot be substituted for either
            // parameter. This test proves the seam actually works end to
            // end from a real acquired `Page`, the only input it accepts.
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let page = page_with_html("https://example.test/", "<html></html>");
            let evidenced = EvidencedPageFacts::record(&store, &page).await.unwrap();
            assert_eq!(evidenced.facts().requested_url(), page.get_url());
        }
    }
}
