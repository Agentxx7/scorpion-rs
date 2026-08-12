# Canonical crawler transport execution seam

Frontier: `SCORPION_CANONICAL_CRAWLER_TRANSPORT_EXECUTION_SEAM_001`

Baseline and closed response/error prerequisite:
`783de145f609b0b527887203a2e521651a3c2bd2`.

## Recovery history

The blocked evidence commit `3c203fb281b0f60d67c7e06e89e0d4c284892136`
was based on `387364bdf307e5cc4ea8dc05dd5aaf632fe03ebc`. Its persistent
executor, request-only input, fail-closed proxy resolution, and private pool
were useful implementation evidence. Its `CrawlerExecutionError`, raw-response
`CrawlerResponse`, and separate `ExecutionProvenance` response wrapper were
superseded when the response/error prerequisite closed. They are not replayed.

## Decision

Canonical crawler traffic resolves one persistent, leaf-owned
`spider_transport::ResolvedExecutor` before target execution. The executor owns
all reusable sessions and network facts. It accepts `CrawlerRequest` data and
returns only the closed neutral `CrawlerResponse` or `CrawlerFailure` seam:

```text
Website crawler policy
  -> CrawlerTransportConfiguration
  -> spider_transport::ResolvedExecutor
  -> canonical request execution
  -> CrawlerResponse / CrawlerFailure
  -> Page / Spider retry and crawl policy
```

`CrawlerResponse`, `CrawlerBodyStream`, `CrawlerFailure`,
`CrawlerFailureKind`, `BackendProvenance`, and `ResponseOrigin` have exactly one
owner: `spider_transport::crawler_outcome`. The executor translates reqwest
metadata and body failures at that boundary. Spider owns status synthesis,
retry classification, backoff, parsing, link policy, and body materialization.

## Executor ownership

`ResolvedExecutor` owns its transport policy, private client/session pool,
round-robin proxy realization, target/onion validation, redirect and SSRF
policy, TLS certificate policy, total/connect/read timeouts, local-address
binding, cookie jars, default headers, `SecretRequestHeaders`, request
construction, execution, and neutral outcome translation. Explicit invalid
proxies fail resolution; Tor plus crawler proxies fails closed. No raw client
accessor exists.

DNS overrides not representable by the leaf configuration are rejected at the
migration boundary rather than silently ignored. Cookie jars are shared by all
clients in a resolved pool, preserving session continuity across rotation.

## Modes and compatibility boundary

Execution authority is fixed before crawl execution: `Canonical`,
`NoncanonicalHttpFetchEngine`, `NoncanonicalRemoteFetcher`, or
`UpstreamCompatibility`. Engine `should_fetch` decisions remain within the
explicit noncanonical engine mode; they do not give the crawl canonical-only
provenance. Tor rejects noncanonical modes rather than falling back.

Public raw-client Page APIs and `Website::set_http_client` may remain only as
an `EXPLICIT_UPSTREAM_COMPATIBILITY_BOUNDARY` for approved consumers such as
`spider_worker`. Canonical Website, evidence, and Scorpion capability code may
not call them. `get_client`, `Crawler::client`, `ClientRotator`, and
`secondary_http_client_for` are legacy APIs outside the canonical graph.

## Migration and cache boundary

The migration boundary is the canonical Website/Page request graph plus Tor
evidence acquisition. `wreq`, `cache_request`, `HttpFetchEngine`, and
`RemoteFetcher` remain explicitly noncanonical. This frontier does not converge wreq or caching.
A future cache miss must invoke `ResolvedExecutor::execute`
with a `CrawlerRequest`; it must not borrow a client or duplicate redirect,
proxy, TLS, or backend-error behavior.

`TWO_BRANCH_NOT_APPLICABLE` remains valid: closure of the response/error seam
removes duplicate outcome designs but does not create a second valid executor
architecture.
