# Scorpion — Software Design Specification

**Status:** ACTIVE — the design contract every Scorpion capability frontier
must satisfy. Enforced by `spider/tests/architecture_guardrails.rs` wherever
mechanical enforcement is practical; the rest is enforced by frontier review.

**Baseline:** `643c963fb498cb900ba4055395c450505dae7d89`

This document is the SDD for Scorpion's canonical architecture. It defines the
layer model, dependency direction, ownership, state model, security ownership,
and error ownership. `SCORPION_ARCHITECTURE.md` remains the classified
inventory of what currently exists; this document defines what the architecture
*is* — the target both documents and guardrails enforce. `SCORPION_PROCESS.md`
defines the frontier lifecycle that changes this architecture.

---

## 1. Hard Single-Flow Rule

For every Scorpion capability there is exactly:

```
ONE CAPABILITY
→ ONE CANONICAL MODEL
→ ONE CANONICAL SEAM
→ ONE ACTIVE EXECUTION GRAPH
→ ONE RESULT / STATE VOCABULARY
```

Forbidden:

```
CAPABILITY
├── canonical implementation
├── legacy implementation
└── fallback implementation
```

A superseded Scorpion implementation must become exactly one of:

- **REMOVED** — deleted from the tree, or
- **UNREACHABLE** — not callable from the canonical graph.

REJECTED means physically removed or structurally unreachable. Rejected code
must not remain as a fallback, compatibility shim, legacy callable path, old
implementation, v1/v2 alternative, temporary rescue path, hidden helper, or
feature-gated alternate execution path.

---

## 2. Canonical Layer Model

```
┌─────────────────────────────────────────────────────────┐
│ INTERFACES (thin)                                       │
│ spider_cli · spider_mcp · future API/Web · future TUI   │
├─────────────────────────────────────────────────────────┤
│ SCORPION CANONICAL GRAPH                                │
│ ┌───────────┐ ┌───────────┐ ┌────────────────────────┐  │
│ │ Research  │ │ Discovery │ │ Source/provider        │  │
│ │ (plan)    │→│ (targets) │→│ adapters               │  │
│ └───────────┘ └─────┬─────┘ └───────────┬────────────┘  │
│                     ↓                   ↓               │
│ ┌───────────┐ ┌───────────┐ ┌────────────────────────┐  │
│ │ Artifact  │ │ Evidence/ │ │ Acquisition            │  │
│ │ domain    │ │ provenance│ │ (one-shot + crawl)     │  │
│ └─────┬─────┘ └─────┬─────┘ └───────────┬────────────┘  │
│       └─────────────┴───────────────────┘               │
│                     ↓                                   │
│              ┌─────────────┐                            │
│              │ Transport   │  (security-critical core)  │
│              └─────────────┘                            │
├─────────────────────────────────────────────────────────┤
│ SPIDER UPSTREAM COMPATIBILITY GRAPH                     │
│ Website client stack · fetch_page_html* · proxy         │
│ rotation · legacy redirect policies · spider.cloud      │
│ fallback · decentralized fetch · spider_agent stacks    │
└─────────────────────────────────────────────────────────┘
```

### Layers

1. **Interfaces** — `spider_cli`, `spider_mcp`, future API/Web, future TUI.
   Thin: parse input, presentation-level validation, call the canonical core,
   serialize/present output. They own no canonical business, acquisition,
   transport, evidence, artifact, or provider logic.
2. **Research / Discovery** — pure planning. No network, no transport.
3. **Source/provider adapters** — provider-specific request construction and
   response parsing only. All network execution delegated to transport.
4. **Acquisition** — one-shot fetch (`utils/evidence.rs` seams) and multi-page
   crawl (`Website::with_transport()` + `crawl_raw()`/`scrape()` public seam).
5. **Evidence / provenance** — `EvidenceBundle` construction from `Page`;
   `Page::transport()` provenance stamp is the single source of truth for
   observed transport.
6. **Artifact domain** — reference → binding → execution; hashing and
   filesystem handling owned here, network delegated to transport.
7. **Transport** — the only layer that may construct HTTP/Tor clients for
   canonical Scorpion execution. Owns all security primitives (§6).
8. **Spider upstream compatibility graph** — retained upstream machinery.
   Canonical Scorpion must not select it as an alternate execution path.
   It may execute *transitively* underneath an explicitly approved canonical
   boundary primitive (see §3 and §9) — as an implementation dependency of
   that primitive, never as a graph canonical code addresses directly.

---

