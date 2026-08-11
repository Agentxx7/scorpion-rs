# SDD Template

Copy this structure when specifying a frontier (state: SPECIFIED). Written
before implementation.

```markdown
# <FRONTIER_ID> — Software Design Specification

## 1. Purpose
<One paragraph: what this frontier establishes.>

## 2. Canonical Model
<The domain model(s) this frontier adds or changes. One canonical model per
capability.>

## 3. Canonical Seam
<The single public execution seam, with signature-level detail.>

## 4. Execution Graph
ENTRYPOINT → MODEL → BINDING/PLAN → EXECUTION SEAM → LOWER LAYER → RESULT
<The one active path. State explicitly which layers it touches.>

## 5. Dependencies
Allowed: <…>
Forbidden: <…>

## 6. State
Stateless (binding → execute → result) or state-driven (Id → Definition →
State → Snapshot → Transition → Event/Result → persisted state). Justify.

## 7. Security
<Which canonical security primitives are used. No duplicates. Fail-closed
behavior.>

## 8. Errors
<Error vocabulary, ownership, propagation. No flattening.>

## 9. Out of Scope
<What this SDD deliberately does not cover.>
```
