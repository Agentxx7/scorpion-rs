//! Durable ownership for one canonical research invocation.
//!
//! This module wraps the provider-neutral `spider_agent::Agent::research`
//! engine without changing it. It claims a fresh [`ResearchId`] before any
//! search side effect, injects Spider's durable [`CanonicalPageAcquirer`],
//! and persists only invocation identity, evidence accounting, Source-N
//! lineage, counts, and truthful terminal outcome. It deliberately does not
//! persist `ResearchResult`, extraction payloads, model traffic, or replay
//! state.

use crate::features::agent_acquisition::{CanonicalPageAcquirer, ResearchAcquisitionEvidence};
use crate::features::domain_persistence::{DomainPersistence, PersistenceError};
use crate::features::identity::{EvidenceId, IdentityParseError, ResearchId};
use crate::utils::evidence::{AcquisitionOptions, EvidenceLedgerError, EvidenceRef};
use spider_agent::{AgentBuilder, AgentError, ResearchOptions, ResearchResult};
use std::future::Future;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Whether one durably acquired source reached successful extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchSourceDisposition {
    /// The evidence backs one successful `PageExtraction` and therefore has
    /// a Source-N binding.
    SuccessfullyExtracted,
    /// Evidence was recorded, but admission, materialization, or extraction
    /// did not produce a successful `PageExtraction`. The current neutral
    /// contract does not expose a more detailed reason.
    RejectedOrSkippedBeforeSuccessfulExtraction,
}

/// One durable source observed during this research invocation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchSessionSource {
    /// Canonical immutable source evidence.
    pub evidence: EvidenceRef,
    /// Truthful classification available from the current result contract.
    pub disposition: ResearchSourceDisposition,
}

/// Presentation-local Source-N label bound to canonical durable evidence.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchSourceBinding {
    /// One-based position in successful `PageExtraction` order.
    pub source_number: usize,
    /// Evidence used to produce that successful extraction.
    pub evidence: EvidenceRef,
}

impl ResearchSourceBinding {
    /// The synthesis presentation label. This is not an identity type.
    pub fn source_label(&self) -> String {
        format!("Source {}", self.source_number)
    }
}

/// Minimal counts needed to interpret a terminal research outcome.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchSessionCounts {
    /// Search results returned by the provider.
    pub search_results: usize,
    /// Search results selected for acquisition by `max_pages`.
    pub acquisition_attempts: usize,
    /// Acquisition attempts that produced durable canonical evidence.
    pub durable_sources: usize,
    /// Pages that produced a successful `PageExtraction`.
    pub successful_extractions: usize,
}

/// Truthful state of the durable session record.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchSessionState {
    /// Initial durable claim, written before research begins.
    Claimed,
    /// Research returned an error from its initial search operation.
    SearchFailed,
    /// Search completed with no results.
    CompletedNoSearchResults,
    /// Search completed but no page produced a successful extraction.
    CompletedNoExtractions,
    /// Successful extractions completed and synthesis was not requested.
    CompletedWithoutSynthesisRequested,
    /// Synthesis completed and truthfully found collective evidence
    /// insufficient.
    CompletedSynthesisInsufficient,
    /// Synthesis was requested with successful extractions but no synthesis
    /// result was returned, which is the current technical-failure contract.
    CompletedSynthesisFailed,
    /// Synthesis completed and found collective evidence sufficient.
    CompletedSuccessfully,
}

/// Minimal durable owner for one canonical research invocation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchSession {
    /// Identity of exactly this invocation.
    pub id: ResearchId,
    /// Original research topic supplied by the caller.
    pub topic: String,
    /// Caller-supplied extraction instructions, without inventing defaults.
    pub extraction_instructions: Option<String>,
    /// Every canonical evidence record produced by this invocation, in
    /// acquisition order.
    pub sources: Vec<ResearchSessionSource>,
    /// Successful extraction order mapped explicitly to durable evidence.
    pub source_bindings: Vec<ResearchSourceBinding>,
    /// Minimal observable counts.
    pub counts: ResearchSessionCounts,
    /// Claimed or truthful terminal state.
    pub state: ResearchSessionState,
    /// Unix epoch milliseconds immediately before the initial claim write.
    pub created_at_unix_ms: u64,
    /// Unix epoch milliseconds at terminal recording; absent while claimed.
    pub completed_at_unix_ms: Option<u64>,
}

