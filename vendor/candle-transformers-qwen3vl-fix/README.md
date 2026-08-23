# candle-transformers (PaliGemma / Gemma-1 correctness patches)

Patched fork of `candle-transformers` 0.11.0, scoped to
`src/models/gemma.rs` and `src/models/paligemma.rs`, for
`SCORPION_LOCAL_CROSS_MODEL_VISION_CAPTCHA_PROVIDER_QUALIFICATION_001`.

## PaliGemma / Gemma-1 patches

Root-caused against a pinned Hugging Face `transformers` reference oracle
(running the identical `google/paligemma-3b-mix-224` weights/config/
tokenizer/image/prompt as `spider`'s own `paligemma_runtime.rs`; see
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
`forward_embeds`'s multi-position prefill call. PaliGemma's real
architecture is not a plain causal decoder-only prefill:
`modeling_paligemma.py` explicitly gives the *entire* image+prompt prefix
full bidirectional attention (driven by `token_type_ids`/
`block_sequence_ids` — "can attend bidirectionally in prefix and only
causally in suffix"), and only tokens *generated after* the prefill are
causal. Adding a causal mask to the prefill call was itself a regression,
confirmed empirically via per-layer hidden-state checkpoint comparison
against the reference oracle: statistics only matched exactly with
bidirectional prefill attention restored (the original, unmasked
`forward_embeds` behavior), and compounded to a ~2x final-layer divergence
under the incorrectly-added mask. `forward_embeds`/
`forward_embeds_without_projection` build no mask of their own; the caller
is responsible for supplying whatever masking its own architecture
requires, same as the embedding-scale fix above.

## Provenance

Base: `candle-transformers` 0.11.0 from crates.io (unpacked registry
source). Diff is confined to `src/models/gemma.rs` and
`src/models/paligemma.rs`; no other model or shared utility code is
touched. Not published; consumed only via this workspace's root
`Cargo.toml` `[patch.crates-io]` entry, exactly like `vendor/chromey`
patches `chromiumoxide`.

This fork previously also carried a `src/models/qwen3_vl/` MRoPE/
causal-mask correctness patch, used by `spider`'s own (now removed)
Qwen3-VL CAPTCHA provider. That patch, and the module it patched, were
deleted when Qwen3-VL was rejected as an architectural direction
(`SCORPION_QWEN3_VL_TOTAL_REJECTION_AND_REMOVAL_001`); this fork is kept
solely for the PaliGemma/Gemma-1 patches above. The directory name and
crate identity (`candle-transformers`, required for the
`[patch.crates-io]` mechanism to apply) are otherwise unchanged from
upstream and are not renamed by that removal.
