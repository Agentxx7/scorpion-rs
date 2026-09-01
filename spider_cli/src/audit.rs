//! `scorpion audit <url>` — Scorpion's canonical deterministic page audit,
//! run from the CLI.
//!
//! Calls the identical `spider::features::audit::audit_page` seam
//! `spider_mcp`'s `spider_audit_page` MCP tool and `scorpion_app`'s `POST
//! /api/audit` Web Console route call — no rule evaluation,
//! technology-marker extraction, or evidence construction is duplicated
//! here, only the JSON presentation shape is CLI-local, kept field-for-
//! field identical to those two peer adapters rather than inventing a
//! separate normalized contract. MCP, Web Console, and CLI are three
//! peer adapters over the one audit engine — none of them call each
//! other; each opens the same shared domain store independently through
//! `open_shared_domain_store`.
//!
//! Mirrors `spider_cli::discovery`'s own `run_*` convention: this
//! function returns the JSON string to print, it never writes to stdout
//! itself, so the shaping logic is directly testable without capturing
//! process output.

use serde::Serialize;
use spider::features::audit::{audit_page, AuditError, Finding, ObservedTechnologyMarker};
use spider::features::domain_runtime::{open_shared_domain_store, DomainRuntimeError};
use spider::features::identity::EvidenceId;
use std::path::PathBuf;

/// Thin, presentation-only wire projection of one canonical
/// `PageAuditResult`. Reuses `Finding`'s and `ObservedTechnologyMarker`'s
/// own `Serialize` implementations verbatim — no field is re-derived,
/// renamed, reordered, or reinterpreted; `evidence_ref` is the exact
/// `EvidenceId` `PageAuditResult::evidence_ref()` already carries, never
/// a minted, hashed, or translated identity. Field-for-field identical
/// to `spider_mcp::tools::audit::AuditPageResponse` and
/// `scorpion_app::audit::AuditResponse`.
#[derive(Serialize)]
struct AuditOutput {
    evidence_ref: EvidenceId,
    findings: Vec<Finding>,
    technology_markers: Vec<ObservedTechnologyMarker>,
}

/// Run one canonical deterministic page audit and print the resulting
/// JSON, using real process environment for store configuration.
pub async fn run_audit(url: &str, database: Option<PathBuf>) -> Result<String, String> {
    run_audit_with_environment(url, database, &|name| std::env::var(name).ok()).await
}

/// Real implementation, parameterized over environment lookup — mirrors
/// `spider_mcp::tools::audit`'s and `scorpion_app::audit`'s own
/// `run`/`run_with_environment` split, so tests can deterministically
/// exercise every configured/unconfigured/misconfigured store shape
/// without mutating real process environment.
async fn run_audit_with_environment(
    url: &str,
    database: Option<PathBuf>,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<String, String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("url must not be empty".to_string());
    }

    let store = open_shared_domain_store(database, lookup)
        .await
        .map_err(map_store_error)?;

    let result = audit_page(&store, trimmed)
        .await
        .map_err(|error| map_audit_error(&error))?;

    let output = AuditOutput {
        evidence_ref: result.evidence_ref().id(),
        findings: result.findings().to_vec(),
        technology_markers: result.technology_markers().to_vec(),
    };
    serde_json::to_string_pretty(&output).map_err(|error| error.to_string())
}

/// Project a [`DomainRuntimeError`] into a sanitized CLI error message.
/// `NotConfigured`'s own message is safe to surface verbatim — it names
/// only the two environment variable names, never a resolved path.
/// `Persistence` wraps a `PersistenceError` whose `Display` can include
/// the underlying SQLite/filesystem diagnostic — logged for the operator,
/// never printed to the caller.
fn map_store_error(error: DomainRuntimeError) -> String {
    match error {
        DomainRuntimeError::NotConfigured(message) => message,
        DomainRuntimeError::Persistence(internal) => {
            eprintln!("scorpion audit: domain store open failed: {internal}");
            "the canonical audit persistence store is unavailable".to_string()
        }
    }
}

