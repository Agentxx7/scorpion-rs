//! Parse standard sitemap documents into generic discovery candidates.
//!
//! This module owns no network or crawler behavior. Callers provide the exact
//! bytes already retrieved and the sitemap URL used for discovery.

use crate::features::source::SourceItem;
use sitemap::reader::{SiteMapEntity, SiteMapReader};
use std::fmt;

/// A child sitemap document declared by a sitemap index.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct SitemapReference {
    /// Provider-declared child sitemap URL.
    pub url: String,
    /// Provider-declared last modification time as Unix epoch milliseconds.
    pub updated_at: Option<u64>,
}

/// Normalized result of parsing a standard sitemap document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct SitemapDiscoveryResult {
    /// `urlset` or `sitemapindex`, determined from the XML root.
    pub sitemap_type: String,
    /// Content/resource candidates from a urlset, in source order.
    pub entries: Vec<SourceItem>,
    /// Child sitemap candidates from a sitemapindex, in source order.
    pub child_sitemaps: Vec<SitemapReference>,
}

/// Explicit failure at the contained sitemap parser boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SitemapParseFailure {
    /// The input is XML but its root is not a supported sitemap root.
    NotSitemap(String),
    /// XML or sitemap entity parsing failed.
    Parse(String),
    /// The blocking parser task panicked or could not be joined.
    Panicked(String),
}

impl fmt::Display for SitemapParseFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotSitemap(message) | Self::Parse(message) | Self::Panicked(message) => {
                f.write_str(message)
            }
        }
    }
}

impl std::error::Error for SitemapParseFailure {}

/// Parse already-fetched bytes on a panic-contained blocking task.
pub async fn parse(
    bytes: &[u8],
    sitemap_url: &str,
) -> Result<SitemapDiscoveryResult, SitemapParseFailure> {
    let bytes = bytes.to_vec();
    let sitemap_url = sitemap_url.to_string();
    tokio::task::spawn_blocking(move || {
        std::panic::catch_unwind(|| parse_sync(&bytes, &sitemap_url)).map_err(|payload| {
            let message = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("unknown parser panic");
            SitemapParseFailure::Panicked(format!("sitemap parser panicked: {message}"))
        })?
    })
    .await
    .map_err(|error| {
        SitemapParseFailure::Panicked(format!("sitemap parser task failed: {error}"))
    })?
}

fn parse_sync(
    bytes: &[u8],
    sitemap_url: &str,
) -> Result<SitemapDiscoveryResult, SitemapParseFailure> {
    let sitemap_type = classify_root(bytes)?;
    let mut entries = Vec::new();
    let mut child_sitemaps = Vec::new();

    for entity in SiteMapReader::new(bytes) {
        match entity {
            SiteMapEntity::Url(entry) => {
                if sitemap_type != "urlset" {
                    return Err(SitemapParseFailure::Parse(
                        "sitemapindex contained an unexpected url entry".into(),
                    ));
                }
                let url = entry.loc.get_url().ok_or_else(|| {
                    SitemapParseFailure::Parse("sitemap url entry has an invalid loc".into())
                })?;
                entries.push(SourceItem {
                    source_type: "sitemap".into(),
                    source_item_id: None,
                    url: Some(url.to_string()),
                    title: None,
                    snippet: None,
                    authors: Vec::new(),
                    published_at: None,
                    updated_at: timestamp_millis(&entry.lastmod),
                    discovered_via: sitemap_url.to_string(),
                    media_references: Vec::new(),
                });
            }
            SiteMapEntity::SiteMap(entry) => {
                if sitemap_type != "sitemapindex" {
                    return Err(SitemapParseFailure::Parse(
                        "urlset contained an unexpected sitemap entry".into(),
                    ));
                }
                let url = entry.loc.get_url().ok_or_else(|| {
                    SitemapParseFailure::Parse("sitemap index entry has an invalid loc".into())
                })?;
                child_sitemaps.push(SitemapReference {
                    url: url.to_string(),
                    updated_at: timestamp_millis(&entry.lastmod),
                });
            }
            SiteMapEntity::Err(error) => {
                return Err(SitemapParseFailure::Parse(error.to_string()));
            }
        }
    }

    Ok(SitemapDiscoveryResult {
        sitemap_type,
        entries,
        child_sitemaps,
    })
}

fn classify_root(bytes: &[u8]) -> Result<String, SitemapParseFailure> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_reader(bytes);
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                return match element.local_name().as_ref() {
                    b"urlset" => Ok("urlset".into()),
                    b"sitemapindex" => Ok("sitemapindex".into()),
                    _ => Err(SitemapParseFailure::NotSitemap(
                        "the fetched XML document is not a standard sitemap".into(),
                    )),
                };
            }
            Ok(Event::Eof) => {
                return Err(SitemapParseFailure::Parse(
                    "sitemap XML has no root element".into(),
                ));
            }
            Err(error) => return Err(SitemapParseFailure::Parse(error.to_string())),
            _ => {}
        }
    }
}

