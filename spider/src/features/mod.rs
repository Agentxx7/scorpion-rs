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
/// Passive, provider-neutral browser challenge detector: real Chrome page →
/// evidence-based detection → canonical [`browser_challenge::BrowserChallengeSnapshot`].
/// No provider routing, no solving, no browser mutation.
#[cfg(feature = "chrome")]
pub mod browser_challenge_detection;
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
/// Canonical Spider acquisition adapter for `spider_agent` research.
#[cfg(all(feature = "agent", feature = "evidence"))]
pub mod agent_acquisition;
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
/// Canonical audit fact/finding contract:
/// `ACQUIRED -> OBSERVED -> EVIDENCED -> DERIVED FINDING`. `PageFacts` is
/// a transient, deterministic, network-free projection of one acquired
/// `Page` (canonical link observations, closed-allowlist audit response
/// headers, effective/observed status); `EvidencedPageFacts` binds those
/// facts to the exact `EvidenceRef` recording of the same `Page` — never
/// by index/ordering/URL-string equality, only by construction from the
/// same acquired value. `Finding` is a content-addressed, immutable,
/// evidence-linked derived record (Track 6/9's own
/// `TransformLineageId`/`ChangeEventId` construction and
/// `DomainPersistence::append_history` idempotent-duplicate pattern,
/// reused verbatim — no second persistence backend). Exactly one
/// production rule lives here today: `SEO_CANONICAL_MISSING`. No
/// analyzer in this module performs network acquisition (that remains
/// `crate::utils::evidence::fetch_single_page`'s sole responsibility, the
/// same one-shot primitive every other evidence-first caller uses); no
/// discovery/search type (`SearchProvider`/`SearchResult`/
/// `SearchResults`) is imported or reachable — discovery may select a
/// URL, only acquisition can establish page evidence. Requires
/// `evidence` and `disk`, exactly like `change_detection`, which this
/// module's persistence pattern mirrors. No CLI/API/MCP/Web Console
/// surface, no site-wide analytics, no network/Nmap capability, no AI —
/// see this module's own doc comment for the full scope firewall.
#[cfg(all(feature = "evidence", feature = "disk"))]
pub mod audit;
/// Canonical authenticated-session lifecycle: `AuthSessionState`
/// (Active/Paused/Invalidated), `PauseSession`/`ResumeSession`/
/// `InvalidateSession` transitions (built on `domain_state::Transition`),
/// and — behind `disk` + `serde` — `create_session`/
/// `apply_session_transition` persisting through `DomainPersistence`.
/// Identity (`AuthSessionId`) lives in `features/identity.rs`. Always
/// available; no feature gate on the state/transition vocabulary itself.
/// See `SCORPION.md` §5.
pub mod auth_session;
/// Canonical change detection: `ChangeResult` (the truthful outcome of
/// comparing two durable evidence records for the same watch) and
/// `ChangeEvent` (the durable, append-only historical record of a
/// detected comparison). Compares only evidence a watch's own history
/// (`features/watch.rs`) actually associates with it; never reduces an
/// uncomparable pair to "unchanged". Reuses `sha256_hex`-derived hash
/// fields already present on `EvidenceBundle` and Track 6's
/// content-addressed idempotent-append persistence pattern — no new
/// hashing/fingerprint architecture, no second Evidence/Watch model, no
/// scheduler of its own (`features/watch_schedule.rs` remains the sole
/// scheduler/execution owner). Requires `evidence` and `disk` (like
/// `watch`, which it reads). See `SCORPION_SDD.md` §5.2.
#[cfg(all(feature = "evidence", feature = "disk"))]
pub mod change_detection;
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
/// Canonical, neutral runtime binding to the one shared `DomainPersistence`
/// store every Scorpion interface may open — `SCORPION_DOMAIN_DB`
/// (preferred) / `RESEARCH_EVIDENCE_DB` (legacy fallback, explicitly
/// reconciled, not silently aliased). Resolves a path and opens a handle;
/// decides no domain semantics of its own. See
/// `SCORPION_CANONICAL_SHARED_DOMAIN_PERSISTENCE_RUNTIME_BINDING_001` and
/// `SCORPION_ARCHITECTURE.md`.
#[cfg(feature = "disk")]
pub mod domain_runtime;
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
/// `ResearchId`, `WatchId`, and `AuthSessionId`. Identity only — no
/// persistence, no state/lifecycle, no domain object. Always available. See
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
/// PaliGemma CAPTCHA requests (SigLIP + Gemma), sharing the same canonical
/// CAPTCHA contract as every other provider.
#[cfg(feature = "local_paligemma")]
pub mod paligemma_captcha;
/// Request-isolated generation sessions for the native Candle PaliGemma
/// runtime. Every session owns fresh KV state while immutable weight backend
/// resources remain factory-owned.
#[cfg(feature = "local_paligemma")]
pub mod paligemma_generation;
/// Offline CPU/F32 production runtime for the pinned PaliGemma-3b-mix-224
/// model.
#[cfg(feature = "local_paligemma")]
pub mod paligemma_runtime;
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
/// Durable owner for one canonical research invocation: claims a fresh
/// `ResearchId`, forces durable canonical source acquisition, and records
/// evidence accounting, Source-N bindings, counts, and terminal outcome.
#[cfg(all(feature = "agent_acquisition", feature = "disk"))]
pub mod research_session;
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

