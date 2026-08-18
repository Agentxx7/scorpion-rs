# Scheduling and watch execution

Frontier: `SCORPION_SCHEDULING_AND_WATCH_EXECUTION_001`

Baseline: `d45f4771`

## Purpose

Track 8 of the frozen roadmap — the successor boundary Track 7's own
closure explicitly deferred: *"a scheduler deciding when a watch is
checked... remain later, separate frontiers."* This frontier realizes
cadence and the execution path for one scheduled watch run only, and no
further: no background scheduling daemon, no change detection
(`ChangeResult`/`ChangeEvent`), no health, no notification system, no
generic `Job`/`Operation` model.

## A. Scheduling model

`spider/src/features/watch_schedule.rs` (new module, gated behind
`evidence`+`disk` — like `watch.rs`, it needs `DomainPersistence`/
`EvidenceRef` — plus `cron`, for the cadence primitive):
`WatchSchedule { cron_str: String }`. Immutable, write-once — persisted
via `DomainPersistence::append_history` at a namespaced key
(`"<id>#schedule"`), exactly mirroring `WatchDefinition`'s own pattern
from Track 7. Validated at definition time
(`cron_str.parse::<async_job::Schedule>()`); an invalid cadence is
rejected fail closed (`WatchScheduleError::InvalidCadence`) with nothing
persisted.

## B. Cadence ownership/reuse

The frontier's initial framing named `website::CronType` as the reuse
candidate; inspection showed `CronType` is actually a *what-to-run*
selector (`Crawl`/`Scrape`) for `Website`'s own cron integration — it
carries no cadence syntax at all, so it was ruled out. The actual cadence
primitive `Website`'s cron feature already depends on is
`async_job::Schedule` (`Website::schedule()` parses
`Configuration::cron_str` via `cron_str.parse::<async_job::Schedule>()`,
gated behind the existing `cron` feature). `WatchSchedule` reuses that
exact parser to validate cadence syntax — it does not reimplement cron
parsing, and it does not adopt `async_job::Job`/`async_job::Runner` (a
`Website`-owned, always-running scheduler daemon abstraction): a
`WatchSchedule` is validated, durable data, not a running process. This
realizes "adapt an existing primitive cleanly, do not invent a second
scheduler abstraction" precisely — adopt the parser, not the daemon.
Guardrailed directly: `watch_schedule_reuses_async_job_schedule_not_website_crontype`
proves both the reuse and the non-adoption, as code-shaped patterns (not
prose, since this module's own doc comment legitimately names both
`CronType` and `async_job::Runner` in English to explain the decision).

## C. Watch execution path

`execute_scheduled_watch_run(store, id, scheduled_at, transport_request)`
realizes exactly `WatchDefinition → scheduled trigger → canonical
acquisition → durable EvidenceRef → WatchState transition`:

