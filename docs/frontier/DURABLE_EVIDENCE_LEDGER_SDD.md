# Durable evidence ledger

Frontier: `SCORPION_DURABLE_EVIDENCE_LEDGER_001`

Baseline: `e121153f21161f0fe91718650e62a8d06bd0e7e8`

## Purpose

Tracks 1–3 gave Scorpion identity (`EvidenceId`), state/transition
semantics, and one persistence seam (`DomainPersistence`) — all deferred
by design from any real capability. Evidence, however, already exists as
a real, working capability: `spider/src/utils/evidence.rs`'s
`EvidenceBundle`/`build_evidence()` have produced truthful, non-fabricated
retrieval evidence since before this roadmap began. Track 4 is the first
track to actually connect the three prior tracks to a concrete
capability — durably recording that evidence, immutably, without
inventing a second model to do it or a second store to hold it.

## A. Ledger ownership

`spider/src/utils/evidence.rs` — the existing canonical evidence module —
gains the ledger. No new file, no new module. `EvidenceBundle` remains the
one canonical evidence model (already asserted by the pre-existing
`evidence_bundle_model_is_unique` guardrail, unmodified by this
frontier); the ledger functions (`record_evidence`, `read_evidence`) and
the new `EvidenceRef`/`EvidenceLedgerError` types are additions to this
same file's existing ownership, gated behind the `disk` feature (they
depend on `DomainPersistence`), while `EvidenceBundle`'s new `id` field
stays unconditional so the bundle's shape does not change across builds
depending on whether `disk` happens to be enabled.

## B. EvidenceRef model

```rust
pub struct EvidenceRef { id: EvidenceId }
```

A `Copy`, 16-byte wrapper around exactly one `EvidenceId` — nothing else.
`EvidenceRef::new`/`::from(EvidenceId)` construct it (a pure value
operation, not a claim that the referenced record exists);
`EvidenceRef::id()` reads the identity back; `EvidenceRef::resolve(&store)`
looks the record up through the same `read_evidence` every other caller
uses, returning `None` if nothing was ever recorded for that id. It
carries no evidence content, no provenance, no status — later
Watch/Change/Lineage frontiers can hold one cheaply (in a `WatchState`,
an event, a diff) without duplicating or even touching the evidence
payload until they actually need it.

## C. Persisted EvidenceBundle semantics

`record_evidence(store, bundle)`:

