//! Live operator check for canonical Spider-backed research acquisition.
//!
//! By default this retains evidence ephemerally for inspection. With the
//! `disk` feature and `RESEARCH_EVIDENCE_DB` set, it records through the
//! canonical research-session boundary, drops its adapter/store, reopens the
//! database, reloads the same `ResearchId`, and resolves every ordered
//! Source-N `EvidenceRef`.

use spider::agent::{Agent, AgentConfig, ResearchOptions, SearchOptions};
use spider::features::agent_acquisition::CanonicalPageAcquirer;
#[cfg(feature = "disk")]
use spider::features::{
    domain_persistence::DomainPersistence,
    research_session::{read_research_session, run_durable_research},
};
use std::collections::{HashMap, HashSet};
#[cfg(feature = "disk")]
use std::sync::Arc;

const HTML_MAX_BYTES: usize = 10_000;
const PREVIEW_CHARS: usize = 1_200;

fn bounded_preview(content: &str) -> String {
    let mut preview: String = content.chars().take(PREVIEW_CHARS).collect();
    if content.chars().count() > PREVIEW_CHARS {
        preview.push_str("\n...[preview truncated]...");
    }
    preview
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let searxng_base_url =
        std::env::var("SEARXNG_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
    let openai_base_url = std::env::var("OPENAI_COMPAT_BASE_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:11434/v1".to_string());
    let model = std::env::var("OPENAI_COMPAT_MODEL")
        .expect("OPENAI_COMPAT_MODEL must name the model served by llama.cpp");
    let api_key = std::env::var("OPENAI_COMPAT_API_KEY").unwrap_or_else(|_| "local".to_string());

    let evidence_database = std::env::var_os("RESEARCH_EVIDENCE_DB").map(std::path::PathBuf::from);
    let builder = Agent::builder()
        .with_config(
            AgentConfig::default()
                .with_html_max_bytes(HTML_MAX_BYTES)
                .with_max_tokens(1024),
        )
        .with_openai_compatible(openai_base_url, api_key, model)
        .with_search_searxng(searxng_base_url);

    let query = "How do Tokio and async-std compare for Rust async programming?";
    let options = ResearchOptions::new()
        .with_max_pages(3)
        .with_search_options(SearchOptions::new().with_limit(5))
        .with_extraction_prompt(
            "Extract key differences, pros, cons, and use cases for the async runtimes mentioned.",
        )
        .with_synthesize(true);
    println!("Research query: {query}");
    #[cfg(feature = "disk")]
    let (research, evidence_handle, durable_session) = match evidence_database.as_deref() {
        Some(path) => {
            let run = run_durable_research(
                Arc::new(DomainPersistence::open(path).await?),
                builder,
                query,
                options,
            )
            .await?;
            let research = run.result?;
            (research, None, Some(run.session))
        }
        None => {
            let canonical_acquirer = CanonicalPageAcquirer::default();
            let evidence_handle = canonical_acquirer.clone();
            let agent = builder
                .with_page_acquirer(Box::new(canonical_acquirer))
                .build()?;
            (
                agent.research(query, options).await?,
                Some(evidence_handle),
                None,
            )
        }
    };
    #[cfg(not(feature = "disk"))]
    let (research, evidence_handle) = {
        if evidence_database.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "RESEARCH_EVIDENCE_DB requires the spider disk feature",
            )
            .into());
        }
        let canonical_acquirer = CanonicalPageAcquirer::default();
        let evidence_handle = canonical_acquirer.clone();
        let agent = builder
            .with_page_acquirer(Box::new(canonical_acquirer))
            .build()?;
        (agent.research(query, options).await?, Some(evidence_handle))
    };

    println!("Search result count: {}", research.search_results.len());
    println!(
        "Successful extraction count: {}",
        research.extractions.len()
    );
    let successful_ids: HashSet<&str> = research
        .extractions
        .iter()
        .filter_map(|extraction| extraction.acquisition_id.as_deref())
        .collect();
    let extraction_input_bytes: HashMap<&str, usize> = research
        .extractions
        .iter()
        .filter_map(|extraction| {
            extraction
                .acquisition_id
                .as_deref()
                .map(|id| (id, extraction.extraction_input_bytes))
        })
        .collect();
    for extraction in &research.extractions {
        println!(
            "Successful extraction: url={} acquisition_id={} extraction_input_bytes={} facts={} missing_evidence={} finish_reason={:?} json={}",
            extraction.url,
            extraction.acquisition_id.as_deref().unwrap_or("none"),
            extraction.extraction_input_bytes,
            extraction.extracted.facts.len(),
            extraction.extracted.missing_evidence.len(),
            extraction.finish_reason,
            bounded_preview(&serde_json::to_string_pretty(&extraction.extracted)?)
        );
    }

    #[cfg(feature = "disk")]
    let retained = if let Some(session) = durable_session.as_ref() {
        println!(
            "Canonical acquisition attempts: {}",
            session.counts.acquisition_attempts
        );
        println!(
            "Durable canonical evidence count: {}",
            session.counts.durable_sources
        );
        Vec::new()
    } else {
        let retained = evidence_handle
            .as_ref()
            .expect("ephemeral mode must retain its canonical acquirer")
            .retained_evidence();
        println!("Canonical acquisition attempts: {}", retained.len());
        println!("Retained canonical evidence count: {}", retained.len());
        retained
    };
    #[cfg(not(feature = "disk"))]
    let retained = {
        let retained = evidence_handle
            .as_ref()
            .expect("ephemeral mode must retain its canonical acquirer")
            .retained_evidence();
        println!("Canonical acquisition attempts: {}", retained.len());
        println!("Retained canonical evidence count: {}", retained.len());
        retained
    };
    for record in &retained {
        let id = record.acquisition_id.to_string();
        let outcome = if successful_ids.contains(id.as_str()) {
            "extracted"
        } else {
            "rejected/skipped before successful extraction"
        };
        println!(
            "Canonical evidence: acquisition_id={} requested_url={} final_url={} status={} outcome={}",
            id,
            record.evidence.requested_url.as_deref().unwrap_or("unknown"),
            record.evidence.final_url.as_deref().unwrap_or("unknown"),
            record
                .evidence
                .observed_status_code
                .or(record.evidence.status_code)
                .map(|status| status.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            outcome
        );

        match record.evidence.content.as_deref() {
            Some(original) => match spider_agent_html::materialize_research_markdown(
                original,
                record.evidence.final_url.as_deref().unwrap_or_default(),
            ) {
                Ok(readable) => {
                    println!(
                        "Research-readable input: acquisition_id={} original_body_bytes={} derived_readable_bytes={} admission=admitted extraction_input_bytes={}",
                        id,
                        original.len(),
                        readable.len(),
                        extraction_input_bytes
                            .get(id.as_str())
                            .map(|bytes| bytes.to_string())
                            .unwrap_or_else(|| "not supplied to a successful extraction".to_string())
                    );
                }
                Err(error) => println!(
                    "Research-readable input: acquisition_id={} original_body_bytes={} derived_readable_bytes=none admission=rejected reason={:?}",
                    id,
                    original.len(),
                    error.to_string()
                ),
            },
            None => println!(
                "Research-readable input: acquisition_id={} original_body_bytes=none derived_readable_bytes=none admission=rejected reason={:?}",
                id,
                "canonical evidence contains no materialized body"
            ),
        }
    }

    println!("Final synthesis:");
    println!(
        "{}",
        research
            .summary
            .as_deref()
            .unwrap_or("No synthesis produced")
    );
    println!("Final research output complete.");
    #[cfg(feature = "disk")]
    if let Some(path) = evidence_database {
        let durable_session = durable_session.expect("durable path must return a session");
        let research_id = durable_session.id;
        println!(
            "Durable research session: research_id={} state={:?} durable_sources={} successful_extractions={}",
            research_id,
            durable_session.state,
            durable_session.counts.durable_sources,
            durable_session.counts.successful_extractions
        );
        drop(durable_session);
        let reopened = DomainPersistence::open(&path).await?;
        let (_, reopened_session) = read_research_session(&reopened, research_id)
            .await?
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("durable research session {research_id} was not found after reopen"),
                )
            })?;
        println!(
            "Reopened durable research session: research_id={} state={:?}",
            reopened_session.id, reopened_session.state
        );
        let reopened_result = reopened_session.result.as_ref().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("durable research session {research_id} has no reopened result payload"),
            )
        })?;
        for extraction in &reopened_result.extractions {
            println!(
                "Reopened durable extraction: source=Source {} evidence_id={} extraction_input_bytes={} finish_reason={:?} facts={} missing_evidence={} json={}",
                extraction.source_number,
                extraction.evidence.id(),
                extraction.extraction_input_bytes,
                extraction.finish_reason,
                extraction.extracted.facts.len(),
                extraction.extracted.missing_evidence.len(),
                bounded_preview(&serde_json::to_string_pretty(&extraction.extracted)?)
            );
        }
        if let Some(synthesis) = &reopened_result.synthesis {
            println!("Reopened durable final synthesis:");
            println!("{}", synthesis.summary);
            println!(
                "Reopened synthesis token usage: prompt={} completion={} total={}",
                synthesis.usage.prompt_tokens,
                synthesis.usage.completion_tokens,
                synthesis.usage.total_tokens
            );
            for citation in &synthesis.citations {
                citation.evidence.resolve(&reopened).await?.ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!(
                            "reopened synthesis citation Source {} evidence {} was not resolvable",
                            citation.source_number,
                            citation.evidence.id()
                        ),
                    )
                })?;
                println!(
                    "Reopened durable citation: source=Source {} evidence_id={}",
                    citation.source_number,
                    citation.evidence.id()
                );
            }
        } else {
            println!("Reopened durable final synthesis: none for this terminal state");
        }
        for binding in &reopened_session.source_bindings {
            let evidence = binding.evidence.resolve(&reopened).await?.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!(
                        "{} evidence {} was not resolvable after reopen",
                        binding.source_label(),
                        binding.evidence.id()
                    ),
                )
            })?;
            println!(
                "Reopened durable binding: research_id={} source={} evidence_id={} requested_url={} final_url={} transport={} dns={} backend_provenance={} response_origin={}",
                reopened_session.id,
                binding.source_label(),
                binding.evidence.id(),
                evidence.requested_url.as_deref().unwrap_or("unknown"),
                evidence.final_url.as_deref().unwrap_or("unknown"),
                evidence.transport.as_deref().unwrap_or("unknown"),
                evidence.dns.as_deref().unwrap_or("unspecified"),
                evidence
                    .backend_provenance
                    .as_deref()
                    .unwrap_or("unknown"),
                evidence.response_origin.as_deref().unwrap_or("unknown")
            );
        }
        println!(
            "Durable research result, session, and Source-N evidence resolved from the reopened canonical ledger."
        );
    } else {
        println!("Evidence was retained ephemerally; no EvidenceRef was persisted.");
    }
    #[cfg(not(feature = "disk"))]
    println!("Evidence was retained ephemerally; no EvidenceRef was persisted.");
    Ok(())
}
