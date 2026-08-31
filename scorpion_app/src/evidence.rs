//! Thin application boundary for reading one already-persisted, durable
//! Scorpion [`EvidenceBundle`] by its canonical `EvidenceRef` string — the
//! Web Console's peer of `spider_mcp`'s own `spider_evidence_read` MCP
//! tool, over the identical canonical seam.
//!
//! **Read means read.** This module performs no network request, no
//! acquisition, no audit, and no evidence construction — it resolves one
//! existing durable evidence identity through the exact same seam
//! `spider_evidence_read` uses (`EvidenceRef::resolve`, which itself calls
//! `read_evidence`), and serializes the [`EvidenceBundle`] it reads back
//! unchanged — no new timestamp, no recalculated hash, no reconstructed
//! provenance, no normalization. See
//! `SCORPION_WEB_CONSOLE_CANONICAL_EVIDENCE_INSPECTION_001` and
//! `spider/tests/architecture_guardrails.rs`'s
//! `evidence_read_tool_has_a_precise_allowed_consumer_boundary`.
//!
//! # Peer interface, not a second implementation
//!
//! MCP and the Web Console are peer interfaces over the same canonical
//! core: neither calls the other. This module does not talk to
//! `spider_mcp` in any way — it opens the same shared
//! [`DomainPersistence`](spider::features::domain_persistence::DomainPersistence)
//! store directly, through the identical
//! [`open_shared_domain_store`] seam, so "what an AI resolved through MCP"
//! and "what a human resolves here" are the same persisted record, never
//! two independently derived ones.
//!
//! # Store lifetime
//!
//! The shared store is resolved fresh on every call, never a
//! process-owned eagerly-opened handle — mirrors `spider_mcp`'s own
//! `tools::audit`/`tools::evidence_read` decision, for the same reason:
//! `scorpion-api` must keep serving unrelated routes (`/`, `/health`,
//! `/api/search`, `/api/research`) even with no durable configuration
//! (`SCORPION_DOMAIN_DB`/`RESEARCH_EVIDENCE_DB`) present at all.

use spider::features::domain_runtime::{open_shared_domain_store, DomainRuntimeError};
use spider::features::identity::EvidenceId;
use spider::utils::evidence::{EvidenceBundle, EvidenceLedgerError, EvidenceRef};

/// Errors this application boundary exposes for evidence inspection.
/// Secrets and filesystem paths are never carried in these messages.
#[derive(Debug, PartialEq, Eq)]
pub enum EvidenceError {
    /// The supplied string is not a well-formed canonical `EvidenceId`.
    InvalidReference(String),
    /// No canonical domain database is configured for this process.
    NotConfigured,
    /// The canonical store is configured but could not be opened.
    Unavailable,
    /// The reference parsed, but nothing has ever been durably recorded
    /// for it.
    NotFound,
    /// The store opened, but reading/decoding the record failed.
    ReadFailed,
}

impl std::fmt::Display for EvidenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidReference(message) => write!(f, "invalid evidence reference: {message}"),
            Self::NotConfigured => f.write_str("evidence store is not configured"),
            Self::Unavailable => f.write_str("evidence store is unavailable"),
            Self::NotFound => {
                f.write_str("no evidence has been durably recorded for this reference")
            }
            Self::ReadFailed => {
                f.write_str("failed to read the requested evidence from the canonical store")
            }
        }
    }
}

/// HTTP status for a public evidence-inspection error.
///
/// `400` — malformed reference (client error, never reached the store).
/// `404` — a well-formed reference that names no durable record.
/// `503` — the canonical store is not configured, or configured but
/// unreachable (an execution/runtime dependency failure, matching
/// `ResearchError::Unavailable`'s own convention would suggest `502`, but
/// evidence inspection has no "provider" to distinguish from — both
/// configuration classes are a service-availability fact from this
/// boundary's point of view).
/// `500` — the store opened but the read/decode itself failed
/// unexpectedly.
pub fn evidence_error_status(error: &EvidenceError) -> u16 {
    match error {
        EvidenceError::InvalidReference(_) => 400,
        EvidenceError::NotConfigured | EvidenceError::Unavailable => 503,
        EvidenceError::NotFound => 404,
        EvidenceError::ReadFailed => 500,
    }
}

