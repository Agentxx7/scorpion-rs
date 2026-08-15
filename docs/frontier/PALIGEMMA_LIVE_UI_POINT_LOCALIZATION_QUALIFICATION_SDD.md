# PaliGemma live-UI point localization qualification

Frontier: `SCORPION_PALIGEMMA_LIVE_UI_POINT_LOCALIZATION_QUALIFICATION_001`

Baseline: `079c49c7baeab1a35dd1ec36e80a8d7d89453160`

Blocked dependent frontier: `SCORPION_CANONICAL_CAPTCHA_BROWSER_EXECUTION_BINDING_001`
(classified blocker: `LIVE_WIDGET_POINT_LOCALIZATION_INSUFFICIENT` —
`SCORPION_TURNSTILE_POST_ACTION_PROGRESSION_DIAGNOSTIC_001` proved every
non-model layer correct, including a known-good canonical click producing a
genuine Turnstile token in 0.25s, and isolated the remaining gap to the
model's own live-UI point localization).

## Protocol (frozen before any real inference)

Prior qualification (`paligemma_point_precision_audit.rs`,
`paligemma_cuda_point_precision_audit.rs`) used **solid-fill** squares on
flat backgrounds — 7/7 PASS on both CPU/F32 and CUDA/F16. The real
Turnstile checkbox is a ~22-24px **outlined** (not filled) control embedded
in a realistic branded widget with competing visual elements. This
frontier tests that specific, previously-untested capability gap:
localizing small outlined interactive controls in cluttered UI, using
generic fixtures that do not reproduce Cloudflare branding or the exact
Turnstile layout.

**Canvas**: 224x224 (same fixed qualified PaliGemma envelope as prior
audits — this frontier isolates the outlined/cluttered-UI variable, not
resolution).

**Actionable region** (frozen, not chosen after seeing results): the
control's own true bounding box — `|predicted_x - true_center_x| <=
size/2` and `|predicted_y - true_center_y| <= size/2` — identical
containment criterion to the existing solid-fill audits and to the
production browser-action seam (`predicted point ∈ actual actionable
region`, never a generic distance threshold).

**8 standard fixtures**, each combining one or more of the 10 required
characteristics so all 10 are covered across the set:

| # | Fixture | Size (px) | Position | Characteristics covered |
|---|---|---|---|---|
| 1 | `plain_outlined_square` | 22 | (50,50) | small outlined square (baseline) |
| 2 | `outlined_circle` | 22 | (170,55) | outlined circular control |
| 3 | `adjacent_to_text` | 22 | (55,125) | target adjacent to text-like label |
| 4 | `adjacent_to_logo_distractor` | 22 | (160,165) | target adjacent to abstract logo-like distractor |
| 5 | `multiple_similar_controls` | 22 | (110,190) | multiple non-target controls + visually similar distractors that must not be selected |
| 6 | `low_contrast` | 22 | (140,40) | low-contrast target (border ~20 luma above background) |
| 7 | `near_edge` | 20 | (15,15) | target near image edge |
| 8 | `smallest_stress` | 16 | (95,95) | smallest standard control (catastrophic-miss check) |

Positions are spread asymmetrically across the canvas (not merely corners
or center), covering "asymmetric target locations." Sizes span
16-22px, covering "varying target sizes around the live-control scale"
(the real measured Turnstile checkbox: border-to-border ~23-24px,
interior ~19-20px — see
`docs/frontier/PALIGEMMA_LOCAL_INFERENCE_LATENCY_QUALIFICATION_SDD.md`'s
sibling diagnostic frontier).

**Prompt contract**: existing canonical `PointSelection`
(`CaptchaChallenge.instruction`), one short visual description per fixture
(e.g. "the outlined square", "the outlined square with the blue border").
No coordinates, no bounding box, no DOM information, no Turnstile/Cloudflare/
checkbox/"verify human" wording anywhere in any fixture or instruction.

**Frozen pass threshold** (declared before running real inference):
`>= 7/8` of the 8 standard fixtures must land inside their actionable
region, **and** fixture 8 (`smallest_stress`, the smallest standard
control) must not catastrophically miss — frozen catastrophic-miss bound:
Euclidean error `<= 100px` (well under half the 224x224 canvas's ~317px
diagonal), regardless of whether fixture 8 counts as the one permitted
failure. This threshold is not loosened after seeing results.

**Anti-degeneracy** (same battery as the existing solid-fill audits):
non-constant output across fixtures, `x != y` for at least one fixture,
left-side fixtures average a lower predicted x than right-side fixtures,
top-side fixtures average a lower predicted y than bottom-side fixtures.

**Determinism**: one fixture repeated; greedy decode must reproduce the
identical point.

**Runtime**: `paligemma-local`, CUDA/F16 only (`initialize_cuda_f16_from_host`),
the already-qualified backend from
`SCORPION_PALIGEMMA_LOCAL_INFERENCE_LATENCY_QUALIFICATION_001`. No CPU
fallback, no model/runtime/provider changes.

## Measured per fixture

Predicted x/y, ground-truth true center, actionable region, Euclidean
pixel error, normalized error (error / canvas diagonal), containment
PASS/FAIL, inference latency (`PaligemmaDetectionBox.elapsed`).

## Instrumentation hardening (required before the next real Turnstile
acceptance, not part of this frontier's own file set)

`SCORPION_TURNSTILE_POST_ACTION_PROGRESSION_DIAGNOSTIC_001` found the
prior real-acceptance test (`captcha_browser_turnstile_real.rs`, currently
held in the still-open dependent frontier's preserved stash — not part of
this frontier's clean baseline) logged zero coordinates or timestamps,
making its one real failure permanently unclassifiable down to the exact
model point. Before the next real Turnstile acceptance run (the first
action when `SCORPION_CANONICAL_CAPTCHA_BROWSER_EXECUTION_BINDING_001`
resumes), that test must additionally log, via `eprintln!` only
(diagnostic/provenance-safe, zero effect on solving behavior, no
secrets/tokens):

- the model's image-space point (`CaptchaSolution::Point { x, y }` from
  the provider's real outcome, or the raw detection box)
- the transformed browser-space point actually dispatched
- captured image dimensions (`captured_pixel_width`/`captured_pixel_height`)
- target/frame geometry (`snapshot.transform`, `frame.frame_owner`)
- timestamps for: capture, inference completion, revalidation, click
  dispatch, progression-poll start/end

This closes the exact gap the diagnostic frontier hit; it does not touch
solving/materialization logic and is not itself a browser/frame/model
architecture change.

## Result

**Decision A — PASS.** Real CUDA/F16 inference, `spider/tests/paligemma_live_ui_localization_qualification.rs`:

| Fixture | True center | Predicted | Error (px) | Contained | Latency (s) |
|---|---|---|---|---|---|
| `plain_outlined_square` | (50.0,50.0) | (50.6,50.3) | 0.7 | true | 1.651 |
| `outlined_circle` | (170.0,55.0) | (169.9,55.8) | 0.8 | true | 1.070 |
| `adjacent_to_text` | (55.0,125.0) | (55.5,124.0) | 1.1 | true | 1.068 |
| `adjacent_to_logo_distractor` | (160.0,165.0) | (158.8,163.5) | 1.9 | true | 1.067 |
| `multiple_similar_controls` | (110.0,190.0) | (109.7,187.2) | 2.8 | true | 1.065 |
| `low_contrast` | (140.0,40.0) | (139.7,39.5) | 0.6 | true | 1.068 |
| `near_edge` | (15.0,15.0) | (14.3,15.5) | 0.9 | true | 1.063 |
| `smallest_stress` (16px) | (95.0,95.0) | (95.3,94.7) | 0.4 | true | 1.065 |

**Containment: 8/8**, clearing the frozen `>= 7/8` threshold with zero
misses. Mean error ~1.15px, max error 2.8px (`multiple_similar_controls`,
still well inside its 22px actionable region). Smallest-control
catastrophic-miss check: 0.4px, far under the frozen 100px bound.
Determinism (repeated `plain_outlined_square`) and all anti-degeneracy
checks (position dependence, non-constant output, x≠y) passed.

Outlined-vs-filled rendering and cluttered-UI visual complexity
(text/logo/multi-control distractors, low contrast, edge proximity, 16-26px
size range) were **not** the source of the real Turnstile run's failure —
PaliGemma localizes all of these synthetic variations with sub-3px
precision. The real widget's failure most plausibly involves real-world
rendering detail these synthetic fixtures don't reproduce (genuine
anti-aliasing, actual Cloudflare branding/font rendering, or a
transient/non-representative issue in that one unlogged run) — not
investigated further here, per this frontier's explicit scope boundary.
The next genuine Turnstile acceptance, run with the hardened
instrumentation below, will show directly rather than requiring further
inference by elimination.
