/// Chrome utils
#[cfg(feature = "chrome")]
pub mod chrome;
#[cfg(feature = "chrome")]
/// Chrome launch args.
pub(crate) mod chrome_args;
/// Common modules for Chrome
pub mod chrome_common;
#[cfg(feature = "real_browser")]
/// Viewport
pub mod chrome_viewport;

/// WebDriver utils
#[cfg(feature = "webdriver")]
pub mod webdriver;
#[cfg(feature = "webdriver")]
/// WebDriver launch args.
pub(crate) mod webdriver_args;
/// Common modules for WebDriver
pub mod webdriver_common;

/// Decentralized header handling
#[cfg(feature = "decentralized_headers")]
pub mod decentralized_headers;
/// Disk options
pub mod disk;
/// URL globbing
#[cfg(feature = "glob")]
pub mod glob;
/// OpenAI
#[cfg(feature = "openai")]
pub mod openai;
/// Common modules for OpenAI
pub mod openai_common;

/// Gemini
#[cfg(feature = "gemini")]
pub mod gemini;
/// Common modules for Gemini
pub mod gemini_common;

/// Immutable browser challenge snapshot, revalidation and exact action seam.
#[cfg(feature = "chrome")]
pub mod browser_challenge;
/// Provider-neutral CAPTCHA solver capability.
pub mod captcha;
/// Thin canonical binding between browser attempts and CAPTCHA providers.
#[cfg(feature = "chrome")]
pub mod captcha_browser;
/// Provider-neutral governance contract for acquiring, independently
/// annotating, splitting and immutably freezing CAPTCHA evaluation corpora.
pub mod captcha_evaluation_corpus;
/// Canonical Chromium frame-context identity seam: `FrameId -> TargetId ->
/// SessionId -> ExecutionContextId -> frame DOM identity -> frame owner`.
#[cfg(feature = "chrome")]
pub mod frame_context;

/// Solve all.
pub mod solvers;

