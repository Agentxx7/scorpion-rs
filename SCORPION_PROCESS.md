# Scorpion — Frontier Process Contract

**Status:** ACTIVE — binding for all Scorpion frontier work.

This document defines the canonical frontier lifecycle, the SDD/TDD process,
and the two-branch process for architecture-critical changes. Templates for
every required artifact live in `docs/frontier/templates/`.

---

## 1. Canonical Order

```
AUDIT
→ SDD (specification)
→ SHARED ACCEPTANCE TESTS (TDD contract)
→ IMPLEMENTATION
→ VERIFICATION (tests + regression)
→ OPERATOR REVIEW
→ CLOSURE
```

Never:

```
PROMPT → IMPLEMENTATION → TESTS AFTERWARDS
```

---

## 2. Frontier State Machine

```
PROPOSED
→ AUDITING
→ SPECIFIED
→ IMPLEMENTING
→ VERIFYING
→ READY_FOR_REVIEW
→ READY_FOR_CLOSURE
→ CLOSED
```

Failure path:

```
AUDITING / SPECIFIED / IMPLEMENTING
→ BLOCKED
→ prerequisite frontier
```

Rules:

- One frontier at a time.
- A BLOCKED frontier must not accumulate workaround code. The only artifacts
  a BLOCKED frontier leaves behind are its audit and its BLOCKED report,
  identifying the smallest prerequisite frontier.
- A capability that lacks the canonical model/seam required for truthful
  implementation is BLOCKED, not worked around.
- CLOSED requires, all of:
  - acceptance tests PASS
  - regression PASS
  - operator review accepted
  - commit succeeds
  - push succeeds
  - `HEAD == origin/main`
  - worktree clean
  - index clean
  - `git diff --check` PASS
  - `git diff --cached --check` PASS

---

## 3. Required Artifacts

| Artifact | Template | When |
|---|---|---|
| Frontier card | `docs/frontier/templates/FRONTIER_CARD.md` | PROPOSED |
| Audit report | (free-form section of the frontier report) | AUDITING |
| SDD | `docs/frontier/templates/SDD.md` | SPECIFIED |
| Acceptance contract | `docs/frontier/templates/ACCEPTANCE_CONTRACT.md` | SPECIFIED, before implementation |
| Implementation report | `docs/frontier/templates/IMPLEMENTATION_REPORT.md` | VERIFYING |
| Architecture comparison | `docs/frontier/templates/ARCHITECTURE_COMPARISON.md` | two-branch frontiers, before selection |
| BLOCKED report | `docs/frontier/templates/BLOCKED_REPORT.md` | on BLOCKED transition |
| Prerequisite frontier report | `docs/frontier/templates/PREREQUISITE_FRONTIER.md` | on BLOCKED transition |
| Closure report | `docs/frontier/templates/CLOSURE_REPORT.md` | CLOSED |

The acceptance contract is written **before** implementation and is shared
verbatim by both branches in a two-branch frontier.

---

## 4. Two-Branch Process (architecture-critical frontiers)

When an architecture-critical frontier contains a genuine competing design
decision, the decision is resolved by two independent implementation branches:

```
CLEAN MAIN BASELINE
        ↓
       SDD
        ↓
SHARED ACCEPTANCE/TDD CONTRACT
        ↓
   ┌─────────────┐
   │             │
BRANCH A      BRANCH B
   │             │
   └──────┬──────┘
          ↓
 SAME ACCEPTANCE SUITE
          ↓
 ARCHITECTURE COMPARISON
          ↓
 SELECT EXACTLY ONE
          ↓
 LOSER = REJECTED
          ↓
 REMOVE LOSER
          ↓
 GUARDRAILS LOCK WINNER
          ↓
 REGRESSION
          ↓
 OPERATOR REVIEW
          ↓
 COMMIT / PUSH / CLEAN CLOSURE
```

Requirements:

- Both branches start from the exact same baseline SHA.
- Both receive the same SDD and the same canonical acceptance tests.
- Branch-specific unit tests are allowed; acceptance criteria are not
  branch-specific.
- The architecture comparison is explicit and recorded
  (`ARCHITECTURE_COMPARISON.md`).
- Do not merge fragments from both branches without a new architecture review.
- The losing implementation must not remain callable on main. It is removed;
  guardrails are added so the winning shape is what the tree enforces.

Do not fake two branches where no competing design decision exists. If the
audit surfaces exactly one truthful design, a single-branch frontier with the
SDD and acceptance contract is correct.

---

## 5. Precondition Gate

Every frontier begins by proving, before any audit or implementation:

- `HEAD == origin/main`
- clean worktree
- clean index
- `git diff --check` PASS
- `git diff --cached --check` PASS

If any precondition fails: STOP. Do not stash, commit, amend, clean, or mix
another frontier. Return PRECONDITION BLOCK.
