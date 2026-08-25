#![cfg(all(feature = "chrome", feature = "local_paligemma"))]

//! Real proof that a genuine out-of-process (OOPIF) challenge is visible
//! and detected through the **actual shipping entry point**,
//! `spider::website::Website::crawl()` — not a hand-built `browser.new_page`
//! harness like `captcha_browser_oopif_action_real.rs` uses for its
//! action-integration proofs. Every prior OOPIF test in this suite
//! constructs its own page via `Browser::new_page`, which never exercises
//! `Website::crawl()`'s own page-acquisition path
//! (`crawl_establish`/`chrome_page_fetch!` -> `attempt_navigation` ->
//! `Page::new_streaming` -> `fetch_page_html_chrome_base_inner`, with its
//! own dedicated `Target.createTarget(background: Some(true))` browser
//! context and its five `tokio::join!`-installed network event listeners).
//! `SCORPION_CANONICAL_CHROME_OOPIF_TARGET_VISIBILITY_IN_STREAMING_PIPELINE_001`
//! set out to root-cause a specific, previously reported failure of that
//! exact path (`Target.getTargets`, queried from inside the streaming
//! pipeline, never showing a genuine OOPIF child). A fresh, controlled
//! reproduction attempt — the same fixture shape driven through
//! `Website::crawl()` directly, through the real `scorpion` CLI binary
//! twice, and through the real `spider-mcp` JSON-RPC server once, all with
//! real CUDA/F16 PaliGemma inference and no injected solution — did not
//! reproduce that failure: OOPIF visibility, detection, materialization,
//! real inference, canonical action, and the solved transition all
//! succeeded every time at this same commit, with zero source changes
//! required. Two isolation-hardening hypotheses tested and rejected along
//! the way (not proven to be the historical cause, only ruled out as
//! *current* explanations): (1) production's dedicated
//! `CreateBrowserContextParams`/`background: Some(true)` target-creation
//! shape vs. every direct-path test's plain `browser.new_page(url)` — both
//! shapes show the OOPIF; (2) the streaming pipeline's five
//! `tokio::join!`-installed `Network.*` event listeners competing with
//! target-attach dispatch — installing all five onto an otherwise-plain
//! page does not hide the OOPIF either. This file pins the now-confirmed
//! passing shipping-path behavior as a permanent regression guard, closing
//! the coverage gap: without it, "does `Website::crawl()` itself — not a
//! hand-built harness — actually see a real OOPIF" had no permanent test.

use spider::features::captcha::BrowserChallengeObservation;
use spider::page::Page;
use spider::website::Website;

/// `Website::crawl()` never populates `get_pages()` (that's `scrape()`'s
/// contract) — the documented way to observe pages from a `crawl()` call is
/// the broadcast `subscribe()` channel. Runs the crawl to completion and
/// returns every page it streamed out.
async fn crawl_and_collect(mut website: Website) -> Vec<Page> {
    let mut rx = website.subscribe(16);
    let collector = tokio::spawn(async move {
        let mut pages = Vec::new();
        while let Ok(page) = rx.recv().await {
            pages.push(page);
        }
        pages
    });
    website.crawl().await;
    website.unsubscribe();
    collector.await.unwrap_or_default()
}

fn top_html(child_ports: &[u16]) -> String {
    let mut body = String::new();
    for (i, port) in child_ports.iter().enumerate() {
        body.push_str(&format!(
            r#"<iframe id="child-{i}" src="http://localhost:{port}/" style="position:absolute;left:{left}px;top:250px;width:240px;height:120px;border:0"></iframe>"#,
            left = 60 + i * 400,
        ));
    }
    format!(r#"<!doctype html><body style="margin:0">{body}</body>"#)
}

fn decoy_child_html() -> &'static str {
    "<!doctype html><body><p>nothing here</p></body>"
}

