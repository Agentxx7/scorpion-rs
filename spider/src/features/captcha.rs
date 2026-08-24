//! Provider-neutral CAPTCHA solving vocabulary and dispatch boundary.
//!
//! Detection, browser extraction/application, provider selection, retries and
//! admission policy remain caller-owned. Providers receive normalized visual
//! inputs and never receive browser handles or raw HTTP clients.

use std::time::Duration;

use spider_transport::{BackendProvenance, CrawlerFailure, ResponseOrigin};

/// A provider-independent class of visual CAPTCHA task.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CaptchaChallengeKind {
    /// Select zero or more identified images matching an instruction.
    ImageGridSelection,
    /// Locate a horizontal position within one visual input.
    HorizontalOffset,
    /// Locate a two-dimensional point within one visual input.
    PointSelection,
}

/// Summary of one passive browser-challenge detection pass, retained on the
/// crawled-result record (`crate::page::Page::detected_browser_challenge`)
/// so callers — including outside this crate, through
/// `Page::detected_browser_challenge()` — can observe what
/// [`crate::features::browser_challenge_detection`] found and, when a
/// provider was configured, what the canonical router did about it.
/// Deliberately not the live `BrowserChallengeSnapshot` — that snapshot's
/// CDP object handles stop being valid the moment the shared chrome-backed
/// constructor returns and the navigating tab's caller proceeds, which
/// happens before any caller outside that construction could observe this
/// field, so retaining only plain evidence (frame identity, element ids,
/// captured bytes) is what a caller can actually act on, not a
/// papered-over `Clone`/`Debug` workaround. Public (not a registry, not a
/// provider handle, not a browser-action dispatcher) precisely so a
/// shipping binary that can now *select* a provider
/// (`SCORPION_CANONICAL_CAPTCHA_PROVIDER_PUBLIC_SELECTION_BINDING_001`) can
/// also observe, at minimum, whether a challenge was found and whether a
/// solution was produced — never by reconstructing routing itself.
#[cfg(feature = "chrome")]
#[derive(Clone, Debug, PartialEq)]
pub enum BrowserChallengeObservation {
    /// The page was inspected under Chrome and no supported challenge
    /// evidence was found. Distinct from the field being `None`, which
    /// means detection was never attempted (e.g. HTTP-only transport).
    NoChallenge,
    /// A supported challenge was found and fully materialized in the
    /// page's top-level document.
    TopLevel {
        /// Provider-neutral challenge shape.
        kind: CaptchaChallengeKind,
        /// Exact top-level `FrameId` (as reported by Chromium).
        frame_id: String,
        /// `id` attribute of the matched challenge-candidate element.
        challenge_element_id: String,
        /// `id` attributes of the matched target-candidate elements.
        target_element_ids: Vec<String>,
        /// Captured PNG bytes of the challenge region at detection time.
        visual_bytes: std::sync::Arc<[u8]>,
        /// Exact PNG width in pixels.
        captured_pixel_width: u32,
        /// Exact PNG height in pixels.
        captured_pixel_height: u32,
        /// Outcome of routing this challenge through
        /// [`crate::features::solvers::route_detected_browser_challenge`].
        route_outcome: CaptchaRouteOutcomeSummary,
    },
    /// A supported challenge was found inside a same-session child frame.
    /// Frame identity is always real and correct; `materialized` reports
    /// whether this evidence could also be turned into an action-ready,
    /// frame-aware snapshot (`SCORPION_CANONICAL_CAPTCHA_FRAME_ACTION_BINDING_001`)
    /// — see [`crate::features::browser_challenge_detection`]'s module
    /// docs for exactly what that can and cannot prove (a genuine
    /// out-of-process/OOPIF child never materializes through this seam).
    Framed {
        /// Exact `FrameId` owning the matched challenge evidence.
        frame_id: String,
        /// Exact `FrameId` of the parent document, when known.
        parent_frame_id: Option<String>,
        /// `id` attribute of the matched challenge-candidate element.
        challenge_element_id: String,
        /// Whether this evidence was materialized into an action-ready
        /// snapshot. `false` covers both "no live browser handle was
        /// offered to detection" and "a handle was offered but frame-context
        /// resolution or capture failed" — the specific reason is available
        /// only in-process at detection time, not retained here (matching
        /// `ProviderFailed`'s own reduced-detail precedent).
        materialized: bool,
        /// Outcome of routing this challenge through
        /// [`crate::features::solvers::route_detected_framed_browser_challenge`],
        /// when `materialized` is `true`. `NotConfigured` whenever
        /// `materialized` is `false` (never routed) or no provider was
        /// selected.
        route_outcome: CaptchaRouteOutcomeSummary,
    },
    /// Detection was attempted and failed with a typed observation error.
    /// Distinct from `NoChallenge`: this means the observation itself could
    /// not be trusted, not that the page was inspected and found clean.
    DetectionFailed,
}

/// Public, `Clone + Debug + PartialEq`-safe summary of one canonical
/// provider-routing attempt for a detected browser challenge, retained
/// alongside [`BrowserChallengeObservation::TopLevel`]. Reduced from
/// [`CaptchaSolveOutcome`] the same way `BrowserChallengeObservation` itself
/// is reduced from the live `BrowserChallengeSnapshot` — `CaptchaSolveFailure`
/// is intentionally not `Clone`/`PartialEq`, so its debug text is retained
/// rather than the value itself. This is the coarse, five-way distinction a
/// caller outside this crate can make (`NotConfigured` /
/// `ProviderUnavailable` / `UnsupportedChallenge` / `ProviderFailed` /
/// `SolutionProduced`) — the specific internal failure reason behind
/// `ProviderFailed`'s debug text (for example PaliGemma's own richer typed
/// failure vocabulary) is intentionally not redesigned or widened by
/// exposing this type; that remains a separate observability frontier's
/// concern.
///
/// `SolutionProduced` is deliberately not named `Solved`: a provider
/// returning a normalized answer is not itself a claim that the browser
/// challenge was actually solved. `SCORPION_CANONICAL_CAPTCHA_SOLUTION_BROWSER_ACTION_BINDING_001`
/// binds a produced solution to a real browser action through the existing,
/// pre-proven `crate::features::captcha_browser::execute_browser_captcha_attempt`
/// seam and carries that action's own truthful outcome as `action` —
/// still never equating a successfully dispatched action with "solved";
/// see [`CaptchaBrowserActionOutcome`]'s doc comment.
#[cfg(feature = "chrome")]
#[derive(Clone, Debug, PartialEq)]
pub enum CaptchaRouteOutcomeSummary {
    /// No provider was configured (`Configuration::captcha_provider` is
    /// `None`); the router returns this without constructing any registry
    /// or provider — no model init, no credential lookup, no external call.
    NotConfigured,
    /// The configured provider could not be resolved (not registered in
    /// this build/runtime) or reported itself unavailable / missing
    /// credentials before execution.
    ProviderUnavailable,
    /// The configured, resolved provider does not support this challenge's
    /// kind, media type, or input shape. `solve()` was never called.
    UnsupportedChallenge,
    /// The provider executed and returned a typed failure, preserved as
    /// its `Debug` text (`CaptchaSolveFailure` is not `Clone`/`PartialEq`).
    ProviderFailed(String),
    /// The provider executed and returned a normalized solution. Not a
    /// claim of "solved" — see the type doc comment. `action` is the
    /// truthful, separately-owned outcome of binding that solution to the
    /// real browser, if any binding was attempted at this call site.
    SolutionProduced {
        /// Outcome of applying the produced solution to the live browser.
        action: CaptchaBrowserActionOutcome,
    },
}

