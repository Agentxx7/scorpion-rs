//! Scorpion's first concrete evidence/provenance representation
//! (SCORPION.md §3, "Evidence-First Direction").
//!
//! The actual `EvidenceBundle` type and its constructor now live in
//! `spider::utils::evidence` — the canonical core seam shared by this MCP
//! server and the CLI, so neither independently reimplements
//! acquisition/evidence logic. This module re-exports them under their
//! established crate-local names so every existing call site in
//! `spider_mcp` (`crate::evidence::{EvidenceBundle, build_evidence,
//! sha256_hex}`) keeps working unchanged.

pub use spider::utils::evidence::{build_evidence, EvidenceBundle};

#[cfg(test)]
mod tests {
    use super::*;
    use spider::utils::evidence::sha256_hex;

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
