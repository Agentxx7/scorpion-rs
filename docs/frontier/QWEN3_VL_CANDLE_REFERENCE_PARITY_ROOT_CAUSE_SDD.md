# Qwen3-VL Candle reference-parity root cause

Frontier: `SCORPION_QWEN3_VL_CANDLE_REFERENCE_PARITY_ROOT_CAUSE_001`

Baseline: `2e27e460de20a1abc64b9b2c8b0ea009f6920191`

## Verdict

Two independent, compounding Candle wiring defects — not a model-capability
ceiling. Both are fixed. `real_offline_generation_unload_and_reinitialize`
now reproduces the pinned Hugging Face `transformers` 5.15.0 / PyTorch CPU
reference oracle's exact greedy output ("Gradient.") for the identical
weights, config, tokenizer, image and prompt.

## Diagnostic method

A pinned `transformers`/`torch` CPU reference oracle was built in an
isolated `uv` virtualenv, loading the exact same local model files
`spider`'s own runtime uses (`/tmp/scorpion-qwen3vl-qualification`), never
becoming a Scorpion runtime/production dependency (offline diagnostic
material only, matching the frontier's REFERENCE POLICY). The controlled
diagnostic cases (Case A–D from the frontier spec) established, cheaply and
early:

- Preprocessing (Phase 1) matched: `image_grid_thw`, `pixel_values` shape,
  and per-pixel min/max/mean/std matched Python to ~4 decimal places for
  both the historical gradient fixture and a 320x224 CAPTCHA-style square
  fixture.
- The pinned model, run through the real reference implementation, is
  genuinely capable: `"Gradient."` for the gradient fixture, and a correct
  "upper-left corner" spatial description for an asymmetric square fixture
  — ruling out `REFERENCE_PARITY_CONFIRMED_MODEL_CAPABILITY_INSUFFICIENT`
  immediately.
- The Rust runtime, same inputs, produced incoherent output
  (`"### 1, 2, "`, `")ui, 1, 1, 1, ..."`) even for `generate()`'s completely
  unconstrained path — ruling out the constrained-decoding grammar as the
  root cause (a grammar bug could bias which *valid* token wins; it cannot
  explain garbage with no schema active at all).
- Vision-tower checkpoint statistics (patch embed, first/final block output,
  merger, deepstack features), captured via PyTorch forward hooks and a
  matching `QWEN3_VL_AUDIT_DUMP`-gated Rust dump, matched closely (Phase 2
  confirmed correct) — narrowing the search to positional encoding and the
  language-model forward pass (Phases 3, 5, 7).

## Defect 1: `MROPE_POSITIONAL_PARITY_FAILURE`

`candle-transformers` 0.11.0's `Qwen3VLTextModel`/`RotaryEmbedding` had no
`rope_scaling` field on `TextConfig` at all — `serde`'s default
unknown-field tolerance silently dropped the pinned config's
`mrope_section: [24, 20, 20]` — and applied a single flat sequential 1D
position (`Tensor::arange(0, max_position_embeddings)`, narrowed per call)
identically to text and image-patch tokens alike. The reference model's
real positional scheme is a genuine interleaved 3-axis
(temporal/height/width) MRoPE: image patches get spatially-meaningful 2D
`[t, h, w]` positions (via `get_rope_index`/`get_vision_position_ids`), and
the per-frequency-index selection between the three axes is *interleaved*
(index ≡ 0 mod 3 → temporal, ≡ 1 mod 3 → height, ≡ 2 mod 3 → width, each up
to its `mrope_section` count), not chunked or absent.

Fixed in the vendored fork (`vendor/candle-transformers-qwen3vl-fix`):
- `config.rs`: `RopeScaling { mrope_section, mrope_interleaved }`, `#[serde(default)]` on `TextConfig` (backward compatible with configs lacking it).
- `text.rs`: `RotaryEmbedding` now stores `inv_freq`/`mrope_section` and computes cos/sin per call from caller-supplied `[t, h, w]` position triples, mirroring the reference's `apply_interleaved_mrope` exactly (verified numerically, see below).
- `mod.rs`: new `compute_mrope_position_ids`, mirroring the reference `Qwen3VLModel.get_rope_index` for this runtime's exact usage (batch size 1, at most one image, no video).

Verified independently: both the computed `[t, h, w]` position ids and the
resulting per-index rotary frequencies matched the reference oracle's own
`get_rope_index`/`rotary_emb.forward` output exactly, position-by-position,
for the 87-token gradient-fixture prompt (image span, `mrope_delta`, and
every value up to floating-point tolerance).

## Defect 2: `KV_CACHE_GENERATION_PARITY_FAILURE`

`Qwen3VLModel::forward`'s causal-attention-mask condition was inverted:
`if seqlen <= 1 { Some(mask) } else { None }`. The multi-token prefill call
(processing the entire prompt — text and image patches — in one forward
pass) got **no** causal mask at all, so every position attended
bidirectionally across the whole prompt, including tokens that come later.
The single-token incremental-decode case (which needs no explicit mask —
the KV cache already contains only past tokens) got a mask instead, which
is harmless but pointless. Fixed by flipping the condition (`if seqlen >
1`).

Both defects were necessary and neither was sufficient alone: fixing only
the MRoPE defect still produced incoherent output (`"###UI\n###UI\n###UI"`);
fixing both together reproduced the reference's exact "Gradient." output.

## Regression test

`real_offline_generation_unload_and_reinitialize` (previously asserted only
non-empty output — a genuinely broken model satisfies that trivially, and
did, silently, since this repo's own `QWEN3_VL_CPU_PRODUCTION_RUNTIME_001`
frontier closed) now asserts:
- the gradient fixture's description is semantically correct (contains
  "gradient", matching the reference oracle's own greedy output),
- a materially different (flat-color) image produces a *different*
  description — proving content-dependence, not a fixed answer,
- repeated isolated sessions with identical input produce identical output
  (deterministic greedy decoding).

## A third, unrelated defect surfaced and fixed in passing

Restoring genuine coherence exposed — it could not have been caught before,
since a sufficiently broken model makes *any* grammar-valid token equally
reachable — a real gap in the structured-generation search
(`spider`'s own `qwen3_vl_runtime.rs`, not `candle-transformers`): for a
`StringIdArray` schema with `allow_empty: false`, the per-token constrained
search failed to find a valid continuation for the array's opening quote
character when nothing had been generated yet, even though `id_array_state`
correctly accepts it once present. Since that first quote is deterministically
forced by the grammar regardless of what the model would otherwise choose,
pre-committing it into the `assistant_prefill` (rather than requiring the
search to rediscover it) is a safe, minimal fix — applied in both
`qwen3_vl_captcha.rs`'s production `ImageGridSelection` request builder and
this frontier's own structured-generation test. This is a narrow,
characterized workaround for a real search gap, not a redesign of the
grammar engine; root-causing the search itself is out of this frontier's
scope.

## Semantic re-qualification evidence

Controlled point-selection and horizontal-offset trials (synthetic
fixtures, known ground truth, no coordinates ever leaked into a prompt, run
through the exact production `CaptchaProvider::solve` seam) at the existing
qualified 320x224 envelope:

- Before this frontier's fixes: 9/12 point-selection trials returned the
  same degenerate `(1, 1)`-ish answer regardless of true target position;
  mean absolute error grew *with* image resolution (172.7px → 267.0px →
  346.9px across three envelope sizes) — the signature of a fixed wrong
  answer, not a precision ceiling.
- After: real, differentiated, position-dependent answers — one exact
  match (0px error), Y-axis exact in three of four trials, mean absolute
  error 62.0px (down from 172.7px at the identical envelope) — and a real
  horizontal-offset trial returned 160px against a true 140px displacement.
  Residual imprecision is consistent with a genuinely small (2B-parameter)
  vision-language model doing real work, not degenerate collapse.

Confirms the prior blocked frontier's own finding
(`SCORPION_QWEN3_VL_LOCAL_CAPTCHA_POINT_PRECISION_AND_INPUT_ENVELOPE_001`)
was correct that resolution scaling would not help, and correctly
classified the underlying problem as deeper than "precision" — it was two
Candle correctness defects.

## Vendoring decision

`candle-transformers` is patched via a vendored fork
(`vendor/candle-transformers-qwen3vl-fix/`, `[patch.crates-io]` in the root
`Cargo.toml`), mirroring this repo's existing `vendor/chromey` pattern for
exactly this situation (a third-party crate needing a genuine correctness
patch). The diff is confined to `src/models/qwen3_vl/{mod,text,config}.rs`;
no other model or shared utility code is touched.

## Successor boundary

Out of scope, deliberately, and untouched: browser/frame architecture, the
blocked CAPTCHA browser binding frontier's own code, provider fallback,
Gemini, model replacement, Turnstile-specific behavior, and deep root-
causing of the structured-generation search gap described above (fixed only
at its single, narrow, already-identified call site). Per this frontier's
own AFTER SUCCESSFUL BUG FIX instructions: next, re-run
`SCORPION_QWEN3_VL_LOCAL_CAPTCHA_POINT_PRECISION_AND_INPUT_ENVELOPE_001`'s
controlled precision qualification (informally already re-run above, with
materially improved results) and, if accepted, resume
`SCORPION_CANONICAL_CAPTCHA_BROWSER_EXECUTION_BINDING_001` from `stash@{1}`
for a genuine Turnstile acceptance rerun — not attempted in this frontier,
per its explicit "Do NOT use Turnstile to debug the implementation."