/// Truthful outcome of binding one produced [`CaptchaRouteOutcomeSummary::SolutionProduced`]
/// solution to the real browser through
/// `crate::features::captcha_browser::execute_browser_captcha_attempt`.
/// Deliberately does not contain a "solved" state: dispatching a real
/// browser action (a real CDP mouse click at a real transformed point) is
/// observable and reported here as `Applied`, but whether the underlying
/// challenge actually progressed as a result stays a separate, narrower,
/// best-effort observation (`challenge_observed_after_action`) — one more
/// pass of the exact same evidence-based detection convention used to find
/// the challenge in the first place, never a provider- or fixture-specific
/// "solved" heuristic, and never a claim beyond what that convention can
/// truthfully support.
#[cfg(feature = "chrome")]
#[derive(Clone, Debug, PartialEq)]
pub enum CaptchaBrowserActionOutcome {
    /// A solution was produced but this call site never attempts a browser
    /// action (no live page/snapshot was available to bind against).
    NotAttempted,
    /// One or more real, exact browser actions were dispatched successfully.
    Applied {
        /// Number of exact browser actions successfully dispatched.
        actions_applied: usize,
        /// Result of re-running the same passive, provider-neutral
        /// challenge detector once, immediately after the action: `true`
        /// when the same evidence-based challenge convention still matches
        /// this exact challenge element afterwards (best-effort "not yet
        /// gone" signal), `false` when it no longer does (best-effort
        /// "state changed" signal). Never a general CAPTCHA-solved claim.
        challenge_observed_after_action: bool,
    },
    /// A real solution was produced but binding it to the browser failed —
    /// materialization, revalidation, bounds or the exact action dispatch
    /// itself. Preserved as `Debug` text
    /// (`CaptchaBrowserExecutionFailure` is not `Clone`/`PartialEq`).
    Failed(String),
}

/// One explicitly identified cell in a materialized full-grid image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptchaImageGridCell {
    choice_id: String,
    row: usize,
    column: usize,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl CaptchaImageGridCell {
    /// Construct one cell in original full-grid image coordinates.
    pub fn new(
        choice_id: impl Into<String>,
        row: usize,
        column: usize,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Self {
        Self {
            choice_id: choice_id.into(),
            row,
            column,
            x,
            y,
            width,
            height,
        }
    }

    /// Stable caller-assigned choice identity.
    pub fn choice_id(&self) -> &str {
        &self.choice_id
    }

    /// Zero-based grid row.
    pub const fn row(&self) -> usize {
        self.row
    }

    /// Zero-based grid column.
    pub const fn column(&self) -> usize {
        self.column
    }

    /// Cell rectangle `(x, y, width, height)` in original-image coordinates.
    pub const fn geometry(&self) -> (u32, u32, u32, u32) {
        (self.x, self.y, self.width, self.height)
    }

    /// Left edge in original-image coordinates.
    pub const fn x(&self) -> u32 {
        self.x
    }

    /// Top edge in original-image coordinates.
    pub const fn y(&self) -> u32 {
        self.y
    }

    /// Cell width in original-image pixels.
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Cell height in original-image pixels.
    pub const fn height(&self) -> u32 {
        self.height
    }
}

/// Validation failure for canonical materialized full-grid semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptchaImageGridValidationError {
    /// Image dimensions, row count, or column count is zero.
    InvalidDimensions,
    /// The supplied visual is not already materialized bytes.
    FullGridNotMaterialized,
    /// Cell count does not equal `rows * columns`.
    ChoiceCountMismatch,
    /// A stable choice identity is empty or duplicated.
    InvalidChoiceIdentity,
    /// A row/column position is missing, duplicated, or outside the grid.
    InvalidGridPosition,
    /// A cell has zero area or extends outside the original image.
    CellOutsideImage,
    /// Two cell rectangles overlap with positive area.
    AmbiguousCellOverlap,
}

/// One already-materialized full-grid image with complete canonical grid
/// semantics. Construction validates and stores cells in row-major order;
/// identity always comes from each cell's explicit `choice_id`.
#[derive(Clone, Debug)]
pub struct CaptchaImageGridInput {
    full_grid: Box<CaptchaVisualInput>,
    original_width: u32,
    original_height: u32,
    rows: usize,
    columns: usize,
    cells: Vec<CaptchaImageGridCell>,
    empty_selection_valid: bool,
}

impl CaptchaImageGridInput {
    /// Validate and construct one canonical materialized full-grid input.
    pub fn new(
        full_grid: CaptchaVisualInput,
        original_dimensions: (u32, u32),
        rows: usize,
        columns: usize,
        mut cells: Vec<CaptchaImageGridCell>,
        empty_selection_valid: bool,
    ) -> Result<Self, CaptchaImageGridValidationError> {
        if !matches!(full_grid, CaptchaVisualInput::Materialized { .. }) {
            return Err(CaptchaImageGridValidationError::FullGridNotMaterialized);
        }
        let (original_width, original_height) = original_dimensions;
        if original_width == 0 || original_height == 0 || rows == 0 || columns == 0 {
            return Err(CaptchaImageGridValidationError::InvalidDimensions);
        }
        let expected = rows
            .checked_mul(columns)
            .ok_or(CaptchaImageGridValidationError::ChoiceCountMismatch)?;
        if cells.len() != expected {
            return Err(CaptchaImageGridValidationError::ChoiceCountMismatch);
        }
        let mut ids = std::collections::HashSet::with_capacity(expected);
        let mut positions = std::collections::HashSet::with_capacity(expected);
        for cell in &cells {
            if cell.choice_id.is_empty() || !ids.insert(cell.choice_id.clone()) {
                return Err(CaptchaImageGridValidationError::InvalidChoiceIdentity);
            }
            if cell.row >= rows
                || cell.column >= columns
                || !positions.insert((cell.row, cell.column))
            {
                return Err(CaptchaImageGridValidationError::InvalidGridPosition);
            }
            let Some(right) = cell.x.checked_add(cell.width) else {
                return Err(CaptchaImageGridValidationError::CellOutsideImage);
            };
            let Some(bottom) = cell.y.checked_add(cell.height) else {
                return Err(CaptchaImageGridValidationError::CellOutsideImage);
            };
            if cell.width == 0
                || cell.height == 0
                || right > original_width
                || bottom > original_height
            {
                return Err(CaptchaImageGridValidationError::CellOutsideImage);
            }
        }
        for (index, first) in cells.iter().enumerate() {
            for second in cells.iter().skip(index + 1) {
                if rectangles_overlap(first, second) {
                    return Err(CaptchaImageGridValidationError::AmbiguousCellOverlap);
                }
            }
        }
        cells.sort_by_key(|cell| (cell.row, cell.column));
        if cells
            .iter()
            .enumerate()
            .any(|(index, cell)| (cell.row, cell.column) != (index / columns, index % columns))
        {
            return Err(CaptchaImageGridValidationError::InvalidGridPosition);
        }
        Ok(Self {
            full_grid: Box::new(full_grid),
            original_width,
            original_height,
            rows,
            columns,
            cells,
            empty_selection_valid,
        })
    }

    /// Already-materialized full-grid visual.
    pub fn full_grid(&self) -> &CaptchaVisualInput {
        &self.full_grid
    }

