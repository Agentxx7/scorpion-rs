#![cfg(feature = "chrome")]
//! Genuine-Chromium acceptance for the canonical frame-context identity seam
//! (`spider::features::frame_context`), proving the full
//! `FrameId -> TargetId -> SessionId -> ExecutionContextId -> DOM identity ->
//! frame owner -> lifecycle -> revalidation` chain against a controlled
//! `--site-per-process` OOPIF fixture. No selector, URL, origin or geometry
//! ever substitutes for canonical identity here.

use spider::chromiumoxide::browser::{Browser, HeadlessMode};
use spider::chromiumoxide::cdp::browser_protocol::dom::{
    DescribeNodeParams, GetDocumentParams, GetFrameOwnerParams, QuerySelectorParams,
};
use spider::chromiumoxide::cdp::browser_protocol::target::{
    EventAttachedToTarget, EventDetachedFromTarget, EventTargetCreated, TargetInfo,
};
use spider::chromiumoxide::BrowserConfig;
use spider::features::frame_context::{FrameClassification, FrameContext, FrameContextFailure};
use spider::tokio_stream::StreamExt;
use std::time::Duration;

/// Serve `body` to every connection for `duration`, rather than an exact
/// request count. This environment was observed (live) to occasionally drop
/// or race an individual TCP handshake from the sandboxed Chromium process;
/// serving for a bounded window instead absorbs any such extra or retried
/// connection attempt instead of running out of accepts, and the task
/// self-terminates either way rather than blocking a caller's `.await`
/// forever on a connection that never arrives.
async fn serve(listener: tokio::net::TcpListener, body: &'static str, duration: Duration) {
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
            let _ = stream.read(&mut request).await;
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

/// Resolve the child `FrameContext` for the next iframe-typed attach event
/// that genuinely proves ownership under `parent`. A dynamically inserted
/// iframe can fire more than one `Target.attachedToTarget` event for the
/// same logical frame — e.g. a transient same-process placeholder before
/// Chromium swaps it out to a genuine OOPIF target — and only the final one
/// is a live target `DOM.getFrameOwner` can corroborate. This loops over
/// observed attach events (never re-deriving identity any other way) until
/// one resolves, rather than trusting the first.
async fn resolve_next_child(
    browser: &spider::chromiumoxide::browser::Browser,
    attached: &mut spider::chromiumoxide::listeners::EventStream<EventAttachedToTarget>,
    parent: &FrameContext,
) -> (EventAttachedToTarget, FrameContext) {
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let attach = next_iframe_attach(attached).await;
            if let Ok(context) =
                FrameContext::resolve_child(browser, parent, &attach.target_info).await
            {
                return (attach, context);
            }
        }
    })
    .await
    .expect("a resolvable child attach must eventually arrive")
}

/// Resolve every child whose committed port is in `expected_ports`,
/// regardless of the order their `Target.attachedToTarget` events actually
/// arrive in (observed live: concurrently created sibling OOPIF targets do
/// not attach in a fixed order). Each candidate's own committed URL — not
/// event-arrival order — decides which expected port it satisfies.
async fn resolve_children_by_port(
    browser: &spider::chromiumoxide::browser::Browser,
    attached: &mut spider::chromiumoxide::listeners::EventStream<EventAttachedToTarget>,
    parent: &FrameContext,
    expected_ports: &[u16],
) -> std::collections::HashMap<u16, (EventAttachedToTarget, FrameContext)> {
    let mut found = std::collections::HashMap::new();
    while found.len() < expected_ports.len() {
        let (attach, context) = resolve_next_child(browser, attached, parent).await;
        let target_info = browser
            .execute(
                spider::chromiumoxide::cdp::browser_protocol::target::GetTargetInfoParams::builder(
                )
                .target_id(context.target_id.clone())
                .build(),
            )
            .await
            .unwrap()
            .result
            .target_info;
        let port = expected_ports
            .iter()
            .find(|port| target_info.url.contains(&format!(":{port}")));
        if let Some(&port) = port {
            found.insert(port, (attach, context));
        }
    }
    found
}

