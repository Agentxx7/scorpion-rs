//! Offline production runtime for the pinned PaliGemma-3b-mix-224 model.
//!
//! Construction accepts only a verified [`LocalModelInstallation`]. It never
//! discovers or downloads files and it has no transport dependency.
//!
//! PaliGemma (SigLIP vision encoder + Gemma-1 language model) is a
//! genuinely different model family from Qwen3-VL: no MRoPE, no
//! multi-axis position encoding, no deepstack merger, and a single fixed
//! 224x224 input envelope (no dynamic smart-resize). Its detection
//! grounding uses `<locNNNN>` tokens that are ordinary vocabulary entries —
//! plain next-token generation, not a special regression head — so the
//! same grammar-constrained greedy-decode pattern already proven for
//! Qwen3-VL structured generation applies directly, just against a
//! different vocabulary/grammar shape.
//!
//! Two genuine `candle-transformers` 0.11.0 defects were found and fixed in
//! the vendored fork (`vendor/candle-transformers-qwen3vl-fix`) while
//! qualifying this model — see its README and
//! `docs/frontier/LOCAL_CROSS_MODEL_VISION_CAPTCHA_PROVIDER_QUALIFICATION_SDD.md`:
//! a spurious L2-normalization of image embeddings absent from the real
//! reference architecture, and a missing causal mask on the multimodal
//! prefill forward pass (every prefill position attended bidirectionally).

