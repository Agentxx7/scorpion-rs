//! Generic vocabulary shared by source-discovery adapters.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
/// One provider-declared item discovered by a source adapter.
pub struct SourceItem {
    /// Stable adapter family that produced this item.
    pub source_type: String,
    /// Provider-native item identifier, when genuinely supplied.
    pub source_item_id: Option<String>,
    /// Provider-declared article or resource URL.
    pub url: Option<String>,
    /// Provider-declared title.
    pub title: Option<String>,
    /// Provider-declared summary or description.
    pub snippet: Option<String>,
    /// Provider-declared author names in source order.
    pub authors: Vec<String>,
    /// Provider-declared publication time as Unix epoch milliseconds.
    pub published_at: Option<u64>,
    /// Provider-declared update time as Unix epoch milliseconds.
    pub updated_at: Option<u64>,
    /// Exact source URL from which this item was discovered.
    pub discovered_via: String,
    /// Provider-declared media associated with this item.
    pub media_references: Vec<MediaReference>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
/// Provider-declared media associated with a source item.
pub struct MediaReference {
    /// Direct provider-declared media URL.
    pub url: String,
    /// Provider-declared MIME type.
    pub mime_type: Option<String>,
    /// Provider-declared media caption or title.
    pub caption: Option<String>,
    /// Provider-declared width in pixels.
    pub width: Option<u32>,
    /// Provider-declared height in pixels.
    pub height: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_source_values_stay_empty_or_none() {
        let item = SourceItem::default();
        assert!(item.authors.is_empty());
        assert!(item.media_references.is_empty());
        assert_eq!(item.source_item_id, None);
        assert_eq!(item.url, None);
        assert_eq!(item.published_at, None);
        assert_eq!(item.updated_at, None);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serialization_is_discovery_only() {
        let value = serde_json::to_value(SourceItem {
            source_type: "feed".into(),
            discovered_via: "https://example.test/feed.xml".into(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(value["source_type"], "feed");
        assert_eq!(value["discovered_via"], "https://example.test/feed.xml");
        for forbidden in [
            "retrieved_at",
            "status_code",
            "observed_status_code",
            "response_body_hash",
            "screenshot",
            "evidence",
        ] {
            assert!(
                value.get(forbidden).is_none(),
                "unexpected field {forbidden}"
            );
        }
    }
}
