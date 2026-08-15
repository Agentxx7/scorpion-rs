#![cfg(feature = "local_qwen3_vl")]

//! Controlled, deterministic-ground-truth qualification matrix for
//! `SCORPION_QWEN3_VL_LOCAL_CAPTCHA_POINT_PRECISION_AND_INPUT_ENVELOPE_001`.
//!
//! Synthetic fixtures only (no Turnstile, no browser) — this measures the
//! `qwen3-vl-local` provider's real point-selection precision at several
//! candidate input envelopes and target positions, with known ground truth,
//! through the exact production `CaptchaProvider::solve` seam. No coordinate
//! is ever encoded in an instruction; the model must locate the target
//! visually.
//!
//! Ignored in ordinary CI: requires the pinned 4.25 GB Qwen3-VL installation
//! and a qualified CPU/F32 host. Not part of any regression gate — this is
//! audit instrumentation, run and read manually.

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

/// One controlled fixture: an exact-envelope PNG with a known-position
/// target square (and optionally a same-size, different-colored distractor
/// square elsewhere), plus the true center the model should return.
struct Fixture {
    label: &'static str,
    png: Vec<u8>,
    true_center: (f64, f64),
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

fn corner_fixture(width: u32, height: u32) -> Fixture {
    let side = (width.min(height) / 14).max(16);
    let mut canvas = ImageBuffer::from_pixel(width, height, BACKGROUND);
    let center = (side, side);
    square(&mut canvas, center, side, TARGET_COLOR);
    Fixture {
        label: "corner",
        png: encode_png(canvas),
        true_center: (center.0 as f64, center.1 as f64),
    }
}

fn edge_fixture(width: u32, height: u32) -> Fixture {
    let side = (width.min(height) / 14).max(16);
    let mut canvas = ImageBuffer::from_pixel(width, height, BACKGROUND);
    let center = (width - side, height / 2);
    square(&mut canvas, center, side, TARGET_COLOR);
    Fixture {
        label: "edge",
        png: encode_png(canvas),
        true_center: (center.0 as f64, center.1 as f64),
    }
}

fn center_fixture(width: u32, height: u32) -> Fixture {
    let side = (width.min(height) / 14).max(16);
    let mut canvas = ImageBuffer::from_pixel(width, height, BACKGROUND);
    let center = (width / 2, height / 2);
    square(&mut canvas, center, side, TARGET_COLOR);
    Fixture {
        label: "center",
        png: encode_png(canvas),
        true_center: (center.0 as f64, center.1 as f64),
    }
}

fn distractor_fixture(width: u32, height: u32) -> Fixture {
    let side = (width.min(height) / 14).max(16);
    let mut canvas = ImageBuffer::from_pixel(width, height, BACKGROUND);
    let target_center = (width / 3, height / 2);
    let distractor_center = (2 * width / 3, height / 2);
    square(&mut canvas, distractor_center, side, DISTRACTOR_COLOR);
    square(&mut canvas, target_center, side, TARGET_COLOR);
    Fixture {
        label: "distractor",
        png: encode_png(canvas),
        true_center: (target_center.0 as f64, target_center.1 as f64),
    }
}

struct Trial {
    envelope: &'static str,
    label: &'static str,
    true_center: (f64, f64),
    returned: Option<(f64, f64)>,
    valid_structured_output: bool,
    elapsed: Duration,
}

async fn run_trial(
    provider: &Qwen3VlLocalCaptchaProvider,
    envelope: &'static str,
    fixture: &Fixture,
) -> Trial {
    let instruction = if fixture.label == "distractor" {
        "The image shows two small colored squares on a dark background: \
         one red, one blue. Return the pixel coordinates of the center of \
         the RED square only."
            .to_string()
    } else {
        "The image shows a small colored square on a plain dark background. \
         Return the pixel coordinates of the exact center of that square."
            .to_string()
    };
    let request = CaptchaSolveRequest {
        correlation_id: format!("precision-audit-{envelope}-{}", fixture.label),
        selected_provider: CaptchaProviderId::QWEN3_VL_LOCAL,
        challenge: CaptchaChallenge {
            kind: CaptchaChallengeKind::PointSelection,
            instruction,
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
        envelope,
        label: fixture.label,
        true_center: fixture.true_center,
        returned,
        valid_structured_output: valid,
        elapsed,
    }
}

fn report(trials: &[Trial]) {
    eprintln!(
        "{:<10} {:<12} {:>16} {:>16} {:>12} {:>10} {:>8}",
        "envelope", "target", "true_center", "returned", "abs_err_px", "norm_err", "elapsed"
    );
    for trial in trials {
        let returned_text = match trial.returned {
            Some((x, y)) => format!("({x:.1},{y:.1})"),
            None => "-".to_string(),
        };
        let (abs_err, norm_err) = match trial.returned {
            Some((x, y)) => {
                let dx = x - trial.true_center.0;
                let dy = y - trial.true_center.1;
                let abs_err = (dx * dx + dy * dy).sqrt();
                (Some(abs_err), Some(abs_err))
            }
            None => (None, None),
        };
        eprintln!(
            "{:<10} {:<12} {:>16} {:>16} {:>12} {:>10} {:>7.1}s",
            trial.envelope,
            trial.label,
            format!("({:.1},{:.1})", trial.true_center.0, trial.true_center.1),
            returned_text,
            abs_err.map(|v| format!("{v:.1}")).unwrap_or("-".into()),
            norm_err.map(|v| format!("{v:.2}")).unwrap_or("-".into()),
            trial.elapsed.as_secs_f64(),
        );
    }
    let valid_count = trials.iter().filter(|t| t.valid_structured_output).count();
    eprintln!(
        "\nvalid structured-output rate: {}/{}",
        valid_count,
        trials.len()
    );
    for envelope in trials
        .iter()
        .map(|t| t.envelope)
        .collect::<std::collections::BTreeSet<_>>()
    {
        let errors: Vec<f64> = trials
            .iter()
            .filter(|t| t.envelope == envelope)
            .filter_map(|t| {
                t.returned.map(|(x, y)| {
                    let dx = x - t.true_center.0;
                    let dy = y - t.true_center.1;
                    (dx * dx + dy * dy).sqrt()
                })
            })
            .collect();
        if !errors.is_empty() {
            let mean = errors.iter().sum::<f64>() / errors.len() as f64;
            eprintln!(
                "{envelope}: mean abs error = {mean:.1}px over {} samples",
                errors.len()
            );
        }
    }
}

/// Real qwen3-vl-local point-selection precision at the qualified 320x224
/// envelope, across four target positions with known ground truth, through
/// the exact production `CaptchaProvider::solve` seam — re-run after
/// `SCORPION_QWEN3_VL_CANDLE_REFERENCE_PARITY_ROOT_CAUSE_001`'s MRoPE and
/// prefill-causal-mask fixes. The prior frontier's own audit already
/// established resolution scaling does not affect precision (error grew
/// proportionally with canvas size for the same degenerate answer), so this
/// no longer needs the widened multi-envelope allowlist that investigation
/// used — only the existing qualified envelope is exercised.
#[tokio::test]
#[ignore = "requires pinned Qwen3-VL artifacts and a qualified CPU/F32 host; not part of the regression gate list"]
async fn point_precision_at_qualified_envelope() {
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

    let envelopes: [(&str, u32, u32); 1] = [("320x224", 320, 224)];

    let mut trials = Vec::new();
    for (label, w, h) in envelopes {
        for fixture in [
            corner_fixture(w, h),
            edge_fixture(w, h),
            center_fixture(w, h),
            distractor_fixture(w, h),
        ] {
            let trial = run_trial(&provider, label, &fixture).await;
            eprintln!(
                "[progress] {label}/{} -> {:?} in {:.1}s",
                trial.label,
                trial.returned,
                trial.elapsed.as_secs_f64()
            );
            trials.push(trial);
        }
    }

    report(&trials);
    provider.unload();
}

/// Real qwen3-vl-local horizontal-offset precision, with known ground
/// truth, through the exact production `CaptchaProvider::solve` seam —
/// there was no existing real-model coverage for this challenge kind at
/// all prior to this frontier's fix.
#[tokio::test]
#[ignore = "requires pinned Qwen3-VL artifacts and a qualified CPU/F32 host; not part of the regression gate list"]
async fn horizontal_offset_at_qualified_envelope() {
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

    let (width, height) = (320u32, 224u32);
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
        correlation_id: "precision-audit-horizontal-offset".into(),
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
    eprintln!(
        "[progress] horizontal_offset true={true_offset} outcome={outcome:?} in {:.1}s",
        start.elapsed().as_secs_f64()
    );
    provider.unload();
}
