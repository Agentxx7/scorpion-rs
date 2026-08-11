# Frontier Card Template

Copy this structure when proposing a frontier. Delete guidance in italics.

```text
## FRONTIER
<FRONTIER_ID>

## BASELINE
<SHA the frontier must start from; HEAD == origin/main required>

## CONTEXT
<Why this frontier exists. What is closed before it.>

## OBJECTIVE
<The single capability/architecture change. One frontier = one objective.>

## AUDIT FIRST
<What must be audited before implementation.>

## REQUIREMENTS
<Normative requirements. Include fail-closed and no-fallback rules.>

## MUST NOT IMPLEMENT
<Explicit scope exclusions.>

## FILE FRONTIER
<Exact files expected to change. Determined after audit.>

## EXPECTED OUTPUT
<Report sections to return.>

## STOP CONDITION
Stop before commit/push.
<What to return if the frontier cannot complete: BLOCKED + smallest
prerequisite frontier.>
```