1. Assigns `bundle.id` a fresh `EvidenceId::new()`, unless the caller
   already set one (in which case that value is used — see "duplicate
   behavior" below).
2. Serializes the *entire* bundle (now including `id`) to JSON via
   `serde_json` — byte-for-byte what `build_evidence()` produced, nothing
   added, nothing removed.
3. Calls `DomainPersistence::append_history(&id.to_string(), 1, &payload,
   SystemTime::now())` — Track 3's append-only historical semantics,
   **never** `write_current`. Evidence has no "current state" to replace;
   it is immutable and historical from the instant it is captured, so it
   belongs entirely on the historical side of Track 2's identity/current-
   state/historical-record/transition distinction. The fixed revision `1`
   means every `EvidenceId` has exactly one record, ever — there is no
   revision counter to imply a second, later version of "the same"
   evidence.

`read_evidence(store, id)` reads that same record back via
`DomainPersistence::read_history` and deserializes it — no
reconstruction, no re-derivation from `Page` or anywhere else. What was
written is exactly what is read.

`SystemTime::now()` passed to `append_history` is the persistence layer's
own "when was this appended to the ledger" fact (Track 3's concern,
recorded at write time) — kept distinct from `bundle.retrieved_at` (the
domain fact of when the page was actually fetched, already captured by
`build_evidence()` and preserved unchanged inside the payload).

## D. Provenance preservation

`EvidenceBundle` already preserved `transport`/`dns` truthfully (read
from `Page::transport()`, never fabricated). This frontier extends that
same pattern with two fields the frontier brief specifically named as
missing: `backend_provenance` (read from `Page::backend_provenance()` →
`spider_transport::BackendProvenance`) and `response_origin` (read from
`Page::response_origin()` → `spider_transport::ResponseOrigin`) — the
exact canonical provenance types Track 3's own closure report identified
as already stamped by the transport/cache execution seams, just not yet
surfaced into evidence. Both are stringified locally in `evidence.rs`
(`backend_provenance_label`/`response_origin_label`, mirroring
`AcquisitionTransport::label()`'s existing style) and are `None` whenever
`Page` carries no stamp — never invented. A dedicated test
(`record_evidence_never_fabricates_absent_provenance`) proves an
unstamped bundle reads back with every provenance field still absent
after a full record/persist/read round trip.

`retrieved_at`, `observed_status_code`, `source`/`provider`/`query`, and
`links` (the "artifact/reference relationships" already representable —
this frontier defines no *new* artifact-relationship vocabulary, per
scope) were already truthfully captured by the existing `EvidenceBundle`/
`build_evidence()` and are preserved unchanged through the ledger
round-trip, proven directly in
`record_evidence_assigns_id_and_reads_back_truthfully`.

## E. Duplicate/conflict behavior

`record_evidence` invents no conflict logic of its own — it inherits
Track 3's `(identity, revision)` uniqueness constraint unmodified. Because
every evidence write uses the fixed revision `1`, attempting to record a
*second* bundle under an `EvidenceId` that was already written — the
scenario `duplicate_evidence_id_write_fails_closed_and_leaves_original_untouched`
exercises directly — fails closed with
`EvidenceLedgerError::Persistence(PersistenceError::HistoryAlreadyExists)`,
and the original record is left byte-for-byte untouched (asserted by
reading it back after the failed second attempt). Ordinary use never hits
this: `record_evidence` mints a fresh, 128-bit-random `EvidenceId` for
every bundle that doesn't already carry one, so distinct evidence gets
distinct identities as a matter of course
(`distinct_bundles_get_distinct_ids_and_do_not_collide`).

## Not implemented here

Per this frontier's explicit scope: no `WatchDefinition`/`WatchState`, no
scheduling, no `ChangeResult`/`ChangeEvent`, no authenticated-session
lifecycle, no Fingerprint/Lineage, no event sourcing, no second evidence
store, and no rewrite of transport/acquisition. `record_evidence`/
`read_evidence` are the only new call surface; everything upstream of
them (`build_evidence`, `Page`, transport) is unchanged.

## Acceptance summary

- `spider/src/utils/evidence.rs` — `EvidenceBundle` gains `id`,
  `backend_provenance`, `response_origin` (and now derives
  `Deserialize` alongside `Serialize`); `build_evidence()` populates the
  two new provenance fields; new `EvidenceLedgerError`,
  `record_evidence()`, `read_evidence()`, `EvidenceRef` (all behind
  `disk`); 8 new unit tests under a `ledger` submodule plus the existing
  field-completeness test extended to cover the 3 new fields.
- `SCORPION_ARCHITECTURE.md` — §3.6's table gains a "Durable evidence
  ledger" row and a clarification paragraph; §7.6 and §11 updated.
- `spider/tests/architecture_guardrails.rs` — 5 new guardrails (one
  redundant duplicate of a pre-existing `EvidenceBundle`-uniqueness check
  was found while writing these and removed rather than kept
  side-by-side): `evidence_ledger_types_are_defined_exactly_once`,
  `evidence_ledger_writes_are_append_only_never_current_state`,
  `evidence_ledger_never_defines_its_own_conflict_or_lifecycle_logic`,
  `evidence_bundle_provenance_is_read_from_page_never_fabricated`,
  `no_shadow_evidence_model_in_cli_or_mcp`; the pre-existing
  `interfaces_define_no_shadow_domain_models` shadow-pattern list was
  extended with `EvidenceRef`/`EvidenceLedgerError` rather than
  duplicated.
- 145/145 architecture guardrails pass; 14/14 evidence unit tests pass
  (`basic evidence disk` and the narrower `evidence`-without-`disk`
  combination, confirming the ledger surface disappears cleanly without
  the `disk` feature while `id` stays present); 747/747 default-feature
  lib tests pass (unaffected — `evidence` is not a default feature); `cargo
  fmt --check` and `cargo clippy --lib -D warnings` clean across
  `basic+evidence+disk`, plain `evidence` (no `disk`), and default;
  `git diff --check` clean; full workspace `cargo check` clean;
  `spider_mcp`'s 138 existing tests (which construct `EvidenceBundle` via
  `..Default::default()`, unaffected by the new fields) pass unchanged.

## Successor boundary

This frontier connects identity + semantics + persistence to evidence
only. Explicitly out of scope, left for later, separate frontiers:
`WatchDefinition`/`WatchState`, scheduling, `ChangeResult`/`ChangeEvent`,
authenticated-session lifecycle, Fingerprint/Lineage (Track 6), and any
capability that would *consume* an `EvidenceRef` (a future watch or
change-detection frontier holding one to track "the evidence this
decision was based on") — this frontier only makes that reference
constructible and resolvable, not consumed by anything yet.
