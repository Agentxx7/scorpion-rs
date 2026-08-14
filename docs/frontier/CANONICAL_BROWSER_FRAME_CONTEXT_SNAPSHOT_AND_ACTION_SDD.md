# Canonical frame-aware browser challenge snapshot/action

Frontier: `SCORPION_CANONICAL_BROWSER_FRAME_CONTEXT_SNAPSHOT_AND_ACTION_001`

Baseline: `d2f44e5a155a5ada594b4d81196fb60692384068`

## Canonical composition

`spider::features::browser_challenge::BrowserChallengeSnapshot` gains three
frame-aware entry points — `capture_in_frame`, `revalidate_in_frame`,
`apply_in_frame` — that compose the existing canonical browser-challenge
primitive (`SCORPION_CANONICAL_BROWSER_CHALLENGE_SNAPSHOT_AND_ACTION_PRIMITIVE_001`)
with the existing canonical frame-identity seam
(`SCORPION_CANONICAL_CHROMIUM_OOPIF_TARGET_SESSION_AND_FRAME_CONTEXT_001`).
This is the only frame-aware snapshot/action stack in the crate; the
top-level-only `capture`/`revalidate`/`apply` trio is unchanged and remains
the sole implementation for the no-iframe case.

```
captured image coords -> frame-local content coords -> frame viewport
  -> parent frame coords -> [recursively through further owners, if proven]
  -> top-level viewport -> browser input coords
```

## Ownership

`FrameContext` (unchanged by this frontier) owns identity facts: `FrameId`,
`TargetId`, `SessionId`, `ExecutionContextId`, frame-owner `BackendNodeId`,
lifecycle/revalidation.

The existing browser-challenge primitive (unchanged in its top-level form)
owns captured visual state, dimensions/transforms, target binding,
pre-action revalidation and the three exact actions.

This frontier owns exactly one thing: the composition making those
capabilities frame-aware — deriving the frame-owner's authoritative offset
and threading every geometry/action call through the caller-supplied
`FrameContext`'s own session instead of the top-level page's. It does not
own CAPTCHA reasoning, provider selection, retry/fallback, or challenge
detection.

## Frame-owner transform

`resolve_frame_owner_offset(top_level, frame)` derives the offset from the
frame-owner `<iframe>` element's content box
(`DOM.getBoxModel` through the *parent's* exact session — proven, never
inferred), and only supports one level: `frame.parent_frame_id` must name
`top_level.frame_id` exactly, or it fails closed
(`BrowserChallengeFailure::UnsupportedContext`) rather than composing an
unproven chain. A deeper nesting depth is therefore explicitly unsupported,
not silently assumed correct.

The offset's arithmetic is classification-dependent, proven empirically
against real Chromium rather than assumed:

- **Genuine OOPIF child** (distinct session/target, own compositor
  coordinate origin): the frame-owner offset is added to frame-local
  geometry — plain CSS-pixel addition, the same convention
  `chromiumoxide::element::Element::bounding_box` already relies on for the
  top-level primitive.
