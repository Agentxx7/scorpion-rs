#![cfg(all(feature = "chrome", feature = "local_paligemma_cuda"))]

//! Genuine authorized-Cloudflare-Turnstile acceptance for
//! `SCORPION_CANONICAL_CAPTCHA_BROWSER_EXECUTION_BINDING_001`'s frame-aware
//! resumption: a real interactive Turnstile test widget (Cloudflare's own
//! documented "forces an interactive challenge" test sitekey
//! `3x00000000000000000000FF` — visibly marked "For testing only. If seen,
//! report to site owner"), rendered in a genuine out-of-process child frame,
//! solved end to end through
//!
//! Turnstile iframe -> canonical FrameContext -> frame-aware
//! BrowserChallengeSnapshot (`capture_in_frame`) -> canonical CAPTCHA
//! materialization -> `paligemma-local` real pinned CUDA/F16 inference ->
//! `CaptchaSolveOutcome` -> frame-aware revalidation
//! (`revalidate_in_frame`) -> exact canonical browser action
//! (`apply_in_frame`) -> observable genuine challenge progression (the
//! widget's own `data-callback`, invoked by Cloudflare's real client script
//! with a token, plus its visible state turning to "Success!").
//!
//! Ignored in ordinary CI: requires the pinned ~11.7 GB PaliGemma
//! installation, a qualified CUDA/F16 host with the required free VRAM, and
//! outbound network access to challenges.cloudflare.com.
//!
//! `paligemma-local` is the canonical CAPTCHA provider for this binding.
//! Two distinct gaps were found and closed by prior frontiers before this
//! run was attempted:
//!
//! - **Latency** (`SCORPION_PALIGEMMA_LOCAL_INFERENCE_LATENCY_QUALIFICATION_001`):
//!   an earlier attempt through this identical frame-aware seam, using the
//!   CPU/F32 backend, genuinely proved every layer up to and including the
//!   exact browser action correct (hand-computing the checkbox's true
//!   center and dispatching that exact point through this identical
//!   `apply_in_frame` path reliably produced Cloudflare's real dummy
//!   success token), but the fully automated run failed at frame-aware
//!   revalidation (`BrowserChallengeFailure::TargetReplaced`,
//!   `actions_applied: 0`): CPU/F32 `detect` inference took ~426s, longer
//!   than a real Turnstile interactive test challenge's own measured
//!   lifetime (~110s), so Cloudflare's own client script had already
//!   replaced the widget's child target before the real answer came back.
//!   The CUDA/F16 backend closed that gap: ~1.0-3.1s per query, ~18-55x
//!   inside the frozen 55.03s budget (50% of the measured minimum
//!   challenge lifetime).
//! - **Real-world grounding** (`SCORPION_PALIGEMMA_REAL_BROWSER_RASTER_GROUNDING_ROOT_CAUSE_001`
//!   through `SCORPION_PALIGEMMA_448_PRODUCTION_RUNTIME_INTEGRATION_001`):
//!   the original `google/paligemma-3b-mix-224` checkpoint, once the
//!   canonical short-label prompt contract was also fixed, still produced
//!   a real-content X-axis grounding failure on a genuine captured
//!   Turnstile raster (Y-axis correct, X collapsed to mid-canvas). The
//!   qualified `google/paligemma-3b-mix-448` checkpoint (same provider,
//!   same architecture, same grammar, higher native resolution) resolved
//!   this on the identical frozen genuine raster: 2/2 deterministic
//!   containment, predicted point ≈(20.1, 33.3) against a true checkbox
//!   center of ≈(20.5, 32.5).
//!
//! This test exclusively uses the qualified 448 CUDA/F16 constructor
//! (`initialize_448_cuda_f16_from_host`) — never the CPU/F32 path, never
//! the 224 checkpoint, and never a silent fallback between them.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use spider::chromiumoxide::browser::{Browser, HeadlessMode};
use spider::chromiumoxide::cdp::browser_protocol::dom::{
    BackendNodeId, DescribeNodeParams, GetBoxModelParams, ResolveNodeParams,
};
use spider::chromiumoxide::cdp::browser_protocol::target::{
    EventAttachedToTarget, EventTargetCreated,
};
use spider::chromiumoxide::cdp::js_protocol::runtime::{CallFunctionOnParams, EvaluateParams};
use spider::chromiumoxide::BrowserConfig;
use spider::features::browser_challenge::BrowserChallengeSnapshot;
use spider::features::captcha::{CaptchaProviderId, CaptchaProviderRegistry};
use spider::features::captcha_browser::{
    execute_browser_captcha_attempt_in_frame, CaptchaBrowserAttempt, CaptchaBrowserChallenge,
    CaptchaBrowserExecutionStage,
};
use spider::features::frame_context::FrameContext;
use spider::features::paligemma_captcha::PaligemmaLocalCaptchaProvider;
use spider::features::paligemma_runtime::paligemma_448_cuda_f16_manifest;
use spider::tokio_stream::StreamExt;

