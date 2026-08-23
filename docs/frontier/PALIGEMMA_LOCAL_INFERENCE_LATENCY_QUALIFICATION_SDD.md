# PaliGemma-local inference latency qualification

Frontier: `SCORPION_PALIGEMMA_LOCAL_INFERENCE_LATENCY_QUALIFICATION_001`

Baseline: `0bb88947be76b265279184dbc805c7be8b25dc4f`

Blocked dependent frontier: `SCORPION_CANONICAL_CAPTCHA_BROWSER_EXECUTION_BINDING_001`
(classified blocker: `REAL_ACCEPTANCE_LATENCY_EXCEEDS_CHALLENGE_LIFETIME` — real
CPU/F32 `paligemma-local` single-query inference took ~426s, longer than a
genuine Cloudflare Turnstile interactive test challenge stays valid, so the
frame-aware revalidation seam correctly fail-closed with `TargetReplaced`
before any browser action was dispatched — `actions_applied: 0`).

## Verdict

**A. Accelerated runtime meets the latency budget and reference-parity
precision.** `paligemma-local` gains a second, explicit, fail-closed
constructor — CUDA/F16 — on the *same* `PaligemmaCpuRuntime` type and the
*same* `CaptchaProviderId::PALIGEMMA_LOCAL` provider. No new provider, no
CPU fallback, no vendored model changes: `candle-transformers`' existing
`paligemma`/`siglip`/`gemma` modules were already fully device/dtype-generic
before this frontier began.

## Audit (required order)

1. **Existing Candle CUDA support for the exact PaliGemma architecture.**
   `candle-core` 0.11.0 ships a first-party `cuda` feature (`cudarc` 0.19.8
   + `cublas`), forwarded unchanged by `candle-nn` and by the vendored
   `candle-transformers` fork. `siglip.rs`/`gemma.rs`/`paligemma.rs` contain
   zero `Device::Cpu` literals and zero `cfg(feature = "cuda")` branches —
   every op is a generic `candle`/`candle_nn` call. Real: `nvcc` 12.4 and
   the NVIDIA 550.163.01 driver (CUDA 12.4) are present on this host, and
   `cargo check --features local_paligemma_cuda` genuinely compiles
   `candle-kernels` (real `nvcc` kernel builds), not merely CPU code
   guarded by a feature flag.
2. **F16 execution correctness.** `gemma::RmsNorm` already upcasts
   `F16 | BF16 -> F32` internally before computing (pre-existing code, not
   added by this frontier) — the architecture already anticipated
   low-precision execution. Confirmed empirically, not just theoretically:
   see Reference parity below.
3/4. **GPU placement of the vision tower and decoder.** Both together: the
   pinned checkpoint's full weight set is 11.7 GB in native F32 — larger
   than this host's 10 GB VRAM budget — so full-model F32 GPU residency is
   architecturally impossible, but F16 halves that to ~5.85 GB of weights,
   comfortably fitting the whole model (vision tower + projector + decoder)
   on one device. `VarBuilder::from_mmaped_safetensors(paths, DType::F16,
   &cuda_device)` converts and uploads directly per-tensor from the F32
   mmap source; no intermediate full-precision GPU copy of the whole model.
5. **Mixed CPU/GPU placement.** Not needed — single-device F16 residency
   already fits with real measured headroom (see Resource qualification).
6. **KV-cache placement/dtype.** Follows the model's own compute dtype
   (F16) automatically; real measured footprint is negligible next to the
   weights (18 Gemma layers, 1 KV head, head_dim 256 — a few MB).
