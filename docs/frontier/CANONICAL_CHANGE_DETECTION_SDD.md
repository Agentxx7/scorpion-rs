# Canonical change detection

Frontier: `SCORPION_CANONICAL_CHANGE_DETECTION_001`

Baseline: `4bb5af0d`

## Purpose

Track 9 of the frozen roadmap — the successor boundary Track 7's own
closure deferred alongside scheduling: *"no `ChangeResult`/`ChangeEvent`
... no health, no notification system... those remain later, separate
frontiers."* This frontier realizes exactly the first two, and no
further: no health, no notifications, no generic event framework, no
`Job`/`Operation`, no second `Evidence`/`Watch` model, no new acquisition
path, and no scheduler of its own — Track 8
(`features/watch_schedule.rs`) remains the sole scheduler/execution
owner.

## A. ChangeResult model

`spider/src/features/change_detection.rs` (new module, gated behind
`evidence`+`disk` — like `watch.rs`, which it reads):

```rust
enum ChangeResult {
    Unchanged { basis: ComparisonBasis },
    Changed { basis: ComparisonBasis, previous_hash: String, current_hash: String },
    Uncomparable { reason: UncomparableReason },
}
```

`compute_change_result(previous: &EvidenceBundle, current: &EvidenceBundle) -> ChangeResult`
is a pure function. `ComparisonBasis` names which of `EvidenceBundle`'s
existing hash fields was actually compared
(`ResponseBodyHash`/`TransformedContentHash`); `UncomparableReason` has
exactly one source-justified variant
(`NoConsistentContentSignal`) — with a single code path resolving both
bundles before comparison, there is exactly one way a pure comparison can
fail to produce a definite answer.

## B. ChangeEvent model

```rust
struct ChangeEvent {
    watch: WatchId,
    previous_evidence: EvidenceRef,
    current_evidence: EvidenceRef,
    result: ChangeResult,
    detected_at: SystemTime,
}
```

Fields are private with accessor methods — mirroring
`TransformLineageRecord`'s own precedent — so no caller can fabricate a
`ChangeEvent` (and therefore its content-addressed id) from a
`ChangeResult` it did not actually compute. The only ways to obtain one
are `detect_and_record_change`'s return path and `read_change_event`.

## C. Comparison semantics

`compute_change_result` never invents a new hash: it reuses exactly the
SHA-256 fields `build_evidence` already computed —
`response_body_hash` (raw acquired bytes) preferred, falling back to
`transformed_content_hash` (extracted/transformed text) only when the
former is absent from *both* bundles. Two hashes are only ever compared
when both bundles produced the same basis — comparing a raw-body hash
against a transformed-content hash would not be a truthful signal.
Neither a mismatched basis nor an entirely absent one is ever reduced to
`Unchanged`; both produce `Uncomparable`. Evidence that cannot even be
*resolved* (an `EvidenceRef` naming a record that was never durably
written) is rejected before comparison is attempted at all
(`ChangeDetectionError::PreviousEvidenceUnresolvable`/
`CurrentEvidenceUnresolvable`) — never silently treated as "no change."
Proven directly by dedicated unit tests for each case: equal hashes
(`Unchanged`), differing hashes (`Changed`), fallback-basis agreement
(`Changed` on `TransformedContentHash`), mismatched bases
(`Uncomparable`), and no usable hash on either side (`Uncomparable`).

## D. EvidenceRef / WatchId relationship

`detect_and_record_change` never trusts a caller-supplied `EvidenceRef`
pairing blindly. `watch_evidence_refs` reads `watch`'s own current
`WatchState` (`watch::read_current_watch_state`) plus every superseded
historical value (`DomainPersistence::read_history` at the exact plain
`watch.to_string()` key `apply_watch_transition` already writes to — no
new key convention) and collects every `last_evidence` value ever
associated with that specific watch.
`ensure_evidence_belongs_to_watch` confirms both `previous_evidence` and
`current_evidence` appear in that set before any comparison is
attempted; an unrelated watch's evidence (or a phantom `EvidenceRef` that
was never recorded against any watch at all) is rejected as
`ChangeDetectionError::EvidenceNotAssociatedWithWatch`. Proven by
`evidence_from_an_unrelated_watch_is_rejected` (two real watches, each
with their own recorded evidence, cross-checked) and
`unresolvable_evidence_is_rejected_not_treated_as_unchanged`.

## E. Lineage/fingerprint reuse

`TransformLineageId`/`TransformLineageRecord` (Track 6) model a
materially different fact — `source input → transformation → output` —
and are not reused as a *type* here: a change comparison has no
transformation step and no output distinct from the evidence being
compared. What genuinely is reused: (a) `sha256_hex` itself — no new
hashing/fingerprinting logic anywhere in this module, every hash it
compares was already computed by the existing evidence-construction seam
— and (b) Track 6's own content-addressed, idempotent-duplicate-append
persistence pattern, applied to a structurally identical kind of fact (an
immutable observation about two already-durable inputs, not a lifecycle
state). `spider::configuration::Fingerprint` remains untouched,
unimported, and unreferenced, exactly as Track 6 left it.

