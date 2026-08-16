#![cfg(all(feature = "chrome", feature = "local_paligemma"))]

use std::path::PathBuf;
use std::time::Duration;

use spider::features::browser_challenge::BrowserChallengeSnapshot;
use spider::features::captcha::{CaptchaProviderId, CaptchaProviderRegistry};
use spider::features::captcha_browser::{
    execute_browser_captcha_attempt, CaptchaBrowserAttempt, CaptchaBrowserChallenge,
    CaptchaBrowserGridCell,
};
use spider::features::paligemma_captcha::PaligemmaLocalCaptchaProvider;
use spider::features::paligemma_runtime::paligemma_cpu_f32_manifest;

/// Real, top-level (non-frame) end-to-end proof of the canonical browser
/// binding through the qualified `paligemma-local` provider — complementary
/// to `captcha_browser_turnstile_real.rs`'s frame-aware genuine Turnstile
/// acceptance, exercising `execute_browser_captcha_attempt` (not its
/// `_in_frame` sibling) with real local inference instead of a
/// `FakeProvider`. Qualification-host proof; ignored in ordinary CI because
/// it requires the pinned ~11.7 GB PaliGemma installation and its qualified
/// CPU/F32 RAM envelope.
#[tokio::test]
#[ignore = "requires pinned PaliGemma artifacts and a qualified CPU/F32 host"]
async fn real_browser_snapshot_paligemma_inference_and_exact_action() {
    let source = PathBuf::from(
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
    let installation = paligemma_cpu_f32_manifest()
        .activate(&staging, &active)
        .unwrap();
    let provider = PaligemmaLocalCaptchaProvider::initialize_from_host(&installation).unwrap();
    let mut registry = CaptchaProviderRegistry::new();
    registry.register(&provider).unwrap();

    let html = r#"<!doctype html><style>body{margin:0}#challenge,#only{position:absolute;left:0;top:0;width:96px;height:64px}#only{font-size:16px}</style><div id=challenge><button id=only onclick="window.progressed=true">ONLY CHOICE</button></div>"#;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut request = [0u8; 2048];
        let _ = stream.read(&mut request).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            html.len(),
            html
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });
    let config = spider::configuration::Configuration::default();
    let Some((browser, handler, _, _, _)) =
        spider::features::chrome::launch_browser(&config, &None).await
    else {
        panic!("qualification host requires local Chrome");
    };
    let page = browser
        .new_page(format!("http://{address}/"))
        .await
        .unwrap();
    page.wait_for_navigation().await.unwrap();
    let snapshot = BrowserChallengeSnapshot::capture(
        &page,
        page.find_element("#challenge").await.unwrap(),
        vec![("only".into(), page.find_element("#only").await.unwrap())],
    )
    .await
    .unwrap();
    let report = execute_browser_captcha_attempt(
        &page,
        &snapshot,
        &registry,
        CaptchaBrowserAttempt {
            correlation_id: "real-browser-paligemma".into(),
            selected_provider: CaptchaProviderId::PALIGEMMA_LOCAL,
            deadline: Duration::from_secs(1_800),
            challenge: CaptchaBrowserChallenge::ImageGridSelection {
                instruction: "Select the only cell. Its stable ID is only.".into(),
                rows: 1,
                columns: 1,
                cells: vec![CaptchaBrowserGridCell {
                    choice_id: "only".into(),
                    row: 0,
                    column: 0,
                }],
                empty_selection_valid: false,
            },
        },
    )
    .await
    .unwrap();
    assert_eq!(report.actions_applied, 1);
    assert_eq!(
        page.evaluate("window.progressed").await.unwrap().value(),
        Some(&serde_json::json!(true))
    );

    page.close().await.unwrap();
    browser.close().await.unwrap();
    handler.abort();
    server.await.unwrap();
    provider.unload();
}
