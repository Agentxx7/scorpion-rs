# Authenticated session lifecycle

Frontier: `SCORPION_AUTHENTICATED_SESSION_LIFECYCLE_001`

Baseline: `897f40cc285802c63920e3a6b8c4995d0442232a`

## Purpose

`SCORPION.md` §5 ("Authenticated Research") locked a non-negotiable rule
years before this roadmap existed: *"Future MFA/interactive
authentication must support pausing and resuming the same authenticated
browser session — not re-authenticating from scratch, and not silently
continuing unauthenticated."* Tracks 1–4 gave Scorpion identity,
state/transition semantics, persistence, and a durable evidence ledger —
but none of them had a *stateful* domain object with a real "current
state that changes over time" to persist. Track 5 is the first
capability whose entire point is exactly that, and it is the concrete
realization of §5's pause/resume rule: not a comment promising
continuity, but a transition that structurally cannot succeed without
proof of it.

## A. AuthSessionId model

Added to `spider/src/features/identity.rs` (Track 1's one canonical
identity module), in the exact same shape as `EvidenceId`/`WatchId`: 16
opaque random bytes, wire form `auth_<32 lowercase hex>`, full
`Display`/`FromStr`/`TryFrom`/`Eq`/`Hash`/`Ord` surface, `new()` minting
via the same dependency-free entropy source. Structurally identical to
the other two identities means structurally identical guarantee: there is
no field, variant, or constructor through which a cookie, `Authorization`
value, token, or other credential could ever be supplied.

## B. Lifecycle vocabulary

Three states, source-justified directly from `SCORPION.md` §5's own
words — not invented for symmetry:

- `Active { origin, profile }` — authenticated and usable.
- `Paused { origin, profile, continuity }` — suspended, not revoked;
  carries the [`BrowserContinuityToken`] a resume must match exactly.
- `Invalidated { origin, profile }` — permanently revoked. **Terminal**:
  no transition in this module leads out of it.

`"resumed"` is deliberately **not** a fourth state. After a resume
succeeds the session's current state is `Active`, indistinguishable from
one that was never paused except by its historical record — a bare
`Resumed` state distinct from `Active` would be inventing a distinction
for symmetry, which both this frontier's scope and Track 2's own "no
lifecycle semantics for symmetry" precedent forbid.
[`ResumeSession`] is the transition that produces `Active` again.

Every variant carries `origin` (§5: "authentication is origin/policy
scoped") and `profile: AuthenticationProfile` — `SCORPION.md` §5's
locked, previously-undefined method vocabulary (`None`, `FormLogin`,
`BasicAuth`, `BearerToken`, `CookieSession`, `OAuth`,
`InteractiveBrowser`), realized here for the first time as a
classification enum, not a secret.

## C. Transition semantics

Built directly on Track 2's `Transition<S>` trait, unmodified:

- `PauseSession { continuity }` — `Active → Paused`. Rejected from
  anywhere else.
- `ResumeSession { continuity }` — `Paused → Active`, **only** if
  `continuity` equals exactly the token recorded by `PauseSession`.
  Otherwise `AuthSessionTransitionRejected::ContinuityMismatch` — the
  session's state is returned completely unchanged (still `Paused`, still
  its *original* token, proven directly in
  `resume_with_mismatched_continuity_fails_closed_and_leaves_state_unchanged`).
- `InvalidateSession` — `Active | Paused → Invalidated`. Rejected
  (`Terminal`) if already invalidated — not idempotent-by-silent-success.

Every non-permitted combination is exercised directly: resuming a
never-paused session, pausing/resuming/re-invalidating an already-
invalidated one. All three transition types are `Transition<S>` impls
with no special-cased escape hatch — `CurrentState::apply`'s existing
"reject leaves the original unchanged" guarantee (Track 2, unmodified)
is what makes every rejection here fail closed.

## D. Persistence semantics

The first capability to use **both** of `DomainPersistence`'s primitives
together (Track 4's evidence ledger only ever needed the append-only
half, since evidence has no current state):

- `create_session()` writes the fresh session's `Active` state via
  `write_current(id, expected_revision: None, ...)` — a genuine first
  write.
- `apply_session_transition()` reads the current `(revision, state)`,
  runs it through `CurrentState::apply` (Track 2, unmodified), then
  writes the new current state via `write_current(id,
  Some(revision), ...)` (compare-and-swap — a concurrent writer racing
  this call is rejected as `AuthSessionError::ConcurrentModification`,
  never silently lost) and appends the just-superseded state via
  `append_history(id, revision, ...)` (Track 3's immutable, append-only
  historical record, unmodified).

`full_pause_resume_invalidate_lifecycle_persists_and_reads_back_truthfully`
drives a session through all three transitions and asserts the resulting
history log reads back exactly `[active, paused, active]` — the precise
sequence of superseded states, in order, byte-for-byte round-tripped
through `serde_json`.

## E. Browser-session continuity behavior

