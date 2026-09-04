//! Provider-neutral metadata identifying a retrievable, versioned artifact.
//!
//! An [`ArtifactReference`] records provider-declared identity and location
//! metadata only. It is not acquired content, verified evidence, or proof that
//! any recorded identity matches bytes subsequently downloaded.

use crate::features::source_provider::ProviderId;

/// Namespace of one provider-declared artifact identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ArtifactIdentityKind {
    /// Git blob object ID declared by the provider.
    GitBlobOid,
    /// Git LFS SHA-256 content identity declared by the provider.
    LfsSha256,
    /// Xet storage identity declared by the provider.
    XetHash,
}

impl ArtifactIdentityKind {
    /// Stable snake_case label — matches this type's own `serde` wire
    /// representation exactly (`rename_all = "snake_case"` above) — for
    /// CLI/display use regardless of whether the `serde` feature is
    /// active. Single source of truth for both `spider_cli`'s
    /// `hugging-face-artifacts` and `artifact-download` commands, which
    /// otherwise have no reason to share a feature gate with each other
    /// (`SCORPION_CANONICAL_ARTIFACT_DOWNLOAD_CLI_SURFACE_001`).
    pub fn as_label(self) -> &'static str {
        match self {
            Self::GitBlobOid => "git_blob_oid",
            Self::LfsSha256 => "lfs_sha256",
            Self::XetHash => "xet_hash",
        }
    }
}

/// One labeled provider-native artifact identity.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ArtifactIdentity {
    /// Identity namespace; never inferred from the value.
    pub kind: ArtifactIdentityKind,
    /// Provider-declared value, retained without local verification claims.
    pub value: String,
}

