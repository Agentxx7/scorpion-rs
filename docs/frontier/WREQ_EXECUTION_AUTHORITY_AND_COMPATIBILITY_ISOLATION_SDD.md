# Wreq Execution Authority and Compatibility Isolation SDD

Frontier: `SCORPION_WREQ_EXECUTION_AUTHORITY_AND_COMPATIBILITY_ISOLATION_001`

## Status and authority

`wreq` remains an alternate, explicitly noncanonical Spider HTTP implementation. A build with the feature resolves ordinary Website execution to `ExecutionMode::NoncanonicalWreq` before setup, robots, sitemap, or Page acquisition. It never resolves to `Canonical` and never receives a `ResolvedExecutor`.

Canonical Scorpion capabilities do not select wreq: evidence and Tor fail closed; artifact and canonical source providers are unavailable; search retains `spider_transport`; cache combinations remain compile-time rejected; CLI, MCP, and agent manifests do not enable it.

## Website entry points

`crawl`, `crawl_raw`, `crawl_smart`, `crawl_sitemap`, and `crawl_sitemap_chrome` are explicit noncanonical wreq execution when compiled with wreq. `scrape*` delegates to those modes. `crawl_raw_send`, `crawl_chrome_send`, `fetch_chrome`, and caller-driven configure/setup/client operations are upstream compatibility APIs and do not claim canonical authority. Robots, sitemap, and conditional requests reached by the noncanonical Website mode inherit that identity; their standalone raw-client APIs remain compatibility-only.

## Compatibility boundary

The public `Client`/`ClientBuilder` aliases and client module exports, wreq emulation configuration, Website raw-client configuration/set/get APIs, raw Page constructors, raw fetch utilities, and robots methods accepting a client are `UPSTREAM_COMPATIBILITY_BOUNDARY`. Their existence does not authorize canonical Scorpion callers to use them.

## Provenance and errors

Every live wreq response is network-originated and carries `BackendProvenance::Wreq` into Page. Wreq errors are translated to neutral `CrawlerFailure` facts with the same backend. This translation does not make execution canonical. Wreq sitemap execution is wreq provenance, not upstream-compatibility provenance when invoked by the explicit Website mode.

## Security limitations

This frontier does not claim parity with canonical target/onion validation, redirect/SSRF ownership, proxy validation, TLS, DNS/interface, cookies, or `SecretRequestHeaders`. The existing Website redirect checks and configuration remain noncanonical wreq behavior. Invalid proxy skipping remains architecture debt and cannot be described as canonical. Canonical callers reject wreq before network execution.

## Cache and Tor

`cache_request + wreq`, `cache + wreq`, and `cache_mem + wreq` remain compile-time rejected. `transport_tor + wreq` remains fail closed with no fallback. Cache misses remain `CanonicalCacheExecutor -> ResolvedExecutor`.

## Gemini

The Gemini solver's direct wreq client is a capability-local explicit noncanonical transport, separate from Website compatibility. It is guarded as such here; converging or replacing it requires a separate capability transport frontier.

## Public API and future decision

No broad public wreq API is removed. A future audit may choose canonical backend convergence, continued noncanonical isolation, or removal, but only after security and compatibility evaluation.

## Done

Execution authority resolves before requests; canonical mode cannot become wreq; wreq provenance is truthful; canonical callers and unsupported combinations fail closed; compatibility surfaces are explicit; Gemini is separately classified; negative guardrails prevent regression; supported feature checks pass.
