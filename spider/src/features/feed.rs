//! Parse RSS and Atom bytes into generic source-discovery items.
//!
//! This module deliberately owns no network behavior. Callers supply the
//! exact bytes already retrieved and the feed URL used for discovery.

use crate::features::source::{MediaReference, SourceItem};
use feed_rs::model::{Entry, FeedType, MediaObject};
use std::fmt;

const MISSING_ID: &str = "scorpion:feed-rs:missing-native-id";

/// Feed metadata and entries normalized for source discovery.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct FeedDiscoveryResult {
    /// Source representation family reported by the parser.
    pub feed_type: String,
    /// Provider-declared feed title.
    pub feed_title: Option<String>,
    /// Provider-declared feed authors in source order.
    pub feed_authors: Vec<String>,
    /// Provider-declared feed language.
    pub language: Option<String>,
    /// Entries in provider order.
    pub entries: Vec<SourceItem>,
}

/// Explicit failure at the contained parser boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeedParseFailure {
    /// The input parsed as a supported document, but not RSS or Atom.
    NotFeed(String),
    /// feed-rs rejected the representation.
    Parse(String),
    /// The blocking parser task panicked or could not be joined.
    Panicked(String),
}

impl fmt::Display for FeedParseFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFeed(message) | Self::Parse(message) | Self::Panicked(message) => {
                f.write_str(message)
            }
        }
    }
}

impl std::error::Error for FeedParseFailure {}

#[derive(Clone, Debug, Default)]
struct RssEntryFacts {
    guid: Option<String>,
    guid_is_permalink: bool,
}

/// Parse already-fetched bytes on a panic-contained blocking task.
pub async fn parse(bytes: &[u8], feed_url: &str) -> Result<FeedDiscoveryResult, FeedParseFailure> {
    let owned_bytes = bytes.to_vec();
    let feed_url = feed_url.to_string();
    tokio::task::spawn_blocking(move || {
        std::panic::catch_unwind(|| parse_sync(&owned_bytes, &feed_url)).map_err(|payload| {
            let message = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("unknown parser panic");
            FeedParseFailure::Panicked(format!("feed parser panicked: {message}"))
        })?
    })
    .await
    .map_err(|error| FeedParseFailure::Panicked(format!("feed parser task failed: {error}")))?
}

fn parse_sync(bytes: &[u8], feed_url: &str) -> Result<FeedDiscoveryResult, FeedParseFailure> {
    ensure_supported_xml_root(bytes)?;
    let parser = feed_rs::parser::Builder::new()
        .base_uri(Some(feed_url))
        .id_generator(|_, _, _| MISSING_ID.to_string())
        .build();
    let feed = parser
        .parse(bytes)
        .map_err(|error| FeedParseFailure::Parse(error.to_string()))?;

    let feed_type = match feed.feed_type {
        FeedType::Atom => "atom",
        FeedType::RSS0 => "rss0",
        FeedType::RSS1 => "rss1",
        FeedType::RSS2 => "rss2",
        FeedType::JSON => {
            return Err(FeedParseFailure::NotFeed(
                "the fetched representation is not an RSS or Atom feed".into(),
            ))
        }
    };
    let rss_facts = if matches!(
        feed.feed_type,
        FeedType::RSS0 | FeedType::RSS1 | FeedType::RSS2
    ) {
        rss_entry_facts(bytes)
    } else {
        Vec::new()
    };

    let entries = feed
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| map_entry(entry, &feed.feed_type, feed_url, rss_facts.get(index)))
        .collect();

    Ok(FeedDiscoveryResult {
        feed_type: feed_type.into(),
        feed_title: feed.title.map(|title| title.content),
        feed_authors: feed.authors.into_iter().map(|author| author.name).collect(),
        language: feed.language,
        entries,
    })
}

fn ensure_supported_xml_root(bytes: &[u8]) -> Result<(), FeedParseFailure> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_reader(bytes);
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                return match element.local_name().as_ref() {
                    b"rss" | b"RDF" | b"feed" => Ok(()),
                    _ => Err(FeedParseFailure::NotFeed(
                        "the fetched XML document is not an RSS or Atom feed".into(),
                    )),
                };
            }
            Ok(Event::Eof) => return Ok(()),
            Err(_) => return Ok(()),
            _ => {}
        }
    }
}

