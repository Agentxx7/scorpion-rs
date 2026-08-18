# Canonical public surface ownership convergence

Frontier: `SCORPION_CANONICAL_PUBLIC_SURFACE_OWNERSHIP_CONVERGENCE_001`

Baseline: `6b32ae26`

## Purpose

Converge CLI/MCP/library surfaces onto the same canonical core ownership,
starting from a source-truth audit rather than an assumed finding.

## A. Audit findings

**MCP scrape path** (`spider_mcp/src/tools/scrape.rs`), the area
explicitly named for particular attention:

- **Content classification** — `route_auto_http`/`declared_mime`
  independently reimplemented byte-signature detection (`infer::get`)
  reconciled against a declared `Content-Type` header, producing a
  routing decision (`AutoRoute`). `spider` core's
  `utils/evidence.rs::build_evidence` already derives
  `detected_content_type` from the exact same `infer::get` call, and
  `infer` is already a `spider` core dependency (gated behind the
  `evidence` feature) — but the *reconciliation logic itself* (declared-
  header normalization; byte-signature-to-category mapping) existed only
  in `spider_mcp`, with no equivalent in `spider` core or `spider_cli`.
  **Classification: C — canonical core logic incorrectly owned by the
  interface.**
- **PDF extraction** (`extract_pdf_text`, via the `pdf-extract` crate) —
  a content *transformation* (raw PDF bytes → text), the same category
  `spider_transformations::transform_content_input` already occupies for
  HTML→Markdown/text conversion. Established, already-documented
  doctrine (`spider` core has no dependency on `spider_transformations`;
  that crate is interface-layer only) places content transformation in
  the interface layer deliberately. `pdf-extract` is not a `spider` core
  dependency, and no other interface (CLI) has an equivalent capability
  to deduplicate against. **Classification: B — legitimate adapter
  concern.** Not moved.
- **Scrape/result shaping** (`run()`'s response assembly: URL/status/
  content/links JSON, `EvidenceBundle` serialization, screenshot
  handling) — assembles already-canonical data (`Page`, `EvidenceBundle`
  via `build_evidence`) into this specific MCP tool's JSON response
  contract. **Classification: A — interface-only orchestration.** Not
  moved.

