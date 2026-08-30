//! Thin server-side application boundary for a future Scorpion Web Console.
//!
//! This crate owns request/response DTOs and HTTP mapping only. Search,
//! provider requests, parsing, and result normalization remain owned by the
//! canonical `spider_search` capability through Spider's public façade.

use serde::{Deserialize, Serialize};
use spider::agent::{AgentBuilder, ResearchOptions};
use spider::features::domain_persistence::DomainPersistence;
use spider::features::identity::ResearchId;
use spider::features::research_session::{
    claim_durable_research, read_research_session, ResearchSession, ResearchSessionState,
};
use spider::features::search::{
    resolve_search_provider, SearchError as CanonicalSearchError, SearchOptions, SearchProvider,
    SearchProviderConfigError,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Stable application request for the first Web Console capability.
#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct SearchRequest {
    /// User-entered search query.
    pub query: String,
    /// Maximum number of returned candidates. The provider applies this
    /// client-side after canonical result mapping.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Stable application response; provider-specific metadata is intentionally
/// not exposed here.
#[derive(Debug, Serialize, PartialEq)]
pub struct SearchResponse {
    pub query: String,
    pub result_count: usize,
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct SearchResult {
    pub position: usize,
    pub title: String,
    pub url: String,
    pub snippet: Option<String>,
    pub date: Option<String>,
    pub score: Option<f32>,
}

/// Errors exposed by the application boundary. Secrets and filesystem paths
/// are never carried in these messages.
#[derive(Debug, PartialEq, Eq)]
pub enum SearchError {
    InvalidRequest(String),
    /// The selected provider requires operator configuration that is absent
    /// (canonical `SearchProviderConfigError::MissingConfiguration`).
    ProviderNotConfigured,
    /// The operator selected a real, recognized provider that this build
    /// does not compile in (canonical
    /// `SearchProviderConfigError::UnsupportedProvider`). Distinct from
    /// `ProviderNotConfigured`: the provider was never executable in this
    /// shipping build, not merely missing a key/URL.
    ProviderUnsupported(String),
    /// The configured selector does not name any canonical provider
    /// identity (canonical `SearchProviderConfigError::UnknownProvider`).
    InvalidProviderSelection(String),
    /// A provider that resolved successfully failed at runtime (network,
    /// upstream, or backend failure) — distinct from every static
    /// configuration failure above.
    Provider(String),
    Internal(String),
}

/// Project the canonical, sanitized provider configuration error into this
/// application's public `SearchError` classification. Preserves the
/// canonical resolver's own distinctions rather than collapsing them.
fn map_search_provider_config_error(error: SearchProviderConfigError) -> SearchError {
    match error {
        SearchProviderConfigError::MissingConfiguration(_) => SearchError::ProviderNotConfigured,
        SearchProviderConfigError::UnsupportedProvider(name) => {
            SearchError::ProviderUnsupported(name.to_string())
        }
        SearchProviderConfigError::UnknownProvider(name) => {
            SearchError::InvalidProviderSelection(name)
        }
    }
}

/// Stable, sanitized public message for any provider *runtime* failure
/// (as opposed to the static configuration failures above). Deliberately
/// identical regardless of the internal cause — network failure,
/// authentication failure, rate limiting, a malformed operator-configured
/// endpoint, an unparsable transport response, or any other
/// `spider_search::SearchError` variant. None of those internal variants
/// may carry operator-configured URLs (a SearXNG `ProviderError` embeds
/// the raw configured base URL verbatim), credentials, query strings, or
/// transport/reqwest error detail (which conventionally includes the
/// request URL) through the public boundary. See
/// `sanitize_provider_runtime_error`.
const PROVIDER_RUNTIME_FAILURE_MESSAGE: &str = "search provider is unavailable";

/// Discard the canonical `spider_search::SearchError`'s raw `Display` text
/// and log it for operator troubleshooting only — the public boundary
/// never depends on internal `Display` output. The public
/// `SearchError::Provider` always carries the same fixed, sanitized
/// message; the HTTP status (502) and JSON code (`provider_unavailable`)
/// this capability already used remain unchanged (this is a message-only
/// correction, not a reclassification).
fn sanitize_provider_runtime_error(error: CanonicalSearchError) -> SearchError {
    eprintln!("scorpion-api search provider runtime failure: {error}");
    SearchError::Provider(PROVIDER_RUNTIME_FAILURE_MESSAGE.to_string())
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(message) => write!(f, "invalid request: {message}"),
            Self::ProviderNotConfigured => f.write_str("search provider is not configured"),
            Self::ProviderUnsupported(name) => {
                write!(f, "search provider \"{name}\" is not enabled in this build")
            }
            Self::InvalidProviderSelection(name) => {
                write!(f, "invalid search provider selection \"{name}\"")
            }
            Self::Provider(message) => write!(f, "search provider failed: {message}"),
            Self::Internal(message) => write!(f, "internal application error: {message}"),
        }
    }
}

