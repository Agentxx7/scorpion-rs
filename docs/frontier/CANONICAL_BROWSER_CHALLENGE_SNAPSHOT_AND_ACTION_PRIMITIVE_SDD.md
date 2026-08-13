# Canonical browser challenge snapshot and action primitive

Frontier: `SCORPION_CANONICAL_BROWSER_CHALLENGE_SNAPSHOT_AND_ACTION_PRIMITIVE_001`

Baseline: `fba9dbdbfb364ef6b5437ccec0eae3a6d3bf17bc`

The initial canonical binding supports only a single top-level Chromium frame.
The caller identifies a challenge element and supplies explicit stable IDs with
their exact `Element` objects. The seam stores backend-node, node and remote
object identity; it never re-queries selectors.

Capture records PNG dimensions, CSS viewport, scroll, DPR, clip, measured
capture scale, challenge geometry and content identity. Image coordinates map
to viewport input coordinates only through this recorded transform.

Before every action, the seam verifies the same frame, scroll, DPR, challenge
object/content, target objects and geometry. It then performs exactly one click,
point click or horizontal drag. Mutation, ambiguity, stale objects, bounds and
browser failures are typed. There is no clamping, retry, substitution, JS action
or alternate browser primitive.

## Recovery audit and ownership

The reusable canonical primitives are Chromium PNG capture, layout metrics,
`Element` backend/node identity, retained remote objects, content quads and the
existing smooth pointer dispatch. Raw screenshot, geometry and input methods
remain implementation primitives below this seam.

Existing `not_a_robot*` and multimodal automation loops are rejected as a
canonical implementation: they combine solver policy with page re-query,
page/element fallback clicks, JavaScript drag fallback, ignored action errors,
coordinate clamping or provider substitution. WebDriver helpers are upstream
compatibility primitives only because they do not expose an equivalent stable
node/frame/capture identity contract.

The browser seam owns capture, coordinate facts, retained target identity,
immediate revalidation and exactly one input action. Callers own challenge
identification and retry/admission policy. CAPTCHA core owns neutral challenge
and solution semantics; the registry owns explicit provider resolution; a
provider owns reasoning only.

## Coordinate and identity contract

Chromium element geometry is recorded in CSS viewport coordinates. The capture
clip is that geometry translated by the recorded page scroll into page
coordinates. The returned PNG supplies the authoritative pixel dimensions;
the measured, equal x/y pixel-to-CSS ratio is the capture scale. Image points
are divided by that scale and offset by recorded viewport geometry. DPR,
viewport, scroll, clip and scale are all retained and viewport/scroll/DPR are
revalidated. A non-finite, non-positive, unequal or out-of-bounds transform
fails closed.

Each caller ID maps to one retained Chromium `Element`, backend-node ID, node
ID, attempt nonce and geometry. The challenge likewise retains its exact
remote object, frame, content identity and geometry. A later snapshot may bind
a new attempt to the same live element, but doing so invalidates the earlier
attempt rather than silently refreshing it.

## Actions and context

An exact target click derives its clickable point from the retained backend
node after revalidation and verifies that the point remains inside the captured
target geometry. Point and drag actions use only the recorded transform. No
action clamps, retries, re-queries, substitutes an element, executes JavaScript
input or suppresses a browser error.

Top-level Chromium is the only supported initial context. Any additional frame
or non-top execution context is rejected as `UnsupportedContext`; shadow DOM
and cross-frame coordinates are not claimed.

CAPTCHA detection, reasoning, routing, retry and provider execution remain out
of scope. Nested frames and WebDriver are explicitly unsupported until their
coordinate and identity contracts are independently proven.
