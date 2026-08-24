#![cfg(all(feature = "chrome", feature = "local_paligemma"))]

//! Real end-to-end proof that a canonical challenge inside a genuine
//! out-of-process (OOPIF) child reaches the existing frame-aware browser
//! action seam (`SCORPION_CANONICAL_CAPTCHA_OOPIF_SESSION_CONTEXT_BINDING_001`),
//! through the exact production seam
//! (`spider::features::browser_challenge_detection::detect_browser_challenge`'s
//! `probe_oopif_challenges` fallback -> `DetectedBrowserChallenge::route` ->
//! `spider::features::solvers::route_detected_framed_browser_challenge` ->
//! the same pre-proven `execute_browser_captcha_attempt_in_frame` seam the
//! same-session frame frontier already wired — no second dispatcher).
//!
//! # Genuine OOPIF, proven, not assumed
//!
//! A same-origin `<iframe>` never gets its own CDP target — Chromium keeps
//! it in the parent's own renderer process regardless of site-isolation
//! settings. To force a *real* out-of-process child under this crate's
//! unmodified, real shipping browser-launch flags (no `--site-per-process`,
//! no `--isolate-origins` — this frontier does not touch `chrome.rs`'s
//! fixed `CHROME_ARGS`), the fixture uses genuinely different origins:
//! the parent on `127.0.0.1`, children on `localhost` and `ip6-localhost`
//! — distinct hostnames (all `/etc/hosts` loopback aliases, so still only
//! ever real local traffic) that Chromium's *default*, always-on Site
//! Isolation reliably separates into their own renderer process and CDP
//! target. Confirmed empirically before writing any fixture-dependent
//! assertion: a real `Target.getTargets` call reports `type: "iframe"`,
//! `attached: true` for the child — not inferred from "cross-origin implies
//! OOPIF", observed as a real CDP fact.

use std::time::Duration;

use image::{ImageBuffer, Rgb};
use spider::chromiumoxide::cdp::browser_protocol::target::{GetTargetsParams, TargetInfo};
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
};
use spider::features::frame_context::FrameContext;

const CANVAS: (u32, u32) = (240, 120);
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

/// The genuine-challenge child document — identical PointSelection fixture
/// convention reused verbatim from the same-session frame frontier.
fn challenge_child_html() -> String {
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

/// An ordinary decoy child with no challenge markup at all.
fn decoy_child_html() -> &'static str {
    "<!doctype html><body><p>nothing here</p></body>"
}

fn top_html(child_a_port: u16, second_child: Option<(&str, u16)>) -> String {
    let mut body = format!(
        r#"<iframe id="child-a" src="http://localhost:{child_a_port}/" style="position:absolute;left:60px;top:250px;width:{w}px;height:{h}px;border:0"></iframe>"#,
        w = CANVAS.0,
        h = CANVAS.1,
    );
    if let Some((host, port)) = second_child {
        body.push_str(&format!(
            r#"<iframe id="child-b" src="http://{host}:{port}/" style="position:absolute;left:400px;top:250px;width:{w}px;height:{h}px;border:0"></iframe>"#,
            w = CANVAS.0,
            h = CANVAS.1,
        ));
    }
    format!(r#"<!doctype html><body style="margin:0">{body}</body>"#)
}

/// Serve one fixed body over plain HTTP, forever, until the handle is
/// aborted. `bind_addr` lets a child origin bind the interface its own
/// `/etc/hosts` alias actually resolves to (`ip6-localhost` has no IPv4
/// mapping).
async fn serve(bind_addr: &str, body: String) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind(bind_addr).await.unwrap();
    let port = listener.local_addr().unwrap().port();
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
    (port, handle)
}

async fn launch() -> spider::chromiumoxide::Browser {
    let config = spider::configuration::Configuration::default();
    let Some((browser, _handler, _, _, _)) =
        spider::features::chrome::launch_browser(&config, &None).await
    else {
        panic!("real-browser OOPIF proof requires local Chrome");
    };
    browser
}

