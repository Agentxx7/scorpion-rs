# SDD — SCORPION_CANONICAL_TRANSPORT_LEAF_EXTRACTION_001

Status: SHARED (SHA-256 pinned across experiment branches)
Baseline: `41c5ad83232a54eba55071ddd8850c66b3bd07d9`
Prerequisite for: `SCORPION_SPIDER_AGENT_SEARCH_STACK_CONVERGENCE_001` (BLOCKED)

## 1. Purpose

Move Scorpion's canonical transport ownership out of the `spider` crate into a
neutral leaf crate `spider_transport` that sits below every canonical
consumer, so that exactly one crate owns network transport semantics and so
that a future `spider_search` (or any canonical consumer) can use the
canonical transport without a dependency cycle. No product behavior is added
or changed.

## 2. Current dependency graph (audited)

`spider/src/features/transport.rs` (1494 lines) has exactly FOUR spider-local
dependencies:

| # | Dependency | Used by | Audit classification |
|---|---|---|---|
| 1 | `crate::website::Website::is_ssrf_redirect` | `ssrf_screened_base_policy` | PURE associated fn (no `&self`, no `Website` state; deps: `std::net` + `url` only). The canonical SSRF primitive. Other callers: `website.rs` redirect policies (2 sites) + its tests. **Extraction target — see §4 designs.** |
| 2 | `crate::configuration::get_ua(false)` | `build_tor_client`, `build_default_streaming_client` | Spider-owned UA source (ua_generator spoof, or crate name/version fallback). **PARAMETERIZE**: leaf client builders take `user_agent: &str`; spider façade wrappers inject `get_ua(false)` so every existing call site and the public seam signature are preserved. |
| 3 | `crate::features::secret_request_headers::SecretRequestHeaders` | `execute_streaming_request` param | Zero spider-local deps (reqwest::header only). **MOVE TO LEAF.** |
| 4 | Doc-only references (`crate::page::Page`, `crate::utils::*`) | comments | No code dependency. Task-local readers/writers live in spider and will reference the leaf through the façade (spider → leaf is the allowed direction). |

`secret_request_headers.rs` (188 lines) is already leaf-pure.

Everything else in transport.rs depends only on `reqwest`, `url`, `tokio`,
`serde` (optional) — leaf-safe.

## 3. Target crate graph

```
spider_transport            (leaf: reqwest, url, tokio, serde?  — NO path deps)
   ▲              ▲
   │              │
spider       (future spider_search, unblocked — proven, not built here)
   │
   ├─ features::transport              thin façade
   └─ features::secret_request_headers thin façade
   ▲
spider_cli, spider_mcp (unchanged call sites)
```

Forbidden: `spider_transport` → any workspace crate. Proven by guardrail
(`spider_transport/Cargo.toml` contains no `path =` dependency).

## 4. Canonical ownership and the two truthful designs

The audit found exactly ONE genuinely contested ownership decision: the SSRF
redirect classifier. It is a pure function; the card permits either downward
extraction or neutral injection, and forbids duplication. Two valid
architectures result:

- **DESIGN A — SSRF primitive moves DOWN.** `is_ssrf_redirect` moves into
  `spider_transport` (its sole owner). `Website::is_ssrf_redirect` becomes a
  one-line delegating associated fn (no logic duplicated). Leaf redirect
  policies close over the leaf-owned classifier — screening is
  non-bypassable by construction for every consumer of the leaf.
- **DESIGN B — SSRF injection.** `is_ssrf_redirect` stays in `spider`
  (Website remains its sole owner). The leaf's redirect-policy constructors
  take the classifier as a parameter (`fn(&url::Url) -> bool`-shaped,
  `Send + Sync + 'static`); spider's façade wrappers inject
  `Website::is_ssrf_redirect`. The leaf stays free of SSRF policy ownership,
  at the cost of a bypassable injection point and extra parameter plumbing
  on every client-construction seam.

