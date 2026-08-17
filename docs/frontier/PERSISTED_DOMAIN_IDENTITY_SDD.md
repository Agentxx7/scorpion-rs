# Persisted domain identity

Frontier: `SCORPION_PERSISTED_DOMAIN_IDENTITY_001`

Baseline: `80f1e78d0e58c7cee89ccae3373ea9fd538b3481`

## Purpose

`SCORPION.md` §3 locked the *concept* of `EvidenceId` — "a unit of captured
evidence derived from a fetch" — but explicitly deferred representation:
"None of the identifier types below are defined, typed, or implemented at
this baseline." `SCORPION_SDD.md` §5.2 locked the first link of the
WATCH/MONITOR state-driven capability chain — `WatchId → WatchDefinition →
WatchState → ...` — while declaring the capability itself **BLOCKED** until
a canonical state model exists.

Track 1 of the roadmap is realizing identity for exactly these two locked
concepts, and nothing else: no new identity types "for symmetry"
(`ResearchId`/`CrawlId`/`FetchId`/`SessionId`/`AuthSessionId`/`JobId`/
`OperationId` remain unrealized), no persistence, and no progress on the
still-BLOCKED WATCH/MONITOR state model. Naming vocabulary without a typed
representation invites every future frontier to invent its own ad hoc
string/UUID scheme; this frontier fixes the representation once so later
work has one place to depend on.

## Canonical model

`spider/src/features/identity.rs` defines two structs, `EvidenceId` and
`WatchId`, each backed by 16 opaque bytes (128 bits, UUID-width but not
claimed to be an RFC 4122 UUID). Each type is:

- **Explicitly typed** — `EvidenceId` and `WatchId` are distinct Rust
  structs; the compiler rejects passing one where the other is expected.
  There is no shared generic `Id<Tag>` erasing that distinction.
- **Deterministically serialized** — `Display`/`to_string()` always produces
  `"<prefix>" + 32 lowercase hex chars` (`evid_…` / `watch_…`); there is
  exactly one valid textual form per value.
- **Validated on parse** — `FromStr`/`TryFrom<&str>`/`TryFrom<String>`
  reject anything but that exact form (wrong prefix, wrong length,
  uppercase hex, non-hex bytes, stray whitespace) via `IdentityParseError`.
  An `EvidenceId`'s serialized form is never accepted as a `WatchId` and
  vice versa — the prefix is a hard boundary on the wire, not only in the
  type system.
- **Value-equal** — `PartialEq`/`Eq`/`Hash`/`PartialOrd`/`Ord` all derive
  from the 16 raw bytes; two IDs with the same serialized form always
  compare and hash equal.
- **Cheap and `Copy`** — no heap allocation to hold or compare an ID; the
  `String` only exists transiently when formatting/parsing.

`EvidenceId::new()`/`WatchId::new()` mint a fresh value from
process-local, non-cryptographic entropy (`ahash` + a monotonic counter +
thread id + timestamp — the same dependency-free technique already used
for WARC record IDs in `utils/warc.rs`, chosen so minting behaves
identically regardless of which optional cargo features are compiled in).
Minting is pure value construction: no I/O, no registration, no side
effect visible outside the returned value.

`serde::Serialize`/`Deserialize` are implemented behind the crate's
existing `serde` feature, round-tripping through the same canonical
string — not an additional representation.

## Ownership

One module, unconditionally compiled (no cargo feature gate — persisted
domain identity must exist regardless of which optional stacks are
enabled), owns both types and nothing else. It is declared in
`spider/src/features/mod.rs` alongside the crate's other single-purpose
domain-vocabulary modules (`artifact_reference`, `research_scope`,
`discovery_target`). `SCORPION_ARCHITECTURE.md` §3.9 registers it in the
canonical ownership map with the same shape as every other row: owner,
allowed/forbidden dependencies, public seam, upstream-compat paths (none).
§7.6 (NO SHADOW MODELS) now names `EvidenceId`/`WatchId` explicitly.

## What identity is not

Per `SCORPION_SDD.md` §5.1/§5.2, identity is deliberately kept distinct
from state, transitions, and persistence:

- No database, file, or cache read/write anywhere in the module.
- No `WatchDefinition`/`WatchState`/`Snapshot`/`Transition` — the rest of
  §5.2's chain remains unrealized and BLOCKED exactly as before this
  frontier; only its first link now has a type.
- No `Evidence`/`Watch` record type — `EvidenceId` names a future evidence
  record, it does not hold, store, or resolve one.
- Neither `EvidenceBundle` (`utils/evidence.rs`) nor any other existing
  domain type gained an identity field in this frontier. Wiring identity
  into a domain object, an interface, or a persistence layer is explicitly
  later-track work.

## Acceptance summary

- `spider/src/features/identity.rs` — new module: `EvidenceId`, `WatchId`,
  `IdentityParseError`, private `random_bytes`/`format_id`/`parse_id`
  helpers, 8 unit tests (9 with `serde`).
- `spider/src/features/mod.rs` — unconditional `pub mod identity;`.
- `SCORPION_ARCHITECTURE.md` — new §3.9, §3.8 WATCH/MONITOR row updated to
  note `WatchId` now exists (state model still BLOCKED), §7.6 updated,
  §11 coverage bullet added.
- `spider/tests/architecture_guardrails.rs` — 5 new guardrails: exactly-one
  definition site for each type, unconditional module declaration, no
  persistence/lifecycle implementation inside the module, presence of the
  deterministic-serialization/validation markers, and no shadow
  `EvidenceId`/`WatchId` in `spider_cli`/`spider_mcp`.
- 125/125 architecture guardrails pass; 732/732 default-feature lib tests
  pass (was 724 before this frontier); `cargo fmt --check` and
  `cargo clippy --lib -D warnings` clean (default and `serde` feature
  sets); `git diff --check` clean; full workspace `cargo check` clean.

## Successor boundary

This frontier realizes identity only. Explicitly out of scope, left for
later, separate frontiers:

- Minting/attaching a real `EvidenceId` to `EvidenceBundle` construction.
- The WATCH/MONITOR state model itself (`WatchDefinition`, `WatchState`,
  snapshots, transitions, persisted state) — still BLOCKED per
  `SCORPION_SDD.md` §5.2 until its own canonical-model frontier lands.
- Any persistence layer for either identity (database, cache, file).
- `ResearchId`, `CrawlId`, `FetchId`, `SessionId`, `AuthSessionId`,
  `JobId`, `OperationId`, or any other identity type — each requires its
  own frontier scoped to an actually-locked, actually-needed concept.
- CLI/MCP surfaces that accept or display these identities.