/// The canonical Watch model: `WatchDefinition` (wraps the existing
/// `DiscoveryTarget` — no new `WatchTarget`/`WatchSpec`) and `WatchState`
/// (`Active`/`Stopped`, built on `domain_state::CurrentState`/
/// `Transition`), using the existing `WatchId`. Persists through
/// `DomainPersistence`: the definition immutably (`append_history`), the
/// current lifecycle state via compare-and-swap (`write_current`) plus
/// an immutable historical record of each superseded state
/// (`append_history`). Requires `evidence` (for `EvidenceRef`) and `disk`
/// (for `DomainPersistence`). See `SCORPION_SDD.md` §5.2.
#[cfg(all(feature = "evidence", feature = "disk"))]
pub mod watch;

/// Canonical scheduling semantics for `WatchDefinition` and the execution
/// path for one scheduled watch run: `WatchSchedule` (cadence, validated
/// via the existing `async_job::Schedule` cron primitive — never
/// `website::CronType`, which is a what-to-run selector, not cadence
/// syntax), plus `execute_scheduled_watch_run`, which reuses
/// `acquisition_binding` for acquisition, `utils::evidence` for the
/// durable `EvidenceRef`, and `features::watch::apply_watch_transition`
/// for the resulting `WatchState` transition — `WatchState` itself
/// remains owned exclusively by Track 7. Requires `evidence` and `disk`
/// (like `watch`) plus `cron` (for the cadence primitive).
#[cfg(all(feature = "evidence", feature = "disk", feature = "cron"))]
pub mod watch_schedule;

/// Canonical, purely observational health for the complete watch
/// pipeline (`WatchDefinition → Scheduling → acquisition → EvidenceRef →
/// WatchState → ChangeResult/ChangeEvent`): `HealthStatus`
/// (Unknown/Healthy/Degraded/Failed, source-justified per dimension) and
/// `ChangeDetectionReadiness` (structurally distinct `TypeLevelReady`
/// vs. `ProductionExercised` variants, so the two can never be
/// conflated). Reads only — never calls `apply_watch_transition`,
/// `execute_scheduled_watch_run`, `define_watch_schedule`, or
/// `detect_and_record_change`. Requires `evidence`+`disk`+`cron` (it
/// reads `watch_schedule`). See `SCORPION_SDD.md` §5.2.
#[cfg(all(feature = "evidence", feature = "disk", feature = "cron"))]
pub mod watch_health;

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
