//! `DiscoveryTarget`: the smallest canonical planning boundary for
//! discovery *pointers* — URLs that should be acquired **later**, never
//! content candidates and never something already fetched.
//!
//! Distinct from [`crate::features::research_scope`]'s `SourceItem`
//! candidate model:
//!
//! - a `SourceItem` (`research_scope`/`source`) is a **content
//!   candidate** — something a caller might eventually read/display.
//! - a [`DiscoveryTarget`] is a **pointer** — a URL that should be
//!   acquired later to discover more content or more pointers (a
//!   sitemap index's child sitemap, a robots.txt-declared sitemap, or a
//!   caller/request-supplied URL naming something to fetch).
//!
//! `crate::features::sitemap::SitemapDiscoveryResult::child_sitemaps`
//! and `crate::features::robots_sitemap::RobotsSitemapDiscoveryResult::sitemaps`
//! map into [`DiscoveryTarget`] **here** — never into `SourceItem`.
//! `crate::features::research_scope`'s `discover` deliberately excludes
//! them for exactly this reason (see its module docs); this module is
//! where they belong instead.
//!
//! **Planning is declarative only:**
//!
//! ```text
//! ResearchScope / discovered pointers
//!         │
//!         ▼
//! canonical planning (this module: PlanningInput -> plan(..))
//!         │
//!         ▼
//!    DiscoveryTarget
//!         │
//!         ▼
//!       [STOP]
//!         │
//!         ▼
//! future acquisition boundary (not implemented anywhere in this crate)
//! ```
//!
//! **Zero acquisition**: no HTTP client, no Tor/SOCKS, no DNS, no
//! socket, no filesystem access anywhere in this module; it never
//! constructs a `Page`, an `EvidenceBundle`, or executes a
//! `TransportPolicy`. A [`DiscoveryTarget`] is not proof anything was
//! ever reached — binding one to an actual fetch is later, separate
//! orchestration's job.
//!
//! **Onion classification** reuses the one canonical classifier,
//! [`crate::features::transport::is_onion_url`] — never reimplemented.
//! [`DiscoveryTarget::is_onion`] is a method, not a stored field: it
//! derives the answer fresh from `DiscoveryTarget::url` on every call,
//! so there is no independent onion-classification state that could
//! ever disagree with the target's own URL — `url` is the sole
//! canonical truth. This module never selects, attaches, or executes a
//! transport, and never silently defaults a `.onion` target to
//! `Default` transport. A future acquisition boundary decides transport
//! explicitly (e.g. via `crate::features::transport::TransportRequest`).
//!
//! **Credential/scheme discipline** matches
//! [`crate::features::onion_seed`]'s established contract exactly (parse
//! → scheme → credentials, in that order; a credential-bearing target is
//! rejected outright, never stripped and continued; error variants never
//! echo the supplied URL) — reused as the same policy, not a weaker or
//! stronger one, even though this module is not restricted to `.onion`
//! targets the way `onion_seed` is.

use crate::features::transport::is_onion_url;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// What produced a [`DiscoveryTarget`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum DiscoveryTargetKind {
    /// A caller/request-supplied URL, declared directly — no containing
    /// document. Not restricted to `.onion` (unlike
    /// `crate::features::research_scope::ScopeSeed::OnionSeed`, which
    /// exists specifically for onion-only manual *candidates*); a
    /// requested target may be clearnet or onion.
    Requested,
    /// A child sitemap URL declared by a sitemap index
    /// (`crate::features::sitemap::SitemapReference` /
    /// `SitemapDiscoveryResult::child_sitemaps`).
    ChildSitemap,
    /// A sitemap URL declared by a robots.txt `Sitemap:` directive
    /// (`crate::features::robots_sitemap::RobotsSitemapReference` /
    /// `RobotsSitemapDiscoveryResult::sitemaps`).
    DeclaredSitemap,
}

/// Why a candidate target failed to plan. Every variant is a pure URL
/// classification outcome, decided before any acquisition could even be
/// attempted. Matching `onion_seed::OnionSeedError`'s secret-safety
/// discipline: no variant retains or echoes the supplied URL — only
/// [`DiscoveryTargetError::UnsupportedScheme`] carries data, and only the
/// parsed scheme token (`"ftp"`, `"mailto"`, …), never the URL it came
/// from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryTargetError {
    /// The URL did not parse canonically (includes a missing/empty
    /// host, since `http`/`https` require an authority).
    InvalidUrl,
    /// The URL parsed, but its scheme is not `http` or `https`. Carries
    /// only the parsed scheme token, never the full URL.
    UnsupportedScheme(String),
    /// The URL carries userinfo (`user:pass@`/`user@`) — rejected
    /// outright, never stripped and continued.
    CredentialsNotAllowed,
}

