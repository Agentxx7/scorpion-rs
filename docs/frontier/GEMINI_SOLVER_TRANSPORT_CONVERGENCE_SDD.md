# Gemini Solver Transport Convergence SDD

Frontier: `SCORPION_GEMINI_SOLVER_TRANSPORT_CONVERGENCE_001`

Baseline: `76f8e5eb75b877f83e132ea44ba96ecf9d016c08`

## Decision

The external Gemini captcha solver is domain policy above the canonical
transport seam. It is not a capability-local transport backend. One lazy,
persistent, feature-selected `CanonicalExecutor` owns every solver network
operation. The executor is `ResolvedExecutor` in reqwest builds and
`ResolvedWreqExecutor` in wreq builds; neither raw client is exposed.

The separate `gemini-rust` chat/client capability is outside this frontier and
is unchanged.

## Ownership and graph

The solver owns prompts, provider payloads, endpoint composition, JSON parsing,
semaphore/rate policy, per-operation and overall deadlines, and captcha-domain
skip/failure decisions.

`spider_transport` owns request execution, initial target and onion validation,
redirect/SSRF policy, proxy/TLS/DNS/interface policy, transport timeouts,
secret-header application, pooled backend resources, and neutral response and
failure facts.

```text
Gemini solver policy
  -> CrawlerRequest
  -> feature-selected CanonicalExecutor
  -> canonical validation and redirect/SSRF policy
  -> private Reqwest or Wreq client/session
  -> CrawlerResponse / CrawlerFailure
  -> Gemini parsing and captcha decision
```

## Requests and authentication

Challenge-controlled image URLs are parsed into `Url` and submitted as GET
`CrawlerRequest`s. Vision, slider, and captcha provider calls serialize their
existing JSON payload and submit POST `CrawlerRequest`s. No second request or
response model exists.

Endpoint URLs never contain API credentials. `x-goog-api-key` is inserted into
`SecretRequestHeaders`, whose values are sensitive, non-serializable, and
redacted from diagnostics. The executor applies those headers only at backend
request construction.

## Outcomes and policy

Live responses retain `ResponseOrigin::Network` and the actual
`BackendProvenance::Reqwest` or `BackendProvenance::Wreq`. Bodies are consumed
from `CrawlerBodyStream`. Transport and body-stream failures remain
`CrawlerFailure` until the solver boundary explicitly propagates them or maps
them to its established skip result. Structured kind, backend, and status facts
are retained in secret-free diagnostics for skip decisions.

The existing semaphore and Tokio deadlines stay above the executor. The
executor also applies its canonical 20-second transport timeout. Persistent
executor state preserves pooling and any backend session continuity.

## Security and configuration

Both feature-selected executors use `TransportPolicy::Default`, canonical
target/onion checks, the canonical redirect/SSRF decision, verified TLS, and no
configured proxy, custom DNS resolver, interface, local address, or cookie jar.
Those are explicit solver transport settings, not silent alternate-client
defaults. Any future non-default setting must be represented by the canonical
configuration and resolved before use; a capability-local fallback is
forbidden.

## Done

- all three external solver flow families use `CrawlerRequest` and the shared
  persistent executor;
- raw `GEMINI_CLIENT`, direct sends, and query-string credentials are absent;
- neutral outcome/provenance and solver-owned policy are preserved;
- guardrails reject raw Gemini client construction/execution and secret URLs;
- relevant reqwest and wreq builds, tests, lint, format, and diff checks pass.
