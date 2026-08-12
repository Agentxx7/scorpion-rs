# Canonical crawler response/error seam

Frontier: `SCORPION_CANONICAL_CRAWLER_RESPONSE_ERROR_SEAM_001`

Baseline: `387364bdf307e5cc4ea8dc05dd5aaf632fe03ebc`

## Ownership

`spider_transport` owns the backend-neutral facts produced by request
execution. Spider owns the policy that maps those facts to synthetic Page
status, retry, backoff, or abandonment. No retry table lives in the transport
leaf.

## Failure vocabulary

`CrawlerFailure` contains a `CrawlerFailureKind`, optional observed HTTP
status, backend provenance, and an optional type-erased `Arc<dyn Error + Send
+ Sync>`. The variants are timeout, DNS, TLS handshake, proxy tunnel,
connection refused/aborted/reset/unreachable/other, request, body stream,
decode, HTTP status, and other. Adapters classify backend facts once. Crawler
policy matches only this vocabulary.

The source is available through `Error::source` and an explicit borrowed
accessor. Debug output identifies kind/backend but never stringifies the source
implicitly. Backend-specific diagnostics remain reachable without making
their concrete types canonical.

Typed backend predicates and typed source-chain values take precedence.
Bounded phrase recognition is permitted only where a backend exposes no typed
fact (currently selected TLS/proxy/DNS inner errors); it is confined to the
adapter and never becomes crawler policy.

## Response vocabulary

`CrawlerResponse` contains HTTP status, headers, final URL, response origin,
transport provenance, backend provenance, and a type-erased byte stream. A
network adapter transfers the original body stream without collecting it.
Each stream failure is translated to `CrawlerFailureKind::BodyStream` while
retaining its source.

Origins distinguish `Network`, `ReconstructedCache`, and `Synthetic`. A cache
response may implement the same stream interface over reconstructed bytes but
never claims network provenance.

## Backend adapters

The reqwest adapter lives in `spider_transport` because reqwest is the
canonical transport implementation detail. Wreq and cache adapters, when
compiled, live at their compatibility boundary in Spider and translate into
the same vocabulary. They do not define variants or retry policy. This
frontier neither converges nor repairs those stacks.

## Page integration

`PageResponse.error_for_status` becomes `Option<CrawlerFailure>`; successful
response transport is represented separately. `page_error_status_details`
stores `Arc<CrawlerFailure>`, not `Arc<reqwest::Error>`. Existing string detail
mode formats the neutral failure.

Spider provides exactly one mapping from `CrawlerFailureKind` plus observed
status to its existing synthetic statuses. Existing `is_retryable_status` and
retry/backoff machinery remain authoritative and unchanged.

Raw-client compatibility functions may adapt their backend errors at their
boundary. Canonical Page-facing functions never accept backend error types.

## DONE

The seam is complete when response/error vocabulary has one owner, reqwest
errors translate without loss of typed facts/source, Page status-detail and
retry policy consume only neutral failures, network bodies remain streaming,
cache reconstruction has distinct provenance, wreq/cache remain noncanonical,
guardrails reject backend leakage, and the scoped regression matrix passes.