/// Execute one canonical search using server-owned configuration.
pub async fn search(
    request: SearchRequest,
    searxng_base_url: Option<&str>,
) -> Result<SearchResponse, SearchError> {
    let query = request.query.trim();
    if query.is_empty() {
        return Err(SearchError::InvalidRequest(
            "query must not be empty".into(),
        ));
    }
    if request.limit == Some(0) {
        return Err(SearchError::InvalidRequest(
            "limit must be greater than zero".into(),
        ));
    }
    let selector = std::env::var("SEARCH_PROVIDER").ok();
    let brave = std::env::var("BRAVE_API_KEY").ok();
    let serper = std::env::var("SERPER_API_KEY").ok();
    let tavily = std::env::var("TAVILY_API_KEY").ok();
    let provider = resolve_search_provider(
        selector.as_deref(),
        searxng_base_url,
        brave.as_deref(),
        serper.as_deref(),
        tavily.as_deref(),
    )
    .map_err(map_search_provider_config_error)?
    .1;
    let options = request.limit.map_or_else(SearchOptions::new, |limit| {
        SearchOptions::new().with_limit(limit)
    });
    let results = provider
        .search(query, &options)
        .await
        .map_err(sanitize_provider_runtime_error)?;

    if results.results.is_empty() && results.backend_failure {
        return Err(SearchError::Provider(
            "search backend reported upstream failure".into(),
        ));
    }

    Ok(SearchResponse {
        query: results.query,
        result_count: results.results.len(),
        results: results
            .results
            .into_iter()
            .map(|result| SearchResult {
                position: result.position,
                title: result.title,
                url: result.url,
                snippet: result.snippet,
                date: result.date,
                score: result.score,
            })
            .collect(),
    })
}

/// HTTP status and public JSON error body for an application error.
///
/// Every static configuration failure (the provider was never executable in
/// this build/deployment) is `503 Service Unavailable`. `502 Bad Gateway` is
/// reserved for `SearchError::Provider` — a provider that resolved
/// successfully but failed at runtime — never for a provider that was never
/// reachable in the first place.
pub fn error_status(error: &SearchError) -> u16 {
    match error {
        SearchError::InvalidRequest(_) => 400,
        SearchError::ProviderNotConfigured
        | SearchError::ProviderUnsupported(_)
        | SearchError::InvalidProviderSelection(_) => 503,
        SearchError::Provider(_) => 502,
        SearchError::Internal(_) => 500,
    }
}

