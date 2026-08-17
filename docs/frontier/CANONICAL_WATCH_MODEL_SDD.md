# Canonical Watch model

Frontier: `SCORPION_CANONICAL_WATCH_MODEL_001`

Baseline: `ff4c1291`

## Purpose

Track 7 of the frozen roadmap. `SCORPION_SDD.md` §5.2 locked the chain
`WatchId → WatchDefinition → WatchState → Snapshot → Transition → ...`
and named establishing canonical ownership of `WatchDefinition`/
`WatchState` as the condition for WATCH/MONITOR to stop being
**BLOCKED**. Tracks 1-4 (identity, domain state/transition semantics,
domain persistence, the durable evidence ledger) already existed as the
prerequisite seams; this frontier realizes the model itself — and no
further. No scheduler, no change detection (`ChangeResult`/
`ChangeEvent`), no health, no notification system, and no generic `Job`
model.

## A. Target-model reuse

Rule #2 required preferring an existing target concept over inventing
`WatchTarget`/`WatchSpec`. `features/discovery_target.rs`'s
`DiscoveryTarget` (`{ url, kind, discovered_via }`) already names exactly
"a pointer — a URL to acquire later," and its own module doc already
distinguishes it from `SourceItem` ("a content candidate... never a
pointer"), which rules `SourceItem` out on its own terms. No field a
`WatchDefinition` needs is missing, so `WatchDefinition` wraps
`DiscoveryTarget` directly instead of introducing a new type.
`DiscoveryTarget` gained `Serialize`/`Deserialize` derives (feature-gated
behind `serde`, mirroring `SourceItem`'s existing pattern exactly) so it
can be persisted — the only change to a pre-existing type this frontier
made.

## B. Definition vs. state

- `WatchDefinition { target: DiscoveryTarget }` describes *what* is
  watched. It owns no execution history and no mutable lifecycle state;
  `define_watch()` is the only way to create one, persisted once through
  `DomainPersistence::append_history` at a namespaced key
  (`"<id>#definition"`, fixed revision `1`) — immutable thereafter.
- `WatchState` is the canonical *current lifecycle state*, built
  entirely on Track 2's unmodified `CurrentState`/`Transition`/
  `HistoryEntry`/`HistoryLog` contract. It carries no target/URL of its
  own — duplicating that would blur the separation rule #3 required.

## C. Lifecycle vocabulary (source-justified, not invented for symmetry)

Exactly two states: `Active` and `Stopped` (terminal). This is the
minimum vocabulary a watch can have at all once scheduling, change
detection, health, and notification are all correctly out of scope —
reasoned from the frontier's own DO-NOT list backward, not copied from
`AuthSessionState`'s three-state (`Active`/`Paused`/`Invalidated`)
precedent, since no locked doc justifies pausability for watches the way
`SCORPION.md` §5 explicitly justified it for auth sessions. Two
transitions:

- `ObserveEvidence { evidence: EvidenceRef }` — realizes the locked
  chain's "Snapshot" step exactly as `domain_state.rs`'s own doc comment
  prescribed for whichever frontier realized it: a watch-specific input
  type to its own `Transition<WatchState>` impl. Only valid from
  `Active`; updates the single current-evidence pointer (not execution
  history — every superseded `WatchState` is preserved separately via
  `HistoryLog`/`DomainPersistence`'s append-only records).
- `StopWatch` — `Active → Stopped`, preserving whatever evidence was last
  observed. Terminal: no transition in this module leads back out,
  mirroring the precedent `AuthSessionState::Invalidated` already set —
  a caller who wants to watch again defines a new `WatchId`.

With only two states, every transition's `apply()` has exactly two match
arms (the valid target state, or `Terminal`) — there is never a genuine
"wrong non-terminal state" case, so `WatchTransitionRejected` has exactly
one variant (`Terminal`), not an `InvalidFromState`-shaped variant that
would be unreachable dead code.

## D. Persistence

Both halves reuse `DomainPersistence` — never a second persistence
mechanism:

- `define_watch()` persists the `WatchDefinition` via `append_history`
  (namespaced key, immutable, write-once) and, separately, the initial
  `WatchState::Active` via `write_current` (`expected_revision: None`, a
  genuine first write) under the identity's own plain key. Two separate
  `DomainPersistence` calls — the same accepted characteristic
  `apply_session_transition` (Track 5) already has for its own two-call
  sequence.
- `apply_watch_transition()` mirrors `apply_session_transition()`
  exactly: read the current `(revision, state)`, run it through
  `CurrentState::apply` (Track 2, unmodified), write the new current
  state via `write_current` (compare-and-swap against the revision just
  read — a concurrent writer racing this call is rejected via
  `WatchError::ConcurrentModification`, never silently lost), and append
  the just-superseded state via `append_history` (immutable). No blind
  overwrite exists anywhere in this path.

The definition's namespaced key (`"<id>#definition"`) and the state's
plain key (`id.to_string()`) never collide in `DomainPersistence`'s
shared history table — proven directly by
`definition_and_state_keys_do_not_collide`.

## E. EvidenceRef integration

`WatchState::Active`/`Stopped` both carry `last_evidence:
Option<EvidenceRef>` — a 16-byte, `Copy` reference, never a duplicate of
`EvidenceBundle`'s payload. This is the first realization of
`EvidenceRef`'s own module doc's stated intent: "later Watch/Change/
Lineage frontiers can hold one cheaply... without duplicating the
evidence payload" — Track 6's lineage record was the first, this is the
second.

## Not implemented here

Per this frontier's explicit scope: no scheduler deciding *when* a watch
is checked, no `ChangeResult`/`ChangeEvent`, no health, no notification
system, no generic `Job`/`Operation` model, no `WatchTarget`/`WatchSpec`,
no second persistence mechanism, no second evidence model, and no
CLI/MCP surface for defining or reading watches (this frontier defines
the seam; nothing calls it yet).

## Acceptance summary

- `spider/src/features/watch.rs` — new module: `WatchDefinition`,
  `WatchState`, `WatchTransitionRejected`, `WatchError`,
  `ObserveEvidence`, `StopWatch`, `define_watch()`,
  `read_watch_definition()`, `read_current_watch_state()`,
  `apply_watch_transition()`; 11 unit/persistence tests.
- `spider/src/features/discovery_target.rs` — `DiscoveryTarget`/
  `DiscoveryTargetKind` gained `Serialize`/`Deserialize` derives
  (feature-gated behind `serde`, mirroring `SourceItem`'s existing
  pattern); no other change.
- `spider/src/features/mod.rs` — `pub mod watch;` gated behind
  `#[cfg(all(feature = "evidence", feature = "disk"))]`.
- `SCORPION_ARCHITECTURE.md` — new §3.14, §3.8's WATCH/MONITOR row
  updated to **PARTIALLY BLOCKED**, §7.6 and §11 updated.
- `spider/tests/architecture_guardrails.rs` — 9 new guardrails: exactly-
  one-owner proofs for every new type, `WatchId` reuse (not
  redefinition), definition/state separation, the Track 2 transition-
  contract proof, the Track 3 CAS/append-only persistence proof, the
  non-duplicative `EvidenceRef` proof, the `DiscoveryTarget`-reuse (no
  `WatchTarget`/`WatchSpec`) proof, the out-of-scope-capability absence
  proof, and the no-shadow-model-in-CLI/MCP proof.
- 174/174 architecture guardrails pass; 11/11 new `watch` unit tests
  pass; 774/774 lib tests pass with `basic evidence disk`; 755/755 pass
  with the default feature set; `cargo check --workspace` clean; `cargo
  fmt --check` clean; `cargo clippy --lib --tests -D warnings` clean
  (confirmed against baseline: the handful of `-D warnings` failures
  present are pre-existing, unrelated findings reproduced identically via
  `git stash`); `git diff --check` clean.

## Successor boundary

This frontier realizes `WatchDefinition`, `WatchState`, and their
persistence only. Explicitly out of scope, left for later, separate
frontiers: a scheduler, change detection (`ChangeResult`/`ChangeEvent`),
health, a notification system, and any CLI/MCP surface for defining,
observing, or stopping watches.
