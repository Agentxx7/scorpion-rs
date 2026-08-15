//! Canonical local Qwen3-VL CAPTCHA provider adapter.
//!
//! This module owns prompt/strict-output translation only. Model installation,
//! image processing, tokenization and generation remain exclusively owned by
//! [`Qwen3VlCpuRuntime`]. Executability is not empirical qualification.

use crate::features::captcha::{
    CaptchaCapabilityQualification, CaptchaChallengeKind, CaptchaImageGridInput,
    CaptchaLocalRuntimeProvenance, CaptchaProvider, CaptchaProviderAvailability,
    CaptchaProviderCapabilities, CaptchaProviderId, CaptchaProviderLocality, CaptchaSolution,
    CaptchaSolveFailure, CaptchaSolveOutcome, CaptchaSolveProvenance, CaptchaSolveRequest,
};
use crate::features::local_model::LocalModelInstallation;
use crate::features::qwen3_vl_runtime::{
    Qwen3VlCpuRuntime, Qwen3VlRuntimeFailure, Qwen3VlStructuredGenerationContract,
    Qwen3VlStructuredSchema,
};
use serde::Deserialize;
use std::collections::HashSet;
use std::time::{Duration, Instant};

const RUNTIME_IDENTITY: &str = "candle-0.11.0-cpu-f32";
const IMAGE_GRID_GRAMMAR: &str = "qwen3-vl-captcha-image-grid-json-v1";
const HORIZONTAL_OFFSET_GRAMMAR: &str = "qwen3-vl-captcha-horizontal-offset-json-v1";
const POINT_SELECTION_GRAMMAR: &str = "qwen3-vl-captcha-point-selection-json-v1";

static CAPABILITIES: CaptchaProviderCapabilities = CaptchaProviderCapabilities {
    provider: CaptchaProviderId::QWEN3_VL_LOCAL,
    locality: CaptchaProviderLocality::Local,
    supported_kinds: &[
        CaptchaChallengeKind::ImageGridSelection,
        CaptchaChallengeKind::HorizontalOffset,
        CaptchaChallengeKind::PointSelection,
    ],
    supported_media_types: &["image/jpeg", "image/png"],
    maximum_inputs: 1,
    requires_credentials: false,
};

/// Canonical local Qwen3-VL provider. Its private runtime owns all model and
/// processor resources; the adapter exposes neither runtime internals nor raw
/// model handles.
pub struct Qwen3VlLocalCaptchaProvider {
    runtime: Qwen3VlCpuRuntime,
}

impl Qwen3VlLocalCaptchaProvider {
    /// Initialize from one verified installation with an explicit RAM value.
    /// This form supports deterministic fail-closed operator preflight tests.
    pub fn new(
        installation: &LocalModelInstallation,
        available_ram_bytes: u64,
    ) -> Result<Self, Qwen3VlRuntimeFailure> {
        Ok(Self {
            runtime: Qwen3VlCpuRuntime::initialize(installation, available_ram_bytes)?,
        })
    }

    /// Initialize from a verified installation using observed host RAM.
    pub fn initialize_from_host(
        installation: &LocalModelInstallation,
    ) -> Result<Self, Qwen3VlRuntimeFailure> {
        Ok(Self {
            runtime: Qwen3VlCpuRuntime::initialize_from_host(installation)?,
        })
    }

    /// Consume the provider and release runtime-owned model resources.
    pub fn unload(self) {
        self.runtime.unload();
    }
}

