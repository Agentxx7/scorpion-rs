# Canonical Local Model Installation and Runtime Contract SDD

Frontier: `SCORPION_CANONICAL_LOCAL_MODEL_INSTALLATION_AND_RUNTIME_CONTRACT_001`

Baseline: `505470e13c71c47801529be6b75869f9f4c26f8a`

## 1. Purpose

This frontier establishes provider- and inference-runtime-neutral vocabulary
for immutable multi-file local model installation, verified identity, device
preflight, persistent runtime lifecycle and empirical capability qualification.
It installs no model and adds no inference engine.

## 2. Canonical model

`LocalModelManifest` binds one `LocalModelIdentity`—provider, repository,
model and immutable resolved revision—to a complete set of
`LocalModelArtifact` members. Every member has a safe installation-relative
path, mandatory nonzero size and mandatory SHA-256 digest, while retaining its
existing `ArtifactReference` acquisition metadata.

`InstalledModelIdentity` binds the exact model identity to a normalized
manifest SHA-256 and exact runtime/preprocessing contract. It is persisted in
the active installation and rechecked when the installation is opened.

`LocalModelRuntimeRequirements` declares runtime identity, preprocessing and
per-device minimum resources. `LocalModelDevicePolicy` contains an
operator-selected primary device and the only permitted fallback sequence.
`LocalModelRuntimeState` and `LocalModelRuntimeLifecycle` expose explicit
uninitialized, initializing, ready/reusable, failed and unloaded states without
exposing an inference handle.

Each `LocalModelQualification` applies to exactly one CAPTCHA challenge kind,
one pinned evaluation identity/digest and the manifest's exact
model/revision/runtime/preprocessing tuple. Qualification is not inferred from
model-family claims.

## 3. Canonical seams

```text
LocalModelManifest::validate()
LocalModelManifest::activate(staging, active)
    -> LocalModelInstallation | LocalModelFailure
LocalModelManifest::open_installation(active)
    -> LocalModelInstallation | LocalModelFailure
preflight_device(requirements, operator_policy, adapter_inspection)
    -> LocalModelDevice | LocalModelFailure
LocalModelRuntimeLifecycle::begin_initialization(&installation, device)
```

Only `LocalModelInstallation`, whose fields are private, authorizes runtime
initialization. It reverifies durable identity and all members immediately
before the lifecycle enters initialization. Runtime adapters privately own
backend handles.

## 4. Installation graph

```text
ArtifactReference
→ ArtifactDownloadBinding
→ canonical streaming acquisition and single-file atomic finalization
→ caller-owned sibling staging directory
→ LocalModelManifest completeness/size/SHA-256/revision verification
→ durable installed identity marker
→ atomic directory rename
→ LocalModelInstallation
```

Acquisition remains outside this module. The module contains no HTTP client,
request model, redirect policy or download fallback. Missing, extra, symlinked,
corrupt or cross-revision members fail before activation.

## 5. Runtime graph

```text
verified LocalModelInstallation
→ explicit LocalModelDevicePolicy
→ adapter-reported availability/resources
→ fail-closed preflight
→ persistent private runtime initialization
→ Ready/reuse
→ explicit Failed or Unloaded transition
```

Initialization and inference are offline-only by contract. Automatic model
discovery, mutable revisions and hidden network acquisition are forbidden.

## 6. Dependencies

Allowed: existing `ArtifactReference`, CAPTCHA challenge-kind vocabulary,
filesystem primitives and SHA-256.

Forbidden: Candle, ONNX Runtime, LibTorch, raw HTTP clients, provider SDKs,
model downloads, runtime-owned routing and implicit device fallback.

SHA-256 is feature-independent because installation integrity is mandatory in
all builds, not evidence-only behavior.

## 7. Security and integrity

Manifest paths must be relative normal components and unique. Revisions must
be immutable and every `ArtifactReference.resolved_revision` must equal the
manifest revision. Staging and activation directories must be siblings so the
directory rename cannot cross filesystems. The active destination must not
exist. No partially verified directory becomes active.

Opening an installation rechecks the durable identity marker, complete file
membership, sizes and digests. Symlinks and undeclared members are rejected.

## 8. Failures

The canonical failures are `ModelNotInstalled`, `InstallationInvalid`,
`IntegrityFailure`, `RevisionMismatch`, `RuntimeUnavailable`,
`DeviceUnavailable`, `ResourceLimitExceeded`, `InitializationFailure` and
`QualificationMissing`. Backend exceptions, filenames outside the manifest and
runtime handles never enter this vocabulary.

## 9. Done definition

- all required neutral types exist once;
- incomplete or corrupt staging cannot activate;
- installed identity is durable and revalidated;
- runtime initialization requires a verified installation;
- device fallback is explicitly operator-controlled;
- resource/device preflight fails closed;
- CAPTCHA kinds require independent pinned qualification;
- the module owns neither artifact transport nor inference execution;
- architecture scanners reject hidden download/runtime authority.

## 10. Out of scope

No model, Candle dependency, CAPTCHA provider, download, runtime adapter,
Ollama/Anthropic integration, routing change, fallback chain or mutable model
resolution is introduced.
