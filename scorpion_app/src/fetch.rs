//! Thin application boundary for executing Scorpion's canonical one-shot
//! Fetch capability — the Web Console's peer of the CLI's `scorpion fetch
//! <url>` command, over the identical canonical acquisition/evidence seam
//! (`SCORPION_CANONICAL_WEB_FETCH_SURFACE_001`).
//!
//! This module owns presentation only: request validation, durable
//! evidence recording through `fetch_single_page_with_options`,
//! `build_evidence`, and `record_evidence` — the same canonical
//! acquisition/evidence primitives, composed directly here rather than
//! through the audit engine, since Fetch has no rules or technology
//! markers to derive. This module calls no audit-module function at all
//! (proven by `scorpion_app_fetch_seam_has_no_independent_acquisition` and
//! the repository-wide audit-consumer-boundary check — this module is not
//! an authorized audit consumer and must never become one). It performs
//! no crawling (the crawl/scrape type is never constructed here), no
//! discovered-link traversal, no Chrome/headless acquisition, no CAPTCHA
//! handling, and no content transformation — one caller-supplied URL in,
//! one bounded, non-following HTTP(S) fetch, recorded as durable evidence
//! exactly once. Transport is always `TransportPolicy::Default`
//! (`AcquisitionOptions::default()`); this boundary exposes no
//! transport-selection field at all — Tor enablement for the Web Console
//! is explicitly not authorized by
//! `SCORPION_CANONICAL_WEB_FETCH_SURFACE_001`'s own scope.
//!
//! # Peer interface, not a second implementation
//!
//! Mirrors `scorpion_app::audit`'s own documented relationship to MCP: this
//! module does not talk to `spider_cli`'s own Fetch command in any way — it
//! calls the canonical acquisition/evidence primitives directly against the
//! same shared
//! [`DomainPersistence`](spider::features::domain_persistence::DomainPersistence)
//! store `scorpion_app::evidence`/`scorpion_app::audit` already read from
//! and write to, through the identical [`open_shared_domain_store`] seam,
//! so an `EvidenceRef` a Web fetch produces resolves identically whether
//! read back through the Web Console's own Evidence Inspector or through
//! MCP's `spider_evidence_read` — one acquisition primitive, one domain
//! store, one evidence truth.
//!
//! # Store lifetime
//!
//! Mirrors `scorpion_app::audit`/`scorpion_app::evidence`'s own decision
//! exactly, for the same reason: the canonical shared store is resolved
//! fresh on every call through [`open_shared_domain_store`], never a
//! process-owned eagerly-opened handle.

use serde::{Deserialize, Serialize};
use spider::features::domain_runtime::{open_shared_domain_store, DomainRuntimeError};
use spider::features::identity::EvidenceId;
use spider::utils::evidence::{
    build_evidence, fetch_single_page_with_options, record_evidence, AcquisitionOptions,
    EvidenceLedgerError,
};

/// Stable application request for the canonical one-shot Fetch capability.
/// Exactly one semantic property — no transport/headless/crawl/link-follow
/// configuration is exposed: transport is always `Default`; every other
/// dial remains entirely canonical-acquisition policy, never a Web Console
/// input.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FetchRequest {
    pub url: String,
}

/// Thin, presentation-only wire projection of the durable
/// [`spider::utils::evidence::EvidenceBundle`] this fetch recorded. Every
/// field is copied verbatim from that same bundle — never re-derived,
/// renamed, or reinterpreted. `evidence_ref` is the exact [`EvidenceId`]
/// `record_evidence` assigned, so the client can immediately resolve the
/// complete record (including captured content, response headers, and
/// downloads) through the existing Evidence Inspector / `/api/evidence/*`
/// routes without this response duplicating that presentation.
#[derive(Debug, Serialize)]
pub struct FetchResponse {
    pub evidence_ref: EvidenceId,
    pub requested_url: Option<String>,
    pub final_url: Option<String>,
    pub status_code: Option<u16>,
    pub observed_status_code: Option<u16>,
    pub content_type: Option<String>,
}

/// Errors this application boundary exposes for Fetch execution. Secrets
/// and filesystem paths are never carried in these messages.
#[derive(Debug, PartialEq, Eq)]
pub enum FetchError {
    /// Request-level validation failed before any store/acquisition
    /// activity.
    InvalidRequest(String),
    /// The supplied target is not a valid HTTP(S) fetch target — rejected
    /// before any acquisition; no evidence was minted.
    InvalidTarget,
    /// No canonical domain database is configured for this process.
    NotConfigured,
    /// The canonical store is configured but could not be opened.
    Unavailable,
    /// Acquiring the target page failed.
    AcquisitionFailed,
    /// The store opened and acquisition succeeded, but recording evidence
    /// failed.
    PersistenceFailed,
    /// An unexpected internal invariant violation (evidence serialization).
    Internal,
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(message) => write!(f, "invalid fetch request: {message}"),
            Self::InvalidTarget => {
                f.write_str("invalid fetch target: the target must be a valid http(s) URL")
            }
            Self::NotConfigured => f.write_str("fetch store is not configured"),
            Self::Unavailable => f.write_str("fetch store is unavailable"),
            Self::AcquisitionFailed => f.write_str("failed to acquire the target page"),
            Self::PersistenceFailed => {
                f.write_str("failed to persist durable evidence for the fetched page")
            }
            Self::Internal => f.write_str("the fetch engine returned an unexpected internal state"),
        }
    }
}

