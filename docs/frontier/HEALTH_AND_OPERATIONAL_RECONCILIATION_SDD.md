# Health and operational reconciliation

Frontier: `SCORPION_HEALTH_AND_OPERATIONAL_RECONCILIATION_001`

Baseline: `640a7650`

## Purpose

Track 10 of the frozen roadmap — the last of the health/notifications
pair every prior watch-pipeline track (7/8/9) explicitly deferred.
Notifications remain out of scope; this frontier realizes truthful,
purely observational operational health for the complete pipeline:
`WatchDefinition → Scheduling → canonical acquisition → EvidenceRef →
WatchState → ChangeResult/ChangeEvent`. Health reports what is actually
known, never infers success from configuration alone, and owns none of
the capabilities it reports on.

## A. Health model

`spider/src/features/watch_health.rs` (new module, gated behind
`evidence`+`disk`+`cron` — the same triple as `watch_schedule.rs`, which
it reads): `WatchHealthReport { watch, scheduling, execution, evidence,
change_detection }` with accessor methods, produced by
`assess_watch_health(store, watch) -> Result<WatchHealthReport,
WatchHealthError>`. Fails closed (`WatchNotFound`) if the watch was
never defined at all — there is nothing to assess.

## B. Health vocabulary

`HealthStatus { Unknown, Healthy, Degraded, Failed }` — four variants,
but not every dimension can truthfully reach every one purely
observationally, and none is forced "for symmetry":

- **Scheduling**: `Unknown` (no `WatchSchedule` recorded) or `Healthy`
  (one is recorded — Track 8's own `define_watch_schedule` already
  validates cadence fail-closed before persisting, so a recorded
  schedule is never itself malformed).
- **Execution**: `Unknown` (no `ObserveEvidence` transition has ever
  occurred) or `Healthy` (at least one has). `Degraded`/`Failed` are
  deliberately not derived from Track 8's private per-run claim
  bookkeeping — reaching into that would blur the ownership boundary
  this module must respect.
- **Evidence production**: the one dimension where all four are
  genuinely reachable — `Unknown` (nothing observed), `Healthy` (current
  and every historical evidence value resolve), `Degraded` (current
  resolves but a historical value does not — a real, discoverable
  integrity gap), `Failed` (current itself does not resolve).
- **Change detection**: reported as `ChangeDetectionReadiness`, not a
  bare `HealthStatus` — see D below.

## C. Production-readiness semantics

`ChangeDetectionReadiness::ProductionExercised { status, most_recent_change_event }`
is only ever constructed inside the branch of `assess_change_detection`
that actually found a durable `ChangeEvent` via
`change_detection::read_change_event` for the watch's most recent
consecutive evidence pair. `status` maps `ChangeResult::Changed`/
`Unchanged` to `Healthy` and `ChangeResult::Uncomparable` to `Degraded` —
never `Unknown` (a record was found) or `Failed` (nothing in
`ChangeResult` represents a comparison failure distinct from
`Uncomparable`).

## D. Track 8/9 reconciliation

Rule #4's central requirement — type-level readiness must never be
reported as the same thing as production exercise — is enforced
structurally, not by convention: `ChangeDetectionReadiness::TypeLevelReady`
and `::ProductionExercised` are different enum variants, so a caller
cannot collapse them into one boolean or status value even by accident.
`TypeLevelReady` covers both sub-cases honestly: fewer than two evidence
observations exist yet (nothing to compare), or two or more exist but no
real `ChangeEvent` has actually been recorded for the most recent pair.
Proven directly by `change_detection_stays_type_level_ready_until_actually_recorded`:
two real evidence observations are produced via real
`execute_scheduled_watch_run` calls, health is asserted `TypeLevelReady`
*before* `detect_and_record_change` is ever called, then
`ProductionExercised` only *after* it actually is. Checking "was this
exact pair already compared" reuses `ChangeEventId::derive` (widened from
module-private to `pub(crate)` by this frontier — see F) rather than
recomputing the comparison, which would mean Health owning change
computation.

## E. Evidence/history inputs

`ordered_watch_evidence` builds the chronological (oldest → current)
sequence of every `EvidenceRef` a watch has ever observed by reading
Track 7's own append-only history (`DomainPersistence::read_history` at
the plain `watch.to_string()` key `apply_watch_transition` already
writes to) followed by the current `WatchState`
(`watch::read_current_watch_state`) — no new index, no second history.
This single ordered list feeds both evidence-production health (last
element = current; earlier elements = historical) and change-detection
readiness (last two elements = the most recent comparable pair).

