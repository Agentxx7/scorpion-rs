//! Provider-native Hugging Face model-repository discovery through the
//! official Hub API.
//!
//! This module performs one bounded model-list/search request and stops. It
//! never downloads model files, cards, revisions, weights, or tokenizers; it
//! also never invokes inference, a parser, a browser, or generic acquisition.

use crate::features::source::SourceItem;
use crate::features::source_provider::{
    ProviderCapabilities, ProviderDescriptor, ProviderDiscovery, SourceProvider,
};
use reqwest::header::{HeaderValue, AUTHORIZATION};

const PROVIDER_ID: &str = "hugging_face";
const HUB_BASE: &str = "https://huggingface.co/";
const DEFAULT_LIMIT: usize = 10;
// A Scorpion safety bound, not a claim about an undocumented Hub maximum.
const MAX_LIMIT: usize = 100;

/// One explicitly bounded model-search request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HuggingFaceModelSearchRequest {
    /// Text passed unchanged to the Hub API's `search` parameter.
    pub query: String,
    /// Maximum results requested from the single response (`1..=100`).
    pub limit: usize,
}

impl HuggingFaceModelSearchRequest {
    /// Construct a request with a conservative result bound of 10.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            limit: DEFAULT_LIMIT,
        }
    }

    /// Set the one-request result bound. Validation occurs before networking.
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }
}

/// Official Hugging Face Hub model-repository source provider.
pub struct HuggingFaceModelProvider {
    descriptor: ProviderDescriptor,
    client: reqwest::Client,
    api_base: url::Url,
    public_base: url::Url,
    authorization: Option<HeaderValue>,
}

impl std::fmt::Debug for HuggingFaceModelProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HuggingFaceModelProvider")
            .field("descriptor", &self.descriptor)
            .field("api_base", &self.api_base)
            .field("public_base", &self.public_base)
            .field("authenticated", &self.authorization.is_some())
            .finish_non_exhaustive()
    }
}

impl Default for HuggingFaceModelProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl HuggingFaceModelProvider {
    /// Construct an unauthenticated provider against the public Hub.
    pub fn new() -> Self {
        Self::with_client_and_base(
            reqwest::Client::new(),
            url::Url::parse(HUB_BASE).expect("static Hugging Face Hub URL is valid"),
        )
    }

    fn with_client_and_base(client: reqwest::Client, api_base: url::Url) -> Self {
        Self {
            descriptor: ProviderDescriptor::new(
                PROVIDER_ID,
                "Hugging Face",
                ProviderCapabilities::ITEMS,
            ),
            client,
            api_base,
            public_base: url::Url::parse(HUB_BASE)
                .expect("static Hugging Face public URL is valid"),
            authorization: None,
        }
    }

    /// Attach a caller-supplied token in memory. It is immediately converted
    /// to a sensitive header and never stored in provider metadata, request
    /// vocabulary, URLs, outputs, Debug, Display, errors, or persistent state.
    pub fn with_token(mut self, token: &str) -> Result<Self, HuggingFaceProviderError> {
        let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| HuggingFaceProviderError::InvalidToken)?;
        value.set_sensitive(true);
        self.authorization = Some(value);
        Ok(self)
    }

    /// Execute exactly one Hub model-search request and preserve response
    /// order and duplicates in normalized provider output.
    pub async fn search_models(
        &self,
        request: &HuggingFaceModelSearchRequest,
    ) -> Result<Vec<ProviderDiscovery>, HuggingFaceProviderError> {
        validate_request(request)?;

        let endpoint = self
            .api_base
            .join("api/models")
            .map_err(|_| HuggingFaceProviderError::InvalidApiEndpoint)?;
        let mut builder = self.client.get(endpoint).query(&[
            ("search", request.query.as_str()),
            ("limit", &request.limit.to_string()),
        ]);
        if let Some(authorization) = &self.authorization {
            builder = builder.header(AUTHORIZATION, authorization.clone());
        }

        let response = builder
            .send()
            .await
            .map_err(|error| HuggingFaceProviderError::RequestFailed(error.to_string()))?;
        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(HuggingFaceProviderError::RateLimited {
                reset_after_seconds: response
                    .headers()
                    .get("ratelimit")
                    .and_then(|value| value.to_str().ok())
                    .and_then(parse_rate_limit_reset),
            });
        }
        if !status.is_success() {
            return Err(HuggingFaceProviderError::ProviderStatus(status.as_u16()));
        }

        let containing_url = response.url().to_string();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| HuggingFaceProviderError::RequestFailed(error.to_string()))?;
        let models: Vec<ModelSearchItem> = serde_json::from_slice(&bytes)
            .map_err(|error| HuggingFaceProviderError::ResponseDecode(error.to_string()))?;

        models
            .into_iter()
            .map(|model| normalize_model(model, &containing_url, &self.public_base))
            .collect()
    }
}