#[async_trait::async_trait]
impl CaptchaProvider for Qwen3VlLocalCaptchaProvider {
    fn capabilities(&self) -> &'static CaptchaProviderCapabilities {
        &CAPABILITIES
    }

    fn availability(&self) -> CaptchaProviderAvailability {
        CaptchaProviderAvailability::Available
    }

    fn qualification_state(
        &self,
        kind: CaptchaChallengeKind,
    ) -> Option<CaptchaCapabilityQualification> {
        CAPABILITIES
            .supported_kinds
            .contains(&kind)
            .then_some(CaptchaCapabilityQualification::ExecutableUnqualified)
    }

    async fn solve(&self, request: &CaptchaSolveRequest) -> CaptchaSolveOutcome {
        let started = Instant::now();
        let prepared = match prepare_request(&self.runtime, request) {
            Ok(prepared) => prepared,
            Err(failure) => return failed(&self.runtime, request, failure, started.elapsed()),
        };
        let generation = match tokio::time::timeout(
            request.deadline,
            self.runtime.generate_structured(
                prepared.bytes,
                &prepared.prompt,
                Qwen3VlStructuredGenerationContract {
                    assistant_prefill: prepared.assistant_prefill,
                    maximum_generated_tokens: 16,
                    schema: prepared.schema,
                },
            ),
        )
        .await
        {
            Err(_) => {
                return failed(
                    &self.runtime,
                    request,
                    CaptchaSolveFailure::DeadlineExceeded,
                    started.elapsed(),
                )
            }
            Ok(Err(_)) => {
                return failed(
                    &self.runtime,
                    request,
                    CaptchaSolveFailure::LocalExecutionFailure,
                    started.elapsed(),
                )
            }
            Ok(Ok(generation)) => generation,
        };
        match parse_solution(request, prepared.context, &generation.text) {
            Ok(solution) => CaptchaSolveOutcome::Solved {
                solution,
                provenance: provenance(&self.runtime, request, generation.elapsed, true),
            },
            Err(failure) => failed(&self.runtime, request, failure, started.elapsed()),
        }
    }
}

enum ParseContext<'a> {
    ImageGrid(&'a CaptchaImageGridInput),
    Coordinates { dimensions: (u32, u32) },
}

struct PreparedRequest<'a> {
    bytes: &'a [u8],
    prompt: String,
    assistant_prefill: String,
    schema: Qwen3VlStructuredSchema,
    context: ParseContext<'a>,
}

fn prepare_request<'a>(
    runtime: &Qwen3VlCpuRuntime,
    request: &'a CaptchaSolveRequest,
) -> Result<PreparedRequest<'a>, CaptchaSolveFailure> {
    if request.challenge.visuals.len() != 1 {
        return Err(CaptchaSolveFailure::InvalidChallenge);
    }
    let visual = &request.challenge.visuals[0];
    let bytes = visual
        .bytes()
        .ok_or(CaptchaSolveFailure::InvalidChallenge)?;
    let metadata = runtime
        .image_metadata(bytes)
        .map_err(|_| CaptchaSolveFailure::InvalidChallenge)?;
    match request.challenge.kind {
        CaptchaChallengeKind::ImageGridSelection => {
            let grid = visual
                .image_grid()
                .ok_or(CaptchaSolveFailure::InvalidChallenge)?;
            if grid.original_dimensions() != metadata.original_dimensions {
                return Err(CaptchaSolveFailure::InvalidChallenge);
            }
            // When an empty selection is not a valid answer, the array's
            // first character is forced to be the opening quote of the
            // first chosen id regardless of what the model generates — pre-
            // committing it in the prefill rather than requiring the
            // constrained search to (re)discover that single forced token
            // works around a real search-vocabulary gap: some tokenizer
            // entries for an isolated opening quote don't get found by the
            // ranked per-token search when nothing has been generated yet.
            // When an empty selection is valid, the prefill must stay at
            // `[` so the model can still legitimately close immediately.
            let assistant_prefill = if grid.empty_selection_valid() {
                "{\"selected_ids\":[".to_string()
            } else {
                "{\"selected_ids\":[\"".to_string()
            };
            Ok(PreparedRequest {
                bytes,
                prompt: image_grid_prompt(request, grid),
                assistant_prefill,
                schema: Qwen3VlStructuredSchema::StringIdArray {
                    field: "selected_ids".into(),
                    allowed_ids: grid
                        .cells()
                        .iter()
                        .map(|cell| cell.choice_id().to_owned())
                        .collect(),
                    allow_empty: grid.empty_selection_valid(),
                },
                context: ParseContext::ImageGrid(grid),
            })
        }
        CaptchaChallengeKind::HorizontalOffset => Ok(PreparedRequest {
            bytes,
            prompt: horizontal_offset_prompt(request, metadata.original_dimensions),
            assistant_prefill: "{\"x\":".into(),
            schema: Qwen3VlStructuredSchema::FiniteNumbers {
                fields: vec![("x".into(), 0.0, f64::from(metadata.original_dimensions.0))],
            },
            context: ParseContext::Coordinates {
                dimensions: metadata.original_dimensions,
            },
        }),
        CaptchaChallengeKind::PointSelection => Ok(PreparedRequest {
            bytes,
            prompt: point_selection_prompt(request, metadata.original_dimensions),
            assistant_prefill: "{\"x\":".into(),
            schema: Qwen3VlStructuredSchema::FiniteNumbers {
                fields: vec![
                    ("x".into(), 0.0, f64::from(metadata.original_dimensions.0)),
                    ("y".into(), 0.0, f64::from(metadata.original_dimensions.1)),
                ],
            },
            context: ParseContext::Coordinates {
                dimensions: metadata.original_dimensions,
            },
        }),
    }
}