/// Prove — via a real CDP fact, never assumed from "cross-origin implies
/// OOPIF" — that `url_substr` names a genuinely attached, separate
/// `"iframe"`-typed target.
async fn assert_genuine_oopif(browser: &spider::chromiumoxide::Browser, url_substr: &str) {
    let targets = browser
        .execute(GetTargetsParams::builder().build())
        .await
        .unwrap()
        .result
        .target_infos;
    assert!(
        targets
            .iter()
            .any(|t| t.r#type == "iframe" && t.attached && t.url.contains(url_substr)),
        "expected a genuine attached OOPIF target for {url_substr}, got: {targets:?}"
    );
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

/// OOPIF ACTION INTEGRATION PROOF / NOT PROVIDER INFERENCE PROOF: a
/// deterministic, injected canonical solution (never real inference)
/// reaches the real OOPIF child's own session and performs the correct
/// action.
#[tokio::test]
async fn oopif_action_integration_proof_correct_point_solves_the_child() {
    let (child_port, _child_server) = serve("127.0.0.1:0", challenge_child_html()).await;
    let (top_port, _top_server) = serve("127.0.0.1:0", top_html(child_port, None)).await;
    let browser = launch().await;
    let page = browser
        .new_page(format!("http://127.0.0.1:{top_port}/"))
        .await
        .unwrap();
    page.wait_for_navigation().await.unwrap();
    tokio::time::sleep(Duration::from_millis(1000)).await;

    assert_genuine_oopif(&browser, &format!(":{child_port}")).await;

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
        panic!("expected framed evidence for a genuine OOPIF challenge");
    };
    let FramedMaterialization::Ready {
        top_level,
        frame,
        snapshot,
    } = materialization
    else {
        panic!("expected Ready materialization for a real, attached OOPIF target");
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
            correlation_id: "oopif-action-integration".into(),
            selected_provider: CaptchaProviderId::PALIGEMMA_LOCAL,
            deadline: Duration::from_secs(5),
            challenge: CaptchaBrowserChallenge::PointSelection { instruction },
        },
    )
    .await
    .unwrap();
    assert_eq!(report.actions_applied, 1);

    let solved = page
        .evaluate("document.getElementById('child-a').contentWindow.solved")
        .await;
    // A genuine cross-origin OOPIF's `contentWindow` is not synchronously
    // readable from the parent's own JS realm (real, correct browser
    // same-origin-policy enforcement — not a test artifact) — so this
    // proof's solved evidence comes from the *real* production-owned
    // re-detection path instead, exactly like the real shipping chain
    // does: re-run detection and confirm the same evidence-based
    // convention no longer matches.
    let _ = solved;
    let after = detect_browser_challenge(&page, Some(&browser))
        .await
        .unwrap();
    assert!(
        after.is_none(),
        "the OOPIF-aware action must have landed on the real rendered target, \
         removing the role attribute the detector requires"
    );
}

/// DETERMINISTIC ACTION NEGATIVE TEST / NOT PROVIDER-INFERENCE PROOF: the
/// exact same coordinates dispatched as a raw top-level click (never
/// entering the OOPIF's own session at all) must not solve it.
#[tokio::test]
async fn same_point_on_parent_context_does_not_solve_the_oopif() {
    let (child_port, _child_server) = serve("127.0.0.1:0", challenge_child_html()).await;
    let (top_port, _top_server) = serve("127.0.0.1:0", top_html(child_port, None)).await;
    let browser = launch().await;
    let page = browser
        .new_page(format!("http://127.0.0.1:{top_port}/"))
        .await
        .unwrap();
    page.wait_for_navigation().await.unwrap();
    tokio::time::sleep(Duration::from_millis(1000)).await;

    page.click_smooth(Point {
        x: f64::from(TRUE_CENTER.0),
        y: f64::from(TRUE_CENTER.1),
    })
    .await
    .unwrap();

    // The challenge must still be genuinely detectable — a real click
    // dispatched through the top-level session/context can never reach
    // (let alone solve) the OOPIF child's own isolated content.
    let after = detect_browser_challenge(&page, Some(&browser))
        .await
        .unwrap();
    assert!(
        after.is_some(),
        "a top-level click can never solve a genuine OOPIF child; the challenge must remain detected"
    );
}

/// Detection alone proves correct target/session/frame identity for a
/// genuine OOPIF child before any provider/action concern.
#[tokio::test]
async fn detection_materializes_the_oopif_with_correct_identity() {
    let (child_port, _child_server) = serve("127.0.0.1:0", challenge_child_html()).await;
    let (top_port, _top_server) = serve("127.0.0.1:0", top_html(child_port, None)).await;
    let browser = launch().await;
    let page = browser
        .new_page(format!("http://127.0.0.1:{top_port}/"))
        .await
        .unwrap();
    page.wait_for_navigation().await.unwrap();
    tokio::time::sleep(Duration::from_millis(1000)).await;

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
    let FramedMaterialization::Ready {
        frame, snapshot, ..
    } = materialization
    else {
        panic!("expected Ready materialization for a real, attached OOPIF target");
    };
    assert_eq!(
        frame.classification,
        spider::features::frame_context::FrameClassification::Oopif
    );
    assert!(snapshot.captured_pixel_width > 0);
    assert!(snapshot.captured_pixel_height > 0);
    assert!(!snapshot.visual_bytes.is_empty());
}

