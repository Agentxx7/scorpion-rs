# Scorpion

Scorpion is a local, evidence-first **web acquisition**, **crawling/scraping**,
**audit and evidence**, **research**, and **IAM/federation diagnostics**
toolkit, built on [Spider](https://github.com/spider-rs/spider). One
canonical engine is exposed through three real interfaces — a **Windows Web
Console**, a **CLI**, and an **MCP server** — so an AI assistant, a script,
and a person clicking a button all resolve through the exact same
crawl/audit/evidence/IAM code, never a duplicated implementation.

Scorpion is a fork of `spider-rs/spider`. Inherited crawler code, crate names,
imports, authorship, and MIT licensing remain attributed to Spider and its
contributors. Scorpion adds its canonical evidence, provenance, identity,
research, and IAM diagnostics capabilities on top; it does not claim to have
created the inherited crawler.

## What Scorpion does

| Facet | What it means today |
|---|---|
| **Web acquisition toolkit** | Fetch exactly one resource with full retrieval evidence (Web, CLI, MCP) |
| **Crawling/scraping toolkit** | Crawl/scrape/download a site to markdown/text/HTML (CLI, MCP) |
| **Audit/evidence toolkit** | Deterministic, rule-based page audit plus durable evidence storage and inspection (Web, CLI, MCP) |
| **Research platform** | Search → acquire → synthesize → durable, citable result, once you configure a search backend and an LLM (Web, CLI) |
| **IAM/federation diagnostics tool** | Receive a real OAuth2/OIDC/SAML callback and inspect its decoded structure — not a protocol implementation, not a signature verifier (Web only) |

No single interface exposes every facet — see [Current interfaces](#current-interfaces)
and the [capability overview](#capability-overview) for exactly what each one
does and doesn't do.

## Recommended setup

Pick the section below that matches what you want to do first. All of these
can be combined later — they are not mutually exclusive installs.

### A. IAM troubleshooting

1. Download the Windows installer: `scorpion_app-2.53.4-x86_64.msi`
   (see [Windows download](#windows-download)).
2. Verify its SHA-256: `8578692a99c7317448d14b64820786b826f28e23305b1c509e4e30ec6f2f59c6`.
3. Install Scorpion.
4. Launch **Scorpion** from the Start Menu.
5. Your browser opens automatically at `http://127.0.0.1:8787/`.
6. Open **IAM Callback Inspector**.

**No LLM, no Qwen (or any other) model, no llama-server, and no SearXNG are
required for IAM callback inspection.** See [IAM Callback Inspector](#iam-callback-inspector)
for the full operator guide.

### B. Web / integration toolkit

Install: the **Scorpion Windows MSI**. Nothing else.

Available immediately, with no LLM and no search backend configured:

- Web Console home page
- Fetch (single-resource acquisition with evidence)
- Page Audit (deterministic rule-based audit)
- Evidence Inspector
- IAM Callback Inspector

The Web Console does **not** expose general crawl/scrape/download commands —
that needs the CLI (below).

### C. CLI toolkit

For crawl, scrape, download, audit, artifact discovery/download, and
transport options (e.g. Tor), use the `scorpion` CLI binary. There is no
downloadable CLI installer — build/install it from the workspace:

```sh
cargo install spider_cli
```

```sh
scorpion --url https://example.com crawl --limit 1
scorpion --url https://example.com scrape
scorpion fetch https://example.com
scorpion audit https://example.com
```

Full command reference: [Installation and Runtime Guide § CLI setup](./docs/INSTALLATION_AND_RUNTIME_GUIDE.md#9-cli-setup)
and [spider_cli/README.md](./spider_cli/README.md).

### D. MCP integration

For an MCP-capable AI client (Claude Code, Claude Desktop, etc.) to call
Scorpion's crawl/scrape/audit/evidence tools directly:

```sh
cargo install spider_mcp
spider-mcp
```

or, the same server implementation launched through the CLI instead:

```sh
scorpion mcp
```

14 tools are exposed (`spider_scrape`, `spider_crawl`, `spider_audit_page`,
`spider_evidence_read`, and more) — see [MCP setup](./docs/INSTALLATION_AND_RUNTIME_GUIDE.md#8-mcp-setup)
for the full list and a client configuration example.

**The IAM Callback Inspector is not an MCP surface**, and **durable
Research's search → synthesis pipeline is not an MCP surface** either — MCP
exposes deterministic, single-call tools only.

### E. Full local AI / Research stack

Recommended combination:

```text
Scorpion + an OpenAI-compatible local LLM server + a local model + SearXNG
```

Install/configure, in order:

1. Scorpion (Windows MSI and/or CLI, for `scorpion research`)
2. A local OpenAI-compatible LLM runtime (e.g. `llama-server`) — manual
   external setup, not installed by Scorpion
3. A model file for that runtime — manual external setup
4. SearXNG — manual external setup (Docker recommended), not bundled

See [Local AI / LLM setup](#local-ai--llm-setup) and [Search / SearXNG](#search--searxng)
below for the exact configuration contract.

### Recommended install profiles

| Goal | Install |
|---|---|
| IAM troubleshooting | Scorpion Windows MSI |
| Fetch / Audit / Evidence | Scorpion Windows MSI |
| Crawl / Scrape / Download | Scorpion CLI (`cargo install spider_cli`) |
| MCP integration | `spider-mcp` (or `scorpion mcp`) |
| Local AI Research | Scorpion + LLM runtime + model + SearXNG |
| Development | Git + Rust toolchain + this repository |

## Windows download

| | |
|---|---|
| Workflow | `Windows Package` (`.github/workflows/windows-package.yml`) |
| Example run | [33972458053](https://github.com/Agentxx7/scorpion-rs/actions/runs/33972458053) |
| Artifact | `scorpion-windows-installer` |
| Installer | `scorpion_app-2.53.4-x86_64.msi` |
| SHA-256 | `8578692a99c7317448d14b64820786b826f28e23305b1c509e4e30ec6f2f59c6` |
| Status | `TESTED / ARTIFACT_BUILT` — **PENDING OWNER WINDOWS INSTALL** |

Download the artifact from the workflow run, verify the hash yourself
(`certutil -hashfile scorpion_app-2.53.4-x86_64.msi SHA256` on Windows)
before installing.

**The MSI installs:**
- `scorpion-api.exe`
- `scorpion-launcher.exe`
- a Start Menu shortcut
- a Desktop shortcut

**The MSI does NOT install:**
- Qwen (or any other) model weights
- `llama-server`
- the llama.cpp runtime
- SearXNG
- the CLI binary (`scorpion`)
- the MCP binary (`spider-mcp`)

Full download/verify/install/uninstall walkthrough, application-data paths,
and troubleshooting: [Installation and Runtime Guide § Windows quick start](./docs/INSTALLATION_AND_RUNTIME_GUIDE.md#2-windows-quick-start).
This README intentionally does not duplicate that detail.

## Current interfaces

Scorpion now has a **local, operator-facing** Web Console — this is not the
same claim as being a public, Google-like hosted search service, and it
never will be one. The Web Console (served by `scorpion-api` at
`http://127.0.0.1:8787/`) currently contains:

- **Search** — a plain search box over an operator-provided SearXNG instance
- **Research** — durable, citable synthesis (needs LLM + search configured)
- **Evidence Inspector** — read back exactly what Audit/Fetch/Research recorded
- **Fetch** — one URL, HTTP(S) only, no crawl following
- **Page Audit** — the same deterministic rule engine the CLI/MCP audit
  commands call
- **IAM Callback Inspector** — see below

The **CLI** (`scorpion`) additionally exposes crawl, scrape, download, feed/
sitemap/robots/news-sitemap discovery, artifact discovery/download, and Tor
transport options — none of which the Web Console exposes.

The **MCP server** (`spider-mcp` / `scorpion mcp`) exposes 14 deterministic,
single-call tools mirroring most of the CLI's read/acquisition surface, but
not durable Research's synthesis pipeline and not IAM.

No single interface exposes every capability — see the
[capability overview](#capability-overview).

## IAM Callback Inspector

The current shipped workflow:

```text
New Trace → copy the callback URI → point your IdP/application's callback at it
          → receive the real callback → inspect the decoded timeline
```

Supported structural inspection:

- generic HTTP `GET`/`POST` callbacks
- OAuth2 (`code`, `error`, `state`)
- OIDC (`id_token` presence)
- JWT structural decode (header + payload claims)
- SAML 2.0 HTTP-POST binding Response/Assertion structural decode

Every decoded fact carries one of four statuses:

```text
OBSERVED · VALIDATED · NOT VALIDATED · REDACTED
```

`VALIDATED` is reserved for a future capability — no code path currently
constructs it for any signature or token-authenticity claim. **Decoded does
not mean verified.** Current limitations, stated explicitly:

| Capability | Status |
|---|---|
| JWT cryptographic signature verification | **NOT IMPLEMENTED** |
| SAML XML-DSig verification | **NOT IMPLEMENTED** |
| `state` expectation comparison | **NOT IMPLEMENTED** |
| `nonce` expectation comparison | **NOT IMPLEMENTED** |
| `InResponseTo` expectation comparison | **NOT IMPLEMENTED** |
| Public callback relay/tunnel | **NOT IMPLEMENTED** |
| Real IdP interoperability | **NOT YET LIVE/OWNER VERIFIED** |

Full operator guide, five worked troubleshooting examples, and the complete
fact/status vocabulary: [Installation and Runtime Guide §§10–11](./docs/INSTALLATION_AND_RUNTIME_GUIDE.md#10-iam-callback-inspector).

## Local AI / LLM setup

**Scorpion itself is not an LLM.** Durable Research's synthesis step talks to
a separate, operator-configured server over the standard OpenAI Chat
Completions HTTP API:

```sh
OPENAI_COMPAT_BASE_URL=http://127.0.0.1:8081/v1
OPENAI_COMPAT_MODEL=<your-model>
OPENAI_COMPAT_API_KEY=local
```

Optionally: `OPENAI_COMPAT_TIMEOUT_SECS`.

`8081` above is only an example local port, not a Scorpion default — Scorpion
defines no default for `OPENAI_COMPAT_BASE_URL`; all three required
variables must be set explicitly, or Research stays in its existing truthful
"not configured" state.

Getting a local model running is **manual external setup** — Scorpion does
not choose, bundle, or download one for you:

1. Choose a compatible model (Qwen is one example among many that work with
   an OpenAI-Chat-Completions-shaped local server — it is not a requirement).
2. Download that model's weights yourself.
3. Install/run an OpenAI-compatible runtime such as `llama-server`.
4. Confirm it exposes a working `/v1` endpoint.
5. Configure Scorpion with the three variables above.

Full walkthrough: [Installation and Runtime Guide §§4–5](./docs/INSTALLATION_AND_RUNTIME_GUIDE.md#4-local-llm-stack).

## Search / SearXNG

```sh
SEARXNG_BASE_URL=http://127.0.0.1:8080
```

`8080` above is only an example local setup — Scorpion defines no default for
`SEARXNG_BASE_URL`. **SearXNG is external and is not bundled with Scorpion**
on any platform.

Search and synthesis are two independent dependencies of Research:

| Step | Dependency |
|---|---|
| Search | SearXNG |
| Synthesis | An OpenAI-compatible LLM |

The plain Web Console search box needs only `SEARXNG_BASE_URL` — it does not
need an LLM at all. Full details: [Installation and Runtime Guide §6](./docs/INSTALLATION_AND_RUNTIME_GUIDE.md#6-search--searxng).

A completed Research run's durable lineage is:

```text
ResearchId → ResearchSession → DurableResearchResult
           → Source N + EvidenceRef → EvidenceId → EvidenceBundle
```

A `ResearchId` reopens one persisted invocation (`scorpion research show
<id>`); each `EvidenceId` identifies durable source evidence behind its
citations. This does not claim deterministic replay or complete request
snapshots — see [Public roadmap](#public-roadmap).

## Capability overview

```text
Browser
   |
   v
Scorpion Web Console
   |
   v
scorpion-api :8787                      (real default; SCORPION_API_BIND)
   |
   +-- Fetch / Audit / Evidence / IAM   (built in, no external service)
   |
   +-- Research
        |
        +-- Search     -> SearXNG                    (:8080 is an example port)
        |
        +-- Synthesis  -> OpenAI-compatible LLM       (:8081 is an example port)
                            |
                            +-- local model
```

| Capability | Windows MSI | Needs LLM | Needs SearXNG | CLI | MCP | Web |
|---|---|---|---|---|---|---|
| IAM Callback Inspector | YES | NO | NO | NO | NO | YES |
| Fetch / Page Audit / Evidence | YES | NO | NO | YES | YES | YES |
| Plain search | YES (runtime) | NO | YES | YES | YES | YES |
| Crawl / scrape / download | NO — CLI not in MSI | NO | NO | YES | YES | NO |
| Durable Research synthesis | Routes exist; PARTIAL until configured | YES | YES | YES | NO | YES |

Full matrix with every route/tool/command cross-referenced against the
actual code: [Installation and Runtime Guide §14](./docs/INSTALLATION_AND_RUNTIME_GUIDE.md#14-capability-matrix).

## Security / current limitations

- Local loopback backend by default (`127.0.0.1:8787`) — never `0.0.0.0`.
- No public listener and no public relay/tunnel by default.
- Sensitive IAM callback material (tokens, `SAMLResponse`, secrets, cookies)
  is redacted to a SHA-256 digest — never persisted or logged in plaintext.
- Local SQLite persistence only.
- No bundled credentials of any kind.
- No automatic token replay, and no credential vault.
- No bundled model, and no bundled external search service.
- The current Windows MSI is **unsigned** (`CODE_SIGNING: NOT CONFIGURED`).
- Every optional external service (LLM, SearXNG) is explicitly
  operator-configured — Scorpion never auto-discovers or silently connects
  to one.
- HTTP/2 is not a guaranteed protocol.
- Real Chrome/CDP acquisitions may truthfully expose
  `BackendProvenance = None` and `ResponseOrigin = None` when the model
  cannot identify an HTTP backend.
- Tor requires operator-provided SOCKS5h/Tor infrastructure.
- Local PaliGemma challenge routing is optional, bounded by its pinned
  model/hardware/supported challenge-family contract — Scorpion does not
  promise general third-party CAPTCHA bypass.
- `wreq`, decentralized, memvid/full-agent, `spider_worker`, experimental
  backend combinations, and direct/internal Chrome APIs remain excluded from
  the production-ready surface described here.
- The Windows MSI is `TESTED / ARTIFACT_BUILT` — not yet `OWNER_VERIFIED` or
  `LIVE_VERIFIED` on a real Windows machine (see [Windows download](#windows-download)).
  Linux/CLI/MCP distribution remains source-build-only; there is currently no
  prebuilt CLI/MCP binary release.

Full evidence-level vocabulary (`INTERNAL`/`SURFACED`/`TESTED`/
`LIVE_VERIFIED`/`OWNER_VERIFIED`) and its exact meaning: [Installation and
Runtime Guide §17](./docs/INSTALLATION_AND_RUNTIME_GUIDE.md#17-verified-vs-not-verified).

## Development

Build from source (unchanged CLI/MCP path):

```sh
git clone https://github.com/Agentxx7/scorpion-rs.git
cd scorpion-rs
cargo build --locked -p spider_cli -p spider_mcp
```

Resulting binaries: `target/debug/scorpion` and `target/debug/spider-mcp`
(add `--release` for optimized artifacts). Run `scorpion --help` and
`scorpion <command> --help` for the complete shipping command surface.

Building/running the Web Console (`scorpion-api`) directly, the Windows
launcher, developer database configuration, and the test commands for every
package: [Installation and Runtime Guide §13](./docs/INSTALLATION_AND_RUNTIME_GUIDE.md#13-developer-setup).

### Proof-gated development

AI may propose and implement changes, but AI is not the authority that a
capability works. Scorpion narrows every claim to evidence:

```text
architecture contract → implementation → TDD → guardrails
→ shipping-binary tests → adversarial failures → production reality
→ exact-SHA CI → release gate
```

Green unit tests alone do not mean `CLOSED`. The project checks that code
exists, the real shipping path reaches it, failures cannot become false
success, provenance remains truthful, the exact commit passed CI, and required
external behavior was observed. See [SCORPION_PROCESS.md](./SCORPION_PROCESS.md),
[SCORPION_ARCHITECTURE.md](./SCORPION_ARCHITECTURE.md), and
[docs/INTELLIGENT_FAILURE.md](./docs/INTELLIGENT_FAILURE.md).

### V1 production reality

The exact released SHA is `6c7253b8c3f7a4073975dc5cdc25a6572e2113e7`.
Production Reality run [33144608097](https://github.com/Agentxx7/scorpion-rs/actions/runs/33144608097)
executed **17/17 required cases with zero skips**. It proved public DNS,
valid TLS, invalid-certificate rejection, redirects, remote 404/500, NXDOMAIN,
Website crawl and scrape, CLI crawl/scrape/download/fetch, external MCP HTTPS
and failure behavior, real external Chrome with JavaScript rendering, truthful
evidence/provenance, and false-success protection. Release workflow run
[33146075800](https://github.com/Agentxx7/scorpion-rs/actions/runs/33146075800)
created tag `v1.0.0` only after the exact build and security gates passed.
This remains the CLI/MCP engine's own production-reality record; the Windows
installable application ([above](#windows-download)) is a later, separately
verified addition on top of it.

### Public roadmap

**Shipped since v1.0.0:** the Web Console (Search, Research, Evidence
Inspector, Fetch, Page Audit), the IAM Callback Inspector (generic/OAuth2/
OIDC/JWT/SAML structural decoding), and a real Windows-installable
application (MSI, `TESTED / ARTIFACT_BUILT`, pending owner Windows install —
see [above](#windows-download)). These were previously listed here as future
work; they are real, shipped capabilities now, subject to the limitations
stated throughout this README.

Still genuinely future work, with no dates or version promises: cryptographic
JWT/SAML signature verification, a `state`/`nonce`/`InResponseTo` expectation
model, real IdP interoperability testing, deterministic research replay and
complete search/model request snapshots, richer browser/DOM/network traces,
a background watch-scheduler daemon and notifications, and packaging
improvements (Windows code signing, Linux/macOS installable artifacts).

## Documentation

- [docs/INSTALLATION_AND_RUNTIME_GUIDE.md](./docs/INSTALLATION_AND_RUNTIME_GUIDE.md) — install, configure, and troubleshoot Scorpion (start here for setup)
- [SCORPION.md](./SCORPION.md) — public product contract
- [SCORPION_ARCHITECTURE.md](./SCORPION_ARCHITECTURE.md) — canonical ownership and architecture truth
- [SCORPION_SDD.md](./SCORPION_SDD.md) — design specification
- [SCORPION_PROCESS.md](./SCORPION_PROCESS.md) — frontier and verification process
- [docs/INTELLIGENT_FAILURE.md](./docs/INTELLIGENT_FAILURE.md) — AI/TDD/product-confidence lessons
- [CLI guide](./spider_cli/README.md)
- [MCP guide](./spider_mcp/README.md)

## Upstream and license

For inherited Spider APIs, packages, and managed service, see the
[Spider repository](https://github.com/spider-rs/spider),
[Spider crate](https://crates.io/crates/spider), [docs.rs](https://docs.rs/spider),
and [Spider Cloud](https://spider.cloud). Spider Cloud is optional; Scorpion
core remains self-hostable. The repository retains Spider's MIT license and
attribution.
