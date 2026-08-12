# Cache-request transport convergence

Frontier: `SCORPION_CACHE_REQUEST_TRANSPORT_CONVERGENCE_001`

Baseline: `1484ca35522dad7942e3d712aff9435ce1cfb6c0`

Closed prerequisites: the canonical crawler response/error seam and the canonical crawler transport execution seam. A cache miss can therefore call `ResolvedExecutor::execute(CrawlerRequest)` without borrowing a client.

## Updated library audit

`reqwest_middleware`, `spider-http-cache-reqwest`, and `http-global-cache` form the old parallel transport stack. Their public integration builds and owns a `ClientWithMiddleware`, so it cannot remain on canonical Website execution. The lower-level `http-cache` crate exposes cache policy and manager primitives independently of reqwest middleware. Those primitives are reusable; its reqwest middleware adapter is not.

## Ownership and graph

Spider owns a cache executor above transport. It owns lookup, cache identity, RFC cacheability and validation metadata, expiration, materialization, storage, and hit reconstruction. `spider_transport::ResolvedExecutor` remains the only owner of target/onion validation, redirects and SSRF, proxy/Tor/TLS/DNS/interface policy, secret-header application, sessions, and network execution.

```text
Website / Page
  -> CanonicalCacheExecutor (when request caching is enabled)
       -> HIT: reconstructed CrawlerResponse
       -> MISS: ResolvedExecutor::execute(CrawlerRequest)
  -> Page / Spider retry and link policy
```

The cache executor never owns or exposes a raw HTTP client.

## Request identity and secrets

`CrawlerRequest` is the sole network request model. Cache identity is a cache-owned digest of namespace, method, and normalized URL only. It contains no header values, body, credentials, or debug rendering. Only GET and HEAD requests with no body are eligible. Any non-empty `SecretRequestHeaders`, or any ordinary `Authorization`, `Proxy-Authorization`, `Cookie`, or `Set-Cookie` request header, makes the request explicitly non-cacheable. Authorized cache options also bypass request caching. This conservative rule prevents secret material from entering keys, metadata, filenames, or logs.

## Hit, miss, body, and provenance

A miss invokes `ResolvedExecutor::execute` directly. Its `CrawlerFailure` is preserved as the network failure; cache code does not classify it again. The network body is explicitly materialized once because the selected cache managers persist complete bodies. The stored record contains only response status, public response headers, final URL, acquisition transport, cache policy metadata, and body bytes.

A hit reconstructs a new body stream from stored bytes and reports `ResponseOrigin::ReconstructedCache` and `BackendProvenance::CacheLayer`. `CacheLayer` means that the canonical cache layer reconstructed the response or produced the failure; it does not name a network backend and does not imply the removed reqwest-middleware transport architecture. It never claims network streaming. A miss reports the executor's `ResponseOrigin::Network` and network backend. Cache-manager or cache-policy failures use `CacheLayer` provenance and remain distinct from executor failures.

## Tor and failure behavior

Local cache hits are permitted under Tor and remain explicitly reconstructed-cache responses. A miss uses the already Tor-resolved executor. Cache code has no direct/default client and therefore cannot downgrade Tor. Invalid proxies and onion targets retain executor fail-closed behavior.

## Feature topology and manager disposition

`cache_request` selects the neutral request-cache executor and an in-memory manager. `cache_mem` retains that manager. `cache` selects the disk manager. If both storage features are unified, memory remains the deterministic selection to preserve the prior explicit precedence. The old `http-global-cache` global and the reqwest middleware adapter are removed; manager-cacache is enabled only for the winning disk-backed architecture, resolving rather than papering over the old bare-feature failure.

`wreq` is not converged here. `cache_request + wreq` is explicitly rejected from canonical execution; cache_request never makes wreq canonical. Artifact, search, provider, and worker compatibility paths are unchanged.

## Alternatives and two-branch decision

A lower-level `http-cache` adapter and a Spider-owned cache executor are not two independent architectures: the former supplies reusable policy/manager primitives inside the latter. The only other concrete design is the rejected reqwest-middleware transport owner. It violates the binding transport ownership. Therefore `TWO_BRANCH_NOT_APPLICABLE`; fabricating a second branch would compare the valid architecture with a known violation.

## Old-stack removal and DONE

Canonical cache_request builds must not contain cache-specific `ClientBuilder`, `ClientWithMiddleware`, `.send()`, redirect, proxy, TLS, timeout, default-header, or unsafe client construction. DONE means cache hits perform no network call, misses call `ResolvedExecutor`, origins and failures are truthful, secret-bearing requests bypass storage, Tor cannot downgrade, static negative fixtures reject reintroduction, supported feature builds and the regression suites pass, and no cache changes are committed or pushed by this frontier implementation turn.
