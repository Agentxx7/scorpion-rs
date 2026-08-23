//! Canonical local PaliGemma CAPTCHA provider adapter.
//!
//! This module owns prompt/strict-output translation only. Model
//! installation, image processing, tokenization and generation remain
//! exclusively owned by [`PaligemmaCpuRuntime`]. Executability is not
//! empirical qualification.
//!
//! `PointSelection` and `HorizontalOffset` use PaliGemma's own genuine
//! `detect {label}` grounding task (`<locNNNN>` tokens — ordinary
//! vocabulary, plain next-token generation) rather than asking the model to
//! emit raw JSON coordinates it was never trained to produce for spatial
//! answers; a returned point is the deterministic center of the model's own
//! real bounding box, not a corrected or leaked one.
//! `ImageGridSelection` reuses the same JSON string-id-array structured
//! contract already proven elsewhere in this crate, since categorical
//! choice selection is not a spatial-precision task.

use crate::features::captcha::{
    CaptchaCapabilityQualification, CaptchaChallengeKind, CaptchaImageGridInput,
    CaptchaLocalRuntimeProvenance, CaptchaProvider, CaptchaProviderAvailability,
    CaptchaProviderCapabilities, CaptchaProviderId, CaptchaProviderLocality, CaptchaSolution,
    CaptchaSolveFailure, CaptchaSolveOutcome, CaptchaSolveProvenance, CaptchaSolveRequest,
};
use crate::features::local_model::LocalModelInstallation;
use crate::features::paligemma_runtime::{
    PaligemmaCpuRuntime, PaligemmaDetectionBox, PaligemmaRuntimeFailure,
    PaligemmaStringIdArraySchema,
};
use std::collections::HashSet;
use std::time::{Duration, Instant};

const DETECT_GRAMMAR: &str = "paligemma-captcha-detect-loc4-v1";
const IMAGE_GRID_GRAMMAR: &str = "paligemma-captcha-image-grid-json-v1";
/// This provider's own documented convention for `HorizontalOffset`: the
/// instruction names the handle description and the target description,
/// separated by this literal delimiter, so two independent `detect` queries
/// can be issued, each embedding the instruction into a provider-specific
/// template.
const HORIZONTAL_OFFSET_DELIMITER: &str = " -> ";

