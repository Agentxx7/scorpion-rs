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
//! [`ScopeSeed`] or [`DiscoveryMaterial`]) into [`SourceItem`]
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

/// Already-acquired discovery material: fetched document bytes the
/// caller already retrieved through some acquisition step entirely
/// outside this module's concern, paired with the document's own URL.
/// **Not part of [`ResearchScope`]** — acquisition output is a
/// structurally distinct concern from declarative scope (see the module
/// docs' domain-boundary diagram). Constructing a [`DiscoveryMaterial`]
/// performs no work; normalization happens only inside [`discover`].
#[derive(Debug, Clone)]
pub enum DiscoveryMaterial {
    /// An already-fetched RSS/Atom feed document's bytes, plus the
    /// feed's own URL (used for `discovered_via`). The bytes must
    /// already have been retrieved by the caller — this module never
    /// fetches them.
    #[cfg(feature = "feed")]
    Feed {
        /// Exact bytes already retrieved by the caller.
        bytes: Vec<u8>,
        /// The feed document's own URL.
        feed_url: String,
    },
    /// An already-fetched standard sitemap document's bytes, plus its
    /// own URL. Only `urlset` content entries become candidates; a
    /// `sitemapindex`'s child-sitemap pointers do not (see module docs).
    #[cfg(feature = "sitemap")]
    Sitemap {
        /// Exact bytes already retrieved by the caller.
        bytes: Vec<u8>,
        /// The sitemap document's own URL.
        sitemap_url: String,
    },
    /// An already-fetched Google News Sitemap document's bytes, plus its
    /// own URL. Only the generic `SourceItem` half of each entry becomes
    /// a candidate — the News-specific metadata
    /// (`NewsSitemapEntry::news`) is intentionally not merged into the
    /// generic candidate shape; a caller who needs it should call
    /// [`crate::features::news_sitemap::parse`] directly.
    #[cfg(feature = "news_sitemap")]
    NewsSitemap {
        /// Exact bytes already retrieved by the caller.
        bytes: Vec<u8>,
        /// The News Sitemap document's own URL.
        sitemap_url: String,
    },
}

/// One item for [`discover`] to normalize, in the exact order the caller
/// wants processed — a [`ScopeSeed`] (declarative) or [`DiscoveryMaterial`]
/// (already-acquired). This is the orchestration boundary's working
/// unit: it exists only as `discover`'s input shape, and is **never**
/// stored inside [`ResearchScope`] — that separation is the whole point
/// of this module's design (see the module docs' domain-boundary
/// diagram). A caller coordinates scope seeds and discovery material
/// together by building a `Vec<DiscoveryInput>` (via
/// [`ResearchScope::into_inputs`] plus `.into()` on any
/// [`DiscoveryMaterial`] values, interleaved in whatever order is
/// wanted) and passing it to [`discover`] in one call.
#[derive(Debug, Clone)]
pub enum DiscoveryInput {
    /// A declarative scope seed.
    Scope(ScopeSeed),
    /// Already-acquired discovery material.
    Material(DiscoveryMaterial),
}

impl From<ScopeSeed> for DiscoveryInput {
    fn from(seed: ScopeSeed) -> Self {
        DiscoveryInput::Scope(seed)
    }
}

impl From<DiscoveryMaterial> for DiscoveryInput {
    fn from(material: DiscoveryMaterial) -> Self {
        DiscoveryInput::Material(material)
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
            #[cfg(feature = "feed")]
            DiscoveryInput::Material(DiscoveryMaterial::Feed { bytes, feed_url }) => {
                crate::features::feed::parse(bytes, feed_url)
                    .await
                    .map(|result| result.entries)
                    .map_err(DiscoveryError::Feed)
            }
            #[cfg(feature = "sitemap")]
            DiscoveryInput::Material(DiscoveryMaterial::Sitemap { bytes, sitemap_url }) => {
                crate::features::sitemap::parse(bytes, sitemap_url)
                    .await
                    .map(|result| result.entries)
                    .map_err(DiscoveryError::Sitemap)
            }
            #[cfg(feature = "news_sitemap")]
            DiscoveryInput::Material(DiscoveryMaterial::NewsSitemap { bytes, sitemap_url }) => {
                crate::features::news_sitemap::parse(bytes, sitemap_url)
                    .await
                    .map(|result| result.entries.into_iter().map(|entry| entry.item).collect())
                    .map_err(DiscoveryError::NewsSitemap)
            }
            // Reachable only when none of feed/sitemap/news_sitemap is
            // enabled — `DiscoveryMaterial` is then an uninhabited type
            // (all three variants are individually feature-gated), so no
            // `DiscoveryInput::Material(..)` value can actually exist at
            // runtime. This arm exists purely to satisfy exhaustiveness
            // across that feature combination; the empty match on
            // `*material` is itself exhaustive for an uninhabited type.
            #[cfg(not(any(feature = "feed", feature = "sitemap", feature = "news_sitemap")))]
            DiscoveryInput::Material(material) => match *material {},
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
            DiscoveryInput::Material(DiscoveryMaterial::Feed {
                bytes: RSS.as_bytes().to_vec(),
                feed_url: "https://example.test/feed.xml".to_string(),
            }),
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

    /// 4. Ordering: candidates preserve caller-supplied order across
    /// scope/material input classes, and within a multi-item input, that
    /// adapter's own order.
    #[cfg(feature = "sitemap")]
    #[tokio::test]
    async fn ordering_is_preserved_across_and_within_inputs() {
        const SITEMAP: &str = r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><url><loc>https://example.test/1</loc></url><url><loc>https://example.test/2</loc></url></urlset>"#;
        let inputs = vec![
            onion("http://z.onion/"),
            DiscoveryInput::Material(DiscoveryMaterial::Sitemap {
                bytes: SITEMAP.as_bytes().to_vec(),
                sitemap_url: "https://example.test/sitemap.xml".to_string(),
            }),
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
            DiscoveryInput::Material(DiscoveryMaterial::Sitemap {
                bytes: SITEMAP.as_bytes().to_vec(),
                sitemap_url: "https://example.test/sitemap.xml".to_string(),
            }),
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

    /// 9. Feature gating: `DiscoveryMaterial`/`DiscoveryError`'s feed/
    /// sitemap/news_sitemap variants only exist when their respective
    /// feature is enabled — `ResearchScope`/`ScopeSeed` (and hence
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
        inputs.push(DiscoveryInput::Material(DiscoveryMaterial::Feed {
            bytes: RSS.as_bytes().to_vec(),
            feed_url: "https://example.test/feed.xml".to_string(),
        }));
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
            DiscoveryInput::Material(DiscoveryMaterial::Feed {
                bytes: b"not xml at all".to_vec(),
                feed_url: "http://thishostwillneverresolveorconnect.invalid/feed.xml".to_string(),
            }),
            DiscoveryInput::Material(DiscoveryMaterial::Sitemap {
                bytes: b"not xml at all".to_vec(),
                sitemap_url: "http://thishostwillneverresolveorconnect.invalid/sitemap.xml"
                    .to_string(),
            }),
            DiscoveryInput::Material(DiscoveryMaterial::NewsSitemap {
                bytes: b"not xml at all".to_vec(),
                sitemap_url: "http://thishostwillneverresolveorconnect.invalid/news.xml"
                    .to_string(),
            }),
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