use crate::features::artifact_reference::ArtifactReference;
use crate::features::local_model::{
    LocalModelArtifact, LocalModelDevice, LocalModelDeviceRequirement, LocalModelIdentity,
    LocalModelManifest, LocalModelRuntimeRequirements,
};
use crate::features::local_model::{LocalModelFailure, LocalModelInstallation};
use crate::features::paligemma_generation::PaligemmaGenerationFactory;
use crate::features::source_provider::ProviderId;
use candle::{DType, Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::paligemma::Config;
use image::{imageops::FilterType, GenericImageView};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Instant;
use sysinfo::System;
use tokenizers::Tokenizer;

/// Immutable upstream model revision accepted by this runtime.
pub const PALIGEMMA_MODEL_REVISION: &str = "d1d8734c9c3ad0ccfeea4afc270faa356c2ba515";
/// Minimum available host memory required before initialization. Derived
/// from a real measured peak RSS of ~23.6 GB (F32-native 11.7 GB weight
/// shards plus vision-tower activations and KV cache) during a real
/// load-and-detect cycle, rounded up with a safety margin — not a
/// formula-only estimate. See the frontier SDD for the full measurement.
pub const PALIGEMMA_MINIMUM_RAM_BYTES: u64 = 25_500_000_000;
/// Stable implementation identity for the pinned processor/template contract.
pub const PALIGEMMA_PROCESSOR_ID: &str =
    "paligemma-3b-mix-224@d1d8734c-candle-0.11.0-cpu-f32-processor-v1-224x224";
/// Immutable accelerated-runtime identity: the same pinned model/revision
/// and the same 224x224 processor pipeline as the CPU/F32 path, executed on
/// CUDA with F16 weights instead — see
/// `SCORPION_PALIGEMMA_LOCAL_INFERENCE_LATENCY_QUALIFICATION_001`. A
/// distinct identity string (never reused across dtype/device) keeps
/// provenance truthful about which backend actually produced a result.
#[cfg(feature = "local_paligemma_cuda")]
pub const PALIGEMMA_CUDA_RUNTIME_IDENTITY: &str = "candle-0.11.0-cuda-f16";
/// Stable implementation identity for the CUDA/F16 processor contract. The
/// image pipeline itself (resize/normalize) is identical to
/// `PALIGEMMA_PROCESSOR_ID`; only the executing backend differs.
#[cfg(feature = "local_paligemma_cuda")]
pub const PALIGEMMA_CUDA_PROCESSOR_ID: &str =
    "paligemma-3b-mix-224@d1d8734c-candle-0.11.0-cuda-f16-processor-v1-224x224";
/// Minimum available host RAM required before CUDA/F16 initialization.
/// Real measured peak RSS during a real load-and-detect cycle was
/// 12,338,528,256 bytes (~11.49 GB) — far below `PALIGEMMA_MINIMUM_RAM_BYTES`
/// because weights do not remain host-resident once uploaded to VRAM — with
/// this floor rounded up with the same ~8% safety margin used for the
/// CPU/F32 floor, not a formula-only estimate. See the frontier SDD for the
/// full measurement.
#[cfg(feature = "local_paligemma_cuda")]
pub const PALIGEMMA_CUDA_MINIMUM_RAM_BYTES: u64 = 13_500_000_000;
/// Minimum available device (VRAM) memory required before CUDA/F16
/// initialization. Real measured peak VRAM during a real load-and-detect
/// cycle was 7,098,646,528 bytes (6,769 MiB) — F16-native ~5.85 GB weight
/// shards plus vision-tower activations, KV cache, and CUDA
/// context/cuBLAS/cuRAND overhead — rounded up with a safety margin, not a
/// formula-only estimate. See the frontier SDD for the full measurement.
#[cfg(feature = "local_paligemma_cuda")]
pub const PALIGEMMA_CUDA_MINIMUM_VRAM_BYTES: u64 = 8_000_000_000;

const REQUIRED_ARTIFACTS: [&str; 7] = [
    "model-00001-of-00003.safetensors",
    "model-00002-of-00003.safetensors",
    "model-00003-of-00003.safetensors",
    "config.json",
    "tokenizer.json",
    "tokenizer_config.json",
    "preprocessor_config.json",
];
/// Real tensor count across the three pinned shards (`model.safetensors.index.json`).
const EXPECTED_TENSORS: usize = 603;
const IMAGE_SIZE: u32 = 224;
const PATCH_SIZE: u32 = 14;
/// `(224 / 14)^2`.
const IMAGE_SEQ_LEN: usize = 256;
const IMAGE_TOKEN_ID: u32 = 257_152;
const BOS_TOKEN_ID: u32 = 2;
const EOS_TOKEN_ID: u32 = 1;
/// `<loc0000>`; the 1024 location tokens are contiguous, `<loc0000>` +
/// bin value.
const LOC_TOKEN_BASE: u32 = 256_000;
const LOC_TOKEN_COUNT: u32 = 1024;

const PINNED_ARTIFACTS: [(&str, u64, &str); 7] = [
    (
        "model-00001-of-00003.safetensors",
        4_953_412_480,
        "63fd690cdad783794ce4c1d7893ef1f88bc26f4ae94b1925f5d3ad199dbbed87",
    ),
    (
        "model-00002-of-00003.safetensors",
        4_999_820_608,
        "c24cf160613c9fcf69641b4eace7de94d07aaabba88800815137453a5fc32f38",
    ),
    (
        "model-00003-of-00003.safetensors",
        1_740_714_288,
        "6222ec75c92d438886eaa3a2c5970c7e7973c213937a799ca6c5106f5fae829a",
    ),
    (
        "config.json",
        1_028,
        "acdf1016c0f752846bd0f3b4289256352b8f81aa8e39f6270220cc6f2fb5c1a7",
    ),
    (
        "tokenizer.json",
        17_549_604,
        "ef6773c135b77b834de1d13c75a4c98ab7a3684ffd602d1831e1f1bf5467c563",
    ),
    (
        "tokenizer_config.json",
        39_968,
        "3259402b1d1802e02417d7bff75a889ec61d359d15be6050a957b307c48edbbe",
    ),
    (
        "preprocessor_config.json",
        699,
        "ae5e5f748a642ae71344facf4012b1607852db13f080e42cd12b5db4e75ca7e8",
    ),
];

/// Exact immutable manifest for the qualified image-only production runtime.
/// It contains no mutable URL identity and advertises no CAPTCHA qualification.
pub fn paligemma_cpu_f32_manifest() -> LocalModelManifest {
    LocalModelManifest {
        identity: pinned_model_identity(),
        artifacts: pinned_artifacts(),
        runtime_requirements: LocalModelRuntimeRequirements {
            runtime: "candle-0.11.0-cpu-f32".into(),
            preprocessing: PALIGEMMA_PROCESSOR_ID.into(),
            minimum_ram_bytes: PALIGEMMA_MINIMUM_RAM_BYTES,
            devices: vec![LocalModelDeviceRequirement {
                device: LocalModelDevice::Cpu,
                minimum_device_memory_bytes: None,
            }],
        },
        qualifications: Vec::new(),
    }
}

/// Exact immutable manifest for the accelerated CUDA/F16 runtime — the same
/// pinned model/revision and pinned artifact bytes as
/// [`paligemma_cpu_f32_manifest`], loaded through a different backend.
/// `devices` declares CUDA only, with no CPU entry: this manifest itself
/// carries no fallback, matching the runtime's own fail-closed construction.
#[cfg(feature = "local_paligemma_cuda")]
pub fn paligemma_cuda_f16_manifest() -> LocalModelManifest {
    LocalModelManifest {
        identity: pinned_model_identity(),
        artifacts: pinned_artifacts(),
        runtime_requirements: LocalModelRuntimeRequirements {
            runtime: PALIGEMMA_CUDA_RUNTIME_IDENTITY.into(),
            preprocessing: PALIGEMMA_CUDA_PROCESSOR_ID.into(),
            minimum_ram_bytes: PALIGEMMA_CUDA_MINIMUM_RAM_BYTES,
            devices: vec![LocalModelDeviceRequirement {
                device: LocalModelDevice::Cuda,
                minimum_device_memory_bytes: Some(PALIGEMMA_CUDA_MINIMUM_VRAM_BYTES),
            }],
        },
        qualifications: Vec::new(),
    }
}

fn pinned_model_identity() -> LocalModelIdentity {
    LocalModelIdentity {
        provider: "huggingface".into(),
        repository: "google/paligemma-3b-mix-224".into(),
        model: "paligemma-3b-mix-224".into(),
        revision: PALIGEMMA_MODEL_REVISION.into(),
    }
}

fn pinned_artifacts() -> Vec<LocalModelArtifact> {
    PINNED_ARTIFACTS
        .iter()
        .map(|(path, size, sha256)| LocalModelArtifact {
            reference: ArtifactReference {
                provider_id: ProviderId::from("huggingface"),
                repository_id: "google/paligemma-3b-mix-224".into(),
                path: (*path).into(),
                requested_revision: Some(PALIGEMMA_MODEL_REVISION.into()),
                resolved_revision: Some(PALIGEMMA_MODEL_REVISION.into()),
                size_bytes: Some(*size),
                identities: Vec::new(),
                download_url: None,
                discovered_via: None,
            },
            relative_path: PathBuf::from(path),
            size_bytes: *size,
            sha256: (*sha256).into(),
        })
        .collect()
}

/// Fail-closed runtime construction or inference error.
#[derive(Debug)]
pub enum PaligemmaRuntimeFailure {
    /// The verified installation does not identify the pinned model/runtime.
    Installation(LocalModelFailure),
    /// A required pinned artifact or semantic identity is invalid.
    InvalidArtifact(&'static str),
    /// Available host memory is below the qualified floor.
    ResourceLimitExceeded {
        /// Available host RAM observed during preflight.
        available: u64,
        /// Qualified minimum host RAM.
        required: u64,
    },
    /// Available device (GPU) memory is below the qualified floor. Distinct
    /// from `ResourceLimitExceeded` (host RAM) so the two resource classes
    /// are never conflated in a failure report.
    DeviceMemoryLimitExceeded {
        /// Available device memory observed during preflight.
        available: u64,
        /// Qualified minimum device memory.
        required: u64,
    },
    /// The accelerated device could not be initialized. No CPU fallback is
    /// attempted: this is a fail-closed terminal error for the accelerated
    /// initializer, never a silent downgrade.
    DeviceUnavailable(String),
    /// The input cannot be represented by the qualified processor envelope.
    InvalidInput(&'static str),
    /// Offline model initialization failed.
    Initialization(String),
    /// Image preprocessing failed.
    Processing(String),
    /// Tokenization or decoding failed.
    Tokenization(String),
    /// Request-local inference failed.
    Inference(String),
    /// No model token can legally continue the active structured grammar.
    NoValidStructuredContinuation,
}

impl std::fmt::Display for PaligemmaRuntimeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Installation(error) => write!(f, "local-model installation failed: {error:?}"),
            Self::InvalidArtifact(name) => write!(f, "invalid pinned PaliGemma artifact: {name}"),
            Self::ResourceLimitExceeded {
                available,
                required,
            } => write!(
                f,
                "insufficient available RAM ({available} bytes; {required} required)"
            ),
            Self::DeviceMemoryLimitExceeded {
                available,
                required,
            } => write!(
                f,
                "insufficient available device memory ({available} bytes; {required} required)"
            ),
            Self::DeviceUnavailable(reason) => {
                write!(f, "PaliGemma accelerated device unavailable: {reason}")
            }
            Self::InvalidInput(reason) => write!(f, "invalid PaliGemma input: {reason}"),
            Self::Initialization(reason) => write!(f, "PaliGemma initialization failed: {reason}"),
            Self::Processing(reason) => write!(f, "PaliGemma processing failed: {reason}"),
            Self::Tokenization(reason) => write!(f, "PaliGemma tokenization failed: {reason}"),
            Self::Inference(reason) => write!(f, "PaliGemma inference failed: {reason}"),
            Self::NoValidStructuredContinuation => {
                write!(f, "PaliGemma structured grammar has no valid continuation")
            }
        }
    }
}

