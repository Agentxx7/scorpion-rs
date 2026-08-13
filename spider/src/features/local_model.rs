//! Canonical provider-neutral contract for immutable local AI model installs.
//!
//! This module performs no acquisition and has no inference dependency. Each
//! [`LocalModelArtifact`] refers to the existing canonical artifact vocabulary;
//! callers acquire every member through that seam into a sibling staging
//! directory. This module then verifies the complete set and atomically renames
//! the verified directory into its active location. Runtime adapters may only
//! consume the resulting [`LocalModelInstallation`].

use crate::features::artifact_reference::ArtifactReference;
use crate::features::captcha::CaptchaChallengeKind;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const IDENTITY_FILE: &str = ".scorpion-local-model-identity-v1";

/// Exact immutable identity of a model artifact set.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LocalModelIdentity {
    /// Provider or publishing organization.
    pub provider: String,
    /// Provider-native repository identity.
    pub repository: String,
    /// Stable model name within the repository.
    pub model: String,
    /// Immutable provider-resolved revision; never `latest`, a branch, or tag.
    pub revision: String,
}

/// One required member of an immutable model installation.
#[derive(Clone, Debug)]
pub struct LocalModelArtifact {
    /// Existing provider-neutral acquisition metadata.
    pub reference: ArtifactReference,
    /// Safe path relative to the installation root.
    pub relative_path: PathBuf,
    /// Mandatory exact byte length.
    pub size_bytes: u64,
    /// Mandatory lowercase or uppercase hexadecimal SHA-256 of file bytes.
    pub sha256: String,
}

/// Runtime device class. Availability is established by an adapter preflight,
/// not inferred from compilation or host naming.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LocalModelDevice {
    /// Host CPU execution.
    Cpu,
    /// NVIDIA CUDA execution.
    Cuda,
    /// Apple Metal execution.
    Metal,
}

/// Declared minimum resources for one pinned runtime/model combination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalModelRuntimeRequirements {
    /// Stable runtime adapter identity and version contract.
    pub runtime: String,
    /// Exact preprocessing contract identity.
    pub preprocessing: String,
    /// Minimum host RAM required before initialization.
    pub minimum_ram_bytes: u64,
    /// Device-specific support and memory declarations.
    pub devices: Vec<LocalModelDeviceRequirement>,
}

/// Requirements for one explicitly supported runtime device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalModelDeviceRequirement {
    /// Supported device.
    pub device: LocalModelDevice,
    /// Minimum device-local memory when applicable. CPU runtimes normally use
    /// `None` and rely on [`LocalModelRuntimeRequirements::minimum_ram_bytes`].
    pub minimum_device_memory_bytes: Option<u64>,
}

/// Explicit operator device selection. Fallbacks are attempted only in this
/// declared order; an empty list means no fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalModelDevicePolicy {
    /// Required first choice.
    pub primary: LocalModelDevice,
    /// Operator-authorized fallback order.
    pub fallbacks: Vec<LocalModelDevice>,
}

impl LocalModelDevicePolicy {
    /// Ordered choices without implicit host-driven additions.
    pub fn choices(&self) -> impl Iterator<Item = LocalModelDevice> + '_ {
        std::iter::once(self.primary).chain(self.fallbacks.iter().copied())
    }
}

/// Empirical qualification for one exact model/runtime/preprocessing tuple.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalModelQualification {
    /// CAPTCHA task evaluated; kinds remain independently qualified.
    pub challenge_kind: CaptchaChallengeKind,
    /// Immutable evaluation suite identity.
    pub evaluation_id: String,
    /// SHA-256 of the pinned evaluation definition/results record.
    pub evaluation_sha256: String,
    /// Explicit passing threshold description or versioned policy identity.
    pub passing_policy: String,
}

/// Complete immutable model manifest.
#[derive(Clone, Debug)]
pub struct LocalModelManifest {
    /// Exact model identity shared by every member.
    pub identity: LocalModelIdentity,
    /// Complete required-file set.
    pub artifacts: Vec<LocalModelArtifact>,
    /// Exact runtime and preprocessing requirements.
    pub runtime_requirements: LocalModelRuntimeRequirements,
    /// Independently pinned empirical qualifications.
    pub qualifications: Vec<LocalModelQualification>,
}

