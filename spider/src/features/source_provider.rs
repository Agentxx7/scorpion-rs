//! Parser-, acquisition-, and transport-neutral vocabulary for source-specific
//! discovery providers and their deterministic metadata registry.
//!
//! This module deliberately does not define provider execution. Existing web
//! search implementations retain their narrower `SearchProvider` contract;
//! future provider execution can consume this vocabulary without making the
//! registry own clients, credentials, transport policy, or parser state.

use crate::features::artifact_reference::ArtifactReference;
use crate::features::discovery_target::DiscoveryTarget;
use crate::features::source::SourceItem;
use std::collections::BTreeMap;

/// Stable, machine-readable identity of a source provider.
///
/// Identity is independent of presentation: changing a provider's display
/// name does not change registry lookup or duplicate detection.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ProviderId(String);

impl ProviderId {
    /// Construct an opaque provider ID. The registry never derives IDs from
    /// display names and does not assign provider-specific meaning to them.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Borrow the stable machine-readable value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<&str> for ProviderId {
    fn from(id: &str) -> Self {
        Self::new(id)
    }
}

impl From<String> for ProviderId {
    fn from(id: String) -> Self {
        Self::new(id)
    }
}

/// The canonical output shapes a provider declares it can produce.
///
/// These booleans describe output only. They do not select acquisition,
/// transport, parsing, ranking, authentication, or persistence behavior.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProviderCapabilities {
    /// Provider may emit already-normalized [`SourceItem`] values.
    pub items: bool,
    /// Provider may emit [`DiscoveryTarget`] values for later acquisition.
    pub targets: bool,
    /// Provider may emit versioned [`ArtifactReference`] metadata.
    pub artifacts: bool,
}

impl ProviderCapabilities {
    /// A provider that emits normalized items only.
    pub const ITEMS: Self = Self {
        items: true,
        targets: false,
        artifacts: false,
    };

    /// A provider that emits acquisition targets only.
    pub const TARGETS: Self = Self {
        items: false,
        targets: true,
        artifacts: false,
    };

    /// A provider that may emit both independent output shapes.
    pub const ITEMS_AND_TARGETS: Self = Self {
        items: true,
        targets: true,
        artifacts: false,
    };

    /// A provider that emits versioned artifact references only.
    pub const ARTIFACTS: Self = Self {
        items: false,
        targets: false,
        artifacts: true,
    };

    /// A provider that emits normalized items and versioned artifacts.
    pub const ITEMS_AND_ARTIFACTS: Self = Self {
        items: true,
        targets: false,
        artifacts: true,
    };
}

/// Minimal provider metadata used by registry lookup and future orchestration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderDescriptor {
    /// Stable identity used for lookup and duplicate detection.
    pub id: ProviderId,
    /// Human-readable presentation label; never canonical identity.
    pub display_name: String,
    /// Output shapes the provider may produce.
    pub capabilities: ProviderCapabilities,
}

impl ProviderDescriptor {
    /// Construct a descriptor without registering or executing a provider.
    pub fn new(
        id: impl Into<ProviderId>,
        display_name: impl Into<String>,
        capabilities: ProviderCapabilities,
    ) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            capabilities,
        }
    }
}

/// Metadata contract implemented by future concrete source providers.
///
/// Execution is intentionally absent: a provider descriptor cannot acquire a
/// URL, select a transport, parse a document, or access credentials.
pub trait SourceProvider {
    /// Describe stable identity and output capability.
    fn descriptor(&self) -> &ProviderDescriptor;
}

/// One provider-native discovery output, preserving the canonical distinction
/// between normalized content and a URL that still needs acquisition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderDiscovery {
    /// Already-normalized provider content/artifact.
    Item(SourceItem),
    /// A validated target for the separate canonical acquisition pipeline.
    Target(DiscoveryTarget),
    /// Versioned provider-native artifact metadata for possible later download.
    Artifact(ArtifactReference),
}

/// Deterministic metadata-only provider registry.
///
/// `BTreeMap` makes exposed iteration stable by [`ProviderId`]. The registry
/// owns no provider implementation, client, credential, or execution state.
#[derive(Clone, Debug, Default)]
pub struct SourceProviderRegistry {
    descriptors: BTreeMap<ProviderId, ProviderDescriptor>,
}

impl SourceProviderRegistry {
    /// Construct an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register metadata, rejecting duplicate IDs without overwriting the
    /// descriptor already associated with that identity.
    pub fn register(
        &mut self,
        descriptor: ProviderDescriptor,
    ) -> Result<(), ProviderRegistryError> {
        if self.descriptors.contains_key(&descriptor.id) {
            return Err(ProviderRegistryError::DuplicateProviderId(
                descriptor.id.clone(),
            ));
        }
        self.descriptors.insert(descriptor.id.clone(), descriptor);
        Ok(())
    }

    /// Deterministic lookup. An unknown ID is represented truthfully as
    /// `None`; lookup performs no provider construction or fallback.
    pub fn get(&self, id: &ProviderId) -> Option<&ProviderDescriptor> {
        self.descriptors.get(id)
    }