impl SourceProvider for HuggingFaceModelProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }
}

fn validate_request(
    request: &HuggingFaceModelSearchRequest,
) -> Result<(), HuggingFaceProviderError> {
    if request.query.trim().is_empty() {
        return Err(HuggingFaceProviderError::InvalidRequest(
            HuggingFaceRequestError::EmptyQuery,
        ));
    }
    if !(1..=MAX_LIMIT).contains(&request.limit) {
        return Err(HuggingFaceProviderError::InvalidRequest(
            HuggingFaceRequestError::LimitOutOfRange,
        ));
    }
    Ok(())
}

fn normalize_model(
    model: ModelSearchItem,
    containing_url: &str,
    hub_base: &url::Url,
) -> Result<ProviderDiscovery, HuggingFaceProviderError> {
    let segments = model.id.split('/').collect::<Vec<_>>();
    if segments.is_empty()
        || segments.len() > 2
        || segments.iter().any(|segment| segment.is_empty())
    {
        return Err(HuggingFaceProviderError::InvalidModelIdentity);
    }
    let mut public_url = hub_base.clone();
    {
        let mut path = public_url
            .path_segments_mut()
            .map_err(|_| HuggingFaceProviderError::InvalidApiEndpoint)?;
        path.clear();
        path.extend(segments);
    }

    Ok(ProviderDiscovery::Item(SourceItem {
        source_type: "hugging_face_model".to_string(),
        source_item_id: Some(model.id.clone()),
        url: Some(public_url.to_string()),
        title: Some(model.id),
        snippet: None,
        authors: model.author.into_iter().collect(),
        discovered_via: Some(containing_url.to_string()),
        ..Default::default()
    }))
}

fn parse_rate_limit_reset(header: &str) -> Option<u64> {
    header.split(';').find_map(|part| {
        part.trim()
            .strip_prefix("t=")
            .and_then(|value| value.parse().ok())
    })
}

#[derive(serde::Deserialize)]
struct ModelSearchItem {
    id: String,
    author: Option<String>,
}

/// Deterministic local request-validation failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HuggingFaceRequestError {
    /// Search query is empty or whitespace-only.
    EmptyQuery,
    /// Scorpion's one-request safety bound is outside `1..=100`.
    LimitOutOfRange,
}

/// Typed provider-native Hugging Face discovery errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HuggingFaceProviderError {
    /// Request failed before a Hub response was available.
    RequestFailed(String),
    /// Hub returned its documented HTTP 429 rate-limit response.
    RateLimited {
        /// Seconds until reset parsed from the Hub `RateLimit` header.
        reset_after_seconds: Option<u64>,
    },
    /// Hub returned another non-success HTTP status.
    ProviderStatus(u16),
    /// Successful response did not match the model-list schema.
    ResponseDecode(String),
    /// Caller request failed deterministic pre-network validation.
    InvalidRequest(HuggingFaceRequestError),
    /// Explicit token could not be represented as an HTTP header.
    InvalidToken,
    /// Configured Hub base could not form the fixed API or public URL.
    InvalidApiEndpoint,
    /// Provider response contained no valid model repository identity.
    InvalidModelIdentity,
}

