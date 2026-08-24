use aho_corasick::{AhoCorasick, AhoCorasickBuilder};

#[cfg(all(feature = "chrome", feature = "real_browser"))]
use chromiumoxide::{
    cdp::js_protocol::runtime::{CallFunctionOnParams, EvaluateParams},
    error::CdpError,
    layout::Point,
    Page,
};
#[cfg(feature = "chrome")]
use std::time::Duration;

#[cfg(any(feature = "openai", all(feature = "chrome", feature = "real_browser")))]
use crate::features::captcha::{
    solve_captcha, CaptchaChallengeKind, CaptchaProvider, CaptchaProviderAvailability,
    CaptchaProviderCapabilities, CaptchaProviderLocality, CaptchaSolution, CaptchaSolveProvenance,
    CaptchaVisualInput,
};
// The minimal routing vocabulary `route_detected_browser_challenge` itself
// needs, unconditionally under plain `chrome` — it always builds a
// (possibly empty) registry and always makes exactly one explicit attempt,
// regardless of which specific providers this build can construct.
#[cfg(feature = "chrome")]
use crate::features::captcha::{
    CaptchaBrowserActionOutcome, CaptchaChallenge, CaptchaProviderId, CaptchaProviderRegistry,
    CaptchaRouteAttempts, CaptchaRouteOutcomeSummary, CaptchaSolveFailure, CaptchaSolveOutcome,
    CaptchaSolveRequest,
};
// The canonical, provider-neutral, pre-proven solution -> browser-action
// binding seam (`SCORPION_CANONICAL_CAPTCHA_BROWSER_EXECUTION_BINDING_001`)
// this router now composes, never reimplements, when a live snapshot is
// available — see `route_detected_browser_challenge`'s own doc comment.
#[cfg(feature = "chrome")]
use crate::features::captcha_browser::{
    execute_browser_captcha_attempt, CaptchaBrowserAttempt, CaptchaBrowserChallenge,
    CaptchaBrowserExecutionFailure, CaptchaBrowserExecutionFailureKind,
};
#[cfg(all(feature = "chrome", feature = "real_browser"))]
use crate::utils::{page_wait, perform_smart_mouse_movement, CF_WAIT_FOR};
#[cfg(any(feature = "openai", all(feature = "chrome", feature = "real_browser")))]
use base64::prelude::*;
#[cfg(any(
    feature = "openai",
    feature = "gemini",
    all(feature = "chrome", feature = "real_browser")
))]
use spider_transport::{
    CanonicalExecutor, CrawlerBodyStream, CrawlerFailure, CrawlerFailureKind, CrawlerRequest,
    SecretRequestHeaders,
};
#[cfg(any(
    feature = "openai",
    feature = "gemini",
    all(feature = "chrome", feature = "real_browser")
))]
use tokio_stream::StreamExt;

#[cfg(feature = "openai")]
static OPENAI_VISION_CAPABILITIES: CaptchaProviderCapabilities = CaptchaProviderCapabilities {
    provider: CaptchaProviderId::OPENAI_VISION,
    locality: CaptchaProviderLocality::External,
    supported_kinds: &[
        CaptchaChallengeKind::ImageGridSelection,
        CaptchaChallengeKind::HorizontalOffset,
        CaptchaChallengeKind::PointSelection,
    ],
    supported_media_types: &["image/jpeg", "image/png"],
    maximum_inputs: 16,
    requires_credentials: true,
};

#[cfg(all(feature = "chrome", feature = "real_browser"))]
static LOCAL_LANGUAGE_MODEL_CAPABILITIES: CaptchaProviderCapabilities =
    CaptchaProviderCapabilities {
        provider: CaptchaProviderId::LOCAL_LANGUAGE_MODEL,
        locality: CaptchaProviderLocality::Local,
        supported_kinds: &[
            CaptchaChallengeKind::ImageGridSelection,
            CaptchaChallengeKind::HorizontalOffset,
            CaptchaChallengeKind::PointSelection,
        ],
        supported_media_types: &["image/jpeg", "image/png"],
        maximum_inputs: 64,
        requires_credentials: false,
    };

#[cfg(all(feature = "chrome", feature = "real_browser"))]
static EXTERNAL_GEMINI_CAPABILITIES: CaptchaProviderCapabilities = CaptchaProviderCapabilities {
    provider: CaptchaProviderId::EXTERNAL_GEMINI,
    locality: CaptchaProviderLocality::External,
    supported_kinds: &[
        CaptchaChallengeKind::ImageGridSelection,
        CaptchaChallengeKind::HorizontalOffset,
        CaptchaChallengeKind::PointSelection,
    ],
    supported_media_types: &["image/jpeg", "image/png"],
    maximum_inputs: 64,
    requires_credentials: true,
};

static VERIFY_PATTERNS: &[&[u8]] = &[
    b"verifying you are human",
    b"review the security of your connection",
    b"please verify you are a human",
    b"checking your browser before accessing",
    b"prove you are human",
    b"checking if the site connection is secure",
];

/// Imperva iframe patterns.
static IMPERVA_IFRAME_PATTERNS: &[&[u8]] = &[
    b"geo.captcha-delivery.com",
    b"captcha-delivery.com",
    b"Verification system",
    b"Verification Required",
    b"Verification successful",
    b"Verifying device",
];

/// Recaptcha iframe patterns.
static RECAPTCHA_PATTERNS: &[&[u8]] = &[
    b"https://www.google.com/recaptcha/",
    b"/recaptcha/",
    b"recaptcha/api2/anchor",
    b"recaptcha/enterprise/bframe",
];

/// Geetest patterns.
static GEETEST_PATTERNS: &[&[u8]] = &[
    b"id=\"embed-captcha\"",
    b"class=\"gee-test",
    b"class=\"gee-test__placeholder",
];

/// Geetest loading patterns.
static GEETEST_LOADING_PATTERNS: &[&[u8]] = &[b"Loading GeeTest", b"geetest_wait", b"geetest_init"];

/// Geetest visible patterns.
static GEETEST_VISIBLE_PATTERNS: &[&[u8]] = &[
    b"geetest_widget",
    b"geetest_slider_button",
    b"geetest_canvas",
    b"geetest_canvas_slice",
];

/// Imperva wait patterns.
static IMPERVA_WAIT_PATTERNS: &[&[u8]] = &[
    b"Verifying the device",
    b"Verifying the device...",
    b"The requested content will be available after verification",
    b"available after verification",
];

/// Imperva iframe phase patterns.
static IMPERVA_IFRAME_PHASE_PATTERNS: &[&[u8]] = &[
    b"geo.captcha-delivery.com",
    b"captcha-delivery.com",
    b"Verification system",
];

/// Hcaptcha wait patterns.
#[cfg(all(feature = "chrome", feature = "real_browser"))]
static HCAPTCHA_IFRAME_PATTERNS: &[&[u8]] = &[
    b"newassets.hcaptcha.com",
    b"hcaptcha.com/captcha",
    b"Widget containing checkbox for hCaptcha",
    b"data-hcaptcha-widget-id",
];

/// RC enterprise guards.
#[cfg(all(feature = "chrome", feature = "real_browser"))]
static RC_ENTERPRISE_GUARD_PATTERNS: &[&[u8]] = &[
    b"__recaptcha_api",
    b"/recaptcha/enterprise/",
    b"rc-imageselect",
    b"rc-imageselect-tile",
];

static LEMIN_PATTERNS: &[&[u8]] = &[b"id=\"lemin-cropped-captcha\""];

/// RC verify btn patterns.
#[cfg(all(feature = "chrome", feature = "real_browser"))]
static RC_VERIFY_BUTTON_PATTERNS: &[&[u8]] = &[b"id=\"recaptcha-verify-button\"", b">Verify<"];

/// RC tile patterns.
#[cfg(all(feature = "chrome", feature = "real_browser"))]
static RC_TILE_CLASS_PATTERNS: &[&[u8]] = &[b"rc-imageselect-tile"];

#[cfg(all(feature = "chrome", feature = "real_browser"))]
lazy_static! {
    /// hCaptcha‑iframe matcher (used inside Imperva flow)
    static ref HCAPTCHA_IFRAME_AC: AhoCorasick = AhoCorasickBuilder::new()
        .ascii_case_insensitive(true)
        .match_kind(aho_corasick::MatchKind::LeftmostFirst)
        .build(HCAPTCHA_IFRAME_PATTERNS)
        .expect("valid hCaptcha iframe patterns");

            static ref RC_ENTERPRISE_GUARD_AC: AhoCorasick = AhoCorasickBuilder::new()
        .match_kind(aho_corasick::MatchKind::LeftmostFirst)
        .ascii_case_insensitive(false)
        .build(RC_ENTERPRISE_GUARD_PATTERNS)
        .expect("valid enterprise‑recaptcha guard patterns");

    // Verify‑button detection (either the hidden button id or the visible “Verify” text).
    static ref RC_VERIFY_BUTTON_AC: AhoCorasick = AhoCorasickBuilder::new()
        .match_kind(aho_corasick::MatchKind::LeftmostFirst)
        .ascii_case_insensitive(false)
        .build(RC_VERIFY_BUTTON_PATTERNS)
        .expect("valid verify‑button patterns");

    // Tile‑class matcher – used to locate every tile in the HTML.
    static ref RC_TILE_CLASS_AC: AhoCorasick = AhoCorasickBuilder::new()
        .match_kind(aho_corasick::MatchKind::LeftmostFirst)
        .ascii_case_insensitive(false)
        .build(RC_TILE_CLASS_PATTERNS)
        .expect("valid tile‑class pattern");
}

#[cfg(all(test, feature = "gemini"))]
mod gemini_transport_tests {
    use super::*;

    #[test]
    fn provider_key_is_secret_header_not_url_material() {
        const SENTINEL: &str = "solver-secret-sentinel";
        let request = gemini_post_request(
            "https://generativelanguage.googleapis.com/v1beta/models/test:generateContent",
            SENTINEL,
            b"{}".to_vec(),
        )
        .unwrap();
        assert!(request.url.query().is_none());
        assert_eq!(request.secret_headers.len(), 1);
        let debug = format!("{:?}", request.secret_headers);
        assert!(!debug.contains(SENTINEL));
    }

    #[test]
    fn configured_endpoint_cannot_smuggle_key_query_parameter() {
        let endpoint = format!(
            "https://generativelanguage.googleapis.com/model?{}=forbidden",
            "key"
        );
        let result = gemini_post_request(&endpoint, "header-secret", b"{}".to_vec());
        assert!(result.is_err());
    }
}

#[cfg(all(test, feature = "openai"))]
mod openai_vision_provider_tests {
    use super::*;

    fn request(kind: CaptchaChallengeKind, ids: &[&str]) -> CaptchaSolveRequest {
        CaptchaSolveRequest {
            correlation_id: "openai-test".into(),
            selected_provider: CaptchaProviderId::OPENAI_VISION,
            challenge: CaptchaChallenge {
                kind,
                instruction: "test".into(),
                visuals: ids
                    .iter()
                    .map(|id| {
                        CaptchaVisualInput::materialized(
                            Some((*id).into()),
                            "image/png",
                            vec![1_u8],
                        )
                    })
                    .collect(),
            },
            deadline: Duration::from_secs(1),
        }
    }

    #[test]
    fn credential_is_secret_header_and_debug_is_redacted() {
        const SECRET: &str = "openai-provider-secret-sentinel";
        let crawler_request = openai_request(SECRET, b"{}".to_vec()).unwrap();
        assert!(crawler_request.url.query().is_none());
        assert_eq!(crawler_request.secret_headers.len(), 1);
        assert!(!format!("{:?}", crawler_request.secret_headers).contains(SECRET));
        let provider = OpenAiVisionCaptchaProvider::new("vision-model", SECRET);
        assert!(!format!("{provider:?}").contains(SECRET));
    }

    #[test]
    fn payload_contains_explicit_model_and_images_but_no_credential() {
        const SECRET: &str = "must-not-enter-payload";
        let payload = openai_payload(
            &request(CaptchaChallengeKind::PointSelection, &["visual-a"]),
            "explicit-vision-model",
        )
        .unwrap();
        let encoded = serde_json::to_string(&payload).unwrap();
        assert!(encoded.contains("explicit-vision-model"));
        assert!(encoded.contains("data:image/png;base64,"));
        assert!(encoded.contains("visual-a"));
        assert!(!encoded.contains(SECRET));
        assert!(!encoded.contains("api_key"));
        assert!(!encoded.contains("authorization"));
    }

    #[test]
    fn responses_output_text_is_extracted_only_from_output_text_content() {
        let response = serde_json::json!({
            "output": [{
                "content": [
                    {"type": "refusal", "text": "ignored"},
                    {"type": "output_text", "text": "{\"x\":1}"}
                ]
            }]
        });
        assert_eq!(openai_output_text(&response), Some("{\"x\":1}"));
        assert_eq!(openai_output_text(&serde_json::json!({"output": []})), None);
    }

    #[test]
    fn explicit_model_and_credential_availability_are_truthful() {
        let available = OpenAiVisionCaptchaProvider::new("vision-model", "secret");
        assert_eq!(available.model(), "vision-model");
        assert_eq!(
            available.availability(),
            CaptchaProviderAvailability::Available
        );
        assert_eq!(
            OpenAiVisionCaptchaProvider::new("vision-model", "").availability(),
            CaptchaProviderAvailability::CredentialUnavailable
        );
        assert_eq!(
            OpenAiVisionCaptchaProvider::new("", "secret").availability(),
            CaptchaProviderAvailability::ProviderUnavailable
        );
    }

    #[test]
    fn strict_response_parser_accepts_only_kind_specific_shapes() {
        assert!(matches!(
            openai_prompt_and_solution(
                &request(CaptchaChallengeKind::ImageGridSelection, &["a", "b"]),
                r#"{"selected_ids":["b"]}"#,
            ),
            Ok(CaptchaSolution::SelectedChoices(ids)) if ids == ["b"]
        ));
        assert!(matches!(
            openai_prompt_and_solution(
                &request(CaptchaChallengeKind::HorizontalOffset, &["a"]),
                r#"{"x":12.5}"#,
            ),
            Ok(CaptchaSolution::HorizontalOffset(12.5))
        ));
        assert!(matches!(
            openai_prompt_and_solution(
                &request(CaptchaChallengeKind::PointSelection, &["a"]),
                r#"{"x":1.0,"y":2.0}"#,
            ),
            Ok(CaptchaSolution::Point { x: 1.0, y: 2.0 })
        ));
    }

    #[test]
    fn strict_response_parser_rejects_unknown_duplicate_and_wrong_shapes() {
        let grid = request(CaptchaChallengeKind::ImageGridSelection, &["a", "b"]);
        for invalid in [
            r#"{"selected_ids":["missing"]}"#,
            r#"{"selected_ids":["a","a"]}"#,
            r#"{"selected_ids":[],"extra":true}"#,
            r#"{"x":1}"#,
        ] {
            assert!(matches!(
                openai_prompt_and_solution(&grid, invalid),
                Err(CaptchaSolveFailure::InvalidProviderResponse)
            ));
        }
    }
}

#[cfg(any(feature = "gemini", all(feature = "chrome", feature = "real_browser")))]
lazy_static! {
    /// One persistent, feature-selected canonical executor for all external
    /// Gemini solver traffic. Backend clients and session state never leave it.
    static ref GEMINI_EXECUTOR: CanonicalExecutor = resolve_gemini_executor();
}

#[cfg(all(
    not(feature = "wreq"),
    any(feature = "gemini", all(feature = "chrome", feature = "real_browser"))
))]
fn resolve_gemini_executor() -> CanonicalExecutor {
    let mut config = spider_transport::CrawlerTransportConfiguration::default();
    config.user_agent = "spider-gemini-solver".into();
    config.request_timeout = Duration::from_secs(20);
    CanonicalExecutor::resolve(config).expect("failed to resolve canonical Gemini executor")
}

#[cfg(all(
    feature = "wreq",
    any(feature = "gemini", all(feature = "chrome", feature = "real_browser"))
))]
fn resolve_gemini_executor() -> CanonicalExecutor {
    CanonicalExecutor::resolve(spider_transport::WreqTransportConfiguration {
        policy: spider_transport::TransportPolicy::Default,
        user_agent: "spider-gemini-solver".into(),
        default_headers: reqwest::header::HeaderMap::new(),
        proxies: Vec::new(),
        request_timeout: Duration::from_secs(20),
        connect_timeout: Duration::from_secs(20),
        read_timeout: Duration::from_secs(20),
        accept_invalid_certs: false,
        local_address: None,
        network_interface: None,
        dns_resolver: None,
        cookie_jar: None,
        emulation: None,
        redirect_limit: 10,
        redirect_mode: spider_transport::WreqRedirectMode::Follow,
    })
    .expect("failed to resolve canonical Gemini executor")
}

#[cfg(any(
    feature = "openai",
    feature = "gemini",
    all(feature = "chrome", feature = "real_browser")
))]
async fn collect_captcha_body(mut body: CrawlerBodyStream) -> Result<Vec<u8>, CrawlerFailure> {
    let mut bytes = Vec::new();
    while let Some(chunk) = body.next().await {
        bytes.extend_from_slice(&chunk?);
    }
    Ok(bytes)
}

#[cfg(all(feature = "chrome", feature = "real_browser"))]
fn gemini_post_request(
    endpoint: &str,
    api_key: &str,
    body: Vec<u8>,
) -> Result<CrawlerRequest, Box<dyn std::error::Error>> {
    let url = url::Url::parse(endpoint)?;
    if url
        .query_pairs()
        .any(|(name, _)| name.eq_ignore_ascii_case("key"))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Gemini endpoint must not contain an API key query parameter",
        )
        .into());
    }
    let mut secret_headers = SecretRequestHeaders::new();
    secret_headers.try_insert("x-goog-api-key", api_key)?;
    Ok(CrawlerRequest {
        url,
        method: reqwest::Method::POST,
        headers: reqwest::header::HeaderMap::new(),
        secret_headers,
        body: Some(body),
        content_type: Some("application/json".into()),
    })
}

#[cfg(any(feature = "openai", all(feature = "chrome", feature = "real_browser")))]
fn visual_as_data_url(visual: &CaptchaVisualInput) -> Result<String, CaptchaSolveFailure> {
    let bytes = visual
        .bytes()
        .ok_or(CaptchaSolveFailure::InvalidChallenge)?;
    Ok(format!(
        "data:{};base64,{}",
        visual.media_type(),
        BASE64_STANDARD.encode(bytes)
    ))
}

#[cfg(feature = "openai")]
const OPENAI_RESPONSES_ENDPOINT: &str = "https://api.openai.com/v1/responses";

#[cfg(feature = "openai")]
lazy_static! {
    /// Persistent canonical executor for OpenAI vision provider traffic.
    static ref OPENAI_VISION_EXECUTOR: CanonicalExecutor = resolve_openai_vision_executor();
}

#[cfg(all(feature = "openai", not(feature = "wreq")))]
fn resolve_openai_vision_executor() -> CanonicalExecutor {
    let mut config = spider_transport::CrawlerTransportConfiguration::default();
    config.user_agent = "spider-openai-vision-captcha".into();
    config.request_timeout = Duration::from_secs(30);
    CanonicalExecutor::resolve(config).expect("failed to resolve canonical OpenAI executor")
}

#[cfg(all(feature = "openai", feature = "wreq"))]
fn resolve_openai_vision_executor() -> CanonicalExecutor {
    CanonicalExecutor::resolve(spider_transport::WreqTransportConfiguration {
        policy: spider_transport::TransportPolicy::Default,
        user_agent: "spider-openai-vision-captcha".into(),
        default_headers: reqwest::header::HeaderMap::new(),
        proxies: Vec::new(),
        request_timeout: Duration::from_secs(30),
        connect_timeout: Duration::from_secs(20),
        read_timeout: Duration::from_secs(30),
        accept_invalid_certs: false,
        local_address: None,
        network_interface: None,
        dns_resolver: None,
        cookie_jar: None,
        emulation: None,
        redirect_limit: 10,
        redirect_mode: spider_transport::WreqRedirectMode::Follow,
    })
    .expect("failed to resolve canonical OpenAI executor")
}

