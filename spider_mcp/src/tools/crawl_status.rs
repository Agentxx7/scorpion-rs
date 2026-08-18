//! Completes the background-crawl lifecycle that `spider_crawl` already
//! promises but never made reachable: `crawl.rs`'s background-session branch
//! tells callers to "Use spider_crawl_status tool or read resource
//! crawl://{crawl_id}/summary" — neither existed. This module is the one
//! shared read-side surface for both.
//!
//! No new session store: everything here reads `SharedState.sessions`
//! (`crate::state`), which `spider_crawl`'s background path already
//! populates. `summarize` is the single serialization helper consumed by
//! both the `spider_crawl_status` tool (`server.rs`) and the
//! `crawl://{crawl_id}/summary` resource (`read_resource` below), so the two
//! surfaces cannot drift.

use rmcp::schemars;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::state::SharedState;

/// URI template advertised via `list_resource_templates` and matched by
/// [`parse_crawl_id_from_uri`]. Single source of truth for both directions.
pub const RESOURCE_URI_TEMPLATE: &str = "crawl://{crawl_id}/summary";

#[derive(Deserialize, schemars::JsonSchema)]
pub struct CrawlStatusParams {
    /// The crawl_id returned by a prior spider_crawl call that used the
    /// background/session path (crawl limit above the inline threshold).
    pub crawl_id: String,
}

/// Build the deterministic lifecycle summary for one crawl session. Shared by
/// the `spider_crawl_status` tool and the `crawl://{id}/summary` resource so
/// both surfaces report identical data. `Err` for an unknown/expired
/// crawl_id — callers must not paper over that with an empty success.
pub fn summarize(state: &SharedState, crawl_id: &str) -> Result<serde_json::Value, String> {
    let session = state
        .sessions
        .get(crawl_id)
        .ok_or_else(|| format!("No crawl session found for crawl_id \"{crawl_id}\""))?;

    Ok(json!({
        "crawl_id": crawl_id,
        "status": session.status,
        "page_count": session.pages.len(),
        "pages": session.pages,
        "error": session.error,
        "error_code": session.error_code,
    }))
}

/// `spider_crawl_status` tool entry point.
pub fn run(params: CrawlStatusParams, state: Arc<SharedState>) -> Result<String, String> {
    let summary = summarize(&state, &params.crawl_id)?;
    serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())
}

/// Parse a `crawl://{crawl_id}/summary` URI into its crawl_id. `None` for any
/// URI that doesn't match the advertised template shape.
pub fn parse_crawl_id_from_uri(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("crawl://")?;
    let id = rest.strip_suffix("/summary")?;
    (!id.is_empty()).then(|| id.to_string())
}

/// The resource template advertised via `list_resource_templates`. Also
/// exercised directly by the offline reachability test below, so the test
/// proves the exact value the trait method will return rather than a
/// parallel copy of it.
pub fn resource_template() -> rmcp::model::ResourceTemplate {
    rmcp::model::Annotated::new(
        rmcp::model::RawResourceTemplate {
            uri_template: RESOURCE_URI_TEMPLATE.to_string(),
            name: "crawl_summary".to_string(),
            title: Some("Background crawl summary".to_string()),
            description: Some(
                "Lifecycle summary (status, page count, collected pages) for a background \
                 crawl session started via spider_crawl. Only reachable for crawls that used \
                 the background/session path (limit above the inline threshold)."
                    .to_string(),
            ),
            mime_type: Some("application/json".to_string()),
            icons: None,
        },
        None,
    )
}