/// A completed orchestration call. Search failure remains available as the
/// original provider-neutral error while the durable session records its
/// terminal state.
#[derive(Debug)]
pub struct DurableResearchRun {
    /// Durable terminal session record.
    pub session: ResearchSession,
    /// Provider-neutral research result or its original execution error.
    pub result: Result<ResearchResult, AgentError>,
}

/// Failure of durable session ownership or persistence.
#[derive(Debug)]
pub enum ResearchSessionError {
    /// The configured provider-neutral agent could not be constructed.
    AgentSetup(AgentError),
    /// Canonical domain persistence failed.
    Persistence(PersistenceError),
    /// Session JSON encoding/decoding failed.
    Serialization(serde_json::Error),
    /// A successful extraction exposed a malformed acquisition identity.
    InvalidEvidenceId(IdentityParseError),
    /// Canonical evidence could not be resolved.
    Evidence(EvidenceLedgerError),
    /// Durable acquisition/result lineage violated the canonical contract.
    InvalidDurableBinding(String),
}

impl std::fmt::Display for ResearchSessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AgentSetup(error) => write!(f, "research session agent setup failed: {error}"),
            Self::Persistence(error) => write!(f, "research session persistence failed: {error}"),
            Self::Serialization(error) => {
                write!(f, "research session serialization failed: {error}")
            }
            Self::InvalidEvidenceId(error) => {
                write!(
                    f,
                    "research session received an invalid evidence ID: {error}"
                )
            }
            Self::Evidence(error) => write!(f, "research session evidence failed: {error}"),
            Self::InvalidDurableBinding(message) => {
                write!(f, "research session durable binding failed: {message}")
            }
        }
    }
}

impl std::error::Error for ResearchSessionError {}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

async fn persist_claim(
    store: &DomainPersistence,
    session: &ResearchSession,
) -> Result<u64, ResearchSessionError> {
    let payload = serde_json::to_vec(session).map_err(ResearchSessionError::Serialization)?;
    let revision = store
        .write_current(&session.id.to_string(), None, &payload)
        .await
        .map_err(ResearchSessionError::Persistence)?;
    store
        .append_history(
            &session.id.to_string(),
            revision,
            &payload,
            SystemTime::now(),
        )
        .await
        .map_err(ResearchSessionError::Persistence)?;
    Ok(revision)
}

async fn persist_terminal(
    store: &DomainPersistence,
    session: &ResearchSession,
    expected_revision: u64,
) -> Result<(), ResearchSessionError> {
    let payload = serde_json::to_vec(session).map_err(ResearchSessionError::Serialization)?;
    let revision = store
        .write_current(&session.id.to_string(), Some(expected_revision), &payload)
        .await
        .map_err(ResearchSessionError::Persistence)?;
    store
        .append_history(
            &session.id.to_string(),
            revision,
            &payload,
            SystemTime::now(),
        )
        .await
        .map_err(ResearchSessionError::Persistence)?;
    Ok(())
}

/// Read the current durable record for `id`.
pub async fn read_research_session(
    store: &DomainPersistence,
    id: ResearchId,
) -> Result<Option<(u64, ResearchSession)>, ResearchSessionError> {
    match store
        .read_current(&id.to_string())
        .await
        .map_err(ResearchSessionError::Persistence)?
    {
        Some((revision, payload)) => {
            let session =
                serde_json::from_slice(&payload).map_err(ResearchSessionError::Serialization)?;
            Ok(Some((revision, session)))
        }
        None => Ok(None),
    }
}

