#![cfg(all(
    feature = "evidence",
    any(
        feature = "cache",
        feature = "cache_mem",
        feature = "chrome_remote_cache"
    )
))]

//! Section G/J/H regression
//! (`SCORPION_CANONICAL_MULTI_PAGE_HTTP_TOR_CRAWL_BLOCKER_FIX_001` /
//! `..._FINAL_BLOCKER_FIX_001`):
//!
//! `AcquisitionTransport` means "this `Page` was acquired over this
//! network transport", never "the surrounding `Website` crawl happened to
//! be configured for this transport". A disk/mem cache hit must not be
//! stamped merely because it was produced inside a crawl configured for
//! `Default` transport — the `ACQUISITION_TRANSPORT_SCOPE` wrapping
//! `crawl()`'s entire body (including `try_cache_shortcircuit`) would
//! otherwise leak onto pages that never touched the network. Fixed
//! centrally via `crate::utils::AcquisitionOrigin` on `PageResponse`
//! rather than per-call-site manual resets — see `Page::build`'s doc
//! comment for the canonical rule this file proves.
//!
//! Gated on `evidence` in addition to a cache feature: this file
//! exercises `build_evidence`, which only exists behind
//! `feature = "evidence"`. A `cache`-only build (no `evidence`) simply
//! excludes this whole test binary rather than failing to compile — no
//! new production feature coupling was added; `cache` still does not
//! depend on `evidence`, only this test does.
//!
//! Kept in its own test binary (not inside `website.rs`'s internal
//! `#[cfg(test)] mod tests`) deliberately: a pre-existing, unrelated
//! baseline defect elsewhere in that module's test suite fails to compile
//! under any `cache_request`-implying feature combination (confirmed via
//! `git stash` against baseline `eb371a00` — `Client::new()` calls at
//! `spider/src/utils/mod.rs` around line 10464/10475 assume a plain
//! `reqwest::Client`, which is not the type `crate::Client` resolves to
//! once `cache`/`cache_mem` pulls in `cache_request`). An integration
//! test file compiles only the library itself (not its internal unit
//! test module), so it sidesteps that unrelated, out-of-scope defect
//! entirely — see the final report's `BASELINE_ISSUES` section.

use spider::features::transport::AcquisitionTransport;
use spider::hashbrown::HashMap;
use spider::utils::evidence::build_evidence;
use spider::website::Website;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Two-pass proof against a real local HTTP fixture, so a cache hit is
/// genuinely proven (zero additional fixture hits), not merely assumed:
/// pass 1 is a real network fetch (must be stamped `Some(Default)`); pass
/// 2, a fresh `Website` for the same URL, must be served entirely from
/// cache (`try_cache_shortcircuit`) and must be stamped `None`.
#[tokio::test]
async fn cache_shortcircuit_page_is_never_stamped_with_acquisition_transport() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = hits.clone();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            hits_clone.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buf = [0_u8; 1024];
                let _ = stream.read(&mut buf).await;
                let body = b"<html><body>cache provenance fixture</body></html>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nCache-Control: public, max-age=3600\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.write_all(body).await;
            });
        }
    });

    let target_url = format!("http://{addr}/cache-provenance-test");

    // Pass 1: genuine network fetch — populates the cache.
    let mut website = Website::new(&target_url);
    website.configuration.cache = true;
    website.with_cache_skip_browser(true);
    website.with_budget(Some(HashMap::from([("*", 1)])));

    let mut rx = website.subscribe(4);
    let handle = tokio::spawn(async move { rx.recv().await.ok() });
    website.crawl().await;
    let page1 = handle
        .await
        .unwrap()
        .expect("page received via channel (pass 1)");

    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "pass 1 must be a real network fetch"
    );
    assert_eq!(
        page1.transport(),
        Some(AcquisitionTransport::Default),
        "a genuine network fetch must be truthfully stamped Default"
    );
    // Section H/I: evidence built from a Default-acquired page must
    // canonicalize to transport = "default", dns = null — read directly
    // off `Page.transport()`, with no caller-side reconstruction.
    let evidence1 = build_evidence(&page1, None, false, false);
    assert_eq!(evidence1.transport.as_deref(), Some("default"));
    assert_eq!(evidence1.dns, None);

    // Pass 2: same URL, fresh Website — must hit `try_cache_shortcircuit`
    // (zero additional fixture hits) and must NOT inherit the ambient
    // Default-transport stamp merely because the crawl is configured for
    // Default transport.
    let mut website2 = Website::new(&target_url);
    website2.configuration.cache = true;
    website2.with_cache_skip_browser(true);
    website2.with_budget(Some(HashMap::from([("*", 1)])));

    let mut rx2 = website2.subscribe(4);
    let handle2 = tokio::spawn(async move { rx2.recv().await.ok() });
    website2.crawl().await;
    let page2 = handle2
        .await
        .unwrap()
        .expect("cached page received via channel (pass 2)");

    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "pass 2 must be served entirely from cache — zero additional network hits"
    );
    assert_eq!(
        page2.transport(),
        None,
        "a cache-hit Page must not be stamped with AcquisitionTransport merely \
         because the crawl is configured for Default transport"
    );
    // Section H/I: a page this frontier's acquisition scope never
    // stamped must yield transport = null, dns = null — never fabricated
    // from `Website.configuration.transport_policy`, which a cache-hit
    // `Website` here still has set to `Default`.
    let evidence2 = build_evidence(&page2, None, false, false);
    assert_eq!(evidence2.transport, None);
    assert_eq!(evidence2.dns, None);
}