## 3. Dependency Direction

Three distinct dependency categories must never be conflated:

1. **CANONICAL DIRECT DEPENDENCY** — allowed only on canonical seams
   (the public execution seams listed in §4).
2. **TRANSITIVE UPSTREAM IMPLEMENTATION** — permitted only behind an
   explicitly approved canonical boundary primitive. The boundary primitive
   (e.g. `Website::with_transport()` + `crawl_raw()`) may currently be
   *implemented using* isolated Spider upstream-compatible machinery
   (`configure_base_client`, `fetch_page_html*`, legacy redirect/proxy
   behavior). That machinery is an implementation dependency *of the
   boundary primitive*, not a graph canonical code addresses directly.
3. **CANONICAL → LEGACY/UPSTREAM DIRECT ALTERNATE EXECUTION** — forbidden.
   Canonical code must not directly select upstream machinery, call it as an
   alternate path, depend on it as fallback, or extend it for new Scorpion
   capabilities.

"Not reachable from canonical seams" therefore means precisely: no canonical
module *directly calls or selects* upstream machinery. It does **not** mean
upstream machinery is never transitively executed underneath an approved
boundary primitive — today, Default one-shot acquisition legitimately runs
through `crawl_raw()`'s upstream implementation.

Allowed (downward only):

```
Interfaces          → Canonical graph (public seams only)
Research/Discovery  → models, vocabulary (no network, no transport)
Provider adapters   → transport::execute_streaming_request,
                      secret_request_headers, domain models
Acquisition         → transport, Page (upstream primitive)
Evidence            → Page, transport provenance
Artifact domain     → transport::execute_streaming_request, uring_fs
Transport           → reqwest, url (leaf; depends on nothing Scorpion)
Boundary primitives → their own upstream implementation machinery
                      (transitive; invisible to canonical callers)
```

Forbidden:

- Any canonical module → independent HTTP/Tor client construction.
- Any canonical module → *direct* call into the upstream compatibility
  graph, except through explicitly approved boundary primitives
  (`Website::with_transport`/`crawl_raw` seam, `Page`, `is_ssrf_redirect`).
  These are listed in `SCORPION_ARCHITECTURE.md` §3; the list is closed.
- Interfaces → canonical internals not exposed as public seams.
- Neutral layers (transport/acquisition/artifact) → provider-specific logic.
- A canonical path failing → silently executing a legacy/alternate path.

---

## 4. Canonical Capability Ownership

| Capability | Owner | Canonical seam | Model |
|---|---|---|---|
| Research | `features/research_scope.rs` | `discover()` | `ResearchScope` |
| Discovery | `features/discovery_target.rs` | `plan()` | `DiscoveryTarget` |
| Sources/adapters | `features/source_provider.rs` + one module per provider | provider methods | `SourceItem`, `ProviderDiscovery` |
| GitHub provider | `features/github_source_provider.rs` | `search_repositories()` | `GitHubRepositorySearchRequest` |
| Hugging Face provider | `features/hugging_face_source_provider.rs` | `search_models()` / `discover_artifacts()` | `HuggingFaceModelSearchRequest`, `HuggingFaceArtifactDiscoveryRequest` |
| Acquisition (one-shot) | `utils/evidence.rs` | `fetch_single_page_with_options()` | `AcquisitionOptions`, `TransportAcquisition` |
| Acquisition (crawl) | `website.rs` methods only | `Website::with_transport()` + `crawl_raw()`/`scrape()` | `Website`, `Page` |
| Acquisition binding | `features/acquisition_binding.rs` | `bind()` / `execute()` | `AcquisitionBinding` |
| Transport | `features/transport.rs` | `execute_streaming_request()` | `TransportPolicy`, `TransportRequest` |
| Evidence/provenance | `utils/evidence.rs` | `build_evidence()` | `EvidenceBundle` |
| Artifact reference | `features/artifact_reference.rs` | — | `ArtifactReference` |
| Artifact binding | `features/artifact_download_binding.rs` | `bind()` | `ArtifactDownloadBinding` |
| Artifact execution | `features/artifact_download_execution.rs` | `execute()` | `AcquiredArtifact`, `ArtifactDownloadExecutionError` |
| Secret headers | `features/secret_request_headers.rs` | `SecretRequestHeaders` | — |
| Durable research | `features/research_session.rs` | `run_durable_research()`, `read_research_session()` | `ResearchSession`, `DurableResearchResult` |
| CLI | `spider_cli` | `scorpion` binary | — |
| MCP | `spider_mcp` | `serve_stdio()` | — |
| future API/Web | not started | — | — |
| future TUI | not started | — | — |
| Watch identity/state | `features/{identity,watch}.rs` | `define_watch()`, watch transitions/read seams | `WatchId`, `WatchDefinition`, `WatchState` |
| Watch scheduling/execution | `features/watch_schedule.rs` | `define_watch_schedule()`, `execute_scheduled_watch_run()` | `WatchSchedule`, `ScheduledRunRecord` |
| Watch change/health | `features/{change_detection,watch_health}.rs` | change recording/read and health assessment seams | `ChangeResult`, `ChangeEvent`, `WatchHealthReport` |
| Deterministic page audit | `features/audit.rs` | `audit_page()` | `PageFacts`, `HtmlPageFacts`, `EvidencedPageFacts`, `Finding`, `ObservedTechnologyMarker`, `PageAuditResult` |
| Domain persistence runtime binding | `features/domain_runtime.rs` | `resolve_domain_database_path()`, `open_shared_domain_store()` | — (resolves a path/handle; owns no domain model of its own) |

