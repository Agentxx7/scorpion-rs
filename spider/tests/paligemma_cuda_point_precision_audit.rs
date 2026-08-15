#![cfg(feature = "local_paligemma_cuda")]

//! Reference-parity point-selection precision qualification for the
//! accelerated `paligemma-local` CUDA/F16 backend
//! (`SCORPION_PALIGEMMA_LOCAL_INFERENCE_LATENCY_QUALIFICATION_001`), using
//! EXACTLY the same 8-fixture required position matrix, WCAG 2.5.5 44px
//! actionable-region tolerance, anti-degeneracy assertions and determinism
//! proof as the frozen CPU/F32 qualification
//! (`spider/tests/paligemma_point_precision_audit.rs`). Only the
//! constructed provider differs — `initialize_cuda_f16_from_host` instead
//! of `initialize_from_host` — so a pass here is a genuine, independent
//! reference-parity check: the accelerated device/dtype must not trade
//! accuracy for latency.
//!
//! Ignored in ordinary CI: requires the pinned ~11.7 GB PaliGemma
//! installation and a qualified CUDA/F16 host with the required free VRAM.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use image::{ImageBuffer, Rgb};
use spider::features::captcha::{
    CaptchaChallenge, CaptchaChallengeKind, CaptchaProvider, CaptchaProviderId, CaptchaSolution,
    CaptchaSolveOutcome, CaptchaSolveRequest, CaptchaVisualInput,
};
use spider::features::paligemma_captcha::PaligemmaLocalCaptchaProvider;
use spider::features::paligemma_runtime::paligemma_cuda_f16_manifest;

const BACKGROUND: Rgb<u8> = Rgb([40, 40, 40]);
const TARGET_COLOR: Rgb<u8> = Rgb([220, 40, 40]);
const DISTRACTOR_COLOR: Rgb<u8> = Rgb([40, 90, 220]);
/// WCAG 2.5.5 minimum touch/click target size, in pixels — identical to the
/// CPU/F32 qualification's own tolerance.
const STANDARD_SIDE: u32 = 44;
/// Deliberately below the actionable minimum, to probe the precision floor.
const SMALL_SIDE: u32 = 16;
const CANVAS: (u32, u32) = (224, 224);

const RELIABLE_CONTAINMENT_THRESHOLD: usize = 6;
const STANDARD_FIXTURE_COUNT: usize = 7;
/// The closed CPU/F32 `paligemma-local` frontier's own measured
/// containment, out of 7 standard fixtures — the reference-parity floor
/// this accelerated backend must not fall below.
const CPU_F32_BASELINE_CONTAINED: usize = 7;

struct Fixture {
    label: &'static str,
    png: Vec<u8>,
    true_center: (f64, f64),
    side: u32,
}

fn square(
    canvas: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    center: (u32, u32),
    side: u32,
    color: Rgb<u8>,
) {
    let half = side / 2;
    let x0 = center.0.saturating_sub(half);
    let y0 = center.1.saturating_sub(half);
    for y in y0..(y0 + side).min(canvas.height()) {
        for x in x0..(x0 + side).min(canvas.width()) {
            canvas.put_pixel(x, y, color);
        }
    }
}

fn encode_png(image: ImageBuffer<Rgb<u8>, Vec<u8>>) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

fn single_fixture(label: &'static str, center: (u32, u32), side: u32) -> Fixture {
    let mut canvas = ImageBuffer::from_pixel(CANVAS.0, CANVAS.1, BACKGROUND);
    square(&mut canvas, center, side, TARGET_COLOR);
    Fixture {
        label,
        png: encode_png(canvas),
        true_center: (f64::from(center.0), f64::from(center.1)),
        side,
    }
}

fn distractor_fixture() -> Fixture {
    let mut canvas = ImageBuffer::from_pixel(CANVAS.0, CANVAS.1, BACKGROUND);
    let target = (72u32, 112u32);
    let distractor = (152u32, 112u32);
    square(&mut canvas, distractor, STANDARD_SIDE, DISTRACTOR_COLOR);
    square(&mut canvas, target, STANDARD_SIDE, TARGET_COLOR);
    Fixture {
        label: "distractor",
        png: encode_png(canvas),
        true_center: (f64::from(target.0), f64::from(target.1)),
        side: STANDARD_SIDE,
    }
}

