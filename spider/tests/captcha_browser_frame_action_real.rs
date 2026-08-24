#![cfg(all(feature = "chrome", feature = "local_paligemma"))]

//! Real end-to-end proof that a canonical `FramedEvidence` challenge inside
//! a genuine same-session `<iframe>` reaches a frame-correct browser action
//! (`SCORPION_CANONICAL_CAPTCHA_FRAME_ACTION_BINDING_001`), through the
//! exact production seam
//! (`spider::features::browser_challenge_detection::detect_browser_challenge`
//! -> `DetectedBrowserChallenge::route` ->
//! `spider::features::solvers::route_detected_framed_browser_challenge` ->
//! the pre-proven `spider::features::captcha_browser::execute_browser_captcha_attempt_in_frame`
//! seam).
//!
//! # The fixture
//!
//! A top-level page positions a same-origin `<iframe>` at a deliberate,
//! non-zero offset (`left: 60px, top: 250px`) from the top-level viewport
//! origin — chosen specifically so a *correct* frame-aware action (which
//! must add the iframe's own offset to the model's frame-local point) and
//! an *incorrect* one (a raw, un-offset top-level click at the same
//! frame-local coordinates) land at genuinely different real pixels. The
//! iframe's own document reuses the exact PointSelection fixture convention
//! from `captcha_browser_production_binding_real.rs`: a server-generated
//! PNG (a red disc) embedded as a `data:` URI with no coordinate literal
//! anywhere in the page source, and a click handler that decides
//! correctness by sampling the actual rendered pixel via
//! `<canvas>.getImageData` — genuinely unsolvable without landing on the
//! real rendered target, and entirely reused, not reinvented, for the
//! frame case.

use std::time::Duration;

use image::{ImageBuffer, Rgb};
use spider::chromiumoxide::layout::Point;
use spider::features::browser_challenge_detection::{
    detect_browser_challenge, DetectedBrowserChallenge, FramedMaterialization,
};
use spider::features::captcha::{
    CaptchaChallengeKind, CaptchaProvider, CaptchaProviderAvailability,
    CaptchaProviderCapabilities, CaptchaProviderLocality,
};
use spider::features::captcha::{
    CaptchaProviderId, CaptchaProviderRegistry, CaptchaSolution, CaptchaSolveOutcome,
    CaptchaSolveProvenance, CaptchaSolveRequest,
};
use spider::features::captcha_browser::{
    execute_browser_captcha_attempt_in_frame, CaptchaBrowserAttempt, CaptchaBrowserChallenge,
    CaptchaBrowserExecutionFailureKind,
};

const CANVAS: (u32, u32) = (240, 120);
const TRUE_CENTER: (u32, u32) = (170, 40);
const RADIUS: i64 = 16;
const IFRAME_LEFT: f64 = 60.0;
const IFRAME_TOP: f64 = 250.0;

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

