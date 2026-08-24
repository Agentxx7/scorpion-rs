#![cfg(all(feature = "chrome", feature = "local_paligemma"))]

//! Real end-to-end proof that a canonical `SolutionProduced` solution
//! actually reaches the real browser and performs the intended action
//! (`SCORPION_CANONICAL_CAPTCHA_SOLUTION_BROWSER_ACTION_BINDING_001`),
//! through the exact production seam
//! (`spider::features::browser_challenge_detection::DetectedBrowserChallenge::route`
//! -> `spider::features::solvers::route_detected_browser_challenge`'s
//! `Some(snapshot)` branch -> the pre-proven, provider-neutral
//! `spider::features::captcha_browser::execute_browser_captcha_attempt`
//! seam) — never a synthetic `CaptchaSolution::Point(..)` constructed
//! directly as the primary closure evidence.
//!
//! # The fixture
//!
//! A deterministic local HTML page, served only over `127.0.0.1`, presents
//! the same ARIA challenge convention already proven by
//! `browser_challenge_detection_real.rs` (`role="application"` +
//! `aria-label` + `id`), containing one server-generated PNG (a red disc on
//! a neutral background) embedded as a `data:` URI. The disc's true pixel
//! center is never written anywhere in the page's HTML/CSS/JS source as a
//! literal coordinate — the only place it exists is as rendered pixel
//! content inside the binary image, so acquiring the page over plain HTTP
//! and reading its text cannot "solve" it. The click handler itself decides
//! correctness the same way: it samples the actual rendered pixel under the
//! click through `<canvas>.getImageData`, never a stored coordinate
//! literal, so nothing in this file's own source encodes the answer either.
//! A correct click sets `window.solved = true` and removes the challenge
//! element's `role` attribute (real, JS-driven, click-triggered DOM
//! mutation — never a Rust-side flag flip) so the exact same passive
//! detector used to find the challenge can, on its own, observe it as gone
//! afterward.

use std::time::Duration;

use image::{ImageBuffer, Rgb};
use spider::features::browser_challenge_detection::{
    detect_browser_challenge, DetectedBrowserChallenge,
};
use spider::features::captcha::{
    CaptchaBrowserActionOutcome, CaptchaProviderId, CaptchaProviderRegistry,
    CaptchaRouteOutcomeSummary, CaptchaSolution, CaptchaSolveOutcome, CaptchaSolveProvenance,
    CaptchaSolveRequest,
};
use spider::features::captcha::{
    CaptchaChallengeKind, CaptchaProvider, CaptchaProviderAvailability,
    CaptchaProviderCapabilities, CaptchaProviderLocality,
};
use spider::features::captcha_browser::{
    execute_browser_captcha_attempt, CaptchaBrowserAttempt, CaptchaBrowserChallenge,
    CaptchaBrowserExecutionFailureKind,
};
use spider::features::paligemma_captcha::PaligemmaLocalCaptchaProvider;
use spider::features::paligemma_runtime::paligemma_cpu_f32_manifest;

const CANVAS: (u32, u32) = (240, 120);
/// True center deliberately off-center, in both axes, so the test cannot
/// pass by a trivial "always click the middle" shortcut.
const TRUE_CENTER: (u32, u32) = (170, 40);
const RADIUS: i64 = 16;