Both designs share everything else: the UA parameterization (§2 #2), the
façade strategy (§5), the feature topology (§6), and the moved inventory
(§7). Both are implementable without duplication, cycles, or weakened
security. (A third "do nothing different" packaging variant — e.g. splitting
secret headers into its own micro-crate — is not a genuinely different
architecture and is not fabricated as one.)

## 5. Façade contract (`spider::features::transport`, `…::secret_request_headers`)

- All TYPES are pure re-exports (`pub use spider_transport::{…}`), preserving
  `spider::features::transport::{TransportPolicy, TorTransportConfig,
  TransportError, TransportRequest, TransportMode, AcquisitionTransport,
  is_onion_url, validate_target, …}` and
  `spider::features::secret_request_headers::{SecretRequestHeaders,
  SecretHeaderError}` paths. Type identity is therefore literal.
- The only non-re-export items allowed in the façade are thin delegating
  wrappers that inject spider-owned defaults (`get_ua(false)`, and under
  design B the SSRF guard): `execute_streaming_request(url, policy, headers)`
  (public 3-arg signature preserved), `build_tor_client(policy)`
  (`pub(crate)`), plus the `pub(crate)` re-exports spider-internal callers
  use (`is_onion_host`, `ACQUISITION_TRANSPORT_SCOPE`,
  `current_acquisition_transport`, `target_dns_suppressed`,
  `acquisition_transport_for`, `CrawlBoundary`, `crawl_boundary_allows`,
  `apply_transport_policy`-adjacent internals as needed).
- Façades must not: construct clients, implement Tor behavior, implement
  validation/redirect/SSRF logic, duplicate error semantics, or contain
  fallback execution.
- `Website::is_ssrf_redirect` call sites (both designs) keep compiling:
  design A via one-line delegation to the leaf, design B unchanged.
- The leaf makes formerly `pub(crate)` items `pub` where spider needs them;
  spider's façade restores the original visibility (`pub(crate) use`).

## 6. Feature topology (semantics preserved, names forwarded)

- Leaf features: `default = []`; `tor` (enables `reqwest/socks`; gates
  `apply_transport_policy`, `build_tor_client`, `TorTransportConfig::endpoint`,
  the `transport_tor`-compiled `build_streaming_client` variant); `serde`
  (gates Serialize/Deserialize incl. the hand-written validating
  `Deserialize` for `TorTransportConfig`).
- Leaf has NO `wreq`/`cache_request` concepts — inside the leaf those
  `not(...)` cfgs are dropped (the leaf never has an alternate stack). The
  spider façade re-applies the identical
  `#[cfg(all(not(feature = "wreq"), not(feature = "cache_request")))]` gates
  on the re-exports/wrappers, and the `transport_tor`-gated items stay gated
  through `spider_transport/tor` forwarding — the availability matrix for
  every spider feature combination is unchanged.
- `spider/Cargo.toml`: `spider_transport = { path, version, default-features=false }`
  non-optional (transport module is ungated today); `transport_tor =
  ["reqwest/socks", "evidence", "spider_transport/tor"]`; `serde` forwards
  `spider_transport/serde`.
- `TransportPolicy`/`TorTransportConfig` `PartialEq`: derived
  UNCONDITIONALLY in the leaf (pure superset — the baseline
  `cfg_attr(all(not(regex/openai/cache_openai/gemini/cache_gemini)))` gating
  exists only to support `Configuration`'s conditional `PartialEq` derive,
  which keeps working against an always-`PartialEq` field type). No semantic
  drift.
- Tor fail-closed matrix unchanged: Tor without `transport_tor` →
  `TorNotCompiled`; `.onion` under `Default` → `OnionRequiresTor`;
  cross-transport/different-onion redirects → `RedirectTransportViolation`;
  SSRF redirect → rejected; credential-bearing/malformed Tor endpoints →
  rejected at construction AND at deserialization.
- Leaf reqwest dep mirrors spider's target split (native: brotli/gzip/
  deflate/zstd/stream; wasm: no zstd) so decompression behavior is
  unchanged; `tor` adds `socks`.