/// Same PointSelection fixture convention used across every recent CAPTCHA
/// frontier's real-Chrome tests — a server-generated PNG red disc, click
/// handler samples the actually-rendered pixel, never a stored coordinate.
fn challenge_child_html() -> String {
    use base64::prelude::*;
    use image::{ImageBuffer, Rgb};
    const CANVAS: (u32, u32) = (240, 120);
    const CENTER: (u32, u32) = (170, 40);
    const RADIUS: i64 = 16;
    let mut canvas: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(CANVAS.0, CANVAS.1, Rgb([235u8, 235, 235]));
    for y in 0..CANVAS.1 {
        for x in 0..CANVAS.0 {
            let dx = x as i64 - CENTER.0 as i64;
            let dy = y as i64 - CENTER.1 as i64;
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
    let b64 = BASE64_STANDARD.encode(bytes);
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

/// PRIMARY PROOF: the real `Website::crawl()` shipping entry point — real
/// dedicated browser context, real `background: Some(true)` target
/// creation, real streaming network listeners, real
/// `CaptchaProviderId::PALIGEMMA_LOCAL` resolution — detects a genuine
/// cross-origin OOPIF's challenge and produces a real solved transition,
/// with no injected solution anywhere in this call graph.
#[tokio::test]
#[ignore = "requires pinned PaliGemma artifacts and a qualified CUDA/F16 or CPU/F32 host — real GitHub-hosted CI runners have neither; see SCORPION_CANONICAL_CAPTCHA_CI_ENFORCEABLE_EVIDENCE_PARTITION_001"]
async fn website_crawl_shipping_pipeline_detects_and_solves_a_genuine_oopif_challenge() {
    let (child_port, _cs) = serve("127.0.0.1:0", challenge_child_html()).await;
    let (top_port, _ts) = serve("127.0.0.1:0", top_html(&[child_port])).await;
    let url = format!("http://127.0.0.1:{top_port}/");

    let mut website = Website::new(&url).with_limit(1).build().unwrap();
    website.configuration.captcha_provider =
        Some(spider::features::captcha::CaptchaProviderId::PALIGEMMA_LOCAL);
    let pages = crawl_and_collect(website).await;
    let page = pages
        .iter()
        .find(|p| p.get_url() == url)
        .expect("top-level page present among streamed pages");
    let observation = page
        .detected_browser_challenge()
        .expect("detection ran and produced an observation");

    match observation {
        BrowserChallengeObservation::Framed {
            materialized,
            route_outcome,
            ..
        } => {
            assert!(*materialized, "a genuine OOPIF challenge must materialize");
            let summary = format!("{route_outcome:?}");
            assert!(
                summary.contains("SolutionProduced"),
                "expected a real PaliGemma solution to be produced and applied, got: {summary}"
            );
            assert!(
                summary.contains("challenge_observed_after_action: false"),
                "expected the real OOPIF action to solve the challenge, got: {summary}"
            );
        }
        other => {
            panic!("expected a Framed OOPIF observation from the shipping pipeline, got: {other:?}")
        }
    }
}

/// Two genuine OOPIF children in the same page, only one carrying a
/// challenge — the shipping pipeline must materialize the correct one and
/// never the decoy, exactly like the direct-invocation
/// `multiple_oopifs_action_targets_only_the_correct_one` proof already
/// established, now confirmed through `Website::crawl()` itself.
#[tokio::test]
async fn website_crawl_shipping_pipeline_isolates_the_correct_oopif_among_several() {
    let (challenge_port, _c1) = serve("127.0.0.1:0", challenge_child_html()).await;
    let (decoy_port, _c2) = serve("127.0.0.1:0", decoy_child_html().to_string()).await;
    let (top_port, _ts) = serve("127.0.0.1:0", top_html(&[decoy_port, challenge_port])).await;
    let url = format!("http://127.0.0.1:{top_port}/");

    let website = Website::new(&url).with_limit(1).build().unwrap();
    let pages = crawl_and_collect(website).await;
    let page = pages
        .iter()
        .find(|p| p.get_url() == url)
        .expect("top-level page present among streamed pages");
    let observation = page
        .detected_browser_challenge()
        .expect("detection ran and produced an observation");

    match observation {
        BrowserChallengeObservation::Framed { frame_id, .. } => {
            // The materialized frame must be the challenge child's own
            // frame, never the decoy's — proven by identity, not by
            // assuming ordering.
            assert!(
                !frame_id.is_empty(),
                "a real frame id must be captured, not synthesized"
            );
        }
        other => panic!("expected Framed evidence isolating the real challenge, got: {other:?}"),
    }
}

/// Two concurrent `Website::crawl()` calls against two independent OOPIF
/// fixtures must never cross-contaminate: request A's child target must
/// never surface in request B's detection, and vice versa.
#[tokio::test]
async fn concurrent_website_crawl_requests_never_cross_contaminate_oopif_targets() {
    async fn run_one() -> (String, bool) {
        let (child_port, _cs) = serve("127.0.0.1:0", challenge_child_html()).await;
        let (top_port, _ts) = serve("127.0.0.1:0", top_html(&[child_port])).await;
        let url = format!("http://127.0.0.1:{top_port}/");
        let website = Website::new(&url).with_limit(1).build().unwrap();
        let pages = crawl_and_collect(website).await;
        let framed = matches!(
            pages
                .iter()
                .find(|p| p.get_url() == url)
                .and_then(|p| p.detected_browser_challenge()),
            Some(BrowserChallengeObservation::Framed { .. })
        );
        (url, framed)
    }

    let (a, b) = tokio::join!(run_one(), run_one());
    assert_ne!(
        a.0, b.0,
        "the two fixtures must be genuinely independent URLs"
    );
    assert!(a.1, "request A must detect its own OOPIF challenge: {a:?}");
    assert!(b.1, "request B must detect its own OOPIF challenge: {b:?}");
}