impl std::fmt::Display for HuggingFaceProviderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RequestFailed(error) => {
                write!(formatter, "Hugging Face request failed: {error}")
            }
            Self::RateLimited {
                reset_after_seconds,
            } => match reset_after_seconds {
                Some(reset) => write!(
                    formatter,
                    "Hugging Face rate limit exceeded; reset in {reset} seconds"
                ),
                None => formatter.write_str("Hugging Face rate limit exceeded"),
            },
            Self::ProviderStatus(status) => {
                write!(formatter, "Hugging Face returned HTTP status {status}")
            }
            Self::ResponseDecode(error) => {
                write!(formatter, "Hugging Face response decoding failed: {error}")
            }
            Self::InvalidRequest(HuggingFaceRequestError::EmptyQuery) => {
                formatter.write_str("Hugging Face model search query must not be empty")
            }
            Self::InvalidRequest(HuggingFaceRequestError::LimitOutOfRange) => {
                formatter.write_str("Hugging Face model search limit must be between 1 and 100")
            }
            Self::InvalidToken => {
                formatter.write_str("Hugging Face token is not a valid header value")
            }
            Self::InvalidApiEndpoint => {
                formatter.write_str("Hugging Face endpoint could not form a canonical URL")
            }
            Self::InvalidModelIdentity => {
                formatter.write_str("Hugging Face response contained an invalid model identity")
            }
        }
    }
}