/// Binds a validated `DiscoveryTarget` to Scorpion's existing canonical
/// acquisition/transport request vocabulary (`AcquisitionOptions` /
/// `TransportRequest`) — the smallest seam between planning and a
/// caller's own, separate execution. Zero acquisition; terminates in an
/// `AcquisitionBinding`, never itself executable. Requires the
/// `evidence` feature (the same feature the vocabulary it binds into
/// already requires); does not require `transport_tor`.
#[cfg(feature = "evidence")]
pub mod acquisition_binding;
/// Pure provider-neutral binding from resolved artifact metadata to future
/// download execution intent. Performs no acquisition or filesystem work.
pub mod artifact_download_binding;
/// Canonical execution of an already-resolved `ArtifactDownloadBinding`:
/// streams the remote artifact through the canonical transport streaming
/// request seam straight to a caller-owned destination on disk, hashing
/// while streaming, without ever materializing the full body in memory.
/// Requires `evidence` (for the `sha2` dependency) and, like the
/// streaming transport seam it consumes, is unavailable under `wreq`.
#[cfg(all(feature = "evidence", not(feature = "wreq")))]
pub mod artifact_download_execution;
/// Provider-neutral metadata for versioned repository artifacts. Always
/// available and performs no acquisition, download, parsing, or verification.
pub mod artifact_reference;
/// Canonical authenticated-session lifecycle: `AuthSessionState`
/// (Active/Paused/Invalidated), `PauseSession`/`ResumeSession`/
/// `InvalidateSession` transitions (built on `domain_state::Transition`),
/// and — behind `disk` + `serde` — `create_session`/
/// `apply_session_transition` persisting through `DomainPersistence`.
/// Identity (`AuthSessionId`) lives in `features/identity.rs`. Always
/// available; no feature gate on the state/transition vocabulary itself.
/// See `SCORPION.md` §5.
pub mod auth_session;
/// `DiscoveryTarget`: the smallest canonical planning boundary for
/// discovery pointers (sitemap index child sitemaps, robots.txt-declared
/// sitemaps, caller/request-supplied URLs) — URLs to acquire *later*,
/// never `SourceItem` content candidates and never something already
/// fetched. Zero acquisition; terminates in targets. Always available —
/// the module itself has no feature gate, though its sitemap/
/// robots_sitemap `PlanningInput` variants are individually gated behind
/// their respective existing features.
pub mod discovery_target;
/// Canonical persistence seam for Scorpion domain state: `DomainPersistence`.
/// Stores opaque identity-keyed state — compare-and-swap current state,
/// append-only historical records — and decides no domain semantics.
/// Reuses the crate's existing `sqlx`/SQLite dependency (`features/disk.rs`);
/// gated behind the same `disk` feature, no new storage stack. See
/// `SCORPION_SDD.md` §5.2.
#[cfg(feature = "disk")]
pub mod domain_persistence;
/// Canonical state/transition semantics for persisted Scorpion domain
/// objects: `CurrentState`, `HistoryEntry`, `HistoryLog`, `Transition`.
/// Semantics only — no persistence, no concrete state machine, no product
/// model. Always available; no feature gate. Built on `identity`. See
/// `SCORPION_SDD.md` §5.2.
pub mod domain_state;
/// RSS and Atom feed parsing and normalization.
#[cfg(feature = "feed")]
pub mod feed;
/// Provider-native GitHub repository discovery through the official REST API.
#[cfg(feature = "source_github")]
pub mod github_source_provider;
/// Provider-native Hugging Face model discovery through the official Hub API.
#[cfg(feature = "source_hugging_face")]
pub mod hugging_face_source_provider;
/// Canonical identity for persisted Scorpion domain objects: `EvidenceId`,
/// `WatchId`. Identity only — no persistence, no state/lifecycle, no
/// domain object. Always available; no feature gate. See
/// `SCORPION.md` §3 (`EvidenceId`) and `SCORPION_SDD.md` §5.2 (`WatchId`).
pub mod identity;
/// Provider-neutral immutable local multi-file model installation, identity,
/// qualification and offline runtime lifecycle contract.
pub mod local_model;
/// Google News Sitemap parsing and normalization.
#[cfg(feature = "news_sitemap")]
pub mod news_sitemap;
/// Manual/request-supplied onion seed URL discovery (classification and
/// `SourceItem` normalization only — zero target acquisition). Available
/// unconditionally, independent of the `transport_tor` feature: this is
/// URL classification, not Tor networking.
pub mod onion_seed;
/// Canonical provider adapter for executable, empirically unqualified local
/// PaliGemma CAPTCHA requests — a genuinely different model family from
/// Qwen3-VL (SigLIP + Gemma), sharing the same canonical CAPTCHA contract.
#[cfg(feature = "local_paligemma")]
pub mod paligemma_captcha;
/// Request-isolated generation sessions for the native Candle PaliGemma
/// runtime, mirroring `qwen3_vl_generation`'s own design.
#[cfg(feature = "local_paligemma")]
pub mod paligemma_generation;
/// Offline CPU/F32 production runtime for the pinned PaliGemma-3b-mix-224
/// model.
#[cfg(feature = "local_paligemma")]
pub mod paligemma_runtime;
/// Canonical provider adapter for executable, empirically unqualified local
/// Qwen3-VL CAPTCHA requests.
#[cfg(feature = "local_qwen3_vl")]
pub mod qwen3_vl_captcha;
/// Request-isolated generation sessions for the native Candle Qwen3-VL
/// runtime. Every session owns fresh KV state while immutable weight backend
/// resources remain factory-owned.
#[cfg(feature = "local_qwen3_vl")]
pub mod qwen3_vl_generation;
/// Offline CPU/F32 production runtime for the pinned Qwen3-VL-2B model.
#[cfg(feature = "local_qwen3_vl")]
pub mod qwen3_vl_runtime;
/// `ResearchScope`: the smallest canonical declarative discovery-scope
/// boundary (onion seeds / already-produced candidates only — never
/// fetched document bytes), plus the `discover` orchestration seam that
/// normalizes a `ResearchScope` together with parser-neutral,
/// already-acquired `DiscoveryMaterial` (document bytes + containing URL)
/// paired with explicit `DiscoveryParserIntent` into ordered `SourceItem`
/// candidates. Zero acquisition;
/// terminates in candidates. Always available — the module itself has
/// no feature gate. `DiscoveryMaterial` itself is always available; parser
/// intent variants are individually gated behind their respective existing
/// feed/sitemap/news_sitemap features.
pub mod research_scope;
/// robots.txt `Sitemap:` directive discovery.
#[cfg(feature = "robots_sitemap")]
pub mod robots_sitemap;
/// Ephemeral, non-serializable, fully value-redacted request headers for
/// future execution bindings. Always available and performs no network or
/// persistence work.
pub mod secret_request_headers;
/// Standard sitemap urlset and sitemapindex parsing and normalization.
#[cfg(feature = "sitemap")]
pub mod sitemap;
/// Generic source-discovery vocabulary.
pub mod source;
/// Parser-, acquisition-, and transport-neutral source-provider identity,
/// output, descriptor, and deterministic metadata-registry vocabulary.
/// Always available; concrete provider execution remains a later frontier.
pub mod source_provider;
/// Canonical content/transform lineage: `source input → transformation →
/// output`, recorded immutably through `DomainPersistence`.
/// `TransformLineageId` is content-addressed (deterministic SHA-256 of
/// the input/transformation/output triple) — a different construction
/// than `features/identity.rs`'s three randomly-minted identity types,
/// which is why it lives here instead. Does not redefine, shadow, or
/// import `configuration::Fingerprint` (an unrelated browser
/// anti-detection concept). Requires `evidence` (for `sha256_hex`) and
/// `disk` (for `DomainPersistence`/`EvidenceRef`).
#[cfg(all(feature = "evidence", feature = "disk"))]
pub mod transform_lineage;

/// Canonical HTTP transport policy (`Default` / Tor-over-SOCKS5h), with
/// fail-closed `.onion` protection and transport-pinned redirects.
pub mod transport;

#[cfg(all(not(feature = "simd"), feature = "openai"))]
pub(crate) use serde_json;
#[cfg(all(feature = "simd", feature = "openai"))]
pub(crate) use sonic_rs as serde_json;

/// Automation scripts.
pub mod automation;

/// Web search integration.
#[cfg(feature = "search")]
pub mod search;
/// Search provider implementations.
#[cfg(feature = "search")]
pub mod search_providers;
