# Scorpion — Canonical Architecture & Guardrail Contract

**Status:** ACTIVE — enforced by architecture tests and code review.

**Baseline:** `3031b1a25b2cd1d1207f1f039a5c5e6bb36bcb24`

This document is the machine-readable source of truth for Scorpion's canonical
architecture. It records what is canonical, what is compatibility-only, what is
forbidden, and how future frontiers must be scoped.

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
| One-shot fetch/evidence | `spider/src/utils/evidence.rs` | `Website` (UPSTREAM_COMPAT), `transport` | Independent client construction | `fetch_single_page_with_options()` | `fetch_single_page()` (LEGACY predecessor) |
| Crawl/scrape orchestration seam | `spider/src/website.rs` — **methods only**: `Website::with_transport()`, `crawl_raw()`, `scrape()` | `transport`, `page` | New transport stacks | `Website::with_transport()` + `crawl_raw()`/`scrape()` | `configure_base_client` (UPSTREAM_COMPAT) |
| Streaming artifact download | `spider/src/features/artifact_download_execution.rs` | `transport::execute_streaming_request`, `uring_fs` | `Website`, `Page`, independent clients | `execute()` | None |
| Acquisition binding | `spider/src/features/acquisition_binding.rs` | `discovery_target`, `evidence` | Network, transport | `bind()`/`execute()` | None |

**Clarification on `website.rs`:** The `Website` type and its crawl/scrape methods are the canonical public seam for multi-page acquisition. The internal client construction (`configure_base_client`, proxy rotation, legacy redirect policies) is UPSTREAM_COMPAT — retained for upstream parity, not to be extended by new Scorpion capabilities. A canonical seam may internally depend on upstream-compatible machinery; the seam and the machinery are classified separately.

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
| Transport provenance stamping | `spider/src/page.rs` — **field/method only**: `Page::transport`, `Page::transport()` | `ACQUISITION_TRANSPORT_SCOPE` | Caller-supplied policy | `Page::transport()` | None |
| Acquisition scope | `spider/src/features/transport.rs` | `tokio` | Independent provenance | `ACQUISITION_TRANSPORT_SCOPE` | None |

**Clarification on `page.rs`:** `Page` is the upstream Spider response/evidence vocabulary. The `Page::transport` provenance field and its getter are canonical Scorpion additions — the single source of truth for transport provenance. The rest of `Page` (structure, `page::build`, legacy link extraction, anti-bot detection) is UPSTREAM_COMPAT. The canonical evidence seam is `spider/src/utils/evidence.rs`; `Page` is consumed by it as an upstream primitive, not owned by it.

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
| WATCH/MONITOR | **BLOCKED** | No canonical model/seam exists yet; must not be implemented until a frontier establishes canonical ownership. |

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

| Path | Why Retained | Constraint |
|---|---|---|
| `Website::configure_base_client` (reqwest/wreq variants) | Legacy crawl stack | Must not be extended for new transport work |
| `configuration.proxies` rotation list | Backward compatibility | Must not be extended; rejected under Tor |
| `socks://` → `http://` rewrite | Linux reqwest compatibility | Must not be extended; rejected under Tor |
| `setup_strict_policy` / `setup_redirect_policy` | Legacy redirect policies | Must not be extended; canonical transport uses `pin_redirect_policy` |
| `fetch_page_html*` variants | Legacy fetch matrix | Must not be extended |
| `spider_mcp::evidence` re-export shim | API stability | Must not be extended; use `spider::utils::evidence` directly |
| `spider_agent::automation` re-export layer | Agent compatibility | Compat only; new code should import canonical crates directly |

---

## 6. Legacy / Rejected / Unknown Map

### 6.1 LEGACY

| Path | Status | Rule |
|---|---|---|
| `spider_agent/src/search/*` | Duplicate of `spider::features::search_providers` | Do not extend; converge or freeze |
| `RemoteFetcher` (`fetcher.rs`) | Coarser hook than `HttpFetchEngine` | Keep both; document `HttpFetchEngine` as preferred for transport swaps |
| `build_evidence_with_transport` | Compatibility shim over `build_evidence`; superseded by canonical seam | Do not extend |
| `page::build` / `Page` `decentralized` variants | Weaker behavior | Flag as legacy |
| `Agent::new_page_with_url` | Deprecated | Schedule removal at next major |
| `spider_worker` | Decentralized crawling proxy server | Defaults preserve legacy behavior; hardening is env-opt-in. Not a canonical owner. |

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
`AcquiredArtifact` are only defined in their canonical modules.

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

New violations are caught by `cargo test -p spider --test architecture_guardrails`.
