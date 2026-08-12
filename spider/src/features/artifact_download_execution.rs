//! Canonical execution of an already-resolved [`ArtifactDownloadBinding`]:
//! stream the remote artifact through the canonical transport streaming
//! request seam, straight to a caller-owned destination on disk, without
//! ever materializing the full body in memory.
//!
//! ```text
//! ArtifactDownloadBinding + destination
//!       │
//!       ▼
//! transport::execute_streaming_request  <- status/final URL/headers only
//!       │
//!       ▼
//! non-success status?  -> NonSuccessStatus, nothing written
//!       │
//!       ▼
//! destination already exists? -> DestinationAlreadyExists, nothing written
//!       │
//!       ▼
//! stream chunks -> temp file (StreamingWriter) + running SHA-256
//!       │                       (bytes_written tracked exactly)
//!       ▼
//! mid-stream body error? -> StreamFailed, temp file removed
//!       │                                      (or CleanupFailed
//!       ▼                                       if removal itself fails)
//! declared size (Content-Length / ArtifactReference::size_bytes)
//! disagrees with bytes_written? -> SizeMismatch, temp file removed
//!       │                                      (or CleanupFailed
//!       ▼                                       if removal itself fails)
//! locally verifiable declared identities (LfsSha256 only — see
//! [`is_locally_verifiable`]'s doc comment) disagree with the computed
//! hash?  -> IdentityMismatch, temp file removed
//!       │                                      (or CleanupFailed
//!       ▼                                       if removal itself fails)
//! atomic rename: temp file -> destination
//!       │
//!       ▼
//! Ok(AcquiredArtifact)
//! ```
//!
//! This module performs no provider resolution, no provider-specific
//! behavior, no retry orchestration, no evidence-bundle integration, and
//! introduces no second HTTP client or storage subsystem: the one and
//! only network call is [`transport::execute_streaming_request`]; the one
//! and only filesystem primitive is [`crate::utils::uring_fs`]'s existing
//! streaming writer plus a single `rename` for atomic finalization.
//!
//! Requires the `evidence` feature (for the `sha2` dependency used to
//! verify locally-computable identities) and, like the streaming
//! transport seam itself, is unavailable under `wreq` —
//! neither of those client stacks is audited by `transport`'s streaming
//! seam, so this executor is equally absent there rather than silently
//! degrading.

use crate::features::artifact_download_binding::ArtifactDownloadBinding;
use crate::features::artifact_reference::{ArtifactIdentityKind, ArtifactReference};
use crate::features::secret_request_headers::SecretRequestHeaders;
use crate::features::transport::{self, TransportError};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_stream::StreamExt;

/// Process-local uniqueness source for temp file names — mirrors the
/// established `SPOOL_FILE_COUNTER` idiom in
/// `crate::utils::html_spool`, applied here instead of inventing a new
/// naming scheme.
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Outcome of comparing one provider-declared [`ArtifactIdentityKind`]
/// against the downloaded bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactIdentityVerification {
    /// Locally computed from the downloaded bytes and matched the
    /// provider-declared value exactly (case-insensitive hex compare).
    Verified,
    /// This identity's algorithm/encoding is not locally computable from
    /// raw downloaded bytes alone with the primitives this crate has
    /// audited — see [`is_locally_verifiable`]'s doc comment. Preserved
    /// as metadata, explicitly reported as unverified, never silently
    /// dropped and never treated as failure.
    NotLocallyVerified,
}

/// One declared identity paired with the outcome of attempting to verify
/// it locally. A [`ArtifactIdentityVerification::Verified`] entry never
/// coexists with a mismatch — a mismatch fails the whole download closed
/// (see [`ArtifactDownloadExecutionError::IdentityMismatch`]) rather than
/// being reported here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedIdentity {
    /// Identity namespace, copied from the provider-declared identity.
    pub kind: ArtifactIdentityKind,
    /// Provider-declared value, copied verbatim.
    pub declared_value: String,
    /// What this executor was able to establish about it.
    pub verification: ArtifactIdentityVerification,
}

