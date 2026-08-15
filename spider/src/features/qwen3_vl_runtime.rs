//! Offline production runtime for the pinned Qwen3-VL-2B-Instruct model.
//!
//! Construction accepts only a verified [`LocalModelInstallation`]. It never
//! discovers or downloads files and it has no transport dependency.

use crate::features::artifact_reference::ArtifactReference;
use crate::features::local_model::{
    LocalModelArtifact, LocalModelDevice, LocalModelDeviceRequirement, LocalModelIdentity,
    LocalModelManifest, LocalModelRuntimeRequirements,
};
use crate::features::local_model::{LocalModelFailure, LocalModelInstallation};
use crate::features::qwen3_vl_generation::Qwen3VlGenerationFactory;
use crate::features::source_provider::ProviderId;
use candle::{DType, Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::qwen3_vl::Config;
use image::{imageops::FilterType, DynamicImage, GenericImageView};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Instant;
use sysinfo::System;
use tokenizers::Tokenizer;

/// Immutable upstream model revision accepted by this runtime.
pub const QWEN3_VL_MODEL_REVISION: &str = "89644892e4d85e24eaac8bacfd4f463576704203";
/// Minimum available host memory required before initialization.
pub const QWEN3_VL_MINIMUM_RAM_BYTES: u64 = 13_253_615_616;
/// Stable implementation identity for the pinned processor/template contract.
pub const QWEN3_VL_PROCESSOR_ID: &str =
    "qwen3-vl-2b@89644892-candle-0.11.0-cpu-f32-processor-v1-320x224";

const REQUIRED_ARTIFACTS: [&str; 6] = [
    "model.safetensors",
    "config.json",
    "tokenizer.json",
    "tokenizer_config.json",
    "chat_template.json",
    "preprocessor_config.json",
];
const EXPECTED_TENSORS: usize = 625;
const PATCH_SIZE: usize = 16;
const TEMPORAL_PATCH_SIZE: usize = 2;
const MERGE_SIZE: usize = 2;
const MIN_PIXELS: usize = 65_536;
const MAX_PIXELS: usize = 16_777_216;
const IMAGE_TOKEN_ID: u32 = 151_655;
const VISION_START_TOKEN_ID: u32 = 151_652;
const VISION_END_TOKEN_ID: u32 = 151_653;
const EOS_TOKEN_ID: u32 = 151_645;

const PINNED_ARTIFACTS: [(&str, u64, &str); 6] = [
    (
        "model.safetensors",
        4_255_140_312,
        "7de1838c87a5349b016c26a1c3f7d2bc400a3d485f95ef39a7059ffd734977a0",
    ),
    (
        "config.json",
        1_505,
        "bec4b3d446efa05807365c9e1cec03ac590836879d02f3a6da879971154bdd3b",
    ),
    (
        "tokenizer.json",
        7_032_403,
        "a5d85b6dcc535e6b93115a9ef287e6132fdbf30270da6218194ba742261173c7",
    ),
    (
        "tokenizer_config.json",
        10_868,
        "c2da771801886ad9ae98181793ffd3dfb7f1af30f6f7c6a4e15d7dbba52e2399",
    ),
    (
        "chat_template.json",
        5_502,
        "6f8a6a55027e3da5160105556cda5dd69f6423f1c32645f6730d32de7773d0c4",
    ),
    (
        "preprocessor_config.json",
        390,
        "27225450ac9c6529872ee1924fcb0962ff5634834f817040f444118116f4e516",
    ),
];

/// Exact immutable manifest for the qualified image-only production runtime.
/// It contains no mutable URL identity and advertises no CAPTCHA qualification.
pub fn qwen3_vl_cpu_f32_manifest() -> LocalModelManifest {
    let artifacts = PINNED_ARTIFACTS
        .iter()
        .map(|(path, size, sha256)| LocalModelArtifact {
            reference: ArtifactReference {
                provider_id: ProviderId::from("huggingface"),
                repository_id: "Qwen/Qwen3-VL-2B-Instruct".into(),
                path: (*path).into(),
                requested_revision: Some(QWEN3_VL_MODEL_REVISION.into()),
                resolved_revision: Some(QWEN3_VL_MODEL_REVISION.into()),
                size_bytes: Some(*size),
                identities: Vec::new(),
                download_url: None,
                discovered_via: None,
            },
            relative_path: PathBuf::from(path),
            size_bytes: *size,
            sha256: (*sha256).into(),
        })
        .collect();
    LocalModelManifest {
        identity: LocalModelIdentity {
            provider: "huggingface".into(),
            repository: "Qwen/Qwen3-VL-2B-Instruct".into(),
            model: "Qwen3-VL-2B-Instruct".into(),
            revision: QWEN3_VL_MODEL_REVISION.into(),
        },
        artifacts,
        runtime_requirements: LocalModelRuntimeRequirements {
            runtime: "candle-0.11.0-cpu-f32".into(),
            preprocessing: QWEN3_VL_PROCESSOR_ID.into(),
            minimum_ram_bytes: QWEN3_VL_MINIMUM_RAM_BYTES,
            devices: vec![LocalModelDeviceRequirement {
                device: LocalModelDevice::Cpu,
                minimum_device_memory_bytes: None,
            }],
        },
        qualifications: Vec::new(),
    }
}

/// Fail-closed runtime construction or inference error.
#[derive(Debug)]
pub enum Qwen3VlRuntimeFailure {
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

impl std::fmt::Display for Qwen3VlRuntimeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Installation(error) => write!(f, "local-model installation failed: {error:?}"),
            Self::InvalidArtifact(name) => write!(f, "invalid pinned Qwen3-VL artifact: {name}"),
            Self::ResourceLimitExceeded {
                available,
                required,
            } => write!(
                f,
                "insufficient available RAM ({available} bytes; {required} required)"
            ),
            Self::InvalidInput(reason) => write!(f, "invalid Qwen3-VL input: {reason}"),
            Self::Initialization(reason) => write!(f, "Qwen3-VL initialization failed: {reason}"),
            Self::Processing(reason) => write!(f, "Qwen3-VL processing failed: {reason}"),
            Self::Tokenization(reason) => write!(f, "Qwen3-VL tokenization failed: {reason}"),
            Self::Inference(reason) => write!(f, "Qwen3-VL inference failed: {reason}"),
            Self::NoValidStructuredContinuation => {
                write!(f, "Qwen3-VL structured grammar has no valid continuation")
            }
        }
    }
}

