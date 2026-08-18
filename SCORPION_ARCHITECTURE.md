# Scorpion — Canonical Architecture & Guardrail Contract

**Status:** ACTIVE — enforced by architecture tests and code review.

**Baseline:** `3031b1a25b2cd1d1207f1f039a5c5e6bb36bcb24`

This document is the machine-readable source of truth for Scorpion's canonical
architecture. It records what is canonical, what is compatibility-only, what is
forbidden, and how future frontiers must be scoped.

Companion documents:

- `SCORPION_SDD.md` — the Software Design Specification: layer model,
  dependency direction, ownership, state model, security ownership, error
  ownership.
- `SCORPION_PROCESS.md` — the frontier lifecycle, SDD/TDD process, and
  two-branch process for architecture-critical changes, with templates in
  `docs/frontier/templates/`.

---

## 1. Core Principle

Scorpion is **one canonical engine/platform** with multiple capabilities and
thin interfaces.

- Canonical core owns behavior and state.
- Interfaces (CLI, MCP, future API/Web, future TUI, library consumers) call the
  same canonical core capabilities.
- Interfaces must not implement independent business, acquisition, transport,
  evidence, artifact, watch, or provider logic.

---

## 2. Classification Model

Every architecture-relevant implementation is classified as exactly one of:

| Classification | Meaning | New Development |
|---|---|---|
| **CANONICAL** | Approved Scorpion path. Single implementation. | **Must use** |
| **UPSTREAM_COMPAT** | Retained because Spider/upstream functionality requires it. | **Must not extend**; new Scorpion capabilities must not depend on it |
| **LEGACY** | Older Scorpion path superseded by canonical implementation. | **Must not receive new features** |
| **REJECTED** | Known-invalid architecture or implementation. | **Must not remain available** as fallback or alternate execution |
| **UNKNOWN** | Ownership or architectural status cannot yet be proven. | **Must not be silently promoted** to canonical |

---

## 3. Canonical Ownership Map

### 3.1 Research / Discovery Orchestration

| Area | Canonical Owner | Allowed Dependencies | Forbidden Dependencies | Public Execution Seam | Upstream Compatibility Paths |
|---|---|---|---|---|---|
| Research scope planning | `spider/src/features/research_scope.rs` | `discovery_target`, `source`, parser intents | Acquisition, transport, network | `discover()` | None |
| Discovery target planning | `spider/src/features/discovery_target.rs` | `source`, `sitemap`, `robots_sitemap` | Acquisition, transport | `plan()` | None |
| Onion seed classification | `spider/src/features/onion_seed.rs` | `transport::is_onion_url` | Local DNS, network | `normalize_onion_seed()` | None |

### 3.2 Sources / Adapters

| Area | Canonical Owner | Allowed Dependencies | Forbidden Dependencies | Public Execution Seam | Upstream Compatibility Paths |
|---|---|---|---|---|---|
| Provider-neutral vocabulary | `spider/src/features/source_provider.rs` | `source` | Network, transport | `ProviderRegistry` | None |
| GitHub discovery | `spider/src/features/github_source_provider.rs` | `source_provider`, `source`, `transport::execute_streaming_request`, `secret_request_headers` | Independent client construction | `search_repositories()` | None |
| Hugging Face discovery | `spider/src/features/hugging_face_source_provider.rs` | `source_provider`, `artifact_reference`, `transport::execute_streaming_request`, `secret_request_headers` | Independent client construction | `search_models()` / `discover_artifacts()` | None |
| Feed parsing | `spider/src/features/feed.rs` | `source` | Network | `parse()` | None |
| Sitemap parsing | `spider/src/features/sitemap.rs` | `source` | Network | `parse()` | None |
| News sitemap parsing | `spider/src/features/news_sitemap.rs` | `source` | Network | `parse()` | None |
| Robots sitemap discovery | `spider/src/features/robots_sitemap.rs` | `source` | Network | `parse()` | `robotparser` (crawl policy) |
| Search (self-hosted) | `spider/src/features/search_providers/searxng.rs` | `search` | Neutral transport | `search()` | None |
| Search (commercial) | `spider/src/features/search_providers/{serper,brave,bing,tavily}.rs` | `search` | Neutral transport | `search()` | `spider_agent/src/search/` (LEGACY duplicate) |

### 3.3 Acquisition

| Area | Canonical Owner | Allowed Dependencies | Forbidden Dependencies | Public Execution Seam | Upstream Compatibility Paths |
|---|---|---|---|---|---|
| One-shot fetch/evidence | `spider/src/utils/evidence.rs` | `Website` (UPSTREAM_COMPAT boundary primitive), `transport` | Independent client construction | `fetch_single_page_with_options()` | `fetch_single_page()` (Default-transport implementation behind the canonical seam — not a caller-selectable alternate) |
| Crawl/scrape orchestration seam | `spider/src/website.rs` — **methods only**: `Website::with_transport()`, `crawl_raw()`, `scrape()` | `transport`, `page` | New transport stacks | `Website::with_transport()` + `crawl_raw()`/`scrape()` | `configure_base_client` (UPSTREAM_COMPAT — transitive implementation of this seam, never directly callable from canonical code) |
| Streaming artifact download | `spider/src/features/artifact_download_execution.rs` | `transport::execute_streaming_request`, `uring_fs` | `Website`, `Page`, independent clients | `execute()` | None |
| Acquisition binding | `spider/src/features/acquisition_binding.rs` | `discovery_target`, `evidence` | Network, transport | `bind()`/`execute()` | None |

**Clarification on `website.rs`:** The `Website` type and its crawl/scrape methods are the canonical public seam for multi-page acquisition. The internal client construction (`configure_base_client`, proxy rotation, legacy redirect policies) is UPSTREAM_COMPAT — retained for upstream parity, not to be extended by new Scorpion capabilities. A canonical boundary primitive may internally execute upstream-compatible machinery; the boundary primitive and the machinery are classified separately. Upstream machinery is therefore an **implementation dependency behind an approved boundary** — it is transitively executed underneath the seam, but canonical code never directly selects it, never calls it as an alternate path, and never depends on it as fallback. See `SCORPION_SDD.md` §3 for the three-category dependency model.

### 3.4 Transport

| Area | Canonical Owner | Allowed Dependencies | Forbidden Dependencies | Public Execution Seam | Upstream Compatibility Paths |
|---|---|---|---|---|---|
| Transport policy | `spider/src/features/transport.rs` | `reqwest` | `wreq`, `cache_request` stacks for Tor | `TransportPolicy` | None |
| Tor client construction | `spider/src/features/transport.rs` | `reqwest` | Legacy proxy rotation, `socks://` rewrite | `build_tor_client()` | None |
| Streaming request | `spider/src/features/transport.rs` | `reqwest`, `secret_request_headers` | `Website`, `Page` | `execute_streaming_request()` | None |
| Onion classification | `spider/src/features/transport.rs` | `url` | Local DNS, string matching | `is_onion_url()` | None |
| Target validation | `spider/src/features/transport.rs` | `url`, `is_onion_url` | Network | `validate_target()` | None |
| Redirect policy | `spider/src/features/transport.rs` | `reqwest`, `is_ssrf_redirect` | `setup_redirect_policy` | `pin_redirect_policy()` | `setup_strict_policy`/`setup_redirect_policy` (UPSTREAM_COMPAT) |
| SSRF guard | `spider/src/website.rs` — **method only**: `is_ssrf_redirect()` | `url` | New redirect policies | `is_ssrf_redirect()` | Shared with canonical transport |