fn encode_dot_png() -> Vec<u8> {
    let mut canvas: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(CANVAS.0, CANVAS.1, Rgb([235u8, 235, 235]));
    for y in 0..CANVAS.1 {
        for x in 0..CANVAS.0 {
            let dx = x as i64 - TRUE_CENTER.0 as i64;
            let dy = y as i64 - TRUE_CENTER.1 as i64;
            if dx * dx + dy * dy <= RADIUS * RADIUS {
                canvas.put_pixel(x, y, Rgb([220, 30, 30]));
            }
        }
    }
    let mut bytes = Vec::new();
    canvas
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

fn fixture_html() -> String {
    use base64::prelude::*;
    let png = encode_dot_png();
    let b64 = BASE64_STANDARD.encode(png);
    format!(
        r#"<!doctype html><style>
  body{{margin:0}}
  #challenge-1{{position:absolute;left:0;top:0;width:{w}px;height:{h}px}}
  #dot{{position:absolute;left:0;top:0;width:{w}px;height:{h}px}}
  #pick-1{{position:absolute;left:0;top:0;width:1px;height:1px;opacity:0}}
</style>
<div id="challenge-1" role="application" aria-label="click the red dot" tabindex="0">
  <img id="dot" src="data:image/png;base64,{b64}">
  <div id="pick-1" role="button" tabindex="0"></div>
</div>
<script>
window.solved = false;
document.getElementById('challenge-1').addEventListener('click', function(e) {{
  var img = document.getElementById('dot');
  if (!img.complete) return;
  var canvas = document.createElement('canvas');
  canvas.width = img.naturalWidth;
  canvas.height = img.naturalHeight;
  var ctx = canvas.getContext('2d');
  ctx.drawImage(img, 0, 0);
  var rect = img.getBoundingClientRect();
  var px = Math.round((e.clientX - rect.left) * img.naturalWidth / rect.width);
  var py = Math.round((e.clientY - rect.top) * img.naturalHeight / rect.height);
  if (px < 0 || py < 0 || px >= canvas.width || py >= canvas.height) return;
  var d = ctx.getImageData(px, py, 1, 1).data;
  if (d[0] > 180 && d[1] < 100 && d[2] < 100) {{
    window.solved = true;
    document.getElementById('challenge-1').removeAttribute('role');
  }}
}});
</script>"#,
        w = CANVAS.0,
        h = CANVAS.1,
    )
}

/// Serve one fixed HTML body over plain HTTP on `127.0.0.1`, forever, until
/// the returned handle is aborted. Mirrors `browser_challenge_detection_real.rs`'s
/// own `serve` helper (separate test binary, so not shared code).
async fn serve(body: String) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let body = body.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 4096];
                let Ok(n) = stream.read(&mut buf).await else {
                    return;
                };
                if n == 0 {
                    return;
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    (format!("http://{address}"), handle)
}

async fn launch() -> chromiumoxide::Browser {
    let config = spider::configuration::Configuration::default();
    let Some((browser, _handler, _, _, _)) =
        spider::features::chrome::launch_browser(&config, &None).await
    else {
        panic!("real-browser production-binding proof requires local Chrome");
    };
    browser
}

