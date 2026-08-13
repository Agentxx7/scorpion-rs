# Wreq Security and Transport Convergence SDD

## Baseline and preserved decisions

This frontier starts at `588b90581eef9e518eb3092ec9b7ada9d986fb65`
(`refactor: isolate wreq execution authority`). The compatibility boundary,
truthful Wreq provenance, fail-closed Tor and cache combinations, and Gemini's
`CAPABILITY_LOCAL_NONCANONICAL_WREQ` classification remain binding.

## Ownership and graph

Canonical Website policy resolves a feature-selected backend before execution.
The Wreq build resolves a leaf-owned `ResolvedWreqExecutor`; Website never
receives its private clients. Both canonical backends accept `CrawlerRequest`
and return `CrawlerResponse` or `CrawlerFailure`. Spider retains retries,
backoff, link extraction, robots, sitemap, and page policy.

`ResolvedWreqExecutor` owns persistent client/session pools and realizes the
canonical transport configuration through Wreq APIs. `spider_transport` owns
initial target/onion validation, redirect SSRF decisions, proxy validity, TLS
policy decisions, DNS/interface/local-address configuration, timeouts, secret
headers, execution, and response/failure translation.

## Request, redirect, and proxy semantics

`CrawlerRequest` is the only canonical network request model. Every request is
validated immediately before selection of a private client. The Wreq redirect
callback is only an adapter over the same canonical redirect decision used by
the reqwest executor. Website's loose, none, and strict redirect intent and its
configured redirect limit are resolved into leaf-owned Wreq redirect mode.
Invalid configured proxies fail executor resolution;
none may be filtered, dropped, or replaced with direct execution.

## TLS, DNS, interface, and sessions

Wreq BoringTLS emulation is backend realization, not policy ownership.
`accept_invalid_certs` is supported and applied by disabling both certificate
and hostname verification. Emulation is supported and applied. TLS-version
knobs absent from `CrawlerTransportConfiguration` are not applicable.
Local-address and supported interface binding are applied. A supplied Wreq DNS
resolver is applied through a private erased-resolver adapter. Cookie jars and
connection pools live for the executor lifetime.

## Secrets and outcomes

`SecretRequestHeaders` is applied only while constructing the outgoing Wreq
request. It is not logged, serialized, or included in provenance. Live Wreq
responses have `ResponseOrigin::Network`, `BackendProvenance::Wreq`, and a
neutral `CrawlerBodyStream`. Wreq errors are translated at the leaf boundary
to `CrawlerFailure`; Page never owns a raw Wreq error.

## Execution identity and compatibility

Successfully resolved Website Wreq execution is `ExecutionMode::CanonicalWreq`.
`BackendProvenance::Wreq` continues to identify the backend. Public Client and
ClientBuilder aliases, raw Page constructors, raw fetch utilities, client
set/get/configuration APIs, robots methods accepting clients, and emulation
configuration surfaces remain `UPSTREAM_COMPATIBILITY_BOUNDARY`; they cannot
inject clients into the canonical executor.

## Rejections and exclusions

`transport_tor + wreq` remains fail closed. `cache_request + wreq`, `cache +
wreq`, and `cache_mem + wreq` remain rejected. Gemini remains separately
classified and is not migrated.

## Design selection and DONE

`TWO_BRANCH_NOT_APPLICABLE`: putting Wreq realization into the reqwest-specific
`ResolvedExecutor` would contaminate its concrete resource model, while keeping
it in Website would preserve parallel ownership. A separate leaf-owned
executor implementing the same request/outcome contract is the only design
that preserves both layering and public compatibility.

DONE requires canonical Website Wreq requests to use the persistent leaf
executor; canonical validation, redirect, proxy, configuration, secret, and
outcome invariants to be enforced; compatibility surfaces to remain isolated;
negative guardrails and supported feature checks to pass; and no Tor, cache,
Gemini, or broad compatibility redesign.