fn map_entry(
    entry: &Entry,
    feed_type: &FeedType,
    feed_url: &str,
    rss_facts: Option<&RssEntryFacts>,
) -> SourceItem {
    let is_atom = matches!(feed_type, FeedType::Atom);
    let url = if is_atom {
        entry
            .links
            .iter()
            .find(|link| link.rel.as_deref() == Some("alternate"))
            .map(|link| link.href.clone())
    } else {
        entry
            .links
            .first()
            .map(|link| link.href.clone())
            .or_else(|| {
                let facts = rss_facts?;
                (facts.guid_is_permalink)
                    .then_some(facts.guid.as_deref())
                    .flatten()
                    .filter(|guid| url::Url::parse(guid).is_ok())
                    .map(str::to_string)
            })
    };

    let source_item_id = (entry.id != MISSING_ID && !entry.id.is_empty()).then(|| entry.id.clone());
    SourceItem {
        source_type: "feed".into(),
        source_item_id,
        url,
        title: entry.title.as_ref().map(|title| title.content.clone()),
        snippet: entry
            .summary
            .as_ref()
            .map(|summary| summary.content.clone()),
        authors: entry
            .authors
            .iter()
            .map(|author| author.name.clone())
            .collect(),
        published_at: entry
            .published
            .and_then(|value| u64::try_from(value.timestamp_millis()).ok()),
        updated_at: is_atom
            .then(|| {
                entry
                    .updated
                    .and_then(|value| u64::try_from(value.timestamp_millis()).ok())
            })
            .flatten(),
        discovered_via: Some(feed_url.to_string()),
        media_references: media_references(entry, is_atom),
    }
}

fn media_references(entry: &Entry, is_atom: bool) -> Vec<MediaReference> {
    let mut references = Vec::new();
    if is_atom {
        references.extend(
            entry
                .links
                .iter()
                .filter(|link| link.rel.as_deref() == Some("enclosure"))
                .map(|link| MediaReference {
                    url: link.href.clone(),
                    mime_type: link.media_type.clone(),
                    caption: link.title.clone(),
                    width: None,
                    height: None,
                }),
        );
    }
    for media in &entry.media {
        append_media(&mut references, media);
    }
    references
}

fn append_media(references: &mut Vec<MediaReference>, media: &MediaObject) {
    let caption = media
        .title
        .as_ref()
        .or(media.description.as_ref())
        .map(|text| text.content.clone());
    references.extend(media.content.iter().filter_map(|content| {
        Some(MediaReference {
            url: content.url.as_ref()?.to_string(),
            mime_type: content
                .content_type
                .as_ref()
                .map(|mime| mime.as_str().to_string()),
            caption: caption.clone(),
            width: content.width,
            height: content.height,
        })
    }));
    references.extend(media.thumbnails.iter().map(|thumbnail| MediaReference {
        url: thumbnail.image.uri.clone(),
        mime_type: None,
        caption: caption.clone(),
        width: thumbnail.image.width,
        height: thumbnail.image.height,
    }));
}

