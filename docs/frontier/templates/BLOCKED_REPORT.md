# BLOCKED Report Template

```markdown
# <FRONTIER_ID> — BLOCKED Report

## State Reached
<AUDITING / SPECIFIED / IMPLEMENTING>

## Why Blocked
<The missing canonical model/seam/prerequisite, with evidence.>

## What Was NOT Done
- No workaround code was written.
- No local shim, fallback, or temporary alternate path was added.
- No unrelated files were modified.

## Repository State
<HEAD, worktree/index status — must be clean of implementation artifacts;
only audit/report artifacts may remain if any.>

## Smallest Prerequisite Frontier
<The one frontier that unblocks this one. Scoped as small as possible.>

## Resume Condition
<Exactly what must be CLOSED before this frontier can restart, and from
which baseline.>
```
