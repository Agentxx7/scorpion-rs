//! Canonical provider-neutral browser challenge snapshot and action seam.
//!
//! The original primitive supported only a top-level Chromium frame. It now
//! also composes with [`crate::features::frame_context::FrameContext`] to
//! operate truthfully inside a same-origin child frame or a genuine
//! out-of-process (OOPIF) child, through [`BrowserChallengeSnapshot::capture_in_frame`]
//! and its matching revalidate/apply entry points — the same struct, same
//! typed failures, same exact-action contract, composed with authoritative
//! frame geometry rather than a second implementation. A snapshot retains
//! the exact remote DOM objects supplied by its caller and never re-queries
//! a selector when applying an action.

#![cfg(feature = "chrome")]

use crate::features::frame_context::{FrameClassification, FrameContext, FrameContextFailure};
use chromiumoxide::cdp::browser_protocol::dom::{
    BackendNodeId, DescribeNodeParams, GetBoxModelParams, GetContentQuadsParams, NodeId, Quad,
    ResolveNodeParams,
};
use chromiumoxide::cdp::browser_protocol::page::{CaptureScreenshotFormat, FrameId, Viewport};
use chromiumoxide::cdp::browser_protocol::target::{SessionId, TargetId};
use chromiumoxide::cdp::js_protocol::runtime::{
    CallFunctionOnParams, CallFunctionOnReturns, ExecutionContextId, RemoteObjectId,
};
use chromiumoxide::element::Element;
use chromiumoxide::layout::{BoundingBox, ElementQuad, Point};
use chromiumoxide::Page;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

static ATTEMPT_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const GEOMETRY_EPSILON: f64 = 0.25;

/// Typed browser snapshot/action failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserChallengeFailure {
    /// The active browser/frame context cannot be represented truthfully.
    UnsupportedContext,
    /// The browser could not produce the requested immutable snapshot.
    SnapshotCaptureFailed,
    /// Captured image or browser geometry is empty or invalid.
    InvalidSnapshotDimensions,
    /// An exact browser identity cannot be bound to a stable caller ID.
    TargetIdentityUnavailable,
    /// A previously bound browser target no longer denotes the same live object.
    TargetStale,
    /// Challenge content changed after capture.
    ChallengeMutated,
    /// Browser geometry changed enough to invalidate the recorded transform.
    GeometryChanged,
    /// Browser and captured-image coordinates cannot be mapped authoritatively.
    TransformAmbiguous,
    /// A requested image-space point lies outside the captured image.
    PointOutOfBounds,
    /// A requested drag endpoint lies outside the captured image.
    DragOutOfBounds,
    /// The one authoritative browser input action failed.
    BrowserActionFailed,
    /// Browser state could not be revalidated immediately before action.
    RevalidationFailed,
    /// The captured frame detached.
    FrameDetached,
    /// The captured frame navigated (new document).
    FrameNavigated,
    /// The frame's owning target was destroyed or replaced.
    TargetReplaced,
    /// The frame's owning session was replaced by a new attachment generation.
    SessionChanged,
    /// The frame's execution context no longer resolves.
    ExecutionContextChanged,
    /// The parent's frame-owner element changed identity.
    FrameOwnerChanged,
    /// Chromium did not expose authoritative frame-owner geometry.
    FrameGeometryUnavailable,
    /// Frame-local and top-level coordinates cannot be related uniquely.
    FrameTransformAmbiguous,
}

/// Immutable rectangle in CSS/browser coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrowserRect {
    /// Left edge in the rectangle's declared coordinate space.
    pub x: f64,
    /// Top edge in the rectangle's declared coordinate space.
    pub y: f64,
    /// Rectangle width.
    pub width: f64,
    /// Rectangle height.
    pub height: f64,
}

impl BrowserRect {
    fn valid(self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|value| value.is_finite())
            && self.width > 0.0
            && self.height > 0.0
    }

    fn materially_differs(self, other: Self) -> bool {
        (self.x - other.x).abs() > GEOMETRY_EPSILON
            || (self.y - other.y).abs() > GEOMETRY_EPSILON
            || (self.width - other.width).abs() > GEOMETRY_EPSILON
            || (self.height - other.height).abs() > GEOMETRY_EPSILON
    }
}

impl From<BoundingBox> for BrowserRect {
    fn from(value: BoundingBox) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
        }
    }
}

/// Explicit image-pixel to browser-viewport transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrowserImageTransform {
    /// Captured image width in pixels.
    pub image_width: u32,
    /// Captured image height in pixels.
    pub image_height: u32,
    /// Captured challenge geometry in CSS viewport coordinates.
    pub browser_geometry: BrowserRect,
    /// Screenshot clip in page coordinates.
    pub capture_clip: BrowserRect,
    /// Captured pixels per CSS pixel.
    pub capture_scale: f64,
    /// Browser device pixel ratio recorded at capture.
    pub device_pixel_ratio: f64,
    /// Horizontal page scroll recorded at capture.
    pub scroll_x: f64,
    /// Vertical page scroll recorded at capture.
    pub scroll_y: f64,
}

impl BrowserImageTransform {
    /// Translate one captured-image point into exact browser input coordinates.
    pub fn image_to_browser(&self, x: f64, y: f64) -> Result<Point, BrowserChallengeFailure> {
        if !x.is_finite()
            || !y.is_finite()
            || x < 0.0
            || y < 0.0
            || x >= f64::from(self.image_width)
            || y >= f64::from(self.image_height)
        {
            return Err(BrowserChallengeFailure::PointOutOfBounds);
        }
        if !self.capture_scale.is_finite() || self.capture_scale <= 0.0 {
            return Err(BrowserChallengeFailure::TransformAmbiguous);
        }
        Ok(Point {
            x: self.browser_geometry.x + x / self.capture_scale,
            y: self.browser_geometry.y + y / self.capture_scale,
        })
    }

    /// Translate one CSS viewport point into captured-image coordinates.
    pub fn browser_to_image(&self, x: f64, y: f64) -> Result<Point, BrowserChallengeFailure> {
        if !x.is_finite()
            || !y.is_finite()
            || !self.capture_scale.is_finite()
            || self.capture_scale <= 0.0
            || x < self.browser_geometry.x
            || y < self.browser_geometry.y
            || x >= self.browser_geometry.x + self.browser_geometry.width
            || y >= self.browser_geometry.y + self.browser_geometry.height
        {
            return Err(BrowserChallengeFailure::TransformAmbiguous);
        }
        Ok(Point {
            x: (x - self.browser_geometry.x) * self.capture_scale,
            y: (y - self.browser_geometry.y) * self.capture_scale,
        })
    }

