# Candle Qwen3-VL Generation State Seam SDD

Frontier: `SCORPION_CANDLE_QWEN3_VL_GENERATION_STATE_SEAM_001`

Baseline: `ff784ee7a7a0180cbf7f0693578c661df8a011a9`

## Decision

Design B is selected: immutable `VarBuilder` backend data and pinned config are
factory-owned; every independent request constructs and exclusively owns a new
`Qwen3VLModel`. Candle creates new private KV caches in every model constructor.
The request model is never pooled, cloned, reset through private fields, or
returned to the factory.

Design A is rejected because Candle exposes no public Qwen3-VL cache-reset
operation. Adding one would require an upstream patch. Design C is the ideal
long-term upstream shape, but Candle does not expose weights/session separation
and implementing it locally would be a private backend fork.

## Ownership

```text
Qwen3VlGenerationFactory
├─ pinned Config
└─ cloneable VarBuilder ── Arc<immutable backend>
       │
       ├─ begin request A → new Qwen3VLModel → private KV A → drop
       └─ begin request B → new Qwen3VLModel → private KV B → drop
```

`VarBuilder::clone` shares its backend `Arc`; it does not contain generation
state. `Qwen3VLModel::new` creates every layer's `KvCache::new` independently.

## Termination

Success, model/provider error, cancellation, deadline and panic all unwind or
drop the request-owned session. Its model and KV caches are discarded
infallibly. There is no cleanup operation that could fail and incorrectly move
a runtime back to Ready. A construction failure returns a sanitized error and
creates no session.

## Concurrency

Sessions are state-isolated and serialized through one factory-owned async gate
retained by the request session. Cancellation and deadline drops release both
session state and gate. Concurrent inference remains unavailable until
independently proven.

## Upstream delta

None. Candle 0.11.0 is consumed through public `Config`, `VarBuilder`,
`Qwen3VLModel::new`, and `Qwen3VLModel::forward` APIs. No vendoring, patch
section, private field access or fork is introduced. An upstream session API
could reduce repeated construction cost later but is not required for
correctness.

## Dependencies

The `local_qwen3_vl` feature explicitly selects aligned Candle 0.11.0 core,
NN and transformers crates. Existing optional `memvid-rs` remains on Candle
0.8.4; types never cross that compatibility boundary.

## Out of scope

No model acquisition, processor, tokenizer/template work, generation loop,
CAPTCHA provider, CPU/CUDA benchmark, routing, Moondream, or runtime network
access is introduced.