/// Durable identity written into and returned from a verified installation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledModelIdentity {
    /// Exact model identity.
    pub model: LocalModelIdentity,
    /// SHA-256 of the normalized complete manifest.
    pub manifest_sha256: String,
    /// Exact runtime contract.
    pub runtime: String,
    /// Exact preprocessing contract.
    pub preprocessing: String,
}

/// An atomically activated, verified installation suitable for offline-only
/// runtime initialization.
#[derive(Clone, Debug)]
pub struct LocalModelInstallation {
    /// Active installation directory.
    root: PathBuf,
    /// Durable verified identity.
    identity: InstalledModelIdentity,
    /// Exact verified member paths.
    members: HashSet<PathBuf>,
    /// Manifest retained privately so initialization can reverify an
    /// installation instead of trusting an arbitrarily old handle.
    manifest: LocalModelManifest,
}

impl LocalModelInstallation {
    /// Active installation root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Verified installed identity.
    pub fn identity(&self) -> &InstalledModelIdentity {
        &self.identity
    }

    /// Resolve one manifest-declared member path without exposing arbitrary
    /// filesystem traversal. Runtime adapters use this to locate their pinned
    /// weights/configuration members after installation verification.
    pub fn artifact_path(&self, relative_path: &Path) -> Result<PathBuf, LocalModelFailure> {
        if !self.members.contains(relative_path) {
            return Err(LocalModelFailure::InstallationInvalid);
        }
        Ok(self.root.join(relative_path))
    }

    /// Reverify durable identity, membership, sizes and digests immediately
    /// before an adapter initializes its private runtime.
    pub fn reverify(&self) -> Result<(), LocalModelFailure> {
        self.manifest.open_installation(&self.root).map(|_| ())
    }
}

#[cfg(all(feature = "evidence", not(feature = "wreq")))]
impl LocalModelArtifact {
    /// Bind an artifact member to the truthful size and SHA-256 already
    /// observed by canonical streaming acquisition. This performs no second
    /// download and retains the exact acquired [`ArtifactReference`].
    pub fn from_acquired(
        acquired: &crate::features::artifact_download_execution::AcquiredArtifact,
        relative_path: PathBuf,
    ) -> Result<Self, LocalModelFailure> {
        if !safe_relative_path(&relative_path)
            || acquired.bytes_written == 0
            || !valid_sha256(&acquired.sha256_hex)
        {
            return Err(LocalModelFailure::InstallationInvalid);
        }
        Ok(Self {
            reference: acquired.artifact.clone(),
            relative_path,
            size_bytes: acquired.bytes_written,
            sha256: acquired.sha256_hex.clone(),
        })
    }
}

/// Observable lifecycle of one persistent runtime instance. Backend handles
/// remain private to their adapter and are never part of this vocabulary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalModelRuntimeState {
    /// No initialization has been attempted.
    Uninitialized,
    /// Initialization is in progress for the chosen device.
    Initializing {
        /// Explicitly selected device.
        device: LocalModelDevice,
    },
    /// One persistent runtime is ready for reuse.
    Ready {
        /// Verified installation loaded by the adapter.
        installed: InstalledModelIdentity,
        /// Explicit device used by the persistent runtime.
        device: LocalModelDevice,
    },
    /// Initialization or later local execution failed.
    Failed {
        /// Sanitized canonical failure.
        failure: LocalModelFailure,
    },
    /// Runtime resources were explicitly released.
    Unloaded,
}

/// Persistent runtime lifecycle controller. It holds no backend handle and
/// permits initialization only from a verified installation.
#[derive(Clone, Debug)]
pub struct LocalModelRuntimeLifecycle {
    state: LocalModelRuntimeState,
}

impl Default for LocalModelRuntimeLifecycle {
    fn default() -> Self {
        Self {
            state: LocalModelRuntimeState::Uninitialized,
        }
    }
}

impl LocalModelRuntimeLifecycle {
    /// Current observable lifecycle state.
    pub fn state(&self) -> &LocalModelRuntimeState {
        &self.state
    }

    /// Begin offline initialization for an already verified installation.
    pub fn begin_initialization(
        &mut self,
        installation: &LocalModelInstallation,
        device: LocalModelDevice,
    ) -> Result<(), LocalModelFailure> {
        if !matches!(
            self.state,
            LocalModelRuntimeState::Uninitialized | LocalModelRuntimeState::Unloaded
        ) {
            return Err(LocalModelFailure::InitializationFailure);
        }
        installation.reverify()?;
        self.state = LocalModelRuntimeState::Initializing { device };
        Ok(())
    }