1. Reads (never redefines) `id`'s `WatchSchedule` and `WatchDefinition`
   (Track 7's own `watch::read_watch_definition`) — fails closed
   (`NoSchedule`/`WatchNotFound`) if either is missing, before any
   acquisition.
2. Acquires through `acquisition_binding::bind`/`::execute` — the same
   seam CLI/MCP fetch already uses (`spider_cli::discovery::run_fetch`'s
   own pattern, reused verbatim: bind a `DiscoveryTarget` +
   `TransportRequest`, execute, read `.page()`, decode UTF-8 content).
   No new `reqwest::Client`, `Website::new`, `.crawl()`/`.scrape()`, or
   Tor client construction anywhere in this module.
3. Builds and durably records evidence through the unmodified
   `utils::evidence` ledger (`build_evidence` + `record_evidence`,
   Track 4).
4. Applies the resulting transition through Track 7's own
   `watch::apply_watch_transition` with `ObserveEvidence` —
   `WatchState`'s variants, transitions, and persistence rules are never
   touched, extended, or redefined here.

Guardrailed: `watch_execution_reuses_watch_definition_and_never_redefines_watch_state`,
`watch_execution_reuses_canonical_acquisition_binding`,
`watch_execution_produces_durable_evidence_via_canonical_ledger`.

## D. Idempotency semantics

"The same scheduled run" is identified by `(WatchId, scheduled_at)`. A
retry for the exact tick that already ran must not duplicate the fetch,
the durable evidence record, or the `WatchState` transition. Enforced by
claiming the run's identity *before* any side effect:
`DomainPersistence::write_current`'s compare-and-swap
(`expected_revision: None` — a genuine first write for this exact run)
against a namespaced key (`"<id>#run#<unix_seconds>"`). Only the claim
winner performs acquisition/evidence/transition, then finalizes the same
identity to `Completed { evidence }` via a second compare-and-swap. A
caller that loses the claim never touches acquisition, evidence, or
`WatchState` at all: a `Completed` record is replayed (the
already-produced `EvidenceRef` returned, zero new work); a
`Claimed`-but-incomplete record (a concurrent or crashed prior attempt)
is rejected fail closed as `WatchExecutionError::RunAlreadyInProgress`
rather than guessing whether duplicating the in-flight work is safe.
Proven directly: `retry_of_the_same_scheduled_run_is_idempotent_and_does_not_refetch`
(a second call for the identical tick returns the same `EvidenceRef`,
performs zero additional HTTP hits, and appends zero additional
`WatchState` history entries);
`different_scheduled_tick_executes_independently_not_suppressed` proves
idempotency is scoped to one exact run, not a global "already ran" flag;
`a_claimed_but_incomplete_run_fails_closed_without_duplicating_work`
simulates a crashed prior claim and proves the retry is rejected without
ever reaching the network. Guardrailed structurally too:
`watch_execution_claims_run_identity_before_any_side_effect` proves (via
source position) that the claim happens strictly before acquisition and
the finalize happens strictly after the transition.

## E. Persistence semantics

Reuses `DomainPersistence` exclusively — no second persistence
mechanism. Durable scheduler-owned state is kept to exactly what
idempotency and cadence discovery actually require: the one immutable
`WatchSchedule` record per watch, plus one claim/completion record per
scheduled run identity. No generic `Job`/`Task`/`Operation` table; no
execution-history log beyond that one current record per run (Track 7's
own `HistoryLog`/`append_history` already preserves every superseded
`WatchState`, so this module does not duplicate that).

## F. EvidenceRef/WatchState integration

Every successful execution (first attempt or idempotent replay) yields
the same `EvidenceRef` — a plain reference, resolvable via
`utils::evidence::read_evidence`, never a duplicate of the evidence
payload. `WatchState` remains owned exclusively by Track 7: this module
holds no `WatchState` variant of its own, and its only interaction with
lifecycle state is a single call to `watch::apply_watch_transition` per
successfully completed run.

## Not implemented here

Per this frontier's explicit scope: no background scheduling daemon
deciding *when* a trigger fires (this module defines what happens once
one does), no `ChangeResult`/`ChangeEvent`, no health, no notification
system, no generic `Job`/`Operation` model, no `WatchTarget`/`WatchSpec`,
no second persistence or evidence mechanism, and no CLI/MCP surface for
defining schedules or triggering runs (this frontier defines the seam;
nothing calls it yet).

## Acceptance summary

- `spider/src/features/watch_schedule.rs` — new module: `WatchSchedule`,
  `WatchScheduleError`, `WatchExecutionError`, `ScheduledRunRecord`,
  `define_watch_schedule()`, `read_watch_schedule()`,
  `execute_scheduled_watch_run()`; 9 unit/persistence/idempotency tests.
- `spider/src/features/mod.rs` — `pub mod watch_schedule;` gated behind
  `#[cfg(all(feature = "evidence", feature = "disk", feature = "cron"))]`.
- `SCORPION_ARCHITECTURE.md` — new §3.15, §3.8's WATCH/MONITOR row
  updated, §7.6 and §11 updated.
- `spider/tests/architecture_guardrails.rs` — 9 new guardrails: exactly-
  one-owner proofs, the cadence-reuse/non-daemon-adoption proof, the
  `WatchDefinition`-reuse/`WatchState`-non-redefinition proof, the
  canonical-acquisition-reuse proof, the canonical-evidence-ledger-reuse
  proof, the claim-before-side-effect/finalize-after-transition ordering
  proof, the out-of-scope-capability absence proof, and the
  no-shadow-model-in-CLI/MCP proof.
- 183/183 architecture guardrails pass (with and without `cron`
  enabled); 9/9 new `watch_schedule` unit tests pass; 785/785 lib tests
  pass with `basic evidence disk cron` (778/778 with `basic evidence
  disk`, unaffected); `cargo check --workspace` clean; `cargo fmt --check`
  clean; `cargo clippy --lib --tests -D warnings` clean (confirmed
  against the same pre-existing, unrelated baseline errors as every
  prior frontier in this session); `git diff --check` clean.

## Successor boundary

This frontier realizes cadence definition, one scheduled run's execution,
and its idempotency only. Explicitly out of scope, left for later,
separate frontiers: a background scheduler daemon that actually decides
when to fire triggers, change detection (`ChangeResult`/`ChangeEvent`),
health, a notification system, and any CLI/MCP surface for defining
schedules or triggering runs.