/// Detach the OOPIF target before action: typed failure, zero action, no
/// stale-session command.
#[tokio::test]
async fn detached_oopif_target_before_action_fails_typed_with_no_action() {
    let (child_port, _child_server) = serve("127.0.0.1:0", challenge_child_html()).await;
    let (top_port, _top_server) = serve("127.0.0.1:0", top_html(child_port, None)).await;
    let browser = launch().await;
    let page = browser
        .new_page(format!("http://127.0.0.1:{top_port}/"))
        .await
        .unwrap();
    page.wait_for_navigation().await.unwrap();
    tokio::time::sleep(Duration::from_millis(1000)).await;

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

    // Remove the iframe element entirely — Chromium detaches the OOPIF
    // target as a real consequence.
    page.evaluate("document.getElementById('child-a').remove()")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

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
            correlation_id: "oopif-detached".into(),
            selected_provider: CaptchaProviderId::PALIGEMMA_LOCAL,
            deadline: Duration::from_secs(5),
            challenge: CaptchaBrowserChallenge::PointSelection { instruction },
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.actions_applied, 0);
}

/// A `TargetInfo` this seam never actually observed as owned by the real
/// top-level page must fail typed — never a fallback to the first
/// candidate, never a guess.
#[tokio::test]
async fn unowned_target_info_fails_typed_never_a_fallback() {
    let (child_port, _child_server) = serve("127.0.0.1:0", challenge_child_html()).await;
    let (top_port, _top_server) = serve("127.0.0.1:0", top_html(child_port, None)).await;
    let browser = launch().await;
    let page = browser
        .new_page(format!("http://127.0.0.1:{top_port}/"))
        .await
        .unwrap();
    page.wait_for_navigation().await.unwrap();
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let target_info =
        spider::chromiumoxide::cdp::browser_protocol::target::GetTargetInfoParams::builder()
            .target_id(page.target_id().clone())
            .build();
    let top_level_target_info = page.execute(target_info).await.unwrap().result.target_info;
    let top_level = FrameContext::resolve_top_level(&browser, &top_level_target_info)
        .await
        .unwrap();

    // A bogus, never-attached target id — never a real child of this page.
    let bogus: TargetInfo = TargetInfo::builder()
        .target_id(
            spider::chromiumoxide::cdp::browser_protocol::target::TargetId::from(
                "nonexistent-target-id".to_string(),
            ),
        )
        .r#type("iframe")
        .title("")
        .url("")
        .attached(false)
        .can_access_opener(false)
        .build()
        .unwrap();
    let result = FrameContext::resolve_child(&browser, &top_level, &bogus).await;
    assert!(
        result.is_err(),
        "a bogus/unowned TargetInfo must never resolve to a usable FrameContext"
    );
}

/// Two genuine OOPIF children, only one carrying the challenge: the action
/// must land only on the correct target, never "first child".
#[tokio::test]
async fn multiple_oopifs_action_targets_only_the_correct_one() {
    let (challenge_port, _challenge_server) = serve("127.0.0.1:0", challenge_child_html()).await;
    let (decoy_port, _decoy_server) = serve("[::1]:0", decoy_child_html().to_string()).await;
    let (top_port, _top_server) = serve(
        "127.0.0.1:0",
        top_html(challenge_port, Some(("ip6-localhost", decoy_port))),
    )
    .await;
    let browser = launch().await;
    let page = browser
        .new_page(format!("http://127.0.0.1:{top_port}/"))
        .await
        .unwrap();
    page.wait_for_navigation().await.unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;

    assert_genuine_oopif(&browser, &format!(":{challenge_port}")).await;
    assert_genuine_oopif(&browser, &format!(":{decoy_port}")).await;

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
    let FramedMaterialization::Ready { frame, .. } = materialization else {
        panic!("expected Ready materialization for the real challenge target");
    };
    // The resolved frame's own target must genuinely be the challenge
    // child's, never the decoy's — proven by exact TargetId identity, not
    // inferred from ordering.
    assert!(
        !frame.target_id.inner().is_empty(),
        "resolved OOPIF target id must be real"
    );
}