## F. Ownership boundary

`assess_watch_health`'s production code — everything outside
`#[cfg(test)] mod tests` — never calls
`apply_watch_transition`/`execute_scheduled_watch_run`/
`define_watch_schedule`/`define_watch`/`detect_and_record_change`/
`record_evidence`/`append_history`/`write_current`. It calls only:
`watch::read_watch_definition`, `watch::read_current_watch_state`,
`DomainPersistence::read_history`, `watch_schedule::read_watch_schedule`,
`EvidenceRef::resolve`, `change_detection::read_change_event`. This is
guardrailed directly (`watch_health_is_observational_only_never_a_write_owner`
scans the module's own source, excluding its test fixtures which
legitimately drive real watches/schedules/runs/comparisons to exercise
health against real data). `WatchHealthReport` never embeds a
`WatchState`, `EvidenceBundle`, or `ChangeEvent` — the most recent
comparison is referenced by `ChangeEventId` only, resolved on demand by a
caller through `change_detection::read_change_event`, the same
"reference, not payload" discipline every prior track has used for
`EvidenceRef`.

`ChangeEventId::derive` was widened from module-private to `pub(crate)`
in `change_detection.rs` — the only change to a previously-closed track
this frontier makes, and purely additive/visibility-only (the formula
itself is unchanged, and no crate-external caller gains access). This
was required precisely because Health must check "was this exact pair
already compared" without recomputing the comparison itself, which would
mean owning change computation (rule #6).

## Provider-health reconciliation (rule #2)

`ProviderDescriptor`/`ProviderCapabilities` (`features/source_provider.rs`)
were inspected: pure declarative metadata (`id`, `display_name`,
`capabilities`) with no runtime or health state of their own, and no
`*Health`/`*Status` type exists anywhere else in the crate for providers
or sources. There is no existing type `HealthStatus` could meaningfully
extend without forcing an unrelated shape onto watch-pipeline health, so
it is not routed through `ProviderDescriptor`. What is honored is the
same design discipline — pure, declarative, execution-free — and no
second provider-health architecture is introduced anywhere; guardrailed
directly (`no_second_provider_health_architecture`).

## Not implemented here

Per this frontier's explicit scope: no notifications, no generic
monitoring framework, no `Job`/`Operation`, no second `Watch`/`Evidence`/
`Change` model, and no redesign of transport/providers. A future frontier
may wire `assess_watch_health` into an actual CLI/MCP surface; that is
not done here.

## Acceptance summary

- `spider/src/features/watch_health.rs` — new module: `HealthStatus`,
  `ChangeDetectionReadiness`, `WatchHealthReport`, `WatchHealthError`,
  `assess_watch_health()`; 8 unit tests.
- `spider/src/features/change_detection.rs` — `ChangeEventId::derive`
  widened from module-private to `pub(crate)` (no other change).
- `spider/src/features/mod.rs` — `pub mod watch_health;` gated behind
  `#[cfg(all(feature = "evidence", feature = "disk", feature = "cron"))]`.
- `SCORPION_ARCHITECTURE.md` — new §3.17, §3.8's WATCH/MONITOR row
  updated, §7.6 and §11 updated.
- `spider/tests/architecture_guardrails.rs` — 8 new guardrails: exactly-
  one-owner proofs, the module-gate proof, the observational-only/
  no-write-ownership proof (production code only), the
  type-level-vs-production-exercise structural-non-conflation proof, the
  no-duplication proof, the no-second-provider-health-architecture proof,
  the out-of-scope-capability absence proof, and the no-shadow-model-
  in-CLI/MCP proof.
- 201/201 architecture guardrails pass (with and without `cron`); 8/8 new
  `watch_health` unit tests pass; 807/807 lib tests pass with `basic
  evidence disk cron`; `cargo check --workspace` clean; `cargo fmt
  --check` clean; `cargo clippy --lib --tests -D warnings` clean
  (confirmed against the same pre-existing, unrelated baseline errors as
  every prior frontier in this session); `git diff --check` clean.

## Successor boundary

This frontier realizes purely observational health assessment only.
Explicitly out of scope, left for later, separate frontiers:
notifications, a generic monitoring framework, and any CLI/MCP surface
for reading health.