fn rss_entry_facts(bytes: &[u8]) -> Vec<RssEntryFacts> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut facts = Vec::new();
    let mut current: Option<RssEntryFacts> = None;
    let mut in_guid = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) if element.local_name().as_ref() == b"item" => {
                current = Some(RssEntryFacts::default());
            }
            Ok(Event::Start(element))
                if current.is_some() && element.local_name().as_ref() == b"guid" =>
            {
                in_guid = true;
                if let Some(current) = current.as_mut() {
                    current.guid_is_permalink = element.attributes().flatten().any(|attribute| {
                        attribute.key.local_name().as_ref() == b"isPermaLink"
                            && attribute
                                .decoded_and_normalized_value(
                                    quick_xml::XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .map(|value| value.eq_ignore_ascii_case("true"))
                                .unwrap_or(false)
                    });
                }
            }
            Ok(Event::Text(text)) if in_guid => {
                if let (Some(current), Ok(value)) = (current.as_mut(), text.decode()) {
                    current.guid = Some(value.trim().to_string());
                }
            }
            Ok(Event::End(element)) if element.local_name().as_ref() == b"guid" => {
                in_guid = false;
            }
            Ok(Event::End(element)) if element.local_name().as_ref() == b"item" => {
                if let Some(current) = current.take() {
                    facts.push(current);
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    facts
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0"?>
<rss version="2.0" xmlns:dc="http://purl.org/dc/elements/1.1/">
<channel><title>Example RSS</title><language>en-GB</language>
<item><title>First</title><link>https://example.test/article</link><guid isPermaLink="false">native-guid</guid><author>author@example.test</author><dc:creator>DC Creator</dc:creator><pubDate>Tue, 03 Jun 2003 09:39:21 GMT</pubDate><description>Short summary</description><content:encoded xmlns:content="http://purl.org/rss/1.0/modules/content/">Full body</content:encoded><enclosure url="https://example.test/audio.mp3" type="audio/mpeg" length="123" /></item>
<item><title>No guid</title><link>https://example.test/no-guid</link><enclosure url="https://example.test/not-article.jpg" type="image/jpeg" /></item>
<item><title>Permalink guid</title><guid isPermaLink="true">https://example.test/from-guid</guid></item>
</channel></rss>"#;

    const ATOM: &str = r#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom"><title>Example Atom</title><id>feed-id</id><updated>2024-02-02T03:04:05Z</updated>
<author><name>Feed Author</name></author>
<entry><title>Atom first</title><id>native-atom-id</id><link rel="self" href="https://example.test/entry.atom"/><link rel="enclosure" href="https://example.test/video.mp4" type="video/mp4" title="Video"/><link rel="alternate" href="https://example.test/article"/><author><name>A One</name></author><author><name>A Two</name></author><published>2024-01-01T00:00:00Z</published><updated>2024-01-02T00:00:00Z</updated><summary>Atom summary</summary><content>Full atom body</content></entry>
<entry><title>Missing id</title><link rel="self" href="https://example.test/only-self"/></entry>
</feed>"#;

    #[tokio::test]
    async fn rss_identity_links_dates_summary_authors_and_media_are_normalized() {
        let result = parse(RSS.as_bytes(), "https://example.test/feed.xml")
            .await
            .unwrap();
        assert_eq!(result.feed_type, "rss2");
        assert_eq!(result.feed_title.as_deref(), Some("Example RSS"));
        assert_eq!(result.language.as_deref(), Some("en-gb"));
        assert_eq!(result.entries.len(), 3);
        let first = &result.entries[0];
        assert_eq!(first.source_item_id.as_deref(), Some("native-guid"));
        assert_eq!(first.url.as_deref(), Some("https://example.test/article"));
        assert_eq!(first.authors, ["author", "DC Creator"]);
        assert!(first.published_at.is_some());
        assert_eq!(first.updated_at, None);
        assert_eq!(first.snippet.as_deref(), Some("Short summary"));
        assert_eq!(
            first.media_references[0].url,
            "https://example.test/audio.mp3"
        );
        assert_eq!(
            first.media_references[0].mime_type.as_deref(),
            Some("audio/mpeg")
        );
        assert_eq!(
            first.discovered_via.as_deref(),
            Some("https://example.test/feed.xml"),
            "discovered_via must be the actual containing feed URL, not absent"
        );
        assert_eq!(result.entries[1].source_item_id, None);
        assert_eq!(
            result.entries[1].url.as_deref(),
            Some("https://example.test/no-guid")
        );
        assert_eq!(
            result.entries[2].url.as_deref(),
            Some("https://example.test/from-guid")
        );
        assert_eq!(
            result.entries[1].url.as_deref(),
            Some("https://example.test/no-guid")
        );
        assert_ne!(
            result.entries[1].url.as_deref(),
            Some("https://example.test/not-article.jpg")
        );
    }

    #[tokio::test]
    async fn atom_native_id_alternate_dates_authors_summary_and_enclosure_are_normalized() {
        let result = parse(ATOM.as_bytes(), "https://example.test/feed.atom")
            .await
            .unwrap();
        assert_eq!(result.feed_type, "atom");
        assert_eq!(result.feed_authors, ["Feed Author"]);
        let first = &result.entries[0];
        assert_eq!(first.source_item_id.as_deref(), Some("native-atom-id"));
        assert_eq!(first.url.as_deref(), Some("https://example.test/article"));
        assert_eq!(first.authors, ["A One", "A Two"]);
        assert_ne!(first.published_at, first.updated_at);
        assert!(first.published_at.is_some() && first.updated_at.is_some());
        assert_eq!(first.snippet.as_deref(), Some("Atom summary"));
        assert_eq!(
            first.media_references[0].url,
            "https://example.test/video.mp4"
        );
        assert_eq!(result.entries[1].source_item_id, None);
        assert_eq!(result.entries[1].url, None);
    }

    #[tokio::test]
    async fn valid_empty_feeds_succeed_and_invalid_documents_fail_explicitly() {
        for empty in [
            r#"<rss version="2.0"><channel><title>Empty</title></channel></rss>"#,
            r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>Empty</title><id>x</id><updated>2024-01-01T00:00:00Z</updated></feed>"#,
        ] {
            assert!(parse(empty.as_bytes(), "https://example.test/feed")
                .await
                .unwrap()
                .entries
                .is_empty());
        }
        for invalid in ["<rss><broken>", "<document><value>x</value></document>"] {
            let error = parse(invalid.as_bytes(), "https://example.test/feed")
                .await
                .unwrap_err();
            assert!(!error.to_string().is_empty());
        }
    }
}
