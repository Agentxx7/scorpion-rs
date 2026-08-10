//! `ResearchScope`: the smallest canonical declarative discovery-scope
//! boundary, plus the orchestration seam that normalizes it — together
//! with already-acquired discovery material — into ordered [`SourceItem`]
//! candidates.
//!
//! **The domain boundary this module enforces:**
//!
//! ```text
//! ResearchScope                        already-acquired discovery material
//! (declarative scope/seeds only)        (fetched bytes, from an out-of-
//!         │                              scope acquisition step)
//!         │                                       │
//!         └──────────────┬────────────────────────┘
//!                         ▼
//!                 DiscoveryInput (orchestration's working unit —
//!                                 never stored inside ResearchScope)
//!                         │
//!                         ▼
//!                    discover(..)
//!                         │
//!                         ▼
//!                 SourceItem candidates
//!                         │
//!                         ▼
//!                       [STOP]
//! ```
//!
//! [`ResearchScope`] is **declarative input, not fetched state**: it can
//! only ever hold a [`ScopeSeed`] — a manual onion seed URL (a bare
//! `String`) or an already-produced candidate ([`SourceItem`]). Neither
//! variant can carry a byte buffer; `ResearchScope` is structurally
//! incapable of holding fetched document bytes (see
//! `research_scope_seeds_cannot_hold_fetched_document_bytes` in this
//! module's tests for the compile-time-enforced proof).
//!
//! **Already-acquired discovery material** — fetched feed/sitemap/News
//! Sitemap document bytes the caller retrieved through some acquisition
//! step this module has nothing to do with — is a structurally distinct
//! type, [`DiscoveryMaterial`]. It is never part of `ResearchScope`.
//!
//! [`discover`] is the one orchestration seam that normalizes a mixed,
//! caller-ordered list of [`DiscoveryInput`] (each either a
//! [`ScopeSeed`] or parser-neutral [`DiscoveryMaterial`] paired with an
//! explicit [`DiscoveryParserIntent`]) into [`SourceItem`]
//! candidates. **Zero acquisition occurs here or anywhere in this
//! module**: every byte/string a [`DiscoveryInput`] carries was already
//! supplied by the caller. This module has no HTTP client, no Tor/SOCKS,
//! no DNS, no socket, no filesystem access, and constructs no
//! `Page`/`EvidenceBundle`/`TransportPolicy`. Discovery terminates in
//! candidates — binding a candidate to an actual fetch (Tor or
//! otherwise) is later, separate orchestration's job, not yet
//! implemented anywhere in this crate.
//!
//! Reuses every existing canonical adapter exactly as it stands — no
//! parsing logic is duplicated here:
//! [`crate::features::onion_seed::normalize_onion_seed`] for manual/
//! request-supplied onion seeds, [`crate::features::feed::parse`],
//! [`crate::features::sitemap::parse`], and
//! [`crate::features::news_sitemap::parse`] for already-fetched documents
//! (all three are pure CPU-bound parsing over caller-supplied bytes —
//! `spawn_blocking`, never network I/O, per their own module docs).
//!
//! `robots.txt` `Sitemap:` discovery
//! ([`crate::features::robots_sitemap`]) is deliberately **not**
//! represented here: it produces `RobotsSitemapReference` (a *pointer* to
//! another document to independently fetch and parse), never a
//! `SourceItem` — exactly like a sitemap index's own `child_sitemaps`.
//! Folding declared-pointer vocabulary into candidate vocabulary would be
//! a new, unauthorized semantic, so this module only ever emits
//! candidates for the sitemap/feed adapters' genuine content entries
//! (`SitemapDiscoveryResult::entries`, never `::child_sitemaps`).

use crate::features::source::SourceItem;

/// One declarative scope seed — the only thing [`ResearchScope`] can
/// hold. Never fetched document bytes: a bare onion seed URL, or an
/// already-produced candidate. Constructing a [`ScopeSeed`] performs no
/// work.
#[derive(Debug, Clone)]
pub enum ScopeSeed {
    /// A manually/request-supplied onion seed URL. Normalized, when
    /// [`discover`] runs, via the single canonical manual-URL discovery
    /// seam, [`crate::features::onion_seed::normalize_onion_seed`] — this
    /// variant itself does no classification or validation; it is just
    /// the caller's declared string. This variant carries no policy
    /// about which onion targets a research session is *permitted* to
    /// include — that is later, separate orchestration's job.
    OnionSeed(String),
    /// An already-produced candidate, included verbatim when
    /// [`discover`] runs — no re-validation, no re-derivation, and
    /// critically, **no re-normalization**: this is not a second onion
    /// seed / manual-URL classification path. A caller must never
    /// construct a raw onion or clearnet URL string as a `Candidate` to
    /// bypass `OnionSeed`'s canonical checks (credential rejection,
    /// `.onion` classification) — that classification only exists on
    /// the `OnionSeed` path. `Candidate` exists for a caller who already
    /// produced a genuine `SourceItem` through *some* canonical means
    /// (any existing adapter, including but not limited to
    /// `onion_seed`) and wants it folded into one ordered scope/result
    /// alongside other seeds/material.
    Candidate(SourceItem),
}

/// A declarative, ordered discovery scope — **never fetched state**.
/// Can only ever hold [`ScopeSeed`] values (an onion seed URL string, or
/// an already-produced candidate) — structurally incapable of holding
/// fetched document bytes. Building or mutating a `ResearchScope`
/// performs no work.
#[derive(Debug, Clone, Default)]
pub struct ResearchScope {
    seeds: Vec<ScopeSeed>,
}

impl ResearchScope {
    /// An empty scope.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one scope seed, preserving call order. Returns `&mut Self`
    /// for chaining; performs no work.
    pub fn push(&mut self, seed: ScopeSeed) -> &mut Self {
        self.seeds.push(seed);
        self
    }