---

## 5. State Model

Two classes of capability, distinguished explicitly:

### 5.1 Stateless canonical operations

Bounded operations with no persistent state:

```
binding → execute → result
```

Examples: `fetch_single_page_with_options`, `execute_streaming_request`,
`artifact_download_execution::execute`, provider searches,
`audit_page()`. They take an immutable input model, execute once, and
return a result vocabulary. No identity, no persistence, no resume.

`audit_page()` durably records the `Finding`s it produces (through
`DomainPersistence::append_history`, exactly like the evidence ledger,
change detection, and transform lineage already do) and does not persist
the `ObservedTechnologyMarker`s it produces at all — both facts are
consistent with "stateless," not contradictions of it. Content-addressed,
append-only derived-record persistence (evidence, change events, transform
lineage, audit findings) does not, by itself, make an operation
state-driven: the distinguishing test for §5.2 is identity + a mutable
current state + explicit transitions between named states. `audit_page()`
has none of these — no `AuditId`, no `AuditState`, no transition — it is a
bounded `binding → execute → result` operation over one already-acquired
page whose result happens to be durably, idempotently recorded underneath
it. `AuditId`, `AuditSession`, `AuditState`, and independent
technology-marker persistence remain unauthorized unless a future frontier
proves an actual ownership requirement neither of the three CLOSED audit
frontiers established.

### 5.2 State-driven capabilities

Durable state-driven capabilities include research sessions, authenticated
sessions, and watch state. Durable research follows:

```text
ResearchId
→ initial claimed ResearchSession persisted before search side effects
→ provider-neutral research execution with durable source acquisition
→ truthful terminal ResearchSession + compatible DurableResearchResult
```

`ResearchSession` owns invocation identity, lifecycle, evidence/source
accounting, Source-N bindings, and the nested durable result. It does not own
deterministic replay inputs or raw provider traffic.

The implemented watch state chain follows:

```
WatchId
→ WatchDefinition
→ WatchState
→ Snapshot
→ Transition
→ Event/Result
→ persisted updated state
```

Implemented watch ownership includes `WatchId`, `WatchDefinition`,
`WatchState`, canonical transitions/persistence, a scheduled execution seam,
change detection, and health/readiness. A continuously running background
scheduler daemon and notifications remain separate future product surfaces.

Rules for this class:

- One canonical state type per capability, owned by the canonical core.
- Interfaces always operate on the same canonical state — never on
  interface-local copies or re-derivations.
- Transitions are explicit, typed, and persisted; no implicit state drift.
- New state-driven capabilities must not be implemented before a frontier
  establishes their canonical model and ownership.

---

## 6. Security Ownership

Exactly one canonical owner per security primitive. Duplication is forbidden
and mechanically guarded.

| Primitive | Canonical owner | Seam |
|---|---|---|
| Target validation | `features/transport.rs` | `validate_target()` |
| Onion classification | `features/transport.rs` | `is_onion_url()` |
| SSRF screening | `website.rs::is_ssrf_redirect()` (shared primitive) | used by `pin_redirect_policy` |
| Redirect policy | `features/transport.rs` | `pin_redirect_policy()` |
| Transport policy | `features/transport.rs` | `TransportPolicy`, `TransportRequest::into_policy()` |
| Tor client construction | `features/transport.rs` | `build_tor_client()` (crate-private) |
| Secret request headers | `features/secret_request_headers.rs` | `SecretRequestHeaders` |
| Artifact integrity verification | `features/artifact_download_execution.rs` | `execute()` (hash/size verification) |
| Feature-based fail-closed | each feature-gated seam | explicit `Err`, never downgrade |