fn timestamp_millis(lastmod: &sitemap::structs::LastMod) -> Option<u64> {
    lastmod
        .get_time()
        .and_then(|value| u64::try_from(value.timestamp_millis()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    const URLSET: &str = r#"<?xml version="1.0"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>https://example.test/a</loc><lastmod>2024-01-02T03:04:05Z</lastmod></url>
  <url><loc>https://example.test/b</loc></url>
  <url><loc>https://example.test/a</loc></url>
  <url><loc>https://example.test/image.jpg</loc></url>
</urlset>"#;

    const INDEX: &str = r#"<?xml version="1.0"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <sitemap><loc>https://example.test/child-a.xml</loc><lastmod>2024-02-03</lastmod></sitemap>
  <sitemap><loc>https://example.test/child-b.xml</loc></sitemap>
</sitemapindex>"#;

    #[tokio::test]
    async fn urlset_maps_truthful_fields_preserving_duplicates_and_order() {
        let result = parse(URLSET.as_bytes(), "https://example.test/sitemap.xml")
            .await
            .unwrap();
        assert_eq!(result.sitemap_type, "urlset");
        assert!(result.child_sitemaps.is_empty());
        assert_eq!(result.entries.len(), 4);
        assert_eq!(
            result
                .entries
                .iter()
                .map(|item| item.url.as_deref().unwrap())
                .collect::<Vec<_>>(),
            [
                "https://example.test/a",
                "https://example.test/b",
                "https://example.test/a",
                "https://example.test/image.jpg",
            ]
        );
        let first = &result.entries[0];
        assert_eq!(first.source_type, "sitemap");
        assert_eq!(first.source_item_id, None);
        assert_eq!(first.title, None);
        assert_eq!(first.snippet, None);
        assert!(first.authors.is_empty());
        assert_eq!(first.published_at, None);
        assert!(first.updated_at.is_some());
        assert_eq!(result.entries[1].updated_at, None);
        assert_eq!(first.discovered_via, "https://example.test/sitemap.xml");
        assert!(first.media_references.is_empty());
    }

    #[tokio::test]
    async fn sitemapindex_returns_only_ordered_child_references() {
        let result = parse(INDEX.as_bytes(), "https://example.test/index.xml")
            .await
            .unwrap();
        assert_eq!(result.sitemap_type, "sitemapindex");
        assert!(result.entries.is_empty());
        assert_eq!(result.child_sitemaps.len(), 2);
        assert_eq!(
            result.child_sitemaps[0].url,
            "https://example.test/child-a.xml"
        );
        assert!(result.child_sitemaps[0].updated_at.is_some());
        assert_eq!(
            result.child_sitemaps[1].url,
            "https://example.test/child-b.xml"
        );
        assert_eq!(result.child_sitemaps[1].updated_at, None);
    }

    #[tokio::test]
    async fn empty_supported_roots_succeed_with_truthful_type() {
        for (xml, expected) in [
            ("<urlset></urlset>", "urlset"),
            ("<sitemapindex></sitemapindex>", "sitemapindex"),
        ] {
            let result = parse(xml.as_bytes(), "https://example.test/sitemap.xml")
                .await
                .unwrap();
            assert_eq!(result.sitemap_type, expected);
            assert!(result.entries.is_empty());
            assert!(result.child_sitemaps.is_empty());
        }
    }

    #[tokio::test]
    async fn non_sitemap_and_malformed_documents_fail_without_partial_success() {
        assert!(matches!(
            parse(b"<document></document>", "https://example.test/x")
                .await
                .unwrap_err(),
            SitemapParseFailure::NotSitemap(_)
        ));
        for malformed in [
            "<?xml version=\"1.0\" <urlset></urlset>",
            "<urlset><url><loc>https://example.test/a</loc></url><url>",
        ] {
            assert!(matches!(
                parse(malformed.as_bytes(), "https://example.test/x")
                    .await
                    .unwrap_err(),
                SitemapParseFailure::Parse(_)
            ));
        }
    }

    #[tokio::test]
    async fn news_extension_does_not_fabricate_news_metadata() {
        let xml = r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:news="http://www.google.com/schemas/sitemap-news/0.9"><url><loc>https://example.test/article</loc><news:news><news:title>Not mapped</news:title><news:publication_date>2024-01-01</news:publication_date></news:news></url></urlset>"#;
        let result = parse(xml.as_bytes(), "https://example.test/news.xml")
            .await
            .unwrap();
        assert_eq!(
            result.entries[0].url.as_deref(),
            Some("https://example.test/article")
        );
        assert_eq!(result.entries[0].title, None);
        assert_eq!(result.entries[0].published_at, None);
        assert!(result.entries[0].authors.is_empty());
        assert_eq!(result.entries[0].snippet, None);
    }
}
