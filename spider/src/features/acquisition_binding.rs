//! Binds a validated [`DiscoveryTarget`] to Scorpion's existing
//! canonical acquisition/transport request vocabulary, and — via
//! [`execute`] — runs that binding through the one canonical acquisition
//! entry point.
//!
//! ```text
//! DiscoveryTarget
//!       │
//!       ▼
//! canonical binding (bind / bind_all)
//!       │
//!       ▼
//! AcquisitionBinding = (url, AcquisitionOptions)   <- exactly what
//!                                 crate::utils::evidence::fetch_single_page_with_options
//!                                 already accepts
//!       │
//!       ▼
//! canonical execution seam (execute)
//!       │
//!       ▼
//! fetch_single_page_with_options(&binding.url, binding.options)
//!       │
//!       ▼
//! TransportAcquisition
//!       │
//!       ▼
//! canonical local materialization ([`materialize`])
//!       │
//!       ▼
//! DiscoveryMaterial = (exact acquired bytes, final containing URL)
//!       │
//!       ▼
//!     [STOP]
//! ```
//!
//! [`bind`]/[`bind_all`] never call `fetch_single_page_with_options` (or
//! any other acquisition function) — constructing an
//! [`AcquisitionBinding`] performs no network activity whatsoever.
//! [`execute`] is the one place this module actually reaches the
//! network, and it does so by **delegating entirely** to the existing
//! canonical acquisition function — no new `reqwest::Client`, no new
//! SOCKS client, no new `Website` crawl path, no new redirect
//! implementation, no new target validator, no new Tor/`Default`
//! resolver, and no new acquisition response type. `execute` does not
//! call `TransportRequest::into_policy()` — that resolution already
//! happened, once, inside [`bind`]; `execute` consumes
//! `binding.options.transport` exactly as already resolved, unmodified.
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
use crate::features::research_scope::DiscoveryMaterial;
use crate::features::transport::{self, TransportError, TransportRequest};
use crate::utils::evidence::{AcquisitionOptions, TransportAcquisition};

/// Why an acquired page could not become parser-neutral discovery material.
/// No variant retains a URL or body, so errors cannot echo credentials,
/// query tokens, fragments, or acquired content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryMaterialError {
    /// The canonical Page parsing-status contract does not consider this
    /// response usable material. Carries only the numeric status code.
    UnusableStatus(u16),
    /// The acquisition retained no body, or its already-acquired canonical
    /// disk spool could not be materialized.
    BodyUnavailable,
}

impl std::fmt::Display for DiscoveryMaterialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscoveryMaterialError::UnusableStatus(status) => {
                write!(
                    f,
                    "acquired response status {status} is not usable discovery material"
                )
            }
            DiscoveryMaterialError::BodyUnavailable => {
                write!(f, "acquired response body is unavailable")
            }
        }
    }
}

impl std::error::Error for DiscoveryMaterialError {}

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

/// Execute an already-validated [`AcquisitionBinding`] through the one
/// canonical acquisition entry point,
/// [`crate::utils::evidence::fetch_single_page_with_options`]. Pure
/// delegation — a one-call wrapper, not a reimplementation: no client
/// construction, no transport resolution, no target/onion revalidation
/// happens in this function. `binding.options.transport` (the
/// `TransportPolicy` already resolved by [`bind`]) is passed through
/// exactly as given; there is no second resolution and no opportunity
/// for `execute` to silently choose a different transport than what was
/// bound.
///
/// Returns the underlying function's own `Result<TransportAcquisition,
/// String>` unwrapped and unwrapped only — never re-wrapped in a new
/// error type, so every existing error-message/category guarantee
/// `fetch_single_page_with_options` already makes (secret-safe,
/// deterministic, never fabricating success on failure) carries over
/// verbatim. A network-level failure that actually reached the wire
/// still surfaces as `Ok(TransportAcquisition)` with a non-success
/// `Page` status (that function's own established contract, not
/// something this seam adds or changes); `Err` remains reserved for
/// failures before any request was attempted (a malformed/incompatible
/// transport configuration — the only such condition still reachable
/// here is `TorNotCompiled`, if the binding's `Tor` policy was built in
/// a process without the `transport_tor` feature).
pub async fn execute(binding: AcquisitionBinding) -> Result<TransportAcquisition, String> {
    crate::utils::evidence::fetch_single_page_with_options(&binding.url, binding.options).await
}

