# Qwen3-VL higher-capacity local CAPTCHA model qualification

Frontier: `SCORPION_QWEN3_VL_LOCAL_CAPTCHA_HIGHER_CAPACITY_MODEL_QUALIFICATION_001`

Baseline: `4fbb3c67995f9026abb8d80a27d3543d2d765758`

## Verdict

**`QWEN3_VL_FAMILY_POINT_PRECISION_INSUFFICIENT`** (frontier branch C).

The candidate — `Qwen/Qwen3-VL-4B-Instruct`, the smallest higher-capacity
same-family checkpoint — loads and runs truthfully through the existing
corrected runtime with **zero architecture-specific hacks**, passes every
reference-parity/semantic-coherence check, and produces a real, measurable
reduction in mean point-selection pixel error (~34% lower than the 2B
baseline) — but its containment rate against the predeclared actionable-region
tolerance is **exactly tied** with the already-rejected 2B baseline (3 of 7
standard fixtures, both), not a material improvement, and does not clear the
predefined reliable-single-shot threshold (6 of 7). Per the frontier's own
instruction, a tie is not a qualifying improvement. Resource cost is also
genuinely tight on the only available test host (peak RSS ~26–27 GB of 31 GB
total system RAM), a compounding but secondary fact.

`SCORPION_CANONICAL_CAPTCHA_BROWSER_EXECUTION_BINDING_001` remains blocked
and untouched. Per this frontier's own text: "request a separate cross-model
vision-provider qualification frontier."

## Audit: same-family runtime compatibility

Checked before downloading any weights, using the HuggingFace API and small
metadata/config file fetches only:

| Property | 2B (`Qwen/Qwen3-VL-2B-Instruct`) | 4B (`Qwen/Qwen3-VL-4B-Instruct`) |
|---|---|---|
| `vision_config` | depth 24, hidden 1024, patch 16, merge 2, temporal 2, `deepstack_visual_indexes: [5,11,17]` | **byte-identical**, only `out_hidden_size` differs (2048→2560, config-driven) |
| `text_config.rope_scaling` | `mrope_section: [24,20,20]`, `mrope_interleaved: true` | **identical** |
| `text_config.head_dim` | 128 | **identical** (128) |
| `text_config` width/depth | hidden 2048, 16 heads, 28 layers | hidden 2560, 32 heads, 36 layers |
| tokenizer.json / tokenizer_config.json / chat_template.json / preprocessor_config.json | pinned hashes | **byte-identical SHA-256** to the 2B checkpoint's own pinned files — verified, not assumed |
| weight file layout | one `model.safetensors` | two shards + `model.safetensors.index.json` (713 real tensors vs. 625; consistent with 8 more decoder layers × ~11 tensors/layer) |
| tensor naming | `model.language_model.layers.N.*`, `model.visual.{blocks,merger,deepstack_merger_list,patch_embed}.*` | **identical naming convention** |
| weight dtype | BF16 | BF16 |

Conclusion: this is a textbook dense width/depth scale-up within one
architecture family — same vision tower, same MRoPE section, same tokenizer,
same tensor naming. The only genuine loading-layer gap was **pre-sharded
weights**, and `candle-core` 0.11.0's own `MmapedSafetensors::multi`/
`VarBuilder::from_mmaped_safetensors` already accept an arbitrary path slice
— no fork, no second runtime, no architecture-specific code required.

## What changed in the runtime

- `spider/src/features/qwen3_vl_runtime.rs`: generalized from a single
  hardcoded checkpoint to an explicit, closed allowlist of exactly two
  pinned checkpoint specs (`resolve_checkpoint_spec`), selected only by an
  exact match against an installation's own verified identity — never
  inferred. Added `QWEN3_VL_4B_MODEL_REVISION`, `QWEN3_VL_4B_MINIMUM_RAM_BYTES`,
  `QWEN3_VL_4B_PROCESSOR_ID`, `PINNED_ARTIFACTS_4B`, `qwen3_vl_cpu_f32_manifest_4b()`.
  `required_paths`/`validate_pinned_json`/`validate_safetensors` now operate
  on a named `ResolvedArtifactPaths` (one or more weight shard paths plus
  the five artifacts identical across every pinned checkpoint) instead of
  positional-index `Vec<PathBuf>` — a structural necessity for sharded
  weights, not an opportunistic refactor. RAM preflight now happens *after*
  checkpoint resolution (genuinely required: two checkpoints have two
  different minimums; checking a fixed constant before knowing which
  checkpoint applies would silently under-preflight a bigger model). The 2B
  path's own constants, manifest function, and single-file loading are
  **byte-for-byte unchanged** — verified by re-running the full 2B real-model
  suite and reproducing identical output to the prior closed frontier.