static CAPABILITIES: CaptchaProviderCapabilities = CaptchaProviderCapabilities {
    provider: CaptchaProviderId::PALIGEMMA_LOCAL,
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

/// Canonical local PaliGemma provider. Its private runtime owns all model and
/// processor resources; the adapter exposes neither runtime internals nor raw
/// model handles.
pub struct PaligemmaLocalCaptchaProvider {
    runtime: PaligemmaCpuRuntime,
}

impl PaligemmaLocalCaptchaProvider {
    /// Initialize from one verified installation with an explicit RAM value.
    /// This form supports deterministic fail-closed operator preflight tests.
    pub fn new(
        installation: &LocalModelInstallation,
        available_ram_bytes: u64,
    ) -> Result<Self, PaligemmaRuntimeFailure> {
        Ok(Self {
            runtime: PaligemmaCpuRuntime::initialize(installation, available_ram_bytes)?,
        })
    }

    /// Initialize using current available host memory.
    pub fn initialize_from_host(
        installation: &LocalModelInstallation,
    ) -> Result<Self, PaligemmaRuntimeFailure> {
        Ok(Self {
            runtime: PaligemmaCpuRuntime::initialize_from_host(installation)?,
        })
    }

    /// Initialize the accelerated CUDA/F16 backend using current available
    /// host RAM and current free device VRAM. Same provider
    /// (`CaptchaProviderId::PALIGEMMA_LOCAL`), same pinned model/revision,
    /// same processor pipeline, same `CaptchaSolveOutcome` semantics — only
    /// the executing device/dtype differs, and provenance reports it
    /// truthfully. No CPU fallback: see
    /// `PaligemmaCpuRuntime::initialize_cuda_f16_from_host`.
    #[cfg(feature = "local_paligemma_cuda")]
    pub fn initialize_cuda_f16_from_host(
        installation: &LocalModelInstallation,
    ) -> Result<Self, PaligemmaRuntimeFailure> {
        Ok(Self {
            runtime: PaligemmaCpuRuntime::initialize_cuda_f16_from_host(installation)?,
        })
    }

    /// Initialize the qualified `google/paligemma-3b-mix-448` checkpoint's
    /// accelerated CUDA/F16 backend using current available host RAM and
    /// current free device VRAM. Same provider
    /// (`CaptchaProviderId::PALIGEMMA_LOCAL`), same challenge kinds, same
    /// `CaptchaSolveOutcome` semantics as every other constructor on this
    /// type — only the pinned checkpoint (repository, revision, image
    /// envelope) and its own qualified resource floors differ, and
    /// provenance reports the difference truthfully. No CPU fallback: see
    /// `PaligemmaCpuRuntime::initialize_448_cuda_f16_from_host`.
    #[cfg(feature = "local_paligemma_cuda")]
    pub fn initialize_448_cuda_f16_from_host(
        installation: &LocalModelInstallation,
    ) -> Result<Self, PaligemmaRuntimeFailure> {
        Ok(Self {
            runtime: PaligemmaCpuRuntime::initialize_448_cuda_f16_from_host(installation)?,
        })
    }

    /// Consume the provider and release runtime-owned model resources.
    pub fn unload(self) {
        self.runtime.unload();
    }
}

#[async_trait::async_trait]
impl CaptchaProvider for PaligemmaLocalCaptchaProvider {
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
        let runtime_identity = self.runtime.runtime_identity();
        let processor_identity = self.runtime.processor_identity();
        // Truthful exact revision of the checkpoint this instance was
        // actually verified and initialized against — not a compile-time
        // constant selected independently of which manifest was activated,
        // so a 224 provider and a 448 provider never report each other's
        // revision. See `SCORPION_PALIGEMMA_448_PRODUCTION_RUNTIME_INTEGRATION_001`.
        let model_revision = self.runtime.model_revision();
        if request.challenge.visuals.len() != 1 {
            return failed(
                request,
                CaptchaSolveFailure::InvalidChallenge,
                started.elapsed(),
                model_revision,
                runtime_identity,
                processor_identity,
            );
        }
        let visual = &request.challenge.visuals[0];
        let Some(bytes) = visual.bytes() else {
            return failed(
                request,
                CaptchaSolveFailure::InvalidChallenge,
                started.elapsed(),
                model_revision,
                runtime_identity,
                processor_identity,
            );
        };
        let metadata = match self.runtime.image_metadata(bytes) {
            Ok(metadata) => metadata,
            Err(_) => {
                return failed(
                    request,
                    CaptchaSolveFailure::InvalidChallenge,
                    started.elapsed(),
                    model_revision,
                    runtime_identity,
                    processor_identity,
                )
            }
        };

        let outcome = tokio::time::timeout(
            request.deadline,
            solve_by_kind(
                &self.runtime,
                request,
                bytes,
                metadata.original_dimensions,
                visual,
            ),
        )
        .await;
        match outcome {
            Err(_) => failed(
                request,
                CaptchaSolveFailure::DeadlineExceeded,
                started.elapsed(),
                model_revision,
                runtime_identity,
                processor_identity,
            ),
            Ok(Err(failure)) => failed(
                request,
                failure,
                started.elapsed(),
                model_revision,
                runtime_identity,
                processor_identity,
            ),
            Ok(Ok((solution, elapsed))) => CaptchaSolveOutcome::Solved {
                solution,
                provenance: provenance(
                    request,
                    elapsed,
                    true,
                    model_revision,
                    runtime_identity,
                    processor_identity,
                ),
            },
        }
    }
}