/// The exact same required position matrix as the CPU/F32 qualification.
fn required_matrix() -> Vec<Fixture> {
    vec![
        single_fixture("upper_left", (30, 30), STANDARD_SIDE),
        single_fixture("upper_right", (194, 30), STANDARD_SIDE),
        single_fixture("lower_left", (30, 194), STANDARD_SIDE),
        single_fixture("lower_right", (194, 194), STANDARD_SIDE),
        single_fixture("center", (112, 112), STANDARD_SIDE),
        single_fixture("asymmetric", (60, 160), STANDARD_SIDE),
        single_fixture("small_isolated", (180, 50), SMALL_SIDE),
        distractor_fixture(),
    ]
}

struct Trial {
    label: &'static str,
    true_center: (f64, f64),
    side: u32,
    returned: Option<(f64, f64)>,
    valid_structured_output: bool,
    elapsed: Duration,
}

async fn run_trial(provider: &PaligemmaLocalCaptchaProvider, fixture: &Fixture) -> Trial {
    let request = CaptchaSolveRequest {
        correlation_id: format!("paligemma-cuda-precision-matrix-{}", fixture.label),
        selected_provider: CaptchaProviderId::PALIGEMMA_LOCAL,
        challenge: CaptchaChallenge {
            kind: CaptchaChallengeKind::PointSelection,
            instruction: "red square".to_string(),
            visuals: vec![CaptchaVisualInput::materialized(
                None,
                "image/png",
                fixture.png.clone(),
            )],
        },
        deadline: Duration::from_secs(1_800),
    };
    let start = Instant::now();
    let outcome = provider.solve(&request).await;
    let elapsed = start.elapsed();
    let (returned, valid) = match outcome {
        CaptchaSolveOutcome::Solved {
            solution: CaptchaSolution::Point { x, y },
            ..
        } => (Some((x, y)), true),
        CaptchaSolveOutcome::Solved { .. } => (None, true),
        CaptchaSolveOutcome::Failed { .. } => (None, false),
    };
    Trial {
        label: fixture.label,
        true_center: fixture.true_center,
        side: fixture.side,
        returned,
        valid_structured_output: valid,
        elapsed,
    }
}

fn euclidean_error(trial: &Trial) -> Option<f64> {
    trial.returned.map(|(x, y)| {
        let dx = x - trial.true_center.0;
        let dy = y - trial.true_center.1;
        (dx * dx + dy * dy).sqrt()
    })
}

/// Actionable-region containment: identical criterion to the CPU/F32
/// qualification and the production browser-action seam.
fn contained(trial: &Trial) -> bool {
    match trial.returned {
        Some((x, y)) => {
            let half = f64::from(trial.side) / 2.0;
            (x - trial.true_center.0).abs() <= half && (y - trial.true_center.1).abs() <= half
        }
        None => false,
    }
}

fn report(trials: &[Trial]) {
    let diagonal = (f64::from(CANVAS.0)).hypot(f64::from(CANVAS.1));
    eprintln!(
        "{:<15} {:>14} {:>14} {:>9} {:>8} {:>10} {:>7}",
        "target", "true_center", "returned", "err_px", "norm", "contained", "valid"
    );
    for trial in trials {
        let returned_text = match trial.returned {
            Some((x, y)) => format!("({x:.1},{y:.1})"),
            None => "-".to_string(),
        };
        let error = euclidean_error(trial);
        eprintln!(
            "{:<15} {:>14} {:>14} {:>9} {:>8} {:>10} {:>7}",
            trial.label,
            format!("({:.1},{:.1})", trial.true_center.0, trial.true_center.1),
            returned_text,
            error.map(|v| format!("{v:.1}")).unwrap_or("-".into()),
            error
                .map(|v| format!("{:.3}", v / diagonal))
                .unwrap_or("-".into()),
            contained(trial),
            trial.valid_structured_output,
        );
    }
}

