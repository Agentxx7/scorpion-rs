# Canonical credential/cache isolation

Frontier: `SCORPION_CANONICAL_CREDENTIAL_CACHE_ISOLATION_001`

Baseline: `326b8d81`

## Purpose

Close all remaining live paths where authentication material can
influence cache identity, persisted cache content, or remote cache
uploads.

## A. Audit findings — the confirmed live gap

Tracing `CacheOptions::Authorized`/`SkipBrowserAuthorized` end to end
found the HTTP-only legacy write paths already correctly fail closed
(`cache_chrome_response`, `cache_http_response_skip_browser`,
`cache_chrome_response_from_cdp_body`, `get_cached_url_base` in
`spider/src/utils/mod.rs` all reject `Authorized`/`SkipBrowserAuthorized`
before touching the cache — this predates this frontier and was left
alone). The **live, exploitable gap** was the Chrome/browser-rendered
path:

- `cache_enabled()` (`spider/src/utils/mod.rs`) — the gate deciding
  whether `set_document_content_if_requested_cached` runs at all —
  treated `CacheOptions::Authorized(_)` as **cache-enabled**, directly
  contradicting every sibling HTTP write path and `cacheable_request()`'s
  canonical rule in `cache_request.rs` (never persist an authenticated
  request, full stop).
- When enabled, `set_document_content_if_requested_cached` extracts the
  live token via `cache_auth_token(cache_options)`, derives a site key
  from it (`chromiumoxide::cache::manager::site_key_for_target_url` —
  blake3-hashed, so at least not persisted raw), and calls
  `page.spawn_cache_listener(...)` with `dump_remote: Some("true")`
  whenever not explicitly read-only.
- `Page::spawn_cache_listener` →
  `chromiumoxide::cache::manager::spawn_response_cache_listener` →
  `handle_single_response`: for **every** intercepted CDP
  `Network.responseReceived` event, this reads the real request/response
  headers from the live browser session (`ev.response.request_headers`/
  `ev.response.headers`) — completely unscrubbed — computes
  `cache_key = create_cache_key_raw(url, method, auth)` with the **raw,
  unhashed** credential embedded directly in the composed string, and
  (when `dump_remote` is set) uploads a `DumpJob { cache_key,
  request_headers, response_headers, body, ... }` to a remote cache
  server.
