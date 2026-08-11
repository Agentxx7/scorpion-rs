//! Provider-native GitHub repository discovery through the official REST API.
//!
//! This is deliberately not a generic acquisition client: it performs one
//! bounded GitHub repository-search request, normalizes API metadata into
//! [`ProviderDiscovery::Item`], and stops. It never fetches repository URLs,
//! invokes a parser, selects a transport, or constructs evidence.
//!
//! Network execution is delegated to Scorpion's canonical streaming transport
//! seam ([`crate::features::transport::execute_streaming_request`]) so the
//! provider owns only GitHub-specific request construction and response
//! parsing.

use crate::features::secret_request_headers::SecretRequestHeaders;
use crate::features::source::SourceItem;
use crate::features::source_provider::{
    ProviderCapabilities, ProviderDescriptor, ProviderDiscovery, SourceProvider,
};
use crate::features::transport::TransportPolicy;
use reqwest::header::{HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};

const GITHUB_PROVIDER_ID: &str = "github";
const GITHUB_API_BASE: &str = "https://api.github.com/";
const DEFAULT_LIMIT: usize = 10;
const MAX_LIMIT: usize = 100;

/// One bounded repository-search request. Pagination beyond this single page
/// is intentionally absent from the first provider frontier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubRepositorySearchRequest {
    /// GitHub repository-search query using GitHub's native query grammar.
    pub query: String,
    /// Number of results requested from the one API page (`1..=100`).
    pub limit: usize,
}

impl GitHubRepositorySearchRequest {
    /// Construct a request using the conservative default page size of 10.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            limit: DEFAULT_LIMIT,
        }
    }

    /// Set the one-page result limit. Validation occurs before network access.
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }
}

/// Official GitHub REST repository-search provider.
pub struct GitHubRepositoryProvider {
    descriptor: ProviderDescriptor,
    transport: TransportPolicy,
    api_base: url::Url,
    headers: SecretRequestHeaders,
    authenticated: bool,
}

impl std::fmt::Debug for GitHubRepositoryProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GitHubRepositoryProvider")
            .field("descriptor", &self.descriptor)
            .field("api_base", &self.api_base)
            .field("transport", &self.transport)
            .field("authenticated", &self.authenticated)
            .finish_non_exhaustive()
    }
}

impl Default for GitHubRepositoryProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl GitHubRepositoryProvider {
    /// Construct an unauthenticated provider using GitHub's official API.
    pub fn new() -> Self {
        Self::with_base(url::Url::parse(GITHUB_API_BASE).expect("static GitHub API URL is valid"))
    }

    fn with_base(api_base: url::Url) -> Self {
        let mut headers = SecretRequestHeaders::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static("scorpion-source-provider"),
        );
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            HeaderName::from_static("x-github-api-version"),
            HeaderValue::from_static("2022-11-28"),
        );
        Self {
            descriptor: ProviderDescriptor::new(
                GITHUB_PROVIDER_ID,
                "GitHub",
                ProviderCapabilities::ITEMS,
            ),
            transport: TransportPolicy::default(),
            api_base,
            headers,
            authenticated: false,
        }
    }

    /// Select the transport policy used for network execution.
    pub fn with_transport(mut self, transport: TransportPolicy) -> Self {
        self.transport = transport;
        self
    }

    /// Attach an explicitly supplied token in memory. The token is converted
    /// immediately into a sensitive HTTP header value and is never stored in
    /// descriptor, request vocabulary, outputs, URLs, Debug, Display, or
    /// errors. No environment or persistent credential store is consulted.
    pub fn with_token(mut self, token: &str) -> Result<Self, GitHubProviderError> {
        let value = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| GitHubProviderError::InvalidToken)?;
        self.headers.insert(AUTHORIZATION, value);
        self.authenticated = true;
        Ok(self)
    }

    /// Execute exactly one official repository-search request and return
    /// results in GitHub's response order, including duplicates if supplied.
    pub async fn search_repositories(
        &self,
        request: &GitHubRepositorySearchRequest,
    ) -> Result<Vec<ProviderDiscovery>, GitHubProviderError> {
        validate_request(request)?;

        let mut endpoint = self
            .api_base
            .join("search/repositories")
            .map_err(|_| GitHubProviderError::InvalidApiEndpoint)?;
        endpoint
            .query_pairs_mut()
            .append_pair("q", &request.query)
            .append_pair("per_page", &request.limit.to_string());

        let response = self.execute_request(endpoint).await?;
        let status = response.status();
        let rate_limit_remaining = response
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        let rate_limit_reset = response
            .headers()
            .get("x-ratelimit-reset")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS
            || (status == reqwest::StatusCode::FORBIDDEN && rate_limit_remaining == Some(0))
        {
            return Err(GitHubProviderError::RateLimited {
                reset_epoch_seconds: rate_limit_reset,
            });
        }
        if !status.is_success() {
            return Err(GitHubProviderError::ProviderStatus(status.as_u16()));
        }

        let containing_url = response.url().to_string();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| GitHubProviderError::RequestFailed(error.to_string()))?;
        let payload: RepositorySearchResponse = serde_json::from_slice(&bytes)
            .map_err(|error| GitHubProviderError::ResponseDecode(error.to_string()))?;

        Ok(payload
            .items
            .into_iter()
            .map(|repository| {
                ProviderDiscovery::Item(SourceItem {
                    source_type: "github_repository".to_string(),
                    source_item_id: Some(repository.id.to_string()),
                    url: Some(repository.html_url),
                    title: Some(repository.full_name),
                    snippet: repository.description,
                    authors: vec![repository.owner.login],
                    discovered_via: Some(containing_url.clone()),
                    ..Default::default()
                })
            })
            .collect())
    }

    /// Dispatch one GitHub API request through Scorpion's canonical transport
    /// seam. Under unsupported feature combinations the provider fails closed
    /// rather than constructing an independent HTTP stack.
    async fn execute_request(
        &self,
        endpoint: url::Url,
    ) -> Result<reqwest::Response, GitHubProviderError> {
        #[cfg(all(not(feature = "wreq"), not(feature = "cache_request")))]
        {
            crate::features::transport::execute_streaming_request(
                &endpoint,
                &self.transport,
                &self.headers,
            )
            .await
            .map_err(|error| GitHubProviderError::RequestFailed(error.to_string()))
        }
        #[cfg(any(feature = "wreq", feature = "cache_request"))]
        {
            let _ = endpoint;
            Err(GitHubProviderError::RequestFailed(
                "GitHub provider is unavailable under wreq or cache_request".to_string(),
            ))
        }
    }
}