    /// Original full-grid dimensions.
    pub const fn original_dimensions(&self) -> (u32, u32) {
        (self.original_width, self.original_height)
    }

    /// Grid dimensions `(rows, columns)`.
    pub const fn layout(&self) -> (usize, usize) {
        (self.rows, self.columns)
    }

    /// Number of grid rows.
    pub const fn rows(&self) -> usize {
        self.rows
    }

    /// Number of grid columns.
    pub const fn columns(&self) -> usize {
        self.columns
    }

    /// Complete stable mapping in canonical row-major order.
    pub fn cells(&self) -> &[CaptchaImageGridCell] {
        &self.cells
    }

    /// Whether a successful empty selected-ID set is semantically valid.
    pub const fn empty_selection_valid(&self) -> bool {
        self.empty_selection_valid
    }
}

fn rectangles_overlap(first: &CaptchaImageGridCell, second: &CaptchaImageGridCell) -> bool {
    first.x < second.x + second.width
        && second.x < first.x + first.width
        && first.y < second.y + second.height
        && second.y < first.y + first.height
}

/// One visual input. Remote assets are planning inputs and must be
/// materialized through canonical transport before provider execution.
#[derive(Clone, Debug)]
pub enum CaptchaVisualInput {
    /// Bytes already acquired without giving the provider transport authority.
    Materialized {
        /// Optional stable choice identity.
        id: Option<String>,
        /// Declared media type.
        media_type: String,
        /// Immutable visual bytes.
        bytes: std::sync::Arc<[u8]>,
    },
    /// A caller-side acquisition plan that providers must never receive.
    RemoteAsset {
        /// Optional stable choice identity.
        id: Option<String>,
        /// Expected media type.
        media_type: String,
        /// Canonically validated acquisition target.
        url: url::Url,
    },
    /// One full-grid image whose cell semantics have already been validated.
    MaterializedFullGrid(Box<CaptchaImageGridInput>),
}

impl CaptchaVisualInput {
    /// Construct an already-materialized visual.
    pub fn materialized(
        id: Option<String>,
        media_type: impl Into<String>,
        bytes: impl Into<std::sync::Arc<[u8]>>,
    ) -> Self {
        Self::Materialized {
            id,
            media_type: media_type.into(),
            bytes: bytes.into(),
        }
    }

    /// Wrap a validated materialized full-grid input without reinterpreting an
    /// existing collection of ordinary visuals.
    pub fn materialized_full_grid(grid: CaptchaImageGridInput) -> Self {
        Self::MaterializedFullGrid(Box::new(grid))
    }

    /// Return the declared media type.
    pub fn media_type(&self) -> &str {
        match self {
            Self::Materialized { media_type, .. } | Self::RemoteAsset { media_type, .. } => {
                media_type
            }
            Self::MaterializedFullGrid(grid) => grid.full_grid().media_type(),
        }
    }

    /// Return the optional choice identity.
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::Materialized { id, .. } | Self::RemoteAsset { id, .. } => id.as_deref(),
            Self::MaterializedFullGrid(grid) => grid.full_grid().id(),
        }
    }

    /// Return materialized bytes, or `None` for an acquisition plan.
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Materialized { bytes, .. } => Some(bytes),
            Self::RemoteAsset { .. } => None,
            Self::MaterializedFullGrid(grid) => grid.full_grid().bytes(),
        }
    }

    /// Return canonical full-grid semantics for the explicit full-grid form.
    pub fn image_grid(&self) -> Option<&CaptchaImageGridInput> {
        match self {
            Self::MaterializedFullGrid(grid) => Some(grid),
            Self::Materialized { .. } | Self::RemoteAsset { .. } => None,
        }
    }
}

/// A normalized challenge, independent of CAPTCHA vendor and solver provider.
#[derive(Clone, Debug)]
pub struct CaptchaChallenge {
    /// Provider-neutral task shape.
    pub kind: CaptchaChallengeKind,
    /// Semantic task instruction, not a provider protocol payload.
    pub instruction: String,
    /// Visual inputs in caller-defined order.
    pub visuals: Vec<CaptchaVisualInput>,
}

/// Stable solver-provider identity, distinct from HTTP backend provenance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CaptchaProviderId(&'static str);

impl CaptchaProviderId {
    /// Chrome's local in-page LanguageModel provider.
    pub const LOCAL_LANGUAGE_MODEL: Self = Self("local-language-model");
    /// External Gemini provider reached through canonical transport.
    pub const EXTERNAL_GEMINI: Self = Self("external-gemini");
    /// OpenAI vision provider reached through canonical transport.
    pub const OPENAI_VISION: Self = Self("openai-vision");
    /// Embedded PaliGemma runtime using the canonical local-model contract
    /// (SigLIP + Gemma).
    pub const PALIGEMMA_LOCAL: Self = Self("paligemma-local");

    /// Return the stable provider label.
    pub const fn as_str(self) -> &'static str {
        self.0
    }

    /// The complete, closed set of known provider identities. Used to
    /// validate configuration/deserialization input against exactly the
    /// providers this crate's vocabulary actually knows about — never an
    /// open string. Public so a shipping binary's own CLI/config parser
    /// (e.g. `--captcha-provider`) can list the exact same canonical set
    /// in its own error messages instead of hand-duplicating provider
    /// strings.
    pub const KNOWN: &'static [Self] = &[
        Self::LOCAL_LANGUAGE_MODEL,
        Self::EXTERNAL_GEMINI,
        Self::OPENAI_VISION,
        Self::PALIGEMMA_LOCAL,
    ];

    /// Resolve a stable label back to its known provider identity. `None`
    /// for any string outside the closed set above — never a fabricated
    /// identity.
    pub fn from_str_exact(label: &str) -> Option<Self> {
        Self::KNOWN.iter().copied().find(|id| id.0 == label)
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for CaptchaProviderId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.0)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for CaptchaProviderId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let label = String::deserialize(deserializer)?;
        Self::from_str_exact(&label).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "unknown captcha provider id {label:?}; expected one of {:?}",
                Self::KNOWN.iter().map(|id| id.as_str()).collect::<Vec<_>>()
            ))
        })
    }
}

/// Qualification state is independent of whether a challenge representation
/// is executable by a provider adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptchaCapabilityQualification {
    /// Empirical qualification is not part of this provider's capability
    /// contract (for example external or compatibility providers).
    NotApplicable,
    /// The shape is executable, but no empirical accuracy claim is permitted.
    ExecutableUnqualified,
    /// A separately pinned empirical evaluation authorizes advertisement.
    EmpiricallyQualified,
}

/// Whether provider execution is local or uses external transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptchaProviderLocality {
    /// Execution occurs locally without HTTP authority.
    Local,
    /// Execution calls an external service through canonical transport.
    External,
}

/// Immutable provider capability advertisement.
#[derive(Clone, Copy, Debug)]
pub struct CaptchaProviderCapabilities {
    /// Provider being advertised.
    pub provider: CaptchaProviderId,
    /// Provider execution locality.
    pub locality: CaptchaProviderLocality,
    /// Challenge kinds accepted by the provider.
    pub supported_kinds: &'static [CaptchaChallengeKind],
    /// Materialized media types accepted by the provider.
    pub supported_media_types: &'static [&'static str],
    /// Maximum visual inputs accepted per request.
    pub maximum_inputs: usize,
    /// Whether provider execution requires a credential.
    pub requires_credentials: bool,
}