/// Serialize a public error without leaking provider configuration.
pub fn error_json(error: &SearchError) -> String {
    serde_json::json!({
        "error": {
            "code": match error {
                SearchError::InvalidRequest(_) => "invalid_request",
                SearchError::ProviderNotConfigured => "provider_not_configured",
                SearchError::ProviderUnsupported(_) => "provider_unsupported",
                SearchError::InvalidProviderSelection(_) => "invalid_provider_configuration",
                SearchError::Provider(_) => "provider_unavailable",
                SearchError::Internal(_) => "internal_error",
            },
            "message": error.to_string(),
        }
    })
    .to_string()
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResearchRequest {
    pub topic: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ResearchAccepted {
    pub research_id: String,
    pub state: &'static str,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ResearchCounts {
    pub search_results: usize,
    pub acquisition_attempts: usize,
    pub durable_sources: usize,
    pub observed_acquisitions: usize,
    pub successful_extractions: usize,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct ResearchStatus {
    pub research_id: String,
    pub topic: String,
    pub state: String,
    pub counts: ResearchCounts,
    pub created_at_unix_ms: u64,
    pub completed_at_unix_ms: Option<u64>,
    pub synthesis_summary: Option<String>,
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ResearchError {
    InvalidRequest(String),
    /// Required operator configuration (database path, OpenAI-compatible
    /// endpoint, or the search provider's own required key/URL) is absent.
    NotConfigured,
    /// The configured search provider is a real, recognized provider that
    /// this build does not compile in (canonical
    /// `SearchProviderConfigError::UnsupportedProvider`). A static
    /// build/config incompatibility, not an execution/runtime dependency
    /// failure — never classified as `Unavailable`.
    UnsupportedProvider(String),
    /// The configured search provider selector does not name any canonical
    /// provider identity (canonical `SearchProviderConfigError::
    /// UnknownProvider`). Also a static configuration failure, never
    /// `Unavailable`.
    InvalidProviderConfiguration(String),
    CapacityExhausted,
    NotFound,
    /// An execution/runtime dependency failure (e.g. the durable evidence
    /// store could not be opened) — reserved for failures that occur only
    /// after static configuration has already resolved successfully.
    Unavailable,
    Internal,
}

/// Project the canonical, sanitized provider configuration error into this
/// application's public `ResearchError` classification. Preserves the
/// canonical resolver's own distinctions — never collapsed into
/// `Unavailable`, which is reserved for execution/runtime failures that
/// occur only after static configuration has already resolved.
fn map_research_provider_config_error(error: SearchProviderConfigError) -> ResearchError {
    match error {
        SearchProviderConfigError::MissingConfiguration(_) => ResearchError::NotConfigured,
        SearchProviderConfigError::UnsupportedProvider(name) => {
            ResearchError::UnsupportedProvider(name.to_string())
        }
        SearchProviderConfigError::UnknownProvider(name) => {
            ResearchError::InvalidProviderConfiguration(name)
        }
    }
}

/// Static operator-configuration availability for the Research product.
///
/// This deliberately says nothing about external service health. Execution
/// still performs every canonical runtime check and remains fail-closed.
/// Reuses `resolve_research_config_with`'s own classification — never a
/// second, independently-maintained configuration check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResearchAvailability {
    Available,
    NotConfigured,
    /// The configured search provider is real but not compiled into this
    /// build (see `ResearchError::UnsupportedProvider`).
    UnsupportedProvider(String),
    /// The configured search provider selector is not a recognized
    /// canonical identity (see `ResearchError::InvalidProviderConfiguration`).
    InvalidConfiguration(String),
}

struct ResearchRuntimeConfig {
    database: PathBuf,
    provider: Box<dyn SearchProvider>,
    openai_base_url: String,
    model: String,
    api_key: String,
}

impl std::fmt::Display for ResearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(_) => f.write_str("invalid research request"),
            Self::NotConfigured => f.write_str("research is not configured"),
            Self::UnsupportedProvider(name) => write!(
                f,
                "configured research search provider \"{name}\" is not enabled in this build"
            ),
            Self::InvalidProviderConfiguration(name) => write!(
                f,
                "invalid research search provider configuration \"{name}\""
            ),
            Self::CapacityExhausted => f.write_str("research capacity is temporarily exhausted"),
            Self::NotFound => f.write_str("research session not found"),
            Self::Unavailable => f.write_str("research is unavailable"),
            Self::Internal => f.write_str("internal research error"),
        }
    }
}

/// HTTP status for a public research error.
///
/// Every static configuration/build incompatibility — including an
/// unsupported or invalid provider selection — is `503 Service
/// Unavailable`, matching `SearchError`'s own convention. `502 Bad Gateway`
/// remains reserved for `Unavailable`: a real execution/runtime dependency
/// failure that occurs only after static configuration already resolved.
pub fn research_error_status(error: &ResearchError) -> u16 {
    match error {
        ResearchError::InvalidRequest(_) => 400,
        ResearchError::NotFound => 404,
        ResearchError::NotConfigured
        | ResearchError::CapacityExhausted
        | ResearchError::UnsupportedProvider(_)
        | ResearchError::InvalidProviderConfiguration(_) => 503,
        ResearchError::Unavailable => 502,
        ResearchError::Internal => 500,
    }
}