/// Truthful result of one completed artifact download. Every field
/// reflects something this executor actually observed — never
/// provider-declared metadata restated as if independently confirmed
/// beyond what [`VerifiedIdentity`] itself states.
#[derive(Debug, Clone)]
pub struct AcquiredArtifact {
    /// The provider-declared metadata this download was executed
    /// against, unmutated.
    pub artifact: ArtifactReference,
    /// The response URL after any redirects the transport seam followed.
    pub final_url: url::Url,
    /// The HTTP status actually observed (always a 2xx by the time this
    /// type exists — see [`ArtifactDownloadExecutionError::NonSuccessStatus`]).
    pub status_code: u16,
    /// The response's `Content-Type` header, verbatim, when present.
    pub content_type: Option<String>,
    /// Exact path the completed artifact was atomically finalized to.
    pub destination: PathBuf,
    /// Exact number of bytes written — always equal to the response body
    /// length actually observed, never a declared/assumed value.
    pub bytes_written: u64,
    /// SHA-256 of the exact downloaded bytes, computed while streaming.
    /// Always present, independent of whether the provider declared a
    /// `LfsSha256` identity to compare it against.
    pub sha256_hex: String,
    /// One entry per provider-declared identity in
    /// [`ArtifactReference::identities`], in the same order.
    pub identity_verifications: Vec<VerifiedIdentity>,
}

/// Deterministic, secret-safe artifact download execution failures. Every
/// variant that could plausibly have already started writing bytes to
/// disk either had its temp file removed before this error is returned,
/// or is wrapped by [`ArtifactDownloadExecutionError::CleanupFailed`] when
/// removal itself failed — see module docs.
#[derive(Debug)]
pub enum ArtifactDownloadExecutionError {
    /// [`transport::execute_streaming_request`] itself failed — covers
    /// request construction/execution failure, a rejected redirect, and
    /// a binding whose target the active transport policy forbids (a
    /// hand-constructed [`ArtifactDownloadBinding`] bypassing
    /// [`crate::features::artifact_download_binding::bind`]'s own
    /// validation is still caught here, fail-closed, before any file is
    /// touched — this executor never re-implements that check).
    Transport(TransportError),
    /// The response was established but its HTTP status was not success
    /// (2xx). Nothing was written to disk.
    NonSuccessStatus {
        /// The observed status code.
        status: u16,
        /// The final resolved URL the non-success status came from.
        final_url: url::Url,
    },
    /// `destination` already exists. This executor never silently
    /// overwrites a completed artifact — no canonical overwrite policy
    /// exists yet, so the safe default is to fail closed. Nothing was
    /// written.
    DestinationAlreadyExists(PathBuf),
    /// Creating, writing to, or closing the temporary destination file
    /// failed. Best-effort cleanup of any partial temp file was
    /// attempted.
    DestinationIoFailed(String),
    /// The response body stream itself failed after status/headers were
    /// already established (network drop, decode error, truncated
    /// chunked body). The temp file was removed.
    StreamFailed(String),
    /// A declared byte count — either the response's own `Content-Length`
    /// or [`ArtifactReference::size_bytes`] — did not match the exact
    /// number of bytes actually streamed. The temp file was removed.
    SizeMismatch {
        /// Which declared value disagreed.
        source: DeclaredSizeSource,
        /// The declared byte count.
        declared: u64,
        /// The exact byte count actually streamed.
        actual: u64,
    },
    /// A locally-verifiable declared identity (see
    /// [`is_locally_verifiable`]) did not match the computed value. The
    /// temp file was removed.
    IdentityMismatch {
        /// Which identity namespace disagreed.
        kind: ArtifactIdentityKind,
        /// Provider-declared value.
        declared: String,
        /// Locally computed value.
        computed: String,
    },
    /// The completed, fully-verified download could not be atomically
    /// finalized into `destination` (the `rename` itself failed). Cleanup
    /// of the temp file was attempted; if that cleanup also failed, this
    /// is wrapped by [`ArtifactDownloadExecutionError::CleanupFailed`].
    FinalizationFailed(String),
    /// The primary execution failure (`primary`) occurred, and the
    /// best-effort cleanup of the temporary `.part-*` file also failed
    /// (`cleanup`). The destination was never touched. The temp file may
    /// still exist; the caller receives both failure reasons truthfully.
    CleanupFailed {
        /// The original execution failure that triggered cleanup.
        primary: Box<ArtifactDownloadExecutionError>,
        /// Why the temp file could not be removed.
        cleanup: String,
    },
}

/// Which declared byte count a [`ArtifactDownloadExecutionError::SizeMismatch`]
/// disagreed with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclaredSizeSource {
    /// The response's own `Content-Length` header.
    ContentLength,
    /// [`ArtifactReference::size_bytes`], as declared by the provider.
    ProviderDeclaredSize,
}

