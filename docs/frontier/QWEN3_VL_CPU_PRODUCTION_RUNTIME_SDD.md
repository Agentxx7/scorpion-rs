# Qwen3-VL CPU Production Runtime SDD

Frontier: `SCORPION_QWEN3_VL_CPU_PRODUCTION_RUNTIME_001`

Baseline: `57c75fec029507e66ada2a31768db591f8a7154b`

## Ownership and identity

The runtime is an offline leaf above `LocalModelInstallation`. The canonical
local-model layer owns immutable artifact identity, SHA-256 verification,
atomic activation and device/resource policy. The Qwen adapter owns only the
pinned image processor, tokenizer/template realization, Candle model loading,
generation and decoding.

The accepted tuple is Qwen/Qwen3-VL-2B-Instruct revision
`89644892e4d85e24eaac8bacfd4f463576704203`, Candle 0.11.0, CPU/F32,
processor `qwen3-vl-2b@89644892-candle-0.11.0-cpu-f32-processor-v1-320x224`.
The manifest contains exactly six files, each with exact size and SHA-256.

## Runtime graph

```text
LocalModelInstallation
→ reverify + artifact_path
→ CPU/RAM preflight
→ pinned JSON/tensor validation
→ private F32 VarBuilder + tokenizer
→ Qwen3VlGenerationFactory
→ begin_request (fresh model/KV state)
→ processor + template
→ multimodal forward + bounded greedy generation
→ decoded text
→ discard request session
```

There is no HTTP dependency, hidden cache lookup, artifact discovery, CUDA
fallback, quantization or provider/CAPTCHA logic. Runtime construction fails
below 13,253,615,616 available bytes. Only one request is active because the
closed generation-state factory owns the serialization permit.

## Processor and generation

The pinned image-only processor decodes JPEG/PNG, preserves original
dimensions, applies the upstream smart-resize pixel budget, RGB normalization,
temporal duplication, patch/merge ordering and `image_grid_thw`. The reference
96×64 fixture becomes 320×224, `[1,14,20]`, 280 patches and 70 merged tokens.
The installed tokenizer supplies special-token identities; the installed chat
template is validated before initialization. Generation is deterministic
greedy decoding with an explicit token bound and `<|im_end|>` termination.

## Lifecycle proof

The qualification-host ignored test performs canonical atomic installation
from the six already-acquired files, real image+text inference, decoded output,
unload, reinitialization and a second independent inference. Runtime network
access is structurally absent. CAPTCHA capability advertisement and empirical
qualification remain outside this frontier.
