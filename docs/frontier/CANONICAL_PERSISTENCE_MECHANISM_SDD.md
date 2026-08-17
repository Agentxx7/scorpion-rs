# Canonical persistence mechanism

Frontier: `SCORPION_CANONICAL_PERSISTENCE_MECHANISM_001`

Baseline: `46ddaaf9afb3782f2ef565da2033d1327569e73f`

## Purpose

Track 2 (`SCORPION_PERSISTED_DOMAIN_SEMANTICS_001`) defined what "current
state," "historical record," and "transition" mean, and drew the
ownership boundary between domain code (decides transitions) and a future
persistence layer (stores the result) — but implemented no storage at
all, by design. Every future stateful capability (WatchState,
authenticated-session lifecycle, the evidence ledger, change detection)
needs somewhere durable to put a `CurrentState`/`HistoryEntry`. Without
one canonical seam, each would either block or build its own ad hoc
storage, immediately reopening the exact "no implicit state drift" risk
`SCORPION_SDD.md` §5.2 names. Track 3 builds that one seam — mechanism
only, no capability.

## Reuse audit

Before writing anything, the existing codebase was searched for storage
primitives already available to build on:

- `spider/src/features/disk.rs` — `DatabaseHandler`, a SQLite-backed
  (`sqlx`, feature `disk`) store already wired into `website.rs` for
  Spider's own crawl-resume/dedup (`resources`, `signatures` tables).
  Confirmed unclassified anywhere in `SCORPION_ARCHITECTURE.md` — it is
  Spider upstream-compatibility machinery, not canonical Scorpion domain
  state, and its schema is unconditionally overwritable/dedup-shaped, not
  transition-aware. Not reusable *as a type* without conflating two
  unrelated concerns, but its *dependency* — `sqlx` + SQLite, already an
  optional-but-effectively-always-on dependency via the `disk`/
  `disk_native_tls` feature chain reachable from `default` — is exactly
  the "suitable existing storage technology" the frontier brief asks to
  reuse.
- `cacache`/`http_cache` (`spider::cache_request`) — a content-addressed
  HTTP response cache keyed by opaque request identity. Not fit for
  purpose: no compare-and-swap semantics, no natural historical-append
  concept, and mixing an HTTP cache with canonical domain persistence
  would itself be a shadow-persistence-path risk.

Conclusion: reuse `sqlx`/SQLite (the dependency and feature gate), build
one new, narrowly-scoped table pair rather than repurposing
`DatabaseHandler`'s schema or introducing a second database crate.

## A. Persistence ownership

`spider/src/features/domain_persistence.rs`, gated behind the existing
`disk` feature (no new feature flag). It owns exactly one type,
`DomainPersistence`, and one error type, `PersistenceError` — both
defined nowhere else, verified by guardrail.

## B. Storage primitive reused or introduced

Reused: `sqlx` + SQLite (already a workspace dependency, `disk` feature).
Introduced: two new tables, owned exclusively by this module —
`scorpion_domain_current_state` (`identity TEXT PRIMARY KEY, revision
INTEGER NOT NULL, state BLOB NOT NULL`) and `scorpion_domain_history`
(`identity TEXT, revision INTEGER, state BLOB, recorded_at_unix_ms
INTEGER, PRIMARY KEY (identity, revision)`). The connection pool is
capped at one connection deliberately: SQLite only ever has one writer
regardless, and a single connection makes the read-then-conditionally-write
sequence inside `write_current` race-free without needing an explicit
`BEGIN IMMEDIATE` — no other query on this seam can interleave between
the read and the write because there is only ever one connection to run
either on.