- `create_cache_key_raw`'s raw output is also what `CACacheManager`
  (`http-cache`'s disk cache manager, backed by the `cacache` crate)
  passes to `cacache::write`/`cacache::read` — `cacache`'s own index
  format persists the literal key string in its on-disk `Metadata`
  (`key: String`), so an unhashed key carrying a raw credential would be
  written to disk as plaintext in cacache's index metadata, not just held
  in memory.
- **Requirement #6's "existing architectural record" gap, corrected**:
  only `create_site_key`/`site_key_for_target_url` hashed their output
  (blake3, via the shared `hash_key_v1` helper). `create_cache_key_raw`
  — the function that actually produces the identity used for local
  cacache storage *and* the `DumpJob::cache_key` uploaded remotely — did
  not hash at all. Any assumption that blake3 protected this function
  was false; the fix (below) makes it true.
- **Requirement #3's gap**: `Configuration::get_cache_options()` only
  inspected the `Authorization` header. A cookie-jar-authenticated crawl
  (`Configuration::cookie_str`, or an explicit `Cookie` header, with no
  `Authorization` header) was classified as plain `CacheOptions::Yes`/
  `SkipBrowser` — cache-eligible, with no fail-closed protection at all.
- **Requirement #4's gap**: even for requests `get_cache_options()`
  *did* correctly classify as unauthenticated, no write path scrubbed
  `Authorization`/`Cookie`/`Proxy-Authorization` from request headers or
  `Set-Cookie` from response headers before persisting locally or
  dumping to `DumpJob` — a header a caller never classified as "auth"
  (a raw `Cookie` set directly, an origin's `Set-Cookie`) had no
  independent check keeping it out.

## B. Fixes

1. **`cache_enabled()`** (`spider/src/utils/mod.rs`) now matches only
   `Some(CacheOptions::Yes)`. An authenticated Chrome crawl no longer
   reaches `set_document_content_if_requested_cached` at all — no site
   key derived from the live token, no listener installed, no remote
   dump enabled.
2. **`Configuration::get_cache_options()`** now also inspects
   `self.cookie_str` and an explicit `Cookie`/`cookie` header, falling
   back to the `Authorization` classification's exact same
   `Authorized`/`SkipBrowserAuthorized` shape. Cookie-jar authentication
   now participates in cache eligibility exactly like an `Authorization`
   header.
3. **`chromiumoxide::cache::manager::create_cache_key_raw`** now hashes
   its composed `{method}:{uri}:{auth}` string with the same `hash_key_v1`
   (blake3) primitive `create_site_key` already used, before returning
   it. Deterministic (identical inputs still produce identical keys —
   dump/retrieve round-tripping is unaffected), but the raw credential
   never appears in the returned string.
4. **`contains_disqualifying_secret_header`** — a new, shared,
   case-insensitive check for `Authorization`/`Cookie`/
   `Proxy-Authorization`/`Set-Cookie` header names — added independently
   in both `vendor/chromey/src/cache/manager.rs` and
   `spider/src/utils/mod.rs` (two copies; see "Ownership" below for why),
   and called as defense in depth, **independent of** the
   `CacheOptions` classification, at every legacy write site:
   `handle_single_response` and `put_hybrid_cache` (chromey);
   `cache_chrome_response`, `cache_chrome_response_from_cdp_body`, and
   `cache_http_response_skip_browser` (spider). A disqualifying header on
   either side (request or response) skips all local caching, session-
   cache seeding, and remote dumping for that response — no partial
   write, no partial dump.

## C. Ownership / why two copies of the header check

`spider`'s three call sites are gated behind different, non-overlapping
feature combinations (`chrome_remote_cache` for two of them,
`cache_request` alone — without `chrome_remote_cache` — for the third),
and only `chrome_remote_cache` pulls in `chromey/_cache` (the feature
gating chromiumoxide's own cache module). A single shared cross-crate
call would not compile in every valid `spider` feature combination, so
`spider` keeps a structurally identical, independently-defined copy
rather than adding a hard dependency edge that doesn't exist today. Each
copy is defined exactly once within its own crate (guardrailed).

## D. Set-Cookie / authenticated-response fail-closed

Requirement #5 ("Authenticated responses and Set-Cookie-bearing
responses must fail closed... unless an explicit canonical policy proves
them safe") is satisfied on the **legacy/vendor-chromey** side by the
combination of B.1–B.2 (no authenticated request ever reaches a write path
at all) and B.4 (a `Set-Cookie` response header disqualifies caching
independent of how the request was classified — no canonical policy in
this codebase proves a `Set-Cookie`-bearing response safe to cache, so
none is asserted).

### D.1 The canonical `cache_request.rs` gap (found in operator adversarial review)

An operator-driven adversarial review of the first pass at this frontier
found a second, separate live gap: `spider/src/cache_request.rs`'s
`cacheable_request()` only ever inspects the outgoing **request's** headers
(`Authorization`/`Cookie`/`Proxy-Authorization`/`Set-Cookie`, defensively,
though a client sending `Set-Cookie` on a request is not meaningful). It has
no visibility into headers the **origin only sends back on the response** —
a credential-free request whose fetched response carries `Set-Cookie` had no
independent check keeping it out of `http-cache`'s local disk/memory
persistence.

**Fix 1 — the fresh-write path.** `Middleware::policy`/`policy_with_options`
are the only hooks in `http_cache::Middleware` that see the actual fetched
`&HttpResponse` before `http-cache`'s `should_cache_response` consults
`CachePolicy::is_storable()` on the real write path (`HttpCache::run` ->
`remote_fetch` -> `middleware.policy(&res)`, confirmed by reading
`http-cache-1.0.0-alpha.6/src/lib.rs`). A new `fail_closed_on_set_cookie`
helper checks the response parts for `Set-Cookie` and, if present, inserts a
synthetic `Cache-Control: no-store` — an absolute veto inside
`CachePolicy::is_storable()` ahead of every other storability rule — before
`CachePolicy::new`/`new_options` ever see the response. Both `policy` and
`policy_with_options` route through it. This closes the case reported in the
original review: a first unauthenticated request whose response carries
`Set-Cookie` is never persisted, and no subsequent lookup can retrieve it
from the cache, while ordinary `Set-Cookie`-free responses continue to cache
normally.

**Fix 2 — the 304-revalidation re-persist path (found during this fix's own
adversarial verification, not by the operator).** Tracing `should_cache_response`
call sites in `http-cache` further found that `HttpCache<T>::conditional_fetch`'s
304 branch never calls `Middleware::policy` at all: it calls
`policy.after_response(...)` directly on the **already-stored**
`CachePolicy` object and then **unconditionally** calls `CacheManager::put`,
with no `is_storable`/`should_cache_response` check in that branch. When the
304 response's validators (`ETag`/`Last-Modified`) don't match the stored
ones — including the common case of a bare `304` that echoes no validators
at all — `CachePolicy::after_response` folds the raw 304 response headers in
**verbatim**, which would let an origin smuggle a fresh `Set-Cookie` straight
into that unconditional re-persist, bypassing Fix 1 entirely (since
`fail_closed_on_set_cookie` is never invoked for this write). Because
`Middleware` exposes no hook over `after_response`/the 304 put, the only
remaining interception point is `remote_fetch` itself
(`materialize_network_response`), which sees every raw network response,
including 304s, before it re-enters `http-cache`'s revalidation-merge
internals. The fix strips `Set-Cookie` from a materialized response
specifically when its status is 304 — before it can ever reach
`after_response`'s merge — so there is nothing left to smuggle in. Ordinary
200 responses are untouched by this: their `Set-Cookie` still reaches the
caller and is kept out of the cache by Fix 1. (A 304 has no body of its own,
so there is no comparable "the caller needs this value" requirement pulling
the other way, unlike a 200.)

### D.2 Final verified wire-truth boundary (operator pre-commit check)

A final pre-commit adversarial pass verified that neither fix falsifies the
network exchange it protects — cache-persistence isolation and truthful
response observation are enforced at two different, deliberately distinct
boundaries:

- **Fresh/`Network`-origin `Set-Cookie` remains observable to the caller,
  and is only kept out of persistence.** `fail_closed_on_set_cookie` builds
  its synthetic `Cache-Control: no-store` from a freshly constructed
  `response.parts()?` clone (`HttpResponse::parts(&self)` returns a new
  owned value each call) — it never mutates the actual `HttpResponse`
  object. `http-cache`'s `should_cache_response` gate only decides whether
  `manager.put` runs; the *response itself*, real `Set-Cookie` intact, is
  what's returned to the caller either way (`if is_cacheable {
  manager.put(...) } else { Ok(cond_res) }`). Proven by an assertion in
  `set_cookie_response_is_never_persisted_or_served_from_cache`:
  `response.headers.get(SET_COOKIE) == Some("session=must-never-be-cached")`
  on every `ResponseOrigin::Network` iteration.
- **304-revalidation `Set-Cookie` is suppressed before the cache merge
  because `http-cache` (v1.0.0-alpha.6) exposes no narrower,
  persistence-only interception point for this branch.**
  `HttpCache::conditional_fetch`'s 304 arm computes `after_response`
  directly on the already-stored `CachePolicy` (never through
  `Middleware::policy`) and calls `CacheManager::put` unconditionally, with
  no storability gate. `CacheManager::put` (`managers/moka.rs`,
  `managers/cacache.rs`) takes ownership of the response, persists it, and
  returns that *same* object — "returned to caller" and "persisted to
  disk/memory" are structurally the same value for this code path; the full
  `Middleware` trait (10 methods) has no hook between the merge and the
  `put` call. `materialize_network_response`/`remote_fetch` is therefore
  not an arbitrarily broad suppression point — it is the only code in
  `cache_request.rs` that runs before the raw response ever reaches
  `http-cache`'s internals, i.e. the narrowest boundary the library's
  public API actually makes available.
- **The resulting exchange is truthfully classified as
  `ResponseOrigin::ReconstructedCache`, never `Network`.** `http-cache`
  itself tags a 304-revalidated response `x-cache: HIT`
  (`cache_status_headers: true` by default), which `reconstruct_response`
  reads to set `ResponseOrigin::ReconstructedCache`. Scorpion's own
  provenance model therefore never claims this exchange was a first-hand,
  untouched live-network capture in the first place — the suppression
  applies only to an exchange already labeled as a cache reconstruction.
  Proven by an assertion in
  `set_cookie_on_a_304_revalidation_response_is_never_persisted`:
  `response.origin == ResponseOrigin::ReconstructedCache` on every
  post-initial iteration.

Both fixes are scoped entirely to `spider/src/cache_request.rs`; neither
touches request-side classification, cache identity/key derivation, or
ordinary credential-free caching semantics. `cache_request.rs` has no remote
upload mechanism at all (no `DumpJob`, no remote-cache client — purely local
`CACacheManager`/`MokaManager` persistence), so the "remote cache uploads"
leg of the overall goal is inherently satisfied by absence of such a
mechanism in this file.

## E. Non-goals honored

No redesign of the complete legacy cache — every fix is a targeted
guard, a hash, or a shared predicate, not a restructuring. `spider_agent`
untouched. CAPTCHA untouched. Tracks 1–10 (Watch/state/persistence)
untouched. No CI execution added. No broadening into generic credential
storage cleanup — every change traces directly to the cache-identity/
cache-persistence/remote-upload invariant.

## F. Guardrails/tests

`spider/tests/architecture_guardrails.rs` — 8 new guardrails:
`chrome_cache_enabled_never_treats_authorized_as_cache_enabled`,
`legacy_write_paths_fail_closed_on_authorized_cache_options`,
`cookie_jar_authentication_participates_in_cache_eligibility`,
`disqualifying_secret_header_check_is_defined_once_per_crate_and_reused`,
`vendor_chromey_cache_key_is_hashed_never_the_raw_credential`,
`canonical_cache_request_fails_closed_on_set_cookie_response`,
`canonical_cache_request_strips_set_cookie_from_304_revalidation_responses`,
`no_shadow_credential_aware_cache_policy_in_cli_or_mcp`.

### F.1 `spider/src/cache_request.rs` — 4 new tests (operator-mandated fix)

`set_cookie_response_is_never_persisted_or_served_from_cache` — an
otherwise-cacheable (`Cache-Control: public, max-age=3600`) response that
also carries `Set-Cookie` is fetched 3 times; every single fetch must
originate from the network (never a cache hit). Also asserts (D.2) that
`response.headers.get(SET_COOKIE)` still returns the real value on every
`Network`-origin iteration — proving the persistence veto is policy-local
and never falsifies what the caller observes.

`response_without_set_cookie_still_caches_normally` — non-regression: an
ordinary, `Set-Cookie`-free cacheable response still serves the second
request from the local cache with zero further network traffic.

`request_side_cookie_and_proxy_authorization_headers_still_bypass_cache` —
non-regression: a plain (not `secret_headers`) `Cookie` or
`Proxy-Authorization` request header still forces every request to the
network, exercising `cacheable_request()`'s existing request-side rejection
independently of the `secret_headers`-based `Authorization` case already
covered by `secret_headers_bypass_lookup_and_persistence`.

`set_cookie_on_a_304_revalidation_response_is_never_persisted` — proves Fix
2 (D.1): an initial `200` (with `ETag`, `max-age=0`, no `Set-Cookie`) is
followed by repeated conditional-GET revalidations that all receive a bare
`304` carrying `Set-Cookie` (no validators echoed, so `http-cache`'s
validator-match resolves to "does not match" and the merge would otherwise
copy the 304's headers in verbatim). Confirmed, by temporarily disabling the
fix, that this test fails without it (`left: ... Set-Cookie ... ` panic on
iteration 0) and passes with it — across every iteration, the returned
response never carries `Set-Cookie`. Also asserts (D.2) that
`response.origin == ResponseOrigin::ReconstructedCache` on every
post-initial iteration — proving Scorpion never mislabels this exchange as
a first-hand `Network` observation of the suppressed header.

`vendor/chromey/tests/cache_round_trip.rs` — 4 new tests:
`cache_key_never_exposes_a_raw_credential`,
`disqualifying_secret_header_detection_is_case_insensitive_and_exhaustive`,
`put_hybrid_cache_fails_closed_on_authorization_request_header`,
`put_hybrid_cache_fails_closed_on_set_cookie_response_header` (the last
two are real end-to-end tests against the file's own mock remote cache
server, proving nothing lands locally *or* remotely). The pre-existing
`test_dump_with_auth` — which asserted the raw credential *did* appear in
the uploaded `resource_key` — has been corrected to assert the opposite
(the now-true, safe invariant) plus determinism/distinctness checks; this
was the one existing test that encoded the vulnerable behavior as
"expected," so it is the one test this frontier's fix required changing.

`spider/src/configuration.rs` — new `get_cache_options_tests` module (6
tests) covering the `Authorization` header, `cookie_str`, and explicit
`Cookie` header classification paths, and the both-present-at-once case.

`spider/src/utils/mod.rs` — new `test_contains_disqualifying_secret_header`
and `test_cache_enabled_excludes_authorized`.

**Also fixed, in scope**: `vendor/chromey/tests/cache_round_trip.rs` had
3 pre-existing call sites (unrelated to this frontier — a stale
`put_hybrid_cache` positional-argument count from before this frontier's
own `dump_readonly: bool` parameter was added) that failed to compile at
all; fixed as a 1-line addition per site since this test file directly
exercises the functions this frontier changes and is required for real
regression coverage of the fix (matches this repo's established
"trivial, safe, directly enables verifying this frontier's own change"
carve-out for pre-existing breakage). `vendor/chromey/src/dns.rs` picked
up incidental `cargo fmt` reformatting (whole-crate `cargo fmt`, not a
targeted edit) — no logic change.

## Test results

- `vendor/chromey` (`--features "bytes,stream,cache"`): `--lib` 127/127;
  `cache_round_trip` 14/14; `remote_cache_e2e` 6/6;
  `session_cache_deadlock` 5/5; benches compile clean. Unaffected by the
  `cache_request.rs` fix (re-confirmed unchanged).
- `spider --lib cache_request` (`chrome cache cache_request`): 7/7 (3
  pre-existing + 4 new).
- `spider --test architecture_guardrails` (with and without
  `chrome_remote_cache`): 222/222 (220 + 2 new for the `cache_request.rs`
  fix).
- `spider --lib` (`chrome_remote_cache`): with `RUST_MIN_STACK=64MiB`, the
  full suite, including every `website::*` test, passes: **850/850**
  (846 + 4 new `cache_request` tests).
- `spider --lib` (default features): 755/755.
- `spider_mcp --lib`: 138 passed / 7 ignored (pre-existing convention) /
  0 failed. `spider_cli`: 25/25 (its own binary+integration suites).
- `cargo check --workspace`: clean.
- `cargo fmt --check`: clean (`spider` workspace).
- `cargo clippy --lib --tests -D warnings` (`chrome cache cache_request`
  and default features): clean for `spider/src/cache_request.rs` and every
  other file this fix touched — confirmed against an identical
  pre-existing, unrelated baseline error set via `git stash`
  (`source_provider.rs` constant-assertion lints, `page.rs`
  constant-assertion lints, and clippy-vs-test-compile issues in
  `transport_leaf_acceptance.rs` / `canonical_captcha_image_grid_input_acceptance.rs`
  — none in any file this fix changed).
- `git diff --check`: clean.

## G. Changed files

- `spider/src/configuration.rs` — `get_cache_options()` cookie-jar
  participation; 6 new tests.
- `spider/src/utils/mod.rs` — `cache_enabled()` fix; new
  `contains_disqualifying_secret_header`; defense-in-depth calls at 3
  write sites; doc-comment correction on spider's own
  `create_cache_key_raw`; 2 new tests.
- `spider/src/cache_request.rs` — new `fail_closed_on_set_cookie`, routed
  through `Middleware::policy`/`policy_with_options`; `Set-Cookie`
  stripped from 304 responses in `materialize_network_response`; 4 new
  tests; new `fixture_with_response`/`fixture_sequenced` test helpers.
- `spider/tests/architecture_guardrails.rs` — 8 new guardrails.
- `vendor/chromey/src/cache/manager.rs` — `create_cache_key_raw` now
  hashes; new `contains_disqualifying_secret_header`; calls at
  `handle_single_response`/`put_hybrid_cache`.
- `vendor/chromey/tests/cache_round_trip.rs` — corrected
  `test_dump_with_auth`; corrected a stale comment; fixed 3 pre-existing
  broken call sites; 4 new tests.
- `vendor/chromey/src/dns.rs` — incidental whole-crate `cargo fmt`
  reformatting only.
- `docs/frontier/CANONICAL_CREDENTIAL_CACHE_ISOLATION_SDD.md` — this
  document.
