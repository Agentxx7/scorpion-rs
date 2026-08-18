# Canonical production CAPTCHA execution convergence

Frontier: `SCORPION_CANONICAL_PRODUCTION_CAPTCHA_EXECUTION_CONVERGENCE_001`

Baseline: `539cd279`

Status: **PARTIALLY CONVERGED — dispatch-layer convergence and one
truthfulness fix landed; full browser-binding convergence (production
image capture/action-application through `BrowserChallengeSnapshot`/
`execute_browser_captcha_attempt`) is DEFERRED/BLOCKED pending live
challenge-page validation.** This frontier's original acceptance
criteria (route production execution through the canonical provider
binding + execution seam) are **not** fully met, and this document does
not claim they are.

## A. Previous production path

The live production CAPTCHA path is `spider/src/utils/mod.rs`'s solver
gate (under `real_browser`), which detects a vendor
(`detect_cf_turnstyle`/`looks_like_imperva_verify`/`detect_recaptcha`/
`detect_geetest`/`detect_lemin`) and dispatches to one of five
`spider/src/features/solvers.rs` handlers: `cf_handle`, `imperva_handle`,
`recaptcha_handle`, `geetest_handle`, `lemin_handle`. None of these five
ever called `spider/src/features/captcha_browser.rs`'s canonical
`execute_browser_captcha_attempt`/`execute_browser_captcha_attempt_in_frame`
— confirmed by grep: those two functions had zero callers anywhere in
`spider/src` before this frontier, and still do (that binding remains
unwired from production; see "Not implemented here").

Deeper audit corrected an initial premise: the five handlers are **not**
uniform. `cf_handle` and `imperva_handle` invoke **zero** CAPTCHA
providers — every branch (Turnstile widget click-and-poll, Imperva
slider drag-to-edge, hCaptcha checkbox click) is DOM automation with no
visual-challenge reasoning, so the canonical `CaptchaProvider`/
`CaptchaProviderRegistry` seam does not apply to them at all.
`recaptcha_handle`/`geetest_handle`/`lemin_handle` **already** dispatch
real visual-challenge solving through the canonical registry —
confirmed by the pre-existing, already-passing acceptance test
`all_three_legacy_routes_use_explicit_attempts`
(`spider/tests/canonical_captcha_provider_routing_acceptance.rs`) — via
`solve_enterprise_with_browser_gemini`, `solve_horizontal_offset_with_legacy_routing`,
and `solve_point_with_legacy_routing`, each of which constructs a real
`CaptchaProviderRegistry`, registers `LocalLanguageModelProvider`
(falling back to `ExternalGeminiProvider` when the local path is
unavailable and `GEMINI_API_KEY` is set), and dispatches via
`CaptchaRouteAttempts::execute_explicit_attempt` — the same capability-
prevalidated canonical dispatch primitive `captcha_browser.rs` itself
composes. What none of the three do is apply the resulting solution
through `BrowserChallengeSnapshot`'s captured-target/revalidate/apply
machinery — the puzzle image is captured via an in-page `<canvas>` draw
(not `BrowserChallengeSnapshot::capture`'s live `page.screenshot()` off a
bound DOM element), and the solved answer is applied via direct
`click_smooth()`/synthetic-drag JS, not `snapshot.apply`.

One confirmed truthfulness bug found during the audit:
`solve_enterprise_with_browser_gemini` returned `Ok(Vec::new())` — a
fabricated "no tiles need clicking" success — when the local provider was
unavailable and `GEMINI_API_KEY` was unset, instead of a typed failure.

## B. Canonical production path after convergence