impl std::error::Error for PaligemmaRuntimeFailure {}

impl From<LocalModelFailure> for PaligemmaRuntimeFailure {
    fn from(value: LocalModelFailure) -> Self {
        Self::Installation(value)
    }
}

/// Pinned deterministic generation settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaligemmaGenerationConfiguration {
    /// Hard output-token bound.
    pub maximum_generated_tokens: usize,
}

/// One genuine `detect {label}` result: the four quantized location bins
/// (`y_min, x_min, y_max, x_max`, each in `0..1024`) exactly as the model
/// produced them — an unmodified parse of the model's own real spatial
/// answer, not a corrected or averaged one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaligemmaDetectionBox {
    /// Top edge, quantized bin (`0..1024`).
    pub y_min: u32,
    /// Left edge, quantized bin (`0..1024`).
    pub x_min: u32,
    /// Bottom edge, quantized bin (`0..1024`).
    pub y_max: u32,
    /// Right edge, quantized bin (`0..1024`).
    pub x_max: u32,
    /// Wall-clock inference duration.
    pub elapsed: std::time::Duration,
}

impl PaligemmaDetectionBox {
    /// The box center in quantized bin units (`0.0..1024.0`).
    pub fn center_bins(&self) -> (f64, f64) {
        (
            f64::from(self.x_min + self.x_max) / 2.0,
            f64::from(self.y_min + self.y_max) / 2.0,
        )
    }
}

/// Neutral structured-output grammars for non-spatial local model consumers
/// (categorical choice — `ImageGridSelection`). Mirrors
/// `qwen3_vl_runtime::Qwen3VlStructuredSchema::StringIdArray` exactly; kept
/// as an independent, self-contained copy rather than a shared cross-model
/// module, matching this frontier's instruction not to redesign or expand
/// structured-generation machinery beyond what this candidate genuinely
/// needs.
#[derive(Clone, Debug, PartialEq)]
pub struct PaligemmaStringIdArraySchema {
    /// JSON object field name.
    pub field: String,
    /// Complete set of legal variable string values.
    pub allowed_ids: Vec<String>,
    /// Whether an empty array is legal.
    pub allow_empty: bool,
}

/// One decoded production inference result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaligemmaGenerationResult {
    /// Decoded generated suffix only.
    pub text: String,
    /// Generated token IDs, excluding prompt tokens.
    pub token_ids: Vec<u32>,
    /// Runtime duration for this request.
    pub elapsed: std::time::Duration,
    /// Immutable processor/runtime identity.
    pub processor_identity: &'static str,
}

/// Observable preprocessing facts required for coordinate and parity checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaligemmaImageMetadata {
    /// Original decoded image dimensions.
    pub original_dimensions: (u32, u32),
    /// Dimensions admitted by the qualified fixed processor envelope
    /// (always `224x224`).
    pub processed_dimensions: (u32, u32),
}

/// Persistent offline construction resources. Mutable model/KV state is
/// request-local inside `PaligemmaGenerationFactory::begin_request`.
///
/// Despite the historical name, execution device/dtype are held generically
/// (`device`, plus the runtime/processor identity strings) rather than
/// hardcoded — `initialize` (CPU/F32) and, under `local_paligemma_cuda`,
/// `initialize_cuda_f16` are two explicit constructors over the same type,
/// never two providers. See
/// `SCORPION_PALIGEMMA_LOCAL_INFERENCE_LATENCY_QUALIFICATION_001`.
pub struct PaligemmaCpuRuntime {
    installation_identity: crate::features::local_model::InstalledModelIdentity,
    factory: PaligemmaGenerationFactory,
    tokenizer: Tokenizer,
    device: Device,
    dtype: DType,
    runtime_identity: &'static str,
    processor_identity: &'static str,
}

impl PaligemmaCpuRuntime {
    /// Initialize exclusively from an already verified canonical installation.
    pub fn initialize(
        installation: &LocalModelInstallation,
        available_ram_bytes: u64,
    ) -> Result<Self, PaligemmaRuntimeFailure> {
        preflight_cpu_resources(available_ram_bytes)?;
        Self::initialize_on_device(
            installation,
            Device::Cpu,
            DType::F32,
            "candle-0.11.0-cpu-f32",
            PALIGEMMA_PROCESSOR_ID,
        )
    }