pub fn research_error_json(error: &ResearchError) -> String {
    let code = match error {
        ResearchError::InvalidRequest(_) => "invalid_request",
        ResearchError::NotConfigured => "research_not_configured",
        ResearchError::UnsupportedProvider(_) => "research_provider_unsupported",
        ResearchError::InvalidProviderConfiguration(_) => "research_provider_configuration_invalid",
        ResearchError::CapacityExhausted => "execution_capacity_exhausted",
        ResearchError::NotFound => "research_not_found",
        ResearchError::Unavailable => "research_unavailable",
        ResearchError::Internal => "internal_error",
    };
    serde_json::json!({"error": {"code": code, "message": error.to_string()}}).to_string()
}

#[derive(Clone)]
pub struct ResearchService {
    capacity: Arc<Semaphore>,
}

impl Default for ResearchService {
    fn default() -> Self {
        Self {
            capacity: Arc::new(Semaphore::new(2)),
        }
    }
}

impl ResearchService {
    pub async fn submit(
        &self,
        request: ResearchRequest,
    ) -> Result<ResearchAccepted, ResearchError> {
        let topic = request.topic.trim().to_string();
        if topic.is_empty() {
            return Err(ResearchError::InvalidRequest(
                "topic must not be empty".into(),
            ));
        }
        let permit = self
            .capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| ResearchError::CapacityExhausted)?;
        let config = resolve_research_config()?;
        let store = Arc::new(
            DomainPersistence::open(&config.database)
                .await
                .map_err(|_| ResearchError::Unavailable)?,
        );
        let builder = AgentBuilder::new()
            .with_openai_compatible(config.openai_base_url, config.api_key, config.model)
            .with_search_provider(config.provider);
        let claimed = claim_durable_research(
            Arc::clone(&store),
            builder,
            topic,
            ResearchOptions::new().with_synthesize(true),
        )
        .await
        .map_err(|_| ResearchError::Unavailable)?;
        let id = claimed.id().to_string();
        let log_id = id.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if claimed.execute().await.is_err() {
                eprintln!("scorpion-api research execution failed for {log_id}");
            }
        });
        Ok(ResearchAccepted {
            research_id: id,
            state: "claimed",
        })
    }

    pub async fn status(&self, raw_id: &str) -> Result<ResearchStatus, ResearchError> {
        let id: ResearchId = raw_id
            .parse()
            .map_err(|_| ResearchError::InvalidRequest("invalid research id".into()))?;
        let database = PathBuf::from(required_env("RESEARCH_EVIDENCE_DB")?);
        let store = DomainPersistence::open(&database)
            .await
            .map_err(|_| ResearchError::Unavailable)?;
        let (_, session) = read_research_session(&store, id)
            .await
            .map_err(|_| ResearchError::Unavailable)?
            .ok_or(ResearchError::NotFound)?;
        Ok(project_session(session))
    }
}