/// OpenAI vision CAPTCHA provider configuration and adapter.
///
/// The API key is caller-supplied, remains private and is applied only through
/// `SecretRequestHeaders`. The adapter owns no raw HTTP client.
#[cfg(feature = "openai")]
pub struct OpenAiVisionCaptchaProvider {
    model: String,
    api_key: String,
}

#[cfg(feature = "openai")]
impl std::fmt::Debug for OpenAiVisionCaptchaProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenAiVisionCaptchaProvider")
            .field("model", &self.model)
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}

#[cfg(feature = "openai")]
impl OpenAiVisionCaptchaProvider {
    /// Construct an explicitly configured provider. Credential acquisition is
    /// caller-owned; this constructor performs no environment lookup.
    pub fn new(model: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            api_key: api_key.into(),
        }
    }

    /// Return the explicitly configured OpenAI model identifier.
    pub fn model(&self) -> &str {
        &self.model
    }
}

#[cfg(feature = "openai")]
#[async_trait::async_trait]
impl CaptchaProvider for OpenAiVisionCaptchaProvider {
    fn capabilities(&self) -> &'static CaptchaProviderCapabilities {
        &OPENAI_VISION_CAPABILITIES
    }

    fn availability(&self) -> CaptchaProviderAvailability {
        if self.api_key.is_empty() {
            CaptchaProviderAvailability::CredentialUnavailable
        } else if self.model.trim().is_empty() {
            CaptchaProviderAvailability::ProviderUnavailable
        } else {
            CaptchaProviderAvailability::Available
        }
    }

    async fn solve(&self, request: &CaptchaSolveRequest) -> CaptchaSolveOutcome {
        solve_openai_vision_request(request, &self.model, &self.api_key).await
    }
}

#[cfg(feature = "openai")]
fn openai_request(api_key: &str, body: Vec<u8>) -> Result<CrawlerRequest, CaptchaSolveFailure> {
    let mut secret_headers = SecretRequestHeaders::new();
    let bearer = format!("Bearer {api_key}");
    secret_headers
        .try_insert("authorization", &bearer)
        .map_err(|_| CaptchaSolveFailure::CredentialUnavailable)?;
    Ok(CrawlerRequest {
        url: url::Url::parse(OPENAI_RESPONSES_ENDPOINT)
            .map_err(|_| CaptchaSolveFailure::ProviderUnavailable)?,
        method: reqwest::Method::POST,
        headers: reqwest::header::HeaderMap::new(),
        secret_headers,
        body: Some(body),
        content_type: Some("application/json".into()),
    })
}

#[cfg(feature = "openai")]
fn openai_failure(
    failure: CaptchaSolveFailure,
    transport_backend: Option<spider_transport::BackendProvenance>,
    response_origin: Option<spider_transport::ResponseOrigin>,
) -> CaptchaSolveOutcome {
    CaptchaSolveOutcome::Failed {
        failure,
        provenance: Some(CaptchaSolveProvenance {
            provider: CaptchaProviderId::OPENAI_VISION,
            locality: CaptchaProviderLocality::External,
            transport_backend,
            response_origin,
            local_runtime: None,
        }),
    }
}

#[cfg(feature = "openai")]
fn openai_output_text(value: &serde_json::Value) -> Option<&str> {
    value
        .get("output")?
        .as_array()?
        .iter()
        .flat_map(|item| {
            item.get("content")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
        })
        .find_map(|content| {
            (content.get("type").and_then(|v| v.as_str()) == Some("output_text"))
                .then(|| content.get("text").and_then(|v| v.as_str()))
                .flatten()
        })
}

#[cfg(feature = "openai")]
fn openai_prompt_and_solution(
    request: &CaptchaSolveRequest,
    text: &str,
) -> Result<CaptchaSolution, CaptchaSolveFailure> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Choices {
        selected_ids: Vec<String>,
    }
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Offset {
        x: f64,
    }
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Point {
        x: f64,
        y: f64,
    }

    match request.challenge.kind {
        CaptchaChallengeKind::ImageGridSelection => {
            let parsed: Choices = serde_json::from_str(text)
                .map_err(|_| CaptchaSolveFailure::InvalidProviderResponse)?;
            let valid_ids: std::collections::HashSet<&str> = request
                .challenge
                .visuals
                .iter()
                .filter_map(CaptchaVisualInput::id)
                .collect();
            let mut observed = std::collections::HashSet::new();
            if parsed
                .selected_ids
                .iter()
                .any(|id| !valid_ids.contains(id.as_str()) || !observed.insert(id.as_str()))
            {
                return Err(CaptchaSolveFailure::InvalidProviderResponse);
            }
            Ok(CaptchaSolution::SelectedChoices(parsed.selected_ids))
        }
        CaptchaChallengeKind::HorizontalOffset => {
            let parsed: Offset = serde_json::from_str(text)
                .map_err(|_| CaptchaSolveFailure::InvalidProviderResponse)?;
            if !parsed.x.is_finite() {
                return Err(CaptchaSolveFailure::InvalidProviderResponse);
            }
            Ok(CaptchaSolution::HorizontalOffset(parsed.x))
        }
        CaptchaChallengeKind::PointSelection => {
            let parsed: Point = serde_json::from_str(text)
                .map_err(|_| CaptchaSolveFailure::InvalidProviderResponse)?;
            if !parsed.x.is_finite() || !parsed.y.is_finite() {
                return Err(CaptchaSolveFailure::InvalidProviderResponse);
            }
            Ok(CaptchaSolution::Point {
                x: parsed.x,
                y: parsed.y,
            })
        }
    }
}

#[cfg(feature = "openai")]
fn openai_instruction(request: &CaptchaSolveRequest) -> String {
    match request.challenge.kind {
        CaptchaChallengeKind::ImageGridSelection => format!(
            "Select images matching this instruction: {}. Return only strict JSON {{\"selected_ids\":[\"id\"]}} using supplied IDs; an empty array is valid.",
            request.challenge.instruction
        ),
        CaptchaChallengeKind::HorizontalOffset => format!(
            "{}. Return only strict JSON {{\"x\":number}}.",
            request.challenge.instruction
        ),
        CaptchaChallengeKind::PointSelection => format!(
            "{}. Return only strict JSON {{\"x\":number,\"y\":number}}.",
            request.challenge.instruction
        ),
    }
}

#[cfg(feature = "openai")]
fn openai_payload(
    request: &CaptchaSolveRequest,
    model: &str,
) -> Result<serde_json::Value, CaptchaSolveFailure> {
    let mut content = vec![serde_json::json!({
        "type": "input_text",
        "text": openai_instruction(request),
    })];
    for visual in &request.challenge.visuals {
        if let Some(id) = visual.id() {
            content
                .push(serde_json::json!({"type": "input_text", "text": format!("Image ID: {id}")}));
        }
        content.push(serde_json::json!({
            "type": "input_image",
            "image_url": visual_as_data_url(visual)?,
        }));
    }
    Ok(serde_json::json!({
        "model": model,
        "input": [{"role": "user", "content": content}],
        "temperature": 0,
        "max_output_tokens": 128,
    }))
}

#[cfg(feature = "openai")]
async fn solve_openai_vision_request(
    request: &CaptchaSolveRequest,
    model: &str,
    api_key: &str,
) -> CaptchaSolveOutcome {
    let payload = match openai_payload(request, model) {
        Ok(payload) => payload,
        Err(failure) => return openai_failure(failure, None, None),
    };
    let body = match serde_json::to_vec(&payload) {
        Ok(body) => body,
        Err(_) => return openai_failure(CaptchaSolveFailure::InvalidChallenge, None, None),
    };
    let crawler_request = match openai_request(api_key, body) {
        Ok(request) => request,
        Err(failure) => return openai_failure(failure, None, None),
    };
    let response = match tokio::time::timeout(
        request.deadline,
        OPENAI_VISION_EXECUTOR.execute(crawler_request),
    )
    .await
    {
        Err(_) => return openai_failure(CaptchaSolveFailure::DeadlineExceeded, None, None),
        Ok(Err(failure)) => {
            let backend = failure.backend();
            return openai_failure(CaptchaSolveFailure::Transport(failure), Some(backend), None);
        }
        Ok(Ok(response)) => response,
    };
    let backend = response.backend;
    let origin = response.origin;
    if !response.status.is_success() {
        return openai_failure(
            CaptchaSolveFailure::ProviderRejected,
            Some(backend),
            Some(origin),
        );
    }
    let bytes = match collect_captcha_body(response.body).await {
        Ok(bytes) => bytes,
        Err(failure) => {
            return openai_failure(
                CaptchaSolveFailure::Transport(failure),
                Some(backend),
                Some(origin),
            )
        }
    };
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => {
            return openai_failure(
                CaptchaSolveFailure::InvalidProviderResponse,
                Some(backend),
                Some(origin),
            )
        }
    };
    let solution = match openai_output_text(&value)
        .ok_or(CaptchaSolveFailure::InvalidProviderResponse)
        .and_then(|text| openai_prompt_and_solution(request, text))
    {
        Ok(solution) => solution,
        Err(failure) => return openai_failure(failure, Some(backend), Some(origin)),
    };
    CaptchaSolveOutcome::Solved {
        solution,
        provenance: CaptchaSolveProvenance::external(
            CaptchaProviderId::OPENAI_VISION,
            backend,
            origin,
        ),
    }
}

#[cfg(all(feature = "chrome", feature = "real_browser"))]
fn local_failure(error: &CdpError) -> CaptchaSolveFailure {
    if is_missing_helper_error(error) {
        CaptchaSolveFailure::ProviderUnavailable
    } else if matches!(error, CdpError::Timeout) {
        CaptchaSolveFailure::DeadlineExceeded
    } else {
        CaptchaSolveFailure::LocalExecutionFailure
    }
}

#[cfg(all(feature = "chrome", feature = "real_browser"))]
struct LocalLanguageModelProvider<'a> {
    page: &'a Page,
}

#[cfg(all(feature = "chrome", feature = "real_browser"))]
#[async_trait::async_trait]
impl CaptchaProvider for LocalLanguageModelProvider<'_> {
    fn capabilities(&self) -> &'static CaptchaProviderCapabilities {
        &LOCAL_LANGUAGE_MODEL_CAPABILITIES
    }

    fn availability(&self) -> CaptchaProviderAvailability {
        CaptchaProviderAvailability::Available
    }

    async fn solve(&self, request: &CaptchaSolveRequest) -> CaptchaSolveOutcome {
        let solution = match request.challenge.kind {
            CaptchaChallengeKind::ImageGridSelection => {
                let mut tiles = Vec::with_capacity(request.challenge.visuals.len());
                for visual in &request.challenge.visuals {
                    let dataurl = match visual_as_data_url(visual) {
                        Ok(value) => value,
                        Err(failure) => {
                            return CaptchaSolveOutcome::Failed {
                                failure,
                                provenance: None,
                            }
                        }
                    };
                    tiles.push(serde_json::json!({
                        "id": visual.id().and_then(|id| id.parse::<u8>().ok()).unwrap_or_default(),
                        "dataurl": dataurl,
                    }));
                }
                match solve_with_inpage_helper(
                    self.page,
                    &tiles,
                    &request.challenge.instruction,
                    request.deadline.as_millis() as u64,
                )
                .await
                {
                    Ok(ids) => CaptchaSolution::SelectedChoices(
                        ids.into_iter().map(|id| id.to_string()).collect(),
                    ),
                    Err(error) => {
                        return CaptchaSolveOutcome::Failed {
                            failure: local_failure(&error),
                            provenance: Some(CaptchaSolveProvenance::local(
                                CaptchaProviderId::LOCAL_LANGUAGE_MODEL,
                            )),
                        }
                    }
                }
            }
            CaptchaChallengeKind::HorizontalOffset => {
                let dataurl = match request
                    .challenge
                    .visuals
                    .first()
                    .ok_or(CaptchaSolveFailure::InvalidChallenge)
                    .and_then(visual_as_data_url)
                {
                    Ok(value) => value,
                    Err(failure) => {
                        return CaptchaSolveOutcome::Failed {
                            failure,
                            provenance: None,
                        }
                    }
                };
                match solve_geetest_with_local_language_model(
                    self.page,
                    &dataurl,
                    request.deadline.as_millis() as u64,
                )
                .await
                {
                    Ok(offset) => CaptchaSolution::HorizontalOffset(offset),
                    Err(error) => {
                        return CaptchaSolveOutcome::Failed {
                            failure: local_failure(&error),
                            provenance: Some(CaptchaSolveProvenance::local(
                                CaptchaProviderId::LOCAL_LANGUAGE_MODEL,
                            )),
                        }
                    }
                }
            }
            CaptchaChallengeKind::PointSelection => {
                let dataurl = match request
                    .challenge
                    .visuals
                    .first()
                    .ok_or(CaptchaSolveFailure::InvalidChallenge)
                    .and_then(visual_as_data_url)
                {
                    Ok(value) => value,
                    Err(failure) => {
                        return CaptchaSolveOutcome::Failed {
                            failure,
                            provenance: None,
                        }
                    }
                };
                match solve_lemin_with_inpage_helper(
                    self.page,
                    &dataurl,
                    request.deadline.as_millis() as u64,
                )
                .await
                {
                    Ok((x, y)) => CaptchaSolution::Point { x, y },
                    Err(error) => {
                        return CaptchaSolveOutcome::Failed {
                            failure: local_failure(&error),
                            provenance: Some(CaptchaSolveProvenance::local(
                                CaptchaProviderId::LOCAL_LANGUAGE_MODEL,
                            )),
                        }
                    }
                }
            }
        };
        CaptchaSolveOutcome::Solved {
            solution,
            provenance: CaptchaSolveProvenance::local(CaptchaProviderId::LOCAL_LANGUAGE_MODEL),
        }
    }
}

/// Process-lifetime PaliGemma provider singleton. Resolved at most once per
/// process: the first caller to route a challenge with `PALIGEMMA_LOCAL`
/// selected pays the real model-load cost (an 11+ GB checkpoint; not
/// something any per-request deadline could tolerate), every later caller —
/// concurrently or sequentially — observes the same already-resolved
/// `Some`/`None` instantly. A failed resolution (no canonical artifact
/// source configured, or verification/load failure) is remembered as
/// `None` for the rest of the process's life; it is never silently
/// retried, never falls back to downloading anything, and never
/// substitutes a different model.
#[cfg(all(feature = "chrome", feature = "local_paligemma"))]
static PALIGEMMA_PROVIDER: tokio::sync::OnceCell<
    Option<crate::features::paligemma_captcha::PaligemmaLocalCaptchaProvider>,
> = tokio::sync::OnceCell::const_new();

/// Resolve (once) and return the process-lifetime PaliGemma provider, or
/// `None` if no canonical local installation could be verified. Reuses the
/// exact same canonical artifact contract every PaliGemma test in this
/// crate already relies on (`SCORPION_PALIGEMMA_PINNED_ARTIFACTS`: an
/// offline directory holding the pinned, hash-identified 224/CPU/F32
/// checkpoint) — this router introduces no new artifact-source convention.
/// Real disk I/O and model loading always run on a blocking thread, never
/// inline on an async worker.
#[cfg(all(feature = "chrome", feature = "local_paligemma"))]
async fn paligemma_provider(
) -> Option<&'static crate::features::paligemma_captcha::PaligemmaLocalCaptchaProvider> {
    PALIGEMMA_PROVIDER
        .get_or_init(|| async {
            tokio::task::spawn_blocking(resolve_paligemma_provider)
                .await
                .ok()
                .flatten()
        })
        .await
        .as_ref()
}

/// Real, synchronous artifact resolution + model load — deliberately run on
/// a blocking thread by its only caller, [`paligemma_provider`]. Returns
/// `None` (never panics, never fabricates a provider) for every failure
/// case: no `SCORPION_PALIGEMMA_PINNED_ARTIFACTS` configured, missing/
/// corrupt pinned artifacts, or a genuine model-load failure.
///
/// Backend selection is a build-time decision, matching every other
/// CUDA/CPU choice `PaligemmaCpuRuntime` already makes: when
/// `local_paligemma_cuda` is compiled, this router uses the accelerated
/// CUDA/F16 constructor exclusively — never CPU/F32, and never a runtime
/// "try CUDA, fall back to CPU" branch. This is not a new fallback: it is
/// the same fail-closed contract `initialize_cuda_f16_from_host` itself
/// already enforces (no CUDA device / insufficient VRAM -> `Err`, never a
/// silent CPU downgrade) — this router simply stops shadowing it with an
/// unconditional CPU/F32 call. Real measurement
/// (`SCORPION_PALIGEMMA_LOCAL_VL_REAL_INFERENCE_QUALIFICATION_001`): a
/// real CPU/F32 `detect` call took ~400-420s; the real CUDA/F16 path
/// completed the exact same query in ~1-11s (build-profile dependent) —
/// CPU/F32 alone is not an operationally practical router default when a
/// qualified CUDA build exists.
#[cfg(all(feature = "chrome", feature = "local_paligemma"))]
fn resolve_paligemma_provider(
) -> Option<crate::features::paligemma_captcha::PaligemmaLocalCaptchaProvider> {
    use crate::features::paligemma_captcha::PaligemmaLocalCaptchaProvider;
    #[cfg(not(feature = "local_paligemma_cuda"))]
    use crate::features::paligemma_runtime::paligemma_cpu_f32_manifest as paligemma_manifest;
    #[cfg(feature = "local_paligemma_cuda")]
    use crate::features::paligemma_runtime::paligemma_cuda_f16_manifest as paligemma_manifest;

    let source = std::env::var_os("SCORPION_PALIGEMMA_PINNED_ARTIFACTS")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)?;
    let manifest = paligemma_manifest();

    // A stable (not per-process-random) location so a process restart on
    // the same host reuses an already-verified activation instead of
    // re-staging every pinned artifact from the canonical source again.
    // Shared across the CPU/F32 and CUDA/F16 builds: both manifests pin
    // the exact same artifact set/hashes for the 224 checkpoint (only
    // `runtime_requirements` differs), so an installation activated by
    // one build's binary reopens cleanly under the other.
    let base = std::env::temp_dir().join("scorpion-paligemma-runtime");
    let active = base.join("active");

    let installation = manifest.open_installation(&active).ok().or_else(|| {
        let staging = base.join("staging");
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).ok()?;
        for artifact in &manifest.artifacts {
            let from = source.join(&artifact.relative_path);
            let to = staging.join(&artifact.relative_path);
            // Prefer a hard link (instant, no extra disk use for an
            // 11+ GB checkpoint); fall back to a real copy only if the
            // source and the OS temp dir are on different filesystems.
            if std::fs::hard_link(&from, &to).is_err() && std::fs::copy(&from, &to).is_err() {
                return None;
            }
        }
        manifest.activate(&staging, &active).ok()
    })?;

    #[cfg(feature = "local_paligemma_cuda")]
    {
        PaligemmaLocalCaptchaProvider::initialize_cuda_f16_from_host(&installation).ok()
    }
    #[cfg(not(feature = "local_paligemma_cuda"))]
    {
        PaligemmaLocalCaptchaProvider::initialize_from_host(&installation).ok()
    }
}