    /// Translate one browser rectangle into exact integer image-pixel geometry.
    pub fn browser_rect_to_image(
        &self,
        rect: BrowserRect,
    ) -> Result<(u32, u32, u32, u32), BrowserChallengeFailure> {
        if !rect.valid() {
            return Err(BrowserChallengeFailure::TransformAmbiguous);
        }
        let top_left = self.browser_to_image(rect.x, rect.y)?;
        let right = (rect.x + rect.width - self.browser_geometry.x) * self.capture_scale;
        let bottom = (rect.y + rect.height - self.browser_geometry.y) * self.capture_scale;
        if right > f64::from(self.image_width) + GEOMETRY_EPSILON
            || bottom > f64::from(self.image_height) + GEOMETRY_EPSILON
        {
            return Err(BrowserChallengeFailure::TransformAmbiguous);
        }
        let values = [top_left.x, top_left.y, right, bottom];
        if values
            .iter()
            .any(|value| !value.is_finite() || (value - value.round()).abs() > 0.01)
        {
            return Err(BrowserChallengeFailure::TransformAmbiguous);
        }
        let x = top_left.x.round() as u32;
        let y = top_left.y.round() as u32;
        let right = right.round() as u32;
        let bottom = bottom.round() as u32;
        let width = right
            .checked_sub(x)
            .ok_or(BrowserChallengeFailure::TransformAmbiguous)?;
        let height = bottom
            .checked_sub(y)
            .ok_or(BrowserChallengeFailure::TransformAmbiguous)?;
        if width == 0 || height == 0 {
            return Err(BrowserChallengeFailure::TransformAmbiguous);
        }
        Ok((x, y, width, height))
    }
}

/// Authoritative offset from a direct child frame's own CSS viewport origin
/// to its parent's CSS viewport coordinates, derived from the frame-owner
/// element's content box (`DOM.getBoxModel`) through the parent's exact
/// session. For a genuine OOPIF child, composing frame-local geometry into
/// parent/top-level geometry is plain CSS-pixel addition: CDP box-model/
/// content-quad coordinates are already viewport-relative (the owning
/// document's scroll excluded), the same convention
/// `chromiumoxide::element::Element::bounding_box` already relies on for the
/// existing top-level primitive — so no separate DPR or scale factor applies
/// between a child frame and its parent. For a same-session (in-process)
/// child, whose geometry queries already resolve in the top-level document's
/// own coordinate space (same session, same compositor frame tree), the
/// offset is identity (`x`/`y` zero) — see [`resolve_frame_owner_offset`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct FrameOwnerOffset {
    x: f64,
    y: f64,
    /// The frame-owner's own content-box geometry, retained so revalidation
    /// can detect the iframe element moving or resizing without re-deriving
    /// the offset from scratch.
    content_box: BrowserRect,
}

impl FrameOwnerOffset {
    fn apply_to_point(&self, point: Point) -> Point {
        Point {
            x: point.x + self.x,
            y: point.y + self.y,
        }
    }

    fn apply_to_rect(&self, rect: BrowserRect) -> BrowserRect {
        BrowserRect {
            x: rect.x + self.x,
            y: rect.y + self.y,
            width: rect.width,
            height: rect.height,
        }
    }
}

/// Derive the authoritative offset for `frame`, a direct child of
/// `top_level`. Only one level is proven here — `frame.parent_frame_id` must
/// name `top_level.frame_id` exactly; a deeper chain is explicitly
/// unsupported rather than assumed correct by composing unproven levels.
async fn resolve_frame_owner_offset(
    top_level: &FrameContext,
    frame: &FrameContext,
) -> Result<FrameOwnerOffset, BrowserChallengeFailure> {
    if top_level.classification != FrameClassification::TopLevel {
        return Err(BrowserChallengeFailure::UnsupportedContext);
    }
    if frame.parent_frame_id.as_ref() != Some(&top_level.frame_id) {
        return Err(BrowserChallengeFailure::UnsupportedContext);
    }
    let owner = frame
        .frame_owner
        .as_ref()
        .ok_or(BrowserChallengeFailure::FrameGeometryUnavailable)?;
    if owner.owner_target_id != top_level.target_id
        || owner.owner_session_id != top_level.session_id
    {
        return Err(BrowserChallengeFailure::FrameOwnerChanged);
    }
    let model = top_level
        .execute(
            GetBoxModelParams::builder()
                .backend_node_id(owner.backend_node_id)
                .build(),
        )
        .await
        .map_err(|_| BrowserChallengeFailure::FrameGeometryUnavailable)?
        .result
        .model;
    let content_box = rect_from_quad(&model.content)?;
    // A genuinely same-session (in-process) child shares its exact session
    // with `top_level`: CDP's DOM domain resolves `getBoxModel`/
    // `getContentQuads` geometry for such a node already in the top-level
    // document's own viewport coordinates (there is only one compositor
    // frame tree spanning both documents in that case), so composing the
    // frame-owner's own offset on top of it would double it. Only a genuine
    // OOPIF child — a distinct session/target with its own coordinate
    // origin — needs the additive offset. Confirmed empirically: applying
    // it unconditionally lands same-session clicks off-target.
    if frame.classification == FrameClassification::SameSessionChild {
        return Ok(FrameOwnerOffset {
            x: 0.0,
            y: 0.0,
            content_box,
        });
    }
    Ok(FrameOwnerOffset {
        x: content_box.x,
        y: content_box.y,
        content_box,
    })
}

/// Convert one CDP box-model/content quad (8 numbers: four corners) into an
/// axis-aligned [`BrowserRect`]. Fails closed rather than approximating when
/// the quad is not actually axis-aligned (a rotation/skew transform on the
/// frame-owner element, which this primitive does not claim to handle).
fn rect_from_quad(quad: &Quad) -> Result<BrowserRect, BrowserChallengeFailure> {
    let values = quad.inner();
    if values.len() != 8 || values.iter().any(|value| !value.is_finite()) {
        return Err(BrowserChallengeFailure::FrameTransformAmbiguous);
    }
    let xs = [values[0], values[2], values[4], values[6]];
    let ys = [values[1], values[3], values[5], values[7]];
    let left = xs.iter().copied().fold(f64::INFINITY, f64::min);
    let right = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let top = ys.iter().copied().fold(f64::INFINITY, f64::min);
    let bottom = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let axis_aligned = (values[1] - values[3]).abs() <= GEOMETRY_EPSILON
        && (values[2] - values[4]).abs() <= GEOMETRY_EPSILON
        && (values[5] - values[7]).abs() <= GEOMETRY_EPSILON
        && (values[6] - values[0]).abs() <= GEOMETRY_EPSILON;
    let rect = BrowserRect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    };
    if !rect.valid() || !axis_aligned {
        return Err(BrowserChallengeFailure::FrameTransformAmbiguous);
    }
    Ok(rect)
}