/// Read-only runtime availability reported by a provider adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptchaProviderAvailability {
    /// The provider can accept an eligible request.
    Available,
    /// The provider runtime is not currently available.
    ProviderUnavailable,
    /// The provider requires credentials that are not currently available.
    CredentialUnavailable,
}

/// A caller-routed solve request. Exactly one provider is selected before
/// dispatch; the core contains no fallback chain or substitution policy.
#[derive(Clone, Debug)]
pub struct CaptchaSolveRequest {
    /// Caller-owned correlation identity.
    pub correlation_id: String,
    /// Exactly one explicitly selected provider.
    pub selected_provider: CaptchaProviderId,
    /// Normalized challenge.
    pub challenge: CaptchaChallenge,
    /// Caller-owned operation deadline.
    pub deadline: Duration,
}

/// A normalized successful solution.
#[derive(Clone, Debug, PartialEq)]
pub enum CaptchaSolution {
    /// Stable choice identities selected from an image grid.
    SelectedChoices(Vec<String>),
    /// Horizontal coordinate within the supplied visual space.
    HorizontalOffset(f64),
    /// Two-dimensional coordinate within the supplied visual space.
    Point {
        /// Horizontal coordinate.
        x: f64,
        /// Vertical coordinate.
        y: f64,
    },
}

/// Provider identity plus execution facts. HTTP backend identity is optional
/// and deliberately cannot be used as provider identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptchaSolveProvenance {
    /// Solver-provider identity.
    pub provider: CaptchaProviderId,
    /// Local or external execution.
    pub locality: CaptchaProviderLocality,
    /// Actual HTTP backend for external execution only.
    pub transport_backend: Option<BackendProvenance>,
    /// Canonical response origin for external execution only.
    pub response_origin: Option<ResponseOrigin>,
    /// Detailed local runtime facts, absent for browser-local compatibility and
    /// external providers.
    pub local_runtime: Option<CaptchaLocalRuntimeProvenance>,
}

/// Immutable facts for one local model execution attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptchaLocalRuntimeProvenance {
    /// Exact immutable model revision.
    pub model_revision: String,
    /// Stable device/dtype/runtime identity.
    pub runtime_identity: String,
    /// Stable preprocessing identity.
    pub processor_identity: String,
    /// Challenge kind executed.
    pub challenge_kind: CaptchaChallengeKind,
    /// Deterministic prompt and output grammar identity.
    pub prompt_grammar_identity: String,
    /// Time spent in this provider attempt.
    pub elapsed: Duration,
    /// Whether strict provider translation produced a solution.
    pub succeeded: bool,
}

impl CaptchaSolveProvenance {
    /// Construct local provenance without transport claims.
    pub fn local(provider: CaptchaProviderId) -> Self {
        Self {
            provider,
            locality: CaptchaProviderLocality::Local,
            transport_backend: None,
            response_origin: None,
            local_runtime: None,
        }
    }

    /// Construct external provenance from canonical transport facts.
    pub fn external(
        provider: CaptchaProviderId,
        backend: BackendProvenance,
        origin: ResponseOrigin,
    ) -> Self {
        Self {
            provider,
            locality: CaptchaProviderLocality::External,
            transport_backend: Some(backend),
            response_origin: Some(origin),
            local_runtime: None,
        }
    }

    /// Construct detailed local runtime provenance without transport claims.
    pub fn local_runtime(
        provider: CaptchaProviderId,
        facts: CaptchaLocalRuntimeProvenance,
    ) -> Self {
        Self {
            provider,
            locality: CaptchaProviderLocality::Local,
            transport_backend: None,
            response_origin: None,
            local_runtime: Some(facts),
        }
    }
}

/// Explicit provider-neutral failure vocabulary.
#[derive(Debug)]
pub enum CaptchaSolveFailure {
    /// Challenge structure or values are invalid.
    InvalidChallenge,
    /// Selected provider does not support the challenge.
    UnsupportedChallenge,
    /// Provider runtime is not available.
    ProviderUnavailable,
    /// Required provider credentials are absent.
    CredentialUnavailable,
    /// Caller-owned deadline elapsed.
    DeadlineExceeded,
    /// Canonical transport failed.
    Transport(CrawlerFailure),
    /// Provider rejected the request or returned unsuccessful status.
    ProviderRejected,
    /// Provider response could not be translated truthfully.
    InvalidProviderResponse,
    /// Provider completed without a conclusive answer.
    Inconclusive,
    /// Local provider execution failed.
    LocalExecutionFailure,
    /// Caller cancelled execution.
    Cancelled,
}

/// A solve always returns an explicit solution or failure; empty selections
/// and zero coordinates remain valid values rather than failure sentinels.
#[derive(Debug)]
pub enum CaptchaSolveOutcome {
    /// A successful normalized solution with provenance.
    Solved {
        /// Provider-neutral answer.
        solution: CaptchaSolution,
        /// Solver and optional transport provenance.
        provenance: CaptchaSolveProvenance,
    },
    /// An explicit failure, optionally carrying observed provenance.
    Failed {
        /// Failure classification and retained transport facts.
        failure: CaptchaSolveFailure,
        /// Provenance available before or during failure.
        provenance: Option<CaptchaSolveProvenance>,
    },
}

/// Provider adapter contract. Implementations advertise immutable capability
/// facts and receive only normalized, prevalidated requests.
#[async_trait::async_trait]
pub trait CaptchaProvider: Send + Sync {
    /// Advertise immutable provider capabilities.
    fn capabilities(&self) -> &'static CaptchaProviderCapabilities;

    /// Report runtime availability without acquiring credentials or executing.
    fn availability(&self) -> CaptchaProviderAvailability {
        CaptchaProviderAvailability::Available
    }

    /// Return the truthful qualification state for one advertised kind.
    fn qualification_state(
        &self,
        kind: CaptchaChallengeKind,
    ) -> Option<CaptchaCapabilityQualification> {
        self.capabilities()
            .supported_kinds
            .contains(&kind)
            .then_some(CaptchaCapabilityQualification::NotApplicable)
    }

    /// Execute one already-routed normalized request.
    async fn solve(&self, request: &CaptchaSolveRequest) -> CaptchaSolveOutcome;
}

/// Registration failure for a runtime-scoped provider registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptchaProviderRegistrationError {
    /// A provider with the same stable identity is already registered.
    DuplicateProvider(CaptchaProviderId),
}

/// Runtime-scoped provider lookup. Registration order has no routing meaning.
#[derive(Default)]
pub struct CaptchaProviderRegistry<'a> {
    providers: std::collections::HashMap<CaptchaProviderId, &'a dyn CaptchaProvider>,
}

impl<'a> CaptchaProviderRegistry<'a> {
    /// Construct an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one provider, rejecting ambiguous duplicate identities.
    pub fn register(
        &mut self,
        provider: &'a dyn CaptchaProvider,
    ) -> Result<(), CaptchaProviderRegistrationError> {
        let id = provider.capabilities().provider;
        if self.providers.contains_key(&id) {
            return Err(CaptchaProviderRegistrationError::DuplicateProvider(id));
        }
        self.providers.insert(id, provider);
        Ok(())
    }

