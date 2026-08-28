//! Thin shipping CLI binding for Spider-owned durable canonical research.
//!
//! This module resolves product configuration and presents durable sessions.
//! It deliberately delegates execution, identity, persistence, evidence,
//! extraction, and synthesis to Spider's canonical APIs.

use spider::agent::{Agent, ResearchOptions};
use spider::features::domain_persistence::DomainPersistence;
use spider::features::identity::ResearchId;
use spider::features::research_session::{
    read_research_session, run_durable_research, ResearchSession, ResearchSessionState,
};
use std::path::PathBuf;
use std::sync::Arc;

const DATABASE_ENV: &str = "RESEARCH_EVIDENCE_DB";
const SEARXNG_ENV: &str = "SEARXNG_BASE_URL";
const OPENAI_BASE_ENV: &str = "OPENAI_COMPAT_BASE_URL";
const MODEL_ENV: &str = "OPENAI_COMPAT_MODEL";
const API_KEY_ENV: &str = "OPENAI_COMPAT_API_KEY";

#[derive(Clone, Debug)]
pub struct RunParams {
    pub topic: String,
    pub database: Option<PathBuf>,
    pub searxng_url: Option<String>,
    pub openai_base_url: Option<String>,
    pub model: Option<String>,
    pub extraction_instructions: Option<String>,
    pub max_pages: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct ShowParams {
    pub research_id: String,
    pub database: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub enum Request {
    Run(RunParams),
    Show(ShowParams),
}

#[derive(Debug, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: Option<String>,
    pub exit_code: i32,
}

#[derive(Debug)]
struct RunConfig {
    database: PathBuf,
    searxng_url: String,
    openai_base_url: String,
    model: String,
    api_key: String,
}

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn configured_string(
    cli: Option<String>,
    environment: &str,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<String, String> {
    cli.and_then(nonempty)
        .or_else(|| lookup(environment).and_then(nonempty))
        .ok_or_else(|| format!("missing required research configuration: {environment}"))
}

fn configured_database(
    cli: Option<PathBuf>,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<PathBuf, String> {
    cli.filter(|path| !path.as_os_str().is_empty())
        .or_else(|| lookup(DATABASE_ENV).and_then(nonempty).map(PathBuf::from))
        .ok_or_else(|| format!("missing required research configuration: {DATABASE_ENV}"))
}

fn resolve_run_config(
    params: &RunParams,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<RunConfig, String> {
    Ok(RunConfig {
        database: configured_database(params.database.clone(), lookup)?,
        searxng_url: configured_string(params.searxng_url.clone(), SEARXNG_ENV, lookup)?,
        openai_base_url: configured_string(
            params.openai_base_url.clone(),
            OPENAI_BASE_ENV,
            lookup,
        )?,
        model: configured_string(params.model.clone(), MODEL_ENV, lookup)?,
        api_key: configured_string(None, API_KEY_ENV, lookup)?,
    })
}

fn resolve_show_database(
    database: Option<PathBuf>,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<PathBuf, String> {
    configured_database(database, lookup)
}

fn terminal_exit(session: &ResearchSession) -> (i32, Option<String>) {
    match session.state {
        ResearchSessionState::Claimed => (
            2,
            Some("Research session has not reached a durable terminal state.".to_string()),
        ),
        ResearchSessionState::SearchFailed => (
            2,
            Some("Research search failed after the durable session was claimed.".to_string()),
        ),
        ResearchSessionState::CompletedNoObservedAcquisitions => (
            2,
            Some(
                "Research search succeeded, but no candidate acquisition observed a real \
                 network response — total acquisition failure."
                    .to_string(),
            ),
        ),
        ResearchSessionState::CompletedSynthesisFailed => (
            2,
            Some(
                "Research synthesis failed technically; durable extractions remain available."
                    .to_string(),
            ),
        ),
        _ => (0, None),
    }
}

async fn format_session(
    store: &DomainPersistence,
    session: &ResearchSession,
    verbose: bool,
) -> Result<String, String> {
    for binding in &session.source_bindings {
        if binding
            .evidence
            .resolve(store)
            .await
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Err(format!(
                "{} evidence {} is not resolvable from the canonical ledger",
                binding.source_label(),
                binding.evidence.id()
            ));
        }
    }
    if let Some(result) = &session.result {
        for synthesis in result.synthesis.iter() {
            for citation in &synthesis.citations {
                if citation
                    .evidence
                    .resolve(store)
                    .await
                    .map_err(|error| error.to_string())?
                    .is_none()
                {
                    return Err(format!(
                        "Source {} citation evidence {} is not resolvable from the canonical ledger",
                        citation.source_number,
                        citation.evidence.id()
                    ));
                }
            }
        }
    }

    let mut output = format!("Research ID: {}\nState: {:?}\n", session.id, session.state);
    match session.state {
        ResearchSessionState::CompletedNoSearchResults => {
            output.push_str("\nNo search results.\n");
        }
        ResearchSessionState::CompletedNoObservedAcquisitions => {
            output.push_str(
                "\nNo observed acquisitions: every candidate acquisition failed before a \
                 response was received.\n",
            );
        }
        ResearchSessionState::CompletedNoExtractions => {
            output.push_str("\nNo supported extractions.\n");
        }
        ResearchSessionState::SearchFailed => {
            output.push_str("\nSearch failed before a research result could be produced.\n");
        }
        _ => {}
    }

    match session.result.as_ref() {
        Some(result) => {
            if let Some(synthesis) = &result.synthesis {
                output.push_str("\nSummary:\n");
                output.push_str(&synthesis.summary);
                output.push('\n');
                if matches!(
                    session.state,
                    ResearchSessionState::CompletedSynthesisInsufficient
                ) {
                    output.push_str("Evidence status: insufficient.\n");
                }
            } else if !result.extractions.is_empty() {
                output.push_str("\nExtracted facts:\n");
                for extraction in &result.extractions {
                    output.push_str(&format!("Source {}:\n", extraction.source_number));
                    for fact in &extraction.extracted.facts {
                        output.push_str(&format!("- {}: {}\n", fact.topic, fact.finding));
                    }
                }
            }

            if !session.source_bindings.is_empty() {
                output.push_str("\nSources:\n");
                for binding in &session.source_bindings {
                    output.push_str(&format!(
                        "[{}] Evidence: {}\n",
                        binding.source_number,
                        binding.evidence.id()
                    ));
                }
            }

            let with_missing = result
                .extractions
                .iter()
                .filter(|extraction| !extraction.extracted.missing_evidence.is_empty())
                .collect::<Vec<_>>();
            if !with_missing.is_empty() {
                output.push_str("\nMissing evidence:\n");
                for extraction in with_missing {
                    output.push_str(&format!("Source {}:\n", extraction.source_number));
                    for missing in &extraction.extracted.missing_evidence {
                        output.push_str(&format!("- {missing}\n"));
                    }
                }
            }

            if verbose {
                output.push_str("\nDiagnostics:\n");
                output.push_str(&format!(
                    "Search results: {}\nAcquisition attempts: {}\nDurable sources: {}\nObserved acquisitions: {}\nSuccessful extractions: {}\n",
                    session.counts.search_results,
                    session.counts.acquisition_attempts,
                    session.counts.durable_sources,
                    session.counts.observed_acquisitions,
                    session.counts.successful_extractions
                ));
                for extraction in &result.extractions {
                    output.push_str(&format!(
                        "Source {} extraction_input_bytes={} finish_reason={:?}\n",
                        extraction.source_number,
                        extraction.extraction_input_bytes,
                        extraction.finish_reason
                    ));
                }
                if let Some(synthesis) = &result.synthesis {
                    output.push_str(&format!(
                        "Synthesis tokens: prompt={} completion={} total={}\n",
                        synthesis.usage.prompt_tokens,
                        synthesis.usage.completion_tokens,
                        synthesis.usage.total_tokens
                    ));
                }
            }
        }
        None if !matches!(
            session.state,
            ResearchSessionState::Claimed
                | ResearchSessionState::SearchFailed
                | ResearchSessionState::CompletedNoSearchResults
                | ResearchSessionState::CompletedNoObservedAcquisitions
                | ResearchSessionState::CompletedNoExtractions
        ) =>
        {
            output.push_str(
                "\nDurable result payload is unavailable for this legacy research session.\n",
            );
        }
        None => {}
    }
    Ok(output)
}

async fn execute_with_environment(
    request: Request,
    verbose: bool,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<CommandOutput, String> {
    match request {
        Request::Run(params) => {
            if params.topic.trim().is_empty() || params.topic == "show" {
                return Err(
                    "research topic must be non-empty and cannot be the reserved word \"show\""
                        .to_string(),
                );
            }
            let config = resolve_run_config(&params, &lookup)?;
            let store = Arc::new(
                DomainPersistence::open(&config.database)
                    .await
                    .map_err(|error| error.to_string())?,
            );
            let builder = Agent::builder()
                .with_openai_compatible(config.openai_base_url, config.api_key, config.model)
                .with_search_searxng(config.searxng_url);
            let mut options = ResearchOptions::new().with_synthesize(true);
            if let Some(max_pages) = params.max_pages {
                options = options.with_max_pages(max_pages);
            }
            if let Some(instructions) = params.extraction_instructions {
                options = options.with_extraction_prompt(instructions);
            }
            let run = run_durable_research(store.clone(), builder, params.topic, options)
                .await
                .map_err(|error| error.to_string())?;
            let stdout = format_session(&store, &run.session, verbose).await?;
            let (mut exit_code, mut stderr) = terminal_exit(&run.session);
            if let Err(error) = run.result {
                exit_code = 2;
                stderr = Some(format!(
                    "{} Technical error: {error}",
                    stderr.unwrap_or_else(|| "Research execution failed.".to_string())
                ));
            }
            Ok(CommandOutput {
                stdout,
                stderr,
                exit_code,
            })
        }
        Request::Show(params) => {
            let database = resolve_show_database(params.database, &lookup)?;
            let research_id: ResearchId = params
                .research_id
                .parse()
                .map_err(|error| format!("invalid ResearchId: {error}"))?;
            let store = DomainPersistence::open(&database)
                .await
                .map_err(|error| error.to_string())?;
            let (_, session) = read_research_session(&store, research_id)
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("Research session not found: {research_id}"))?;
            let stdout = format_session(&store, &session, verbose).await?;
            let (exit_code, stderr) = terminal_exit(&session);
            Ok(CommandOutput {
                stdout,
                stderr,
                exit_code,
            })
        }
    }
}

pub async fn execute(request: Request, verbose: bool) -> Result<CommandOutput, String> {
    execute_with_environment(request, verbose, |name| std::env::var(name).ok()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use spider::features::identity::EvidenceId;
    use spider::features::research_session::{
        DurableResearchExtraction, DurableResearchResult, ResearchSessionCounts,
        ResearchSourceBinding,
    };
    use spider::spider_agent::{FinishReason, ResearchExtraction, ResearchExtractionFact};
    use spider::utils::evidence::{record_evidence, EvidenceBundle, EvidenceRef};
    use std::collections::HashMap;

    fn run_params() -> RunParams {
        RunParams {
            topic: "topic".to_string(),
            database: None,
            searxng_url: None,
            openai_base_url: None,
            model: None,
            extraction_instructions: None,
            max_pages: None,
        }
    }

    #[test]
    fn explicit_non_secret_configuration_precedes_environment() {
        let mut params = run_params();
        params.database = Some(PathBuf::from("cli.sqlite"));
        params.searxng_url = Some("https://cli-search".to_string());
        params.openai_base_url = Some("https://cli-model/v1".to_string());
        params.model = Some("cli-model".to_string());
        let environment = HashMap::from([
            (DATABASE_ENV, "env.sqlite"),
            (SEARXNG_ENV, "https://env-search"),
            (OPENAI_BASE_ENV, "https://env-model/v1"),
            (MODEL_ENV, "env-model"),
            (API_KEY_ENV, "secret"),
        ]);
        let config = resolve_run_config(&params, &|name| {
            environment.get(name).map(ToString::to_string)
        })
        .unwrap();
        assert_eq!(config.database, PathBuf::from("cli.sqlite"));
        assert_eq!(config.searxng_url, "https://cli-search");
        assert_eq!(config.openai_base_url, "https://cli-model/v1");
        assert_eq!(config.model, "cli-model");
        assert_eq!(config.api_key, "secret");
    }

    #[test]
    fn environment_fallback_and_every_missing_value_fail_closed() {
        let values = HashMap::from([
            (DATABASE_ENV, "env.sqlite"),
            (SEARXNG_ENV, "https://env-search"),
            (OPENAI_BASE_ENV, "https://env-model/v1"),
            (MODEL_ENV, "env-model"),
            (API_KEY_ENV, "secret"),
        ]);
        let config = resolve_run_config(&run_params(), &|name| {
            values.get(name).map(ToString::to_string)
        })
        .unwrap();
        assert_eq!(config.database, PathBuf::from("env.sqlite"));

        for missing in [
            DATABASE_ENV,
            SEARXNG_ENV,
            OPENAI_BASE_ENV,
            MODEL_ENV,
            API_KEY_ENV,
        ] {
            let error = resolve_run_config(&run_params(), &|name| {
                (name != missing)
                    .then(|| values.get(name))
                    .flatten()
                    .map(ToString::to_string)
            })
            .unwrap_err();
            assert!(error.contains(missing), "{error}");
        }
    }

    #[tokio::test]
    async fn formatter_uses_durable_bindings_is_source_local_and_never_prints_evidence_payload() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let id = EvidenceId::new();
        let mut evidence = EvidenceBundle::default();
        evidence.id = Some(id);
        evidence.requested_url = Some("https://user:secret@example.test/?token=secret".to_string());
        evidence.final_url = Some("https://example.test/final?cookie=secret".to_string());
        evidence.content = Some("SECRET EVIDENCE BODY".to_string());
        record_evidence(&store, evidence).await.unwrap();
        let reference = EvidenceRef::new(id);
        let session = ResearchSession {
            id: ResearchId::new(),
            topic: "topic".to_string(),
            extraction_instructions: None,
            sources: Vec::new(),
            source_bindings: vec![ResearchSourceBinding {
                source_number: 1,
                evidence: reference,
            }],
            extraction_diagnostics: Vec::new(),
            synthesis_diagnostic: None,
            counts: ResearchSessionCounts {
                search_results: 1,
                acquisition_attempts: 1,
                durable_sources: 1,
                observed_acquisitions: 1,
                successful_extractions: 1,
            },
            state: ResearchSessionState::CompletedWithoutSynthesisRequested,
            result: Some(DurableResearchResult {
                extractions: vec![DurableResearchExtraction {
                    source_number: 1,
                    evidence: reference,
                    extracted: ResearchExtraction {
                        facts: vec![ResearchExtractionFact {
                            topic: "Fact".to_string(),
                            finding: "Grounded finding".to_string(),
                        }],
                        missing_evidence: vec!["Source-local gap".to_string()],
                    },
                    extraction_input_bytes: 321,
                    finish_reason: Some(FinishReason::Stop),
                }],
                synthesis: None,
            }),
            created_at_unix_ms: 1,
            completed_at_unix_ms: Some(2),
        };
        for verbose in [false, true] {
            let output = format_session(&store, &session, verbose).await.unwrap();
            assert!(output.contains(&format!("[1] Evidence: {id}")));
            assert!(output.contains("Source 1:\n- Source-local gap"));
            assert!(!output.contains("SECRET"));
            assert!(!output.contains("https://"));
            assert!(!output.contains("token="));
            assert!(!output.contains("cookie="));
        }
    }

    #[tokio::test]
    async fn legacy_terminal_session_is_presented_without_fabrication() {
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let session = ResearchSession {
            id: ResearchId::new(),
            topic: "legacy".to_string(),
            extraction_instructions: None,
            sources: Vec::new(),
            source_bindings: Vec::new(),
            extraction_diagnostics: Vec::new(),
            synthesis_diagnostic: None,
            counts: Default::default(),
            state: ResearchSessionState::CompletedSuccessfully,
            result: None,
            created_at_unix_ms: 1,
            completed_at_unix_ms: Some(2),
        };
        let output = format_session(&store, &session, false).await.unwrap();
        assert!(output.contains("Durable result payload is unavailable"));
        assert!(!output.contains("Summary:"));
    }

    #[test]
    fn terminal_states_have_truthful_process_semantics() {
        let session = |state| ResearchSession {
            id: ResearchId::new(),
            topic: "topic".to_string(),
            extraction_instructions: None,
            sources: Vec::new(),
            source_bindings: Vec::new(),
            extraction_diagnostics: Vec::new(),
            synthesis_diagnostic: None,
            counts: Default::default(),
            state,
            result: None,
            created_at_unix_ms: 1,
            completed_at_unix_ms: Some(2),
        };
        for state in [
            ResearchSessionState::CompletedNoSearchResults,
            ResearchSessionState::CompletedNoExtractions,
            ResearchSessionState::CompletedWithoutSynthesisRequested,
            ResearchSessionState::CompletedSynthesisInsufficient,
            ResearchSessionState::CompletedSuccessfully,
        ] {
            assert_eq!(terminal_exit(&session(state)), (0, None));
        }
        for state in [
            ResearchSessionState::Claimed,
            ResearchSessionState::SearchFailed,
            ResearchSessionState::CompletedNoObservedAcquisitions,
            ResearchSessionState::CompletedSynthesisFailed,
        ] {
            let durable = session(state);
            let (code, error) = terminal_exit(&durable);
            assert_eq!(code, 2);
            assert!(error.is_some());
            assert!(format!("Research ID: {}", durable.id).contains("research_"));
        }
    }
}