`PauseSession`'s `BrowserContinuityToken` is an opaque, caller-supplied
reference — this module does not construct or prescribe its derivation
(doing so would mean building new browser-suspension architecture, out
of this frontier's explicit scope). What this module *does* guarantee is
the truthfulness contract §5 requires: `ResumeSession` cannot reach
`Active` without presenting the exact token `PauseSession` recorded.
There is no other transition path from `Paused` to `Active`, so it is
structurally impossible for this module to "silently re-authenticate and
claim continuity" — a resume with a different, absent, or fresh-context
token is rejected outright, never accepted as continuous. A real
integration wiring an actual live cookie jar or CDP session identity into
`BrowserContinuityToken` is left to a future frontier that touches
browser code — this one deliberately does not.

## F. Secret/identity separation proof

- `AuthSessionId`: 16 opaque bytes, same shape as `EvidenceId`/`WatchId`
  — structurally incapable of holding secret material. Guardrailed
  directly (`auth_session_credential_types_never_appear_in_identity_or_lifecycle`
  checks the exact struct definition).
- `AuthSessionState`: only `origin` (a plain string), `AuthenticationProfile`
  (a classification enum), and `BrowserContinuityToken` (an opaque
  reference string, never a cookie/token value) — no field ever holds a
  raw credential.
- No real credential-carrying type from this codebase (`HeaderValue`,
  `HeaderMap`, `SecretRequestHeaders`, a cookie jar import) is referenced
  anywhere in `identity.rs` or `auth_session.rs` — checked as actual
  code forms (imports/field-type annotations), not prose, since this
  module's own doc comments legitimately *discuss* cookies/tokens in
  English to explain why none can enter.
- `SCORPION.md` §5's `CredentialRef` remains locked/undefined, exactly as
  before this frontier — not implemented here.

## G. Collision audit

"Session" already named three unrelated things; none redefined:

1. `chromiumoxide::cdp::browser_protocol::target::SessionId` — CDP
   browser-automation transport identity (vendored/upstream).
2. `features/frame_context.rs`'s `session_id`/`owner_session_id` — the
   same CDP type, used in the canonical OOPIF frame-identity chain.
   Transport-layer plumbing, no relation to authentication.
3. `spider_mcp`'s `CrawlSession`/`CrawlSessionStatus` — in-memory,
   `DashMap<String, CrawlSession>`-keyed async MCP tool-call progress
   tracking (`Running`/`Complete`/`Failed`), TTL/LRU-evicted. Server
   bookkeeping for one tool invocation; not durable, not authentication.

None represent "this identity is currently authenticated against some
origin, and that fact should survive across requests" — the concept
`AuthSessionId` names. `auth_session_id_never_collides_with_existing_session_meanings`
proves both `frame_context.rs` and `spider_mcp/src/state.rs` remain
untouched (no `AuthSessionId`/`AuthSessionState` reference in either) and
that the reconciliation is documented, not merely coincidentally true.

## Not implemented here

Per this frontier's explicit scope: no `WatchDefinition`/`WatchState`, no
scheduling, no `ChangeResult`/`ChangeEvent`, no Fingerprint/Lineage, no
new browser architecture, no second cookie/session subsystem, no CAPTCHA
or transport modification, no generic/bare `SessionId`, and no concrete
`CredentialRef`/authentication-flow implementation for any
`AuthenticationProfile` variant (form submission, OAuth redirect
handling, etc. remain unbuilt — only the classification enum exists).

## Acceptance summary

- `spider/src/features/identity.rs` — new `AuthSessionId` (mirroring
  `EvidenceId`/`WatchId` exactly); module doc updated to name three
  identity types and the three-way session-collision reconciliation; 2
  new + 1 extended unit test.
- `spider/src/features/auth_session.rs` — new module: `AuthenticationProfile`,
  `BrowserContinuityToken`, `AuthSessionState`, `AuthSessionTransitionRejected`,
  `PauseSession`/`ResumeSession`/`InvalidateSession`, and — behind
  `disk`+`serde` — `AuthSessionError`/`create_session`/
  `read_current_session`/`apply_session_transition`; 6 pure
  transition-contract unit tests (unconditional) + 7 persistence-ledger
  tests (behind `disk`+`serde`).
- `spider/src/features/mod.rs` — unconditional `pub mod auth_session;`.
- `SCORPION_ARCHITECTURE.md` — new §3.12, §7.6, and §11 updated.
- `spider/tests/architecture_guardrails.rs` — 10 new guardrails: exactly-
  one definition site for `AuthSessionId` and every `auth_session.rs`
  type, no bare `SessionId` anywhere, the collision-reconciliation proof,
  exactly 3 `Transition<AuthSessionState>` impls, use of both
  `DomainPersistence` primitives (never a second persistence mechanism),
  the exact-match continuity check, no real credential type referenced,
  no out-of-scope capability implemented, and no shadow model in
  `spider_cli`/`spider_mcp`.
- 155/155 architecture guardrails pass; 13 new `auth_session` unit tests
  pass (6 unconditional + 7 with `disk`+`serde`); 3 new `identity` unit
  tests pass; 752/752 lib tests pass with `basic disk evidence`; `cargo
  fmt --check` and `cargo clippy --lib --tests -D warnings` clean on
  default and `basic disk evidence`; `git diff --check` clean; full
  workspace `cargo check` clean. Two pre-existing live-network lib tests
  (`website::crawl`, `website::scrape`) failed again in one run of this
  frontier's verification sweep — the same environment-specific flake
  confirmed against baseline `main` in an earlier frontier of this
  session, unrelated to this change.

## Successor boundary

This frontier realizes identity + lifecycle semantics + persistence for
authenticated sessions only. Explicitly out of scope, left for later,
separate frontiers: a concrete `CredentialRef`/authentication-flow
implementation for any `AuthenticationProfile` variant, real
`BrowserContinuityToken` derivation from a live browser/cookie-jar
primitive, `WatchDefinition`/`WatchState`, scheduling,
`ChangeResult`/`ChangeEvent`, Fingerprint/Lineage (Track 6), and any
interface (CLI/MCP) surface for creating or transitioning authenticated
sessions.