Dispatch-layer convergence was already true and remains true, now
guardrailed permanently: `recaptcha_handle`/`geetest_handle`/
`lemin_handle` continue to reach `CaptchaProviderRegistry::new()` +
`CaptchaRouteAttempts::new()`/`.execute_explicit_attempt(...)` — the
canonical, capability-prevalidated dispatch seam — for every real
visual-challenge solve in production. The confirmed fake-success bug is
fixed: `solve_enterprise_with_browser_gemini`'s provider-unavailable
branch now returns `Err(CdpError::msg("recaptcha enterprise grid: local
CAPTCHA provider unavailable and GEMINI_API_KEY not set"))` instead of
`Ok(Vec::new())`. This changes no externally observable crawl behavior
(`recaptcha_handle`'s outer `match overall { Ok(_) => Ok(validated), ...
}` already discarded the inner closure's `Result` and returned
`Ok(validated)` — `validated` stays `false` either way, since nothing
about the challenge state changes when zero tiles are clicked) — it only
removes the fabricated “definite empty answer” claim, so the route-
attempt ledger and any future caller inspecting `CaptchaSolveOutcome`
can no longer mistake "no provider ran" for "a provider looked and found
nothing."

`cf_handle`/`imperva_handle` are unchanged in behavior and now carry an
explicit `LEGACY_DOM_HEURISTIC` classification doc comment stating they
invoke no CAPTCHA provider. `recaptcha_handle`/`geetest_handle`/
`lemin_handle` now carry an explicit
`CANONICAL_PROVIDER_DISPATCH_LEGACY_BINDING` classification doc comment
naming exactly which dispatch function is canonical and exactly what
remains bespoke around it.

## C. Legacy paths retained/removed

Nothing was removed. Every legacy path is explicitly classified in place
(rule #5's "must be explicitly classified"):

| Path | Classification | Provider involvement |
|---|---|---|
| `cf_handle` | `LEGACY_DOM_HEURISTIC` | None |
| `imperva_handle` | `LEGACY_DOM_HEURISTIC` | None |
| `recaptcha_handle` (+ `solve_enterprise_with_browser_gemini`) | `CANONICAL_PROVIDER_DISPATCH_LEGACY_BINDING` | Canonical registry/route-attempts; bespoke browser binding |
| `geetest_handle` (+ `solve_horizontal_offset_with_legacy_routing`) | `CANONICAL_PROVIDER_DISPATCH_LEGACY_BINDING` | Canonical registry/route-attempts; bespoke browser binding |
| `lemin_handle` (+ `solve_point_with_legacy_routing`) | `CANONICAL_PROVIDER_DISPATCH_LEGACY_BINDING` | Canonical registry/route-attempts; bespoke browser binding |

Rule #5's "must be unreachable from canonical production execution" is
satisfied in the sense that matters here: none of these five legacy
paths are called *by* the canonical seam (`captcha_browser.rs` never
calls into `solvers.rs`), so the canonical seam's own guarantees
(capability prevalidation, no retry cascade, no fabricated success) are
never silently weakened by a legacy fallback underneath it. The reverse
direction — production still calling legacy handlers directly, not the
canonical seam — is the state left DEFERRED, not converged; see below.

## D. Reachability proof

`production_solver_gate_reaches_every_vendor_handler`
(`spider/tests/architecture_guardrails.rs`) proves `spider/src/utils/mod.rs`
still calls all five `solvers::{cf,imperva,recaptcha,geetest,lemin}_handle`
functions — the reachability audit's own anchor, so it goes stale (and
fails) the moment the live wiring changes without this document being
updated. `canonical_captcha_execution_seam_is_not_reimplemented_in_solvers`
proves `execute_browser_captcha_attempt`/`_in_frame` remain defined
exactly once, in `captcha_browser.rs`, and that `solvers.rs` never grows
its own `BrowserChallengeSnapshot::capture`/struct-literal/import —
i.e., the canonical seam is real, present, and not shadowed, even though
production does not yet call it.

## Why full browser-binding convergence is deferred

Converting `recaptcha_handle`/`geetest_handle`/`lemin_handle` to call
`execute_browser_captcha_attempt` requires replacing their image-capture
mechanism (`<canvas>`-draw dataURL → `BrowserChallengeSnapshot::capture`'s
live `page.screenshot()` off a bound, nonce-verified DOM element) and,
for `geetest_handle` specifically, binding the actual slider-handle
element as an explicit `targets` entry (`HorizontalOffset` requires
`handle_target_id`; `recaptcha_handle` would need every tile element
bound similarly for `ImageGridSelection`). This is a genuine change to
*which pixels a real solve is run against* and *how the resulting
action is dispatched to the live page* for three anti-bot bypass paths
actually used in production crawls. This environment has no live
Cloudflare/reCAPTCHA/GeeTest/Lemin challenge page to validate against
(no network egress to real challenge infrastructure, and the qualified
local vision-model runtime the prior `SCORPION_CANONICAL_CAPTCHA_BROWSER_EXECUTION_BINDING_001`
frontier used for its own real Turnstile validation requires a
CUDA-capable GPU not available here). Shipping this rewrite unvalidated
would risk exactly what rule #6 forbids — silently degrading working
production CAPTCHA bypass — so it is left as future work requiring a
session with live challenge-page and/or qualified-GPU access, per the
explicit corrected scope decision made mid-frontier.

## Not implemented here

Per the corrected, explicitly agreed scope: no rewrite of
`recaptcha_handle`/`geetest_handle`/`lemin_handle`'s image-capture or
action-application mechanics; no wiring of `cf_handle`/`imperva_handle`
onto the canonical seam (they are not provider-solving tasks); no new
CAPTCHA providers; no transport/local-model architecture changes; no
Watch/state/persistence changes; no global `solvers.rs` redesign.

## E. Guardrails/tests

`spider/tests/architecture_guardrails.rs` — 7 new guardrails:
`production_solver_gate_reaches_every_vendor_handler` (reachability
proof), `dom_heuristic_handlers_are_explicitly_classified_and_invoke_no_provider`,
`provider_dispatch_handlers_are_explicitly_classified_and_reuse_canonical_dispatch`
(also proves each dispatch function genuinely constructs
`CaptchaProviderRegistry`/`CaptchaRouteAttempts` and calls
`execute_explicit_attempt`), `provider_unavailable_never_becomes_a_fabricated_empty_success`
(proves the fixed bug's exact pattern never reappears, and that the fix
is present), `canonical_captcha_execution_seam_is_not_reimplemented_in_solvers`,
`no_shadow_captcha_dispatch_in_cli_or_mcp`.

207/207 architecture guardrails pass (with `chrome real_browser gemini`
and with the default feature set). All existing CAPTCHA acceptance
tests pass unmodified and unregressed: `canonical_captcha_corpus_protocol_acceptance`
(8), `canonical_captcha_provider_routing_acceptance` (8, including
`all_three_legacy_routes_use_explicit_attempts`),
`canonical_captcha_solver_capability_acceptance` (9),
`captcha_browser_binding_acceptance` (8),
`openai_vision_captcha_provider_acceptance` (8) — 41 tests, all green.
`solvers.rs`'s own inline unit tests (10) pass. Full default `cargo test
-p spider --lib` (755/755) passes. `cargo check --workspace` clean;
`cargo fmt --check` clean; `cargo clippy --lib --tests --features
"chrome real_browser gemini" -D warnings` produces only pre-existing,
unrelated baseline errors (confirmed identical via `git stash` against
this same feature set — none in `solvers.rs` or
`architecture_guardrails.rs`); `git diff --check` clean. The full
`--features "chrome real_browser gemini" --lib` regression run was not
completed in this session — it requires a real Chrome binary/browser
automation environment this sandbox does not reliably provide within a
bounded timeout (consistent with this repo's own noted Chromium E2E
environment quirks); the narrower, targeted evidence above (solver-
specific unit tests, all CAPTCHA acceptance suites, clippy across the
same feature set) is the regression evidence actually gathered for the
changed surface.

## F. Changed files

- `spider/src/features/solvers.rs` — one behavioral fix
  (`solve_enterprise_with_browser_gemini`'s provider-unavailable branch),
  five classification doc comments (`cf_handle`, `imperva_handle`,
  `recaptcha_handle`, `geetest_handle`, `lemin_handle`). No other logic
  changed.
- `spider/tests/architecture_guardrails.rs` — 7 new guardrails.
- `docs/frontier/CANONICAL_PRODUCTION_CAPTCHA_EXECUTION_CONVERGENCE_SDD.md`
  — this document.

## Successor boundary

A future, separately-scoped frontier — with either live challenge-page
network access or a qualified local GPU vision-model runtime available
for validation — should migrate `recaptcha_handle`/`geetest_handle`/
`lemin_handle`'s image capture and action application onto
`BrowserChallengeSnapshot`/`execute_browser_captcha_attempt`, the way
`SCORPION_CANONICAL_CAPTCHA_BROWSER_EXECUTION_BINDING_001` already
validated for Turnstile's interactive PointSelection case. Until then,
production CAPTCHA execution for these three vendors remains a
canonical-dispatch-plus-bespoke-binding hybrid, truthfully documented
and guardrailed as such rather than silently presented as fully
converged.