    /// Mark a successfully initialized backend handle ready for persistent
    /// reuse. The private handle remains owned by the runtime adapter.
    pub fn mark_ready(
        &mut self,
        installation: &LocalModelInstallation,
    ) -> Result<(), LocalModelFailure> {
        let LocalModelRuntimeState::Initializing { device } = self.state else {
            return Err(LocalModelFailure::InitializationFailure);
        };
        self.state = LocalModelRuntimeState::Ready {
            installed: installation.identity.clone(),
            device,
        };
        Ok(())
    }

    /// Record a sanitized initialization or execution failure.
    pub fn mark_failed(&mut self, failure: LocalModelFailure) {
        self.state = LocalModelRuntimeState::Failed { failure };
    }

    /// Explicitly release the adapter-owned runtime resources.
    pub fn unload(&mut self) {
        self.state = LocalModelRuntimeState::Unloaded;
    }
}

/// Exact resources observed by a runtime adapter during fail-closed preflight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalModelRuntimeResources {
    /// Available host RAM.
    pub available_ram_bytes: u64,
    /// Available device memory for the selected device.
    pub available_device_memory_bytes: Option<u64>,
}

/// Canonical installation/runtime failures; no backend error object or secret
/// material crosses this boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalModelFailure {
    /// Requested active installation does not exist.
    ModelNotInstalled,
    /// Manifest, directory membership, marker, or activation shape is invalid.
    InstallationInvalid,
    /// A required file is absent, not regular, has the wrong size, or has the
    /// wrong SHA-256 digest.
    IntegrityFailure {
        /// Manifest-relative member path.
        path: PathBuf,
    },
    /// A member does not belong to the manifest's immutable revision.
    RevisionMismatch {
        /// Manifest-relative member path.
        path: PathBuf,
    },
    /// The named runtime adapter is not compiled or installed.
    RuntimeUnavailable,
    /// None of the explicitly authorized choices is supported.
    DeviceUnavailable {
        /// Operator's primary requested device.
        device: LocalModelDevice,
    },
    /// Supported device choices exist but none meets declared resources.
    ResourceLimitExceeded,
    /// Persistent runtime initialization or lifecycle transition failed.
    InitializationFailure,
    /// The exact model/runtime/preprocessing tuple lacks a passing pinned
    /// evaluation for the requested challenge kind.
    QualificationMissing {
        /// Independently qualified CAPTCHA task.
        challenge_kind: CaptchaChallengeKind,
    },
}

impl LocalModelManifest {
    /// Validate immutable identity, complete member metadata, runtime contract,
    /// and qualification identities without touching disk or network.
    pub fn validate(&self) -> Result<(), LocalModelFailure> {
        if self.identity.provider.trim().is_empty()
            || self.identity.repository.trim().is_empty()
            || self.identity.model.trim().is_empty()
            || !immutable_revision(&self.identity.revision)
            || self.artifacts.is_empty()
            || identity_component_invalid(&self.identity.provider)
            || identity_component_invalid(&self.identity.repository)
            || identity_component_invalid(&self.identity.model)
            || identity_component_invalid(&self.identity.revision)
            || self.runtime_requirements.runtime.trim().is_empty()
            || self.runtime_requirements.preprocessing.trim().is_empty()
            || identity_component_invalid(&self.runtime_requirements.runtime)
            || identity_component_invalid(&self.runtime_requirements.preprocessing)
            || self.runtime_requirements.devices.is_empty()
        {
            return Err(LocalModelFailure::InstallationInvalid);
        }
        let mut devices = HashSet::new();
        if self
            .runtime_requirements
            .devices
            .iter()
            .any(|requirement| !devices.insert(requirement.device))
        {
            return Err(LocalModelFailure::InstallationInvalid);
        }

        let mut paths = HashSet::new();
        for artifact in &self.artifacts {
            if !safe_relative_path(&artifact.relative_path)
                || artifact.size_bytes == 0
                || !valid_sha256(&artifact.sha256)
                || artifact.reference.path.trim().is_empty()
                || identity_component_invalid(&artifact.reference.path)
                || artifact
                    .reference
                    .size_bytes
                    .is_some_and(|size| size != artifact.size_bytes)
                || artifact.reference.identities.iter().any(|identity| {
                    matches!(
                        identity.kind,
                        crate::features::artifact_reference::ArtifactIdentityKind::LfsSha256
                    ) && !identity.value.eq_ignore_ascii_case(&artifact.sha256)
                })
                || !paths.insert(artifact.relative_path.clone())
            {
                return Err(LocalModelFailure::InstallationInvalid);
            }
            if artifact.reference.provider_id.as_str() != self.identity.provider
                || artifact.reference.repository_id != self.identity.repository
                || artifact.reference.resolved_revision.as_deref()
                    != Some(self.identity.revision.as_str())
            {
                return Err(LocalModelFailure::RevisionMismatch {
                    path: artifact.relative_path.clone(),
                });
            }
        }

        let mut kinds = HashSet::new();
        for qualification in &self.qualifications {
            if !kinds.insert(qualification.challenge_kind)
                || qualification.evaluation_id.trim().is_empty()
                || !valid_sha256(&qualification.evaluation_sha256)
                || qualification.passing_policy.trim().is_empty()
                || identity_component_invalid(&qualification.evaluation_id)
                || identity_component_invalid(&qualification.passing_policy)
            {
                return Err(LocalModelFailure::InstallationInvalid);
            }
        }
        Ok(())
    }