impl std::fmt::Display for DiscoveryTargetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscoveryTargetError::InvalidUrl => write!(f, "discovery target is not a valid URL"),
            DiscoveryTargetError::UnsupportedScheme(scheme) => write!(
                f,
                "discovery target scheme \"{scheme}\" is not supported — only http/https are \
                 accepted"
            ),
            DiscoveryTargetError::CredentialsNotAllowed => write!(
                f,
                "discovery target must not include userinfo/credentials (user:pass@) — \
                 rejected outright, never stripped and continued"
            ),
        }
    }
}

impl std::error::Error for DiscoveryTargetError {}

/// A single, not-yet-fetched acquisition target — planning only. Never
/// evidence, never an acquisition attempt, never a content candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DiscoveryTarget {
    /// The canonical URL to acquire later.
    pub url: String,
    /// What produced this target.
    pub kind: DiscoveryTargetKind,
    /// The exact containing document URL that declared this target, if
    /// any — `None` for a directly caller/request-supplied target
    /// ([`DiscoveryTargetKind::Requested`]). Reuses the same
    /// "discovered_via" concept `SourceItem::discovered_via` already
    /// established (containing-document URL, or genuinely absent) —
    /// reused vocabulary, not a duplicate provenance model.
    pub discovered_via: Option<String>,
}

impl DiscoveryTarget {
    /// Whether this target's host is `.onion`, derived exclusively
    /// through the canonical classifier,
    /// [`crate::features::transport::is_onion_url`], applied to `self.url`
    /// — never reimplemented, and never stored as independent state that
    /// could disagree with `url`. `url` is the sole canonical truth; this
    /// is a pure, side-effect-free derivation from it, computed fresh on
    /// every call.
    ///
    /// `false` for a `url` that does not parse (only reachable if a
    /// caller constructs a `DiscoveryTarget` by hand with a malformed
    /// `url` — every target produced by [`plan`] already has a
    /// canonically parseable `url`, so this can't happen through the
    /// normal planning path).
    ///
    /// Pure information — see the module docs for the transport-
    /// selection boundary this deliberately does not cross.
    pub fn is_onion(&self) -> bool {
        url::Url::parse(&self.url).is_ok_and(|parsed| is_onion_url(&parsed))
    }
}

/// One planning input, in the exact order the caller wants processed.
/// Constructing a [`PlanningInput`] performs no work — normalization
/// (and any error) happens only inside [`plan`].
#[derive(Debug, Clone)]
pub enum PlanningInput {
    /// A caller/request-supplied URL — no containing document.
    Requested(String),
    /// A child sitemap reference declared by a sitemap index, plus the
    /// index's own URL (used as `discovered_via`).
    #[cfg(feature = "sitemap")]
    ChildSitemap {
        /// The declared child sitemap reference.
        reference: crate::features::sitemap::SitemapReference,
        /// The sitemap index document's own URL.
        sitemap_url: String,
    },
    /// A sitemap reference declared by a robots.txt document, plus the
    /// robots.txt's own URL (used as `discovered_via`).
    #[cfg(feature = "robots_sitemap")]
    DeclaredSitemap {
        /// The declared robots.txt `Sitemap:` reference.
        reference: crate::features::robots_sitemap::RobotsSitemapReference,
        /// The robots.txt document's own URL.
        robots_url: String,
    },
}

/// Parse, classify, and validate one target URL. The one place scheme/
/// credential policy and onion classification are decided — every
/// [`plan`] input funnels through here, so there is exactly one
/// validation matrix, not one per [`PlanningInput`] variant.
fn plan_one(
    url: &str,
    kind: DiscoveryTargetKind,
    discovered_via: Option<String>,
) -> Result<DiscoveryTarget, DiscoveryTargetError> {
    let parsed = url::Url::parse(url).map_err(|_| DiscoveryTargetError::InvalidUrl)?;

    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(DiscoveryTargetError::UnsupportedScheme(other.to_string())),
    }

    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(DiscoveryTargetError::CredentialsNotAllowed);
    }

    Ok(DiscoveryTarget {
        url: parsed.as_str().to_string(),
        kind,
        discovered_via,
    })
}

