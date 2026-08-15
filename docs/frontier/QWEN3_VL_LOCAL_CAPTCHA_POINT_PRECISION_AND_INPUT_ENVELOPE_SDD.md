# Qwen3-VL local CAPTCHA point precision and input envelope

Frontier: `SCORPION_QWEN3_VL_LOCAL_CAPTCHA_POINT_PRECISION_AND_INPUT_ENVELOPE_001`

Baseline: `e6c135263a1f699694c07bd901f60d8d8fbef4dd`

Prerequisite: `SCORPION_QWEN3_VL_CANDLE_REFERENCE_PARITY_ROOT_CAUSE_001` (closed at the
same SHA — MRoPE and prefill-causal-mask defects fixed).

## Verdict

**`QWEN3_VL_2B_POINT_PRECISION_INSUFFICIENT`.**

The corrected runtime is genuinely non-degenerate — real, position-dependent
point-selection answers, not a fixed or prompt-echoed constant — but its raw
single-shot precision does not reliably land inside an actionable click
target at the currently qualified 320x224 envelope, and a controlled probe
of a larger envelope did not reproduce a material, reliable improvement (see
below). This is now a legitimate capability-ceiling classification, not a
runtime defect: reference/runtime parity was independently proven in the
prerequisite frontier.

## Qualified visual envelope

**Unchanged: 320x224.** Widening was evaluated on real evidence and
rejected — not skipped, not assumed unnecessary.

## Method

Deterministic real CPU/F32 inference (`Qwen3VlCpuRuntime`, greedy decoding)
against synthetic fixtures with known ground truth, through the exact
production `CaptchaProvider::solve` seam
(`spider/tests/qwen3_vl_point_precision_audit.rs`). No coordinate was ever
encoded in an instruction. The required position matrix: four corners,
center, an asymmetric x≠y position, a small isolated target, and a
distractor-containing target — plus a repeated trial to prove deterministic
repeatability.

### Tolerance

"Actionable" is defined as the predicted point landing inside the true
target's own bounding box — identical containment semantics to what the
production browser-action seam already enforces for
`BrowserChallengeAction::ExactTargetClick`
(`browser_challenge.rs::apply`, lines checking
`point.x < target.geometry.x || … `). A numerically small error that still
falls outside the target counts as a failure, per this frontier's own
instruction not to choose a tolerance that flatters the model.

Standard-size fixtures use a 44x44px target: the WCAG 2.5.5 minimum
touch/click target size, fixed before any trial was run — not tuned to the
model's measured error. A `small_isolated` fixture (16px, below the
actionable minimum) probes the precision floor and is reported, not counted
toward the envelope decision.

## Measured precision (320x224, post-fix)

| target | true center | returned | error (px) | normalized | contained |
|---|---|---|---|---|---|
| upper_left | (40, 40) | (32, 32) | 11.3 | 0.029 | yes |
| upper_right | (280, 40) | (288, 184) | 144.2 | 0.369 | no |
| lower_left | (40, 184) | (32, 192) | 11.3 | 0.029 | yes |
| lower_right | (280, 184) | (288, 192) | 11.3 | 0.029 | yes |
| center | (160, 112) | (208, 112) | 48.0 | 0.123 | no |
| asymmetric | (90, 170) | (128, 168) | 38.1 | 0.097 | no |
| small_isolated (16px) | (250, 60) | (288, 128) | 77.9 | 0.199 | no |
| distractor | (100, 112) | (160, 168) | 82.1 | 0.210 | no |

**Standard-size (44px) actionable containment: 3/7 (43%).** Not sufficient
for a reliable single-shot actionable-click claim.

Structured-output validity: 8/8 (9/9 including the determinism repeat).
Determinism: the repeated `center` trial reproduced the identical point
exactly, confirming stable greedy decoding.

### Genuine position dependence (anti-degeneracy)

The important requirement per this frontier is position dependence, not raw
accuracy, and it holds:

- Not a repeated constant answer across materially different targets.
- Not `x == y` regardless of target.
- Left-side targets (`upper_left`, `lower_left`) average a lower predicted
  x (32.0) than right-side targets (`upper_right`, `lower_right`, 288.0) —
  matching the true 40 vs 280 separation almost exactly in magnitude.
- Top-side targets average a lower predicted y (108.0) than bottom-side
  targets (192.0), matching the true 40 vs 184 direction (though with a
  real per-case miss on `upper_right`'s y, which is exactly why 4/7
  standard fixtures fail containment).