/// One retained remote DOM object, bound either through chromiumoxide's own
/// `Element` (top-level captures) or through this frame's exact attached
/// session (frame-scoped captures). `Element` cannot represent the latter:
/// it is hard-tied to a `Page`'s own internal handle and has no way to route
/// commands through an arbitrary [`FrameContext`]-owned session — including
/// a genuine OOPIF child's, which is not the top-level page's session at
/// all. Both variants retain the identical identity facts
/// (`backend_node_id`, `node_id`, `remote_object_id`); only how commands
/// reach the object differs.
enum BoundObject {
    TopLevel(Element),
    Frame {
        backend_node_id: BackendNodeId,
        node_id: NodeId,
        remote_object_id: RemoteObjectId,
    },
}

impl BoundObject {
    fn backend_node_id(&self) -> i64 {
        match self {
            Self::TopLevel(element) => *element.backend_node_id.inner(),
            Self::Frame {
                backend_node_id, ..
            } => *backend_node_id.inner(),
        }
    }

    fn node_id(&self) -> i64 {
        match self {
            Self::TopLevel(element) => *element.node_id.inner(),
            Self::Frame { node_id, .. } => *node_id.inner(),
        }
    }

    /// Border-box geometry, in this object's own session's CSS viewport
    /// coordinates (frame-local for a frame-scoped object). Mirrors
    /// `chromiumoxide::element::Element::bounding_box`'s exact quad choice.
    async fn bounding_box(
        &self,
        frame: Option<&FrameContext>,
    ) -> Result<BrowserRect, BrowserChallengeFailure> {
        match self {
            Self::TopLevel(element) => element
                .bounding_box()
                .await
                .map(BrowserRect::from)
                .map_err(|_| BrowserChallengeFailure::TargetStale),
            Self::Frame {
                backend_node_id, ..
            } => {
                let frame = frame.ok_or(BrowserChallengeFailure::UnsupportedContext)?;
                let model = frame
                    .execute(
                        GetBoxModelParams::builder()
                            .backend_node_id(*backend_node_id)
                            .build(),
                    )
                    .await
                    .map_err(map_frame_context_failure)?
                    .result
                    .model;
                let border = ElementQuad::from_quad(&model.border);
                let rect = BrowserRect {
                    x: border.most_left(),
                    y: border.most_top(),
                    width: border.most_right() - border.most_left(),
                    height: border.most_bottom() - border.most_top(),
                };
                if !rect.valid() {
                    return Err(BrowserChallengeFailure::TargetStale);
                }
                Ok(rect)
            }
        }
    }

    /// The best point to act on, in this object's own session's CSS
    /// viewport coordinates. Mirrors
    /// `chromiumoxide::element::Element::clickable_point`'s exact algorithm:
    /// the center of the first content quad with non-trivial area.
    async fn clickable_point(
        &self,
        frame: Option<&FrameContext>,
    ) -> Result<Point, BrowserChallengeFailure> {
        match self {
            Self::TopLevel(element) => element
                .clickable_point()
                .await
                .map_err(|_| BrowserChallengeFailure::TargetStale),
            Self::Frame {
                backend_node_id, ..
            } => {
                let frame = frame.ok_or(BrowserChallengeFailure::UnsupportedContext)?;
                let quads = frame
                    .execute(
                        GetContentQuadsParams::builder()
                            .backend_node_id(*backend_node_id)
                            .build(),
                    )
                    .await
                    .map_err(map_frame_context_failure)?
                    .result
                    .quads;
                quads
                    .iter()
                    .filter(|quad| quad.inner().len() == 8)
                    .map(ElementQuad::from_quad)
                    .filter(|quad| quad.quad_area() > 1.0)
                    .map(|quad| quad.quad_center())
                    .next()
                    .ok_or(BrowserChallengeFailure::TargetStale)
            }
        }
    }

    /// Call a JavaScript function declaration on the exact retained remote
    /// object. Mirrors `chromiumoxide::element::Element::call_js_fn`.
    async fn call_js_fn(
        &self,
        frame: Option<&FrameContext>,
        function_declaration: impl Into<String>,
        await_promise: bool,
    ) -> Result<CallFunctionOnReturns, BrowserChallengeFailure> {
        match self {
            Self::TopLevel(element) => element
                .call_js_fn(function_declaration, await_promise)
                .await
                .map_err(|_| BrowserChallengeFailure::TargetStale),
            Self::Frame {
                remote_object_id, ..
            } => {
                let frame = frame.ok_or(BrowserChallengeFailure::UnsupportedContext)?;
                let params = CallFunctionOnParams::builder()
                    .object_id(remote_object_id.clone())
                    .function_declaration(function_declaration)
                    .generate_preview(true)
                    .await_promise(await_promise)
                    .build()
                    .map_err(|_| BrowserChallengeFailure::TargetStale)?;
                frame
                    .execute(params)
                    .await
                    .map(|response| response.result)
                    .map_err(map_frame_context_failure)
            }
        }
    }
}

/// Resolve the identity facts a frame-scoped [`BoundObject::Frame`] needs
/// from an already-known `backend_node_id`: never a selector, never a fresh
/// query beyond confirming this exact node still resolves.
async fn resolve_frame_object(
    frame: &FrameContext,
    backend_node_id: BackendNodeId,
) -> Result<BoundObject, BrowserChallengeFailure> {
    let node = frame
        .execute(DescribeNodeParams {
            backend_node_id: Some(backend_node_id),
            ..Default::default()
        })
        .await
        .map_err(map_frame_context_failure)?
        .result
        .node;
    let object = frame
        .execute(
            ResolveNodeParams::builder()
                .backend_node_id(backend_node_id)
                .build(),
        )
        .await
        .map_err(map_frame_context_failure)?
        .result
        .object;
    let remote_object_id = object
        .object_id
        .ok_or(BrowserChallengeFailure::TargetIdentityUnavailable)?;
    Ok(BoundObject::Frame {
        backend_node_id,
        node_id: node.node_id,
        remote_object_id,
    })
}

