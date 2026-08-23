# Canonical CAPTCHA browser execution binding

Frontier: `SCORPION_CANONICAL_CAPTCHA_BROWSER_EXECUTION_BINDING_001`

Baseline: `5a4d7912f82c1bb582228fa1397aeb610be734dc`

The binding composes one immutable `BrowserChallengeSnapshot`, one normalized
`CaptchaSolveRequest`, one explicitly selected provider attempt and the exact
browser action already owned by the snapshot seam. It owns no detection,
provider choice, retry, model execution, output repair, browser lookup or input
dispatch implementation.

## Materialization

The PNG bytes and exact pixel dimensions come only from the snapshot. Point
and horizontal-offset forms use one ordinary materialized visual. Image grids
use `MaterializedFullGrid`; caller-provided stable IDs and row/column positions
must exactly equal the snapshot's bound target set. Cell rectangles are
translated by the browser seam's authoritative inverse transform. Missing,
duplicate, overlapping or non-integral image geometry fails closed.

Horizontal offset binds an explicit retained handle ID. The browser seam,
rather than this binding, derives its image-space center, adds the finite
offset, validates the endpoint and constructs the exact drag. Point solutions
are prevalidated through the same browser transform. Nothing clamps or guesses.

## Routing and execution

`CaptchaBrowserAttempt` names exactly one provider. The existing
`CaptchaRouteAttempts::execute_explicit_attempt` performs capability,
availability and provider dispatch. There is no provider fallback, ranking,
racing or retry. Browser handles never enter `CaptchaSolveRequest`.

After a solution is produced, every returned grid ID is checked before any
action. The snapshot is explicitly revalidated, and each action is then applied
through `BrowserChallengeSnapshot::apply`, which revalidates immediately again.
Provider failure and revalidation failure therefore dispatch zero actions.
Multi-selection reports partial action count if a later action fails after an
earlier exact click.

## Outcome and ownership

The execution report retains the canonical provider-attempt ledger, the
furthest truthful stage and exact action count. It always records progression
as `NotObservedByBinding`: successful browser input is not a solved-CAPTCHA
claim. Challenge progression observation remains caller-owned.

Typed binding failures distinguish materialization, provider failure, solution
kind, unknown/duplicate identity, empty-selection policy, bounds and the full
canonical browser failure vocabulary. Solver provenance stays in the retained
attempt ledger.

## Closure gates

Controlled local Chromium fixtures prove grid, point and drag composition
without model cost. A qualification-host test must additionally use a real
pinned local-model installation. Final closure still requires an authorized
genuine browser challenge with observable progression; synthetic fixtures
cannot substitute for that external acceptance gate.

## Frame-aware resumption — closed with a genuine PaliGemma Turnstile acceptance

Resumed after `SCORPION_CANONICAL_BROWSER_FRAME_CONTEXT_SNAPSHOT_AND_ACTION_001`
closed. `execute_browser_captcha_attempt_in_frame` is the frame-aware sibling
of `execute_browser_captcha_attempt`: identical contract, composing
`BrowserChallengeSnapshot::revalidate_in_frame`/`apply_in_frame` instead of
the top-level-only pair, for a challenge captured via `capture_in_frame`
inside a genuine child frame. `materialize_request`/`actions_for_solution`
are shared verbatim by both entry points — they only ever read already-
captured snapshot facts, never a live page/frame handle, so a frame-captured
snapshot needed no different treatment there. This is not a redesign: no new
CAPTCHA model, no Turnstile-specific solver logic, no second frame-aware
action stack (proven by
`captcha_browser_binding_composes_frame_aware_seam_without_duplicating_routing`
in `architecture_guardrails.rs`).

A genuine authorized-Cloudflare-Turnstile acceptance
(`captcha_browser_turnstile_real.rs`, using Cloudflare's own documented
"forces an interactive challenge" test sitekey `3x00000000000000000000FF`)
proves every layer through the exact browser action genuinely correct, fully
automated, end to end: a real out-of-process Turnstile child frame resolves
via `FrameContext`; the widget's real rendered content is captured via
`capture_in_frame`; canonical CAPTCHA materialization routes to the explicit
`paligemma-local` provider — the qualified PaliGemma 448 CUDA/F16 runtime
(`initialize_448_cuda_f16_from_host`, closing two real gaps found and fixed
by prior frontiers: CPU/F32 `detect()` at ~426s exceeded a real Turnstile
challenge's own ~110s lifetime, closed by the CUDA/F16 runtime; the 224
checkpoint's real-content X-axis grounding failure, closed by the qualified
448 checkpoint) — produces `CaptchaSolveOutcome::Solved` with model
image-space point `(20.125, 33.25)`, matching the frozen genuine raster
qualification's own measurement almost exactly; the transformed browser
point (1:1 scale, no letterboxing) is dispatched through this identical
`apply_in_frame` path after frame-aware revalidation, reliably producing
Cloudflare's real dummy success token and the widget's visible "Success!"
state. Inference-through-action elapsed 11.012s, comfortably inside the
frozen 55.03s budget. No retry, no model fallback, no CPU fallback, no
coordinate repair, no DOM-assisted localization, no enlarged hitbox, no
JavaScript click, no manual completion, no stale-target substitution. See
`ff4c1291` ("feat: close canonical CAPTCHA browser execution binding with
real Turnstile acceptance").

Note: Turnstile's native rendered footprint (~300x65 CSS pixels) does not
smart-resize onto the qualified 320x224 envelope at all (its aspect ratio is
too extreme). The acceptance fixture grows the widget's own owning `<iframe>`
element to exactly 320x224 via CDP (`FrameContext::frame_owner`'s backend
node id — never a selector, which cannot reach it: it sits behind a closed
shadow root) before capturing; Cloudflare's real widget repaints its own real
background/content to fill whatever box its iframe occupies, so the capture
is still entirely genuine rendered content, not synthetic padding.
