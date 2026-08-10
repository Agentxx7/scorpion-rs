//! Pure binding from provider-neutral artifact metadata to future download
//! execution intent.
//!
//! This module performs no acquisition. It only requires an already-resolved
//! HTTP(S) download location, validates that location against the canonical
//! transport policy, and keeps ephemeral request headers separate from
//! artifact identity.

use crate::features::artifact_reference::ArtifactReference;
use crate::features::secret_request_headers::SecretRequestHeaders;
use crate::features::transport::{self, TransportError, TransportPolicy};

/// Provider-neutral instructions sufficient for a future artifact downloader.
pub struct ArtifactDownloadBinding {
    /// Canonical provider-declared artifact metadata, consumed without mutation.
    pub artifact: ArtifactReference,
    /// Parsed HTTP(S) retrieval location; never canonical artifact identity.
    pub resolved_url: url::Url,
    /// Canonical transport policy a future executor must honor exactly.
    pub transport: TransportPolicy,
    /// Optional ephemeral, non-serializable request headers.
    pub headers: Option<SecretRequestHeaders>,
}

impl Clone for ArtifactDownloadBinding {
    fn clone(&self) -> Self {
        Self {
            artifact: self.artifact.clone(),
            resolved_url: self.resolved_url.clone(),
            transport: self.transport.clone(),
            headers: self.headers.clone(),
        }
    }
}

impl std::fmt::Debug for ArtifactDownloadBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ArtifactDownloadBinding")
            .field("has_headers", &self.headers.is_some())
            .finish_non_exhaustive()
    }
}

/// Deterministic, secret-safe artifact binding failures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArtifactDownloadBindingError {
    /// Provider resolution has not supplied a retrievable location.
    MissingDownloadUrl,
    /// The supplied location is not parseable. The raw value is omitted.
    InvalidDownloadUrl,
    /// The future canonical downloader supports HTTP(S) request targets only.
    UnsupportedScheme(String),
    /// URL userinfo is forbidden; credentials belong in ephemeral headers.
    CredentialsNotAllowed,
    /// The URL is incompatible with the selected canonical transport policy.
    Transport(TransportError),
}