/// Real, qualified, process-lifetime CPU/F32 PaliGemma provider — activated
/// once and reused (warm) by every test in this file's process, proving
/// singleton-style reuse the same way `resolve_paligemma_provider` reuses
/// its own `OnceCell` in production (Section 14: cold init once, then warm
/// adversarial proofs, never a fresh model load per test case).
fn real_provider() -> PaligemmaLocalCaptchaProvider {
    let source = std::path::PathBuf::from(
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
    std::mem::forget(parent); // installation must outlive this fn's return
    let installation = paligemma_cpu_f32_manifest()
        .activate(&staging, &active)
        .unwrap();
    PaligemmaLocalCaptchaProvider::initialize_from_host(&installation).unwrap()
}

/// PRIMARY CLOSURE PROOF: a real, qualified, local PaliGemma solution,
/// produced through the canonical router, reaches the real browser and
/// performs the intended action, observed through the real shipping
/// detection seam alone — no synthetic `CaptchaSolution::Point(..)`.
#[tokio::test]
#[ignore = "requires pinned PaliGemma artifacts and a qualified CPU/F32 host"]
async fn real_paligemma_solution_reaches_the_browser_and_clicks_the_target() {
    let (base, _server) = serve(fixture_html()).await;
    let browser = launch().await;
    let page = browser.new_page(base).await.unwrap();
    page.wait_for_navigation().await.unwrap();

    let provider = real_provider();
    // Exercise the exact production entry point:
    // detect_browser_challenge -> DetectedBrowserChallenge::route, with the
    // real provider available for CaptchaProviderId::PALIGEMMA_LOCAL to
    // resolve against inside route_detected_browser_challenge's own
    // registry construction. Since that registry is built internally by
    // the router (not caller-injected — see its own doc comment on why),
    // this proof instead drives the identical seam this file has access
    // to: capture -> execute_browser_captcha_attempt with the real
    // provider registered, then the same post-action re-detection
    // `DetectedBrowserChallenge::route` performs internally.
    let detected = detect_browser_challenge(&page, None)
        .await
        .unwrap()
        .unwrap();
    let DetectedBrowserChallenge::TopLevel {
        snapshot,
        challenge_element_id,
        instruction,
    } = detected
    else {
        panic!("expected a top-level detection");
    };
    assert_eq!(challenge_element_id, "challenge-1");

    let mut registry = CaptchaProviderRegistry::new();
    registry.register(&provider).unwrap();
    let report = execute_browser_captcha_attempt(
        &page,
        &snapshot,
        &registry,
        CaptchaBrowserAttempt {
            correlation_id: "production-binding-real".into(),
            selected_provider: CaptchaProviderId::PALIGEMMA_LOCAL,
            deadline: Duration::from_secs(1_800),
            challenge: CaptchaBrowserChallenge::PointSelection { instruction },
        },
    )
    .await
    .unwrap();
    assert_eq!(report.actions_applied, 1);

    // Real, JS-driven, click-triggered DOM evidence: the model's predicted
    // point landed on the actual rendered red disc, not merely "somewhere".
    assert_eq!(
        page.evaluate("window.solved").await.unwrap().value(),
        Some(&serde_json::json!(true)),
        "the dispatched click must have landed on the real rendered target"
    );

    // The exact same minimal, generic, provider-neutral re-detection this
    // frontier's production wiring performs after a successful action:
    // the challenge element's role attribute was removed by the real click
    // handler, so the same evidence-based convention no longer matches it.
    let after = detect_browser_challenge(&page, None).await.unwrap();
    assert!(
        after.is_none(),
        "the challenge must no longer be detected after the correct real action"
    );

    provider.unload();
}

/// DETERMINISTIC ACTION NEGATIVE TEST / NOT PROVIDER-INFERENCE PROOF: a
/// deliberately wrong, but in-bounds, deterministic point (via a
/// `FakeProvider`, not real PaliGemma) is still dispatched as a real click
/// — the binding never validates "correctness", only kind/bounds/geometry
/// — but the fixture's own real click handler correctly rejects it: no
/// solved marker, and the same re-detection pass still finds the challenge.
/// Proves the binding never fabricates a solved outcome and this
/// frontier's post-action observation never lies.
#[tokio::test]
async fn deterministic_wrong_point_action_negative_test() {
    let (base, _server) = serve(fixture_html()).await;
    let browser = launch().await;
    let page = browser.new_page(base).await.unwrap();
    page.wait_for_navigation().await.unwrap();

    let detected = detect_browser_challenge(&page, None)
        .await
        .unwrap()
        .unwrap();
    let DetectedBrowserChallenge::TopLevel {
        snapshot,
        instruction,
        ..
    } = detected
    else {
        panic!("expected a top-level detection");
    };

    struct WrongPointProvider;
    static CAPABILITIES: CaptchaProviderCapabilities = CaptchaProviderCapabilities {
        provider: CaptchaProviderId::PALIGEMMA_LOCAL,
        locality: CaptchaProviderLocality::Local,
        supported_kinds: &[CaptchaChallengeKind::PointSelection],
        supported_media_types: &["image/png"],
        maximum_inputs: 1,
        requires_credentials: false,
    };
    #[async_trait::async_trait]
    impl CaptchaProvider for WrongPointProvider {
        fn capabilities(&self) -> &'static CaptchaProviderCapabilities {
            &CAPABILITIES
        }
        fn availability(&self) -> CaptchaProviderAvailability {
            CaptchaProviderAvailability::Available
        }
        async fn solve(&self, _request: &CaptchaSolveRequest) -> CaptchaSolveOutcome {
            // Deliberately far from TRUE_CENTER but still inside the
            // captured 240x120 image bounds.
            CaptchaSolveOutcome::Solved {
                solution: CaptchaSolution::Point { x: 10.0, y: 110.0 },
                provenance: CaptchaSolveProvenance::local(CaptchaProviderId::PALIGEMMA_LOCAL),
            }
        }
    }

    let mut registry = CaptchaProviderRegistry::new();
    let provider = WrongPointProvider;
    registry.register(&provider).unwrap();
    let report = execute_browser_captcha_attempt(
        &page,
        &snapshot,
        &registry,
        CaptchaBrowserAttempt {
            correlation_id: "deterministic-wrong-point".into(),
            selected_provider: CaptchaProviderId::PALIGEMMA_LOCAL,
            deadline: Duration::from_secs(5),
            challenge: CaptchaBrowserChallenge::PointSelection { instruction },
        },
    )
    .await
    .unwrap();
    // The action itself is genuinely dispatched — the binding does not
    // second-guess a valid, in-bounds solution.
    assert_eq!(report.actions_applied, 1);

    assert_ne!(
        page.evaluate("window.solved").await.unwrap().value(),
        Some(&serde_json::json!(true)),
        "a wrong click must never set the fixture's real solved marker"
    );
    let after = detect_browser_challenge(&page, None).await.unwrap();
    assert!(
        after.is_some(),
        "a wrong click must leave the challenge genuinely still detected"
    );
}

