# candle-transformers (Qwen3-VL + PaliGemma correctness patches)

Patched fork of `candle-transformers` 0.11.0, scoped to
`src/models/qwen3_vl/`, `src/models/gemma.rs` and `src/models/paligemma.rs`,
for `SCORPION_QWEN3_VL_CANDLE_REFERENCE_PARITY_ROOT_CAUSE_001` and
`SCORPION_LOCAL_CROSS_MODEL_VISION_CAPTCHA_PROVIDER_QUALIFICATION_001`.

## What's patched and why

Root-caused against a pinned Hugging Face `transformers` 5.15.0 / PyTorch CPU
reference oracle, running the identical model weights, config, tokenizer and
prompt as `spider`'s own `qwen3_vl_runtime.rs` (see
`docs/frontier/QWEN3_VL_CANDLE_REFERENCE_PARITY_ROOT_CAUSE_SDD.md` for the
full diagnostic chain). Two independent, compounding defects were found and
fixed here:

1. **`MROPE_POSITIONAL_PARITY_FAILURE`** — Qwen3-VL's `rope_scaling`
   (`mrope_section: [24, 20, 20]`) config key was not even present on
   `TextConfig`, so it was silently dropped by `serde`. `RotaryEmbedding`
   applied plain sequential 1D RoPE identically to every token — including
   every image patch — instead of the model's real interleaved 3-axis
   (temporal/height/width) positional scheme, destroying all spatial
   position information the vision-language model needs to reason about
   *where* something is in an image. Fixed in `text.rs`
   (`RotaryEmbedding::cos_sin`, mirroring the reference's
   `apply_interleaved_mrope`) and `config.rs` (`RopeScaling`); the actual
   `[t, h, w]` position ids are computed by the new `compute_mrope_position_ids`
   in `mod.rs`, mirroring the reference `Qwen3VLModel.get_rope_index`.
2. **`KV_CACHE_GENERATION_PARITY_FAILURE`** — `Qwen3VLModel::forward`'s
   causal-attention-mask condition was inverted: `if seqlen <= 1 { Some(mask)
   } else { None }` meant the multi-token prefill pass (the entire prompt,
   processed in one forward call) got **no** causal mask at all, letting
   every position attend bidirectionally across the whole prompt including
   tokens that come later — while the single-token incremental-decode case
   (which needs no explicit mask; the KV cache already contains only past
   tokens) got a mask instead. Fixed by flipping the condition (`if seqlen >
   1`) in `mod.rs`.

Both were verified against the reference oracle (position ids, per-index
rotary frequencies, and vision-tower checkpoint statistics all match to
float32 tolerance) and both were necessary: fixing only one still produced
incoherent generation. With both fixed, real inference reproduces the
reference's exact greedy output for the frontier's controlled fixtures.

## PaliGemma / Gemma-1 patches

Root-caused against the same kind of pinned Hugging Face `transformers`
reference oracle (running the identical `google/paligemma-3b-mix-224`
weights/config/tokenizer/image/prompt as `spider`'s own
`paligemma_runtime.rs`; see
`docs/frontier/LOCAL_CROSS_MODEL_VISION_CAPTCHA_PROVIDER_QUALIFICATION_SDD.md`).
Three independent defects were found and fixed, confirmed by an exact
bit-for-bit match of real greedy `detect` output against the reference
oracle once all three were fixed together (none was sufficient alone):

1. **Spurious image-embedding L2 normalization** — `paligemma::Model::setup`
   (and `setup_without_projection`) called `clip::div_l2_norm` on the vision
   tower + projector's output before merging it with text embeddings. The
   real `PaliGemmaModel.get_image_features` (`modeling_paligemma.py`) uses
   the projector's raw `nn.Linear` output directly via `masked_scatter` —
   there is no normalization step anywhere in the real architecture.
   Rescaling every image embedding to unit length before it ever reached the
   language model is not what the pinned weights were trained to expect.
   Fixed by removing the call in `paligemma.rs`.
2. **Missing/incorrect text-only embedding scale on the multimodal
   prefill** — the real Gemma-1 architecture's `sqrt(hidden_size)` embedding
   scale lives inside `GemmaTextScaledWordEmbedding` itself (applied only
   when computing embeddings fresh from `input_ids`), and PaliGemma's own
   image features — which never pass through that embedding layer — are
   never scaled. `gemma::Model::forward_embeds`/
   `forward_embeds_without_projection` previously applied
   `xs * sqrt(hidden_size)` unconditionally to whatever the caller passed
   in; for PaliGemma's multimodal caller this incorrectly inflated the
   *image* portion of the merged embedding by the same ~45x factor
   (`sqrt(2048)`), corrupting every downstream position. Fixed by removing
   the scale from both `forward_embeds` methods (they now trust the caller's
   embeddings as-is, matching the reference's real
   `GemmaModel.forward(inputs_embeds=...)` behavior of never re-scaling) and
   adding a new `gemma::Model::embed_scale()` accessor that `paligemma.rs`
   calls explicitly on exactly the text-embedding span, before merging with
   the untouched image features.
3. **Duplicated image-placeholder span passed to the multimodal prefill** —
   `paligemma_runtime.rs`'s own call site (not this vendored fork) initially
   passed the *entire* rendered prompt — 256 `<image>` placeholder tokens
   plus the real text tokens — as `Model::setup`'s `input_ids`, on top of
   the real 256-position vision-tower embeddings `setup` already
   concatenates internally. `Model::setup` expects `input_ids` to be the
   text-only suffix; passing the placeholders too doubled the image span
   into a 517-position sequence instead of the reference's real 261,
   corrupting every downstream position. This is a `spider`-side call-site
   fix, not a change to this vendored fork, documented here because it was
   discovered and root-caused alongside the two fixes above during the same
   diagnostic pass.

A fourth apparent fix was attempted and **reverted**: causally masking
`forward_embeds`'s multi-position prefill call, by direct (and, on
inspection, incorrect) analogy with the unrelated Qwen3-VL prefill-mask
defect above. PaliGemma's real architecture is not a plain causal
decoder-only prefill: `modeling_paligemma.py` explicitly gives the *entire*
image+prompt prefix full bidirectional attention (driven by
`token_type_ids`/`block_sequence_ids` — "can attend bidirectionally in
prefix and only causally in suffix"), and only tokens *generated after* the
prefill are causal. Adding a causal mask to the prefill call was itself a
regression, confirmed empirically via per-layer hidden-state checkpoint
comparison against the reference oracle: statistics only matched exactly
with bidirectional prefill attention restored (the original, unmasked
`forward_embeds` behavior), and compounded to a ~2x final-layer divergence
under the incorrectly-added mask. `forward_embeds`/
`forward_embeds_without_projection` build no mask of their own; the caller
is responsible for supplying whatever masking its own architecture
requires, same as the embedding-scale fix above.

## Provenance

Base: `candle-transformers` 0.11.0 from crates.io (unpacked registry
source). Diff is confined to `src/models/qwen3_vl/{mod,text,config}.rs`,
`src/models/gemma.rs`, and `src/models/paligemma.rs`; no other model or
shared utility code is touched. Not published; consumed only via this
workspace's root `Cargo.toml` `[patch.crates-io]` entry, exactly like
`vendor/chromey` patches `chromiumoxide`.
