# Canonical evidence provenance surfacing

Frontier: `SCORPION_CANONICAL_EVIDENCE_PROVENANCE_SURFACING_001`

Baseline: `d042848a`

## Purpose

Ensure provenance already captured by Scorpion is preserved and exposed
through the primary canonical crawl/scrape result paths instead of being
silently dropped.

## A. Provenance audit findings

Traced the full chain: acquisition/transport stamps `Page`'s `pub(crate)`
fields (`transport`, `backend`, `response_origin`) and
`observed_status_code` (a plain `pub` field) — all already exposed via
public accessors (`Page::transport()`/`backend_provenance()`/
`response_origin()`, `Page::observed_status_code`). `utils/evidence.rs`'s
`build_evidence` already reads every one of these into `EvidenceBundle`
when a caller opts into evidence mode. The break was downstream of
`Page`, not upstream of it: every **default** (non-evidence-opt-in)
crawl/scrape result path independently constructed its own JSON/struct
shape from a `Page` it already held, and none of them read these already-
public fields at all.

Confirmed by direct inspection with particular attention to the named
fields:

- **Backend provenance** (`Page::backend_provenance()`) — captured,
  never read by any default-path output.
- **Response origin** (`Page::response_origin()`) — captured, never read.
- **Transport provenance** (`Page::transport()`) — captured, never read.
- **DNS provenance** (derived label, `"proxy"` under Tor) — captured
  (derivable), never read.
- **Failure/degraded-status provenance** (`Page::observed_status_code`,
  the HTTP status actually observed independent of Spider's effective/
  retry-reclassified `status_code`) — captured, never read; every default
  path surfaced only `status_code`, silently hiding a truthful signal
  that a page's status was reclassified.

## B. Where provenance was previously lost

| Path | Shape before | Provenance present? |
|---|---|---|
| `spider_mcp` scrape tool, default (non-`evidence`) mode | `{url, status_code, content, links}` | No |
| `spider_mcp` crawl tool, inline (≤10 pages) | `{url, status_code, content, links}` | No |
| `spider_mcp` crawl tool, background (`CrawlPageResult`) | `{url, content, status_code, links}` | No |
| `spider_cli scrape` | `{url, content, links, headers?, duration_elapsed_ms?, status_code?, remote_address?}` | No |
| `spider_cli fetch` (`run_fetch`, full `EvidenceBundle`) | Full `EvidenceBundle` | Already present (unaffected) |
| `spider_mcp` scrape tool, `evidence: true` mode | Full `EvidenceBundle` | Already present (unaffected) |

`spider_cli crawl`'s plain URL/header text-stream output was inspected
and excluded: it is not a JSON result path at all (no content/evidence
shape to enrich the same way), so it is out of scope here — enriching it
would mean redesigning that command's output format, not surfacing
already-captured facts through an existing shape.

## C. Canonical surfacing path

Reused, never duplicated: `spider::utils::evidence::page_provenance(page: &Page) -> PageProvenance`
was factored directly out of `build_evidence`'s own provenance-field
derivation — the exact same code, not a reimplementation.
`build_evidence` itself now calls `page_provenance` for its `transport`/
`dns`/`backend_provenance`/`response_origin`/`observed_status_code`
fields, so there is structurally one source of truth: any future change
to how a field is derived changes both call sites at once, and
`build_evidence_and_page_provenance_agree_exactly` (unit test) proves
they can never silently drift apart.

`spider_mcp`/`spider_cli` call `page_provenance(&page)` directly at each
of the four previously-lossy sites — no interface reimplements label
derivation, transport-to-DNS inference, or any other provenance logic.

## D. Fields surfaced

`PageProvenance { transport, dns, backend_provenance, response_origin, observed_status_code }`
— field names and value vocabulary identical to `EvidenceBundle`'s own
(`"default"`/`"tor"`; `"reqwest"`/`"wreq"`/`"cache_layer"`/
`"noncanonical_fetch_engine"`/`"noncanonical_remote_fetcher"`/
`"upstream_compatibility"`; `"network"`/`"reconstructed_cache"`/
`"synthetic"`; `"proxy"`/`None`). Now present in:

- MCP scrape (default mode): new `"provenance"` key alongside
  `url`/`status_code`/`content`/`links`.