- `spider/src/features/qwen3_vl_captcha.rs`: `provenance()`/`failed()` now
  read `runtime.model_revision()`/`runtime.processor_identity()` (resolved
  per-instance from whichever checkpoint actually loaded) instead of fixed
  module constants — required because more than one pinned checkpoint can
  now back the same `qwen3-vl-local` provider identity, and provenance must
  truthfully name whichever one ran.
- **Provider identity unchanged**: still exactly `qwen3-vl-local`
  (`CaptchaProviderId::QWEN3_VL_LOCAL`). No `qwen3-vl-4b-local` was created —
  checkpoint size is a pinned-configuration detail, not a distinct provider
  identity, matching the frontier's explicit preference. `qualification_state`
  is untouched (`ExecutableUnqualified` for every supported kind, for either
  checkpoint) — this frontier does not promote anything to
  `EmpiricallyQualified`.

## Reference parity gate (passed)

Through the exact same corrected runtime code path as the already-hardened
2B tests — no checkpoint-specific inference logic exists anywhere:

- `real_4b_offline_generation_unload_and_reinitialize`: two independent
  init→generate→unload cycles. Gradient fixture → `"Gradient"`, matching
  both the pinned reference oracle and the 2B checkpoint's own output on the
  identical fixture. A materially different (flat-color) image produces a
  materially different description. Repeated isolated sessions reproduce
  identical output (deterministic greedy decoding).
- `real_4b_structured_generation_is_nonempty_and_strictly_parsed`: numeric
  and string-array structured schemas both produce valid, strictly-parsed,
  deterministic output.
- `real_4b_provider_registry_runtime_and_strict_outcome`: the real
  `CaptchaProvider::solve` seam through the registry, proving provenance
  truthfully carries the 4B revision/processor identity (not the 2B
  constants) — directly exercises the `provenance()` change above.

All three pass. No MRoPE regression, no prefill/KV-cache regression — the
same defects fixed for 2B in the prerequisite frontier do not reappear for a
differently-shaped decoder, confirming those fixes were genuinely
config-driven rather than accidentally tuned to 2B's specific dimensions.

## Point-precision qualification (failed)

