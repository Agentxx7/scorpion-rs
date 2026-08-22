//! Canonical Spider acquisition adapter for `spider_agent` research.
//!
//! This adapter delegates every request to Spider's existing one-shot
//! acquisition and evidence machinery. It retains typed evidence here and
//! exposes only neutral research input across the `spider_agent` boundary.

use crate::features::identity::EvidenceId;
use crate::utils::evidence::{
    build_evidence, fetch_single_page_with_options, AcquisitionOptions, EvidenceBundle,
};
#[cfg(feature = "disk")]
use crate::{
    features::domain_persistence::DomainPersistence,
    utils::evidence::{record_evidence, EvidenceRef},
};
use spider_agent::{AcquiredSource, AgentError, AgentResult, PageAcquirer};
use std::sync::{Arc, Mutex};

/// Canonical evidence retained for one research acquisition attempt.
#[derive(Debug, Clone)]
pub struct ResearchAcquisitionEvidence {
    /// Canonical acquisition identity returned opaquely to `spider_agent`.
    /// It is only a process-local correlation in ephemeral mode; in durable
    /// mode it is also the exact identity assigned to the ledger bundle.
    pub acquisition_id: EvidenceId,
    /// Canonical evidence built from the acquired [`crate::page::Page`].
    pub evidence: EvidenceBundle,
}

impl ResearchAcquisitionEvidence {
    /// Reference the durable evidence record, when this acquisition was
    /// performed by a durable adapter. Ephemeral acquisitions deliberately
    /// return `None` rather than manufacturing a reference to an unrecorded ID.
    #[cfg(feature = "disk")]
    pub fn evidence_ref(&self) -> Option<EvidenceRef> {
        self.evidence.id.map(EvidenceRef::new)
    }
}

/// Spider-owned implementation of the neutral research acquisition contract.
#[derive(Clone)]
pub struct CanonicalPageAcquirer {
    options: AcquisitionOptions,
    retained: Arc<Mutex<Vec<ResearchAcquisitionEvidence>>>,
    #[cfg(feature = "disk")]
    durable_store: Option<Arc<DomainPersistence>>,
}

impl CanonicalPageAcquirer {
    /// Construct an adapter using the supplied canonical acquisition options.
    pub fn new(options: AcquisitionOptions) -> Self {
        Self {
            options,
            retained: Arc::new(Mutex::new(Vec::new())),
            #[cfg(feature = "disk")]
            durable_store: None,
        }
    }

    /// Construct an adapter that records every acquired evidence bundle in
    /// the canonical durable evidence ledger before returning the source.
    /// Persistence failures are returned through the neutral acquisition
    /// error boundary; durable mode never falls back to ephemeral evidence.
    #[cfg(feature = "disk")]
    pub fn new_durable(options: AcquisitionOptions, store: Arc<DomainPersistence>) -> Self {
        Self {
            options,
            retained: Arc::new(Mutex::new(Vec::new())),
            durable_store: Some(store),
        }
    }

    /// Snapshot all evidence retained by this adapter in acquisition order.
    pub fn retained_evidence(&self) -> Vec<ResearchAcquisitionEvidence> {
        self.retained
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    async fn acquire_with_id(
        &self,
        url: &str,
        acquisition_id: EvidenceId,
    ) -> AgentResult<AcquiredSource> {
        let acquisition = fetch_single_page_with_options(url, self.options.clone())
            .await
            .map_err(AgentError::Remote)?;
        let page = acquisition.into_page();

        #[cfg(all(feature = "balance", not(feature = "decentralized")))]
        let mut page = page;

        #[cfg(all(feature = "balance", not(feature = "decentralized")))]
        if page.get_bytes().is_none() {
            page.ensure_html_loaded_async().await;
        }

        let content = page
            .get_bytes()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned());
        let requested_url = page.get_url().to_string();
        let final_url = page.get_url_final().to_string();
        let status = page.status_code.as_u16();
        let content_type = page
            .headers
            .as_ref()
            .and_then(|headers| headers.get(reqwest::header::CONTENT_TYPE))
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();

        let evidence = build_evidence(&page, content.clone(), false, false);
        #[cfg(feature = "disk")]
        let evidence = match &self.durable_store {
            Some(store) => {
                let mut evidence = evidence;
                evidence.id = Some(acquisition_id);
                let evidence = record_evidence(store, evidence).await.map_err(|error| {
                    AgentError::Remote(format!("durable evidence persistence failed: {error}"))
                })?;
                if evidence.id != Some(acquisition_id) {
                    return Err(AgentError::Remote(
                        "durable evidence persistence returned a different identity".to_string(),
                    ));
                }
                evidence
            }
            None => evidence,
        };
        self.retained
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(ResearchAcquisitionEvidence {
                acquisition_id,
                evidence,
            });

        let content = content.ok_or_else(|| {
            AgentError::Remote("canonical acquisition produced no materialized body".to_string())
        })?;

        Ok(AcquiredSource {
            requested_url,
            final_url,
            status,
            content_type,
            content,
            acquisition_id: Some(acquisition_id.to_string()),
        })
    }
}