    /// Iterate descriptors in stable [`ProviderId`] order.
    pub fn iter(&self) -> impl Iterator<Item = &ProviderDescriptor> {
        self.descriptors.values()
    }

    /// Number of registered descriptors.
    pub fn len(&self) -> usize {
        self.descriptors.len()
    }

    /// Whether no descriptors are registered.
    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }
}

/// Metadata registration errors. Provider execution errors do not belong to
/// this registry contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderRegistryError {
    /// The stable ID is already registered; the original descriptor remains.
    DuplicateProviderId(ProviderId),
}

impl std::fmt::Display for ProviderRegistryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateProviderId(id) => {
                write!(
                    formatter,
                    "source provider ID \"{id}\" is already registered"
                )
            }
        }
    }
}

impl std::error::Error for ProviderRegistryError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::discovery_target::DiscoveryTargetKind;

    fn descriptor(id: &str, display_name: &str) -> ProviderDescriptor {
        ProviderDescriptor::new(id, display_name, ProviderCapabilities::TARGETS)
    }

    #[test]
    fn provider_id_is_stable_and_independent_of_display_name() {
        let first = descriptor("example", "First presentation");
        let renamed = descriptor("example", "Renamed presentation");
        assert_eq!(first.id, renamed.id);
        assert_ne!(first.display_name, renamed.display_name);
        assert_eq!(first.id.as_str(), "example");
    }

    #[test]
    fn lookup_and_iteration_are_deterministic() {
        let mut registry = SourceProviderRegistry::new();
        registry.register(descriptor("zeta", "Zeta")).unwrap();
        registry.register(descriptor("alpha", "Alpha")).unwrap();

        assert_eq!(
            registry
                .get(&ProviderId::from("zeta"))
                .unwrap()
                .display_name,
            "Zeta"
        );
        assert_eq!(
            registry
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "zeta"]
        );
    }

    #[test]
    fn duplicate_id_is_rejected_without_overwrite() {
        let mut registry = SourceProviderRegistry::new();
        registry.register(descriptor("same", "Original")).unwrap();
        let error = registry
            .register(descriptor("same", "Replacement"))
            .unwrap_err();

        assert_eq!(
            error,
            ProviderRegistryError::DuplicateProviderId(ProviderId::from("same"))
        );
        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry
                .get(&ProviderId::from("same"))
                .unwrap()
                .display_name,
            "Original"
        );
    }

    #[test]
    fn unknown_lookup_is_none_without_fallback() {
        let registry = SourceProviderRegistry::new();
        assert_eq!(registry.get(&ProviderId::from("unknown")), None);
        assert!(registry.is_empty());
    }

    #[test]
    fn item_and_target_outputs_remain_distinct_without_conversion() {
        let item = SourceItem {
            source_type: "fixture".to_string(),
            title: Some("Item".to_string()),
            ..Default::default()
        };
        let target = DiscoveryTarget {
            url: "https://example.test/".to_string(),
            kind: DiscoveryTargetKind::Requested,
            discovered_via: None,
        };

        let outputs = [
            ProviderDiscovery::Item(item.clone()),
            ProviderDiscovery::Target(target.clone()),
        ];
        assert!(matches!(&outputs[0], ProviderDiscovery::Item(value) if value == &item));
        assert!(matches!(&outputs[1], ProviderDiscovery::Target(value) if value == &target));
    }

    #[test]
    fn artifact_output_remains_explicitly_separate() {
        let artifact = ArtifactReference {
            provider_id: ProviderId::from("example"),
            repository_id: "owner/repository".to_string(),
            path: "artifact.bin".to_string(),
            requested_revision: Some("main".to_string()),
            resolved_revision: None,
            size_bytes: None,
            identities: Vec::new(),
            download_url: None,
            discovered_via: None,
        };
        let output = ProviderDiscovery::Artifact(artifact.clone());

        assert!(matches!(output, ProviderDiscovery::Artifact(value) if value == artifact));
        assert!(ProviderCapabilities::ARTIFACTS.artifacts);
        assert!(!ProviderCapabilities::ARTIFACTS.items);
        assert!(!ProviderCapabilities::ARTIFACTS.targets);
    }

    #[test]
    fn base_contract_is_metadata_only_and_has_no_execution_method() {
        struct FixtureProvider(ProviderDescriptor);
        impl SourceProvider for FixtureProvider {
            fn descriptor(&self) -> &ProviderDescriptor {
                &self.0
            }
        }

        fn type_check(provider: &dyn SourceProvider) -> &ProviderDescriptor {
            provider.descriptor()
        }

        let provider = FixtureProvider(ProviderDescriptor::new(
            "fixture",
            "Fixture",
            ProviderCapabilities::ITEMS_AND_TARGETS,
        ));
        assert_eq!(type_check(&provider).id.as_str(), "fixture");
    }
}