/// The one canonical production entry point from a detected browser
/// challenge to a provider-routed outcome, optionally bound to a real
/// browser action —
/// `CANONICAL CHALLENGE -> PROVIDER ROUTER -> PROVIDER RESOLUTION -> CANONICAL PROVIDER OUTCOME [-> BROWSER ACTION]`.
///
/// Composes only canonical primitives already defined in
/// [`crate::features::captcha`] ([`CaptchaProviderRegistry`],
/// [`CaptchaRouteAttempts::execute_explicit_attempt`], which itself already
/// gates on [`CaptchaProvider::availability`] and, through
/// [`solve_captcha`], on challenge/provider compatibility) — it owns no
/// retry, fallback, or provider-substitution policy of its own, and
/// performs exactly one explicit attempt against exactly the
/// caller-configured `selected_provider`, or none at all when
/// `selected_provider` is `None`.
///
/// Registers every provider this build can construct with no
/// caller-supplied external credential: [`CaptchaProviderId::LOCAL_LANGUAGE_MODEL`]
/// (needs only the live page already in hand, only when this build compiles
/// it — `real_browser`) and [`CaptchaProviderId::PALIGEMMA_LOCAL`] (needs
/// the process-lifetime singleton in [`paligemma_provider`] to have
/// resolved a verified local installation, only when this build compiles it
/// — `local_paligemma`). `EXTERNAL_GEMINI` / `OPENAI_VISION` need
/// caller-supplied API keys this router has no canonical source for.
/// Selecting any provider this build cannot construct, or whose local
/// installation could not be verified, resolves to `ProviderUnavailable` —
/// a truthful "not registered", never a silent no-op or a fabricated
/// credential. Registering more than one provider is not a fallback chain:
/// [`CaptchaRouteAttempts::execute_explicit_attempt`] still resolves and
/// invokes exactly the one `selected_provider`.
///
/// `snapshot` gates whether a produced solution is ever bound to the real
/// browser (`SCORPION_CANONICAL_CAPTCHA_SOLUTION_BROWSER_ACTION_BINDING_001`):
/// `None` preserves this function's original solve-only contract exactly
/// (no snapshot, no action, ever — the shape every existing caller and unit
/// test in this module still uses). `Some(snapshot)` additionally binds a
/// produced solution to the browser through the pre-proven, provider-neutral
/// [`crate::features::captcha_browser::execute_browser_captcha_attempt`]
/// seam — the *only* browser-input dispatcher this function is ever allowed
/// to reach (see `provider_router_binding_only_acts_through_the_canonical_browser_seam`
/// in `architecture_guardrails.rs`): no ad-hoc click/type/submit here, and
/// no branch on which specific provider produced the solution. This makes
/// exactly one explicit provider attempt regardless of branch — the
/// solve-then-bind sequence is `execute_browser_captcha_attempt`'s own
/// internal composition, never a second, duplicate solve.
///
/// Dispatching a browser action is still never a "solved" claim — see
/// [`crate::features::captcha::CaptchaBrowserActionOutcome`]'s doc comment.
/// The truthful, minimal, real-DOM post-action observation
/// (`challenge_observed_after_action`) is filled in by this function's sole
/// caller, [`crate::features::browser_challenge_detection::DetectedBrowserChallenge::route`],
/// which alone owns a second passive detection pass — this function never
/// re-implements or reaches into that detector itself, preserving the
/// existing one-way `browser_challenge_detection -> solvers` layering.
#[cfg(feature = "chrome")]
pub(crate) async fn route_detected_browser_challenge(
    page: &chromiumoxide::Page,
    snapshot: Option<&crate::features::browser_challenge::BrowserChallengeSnapshot>,
    challenge: CaptchaChallenge,
    selected_provider: Option<CaptchaProviderId>,
    deadline: Duration,
) -> CaptchaRouteOutcomeSummary {
    let Some(selected_provider) = selected_provider else {
        return CaptchaRouteOutcomeSummary::NotConfigured;
    };

    #[cfg(feature = "real_browser")]
    let local_language_model_provider = LocalLanguageModelProvider { page };
    #[cfg(not(feature = "real_browser"))]
    let _ = page;

    #[cfg(feature = "local_paligemma")]
    let paligemma_local_provider = paligemma_provider().await;

    // Genuinely unused when neither `real_browser` nor `local_paligemma` is
    // compiled — no provider can be registered at all, and
    // `execute_explicit_attempt` below already reports `ProviderUnavailable`
    // for an empty registry.
    #[cfg_attr(
        not(any(feature = "real_browser", feature = "local_paligemma")),
        allow(unused_mut)
    )]
    let mut registry = CaptchaProviderRegistry::new();
    // Every registration failure here can only be `DuplicateProvider`,
    // structurally unreachable (each provider identity is registered at
    // most once) — ignored rather than unwrapped so a future duplicate
    // fails closed (resolve() simply won't find it) instead of panicking
    // the whole page construction.
    #[cfg(feature = "real_browser")]
    {
        let _ = registry.register(&local_language_model_provider);
    }
    #[cfg(feature = "local_paligemma")]
    if let Some(provider) = paligemma_local_provider {
        let _ = registry.register(provider);
    }

    if let Some(snapshot) = snapshot {
        let attempt = CaptchaBrowserAttempt {
            correlation_id: "browser-challenge-point-selection".into(),
            selected_provider,
            deadline,
            challenge: CaptchaBrowserChallenge::PointSelection {
                instruction: challenge.instruction,
            },
        };
        return match execute_browser_captcha_attempt(page, snapshot, &registry, attempt).await {
            Ok(report) => CaptchaRouteOutcomeSummary::SolutionProduced {
                action: CaptchaBrowserActionOutcome::Applied {
                    actions_applied: report.actions_applied,
                    // Always immediately overwritten by the sole caller
                    // with a real post-action detection result before this
                    // outcome is ever retained or observed externally — see
                    // this function's own doc comment.
                    challenge_observed_after_action: false,
                },
            },
            Err(failure) => outcome_for_browser_action_failure(failure),
        };
    }

    let request = CaptchaSolveRequest {
        correlation_id: "browser-challenge-point-selection".into(),
        selected_provider,
        challenge,
        deadline,
    };
    let mut attempts = CaptchaRouteAttempts::new();
    let outcome = attempts.execute_explicit_attempt(&registry, &request).await;
    summarize_route_outcome(outcome)
}

/// Reduce one [`CaptchaBrowserExecutionFailure`] to the same
/// `Clone + Debug + PartialEq` summary vocabulary as the solve-only path.
/// `ProviderFailure` means the provider attempt itself failed before any
/// browser binding was attempted — recovered from the retained attempt
/// ledger and classified through the exact same [`summarize_route_outcome`]
/// used by the solve-only path, so both paths report a given provider
/// failure identically. Every other kind means a solution *was* produced
/// but binding it to the browser failed (materialization, an unbound or
/// out-of-bounds solution, or the exact action/revalidation dispatch
/// itself) — reported as `SolutionProduced`'s own `Failed` action outcome,
/// preserving the true fact that the provider succeeded.
#[cfg(feature = "chrome")]
fn outcome_for_browser_action_failure(
    failure: CaptchaBrowserExecutionFailure,
) -> CaptchaRouteOutcomeSummary {
    match &failure.kind {
        CaptchaBrowserExecutionFailureKind::ProviderFailure => {
            match failure.attempts.recorded().last() {
                Some(attempt) => summarize_route_outcome(&attempt.outcome),
                None => CaptchaRouteOutcomeSummary::ProviderFailed(format!("{:?}", failure.kind)),
            }
        }
        _ => CaptchaRouteOutcomeSummary::SolutionProduced {
            action: CaptchaBrowserActionOutcome::Failed(format!("{:?}", failure.kind)),
        },
    }
}

/// Reduce a live [`CaptchaSolveOutcome`] to the `Clone + Debug + PartialEq`
/// summary retained on [`crate::page::Page`] — see
/// [`CaptchaRouteOutcomeSummary`]'s doc comment for why.
#[cfg(feature = "chrome")]
fn summarize_route_outcome(outcome: &CaptchaSolveOutcome) -> CaptchaRouteOutcomeSummary {
    match outcome {
        CaptchaSolveOutcome::Solved { .. } => CaptchaRouteOutcomeSummary::SolutionProduced {
            action: CaptchaBrowserActionOutcome::NotAttempted,
        },
        CaptchaSolveOutcome::Failed { failure, .. } => match failure {
            CaptchaSolveFailure::ProviderUnavailable
            | CaptchaSolveFailure::CredentialUnavailable => {
                CaptchaRouteOutcomeSummary::ProviderUnavailable
            }
            CaptchaSolveFailure::UnsupportedChallenge => {
                CaptchaRouteOutcomeSummary::UnsupportedChallenge
            }
            other => CaptchaRouteOutcomeSummary::ProviderFailed(format!("{other:?}")),
        },
    }
}

#[cfg(all(feature = "chrome", feature = "real_browser"))]
struct ExternalGeminiProvider<'a> {
    api_key: &'a str,
}

#[cfg(all(feature = "chrome", feature = "real_browser"))]
async fn execute_external_gemini_json(
    endpoint: &str,
    api_key: &str,
    payload: &serde_json::Value,
) -> Result<(serde_json::Value, CaptchaSolveProvenance), CaptchaSolveFailure> {
    let body = serde_json::to_vec(payload).map_err(|_| CaptchaSolveFailure::InvalidChallenge)?;
    let request = gemini_post_request(endpoint, api_key, body)
        .map_err(|_| CaptchaSolveFailure::InvalidChallenge)?;
    let response = GEMINI_EXECUTOR
        .execute(request)
        .await
        .map_err(CaptchaSolveFailure::Transport)?;
    let provenance = CaptchaSolveProvenance::external(
        CaptchaProviderId::EXTERNAL_GEMINI,
        response.backend,
        response.origin,
    );
    if !response.status.is_success() {
        return Err(CaptchaSolveFailure::ProviderRejected);
    }
    let body = collect_captcha_body(response.body)
        .await
        .map_err(CaptchaSolveFailure::Transport)?;
    let value =
        serde_json::from_slice(&body).map_err(|_| CaptchaSolveFailure::InvalidProviderResponse)?;
    Ok((value, provenance))
}

#[cfg(all(feature = "chrome", feature = "real_browser"))]
fn external_failure(failure: CaptchaSolveFailure) -> CaptchaSolveOutcome {
    let transport_backend = match &failure {
        CaptchaSolveFailure::Transport(failure) => Some(failure.backend()),
        _ => None,
    };
    CaptchaSolveOutcome::Failed {
        failure,
        provenance: Some(CaptchaSolveProvenance {
            provider: CaptchaProviderId::EXTERNAL_GEMINI,
            locality: CaptchaProviderLocality::External,
            transport_backend,
            response_origin: None,
            local_runtime: None,
        }),
    }
}

#[cfg(all(feature = "chrome", feature = "real_browser"))]
async fn solve_external_gemini_request(
    request: &CaptchaSolveRequest,
    api_key: &str,
) -> CaptchaSolveOutcome {
    if api_key.is_empty() {
        return external_failure(CaptchaSolveFailure::CredentialUnavailable);
    }
    let solve = async {
        match request.challenge.kind {
            CaptchaChallengeKind::ImageGridSelection => {
                let mut selected = Vec::new();
                let mut provenance = None;
                let mut successful_answers = 0usize;
                let mut last_failure = None;
                let per_operation = request.deadline / (request.challenge.visuals.len() as u32 + 1);
                for visual in &request.challenge.visuals {
                    let bytes = match visual.bytes() {
                        Some(bytes) => bytes,
                        None => return external_failure(CaptchaSolveFailure::InvalidChallenge),
                    };
                    let payload = serde_json::json!({
                        "contents": [{
                            "role": "user",
                            "parts": [
                                { "text": format!("Does this image contain a {}? Answer only with \"yes\" or \"no\".", request.challenge.instruction) },
                                { "inlineData": { "mimeType": visual.media_type(), "data": BASE64_STANDARD.encode(bytes) } }
                            ]
                        }],
                        "generationConfig": { "maxOutputTokens": 5, "temperature": 0.0 }
                    });
                    match tokio::time::timeout(
                        per_operation,
                        execute_external_gemini_json(&GEMINI_VISION_ENDPOINT, api_key, &payload),
                    )
                    .await
                    {
                        Err(_) => last_failure = Some(CaptchaSolveFailure::DeadlineExceeded),
                        Ok(Err(failure)) => last_failure = Some(failure),
                        Ok(Ok((response, observed))) => {
                            successful_answers += 1;
                            provenance.get_or_insert(observed);
                            let answer = response
                                .get("candidates")
                                .and_then(|value| value.get(0))
                                .and_then(|value| value.get("content"))
                                .and_then(|value| value.get("parts"))
                                .and_then(|value| value.get(0))
                                .and_then(|value| value.get("text"))
                                .and_then(|value| value.as_str())
                                .unwrap_or("")
                                .trim()
                                .to_ascii_lowercase();
                            if answer.contains("yes") {
                                selected.push(visual.id().unwrap_or_default().to_string());
                            }
                        }
                    }
                }
                if successful_answers == 0 {
                    return external_failure(
                        last_failure.unwrap_or(CaptchaSolveFailure::Inconclusive),
                    );
                }
                CaptchaSolveOutcome::Solved {
                    solution: CaptchaSolution::SelectedChoices(selected),
                    provenance: provenance.expect("successful response records provenance"),
                }
            }
            CaptchaChallengeKind::PointSelection => {
                let visual = match request.challenge.visuals.first() {
                    Some(visual) => visual,
                    None => return external_failure(CaptchaSolveFailure::InvalidChallenge),
                };
                let bytes = match visual.bytes() {
                    Some(bytes) => bytes,
                    None => return external_failure(CaptchaSolveFailure::InvalidChallenge),
                };
                let payload = serde_json::json!({
                    "contents": [{
                        "role": "user",
                        "parts": [
                            { "text": "Give me the centre (x and y coordinates) of the missing puzzle piece in this image. Return a JSON array like [x, y] with numbers only." },
                            { "inlineData": { "mimeType": visual.media_type(), "data": BASE64_STANDARD.encode(bytes) } }
                        ]
                    }],
                    "generationConfig": { "maxOutputTokens": 16, "temperature": 0.0 }
                });
                match execute_external_gemini_json(&GEMINI_VISION_ENDPOINT, api_key, &payload).await
                {
                    Ok((response, provenance)) => {
                        let text = response
                            .get("candidates")
                            .and_then(|value| value.get(0))
                            .and_then(|value| value.get("content"))
                            .and_then(|value| value.get("parts"))
                            .and_then(|value| value.get(0))
                            .and_then(|value| value.get("text"))
                            .and_then(|value| value.as_str());
                        let coordinates = text
                            .and_then(|value| serde_json::from_str::<Vec<f64>>(value.trim()).ok());
                        match coordinates {
                            Some(coordinates) if coordinates.len() == 2 => {
                                CaptchaSolveOutcome::Solved {
                                    solution: CaptchaSolution::Point {
                                        x: coordinates[0],
                                        y: coordinates[1],
                                    },
                                    provenance,
                                }
                            }
                            _ => external_failure(CaptchaSolveFailure::InvalidProviderResponse),
                        }
                    }
                    Err(failure) => external_failure(failure),
                }
            }
            CaptchaChallengeKind::HorizontalOffset => {
                let visual = match request.challenge.visuals.first() {
                    Some(visual) => visual,
                    None => return external_failure(CaptchaSolveFailure::InvalidChallenge),
                };
                let image = match visual_as_data_url(visual) {
                    Ok(image) => image,
                    Err(failure) => return external_failure(failure),
                };
                let payload = serde_json::json!({
                    "image": image,
                    "prompt": r#"
You are shown a screenshot of a GeeTest sliding‑puzzle captcha.
The image contains a background with a single missing puzzle piece cut‑out.
Return **only** the horizontal pixel offset (integer or float) of the left edge of the missing piece
measured from the left border of the image.
Do NOT return any extra text, JSON keys, or explanations.
"#,
                });
                let endpoint = format!("{}:generateContent", *GEMINI_VISION_ENDPOINT);
                match execute_external_gemini_json(&endpoint, api_key, &payload).await {
                    Ok((response, provenance)) => {
                        match response.get("x").and_then(|x| x.as_f64()) {
                            Some(offset) => CaptchaSolveOutcome::Solved {
                                solution: CaptchaSolution::HorizontalOffset(offset),
                                provenance,
                            },
                            None => external_failure(CaptchaSolveFailure::InvalidProviderResponse),
                        }
                    }
                    Err(failure) => external_failure(failure),
                }
            }
        }
    };
    match tokio::time::timeout(request.deadline, solve).await {
        Ok(outcome) => outcome,
        Err(_) => external_failure(CaptchaSolveFailure::DeadlineExceeded),
    }
}

#[cfg(all(feature = "chrome", feature = "real_browser"))]
fn visual_from_data_url(
    id: Option<String>,
    dataurl: &str,
) -> Result<CaptchaVisualInput, CaptchaSolveFailure> {
    let (metadata, encoded) = dataurl
        .split_once(',')
        .ok_or(CaptchaSolveFailure::InvalidChallenge)?;
    let media_type = metadata
        .strip_prefix("data:")
        .and_then(|value| value.strip_suffix(";base64"))
        .ok_or(CaptchaSolveFailure::InvalidChallenge)?;
    let bytes = BASE64_STANDARD
        .decode(encoded.trim())
        .map_err(|_| CaptchaSolveFailure::InvalidChallenge)?;
    Ok(CaptchaVisualInput::materialized(id, media_type, bytes))
}

#[cfg(all(feature = "chrome", feature = "real_browser"))]
async fn materialize_remote_challenge(
    challenge: CaptchaChallenge,
) -> Result<CaptchaChallenge, CaptchaSolveFailure> {
    let mut visuals = Vec::with_capacity(challenge.visuals.len());
    for visual in challenge.visuals {
        match visual {
            CaptchaVisualInput::Materialized { .. }
            | CaptchaVisualInput::MaterializedFullGrid(_) => visuals.push(visual),
            CaptchaVisualInput::RemoteAsset {
                id,
                media_type,
                url,
            } => {
                let response = GEMINI_EXECUTOR
                    .execute(CrawlerRequest::get(url))
                    .await
                    .map_err(CaptchaSolveFailure::Transport)?;
                if !response.status.is_success() {
                    return Err(CaptchaSolveFailure::Transport(
                        CrawlerFailure::new(CrawlerFailureKind::HttpStatus, response.backend)
                            .with_status(response.status),
                    ));
                }
                let bytes = collect_captcha_body(response.body)
                    .await
                    .map_err(CaptchaSolveFailure::Transport)?;
                visuals.push(CaptchaVisualInput::materialized(id, media_type, bytes));
            }
        }
    }
    Ok(CaptchaChallenge {
        visuals,
        ..challenge
    })
}

#[cfg(all(feature = "chrome", feature = "real_browser"))]
fn route_error(route: &CaptchaRouteAttempts) -> CdpError {
    CdpError::msg(format!(
        "CAPTCHA solver route failed: {:?}",
        route.attempts()
    ))
}

