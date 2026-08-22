//! Canonical Spider acquisition adapter for `spider_agent` research.
//!
//! This adapter delegates every request to Spider's existing one-shot
//! acquisition and evidence machinery. It retains typed evidence here and
//! exposes only neutral research input across the `spider_agent` boundary.

use crate::features::identity::EvidenceId;
use crate::utils::evidence::{
    build_evidence, fetch_single_page_with_options, AcquisitionOptions, EvidenceBundle,
};
use spider_agent::{AcquiredSource, AgentError, AgentResult, PageAcquirer};
use std::sync::{Arc, Mutex};

/// Canonical evidence retained for one research acquisition attempt.
#[derive(Debug, Clone)]
pub struct ResearchAcquisitionEvidence {
    /// In-memory correlation identity returned opaquely to `spider_agent`.
    /// Minting it does not persist the evidence bundle.
    pub acquisition_id: EvidenceId,
    /// Canonical evidence built from the acquired [`crate::page::Page`].
    pub evidence: EvidenceBundle,
}

/// Spider-owned implementation of the neutral research acquisition contract.
#[derive(Clone)]
pub struct CanonicalPageAcquirer {
    options: AcquisitionOptions,
    retained: Arc<Mutex<Vec<ResearchAcquisitionEvidence>>>,
}

impl CanonicalPageAcquirer {
    /// Construct an adapter using the supplied canonical acquisition options.
    pub fn new(options: AcquisitionOptions) -> Self {
        Self {
            options,
            retained: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Snapshot all evidence retained by this adapter in acquisition order.
    pub fn retained_evidence(&self) -> Vec<ResearchAcquisitionEvidence> {
        self.retained
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
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
        let acquisition_id = EvidenceId::new();
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

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
}