    /// Require empirical qualification for one challenge kind before it may be
    /// advertised by a provider.
    pub fn require_qualification(
        &self,
        kind: CaptchaChallengeKind,
    ) -> Result<&LocalModelQualification, LocalModelFailure> {
        self.qualifications
            .iter()
            .find(|qualification| qualification.challenge_kind == kind)
            .ok_or(LocalModelFailure::QualificationMissing {
                challenge_kind: kind,
            })
    }

    /// Verify every required staging member and atomically activate the whole
    /// directory. `staging` and `active` must be siblings, ensuring rename is
    /// a same-filesystem atomic operation. No acquisition occurs here.
    pub fn activate(
        &self,
        staging: &Path,
        active: &Path,
    ) -> Result<LocalModelInstallation, LocalModelFailure> {
        self.validate()?;
        if !staging.is_dir() || active.exists() || staging.parent() != active.parent() {
            return Err(LocalModelFailure::InstallationInvalid);
        }

        let expected: HashSet<_> = self
            .artifacts
            .iter()
            .map(|artifact| artifact.relative_path.clone())
            .collect();
        let observed = regular_files(staging)?;
        if observed != expected {
            return Err(LocalModelFailure::InstallationInvalid);
        }

        for artifact in &self.artifacts {
            verify_file(staging, artifact)?;
        }
        let identity = self.installed_identity();
        std::fs::write(staging.join(IDENTITY_FILE), identity_record(&identity))
            .map_err(|_| LocalModelFailure::InstallationInvalid)?;
        std::fs::rename(staging, active).map_err(|_| LocalModelFailure::InstallationInvalid)?;
        Ok(LocalModelInstallation {
            root: active.to_path_buf(),
            identity,
            members: expected,
            manifest: self.clone(),
        })
    }

    /// Open an existing installation only after re-verifying its durable
    /// identity and every member. This operation is filesystem-only.
    pub fn open_installation(
        &self,
        active: &Path,
    ) -> Result<LocalModelInstallation, LocalModelFailure> {
        self.validate()?;
        if !active.is_dir() {
            return Err(LocalModelFailure::ModelNotInstalled);
        }
        let identity = self.installed_identity();
        let marker = std::fs::read_to_string(active.join(IDENTITY_FILE))
            .map_err(|_| LocalModelFailure::InstallationInvalid)?;
        if marker != identity_record(&identity) {
            return Err(LocalModelFailure::InstallationInvalid);
        }
        let expected: HashSet<_> = self
            .artifacts
            .iter()
            .map(|artifact| artifact.relative_path.clone())
            .collect();
        if regular_files(active)? != expected {
            return Err(LocalModelFailure::InstallationInvalid);
        }
        for artifact in &self.artifacts {
            verify_file(active, artifact)?;
        }
        Ok(LocalModelInstallation {
            root: active.to_path_buf(),
            identity,
            members: expected,
            manifest: self.clone(),
        })
    }

    fn installed_identity(&self) -> InstalledModelIdentity {
        InstalledModelIdentity {
            model: self.identity.clone(),
            manifest_sha256: manifest_sha256(self),
            runtime: self.runtime_requirements.runtime.clone(),
            preprocessing: self.runtime_requirements.preprocessing.clone(),
        }
    }
}