    /// Resolve exactly the caller-selected provider identity.
    pub fn resolve(&self, id: CaptchaProviderId) -> Option<&'a dyn CaptchaProvider> {
        self.providers.get(&id).copied()
    }

    /// Expose immutable capabilities for one explicitly identified provider.
    pub fn capabilities(
        &self,
        id: CaptchaProviderId,
    ) -> Option<&'static CaptchaProviderCapabilities> {
        self.resolve(id).map(CaptchaProvider::capabilities)
    }

    /// Expose the provider-owned read-only qualification state for one kind.
    pub fn qualification_state(
        &self,
        id: CaptchaProviderId,
        kind: CaptchaChallengeKind,
    ) -> Option<CaptchaCapabilityQualification> {
        self.resolve(id)?.qualification_state(kind)
    }
}

/// One explicit provider attempt and its unmodified canonical outcome.
#[derive(Debug)]
pub struct CaptchaRouteAttempt {
    /// Provider explicitly selected for this attempt.
    pub provider: CaptchaProviderId,
    /// Provider result, including provider/locality/transport provenance.
    pub outcome: CaptchaSolveOutcome,
}

/// Caller-owned attempt ledger. It performs no retry or provider substitution.
#[derive(Debug, Default)]
pub struct CaptchaRouteAttempts {
    attempts: Vec<CaptchaRouteAttempt>,
}

impl CaptchaRouteAttempts {
    /// Construct an empty explicit route history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Read-only diagnostic/provenance access to every recorded attempt, in
    /// order. Never used for retry, substitution, or re-routing — this
    /// binding performs exactly one explicit attempt regardless of what a
    /// caller does with this accessor afterward.
    pub fn recorded(&self) -> &[CaptchaRouteAttempt] {
        &self.attempts
    }

    /// Execute exactly the provider selected by `request` and retain its outcome.
    pub async fn execute_explicit_attempt(
        &mut self,
        registry: &CaptchaProviderRegistry<'_>,
        request: &CaptchaSolveRequest,
    ) -> &CaptchaSolveOutcome {
        let outcome = match registry.resolve(request.selected_provider) {
            None => CaptchaSolveOutcome::Failed {
                failure: CaptchaSolveFailure::ProviderUnavailable,
                provenance: None,
            },
            Some(provider) => match provider.availability() {
                CaptchaProviderAvailability::Available => solve_captcha(provider, request).await,
                CaptchaProviderAvailability::ProviderUnavailable => CaptchaSolveOutcome::Failed {
                    failure: CaptchaSolveFailure::ProviderUnavailable,
                    provenance: unavailable_provenance(provider.capabilities()),
                },
                CaptchaProviderAvailability::CredentialUnavailable => CaptchaSolveOutcome::Failed {
                    failure: CaptchaSolveFailure::CredentialUnavailable,
                    provenance: unavailable_provenance(provider.capabilities()),
                },
            },
        };
        self.attempts.push(CaptchaRouteAttempt {
            provider: request.selected_provider,
            outcome,
        });
        &self
            .attempts
            .last()
            .expect("attempt was just recorded")
            .outcome
    }

    /// Return every explicit attempt in caller-selected order.
    pub fn attempts(&self) -> &[CaptchaRouteAttempt] {
        &self.attempts
    }
}

fn unavailable_provenance(
    capabilities: &CaptchaProviderCapabilities,
) -> Option<CaptchaSolveProvenance> {
    match capabilities.locality {
        CaptchaProviderLocality::Local => {
            Some(CaptchaSolveProvenance::local(capabilities.provider))
        }
        CaptchaProviderLocality::External => Some(CaptchaSolveProvenance {
            provider: capabilities.provider,
            locality: CaptchaProviderLocality::External,
            transport_backend: None,
            response_origin: None,
            local_runtime: None,
        }),
    }
}

fn unsupported() -> CaptchaSolveOutcome {
    CaptchaSolveOutcome::Failed {
        failure: CaptchaSolveFailure::UnsupportedChallenge,
        provenance: None,
    }
}

fn invalid() -> CaptchaSolveOutcome {
    CaptchaSolveOutcome::Failed {
        failure: CaptchaSolveFailure::InvalidChallenge,
        provenance: None,
    }
}

/// Maximum length, in characters, of a canonical `PointSelection` visual
/// grounding label — generous enough for a genuinely descriptive noun
/// phrase ("the small outlined checkbox in the top-left corner", 47
/// chars), tight enough to reject explanatory task prose. This is a
/// rejection threshold, never a truncation point: an overlong label fails
/// closed with `CaptchaSolveFailure::InvalidChallenge` rather than being
/// silently chopped.
pub const CAPTCHA_POINT_SELECTION_LABEL_MAX_CHARS: usize = 80;

/// The canonical, provider-neutral contract for `PointSelection.instruction`:
/// a concise visual grounding target ("what should I locate"), never task
/// orchestration prose ("how browser verification works", "click to
/// complete", vendor/workflow narration). Deterministic and generic — no
/// vendor name or challenge-provider branching, no heuristic rewriting.
/// Enforced once, here, so every provider (present and future) receives an
/// already-conforming label instead of each reimplementing this check or
/// attempting its own prose-to-label summarization.
fn is_canonical_point_selection_label(instruction: &str) -> bool {
    let trimmed = instruction.trim();
    !trimmed.is_empty()
        && trimmed.chars().count() <= CAPTCHA_POINT_SELECTION_LABEL_MAX_CHARS
        // A concise noun-phrase label never needs sentence-terminating
        // punctuation; task-orchestration prose reliably does.
        && !trimmed.contains(['.', '!', '?'])
}

/// Validate advertised capabilities before invoking the explicitly selected
/// provider. This is the only canonical dispatch operation and performs no
/// fallback, racing, retry or provider substitution.
pub async fn solve_captcha(
    provider: &dyn CaptchaProvider,
    request: &CaptchaSolveRequest,
) -> CaptchaSolveOutcome {
    let capabilities = provider.capabilities();
    if request.selected_provider != capabilities.provider
        || !capabilities
            .supported_kinds
            .contains(&request.challenge.kind)
        || request.challenge.visuals.len() > capabilities.maximum_inputs
        || request.challenge.visuals.iter().any(|visual| {
            !capabilities
                .supported_media_types
                .contains(&visual.media_type())
        })
    {
        return unsupported();
    }
    let full_grid_count = request
        .challenge
        .visuals
        .iter()
        .filter(|visual| visual.image_grid().is_some())
        .count();
    if (full_grid_count > 0
        && (request.challenge.kind != CaptchaChallengeKind::ImageGridSelection
            || full_grid_count != 1
            || request.challenge.visuals.len() != 1))
        || request.challenge.visuals.is_empty()
        || request
            .challenge
            .visuals
            .iter()
            .any(|visual| visual.bytes().is_none())
        || (request.challenge.kind == CaptchaChallengeKind::PointSelection
            && !is_canonical_point_selection_label(&request.challenge.instruction))
    {
        return invalid();
    }
    let outcome = provider.solve(request).await;
    match &outcome {
        CaptchaSolveOutcome::Solved { solution, .. }
            if !solution_matches(request.challenge.kind, solution) =>
        {
            CaptchaSolveOutcome::Failed {
                failure: CaptchaSolveFailure::InvalidProviderResponse,
                provenance: None,
            }
        }
        _ => outcome,
    }
}