#[cfg(all(feature = "chrome", feature = "real_browser"))]
async fn solve_horizontal_offset_with_legacy_routing(
    page: &Page,
    dataurl: &str,
    timeout_ms: u64,
    _compatibility_fallback: f64,
) -> Result<f64, CdpError> {
    let visual = visual_from_data_url(None, dataurl)
        .map_err(|_| CdpError::msg("invalid CAPTCHA visual input"))?;
    let challenge = CaptchaChallenge {
        kind: CaptchaChallengeKind::HorizontalOffset,
        instruction: "Return only the horizontal pixel offset (as a number) of the missing puzzle piece gap in this image.".into(),
        visuals: vec![visual],
    };
    let local_request = CaptchaSolveRequest {
        correlation_id: "geetest-horizontal-offset".into(),
        selected_provider: CaptchaProviderId::LOCAL_LANGUAGE_MODEL,
        challenge: challenge.clone(),
        deadline: Duration::from_millis(timeout_ms),
    };
    let local_provider = LocalLanguageModelProvider { page };
    let mut registry = CaptchaProviderRegistry::new();
    registry
        .register(&local_provider)
        .expect("local provider identity is unique");
    let mut route = CaptchaRouteAttempts::new();
    match route
        .execute_explicit_attempt(&registry, &local_request)
        .await
    {
        CaptchaSolveOutcome::Solved {
            solution: CaptchaSolution::HorizontalOffset(offset),
            ..
        } => Ok(*offset),
        CaptchaSolveOutcome::Failed {
            failure: CaptchaSolveFailure::ProviderUnavailable,
            ..
        } => {
            #[cfg(feature = "gemini")]
            {
                let api_key = std::env::var("GEMINI_API_KEY")
                    .map_err(|_| CdpError::msg("GEMINI_API_KEY not set"))?;
                let request = CaptchaSolveRequest {
                    correlation_id: "geetest-horizontal-offset-external".into(),
                    selected_provider: CaptchaProviderId::EXTERNAL_GEMINI,
                    challenge,
                    deadline: Duration::from_millis(timeout_ms),
                };
                let _permit = crate::utils::GEMINI_SEM
                    .acquire()
                    .await
                    .map_err(|_| CdpError::msg("Gemini solver admission cancelled"))?;
                let external_provider = ExternalGeminiProvider { api_key: &api_key };
                registry
                    .register(&external_provider)
                    .expect("external provider identity is unique");
                match route.execute_explicit_attempt(&registry, &request).await {
                    CaptchaSolveOutcome::Solved {
                        solution: CaptchaSolution::HorizontalOffset(offset),
                        ..
                    } => Ok(*offset),
                    _ => Err(route_error(&route)),
                }
            }
            #[cfg(not(feature = "gemini"))]
            {
                Ok(_compatibility_fallback)
            }
        }
        _ => Err(route_error(&route)),
    }
}

/// Upstream-compatible in-page GeeTest entrypoint. Provider routing remains
/// caller policy and delegates to the canonical single-provider seam.
#[cfg(all(feature = "chrome", feature = "real_browser"))]
pub async fn solve_geetest_with_inpage_helper(
    page: &Page,
    canvas_dataurl: &str,
    timeout_ms: u64,
) -> Result<f64, CdpError> {
    solve_horizontal_offset_with_legacy_routing(page, canvas_dataurl, timeout_ms, 0.0).await
}

#[cfg(all(feature = "chrome", feature = "real_browser"))]
#[async_trait::async_trait]
impl CaptchaProvider for ExternalGeminiProvider<'_> {
    fn capabilities(&self) -> &'static CaptchaProviderCapabilities {
        &EXTERNAL_GEMINI_CAPABILITIES
    }

    fn availability(&self) -> CaptchaProviderAvailability {
        if self.api_key.is_empty() {
            CaptchaProviderAvailability::CredentialUnavailable
        } else {
            CaptchaProviderAvailability::Available
        }
    }

    async fn solve(&self, request: &CaptchaSolveRequest) -> CaptchaSolveOutcome {
        solve_external_gemini_request(request, self.api_key).await
    }
}

lazy_static! {
    /// Imperva check
    static ref AC_IMPERVA_IFRAME: aho_corasick::AhoCorasick = aho_corasick::AhoCorasickBuilder::new()
        .ascii_case_insensitive(true)
        .match_kind(aho_corasick::MatchKind::LeftmostFirst)
        .build(IMPERVA_IFRAME_PATTERNS)
        .expect("valid imperva iframe patterns");
    /// Bot verify.
    static ref AC: AhoCorasick =  aho_corasick::AhoCorasickBuilder::new()
        .match_kind(aho_corasick::MatchKind::LeftmostLongest)
        .build(VERIFY_PATTERNS)
        .unwrap();
    /// Recaptcha patterns.
    static ref RECAPTCHA_AC: AhoCorasick = AhoCorasickBuilder::new()
        .ascii_case_insensitive(true)
        .match_kind(aho_corasick::MatchKind::LeftmostFirst)
        .build(RECAPTCHA_PATTERNS)
        .expect("valid recaptcha patterns");
    /// GeeTest patterns.
    static ref GEETEST_AC: AhoCorasick = AhoCorasickBuilder::new()
        .ascii_case_insensitive(true)
        .match_kind(aho_corasick::MatchKind::LeftmostFirst)
        .build(GEETEST_PATTERNS)
        .expect("valid geetest patterns");
    /// GeeTest loading AC.
    static ref GEETEST_LOADING_AC: AhoCorasick = AhoCorasickBuilder::new()
        .ascii_case_insensitive(true)
        .match_kind(aho_corasick::MatchKind::LeftmostFirst)
        .build(GEETEST_LOADING_PATTERNS)
        .expect("valid geetest loading patterns");
    /// GeeTest visible‑challenge matcher
    static ref GEETEST_VISIBLE_AC: AhoCorasick = AhoCorasickBuilder::new()
        .ascii_case_insensitive(true)
        .match_kind(aho_corasick::MatchKind::LeftmostFirst)
        .build(GEETEST_VISIBLE_PATTERNS)
        .expect("valid geetest visible patterns");
    /// Imperva wait AC.
    static ref IMPERVA_WAIT_AC: AhoCorasick = AhoCorasickBuilder::new()
            .ascii_case_insensitive(true)
            .match_kind(aho_corasick::MatchKind::LeftmostFirst)
            .build(IMPERVA_WAIT_PATTERNS)
            .expect("valid Imperva wait‑screen patterns");

    /// Imperva iframe matcher.
    static ref IMPERVA_IFRAME_PHASE_AC: AhoCorasick = AhoCorasickBuilder::new()
        .ascii_case_insensitive(true)
        .match_kind(aho_corasick::MatchKind::LeftmostFirst)
        .build(IMPERVA_IFRAME_PHASE_PATTERNS)
        .expect("valid Imperva iframe‑phase patterns");
    /// Lemin match.
    static ref LEMIN_AC: AhoCorasick = AhoCorasickBuilder::new()
        .ascii_case_insensitive(true)
        .match_kind(aho_corasick::MatchKind::LeftmostFirst)
        .build(LEMIN_PATTERNS)
        .expect("valid lemin patterns");
}

#[cfg(feature = "chrome")]
/// CF prefix scan bytes.
const CF_PREFIX_SCAN_BYTES: usize = 120;

#[cfg(feature = "chrome")]
#[inline(always)]
/// CF slice prefix.
fn cf_prefix_slice(b: &[u8]) -> &[u8] {
    if b.len() > CF_PREFIX_SCAN_BYTES {
        &b[..CF_PREFIX_SCAN_BYTES]
    } else {
        b
    }
}

#[cfg(feature = "chrome")]
lazy_static! {
    /// CF end match.
    static ref CF_END: &'static [u8; 62] =
        b"target=\"_blank\">Cloudflare</a></div></div></div></body></html>";
    /// CF end second template.
    static ref CF_END2: &'static [u8; 72] =
        b"Performance &amp; security by Cloudflare</div></div></div></body></html>";
    /// CF head.
    static ref CF_HEAD: &'static [u8; 34] = b"<html><head>\n    <style global=\"\">";
    /// CF mock frame.
    static ref CF_MOCK_FRAME: &'static [u8; 137] = b"<iframe height=\"1\" width=\"1\" style=\"position: absolute; top: 0px; left: 0px; border: none; visibility: hidden;\"></iframe>\n\n</body></html>";
    /// Cf just a moment.
    static ref CF_JUST_A_MOMENT: &'static [u8] =
        b"<!DOCTYPE html><html lang=\"en-US\" dir=\"ltr\"><head><title>Just a moment...</title>";

    // Fast prefix-only matcher (scan only the first ~120 bytes).
    static ref CF_JUST_A_MOMENT_AC: aho_corasick::AhoCorasick = aho_corasick::AhoCorasickBuilder::new()
        .ascii_case_insensitive(true)
        .build([
            b"<title>Just a moment...</title>".as_slice(),
            b"Just a moment...".as_slice(),
        ])
        .expect("valid CF just-a-moment patterns");

    /// Embedded Turnstile widget matcher.
    ///
    /// Distinct from the wall-page detector above: these patterns
    /// recognise pages that aren't a full "Just a moment..." wall but
    /// still host a real Turnstile widget (CF-protected pages that
    /// render an embedded challenge, or managed-challenge variants).
    ///
    /// Patterns are intentionally narrow to avoid false positives on
    /// documentation pages that quote the markup:
    ///
    /// * `challenges.cloudflare.com/turnstile` — the official api.js
    ///   endpoint. Only loaded by sites that actually use Turnstile.
    /// * `<div class="cf-turnstile"` / `<div class='cf-turnstile` —
    ///   the canonical widget markup with HTML tag context, so a
    ///   string literal `"cf-turnstile"` inside a script body or a
    ///   JSON blob cannot trigger a match.
    static ref CF_EMBEDDED_TURNSTILE_AC: aho_corasick::AhoCorasick = aho_corasick::AhoCorasickBuilder::new()
        .ascii_case_insensitive(true)
        .match_kind(aho_corasick::MatchKind::LeftmostFirst)
        .build([
            b"challenges.cloudflare.com/turnstile".as_slice(),
            b"<div class=\"cf-turnstile".as_slice(),
            b"<div class='cf-turnstile".as_slice(),
        ])
        .expect("valid CF embedded turnstile patterns");
}

#[inline(always)]
/// Detect recaptcha.
pub fn detect_recaptcha(html: &[u8]) -> bool {
    RECAPTCHA_AC.is_match(html)
}

#[inline(always)]
/// Detect GeeTest.
pub fn detect_geetest(html: &[u8]) -> bool {
    GEETEST_AC.is_match(html)
}

#[inline(always)]
/// Detect lemin.
pub fn detect_lemin(html: &[u8]) -> bool {
    LEMIN_AC.is_match(html)
}

/// Looks like GeeTest.
#[inline(always)]
pub fn looks_like_geetest(html: &[u8]) -> bool {
    GEETEST_AC.is_match(html)
}

/// Looks like GeeTest loading.
#[inline(always)]
pub fn looks_like_geetest_loading(html: &[u8]) -> bool {
    GEETEST_LOADING_AC.is_match(html)
}

/// Geetest challenge visible.
#[inline(always)]
pub fn looks_like_geetest_challenge_visible(html: &[u8]) -> bool {
    GEETEST_VISIBLE_AC.is_match(html)
}

#[inline(always)]
/// Imperva challenge size
pub fn imperva_challenge_sized(len: usize) -> bool {
    len > 0 && len <= 220_000
}

#[inline(always)]
/// Looks like imperva wait screen.
pub fn looks_like_imperva_wait_screen(html: &[u8]) -> bool {
    imperva_challenge_sized(html.len()) && IMPERVA_WAIT_AC.is_match(html)
}

#[inline(always)]
/// Looks like imperva phase screen.
pub fn looks_like_imperva_iframe_phase(html: &[u8]) -> bool {
    imperva_challenge_sized(html.len()) && IMPERVA_IFRAME_PHASE_AC.is_match(html)
}

#[inline(always)]
#[cfg(all(feature = "chrome", feature = "real_browser"))]
/// Looks like hcaptcha iframe.
pub fn looks_like_hcaptcha_iframe(html: &[u8]) -> bool {
    imperva_challenge_sized(html.len()) && HCAPTCHA_IFRAME_AC.is_match(html)
}

#[inline(always)]
#[cfg(all(feature = "chrome", feature = "real_browser"))]
/// Looks like imperva.
pub fn looks_like_imperva_any(html: &[u8]) -> bool {
    looks_like_imperva_wait_screen(html)
        || looks_like_imperva_iframe_phase(html)
        || looks_like_hcaptcha_iframe(html)
}

#[cfg(feature = "chrome")]
#[inline]
/// Is turnstile page?
pub(crate) fn detect_cf_turnstyle(b: &[u8]) -> bool {
    if b.ends_with(CF_END.as_ref()) || b.ends_with(CF_END2.as_ref()) {
        return true;
    }

    if b.starts_with(CF_HEAD.as_ref()) && b.ends_with(CF_MOCK_FRAME.as_ref()) {
        return true;
    }

    let pfx = cf_prefix_slice(b);

    if pfx.starts_with(CF_JUST_A_MOMENT.as_ref()) || CF_JUST_A_MOMENT_AC.is_match(pfx) {
        return true;
    }

    // Embedded Turnstile widget on an otherwise-non-wall page. Scans the
    // whole body (not just the prefix) because the widget markup can
    // appear anywhere. Patterns are scoped to actual HTML tag context
    // and the official api.js endpoint so quoted code samples in docs
    // pages don't false-positive.
    detect_cf_embedded_turnstile(b)
}

/// Detect an embedded Cloudflare Turnstile widget anywhere in the body.
///
/// Distinct entry point so callers that only want the embedded-widget
/// signal (without the wall-page fingerprints) can use it directly. Pure
/// byte-level Aho-Corasick scan — no allocations, no mutexes, no panics.
#[cfg(feature = "chrome")]
#[inline]
pub(crate) fn detect_cf_embedded_turnstile(b: &[u8]) -> bool {
    !b.is_empty() && CF_EMBEDDED_TURNSTILE_AC.is_match(b)
}

lazy_static! {
    /// Open Resty forbidden.
    pub static ref OPEN_RESTY_FORBIDDEN: &'static [u8; 125] = br#"<html><head><title>403 Forbidden</title></head>
<body>
<center><h1>403 Forbidden</h1></center>
<hr><center>openresty</center>"#;


  /// Empty html.
  pub static ref EMPTY_HTML_BASIC: &'static [u8; 13] = b"<html></html>";
  /// The vision endpoint gemini.
  pub static ref GEMINI_VISION_ENDPOINT: String = {
    std::env::var("GEMINI_VISION_ENDPOINT").unwrap_or("https://generativelanguage.googleapis.com/v1beta/models/gemini-pro-vision".into())
  };
}

#[inline(always)]
/// Detect imperva verification iframe.
pub fn detect_imperva_verification_iframe(html: &[u8]) -> bool {
    AC_IMPERVA_IFRAME.is_match(html)
}

/// A combined “looks like Imperva verification page” check.
/// Use this before deciding that X-Cdn: Imperva implies Imperva.
#[inline(always)]
pub fn looks_like_imperva_verify(content_len: usize, html: &[u8]) -> bool {
    imperva_challenge_sized(content_len) && detect_imperva_verification_iframe(html)
}

/// Needs bot verification.
#[inline(always)]
pub fn contains_verification(text: &Vec<u8>) -> bool {
    AC.is_match(text)
}

/// Handle protected pages via chrome. This does nothing without the real_browser feature enabled.
///
/// LEGACY_DOM_HEURISTIC (SCORPION_CANONICAL_PRODUCTION_CAPTCHA_EXECUTION_CONVERGENCE_001):
/// no CAPTCHA provider is ever invoked here. This is DOM click/wait
/// automation for Cloudflare Turnstile's non-interactive/managed mode
/// (find the widget, click it, poll until it clears) — there is no
/// visual challenge to reason about, so it is not a task the canonical
/// `CaptchaProvider`/`CaptchaProviderRegistry` seam (`features/captcha.rs`,
/// `features/captcha_browser.rs`) applies to. Retained as-is; see
/// `docs/frontier/CANONICAL_PRODUCTION_CAPTCHA_EXECUTION_CONVERGENCE_SDD.md`
/// for the full classification. Guardrailed: never claims canonical
/// provider dispatch.
#[cfg(all(feature = "chrome", feature = "real_browser"))]
#[inline(always)]
pub async fn cf_handle(
    b: &mut Vec<u8>,
    page: &chromiumoxide::Page,
    target_url: &str,
    viewport: &Option<crate::configuration::Viewport>,
) -> Result<bool, chromiumoxide::error::CdpError> {
    let mut validated = false;

    let page_result = tokio::time::timeout(tokio::time::Duration::from_secs(30), async {
        let page_navigate = async {
            // force upgrade https check.
            if let Some(page_url) = page.url().await? {
                if page_url == "about:blank" {
                    let target_url = if target_url.starts_with("http://") {
                        let mut s = String::with_capacity(target_url.len() + 1);
                        s.push_str("https://");
                        s.push_str(&target_url["http://".len()..]);
                        s
                    } else {
                        target_url.to_string()
                    };
                    let _ = page.goto(target_url).await?.wait_for_navigation().await?;
                }
                else if page_url.starts_with("http://") {
                    let _ = page.goto(page_url.replacen("http://", "https://", 1)).await?;
                } else {
                    tokio::time::sleep(Duration::from_millis(3_500)).await;
                }
            }

            Ok::<(), chromiumoxide::error::CdpError>(())
        };

        // get the csp settings before hand
        let _ = tokio::join!(page.disable_network_cache(true), page_navigate, perform_smart_mouse_movement(page, viewport));

        for _ in 0..10 {
            let mut wait_for = CF_WAIT_FOR.clone();

            let mut clicks = 0usize;
            let mut hidden = false;

            if let Ok(els) = page
                .find_elements_pierced(
                    r#"
                div[id*="turnstile"],
                iframe[src*="challenges.cloudflare.com"],
                iframe[src*="turnstile"],
                iframe[title*="widget"],
                input[type="checkbox"]"#,
                )
                .await
            {
                perform_smart_mouse_movement(page, viewport).await;
                for el in els {
                    let f = async {
                        match el.clickable_point().await {
                            Ok(pt) => page.click_smooth(pt).await.is_ok() || el.click_smooth().await.is_ok(),
                            Err(_) => el.click_smooth().await.is_ok(),
                        }
                    };

                    let (did_click, _) =
                        tokio::join!(f, perform_smart_mouse_movement(page, viewport));

                    if did_click {
                        clicks += 1;
                    }
                }
            } else {

                hidden = true;
                let wait = Some(wait_for.clone());
                let _ = tokio::join!(
                    page_wait(page, &wait),
                    perform_smart_mouse_movement(page, viewport)
                );
            }

            if !hidden && clicks == 0 {
                let f = page.evaluate(
                    r#"document.querySelectorAll("iframe,input")?.forEach(el => el.click());document.querySelector('.cf-turnstile')?.click();"#,
                );
                let _ = tokio::join!(f, perform_smart_mouse_movement(page, viewport));
            }

            wait_for.page_navigations = true;
            let wait = Some(wait_for.clone());

            let _ = tokio::join!(
                page_wait(page, &wait),
                perform_smart_mouse_movement(page, viewport),
            );

            if let Ok(mut next_content) = page.outer_html_bytes().await {
                if !detect_cf_turnstyle(&next_content) {
                    validated = true;
                    wait_for.delay = crate::features::chrome_common::WaitForDelay::new(Some(
                        core::time::Duration::from_secs(4),
                    ))
                    .into();
                    page_wait(page, &Some(wait_for)).await;
                    if let Ok(nc) = page.outer_html_bytes().await {
                        next_content = nc;
                    }
                } else if contains_verification(&next_content) {
                    wait_for.delay = crate::features::chrome_common::WaitForDelay::new(Some(
                        core::time::Duration::from_millis(3500),
                    ))
                    .into();
                    page_wait(page, &Some(wait_for.clone())).await;

                    if let Ok(nc) = page.outer_html_bytes().await {
                        next_content = nc;
                    }
                    if !detect_cf_turnstyle(&next_content) {
                        validated = true;
                        page_wait(page, &Some(wait_for)).await;
                        if let Ok(nc) = page.outer_html_bytes().await {
                            next_content = nc;
                        }
                    }
                };

                *b = next_content;

                if validated {
                    break;
                }
            }
        }

        Ok::<(), chromiumoxide::error::CdpError>(())
    })
    .await;

    match page_result {
        Ok(_) => Ok(validated),
        _ => Err(chromiumoxide::error::CdpError::Timeout),
    }
}

