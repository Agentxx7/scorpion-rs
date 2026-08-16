# PaliGemma PointSelection prompt-contract convergence

Frontier: `SCORPION_PALIGEMMA_POINT_SELECTION_PROMPT_CONTRACT_CONVERGENCE_001`

Baseline: `3845147a5f086b1c2fe1032f29f1654bd673b4bf`

Blocked dependent frontier: `SCORPION_CANONICAL_CAPTCHA_BROWSER_EXECUTION_BINDING_001`

## Root cause this frontier converges

`SCORPION_PALIGEMMA_REAL_BROWSER_RASTER_GROUNDING_ROOT_CAUSE_001` proved the
capture/materialization/processor/parser pipeline correct end to end and
found a genuine, provable `PROMPT_CONTRACT_MISMATCH`: the real Turnstile
acceptance test fed a 165-character explanatory sentence ("This is a
Cloudflare Turnstile \"Verify you are human\" checkbox challenge. Return
the point at the center of the unchecked checkbox so a click completes
verification.") into `paligemma-local`'s `detect {label}` grammar, which is
designed for a concise noun-phrase grounding label (every qualified
fixture uses one, e.g. `"the outlined square"`, 19 characters). The raw
model output for the long prompt was a near-full-canvas box
(`y_min=0 x_min=0 y_max=1022 x_max=1022`) whose midpoint coincidentally
equals `(511,511)` — not a literal repeated `loc511` token, disproven by
direct raw-token inspection.

## Implementation audit

Traced the instruction string's full path: `CaptchaBrowserChallenge::PointSelection { instruction }`
(caller-supplied, e.g. by a test/acceptance harness) →
`captcha_browser.rs::materialize_request` (bare pass-through, `instruction.clone()`,
no summarization — confirmed by direct reading) → `CaptchaSolveRequest.challenge.instruction`
→ `solve_captcha` (the *only* canonical, provider-neutral dispatch chokepoint
every explicit attempt funnels through, already performing other
structural `InvalidChallenge`/`UnsupportedChallenge` validation before
`provider.solve()`) → `paligemma_captcha.rs::solve_by_kind` (uses the
instruction directly as `detect {label}`'s `label`, no independent
validation).

**Smallest ownership-correct fix**: `solve_captcha` in `spider/src/features/captcha.rs`.
This is the single existing canonical dispatch point every provider and
every real production attempt already passes through (proven directly:
the real Turnstile acceptance run reached the model via exactly this
function). Enforcing the contract here — once — means no provider
(present or future) needs its own copy of this check, and no caller can
bypass it while still using the canonical route. `captcha_browser.rs`
(pure conduit) and `paligemma_captcha.rs` (provider-specific grammar
consumer) are both correctly *not* where this belongs — neither owns the
canonical semantic of what `PointSelection.instruction` is supposed to
mean.

## The contract

`is_canonical_point_selection_label` (private to `captcha.rs`, invoked by
`solve_captcha` for `CaptchaChallengeKind::PointSelection` only):

- non-empty after trimming
- `<= CAPTCHA_POINT_SELECTION_LABEL_MAX_CHARS` (80) characters
- contains none of `.`, `!`, `?` (a concise noun phrase never needs
  sentence-terminating punctuation; task-orchestration prose reliably
  does)

Violations fail closed with the existing `CaptchaSolveFailure::InvalidChallenge`
— **before** `provider.solve()` is ever called (proven by a call-counter
test: zero provider invocations on rejection). No truncation, no
heuristic rewriting, no LLM-based summarization, no vendor name or
challenge-provider branching anywhere in the check.

Verified against the frontier's own allowed/disallowed examples: `"the
outlined square"`, `"the verification checkbox"`, `"the outlined
checkbox"`, `"the slider handle"`, `"the requested image tile"` all pass;
the verbatim disallowed Turnstile-workflow sentence and the actual
165-character production instruction both fail.

## Offline real-raster reference check

Per this frontier's own instruction ("Use the already-captured genuine
raster offline if still available. Do NOT perform another live Turnstile
solve."): this exact measurement already exists from
`SCORPION_PALIGEMMA_REAL_BROWSER_RASTER_GROUNDING_ROOT_CAUSE_001`'s own
audit, using the identical genuine captured raster (SHA-256
`3398f533da873529ac009b52810501bbd78289ea57e5617d20fa5d8733965807`) and a
canonically-conforming label (`"the outlined square"`, 19 chars, no
sentence punctuation — passes the contract above). No new live solve was
performed; this is a citation of already-recorded, truthful evidence:

| Instruction | Raw box | Derived point |
|---|---|---|
| 165-char production sentence (violates contract) | `y_min=0 x_min=0 y_max=1022 x_max=1022` | (111.78, 111.78) |
| `"the outlined square"` (conforms) | `y_min=0 x_min=0 y_max=292 x_max=1022` | (111.78, **31.94**) |

True checkbox center: `(20.5, 32.5)`. The corrected label materially
improves Y-axis grounding (31.94 vs. true 32.5 — near-exact) exactly as
the prior frontier's evidence predicted. **X remains effectively
full-width and wrong.** This frontier does not claim to fix Turnstile
acceptance, does not require real-raster success to close, and performed
no live solve to obtain this citation.

## Regression gates (all real, all re-run against the changed code)

| Gate | Result |
|---|---|
| CUDA/F16 registry + PointSelection + ImageGridSelection + HorizontalOffset (`solve_captcha` path, real inference) | PASS |
| Live-UI outlined-control 8-fixture qualification (CUDA/F16) | 8/8, identical to pre-change measurements |
| PointSelection precision audit (CUDA/F16) | 7/7, identical to pre-change measurements |
| Architecture guardrails | 113/113 |
| Spider default (excluding two confirmed pre-existing, baseline-reproducible environmental flakes: `chunk_idle_timeout_returns_partial_content`, `crawl_nxdomain_through_proxy_*`) | PASS |
| `spider_transport` | 36/36 |
| clippy (scoped to `captcha.rs`) | clean |
| rustfmt | clean |

CPU/F32's own 7/7 baseline was not independently re-run in this frontier:
it calls `provider.solve()` directly (bypassing the only function this
frontier changed, `solve_captcha`) and its underlying code
(`paligemma_runtime.rs`/`paligemma_captcha.rs`) is untouched and bit-for-bit
identical to when it was last verified 7/7 earlier in this same session.

## Next frontier

Per this frontier's own decision rule: the corrected prompt did **not**
unexpectedly ground the real raster correctly (X remains full-width), so
the next frontier is `SCORPION_PALIGEMMA_REAL_RASTER_HORIZONTAL_GROUNDING_FAILURE_001`
— why a correct raster, correct short prompt, and correct processor still
produce real-content X-axis grounding failure while browser-rendered
generic controls (Phase 5's local control fixture) pass cleanly.