fn image_grid_prompt(request: &CaptchaSolveRequest, grid: &CaptchaImageGridInput) -> String {
    let cells: Vec<_> = grid
        .cells()
        .iter()
        .map(|cell| {
            let (x, y, width, height) = cell.geometry();
            serde_json::json!({
                "id": cell.choice_id(),
                "row": cell.row(),
                "column": cell.column(),
                "x": x,
                "y": y,
                "width": width,
                "height": height,
            })
        })
        .collect();
    format!(
        "Task: {}\nGrid: {}x{}, {}x{} pixels\nCells: {}\nEmpty allowed: {}\nReturn JSON only using this schema: {{\"selected_ids\":[\"cell-id\"]}}. IDs must come from Cells. No prose. Grammar: {IMAGE_GRID_GRAMMAR}",
        request.challenge.instruction,
        grid.rows(),
        grid.columns(),
        grid.original_dimensions().0,
        grid.original_dimensions().1,
        serde_json::to_string(&cells).expect("cell JSON serialization is infallible"),
        grid.empty_selection_valid(),
    )
}

fn horizontal_offset_prompt(request: &CaptchaSolveRequest, dimensions: (u32, u32)) -> String {
    format!(
        "Grammar: {HORIZONTAL_OFFSET_GRAMMAR}. Task: {}. Return the horizontal displacement in original-image pixels for width={} and height={}. Return exactly one JSON object {{\"x\":number}} and no prose.",
        request.challenge.instruction, dimensions.0, dimensions.1,
    )
}