/// Handle imperva protected pages via chrome. This does nothing without the real_browser feature enabled.
///
/// LEGACY_DOM_HEURISTIC (SCORPION_CANONICAL_PRODUCTION_CAPTCHA_EXECUTION_CONVERGENCE_001):
/// no CAPTCHA provider is ever invoked here, in any branch (hCaptcha
/// checkbox click, native slider drag-to-edge, or generic interstitial
/// wait). None of these require visual reasoning about challenge
/// content, so none are a task the canonical `CaptchaProvider`/
/// `CaptchaProviderRegistry` seam applies to. Retained as-is; see
/// `docs/frontier/CANONICAL_PRODUCTION_CAPTCHA_EXECUTION_CONVERGENCE_SDD.md`.
/// Guardrailed: never claims canonical provider dispatch.
#[cfg(all(feature = "chrome", feature = "real_browser"))]
#[inline(always)]
pub async fn imperva_handle(
    b: &mut Vec<u8>,
    page: &chromiumoxide::Page,
    _target_url: &str,
    viewport: &Option<crate::configuration::Viewport>,
) -> Result<bool, chromiumoxide::error::CdpError> {
    // -----------------------------------------------------------------
    // Fast‑path – bail out early if the response does not look like an
    // Imperva challenge at all.
    // -----------------------------------------------------------------
    if !looks_like_imperva_any(b.as_slice()) {
        return Ok(false);
    }

    // -----------------------------------------------------------------
    // Drag‑helpers (unchanged)
    // -----------------------------------------------------------------
    #[inline(always)]
    fn pt(x: f64, y: f64) -> chromiumoxide::layout::Point {
        chromiumoxide::layout::Point { x, y }
    }

    #[inline(always)]
    fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
        if v < lo {
            lo
        } else if v > hi {
            hi
        } else {
            v
        }
    }

    /// Build js drag handler.
    #[inline(always)]
    fn build_js_drag(fx: f64, fy: f64, tx: f64, ty: f64) -> String {
        use core::fmt::Write as _;
        let mut s = String::with_capacity(1024);
        let _ = write!(
            &mut s,
            r#"(function(){{const fx={:.3},fy={:.3},tx={:.3},ty={:.3};
const at=(x,y)=>document.elementFromPoint(x,y);
const fire=(el,type,x,y)=>{{if(!el)return;const o={{bubbles:true,cancelable:true,clientX:x,clientY:y,buttons:1}};el.dispatchEvent(new MouseEvent(type,o));try{{const p=type==='mousedown'?'pointerdown':type==='mousemove'?'pointermove':type==='mouseup'?'pointerup':type;el.dispatchEvent(new PointerEvent(p,{{bubbles:true,cancelable:true,clientX:x,clientY:y,buttons:1,pointerId:1,isPrimary:true}}));}}catch(e){{}}}};
const el0=at(fx,fy);fire(el0,'mousedown',fx,fy);
for(let i=1;i<=18;i++){{const t=i/18,x=fx+(tx-fx)*t,y=fy+(ty-fy)*t;fire(at(x,y)||el0,'mousemove',x,y);}}
fire(at(tx,ty)||el0,'mouseup',tx,ty);return true;}})()"#,
            fx, fy, tx, ty
        );
        s
    }

    // -----------------------------------------------------------------
    // Main solving loop (unchanged apart from the matcher calls)
    // -----------------------------------------------------------------
    let mut validated = false;

    let page_result = tokio::time::timeout(tokio::time::Duration::from_secs(30), async {
        // Disable cache + a little mouse‑movement jitter.
        let _ = tokio::join!(
            page.disable_network_cache(true),
            perform_smart_mouse_movement(page, viewport)
        );

        for _ in 0..10 {
            let mut wait_for = CF_WAIT_FOR.clone();

            // ---------------------------------------------------------
            // Pull HTML once per iteration.
            // ---------------------------------------------------------
            let cur_html = match page.outer_html_bytes().await {
                Ok(h) => h,
                Err(_) => {
                    let wait = Some(wait_for.clone());
                    let _ = tokio::join!(
                        page_wait(page, &wait),
                        perform_smart_mouse_movement(page, viewport),
                    );
                    continue;
                }
            };
            *b = cur_html;

            // ---------------------------------------------------------
            // If we have left the challenge, we are done.
            // ---------------------------------------------------------
            if !looks_like_imperva_any(b.as_slice()) {
                validated = true;
                break;
            }

            // ---------------------------------------------------------
            // 0️⃣  hCaptcha checkbox flow (used inside Imperva pages)
            // ---------------------------------------------------------
            let hcaptcha_iframe_present = page
                .find_elements_pierced(
                    r#"iframe[src*="hcaptcha.com"], iframe[src*="newassets.hcaptcha.com"]"#,
                )
                .await
                .map(|els| !els.is_empty())
                .unwrap_or(false);

            if hcaptcha_iframe_present || looks_like_hcaptcha_iframe(b.as_slice()) {
                if let Ok(boxes) = page.find_elements_pierced(r#"#checkbox"#).await {
                    if let Some(cb_el) = boxes.into_iter().next() {
                        // Click the checkbox – prefer the clickable point.
                        let clicked = match cb_el.clickable_point().await {
                            Ok(p) => {
                                page.click_smooth(p).await.is_ok()
                                    || cb_el.click_smooth().await.is_ok()
                            }
                            Err(_) => cb_el.click_smooth().await.is_ok(),
                        };

                        if clicked {
                            // Give the page a moment to render/transition.
                            wait_for.delay = crate::features::chrome_common::WaitForDelay::new(
                                Some(core::time::Duration::from_millis(900)),
                            )
                            .into();
                            wait_for.idle_network =
                                crate::features::chrome_common::WaitForIdleNetwork::new(
                                    core::time::Duration::from_secs(6).into(),
                                )
                                .into();
                            wait_for.page_navigations = true;

                            let wait = Some(wait_for.clone());
                            let _ = tokio::join!(
                                page_wait(page, &wait),
                                perform_smart_mouse_movement(page, viewport),
                            );

                            if let Ok(nc) = page.outer_html_bytes().await {
                                *b = nc;
                                if !looks_like_imperva_any(b.as_slice()) {
                                    validated = true;
                                    break;
                                }
                            }
                        } else {
                            // Click failed – wait a bit and retry.
                            wait_for.delay = crate::features::chrome_common::WaitForDelay::new(
                                Some(core::time::Duration::from_millis(650)),
                            )
                            .into();
                            let wait = Some(wait_for.clone());
                            let _ = tokio::join!(
                                page_wait(page, &wait),
                                perform_smart_mouse_movement(page, viewport),
                            );
                        }

                        // Continue the outer loop – we may now be in the slider phase.
                        continue;
                    }
                }

                // No checkbox yet – behave like Cloudflare: just wait for the iframe to load.
                wait_for.delay = crate::features::chrome_common::WaitForDelay::new(Some(
                    core::time::Duration::from_millis(900),
                ))
                .into();
                wait_for.idle_network = crate::features::chrome_common::WaitForIdleNetwork::new(
                    core::time::Duration::from_secs(6).into(),
                )
                .into();
                wait_for.page_navigations = true;

                let wait = Some(wait_for.clone());
                let _ = tokio::join!(
                    page_wait(page, &wait),
                    perform_smart_mouse_movement(page, viewport),
                );
                if let Ok(nc) = page.outer_html_bytes().await {
                    *b = nc;
                    if !looks_like_imperva_any(b.as_slice()) {
                        validated = true;
                        break;
                    }
                }
                continue;
            }

            // ---------------------------------------------------------
            // 1️⃣  WAIT SCREEN – just a static “please wait” page.
            // ---------------------------------------------------------
            if looks_like_imperva_wait_screen(b.as_slice()) {
                wait_for.delay = crate::features::chrome_common::WaitForDelay::new(Some(
                    core::time::Duration::from_millis(1_100),
                ))
                .into();
                wait_for.idle_network = crate::features::chrome_common::WaitForIdleNetwork::new(
                    core::time::Duration::from_secs(7).into(),
                )
                .into();
                wait_for.page_navigations = true;

                let wait = Some(wait_for.clone());
                let _ = tokio::join!(
                    page_wait(page, &wait),
                    perform_smart_mouse_movement(page, viewport),
                );

                if let Ok(nc) = page.outer_html_bytes().await {
                    *b = nc;
                }
                continue;
            }

            // ---------------------------------------------------------
            // 2️⃣  Imperva iframe / slider phase.
            // ---------------------------------------------------------
            let verify_iframe_present = page
                .find_elements_pierced(
                    r#"
                    iframe[src*="geo.captcha-delivery.com"],
                    iframe[src*="captcha-delivery.com"],
                    iframe[title*="Verification system"],
                    iframe[title*="verification system"]
                    "#,
                )
                .await
                .map(|els| !els.is_empty())
                .unwrap_or(false);

            if verify_iframe_present || looks_like_imperva_iframe_phase(b.as_slice()) {
                let mut did_drag = false;

                // -----------------------------------------------------------------
                // Try to locate a native slider handle first.
                // -----------------------------------------------------------------
                if let Ok(handles) = page
                    .find_elements_pierced(
                        r#"
                        .slider,
                        [class*="sliderHandle"],
                        [class*="slider-handle"],
                        [class*="slider"]
                        "#,
                    )
                    .await
                {
                    if let Some(handle) = handles.into_iter().next() {
                        if let (Ok(hb), Ok(conts)) = (
                            handle.bounding_box().await,
                            page.find_elements_pierced(
                                r#"
                                .sliderContainer,
                                [class*="sliderContainer"],
                                .slider-container,
                                [class*="slider-container"]
                                "#,
                            )
                            .await,
                        ) {
                            if let Some(container) = conts.into_iter().next() {
                                if let Ok(cb) = container.bounding_box().await {
                                    let from = pt(hb.x + hb.width * 0.5, hb.y + hb.height * 0.5);
                                    let to_x = cb.x + cb.width - 8.0;
                                    let to_y = cb.y + cb.height * 0.5;
                                    let to = pt(
                                        clamp(to_x, cb.x + 2.0, cb.x + cb.width - 2.0),
                                        clamp(to_y, cb.y + 2.0, cb.y + cb.height - 2.0),
                                    );

                                    // Small mouse‑move jitter before the drag.
                                    let _ = tokio::join!(
                                        perform_smart_mouse_movement(page, viewport),
                                        async {
                                            let _ = page.move_mouse_smooth(from).await;
                                        }
                                    );

                                    if page.click_and_drag_smooth(from, to).await.is_ok() {
                                        did_drag = true;
                                    }
                                }
                            }
                        }
                    }
                }

                // -----------------------------------------------------------------
                // Fallback – build a JS drag that uses the container bbox.
                // -----------------------------------------------------------------
                if !did_drag {
                    if let Ok(conts) = page
                        .find_elements_pierced(
                            r#"
                            .sliderContainer,
                            [class*="sliderContainer"],
                            .slider-container,
                            [class*="slider-container"]
                            "#,
                        )
                        .await
                    {
                        if let Some(container) = conts.into_iter().next() {
                            if let Ok(cb) = container.bounding_box().await {
                                let from_x = clamp(cb.x + 10.0, cb.x + 2.0, cb.x + cb.width - 2.0);
                                let from_y = clamp(
                                    cb.y + cb.height * 0.5,
                                    cb.y + 2.0,
                                    cb.y + cb.height - 2.0,
                                );
                                let to_x = clamp(
                                    cb.x + cb.width - 10.0,
                                    cb.x + 2.0,
                                    cb.x + cb.width - 2.0,
                                );
                                let to_y = from_y;

                                let js = build_js_drag(from_x, from_y, to_x, to_y);
                                let _ = page.evaluate(js).await;
                                did_drag = true;
                            }
                        }
                    }
                }

                // -----------------------------------------------------------------
                // Wait a little after the drag (or after the JS fallback).
                // -----------------------------------------------------------------
                if did_drag {
                    wait_for.delay = crate::features::chrome_common::WaitForDelay::new(Some(
                        core::time::Duration::from_millis(900),
                    ))
                    .into();
                    wait_for.idle_network =
                        crate::features::chrome_common::WaitForIdleNetwork::new(
                            core::time::Duration::from_secs(6).into(),
                        )
                        .into();
                    wait_for.page_navigations = true;
                } else {
                    wait_for.delay = crate::features::chrome_common::WaitForDelay::new(Some(
                        core::time::Duration::from_millis(650),
                    ))
                    .into();
                    wait_for.page_navigations = true;
                }

                let wait = Some(wait_for.clone());
                let _ = tokio::join!(
                    page_wait(page, &wait),
                    perform_smart_mouse_movement(page, viewport),
                );

                if let Ok(nc) = page.outer_html_bytes().await {
                    *b = nc;
                    if !looks_like_imperva_any(b.as_slice()) {
                        validated = true;
                        break;
                    }
                }

                continue;
            }

            // ---------------------------------------------------------
            // 3️⃣  Unknown interstitial – do a generic CF‑style wait and retry.
            // ---------------------------------------------------------
            wait_for.delay = crate::features::chrome_common::WaitForDelay::new(Some(
                core::time::Duration::from_millis(900),
            ))
            .into();
            wait_for.idle_network = crate::features::chrome_common::WaitForIdleNetwork::new(
                core::time::Duration::from_secs(6).into(),
            )
            .into();
            wait_for.page_navigations = true;

            let wait = Some(wait_for.clone());
            let _ = tokio::join!(
                page_wait(page, &wait),
                perform_smart_mouse_movement(page, viewport),
            );

            if let Ok(nc) = page.outer_html_bytes().await {
                *b = nc;
                if !looks_like_imperva_any(b.as_slice()) {
                    validated = true;
                    break;
                }
            }
        }

        Ok::<(), chromiumoxide::error::CdpError>(())
    })
    .await;

    match page_result {
        Ok(_) => Ok(validated),
        _ => Err(chromiumoxide::error::CdpError::Timeout),
    }
}

#[cfg(all(feature = "chrome", feature = "real_browser"))]
/// Returns the `data:image/...;base64,…` string for the `<img>` whose
/// `src` attribute equals `src`.  The image is already loaded in the
/// page, so this does **not** trigger a network request – it draws the
/// image onto a temporary canvas and reads the data‑URL from that canvas.
async fn extract_image_dataurl(page: &chromiumoxide::Page, src: &str) -> Result<String, CdpError> {
    // JavaScript that receives the exact `src` value, finds the <img>,
    // draws it onto a canvas, and returns `canvas.toDataURL()`.
    let js = format!(
        r#"(function(){{
            const img = document.querySelector('img[src="{src}"]');
            if (!img) return null;
            const canvas = document.createElement('canvas');
            canvas.width = img.naturalWidth || img.width;
            canvas.height = img.naturalHeight || img.height;
            const ctx = canvas.getContext('2d');
            ctx.drawImage(img, 0, 0);
            return canvas.toDataURL();
        }})()"#
    );

    // `page.evaluate` returns an `EvaluationResult`.
    let eval = page.evaluate(js).await?;
    let dataurl = eval
        .value()
        .and_then(|v| v.as_str().map(|s| s.to_owned()))
        .ok_or_else(|| CdpError::msg("failed to extract tile data‑url"))?;

    Ok(dataurl)
}

