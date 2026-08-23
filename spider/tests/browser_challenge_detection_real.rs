#![cfg(feature = "chrome")]

//! Real-Chrome integration proof for
//! `spider::features::browser_challenge_detection::detect_browser_challenge`,
//! covering the required adversarial matrix (A/B/C/D) against local,
//! deterministic HTML fixtures — no third-party CAPTCHA service, no
//! network egress beyond `127.0.0.1`.

use spider::features::browser_challenge_detection::{
    detect_browser_challenge, ChallengeDetectionFailure, DetectedBrowserChallenge,
};

/// Serve a small fixed route table over plain HTTP on `127.0.0.1`,
/// forever, until the returned handle is aborted. Supports multiple
/// requests (needed for the framed-challenge fixture, which loads both the
/// top document and its iframe).
async fn serve(routes: Vec<(&'static str, &'static str)>) -> (String, tokio::task::JoinHandle<()>) {
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
                let n = match stream.read(&mut buf).await {
                    Ok(n) if n > 0 => n,
                    _ => return,
                };
                let request = String::from_utf8_lossy(&buf[..n]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                let body = routes
                    .iter()
                    .find(|(route, _)| *route == path)
                    .map(|(_, body)| *body)
                    .unwrap_or("<!doctype html><html></html>");
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

const CHALLENGE_STYLE: &str = r#"<style>
  body{margin:0}
  #challenge-1{position:absolute;left:40px;top:40px;width:240px;height:120px;background:#ddd}
  #pick-1{position:absolute;left:20px;top:15px;width:60px;height:35px;background:#bbb}
</style>"#;

const SUPPORTED_CHALLENGE_BODY: &str = r#"<div id="challenge-1" role="application" aria-label="select the matching point">
  <div id="pick-1" role="button" tabindex="0">pick</div>
</div>"#;

/// Adversarial case A — an ordinary page. Expected: `Ok(None)`.
#[tokio::test]
async fn case_a_normal_page_is_not_detected() {
    let (base, _server) = serve(vec![(
        "/",
        "<!doctype html><body><h1>Hello world</h1><p>Just an ordinary page.</p></body>",
    )])
    .await;
    let browser = launch().await;
    let page = browser.new_page(base).await.unwrap();
    page.wait_for_navigation().await.unwrap();

    let result = detect_browser_challenge(&page).await.unwrap();
    assert!(result.is_none(), "an ordinary page must never be DETECTED");
}

/// Adversarial case B — a real, local, supported challenge structure.
/// Expected: `DETECTED`, fully materialized, top-level.
#[tokio::test]
async fn case_b_supported_challenge_is_detected_and_materialized() {
    let body = format!("<!doctype html>{CHALLENGE_STYLE}<body>{SUPPORTED_CHALLENGE_BODY}</body>");
    let (base, _server) = serve(vec![("/", Box::leak(body.into_boxed_str()))]).await;
    let browser = launch().await;
    let page = browser.new_page(base).await.unwrap();
    page.wait_for_navigation().await.unwrap();

    let result = detect_browser_challenge(&page).await.unwrap();
    match result {
        Some(DetectedBrowserChallenge::TopLevel {
            snapshot,
            challenge_element_id,
            instruction,
        }) => {
            assert_eq!(challenge_element_id, "challenge-1");
            assert_eq!(instruction, "select the matching point");
            assert!(snapshot.targets.contains_key("pick-1"));
            assert!(!snapshot.visual_bytes.is_empty());
            assert!(snapshot.captured_pixel_width > 0);
            assert!(snapshot.captured_pixel_height > 0);
        }
        Some(DetectedBrowserChallenge::FramedEvidence { .. }) => {
            panic!("expected a top-level detection, got framed evidence instead")
        }
        None => panic!("expected a top-level detection, got no detection"),
    }
}

/// Adversarial case C — misleading text ("captcha"/"verify"/"human") with
/// no supported structure. Expected: `Ok(None)` — proves this is not
/// page-text matching.
#[tokio::test]
async fn case_c_text_only_false_positive_is_not_detected() {
    let (base, _server) = serve(vec![(
        "/",
        r#"<!doctype html><body>
  <div class="captcha-banner">Please verify you are human before continuing.</div>
  <button>Continue</button>
</body>"#,
    )])
    .await;
    let browser = launch().await;
    let page = browser.new_page(base).await.unwrap();
    page.wait_for_navigation().await.unwrap();

    let result = detect_browser_challenge(&page).await.unwrap();
    assert!(
        result.is_none(),
        "plain text mentioning captcha/verify/human without ARIA challenge \
         structure must never be DETECTED — the detector must not be doing \
         naive text matching"
    );
}

/// Adversarial case D — the same supported challenge, but placed inside a
/// same-origin (same-session) child `<iframe>`. Expected: `DETECTED`, with
/// the exact child `FrameId` (not the top-level one) preserved.
#[tokio::test]
async fn case_d_framed_challenge_is_detected_with_correct_frame_identity() {
    let top = r#"<!doctype html><body><iframe src="/child" style="width:400px;height:300px;border:0"></iframe></body>"#;
    let child = format!("<!doctype html>{CHALLENGE_STYLE}<body>{SUPPORTED_CHALLENGE_BODY}</body>");
    let (base, _server) = serve(vec![
        ("/", top),
        ("/child", Box::leak(child.into_boxed_str())),
    ])
    .await;
    let browser = launch().await;
    let page = browser.new_page(base).await.unwrap();
    page.wait_for_navigation().await.unwrap();
    // Let the iframe's own document finish loading before inspecting.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let top_level_frame_id = page.mainframe().await.unwrap().unwrap().inner().to_string();

    let result = detect_browser_challenge(&page).await.unwrap();
    match result {
        Some(DetectedBrowserChallenge::FramedEvidence {
            frame_id,
            parent_frame_id,
            challenge_element_id,
        }) => {
            assert_ne!(
                frame_id, top_level_frame_id,
                "a framed challenge's frame_id must be the child frame's, not the top-level frame's"
            );
            assert_eq!(parent_frame_id, Some(top_level_frame_id));
            assert_eq!(challenge_element_id, "challenge-1");
        }
        Some(DetectedBrowserChallenge::TopLevel { .. }) => {
            panic!("expected framed evidence, got a top-level detection instead")
        }
        None => panic!("expected framed evidence, got no detection"),
    }
}

/// Adversarial case F — detection performs no browser mutation. Detect
/// twice in a row and assert the DOM state used to prove "no mutation" (an
/// untouched sentinel counter) never moved.
#[tokio::test]
async fn case_f_detection_performs_no_browser_mutation() {
    let body = format!(
        "<!doctype html>{CHALLENGE_STYLE}<body>{SUPPORTED_CHALLENGE_BODY}\
         <script>window.__mutationSentinel = 0;\
         new MutationObserver(() => {{ window.__mutationSentinel++; }})\
           .observe(document.body, {{subtree:true, childList:true, attributes:true}});\
         </script></body>"
    );
    let (base, _server) = serve(vec![("/", Box::leak(body.into_boxed_str()))]).await;
    let browser = launch().await;
    let page = browser.new_page(base).await.unwrap();
    page.wait_for_navigation().await.unwrap();

    let _ = detect_browser_challenge(&page).await.unwrap();
    let _ = detect_browser_challenge(&page).await.unwrap();
    // Give the MutationObserver's microtask a turn to run, if anything did
    // mutate.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let sentinel = page
        .evaluate("window.__mutationSentinel")
        .await
        .unwrap()
        .value()
        .cloned();
    assert_eq!(
        sentinel,
        Some(serde_json::json!(0)),
        "detection must never mutate the page's DOM"
    );
}

/// Adversarial case E — observation itself fails (the tab is gone).
/// Expected: a typed `ChallengeDetectionFailure`, never `Ok(None)` and
/// never a false `Some(..)` — a failed observation must not be
/// indistinguishable from "genuinely inspected and clean".
#[tokio::test]
async fn case_e_detector_failure_is_typed_not_no_challenge() {
    let (base, _server) = serve(vec![(
        "/",
        "<!doctype html><body><h1>Hello world</h1></body>",
    )])
    .await;
    let browser = launch().await;
    let page = browser.new_page(base).await.unwrap();
    page.wait_for_navigation().await.unwrap();
    let closable = page.clone();
    closable.close().await.unwrap();

    let result = detect_browser_challenge(&page).await;
    match result {
        Err(ChallengeDetectionFailure::ObservationFailed) => {}
        Err(other) => {
            panic!("expected ObservationFailed, got a different typed failure: {other:?}")
        }
        Ok(_) => panic!("a closed/unreachable tab must fail typed, not report Ok(..)"),
    }
}