## F. Persistence/idempotency semantics

`ChangeEventId` is content-addressed — deterministic SHA-256 of
`(watch, previous_evidence, current_evidence)`, exactly
`TransformLineageId`'s own construction pattern — because the resulting
`ChangeResult` is itself a pure function of that same triple's
already-durable inputs. Persisted through
`DomainPersistence::append_history` only (never `write_current` — a
change-detection fact has no current state to replace), fixed revision
`1`, exactly like Track 6's lineage ledger. Recording an identical
`(watch, previous, current)` fact twice is idempotent, not a conflict:
`Err(PersistenceError::HistoryAlreadyExists) => Ok(event)`, reusing Track
6's own precedent verbatim. Proven by
`recording_the_identical_comparison_twice_is_idempotent_not_a_conflict`
and `different_facts_never_silently_collapse`.
`ChangeResult` computation is kept separate from `ChangeEvent`
persistence: `compute_change_result` is plain and synchronous (cannot
touch `DomainPersistence` at all); `detect_and_record_change` is the only
function in this module that performs I/O, and it performs its three
steps (association check, comparison, persistence) as visibly separate
calls, never fused into one un-inspectable operation.

## G. Real Track 8 production-path verification

A dedicated `production_path` test submodule (gated `#[cfg(feature =
"cron")]`, since it depends on `features::watch_schedule`) drives change
detection entirely through two real `execute_scheduled_watch_run` calls
against a local HTTP fixture — never hand-built `EvidenceBundle` values:

- `change_detection_over_two_real_track_8_scheduled_runs` — a fixture
  serving `"version one"` then `"version two"` across two real scheduled
  ticks; asserts `ChangeResult::Changed` with hashes matching
  `sha256_hex` of each real served body.
- `no_change_across_two_real_track_8_scheduled_runs_of_identical_content`
  — a fixture serving the same body twice; asserts
  `ChangeResult::Unchanged`.

This is also the direct proof for "Track 9 must not own scheduling":
Track 9's own tests never call anything but Track 8's public
`execute_scheduled_watch_run`/`define_watch_schedule` to produce the
evidence it compares.

## Not implemented here

Per this frontier's explicit scope: no health, no notification system, no
generic event framework, no `Job`/`Operation`, no second `Evidence`/
`Watch` model, no new acquisition path, and no scheduler of its own — a
future frontier may wire `detect_and_record_change` into an actual
CLI/MCP surface or into Track 8's own execution path automatically;
neither is done here.

## Acceptance summary

- `spider/src/features/change_detection.rs` — new module: `ChangeResult`,
  `ChangeEvent`, `ChangeEventId`, `ChangeDetectionError`,
  `ComparisonBasis`, `UncomparableReason`, `compute_change_result()`,
  `detect_and_record_change()`, `read_change_event()`; 12 unit/
  persistence/idempotency tests plus 2 real Track-8-production-path
  tests (14 total, cron-gated subset included).
- `spider/src/features/mod.rs` — `pub mod change_detection;` gated behind
  `#[cfg(all(feature = "evidence", feature = "disk"))]`.
- `SCORPION_ARCHITECTURE.md` — new §3.16, §3.8's WATCH/MONITOR row
  updated, §7.6 and §11 updated.
- `spider/tests/architecture_guardrails.rs` — 10 new guardrails:
  exactly-one-owner proofs, the same-watch-association proof, the
  never-defaults-to-unchanged proof, the hash-reuse/no-new-fingerprint
  proof, the append-only/content-addressed-idempotent-persistence proof,
  the computation/persistence-separation proof, the
  Track-8-remains-sole-scheduler proof, the out-of-scope-capability
  absence proof, and the no-shadow-model-in-CLI/MCP proof.
- 193/193 architecture guardrails pass (with and without `cron`); 14/14
  new `change_detection` unit tests pass with `basic evidence disk cron`
  (12/12 core tests pass with `basic evidence disk`, production-path
  tests correctly gated out); 799/799 lib tests pass with `basic evidence
  disk cron`; `cargo check --workspace` clean; `cargo fmt --check` clean;
  `cargo clippy --lib --tests -D warnings` clean (confirmed against the
  same pre-existing, unrelated baseline errors as every prior frontier in
  this session); `git diff --check` clean.

## Successor boundary

This frontier realizes `ChangeResult`/`ChangeEvent` computation and
persistence only. Explicitly out of scope, left for later, separate
frontiers: health, a notification system, wiring change detection into
an automatic post-scheduled-run step, and any CLI/MCP surface for
triggering or reading change events.
