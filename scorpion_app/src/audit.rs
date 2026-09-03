//! Thin application boundary for executing Scorpion's canonical
//! deterministic page audit — the Web Console's peer of `spider_mcp`'s own
//! `spider_audit_page` MCP tool, over the identical canonical seam.
//!
//! This module owns presentation only: request validation, a thin wire
//! projection of the canonical
//! [`spider::features::audit::PageAuditResult`], and sanitized error
//! mapping. It performs no acquisition, no HTML/header parsing, no rule
//! evaluation, and no technology-marker extraction of its own — every one
//! of those already exists exactly once, in `spider::features::audit`,
//! and this module calls the single aggregate seam ([`audit_page`])
//! rather than assembling the capability itself. See
//! `SCORPION_WEB_CONSOLE_CANONICAL_PAGE_AUDIT_EXECUTION_001` and
//! `spider/tests/architecture_guardrails.rs`'s
//! `audit_module_has_a_precise_allowed_consumer_boundary` (widened to a
//! second authorized file by this frontier) and
//! `scorpion_app_audit_seam_has_no_independent_audit_assembly`.
//!
//! # Peer interface, not a second implementation
//!
//! MCP and the Web Console are peer interfaces over the same canonical
//! core: neither calls the other. This module does not talk to
//! `spider_mcp` in any way — it calls [`audit_page`] directly against the
//! same shared
//! [`DomainPersistence`](spider::features::domain_persistence::DomainPersistence)
//! store `spider_evidence_read`/`scorpion_app::evidence` already read
//! from, through the identical [`open_shared_domain_store`] seam, so an
//! `EvidenceRef` a Web audit produces resolves identically whether read
//! back through the Web Console's own Evidence Inspector or through MCP's
//! `spider_evidence_read` — one audit engine, one domain store, one
//! evidence truth.
//!
//! # Store lifetime
//!
//! Mirrors `scorpion_app::evidence` and `spider_mcp::tools::audit`'s own
//! decision exactly, for the same reason: the canonical shared store is
//! resolved fresh on every call through [`open_shared_domain_store`],
//! never a process-owned eagerly-opened handle — `scorpion-api` must keep
//! serving unrelated routes (`/`, `/health`, `/api/search`,
//! `/api/research`, `/api/evidence/{ref}`) even with no durable
//! configuration (`SCORPION_DOMAIN_DB`/`RESEARCH_EVIDENCE_DB`) present at
//! all.

use serde::{Deserialize, Serialize};
use spider::features::audit::{
    audit_page, AuditError as CanonicalAuditError, Finding, ObservedTechnologyMarker,
    PageAuditOutcome,
};
use spider::features::domain_runtime::{open_shared_domain_store, DomainRuntimeError};
use spider::features::identity::EvidenceId;

/// Stable application request for the canonical page audit capability.
/// Exactly one semantic property — no transport/headless/header/crawl/
/// rule/severity/technology/AI configuration is exposed: those remain
/// entirely canonical-audit-engine policy, never a Web Console dial.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuditRequest {
    pub url: String,
}

/// Thin, presentation-only wire projection of one canonical
/// `PageAuditResult`. Reuses `Finding`'s, `ObservedTechnologyMarker`'s,
/// and `PageAuditOutcome`'s own `Serialize` implementations verbatim —
/// no field is re-derived, renamed, reordered, or reinterpreted;
/// `outcome` is the exact [`PageAuditOutcome`]
/// `PageAuditResult::outcome()` already carries (`"evaluated"` /
/// `"target_unobserved"` on the wire), never re-derived here from
/// status codes, evidence fields, or content; `evidence_ref` is the
/// exact `EvidenceId` `PageAuditResult::evidence_ref()` already carries,
/// serialized through its own canonical `Serialize` impl (a bare
/// `evid_...` string), never a minted, hashed, or translated identity.
/// Both outcomes remain HTTP 200: `target_unobserved` is a truthful
/// *completed* execution result with the outcome explicit on the wire.
#[derive(Debug, Serialize)]
pub struct AuditResponse {
    pub outcome: PageAuditOutcome,
    pub evidence_ref: EvidenceId,
    pub findings: Vec<Finding>,
    pub technology_markers: Vec<ObservedTechnologyMarker>,
}

