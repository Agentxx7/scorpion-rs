//! SCORPION_CI_REAL_CHROME_EXECUTION_STABILITY_001: a minimal, fast,
//! fail-fast proof that this environment's Chrome/Chromium runtime is
//! genuinely sane BEFORE the expensive required real-browser evidence
//! suites run. This is deliberately NOT a second browser implementation
//! and NOT a substitute for those suites' own production-reality proof —
//! it drives the exact same canonical entrypoint (`Website::scrape()`
//! with `chrome` compiled in, the identical path
//! `chrome_cache_provenance_population.rs` and every other real-browser
//! suite already uses) against one trivial local fixture, and asserts
//! genuine JS execution mutated the DOM (a marker only real Chrome
//! execution can produce) before shutting down cleanly. It only
//! establishes that the CI runtime is sane; it proves nothing about any
//! specific capability those other suites exist to prove.
//!
//! This frontier's own investigation
//! (`SCORPION_CI_REAL_CHROME_EXECUTION_STABILITY_001`'s closure report)
//! found the canonical launch failure path
//! (`spider::features::chrome::setup_browser_configuration`'s
//! `Browser::launch()` error arm) already logs its real error via
//! `log::error!` — but no real-browser test in this crate ever
//! initialized a logger, so that diagnostic was always silently
//! discarded, in CI and locally alike, for every historical Chrome CI
//! failure. This is the first real-browser test file in this crate to
//! initialize one, with an explicit default level so the diagnostic is
//! visible without requiring an operator to remember to set `RUST_LOG`.
//!
//!   cargo test -p spider --test chrome_readiness_probe --features chrome

use spider::tokio;
use spider::website::Website;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

/// See `chrome_cache_provenance_population.rs`'s identical helper for
/// the full rationale (large-stack isolated runtime for chrome futures
/// — a pre-existing, environment-specific stack-overflow under this
/// feature combination at the default thread stack size, unrelated to
/// this frontier).
fn run_isolated<F>(body: F) -> F::Output
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build isolated current_thread runtime");
            rt.block_on(body)
        })
        .expect("spawn isolated test thread")
        .join()
        .expect("isolated test thread panicked")
}

/// Install a logger with an explicit default level (`warn`) so
/// `log::warn!`/`log::error!` diagnostics from the canonical launch seam
/// are visible by default — `RUST_LOG` still overrides this when set,
/// matching `env_logger`'s own established precedent elsewhere in this
/// crate's test suite (`smart_vs_chrome.rs` and friends), but those
/// callers rely on the operator remembering to set `RUST_LOG`, which is
/// exactly the gap that left every historical Chrome CI failure this
/// far undiagnosed.
fn init_logging() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .is_test(true)
        .try_init();
}

fn one_shot_fixture(body: String) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let handle = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0_u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(body.as_bytes());
        }
    });
    (format!("http://{addr}/"), handle)
}

/// The one readiness proof this file exists for: executable resolution
/// -> real launch -> real navigation -> real JS execution -> clean
/// shutdown, through the exact canonical `Website::scrape()` entrypoint,
/// bounded so a genuinely hung/broken runtime fails within this test's
/// own timeout rather than the CI job's outer step timeout.
#[test]
fn chrome_runtime_is_ready_for_real_browser_evidence() {
    run_isolated(async move {
        init_logging();

        const MARKER: &str = "scorpion-chrome-readiness-marker";
        let (url, server) = one_shot_fixture(format!(
            "<html><body><div id=\"m\">not-yet-rendered</div>\
             <script>document.getElementById('m').textContent = '{MARKER}';</script>\
             </body></html>"
        ));

        let mut website = Website::new(&url);
        website.with_limit(1);

        let outcome = tokio::time::timeout(Duration::from_secs(45), website.scrape()).await;
        server.join().expect("fixture server thread");

        assert!(
            outcome.is_ok(),
            "chrome readiness probe exceeded its 45s bound -- the browser runtime in this \
             environment did not complete a real launch+navigate+execute+shutdown cycle in a \
             reasonable time; see this test's own log output (RUST_LOG=warn by default) for the \
             real Browser::launch()/CDP diagnostic the canonical launch seam already emits"
        );

        let pages = website
            .get_pages()
            .expect("scrape must set self.pages after a completed run");
        assert_eq!(pages.len(), 1, "expected exactly 1 collected page");
        let content = pages[0].get_html();
        assert!(
            content.contains(MARKER),
            "the fixture's own inline <script> must have genuinely executed inside a real \
             Chrome/CDP navigation and mutated the DOM -- its absence means the browser \
             runtime itself is not sane in this environment (content={content:?})"
        );
    })
}
