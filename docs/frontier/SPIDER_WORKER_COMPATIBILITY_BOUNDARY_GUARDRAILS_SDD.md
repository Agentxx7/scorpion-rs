# Spider Worker Compatibility Boundary Guardrails SDD

Frontier: `SCORPION_SPIDER_WORKER_COMPATIBILITY_BOUNDARY_GUARDRAILS_001`

Baseline: `1f570fa78768614e39611d40fbd9f67e47a47d4e`

## Classification

`spider_worker` is an `UPSTREAM_COMPATIBILITY_BOUNDARY`: a terminal executable
implementing the external protocol used only by Spider's explicitly enabled
`decentralized` mode. It owns no canonical Scorpion capability.

## Permitted graph

`spider_worker -> spider -> spider_transport` is permitted. At runtime, Spider
decentralized mode may call the external worker protocol. The worker may retain
exactly these upstream acquisition primitives: `Website::configure_http_client`,
`Page::new_page_streaming`, and `fetch_page_html_raw`.

## Prohibited graph and ownership

No workspace capability/library crate may depend on `spider_worker`. Canonical
transport, acquisition, search, evidence, discovery/source, artifact, agent,
watch/state, or job capabilities may not select the worker as alternate or
fallback execution. The worker may not define those canonical seams or models.

## SSRF classification

`target_host_blocked` is `COMPATIBILITY_LOCAL_DEFENSE`. It is private, incomplete
by design, and neither exported nor equivalent to canonical `spider_transport`
SSRF enforcement. Canonical modules may not import it.

## Tor isolation

Spider's Tor preflight must continue rejecting `decentralized`; the worker is
never a Tor fallback or bypass.

## Enforcement and DONE

Architecture tests scan workspace manifests, canonical sources, worker source,
the exact primitive allowlist, SSRF visibility/imports, and Tor rejection. They
include synthetic negative cases for every requested violation class. No
production behavior changes. All focused and existing guardrails, worker and
decentralized checks, clippy, rustfmt, and diff checks pass.

## Two-branch applicability

`TWO_BRANCH_NOT_APPLICABLE`: a separate policy registry would duplicate facts
already machine-readable in manifests and source. Direct manifest/source
guardrails are the sole minimal enforcement architecture.
