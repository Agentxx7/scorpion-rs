# Architecture Comparison Template

Used only in two-branch frontiers, before selecting the canonical
implementation. Both branches ran the same acceptance suite against this
comparison.

```markdown
# <FRONTIER_ID> — Architecture Comparison

## Baseline SHA (shared)
<SHA>

## Branch A
- Approach: <summary>
- Acceptance suite: <PASS/FAIL per criterion>
- Architecture fit: <ownership, dependency direction, state, security,
  errors>
- Cost/debt introduced: <…>

## Branch B
- Approach: <summary>
- Acceptance suite: <PASS/FAIL per criterion>
- Architecture fit: <…>
- Cost/debt introduced: <…>

## Decision
Winner: <A or B>
Rationale: <explicit, architecture-level reasons — not taste>

## Loser Disposition
REJECTED. <How the loser was removed and which guardrails now lock the
winner's shape. Loser code must not remain callable on main.>
```
