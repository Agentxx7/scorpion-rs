//! One-shot resource acquisition and evidence/provenance construction.
//!
//! This is the canonical seam both `spider_mcp` and `spider_cli` call into
//! for "fetch exactly one resource and produce truthful retrieval evidence
//! for it" — the shared acquisition/evidence layer beneath Scorpion's
//! feed/sitemap/news-sitemap/robots-sitemap discovery adapters and any
//! evidence-first single-resource fetch. Relocated from `spider_mcp` (where
//! it originated) so a second, independently-drifting implementation is
//! never written for the CLI. This is plumbing, not a new capability: every
//! field `EvidenceBundle` can populate is sourced from data `Page` already
//! captures — nothing here changes crawling, fetching, or rendering
//! behavior.

use crate::page::Page;
use crate::website::Website;

#[cfg(feature = "serde")]
use serde::Serialize;
use sha2::{Digest, Sha256};

/// SHA-256 of exactly the supplied bytes, encoded as lowercase hexadecimal.
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Extract a page's captured screenshot bytes, if any. `Page::screenshot_bytes`
/// only exists at all behind the `chrome` feature, so this is `None`
/// unconditionally without it.
#[cfg(feature = "chrome")]
pub fn page_screenshot_bytes(page: &Page) -> Option<&[u8]> {
    page.screenshot_bytes.as_deref()
}

/// No screenshot bytes are ever available without the `chrome` feature.
#[cfg(not(feature = "chrome"))]
pub fn page_screenshot_bytes(_page: &Page) -> Option<&[u8]> {
    None
}

/// Truthful retrieval evidence for one fetched resource. Every field is
/// `Option`, populated only when the underlying data was actually observed
/// during a fetch — never fabricated or guessed.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct EvidenceBundle {
    /// The URL that was actually requested.
    pub requested_url: Option<String>,
    /// The URL after following any redirects. Equal to `requested_url` when
    /// no redirect occurred — this field is populated whenever a page was
    /// fetched, never omitted just because the two happen to match, so its
    /// presence always reflects ground truth rather than leaving
    /// redirect-vs-no-redirect ambiguous.
    pub final_url: Option<String>,
    /// Unix epoch milliseconds when the live HTTP- or Chrome-produced
    /// representation backing this page finished materializing. `None` when
    /// that canonical completion time was not captured (including cache and
    /// error paths). This is not a server timestamp or request-start time.
    pub retrieved_at: Option<u64>,
    /// Spider's effective/crawler status after existing operational
    /// reclassification and retry policy.
    pub status_code: Option<u16>,
    /// HTTP status actually observed from a response or trusted relay. This
    /// remains independent of Spider's effective/crawler `status_code`.
    pub observed_status_code: Option<u16>,
    /// The response's `Content-Type` header, verbatim, when present.
    pub content_type: Option<String>,
    /// MIME type detected directly from the retained non-browser HTTP
    /// response bytes. Independent of the declared `content_type`; `None`
    /// when bytes are absent, unrecognized, or produced by a browser path.
    pub detected_content_type: Option<String>,
    /// SHA-256 of the exact HTTP content-decoded response-body bytes retained
    /// by `Page` on the non-browser HTTP scrape path. This is not a hash of
    /// transport/wire bytes and no character normalization is applied. Always
    /// `None` for browser/headless fetches because their `Page` bytes represent
    /// Chromium's rendered DOM rather than an HTTP response body.
    pub response_body_hash: Option<String>,
    /// SHA-256 of `content.as_bytes()` exactly as returned in this bundle.
    pub transformed_content_hash: Option<String>,
    /// Textual content in the requested format (markdown/text/raw/xml).
    /// `None` when the request was for a screenshot instead — see
    /// `screenshot`.
    pub content: Option<String>,
    /// Links discovered on the page, when link collection was enabled.
    pub links: Option<Vec<String>>,
    /// Which engine/site surfaced this evidence — populated only for
    /// search-derived evidence (e.g. "youtube"). `None` for a direct fetch:
    /// a URL fetch has no "source" distinct from the URL itself.
    pub source: Option<String>,
    /// Which search provider produced this evidence — populated only for
    /// search-derived evidence (e.g. "searxng"). `None` for a direct fetch.
    pub provider: Option<String>,
    /// The search query that led to this evidence — populated only for
    /// search-derived evidence. `None` for a direct fetch.
    pub query: Option<String>,
    /// Base64-encoded screenshot, when a screenshot was requested and
    /// captured. Kept distinct from `content` — image bytes are not
    /// textual content.
    pub screenshot: Option<String>,
    /// SHA-256 of the original captured PNG bytes, never its base64 encoding.
    pub screenshot_hash: Option<String>,
    /// Reserved for future structured metadata. Always `None` today —
    /// nothing currently populates it honestly. `serde_json::Value` is
    /// available unconditionally here because the `evidence` feature
    /// requires `serde` (which pulls in `serde_json`).
    pub metadata: Option<serde_json::Value>,
}

