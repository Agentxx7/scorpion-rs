# Local cross-model vision CAPTCHA provider qualification

Frontier: `SCORPION_LOCAL_CROSS_MODEL_VISION_CAPTCHA_PROVIDER_QUALIFICATION_001`

Baseline: `72f3ef43e5b3fa5740f11fbc2be367b776d0ebac`

## Verdict

**A. Candidate qualified.** `google/paligemma-3b-mix-224` (SigLIP + Gemma-1),
pinned at revision `d1d8734c9c3ad0ccfeea4afc270faa356c2ba515`, passes
PointSelection, ImageGridSelection, HorizontalOffset, and the resource gate,
under the new `paligemma-local` provider identity
(`CaptchaProviderId::PALIGEMMA_LOCAL`). `SCORPION_CANONICAL_CAPTCHA_BROWSER_EXECUTION_BINDING_001`
may now resume.

## Audit: candidate selection

Ranked by expected point/localization competence, resource feasibility,
Rust runtime feasibility, architecture complexity, and structured-generation
compatibility — checked before implementing anything:

| Candidate | Grounding training | Runtime fit | Verdict |
|---|---|---|---|
| Moondream2 (`vikhyatk/moondream2`) | Real, purpose-built `point`/`detect` capability — but implemented via a **separate coordinate-regression head** (`region.py`/`region_model.py`: Fourier-feature coordinate encoder/decoder MLPs interleaved into the generation stream), not plain text tokens. Current checkpoint's tensor layout (`model.region.*`, `model.text.blocks.N.*`) no longer matches the vendored `candle-transformers` `moondream.rs` port at all (that port is an older `text_model`/`vision_encoder` architecture). | Would require a genuinely new generation-loop mechanism plus new tensors — real new Rust runtime work, not a config/weights swap. | Rejected: would trigger `CROSS_MODEL_RUNTIME_PREREQUISITE_REQUIRED` on its own. |
| PaliGemma (`google/paligemma-3b-mix-224`) | Real, purpose-built `detect {label}` grounding via `<locNNNN>` **vocabulary tokens** (1024 quantized coordinate bins, ordinary next-token generation — no special head). | `candle-transformers` already ships `paligemma.rs`/`siglip.rs`/`gemma.rs`; config values match the pinned checkpoint's real `config.json` exactly. Output is plain text tokens, directly compatible with a grammar-constrained greedy-decode pattern. | **Selected.** Gated on HuggingFace (resolved: user provided an authenticated, license-accepted access token). |
| Florence-2, OWL-ViT, GroundingDINO | Excellent, purpose-built detection/grounding. | Not present in `candle-transformers` at all (novel vision encoders / encoder-decoder architectures) — would need a full new port. | Not pursued: real new runtime work, and PaliGemma already satisfied the requirement. |
| LLaVA, Pixtral, SmolVLM, base (text-only) Moondream path | General VQA/captioning; no dedicated grounding training. | Already Rust-feasible. | Not pursued: no credible reason to expect materially better pointing than a general-VQA model's free-text coordinate guessing. |

One candidate selected, one implemented — no model zoo.

## Runtime decision

**A: existing Candle primitives support the architecture truthfully**, with
three genuine correctness defects found and fixed (Branch-C-scale "small
upstream-compatible extension," not a new runtime prerequisite) — see the
vendored fork's own README for full technical detail. Root-caused against a
pinned Hugging Face `transformers` reference oracle running the identical
weights/config/tokenizer/image/prompt. All three were required together;
fixing any two alone still produced degenerate or incoherent output:

1. **Spurious image-embedding L2 normalization** in `paligemma::Model::setup`
   — the real architecture's projector output is used raw (`masked_scatter`,
   no normalization anywhere). Removed.
2. **Missing/incorrect text-only embedding scale on the multimodal prefill**
   — the real Gemma-1 `sqrt(hidden_size)` scale belongs only to
   `GemmaTextScaledWordEmbedding` (text tokens), never to vision-tower image
   features. `gemma::Model::forward_embeds` previously scaled the *entire*
   caller-supplied tensor uniformly, inflating image features ~45x. Fixed by
   removing the scale from `forward_embeds`/`forward_embeds_without_projection`
   and adding an explicit `embed_scale()` accessor the caller applies to
   exactly the text span before merging.
3. **Duplicated image-placeholder span** — `spider`'s own runtime call site
   (not the vendored fork) initially passed the full 261-token rendered
   prompt (256 `<image>` placeholders + text) as `Model::setup`'s
   `input_ids`, on top of the 256-position vision embeddings `setup` already
   concatenates internally, doubling the image span to 517 positions.
   Fixed by passing only the text-only suffix.

