/// Page viewport configuration used when emulating screen metrics.
///
/// Width/height are in CSS pixels. Optional fields allow tuning device pixel
/// ratio, mobile emulation, orientation, and touch support.
#[derive(Debug, Clone, PartialEq)]
pub struct Viewport {
    /// CSS pixel width of the viewport (layout viewport, not device pixels).
    pub width: u32,
    /// CSS pixel height of the viewport.
    pub height: u32,
    /// Device pixel ratio (DPR). If `None`, the browser default is used.
    /// Common values: `1.0` for standard displays, `2.0` for “Retina”-like.
    pub device_scale_factor: Option<f64>,
    /// Simulate a mobile device (affects UA hints/metrics in some engines).
    /// Set to `true` to enable mobile-specific layout behavior.
    pub emulating_mobile: bool,
    /// Treat the viewport as landscape (`true`) or portrait (`false`).
    pub is_landscape: bool,
    /// Advertise touch support (affects input/event capability).
    /// Set to `true` to enable touch-enabled emulation.
    pub has_touch: bool,
}

impl Default for Viewport {
    /// Default viewport size.
    fn default() -> Self {
        Viewport {
            width: 800,
            height: 600,
            device_scale_factor: None,
            emulating_mobile: false,
            is_landscape: false,
            has_touch: false,
        }
    }
}
