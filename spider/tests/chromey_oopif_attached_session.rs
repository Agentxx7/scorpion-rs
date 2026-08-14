#![cfg(feature = "chrome")]

use spider::chromiumoxide::browser::HeadlessMode;
use spider::chromiumoxide::browser::{AttachedSessionError, Browser};
use spider::chromiumoxide::cdp::browser_protocol::{
    dom::{GetDocumentParams, QuerySelectorParams},
    target::{EventAttachedToTarget, EventDetachedFromTarget, EventTargetCreated},
};
use spider::chromiumoxide::cdp::js_protocol::runtime::{
    DisableParams as RuntimeDisableParams, EnableParams as RuntimeEnableParams, EvaluateParams,
    EventExecutionContextCreated,
};
use spider::chromiumoxide::BrowserConfig;
use spider::tokio_stream::StreamExt;
use std::time::Duration;

async fn serve(listener: tokio::net::TcpListener, body: &'static str, requests: usize) {
    for _ in 0..requests {
        let (mut stream, _) = listener.accept().await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut request = [0_u8; 2048];
        let _ = stream.read(&mut request).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(), body
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oopif_attached_session_routes_commands_and_invalidates_lifecycle() {
    let child_listener = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
    let child_port = child_listener.local_addr().unwrap().port();
    let child_server = tokio::spawn(serve(child_listener, "<button id=inside>child</button>", 2));
    let parent_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let parent_port = parent_listener.local_addr().unwrap().port();
    let parent_html = Box::leak(
        format!("<iframe id=frame src='http://localhost:{child_port}/'></iframe>").into_boxed_str(),
    );
    let parent_server = tokio::spawn(serve(parent_listener, parent_html, 1));

    let profile = std::env::temp_dir().join(format!("chromey-oopif-{}", std::process::id()));
    let config = BrowserConfig::builder()
        .user_data_dir(profile)
        .chrome_executable("/usr/bin/chromium")
        .headless_mode(HeadlessMode::True)
        .arg("--no-sandbox")
        .arg("--site-per-process")
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
    let parent_target = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = creations.next().await.unwrap();
            if event.target_info.r#type == "page" && event.target_info.url == parent_url {
                break event.target_info.target_id.clone();
            }
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = attached.next().await.unwrap();
            if event.target_info.target_id == parent_target {
                break;
            }
        }
    })
    .await
    .unwrap();
    let page = browser.get_page(parent_target).await.unwrap();

    let first_event = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = attached.next().await.unwrap();
            if event.target_info.r#type == "iframe" {
                break event;
            }
        }
    })
    .await
    .unwrap();
    let first = browser
        .attached_session(first_event.target_info.target_id.clone())
        .await
        .unwrap();
    assert_eq!(first.session_id(), &first_event.session_id);

    first
        .execute(RuntimeDisableParams::default())
        .await
        .unwrap();
    let mut child_contexts = first
        .event_listener::<EventExecutionContextCreated>()
        .await
        .unwrap();
    first.execute(RuntimeEnableParams::default()).await.unwrap();
    let child_context = tokio::time::timeout(Duration::from_secs(10), child_contexts.next())
        .await
        .unwrap()
        .unwrap();
    assert!(child_context.context.origin.contains("localhost"));
    let child_eval = first
        .execute(EvaluateParams::new(
            "document.body.dataset.owner='child'; document.body.dataset.owner",
        ))
        .await
        .unwrap();
    assert_eq!(
        child_eval
            .result
            .result
            .value
            .as_ref()
            .and_then(|value| value.as_str()),
        Some("child")
    );
    assert_eq!(
        page.evaluate("document.body.dataset.owner")
            .await
            .unwrap()
            .value(),
        None
    );
    let document = first.execute(GetDocumentParams::default()).await.unwrap();
    let node = first
        .execute(QuerySelectorParams::new(
            document.result.root.node_id,
            "#inside",
        ))
        .await
        .unwrap();
    assert_ne!(*node.result.node_id.inner(), 0);

    page.evaluate("document.querySelector('#frame').remove()")
        .await
        .unwrap();
    let detached_event = tokio::time::timeout(Duration::from_secs(10), detached.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detached_event.session_id, *first.session_id());
    assert!(matches!(
        first.validate().await,
        Err(AttachedSessionError::SessionDetached) | Err(AttachedSessionError::TargetDestroyed)
    ));

    page.evaluate(format!(
        "document.body.insertAdjacentHTML('beforeend', `<iframe id=frame2 src='http://localhost:{child_port}/'></iframe>`);"
    ))
    .await
    .unwrap();
    let second_event = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = attached.next().await.unwrap();
            if event.target_info.r#type == "iframe" {
                break event;
            }
        }
    })
    .await
    .unwrap();
    assert_ne!(second_event.session_id, *first.session_id());
    assert!(first.validate().await.is_err());
    assert!(matches!(
        browser
            .attached_session("unknown-target".to_string().into())
            .await,
        Err(AttachedSessionError::UnknownTarget)
    ));

    browser.close().await.unwrap();
    assert!(matches!(
        first.validate().await,
        Err(AttachedSessionError::CommandRoutingFailed(_))
    ));
    handler_task.abort();
    navigation.abort();
    parent_server.await.unwrap();
    child_server.await.unwrap();
}
