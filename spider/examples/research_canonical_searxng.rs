//! Live operator check for canonical Spider-backed research acquisition.
//!
//! This retains evidence in memory for inspection. It does not record an
//! `EvidenceRef` or claim durable evidence persistence.

use spider::agent::{Agent, AgentConfig, ResearchOptions, SearchOptions};
use spider::features::agent_acquisition::CanonicalPageAcquirer;
use std::collections::HashSet;

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

    let canonical_acquirer = CanonicalPageAcquirer::default();
    let evidence_handle = canonical_acquirer.clone();
    let agent = Agent::builder()
        .with_config(
            AgentConfig::default()
                .with_html_max_bytes(10_000)
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

    for extraction in &research.extractions {
        println!(
            "Successful extraction: url={} acquisition_id={}",
            extraction.url,
            extraction.acquisition_id.as_deref().unwrap_or("none")
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
    println!("Evidence was retained in memory; no EvidenceRef was persisted.");

    Ok(())
}