/// HTTP status for a public Fetch error.
///
/// `400` — malformed/empty request, or a target that is not a valid
/// HTTP(S) fetch target, never reached the store.
/// `503` — the canonical store is not configured, or configured but
/// unreachable — matching `scorpion_app::audit::audit_error_status`'s own
/// convention for the identical store-availability classes.
/// `502` — the target page could not be acquired.
/// `500` — the store opened and acquisition succeeded, but persisting
/// evidence failed, or an internal invariant was violated.
pub fn fetch_error_status(error: &FetchError) -> u16 {
    match error {
        FetchError::InvalidRequest(_) | FetchError::InvalidTarget => 400,
        FetchError::NotConfigured | FetchError::Unavailable => 503,
        FetchError::AcquisitionFailed => 502,
        FetchError::PersistenceFailed | FetchError::Internal => 500,
    }
}

/// Serialize a public Fetch error without leaking filesystem paths, SQLite
/// diagnostics, or acquisition/transport internals.
pub fn fetch_error_json(error: &FetchError) -> String {
    let code = match error {
        FetchError::InvalidRequest(_) => "invalid_request",
        FetchError::InvalidTarget => "invalid_fetch_target",
        FetchError::NotConfigured => "fetch_store_not_configured",
        FetchError::Unavailable => "fetch_store_unavailable",
        FetchError::AcquisitionFailed => "fetch_acquisition_failed",
        FetchError::PersistenceFailed => "fetch_persistence_failed",
        FetchError::Internal => "internal_error",
    };
    serde_json::json!({"error": {"code": code, "message": error.to_string()}}).to_string()
}

/// Reject a target that is not an absolute, parseable `http`/`https` URL
/// before any acquisition — the same coarse pre-classification the audit
/// engine's own target validator performs for the same reason
/// (distinguishing a client-correctable 400 from a real acquisition-layer
/// 502), not a second copy of
/// `spider::features::transport::validate_target`'s own SSRF/onion
/// screening, which `fetch_single_page_with_options` still performs in
/// full underneath this check regardless.
fn validate_fetch_target(url: &str) -> Result<(), FetchError> {
    let admitted = spider::url::Url::parse(url)
        .map(|parsed| matches!(parsed.scheme(), "http" | "https"))
        .unwrap_or(false);
    if admitted {
        Ok(())
    } else {
        Err(FetchError::InvalidTarget)
    }
}

/// Run one canonical one-shot Fetch and project it into the Web
/// application's wire shape, using real process environment for store
/// configuration.
pub async fn run_fetch(request: FetchRequest) -> Result<FetchResponse, FetchError> {
    run_fetch_with_environment(request, &|name| std::env::var(name).ok()).await
}

