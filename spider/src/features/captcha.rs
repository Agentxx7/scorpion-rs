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

    /// Return the declared media type.
    pub fn media_type(&self) -> &str {
        match self {
            Self::Materialized { media_type, .. } | Self::RemoteAsset { media_type, .. } => {
                media_type
            }
        }
    }

    /// Return the optional choice identity.
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::Materialized { id, .. } | Self::RemoteAsset { id, .. } => id.as_deref(),
        }
    }

    /// Return materialized bytes, or `None` for an acquisition plan.
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Materialized { bytes, .. } => Some(bytes),
            Self::RemoteAsset { .. } => None,
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

    /// Return the stable provider label.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
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
}

impl CaptchaSolveProvenance {
    /// Construct local provenance without transport claims.
    pub fn local(provider: CaptchaProviderId) -> Self {
        Self {
            provider,
            locality: CaptchaProviderLocality::Local,
            transport_backend: None,
            response_origin: None,
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

    /// Execute one already-routed normalized request.
    async fn solve(&self, request: &CaptchaSolveRequest) -> CaptchaSolveOutcome;
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
    if request.challenge.visuals.is_empty()
        || request
            .challenge
            .visuals
            .iter()
            .any(|visual| visual.bytes().is_none())
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static CAPS: CaptchaProviderCapabilities = CaptchaProviderCapabilities {
        provider: CaptchaProviderId::LOCAL_LANGUAGE_MODEL,
        locality: CaptchaProviderLocality::Local,
        supported_kinds: &[CaptchaChallengeKind::HorizontalOffset],
        supported_media_types: &["image/png"],
        maximum_inputs: 1,
        requires_credentials: false,
    };

    struct Provider;

    #[async_trait::async_trait]
    impl CaptchaProvider for Provider {
        fn capabilities(&self) -> &'static CaptchaProviderCapabilities {
            &CAPS
        }

        async fn solve(&self, _request: &CaptchaSolveRequest) -> CaptchaSolveOutcome {
            CALLS.fetch_add(1, Ordering::SeqCst);
            CaptchaSolveOutcome::Failed {
                failure: CaptchaSolveFailure::Inconclusive,
                provenance: Some(CaptchaSolveProvenance::local(CAPS.provider)),
            }
        }
    }

    #[tokio::test]
    async fn unsupported_kind_is_rejected_before_provider_execution() {
        CALLS.store(0, Ordering::SeqCst);
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
            solve_captcha(&Provider, &request).await,
            CaptchaSolveOutcome::Failed {
                failure: CaptchaSolveFailure::UnsupportedChallenge,
                ..
            }
        ));
        assert_eq!(CALLS.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn remote_asset_is_rejected_before_provider_execution() {
        CALLS.store(0, Ordering::SeqCst);
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
            solve_captcha(&Provider, &request).await,
            CaptchaSolveOutcome::Failed {
                failure: CaptchaSolveFailure::InvalidChallenge,
                ..
            }
        ));
        assert_eq!(CALLS.load(Ordering::SeqCst), 0);
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
}