fn terminal_state(
    result: &Result<ResearchResult, AgentError>,
    synthesize: bool,
) -> ResearchSessionState {
    let Ok(result) = result else {
        return ResearchSessionState::SearchFailed;
    };
    if result.search_results.is_empty() {
        ResearchSessionState::CompletedNoSearchResults
    } else if result.extractions.is_empty() {
        ResearchSessionState::CompletedNoExtractions
    } else if !synthesize {
        ResearchSessionState::CompletedWithoutSynthesisRequested
    } else {
        match result.synthesis_sufficient {
            Some(true) => ResearchSessionState::CompletedSuccessfully,
            Some(false) => ResearchSessionState::CompletedSynthesisInsufficient,
            None => ResearchSessionState::CompletedSynthesisFailed,
        }
    }
}

async fn bind_sources(
    store: &DomainPersistence,
    retained: &[ResearchAcquisitionEvidence],
    result: &Result<ResearchResult, AgentError>,
) -> Result<(Vec<ResearchSessionSource>, Vec<ResearchSourceBinding>), ResearchSessionError> {
    let mut bindings = Vec::new();
    if let Ok(result) = result {
        for (index, extraction) in result.extractions.iter().enumerate() {
            let raw_id = extraction.acquisition_id.as_deref().ok_or_else(|| {
                ResearchSessionError::InvalidDurableBinding(
                    "successful extraction has no acquisition identity".to_string(),
                )
            })?;
            let id: EvidenceId = raw_id
                .parse()
                .map_err(ResearchSessionError::InvalidEvidenceId)?;
            let retained_record = retained
                .iter()
                .find(|record| record.acquisition_id == id && record.evidence.id == Some(id));
            if retained_record.is_none() {
                return Err(ResearchSessionError::InvalidDurableBinding(format!(
                    "successful extraction identity {id} was not retained by this durable acquirer"
                )));
            }
            let evidence = EvidenceRef::new(id);
            if evidence
                .resolve(store)
                .await
                .map_err(ResearchSessionError::Evidence)?
                .is_none()
            {
                return Err(ResearchSessionError::InvalidDurableBinding(format!(
                    "successful extraction identity {id} is not durable"
                )));
            }
            bindings.push(ResearchSourceBinding {
                source_number: index + 1,
                evidence,
            });
        }
    }

    let mut sources = Vec::with_capacity(retained.len());
    for record in retained {
        let evidence = record.evidence_ref().ok_or_else(|| {
            ResearchSessionError::InvalidDurableBinding(format!(
                "acquisition {} was retained without a durable evidence identity",
                record.acquisition_id
            ))
        })?;
        if evidence
            .resolve(store)
            .await
            .map_err(ResearchSessionError::Evidence)?
            .is_none()
        {
            return Err(ResearchSessionError::InvalidDurableBinding(format!(
                "retained acquisition {} is not resolvable",
                record.acquisition_id
            )));
        }
        let disposition = if bindings.iter().any(|binding| binding.evidence == evidence) {
            ResearchSourceDisposition::SuccessfullyExtracted
        } else {
            ResearchSourceDisposition::RejectedOrSkippedBeforeSuccessfulExtraction
        };
        sources.push(ResearchSessionSource {
            evidence,
            disposition,
        });
    }
    Ok((sources, bindings))
}

async fn run_with_id<F, Fut>(
    store: Arc<DomainPersistence>,
    acquirer: CanonicalPageAcquirer,
    id: ResearchId,
    topic: String,
    options: ResearchOptions,
    execute: F,
) -> Result<DurableResearchRun, ResearchSessionError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<ResearchResult, AgentError>>,
{
    let mut session = ResearchSession {
        id,
        topic,
        extraction_instructions: options.extraction_prompt.clone(),
        sources: Vec::new(),
        source_bindings: Vec::new(),
        counts: ResearchSessionCounts::default(),
        state: ResearchSessionState::Claimed,
        created_at_unix_ms: unix_time_ms(),
        completed_at_unix_ms: None,
    };
    let claim_revision = persist_claim(&store, &session).await?;

    let result = execute().await;
    let retained = acquirer.retained_evidence();
    let (sources, source_bindings) = bind_sources(&store, &retained, &result).await?;
    session.sources = sources;
    session.source_bindings = source_bindings;
    if let Ok(result) = &result {
        session.counts.search_results = result.search_results.len();
        session.counts.acquisition_attempts =
            options.max_pages.min(result.search_results.results.len());
        session.counts.successful_extractions = result.extractions.len();
    }
    session.counts.durable_sources = retained.len();
    session.state = terminal_state(&result, options.synthesize);
    session.completed_at_unix_ms = Some(unix_time_ms());
    persist_terminal(&store, &session, claim_revision).await?;

    Ok(DurableResearchRun { session, result })
}