impl Default for CanonicalPageAcquirer {
    fn default() -> Self {
        Self::new(AcquisitionOptions::default())
    }
}

#[async_trait::async_trait]
impl PageAcquirer for CanonicalPageAcquirer {
    async fn acquire(&self, url: &str) -> AgentResult<AcquiredSource> {
        let acquisition_id = EvidenceId::new();
        self.acquire_with_id(url, acquisition_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "disk")]
    use crate::utils::evidence::{read_evidence, record_evidence};
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[cfg(feature = "disk")]
    fn temporary_database_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "scorpion-research-{label}-{}.sqlite3",
            EvidenceId::new()
        ))
    }

    #[cfg(feature = "disk")]
    fn remove_temporary_database(path: &std::path::Path) {
        for candidate in [
            path.to_path_buf(),
            std::path::PathBuf::from(format!("{}-shm", path.display())),
            std::path::PathBuf::from(format!("{}-wal", path.display())),
        ] {
            let _ = std::fs::remove_file(candidate);
        }
    }

    fn fixture(
        status: u16,
        body: &'static [u8],
    ) -> (String, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/article", listener.local_addr().unwrap());
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_thread = attempts.clone();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            attempts_thread.fetch_add(1, Ordering::SeqCst);
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 {status} Test\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        });
        (url, attempts, handle)
    }

    #[tokio::test]
    async fn canonical_adapter_performs_one_acquisition_and_builds_existing_evidence() {
        const BODY: &[u8] = b"<html><body>canonical research</body></html>";
        let (url, attempts, handle) = fixture(200, BODY);
        let adapter = CanonicalPageAcquirer::default();

        let source = adapter.acquire(&url).await.unwrap();
        handle.join().unwrap();

        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(source.content.as_bytes(), BODY);
        let retained = adapter.retained_evidence();
        assert_eq!(retained.len(), 1);
        assert_eq!(
            source.acquisition_id,
            Some(retained[0].acquisition_id.to_string())
        );
        assert_eq!(retained[0].evidence.id, None, "evidence was not persisted");
        assert_eq!(
            retained[0].evidence.content.as_deref(),
            std::str::from_utf8(BODY).ok()
        );
        assert_eq!(
            retained[0].evidence.response_body_hash.as_deref(),
            Some(format!("{:x}", Sha256::digest(BODY)).as_str())
        );
    }

    #[tokio::test]
    async fn rejected_http_response_still_retains_canonical_evidence() {
        const BODY: &[u8] = b"<html><body>forbidden</body></html>";
        let (url, attempts, handle) = fixture(403, BODY);
        let adapter = CanonicalPageAcquirer::default();

        let source = adapter.acquire(&url).await.unwrap();
        handle.join().unwrap();

        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(source.status, 403);
        let retained = adapter.retained_evidence();
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].evidence.status_code, Some(403));
        assert_eq!(retained[0].evidence.observed_status_code, Some(403));
        assert!(retained[0].evidence.response_body_hash.is_some());
    }

    #[cfg(feature = "disk")]
    #[tokio::test]
    async fn ephemeral_adapter_never_writes_the_canonical_ledger() {
        const BODY: &[u8] = b"<html><body>ephemeral research</body></html>";
        let store = DomainPersistence::open_in_memory().await.unwrap();
        let (url, attempts, handle) = fixture(200, BODY);
        let adapter = CanonicalPageAcquirer::default();

        let source = adapter.acquire(&url).await.unwrap();
        handle.join().unwrap();

        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        let id: EvidenceId = source.acquisition_id.unwrap().parse().unwrap();
        assert!(read_evidence(&store, id).await.unwrap().is_none());
        let retained = adapter.retained_evidence();
        assert_eq!(retained[0].evidence.id, None);
        assert!(retained[0].evidence_ref().is_none());
    }

    #[cfg(feature = "disk")]
    #[tokio::test]
    async fn durable_adapter_preserves_identity_payload_and_resolves_after_reopen() {
        const BODY: &[u8] = b"<html><body>durable canonical research</body></html>";
        let database_path = temporary_database_path("reopen");
        remove_temporary_database(&database_path);
        let store = Arc::new(DomainPersistence::open(&database_path).await.unwrap());
        let (url, attempts, handle) = fixture(200, BODY);
        let adapter =
            CanonicalPageAcquirer::new_durable(AcquisitionOptions::default(), Arc::clone(&store));

        let source = adapter.acquire(&url).await.unwrap();
        handle.join().unwrap();

        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        let source_id: EvidenceId = source.acquisition_id.as_deref().unwrap().parse().unwrap();
        let retained = adapter.retained_evidence();
        assert_eq!(retained.len(), 1);
        let record = &retained[0];
        assert_eq!(record.acquisition_id, source_id);
        assert_eq!(record.evidence.id, Some(source_id));
        let evidence_ref = record.evidence_ref().unwrap();
        assert_eq!(evidence_ref.id(), source_id);
        assert_eq!(source.requested_url, url);
        assert_eq!(source.final_url, url);
        assert_eq!(source.status, 200);

        let expected = record.evidence.clone();
        assert_eq!(expected.requested_url.as_deref(), Some(url.as_str()));
        assert_eq!(expected.final_url.as_deref(), Some(url.as_str()));
        assert_eq!(expected.status_code, Some(200));
        assert_eq!(expected.observed_status_code, Some(200));
        assert_eq!(expected.content.as_deref(), std::str::from_utf8(BODY).ok());
        assert_eq!(
            expected.response_body_hash.as_deref(),
            Some(format!("{:x}", Sha256::digest(BODY)).as_str())
        );

        drop(retained);
        drop(adapter);
        drop(store);

        let reopened = DomainPersistence::open(&database_path).await.unwrap();
        let resolved = evidence_ref.resolve(&reopened).await.unwrap().unwrap();
        assert_eq!(resolved.id, expected.id);
        assert_eq!(resolved.requested_url, expected.requested_url);
        assert_eq!(resolved.final_url, expected.final_url);
        assert_eq!(resolved.status_code, expected.status_code);
        assert_eq!(resolved.observed_status_code, expected.observed_status_code);
        assert_eq!(resolved.content_type, expected.content_type);
        assert_eq!(
            resolved.detected_content_type,
            expected.detected_content_type
        );
        assert_eq!(resolved.content, expected.content);
        assert_eq!(resolved.response_body_hash, expected.response_body_hash);
        assert_eq!(
            resolved.transformed_content_hash,
            expected.transformed_content_hash
        );
        assert_eq!(resolved.transport, expected.transport);
        assert_eq!(resolved.dns, expected.dns);
        assert_eq!(resolved.backend_provenance, expected.backend_provenance);
        assert_eq!(resolved.response_origin, expected.response_origin);

        drop(reopened);
        remove_temporary_database(&database_path);
    }

    #[cfg(feature = "disk")]
    #[tokio::test]
    async fn durable_persistence_failure_returns_no_source_and_never_falls_back() {
        const BODY: &[u8] = b"<html><body>duplicate durable identity</body></html>";
        let store = Arc::new(DomainPersistence::open_in_memory().await.unwrap());
        let acquisition_id = EvidenceId::new();
        let existing = EvidenceBundle {
            id: Some(acquisition_id),
            content: Some("existing immutable evidence".to_string()),
            ..EvidenceBundle::default()
        };
        record_evidence(&store, existing).await.unwrap();
        let (url, attempts, handle) = fixture(200, BODY);
        let adapter =
            CanonicalPageAcquirer::new_durable(AcquisitionOptions::default(), Arc::clone(&store));

        let error = adapter
            .acquire_with_id(&url, acquisition_id)
            .await
            .unwrap_err();
        handle.join().unwrap();

        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert!(error
            .to_string()
            .contains("durable evidence persistence failed"));
        assert!(adapter.retained_evidence().is_empty());
        let resolved = EvidenceRef::new(acquisition_id)
            .resolve(&store)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            resolved.content.as_deref(),
            Some("existing immutable evidence")
        );
    }

    #[cfg(feature = "disk")]
    #[tokio::test]
    async fn durable_adapter_records_pages_rejected_by_later_research_validation() {
        const BODY: &[u8] = b"<html><body>forbidden but durable</body></html>";
        let store = Arc::new(DomainPersistence::open_in_memory().await.unwrap());
        let (url, attempts, handle) = fixture(403, BODY);
        let adapter =
            CanonicalPageAcquirer::new_durable(AcquisitionOptions::default(), Arc::clone(&store));

        let source = adapter.acquire(&url).await.unwrap();
        handle.join().unwrap();

        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(source.status, 403);
        let id: EvidenceId = source.acquisition_id.unwrap().parse().unwrap();
        let resolved = EvidenceRef::new(id).resolve(&store).await.unwrap().unwrap();
        assert_eq!(resolved.id, Some(id));
        assert_eq!(resolved.status_code, Some(403));
        assert_eq!(resolved.observed_status_code, Some(403));
        assert_eq!(resolved.content.as_deref(), std::str::from_utf8(BODY).ok());
    }
}