    /// Truthful identity of the backend that actually executed this
    /// instance — `"candle-0.11.0-cpu-f32"` or, under `local_paligemma_cuda`,
    /// `PALIGEMMA_CUDA_RUNTIME_IDENTITY`.
    pub fn runtime_identity(&self) -> &'static str {
        self.runtime_identity
    }

    /// Truthful processor identity of the backend that actually executed
    /// this instance.
    pub fn processor_identity(&self) -> &'static str {
        self.processor_identity
    }

    /// Shared construction path for every device/dtype combination this
    /// runtime supports. `runtime_identity`/`processor_identity` must match
    /// the installation's own declared identity (`validate_installation_identity`)
    /// so an installation built for one backend can never silently execute
    /// on another.
    fn initialize_on_device(
        installation: &LocalModelInstallation,
        device: Device,
        dtype: DType,
        runtime_identity: &'static str,
        processor_identity: &'static str,
    ) -> Result<Self, PaligemmaRuntimeFailure> {
        installation.reverify()?;
        validate_installation_identity(installation, runtime_identity, processor_identity)?;
        let paths = required_paths(installation)?;
        validate_pinned_json(&paths)?;
        validate_safetensors(&paths[0..3])?;

        // The real pinned config.json omits several architecture constants
        // (attention_bias, head_dim, hidden_act, rms_norm_eps, rope_theta)
        // that the reference HF processor/model fill in from fixed
        // defaults for this exact checkpoint family — generic JSON
        // deserialization of the full `Config` would fail closed on a
        // "missing field" error. `validate_pinned_json` above already
        // confirms the JSON's own declared fields match this exact pinned
        // architecture; the full `Config` is the same one
        // `candle-transformers` itself ships for this checkpoint.
        let config = Config::paligemma_3b_224();
        // SAFETY: `LocalModelInstallation::reverify` has just checked the full
        // file digest and immutable membership. The installation cannot be
        // activated partially, and this runtime retains no mutable file API.
        // The pinned shards are F32 on disk (`validate_safetensors` enforces
        // this regardless of `dtype`); candle converts per-tensor from that
        // source dtype to `dtype` directly during `VarBuilder::get`, without
        // an intermediate full-precision copy of the whole model.
        let weights = unsafe {
            VarBuilder::from_mmaped_safetensors(&[&paths[0], &paths[1], &paths[2]], dtype, &device)
        }
        .map_err(|e| PaligemmaRuntimeFailure::Initialization(e.to_string()))?;
        let factory = PaligemmaGenerationFactory::new(config, weights);
        let tokenizer = Tokenizer::from_file(&paths[4])
            .map_err(|e| PaligemmaRuntimeFailure::Tokenization(e.to_string()))?;
        Ok(Self {
            installation_identity: installation.identity().clone(),
            factory,
            tokenizer,
            device,
            dtype,
            runtime_identity,
            processor_identity,
        })
    }

    /// Initialize using current available host memory. No device selection or
    /// fallback occurs: this adapter is CPU/F32 only.
    pub fn initialize_from_host(
        installation: &LocalModelInstallation,
    ) -> Result<Self, PaligemmaRuntimeFailure> {
        let mut system = System::new();
        system.refresh_memory();
        Self::initialize(installation, system.available_memory())
    }

    /// Initialize the accelerated CUDA/F16 backend from an explicit host RAM
    /// reading and an already-established CUDA device. Fails closed — no
    /// CPU fallback is ever attempted here: a caller that gets `Err` has an
    /// unstarted runtime, never a silently downgraded one.
    #[cfg(feature = "local_paligemma_cuda")]
    pub fn initialize_cuda_f16(
        installation: &LocalModelInstallation,
        available_ram_bytes: u64,
        device: Device,
    ) -> Result<Self, PaligemmaRuntimeFailure> {
        if !device.is_cuda() {
            return Err(PaligemmaRuntimeFailure::DeviceUnavailable(
                "initialize_cuda_f16 requires a CUDA device".into(),
            ));
        }
        preflight_cpu_resources_cuda(available_ram_bytes)?;
        Self::initialize_on_device(
            installation,
            device,
            DType::F16,
            PALIGEMMA_CUDA_RUNTIME_IDENTITY,
            PALIGEMMA_CUDA_PROCESSOR_ID,
        )
    }

    /// Initialize the accelerated CUDA/F16 backend using current available
    /// host RAM and current free device VRAM. No CPU fallback: a host
    /// without a working CUDA device, or one without the qualified free
    /// VRAM floor, fails closed with `PaligemmaRuntimeFailure`, never a
    /// silent downgrade to the CPU/F32 path.
    #[cfg(feature = "local_paligemma_cuda")]
    pub fn initialize_cuda_f16_from_host(
        installation: &LocalModelInstallation,
    ) -> Result<Self, PaligemmaRuntimeFailure> {
        let mut system = System::new();
        system.refresh_memory();
        let available_ram_bytes = system.available_memory();
        // `Device::new_cuda` establishes (and binds to this thread) the CUDA
        // context that `cuMemGetInfo` reads; candle-core exposes no
        // higher-level free-VRAM query, so this one raw driver call is made
        // directly against the same `cudarc` version candle-core itself
        // depends on for its `cuda` feature. The device is queried for VRAM
        // *before* any weight is loaded onto it, then reused (not
        // recreated) for the real load — never a second, separate context.
        let device = Device::new_cuda(0)
            .map_err(|e| PaligemmaRuntimeFailure::DeviceUnavailable(e.to_string()))?;
        let (available_vram_bytes, _total) = cudarc::driver::result::mem_get_info()
            .map_err(|e| PaligemmaRuntimeFailure::DeviceUnavailable(e.to_string()))?;
        preflight_cuda_resources(available_vram_bytes as u64)?;
        Self::initialize_cuda_f16(installation, available_ram_bytes, device)
    }

    /// Verified installation identity used by this initialized runtime.
    pub fn installation_identity(&self) -> &crate::features::local_model::InstalledModelIdentity {
        &self.installation_identity
    }

    /// Validate an encoded image through the canonical processor and expose
    /// only dimensions required for provider prompt and bounds semantics.
    pub fn image_metadata(
        &self,
        image_bytes: &[u8],
    ) -> Result<PaligemmaImageMetadata, PaligemmaRuntimeFailure> {
        let processed = process_image(image_bytes, &self.device, self.dtype)?;
        Ok(PaligemmaImageMetadata {
            original_dimensions: processed.original_dimensions,
            processed_dimensions: (IMAGE_SIZE, IMAGE_SIZE),
        })
    }

    /// Real `detect {label}` grounding query: deterministic greedy decode
    /// constrained to exactly four `<locNNNN>` tokens (the model's own
    /// genuine bounding-box answer), then stop — the trailing free-text
    /// label is not needed and is not generated.
    pub async fn detect(
        &self,
        image_bytes: &[u8],
        label: &str,
    ) -> Result<PaligemmaDetectionBox, PaligemmaRuntimeFailure> {
        let processed = process_image(image_bytes, &self.device, self.dtype)?;
        let prompt_ids = build_prompt_tokens(&self.tokenizer, &format!("detect {label}"))?;
        let started = Instant::now();
        let mut session = self
            .factory
            .begin_request()
            .await
            .map_err(|e| PaligemmaRuntimeFailure::Initialization(e.to_string()))?;
        let input = Tensor::new(prompt_ids.as_slice(), &self.device)
            .and_then(|t| t.unsqueeze(0))
            .map_err(|e| PaligemmaRuntimeFailure::Inference(e.to_string()))?;
        let mut logits = session
            .model()
            .setup(&processed.pixel_values, &input)
            .map_err(|e| PaligemmaRuntimeFailure::Inference(e.to_string()))?;
        let mut bins = Vec::with_capacity(4);
        for _ in 0..4 {
            let token = loc_constrained_token(&logits)?;
            bins.push(token - LOC_TOKEN_BASE);
            let next = Tensor::new(&[token], &self.device)
                .and_then(|t| t.unsqueeze(0))
                .map_err(|e| PaligemmaRuntimeFailure::Inference(e.to_string()))?;
            logits = session
                .model()
                .forward(&next)
                .map_err(|e| PaligemmaRuntimeFailure::Inference(e.to_string()))?;
        }
        Ok(PaligemmaDetectionBox {
            y_min: bins[0],
            x_min: bins[1],
            y_max: bins[2],
            x_max: bins[3],
            elapsed: started.elapsed(),
        })
    }

    /// Generate a structured JSON response using
    /// [`PaligemmaStringIdArraySchema`] for categorical (`ImageGridSelection`)
    /// choices. Immediate empty output is a typed failure, never synthetic
    /// success.
    pub async fn generate_structured_ids(
        &self,
        image_bytes: &[u8],
        prompt: &str,
        schema: &PaligemmaStringIdArraySchema,
        maximum_generated_tokens: usize,
    ) -> Result<PaligemmaGenerationResult, PaligemmaRuntimeFailure> {
        if !schema.allow_empty && schema.allowed_ids.is_empty() {
            return Err(PaligemmaRuntimeFailure::NoValidStructuredContinuation);
        }
        // See the identical, documented workaround in
        // `qwen3_vl_runtime`/`qwen3_vl_captcha`: pre-committing the
        // deterministically-forced opening quote works around a real
        // per-token search gap for an isolated opening quote when nothing
        // has been generated yet.
        let assistant_prefill = if schema.allow_empty {
            "{\"selected_ids\":[".to_string()
        } else {
            "{\"selected_ids\":[\"".to_string()
        };
        let processed = process_image(image_bytes, &self.device, self.dtype)?;
        let prompt_ids = build_prompt_tokens(
            &self.tokenizer,
            &format!("{prompt} Return JSON only: {{\"selected_ids\":[\"id\"]}}."),
        )?;
        let prefill_ids = self
            .tokenizer
            .encode(assistant_prefill.as_str(), false)
            .map_err(|e| PaligemmaRuntimeFailure::Tokenization(e.to_string()))?
            .get_ids()
            .to_vec();
        let mut full_ids = prompt_ids;
        full_ids.extend_from_slice(&prefill_ids);
        let started = Instant::now();
        let mut session = self
            .factory
            .begin_request()
            .await
            .map_err(|e| PaligemmaRuntimeFailure::Initialization(e.to_string()))?;
        let input = Tensor::new(full_ids.as_slice(), &self.device)
            .and_then(|t| t.unsqueeze(0))
            .map_err(|e| PaligemmaRuntimeFailure::Inference(e.to_string()))?;
        let mut logits = session
            .model()
            .setup(&processed.pixel_values, &input)
            .map_err(|e| PaligemmaRuntimeFailure::Inference(e.to_string()))?;
        let mut generated: Vec<u32> = Vec::with_capacity(maximum_generated_tokens);
        for step in 0..maximum_generated_tokens {
            let text = decode_structured(&self.tokenizer, &assistant_prefill, &generated)?;
            if id_array_grammar_state(&strip_field_prefix(&text, &schema.field)?, schema)
                == GrammarState::Complete
            {
                break;
            }
            let token = id_array_constrained_token(
                &logits,
                &self.tokenizer,
                &assistant_prefill,
                &generated,
                schema,
                step + 1 == maximum_generated_tokens,
            )?;
            if token == EOS_TOKEN_ID {
                break;
            }
            generated.push(token);
            let next = Tensor::new(&[token], &self.device)
                .and_then(|t| t.unsqueeze(0))
                .map_err(|e| PaligemmaRuntimeFailure::Inference(e.to_string()))?;
            logits = session
                .model()
                .forward(&next)
                .map_err(|e| PaligemmaRuntimeFailure::Inference(e.to_string()))?;
        }
        if generated.is_empty() {
            return Err(PaligemmaRuntimeFailure::Inference(
                "structured generation ended before producing a suffix".into(),
            ));
        }
        let generated_text = self
            .tokenizer
            .decode(&generated, true)
            .map_err(|e| PaligemmaRuntimeFailure::Tokenization(e.to_string()))?;
        let text = format!("{assistant_prefill}{generated_text}");
        let stripped = strip_field_prefix(&text, &schema.field)?;
        if id_array_grammar_state(&stripped, schema) != GrammarState::Complete {
            return Err(PaligemmaRuntimeFailure::NoValidStructuredContinuation);
        }
        Ok(PaligemmaGenerationResult {
            text,
            token_ids: generated,
            elapsed: started.elapsed(),
            processor_identity: self.processor_identity,
        })
    }

    /// Consume the runtime and release its mmap/tokenizer/factory references.
    pub fn unload(self) {
        self.factory.unload();
    }
}

