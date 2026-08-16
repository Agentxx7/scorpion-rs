#![cfg(feature = "local_paligemma_cuda")]

//! Real production-path qualification for the newly integrated
//! `google/paligemma-3b-mix-448` CUDA/F16 checkpoint variant —
//! `SCORPION_PALIGEMMA_448_PRODUCTION_RUNTIME_INTEGRATION_001`.
//!
//! Every test here goes through the real production
//! `CaptchaProvider::solve`/`solve_captcha` seam via
//! `PaligemmaLocalCaptchaProvider::initialize_448_cuda_f16_from_host` and
//! `paligemma_448_cuda_f16_manifest()` — never a standalone diagnostic
//! reimplementation of the runtime's internals. The point-selection and
//! live-UI fixture matrices are genuine reproductions of the already-closed
//! 224 CUDA qualifications
//! (`paligemma_cuda_point_precision_audit.rs`/`paligemma_live_ui_localization_qualification.rs`)
//! — same fixtures, same thresholds, only the constructed provider/manifest
//! differ, so a pass here is a genuine reference-parity check that the 448
//! envelope does not trade accuracy for its larger image token budget.
//!
//! Ignored in ordinary CI: requires the pinned ~11.7 GB
//! `google/paligemma-3b-mix-448` installation, a qualified CUDA/F16 host,
//! the frozen genuine Turnstile raster fixture, and (for the matched
//! browser fixture only) a working local `chromium` binary.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use image::{ImageBuffer, Rgb};
use sha2::{Digest, Sha256};
use spider::features::captcha::{
    CaptchaChallenge, CaptchaChallengeKind, CaptchaImageGridCell, CaptchaImageGridInput,
    CaptchaProvider, CaptchaProviderId, CaptchaSolution, CaptchaSolveOutcome, CaptchaSolveRequest,
    CaptchaVisualInput,
};
use spider::features::paligemma_captcha::PaligemmaLocalCaptchaProvider;
use spider::features::paligemma_runtime::paligemma_448_cuda_f16_manifest;

const CANVAS: (u32, u32) = (224, 224);

// ---------------------------------------------------------------------
// Shared installation helper
// ---------------------------------------------------------------------

