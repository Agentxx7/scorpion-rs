#![cfg(feature = "local_paligemma_cuda")]

//! Live-UI PointSelection localization qualification for the CUDA/F16
//! `paligemma-local` backend — `SCORPION_PALIGEMMA_LIVE_UI_POINT_LOCALIZATION_QUALIFICATION_001`.
//!
//! `paligemma_point_precision_audit.rs`/`paligemma_cuda_point_precision_audit.rs`
//! qualified 7/7 against **solid-fill** squares on flat backgrounds. The
//! real Cloudflare Turnstile checkbox that
//! `SCORPION_CANONICAL_CAPTCHA_BROWSER_EXECUTION_BINDING_001` needs is a
//! ~22-24px **outlined** control embedded in a cluttered, realistic
//! widget — a genuinely untested capability gap
//! (`SCORPION_TURNSTILE_POST_ACTION_PROGRESSION_DIAGNOSTIC_001` isolated
//! exactly this). This suite tests the general capability ("locate this
//! small outlined UI control amid visual clutter"), never Cloudflare/
//! Turnstile-specific layout, branding, or wording. See
//! `docs/frontier/PALIGEMMA_LIVE_UI_POINT_LOCALIZATION_QUALIFICATION_SDD.md`
//! for the full frozen protocol (declared before any real inference ran).
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

const CANVAS: (u32, u32) = (224, 224);
const BACKGROUND: Rgb<u8> = Rgb([45, 48, 52]);
const OUTLINE: Rgb<u8> = Rgb([205, 210, 215]);
const LOW_CONTRAST_OUTLINE: Rgb<u8> = Rgb([66, 69, 75]);
const TEXT_COLOR: Rgb<u8> = Rgb([190, 195, 200]);
const LOGO_ACCENT_A: Rgb<u8> = Rgb([90, 200, 190]);
const LOGO_ACCENT_B: Rgb<u8> = Rgb([150, 110, 210]);
const DISTRACTOR_OUTLINE: Rgb<u8> = Rgb([140, 145, 150]);
const TARGET_OUTLINE: Rgb<u8> = Rgb([90, 150, 230]);
const STROKE: u32 = 2;

const RELIABLE_CONTAINMENT_THRESHOLD: usize = 7;
const STANDARD_FIXTURE_COUNT: usize = 8;
/// Frozen before any real inference — see the frontier SDD.
const CATASTROPHIC_MISS_PX: f64 = 100.0;

struct Fixture {
    label: &'static str,
    png: Vec<u8>,
    instruction: &'static str,
    true_center: (f64, f64),
    size: u32,
}