- **Same-session (in-process) child** (shares the parent's exact session):
  CDP's DOM domain already resolves that node's geometry in the top-level
  document's own coordinate space — there is only one compositor frame tree
  spanning both documents in that case. Applying the additive offset here
  as well was tried first and **empirically falsified**: it silently
  double-offsets every point, landing clicks near — but not on — the
  intended element, passing every internal self-consistency check (target
  geometry was captured with the same doubled formula) while still missing
  the real screen pixel. The offset is therefore identity (`x`/`y` zero) for
  this classification; only the frame-owner's own content box is still
  retained, to detect the iframe element itself moving or resizing.

`ExactPoint`/`ExactHorizontalDrag` reuse `self.transform.image_to_browser()`
completely unchanged from the top-level primitive, since the transform is
fully pre-composed into top-level viewport coordinates at capture time.
`ExactTargetClick` re-derives the frame-owner offset and the target's
frame-local clickable point fresh at click time (mirroring the top-level
primitive's own live re-query), then composes them.

## Frame-scoped DOM objects

chromiumoxide's own `Element` is hard-tied to `Arc<PageInner>` — it
structurally cannot represent a node reached through an arbitrary
`FrameContext`-owned session. `BoundObject` (`TopLevel(Element) |
Frame { backend_node_id, node_id, remote_object_id }`) is the parallel,
decoupled representation for the frame-scoped case; its `bounding_box`,
`clickable_point` and `call_js_fn` methods hand-mirror `Element`'s own CDP
command sequences (border quad for bounding box, content-quad centroid for
clickable point), routed through `FrameContext::execute` instead of a
page-bound session. `resolve_frame_object` mirrors `Element::new`'s
`DescribeNodeParams` + `ResolveNodeParams` recipe for the frame case.

## Frame snapshot binding

A frame-captured `BrowserChallengeSnapshot` retains a private
`FrameSnapshotContext`: both frames' full identity chain
(`top_level_frame_id/target_id/session_id`, `frame_id/target_id/session_id`,
`execution_context_id`), the frame-owner `BackendNodeId`, and the resolved
`FrameOwnerOffset`. `revalidate_in_frame` checks, in order: the caller's
supplied `top_level`/`frame` identities match the captured ones exactly
(`UnsupportedContext` otherwise — this is what makes a same-selector/
same-URL/same-geometry *replacement* frame unable to satisfy an old
snapshot, since its identity never matches); `top_level.revalidate(None)`
and `frame.revalidate(Some(top_level))` (both reused, not reimplemented, via
`map_frame_context_failure`); execution-context match; frame-owner
`BackendNodeId` match; viewport/scroll/DPR match; a freshly re-derived
frame-owner offset's content box against the captured one (owner moved/
resized); the existing primitive's own challenge/target content-identity
nonce and geometry checks, now routed through the frame's session. Any
single mismatch fails closed; no repair, no re-resolution, no fallback to a
different node.

## Actions

Exactly the three the top-level primitive already supports — exact target
click, exact point, exact horizontal drag — via the identical authoritative
transform, now frame-aware. No alternate iframe-specific action
implementation, no JavaScript click/drag fallback, no `page.click_smooth`
substitute path, no element re-query, no coordinate clamp, no DOM
substitution, no hidden frame switching by selector, no automatic retry, no
stale-context repair, no CAPTCHA/Turnstile-specific handling anywhere in
this seam.

## Failure model

Additions to `BrowserChallengeFailure`, reusing `FrameContextFailure`
variants through one translation function (`map_frame_context_failure`)
rather than reimplementing frame liveness detection:

```
FrameDetached, FrameNavigated, TargetReplaced, SessionChanged,
ExecutionContextChanged, FrameOwnerChanged, FrameGeometryUnavailable,
FrameTransformAmbiguous
```

Combined with the existing primitive's own `TargetStale`, `ChallengeMutated`,
`GeometryChanged`, `PointOutOfBounds`, `DragOutOfBounds`,
`BrowserActionFailed`, `RevalidationFailed`, `UnsupportedContext` — every one
reachable from the frame-aware path produces zero browser actions.

## Genuine controlled acceptance

`spider/tests/canonical_frame_aware_browser_challenge.rs` proves all 18
required facts against a real, controlled Chromium fixture (distinct
loopback origins/hostnames, `--site-per-process`, no mocks): the unmodified
top-level primitive still passes on its own dedicated iframe-free page (the
existing primitive's `capture()` requires `page.frames().len() == 1`, so it
is proven on a page that genuinely satisfies that, not silently skipped);
same-origin in-process child snapshot/action; genuine OOPIF snapshot/action;
child `FrameContext`/frame-owner geometry retained across the transform;
exact click/point/drag inside a genuine OOPIF each independently change
observable DOM state (fresh captures between actions, since each mutates the
challenge's own content-identity nonce — mirroring the existing top-level
acceptance test's established multi-action pattern); child navigation,
OOPIF target replacement, frame/session teardown, frame-owner replacement,
inner target replacement and challenge geometry mutation each independently
produce zero actions when attempted against the stale pre-mutation context;
out-of-bounds point/drag fail closed with the exact typed failure; and a
replacement OOPIF frame serving byte-identical markup (same selectors, same
geometry, only origin/target/session differ) cannot satisfy the old
snapshot.

### Environment notes (fixture, not seam behavior)

- `DOM.requestNode`'s returned `NodeId` is only registered once the DOM
  domain has been enabled on that exact session; resolving a frame-scoped
  backend node id from a `Runtime.evaluate` remote object therefore goes
  through `DOM.describeNode`'s own `objectId` parameter directly, skipping
  the `requestNode` round trip rather than depending on an implicit
  `DOM.enable`.
- Forcing session/target teardown via a raw `Target.closeTarget` mid-test
  was observed live to destabilize the shared CDP connection's own command
  channel for every later scenario in the same browser instance
  (`ChannelSendError`); the fixture instead removes the iframe element from
  the DOM, letting Chromium's normal frame-lifecycle path tear the
  session/target down, which chromiumoxide's handler processes cleanly.
- Each lifecycle-invalidation scenario inserts its own uniquely identified,
  freshly created `<iframe>` rather than mutating/replacing a shared one, so
  scenarios cannot interfere with each other's target/session identity.

## Successor boundary

Out of scope, deliberately: CAPTCHA solving, Turnstile-specific behavior,
CAPTCHA detection, retry orchestration, provider fallback, CLI/MCP exposure.
The successor frontier
(`SCORPION_CANONICAL_CAPTCHA_BROWSER_EXECUTION_BINDING_001`) binds canonical
CAPTCHA materialization and local Qwen inference to this frame-aware
snapshot/action seam for a genuine Turnstile acceptance.