/// Fail-closed CPU resource preflight.
pub fn preflight_cpu_resources(available_ram_bytes: u64) -> Result<(), PaligemmaRuntimeFailure> {
    if available_ram_bytes < PALIGEMMA_MINIMUM_RAM_BYTES {
        return Err(PaligemmaRuntimeFailure::ResourceLimitExceeded {
            available: available_ram_bytes,
            required: PALIGEMMA_MINIMUM_RAM_BYTES,
        });
    }
    Ok(())
}

/// Fail-closed host-RAM preflight for the CUDA/F16 backend. Distinct floor
/// from `preflight_cpu_resources`: weights do not remain resident in host
/// RAM once uploaded, so the qualified floor is real-measured, not the same
/// constant as the CPU/F32 path.
#[cfg(feature = "local_paligemma_cuda")]
pub fn preflight_cpu_resources_cuda(
    available_ram_bytes: u64,
) -> Result<(), PaligemmaRuntimeFailure> {
    if available_ram_bytes < PALIGEMMA_CUDA_MINIMUM_RAM_BYTES {
        return Err(PaligemmaRuntimeFailure::ResourceLimitExceeded {
            available: available_ram_bytes,
            required: PALIGEMMA_CUDA_MINIMUM_RAM_BYTES,
        });
    }
    Ok(())
}

/// Fail-closed device (VRAM) preflight for the CUDA/F16 backend.
#[cfg(feature = "local_paligemma_cuda")]
pub fn preflight_cuda_resources(available_vram_bytes: u64) -> Result<(), PaligemmaRuntimeFailure> {
    if available_vram_bytes < PALIGEMMA_CUDA_MINIMUM_VRAM_BYTES {
        return Err(PaligemmaRuntimeFailure::DeviceMemoryLimitExceeded {
            available: available_vram_bytes,
            required: PALIGEMMA_CUDA_MINIMUM_VRAM_BYTES,
        });
    }
    Ok(())
}

fn validate_installation_identity(
    installation: &LocalModelInstallation,
    expected_runtime_identity: &str,
    expected_processor_identity: &str,
) -> Result<(), PaligemmaRuntimeFailure> {
    let identity = installation.identity();
    if identity.model.repository != "google/paligemma-3b-mix-224"
        || identity.model.revision != PALIGEMMA_MODEL_REVISION
        || identity.runtime != expected_runtime_identity
        || identity.preprocessing != expected_processor_identity
    {
        return Err(PaligemmaRuntimeFailure::InvalidArtifact(
            "installed identity",
        ));
    }
    Ok(())
}

fn required_paths(
    installation: &LocalModelInstallation,
) -> Result<Vec<PathBuf>, PaligemmaRuntimeFailure> {
    REQUIRED_ARTIFACTS
        .iter()
        .map(|name| {
            installation
                .artifact_path(Path::new(name))
                .map_err(Into::into)
        })
        .collect()
}

