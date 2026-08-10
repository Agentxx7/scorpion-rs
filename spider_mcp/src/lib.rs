//! Library seam for Scorpion's canonical MCP server implementation.
//!
//! [`serve_stdio`] is the single shared entry point both the `spider-mcp`
//! binary and `scorpion mcp` call into. Tool registration, schemas, and
//! server behavior all live in `server`/`tools`/`evidence`/`state`, kept
//! private to this crate — the only thing exposed across the crate
//! boundary is "start the canonical server over stdio and run it to
//! completion", so no caller can duplicate or drift from the real
//! implementation.

mod evidence;
mod server;
mod state;
#[cfg(test)]
mod test_support;
mod tools;
mod transport;

use rmcp::ServiceExt;
use server::SpiderMcpServer;

/// Start the canonical Spider MCP server over stdio and run it to
/// completion. Stdio is reserved for MCP protocol traffic exclusively —
/// callers must send their own logging/diagnostics to stderr (and finish
/// any log-level setup) before calling this.
pub async fn serve_stdio() -> Result<(), Box<dyn std::error::Error>> {
    let server = SpiderMcpServer::new();
    let transport = (tokio::io::stdin(), tokio::io::stdout());
    server.serve(transport).await?.waiting().await?;
    Ok(())
}