/// Resolve a `NodeId` inside `session` into an authoritative `BackendNodeId`
/// through DOM commands alone — never a canonical identity in itself, only
/// the caller-owned lookup step that produces the value handed to
/// `FrameContext::resolve_dom_identity`.
async fn backend_node_id_for_selector(
    context: &FrameContext,
    selector: &str,
) -> spider::chromiumoxide::cdp::browser_protocol::dom::BackendNodeId {
    // Test-only robustness: the child document may still be loading
    // immediately after attach, before `selector` exists in its DOM. This
    // polling is a fixture concern, not part of the canonical seam, which
    // never re-queries a selector once identity is bound.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let document = context
                .execute(GetDocumentParams::default())
                .await
                .unwrap()
                .result;
            let node = context
                .execute(QuerySelectorParams::new(document.root.node_id, selector))
                .await
                .unwrap()
                .result;
            if *node.node_id.inner() != 0 {
                let backend_node_id = context
                    .execute(DescribeNodeParams {
                        node_id: Some(node.node_id),
                        ..Default::default()
                    })
                    .await
                    .unwrap()
                    .result
                    .node
                    .backend_node_id;
                return backend_node_id;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("selector must eventually resolve in the child document")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn canonical_oopif_frame_context_identity_and_lifecycle() {
    // Three genuinely distinct Chromium "sites" under `--site-per-process`.
    // Site classification for single-label hosts is by exact host string
    // (all three reach the same `::1`/`127.0.0.1` loopback interfaces, but
    // each is a different site): parent on `127.0.0.1`, child A on
    // `localhost`, child B on `ip6-localhost` — both `/etc/hosts` aliases for
    // `::1`, matching the already-closed prerequisite chromey OOPIF
    // acceptance fixture's proven-reliable scheme. Other loopback aliases
    // (`127.0.0.2/3`, `127.0.1.1`, the bare IPv6 literal `[::1]`) were tried
    // and intermittently produced genuine Chromium connection failures in
    // this environment (`chrome-error://chromewebdata/`, confirmed live)
    // even though these `/etc/hosts` names reaching the identical addresses
    // did not — an environment networking limitation, not a reason to weaken
    // the acceptance bar. `--isolate-origins` with same-host-different-port
    // IPv4 aliases was also tried and did not actually separate targets in
    // this environment (confirmed live: `page.frames()` showed ordinary
    // same-target child frames, never an `attachedToTarget` event for them).
    // Because sibling OOPIF targets do not necessarily attach in creation
    // order (confirmed live), children are matched to their expected origin
    // by committed port via `resolve_children_by_port`, never by arrival
    // order.
    let child_a_listener = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
    let child_a_port = child_a_listener.local_addr().unwrap().port();
    let child_a_server = tokio::spawn(serve(
        child_a_listener,
        "<button id=inside>child-a</button>",
        Duration::from_secs(60),
    ));
    // A brand-new, never-before-navigated origin for the later "remove and
    // recreate the iframe" scenario: reusing child A's exact origin was
    // observed (live) to sometimes stay in-process on the second attach
    // instead of forcing a fresh OOPIF target — a Chromium process-reuse
    // heuristic for a just-freed site instance, not something this fixture
    // controls. A genuinely fresh origin sidesteps that reuse path entirely.
    let replacement_listener = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
    let replacement_port = replacement_listener.local_addr().unwrap().port();
    let replacement_server = tokio::spawn(serve(
        replacement_listener,
        "<button id=inside>child-a-replacement</button>",
        Duration::from_secs(60),
    ));

    let child_b_listener = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
    let child_b_port = child_b_listener.local_addr().unwrap().port();
    let child_b_server = tokio::spawn(serve(
        child_b_listener,
        "<p>child-b</p>",
        Duration::from_secs(60),
    ));

    let parent_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let parent_port = parent_listener.local_addr().unwrap().port();
    let parent_html = Box::leak(
        format!(
            "<iframe id=frame src='http://localhost:{child_a_port}/'></iframe>\
             <iframe id=frame2 src='http://ip6-localhost:{child_b_port}/'></iframe>"
        )
        .into_boxed_str(),
    );
    let parent_server = tokio::spawn(serve(parent_listener, parent_html, Duration::from_secs(60)));

    let profile =
        std::env::temp_dir().join(format!("scorpion-frame-context-{}", std::process::id()));
    let config = BrowserConfig::builder()
        .user_data_dir(profile)
        .chrome_executable("/usr/bin/chromium")
        .headless_mode(HeadlessMode::New)
        .incognito()
        .arg("--no-sandbox")
        .arg("--site-per-process")
        // Explicit, permanent force-isolation for child A's origin: relying
        // on `--site-per-process`'s heuristic alone was observed (live) to
        // stop isolating a *second* same-origin iframe once the first one's
        // site instance had already been freed (the replacement then stayed
        // in-process instead of attaching as a new OOPIF target).
        .arg(format!(
            "--isolate-origins=http://localhost:{child_a_port},http://ip6-localhost:{child_b_port},http://ip6-localhost:{replacement_port}"
        ))
        // Chromium's own default new-tab page can fetch a live
        // `chrome-untrusted://new-tab-page` "one-google-bar" iframe over the
        // network; this browser-wide fixture listens for every attach event,
        // so that noise must not exist rather than be filtered after the
        // fact — `--incognito` plus `--disable-background-networking` is the
        // standard combination automation tooling uses to suppress it.
        .arg("--disable-background-networking")
        .launch_timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let (browser, mut handler) = Browser::launch(config).await.unwrap();
    let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });
    let browser = std::sync::Arc::new(browser);

    let mut creations = browser
        .event_listener::<EventTargetCreated>()
        .await
        .unwrap();
    let mut attached = browser
        .event_listener::<EventAttachedToTarget>()
        .await
        .unwrap();
    let mut detached = browser
        .event_listener::<EventDetachedFromTarget>()
        .await
        .unwrap();

    let parent_url = format!("http://127.0.0.1:{parent_port}/");
    let navigation_browser = browser.clone();
    let navigation_url = parent_url.clone();
    let navigation = tokio::spawn(async move { navigation_browser.new_page(navigation_url).await });

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
    // Wait for the parent's real navigation to actually commit before
    // resolving its canonical context: the target attaches (and its initial
    // "about:blank" document briefly exists) before the real navigation to
    // `parent_url` lands, and Chromium can assign the committed document a
    // different frame id than that initial placeholder.
    let page = navigation.await.unwrap().unwrap();
    // Sanity: both iframe elements really are in the parsed DOM before any
    // frame-context resolution begins.
    assert_eq!(
        page.evaluate("document.querySelectorAll('iframe').length")
            .await
            .unwrap()
            .value(),
        Some(&serde_json::Value::from(2))
    );

    // --- 1: top-level canonical context resolves through the same seam ---
    let top_level = FrameContext::resolve_top_level(&browser, &parent_target_info)
        .await
        .unwrap();
    assert_eq!(top_level.classification, FrameClassification::TopLevel);
    assert_eq!(top_level.target_id, parent_target_id);

    // --- 1/2/3: child FrameId, OOPIF TargetId and attached SessionId ---
    // Both siblings are resolved up front, matched to their expected origin
    // by committed port rather than event-arrival order.
    let mut children = resolve_children_by_port(
        &browser,
        &mut attached,
        &top_level,
        &[child_a_port, child_b_port],
    )
    .await;
    let (child_a_attach, child_a) = children.remove(&child_a_port).unwrap();
    let (child_b_attach, child_b) = children.remove(&child_b_port).unwrap();
    let child_a_target_id = child_a_attach.target_info.target_id.clone();
    assert_eq!(child_a.classification, FrameClassification::Oopif);
    assert_eq!(child_a.target_id, child_a_target_id);
    assert_eq!(child_a.session_id, child_a_attach.session_id);
    assert_eq!(child_a.parent_frame_id.as_ref(), Some(&top_level.frame_id));
    let original_url = browser
        .execute(
            spider::chromiumoxide::cdp::browser_protocol::target::GetTargetInfoParams::builder()
                .target_id(child_a.target_id.clone())
                .build(),
        )
        .await
        .unwrap()
        .result
        .target_info
        .url;

    // Note: chromiumoxide's own `FrameManager` (backing `page.frames()` /
    // `page.frame_parent()`) is fed only by the *parent* session's `Page`
    // domain events. Once a frame becomes a genuine OOPIF, Chromium reports
    // its lifecycle on the *child* target's own `Page` domain instead, so
    // that parent-side bookkeeping does not observe it — confirmed live
    // against this fixture. It is therefore not an available independent
    // oracle for OOPIF frames; the association proof this module relies on
    // (`TargetInfo.parent_frame_id` plus `DOM.getFrameOwner` executed
    // through the parent's own session, both checked in `FrameContext::resolve`)
    // is the authoritative one.

    // --- 4: exact child execution context resolved (not the first found) ---
    let child_eval = child_a
        .execute(
            spider::chromiumoxide::cdp::js_protocol::runtime::EvaluateParams::builder()
                .expression("document.body.dataset.owner='child-a';document.body.dataset.owner")
                .context_id(child_a.execution_context_id)
                .build()
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        child_eval
            .result
            .result
            .value
            .as_ref()
            .and_then(|value| value.as_str()),
        Some("child-a")
    );
    // The top-level document was never touched by that evaluation.
    assert_eq!(
        page.evaluate("document.body.dataset.owner")
            .await
            .unwrap()
            .value(),
        None
    );

    // --- 5: frame-scoped DOM/backend-node identity through the child session ---
    let backend_node_id = backend_node_id_for_selector(&child_a, "#inside").await;
    let dom_identity = child_a.resolve_dom_identity(backend_node_id).await.unwrap();

    // --- 6: parent frame-owner identity ---
    let frame_owner = child_a
        .frame_owner
        .clone()
        .expect("oopif has a frame owner");
    let independently_queried_owner = top_level
        .execute(GetFrameOwnerParams::new(child_a.frame_id.clone()))
        .await
        .unwrap()
        .result
        .backend_node_id;
    assert_eq!(frame_owner.backend_node_id, independently_queried_owner);
    assert_eq!(frame_owner.owner_target_id, top_level.target_id);
    assert_eq!(frame_owner.owner_session_id, top_level.session_id);

    // --- 7: unchanged context revalidates PASS ---
    child_a.revalidate(Some(&top_level)).await.unwrap();
    top_level.revalidate(None).await.unwrap();
    child_a.revalidate_dom_identity(dom_identity).await.unwrap();

    // --- 16: ambiguous association fails closed, using genuinely observed facts ---
    let both_candidates: Vec<TargetInfo> = vec![
        child_a_attach.target_info.clone(),
        child_b_attach.target_info.clone(),
    ];
    assert!(matches!(
        spider::features::frame_context::select_unique_child_target(
            &top_level.frame_id,
            &both_candidates,
        ),
        Err(FrameContextFailure::FrameTargetAssociationAmbiguous)
    ));
    assert_ne!(child_b.target_id, child_a.target_id);
    assert_ne!(child_b.frame_id, child_a.frame_id);

    // Record geometry now: the replacement iframe below keeps identical
    // geometry, proving revalidation never depends on it (criterion 15).
    const FRAME_GEOMETRY_JS: &str = "(()=>{const r=document.querySelector('#frame').getBoundingClientRect();return {width:r.width,height:r.height};})()";
    let geometry_before = page
        .evaluate(FRAME_GEOMETRY_JS)
        .await
        .unwrap()
        .value()
        .cloned()
        .unwrap();

    // --- 10/11/12/13/14/15: detach, target/session replacement, frame-owner
    // replacement, selector- and origin/geometry-independence, all from one
    // genuine remove-then-recreate sequence. ---
    page.evaluate("document.querySelector('#frame').remove()")
        .await
        .unwrap();
    let detached_event = tokio::time::timeout(Duration::from_secs(10), detached.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detached_event.session_id, child_a.session_id);
    assert!(matches!(
        child_a.revalidate(Some(&top_level)).await,
        Err(FrameContextFailure::FrameDetached) | Err(FrameContextFailure::TargetDetached)
    ));

    page.evaluate(format!(
        "document.body.insertAdjacentHTML('beforeend', \
         `<iframe id=frame src='http://ip6-localhost:{replacement_port}/'></iframe>`);"
    ))
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    // The replacement stays invisible to the parent-side `FrameManager` too
    // (same reasoning as the note above): still only the main frame.
    assert_eq!(page.frames().await.unwrap().len(), 1);
    let (replacement_attach, replacement) =
        resolve_next_child(&browser, &mut attached, &top_level).await;
    assert_ne!(replacement_attach.session_id, child_a.session_id);

    // 11: genuinely different target/session identity, same selector/id.
    assert_ne!(replacement.target_id, child_a.target_id);
    assert_ne!(replacement.session_id, child_a.session_id);
    // 12: a genuinely different frame-owner backend-node identity.
    assert_ne!(
        replacement.frame_owner.as_ref().unwrap().backend_node_id,
        frame_owner.backend_node_id
    );
    // 14: URL/origin cannot satisfy old identity, in either direction.
    // `FrameContext::revalidate` never reads URL or origin at all (by
    // construction — only session liveness, frame id, loader id, frame
    // owner and execution context), so neither a matching nor a differing
    // origin changes its outcome. The replacement here uses a genuinely
    // different origin than child A's original (`original_url`, still on
    // record) specifically so this is not a same-origin coincidence: the old
    // context fails regardless of what the new one's origin is.
    assert_ne!(original_url, "", "child A's original URL was captured");
    let replacement_url = browser
        .execute(
            spider::chromiumoxide::cdp::browser_protocol::target::GetTargetInfoParams::builder()
                .target_id(replacement.target_id.clone())
                .build(),
        )
        .await
        .unwrap()
        .result
        .target_info
        .url;
    assert_ne!(
        replacement_url, original_url,
        "this run's replacement deliberately uses a different origin than child A's original"
    );
    assert!(child_a.revalidate(Some(&top_level)).await.is_err());

    // 13: the same selector resolved fresh in the replacement frame cannot
    // satisfy the old DOM identity — the old identity is checked only
    // through the old (dead) session, never re-queried by selector.
    let _replacement_backend_node_id = backend_node_id_for_selector(&replacement, "#inside").await;
    assert!(matches!(
        child_a.revalidate_dom_identity(dom_identity).await,
        Err(FrameContextFailure::DomIdentityUnavailable)
    ));

    // 15: identical geometry does not satisfy the old identity either —
    // revalidate() never reads geometry at all, so this is true by
    // construction; confirm the geometry really was unchanged so the point
    // is not vacuous.
    let geometry_after = page
        .evaluate(FRAME_GEOMETRY_JS)
        .await
        .unwrap()
        .value()
        .cloned()
        .unwrap();
    assert_eq!(geometry_before["width"], geometry_after["width"]);
    assert_eq!(geometry_before["height"], geometry_after["height"]);
    assert!(child_a.revalidate(Some(&top_level)).await.is_err());

    // --- 8/9: a real top-level navigation invalidates both the frame
    // identity (checked structurally via loader id, criterion 8) and the
    // execution context (criterion 9). Proven on the top-level frame, whose
    // navigation semantics are Chromium's ordinary, well-defined case; a
    // genuine OOPIF child target that navigates to a new document was
    // observed in this fixture to always be replaced by a brand new target
    // (never staying attached with just a new loader id) — that scenario is
    // exactly what criteria 10-13 above already prove, so it is not
    // duplicated here as a separate "in-place" case.
    let top_level_before_nav = FrameContext::resolve_top_level(&browser, &parent_target_info)
        .await
        .unwrap();
    page.goto(format!("http://127.0.0.1:{parent_port}/?nav=2"))
        .await
        .unwrap();
    let navigated = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Err(FrameContextFailure::FrameNavigated) =
                top_level_before_nav.revalidate(None).await
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(
        navigated.is_ok(),
        "top-level navigation must invalidate the old context"
    );
    // 9, proven directly and independently of `revalidate`'s check order: an
    // evaluation pinned to the pre-navigation execution context id no longer
    // resolves, because Chromium destroyed and replaced it.
    let stale_context_eval = top_level_before_nav
        .execute(
            spider::chromiumoxide::cdp::js_protocol::runtime::EvaluateParams::builder()
                .expression("true")
                .context_id(top_level_before_nav.execution_context_id)
                .build()
                .unwrap(),
        )
        .await;
    assert!(
        stale_context_eval.is_err(),
        "the pre-navigation execution context must no longer resolve"
    );

    browser.close().await.unwrap();
    handler_task.abort();
    // A `chrome-error` navigation (a genuine connection failure, observed
    // live for some of these origins in this environment) never completes a
    // TCP handshake, so a server task's `accept()` can be left pending
    // forever; abort rather than await these to avoid hanging cleanup on
    // that environment condition.
    parent_server.abort();
    child_a_server.abort();
    child_b_server.abort();
    replacement_server.abort();
}