async fn solve_by_kind(
    runtime: &PaligemmaCpuRuntime,
    request: &CaptchaSolveRequest,
    bytes: &[u8],
    original_dimensions: (u32, u32),
    visual: &crate::features::captcha::CaptchaVisualInput,
) -> Result<(CaptchaSolution, Duration), CaptchaSolveFailure> {
    match request.challenge.kind {
        CaptchaChallengeKind::PointSelection => {
            let box_ = runtime
                .detect(bytes, &request.challenge.instruction)
                .await
                .map_err(|_| CaptchaSolveFailure::LocalExecutionFailure)?;
            let (x, y) = point_from_box(&box_, original_dimensions);
            Ok((CaptchaSolution::Point { x, y }, box_.elapsed))
        }
        CaptchaChallengeKind::HorizontalOffset => {
            let (handle_label, target_label) = request
                .challenge
                .instruction
                .split_once(HORIZONTAL_OFFSET_DELIMITER)
                .ok_or(CaptchaSolveFailure::InvalidChallenge)?;
            let handle = runtime
                .detect(bytes, handle_label.trim())
                .await
                .map_err(|_| CaptchaSolveFailure::LocalExecutionFailure)?;
            let target = runtime
                .detect(bytes, target_label.trim())
                .await
                .map_err(|_| CaptchaSolveFailure::LocalExecutionFailure)?;
            let (handle_x, _) = point_from_box(&handle, original_dimensions);
            let (target_x, _) = point_from_box(&target, original_dimensions);
            let offset = target_x - handle_x;
            if !offset.is_finite() {
                return Err(CaptchaSolveFailure::InvalidProviderResponse);
            }
            Ok((
                CaptchaSolution::HorizontalOffset(offset),
                handle.elapsed + target.elapsed,
            ))
        }
        CaptchaChallengeKind::ImageGridSelection => {
            let grid = visual
                .image_grid()
                .ok_or(CaptchaSolveFailure::InvalidChallenge)?;
            if grid.original_dimensions() != original_dimensions {
                return Err(CaptchaSolveFailure::InvalidChallenge);
            }
            let schema = PaligemmaStringIdArraySchema {
                field: "selected_ids".into(),
                allowed_ids: grid
                    .cells()
                    .iter()
                    .map(|cell| cell.choice_id().to_owned())
                    .collect(),
                allow_empty: grid.empty_selection_valid(),
            };
            let result = runtime
                .generate_structured_ids(bytes, &image_grid_prompt(request, grid), &schema, 24)
                .await
                .map_err(|_| CaptchaSolveFailure::LocalExecutionFailure)?;
            let solution = parse_grid_solution(grid, &result.text)?;
            Ok((solution, result.elapsed))
        }
    }
}

/// The deterministic center of the model's own real bounding box, converted
/// from quantized bin units (`0..1024` over the fixed 224x224 processed
/// envelope) into the original image's pixel space. A faithful parse of the
/// model's genuine spatial answer, not a correction of it.
fn point_from_box(box_: &PaligemmaDetectionBox, original_dimensions: (u32, u32)) -> (f64, f64) {
    let (center_x_bin, center_y_bin) = box_.center_bins();
    let processed_x = center_x_bin / 1024.0 * 224.0;
    let processed_y = center_y_bin / 1024.0 * 224.0;
    let x = processed_x / 224.0 * f64::from(original_dimensions.0);
    let y = processed_y / 224.0 * f64::from(original_dimensions.1);
    (x, y)
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
        "answer en Task: {}\nGrid: {}x{}, {}x{} pixels\nCells: {}\nEmpty allowed: {}\nIDs must come from Cells.",
        request.challenge.instruction,
        grid.rows(),
        grid.columns(),
        grid.original_dimensions().0,
        grid.original_dimensions().1,
        serde_json::to_string(&cells).expect("cell JSON serialization is infallible"),
        grid.empty_selection_valid(),
    )
}

fn parse_grid_solution(
    grid: &CaptchaImageGridInput,
    text: &str,
) -> Result<CaptchaSolution, CaptchaSolveFailure> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct GridOutput {
        selected_ids: Vec<String>,
    }
    let trimmed = text.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return Err(CaptchaSolveFailure::InvalidProviderResponse);
    }
    let output: GridOutput =
        serde_json::from_str(trimmed).map_err(|_| CaptchaSolveFailure::InvalidProviderResponse)?;
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