## 7. Moved inventory (leaf owns exactly these)

From `transport.rs`: `TransportPolicy`, `TorTransportConfig` (+hand
Deserialize +redacted Debug), `TransportError`, `TransportRequest`,
`TransportMode`, `is_onion_host`, `is_onion_url`, `validate_target`,
`apply_transport_policy`, `ssrf_screened_base_policy`, `pin_redirect_policy`,
`AcquisitionTransport` (+`label`), `ACQUISITION_TRANSPORT_SCOPE`,
`current_acquisition_transport`, `target_dns_suppressed`,
`acquisition_transport_for`, `CrawlBoundary` (+`from_seed`),
`crawl_boundary_allows`, `TOR_CONNECT_TIMEOUT`/`TOR_READ_TIMEOUT`/
`TOR_TOTAL_TIMEOUT`/`TOR_REDIRECT_LIMIT`, `build_tor_client`,
`build_streaming_client` (both cfg variants), `build_default_streaming_client`,
`execute_streaming_request`, and the full test module.
From `secret_request_headers.rs`: `SecretRequestHeaders`, `SecretHeaderError`
(+tests).
Design A additionally: `is_ssrf_redirect` (from `website.rs`).

Nothing else moves. `Website`'s own client construction, multi-proxy
rotation, `fetch_page_html*`, evidence/crawl orchestration: untouched.

## 8. Migration plan

1. Create `spider_transport` crate; move §7 inventory verbatim except the two
   audited adaptations (UA parameterization; design-specific SSRF handling).
2. Convert spider's two modules to façades; wire Cargo features; update
   `website.rs` SSRF delegation (design A) or injection call sites (design B).
3. Update `spider/tests/architecture_guardrails.rs`: canonical-path
   assertions re-pointed at the leaf; façade-purity guards; leaf-has-no-
   path-deps guard; uniqueness guards per primitive; synthetic negative
   proofs. Allowlists shrink or stay equal (transport entries move, page.rs
   grandfathered entry stays).
4. Run the shared acceptance suite + full regression matrix.

## 9. Acceptance criteria (shared suite)

The 20 items in the frontier card §PHASE 3, mechanized as
`spider/tests/transport_leaf_acceptance.rs` (identical SHA-256 on both
branches), including compile-time type identity
(`spider::features::transport::TransportPolicy` IS
`spider_transport::TransportPolicy`, same for `SecretRequestHeaders`),
offline behavioral tests (local TCP fixtures; fail-closed matrices), and
source-scan guardrails (single owner per primitive, façade purity, no path
deps, no alternate client construction by canonical consumers).

## 10. DONE definition

- One canonical transport owner; façades pure; every baseline caller
  compiles unchanged through the same paths.
- Shared acceptance suite green; `architecture_guardrails` green (updated,
  allowlists shrunk-or-equal); transport/Tor/artifact/provider tests green;
  feature matrix (`default`, `evidence`, `transport_tor`, no-default,
  `wreq`, `cache_request`, `transport_tor+wreq` rejection path) green.
- `cargo check -p spider_transport` with zero features compiles with no
  workspace path deps.
- A hypothetical `spider_search → spider_transport` edge is now legal
  (proven by dependency-direction guardrail; the crate itself is NOT built
  here).
- rustfmt/clippy clean; diff checks pass; winner on main as uncommitted
  diff; loser rejected and removed; nothing committed, nothing pushed.

## 11. Negative criteria (any one ⇒ design rejected)

- Duplicate or shadowed security primitive (policy/validator/classifier/
  redirect/SSRF/Tor builder/secret headers) in any second location.
- Façade containing implementation logic (client construction, policy
  branching beyond a single delegating call).
- Any `spider_transport` path dependency on a workspace consumer.
- Feature combination that silently downgrades security or changes the
  availability matrix vs baseline.
- Public path breakage beyond zero (all `spider::features::transport::*`
  and `…::secret_request_headers::*` paths must resolve to the same items).
- Signature change to the public `execute_streaming_request` 3-arg seam.