/// Top-level support must keep working through the exact same canonical
/// abstraction — no separate code path for the non-OOPIF case.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canonical_top_level_frame_context_resolves_and_revalidates() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(serve(
        listener,
        "<p>top level only</p>",
        Duration::from_secs(30),
    ));

    let profile =
        std::env::temp_dir().join(format!("scorpion-frame-context-top-{}", std::process::id()));
    let config = BrowserConfig::builder()
        .user_data_dir(profile)
        .chrome_executable("/usr/bin/chromium")
        .headless_mode(HeadlessMode::New)
        .incognito()
        .arg("--no-sandbox")
        .arg("--disable-background-networking")
        .launch_timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let (browser, mut handler) = Browser::launch(config).await.unwrap();
    let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });
    let browser = std::sync::Arc::new(browser);
    let mut attached = browser
        .event_listener::<EventAttachedToTarget>()
        .await
        .unwrap();

    let url = format!("http://127.0.0.1:{port}/");
    let navigation_browser = browser.clone();
    let navigation = tokio::spawn(async move { navigation_browser.new_page(url).await });
    let target_info = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = attached.next().await.unwrap();
            if event.target_info.r#type == "page" {
                return event.target_info.clone();
            }
        }
    })
    .await
    .unwrap();
    let _page = navigation.await.unwrap().unwrap();
    let context = FrameContext::resolve_top_level(&browser, &target_info)
        .await
        .unwrap();
    assert_eq!(context.classification, FrameClassification::TopLevel);
    assert!(context.parent_frame_id.is_none());
    assert!(context.frame_owner.is_none());
    context.revalidate(None).await.unwrap();

    browser.close().await.unwrap();
    handler_task.abort();
    server.abort();
}
