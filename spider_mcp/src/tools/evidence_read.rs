//! Thin MCP binding for reading one already-persisted, durable Scorpion
//! [`EvidenceBundle`] by its canonical [`EvidenceRef`].
//!
//! **Read means read.** This module performs no network request, no
//! acquisition, no audit, and no evidence construction — it resolves one
//! existing durable evidence identity through the exact same seam
//! [`spider_audit_page`](crate::tools::audit) and every other evidence
//! consumer already uses (`EvidenceRef::resolve`, which itself calls
//! `read_evidence`), and serializes the [`EvidenceBundle`] it reads back
//! unchanged — no new timestamp, no recalculated hash, no reconstructed
//! provenance, no normalization. See
//! `SCORPION_MCP_CANONICAL_EVIDENCE_READ_001` and
//! `spider/tests/architecture_guardrails.rs`'s
//! `evidence_read_tool_has_a_precise_allowed_consumer_boundary`.
//!
//! # Store lifetime
//!
//! Mirrors `tools::audit`'s own decision exactly, for the same reason:
//! the canonical shared store is resolved fresh on every call through
//! [`open_shared_domain_store`], never a server-owned eagerly-opened
//! handle — `SpiderMcpServer::new()` keeps starting, and every unrelated
//! tool keeps working, even with no durable configuration
//! (`SCORPION_DOMAIN_DB`/`RESEARCH_EVIDENCE_DB`) present at all.

use rmcp::schemars;
use serde::Deserialize;
use serde_json::json;
use spider::features::domain_runtime::{open_shared_domain_store, DomainRuntimeError};
use spider::features::identity::EvidenceId;
use spider::utils::evidence::{EvidenceLedgerError, EvidenceRef};

#[derive(Deserialize, schemars::JsonSchema)]
pub struct EvidenceReadParams {
    /// The canonical EvidenceRef to read — the exact `evid_...` string returned by spider_audit_page's own `evidence_ref` field, passed through unchanged.
    pub evidence_ref: String,
}

fn evidence_read_error(code: &str, message: impl Into<String>) -> String {
    serde_json::to_string_pretty(&json!({
        "error": code,
        "message": message.into(),
    }))
    .expect("evidence read error JSON contains only serializable values")
}

fn map_store_error(error: DomainRuntimeError) -> String {
    match error {
        DomainRuntimeError::NotConfigured(message) => {
            evidence_read_error("evidence_store_not_configured", message)
        }
        DomainRuntimeError::Persistence(internal) => {
            eprintln!("spider-mcp evidence read tool: domain store open failed: {internal}");
            evidence_read_error(
                "evidence_store_unavailable",
                "the canonical evidence persistence store is unavailable",
            )
        }
    }
}

fn map_ledger_error(error: EvidenceLedgerError) -> String {
    match error {
        EvidenceLedgerError::Persistence(internal) => {
            eprintln!("spider-mcp evidence read tool: persistence read failed: {internal}");
            evidence_read_error(
                "evidence_read_failed",
                "failed to read the requested evidence from the canonical store",
            )
        }
        EvidenceLedgerError::Serialization(internal) => {
            eprintln!("spider-mcp evidence read tool: bundle deserialization failed: {internal}");
            evidence_read_error(
                "evidence_read_failed",
                "the persisted evidence record could not be decoded",
            )
        }
    }
}

/// Read one already-persisted `EvidenceBundle` by canonical `EvidenceRef`
/// and serialize it directly — no wrapper DTO, no field reconstruction.
/// Performs no network request and does not reconstruct evidence.
pub async fn run(params: EvidenceReadParams) -> Result<String, String> {
    run_with_environment(params, &|name| std::env::var(name).ok()).await
}

