//! CLI adapter for Spider's canonical artifact-download binding/execution
//! seam (`SCORPION_CANONICAL_ARTIFACT_DOWNLOAD_CLI_SURFACE_001`).
//!
//! This module owns no HTTP client, redirect logic, SSRF validation, hash
//! computation, LFS/Xet verification, size validation, or temp-file/cleanup
//! logic of its own — every one of those already exists exactly once, in
//! `spider::features::artifact_download_binding` (`bind`) and
//! `spider::features::artifact_download_execution` (`execute`), and this
//! module only translates CLI parameters into a call through that seam and
//! projects its truthful result to JSON.
//!
//! Owner security decisions this command enforces by construction, not by
//! any check of its own:
//! - the destination is always the exact caller-supplied path — no
//!   provider filename, directory default, or archive extraction is ever
//!   derived or performed here;
//! - no-overwrite and the streaming byte ceiling are both enforced inside
//!   the canonical `execute()` seam itself, never re-implemented here as a
//!   pre-check or post-download validation;
//! - `--max-bytes` is a required argument — there is no unbounded CLI
//!   download path.
//!
//! Input authority: this command consumes a canonical
//! [`ArtifactReference`], deserialized from a JSON file, and constructs no
//! provider-specific metadata of its own. The expected file content is
//! exactly one element of the `"artifacts"` array `scorpion
//! hugging-face-artifacts` already prints — `ArtifactReference`'s own
//! `Deserialize` impl (behind the `serde` feature this command's Cargo
//! feature forwards to via `spider/evidence`) defines that shape, not this
//! module.

use spider::features::artifact_download_binding::{self, ArtifactDownloadBindingError};
use spider::features::artifact_download_execution::{
    self, ArtifactDownloadExecutionError, ArtifactIdentityVerification,
};
use spider::features::artifact_reference::ArtifactReference;
use spider::features::transport::TransportPolicy;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct ArtifactDownloadParams {
    /// Path to a JSON file holding exactly one serialized
    /// [`ArtifactReference`] — e.g. one element of the `"artifacts"` array
    /// `scorpion hugging-face-artifacts` prints, saved verbatim.
    pub reference_file: String,
    /// Exact destination file path, chosen by the operator. Never a
    /// directory, never derived from provider metadata.
    pub destination: String,
    /// Operator-controlled maximum byte budget, enforced while streaming
    /// by the canonical execution seam. Required — there is no unbounded
    /// download path.
    pub max_bytes: u64,
}

fn verification_label(verification: ArtifactIdentityVerification) -> &'static str {
    match verification {
        ArtifactIdentityVerification::Verified => "verified",
        ArtifactIdentityVerification::NotLocallyVerified => "not_locally_verified",
    }
}

fn binding_error_message(error: ArtifactDownloadBindingError) -> String {
    error.to_string()
}

fn execution_error_message(error: ArtifactDownloadExecutionError) -> String {
    error.to_string()
}

