# Canonical CAPTCHA Solver Capability SDD

Frontier: `SCORPION_CANONICAL_CAPTCHA_SOLVER_CAPABILITY_001`

Baseline: `dbc82991f7407dc5c0133ecc5c0132041df4c806`

## Decision

CAPTCHA solving is a provider-neutral domain capability above browser
orchestration and canonical transport. The core represents image-grid
selection, horizontal-offset and point-selection challenges without CAPTCHA
vendor, model provider, browser handle or HTTP backend types.

Exactly one provider is selected on every `CaptchaSolveRequest`. Canonical
dispatch validates the provider's immutable capability advertisement before
calling it. Dispatch contains no fallback, racing, retry or substitution.

## Ownership

The canonical core owns normalized challenges, materialized visual inputs,
solutions, explicit failures, provider identity, locality and solver
provenance. Provider adapters own protocol payloads, unchanged prompts and
response translation.

Callers own anti-bot detection, provider selection, browser DOM extraction,
result application, clicks/drags/mouse movement, challenge refresh, admission
semaphores, deadlines, retries and the temporarily retained legacy routing
order.

`spider_transport` remains the sole owner of external acquisition and provider
traffic. Remote challenge assets are planning inputs only and are materialized
through `CanonicalExecutor` before provider dispatch. External Gemini uses
`CrawlerRequest`, `SecretRequestHeaders`, `CrawlerResponse` and
`CrawlerFailure`. Local LanguageModel receives no transport authority.

## Provider contract

Providers advertise provider ID, locality, supported challenge kinds, accepted
media types, maximum input count and credential requirement. The two current
provider IDs are distinct:

- `local-language-model`
- `external-gemini`

Provider ID and transport backend provenance are separate types. Local results
make no transport claim. External results retain the actual Reqwest/Wreq
backend and response origin.

## Failures

Canonical outcomes explicitly represent invalid/unsupported challenges,
provider or credential unavailability, deadlines, retained transport failures,
provider rejection, invalid responses, inconclusive answers, local execution
failure and cancellation. Empty grid selections and zero coordinates remain
valid solution values and are not canonical failure sentinels. Historical
public tuple/vector wrappers may translate explicit outcomes for compatibility,
but canonical callers consume `CaptchaSolveOutcome`.

## Migration boundary

Current browser flows retain their established provider order where required:
local LanguageModel is attempted first and external Gemini is selected only by
the caller's explicit compatibility routing after `ProviderUnavailable`.
Neither provider performs substitution. Existing prompts, response parsing,
semaphore policy and orchestration deadlines are preserved.

No evidence persistence, new provider, fallback-chain abstraction, provider
racing or browser orchestration redesign belongs to this frontier.

## Done

- all required neutral types exist;
- both current providers implement one contract and advertise capabilities;
- unsupported or unmaterialized requests fail before provider execution;
- browser state and raw clients do not enter the core request;
- remote acquisition and external execution remain canonical;
- provider and transport provenance remain distinct;
- guardrails reject provider-owned raw networking and implicit core routing.
