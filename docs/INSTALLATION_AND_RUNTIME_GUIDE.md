# Scorpion — Installation and Runtime Guide

This is the canonical, single entry point for going from a fresh machine to a
running Scorpion install, and for understanding exactly which capabilities
work out of the box versus which require optional external services you
configure yourself.

Every claim in this guide is checked against the actual shipping code and the
real, verified Windows installer artifact — not aspirational. Where something
is not yet proven end-to-end, it says so explicitly, using the evidence
vocabulary defined in [§17](#17-verified-vs-not-verified).

---

## Table of contents

1. [What Scorpion is](#1-what-scorpion-is)
2. [Windows quick start](#2-windows-quick-start)
3. [Base Scorpion — no LLM](#3-base-scorpion--no-llm)
4. [Local LLM stack](#4-local-llm-stack)
5. [Local model installation](#5-local-model-installation)
6. [Search / SearXNG](#6-search--searxng)
7. [Full local stack](#7-full-local-stack)
8. [MCP setup](#8-mcp-setup)
9. [CLI setup](#9-cli-setup)
10. [IAM Callback Inspector](#10-iam-callback-inspector)
11. [IAM troubleshooting examples](#11-iam-troubleshooting-examples)
12. [Troubleshooting Scorpion itself](#12-troubleshooting-scorpion-itself)
13. [Developer setup](#13-developer-setup)
14. [Capability matrix](#14-capability-matrix)
15. [Security model](#15-security-model)
16. ["What should I install?"](#16-what-should-i-install)
17. [Verified vs not verified](#17-verified-vs-not-verified)

---

## 1. What Scorpion is

Scorpion is an evidence-first web crawling, acquisition, audit, and IAM
diagnostics engine. **Scorpion itself is not an LLM** — it has no model
weights and does no inference. Everything that involves language-model
reasoning (durable Research's synthesis step) is delegated to a separate,
operator-configured OpenAI-compatible server that you point Scorpion at.

```text
User
  ↓
Scorpion UI / MCP / CLI          ← three thin interfaces over one engine
  ↓
scorpion-api / canonical core    ← spider, spider_agent, scorpion_app
  ↓
crawler / audit / evidence / IAM / research
  ↓
optional external services       ← LLM server, SearXNG — you configure these
```

Every interface (Web Console, MCP, CLI) calls the same canonical engine —
none of them re-implements crawling, auditing, or evidence handling on its
own. See [SCORPION_ARCHITECTURE.md](../SCORPION_ARCHITECTURE.md) for the full
ownership map this guide is derived from.

| Category | Meaning | Examples |
|---|---|---|
| **BUILT IN** | Ships in the Windows MSI, works with zero extra configuration | Web Console, IAM Callback Inspector, crawl/scrape/fetch/audit engine, evidence storage |
| **OPTIONAL** | A real capability that exists in the code, but only activates when you configure it | Durable Research synthesis, plain search |
| **EXTERNAL** | Separate software Scorpion talks to over HTTP — Scorpion does not install, bundle, or manage it | An OpenAI-compatible LLM server, a SearXNG instance |
| **OWNER-CONFIGURED** | A setting only you can supply, because Scorpion has no default for it | `OPENAI_COMPAT_*`, `SEARXNG_BASE_URL` |

---

## 2. Windows quick start

### The artifact

| | |
|---|---|
| File | `scorpion_app-2.53.4-x86_64.msi` |
| SHA-256 | `8578692a99c7317448d14b64820786b826f28e23305b1c509e4e30ec6f2f59c6` |
| Built by | `.github/workflows/windows-package.yml`, on a real `windows-latest` GitHub Actions runner |
| Status | **TESTED / ARTIFACT_BUILT** — see [§17](#17-verified-vs-not-verified) |

### Steps

1. **Download** `scorpion_app-2.53.4-x86_64.msi` from the `scorpion-windows-installer`
   artifact of the `Windows Package` GitHub Actions workflow run for the
   commit you want.
2. **Verify the hash** before running it:
   ```powershell
   certutil -hashfile scorpion_app-2.53.4-x86_64.msi SHA256
   ```
   Confirm it prints `8578692a99c7317448d14b64820786b826f28e23305b1c509e4e30ec6f2f59c6`.
3. **Install it.** Windows will show a User Account Control (UAC) prompt —
   this is normal for any MSI that installs into `Program Files`, and is
   unrelated to the next point.
4. **Expect an "Unknown publisher" / SmartScreen warning.** The installer is
   genuinely **unsigned** — there is no code-signing certificate configured
   for this build (see [§15](#15-security-model)). This is not a defect
   being hidden from you; it is the honest current state. If you are not
   comfortable running an unsigned installer, don't — build from source
   instead ([§13](#13-developer-setup)).
5. **Launch Scorpion** from the Start Menu or the Desktop shortcut the
   installer created. Both launch `scorpion-launcher.exe`, never
   `scorpion-api.exe` directly, and never a terminal.
6. Scorpion starts its local backend, waits until it reports healthy, then
   opens your default browser to `http://127.0.0.1:8787/`.
7. **No terminal, WSL, or Cargo/Rust toolchain is required** for this normal
   installed use. Those are only needed for [developer setup](#13-developer-setup).

### Application data

Scorpion never writes anywhere under its own (read-only) install directory.
Everything mutable lives under your user profile:

| | Path |
|---|---|
| Application-data root | `%LOCALAPPDATA%\Scorpion\` |
| Database | `%LOCALAPPDATA%\Scorpion\scorpion.sqlite3` |
| Logs | `%LOCALAPPDATA%\Scorpion\logs\launcher.log`, `%LOCALAPPDATA%\Scorpion\logs\scorpion-api.log` |

### Uninstalling

Uninstall through **Settings → Apps → Installed apps** (or the classic
**Programs and Features** control panel). This removes the installed
binaries and both shortcuts. It does **not** touch `%LOCALAPPDATA%\Scorpion\`
— your database and logs are outside the installer-managed directory
entirely, so they are preserved by default. If you want to delete your data
too, remove `%LOCALAPPDATA%\Scorpion\` yourself afterward.

---

## 3. Base Scorpion — no LLM

Everything in this section works immediately after installing the MSI, with
**no** local model, no SearXNG, and no configuration of any kind.

| Capability | Interface | Status | Notes |
|---|---|---|---|
| Web Console home page | Web (`GET /`) | **BUILT IN** | |
| IAM Callback Inspector | Web | **BUILT IN** | New Trace, receive a real callback, inspect decoded facts — see [§10](#10-iam-callback-inspector) |
| Page Audit | Web (`POST /api/audit`) | **BUILT IN** | Deterministic rule-based audit; needs only a local SQLite path, which the installed app already configures |
| Fetch (single resource) | Web (`POST /api/fetch`) | **BUILT IN** | One URL, HTTP(S) only, no crawl following |
| Evidence Inspector | Web (`GET /api/evidence/{ref}` + content/export downloads) | **BUILT IN** | Reads back exactly what Audit/Fetch/Research recorded |
| `/health` | Web | **BUILT IN** | Trivial liveness check, no persistence dependency |
| Plain search (`POST /api/search`, the home-page search box) | Web | **OPTIONAL** | Works with only `SEARXNG_BASE_URL` set — no LLM needed at all |
| Durable Research (search → acquire → synthesize) | Web + CLI | **OPTIONAL** | Needs a database path, `SEARXNG_BASE_URL`, and all three `OPENAI_COMPAT_*` variables — see [§4](#4-local-llm-stack)/[§6](#6-search--searxng) |
| `crawl` / `scrape` / `download` | CLI only | **BUILT IN** (once the CLI is separately installed — see [§9](#9-cli-setup)) | Not exposed by the Windows MSI's Web Console; the MSI does not install the CLI binary at all |
| MCP tools (`spider_scrape`, `spider_crawl`, …) | MCP only | **BUILT IN** (once `spider-mcp`/`scorpion mcp` is separately installed — see [§8](#8-mcp-setup)) | Not part of the Windows MSI |

**Do not assume every interface exposes every capability.** Intentional
restrictions, verified against the actual routes/commands/tools in the code:

- The Windows MSI installs **only** `scorpion-api.exe` and
  `scorpion-launcher.exe`. It does **not** install the `scorpion` CLI binary
  or the `spider-mcp` MCP server binary.
- The **Web Console** exposes search, Research, Evidence, Audit, Fetch, and
  IAM — it does **not** expose a general crawl/scrape/download UI.
- **MCP** exposes read-only/single-acquisition tools (scrape, crawl, links,
  transform, crawl-status, search, media-search, news-search, audit-page,
  evidence-read, and feed/sitemap/robots/news-sitemap readers). It does
  **not** expose durable Research's synthesis flow, and it does **not**
  expose any IAM Callback Inspector tool.
- **IAM** is Web-only. There is no CLI or MCP surface for it.

---

## 4. Local LLM stack

The Windows MSI does **not** install any of the following:

- llama.cpp / `llama-server`
- a Qwen (or any other) model's weight files
- SearXNG
- any other LLM runtime

These are entirely **your own** responsibility to install and run, exactly
as you would for any other OpenAI-compatible-API consumer.

### Configuration model

Scorpion's Research capability talks to your LLM server over the standard
OpenAI Chat Completions HTTP API. It is configured through exactly these
environment variables — these are the real names read by the running code,
not illustrative placeholders:

| Variable | Required for Research | Meaning |
|---|---|---|
| `OPENAI_COMPAT_BASE_URL` | Yes | Base URL of an OpenAI-Chat-Completions-compatible server, including the `/v1` suffix your server expects |
| `OPENAI_COMPAT_MODEL` | Yes | The model identifier/path your server expects in requests |
| `OPENAI_COMPAT_API_KEY` | Yes | API key your server expects — many local servers accept any non-empty string |
| `OPENAI_COMPAT_TIMEOUT_SECS` | No | Optional request timeout override, in whole seconds |

A representative **development-only example** (illustrative — your own
model path and port will differ):

```sh
OPENAI_COMPAT_BASE_URL=http://127.0.0.1:8081/v1
OPENAI_COMPAT_MODEL=<the model identifier/path your server expects>
OPENAI_COMPAT_API_KEY=local
```

`8081` above is only an example port, not a Scorpion default — Scorpion has
no built-in default for any of these three variables; all three are
required, and Research fails closed with a truthful "not configured" state
if any is missing.

### Relationship

```text
Scorpion  ──OpenAI-compatible HTTP (Chat Completions)──▶  llama-server (or equivalent)
                                                                ↓
                                                          local model weights
```

### Can I use a different OpenAI-compatible server?

Scorpion's client is a generic OpenAI Chat-Completions client with an
overridable base URL — it is not hardcoded to `llama-server` specifically.
Any server that correctly implements that same HTTP contract at the URL you
give it should work. This is a statement about the transport contract, not a
promise that every third-party server's behavior/quirks have been tested —
if a particular server diverges from the standard Chat Completions shape,
Research's own error handling is the only thing standing between you and a
truthful failure, not a guarantee of compatibility.

---

## 5. Local model installation

This is entirely **manual external setup** — Scorpion does not bootstrap,
download, or manage a model for you, on Windows or anywhere else.

**MANUAL EXTERNAL SETUP** covers all of the following; none of it is a
Scorpion installer flow:

- **Disk space**: model weight files for a usable local model are typically
  several gigabytes to tens of gigabytes, depending on parameter count and
  quantization. Plan storage accordingly before downloading anything.
- **CPU vs GPU execution**: a llama.cpp-family server can run on CPU alone
  (slower) or accelerated on a GPU if your build/hardware supports it. This
  is a property of the LLM runtime you choose, not of Scorpion.
- **llama.cpp / `llama-server`**: obtain and build/install it per its own
  project's instructions for your platform. This is third-party software.
- **Model file**: obtain a model file compatible with your chosen runtime
  (e.g. a GGUF file for llama.cpp) from a source you trust.
- **Server port**: start your LLM server on a port of your choosing (any
  free local port — there is no Scorpion-mandated port).
- **Confirm the `/v1` endpoint is up** before pointing Scorpion at it — for
  an OpenAI-compatible server this usually means confirming
  `GET http://<host>:<port>/v1/models` (or your server's equivalent) returns
  a response.
- **Configure Scorpion** to use it: set `OPENAI_COMPAT_BASE_URL` to that
  server's base URL (including `/v1`), `OPENAI_COMPAT_MODEL` to whatever
  identifier your server expects, and `OPENAI_COMPAT_API_KEY` to whatever
  key value your server expects (often any non-empty string for local
  servers).

---

## 6. Search / SearXNG

Durable Research needs a **search** step to discover candidate sources, and
that is a separate concern from the LLM used for synthesis — you can have
one configured without the other, though Research itself needs both.

```text
Scorpion Research
   ├── search      → SearXNG
   ├── acquisition → canonical Scorpion transport (built in)
   └── synthesis   → LLM (§4)
```

### Configuration

| Variable | Meaning |
|---|---|
| `SEARXNG_BASE_URL` | Base URL of an operator-provided SearXNG instance |
| `SEARCH_PROVIDER` | Optional selector; defaults to SearXNG when unset |

Example:

```sh
SEARXNG_BASE_URL=http://127.0.0.1:8080
```

`8080` above is only an example port — Scorpion has no built-in default for
`SEARXNG_BASE_URL`; it must be set explicitly, or Research/plain-search stay
in their existing truthful "not configured" state.

**SearXNG is not bundled in the Windows MSI, and Scorpion does not install
it for you on any platform.** The plain search box on the Web Console home
page (`POST /api/search`) only needs `SEARXNG_BASE_URL` — it does **not**
need any `OPENAI_COMPAT_*` variable at all. Durable Research needs both
search and the LLM stack together.

Other search-provider selectors (`brave`, `serper`, `tavily`) are recognized
by the configuration parser, but the Windows MSI's `scorpion-api.exe` (and a
default, no-extra-flags `cargo install spider_cli` build of the CLI) are not
compiled with those providers' feature flags enabled — selecting one of them
resolves to an explicit "unsupported provider" state, not a missing-config
one, and not a silent fallback to SearXNG.

### Docker-based SearXNG

A verified, reproducible way to run SearXNG locally is via its own official
Docker image, run as a normal container exposing its web port to your host
(e.g. `-p 8080:8080`) — consult SearXNG's own documentation for the exact
compose/run invocation, since that is third-party software this guide does
not maintain. There is no Windows-native SearXNG installer, and this guide
does not invent one.

---

## 7. Full local stack

```text
Browser
   ↓
Scorpion UI (installed by the Windows MSI)
   ↓
scorpion-api :8787                      ← fixed default (SCORPION_API_BIND)
   ├── IAM / crawl / scrape / audit / evidence   ← built in, no external service
   ├── Research
   │     ├── SearXNG :8080               ← example port — you choose it
   │     └── OpenAI-compatible LLM :8081 ← example port — you choose it
   └── persistence
         ↓
      SQLite  (%LOCALAPPDATA%\Scorpion\scorpion.sqlite3 on the installed app)
```

- `:8787` is a real default (`SCORPION_API_BIND` falls back to
  `127.0.0.1:8787` if unset) — this is what the installed app always binds.
- `:8080` and `:8081` above are **illustrative example ports only** for
  SearXNG and your LLM server respectively — Scorpion imposes no default or
  requirement on either; you choose the ports your own external services
  listen on and tell Scorpion about them via `SEARXNG_BASE_URL`/
  `OPENAI_COMPAT_BASE_URL`.

---

## 8. MCP setup

`spider_mcp` is Scorpion's canonical [Model Context Protocol](https://modelcontextprotocol.io/)
server — it exposes a fixed set of crawl/scrape/audit/evidence tools for an
AI assistant client to call. It is a **separate binary**, not part of the
Windows MSI.

### Launching the server

Two equivalent ways to run the identical server implementation:

```sh
# Standalone binary
cargo install spider_mcp
spider-mcp

# Or via the CLI (same server implementation, in-process)
cargo install spider_cli
scorpion mcp
```

Both speak MCP over stdio — stdout is reserved for MCP protocol traffic;
diagnostic logs go to stderr. The only CLI flag either form accepts is
`--log-level <LEVEL>` (default `warn`).

### Connecting a client

A generic MCP client configuration (this repo's own convention, from
`spider_mcp/README.md`):

```json
{
  "mcpServers": {
    "spider": {
      "command": "spider-mcp",
      "args": [],
      "env": {}
    }
  }
}
```

### Available, owner-verified MCP tools

| Tool | What it does |
|---|---|
| `spider_scrape` | Fetch one page, return markdown/text/HTML/XML/screenshot (or a structured evidence bundle with `evidence: true`) |
| `spider_crawl` | Crawl a site to a configurable depth/limit |
| `spider_links` | Extract links from a page without fetching full content |
| `spider_transform` | Convert raw HTML to markdown/text/XML — offline, no network |
| `spider_crawl_status` | Poll a background crawl started by `spider_crawl` |
| `spider_search` | Search via a self-hosted SearXNG instance — the caller passes `base_url` as a tool argument on each call (not an env var); does not fetch results |
| `spider_media_search` | Search a media category (video/image) — same `base_url` tool-argument shape as `spider_search` |
| `spider_news_search` | Discover news results — same `base_url` tool-argument shape as `spider_search` |
| `spider_audit_page` | Run the same deterministic page-audit engine the Web Console's `POST /api/audit` uses |
| `spider_evidence_read` | Read back one already-persisted `EvidenceBundle` by its `evidence_ref` — no re-fetch |
| `spider_sitemap_read` / `spider_feed_read` / `spider_robots_sitemap_read` / `spider_news_sitemap_read` | Read one sitemap/feed/robots.txt/news-sitemap document — discovery only, never fetch the discovered URLs |

### Intentionally not exposed via MCP

- **Durable Research's synthesis flow.** MCP does not own or link
  `spider_agent`'s LLM execution — there is no MCP tool that runs the full
  search → acquire → synthesize → persist Research pipeline. That exists
  only through the Web Console and the `scorpion research` CLI command.
- **The IAM Callback Inspector.** There is no `spider_iam_*` tool of any
  kind — IAM is Web-only.

---

## 9. CLI setup

The Windows MSI does **not** install the CLI. The CLI binary is named
`scorpion` (crate `spider_cli`) and must be installed/built separately:

```sh
# default install (includes chrome support)
cargo install spider_cli
```

See [`spider_cli/README.md`](../spider_cli/README.md) for platform
dependencies (OpenSSL/`pkg-config` on Linux) and feature-flag details.

### Representative commands

`--url` is a **top-level** flag — it must come before the subcommand for
`crawl`/`scrape`/`download`, which read it from there:

```sh
# Crawl, printing discovered links
scorpion --url https://choosealicense.com crawl --output-links

# Crawl with a page-count budget, JSON output
scorpion --url https://choosealicense.com --budget "*,100,/blog/,10" crawl -o

# Scrape one page as markdown
scorpion --url https://choosealicense.com --return-format markdown scrape --output-html

# Download page markup to a directory
scorpion --url https://choosealicense.com download -t _temp_spider_downloads
```

`audit` and `fetch` take the URL as their own positional argument instead
(not the top-level `--url`):

```sh
# Deterministic page audit — same engine as POST /api/audit and spider_audit_page
scorpion audit https://example.com

# Fetch exactly one resource and print its evidence
scorpion fetch https://example.com
```

Artifact discovery/download (Hugging Face repositories only, read-only
discovery plus an explicit, bounded download step):

```sh
scorpion hugging-face-artifacts owner/model

scorpion artifact-download \
  --reference-file artifact.json \
  --destination ./model.bin \
  --max-bytes 10000000000
```

Transport options (`crawl`/`scrape`/`download`/`fetch`/feed/sitemap
commands all accept these):

```sh
--transport tor --tor-proxy socks5h://127.0.0.1:9050
```

Durable Research from the CLI — see [`spider_cli/README.md`](../spider_cli/README.md#durable-research)
for the full configuration walkthrough; it uses the identical
`OPENAI_COMPAT_*`/`SEARXNG_BASE_URL` variables documented in
[§4](#4-local-llm-stack)/[§6](#6-search--searxng):

```sh
scorpion research "your research topic"
scorpion research show <research-id>
```

`SCORPION_DOMAIN_DB` is the current, preferred canonical database variable
(checked first); the CLI's own README examples still show the older
`RESEARCH_EVIDENCE_DB` name, which remains fully honored as a fallback for
existing deployments — either works, `SCORPION_DOMAIN_DB` is simply the one
new deployments should set.

There is currently no `scorpion iam` subcommand — IAM is Web-only
(see [§3](#3-base-scorpion--no-llm)).

---

## 10. IAM Callback Inspector

A practical operator flow, using the same interface described in
[§3](#3-base-scorpion--no-llm):

```text
Launch Scorpion
  → IAM Callback Inspector (on the Web Console home page)
  → New Trace
  → copy the callback URI shown (a real http://127.0.0.1:8787/iam/callback/<id> URL)
  → point your application's or IdP's callback/redirect setting at it
  → trigger the real login/callback flow
  → the trace's timeline updates with the decoded observation
  → inspect the facts
```

### What it inspects

- Generic HTTP `GET`/`POST` callbacks (any query/form/JSON parameters)
- OAuth2 (`code`, `error`, `state`)
- OIDC (`id_token` presence, decoded claims)
- JWT structural decode of an `id_token`: header (`alg`/`typ`/`kid`) and
  payload (`iss`/`aud`/`exp`/`iat`/`nbf`/`nonce`) claims
- SAML 2.0 HTTP-POST binding: structural decode of the `SAMLResponse`
  Response/Assertion (ID, Destination, Version, `InResponseTo`, Issuer,
  Status, Assertion ID/IssueInstant, `NameID`(+Format), Conditions
  `NotBefore`/`NotOnOrAfter`, Audience, Attribute name/value, and signature
  *presence*)

### Fact statuses

Every decoded fact carries exactly one of four statuses, shown as a distinct
badge:

| Status | Meaning |
|---|---|
| **OBSERVED** | Present in the callback and decoded — a plain, unverified fact |
| **VALIDATED** | Reserved for a future capability — no code path in Scorpion currently constructs this status for any signature or token-authenticity claim |
| **NOT VALIDATED** | Present, but Scorpion has not checked it against anything — this is the status for every signature-verification result and every anti-replay/correlation value (`state`, `nonce`, `InResponseTo`) |
| **REDACTED** | The raw value is sensitive (`access_token`, `id_token`, `SAMLResponse`, `password`, etc.) — only a SHA-256 digest of it is ever shown or stored, never the plaintext |

### What is explicitly not implemented — do not read "decoded" as "verified"

| Capability | Status |
|---|---|
| JWT cryptographic signature verification | **NOT IMPLEMENTED** |
| SAML XML-DSig verification | **NOT IMPLEMENTED** |
| `state` expectation comparison | **NOT IMPLEMENTED** |
| `nonce` expectation comparison | **NOT IMPLEMENTED** |
| `InResponseTo` expectation comparison | **NOT IMPLEMENTED** |
| Real IdP interoperability | **NOT YET LIVE/OWNER VERIFIED** — only synthetic local JWT/SAML material has been exercised so far |

Structural decoding is not, and must never be read as, cryptographic
validation. A JWT or SAML Response that decodes cleanly tells you the
*shape* of what was sent — it tells you nothing about whether a real,
trusted party actually signed it.

---

## 11. IAM troubleshooting examples

**A. "Login returns to the wrong callback"** — inspect the observed
`Destination`/redirect target facts in the trace's timeline against what
your application/IdP is actually configured to use.

**B. "User authenticated but the application rejects the token"** —
inspect the decoded `iss` (issuer) and `aud` (audience) facts against what
your application expects, and `exp`/`nbf` against the current time. All of
these are shown as **OBSERVED** facts, not verdicts — you are comparing the
raw values yourself.

**C. "SAML attributes are missing"** — inspect the `NameID` and
`AttributeStatement`-derived attribute facts in the timeline. If an
attribute you expect is entirely absent from the trace, the IdP did not
include it in the Assertion at all — Scorpion surfaces every attribute the
Assertion actually contained, nothing more and nothing less.

**D. "Possible request/response correlation problem"** — inspect the
`state`, `nonce`, and `InResponseTo` facts. **Scorpion currently only
observes these values — it does not compare them against a stored
expectation from the original request.** A mismatch you find by manually
comparing the observed value against what your application actually sent is
real; Scorpion will not flag it for you automatically.

**E. "Signature present"** — the Inspector reports whether a
`<Signature>` element (SAML) or a signature segment (JWT) is structurally
present in the material. **This is not the same thing as the signature
being cryptographically verified.** `signature.present = true` and
`signature.verification = NOT VALIDATED` can both be true for the exact
same observation — presence and verification are two separate facts, and
only presence is currently checked.

---

## 12. Troubleshooting Scorpion itself

### Health check

```text
http://127.0.0.1:8787/health
```

A truthful `{"status":"ok"}` means the HTTP listener is up. It does **not**
by itself mean Research/search are configured — those report their own
independent status.

### Windows logs

```text
%LOCALAPPDATA%\Scorpion\logs\launcher.log       ← startup orchestration
%LOCALAPPDATA%\Scorpion\logs\scorpion-api.log   ← the backend's own stdout/stderr
```

### Port diagnosis

```powershell
netstat -ano | findstr 8787
```

### Common conditions

| Symptom | What to check |
|---|---|
| Browser did not open | Check `launcher.log` for the exact failure — the launcher shows a native error dialog and never opens a browser on a fabricated success |
| Port `8787` occupied | The launcher probes `/health` first: if something already answers healthily there, it reuses it; if something else owns the port, it fails visibly rather than guessing a different port. `launcher.log` states which happened |
| Backend never became healthy | Check `scorpion-api.log` for the real startup error (e.g. a corrupted install) |
| Database path problem | Confirm `%LOCALAPPDATA%\Scorpion\` exists and is writable by your user account — it is created automatically on first launch |
| Optional LLM unavailable | Research/synthesis reports a truthful "not configured"/"unavailable" state rather than failing silently — this is expected until you configure `OPENAI_COMPAT_*` |
| Optional search unavailable | Plain search and Research report a truthful "not configured" state until `SEARXNG_BASE_URL` is set |
| MSI SmartScreen/unsigned-installer warning | Expected — see [§2](#2-windows-quick-start) and [§15](#15-security-model). **Never disable Windows security features globally** to work around this; if you are not comfortable proceeding, verify the SHA-256 yourself and/or build from source instead |

---

## 13. Developer setup

This section is **only** for building Scorpion from source or contributing
to it. **None of this is required for normal use of the installed Windows
MSI** — that is covered entirely by [§2](#2-windows-quick-start).

1. **Checkout:**
   ```sh
   git clone https://github.com/Agentxx7/scorpion-rs.git
   cd scorpion-rs
   ```
2. **Rust toolchain:** the pinned version is declared in `rust-toolchain.toml`
   at the repo root (`rustup` will fetch it automatically on first build).
3. **Build the packages/binaries you need:**
   ```sh
   cargo build -p scorpion_app --bin scorpion-api --bin scorpion-launcher
   cargo build -p spider_cli --bin scorpion
   cargo build -p spider_mcp --bin spider-mcp
   ```
4. **Linux/WSL use:** the whole workspace (including `scorpion-api`) builds
   and runs natively on Linux — this is this repository's own primary
   development environment. `scorpion-launcher.exe`'s meaningful behavior
   (hidden console window, `%LOCALAPPDATA%` resolution) is Windows-only by
   design; it still compiles and runs on Linux with a dev-only fallback
   (a `$HOME`-based application-data directory) used purely so its own test
   suite can exercise it — that fallback is not part of the shipped Windows
   product.
5. **Development database configuration:** set `SCORPION_DOMAIN_DB` to any
   local file path before running `scorpion-api` directly:
   ```sh
   SCORPION_DOMAIN_DB=/tmp/scorpion-dev.sqlite3 \
   SCORPION_API_BIND=127.0.0.1:8787 \
     cargo run -p scorpion_app --bin scorpion-api
   ```
6. **Tests:**
   ```sh
   cargo test -p scorpion_app
   cargo test -p spider_cli
   cargo test -p spider_mcp
   cargo test -p spider --test architecture_guardrails
   ```

For the full architecture/ownership contract, see
[SCORPION_ARCHITECTURE.md](../SCORPION_ARCHITECTURE.md).

---

## 14. Capability matrix

| Capability | Base MSI | Needs LLM | Needs SearXNG | CLI | MCP | Web |
|---|---|---|---|---|---|---|
| Web Console home page | YES | NO | NO | — | — | YES |
| IAM Callback Inspector | YES | NO | NO | NO | NO | YES |
| Page Audit | YES | NO | NO | YES | YES (`spider_audit_page`) | YES |
| Fetch (single resource) | YES | NO | NO | YES | via `spider_scrape` (`evidence: true`) | YES |
| Evidence Inspector / download | YES | NO | NO | — | YES (`spider_evidence_read`) | YES |
| Plain search | Base MSI runtime: YES | NO | YES | YES | YES (`spider_search`) | YES |
| Crawl / scrape / download | NO (CLI not in MSI) | NO | NO | YES | YES (`spider_crawl`/`spider_scrape`/`spider_links`) | NO |
| Feed / sitemap / robots / news-sitemap discovery | NO (CLI not in MSI) | NO | NO | YES | YES | NO |
| Durable Research (synthesis) | Base MSI runtime: PARTIAL — routes exist, need config | YES | YES | YES | NO | YES |
| Artifact discovery/download (Hugging Face) | NO (CLI not in MSI) | NO | NO | YES | NO | NO |
| Tor transport | NO (CLI not in MSI) | NO | NO | YES (`--transport tor`) | YES (feature-gated) | NO |

"Base MSI: NO" above means the *capability's interface* is not installed by
the Windows MSI (the CLI/MCP binaries aren't part of it) — not that the
underlying engine code doesn't exist. "Base MSI runtime: PARTIAL" for
Research means the `/api/research` routes exist and answer truthfully in the
installed app, but return a "not configured" state until you supply
`OPENAI_COMPAT_*` and `SEARXNG_BASE_URL` yourself.

---

## 15. Security model

- **Default bind is loopback-only:** `127.0.0.1:8787`, never `0.0.0.0`. The
  installed Windows app never overrides this.
- **No public relay or tunnel** of any kind exists or is installed.
- **Sensitive IAM values are redacted:** raw `id_token`, `SAMLResponse`,
  `access_token`, `refresh_token`, `client_secret`, `password`,
  `authorization`, cookies, and equivalents are never persisted or logged in
  plaintext — only a SHA-256 digest, ever.
- **Local database only:** SQLite, on your own machine
  (`%LOCALAPPDATA%\Scorpion\scorpion.sqlite3` on the installed Windows app).
- **No bundled credentials** of any kind ship in the installer or the
  repository.
- **No automatic token replay, and no credential vault.** IAM inspection is
  passive observation only — it never re-uses a captured token or code
  against anything.
- **The Windows installer is currently unsigned.** `CODE_SIGNING: NOT
  CONFIGURED` — there is no code-signing certificate in this project's
  current build pipeline. This is stated plainly rather than worked around;
  producing a signed build is a separate, not-yet-authorized effort.
- **Every optional external service is explicitly operator-configured** —
  Scorpion never silently discovers, assumes, or auto-connects to an LLM
  server or a SearXNG instance; you set the environment variables yourself.

---

## 16. "What should I install?"

### Profile A — IAM troubleshooting

**Install:** the Scorpion Windows MSI. Nothing else.

Launch it, open the IAM Callback Inspector, and start inspecting real
callbacks immediately — no LLM, no search, no additional install.

### Profile B — Web / integration toolkit

**Install:** the Scorpion Windows MSI, plus whichever of the CLI
(`cargo install spider_cli`) or MCP server (`cargo install spider_mcp`) you
actually need for crawling/scraping/auditing workflows.

No LLM is required for any of the ordinary deterministic inspection
capabilities (audit, fetch, evidence, crawl, scrape, feed/sitemap reading).

### Profile C — Full local AI research

**Install/configure:**

- Scorpion (Windows MSI, and/or CLI for `scorpion research`)
- A local OpenAI-compatible LLM runtime (e.g. `llama-server`) — manual
  external setup, [§5](#5-local-model-installation)
- A model file for that runtime
- SearXNG — manual external setup (Docker recommended), [§6](#6-search--searxng)
- Optionally, an MCP client integration if you want an AI assistant driving
  the deterministic tools directly

---

## 17. Verified vs not verified

This guide uses five evidence levels consistently. A capability's real,
current status is always one of these — never a bare "works":

| Level | Meaning |
|---|---|
| **INTERNAL** | Implemented in code, exercised only by unit tests |
| **SURFACED** | Reachable through a real interface (a route, a CLI command, an MCP tool) |
| **TESTED** | Automated test coverage exists and passes against the real interface |
| **LIVE_VERIFIED** | Personally exercised against a real, live dependency (a real HTTP call, a real callback, a real CI build) — by whoever is claiming it, not merely by an automated test fixture |
| **OWNER_VERIFIED** | The project owner has personally confirmed it works, on the actual target environment |

**The Windows MSI installer is currently:**

```text
ARTIFACT_BUILT / TESTED
PENDING OWNER WINDOWS INSTALL
```

A real `scorpion_app-2.53.4-x86_64.msi` was built, linked, and packaged by
genuine `windows-latest` GitHub Actions CI, and that CI run's own test suite
passed against the real, compiled binaries on real Windows. That is
**TESTED / ARTIFACT_BUILT** — it is honestly **not** the same claim as
"works on Windows for a real user," which requires a human installing and
running it. Do not read a green CI badge as that stronger claim. This guide
will be updated with the real result once that owner acceptance pass
happens (see [SCORPION_ARCHITECTURE.md](../SCORPION_ARCHITECTURE.md)'s
Installable Application row for the live status).