**Clarification on `website.rs` SSRF:** The `is_ssrf_redirect()` method is the shared SSRF engine used by both the canonical `pin_redirect_policy` and the legacy `setup_*_policy` redirect policies. It is classified as a shared primitive — canonical transport depends on it, but it lives in `website.rs` because it was originally an upstream guard elevated to canonical-shared status.

### 3.5 Artifact Reference / Binding / Execution

| Area | Canonical Owner | Allowed Dependencies | Forbidden Dependencies | Public Execution Seam | Upstream Compatibility Paths |
|---|---|---|---|---|---|
| Artifact metadata | `spider/src/features/artifact_reference.rs` | `source_provider` | Network, I/O | `ArtifactReference` | None |
| Download binding | `spider/src/features/artifact_download_binding.rs` | `artifact_reference`, `transport` | Network, I/O | `bind()` | None |
| Download execution | `spider/src/features/artifact_download_execution.rs` | `artifact_download_binding`, `transport`, `uring_fs` | `Website`, `Page`, independent clients | `execute()` | None |
| Hashing | `spider/src/utils/evidence.rs` (`sha256_hex`) / `artifact_download_execution.rs` (streaming) | `sha2` | Network | `sha256_hex()` / running hash | None |
| Filesystem | `spider/src/utils/uring_fs.rs` | `tokio` | Blocking I/O | `StreamingWriter`, `remove_file` | Tokio fallback (UPSTREAM_COMPAT) |

### 3.6 Evidence / Provenance

| Area | Canonical Owner | Allowed Dependencies | Forbidden Dependencies | Public Execution Seam | Upstream Compatibility Paths |
|---|---|---|---|---|---|
| Evidence bundle construction | `spider/src/utils/evidence.rs` | `Page` (UPSTREAM_COMPAT), `transport` | Independent provenance stamping | `build_evidence()` | `spider_mcp::evidence` shim (UPSTREAM_COMPAT) |
| Durable evidence ledger | `spider/src/utils/evidence.rs` | `features/identity.rs` (`EvidenceId`), `features/domain_persistence.rs` (`disk` feature) | `write_current` (evidence has no current state), a second evidence store | `record_evidence()`, `read_evidence()`, `EvidenceRef` | None |
| Transport provenance stamping | `spider/src/page.rs` — **field/method only**: `Page::transport`, `Page::transport()`, `Page::backend_provenance()`, `Page::response_origin()` | `ACQUISITION_TRANSPORT_SCOPE` | Caller-supplied policy | `Page::transport()` | None |
| Acquisition scope | `spider/src/features/transport.rs` | `tokio` | Independent provenance | `ACQUISITION_TRANSPORT_SCOPE` | None |

**Clarification on `page.rs`:** `Page` is the upstream Spider response/evidence vocabulary. The `Page::transport` provenance field and its getter are canonical Scorpion additions — the single source of truth for transport provenance. The rest of `Page` (structure, `page::build`, legacy link extraction, anti-bot detection) is UPSTREAM_COMPAT. The canonical evidence seam is `spider/src/utils/evidence.rs`; `Page` is consumed by it as an upstream primitive, not owned by it.

**Clarification on the durable evidence ledger:** `EvidenceBundle` (already the one canonical evidence model) gained an `id: Option<EvidenceId>` field and `backend_provenance`/`response_origin` fields — both read from `Page::backend_provenance()`/`Page::response_origin()` (`spider_transport::BackendProvenance`/`ResponseOrigin`, the same canonical provenance types the transport/cache execution seams already stamp) exactly the way `transport`/`dns` were already read from `Page::transport()`; no new provenance source, nothing fabricated. `record_evidence()` mints (or reuses a caller-supplied) `EvidenceId`, then persists the bundle through `DomainPersistence::append_history` — Track 3's append-only historical semantics, never `write_current` — because evidence is immutable and historical from the moment it is captured, not a value that later gets replaced. Every write uses the fixed revision `1`; Track 3's own `(identity, revision)` uniqueness constraint is what makes a duplicate `EvidenceId` write fail closed, unmodified by this frontier. `EvidenceRef` is a plain `EvidenceId` wrapper (`Copy`, no payload) for later Watch/Change/Lineage frontiers to hold cheaply and resolve via `read_evidence()` when they need the content — it is not a second evidence model.

### 3.7 Interfaces

| Area | Canonical Owner | Allowed Dependencies | Forbidden Dependencies | Public Execution Seam | Upstream Compatibility Paths |
|---|---|---|---|---|---|
| CLI | `spider_cli` | `spider::utils::evidence`, `spider::features::transport`, `spider::features::search_providers` | `reqwest::Client` construction, independent domain logic | `scorpion` binary | None |
| MCP | `spider_mcp` | `spider::utils::evidence`, `spider::features::transport`, `spider::features::search_providers` | `reqwest::Client` construction, independent domain logic | `serve_stdio()` | `spider_mcp::evidence` shim |
| Agent types | `spider_agent_types` | Pure data | `spider`, `spider_agent` | Data types | None |
| Agent HTML | `spider_agent_html` | `lol_html` | `spider`, `spider_agent` | `clean_html*()` | None |

### 3.8 Future Areas

| Area | Status | Rule |
|---|---|---|
| WATCH/MONITOR | **PARTIALLY BLOCKED** | `WatchDefinition`/`WatchState` (§3.14), cadence/execution (`WatchSchedule`, `execute_scheduled_watch_run`, §3.15), and change detection (`ChangeResult`/`ChangeEvent`, §3.16) now exist and are canonically owned — `WatchId → WatchDefinition → WatchState → Snapshot(ObserveEvidence) → Transition` is realized end-to-end and persisted, a scheduled run executes through the canonical acquisition/evidence/transition path idempotently, and two of a watch's own durable evidence records can be truthfully compared and durably recorded. Still blocked, pending their own future frontiers: a background scheduler daemon deciding *when* a trigger fires, health, and a notification system. |

### 3.9 Identity

| Area | Canonical Owner | Allowed Dependencies | Forbidden Dependencies | Public Execution Seam | Upstream Compatibility Paths |
|---|---|---|---|---|---|
| Persisted-domain identity | `spider/src/features/identity.rs` | `std`, `ahash` (entropy mixing only) | Network, transport, persistence, state/lifecycle, the domain object types themselves | `EvidenceId`, `WatchId` | None |

**Clarification on `identity.rs`:** This module owns identity only —
explicit type, deterministic serialization, validating parse, and value
equality/hash/ordering per identity kind. It owns no persistence, no
state/lifecycle, and no domain object. `EvidenceId` realizes the concept
locked in `SCORPION.md` §3; `WatchId` realizes the first link of the
state-driven capability chain locked in `SCORPION_SDD.md` §5.2. Only these
two identity types exist — `ResearchId`, `CrawlId`, `FetchId`, `SessionId`,
`AuthSessionId`, `JobId`, `OperationId`, and any other identity type each
require their own frontier scoped to an actually-locked, actually-needed
concept; none may be added "for symmetry" with these two.

### 3.10 State/Transition Semantics

