# Canonical Chromium OOPIF target/session and frame-context identity

Frontier: `SCORPION_CANONICAL_CHROMIUM_OOPIF_TARGET_SESSION_AND_FRAME_CONTEXT_001`

Baseline: `d2a6eb26407cdf45277ffa2009d7ca723b34ac35`

## Canonical chain

`spider::features::frame_context::FrameContext` proves, for both the
top-level Chromium frame and a genuine out-of-process (OOPIF) child frame:

```
FrameId -> TargetId -> SessionId -> ExecutionContextId
  -> frame-scoped DOM identity -> frame-owner identity
  -> lifecycle state -> authoritative revalidation
```

Every fact is read directly from a CDP response, or reused from chromey's
existing `AttachedTargetSession`/`AttachedSessionError`. Nothing is inferred
from a selector, DOM order, URL, origin or geometry.

## Ownership

chromey (unchanged by this frontier) owns raw `TargetId`<->`SessionId`
attachment, session-scoped CDP command routing and attached-session lifecycle
transport, via `Browser::attached_session` / `AttachedTargetSession`.

`frame_context.rs` owns canonical `FrameId`<->`TargetId`<->`SessionId`
association, canonical execution-context identity, frame-scoped DOM identity,
the frame-owner relationship and lifecycle/revalidation semantics. It does
not own CAPTCHA logic, provider selection, browser challenge actions,
coordinate transforms or retry/fallback policy.

The raw `AttachedTargetSession` handle and `Browser::attached_session` are
named/called only inside `frame_context.rs`
(`only_frame_context_calls_raw_attached_session_api`); CAPTCHA and
browser-challenge code cannot reach past the canonical seam
(`captcha_and_browser_challenge_cannot_reconstruct_frame_identity`); the seam
never resolves identity via selector
(`frame_context_never_resolves_identity_by_selector`).

## Frame <-> target association

Association is proven, never inferred. For a child frame:

1. The caller supplies an exact `TargetInfo` it directly observed from
   `Target.attachedToTarget` (or `.targetCreated`) — never re-derived via a
   fresh `Target.getTargetInfo` query, which was observed live to race
   Chromium still populating a just-attached target's metadata.
2. A coarse type check rejects a `"page"` target as a child, or an
   `"iframe"` target as top-level.
3. The candidate's own `Page.getFrameTree` (via its attached session) gives
   its own frame id and loader id.
4. The parent's own session executes `DOM.getFrameOwner(frame_id)`. Success
   is the authoritative association proof: the parent's document really does
   contain an iframe element owning exactly this frame id. This is the
   proof — not `TargetInfo.parentFrameId`, which was observed live to
   sometimes name a transient intermediate frame instead of the ultimate
   parent for a dynamically inserted iframe that briefly exists in-process
   before Chromium swaps it out to a genuine OOPIF target.

Steps 3-4 are retried together, re-fetching the frame id fresh each attempt,
for a bounded window (`FrameContext::wait_for_frame_and_owner`): immediately
after a target attaches its own default document can briefly still be its
pre-navigation placeholder, whose frame id the parent legitimately never
recognizes. Fail-closed applies once the window elapses.

`select_unique_child_target` is a caller-side convenience pre-filter over
already-observed `TargetInfo` candidates (by declared type and
`parentFrameId`) for disambiguating between siblings before calling
`resolve_child`; it is not itself the association proof and fails closed
(`FrameTargetAssociationAmbiguous`) when more than one candidate matches.

## Execution context selection

Within the owning session, `Runtime.disable` then `Runtime.enable` forces
Chromium to replay `Runtime.executionContextCreated` for every context
currently live (the same technique the chromey OOPIF prerequisite's own
acceptance test already established). The unique context whose `auxData`
reports `isDefault: true` for this exact frame id is selected; more than one
match is `ExecutionContextAmbiguous`, none is `ExecutionContextUnavailable`.
The first event is never selected blindly.

## DOM identity

`resolve_dom_identity` binds a caller-supplied `BackendNodeId` (the caller
resolves it themselves through this context's own `execute`, e.g.
`DOM.getDocument` + `DOM.querySelector`, entirely outside this module) as
canonical frame-scoped identity, proved live via `DOM.describeNode` through
the exact owning session. `revalidate_dom_identity` re-checks only that exact
backend-node id through that exact session — never by selector, never
accepting a different live node that happens to match the same selector.

## Frame owner

For a child frame, `frame_owner: Option<FrameOwnerIdentity>` retains the
owning `<iframe>` element's `BackendNodeId` in the parent's document, plus
the parent's `TargetId`/`SessionId` at proof time — enough for the successor
frame-transform frontier to derive parent-owner geometry without this
frontier implementing any coordinate transform itself.

## Lifecycle and revalidation

`revalidate(parent)` is pull-based and read-only:

1. `AttachedTargetSession::validate()` — chromey's own target/session
   liveness check, reused (not reimplemented) via
   `map_attached_session_error`, the single translation point onto this
   seam's typed vocabulary.