/// High‑level wrapper – first tries the in‑page Gemini helper,
/// falls back to the external Gemini HTTP call when the helper is missing.
#[cfg(all(feature = "chrome", feature = "real_browser"))]
pub async fn solve_enterprise_with_browser_gemini(
    page: &chromiumoxide::Page,
    challenge: &RcEnterpriseChallenge<'_>,
    timeout_ms: u64,
) -> Result<Vec<u8>, CdpError> {
    let mut visuals = Vec::with_capacity(challenge.tiles.len());

    for tile in &challenge.tiles {
        let dataurl = extract_image_dataurl(page, tile.img_src).await?;
        visuals.push(
            visual_from_data_url(Some(tile.id.to_string()), &dataurl)
                .map_err(|_| CdpError::msg("invalid reCAPTCHA tile image"))?,
        );
    }

    let target = challenge.target.unwrap_or("target object").to_string();
    let normalized = CaptchaChallenge {
        kind: CaptchaChallengeKind::ImageGridSelection,
        instruction: target.clone(),
        visuals,
    };
    let local_request = CaptchaSolveRequest {
        correlation_id: "recaptcha-enterprise-grid".into(),
        selected_provider: CaptchaProviderId::LOCAL_LANGUAGE_MODEL,
        challenge: normalized,
        deadline: Duration::from_millis(timeout_ms),
    };
    let local_provider = LocalLanguageModelProvider { page };
    let mut registry = CaptchaProviderRegistry::new();
    registry
        .register(&local_provider)
        .expect("local provider identity is unique");
    let mut route = CaptchaRouteAttempts::new();
    match route
        .execute_explicit_attempt(&registry, &local_request)
        .await
    {
        CaptchaSolveOutcome::Solved {
            solution: CaptchaSolution::SelectedChoices(ids),
            ..
        } => Ok(ids.iter().filter_map(|id| id.parse::<u8>().ok()).collect()),
        CaptchaSolveOutcome::Failed {
            failure: CaptchaSolveFailure::ProviderUnavailable,
            ..
        } => {
            let api_key = match std::env::var("GEMINI_API_KEY") {
                Ok(api_key) => api_key,
                // Capability unavailable — never presented as a truthful
                // empty selection. `CaptchaSolution::SelectedChoices(vec![])`
                // is a legitimate answer only when a provider actually
                // examined the tiles and concluded none apply (see
                // `captcha.rs`'s own contract); no provider ran here at
                // all, so returning `Ok(Vec::new())` would fabricate that
                // claim. Fail closed instead.
                Err(_) => {
                    return Err(CdpError::msg(
                        "recaptcha enterprise grid: local CAPTCHA provider unavailable and \
                         GEMINI_API_KEY not set",
                    ))
                }
            };
            let permits = challenge
                .tiles
                .len()
                .min(*crate::utils::GEMINI_SEM_PERMITS)
                .max(1) as u32;
            let _permit = crate::utils::GEMINI_SEM
                .acquire_many(permits)
                .await
                .map_err(|_| CdpError::msg("Gemini solver admission cancelled"))?;
            let remote = CaptchaChallenge {
                kind: CaptchaChallengeKind::ImageGridSelection,
                instruction: target,
                visuals: challenge
                    .tiles
                    .iter()
                    .map(|tile| {
                        url::Url::parse(tile.img_src)
                            .map(|url| CaptchaVisualInput::RemoteAsset {
                                id: Some(tile.id.to_string()),
                                media_type: "image/jpeg".into(),
                                url,
                            })
                            .map_err(|_| CdpError::msg("invalid reCAPTCHA tile URL"))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            };
            let remote = materialize_remote_challenge(remote)
                .await
                .map_err(|failure| CdpError::msg(format!("CAPTCHA asset failure: {failure:?}")))?;
            let external_request = CaptchaSolveRequest {
                correlation_id: "recaptcha-enterprise-grid-external".into(),
                selected_provider: CaptchaProviderId::EXTERNAL_GEMINI,
                challenge: remote,
                deadline: Duration::from_millis(timeout_ms),
            };
            let external_provider = ExternalGeminiProvider { api_key: &api_key };
            registry
                .register(&external_provider)
                .expect("external provider identity is unique");
            match route
                .execute_explicit_attempt(&registry, &external_request)
                .await
            {
                CaptchaSolveOutcome::Solved {
                    solution: CaptchaSolution::SelectedChoices(ids),
                    ..
                } => Ok(ids.iter().filter_map(|id| id.parse::<u8>().ok()).collect()),
                _ => Err(route_error(&route)),
            }
        }
        _ => Err(route_error(&route)),
    }
}

/// In‑page Gemini helper – receives tiles that already contain a
/// `dataurl` field (the image as a `data:image/...;base64,…` string).
#[cfg(all(feature = "chrome", feature = "real_browser"))]
async fn solve_with_inpage_helper(
    page: &chromiumoxide::Page,
    tiles_json: &[serde_json::Value],
    target: &str,
    timeout_ms: u64,
) -> Result<Vec<u8>, CdpError> {
    let script = format!(
        r#"
        async function solveRecaptchaEnterpriseWithGemini(tiles, target) {{
            const session = await LanguageModel.create({{
                expectedInputs: [
                    {{ type: "text", languages: ["en"] }},
                    {{ type: "image" }},
                ],
                expectedOutputs: [{{ type: "text", languages: ["en"] }}],
            }});
            const yesIds = [];
            for (const tile of tiles) {{
                const resp = await fetch(tile.dataurl);
                if (!resp.ok) continue;
                const blob = await resp.blob();
                const prompt = [{{
                    role: "user",
                    content: [
                        {{ type: "text", value: `Does this image contain a ${{
                            target
                        }}? Answer only with "yes" or "no".` }},
                        {{ type: "image", value: blob }},
                    ],
                }}];
                const answer = await session.prompt(prompt);
                const txt = (answer || "").toString().trim().toLowerCase();
                if (txt.includes("yes")) yesIds.push(tile.id);
            }}
            return yesIds;
        }}

        (async () => {{
            const result = await solveRecaptchaEnterpriseWithGemini(
                {tiles},
                {target}
            );
            return result;
        }})()
        "#,
        tiles = serde_json::to_string(tiles_json).unwrap_or_default(),
        target = serde_json::to_string(target).unwrap_or_default(),
    );

    // -----------------------------------------------------------------
    // Ask Chrome to evaluate the script (same timeout logic as before).
    // -----------------------------------------------------------------
    let params = EvaluateParams::builder()
        .expression(&script)
        .await_promise(true)
        .build()
        .map_err(|e| CdpError::msg(format!("evaluate params: {e}")))?;

    let eval_fut = page.evaluate(params);
    let eval_res = tokio::time::timeout(Duration::from_millis(timeout_ms + 5_000), eval_fut)
        .await
        .map_err(|_| CdpError::Timeout)?;

    match eval_res {
        Ok(eval) => match eval.value() {
            Some(serde_json::Value::Array(arr)) => {
                let ids = arr
                    .iter()
                    .filter_map(|v| v.as_u64().map(|n| n as u8))
                    .collect();
                Ok(ids)
            }
            _ => Ok(vec![]),
        },
        Err(e) => Err(e),
    }
}

#[cfg(all(feature = "chrome", feature = "real_browser"))]
/// Is the language model missing.
fn is_missing_helper_error(err: &CdpError) -> bool {
    let txt = format!("{err}");
    txt.contains("LanguageModel is not defined")
        || txt.contains("ReferenceError")
        || txt.contains("Uncaught ReferenceError")
        || txt.contains("cannot read property 'create' of undefined")
}

#[cfg(all(feature = "chrome", feature = "real_browser"))]
/// Extract gemini fallback.
#[cfg(all(feature = "chrome", feature = "real_browser"))]
pub async fn warm_gemini_model(page: &Page) -> Result<(), CdpError> {
    let eval_params = EvaluateParams::builder()
        .expression(r#"(async()=>{try{const s=await LanguageModel.create({expectedInputs:[{type:"text",languages:["en"]}],expectedOutputs:[{type:"text",languages:["en"]}]});await s.prompt([{role:"user",content:[{type:"text",value:"ping"}]}])}catch(_){}})()"#)
        .await_promise(true)
        .build()
        .map_err(|e| CdpError::msg(format!("evaluate params: {e}")))?;

    tokio::time::timeout(Duration::from_secs(60), page.evaluate(eval_params))
        .await
        .map_err(|_| CdpError::Timeout)??;

    Ok(())
}

/// Handle reCAPTCHA checkbox (anchor iframe) via chrome.
/// This does nothing without the real_browser feature enabled.
#[cfg(all(feature = "chrome", feature = "real_browser"))]
/// CANONICAL_PROVIDER_DISPATCH_LEGACY_BINDING
/// (SCORPION_CANONICAL_PRODUCTION_CAPTCHA_EXECUTION_CONVERGENCE_001): the
/// enterprise-grid solve step (`solve_enterprise_with_browser_gemini`)
/// already routes through the canonical `CaptchaProviderRegistry`/
/// `CaptchaRouteAttempts::execute_explicit_attempt` (real capability
/// prevalidation, no silent fallback). What is *not* yet canonical is the
/// image capture and click/action application immediately around it —
/// tile bytes come from an in-page `<canvas>` draw, and solved tile IDs
/// are applied via direct DOM `click_smooth()`, not the canonical
/// snapshot-capture/execution-seam pair (`captcha_browser.rs`).
/// That migration is deliberately deferred — see
/// `docs/frontier/CANONICAL_PRODUCTION_CAPTCHA_EXECUTION_CONVERGENCE_SDD.md`
/// for why. Guardrailed: the provider-dispatch call remains present and
/// unmodified.
#[inline(always)]
pub async fn recaptcha_handle(
    b: &mut Vec<u8>,
    page: &chromiumoxide::Page,
    viewport: &Option<crate::configuration::Viewport>,
) -> Result<bool, CdpError> {
    if !detect_recaptcha(b.as_slice()) {
        return Ok(false);
    }

    let mut validated = false;

    let overall = tokio::time::timeout(Duration::from_secs(30), async {
        // Keep the mouse moving a little – helps not being flagged as a bot.
        let _ = tokio::join!(
            page.disable_network_cache(true),
            perform_smart_mouse_movement(page, viewport)
        );

        for _ in 0..10 {
            // ---------------------------------------------------------
            // a) Refresh HTML into the caller’s buffer.
            // ---------------------------------------------------------
            if let Ok(cur) = page.outer_html_bytes().await {
                *b = cur;
            }

            // ---------------------------------------------------------
            // b) If Recaptcha vanished → success.
            // ---------------------------------------------------------
            if !detect_recaptcha(b.as_slice()) {
                validated = true;
                break;
            }

            // ---------------------------------------------------------
            // c) **Enterprise** handling – now solved with the built‑in Gemini.
            // ---------------------------------------------------------
            if extract_rc_enterprise_challenge(b.as_slice()).is_some() {
                // 1️⃣  Ensure the anchor iframe exists (first click).
                let anchor_present = page
                    .find_elements_pierced(r#"iframe[src*="/recaptcha/api2/anchor"]"#)
                    .await
                    .map(|els| !els.is_empty())
                    .unwrap_or(false);

                if !anchor_present {
                    // Wait for it to appear – same CF‑style wait.
                    let mut wait_for = CF_WAIT_FOR.clone();
                    wait_for.delay = crate::features::chrome_common::WaitForDelay::new(Some(
                        Duration::from_millis(900),
                    ))
                    .into();
                    wait_for.idle_network =
                        crate::features::chrome_common::WaitForIdleNetwork::new(
                            Duration::from_secs(6).into(),
                        )
                        .into();
                    wait_for.page_navigations = true;
                    let wait = Some(wait_for.clone());
                    let _ = tokio::join!(
                        page_wait(page, &wait),
                        perform_smart_mouse_movement(page, viewport),
                    );
                    continue; // retry outer loop
                }

                // 2️⃣  Click the classic checkbox (same logic as before).
                async fn click_anchor(page: &chromiumoxide::Page) -> bool {
                    if let Ok(els) = page.find_elements_pierced(r#"#recaptcha-anchor"#).await {
                        if let Some(el) = els.into_iter().next() {
                            return match el.clickable_point().await {
                                Ok(p) => {
                                    page.click_smooth(p).await.is_ok()
                                        || el.click_smooth().await.is_ok()
                                }
                                Err(_) => el.click_smooth().await.is_ok(),
                            };
                        }
                    }
                    if let Ok(els) = page
                        .find_elements_pierced(r#".recaptcha-checkbox-checkmark"#)
                        .await
                    {
                        if let Some(el) = els.into_iter().next() {
                            return match el.clickable_point().await {
                                Ok(p) => {
                                    page.click_smooth(p).await.is_ok()
                                        || el.click_smooth().await.is_ok()
                                }
                                Err(_) => el.click_smooth().await.is_ok(),
                            };
                        }
                    }
                    false
                }

                let clicked = click_anchor(page).await;

                // 3️⃣  Wait a bit for the grid iframe to load.
                let mut wait_for = CF_WAIT_FOR.clone();
                wait_for.delay =
                    crate::features::chrome_common::WaitForDelay::new(Some(if clicked {
                        Duration::from_millis(1_100)
                    } else {
                        Duration::from_millis(700)
                    }))
                    .into();
                wait_for.idle_network = crate::features::chrome_common::WaitForIdleNetwork::new(
                    Duration::from_secs(7).into(),
                )
                .into();
                wait_for.page_navigations = true;
                let wait = Some(wait_for.clone());
                let _ = tokio::join!(
                    page_wait(page, &wait),
                    perform_smart_mouse_movement(page, viewport),
                );

                // ---------------------------------------------------------
                // d) Grab the grid HTML again – we need the *latest* tile URLs.
                // ---------------------------------------------------------
                let grid_html = match page.outer_html_bytes().await {
                    Ok(h) => h,
                    Err(_) => continue,
                };
                *b = grid_html.clone();

                // If the grid disappeared after the click, we’re done.
                if !detect_recaptcha(b.as_slice()) {
                    validated = true;
                    break;
                }

                // Extract the challenge *again* (now we are sure the grid is present).
                let challenge = match extract_rc_enterprise_challenge(&grid_html) {
                    Some(c) => c,
                    None => continue,
                };

                // ---------------------------------------------------------
                // e) **Solve with the built‑in Gemini** (the function above).
                // ---------------------------------------------------------
                let yes_ids = solve_enterprise_with_browser_gemini(page, &challenge, 20_000)
                    .await
                    .map_err(|e| {
                        CdpError::ChromeMessage(format!("gemini in‑page failed: {}", e))
                    })?;

                // ---------------------------------------------------------
                // f) Click every tile that received a “yes”.
                // ---------------------------------------------------------
                for id in yes_ids {
                    if let Some(tile) = challenge.tiles.iter().find(|t| t.id == id) {
                        // Build a selector that matches the exact `<img src="…">`.
                        let selector = format!(r#"img[src="{}"]"#, tile.img_src);
                        if let Ok(els) = page.find_elements_pierced(&selector).await {
                            if let Some(el) = els.into_iter().next() {
                                let _ = el.click_smooth().await; // ignore possible errors
                            }
                        }
                    }
                }

                // ---------------------------------------------------------
                // g) Click the Verify button if it exists.
                // ---------------------------------------------------------
                if challenge.has_verify_button {
                    if let Ok(btns) = page
                        .find_elements_pierced(
                            r#"button[id*="recaptcha-verify-button"], button:contains("Verify")"#,
                        )
                        .await
                    {
                        if let Some(btn) = btns.into_iter().next() {
                            let _ = btn.click_smooth().await;
                        }
                    }
                }

                // ---------------------------------------------------------
                // h) Final wait for navigation / network idle.
                // ---------------------------------------------------------
                let mut wait_for = CF_WAIT_FOR.clone();
                wait_for.delay = crate::features::chrome_common::WaitForDelay::new(Some(
                    Duration::from_millis(1_500),
                ))
                .into();
                wait_for.idle_network = crate::features::chrome_common::WaitForIdleNetwork::new(
                    Duration::from_secs(8).into(),
                )
                .into();
                wait_for.page_navigations = true;
                let wait = Some(wait_for.clone());
                let _ = tokio::join!(
                    page_wait(page, &wait),
                    perform_smart_mouse_movement(page, viewport),
                );

                // ---------------------------------------------------------
                // i) Refresh HTML one last time – if the whole Recaptcha is gone we’re finished.
                // ---------------------------------------------------------
                if let Ok(new_html) = page.outer_html_bytes().await {
                    *b = new_html;
                    if !detect_recaptcha(b.as_slice()) {
                        validated = true;
                        break;
                    }
                }

                // If we are still here the grid is still present – loop again (maybe a slider appears).
                continue;
            }

            let anchor_iframe_present = page
                .find_elements_pierced(r#"iframe[src*="/recaptcha/api2/anchor"]"#)
                .await
                .map(|els| !els.is_empty())
                .unwrap_or(false);

            if !anchor_iframe_present {
                let mut wait_for = CF_WAIT_FOR.clone();
                wait_for.delay = crate::features::chrome_common::WaitForDelay::new(Some(
                    Duration::from_millis(900),
                ))
                .into();
                wait_for.idle_network = crate::features::chrome_common::WaitForIdleNetwork::new(
                    Duration::from_secs(6).into(),
                )
                .into();
                wait_for.page_navigations = true;
                let wait = Some(wait_for.clone());
                let _ = tokio::join!(
                    page_wait(page, &wait),
                    perform_smart_mouse_movement(page, viewport),
                );
                continue;
            }

            // Click the classic checkbox (same logic you already had)
            let mut clicked = false;
            if let Ok(els) = page.find_elements_pierced(r#"#recaptcha-anchor"#).await {
                if let Some(el) = els.into_iter().next() {
                    clicked = match el.clickable_point().await {
                        Ok(p) => {
                            page.click_smooth(p).await.is_ok() || el.click_smooth().await.is_ok()
                        }
                        Err(_) => el.click_smooth().await.is_ok(),
                    };
                }
            }
            if !clicked {
                if let Ok(els) = page
                    .find_elements_pierced(r#".recaptcha-checkbox-checkmark"#)
                    .await
                {
                    if let Some(el) = els.into_iter().next() {
                        clicked = match el.clickable_point().await {
                            Ok(p) => {
                                page.click_smooth(p).await.is_ok()
                                    || el.click_smooth().await.is_ok()
                            }
                            Err(_) => el.click_smooth().await.is_ok(),
                        };
                    }
                }
            }

            let mut wait_for = CF_WAIT_FOR.clone();
            wait_for.delay = crate::features::chrome_common::WaitForDelay::new(Some(if clicked {
                Duration::from_millis(1_100)
            } else {
                Duration::from_millis(700)
            }))
            .into();
            wait_for.idle_network = crate::features::chrome_common::WaitForIdleNetwork::new(
                Duration::from_secs(7).into(),
            )
            .into();
            wait_for.page_navigations = true;
            let wait = Some(wait_for.clone());
            let _ = tokio::join!(
                page_wait(page, &wait),
                perform_smart_mouse_movement(page, viewport),
            );

            if let Ok(new_html) = page.outer_html_bytes().await {
                *b = new_html;
                if !detect_recaptcha(b.as_slice()) {
                    validated = true;
                    break;
                }
            }
        }

        Ok::<(), CdpError>(())
    })
    .await;

    match overall {
        Ok(_) => Ok(validated),
        Err(_) => Err(CdpError::Timeout),
    }
}
#[cfg(all(feature = "chrome", feature = "real_browser"))]
/// Upstream-compatible external Gemini entrypoint. Canonical callers use the
/// neutral provider seam; this wrapper preserves the historical tuple shape.
pub async fn solve_lemin_with_external_gemini(image_dataurl: &str, timeout_ms: u64) -> (f64, f64) {
    let api_key = match std::env::var("GEMINI_API_KEY") {
        Ok(api_key) => api_key,
        Err(_) => return (0.0, 0.0),
    };
    let visual = match visual_from_data_url(None, image_dataurl) {
        Ok(visual) => visual,
        Err(_) => return (0.0, 0.0),
    };
    let request = CaptchaSolveRequest {
        correlation_id: "lemin-point-external-compatibility".into(),
        selected_provider: CaptchaProviderId::EXTERNAL_GEMINI,
        challenge: CaptchaChallenge {
            kind: CaptchaChallengeKind::PointSelection,
            instruction: "Give me the centre (x and y coordinates) of the missing puzzle piece in this image.".into(),
            visuals: vec![visual],
        },
        deadline: Duration::from_millis(timeout_ms / 2),
    };
    let _permit = match crate::utils::GEMINI_SEM.acquire().await {
        Ok(permit) => permit,
        Err(_) => return (0.0, 0.0),
    };
    match solve_captcha(&ExternalGeminiProvider { api_key: &api_key }, &request).await {
        CaptchaSolveOutcome::Solved {
            solution: CaptchaSolution::Point { x, y },
            ..
        } => (x, y),
        _ => (0.0, 0.0),
    }
}

#[cfg(all(feature = "chrome", feature = "real_browser"))]
async fn solve_lemin_with_inpage_helper(
    page: &Page,
    image_dataurl: &str,
    timeout_ms: u64,
) -> Result<(f64, f64), CdpError> {
    let script = format!(
        r#"(async () => {{
            try {{
                const session = await LanguageModel.create({{
                    expectedInputs: [
                        {{ type: "text", languages: ["en"] }},
                        {{ type: "image" }},
                    ],
                    expectedOutputs: [{{ type: "text", languages: ["en"] }}],
                }});
                const imgResp = await fetch("{image_dataurl}");
                if (!imgResp.ok) return null;
                const blob = await imgResp.blob();
                const prompt = [{{
                    role: "user",
                    content: [
                        {{ type: "text", value: "Give me the centre (x and y coordinates) of the missing puzzle piece in this image. Return a JSON array like [x, y] with numbers only." }},
                        {{ type: "image", value: blob }},
                    ],
                }}];
                const answer = await session.prompt(prompt);
                const txt = (answer || "").toString().trim();
                try {{ return JSON.parse(txt); }}
                catch {{ return null; }}
            }} catch (e) {{ throw e; }}
        }})()"#
    );

    let params = EvaluateParams::builder()
        .expression(&script)
        .await_promise(true)
        .build()
        .map_err(|e| CdpError::msg(format!("evaluate params: {e}")))?;

    let eval_fut = page.evaluate(params);

    let eval_res = tokio::time::timeout(Duration::from_millis(timeout_ms + 5_000), eval_fut)
        .await
        .map_err(|_| CdpError::Timeout)?; // outer timeout → CdpError::Timeout

    match eval_res {
        Ok(eval) => match eval.value() {
            Some(serde_json::Value::Array(arr)) if arr.len() == 2 => {
                let x = arr[0]
                    .as_f64()
                    .ok_or_else(|| CdpError::msg("Gemini did not return a numeric x"))?;
                let y = arr[1]
                    .as_f64()
                    .ok_or_else(|| CdpError::msg("Gemini did not return a numeric y"))?;
                Ok((x, y))
            }
            _ => Err(CdpError::msg("Gemini did not return a valid [x, y] array")),
        },
        Err(e) => Err(e), // propagate Chrome errors (including missing helper)
    }
}

#[cfg(all(feature = "chrome", feature = "real_browser"))]
async fn solve_point_with_legacy_routing(
    page: &Page,
    dataurl: &str,
    timeout_ms: u64,
) -> Result<(f64, f64), CdpError> {
    let visual = visual_from_data_url(None, dataurl)
        .map_err(|_| CdpError::msg("invalid CAPTCHA visual input"))?;
    let challenge = CaptchaChallenge {
        kind: CaptchaChallengeKind::PointSelection,
        instruction:
            "Give me the centre (x and y coordinates) of the missing puzzle piece in this image."
                .into(),
        visuals: vec![visual],
    };
    let local_request = CaptchaSolveRequest {
        correlation_id: "lemin-point".into(),
        selected_provider: CaptchaProviderId::LOCAL_LANGUAGE_MODEL,
        challenge: challenge.clone(),
        deadline: Duration::from_millis(timeout_ms),
    };
    let local_provider = LocalLanguageModelProvider { page };
    let mut registry = CaptchaProviderRegistry::new();
    registry
        .register(&local_provider)
        .expect("local provider identity is unique");
    let mut route = CaptchaRouteAttempts::new();
    match route
        .execute_explicit_attempt(&registry, &local_request)
        .await
    {
        CaptchaSolveOutcome::Solved {
            solution: CaptchaSolution::Point { x, y },
            ..
        } => Ok((*x, *y)),
        CaptchaSolveOutcome::Failed {
            failure: CaptchaSolveFailure::ProviderUnavailable,
            ..
        } => {
            let api_key = std::env::var("GEMINI_API_KEY")
                .map_err(|_| CdpError::msg("GEMINI_API_KEY not set"))?;
            let external_request = CaptchaSolveRequest {
                correlation_id: "lemin-point-external".into(),
                selected_provider: CaptchaProviderId::EXTERNAL_GEMINI,
                challenge,
                deadline: Duration::from_millis(timeout_ms / 2),
            };
            let _permit = crate::utils::GEMINI_SEM
                .acquire()
                .await
                .map_err(|_| CdpError::msg("Gemini solver admission cancelled"))?;
            let external_provider = ExternalGeminiProvider { api_key: &api_key };
            registry
                .register(&external_provider)
                .expect("external provider identity is unique");
            match route
                .execute_explicit_attempt(&registry, &external_request)
                .await
            {
                CaptchaSolveOutcome::Solved {
                    solution: CaptchaSolution::Point { x, y },
                    ..
                } => Ok((*x, *y)),
                _ => Err(route_error(&route)),
            }
        }
        _ => Err(route_error(&route)),
    }
}

#[cfg(all(feature = "chrome", feature = "real_browser"))]
/// Lemin solve handler.
///
/// CANONICAL_PROVIDER_DISPATCH_LEGACY_BINDING
/// (SCORPION_CANONICAL_PRODUCTION_CAPTCHA_EXECUTION_CONVERGENCE_001):
/// the point solve step (`solve_point_with_legacy_routing`) already
/// routes through the canonical `CaptchaProviderRegistry`/
/// `CaptchaRouteAttempts::execute_explicit_attempt`. Image capture and
/// click application immediately around it remain bespoke, not
/// `BrowserChallengeSnapshot`/`execute_browser_captcha_attempt` — see
/// `docs/frontier/CANONICAL_PRODUCTION_CAPTCHA_EXECUTION_CONVERGENCE_SDD.md`
/// for why that migration is deferred. Guardrailed: the provider-dispatch
/// call remains present and unmodified.
pub async fn lemin_handle(
    b: &mut Vec<u8>,
    page: &Page,
    viewport: &Option<crate::configuration::Viewport>,
) -> Result<bool, CdpError> {
    // -----------------------------------------------------------------
    // Fast‑gate – bail out early if the page does not contain a Lemin widget.
    // -----------------------------------------------------------------
    if !detect_lemin(b.as_slice()) {
        return Ok(false);
    }

    let mut progressed = false;

    // -----------------------------------------------------------------
    // Whole routine lives inside a 30 s timeout (same pattern as the rest).
    // -----------------------------------------------------------------
    let page_result = tokio::time::timeout(Duration::from_secs(30), async {
        // Disable cache + a little “human” mouse movement.
        let _ = tokio::join!(
            page.disable_network_cache(true),
            perform_smart_mouse_movement(page, viewport)
        );

        for _ in 0..10 {
            // ---------------------------------------------------------
            // a) Refresh the HTML source.
            // ---------------------------------------------------------
            if let Ok(cur) = page.outer_html_bytes().await {
                *b = cur;
            }

            // ---------------------------------------------------------
            // b) If Lemin vanished → success.
            // ---------------------------------------------------------
            if !detect_lemin(b.as_slice()) {
                progressed = true;
                break;
            }

            // ---------------------------------------------------------
            // c) Click the hidden checkbox that activates the puzzle.
            // ---------------------------------------------------------
            if let Ok(checkboxes) = page
                .find_elements_pierced(
                    r#"div[id*="lemin-cropped-captcha"] input[type="checkbox"]"#,
                )
                .await
            {
                if let Some(cb) = checkboxes.into_iter().next() {
                    let clicked = match cb.clickable_point().await {
                        Ok(p) => page.click_smooth(p).await.is_ok() || cb.click_smooth().await.is_ok(),
                        Err(_) => cb.click_smooth().await.is_ok(),
                    };
                    if clicked {
                        let mut wait_for = CF_WAIT_FOR.clone();
                        wait_for.delay =
                            crate::features::chrome_common::WaitForDelay::new(Some(
                                Duration::from_millis(900),
                            ))
                            .into();
                        wait_for.idle_network =
                            crate::features::chrome_common::WaitForIdleNetwork::new(
                                Duration::from_secs(6).into(),
                            )
                            .into();
                        wait_for.page_navigations = true;
                        let wait = Some(wait_for.clone());
                        let _ = tokio::join!(
                            page_wait(page, &wait),
                            perform_smart_mouse_movement(page, viewport),
                        );
                    }
                }
            }

            // ---------------------------------------------------------
            // d) Locate the **full background image** and turn it into a data‑URL.
            // ---------------------------------------------------------
            let img_el = match page
                .find_elements_pierced(
                    r#"div[id*="lemin-captcha-popup"] img[src][width][height]"#,
                )
                .await
            {
                Ok(mut els) => els.pop(),
                Err(_) => None,
            };

            let dataurl = if let Some(img) = &img_el {
                // Use a temporary canvas to read the image as a data‑URL.
                let call = CallFunctionOnParams::builder()
                    .object_id(img.remote_object_id.clone())
                    .function_declaration(
                        "(function(){ const canvas = document.createElement('canvas'); \
                           canvas.width = this.naturalWidth || this.width; \
                           canvas.height = this.naturalHeight || this.height; \
                           const ctx = canvas.getContext('2d'); \
                           ctx.drawImage(this,0,0); \
                           return canvas.toDataURL(); })",
                    )
                    .await_promise(true)
                    .build()
                    .map_err(|e| CdpError::msg(format!("call function params: {e}")))?;

                let eval = page.evaluate_function(call).await?;
                eval.value()
                    .and_then(|v| v.as_str().map(|s| s.to_owned()))
                    .ok_or_else(|| CdpError::msg("failed to get data‑url from Lemin image"))?
            } else {
                return Err(CdpError::msg(
                    "Lemin puzzle image not found – cannot continue",
                ));
            };

            // ---------------------------------------------------------
            // e) Ask Gemini for the missing piece centre (x, y) – first try the
            //    in‑page helper, then fall back to the remote call.
            // ---------------------------------------------------------
            let (target_x, target_y) =
                solve_point_with_legacy_routing(page, &dataurl, 20_000).await?;

            // ---------------------------------------------------------
            // f) Locate the **draggable piece**.
            // ---------------------------------------------------------
            let piece_el = match page
                .find_elements_pierced(
                    r#"div[style*="touch-action: none"][style*="cursor: move"][style*="position: absolute"]"#,
                )
                .await
            {
                Ok(mut els) => els.pop(),
                Err(_) => None,
            };

            let piece_bb = if let Some(el) = piece_el {
                el.bounding_box().await?
            } else {
                return Err(CdpError::msg(
                    "Lemin draggable piece not found – cannot solve",
                ));
            };

            // ---------------------------------------------------------
            // g) Transform the coordinates returned by Gemini (relative to the
            //    full image) into absolute page coordinates.
            // ---------------------------------------------------------
            let img_bb = if let Some(img) = &img_el {
                img.bounding_box().await?
            } else {
                return Err(CdpError::msg(
                    "Lemin full image missing when calculating drag target",
                ));
            };

            // The image might be scaled, so compute a scale factor.
            let scale_x = img_bb.width / img_bb.width.max(1.0);
            let scale_y = img_bb.height / img_bb.height.max(1.0);
            let page_target_x = img_bb.x + target_x * scale_x;
            let page_target_y = img_bb.y + target_y * scale_y;

            // ---------------------------------------------------------
            // h) Drag the piece to the target.
            // ---------------------------------------------------------
            let from = Point {
                x: piece_bb.x + piece_bb.width * 0.5,
                y: piece_bb.y + piece_bb.height * 0.5,
            };
            let to = Point {
                x: page_target_x,
                y: page_target_y,
            };
            let _ = page.click_and_drag_smooth(from, to).await;

            // ---------------------------------------------------------
            // i) Click the **Verify** button (if present).
            // ---------------------------------------------------------
            if let Ok(btns) = page
                .find_elements_pierced(r#"button.verify-button, button[id*="verify-button"]"#)
                .await
            {
                if let Some(btn) = btns.into_iter().next() {
                    let _ = btn.click_smooth().await;
                }
            }

            // ---------------------------------------------------------
            // j) Wait a little, then check whether the widget disappeared.
            // ---------------------------------------------------------
            let mut wf = CF_WAIT_FOR.clone();
            wf.delay = crate::features::chrome_common::WaitForDelay::new(Some(
                Duration::from_millis(1_100),
            ))
            .into();
            wf.idle_network = crate::features::chrome_common::WaitForIdleNetwork::new(
                Duration::from_secs(7).into(),
            )
            .into();
            wf.page_navigations = true;
            let wait = Some(wf.clone());
            let _ = tokio::join!(
                page_wait(page, &wait),
                perform_smart_mouse_movement(page, viewport),
            );

            // ---------------------------------------------------------
            // k) Final check – if the Lemin widget vanished we are done.
            // ---------------------------------------------------------
            if let Ok(nc2) = page.outer_html_bytes().await {
                *b = nc2;
                if !detect_lemin(b.as_slice()) {
                    progressed = true;
                    break;
                }
            }

            // If we get here the puzzle was not solved – the outer loop will retry.
        }

        Ok::<(), CdpError>(())
    })
    .await;

    match page_result {
        Ok(_) => Ok(progressed),
        Err(_) => Err(CdpError::Timeout),
    }
}

#[cfg(all(feature = "chrome", feature = "real_browser"))]
#[derive(Debug, Clone)]
/// The RC tile reference.
pub struct RcTileRef<'a> {
    /// The id.
    pub id: u8,
    /// The img src.
    pub img_src: &'a str,
}

#[cfg(all(feature = "chrome", feature = "real_browser"))]
/// Enterprise challenge.
#[derive(Debug, Default, Clone)]
pub struct RcEnterpriseChallenge<'a> {
    /// e.g. "bridges" (from `<strong>bridges</strong>`)
    pub target: Option<&'a str>,
    /// full instruction line if you want it
    pub instruction_text: Option<&'a str>,
    /// The tile space.
    pub tiles: Vec<RcTileRef<'a>>,
    /// Has the verification button.
    pub has_verify_button: bool,
}

/// Byte‑wise equality (fast, zero‑allocation).  
/// Returns `true` iff `a` and `b` have the same length **and** identical bytes.
#[inline(always)]
#[cfg(all(feature = "chrome", feature = "real_browser"))]
fn memeq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x == y)
}

/// Search for `needle` in `haystack` starting at `start`.  
/// Returns the absolute index of the first match or `None` if not found.
#[inline(always)]
#[cfg(all(feature = "chrome", feature = "real_browser"))]
fn find(h: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    let nl = needle.len();
    if nl == 0 || start >= h.len() || nl > h.len() - start {
        return None;
    }
    h[start..]
        .windows(nl)
        .position(|w| memeq(w, needle))
        .map(|p| start + p)
}

/// Find the next double‑quote (`"`) after `start`.  
/// Returns its absolute index or `None` if missing.
#[inline(always)]
#[cfg(all(feature = "chrome", feature = "real_browser"))]
fn find_quote_end(h: &[u8], start: usize) -> Option<usize> {
    h.get(start..)?
        .iter()
        .position(|&c| c == b'"')
        .map(|p| start + p)
}

/// Is `b` an ASCII digit (`0`‑`9`)?
#[inline(always)]
#[cfg(all(feature = "chrome", feature = "real_browser"))]
fn is_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

/// Convert a single ASCII digit to `u8`. Returns `None` for non‑digits.
#[inline(always)]
#[cfg(all(feature = "chrome", feature = "real_browser"))]
fn parse_u8_1digit(b: u8) -> Option<u8> {
    if is_digit(b) {
        Some(b - b'0')
    } else {
        None
    }
}

/// Extracts recaptcha enterprise image-grid metadata from the iframe inner HTML.
#[cfg(all(feature = "chrome", feature = "real_browser"))]
#[inline(always)]
pub fn extract_rc_enterprise_challenge<'a>(html: &'a [u8]) -> Option<RcEnterpriseChallenge<'a>> {
    // -----------------------------------------------------------------
    // Quick gate – all four guard patterns must be present.
    // -----------------------------------------------------------------
    // `RC_ENTERPRISE_GUARD_AC` contains the four patterns in the order
    // they appear in `RC_ENTERPRISE_GUARD_PATTERNS`.  We check each one
    // individually because we need **all** of them.
    let mut guard_hits = [false; 4];
    for m in RC_ENTERPRISE_GUARD_AC.find_iter(html) {
        guard_hits[m.pattern()] = true;
    }
    if !guard_hits.iter().all(|&b| b) {
        return None;
    }

    // -----------------------------------------------------------------
    // Does the page have a “Verify” button?
    // -----------------------------------------------------------------
    let has_verify_button = RC_VERIFY_BUTTON_AC.is_match(html);

    let mut out = RcEnterpriseChallenge {
        target: None,
        instruction_text: None,
        tiles: Vec::with_capacity(12),
        has_verify_button,
    };

    // -----------------------------------------------------------------
    // 1️⃣  Extract the *target* word (the word that appears inside the
    //      <strong …> … </strong> that is near the description).
    // -----------------------------------------------------------------
    const DESC_PAT: &[u8] = b"rc-imageselect-desc";
    const STRONG_OPEN: &[u8] = b"<strong";
    const GT: &[u8] = b">";
    const STRONG_CLOSE: &[u8] = b"</strong>";

    if let Some(desc_pos) = find(html, DESC_PAT, 0) {
        // Look forward a bounded window for the <strong> element.
        let win_end = (desc_pos + 900).min(html.len());

        if let Some(strong_pos) = find(html, STRONG_OPEN, desc_pos) {
            if strong_pos < win_end {
                if let Some(gt_pos) = find(html, GT, strong_pos) {
                    let txt_start = gt_pos + 1;
                    if let Some(close_pos) = find(html, STRONG_CLOSE, txt_start) {
                        if close_pos <= win_end {
                            if let Ok(word) = core::str::from_utf8(&html[txt_start..close_pos]) {
                                let word = word.trim();
                                if !word.is_empty() {
                                    out.target = Some(word);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Optional – full description text (everything between the first ‘>’
        // after the descriptor and the next ‘<’).
        if let Some(tag_end) = find(html, b">", desc_pos) {
            let t0 = tag_end + 1;
            if let Some(t1) = find(html, b"<", t0) {
                if let Ok(txt) = core::str::from_utf8(&html[t0..t1]) {
                    let txt = txt.trim();
                    if !txt.is_empty() {
                        out.instruction_text = Some(txt);
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // 2️⃣  Extract every tile (id + image URL).
    // -----------------------------------------------------------------
    const ID_PAT: &[u8] = b"id=\"";
    const SRC_PAT: &[u8] = b"src=\"";
    const PAYLOAD_PREFIX: &[u8] = b"https://www.google.com/recaptcha/enterprise/payload";

    // `RC_TILE_CLASS_AC` yields the start offset of every occurrence of
    // `rc-imageselect-tile`.  We iterate over those offsets instead of the
    // previous while‑loop that scanned the whole buffer.
    for m in RC_TILE_CLASS_AC.find_iter(html) {
        let tile_pos = m.start();

        // Back‑scan (max 240 bytes) for the id attribute that belongs to this tile.
        let back = tile_pos.saturating_sub(240);
        let id_pos = match find(html, ID_PAT, back) {
            Some(p) if p < tile_pos => p,
            _ => continue,
        };
        // The id is a single digit (0‑9) in the official widget.
        let id = match html
            .get(id_pos + ID_PAT.len())
            .copied()
            .and_then(parse_u8_1digit)
        {
            Some(v) => v,
            None => continue,
        };

        // Find the image src *after* the tile marker.
        let src_pos = match find(html, SRC_PAT, tile_pos) {
            Some(p) => p,
            None => continue,
        };
        let url_start = src_pos + SRC_PAT.len();

        // Ensure the URL really points to the Enterprise payload endpoint.
        if html.get(url_start..url_start + PAYLOAD_PREFIX.len()) != Some(PAYLOAD_PREFIX) {
            continue;
        }

        // The URL ends at the next double‑quote.
        let url_end = match find_quote_end(html, url_start) {
            Some(e) => e,
            None => continue,
        };
        let url = match core::str::from_utf8(&html[url_start..url_end]) {
            Ok(s) => s,
            Err(_) => continue,
        };

        // De‑duplicate tiles that may re‑appear after a re‑render.
        if !out.tiles.iter().any(|t| t.id == id) {
            out.tiles.push(RcTileRef { id, img_src: url });
        }
    }

    if out.tiles.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(all(feature = "chrome", feature = "real_browser"))]
/// In page geetest helper.
async fn solve_geetest_with_local_language_model(
    page: &Page,
    canvas_dataurl: &str,
    timeout_ms: u64,
) -> Result<f64, CdpError> {
    // -----------------------------------------------------------------
    // 1️⃣  Encode the data‑url as a JSON string so that it can be safely
    //     interpolated into the JS source.
    // -----------------------------------------------------------------
    let js_literal = serde_json::to_string(canvas_dataurl)
        .map_err(|e| CdpError::msg(format!("JSON encode error: {e}")))?;

    // -----------------------------------------------------------------
    // 2️⃣  The in‑page helper script.
    // -----------------------------------------------------------------
    //    • Creates a `LanguageModel` (the same model Chrome exposes to
    //      extensions).
    //    • Downloads the image from the data‑url, sends it together with a
    //      short prompt that asks for *only* the horizontal offset.
    //    • Returns that offset as a plain number (or `null` on any error).
    // -----------------------------------------------------------------
    let script = format!(
        r#"(async () => {{
            try {{
                const session = await LanguageModel.create({{
                    expectedInputs: [
                        {{ type: "image" }},
                        {{ type: "text", languages: ["en"] }},
                    ],
                    expectedOutputs: [{{ type: "text", languages: ["en"] }}],
                }});
                const imgResp = await fetch({js_literal});
                if (!imgResp.ok) return null;
                const blob = await imgResp.blob();

                const prompt = [{{
                    role: "user",
                    content: [
                        {{ type: "image", value: blob }},
                        {{ type: "text", value: "Return only the horizontal pixel offset (as a number) of the missing puzzle piece gap in this image." }},
                    ],
                }}];

                const answer = await session.prompt(prompt);
                const txt = (answer ?? "").toString().trim();
                const num = parseFloat(txt);
                return isNaN(num) ? null : num;
            }} catch (e) {{
                throw e;
            }}
        }})()"#
    );

    let params = EvaluateParams::builder()
        .expression(&script)
        .await_promise(true)
        .build()
        .map_err(|e| CdpError::msg(format!("evaluate params: {e}")))?;

    let eval_fut = page.evaluate(params);

    let eval_outcome = tokio::time::timeout(Duration::from_millis(timeout_ms + 5_000), eval_fut)
        .await
        .map_err(|_| CdpError::Timeout)?; // outer timeout → CdpError::Timeout

    // -----------------------------------------------------------------
    // 4️⃣  Distinguish three cases:
    //     a) The script succeeded (`Ok(EvaluationResult)`).
    //     b) The script threw → we get `Err(CdpError)`.  If the error
    //        signals a missing helper we fall back, otherwise we bubble it.
    //     c) The script succeeded but returned no numeric value.
    // -----------------------------------------------------------------
    let eval_res = match eval_outcome {
        Ok(res) => res,
        Err(err) => {
            return Err(err);
        }
    };

    let maybe_offset = match eval_res.value() {
        Some(v) => match v {
            serde_json::Value::Number(n) => n.as_f64(),
            serde_json::Value::String(s) => s.parse::<f64>().ok(),
            _ => None,
        },
        None => None,
    };

    if let Some(off) = maybe_offset {
        return Ok(off);
    }

    Err(CdpError::msg(
        "In‑page Gemini helper returned no numeric result",
    ))
}

/// Geetest solving
#[cfg(all(feature = "chrome", feature = "real_browser"))]
/// CANONICAL_PROVIDER_DISPATCH_LEGACY_BINDING
/// (SCORPION_CANONICAL_PRODUCTION_CAPTCHA_EXECUTION_CONVERGENCE_001): the
/// horizontal-offset solve step (`solve_horizontal_offset_with_legacy_routing`)
/// already routes through the canonical `CaptchaProviderRegistry`/
/// `CaptchaRouteAttempts::execute_explicit_attempt`. Image capture and
/// slider-drag application immediately around it remain bespoke, not
/// `BrowserChallengeSnapshot`/`execute_browser_captcha_attempt` — see
/// `docs/frontier/CANONICAL_PRODUCTION_CAPTCHA_EXECUTION_CONVERGENCE_SDD.md`
/// for why that migration is deferred. Guardrailed: the provider-dispatch
/// call remains present and unmodified.
#[inline(always)]
pub async fn geetest_handle(
    b: &mut Vec<u8>,
    page: &Page,
    viewport: &Option<crate::configuration::Viewport>,
) -> Result<bool, CdpError> {
    // -----------------------------------------------------------------
    // Fast gate – bail out early if the page does not look like GeeTest.
    // -----------------------------------------------------------------
    if !looks_like_geetest(b.as_slice()) {
        return Ok(false);
    }

    let mut progressed = false;

    // -----------------------------------------------------------------
    // Whole routine lives inside a 30 s timeout (same pattern as the rest
    // of the code‑base).
    // -----------------------------------------------------------------
    let page_result = tokio::time::timeout(Duration::from_secs(30), async {
        // Disable the network cache + a little “human” mouse movement.
        let _ = tokio::join!(
            page.disable_network_cache(true),
            perform_smart_mouse_movement(page, viewport)
        );

        for _ in 0..10 {
            // -------------------------------------------------------------
            // a) Refresh the HTML source.
            // -------------------------------------------------------------
            if let Ok(cur) = page.outer_html_bytes().await {
                *b = cur;
            }

            // -------------------------------------------------------------
            // b) If GeeTest vanished → success.
            // -------------------------------------------------------------
            if !looks_like_geetest(b.as_slice()) {
                progressed = true;
                break;
            }

            // -------------------------------------------------------------
            // c) Still loading?  Wait like Cloudflare.
            // -------------------------------------------------------------
            if looks_like_geetest_loading(b.as_slice()) {
                let mut wait_for = CF_WAIT_FOR.clone();
                wait_for.delay = crate::features::chrome_common::WaitForDelay::new(Some(
                    Duration::from_millis(1_000),
                ))
                .into();
                wait_for.idle_network = crate::features::chrome_common::WaitForIdleNetwork::new(
                    Duration::from_secs(7).into(),
                )
                .into();
                wait_for.page_navigations = true;
                let wait = Some(wait_for.clone());
                let _ = tokio::join!(
                    page_wait(page, &wait),
                    perform_smart_mouse_movement(page, viewport),
                );
                continue;
            }

            // -------------------------------------------------------------
            // d) Click the “Click to verify” radar.
            // -------------------------------------------------------------
            let mut clicked = false;
            if let Ok(els) = page.find_elements_pierced(r#".geetest_radar"#).await {
                if let Some(el) = els.into_iter().next() {
                    clicked = match el.clickable_point().await {
                        Ok(p) => {
                            page.click_smooth(p).await.is_ok() || el.click_smooth().await.is_ok()
                        }
                        Err(_) => el.click_smooth().await.is_ok(),
                    };
                }
            }
            // Fallback element.
            if !clicked {
                if let Ok(els) = page
                    .find_elements_pierced(r#".geetest_radar_tip_content"#)
                    .await
                {
                    if let Some(el) = els.into_iter().next() {
                        clicked = match el.clickable_point().await {
                            Ok(p) => {
                                page.click_smooth(p).await.is_ok()
                                    || el.click_smooth().await.is_ok()
                            }
                            Err(_) => el.click_smooth().await.is_ok(),
                        };
                    }
                }
            }

            // -------------------------------------------------------------
            // e) Short wait after the click so the widget can render.
            // -------------------------------------------------------------
            let mut wait_for = CF_WAIT_FOR.clone();
            wait_for.delay = crate::features::chrome_common::WaitForDelay::new(Some(if clicked {
                Duration::from_millis(900)
            } else {
                Duration::from_millis(700)
            }))
            .into();
            wait_for.idle_network = crate::features::chrome_common::WaitForIdleNetwork::new(
                Duration::from_secs(6).into(),
            )
            .into();
            wait_for.page_navigations = true;
            let wait = Some(wait_for.clone());
            let _ = tokio::join!(
                page_wait(page, &wait),
                perform_smart_mouse_movement(page, viewport),
            );

            // -------------------------------------------------------------
            // f) Refresh HTML again – now the slider should be visible.
            // -------------------------------------------------------------
            if let Ok(nc) = page.outer_html_bytes().await {
                *b = nc;

                if looks_like_geetest_challenge_visible(b.as_slice()) {
                    // -------------------------------------------------
                    //   🎯  ***  SOLVE THE SLIDER  ***  🎯
                    // -------------------------------------------------
                    // 1️⃣  Grab the *track* (the gray bar the button slides on)
                    //     and the slider button.
                    //     Try the v3 selectors first; fall back to the v4 ones.
                    // -------------------------------------------------
                    async fn first_of(
                        page: &Page,
                        sel_a: &str,
                        sel_b: &str,
                    ) -> Result<chromiumoxide::Element, CdpError> {
                        // Try selector A.
                        if let Ok(els) = page.find_elements_pierced(sel_a).await {
                            if let Some(el) = els.into_iter().next() {
                                return Ok(el);
                            }
                        }
                        // Fallback to selector B.
                        let els = page.find_elements_pierced(sel_b).await?;
                        let el = els.into_iter().next().ok_or_else(|| {
                            CdpError::msg(format!("neither {sel_a} nor {sel_b} found"))
                        })?;
                        Ok(el)
                    }

                    // Track – v3: .geetest_slicebg  |  v4: .geetest_wrap
                    let track_el = first_of(page, ".geetest_slicebg", ".geetest_wrap").await?;
                    let track_bb = track_el.bounding_box().await?;

                    // Button – v3: .geetest_slider_button  |  v4: .geetest_btn
                    let btn_el = first_of(page, ".geetest_slider_button", ".geetest_btn").await?;
                    let btn_bb = btn_el.bounding_box().await?;

                    // -------------------------------------------------
                    // 2️⃣  Locate the *canvas* that holds the puzzle image.
                    // -------------------------------------------------
                    let canvas_el = page
                        .find_elements_pierced(r#".geetest_canvas_slice.geetest_absolute"#)
                        .await?
                        .into_iter()
                        .next()
                        .ok_or_else(|| CdpError::msg("canvas element not found"))?;

                    // -------------------------------------------------
                    // 3️⃣  Pull the canvas data‑URL using the element we just
                    //     fetched (no unused‑variable warning).
                    // -------------------------------------------------
                    let dataurl: String = {
                        let call = CallFunctionOnParams::builder()
                            .object_id(canvas_el.remote_object_id.clone())
                            .function_declaration("(function(){ return this.toDataURL(); })")
                            .await_promise(true)
                            .build()
                            .map_err(|e| CdpError::msg(format!("call function params: {e}")))?;

                        // `page.evaluate_function` returns an `EvaluationResult`.
                        let eval_res = page.evaluate_function(call).await?;
                        eval_res
                            .value()
                            .and_then(|v| v.as_str().map(|s| s.to_owned()))
                            .ok_or_else(|| {
                                CdpError::msg("Failed to extract data‑url from canvas")
                            })?
                    };

                    // -------------------------------------------------
                    // 4️⃣  Try the in‑page Gemini helper first.  If it does not
                    //     exist we fall back to the external Gemini API (or the
                    //     centre‑of‑track when the gemini feature is disabled).
                    // -------------------------------------------------
                    let gap_x = match solve_horizontal_offset_with_legacy_routing(
                        page,
                        &dataurl,
                        20_000,
                        (track_bb.width * 0.5) as f64,
                    )
                    .await
                    {
                        Ok(x) => x,
                        Err(e) => return Err(e), // real Chrome error – bubble up
                    };

                    // -------------------------------------------------
                    // 5️⃣  Convert the canvas‑relative offset into a *page*
                    //     coordinate.
                    // -------------------------------------------------
                    let canvas_width: f64 = page
                        .evaluate(format!(
                            "document.querySelector('{}').width",
                            ".geetest_canvas_slice.geetest_absolute"
                        ))
                        .await?
                        .into_value()?;

                    let proportion = (gap_x / canvas_width).clamp(0.0, 1.0);
                    let target_x = track_bb.x + proportion * track_bb.width;

                    // -------------------------------------------------
                    // 6️⃣  Build the drag points.
                    // -------------------------------------------------
                    let from = Point {
                        x: btn_bb.x + btn_bb.width * 0.5,
                        y: btn_bb.y + btn_bb.height * 0.5,
                    };
                    let to = Point {
                        x: target_x,
                        y: track_bb.y + track_bb.height * 0.5,
                    };

                    // -------------------------------------------------
                    // 7️⃣  Perform the drag.
                    // -------------------------------------------------
                    let _ = page.click_and_drag_smooth(from, to).await;

                    // -------------------------------------------------
                    // 8️⃣  Wait a little, then verify whether the widget vanished.
                    // -------------------------------------------------
                    let mut wf = CF_WAIT_FOR.clone();
                    wf.delay = crate::features::chrome_common::WaitForDelay::new(Some(
                        Duration::from_millis(1_100),
                    ))
                    .into();
                    wf.idle_network = crate::features::chrome_common::WaitForIdleNetwork::new(
                        Duration::from_secs(7).into(),
                    )
                    .into();
                    wf.page_navigations = true;
                    let wait = Some(wf.clone());
                    let _ = tokio::join!(
                        page_wait(page, &wait),
                        perform_smart_mouse_movement(page, viewport),
                    );

                    // Refresh the HTML one final time.
                    if let Ok(nc2) = page.outer_html_bytes().await {
                        *b = nc2;
                        if !looks_like_geetest(b.as_slice()) {
                            progressed = true;
                            break;
                        }
                    }

                    // If we are still here the slider failed – loop again (max 10).
                    continue;
                }

                // If the widget disappeared after any step, we are done.
                if !looks_like_geetest(b.as_slice()) {
                    progressed = true;
                    break;
                }
            }
        }

        Ok::<(), CdpError>(())
    })
    .await;

    match page_result {
        Ok(_) => Ok(progressed),
        Err(_) => Err(CdpError::Timeout),
    }
}

#[cfg(all(test, feature = "chrome"))]
mod cf_turnstile_detection_tests {
    use super::{detect_cf_embedded_turnstile, detect_cf_turnstyle};

    #[test]
    fn empty_body_no_match() {
        assert!(!detect_cf_turnstyle(&[]));
        assert!(!detect_cf_embedded_turnstile(&[]));
    }

    #[test]
    fn just_a_moment_wall_still_detected() {
        // Regression guard: the prefix-only AC for the wall-page
        // fingerprint must keep matching exactly as before, even
        // though the wall page contains no embedded-widget markup.
        let body = br#"<!DOCTYPE html><html lang="en-US"><head><title>Just a moment...</title></head><body>cf wall</body></html>"#;
        assert!(detect_cf_turnstyle(body));
    }

    #[test]
    fn embedded_widget_div_double_quoted() {
        let body = br#"<html><body><form><div class="cf-turnstile" data-sitekey="abc"></div></form></body></html>"#;
        assert!(detect_cf_embedded_turnstile(body));
        assert!(detect_cf_turnstyle(body));
    }

    #[test]
    fn embedded_widget_div_single_quoted() {
        let body =
            br#"<html><body><div class='cf-turnstile' data-sitekey='abc'></div></body></html>"#;
        assert!(detect_cf_embedded_turnstile(body));
        assert!(detect_cf_turnstyle(body));
    }

    #[test]
    fn embedded_widget_api_js_endpoint() {
        let body = br#"<html><head><script src="https://challenges.cloudflare.com/turnstile/v0/api.js" async defer></script></head><body></body></html>"#;
        assert!(detect_cf_embedded_turnstile(body));
        assert!(detect_cf_turnstyle(body));
    }

    #[test]
    fn quoted_class_in_script_does_not_match() {
        // Critical false-positive guard: a script body or JSON blob
        // that mentions the string "cf-turnstile" without an actual
        // HTML tag wrapper must NOT trigger detection. The patterns
        // require the `<div class=` tag prefix specifically.
        let body = br#"<html><body><script>var name="cf-turnstile";var data={"class":"cf-turnstile"};</script></body></html>"#;
        assert!(!detect_cf_embedded_turnstile(body));
        assert!(!detect_cf_turnstyle(body));
    }

    #[test]
    fn unrelated_page_does_not_match() {
        let body = br#"<html><body><h1>Welcome</h1><p>Hello world.</p></body></html>"#;
        assert!(!detect_cf_embedded_turnstile(body));
        assert!(!detect_cf_turnstyle(body));
    }

    #[test]
    fn case_insensitive_class_attribute() {
        // HTML attribute values are case-insensitive per spec — make
        // sure shouty markup still routes through the same path.
        let body =
            br#"<HTML><BODY><DIV CLASS="CF-TURNSTILE" DATA-SITEKEY="abc"></DIV></BODY></HTML>"#;
        assert!(detect_cf_embedded_turnstile(body));
    }
}

/// Real-Chrome production proof for `route_detected_browser_challenge`
/// (SCORPION_CANONICAL_CAPTCHA_PROVIDER_ROUTING_BINDING_001). Inline (not
/// under `spider/tests/`) because `route_detected_browser_challenge` and
/// `CaptchaRouteOutcomeSummary` are deliberately `pub(crate)` — this
/// binding is production wiring, not a public API expansion — so only an
/// in-crate test can observe the outcome directly. `about:blank` is enough:
/// this suite proves *routing* behavior (no provider configured; unknown
/// provider; the real, non-mocked `LOCAL_LANGUAGE_MODEL` provider genuinely
/// failing in this environment), not detection, which is already proven
/// separately against real fixtures in
/// `spider/tests/browser_challenge_detection_real.rs`.
#[cfg(all(test, feature = "chrome"))]
mod route_detected_browser_challenge_tests {
    use super::route_detected_browser_challenge;
    use crate::features::captcha::{
        CaptchaChallenge, CaptchaChallengeKind, CaptchaProviderId, CaptchaRouteOutcomeSummary,
        CaptchaVisualInput,
    };

    async fn launch() -> chromiumoxide::Browser {
        let config = crate::configuration::Configuration::default();
        let Some((browser, _handler, _, _, _)) =
            crate::features::chrome::launch_browser(&config, &None).await
        else {
            panic!("real-browser production routing proof requires local Chrome");
        };
        browser
    }

    fn dummy_challenge() -> CaptchaChallenge {
        CaptchaChallenge {
            kind: CaptchaChallengeKind::PointSelection,
            instruction: "select the matching point".into(),
            visuals: vec![CaptchaVisualInput::materialized(
                None,
                "image/png",
                vec![1u8, 2, 3],
            )],
        }
    }

    /// Zero-provider-cost default: no provider configured, no registry or
    /// provider constructed — proven by getting `NotConfigured` back
    /// without the call ever needing the live page beyond the parameter
    /// itself.
    #[tokio::test]
    async fn no_provider_configured_returns_not_configured() {
        let browser = launch().await;
        let page = browser.new_page("about:blank").await.unwrap();
        let outcome = route_detected_browser_challenge(
            &page,
            None,
            dummy_challenge(),
            None,
            std::time::Duration::from_secs(5),
        )
        .await;
        assert_eq!(outcome, CaptchaRouteOutcomeSummary::NotConfigured);
    }

    /// A configured provider this build never registers (PaliGemma is a
    /// non-default, non-shipping feature; this router registers only
    /// `LOCAL_LANGUAGE_MODEL`) fails typed, not silently, not with a panic.
    #[tokio::test]
    async fn unregistered_provider_selection_is_typed_unavailable() {
        let browser = launch().await;
        let page = browser.new_page("about:blank").await.unwrap();
        let outcome = route_detected_browser_challenge(
            &page,
            None,
            dummy_challenge(),
            Some(CaptchaProviderId::PALIGEMMA_LOCAL),
            std::time::Duration::from_secs(5),
        )
        .await;
        assert_eq!(outcome, CaptchaRouteOutcomeSummary::ProviderUnavailable);
    }

    /// REAL CHROME PRODUCTION ROUTING PROOF (not real inference): the only
    /// provider this router can construct with no external credential,
    /// `LOCAL_LANGUAGE_MODEL`, genuinely gets registered, resolved, and
    /// invoked — and genuinely fails, because this environment's Chromium
    /// build has no `window.LanguageModel` API. Truthful
    /// `ProviderUnavailable`, not a fabricated success, not a hang, not a
    /// panic.
    #[cfg(feature = "real_browser")]
    #[tokio::test]
    async fn local_language_model_selected_genuinely_fails_in_this_environment() {
        let browser = launch().await;
        let page = browser.new_page("about:blank").await.unwrap();
        let outcome = route_detected_browser_challenge(
            &page,
            None,
            dummy_challenge(),
            Some(CaptchaProviderId::LOCAL_LANGUAGE_MODEL),
            std::time::Duration::from_secs(15),
        )
        .await;
        assert_eq!(outcome, CaptchaRouteOutcomeSummary::ProviderUnavailable);
    }

    /// REAL CHROME PRODUCTION ROUTING PROOF for the `Some(snapshot)` branch
    /// added by `SCORPION_CANONICAL_CAPTCHA_SOLUTION_BROWSER_ACTION_BINDING_001`:
    /// a genuine `BrowserChallengeSnapshot`, obtained the same way
    /// production does — through the canonical detector's own capture
    /// step, never by reimplementing that step directly in this module
    /// (see `canonical_captcha_execution_seam_is_not_reimplemented_in_solvers`
    /// in `architecture_guardrails.rs`, which forbids exactly that) — routed
    /// with an unregistered provider still classifies through
    /// `outcome_for_browser_action_failure`'s `ProviderFailure` recovery to
    /// the exact same `ProviderUnavailable` the solve-only path reports —
    /// proving both branches agree on provider-level failure, and that no
    /// browser action is ever dispatched when the provider itself never
    /// resolved (no `Applied`/`Failed` action outcome leaks through here).
    #[tokio::test]
    async fn snapshot_bound_route_with_unregistered_provider_is_typed_unavailable() {
        use crate::features::browser_challenge_detection::{
            detect_browser_challenge, DetectedBrowserChallenge,
        };

        let browser = launch().await;
        let html = r#"<!doctype html><style>
  body{margin:0}
  #challenge-1{position:absolute;left:0;top:0;width:240px;height:120px}
  #pick-1{position:absolute;left:0;top:0;width:1px;height:1px}
</style>
<div id="challenge-1" role="application" aria-label="pick a point">
  <div id="pick-1" role="button" tabindex="0"></div>
</div>"#;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                html.len(),
                html
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let page = browser
            .new_page(format!("http://{address}/"))
            .await
            .unwrap();
        page.wait_for_navigation().await.unwrap();
        let detected = detect_browser_challenge(&page).await.unwrap().unwrap();
        let DetectedBrowserChallenge::TopLevel { snapshot, .. } = detected else {
            panic!("expected a top-level detection");
        };
        let outcome = route_detected_browser_challenge(
            &page,
            Some(&snapshot),
            dummy_challenge(),
            Some(CaptchaProviderId::PALIGEMMA_LOCAL),
            std::time::Duration::from_secs(5),
        )
        .await;
        assert_eq!(outcome, CaptchaRouteOutcomeSummary::ProviderUnavailable);
        server.await.unwrap();
    }
}