fn solution_matches(kind: CaptchaChallengeKind, solution: &CaptchaSolution) -> bool {
    match (kind, solution) {
        (CaptchaChallengeKind::ImageGridSelection, CaptchaSolution::SelectedChoices(_)) => true,
        (CaptchaChallengeKind::HorizontalOffset, CaptchaSolution::HorizontalOffset(value)) => {
            value.is_finite()
        }
        (CaptchaChallengeKind::PointSelection, CaptchaSolution::Point { x, y }) => {
            x.is_finite() && y.is_finite()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    static CAPS: CaptchaProviderCapabilities = CaptchaProviderCapabilities {
        provider: CaptchaProviderId::LOCAL_LANGUAGE_MODEL,
        locality: CaptchaProviderLocality::Local,
        supported_kinds: &[CaptchaChallengeKind::HorizontalOffset],
        supported_media_types: &["image/png"],
        maximum_inputs: 1,
        requires_credentials: false,
    };

    struct Provider {
        calls: Arc<AtomicUsize>,
    }

    fn cells(rows: usize, columns: usize) -> Vec<CaptchaImageGridCell> {
        (0..rows)
            .flat_map(|row| {
                (0..columns).map(move |column| {
                    CaptchaImageGridCell::new(
                        format!("choice-{row}-{column}"),
                        row,
                        column,
                        column as u32 * 10,
                        row as u32 * 10,
                        10,
                        10,
                    )
                })
            })
            .collect()
    }

    fn grid(
        rows: usize,
        columns: usize,
        cells: Vec<CaptchaImageGridCell>,
        empty_selection_valid: bool,
    ) -> Result<CaptchaImageGridInput, CaptchaImageGridValidationError> {
        CaptchaImageGridInput::new(
            CaptchaVisualInput::materialized(None, "image/png", [1, 2, 3]),
            (columns as u32 * 10, rows as u32 * 10),
            rows,
            columns,
            cells,
            empty_selection_valid,
        )
    }

    #[async_trait::async_trait]
    impl CaptchaProvider for Provider {
        fn capabilities(&self) -> &'static CaptchaProviderCapabilities {
            &CAPS
        }

        async fn solve(&self, _request: &CaptchaSolveRequest) -> CaptchaSolveOutcome {
            self.calls.fetch_add(1, Ordering::SeqCst);
            CaptchaSolveOutcome::Failed {
                failure: CaptchaSolveFailure::Inconclusive,
                provenance: Some(CaptchaSolveProvenance::local(CAPS.provider)),
            }
        }
    }

    static POINT_CAPS: CaptchaProviderCapabilities = CaptchaProviderCapabilities {
        provider: CaptchaProviderId::LOCAL_LANGUAGE_MODEL,
        locality: CaptchaProviderLocality::Local,
        supported_kinds: &[CaptchaChallengeKind::PointSelection],
        supported_media_types: &["image/png"],
        maximum_inputs: 1,
        requires_credentials: false,
    };

    struct PointProvider {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CaptchaProvider for PointProvider {
        fn capabilities(&self) -> &'static CaptchaProviderCapabilities {
            &POINT_CAPS
        }

        async fn solve(&self, _request: &CaptchaSolveRequest) -> CaptchaSolveOutcome {
            self.calls.fetch_add(1, Ordering::SeqCst);
            CaptchaSolveOutcome::Solved {
                solution: CaptchaSolution::Point { x: 1.0, y: 1.0 },
                provenance: CaptchaSolveProvenance::local(POINT_CAPS.provider),
            }
        }
    }

    fn point_request(instruction: &str) -> CaptchaSolveRequest {
        CaptchaSolveRequest {
            correlation_id: "point-label-contract".into(),
            selected_provider: POINT_CAPS.provider,
            challenge: CaptchaChallenge {
                kind: CaptchaChallengeKind::PointSelection,
                instruction: instruction.into(),
                visuals: vec![CaptchaVisualInput::materialized(
                    None,
                    "image/png",
                    vec![1u8],
                )],
            },
            deadline: Duration::from_secs(1),
        }
    }

    #[tokio::test]
    async fn explanatory_prose_point_selection_label_is_rejected_before_provider_execution() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = PointProvider {
            calls: Arc::clone(&calls),
        };
        let request = point_request(
            "This is a Cloudflare Turnstile \"Verify you are human\" checkbox challenge. \
             Return the point at the center of the unchecked checkbox so a click \
             completes verification.",
        );
        assert!(matches!(
            solve_captcha(&provider, &request).await,
            CaptchaSolveOutcome::Failed {
                failure: CaptchaSolveFailure::InvalidChallenge,
                ..
            }
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn overlong_point_selection_label_is_rejected_before_provider_execution() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = PointProvider {
            calls: Arc::clone(&calls),
        };
        // No sentence punctuation, but exceeds the canonical length bound —
        // rejected on length alone, not just punctuation.
        let request = point_request(&"the outlined checkbox ".repeat(6));
        assert!(matches!(
            solve_captcha(&provider, &request).await,
            CaptchaSolveOutcome::Failed {
                failure: CaptchaSolveFailure::InvalidChallenge,
                ..
            }
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn empty_point_selection_label_is_rejected_before_provider_execution() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = PointProvider {
            calls: Arc::clone(&calls),
        };
        let request = point_request("   ");
        assert!(matches!(
            solve_captcha(&provider, &request).await,
            CaptchaSolveOutcome::Failed {
                failure: CaptchaSolveFailure::InvalidChallenge,
                ..
            }
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn concise_point_selection_labels_are_accepted() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = PointProvider {
            calls: Arc::clone(&calls),
        };
        for label in [
            "the outlined square",
            "the verification checkbox",
            "the outlined checkbox",
            "the slider handle",
            "the requested image tile",
        ] {
            let request = point_request(label);
            assert!(matches!(
                solve_captcha(&provider, &request).await,
                CaptchaSolveOutcome::Solved { .. }
            ));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 5);
    }

    #[test]
    fn registry_rejects_duplicate_provider_ids() {
        let calls = Arc::new(AtomicUsize::new(0));
        let first = Provider {
            calls: Arc::clone(&calls),
        };
        let duplicate = Provider { calls };
        let mut registry = CaptchaProviderRegistry::new();
        assert_eq!(registry.register(&first), Ok(()));
        assert_eq!(
            registry.register(&duplicate),
            Err(CaptchaProviderRegistrationError::DuplicateProvider(
                CAPS.provider
            ))
        );
    }

    #[test]
    fn registry_resolves_only_an_explicit_identity() {
        let provider = Provider {
            calls: Arc::new(AtomicUsize::new(0)),
        };
        let mut registry = CaptchaProviderRegistry::new();
        registry.register(&provider).unwrap();
        assert_eq!(
            registry
                .capabilities(CaptchaProviderId::LOCAL_LANGUAGE_MODEL)
                .map(|capabilities| capabilities.provider),
            Some(CaptchaProviderId::LOCAL_LANGUAGE_MODEL)
        );
        assert!(registry
            .resolve(CaptchaProviderId::EXTERNAL_GEMINI)
            .is_none());
    }

    #[tokio::test]
    async fn explicit_attempt_ledger_preserves_every_outcome() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = Provider {
            calls: Arc::clone(&calls),
        };
        let mut registry = CaptchaProviderRegistry::new();
        registry.register(&provider).unwrap();
        let request = CaptchaSolveRequest {
            correlation_id: "ledger".into(),
            selected_provider: CAPS.provider,
            challenge: CaptchaChallenge {
                kind: CaptchaChallengeKind::HorizontalOffset,
                instruction: String::new(),
                visuals: vec![CaptchaVisualInput::materialized(
                    None,
                    "image/png",
                    Vec::<u8>::new(),
                )],
            },
            deadline: Duration::from_secs(1),
        };
        let mut route = CaptchaRouteAttempts::new();
        route.execute_explicit_attempt(&registry, &request).await;
        route.execute_explicit_attempt(&registry, &request).await;
        assert_eq!(route.attempts().len(), 2);
        assert!(route.attempts().iter().all(|attempt| {
            attempt.provider == CAPS.provider
                && matches!(
                    &attempt.outcome,
                    CaptchaSolveOutcome::Failed {
                        failure: CaptchaSolveFailure::Inconclusive,
                        provenance: Some(CaptchaSolveProvenance {
                            provider: CaptchaProviderId::LOCAL_LANGUAGE_MODEL,
                            locality: CaptchaProviderLocality::Local,
                            ..
                        })
                    }
                )
        }));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn unsupported_kind_is_rejected_before_provider_execution() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = Provider {
            calls: Arc::clone(&calls),
        };
        let request = CaptchaSolveRequest {
            correlation_id: "test".into(),
            selected_provider: CAPS.provider,
            challenge: CaptchaChallenge {
                kind: CaptchaChallengeKind::PointSelection,
                instruction: String::new(),
                visuals: vec![CaptchaVisualInput::materialized(
                    None,
                    "image/png",
                    Vec::<u8>::new(),
                )],
            },
            deadline: Duration::from_secs(1),
        };
        assert!(matches!(
            solve_captcha(&provider, &request).await,
            CaptchaSolveOutcome::Failed {
                failure: CaptchaSolveFailure::UnsupportedChallenge,
                ..
            }
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn remote_asset_is_rejected_before_provider_execution() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = Provider {
            calls: Arc::clone(&calls),
        };
        let request = CaptchaSolveRequest {
            correlation_id: "remote".into(),
            selected_provider: CAPS.provider,
            challenge: CaptchaChallenge {
                kind: CaptchaChallengeKind::HorizontalOffset,
                instruction: String::new(),
                visuals: vec![CaptchaVisualInput::RemoteAsset {
                    id: None,
                    media_type: "image/png".into(),
                    url: url::Url::parse("https://example.invalid/challenge.png").unwrap(),
                }],
            },
            deadline: Duration::from_secs(1),
        };
        assert!(matches!(
            solve_captcha(&provider, &request).await,
            CaptchaSolveOutcome::Failed {
                failure: CaptchaSolveFailure::InvalidChallenge,
                ..
            }
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn zero_coordinates_and_empty_selections_are_valid_solution_values() {
        assert!(solution_matches(
            CaptchaChallengeKind::ImageGridSelection,
            &CaptchaSolution::SelectedChoices(Vec::new())
        ));
        assert!(solution_matches(
            CaptchaChallengeKind::HorizontalOffset,
            &CaptchaSolution::HorizontalOffset(0.0)
        ));
        assert!(solution_matches(
            CaptchaChallengeKind::PointSelection,
            &CaptchaSolution::Point { x: 0.0, y: 0.0 }
        ));
    }

    #[test]
    fn valid_three_by_three_full_grid_preserves_explicit_row_major_mapping() {
        let mut supplied = cells(3, 3);
        supplied.reverse();
        let grid = grid(3, 3, supplied, true).unwrap();
        assert_eq!(grid.original_dimensions(), (30, 30));
        assert_eq!(grid.layout(), (3, 3));
        assert!(grid.empty_selection_valid());
        let ids: Vec<_> = grid.cells().iter().map(|cell| cell.choice_id()).collect();
        assert_eq!(ids.first(), Some(&"choice-0-0"));
        assert_eq!(ids.last(), Some(&"choice-2-2"));
        assert!(grid
            .cells()
            .iter()
            .enumerate()
            .all(|(index, cell)| (cell.row(), cell.column()) == (index / 3, index % 3)));
    }

    #[test]
    fn valid_four_by_four_full_grid_is_one_explicit_visual_form() {
        let grid = grid(4, 4, cells(4, 4), false).unwrap();
        let visual = CaptchaVisualInput::materialized_full_grid(grid);
        assert_eq!(visual.bytes(), Some([1, 2, 3].as_slice()));
        assert_eq!(visual.media_type(), "image/png");
        assert_eq!(visual.image_grid().unwrap().cells().len(), 16);
        assert!(!visual.image_grid().unwrap().empty_selection_valid());
    }

    #[test]
    fn duplicate_choice_identity_is_rejected() {
        let mut values = cells(3, 3);
        values[1].choice_id = values[0].choice_id.clone();
        assert_eq!(
            grid(3, 3, values, false).unwrap_err(),
            CaptchaImageGridValidationError::InvalidChoiceIdentity
        );
    }

    #[test]
    fn missing_and_extra_cells_are_rejected() {
        let mut missing = cells(3, 3);
        missing.pop();
        assert_eq!(
            grid(3, 3, missing, false).unwrap_err(),
            CaptchaImageGridValidationError::ChoiceCountMismatch
        );
        let mut extra = cells(3, 3);
        extra.push(CaptchaImageGridCell::new("extra", 3, 0, 0, 0, 1, 1));
        assert_eq!(
            grid(3, 3, extra, false).unwrap_err(),
            CaptchaImageGridValidationError::ChoiceCountMismatch
        );
    }

    #[test]
    fn duplicate_or_out_of_range_grid_positions_are_rejected() {
        let mut duplicate = cells(3, 3);
        duplicate[1].column = duplicate[0].column;
        assert_eq!(
            grid(3, 3, duplicate, false).unwrap_err(),
            CaptchaImageGridValidationError::InvalidGridPosition
        );
        let mut outside = cells(3, 3);
        outside[8].row = 3;
        assert_eq!(
            grid(3, 3, outside, false).unwrap_err(),
            CaptchaImageGridValidationError::InvalidGridPosition
        );
    }

    #[test]
    fn invalid_layout_and_original_dimensions_are_rejected() {
        assert_eq!(
            grid(0, 3, Vec::new(), false).unwrap_err(),
            CaptchaImageGridValidationError::InvalidDimensions
        );
        assert_eq!(
            CaptchaImageGridInput::new(
                CaptchaVisualInput::materialized(None, "image/png", [1]),
                (0, 30),
                3,
                3,
                cells(3, 3),
                false,
            )
            .unwrap_err(),
            CaptchaImageGridValidationError::InvalidDimensions
        );
    }

    #[test]
    fn cells_must_have_positive_area_inside_original_bounds() {
        let mut zero = cells(3, 3);
        zero[0].width = 0;
        assert_eq!(
            grid(3, 3, zero, false).unwrap_err(),
            CaptchaImageGridValidationError::CellOutsideImage
        );
        let mut outside = cells(3, 3);
        outside[8].width = 11;
        assert_eq!(
            grid(3, 3, outside, false).unwrap_err(),
            CaptchaImageGridValidationError::CellOutsideImage
        );
    }

    #[test]
    fn overlapping_geometry_is_rejected() {
        let mut values = cells(3, 3);
        values[1].x = 9;
        assert_eq!(
            grid(3, 3, values, false).unwrap_err(),
            CaptchaImageGridValidationError::AmbiguousCellOverlap
        );
    }

    #[test]
    fn full_grid_requires_materialized_bytes() {
        let remote = CaptchaVisualInput::RemoteAsset {
            id: None,
            media_type: "image/png".into(),
            url: url::Url::parse("https://example.invalid/grid.png").unwrap(),
        };
        assert_eq!(
            CaptchaImageGridInput::new(remote, (30, 30), 3, 3, cells(3, 3), false).unwrap_err(),
            CaptchaImageGridValidationError::FullGridNotMaterialized
        );
    }

    // --- SCORPION_CANONICAL_CAPTCHA_PROVIDER_ROUTING_BINDING_001: deterministic
    // routing-machinery proof. These prove `CaptchaProviderRegistry` /
    // `CaptchaRouteAttempts::execute_explicit_attempt` / `solve_captcha`
    // routing semantics with deterministic test-double providers registered
    // through the real canonical registry — never real model inference.

    static SECOND_CAPS: CaptchaProviderCapabilities = CaptchaProviderCapabilities {
        provider: CaptchaProviderId::EXTERNAL_GEMINI,
        locality: CaptchaProviderLocality::External,
        supported_kinds: &[CaptchaChallengeKind::HorizontalOffset],
        supported_media_types: &["image/png"],
        maximum_inputs: 1,
        requires_credentials: false,
    };

    struct SecondProvider {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CaptchaProvider for SecondProvider {
        fn capabilities(&self) -> &'static CaptchaProviderCapabilities {
            &SECOND_CAPS
        }

        async fn solve(&self, _request: &CaptchaSolveRequest) -> CaptchaSolveOutcome {
            self.calls.fetch_add(1, Ordering::SeqCst);
            CaptchaSolveOutcome::Solved {
                solution: CaptchaSolution::HorizontalOffset(4.0),
                provenance: CaptchaSolveProvenance::local(SECOND_CAPS.provider),
            }
        }
    }

    fn offset_request(selected_provider: CaptchaProviderId) -> CaptchaSolveRequest {
        CaptchaSolveRequest {
            correlation_id: "routing-matrix".into(),
            selected_provider,
            challenge: CaptchaChallenge {
                kind: CaptchaChallengeKind::HorizontalOffset,
                instruction: String::new(),
                visuals: vec![CaptchaVisualInput::materialized(
                    None,
                    "image/png",
                    Vec::<u8>::new(),
                )],
            },
            deadline: Duration::from_secs(1),
        }
    }

    /// ROUTING PROOF — NOT REAL INFERENCE. Case B: two providers registered,
    /// configuration selects one; only the selected provider executes.
    #[tokio::test]
    async fn two_registered_providers_only_the_selected_one_executes() {
        let calls_a = Arc::new(AtomicUsize::new(0));
        let calls_b = Arc::new(AtomicUsize::new(0));
        let provider_a = Provider {
            calls: Arc::clone(&calls_a),
        };
        let provider_b = SecondProvider {
            calls: Arc::clone(&calls_b),
        };
        let mut registry = CaptchaProviderRegistry::new();
        registry.register(&provider_a).unwrap();
        registry.register(&provider_b).unwrap();

        let request = offset_request(CaptchaProviderId::EXTERNAL_GEMINI);
        let mut route = CaptchaRouteAttempts::new();
        let outcome = route.execute_explicit_attempt(&registry, &request).await;

        assert!(matches!(
            outcome,
            CaptchaSolveOutcome::Solved {
                solution: CaptchaSolution::HorizontalOffset(offset),
                ..
            } if *offset == 4.0
        ));
        assert_eq!(
            calls_a.load(Ordering::SeqCst),
            0,
            "provider A must never execute"
        );
        assert_eq!(
            calls_b.load(Ordering::SeqCst),
            1,
            "provider B must execute exactly once"
        );
    }

    /// ROUTING PROOF — NOT REAL INFERENCE. Case C: the configured provider
    /// identity is not registered at all. Typed `ProviderUnavailable`, never
    /// a panic or a silently-substituted provider.
    #[tokio::test]
    async fn unregistered_provider_id_is_typed_provider_unavailable() {
        let registry = CaptchaProviderRegistry::new();
        let request = offset_request(CaptchaProviderId::PALIGEMMA_LOCAL);
        let mut route = CaptchaRouteAttempts::new();
        let outcome = route.execute_explicit_attempt(&registry, &request).await;
        assert!(matches!(
            outcome,
            CaptchaSolveOutcome::Failed {
                failure: CaptchaSolveFailure::ProviderUnavailable,
                provenance: None,
            }
        ));
    }

    /// ROUTING PROOF — NOT REAL INFERENCE. Case F: the provider returns a
    /// structurally valid-looking `CaptchaSolveOutcome::Solved`, but the
    /// solution variant does not match the requested challenge kind.
    /// `solve_captcha` must fail closed, never pass through a mismatched
    /// solution.
    #[tokio::test]
    async fn mismatched_solution_kind_fails_closed() {
        struct WrongKindProvider;
        #[async_trait::async_trait]
        impl CaptchaProvider for WrongKindProvider {
            fn capabilities(&self) -> &'static CaptchaProviderCapabilities {
                &CAPS
            }
            async fn solve(&self, _request: &CaptchaSolveRequest) -> CaptchaSolveOutcome {
                // CAPS advertises HorizontalOffset; return a Point instead.
                CaptchaSolveOutcome::Solved {
                    solution: CaptchaSolution::Point { x: 1.0, y: 1.0 },
                    provenance: CaptchaSolveProvenance::local(CAPS.provider),
                }
            }
        }
        let provider = WrongKindProvider;
        let request = offset_request(CAPS.provider);
        assert!(matches!(
            solve_captcha(&provider, &request).await,
            CaptchaSolveOutcome::Failed {
                failure: CaptchaSolveFailure::InvalidProviderResponse,
                ..
            }
        ));
    }

    /// ROUTING PROOF — NOT REAL INFERENCE. Case F (continued): a
    /// non-finite coordinate is not a valid solution value either.
    #[tokio::test]
    async fn non_finite_solution_value_fails_closed() {
        struct NonFiniteProvider;
        #[async_trait::async_trait]
        impl CaptchaProvider for NonFiniteProvider {
            fn capabilities(&self) -> &'static CaptchaProviderCapabilities {
                &CAPS
            }
            async fn solve(&self, _request: &CaptchaSolveRequest) -> CaptchaSolveOutcome {
                CaptchaSolveOutcome::Solved {
                    solution: CaptchaSolution::HorizontalOffset(f64::NAN),
                    provenance: CaptchaSolveProvenance::local(CAPS.provider),
                }
            }
        }
        let provider = NonFiniteProvider;
        let request = offset_request(CAPS.provider);
        assert!(matches!(
            solve_captcha(&provider, &request).await,
            CaptchaSolveOutcome::Failed {
                failure: CaptchaSolveFailure::InvalidProviderResponse,
                ..
            }
        ));
    }

    /// ROUTING PROOF — NOT REAL INFERENCE. `CaptchaProviderId` round-trips
    /// through its `serde` representation, and an unknown label is rejected
    /// rather than fabricated into some default provider — this is the same
    /// validation `Configuration::captcha_provider` relies on when read
    /// from a serialized config.
    #[cfg(feature = "serde")]
    #[test]
    fn provider_id_serde_round_trips_and_rejects_unknown_labels() {
        let json = serde_json::to_string(&CaptchaProviderId::LOCAL_LANGUAGE_MODEL).unwrap();
        assert_eq!(json, "\"local-language-model\"");
        let parsed: CaptchaProviderId = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, CaptchaProviderId::LOCAL_LANGUAGE_MODEL);

        let unknown: Result<CaptchaProviderId, _> = serde_json::from_str("\"not-a-real-provider\"");
        assert!(unknown.is_err());
    }
}
