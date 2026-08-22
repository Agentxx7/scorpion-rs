# Scorpion Intelligent Failure — AI, TDD, and False Product Confidence

## The failure

Scorpion's major intelligent failure was not simply that “AI hallucinated.”
The deeper problem was a development loop in which AI could simultaneously be
the requirement interpreter, architect, implementation author, test author,
test interpreter, and closure-report author:

```text
AI interprets requirement
        ↓
AI writes test
        ↓
AI writes implementation
        ↓
implementation satisfies AI's own test
        ↓
test becomes green
        ↓
AI reports capability as working
```

That loop can produce substantial, correct engineering. It can also establish
only that an implementation satisfies the AI's model of the requirement and
the test it derived from that model. Without independent evidence, it does not
establish that:

- the shipping interface reaches the implementation;
- the intended canonical path is the path actually used;
- real network, provider, browser, or model infrastructure works;
- durable state survives process and store restart;
- a test-only or parallel implementation was not used; or
- an operator can perform the intended product task.

The failure was epistemic and architectural: evidence generated inside one
self-referential loop was promoted into a stronger product claim than that
evidence could support. This does not mean every historical closure was false
or every test meaningless. Different capabilities had different evidence, and
their proof status must be evaluated independently.

## The TDD lesson

Traditional test-driven development remains useful. It helps make behavior
specific, keeps changes bounded, and catches regressions. But TDD is
insufficient as the sole control architecture when the same AI writes both the
tests and implementation and then decides what the passing tests mean.

Scorpion's canonical rule is:

```text
TEST PASS = CODE_PROVEN
```

It means nothing stronger without the corresponding additional evidence. A
passing deterministic test does not by itself prove CI execution, a shipping
interface, a live environment, or an operator-observed outcome.

## The guardrail lesson

Architecture guardrails remain valuable evidence instruments. They can prove,
for example, that:

- the CLI does not mint `ResearchId`;
- the CLI does not call `Agent::research` directly; and
- no parallel persistence path exists in the guarded architecture.

Those are important code facts. They do not by themselves prove that:

```text
scorpion research "<topic>"
```

works against real search, acquisition, model, evidence, and persistence
infrastructure. Guardrails constrain architecture; they are not product
reality.

## Operator reality: durable research

A concrete durable-research observation was made on shipping research commit:

```text
b8671f335642e81bfc279e521c8d0c80729a3d12
```

The real shipping command `scorpion research ...` completed with:

```text
ResearchId:
research_8a163f3ccffa2e19949cbad189452ead

EvidenceIds:
evid_42db65ff48258b4607efcff1ee44a29e
evid_9cb47b3e6b903e41875d843c89ecff4c
evid_0f3bc93a7aa1875104c8ca5ee5c74892
```

After that process ended, a separate process ran
`scorpion research show research_8a163f3ccffa2e19949cbad189452ead`.
It reopened the same durable result and Source-N bindings without requiring
the original search or model runtime.

That observation established `OPERATOR_OBSERVED` for the commit on which it
occurred. Fixture tests are a different evidence class and cannot substitute
for it. The observation must not be silently moved to a newer commit.

## CI enforcement is not CI proof

Scorpion distinguishes two separate facts:

- `CI_ENFORCED`: the workflow truthfully contains the exact required command.
- `CI_PROVEN`: GitHub actually executed that command successfully, bound to an
  exact commit, workflow, run ID, job, step, command, and conclusion.

Candidate commit:

```text
721f11f2260e82e922670eaff976478b7fa16c1d
```

had green local tests and a workflow configured for pushes to `main`. GitHub
created no repository-owned workflow run for the candidate. No job reached a
runner. The truthful status was therefore:

```text
CI-OBSERVED / FAIL / NOT CLOSED
```

The candidate does not have `CI_PROVEN`, and the current Actions control-plane
problem is not fixed merely because the workflow is correctly configured.
This is precisely why workflow configuration cannot stand in for observed CI.

## Evidence-first, proof-gated development

The resulting engineering process is:

```text
audit reality
→ identify one real gap
→ reuse the canonical path
→ minimal implementation
→ deterministic proof
→ external CI proof
→ operator/live observation where required
→ closure
```

Its proof classes are deliberately independent:

- `CODE_PROVEN`: source/static evidence and deterministic tests support the
  claim.
- `CI_PROVEN`: a concrete external CI execution supports the claim.
- `OPERATOR_OBSERVED`: a real product command or path was concretely observed.
- `LIVE_ENVIRONMENT_DEPENDENT`: the capability requires external
  infrastructure; the classification itself is not an observation.
- `UNPROVEN`: required proof does not yet exist.

`CLOSED` requires the declared maturity state and every proof class that the
capability requires. Operator observation is not universal: structural or
library capabilities may have no meaningful operator acceptance, while
shipping or live capabilities often do.

`UNPROVEN` is an acceptable and useful state. Explicit incomplete proof is
safer than false certainty.

## Why this is an intelligent failure

The same development process produced useful engineering, including canonical
acquisition, provenance, durable evidence, canonical identities, durable
research sessions and results, the shipping research CLI, reopening by
`ResearchId`, and architecture guardrails.

The intelligent failure was discovering that convincing engineering evidence
and observed product reality are different things. The output was often
technically sophisticated; the mistake was promoting one kind of evidence
into another. Scorpion changed its development architecture in response:
claims are now limited by independently classified proof.

## AI's role after the correction

AI may still:

- audit code and trace call chains;
- explain architecture and compare designs;
- propose and implement bounded changes;
- write deterministic and adversarial tests; and
- document observed behavior and remaining uncertainty.

AI may not independently promote its own work to claims such as `PRODUCT
WORKS`, `CI PASSED`, `LIVE PATH WORKS`, `OPERATOR VERIFIED`, or `CLOSED`.
Those claims require their corresponding independent evidence.

This model does not assert that TDD is broken, that AI cannot build production
software, or that all of Scorpion is operator-verified. It requires only that
each capability say exactly what has—and has not—been proved.
