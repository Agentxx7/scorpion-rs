#![cfg(feature = "chrome")]

use std::time::Duration;

use spider::features::browser_challenge::{BrowserChallengeFailure, BrowserChallengeSnapshot};
use spider::features::captcha::{
    CaptchaChallengeKind, CaptchaProvider, CaptchaProviderAvailability,
    CaptchaProviderCapabilities, CaptchaProviderId, CaptchaProviderLocality,
    CaptchaProviderRegistry, CaptchaSolution, CaptchaSolveFailure, CaptchaSolveOutcome,
    CaptchaSolveProvenance, CaptchaSolveRequest,
};
use spider::features::captcha_browser::{
    execute_browser_captcha_attempt, CaptchaBrowserAttempt, CaptchaBrowserChallenge,
    CaptchaBrowserExecutionFailureKind, CaptchaBrowserExecutionStage, CaptchaBrowserGridCell,
    CaptchaBrowserProgression,
};

static CAPABILITIES: CaptchaProviderCapabilities = CaptchaProviderCapabilities {
    provider: CaptchaProviderId::PALIGEMMA_LOCAL,
    locality: CaptchaProviderLocality::Local,
    supported_kinds: &[
        CaptchaChallengeKind::ImageGridSelection,
        CaptchaChallengeKind::HorizontalOffset,
        CaptchaChallengeKind::PointSelection,
    ],
    supported_media_types: &["image/png"],
    maximum_inputs: 1,
    requires_credentials: false,
};

enum FakeAnswer {
    Solution(CaptchaSolution),
    Failure,
}

struct FakeProvider(FakeAnswer);

#[async_trait::async_trait]
impl CaptchaProvider for FakeProvider {
    fn capabilities(&self) -> &'static CaptchaProviderCapabilities {
        &CAPABILITIES
    }

    fn availability(&self) -> CaptchaProviderAvailability {
        CaptchaProviderAvailability::Available
    }

    async fn solve(&self, _request: &CaptchaSolveRequest) -> CaptchaSolveOutcome {
        match &self.0 {
            FakeAnswer::Solution(solution) => CaptchaSolveOutcome::Solved {
                solution: solution.clone(),
                provenance: CaptchaSolveProvenance::local(CaptchaProviderId::PALIGEMMA_LOCAL),
            },
            FakeAnswer::Failure => CaptchaSolveOutcome::Failed {
                failure: CaptchaSolveFailure::LocalExecutionFailure,
                provenance: Some(CaptchaSolveProvenance::local(
                    CaptchaProviderId::PALIGEMMA_LOCAL,
                )),
            },
        }
    }
}

const HTML: &str = r#"<!doctype html><style>
body{margin:0;height:1000px}#challenge{position:absolute;left:40px;top:180px;width:300px;height:200px;background:#ddd}
.cell{position:absolute;top:20px;width:100px;height:70px}.left{left:20px}.right{left:180px}
#handle{position:absolute;left:20px;top:130px;width:30px;height:30px;background:#555}
</style><div id=challenge onclick="if(event.target===this)window.pointApplied=(event.clientX+','+event.clientY)">
<button id=left class="cell left" onclick="window.gridChoice='left'">left</button>
<button id=right class="cell right" onclick="window.gridChoice='right'">right</button>
<div id=handle></div></div><script>
window.actionCount=0;
document.addEventListener('click',()=>window.actionCount++);
handle.addEventListener('mousedown',()=>window.dragStarted=true);
document.addEventListener('mouseup',()=>{if(window.dragStarted){window.dragApplied=true;window.actionCount++}});
</script>"#;

fn attempt(challenge: CaptchaBrowserChallenge) -> CaptchaBrowserAttempt {
    CaptchaBrowserAttempt {
        correlation_id: "controlled-browser".into(),
        selected_provider: CaptchaProviderId::PALIGEMMA_LOCAL,
        deadline: Duration::from_secs(2),
        challenge,
    }
}

async fn registry_attempt(
    page: &chromiumoxide::Page,
    snapshot: &BrowserChallengeSnapshot,
    challenge: CaptchaBrowserChallenge,
    answer: FakeAnswer,
) -> Result<
    spider::features::captcha_browser::CaptchaBrowserExecutionReport,
    spider::features::captcha_browser::CaptchaBrowserExecutionFailure,
> {
    let provider = FakeProvider(answer);
    let mut registry = CaptchaProviderRegistry::new();
    registry.register(&provider).unwrap();
    execute_browser_captcha_attempt(page, snapshot, &registry, attempt(challenge)).await
}

