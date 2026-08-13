# OpenAI Vision CAPTCHA Provider SDD

Frontier: `SCORPION_OPENAI_VISION_CAPTCHA_PROVIDER_001`

Baseline: `034c24397ea40de13a1067ef0155e57b1513753f`

## Decision

OpenAI vision is an external `CaptchaProvider` adapter. It adds no routing,
fallback, ranking or retry behavior. Callers explicitly construct it with a
model identifier and credential, register it, and select
`CaptchaProviderId::OPENAI_VISION` on one `CaptchaSolveRequest`.

## Capability

The immutable advertisement supports `ImageGridSelection`,
`HorizontalOffset` and `PointSelection`, accepts only JPEG and PNG, limits a
request to 16 materialized inputs, has `External` locality and requires a
credential. Remote assets remain invalid at provider dispatch and must be
materialized by the caller through canonical transport.

## Transport and secrets

One private persistent `CanonicalExecutor` sends requests to the fixed OpenAI
Responses endpoint. The adapter constructs `CrawlerRequest` and consumes
`CrawlerResponse`/`CrawlerFailure`; it owns no OpenAI SDK client or raw HTTP
client. The caller-supplied API key is applied as a bearer credential through
`SecretRequestHeaders`. It never enters the URL, payload, debug output,
provenance or serialized configuration.

## Translation

The provider converts materialized inputs to inline data URLs and requests a
strict JSON answer. Parsing denies unknown fields, rejects non-finite
coordinates and validates image-grid IDs as a unique subset of supplied IDs.
Success retains `OPENAI_VISION` provider identity plus actual canonical
transport backend and response origin. Existing neutral CAPTCHA failures cover
credential, deadline, transport, rejection and invalid-response outcomes.

## Done

- explicit model configuration and read-only availability;
- conservative deterministic capability advertisement;
- canonical transport and secret-header authentication only;
- strict translations for all advertised challenge kinds;
- neutral outcomes and truthful provenance;
- no provider/core/routing ownership expansion.
