//! CLI adapter for Spider's canonical Hugging Face artifact-discovery
//! provider (`SCORPION_CANONICAL_HUGGING_FACE_ARTIFACT_DISCOVERY_CLI_SURFACE_001`).
//!
//! This module only translates CLI parameters and the canonical
//! `discover_artifacts()` result to JSON. It owns no Hugging Face API
//! implementation, transport, authentication, parsing, or revision
//! resolution of its own — every one of those already exists exactly once,
//! in `spider::features::hugging_face_source_provider`, and this module
//! calls that seam rather than reassembling it. It never fetches, binds, or
//! downloads any discovered artifact: this is a discovery-only, read-only
//! command with no filesystem write of any kind.
//!
//! Transport is always [`spider::features::transport::TransportPolicy::Default`]
//! — no transport/Tor selection is exposed here, since
//! `HuggingFaceArtifactDiscoveryRequest` itself carries no transport field
//! (transport is a provider-builder property, not part of the canonical
//! request); this command's own scope is limited to what that request
//! truthfully supports (`repository_id`, optional `requested_revision`,
//! bounded `limit`).

use spider::features::artifact_reference::{ArtifactIdentityKind, ArtifactReference};
use spider::features::hugging_face_source_provider::{
    HuggingFaceArtifactDiscoveryRequest, HuggingFaceModelProvider,
};
use spider::features::source_provider::ProviderDiscovery;

#[derive(Clone, Debug)]
pub struct HuggingFaceArtifactsParams {
    pub repository_id: String,
    pub revision: Option<String>,
    pub limit: Option<usize>,
}

fn identity_kind_label(kind: ArtifactIdentityKind) -> &'static str {
    match kind {
        ArtifactIdentityKind::GitBlobOid => "git_blob_oid",
        ArtifactIdentityKind::LfsSha256 => "lfs_sha256",
        ArtifactIdentityKind::XetHash => "xet_hash",
    }
}

/// Project one canonical [`ArtifactReference`] into JSON, verbatim — every
/// field copied as-is, `null` where the canonical value is `None`, never
/// invented or reinterpreted. `identities` are labeled `declared_identities`
/// to keep this command's own output honest about what
/// `spider::features::artifact_reference`'s own doc comment already states:
/// "recorded claims, not locally verified checksums" — this command
/// performs no download and therefore no local verification of any kind.
fn artifact_to_json(artifact: ArtifactReference) -> serde_json::Value {
    serde_json::json!({
        "provider_id": artifact.provider_id.as_str(),
        "repository_id": artifact.repository_id,
        "path": artifact.path,
        "requested_revision": artifact.requested_revision,
        "resolved_revision": artifact.resolved_revision,
        "size_bytes": artifact.size_bytes,
        "declared_identities": artifact.identities.into_iter().map(|identity| {
            serde_json::json!({
                "kind": identity_kind_label(identity.kind),
                "value": identity.value,
            })
        }).collect::<Vec<_>>(),
        "download_url": artifact.download_url,
        "discovered_via": artifact.discovered_via,
    })
}

pub async fn run(params: HuggingFaceArtifactsParams) -> Result<String, String> {
    let mut request = HuggingFaceArtifactDiscoveryRequest::new(params.repository_id.clone());
    if let Some(revision) = params.revision.clone() {
        request = request.with_revision(revision);
    }
    if let Some(limit) = params.limit {
        request = request.with_limit(limit);
    }

    let provider = HuggingFaceModelProvider::new();
    let discoveries = provider
        .discover_artifacts(&request)
        .await
        .map_err(|error| error.to_string())?;

    let artifacts: Vec<serde_json::Value> = discoveries
        .into_iter()
        .filter_map(|discovery| match discovery {
            ProviderDiscovery::Artifact(artifact) => Some(artifact_to_json(artifact)),
            // discover_artifacts only ever constructs the Artifact variant
            // in production; any other variant is treated as truthfully
            // absent rather than fabricated into a shape this command
            // never promised.
            ProviderDiscovery::Item(_) | ProviderDiscovery::Target(_) => None,
        })
        .collect();

    let output = serde_json::json!({
        "provider": "hugging_face",
        "repository_id": params.repository_id,
        "requested_revision": params.revision,
        "artifact_count": artifacts.len(),
        "artifacts": artifacts,
    });
    serde_json::to_string_pretty(&output).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(repository_id: &str) -> HuggingFaceArtifactsParams {
        HuggingFaceArtifactsParams {
            repository_id: repository_id.to_string(),
            revision: None,
            limit: None,
        }
    }

    /// `spider::features::hugging_face_source_provider::HuggingFaceModelProvider`
    /// has no test-only seam for pointing `api_base` at a local fixture from
    /// outside its own crate, so this module's tests prove the two parts it
    /// actually owns instead: real request-validation passthrough (the
    /// canonical seam's own `validate_artifact_request`, exercised through
    /// this module's real `run()`, before any network call) and JSON
    /// projection correctness. The real network success path is proven
    /// separately by a real `scorpion hugging-face-artifacts` invocation
    /// against the public Hub (see this frontier's own closure report), not
    /// faked here.
    #[test]
    fn artifact_to_json_preserves_null_fields_verbatim_and_labels_declared_identities() {
        let artifact = ArtifactReference {
            provider_id: spider::features::source_provider::ProviderId::from("hugging_face"),
            repository_id: "owner/repo".to_string(),
            path: "weights/model.safetensors".to_string(),
            requested_revision: None,
            resolved_revision: None,
            size_bytes: Some(123),
            identities: vec![spider::features::artifact_reference::ArtifactIdentity {
                kind: ArtifactIdentityKind::GitBlobOid,
                value: "abc123".to_string(),
            }],
            download_url: None,
            discovered_via: Some("http://example.test/api/models/owner/repo/tree/main".into()),
        };
        let json = artifact_to_json(artifact);
        assert!(json["resolved_revision"].is_null());
        assert!(json["download_url"].is_null());
        assert!(json["requested_revision"].is_null());
        assert_eq!(json["size_bytes"], 123);
        assert_eq!(json["declared_identities"][0]["kind"], "git_blob_oid");
        assert_eq!(json["declared_identities"][0]["value"], "abc123");
        // Never present as a claim of verification anywhere in this output.
        let serialized = json.to_string();
        assert!(!serialized.contains("verified"));
    }

    #[tokio::test]
    async fn empty_repository_id_is_a_truthful_provider_error_never_fabricated_output() {
        let error = run(params("")).await.unwrap_err();
        assert!(error.contains("repository ID"));
    }

    #[tokio::test]
    async fn out_of_range_limit_is_a_truthful_provider_error() {
        let mut input = params("owner/repo");
        input.limit = Some(0);
        let error = run(input).await.unwrap_err();
        assert!(error.contains("between 1 and 100"));
    }
}