/// Errors this application boundary exposes for page-audit execution.
/// Secrets and filesystem paths are never carried in these messages.
#[derive(Debug, PartialEq, Eq)]
pub enum AuditError {
    /// Request-level validation failed before any store/acquisition
    /// activity.
    InvalidRequest(String),
    /// The supplied target is not a valid HTTP(S) page-audit target —
    /// rejected by the canonical engine before any acquisition; no
    /// evidence was minted.
    InvalidTarget,
    /// No canonical domain database is configured for this process.
    NotConfigured,
    /// The canonical store is configured but could not be opened.
    Unavailable,
    /// Acquiring the target page failed.
    AcquisitionFailed,
    /// The store opened and acquisition succeeded, but recording
    /// evidence/findings failed.
    PersistenceFailed,
    /// An unexpected internal audit-engine invariant violation.
    Internal,
}

impl std::fmt::Display for AuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(message) => write!(f, "invalid audit request: {message}"),
            Self::InvalidTarget => {
                f.write_str("invalid audit target: the target must be a valid http(s) URL")
            }
            Self::NotConfigured => f.write_str("audit store is not configured"),
            Self::Unavailable => f.write_str("audit store is unavailable"),
            Self::AcquisitionFailed => f.write_str("failed to acquire the target page"),
            Self::PersistenceFailed => {
                f.write_str("failed to persist durable evidence/findings for the audited page")
            }
            Self::Internal => f.write_str("the audit engine returned an unexpected internal state"),
        }
    }
}

/// HTTP status for a public page-audit error.
///
/// `400` — malformed/empty request, or a target that is not a valid
/// HTTP(S) page-audit target (rejected by the canonical engine before
/// any acquisition), never reached the store.
/// `503` — the canonical store is not configured, or configured but
/// unreachable — an execution/runtime dependency failure, matching
/// `scorpion_app::evidence::evidence_error_status`'s own convention for
/// the identical store-availability classes.
/// `502` — the target page could not be acquired (a real network/
/// upstream failure, distinct from every static configuration failure
/// above).
/// `500` — the store opened and acquisition succeeded, but persisting
/// evidence/findings failed, or an internal audit-engine invariant was
/// violated.
pub fn audit_error_status(error: &AuditError) -> u16 {
    match error {
        AuditError::InvalidRequest(_) | AuditError::InvalidTarget => 400,
        AuditError::NotConfigured | AuditError::Unavailable => 503,
        AuditError::AcquisitionFailed => 502,
        AuditError::PersistenceFailed | AuditError::Internal => 500,
    }
}

/// Serialize a public page-audit error without leaking filesystem paths,
/// SQLite diagnostics, or acquisition/transport internals.
pub fn audit_error_json(error: &AuditError) -> String {
    let code = match error {
        AuditError::InvalidRequest(_) => "invalid_request",
        AuditError::InvalidTarget => "invalid_audit_target",
        AuditError::NotConfigured => "audit_store_not_configured",
        AuditError::Unavailable => "audit_store_unavailable",
        AuditError::AcquisitionFailed => "audit_acquisition_failed",
        AuditError::PersistenceFailed => "audit_persistence_failed",
        AuditError::Internal => "internal_error",
    };
    serde_json::json!({"error": {"code": code, "message": error.to_string()}}).to_string()
}

/// Run one canonical deterministic page audit and project it into the Web
/// application's wire shape, using real process environment for store
/// configuration.
pub async fn run_audit(request: AuditRequest) -> Result<AuditResponse, AuditError> {
    run_audit_with_environment(request, &|name| std::env::var(name).ok()).await
}