/// Metadata identifying one retrievable repository artifact.
///
/// Construction and storage are pure metadata operations. This type performs
/// no network, filesystem, parsing, transport, or verification work.
///
/// `Serialize`/`Deserialize` (behind the `serde` feature) are this type's
/// own canonical wire format
/// (`SCORPION_CANONICAL_ARTIFACT_DOWNLOAD_CLI_SURFACE_001`) — field names
/// and the `identities` -> `declared_identities` rename and
/// `ArtifactIdentityKind`'s `snake_case` labels deliberately match the
/// JSON shape `spider_cli`'s `hugging-face-artifacts` discovery command
/// already prints per artifact, so a real discovery result's artifact
/// object can be saved to a file and read back as this exact type with no
/// translation layer. `provider_id`/`repository_id`/`path` are load-bearing
/// identity and stay required on deserialize — a reference file missing
/// one of those fails closed with a parse error rather than silently
/// becoming an empty string. Every other field is genuinely optional
/// metadata and defaults to absent when its key is missing.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ArtifactReference {
    /// Stable machine-readable provider identity.
    pub provider_id: ProviderId,
    /// Provider-native repository identity, independent of any URL.
    pub repository_id: String,
    /// Repository-relative path retained exactly as declared.
    pub path: String,
    /// Caller-requested branch, tag, commit, or other revision, when supplied.
    /// This value may be mutable and is never treated as resolved identity.
    #[cfg_attr(feature = "serde", serde(default))]
    pub requested_revision: Option<String>,
    /// Immutable provider-resolved revision, when genuinely supplied.
    #[cfg_attr(feature = "serde", serde(default))]
    pub resolved_revision: Option<String>,
    /// Provider-declared file size in bytes, when supplied.
    #[cfg_attr(feature = "serde", serde(default))]
    pub size_bytes: Option<u64>,
    /// Distinct labeled provider-native identities. These are recorded claims,
    /// not locally verified checksums.
    #[cfg_attr(feature = "serde", serde(default, rename = "declared_identities"))]
    pub identities: Vec<ArtifactIdentity>,
    /// Provider download location, when one is available. The URL is not the
    /// canonical artifact identity and must never carry credentials.
    #[cfg_attr(feature = "serde", serde(default))]
    pub download_url: Option<String>,
    /// Exact provider document/API URL that declared this reference, when
    /// available. Provenance is separate from repository/artifact identity.
    #[cfg_attr(feature = "serde", serde(default))]
    pub discovered_via: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference() -> ArtifactReference {
        ArtifactReference {
            provider_id: ProviderId::from("provider"),
            repository_id: "owner/repository".to_string(),
            path: "weights/model.gguf".to_string(),
            requested_revision: Some("main".to_string()),
            resolved_revision: Some("34abcdef".to_string()),
            size_bytes: Some(u64::MAX),
            identities: vec![
                ArtifactIdentity {
                    kind: ArtifactIdentityKind::GitBlobOid,
                    value: "git-oid".to_string(),
                },
                ArtifactIdentity {
                    kind: ArtifactIdentityKind::LfsSha256,
                    value: "lfs-sha256".to_string(),
                },
                ArtifactIdentity {
                    kind: ArtifactIdentityKind::XetHash,
                    value: "xet-hash".to_string(),
                },
            ],
            download_url: Some(
                "https://provider.example/owner/repository/resolve/main/weights/model.gguf"
                    .to_string(),
            ),
            discovered_via: Some(
                "https://provider.example/api/repositories/owner/repository/tree/main".to_string(),
            ),
        }
    }

    #[test]
    fn preserves_provider_repository_path_and_revision_semantics() {
        let artifact = reference();

        assert_eq!(artifact.provider_id, ProviderId::from("provider"));
        assert_eq!(artifact.repository_id, "owner/repository");
        assert_eq!(artifact.path, "weights/model.gguf");
        assert_eq!(artifact.requested_revision.as_deref(), Some("main"));
        assert_eq!(artifact.resolved_revision.as_deref(), Some("34abcdef"));
        assert_ne!(artifact.requested_revision, artifact.resolved_revision);
    }

    #[test]
    fn preserves_size_and_distinct_identity_namespaces() {
        let artifact = reference();

        assert_eq!(artifact.size_bytes, Some(u64::MAX));
        assert_eq!(artifact.identities.len(), 3);
        assert_eq!(
            artifact.identities[0].kind,
            ArtifactIdentityKind::GitBlobOid
        );
        assert_eq!(artifact.identities[1].kind, ArtifactIdentityKind::LfsSha256);
        assert_eq!(artifact.identities[2].kind, ArtifactIdentityKind::XetHash);
    }

    #[test]
    fn absence_stays_absent_without_synthetic_identity() {
        let artifact = ArtifactReference {
            provider_id: ProviderId::from("provider"),
            repository_id: "repository".to_string(),
            path: "README.md".to_string(),
            requested_revision: Some("main".to_string()),
            resolved_revision: None,
            size_bytes: None,
            identities: Vec::new(),
            download_url: None,
            discovered_via: None,
        };

        assert_eq!(artifact.resolved_revision, None);
        assert_eq!(artifact.size_bytes, None);
        assert!(artifact.identities.is_empty());
        assert_eq!(artifact.download_url, None);
        assert_eq!(artifact.discovered_via, None);
    }

    // SCORPION_CANONICAL_ARTIFACT_DOWNLOAD_CLI_SURFACE_001: this type's own
    // Serialize/Deserialize (behind the `serde` feature this test module
    // does not otherwise gate on, since these tests only compile when the
    // crate itself is built with `serde` active) must round-trip exactly,
    // and must parse the precise JSON shape the already-shipped
    // `hugging-face-artifacts` CLI command prints per artifact — that
    // command's output is the reference-file input this frontier's own
    // CLI command consumes, with no translation layer in between.
    #[cfg(feature = "serde")]
    #[test]
    fn round_trips_through_json_exactly() {
        let original = reference();
        let json = serde_json::to_string(&original).unwrap();
        let restored: ArtifactReference = serde_json::from_str(&json).unwrap();
        assert_eq!(original, restored);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn deserializes_the_exact_shape_hugging_face_artifacts_cli_already_prints() {
        // Verbatim shape of one element of that command's `"artifacts"`
        // array -- see spider_cli/src/hugging_face_artifacts.rs's own
        // `artifact_to_json`. Field names, the `declared_identities`
        // label, and the snake_case `kind` values are not incidental.
        let json = serde_json::json!({
            "provider_id": "hugging_face",
            "repository_id": "owner/repo",
            "path": "weights/model.safetensors",
            "requested_revision": null,
            "resolved_revision": null,
            "size_bytes": 123,
            "declared_identities": [
                { "kind": "git_blob_oid", "value": "abc123" }
            ],
            "download_url": null,
            "discovered_via": "http://example.test/api/models/owner/repo/tree/main"
        })
        .to_string();

        let artifact: ArtifactReference = serde_json::from_str(&json).unwrap();
        assert_eq!(artifact.provider_id, ProviderId::from("hugging_face"));
        assert_eq!(artifact.repository_id, "owner/repo");
        assert_eq!(artifact.path, "weights/model.safetensors");
        assert_eq!(artifact.size_bytes, Some(123));
        assert_eq!(artifact.identities.len(), 1);
        assert_eq!(
            artifact.identities[0].kind,
            ArtifactIdentityKind::GitBlobOid
        );
        assert_eq!(artifact.identities[0].value, "abc123");
        assert_eq!(artifact.download_url, None);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn missing_required_identity_field_fails_closed_instead_of_defaulting() {
        // `path` is load-bearing identity, not optional metadata -- it must
        // never silently become an empty string when a hand-authored
        // reference file omits it.
        let json = serde_json::json!({
            "provider_id": "hugging_face",
            "repository_id": "owner/repo",
        })
        .to_string();
        assert!(serde_json::from_str::<ArtifactReference>(&json).is_err());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn optional_fields_default_to_absent_when_keys_are_omitted() {
        let json = serde_json::json!({
            "provider_id": "hugging_face",
            "repository_id": "owner/repo",
            "path": "README.md",
        })
        .to_string();
        let artifact: ArtifactReference = serde_json::from_str(&json).unwrap();
        assert_eq!(artifact.resolved_revision, None);
        assert_eq!(artifact.size_bytes, None);
        assert!(artifact.identities.is_empty());
        assert_eq!(artifact.download_url, None);
    }

    #[test]
    fn url_and_provenance_are_not_artifact_identity() {
        let mut first = reference();
        let mut second = first.clone();
        second.download_url = Some("https://mirror.example/model.gguf".to_string());
        second.discovered_via = Some("https://mirror.example/api/tree".to_string());

        first.download_url = None;
        first.discovered_via = None;
        second.download_url = None;
        second.discovered_via = None;
        assert_eq!(first, second);
    }
}