fn point_selection_prompt(request: &CaptchaSolveRequest, dimensions: (u32, u32)) -> String {
    format!(
        "Grammar: {POINT_SELECTION_GRAMMAR}. Task: {}. Return a point in original-image coordinates for width={} and height={}. Return exactly one JSON object {{\"x\":number,\"y\":number}} and no prose.",
        request.challenge.instruction, dimensions.0, dimensions.1,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GridOutput {
    selected_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OffsetOutput {
    x: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PointOutput {
    x: f64,
    y: f64,
}

fn parse_solution(
    request: &CaptchaSolveRequest,
    context: ParseContext<'_>,
    text: &str,
) -> Result<CaptchaSolution, CaptchaSolveFailure> {
    match (request.challenge.kind, context) {
        (CaptchaChallengeKind::ImageGridSelection, ParseContext::ImageGrid(grid)) => {
            let output: GridOutput = strict_json(text)?;
            let known: HashSet<_> = grid.cells().iter().map(|cell| cell.choice_id()).collect();
            let mut observed = HashSet::with_capacity(output.selected_ids.len());
            if (!grid.empty_selection_valid() && output.selected_ids.is_empty())
                || output
                    .selected_ids
                    .iter()
                    .any(|id| !known.contains(id.as_str()) || !observed.insert(id.as_str()))
            {
                return Err(CaptchaSolveFailure::InvalidProviderResponse);
            }
            Ok(CaptchaSolution::SelectedChoices(output.selected_ids))
        }
        (CaptchaChallengeKind::HorizontalOffset, ParseContext::Coordinates { dimensions }) => {
            let output: OffsetOutput = strict_json(text)?;
            if !output.x.is_finite() || output.x < 0.0 || output.x > f64::from(dimensions.0) {
                return Err(CaptchaSolveFailure::InvalidProviderResponse);
            }
            Ok(CaptchaSolution::HorizontalOffset(output.x))
        }
        (CaptchaChallengeKind::PointSelection, ParseContext::Coordinates { dimensions }) => {
            let output: PointOutput = strict_json(text)?;
            if !output.x.is_finite()
                || !output.y.is_finite()
                || output.x < 0.0
                || output.y < 0.0
                || output.x > f64::from(dimensions.0)
                || output.y > f64::from(dimensions.1)
            {
                return Err(CaptchaSolveFailure::InvalidProviderResponse);
            }
            Ok(CaptchaSolution::Point {
                x: output.x,
                y: output.y,
            })
        }
        _ => Err(CaptchaSolveFailure::InvalidChallenge),
    }
}

fn strict_json<T: for<'de> Deserialize<'de>>(text: &str) -> Result<T, CaptchaSolveFailure> {
    let trimmed = text.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return Err(CaptchaSolveFailure::InvalidProviderResponse);
    }
    serde_json::from_str(trimmed).map_err(|_| CaptchaSolveFailure::InvalidProviderResponse)
}

fn grammar(kind: CaptchaChallengeKind) -> &'static str {
    match kind {
        CaptchaChallengeKind::ImageGridSelection => IMAGE_GRID_GRAMMAR,
        CaptchaChallengeKind::HorizontalOffset => HORIZONTAL_OFFSET_GRAMMAR,
        CaptchaChallengeKind::PointSelection => POINT_SELECTION_GRAMMAR,
    }
}

/// Provenance always names the exact checkpoint/revision the runtime
/// instance actually loaded (`runtime.model_revision()`/
/// `runtime.processor_identity()`) — never a fixed module constant. More
/// than one pinned checkpoint can back this same `qwen3-vl-local` provider
/// identity now, and provenance must be truthful about which one ran.
fn provenance(
    runtime: &Qwen3VlCpuRuntime,
    request: &CaptchaSolveRequest,
    elapsed: Duration,
    succeeded: bool,
) -> CaptchaSolveProvenance {
    CaptchaSolveProvenance::local_runtime(
        CaptchaProviderId::QWEN3_VL_LOCAL,
        CaptchaLocalRuntimeProvenance {
            model_revision: runtime.model_revision().into(),
            runtime_identity: RUNTIME_IDENTITY.into(),
            processor_identity: runtime.processor_identity().into(),
            challenge_kind: request.challenge.kind,
            prompt_grammar_identity: grammar(request.challenge.kind).into(),
            elapsed,
            succeeded,
        },
    )
}

fn failed(
    runtime: &Qwen3VlCpuRuntime,
    request: &CaptchaSolveRequest,
    failure: CaptchaSolveFailure,
    elapsed: Duration,
) -> CaptchaSolveOutcome {
    CaptchaSolveOutcome::Failed {
        failure,
        provenance: Some(provenance(runtime, request, elapsed, false)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::captcha::{
        solve_captcha, CaptchaChallenge, CaptchaImageGridCell, CaptchaProviderRegistry,
        CaptchaVisualInput,
    };
    use crate::features::qwen3_vl_runtime::{
        qwen3_vl_cpu_f32_manifest, qwen3_vl_cpu_f32_manifest_4b, QWEN3_VL_4B_MINIMUM_RAM_BYTES,
        QWEN3_VL_4B_MODEL_REVISION, QWEN3_VL_4B_PROCESSOR_ID, QWEN3_VL_MINIMUM_RAM_BYTES,
        QWEN3_VL_MODEL_REVISION, QWEN3_VL_PROCESSOR_ID,
    };
    use image::{DynamicImage, ImageBuffer, Rgb};
    use std::io::Cursor;
    use std::path::PathBuf;

    fn request(kind: CaptchaChallengeKind) -> CaptchaSolveRequest {
        CaptchaSolveRequest {
            correlation_id: "unit".into(),
            selected_provider: CaptchaProviderId::QWEN3_VL_LOCAL,
            challenge: CaptchaChallenge {
                kind,
                instruction: "solve".into(),
                visuals: Vec::new(),
            },
            deadline: Duration::from_secs(1),
        }
    }

    fn grid(empty_valid: bool) -> CaptchaImageGridInput {
        CaptchaImageGridInput::new(
            CaptchaVisualInput::materialized(None, "image/png", [1]),
            (20, 10),
            1,
            2,
            vec![
                CaptchaImageGridCell::new("left", 0, 0, 0, 0, 10, 10),
                CaptchaImageGridCell::new("right", 0, 1, 10, 0, 10, 10),
            ],
            empty_valid,
        )
        .unwrap()
    }

    #[test]
    fn strict_grid_ids_and_empty_semantics() {
        let request = request(CaptchaChallengeKind::ImageGridSelection);
        assert_eq!(
            parse_solution(
                &request,
                ParseContext::ImageGrid(&grid(false)),
                r#"{"selected_ids":["right"]}"#,
            )
            .unwrap(),
            CaptchaSolution::SelectedChoices(vec!["right".into()])
        );
        for invalid in [
            r#"{"selected_ids":[]}"#,
            r#"{"selected_ids":["unknown"]}"#,
            r#"{"selected_ids":["left","left"]}"#,
            r#"answer: {"selected_ids":["left"]}"#,
        ] {
            assert!(
                parse_solution(&request, ParseContext::ImageGrid(&grid(false)), invalid,).is_err()
            );
        }
        assert_eq!(
            parse_solution(
                &request,
                ParseContext::ImageGrid(&grid(true)),
                r#"{"selected_ids":[]}"#,
            )
            .unwrap(),
            CaptchaSolution::SelectedChoices(Vec::new())
        );
    }

    #[test]
    fn coordinates_are_finite_strict_and_original_space_bounded() {
        let offset = request(CaptchaChallengeKind::HorizontalOffset);
        assert_eq!(
            parse_solution(
                &offset,
                ParseContext::Coordinates {
                    dimensions: (96, 64),
                },
                r#"{"x":48}"#,
            )
            .unwrap(),
            CaptchaSolution::HorizontalOffset(48.0)
        );
        for invalid in [r#"{"x":97}"#, r#"{"x":-1}"#, r#"{"x":1,"y":2}"#, "NaN"] {
            assert!(parse_solution(
                &offset,
                ParseContext::Coordinates {
                    dimensions: (96, 64),
                },
                invalid,
            )
            .is_err());
        }
        let point = request(CaptchaChallengeKind::PointSelection);
        assert_eq!(
            parse_solution(
                &point,
                ParseContext::Coordinates {
                    dimensions: (96, 64),
                },
                r#"{"x":12,"y":34}"#,
            )
            .unwrap(),
            CaptchaSolution::Point { x: 12.0, y: 34.0 }
        );
        for invalid in [r#"{"x":97,"y":1}"#, r#"{"x":1,"y":65}"#, r#"{"x":1}"#] {
            assert!(parse_solution(
                &point,
                ParseContext::Coordinates {
                    dimensions: (96, 64),
                },
                invalid,
            )
            .is_err());
        }
    }

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

    /// Qualification-host integration proof. Acquisition remains external;
    /// the provider receives only a canonical verified installation.
    #[tokio::test]
    #[ignore = "requires the pinned 4.25 GB offline model installation"]
    async fn real_provider_registry_runtime_and_strict_outcome() {
        let source = PathBuf::from(
            std::env::var("SCORPION_QWEN3_VL_PINNED_ARTIFACTS")
                .expect("set pinned offline artifact directory"),
        );
        let parent = tempfile::tempdir_in(source.parent().unwrap()).unwrap();
        let staging = parent.path().join("staging");
        let active = parent.path().join("active");
        std::fs::create_dir(&staging).unwrap();
        for name in [
            "model.safetensors",
            "config.json",
            "tokenizer.json",
            "tokenizer_config.json",
            "chat_template.json",
            "preprocessor_config.json",
        ] {
            std::fs::hard_link(source.join(name), staging.join(name)).unwrap();
        }
        let installation = qwen3_vl_cpu_f32_manifest()
            .activate(&staging, &active)
            .unwrap();
        assert!(matches!(
            Qwen3VlLocalCaptchaProvider::new(&installation, QWEN3_VL_MINIMUM_RAM_BYTES - 1),
            Err(Qwen3VlRuntimeFailure::ResourceLimitExceeded { .. })
        ));
        let provider = Qwen3VlLocalCaptchaProvider::initialize_from_host(&installation).unwrap();
        let mut registry = CaptchaProviderRegistry::new();
        registry.register(&provider).unwrap();
        assert!(std::ptr::eq(
            registry.resolve(CaptchaProviderId::QWEN3_VL_LOCAL).unwrap(),
            &provider as &dyn CaptchaProvider,
        ));
        assert_eq!(
            registry.qualification_state(
                CaptchaProviderId::QWEN3_VL_LOCAL,
                CaptchaChallengeKind::ImageGridSelection,
            ),
            Some(CaptchaCapabilityQualification::ExecutableUnqualified)
        );
        let visual_bytes = fixture_png();
        let grid = CaptchaImageGridInput::new(
            CaptchaVisualInput::materialized(None, "image/png", visual_bytes),
            (96, 64),
            1,
            1,
            vec![CaptchaImageGridCell::new("cell-1", 0, 0, 0, 0, 96, 64)],
            false,
        )
        .unwrap();
        let request = CaptchaSolveRequest {
            correlation_id: "real-offline-provider".into(),
            selected_provider: CaptchaProviderId::QWEN3_VL_LOCAL,
            challenge: CaptchaChallenge {
                kind: CaptchaChallengeKind::ImageGridSelection,
                instruction: "Select the only cell. Its stable ID is cell-1.".into(),
                visuals: vec![CaptchaVisualInput::materialized_full_grid(grid)],
            },
            deadline: Duration::from_secs(1_800),
        };
        let outcome = solve_captcha(&provider, &request).await;
        match outcome {
            CaptchaSolveOutcome::Solved {
                solution: CaptchaSolution::SelectedChoices(ids),
                provenance,
            } => {
                assert_eq!(ids, ["cell-1"]);
                assert_eq!(provenance.provider, CaptchaProviderId::QWEN3_VL_LOCAL);
                let facts = provenance.local_runtime.unwrap();
                assert!(facts.succeeded);
                assert_eq!(facts.model_revision, QWEN3_VL_MODEL_REVISION);
                assert_eq!(facts.processor_identity, QWEN3_VL_PROCESSOR_ID);
                assert_eq!(
                    facts.challenge_kind,
                    CaptchaChallengeKind::ImageGridSelection
                );
            }
            _ => panic!("real provider did not produce a strict solved outcome"),
        }
        provider.unload();
    }

    /// Same end-to-end proof as `real_provider_registry_runtime_and_strict_outcome`,
    /// for the higher-capacity 4B checkpoint
    /// (`SCORPION_QWEN3_VL_LOCAL_CAPTCHA_HIGHER_CAPACITY_MODEL_QUALIFICATION_001`)
    /// — the exact same canonical `qwen3-vl-local` provider identity and
    /// registry contract, backed by a different pinned checkpoint. Directly
    /// proves provenance now truthfully names the checkpoint that actually
    /// ran (`facts.model_revision`/`facts.processor_identity` here are the
    /// 4B constants, not the 2B ones asserted above).
    #[tokio::test]
    #[ignore = "requires the pinned ~8.3 GB offline 4B model installation"]
    async fn real_4b_provider_registry_runtime_and_strict_outcome() {
        let source = PathBuf::from(
            std::env::var("SCORPION_QWEN3_VL_4B_PINNED_ARTIFACTS")
                .expect("set pinned offline 4B artifact directory"),
        );
        let parent = tempfile::tempdir_in(source.parent().unwrap()).unwrap();
        let staging = parent.path().join("staging");
        let active = parent.path().join("active");
        std::fs::create_dir(&staging).unwrap();
        for name in [
            "model-00001-of-00002.safetensors",
            "model-00002-of-00002.safetensors",
            "config.json",
            "tokenizer.json",
            "tokenizer_config.json",
            "chat_template.json",
            "preprocessor_config.json",
        ] {
            std::fs::hard_link(source.join(name), staging.join(name)).unwrap();
        }
        let installation = qwen3_vl_cpu_f32_manifest_4b()
            .activate(&staging, &active)
            .unwrap();
        assert!(matches!(
            Qwen3VlLocalCaptchaProvider::new(&installation, QWEN3_VL_4B_MINIMUM_RAM_BYTES - 1),
            Err(Qwen3VlRuntimeFailure::ResourceLimitExceeded { .. })
        ));
        let provider = Qwen3VlLocalCaptchaProvider::initialize_from_host(&installation).unwrap();
        let mut registry = CaptchaProviderRegistry::new();
        registry.register(&provider).unwrap();
        assert!(std::ptr::eq(
            registry.resolve(CaptchaProviderId::QWEN3_VL_LOCAL).unwrap(),
            &provider as &dyn CaptchaProvider,
        ));
        assert_eq!(
            registry.qualification_state(
                CaptchaProviderId::QWEN3_VL_LOCAL,
                CaptchaChallengeKind::ImageGridSelection,
            ),
            Some(CaptchaCapabilityQualification::ExecutableUnqualified)
        );
        let visual_bytes = fixture_png();
        let grid = CaptchaImageGridInput::new(
            CaptchaVisualInput::materialized(None, "image/png", visual_bytes),
            (96, 64),
            1,
            1,
            vec![CaptchaImageGridCell::new("cell-1", 0, 0, 0, 0, 96, 64)],
            false,
        )
        .unwrap();
        let request = CaptchaSolveRequest {
            correlation_id: "real-offline-4b-provider".into(),
            selected_provider: CaptchaProviderId::QWEN3_VL_LOCAL,
            challenge: CaptchaChallenge {
                kind: CaptchaChallengeKind::ImageGridSelection,
                instruction: "Select the only cell. Its stable ID is cell-1.".into(),
                visuals: vec![CaptchaVisualInput::materialized_full_grid(grid)],
            },
            deadline: Duration::from_secs(1_800),
        };
        let outcome = solve_captcha(&provider, &request).await;
        match outcome {
            CaptchaSolveOutcome::Solved {
                solution: CaptchaSolution::SelectedChoices(ids),
                provenance,
            } => {
                assert_eq!(ids, ["cell-1"]);
                assert_eq!(provenance.provider, CaptchaProviderId::QWEN3_VL_LOCAL);
                let facts = provenance.local_runtime.unwrap();
                assert!(facts.succeeded);
                assert_eq!(facts.model_revision, QWEN3_VL_4B_MODEL_REVISION);
                assert_eq!(facts.processor_identity, QWEN3_VL_4B_PROCESSOR_ID);
                assert_eq!(
                    facts.challenge_kind,
                    CaptchaChallengeKind::ImageGridSelection
                );
            }
            _ => panic!("real 4B provider did not produce a strict solved outcome"),
        }
        provider.unload();
    }
}