fn canvas() -> ImageBuffer<Rgb<u8>, Vec<u8>> {
    ImageBuffer::from_pixel(CANVAS.0, CANVAS.1, BACKGROUND)
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

/// An outlined (unfilled) square ring, matching the real Turnstile
/// checkbox's own rendering (a bordered control, not a solid block) — the
/// specific untested variable this qualification exists to check.
fn outlined_square(
    canvas: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    center: (u32, u32),
    size: u32,
    color: Rgb<u8>,
) {
    let half = size / 2;
    let x0 = center.0.saturating_sub(half);
    let y0 = center.1.saturating_sub(half);
    for y in y0..(y0 + size).min(canvas.height()) {
        for x in x0..(x0 + size).min(canvas.width()) {
            let on_border = x < x0 + STROKE
                || x >= x0 + size - STROKE
                || y < y0 + STROKE
                || y >= y0 + size - STROKE;
            if on_border {
                canvas.put_pixel(x, y, color);
            }
        }
    }
}

fn outlined_circle(
    canvas: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    center: (u32, u32),
    diameter: u32,
    color: Rgb<u8>,
) {
    let radius = diameter as f64 / 2.0;
    let inner = radius - STROKE as f64;
    let half = diameter / 2;
    let x0 = center.0.saturating_sub(half + 1);
    let y0 = center.1.saturating_sub(half + 1);
    for y in y0..(y0 + diameter + 2).min(canvas.height()) {
        for x in x0..(x0 + diameter + 2).min(canvas.width()) {
            let dx = f64::from(x) - f64::from(center.0);
            let dy = f64::from(y) - f64::from(center.1);
            let d = (dx * dx + dy * dy).sqrt();
            if d <= radius && d >= inner {
                canvas.put_pixel(x, y, color);
            }
        }
    }
}

/// A short row of solid blocks simulating a line of text at a fixed
/// glyph-like cadence — no real font rendering is available to test
/// fixtures, and none is needed: the model only needs to recognize "text
/// is present here," not read it.
fn text_like_label(
    canvas: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    top_left: (u32, u32),
    color: Rgb<u8>,
) {
    let widths = [6u32, 4, 7, 5, 3, 6, 4, 8, 5];
    let mut x = top_left.0;
    for w in widths {
        for gy in 0..8u32 {
            for gx in 0..w {
                let px = x + gx;
                let py = top_left.1 + gy;
                if px < canvas.width() && py < canvas.height() {
                    canvas.put_pixel(px, py, color);
                }
            }
        }
        x += w + 3;
    }
}

/// An abstract, generic logo-like distractor (circle + overlapping
/// triangle in an accent color) — deliberately not Cloudflare's flame
/// mark, not any real brand.
fn logo_like_distractor(canvas: &mut ImageBuffer<Rgb<u8>, Vec<u8>>, center: (u32, u32)) {
    let radius = 9.0;
    for y in center.1.saturating_sub(12)..(center.1 + 12).min(canvas.height()) {
        for x in center.0.saturating_sub(12)..(center.0 + 12).min(canvas.width()) {
            let dx = f64::from(x) - f64::from(center.0) + 4.0;
            let dy = f64::from(y) - f64::from(center.1);
            if (dx * dx + dy * dy).sqrt() <= radius {
                canvas.put_pixel(x, y, LOGO_ACCENT_A);
            }
        }
    }
    // Overlapping triangle to the upper-right of the circle.
    for y in center.1.saturating_sub(14)..center.1.saturating_sub(2) {
        for x in center.0..(center.0 + 14).min(canvas.width()) {
            let rel_y = f64::from(y) - f64::from(center.1.saturating_sub(14));
            let rel_x = f64::from(x) - f64::from(center.0);
            if rel_x <= rel_y * 1.1 {
                canvas.put_pixel(x, y, LOGO_ACCENT_B);
            }
        }
    }
}

fn plain_outlined_square() -> Fixture {
    let mut c = canvas();
    let center = (50u32, 50);
    outlined_square(&mut c, center, 22, OUTLINE);
    Fixture {
        label: "plain_outlined_square",
        png: encode_png(c),
        instruction: "the outlined square",
        true_center: (f64::from(center.0), f64::from(center.1)),
        size: 22,
    }
}

fn outlined_circle_fixture() -> Fixture {
    let mut c = canvas();
    let center = (170u32, 55);
    outlined_circle(&mut c, center, 22, OUTLINE);
    Fixture {
        label: "outlined_circle",
        png: encode_png(c),
        instruction: "the outlined circle",
        true_center: (f64::from(center.0), f64::from(center.1)),
        size: 22,
    }
}

fn adjacent_to_text() -> Fixture {
    let mut c = canvas();
    let center = (55u32, 125);
    outlined_square(&mut c, center, 22, OUTLINE);
    text_like_label(&mut c, (75, 118), TEXT_COLOR);
    Fixture {
        label: "adjacent_to_text",
        png: encode_png(c),
        instruction: "the outlined square",
        true_center: (f64::from(center.0), f64::from(center.1)),
        size: 22,
    }
}

fn adjacent_to_logo_distractor() -> Fixture {
    let mut c = canvas();
    let center = (160u32, 165);
    outlined_square(&mut c, center, 22, OUTLINE);
    logo_like_distractor(&mut c, (195, 160));
    Fixture {
        label: "adjacent_to_logo_distractor",
        png: encode_png(c),
        instruction: "the outlined square",
        true_center: (f64::from(center.0), f64::from(center.1)),
        size: 22,
    }
}

fn multiple_similar_controls() -> Fixture {
    let mut c = canvas();
    let target = (110u32, 190);
    outlined_square(&mut c, (55, 195), 22, DISTRACTOR_OUTLINE);
    outlined_square(&mut c, (170, 185), 22, DISTRACTOR_OUTLINE);
    outlined_square(&mut c, target, 22, TARGET_OUTLINE);
    Fixture {
        label: "multiple_similar_controls",
        png: encode_png(c),
        instruction: "the outlined square with the blue border",
        true_center: (f64::from(target.0), f64::from(target.1)),
        size: 22,
    }
}

fn low_contrast() -> Fixture {
    let mut c = canvas();
    let center = (140u32, 40);
    outlined_square(&mut c, center, 22, LOW_CONTRAST_OUTLINE);
    Fixture {
        label: "low_contrast",
        png: encode_png(c),
        instruction: "the outlined square",
        true_center: (f64::from(center.0), f64::from(center.1)),
        size: 22,
    }
}

fn near_edge() -> Fixture {
    let mut c = canvas();
    let center = (15u32, 15);
    outlined_square(&mut c, center, 20, OUTLINE);
    Fixture {
        label: "near_edge",
        png: encode_png(c),
        instruction: "the outlined square",
        true_center: (f64::from(center.0), f64::from(center.1)),
        size: 20,
    }
}

fn smallest_stress() -> Fixture {
    let mut c = canvas();
    let center = (95u32, 95);
    outlined_square(&mut c, center, 16, OUTLINE);
    Fixture {
        label: "smallest_stress",
        png: encode_png(c),
        instruction: "the outlined square",
        true_center: (f64::from(center.0), f64::from(center.1)),
        size: 16,
    }
}

/// The exact 8 standard fixtures, frozen before any real inference.
fn required_matrix() -> Vec<Fixture> {
    vec![
        plain_outlined_square(),
        outlined_circle_fixture(),
        adjacent_to_text(),
        adjacent_to_logo_distractor(),
        multiple_similar_controls(),
        low_contrast(),
        near_edge(),
        smallest_stress(),
    ]
}

struct Trial {
    label: &'static str,
    true_center: (f64, f64),
    size: u32,
    returned: Option<(f64, f64)>,
    valid_structured_output: bool,
    elapsed: Duration,
}

async fn run_trial(provider: &PaligemmaLocalCaptchaProvider, fixture: &Fixture) -> Trial {
    let request = CaptchaSolveRequest {
        correlation_id: format!("paligemma-live-ui-matrix-{}", fixture.label),
        selected_provider: CaptchaProviderId::PALIGEMMA_LOCAL,
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
        size: fixture.size,
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

/// Actionable-region containment: the control's own true bounding box —
/// identical criterion to the existing solid-fill audits and to the
/// production browser-action seam. Never a generic distance threshold.
fn contained(trial: &Trial) -> bool {
    match trial.returned {
        Some((x, y)) => {
            let half = f64::from(trial.size) / 2.0;
            (x - trial.true_center.0).abs() <= half && (y - trial.true_center.1).abs() <= half
        }
        None => false,
    }
}

fn report(trials: &[Trial]) {
    let diagonal = (f64::from(CANVAS.0)).hypot(f64::from(CANVAS.1));
    eprintln!(
        "{:<28} {:>14} {:>14} {:>9} {:>8} {:>10} {:>7} {:>8}",
        "target", "true_center", "returned", "err_px", "norm", "contained", "valid", "secs"
    );
    for trial in trials {
        let returned_text = match trial.returned {
            Some((x, y)) => format!("({x:.1},{y:.1})"),
            None => "-".to_string(),
        };
        let error = euclidean_error(trial);
        eprintln!(
            "{:<28} {:>14} {:>14} {:>9} {:>8} {:>10} {:>7} {:>8.3}",
            trial.label,
            format!("({:.1},{:.1})", trial.true_center.0, trial.true_center.1),
            returned_text,
            error.map(|v| format!("{v:.1}")).unwrap_or("-".into()),
            error
                .map(|v| format!("{:.3}", v / diagonal))
                .unwrap_or("-".into()),
            contained(trial),
            trial.valid_structured_output,
            trial.elapsed.as_secs_f64(),
        );
    }
}

/// Real accelerated paligemma-local (CUDA/F16) live-UI outlined-control
/// point-selection localization, across the frozen 8-fixture matrix,
/// through the exact production `CaptchaProvider::solve` seam.
#[tokio::test]
#[ignore = "requires the pinned ~11.7 GB PaliGemma installation and a qualified CUDA/F16 host"]
async fn real_live_ui_point_selection_localization_matrix() {
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

    // Determinism: rerun one fixture and require byte-identical structured
    // output from independent greedy decoding.
    let repeat_fixture = fixtures
        .iter()
        .find(|f| f.label == "plain_outlined_square")
        .unwrap();
    let repeat = run_trial(&provider, repeat_fixture).await;
    let first = trials
        .iter()
        .find(|t| t.label == "plain_outlined_square")
        .unwrap();
    assert_eq!(
        repeat.returned, first.returned,
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

    // Anti-degeneracy: genuine position dependence.
    let all_points: Vec<(f64, f64)> = trials.iter().map(|t| t.returned.unwrap()).collect();
    assert!(
        all_points.windows(2).any(|pair| pair[0] != pair[1]),
        "reject repeated constant coordinates across materially different targets"
    );
    assert!(
        all_points.iter().any(|(x, y)| (x - y).abs() > 1.0),
        "reject x == y regardless of target"
    );
    let left = trials
        .iter()
        .find(|t| t.label == "plain_outlined_square")
        .unwrap()
        .returned
        .unwrap();
    let right = trials
        .iter()
        .find(|t| t.label == "outlined_circle")
        .unwrap()
        .returned
        .unwrap();
    assert!(
        left.0 < right.0,
        "reject output independent of image position: the left-side fixture's predicted x \
         must be less than the right-side fixture's"
    );
    let top = trials
        .iter()
        .find(|t| t.label == "low_contrast")
        .unwrap()
        .returned
        .unwrap();
    let bottom = trials
        .iter()
        .find(|t| t.label == "multiple_similar_controls")
        .unwrap()
        .returned
        .unwrap();
    assert!(
        top.1 < bottom.1,
        "reject output independent of image position: the top-side fixture's predicted y \
         must be less than the bottom-side fixture's"
    );

    // Catastrophic-miss check on the smallest standard control, frozen
    // before this trial ran, independent of whether it counts as the one
    // permitted containment failure.
    let smallest = trials
        .iter()
        .find(|t| t.label == "smallest_stress")
        .unwrap();
    let smallest_error = euclidean_error(smallest).unwrap();
    eprintln!(
        "\nsmallest_stress catastrophic-miss check: err_px={smallest_error:.1} \
         (frozen bound: <= {CATASTROPHIC_MISS_PX})"
    );
    assert!(
        smallest_error <= CATASTROPHIC_MISS_PX,
        "the smallest standard control must not be catastrophically missed: \
         err_px={smallest_error:.1} exceeds the frozen {CATASTROPHIC_MISS_PX}px bound"
    );

    assert_eq!(trials.len(), STANDARD_FIXTURE_COUNT);
    let contained_count = trials.iter().filter(|t| contained(t)).count();
    eprintln!(
        "\nlive-UI outlined-control containment: {contained_count}/{STANDARD_FIXTURE_COUNT} \
         (frozen reliable-single-shot threshold: {RELIABLE_CONTAINMENT_THRESHOLD}/{STANDARD_FIXTURE_COUNT})"
    );
    assert!(
        contained_count >= RELIABLE_CONTAINMENT_THRESHOLD,
        "must clear the predefined reliable-single-shot threshold, frozen before this trial ran"
    );

    provider.unload();
}