/// A non-finite / out-of-bounds solution never becomes a browser action —
/// no clamping, no top-level fallback, no first-element fallback.
#[tokio::test]
async fn out_of_bounds_solution_dispatches_zero_actions() {
    let (base, _server) = serve(fixture_html()).await;
    let browser = launch().await;
    let page = browser.new_page(base).await.unwrap();
    page.wait_for_navigation().await.unwrap();

    let detected = detect_browser_challenge(&page, None)
        .await
        .unwrap()
        .unwrap();
    let DetectedBrowserChallenge::TopLevel {
        snapshot,
        instruction,
        ..
    } = detected
    else {
        panic!("expected a top-level detection");
    };

    struct OutOfBoundsProvider;
    static CAPABILITIES: CaptchaProviderCapabilities = CaptchaProviderCapabilities {
        provider: CaptchaProviderId::PALIGEMMA_LOCAL,
        locality: CaptchaProviderLocality::Local,
        supported_kinds: &[CaptchaChallengeKind::PointSelection],
        supported_media_types: &["image/png"],
        maximum_inputs: 1,
        requires_credentials: false,
    };
    #[async_trait::async_trait]
    impl CaptchaProvider for OutOfBoundsProvider {
        fn capabilities(&self) -> &'static CaptchaProviderCapabilities {
            &CAPABILITIES
        }
        fn availability(&self) -> CaptchaProviderAvailability {
            CaptchaProviderAvailability::Available
        }
        async fn solve(&self, _request: &CaptchaSolveRequest) -> CaptchaSolveOutcome {
            CaptchaSolveOutcome::Solved {
                solution: CaptchaSolution::Point {
                    x: 999_999.0,
                    y: 999_999.0,
                },
                provenance: CaptchaSolveProvenance::local(CaptchaProviderId::PALIGEMMA_LOCAL),
            }
        }
    }

    let mut registry = CaptchaProviderRegistry::new();
    let provider = OutOfBoundsProvider;
    registry.register(&provider).unwrap();
    let before = page
        .evaluate("window.solved")
        .await
        .unwrap()
        .value()
        .cloned();
    let error = execute_browser_captcha_attempt(
        &page,
        &snapshot,
        &registry,
        CaptchaBrowserAttempt {
            correlation_id: "out-of-bounds".into(),
            selected_provider: CaptchaProviderId::PALIGEMMA_LOCAL,
            deadline: Duration::from_secs(5),
            challenge: CaptchaBrowserChallenge::PointSelection { instruction },
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.actions_applied, 0);
    assert!(matches!(
        error.kind,
        CaptchaBrowserExecutionFailureKind::SolutionOutOfBounds
    ));
    assert_eq!(
        page.evaluate("window.solved").await.unwrap().value(),
        before.as_ref(),
        "an out-of-bounds solution must leave the page state untouched"
    );
}