/// Select the first explicitly authorized and available device and enforce its
/// declared resources. No undeclared fallback is considered.
pub fn preflight_device(
    requirements: &LocalModelRuntimeRequirements,
    policy: &LocalModelDevicePolicy,
    mut inspect: impl FnMut(LocalModelDevice) -> Option<LocalModelRuntimeResources>,
) -> Result<LocalModelDevice, LocalModelFailure> {
    let mut available_choice_observed = false;
    for device in policy.choices() {
        let Some(requirement) = requirements
            .devices
            .iter()
            .find(|requirement| requirement.device == device)
        else {
            continue;
        };
        let Some(resources) = inspect(device) else {
            continue;
        };
        available_choice_observed = true;
        if resources.available_ram_bytes < requirements.minimum_ram_bytes
            || requirement
                .minimum_device_memory_bytes
                .is_some_and(|minimum| {
                    resources.available_device_memory_bytes.unwrap_or(0) < minimum
                })
        {
            continue;
        }
        return Ok(device);
    }
    if available_choice_observed {
        Err(LocalModelFailure::ResourceLimitExceeded)
    } else {
        Err(LocalModelFailure::DeviceUnavailable {
            device: policy.primary,
        })
    }
}

fn immutable_revision(revision: &str) -> bool {
    let value = revision.trim();
    !value.is_empty()
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "latest" | "main" | "master"
        )
}

fn identity_component_invalid(value: &str) -> bool {
    value
        .bytes()
        .any(|byte| byte == 0 || byte == b'\n' || byte == b'\r')
}

fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path.to_str().is_some()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        && path != Path::new(IDENTITY_FILE)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn verify_file(root: &Path, artifact: &LocalModelArtifact) -> Result<(), LocalModelFailure> {
    let path = root.join(&artifact.relative_path);
    let metadata =
        std::fs::symlink_metadata(&path).map_err(|_| LocalModelFailure::IntegrityFailure {
            path: artifact.relative_path.clone(),
        })?;
    if !metadata.file_type().is_file() || metadata.len() != artifact.size_bytes {
        return Err(LocalModelFailure::IntegrityFailure {
            path: artifact.relative_path.clone(),
        });
    }
    let mut file = File::open(&path).map_err(|_| LocalModelFailure::IntegrityFailure {
        path: artifact.relative_path.clone(),
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| LocalModelFailure::IntegrityFailure {
                path: artifact.relative_path.clone(),
            })?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    if !artifact
        .sha256
        .eq_ignore_ascii_case(&format!("{:x}", hasher.finalize()))
    {
        return Err(LocalModelFailure::IntegrityFailure {
            path: artifact.relative_path.clone(),
        });
    }
    Ok(())
}

fn regular_files(root: &Path) -> Result<HashSet<PathBuf>, LocalModelFailure> {
    fn visit(
        base: &Path,
        dir: &Path,
        files: &mut HashSet<PathBuf>,
    ) -> Result<(), LocalModelFailure> {
        for entry in std::fs::read_dir(dir).map_err(|_| LocalModelFailure::InstallationInvalid)? {
            let entry = entry.map_err(|_| LocalModelFailure::InstallationInvalid)?;
            let ty = entry
                .file_type()
                .map_err(|_| LocalModelFailure::InstallationInvalid)?;
            if ty.is_symlink() {
                return Err(LocalModelFailure::InstallationInvalid);
            }
            if ty.is_dir() {
                visit(base, &entry.path(), files)?;
            } else if ty.is_file() {
                let relative = entry
                    .path()
                    .strip_prefix(base)
                    .map_err(|_| LocalModelFailure::InstallationInvalid)?
                    .to_path_buf();
                if relative != Path::new(IDENTITY_FILE) {
                    files.insert(relative);
                }
            } else {
                return Err(LocalModelFailure::InstallationInvalid);
            }
        }
        Ok(())
    }
    let mut files = HashSet::new();
    visit(root, root, &mut files)?;
    Ok(files)
}

fn manifest_sha256(manifest: &LocalModelManifest) -> String {
    let mut artifacts: Vec<_> = manifest.artifacts.iter().collect();
    artifacts.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    let mut qualifications: Vec<_> = manifest.qualifications.iter().collect();
    qualifications.sort_by_key(|qualification| qualification.challenge_kind as u8);
    fn field(hasher: &mut Sha256, bytes: &[u8]) {
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }
    let mut hasher = Sha256::new();
    for value in [
        manifest.identity.provider.as_bytes(),
        manifest.identity.repository.as_bytes(),
        manifest.identity.model.as_bytes(),
        manifest.identity.revision.as_bytes(),
        manifest.runtime_requirements.runtime.as_bytes(),
        manifest.runtime_requirements.preprocessing.as_bytes(),
    ] {
        field(&mut hasher, value);
    }
    hasher.update(
        manifest
            .runtime_requirements
            .minimum_ram_bytes
            .to_be_bytes(),
    );
    hasher.update((manifest.runtime_requirements.devices.len() as u64).to_be_bytes());
    for requirement in &manifest.runtime_requirements.devices {
        hasher.update([match requirement.device {
            LocalModelDevice::Cpu => 0,
            LocalModelDevice::Cuda => 1,
            LocalModelDevice::Metal => 2,
        }]);
        match requirement.minimum_device_memory_bytes {
            Some(bytes) => {
                hasher.update([1]);
                hasher.update(bytes.to_be_bytes());
            }
            None => hasher.update([0]),
        }
    }
    hasher.update((artifacts.len() as u64).to_be_bytes());
    for artifact in artifacts {
        field(
            &mut hasher,
            artifact
                .relative_path
                .to_str()
                .expect("validated path")
                .as_bytes(),
        );
        field(&mut hasher, artifact.reference.path.as_bytes());
        hasher.update(artifact.size_bytes.to_be_bytes());
        field(&mut hasher, artifact.sha256.to_ascii_lowercase().as_bytes());
    }
    hasher.update((qualifications.len() as u64).to_be_bytes());
    for qualification in qualifications {
        hasher.update([match qualification.challenge_kind {
            CaptchaChallengeKind::ImageGridSelection => 0,
            CaptchaChallengeKind::HorizontalOffset => 1,
            CaptchaChallengeKind::PointSelection => 2,
        }]);
        field(&mut hasher, qualification.evaluation_id.as_bytes());
        field(
            &mut hasher,
            qualification
                .evaluation_sha256
                .to_ascii_lowercase()
                .as_bytes(),
        );
        field(&mut hasher, qualification.passing_policy.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn identity_record(identity: &InstalledModelIdentity) -> String {
    format!("version=1\nprovider={}\nrepository={}\nmodel={}\nrevision={}\nmanifest_sha256={}\nruntime={}\npreprocessing={}\n",
        identity.model.provider, identity.model.repository, identity.model.model,
        identity.model.revision, identity.manifest_sha256, identity.runtime,
        identity.preprocessing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::artifact_reference::{ArtifactIdentity, ArtifactIdentityKind};
    use crate::features::source_provider::ProviderId;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn scratch() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "scorpion-local-model-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn manifest(body: &[u8]) -> LocalModelManifest {
        let digest = format!("{:x}", Sha256::digest(body));
        LocalModelManifest {
            identity: LocalModelIdentity {
                provider: "fixture".into(),
                repository: "models/fixture".into(),
                model: "vision".into(),
                revision: "0123456789abcdef".into(),
            },
            artifacts: vec![LocalModelArtifact {
                reference: ArtifactReference {
                    provider_id: ProviderId::from("fixture"),
                    repository_id: "models/fixture".into(),
                    path: "weights.bin".into(),
                    requested_revision: None,
                    resolved_revision: Some("0123456789abcdef".into()),
                    size_bytes: Some(body.len() as u64),
                    identities: vec![ArtifactIdentity {
                        kind: ArtifactIdentityKind::LfsSha256,
                        value: digest.clone(),
                    }],
                    download_url: None,
                    discovered_via: None,
                },
                relative_path: "weights.bin".into(),
                size_bytes: body.len() as u64,
                sha256: digest,
            }],
            runtime_requirements: LocalModelRuntimeRequirements {
                runtime: "fixture-runtime@1".into(),
                preprocessing: "fixture-preprocess@1".into(),
                minimum_ram_bytes: 1024,
                devices: vec![LocalModelDeviceRequirement {
                    device: LocalModelDevice::Cpu,
                    minimum_device_memory_bytes: None,
                }],
            },
            qualifications: vec![LocalModelQualification {
                challenge_kind: CaptchaChallengeKind::ImageGridSelection,
                evaluation_id: "suite@1".into(),
                evaluation_sha256: "a".repeat(64),
                passing_policy: "accuracy>=0.9".into(),
            }],
        }
    }

    #[test]
    fn complete_verified_staging_activates_atomically_and_reopens() {
        let root = scratch();
        let staging = root.join("model.staging");
        let active = root.join("model");
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(staging.join("weights.bin"), b"weights").unwrap();
        let manifest = manifest(b"weights");

        let installation = manifest.activate(&staging, &active).unwrap();
        assert!(!staging.exists());
        assert_eq!(installation.root(), active);
        assert_eq!(
            manifest.open_installation(&active).unwrap().identity(),
            installation.identity()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_extra_or_corrupt_members_never_activate() {
        for mutate in 0..3 {
            let root = scratch();
            let staging = root.join("model.staging");
            let active = root.join("model");
            std::fs::create_dir(&staging).unwrap();
            if mutate != 0 {
                std::fs::write(
                    staging.join("weights.bin"),
                    if mutate == 1 {
                        b"bad" as &[u8]
                    } else {
                        b"weights"
                    },
                )
                .unwrap();
            }
            if mutate == 2 {
                std::fs::write(staging.join("extra"), b"x").unwrap();
            }
            assert!(manifest(b"weights").activate(&staging, &active).is_err());
            assert!(!active.exists());
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn cross_file_revision_mismatch_fails_before_disk_access() {
        let mut manifest = manifest(b"weights");
        manifest.artifacts[0].reference.resolved_revision = Some("different".into());
        assert!(matches!(
            manifest.validate(),
            Err(LocalModelFailure::RevisionMismatch { .. })
        ));
    }

    #[test]
    fn qualifications_are_independent_and_mandatory_for_advertisement() {
        let manifest = manifest(b"weights");
        assert!(manifest
            .require_qualification(CaptchaChallengeKind::ImageGridSelection)
            .is_ok());
        assert_eq!(
            manifest.require_qualification(CaptchaChallengeKind::PointSelection),
            Err(LocalModelFailure::QualificationMissing {
                challenge_kind: CaptchaChallengeKind::PointSelection
            })
        );
    }

    #[test]
    fn device_fallback_is_only_the_explicit_operator_order() {
        let requirements = LocalModelRuntimeRequirements {
            runtime: "runtime@1".into(),
            preprocessing: "preprocess@1".into(),
            minimum_ram_bytes: 10,
            devices: vec![
                LocalModelDeviceRequirement {
                    device: LocalModelDevice::Cuda,
                    minimum_device_memory_bytes: Some(20),
                },
                LocalModelDeviceRequirement {
                    device: LocalModelDevice::Cpu,
                    minimum_device_memory_bytes: None,
                },
            ],
        };
        let policy = LocalModelDevicePolicy {
            primary: LocalModelDevice::Cuda,
            fallbacks: vec![LocalModelDevice::Cpu],
        };
        let selected = preflight_device(&requirements, &policy, |device| match device {
            LocalModelDevice::Cuda => None,
            LocalModelDevice::Cpu => Some(LocalModelRuntimeResources {
                available_ram_bytes: 100,
                available_device_memory_bytes: Some(100),
            }),
            LocalModelDevice::Metal => panic!("undeclared fallback inspected"),
        })
        .unwrap();
        assert_eq!(selected, LocalModelDevice::Cpu);
    }

    #[test]
    fn runtime_lifecycle_requires_verified_installation_and_explicit_unload() {
        let root = scratch();
        let staging = root.join("staging");
        let active = root.join("active");
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(staging.join("weights.bin"), b"weights").unwrap();
        let installation = manifest(b"weights").activate(&staging, &active).unwrap();
        let mut lifecycle = LocalModelRuntimeLifecycle::default();
        lifecycle
            .begin_initialization(&installation, LocalModelDevice::Cpu)
            .unwrap();
        lifecycle.mark_ready(&installation).unwrap();
        assert!(matches!(
            lifecycle.state(),
            LocalModelRuntimeState::Ready { .. }
        ));
        lifecycle.unload();
        assert_eq!(lifecycle.state(), &LocalModelRuntimeState::Unloaded);
        std::fs::remove_dir_all(root).unwrap();
    }
}
