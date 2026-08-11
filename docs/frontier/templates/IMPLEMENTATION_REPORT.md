# Implementation Report Template

```markdown
# <FRONTIER_ID> — Implementation Report

## A. PRE_IMPLEMENTATION_STATE
<HEAD, origin/main, worktree/index status, diff checks.>

## B. AUDIT
<What the audit found, with path:line evidence.>

## C. DESIGN (reference to SDD)
<Which SDD governs this implementation.>

## D. IMPLEMENTATION
<What changed, per file, and why.>

## E. TESTS
<Acceptance tests, unit tests, negative tests — commands and results.>

## F. FEATURE_GATE_RESULTS
<Matrix: PASS / EXPECTED ABSENT BY CFG / PRE-EXISTING BASELINE FAILURE /
NEW FAILURE.>

## G. REGRESSION_RESULTS
<Commands and results.>

## H. QUALITY_GATE
<rustfmt, git diff --check, git diff --cached --check.>

## I. DIFF_SUMMARY
<git diff --stat output.>

## J. EXPLICITLY_NOT_IMPLEMENTED
<Scope exclusions honored.>

## K. ARCHITECTURE_DEBT
<Found, removed-now vs requires-follow-up.>

## L. BLOCKERS
<None, or the exact blocker.>

Stop before commit/push.
```