/// Build retrieval evidence for one fetched page. Content and screenshot
/// remain mutually exclusive; byte-derived fields are never claimed for a
/// browser-produced representation.
pub fn build_evidence(
    page: &Page,
    content: Option<String>,
    wants_screenshot: bool,
    used_browser: bool,
) -> EvidenceBundle {
    let response_body_hash = (!used_browser)
        .then(|| page.get_bytes().map(sha256_hex))
        .flatten();
    let detected_content_type = if used_browser {
        None
    } else {
        page.get_bytes()
            .and_then(infer::get)
            .map(|kind| kind.mime_type().to_string())
    };
    let screenshot_bytes = page_screenshot_bytes(page);
    let transformed_content_hash = if wants_screenshot {
        None
    } else {
        content.as_deref().map(|text| sha256_hex(text.as_bytes()))
    };
    let screenshot_hash = wants_screenshot
        .then_some(screenshot_bytes)
        .flatten()
        .map(sha256_hex);
    let content_type = page
        .headers
        .as_ref()
        .and_then(|headers| headers.get("content-type"))
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let links = page.page_links.as_ref().map(|links| {
        links
            .iter()
            .map(|link| link.inner().to_string())
            .collect::<Vec<_>>()
    });

    EvidenceBundle {
        requested_url: Some(page.get_url().to_string()),
        final_url: Some(page.get_url_final().to_string()),
        retrieved_at: page.get_retrieved_at(),
        status_code: Some(page.status_code.as_u16()),
        observed_status_code: page.observed_status_code.map(|status| status.as_u16()),
        content_type,
        detected_content_type,
        response_body_hash,
        transformed_content_hash,
        content: (!wants_screenshot).then_some(content.clone()).flatten(),
        links,
        source: None,
        provider: None,
        query: None,
        screenshot: wants_screenshot.then_some(content).flatten(),
        screenshot_hash,
        metadata: None,
    }
}

/// Fetch exactly one page through Spider's ordinary non-browser HTTP path:
/// one target URL, no Chrome/browser fallback, no discovered-link
/// traversal. The canonical one-shot acquisition primitive shared by every
/// discovery adapter (feed/sitemap/news-sitemap/robots-sitemap) and any
/// evidence-first single-resource fetch, in both the MCP server and the
/// CLI.
pub async fn fetch_single_page(url: &str) -> Result<Page, String> {
    let mut website = Website::new(url);
    website.with_limit(1);
    let mut website = website.build().map_err(|_| "Invalid URL".to_string())?;
    let mut receiver = website.subscribe(1);
    tokio::spawn(async move {
        website.crawl_raw().await;
        website.unsubscribe();
    });
    receiver
        .recv()
        .await
        .map_err(|_| "Retrieval completed without producing a page".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only actually-known fields are populated; the rest remain absent
    /// (default `None`), never fabricated.
    #[test]
    fn only_known_fields_are_populated_rest_stay_none() {
        let bundle = EvidenceBundle {
            requested_url: Some("https://example.test/".to_string()),
            status_code: Some(200),
            ..Default::default()
        };
        assert_eq!(
            bundle.requested_url.as_deref(),
            Some("https://example.test/")
        );
        assert_eq!(bundle.status_code, Some(200));
        assert_eq!(bundle.final_url, None);
        assert_eq!(bundle.retrieved_at, None);
        assert_eq!(bundle.observed_status_code, None);
        assert_eq!(bundle.content_type, None);
        assert_eq!(bundle.detected_content_type, None);
        assert_eq!(bundle.response_body_hash, None);
        assert_eq!(bundle.transformed_content_hash, None);
        assert_eq!(bundle.content, None);
        assert_eq!(bundle.links, None);
        assert_eq!(bundle.source, None);
        assert_eq!(bundle.provider, None);
        assert_eq!(bundle.query, None);
        assert_eq!(bundle.screenshot, None);
        assert_eq!(bundle.screenshot_hash, None);
        assert_eq!(bundle.metadata, None);
    }

    /// requested_url and final_url are independent fields — a redirect
    /// changing one must never overwrite or conflate the other.
    #[test]
    fn requested_and_final_url_remain_independent() {
        let bundle = EvidenceBundle {
            requested_url: Some("https://example.test/".to_string()),
            final_url: Some("https://example.test/final".to_string()),
            ..Default::default()
        };
        assert_ne!(bundle.requested_url, bundle.final_url);
    }

    #[test]
    fn sha256_is_deterministic_and_byte_sensitive() {
        let bytes = b"scorpion evidence";
        assert_eq!(sha256_hex(bytes), sha256_hex(bytes));
        assert_ne!(sha256_hex(bytes), sha256_hex(b"scorpion evidencf"));
    }

    #[test]
    fn sha256_matches_known_vector_and_is_lowercase_hex() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[tokio::test]
    async fn fetch_single_page_rejects_invalid_url() {
        let result = fetch_single_page("not a url").await;
        assert!(result.is_err());
    }
}
