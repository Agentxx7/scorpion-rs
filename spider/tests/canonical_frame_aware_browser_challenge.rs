#![cfg(feature = "chrome")]
//! Genuine-Chromium acceptance for the frame-aware browser challenge
//! snapshot/action seam: canonical `FrameContext` composed with the
//! existing canonical browser-challenge primitive, proven against a real
//! top-level frame, a genuinely same-origin (in-process) child frame, and a
//! genuine out-of-process (OOPIF) child frame — never a second snapshot/
//! action implementation.

use spider::chromiumoxide::browser::{Browser, HeadlessMode};
use spider::chromiumoxide::cdp::browser_protocol::dom::{BackendNodeId, DescribeNodeParams};
use spider::chromiumoxide::cdp::browser_protocol::page::{FrameId, GetFrameTreeParams};
use spider::chromiumoxide::cdp::browser_protocol::target::{
    EventAttachedToTarget, EventTargetCreated,
};
use spider::chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
use spider::chromiumoxide::BrowserConfig;
use spider::features::browser_challenge::{
    BrowserChallengeAction, BrowserChallengeFailure, BrowserChallengeSnapshot,
    BrowserHorizontalDrag,
};
use spider::features::frame_context::FrameContext;
use spider::tokio_stream::StreamExt;
use std::time::Duration;

/// The exact challenge markup `browser_challenge_real.rs` already proves the
/// top-level primitive against, reused verbatim inside a child frame's
/// document so the same click/point/drag observable-state contract applies.
const CHALLENGE_HTML: &str = r#"<!doctype html><style>
  body{margin:0;height:600px} #challenge{position:absolute;left:40px;top:80px;width:240px;height:120px;background:#ddd}
  button{position:absolute;left:20px;top:15px;width:60px;height:35px}
</style><div id=challenge onclick="if(event.target===this)this.dataset.point='yes'">
  <button id=target onclick="this.dataset.clicked='yes'">pick</button>
</div><script>
  const c=document.querySelector('#challenge');
  c.addEventListener('mousedown',()=>c.dataset.down='yes');
  c.addEventListener('mouseup',()=>{if(c.dataset.down==='yes')c.dataset.dragged='yes'});
</script>"#;

/// The existing top-level primitive's `capture()` requires `page.frames()`
/// to report exactly one frame (see `browser_challenge.rs`, unchanged by
/// this frontier) — it was never meant to operate on a page that itself
/// embeds child frames. Criterion 1 ("top-level snapshot/action remains
/// PASS") therefore exercises it on its own dedicated, iframe-free page,
/// kept completely separate from the multi-frame fixture used for every
/// frame-aware criterion.
const PLAIN_TOP_LEVEL_HTML: &str = r#"<!doctype html><style>
  body{margin:0;height:600px}
  #top-challenge{position:absolute;left:10px;top:10px;width:100px;height:60px;background:#eee}
</style><div id=top-challenge onclick="this.dataset.point='yes'"></div>"#;

/// A minimal HTTP/1.1 server that routes by exact request path, serving
/// each configured route's body indefinitely for `duration` rather than an
/// exact request count (see `chromium-e2e-test-environment-quirks`: this
/// environment can race or drop an individual TCP handshake from the
/// sandboxed Chromium process, so an exact-count server can wedge a
/// caller's cleanup `.await` forever on a connection that never lands).
async fn serve_paths(
    listener: tokio::net::TcpListener,
    routes: &'static [(&'static str, &'static str)],
    duration: Duration,
) {
    let deadline = tokio::time::sleep(duration);
    tokio::pin!(deadline);
    loop {
        let stream = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(_) => continue,
            },
            _ = &mut deadline => return,
        };
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut stream = stream;
            let mut request = [0_u8; 2048];
            let read = stream.read(&mut request).await.unwrap_or(0);
            let request_line = String::from_utf8_lossy(&request[..read]);
            let path = request_line
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .to_string();
            let body = routes
                .iter()
                .find(|(route, _)| *route == path)
                .map(|(_, body)| *body)
                .unwrap_or("");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body
            );
            let _ = stream.write_all(response.as_bytes()).await;
        });
    }
}

