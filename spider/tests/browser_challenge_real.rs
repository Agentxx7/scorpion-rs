#![cfg(feature = "chrome")]

use spider::features::browser_challenge::{
    BrowserChallengeAction, BrowserChallengeFailure, BrowserChallengeSnapshot,
    BrowserHorizontalDrag,
};

const HTML: &str = r#"<!doctype html><style>
  body{margin:0;height:1200px} #challenge{position:absolute;left:40px;top:230px;width:240px;height:120px;background:#ddd}
  button{position:absolute;left:20px;top:15px;width:60px;height:35px}
</style><div id=challenge onclick="if(event.target===this)this.dataset.point='yes'">
  <button id=target onclick="this.dataset.clicked='yes'">pick</button>
</div><script>
  const c=document.querySelector('#challenge');
  c.addEventListener('mousedown',()=>c.dataset.down='yes');
  c.addEventListener('mouseup',()=>{if(c.dataset.down==='yes')c.dataset.dragged='yes'});
</script>"#;

#[tokio::test]
async fn controlled_browser_snapshot_click_point_and_drag_change_dom() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut request = [0u8; 2048];
        let _ = stream.read(&mut request).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            HTML.len(), HTML
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });
    let config = spider::configuration::Configuration::default();
    let Some((browser, handler, _, _, _)) =
        spider::features::chrome::launch_browser(&config, &None).await
    else {
        panic!("controlled real-browser acceptance requires local Chrome");
    };
    let url = format!("http://{address}/");
    let page = browser.new_page(url).await.unwrap();
    page.wait_for_navigation().await.unwrap();
    page.evaluate("window.scrollTo(0,100)").await.unwrap();

    let challenge = page.find_element("#challenge").await.unwrap();
    let target = page.find_element("#target").await.unwrap();
    let click =
        BrowserChallengeSnapshot::capture(&page, challenge, vec![("choice-1".into(), target)])
            .await
            .unwrap();
    click.revalidate(&page).await.unwrap();
    click
        .apply(
            &page,
            BrowserChallengeAction::ExactTargetClick {
                stable_id: "choice-1".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        page.evaluate("document.querySelector('#target').dataset.clicked")
            .await
            .unwrap()
            .value(),
        Some(&serde_json::json!("yes"))
    );

    let point = BrowserChallengeSnapshot::capture(
        &page,
        page.find_element("#challenge").await.unwrap(),
        Vec::new(),
    )
    .await
    .unwrap();
    assert_eq!(click.transform.scroll_y, 100.0);
    assert_eq!(
        click.transform.capture_clip.y,
        click.transform.browser_geometry.y + click.transform.scroll_y
    );
    point
        .apply(
            &page,
            BrowserChallengeAction::ExactPoint {
                x: f64::from(point.captured_pixel_width) * 0.75,
                y: f64::from(point.captured_pixel_height) * 0.75,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        page.evaluate("document.querySelector('#challenge').dataset.point")
            .await
            .unwrap()
            .value(),
        Some(&serde_json::json!("yes"))
    );

    let drag = BrowserChallengeSnapshot::capture(
        &page,
        page.find_element("#challenge").await.unwrap(),
        Vec::new(),
    )
    .await
    .unwrap();
    drag.apply(
        &page,
        BrowserChallengeAction::ExactHorizontalDrag(BrowserHorizontalDrag {
            start_x: 120.0,
            start_y: 60.0,
            end_x: 180.0,
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        page.evaluate("document.querySelector('#challenge').dataset.dragged")
            .await
            .unwrap()
            .value(),
        Some(&serde_json::json!("yes"))
    );

    let stale = BrowserChallengeSnapshot::capture(
        &page,
        page.find_element("#challenge").await.unwrap(),
        vec![(
            "choice-1".into(),
            page.find_element("#target").await.unwrap(),
        )],
    )
    .await
    .unwrap();
    page.evaluate("target.replaceWith(target.cloneNode(true))")
        .await
        .unwrap();
    assert_eq!(
        stale.revalidate(&page).await,
        Err(BrowserChallengeFailure::TargetStale)
    );

    let mutated = BrowserChallengeSnapshot::capture(
        &page,
        page.find_element("#challenge").await.unwrap(),
        Vec::new(),
    )
    .await
    .unwrap();
    page.evaluate("challenge.dataset.mutated='yes'")
        .await
        .unwrap();
    assert_eq!(
        mutated.revalidate(&page).await,
        Err(BrowserChallengeFailure::ChallengeMutated)
    );
    page.evaluate("delete challenge.dataset.mutated")
        .await
        .unwrap();

    let geometry = BrowserChallengeSnapshot::capture(
        &page,
        page.find_element("#challenge").await.unwrap(),
        Vec::new(),
    )
    .await
    .unwrap();
    page.evaluate("document.styleSheets[0].insertRule('#challenge{width:260px!important}',0)")
        .await
        .unwrap();
    assert_eq!(
        geometry.revalidate(&page).await,
        Err(BrowserChallengeFailure::GeometryChanged)
    );
    page.evaluate("document.styleSheets[0].deleteRule(0)")
        .await
        .unwrap();

    page.evaluate("document.body.appendChild(document.createElement('iframe'))")
        .await
        .unwrap();
    let nested = BrowserChallengeSnapshot::capture(
        &page,
        page.find_element("#challenge").await.unwrap(),
        Vec::new(),
    )
    .await;
    assert!(matches!(
        nested,
        Err(BrowserChallengeFailure::UnsupportedContext)
    ));
    page.close().await.unwrap();
    browser.close().await.unwrap();
    handler.abort();
    server.await.unwrap();
}