    /// The scope's seeds, in the order they were pushed.
    pub fn seeds(&self) -> &[ScopeSeed] {
        &self.seeds
    }

    /// `true` when no seed has been added.
    pub fn is_empty(&self) -> bool {
        self.seeds.is_empty()
    }

    /// Number of seeds in the scope.
    pub fn len(&self) -> usize {
        self.seeds.len()
    }

    /// Convert this scope's seeds into orchestration-ready
    /// [`DiscoveryInput`] values, in scope order — the one sanctioned way
    /// to feed a `ResearchScope`'s declarative seeds into [`discover`].
    /// Consumes `self`; use `.clone()` first if the scope is still
    /// needed afterward.
    pub fn into_inputs(self) -> Vec<DiscoveryInput> {
        self.seeds.into_iter().map(DiscoveryInput::Scope).collect()
    }
}

impl FromIterator<ScopeSeed> for ResearchScope {
    fn from_iter<I: IntoIterator<Item = ScopeSeed>>(iter: I) -> Self {
        Self {
            seeds: iter.into_iter().collect(),
        }
    }
}

/// Parser-neutral already-acquired discovery material: fetched document
/// bytes the caller already retrieved through some acquisition step
/// entirely outside this module's concern, paired with the document's own
/// URL.
/// **Not part of [`ResearchScope`]** — acquisition output is a
/// structurally distinct concern from declarative scope (see the module
/// docs' domain-boundary diagram). Constructing a [`DiscoveryMaterial`]
/// performs no work; normalization happens only inside [`discover`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryMaterial {
    /// Exact bytes already retrieved by the caller.
    pub bytes: Vec<u8>,
    /// The acquired document's actual containing URL. Existing parsers use
    /// this value as `SourceItem::discovered_via`; this type never derives or
    /// rewrites it.
    pub url: String,
}

/// Explicit selection of the canonical parser that should consume a
/// [`DiscoveryMaterial`]. Parser intent is caller-supplied and independent of
/// the material's URL, bytes, acquisition transport, and target provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryParserIntent {
    /// Route to [`crate::features::feed::parse`].
    #[cfg(feature = "feed")]
    Feed,
    /// Route to [`crate::features::sitemap::parse`].
    #[cfg(feature = "sitemap")]
    Sitemap,
    /// Route to [`crate::features::news_sitemap::parse`].
    #[cfg(feature = "news_sitemap")]
    NewsSitemap,
}

/// One item for [`discover`] to normalize, in the exact order the caller
/// wants processed — a [`ScopeSeed`] (declarative) or parser-neutral
/// [`DiscoveryMaterial`] paired with an explicit [`DiscoveryParserIntent`].
/// This is the orchestration boundary's working
/// unit: it exists only as `discover`'s input shape, and is **never**
/// stored inside [`ResearchScope`] — that separation is the whole point
/// of this module's design (see the module docs' domain-boundary
/// diagram). A caller coordinates scope seeds and discovery material
/// together by building a `Vec<DiscoveryInput>` (via
/// [`ResearchScope::into_inputs`] plus explicit `Material { material, intent }`
/// values, interleaved in whatever order is wanted) and passing it to
/// [`discover`] in one call.
#[derive(Debug, Clone)]
pub enum DiscoveryInput {
    /// A declarative scope seed.
    Scope(ScopeSeed),
    /// Already-acquired discovery material plus the parser the caller
    /// explicitly selected for it.
    Material {
        /// Parser-neutral acquired payload.
        material: DiscoveryMaterial,
        /// Explicit parser selection; never inferred from `material`.
        intent: DiscoveryParserIntent,
    },
}

impl From<ScopeSeed> for DiscoveryInput {
    fn from(seed: ScopeSeed) -> Self {
        DiscoveryInput::Scope(seed)
    }
}

/// Why one [`DiscoveryInput`] failed to normalize into candidates. Each
/// variant delegates directly to the underlying canonical adapter's own
/// error type — never a re-derived or re-worded copy of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryError {
    /// See [`crate::features::onion_seed::OnionSeedError`].
    OnionSeed(crate::features::onion_seed::OnionSeedError),
    /// See [`crate::features::feed::FeedParseFailure`].
    #[cfg(feature = "feed")]
    Feed(crate::features::feed::FeedParseFailure),
    /// See [`crate::features::sitemap::SitemapParseFailure`].
    #[cfg(feature = "sitemap")]
    Sitemap(crate::features::sitemap::SitemapParseFailure),
    /// See [`crate::features::news_sitemap::NewsSitemapParseFailure`].
    #[cfg(feature = "news_sitemap")]
    NewsSitemap(crate::features::news_sitemap::NewsSitemapParseFailure),
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscoveryError::OnionSeed(error) => write!(f, "{error}"),
            #[cfg(feature = "feed")]
            DiscoveryError::Feed(error) => write!(f, "{error}"),
            #[cfg(feature = "sitemap")]
            DiscoveryError::Sitemap(error) => write!(f, "{error}"),
            #[cfg(feature = "news_sitemap")]
            DiscoveryError::NewsSitemap(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for DiscoveryError {}

/// The result of running [`discover`] over a `&[DiscoveryInput]`: one
/// `Result` per input, in the exact order supplied — a single input may
/// contribute zero, one, or many candidates (a rejected onion seed
/// contributes zero; an accepted one or a `Candidate` passthrough
/// contributes exactly one; a feed document contributes as many as it
/// has entries). Alignment between input position and outcome position
/// is always index-for-index, so a caller can always trace a failure
/// back to the exact input that produced it.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryOutcome {
    /// One entry per [`DiscoveryInput`], in the order supplied to
    /// [`discover`].
    pub per_input: Vec<Result<Vec<SourceItem>, DiscoveryError>>,
}

