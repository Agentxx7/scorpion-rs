# Canonical CAPTCHA Provider Registry and Explicit Routing SDD

Frontier: `SCORPION_CANONICAL_CAPTCHA_PROVIDER_REGISTRY_AND_EXPLICIT_ROUTING_001`

Baseline: `e6d138f7a2349d06d6632e6ea24959eac528c78f`

## Decision

Provider registration and provider routing are separate authorities. A
runtime-scoped `CaptchaProviderRegistry` maps stable provider IDs to borrowed
provider instances, rejects duplicate IDs and resolves only an identity named
by the caller. Registration order has no selection meaning.

`CaptchaRouteAttempts` is a caller-owned ledger. Each call executes exactly
the provider named by one `CaptchaSolveRequest` and retains the unmodified
outcome, including provider, locality and transport provenance. It contains no
retry, fallback, racing or substitution decision.

## Availability and credentials

Providers expose immutable capabilities and read-only runtime availability.
Availability can distinguish available, provider-unavailable and
credential-unavailable states. The registry neither acquires credentials nor
promotes another provider when one is unavailable.

## Compatibility migration

The horizontal-offset, image-grid and point-selection browser flows retain
their established policy: local LanguageModel is explicitly attempted first,
and only `ProviderUnavailable` permits the caller to acquire the Gemini
credential and explicitly attempt external Gemini. Both outcomes coexist in
the route ledger. Locality remains metadata and never affects ordering.

Canonical transport continues to own remote asset acquisition and external
provider traffic. Provider adapters receive no raw transport authority.

## Done

- duplicate provider identities fail registration;
- resolution requires an explicit provider ID;
- capabilities and availability are observable without execution;
- every attempt retains its canonical outcome and provenance;
- the registry owns no fallback, retry, ranking or credential acquisition;
- all three compatibility routes use registry resolution and explicit attempts;
- guardrails reject implicit registry routing and provenance loss.