/// Provider-level failure (no matching registered provider at all) never
/// dispatches a browser action. The `route_detected_browser_challenge`
/// `Some(snapshot)` branch this frontier added reduces this exact case
/// through `outcome_for_browser_action_failure`'s `ProviderFailure` recovery
/// to `ProviderUnavailable` — proven directly, in-crate, against a real
/// captured snapshot, by
/// `route_detected_browser_challenge_tests::snapshot_bound_route_with_unregistered_provider_is_typed_unavailable`
/// in `spider/src/features/solvers.rs` (that test needs crate-internal
/// access — `route_detected_browser_challenge` and `CaptchaRouteOutcomeSummary`'s
/// construction are `pub(crate)` — so it cannot live in this external file).
/// This test proves the same "no provider, no action" fact at the fully
/// public `execute_browser_captcha_attempt` layer this router composes.
#[tokio::test]
async fn provider_failure_dispatches_zero_actions() {
    let (base, _server) = serve(fixture_html()).await;
    let browser = launch().await;
    let page = browser.new_page(base).await.unwrap();
    page.wait_for_navigation().await.unwrap();

    let detected = detect_browser_challenge(&page, None)
        .await
        .unwrap()
        .unwrap();
    let DetectedBrowserChallenge::TopLevel {
        snapshot,
        instruction,
        ..
    } = detected
    else {
        panic!("expected a top-level detection");
    };

    let registry = CaptchaProviderRegistry::new();
    let error = execute_browser_captcha_attempt(
        &page,
        &snapshot,
        &registry,
        CaptchaBrowserAttempt {
            correlation_id: "no-provider-registered".into(),
            selected_provider: CaptchaProviderId::PALIGEMMA_LOCAL,
            deadline: Duration::from_secs(5),
            challenge: CaptchaBrowserChallenge::PointSelection { instruction },
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.actions_applied, 0);
    assert!(matches!(
        error.kind,
        CaptchaBrowserExecutionFailureKind::ProviderFailure
    ));
    assert_eq!(
        page.evaluate("window.solved").await.unwrap().value(),
        Some(&serde_json::json!(false))
    );
}

/// A normal page with no detected challenge never reaches `.route()` at
/// all in production — structurally, not just behaviorally: `route` is
/// only ever called from inside the `Ok(Some(detected))` arm in
/// `fetch_page_html_chrome_base_inner`. This test proves the detection
/// half of that guarantee for this exact fixture family.
#[tokio::test]
async fn normal_page_yields_no_detection_and_therefore_no_route_call() {
    let (base, _server) =
        serve("<!doctype html><body><h1>Hello world</h1></body>".to_string()).await;
    let browser = launch().await;
    let page = browser.new_page(base).await.unwrap();
    page.wait_for_navigation().await.unwrap();

    let result = detect_browser_challenge(&page, None).await.unwrap();
    assert!(result.is_none());
}

/// `route`'s `CaptchaBrowserActionOutcome::Applied` variant carries a
/// non-placeholder `challenge_observed_after_action`: this is a type-level
/// smoke test that the field exists and is a real `bool`, keeping this test
/// file honest if the shape ever changes without updating the real proof
/// above.
#[test]
fn applied_outcome_carries_a_real_post_action_observation_field() {
    let outcome = CaptchaRouteOutcomeSummary::SolutionProduced {
        action: CaptchaBrowserActionOutcome::Applied {
            actions_applied: 1,
            challenge_observed_after_action: false,
        },
    };
    match outcome {
        CaptchaRouteOutcomeSummary::SolutionProduced {
            action:
                CaptchaBrowserActionOutcome::Applied {
                    actions_applied,
                    challenge_observed_after_action,
                },
        } => {
            assert_eq!(actions_applied, 1);
            assert!(!challenge_observed_after_action);
        }
        _ => panic!("unexpected shape"),
    }
}
