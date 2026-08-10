//! Binds a validated [`DiscoveryTarget`] to Scorpion's existing
//! canonical acquisition/transport request vocabulary — the smallest
//! possible seam between planning and (a caller's own, separate)
//! execution.
//!
//! ```text
//! DiscoveryTarget
//!       │
//!       ▼
//! canonical binding (this module)
//!       │
//!       ▼
//! (url, AcquisitionOptions)   <- exactly what
//!                                 crate::utils::evidence::fetch_single_page_with_options
//!                                 already accepts
//!       │
//!       ▼
//!     [STOP]
//! ```
//!
//! This module never calls `fetch_single_page_with_options` (or any
//! other acquisition function) itself — constructing an
//! [`AcquisitionBinding`] performs no network activity whatsoever. A
//! caller must separately choose to execute it.
//!
//! **No new transport system.** Transport *choice* is exactly
//! [`crate::features::transport::TransportRequest`] (mode + proxy),
//! resolved through its own canonical `into_policy()` — the same one
//! seam the CLI and MCP surfaces already use; there is no second Tor
//! flag, no second proxy field, and no independent onion-classification
//! logic here. Onion-ness is read via [`DiscoveryTarget::is_onion`]
//! (itself derived from the one canonical `is_onion_url`), and the
//! already-closed "`.onion` target under `Default` transport is
//! rejected before any network activity" rule is enforced by reusing
//! [`crate::features::transport::validate_target`] directly — not
//! reimplemented, not relaxed, not silently bypassed. Binding an onion
//! target against a `Default`-resolving [`TransportRequest`] fails
//! closed at [`bind`], exactly as the acquisition seam itself would.
//!
//! [`DiscoveryTargetKind`] is deliberately **not** carried into
//! [`AcquisitionBinding`]: `fetch_single_page_with_options` has no use
//! for it, and a caller already holds the original `DiscoveryTarget`
//! (with its `kind`) if it's needed for anything else — duplicating it
//! into the binding would be redundant state with no consumer (see
//! `crate::features::discovery_target`'s own `kind` field for the
//! authoritative copy).
//!
//! Requires the `evidence` feature — the same feature
//! `AcquisitionOptions`/`fetch_single_page_with_options` themselves
//! require; this module cannot exist without the vocabulary it binds
//! into. Does **not** require `transport_tor`: constructing a binding
//! (for either `Default` or `Tor` transport) is pure request-shape
//! validation, never actual Tor execution — `transport_tor` only gates
//! whether a later, separate call to `fetch_single_page_with_options`
//! can actually execute a `Tor` binding.

use crate::features::discovery_target::DiscoveryTarget;
use crate::features::transport::{self, TransportError, TransportRequest};
use crate::utils::evidence::AcquisitionOptions;

/// Why a [`DiscoveryTarget`] could not be bound to acquisition intent.
/// Every variant delegates directly to an existing canonical error type
/// — never a re-derived or re-worded copy — so no new secret-leak
/// surface is introduced here: whatever secret-safety
/// [`TransportError`] already guarantees, this type inherits verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcquisitionBindingError {
    /// The requested transport itself is malformed, or is incompatible
    /// with this specific target — including the closed "`.onion`
    /// target under `Default` transport" rejection. See
    /// [`TransportRequest::into_policy`] and
    /// [`crate::features::transport::validate_target`].
    Transport(TransportError),
    /// The target's `url` did not parse canonically. Only reachable for
    /// a hand-constructed `DiscoveryTarget` with a malformed `url` —
    /// every target produced by
    /// [`crate::features::discovery_target::plan`] already has one that
    /// parses.
    InvalidTargetUrl,
}

impl std::fmt::Display for AcquisitionBindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AcquisitionBindingError::Transport(error) => write!(f, "{error}"),
            AcquisitionBindingError::InvalidTargetUrl => {
                write!(f, "discovery target url is not a valid URL")
            }
        }
    }
}

impl std::error::Error for AcquisitionBindingError {}

/// The result of binding one [`DiscoveryTarget`] to acquisition intent —
/// exactly the `(url, AcquisitionOptions)` pair
/// [`crate::utils::evidence::fetch_single_page_with_options`] already
/// accepts. **Not itself executable**: constructing one performs no
/// network activity; a caller must separately call
/// `fetch_single_page_with_options(&binding.url, binding.options)` (or
/// an equivalent canonical acquisition entry point) to actually acquire
/// anything.
#[derive(Debug, Clone)]
pub struct AcquisitionBinding {
    /// The canonical target URL to acquire — copied verbatim from
    /// [`DiscoveryTarget::url`].
    pub url: String,
    /// The exact options the canonical one-shot acquisition seam
    /// accepts.
    pub options: AcquisitionOptions,
}