Neither table, nor `DomainPersistence` itself, imports or depends on
`features/identity.rs` or `features/domain_state.rs`. Every identity is
an opaque `&str` (any canonical identity's `Display` form); every state
is an opaque `&[u8]` — a storage mechanism that had to import a concrete
domain type to compile would already be deciding something about that
domain.

## C. Current-state write semantics

`DomainPersistence::write_current(identity, expected_revision, new_state)`
is the *only* method that changes current state, and it is
compare-and-swap, not overwrite:

- `expected_revision: None` means "no row must exist yet" — succeeds only
  as a genuine first write (revision becomes `1`); fails if a row already
  exists.
- `expected_revision: Some(n)` succeeds only if the revision actually
  stored right now is exactly `n` (advances to `n + 1`); fails otherwise.

There is no second, unconditional "just set it" method anywhere in the
type — verified by a guardrail that fails if `set_current`/
`overwrite_current`/`force_write_current`/`put_current` (or any similarly
named unconditional write) ever appears.

## D. Historical append semantics

`DomainPersistence::append_history(identity, revision, state,
recorded_at)` can only add a record. A `(identity, revision)` pair that
already has one is rejected — the table's own `PRIMARY KEY (identity,
revision)` constraint enforces this at the database level, not
application logic that a future change could accidentally bypass. There
is no `UPDATE`, `DELETE`, `INSERT OR REPLACE`, or `INSERT OR IGNORE`
against the history table anywhere in the module (guardrailed) — a
historical record, once appended, cannot be altered or removed through
this seam by any method.

## E. Stale-write / conflict behavior

Both failure modes return typed, storage-shaped errors and guarantee
nothing was written:

- `PersistenceError::CurrentStateConflict { actual: Option<u64> }` —
  `write_current`'s expectation didn't match; `actual` reports what is
  really stored (or `None`) so the caller can decide whether to retry
  with a fresh read. The read-check-write sequence runs inside one SQL
  transaction; on mismatch the function returns before issuing any
  mutating query, so the transaction rolls back on drop and the row is
  provably untouched (asserted directly in tests: the stored
  revision/state after a rejected write is byte-identical to before it).
- `PersistenceError::HistoryAlreadyExists` — `append_history` hit the
  unique-constraint violation; the pre-existing record is left
  byte-for-byte untouched (also asserted directly in tests).

Both variants are distinguished from `PersistenceError::Backend(sqlx::Error)`,
which covers genuine storage-engine failures (I/O, corruption, connection
loss) unrelated to the conflict semantics above.

## Boundary held

- Persistence decides no transition: `write_current`'s signature has no
  `Transition` parameter of any kind, structurally incapable of running
  one.
- Persistence invents no lifecycle state: the only non-opaque value it
  tracks is a monotonically increasing `u64` revision counter — no
  status, no "active"/"closed," nothing domain-shaped.
- Persistence owns no domain semantics: `state` is always `BLOB`/`&[u8]`;
  no field of any stored value is ever inspected.
- No shadow identity model: identities are plain `&str`; the module never
  constructs, matches on, or imports `EvidenceId`/`WatchId`.

## Not implemented here

Exactly as scoped: no Evidence Ledger product semantics, no
authenticated-session lifecycle, no `WatchDefinition`/`WatchState`, no
Fingerprint/Lineage, no scheduling, no change detection, no health, no
event sourcing, no generic Job/Operation persistence. This module is a
mechanism two future capabilities will call into, not a capability
itself.

## Acceptance summary

- `spider/src/features/domain_persistence.rs` — new module:
  `DomainPersistence`, `PersistenceError`, 9 unit tests (first write,
  blind-second-first-write rejection, stale-revision rejection, correct
  CAS success, unknown-identity read, historical duplicate rejection,
  historical append-only ordering, cross-identity isolation, real-file
  open/reopen round trip).
- `spider/src/features/mod.rs` — `pub mod domain_persistence;` gated
  behind `#[cfg(feature = "disk")]`.
- `SCORPION_ARCHITECTURE.md` — new §3.11 registers the module; §3.8's
  WATCH/MONITOR row updated; new §7.11 (NO BLIND PERSISTENCE WRITES);
  §7.6 and §11 updated.
- `spider/tests/architecture_guardrails.rs` — 8 new guardrails:
  exactly-one definition site for `DomainPersistence`/`PersistenceError`,
  the module gated behind `disk` (not a new always-on stack), no import
  of Track 1/2 types, no domain-semantics/product-model implementation,
  the compare-and-swap-only shape of `write_current`, the fail-closed
  shape of `append_history`, reuse of the existing `sqlx` dependency (not
  a second database crate), and no shadow persistence type in
  `spider_cli`/`spider_mcp`.
- 140/140 architecture guardrails pass; 747/747 default-feature lib tests
  pass (was 738; +9 new — `disk`/`sqlx` is already reachable from
  `default` via the `basic → disk_native_tls → disk` feature chain, so
  no extra feature flag was needed to exercise this in CI); `cargo fmt
  --check` and `cargo clippy --lib -D warnings` clean (default and
  `basic disk` feature sets); `git diff --check` clean; full workspace
  `cargo check` clean.

## Successor boundary

This frontier is mechanism only. Explicitly out of scope, left for
later, separate frontiers: the Evidence Ledger, authenticated-session
lifecycle, `WatchDefinition`/`WatchState`, Fingerprint/Lineage
reconciliation (Track 6), scheduling, change detection, health, event
sourcing, and generic Job/Operation persistence — each is a capability
that would call `DomainPersistence::write_current`/`append_history` after
computing its own transition via Track 2's `CurrentState::apply`, never
by extending this module itself.