This confirms the model is doing real visual work — the prior blocked
frontier's degenerate near-`(1,1)` collapse and this frontier's own earlier
informal 172.7px/267.0px/346.9px-grows-with-resolution pattern are both
gone — but 2B-parameter capacity is not precise enough for reliable
actionable single-shot point clicks at realistic click-target sizes.

## Input envelope decision (branch B evaluated and rejected)

A controlled 640x448 envelope probe was run (temporary local diagnostic
edit to the qualified-envelope gate in `qwen3_vl_runtime.rs`, run, and
fully reverted — never shipped, never committed) to check whether the
prior frontier's now-invalidated "error grows with resolution" finding
still held on the corrected runtime, per this frontier's explicit
instruction not to treat that finding as authoritative anymore.

Findings, 2x-scaled versions of the same relative positions:

- `upper_left` and `center`: exact matches (0px error) — genuinely better
  than at 320x224.
- `asymmetric`: 48.3px error, contained (half-side 44px).
- `upper_right`: **a new failure mode** — for several probed target
  x-positions in the upper few hundred pixels of range (500, 600, 630), the
  model returned exactly `(640, 448)`: the schema's declared upper bound,
  copied verbatim from the prompt's stated `width=640 height=448`,
  regardless of the true target position. This is a textbook degenerate
  collapse, not real localization, and it does not occur at 320x224 where
  legal coordinate values rarely approach the declared bound as closely in
  relative terms.
- For target x-positions in approximately 550-569, structured generation
  failed outright (`NoValidStructuredContinuation`) even at a 64-token
  budget — a real, narrow, reproducible search gap in the constrained
  numeric grammar for that specific magnitude range. Root-caused only
  partially (ruled out token budget; not further pursued) because it is
  unreachable at the 320x224 envelope this frontier ships (max coordinate
  320), and per this frontier's own instruction not to opportunistically
  refactor unrelated Qwen runtime code for a path that is not shipped. Left
  as an explicit, documented open finding for a possible future envelope
  or grammar-hardening frontier — not fixed here.

Net: widening the envelope traded "consistently imprecise but genuinely
position-dependent" for "sometimes better, sometimes a new prompt-echoing
degenerate mode, sometimes an outright generation failure." That is not a
reproducible, material, reliable improvement, so per this frontier's branch
B/C logic the smallest justified action is **not** to widen the envelope,
and to report the honest capability-ceiling verdict instead.

## No production code change to qualification state

`Qwen3VlLocalCaptchaProvider::qualification_state` already returns
`ExecutableUnqualified` uniformly for every supported challenge kind
(`qwen3_vl_captcha.rs`). That is correct and unchanged: this frontier does
not promote `PointSelection` to `EmpiricallyQualified`, and no code change
was needed to keep it that way.

## What changed

- `spider/tests/qwen3_vl_point_precision_audit.rs`: replaced the prior
  informal 4-fixture audit with the full required 8-fixture matrix,
  bounding-box containment tolerance, determinism check, and explicit
  anti-degeneracy assertions — now a real regression gate (still `#[ignore]`,
  matching every other real-model test in this repo, run explicitly via
  `--ignored`). The `horizontal_offset` test gained a real assertion (it
  previously only logged output without checking the outcome at all).
- `docs/frontier/QWEN3_VL_LOCAL_CAPTCHA_POINT_PRECISION_AND_INPUT_ENVELOPE_SDD.md`
  (this document).

No production runtime or provider code changed. The qualified-envelope gate
in `qwen3_vl_runtime.rs` is untouched (the diagnostic widening used to
gather the 640x448 evidence above was never committed).

## Successor boundary

Per this frontier's own chaining instruction
("Resume `SCORPION_QWEN3_VL_LOCAL_CAPTCHA_POINT_PRECISION_AND_INPUT_ENVELOPE_001`.
… If that now passes: resume
`SCORPION_CANONICAL_CAPTCHA_BROWSER_EXECUTION_BINDING_001` … Do not touch
browser/frame architecture.") — the controlled localization qualification
did **not** pass. `SCORPION_CANONICAL_CAPTCHA_BROWSER_EXECUTION_BINDING_001`
therefore remains blocked and untouched; `stash@{1}` is preserved exactly
as-is. No browser/frame architecture, provider routing, or model was
touched in this frontier.
