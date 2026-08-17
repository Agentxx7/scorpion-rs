# Fingerprint reconciliation and transform lineage

Frontier: `SCORPION_FINGERPRINT_AND_TRANSFORM_LINEAGE_001`

Baseline: `d528028ca8f05afa84ca8181961666dba9f32d79`

## Purpose

The roadmap's own naming-collision debt named "Fingerprint" as a known,
separate pressure and deliberately deferred it to this track (Track 2's
closure report: *"deliberately not reconciled here — that belongs to
Track 6"*). Track 6 first resolves that debt, then — only if source
evidence actually requires a new concept — introduces canonical
content/transform lineage on top of Tracks 2-4, reusing exactly what
already exists (`sha256_hex`, `EvidenceRef`, `DomainPersistence`) rather
than building a parallel evidence or identity architecture.

## A. Fingerprint naming reconciliation

`spider::configuration::Fingerprint` (`pub use spider_fingerprint::Fingerprint;`)
is an `enum { Basic, NativeGPU, None }` controlling browser
anti-detection stealth spoofing (WebGL/GPU emulation) for Chrome
automation — owned entirely by the external `spider_fingerprint` crate,
merely re-exported. It has no notion of content, transformation, hashing,
or provenance; it configures how a browser *presents itself*, not what
happened to fetched content afterward. Confirmed: no `struct`/`enum
Fingerprint` is defined anywhere in `spider/src` — the type genuinely
lives entirely upstream. Reconciliation conclusion: the two concepts
share only an English word, not a domain (frontier rule #7 — source
evidence does not support treating them as the same concept). The
`configuration::Fingerprint` re-export is untouched, unimported, and
unreferenced by any new code; the new content/transform identity type is
named `TransformLineageId` — domain-qualified, never bare `Fingerprint`.
Guardrailed directly: `configuration_fingerprint_ownership_remains_intact`
proves the re-export line is unchanged;
`no_bare_fingerprint_type_is_defined_anywhere_in_spider` proves no shadow
definition exists anywhere; `transform_lineage_never_imports_or_references_configuration_fingerprint`
proves the new module never imports it.

## B. Lineage model

`spider/src/features/transform_lineage.rs` (new module, gated behind
`evidence` + `disk` — it needs `sha256_hex` and
`DomainPersistence`/`EvidenceRef`, so it cannot compile without both):

- `TransformationIdentity(String)` — SHA-256 (via `sha256_hex`) of a
  caller-supplied, deterministic transformation description.
- `TransformLineageId(String)` — content-addressed: SHA-256 of
  `(input_hash, transformation, output_hash)`, prefixed `lineage_`.
- `TransformLineageRecord` — the immutable fact: `input_hash`,
  `input_evidence: Option<EvidenceRef>`, `transformation`, `output_hash`,
  `recorded_at`.
- `TransformLineageError`, `record_lineage()`, `read_lineage()`.

## C. Transformation identity semantics

`spider` (the canonical core crate) has no dependency on
`spider_transformations` — that crate belongs to the interface layer
(`spider_cli`/`spider_mcp` only). Rather than pull a transformation
library down into canonical core (which would itself be a form of
"redesigning" the dependency graph this frontier doesn't authorize),
`TransformationIdentity::of(description: &str)` accepts the
transformation's description as an opaque, caller-supplied string — the
same "neutral seam, caller supplies the concrete specifics" pattern
`DomainPersistence` already established for domain state. A caller with
a real `spider_transformations::TransformConfig` derives a deterministic
description from it (its own `Debug` output is sufficient, since
`TransformConfig`'s fields are all plain `bool`/`ReturnFormat`) and
passes that in — this frontier does not perform that wiring itself (see
"Successor boundary").

## D. Input/output linkage

`record_lineage(store, input_bytes, input_evidence, transformation_description, output_bytes)`
hashes `input_bytes`/`output_bytes` itself, via `sha256_hex`, from the
real bytes given — there is no code path accepting a pre-computed hash
string instead, so a lineage record can never be fabricated for content
that was never actually observed (rule #6). `input_evidence`, when
supplied, is an `EvidenceRef` (Track 4) pointing at the durable evidence
record the input came from — stored by reference (16 bytes, `Copy`),
never duplicating the evidence payload, realizing `EvidenceRef`'s own
stated design intent ("later Watch/Change/Lineage frontiers can hold one
cheaply... without duplicating the evidence payload") for the first
time. `TransformLineageId` is a pure function of the
`(input_hash, transformation, output_hash)` triple — deliberately
excluding `recorded_at` and `input_evidence` — which is what the
determinism requirement actually needs: two runs of the identical
transformation on the identical input at different times must produce
the identical lineage identity.
`same_input_and_transformation_produce_stable_lineage_identity` proves
this directly (constructs two records differing only in `recorded_at`
and asserts equal ids); `different_input_does_not_collapse_identity`/
`different_transformation_does_not_collapse_identity`/
`different_output_does_not_collapse_identity` prove the converse for
each of the three links independently.

## E. Persistence semantics

Through `DomainPersistence::append_history` only — never
`write_current` — exactly like Track 4's evidence ledger and unlike
Track 5's authenticated sessions: a lineage fact has no current state to
replace. Every record uses the fixed revision `1`. Because the id is
content-addressed, a duplicate append (the identical input,
transformation, and output triple, presumably from re-running the same
transformation again) is not a conflict:
`Err(PersistenceError::HistoryAlreadyExists) => Ok(id)` treats it as
success, proven directly by
`recording_the_identical_fact_twice_is_idempotent_not_a_conflict`
(records the same fact twice, asserts equal ids, asserts only one
historical record exists — the second call appended nothing). A
genuinely different fact always hashes to a different key, so different
facts never silently collapse — proven by
`different_facts_never_silently_collapse`. No `write_current`, no raw
SQL, and no second `DomainPersistence`-shaped type exist in this module
(guardrailed).

## F. EvidenceRef relationship

Non-duplicative by construction: `TransformLineageRecord.input_evidence`
is `Option<EvidenceRef>` — a 16-byte, `Copy` reference — never a field
holding evidence content, a hash of evidence content beyond
`input_hash` (which the lineage record needs regardless, since the
input's own bytes must be hashed to compute the lineage identity at
all), or any other duplication of `EvidenceBundle`'s payload.
`evidence_ref_is_stored_by_reference_not_duplicated` proves the round
trip preserves exactly the reference's `EvidenceId`, nothing more.
`EvidenceBundle` itself is untouched — no new field, no redesign.

## Not implemented here

Per this frontier's explicit scope: no `WatchDefinition`/`WatchState`, no
scheduling, no `ChangeResult`/`ChangeEvent`, no health, no redesign of
`EvidenceBundle` or transport, no generic content ID "for symmetry" (this
one is specifically the input/transformation/output triple's address,
not a bare arbitrary `ContentId`), and no second evidence architecture
(lineage persists through the exact same `DomainPersistence` Evidence and
AuthSession already use).

## Acceptance summary

- `spider/src/features/transform_lineage.rs` — new module: `TransformationIdentity`,
  `TransformLineageId`, `TransformLineageRecord`, `TransformLineageError`,
  `record_lineage()`, `read_lineage()`; 5 pure-determinism unit tests
  (unconditional within the module's own gate) + 6 persistence-ledger
  tests.
- `spider/src/features/mod.rs` — `pub mod transform_lineage;` gated
  behind `#[cfg(all(feature = "evidence", feature = "disk"))]`.
- `SCORPION_ARCHITECTURE.md` — new §3.13, §7.6 and §11 updated.
- `spider/tests/architecture_guardrails.rs` — 10 new guardrails: the
  Fingerprint reconciliation proof (re-export intact, no bare
  definition anywhere, no import from the new module), exactly-one
  definition site for every new type, correct feature gating, the
  content-addressed (not randomly-minted) identity proof, append-only/
  idempotent-duplicate persistence proof, non-duplicative `EvidenceRef`/
  `sha256_hex` reuse proof, no out-of-scope capability, and no shadow
  model in `spider_cli`/`spider_mcp`.
- 165/165 architecture guardrails pass; 11 new `transform_lineage` unit
  tests pass; 763/763 lib tests pass with `basic evidence disk`; `cargo
  fmt --check` clean; `cargo clippy --lib --tests -D warnings` clean
  (confirmed against baseline: the handful of `-D warnings` failures
  present are pre-existing, unrelated findings in other files —
  `source_provider.rs`, `transport_leaf_acceptance.rs`, and two more
  reproduced identically via `git stash`); `git diff --check` clean;
  full workspace `cargo check` clean.

## Successor boundary

This frontier realizes lineage identity, determinism, and persistence
only. Explicitly out of scope, left for later, separate frontiers:
wiring `record_lineage` into `spider_cli`/`spider_mcp`'s actual
`transform_content_input` call sites (this frontier defines the seam;
nothing calls it yet), `WatchDefinition`/`WatchState`, scheduling,
`ChangeResult`/`ChangeEvent`, health, and any interface (CLI/MCP) surface
for reading recorded lineage.
