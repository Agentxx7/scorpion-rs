# Local CAPTCHA model cleanup and PaliGemma convergence

Frontier: `SCORPION_LOCAL_CAPTCHA_MODEL_CLEANUP_AND_PALIGEMMA_CONVERGENCE_001`

Baseline: `6336153a485344b96cee4e031ba5b644f94a030b`

## Outcome

Local CAPTCHA model ownership converges on **`paligemma-local`**. Qwen3-VL's
disqualified-for-CAPTCHA material (4B checkpoint support, both models'
point-precision qualification fixtures) is removed. Qwen3-VL's genuinely
reusable runtime (corrected MRoPE/prefill-causal-mask Candle fixes, 2B
checkpoint, structured generation, hardened semantic tests) is retained,
truthfully marked `ExecutableUnqualified`, never implicitly selectable.

## Audit classification

| Artifact | Classification | Disposition |
|---|---|---|
| `vendor/candle-transformers-qwen3vl-fix/src/models/qwen3_vl/{mod,text,config}.rs` (MRoPE + prefill-causal-mask fixes) | KEEP_CANONICAL_RUNTIME | Untouched. Genuine upstream defects, independent of which model is CAPTCHA-qualified. |
| `spider/src/features/qwen3_vl_runtime.rs` — 2B checkpoint, structured-generation grammar, hardened semantic tests | KEEP_GENERAL_VLM_CAPABILITY | Reverted to its pre-4B-frontier state (see below) — every genuine fix and generic capability preserved exactly. |
| `spider/src/features/qwen3_vl_generation.rs` — request-isolated generation-state seam | KEEP_GENERAL_VLM_CAPABILITY | Untouched; not qualification-specific. |
| `spider/src/features/qwen3_vl_captcha.rs` — 2B-only CAPTCHA provider adapter | KEEP_GENERAL_VLM_CAPABILITY (explicitly permitted as an unqualified experimental option) | Reverted to its pre-4B-frontier state; module doc comment strengthened to state unambiguously that it is **not** the canonical/qualified provider. |
| 4B pinned constants/manifest/`resolve_checkpoint_spec` generalization, `real_4b_*` tests (introduced in `SCORPION_QWEN3_VL_LOCAL_CAPTCHA_HIGHER_CAPACITY_MODEL_QUALIFICATION_001`) | REMOVE_CAPTCHA_PROVIDER_PATH / REMOVE_QUALIFICATION_ONLY | Removed — introduced solely for the failed higher-capacity CAPTCHA qualification; no other consumer. |
| `spider/tests/qwen3_vl_point_precision_audit.rs` (2B + 4B point-selection matrices) | REMOVE_QUALIFICATION_ONLY | Deleted — the qualification question it existed to answer is closed (disqualified); no forward regression value once nothing will ever route CAPTCHA `PointSelection` to Qwen. |
| `spider/tests/qwen3_vl_cpu_runtime_acceptance.rs`, `qwen3_vl_generation_state_acceptance.rs`, `qwen3_vl_local_captcha_provider_acceptance.rs`, `qwen3_vl_structured_generation_acceptance.rs` | KEEP_GENERAL_VLM_CAPABILITY | Untouched — static architecture/contract acceptance, not qualification-specific; still verify the retained runtime's real contract. |
| `docs/frontier/QWEN3_VL_CANDLE_REFERENCE_PARITY_ROOT_CAUSE_SDD.md`, `QWEN3_VL_CPU_PRODUCTION_RUNTIME_SDD.md`, `QWEN3_VL_DETERMINISTIC_STRUCTURED_GENERATION_CONTRACT_SDD.md` | HISTORICAL_DOC_ONLY | Untouched — architectural provenance for the retained runtime/Candle fixes. |
| `docs/frontier/QWEN3_VL_LOCAL_CAPTCHA_POINT_PRECISION_AND_INPUT_ENVELOPE_SDD.md`, `QWEN3_VL_LOCAL_CAPTCHA_HIGHER_CAPACITY_MODEL_QUALIFICATION_SDD.md` | HISTORICAL_DOC_ONLY | Untouched — the closure record of *why* both checkpoints were disqualified; required provenance even though the qualification-only code they describe is now removed. |
| `/home/jonny/.cache/scorpion-qwen3vl-4b-candidate/` (4B weights, 8.3 GB) | REMOVE_MODEL_ARTIFACT | Deleted. |
| `/tmp/scorpion-qwen3vl-qualification/` (2B weights, 4.0 GB) | KEEP_CANONICAL_RUNTIME | Retained — required by the retained 2B real tests. |
| `/home/jonny/.cache/qwen3vl-reference/` (Python reference-oracle venv + scripts, 990 MB) | KEEP_CANONICAL_RUNTIME | Retained — small, reusable diagnostic tooling underpinning the kept Candle fixes' own provenance; not qualification-only. |

