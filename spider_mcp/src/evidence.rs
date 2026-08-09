//! Scorpion's first concrete evidence/provenance representation
//! (SCORPION.md §3, "Evidence-First Direction"). `EvidenceBundle` is
//! deliberately minimal and purely additive: every field is `Option`,
//! populated only when the underlying data was actually observed during a
//! fetch — never fabricated or guessed. `EvidenceId`/`ArtifactId` are
//! intentionally NOT represented here — SCORPION.md §3 has not locked a
//! representation for either yet, and this bundle does not invent one.
//!
//! This is plumbing, not a new capability: every field it can populate is
//! sourced from data Spider/Scorpion already capture (`Page`,
//! `spider_transformations`, the existing screenshot pipeline) — nothing
//! here changes crawling, fetching, or rendering behavior.

use serde::Serialize;
use sha2::{Digest, Sha256};

/// SHA-256 of exactly the supplied bytes, encoded as lowercase hexadecimal.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Debug, Clone, Default, Serialize)]
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
    /// textual content (SCORPION.md §9.3's `PAGE_SCREENSHOT` distinction).
    pub screenshot: Option<String>,
    /// SHA-256 of the original captured PNG bytes, never its base64 encoding.
    pub screenshot_hash: Option<String>,
    /// Reserved for future structured metadata. Always `None` today —
    /// nothing currently populates it honestly.
    pub metadata: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1/2. Only actually-known fields are populated; the rest remain
    /// absent (null), never fabricated.
    #[test]
    fn only_known_fields_are_populated_rest_are_null() {
        let bundle = EvidenceBundle {
            requested_url: Some("https://example.test/".to_string()),
            status_code: Some(200),
            ..Default::default()
        };

        let v = serde_json::to_value(&bundle).unwrap();
        assert_eq!(v["requested_url"], "https://example.test/");
        assert_eq!(v["status_code"], 200);

        for field in [
            "final_url",
            "retrieved_at",
            "observed_status_code",
            "content_type",
            "detected_content_type",
            "response_body_hash",
            "transformed_content_hash",
            "content",
            "links",
            "source",
            "provider",
            "query",
            "screenshot",
            "screenshot_hash",
            "metadata",
        ] {
            assert!(
                v[field].is_null(),
                "field {field} should be null, got {:?}",
                v[field]
            );
        }
    }

    /// 3. requested_url and final_url are independent fields — a redirect
    /// changing one must never overwrite or conflate the other.
    #[test]
    fn requested_and_final_url_remain_independent() {
        let bundle = EvidenceBundle {
            requested_url: Some("https://example.test/".to_string()),
            final_url: Some("https://example.test/final".to_string()),
            ..Default::default()
        };
        assert_eq!(
            bundle.requested_url.as_deref(),
            Some("https://example.test/")
        );
        assert_eq!(
            bundle.final_url.as_deref(),
            Some("https://example.test/final")
        );
        assert_ne!(bundle.requested_url, bundle.final_url);
    }

    /// Same-URL case: no redirect occurred, both fields still populated
    /// (not omitted), and correctly equal — this is the "not conflated"
    /// contract holding in the no-redirect case too.
    #[test]
    fn requested_and_final_url_can_be_equal_without_conflation() {
        let bundle = EvidenceBundle {
            requested_url: Some("https://example.test/".to_string()),
            final_url: Some("https://example.test/".to_string()),
            ..Default::default()
        };
        assert_eq!(bundle.requested_url, bundle.final_url);
        // Still two independently-set fields, not one shared value.
        let v = serde_json::to_value(&bundle).unwrap();
        assert!(!v["requested_url"].is_null());
        assert!(!v["final_url"].is_null());
    }

    /// 6. Content survives without mutation when passed through verbatim.
    #[test]
    fn content_field_is_not_mutated() {
        let original = "# Title\n\nSome *markdown* content.".to_string();
        let bundle = EvidenceBundle {
            content: Some(original.clone()),
            ..Default::default()
        };
        assert_eq!(bundle.content.as_deref(), Some(original.as_str()));
    }

    /// 5. Links survive where available, order preserved.
    #[test]
    fn links_survive_in_order() {
        let bundle = EvidenceBundle {
            links: Some(vec![
                "https://a.example/".to_string(),
                "https://b.example/".to_string(),
            ]),
            ..Default::default()
        };
        let v = serde_json::to_value(&bundle).unwrap();
        assert_eq!(v["links"][0], "https://a.example/");
        assert_eq!(v["links"][1], "https://b.example/");
    }

    /// A bundle without a live-fetch completion timestamp must serialize
    /// `retrieved_at` as null rather than fabricating one during assembly.
    #[test]
    fn retrieved_at_is_null_by_default() {
        let bundle = EvidenceBundle {
            requested_url: Some("https://example.test/".to_string()),
            status_code: Some(200),
            ..Default::default()
        };
        assert_eq!(bundle.retrieved_at, None);
        let v = serde_json::to_value(&bundle).unwrap();
        assert!(v["retrieved_at"].is_null());
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
}