/// Map the canonical frame-context seam's typed vocabulary onto this
/// seam's own — the single translation point; nothing here re-derives
/// frame/target/session liveness from scratch.
fn map_frame_context_failure(error: FrameContextFailure) -> BrowserChallengeFailure {
    match error {
        FrameContextFailure::FrameUnavailable | FrameContextFailure::FrameDetached => {
            BrowserChallengeFailure::FrameDetached
        }
        FrameContextFailure::FrameNavigated => BrowserChallengeFailure::FrameNavigated,
        FrameContextFailure::TargetDetached | FrameContextFailure::TargetReplaced => {
            BrowserChallengeFailure::TargetReplaced
        }
        FrameContextFailure::TargetSessionUnavailable(_) => BrowserChallengeFailure::TargetReplaced,
        FrameContextFailure::SessionChanged => BrowserChallengeFailure::SessionChanged,
        FrameContextFailure::ExecutionContextUnavailable
        | FrameContextFailure::ExecutionContextAmbiguous
        | FrameContextFailure::ExecutionContextChanged => {
            BrowserChallengeFailure::ExecutionContextChanged
        }
        FrameContextFailure::FrameOwnerUnavailable | FrameContextFailure::FrameOwnerChanged => {
            BrowserChallengeFailure::FrameOwnerChanged
        }
        FrameContextFailure::DomIdentityUnavailable => BrowserChallengeFailure::TargetStale,
        FrameContextFailure::FrameTargetAssociationUnavailable
        | FrameContextFailure::FrameTargetAssociationAmbiguous
        | FrameContextFailure::UnsupportedContext => BrowserChallengeFailure::UnsupportedContext,
    }
}

/// One exact selectable browser target bound to a stable caller ID.
pub struct BoundBrowserTarget {
    /// Stable caller-facing identity.
    pub stable_id: String,
    /// Chromium backend-node identity captured for the exact object.
    pub backend_node_id: i64,
    /// Chromium node identity captured for the exact object.
    pub node_id: i64,
    /// Target geometry at capture time, in top-level viewport coordinates
    /// (identical to frame-local coordinates for a top-level capture).
    pub geometry: BrowserRect,
    nonce: String,
    object: BoundObject,
}

/// Exact horizontal drag in captured image coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrowserHorizontalDrag {
    /// Drag start x coordinate in captured-image pixels.
    pub start_x: f64,
    /// Drag y coordinate in captured-image pixels.
    pub start_y: f64,
    /// Drag end x coordinate in captured-image pixels.
    pub end_x: f64,
}

/// Provider-neutral browser action.
#[derive(Clone, Debug, PartialEq)]
pub enum BrowserChallengeAction {
    /// Click the exact retained browser target bound to `stable_id`.
    ExactTargetClick {
        /// Stable caller-facing target identity captured in the snapshot.
        stable_id: String,
    },
    /// Click an exact point in captured-image coordinates.
    ExactPoint {
        /// Horizontal captured-image coordinate.
        x: f64,
        /// Vertical captured-image coordinate.
        y: f64,
    },
    /// Perform one exact horizontal drag in captured-image coordinates.
    ExactHorizontalDrag(BrowserHorizontalDrag),
}

/// Immutable top-level browser challenge attempt.
pub struct BrowserChallengeSnapshot {
    /// Captured PNG bytes.
    pub visual_bytes: Vec<u8>,
    /// Exact PNG width in pixels.
    pub captured_pixel_width: u32,
    /// Exact PNG height in pixels.
    pub captured_pixel_height: u32,
    /// CSS viewport width at capture.
    pub viewport_width: f64,
    /// CSS viewport height at capture.
    pub viewport_height: f64,
    /// Exact top-level Chromium frame identity.
    pub frame_id: String,
    /// Attempt-local challenge content identity.
    pub challenge_identity: u64,
    /// Authoritative captured-image to browser transform.
    pub transform: BrowserImageTransform,
    /// Exact target bindings indexed by caller-owned stable ID.
    pub targets: HashMap<String, BoundBrowserTarget>,
    challenge_nonce: String,
    challenge: BoundObject,
    /// Present only for a frame-scoped capture (same-origin child or
    /// genuine OOPIF child); absent, `frame_id` above is the top-level
    /// frame and every coordinate is already top-level.
    frame: Option<FrameSnapshotContext>,
}

/// Identity a frame-scoped snapshot retains to prove, at revalidate/apply
/// time, that both the captured browser challenge facts *and* the canonical
/// [`FrameContext`] facts they were derived from are still true. Never a
/// live handle — the caller supplies fresh, already-current
/// `top_level`/`frame` references at each call, exactly like `page: &Page`
/// already is for the top-level path.
struct FrameSnapshotContext {
    top_level_frame_id: FrameId,
    top_level_target_id: TargetId,
    top_level_session_id: SessionId,
    frame_id: FrameId,
    target_id: TargetId,
    session_id: SessionId,
    execution_context_id: ExecutionContextId,
    frame_owner_backend_node_id: BackendNodeId,
    offset: FrameOwnerOffset,
}

impl BrowserChallengeSnapshot {
    /// Capture one already-identified top-level challenge and exact targets.
    pub async fn capture(
        page: &Page,
        challenge: Element,
        targets: Vec<(String, Element)>,
    ) -> Result<Self, BrowserChallengeFailure> {
        if page
            .frames()
            .await
            .map_err(|_| BrowserChallengeFailure::UnsupportedContext)?
            .len()
            != 1
        {
            return Err(BrowserChallengeFailure::UnsupportedContext);
        }
        let frame_id = page
            .mainframe()
            .await
            .map_err(|_| BrowserChallengeFailure::UnsupportedContext)?
            .ok_or(BrowserChallengeFailure::UnsupportedContext)?
            .0;
        let metrics = page
            .layout_metrics()
            .await
            .map_err(|_| BrowserChallengeFailure::SnapshotCaptureFailed)?;
        let viewport = metrics.css_layout_viewport;
        let environment = browser_environment(page).await?;
        let geometry = BrowserRect::from(
            challenge
                .bounding_box()
                .await
                .map_err(|_| BrowserChallengeFailure::SnapshotCaptureFailed)?,
        );
        if !geometry.valid() || environment.dpr <= 0.0 {
            return Err(BrowserChallengeFailure::InvalidSnapshotDimensions);
        }
        let attempt = ATTEMPT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let challenge_nonce = format!("scorpion-browser-challenge-{attempt}");
        let challenge_state = bind_element(&challenge, &challenge_nonce).await?;
        let clip = BrowserRect {
            x: geometry.x + environment.scroll_x,
            y: geometry.y + environment.scroll_y,
            width: geometry.width,
            height: geometry.height,
        };
        let bytes = page
            .screenshot(
                chromiumoxide::page::ScreenshotParams::builder()
                    .format(CaptureScreenshotFormat::Png)
                    .clip(Viewport {
                        x: clip.x,
                        y: clip.y,
                        width: clip.width,
                        height: clip.height,
                        scale: 1.0,
                    })
                    .build(),
            )
            .await
            .map_err(|_| BrowserChallengeFailure::SnapshotCaptureFailed)?;
        let (pixel_width, pixel_height) = png_dimensions(&bytes)?;
        let scale_x = f64::from(pixel_width) / geometry.width;
        let scale_y = f64::from(pixel_height) / geometry.height;
        if !scale_x.is_finite() || (scale_x - scale_y).abs() > 0.01 {
            return Err(BrowserChallengeFailure::TransformAmbiguous);
        }

        let mut bound = HashMap::with_capacity(targets.len());
        let mut backend_ids = HashSet::with_capacity(targets.len());
        for (stable_id, element) in targets {
            if stable_id.is_empty() || bound.contains_key(&stable_id) {
                return Err(BrowserChallengeFailure::TargetIdentityUnavailable);
            }
            let backend_node_id = *element.backend_node_id.inner();
            if !backend_ids.insert(backend_node_id) {
                return Err(BrowserChallengeFailure::TargetIdentityUnavailable);
            }
            let nonce = format!("{challenge_nonce}-target-{stable_id}");
            bind_element(&element, &nonce).await?;
            let target_geometry = BrowserRect::from(
                element
                    .bounding_box()
                    .await
                    .map_err(|_| BrowserChallengeFailure::TargetIdentityUnavailable)?,
            );
            if !target_geometry.valid() {
                return Err(BrowserChallengeFailure::TargetIdentityUnavailable);
            }
            bound.insert(
                stable_id.clone(),
                BoundBrowserTarget {
                    stable_id,
                    backend_node_id,
                    node_id: *element.node_id.inner(),
                    geometry: target_geometry,
                    nonce,
                    object: BoundObject::TopLevel(element),
                },
            );
        }

        Ok(Self {
            visual_bytes: bytes,
            captured_pixel_width: pixel_width,
            captured_pixel_height: pixel_height,
            viewport_width: viewport.client_width as f64,
            viewport_height: viewport.client_height as f64,
            frame_id,
            challenge_identity: challenge_state.identity,
            transform: BrowserImageTransform {
                image_width: pixel_width,
                image_height: pixel_height,
                browser_geometry: geometry,
                capture_clip: clip,
                capture_scale: scale_x,
                device_pixel_ratio: environment.dpr,
                scroll_x: environment.scroll_x,
                scroll_y: environment.scroll_y,
            },
            targets: bound,
            challenge_nonce,
            challenge: BoundObject::TopLevel(challenge),
            frame: None,
        })
    }