#[tokio::test]
async fn controlled_browser_binding_applies_all_forms_and_fails_closed() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut request = [0u8; 2048];
        let _ = stream.read(&mut request).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            HTML.len(),
            HTML
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });
    let config = spider::configuration::Configuration::default();
    let Some((browser, handler, _, _, _)) =
        spider::features::chrome::launch_browser(&config, &None).await
    else {
        panic!("controlled real-browser acceptance requires local Chrome");
    };
    let page = browser
        .new_page(format!("http://{address}/"))
        .await
        .unwrap();
    page.wait_for_navigation().await.unwrap();
    page.evaluate("window.scrollTo(0,100)").await.unwrap();

    let grid = BrowserChallengeSnapshot::capture(
        &page,
        page.find_element("#challenge").await.unwrap(),
        vec![
            ("left".into(), page.find_element("#left").await.unwrap()),
            ("right".into(), page.find_element("#right").await.unwrap()),
        ],
    )
    .await
    .unwrap();
    let report = registry_attempt(
        &page,
        &grid,
        CaptchaBrowserChallenge::ImageGridSelection {
            instruction: "select right".into(),
            rows: 1,
            columns: 2,
            cells: vec![
                CaptchaBrowserGridCell {
                    choice_id: "left".into(),
                    row: 0,
                    column: 0,
                },
                CaptchaBrowserGridCell {
                    choice_id: "right".into(),
                    row: 0,
                    column: 1,
                },
            ],
            empty_selection_valid: false,
        },
        FakeAnswer::Solution(CaptchaSolution::SelectedChoices(vec!["right".into()])),
    )
    .await
    .unwrap();
    assert_eq!(report.actions_applied, 1);
    assert_eq!(report.stage, CaptchaBrowserExecutionStage::ActionApplied);
    assert_eq!(
        report.progression,
        CaptchaBrowserProgression::NotObservedByBinding
    );
    assert_eq!(
        page.evaluate("window.gridChoice").await.unwrap().value(),
        Some(&serde_json::json!("right"))
    );

    let point = BrowserChallengeSnapshot::capture(
        &page,
        page.find_element("#challenge").await.unwrap(),
        Vec::new(),
    )
    .await
    .unwrap();
    registry_attempt(
        &page,
        &point,
        CaptchaBrowserChallenge::PointSelection {
            instruction: "point".into(),
        },
        FakeAnswer::Solution(CaptchaSolution::Point { x: 150.0, y: 110.0 }),
    )
    .await
    .unwrap();
    assert!(page
        .evaluate("window.pointApplied")
        .await
        .unwrap()
        .value()
        .is_some());

    let drag = BrowserChallengeSnapshot::capture(
        &page,
        page.find_element("#challenge").await.unwrap(),
        vec![("handle".into(), page.find_element("#handle").await.unwrap())],
    )
    .await
    .unwrap();
    registry_attempt(
        &page,
        &drag,
        CaptchaBrowserChallenge::HorizontalOffset {
            instruction: "drag".into(),
            handle_target_id: "handle".into(),
        },
        FakeAnswer::Solution(CaptchaSolution::HorizontalOffset(60.0)),
    )
    .await
    .unwrap();
    assert_eq!(
        page.evaluate("window.dragApplied").await.unwrap().value(),
        Some(&serde_json::json!(true))
    );

    let before = page
        .evaluate("window.actionCount")
        .await
        .unwrap()
        .value()
        .cloned();
    let failed = BrowserChallengeSnapshot::capture(
        &page,
        page.find_element("#challenge").await.unwrap(),
        Vec::new(),
    )
    .await
    .unwrap();
    let error = registry_attempt(
        &page,
        &failed,
        CaptchaBrowserChallenge::PointSelection {
            instruction: "point".into(),
        },
        FakeAnswer::Failure,
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error.kind,
        CaptchaBrowserExecutionFailureKind::ProviderFailure
    ));
    assert_eq!(error.actions_applied, 0);
    assert_eq!(
        page.evaluate("window.actionCount").await.unwrap().value(),
        before.as_ref()
    );

    let mutated = BrowserChallengeSnapshot::capture(
        &page,
        page.find_element("#challenge").await.unwrap(),
        Vec::new(),
    )
    .await
    .unwrap();
    page.evaluate("challenge.dataset.changed='yes'")
        .await
        .unwrap();
    let error = registry_attempt(
        &page,
        &mutated,
        CaptchaBrowserChallenge::PointSelection {
            instruction: "point".into(),
        },
        FakeAnswer::Solution(CaptchaSolution::Point { x: 10.0, y: 10.0 }),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error.kind,
        CaptchaBrowserExecutionFailureKind::Browser(BrowserChallengeFailure::ChallengeMutated)
    ));
    assert_eq!(error.actions_applied, 0);

    page.evaluate("delete challenge.dataset.changed")
        .await
        .unwrap();
    let stale = BrowserChallengeSnapshot::capture(
        &page,
        page.find_element("#challenge").await.unwrap(),
        vec![("left".into(), page.find_element("#left").await.unwrap())],
    )
    .await
    .unwrap();
    page.evaluate("left.replaceWith(left.cloneNode(true))")
        .await
        .unwrap();
    let error = registry_attempt(
        &page,
        &stale,
        CaptchaBrowserChallenge::ImageGridSelection {
            instruction: "select left".into(),
            rows: 1,
            columns: 1,
            cells: vec![CaptchaBrowserGridCell {
                choice_id: "left".into(),
                row: 0,
                column: 0,
            }],
            empty_selection_valid: false,
        },
        FakeAnswer::Solution(CaptchaSolution::SelectedChoices(vec!["left".into()])),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error.kind,
        CaptchaBrowserExecutionFailureKind::Browser(BrowserChallengeFailure::TargetStale)
    ));
    assert_eq!(error.actions_applied, 0);

    let geometry = BrowserChallengeSnapshot::capture(
        &page,
        page.find_element("#challenge").await.unwrap(),
        Vec::new(),
    )
    .await
    .unwrap();
    page.evaluate("document.styleSheets[0].insertRule('#challenge{width:320px!important}',0)")
        .await
        .unwrap();
    let error = registry_attempt(
        &page,
        &geometry,
        CaptchaBrowserChallenge::PointSelection {
            instruction: "point".into(),
        },
        FakeAnswer::Solution(CaptchaSolution::Point { x: 10.0, y: 10.0 }),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error.kind,
        CaptchaBrowserExecutionFailureKind::Browser(BrowserChallengeFailure::GeometryChanged)
    ));
    assert_eq!(error.actions_applied, 0);

    page.close().await.unwrap();
    browser.close().await.unwrap();
    handler.abort();
    server.await.unwrap();
}
