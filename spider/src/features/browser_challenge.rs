//! Canonical provider-neutral browser challenge snapshot and action seam.
//!
//! Initial support is deliberately limited to a top-level Chromium frame. A
//! snapshot retains the exact remote DOM objects supplied by its caller and
//! never re-queries selectors when applying an action.

#![cfg(feature = "chrome")]

use chromiumoxide::cdp::browser_protocol::page::{CaptureScreenshotFormat, Viewport};
use chromiumoxide::element::Element;
use chromiumoxide::layout::{BoundingBox, Point};
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
}

/// One exact selectable browser target bound to a stable caller ID.
pub struct BoundBrowserTarget {
    /// Stable caller-facing identity.
    pub stable_id: String,
    /// Chromium backend-node identity captured for the exact object.
    pub backend_node_id: i64,
    /// Chromium node identity captured for the exact object.
    pub node_id: i64,
    /// Target geometry at capture time.
    pub geometry: BrowserRect,
    nonce: String,
    element: Element,
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
    challenge: Element,
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
                    element,
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
            challenge,
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
        let state = inspect_element(&self.challenge, &self.challenge_nonce).await?;
        if state.identity != self.challenge_identity {
            return Err(BrowserChallengeFailure::ChallengeMutated);
        }
        let geometry = self
            .challenge
            .bounding_box()
            .await
            .map(BrowserRect::from)
            .map_err(|_| BrowserChallengeFailure::TargetStale)?;
        if geometry.materially_differs(self.transform.browser_geometry) {
            return Err(BrowserChallengeFailure::GeometryChanged);
        }
        for target in self.targets.values() {
            inspect_element(&target.element, &target.nonce)
                .await
                .map_err(|_| BrowserChallengeFailure::TargetStale)?;
            if *target.element.backend_node_id.inner() != target.backend_node_id
                || *target.element.node_id.inner() != target.node_id
            {
                return Err(BrowserChallengeFailure::TargetStale);
            }
            let geometry = target
                .element
                .bounding_box()
                .await
                .map(BrowserRect::from)
                .map_err(|_| BrowserChallengeFailure::TargetStale)?;
            if geometry.materially_differs(target.geometry) {
                return Err(BrowserChallengeFailure::GeometryChanged);
            }
        }
        Ok(())
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
                    .element
                    .clickable_point()
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

async fn inspect_element(
    element: &Element,
    nonce: &str,
) -> Result<ElementState, BrowserChallengeFailure> {
    inspect_binding_result(
        element
            .call_js_fn("function(){if(this.ownerDocument!==document||!this.isConnected)return null;return JSON.stringify({nonce:this.__scorpionChallengeIdentity||null,content:this.outerHTML});}", true)
            .await,
        nonce,
    )
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
    }

    #[test]
    fn png_identity_is_exact() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&320u32.to_be_bytes());
        png.extend_from_slice(&224u32.to_be_bytes());
        assert_eq!(png_dimensions(&png), Ok((320, 224)));
    }
}