Fail-closed rule: when a security-sensitive capability is unavailable
(missing feature, missing Tor support, rejected combination), the result is an
explicit failure. Semantics must never be silently downgraded to a weaker
transport, weaker validation, or an alternate stack.

---

## 7. Error Ownership

- Each canonical layer owns a typed error vocabulary
  (e.g. `TransportError`, `ArtifactDownloadExecutionError`,
  `GitHubProviderError`).
- Lower-layer errors may be wrapped into the caller's vocabulary, but the
  failure semantics must survive: a transport failure must remain
  distinguishable from a parse failure, a validation failure from a network
  failure.
- Interfaces may map errors for presentation (exit codes, MCP error payloads)
  but must not flatten semantically distinct canonical failures into one
  opaque "error" merely because the interface is simpler that way.
- No layer may swallow an error and retry an alternate path unless that
  fallback is itself an explicitly modeled canonical policy.

---

## 8. Interface Boundary Model

Interfaces (CLI/MCP/future API/future TUI) may:

- parse and validate presentation-level input
- convert wire formats into canonical request types (e.g. `TransportRequest`)
- invoke canonical core seams
- serialize/present canonical results

Interfaces must not:

- construct HTTP/Tor clients
- implement crawl orchestration, evidence construction, artifact logic,
  provider logic, or transport policy beyond parsing
- define shadow domain models duplicating canonical types
- decide between canonical and legacy execution paths
- independently implement deterministic audit rule evaluation
  (SEO/security-header rules or any future rule), technology-marker
  extraction, applicability decisions, rule-version semantics, or
  `FindingId`/marker identity derivation — the `spider_audit_page` MCP
  tool (`spider_mcp/src/tools/audit.rs`, realized by
  `SCORPION_MCP_CANONICAL_PAGE_AUDIT_SHIPPING_001`) and, in future, a Web
  Console/API must call `audit_page()`/`PageAuditResult` (§4; see
  `SCORPION_ARCHITECTURE.md` §3.19) as their sole source of audit truth,
  never re-derive or approximate it, and must never reconstruct a
  parallel evidence truth where a shared `EvidenceRef` already resolves
  the same underlying evidence for both an AI-visible and a human-facing
  consumer — no other file, in any shipping crate, may reference the
  audit module's result vocabulary at all
- independently resolve/reconstruct durable evidence — the
  `spider_evidence_read` MCP tool (`spider_mcp/src/tools/evidence_read.rs`,
  realized by `SCORPION_MCP_CANONICAL_EVIDENCE_READ_001`) is the sole
  authorized shipping consumer of the persistence-touching
  `EvidenceRef::resolve`/`read_evidence` seam, and returns the canonical
  `EvidenceBundle` exactly as persisted — no re-fetch, no recalculated
  hash, no reconstructed provenance, no normalization; "read means read"

Current conformance: `spider_cli` and `spider_mcp` call canonical seams
(`fetch_single_page_with_options`, `build_evidence`, `Website` crawl seam,
search providers, and — `spider_mcp` only, through the two authorized
files above — `audit_page()`/`EvidenceRef::resolve()`/
`domain_runtime::open_shared_domain_store()`) and hold no duplicate
domain models. `spider_cli::oauth` constructs an HTTP client as a
documented authentication-flow exception — not an acquisition path.

---

## 9. Upstream Compatibility Boundary

The Spider upstream compatibility graph (enumerated in
`SCORPION_ARCHITECTURE.md` §5) remains because removal would break
upstream-compatible behavior. Rules:

- New Scorpion code must not depend on it directly.
- It must not become an alternate execution path for canonical capabilities.
- It may execute transitively as the *implementation* of an approved
  canonical boundary primitive (see §3, category 2). Truthful boundary
  language is "implementation dependency behind an approved boundary," never
  "never executed."
- Canonical dependence on it flows only through the approved boundary
  primitives listed in §3 of this document and `SCORPION_ARCHITECTURE.md`.
- That list is closed: adding a new boundary primitive requires an
  architecture frontier, not a local decision.

---

## 10. Change Discipline

Any change to this SDD, to canonical ownership, or to the approved shared
primitive list is itself an architecture-critical frontier and follows
`SCORPION_PROCESS.md` — including the two-branch process where a genuine
competing design decision exists.