impl std::fmt::Display for ArtifactDownloadExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(error) => write!(f, "{error}"),
            Self::NonSuccessStatus { status, final_url } => {
                write!(
                    f,
                    "artifact download failed with HTTP status {status} at {final_url}"
                )
            }
            Self::DestinationAlreadyExists(path) => {
                write!(f, "artifact destination already exists: {}", path.display())
            }
            Self::DestinationIoFailed(message) => {
                write!(f, "artifact destination I/O failed: {message}")
            }
            Self::StreamFailed(message) => write!(f, "artifact body stream failed: {message}"),
            Self::SizeMismatch {
                source,
                declared,
                actual,
            } => {
                let source = match source {
                    DeclaredSizeSource::ContentLength => "Content-Length",
                    DeclaredSizeSource::ProviderDeclaredSize => "provider-declared size",
                };
                write!(
                    f,
                    "artifact size mismatch: {source} declared {declared} bytes, actually streamed {actual} bytes"
                )
            }
            Self::IdentityMismatch {
                kind,
                declared,
                computed,
            } => write!(
                f,
                "artifact identity mismatch ({kind:?}): declared {declared}, computed {computed}"
            ),
            Self::FinalizationFailed(message) => {
                write!(f, "artifact finalization failed: {message}")
            }
            Self::CleanupFailed { primary, cleanup } => {
                write!(
                    f,
                    "{primary}; additionally, cleanup of the temporary artifact file failed: {cleanup}"
                )
            }
        }
    }
}

impl std::error::Error for ArtifactDownloadExecutionError {}

/// Whether `kind`'s algorithm/encoding is one this crate can truthfully
/// compute from raw downloaded bytes alone.
///
/// - [`ArtifactIdentityKind::LfsSha256`]: Git LFS's declared identity
///   *is* the plain SHA-256 of the raw file content (the pointer file's
///   `oid sha256:<hex>` field) — locally computable in one streaming
///   pass, no different from any other whole-content SHA-256. The only
///   locally verifiable kind.
/// - [`ArtifactIdentityKind::GitBlobOid`]: deliberately **not**
///   verified. A Git blob object ID hashes `"blob " + <exact length> +
///   "\0" + <content>`, not raw content alone, and its algorithm
///   (SHA-1 vs SHA-256, depending on the repository's object format) is
///   not recorded anywhere in [`ArtifactReference`]. Computing it
///   correctly while streaming would require trusting an upfront
///   declared length to seed the hash prefix before any content is
///   verified against it — exactly the kind of length source
///   ([`ArtifactReference::size_bytes`] or `Content-Length`) this
///   executor already treats as untrusted until independently confirmed
///   against actual bytes. Verifying it would mean building on an
///   unverified premise; preserved as metadata instead.
/// - [`ArtifactIdentityKind::XetHash`]: Xet identity is a content-defined
///   chunking/deduplication scheme, not a single whole-file hash
///   function — not reproducible from a linear byte stream with the
///   primitives this crate has audited. Preserved as metadata instead.
fn is_locally_verifiable(kind: ArtifactIdentityKind) -> bool {
    matches!(kind, ArtifactIdentityKind::LfsSha256)
}

/// A unique sibling temp-file path for `destination`, in the same parent
/// directory (so the final `rename` is guaranteed same-filesystem and
/// therefore atomic on every platform this crate targets).
fn temp_path_for(destination: &Path) -> PathBuf {
    let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut file_name = destination
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    file_name.push(format!(".part-{}-{counter}", std::process::id()));
    destination.with_file_name(file_name)
}

// Test-only seam to force `cleanup_temp_file` to fail deterministically
// on the current thread. Never used outside unit tests.
#[cfg(test)]
thread_local! {
    static FORCE_CLEANUP_FAILURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Remove the temporary `.part-*` file at `path`. Returns `Ok(())` when the
/// file no longer exists (including if it never existed). Returns `Err` if
/// the removal itself failed, so callers can surface that failure truthfully.
async fn cleanup_temp_file(path: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if FORCE_CLEANUP_FAILURE.with(|flag| flag.get()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "injected cleanup failure",
        ));
    }
    crate::utils::uring_fs::remove_file(path.display().to_string()).await
}

/// Attempt to remove `temp_path` and return `primary` if cleanup succeeds,
/// or [`ArtifactDownloadExecutionError::CleanupFailed`] if cleanup itself
/// fails. The destination is never touched either way.
async fn cleanup_and_fail_with(
    temp_path: &Path,
    primary: ArtifactDownloadExecutionError,
) -> ArtifactDownloadExecutionError {
    match cleanup_temp_file(temp_path).await {
        Ok(()) => primary,
        Err(cleanup) => ArtifactDownloadExecutionError::CleanupFailed {
            primary: Box::new(primary),
            cleanup: cleanup.to_string(),
        },
    }
}