/// Serialize a public evidence-inspection error without leaking
/// filesystem paths or persistence diagnostics.
pub fn evidence_error_json(error: &EvidenceError) -> String {
    let code = match error {
        EvidenceError::InvalidReference(_) => "invalid_evidence_reference",
        EvidenceError::NotConfigured => "evidence_store_not_configured",
        EvidenceError::Unavailable => "evidence_store_unavailable",
        EvidenceError::NotFound => "evidence_not_found",
        EvidenceError::ReadFailed => "evidence_read_failed",
    };
    serde_json::json!({"error": {"code": code, "message": error.to_string()}}).to_string()
}

/// Resolve one durable [`EvidenceBundle`] by its canonical `EvidenceRef`
/// string, using real process environment for store configuration.
pub async fn evidence(raw_ref: &str) -> Result<EvidenceBundle, EvidenceError> {
    evidence_with_environment(raw_ref, &|name| std::env::var(name).ok()).await
}

/// Real implementation, parameterized over environment lookup — mirrors
/// `spider_mcp::tools::evidence_read`'s own `run`/`run_with_environment`
/// split, so tests can deterministically exercise every
/// configured/unconfigured/misconfigured store shape without mutating
/// real process environment.
async fn evidence_with_environment(
    raw_ref: &str,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<EvidenceBundle, EvidenceError> {
    let raw = raw_ref.trim();
    if raw.is_empty() {
        return Err(EvidenceError::InvalidReference(
            "evidence reference must not be empty".into(),
        ));
    }
    let id: EvidenceId = raw.parse().map_err(|error| {
        EvidenceError::InvalidReference(format!(
            "evidence reference is not a valid canonical EvidenceId: {error}"
        ))
    })?;

    let store = open_shared_domain_store(None, lookup)
        .await
        .map_err(|error| match error {
            DomainRuntimeError::NotConfigured(_) => EvidenceError::NotConfigured,
            DomainRuntimeError::Persistence(internal) => {
                eprintln!("scorpion-api evidence read: domain store open failed: {internal}");
                EvidenceError::Unavailable
            }
        })?;

    EvidenceRef::new(id)
        .resolve(&store)
        .await
        .map_err(|error: EvidenceLedgerError| {
            eprintln!("scorpion-api evidence read: {error}");
            EvidenceError::ReadFailed
        })?
        .ok_or(EvidenceError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use spider::features::domain_persistence::DomainPersistence;
    use spider::utils::evidence::record_evidence;
    use std::collections::BTreeMap;

    fn store_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "scorpion-app-evidence-seam-test-{}-{}.sqlite3",
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

    #[tokio::test]
    async fn evidence_read_returns_every_persisted_semantic_field_exactly_as_stored() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);

        let writer = DomainPersistence::open(&path).await.unwrap();
        let recorded = record_evidence(&writer, representative_bundle())
            .await
            .unwrap();
        let id = recorded.id.unwrap();
        drop(writer); // producer handle closed

        let read_back = evidence_with_environment(&id.to_string(), &lookup)
            .await
            .unwrap();

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

    #[tokio::test]
    async fn repeated_reads_leave_history_unchanged() {
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

        let first = evidence_with_environment(&id.to_string(), &lookup)
            .await
            .unwrap();
        let second = evidence_with_environment(&id.to_string(), &lookup)
            .await
            .unwrap();
        assert_eq!(
            first.content, second.content,
            "repeated reads must return identical output"
        );

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

    #[tokio::test]
    async fn empty_evidence_ref_is_invalid_reference() {
        let path = store_path();
        let lookup = configured_lookup(&path);
        let error = evidence_with_environment("", &lookup).await.unwrap_err();
        assert!(matches!(error, EvidenceError::InvalidReference(_)));
        assert_eq!(evidence_error_status(&error), 400);
    }

    #[tokio::test]
    async fn whitespace_only_evidence_ref_is_invalid_reference() {
        let path = store_path();
        let lookup = configured_lookup(&path);
        let error = evidence_with_environment("   ", &lookup).await.unwrap_err();
        assert!(matches!(error, EvidenceError::InvalidReference(_)));
    }

    #[tokio::test]
    async fn malformed_reference_is_invalid_reference() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        for malformed in [
            "not-an-evidence-ref",
            "watch_0123456789abcdef0123456789abcdef",
            "evid_tooShort",
            "https://example.test/",
        ] {
            let error = evidence_with_environment(malformed, &lookup)
                .await
                .unwrap_err();
            assert!(
                matches!(error, EvidenceError::InvalidReference(_)),
                "{malformed:?} must be rejected as InvalidReference"
            );
            assert_eq!(evidence_error_status(&error), 400);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn valid_but_absent_evidence_id_is_not_found() {
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let never_recorded = EvidenceId::new();
        let error = evidence_with_environment(&never_recorded.to_string(), &lookup)
            .await
            .unwrap_err();
        assert_eq!(error, EvidenceError::NotFound);
        assert_eq!(evidence_error_status(&error), 404);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn missing_store_configuration_is_not_configured() {
        let error =
            evidence_with_environment(&EvidenceId::new().to_string(), &unconfigured_lookup())
                .await
                .unwrap_err();
        assert_eq!(error, EvidenceError::NotConfigured);
        assert_eq!(evidence_error_status(&error), 503);
        assert!(evidence_error_json(&error).contains("evidence_store_not_configured"));
    }

    #[tokio::test]
    async fn unopenable_store_fails_safely_without_leaking_the_path() {
        let dir = std::env::temp_dir().join(format!(
            "scorpion-app-evidence-seam-unopenable-{}-{}",
            std::process::id(),
            EvidenceId::new()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let lookup = configured_lookup(&dir);

        let error = evidence_with_environment(&EvidenceId::new().to_string(), &lookup)
            .await
            .unwrap_err();
        assert_eq!(error, EvidenceError::Unavailable);
        let json = evidence_error_json(&error);
        let path_string = dir.to_string_lossy().to_string();
        assert!(!json.contains(&path_string));

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

        let bundle = evidence_with_environment(&id.to_string(), &lookup)
            .await
            .unwrap();
        let output = serde_json::to_string(&bundle).unwrap();

        for forbidden in [
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

    #[test]
    fn error_status_and_json_codes_are_distinct_and_deterministic() {
        assert_eq!(
            evidence_error_status(&EvidenceError::InvalidReference("x".into())),
            400
        );
        assert_eq!(evidence_error_status(&EvidenceError::NotConfigured), 503);
        assert_eq!(evidence_error_status(&EvidenceError::Unavailable), 503);
        assert_eq!(evidence_error_status(&EvidenceError::NotFound), 404);
        assert_eq!(evidence_error_status(&EvidenceError::ReadFailed), 500);

        assert!(
            evidence_error_json(&EvidenceError::InvalidReference("x".into()))
                .contains("\"invalid_evidence_reference\"")
        );
        assert!(evidence_error_json(&EvidenceError::NotConfigured)
            .contains("\"evidence_store_not_configured\""));
        assert!(evidence_error_json(&EvidenceError::Unavailable)
            .contains("\"evidence_store_unavailable\""));
        assert!(evidence_error_json(&EvidenceError::NotFound).contains("\"evidence_not_found\""));
        assert!(
            evidence_error_json(&EvidenceError::ReadFailed).contains("\"evidence_read_failed\"")
        );
    }
}