**Broader CLI/MCP sweep** (required item #1, beyond the named scrape
path): `spider_mcp/src/tools/{sitemap,feed,robots_sitemap,news_sitemap}.rs`
each have a `shape_result` function, and `search.rs`/`news_search.rs`/
`media_search.rs` have `render_results`/`render_video_results`/
`render_image_results` — every one of them calls straight into an
existing `spider::features::*` canonical parser/discovery function
(`sitemap::parse`, `feed::parse`, `robots_sitemap::discover`, search
providers) and only truncates/counts/reshapes the already-produced
result into that tool's response schema. **Classification: A** for all
of them — spot-checked `sitemap.rs::shape_result` in full; the pattern
is consistent across the others by inspection. `spider_cli/src/discovery.rs`
(1042 lines) contains exactly two top-level functions
(`fetch_document`/`run_fetch`), both thin wrappers around
`fetch_single_page_with_options`/`build_evidence` — no core-shaped logic
found there at all. `spider_cli/src/oauth.rs` constructs its own
`reqwest::Client` — reviewed and found out of scope: this is CLI login/
OAuth token-exchange plumbing, unrelated to crawl acquisition/transport
(explicitly excluded by this frontier's "do not rewrite transport/
acquisition"), not a duplicate of any canonical crawling capability.

No other production-logic duplication or canonical-core bypass was found
in this audit. The previously reported divergence (MCP scrape content
classification) was real and is now converged; PDF extraction and result
shaping were also flagged for attention and were found, on inspection, to
already be correctly interface-owned.

## B. Ownership classification

| Concern | Classification | Action |
|---|---|---|
| MIME classification (declared-header normalization + byte-signature category) | C | Moved to `spider::utils::evidence` |
| PDF text extraction | B | Left in `spider_mcp` |
| Auto-mode routing policy, error messages, undetermined-bytes textual heuristic | B | Left in `spider_mcp` (tool-contract policy) |
| Scrape response assembly (`run()`) | A | Left in `spider_mcp` |
| `shape_result`/`render_*` across sitemap/feed/robots_sitemap/news_sitemap/search/news_search/media_search | A | Left in `spider_mcp` |
| `spider_cli/discovery.rs` | A/thin already | No change |
| `spider_cli/oauth.rs`'s `reqwest::Client` | Out of scope (not crawl transport/acquisition) | No change |

## C. Logic moved to core

`spider/src/utils/evidence.rs` gains two new, narrowly-scoped, pure
primitives, placed directly beside the existing `detected_content_type`/
`build_evidence` ownership they extend:

- `pub fn declared_mime(content_type: Option<&str>) -> Option<String>` —
  normalizes a declared `Content-Type` header to its base MIME token
  (strip parameters, trim, lowercase). Byte-for-byte identical to MCP's
  former private helper of the same name.
- `pub fn classify_detected_content(bytes: &[u8]) -> Option<DetectedContentClass>` —
  classifies a byte-signature match (`infer::get`) into
  `DetectedContentClass::{Html,Xml,Pdf,Image,AudioVideo,UnclassifiedBinary}`,
  or `None` when no signature was found at all. This is deliberately
  scoped to *only* the self-contained, signature-only classification —
  not the declared-header fallback branching, JSON-parse-attempt, or
  safely-textual-bytes heuristic that follow it in MCP's own auto-mode
  policy, each of which is genuinely tool-contract-specific (a different
  interface could reasonably choose different fallback behavior) and
  remains MCP-owned.

11 new unit tests in `spider/src/utils/evidence.rs`'s own test module
cover both primitives directly.

## D. Interface paths after convergence

`spider_mcp/src/tools/scrape.rs::route_auto_http` no longer defines
`declared_mime` or its own byte-signature match; it calls
`spider::utils::evidence::{classify_detected_content, declared_mime}`
and keeps only the auto-mode *policy* on top (which `AutoRoute` variant
or `auto_error` message a given classification produces) — exactly the
same control flow and exact same error-message text as before, now
sourced from a canonical fact rather than a locally reimplemented one.
The function's public signature, `AutoRoute` enum, and every existing
test in `auto_router_tests` are unchanged and unmodified.

## E. Guardrails/tests

`spider/tests/architecture_guardrails.rs` — 3 new guardrails:
`content_classification_primitives_are_defined_exactly_once_in_canonical_core`,
`mcp_scrape_no_longer_reimplements_mime_classification` (proves the
canonical call sites are present and the old function/enum bodies are
gone from `scrape.rs`), `no_shadow_content_classification_anywhere_in_cli_or_mcp`
(a permanent guardrail against either crate regaining ownership of the
migrated logic, per requirement #7).

210/210 architecture guardrails pass. All existing `spider_mcp` scrape
tests pass unmodified: 37 passed (7 correctly `ignored` — localhost
socket/real-Chromium acceptance tests requiring loopback bind, the
existing, pre-established convention in this suite), 0 failed — including
every `auto_router_tests` case (`auto_routes_confident_html_to_markdown_path`,
`auto_formats_json_without_html_transformation`,
`declared_invalid_json_is_an_extraction_error`,
`auto_preserves_xml_instead_of_using_html_to_xml`,
`auto_rejects_png_mp4_and_unknown_binary`,
`auto_rejects_infer_known_zip_through_binary_catchall`,
`byte_signature_takes_precedence_without_mutating_declared_signal`,
`conservative_unknown_utf8_is_text_but_controls_are_undetermined`,
`historical_return_formats_and_unknown_fallback_are_unchanged`) and the
full `tools::scrape::tests` module (PDF extraction, evidence hashing,
Tor transport). `spider_mcp --lib` full suite: 138 passed, 7 ignored, 0
failed. `spider_cli` full test suite (46 + 2 + 24 = 72 tests across its
integration test binaries): all pass. `spider` default `--lib`: 755/755
pass. `cargo check --workspace` clean. `cargo fmt --check` clean.
`cargo clippy --lib --tests` (`spider`, `basic evidence disk cron`) and
`cargo clippy --lib` (`spider_mcp`) both produce only pre-existing,
unrelated baseline errors — confirmed identical via `git stash` for both
crates, none in `evidence.rs`, `scrape.rs`, or `architecture_guardrails.rs`.
`git diff --check` clean.

## F. Changed files

- `spider/src/utils/evidence.rs` — new `declared_mime`,
  `classify_detected_content`, `DetectedContentClass`; 11 new unit tests.
- `spider_mcp/src/tools/scrape.rs` — `declared_mime` removed;
  `route_auto_http`'s detected-signature branch now calls the canonical
  primitive. No test file changes — the existing suite is the
  regression proof.
- `spider/tests/architecture_guardrails.rs` — 3 new guardrails.
- `docs/frontier/CANONICAL_PUBLIC_SURFACE_OWNERSHIP_CONVERGENCE_SDD.md`
  — this document.

## Successor boundary

This frontier converges MIME classification only, per its own audit
findings — no other category-C logic was found. PDF extraction and all
result-shaping remain deliberately interface-owned. CLI does not
currently expose an equivalent to MCP's `return_format="auto"`; this
frontier does not add one (that would be new capability, not
convergence of existing duplicated logic) — a future CLI frontier could
now build such a feature on top of the same canonical
`classify_detected_content`/`declared_mime` primitives without
reimplementing them.
