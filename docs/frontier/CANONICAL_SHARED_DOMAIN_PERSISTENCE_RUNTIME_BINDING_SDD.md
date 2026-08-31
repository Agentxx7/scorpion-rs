# Canonical shared domain persistence runtime binding SDD

Frontier: `SCORPION_CANONICAL_SHARED_DOMAIN_PERSISTENCE_RUNTIME_BINDING_001`

Baseline: `1fc02d00434f902120fe8c218e5f5c18cfebfe80`

## 1. Purpose

`SCORPION_MCP_CANONICAL_PAGE_AUDIT_SHIPPING_001` reconnaissance proved no
neutral, operator-owned, application-wide `DomainPersistence` runtime
binding existed — the only precedent, `RESEARCH_EVIDENCE_DB`, was genuinely
Research-scoped by name, module framing, and owning struct
(`resolve_research_config`/`ResearchService`), and `spider_mcp` had zero
references to `DomainPersistence` at all. That frontier BLOCKED on this one.
This SDD defines the resolved prerequisite: a canonical seam any interface
resolves its shared database path/handle through, plus the safety proof that
makes sharing one file across independently-opened handles legitimate.

## 2. Canonical Model

`spider/src/features/domain_runtime.rs`: `DOMAIN_DATABASE_ENV`
(`"SCORPION_DOMAIN_DB"`, new, preferred), `LEGACY_RESEARCH_DATABASE_ENV`
(`"RESEARCH_EVIDENCE_DB"`, explicit tested fallback), `DomainRuntimeError`.
No new domain model — this module resolves a path and opens a
`DomainPersistence` handle; it decides no domain semantics of its own
(mirrors `domain_persistence.rs`'s own mechanism-only posture).

## 3. Canonical Seam

`resolve_domain_database_path(explicit: Option<PathBuf>, lookup: &impl Fn(&str) -> Option<String>) -> Result<PathBuf, String>`
and `open_shared_domain_store(explicit, lookup) -> Result<DomainPersistence, DomainRuntimeError>`.
Priority order: `explicit` > `SCORPION_DOMAIN_DB` > `RESEARCH_EVIDENCE_DB`.

## 4. Execution Graph

```
interface (CLI flag / env)
  → resolve_domain_database_path / open_shared_domain_store
        → DomainPersistence::open(path)
```

`spider_cli::research::configured_database` and `scorpion_app`'s
`resolve_research_config_with`/`status` now delegate here — real, tested
reconciliation of `RESEARCH_EVIDENCE_DB`, not documentation asserting a
coincidence.

## 5. Dependencies

Allowed: `features/domain_persistence.rs` (`disk` feature) only.
Forbidden: network, transport, `features/identity.rs`,
`features/domain_state.rs`, any concrete domain/product-model type, a
second persistence mechanism.

## 6. State

Stateless (`binding → execute → result`) — resolves a path, opens a handle.
No identity, no current state, no transition of its own.

## 7. Security

No new primitive. The resolved path is never logged; `Err` messages name
only environment-variable names, never a resolved filesystem path or
credential.

## 8. Errors

`DomainRuntimeError::NotConfigured(String)` (names every variable checked)
/ `::Persistence(PersistenceError)` (wraps, never flattens, the underlying
open failure).

## 9. Shared-store safety proof (the actual deliverable)

Proven empirically, not assumed, before this binding was defined:

1. **Identity namespace safety** — every current canonical
   identity/derived-record-identity wire prefix (`evid_`, `research_`,
   `watch_`, `auth_`, `finding_`, `change_`, `lineage_`) is pairwise
   distinct; guardrail `every_canonical_identity_prefix_is_pairwise_distinct`
   scans `spider/src` for every `PREFIX` declaration and proves it, and is
   empirically confirmed to fail on an injected collision.
2. **Cross-handle visibility** — two independently constructed
   `DomainPersistence` values opened against the same real file observe
   each other's `write_current`/`append_history` writes immediately
   (`two_independently_opened_handles_against_the_same_file_see_each_others_writes`),
   including after one handle is dropped and reopened
   (`a_handle_opened_after_the_first_is_dropped_sees_everything_the_first_wrote`).
3. **Evidence cross-handle resolution** — evidence recorded through one
   handle resolves through a second, independently opened handle
   (`evidence_recorded_through_one_handle_resolves_through_a_second_handle_on_the_same_file`).
4. **Finding cross-handle resolution** — a `Finding` recorded through one
   handle reads back through a second
   (`finding_recorded_through_one_handle_is_readable_through_a_second_handle_on_the_same_file`).
5. **Concurrency safety** — near-concurrent writes from two handles against
   the same file, both to disjoint and to the same identity, neither
   deadlock, corrupt, nor silently lose a write
   (`concurrent_use_from_two_handles_against_the_same_file_does_not_deadlock_or_corrupt`).
   This test *initially failed* with a real `"database is locked"`
   (`SQLITE_BUSY`) error under an unhardened connection configuration — a
   genuine architectural gap, not a hypothetical one. Fixed within the
   existing single SQLite mechanism (no second persistence stack): WAL
   journal mode, an explicit 5-second busy timeout, and `BEGIN IMMEDIATE`
   for `DomainPersistence::write_current` (a deferred `BEGIN` let two
   concurrent transactions each acquire a read lock and then deadlock on
   upgrading to a write lock — `busy_timeout` alone cannot resolve that
   specific upgrade conflict; `BEGIN IMMEDIATE` acquires the write lock up
   front instead).

## 10. Out of Scope

No MCP code, no `spider_audit_page` tool, no audit semantics change, no
second persistence mechanism, no `open_in_memory()` as a production
solution, no new identity type. `SCORPION_MCP_CANONICAL_PAGE_AUDIT_SHIPPING_001`
remains a separate, still-not-started frontier this one unblocks but does
not resume.
