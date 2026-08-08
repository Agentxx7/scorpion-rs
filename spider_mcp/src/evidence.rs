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
    /// Unix epoch seconds of the actual page retrieval. Currently always
    /// `None` — no canonical retrieval wall-clock timestamp exists anywhere
    /// in the reachable scrape path today (see SCORPION.md §3's
    /// `retrieved_at` gap). An MCP-side `SystemTime::now()` captured at
    /// evidence-assembly time was tried and reverted
    /// (SCORPION_EVIDENCE_BUNDLE_001A): it only marks when this code
    /// happened to run, not when the network fetch completed, and labeling
    /// that approximation `retrieved_at` would overclaim provenance the
    /// runtime doesn't actually have. This field stays populated with real
    /// data only once Spider core exposes a genuine retrieval timestamp.
    pub retrieved_at: Option<u64>,
    /// HTTP status code of the response.
    pub status_code: Option<u16>,
    /// The response's `Content-Type` header, verbatim, when present.
    pub content_type: Option<String>,
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
            "content_type",
            "content",
            "links",
            "source",
            "provider",
            "query",
            "screenshot",
            "metadata",
        ] {
            assert!(v[field].is_null(), "field {field} should be null, got {:?}", v[field]);
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
        assert_eq!(bundle.requested_url.as_deref(), Some("https://example.test/"));
        assert_eq!(bundle.final_url.as_deref(), Some("https://example.test/final"));
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

    /// SCORPION_EVIDENCE_BUNDLE_001A: retrieved_at is null when no
    /// canonical retrieval timestamp exists — which is unconditionally
    /// true today, since nothing in this crate populates it. `Default`
    /// alone proves this: the struct offers no associated function to
    /// derive a value for the field (the earlier `now_unix()` helper that
    /// did was removed in this correction) — only explicit, caller-
    /// supplied data can set it.
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
}