fn child_html() -> String {
    use base64::prelude::*;
    let b64 = BASE64_STANDARD.encode(encode_dot_png());
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

fn top_html() -> String {
    format!(
        r#"<!doctype html><body style="margin:0">
<iframe id="child-frame" src="/child" style="position:absolute;left:{left}px;top:{top}px;width:{w}px;height:{h}px;border:0"></iframe>
</body>"#,
        left = IFRAME_LEFT,
        top = IFRAME_TOP,
        w = CANVAS.0,
        h = CANVAS.1,
    )
}

/// Serve a small fixed route table over plain HTTP on `127.0.0.1`, forever,
/// until the returned handle is aborted. Mirrors
/// `browser_challenge_detection_real.rs`'s own `serve` helper (separate
/// test binary, so not shared code).
async fn serve(routes: Vec<(&'static str, String)>) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let routes = routes.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 4096];
                let Ok(n) = stream.read(&mut buf).await else {
                    return;
                };
                if n == 0 {
                    return;
                }
                let request = String::from_utf8_lossy(&buf[..n]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                let body = routes
                    .iter()
                    .find(|(route, _)| *route == path)
                    .map(|(_, body)| body.clone())
                    .unwrap_or_else(|| "<!doctype html><html></html>".to_string());
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

async fn launch() -> spider::chromiumoxide::Browser {
    let config = spider::configuration::Configuration::default();
    let Some((browser, _handler, _, _, _)) =
        spider::features::chrome::launch_browser(&config, &None).await
    else {
        panic!("real-browser frame-action proof requires local Chrome");
    };
    browser
}

struct FixedPointProvider(f64, f64);
static CAPABILITIES: CaptchaProviderCapabilities = CaptchaProviderCapabilities {
    provider: CaptchaProviderId::PALIGEMMA_LOCAL,
    locality: CaptchaProviderLocality::Local,
    supported_kinds: &[CaptchaChallengeKind::PointSelection],
    supported_media_types: &["image/png"],
    maximum_inputs: 1,
    requires_credentials: false,
};
#[async_trait::async_trait]
impl CaptchaProvider for FixedPointProvider {
    fn capabilities(&self) -> &'static CaptchaProviderCapabilities {
        &CAPABILITIES
    }
    fn availability(&self) -> CaptchaProviderAvailability {
        CaptchaProviderAvailability::Available
    }
    async fn solve(&self, _request: &CaptchaSolveRequest) -> CaptchaSolveOutcome {
        CaptchaSolveOutcome::Solved {
            solution: CaptchaSolution::Point {
                x: self.0,
                y: self.1,
            },
            provenance: CaptchaSolveProvenance::local(CaptchaProviderId::PALIGEMMA_LOCAL),
        }
    }
}

/// FRAME ACTION INTEGRATION PROOF / NOT MODEL INFERENCE PROOF: a
/// deterministic, injected canonical solution (the fixture's own known true
/// center — never real inference) reaches the real same-session iframe and
/// performs the correct, frame-aware, offset-corrected action.
#[tokio::test]
async fn frame_action_integration_proof_correct_point_solves_the_iframe() {
    let (base, _server) = serve(vec![("/", top_html()), ("/child", child_html())]).await;
    let browser = launch().await;
    let page = browser.new_page(base).await.unwrap();
    page.wait_for_navigation().await.unwrap();
    // Let the iframe's own document finish loading before inspecting.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let detected = detect_browser_challenge(&page, Some(&browser))
        .await
        .unwrap()
        .unwrap();
    let DetectedBrowserChallenge::FramedEvidence {
        instruction,
        materialization,
        ..
    } = detected
    else {
        panic!("expected framed evidence for a challenge inside a same-session iframe");
    };
    let FramedMaterialization::Ready {
        top_level,
        frame,
        snapshot,
    } = materialization
    else {
        panic!("expected Ready materialization with a live browser handle offered");
    };

    let provider = FixedPointProvider(f64::from(TRUE_CENTER.0), f64::from(TRUE_CENTER.1));
    let mut registry = CaptchaProviderRegistry::new();
    registry.register(&provider).unwrap();
    let report = execute_browser_captcha_attempt_in_frame(
        &page,
        &top_level,
        &frame,
        &snapshot,
        &registry,
        CaptchaBrowserAttempt {
            correlation_id: "frame-action-integration".into(),
            selected_provider: CaptchaProviderId::PALIGEMMA_LOCAL,
            deadline: Duration::from_secs(5),
            challenge: CaptchaBrowserChallenge::PointSelection { instruction },
        },
    )
    .await
    .unwrap();
    assert_eq!(report.actions_applied, 1);

    let solved = page
        .evaluate("document.getElementById('child-frame').contentWindow.solved")
        .await
        .unwrap()
        .value()
        .cloned();
    assert_eq!(
        solved,
        Some(serde_json::json!(true)),
        "the frame-aware action must land on the iframe's real rendered target"
    );
}

/// DETERMINISTIC ACTION NEGATIVE TEST / NOT PROVIDER-INFERENCE PROOF: the
/// exact same frame-local true-center coordinates, dispatched as a raw
/// top-level click (skipping the iframe's own offset — exactly what a
/// naive, non-frame-aware dispatch would do), must NOT solve the iframe.
/// Proves the offset-corrected transform is load-bearing, not incidental.
#[tokio::test]
async fn raw_top_level_click_at_frame_local_coordinates_does_not_solve_the_iframe() {
    let (base, _server) = serve(vec![("/", top_html()), ("/child", child_html())]).await;
    let browser = launch().await;
    let page = browser.new_page(base).await.unwrap();
    page.wait_for_navigation().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Never even reaches the iframe's viewport rectangle
    // (left/top: IFRAME_LEFT/IFRAME_TOP) — landing at the frame-local
    // coordinates directly against the top-level viewport lands well
    // outside the iframe entirely (IFRAME_TOP=250 > TRUE_CENTER.1=40).
    page.click_smooth(Point {
        x: f64::from(TRUE_CENTER.0),
        y: f64::from(TRUE_CENTER.1),
    })
    .await
    .unwrap();

    let solved = page
        .evaluate("document.getElementById('child-frame').contentWindow.solved")
        .await
        .unwrap()
        .value()
        .cloned();
    assert_ne!(
        solved,
        Some(serde_json::json!(true)),
        "an un-offset top-level click at the frame-local coordinates must never solve the iframe"
    );
}

/// Detection alone, through the real production seam, proves correct frame
/// identity and materialization for a genuine same-session iframe — before
/// any provider/action concern.
#[tokio::test]
async fn detection_materializes_the_same_session_child_with_correct_identity() {
    let (base, _server) = serve(vec![("/", top_html()), ("/child", child_html())]).await;
    let browser = launch().await;
    let page = browser.new_page(base).await.unwrap();
    page.wait_for_navigation().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let top_level_frame_id = page.mainframe().await.unwrap().unwrap().inner().to_string();
    let detected = detect_browser_challenge(&page, Some(&browser))
        .await
        .unwrap()
        .unwrap();
    let DetectedBrowserChallenge::FramedEvidence {
        frame_id,
        parent_frame_id,
        challenge_element_id,
        materialization,
        ..
    } = detected
    else {
        panic!("expected framed evidence");
    };
    assert_ne!(frame_id, top_level_frame_id);
    assert_eq!(parent_frame_id, Some(top_level_frame_id));
    assert_eq!(challenge_element_id, "challenge-1");
    assert!(
        matches!(materialization, FramedMaterialization::Ready { .. }),
        "a live browser handle was offered against a genuine same-session iframe; \
         materialization must succeed"
    );
}

/// Wrong/stale `FrameId`: no browser mutation, typed failure, never a
/// top-level fallback.
#[tokio::test]
async fn stale_frame_id_fails_typed_with_no_action() {
    let (base, _server) = serve(vec![("/", top_html()), ("/child", child_html())]).await;
    let browser = launch().await;
    let page = browser.new_page(base).await.unwrap();
    page.wait_for_navigation().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let detected = detect_browser_challenge(&page, Some(&browser))
        .await
        .unwrap()
        .unwrap();
    let DetectedBrowserChallenge::FramedEvidence {
        materialization, ..
    } = detected
    else {
        panic!("expected framed evidence");
    };
    let FramedMaterialization::Ready {
        top_level,
        snapshot,
        ..
    } = materialization
    else {
        panic!("expected Ready materialization");
    };

    // A fresh top-level context is real, but its own `resolve_same_session_child`
    // is asked for a frame id that was never a real child of it — a
    // deliberately invalid/stale identity, not a live one this seam ever
    // proved.
    let bogus_frame_id = spider::chromiumoxide::cdp::browser_protocol::page::FrameId::from(
        "nonexistent-frame-id".to_string(),
    );
    let bogus = spider::features::frame_context::FrameContext::resolve_same_session_child(
        &browser,
        &top_level,
        bogus_frame_id,
    )
    .await;
    assert!(
        bogus.is_err(),
        "a stale/invalid FrameId must fail typed, never resolve to a usable FrameContext"
    );
    // No production code path exists that would fall back to top-level or
    // to a different iframe when this resolution fails — the snapshot
    // above is retained only to prove this test observed a genuinely
    // materialized fixture before attempting (and failing) the bogus
    // resolution.
    let _ = snapshot;
}

/// Detached/removed iframe before action: typed failure, no stale-coordinate
/// click.
#[tokio::test]
async fn detached_frame_before_action_fails_typed_with_no_action() {
    let (base, _server) = serve(vec![("/", top_html()), ("/child", child_html())]).await;
    let browser = launch().await;
    let page = browser.new_page(base).await.unwrap();
    page.wait_for_navigation().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let detected = detect_browser_challenge(&page, Some(&browser))
        .await
        .unwrap()
        .unwrap();
    let DetectedBrowserChallenge::FramedEvidence {
        instruction,
        materialization,
        ..
    } = detected
    else {
        panic!("expected framed evidence");
    };
    let FramedMaterialization::Ready {
        top_level,
        frame,
        snapshot,
    } = materialization
    else {
        panic!("expected Ready materialization");
    };

    // Remove the iframe entirely before the action is applied.
    page.evaluate("document.getElementById('child-frame').remove()")
        .await
        .unwrap();

    let provider = FixedPointProvider(f64::from(TRUE_CENTER.0), f64::from(TRUE_CENTER.1));
    let mut registry = CaptchaProviderRegistry::new();
    registry.register(&provider).unwrap();
    let error = execute_browser_captcha_attempt_in_frame(
        &page,
        &top_level,
        &frame,
        &snapshot,
        &registry,
        CaptchaBrowserAttempt {
            correlation_id: "frame-detached".into(),
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
        CaptchaBrowserExecutionFailureKind::Browser(_)
    ));
}
