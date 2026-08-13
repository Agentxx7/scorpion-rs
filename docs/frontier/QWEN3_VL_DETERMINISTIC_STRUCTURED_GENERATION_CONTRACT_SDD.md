# Qwen3-VL deterministic structured-generation contract

Frontier: `SCORPION_QWEN3_VL_DETERMINISTIC_STRUCTURED_GENERATION_CONTRACT_001`

Baseline: `09db29c9f38c95952ea4e3965cd031868c795b7f`

## Decision

The pinned Qwen3-VL runtime owns a neutral token-level JSON grammar contract.
Callers declare either a variable string-ID array or ordered finite numeric
fields. CAPTCHA semantics, parsing heuristics, routing, and transport remain
outside this seam.

## Execution

The verified CPU/F32 runtime renders the pinned chat template with an explicit
assistant prefill. At every generation step it ranks the model logits, decodes
candidate tokens, and admits only candidates whose complete output remains a
prefix of the declared grammar. The final token budget step admits only a
grammar-complete candidate. EOS is accepted only after completion.

An invalid schema, invalid prefill, or absence of a legal model-token
continuation returns `NoValidStructuredContinuation`. Output is never
truncated, completed, extracted, or repaired after generation.

## Ownership and lifecycle

`Qwen3VlCpuRuntime` retains canonical installation, processor, tokenizer,
model, CPU/F32, resource, and serialized request-state ownership. Structured
decoding uses the existing fresh `Qwen3VlGenerationSession`; it adds no model,
network, CUDA, or provider ownership. Free-form generation remains unchanged.

## Real proof

The ignored qualification-host test uses the immutable pinned installation and
proves strict finite-number JSON, repeat determinism, variable string IDs,
non-empty model-generated suffixes, and strict `serde_json` parsing. Synthetic
tests cover grammar rejection only and are not closure evidence by themselves.

## Done

Done requires the real pinned-model test, unit and architecture acceptance,
strict clippy, rustfmt, both diff checks, an isolated commit, pushed synchronized
refs, and a clean repository.
