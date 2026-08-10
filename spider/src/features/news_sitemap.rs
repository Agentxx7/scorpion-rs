//! Network-independent Google News Sitemap parsing.

use crate::features::source::SourceItem;
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;
use std::fmt;

const NEWS_NS: &[u8] = b"http://www.google.com/schemas/sitemap-news/0.9";
const SITEMAP_NS: &[u8] = b"http://www.sitemaps.org/schemas/sitemap/0.9";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
/// Provider-declared metadata from one namespace-qualified `news:news` block.
pub struct NewsSitemapMetadata {
    /// `news:publication/news:name`, when present.
    pub publication_name: Option<String>,
    /// `news:publication/news:language`, when present.
    pub language: Option<String>,
    /// Exact non-empty `news:publication_date` text without normalization.
    pub publication_date_raw: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
/// One sitemap URL and News metadata structurally attached to that URL frame.
pub struct NewsSitemapEntry {
    /// Generic source-adapter representation of the URL.
    pub item: SourceItem,
    /// News metadata when this URL contained a genuine News namespace block.
    pub news: Option<NewsSitemapMetadata>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
/// Fully parsed News Sitemap discovery result.
pub struct NewsSitemapDiscoveryResult {
    /// Actual root classification: `urlset` or `sitemapindex`.
    pub sitemap_type: String,
    /// URL entries in document order; empty for a sitemap index.
    pub entries: Vec<NewsSitemapEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Contained failures from the blocking News Sitemap parser boundary.
pub enum NewsSitemapParseFailure {
    /// Well-formed XML whose root is neither `urlset` nor `sitemapindex`.
    NotSitemap(String),
    /// Structurally invalid XML or invalid required sitemap URL data.
    Parse(String),
    /// A contained parser panic or blocking-task join failure.
    Panicked(String),
}

impl fmt::Display for NewsSitemapParseFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotSitemap(message) | Self::Parse(message) | Self::Panicked(message) => {
                f.write_str(message)
            }
        }
    }
}

impl std::error::Error for NewsSitemapParseFailure {}

/// Parse already-fetched bytes on a panic-contained blocking task.
pub async fn parse(
    bytes: &[u8],
    sitemap_url: &str,
) -> Result<NewsSitemapDiscoveryResult, NewsSitemapParseFailure> {
    let bytes = bytes.to_vec();
    let sitemap_url = sitemap_url.to_string();
    tokio::task::spawn_blocking(move || {
        std::panic::catch_unwind(|| parse_sync(&bytes, &sitemap_url)).map_err(|payload| {
            let message = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("unknown parser panic");
            NewsSitemapParseFailure::Panicked(format!("news sitemap parser panicked: {message}"))
        })?
    })
    .await
    .map_err(|error| {
        NewsSitemapParseFailure::Panicked(format!("news sitemap parser task failed: {error}"))
    })?
}

#[derive(Default)]
struct UrlFrame {
    url_depth: usize,
    loc: Option<String>,
    lastmod: Option<String>,
    news_present: bool,
    news_depth: Option<usize>,
    publication_depth: Option<usize>,
    publication_name: Option<String>,
    language: Option<String>,
    publication_date_raw: Option<String>,
    title: Option<String>,
}

#[derive(Clone, Copy)]
enum TextField {
    Loc,
    Lastmod,
    PublicationName,
    Language,
    PublicationDate,
    Title,
}

#[derive(Clone, Copy)]
struct ActiveField {
    kind: TextField,
    depth: usize,
}

fn is_ns(namespace: &ResolveResult<'_>, expected: &[u8]) -> bool {
    matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == expected)
}

fn is_standard(namespace: &ResolveResult<'_>) -> bool {
    matches!(namespace, ResolveResult::Unbound) || is_ns(namespace, SITEMAP_NS)
}