    /// Revalidate the exact frame, challenge and target objects without query.
    pub async fn revalidate(&self, page: &Page) -> Result<(), BrowserChallengeFailure> {
        if page
            .frames()
            .await
            .map_err(|_| BrowserChallengeFailure::RevalidationFailed)?
            .len()
            != 1
        {
            return Err(BrowserChallengeFailure::UnsupportedContext);
        }
        let frame = page
            .mainframe()
            .await
            .map_err(|_| BrowserChallengeFailure::RevalidationFailed)?
            .ok_or(BrowserChallengeFailure::RevalidationFailed)?;
        if frame.0 != self.frame_id {
            return Err(BrowserChallengeFailure::RevalidationFailed);
        }
        let environment = browser_environment(page).await?;
        let viewport = page
            .layout_metrics()
            .await
            .map_err(|_| BrowserChallengeFailure::RevalidationFailed)?
            .css_layout_viewport;
        if (environment.scroll_x - self.transform.scroll_x).abs() > GEOMETRY_EPSILON
            || (environment.scroll_y - self.transform.scroll_y).abs() > GEOMETRY_EPSILON
            || (environment.dpr - self.transform.device_pixel_ratio).abs() > 0.001
            || (viewport.client_width as f64 - self.viewport_width).abs() > GEOMETRY_EPSILON
            || (viewport.client_height as f64 - self.viewport_height).abs() > GEOMETRY_EPSILON
        {
            return Err(BrowserChallengeFailure::TransformAmbiguous);
        }
        let state = inspect_object(&self.challenge, None, &self.challenge_nonce).await?;
        if state.identity != self.challenge_identity {
            return Err(BrowserChallengeFailure::ChallengeMutated);
        }
        let geometry = self
            .challenge
            .bounding_box(None)
            .await
            .map_err(|_| BrowserChallengeFailure::TargetStale)?;
        if geometry.materially_differs(self.transform.browser_geometry) {
            return Err(BrowserChallengeFailure::GeometryChanged);
        }
        for target in self.targets.values() {
            inspect_object(&target.object, None, &target.nonce)
                .await
                .map_err(|_| BrowserChallengeFailure::TargetStale)?;
            if target.object.backend_node_id() != target.backend_node_id
                || target.object.node_id() != target.node_id
            {
                return Err(BrowserChallengeFailure::TargetStale);
            }
            let geometry = target
                .object
                .bounding_box(None)
                .await
                .map_err(|_| BrowserChallengeFailure::TargetStale)?;
            if geometry.materially_differs(target.geometry) {
                return Err(BrowserChallengeFailure::GeometryChanged);
            }
        }
        Ok(())
    }

    /// Build an exact image-space drag from one retained target and offset.
    pub fn horizontal_drag_from_target(
        &self,
        stable_id: &str,
        offset: f64,
    ) -> Result<BrowserHorizontalDrag, BrowserChallengeFailure> {
        if !offset.is_finite() {
            return Err(BrowserChallengeFailure::DragOutOfBounds);
        }
        let target = self
            .targets
            .get(stable_id)
            .ok_or(BrowserChallengeFailure::TargetIdentityUnavailable)?;
        let center = self.transform.browser_to_image(
            target.geometry.x + target.geometry.width / 2.0,
            target.geometry.y + target.geometry.height / 2.0,
        )?;
        let end_x = center.x + offset;
        self.transform
            .image_to_browser(end_x, center.y)
            .map_err(|_| BrowserChallengeFailure::DragOutOfBounds)?;
        Ok(BrowserHorizontalDrag {
            start_x: center.x,
            start_y: center.y,
            end_x,
        })
    }

    /// Revalidate then apply exactly one authoritative browser action.
    pub async fn apply(
        &self,
        page: &Page,
        action: BrowserChallengeAction,
    ) -> Result<(), BrowserChallengeFailure> {
        self.revalidate(page).await?;
        match action {
            BrowserChallengeAction::ExactTargetClick { stable_id } => {
                let target = self
                    .targets
                    .get(&stable_id)
                    .ok_or(BrowserChallengeFailure::TargetIdentityUnavailable)?;
                let point = target
                    .object
                    .clickable_point(None)
                    .await
                    .map_err(|_| BrowserChallengeFailure::TargetStale)?;
                if point.x < target.geometry.x
                    || point.y < target.geometry.y
                    || point.x >= target.geometry.x + target.geometry.width
                    || point.y >= target.geometry.y + target.geometry.height
                {
                    return Err(BrowserChallengeFailure::GeometryChanged);
                }
                page.click_smooth(point)
                    .await
                    .map_err(|_| BrowserChallengeFailure::BrowserActionFailed)?;
            }
            BrowserChallengeAction::ExactPoint { x, y } => {
                let point = self.transform.image_to_browser(x, y)?;
                page.click_smooth(point)
                    .await
                    .map_err(|_| BrowserChallengeFailure::BrowserActionFailed)?;
            }
            BrowserChallengeAction::ExactHorizontalDrag(drag) => {
                let from = self
                    .transform
                    .image_to_browser(drag.start_x, drag.start_y)
                    .map_err(|_| BrowserChallengeFailure::DragOutOfBounds)?;
                let to = self
                    .transform
                    .image_to_browser(drag.end_x, drag.start_y)
                    .map_err(|_| BrowserChallengeFailure::DragOutOfBounds)?;
                page.click_and_drag_smooth(from, to)
                    .await
                    .map_err(|_| BrowserChallengeFailure::BrowserActionFailed)?;
            }
        }
        Ok(())
    }

