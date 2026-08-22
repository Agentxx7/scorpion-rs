# Scorpion

Scorpion is an independent Rust crawling, web-research, evidence, and
intelligence engine. It combines Spider's mature crawling foundation with a
canonical evidence-first architecture, durable research sessions, and a
shipping `scorpion` command-line interface.

## Built on Spider

Scorpion is a fork of [Spider](https://github.com/spider-rs/spider), not a
replacement history for it. The inherited crawler, crate names, Rust import
paths, authorship, and MIT licensing remain attributed to the Spider project
and contributors. Existing Spider library and Spider Cloud integrations remain
available where their corresponding features are enabled.

The fork adds Scorpion-owned canonical capabilities while keeping inherited
Spider compatibility paths clearly separated from new product behavior.

## Current capabilities

- High-concurrency crawling and scraping inherited from Spider
- HTTP and browser-backed acquisition
- One-shot fetch with canonical evidence and provenance
- Feed, sitemap, news-sitemap, robots-sitemap, and web-search discovery
- Fail-closed Tor transport when the Tor feature is enabled
- Durable evidence records identified by `EvidenceId`
- Durable research sessions and results identified by `ResearchId`
- Reopening completed research in a later process
- Canonical Source-N-to-evidence citation bindings
- MCP access through the existing Spider MCP implementation
- Canonical watch identity, state, scheduled execution, change detection, and
  health/readiness primitives

The normal default build of `spider_cli` produces the `scorpion` binary and
includes the durable research feature. Custom `--no-default-features` builds
enable only the capabilities selected by the builder.

## Command-line interface

Build the shipping binary from this repository:

```sh
cargo build -p spider_cli
```

Inspect its commands with:

```sh
cargo run -p spider_cli -- --help
```

The CLI exposes crawling, scraping, fetch/discovery, search, MCP, transport,
authentication, and durable research surfaces according to the compiled
features. See [the CLI guide](./spider_cli/README.md) for command details.

## Durable research

Configure a durable database, SearXNG, and an OpenAI-compatible model endpoint,
then run:

```sh
scorpion research "How do Tokio and async-std compare for Rust async programming?"
```

The command prints a durable `ResearchId`, the final synthesis, and ordered
source bindings. Reopen that result later, without the search or model process:

```sh
scorpion research show research_00112233445566778899aabbccddeeff \
  --database /path/to/scorpion-research.sqlite
```

The durable lineage is:

```text
ResearchId
→ ResearchSession
→ DurableResearchResult
→ Source N + EvidenceRef
→ EvidenceId
→ EvidenceBundle
```

- `ResearchId` identifies one persisted research invocation.
- `EvidenceId` identifies one canonical durable source-evidence record.
- `Source N` is a presentation-local binding to an `EvidenceRef`; it is not an
  identity of its own.

Durable results retain source-grounded facts, missing-evidence statements,
extraction metadata, final synthesis, synthesis token usage, and citation
bindings. They do not provide deterministic replay.

## Architecture and verification model

Scorpion follows two connected principles: **one canonical engine with thin
interfaces**, and **evidence-first, proof-gated development**.

```text
CLI / MCP / library consumers
            ↓
    canonical capability seams
            ↓
research / discovery / acquisition / transport
            ↓
      evidence + provenance
            ↓
 identity / persistence / state
```

Core modules own behavior and state; interfaces call the canonical seams. The
CLI, for example, must not build its own research engine. An interface or test
must not introduce a parallel transport, evidence, identity, or persistence
implementation. Spider compatibility machinery may remain behind approved
boundaries, while new Scorpion development uses canonical Scorpion paths.

Durable research follows the single lineage shown above:

```text
ResearchId
→ ResearchSession
→ DurableResearchResult
→ Source N + EvidenceRef
→ EvidenceId
→ EvidenceBundle
```

Development proceeds by narrowing claims to the proof actually obtained:

```text
AUDIT REALITY
      ↓
DEFINE ONE FRONTIER
      ↓
TRACE CANONICAL PRODUCT PATH
      ↓
MINIMAL IMPLEMENTATION
      ↓
CODE_PROVEN
      ↓
CI_PROVEN
      ↓
OPERATOR_OBSERVED
      ↓
CLOSED
```

`OPERATOR_OBSERVED` is required only when meaningful and feasible and when the
capability declares it. The canonical proof classes are `CODE_PROVEN`
(source/static evidence and deterministic tests), `CI_PROVEN` (an identified
real CI execution), `OPERATOR_OBSERVED` (a concrete product-path observation),
`LIVE_ENVIRONMENT_DEPENDENT` (a declared external-environment dependency, not
an observation), and `UNPROVEN` (required proof does not yet exist).

Consequently, green tests do not mean `CLOSED`; a configured workflow is not
`CI_PROVEN`; successful CI is not `OPERATOR_OBSERVED`; and
`LIVE_ENVIRONMENT_DEPENDENT` does not mean live execution was observed.

## Architecture and process

- [Product contract](./SCORPION.md)
- [Canonical architecture and guardrails](./SCORPION_ARCHITECTURE.md)
- [Software design specification](./SCORPION_SDD.md)
- [Frontier process](./SCORPION_PROCESS.md)
- [Intelligent failure: AI, TDD, and false product confidence](./docs/INTELLIGENT_FAILURE.md)
- [Canonical closure and production-reality harness](./docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md)

These documents distinguish canonical Scorpion ownership from inherited
Spider compatibility code and future roadmap work.

## Current limitations and roadmap

The following remain separate work rather than completed product claims:

- Deterministic research replay and complete model/search request snapshots
- Background watch scheduler daemon operation
- Watch notifications
- Unified mixed clearweb/`.onion` research orchestration
- A stable public JSON contract for research CLI output

Some provider, browser, model, CAPTCHA, and transport paths are feature-gated
or require separate operator qualification. Consult the architecture inventory
before treating an internal seam as a shipping product capability.

## Upstream libraries and services

For the inherited Spider Rust API, packages, examples, and managed service:

- [Spider repository](https://github.com/spider-rs/spider)
- [Spider crate](https://crates.io/crates/spider)
- [Spider API documentation](https://docs.rs/spider)
- [Spider Cloud](https://spider.cloud)

Scorpion core remains independently self-hostable; Spider Cloud is an optional
inherited integration, not a requirement for canonical local research.

## License

This repository retains Spider's [MIT license](./LICENSE) and attribution.
Scorpion-specific additions are distributed under the same repository license.