2. A fresh `Page.getFrameTree` compares frame id and loader id against the
   captured values; either mismatching is `FrameNavigated`.
3. For a child, a fresh `DOM.getFrameOwner` through the parent's live session
   is compared against the captured owner; a mismatch is `FrameOwnerChanged`.
4. `Runtime.evaluate` pinned to the captured `execution_context_id` proves it
   still resolves; failure is `ExecutionContextChanged`.

Every attached-session round trip in this module (`execute`, `validate`,
`event_listener`) is wrapped in a bounded wait
(`with_session_timeout`, `SESSION_COMMAND_TIMEOUT`): a session racing a
target/frame detach can have Chromium silently drop a command without ever
answering it — chromey's own `AttachedTargetSession` does not bound that
wait, so this seam does, converting an expired deadline into
`TargetSessionUnavailable` rather than hanging a caller forever.

There is no repair path. `Ok(())` means every asserted fact is still true;
any other outcome means the caller must discard the context and re-resolve
from scratch through `resolve_top_level`/`resolve_child` — this module never
rebinds a context to a replacement target, session or execution context.

## Failure model

```
FrameUnavailable, FrameTargetAssociationUnavailable,
FrameTargetAssociationAmbiguous, TargetSessionUnavailable,
ExecutionContextUnavailable, ExecutionContextAmbiguous,
ExecutionContextChanged, FrameDetached, FrameNavigated, TargetDetached,
TargetReplaced, SessionChanged, FrameOwnerUnavailable, FrameOwnerChanged,
DomIdentityUnavailable, UnsupportedContext
```

`TargetDetached`, `FrameDetached`, `SessionChanged`, `TargetReplaced` and
`TargetSessionUnavailable` are derived from chromey's own
`AttachedSessionError` variants (`TargetDestroyed`, `SessionDetached`,
`SessionReplaced`, `UnknownTarget`, `TargetNotAttached`/
`CommandRoutingFailed`) through the one translation function; they are not a
duplicate implementation of chromey's liveness detection.

## Genuine controlled OOPIF acceptance

`spider/tests/canonical_oopif_frame_context.rs` proves all 16 required facts
against a real, controlled `--site-per-process` Chromium fixture (three
distinct origins on loopback, no mocks): child `FrameId`/`TargetId`/
`SessionId` discovery and association; exact (non-first) execution-context
resolution; frame-scoped DOM identity through the child session; parent
frame-owner identity, cross-checked against an independent
`DOM.getFrameOwner` call; unchanged-context revalidation PASS; top-level
navigation invalidating both frame identity and execution context,
independently of `revalidate`'s internal check order; target detach,
target/session replacement and frame-owner replacement all invalidating the
old context from one genuine remove-then-recreate sequence; the same
selector, origin/URL and geometry in the replacement frame never satisfying
the old identity; and fail-closed ambiguous association using genuinely
observed `TargetInfo` facts. A second test proves top-level support resolves
and revalidates through the identical `FrameContext` abstraction.

Chromium's own `FrameManager`-equivalent (`page.frames()` /
`page.frame_parent()`) does not observe OOPIF frames once they go
out-of-process — confirmed live — so it is not an available independent
oracle for the OOPIF cases; the association proof this module relies on
(`DOM.getFrameOwner` executed through the exact owning session, in both
directions) is the authoritative one, and is what the acceptance test
cross-checks against instead.

### Environment notes (fixture, not seam behavior)

Three live Chromium/test-environment behaviors shaped the fixture and are
recorded here so they are not mistaken for canonical-seam bugs on a future
re-run in a different environment:

- A dynamically inserted iframe reusing an origin whose site instance was
  just freed was observed to sometimes stay in-process instead of
  re-attaching as a new OOPIF target (a Chromium process-reuse heuristic).
  The fixture's remove-then-recreate step therefore uses a freshly
  never-navigated origin for the replacement.
- Loopback addresses reached by a bare IP literal or an uncommon
  `/etc/hosts` alias intermittently produced genuine Chromium connection
  failures (`chrome-error://chromewebdata/`) in this sandboxed environment,
  while the identical address reached by `localhost`/`ip6-localhost` did
  not; `DOM.getFrameOwner` association still succeeds even when the
  document itself fails to load, since frame identity does not depend on
  content.
- Sibling OOPIF targets do not necessarily attach in creation order; the
  fixture matches each resolved child to its expected origin by committed
  port rather than event-arrival order.

## Successor boundary

Out of scope, deliberately: the full iframe coordinate transform, browser
click/point/drag application inside frames, CAPTCHA solving, provider
routing, retry/fallback policy, and CLI/MCP exposure. The successor frontier
(`SCORPION_CANONICAL_BROWSER_FRAME_CONTEXT_SNAPSHOT_AND_ACTION_001`) composes
canonical frame context with frame-owner geometry into an authoritative
child-frame coordinate transform and a frame-aware snapshot/action seam.