    /// Capture one already-identified challenge and exact targets inside a
    /// direct child frame of `top_level` — a same-origin child sharing
    /// `top_level`'s exact session, or a genuine out-of-process (OOPIF)
    /// child. Only one level is proven; `frame` must be a direct child of
    /// `top_level` ([`resolve_frame_owner_offset`] fails closed otherwise.
    ///
    /// The screenshot is still always captured through the top-level page —
    /// only it can produce one — but every challenge/target geometry fact is
    /// resolved through `frame`'s own exact session and composed into
    /// top-level viewport coordinates via the frame-owner's authoritative
    /// content-box offset before being retained, so every later action still
    /// dispatches through the same exact-action contract
    /// ([`Self::apply_in_frame`]) unchanged.
    pub async fn capture_in_frame(
        page: &Page,
        top_level: &FrameContext,
        frame: &FrameContext,
        challenge_backend_node_id: BackendNodeId,
        targets: Vec<(String, BackendNodeId)>,
    ) -> Result<Self, BrowserChallengeFailure> {
        if frame.classification == FrameClassification::TopLevel {
            return Err(BrowserChallengeFailure::UnsupportedContext);
        }
        let offset = resolve_frame_owner_offset(top_level, frame).await?;

        let metrics = page
            .layout_metrics()
            .await
            .map_err(|_| BrowserChallengeFailure::SnapshotCaptureFailed)?;
        let viewport = metrics.css_layout_viewport;
        let environment = browser_environment(page).await?;

        let challenge = resolve_frame_object(frame, challenge_backend_node_id).await?;
        let local_geometry = challenge
            .bounding_box(Some(frame))
            .await
            .map_err(|_| BrowserChallengeFailure::SnapshotCaptureFailed)?;
        let geometry = offset.apply_to_rect(local_geometry);
        if !geometry.valid() || environment.dpr <= 0.0 {
            return Err(BrowserChallengeFailure::InvalidSnapshotDimensions);
        }
        let attempt = ATTEMPT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let challenge_nonce = format!("scorpion-browser-challenge-{attempt}");
        let challenge_state = bind_object(&challenge, Some(frame), &challenge_nonce).await?;
        let clip = BrowserRect {
            x: geometry.x + environment.scroll_x,
            y: geometry.y + environment.scroll_y,
            width: geometry.width,
            height: geometry.height,
        };
        let bytes = page
            .screenshot(
                chromiumoxide::page::ScreenshotParams::builder()
                    .format(CaptureScreenshotFormat::Png)
                    .clip(Viewport {
                        x: clip.x,
                        y: clip.y,
                        width: clip.width,
                        height: clip.height,
                        scale: 1.0,
                    })
                    .build(),
            )
            .await
            .map_err(|_| BrowserChallengeFailure::SnapshotCaptureFailed)?;
        let (pixel_width, pixel_height) = png_dimensions(&bytes)?;
        let scale_x = f64::from(pixel_width) / geometry.width;
        let scale_y = f64::from(pixel_height) / geometry.height;
        if !scale_x.is_finite() || (scale_x - scale_y).abs() > 0.01 {
            return Err(BrowserChallengeFailure::TransformAmbiguous);
        }

        let mut bound = HashMap::with_capacity(targets.len());
        let mut backend_ids = HashSet::with_capacity(targets.len());
        for (stable_id, backend_node_id) in targets {
            if stable_id.is_empty() || bound.contains_key(&stable_id) {
                return Err(BrowserChallengeFailure::TargetIdentityUnavailable);
            }
            if !backend_ids.insert(*backend_node_id.inner()) {
                return Err(BrowserChallengeFailure::TargetIdentityUnavailable);
            }
            let object = resolve_frame_object(frame, backend_node_id).await?;
            let nonce = format!("{challenge_nonce}-target-{stable_id}");
            bind_object(&object, Some(frame), &nonce).await?;
            let local_geometry = object
                .bounding_box(Some(frame))
                .await
                .map_err(|_| BrowserChallengeFailure::TargetIdentityUnavailable)?;
            let target_geometry = offset.apply_to_rect(local_geometry);
            if !target_geometry.valid() {
                return Err(BrowserChallengeFailure::TargetIdentityUnavailable);
            }
            bound.insert(
                stable_id.clone(),
                BoundBrowserTarget {
                    stable_id,
                    backend_node_id: object.backend_node_id(),
                    node_id: object.node_id(),
                    geometry: target_geometry,
                    nonce,
                    object,
                },
            );
        }

        let owner_backend_node_id = frame
            .frame_owner
            .as_ref()
            .ok_or(BrowserChallengeFailure::FrameGeometryUnavailable)?
            .backend_node_id;

        Ok(Self {
            visual_bytes: bytes,
            captured_pixel_width: pixel_width,
            captured_pixel_height: pixel_height,
            viewport_width: viewport.client_width as f64,
            viewport_height: viewport.client_height as f64,
            frame_id: top_level.frame_id.inner().clone(),
            challenge_identity: challenge_state.identity,
            transform: BrowserImageTransform {
                image_width: pixel_width,
                image_height: pixel_height,
                browser_geometry: geometry,
                capture_clip: clip,
                capture_scale: scale_x,
                device_pixel_ratio: environment.dpr,
                scroll_x: environment.scroll_x,
                scroll_y: environment.scroll_y,
            },
            targets: bound,
            challenge_nonce,
            challenge,
            frame: Some(FrameSnapshotContext {
                top_level_frame_id: top_level.frame_id.clone(),
                top_level_target_id: top_level.target_id.clone(),
                top_level_session_id: top_level.session_id.clone(),
                frame_id: frame.frame_id.clone(),
                target_id: frame.target_id.clone(),
                session_id: frame.session_id.clone(),
                execution_context_id: frame.execution_context_id,
                frame_owner_backend_node_id: owner_backend_node_id,
                offset,
            }),
        })
    }