fn parse_sync(
    bytes: &[u8],
    sitemap_url: &str,
) -> Result<NewsSitemapDiscoveryResult, NewsSitemapParseFailure> {
    let mut reader = NsReader::from_reader(bytes);
    reader.config_mut().trim_text(false);
    let mut root: Option<String> = None;
    let mut frame: Option<UrlFrame> = None;
    let mut field: Option<ActiveField> = None;
    let mut entries = Vec::new();
    let mut depth = 0_usize;
    let mut root_closed = false;

    loop {
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| NewsSitemapParseFailure::Parse(error.to_string()))?;
        match event {
            Event::Start(element) => {
                if depth == 0 && root.is_some() {
                    return Err(NewsSitemapParseFailure::Parse(
                        "news sitemap XML contains multiple root elements".into(),
                    ));
                }
                depth += 1;
                let local = element.local_name();
                if root.is_none() {
                    root = Some(match local.as_ref() {
                        b"urlset" if is_standard(&namespace) => "urlset".into(),
                        b"sitemapindex" if is_standard(&namespace) => "sitemapindex".into(),
                        _ => "other".into(),
                    });
                } else if root.as_deref() == Some("urlset")
                    && frame.is_none()
                    && depth == 2
                    && local.as_ref() == b"url"
                    && is_standard(&namespace)
                {
                    frame = Some(UrlFrame {
                        url_depth: depth,
                        ..Default::default()
                    });
                } else if let Some(current) = frame.as_mut() {
                    if local.as_ref() == b"news" && is_ns(&namespace, NEWS_NS) {
                        current.news_present = true;
                        current.news_depth = Some(depth);
                    }
                    if local.as_ref() == b"publication"
                        && current.news_depth.is_some()
                        && is_ns(&namespace, NEWS_NS)
                    {
                        current.publication_depth = Some(depth);
                    }
                    let kind = match local.as_ref() {
                        b"loc" if is_standard(&namespace) => Some(TextField::Loc),
                        b"lastmod" if is_standard(&namespace) => Some(TextField::Lastmod),
                        b"name"
                            if current.publication_depth.is_some()
                                && is_ns(&namespace, NEWS_NS) =>
                        {
                            Some(TextField::PublicationName)
                        }
                        b"language"
                            if current.publication_depth.is_some()
                                && is_ns(&namespace, NEWS_NS) =>
                        {
                            Some(TextField::Language)
                        }
                        b"publication_date"
                            if current.news_depth.is_some() && is_ns(&namespace, NEWS_NS) =>
                        {
                            Some(TextField::PublicationDate)
                        }
                        b"title" if current.news_depth.is_some() && is_ns(&namespace, NEWS_NS) => {
                            Some(TextField::Title)
                        }
                        _ => None,
                    };
                    field = kind.map(|kind| ActiveField { kind, depth });
                }
            }
            Event::Empty(element) => {
                if root.is_none() {
                    root = Some(match element.local_name().as_ref() {
                        b"urlset" if is_standard(&namespace) => "urlset".into(),
                        b"sitemapindex" if is_standard(&namespace) => "sitemapindex".into(),
                        _ => "other".into(),
                    });
                    root_closed = true;
                } else if depth == 0 {
                    return Err(NewsSitemapParseFailure::Parse(
                        "news sitemap XML contains multiple root elements".into(),
                    ));
                } else if let Some(current) = frame.as_mut() {
                    if element.local_name().as_ref() == b"news" && is_ns(&namespace, NEWS_NS) {
                        current.news_present = true;
                    }
                }
            }
            Event::Text(text) => {
                if let (Some(current), Some(active)) = (frame.as_mut(), field) {
                    let decoded = text
                        .decode()
                        .map_err(|error| NewsSitemapParseFailure::Parse(error.to_string()))?;
                    let value = quick_xml::escape::unescape(&decoded)
                        .map_err(|error| NewsSitemapParseFailure::Parse(error.to_string()))?;
                    append_text(current, active.kind, &value);
                }
            }
            Event::CData(text) => {
                if let (Some(current), Some(active)) = (frame.as_mut(), field) {
                    let value = text
                        .decode()
                        .map_err(|error| NewsSitemapParseFailure::Parse(error.to_string()))?;
                    append_text(current, active.kind, &value);
                }
            }
            Event::GeneralRef(reference) => {
                if let (Some(current), Some(active)) = (frame.as_mut(), field) {
                    let reference = reference
                        .decode()
                        .map_err(|error| NewsSitemapParseFailure::Parse(error.to_string()))?;
                    let encoded = format!("&{reference};");
                    let decoded = quick_xml::escape::unescape(&encoded)
                        .map_err(|error| NewsSitemapParseFailure::Parse(error.to_string()))?;
                    append_text(current, active.kind, &decoded);
                }
            }
            Event::End(element) => {
                if field.is_some_and(|active| active.depth == depth) {
                    if let (Some(current), Some(active)) = (frame.as_mut(), field) {
                        trim_field(current, active.kind);
                    }
                    field = None;
                }
                if element.local_name().as_ref() == b"url"
                    && frame
                        .as_ref()
                        .is_some_and(|current| current.url_depth == depth)
                {
                    entries.push(finish_frame(frame.take().unwrap(), sitemap_url)?);
                }
                if let Some(current) = frame.as_mut() {
                    if current.publication_depth == Some(depth) {
                        current.publication_depth = None;
                    }
                    if current.news_depth == Some(depth) {
                        current.news_depth = None;
                    }
                }
                depth = depth.checked_sub(1).ok_or_else(|| {
                    NewsSitemapParseFailure::Parse("unexpected closing XML element".into())
                })?;
                if depth == 0 {
                    root_closed = true;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    if depth != 0 || !root_closed {
        return Err(NewsSitemapParseFailure::Parse(
            "news sitemap XML ended before all elements were closed".into(),
        ));
    }

    match root.as_deref() {
        Some("urlset") => Ok(NewsSitemapDiscoveryResult {
            sitemap_type: "urlset".into(),
            entries,
        }),
        Some("sitemapindex") => Ok(NewsSitemapDiscoveryResult {
            sitemap_type: "sitemapindex".into(),
            entries: Vec::new(),
        }),
        Some("other") => Err(NewsSitemapParseFailure::NotSitemap(
            "the fetched XML document is not a sitemap".into(),
        )),
        _ => Err(NewsSitemapParseFailure::Parse(
            "news sitemap XML has no root element".into(),
        )),
    }
}

fn field_value(frame: &mut UrlFrame, field: TextField) -> &mut Option<String> {
    match field {
        TextField::Loc => &mut frame.loc,
        TextField::Lastmod => &mut frame.lastmod,
        TextField::PublicationName => &mut frame.publication_name,
        TextField::Language => &mut frame.language,
        TextField::PublicationDate => &mut frame.publication_date_raw,
        TextField::Title => &mut frame.title,
    }
}

fn append_text(frame: &mut UrlFrame, field: TextField, fragment: &str) {
    field_value(frame, field)
        .get_or_insert_with(String::new)
        .push_str(fragment);
}

fn trim_field(frame: &mut UrlFrame, field: TextField) {
    let value = field_value(frame, field);
    *value = value
        .take()
        .and_then(|text| (!text.trim().is_empty()).then(|| text.trim().to_string()));
}

fn finish_frame(
    frame: UrlFrame,
    sitemap_url: &str,
) -> Result<NewsSitemapEntry, NewsSitemapParseFailure> {
    let loc = frame
        .loc
        .ok_or_else(|| NewsSitemapParseFailure::Parse("sitemap url entry is missing loc".into()))?;
    url::Url::parse(&loc).map_err(|_| {
        NewsSitemapParseFailure::Parse("sitemap url entry has an invalid loc".into())
    })?;
    let published_at = frame
        .publication_date_raw
        .as_deref()
        .and_then(publication_timestamp_millis);
    let updated_at = frame.lastmod.as_deref().and_then(lastmod_timestamp_millis);
    let news = frame.news_present.then_some(NewsSitemapMetadata {
        publication_name: frame.publication_name,
        language: frame.language,
        publication_date_raw: frame.publication_date_raw,
    });
    Ok(NewsSitemapEntry {
        item: SourceItem {
            source_type: "sitemap".into(),
            source_item_id: None,
            url: Some(loc),
            title: frame.title,
            snippet: None,
            authors: Vec::new(),
            published_at,
            updated_at,
            discovered_via: Some(sitemap_url.to_string()),
            media_references: Vec::new(),
        },
        news,
    })
}

fn publication_timestamp_millis(raw: &str) -> Option<u64> {
    if NaiveDate::parse_from_str(raw, "%Y-%m-%d").is_ok() {
        return None;
    }
    DateTime::parse_from_rfc3339(raw)
        .or_else(|_| DateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M%:z"))
        .ok()
        .and_then(|value| u64::try_from(value.timestamp_millis()).ok())
}

fn lastmod_timestamp_millis(raw: &str) -> Option<u64> {
    let millis = DateTime::parse_from_rfc3339(raw)
        .map(|value| value.timestamp_millis())
        .or_else(|_| {
            NaiveDate::parse_from_str(raw, "%Y-%m-%d").map(|date| {
                Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
                    .timestamp_millis()
            })
        })
        .ok()?;
    u64::try_from(millis).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:news="http://www.google.com/schemas/sitemap-news/0.9">"#;

    async fn parsed(inner: &str) -> NewsSitemapDiscoveryResult {
        parse(
            format!("{HEAD}{inner}</urlset>").as_bytes(),
            "https://example.test/news.xml",
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn complete_partial_ordinary_duplicate_and_order_are_structural() {
        let result = parsed(r#"
          <url><loc>https://example.test/a</loc><lastmod>2024-02-03</lastmod><news:news><news:publication><news:name>Paper A</news:name><news:language>en</news:language></news:publication><news:publication_date>2026-08-09T10:30+02:00</news:publication_date><news:title>Title X</news:title></news:news></url>
          <url><loc>https://example.test/b</loc><news:news><news:title>Only title</news:title></news:news></url>
          <url><loc>https://example.test/c</loc></url>
          <url><loc>https://example.test/a</loc><news:news><news:title>Title Y</news:title></news:news></url>"#).await;
        assert_eq!(result.entries.len(), 4);
        let first = &result.entries[0];
        assert_eq!(first.item.title.as_deref(), Some("Title X"));
        assert_eq!(
            first.news.as_ref().unwrap().publication_name.as_deref(),
            Some("Paper A")
        );
        assert_eq!(first.news.as_ref().unwrap().language.as_deref(), Some("en"));
        assert!(first.item.published_at.is_some());
        assert!(first.item.updated_at.is_some());
        assert_ne!(first.item.published_at, first.item.updated_at);
        assert_eq!(
            result.entries[1].news.as_ref().unwrap().publication_name,
            None
        );
        assert_eq!(result.entries[2].news, None);
        assert_eq!(result.entries[0].item.url, result.entries[3].item.url);
        assert_eq!(result.entries[3].item.title.as_deref(), Some("Title Y"));
    }

    #[tokio::test]
    async fn publication_date_forms_preserve_raw_and_only_exact_instants_map() {
        for (raw, has_timestamp) in [
            ("2026-08-09", false),
            ("2026-08-09T10:30+02:00", true),
            ("2026-08-09T10:30:15Z", true),
            ("2026-08-09T10:30:15.250+02:00", true),
            ("banana", false),
            ("1899-01-01T00:00:00Z", false),
        ] {
            let result = parsed(&format!(r#"<url><loc>https://example.test/{}</loc><news:news><news:publication_date>{raw}</news:publication_date></news:news></url>"#, raw.len())).await;
            let entry = &result.entries[0];
            assert_eq!(
                entry.news.as_ref().unwrap().publication_date_raw.as_deref(),
                Some(raw)
            );
            assert_eq!(entry.item.published_at.is_some(), has_timestamp, "{raw}");
        }
    }

    #[tokio::test]
    async fn namespace_uri_not_prefix_controls_news_recognition() {
        let xml = r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:news="http://www.google.com/schemas/sitemap-news/0.9" xmlns:n="http://www.google.com/schemas/sitemap-news/0.9" xmlns:foo="urn:not-news"><url><loc>https://example.test/a</loc><news:news><news:title>A</news:title></news:news></url><url><loc>https://example.test/b</loc><n:news><n:title>B</n:title></n:news></url><url><loc>https://example.test/c</loc><foo:news><foo:title>Fake</foo:title><foo:publication_date>2026-08-09T10:30:15Z</foo:publication_date></foo:news></url></urlset>"#;
        let result = parse(xml.as_bytes(), "https://example.test/news.xml")
            .await
            .unwrap();
        assert_eq!(result.entries[0].item.title.as_deref(), Some("A"));
        assert_eq!(result.entries[1].item.title.as_deref(), Some("B"));
        assert_eq!(result.entries[2].news, None);
        assert_eq!(result.entries[2].item.title, None);
        assert_eq!(result.entries[2].item.published_at, None);
    }

    #[tokio::test]
    async fn empty_genuine_news_block_is_present_but_incomplete() {
        let result = parsed(r#"<url><loc>https://example.test/a</loc><news:news/></url>"#).await;
        assert_eq!(result.entries[0].news, Some(NewsSitemapMetadata::default()));
    }

    #[tokio::test]
    async fn escaped_loc_and_news_title_entities_are_decoded_without_truncation() {
        let result = parsed(r#"<url><loc>https://example.test/a?x=1&amp;y=2</loc><news:news><news:title>Barnes &amp; Noble opens store</news:title></news:news></url>"#).await;
        assert_eq!(
            result.entries[0].item.url.as_deref(),
            Some("https://example.test/a?x=1&y=2")
        );
        assert_eq!(
            result.entries[0].item.title.as_deref(),
            Some("Barnes & Noble opens store")
        );
    }

    #[tokio::test]
    async fn mixed_text_cdata_and_numeric_reference_accumulate_in_source_order() {
        let result = parsed(r#"<url><loc>https://example.test/a</loc><news:news><news:title>Part A <![CDATA[Part B]]> Part C &#38; Part D</news:title></news:news></url>"#).await;
        assert_eq!(
            result.entries[0].item.title.as_deref(),
            Some("Part A Part B Part C & Part D")
        );
    }

    #[tokio::test]
    async fn fragments_remain_scoped_to_each_field_and_url_frame() {
        let result = parsed(r#"<url><loc>https://example.test/a</loc><lastmod>2024-02-<![CDATA[03]]></lastmod><news:news><news:publication><news:name>Paper <![CDATA[Name]]></news:name><news:language>e<![CDATA[n]]></news:language></news:publication><news:publication_date>2026-08-09T10:<![CDATA[30]]>+02:00</news:publication_date><news:title>First</news:title></news:news></url><url><loc>https://example.test/b</loc><news:news><news:title>Second</news:title></news:news></url>"#).await;
        let first = &result.entries[0];
        assert!(first.item.updated_at.is_some());
        assert!(first.item.published_at.is_some());
        let news = first.news.as_ref().unwrap();
        assert_eq!(news.publication_name.as_deref(), Some("Paper Name"));
        assert_eq!(news.language.as_deref(), Some("en"));
        assert_eq!(
            news.publication_date_raw.as_deref(),
            Some("2026-08-09T10:30+02:00")
        );
        assert_eq!(result.entries[1].item.title.as_deref(), Some("Second"));
        assert_eq!(result.entries[1].news.as_ref().unwrap().language, None);
    }

    #[tokio::test]
    async fn paired_and_self_closing_second_roots_are_structural_parse_failures() {
        for trailing in ["<evil></evil>", "<evil/>"] {
            let xml = format!(
                r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"></urlset>{trailing}"#
            );
            assert!(matches!(
                parse(xml.as_bytes(), "x").await.unwrap_err(),
                NewsSitemapParseFailure::Parse(_)
            ));
        }
    }

    #[tokio::test]
    async fn roots_and_structural_errors_are_classified_after_full_parse() {
        let index = parse(b"<sitemapindex xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\"><sitemap><loc>https://example.test/child.xml</loc></sitemap></sitemapindex>", "x").await.unwrap();
        assert_eq!(index.sitemap_type, "sitemapindex");
        assert!(index.entries.is_empty());
        assert!(matches!(
            parse(b"<document/>", "x").await.unwrap_err(),
            NewsSitemapParseFailure::NotSitemap(_)
        ));
        for xml in [
            b"<urlset><url>".as_slice(),
            b"<urlset><url><loc>https://example.test/a</loc></url><url>".as_slice(),
        ] {
            assert!(matches!(
                parse(xml, "x").await.unwrap_err(),
                NewsSitemapParseFailure::Parse(_)
            ));
        }
    }

    #[cfg(feature = "sitemap")]
    #[tokio::test]
    async fn lastmod_matches_standard_sitemap_for_date_and_timestamp() {
        for lastmod in ["2024-02-03", "2024-02-03T12:34:56+02:00"] {
            let xml = format!(
                r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><url><loc>https://example.test/a</loc><lastmod>{lastmod}</lastmod></url></urlset>"#
            );
            let standard = crate::features::sitemap::parse(xml.as_bytes(), "x")
                .await
                .unwrap();
            let news = parse(xml.as_bytes(), "x").await.unwrap();
            assert_eq!(
                news.entries[0].item.updated_at, standard.entries[0].updated_at,
                "{lastmod}"
            );
        }
    }
}