| Area | Canonical Owner | Allowed Dependencies | Forbidden Dependencies | Public Execution Seam | Upstream Compatibility Paths |
|---|---|---|---|---|---|
| Persisted-domain state/transition semantics | `spider/src/features/domain_state.rs` | `std`, `features/identity.rs` (parameterization only) | Network, transport, persistence, any concrete domain state/product model | `CurrentState`, `HistoryEntry`, `HistoryLog`, `Transition` | None |

**Clarification on `domain_state.rs`:** This module owns semantics
only — the canonical distinction between identity, current state,
historical record, and transition; the transition contract (`current
state + explicit transition → new current state`, realized as
`CurrentState::apply`); the "one current state per identity" invariant
(enforced by `CurrentState` holding exactly one state value, never a
collection); the "historical records are immutable/append-only"
invariant (enforced structurally — `HistoryLog`'s only mutating method is
`append`; `HistoryEntry` has no field-mutating method at all); and the
ownership boundary between domain code (decides transition validity,
`Transition::apply` receives no storage handle) and persistence (stores
state, never re-decides a transition — SCORPION_SDD.md §5.2). It defines
no database/cache/file persistence, no `WatchDefinition`/`WatchState`
product model, no `AuthSessionId`, no scheduling, no
`ChangeResult`/`ChangeEvent`, no health semantics. This frontier also
reconciled two bare-name collisions before adding new canonical
vocabulary: `Observation` remains owned by
`spider_agent_types::PageObservation` (a different, agent-automation
domain) — the closest concept this module needs is named `HistoryEntry`
instead; `Snapshot` remains split between `VitalsSnapshot`,
`BrowserChallengeSnapshot`, and `SCORPION_SDD.md` §5.2's informal locked
use naming a future watch-specific transition *input* — this module
never defines a bare `Snapshot` type. `Fingerprint` is a known, separate
naming collision deliberately left unreconciled; it belongs to Track 6.

### 3.11 Persistence Mechanism

| Area | Canonical Owner | Allowed Dependencies | Forbidden Dependencies | Public Execution Seam | Upstream Compatibility Paths |
|---|---|---|---|---|---|
| Persisted-domain persistence mechanism | `spider/src/features/domain_persistence.rs` | `std`, `sqlx`/SQLite (reused, `disk` feature) | Network, transport, `features/identity.rs`, `features/domain_state.rs`, any concrete domain/product-model type | `DomainPersistence` | None |

**Clarification on `domain_persistence.rs`:** This module owns the
mechanism only — one SQLite-backed store, gated behind the existing
`disk` feature so no second, always-on persistence stack is introduced.
It never imports `features/identity.rs` or `features/domain_state.rs`;
every identity is an opaque `&str` (any identity's `Display` form), every
state is opaque `&[u8]`. Current-state writes
(`DomainPersistence::write_current`) are compare-and-swap only — the
caller supplies the revision it expects is currently stored, and the
write is rejected with nothing changed if that does not match; there is
no unconditional-overwrite method. Historical-record appends
(`DomainPersistence::append_history`) fail closed on a duplicate
`(identity, revision)` key via the table's own `PRIMARY KEY` constraint,
not application-level convention — an existing record can never be
silently replaced. It owns its own tables and connection pool, separate
from `features/disk.rs`'s `DatabaseHandler` (Spider's upstream
crawl-resume/dedup mechanism — a different, non-transition-aware,
freely-overwritable schema serving a different purpose). Per this
frontier's explicit scope, it implements no Evidence Ledger product
semantics, no authenticated-session lifecycle, no `WatchDefinition`/
`WatchState`, no Fingerprint/Lineage, no scheduling, no change detection,
no health, no event sourcing, and no generic Job/Operation persistence —
it is a mechanism two future capabilities will call into, not a
capability itself.

### 3.12 Authenticated Session Lifecycle

| Area | Canonical Owner | Allowed Dependencies | Forbidden Dependencies | Public Execution Seam | Upstream Compatibility Paths |
|---|---|---|---|---|---|
| Authenticated-session identity | `spider/src/features/identity.rs` | `std`, `ahash` (entropy mixing only) | Network, transport, persistence, credential/cookie types | `AuthSessionId` | None |
| Authenticated-session lifecycle | `spider/src/features/auth_session.rs` | `features/identity.rs` (`AuthSessionId`), `features/domain_state.rs` (`Transition`/`CurrentState`), `features/domain_persistence.rs` (`disk`+`serde`) | A second cookie/session subsystem, a new browser architecture, CAPTCHA/transport internals, credential/cookie types | `AuthSessionState`, `PauseSession`/`ResumeSession`/`InvalidateSession`, `create_session()`, `apply_session_transition()` | None |