const HTML: &str = r#"<!doctype html><html><body style="margin:0">
<script>
  window.turnstileToken = null;
  window.turnstileError = null;
  window.onTurnstileSuccess = function(token) { window.turnstileToken = token; };
  window.onTurnstileError = function(err) { window.turnstileError = err; };
</script>
<script src="https://challenges.cloudflare.com/turnstile/v0/api.js" async defer></script>
<div class="cf-turnstile" data-sitekey="3x00000000000000000000FF" data-callback="onTurnstileSuccess" data-error-callback="onTurnstileError"></div>
</body></html>"#;

/// Grow the widget's own owning `<iframe>` element (on the fixture page this
/// test itself controls and serves) from Turnstile's native ~300x65 CSS-pixel
/// footprint to exactly the paligemma-local runtime's qualified 224x224
/// processor envelope, before any capture. This is ordinary,
/// generically-applicable test-fixture sizing — not Turnstile-specific
/// solver logic, not a change to canonical materialization/capture/runtime
/// code, not image padding inside the pipeline: Cloudflare's real widget
/// simply repaints its own real background/content to fill whatever box its
/// iframe occupies, so the captured screenshot is still entirely genuine
/// rendered content. The owning element is resolved purely by the
/// already-canonical `FrameContext::frame_owner` backend node id — never a
/// selector, which would not reach it anyway (it sits behind a closed shadow
/// root on the host page, confirmed empirically).
async fn grow_widget_iframe_to_qualified_envelope(top_level: &FrameContext, frame: &FrameContext) {
    let owner_backend_node_id = frame
        .frame_owner
        .as_ref()
        .expect("a child frame must have a resolved owner")
        .backend_node_id;
    for _ in 0..2 {
        let object = top_level
            .execute(
                ResolveNodeParams::builder()
                    .backend_node_id(owner_backend_node_id)
                    .build(),
            )
            .await
            .expect("resolving the owning <iframe> element must succeed");
        let object_id = object
            .result
            .object
            .object_id
            .expect("owner must be an object");
        top_level
            .execute(
                CallFunctionOnParams::builder()
                    .function_declaration(
                        "function() { this.style.width='224px'; this.style.height='224px'; }",
                    )
                    .object_id(object_id)
                    .build()
                    .unwrap(),
            )
            .await
            .expect("resizing the owning <iframe> element must succeed");
        // Cloudflare's own script re-applies its preferred size shortly
        // after initial layout; a second pass after a short delay wins.
        tokio::time::sleep(Duration::from_secs(4)).await;
    }
    let model = top_level
        .execute(
            GetBoxModelParams::builder()
                .backend_node_id(owner_backend_node_id)
                .build(),
        )
        .await
        .expect("reading the resized owner's box model must succeed");
    assert_eq!(
        (model.result.model.width, model.result.model.height),
        (224, 224),
        "the owning <iframe> element must end up at exactly the qualified envelope size"
    );
}

