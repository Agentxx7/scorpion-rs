# candle-transformers (Qwen3-VL MRoPE + prefill causal-mask patch)

Patched fork of `candle-transformers` 0.11.0, scoped to
`src/models/qwen3_vl/`, for
`SCORPION_QWEN3_VL_CANDLE_REFERENCE_PARITY_ROOT_CAUSE_001`.

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

## Provenance

Base: `candle-transformers` 0.11.0 from crates.io (unpacked registry
source). Diff is confined to `src/models/qwen3_vl/{mod,text,config}.rs`; no
other model or shared utility code is touched. Not published; consumed only
via this workspace's root `Cargo.toml` `[patch.crates-io]` entry, exactly
like `vendor/chromey` patches `chromiumoxide`.