/// Plan every input in `inputs`, in the exact order supplied, into a
/// [`DiscoveryTarget`] or a [`DiscoveryTargetError`] — one `Result` per
/// input, index-aligned, so a caller can always trace a failure back to
/// the exact input that produced it. One malformed target never
/// discards the others. No deduplication, no reordering, no ranking —
/// duplicate inputs produce duplicate targets.
///
/// Performs **zero acquisition**: every input already carries the exact
/// string/reference it needs; this function only parses and classifies
/// what was handed to it. See the module docs for the full domain
/// boundary.
pub fn plan(inputs: &[PlanningInput]) -> Vec<Result<DiscoveryTarget, DiscoveryTargetError>> {
    inputs
        .iter()
        .map(|input| match input {
            PlanningInput::Requested(url) => plan_one(url, DiscoveryTargetKind::Requested, None),
            #[cfg(feature = "sitemap")]
            PlanningInput::ChildSitemap {
                reference,
                sitemap_url,
            } => plan_one(
                &reference.url,
                DiscoveryTargetKind::ChildSitemap,
                Some(sitemap_url.clone()),
            ),
            #[cfg(feature = "robots_sitemap")]
            PlanningInput::DeclaredSitemap {
                reference,
                robots_url,
            } => plan_one(
                &reference.url,
                DiscoveryTargetKind::DeclaredSitemap,
                Some(robots_url.clone()),
            ),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Structural proof: `plan`'s output type is exactly
    /// `DiscoveryTarget`, never `SourceItem` — a `DiscoveryTarget` has no
    /// title/snippet/authors/media/evidence fields to fabricate, and
    /// this exhaustive struct literal (every field named, no `..`) locks
    /// that shape in: if a candidate-only field were ever added to
    /// `DiscoveryTarget` by mistake, this would fail to compile until
    /// explicitly acknowledged here.
    ///
    /// 4. It also proves `DiscoveryTarget` has no independently stored
    ///    onion-classification field: this literal names every field the
    ///    type has (`url`, `kind`, `discovered_via`) with no `..`
    ///    fallback — if a stored `is_onion` (or similarly named) field
    ///    ever existed on the struct, this literal would fail to compile
    ///    as non-exhaustive until updated to set it. `is_onion` is
    ///    reachable only as a method below, derived fresh from `url`.
    #[test]
    fn discovery_target_shape_has_no_source_item_fields_or_stored_onion_flag() {
        let target = DiscoveryTarget {
            url: "https://example.test/sitemap-2.xml".to_string(),
            kind: DiscoveryTargetKind::ChildSitemap,
            discovered_via: Some("https://example.test/sitemap.xml".to_string()),
        };
        assert_eq!(target.url, "https://example.test/sitemap-2.xml");
        assert_eq!(target.kind, DiscoveryTargetKind::ChildSitemap);
        assert_eq!(
            target.discovered_via.as_deref(),
            Some("https://example.test/sitemap.xml")
        );
        assert!(!target.is_onion());
    }

    /// 1. Empty planning input.
    #[test]
    fn empty_planning_input_produces_empty_output() {
        assert!(plan(&[]).is_empty());
    }

    /// 2. Caller/request target.
    #[test]
    fn caller_request_target_plans_with_no_origin() {
        let results = plan(&[PlanningInput::Requested(
            "https://example.test/page".to_string(),
        )]);
        assert_eq!(results.len(), 1);
        let target = results[0].as_ref().unwrap();
        assert_eq!(target.url, "https://example.test/page");
        assert_eq!(target.kind, DiscoveryTargetKind::Requested);
        assert_eq!(target.discovered_via, None);
        assert!(!target.is_onion());
    }

    /// 3. Sitemap child target.
    #[cfg(feature = "sitemap")]
    #[test]
    fn sitemap_child_target_plans_with_index_as_origin() {
        let results = plan(&[PlanningInput::ChildSitemap {
            reference: crate::features::sitemap::SitemapReference {
                url: "https://example.test/child.xml".to_string(),
                updated_at: None,
            },
            sitemap_url: "https://example.test/sitemap.xml".to_string(),
        }]);
        assert_eq!(results.len(), 1);
        let target = results[0].as_ref().unwrap();
        assert_eq!(target.url, "https://example.test/child.xml");
        assert_eq!(target.kind, DiscoveryTargetKind::ChildSitemap);
        assert_eq!(
            target.discovered_via.as_deref(),
            Some("https://example.test/sitemap.xml")
        );
    }

    /// 4. Robots sitemap target.
    #[cfg(feature = "robots_sitemap")]
    #[test]
    fn robots_declared_sitemap_target_plans_with_robots_as_origin() {
        let results = plan(&[PlanningInput::DeclaredSitemap {
            reference: crate::features::robots_sitemap::RobotsSitemapReference {
                url: "https://example.test/sitemap.xml".to_string(),
            },
            robots_url: "https://example.test/robots.txt".to_string(),
        }]);
        assert_eq!(results.len(), 1);
        let target = results[0].as_ref().unwrap();
        assert_eq!(target.url, "https://example.test/sitemap.xml");
        assert_eq!(target.kind, DiscoveryTargetKind::DeclaredSitemap);
        assert_eq!(
            target.discovered_via.as_deref(),
            Some("https://example.test/robots.txt")
        );
    }

    /// 5. Ordering: preserved across mixed input classes.
    #[cfg(all(feature = "sitemap", feature = "robots_sitemap"))]
    #[test]
    fn ordering_is_preserved_across_input_classes() {
        let inputs = vec![
            PlanningInput::Requested("https://example.test/a".to_string()),
            PlanningInput::ChildSitemap {
                reference: crate::features::sitemap::SitemapReference {
                    url: "https://example.test/b".to_string(),
                    updated_at: None,
                },
                sitemap_url: "https://example.test/index.xml".to_string(),
            },
            PlanningInput::DeclaredSitemap {
                reference: crate::features::robots_sitemap::RobotsSitemapReference {
                    url: "https://example.test/c".to_string(),
                },
                robots_url: "https://example.test/robots.txt".to_string(),
            },
            PlanningInput::Requested("https://example.test/d".to_string()),
        ];
        let results = plan(&inputs);
        let urls: Vec<&str> = results
            .iter()
            .map(|r| r.as_ref().unwrap().url.as_str())
            .collect();
        assert_eq!(
            urls,
            [
                "https://example.test/a",
                "https://example.test/b",
                "https://example.test/c",
                "https://example.test/d"
            ]
        );
    }

    /// 6. Duplicate targets are preserved verbatim — no deduplication.
    #[test]
    fn duplicate_targets_are_preserved() {
        let inputs = vec![
            PlanningInput::Requested("https://example.test/a".to_string()),
            PlanningInput::Requested("https://example.test/a".to_string()),
        ];
        let results = plan(&inputs);
        assert_eq!(results.len(), 2);
        for result in &results {
            assert_eq!(result.as_ref().unwrap().url, "https://example.test/a");
        }
    }

    /// 7. Origin/discovered-from semantics: `Requested` is always
    ///    `None`; container-declared targets always carry the actual
    ///    containing document URL, never a self-reference or placeholder.
    #[cfg(feature = "sitemap")]
    #[test]
    fn origin_semantics_distinguish_requested_from_container_declared() {
        let results = plan(&[
            PlanningInput::Requested("https://example.test/manual".to_string()),
            PlanningInput::ChildSitemap {
                reference: crate::features::sitemap::SitemapReference {
                    url: "https://example.test/child.xml".to_string(),
                    updated_at: None,
                },
                sitemap_url: "https://example.test/index.xml".to_string(),
            },
        ]);
        assert_eq!(results[0].as_ref().unwrap().discovered_via, None);
        assert_eq!(
            results[1].as_ref().unwrap().discovered_via.as_deref(),
            Some("https://example.test/index.xml")
        );
        // `discovered_via` is the *index's* URL, never a self-reference
        // to the child target's own URL.
        let target = results[1].as_ref().unwrap();
        assert_ne!(target.discovered_via.as_deref(), Some(target.url.as_str()));
    }

    /// 8. Malformed URL / error alignment: one bad target does not
    ///    destroy unrelated valid targets, and errors are index-traceable.
    #[test]
    fn malformed_target_alignment_and_error_semantics() {
        let inputs = vec![
            PlanningInput::Requested("https://example.test/ok-1".to_string()),
            PlanningInput::Requested("not a url".to_string()),
            PlanningInput::Requested("ftp://example.test/file".to_string()),
            PlanningInput::Requested("https://user:pass@example.test/".to_string()),
            PlanningInput::Requested("https://example.test/ok-2".to_string()),
        ];
        let results = plan(&inputs);
        assert_eq!(results.len(), 5);
        assert!(results[0].is_ok());
        assert_eq!(results[1], Err(DiscoveryTargetError::InvalidUrl));
        assert_eq!(
            results[2],
            Err(DiscoveryTargetError::UnsupportedScheme("ftp".to_string()))
        );
        assert_eq!(results[3], Err(DiscoveryTargetError::CredentialsNotAllowed));
        assert!(results[4].is_ok());
        assert_eq!(
            results[0].as_ref().unwrap().url,
            "https://example.test/ok-1"
        );
        assert_eq!(
            results[4].as_ref().unwrap().url,
            "https://example.test/ok-2"
        );
    }

    /// Errors never echo the supplied URL/secrets — same discipline as
    /// `onion_seed::OnionSeedError`.
    #[test]
    fn errors_never_leak_supplied_url_or_secrets() {
        const SENTINEL: &str = "sekretpw24680";
        let seed = format!("https://user:{SENTINEL}@example.test/?t={SENTINEL}");
        let results = plan(&[PlanningInput::Requested(seed)]);
        let error = results[0].as_ref().unwrap_err();
        assert_eq!(*error, DiscoveryTargetError::CredentialsNotAllowed);
        let debug = format!("{error:?}");
        let display = format!("{error}");
        assert!(!debug.contains(SENTINEL));
        assert!(!display.contains(SENTINEL));
    }

    /// 9. Onion target semantics: reuses the canonical classifier, and
    ///    never selects/attaches a transport — `DiscoveryTarget` has no
    ///    transport field at all (see the exhaustive struct literal proof
    ///    above), and a `.onion` target is never silently rejected or
    ///    coerced to clearnet during planning.
    #[test]
    fn onion_target_semantics_reuse_canonical_classifier_no_transport_selection() {
        let results = plan(&[
            PlanningInput::Requested("http://abc.onion/page".to_string()),
            PlanningInput::Requested("https://example.test/page".to_string()),
        ]);
        assert!(results[0].as_ref().unwrap().is_onion());
        assert!(!results[1].as_ref().unwrap().is_onion());
        // Both succeed identically as plain targets — onion-ness is
        // informational only, never a rejection or transport decision.
        assert!(results[0].is_ok());
        assert!(results[1].is_ok());
    }

    /// 1/2/3 (operator correction): `is_onion()` is derived exclusively
    /// from `url` on every call — never cached/stored at construction.
    /// Proven directly: two `DiscoveryTarget` values built with the
    /// exact same `kind`/`discovered_via` but different `url`s report
    /// different `is_onion()` results, and mutating `url` on one
    /// in-memory value changes what `is_onion()` subsequently reports —
    /// something impossible if the answer were cached in an independent
    /// field rather than derived fresh each time.
    #[test]
    fn is_onion_is_derived_from_url_not_cached() {
        let mut target = DiscoveryTarget {
            url: "http://abc.onion/".to_string(),
            kind: DiscoveryTargetKind::Requested,
            discovered_via: None,
        };
        assert!(target.is_onion(), "onion target => true");

        target.url = "https://example.test/".to_string();
        assert!(
            !target.is_onion(),
            "changing url must change is_onion()'s answer — it is not cached"
        );

        target.url = "http://xyz.onion/".to_string();
        assert!(
            target.is_onion(),
            "flipping url back to onion must flip is_onion() back to true"
        );
    }

    /// 3: an ordinary HTTP/HTTPS (clearnet) target reports `false`.
    #[test]
    fn ordinary_clearnet_target_is_onion_false() {
        let results = plan(&[PlanningInput::Requested(
            "https://example.test/page".to_string(),
        )]);
        assert!(!results[0].as_ref().unwrap().is_onion());
    }

    /// 10 (CRITICAL). Zero acquisition, functionally: an unreachable
    /// `.onion` host plans instantly and successfully — no attempted
    /// contact, no fixture/mock server required to run this test.
    #[test]
    fn zero_acquisition_for_hostile_onion_target() {
        let results = plan(&[PlanningInput::Requested(
            "http://thishostwillneverresolveorconnectxxxxxxxxxxxxxxxxxxxxxxxxxxxx.onion/"
                .to_string(),
        )]);
        assert!(results[0].is_ok());
        assert!(results[0].as_ref().unwrap().is_onion());
    }

    /// 12. Feature gating: `Requested` planning is always available
    ///     regardless of `sitemap`/`robots_sitemap` — proven simply by
    ///     this test running under any feature combination (the
    ///     `#[cfg]`-gated tests above are skipped, not compile errors,
    ///     when a feature is off).
    #[test]
    fn requested_planning_is_always_available() {
        let results = plan(&[PlanningInput::Requested(
            "https://example.test/x".to_string(),
        )]);
        assert!(results[0].is_ok());
    }
}