async fn body_backend_node_id(frame: &FrameContext) -> BackendNodeId {
    let object = frame
        .execute(
            EvaluateParams::builder()
                .expression("document.body")
                .context_id(frame.execution_context_id)
                .build()
                .unwrap(),
        )
        .await
        .unwrap();
    let object_id = object.result.result.object_id.unwrap();
    frame
        .execute(DescribeNodeParams {
            object_id: Some(object_id),
            ..Default::default()
        })
        .await
        .unwrap()
        .result
        .node
        .backend_node_id
}

/// Qualification-host proof. It is ignored in ordinary CI because it
/// requires the pinned ~11.7 GB installation, a qualified CUDA/F16 host
/// with the required free VRAM, and real outbound network access to
/// Cloudflare's Turnstile service. Fails closed (no CPU fallback) if CUDA
/// or the required VRAM is unavailable.
#[tokio::test]
#[ignore = "requires pinned PaliGemma artifacts, a qualified CUDA/F16 host, and network access to challenges.cloudflare.com"]
async fn real_turnstile_snapshot_paligemma_inference_and_exact_action() {
    let source = PathBuf::from(
        std::env::var("SCORPION_PALIGEMMA_448_PINNED_ARTIFACTS")
            .expect("set pinned offline google/paligemma-3b-mix-448 artifact directory"),
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
    // Qualified google/paligemma-3b-mix-448 CUDA/F16 constructor only —
    // fails closed
    // (`PaligemmaRuntimeFailure::DeviceUnavailable`/`DeviceMemoryLimitExceeded`)
    // if no CUDA device or insufficient VRAM is present; never silently
    // falls back to the CPU/F32 path or the 224 checkpoint. See
    // `SCORPION_PALIGEMMA_448_PRODUCTION_RUNTIME_INTEGRATION_001`: this is
    // the checkpoint that genuinely resolved the real-raster horizontal
    // grounding failure the 224 checkpoint could not.
    let provider =
        PaligemmaLocalCaptchaProvider::initialize_448_cuda_f16_from_host(&installation).unwrap();
    let mut registry = CaptchaProviderRegistry::new();
    registry.register(&provider).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => continue,
            };
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut request = [0u8; 2048];
                let _ = stream.read(&mut request).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    HTML.len(),
                    HTML
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });

    let profile =
        std::env::temp_dir().join(format!("scorpion-turnstile-real-{}", std::process::id()));
    let config = BrowserConfig::builder()
        .user_data_dir(profile)
        .chrome_executable("/usr/bin/chromium")
        .headless_mode(HeadlessMode::New)
        .incognito()
        .arg("--no-sandbox")
        .launch_timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let Ok((browser, mut handler)) = Browser::launch(config).await else {
        panic!("real Turnstile acceptance requires local Chrome");
    };
    tokio::spawn(async move { while handler.next().await.is_some() {} });
    let browser = std::sync::Arc::new(browser);

    let mut creations = browser
        .event_listener::<EventTargetCreated>()
        .await
        .unwrap();
    let mut attached = browser
        .event_listener::<EventAttachedToTarget>()
        .await
        .unwrap();

    let url = format!("http://{address}/");
    let nav_browser = browser.clone();
    let nav_url = url.clone();
    let navigation = tokio::spawn(async move { nav_browser.new_page(nav_url).await });

    let parent_target_id = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = creations.next().await.unwrap();
            if event.target_info.r#type == "page" && event.target_info.url == url {
                return event.target_info.target_id.clone();
            }
        }
    })
    .await
    .expect("top-level target must be observed");
    let parent_target_info = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = attached.next().await.unwrap();
            if event.target_info.target_id == parent_target_id {
                return event.target_info.clone();
            }
        }
    })
    .await
    .expect("top-level target must attach");
    let page = navigation.await.unwrap().unwrap();
    let top_level = FrameContext::resolve_top_level(&browser, &parent_target_info)
        .await
        .expect("1: top-level FrameContext must resolve");

    // 1/2: genuine interactive challenge rendered; correct (OOPIF) frame
    // context resolved — Cloudflare serves the interactive test widget from
    // its own real origin, a genuine out-of-process child target.
    let child_attach = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let event = attached.next().await.unwrap();
            if event.target_info.r#type == "iframe" {
                return (*event).clone();
            }
        }
    })
    .await
    .expect("2: the Turnstile widget must attach a genuine child target");
    let frame = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Ok(context) =
                FrameContext::resolve_child(&browser, &top_level, &child_attach.target_info).await
            {
                return context;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("2: the Turnstile child FrameContext must resolve");

    // Let the widget finish laying out the interactive checkbox challenge
    // before capturing.
    tokio::time::sleep(Duration::from_secs(3)).await;

    // The paligemma-local runtime's qualified processor accepts only
    // one fixed input shape (224x224, identical on both the CPU/F32 and
    // CUDA/F16 backends — see
    // `paligemma_runtime::process_dynamic_image`); Turnstile's native
    // ~300x65 widget footprint does not smart-resize onto it. Growing this
    // fixture's own iframe first (see doc comment) makes the genuinely
    // rendered capture already the right shape.
    grow_widget_iframe_to_qualified_envelope(&top_level, &frame).await;

    // 3: frame-aware snapshot captured — the entire visible widget body is
    // the challenge surface; no target binding is needed for a
    // PointSelection form.
    //
    // HARDENED INSTRUMENTATION (SCORPION_PALIGEMMA_LIVE_UI_POINT_LOCALIZATION_QUALIFICATION_001):
    // every eprintln! below is diagnostic/provenance-only — it changes no
    // solving, materialization, revalidation, or action-dispatch behavior,
    // and logs no secret or token value. Its sole purpose is to make any
    // future real-acceptance failure classifiable without a follow-up
    // diagnostic frontier, unlike the run this instrumentation replaces.
    let challenge_backend_node_id = body_backend_node_id(&frame).await;
    let t_capture_start = Instant::now();
    let snapshot = BrowserChallengeSnapshot::capture_in_frame(
        &page,
        &top_level,
        &frame,
        challenge_backend_node_id,
        Vec::new(),
    )
    .await
    .expect("3: frame-aware snapshot must capture the rendered Turnstile widget");
    let t_capture_elapsed = t_capture_start.elapsed();
    eprintln!(
        "T_CAPTURE_SECS={:.3} captured_pixel_width={} captured_pixel_height={} \
         viewport_width={} viewport_height={} transform={:?} frame_owner={:?}",
        t_capture_elapsed.as_secs_f64(),
        snapshot.captured_pixel_width,
        snapshot.captured_pixel_height,
        snapshot.viewport_width,
        snapshot.viewport_height,
        snapshot.transform,
        frame.frame_owner,
    );

    // 4: the challenge maps truthfully onto the canonical PointSelection
    // form — a single click on the visible checkbox, nothing Turnstile- or
    // CAPTCHA-specific added to the canonical vocabulary. The instruction
    // is the same canonical concise noun-phrase label the frozen genuine
    // raster gate itself used (`SCORPION_PALIGEMMA_POINT_SELECTION_PROMPT_CONTRACT_CONVERGENCE_001`
    // / `SCORPION_CAPTCHA_REAL_WORLD_GROUNDING_MODEL_CANDIDATE_QUALIFICATION_001`)
    // — not a target-specific hint invented for this run: the long
    // task-orchestration sentence previously used here is exactly the
    // proven `PROMPT_CONTRACT_MISMATCH` root cause and is now rejected by
    // `solve_captcha`'s own canonical-label validation before ever
    // reaching a provider.
    let attempt = CaptchaBrowserAttempt {
        correlation_id: "real-turnstile".into(),
        selected_provider: CaptchaProviderId::PALIGEMMA_LOCAL,
        deadline: Duration::from_secs(1_800),
        challenge: CaptchaBrowserChallenge::PointSelection {
            instruction: "the outlined square".into(),
        },
    };

    // 5/6/7/8: real paligemma-local pinned CUDA/F16 inference (well inside
    // the frozen 55.03s latency budget) produces a CaptchaSolveOutcome; the
    // frame is revalidated and the exact canonical point action is
    // dispatched through the frame-aware seam.
    let t_seam_start = Instant::now();
    let outcome = execute_browser_captcha_attempt_in_frame(
        &page, &top_level, &frame, &snapshot, &registry, attempt,
    )
    .await;
    let t_seam_elapsed = t_seam_start.elapsed();
    // Logged unconditionally, on both success and failure: this is exactly
    // the gap SCORPION_TURNSTILE_POST_ACTION_PROGRESSION_DIAGNOSTIC_001 hit
    // — a failed run with zero recoverable coordinates. Never again.
    let (report_attempts, report_stage, report_actions_applied) = match &outcome {
        Ok(report) => (&report.attempts, report.stage, report.actions_applied),
        Err(failure) => (&failure.attempts, failure.stage, failure.actions_applied),
    };
    for recorded in report_attempts.recorded() {
        eprintln!(
            "MODEL_ATTEMPT provider={:?} outcome={:?}",
            recorded.provider, recorded.outcome
        );
        if let spider::features::captcha::CaptchaSolveOutcome::Solved {
            solution: spider::features::captcha::CaptchaSolution::Point { x, y },
            ..
        } = &recorded.outcome
        {
            eprintln!("MODEL_IMAGE_SPACE_POINT x={x} y={y}");
            match snapshot.transform.image_to_browser(*x, *y) {
                Ok(point) => eprintln!("TRANSFORMED_BROWSER_POINT={point:?}"),
                Err(e) => eprintln!("TRANSFORMED_BROWSER_POINT_ERROR={e:?}"),
            }
        }
    }
    eprintln!(
        "T_ACTION_SEAM_SECS={:.3} stage={report_stage:?} actions_applied={report_actions_applied}",
        t_seam_elapsed.as_secs_f64(),
    );
    let report = outcome.expect(
        "5-8: materialization, real inference, revalidation and the exact action must all succeed",
    );
    assert_eq!(report.actions_applied, 1);
    assert_eq!(report.stage, CaptchaBrowserExecutionStage::ActionApplied);

    // 9: observable genuine challenge progression — Cloudflare's own client
    // script invokes the widget's real `data-callback` with a token once
    // the (test) challenge is verified server-side.
    let t_progression_poll_start = Instant::now();
    let observed = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let value = page.evaluate("window.turnstileToken").await.unwrap();
            if let Some(token) = value.value().and_then(|v| v.as_str()) {
                return token.to_string();
            }
            let error = page.evaluate("window.turnstileError").await.unwrap();
            if let Some(code) = error.value() {
                panic!("Turnstile reported a client-side error instead of progressing: {code:?}");
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await;
    let t_progression_poll_elapsed = t_progression_poll_start.elapsed();
    // Length only, never the token value itself — diagnostic/provenance,
    // not a secret leak.
    eprintln!(
        "T_PROGRESSION_POLL_SECS={:.3} outcome={}",
        t_progression_poll_elapsed.as_secs_f64(),
        match &observed {
            Ok(token) => format!("token_observed len={}", token.len()),
            Err(_) => "timed_out".to_string(),
        }
    );
    let observed = observed
        .expect("9: observable Turnstile progression (success callback/token) must be detected");
    assert!(!observed.is_empty());

    page.close().await.unwrap();
    browser.close().await.unwrap();
    provider.unload();
}
