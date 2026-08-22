# Scorpion — Product & Architecture Contract

**Status:** ACTIVE PRODUCT CONTRACT. Sections explicitly marked current describe
implemented or partially implemented Scorpion behavior. Sections explicitly
marked future or historical retain the original design direction without
claiming that unfinished capabilities exist today.

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

Scorpion's crawling core is inherited from Spider and remains unmodified in
license terms. This repository's `LICENSE` (MIT,
`Copyright (c) 2026 Spider Contributors`) is preserved byte-for-byte and
governs the inherited codebase. Spider's own attribution — project name,
license text, authorship, inherited crate names/import paths, and relevant
upstream repository, crates.io, docs.rs, and homepage links — is not to be
stripped or obscured as Scorpion-specific work lands. Fork-specific package
metadata may identify this repository as its actual source while retaining
that attribution. Where Scorpion adds new source files or crates, they inherit
the same MIT terms; this document does not change licensing.

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

## 3. Evidence-First Contract

### Current implementation

The canonical identity module currently owns `EvidenceId`, `ResearchId`,
`WatchId`, and `AuthSessionId`. `CrawlId`, `FetchId`, and `ArtifactId` remain
unimplemented concepts and must not be introduced merely for symmetry.

- `EvidenceId` identifies one immutable canonical `EvidenceBundle`.
- `ResearchId` identifies exactly one durable research invocation.
- `WatchId` identifies one canonical watch definition/state chain.
- `AuthSessionId` identifies one canonical authenticated-session lifecycle.

Canonical durable research owns this lineage:

```text
ResearchId
→ ResearchSession
→ DurableResearchResult
→ Source N + EvidenceRef
→ EvidenceId
→ EvidenceBundle
```

Durable results retain extracted facts, missing evidence, extraction metadata,
final synthesis, synthesis token usage, and citation bindings. `Source N` is a
presentation-local ordering label, not an identity. Full deterministic replay
is not implemented by durable result persistence.

The canonical evidence bundle can retain the applicable source facts below:

- requested URL and final URL (redirect-resolved)
- retrieval timestamp
- response status
- declared and detected content type
- textual content in the requested format
- readable/derived content where produced
- links
- screenshot where produced
- content hash and screenshot hash
- transport used (direct / proxy / Tor — see §7)
- DNS, backend, and response-origin provenance where observed

Not every acquisition populates every optional field. The evidence model must
remain truthful about absent data and must not fabricate provenance.

### Historical baseline

At the original fork baseline, `ResearchId`, `EvidenceId`, and a unified
evidence bundle were future vocabulary rather than implemented types. That
historical finding explains the sequence of later identity, evidence-ledger,
research-session, and durable-result frontiers; it is not current status.

---

## 4. Modern Research Status and Roadmap

Durable canonical research, source evidence, research-session identity,
durable results, Source-N evidence bindings, and shipping CLI RUN/SHOW are
implemented. The remaining future capability directions include:

- adaptive / focused research crawling (steer the crawl toward a research
  goal rather than exhaustive breadth-first traversal)
- browser/DOM/network evidence traces (beyond final rendered HTML — what the
  page actually did)
- deterministic/offline replay where possible (re-derive evidence from a
  retained trace without re-fetching)
- first-class non-HTML resources: PDF, JSON, XML, and other document types as
  primary evidence, not incidental byproducts
- richer research/discovery graph lineage beyond the currently durable ordered
  evidence and Source-N bindings
- deterministic change tracking (has this evidence changed since last seen,
  and how)

---

## 5. Authenticated Research

**Current implementation:** canonical `AuthSessionId`, authentication-profile
vocabulary, durable `AuthSessionState`, explicit pause/resume/invalidate
transitions, and browser-continuity validation exist. Concrete authentication
flows (form submission, OAuth exchange, credential loading, and browser login
execution) remain future work.

**`AuthenticationProfile` variants:**

- `NONE`
- `FORM_LOGIN`
- `BASIC_AUTH`
- `BEARER_TOKEN`
- `COOKIE_SESSION`
- `OAUTH`
- `INTERACTIVE_BROWSER`

**Non-negotiable rules for concrete authentication flows:**

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

**Canonical transport profiles:**

- `DIRECT`
- `PROXY`
- `TOR`