/// Execute an already-resolved [`ArtifactDownloadBinding`], streaming the
/// artifact straight to `destination` without ever materializing the
/// full body in memory. See module docs for the exact sequencing and
/// [`ArtifactDownloadExecutionError`] for the full failure vocabulary.
///
/// Never re-resolves a provider URL, never constructs a second HTTP
/// client, never constructs [`crate::page::Page`] or
/// [`crate::website::Website`] — the one network call is
/// [`transport::execute_streaming_request`], reused exactly as the prior
/// frontier defined it, honoring `binding.transport` and
/// `binding.headers` exactly as supplied.
pub async fn execute(
    binding: &ArtifactDownloadBinding,
    destination: &Path,
) -> Result<AcquiredArtifact, ArtifactDownloadExecutionError> {
    let empty_headers = SecretRequestHeaders::new();
    let headers = binding.headers.as_ref().unwrap_or(&empty_headers);

    let response =
        transport::execute_streaming_request(&binding.resolved_url, &binding.transport, headers)
            .await
            .map_err(ArtifactDownloadExecutionError::Transport)?;

    let status = response.status();
    let final_url = response.url().clone();
    if !status.is_success() {
        return Err(ArtifactDownloadExecutionError::NonSuccessStatus {
            status: status.as_u16(),
            final_url,
        });
    }

    if destination.exists() {
        return Err(ArtifactDownloadExecutionError::DestinationAlreadyExists(
            destination.to_path_buf(),
        ));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let declared_content_length = response.content_length();

    let temp_path = temp_path_for(destination);
    let writer = crate::utils::uring_fs::StreamingWriter::create(temp_path.display().to_string())
        .await
        .map_err(|error| ArtifactDownloadExecutionError::DestinationIoFailed(error.to_string()))?;

    let mut hasher = Sha256::new();
    let mut bytes_written: u64 = 0;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                return Err(cleanup_and_fail_with(
                    &temp_path,
                    ArtifactDownloadExecutionError::StreamFailed(error.to_string()),
                )
                .await);
            }
        };
        if let Err(error) = writer.write(&chunk).await {
            return Err(cleanup_and_fail_with(
                &temp_path,
                ArtifactDownloadExecutionError::DestinationIoFailed(error.to_string()),
            )
            .await);
        }
        hasher.update(&chunk);
        bytes_written += chunk.len() as u64;
    }

    if let Err(error) = writer.close().await {
        return Err(cleanup_and_fail_with(
            &temp_path,
            ArtifactDownloadExecutionError::DestinationIoFailed(error.to_string()),
        )
        .await);
    }

    if let Some(declared) = declared_content_length {
        if declared != bytes_written {
            return Err(cleanup_and_fail_with(
                &temp_path,
                ArtifactDownloadExecutionError::SizeMismatch {
                    source: DeclaredSizeSource::ContentLength,
                    declared,
                    actual: bytes_written,
                },
            )
            .await);
        }
    }
    if let Some(declared) = binding.artifact.size_bytes {
        if declared != bytes_written {
            return Err(cleanup_and_fail_with(
                &temp_path,
                ArtifactDownloadExecutionError::SizeMismatch {
                    source: DeclaredSizeSource::ProviderDeclaredSize,
                    declared,
                    actual: bytes_written,
                },
            )
            .await);
        }
    }

    let sha256_hex = format!("{:x}", hasher.finalize());

    let mut identity_verifications = Vec::with_capacity(binding.artifact.identities.len());
    for identity in &binding.artifact.identities {
        if !is_locally_verifiable(identity.kind) {
            identity_verifications.push(VerifiedIdentity {
                kind: identity.kind,
                declared_value: identity.value.clone(),
                verification: ArtifactIdentityVerification::NotLocallyVerified,
            });
            continue;
        }
        if !identity.value.eq_ignore_ascii_case(&sha256_hex) {
            return Err(cleanup_and_fail_with(
                &temp_path,
                ArtifactDownloadExecutionError::IdentityMismatch {
                    kind: identity.kind,
                    declared: identity.value.clone(),
                    computed: sha256_hex,
                },
            )
            .await);
        }
        identity_verifications.push(VerifiedIdentity {
            kind: identity.kind,
            declared_value: identity.value.clone(),
            verification: ArtifactIdentityVerification::Verified,
        });
    }

    if let Err(error) = tokio::fs::rename(&temp_path, destination).await {
        return Err(cleanup_and_fail_with(
            &temp_path,
            ArtifactDownloadExecutionError::FinalizationFailed(error.to_string()),
        )
        .await);
    }

    Ok(AcquiredArtifact {
        artifact: binding.artifact.clone(),
        final_url,
        status_code: status.as_u16(),
        content_type,
        destination: destination.to_path_buf(),
        bytes_written,
        sha256_hex,
        identity_verifications,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::artifact_reference::ArtifactIdentity;
    use crate::features::source_provider::ProviderId;
    use crate::features::transport::TransportPolicy;
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A local, deterministic HTTP fixture. Matches the established
    /// `tokio::net::TcpListener` fixture convention already used by
    /// `spider/tests/transport_tor.rs` and `transport.rs`'s own
    /// `streaming_request` test module. No internet-dependent test.
    struct HttpFixture {
        addr: SocketAddr,
    }

    impl HttpFixture {
        async fn start(
            status: &'static str,
            extra_headers: &'static str,
            body: &'static [u8],
        ) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                loop {
                    let (mut stream, _) = match listener.accept().await {
                        Ok(pair) => pair,
                        Err(_) => break,
                    };
                    tokio::spawn(async move {
                        let mut buf = [0_u8; 4096];
                        let _ = stream.read(&mut buf).await;
                        let response = format!(
                            "HTTP/1.1 {status}\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.write_all(body).await;
                    });
                }
            });
            Self { addr }
        }

        fn url(&self) -> url::Url {
            url::Url::parse(&format!("http://{}/artifact", self.addr)).unwrap()
        }
    }

    /// One process-unique scratch directory per test, removed at the end.
    struct ScratchDir(std::path::PathBuf);

    impl ScratchDir {
        fn new(name: &str) -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let n = COUNTER.fetch_add(1, AtomicOrdering::SeqCst);
            let dir = std::env::temp_dir().join(format!(
                "spider_artifact_download_execution_test_{name}_{}_{n}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self, file_name: &str) -> std::path::PathBuf {
            self.0.join(file_name)
        }

        fn entries(&self) -> Vec<std::path::PathBuf> {
            std::fs::read_dir(&self.0)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect()
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn artifact(size_bytes: Option<u64>, identities: Vec<ArtifactIdentity>) -> ArtifactReference {
        ArtifactReference {
            provider_id: ProviderId::from("generic_provider"),
            repository_id: "owner/repository".to_string(),
            path: "weights/model.bin".to_string(),
            requested_revision: Some("main".to_string()),
            resolved_revision: Some("immutable-revision".to_string()),
            size_bytes,
            identities,
            download_url: None,
            discovered_via: None,
        }
    }

    fn binding_for(url: url::Url, artifact: ArtifactReference) -> ArtifactDownloadBinding {
        ArtifactDownloadBinding {
            artifact,
            resolved_url: url,
            transport: TransportPolicy::Default,
            headers: None,
        }
    }

    fn sha256_of(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    const PAYLOAD: &[u8] = b"artifact download execution frontier fixture payload bytes";

    #[tokio::test]
    async fn successful_download_streams_to_destination_and_computes_sha256() {
        let fixture = HttpFixture::start(
            "200 OK",
            "Content-Type: application/octet-stream\r\n",
            PAYLOAD,
        )
        .await;
        let scratch = ScratchDir::new("success");
        let destination = scratch.path("model.bin");
        let binding = binding_for(fixture.url(), artifact(None, Vec::new()));

        let acquired = execute(&binding, &destination).await.unwrap();

        assert_eq!(acquired.status_code, 200);
        assert_eq!(acquired.bytes_written, PAYLOAD.len() as u64);
        assert_eq!(acquired.sha256_hex, sha256_of(PAYLOAD));
        assert_eq!(acquired.destination, destination);
        assert_eq!(
            acquired.content_type.as_deref(),
            Some("application/octet-stream")
        );
        assert!(acquired.identity_verifications.is_empty());

        let on_disk = std::fs::read(&destination).unwrap();
        assert_eq!(on_disk, PAYLOAD);
        // No leftover temp file beside the finalized artifact.
        assert_eq!(scratch.entries(), vec![destination]);
    }

    #[tokio::test]
    async fn lfs_sha256_identity_matching_the_bytes_is_verified() {
        let fixture = HttpFixture::start("200 OK", "", PAYLOAD).await;
        let scratch = ScratchDir::new("lfs_match");
        let destination = scratch.path("model.bin");
        let identities = vec![ArtifactIdentity {
            kind: ArtifactIdentityKind::LfsSha256,
            value: sha256_of(PAYLOAD).to_ascii_uppercase(), // case-insensitive compare
        }];
        let binding = binding_for(fixture.url(), artifact(None, identities));

        let acquired = execute(&binding, &destination).await.unwrap();

        assert_eq!(acquired.identity_verifications.len(), 1);
        assert_eq!(
            acquired.identity_verifications[0].verification,
            ArtifactIdentityVerification::Verified
        );
        assert_eq!(
            acquired.identity_verifications[0].kind,
            ArtifactIdentityKind::LfsSha256
        );
    }

    #[tokio::test]
    async fn lfs_sha256_identity_mismatch_fails_closed_and_cleans_up() {
        let fixture = HttpFixture::start("200 OK", "", PAYLOAD).await;
        let scratch = ScratchDir::new("lfs_mismatch");
        let destination = scratch.path("model.bin");
        let identities = vec![ArtifactIdentity {
            kind: ArtifactIdentityKind::LfsSha256,
            value: "0".repeat(64),
        }];
        let binding = binding_for(fixture.url(), artifact(None, identities));

        let error = execute(&binding, &destination).await.unwrap_err();
        assert!(matches!(
            error,
            ArtifactDownloadExecutionError::IdentityMismatch { .. }
        ));
        assert!(!destination.exists());
        assert!(scratch.entries().is_empty(), "no leftover temp file");
    }

    #[tokio::test]
    async fn git_blob_oid_and_xet_hash_are_preserved_as_not_locally_verified() {
        let fixture = HttpFixture::start("200 OK", "", PAYLOAD).await;
        let scratch = ScratchDir::new("unverifiable_identities");
        let destination = scratch.path("model.bin");
        let identities = vec![
            ArtifactIdentity {
                kind: ArtifactIdentityKind::GitBlobOid,
                value: "deadbeef".to_string(),
            },
            ArtifactIdentity {
                kind: ArtifactIdentityKind::XetHash,
                value: "xet-opaque-value".to_string(),
            },
        ];
        let binding = binding_for(fixture.url(), artifact(None, identities));

        let acquired = execute(&binding, &destination).await.unwrap();

        assert_eq!(acquired.identity_verifications.len(), 2);
        for verification in &acquired.identity_verifications {
            assert_eq!(
                verification.verification,
                ArtifactIdentityVerification::NotLocallyVerified
            );
        }
        // Never silently dropped: declared values are preserved verbatim.
        assert_eq!(
            acquired.identity_verifications[0].declared_value,
            "deadbeef"
        );
        assert_eq!(
            acquired.identity_verifications[1].declared_value,
            "xet-opaque-value"
        );
    }

    #[tokio::test]
    async fn non_success_status_writes_nothing() {
        let fixture = HttpFixture::start("404 Not Found", "", b"not found").await;
        let scratch = ScratchDir::new("non_success");
        let destination = scratch.path("model.bin");
        let binding = binding_for(fixture.url(), artifact(None, Vec::new()));

        let error = execute(&binding, &destination).await.unwrap_err();
        match error {
            ArtifactDownloadExecutionError::NonSuccessStatus { status, .. } => {
                assert_eq!(status, 404)
            }
            other => panic!("expected NonSuccessStatus, got {other:?}"),
        }
        assert!(scratch.entries().is_empty());
    }

    #[tokio::test]
    async fn destination_already_exists_is_rejected_without_overwriting() {
        let fixture = HttpFixture::start("200 OK", "", PAYLOAD).await;
        let scratch = ScratchDir::new("already_exists");
        let destination = scratch.path("model.bin");
        std::fs::write(&destination, b"pre-existing completed artifact").unwrap();
        let binding = binding_for(fixture.url(), artifact(None, Vec::new()));

        let error = execute(&binding, &destination).await.unwrap_err();
        assert!(matches!(
            error,
            ArtifactDownloadExecutionError::DestinationAlreadyExists(_)
        ));
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"pre-existing completed artifact"
        );
    }

    #[tokio::test]
    async fn provider_declared_size_mismatch_fails_closed_and_cleans_up() {
        let fixture = HttpFixture::start("200 OK", "", PAYLOAD).await;
        let scratch = ScratchDir::new("size_mismatch");
        let destination = scratch.path("model.bin");
        let binding = binding_for(
            fixture.url(),
            artifact(Some(PAYLOAD.len() as u64 + 1), Vec::new()),
        );

        let error = execute(&binding, &destination).await.unwrap_err();
        match error {
            ArtifactDownloadExecutionError::SizeMismatch {
                source,
                declared,
                actual,
            } => {
                assert_eq!(source, DeclaredSizeSource::ProviderDeclaredSize);
                assert_eq!(declared, PAYLOAD.len() as u64 + 1);
                assert_eq!(actual, PAYLOAD.len() as u64);
            }
            other => panic!("expected SizeMismatch, got {other:?}"),
        }
        assert!(!destination.exists());
        assert!(scratch.entries().is_empty());
    }

    /// Mid-stream truncation (advertised `Content-Length` exceeds what the
    /// server actually sends before closing) surfaces as `StreamFailed`,
    /// not a silently short/successful download, and leaves no temp file
    /// behind.
    #[tokio::test]
    async fn mid_stream_truncation_surfaces_as_stream_failed_and_cleans_up() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0_u8; 4096];
                let _ = stream.read(&mut buf).await;
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\nshort",
                    )
                    .await;
            }
        });

        let scratch = ScratchDir::new("truncated");
        let destination = scratch.path("model.bin");
        let url = url::Url::parse(&format!("http://{addr}/artifact")).unwrap();
        let binding = binding_for(url, artifact(None, Vec::new()));

        let error = execute(&binding, &destination).await.unwrap_err();
        assert!(matches!(
            error,
            ArtifactDownloadExecutionError::StreamFailed(_)
        ));
        assert!(!destination.exists());
        assert!(scratch.entries().is_empty(), "no leftover temp file");
    }

    /// The transport-level onion/`Default` fail-closed rule is reused,
    /// not reimplemented: a hand-constructed binding whose target the
    /// active policy forbids is caught by `execute_streaming_request`
    /// itself, before any file is touched.
    #[tokio::test]
    async fn transport_rejection_is_surfaced_and_writes_nothing() {
        let scratch = ScratchDir::new("transport_rejection");
        let destination = scratch.path("model.bin");
        let onion_url =
            url::Url::parse("http://exampleexampleexampleexamp.onion/model.bin").unwrap();
        let binding = binding_for(onion_url, artifact(None, Vec::new()));

        let error = execute(&binding, &destination).await.unwrap_err();
        assert!(matches!(
            error,
            ArtifactDownloadExecutionError::Transport(TransportError::OnionRequiresTor)
        ));
        assert!(scratch.entries().is_empty());
    }

    /// RAII guard that forces `cleanup_temp_file` to fail for the duration
    /// of one test, resetting the flag even if the test panics.
    struct ForceCleanupFailureGuard;

    impl ForceCleanupFailureGuard {
        fn new() -> Self {
            FORCE_CLEANUP_FAILURE.with(|flag| flag.set(true));
            Self
        }
    }

    impl Drop for ForceCleanupFailureGuard {
        fn drop(&mut self) {
            FORCE_CLEANUP_FAILURE.with(|flag| flag.set(false));
        }
    }

    /// Cleanup failure is not silently swallowed: the primary failure is
    /// preserved inside `CleanupFailed`, and the cleanup failure reason is
    /// included verbatim. The destination is untouched and the temp file
    /// remains (because removal failed).
    #[tokio::test(flavor = "current_thread")]
    async fn cleanup_failure_is_surfaced_truthfully() {
        let fixture = HttpFixture::start("200 OK", "", PAYLOAD).await;
        let scratch = ScratchDir::new("cleanup_failure");
        let destination = scratch.path("model.bin");
        let identities = vec![ArtifactIdentity {
            kind: ArtifactIdentityKind::LfsSha256,
            value: "0".repeat(64),
        }];
        let binding = binding_for(fixture.url(), artifact(None, identities));

        let _guard = ForceCleanupFailureGuard::new();
        let error = execute(&binding, &destination).await.unwrap_err();

        match error {
            ArtifactDownloadExecutionError::CleanupFailed { primary, cleanup } => {
                assert!(
                    matches!(
                        *primary,
                        ArtifactDownloadExecutionError::IdentityMismatch { .. }
                    ),
                    "primary error preserved: {primary:?}"
                );
                assert!(
                    cleanup.contains("injected cleanup failure"),
                    "cleanup reason preserved: {cleanup}"
                );
            }
            other => panic!("expected CleanupFailed, got {other:?}"),
        }
        assert!(!destination.exists());
        let entries = scratch.entries();
        assert_eq!(entries.len(), 1, "temp file remains when cleanup fails");
        assert!(
            entries[0]
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".part-"),
            "leftover entry is the temp file: {:?}",
            entries[0]
        );
    }

    /// `StreamingWriter::create` calls `tokio::fs::File::create` and
    /// returns `Err` via `?` before any file descriptor is handed back. An
    /// error at this stage therefore leaves no filesystem state behind.
    #[tokio::test]
    async fn streaming_writer_create_failure_leaves_no_state() {
        let fixture = HttpFixture::start("200 OK", "", PAYLOAD).await;
        let scratch = ScratchDir::new("create_failure");
        let destination = scratch.path("missing_parent/model.bin");
        let binding = binding_for(fixture.url(), artifact(None, Vec::new()));

        let error = execute(&binding, &destination).await.unwrap_err();
        assert!(
            matches!(
                error,
                ArtifactDownloadExecutionError::DestinationIoFailed(_)
            ),
            "expected DestinationIoFailed, got {error:?}"
        );
        assert!(
            scratch.entries().is_empty(),
            "no file or directory created when StreamingWriter::create fails"
        );
    }

    /// Section P: Default/Tor parity — the exact same executor, routed
    /// through a local SOCKS5 fixture that splices to the same HTTP
    /// fixture used by the Default-policy tests above. No public Tor
    /// dependency; mirrors the minimal-fixture convention already
    /// established by `spider/tests/transport_tor.rs`.
    #[cfg(feature = "transport_tor")]
    mod tor_parity {
        use super::*;
        use crate::features::transport::TorTransportConfig;

        struct SocksFixture {
            addr: SocketAddr,
        }

        impl SocksFixture {
            async fn start_splicing(splice_to: SocketAddr) -> Self {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap();
                tokio::spawn(async move {
                    loop {
                        let (stream, _) = match listener.accept().await {
                            Ok(pair) => pair,
                            Err(_) => break,
                        };
                        tokio::spawn(Self::serve_one(stream, splice_to));
                    }
                });
                Self { addr }
            }

            async fn serve_one(
                mut stream: tokio::net::TcpStream,
                splice_to: SocketAddr,
            ) -> std::io::Result<()> {
                let mut header = [0_u8; 2];
                stream.read_exact(&mut header).await?;
                let mut methods = vec![0_u8; header[1] as usize];
                stream.read_exact(&mut methods).await?;
                stream.write_all(&[0x05, 0x00]).await?;

                let mut req_head = [0_u8; 4];
                stream.read_exact(&mut req_head).await?;
                match req_head[3] {
                    0x01 => {
                        let mut rest = [0_u8; 6];
                        stream.read_exact(&mut rest).await?;
                    }
                    0x03 => {
                        let mut len_buf = [0_u8; 1];
                        stream.read_exact(&mut len_buf).await?;
                        let mut rest = vec![0_u8; len_buf[0] as usize + 2];
                        stream.read_exact(&mut rest).await?;
                    }
                    _ => return Ok(()),
                }

                stream
                    .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                    .await?;

                let mut upstream = tokio::net::TcpStream::connect(splice_to).await?;
                let (mut ri, mut wi) = stream.split();
                let (mut ro, mut wo) = upstream.split();
                let _ = tokio::try_join!(
                    tokio::io::copy(&mut ri, &mut wo),
                    tokio::io::copy(&mut ro, &mut wi)
                );
                Ok(())
            }
        }

        #[tokio::test]
        async fn tor_policy_downloads_and_verifies_identity_through_socks() {
            let http_fixture = HttpFixture::start("200 OK", "", PAYLOAD).await;
            let socks_fixture = SocksFixture::start_splicing(http_fixture.addr).await;
            let scratch = ScratchDir::new("tor_parity");
            let destination = scratch.path("model.bin");

            let tor_policy = TransportPolicy::Tor(
                TorTransportConfig::new(&format!("socks5h://{}", socks_fixture.addr)).unwrap(),
            );
            let identities = vec![ArtifactIdentity {
                kind: ArtifactIdentityKind::LfsSha256,
                value: sha256_of(PAYLOAD),
            }];
            let binding = ArtifactDownloadBinding {
                artifact: artifact(Some(PAYLOAD.len() as u64), identities),
                resolved_url: http_fixture.url(),
                transport: tor_policy,
                headers: None,
            };

            let acquired = execute(&binding, &destination).await.unwrap();

            assert_eq!(acquired.bytes_written, PAYLOAD.len() as u64);
            assert_eq!(acquired.sha256_hex, sha256_of(PAYLOAD));
            assert_eq!(
                acquired.identity_verifications[0].verification,
                ArtifactIdentityVerification::Verified
            );
            assert_eq!(std::fs::read(&destination).unwrap(), PAYLOAD);
        }
    }
}