impl SourceProvider for GitHubRepositoryProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }
}

fn validate_request(request: &GitHubRepositorySearchRequest) -> Result<(), GitHubProviderError> {
    if request.query.trim().is_empty() {
        return Err(GitHubProviderError::InvalidRequest(
            GitHubRequestError::EmptyQuery,
        ));
    }
    if !(1..=MAX_LIMIT).contains(&request.limit) {
        return Err(GitHubProviderError::InvalidRequest(
            GitHubRequestError::LimitOutOfRange,
        ));
    }
    Ok(())
}

#[derive(serde::Deserialize)]
struct RepositorySearchResponse {
    items: Vec<RepositorySearchItem>,
}

#[derive(serde::Deserialize)]
struct RepositorySearchItem {
    id: u64,
    html_url: String,
    full_name: String,
    description: Option<String>,
    owner: RepositoryOwner,
}

#[derive(serde::Deserialize)]
struct RepositoryOwner {
    login: String,
}

/// Deterministic request-validation failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitHubRequestError {
    /// Search query is empty or whitespace-only.
    EmptyQuery,
    /// One-page result limit is outside GitHub's `1..=100` contract.
    LimitOutOfRange,
}

/// Typed provider-native GitHub discovery errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitHubProviderError {
    /// Request failed before a GitHub response was available.
    RequestFailed(String),
    /// GitHub rejected the request due to rate limiting.
    RateLimited {
        /// Provider-declared Unix reset time, when supplied and parseable.
        reset_epoch_seconds: Option<u64>,
    },
    /// GitHub returned another non-success HTTP status.
    ProviderStatus(u16),
    /// Successful response did not match the selected GitHub API schema.
    ResponseDecode(String),
    /// Caller request failed deterministic local validation.
    InvalidRequest(GitHubRequestError),
    /// Explicit token could not be represented as an HTTP header.
    InvalidToken,
    /// Configured API endpoint could not form the fixed search path.
    InvalidApiEndpoint,
}

impl std::fmt::Display for GitHubProviderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RequestFailed(error) => write!(formatter, "GitHub request failed: {error}"),
            Self::RateLimited {
                reset_epoch_seconds,
            } => match reset_epoch_seconds {
                Some(reset) => write!(formatter, "GitHub rate limit exceeded; reset at {reset}"),
                None => formatter.write_str("GitHub rate limit exceeded"),
            },
            Self::ProviderStatus(status) => {
                write!(formatter, "GitHub returned HTTP status {status}")
            }
            Self::ResponseDecode(error) => {
                write!(formatter, "GitHub response decoding failed: {error}")
            }
            Self::InvalidRequest(GitHubRequestError::EmptyQuery) => {
                formatter.write_str("GitHub repository search query must not be empty")
            }
            Self::InvalidRequest(GitHubRequestError::LimitOutOfRange) => {
                formatter.write_str("GitHub repository search limit must be between 1 and 100")
            }
            Self::InvalidToken => formatter.write_str("GitHub token is not a valid header value"),
            Self::InvalidApiEndpoint => {
                formatter.write_str("GitHub API endpoint could not form repository search URL")
            }
        }
    }
}

