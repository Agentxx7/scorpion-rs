# Scorpion — Product & Architecture Contract

**Status:** LOCKED — documentation only. Nothing in this document is implemented
yet; it records decisions so future source changes have a fixed target instead
of drifting.

**Baseline:** forked from [`spider-rs/spider`](https://github.com/spider-rs/spider)
at `229e5cd43628f5ee0b43117ffddfcd20a000ced6`. `origin` is this repository
(Scorpion); `upstream` remains `spider-rs/spider`.

---

## 1. Product Identity

Scorpion is an independent Rust web research, crawling, and intelligence
engine built on top of `spider-rs/spider`. It is designed to remain
independently usable, testable, and publishable — it is a complete product on
its own, not a dependency staged inside another one.

**Nightstalker is not part of Scorpion.** If Nightstalker (or anything else)
ever needs Scorpion's capabilities, it will consume them later through a
clean API/MCP/service boundary — never by reaching into Scorpion's internals
or vice versa. Nothing in this contract, and nothing this document
authorizes, touches Nightstalker.

### Attribution

Scorpion's crawling core is Spider, unmodified in license terms and largely
unmodified in code at this baseline. This repository's `LICENSE` (MIT,
`Copyright (c) 2026 Spider Contributors`) is preserved byte-for-byte and
governs the inherited codebase. Spider's own attribution — project name,
license text, and upstream repository/homepage links in crate metadata — is
not to be stripped, obscured, or rewritten as future Scorpion-specific work
lands. Where Scorpion adds new source files or crates, they inherit the same
MIT terms; this document does not change licensing.

---

## 2. Ownership Boundary

Scorpion owns the full acquisition and evidence-production surface:

- search / discovery
- HTTP fetching
- crawling
- browser rendering
- screenshots
- extraction
- evidence capture
- transport / network policy
- crawl provenance

**Scorpion returns evidence and facts. It does not make downstream domain
judgments.** Anything that interprets, scores, or acts on what was
crawled — including Nightstalker — is out of scope here and consumes
Scorpion's output through the boundary described in §1, not by depending on
its internal types.

---

## 3. Evidence-First Direction

This section locks *concepts*, not representations. None of the identifier
types below are defined, typed, or implemented at this baseline — naming them
here fixes vocabulary so later work doesn't invent five different schemes.

**Identifier concepts (future, undefined representation):**

- `ResearchId` — a research session/goal spanning one or more crawls
- `CrawlId` — a single crawl run (one starting point, one policy)
- `FetchId` — a single request/response exchange within a crawl
- `EvidenceId` — a unit of captured evidence derived from a fetch
- `ArtifactId` — a stored byproduct (screenshot, WARC record, raw blob, …)

**Evidence direction** — the fields a future evidence record is expected to
carry, gathered from what the baseline audit found already exists in Spider
(scattered across `Page`, `warc.rs`, and the HTTP/Chrome/WebDriver response
paths) versus what does not yet exist anywhere:

- requested URL and final URL (redirect-resolved)
- retrieval timestamp
- response status
- headers and content type
- raw content / HTML
- cleaned Markdown
- links
- screenshots
- content hash and screenshot hash
- WARC reference
- parent / discovery relationship (what led here)
- crawl depth
- anchor text
- transport used (direct / proxy / Tor — see §7)
- browser usage (HTTP-only vs. Chrome/WebDriver-rendered)

Unifying these into one evidence bundle type is future work, not this
document's job — see the ranked frontier list from the baseline audit
(`unified evidence bundle` was ranked last precisely because it depends on
decisions this section is deferring on purpose).

---

## 4. Modern Research Roadmap

Recorded as future capability directions, not committed designs:

- adaptive / focused research crawling (steer the crawl toward a research
  goal rather than exhaustive breadth-first traversal)
- browser/DOM/network evidence traces (beyond final rendered HTML — what the
  page actually did)
