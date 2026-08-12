# SDD — SCORPION_SPIDER_AGENT_SEARCH_STACK_CONVERGENCE_001

Status: APPROVED FOR IMPLEMENTATION
Baseline: `108167e7fddec0c45dc3288c8083feb1b7cd4313`
Prerequisite: `SCORPION_CANONICAL_TRANSPORT_LEAF_EXTRACTION_001` (CLOSED)

## 1. Audit

The clean baseline has two active search stacks. Spider owns the larger surface:
one `SearchProvider` seam, `SearchOptions` (including `include_keywords`),
`TimeRange`, result models, `SearchError`, and Serper, Brave, Bing, Tavily, and
SearXNG providers. Its seam accepts `Option<&reqwest::Client>`; every provider
either uses that arbitrary client or constructs a local `ClientBuilder`.

`spider_agent` separately owns another object-safe provider seam, shadow
options/time-range/result/error types, and duplicate Serper, Brave, Bing, and
Tavily implementations. Agent orchestration lends its unrelated general HTTP
client to this stack. CLI and MCP consume Spider's surface; Spider configuration
also exposes those models. Neither stack retries provider requests or silently
falls back to another provider.

Authentication is provider-specific and must remain so: Serper, Brave, and
Bing use their established secret headers; Tavily sends its API key in the
established JSON body; SearXNG requires an operator-supplied base URL and no
standard API key. Existing 401/403, 429, other-status, malformed-response,
pagination/limit, query mapping, and result mapping behavior is retained.

## 2. Canonical ownership and graph

`spider_search` is the sole neutral owner of the search seam, models, error,
and exactly one implementation of every provider:

```text
spider_transport
      ↑
spider_search
   ↑       ↑
spider   spider_agent
   ↑
CLI / MCP
```

`spider_search` has no path dependency on Spider, spider_agent, CLI, MCP, or
other higher-level consumers. `spider_transport` remains consumer-neutral and
has no reverse dependency. The resulting graph permits both Spider and
spider_agent to consume identical search types without a cycle.

## 3. Canonical seam and models

The one object-safe async `SearchProvider` receives only query and canonical
options. It never receives a network client. It retains `provider_name` and
`is_configured` so agent orchestration can preserve truthful configuration
checks.

The canonical model is Spider's superset: `SearchOptions`, `TimeRange`,
`SearchResult`, `SearchResults`, plus SearXNG media/news result types. The
canonical six-variant `SearchError` retains existing display strings and source
semantics. `spider_agent` re-exports these exact types; no conversion layer or
shadow model remains.

## 4. Transport semantics

Every provider request executes through `spider_transport` under explicit
`TransportPolicy::Default`. Providers may assemble URLs, query pairs, headers,
and JSON bytes, but may not construct, receive, or expose a client and may not
call `.send()`.

The closed transport leaf receives one neutral extension to its canonical
executor: HTTP method, optional body bytes, optional content type, secret
headers, policy, and user-agent. It reuses the existing target validation,
Default/Tor construction, redirect pinning, SSRF screening, timeouts, secret
header application, acquisition provenance, and streaming response semantics.
The existing GET streaming API delegates to this generalized executor, so no
second client builder or transport implementation is introduced.

There is no fallback transport. A transport error becomes
`SearchError::RequestFailed`; no alternate client or provider is attempted.

## 5. Feature topology

`spider_search` always exposes the lightweight seam and models because
spider_agent already exposes `SearchOptions`, `TimeRange`, and `SearchError`
without its search execution feature; making those conditional would be a
public compatibility regression. The `search` marker enables the provider
module, and each `search_<provider>` enables exactly that provider and implies
`search`. Models retain serde support needed by existing surfaces. Transport is
a non-optional dependency because every enabled provider has exactly one
execution path.

Spider and spider_agent retain their public search feature names and forward
them to `spider_search`. A provider type is absent unless its feature is
enabled. CLI/MCP continue to consume Spider's public façade. Missing features
fail closed at compile time.

## 6. Façades and removal

`spider::features::search`, `spider::features::search_providers`, and
`spider_agent::search` contain re-exports and documentation only. Spider's five
provider implementation files and spider_agent's four provider files are
physically removed. Agent-owned `SearchOptions`, `TimeRange`, and `SearchError`
definitions are replaced with canonical re-exports. Agent dispatch calls the
canonical seam without lending `self.client`.

CLI and MCP changes are limited to removing obsolete client arguments. No new
provider, retry, product behavior, proxy behavior, or fallback is introduced.

## 7. Two-branch applicability

`TWO_BRANCH_NOT_APPLICABLE`.

A Spider-owned canonical stack would require spider_agent to depend on Spider,
while Spider already optionally depends on spider_agent, producing a cycle.
An agent-owned stack would invert ownership into a higher-level orchestrator
and force Spider/CLI/MCP upward. A transport-owned stack would put provider and
product models into the neutral transport leaf. Retaining client injection is
explicitly forbidden. Therefore only a sibling neutral capability crate below
both consumers satisfies every graph and ownership constraint. A second branch
would be cosmetic rather than a genuine architecture.

## 8. Shared acceptance and guardrails

The shared contract proves all twenty frontier requirements: unique seam,
providers, models, and error; type-identical façades; canonical transport
dependency; absence of raw clients/client lending/fallback; truthful auth and
errors; feature gating; behavior preservation; physical legacy removal; and
negative scanner proofs for duplicate seams/providers/models, raw HTTP, and
canonical-to-legacy dependencies.

Existing architecture allowlists may only shrink. Search-provider exceptions
for local client construction are removed. No new exception is permitted.

## 9. DONE

- One canonical neutral search owner and one implementation per provider.
- One canonical model/error surface and object-safe provider seam.
- All search HTTP uses `spider_transport`; no client lending or fallback.
- Spider and spider_agent are thin façades; legacy implementations are gone.
- Shared acceptance, guardrails, provider/consumer/transport tests, feature
  checks, reverse-dependency checks, rustfmt, relevant strict clippy, and diff
  checks pass.
- Known baseline debt remains separately classified where not directly removed
  by mandatory legacy deletion.
- The winning diff remains uncommitted on main; nothing is pushed.

## 10. Rejection conditions

Any duplicate seam/model/error/provider, raw provider client, `.send()` in a
provider, arbitrary client argument, silent fallback, consumer implementation
inside a façade, canonical-to-legacy dependency, dependency cycle, feature
fail-open, or allowlist expansion rejects the design.
