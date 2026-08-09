//! `scorpion mcp` — launches the exact same canonical MCP server
//! implementation as the standalone `spider-mcp` binary, via spider_mcp's
//! own library seam (`spider_mcp::serve_stdio`). No `SpiderMcpServer`/tool
//! registration code lives here or is duplicated here.

/// Configure stderr logging at the requested level, then hand off to the
/// shared, canonical `spider_mcp::serve_stdio` entry point — identical to
/// what `spider-mcp`'s own `main.rs` does. `main.rs` dispatches the `mcp`
/// command before it ever considers the generic `--verbose` flag, so this
/// is always the first (and only) logger-initialization attempt in the
/// process — `init()` (not `try_init()`) is used deliberately: a second,
/// unexpected initialization attempt here would indicate a real ordering
/// bug elsewhere, and should panic loudly rather than be silently
/// swallowed, which is exactly what previously let `--verbose` silently
/// override this command's own `--log-level`.
pub async fn run(log_level: &str) -> Result<(), String> {
    env_logger::Builder::new()
        .parse_filters(log_level)
        .target(env_logger::Target::Stderr)
        .init();

    spider_mcp::serve_stdio()
        .await
        .map_err(|error| error.to_string())
}