/// Real implementation, parameterized over environment lookup — mirrors
/// `scorpion_app::audit`/`scorpion_app::evidence`'s own
/// `run`/`run_with_environment` split, so tests can deterministically
/// exercise every configured/unconfigured/misconfigured store shape
/// without mutating real process environment.
async fn run_fetch_with_environment(
    request: FetchRequest,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<FetchResponse, FetchError> {
    let url = request.url.trim();
    if url.is_empty() {
        return Err(FetchError::InvalidRequest("url must not be empty".into()));
    }
    validate_fetch_target(url)?;

    let store = open_shared_domain_store(None, lookup)
        .await
        .map_err(|error| match error {
            DomainRuntimeError::NotConfigured(_) => FetchError::NotConfigured,
            DomainRuntimeError::Persistence(internal) => {
                eprintln!("scorpion-api fetch: domain store open failed: {internal}");
                FetchError::Unavailable
            }
        })?;

    let acquisition = fetch_single_page_with_options(url, AcquisitionOptions::default())
        .await
        .map_err(|internal| {
            eprintln!("scorpion-api fetch: acquisition failed: {internal}");
            FetchError::AcquisitionFailed
        })?;

    let page = acquisition.page();
    let content = page.get_html();
    let bundle = build_evidence(page, Some(content), false, false);
    let recorded = record_evidence(&store, bundle)
        .await
        .map_err(|error| match error {
            EvidenceLedgerError::Persistence(internal) => {
                eprintln!("scorpion-api fetch: evidence recording failed: {internal}");
                FetchError::PersistenceFailed
            }
            EvidenceLedgerError::Serialization(internal) => {
                eprintln!("scorpion-api fetch: evidence serialization failed: {internal}");
                FetchError::Internal
            }
        })?;

    let evidence_ref = recorded
        .id
        .expect("record_evidence always assigns an id on success");

    Ok(FetchResponse {
        evidence_ref,
        requested_url: recorded.requested_url,
        final_url: recorded.final_url,
        status_code: recorded.status_code,
        observed_status_code: recorded.observed_status_code,
        content_type: recorded.content_type,
    })
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
            "scorpion-app-fetch-seam-test-{}-{}-{name}.sqlite3",
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

    /// A tiny blocking local HTTP fixture — counts every accepted request
    /// so tests can prove exactly one acquisition occurred (this
    /// frontier's own "one Web fetch == one target hit, never a crawl"
    /// invariant).
    struct FetchFixture {
        addr: std::net::SocketAddr,
        hits: Arc<AtomicUsize>,
    }

    impl FetchFixture {
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
    async fn valid_fetch_returns_evidence_ref_and_exactly_one_acquisition() {
        let path = store_path("valid");
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let fixture = FetchFixture::start("200 OK", "text/html", MINIMAL_HTML);

        let response = run_fetch_with_environment(FetchRequest { url: fixture.url() }, &lookup)
            .await
            .unwrap();

        assert_eq!(
            response.requested_url.as_deref(),
            Some(fixture.url().as_str())
        );
        assert_eq!(response.status_code, Some(200));
        assert_eq!(response.observed_status_code, Some(200));
        assert_eq!(response.content_type.as_deref(), Some("text/html"));
        assert_eq!(fixture.hit_count(), 1, "exactly one target acquisition");

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn fetched_evidence_resolves_through_the_shared_store() {
        let path = store_path("resolve");
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let fixture = FetchFixture::start("200 OK", "text/html", MINIMAL_HTML);

        let response = run_fetch_with_environment(FetchRequest { url: fixture.url() }, &lookup)
            .await
            .unwrap();

        let store = spider::features::domain_persistence::DomainPersistence::open(&path)
            .await
            .unwrap();
        let resolved = spider::utils::evidence::read_evidence(&store, response.evidence_ref)
            .await
            .unwrap()
            .expect("evidence recorded by run_fetch must resolve through the shared store");
        assert_eq!(resolved.id, Some(response.evidence_ref));
        assert_eq!(resolved.content.as_deref(), Some(MINIMAL_HTML));

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn empty_url_is_invalid_request() {
        let path = store_path("empty-url");
        let lookup = configured_lookup(&path);
        let error = run_fetch_with_environment(FetchRequest { url: String::new() }, &lookup)
            .await
            .unwrap_err();
        assert!(matches!(error, FetchError::InvalidRequest(_)));
        assert_eq!(fetch_error_status(&error), 400);
    }

    #[tokio::test]
    async fn whitespace_url_is_invalid_request() {
        let path = store_path("ws-url");
        let lookup = configured_lookup(&path);
        let error = run_fetch_with_environment(
            FetchRequest {
                url: "   ".to_string(),
            },
            &lookup,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, FetchError::InvalidRequest(_)));
    }

    #[tokio::test]
    async fn malformed_or_non_http_url_is_invalid_target() {
        let path = store_path("malformed-url");
        let lookup = configured_lookup(&path);
        let fixture = FetchFixture::start("200 OK", "text/html", MINIMAL_HTML);
        let bare_address = fixture
            .url()
            .strip_prefix("http://")
            .unwrap()
            .trim_end_matches('/')
            .to_string();

        for url in [
            "not a url".to_string(),
            bare_address,
            "ftp://example.test/".to_string(),
            "javascript:alert(1)".to_string(),
        ] {
            let error = run_fetch_with_environment(FetchRequest { url }, &lookup)
                .await
                .unwrap_err();
            assert_eq!(error, FetchError::InvalidTarget);
            assert_eq!(fetch_error_status(&error), 400);
            assert!(fetch_error_json(&error).contains("invalid_fetch_target"));
        }
        assert_eq!(
            fixture.hit_count(),
            0,
            "rejection precedes acquisition — the fixture was never hit"
        );
    }

    #[tokio::test]
    async fn missing_store_configuration_is_not_configured() {
        let error = run_fetch_with_environment(
            FetchRequest {
                url: "http://example.test/".to_string(),
            },
            &unconfigured_lookup(),
        )
        .await
        .unwrap_err();
        assert_eq!(error, FetchError::NotConfigured);
        assert_eq!(fetch_error_status(&error), 503);
        assert!(fetch_error_json(&error).contains("fetch_store_not_configured"));
    }

    #[tokio::test]
    async fn unopenable_store_fails_safely_without_leaking_the_path() {
        let dir = std::env::temp_dir().join(format!(
            "scorpion-app-fetch-seam-unopenable-{}-{}",
            std::process::id(),
            EvidenceId::new()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let lookup = configured_lookup(&dir);

        let error = run_fetch_with_environment(
            FetchRequest {
                url: "http://example.test/".to_string(),
            },
            &lookup,
        )
        .await
        .unwrap_err();
        assert_eq!(error, FetchError::Unavailable);
        let json = fetch_error_json(&error);
        let path_string = dir.to_string_lossy().to_string();
        assert!(!json.contains(&path_string));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn unreachable_target_is_a_truthful_completed_fetch_with_no_observed_status() {
        let path = store_path("unreachable");
        let _ = std::fs::remove_file(&path);
        let lookup = configured_lookup(&path);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        // A valid HTTP(S) target nothing listens on is a truthful completed
        // fetch attempt, not an error — matching
        // `spider::features::audit::audit_page`'s own `TargetUnobserved`
        // philosophy (the same acquisition primitive underneath): a
        // reclassified effective `status_code` is recorded, but
        // `observed_status_code` truthfully stays `None` because no
        // response was ever actually observed. Evidence is still recorded
        // durably, exactly as `unobserved_target_returns_target_unobserved_
        // with_zero_findings_and_evidence_ref` already proves for Audit.
        let response = run_fetch_with_environment(
            FetchRequest {
                url: format!("http://{addr}/"),
            },
            &lookup,
        )
        .await
        .unwrap();
        assert!(response.observed_status_code.is_none());
        assert!(response.status_code.is_some());

        let _ = std::fs::remove_file(&path);
    }

    /// The one genuine `AcquisitionFailed` path reachable with this
    /// boundary's fixed `TransportPolicy::Default` (Tor is not exposed —
    /// see this module's own doc comment): an `.onion` target passes
    /// `validate_fetch_target`'s coarse http(s)-scheme pre-check (its
    /// scheme really is `http`) but is then genuinely rejected by
    /// `fetch_single_page_with_options`'s own canonical
    /// `transport::validate_target` (`OnionRequiresTor`) — proving this
    /// boundary does not silently swallow or reclassify that rejection as
    /// a 400, and does not duplicate the onion/SSRF check itself.
    #[tokio::test]
    async fn onion_target_under_fixed_default_transport_is_acquisition_failed() {
        let path = store_path("onion");
        let lookup = configured_lookup(&path);

        let error = run_fetch_with_environment(
            FetchRequest {
                url: "http://duskgytldkxiuqc6.onion/".to_string(),
            },
            &lookup,
        )
        .await
        .unwrap_err();
        assert_eq!(error, FetchError::AcquisitionFailed);
        assert_eq!(fetch_error_status(&error), 502);
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

        let response = run_fetch_with_environment(
            FetchRequest {
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
    fn fetch_request_rejects_unknown_fields() {
        let parsed = serde_json::from_str::<FetchRequest>(
            r#"{"url":"https://example.test/","transport":"tor"}"#,
        );
        assert!(parsed.is_err());
    }

    #[test]
    fn error_status_and_json_codes_are_distinct_and_deterministic() {
        assert_eq!(
            fetch_error_status(&FetchError::InvalidRequest("x".into())),
            400
        );
        assert_eq!(fetch_error_status(&FetchError::InvalidTarget), 400);
        assert_eq!(fetch_error_status(&FetchError::NotConfigured), 503);
        assert_eq!(fetch_error_status(&FetchError::Unavailable), 503);
        assert_eq!(fetch_error_status(&FetchError::AcquisitionFailed), 502);
        assert_eq!(fetch_error_status(&FetchError::PersistenceFailed), 500);
        assert_eq!(fetch_error_status(&FetchError::Internal), 500);

        assert!(fetch_error_json(&FetchError::InvalidRequest("x".into()))
            .contains("\"invalid_request\""));
        assert!(fetch_error_json(&FetchError::InvalidTarget).contains("\"invalid_fetch_target\""));
        assert!(
            fetch_error_json(&FetchError::NotConfigured).contains("\"fetch_store_not_configured\"")
        );
        assert!(fetch_error_json(&FetchError::Unavailable).contains("\"fetch_store_unavailable\""));
        assert!(fetch_error_json(&FetchError::AcquisitionFailed)
            .contains("\"fetch_acquisition_failed\""));
        assert!(fetch_error_json(&FetchError::PersistenceFailed)
            .contains("\"fetch_persistence_failed\""));
        assert!(fetch_error_json(&FetchError::Internal).contains("\"internal_error\""));
    }
}
