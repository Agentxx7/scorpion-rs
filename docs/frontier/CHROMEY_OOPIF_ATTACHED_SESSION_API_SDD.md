# Chromey OOPIF attached-session API

Frontier: `SCORPION_CHROMEY_OOPIF_ATTACHED_SESSION_API_001`

Baseline: `5a4d7912f82c1bb582228fa1397aeb610be734dc`

## Proven gap

Chromey 2.54.0 discovered genuine OOPIF targets, but nested flattened
`Target.attachedToTarget` events terminated inside the owning parent `Target`.
They never updated the browser handler's existing target/session registry.
Consequently `Browser::get_page(oopif_target_id)` returned `NotFound`, even
though Chromium had created and attached the child target.

## Upstream-compatible delta

The vendored 2.54.0 patch retains chromey's single websocket, connection
handler, target registry and command router. It makes only these architectural
changes:

1. A parent `Target` bubbles nested flattened attach/detach events to the
   existing browser handler.
2. The handler updates its existing `TargetId`/`SessionId` registry and forwards
   the same lifecycle events to browser listeners.
3. `Browser::attached_session(TargetId)` returns an immutable
   `AttachedTargetSession` bound to the exact target and session generation.
4. Commands use the existing `CommandMessage::with_session` and browser
   connection. No second websocket or alternate CDP transport exists.
5. Validation fails when the target is detached, destroyed, unknown, or has a
   replacement session. An old handle never follows a replacement attachment.

The patch contains no Scorpion policy, CAPTCHA vocabulary, selector routing,
geometry inference or browser-action behavior. Existing `Page` APIs remain
unchanged.

## Acceptance

A controlled `--site-per-process` Chromium fixture uses `127.0.0.1` and
`localhost` as separate sites. It proves child Runtime and DOM command routing,
backend-node discovery, parent-session isolation, detach invalidation, new
session non-resurrection, unknown-target rejection and browser-disconnect
invalidation.

## Successor boundary

The API supplies only lower-level authoritative target/session ownership. The
successor frame-context frontier remains responsible for:

`FrameId → TargetId → SessionId → execution context → frame DOM identity → frame owner`.

Coordinate transforms, snapshots, actions and CAPTCHA binding remain outside
this frontier.