**Clarification on `auth_session.rs`:** Realizes `SCORPION.md` §5's
lifecycle rule ("pausing and resuming the *same* authenticated browser
session — not re-authenticating from scratch, and not silently
continuing unauthenticated") on top of Track 2/3 unmodified. Three
states — `Active`, `Paused`, `Invalidated` (terminal) — source-justified
directly from §5's own words; "resumed" is the transition
(`ResumeSession`) that produces `Active` again, not a fourth state, since
inventing one would be symmetry, not domain justification. `ResumeSession`
only succeeds when its `BrowserContinuityToken` matches exactly the one
`PauseSession` recorded — a mismatch (including "no token", i.e. a fresh
context) is rejected as `ContinuityMismatch`, which is what makes "same
browser session, not silent re-authentication" a checked property rather
than a comment. This is the first capability to use Track 2's full
contract — current state via `DomainPersistence::write_current`
(compare-and-swap) *and* each superseded state via
`DomainPersistence::append_history` (immutable) — where Track 4's
evidence ledger only ever needed the append-only half. `AuthSessionId`
shares `EvidenceId`/`WatchId`'s exact 16-opaque-byte shape, so it is
structurally incapable of holding a cookie, `Authorization` value, token,
or credential; `AuthSessionState` carries only `origin`,
`AuthenticationProfile` (§5's locked method vocabulary — classification,
not a secret), and, while paused, `BrowserContinuityToken` (a
caller-derived opaque reference, never the underlying cookie jar's
contents). `SCORPION.md` §5's `CredentialRef` remains locked/undefined,
exactly as before this frontier — not implemented here. No new browser
suspension/resumption machinery is built; a real
`BrowserContinuityToken`'s derivation from a live browser/cookie-jar
primitive is left to the caller (a future frontier), consistent with "do
not create a new browser architecture."

**Collision audit:** "session" already names three unrelated things —
`chromiumoxide::cdp::browser_protocol::target::SessionId` (CDP
browser-automation transport identity, also used unchanged in
`features/frame_context.rs`'s frame-identity chain) and `spider_mcp`'s
`CrawlSession`/`CrawlSessionStatus` (in-memory async MCP tool-call
progress tracking, `DashMap<String, CrawlSession>`-keyed). None of the
three represent "this identity is authenticated"; none are redefined,
renamed, or touched by this frontier — guardrailed directly (no
`AuthSessionId`/`AuthSessionState` reference appears in either file).

### 3.13 Content/Transform Lineage

| Area | Canonical Owner | Allowed Dependencies | Forbidden Dependencies | Public Execution Seam | Upstream Compatibility Paths |
|---|---|---|---|---|---|
| Content/transform lineage | `spider/src/features/transform_lineage.rs` | `features/domain_persistence.rs` (`DomainPersistence`), `utils/evidence.rs` (`sha256_hex`, `EvidenceRef`) | `configuration::Fingerprint`/`spider_fingerprint`, `spider_transformations` (interface-layer only), any concrete transformation-library type | `TransformLineageId`, `TransformationIdentity`, `record_lineage()`, `read_lineage()` | None |

**Clarification on `transform_lineage.rs`:** Required Fingerprint
reconciliation first: `spider::configuration::Fingerprint`
(re-exported from the `spider_fingerprint` crate) is an unrelated
browser anti-detection stealth-spoofing profile (`Basic`/`NativeGPU`/
`None`) with no notion of content, transformation, or provenance — it is
not redefined, shadowed, or imported here, and the new type introduced
is named `TransformLineageId`, never bare `Fingerprint`. Unlike
`features/identity.rs`'s three randomly-minted identity types,
`TransformLineageId` is **content-addressed** — a deterministic SHA-256
(reusing `sha256_hex`, never reimplemented) of `(input hash,
transformation identity, output hash)` — which is why it lives in its
own module rather than blending a second minting strategy into
`identity.rs`'s tightly-scoped contract. The same triple always produces
the same id (`record_lineage` treats a resulting
`PersistenceError::HistoryAlreadyExists` as success, not a conflict,
since a content-addressed collision can only mean the identical fact was
already recorded); a different input, transformation, or output always
hashes to a different id. `spider` has no dependency on
`spider_transformations` (an interface-layer crate used only by
`spider_cli`/`spider_mcp`), so the transformation link is named by a
caller-supplied deterministic description string, not a concrete
`TransformConfig` type — the same neutral-seam pattern
`domain_persistence.rs` already established. Persisted only through
`DomainPersistence::append_history` (never `write_current` — a lineage
fact has no current state), exactly like Track 4's evidence ledger.
`EvidenceRef` is stored by reference when the input is already durable
evidence — never a duplicate of the evidence payload. Implements no
`WatchDefinition`/`WatchState`, no scheduling, no
`ChangeResult`/`ChangeEvent`, no health, and does not redesign
`EvidenceBundle` or transport.

### 3.14 Watch Model

| Area | Canonical Owner | Allowed Dependencies | Forbidden Dependencies | Public Execution Seam | Upstream Compatibility Paths |
|---|---|---|---|---|---|
| Watch identity | `spider/src/features/identity.rs` | `std`, `ahash` (entropy mixing only) | Network, transport, persistence, target/lifecycle types | `WatchId` | None |
| Watch model | `spider/src/features/watch.rs` | `features/identity.rs` (`WatchId`), `features/discovery_target.rs` (`DiscoveryTarget`), `features/domain_state.rs` (`Transition`/`CurrentState`), `features/domain_persistence.rs` (`disk`+`serde`), `utils/evidence.rs` (`EvidenceRef`) | A scheduler, change detection (`ChangeResult`/`ChangeEvent`), health, notifications, a generic `Job` model, a new `WatchTarget`/`WatchSpec`, a second persistence or evidence mechanism | `WatchDefinition`, `WatchState`, `ObserveEvidence`/`StopWatch`, `define_watch()`, `apply_watch_transition()` | None |

**Clarification on `watch.rs`:** Realizes exactly the locked chain
`SCORPION_SDD.md` §5.2 named — `WatchId → WatchDefinition → WatchState →
Snapshot → Transition → ...` — and no further. `WatchDefinition` wraps
the existing `DiscoveryTarget` (`{ url, kind, discovered_via }`) rather
than inventing `WatchTarget`/`WatchSpec`: `DiscoveryTarget`'s own module
doc already distinguishes it from `SourceItem` ("a pointer... never a
content candidate"), which is exactly what a watch target is, and
`DiscoveryTargetKind::Requested` already names what a caller defining a
new watch supplies. `WatchDefinition` owns no execution history and no
mutable lifecycle state — `define_watch()` is the only way to create one,
persisted once through `DomainPersistence::append_history` at a
namespaced key (`"<id>#definition"`), immutable thereafter.
`WatchState` is the canonical current lifecycle state, built entirely on
Track 2's unmodified `CurrentState`/`Transition` contract: exactly two
states, `Active`/`Stopped` (terminal) — the minimum any watch can have at
all once scheduling, change detection, health, and notifications are
correctly out of scope, source-justified rather than invented for
symmetry with `AuthSessionState`'s three. `ObserveEvidence` realizes the
locked chain's "Snapshot" step exactly as `domain_state.rs`'s own doc
comment prescribed: a watch-specific input type (an `EvidenceRef`) to its
own `Transition<WatchState>` impl, updating the single current-evidence
pointer (not execution history — every superseded `WatchState` is
preserved separately via `HistoryLog`/`DomainPersistence`'s append-only
records). `StopWatch` is terminal, mirroring the precedent
`AuthSessionState::Invalidated` already set: a caller who wants to watch
again defines a new `WatchId`. `apply_watch_transition()` mirrors
`apply_session_transition()` exactly — compare-and-swap
(`DomainPersistence::write_current`, rejecting a concurrent writer rather
than silently losing it) for the new current state, plus an immutable
append (`DomainPersistence::append_history`) of the just-superseded
state; no blind overwrite exists anywhere in this path. `EvidenceRef` is
held by reference only (`Option<EvidenceRef>`, `Copy`, 16 bytes) — never
duplicating `EvidenceBundle`'s payload, realizing the "later Watch...
frontiers" use `EvidenceRef`'s own module doc anticipated. No scheduler,
no `ChangeResult`/`ChangeEvent`, no health, no notification system, and
no generic `Job` model are implemented here — those remain later,
separate frontiers.

### 3.15 Watch Scheduling / Execution

| Area | Canonical Owner | Allowed Dependencies | Forbidden Dependencies | Public Execution Seam | Upstream Compatibility Paths |
|---|---|---|---|---|---|
| Watch scheduling / execution | `spider/src/features/watch_schedule.rs` | `features/watch.rs` (`WatchDefinition` read-only, `apply_watch_transition`), `features/acquisition_binding.rs`, `features/domain_persistence.rs`, `features/transport.rs` (`TransportRequest`), `utils/evidence.rs` (`build_evidence`/`record_evidence`/`EvidenceRef`), `async_job::Schedule` (`cron` feature, cadence parsing only) | `website::CronType`, `async_job::Job`/`async_job::Runner`, a new fetch/crawl/transport path, `ChangeResult`/`ChangeEvent`, health, notifications, a generic `Job`/`Operation` model, redefining `WatchState` | `WatchSchedule`, `define_watch_schedule()`, `read_watch_schedule()`, `execute_scheduled_watch_run()` | None |

**Clarification on `watch_schedule.rs`:** Track 8 — the successor
boundary Track 7's own closure explicitly deferred ("a scheduler
deciding *when* a watch is checked... remain later, separate
frontiers"). Realizes cadence and one scheduled run's execution only, and
no scheduling daemon: `WatchSchedule { cron_str }` validates cadence
syntax via the exact primitive `Website`'s own cron feature already
depends on (`cron_str.parse::<async_job::Schedule>()`), never
`website::CronType` (a *what-to-run* selector, `Crawl`/`Scrape` — no
cadence syntax at all) and never `async_job::Job`/`async_job::Runner`
(the `Website`-owned always-running scheduler daemon abstraction) —
"adapt an existing primitive cleanly" means adopting the parser, not the
daemon. `WatchSchedule` is persisted immutably via `append_history` at a
namespaced key (`"<id>#schedule"`), exactly like `WatchDefinition`'s own
pattern. `execute_scheduled_watch_run(store, id, scheduled_at,
transport_request)` realizes `WatchDefinition → scheduled trigger →
canonical acquisition → durable EvidenceRef → WatchState transition`:
reads (never redefines) the watch's `WatchDefinition` via
`watch::read_watch_definition`; acquires through
`acquisition_binding::bind`/`::execute` — the same seam CLI/MCP fetch
already uses, no second fetch/crawl/transport architecture; builds and
durably records evidence through the unmodified `utils::evidence`
ledger; and applies the resulting transition through
`watch::apply_watch_transition` with Track 7's own `ObserveEvidence` —
`WatchState`'s variants, transitions, and persistence rules remain owned
exclusively by Track 7, never touched here. Idempotency: "the same
scheduled run" is identified by `(WatchId, scheduled_at)` and claimed
*before* any side effect via `DomainPersistence::write_current`'s
compare-and-swap (`expected_revision: None`) at a namespaced run key
(`"<id>#run#<unix_seconds>"`) — a caller that loses the claim never
touches acquisition/evidence/`WatchState` at all: a `Completed` record is
replayed (the already-produced `EvidenceRef` returned, no new work), a
`Claimed`-but-incomplete record (concurrent or crashed) is rejected fail
closed as `WatchExecutionError::RunAlreadyInProgress` rather than risking
duplicate work. Durable scheduler-owned state is kept to exactly what
this requires — the cadence record plus one claim/completion record per
run identity — no generic `Job`/`Task`/`Operation` table. No
`ChangeResult`/`ChangeEvent`, no health, no notification system, and no
background scheduling daemon are implemented here — those remain later,
separate frontiers.

### 3.16 Change Detection

| Area | Canonical Owner | Allowed Dependencies | Forbidden Dependencies | Public Execution Seam | Upstream Compatibility Paths |
|---|---|---|---|---|---|
| Change detection | `spider/src/features/change_detection.rs` | `features/watch.rs` (`WatchState`/`read_current_watch_state` read-only), `features/domain_persistence.rs`, `utils/evidence.rs` (`sha256_hex`, `EvidenceBundle`, `EvidenceRef`) | A scheduler of its own, health, notifications, a generic event framework, `Job`/`Operation`, a second `Evidence`/`Watch` model, a new acquisition path, a new hashing/fingerprint architecture | `ChangeResult`, `ChangeEvent`, `compute_change_result()`, `detect_and_record_change()`, `read_change_event()` | None |

**Clarification on `change_detection.rs`:** Track 9 — the successor
boundary Track 7's own closure deferred alongside scheduling
(`ChangeResult`/`ChangeEvent`, health, notifications). Realizes exactly
the first two, and no further. Same-watch-only: `detect_and_record_change`
never trusts a caller-supplied `EvidenceRef` pairing blindly — it reads
`watch`'s own current `WatchState` plus every superseded historical value
(Track 7's `HistoryLog`/`DomainPersistence::read_history`, reused
unmodified, at the exact plain key `apply_watch_transition` already
writes to) and confirms both the previous and current evidence actually
appear as a `last_evidence` value in that specific watch's own history —
an unrelated watch's evidence is rejected
(`ChangeDetectionError::EvidenceNotAssociatedWithWatch`) before any
comparison is attempted. Truthful comparison: `compute_change_result` is
a pure function of two already-resolved `EvidenceBundle`s that reuses
exactly the SHA-256 hash fields `build_evidence` already computed
(`response_body_hash` preferred, `transformed_content_hash` only as a
fallback) — two hashes are only ever compared when both bundles produced
the *same* basis; a mismatched or entirely absent basis produces
`ChangeResult::Uncomparable`, never a silently defaulted `Unchanged`.
Evidence that cannot even be resolved is rejected before comparison is
attempted at all (`PreviousEvidenceUnresolvable`/
`CurrentEvidenceUnresolvable`), never treated as "no change." Lineage/
fingerprint reuse: `TransformLineageId`/`TransformLineageRecord` (Track
6) model a different fact (input → transformation → output) and are not
reused as a type here; what is reused is `sha256_hex` itself (no new
hashing logic anywhere) and Track 6's content-addressed,
idempotent-duplicate-append persistence pattern — `ChangeEventId` is a
deterministic SHA-256 of `(watch, previous_evidence, current_evidence)`,
so recording an identical fact twice collapses to the same id
(`Err(PersistenceError::HistoryAlreadyExists) => Ok(event)`, Track 6's
own precedent reused verbatim) rather than conflicting or duplicating.
Persisted through `DomainPersistence::append_history` only (never
`write_current` — a change-detection fact has no current state to
replace), exactly like Track 6's lineage ledger. `ChangeResult`
computation is kept separate from `ChangeEvent` persistence:
`compute_change_result` is a plain, synchronous, non-`DomainPersistence`-
touching function; `detect_and_record_change` is the only function that
performs I/O. Track 8 (`features/watch_schedule.rs`) remains the sole
scheduler/execution owner — this module never defines scheduling of its
own and only ever *reads* the evidence Track 8's execution path already
produced, proven directly by a dedicated test suite that drives change
detection over two real `execute_scheduled_watch_run` calls against a
local HTTP fixture (not hand-built `EvidenceBundle` values). No health,
no notification system, no generic event framework, and no `Job`/
`Operation` model are implemented here — those remain later, separate
frontiers.

---

## 4. Canonical Path Map

### 4.1 One-Shot Fetch / Evidence

```
CLI/MCP
  └─> fetch_single_page_with_options (evidence.rs)
        └─> Website::with_transport + crawl_raw (Default)
              └─> configure_base_client (UPSTREAM_COMPAT)
        └─> fetch_via_tor (Tor)
              └─> build_tor_client (transport.rs)
```

### 4.2 Streaming Artifact Download

```
ArtifactDownloadBinding
  └─> artifact_download_execution::execute
        └─> transport::execute_streaming_request
              └─> build_streaming_client (Default or Tor)
                    └─> build_default_streaming_client / build_tor_client
```

### 4.3 Discovery

```
ResearchScope
  └─> research_scope::discover
        └─> discovery_target::plan
              └─> acquisition_binding::bind/execute
                    └─> fetch_single_page_with_options
```

### 4.4 Source Provider Network Execution

```
GitHubRepositoryProvider / HuggingFaceModelProvider
  └─> transport::execute_streaming_request
        └─> build_streaming_client (Default or Tor)
              └─> build_default_streaming_client / build_tor_client
```

Provider-specific request construction and response parsing remain in the
provider adapter; network execution, transport pinning, and target validation
are owned by canonical transport.

---

## 5. Upstream Compatibility Map

These paths are retained because upstream Spider functionality requires them.
New Scorpion capabilities must not depend on them for new behavior.

They may execute **transitively underneath an approved canonical boundary
primitive** (e.g. `crawl_raw()` internally uses `configure_base_client`) —
that is an implementation dependency of the boundary primitive, not an
alternate execution graph. Canonical code must not call them directly,
select them as alternates, or fall back to them. See `SCORPION_SDD.md` §3.

| Path | Why Retained | Constraint |
|---|---|---|
| `Website::configure_base_client` (reqwest/wreq variants) | Legacy crawl stack | Must not be extended for new transport work |
| `configuration.proxies` rotation list | Backward compatibility | Must not be extended; rejected under Tor |
| `socks://` → `http://` rewrite | Linux reqwest compatibility | Must not be extended; rejected under Tor |
| `setup_strict_policy` / `setup_redirect_policy` | Legacy redirect policies | Must not be extended; canonical transport uses `pin_redirect_policy` |
| `fetch_page_html*` variants | Legacy fetch matrix | Must not be extended |
| `spider_mcp::evidence` re-export shim | API stability | Must not be extended; use `spider::utils::evidence` directly |
| `spider_agent::automation` re-export layer | Agent compatibility | Compat only; new code should import canonical crates directly |
| `spider_worker` | `UPSTREAM_COMPATIBILITY_BOUNDARY`: terminal executable for Spider decentralized mode | Permitted graph is `spider_worker -> spider`; canonical Scorpion capabilities must never select its external protocol as alternate/fallback execution |

`spider_worker::target_host_blocked` is a private
`COMPATIBILITY_LOCAL_DEFENSE`, not canonical SSRF validation and not equivalent
to `spider_transport` enforcement. The worker may retain the exact upstream
compatibility primitives `Website::configure_http_client`,
`Page::new_page_streaming`, and `fetch_page_html_raw`. Tor continues to reject
decentralized execution.

---

## 6. Legacy / Rejected / Unknown Map

### 6.1 LEGACY

| Path | Status | Rule |
|---|---|---|
| `spider_agent/src/search/*` | Duplicate of `spider::features::search_providers` | Do not extend; converge or freeze |
| `RemoteFetcher` (`fetcher.rs`) | Coarser hook than `HttpFetchEngine` | Keep both; document `HttpFetchEngine` as preferred for transport swaps |
| `page::build` / `Page` `decentralized` variants | Weaker behavior | Flag as legacy |
| `Agent::new_page_with_url` | Deprecated | Schedule removal at next major |

### 6.2 REJECTED

| Path | Status | Rule |
|---|---|---|
| Tor + `wreq`/`cache_request`/`chrome`/`smart`/`decentralized`/proxy-rotation/Spider Cloud/parallel_backends/hedge/etag_cache | Rejected at `tor_crawl_preflight` | Do not "fix" by adding fallbacks |
| `.onion` under `Default` transport | Rejected by `validate_target` | Do not bypass |
| Cross-transport redirects | Rejected by `pin_redirect_policy` | Do not bypass |
| Credential-bearing endpoints/seeds/artifact URLs | Rejected by `TorTransportConfig::new` / `artifact_download_binding::bind` | Do not bypass |
| Non-`socks5h` schemes | Rejected by `TorTransportConfig::new` | Do not bypass |
| Artifact destination overwrite | Rejected by `artifact_download_execution` | No overwrite policy yet |
| `spider_mcp` `search_serper`/`search_brave` dead features | Rejected | Remove flags or implement tools |
| `build_evidence_with_transport` | REJECTED = REMOVED (architecture-convergence frontier) | Reintroduction detected by `rejected_build_evidence_with_transport_is_gone` |

### 6.3 UNKNOWN

| Path | Status | Rule |
|---|---|---|
| `spider/src/features/automation.rs:422` `LazyLock<reqwest::Client>` | Unclassified | Must not be promoted to canonical without audit |
| `spider_agent` LLM client proxy handling (silent skip) | Silent-fallback pattern | Should at least warn; not canonical |
| `spider_agent::automation` engine/browser vs spider core chrome | Ownership undecided | Must not be promoted to canonical without decision |
| Canonical streaming/artifact stack absent under `wreq`/`cache_request` | Coverage gap | Deliberate exclusion; must not be silently enabled |

---

## 7. Guardrails

Machine-enforceable guardrails are implemented in `spider/tests/architecture_guardrails.rs`.
The following invariants are enforced:

### 7.1 NO PARALLEL HTTP STACK

New Scorpion capabilities must not construct independent HTTP clients outside
canonical transport ownership. `reqwest::Client::new()` and
`reqwest::Client::builder()` are only permitted in:

- `spider/src/features/transport.rs` (canonical)
- `spider/src/website.rs` (upstream compat)
- `spider/src/utils/mod.rs` (upstream compat)
- `spider/src/features/search_providers/` (legacy search)
- `spider/src/features/automation.rs` (anti-bot solver)
- `spider/src/features/solvers.rs` (LLM solver)

**Grandfathered exception** — the following file contains a pre-existing raw
`reqwest::Client` construction that is **not** canonical approval. It is
classified as UNKNOWN (outside canonical seam, unaudited for SSRF/redirect/
provenance) and is frozen: new Scorpion capabilities must not extend it.
Its presence in the allowlist is a mechanical exception, not architectural
approval.

- `spider/src/page.rs` (test-only usages inside `#[cfg(test)]` blocks)

### 7.2 NO PARALLEL TOR STACK

Tor configuration/client/proxy behavior must remain transport-owned.
`build_tor_client`, `TorTransportConfig::new`, and `apply_transport_policy` are
only permitted in `spider/src/features/transport.rs`.

### 7.3 NO DUPLICATE SECURITY PRIMITIVES

Onion classification, target validation, redirect/SSRF checks, and secret-header
semantics must not be duplicated.

- `ends_with(".onion")` only in `spider/src/features/transport.rs`
- `is_onion_url` only in `spider/src/features/transport.rs`
- `validate_target` only in `spider/src/features/transport.rs`
- `pin_redirect_policy` only in `spider/src/features/transport.rs`
- `ssrf_screened_base_policy` only in `spider/src/features/transport.rs`

### 7.4 NO SILENT FALLBACKS

Do not introduce patterns equivalent to `canonical path fails → silently run
old/alternate implementation`. Fallback behavior is allowed only when explicitly
represented by canonical policy and tested.

### 7.5 REJECTED MEANS REMOVED

Rejected implementations must not remain callable as fallback, compatibility
shim, or hidden alternate path.

### 7.6 NO SHADOW MODELS

Do not create alternative versions of canonical domain models.
`ArtifactReference`, `ArtifactDownloadBinding`, `ArtifactDownloadExecutionError`,
`AcquiredArtifact` are only defined in their canonical modules. `EvidenceId`
and `WatchId` are only defined in `spider/src/features/identity.rs` — no
interface (`spider_cli`, `spider_mcp`, or otherwise) may define its own
identity type for either concept. `CurrentState`, `HistoryEntry`,
`HistoryLog`, and `Transition` are only defined in
`spider/src/features/domain_state.rs`; no bare `Observation` or `Snapshot`
type may be introduced anywhere (see §3.10). `DomainPersistence` is only
defined in `spider/src/features/domain_persistence.rs` (see §3.11 and
§7.11) — no interface, and no other module, may define a second
persistence seam for canonical domain state. `EvidenceBundle` (extended,
never replaced) remains the one canonical evidence model; `EvidenceRef`
and `EvidenceLedgerError` are only defined in
`spider/src/utils/evidence.rs` (see §3.6) — no interface may define its
own evidence-reference or evidence-ledger-error type. `AuthSessionId` is
only defined in `spider/src/features/identity.rs`; `AuthSessionState`,
`AuthenticationProfile`, `BrowserContinuityToken`,
`AuthSessionTransitionRejected`, `PauseSession`/`ResumeSession`/
`InvalidateSession`, and `AuthSessionError` are only defined in
`spider/src/features/auth_session.rs` (see §3.12) — no interface may
define its own session-lifecycle model, and no bare `SessionId` may be
introduced anywhere. `Fingerprint` is never defined anywhere in
`spider/src` — it is owned entirely by the `spider_fingerprint` crate and
only re-exported from `configuration.rs`; `TransformLineageId`,
`TransformationIdentity`, `TransformLineageRecord`, and
`TransformLineageError` are only defined in
`spider/src/features/transform_lineage.rs` (see §3.13) — no interface may
define its own content/transform lineage model. `WatchDefinition`,
`WatchState`, `WatchTransitionRejected`, `WatchError`, `ObserveEvidence`,
and `StopWatch` are only defined in `spider/src/features/watch.rs` (see
§3.14) — no interface may define its own Watch model, and no
`WatchTarget`/`WatchSpec` may be introduced anywhere. `WatchSchedule`,
`WatchScheduleError`, `WatchExecutionError`, and `ScheduledRunRecord` are
only defined in `spider/src/features/watch_schedule.rs` (see §3.15) — no
interface may define its own scheduling/execution model, and no
`Job`/`Operation`/`Scheduler` daemon model may be introduced anywhere.
`ChangeResult`, `ChangeEvent`, `ChangeEventId`, `ChangeDetectionError`,
`ComparisonBasis`, and `UncomparableReason` are only defined in
`spider/src/features/change_detection.rs` (see §3.16) — no interface may
define its own change detection model.

### 7.7 THIN INTERFACES

CLI/MCP/API/TUI/library surfaces may:
- parse input
- resolve presentation concerns
- call canonical core
- serialize output

They must not own canonical domain execution.

### 7.8 PROVIDER ISOLATION

Provider-specific behavior belongs in provider/source adapters. Neutral
transport/acquisition/artifact primitives must remain provider-independent.

### 7.9 FAIL CLOSED

Unavailable security-sensitive capabilities must fail explicitly. Do not
silently weaken Tor/security/identity semantics based on feature availability.

### 7.10 NO MVP/TEMPORARY ARCHITECTURE

Temporary implementations must not become alternate canonical paths. If
temporary code is unavoidable, it must have:
- explicit classification
- bounded scope
- removal condition
- no automatic fallback role

### 7.11 NO BLIND PERSISTENCE WRITES

Canonical domain current-state persistence must be compare-and-swap
(caller states the expected prior revision; a mismatch fails closed with
nothing written) — never blind/unconditional overwrite. Canonical
historical-record persistence must be append-only; writing an
already-recorded historical key must fail closed, enforced at the storage
layer (a database constraint), not merely by application-level
convention. Persistence stores canonical domain state; it must never
decide whether a transition is valid, invent lifecycle state, or import a
concrete domain/product-model type.

---

## 8. Frontier Rules

1. **Audit first**: Every frontier begins with a repository-wide audit.
2. **One frontier at a time**: Do not begin another capability frontier until
   the current one is closed.
3. **Smallest possible change**: The goal is to control what new Scorpion
   development may depend upon, not to broadly rewrite upstream.
4. **Block on missing canonical seams**: If a capability lacks the canonical
   model/seam required for truthful implementation, **BLOCK it**. Do not
   implement a local workaround.
5. **Closure rules**: A frontier is closed only when:
   - Implementation is complete
   - Tests pass
   - Regression checks pass
   - Operator review approves
   - Commit and push succeed
   - Worktree and index are clean

---

## 9. Architecture Debt

Known architecture debt that cannot safely be corrected inside this bounded
frontier:

| Debt | Classification | Smallest Prerequisite Frontier |
|---|---|---|
| `cache_request` unit-test build broken | REJECTED (actionable) | Fix or gate the 4 `Client::new()` call sites in `utils/mod.rs` test module |
| `docs.rs` feature list omits `evidence` and `transport_tor` | UNKNOWN | Add features to `package.metadata.docs.rs` |
| `spider_agent` duplicate search stack | LEGACY | Converge `spider_agent` onto `spider::features::search_providers` or vice versa |
| `spider_agent` LLM client silent proxy skip | UNKNOWN | Make proxy failure explicit |
| Legacy `socks://`→`http://` rewrite remains live in default builds | LEGACY | Harden or quarantine legacy proxy paths |
| SCORPION.md `ArtifactId` vs implemented `ArtifactReference` naming collision | UNKNOWN | Rename one when `ArtifactId` is realized |

---

## 10. Explicitly Not Refactored

The following are intentionally not refactored in this frontier:

- `Website` crawl/scrape family and client construction
- `fetch_page_html*` variants
- TLS scheme-flip / strip-www retry ladder
- Smart-mode Chrome fallback
- spider.cloud fallback modes
- Legacy proxy rotation
- `spider_agent` search/LLM stacks
- `spider_worker`
- io_uring ↔ tokio::fs fallback mirror
- Streaming vs legacy link extraction parity

---

## 11. Architecture Test Coverage

`spider/tests/architecture_guardrails.rs` enforces:

- Canonical module declarations exist
- Canonical seams exist with expected signatures
- No new `reqwest::Client` construction outside allowed paths
- No new `TorTransportConfig` construction outside `transport.rs`
- No new `build_tor_client` outside `transport.rs`
- No new `.onion` suffix matching outside `transport.rs`
- No new `ArtifactReference` / `ArtifactDownloadBinding` definitions outside canonical modules
- Correct feature gates on `artifact_download_execution`
- Correct feature gates on `fetch_via_tor`
- Correct feature gates on `build_streaming_client`
- REJECTED means removed: `build_evidence_with_transport` reintroduction is detected
- Canonical modules must not *directly* call legacy/upstream execution paths
  (`configure_base_client`, `fetch_page_html*`, legacy redirect policies,
  `socks://`→`http://` rewrite); transitive execution underneath an approved
  boundary primitive is permitted and is not what this guard forbids
- Providers must execute through `execute_streaming_request` and must not
  construct `Website`, `Page`, or `reqwest::Client`
- Single execution graph: `discover`, `plan`, `build_evidence`, and
  `EvidenceBundle` are each defined in exactly one canonical module
- Thin interfaces: `spider_cli`/`spider_mcp` define no shadow canonical models
- Negative scanner proofs: synthetic violations in every guarded class are
  proven detected (see `scanner_detects_every_violation_class`)
- `EvidenceId`/`WatchId` are each defined in exactly one canonical module
  (`features/identity.rs`), declared unconditionally (no feature gate),
  implement deterministic serialization/validation, and are not shadowed
  by `spider_cli`/`spider_mcp`; the identity module contains no
  persistence or state/lifecycle implementation
- `CurrentState`/`HistoryEntry`/`HistoryLog`/`Transition` are each defined
  in exactly one canonical module (`features/domain_state.rs`), declared
  unconditionally, contain no persistence or concrete product-model
  implementation, structurally enforce `HistoryLog` append-only semantics
  (no `remove`/`clear`/`get_mut`/`IndexMut`), match the canonical
  transition contract signature, are not shadowed by
  `spider_cli`/`spider_mcp`, and introduce no bare `Observation`/`Snapshot`
  type anywhere in `spider/src`
- `DomainPersistence` is defined in exactly one canonical module
  (`features/domain_persistence.rs`), gated behind the existing `disk`
  feature (no second storage stack), never imports
  `features/identity.rs`/`features/domain_state.rs`, decides no domain
  semantics (no `Transition`, no `WatchState`/`WatchDefinition`, no
  lifecycle status, no scheduling, no event sourcing), exposes
  current-state writes only as compare-and-swap (no unconditional
  overwrite method), enforces historical-append fail-closed-on-duplicate
  at the database constraint level (never an `UPDATE`/`DELETE`/`INSERT OR
  REPLACE`/`INSERT OR IGNORE` on the history table), and is not shadowed
  by `spider_cli`/`spider_mcp`
- `EvidenceRef`/`EvidenceLedgerError`/`record_evidence`/`read_evidence`
  are each defined in exactly one canonical module (`utils/evidence.rs`),
  persist through `DomainPersistence::append_history` only (never
  `write_current` — evidence has no current state), always use the fixed
  revision `1`, define no lifecycle/product-model/scheduling/event-
  sourcing logic of their own, read `backend_provenance`/`response_origin`
  from `Page`'s existing canonical accessors (never a fabricated value),
  and are not shadowed by `spider_cli`/`spider_mcp`
- No bare `SessionId` exists anywhere in `spider/src`; `AuthSessionId` is
  defined in exactly one module (`features/identity.rs`) sharing
  `EvidenceId`/`WatchId`'s exact opaque-byte shape; `AuthSessionState`/
  `AuthenticationProfile`/`BrowserContinuityToken`/
  `AuthSessionTransitionRejected`/the three transition types/
  `AuthSessionError` are defined in exactly one module
  (`features/auth_session.rs`); that module documents (and a guardrail
  proves) it never touches `frame_context.rs` or `spider_mcp`'s
  `CrawlSession`; it uses exactly 3 `Transition<AuthSessionState>` impls;
  its persistence uses both `DomainPersistence::write_current` and
  `::append_history` (never a second persistence mechanism); resume
  requires an exact `BrowserContinuityToken` match; no real
  credential-carrying type (`HeaderValue`, `HeaderMap`,
  `SecretRequestHeaders`, a cookie jar) is referenced by identity or
  lifecycle state; and none of it is shadowed by `spider_cli`/`spider_mcp`
- `configuration::Fingerprint`'s re-export is unchanged; no bare
  `struct`/`enum Fingerprint` is ever defined in `spider/src`;
  `transform_lineage.rs` never imports it or the `spider_fingerprint`
  crate; `TransformLineageId`'s identity is purely content-addressed
  (derived only from the input/transformation/output triple, never a
  random source or `recorded_at`); lineage persists through
  `DomainPersistence::append_history` only (never `write_current`, never
  raw SQL); `EvidenceRef` is stored by reference, never duplicated; and
  none of `transform_lineage.rs`'s types are shadowed by
  `spider_cli`/`spider_mcp`
- `WatchDefinition`/`WatchState`/`WatchTransitionRejected`/`WatchError`/
  `ObserveEvidence`/`StopWatch` are each defined in exactly one canonical
  module (`features/watch.rs`); `WatchId` is reused (imported), never
  redefined, and remains defined in exactly one module
  (`features/identity.rs`); `WatchState` carries no `DiscoveryTarget`/
  target field of its own (definition/state separation); exactly 2
  `Transition<WatchState>` impls exist (`ObserveEvidence`, `StopWatch`);
  persistence uses both `DomainPersistence::write_current`
  (compare-and-swap against the just-read revision, surfacing a conflict
  as `WatchError::ConcurrentModification` rather than silently dropping
  it) and `::append_history` (never a second persistence mechanism, never
  raw SQL); `EvidenceRef` is held by reference only
  (`Option<EvidenceRef>`), never duplicated; no `WatchTarget`/`WatchSpec`
  exists anywhere in `spider/src`; no scheduler/`ChangeResult`/
  `ChangeEvent`/health/notification/generic-`Job` capability is
  implemented; and none of it is shadowed by `spider_cli`/`spider_mcp`
- `WatchSchedule`/`WatchScheduleError`/`WatchExecutionError`/
  `ScheduledRunRecord` are each defined in exactly one canonical module
  (`features/watch_schedule.rs`), gated behind `evidence`+`disk`+`cron`;
  cadence is validated via `cron_str.parse::<async_job::Schedule>()` —
  never `website::CronType` (a what-to-run selector) and never
  `async_job::Job`/`async_job::Runner` (the `Website`-owned scheduler
  daemon); execution reuses `watch::read_watch_definition` and
  `watch::apply_watch_transition` without redefining `WatchDefinition`/
  `WatchState`; acquisition reuses `acquisition_binding::bind`/`::execute`
  (no new `reqwest::Client`, `Website::new`, `.crawl()`/`.scrape()`, or
  Tor client construction); evidence reuses `build_evidence`/
  `record_evidence` (no redefinition of `EvidenceBundle`/`EvidenceRef`);
  the run identity is claimed via `DomainPersistence::write_current`
  compare-and-swap strictly before acquisition begins and finalized
  strictly after the `WatchState` transition completes (proven by source
  position), so a losing concurrent/retried claim never duplicates a
  fetch, evidence record, or transition; no `ChangeResult`/`ChangeEvent`/
  health/notification/`Job`/`Operation`/`Scheduler` capability is
  implemented; and none of it is shadowed by `spider_cli`/`spider_mcp`
- `ChangeResult`/`ChangeEvent`/`ChangeEventId`/`ChangeDetectionError`/
  `ComparisonBasis`/`UncomparableReason` are each defined in exactly one
  canonical module (`features/change_detection.rs`), gated behind
  `evidence`+`disk`; both `previous_evidence` and `current_evidence` are
  validated against the watch's own current + historical `WatchState`
  evidence (via `watch_evidence_refs`/`ensure_evidence_belongs_to_watch`)
  before any comparison is attempted, rejecting an unrelated watch's
  evidence; a mismatched or absent hash basis produces
  `ChangeResult::Uncomparable`, never a silently defaulted `Unchanged`
  (proven by the `if previous_basis == current_basis` guard); hashing
  reuses `EvidenceBundle`'s existing `response_body_hash`/
  `transformed_content_hash` fields and `sha256_hex` — no redefinition of
  either and no new fingerprint architecture; persistence uses
  `DomainPersistence::append_history` only (never `write_current`), keyed
  by a `ChangeEventId` content-addressed from `(watch, previous_evidence,
  current_evidence)`, so a duplicate recording is idempotent
  (`Err(PersistenceError::HistoryAlreadyExists) => Ok(event)`) rather than
  a conflict; `compute_change_result` is a plain synchronous function kept
  separate from `detect_and_record_change`'s persistence I/O; Track 8
  (`features/watch_schedule.rs`) remains the sole scheduler/execution
  owner (no `WatchSchedule`/scheduling type or `async_job::Schedule`
  reference exists in `change_detection.rs`), verified against two real
  `execute_scheduled_watch_run` calls, not hand-built evidence; no
  health/notification/generic-event/`Job`/`Operation` capability is
  implemented; and none of it is shadowed by `spider_cli`/`spider_mcp`

New violations are caught by `cargo test -p spider --test architecture_guardrails`.