fn validate_pinned_json(paths: &[PathBuf]) -> Result<(), PaligemmaRuntimeFailure> {
    let config: Value = read_json(&paths[3], "config.json")?;
    let preprocessor: Value = read_json(&paths[6], "preprocessor_config.json")?;
    let tokenizer_config: Value = read_json(&paths[5], "tokenizer_config.json")?;
    let correct = config["model_type"] == "paligemma"
        && config["image_token_index"] == IMAGE_TOKEN_ID
        && config["text_config"]["model_type"] == "gemma"
        && config["text_config"]["hidden_size"] == 2048
        && config["text_config"]["num_hidden_layers"] == 18
        && config["text_config"]["num_attention_heads"] == 8
        && config["text_config"]["num_key_value_heads"] == 1
        && config["text_config"]["vocab_size"] == 257216
        && config["vision_config"]["model_type"] == "siglip_vision_model"
        && config["vision_config"]["hidden_size"] == 1152
        && config["vision_config"]["num_hidden_layers"] == 27
        && config["vision_config"]["patch_size"] == PATCH_SIZE
        && preprocessor["image_seq_length"] == IMAGE_SEQ_LEN
        && preprocessor["size"]["height"] == IMAGE_SIZE
        && preprocessor["size"]["width"] == IMAGE_SIZE
        && tokenizer_config["tokenizer_class"] == "GemmaTokenizer";
    if !correct {
        return Err(PaligemmaRuntimeFailure::InvalidArtifact(
            "pinned JSON semantics",
        ));
    }
    Ok(())
}

fn read_json(path: &Path, name: &'static str) -> Result<Value, PaligemmaRuntimeFailure> {
    serde_json::from_slice(
        &std::fs::read(path).map_err(|_| PaligemmaRuntimeFailure::InvalidArtifact(name))?,
    )
    .map_err(|_| PaligemmaRuntimeFailure::InvalidArtifact(name))
}

fn validate_safetensors(weight_paths: &[PathBuf]) -> Result<(), PaligemmaRuntimeFailure> {
    // SAFETY: installation verification authenticated every complete file first.
    let tensors = unsafe { candle::safetensors::MmapedSafetensors::multi(weight_paths) }
        .map_err(|e| PaligemmaRuntimeFailure::Initialization(e.to_string()))?;
    let views = tensors.tensors();
    if views.len() != EXPECTED_TENSORS
        || views
            .iter()
            .any(|(_, tensor)| !matches!(DType::try_from(tensor.dtype()), Ok(DType::F32)))
    {
        return Err(PaligemmaRuntimeFailure::InvalidArtifact(
            "model safetensors shards",
        ));
    }
    Ok(())
}

struct ProcessedImage {
    original_dimensions: (u32, u32),
    pixel_values: Tensor,
}

/// Decode and pack one image according to the pinned PaliGemma contract: a
/// direct (non-aspect-preserving) resize to the fixed 224x224 envelope —
/// `SiglipImageProcessor`'s own real behavior, not a Scorpion approximation
/// — then `(pixel/255 - 0.5) / 0.5` normalization (identical formula to the
/// pinned Qwen3-VL processor). Normalization itself always happens in F32
/// (matching the reference processor's own arithmetic); the result is then
/// cast to the runtime's own weight dtype so the vision tower's first
/// `conv2d` never sees a dtype mismatch against F16 (or any future
/// non-F32) weights.
fn process_image(
    encoded: &[u8],
    device: &Device,
    dtype: DType,
) -> Result<ProcessedImage, PaligemmaRuntimeFailure> {
    let image = image::load_from_memory(encoded)
        .map_err(|e| PaligemmaRuntimeFailure::Processing(e.to_string()))?;
    let original_dimensions = image.dimensions();
    let rgb = image
        .resize_exact(IMAGE_SIZE, IMAGE_SIZE, FilterType::CatmullRom)
        .to_rgb8();
    let mut packed = Vec::with_capacity(3 * (IMAGE_SIZE as usize) * (IMAGE_SIZE as usize));
    for channel in 0..3 {
        for y in 0..IMAGE_SIZE {
            for x in 0..IMAGE_SIZE {
                let value = f32::from(rgb.get_pixel(x, y)[channel]);
                packed.push((value / 255.0 - 0.5) / 0.5);
            }
        }
    }
    let pixel_values = Tensor::from_vec(
        packed,
        (1, 3, IMAGE_SIZE as usize, IMAGE_SIZE as usize),
        device,
    )
    .and_then(|t| t.to_dtype(dtype))
    .map_err(|e| PaligemmaRuntimeFailure::Processing(e.to_string()))?;
    Ok(ProcessedImage {
        original_dimensions,
        pixel_values,
    })
}

/// Build the pinned reference `PaliGemmaProcessor` sequence
/// (`f"{image_token * image_seq_len}{bos_token}{prompt}\n"`), validate its
/// framing, then return only the text suffix (`<bos>` + `{prompt}\n`) —
/// `<image>` and `<bos>` are recognized as literal added-token substrings by
/// the tokenizer's own added-vocabulary matching (the same mechanism
/// `qwen3_vl_runtime::build_prompt_tokens` already relies on for
/// `<|image_pad|>`), so a single `encode(..., add_special_tokens=false)`
/// call — bypassing the tokenizer's own `TemplateProcessing` post-processor,
/// which would otherwise inject a second, redundant BOS — produces the
/// exact reference sequence, letting this validate real tokenization
/// end-to-end even though the leading image placeholders are discarded
/// afterward.
///
/// The 256 leading `<image>` placeholder ids are discarded, not fed to the
/// model: `candle_transformers::models::paligemma::Model::setup` takes the
/// real 256-position vision-tower image embeddings and the text-token
/// embeddings as two separate tensors and concatenates them itself
/// (`Tensor::cat(&[image_features, text_features], 1)`); passing the
/// placeholder ids' own (semantically meaningless) text embeddings as well
/// would duplicate the image span into a 517-position sequence instead of
/// the reference's real 261, corrupting every downstream position and
/// producing incoherent output — confirmed empirically (a first attempt
/// that passed the full 261-token sequence, including the placeholders,
/// produced degenerate near-`1023` bin values regardless of true target
/// position).
fn build_prompt_tokens(
    tokenizer: &Tokenizer,
    prompt: &str,
) -> Result<Vec<u32>, PaligemmaRuntimeFailure> {
    let rendered = format!("{}{}{prompt}\n", "<image>".repeat(IMAGE_SEQ_LEN), "<bos>");
    let encoding = tokenizer
        .encode(rendered, false)
        .map_err(|e| PaligemmaRuntimeFailure::Tokenization(e.to_string()))?;
    let ids = encoding.get_ids().to_vec();
    let image_count = ids.iter().filter(|id| **id == IMAGE_TOKEN_ID).count();
    if image_count != IMAGE_SEQ_LEN || ids.first() != Some(&IMAGE_TOKEN_ID) {
        return Err(PaligemmaRuntimeFailure::InvalidArtifact(
            "image token framing",
        ));
    }
    if !ids
        .get(IMAGE_SEQ_LEN..IMAGE_SEQ_LEN + 1)
        .is_some_and(|s| s == [BOS_TOKEN_ID])
    {
        return Err(PaligemmaRuntimeFailure::InvalidArtifact("bos framing"));
    }
    Ok(ids[IMAGE_SEQ_LEN..].to_vec())
}