Identical methodology to the already-closed 2B frontier — same required
8-fixture position matrix (four corners, center, asymmetric x≠y, small
isolated target, distractor-containing target), same WCAG-2.5.5-anchored
44px actionable-region bounding-box tolerance (fixed before this frontier
ran, not tuned to either model's result), same anti-degeneracy assertions,
run through the real `CaptchaProvider::solve` seam
(`real_4b_point_selection_precision_matrix`, shared verbatim runner code
with the 2B test via `run_qualification_matrix` — the comparison is directly
meaningful because nothing about the measurement differs, only the
checkpoint).

| target | true | 2B predicted | 2B err (px) | 4B predicted | 4B err (px) | 4B contained |
|---|---|---|---|---|---|---|
| upper_left | (40,40) | (32,32) | 11.3 | (57,78) | 41.6 | no |
| upper_right | (280,40) | (288,184) | 144.2 | (312,108) | 75.2 | no |
| lower_left | (40,184) | (32,192) | 11.3 | (57,187) | 17.3 | **yes** |
| lower_right | (280,184) | (288,192) | 11.3 | (317,217) | 49.6 | no |
| center | (160,112) | (208,112) | 48.0 | (160,112) | **0.0** | **yes** |
| asymmetric | (90,170) | (128,168) | 38.1 | (100,176) | 11.7 | **yes** |
| small_isolated (16px) | (250,60) | (288,128) | 77.9 | (256,112) | 52.3 | no |
| distractor | (100,112) | (160,168) | 82.1 | (128,128) | 32.2 | no |

- 2B mean error (7 standard fixtures): 49.5px. 4B mean error: 32.5px — a
  real ~34% reduction, and the worst catastrophic miss shrank from 144px to
  75px.
- 2B standard-size containment: **3/7**. 4B standard-size containment:
  **3/7** — exactly tied, not a material improvement, despite the clear
  continuous-error gain.
- Predefined reliable-single-shot threshold (fixed before this frontier's
  4B trial ran, justified in the test's own doc comment: a fail-closed,
  no-retry, security-relevant single browser action needs a high bar,
  deliberately well above a "marginal improvement" the frontier's own text
  warns against declaring qualified): **6/7**. Not met.
- Determinism: repeated `center` trial reproduced identical output.
  Structured-output validity: 9/9. Anti-degeneracy: genuine position
  dependence confirmed (not constant, not `x == y` regardless of target,
  left/right and top/bottom ordering both correct).

The aggregate accuracy gain is real and worth recording, but the frontier's
own predeclared, binary, actionable-region methodology — deliberately
chosen because "if the model output falls outside the actionable target
region, that case is a failure even if the numeric error looks superficially
small" — is what the qualification decision is bound to, and by that
methodology 4B does not qualify.

## Resource envelope (measured, not estimated)

| | 2B (existing) | 4B (candidate) |
|---|---|---|
| weight file size (BF16) | 4.26 GB (1 file) | 8.28 GB (2 shards) |
| peak RSS, real load+generate | ~8.8 GB (prior frontier) | **~26–27 GB** (two independent measured runs, real production `initialize_from_host` path) |
| load time | ~2s | ~5s |
| generation time (8 tokens) | ~7–14s per call | ~13–20s per call |
| minimum RAM declared | 12.34 GB (unchanged) | 28 GB (derived from measured peak + margin, not formula-only) |

Real, honest finding: peak RSS is ~85% of this host's 31 GB total system
RAM. Two of several real preflight attempts on this shared desktop host
failed closed (`ResourceLimitExceeded`, available 22–24 GB against the
28 GB requirement) under ordinary concurrent desktop load (browser, IDE,
other applications) before succeeding on a quieter attempt — the runtime's
fail-closed preflight behaved correctly throughout; this documents real
margin tightness, not a defect. This is compounding evidence, not the
primary verdict: precision already fails on its own, independent of
resource cost. Per the frontier's own instruction to keep capability and
deployment feasibility as separate facts — both point the same direction
here, but only the precision fact is load-bearing for this closure.

An 8B same-family checkpoint was not attempted: extrapolating this
frontier's own measured 2B→4B resource scaling (~2.27× for a 2× parameter
step) would put 8B's peak RSS at roughly 55–60 GB, which cannot fit this
host's 31 GB total RAM at all — not merely tight. Attempting it would
almost certainly fail to complete a load in any reasonable time (or fail
outright), producing no useful accuracy evidence while consuming
substantial wall-clock time. Combined with 4B's own tied (not improved)
containment result, climbing further within the family on this evidence
was judged not worth pursuing inside this frontier's scope.

## Regression gates

Real: `real_4b_offline_generation_unload_and_reinitialize`,
`real_4b_structured_generation_is_nonempty_and_strictly_parsed`,
`real_4b_provider_registry_runtime_and_strict_outcome`,
`real_4b_point_selection_precision_matrix` (formal, real preflight) all
pass; the pre-existing 2B real suite re-verified identical to the prior
closed frontier (byte-identical predictions). Static: architecture
guardrails (113/113), `qwen3_vl_cpu_runtime_acceptance`,
`qwen3_vl_generation_state_acceptance`, `qwen3_vl_local_captcha_provider_acceptance`,
`qwen3_vl_structured_generation_acceptance`, `canonical_captcha_solver_capability_acceptance`,
`canonical_captcha_provider_routing_acceptance`, `canonical_captcha_image_grid_input_acceptance`
all pass unchanged. Vendored `candle-transformers-qwen3vl-fix` tests pass
(8/8 + doctest). Spider default (0 failures; two pre-existing,
diff-independent environmental flakes — an `io_uring` TCP-connect hang and
a timing-sensitive chunk-idle-timeout test, both reproduced identically on
a clean checkout and confirmed unrelated to this diff) and `spider_transport`
(36/36) pass. Changed-surface clippy and full-workspace rustfmt clean; both
diff checks clean.

## What did not change

Browser/frame architecture, the blocked CAPTCHA browser binding frontier's
own code, provider routing/fallback policy, Gemini, retry/voting/ensemble
inference, and Turnstile are all untouched, per this frontier's explicit
scope. The structured-generation search gap found (but not fixed, by design)
in the prior frontier's 640x448 envelope probe remains unfixed and
unreachable at the shipped 320x224 envelope — irrelevant to this frontier's
own 320x224-only evaluation.

## Successor

Per the frontier's own branch-C instruction: this frontier requests a
separate, dedicated cross-model vision-provider qualification frontier (a
genuinely different vision-language model family) rather than continuing to
climb the Qwen3-VL family here. `SCORPION_CANONICAL_CAPTCHA_BROWSER_EXECUTION_BINDING_001`
remains blocked; `stash@{1}` is preserved untouched.
