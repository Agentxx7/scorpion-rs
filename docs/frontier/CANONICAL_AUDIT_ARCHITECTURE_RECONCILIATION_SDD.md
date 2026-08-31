# Canonical Audit Architecture Reconciliation SDD

Frontier: `SCORPION_CANONICAL_AUDIT_ARCHITECTURE_RECONCILIATION_001`

Baseline: `c6be3cd99b104d15472c0dc8bd352ed37fa55dd1`

## 1. Purpose

This is not an implementation SDD — it specifies no new capability. It
documents what `SCORPION_AUDIT_FACTS_AND_FINDING_CONTRACT_FRONTIER_001`,
`SCORPION_AUDIT_DETERMINISTIC_PAGE_ANALYZERS_001`, and
`SCORPION_AUDIT_OBSERVED_TECHNOLOGY_MARKERS_001` already built and CLOSED at
the baseline SHA above, and reconciles `SCORPION.md`,
`SCORPION_ARCHITECTURE.md`, and `SCORPION_SDD.md` — the ACTIVE product,
architecture, and design contracts — to that verified production reality.
Those three frontiers ran as self-directed operator work outside this
repository's own `docs/frontier/` SDD/acceptance-contract process; the
capability they built is real and CLOSED, but the process artifacts this
repository's convention expects were never produced, and the three ACTIVE
contract documents still predate the capability entirely (verified by
repository-wide grep: zero mentions of `audit`, `Finding`, or "technology
marker" existed in any of the three documents before this frontier). This
frontier is the audit-and-reconciliation step that should have accompanied
those three closures; it does not redesign, extend, or reopen any of them.

## 2. Canonical Model (as already implemented, verified from source)

Owned entirely by `spider/src/features/audit.rs` (`#[cfg(all(feature =
"evidence", feature = "disk"))]`):

- `PageFacts` / `HtmlPageFacts` — transient, network-free, one-`lol_html`-pass
  projections of one acquired `Page`. Never persisted themselves.
- `EvidencedPageFacts` — one `PageFacts` bound to the exact `EvidenceRef`
  naming the durable recording of the same `Page` it was derived from. The
  only constructor is `EvidencedPageFacts::record(store, page)`.
- `Finding` / `FindingId` / `FindingCategory` / `FindingSeverity` /
  `FindingCondition` — a deterministic canonical rule evaluation:
  content-addressed identity (`FindingId`, SHA-256, deliberately not
  registered in `features/identity.rs` — mirrors `ChangeEventId`'s and
  `TransformLineageId`'s own derived-record-identity precedent), an
  explicit `rule_version` independent of crate version, and a durable,
  append-only (`DomainPersistence::append_history`, fixed revision `1`,
  idempotent on duplicate identity) record.
- `ObservedTechnologyMarker` / `TechnologyMarkerSource` — a directly
  observed, technology-identifying value the remote page itself exposed.
  No identity field, no version, no severity, no independent persistence —
  a pure, re-derivable projection of the same already-persisted `Evidence`
  `PageFacts` already is.
- `PageAuditResult` — the one canonical aggregate: `evidence_ref`,
  `findings: Vec<Finding>`, `technology_markers: Vec<ObservedTechnologyMarker>`.
  Not a shipping DTO.

## 3. Canonical Seam (as already implemented)

`audit_page(store: &DomainPersistence, url: &str) -> Result<PageAuditResult, AuditError>`
is the single canonical execution seam and the only production acquisition
entrypoint in the module. `analyze_page(&EvidencedPageFacts) -> Vec<Finding>`
and `extract_technology_markers(&EvidencedPageFacts) -> Vec<ObservedTechnologyMarker>`
are the two canonical analysis seams `audit_page` calls internally; both are
pure functions with no acquisition or persistence capability of their own
(verified by `extract_technology_markers_is_a_pure_function_with_no_acquisition_or_persistence_capability`).

## 4. Execution Graph (as already implemented)

```
audit_page(store, url)
  → fetch_single_page(url)                          [one acquisition]
  → EvidencedPageFacts::record(store, page)          [one evidence recording + fact derivation]
        → PageFacts::from_page(page)
              → extract_html_facts(html)             [one lol_html pass]
  → analyze_page(&evidenced)  → PAGE_RULES (11)      → Vec<Finding>
  → extract_technology_markers(&evidenced)           → Vec<ObservedTechnologyMarker>
  → record_finding(store, finding)*                  [durable, per Finding]
  → PageAuditResult { evidence_ref, findings, technology_markers }
```

One acquisition, one evidence recording, one HTML parse — regardless of how
many rules or marker sources run. No interface (CLI/MCP/API/Web Console)
touches this graph today; the Forbidden list in §5 below is the boundary a
future interface must respect when one is built.

## 5. Dependencies (as already implemented)

Allowed: `utils/evidence.rs` (`fetch_single_page`, `build_evidence`,
`record_evidence`, `EvidenceRef`, `EvidenceBundle`, `audit_response_headers`,
`AUDIT_RESPONSE_HEADER_ALLOWLIST`), `features/domain_persistence.rs`
(`disk` feature).

Forbidden (all guardrail-enforced): a second acquisition/transport
path; `Website` or any search-provider type; a technology-fingerprint/CVE
database; AI; process execution/network scanning; a second evidence or
HTML-parse path; an independent marker/`FindingId` identity type; any
CLI/API/MCP/Web Console reference to the module or its result vocabulary.

## 6. State (reconciliation of a genuine SDD ambiguity)

`audit_page()` is a **stateless canonical operation** (`SCORPION_SDD.md`
§5.1): `binding → execute → result`, no identity, no current state, no
transition. It durably records `Finding`s (`append_history`, exactly like
the evidence ledger/change detection/transform lineage already do) and does
not persist `ObservedTechnologyMarker`s at all — neither fact makes it
state-driven. `SCORPION_SDD.md` §5.2's distinguishing test is identity + a
mutable current state + explicit transitions; `audit_page()` has none of
these. No `AuditId`, `AuditSession`, `AuditState`, or independent
technology-marker persistence is introduced by this frontier, and none is
justified by anything either of the two implementation frontiers
established — reconnaissance here found no genuine ownership requirement
for any of them.

## 7. Security

No new security primitive. Reuses `AUDIT_RESPONSE_HEADER_ALLOWLIST`'s
existing closed, credential-free header capture (`utils/evidence.rs`,
`SCORPION_ARCHITECTURE.md` §3.6); `MARKER_HEADER_NAMES` is a fixed
three-entry subset of it. Fails closed:
header-absence security rules never fire when header observation itself was
unavailable; non-UTF-8 header bytes are skipped per-value rather than
fabricated.

## 8. Errors

`AuditError` (owned entirely by `audit.rs`): `Acquisition`,
`EvidenceRecording`, `EmptyEvidence`, `EvidenceUnresolvable`, `Evidence`,
`Persistence`, `Serialization`. No flattening into a generic error; each
variant preserves which layer failed.

## 9. Out of Scope (this frontier)

No audit rule added. No marker source added. No persistence added. No MCP,
CLI, Web Console, or API added. No acquisition-behavior change. No
production Rust implementation file (`spider/src/**`) was modified by this
frontier at all — every capability fact in §2–§8 above describes code that
was already CLOSED under the two prerequisite frontiers before this
reconciliation began (see §1). This frontier's own diff touches only
`SCORPION.md`, `SCORPION_ARCHITECTURE.md`, `SCORPION_SDD.md`,
`spider/tests/architecture_guardrails.rs` (one minimal guardrail extension
proving interfaces cannot reimplement the audit result vocabulary),
and this file.