impl DiscoveryOutcome {
    /// Every successfully produced candidate, flattened across the whole
    /// input list in order: input order first, then (for a multi-item
    /// input) that adapter's own reported order. Duplicates are
    /// preserved exactly as produced — nothing here deduplicates,
    /// reorders, ranks, or scores.
    pub fn candidates(&self) -> Vec<&SourceItem> {
        self.per_input
            .iter()
            .filter_map(|result| result.as_ref().ok())
            .flatten()
            .collect()
    }

    /// Every failed input, paired with its original index in the input
    /// list.
    pub fn errors(&self) -> impl Iterator<Item = (usize, &DiscoveryError)> {
        self.per_input
            .iter()
            .enumerate()
            .filter_map(|(index, result)| result.as_ref().err().map(|error| (index, error)))
    }
}

/// Execute the one parser explicitly selected for already-acquired neutral
/// discovery material, returning its normalized `SourceItem` values in parser
/// order. This is the canonical parser-routing seam used by [`discover`].
///
/// Routing depends only on `intent`. The material URL, suffix, bytes, MIME,
/// hostname, target provenance, and acquisition transport are never inspected
/// to choose a parser. Each branch delegates to its existing canonical parser;
/// no parsing implementation is duplicated here.
///
/// Standard sitemap child references remain pointer output of
/// [`crate::features::sitemap::parse`] and are deliberately excluded from this
/// normalized `SourceItem` result. News Sitemap entries retain the existing
/// orchestration projection to their generic `item`; News-specific metadata is
/// available from the parser's richer result, not fabricated into SourceItem.
pub async fn parse_material(
    material: &DiscoveryMaterial,
    intent: DiscoveryParserIntent,
) -> Result<Vec<SourceItem>, DiscoveryError> {
    // Keeps the parser-neutral material parameter truthful and warning-free in
    // the no-parser-feature build, where `intent` is uninhabited.
    let _ = material;

    match intent {
        #[cfg(feature = "feed")]
        DiscoveryParserIntent::Feed => crate::features::feed::parse(&material.bytes, &material.url)
            .await
            .map(|result| result.entries)
            .map_err(DiscoveryError::Feed),
        #[cfg(feature = "sitemap")]
        DiscoveryParserIntent::Sitemap => {
            crate::features::sitemap::parse(&material.bytes, &material.url)
                .await
                .map(|result| result.entries)
                .map_err(DiscoveryError::Sitemap)
        }
        #[cfg(feature = "news_sitemap")]
        DiscoveryParserIntent::NewsSitemap => {
            crate::features::news_sitemap::parse(&material.bytes, &material.url)
                .await
                .map(|result| result.entries.into_iter().map(|entry| entry.item).collect())
                .map_err(DiscoveryError::NewsSitemap)
        }
    }
}