7. **Avoidable F32 copies.** One genuine defect found and fixed (not
   pre-existing, introduced only because this frontier's own code assumed
   F32 downstream of the model): `process_image` built pixel tensors via
   `Tensor::from_vec::<f32>` unconditionally, and `last_logits` fed the
   final-position logits straight into `.to_vec1::<f32>()` — both hard
   failures under F16 (`dtype mismatch in conv2d`, then `unexpected dtype,
   expected F32, got F16`) until fixed: `process_image` now normalizes in
   F32 (matching the reference processor's own arithmetic) and casts once
   to the runtime's own dtype at the end; `last_logits` now always casts
   its single-row slice to F32 before greedy argmax/ranking, mirroring the
   model's own internal norm-upcast pattern. Both are cheap, one-time,
   small-tensor casts — not a hot-path cost.
8. **Model loading/deserialization overhead vs. actual forward/generation
   latency.** Real measured split (see below): loading is dominated by
   mandatory installation re-verification (SHA-256 over the full 11.7 GB
   pinned shards, run twice — once in `LocalModelManifest::activate`, once
   in `PaligemmaCpuRuntime::initialize`'s `installation.reverify()`) at
   ~239s, identical on both backends since it is device-independent and
   happens once per process lifetime, not per query. This is real overhead
   worth flagging for a future frontier, but it is not part of the
   per-query latency this frontier's budget concerns (the browser-binding
   test constructs the provider once, then solves the live challenge).
9. **Image preprocessing overhead.** Negligible on both backends (~10-27ms).
10. **Token-generation loop overhead.** Not separately broken out from
    prefill: PaliGemma's `detect` grounding query issues one `setup`
    (prefill: vision tower + projector + Gemma prefill in one call) plus
    exactly 4 forced `<locNNNN>` decode steps. Splitting vision tower vs.
    projector vs. decoder prefill further would require instrumenting the
    vendored model's private fields; since the whole model resides on one
    device with no cross-device transitions, the split does not change
    this frontier's conclusion and was not pursued (`Do not optimize an
    assumed hotspot` was already satisfied by measuring load, preprocess,
    and prefill+generation as three real, distinct wall-clock numbers).

Quantization was not evaluated: native CUDA + F16 already met the frozen
latency budget by 34-52x with room to spare, so per the frontier's own
instruction ("Only audit quantization if native CUDA/F16/BF16 cannot meet
the frozen latency/resource budget") it was never in scope here.

## Measurements (real, `SCORPION_PALIGEMMA_PINNED_ARTIFACTS` pinned
installation, `google/paligemma-3b-mix-224` @ `d1d8734c9c3ad0ccfeea4afc270faa356c2ba515`)

Host: AMD Ryzen 7 5800X (8C/16T), 32 GB RAM, NVIDIA RTX 3080 10 GB VRAM,
driver 550.163.01 / CUDA 12.4.

| Phase | CPU/F32 | CUDA/F16 |
|---|---|---|
| Installation re-verify + load | 238.7s | 238.4s (device-independent) |
| Image preprocessing | 0.010s | 0.023s |
| `detect` (prefill + 4-token generation), cold | 421.23s | **1.591s** |
| `detect` (prefill + 4-token generation), warm | 399.90s | **1.049s** |
| Peak host RSS after `detect` | 23,596,589,056 B (~22.0 GiB) | 12,338,528,256 B (~11.5 GiB) |
| Peak VRAM after `detect` | n/a | 7,098,646,528 B (6,769 MiB) |
| Unload | 0.354s | 0.484s |

CUDA/F16 measured speedup on the compute-bound phase: **~265x (cold) /
~381x (warm)**.

## Latency budget

Measured, not assumed: two independent passive observations of one real,
authorized Cloudflare Turnstile interactive test challenge
(`3x00000000000000000000FF`), watching only for the widget's own child CDP
target being replaced or its real `data-expired-callback` firing — **no
challenge was ever solved or clicked** during this measurement, per the
frontier's explicit "do not repeatedly solve it during optimization"
instruction.

| Trial | Observed lifetime before target replacement |
|---|---|
| 1 | 110.062s |
| 2 | 110.088s |

Conservative minimum observed lifetime: **110.062s**. Frozen budget (the
frontier's own preferred rule, `< 50%` of the minimum observed lifetime),
fixed before any accelerated-latency number informed it: **55.031s**.

Real measured CUDA/F16 latency (1.591s cold / 1.049s warm) clears this
budget by roughly **34-52x** — not a marginal pass.

*Note on ordering:* the CPU/F32 baseline and the first CUDA/F16 latency
measurement were both taken (out of necessity — the CUDA path had to be
debugged to a working state first) before this passive lifetime
measurement. The budget rule above was nonetheless applied mechanically
against the real measured lifetime, not reverse-fit to the already-known
CUDA latency; the margin (34-52x) is large enough that no plausible
alternative conservative-minimum methodology would have changed the
outcome.

## Resource qualification

| | CPU/F32 | CUDA/F16 |
|---|---|---|
| Minimum required host RAM | 25.5 GB (`PALIGEMMA_MINIMUM_RAM_BYTES`) | 13.5 GB (`PALIGEMMA_CUDA_MINIMUM_RAM_BYTES`) |
| Minimum required VRAM | n/a | 8.0 GB (`PALIGEMMA_CUDA_MINIMUM_VRAM_BYTES`) |
| Real measured peak | 23.6 GB RSS | 11.5 GB RSS + 6.77 GB VRAM |

Both floors are the real measured peak rounded up with a safety margin
(~8-12%), not formula-only estimates. The CUDA/F16 path's real resource
footprint is dramatically lighter than the CPU/F32 path's on both axes: on
this 32 GB/10 GB-VRAM host, the accelerated path passes preflight under
ordinary desktop load (Firefox/VS Code/Discord running) — unlike the
CPU/F32 path, which needed real headroom freed first. `Device::new_cuda`
fails closed (propagated as `PaligemmaRuntimeFailure::DeviceUnavailable`)
when no CUDA device is present; there is no CPU fallback path reachable
from `initialize_cuda_f16`/`initialize_cuda_f16_from_host`.

## Reference parity

`spider/tests/paligemma_cuda_point_precision_audit.rs` mirrors the frozen
CPU/F32 8-fixture required position matrix
(`spider/tests/paligemma_point_precision_audit.rs`) exactly — identical
fixtures, identical WCAG 2.5.5 44px actionable-region criterion, identical
anti-degeneracy and determinism assertions — through the accelerated
backend instead. Result: **7/7 standard-size fixtures contained**, an exact
match to the CPU/F32 baseline's own 7/7, mean error ~0.65px (CPU/F32
baseline: ~0.96px). The test additionally hard-asserts
`contained_count == CPU_F32_BASELINE_CONTAINED` — a silent accuracy
regression, not just a passed threshold, would fail this test.

`spider/src/features/paligemma_captcha.rs`'s
`tests::real_cuda_provider_registry_runtime_and_strict_outcome` mirrors the
CPU/F32 `real_provider_registry_runtime_and_strict_outcome` test: registry
resolution, `PointSelection`, `ImageGridSelection`, and `HorizontalOffset`
all pass through the real accelerated backend, with provenance asserted to
truthfully report `PALIGEMMA_CUDA_RUNTIME_IDENTITY` /
`PALIGEMMA_CUDA_PROCESSOR_ID` (never the CPU/F32 identity strings).

## Design

No second provider, no second `CaptchaProviderId`, no architecture
reopened. `PaligemmaCpuRuntime` (name unchanged for compatibility; its
`device`/`dtype`/`runtime_identity`/`processor_identity` fields were always
meant to vary, not hardcoded on purpose) gained:

- A private shared `initialize_on_device(installation, device, dtype,
  runtime_identity, processor_identity)` constructor, factored out of the
  original `initialize` with **zero behavior change for the CPU/F32
  path** — same `Device::Cpu`, same `DType::F32`, same identity strings,
  same call order.
- `initialize_cuda_f16` / `initialize_cuda_f16_from_host`, gated behind the
  new, additive `local_paligemma_cuda` Cargo feature (requires a real CUDA
  toolchain to build; `local_paligemma` itself is completely unaffected).
- `paligemma_cuda_f16_manifest()`, sharing the exact same pinned
  artifacts/identity as `paligemma_cpu_f32_manifest()` (same model
  revision, same SHA-256-pinned files) — only `runtime_requirements`
  differs, declaring `LocalModelDevice::Cuda` with **no** `Cpu` fallback
  entry.
- `PaligemmaRuntimeFailure::DeviceMemoryLimitExceeded` /
  `DeviceUnavailable`, distinct from the existing host-RAM
  `ResourceLimitExceeded`, so a VRAM shortfall or a missing CUDA device
  never gets mistaken for (or silently substituted by) a host-RAM issue.

`spider/src/features/paligemma_captcha.rs` gained one new constructor
(`initialize_cuda_f16_from_host`) on the same `PaligemmaLocalCaptchaProvider`
struct; `provenance()`/`failed()` now read `runtime_identity`/
`processor_identity` from the constructed runtime instance instead of a
fixed CPU-only constant, so provenance is truthful regardless of which
constructor built the provider.

## Operational debt noted (out of scope for this frontier)

The ~239s installation re-verification (SHA-256 over the full 11.7 GB
pinned shards, computed **twice** per process lifetime — once in
`LocalModelManifest::activate`, once again in
`PaligemmaCpuRuntime::initialize`'s `installation.reverify()`) is real,
measured overhead, unrelated to device/dtype. It was also traced to a
second, separate finding during this frontier's resource-preflight work: a
~4 GB stale `tmpfs` allocation from an old, unrelated local-model
installation, retained through three hard links across abandoned staging/qualification
directories, was consuming host memory headroom until manually identified
and removed. Both are candidates for a future, separately-scoped canonical
model-artifact staging/cleanup frontier — not addressed here, per this
frontier's own "out of scope: general GPU framework rewrite" and "do not
reopen" instructions.