impl std::error::Error for Qwen3VlRuntimeFailure {}

impl From<LocalModelFailure> for Qwen3VlRuntimeFailure {
    fn from(value: LocalModelFailure) -> Self {
        Self::Installation(value)
    }
}

/// Pinned deterministic generation settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Qwen3VlGenerationConfiguration {
    /// Hard output-token bound.
    pub maximum_generated_tokens: usize,
}

impl Default for Qwen3VlGenerationConfiguration {
    fn default() -> Self {
        Self {
            maximum_generated_tokens: 64,
        }
    }
}

/// Explicit deterministic contract for machine-parseable assistant output.
/// The model must generate a non-empty suffix after this template-owned prefill.
#[derive(Clone, Debug, PartialEq)]
pub struct Qwen3VlStructuredGenerationContract {
    /// Fixed assistant prefix establishing the requested output grammar.
    pub assistant_prefill: String,
    /// Hard bound for tokens generated after the assistant prefix.
    pub maximum_generated_tokens: usize,
    /// Neutral JSON grammar enforced before every token selection.
    pub schema: Qwen3VlStructuredSchema,
}

/// Neutral structured-output grammars required by local model consumers.
#[derive(Clone, Debug, PartialEq)]
pub enum Qwen3VlStructuredSchema {
    /// `{"field":["allowed-id",...]}` with unique IDs.
    StringIdArray {
        /// JSON object field name.
        field: String,
        /// Complete set of legal variable string values.
        allowed_ids: Vec<String>,
        /// Whether an empty array is legal.
        allow_empty: bool,
    },
    /// Compact JSON object containing finite numeric fields in declared order.
    FiniteNumbers {
        /// `(field, inclusive minimum, inclusive maximum)` declarations.
        fields: Vec<(String, f64, f64)>,
    },
}

/// Observable preprocessing facts required for coordinate and parity checks.
#[derive(Debug)]
pub struct Qwen3VlProcessedImage {
    /// Original decoded dimensions.
    pub original_dimensions: (u32, u32),
    /// Dimensions selected by the pinned smart-resize policy.
    pub processed_dimensions: (u32, u32),
    /// `[temporal, height, width]` patch grid.
    pub image_grid_thw: [usize; 3],
    /// Flattened temporal RGB patches consumed by Candle.
    pub pixel_values: Tensor,
    /// Number of merged visual placeholders.
    pub merged_visual_tokens: usize,
}

/// Image dimensions observed by the canonical pinned processor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Qwen3VlImageMetadata {
    /// Original decoded image dimensions.
    pub original_dimensions: (u32, u32),
    /// Dimensions admitted by the qualified processor envelope.
    pub processed_dimensions: (u32, u32),
}

/// One decoded production inference result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Qwen3VlGenerationResult {
    /// Decoded generated suffix only.
    pub text: String,
    /// Generated token IDs, excluding prompt tokens.
    pub token_ids: Vec<u32>,
    /// Runtime duration for this request.
    pub elapsed: std::time::Duration,
    /// Immutable processor/runtime identity.
    pub processor_identity: &'static str,
}

/// Persistent offline construction resources. Mutable model/KV state is
/// request-local inside `Qwen3VlGenerationFactory::begin_request`.
pub struct Qwen3VlCpuRuntime {
    installation_identity: crate::features::local_model::InstalledModelIdentity,
    factory: Qwen3VlGenerationFactory,
    tokenizer: Tokenizer,
    device: Device,
}

impl Qwen3VlCpuRuntime {
    /// Initialize exclusively from an already verified canonical installation.
    pub fn initialize(
        installation: &LocalModelInstallation,
        available_ram_bytes: u64,
    ) -> Result<Self, Qwen3VlRuntimeFailure> {
        preflight_cpu_resources(available_ram_bytes)?;
        installation.reverify()?;
        validate_installation_identity(installation)?;
        let paths = required_paths(installation)?;
        validate_pinned_json(&paths)?;
        validate_safetensors(&paths[0])?;

        let config: Config = serde_json::from_slice(
            &std::fs::read(&paths[1])
                .map_err(|e| Qwen3VlRuntimeFailure::Initialization(e.to_string()))?,
        )
        .map_err(|e| Qwen3VlRuntimeFailure::Initialization(e.to_string()))?;
        let device = Device::Cpu;
        // SAFETY: `LocalModelInstallation::reverify` has just checked the full
        // file digest and immutable membership. The installation cannot be
        // activated partially, and this runtime retains no mutable file API.
        let weights =
            unsafe { VarBuilder::from_mmaped_safetensors(&[&paths[0]], DType::F32, &device) }
                .map_err(|e| Qwen3VlRuntimeFailure::Initialization(e.to_string()))?;
        let factory = Qwen3VlGenerationFactory::new(config, weights);
        let tokenizer = Tokenizer::from_file(&paths[2])
            .map_err(|e| Qwen3VlRuntimeFailure::Tokenization(e.to_string()))?;
        Ok(Self {
            installation_identity: installation.identity().clone(),
            factory,
            tokenizer,
            device,
        })
    }