impl std::error::Error for GitHubProviderError {}

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

    fn provider(fixture: &Fixture) -> GitHubRepositoryProvider {
        GitHubRepositoryProvider::with_base(fixture.base_url.clone())
    }

    #[test]
    fn canonical_descriptor_registers_without_registry_special_case() {
        let provider = GitHubRepositoryProvider::new();
        assert_eq!(provider.descriptor().id, ProviderId::from("github"));
        assert_eq!(provider.descriptor().display_name, "GitHub");
        assert_eq!(
            provider.descriptor().capabilities,
            ProviderCapabilities::ITEMS
        );

        let mut registry = SourceProviderRegistry::new();
        registry.register(provider.descriptor().clone()).unwrap();
        assert_eq!(
            registry.get(&ProviderId::from("github")),
            Some(provider.descriptor())
        );
    }

    #[tokio::test]
    async fn repository_search_preserves_request_order_duplicates_and_native_fields() {
        let body = r#"{"items":[{"id":42,"html_url":"https://github.com/acme/one","full_name":"acme/one","description":"First","owner":{"login":"acme"}},{"id":42,"html_url":"https://github.com/acme/one","full_name":"acme/one","description":"First","owner":{"login":"acme"}},{"id":7,"html_url":"https://github.com/beta/two","full_name":"beta/two","description":null,"owner":{"login":"beta"}}]}"#;
        let server = fixture(200, &[], body).await;
        let outputs = provider(&server)
            .search_repositories(&GitHubRepositorySearchRequest::new("rust crawler").with_limit(3))
            .await
            .unwrap();

        assert_eq!(server.hits.load(Ordering::SeqCst), 1);
        let raw_request = server.request.lock().unwrap().clone();
        assert!(
            raw_request.starts_with("GET /search/repositories?q=rust+crawler&per_page=3 HTTP/1.1")
        );
        assert!(raw_request.contains("x-github-api-version: 2022-11-28"));
        assert_eq!(outputs.len(), 3);
        let items = outputs
            .iter()
            .map(|output| match output {
                ProviderDiscovery::Item(item) => item,
                ProviderDiscovery::Target(_) | ProviderDiscovery::Artifact(_) => {
                    panic!("repository metadata is an Item")
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(items[0], items[1], "provider duplicates are preserved");
        assert_eq!(items[0].source_type, "github_repository");
        assert_eq!(items[0].source_item_id.as_deref(), Some("42"));
        assert_eq!(items[0].url.as_deref(), Some("https://github.com/acme/one"));
        assert_eq!(items[0].title.as_deref(), Some("acme/one"));
        assert_eq!(items[0].snippet.as_deref(), Some("First"));
        assert_eq!(items[0].authors, ["acme"]);
        assert!(items[0]
            .discovered_via
            .as_deref()
            .unwrap()
            .contains("/search/repositories?"));
        assert_eq!(items[2].source_item_id.as_deref(), Some("7"));
        assert_eq!(items[2].snippet, None);
        assert_eq!(items[2].published_at, None);
        assert!(items[2].media_references.is_empty());
    }

    #[tokio::test]
    async fn malformed_and_http_failures_are_typed_not_empty_success() {
        let malformed = fixture(200, &[], "not-json").await;
        let error = provider(&malformed)
            .search_repositories(&GitHubRepositorySearchRequest::new("rust"))
            .await
            .unwrap_err();
        assert!(matches!(error, GitHubProviderError::ResponseDecode(_)));

        let failed = fixture(500, &[], r#"{"message":"failure"}"#).await;
        let error = provider(&failed)
            .search_repositories(&GitHubRepositorySearchRequest::new("rust"))
            .await
            .unwrap_err();
        assert_eq!(error, GitHubProviderError::ProviderStatus(500));
    }

    #[tokio::test]
    async fn rate_limit_is_explicit_and_never_retried() {
        let server = fixture(
            403,
            &[("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", "123")],
            r#"{"message":"rate limit"}"#,
        )
        .await;
        let error = provider(&server)
            .search_repositories(&GitHubRepositorySearchRequest::new("rust"))
            .await
            .unwrap_err();
        assert_eq!(
            error,
            GitHubProviderError::RateLimited {
                reset_epoch_seconds: Some(123)
            }
        );
        assert_eq!(server.hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn invalid_request_fails_before_network() {
        let server = fixture(200, &[], r#"{"items":[]}"#).await;
        let provider = provider(&server);
        let error = provider
            .search_repositories(&GitHubRepositorySearchRequest::new(" "))
            .await
            .unwrap_err();
        assert_eq!(
            error,
            GitHubProviderError::InvalidRequest(GitHubRequestError::EmptyQuery)
        );
        assert_eq!(server.hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn token_is_header_only_and_secret_safe_in_debug_display_and_errors() {
        const SECRET: &str = "github_secret_sentinel";
        let server = fixture(500, &[], r#"{"message":"failure"}"#).await;
        let provider = provider(&server).with_token(SECRET).unwrap();
        assert!(!format!("{provider:?}").contains(SECRET));
        let error = provider
            .search_repositories(&GitHubRepositorySearchRequest::new("rust"))
            .await
            .unwrap_err();
        assert!(!format!("{error:?}").contains(SECRET));
        assert!(!error.to_string().contains(SECRET));
        let raw_request = server.request.lock().unwrap();
        assert!(raw_request.contains("authorization: Bearer github_secret_sentinel"));
        assert!(!raw_request.starts_with("GET /search/repositories?token="));
    }
}