/// Execute one explicitly durable canonical research session.
///
/// The provider/model/search configuration remains caller-owned through the
/// supplied `AgentBuilder`. This function exclusively installs the page
/// acquirer so a successful extraction cannot carry an ephemeral identity.
pub async fn run_durable_research(
    store: Arc<DomainPersistence>,
    builder: AgentBuilder,
    topic: impl Into<String>,
    options: ResearchOptions,
) -> Result<DurableResearchRun, ResearchSessionError> {
    let acquirer =
        CanonicalPageAcquirer::new_durable(AcquisitionOptions::default(), Arc::clone(&store));
    let agent = builder
        .with_page_acquirer(Box::new(acquirer.clone()))
        .build()
        .map_err(ResearchSessionError::AgentSetup)?;
    let topic = topic.into();
    let execution_topic = topic.clone();
    let execution_options = options.clone();
    run_with_id(
        store,
        acquirer,
        ResearchId::new(),
        topic,
        options,
        move || async move { agent.research(&execution_topic, execution_options).await },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use spider_agent::{
        FinishReason, PageAcquirer, PageExtraction, ResearchExtraction, ResearchExtractionFact,
        SearchResult, SearchResults, TokenUsage,
    };
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn fixture(body: &'static [u8]) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/article", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        });
        (url, handle)
    }

    fn result(
        search_count: usize,
        acquisition_ids: &[EvidenceId],
        synthesis_sufficient: Option<bool>,
    ) -> ResearchResult {
        let mut search_results = SearchResults::new("same topic");
        for index in 0..search_count {
            search_results.push(SearchResult::new(
                format!("Result {}", index + 1),
                format!("https://result{}.example", index + 1),
                index + 1,
            ));
        }
        let extractions = acquisition_ids
            .iter()
            .enumerate()
            .map(|(index, id)| PageExtraction {
                url: format!("https://final{}.example", index + 1),
                title: format!("Source {}", index + 1),
                extracted: ResearchExtraction {
                    facts: vec![ResearchExtractionFact {
                        topic: "Evidence".to_string(),
                        finding: "Source-grounded finding".to_string(),
                    }],
                    missing_evidence: Vec::new(),
                },
                acquisition_id: Some(id.to_string()),
                finish_reason: Some(FinishReason::Stop),
                extraction_input_bytes: 128,
            })
            .collect();
        ResearchResult {
            topic: "same topic".to_string(),
            search_results,
            extractions,
            summary: synthesis_sufficient.map(|sufficient| {
                if sufficient {
                    "Supported [Source 1]".to_string()
                } else {
                    "Insufficient evidence: missing support".to_string()
                }
            }),
            synthesis_sufficient,
            synthesis_source_ids: Vec::new(),
            usage: TokenUsage::default(),
        }
    }

    async fn adapter(store: &Arc<DomainPersistence>) -> CanonicalPageAcquirer {
        CanonicalPageAcquirer::new_durable(AcquisitionOptions::default(), Arc::clone(store))
    }

    async fn acquire(acquirer: &CanonicalPageAcquirer, body: &'static [u8]) -> EvidenceId {
        let (url, handle) = fixture(body);
        let source = acquirer.acquire(&url).await.unwrap();
        handle.join().unwrap();
        source.acquisition_id.unwrap().parse().unwrap()
    }

    fn temporary_database_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "scorpion-research-session-{}.sqlite3",
            ResearchId::new()
        ))
    }

    fn remove_temporary_database(path: &std::path::Path) {
        for candidate in [
            path.to_path_buf(),
            std::path::PathBuf::from(format!("{}-shm", path.display())),
            std::path::PathBuf::from(format!("{}-wal", path.display())),
        ] {
            let _ = std::fs::remove_file(candidate);
        }
    }

    #[tokio::test]
    async fn identical_topics_are_distinct_sessions_and_claim_precedes_execution() {
        let store = Arc::new(DomainPersistence::open_in_memory().await.unwrap());
        let options = ResearchOptions::new();
        let first_id = ResearchId::new();
        let observed_claim = Arc::new(AtomicBool::new(false));
        let observed_claim_in_execution = Arc::clone(&observed_claim);
        let execution_store = Arc::clone(&store);
        let first = run_with_id(
            Arc::clone(&store),
            adapter(&store).await,
            first_id,
            "same topic".to_string(),
            options.clone(),
            move || async move {
                let (_, claimed) = read_research_session(&execution_store, first_id)
                    .await
                    .unwrap()
                    .unwrap();
                observed_claim_in_execution.store(
                    claimed.state == ResearchSessionState::Claimed,
                    Ordering::SeqCst,
                );
                Ok(result(0, &[], None))
            },
        )
        .await
        .unwrap();
        let second = run_with_id(
            Arc::clone(&store),
            adapter(&store).await,
            ResearchId::new(),
            "same topic".to_string(),
            options,
            || async { Ok(result(0, &[], None)) },
        )
        .await
        .unwrap();

        assert!(observed_claim.load(Ordering::SeqCst));
        assert_ne!(first.session.id, second.session.id);
        assert_eq!(
            first.session.state,
            ResearchSessionState::CompletedNoSearchResults
        );
    }

    #[tokio::test]
    async fn initial_claim_failure_prevents_execution() {
        let store = Arc::new(DomainPersistence::open_in_memory().await.unwrap());
        let id = ResearchId::new();
        store
            .write_current(&id.to_string(), None, b"occupied")
            .await
            .unwrap();
        let executed = Arc::new(AtomicBool::new(false));
        let executed_in_closure = Arc::clone(&executed);

        let error = run_with_id(
            Arc::clone(&store),
            adapter(&store).await,
            id,
            "topic".to_string(),
            ResearchOptions::new(),
            move || async move {
                executed_in_closure.store(true, Ordering::SeqCst);
                Ok(result(0, &[], None))
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(error, ResearchSessionError::Persistence(_)));
        assert!(!executed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn source_accounting_and_source_numbers_follow_extraction_order() {
        let store = Arc::new(DomainPersistence::open_in_memory().await.unwrap());
        let acquirer = adapter(&store).await;
        let execution_acquirer = acquirer.clone();
        let run = run_with_id(
            Arc::clone(&store),
            acquirer,
            ResearchId::new(),
            "topic".to_string(),
            ResearchOptions::new()
                .with_max_pages(4)
                .with_synthesize(false),
            move || async move {
                let first =
                    acquire(&execution_acquirer, b"<html>first durable source</html>").await;
                let second =
                    acquire(&execution_acquirer, b"<html>second durable source</html>").await;
                let _rejected =
                    acquire(&execution_acquirer, b"<html>rejected durable source</html>").await;
                assert!(execution_acquirer
                    .acquire("not a valid canonical URL")
                    .await
                    .is_err());
                Ok(result(4, &[second, first], None))
            },
        )
        .await
        .unwrap();

        assert_eq!(run.session.sources.len(), 3);
        assert_eq!(run.session.source_bindings.len(), 2);
        assert_eq!(run.session.source_bindings[0].source_label(), "Source 1");
        assert_eq!(run.session.source_bindings[1].source_label(), "Source 2");
        assert_eq!(
            run.session.source_bindings[0].evidence,
            run.session.sources[1].evidence
        );
        assert_eq!(
            run.session.source_bindings[1].evidence,
            run.session.sources[0].evidence
        );
        assert_eq!(
            run.session.sources[2].disposition,
            ResearchSourceDisposition::RejectedOrSkippedBeforeSuccessfulExtraction
        );
        assert_eq!(run.session.counts.acquisition_attempts, 4);
        assert_eq!(run.session.counts.durable_sources, 3);
        assert_eq!(run.session.counts.successful_extractions, 2);
        assert_eq!(
            run.session.state,
            ResearchSessionState::CompletedWithoutSynthesisRequested
        );
    }

    #[tokio::test]
    async fn evidence_shaped_text_without_this_sessions_durable_record_is_rejected() {
        let store = Arc::new(DomainPersistence::open_in_memory().await.unwrap());
        let phantom = EvidenceId::new();
        let error = run_with_id(
            Arc::clone(&store),
            adapter(&store).await,
            ResearchId::new(),
            "topic".to_string(),
            ResearchOptions::new(),
            move || async move { Ok(result(1, &[phantom], None)) },
        )
        .await
        .unwrap_err();

        assert!(matches!(
            error,
            ResearchSessionError::InvalidDurableBinding(_)
        ));
    }

    #[tokio::test]
    async fn terminal_outcomes_are_distinguished_from_observable_contract() {
        async fn run_case(
            search_count: usize,
            extraction: bool,
            synthesize: bool,
            sufficient: Option<bool>,
        ) -> ResearchSessionState {
            let store = Arc::new(DomainPersistence::open_in_memory().await.unwrap());
            let acquirer = adapter(&store).await;
            let execution_acquirer = acquirer.clone();
            run_with_id(
                Arc::clone(&store),
                acquirer,
                ResearchId::new(),
                "topic".to_string(),
                ResearchOptions::new().with_synthesize(synthesize),
                move || async move {
                    let ids = if extraction {
                        vec![acquire(&execution_acquirer, b"<html>extracted source</html>").await]
                    } else {
                        Vec::new()
                    };
                    Ok(result(search_count, &ids, sufficient))
                },
            )
            .await
            .unwrap()
            .session
            .state
        }

        let store = Arc::new(DomainPersistence::open_in_memory().await.unwrap());
        let failed = run_with_id(
            Arc::clone(&store),
            adapter(&store).await,
            ResearchId::new(),
            "topic".to_string(),
            ResearchOptions::new(),
            || async { Err(AgentError::Remote("search failed".to_string())) },
        )
        .await
        .unwrap();
        assert_eq!(failed.session.state, ResearchSessionState::SearchFailed);
        assert_eq!(
            run_case(0, false, true, None).await,
            ResearchSessionState::CompletedNoSearchResults
        );
        assert_eq!(
            run_case(2, false, true, None).await,
            ResearchSessionState::CompletedNoExtractions
        );
        assert_eq!(
            run_case(1, true, false, None).await,
            ResearchSessionState::CompletedWithoutSynthesisRequested
        );
        assert_eq!(
            run_case(1, true, true, None).await,
            ResearchSessionState::CompletedSynthesisFailed
        );
        assert_eq!(
            run_case(1, true, true, Some(false)).await,
            ResearchSessionState::CompletedSynthesisInsufficient
        );
        assert_eq!(
            run_case(1, true, true, Some(true)).await,
            ResearchSessionState::CompletedSuccessfully
        );
    }

    #[tokio::test]
    async fn session_and_bound_evidence_resolve_after_file_reopen() {
        let path = temporary_database_path();
        remove_temporary_database(&path);
        let store = Arc::new(DomainPersistence::open(&path).await.unwrap());
        let acquirer = adapter(&store).await;
        let execution_acquirer = acquirer.clone();
        let run = run_with_id(
            Arc::clone(&store),
            acquirer,
            ResearchId::new(),
            "durable topic".to_string(),
            ResearchOptions::new().with_synthesize(false),
            move || async move {
                let id = acquire(&execution_acquirer, b"<html>reopen source</html>").await;
                Ok(result(1, &[id], None))
            },
        )
        .await
        .unwrap();
        let id = run.session.id;
        let expected_binding = run.session.source_bindings[0].clone();
        drop(run);
        drop(store);

        let reopened = DomainPersistence::open(&path).await.unwrap();
        let (_, session) = read_research_session(&reopened, id).await.unwrap().unwrap();
        assert_eq!(session.id, id);
        assert_eq!(session.source_bindings, [expected_binding.clone()]);
        assert_eq!(
            expected_binding
                .evidence
                .resolve(&reopened)
                .await
                .unwrap()
                .unwrap()
                .id,
            Some(expected_binding.evidence.id())
        );

        drop(reopened);
        remove_temporary_database(&path);
    }
}
