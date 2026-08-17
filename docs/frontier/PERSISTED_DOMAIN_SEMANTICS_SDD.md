# Persisted domain semantics

Frontier: `SCORPION_PERSISTED_DOMAIN_SEMANTICS_001`

Baseline: `04d5d3a8fa1ab53a359ca281fef74a254eb05406`

## Purpose

Track 1 (`SCORPION_PERSISTED_DOMAIN_IDENTITY_001`) gave Scorpion identity
for its two locked persisted-domain concepts — `EvidenceId`, `WatchId` —
but deliberately stopped there: identity names a thing, it does not say
what "having state" means for that thing. `SCORPION_SDD.md` §5.2 already
locks the shape of the future WATCH/MONITOR chain (`WatchId →
WatchDefinition → WatchState → Snapshot → Transition → Event/Result →
persisted updated state`) and the rule that governs it ("Transitions are
explicit, typed, and persisted; no implicit state drift"), but nothing in
the codebase yet says what a transition *is*, what "current" versus
"historical" means, or where the line sits between a domain deciding a
transition is valid and a persistence layer merely storing the result.
Without that vocabulary, every future stateful capability — WatchState,
authenticated-session lifecycle, the evidence ledger, change detection —
would either block on re-deriving it from scratch or, worse, each invent
its own slightly different version. Track 2 builds that vocabulary once,
generically, with no concrete state machine attached to it yet.

## Canonical model

`spider/src/features/domain_state.rs` defines four things, and only four:

1. **`CurrentState<Id, S>`** — the current state of one identity. A plain
   value holding exactly one `identity: Id` and one `state: S`; there is
   no variant of this type, no method, and no companion type in this
   module that can represent more than one live state for the same
   identity. Read access is through `identity()`/`state()`; the only way
   to consume it further is `into_parts()` or `apply()`.
2. **`Transition<S>`** (trait) — the explicit, typed act of computing a
   new state from a current one: `fn apply(&self, current: &S) ->
   Result<S, Self::Rejection>`. The signature takes only `&S` — no
   identity, no storage handle, no I/O capability — so a `Transition` impl
   is structurally incapable of touching persistence, no matter how it is
   implemented.
3. **`HistoryEntry<Id, S>`** — one immutable record of what used to be
   current for `identity`, and when (`recorded_at: SystemTime`). No public
   constructor exists; the only way to obtain one is as the `superseded`
   field `CurrentState::apply` returns. No method mutates any field after
   construction.
4. **`HistoryLog<Id, S>`** — an append-only sequence of `HistoryEntry`
   values. `append` is its only mutating method; there is no `remove`,
   `clear`, `get_mut`, or `IndexMut` anywhere in the type.

## Transition model

The canonical contract from the frontier brief — `current state + explicit
transition → new current state` — is `CurrentState::apply`:

```rust
pub fn apply<T: Transition<S>>(
    self,
    transition: &T,
) -> Result<Applied<Id, S>, (Self, T::Rejection)>
```

`Applied<Id, S>` bundles the new `CurrentState` together with the
`HistoryEntry` recording exactly what it superseded — success produces
both halves of the contract (new current state, plus the historical record
of the old one) from a single call, so nothing can apply a transition and
forget to record what it replaced. On rejection, `apply` hands back the
*original*, byte-for-byte unchanged `CurrentState` alongside the
transition's own `Rejection` value — a rejected transition never partially
applies, and the caller never loses its handle on the current state.

## Current-vs-historical semantics

- **One current state per identity**: enforced by `CurrentState<Id, S>`'s
  shape (never a collection) and by `apply` consuming `self` and producing
  exactly one new `CurrentState` — there is no operation in this module
  that produces two live current states from one. What this module cannot
  enforce — that only one *persisted row* exists per identity in whatever
  future storage holds it — is explicitly persistence's job, not this
  module's (see "Ownership boundary").
- **Historical records are immutable and append-only**: `HistoryEntry` has
  no field-mutating method, full stop. `HistoryLog` is the only collection
  this module provides for them, and its entire mutating surface is
  `append`. A `history_log_is_structurally_append_only` guardrail asserts
  `remove`/`clear`/`get_mut`/`IndexMut` never appear in the module, so this
  can't quietly regress.

## Ownership boundary

Per `SCORPION_SDD.md` §5.2's "persisted updated state" step: persistence's
job is to durably hold the current `CurrentState` for each identity (one
row per identity) and durably append each `HistoryEntry` a transition
produces. It calls `Transition::apply` (via `CurrentState::apply`) to get
the domain's decision; the trait signature makes it structurally
impossible for that decision to itself reach into storage. Domain code
decides validity; persistence stores the result. Neither side is
implemented here — there is no database, cache, or file I/O anywhere in
`domain_state.rs`.

## Observation/Snapshot naming reconciliation

Before adding this vocabulary, two existing bare-word uses needed
reconciling so the words stay unambiguous project-wide:

- **`Observation`** is already `spider_agent_types::PageObservation` — an
  LLM browser-automation tool's page description, a different crate and a
  different domain (agent tool-use, not persisted evidence/watch/session
  state) entirely. This frontier's closest concept — "an immutable record
  of what was true at one point" — is named `HistoryEntry` instead, with
  no relation to `PageObservation`.
- **`Snapshot`** already has two qualified, unrelated owners —
  `VitalsSnapshot` (crawl metrics, `utils/vitals.rs`) and
  `BrowserChallengeSnapshot` (captured CAPTCHA/browser DOM state,
  `features/browser_challenge.rs`) — plus one locked, informal bare use in
  `SCORPION_SDD.md` §5.2's chain. Read in context, that locked step names
  *the freshly captured external reading compared against `WatchState` to
  decide the next transition* — i.e. a future watch-specific input to a
  `Transition<WatchState>` impl, not this module's "current state"
  concept. `domain_state.rs` therefore defines no bare `Snapshot` type; a
  future WatchState frontier realizing §5.2's "Snapshot" step should model
  it as its own input type feeding `Transition<WatchState>`, reusing this
  contract rather than inventing a second "current state" under a
  different name.
- **`Fingerprint`** is a known, separate naming pressure (existing
  browser/anti-detection fingerprinting vs. a future content-fingerprint
  need in Change Detection). Deliberately left unreconciled — Track 6's
  job, not this frontier's.

Both reconciliations are backed by a guardrail
(`no_bare_observation_or_snapshot_types_introduced`) scanning the entire
`spider/src` tree for a bare `struct`/`enum Observation`/`Snapshot`, so a
future frontier cannot silently reopen either collision.

## Acceptance summary

- `spider/src/features/domain_state.rs` — new module: `CurrentState`,
  `Applied`, `Transition`, `HistoryEntry`, `HistoryLog`, 6 unit tests
  exercising the full transition contract (success, rejection,
  chained/one-current-state, append-only ordering, immutability,
  decomposition) against a test-local, non-canonical `TestState` enum.
- `spider/src/features/mod.rs` — unconditional `pub mod domain_state;`.
- `SCORPION_ARCHITECTURE.md` — new §3.10, §3.8 WATCH/MONITOR row updated,
  §7.6 updated, §11 coverage bullet added.
- `spider/tests/architecture_guardrails.rs` — 7 new guardrails: exactly-one
  definition site for each of the four types, unconditional module
  declaration, no persistence/product-model implementation, structural
  append-only/immutability enforcement, the transition contract's exact
  signature shape, no bare `Observation`/`Snapshot` anywhere in
  `spider/src`, and no shadow types in `spider_cli`/`spider_mcp`.
- 132/132 architecture guardrails pass; 738/738 default-feature lib tests
  pass (was 732; +6 new); `cargo fmt --check` and `cargo clippy --lib -D
  warnings` clean (default and `serde` feature sets); `git diff --check`
  clean; full workspace `cargo check` clean.

## Successor boundary

This frontier defines semantics only. Explicitly out of scope, left for
later, separate frontiers:

- Any database/cache/file persistence mechanism for `CurrentState` or
  `HistoryLog`.
- `WatchDefinition` and a concrete `WatchState`/`Transition<WatchState>`
  product model — WATCH/MONITOR remains BLOCKED per `SCORPION_SDD.md`
  §5.2 exactly as before this frontier, now with the generic contract it
  will parameterize with its own `S`.
- `AuthSessionId` and authenticated-session lifecycle.
- Scheduling of any kind.
- `ChangeResult`/`ChangeEvent` or any change-detection product model.
- Health/liveness semantics.
- Reconciling `Fingerprint` (Track 6).