/// The final position's logits, always as F32 — greedy argmax/ranking
/// downstream is done in F32 regardless of the runtime's own compute dtype
/// (mirroring the model's own internal norm upcast pattern), so an F16/BF16
/// runtime never has to special-case its own token-selection code.
fn last_logits(logits: &Tensor) -> Result<Tensor, PaligemmaRuntimeFailure> {
    match logits.dims() {
        [_, sequence, _] => logits.i((0, sequence - 1)),
        [sequence, _] => logits.i(sequence - 1),
        _ => Err(candle::Error::Msg("unexpected logits shape".into())),
    }
    .and_then(|t| t.to_dtype(DType::F32))
    .map_err(|e| PaligemmaRuntimeFailure::Inference(e.to_string()))
}

/// Greedy-select the highest-logit token whose id falls in the 1024-entry
/// `<locNNNN>` range. Every location bin is always a syntactically legal
/// continuation (there is no partial-token state to track, unlike JSON), so
/// this is a plain masked argmax rather than a per-candidate grammar walk.
fn loc_constrained_token(logits: &Tensor) -> Result<u32, PaligemmaRuntimeFailure> {
    let last = last_logits(logits)?;
    let values = last
        .to_vec1::<f32>()
        .map_err(|e| PaligemmaRuntimeFailure::Inference(e.to_string()))?;
    let base = LOC_TOKEN_BASE as usize;
    let count = LOC_TOKEN_COUNT as usize;
    let slice = values
        .get(base..base + count)
        .ok_or(PaligemmaRuntimeFailure::NoValidStructuredContinuation)?;
    let (best_index, _) = slice
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .ok_or(PaligemmaRuntimeFailure::NoValidStructuredContinuation)?;
    Ok(LOC_TOKEN_BASE + best_index as u32)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GrammarState {
    Invalid,
    Prefix,
    Complete,
}

fn decode_structured(
    tokenizer: &Tokenizer,
    assistant_prefill: &str,
    generated: &[u32],
) -> Result<String, PaligemmaRuntimeFailure> {
    let suffix = tokenizer
        .decode(generated, true)
        .map_err(|e| PaligemmaRuntimeFailure::Tokenization(e.to_string()))?;
    Ok(format!("{assistant_prefill}{suffix}"))
}

fn strip_field_prefix(text: &str, field: &str) -> Result<String, PaligemmaRuntimeFailure> {
    let opening = format!("{{{}:[", serde_json::to_string(field).unwrap());
    if text.starts_with(&opening) {
        Ok(text[opening.len()..].to_string())
    } else if opening.starts_with(text) {
        Ok(String::new())
    } else {
        Err(PaligemmaRuntimeFailure::NoValidStructuredContinuation)
    }
}

fn id_array_grammar_state(rest: &str, schema: &PaligemmaStringIdArraySchema) -> GrammarState {
    id_array_state(rest, &schema.allowed_ids, &[], schema.allow_empty)
}

fn id_array_state(
    input: &str,
    allowed: &[String],
    used: &[usize],
    allow_empty: bool,
) -> GrammarState {
    if input.is_empty() {
        return GrammarState::Prefix;
    }
    if (allow_empty || !used.is_empty()) && "]}".starts_with(input) {
        return if input == "]}" {
            GrammarState::Complete
        } else {
            GrammarState::Prefix
        };
    }
    for (index, id) in allowed.iter().enumerate() {
        if used.contains(&index) {
            continue;
        }
        let encoded = serde_json::to_string(id).expect("string JSON encoding is infallible");
        if encoded.starts_with(input) {
            return GrammarState::Prefix;
        }
        let Some(rest) = input.strip_prefix(&encoded) else {
            continue;
        };
        let mut next_used = used.to_vec();
        next_used.push(index);
        if rest.is_empty() {
            return GrammarState::Prefix;
        }
        if let Some(next) = rest.strip_prefix(',') {
            let state = id_array_state(next, allowed, &next_used, false);
            if state != GrammarState::Invalid {
                return state;
            }
        }
        if "]}".starts_with(rest) {
            return if rest == "]}" {
                GrammarState::Complete
            } else {
                GrammarState::Prefix
            };
        }
    }
    GrammarState::Invalid
}

#[allow(clippy::too_many_arguments)]
fn id_array_constrained_token(
    logits: &Tensor,
    tokenizer: &Tokenizer,
    assistant_prefill: &str,
    generated: &[u32],
    schema: &PaligemmaStringIdArraySchema,
    must_complete: bool,
) -> Result<u32, PaligemmaRuntimeFailure> {
    let last = last_logits(logits)?;
    let values = last
        .to_vec1::<f32>()
        .map_err(|e| PaligemmaRuntimeFailure::Inference(e.to_string()))?;
    let mut ranked: Vec<usize> = (0..values.len()).collect();
    ranked.sort_unstable_by(|left, right| values[*right].total_cmp(&values[*left]));
    for token in ranked {
        let token = token as u32;
        if token == EOS_TOKEN_ID {
            continue;
        }
        let mut candidate = generated.to_vec();
        candidate.push(token);
        let text = decode_structured(tokenizer, assistant_prefill, &candidate)?;
        let stripped = strip_field_prefix(&text, &schema.field)?;
        let state = id_array_grammar_state(&stripped, schema);
        if state != GrammarState::Invalid && (!must_complete || state == GrammarState::Complete) {
            return Ok(token);
        }
    }
    Err(PaligemmaRuntimeFailure::NoValidStructuredContinuation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageBuffer, Rgb};
    use std::io::Cursor;

    fn fixture_png() -> Vec<u8> {
        let image = ImageBuffer::from_fn(224, 224, |x, y| {
            Rgb([(x % 255) as u8, (y % 255) as u8, ((x + y) % 255) as u8])
        });
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(image)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn pinned_processor_shape_matches_reference_fixture() {
        let output = process_image(&fixture_png(), &Device::Cpu, DType::F32).unwrap();
        assert_eq!(output.original_dimensions, (224, 224));
        assert_eq!(output.pixel_values.dims(), &[1, 3, 224, 224]);
    }

    #[test]
    fn resource_preflight_is_fail_closed() {
        assert!(matches!(
            preflight_cpu_resources(PALIGEMMA_MINIMUM_RAM_BYTES - 1),
            Err(PaligemmaRuntimeFailure::ResourceLimitExceeded { .. })
        ));
        assert!(preflight_cpu_resources(PALIGEMMA_MINIMUM_RAM_BYTES).is_ok());
    }

    #[test]
    fn id_array_grammar_validates_variable_ids_and_dead_ends() {
        let schema = PaligemmaStringIdArraySchema {
            field: "selected_ids".into(),
            allowed_ids: vec!["cell-alpha".into(), "cell-42".into()],
            allow_empty: true,
        };
        assert_eq!(
            id_array_grammar_state(
                &strip_field_prefix(
                    "{\"selected_ids\":[\"cell-alpha\",\"cell-42\"]}",
                    &schema.field
                )
                .unwrap(),
                &schema
            ),
            GrammarState::Complete
        );
        assert_eq!(
            id_array_grammar_state(
                &strip_field_prefix("{\"selected_ids\":[\"unknown\"", &schema.field).unwrap(),
                &schema
            ),
            GrammarState::Invalid
        );
    }

    #[test]
    fn detection_box_center_is_the_bin_midpoint() {
        let box_ = PaligemmaDetectionBox {
            y_min: 400,
            x_min: 440,
            y_max: 600,
            x_max: 580,
            elapsed: std::time::Duration::from_secs(0),
        };
        assert_eq!(box_.center_bins(), (510.0, 500.0));
    }

    fn square_fixture(width: u32, height: u32, cx: u32, cy: u32, side: u32) -> Vec<u8> {
        let half = side / 2;
        let mut canvas = ImageBuffer::from_pixel(width, height, Rgb([40u8, 40, 40]));
        for y in cy.saturating_sub(half)..(cy + half).min(height) {
            for x in cx.saturating_sub(half)..(cx + half).min(width) {
                canvas.put_pixel(x, y, Rgb([220, 40, 40]));
            }
        }
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(canvas)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    /// Real qualification-host reference-parity proof. Acquisition remains
    /// external; the runtime receives only a canonical verified installation.
    /// Requires the pinned checkpoint at
    /// `$SCORPION_PALIGEMMA_PINNED_ARTIFACTS`.
    ///
    /// Exact-value assertion (not merely "non-empty" or "contains X"): the
    /// real greedy `detect` output for this fixture was independently
    /// verified against the pinned HuggingFace `transformers` reference
    /// oracle running the identical weights/config/image/prompt — see the
    /// frontier SDD — and matched bit-for-bit
    /// (`<loc0407><loc0441><loc0614><loc0579>`). Reproducing that exact
    /// value here is a much stronger regression guard than substring
    /// matching: any future drift in preprocessing, prompt framing,
    /// embedding scale, or attention masking changes these four integers.
    #[tokio::test]
    #[ignore = "requires the pinned ~11.7 GB offline PaliGemma installation"]
    async fn real_offline_detect_reproduces_reference_and_reinitializes() {
        let source = PathBuf::from(
            std::env::var("SCORPION_PALIGEMMA_PINNED_ARTIFACTS")
                .expect("set pinned offline artifact directory"),
        );
        let parent = tempfile::tempdir_in(source.parent().unwrap()).unwrap();
        let staging = parent.path().join("staging");
        let active = parent.path().join("active");
        std::fs::create_dir(&staging).unwrap();
        for name in REQUIRED_ARTIFACTS {
            std::fs::hard_link(source.join(name), staging.join(name)).unwrap();
        }
        let installation = paligemma_cpu_f32_manifest()
            .activate(&staging, &active)
            .unwrap();
        let target_fixture = square_fixture(320, 224, 160, 112, 44);
        let distractor_fixture = square_fixture(320, 224, 60, 60, 44);

        let mut boxes = Vec::new();
        for _ in 0..2 {
            let runtime = PaligemmaCpuRuntime::initialize_from_host(&installation).unwrap();
            let box_ = runtime.detect(&target_fixture, "red square").await.unwrap();
            assert_eq!(
                (box_.y_min, box_.x_min, box_.y_max, box_.x_max),
                (407, 441, 614, 579),
                "expected an exact match with the independently-verified pinned \
                 reference oracle output for this fixture"
            );
            boxes.push((box_.y_min, box_.x_min, box_.y_max, box_.x_max));

            // A materially different target position must produce a
            // materially different box — proves genuine, content-dependent
            // localization, not a fixed answer.
            let other = runtime
                .detect(&distractor_fixture, "red square")
                .await
                .unwrap();
            assert_ne!(
                (other.y_min, other.x_min, other.y_max, other.x_max),
                *boxes.last().unwrap(),
                "a target at a materially different position must not produce \
                 the identical bounding box"
            );
            runtime.unload();
        }
        // Deterministic greedy decoding: repeated isolated sessions with the
        // identical input must produce the identical output.
        assert_eq!(boxes[0], boxes[1]);
    }

    /// Real pinned-model proof that `ImageGridSelection`-style structured
    /// JSON generation produces a valid, strictly-parsed suffix.
    #[tokio::test]
    #[ignore = "requires the pinned ~11.7 GB offline PaliGemma installation"]
    async fn real_structured_ids_generation_is_valid_and_deterministic() {
        let source = PathBuf::from(
            std::env::var("SCORPION_PALIGEMMA_PINNED_ARTIFACTS")
                .expect("set pinned offline artifact directory"),
        );
        let parent = tempfile::tempdir_in(source.parent().unwrap()).unwrap();
        let staging = parent.path().join("staging");
        let active = parent.path().join("active");
        std::fs::create_dir(&staging).unwrap();
        for name in REQUIRED_ARTIFACTS {
            std::fs::hard_link(source.join(name), staging.join(name)).unwrap();
        }
        let installation = paligemma_cpu_f32_manifest()
            .activate(&staging, &active)
            .unwrap();
        let runtime = PaligemmaCpuRuntime::initialize_from_host(&installation).unwrap();
        let schema = PaligemmaStringIdArraySchema {
            field: "selected_ids".into(),
            allowed_ids: vec!["cell-alpha".into(), "cell-beta".into()],
            allow_empty: false,
        };
        let fixture = square_fixture(224, 224, 160, 112, 44);
        let prompt = "The red square is in cell-beta. Choose the cell containing the red square.";
        let result = runtime
            .generate_structured_ids(&fixture, prompt, &schema, 24)
            .await
            .unwrap();
        assert!(!result.token_ids.is_empty());
        let parsed: serde_json::Value = serde_json::from_str(&result.text).unwrap();
        let selected = parsed["selected_ids"].as_array().unwrap();
        assert!(!selected.is_empty());
        assert!(selected
            .iter()
            .all(|id| matches!(id.as_str(), Some("cell-alpha" | "cell-beta"))));

        let repeated = runtime
            .generate_structured_ids(&fixture, prompt, &schema, 24)
            .await
            .unwrap();
        assert_eq!(result.text, repeated.text);
        runtime.unload();
    }
}
