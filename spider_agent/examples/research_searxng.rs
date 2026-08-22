//! Research example using spider_agent against a self-hosted SearXNG
//! instance and a local OpenAI-compatible LLM endpoint — no commercial
//! search or LLM API key required.
//!
//! Calls the exact same `Agent::research()` as `examples/research.rs`;
//! only the search provider and LLM endpoint configuration differ.
//!
//! Run with:
//! ```sh
//! SEARXNG_BASE_URL=http://localhost:8080 \
//! OPENAI_COMPAT_BASE_URL=http://localhost:11434/v1 \
//! OPENAI_COMPAT_MODEL=llama3.1 \
//! cargo run -p spider_agent --example research_searxng --features "openai search_searxng"
//! ```
//!
//! `OPENAI_COMPAT_API_KEY` is optional — most local OpenAI-compatible
//! servers (Ollama, llama.cpp's server, vLLM, LM Studio, ...) accept any
//! non-empty placeholder value, so one is supplied automatically when the
//! variable is absent.

use spider_agent::{Agent, AgentConfig, ResearchOptions, SearchOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let searxng_base_url = std::env::var("SEARXNG_BASE_URL")
        .expect("SEARXNG_BASE_URL environment variable required (e.g. http://localhost:8080)");
    let openai_compat_base_url = std::env::var("OPENAI_COMPAT_BASE_URL").expect(
        "OPENAI_COMPAT_BASE_URL environment variable required (e.g. http://localhost:11434/v1)",
    );
    let openai_compat_model = std::env::var("OPENAI_COMPAT_MODEL")
        .expect("OPENAI_COMPAT_MODEL environment variable required");
    let openai_compat_api_key =
        std::env::var("OPENAI_COMPAT_API_KEY").unwrap_or_else(|_| "local".to_string());

    let agent = Agent::builder()
        .with_config(
            AgentConfig::default()
                .with_html_max_bytes(10_000)
                .with_max_tokens(1024),
        )
        .with_openai_compatible(
            openai_compat_base_url,
            openai_compat_api_key,
            openai_compat_model,
        )
        .with_search_searxng(searxng_base_url)
        .build()?;

    println!("Researching: How do Tokio and async-std compare?\n");
    println!("This will search (SearXNG), fetch pages, extract data, and synthesize findings...\n");

    let research = agent
        .research(
            "How do Tokio and async-std compare for Rust async programming?",
            ResearchOptions::new()
                .with_max_pages(3)
                .with_search_options(SearchOptions::new().with_limit(5))
                .with_extraction_prompt(
                    "Extract key differences, pros, cons, and use cases for the async runtimes mentioned.",
                )
                .with_synthesize(true),
        )
        .await?;

    println!("=== Search Results ===");
    println!("Found {} results\n", research.search_results.len());

    println!("=== Extracted Data ===");
    for (i, extraction) in research.extractions.iter().enumerate() {
        println!("\n{}. {} ({})", i + 1, extraction.title, extraction.url);
        println!(
            "   Extracted: {}",
            serde_json::to_string(&extraction.extracted)
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect::<String>()
        );
    }

    if let Some(summary) = research.summary {
        println!("\n=== Summary ===");
        println!("{}", summary);
    }

    println!("\n=== Token Usage ===");
    println!(
        "Prompt: {}, Completion: {}, Total: {}",
        research.usage.prompt_tokens, research.usage.completion_tokens, research.usage.total_tokens
    );

    Ok(())
}