/// Bind one [`DiscoveryTarget`] to acquisition intent under the given
/// [`TransportRequest`]. Performs no network activity — see the module
/// docs for the full binding contract, including the reused onion +
/// `Default`-transport rejection.
pub fn bind(
    target: &DiscoveryTarget,
    transport_request: TransportRequest,
) -> Result<AcquisitionBinding, AcquisitionBindingError> {
    let policy = transport_request
        .into_policy()
        .map_err(AcquisitionBindingError::Transport)?;

    let parsed =
        url::Url::parse(&target.url).map_err(|_| AcquisitionBindingError::InvalidTargetUrl)?;

    // The exact same closed-loop rejection `fetch_single_page_with_options`
    // itself applies — reused, not reimplemented, so a `.onion` target
    // can never be bound to `Default` transport and later silently
    // acquired over clearnet.
    transport::validate_target(&parsed, &policy).map_err(AcquisitionBindingError::Transport)?;

    Ok(AcquisitionBinding {
        url: target.url.clone(),
        options: AcquisitionOptions { transport: policy },
    })
}

/// Bind every target in `targets`, in order, under the same
/// [`TransportRequest`] — one `Result` per target, index-aligned, so a
/// caller can always trace a failure back to the exact target that
/// produced it. One binding failure never discards the others; no
/// deduplication, no reordering.
pub fn bind_all(
    targets: &[DiscoveryTarget],
    transport_request: TransportRequest,
) -> Vec<Result<AcquisitionBinding, AcquisitionBindingError>> {
    targets
        .iter()
        .map(|target| bind(target, transport_request.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::discovery_target::DiscoveryTargetKind;
    use crate::features::transport::TransportMode;

    fn requested(url: &str) -> DiscoveryTarget {
        DiscoveryTarget {
            url: url.to_string(),
            kind: DiscoveryTargetKind::Requested,
            discovered_via: None,
        }
    }

    fn default_transport() -> TransportRequest {
        TransportRequest {
            mode: TransportMode::Default,
            proxy: None,
        }
    }

    fn tor_transport(proxy: &str) -> TransportRequest {
        TransportRequest {
            mode: TransportMode::Tor,
            proxy: Some(proxy.to_string()),
        }
    }

    /// 1. Ordinary requested target binds correctly.
    #[test]
    fn ordinary_requested_target_binds_correctly() {
        let target = requested("https://example.test/page");
        let binding = bind(&target, default_transport()).unwrap();
        assert_eq!(binding.url, "https://example.test/page");
        assert!(matches!(
            binding.options.transport,
            crate::features::transport::TransportPolicy::Default
        ));
    }

    /// 2. Sitemap child target binds correctly.
    #[cfg(feature = "sitemap")]
    #[test]
    fn sitemap_child_target_binds_correctly() {
        let results = crate::features::discovery_target::plan(&[
            crate::features::discovery_target::PlanningInput::ChildSitemap {
                reference: crate::features::sitemap::SitemapReference {
                    url: "https://example.test/child.xml".to_string(),
                    updated_at: None,
                },
                sitemap_url: "https://example.test/index.xml".to_string(),
            },
        ]);
        let target = results[0].as_ref().unwrap();
        let binding = bind(target, default_transport()).unwrap();
        assert_eq!(binding.url, "https://example.test/child.xml");
    }

    /// 3. Robots-declared sitemap target binds correctly.
    #[cfg(feature = "robots_sitemap")]
    #[test]
    fn robots_declared_sitemap_target_binds_correctly() {
        let results = crate::features::discovery_target::plan(&[
            crate::features::discovery_target::PlanningInput::DeclaredSitemap {
                reference: crate::features::robots_sitemap::RobotsSitemapReference {
                    url: "https://example.test/sitemap.xml".to_string(),
                },
                robots_url: "https://example.test/robots.txt".to_string(),
            },
        ]);
        let target = results[0].as_ref().unwrap();
        let binding = bind(target, default_transport()).unwrap();
        assert_eq!(binding.url, "https://example.test/sitemap.xml");
    }

    /// 4. Target URL preserved canonically (byte-for-byte, no rewrite).
    #[test]
    fn target_url_preserved_canonically() {
        let target = requested("https://example.test/a/b?c=1#d");
        let binding = bind(&target, default_transport()).unwrap();
        assert_eq!(binding.url, target.url);
    }

    /// 5/6/7. Batch binding preserves order, duplicates, and produces
    /// index-aligned errors — one failure never discards the others.
    #[test]
    fn batch_binding_preserves_order_duplicates_and_index_alignment() {
        let targets = vec![
            requested("https://example.test/a"),
            requested("http://abc.onion/b"), // will fail under Default transport
            requested("https://example.test/a"), // duplicate of index 0
            requested("https://example.test/c"),
        ];
        let results = bind_all(&targets, default_transport());
        assert_eq!(results.len(), 4);
        assert_eq!(results[0].as_ref().unwrap().url, "https://example.test/a");
        assert!(matches!(
            results[1],
            Err(AcquisitionBindingError::Transport(
                TransportError::OnionRequiresTor
            ))
        ));
        // Duplicate preserved verbatim, not deduplicated or skipped.
        assert_eq!(results[2].as_ref().unwrap().url, "https://example.test/a");
        assert_eq!(results[3].as_ref().unwrap().url, "https://example.test/c");
    }

    /// 8. Onion classification is not reimplemented: a bound onion
    ///    target under a *valid* Tor `TransportRequest` succeeds and
    ///    resolves to `TransportPolicy::Tor`, exactly matching
    ///    `DiscoveryTarget::is_onion()`'s (canonical-classifier-derived)
    ///    answer — proving this module reads that classification rather
    ///    than re-deriving it independently.
    #[test]
    fn onion_target_under_valid_tor_request_binds_to_tor_policy() {
        let target = requested("http://abc.onion/page");
        assert!(target.is_onion());
        let binding = bind(&target, tor_transport("socks5h://127.0.0.1:9050")).unwrap();
        assert!(matches!(
            binding.options.transport,
            crate::features::transport::TransportPolicy::Tor(_)
        ));
    }

    /// 9 (CRITICAL). An onion target cannot silently bind to `Default`
    /// transport — the exact closed canonical contract
    /// (`.onion` + `Default` => rejected before any network activity),
    /// reused via `transport::validate_target`, not a new rule.
    #[test]
    fn onion_target_cannot_silently_bind_to_default_transport() {
        let target = requested("http://abc.onion/page");
        let error = bind(&target, default_transport()).unwrap_err();
        assert_eq!(
            error,
            AcquisitionBindingError::Transport(TransportError::OnionRequiresTor)
        );
    }

    /// 10. A clearnet target retains existing normal/default semantics
    ///     — binds cleanly to `Default` transport, no special handling.
    #[test]
    fn clearnet_target_retains_default_semantics() {
        let target = requested("https://example.test/page");
        assert!(!target.is_onion());
        let binding = bind(&target, default_transport()).unwrap();
        assert!(matches!(
            binding.options.transport,
            crate::features::transport::TransportPolicy::Default
        ));
    }

    /// 11. No credentials leak: a Tor proxy endpoint carrying userinfo
    ///     is rejected by the existing canonical endpoint validation
    ///     (`TorTransportConfig::new`), and neither the `Debug` nor
    ///     `Display` of the resulting binding error contains the
    ///     sentinel credential.
    #[test]
    fn no_credentials_leak_through_binding_errors() {
        const SENTINEL: &str = "sekretpw13579";
        let target = requested("https://example.test/page");
        let error = bind(
            &target,
            tor_transport(&format!("socks5h://user:{SENTINEL}@127.0.0.1:9050")),
        )
        .unwrap_err();
        let debug = format!("{error:?}");
        let display = format!("{error}");
        assert!(!debug.contains(SENTINEL));
        assert!(!display.contains(SENTINEL));
    }

    /// 12/13. Structural proof: `AcquisitionBinding` synthesizes neither
    /// a `SourceItem` nor a `DiscoveryMaterial` — its only fields are
    /// `url: String` and `options: AcquisitionOptions`, named
    /// exhaustively here with no `..` fallback. If a candidate-shaped
    /// field (title/snippet/authors/…) or a raw-bytes material field
    /// were ever added by mistake, this would fail to compile until
    /// explicitly acknowledged.
    #[test]
    fn acquisition_binding_shape_has_no_source_item_or_material_fields() {
        let binding = AcquisitionBinding {
            url: "https://example.test/page".to_string(),
            options: AcquisitionOptions::default(),
        };
        assert_eq!(binding.url, "https://example.test/page");
        assert!(matches!(
            binding.options.transport,
            crate::features::transport::TransportPolicy::Default
        ));
    }

    /// 14 (CRITICAL). Zero acquisition: binding an unreachable onion
    /// target (under a syntactically valid but never-actually-running
    /// Tor proxy endpoint) completes instantly and successfully — no
    /// attempted contact, no fixture/mock server required to run this
    /// test at all.
    #[test]
    fn zero_acquisition_binding_hostile_onion_target() {
        let target = requested(
            "http://thishostwillneverresolveorconnectxxxxxxxxxxxxxxxxxxxxxxxxxxxx.onion/",
        );
        let binding = bind(&target, tor_transport("socks5h://127.0.0.1:9050")).unwrap();
        assert!(binding.url.contains(".onion"));
    }

    /// A malformed `TransportRequest` (e.g. `Tor` mode with no proxy)
    /// fails at binding exactly as `TransportRequest::into_policy`
    /// already defines — no new validation matrix.
    #[test]
    fn malformed_transport_request_fails_via_existing_into_policy_contract() {
        let target = requested("https://example.test/page");
        let error = bind(
            &target,
            TransportRequest {
                mode: TransportMode::Tor,
                proxy: None,
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            AcquisitionBindingError::Transport(TransportError::IncompatibleConfiguration(_))
        ));
    }
}