/// Section B/I (final-blocker-fix frontier): the top-level
/// `try_cache_shortcircuit` short-circuit above is not the only cache
/// path — `sitemap_parse_crawl`'s per-page fetch calls
/// `Page::new_page_with_cache` -> `fetch_page_html_raw_cached`, a
/// *different* producer that also reaches `build_cached_html_page_response`
/// on a cache hit. Proves the centralized `AcquisitionOrigin` fix (not a
/// per-call-site patch) reaches this path too, and doubles as the
/// "sitemap-derived Default cached Page" coverage Section I asks for.
#[cfg(feature = "sitemap")]
#[tokio::test]
async fn sitemap_derived_cache_hit_page_is_never_stamped_with_acquisition_transport() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture_addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = hits.clone();

    let sitemap_body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>http://{fixture_addr}/from-sitemap</loc></url>
</urlset>"#
    );

    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            hits_clone.fetch_add(1, Ordering::SeqCst);
            let sitemap_body = sitemap_body.clone();
            tokio::spawn(async move {
                let mut buf = [0_u8; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string();

                let (body, cache_control): (String, &str) = match path.as_str() {
                    "/sitemap.xml" => (sitemap_body, "no-store"),
                    "/from-sitemap" => (
                        "<html><body>sitemap-derived cache fixture</body></html>".to_string(),
                        "public, max-age=3600",
                    ),
                    _ => (
                        "<html><body>seed, no links</body></html>".to_string(),
                        "no-store",
                    ),
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nCache-Control: {cache_control}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });

    let seed_url = format!("http://{fixture_addr}/");

    // Pass 1: genuine network fetch of the sitemap-derived page —
    // populates the cache for `/from-sitemap` (`Cache-Control: public`).
    let mut website = Website::new(&seed_url);
    website.configuration.cache = true;
    website.with_cache_skip_browser(true);
    website.with_limit(10);
    let mut rx = website.subscribe(16);
    let pages = tokio::spawn(async move {
        let mut collected = Vec::new();
        while collected.len() < 2 {
            match rx.recv().await {
                Ok(page) => collected.push(page),
                Err(_) => break,
            }
        }
        collected
    });
    website.crawl_raw().await;
    website.unsubscribe();
    let pages = tokio::time::timeout(std::time::Duration::from_secs(5), pages)
        .await
        .expect("page collector must finish once 2 pages have arrived (pass 1)")
        .unwrap();
    assert_eq!(pages.len(), 2, "seed + sitemap-derived page (pass 1)");
    let hits_after_pass1 = hits.load(Ordering::SeqCst);

    let sitemap_page1 = pages
        .iter()
        .find(|p| p.get_url().contains("/from-sitemap"))
        .expect("sitemap-derived page must be present in pass 1");
    assert_eq!(
        sitemap_page1.transport(),
        Some(AcquisitionTransport::Default),
        "a genuine sitemap-derived network fetch must be truthfully stamped Default"
    );

    // Pass 2: fresh Website, same sitemap — `/from-sitemap` must now be
    // served from cache via `Page::new_page_with_cache`
    // (`fetch_page_html_raw_cached`), not re-fetched.
    let mut website2 = Website::new(&seed_url);
    website2.configuration.cache = true;
    website2.with_cache_skip_browser(true);
    website2.with_limit(10);
    let mut rx2 = website2.subscribe(16);
    let pages2 = tokio::spawn(async move {
        let mut collected = Vec::new();
        while collected.len() < 2 {
            match rx2.recv().await {
                Ok(page) => collected.push(page),
                Err(_) => break,
            }
        }
        collected
    });
    website2.crawl_raw().await;
    website2.unsubscribe();
    let pages2 = tokio::time::timeout(std::time::Duration::from_secs(5), pages2)
        .await
        .expect("page collector must finish once 2 pages have arrived (pass 2)")
        .unwrap();
    assert_eq!(pages2.len(), 2, "seed + sitemap-derived page (pass 2)");

    let sitemap_page2 = pages2
        .iter()
        .find(|p| p.get_url().contains("/from-sitemap"))
        .expect("sitemap-derived page must be present in pass 2");
    assert_eq!(
        hits.load(Ordering::SeqCst) - hits_after_pass1,
        2,
        "pass 2 must hit the fixture exactly twice more — the seed and \
         sitemap.xml (both `no-store`, and sitemap.xml is never cache-checked \
         at all), never `/from-sitemap` (served from cache)"
    );
    assert_eq!(
        sitemap_page2.transport(),
        None,
        "a sitemap-derived cache-hit Page must not be stamped with AcquisitionTransport"
    );
    let evidence = build_evidence(sitemap_page2, None, false, false);
    assert_eq!(evidence.transport, None);
    assert_eq!(evidence.dns, None);
}

/// Section J (final-blocker-fix frontier): reconfirm — not merely
/// structurally infer — that Tor combined with an active `cache`
/// configuration fails closed, with zero target network activity. This
/// cache-provenance work is about truthful Default/non-network `Page`s,
/// never about making Tor cache-aware. Only compilable/meaningful when
/// `transport_tor` is also enabled — the top-level file gate deliberately
/// does not require it, so this is function-gated instead.
#[cfg(feature = "transport_tor")]
#[tokio::test]
async fn tor_crawl_with_cache_configured_fails_closed() {
    use spider::features::transport::{TorTransportConfig, TransportPolicy};

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = hits.clone();
    tokio::spawn(async move {
        loop {
            let (_stream, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            hits_clone.fetch_add(1, Ordering::SeqCst);
        }
    });

    let mut website = Website::new(&format!("http://{addr}/"));
    // A `cache`-implying build compiles the Tor-crawl-capable code path
    // only under `not(cache_request)` — with `cache` active, this policy
    // structurally cannot reach the real preflight and must fall back to
    // `TorNotCompiled` regardless of the (unreachable) SOCKS endpoint.
    website.with_transport(TransportPolicy::Tor(
        TorTransportConfig::new("socks5h://127.0.0.1:1").unwrap(),
    ));
    website.configuration.cache = true;
    website.crawl_raw().await;

    assert_eq!(website.get_status(), &spider::website::CrawlStatus::Invalid);
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "a Tor crawl with cache configured must never reach the target network"
    );
}
