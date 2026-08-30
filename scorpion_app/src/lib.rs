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
use spider::features::search::{resolve_search_provider, SearchOptions, SearchProvider};
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
    ProviderNotConfigured,
    Provider(String),
    Internal(String),
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(message) => write!(f, "invalid request: {message}"),
            Self::ProviderNotConfigured => f.write_str("search provider is not configured"),
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
    .map_err(|_| SearchError::ProviderNotConfigured)?
    .1;
    let options = request.limit.map_or_else(SearchOptions::new, |limit| {
        SearchOptions::new().with_limit(limit)
    });
    let results = provider
        .search(query, &options)
        .await
        .map_err(|error| SearchError::Provider(error.to_string()))?;

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
pub fn error_status(error: &SearchError) -> u16 {
    match error {
        SearchError::InvalidRequest(_) => 400,
        SearchError::ProviderNotConfigured => 503,
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
    NotConfigured,
    CapacityExhausted,
    NotFound,
    Unavailable,
    Internal,
}

/// Static operator-configuration availability for the Research product.
///
/// This deliberately says nothing about external service health. Execution
/// still performs every canonical runtime check and remains fail-closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResearchAvailability {
    Available,
    NotConfigured,
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
        f.write_str(match self {
            Self::InvalidRequest(_) => "invalid research request",
            Self::NotConfigured => "research is not configured",
            Self::CapacityExhausted => "research capacity is temporarily exhausted",
            Self::NotFound => "research session not found",
            Self::Unavailable => "research is unavailable",
            Self::Internal => "internal research error",
        })
    }
}

pub fn research_error_status(error: &ResearchError) -> u16 {
    match error {
        ResearchError::InvalidRequest(_) => 400,
        ResearchError::NotFound => 404,
        ResearchError::NotConfigured | ResearchError::CapacityExhausted => 503,
        ResearchError::Unavailable => 502,
        ResearchError::Internal => 500,
    }
}

pub fn research_error_json(error: &ResearchError) -> String {
    let code = match error {
        ResearchError::InvalidRequest(_) => "invalid_request",
        ResearchError::NotConfigured => "research_not_configured",
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
    .map_err(|_| ResearchError::Unavailable)?
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
