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
//! `SCORPION_AUDIT_FACTS_AND_FINDING_CONTRACT_FRONTIER_001`. This is the
//! foundation the queued `SCORPION_AUDIT_DETERMINISTIC_PAGE_ANALYZERS_001`
//! frontier will build many more rules on top of — deliberately not that
//! frontier. Exactly one production rule exists here:
//! [`SEO_CANONICAL_MISSING_RULE_ID`].
//!
//! # Truth chain
//!
//! `ACQUIRED -> OBSERVED -> EVIDENCED -> DERIVED FINDING`. There is no
//! fourth canonical stage named "ANALYZED": [`Finding`] is a derived
//! record that *references* evidence by [`EvidenceRef`] — it is never
//! itself `Evidence`, and it can never be recorded, read back, or
//! serialized as an [`EvidenceBundle`].
//!
//! # Fetch once, observe once, evidence once, analyze many
//!
//! No analyzer in this module performs network acquisition — that
//! remains [`fetch_single_page`]'s sole responsibility (the exact
//! one-shot primitive every other evidence-first caller in this crate
//! already uses; see [`audit_seo_canonical_missing`], the only function
//! here that touches the network). [`PageFacts::from_page`] is a pure,
//! synchronous, network-free projection of an already-acquired [`Page`]:
//! it does not call [`Website`](crate::website::Website), `reqwest`, or
//! any search provider.
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
//! # Scope firewall (this frontier only)
//!
//! Not implemented here: any SEO rule beyond `SEO_CANONICAL_MISSING`
//! (meta description, hreflang, headings, image-alt, structured data,
//! sitemap, duplicate-content, broken-link); any security rule (CSP/HSTS
//! evaluation, cookie rules, CORS, mixed content, form security); any
//! technology/CMS/framework/WAF fingerprinting; any site-wide analytics
//! (duplicate-title aggregation, orphan detection, canonical loops,
//! redirect graphs, sitemap drift); any network/Nmap capability
//! (`NetworkObservation`, port scanning, service detection, process
//! execution, target admission policy); any CLI/API/MCP/Web Console
//! surface; and no AI (summarization, severity generation, report
//! generation). Severity here is fixed rule policy, never observed
//! evidence and never AI-generated narrative.

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

/// Stable semantic identity of this frontier's sole production rule.
/// Follows the `<category>.<check>` naming Phase 21 of the authorizing
/// frontier specified.
pub const SEO_CANONICAL_MISSING_RULE_ID: &str = "seo.canonical.missing";

/// Deterministic version of [`SEO_CANONICAL_MISSING_RULE_ID`]'s
/// predicate. Independent of crate/package version — a future behavior
/// change to the predicate (e.g. treating a self-referential canonical
/// differently) must bump this, never `Cargo.toml`'s version.
pub const SEO_CANONICAL_MISSING_RULE_VERSION: u32 = 1;

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
    canonical_links: Vec<String>,
    response_headers: BTreeMap<String, Vec<Vec<u8>>>,
}

impl PageFacts {
    /// Derive facts from `page`, and only `page` — no network, no
    /// persistence, no AI, no call into `Website`/`reqwest`/any search
    /// provider.
    pub fn from_page(page: &Page) -> Self {
        let provenance = page_provenance(page);
        let response_headers = page
            .headers
            .as_ref()
            .map(audit_response_headers)
            .unwrap_or_default();
        Self {
            requested_url: page.get_url().to_string(),
            final_url: page.get_url_final().to_string(),
            effective_status: page.status_code.as_u16(),
            observed_status: provenance.observed_status_code,
            canonical_links: extract_canonical_links(&page.get_html()),
            response_headers,
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

    /// Every `<link rel="canonical" href="...">` observation truthfully
    /// extracted from the acquired document, in document order. Search
    /// title/snippet/provider score, discovery metadata, an HTTP `Link`
    /// header, and an OpenGraph URL are never treated as an HTML
    /// canonical element — see [`extract_canonical_links`].
    pub fn canonical_links(&self) -> &[String] {
        &self.canonical_links
    }

    /// Every observed value of each closed-allowlist audit-relevant
    /// response header — see
    /// [`audit_response_headers`](crate::utils::evidence::audit_response_headers).
    pub fn response_headers(&self) -> &BTreeMap<String, Vec<Vec<u8>>> {
        &self.response_headers
    }
}

/// Truthfully extract every `<link rel="canonical" href="...">`
/// observation from `html`, in document order. Reuses this crate's
/// existing `lol_html` infrastructure (the same synchronous,
/// side-effecting `element!` handler pattern
/// `crate::utils::clean_html_base` already uses) — no new HTML parser
/// dependency. Only the real HTML canonical-link element counts: a
/// search result's title/snippet, discovery metadata, an HTTP `Link`
/// header, and an OpenGraph URL are structurally different data this
/// function never even sees, let alone accepts as a substitute.
fn extract_canonical_links(html: &str) -> Vec<String> {
    use lol_html::{element, rewrite_str, RewriteStrSettings};
    use std::cell::RefCell;

    let found = RefCell::new(Vec::new());
    // catch_unwind guards against lol_html's internal panic on malformed
    // encodings, exactly like `clean_html_base` — a page whose HTML
    // cannot be safely rewritten yields zero canonical-link observations
    // rather than propagating a panic into the audit seam.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rewrite_str(
            html,
            RewriteStrSettings {
                element_content_handlers: vec![element!("link[rel=\"canonical\"]", |el| {
                    if let Some(href) = el.get_attribute("href") {
                        found.borrow_mut().push(href);
                    }
                    Ok(())
                })],
                ..RewriteStrSettings::default()
            },
        )
    }));
    found.into_inner()
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