/// Convert one completed transport acquisition into parser-neutral discovery
/// material. This performs local materialization only: no request, retry,
/// redirect, browser, DNS, socket, crawl, or transport resolution occurs.
///
/// Status acceptance reuses Spider's canonical parsing-status contract:
/// successful HTTP statuses and `404 Not Found` are usable; every other
/// status is rejected before body materialization. In-memory bytes are copied
/// exactly. Under `balance`, an existing Page spool is reloaded through
/// [`crate::page::Page::ensure_html_loaded_async`], the canonical API for
/// already-acquired spooled content — never through an ad-hoc filesystem
/// reader. The material URL is always [`crate::page::Page::get_url_final`].
///
/// Parser selection, target provenance, and transport provenance are
/// deliberately absent from both input decisions and output shape.
pub async fn materialize(
    acquisition: TransportAcquisition,
) -> Result<DiscoveryMaterial, DiscoveryMaterialError> {
    materialize_page(acquisition.into_page()).await
}

async fn materialize_page(
    page: crate::page::Page,
) -> Result<DiscoveryMaterial, DiscoveryMaterialError> {
    #[cfg(all(feature = "balance", not(feature = "decentralized")))]
    let mut page = page;

    if !crate::utils::valid_parsing_status_code(page.status_code) {
        return Err(DiscoveryMaterialError::UnusableStatus(
            page.status_code.as_u16(),
        ));
    }

    #[cfg(all(feature = "balance", not(feature = "decentralized")))]
    if page.get_bytes().is_none() && !page.ensure_html_loaded_async().await {
        return Err(DiscoveryMaterialError::BodyUnavailable);
    }

    let url = page.get_url_final().to_string();
    let bytes = page
        .get_bytes()
        .ok_or(DiscoveryMaterialError::BodyUnavailable)?
        .to_vec();

    Ok(DiscoveryMaterial { bytes, url })
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

    // -----------------------------------------------------------------
    // `execute` — real, local, deterministic network fixtures. Matches
    // the established blocking-free `tokio::net::TcpListener` fixture
    // convention already used by `spider/tests/transport_tor.rs`. No
    // public network/Tor dependency, no internet-dependent test.
    // -----------------------------------------------------------------

    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    struct HttpFixture {
        addr: SocketAddr,
        hits: Arc<AtomicUsize>,
    }

    impl HttpFixture {
        async fn start() -> Self {
            Self::start_response("200 OK", b"acquisition binding execution fixture ok").await
        }

        async fn start_response(status: &'static str, body: &'static [u8]) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let hits = Arc::new(AtomicUsize::new(0));
            let hits_clone = hits.clone();
            tokio::spawn(async move {
                loop {
                    let (mut stream, _) = match listener.accept().await {
                        Ok(pair) => pair,
                        Err(_) => break,
                    };
                    let hits = hits_clone.clone();
                    tokio::spawn(async move {
                        hits.fetch_add(1, AtomicOrdering::SeqCst);
                        let mut buf = [0_u8; 4096];
                        let _ = stream.read(&mut buf).await;
                        let response = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.write_all(body).await;
                    });
                }
            });
            Self { addr, hits }
        }

        fn hit_count(&self) -> usize {
            self.hits.load(AtomicOrdering::SeqCst)
        }
    }

    /// 1/10: a clearnet + `Default` binding executes through the real,
    /// existing canonical acquisition path — this test never constructs
    /// its own HTTP client; it proves delegation by observing the local
    /// fixture actually get hit and the real `Page`/`TransportAcquisition`
    /// come back.
    #[tokio::test]
    async fn clearnet_default_binding_executes_through_canonical_path() {
        let http = HttpFixture::start().await;
        let target = requested(&format!("http://{}/", http.addr));
        let binding = bind(&target, default_transport()).unwrap();

        let acquisition = execute(binding).await.unwrap();

        assert_eq!(http.hit_count(), 1);
        assert_eq!(acquisition.page().status_code.as_u16(), 200);
        assert!(matches!(
            acquisition.transport(),
            crate::features::transport::TransportPolicy::Default
        ));
    }

    /// A real completed acquisition becomes neutral material without a second
    /// request. Exact non-UTF-8 bytes and the containing URL survive.
    #[tokio::test]
    async fn successful_acquisition_materializes_exact_bytes_without_refetch() {
        const BODY: &[u8] = b"\x00exact\xffbytes";
        let http = HttpFixture::start_response("200 OK", BODY).await;
        let url = format!("http://{}/document", http.addr);
        let binding = bind(&requested(&url), default_transport()).unwrap();
        let acquisition = execute(binding).await.unwrap();
        assert_eq!(http.hit_count(), 1);

        let material = materialize(acquisition).await.unwrap();

        assert_eq!(http.hit_count(), 1, "materialization must not refetch");
        assert_eq!(material.bytes, BODY);
        assert_eq!(material.url, url);
    }

    /// The existing canonical status contract intentionally accepts 404 body
    /// material, matching Spider's established parser-ready Page semantics.
    #[tokio::test]
    async fn not_found_body_is_canonical_usable_material() {
        const BODY: &[u8] = b"404 representation";
        let http = HttpFixture::start_response("404 Not Found", BODY).await;
        let url = format!("http://{}/missing", http.addr);
        let acquisition = execute(bind(&requested(&url), default_transport()).unwrap())
            .await
            .unwrap();

        let material = materialize(acquisition).await.unwrap();

        assert_eq!(material.bytes, BODY);
        assert_eq!(material.url, url);
        assert_eq!(http.hit_count(), 1);
    }

    /// Other non-success/degraded statuses are rejected deterministically and
    /// the typed error contains no URL or body data.
    #[tokio::test]
    async fn unusable_status_is_rejected_with_secret_safe_typed_error() {
        const SECRET: &str = "secret-query-token-7391";
        let http =
            HttpFixture::start_response("500 Internal Server Error", SECRET.as_bytes()).await;
        let url = format!("http://{}/failure?token={SECRET}#fragment", http.addr);
        let acquisition = execute(bind(&requested(&url), default_transport()).unwrap())
            .await
            .unwrap();

        let error = materialize(acquisition).await.unwrap_err();

        assert_eq!(error, DiscoveryMaterialError::UnusableStatus(500));
        assert!(!format!("{error:?}").contains(SECRET));
        assert!(!format!("{error}").contains(SECRET));
        assert_eq!(http.hit_count(), 1);
    }

    /// Missing retained content is a deterministic typed failure, never an
    /// empty successful material.
    #[tokio::test]
    async fn missing_body_is_rejected_deterministically() {
        let mut page = crate::page::build(
            "https://example.test/empty",
            crate::utils::PageResponse {
                status_code: reqwest::StatusCode::OK,
                content: Some(b"initially retained".to_vec()),
                ..Default::default()
            },
        );
        page.set_html_bytes(None);

        assert_eq!(
            materialize_page(page).await,
            Err(DiscoveryMaterialError::BodyUnavailable)
        );
    }

    /// Page's canonical final URL is the material's containing URL; the
    /// requested URL is neither reconstructed nor retained in the payload.
    #[tokio::test]
    async fn final_redirect_destination_is_material_url() {
        let page = crate::page::build(
            "https://example.test/requested",
            crate::utils::PageResponse {
                status_code: reqwest::StatusCode::OK,
                content: Some(b"redirected bytes".to_vec()),
                final_url: Some("https://final.example.test/document".to_string()),
                ..Default::default()
            },
        );

        let material = materialize_page(page).await.unwrap();
        assert_eq!(material.bytes, b"redirected bytes");
        assert_eq!(material.url, "https://final.example.test/document");
    }

    /// Structural proof: the conversion result exhaustively contains only
    /// bytes and URL. No parser intent, target kind, transport, SourceItem, or
    /// evidence field can be copied into this shape unnoticed.
    #[test]
    fn material_output_shape_is_parser_transport_and_candidate_neutral() {
        let material = DiscoveryMaterial {
            bytes: vec![1, 2, 3],
            url: "https://example.test/document".to_string(),
        };
        let DiscoveryMaterial { bytes, url } = material;
        assert_eq!(bytes, vec![1, 2, 3]);
        assert_eq!(url, "https://example.test/document");
    }

    /// Canonical balance spool materialization reloads already-acquired exact
    /// bytes locally and performs no network operation.
    #[cfg(all(feature = "balance", not(feature = "decentralized")))]
    #[tokio::test]
    async fn previously_spooled_body_materializes_through_page_api() {
        const BODY: &[u8] = b"already acquired spooled discovery bytes";
        let mut page = crate::page::build(
            "https://example.test/spooled",
            crate::utils::PageResponse {
                status_code: reqwest::StatusCode::OK,
                content: Some(BODY.to_vec()),
                ..Default::default()
            },
        );
        assert!(page.spool_html_to_disk());
        assert!(page.get_bytes().is_none());
        assert!(page.is_html_on_disk());

        let material = materialize_page(page).await.unwrap();

        assert_eq!(material.bytes, BODY);
        assert_eq!(material.url, "https://example.test/spooled");
    }

    #[cfg(feature = "transport_tor")]
    mod tor_execution {
        use super::*;

        #[derive(Clone, Copy, PartialEq, Eq)]
        enum SocksBehavior {
            Splice,
            Fail,
        }

        struct SocksFixture {
            addr: SocketAddr,
            connect_count: Arc<AtomicUsize>,
        }

        impl SocksFixture {
            async fn start(splice_to: Option<SocketAddr>, behavior: SocksBehavior) -> Self {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap();
                let connect_count = Arc::new(AtomicUsize::new(0));
                let connect_count_clone = connect_count.clone();
                tokio::spawn(async move {
                    loop {
                        let (stream, _) = match listener.accept().await {
                            Ok(pair) => pair,
                            Err(_) => break,
                        };
                        let connect_count = connect_count_clone.clone();
                        tokio::spawn(Self::serve_one(stream, splice_to, behavior, connect_count));
                    }
                });
                Self {
                    addr,
                    connect_count,
                }
            }

            async fn serve_one(
                mut stream: tokio::net::TcpStream,
                splice_to: Option<SocketAddr>,
                behavior: SocksBehavior,
                connect_count: Arc<AtomicUsize>,
            ) {
                let _ =
                    Self::serve_one_inner(&mut stream, splice_to, behavior, connect_count).await;
            }

            async fn serve_one_inner(
                stream: &mut tokio::net::TcpStream,
                splice_to: Option<SocketAddr>,
                behavior: SocksBehavior,
                connect_count: Arc<AtomicUsize>,
            ) -> std::io::Result<()> {
                let mut header = [0_u8; 2];
                stream.read_exact(&mut header).await?;
                let nmethods = header[1] as usize;
                let mut methods = vec![0_u8; nmethods];
                stream.read_exact(&mut methods).await?;
                stream.write_all(&[0x05, 0x00]).await?;

                let mut req_head = [0_u8; 4];
                stream.read_exact(&mut req_head).await?;
                match req_head[3] {
                    0x01 => {
                        let mut addr = [0_u8; 4];
                        stream.read_exact(&mut addr).await?;
                    }
                    0x03 => {
                        let mut len_buf = [0_u8; 1];
                        stream.read_exact(&mut len_buf).await?;
                        let mut name = vec![0_u8; len_buf[0] as usize];
                        stream.read_exact(&mut name).await?;
                    }
                    0x04 => {
                        let mut addr = [0_u8; 16];
                        stream.read_exact(&mut addr).await?;
                    }
                    _ => return Ok(()),
                }
                let mut port_buf = [0_u8; 2];
                stream.read_exact(&mut port_buf).await?;

                connect_count.fetch_add(1, AtomicOrdering::SeqCst);

                if behavior == SocksBehavior::Fail {
                    stream
                        .write_all(&[0x05, 0x01, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                        .await?;
                    return Ok(());
                }

                let Some(splice_to) = splice_to else {
                    stream
                        .write_all(&[0x05, 0x01, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                        .await?;
                    return Ok(());
                };

                stream
                    .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                    .await?;

                let mut upstream = tokio::net::TcpStream::connect(splice_to).await?;
                let (mut client_read, mut client_write) = stream.split();
                let (mut upstream_read, mut upstream_write) = upstream.split();
                let client_to_upstream = tokio::io::copy(&mut client_read, &mut upstream_write);
                let upstream_to_client = tokio::io::copy(&mut upstream_read, &mut client_write);
                let _ = tokio::try_join!(client_to_upstream, upstream_to_client);
                Ok(())
            }

            fn connect_count(&self) -> usize {
                self.connect_count.load(AtomicOrdering::SeqCst)
            }
        }

        /// 2/10: an onion + `Tor` binding executes through the real,
        /// existing canonical Tor acquisition path when `transport_tor`
        /// is enabled — reaches the target exclusively via SOCKS.
        #[tokio::test]
        async fn onion_tor_binding_executes_through_canonical_tor_path() {
            let http = HttpFixture::start().await;
            let socks = SocksFixture::start(Some(http.addr), SocksBehavior::Splice).await;
            let target = requested(&format!(
                "http://onion-exec-test-fixture1234567.onion:{}/",
                http.addr.port()
            ));
            let binding =
                bind(&target, tor_transport(&format!("socks5h://{}", socks.addr))).unwrap();

            let acquisition = execute(binding).await.unwrap();

            assert_eq!(socks.connect_count(), 1);
            assert_eq!(http.hit_count(), 1);
            assert_eq!(acquisition.page().status_code.as_u16(), 200);
            assert!(matches!(
                acquisition.transport(),
                crate::features::transport::TransportPolicy::Tor(_)
            ));
        }

        /// 4/5: execution uses `binding.options.transport` exactly as
        /// already resolved by `bind` — never re-resolves it. Proven by
        /// running the *same* onion target through both a `Default`-
        /// bound (rejected at `bind`, never reaching `execute`) and a
        /// `Tor`-bound path, and confirming the Tor path's SOCKS fixture
        /// is the only one ever contacted — nothing here silently
        /// re-derives transport from the target URL a second time.
        #[tokio::test]
        async fn execute_uses_resolved_transport_exactly_no_second_resolution() {
            let http = HttpFixture::start().await;
            let socks = SocksFixture::start(Some(http.addr), SocksBehavior::Splice).await;
            let onion_url = format!(
                "http://onion-exec-resolve-test123456.onion:{}/",
                http.addr.port()
            );
            let target = requested(&onion_url);

            // Default transport never even produces a valid binding —
            // there is nothing for `execute` to run.
            assert!(bind(&target, default_transport()).is_err());
            assert_eq!(socks.connect_count(), 0);
            assert_eq!(http.hit_count(), 0);

            // The Tor-bound path executes, and only the SOCKS fixture is
            // ever contacted — proving `execute` acted on the transport
            // that was actually resolved, not one it derived itself.
            let binding =
                bind(&target, tor_transport(&format!("socks5h://{}", socks.addr))).unwrap();
            execute(binding).await.unwrap();
            assert_eq!(socks.connect_count(), 1);
            assert_eq!(http.hit_count(), 1);
        }

        /// 8: acquisition errors propagate correctly — a SOCKS-layer
        /// failure never falls back to a direct clearnet request, and
        /// (matching the established "network failure is truthful
        /// degraded-status evidence, not a process `Err`" contract —
        /// see `discovery::fetch_tests` in the CLI, and this crate's own
        /// one-shot Tor regression) `execute` still returns `Ok`, with a
        /// non-success status on the returned `Page`.
        #[tokio::test]
        async fn socks_failure_propagates_as_truthful_degraded_status_not_process_error() {
            let http = HttpFixture::start().await;
            let socks = SocksFixture::start(None, SocksBehavior::Fail).await;
            let target = requested(&format!(
                "http://onion-exec-fail-test1234567890.onion:{}/",
                http.addr.port()
            ));
            let binding =
                bind(&target, tor_transport(&format!("socks5h://{}", socks.addr))).unwrap();

            let acquisition = execute(binding).await.unwrap();

            assert_eq!(
                http.hit_count(),
                0,
                "a SOCKS failure must never fall back to reaching the target directly"
            );
            assert_ne!(acquisition.page().status_code.as_u16(), 200);
        }
    }

    /// 8 (feature-gating half): with `transport_tor` NOT compiled in,
    /// executing a `Tor`-bound binding propagates the existing
    /// `TorNotCompiled` failure verbatim — `execute` neither invents a
    /// new error nor silently falls back to `Default`.
    #[cfg(not(feature = "transport_tor"))]
    #[tokio::test]
    async fn tor_binding_without_transport_tor_feature_propagates_tor_not_compiled() {
        let target = requested("http://onion-no-feature-test1234567.onion/");
        let binding = bind(&target, tor_transport("socks5h://127.0.0.1:9050")).unwrap();

        let error = execute(binding).await.unwrap_err();

        assert!(
            error.to_lowercase().contains("transport_tor"),
            "error must name the missing capability: {error}"
        );
    }

    /// 6/7/10 (structural): `execute`'s signature carries no
    /// `DiscoveryTargetKind`, synthesizes no `SourceItem`, and
    /// synthesizes no `DiscoveryMaterial` — it consumes exactly
    /// `AcquisitionBinding` and returns exactly `TransportAcquisition`,
    /// both pre-existing canonical types. This is a compile-level fact
    /// (the function signature itself), reasserted here so it stays
    /// visible in the test suite rather than only in the source.
    #[test]
    fn execute_signature_carries_only_canonical_binding_and_acquisition_types() {
        fn _type_check(
            binding: AcquisitionBinding,
        ) -> impl std::future::Future<Output = Result<TransportAcquisition, String>> {
            execute(binding)
        }
    }
}
