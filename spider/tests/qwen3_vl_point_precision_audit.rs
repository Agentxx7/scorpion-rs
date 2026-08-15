#![cfg(feature = "local_qwen3_vl")]

//! Formal point-selection precision/input-envelope qualification for
//! `SCORPION_QWEN3_VL_LOCAL_CAPTCHA_POINT_PRECISION_AND_INPUT_ENVELOPE_001`,
//! re-run against the corrected runtime after
//! `SCORPION_QWEN3_VL_CANDLE_REFERENCE_PARITY_ROOT_CAUSE_001`'s MRoPE and
//! prefill-causal-mask fixes.
//!
//! Synthetic fixtures only (no Turnstile, no browser) — this measures the
//! `qwen3-vl-local` provider's real point-selection precision at the
//! existing qualified 320x224 envelope across the full required position
//! matrix (four corners, center, an asymmetric x≠y position, a small
//! isolated target, and a distractor-containing target), with known ground
//! truth, through the exact production `CaptchaProvider::solve` seam. No
//! coordinate is ever encoded in an instruction; the model must locate the
//! target visually.
//!
//! ## Tolerance
//!
//! "Actionable" is defined as the predicted point landing inside the true
//! target's own bounding box — the same containment semantics the
//! production browser-action seam already enforces for
//! `BrowserChallengeAction::ExactTargetClick`
//! (`browser_challenge.rs::apply`). A numerically small error that still
//! lands outside the target is a failure, per the frontier's own
//! instruction. Standard-size fixtures use a 44x44px target — the WCAG
//! 2.5.5 minimum touch/click target size, chosen before any trial was run
//! (not tuned to this model's measured error). The `small_isolated` fixture
//! deliberately uses a 16px target to probe the floor, and is reported but
//! not counted toward the envelope decision, since real CAPTCHA UI point
//! targets are not normally smaller than a realistic click target.
//!
//! ## Result and envelope decision (see the frontier SDD for full detail)
//!
//! At the qualified 320x224 envelope, real position-dependent (non-
//! degenerate) answers are produced, but only 3 of 7 standard-size fixtures
//! land inside their actionable target region (one further miss, 144px,
//! swapped an entire quadrant). A controlled widened-envelope probe
//! (640x448, run and reverted, not shipped) did not reproduce a material,
//! reliable improvement: several trials collapsed to echoing the prompt's
//! declared width/height bound verbatim regardless of true target position
//! (a new degenerate failure mode absent at 320x224), and a genuine,
//! narrow structured-generation search gap was found for certain
//! mid-magnitude numeric values (approximately 550-569) — unreachable at
//! the current 320x224 envelope's coordinate range, so intentionally left
//! unfixed rather than opportunistically touching the grammar engine for a
//! path this frontier does not ship.
//!
//! Verdict: `QWEN3_VL_2B_POINT_PRECISION_INSUFFICIENT`. The qualified
//! envelope remains 320x224 (unchanged; widening was evaluated and
//! rejected on evidence, not skipped). `qualification_state` for
//! `PointSelection` remains `ExecutableUnqualified` — this frontier does
//! not promote it to `EmpiricallyQualified`.
//!
//! This test itself still gates on the properties that matter for
//! regression protection: structured-output validity, deterministic
//! repeatability, and genuine position dependence (rejecting constant,
//! `x == y`-regardless, or position-independent output) — a return to
//! degenerate collapse would fail this test even though precision
//! sufficiency is not asserted.
//!
//! Ignored in ordinary CI: requires the pinned 4.25 GB Qwen3-VL installation
//! and a qualified CPU/F32 host.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use image::{ImageBuffer, Rgb};
use spider::features::captcha::{
    CaptchaChallenge, CaptchaChallengeKind, CaptchaProvider, CaptchaProviderId, CaptchaSolution,
    CaptchaSolveOutcome, CaptchaSolveRequest, CaptchaVisualInput,
};
use spider::features::qwen3_vl_captcha::Qwen3VlLocalCaptchaProvider;
use spider::features::qwen3_vl_runtime::qwen3_vl_cpu_f32_manifest;

const BACKGROUND: Rgb<u8> = Rgb([40, 40, 40]);
const TARGET_COLOR: Rgb<u8> = Rgb([220, 40, 40]);
const DISTRACTOR_COLOR: Rgb<u8> = Rgb([40, 90, 220]);
/// WCAG 2.5.5 minimum touch/click target size, in pixels. Chosen as the
/// actionable-region tolerance before any trial was run.
const STANDARD_SIDE: u32 = 44;
/// Deliberately below the actionable minimum, to probe the precision floor.
const SMALL_SIDE: u32 = 16;
const CANVAS: (u32, u32) = (320, 224);

const SINGLE_INSTRUCTION: &str = "The image shows a small colored square on a plain dark \
     background. Return the pixel coordinates of the exact center of that square.";
const DISTRACTOR_INSTRUCTION: &str = "The image shows two small colored squares on a dark \
     background: one red, one blue. Return the pixel coordinates of the center of the RED \
     square only.";

struct Fixture {
    label: &'static str,
    png: Vec<u8>,
    true_center: (f64, f64),
    side: u32,
    instruction: &'static str,
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
        instruction: SINGLE_INSTRUCTION,
    }
}

fn distractor_fixture() -> Fixture {
    let mut canvas = ImageBuffer::from_pixel(CANVAS.0, CANVAS.1, BACKGROUND);
    let target = (100u32, 112u32);
    let distractor = (220u32, 112u32);
    square(&mut canvas, distractor, STANDARD_SIDE, DISTRACTOR_COLOR);
    square(&mut canvas, target, STANDARD_SIDE, TARGET_COLOR);
    Fixture {
        label: "distractor",
        png: encode_png(canvas),
        true_center: (f64::from(target.0), f64::from(target.1)),
        side: STANDARD_SIDE,
        instruction: DISTRACTOR_INSTRUCTION,
    }
}