## Code changes

`spider/src/features/qwen3_vl_runtime.rs` and `spider/src/features/qwen3_vl_captcha.rs`
were restored to their exact content at `e6c135263a1f699694c07bd901f60d8d8fbef4dd`
(the closure of `SCORPION_QWEN3_VL_CANDLE_REFERENCE_PARITY_ROOT_CAUSE_001` —
the corrected, 2B-only baseline immediately before any 4B qualification work
began), which precisely removes:

- `QWEN3_VL_4B_MODEL_REVISION`, `QWEN3_VL_4B_MINIMUM_RAM_BYTES`,
  `QWEN3_VL_4B_PROCESSOR_ID`, `PINNED_ARTIFACTS_4B`, `REQUIRED_ARTIFACTS_4B`,
  `EXPECTED_TENSORS_4B`, `qwen3_vl_cpu_f32_manifest_4b()`.
- The `Qwen3VlCheckpointSpec`/`resolve_checkpoint_spec` multi-checkpoint
  allowlist and `ResolvedArtifactPaths` generalization — built specifically
  to support two checkpoints; with only one remaining, the generalization
  itself was "qualification-only checkpoint support" and is removed along
  with it, not left as unused complexity.
- The per-instance `runtime.model_revision()`/`processor_identity()`
  provenance accessors — motivated only by the multi-checkpoint need;
  `provenance()` reverts to reading the module's own bare `QWEN3_VL_MODEL_REVISION`/
  `QWEN3_VL_PROCESSOR_ID` constants directly, truthful again for a
  single-checkpoint runtime.
- `real_4b_offline_generation_unload_and_reinitialize`,
  `real_4b_structured_generation_is_nonempty_and_strictly_parsed`,
  `real_4b_provider_registry_runtime_and_strict_outcome`.

Every genuine correctness element from the prerequisite frontier is
preserved exactly, since it was already present at this exact restore
point: `compute_mrope_position_ids` usage, the fixed prefill causal-mask
condition, the hardened semantic tests requiring exact/content-dependent
output (not merely non-empty), and the structured-generation grammar
engine.

`spider/tests/qwen3_vl_point_precision_audit.rs` was deleted outright (not
reverted) — even its original, pre-4B, 2B-only form existed solely to
answer the CAPTCHA qualification question, which is now closed.

`spider/src/features/qwen3_vl_captcha.rs`'s module doc comment was
strengthened (the only change beyond the revert) to state explicitly, in
one place a reader cannot miss, that this provider is **not** canonical —
`paligemma-local` is — with direct pointers to the closure SDDs proving
both disqualifications and the PaliGemma qualification.

## Guardrails proven

- `paligemma-local`'s own qualification/routing/regression suite is
  entirely untouched by this frontier (zero diff to any `paligemma_*.rs`
  file) — it remains the qualified `PointSelection` provider.
- `qwen3-vl-local` reports `CaptchaCapabilityQualification::ExecutableUnqualified`
  for every challenge kind (`qwen3_vl_local_captcha_provider_acceptance.rs::all_shapes_are_executable_but_empirically_unqualified`
  still asserts `!source.contains("EmpiricallyQualified")`) and its module
  doc comment now says so explicitly.
- `CaptchaProviderRegistry`/`solve_captcha` require an explicit
  `selected_provider` on every `CaptchaSolveRequest` — no implicit/default
  provider selection exists anywhere in the canonical CAPTCHA seam
  (`canonical_captcha_provider_routing_acceptance.rs::registry_and_ledger_have_no_implicit_routing_policy`
  still passes), so no fallback path was introduced or needed.
