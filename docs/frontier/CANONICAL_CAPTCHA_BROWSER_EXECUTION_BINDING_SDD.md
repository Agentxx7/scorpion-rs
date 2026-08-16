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
without model cost. A qualification-host test must additionally use the pinned
Qwen installation. Final closure still requires an authorized genuine browser
challenge with observable progression; synthetic fixtures cannot substitute
for that external acceptance gate.

## Frame-aware resumption (still blocked)

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
proves every layer through the exact browser action genuinely correct: a
real out-of-process Turnstile child frame resolves via `FrameContext`; the
widget's real rendered content is captured via `capture_in_frame`; a
hand-computed click dispatched through this identical `apply_in_frame` path
reliably produces Cloudflare's real dummy success token and the widget's
visible "Success!" state. The fully automated run still fails at the last
step — no observable progression — because `qwen3-vl-2b`'s own visual
grounding, at the qualified 320x224 CPU/F32 envelope, does not reliably
locate the checkbox's small (~24x24px) clickable area from real inference
alone (observed real outputs across several honestly-worded instructions:
(8,8), (16,16), (1,1), against a true center near (21,32)). This is a
provider-inference-layer model-precision finding, not a defect in frame
context, snapshot capture, materialization, revalidation, action dispatch or
progression observation — all independently proven correct above. Closing
this for real (not by leaking the answer into the prompt, which would defeat
the point of a genuine acceptance) needs a provider-inference capability
change out of this frontier's scope: a higher-resolution qualified envelope,
a larger/more precise model, or a legitimately larger effective click
target — a decision for a future frontier.

Note: Turnstile's native rendered footprint (~300x65 CSS pixels) does not
smart-resize onto the qualified 320x224 envelope at all (its aspect ratio is
too extreme). The acceptance fixture grows the widget's own owning `<iframe>`
element to exactly 320x224 via CDP (`FrameContext::frame_owner`'s backend
node id — never a selector, which cannot reach it: it sits behind a closed
shadow root) before capturing; Cloudflare's real widget repaints its own real
background/content to fill whatever box its iframe occupies, so the capture
is still entirely genuine rendered content, not synthetic padding.