impl std::error::Error for HuggingFaceProviderError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::source_provider::{ProviderId, SourceProviderRegistry};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    struct Fixture {
        base_url: url::Url,
        hits: Arc<AtomicUsize>,
        request: Arc<Mutex<String>>,
    }

    async fn fixture(status: u16, headers: &[(&str, &str)], body: &str) -> Fixture {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_for_task = hits.clone();
        let request = Arc::new(Mutex::new(String::new()));
        let request_for_task = request.clone();
        let headers = headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}\r\n"))
            .collect::<String>();
        let body = body.to_string();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            hits_for_task.fetch_add(1, Ordering::SeqCst);
            let mut bytes = vec![0; 8192];
            let length = stream.read(&mut bytes).await.unwrap();
            *request_for_task.lock().unwrap() =
                String::from_utf8_lossy(&bytes[..length]).into_owned();
            let response = format!(
                "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        Fixture {
            base_url: url::Url::parse(&format!("http://{address}/")).unwrap(),
            hits,
            request,
        }
    }

    fn provider(fixture: &Fixture) -> HuggingFaceModelProvider {
        HuggingFaceModelProvider::with_client_and_base(
            reqwest::Client::new(),
            fixture.base_url.clone(),
        )
    }

    #[test]
    fn canonical_descriptor_registers_without_registry_special_case() {
        let provider = HuggingFaceModelProvider::new();
        assert_eq!(provider.descriptor().id, ProviderId::from("hugging_face"));
        assert_eq!(provider.descriptor().display_name, "Hugging Face");
        assert_eq!(
            provider.descriptor().capabilities,
            ProviderCapabilities::ITEMS
        );
        let mut registry = SourceProviderRegistry::new();
        registry.register(provider.descriptor().clone()).unwrap();
        assert_eq!(
            registry.get(&ProviderId::from("hugging_face")),
            Some(provider.descriptor())
        );
    }

    #[test]
    fn request_default_and_safety_bounds_are_deterministic() {
        let default = HuggingFaceModelSearchRequest::new("model");
        assert_eq!(default.limit, 10);
        assert_eq!(validate_request(&default), Ok(()));
        assert_eq!(validate_request(&default.clone().with_limit(1)), Ok(()));
        assert_eq!(validate_request(&default.clone().with_limit(100)), Ok(()));
        for limit in [0, 101] {
            assert_eq!(
                validate_request(&default.clone().with_limit(limit)),
                Err(HuggingFaceProviderError::InvalidRequest(
                    HuggingFaceRequestError::LimitOutOfRange
                ))
            );
        }
    }

    #[tokio::test]
    async fn model_search_preserves_request_order_duplicates_and_native_fields() {
        let body = r#"[{"id":"acme/one","author":"acme","tags":["ignored"],"lastModified":"2026-01-01T00:00:00Z","siblings":[{"rfilename":"model.safetensors"}]},{"id":"acme/one","author":"acme"},{"id":"solo-model","author":null}]"#;
        let server = fixture(200, &[], body).await;
        let outputs = provider(&server)
            .search_models(&HuggingFaceModelSearchRequest::new("text model").with_limit(3))
            .await
            .unwrap();

        assert_eq!(server.hits.load(Ordering::SeqCst), 1);
        let raw_request = server.request.lock().unwrap().clone();
        assert!(raw_request.starts_with("GET /api/models?search=text+model&limit=3 HTTP/1.1"));
        let items = outputs
            .iter()
            .map(|output| match output {
                ProviderDiscovery::Item(item) => item,
                ProviderDiscovery::Target(_) | ProviderDiscovery::Artifact(_) => {
                    panic!("Hub model metadata is an Item")
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], items[1], "provider duplicates are preserved");
        assert_eq!(items[0].source_type, "hugging_face_model");
        assert_eq!(items[0].source_item_id.as_deref(), Some("acme/one"));
        assert_eq!(
            items[0].url.as_deref(),
            Some("https://huggingface.co/acme/one")
        );
        assert_eq!(items[0].title.as_deref(), Some("acme/one"));
        assert_eq!(items[0].authors, ["acme"]);
        assert_eq!(items[0].snippet, None);
        assert_eq!(items[0].published_at, None);
        assert_eq!(items[0].updated_at, None);
        assert!(items[0].media_references.is_empty());
        assert!(items[0]
            .discovered_via
            .as_deref()
            .unwrap()
            .contains("/api/models?"));
        assert!(items[2].authors.is_empty());
    }

    #[tokio::test]
    async fn malformed_and_http_failures_are_typed_not_empty_success() {
        let malformed = fixture(200, &[], "not-json").await;
        let error = provider(&malformed)
            .search_models(&HuggingFaceModelSearchRequest::new("model"))
            .await
            .unwrap_err();
        assert!(matches!(error, HuggingFaceProviderError::ResponseDecode(_)));

        let failed = fixture(503, &[], r#"{"error":"unavailable"}"#).await;
        let error = provider(&failed)
            .search_models(&HuggingFaceModelSearchRequest::new("model"))
            .await
            .unwrap_err();
        assert_eq!(error, HuggingFaceProviderError::ProviderStatus(503));
    }

    #[tokio::test]
    async fn documented_rate_limit_is_explicit_and_never_retried() {
        let server = fixture(
            429,
            &[("ratelimit", "\"api|pages|resolvers\";r=0;t=37")],
            r#"{"error":"rate limited"}"#,
        )
        .await;
        let error = provider(&server)
            .search_models(&HuggingFaceModelSearchRequest::new("model"))
            .await
            .unwrap_err();
        assert_eq!(
            error,
            HuggingFaceProviderError::RateLimited {
                reset_after_seconds: Some(37)
            }
        );
        assert_eq!(server.hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn invalid_request_fails_before_network() {
        let server = fixture(200, &[], "[]").await;
        let error = provider(&server)
            .search_models(&HuggingFaceModelSearchRequest::new(" "))
            .await
            .unwrap_err();
        assert_eq!(
            error,
            HuggingFaceProviderError::InvalidRequest(HuggingFaceRequestError::EmptyQuery)
        );
        assert_eq!(server.hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn token_is_header_only_and_secret_safe() {
        const SECRET: &str = "hf_secret_sentinel";
        let server = fixture(503, &[], r#"{"error":"unavailable"}"#).await;
        let provider = provider(&server).with_token(SECRET).unwrap();
        assert!(!format!("{provider:?}").contains(SECRET));
        let error = provider
            .search_models(&HuggingFaceModelSearchRequest::new("model"))
            .await
            .unwrap_err();
        assert!(!format!("{error:?}").contains(SECRET));
        assert!(!error.to_string().contains(SECRET));
        let raw_request = server.request.lock().unwrap();
        assert!(raw_request.contains("authorization: Bearer hf_secret_sentinel"));
        assert!(!raw_request.starts_with("GET /api/models?token="));
    }
}