/// Normalize every input in `inputs`, in the exact order supplied, into
/// a [`DiscoveryOutcome`]. Reuses each canonical adapter exactly as it
/// stands — never a reimplementation. `ScopeSeed::Candidate` is a pure
/// identity passthrough: it never rewrites `source_item_id`,
/// `discovered_via`, `source_type`, or any other field, and never
/// re-runs onion/manual-URL classification (only `ScopeSeed::OnionSeed`
/// does that, via [`crate::features::onion_seed::normalize_onion_seed`]).
///
/// Performs **zero acquisition**: every input already carries the exact
/// bytes/string it needs; this function only parses/classifies what was
/// handed to it. See the module docs for the full domain boundary.
pub async fn discover(inputs: &[DiscoveryInput]) -> DiscoveryOutcome {
    let mut per_input = Vec::with_capacity(inputs.len());

    for input in inputs {
        let result: Result<Vec<SourceItem>, DiscoveryError> = match input {
            DiscoveryInput::Scope(ScopeSeed::OnionSeed(seed)) => {
                crate::features::onion_seed::normalize_onion_seed(seed)
                    .map(|item| vec![item])
                    .map_err(DiscoveryError::OnionSeed)
            }
            DiscoveryInput::Scope(ScopeSeed::Candidate(item)) => Ok(vec![item.clone()]),
            DiscoveryInput::Material { material, intent } => {
                parse_material(material, *intent).await
            }
        };
        per_input.push(result);
    }

    DiscoveryOutcome { per_input }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn onion(seed: &str) -> DiscoveryInput {
        DiscoveryInput::Scope(ScopeSeed::OnionSeed(seed.to_string()))
    }

    fn candidate_item(url: &str, source_type: &str) -> SourceItem {
        SourceItem {
            source_type: source_type.to_string(),
            url: Some(url.to_string()),
            ..Default::default()
        }
    }

    fn candidate_input(url: &str, source_type: &str) -> DiscoveryInput {
        DiscoveryInput::Scope(ScopeSeed::Candidate(candidate_item(url, source_type)))
    }

    /// Compile-enforced shape proof: acquired material is exactly bytes plus
    /// its containing URL. Parser classification and target provenance can
    /// only be supplied separately on `DiscoveryInput::Material`.
    #[test]
    fn discovery_material_is_parser_neutral_bytes_and_url_only() {
        let material = DiscoveryMaterial {
            bytes: vec![0, 1, 2, 255],
            url: "https://example.test/document.bin".to_string(),
        };
        let DiscoveryMaterial { bytes, url } = material;
        assert_eq!(bytes, vec![0, 1, 2, 255]);
        assert_eq!(url, "https://example.test/document.bin");
    }

    /// Compile-time signature proof: the parser seam accepts only neutral
    /// material plus explicit intent and returns the existing normalized error
    /// vocabulary. No Page, TransportAcquisition, TransportPolicy,
    /// DiscoveryTargetKind, or acquisition binding state can enter it.
    #[test]
    fn parse_material_signature_is_the_structural_parser_boundary() {
        fn type_check<'a>(
            material: &'a DiscoveryMaterial,
            intent: DiscoveryParserIntent,
        ) -> impl std::future::Future<Output = Result<Vec<SourceItem>, DiscoveryError>> + 'a
        {
            parse_material(material, intent)
        }

        let _ = type_check;
    }

    #[cfg(feature = "feed")]
    #[tokio::test]
    async fn direct_parser_seam_routes_feed_intent_and_preserves_url() {
        const RSS: &str = r#"<rss version="2.0"><channel><title>T</title><item><guid>one</guid><link>https://example.test/a</link><title>A</title></item><item><guid>one</guid><link>https://example.test/a</link><title>A</title></item></channel></rss>"#;
        let material = DiscoveryMaterial {
            bytes: RSS.as_bytes().to_vec(),
            url: "https://example.test/misleading.sitemap.xml".to_string(),
        };

        let items = parse_material(&material, DiscoveryParserIntent::Feed)
            .await
            .unwrap();

        assert_eq!(items.len(), 2);
        assert_eq!(items[0], items[1], "parser-produced duplicates survive");
        assert!(items.iter().all(|item| item.source_type == "feed"));
        assert!(items.iter().all(|item| {
            item.discovered_via.as_deref() == Some("https://example.test/misleading.sitemap.xml")
        }));
    }

    #[cfg(feature = "sitemap")]
    #[tokio::test]
    async fn direct_parser_seam_returns_sitemap_entries_not_child_pointers() {
        const INDEX: &str = r#"<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><sitemap><loc>https://example.test/child.xml</loc></sitemap></sitemapindex>"#;
        let material = DiscoveryMaterial {
            bytes: INDEX.as_bytes().to_vec(),
            url: "https://example.test/misleading.rss".to_string(),
        };

        let items = parse_material(&material, DiscoveryParserIntent::Sitemap)
            .await
            .unwrap();

        assert!(
            items.is_empty(),
            "child sitemap pointers are not SourceItems"
        );
    }

    #[cfg(feature = "news_sitemap")]
    #[tokio::test]
    async fn direct_parser_seam_routes_news_intent_to_existing_item_projection() {
        const NEWS: &str = r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:news="http://www.google.com/schemas/sitemap-news/0.9"><url><loc>https://example.test/a</loc><news:news><news:publication><news:name>N</news:name><news:language>en</news:language></news:publication><news:publication_date>2026-01-01</news:publication_date><news:title>A</news:title></news:news></url></urlset>"#;
        let material = DiscoveryMaterial {
            bytes: NEWS.as_bytes().to_vec(),
            url: "https://example.test/no-extension".to_string(),
        };

        let items = parse_material(&material, DiscoveryParserIntent::NewsSitemap)
            .await
            .unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].source_type, "sitemap");
        assert_eq!(items[0].title.as_deref(), Some("A"));
        assert_eq!(
            items[0].discovered_via.as_deref(),
            Some("https://example.test/no-extension")
        );
    }

    #[cfg(all(feature = "feed", feature = "sitemap"))]
    #[tokio::test]
    async fn direct_parser_seam_uses_only_explicit_intent_and_existing_errors() {
        let material = DiscoveryMaterial {
            bytes: b"not a supported document".to_vec(),
            url: "https://example.test/feed.xml".to_string(),
        };

        let feed_error = parse_material(&material, DiscoveryParserIntent::Feed)
            .await
            .unwrap_err();
        let sitemap_error = parse_material(&material, DiscoveryParserIntent::Sitemap)
            .await
            .unwrap_err();

        assert!(matches!(feed_error, DiscoveryError::Feed(_)));
        assert!(matches!(sitemap_error, DiscoveryError::Sitemap(_)));
        assert_eq!(material.bytes, b"not a supported document");
        assert_eq!(material.url, "https://example.test/feed.xml");
    }

    /// Structural proof (CRITICAL, per operator review): `ResearchScope`
    /// cannot hold fetched document bytes. `ScopeSeed` is matched here
    /// exhaustively — no `_` wildcard arm — over exactly two variants,
    /// `OnionSeed(String)` and `Candidate(SourceItem)`; neither carries a
    /// byte buffer. If a future edit ever added a byte-carrying variant
    /// to `ScopeSeed` (e.g. attempting to smuggle fetched bytes into
    /// declarative scope), this match stops compiling until it is
    /// explicitly updated here — the boundary is enforced by the type
    /// checker, not just documented.
    #[test]
    fn research_scope_seeds_cannot_hold_fetched_document_bytes() {
        fn seed_carries_no_byte_buffer(seed: &ScopeSeed) -> bool {
            match seed {
                ScopeSeed::OnionSeed(url) => {
                    let _: &String = url; // a URL string, never `Vec<u8>`.
                    true
                }
                ScopeSeed::Candidate(item) => {
                    let _: &SourceItem = item; // already-normalized, never raw bytes.
                    true
                }
            }
        }

        let mut scope = ResearchScope::new();
        scope
            .push(ScopeSeed::OnionSeed("http://abc.onion/".to_string()))
            .push(ScopeSeed::Candidate(SourceItem::default()));
        assert_eq!(scope.len(), 2);
        for seed in scope.seeds() {
            assert!(seed_carries_no_byte_buffer(seed));
        }
    }

    /// 1. Empty scope/inputs produce an empty, error-free outcome.
    #[tokio::test]
    async fn empty_scope_produces_empty_outcome() {
        let scope = ResearchScope::new();
        assert!(scope.is_empty());
        assert_eq!(scope.len(), 0);
        let inputs = scope.into_inputs();
        assert!(inputs.is_empty());
        let outcome = discover(&inputs).await;
        assert!(outcome.candidates().is_empty());
        assert_eq!(outcome.errors().count(), 0);
        assert!(outcome.per_input.is_empty());
    }

    /// 2. One input class (onion seed) normalizes correctly.
    #[tokio::test]
    async fn one_input_class_onion_seed_normalizes() {
        let inputs = vec![onion("http://abc.onion/page")];
        let outcome = discover(&inputs).await;
        let candidates = outcome.candidates();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].url.as_deref(), Some("http://abc.onion/page"));
        assert_eq!(candidates[0].source_type, "onion_seed");
        assert_eq!(candidates[0].discovered_via, None);
        assert_eq!(candidates[0].source_item_id, None);
    }

    /// 2b. One input class (direct candidate passthrough) is included
    /// verbatim, unmodified — see `candidate_passthrough_never_rewrites_fields`
    /// below for the exhaustive per-field proof.
    #[tokio::test]
    async fn one_input_class_candidate_passthrough_unmodified() {
        let item = candidate_item("https://example.test/x", "custom");
        let inputs = vec![DiscoveryInput::Scope(ScopeSeed::Candidate(item.clone()))];
        let outcome = discover(&inputs).await;
        let candidates = outcome.candidates();
        assert_eq!(candidates.len(), 1);
        assert_eq!(*candidates[0], item);
    }

    /// F (operator review): `Candidate` passthrough must not silently
    /// rewrite `source_item_id`, `discovered_via`, `source_type`, or any
    /// other field — proven per-field, with values chosen so any
    /// accidental rewrite (e.g. defaulting `discovered_via` to `None`,
    /// or fabricating a `source_item_id`) would fail this test.
    #[tokio::test]
    async fn candidate_passthrough_never_rewrites_fields() {
        let item = SourceItem {
            source_type: "custom_adapter".to_string(),
            source_item_id: Some("native-id-123".to_string()),
            url: Some("https://example.test/article".to_string()),
            title: Some("Title".to_string()),
            snippet: Some("Snippet".to_string()),
            authors: vec!["Author One".to_string()],
            published_at: Some(1000),
            updated_at: Some(2000),
            discovered_via: Some("https://example.test/index".to_string()),
            media_references: Vec::new(),
        };
        let inputs = vec![DiscoveryInput::Scope(ScopeSeed::Candidate(item.clone()))];
        let outcome = discover(&inputs).await;
        let produced = outcome.candidates()[0];
        assert_eq!(produced.source_type, item.source_type);
        assert_eq!(produced.source_item_id, item.source_item_id);
        assert_eq!(produced.url, item.url);
        assert_eq!(produced.title, item.title);
        assert_eq!(produced.snippet, item.snippet);
        assert_eq!(produced.authors, item.authors);
        assert_eq!(produced.published_at, item.published_at);
        assert_eq!(produced.updated_at, item.updated_at);
        assert_eq!(produced.discovered_via, item.discovered_via);
        assert_eq!(
            *produced, item,
            "no field may differ from the supplied candidate"
        );
    }

    /// F: `Candidate` never re-runs onion/manual-URL classification — a
    /// `SourceItem` whose `url` looks like something `onion_seed` would
    /// reject (credentials, non-onion clearnet host) still passes
    /// through unchanged, because `Candidate` trusts the caller already
    /// normalized it through whatever canonical means was appropriate.
    /// This is the intended, documented trust boundary, not a bug — the
    /// rejection logic lives exclusively on the `OnionSeed` path.
    #[tokio::test]
    async fn candidate_passthrough_does_not_reapply_onion_classification() {
        let item = SourceItem {
            source_type: "custom_adapter".to_string(),
            url: Some("http://user:pass@clearnet-example.test/".to_string()),
            ..Default::default()
        };
        let inputs = vec![DiscoveryInput::Scope(ScopeSeed::Candidate(item.clone()))];
        let outcome = discover(&inputs).await;
        assert_eq!(outcome.per_input.len(), 1);
        assert!(
            outcome.per_input[0].is_ok(),
            "Candidate must never be rejected by onion_seed rules"
        );
        assert_eq!(*outcome.candidates()[0], item);
    }

    /// 3. Multiple input classes (scope + material) combine into one
    /// ordered outcome via a single `discover` call.
    #[cfg(feature = "feed")]
    #[tokio::test]
    async fn multiple_input_classes_combine() {
        const RSS: &str = r#"<rss version="2.0"><channel><title>T</title><item><guid>one</guid><link>https://example.test/a</link><title>A</title></item></channel></rss>"#;
        let inputs = vec![
            onion("http://abc.onion/"),
            DiscoveryInput::Material {
                material: DiscoveryMaterial {
                    bytes: RSS.as_bytes().to_vec(),
                    url: "https://example.test/feed.xml".to_string(),
                },
                intent: DiscoveryParserIntent::Feed,
            },
            candidate_input("https://example.test/manual", "custom"),
        ];
        let outcome = discover(&inputs).await;
        let candidates = outcome.candidates();
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].source_type, "onion_seed");
        assert_eq!(candidates[1].source_type, "feed");
        assert_eq!(
            candidates[1].discovered_via.as_deref(),
            Some("https://example.test/feed.xml")
        );
        assert_eq!(candidates[2].source_type, "custom");
    }

    /// Explicit Feed intent is authoritative even when the containing URL
    /// has a deliberately misleading sitemap suffix.
    #[cfg(feature = "feed")]
    #[tokio::test]
    async fn feed_intent_alone_routes_neutral_material_to_feed_parser() {
        const RSS: &str = r#"<rss version="2.0"><channel><title>T</title><item><guid>one</guid><link>https://example.test/a</link><title>A</title></item></channel></rss>"#;
        let inputs = vec![DiscoveryInput::Material {
            material: DiscoveryMaterial {
                bytes: RSS.as_bytes().to_vec(),
                url: "https://example.test/not-a-feed.sitemap.xml".to_string(),
            },
            intent: DiscoveryParserIntent::Feed,
        }];
        let outcome = discover(&inputs).await;
        assert!(outcome.per_input[0].is_ok());
        let candidate = outcome.candidates()[0];
        assert_eq!(candidate.source_type, "feed");
        assert_eq!(
            candidate.discovered_via.as_deref(),
            Some("https://example.test/not-a-feed.sitemap.xml")
        );
    }

    /// Explicit Sitemap intent is authoritative even when the containing URL
    /// has a deliberately misleading feed suffix.
    #[cfg(feature = "sitemap")]
    #[tokio::test]
    async fn sitemap_intent_alone_routes_neutral_material_to_sitemap_parser() {
        const SITEMAP: &str = r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><url><loc>https://example.test/a</loc></url></urlset>"#;
        let inputs = vec![DiscoveryInput::Material {
            material: DiscoveryMaterial {
                bytes: SITEMAP.as_bytes().to_vec(),
                url: "https://example.test/not-a-sitemap.rss".to_string(),
            },
            intent: DiscoveryParserIntent::Sitemap,
        }];
        let outcome = discover(&inputs).await;
        assert!(outcome.per_input[0].is_ok());
        let candidate = outcome.candidates()[0];
        assert_eq!(candidate.source_type, "sitemap");
        assert_eq!(
            candidate.discovered_via.as_deref(),
            Some("https://example.test/not-a-sitemap.rss")
        );
    }

    /// Explicit News Sitemap intent routes to the News parser without
    /// consulting URL suffix, target provenance, or any classifier.
    #[cfg(feature = "news_sitemap")]
    #[tokio::test]
    async fn news_sitemap_intent_alone_routes_neutral_material_to_news_parser() {
        const NEWS: &str = r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:news="http://www.google.com/schemas/sitemap-news/0.9"><url><loc>https://example.test/a</loc><news:news><news:publication><news:name>N</news:name><news:language>en</news:language></news:publication><news:publication_date>2026-01-01</news:publication_date><news:title>A</news:title></news:news></url></urlset>"#;
        let inputs = vec![DiscoveryInput::Material {
            material: DiscoveryMaterial {
                bytes: NEWS.as_bytes().to_vec(),
                url: "https://example.test/no-extension".to_string(),
            },
            intent: DiscoveryParserIntent::NewsSitemap,
        }];
        let outcome = discover(&inputs).await;
        assert!(outcome.per_input[0].is_ok());
        let candidate = outcome.candidates()[0];
        // News Sitemap intentionally reuses the generic "sitemap" source
        // type, but only its parser maps the namespaced news title.
        assert_eq!(candidate.source_type, "sitemap");
        assert_eq!(candidate.title.as_deref(), Some("A"));
        assert_eq!(
            candidate.discovered_via.as_deref(),
            Some("https://example.test/no-extension")
        );
    }

    /// The same neutral payload can be paired with distinct intents. The
    /// payload remains unchanged; only the explicit enum controls which
    /// typed parser error is returned.
    #[cfg(all(feature = "feed", feature = "sitemap", feature = "news_sitemap"))]
    #[tokio::test]
    async fn same_material_supports_independent_explicit_parser_intents() {
        let material = DiscoveryMaterial {
            bytes: b"deliberately invalid for every parser".to_vec(),
            url: "https://example.test/ambiguous".to_string(),
        };
        let inputs = vec![
            DiscoveryInput::Material {
                material: material.clone(),
                intent: DiscoveryParserIntent::Feed,
            },
            DiscoveryInput::Material {
                material: material.clone(),
                intent: DiscoveryParserIntent::Sitemap,
            },
            DiscoveryInput::Material {
                material: material.clone(),
                intent: DiscoveryParserIntent::NewsSitemap,
            },
        ];
        let outcome = discover(&inputs).await;
        assert!(matches!(outcome.per_input[0], Err(DiscoveryError::Feed(_))));
        assert!(matches!(
            outcome.per_input[1],
            Err(DiscoveryError::Sitemap(_))
        ));
        assert!(matches!(
            outcome.per_input[2],
            Err(DiscoveryError::NewsSitemap(_))
        ));
        assert_eq!(material.bytes, b"deliberately invalid for every parser");
        assert_eq!(material.url, "https://example.test/ambiguous");
    }

    /// 4. Ordering: candidates preserve caller-supplied order across
    /// scope/material input classes, and within a multi-item input, that
    /// adapter's own order.
    #[cfg(feature = "sitemap")]
    #[tokio::test]
    async fn ordering_is_preserved_across_and_within_inputs() {
        const SITEMAP: &str = r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><url><loc>https://example.test/1</loc></url><url><loc>https://example.test/2</loc></url></urlset>"#;
        let inputs = vec![
            onion("http://z.onion/"),
            DiscoveryInput::Material {
                material: DiscoveryMaterial {
                    bytes: SITEMAP.as_bytes().to_vec(),
                    url: "https://example.test/sitemap.xml".to_string(),
                },
                intent: DiscoveryParserIntent::Sitemap,
            },
            onion("http://a.onion/"),
        ];
        let outcome = discover(&inputs).await;
        let candidates = outcome.candidates();
        assert_eq!(candidates.len(), 4);
        assert_eq!(candidates[0].url.as_deref(), Some("http://z.onion/"));
        assert_eq!(candidates[1].url.as_deref(), Some("https://example.test/1"));
        assert_eq!(candidates[2].url.as_deref(), Some("https://example.test/2"));
        assert_eq!(candidates[3].url.as_deref(), Some("http://a.onion/"));
    }

    /// 5. Duplicate inputs (and the candidates they produce) are
    /// preserved verbatim — no deduplication, no invented ranking.
    #[tokio::test]
    async fn duplicate_inputs_and_candidates_are_preserved() {
        let inputs = vec![
            onion("http://a.onion/"),
            onion("http://a.onion/"),
            onion("http://a.onion/"),
        ];
        let outcome = discover(&inputs).await;
        let candidates = outcome.candidates();
        assert_eq!(candidates.len(), 3);
        for candidate in candidates {
            assert_eq!(candidate.url.as_deref(), Some("http://a.onion/"));
        }
    }

    /// Duplicate acquired-material inputs remain distinct and produce their
    /// candidates independently; orchestration performs no deduplication.
    #[cfg(feature = "feed")]
    #[tokio::test]
    async fn duplicate_material_inputs_are_preserved() {
        const RSS: &str = r#"<rss version="2.0"><channel><title>T</title><item><guid>one</guid><link>https://example.test/a</link><title>A</title></item></channel></rss>"#;
        let input = DiscoveryInput::Material {
            material: DiscoveryMaterial {
                bytes: RSS.as_bytes().to_vec(),
                url: "https://example.test/feed".to_string(),
            },
            intent: DiscoveryParserIntent::Feed,
        };
        let outcome = discover(&[input.clone(), input]).await;
        assert_eq!(outcome.per_input.len(), 2);
        assert!(outcome.per_input.iter().all(Result::is_ok));
        assert_eq!(outcome.candidates().len(), 2);
        assert_eq!(outcome.candidates()[0], outcome.candidates()[1]);
    }

    /// A parser failure occupies only its own input index and never discards
    /// successful neighboring inputs.
    #[cfg(feature = "sitemap")]
    #[tokio::test]
    async fn parser_failure_is_index_aligned_and_partial_success_survives() {
        let inputs = vec![
            onion("http://a.onion/"),
            DiscoveryInput::Material {
                material: DiscoveryMaterial {
                    bytes: b"not a sitemap".to_vec(),
                    url: "https://example.test/container".to_string(),
                },
                intent: DiscoveryParserIntent::Sitemap,
            },
            onion("http://b.onion/"),
        ];
        let outcome = discover(&inputs).await;
        assert_eq!(outcome.per_input.len(), 3);
        assert!(outcome.per_input[0].is_ok());
        assert!(matches!(
            outcome.per_input[1],
            Err(DiscoveryError::Sitemap(_))
        ));
        assert!(outcome.per_input[2].is_ok());
        assert_eq!(
            outcome.errors().map(|(index, _)| index).collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(outcome.candidates().len(), 2);
    }

    /// 6. Invalid input reports truthfully, aligned to its input-list
    /// index, and does not discard other inputs' candidates.
    #[tokio::test]
    async fn invalid_input_alignment_and_error_semantics() {
        let inputs = vec![
            onion("http://a.onion/"),      // index 0: ok
            onion("not a url"),            // index 1: InvalidUrl
            onion("https://example.com/"), // index 2: NotOnion
            onion("http://b.onion/"),      // index 3: ok
        ];
        let outcome = discover(&inputs).await;
        assert_eq!(outcome.per_input.len(), 4);
        assert!(outcome.per_input[0].is_ok());
        assert_eq!(
            outcome.per_input[1],
            Err(DiscoveryError::OnionSeed(
                crate::features::onion_seed::OnionSeedError::InvalidUrl
            ))
        );
        assert_eq!(
            outcome.per_input[2],
            Err(DiscoveryError::OnionSeed(
                crate::features::onion_seed::OnionSeedError::NotOnion
            ))
        );
        assert!(outcome.per_input[3].is_ok());

        let errors: Vec<usize> = outcome.errors().map(|(index, _)| index).collect();
        assert_eq!(errors, vec![1, 2]);

        // The two failures never discard the two successes.
        let candidates = outcome.candidates();
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].url.as_deref(), Some("http://a.onion/"));
        assert_eq!(candidates[1].url.as_deref(), Some("http://b.onion/"));
    }

    /// 7. `discovered_via` preservation: manual/onion-seed and direct
    /// candidates keep whatever the underlying adapter/caller supplied
    /// (onion seeds are always `None`), and container-discovered
    /// candidates (from material) carry the actual containing document
    /// URL.
    #[cfg(feature = "sitemap")]
    #[tokio::test]
    async fn discovered_via_preservation_across_input_classes() {
        const SITEMAP: &str = r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><url><loc>https://example.test/a</loc></url></urlset>"#;
        let inputs = vec![
            onion("http://abc.onion/"),
            DiscoveryInput::Material {
                material: DiscoveryMaterial {
                    bytes: SITEMAP.as_bytes().to_vec(),
                    url: "https://example.test/sitemap.xml".to_string(),
                },
                intent: DiscoveryParserIntent::Sitemap,
            },
        ];
        let outcome = discover(&inputs).await;
        let candidates = outcome.candidates();
        assert_eq!(candidates[0].discovered_via, None, "manual onion seed");
        assert_eq!(
            candidates[1].discovered_via.as_deref(),
            Some("https://example.test/sitemap.xml"),
            "container-discovered candidate"
        );
    }

    /// 8. Onion seed integration: credential-bearing and non-onion seeds
    /// are rejected through the exact same canonical
    /// `onion_seed::normalize_onion_seed` semantics — this module does
    /// not re-implement or relax that classification.
    #[tokio::test]
    async fn onion_seed_integration_reuses_canonical_semantics() {
        let inputs = vec![
            onion("http://user:pass@abc.onion/"),
            onion("ftp://abc.onion/"),
        ];
        let outcome = discover(&inputs).await;
        assert_eq!(
            outcome.per_input[0],
            Err(DiscoveryError::OnionSeed(
                crate::features::onion_seed::OnionSeedError::CredentialsNotAllowed
            ))
        );
        assert!(matches!(
            outcome.per_input[1],
            Err(DiscoveryError::OnionSeed(
                crate::features::onion_seed::OnionSeedError::UnsupportedScheme(_)
            ))
        ));
        assert!(outcome.candidates().is_empty());
    }

    /// 9. Feature gating: `DiscoveryParserIntent`/`DiscoveryError`'s feed/
    /// sitemap/news_sitemap variants only exist when their respective
    /// feature is enabled, while neutral `DiscoveryMaterial` remains
    /// available — `ResearchScope`/`ScopeSeed` (and hence
    /// `DiscoveryInput::Scope`) are always available (proven simply by
    /// this whole test module compiling and running regardless of which
    /// of those three features are on; the `#[cfg(feature = "...")]`-
    /// gated tests above are skipped, not compile errors, when a
    /// feature is off).
    #[tokio::test]
    async fn scope_seed_inputs_are_always_available() {
        let inputs = vec![
            onion("http://abc.onion/"),
            candidate_input("https://example.test/x", "custom"),
        ];
        let outcome = discover(&inputs).await;
        assert_eq!(outcome.candidates().len(), 2);
    }

    /// `ResearchScope::into_inputs` preserves scope order and yields
    /// `DiscoveryInput::Scope` values that combine cleanly with
    /// `DiscoveryMaterial` in one `discover` call.
    #[cfg(feature = "feed")]
    #[tokio::test]
    async fn research_scope_into_inputs_combines_with_material() {
        const RSS: &str = r#"<rss version="2.0"><channel><title>T</title><item><guid>one</guid><link>https://example.test/a</link><title>A</title></item></channel></rss>"#;
        let mut scope = ResearchScope::new();
        scope
            .push(ScopeSeed::OnionSeed("http://a.onion/".to_string()))
            .push(ScopeSeed::OnionSeed("http://b.onion/".to_string()));
        let mut inputs = scope.into_inputs();
        inputs.push(DiscoveryInput::Material {
            material: DiscoveryMaterial {
                bytes: RSS.as_bytes().to_vec(),
                url: "https://example.test/feed.xml".to_string(),
            },
            intent: DiscoveryParserIntent::Feed,
        });
        let outcome = discover(&inputs).await;
        let candidates = outcome.candidates();
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].url.as_deref(), Some("http://a.onion/"));
        assert_eq!(candidates[1].url.as_deref(), Some("http://b.onion/"));
        assert_eq!(candidates[2].source_type, "feed");
    }

    /// 10 (CRITICAL). Zero acquisition: a hostile/unreachable `.onion`
    /// seed alongside deliberately malformed/never-real document bytes
    /// for every material input class produces a purely functional
    /// result — no attempted contact, proven structurally by this
    /// module's own import surface (see module docs) and functionally by
    /// every input completing with an ordinary `Ok`/`Err` outcome, never
    /// hanging, erroring on a network condition, or requiring any
    /// fixture/mock network server to run this test at all.
    #[cfg(all(feature = "feed", feature = "sitemap", feature = "news_sitemap"))]
    #[tokio::test]
    async fn zero_acquisition_for_hostile_and_malformed_inputs() {
        let inputs = vec![
            onion("http://thishostwillneverresolveorconnectxxxxxxxxxxxxxxxxxxxxxxxxxxxx.onion/"),
            DiscoveryInput::Material {
                material: DiscoveryMaterial {
                    bytes: b"not xml at all".to_vec(),
                    url: "http://thishostwillneverresolveorconnect.invalid/feed.xml".to_string(),
                },
                intent: DiscoveryParserIntent::Feed,
            },
            DiscoveryInput::Material {
                material: DiscoveryMaterial {
                    bytes: b"not xml at all".to_vec(),
                    url: "http://thishostwillneverresolveorconnect.invalid/sitemap.xml".to_string(),
                },
                intent: DiscoveryParserIntent::Sitemap,
            },
            DiscoveryInput::Material {
                material: DiscoveryMaterial {
                    bytes: b"not xml at all".to_vec(),
                    url: "http://thishostwillneverresolveorconnect.invalid/news.xml".to_string(),
                },
                intent: DiscoveryParserIntent::NewsSitemap,
            },
        ];
        let outcome = discover(&inputs).await;
        // Every input completed (no panic, no hang) — an onion seed to a
        // hostile host is still just a classified candidate, and
        // malformed bytes are still just a parse failure, never a
        // network error (there is no network error variant to produce).
        assert_eq!(outcome.per_input.len(), 4);
        assert!(outcome.per_input[0].is_ok());
        for result in &outcome.per_input[1..] {
            assert!(
                result.is_err(),
                "malformed bytes must fail to parse, not hang or panic"
            );
        }
    }

    #[test]
    fn from_iterator_preserves_order() {
        let scope: ResearchScope = vec![
            ScopeSeed::OnionSeed("http://a.onion/".to_string()),
            ScopeSeed::OnionSeed("http://b.onion/".to_string()),
        ]
        .into_iter()
        .collect();
        assert_eq!(scope.len(), 2);
        assert!(matches!(&scope.seeds()[0], ScopeSeed::OnionSeed(s) if s == "http://a.onion/"));
        assert!(matches!(&scope.seeds()[1], ScopeSeed::OnionSeed(s) if s == "http://b.onion/"));
    }
}