A fourth attempted fix was **found wrong and reverted**: adding a causal
mask to `forward_embeds`'s prefill call, by false analogy with an unrelated,
genuinely causal-decoder-only model's prefill defect. PaliGemma's real
architecture gives the *entire* image+prompt prefix full
bidirectional attention (`modeling_paligemma.py`: "can attend bidirectionally
in prefix and only causally in suffix"); only tokens generated *after* the
prefill are causal. Per-layer hidden-state statistics, checked against the
reference oracle at each of these steps, only matched exactly once this
mask was removed again (compounded to a ~2x divergence by the final layer
when incorrectly applied) — the single most useful diagnostic in this whole
frontier, and worth recording as a caution against pattern-matching a fix
from an unrelated model without independently verifying the target
architecture's own real design.

With all three genuine fixes applied (and the one false fix reverted), real
Rust inference reproduces the pinned reference oracle's exact greedy
`detect` output bit-for-bit: `<loc0407><loc0441><loc0614><loc0579>` for the
frontier's controlled fixture, independently verified against
`transformers` 5.15.0 / PyTorch CPU running the identical weights.

## Provider architecture

New, explicit `CaptchaProviderId::PALIGEMMA_LOCAL` (`"paligemma-local"`) —
not a generic "local vision-language provider" abstraction: the model
family is named directly in its identity string. Inventing a new
provider-neutral abstraction now would itself be an unrequested
provider-routing redesign ("do not redesign provider routing
unless a genuine canonical gap exists" — none was found: the existing
`CaptchaProvider` trait, `CaptchaProviderCapabilities`,
`CaptchaLocalRuntimeProvenance` contract already accommodate a second local
model with zero changes). `qualification_state` returns
`ExecutableUnqualified` uniformly, matching the existing pattern — this
frontier does not (yet) promote it to `EmpiricallyQualified`; that is a
separate, deliberate decision left to whoever resumes the CAPTCHA browser
binding frontier with real Turnstile evidence in hand.

`ImageGridSelection` reuses the same JSON string-id-array structured
grammar shape already proven elsewhere in this crate (independently
implemented in `paligemma_runtime.rs`, not extracted into a shared
cross-model module — matching this frontier's explicit instruction not to
expand structured-generation machinery beyond what this candidate genuinely
needs). `PointSelection`/`HorizontalOffset` use a new, minimal
loc-token-range grammar (`loc_constrained_token`: masked argmax over the
1024 `<locNNNN>` ids) matching PaliGemma's own real trained output format,
rather than forcing an unfamiliar JSON schema onto a model never fine-tuned
to produce one.

## Canonical CAPTCHA contract

`PointSelection`: `detect {instruction}`, parse the resulting four
`<locNNNN>` bins into a bounding box, return its deterministic center as
`CaptchaSolution::Point` — a faithful parse of the model's own genuine
spatial answer (identical in kind to how `HorizontalOffset`/`PointSelection`
already parse structured JSON into `CaptchaSolution`), not a correction.

`HorizontalOffset`: this provider's own documented instruction convention —
`"{handle description} -> {target description}"` — issues two independent
`detect` queries and returns the difference of their box centers. No new
`CaptchaChallengeKind` was introduced.

`ImageGridSelection`: unchanged JSON contract, `{"selected_ids":[...]}`.

## Point-precision qualification (PRIMARY REQUIREMENT — passed)

A required 8-fixture position matrix, a WCAG 2.5.5 44px actionable-region
tolerance (fixed before this frontier's trial ran), anti-degeneracy
assertions, and a determinism proof, through the real production
`CaptchaProvider::solve` seam. Canvas is 224x224, PaliGemma's single fixed
processor envelope.

| target | true center | predicted | error (px) | contained |
|---|---|---|---|---|
| upper_left | (30, 30) | (28.7, 30.0) | 1.3 | yes |
| upper_right | (194, 30) | (193.5, 30.0) | 0.5 | yes |
| lower_left | (30, 194) | (30.0, 193.9) | 0.1 | yes |
| lower_right | (194, 194) | (192.9, 195.0) | 1.5 | yes |
| center | (112, 112) | (111.2, 111.7) | 0.8 | yes |
| asymmetric | (60, 160) | (61.0, 159.2) | 1.3 | yes |
| small_isolated (16px) | (180, 50) | (178.4, 49.5) | 1.7 | yes |
| distractor | (72, 112) | (72.5, 111.9) | 0.5 | yes |

**Standard-size (44px) actionable containment: 7/7 (100%)**, clearing the
predefined 6/7 reliable-single-shot threshold with margin. Mean absolute
error across all 8 fixtures (including
the deliberately-hard, below-tolerance small_isolated target): ~0.96px —
essentially exact, not merely "inside the tolerance region." Determinism:
the repeated `center` trial reproduced the identical point exactly.
Anti-degeneracy: genuine position dependence confirmed (left/right and
top/bottom ordering both correct, no constant collapse, no `x == y`
regardless of target).

This confirms the frontier's own hypothesis: purpose-trained spatial
grounding (vocabulary-token detection, explicitly fine-tuned into the "mix"
checkpoint's task mixture) is a categorically better fit for pixel-precise
`PointSelection` than a general-VQA model's free-text coordinate guessing.

## Grid and offset qualification (passed)

`ImageGridSelection`: real single-cell selection returns the correct stable
ID, valid structured output. `HorizontalOffset`: real two-`detect` offset
computation returns a finite value with genuine image dependence (no
clamping or post-correction of the two boxes' own centers). Both proven
through the real production seam in
`paligemma_captcha.rs::tests::real_provider_registry_runtime_and_strict_outcome`,
alongside `PointSelection`, with truthful per-request provenance
(`model_revision`/`processor_identity` naming this exact pinned checkpoint).

## Resource gate (measured, not estimated)

| | value |
|---|---|
| weight files (F32-native, 3 shards) | 11.69 GB |
| peak RSS, real load+detect | **~23.6 GB** (measured directly, `/proc/<pid>/status` `VmRSS`, real `initialize_from_host` production path) |
| load time | ~7–9s |
| `detect` inference time | ~11–14s per call |
| image tokens | 256 (fixed; `(224/14)^2`) |
| execution | CPU/F32, single fixed envelope — no dynamic resolution |
| minimum RAM declared | 25.5 GB (measured peak + margin) |

Peak RSS is ~76% of this host's 31 GB total system RAM — real, honest
headroom exists, consistent with PaliGemma's smaller, F32-native (no
BF16→F32 doubling) weight
footprint despite comparable total parameters. `PALIGEMMA_MINIMUM_RAM_BYTES`
fails closed exactly as intended: two of several attempts during this same
frontier's own development, under ordinary concurrent desktop load, keyed
the correct `ResourceLimitExceeded` — the runtime's own fail-closed
preflight behaved correctly throughout, this documents real margin, not a
defect.

## Regression gates

Real: `real_offline_detect_reproduces_reference_and_reinitializes` (exact
bit-for-bit reference match, determinism, content-dependence),
`real_structured_ids_generation_is_valid_and_deterministic`,
`real_provider_registry_runtime_and_strict_outcome` (all three challenge
kinds through the real registry+solve seam),
`real_point_selection_precision_matrix` (7/7, threshold cleared) all pass.
Vendored `candle-transformers-qwen3vl-fix` tests pass (8/8 + doctest).
Static acceptance suites (`canonical_captcha_solver_capability`,
`canonical_captcha_provider_routing`, `canonical_captcha_image_grid_input`,
architecture guardrails 113/113) all pass unchanged. Spider default (0
failures; the same two pre-existing, diff-independent environmental flakes
documented in the prior two frontiers reproduced identically) and
`spider_transport` (36/36) pass. Changed-surface clippy and full-workspace
rustfmt clean; both diff checks clean.

(This frontier also re-ran the then-still-present local Qwen3-VL runtime's
own regression suites and confirmed they were unaffected by these edits —
`gemma.rs`/`paligemma.rs` do not touch `qwen3_vl/`. That runtime and its
regression suites no longer exist: `SCORPION_QWEN3_VL_TOTAL_REJECTION_AND_REMOVAL_001`
removed them entirely, unrelated to anything this frontier qualified.)

## What did not change

Browser/frame architecture, the blocked CAPTCHA browser binding frontier's
own preserved implementation (`stash@{1}`, untouched), Turnstile (never
rerun during this frontier's development, per its own explicit
instruction), provider-selection/fallback policy, and retry/voting/ensemble
inference are all untouched. (The then-still-present local Qwen3-VL
runtime/provider code was also untouched by this frontier — see the
regression-gates note above for its subsequent, unrelated removal.)

## Successor

Per this frontier's own Decision-A instruction: resume
`SCORPION_CANONICAL_CAPTCHA_BROWSER_EXECUTION_BINDING_001`, restoring
`stash@{1}`'s preserved implementation, and rerun genuine Turnstile
acceptance through canonical `FrameContext` → frame-aware snapshot →
`paligemma-local` → `CaptchaSolveOutcome` → revalidation → exact canonical
browser action → observable Turnstile progression, without reopening
browser/frame architecture.