/// The full required position matrix: four corners, center, an asymmetric
/// x != y position, a small isolated target, and a distractor-containing
/// target.
fn required_matrix() -> Vec<Fixture> {
    vec![
        single_fixture("upper_left", (40, 40), STANDARD_SIDE),
        single_fixture("upper_right", (280, 40), STANDARD_SIDE),
        single_fixture("lower_left", (40, 184), STANDARD_SIDE),
        single_fixture("lower_right", (280, 184), STANDARD_SIDE),
        single_fixture("center", (160, 112), STANDARD_SIDE),
        single_fixture("asymmetric", (90, 170), STANDARD_SIDE),
        single_fixture("small_isolated", (250, 60), SMALL_SIDE),
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

async fn run_trial(provider: &Qwen3VlLocalCaptchaProvider, fixture: &Fixture) -> Trial {
    let request = CaptchaSolveRequest {
        correlation_id: format!("precision-matrix-{}", fixture.label),
        selected_provider: CaptchaProviderId::QWEN3_VL_LOCAL,
        challenge: CaptchaChallenge {
            kind: CaptchaChallengeKind::PointSelection,
            instruction: fixture.instruction.to_string(),
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

/// Actionable-region containment: the same axis-aligned bounding-box check
/// the production browser-action seam performs for `ExactTargetClick`.
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

/// Real qwen3-vl-local point-selection precision, across the full required
/// position matrix, through the exact production `CaptchaProvider::solve`
/// seam — re-run after `SCORPION_QWEN3_VL_CANDLE_REFERENCE_PARITY_ROOT_CAUSE_001`'s
/// MRoPE and prefill-causal-mask fixes. See the module doc comment for the
/// tolerance definition and this frontier's envelope-decision evidence.
#[tokio::test]
#[ignore = "requires the pinned 4.25 GB Qwen3-VL installation and a qualified CPU/F32 host"]
async fn real_point_selection_precision_matrix() {
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
    let provider = Qwen3VlLocalCaptchaProvider::initialize_from_host(&installation).unwrap();

    let fixtures = required_matrix();
    let mut trials = Vec::new();
    for fixture in &fixtures {
        let trial = run_trial(&provider, fixture).await;
        eprintln!(
            "[progress] {} -> {:?} in {:.1}s",
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
    // parsed, in-bounds structured answer (garbage/None is a hard failure,
    // independent of accuracy).
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

    // Accuracy is measured and reported, not gated: this frontier's own
    // evidence (see module doc comment) classifies current precision as
    // QWEN3_VL_2B_POINT_PRECISION_INSUFFICIENT for reliable single-shot
    // actionable clicks, without this being a runtime defect.
    let standard: Vec<&Trial> = trials.iter().filter(|t| t.side == STANDARD_SIDE).collect();
    let contained_count = standard.iter().filter(|t| contained(t)).count();
    eprintln!(
        "\nstandard-size (44px) actionable containment: {contained_count}/{} \
         — informational, not gated (see module doc comment)",
        standard.len()
    );

    provider.unload();
}

/// Real qwen3-vl-local horizontal-offset precision, with known ground
/// truth, through the exact production `CaptchaProvider::solve` seam.
#[tokio::test]
#[ignore = "requires the pinned 4.25 GB Qwen3-VL installation and a qualified CPU/F32 host"]
async fn real_horizontal_offset_qualification() {
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
    let provider = Qwen3VlLocalCaptchaProvider::initialize_from_host(&installation).unwrap();

    let (width, height) = CANVAS;
    let handle_x = 40u32;
    let target_x = 180u32;
    let true_offset = f64::from(target_x) - f64::from(handle_x);
    let mut canvas = image::ImageBuffer::from_pixel(width, height, BACKGROUND);
    for y in 0..height {
        for dx in 0..6u32 {
            canvas.put_pixel((handle_x + dx).min(width - 1), y, TARGET_COLOR);
            canvas.put_pixel((target_x + dx).min(width - 1), y, DISTRACTOR_COLOR);
        }
    }
    let png = encode_png(canvas);

    let request = CaptchaSolveRequest {
        correlation_id: "precision-matrix-horizontal-offset".into(),
        selected_provider: CaptchaProviderId::QWEN3_VL_LOCAL,
        challenge: CaptchaChallenge {
            kind: CaptchaChallengeKind::HorizontalOffset,
            instruction: "The image shows a vertical red handle bar and a vertical blue \
                          target bar on a dark background. Return the horizontal pixel \
                          distance needed to drag the red handle so it aligns with the \
                          blue target."
                .into(),
            visuals: vec![CaptchaVisualInput::materialized(None, "image/png", png)],
        },
        deadline: Duration::from_secs(1_800),
    };
    let start = Instant::now();
    let outcome = provider.solve(&request).await;
    let elapsed = start.elapsed();
    eprintln!(
        "[result] horizontal_offset true={true_offset} outcome={outcome:?} in {:.1}s",
        elapsed.as_secs_f64()
    );
    match outcome {
        CaptchaSolveOutcome::Solved {
            solution: CaptchaSolution::HorizontalOffset(value),
            ..
        } => {
            assert!(value.is_finite() && value >= 0.0 && value <= f64::from(width));
        }
        other => panic!("expected a valid structured horizontal-offset solution, got {other:?}"),
    }
    provider.unload();
}