pub async fn run(params: ArtifactDownloadParams) -> Result<String, String> {
    let raw = std::fs::read_to_string(&params.reference_file).map_err(|error| {
        format!(
            "failed to read reference file \"{}\": {error}",
            params.reference_file
        )
    })?;
    let artifact: ArtifactReference = serde_json::from_str(&raw).map_err(|error| {
        format!(
            "reference file \"{}\" is not a valid ArtifactReference: {error}",
            params.reference_file
        )
    })?;

    let provider_id = artifact.provider_id.as_str().to_string();
    let repository_id = artifact.repository_id.clone();
    let path = artifact.path.clone();
    let requested_revision = artifact.requested_revision.clone();
    let resolved_revision = artifact.resolved_revision.clone();
    let provider_declared_size_bytes = artifact.size_bytes;

    let binding = artifact_download_binding::bind(artifact, TransportPolicy::Default, None)
        .map_err(binding_error_message)?;

    let destination = Path::new(&params.destination);
    let acquired =
        artifact_download_execution::execute(&binding, destination, Some(params.max_bytes))
            .await
            .map_err(execution_error_message)?;

    let identity_verifications: Vec<serde_json::Value> = acquired
        .identity_verifications
        .iter()
        .map(|verified| {
            serde_json::json!({
                "kind": verified.kind.as_label(),
                "declared_value": verified.declared_value,
                "verification": verification_label(verified.verification),
            })
        })
        .collect();

    let output = serde_json::json!({
        "provider_id": provider_id,
        "repository_id": repository_id,
        "path": path,
        "requested_revision": requested_revision,
        "resolved_revision": resolved_revision,
        "destination": acquired.destination,
        "bytes_written": acquired.bytes_written,
        "sha256": acquired.sha256_hex,
        "identity_verifications": identity_verifications,
        "provider_declared_size_bytes": provider_declared_size_bytes,
        "content_length_bytes": acquired.declared_content_length,
        "status_code": acquired.status_code,
        "final_url": acquired.final_url.as_str(),
        "content_type": acquired.content_type,
        "success": true,
    });
    serde_json::to_string_pretty(&output).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp_json(name_hint: &str, content: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "scorpion_artifact_download_test_{name_hint}_{}_{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, content).unwrap();
        path
    }

    #[tokio::test]
    async fn missing_reference_file_is_a_truthful_error_never_fabricated_output() {
        let error = run(ArtifactDownloadParams {
            reference_file: "/nonexistent/does-not-exist.json".to_string(),
            destination: "/tmp/does-not-matter.bin".to_string(),
            max_bytes: 1024,
        })
        .await
        .unwrap_err();
        assert!(error.contains("failed to read reference file"));
    }

    #[tokio::test]
    async fn malformed_reference_json_is_a_truthful_error() {
        let reference_file = write_temp_json("malformed", "{ not json");
        let error = run(ArtifactDownloadParams {
            reference_file: reference_file.to_string_lossy().to_string(),
            destination: "/tmp/does-not-matter.bin".to_string(),
            max_bytes: 1024,
        })
        .await
        .unwrap_err();
        assert!(error.contains("not a valid ArtifactReference"));
        let _ = std::fs::remove_file(reference_file);
    }

    #[tokio::test]
    async fn missing_download_url_fails_closed_through_the_canonical_binding_seam() {
        let reference_file = write_temp_json(
            "no_download_url",
            r#"{"provider_id":"test","repository_id":"owner/repo","path":"README.md"}"#,
        );
        let error = run(ArtifactDownloadParams {
            reference_file: reference_file.to_string_lossy().to_string(),
            destination: "/tmp/does-not-matter.bin".to_string(),
            max_bytes: 1024,
        })
        .await
        .unwrap_err();
        assert!(error.contains("no resolved download URL"));
        let _ = std::fs::remove_file(reference_file);
    }

    #[tokio::test]
    async fn non_http_download_url_fails_closed_through_the_canonical_binding_seam() {
        let reference_file = write_temp_json(
            "ftp_url",
            r#"{"provider_id":"test","repository_id":"owner/repo","path":"README.md","download_url":"ftp://downloads.example/model.bin"}"#,
        );
        let error = run(ArtifactDownloadParams {
            reference_file: reference_file.to_string_lossy().to_string(),
            destination: "/tmp/does-not-matter.bin".to_string(),
            max_bytes: 1024,
        })
        .await
        .unwrap_err();
        assert!(error.contains("unsupported artifact download URL scheme"));
        let _ = std::fs::remove_file(reference_file);
    }

    #[tokio::test]
    async fn credential_bearing_url_fails_closed_through_the_canonical_binding_seam() {
        let reference_file = write_temp_json(
            "credentials",
            r#"{"provider_id":"test","repository_id":"owner/repo","path":"README.md","download_url":"https://user:pass@downloads.example/model.bin"}"#,
        );
        let error = run(ArtifactDownloadParams {
            reference_file: reference_file.to_string_lossy().to_string(),
            destination: "/tmp/does-not-matter.bin".to_string(),
            max_bytes: 1024,
        })
        .await
        .unwrap_err();
        assert!(error.contains("credentials are not allowed"));
        let _ = std::fs::remove_file(reference_file);
    }
}