/// Project an [`AuditError`] into a sanitized CLI error message. Every
/// variant that could carry an internal diagnostic (a transport/
/// acquisition detail, a SQLite/persistence detail, a serialization
/// detail) is logged for the operator and replaced with a fixed, stable
/// public message — never the raw `Display`. Mirrors
/// `spider_mcp::tools::audit::map_audit_error` and
/// `scorpion_app::audit::map_canonical_audit_error` exactly.
fn map_audit_error(error: &AuditError) -> String {
    match error {
        AuditError::Acquisition(internal) => {
            eprintln!("scorpion audit: acquisition failed: {internal}");
            "failed to acquire the target page".to_string()
        }
        AuditError::EvidenceRecording(internal) => {
            eprintln!("scorpion audit: evidence recording failed: {internal}");
            "failed to record durable evidence for the audited page".to_string()
        }
        AuditError::Evidence(internal) => {
            eprintln!("scorpion audit: evidence resolution failed: {internal}");
            "failed to resolve durable evidence for a finding".to_string()
        }
        AuditError::Persistence(internal) => {
            eprintln!("scorpion audit: finding persistence failed: {internal}");
            "failed to persist a deterministic audit finding".to_string()
        }
        AuditError::EmptyEvidence | AuditError::EvidenceUnresolvable(_) => {
            eprintln!("scorpion audit: internal audit invariant violation: {error:?}");
            "the audit engine returned an unexpected internal state".to_string()
        }
        AuditError::Serialization(internal) => {
            eprintln!("scorpion audit: finding serialization failed: {internal}");
            "failed to serialize an audit finding".to_string()
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

    fn store_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "scorpion-cli-audit-test-{}-{}-{name}.sqlite3",
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

    const MINIMAL_HTML: &str = "<html><head><title>t</title></head><body><h1>hi</h1></body></html>";

    #[tokio::test]
    async fn valid_audit_returns_findings_ref_and_one_acquisition() {
        let path = store_path("valid");
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let fixture = AuditFixture::start("200 OK", "text/html", MINIMAL_HTML);

        let json = run_audit_with_environment(&fixture.url(), None, &lookup)
            .await
            .unwrap();
        assert!(json.contains("\"evidence_ref\": \"evid_"));
        assert!(json.contains("\"findings\": ["));
        assert!(json.contains("\"technology_markers\": ["));
        assert_eq!(fixture.hit_count(), 1, "exactly one target acquisition");

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn empty_url_is_a_deterministic_error() {
        let path = store_path("empty-url");
        let lookup = configured_lookup(&path);
        let error = run_audit_with_environment("", None, &lookup)
            .await
            .unwrap_err();
        assert!(error.contains("must not be empty"));
    }

    #[tokio::test]
    async fn whitespace_url_is_a_deterministic_error() {
        let path = store_path("ws-url");
        let lookup = configured_lookup(&path);
        let error = run_audit_with_environment("   ", None, &lookup)
            .await
            .unwrap_err();
        assert!(error.contains("must not be empty"));
    }

    #[tokio::test]
    async fn malformed_url_fails_deterministically() {
        let path = store_path("malformed-url");
        let lookup = configured_lookup(&path);
        let error = run_audit_with_environment("not a url", None, &lookup)
            .await
            .unwrap_err();
        assert!(error.contains("failed to acquire the target page"));
    }

    #[tokio::test]
    async fn missing_store_configuration_fails_closed() {
        let error =
            run_audit_with_environment("http://example.test/", None, &unconfigured_lookup())
                .await
                .unwrap_err();
        assert!(error.contains(spider::features::domain_runtime::DOMAIN_DATABASE_ENV));
    }

    #[tokio::test]
    async fn unopenable_store_fails_safely_without_leaking_the_path() {
        let dir = std::env::temp_dir().join(format!(
            "scorpion-cli-audit-unopenable-{}-{}",
            std::process::id(),
            EvidenceId::new()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let lookup = configured_lookup(&dir);

        let error = run_audit_with_environment("http://example.test/", None, &lookup)
            .await
            .unwrap_err();
        assert_eq!(
            error,
            "the canonical audit persistence store is unavailable"
        );
        assert!(!error.contains(&dir.to_string_lossy().to_string()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn explicit_database_argument_takes_priority_over_environment() {
        let explicit_path = store_path("explicit");
        let _ = std::fs::remove_file(&explicit_path);
        let env_path = store_path("env-ignored");
        let lookup = configured_lookup(&env_path);
        let fixture = AuditFixture::start("200 OK", "text/html", MINIMAL_HTML);

        let json = run_audit_with_environment(&fixture.url(), Some(explicit_path.clone()), &lookup)
            .await
            .unwrap();
        assert!(json.contains("\"evidence_ref\""));
        assert!(
            explicit_path.exists(),
            "the explicit database path must be the one actually used"
        );
        assert!(
            !env_path.exists(),
            "the env-configured path must never be touched"
        );

        let _ = std::fs::remove_file(&explicit_path);
    }

    #[tokio::test]
    async fn target_content_containing_script_like_text_remains_inert_data() {
        let path = store_path("hostile-content");
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let fixture = AuditFixture::start(
            "200 OK",
            "text/html",
            "<html><head><title>t</title></head><body><script>alert(1)</script></body></html>",
        );

        // The audit must complete deterministically; the hostile content
        // is only ever handled as data for rule evaluation (via
        // audit_page's own HTML fact extraction), never executed, shelled
        // out, or otherwise interpreted as a command by this CLI adapter.
        let json = run_audit_with_environment(&fixture.url(), None, &lookup)
            .await
            .unwrap();
        assert!(json.contains("\"evidence_ref\""));

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn set_cookie_never_leaks_into_output() {
        let path = store_path("set-cookie");
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0_u8; 4096];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nSet-Cookie: session=SUPER_SECRET_SENTINEL; Secure; HttpOnly\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{MINIMAL_HTML}",
                    MINIMAL_HTML.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let json = run_audit_with_environment(&format!("http://{addr}/"), None, &lookup)
            .await
            .unwrap();
        assert!(!json.to_lowercase().contains("set-cookie"));
        assert!(!json.contains("SUPER_SECRET_SENTINEL"));

        let _ = std::fs::remove_file(&path);
    }

    /// Phase 5E: an unreachable target (nothing listening on the port) is
    /// a distinct failure class from a syntactically malformed URL —
    /// both are genuinely distinct failure classes, but their *observable
    /// behavior* differs: a malformed URL fails before any request is
    /// ever attempted (a hard `Err`), while an unreachable target reaches
    /// the wire and gets a real (connection-refused) network response —
    /// canonical acquisition's own established contract
    /// (`fetch_single_page`, `spider/src/utils/evidence.rs`) is that a
    /// network-level failure once a request is actually attempted is
    /// never a hard `Err`, it is `Ok` with a page carrying no observed
    /// HTTP status. This CLI adapter does not, and must not, invent its
    /// own different behavior — it truthfully reflects the exact same
    /// contract `spider_audit_page` (MCP) and `POST /api/audit` (Web
    /// Console) already do, proven here by resolving the produced
    /// evidence and confirming no observed status was fabricated.
    #[tokio::test]
    async fn unreachable_target_completes_with_no_fabricated_observed_status() {
        let path = store_path("unreachable");
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        // Port 1 is a real, well-formed target that nothing listens on.
        let json = run_audit_with_environment("http://127.0.0.1:1/", None, &lookup)
            .await
            .expect("a request that reached the wire and was refused is Ok, not Err");
        let output: serde_json::Value = serde_json::from_str(&json).unwrap();
        let evidence_ref: EvidenceId = output["evidence_ref"].as_str().unwrap().parse().unwrap();

        let store = spider::features::domain_persistence::DomainPersistence::open(&path)
            .await
            .unwrap();
        let bundle = spider::utils::evidence::EvidenceRef::new(evidence_ref)
            .resolve(&store)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            bundle.observed_status_code, None,
            "a genuinely refused connection must never fabricate an observed HTTP status"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// Phase 5A/B (canonical engine agreement + Finding order): calling
    /// this CLI adapter and calling `audit_page` directly against the
    /// same deterministic fixture must produce the exact same Finding
    /// sequence (rule_id/version/category/severity/target/observed/
    /// expected conditions, in order) and the exact same technology
    /// markers (source/value, in order) — the two are genuinely separate
    /// acquisitions, so only the `EvidenceId`s are expected to differ.
    /// No sorting or normalization is applied to either side merely to
    /// make this comparison pass.
    #[tokio::test]
    async fn cli_output_matches_direct_audit_page_call_semantically() {
        let path = store_path("engine-agreement");
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let fixture = AuditFixture::start("200 OK", "text/html", MINIMAL_HTML);

        // Direct canonical engine call.
        let store = spider::features::domain_persistence::DomainPersistence::open(&path)
            .await
            .unwrap();
        let direct = audit_page(&store, &fixture.url()).await.unwrap();

        // CLI adapter call, against the identical target/response shape
        // (a second, independent acquisition).
        let cli_json = run_audit_with_environment(&fixture.url(), None, &lookup)
            .await
            .unwrap();
        let cli_output: serde_json::Value = serde_json::from_str(&cli_json).unwrap();

        assert_ne!(
            cli_output["evidence_ref"].as_str().unwrap(),
            direct.evidence_ref().id().to_string(),
            "independent acquisitions must produce distinct evidence identities"
        );

        let direct_findings = direct.findings();
        let cli_findings = cli_output["findings"].as_array().unwrap();
        assert_eq!(direct_findings.len(), cli_findings.len());
        assert!(
            !direct_findings.is_empty(),
            "the fixture must produce real findings"
        );
        for (direct_finding, cli_finding) in direct_findings.iter().zip(cli_findings.iter()) {
            let direct_json = serde_json::to_value(direct_finding).unwrap();
            for field in [
                "rule_id",
                "rule_version",
                "category",
                "severity",
                "target",
                "observed_condition",
                "expected_condition",
            ] {
                assert_eq!(
                    direct_json.get(field),
                    cli_finding.get(field),
                    "Finding field {field:?} diverged between direct audit_page() and \
                     the CLI adapter: direct={:?} cli={:?}",
                    direct_json.get(field),
                    cli_finding.get(field)
                );
            }
        }

        let direct_markers = direct.technology_markers();
        let cli_markers = cli_output["technology_markers"].as_array().unwrap();
        assert_eq!(direct_markers.len(), cli_markers.len());
        for (direct_marker, cli_marker) in direct_markers.iter().zip(cli_markers.iter()) {
            let direct_marker_json = serde_json::to_value(direct_marker).unwrap();
            assert_eq!(direct_marker_json["source"], cli_marker["source"]);
            assert_eq!(direct_marker_json["value"], cli_marker["value"]);
        }

        let _ = std::fs::remove_file(&path);
    }

    /// Phase 5C: the evidence identity the CLI adapter exposes resolves,
    /// through the existing canonical evidence-retrieval seam
    /// (`EvidenceRef::resolve`, exactly what `spider_evidence_read`/`GET
    /// /api/evidence/{ref}` also call — this test file is `#[cfg(test)]`
    /// code, exempt from the production-only allowed-consumer boundary
    /// exactly like every other module's own test-side proof of this
    /// kind), to the corresponding canonical `EvidenceBundle` — the same
    /// bundle a human/AI would see through the Evidence Inspector or MCP.
    #[tokio::test]
    async fn cli_evidence_identity_resolves_to_the_corresponding_evidence_bundle() {
        let path = store_path("evidence-identity");
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let fixture = AuditFixture::start("200 OK", "text/html", MINIMAL_HTML);

        let json = run_audit_with_environment(&fixture.url(), None, &lookup)
            .await
            .unwrap();
        let output: serde_json::Value = serde_json::from_str(&json).unwrap();
        let evidence_ref: EvidenceId = output["evidence_ref"]
            .as_str()
            .unwrap()
            .parse()
            .expect("CLI must expose a valid canonical EvidenceId");

        let store = spider::features::domain_persistence::DomainPersistence::open(&path)
            .await
            .unwrap();
        let bundle = spider::utils::evidence::EvidenceRef::new(evidence_ref)
            .resolve(&store)
            .await
            .unwrap()
            .expect("the CLI-produced evidence identity must resolve through the canonical ledger");
        assert_eq!(bundle.id, Some(evidence_ref));
        assert_eq!(
            bundle.requested_url.as_deref(),
            Some(fixture.url().as_str())
        );
        assert!(bundle.content.unwrap_or_default().contains("hi"));

        let _ = std::fs::remove_file(&path);
    }
}