fn grammar(kind: CaptchaChallengeKind) -> &'static str {
    match kind {
        CaptchaChallengeKind::ImageGridSelection => IMAGE_GRID_GRAMMAR,
        CaptchaChallengeKind::HorizontalOffset | CaptchaChallengeKind::PointSelection => {
            DETECT_GRAMMAR
        }
    }
}

fn provenance(
    request: &CaptchaSolveRequest,
    elapsed: Duration,
    succeeded: bool,
    model_revision: &str,
    runtime_identity: &'static str,
    processor_identity: &'static str,
) -> CaptchaSolveProvenance {
    CaptchaSolveProvenance::local_runtime(
        CaptchaProviderId::PALIGEMMA_LOCAL,
        CaptchaLocalRuntimeProvenance {
            model_revision: model_revision.into(),
            runtime_identity: runtime_identity.into(),
            processor_identity: processor_identity.into(),
            challenge_kind: request.challenge.kind,
            prompt_grammar_identity: grammar(request.challenge.kind).into(),
            elapsed,
            succeeded,
        },
    )
}

fn failed(
    request: &CaptchaSolveRequest,
    failure: CaptchaSolveFailure,
    elapsed: Duration,
    model_revision: &str,
    runtime_identity: &'static str,
    processor_identity: &'static str,
) -> CaptchaSolveOutcome {
    CaptchaSolveOutcome::Failed {
        failure,
        provenance: Some(provenance(
            request,
            elapsed,
            false,
            model_revision,
            runtime_identity,
            processor_identity,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::captcha::{
        solve_captcha, CaptchaChallenge, CaptchaImageGridCell, CaptchaProviderRegistry,
        CaptchaVisualInput,
    };
    use crate::features::paligemma_runtime::{
        paligemma_cpu_f32_manifest, PALIGEMMA_MODEL_REVISION, PALIGEMMA_PROCESSOR_ID,
    };
    use image::{DynamicImage, ImageBuffer, Rgb};
    use std::io::Cursor;
    use std::path::PathBuf;

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

    /// Qualification-host integration proof through the real production
    /// `CaptchaProvider::solve` seam and registry — `PointSelection`,
    /// `ImageGridSelection`, and `HorizontalOffset` all exercised. Requires
    /// the pinned checkpoint at `$SCORPION_PALIGEMMA_PINNED_ARTIFACTS`.
    #[tokio::test]
    #[ignore = "requires the pinned ~11.7 GB offline PaliGemma installation"]
    async fn real_provider_registry_runtime_and_strict_outcome() {
        let source = PathBuf::from(
            std::env::var("SCORPION_PALIGEMMA_PINNED_ARTIFACTS")
                .expect("set pinned offline artifact directory"),
        );
        let parent = tempfile::tempdir_in(source.parent().unwrap()).unwrap();
        let staging = parent.path().join("staging");
        let active = parent.path().join("active");
        std::fs::create_dir(&staging).unwrap();
        for name in [
            "model-00001-of-00003.safetensors",
            "model-00002-of-00003.safetensors",
            "model-00003-of-00003.safetensors",
            "config.json",
            "tokenizer.json",
            "tokenizer_config.json",
            "preprocessor_config.json",
        ] {
            std::fs::hard_link(source.join(name), staging.join(name)).unwrap();
        }
        let installation = paligemma_cpu_f32_manifest()
            .activate(&staging, &active)
            .unwrap();
        let provider = PaligemmaLocalCaptchaProvider::initialize_from_host(&installation).unwrap();
        let mut registry = CaptchaProviderRegistry::new();
        registry.register(&provider).unwrap();
        assert!(std::ptr::eq(
            registry
                .resolve(CaptchaProviderId::PALIGEMMA_LOCAL)
                .unwrap(),
            &provider as &dyn CaptchaProvider,
        ));
        assert_eq!(
            registry.qualification_state(
                CaptchaProviderId::PALIGEMMA_LOCAL,
                CaptchaChallengeKind::PointSelection,
            ),
            Some(CaptchaCapabilityQualification::ExecutableUnqualified)
        );

        // PointSelection, through the real `detect` grounding task.
        let point_bytes = square_fixture(320, 224, 160, 112, 44);
        let point_request = CaptchaSolveRequest {
            correlation_id: "real-offline-provider-point".into(),
            selected_provider: CaptchaProviderId::PALIGEMMA_LOCAL,
            challenge: CaptchaChallenge {
                kind: CaptchaChallengeKind::PointSelection,
                instruction: "red square".into(),
                visuals: vec![CaptchaVisualInput::materialized(
                    None,
                    "image/png",
                    point_bytes,
                )],
            },
            deadline: Duration::from_secs(1_800),
        };
        match solve_captcha(&provider, &point_request).await {
            CaptchaSolveOutcome::Solved {
                solution: CaptchaSolution::Point { x, y },
                provenance,
            } => {
                assert_eq!(provenance.provider, CaptchaProviderId::PALIGEMMA_LOCAL);
                let facts = provenance.local_runtime.unwrap();
                assert!(facts.succeeded);
                assert_eq!(facts.model_revision, PALIGEMMA_MODEL_REVISION);
                assert_eq!(facts.processor_identity, PALIGEMMA_PROCESSOR_ID);
                assert_eq!(facts.challenge_kind, CaptchaChallengeKind::PointSelection);
                // Independently verified against the pinned reference oracle
                // (see the frontier SDD): true center (160, 112).
                assert!((x - 160.0).abs() < 5.0, "x={x} not close to true 160");
                assert!((y - 112.0).abs() < 5.0, "y={y} not close to true 112");
            }
            other => {
                panic!("real provider did not produce a strict solved point outcome: {other:?}")
            }
        }

        // ImageGridSelection, through the shared JSON structured contract.
        let grid_bytes = square_fixture(224, 224, 112, 112, 40);
        let grid = crate::features::captcha::CaptchaImageGridInput::new(
            CaptchaVisualInput::materialized(None, "image/png", grid_bytes),
            (224, 224),
            1,
            1,
            vec![CaptchaImageGridCell::new("cell-1", 0, 0, 0, 0, 224, 224)],
            false,
        )
        .unwrap();
        let grid_request = CaptchaSolveRequest {
            correlation_id: "real-offline-provider-grid".into(),
            selected_provider: CaptchaProviderId::PALIGEMMA_LOCAL,
            challenge: CaptchaChallenge {
                kind: CaptchaChallengeKind::ImageGridSelection,
                instruction: "Select the only cell. Its stable ID is cell-1.".into(),
                visuals: vec![CaptchaVisualInput::materialized_full_grid(grid)],
            },
            deadline: Duration::from_secs(1_800),
        };
        match solve_captcha(&provider, &grid_request).await {
            CaptchaSolveOutcome::Solved {
                solution: CaptchaSolution::SelectedChoices(ids),
                ..
            } => assert_eq!(ids, ["cell-1"]),
            other => {
                panic!("real provider did not produce a strict solved grid outcome: {other:?}")
            }
        }

        // HorizontalOffset, through the two-detect convention.
        let mut bar_canvas = ImageBuffer::from_pixel(224u32, 224u32, Rgb([40u8, 40, 40]));
        for y in 0..224u32 {
            for dx in 0..6u32 {
                bar_canvas.put_pixel((40 + dx).min(223), y, Rgb([220, 40, 40]));
                bar_canvas.put_pixel((150 + dx).min(223), y, Rgb([40, 90, 220]));
            }
        }
        let mut bar_bytes = Vec::new();
        DynamicImage::ImageRgb8(bar_canvas)
            .write_to(&mut Cursor::new(&mut bar_bytes), image::ImageFormat::Png)
            .unwrap();
        let offset_request = CaptchaSolveRequest {
            correlation_id: "real-offline-provider-offset".into(),
            selected_provider: CaptchaProviderId::PALIGEMMA_LOCAL,
            challenge: CaptchaChallenge {
                kind: CaptchaChallengeKind::HorizontalOffset,
                instruction: "red bar -> blue bar".into(),
                visuals: vec![CaptchaVisualInput::materialized(
                    None,
                    "image/png",
                    bar_bytes,
                )],
            },
            deadline: Duration::from_secs(1_800),
        };
        match solve_captcha(&provider, &offset_request).await {
            CaptchaSolveOutcome::Solved {
                solution: CaptchaSolution::HorizontalOffset(value),
                ..
            } => assert!(value.is_finite()),
            other => {
                panic!("real provider did not produce a strict solved offset outcome: {other:?}")
            }
        }

        provider.unload();
    }

    /// Reference-parity mirror of `real_provider_registry_runtime_and_strict_outcome`
    /// through the accelerated CUDA/F16 backend instead of CPU/F32 — same
    /// provider (`CaptchaProviderId::PALIGEMMA_LOCAL`), same registry
    /// resolution, same three challenge kinds, same strict-outcome
    /// assertions, only the constructed runtime differs. Required by
    /// `SCORPION_PALIGEMMA_LOCAL_INFERENCE_LATENCY_QUALIFICATION_001`'s own
    /// regression gates (ImageGridSelection PASS, HorizontalOffset PASS,
    /// provider registry PASS, truthful provenance PASS) for the
    /// accelerated path specifically, not only the CPU/F32 baseline.
    #[cfg(feature = "local_paligemma_cuda")]
    #[tokio::test]
    #[ignore = "requires the pinned ~11.7 GB offline PaliGemma installation and a qualified CUDA/F16 host"]
    async fn real_cuda_provider_registry_runtime_and_strict_outcome() {
        use crate::features::paligemma_runtime::paligemma_cuda_f16_manifest;

        let source = PathBuf::from(
            std::env::var("SCORPION_PALIGEMMA_PINNED_ARTIFACTS")
                .expect("set pinned offline artifact directory"),
        );
        let parent = tempfile::tempdir_in(source.parent().unwrap()).unwrap();
        let staging = parent.path().join("staging");
        let active = parent.path().join("active");
        std::fs::create_dir(&staging).unwrap();
        for name in [
            "model-00001-of-00003.safetensors",
            "model-00002-of-00003.safetensors",
            "model-00003-of-00003.safetensors",
            "config.json",
            "tokenizer.json",
            "tokenizer_config.json",
            "preprocessor_config.json",
        ] {
            std::fs::hard_link(source.join(name), staging.join(name)).unwrap();
        }
        let installation = paligemma_cuda_f16_manifest()
            .activate(&staging, &active)
            .unwrap();
        let provider =
            PaligemmaLocalCaptchaProvider::initialize_cuda_f16_from_host(&installation).unwrap();
        let mut registry = CaptchaProviderRegistry::new();
        registry.register(&provider).unwrap();
        assert!(std::ptr::eq(
            registry
                .resolve(CaptchaProviderId::PALIGEMMA_LOCAL)
                .unwrap(),
            &provider as &dyn CaptchaProvider,
        ));
        assert_eq!(
            registry.qualification_state(
                CaptchaProviderId::PALIGEMMA_LOCAL,
                CaptchaChallengeKind::PointSelection,
            ),
            Some(CaptchaCapabilityQualification::ExecutableUnqualified)
        );

        // PointSelection, through the real `detect` grounding task.
        let point_bytes = square_fixture(320, 224, 160, 112, 44);
        let point_request = CaptchaSolveRequest {
            correlation_id: "real-offline-provider-cuda-point".into(),
            selected_provider: CaptchaProviderId::PALIGEMMA_LOCAL,
            challenge: CaptchaChallenge {
                kind: CaptchaChallengeKind::PointSelection,
                instruction: "red square".into(),
                visuals: vec![CaptchaVisualInput::materialized(
                    None,
                    "image/png",
                    point_bytes,
                )],
            },
            deadline: Duration::from_secs(1_800),
        };
        match solve_captcha(&provider, &point_request).await {
            CaptchaSolveOutcome::Solved {
                solution: CaptchaSolution::Point { .. },
                provenance,
            } => {
                assert_eq!(provenance.provider, CaptchaProviderId::PALIGEMMA_LOCAL);
                let facts = provenance.local_runtime.unwrap();
                assert!(facts.succeeded);
                assert_eq!(facts.model_revision, PALIGEMMA_MODEL_REVISION);
                assert_eq!(
                    facts.runtime_identity,
                    crate::features::paligemma_runtime::PALIGEMMA_CUDA_RUNTIME_IDENTITY
                );
                assert_eq!(
                    facts.processor_identity,
                    crate::features::paligemma_runtime::PALIGEMMA_CUDA_PROCESSOR_ID
                );
                assert_eq!(facts.challenge_kind, CaptchaChallengeKind::PointSelection);
            }
            other => {
                panic!(
                    "real CUDA provider did not produce a strict solved point outcome: {other:?}"
                )
            }
        }

        // ImageGridSelection, through the shared JSON structured contract.
        let grid_bytes = square_fixture(224, 224, 112, 112, 40);
        let grid = crate::features::captcha::CaptchaImageGridInput::new(
            CaptchaVisualInput::materialized(None, "image/png", grid_bytes),
            (224, 224),
            1,
            1,
            vec![CaptchaImageGridCell::new("cell-1", 0, 0, 0, 0, 224, 224)],
            false,
        )
        .unwrap();
        let grid_request = CaptchaSolveRequest {
            correlation_id: "real-offline-provider-cuda-grid".into(),
            selected_provider: CaptchaProviderId::PALIGEMMA_LOCAL,
            challenge: CaptchaChallenge {
                kind: CaptchaChallengeKind::ImageGridSelection,
                instruction: "Select the only cell. Its stable ID is cell-1.".into(),
                visuals: vec![CaptchaVisualInput::materialized_full_grid(grid)],
            },
            deadline: Duration::from_secs(1_800),
        };
        match solve_captcha(&provider, &grid_request).await {
            CaptchaSolveOutcome::Solved {
                solution: CaptchaSolution::SelectedChoices(ids),
                ..
            } => assert_eq!(ids, ["cell-1"]),
            other => {
                panic!("real CUDA provider did not produce a strict solved grid outcome: {other:?}")
            }
        }

        // HorizontalOffset, through the two-detect convention.
        let mut bar_canvas = ImageBuffer::from_pixel(224u32, 224u32, Rgb([40u8, 40, 40]));
        for y in 0..224u32 {
            for dx in 0..6u32 {
                bar_canvas.put_pixel((40 + dx).min(223), y, Rgb([220, 40, 40]));
                bar_canvas.put_pixel((150 + dx).min(223), y, Rgb([40, 90, 220]));
            }
        }
        let mut bar_bytes = Vec::new();
        DynamicImage::ImageRgb8(bar_canvas)
            .write_to(&mut Cursor::new(&mut bar_bytes), image::ImageFormat::Png)
            .unwrap();
        let offset_request = CaptchaSolveRequest {
            correlation_id: "real-offline-provider-cuda-offset".into(),
            selected_provider: CaptchaProviderId::PALIGEMMA_LOCAL,
            challenge: CaptchaChallenge {
                kind: CaptchaChallengeKind::HorizontalOffset,
                instruction: "red bar -> blue bar".into(),
                visuals: vec![CaptchaVisualInput::materialized(
                    None,
                    "image/png",
                    bar_bytes,
                )],
            },
            deadline: Duration::from_secs(1_800),
        };
        match solve_captcha(&provider, &offset_request).await {
            CaptchaSolveOutcome::Solved {
                solution: CaptchaSolution::HorizontalOffset(value),
                ..
            } => assert!(value.is_finite()),
            other => {
                panic!(
                    "real CUDA provider did not produce a strict solved offset outcome: {other:?}"
                )
            }
        }

        provider.unload();
    }
}