/// Returns the activated installation plus its owning `TempDir` guard — the
/// caller must keep the guard alive for as long as the installation is
/// used. Staging is created as a sibling of the real pinned artifact
/// directory (not a fresh `tempfile::tempdir()`, which usually lands on a
/// separate `tmpfs` mount) so `LocalModelManifest::activate`'s hard-link +
/// same-filesystem atomic rename succeeds.
fn activate_448_installation() -> (
    spider::features::local_model::LocalModelInstallation,
    tempfile::TempDir,
) {
    let source = PathBuf::from(
        std::env::var("SCORPION_PALIGEMMA_448_PINNED_ARTIFACTS")
            .expect("set the pinned google/paligemma-3b-mix-448 offline artifact directory"),
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
    let installation = paligemma_448_cuda_f16_manifest()
        .activate(&staging, &active)
        .unwrap();
    (installation, parent)
}

fn peak_rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            let kb: u64 = rest.trim().trim_end_matches(" kB").trim().parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

// ---------------------------------------------------------------------
// Point-precision matrix — genuine reproduction of
// `paligemma_cuda_point_precision_audit.rs`'s own required 8-fixture matrix.
// ---------------------------------------------------------------------

mod precision {
    use super::*;

    const BACKGROUND: Rgb<u8> = Rgb([40, 40, 40]);
    const TARGET_COLOR: Rgb<u8> = Rgb([220, 40, 40]);
    const DISTRACTOR_COLOR: Rgb<u8> = Rgb([40, 90, 220]);
    const STANDARD_SIDE: u32 = 44;
    const SMALL_SIDE: u32 = 16;

    pub struct Fixture {
        pub label: &'static str,
        pub png: Vec<u8>,
        pub true_center: (f64, f64),
        pub side: u32,
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

    pub fn required_matrix() -> Vec<Fixture> {
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

    pub const RELIABLE_CONTAINMENT_THRESHOLD: usize = 6;
    pub const STANDARD_FIXTURE_COUNT: usize = 7;
    pub const INSTRUCTION: &str = "red square";
}

// ---------------------------------------------------------------------
// Live-UI matrix — genuine reproduction of
// `paligemma_live_ui_localization_qualification.rs`'s own frozen 8-fixture
// matrix.
// ---------------------------------------------------------------------

mod live_ui {
    use super::*;

    const BACKGROUND: Rgb<u8> = Rgb([45, 48, 52]);
    const OUTLINE: Rgb<u8> = Rgb([205, 210, 215]);
    const LOW_CONTRAST_OUTLINE: Rgb<u8> = Rgb([66, 69, 75]);
    const TEXT_COLOR: Rgb<u8> = Rgb([190, 195, 200]);
    const LOGO_ACCENT_A: Rgb<u8> = Rgb([90, 200, 190]);
    const LOGO_ACCENT_B: Rgb<u8> = Rgb([150, 110, 210]);
    const DISTRACTOR_OUTLINE: Rgb<u8> = Rgb([140, 145, 150]);
    const TARGET_OUTLINE: Rgb<u8> = Rgb([90, 150, 230]);
    const STROKE: u32 = 2;

    pub struct Fixture {
        pub label: &'static str,
        pub png: Vec<u8>,
        pub instruction: &'static str,
        pub true_center: (f64, f64),
        pub size: u32,
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

    pub fn required_matrix() -> Vec<Fixture> {
        let mut fixtures = Vec::new();

        let mut c = canvas();
        let center = (50u32, 50);
        outlined_square(&mut c, center, 22, OUTLINE);
        fixtures.push(Fixture {
            label: "plain_outlined_square",
            png: encode_png(c),
            instruction: "the outlined square",
            true_center: (f64::from(center.0), f64::from(center.1)),
            size: 22,
        });

        let mut c = canvas();
        let center = (170u32, 55);
        outlined_circle(&mut c, center, 22, OUTLINE);
        fixtures.push(Fixture {
            label: "outlined_circle",
            png: encode_png(c),
            instruction: "the outlined circle",
            true_center: (f64::from(center.0), f64::from(center.1)),
            size: 22,
        });

        let mut c = canvas();
        let center = (55u32, 125);
        outlined_square(&mut c, center, 22, OUTLINE);
        text_like_label(&mut c, (75, 118), TEXT_COLOR);
        fixtures.push(Fixture {
            label: "adjacent_to_text",
            png: encode_png(c),
            instruction: "the outlined square",
            true_center: (f64::from(center.0), f64::from(center.1)),
            size: 22,
        });

        let mut c = canvas();
        let center = (160u32, 165);
        outlined_square(&mut c, center, 22, OUTLINE);
        logo_like_distractor(&mut c, (195, 160));
        fixtures.push(Fixture {
            label: "adjacent_to_logo_distractor",
            png: encode_png(c),
            instruction: "the outlined square",
            true_center: (f64::from(center.0), f64::from(center.1)),
            size: 22,
        });

        let mut c = canvas();
        let target = (110u32, 190);
        outlined_square(&mut c, (55, 195), 22, DISTRACTOR_OUTLINE);
        outlined_square(&mut c, (170, 185), 22, DISTRACTOR_OUTLINE);
        outlined_square(&mut c, target, 22, TARGET_OUTLINE);
        fixtures.push(Fixture {
            label: "multiple_similar_controls",
            png: encode_png(c),
            instruction: "the outlined square with the blue border",
            true_center: (f64::from(target.0), f64::from(target.1)),
            size: 22,
        });

        let mut c = canvas();
        let center = (140u32, 40);
        outlined_square(&mut c, center, 22, LOW_CONTRAST_OUTLINE);
        fixtures.push(Fixture {
            label: "low_contrast",
            png: encode_png(c),
            instruction: "the outlined square",
            true_center: (f64::from(center.0), f64::from(center.1)),
            size: 22,
        });

        let mut c = canvas();
        let center = (15u32, 15);
        outlined_square(&mut c, center, 20, OUTLINE);
        fixtures.push(Fixture {
            label: "near_edge",
            png: encode_png(c),
            instruction: "the outlined square",
            true_center: (f64::from(center.0), f64::from(center.1)),
            size: 20,
        });

        let mut c = canvas();
        let center = (95u32, 95);
        outlined_square(&mut c, center, 16, OUTLINE);
        fixtures.push(Fixture {
            label: "smallest_stress",
            png: encode_png(c),
            instruction: "the outlined square",
            true_center: (f64::from(center.0), f64::from(center.1)),
            size: 16,
        });

        fixtures
    }

    pub const RELIABLE_CONTAINMENT_THRESHOLD: usize = 7;
    pub const STANDARD_FIXTURE_COUNT: usize = 8;
}

// ---------------------------------------------------------------------
// Shared trial machinery
// ---------------------------------------------------------------------

struct Trial {
    label: String,
    true_center: (f64, f64),
    side: u32,
    returned: Option<(f64, f64)>,
    elapsed: Duration,
}

async fn run_point_trial(
    provider: &PaligemmaLocalCaptchaProvider,
    label: &str,
    instruction: &str,
    png: Vec<u8>,
    true_center: (f64, f64),
    side: u32,
) -> Trial {
    let request = CaptchaSolveRequest {
        correlation_id: format!("paligemma-448-cuda-{label}"),
        selected_provider: CaptchaProviderId::PALIGEMMA_LOCAL,
        challenge: CaptchaChallenge {
            kind: CaptchaChallengeKind::PointSelection,
            instruction: instruction.to_string(),
            visuals: vec![CaptchaVisualInput::materialized(None, "image/png", png)],
        },
        deadline: Duration::from_secs(1_800),
    };
    let start = Instant::now();
    let outcome = provider.solve(&request).await;
    let elapsed = start.elapsed();
    let returned = match outcome {
        CaptchaSolveOutcome::Solved {
            solution: CaptchaSolution::Point { x, y },
            ..
        } => Some((x, y)),
        _ => None,
    };
    Trial {
        label: label.to_string(),
        true_center,
        side,
        returned,
        elapsed,
    }
}

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
    eprintln!(
        "{:<28} {:>14} {:>14} {:>10} {:>8}",
        "target", "true_center", "returned", "contained", "secs"
    );
    for trial in trials {
        let returned_text = match trial.returned {
            Some((x, y)) => format!("({x:.2},{y:.2})"),
            None => "-".to_string(),
        };
        eprintln!(
            "{:<28} {:>14} {:>14} {:>10} {:>8.3}",
            trial.label,
            format!("({:.1},{:.1})", trial.true_center.0, trial.true_center.1),
            returned_text,
            contained(trial),
            trial.elapsed.as_secs_f64(),
        );
    }
}

/// Frozen production latency budget — 50% of the measured minimum genuine
/// Turnstile challenge lifetime (110.06s). Every real inference in this
/// suite must clear it with wide margin.
const LATENCY_BUDGET_SECS: f64 = 55.03;

/// Real accelerated `paligemma-local` (448 CUDA/F16) point-selection
/// precision, across the exact same required position matrix as the
/// already-closed 224 CUDA qualification, through the real production
/// `CaptchaProvider::solve` seam.
#[tokio::test]
#[ignore = "requires the pinned ~11.7 GB google/paligemma-3b-mix-448 installation and a qualified CUDA/F16 host"]
async fn real_448_point_selection_precision_matrix() {
    let (installation, _parent) = activate_448_installation();
    let provider =
        PaligemmaLocalCaptchaProvider::initialize_448_cuda_f16_from_host(&installation).unwrap();

    let mut trials = Vec::new();
    for fixture in precision::required_matrix() {
        let trial = run_point_trial(
            &provider,
            fixture.label,
            precision::INSTRUCTION,
            fixture.png,
            fixture.true_center,
            fixture.side,
        )
        .await;
        assert!(
            trial.elapsed.as_secs_f64() < LATENCY_BUDGET_SECS,
            "{} exceeded the frozen latency budget: {:.3}s",
            trial.label,
            trial.elapsed.as_secs_f64()
        );
        trials.push(trial);
    }
    report(&trials);

    let standard: Vec<&Trial> = trials
        .iter()
        .filter(|t| t.side == 44 && t.label != "distractor")
        .chain(trials.iter().filter(|t| t.label == "distractor"))
        .collect();
    let standard_count = trials.iter().filter(|t| t.side == 44).count();
    assert_eq!(standard_count, precision::STANDARD_FIXTURE_COUNT);
    let contained_count = trials
        .iter()
        .filter(|t| t.side == 44 && contained(t))
        .count();
    eprintln!(
        "\n448 standard-size (44px) actionable containment: {contained_count}/{} \
         (reliable-single-shot threshold: {}/{})",
        precision::STANDARD_FIXTURE_COUNT,
        precision::RELIABLE_CONTAINMENT_THRESHOLD,
        precision::STANDARD_FIXTURE_COUNT
    );
    assert!(
        contained_count >= precision::RELIABLE_CONTAINMENT_THRESHOLD,
        "448 envelope must clear the reliable-single-shot threshold"
    );
    let _ = standard;
    provider.unload();
}

/// Real accelerated 448 CUDA/F16 live-UI outlined-control point-selection
/// localization, across the exact same frozen 8-fixture matrix as the
/// already-closed 224 CUDA qualification.
#[tokio::test]
#[ignore = "requires the pinned ~11.7 GB google/paligemma-3b-mix-448 installation and a qualified CUDA/F16 host"]
async fn real_448_live_ui_point_selection_matrix() {
    let (installation, _parent) = activate_448_installation();
    let provider =
        PaligemmaLocalCaptchaProvider::initialize_448_cuda_f16_from_host(&installation).unwrap();

    let mut trials = Vec::new();
    for fixture in live_ui::required_matrix() {
        let trial = run_point_trial(
            &provider,
            fixture.label,
            fixture.instruction,
            fixture.png,
            fixture.true_center,
            fixture.size,
        )
        .await;
        assert!(
            trial.elapsed.as_secs_f64() < LATENCY_BUDGET_SECS,
            "{} exceeded the frozen latency budget: {:.3}s",
            trial.label,
            trial.elapsed.as_secs_f64()
        );
        trials.push(trial);
    }
    report(&trials);

    assert_eq!(trials.len(), live_ui::STANDARD_FIXTURE_COUNT);
    let contained_count = trials.iter().filter(|t| contained(t)).count();
    eprintln!(
        "\n448 live-UI actionable containment: {contained_count}/{} \
         (reliable threshold: {}/{})",
        live_ui::STANDARD_FIXTURE_COUNT,
        live_ui::RELIABLE_CONTAINMENT_THRESHOLD,
        live_ui::STANDARD_FIXTURE_COUNT
    );
    assert!(
        contained_count >= live_ui::RELIABLE_CONTAINMENT_THRESHOLD,
        "448 envelope must clear the reliable live-UI threshold"
    );
    provider.unload();
}

// ---------------------------------------------------------------------
// Frozen genuine Turnstile raster gate — the mandatory primary gate this
// candidate was qualified against before any production integration.
// ---------------------------------------------------------------------

/// Prefix of the frozen genuine raster's real SHA-256 (established during
/// `SCORPION_CAPTCHA_REAL_WORLD_GROUNDING_MODEL_CANDIDATE_QUALIFICATION_001`).
/// Guards against silently running this gate against the wrong file.
const FROZEN_RASTER_SHA256_PREFIX: &str = "3398f533";
/// Frozen actionable region (original 224x224 raster pixel space).
const REGION_X: (f64, f64) = (9.0, 32.0);
const REGION_Y: (f64, f64) = (21.0, 44.0);

/// Real production-path reproduction of the mandatory real-raster gate:
/// the exact genuine captured Turnstile raster, the closed canonical
/// short-label contract (`"the outlined square"`), no target-aware crop,
/// enlargement, coordinate hint, or manual enhancement — deterministic
/// greedy decode, required 2/2 containment.
#[tokio::test]
#[ignore = "requires the pinned ~11.7 GB google/paligemma-3b-mix-448 installation, a qualified CUDA/F16 host, and the frozen genuine Turnstile raster fixture"]
async fn real_448_frozen_genuine_raster_gate() {
    let raster_path = PathBuf::from(
        std::env::var("SCORPION_PALIGEMMA_REAL_TURNSTILE_RASTER")
            .expect("set the path to the frozen genuine captured Turnstile raster"),
    );
    let png = std::fs::read(&raster_path).unwrap();
    let observed_sha256 = format!("{:x}", Sha256::digest(&png));
    assert!(
        observed_sha256.starts_with(FROZEN_RASTER_SHA256_PREFIX),
        "the raster at SCORPION_PALIGEMMA_REAL_TURNSTILE_RASTER does not match the frozen \
         genuine capture (got {observed_sha256}, expected prefix {FROZEN_RASTER_SHA256_PREFIX})"
    );

    let (installation, _parent) = activate_448_installation();
    let provider =
        PaligemmaLocalCaptchaProvider::initialize_448_cuda_f16_from_host(&installation).unwrap();

    let mut points = Vec::new();
    for attempt in 0..2 {
        let request = CaptchaSolveRequest {
            correlation_id: format!("paligemma-448-real-raster-gate-{attempt}"),
            selected_provider: CaptchaProviderId::PALIGEMMA_LOCAL,
            challenge: CaptchaChallenge {
                kind: CaptchaChallengeKind::PointSelection,
                instruction: "the outlined square".into(),
                visuals: vec![CaptchaVisualInput::materialized(
                    None,
                    "image/png",
                    png.clone(),
                )],
            },
            deadline: Duration::from_secs(1_800),
        };
        let start = Instant::now();
        let outcome = provider.solve(&request).await;
        let elapsed = start.elapsed();
        let point = match outcome {
            CaptchaSolveOutcome::Solved {
                solution: CaptchaSolution::Point { x, y },
                ..
            } => (x, y),
            other => panic!("attempt {attempt}: expected a solved point, got {other:?}"),
        };
        eprintln!(
            "[real-raster-gate attempt {attempt}] point=({:.3},{:.3}) elapsed={:.3}s",
            point.0,
            point.1,
            elapsed.as_secs_f64()
        );
        assert!(
            elapsed.as_secs_f64() < LATENCY_BUDGET_SECS,
            "attempt {attempt} exceeded the frozen latency budget: {:.3}s",
            elapsed.as_secs_f64()
        );
        let inside_region = (REGION_X.0..=REGION_X.1).contains(&point.0)
            && (REGION_Y.0..=REGION_Y.1).contains(&point.1);
        assert!(
            inside_region,
            "attempt {attempt}: point ({:.3},{:.3}) is outside the frozen actionable region \
             x={REGION_X:?} y={REGION_Y:?} — near-miss is FAIL",
            point.0, point.1
        );
        points.push(point);
    }
    assert_eq!(
        points[0], points[1],
        "deterministic greedy decoding must reproduce the identical point across both attempts"
    );

    provider.unload();
}

// ---------------------------------------------------------------------
// Matched real-Chromium fixture — a genuine headless-Chrome-rendered
// raster (not a synthetic `image::ImageBuffer` drawing), with a
// known-by-construction true target position. Uses only the standard
// chrome screenshot capability already exercised elsewhere in this
// workspace; it does not touch the CAPTCHA browser-binding stash.
// ---------------------------------------------------------------------

#[cfg(feature = "chrome")]
mod matched_browser {
    use spider::chromiumoxide::browser::{Browser, BrowserConfig, HeadlessMode};
    use spider::chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
    use spider::chromiumoxide::page::ScreenshotParams;
    use spider::tokio_stream::StreamExt;

    /// True center of the rendered outlined control, by construction of the
    /// HTML below — not measured or assumed.
    pub const TRUE_CENTER: (f64, f64) = (60.0, 60.0);
    pub const TRUE_SIDE: f64 = 24.0;

    async fn serve(listener: tokio::net::TcpListener, body: &'static str) {
        let (mut stream, _) = listener.accept().await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut request = [0_u8; 2048];
        let _ = stream.read(&mut request).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    }

    /// Capture a real headless-Chrome-rendered 224x224 raster of a plain
    /// outlined square button, centered at `TRUE_CENTER`, on a neutral
    /// background — the same "general capability" convention already used
    /// by the live-UI qualification (no Turnstile/Cloudflare-specific
    /// wording, layout, or branding).
    pub async fn capture() -> Vec<u8> {
        let html = "<html><body style=\"margin:0;width:224px;height:224px;\
             background:#2d3034;\"><div style=\"position:absolute;left:48px;top:48px;\
             width:24px;height:24px;box-sizing:border-box;border:2px solid #cdd2d7;\
             background:transparent;\"></div></body></html>";
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(serve(listener, html));

        let profile = std::env::temp_dir().join(format!(
            "paligemma-448-matched-browser-{}",
            std::process::id()
        ));
        let config = BrowserConfig::builder()
            .user_data_dir(profile)
            .chrome_executable("/usr/bin/chromium")
            .headless_mode(HeadlessMode::True)
            .window_size(224, 224)
            .arg("--no-sandbox")
            .build()
            .unwrap();
        let (browser, mut handler) = Browser::launch(config).await.unwrap();
        let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });
        let page = browser
            .new_page(format!("http://127.0.0.1:{port}/"))
            .await
            .unwrap();
        page.wait_for_navigation().await.unwrap();
        let png = page
            .screenshot(
                ScreenshotParams::builder()
                    .format(CaptureScreenshotFormat::Png)
                    .build(),
            )
            .await
            .unwrap();
        drop(page);
        drop(browser);
        handler_task.abort();
        server.await.unwrap();
        png
    }
}

#[cfg(feature = "chrome")]
#[tokio::test]
#[ignore = "requires the pinned ~11.7 GB google/paligemma-3b-mix-448 installation, a qualified CUDA/F16 host, and a local chromium binary"]
async fn real_448_matched_chromium_fixture() {
    let png = matched_browser::capture().await;

    let (installation, _parent) = activate_448_installation();
    let provider =
        PaligemmaLocalCaptchaProvider::initialize_448_cuda_f16_from_host(&installation).unwrap();

    let trial = run_point_trial(
        &provider,
        "matched_chromium_fixture",
        "the outlined square",
        png,
        matched_browser::TRUE_CENTER,
        matched_browser::TRUE_SIDE as u32,
    )
    .await;
    report(std::slice::from_ref(&trial));
    assert!(
        trial.elapsed.as_secs_f64() < LATENCY_BUDGET_SECS,
        "matched Chromium fixture exceeded the frozen latency budget: {:.3}s",
        trial.elapsed.as_secs_f64()
    );
    assert!(
        contained(&trial),
        "real Chromium-rendered fixture must fall inside its own true actionable region"
    );
    provider.unload();
}

// ---------------------------------------------------------------------
// ImageGridSelection + HorizontalOffset production-path compatibility, and
// truthful/distinguishing provenance — through the real registry +
// `solve_captcha`, not `provider.solve` directly, so capability
// advertisement and dispatch validation are exercised too.
// ---------------------------------------------------------------------

/// The comprehensive real-hardware production-path run for this frontier:
/// one loaded provider instance (avoiding the installation's own mandatory
/// double SHA-256 re-verification tax per extra process spin-up) exercises
/// the point-precision matrix, the live-UI matrix, ImageGridSelection,
/// HorizontalOffset, the provider registry, and truthful/distinguishing
/// provenance — plus real peak RSS and real peak VRAM (via
/// `cudarc::driver::result::mem_get_info`, the same raw driver call the
/// runtime's own preflight already depends on) captured immediately after
/// load and again after the full battery. The frozen genuine-raster gate
/// and the matched-Chromium fixture are intentionally kept as separate,
/// independently runnable tests above (the gate in particular must stand
/// alone as the primary go/no-go check).
#[tokio::test]
#[ignore = "requires the pinned ~11.7 GB google/paligemma-3b-mix-448 installation and a qualified CUDA/F16 host"]
async fn real_448_full_production_qualification_battery() {
    use spider::features::captcha::{
        solve_captcha, CaptchaCapabilityQualification, CaptchaProviderRegistry,
    };
    use spider::features::paligemma_runtime::{
        PALIGEMMA_448_CUDA_PROCESSOR_ID, PALIGEMMA_448_CUDA_RUNTIME_IDENTITY,
        PALIGEMMA_448_MODEL_REVISION,
    };

    let (installation, _parent) = activate_448_installation();
    let peak_rss_before = peak_rss_bytes();
    let provider =
        PaligemmaLocalCaptchaProvider::initialize_448_cuda_f16_from_host(&installation).unwrap();
    // Real driver-level VRAM reading, same call the runtime's own preflight
    // already performs — taken on this same OS thread (the default
    // single-threaded `#[tokio::test]` runtime never migrates the task, so
    // the CUDA context `Device::new_cuda` bound during construction is
    // still current here).
    let (vram_free_after_load, vram_total) = cudarc::driver::result::mem_get_info().unwrap();
    let vram_used_after_load = vram_total - vram_free_after_load;
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

    // Point-selection precision matrix — same 8 fixtures, same thresholds
    // as the already-closed 224 CUDA qualification.
    let mut precision_trials = Vec::new();
    for fixture in precision::required_matrix() {
        let trial = run_point_trial(
            &provider,
            fixture.label,
            precision::INSTRUCTION,
            fixture.png,
            fixture.true_center,
            fixture.side,
        )
        .await;
        assert!(trial.elapsed.as_secs_f64() < LATENCY_BUDGET_SECS);
        precision_trials.push(trial);
    }
    report(&precision_trials);
    let precision_standard_count = precision_trials.iter().filter(|t| t.side == 44).count();
    assert_eq!(precision_standard_count, precision::STANDARD_FIXTURE_COUNT);
    let precision_contained = precision_trials
        .iter()
        .filter(|t| t.side == 44 && contained(t))
        .count();
    eprintln!(
        "\n448 standard-size (44px) actionable containment: {precision_contained}/{} \
         (reliable-single-shot threshold: {}/{})",
        precision::STANDARD_FIXTURE_COUNT,
        precision::RELIABLE_CONTAINMENT_THRESHOLD,
        precision::STANDARD_FIXTURE_COUNT
    );
    assert!(precision_contained >= precision::RELIABLE_CONTAINMENT_THRESHOLD);

    // Live-UI outlined-control matrix — same 8 fixtures as the
    // already-closed 224 CUDA qualification.
    let mut live_ui_trials = Vec::new();
    for fixture in live_ui::required_matrix() {
        let trial = run_point_trial(
            &provider,
            fixture.label,
            fixture.instruction,
            fixture.png,
            fixture.true_center,
            fixture.size,
        )
        .await;
        assert!(trial.elapsed.as_secs_f64() < LATENCY_BUDGET_SECS);
        live_ui_trials.push(trial);
    }
    report(&live_ui_trials);
    assert_eq!(live_ui_trials.len(), live_ui::STANDARD_FIXTURE_COUNT);
    let live_ui_contained = live_ui_trials.iter().filter(|t| contained(t)).count();
    eprintln!(
        "\n448 live-UI actionable containment: {live_ui_contained}/{} \
         (reliable threshold: {}/{})",
        live_ui::STANDARD_FIXTURE_COUNT,
        live_ui::RELIABLE_CONTAINMENT_THRESHOLD,
        live_ui::STANDARD_FIXTURE_COUNT
    );
    assert!(live_ui_contained >= live_ui::RELIABLE_CONTAINMENT_THRESHOLD);

    // ImageGridSelection, through the shared JSON structured contract.
    let mut grid_canvas = ImageBuffer::from_pixel(224u32, 224u32, Rgb([40u8, 40, 40]));
    for y in 90..134u32 {
        for x in 90..134u32 {
            grid_canvas.put_pixel(x, y, Rgb([220, 40, 40]));
        }
    }
    let mut grid_bytes = Vec::new();
    image::DynamicImage::ImageRgb8(grid_canvas)
        .write_to(
            &mut std::io::Cursor::new(&mut grid_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    let grid = CaptchaImageGridInput::new(
        CaptchaVisualInput::materialized(None, "image/png", grid_bytes),
        (224, 224),
        1,
        1,
        vec![CaptchaImageGridCell::new("cell-1", 0, 0, 0, 0, 224, 224)],
        false,
    )
    .unwrap();
    let grid_request = CaptchaSolveRequest {
        correlation_id: "real-448-provider-grid".into(),
        selected_provider: CaptchaProviderId::PALIGEMMA_LOCAL,
        challenge: CaptchaChallenge {
            kind: CaptchaChallengeKind::ImageGridSelection,
            instruction: "Select the only cell. Its stable ID is cell-1.".into(),
            visuals: vec![CaptchaVisualInput::materialized_full_grid(grid)],
        },
        deadline: Duration::from_secs(1_800),
    };
    let grid_start = Instant::now();
    match solve_captcha(&provider, &grid_request).await {
        CaptchaSolveOutcome::Solved {
            solution: CaptchaSolution::SelectedChoices(ids),
            provenance,
        } => {
            assert_eq!(ids, ["cell-1"]);
            let facts = provenance.local_runtime.unwrap();
            assert_eq!(facts.model_revision, PALIGEMMA_448_MODEL_REVISION);
            assert_eq!(facts.runtime_identity, PALIGEMMA_448_CUDA_RUNTIME_IDENTITY);
            assert_eq!(facts.processor_identity, PALIGEMMA_448_CUDA_PROCESSOR_ID);
        }
        other => panic!("real 448 provider did not produce a solved grid outcome: {other:?}"),
    }
    let grid_elapsed = grid_start.elapsed();
    assert!(grid_elapsed.as_secs_f64() < LATENCY_BUDGET_SECS);

    // HorizontalOffset, through the two-detect convention.
    let mut bar_canvas = ImageBuffer::from_pixel(224u32, 224u32, Rgb([40u8, 40, 40]));
    for y in 0..224u32 {
        for dx in 0..6u32 {
            bar_canvas.put_pixel((40 + dx).min(223), y, Rgb([220, 40, 40]));
            bar_canvas.put_pixel((150 + dx).min(223), y, Rgb([40, 90, 220]));
        }
    }
    let mut bar_bytes = Vec::new();
    image::DynamicImage::ImageRgb8(bar_canvas)
        .write_to(
            &mut std::io::Cursor::new(&mut bar_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    let offset_request = CaptchaSolveRequest {
        correlation_id: "real-448-provider-offset".into(),
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
    let offset_start = Instant::now();
    match solve_captcha(&provider, &offset_request).await {
        CaptchaSolveOutcome::Solved {
            solution: CaptchaSolution::HorizontalOffset(value),
            provenance,
        } => {
            assert!(value.is_finite());
            // True bar centers are 43 and 153: a genuine ~110px offset.
            assert!(
                (value - 110.0).abs() < 20.0,
                "offset {value} is not close to the true ~110px separation"
            );
            let facts = provenance.local_runtime.unwrap();
            assert_eq!(facts.model_revision, PALIGEMMA_448_MODEL_REVISION);
        }
        other => panic!("real 448 provider did not produce a solved offset outcome: {other:?}"),
    }
    let offset_elapsed = offset_start.elapsed();
    assert!(offset_elapsed.as_secs_f64() < LATENCY_BUDGET_SECS);

    let peak_rss_after = peak_rss_bytes();
    let (vram_free_after_battery, _) = cudarc::driver::result::mem_get_info().unwrap();
    let vram_used_after_battery = vram_total - vram_free_after_battery;
    eprintln!(
        "\n[resources] peak RSS before load: {:?}, after full battery: {:?}",
        peak_rss_before, peak_rss_after
    );
    eprintln!(
        "[resources] VRAM used after load: {vram_used_after_load} bytes ({:.2} GB); \
         after full battery: {vram_used_after_battery} bytes ({:.2} GB); device total: \
         {vram_total} bytes",
        vram_used_after_load as f64 / 1e9,
        vram_used_after_battery as f64 / 1e9,
    );

    provider.unload();
}