/// Real implementation, parameterized over environment lookup — mirrors
/// `tools::audit`'s own `run`/`run_with_environment` split, so tests can
/// deterministically exercise every configured/unconfigured/misconfigured
/// store shape without mutating real process environment.
async fn run_with_environment(
    params: EvidenceReadParams,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<String, String> {
    let raw = params.evidence_ref.trim();
    if raw.is_empty() {
        return Err(evidence_read_error(
            "invalid_request",
            "evidence_ref must not be empty",
        ));
    }
    let id: EvidenceId = raw.parse().map_err(|error| {
        evidence_read_error(
            "invalid_request",
            format!("evidence_ref is not a valid canonical EvidenceId: {error}"),
        )
    })?;

    let store = open_shared_domain_store(None, lookup)
        .await
        .map_err(map_store_error)?;

    let bundle = EvidenceRef::new(id)
        .resolve(&store)
        .await
        .map_err(map_ledger_error)?
        .ok_or_else(|| {
            evidence_read_error(
                "evidence_not_found",
                "no evidence has been durably recorded for this EvidenceRef",
            )
        })?;

    serde_json::to_string_pretty(&bundle)
        .map_err(|error| evidence_read_error("internal_error", error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use spider::features::domain_persistence::DomainPersistence;
    use spider::utils::evidence::{record_evidence, EvidenceBundle};
    use std::collections::BTreeMap;

    fn store_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "spider-mcp-evidence-read-tool-test-{}-{}.sqlite3",
            std::process::id(),
            EvidenceId::new()
        ))
    }

    fn configured_lookup(path: &std::path::Path) -> impl Fn(&str) -> Option<String> {
        let value = path.to_string_lossy().to_string();
        move |name| {
            if name == spider::features::domain_runtime::DOMAIN_DATABASE_ENV {
                Some(value.clone())
            } else {
                None
            }
        }
    }

    fn unconfigured_lookup() -> impl Fn(&str) -> Option<String> {
        |_name: &str| None
    }

    /// A bundle with representative values for every field this frontier
    /// requires proving fidelity for — no field the fixture cannot
    /// truthfully populate is fabricated.
    fn representative_bundle() -> EvidenceBundle {
        let mut response_headers = BTreeMap::new();
        response_headers.insert(
            "content-security-policy".to_string(),
            vec![b"default-src 'self'".to_vec()],
        );
        response_headers.insert("server".to_string(), vec![b"nginx/fixture".to_vec()]);
        EvidenceBundle {
            id: None, // assigned by record_evidence
            requested_url: Some("https://example.test/page".to_string()),
            final_url: Some("https://example.test/page-final".to_string()),
            retrieved_at: Some(1_700_000_000_123),
            status_code: Some(200),
            observed_status_code: Some(200),
            content_type: Some("text/html; charset=utf-8".to_string()),
            detected_content_type: Some("text/html".to_string()),
            response_body_hash: Some("abc123hash".to_string()),
            transformed_content_hash: Some("def456hash".to_string()),
            content: Some("<html><body>representative content</body></html>".to_string()),
            links: Some(vec![
                "https://example.test/a".to_string(),
                "https://example.test/b".to_string(),
            ]),
            transport: Some("default".to_string()),
            dns: None,
            backend_provenance: Some("reqwest".to_string()),
            response_origin: Some("network".to_string()),
            response_headers: Some(response_headers),
            ..Default::default()
        }
    }

    // ---- Phase 8: historical fidelity ----

    #[tokio::test]
    async fn read_returns_every_persisted_semantic_field_exactly_as_stored() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);

        let writer = DomainPersistence::open(&path).await.unwrap();
        let recorded = record_evidence(&writer, representative_bundle())
            .await
            .unwrap();
        let id = recorded.id.unwrap();
        drop(writer); // producer handle closed

        let output = run_with_environment(
            EvidenceReadParams {
                evidence_ref: id.to_string(),
            },
            &lookup, // independent consumer handle, opened inside run_with_environment
        )
        .await
        .unwrap();
        let read_back: EvidenceBundle = serde_json::from_str(&output).unwrap();

        assert_eq!(read_back.id, recorded.id);
        assert_eq!(read_back.requested_url, recorded.requested_url);
        assert_eq!(read_back.final_url, recorded.final_url);
        assert_eq!(read_back.retrieved_at, recorded.retrieved_at);
        assert_eq!(read_back.status_code, recorded.status_code);
        assert_eq!(
            read_back.observed_status_code,
            recorded.observed_status_code
        );
        assert_eq!(read_back.content_type, recorded.content_type);
        assert_eq!(
            read_back.detected_content_type,
            recorded.detected_content_type
        );
        assert_eq!(read_back.response_body_hash, recorded.response_body_hash);
        assert_eq!(
            read_back.transformed_content_hash,
            recorded.transformed_content_hash
        );
        assert_eq!(read_back.content, recorded.content);
        assert_eq!(read_back.links, recorded.links);
        assert_eq!(read_back.transport, recorded.transport);
        assert_eq!(read_back.dns, recorded.dns);
        assert_eq!(read_back.backend_provenance, recorded.backend_provenance);
        assert_eq!(read_back.response_origin, recorded.response_origin);
        assert_eq!(read_back.response_headers, recorded.response_headers);

        let _ = std::fs::remove_file(&path);
    }

    // ---- Phase 11: no-mutation contract ----

    #[tokio::test]
    async fn repeated_reads_are_idempotent_and_leave_history_unchanged() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);

        let writer = DomainPersistence::open(&path).await.unwrap();
        let recorded = record_evidence(&writer, representative_bundle())
            .await
            .unwrap();
        let id = recorded.id.unwrap();
        let history_before = writer.read_history(&id.to_string()).await.unwrap();
        drop(writer);

        let first = run_with_environment(
            EvidenceReadParams {
                evidence_ref: id.to_string(),
            },
            &lookup,
        )
        .await
        .unwrap();
        let second = run_with_environment(
            EvidenceReadParams {
                evidence_ref: id.to_string(),
            },
            &lookup,
        )
        .await
        .unwrap();
        assert_eq!(first, second, "repeated reads must return identical output");

        let checker = DomainPersistence::open(&path).await.unwrap();
        let history_after = checker.read_history(&id.to_string()).await.unwrap();
        assert_eq!(
            history_before.len(),
            history_after.len(),
            "reading must never append/mutate history"
        );
        assert_eq!(history_before, history_after);

        let _ = std::fs::remove_file(&path);
    }

    // ---- Phase 19: negative matrix ----

    #[tokio::test]
    async fn empty_evidence_ref_is_invalid_request() {
        let path = store_path();
        let lookup = configured_lookup(&path);
        let result = run_with_environment(
            EvidenceReadParams {
                evidence_ref: String::new(),
            },
            &lookup,
        )
        .await;
        let error: serde_json::Value = serde_json::from_str(&result.unwrap_err()).unwrap();
        assert_eq!(error["error"], "invalid_request");
    }

    #[tokio::test]
    async fn whitespace_only_evidence_ref_is_invalid_request() {
        let path = store_path();
        let lookup = configured_lookup(&path);
        let result = run_with_environment(
            EvidenceReadParams {
                evidence_ref: "   ".to_string(),
            },
            &lookup,
        )
        .await;
        let error: serde_json::Value = serde_json::from_str(&result.unwrap_err()).unwrap();
        assert_eq!(error["error"], "invalid_request");
    }

    #[tokio::test]
    async fn malformed_prefix_is_invalid_request() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        for malformed in [
            "not-an-evidence-ref",
            "watch_0123456789abcdef0123456789abcdef",
            "evid_tooShort",
            "https://example.test/",
        ] {
            let result = run_with_environment(
                EvidenceReadParams {
                    evidence_ref: malformed.to_string(),
                },
                &lookup,
            )
            .await;
            let error: serde_json::Value = serde_json::from_str(&result.unwrap_err()).unwrap();
            assert_eq!(
                error["error"], "invalid_request",
                "{malformed:?} must be rejected as invalid_request"
            );
        }
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn valid_but_absent_evidence_id_is_not_found() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let never_recorded = EvidenceId::new();
        let result = run_with_environment(
            EvidenceReadParams {
                evidence_ref: never_recorded.to_string(),
            },
            &lookup,
        )
        .await;
        let error: serde_json::Value = serde_json::from_str(&result.unwrap_err()).unwrap();
        assert_eq!(error["error"], "evidence_not_found");
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn missing_store_configuration_is_not_configured() {
        let result = run_with_environment(
            EvidenceReadParams {
                evidence_ref: EvidenceId::new().to_string(),
            },
            &unconfigured_lookup(),
        )
        .await;
        let error: serde_json::Value = serde_json::from_str(&result.unwrap_err()).unwrap();
        assert_eq!(error["error"], "evidence_store_not_configured");
    }

    #[tokio::test]
    async fn unopenable_store_fails_safely_without_leaking_the_path() {
        let dir = std::env::temp_dir().join(format!(
            "spider-mcp-evidence-read-tool-unopenable-{}-{}",
            std::process::id(),
            EvidenceId::new()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let lookup = configured_lookup(&dir);

        let result = run_with_environment(
            EvidenceReadParams {
                evidence_ref: EvidenceId::new().to_string(),
            },
            &lookup,
        )
        .await;
        let raw = result.unwrap_err();
        let error: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(error["error"], "evidence_store_unavailable");
        let path_string = dir.to_string_lossy().to_string();
        assert!(!raw.contains(&path_string));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn no_interpretation_fields_and_no_credentials_in_output() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);

        let writer = DomainPersistence::open(&path).await.unwrap();
        let recorded = record_evidence(&writer, representative_bundle())
            .await
            .unwrap();
        let id = recorded.id.unwrap();
        drop(writer);

        let output = run_with_environment(
            EvidenceReadParams {
                evidence_ref: id.to_string(),
            },
            &lookup,
        )
        .await
        .unwrap();

        for forbidden in [
            "summary",
            "risk_score",
            "confidence",
            "recommendation",
            "assessment",
            "authorization",
            "proxy-authorization",
            "set-cookie",
            "\"cookie\"",
        ] {
            assert!(
                !output.to_lowercase().contains(forbidden),
                "response must not contain {forbidden:?}: {output}"
            );
        }
        assert!(!output.contains(&path.to_string_lossy().to_string()));

        let _ = std::fs::remove_file(&path);
    }
}