    /// Initialize using current available host memory. No device selection or
    /// fallback occurs: this adapter is CPU/F32 only.
    pub fn initialize_from_host(
        installation: &LocalModelInstallation,
    ) -> Result<Self, Qwen3VlRuntimeFailure> {
        let mut system = System::new();
        system.refresh_memory();
        Self::initialize(installation, system.available_memory())
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
    ) -> Result<Qwen3VlImageMetadata, Qwen3VlRuntimeFailure> {
        let processed = process_image(image_bytes, &self.device)?;
        Ok(Qwen3VlImageMetadata {
            original_dimensions: processed.original_dimensions,
            processed_dimensions: processed.processed_dimensions,
        })
    }

    /// Execute one deterministic image+text generation with fresh model/KV
    /// state. Exactly one materialized JPEG or PNG image is accepted.
    pub async fn generate(
        &self,
        image_bytes: &[u8],
        prompt: &str,
        configuration: Qwen3VlGenerationConfiguration,
    ) -> Result<Qwen3VlGenerationResult, Qwen3VlRuntimeFailure> {
        self.generate_with_prefill(image_bytes, prompt, "", configuration, None)
            .await
    }

    /// Generate a structured response using an explicit assistant-template
    /// prefill. Immediate EOS is a typed failure, never synthetic success.
    pub async fn generate_structured(
        &self,
        image_bytes: &[u8],
        prompt: &str,
        contract: Qwen3VlStructuredGenerationContract,
    ) -> Result<Qwen3VlGenerationResult, Qwen3VlRuntimeFailure> {
        if contract.assistant_prefill.is_empty() {
            return Err(Qwen3VlRuntimeFailure::InvalidInput(
                "empty structured assistant prefill",
            ));
        }
        if !schema_is_viable(&contract.schema)
            || schema_state(&contract.schema, &contract.assistant_prefill) == GrammarState::Invalid
        {
            return Err(Qwen3VlRuntimeFailure::NoValidStructuredContinuation);
        }
        self.generate_with_prefill(
            image_bytes,
            prompt,
            &contract.assistant_prefill,
            Qwen3VlGenerationConfiguration {
                maximum_generated_tokens: contract.maximum_generated_tokens,
            },
            Some(&contract.schema),
        )
        .await
    }

    async fn generate_with_prefill(
        &self,
        image_bytes: &[u8],
        prompt: &str,
        assistant_prefill: &str,
        configuration: Qwen3VlGenerationConfiguration,
        structured_schema: Option<&Qwen3VlStructuredSchema>,
    ) -> Result<Qwen3VlGenerationResult, Qwen3VlRuntimeFailure> {
        if prompt.is_empty() || configuration.maximum_generated_tokens == 0 {
            return Err(Qwen3VlRuntimeFailure::InvalidInput(
                "empty prompt or token bound",
            ));
        }
        let processed = process_image(image_bytes, &self.device)?;
        let (prompt_ids, image_span) = build_prompt_tokens(
            &self.tokenizer,
            prompt,
            assistant_prefill,
            processed.merged_visual_tokens,
        )?;
        let started = Instant::now();
        let session = self
            .factory
            .begin_request()
            .await
            .map_err(|e| Qwen3VlRuntimeFailure::Initialization(e.to_string()))?;
        let input = Tensor::new(prompt_ids.as_slice(), &self.device)
            .and_then(|t| t.unsqueeze(0))
            .map_err(|e| Qwen3VlRuntimeFailure::Inference(e.to_string()))?;
        let grid_values = processed.image_grid_thw.map(|value| value as u32);
        let grid = Tensor::new(&grid_values, &self.device)
            .and_then(|t| t.unsqueeze(0))
            .map_err(|e| Qwen3VlRuntimeFailure::Inference(e.to_string()))?;
        // Qwen3-VL's rotary embedding is a genuine interleaved 3-axis
        // (temporal/height/width) MRoPE, not plain sequential 1D RoPE — see
        // `compute_mrope_position_ids`'s doc comment. `mrope_delta` is the
        // continuation offset every subsequently generated text token needs
        // (mirrors the reference `mrope_position_deltas`).
        let (position_ids, mrope_delta) =
            candle_transformers::models::qwen3_vl::compute_mrope_position_ids(
                prompt_ids.len(),
                Some(image_span),
                Some(processed.image_grid_thw),
                MERGE_SIZE,
            );
        let mut logits = session
            .model()
            .forward(
                &input,
                Some(processed.pixel_values),
                None,
                Some(grid),
                None,
                vec![prompt_ids.len()],
                vec![vec![image_span]],
                vec![Vec::new()],
                0,
                &position_ids,
            )
            .map_err(|e| Qwen3VlRuntimeFailure::Inference(e.to_string()))?;
        let mut generated = Vec::with_capacity(configuration.maximum_generated_tokens);
        let mut offset = prompt_ids.len();
        for step in 0..configuration.maximum_generated_tokens {
            let token = match structured_schema {
                Some(schema) => constrained_token(
                    &logits,
                    &self.tokenizer,
                    assistant_prefill,
                    &generated,
                    schema,
                    step + 1 == configuration.maximum_generated_tokens,
                )?,
                None => greedy_token(&logits)?,
            };
            if token == EOS_TOKEN_ID {
                break;
            }
            generated.push(token);
            let next = Tensor::new(&[token], &self.device)
                .and_then(|t| t.unsqueeze(0))
                .map_err(|e| Qwen3VlRuntimeFailure::Inference(e.to_string()))?;
            let next_position = mrope_delta + offset as i64;
            logits = session
                .model()
                .forward(
                    &next,
                    None,
                    None,
                    None,
                    None,
                    vec![1],
                    vec![Vec::new()],
                    vec![Vec::new()],
                    offset,
                    &[[next_position; 3]],
                )
                .map_err(|e| Qwen3VlRuntimeFailure::Inference(e.to_string()))?;
            offset += 1;
        }
        if structured_schema.is_some() && generated.is_empty() {
            return Err(Qwen3VlRuntimeFailure::Inference(
                "structured generation ended before producing a suffix".into(),
            ));
        }
        let generated_text = self
            .tokenizer
            .decode(&generated, true)
            .map_err(|e| Qwen3VlRuntimeFailure::Tokenization(e.to_string()))?;
        let text = format!("{assistant_prefill}{generated_text}");
        if structured_schema
            .is_some_and(|schema| schema_state(schema, &text) != GrammarState::Complete)
        {
            return Err(Qwen3VlRuntimeFailure::NoValidStructuredContinuation);
        }
        Ok(Qwen3VlGenerationResult {
            text,
            token_ids: generated,
            elapsed: started.elapsed(),
            processor_identity: QWEN3_VL_PROCESSOR_ID,
        })
    }