**Current status:** canonical Tor transport is implemented and feature-gated.
It remains fail closed:

- no direct HTTP fallback
- no local DNS fallback
- no direct Chrome/WebDriver fallback
- no clearnet subresource fallback
- no unsafe redirect fallback

"Fail closed" means: if the Tor path cannot be established or verified for
any request or subresource, that request fails — it does not silently
complete over clearnet.

**Historical baseline:** before canonical Tor could be enabled, the original
audit found the following issues (retained here as historical rationale):
Spider's existing proxy-handling paths (HTTP client, Chrome CDP context, and
their DNS resolution) were found to have silent-fallback and scheme-handling
gaps — a failed or misconfigured proxy can currently fall through to a
direct connection rather than hard-failing, and a non-standard `socks://`
scheme is silently rewritten to `http://` in more than one place. Chrome/
WebDriver launches were also found to have no WebRTC leak mitigation. All of
these were exactly the kind of clearnet-fallback behavior the fail-closed rule
above forbids. Canonical Tor was implemented only after its own transport and
guardrail frontiers; retained upstream compatibility paths are not authority
for canonical Tor execution.

---

## Self-Hosting Contract

Scorpion core must remain independently self-hostable. Core functionality
must not require a spider.cloud account, a spider.cloud API key, or any other
proprietary Spider hosted service. Hosted/external providers (Spider Cloud,
third-party search APIs, etc.) may exist only as explicit, optional
integrations layered on top of a core that already works without them.

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

---

## 9. Media & Content Discovery

This section locks *concepts*, not implementation — same status as §3's
identifier concepts. Nothing here is built yet; it fixes vocabulary and a
canonical shape so that movie/series/book/video/image discovery, and future
provider adapters, land against one model instead of several incompatible
ones.

**Architecture rule — the core model is source-neutral.** Scorpion does not
bake a fixed provider allow-list, a fixed provider deny-list, or any
provider-specific policy into the canonical content model, and no named
media/catalog/streaming service is ever a hard dependency of Scorpion core.
A source discovered through the configured search/discovery path is
represented as source/provenance data attached to a result — it is never
part of the identity of the content itself, and it is never singled out by
name in this contract. This is the same posture the Self-Hosting Contract
(above) already takes toward hosted services generally; this section
extends it to media/content discovery specifically.

### 9.1 `MediaType` vs. `ContentKind`

Two separate concepts, not to be collapsed:

- **`MediaType`** — the technical representation: `WEB`, `IMAGE`, `VIDEO`,
  `PDF`, `DOCUMENT`.
- **`ContentKind`** — the semantic classification: `MOVIE`, `TV_SERIES`,
  `BOOK`, `ARTICLE`, `VIDEO`, `IMAGE`, `DOCUMENT`, `OTHER`.

A single piece of content commonly spans several `MediaType`s under one
`ContentKind`. Example: a `MOVIE` may have an `IMAGE` poster, a `VIDEO`
trailer, a `WEB` metadata page, and one or more discovered source/
availability locations — one content identity, several associated
artifacts.

### 9.2 Content result shapes (conceptual only)

No final Rust serialization or canonical IDs are defined here — see §3's
`ArtifactId`/`EvidenceId` status for the precedent.

```
MovieResult {
    title, original_title?, year?, description?, genres?, creators?,
    cast?, runtime?, rating?, poster_url?, backdrop_url?, trailer_url?,
    source_page?, provenance?
}

SeriesResult {
    title, year_or_period?, description?, genres?, creators?, cast?,
    seasons?, poster_url?, trailer_url?, source_page?, provenance?
}

BookResult {
    title, subtitle?, authors?, publication_year?, description?, isbn?,
    publisher?, cover_url?, source_page?, provenance?
}
```

Books may be discovered through general web search, dedicated providers,
catalogs, indexed pages, or future provider adapters — no single commercial
book service may be required by Scorpion core.

### 9.3 Video and image discovery

```
VideoResult {
    title, url, thumbnail_url?, description?, creator_or_channel?,
    published_at?, duration?, source?, provenance?
}

ImageResult {
    title?, image_url, thumbnail_url?, source_page?, width?, height?,
    mime_type?, description?, provenance?
}
```