- No duplicate local CAPTCHA ownership: exactly one provider
  (`paligemma-local`) advertises no false qualification and is the only one
  documented as canonical; `qwen3-vl-local` remains a clearly-labeled,
  truthfully-unqualified experimental option.
- No dead 4B qualification path remains — grepped for every removed
  symbol name across the whole tree; zero references remain outside this
  frontier's own historical SDD prose.
- `git diff --stat` confirms zero browser/frame files touched.

## Stash cleanup

Preserved (per explicit instruction): the CAPTCHA browser-binding stash
(`SCORPION_CANONICAL_CAPTCHA_BROWSER_EXECUTION_BINDING_001`, still required)
— now at `stash@{0}` after the drops below shifted its index. The OOPIF
frame-context prototype stash is also left untouched (browser/frame-related,
out of this frontier's scope) — now `stash@{1}`.

Dropped, all superseded by real, closed, merged work with zero remaining
unique value:

- `05eefee6` — `SCORPION_QWEN3_VL_LOCAL_CAPTCHA_POINT_PRECISION_AND_INPUT_ENVELOPE_001`
  audit evidence: superseded by that frontier's own formal closure commit
  (`4fbb3c67`), which reports the identical conclusion.
- `46dce0d6` — `SCORPION_QWEN3_VL_DETERMINISTIC_STRUCTURED_GENERATION_CONTRACT_001`
  assistant-prefill prototype (110 lines): superseded by the complete, real,
  merged structured-generation engine already live in `qwen3_vl_runtime.rs`.
- `63cd9fcd` — `SCORPION_QWEN3_VL_LOCAL_CAPTCHA_PROVIDER_IMPLEMENTATION_001`
  partial implementation (103 lines across 5 files): superseded by the
  complete, real, merged `qwen3_vl_captcha.rs` this very frontier just
  reverted to its correct baseline.

## Model cache cleanup

| Directory | Before | Action | Reclaimed |
|---|---|---|---|
| `/home/jonny/.cache/scorpion-qwen3vl-4b-candidate/` | 8.3 GB | Deleted | 8.3 GB |
| `/tmp/scorpion-qwen3vl-qualification/` | 4.0 GB | Retained (required by kept 2B tests) | 0 |
| `/home/jonny/.cache/qwen3vl-reference/` | 990 MB | Retained (small, reusable diagnostic tooling) | 0 |
| PaliGemma artifacts (`scorpion-paligemma-candidate/`, `paligemma-reference/`) | — | Untouched, per explicit instruction | 0 |

**Total reclaimed: 8.3 GB.**

## Regression gates

Real: the retained 2B `qwen3_vl_runtime`/`qwen3_vl_captcha` suite (3/3)
reproduces identically post-revert. `paligemma-local`'s real suite was
re-attempted but hit the same real, previously-documented
`ResourceLimitExceeded` preflight rejection under concurrent desktop memory
load (this frontier made zero changes to any `paligemma_*.rs` file —
confirmed via `git status`/`git diff` showing no PaliGemma file touched at
all — so this is host-moment RAM availability, not a code regression;
`paligemma-local`'s own capability was independently, repeatedly proven in
its own closed qualification frontier in this same session). Static:
`qwen3_vl_cpu_runtime_acceptance`, `qwen3_vl_generation_state_acceptance`,
`qwen3_vl_local_captcha_provider_acceptance`,
`qwen3_vl_structured_generation_acceptance`,
`canonical_captcha_solver_capability_acceptance`,
`canonical_captcha_provider_routing_acceptance`,
`canonical_captcha_image_grid_input_acceptance`, architecture guardrails
(113/113) all pass unchanged. Spider default (0 failures; same two
pre-existing, diff-independent environmental flakes documented in every
prior frontier) and `spider_transport` (36/36) pass. Changed-surface clippy
and full-workspace rustfmt clean; both diff checks clean.

## What did not change

Browser/frame architecture, the preserved CAPTCHA browser-binding stash,
`paligemma-local`'s own code (zero diff), and provider-selection/routing
policy (already explicit-only, no fallback existed to remove) are all
untouched.

## Successor

Resume `SCORPION_CANONICAL_CAPTCHA_BROWSER_EXECUTION_BINDING_001` using
`paligemma-local`, restoring the preserved stash (now `stash@{0}`). No
further model qualification frontier.