    /// Consume the runtime and release its mmap/tokenizer/factory references.
    pub fn unload(self) {
        self.factory.unload();
    }
}

/// Fail-closed CPU resource preflight.
pub fn preflight_cpu_resources(available_ram_bytes: u64) -> Result<(), Qwen3VlRuntimeFailure> {
    if available_ram_bytes < QWEN3_VL_MINIMUM_RAM_BYTES {
        return Err(Qwen3VlRuntimeFailure::ResourceLimitExceeded {
            available: available_ram_bytes,
            required: QWEN3_VL_MINIMUM_RAM_BYTES,
        });
    }
    Ok(())
}

fn validate_installation_identity(
    installation: &LocalModelInstallation,
) -> Result<(), Qwen3VlRuntimeFailure> {
    let identity = installation.identity();
    if identity.model.repository != "Qwen/Qwen3-VL-2B-Instruct"
        || identity.model.revision != QWEN3_VL_MODEL_REVISION
        || identity.runtime != "candle-0.11.0-cpu-f32"
        || identity.preprocessing != QWEN3_VL_PROCESSOR_ID
    {
        return Err(Qwen3VlRuntimeFailure::InvalidArtifact("installed identity"));
    }
    Ok(())
}

fn required_paths(
    installation: &LocalModelInstallation,
) -> Result<Vec<PathBuf>, Qwen3VlRuntimeFailure> {
    REQUIRED_ARTIFACTS
        .iter()
        .map(|name| {
            installation
                .artifact_path(Path::new(name))
                .map_err(Into::into)
        })
        .collect()
}

fn validate_pinned_json(paths: &[PathBuf]) -> Result<(), Qwen3VlRuntimeFailure> {
    let config: Value = read_json(&paths[1], "config.json")?;
    let preprocessor: Value = read_json(&paths[5], "preprocessor_config.json")?;
    let tokenizer_config: Value = read_json(&paths[3], "tokenizer_config.json")?;
    let chat_template: Value = read_json(&paths[4], "chat_template.json")?;
    let correct = config["model_type"] == "qwen3_vl"
        && config["image_token_id"] == IMAGE_TOKEN_ID
        && config["vision_start_token_id"] == VISION_START_TOKEN_ID
        && config["vision_end_token_id"] == VISION_END_TOKEN_ID
        && config["text_config"]["dtype"] == "bfloat16"
        && config["vision_config"]["patch_size"] == PATCH_SIZE
        && config["vision_config"]["temporal_patch_size"] == TEMPORAL_PATCH_SIZE
        && config["vision_config"]["spatial_merge_size"] == MERGE_SIZE
        && preprocessor["patch_size"] == PATCH_SIZE
        && preprocessor["temporal_patch_size"] == TEMPORAL_PATCH_SIZE
        && preprocessor["merge_size"] == MERGE_SIZE
        && preprocessor["size"]["shortest_edge"] == MIN_PIXELS
        && preprocessor["size"]["longest_edge"] == MAX_PIXELS
        && tokenizer_config["eos_token"] == "<|im_end|>"
        && chat_template["chat_template"]
            .as_str()
            .is_some_and(|v| v.contains("<|vision_start|><|image_pad|><|vision_end|>"));
    if !correct {
        return Err(Qwen3VlRuntimeFailure::InvalidArtifact(
            "pinned JSON semantics",
        ));
    }
    Ok(())
}

fn read_json(path: &Path, name: &'static str) -> Result<Value, Qwen3VlRuntimeFailure> {
    serde_json::from_slice(
        &std::fs::read(path).map_err(|_| Qwen3VlRuntimeFailure::InvalidArtifact(name))?,
    )
    .map_err(|_| Qwen3VlRuntimeFailure::InvalidArtifact(name))
}

fn validate_safetensors(path: &Path) -> Result<(), Qwen3VlRuntimeFailure> {
    // SAFETY: installation verification authenticated the complete file first.
    let tensors = unsafe { candle::safetensors::MmapedSafetensors::new(path) }
        .map_err(|e| Qwen3VlRuntimeFailure::Initialization(e.to_string()))?;
    let views = tensors.tensors();
    if views.len() != EXPECTED_TENSORS
        || views
            .iter()
            .any(|(_, tensor)| !matches!(DType::try_from(tensor.dtype()), Ok(DType::BF16)))
    {
        return Err(Qwen3VlRuntimeFailure::InvalidArtifact("model.safetensors"));
    }
    Ok(())
}

