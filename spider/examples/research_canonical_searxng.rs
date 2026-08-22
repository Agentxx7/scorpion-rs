//! Live operator check for canonical Spider-backed research acquisition.
//!
//! By default this retains evidence ephemerally for inspection. With the
//! `disk` feature and `RESEARCH_EVIDENCE_DB` set, it records through the
//! canonical evidence ledger, drops the adapter/store, reopens the database,
//! and resolves every successfully extracted acquisition ID as an
//! `EvidenceRef`.

use spider::agent::{Agent, AgentConfig, ResearchOptions, SearchOptions};
use spider::features::agent_acquisition::CanonicalPageAcquirer;
#[cfg(feature = "disk")]
use spider::{
    features::{domain_persistence::DomainPersistence, identity::EvidenceId},
    utils::evidence::{AcquisitionOptions, EvidenceRef},
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
    #[cfg(feature = "disk")]
    let canonical_acquirer = match evidence_database.as_deref() {
        Some(path) => CanonicalPageAcquirer::new_durable(
            AcquisitionOptions::default(),
            Arc::new(DomainPersistence::open(path).await?),
        ),
        None => CanonicalPageAcquirer::default(),
    };
    #[cfg(not(feature = "disk"))]
    let canonical_acquirer = {
        if evidence_database.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "RESEARCH_EVIDENCE_DB requires the spider disk feature",
            )
            .into());
        }
        CanonicalPageAcquirer::default()
    };
    let evidence_handle = canonical_acquirer.clone();
    let agent = Agent::builder()
        .with_config(
            AgentConfig::default()
                .with_html_max_bytes(HTML_MAX_BYTES)
                .with_max_tokens(1024),
        )
        .with_openai_compatible(openai_base_url, api_key, model)
        .with_search_searxng(searxng_base_url)
        .with_page_acquirer(Box::new(canonical_acquirer))
        .build()?;

    let query = "How do Tokio and async-std compare for Rust async programming?";
    println!("Research query: {query}");
    let research = agent
        .research(
            query,
            ResearchOptions::new()
                .with_max_pages(3)
                .with_search_options(SearchOptions::new().with_limit(5))
                .with_extraction_prompt(
                    "Extract key differences, pros, cons, and use cases for the async runtimes mentioned.",
                )
                .with_synthesize(true),
        )
        .await?;

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
    #[cfg(feature = "disk")]
    let durable_ids: Vec<String> = research
        .extractions
        .iter()
        .filter_map(|extraction| extraction.acquisition_id.clone())
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

    let retained = evidence_handle.retained_evidence();
    println!("Canonical acquisition attempts: {}", retained.len());
    println!("Retained canonical evidence count: {}", retained.len());
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
        drop(retained);
        drop(evidence_handle);
        drop(agent);
        let reopened = DomainPersistence::open(&path).await?;
        for id in durable_ids {
            let evidence_id: EvidenceId = id.parse()?;
            let evidence = EvidenceRef::new(evidence_id)
                .resolve(&reopened)
                .await?
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("durable evidence {id} was not resolvable after reopen"),
                    )
                })?;
            println!(
                "Reopened durable evidence: acquisition_id={} requested_url={} final_url={} transport={} dns={} backend_provenance={} response_origin={}",
                id,
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
        println!("Durable evidence resolved from the reopened canonical ledger.");
    } else {
        println!("Evidence was retained ephemerally; no EvidenceRef was persisted.");
    }
    #[cfg(not(feature = "disk"))]
    println!("Evidence was retained ephemerally; no EvidenceRef was persisted.");
    Ok(())
}