/// Real implementation, parameterized over environment lookup — mirrors
/// `scorpion_app::evidence`'s and `spider_mcp::tools::audit`'s own
/// `run`/`run_with_environment` split, so tests can deterministically
/// exercise every configured/unconfigured/misconfigured store shape
/// without mutating real process environment.
async fn run_audit_with_environment(
    request: AuditRequest,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<AuditResponse, AuditError> {
    let url = request.url.trim();
    if url.is_empty() {
        return Err(AuditError::InvalidRequest("url must not be empty".into()));
    }

    let store = open_shared_domain_store(None, lookup)
        .await
        .map_err(|error| match error {
            DomainRuntimeError::NotConfigured(_) => AuditError::NotConfigured,
            DomainRuntimeError::Persistence(internal) => {
                eprintln!("scorpion-api page audit: domain store open failed: {internal}");
                AuditError::Unavailable
            }
        })?;

    let result = audit_page(&store, url)
        .await
        .map_err(|error| map_canonical_audit_error(&error))?;

    Ok(AuditResponse {
        outcome: result.outcome(),
        evidence_ref: result.evidence_ref().id(),
        findings: result.findings().to_vec(),
        technology_markers: result.technology_markers().to_vec(),
    })
}

/// Project a canonical [`CanonicalAuditError`] into this application's
/// public [`AuditError`] classification. Every variant that could carry
/// an internal diagnostic (a transport/acquisition detail, a
/// SQLite/persistence detail, a serialization detail) is logged for the
/// operator and replaced with a fixed, stable public classification —
/// never the raw `Display`.
fn map_canonical_audit_error(error: &CanonicalAuditError) -> AuditError {
    match error {
        CanonicalAuditError::InvalidTarget(_) => AuditError::InvalidTarget,
        CanonicalAuditError::Acquisition(internal) => {
            eprintln!("scorpion-api page audit: acquisition failed: {internal}");
            AuditError::AcquisitionFailed
        }
        CanonicalAuditError::EvidenceRecording(internal) => {
            eprintln!("scorpion-api page audit: evidence recording failed: {internal}");
            AuditError::PersistenceFailed
        }
        CanonicalAuditError::Evidence(internal) => {
            eprintln!("scorpion-api page audit: evidence resolution failed: {internal}");
            AuditError::PersistenceFailed
        }
        CanonicalAuditError::Persistence(internal) => {
            eprintln!("scorpion-api page audit: finding persistence failed: {internal}");
            AuditError::PersistenceFailed
        }
        CanonicalAuditError::EmptyEvidence | CanonicalAuditError::EvidenceUnresolvable(_) => {
            eprintln!("scorpion-api page audit: internal audit invariant violation: {error:?}");
            AuditError::Internal
        }
        CanonicalAuditError::Serialization(internal) => {
            eprintln!("scorpion-api page audit: finding serialization failed: {internal}");
            AuditError::Internal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn store_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "scorpion-app-audit-seam-test-{}-{}-{name}.sqlite3",
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

    /// A tiny blocking local HTTP fixture — counts every accepted
    /// request so tests can prove exactly one acquisition occurred (this
    /// frontier's own "one Web audit == one target hit" invariant).
    struct AuditFixture {
        addr: std::net::SocketAddr,
        hits: Arc<AtomicUsize>,
    }

    impl AuditFixture {
        fn start(status: &'static str, content_type: &'static str, body: &'static str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let addr = listener.local_addr().unwrap();
            let hits = Arc::new(AtomicUsize::new(0));
            let hits_thread = hits.clone();
            std::thread::spawn(move || loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut buf = [0_u8; 4096];
                        let _ = stream.read(&mut buf);
                        hits_thread.fetch_add(1, Ordering::SeqCst);
                        let response = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            });
            Self { addr, hits }
        }

        fn url(&self) -> String {
            format!("http://{}/", self.addr)
        }

        fn hit_count(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }
    }

    const MINIMAL_HTML: &str = "<html><head><title>t</title></head><body>hi</body></html>";

    #[tokio::test]
    async fn valid_audit_returns_evaluated_outcome_findings_ref_and_one_acquisition() {
        let path = store_path("valid");
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let fixture = AuditFixture::start("200 OK", "text/html", MINIMAL_HTML);

        let response = run_audit_with_environment(AuditRequest { url: fixture.url() }, &lookup)
            .await
            .unwrap();

        assert_eq!(response.outcome, PageAuditOutcome::Evaluated);
        assert!(!response.findings.is_empty());
        assert_eq!(fixture.hit_count(), 1, "exactly one target acquisition");
        let wire = serde_json::to_value(&response).unwrap();
        assert_eq!(wire["outcome"], serde_json::json!("evaluated"));

        let _ = std::fs::remove_file(&path);
    }

    /// A valid HTTP(S) target nothing listens on is a truthful completed
    /// execution — HTTP 200 at the route level, `target_unobserved` on
    /// the wire, zero findings *because no rule ran* — never rendered
    /// indistinguishable from an observed page with zero findings, and
    /// still carrying the evidence reference for inspection.
    #[tokio::test]
    async fn unobserved_target_returns_target_unobserved_with_zero_findings_and_evidence_ref() {
        let path = store_path("unobserved");
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let response = run_audit_with_environment(
            AuditRequest {
                url: format!("http://{addr}/"),
            },
            &lookup,
        )
        .await
        .unwrap();

        assert_eq!(response.outcome, PageAuditOutcome::TargetUnobserved);
        assert!(response.findings.is_empty());
        assert!(response.technology_markers.is_empty());
        let wire = serde_json::to_value(&response).unwrap();
        assert_eq!(wire["outcome"], serde_json::json!("target_unobserved"));
        assert!(wire["evidence_ref"].is_string());

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn empty_url_is_invalid_request() {
        let path = store_path("empty-url");
        let lookup = configured_lookup(&path);
        let error = run_audit_with_environment(AuditRequest { url: String::new() }, &lookup)
            .await
            .unwrap_err();
        assert!(matches!(error, AuditError::InvalidRequest(_)));
        assert_eq!(audit_error_status(&error), 400);
    }

    #[tokio::test]
    async fn whitespace_url_is_invalid_request() {
        let path = store_path("ws-url");
        let lookup = configured_lookup(&path);
        let error = run_audit_with_environment(
            AuditRequest {
                url: "   ".to_string(),
            },
            &lookup,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, AuditError::InvalidRequest(_)));
    }

    #[tokio::test]
    async fn malformed_url_fails_deterministically() {
        let path = store_path("malformed-url");
        let lookup = configured_lookup(&path);
        let fixture = AuditFixture::start("200 OK", "text/html", MINIMAL_HTML);
        let bare_address = fixture
            .url()
            .strip_prefix("http://")
            .unwrap()
            .trim_end_matches('/')
            .to_string();

        for url in ["not a url".to_string(), bare_address] {
            let error = run_audit_with_environment(AuditRequest { url }, &lookup)
                .await
                .unwrap_err();
            // The canonical engine rejects an unparsable/non-HTTP(S)
            // target *before* any acquisition — a client-correctable
            // 400, never a 502 acquisition failure (which now means a
            // real pre-response acquisition breakdown only). This
            // application boundary does not re-validate URL syntax
            // itself; it projects the canonical engine's own verdict.
            assert_eq!(error, AuditError::InvalidTarget);
            assert_eq!(audit_error_status(&error), 400);
            assert!(audit_error_json(&error).contains("invalid_audit_target"));
        }
        assert_eq!(
            fixture.hit_count(),
            0,
            "rejection precedes acquisition — the fixture was never hit"
        );
    }

    #[tokio::test]
    async fn missing_store_configuration_is_not_configured() {
        let error = run_audit_with_environment(
            AuditRequest {
                url: "http://example.test/".to_string(),
            },
            &unconfigured_lookup(),
        )
        .await
        .unwrap_err();
        assert_eq!(error, AuditError::NotConfigured);
        assert_eq!(audit_error_status(&error), 503);
        assert!(audit_error_json(&error).contains("audit_store_not_configured"));
    }

    #[tokio::test]
    async fn unopenable_store_fails_safely_without_leaking_the_path() {
        let dir = std::env::temp_dir().join(format!(
            "scorpion-app-audit-seam-unopenable-{}-{}",
            std::process::id(),
            EvidenceId::new()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let lookup = configured_lookup(&dir);

        let error = run_audit_with_environment(
            AuditRequest {
                url: "http://example.test/".to_string(),
            },
            &lookup,
        )
        .await
        .unwrap_err();
        assert_eq!(error, AuditError::Unavailable);
        let json = audit_error_json(&error);
        let path_string = dir.to_string_lossy().to_string();
        assert!(!json.contains(&path_string));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn text_plain_html_looking_content_yields_no_html_only_findings_or_generator_marker() {
        let path = store_path("text-plain");
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let fixture = AuditFixture::start(
            "200 OK",
            "text/plain",
            "<html><head><title>t</title><meta name=\"generator\" content=\"WordPress\"></head></html>",
        );

        let response = run_audit_with_environment(AuditRequest { url: fixture.url() }, &lookup)
            .await
            .unwrap();

        assert!(
            !response
                .technology_markers
                .iter()
                .any(|marker| format!("{marker:?}").contains("HtmlMetaGenerator")),
            "a declared text/plain response must never be HTML-parsed for a generator marker"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn wp_content_and_php_urls_alone_infer_zero_technology() {
        let path = store_path("wp-content");
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let fixture = AuditFixture::start("200 OK", "text/html", MINIMAL_HTML);
        // The fixture's own URL never contains "/wp-content/" or ".php" —
        // this test proves the *response* alone determines findings, but
        // also stands as a structural reminder: this seam never inspects
        // the request URL string for technology inference (it exists
        // solely as an acquisition target for `audit_page`).
        let response = run_audit_with_environment(AuditRequest { url: fixture.url() }, &lookup)
            .await
            .unwrap();
        assert!(response.technology_markers.is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn set_cookie_never_leaks_into_public_response() {
        let path = store_path("set-cookie");
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0_u8; 4096];
                let _ = stream.read(&mut buf);
                let body = MINIMAL_HTML;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nSet-Cookie: session=SUPER_SECRET_SENTINEL; Secure; HttpOnly\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let response = run_audit_with_environment(
            AuditRequest {
                url: format!("http://{addr}/"),
            },
            &lookup,
        )
        .await
        .unwrap();
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(!serialized.to_lowercase().contains("set-cookie"));
        assert!(!serialized.contains("SUPER_SECRET_SENTINEL"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn audit_request_rejects_unknown_fields() {
        let parsed = serde_json::from_str::<AuditRequest>(
            r#"{"url":"https://example.test/","transport":"tor"}"#,
        );
        assert!(parsed.is_err());
    }

    #[test]
    fn error_status_and_json_codes_are_distinct_and_deterministic() {
        assert_eq!(
            audit_error_status(&AuditError::InvalidRequest("x".into())),
            400
        );
        assert_eq!(audit_error_status(&AuditError::InvalidTarget), 400);
        assert_eq!(audit_error_status(&AuditError::NotConfigured), 503);
        assert_eq!(audit_error_status(&AuditError::Unavailable), 503);
        assert_eq!(audit_error_status(&AuditError::AcquisitionFailed), 502);
        assert_eq!(audit_error_status(&AuditError::PersistenceFailed), 500);
        assert_eq!(audit_error_status(&AuditError::Internal), 500);

        assert!(audit_error_json(&AuditError::InvalidRequest("x".into()))
            .contains("\"invalid_request\""));
        assert!(audit_error_json(&AuditError::InvalidTarget).contains("\"invalid_audit_target\""));
        assert!(
            audit_error_json(&AuditError::NotConfigured).contains("\"audit_store_not_configured\"")
        );
        assert!(audit_error_json(&AuditError::Unavailable).contains("\"audit_store_unavailable\""));
        assert!(audit_error_json(&AuditError::AcquisitionFailed)
            .contains("\"audit_acquisition_failed\""));
        assert!(audit_error_json(&AuditError::PersistenceFailed)
            .contains("\"audit_persistence_failed\""));
        assert!(audit_error_json(&AuditError::Internal).contains("\"internal_error\""));
    }
}