First-class video discovery must be supported, including video-hosting
platforms as one discoverable source among others — never a hardcoded
requirement. Self-hosted SearXNG media/video search is the preferred initial
direction where practical, matching the Self-Hosting Contract; provider-
specific adapters may be added later without changing the canonical
`VideoResult` shape.

Image discovery explicitly distinguishes two different artifact/evidence
concepts that must not be merged: a **`PAGE_SCREENSHOT`** (Scorpion's own
capture of a page it rendered — see the MCP screenshot capability) versus a
**`DISCOVERED_IMAGE`** (an image found via search/discovery, not captured by
Scorpion). One is evidence Scorpion produced; the other is a result Scorpion
found.

### 9.4 Source / availability discovery

Content identity and discovered source locations are separate concepts —
this mirrors §3's separation of evidence from the thing evidenced.

```
Availability {
    content_reference, source, url, source_page?, availability_type?,
    metadata?, provenance?
}
```

A single `MovieResult`/`SeriesResult`/`BookResult` may have zero, one, or
many `Availability` records. Discovering the same content from multiple
sources must never fork it into separate content identities — one movie,
many possible availability records, one `MovieResult`.

Availability may be discovered through general web search, configured
search providers, a source/domain hint supplied by a downstream consumer,
future provider adapters, or previously discovered pages. No specific
source is mandatory, and none is named or privileged in this contract.

### 9.5 Discovery modes

Three conceptual modes:

- **General discovery** — `query → search/discovery → candidate
  sources/results`.
- **Source-hinted discovery** — `query + source_hint/domain_hint →
  discovery biased toward that source/domain → candidate results`. The
  caller does not need to know the exact content URL, only a hint toward
  where to look.
- **Direct URL** — `explicit URL → fetch/browser/crawl`, bypassing
  discovery entirely.

### 9.6 Example flow (illustrative, not a spec)

```
"I want to watch a dark science-fiction movie"
    ↓
downstream consumer
    ↓
Scorpion content discovery
    ↓
MovieResult candidates
    ↓
optional poster/trailer discovery
    ↓
optional source/availability discovery
    ↓
Availability results
    ↓
downstream consumer decides presentation
```

A second request shape: `content_kind = MOVIE`, `query = "<title>"`,
`source_hint = "<operator-supplied source>"` — Scorpion discovers matching
URLs from that source where possible. The source is always supplied or
discovered, never assumed by Scorpion itself.

### 9.7 Generic media result

The umbrella shape underlying 9.2–9.4, retained for cases that don't need a
`ContentKind`-specific structure:

```
MediaResult {
    media_type, content_kind?, source, title, url, thumbnail_url?,
    description?, creator_or_channel?, published_at?, duration?,
    mime_type?, source_page?, provenance?
}
```

Conceptual only — a discovery representation, not yet the final canonical
evidence schema §3 will eventually define.

### 9.8 Responsibility boundary

Media/content discovery is additive to §2's Ownership Boundary, not a
separate contract: Scorpion owns content discovery, media discovery,
source/availability discovery, structured metadata, URLs, provenance,
optional subsequent fetch/browser/evidence capture, and MCP/API exposure of
all of it. Scorpion does not own conversational presentation — a downstream
consumer decides how discovered results are described or offered to a user
("I found a movie you may like," "here's a trailer," "here's where it's
available"). That framing logic stays outside Scorpion, per §2's existing
"Scorpion returns evidence and facts" rule.

### 9.9 Search vs. fetch, and self-hosting

Discovery and retrieval stay two deliberate steps, and the existing
Self-Hosting Contract applies without modification:

- **Search vs. fetch:** discovery producing candidate results (this
  section) is separate from fetch/crawl/browser producing content/evidence
  (§3). A discovered movie, series, book, image, or video result must not
  automatically trigger fetching, downloading, or opening it.
- **Self-hosting:** self-hosted/open discovery paths remain first-class,
  and Scorpion core must not require any single external
  discovery/media/catalog provider — consistent with how SearXNG is
  already the preferred self-hosted search path (see the Self-Hosting
  Contract above).

### 9.10 Source integrity

Every discovered media/content/availability result must preserve where it
came from. Source/provenance must survive transformation into structured
result models — discovery must not erase its own trail, matching §3's
provenance fields for evidence generally.