/// `crawl://{crawl_id}/summary` resource entry point (`read_resource`).
/// Fails explicitly — never an empty success — for an unrecognized URI shape
/// or an unknown/expired crawl_id.
pub fn read_resource(
    uri: &str,
    state: &SharedState,
) -> Result<rmcp::model::ReadResourceResult, rmcp::ErrorData> {
    let crawl_id = parse_crawl_id_from_uri(uri).ok_or_else(|| {
        rmcp::ErrorData::invalid_params(format!("Unrecognized resource URI: {uri}"), None)
    })?;
    let summary =
        summarize(state, &crawl_id).map_err(|e| rmcp::ErrorData::resource_not_found(e, None))?;
    let text = serde_json::to_string_pretty(&summary)
        .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;

    Ok(rmcp::model::ReadResourceResult::new(vec![
        rmcp::model::ResourceContents::text(text, uri.to_string()),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{CrawlPageResult, CrawlSession, CrawlSessionStatus};
    use std::time::Instant;

    fn session(status: CrawlSessionStatus, pages: Vec<CrawlPageResult>) -> CrawlSession {
        CrawlSession {
            status,
            pages,
            started_at: Instant::now(),
            abort: None,
            error: None,
            error_code: None,
        }
    }

    /// 1. A stored running crawl session can be queried by crawl_id.
    #[test]
    fn running_session_is_queryable_by_crawl_id() {
        let state = SharedState::new();
        state
            .sessions
            .insert("run-1".into(), session(CrawlSessionStatus::Running, vec![]));

        let summary = summarize(&state, "run-1").expect("running session must be found");
        assert_eq!(summary["crawl_id"], "run-1");
        assert_eq!(summary["status"], "running");
        assert_eq!(summary["page_count"], 0);
    }

    /// 2. A completed session returns deterministic status/summary data.
    #[test]
    fn completed_session_returns_deterministic_summary() {
        let state = SharedState::new();
        let pages = vec![CrawlPageResult {
            url: "http://example.test/".to_string(),
            content: "# hello".to_string(),
            status_code: 200,
            links: vec!["http://example.test/a".to_string()],
            provenance: spider::utils::evidence::PageProvenance::default(),
        }];
        state.sessions.insert(
            "done-1".into(),
            session(CrawlSessionStatus::Complete, pages),
        );

        let summary = summarize(&state, "done-1").expect("complete session must be found");
        assert_eq!(summary["status"], "complete");
        assert_eq!(summary["page_count"], 1);
        assert_eq!(summary["pages"][0]["url"], "http://example.test/");
        assert_eq!(summary["pages"][0]["status_code"], 200);
    }

    /// 3. A failed session reports Failed.
    #[test]
    fn failed_session_reports_failed_status() {
        let state = SharedState::new();
        state
            .sessions
            .insert("fail-1".into(), session(CrawlSessionStatus::Failed, vec![]));

        let summary = summarize(&state, "fail-1").expect("failed session must still be found");
        assert_eq!(summary["status"], "failed");
    }

    /// 3b (Section N): a Tor transport preflight rejection reported through
    /// a background crawl session must surface both the human-readable
    /// `error` and the machine-readable `error_code` in the status
    /// summary — the classification is never lost between the session and
    /// this read path.
    #[test]
    fn failed_session_reports_transport_error_and_error_code() {
        let state = SharedState::new();
        let mut failed = session(CrawlSessionStatus::Failed, vec![]);
        failed.error = Some(
            "mode is \"tor\" but no proxy endpoint was supplied — Tor requires an explicit \
             socks5h://HOST:PORT endpoint; there is no implicit or environment-derived default"
                .to_string(),
        );
        failed.error_code = Some("incompatible_configuration".to_string());
        state.sessions.insert("fail-transport-1".into(), failed);

        let summary =
            summarize(&state, "fail-transport-1").expect("failed session must still be found");
        assert_eq!(summary["status"], "failed");
        assert_eq!(summary["page_count"], 0);
        assert_eq!(summary["error_code"], "incompatible_configuration");
        assert!(summary["error"]
            .as_str()
            .unwrap()
            .contains("no proxy endpoint was supplied"));
    }

    /// A non-transport failure (e.g. the pre-existing screenshot
    /// fail-closed path) leaves `error`/`error_code` `null` — the new
    /// fields are additive, not a requirement on every `Failed` session.
    #[test]
    fn failed_session_without_transport_error_has_null_error_fields() {
        let state = SharedState::new();
        state.sessions.insert(
            "fail-plain-1".into(),
            session(CrawlSessionStatus::Failed, vec![]),
        );

        let summary = summarize(&state, "fail-plain-1").unwrap();
        assert_eq!(summary["status"], "failed");
        assert!(summary["error"].is_null());
        assert!(summary["error_code"].is_null());
    }

    /// 4. Unknown crawl_id returns an explicit not-found result, not an
    /// empty success.
    #[test]
    fn unknown_crawl_id_is_explicit_not_found() {
        let state = SharedState::new();
        let result = summarize(&state, "does-not-exist");
        assert!(
            result.is_err(),
            "unknown crawl_id must not silently succeed"
        );
        assert!(result.unwrap_err().contains("does-not-exist"));
    }

    /// 4b. Same fail-closed contract through the resource entry point.
    #[test]
    fn unknown_crawl_id_resource_read_fails_explicitly() {
        let state = SharedState::new();
        let result = read_resource("crawl://does-not-exist/summary", &state);
        assert!(
            result.is_err(),
            "unknown crawl_id must not silently succeed via the resource"
        );
    }

    /// URI parsing is the other half of resource reachability — exercised
    /// directly since it's pure and needs no MCP plumbing.
    #[test]
    fn parses_crawl_id_from_well_formed_uri() {
        assert_eq!(
            parse_crawl_id_from_uri("crawl://abc-123/summary"),
            Some("abc-123".to_string())
        );
        assert_eq!(parse_crawl_id_from_uri("crawl:///summary"), None);
        assert_eq!(parse_crawl_id_from_uri("http://example.com/"), None);
    }

    /// 5 (resource half). The resource template `read_resource`/
    /// `list_resource_templates` will actually advertise matches the
    /// documented URI shape — proves the exact value the trait method
    /// returns, not a parallel copy of it.
    #[test]
    fn resource_template_advertises_documented_uri_shape() {
        let template = resource_template();
        assert_eq!(template.uri_template, RESOURCE_URI_TEMPLATE);
    }
}