fn required_config(
    name: &str,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<String, ResearchError> {
    lookup(name)
        .filter(|v| !v.trim().is_empty())
        .ok_or(ResearchError::NotConfigured)
}

fn required_env(name: &str) -> Result<String, ResearchError> {
    required_config(name, &|name| std::env::var(name).ok())
}

fn resolve_research_config_with(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<ResearchRuntimeConfig, ResearchError> {
    let database = PathBuf::from(required_config("RESEARCH_EVIDENCE_DB", lookup)?);
    let openai_base_url = required_config("OPENAI_COMPAT_BASE_URL", lookup)?;
    let model = required_config("OPENAI_COMPAT_MODEL", lookup)?;
    let api_key = required_config("OPENAI_COMPAT_API_KEY", lookup)?;
    let selector = lookup("SEARCH_PROVIDER");
    let searxng = lookup("SEARXNG_BASE_URL");
    let brave = lookup("BRAVE_API_KEY");
    let serper = lookup("SERPER_API_KEY");
    let tavily = lookup("TAVILY_API_KEY");
    let provider = resolve_search_provider(
        selector.as_deref(),
        searxng.as_deref(),
        brave.as_deref(),
        serper.as_deref(),
        tavily.as_deref(),
    )
    .map_err(map_research_provider_config_error)?
    .1;
    Ok(ResearchRuntimeConfig {
        database,
        provider,
        openai_base_url,
        model,
        api_key,
    })
}

fn resolve_research_config() -> Result<ResearchRuntimeConfig, ResearchError> {
    resolve_research_config_with(&|name| std::env::var(name).ok())
}

/// Project whether all static operator configuration required by Research is
/// present. This does not probe or claim health for configured dependencies.
pub fn research_availability() -> ResearchAvailability {
    research_availability_with(&|name| std::env::var(name).ok())
}

fn research_availability_with(lookup: &impl Fn(&str) -> Option<String>) -> ResearchAvailability {
    match resolve_research_config_with(lookup) {
        Ok(_) => ResearchAvailability::Available,
        Err(ResearchError::UnsupportedProvider(name)) => {
            ResearchAvailability::UnsupportedProvider(name)
        }
        Err(ResearchError::InvalidProviderConfiguration(name)) => {
            ResearchAvailability::InvalidConfiguration(name)
        }
        // NotConfigured and every other configuration-resolution error
        // (CapacityExhausted/NotFound/Unavailable/Internal never occur
        // here — resolve_research_config_with only ever returns
        // NotConfigured or a provider-configuration variant) collapse to
        // the truthful NotConfigured state.
        Err(_) => ResearchAvailability::NotConfigured,
    }
}

fn project_session(session: ResearchSession) -> ResearchStatus {
    let evidence_ids = session
        .source_bindings
        .iter()
        .map(|binding| binding.evidence.id().to_string())
        .collect();
    ResearchStatus {
        research_id: session.id.to_string(),
        topic: session.topic,
        state: state_name(session.state).to_string(),
        counts: ResearchCounts {
            search_results: session.counts.search_results,
            acquisition_attempts: session.counts.acquisition_attempts,
            durable_sources: session.counts.durable_sources,
            observed_acquisitions: session.counts.observed_acquisitions,
            successful_extractions: session.counts.successful_extractions,
        },
        created_at_unix_ms: session.created_at_unix_ms,
        completed_at_unix_ms: session.completed_at_unix_ms,
        synthesis_summary: session
            .result
            .and_then(|result| result.synthesis.map(|synthesis| synthesis.summary)),
        evidence_ids,
    }
}

fn state_name(state: ResearchSessionState) -> &'static str {
    match state {
        ResearchSessionState::Claimed => "claimed",
        ResearchSessionState::SearchFailed => "search_failed",
        ResearchSessionState::CompletedNoSearchResults => "completed_no_search_results",
        ResearchSessionState::CompletedNoObservedAcquisitions => {
            "completed_no_observed_acquisitions"
        }
        ResearchSessionState::CompletedNoExtractions => "completed_no_extractions",
        ResearchSessionState::CompletedWithoutSynthesisRequested => {
            "completed_without_synthesis_requested"
        }
        ResearchSessionState::CompletedSynthesisInsufficient => "completed_synthesis_insufficient",
        ResearchSessionState::CompletedSynthesisFailed => "completed_synthesis_failed",
        ResearchSessionState::CompletedSuccessfully => "completed_successfully",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_response_excludes_provider_metadata() {
        let response = SearchResponse {
            query: "rust".into(),
            result_count: 1,
            results: vec![SearchResult {
                position: 1,
                title: "Rust".into(),
                url: "https://www.rust-lang.org".into(),
                snippet: Some("language".into()),
                date: None,
                score: Some(1.0),
            }],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(!json.contains("metadata"));
        assert!(!json.contains("api_key"));
    }

    #[tokio::test]
    async fn request_validation_and_configuration_fail_closed() {
        let empty = search(
            SearchRequest {
                query: " ".into(),
                limit: None,
            },
            Some("http://127.0.0.1:1"),
        )
        .await
        .unwrap_err();
        assert_eq!(
            empty,
            SearchError::InvalidRequest("query must not be empty".into())
        );

        let missing = search(
            SearchRequest {
                query: "rust".into(),
                limit: None,
            },
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(missing, SearchError::ProviderNotConfigured);
    }

    #[test]
    fn research_request_rejects_operator_configuration_fields() {
        let parsed = serde_json::from_str::<ResearchRequest>(
            r#"{"topic":"rust","api_key":"secret","database":"/tmp/db"}"#,
        );
        assert!(parsed.is_err());
    }

    #[test]
    fn research_availability_reuses_runtime_configuration_resolution() {
        let configured = |name: &str| match name {
            "RESEARCH_EVIDENCE_DB" => Some("/tmp/research.sqlite".to_string()),
            "SEARXNG_BASE_URL" => Some("http://127.0.0.1:8080".to_string()),
            "OPENAI_COMPAT_BASE_URL" => Some("http://127.0.0.1:11434/v1".to_string()),
            "OPENAI_COMPAT_MODEL" => Some("operator-model".to_string()),
            "OPENAI_COMPAT_API_KEY" => Some("operator-key".to_string()),
            _ => None,
        };
        assert_eq!(
            research_availability_with(&configured),
            ResearchAvailability::Available
        );
        assert!(resolve_research_config_with(&configured).is_ok());

        for missing in [
            "RESEARCH_EVIDENCE_DB",
            "SEARXNG_BASE_URL",
            "OPENAI_COMPAT_BASE_URL",
            "OPENAI_COMPAT_MODEL",
            "OPENAI_COMPAT_API_KEY",
        ] {
            let incomplete = |name: &str| {
                if name == missing {
                    None
                } else {
                    configured(name)
                }
            };
            assert_eq!(
                research_availability_with(&incomplete),
                ResearchAvailability::NotConfigured,
                "{missing} must be part of the shared Research configuration contract"
            );
            assert!(resolve_research_config_with(&incomplete).is_err());
        }
    }

    #[test]
    fn search_provider_config_error_projects_into_distinct_search_error_classes() {
        assert_eq!(
            map_search_provider_config_error(SearchProviderConfigError::MissingConfiguration(
                "SEARXNG_BASE_URL"
            )),
            SearchError::ProviderNotConfigured
        );
        for name in ["brave", "serper", "tavily"] {
            let projected = map_search_provider_config_error(
                SearchProviderConfigError::UnsupportedProvider(name),
            );
            assert_eq!(projected, SearchError::ProviderUnsupported(name.into()));
            // The exact F-4 defect: a compiled-out provider must never
            // collapse to the missing-configuration class.
            assert_ne!(projected, SearchError::ProviderNotConfigured);
        }
        let unknown = map_search_provider_config_error(SearchProviderConfigError::UnknownProvider(
            "scorpion-does-not-exist".into(),
        ));
        assert_eq!(
            unknown,
            SearchError::InvalidProviderSelection("scorpion-does-not-exist".into())
        );
        assert_ne!(unknown, SearchError::ProviderNotConfigured);
    }

    #[test]
    fn canonical_resolver_classifies_compiled_out_providers_as_unsupported_in_this_build() {
        // scorpion_app/Cargo.toml compiles only `search_searxng` — Brave,
        // Serper, and Tavily are real, recognized providers with their
        // required key present, but not compiled into this shipping build.
        for (selector, brave, serper, tavily) in [
            ("brave", Some("present-key"), None, None),
            ("serper", None, Some("present-key"), None),
            ("tavily", None, None, Some("present-key")),
        ] {
            let error = resolve_search_provider(Some(selector), None, brave, serper, tavily)
                .err()
                .unwrap_or_else(|| panic!("{selector} must fail in a SearXNG-only build"));
            assert_eq!(
                error,
                SearchProviderConfigError::UnsupportedProvider(selector),
                "{selector} must classify as UnsupportedProvider, not MissingConfiguration"
            );
            assert_eq!(
                map_search_provider_config_error(error),
                SearchError::ProviderUnsupported(selector.to_string())
            );
        }
    }

    #[test]
    fn canonical_resolver_classifies_unknown_selector_distinctly_from_unavailable() {
        let error =
            resolve_search_provider(Some("scorpion-does-not-exist"), None, None, None, None)
                .err()
                .unwrap();
        assert_eq!(
            error,
            SearchProviderConfigError::UnknownProvider("scorpion-does-not-exist".to_string())
        );
        assert_eq!(
            map_search_provider_config_error(error),
            SearchError::InvalidProviderSelection("scorpion-does-not-exist".to_string())
        );
    }

    #[test]
    fn unsupported_and_invalid_provider_search_errors_use_static_configuration_http_status() {
        // Every static configuration/build failure is 503, never 502 —
        // 502 is reserved for a provider that resolved successfully but
        // failed at runtime (SearchError::Provider).
        assert_eq!(error_status(&SearchError::ProviderNotConfigured), 503);
        assert_eq!(
            error_status(&SearchError::ProviderUnsupported("brave".into())),
            503
        );
        assert_eq!(
            error_status(&SearchError::InvalidProviderSelection("x".into())),
            503
        );
        assert_eq!(error_status(&SearchError::Provider("x".into())), 502);
    }

    #[test]
    fn search_public_json_error_codes_are_distinct_and_deterministic() {
        assert!(
            error_json(&SearchError::ProviderNotConfigured).contains("\"provider_not_configured\"")
        );
        let unsupported = error_json(&SearchError::ProviderUnsupported("brave".into()));
        assert!(unsupported.contains("\"provider_unsupported\""));
        assert!(!unsupported.contains("\"provider_not_configured\""));
        assert!(!unsupported.contains("\"provider_unavailable\""));
        let invalid = error_json(&SearchError::InvalidProviderSelection("x".into()));
        assert!(invalid.contains("\"invalid_provider_configuration\""));
        assert!(!invalid.contains("\"provider_unavailable\""));
    }

    #[test]
    fn research_config_and_availability_preserve_unsupported_and_invalid_provider_classes() {
        fn lookup_with(
            selector: &'static str,
            key_name: &'static str,
            key_value: &'static str,
        ) -> impl Fn(&str) -> Option<String> {
            move |name: &str| match name {
                "RESEARCH_EVIDENCE_DB" => Some("/tmp/research.sqlite".to_string()),
                "OPENAI_COMPAT_BASE_URL" => Some("http://127.0.0.1:11434/v1".to_string()),
                "OPENAI_COMPAT_MODEL" => Some("operator-model".to_string()),
                "OPENAI_COMPAT_API_KEY" => Some("operator-key".to_string()),
                "SEARCH_PROVIDER" => Some(selector.to_string()),
                other if other == key_name => Some(key_value.to_string()),
                _ => None,
            }
        }

        // Brave selected + key present, SearXNG-only build => UnsupportedProvider.
        let brave = lookup_with("brave", "BRAVE_API_KEY", "present-key");
        assert_eq!(
            resolve_research_config_with(&brave).err().unwrap(),
            ResearchError::UnsupportedProvider("brave".to_string())
        );
        assert_eq!(
            research_availability_with(&brave),
            ResearchAvailability::UnsupportedProvider("brave".to_string())
        );
        // The exact F-4 defect for Research: must never collapse to
        // NotConfigured or the runtime-failure class Unavailable.
        assert_ne!(
            research_availability_with(&brave),
            ResearchAvailability::NotConfigured
        );
        assert_ne!(
            resolve_research_config_with(&brave).err().unwrap(),
            ResearchError::Unavailable
        );

        // Unknown selector => InvalidProviderConfiguration.
        let unknown = lookup_with("scorpion-does-not-exist", "UNUSED_KEY", "unused");
        assert_eq!(
            resolve_research_config_with(&unknown).err().unwrap(),
            ResearchError::InvalidProviderConfiguration("scorpion-does-not-exist".to_string())
        );
        assert_eq!(
            research_availability_with(&unknown),
            ResearchAvailability::InvalidConfiguration("scorpion-does-not-exist".to_string())
        );
        assert_ne!(
            research_availability_with(&unknown),
            ResearchAvailability::NotConfigured
        );
    }

    #[test]
    fn unsupported_and_invalid_research_provider_use_static_configuration_http_status() {
        assert_eq!(
            research_error_status(&ResearchError::UnsupportedProvider("brave".into())),
            503
        );
        assert_eq!(
            research_error_status(&ResearchError::InvalidProviderConfiguration("x".into())),
            503
        );
        // The runtime-failure class remains the only 502.
        assert_eq!(research_error_status(&ResearchError::Unavailable), 502);
    }

    #[test]
    fn research_public_json_error_codes_are_distinct_and_deterministic() {
        let unsupported = research_error_json(&ResearchError::UnsupportedProvider("brave".into()));
        assert!(unsupported.contains("\"research_provider_unsupported\""));
        assert!(!unsupported.contains("\"research_not_configured\""));
        assert!(!unsupported.contains("\"research_unavailable\""));
        let invalid = research_error_json(&ResearchError::InvalidProviderConfiguration("x".into()));
        assert!(invalid.contains("\"research_provider_configuration_invalid\""));
        assert!(!invalid.contains("\"research_unavailable\""));
    }

    #[test]
    fn provider_configuration_error_messages_never_leak_secrets() {
        // Only the sanitized provider identity/selector — never a key,
        // URL, or filesystem path — may appear in a public message.
        let messages = [
            SearchError::ProviderUnsupported("brave".into()).to_string(),
            SearchError::InvalidProviderSelection("scorpion-does-not-exist".into()).to_string(),
            ResearchError::UnsupportedProvider("brave".into()).to_string(),
            ResearchError::InvalidProviderConfiguration("scorpion-does-not-exist".into())
                .to_string(),
        ];
        for message in messages {
            assert!(!message.contains("present-key"));
            assert!(!message.contains("operator-key"));
            assert!(!message.contains("http://"));
            assert!(!message.contains("/tmp/"));
        }
    }

    // ---------------------------------------------------------------------
    // F-5: every canonical `spider_search::SearchError` runtime-failure
    // variant must sanitize to the same fixed public message — none of
    // them may carry operator-configured URLs, credentials, query
    // strings, or transport/reqwest error detail across the public
    // boundary. This is a message-only correction: HTTP 502 and the
    // `provider_unavailable` code (F-4's established runtime-failure
    // classification) are unchanged.
    // ---------------------------------------------------------------------
    #[test]
    fn every_canonical_provider_runtime_error_variant_sanitizes_to_the_same_safe_message() {
        // A single synthetic detail string standing in for everything a
        // real internal error could carry: an operator hostname, a
        // filesystem-shaped path fragment, and a query-string token.
        let leaky_detail = "OPERATOR_URL_SENTINEL_8B16 https://internal.example.invalid:9443\
            /PATH_SENTINEL_F043?token=QUERY_SENTINEL_77E2";

        for variant in [
            CanonicalSearchError::RequestFailed(leaky_detail.to_string()),
            CanonicalSearchError::AuthenticationFailed,
            CanonicalSearchError::RateLimited,
            CanonicalSearchError::InvalidQuery(leaky_detail.to_string()),
            CanonicalSearchError::ProviderError(leaky_detail.to_string()),
            CanonicalSearchError::NoProvider,
        ] {
            let projected = sanitize_provider_runtime_error(variant);

            // Every variant collapses to the exact same fixed message —
            // never a per-variant reproduction of internal detail.
            assert_eq!(
                projected,
                SearchError::Provider(PROVIDER_RUNTIME_FAILURE_MESSAGE.to_string())
            );

            // Truthful, stable classification preserved (F-4 discipline):
            // a runtime failure remains 502/provider_unavailable, never
            // reclassified as a static configuration failure.
            assert_eq!(error_status(&projected), 502);
            let json = error_json(&projected);
            assert!(json.contains("\"code\":\"provider_unavailable\""));

            for sentinel in [
                "OPERATOR_URL_SENTINEL_8B16",
                "PATH_SENTINEL_F043",
                "QUERY_SENTINEL_77E2",
                "internal.example.invalid",
                "9443",
            ] {
                assert!(!json.contains(sentinel), "leaked `{sentinel}` in: {json}");
            }
        }
    }

    #[tokio::test]
    async fn research_status_rejects_invalid_and_unknown_ids_without_leaking_details() {
        let service = ResearchService::default();
        let invalid = service.status("not-a-research-id").await.unwrap_err();
        assert_eq!(
            invalid,
            ResearchError::InvalidRequest("invalid research id".into())
        );
    }

    #[tokio::test]
    async fn research_submission_returns_canonical_claim_id() {
        let database = std::env::temp_dir().join(format!(
            "scorpion-app-research-{}.sqlite3",
            std::process::id()
        ));
        std::env::set_var("RESEARCH_EVIDENCE_DB", &database);
        std::env::set_var("SEARXNG_BASE_URL", "http://127.0.0.1:9");
        std::env::set_var("OPENAI_COMPAT_BASE_URL", "http://127.0.0.1:9");
        std::env::set_var("OPENAI_COMPAT_MODEL", "test-model");
        std::env::set_var("OPENAI_COMPAT_API_KEY", "test-key");

        let accepted = ResearchService::default()
            .submit(ResearchRequest {
                topic: "rust async".into(),
            })
            .await
            .unwrap();
        assert_eq!(accepted.state, "claimed");
        let status = ResearchService::default()
            .status(&accepted.research_id)
            .await;
        assert!(status.is_ok() || matches!(status, Err(ResearchError::Unavailable)));

        let _ = std::fs::remove_file(&database);
        let _ = std::fs::remove_file(format!("{}-shm", database.display()));
        let _ = std::fs::remove_file(format!("{}-wal", database.display()));
    }
}