impl std::fmt::Display for ArtifactDownloadBindingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingDownloadUrl => {
                formatter.write_str("artifact has no resolved download URL")
            }
            Self::InvalidDownloadUrl => formatter.write_str("artifact download URL is invalid"),
            Self::UnsupportedScheme(scheme) => write!(
                formatter,
                "unsupported artifact download URL scheme \"{scheme}\": only http and https are accepted"
            ),
            Self::CredentialsNotAllowed => formatter.write_str(
                "artifact download URL credentials are not allowed; use ephemeral request headers",
            ),
            Self::Transport(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for ArtifactDownloadBindingError {}

/// Bind an already-resolved artifact to future download execution intent.
///
/// This consumes the artifact and performs only local parsing and validation:
/// no request, DNS lookup, filesystem access, redirect, retry, or verification
/// occurs. Provider-specific URL resolution must happen before this call.
pub fn bind(
    artifact: ArtifactReference,
    transport: TransportPolicy,
    headers: Option<SecretRequestHeaders>,
) -> Result<ArtifactDownloadBinding, ArtifactDownloadBindingError> {
    let raw_url = artifact
        .download_url
        .as_deref()
        .ok_or(ArtifactDownloadBindingError::MissingDownloadUrl)?;
    let resolved_url =
        url::Url::parse(raw_url).map_err(|_| ArtifactDownloadBindingError::InvalidDownloadUrl)?;

    match resolved_url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(ArtifactDownloadBindingError::UnsupportedScheme(
                scheme.to_string(),
            ));
        }
    }
    if !resolved_url.username().is_empty() || resolved_url.password().is_some() {
        return Err(ArtifactDownloadBindingError::CredentialsNotAllowed);
    }

    transport::validate_target(&resolved_url, &transport)
        .map_err(ArtifactDownloadBindingError::Transport)?;

    Ok(ArtifactDownloadBinding {
        artifact,
        resolved_url,
        transport,
        headers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::artifact_reference::{ArtifactIdentity, ArtifactIdentityKind};
    use crate::features::source_provider::ProviderId;
    use crate::features::transport::TorTransportConfig;
    use reqwest::header::HeaderMap;

    fn artifact(provider: &str, download_url: Option<&str>) -> ArtifactReference {
        ArtifactReference {
            provider_id: ProviderId::from(provider),
            repository_id: "owner/repository".to_string(),
            path: "weights/model.bin".to_string(),
            requested_revision: Some("main".to_string()),
            resolved_revision: Some("immutable-revision".to_string()),
            size_bytes: Some(u64::MAX),
            identities: vec![ArtifactIdentity {
                kind: ArtifactIdentityKind::LfsSha256,
                value: "provider-declared-identity".to_string(),
            }],
            download_url: download_url.map(str::to_string),
            discovered_via: Some("https://provider.example/api/tree".to_string()),
        }
    }

    fn tor_policy() -> TransportPolicy {
        TransportPolicy::Tor(TorTransportConfig::new("socks5h://127.0.0.1:9050").unwrap())
    }

    #[test]
    fn https_default_binding_preserves_artifact_metadata_without_execution() {
        let original = artifact(
            "generic_provider",
            Some("https://downloads.example/weights/model.bin"),
        );
        let binding = bind(original.clone(), TransportPolicy::Default, None).unwrap();

        assert_eq!(binding.artifact, original);
        assert_eq!(
            binding.resolved_url.as_str(),
            "https://downloads.example/weights/model.bin"
        );
        assert_eq!(binding.artifact.size_bytes, Some(u64::MAX));
        assert_eq!(binding.artifact.identities, original.identities);
        assert!(binding.headers.is_none());
    }

    #[test]
    fn missing_invalid_and_unsupported_locations_fail_deterministically() {
        assert_eq!(
            bind(artifact("provider", None), TransportPolicy::Default, None).unwrap_err(),
            ArtifactDownloadBindingError::MissingDownloadUrl
        );
        assert_eq!(
            bind(
                artifact("provider", Some("not a URL")),
                TransportPolicy::Default,
                None,
            )
            .unwrap_err(),
            ArtifactDownloadBindingError::InvalidDownloadUrl
        );
        for scheme in ["ftp", "file", "data"] {
            let raw = match scheme {
                "file" => "file:///tmp/model.bin".to_string(),
                "data" => "data:,model".to_string(),
                _ => format!("{scheme}://downloads.example/model.bin"),
            };
            assert_eq!(
                bind(
                    artifact("provider", Some(&raw)),
                    TransportPolicy::Default,
                    None,
                )
                .unwrap_err(),
                ArtifactDownloadBindingError::UnsupportedScheme(scheme.to_string())
            );
        }
    }

    #[test]
    fn url_credentials_are_rejected_without_echoing_secrets() {
        const SECRET: &str = "download-password-sentinel";
        let error = bind(
            artifact(
                "provider",
                Some("https://user:download-password-sentinel@downloads.example/model.bin"),
            ),
            TransportPolicy::Default,
            None,
        )
        .unwrap_err();

        assert_eq!(error, ArtifactDownloadBindingError::CredentialsNotAllowed);
        assert!(!format!("{error:?}").contains(SECRET));
        assert!(!error.to_string().contains(SECRET));
    }

    #[test]
    fn canonical_transport_validation_fails_closed_for_onion_default() {
        let error = bind(
            artifact("provider", Some("http://artifact-service.onion/model.bin")),
            TransportPolicy::Default,
            None,
        )
        .unwrap_err();
        assert_eq!(
            error,
            ArtifactDownloadBindingError::Transport(TransportError::OnionRequiresTor)
        );
    }

    #[test]
    fn tor_accepts_onion_and_clearnet_without_networking_or_policy_rewrite() {
        for target in [
            "http://artifact-service.onion/model.bin",
            "https://downloads.example/model.bin",
        ] {
            let binding = bind(artifact("provider", Some(target)), tor_policy(), None).unwrap();
            assert_eq!(binding.resolved_url.as_str(), target);
            assert!(matches!(binding.transport, TransportPolicy::Tor(_)));
        }
    }

    #[test]
    fn secret_headers_remain_ephemeral_sensitive_and_debug_redacted() {
        const SECRET: &str = "Bearer artifact-token-sentinel";
        let mut headers = SecretRequestHeaders::new();
        headers.try_insert("authorization", SECRET).unwrap();
        let binding = bind(
            artifact(
                "provider",
                Some("https://downloads.example/model.bin?signature=opaque"),
            ),
            TransportPolicy::Default,
            Some(headers),
        )
        .unwrap();

        let debug = format!("{binding:?}");
        assert!(!debug.contains(SECRET));
        assert!(!debug.contains("signature=opaque"));
        let mut applied = HeaderMap::new();
        binding.headers.as_ref().unwrap().apply_to(&mut applied);
        assert!(applied.get("authorization").unwrap().is_sensitive());
        assert_eq!(
            applied.get("authorization").unwrap().as_bytes(),
            SECRET.as_bytes()
        );
    }

    #[test]
    fn provider_identity_never_changes_binding_rules() {
        for provider in ["hugging_face", "github", "ordinary_web", "future_provider"] {
            let binding = bind(
                artifact(provider, Some("https://downloads.example/model.bin")),
                TransportPolicy::Default,
                None,
            )
            .unwrap();
            assert_eq!(binding.artifact.provider_id, ProviderId::from(provider));
        }
    }

    #[test]
    fn clone_preserves_binding_metadata_and_secret_sensitivity() {
        let mut headers = SecretRequestHeaders::new();
        headers
            .try_insert("cookie", "session=clone-secret")
            .unwrap();
        let binding = bind(
            artifact("provider", Some("https://downloads.example/model.bin")),
            TransportPolicy::Default,
            Some(headers),
        )
        .unwrap();
        let cloned = binding.clone();
        let mut applied = HeaderMap::new();
        cloned.headers.as_ref().unwrap().apply_to(&mut applied);

        assert_eq!(cloned.artifact, binding.artifact);
        assert_eq!(cloned.resolved_url, binding.resolved_url);
        assert!(applied.get("cookie").unwrap().is_sensitive());
    }
}