- deterministic/offline replay where possible (re-derive evidence from a
  WARC/trace without re-fetching)
- first-class non-HTML resources: PDF, JSON, XML, and other document types as
  primary evidence, not incidental byproducts
- reproducible research sessions
- a research/discovery graph (how pages and evidence relate across a
  `ResearchId`, not just within one `CrawlId`)
- deterministic change tracking (has this evidence changed since last seen,
  and how)

---

## 5. Authenticated Research

Future first-class support, concepts only:

**`AuthenticationProfile` variants (locked names, no implementation):**

- `NONE`
- `FORM_LOGIN`
- `BASIC_AUTH`
- `BEARER_TOKEN`
- `COOKIE_SESSION`
- `OAUTH`
- `INTERACTIVE_BROWSER`

**Non-negotiable rules for when this is built:**

- Credentials are referenced, never embedded, via a `CredentialRef` concept
  (locked name, undefined representation — same status as the identifier
  concepts in §3). Evidence records carry a `CredentialRef`, never the
  secret value itself.
- Credentials, tokens, and passwords must never be written into: evidence
  bundles, WARC output, HAR/network traces, Markdown, screenshots (where
  preventable), logs, or MCP tool results.
- Authentication is origin/policy scoped — a credential authorized for one
  origin must not be presented to another, and must fail closed (refuse,
  not silently drop auth) on unsafe redirects that would cross that
  boundary.
- Future MFA/interactive authentication must support pausing and resuming
  the *same* authenticated browser session — not re-authenticating from
  scratch, and not silently continuing unauthenticated.

---

## 6. CAPTCHA / Challenge Handling

Locked scope — authorized, cooperative handling only:

- detect CAPTCHA/challenge state
- support provider-supplied test mechanisms for systems under test (e.g. a
  target explicitly offering a test bypass for its own QA)
- support operator-assisted solve/pause/resume
- preserve browser/session state across a pause for solving

**Explicitly out of scope, now and as a standing policy:** generalized
CAPTCHA bypass intended to defeat third-party anti-bot controls. This
document does not authorize, and future Scorpion work must not implement,
automated defeat of protections a site operator did not choose to relax.

---

## 7. Transport Contract

**`TransportProfile` variants (locked names, no implementation):**

- `DIRECT`
- `PROXY`
- `TOR`

**TOR is not implemented at this baseline.** When it is, it must be fail
closed:

- no direct HTTP fallback
- no local DNS fallback
- no direct Chrome/WebDriver fallback
- no clearnet subresource fallback
- no unsafe redirect fallback

"Fail closed" means: if the Tor path cannot be established or verified for
any request or subresource, that request fails — it does not silently
complete over clearnet.

**Baseline audit findings that must be resolved before `TOR` can be
enabled** (see the prior read-only audit for exact file/line references):
Spider's existing proxy-handling paths (HTTP client, Chrome CDP context, and
their DNS resolution) were found to have silent-fallback and scheme-handling
gaps — a failed or misconfigured proxy can currently fall through to a
direct connection rather than hard-failing, and a non-standard `socks://`
scheme is silently rewritten to `http://` in more than one place. Chrome/
WebDriver launches were also found to have no WebRTC leak mitigation. All of
these are exactly the kind of clearnet-fallback behavior the fail-closed
rule above forbids, and hardening them is a prerequisite for `TOR`, not a
follow-up to it.

---

## 8. Upstream Strategy

- `origin` = Scorpion (this repository)
- `upstream` = `spider-rs/spider`
- History is preserved — this repository is a fork, not a snapshot import.
- New capability work prefers additive modules (new files, new crates, new
  feature flags) over invasive edits to existing upstream files.
- Invasive edits to high-churn upstream files (core client/proxy wiring,
  `Page`, Chrome integration) are minimized and done deliberately, not as a
  side effect of unrelated feature work.
- No unnecessary bulk crate rename. Spider's crate names and structure stay
  as-is unless a specific, justified reason requires otherwise.
