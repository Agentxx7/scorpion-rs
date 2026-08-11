# Acceptance Contract Template

Copy this structure before implementation. This contract is the shared TDD
target: tests are written against it first, and in a two-branch frontier both
branches receive this exact contract unmodified.

```markdown
# <FRONTIER_ID> — Acceptance Contract

## Baseline
<SHA>

## Acceptance Criteria
Each criterion must be mechanically checkable (a test, a scanner assertion,
or a documented command whose output is asserted).

1. <criterion> — proven by: <test/command>
2. …

## Negative Criteria
Things that must NOT exist after this frontier (each proven by a guardrail
or negative test):

1. <e.g. no second HTTP client construction in provider modules> — proven by: <guardrail>
2. …

## Feature-Gate Matrix
<Which feature combinations must compile/test: default, evidence,
transport_tor, no-default-features, wreq, cache, transport_tor+wreq. Mark
combinations that are expected-absent-by-cfg and why.>

## Regression Suite
<The exact commands that must pass.>

## Closure Requirements
- acceptance tests PASS
- regression PASS
- operator review accepted
- commit + push succeed
- HEAD == origin/main, worktree clean, index clean
- git diff --check PASS, git diff --cached --check PASS
```