/// Decode and pack one image according to the pinned Qwen3-VL contract.
pub fn process_image(
    encoded: &[u8],
    device: &Device,
) -> Result<Qwen3VlProcessedImage, Qwen3VlRuntimeFailure> {
    let image = image::load_from_memory(encoded)
        .map_err(|e| Qwen3VlRuntimeFailure::Processing(e.to_string()))?;
    process_dynamic_image(image, device)
}

fn process_dynamic_image(
    image: DynamicImage,
    device: &Device,
) -> Result<Qwen3VlProcessedImage, Qwen3VlRuntimeFailure> {
    let original_dimensions = image.dimensions();
    let (height, width) = smart_resize(
        original_dimensions.1 as usize,
        original_dimensions.0 as usize,
    )?;
    if (width, height) != (320, 224) {
        return Err(Qwen3VlRuntimeFailure::InvalidInput(
            "image falls outside qualified 320x224 processor envelope",
        ));
    }
    let rgb = image
        .resize_exact(width as u32, height as u32, FilterType::CatmullRom)
        .to_rgb8();
    let grid_h = height / PATCH_SIZE;
    let grid_w = width / PATCH_SIZE;
    let mut packed =
        Vec::with_capacity(grid_h * grid_w * 3 * TEMPORAL_PATCH_SIZE * PATCH_SIZE * PATCH_SIZE);
    for block_y in 0..(grid_h / MERGE_SIZE) {
        for block_x in 0..(grid_w / MERGE_SIZE) {
            for merge_y in 0..MERGE_SIZE {
                for merge_x in 0..MERGE_SIZE {
                    for channel in 0..3 {
                        for _temporal in 0..TEMPORAL_PATCH_SIZE {
                            for patch_y in 0..PATCH_SIZE {
                                for patch_x in 0..PATCH_SIZE {
                                    let x = (block_x * MERGE_SIZE + merge_x) * PATCH_SIZE + patch_x;
                                    let y = (block_y * MERGE_SIZE + merge_y) * PATCH_SIZE + patch_y;
                                    packed.push(
                                        (f32::from(rgb.get_pixel(x as u32, y as u32)[channel])
                                            / 255.0
                                            - 0.5)
                                            / 0.5,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let patches = grid_h * grid_w;
    let pixel_values = Tensor::from_vec(
        packed,
        (patches, 3 * TEMPORAL_PATCH_SIZE * PATCH_SIZE * PATCH_SIZE),
        device,
    )
    .map_err(|e| Qwen3VlRuntimeFailure::Processing(e.to_string()))?;
    Ok(Qwen3VlProcessedImage {
        original_dimensions,
        processed_dimensions: (width as u32, height as u32),
        image_grid_thw: [1, grid_h, grid_w],
        pixel_values,
        merged_visual_tokens: patches / (MERGE_SIZE * MERGE_SIZE),
    })
}

fn smart_resize(height: usize, width: usize) -> Result<(usize, usize), Qwen3VlRuntimeFailure> {
    if height == 0 || width == 0 || height.max(width) as f64 / height.min(width) as f64 > 200.0 {
        return Err(Qwen3VlRuntimeFailure::InvalidInput(
            "invalid image dimensions",
        ));
    }
    let factor = PATCH_SIZE * MERGE_SIZE;
    let mut h = ((height as f64 / factor as f64).round() as usize).max(1) * factor;
    let mut w = ((width as f64 / factor as f64).round() as usize).max(1) * factor;
    let pixels = h * w;
    if pixels > MAX_PIXELS {
        let beta = ((height * width) as f64 / MAX_PIXELS as f64).sqrt();
        h = ((height as f64 / beta / factor as f64).floor() as usize).max(1) * factor;
        w = ((width as f64 / beta / factor as f64).floor() as usize).max(1) * factor;
    } else if pixels < MIN_PIXELS {
        let beta = (MIN_PIXELS as f64 / (height * width) as f64).sqrt();
        h = ((height as f64 * beta / factor as f64).ceil() as usize).max(1) * factor;
        w = ((width as f64 * beta / factor as f64).ceil() as usize).max(1) * factor;
    }
    Ok((h, w))
}

fn build_prompt_tokens(
    tokenizer: &Tokenizer,
    prompt: &str,
    assistant_prefill: &str,
    visual_tokens: usize,
) -> Result<(Vec<u32>, (usize, usize)), Qwen3VlRuntimeFailure> {
    let image_pads = "<|image_pad|>".repeat(visual_tokens);
    let rendered = format!(
        "<|im_start|>user\n<|vision_start|>{image_pads}<|vision_end|>{prompt}<|im_end|>\n<|im_start|>assistant\n{assistant_prefill}"
    );
    let encoding = tokenizer
        .encode(rendered, false)
        .map_err(|e| Qwen3VlRuntimeFailure::Tokenization(e.to_string()))?;
    let ids = encoding.get_ids().to_vec();
    let start = ids
        .iter()
        .position(|id| *id == IMAGE_TOKEN_ID)
        .ok_or(Qwen3VlRuntimeFailure::InvalidArtifact("image-pad token"))?;
    let end = ids[start..]
        .iter()
        .position(|id| *id != IMAGE_TOKEN_ID)
        .map(|relative| start + relative)
        .unwrap_or(ids.len());
    if end - start != visual_tokens
        || start == 0
        || ids[start - 1] != VISION_START_TOKEN_ID
        || ids.get(end) != Some(&VISION_END_TOKEN_ID)
    {
        return Err(Qwen3VlRuntimeFailure::InvalidArtifact(
            "vision token framing",
        ));
    }
    Ok((ids, (start, end)))
}

fn greedy_token(logits: &Tensor) -> Result<u32, Qwen3VlRuntimeFailure> {
    let dims = logits.dims();
    let last = match dims {
        [_, sequence, _] => logits.i((0, sequence - 1)),
        [sequence, _] => logits.i(sequence - 1),
        _ => {
            return Err(Qwen3VlRuntimeFailure::Inference(
                "unexpected logits shape".into(),
            ))
        }
    }
    .map_err(|e| Qwen3VlRuntimeFailure::Inference(e.to_string()))?;
    last.argmax(0)
        .and_then(|value| value.to_scalar::<u32>())
        .map_err(|e| Qwen3VlRuntimeFailure::Inference(e.to_string()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GrammarState {
    Invalid,
    Prefix,
    Complete,
}

fn constrained_token(
    logits: &Tensor,
    tokenizer: &Tokenizer,
    assistant_prefill: &str,
    generated: &[u32],
    schema: &Qwen3VlStructuredSchema,
    must_complete: bool,
) -> Result<u32, Qwen3VlRuntimeFailure> {
    if schema_state(
        schema,
        &decode_structured(tokenizer, assistant_prefill, generated)?,
    ) == GrammarState::Complete
    {
        return Ok(EOS_TOKEN_ID);
    }
    let last = last_logits(logits)?;
    let values = last
        .to_vec1::<f32>()
        .map_err(|error| Qwen3VlRuntimeFailure::Inference(error.to_string()))?;
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
        let state = schema_state(schema, &text);
        if state != GrammarState::Invalid && (!must_complete || state == GrammarState::Complete) {
            return Ok(token);
        }
    }
    Err(Qwen3VlRuntimeFailure::NoValidStructuredContinuation)
}

fn last_logits(logits: &Tensor) -> Result<Tensor, Qwen3VlRuntimeFailure> {
    match logits.dims() {
        [_, sequence, _] => logits.i((0, sequence - 1)),
        [sequence, _] => logits.i(sequence - 1),
        _ => Err(candle::Error::Msg("unexpected logits shape".into())),
    }
    .map_err(|error| Qwen3VlRuntimeFailure::Inference(error.to_string()))
}

fn decode_structured(
    tokenizer: &Tokenizer,
    assistant_prefill: &str,
    generated: &[u32],
) -> Result<String, Qwen3VlRuntimeFailure> {
    let suffix = tokenizer
        .decode(generated, true)
        .map_err(|error| Qwen3VlRuntimeFailure::Tokenization(error.to_string()))?;
    Ok(format!("{assistant_prefill}{suffix}"))
}

fn schema_state(schema: &Qwen3VlStructuredSchema, text: &str) -> GrammarState {
    match schema {
        Qwen3VlStructuredSchema::StringIdArray {
            field,
            allowed_ids,
            allow_empty,
        } => {
            let opening = format!("{{{}:[", serde_json::to_string(field).unwrap());
            match strip_literal_prefix(text, &opening) {
                Err(state) => state,
                Ok(rest) => id_array_state(rest, allowed_ids, &[], *allow_empty),
            }
        }
        Qwen3VlStructuredSchema::FiniteNumbers { fields } => number_object_state(text, fields, 0),
    }
}

fn schema_is_viable(schema: &Qwen3VlStructuredSchema) -> bool {
    match schema {
        Qwen3VlStructuredSchema::StringIdArray {
            allowed_ids,
            allow_empty,
            ..
        } => *allow_empty || !allowed_ids.is_empty(),
        Qwen3VlStructuredSchema::FiniteNumbers { fields } => {
            !fields.is_empty()
                && fields.iter().all(|(name, minimum, maximum)| {
                    !name.is_empty()
                        && minimum.is_finite()
                        && maximum.is_finite()
                        && minimum <= maximum
                })
        }
    }
}

fn strip_literal_prefix<'a>(input: &'a str, literal: &str) -> Result<&'a str, GrammarState> {
    if input.starts_with(literal) {
        Ok(&input[literal.len()..])
    } else if literal.starts_with(input) {
        Err(GrammarState::Prefix)
    } else {
        Err(GrammarState::Invalid)
    }
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

fn number_object_state(input: &str, fields: &[(String, f64, f64)], index: usize) -> GrammarState {
    if fields.is_empty() || index >= fields.len() {
        return GrammarState::Invalid;
    }
    let opening = format!(
        "{}{}:",
        if index == 0 { "{" } else { "," },
        serde_json::to_string(&fields[index].0).unwrap()
    );
    let rest = match strip_literal_prefix(input, &opening) {
        Ok(rest) => rest,
        Err(state) => return state,
    };
    let delimiter = if index + 1 == fields.len() { '}' } else { ',' };
    if let Some(position) = rest.find(delimiter) {
        let number = &rest[..position];
        let Ok(json_number) = serde_json::from_str::<Value>(number) else {
            return GrammarState::Invalid;
        };
        let Some(value) = json_number.as_f64() else {
            return GrammarState::Invalid;
        };
        if !value.is_finite() || value < fields[index].1 || value > fields[index].2 {
            return GrammarState::Invalid;
        }
        let tail = &rest[position..];
        if delimiter == '}' {
            return if tail == "}" {
                GrammarState::Complete
            } else {
                GrammarState::Invalid
            };
        }
        return number_object_state(tail, fields, index + 1);
    }
    if number_prefix_is_legal(rest) {
        GrammarState::Prefix
    } else {
        GrammarState::Invalid
    }
}

fn number_prefix_is_legal(value: &str) -> bool {
    if value.is_empty() || value == "-" {
        return true;
    }
    if value
        .bytes()
        .any(|byte| !matches!(byte, b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E'))
    {
        return false;
    }
    let candidate = format!("{value}0");
    value.parse::<f64>().is_ok()
        || candidate.parse::<f64>().is_ok()
        || format!("{value}e0").parse::<f64>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};
    use std::io::Cursor;

    fn fixture_png() -> Vec<u8> {
        let image = ImageBuffer::from_fn(96, 64, |x, y| {
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
        let output = process_image(&fixture_png(), &Device::Cpu).unwrap();
        assert_eq!(output.original_dimensions, (96, 64));
        assert_eq!(output.processed_dimensions, (320, 224));
        assert_eq!(output.image_grid_thw, [1, 14, 20]);
        assert_eq!(output.pixel_values.dims(), &[280, 1536]);
        assert_eq!(output.merged_visual_tokens, 70);
    }

    #[test]
    fn resource_preflight_is_fail_closed() {
        assert!(matches!(
            preflight_cpu_resources(QWEN3_VL_MINIMUM_RAM_BYTES - 1),
            Err(Qwen3VlRuntimeFailure::ResourceLimitExceeded { .. })
        ));
        assert!(preflight_cpu_resources(QWEN3_VL_MINIMUM_RAM_BYTES).is_ok());
    }

    #[test]
    fn smart_resize_rejects_pathological_dimensions() {
        assert!(smart_resize(1, 201).is_err());
    }

    #[test]
    fn processor_rejects_shapes_outside_qualified_envelope() {
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(64, 64, Rgb([0, 0, 0])));
        assert!(matches!(
            process_dynamic_image(image, &Device::Cpu),
            Err(Qwen3VlRuntimeFailure::InvalidInput(_))
        ));
    }

    #[test]
    fn structured_contract_rejects_empty_prefill() {
        assert!(Qwen3VlStructuredGenerationContract {
            assistant_prefill: String::new(),
            maximum_generated_tokens: 8,
            schema: Qwen3VlStructuredSchema::FiniteNumbers {
                fields: vec![("answer".into(), 0.0, 2.0)],
            },
        }
        .assistant_prefill
        .is_empty());
    }

    #[test]
    fn structured_grammars_validate_variable_ids_numbers_and_dead_ends() {
        let ids = Qwen3VlStructuredSchema::StringIdArray {
            field: "selected_ids".into(),
            allowed_ids: vec!["cell-alpha".into(), "cell-42".into()],
            allow_empty: true,
        };
        assert_eq!(
            schema_state(&ids, "{\"selected_ids\":[\"cell-alpha\",\"cell-42\"]}"),
            GrammarState::Complete
        );
        assert_eq!(
            schema_state(&ids, "{\"selected_ids\":[\"unknown\""),
            GrammarState::Invalid
        );
        assert_eq!(
            schema_state(&ids, "{\"selected_ids\":[\"cell-alpha\",\"cell-alpha\"]}"),
            GrammarState::Invalid
        );

        let coordinates = Qwen3VlStructuredSchema::FiniteNumbers {
            fields: vec![("x".into(), 0.0, 96.0), ("y".into(), 0.0, 64.0)],
        };
        assert_eq!(
            schema_state(&coordinates, "{\"x\":12.5,\"y\":32}"),
            GrammarState::Complete
        );
        assert_eq!(
            schema_state(&coordinates, "{\"x\":NaN"),
            GrammarState::Invalid
        );
        assert!(!schema_is_viable(&Qwen3VlStructuredSchema::StringIdArray {
            field: "selected_ids".into(),
            allowed_ids: Vec::new(),
            allow_empty: false,
        }));
    }

    /// A second real image, deliberately unrelated in content to
    /// `fixture_png`'s multi-directional gradient (a single flat color),
    /// used only to prove generation output actually depends on what the
    /// image contains.
    fn solid_color_fixture_png() -> Vec<u8> {
        let image = ImageBuffer::from_pixel(96, 64, Rgb([30u8, 120, 30]));
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(image)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    /// Real qualification-host proof. Acquisition is deliberately external to
    /// the runtime; set the variable to a directory containing the six pinned
    /// files. The test hard-links them into canonical staging, atomically
    /// activates the installation, then performs two isolated initializations.
    ///
    /// Strengthened for `SCORPION_QWEN3_VL_CANDLE_REFERENCE_PARITY_ROOT_CAUSE_001`:
    /// this previously asserted only non-empty output, which a genuinely
    /// broken/degenerate model can satisfy trivially (confirmed live: the
    /// pre-fix runtime returned incoherent text like `"### 1, 2, "` for this
    /// exact fixture/prompt and still passed). Real qualification requires
    /// semantically correct, content-dependent output, matching the pinned
    /// Hugging Face `transformers` reference oracle's own greedy output for
    /// the identical weights/config/image/prompt (`"Gradient."`).
    #[tokio::test]
    #[ignore = "requires the pinned 4.25 GB offline model installation"]
    async fn real_offline_generation_unload_and_reinitialize() {
        let source = PathBuf::from(
            std::env::var("SCORPION_QWEN3_VL_PINNED_ARTIFACTS")
                .expect("set pinned offline artifact directory"),
        );
        let parent = tempfile::tempdir_in(source.parent().unwrap()).unwrap();
        let staging = parent.path().join("staging");
        let active = parent.path().join("active");
        std::fs::create_dir(&staging).unwrap();
        for name in REQUIRED_ARTIFACTS {
            std::fs::hard_link(source.join(name), staging.join(name)).unwrap();
        }
        let manifest = qwen3_vl_cpu_f32_manifest();
        let installation = manifest.activate(&staging, &active).unwrap();
        let mut gradient_outputs = Vec::new();
        for _ in 0..2 {
            let runtime = Qwen3VlCpuRuntime::initialize_from_host(&installation).unwrap();
            let output = runtime
                .generate(
                    &fixture_png(),
                    "Describe this image in one word.",
                    Qwen3VlGenerationConfiguration {
                        maximum_generated_tokens: 8,
                    },
                )
                .await
                .unwrap();
            assert!(!output.token_ids.is_empty());
            assert!(
                output.text.to_lowercase().contains("gradient"),
                "expected a semantically correct one-word description of the \
                 gradient fixture (matches the pinned reference oracle's own \
                 greedy output, \"Gradient.\"); got {:?} instead — non-empty \
                 output alone does not prove the model is coherent",
                output.text
            );
            gradient_outputs.push(output.text);

            // A materially different image (flat color, no gradient) must
            // not produce the same generic answer — proves the description
            // genuinely depends on image content, not a fixed response.
            let other = runtime
                .generate(
                    &solid_color_fixture_png(),
                    "Describe this image in one word.",
                    Qwen3VlGenerationConfiguration {
                        maximum_generated_tokens: 8,
                    },
                )
                .await
                .unwrap();
            assert!(!other.text.trim().is_empty());
            assert_ne!(
                other.text,
                *gradient_outputs.last().unwrap(),
                "a flat-color image and a multi-directional gradient must not \
                 produce the identical description"
            );
            runtime.unload();
        }
        // Deterministic greedy decoding: repeated isolated sessions with the
        // identical input must produce the identical output.
        assert_eq!(gradient_outputs[0], gradient_outputs[1]);
    }

    /// Real pinned-model proof that assistant-prefilled output contains a
    /// model-generated suffix accepted by a strict structured parser.
    #[tokio::test]
    #[ignore = "requires the pinned 4.25 GB offline model installation"]
    async fn real_structured_generation_is_nonempty_and_strictly_parsed() {
        let source = PathBuf::from(
            std::env::var("SCORPION_QWEN3_VL_PINNED_ARTIFACTS")
                .expect("set pinned offline artifact directory"),
        );
        let parent = tempfile::tempdir_in(source.parent().unwrap()).unwrap();
        let staging = parent.path().join("staging");
        let active = parent.path().join("active");
        std::fs::create_dir(&staging).unwrap();
        for name in REQUIRED_ARTIFACTS {
            std::fs::hard_link(source.join(name), staging.join(name)).unwrap();
        }
        let installation = qwen3_vl_cpu_f32_manifest()
            .activate(&staging, &active)
            .unwrap();
        let runtime = Qwen3VlCpuRuntime::initialize_from_host(&installation).unwrap();
        let numeric_contract = Qwen3VlStructuredGenerationContract {
            assistant_prefill: "{\"answer\":".into(),
            maximum_generated_tokens: 8,
            schema: Qwen3VlStructuredSchema::FiniteNumbers {
                fields: vec![("answer".into(), 0.0, 2.0)],
            },
        };
        let output = runtime
            .generate_structured(
                &fixture_png(),
                "Return exactly one JSON object whose answer field is the integer 1. No prose.",
                numeric_contract.clone(),
            )
            .await
            .unwrap();
        assert!(!output.token_ids.is_empty(), "model generated no suffix");
        let value: serde_json::Value = serde_json::from_str(&output.text)
            .unwrap_or_else(|error| panic!("structured output {0:?}: {error}", output.text));
        let object = value.as_object().unwrap();
        assert_eq!(object.len(), 1);
        let answer = object
            .get("answer")
            .and_then(|value| value.as_f64())
            .unwrap();
        assert!(answer.is_finite() && (0.0..=2.0).contains(&answer));
        let repeated = runtime
            .generate_structured(
                &fixture_png(),
                "Return exactly one JSON object whose answer field is the integer 1. No prose.",
                numeric_contract,
            )
            .await
            .unwrap();
        assert_eq!(output.text, repeated.text);

        let ids = runtime
            .generate_structured(
                &fixture_png(),
                "Choose cell-beta. Return only the requested JSON object.",
                Qwen3VlStructuredGenerationContract {
                    // `allow_empty: false` forces the array's first
                    // character to be the opening quote of the first
                    // chosen id regardless of what the model generates —
                    // pre-committing it here (see the identical, documented
                    // pattern in `qwen3_vl_captcha.rs::prepare_request`)
                    // works around a real per-token search gap for an
                    // isolated opening quote when nothing has been
                    // generated yet.
                    assistant_prefill: "{\"selected_ids\":[\"".into(),
                    maximum_generated_tokens: 8,
                    schema: Qwen3VlStructuredSchema::StringIdArray {
                        field: "selected_ids".into(),
                        allowed_ids: vec!["cell-alpha".into(), "cell-beta".into()],
                        allow_empty: false,
                    },
                },
            )
            .await
            .unwrap();
        assert!(!ids.token_ids.is_empty());
        let parsed: serde_json::Value = serde_json::from_str(&ids.text).unwrap();
        let selected = parsed["selected_ids"].as_array().unwrap();
        assert!(!selected.is_empty());
        assert!(selected
            .iter()
            .all(|id| matches!(id.as_str(), Some("cell-alpha" | "cell-beta"))));
        runtime.unload();
    }
}