    /// Revalidate both the canonical [`FrameContext`] facts this capture
    /// depends on and the existing browser-challenge snapshot facts
    /// (viewport/scroll/DPR, frame-owner geometry, challenge/target content
    /// and geometry) — without repairing or re-resolving anything. `top_level`
    /// and `frame` must be the caller's own current, live contexts; any
    /// mismatch against what was captured, or any failure either one's own
    /// [`FrameContext::revalidate`] reports, fails closed.
    pub async fn revalidate_in_frame(
        &self,
        page: &Page,
        top_level: &FrameContext,
        frame: &FrameContext,
    ) -> Result<(), BrowserChallengeFailure> {
        let context = self
            .frame
            .as_ref()
            .ok_or(BrowserChallengeFailure::UnsupportedContext)?;
        if top_level.frame_id != context.top_level_frame_id
            || top_level.target_id != context.top_level_target_id
            || top_level.session_id != context.top_level_session_id
            || frame.frame_id != context.frame_id
            || frame.target_id != context.target_id
            || frame.session_id != context.session_id
        {
            return Err(BrowserChallengeFailure::UnsupportedContext);
        }

        top_level
            .revalidate(None)
            .await
            .map_err(map_frame_context_failure)?;
        frame
            .revalidate(Some(top_level))
            .await
            .map_err(map_frame_context_failure)?;
        if frame.execution_context_id != context.execution_context_id {
            return Err(BrowserChallengeFailure::ExecutionContextChanged);
        }
        let owner = frame
            .frame_owner
            .as_ref()
            .ok_or(BrowserChallengeFailure::FrameOwnerChanged)?;
        if owner.backend_node_id != context.frame_owner_backend_node_id {
            return Err(BrowserChallengeFailure::FrameOwnerChanged);
        }

        let environment = browser_environment(page).await?;
        let viewport = page
            .layout_metrics()
            .await
            .map_err(|_| BrowserChallengeFailure::RevalidationFailed)?
            .css_layout_viewport;
        if (environment.scroll_x - self.transform.scroll_x).abs() > GEOMETRY_EPSILON
            || (environment.scroll_y - self.transform.scroll_y).abs() > GEOMETRY_EPSILON
            || (environment.dpr - self.transform.device_pixel_ratio).abs() > 0.001
            || (viewport.client_width as f64 - self.viewport_width).abs() > GEOMETRY_EPSILON
            || (viewport.client_height as f64 - self.viewport_height).abs() > GEOMETRY_EPSILON
        {
            return Err(BrowserChallengeFailure::TransformAmbiguous);
        }

        // Re-derive the frame-owner offset fresh rather than trusting the
        // captured one: proves the iframe element itself has not moved or
        // resized without silently adopting a replacement offset.
        let fresh_offset = resolve_frame_owner_offset(top_level, frame).await?;
        if fresh_offset
            .content_box
            .materially_differs(context.offset.content_box)
        {
            return Err(BrowserChallengeFailure::GeometryChanged);
        }

        let state = inspect_object(&self.challenge, Some(frame), &self.challenge_nonce).await?;
        if state.identity != self.challenge_identity {
            return Err(BrowserChallengeFailure::ChallengeMutated);
        }
        let local_geometry = self
            .challenge
            .bounding_box(Some(frame))
            .await
            .map_err(|_| BrowserChallengeFailure::TargetStale)?;
        if fresh_offset
            .apply_to_rect(local_geometry)
            .materially_differs(self.transform.browser_geometry)
        {
            return Err(BrowserChallengeFailure::GeometryChanged);
        }
        for target in self.targets.values() {
            inspect_object(&target.object, Some(frame), &target.nonce)
                .await
                .map_err(|_| BrowserChallengeFailure::TargetStale)?;
            if target.object.backend_node_id() != target.backend_node_id
                || target.object.node_id() != target.node_id
            {
                return Err(BrowserChallengeFailure::TargetStale);
            }
            let local_geometry = target
                .object
                .bounding_box(Some(frame))
                .await
                .map_err(|_| BrowserChallengeFailure::TargetStale)?;
            if fresh_offset
                .apply_to_rect(local_geometry)
                .materially_differs(target.geometry)
            {
                return Err(BrowserChallengeFailure::GeometryChanged);
            }
        }
        Ok(())
    }

    /// Revalidate then apply exactly one authoritative browser action inside
    /// a frame-scoped capture — the identical exact-action contract as
    /// [`Self::apply`], composed with the frame's authoritative geometry.
    pub async fn apply_in_frame(
        &self,
        page: &Page,
        top_level: &FrameContext,
        frame: &FrameContext,
        action: BrowserChallengeAction,
    ) -> Result<(), BrowserChallengeFailure> {
        self.revalidate_in_frame(page, top_level, frame).await?;
        match action {
            BrowserChallengeAction::ExactTargetClick { stable_id } => {
                let target = self
                    .targets
                    .get(&stable_id)
                    .ok_or(BrowserChallengeFailure::TargetIdentityUnavailable)?;
                let offset = resolve_frame_owner_offset(top_level, frame).await?;
                let local_point = target
                    .object
                    .clickable_point(Some(frame))
                    .await
                    .map_err(|_| BrowserChallengeFailure::TargetStale)?;
                let point = offset.apply_to_point(local_point);
                if point.x < target.geometry.x
                    || point.y < target.geometry.y
                    || point.x >= target.geometry.x + target.geometry.width
                    || point.y >= target.geometry.y + target.geometry.height
                {
                    return Err(BrowserChallengeFailure::GeometryChanged);
                }
                page.click_smooth(point)
                    .await
                    .map_err(|_| BrowserChallengeFailure::BrowserActionFailed)?;
            }
            BrowserChallengeAction::ExactPoint { x, y } => {
                // `self.transform` was composed into top-level viewport
                // coordinates at capture time, so this is the identical
                // computation `apply` uses for the top-level case.
                let point = self.transform.image_to_browser(x, y)?;
                page.click_smooth(point)
                    .await
                    .map_err(|_| BrowserChallengeFailure::BrowserActionFailed)?;
            }
            BrowserChallengeAction::ExactHorizontalDrag(drag) => {
                let from = self
                    .transform
                    .image_to_browser(drag.start_x, drag.start_y)
                    .map_err(|_| BrowserChallengeFailure::DragOutOfBounds)?;
                let to = self
                    .transform
                    .image_to_browser(drag.end_x, drag.start_y)
                    .map_err(|_| BrowserChallengeFailure::DragOutOfBounds)?;
                page.click_and_drag_smooth(from, to)
                    .await
                    .map_err(|_| BrowserChallengeFailure::BrowserActionFailed)?;
            }
        }
        Ok(())
    }
}

struct BrowserEnvironment {
    dpr: f64,
    scroll_x: f64,
    scroll_y: f64,
}