/// Which product area a [`Finding`] belongs to. Exactly one variant has
/// a real production rule in this frontier: [`FindingCategory::Seo`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FindingCategory {
    /// Search-engine-optimization observations.
    Seo,
}

impl FindingCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::Seo => "seo",
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

/// A structurally typed observed or expected fact a rule's predicate
/// compares — never a free-form narrative string like `"SEO is bad"`.
/// Only the variants `SEO_CANONICAL_MISSING` actually needs exist today;
/// successor rules add their own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FindingCondition {
    /// The exact number of `<link rel="canonical">` observations found.
    CanonicalLinkCount(usize),
    /// The minimum number of `<link rel="canonical">` observations
    /// required.
    CanonicalLinkCountAtLeast(usize),
}

impl FindingCondition {
    /// Stable, explicit wire representation used only for
    /// [`FindingId`] derivation — never `{:?}` (`Debug` formatting is
    /// not a serialization contract).
    fn identity_repr(&self) -> String {
        match self {
            Self::CanonicalLinkCount(n) => format!("canonical_link_count={n}"),
            Self::CanonicalLinkCountAtLeast(n) => format!("canonical_link_count>={n}"),
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

/// The sole production rule this frontier proves. `Some(Finding)` when
/// `evidenced`'s page carries zero `<link rel="canonical">`
/// observations, `None` when at least one is present. No severity
/// gradient, no conflict/loop detection, no normalization policy —
/// those belong to successor rules.
pub fn seo_canonical_missing(evidenced: &EvidencedPageFacts) -> Option<Finding> {
    if !evidenced.facts.canonical_links.is_empty() {
        return None;
    }
    Some(
        Finding::new(
            SEO_CANONICAL_MISSING_RULE_ID,
            SEO_CANONICAL_MISSING_RULE_VERSION,
            FindingCategory::Seo,
            FindingSeverity::Medium,
            evidenced.facts.final_url.clone(),
            FindingCondition::CanonicalLinkCount(0),
            FindingCondition::CanonicalLinkCountAtLeast(1),
            vec![evidenced.evidence_ref],
        )
        .expect("EvidencedPageFacts always carries exactly one EvidenceRef"),
    )
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

/// The internal canonical audit execution seam:
/// acquire exactly one page (through [`fetch_single_page`], the same
/// one-shot primitive every other evidence-first caller uses) -> record
/// its evidence -> derive [`PageFacts`] from that *exact same* `Page` ->
/// run [`seo_canonical_missing`] -> persist any resulting [`Finding`].
/// This is not a CLI/API/MCP/Web Console surface — see this module's doc
/// comment.
pub async fn audit_seo_canonical_missing(
    store: &DomainPersistence,
    url: &str,
) -> Result<Option<Finding>, AuditError> {
    let page = fetch_single_page(url)
        .await
        .map_err(AuditError::Acquisition)?;

    let evidenced = EvidencedPageFacts::record(store, &page).await?;

    match seo_canonical_missing(&evidenced) {
        Some(finding) => Ok(Some(record_finding(store, finding).await?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::build;
    use crate::utils::PageResponse;

    fn page_with_html(url: &str, html: &str) -> Page {
        build(
            url,
            PageResponse {
                content: Some(html.as_bytes().to_vec()),
                status_code: reqwest::StatusCode::OK,
                ..Default::default()
            },
        )
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
                extract_canonical_links(&content).is_empty(),
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

    mod canonical_extraction {
        use super::*;

        #[test]
        fn extracts_a_present_canonical_link() {
            let html = r#"<html><head><link rel="canonical" href="https://example.test/product"></head></html>"#;
            assert_eq!(
                extract_canonical_links(html),
                vec!["https://example.test/product".to_string()]
            );
        }

        #[test]
        fn no_canonical_link_yields_empty() {
            let html = "<html><head><title>Example</title></head><body>hello</body></html>";
            assert!(extract_canonical_links(html).is_empty());
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
            assert!(extract_canonical_links(html).is_empty());
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
                2,
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
            // in a successful, non-erroring seam call).
            let finding = audit_seo_canonical_missing(&store, &url)
                .await
                .unwrap()
                .expect("this fixture has no canonical link");

            // 9: read Finding back by FindingId.
            let id = finding.id();
            let read_back = read_finding(&store, &id).await.unwrap().unwrap();
            assert_eq!(read_back, finding);

            // 10-12: resolve every Finding EvidenceRef, independently
            // confirm it names the acquired page and that the persisted
            // page material lacks a canonical link.
            for reference in read_back.evidence() {
                let bundle = reference
                    .resolve(&store)
                    .await
                    .unwrap()
                    .expect("evidence must resolve");
                assert_eq!(bundle.requested_url.as_deref(), Some(url.as_str()));
                let content = bundle.content.expect("acquired page content persisted");
                assert!(extract_canonical_links(&content).is_empty());
            }

            assert_eq!(finding.rule_id(), SEO_CANONICAL_MISSING_RULE_ID);
        }

        #[tokio::test]
        async fn full_chain_with_a_canonical_link_produces_no_finding() {
            let store = DomainPersistence::open_in_memory().await.unwrap();
            let url = fixture_server(
                r#"<html><head><link rel="canonical" href="https://example.test/product"></head></html>"#,
            );

            let finding = audit_seo_canonical_missing(&store, &url).await.unwrap();
            assert!(finding.is_none());
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