async fn next_iframe_attach(
    attached: &mut spider::chromiumoxide::listeners::EventStream<EventAttachedToTarget>,
) -> EventAttachedToTarget {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let event = attached.next().await.unwrap();
            if event.target_info.r#type == "iframe" {
                return (*event).clone();
            }
        }
    })
    .await
    .unwrap()
}

async fn resolve_next_child(
    browser: &Browser,
    attached: &mut spider::chromiumoxide::listeners::EventStream<EventAttachedToTarget>,
    parent: &FrameContext,
) -> FrameContext {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let attach = next_iframe_attach(attached).await;
            if let Ok(context) =
                FrameContext::resolve_child(browser, parent, &attach.target_info).await
            {
                return context;
            }
        }
    })
    .await
    .expect("a resolvable child attach must eventually arrive")
}

/// Find a genuinely same-origin (in-process) child frame by its document
/// URL, searching `parent`'s own local frame tree — the only way to
/// discover one, since it never fires a `Target.attachedToTarget` event at
/// all (confirmed live; see `chromium-e2e-test-environment-quirks`).
async fn find_same_session_child_frame_id(parent: &FrameContext, url_suffix: &str) -> FrameId {
    fn search(
        tree: &spider::chromiumoxide::cdp::browser_protocol::page::FrameTree,
        suffix: &str,
    ) -> Option<FrameId> {
        if tree.frame.url.ends_with(suffix) {
            return Some(tree.frame.id.clone());
        }
        tree.child_frames
            .iter()
            .flatten()
            .find_map(|child| search(child, suffix))
    }
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let tree = parent
                .execute(GetFrameTreeParams::default())
                .await
                .unwrap()
                .result
                .frame_tree;
            if let Some(frame_id) = search(&tree, url_suffix) {
                return frame_id;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("same-session child frame must eventually appear in the local frame tree")
}

/// Resolve a `BackendNodeId` inside `context`'s exact session into an
/// authoritative identity — never a canonical identity in itself, only the
/// caller-owned lookup step that produces the value
/// [`BrowserChallengeSnapshot::capture_in_frame`] binds canonically.
///
/// Goes through `context`'s own `execution_context_id` (`Runtime.evaluate`
/// -> `DOM.describeNode` keyed by the resulting JS object id) rather than
/// `DOM.getDocument`/`DOM.querySelector` on the session's document root:
/// for a genuinely same-session (in-process) child frame, the session is
/// shared with its parent, so the session's DOM document root is the
/// *parent's* document — the child's own document is only reachable through
/// its distinct execution context, exactly the seam `FrameContext` exists to
/// prove. The identical recipe also works unchanged for top-level/OOPIF
/// contexts, whose execution context already *is* their document's.
async fn backend_node_id_for_selector(context: &FrameContext, selector: &str) -> BackendNodeId {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let remote_object = context
                .execute(
                    EvaluateParams::builder()
                        .expression(format!("document.querySelector({selector:?})"))
                        .context_id(context.execution_context_id)
                        .build()
                        .unwrap(),
                )
                .await
                .unwrap()
                .result
                .result;
            if let Some(object_id) = remote_object.object_id.clone() {
                return context
                    .execute(DescribeNodeParams {
                        object_id: Some(object_id),
                        ..Default::default()
                    })
                    .await
                    .unwrap()
                    .result
                    .node
                    .backend_node_id;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("selector must eventually resolve in the document")
}

/// Evaluate a JS expression inside `context`'s exact execution context and
/// return its JSON value.
/// Insert a brand-new `<iframe id=id src=src>` into the top-level document
/// and resolve its genuine OOPIF `FrameContext` — a fresh, uniquely
/// identified DOM node each call, never a reused/replaced element, so each
/// lifecycle-invalidation scenario gets its own independent, unambiguous
/// child to mutate.
async fn insert_oopif_iframe(
    page: &spider::chromiumoxide::Page,
    browser: &Browser,
    attached: &mut spider::chromiumoxide::listeners::EventStream<EventAttachedToTarget>,
    top_level: &FrameContext,
    id: &str,
    src: &str,
) -> FrameContext {
    page.evaluate(format!(
        "(() => {{ const f=document.createElement('iframe'); f.id={id:?}; f.src={src:?}; \
         f.style.cssText='position:absolute;width:300px;height:220px;border:0;left:150px;top:500px'; \
         document.body.appendChild(f); }})()"
    ))
    .await
    .unwrap();
    resolve_next_child(browser, attached, top_level).await
}

async fn evaluate_in(context: &FrameContext, expression: &str) -> serde_json::Value {
    context
        .execute(
            EvaluateParams::builder()
                .expression(expression)
                .context_id(context.execution_context_id)
                .return_by_value(true)
                .build()
                .unwrap(),
        )
        .await
        .unwrap()
        .result
        .result
        .value
        .unwrap_or(serde_json::Value::Null)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn canonical_frame_aware_browser_challenge_snapshot_and_action() {
    // Genuinely distinct Chromium origins/sites, all reached by name rather
    // than IP literal (see chromium-e2e-test-environment-quirks): parent on
    // 127.0.0.1 (also serving the same-origin child, at a different path on
    // the *identical* origin), the OOPIF child on `localhost`, and its
    // eventual replacement (criteria 10-13) on the genuinely fresh
    // `ip6-localhost` — reusing the same origin for the replacement was
    // observed live to sometimes stay in-process instead of forcing a fresh
    // OOPIF target (a Chromium process-reuse heuristic).
    let oopif_listener = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
    let oopif_port = oopif_listener.local_addr().unwrap().port();
    let _oopif_server = tokio::spawn(serve_paths(
        oopif_listener,
        &[("/", CHALLENGE_HTML)],
        Duration::from_secs(90),
    ));

    let replacement_listener = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
    let replacement_port = replacement_listener.local_addr().unwrap().port();
    let _replacement_server = tokio::spawn(serve_paths(
        replacement_listener,
        &[("/", CHALLENGE_HTML)],
        Duration::from_secs(90),
    ));

    let parent_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let parent_port = parent_listener.local_addr().unwrap().port();
    let parent_html: &'static str = Box::leak(
        format!(
            "<!doctype html><style>\
             body{{margin:0;height:600px}}\
             #top-challenge{{position:absolute;left:10px;top:10px;width:100px;height:60px;background:#eee}}\
             iframe{{position:absolute;width:300px;height:220px;border:0}}\
             #same{{left:150px;top:10px}}\
             #oopif{{left:150px;top:260px}}\
             </style>\
             <div id=top-challenge onclick=\"this.dataset.point='yes'\"></div>\
             <iframe id=same src='/same-origin-child'></iframe>\
             <iframe id=oopif src='http://localhost:{oopif_port}/'></iframe>"
        )
        .into_boxed_str(),
    );
    let _parent_server = tokio::spawn(serve_paths(
        parent_listener,
        Box::leak(Box::new([
            ("/", parent_html),
            ("/same-origin-child", CHALLENGE_HTML),
            ("/plain", PLAIN_TOP_LEVEL_HTML),
        ])),
        Duration::from_secs(90),
    ));

    let profile = std::env::temp_dir().join(format!(
        "scorpion-frame-browser-challenge-{}",
        std::process::id()
    ));
    let isolate_origins = format!(
        "--isolate-origins=http://localhost:{oopif_port},http://ip6-localhost:{replacement_port}"
    );
    let config = BrowserConfig::builder()
        .user_data_dir(profile)
        .chrome_executable("/usr/bin/chromium")
        .headless_mode(HeadlessMode::New)
        .incognito()
        .arg("--no-sandbox")
        .arg("--site-per-process")
        .arg(isolate_origins)
        .arg("--disable-background-networking")
        .launch_timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let (browser, mut handler) = Browser::launch(config).await.unwrap();
    let _handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });
    let browser = std::sync::Arc::new(browser);

    // --- 1: top-level snapshot/action remains PASS, through the exact
    // unmodified `capture`/`revalidate`/`apply` primitive, unmixed with any
    // frame-aware concern. The existing primitive's `capture()` requires
    // `page.frames()` to report exactly one frame (unchanged production
    // behavior, not something this frontier may touch), so this proof runs
    // on its own dedicated iframe-free page rather than the multi-frame
    // fixture used below.
    let plain_url = format!("http://127.0.0.1:{parent_port}/plain");
    let plain_page = browser.new_page(plain_url).await.unwrap();
    let top_element = plain_page.find_element("#top-challenge").await.unwrap();
    let top_snapshot = BrowserChallengeSnapshot::capture(&plain_page, top_element, Vec::new())
        .await
        .unwrap();
    top_snapshot
        .apply(
            &plain_page,
            BrowserChallengeAction::ExactPoint {
                x: f64::from(top_snapshot.captured_pixel_width) / 2.0,
                y: f64::from(top_snapshot.captured_pixel_height) / 2.0,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        plain_page
            .evaluate("document.querySelector('#top-challenge').dataset.point")
            .await
            .unwrap()
            .value()
            .cloned(),
        Some(serde_json::json!("yes"))
    );
    plain_page.close().await.unwrap();

    let mut creations = browser
        .event_listener::<EventTargetCreated>()
        .await
        .unwrap();
    let mut attached = browser
        .event_listener::<EventAttachedToTarget>()
        .await
        .unwrap();

    let parent_url = format!("http://127.0.0.1:{parent_port}/");
    let nav_browser = browser.clone();
    let nav_url = parent_url.clone();
    let navigation = tokio::spawn(async move { nav_browser.new_page(nav_url).await });

    let parent_target_id = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = creations.next().await.unwrap();
            if event.target_info.r#type == "page" && event.target_info.url == parent_url {
                return event.target_info.target_id.clone();
            }
        }
    })
    .await
    .unwrap();
    let parent_target_info = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = attached.next().await.unwrap();
            if event.target_info.target_id == parent_target_id {
                return event.target_info.clone();
            }
        }
    })
    .await
    .unwrap();
    let page = navigation.await.unwrap().unwrap();

    let top_level = FrameContext::resolve_top_level(&browser, &parent_target_info)
        .await
        .unwrap();

    // --- same-origin (in-process) child: discovered through the local
    // frame tree, never through an attach event (there is none). ---
    let same_origin_frame_id =
        find_same_session_child_frame_id(&top_level, "/same-origin-child").await;
    let same_origin =
        FrameContext::resolve_same_session_child(&browser, &top_level, same_origin_frame_id)
            .await
            .unwrap();

    // --- 2: same-origin child snapshot/action PASS. ---
    let same_challenge = backend_node_id_for_selector(&same_origin, "#challenge").await;
    let same_target = backend_node_id_for_selector(&same_origin, "#target").await;
    let same_snapshot = BrowserChallengeSnapshot::capture_in_frame(
        &page,
        &top_level,
        &same_origin,
        same_challenge,
        vec![("target".into(), same_target)],
    )
    .await
    .unwrap();
    same_snapshot
        .apply_in_frame(
            &page,
            &top_level,
            &same_origin,
            BrowserChallengeAction::ExactTargetClick {
                stable_id: "target".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        evaluate_in(
            &same_origin,
            "document.querySelector('#target').dataset.clicked"
        )
        .await,
        serde_json::json!("yes")
    );

    // --- genuine OOPIF child. ---
    let oopif = resolve_next_child(&browser, &mut attached, &top_level).await;
    assert_ne!(oopif.target_id, top_level.target_id);
    assert_ne!(oopif.session_id, top_level.session_id);

    // --- 3/4/5/6: exact child FrameContext and frame-owner geometry
    // retained; image -> child -> parent -> top-level transform proven by a
    // successful capture at all. ---
    let oopif_challenge = backend_node_id_for_selector(&oopif, "#challenge").await;
    let oopif_target = backend_node_id_for_selector(&oopif, "#target").await;
    let click_snapshot = BrowserChallengeSnapshot::capture_in_frame(
        &page,
        &top_level,
        &oopif,
        oopif_challenge,
        vec![("target".into(), oopif_target)],
    )
    .await
    .unwrap();

    // --- 7: exact click inside OOPIF changes observable DOM state. ---
    click_snapshot
        .apply_in_frame(
            &page,
            &top_level,
            &oopif,
            BrowserChallengeAction::ExactTargetClick {
                stable_id: "target".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        evaluate_in(&oopif, "document.querySelector('#target').dataset.clicked").await,
        serde_json::json!("yes")
    );

    // --- 8: exact point action inside OOPIF changes observable DOM state.
    // A fresh capture is required: the click above mutated the challenge
    // element's own outerHTML (the button inside it now carries
    // data-clicked), which is exactly the content-identity nonce
    // `bind_object`/`inspect_object` are supposed to catch — mirrors
    // `browser_challenge_real.rs`'s own established multi-action pattern. ---
    let oopif_challenge_2 = backend_node_id_for_selector(&oopif, "#challenge").await;
    let point_snapshot = BrowserChallengeSnapshot::capture_in_frame(
        &page,
        &top_level,
        &oopif,
        oopif_challenge_2,
        Vec::new(),
    )
    .await
    .unwrap();
    point_snapshot
        .apply_in_frame(
            &page,
            &top_level,
            &oopif,
            BrowserChallengeAction::ExactPoint {
                x: f64::from(point_snapshot.captured_pixel_width) * 0.75,
                y: f64::from(point_snapshot.captured_pixel_height) * 0.75,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        evaluate_in(&oopif, "document.querySelector('#challenge').dataset.point").await,
        serde_json::json!("yes")
    );

    // --- 9: exact horizontal drag inside OOPIF changes observable DOM state. ---
    let oopif_challenge_3 = backend_node_id_for_selector(&oopif, "#challenge").await;
    let drag_snapshot = BrowserChallengeSnapshot::capture_in_frame(
        &page,
        &top_level,
        &oopif,
        oopif_challenge_3,
        Vec::new(),
    )
    .await
    .unwrap();
    drag_snapshot
        .apply_in_frame(
            &page,
            &top_level,
            &oopif,
            BrowserChallengeAction::ExactHorizontalDrag(BrowserHorizontalDrag {
                start_x: 120.0,
                start_y: 60.0,
                end_x: 180.0,
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        evaluate_in(
            &oopif,
            "document.querySelector('#challenge').dataset.dragged"
        )
        .await,
        serde_json::json!("yes")
    );

    // --- 16/17: out-of-bounds point/drag fail closed. ---
    let oopif_challenge_4 = backend_node_id_for_selector(&oopif, "#challenge").await;
    let bounds_snapshot = BrowserChallengeSnapshot::capture_in_frame(
        &page,
        &top_level,
        &oopif,
        oopif_challenge_4,
        Vec::new(),
    )
    .await
    .unwrap();
    assert!(matches!(
        bounds_snapshot
            .apply_in_frame(
                &page,
                &top_level,
                &oopif,
                BrowserChallengeAction::ExactPoint { x: -1.0, y: 0.0 },
            )
            .await,
        Err(BrowserChallengeFailure::PointOutOfBounds)
    ));
    assert!(matches!(
        bounds_snapshot
            .apply_in_frame(
                &page,
                &top_level,
                &oopif,
                BrowserChallengeAction::ExactHorizontalDrag(BrowserHorizontalDrag {
                    start_x: 0.0,
                    start_y: 0.0,
                    end_x: f64::from(bounds_snapshot.captured_pixel_width) + 10.0,
                }),
            )
            .await,
        Err(BrowserChallengeFailure::DragOutOfBounds)
    ));

    // --- 10: child navigation before action produces zero actions. A fresh,
    // uniquely identified OOPIF child, reloaded from *within* its own
    // execution context (new loader id, same target/session) after capture. ---
    let nav_child = insert_oopif_iframe(
        &page,
        &browser,
        &mut attached,
        &top_level,
        "nav-child",
        &format!("http://localhost:{oopif_port}/"),
    )
    .await;
    let nav_challenge = backend_node_id_for_selector(&nav_child, "#challenge").await;
    let nav_snapshot = BrowserChallengeSnapshot::capture_in_frame(
        &page,
        &top_level,
        &nav_child,
        nav_challenge,
        Vec::new(),
    )
    .await
    .unwrap();
    let _ = nav_child
        .execute(
            EvaluateParams::builder()
                .expression("location.reload()")
                .context_id(nav_child.execution_context_id)
                .build()
                .unwrap(),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        nav_snapshot
            .apply_in_frame(
                &page,
                &top_level,
                &nav_child,
                BrowserChallengeAction::ExactPoint { x: 1.0, y: 1.0 },
            )
            .await
            .is_err(),
        "a navigated child must produce zero actions"
    );

    // --- 11/18: OOPIF target replacement before action produces zero
    // actions — even though the replacement iframe serves the identical
    // markup (identical selectors/geometry/content), only a different
    // origin/path, so the *only* thing distinguishing it from the original
    // is canonical identity, exactly what must be proven authoritative. ---
    let replace_child = insert_oopif_iframe(
        &page,
        &browser,
        &mut attached,
        &top_level,
        "target-replace-child",
        &format!("http://localhost:{oopif_port}/"),
    )
    .await;
    let replace_challenge = backend_node_id_for_selector(&replace_child, "#challenge").await;
    let replace_snapshot = BrowserChallengeSnapshot::capture_in_frame(
        &page,
        &top_level,
        &replace_child,
        replace_challenge,
        Vec::new(),
    )
    .await
    .unwrap();
    page.evaluate(format!(
        "document.getElementById('target-replace-child').src={:?}",
        format!("http://ip6-localhost:{replacement_port}/")
    ))
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        replace_snapshot
            .apply_in_frame(
                &page,
                &top_level,
                &replace_child,
                BrowserChallengeAction::ExactPoint { x: 1.0, y: 1.0 },
            )
            .await
            .is_err(),
        "a replaced OOPIF target must produce zero actions, even when the \
         replacement frame's content is selector/geometry-identical"
    );

    // --- 12: session-context replacement (the frame's own owner removed
    // from the top-level document out from under the snapshot, tearing down
    // its session/target through Chromium's normal frame-lifecycle path —
    // not a forced `Target.closeTarget`, which was observed live to
    // destabilize the shared CDP connection's own command channel for every
    // later scenario) before action produces zero actions. ---
    let session_child = insert_oopif_iframe(
        &page,
        &browser,
        &mut attached,
        &top_level,
        "session-replace-child",
        &format!("http://localhost:{oopif_port}/"),
    )
    .await;
    let session_challenge = backend_node_id_for_selector(&session_child, "#challenge").await;
    let session_snapshot = BrowserChallengeSnapshot::capture_in_frame(
        &page,
        &top_level,
        &session_child,
        session_challenge,
        Vec::new(),
    )
    .await
    .unwrap();
    page.evaluate("document.getElementById('session-replace-child').remove()")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        session_snapshot
            .apply_in_frame(
                &page,
                &top_level,
                &session_child,
                BrowserChallengeAction::ExactPoint { x: 1.0, y: 1.0 },
            )
            .await
            .is_err(),
        "a closed session/target must produce zero actions"
    );

    // --- 13: frame-owner replacement (the <iframe> element itself swapped
    // for a fresh, otherwise-identical node) before action produces zero
    // actions. ---
    let owner_child = insert_oopif_iframe(
        &page,
        &browser,
        &mut attached,
        &top_level,
        "owner-replace-child",
        &format!("http://localhost:{oopif_port}/"),
    )
    .await;
    let owner_challenge = backend_node_id_for_selector(&owner_child, "#challenge").await;
    let owner_snapshot = BrowserChallengeSnapshot::capture_in_frame(
        &page,
        &top_level,
        &owner_child,
        owner_challenge,
        Vec::new(),
    )
    .await
    .unwrap();
    page.evaluate(
        "(() => { const o=document.getElementById('owner-replace-child'); \
         const c=o.cloneNode(); o.replaceWith(c); })()",
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        owner_snapshot
            .apply_in_frame(
                &page,
                &top_level,
                &owner_child,
                BrowserChallengeAction::ExactPoint { x: 1.0, y: 1.0 },
            )
            .await
            .is_err(),
        "a replaced frame-owner element must produce zero actions"
    );

    // --- 14: inner target replacement (the captured target node itself
    // swapped for a clone, challenge untouched) before action produces zero
    // actions. ---
    let inner_child = insert_oopif_iframe(
        &page,
        &browser,
        &mut attached,
        &top_level,
        "inner-replace-child",
        &format!("http://localhost:{oopif_port}/"),
    )
    .await;
    let inner_challenge = backend_node_id_for_selector(&inner_child, "#challenge").await;
    let inner_target = backend_node_id_for_selector(&inner_child, "#target").await;
    let inner_snapshot = BrowserChallengeSnapshot::capture_in_frame(
        &page,
        &top_level,
        &inner_child,
        inner_challenge,
        vec![("target".into(), inner_target)],
    )
    .await
    .unwrap();
    inner_child
        .execute(
            EvaluateParams::builder()
                .expression(
                    "(() => { const t=document.querySelector('#target'); \
                     t.replaceWith(t.cloneNode(true)); })()",
                )
                .context_id(inner_child.execution_context_id)
                .build()
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        inner_snapshot
            .apply_in_frame(
                &page,
                &top_level,
                &inner_child,
                BrowserChallengeAction::ExactTargetClick {
                    stable_id: "target".into()
                },
            )
            .await
            .is_err(),
        "a replaced inner target must produce zero actions"
    );

    // --- 15: geometry mutation (the captured challenge resized) before
    // action produces zero actions. ---
    let geometry_child = insert_oopif_iframe(
        &page,
        &browser,
        &mut attached,
        &top_level,
        "geometry-mutate-child",
        &format!("http://localhost:{oopif_port}/"),
    )
    .await;
    let geometry_challenge = backend_node_id_for_selector(&geometry_child, "#challenge").await;
    let geometry_snapshot = BrowserChallengeSnapshot::capture_in_frame(
        &page,
        &top_level,
        &geometry_child,
        geometry_challenge,
        Vec::new(),
    )
    .await
    .unwrap();
    geometry_child
        .execute(
            EvaluateParams::builder()
                .expression("document.querySelector('#challenge').style.width='400px'")
                .context_id(geometry_child.execution_context_id)
                .build()
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        geometry_snapshot
            .apply_in_frame(
                &page,
                &top_level,
                &geometry_child,
                BrowserChallengeAction::ExactPoint { x: 1.0, y: 1.0 },
            )
            .await
            .is_err(),
        "a resized challenge must produce zero actions"
    );
}