async fn browser_environment(page: &Page) -> Result<BrowserEnvironment, BrowserChallengeFailure> {
    let value = page
        .evaluate("({dpr:devicePixelRatio,scrollX:scrollX,scrollY:scrollY,top:window===top})")
        .await
        .map_err(|_| BrowserChallengeFailure::UnsupportedContext)?
        .value()
        .cloned()
        .ok_or(BrowserChallengeFailure::UnsupportedContext)?;
    if value.get("top").and_then(Value::as_bool) != Some(true) {
        return Err(BrowserChallengeFailure::UnsupportedContext);
    }
    Ok(BrowserEnvironment {
        dpr: value
            .get("dpr")
            .and_then(Value::as_f64)
            .ok_or(BrowserChallengeFailure::TransformAmbiguous)?,
        scroll_x: value
            .get("scrollX")
            .and_then(Value::as_f64)
            .ok_or(BrowserChallengeFailure::TransformAmbiguous)?,
        scroll_y: value
            .get("scrollY")
            .and_then(Value::as_f64)
            .ok_or(BrowserChallengeFailure::TransformAmbiguous)?,
    })
}

struct ElementState {
    identity: u64,
}

async fn bind_element(
    element: &Element,
    nonce: &str,
) -> Result<ElementState, BrowserChallengeFailure> {
    let escaped = serde_json::to_string(nonce)
        .map_err(|_| BrowserChallengeFailure::TargetIdentityUnavailable)?;
    let function = format!("function(){{if(this.ownerDocument!==document||!this.isConnected)return null;Object.defineProperty(this,'__scorpionChallengeIdentity',{{value:{escaped},configurable:true}});return JSON.stringify({{nonce:this.__scorpionChallengeIdentity,content:this.outerHTML}});}}");
    inspect_binding_result(element.call_js_fn(function, true).await, nonce)
}

/// [`bind_element`]'s exact identity-marking contract, for a [`BoundObject`]
/// (top-level or frame-scoped) instead of a bare `Element`.
async fn bind_object(
    object: &BoundObject,
    frame: Option<&FrameContext>,
    nonce: &str,
) -> Result<ElementState, BrowserChallengeFailure> {
    let escaped = serde_json::to_string(nonce)
        .map_err(|_| BrowserChallengeFailure::TargetIdentityUnavailable)?;
    let function = format!("function(){{if(this.ownerDocument!==document||!this.isConnected)return null;Object.defineProperty(this,'__scorpionChallengeIdentity',{{value:{escaped},configurable:true}});return JSON.stringify({{nonce:this.__scorpionChallengeIdentity,content:this.outerHTML}});}}");
    inspect_object_binding_result(object.call_js_fn(frame, function, true).await, nonce)
}

/// [`inspect_element`]'s exact contract, for a [`BoundObject`].
async fn inspect_object(
    object: &BoundObject,
    frame: Option<&FrameContext>,
    nonce: &str,
) -> Result<ElementState, BrowserChallengeFailure> {
    inspect_object_binding_result(
        object
            .call_js_fn(frame, "function(){if(this.ownerDocument!==document||!this.isConnected)return null;return JSON.stringify({nonce:this.__scorpionChallengeIdentity||null,content:this.outerHTML});}", true)
            .await,
        nonce,
    )
}

fn inspect_object_binding_result(
    result: Result<CallFunctionOnReturns, BrowserChallengeFailure>,
    nonce: &str,
) -> Result<ElementState, BrowserChallengeFailure> {
    let encoded = result?
        .result
        .value
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or(BrowserChallengeFailure::TargetStale)?;
    let value: Value =
        serde_json::from_str(&encoded).map_err(|_| BrowserChallengeFailure::TargetStale)?;
    if value.get("nonce").and_then(Value::as_str) != Some(nonce) {
        return Err(BrowserChallengeFailure::TargetStale);
    }
    let content = value
        .get("content")
        .and_then(Value::as_str)
        .ok_or(BrowserChallengeFailure::TargetStale)?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    Ok(ElementState {
        identity: hasher.finish(),
    })
}

fn inspect_binding_result(
    result: Result<
        chromiumoxide::cdp::js_protocol::runtime::CallFunctionOnReturns,
        chromiumoxide::error::CdpError,
    >,
    nonce: &str,
) -> Result<ElementState, BrowserChallengeFailure> {
    let encoded = result
        .map_err(|_| BrowserChallengeFailure::TargetStale)?
        .result
        .value
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or(BrowserChallengeFailure::TargetStale)?;
    let value: Value =
        serde_json::from_str(&encoded).map_err(|_| BrowserChallengeFailure::TargetStale)?;
    if value.get("nonce").and_then(Value::as_str) != Some(nonce) {
        return Err(BrowserChallengeFailure::TargetStale);
    }
    let content = value
        .get("content")
        .and_then(Value::as_str)
        .ok_or(BrowserChallengeFailure::TargetStale)?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    Ok(ElementState {
        identity: hasher.finish(),
    })
}

fn png_dimensions(bytes: &[u8]) -> Result<(u32, u32), BrowserChallengeFailure> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return Err(BrowserChallengeFailure::InvalidSnapshotDimensions);
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    if width == 0 || height == 0 {
        return Err(BrowserChallengeFailure::InvalidSnapshotDimensions);
    }
    Ok((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_is_exact_and_never_clamps() {
        let transform = BrowserImageTransform {
            image_width: 400,
            image_height: 200,
            browser_geometry: BrowserRect {
                x: 10.0,
                y: 20.0,
                width: 200.0,
                height: 100.0,
            },
            capture_clip: BrowserRect {
                x: 10.0,
                y: 20.0,
                width: 200.0,
                height: 100.0,
            },
            capture_scale: 2.0,
            device_pixel_ratio: 2.0,
            scroll_x: 0.0,
            scroll_y: 0.0,
        };
        assert_eq!(
            transform.image_to_browser(100.0, 80.0).unwrap(),
            Point { x: 60.0, y: 60.0 }
        );
        assert_eq!(
            transform.image_to_browser(401.0, 0.0),
            Err(BrowserChallengeFailure::PointOutOfBounds)
        );
        assert_eq!(
            transform.browser_to_image(60.0, 60.0).unwrap(),
            Point { x: 100.0, y: 80.0 }
        );
        assert_eq!(
            transform
                .browser_rect_to_image(BrowserRect {
                    x: 20.0,
                    y: 30.0,
                    width: 40.0,
                    height: 20.0,
                })
                .unwrap(),
            (20, 20, 80, 40)
        );
    }

    #[test]
    fn png_identity_is_exact() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&320u32.to_be_bytes());
        png.extend_from_slice(&224u32.to_be_bytes());
        assert_eq!(png_dimensions(&png), Ok((320, 224)));
    }
}
