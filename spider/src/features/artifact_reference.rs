//! Provider-neutral metadata identifying a retrievable, versioned artifact.
//!
//! An [`ArtifactReference`] records provider-declared identity and location
//! metadata only. It is not acquired content, verified evidence, or proof that
//! any recorded identity matches bytes subsequently downloaded.

use crate::features::source_provider::ProviderId;

/// Namespace of one provider-declared artifact identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtifactIdentityKind {
    /// Git blob object ID declared by the provider.
    GitBlobOid,
    /// Git LFS SHA-256 content identity declared by the provider.
    LfsSha256,
    /// Xet storage identity declared by the provider.
    XetHash,
}

/// One labeled provider-native artifact identity.
#[derive(Clone, Debug, PartialEq, Eq)]
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactReference {
    /// Stable machine-readable provider identity.
    pub provider_id: ProviderId,
    /// Provider-native repository identity, independent of any URL.
    pub repository_id: String,
    /// Repository-relative path retained exactly as declared.
    pub path: String,
    /// Caller-requested branch, tag, commit, or other revision, when supplied.
    /// This value may be mutable and is never treated as resolved identity.
    pub requested_revision: Option<String>,
    /// Immutable provider-resolved revision, when genuinely supplied.
    pub resolved_revision: Option<String>,
    /// Provider-declared file size in bytes, when supplied.
    pub size_bytes: Option<u64>,
    /// Distinct labeled provider-native identities. These are recorded claims,
    /// not locally verified checksums.
    pub identities: Vec<ArtifactIdentity>,
    /// Provider download location, when one is available. The URL is not the
    /// canonical artifact identity and must never carry credentials.
    pub download_url: Option<String>,
    /// Exact provider document/API URL that declared this reference, when
    /// available. Provenance is separate from repository/artifact identity.
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
