extern crate env_logger;

use clap::Parser;

#[derive(Parser)]
#[command(name = "spider-mcp", about = "MCP server for Spider web crawler")]
struct Cli {
    /// Log level (default: warn). Logs go to stderr.
    #[arg(long, default_value = "warn")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    env_logger::Builder::new()
        .parse_filters(&cli.log_level)
        .target(env_logger::Target::Stderr)
        .init();

    spider_mcp::serve_stdio().await
}