- MCP crawl (inline): same `"provenance"` key added to each page object.
- MCP crawl (background): `CrawlPageResult` gained a `provenance:
  PageProvenance` field (serialized the same way as its siblings).
- CLI scrape: new `handle_provenance` step (mirroring the existing
  `handle_time`/`handle_status_code`/`handle_remote_address` incremental-
  JSON-field convention exactly), gated on the `fetch` CLI feature (the
  same feature that already enables `spider/evidence`) — a page built
  without that feature simply has no `"provenance"` key, never a
  fabricated one.

## E. Unknown/failure semantics

Every `PageProvenance` field is `Option` and reads straight through
`Page`'s existing accessors — an acquisition path this frontier's scope
never stamped (cache hits, decentralized crawls, or any other
unaudited path) still produces `None`, exactly as `EvidenceBundle`
already guarantees; nothing here invents, infers, or backfills a value.
Proven directly: `unstamped_page_reports_all_provenance_as_none` (a
freshly-`Default`-constructed `Page` yields `PageProvenance::default()`
— all `None`) and `observed_status_code_survives_effective_reclassification`
(a page whose Spider-effective `status_code` differs from what was
actually observed still reports the true observed value, the concrete
"failure/degraded provenance" case named in scope).

## Not implemented here

Per this frontier's explicit scope: `EvidenceBundle` itself is
unmodified (only its internal derivation was refactored to share code,
not its shape or field set); no transport/CAPTCHA/Watch changes; no
generic metadata framework; no new provenance values invented; CLI
`crawl`'s text-stream output is not restructured.

## F. Guardrails/tests

`spider/tests/architecture_guardrails.rs` — 4 new guardrails:
`page_provenance_primitives_are_defined_exactly_once_in_canonical_core`,
`build_evidence_reuses_page_provenance_as_the_single_source_of_truth`,
`primary_crawl_and_scrape_paths_surface_page_provenance` (the
reachability proof — all four previously-lossy sites), and
`no_shadow_provenance_model_in_cli_or_mcp`.

214/214 architecture guardrails pass. 6 new `spider` unit tests
(`page_provenance_tests`) plus the existing 21 `utils::evidence` tests
all pass (27/27). All existing `spider_mcp` tests pass unmodified except
two required, minimal fixture-construction updates
(`crawl_status.rs`'s test now supplies `PageProvenance::default()` for
its hand-built `CrawlPageResult`) — 138 passed, 7 ignored (pre-existing
localhost/Chromium acceptance convention), 0 failed. `spider_cli`'s full
suite (72 pre-existing + 1 new `scrape_output_surfaces_page_provenance`
= 73 tests) passes. `cargo check --workspace` clean; `cargo fmt --check`
clean; `cargo clippy` clean for `spider` (`basic evidence disk cron`),
`spider_mcp`, and `spider_cli` (only the known pre-existing, unrelated
`spider`-lib baseline errors, none in any changed file); `git diff
--check` clean.

## Acceptance summary / closure

The audit's own required framing — "if the reported provenance loss no
longer exists, report that truthfully instead of manufacturing changes"
— does not apply here: the loss was real, confirmed by direct inspection
of all four primary result paths, and is now closed for exactly the
fields this frontier named (backend/response-origin/transport/DNS/
failure provenance), through one canonical, single-source-of-truth
primitive.

## G. Changed files

- `spider/src/utils/evidence.rs` — new `PageProvenance`/`page_provenance`;
  `build_evidence` refactored to call it; 6 new unit tests.
- `spider_mcp/src/tools/scrape.rs` — default-mode output gains
  `"provenance"`.
- `spider_mcp/src/tools/crawl.rs` — inline and background paths gain
  provenance.
- `spider_mcp/src/state.rs` — `CrawlPageResult` gains a `provenance` field.
- `spider_mcp/src/tools/crawl_status.rs` — test fixture updated for the
  new `CrawlPageResult` field.
- `spider_cli/src/main.rs` — new `handle_provenance`, wired into the
  `scrape` command's output assembly.
- `spider_cli/tests/transport_cli.rs` — new
  `scrape_output_surfaces_page_provenance` acceptance test.
- `spider/tests/architecture_guardrails.rs` — 4 new guardrails.
- `docs/frontier/CANONICAL_EVIDENCE_PROVENANCE_SURFACING_SDD.md` — this
  document.