/// Real accelerated paligemma-local (CUDA/F16) point-selection precision,
/// across the full required position matrix, through the exact production
/// `CaptchaProvider::solve` seam — the same seam the CPU/F32 qualification
/// exercises, only the constructed backend differs.
#[tokio::test]
#[ignore = "requires the pinned ~11.7 GB PaliGemma installation and a qualified CUDA/F16 host"]
async fn real_cuda_point_selection_precision_matrix() {
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

    let fixtures = required_matrix();
    let mut trials = Vec::new();
    for fixture in &fixtures {
        let trial = run_trial(&provider, fixture).await;
        eprintln!(
            "[progress] {} -> {:?} in {:.3}s",
            trial.label,
            trial.returned,
            trial.elapsed.as_secs_f64()
        );
        trials.push(trial);
    }

    // Determinism: rerun one fixture (center) and require byte-identical
    // structured output from independent greedy decoding.
    let center_fixture = fixtures.iter().find(|f| f.label == "center").unwrap();
    let repeat = run_trial(&provider, center_fixture).await;
    let first_center = trials.iter().find(|t| t.label == "center").unwrap();
    assert_eq!(
        repeat.returned, first_center.returned,
        "deterministic greedy decoding must reproduce the identical point"
    );

    report(&trials);

    // Structured-output validity: every trial must produce a strictly
    // parsed, in-bounds structured answer.
    for trial in &trials {
        assert!(
            trial.valid_structured_output && trial.returned.is_some(),
            "{} produced no valid structured point",
            trial.label
        );
    }

    // Anti-degeneracy: genuine position dependence is the important
    // requirement, independent of raw precision.
    let all_points: Vec<(f64, f64)> = trials.iter().map(|t| t.returned.unwrap()).collect();
    assert!(
        all_points.windows(2).any(|pair| pair[0] != pair[1]),
        "reject repeated constant coordinates across materially different targets"
    );
    assert!(
        all_points.iter().any(|(x, y)| (x - y).abs() > 1.0),
        "reject x == y regardless of target"
    );
    let mean = |points: &[(f64, f64)], pick: fn((f64, f64)) -> f64| -> f64 {
        points.iter().copied().map(pick).sum::<f64>() / points.len() as f64
    };
    let left: Vec<_> = ["upper_left", "lower_left"]
        .iter()
        .map(|label| {
            trials
                .iter()
                .find(|t| t.label == *label)
                .unwrap()
                .returned
                .unwrap()
        })
        .collect();
    let right: Vec<_> = ["upper_right", "lower_right"]
        .iter()
        .map(|label| {
            trials
                .iter()
                .find(|t| t.label == *label)
                .unwrap()
                .returned
                .unwrap()
        })
        .collect();
    assert!(
        mean(&left, |(x, _)| x) < mean(&right, |(x, _)| x),
        "reject output independent of image position: left-side targets must average a lower \
         predicted x than right-side targets"
    );
    let top: Vec<_> = ["upper_left", "upper_right"]
        .iter()
        .map(|label| {
            trials
                .iter()
                .find(|t| t.label == *label)
                .unwrap()
                .returned
                .unwrap()
        })
        .collect();
    let bottom: Vec<_> = ["lower_left", "lower_right"]
        .iter()
        .map(|label| {
            trials
                .iter()
                .find(|t| t.label == *label)
                .unwrap()
                .returned
                .unwrap()
        })
        .collect();
    assert!(
        mean(&top, |(_, y)| y) < mean(&bottom, |(_, y)| y),
        "reject output independent of image position: top-side targets must average a lower \
         predicted y than bottom-side targets"
    );

    let standard: Vec<&Trial> = trials.iter().filter(|t| t.side == STANDARD_SIDE).collect();
    assert_eq!(standard.len(), STANDARD_FIXTURE_COUNT);
    let contained_count = standard.iter().filter(|t| contained(t)).count();
    eprintln!(
        "\nstandard-size (44px) actionable containment: {contained_count}/{STANDARD_FIXTURE_COUNT} \
         (CPU/F32 reference-parity baseline: {CPU_F32_BASELINE_CONTAINED}/{STANDARD_FIXTURE_COUNT}; \
         reliable-single-shot threshold: {RELIABLE_CONTAINMENT_THRESHOLD}/{STANDARD_FIXTURE_COUNT})"
    );
    assert!(
        contained_count >= RELIABLE_CONTAINMENT_THRESHOLD,
        "must clear the predefined reliable-single-shot threshold"
    );
    assert_eq!(
        contained_count, CPU_F32_BASELINE_CONTAINED,
        "the accelerated CUDA/F16 backend must not trade accuracy for latency: \
         must reference-parity match the CPU/F32 backend's own 7/7 containment"
    );

    provider.unload();
}
