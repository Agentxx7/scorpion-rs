//! Parse robots.txt text for declared `Sitemap:` directive values.
//!
//! This module owns no network behavior. Callers provide the exact bytes
//! already retrieved. A `Sitemap:` declaration only says "this sitemap URL
//! was declared here" — this module never fetches any declared URL, never
//! classifies which kind of sitemap a declared URL points to (ordinary,
//! index, News, Image, ...), and is deliberately independent of
//! `spider::packages::robotparser::RobotFileParser`'s Allow/Disallow
//! crawl-policy state machine, which does not capture Sitemap values at
//! all (it recognizes the directive name only, to keep its own line state
//! correctly transitioned, and discards the value).

use std::fmt;

/// One `Sitemap:` URL declared in a robots.txt document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct RobotsSitemapReference {
    /// Exact, absolute URL as declared by the `Sitemap:` directive.
    pub url: String,
}

/// Result of scanning a robots.txt document for `Sitemap:` declarations.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct RobotsSitemapDiscoveryResult {
    /// Declared sitemap URLs, in source order, with duplicates preserved.
    pub sitemaps: Vec<RobotsSitemapReference>,
}

/// Contained failure at the blocking parser task boundary.
///
/// Scanning robots.txt for `Sitemap:` lines has no document-level
/// well-formedness requirement the way XML does — a malformed or relative
/// individual value is simply skipped (see `parse_sync`), never treated as
/// a whole-document failure. This variant exists only for parser-task
/// infrastructure problems (a contained panic, or a `spawn_blocking` join
/// failure), matching the containment shape already used by
/// `feed`/`sitemap`/`news_sitemap`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RobotsSitemapParseFailure {
    /// The blocking parser task panicked or could not be joined.
    Panicked(String),
}

impl fmt::Display for RobotsSitemapParseFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Panicked(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for RobotsSitemapParseFailure {}

/// Scan already-fetched robots.txt bytes on a panic-contained blocking task.
pub async fn parse(
    bytes: &[u8],
) -> Result<RobotsSitemapDiscoveryResult, RobotsSitemapParseFailure> {
    let bytes = bytes.to_vec();
    tokio::task::spawn_blocking(move || {
        std::panic::catch_unwind(|| parse_sync(&bytes)).map_err(|payload| {
            let message = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("unknown parser panic");
            RobotsSitemapParseFailure::Panicked(format!(
                "robots sitemap parser panicked: {message}"
            ))
        })
    })
    .await
    .map_err(|error| {
        RobotsSitemapParseFailure::Panicked(format!("robots sitemap parser task failed: {error}"))
    })?
}

fn parse_sync(bytes: &[u8]) -> RobotsSitemapDiscoveryResult {
    // robots.txt is text/plain in normal cases but servers may mislabel it;
    // this relies only on the actual retained bytes, never a declared MIME
    // type. Lossy decoding never fails, matching the permissive philosophy
    // already established for this format (unlike the XML adapters).
    let text = String::from_utf8_lossy(bytes);
    let mut sitemaps = Vec::new();

    for raw_line in text.split('\n') {
        // Strip a trailing \r (CRLF tolerance), then an inline `#` comment
        // — the same unconditional-on-the-line convention already used by
        // `RobotFileParser::parse` — then trim surrounding whitespace.
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let line = match line.find('#') {
            Some(hash) => &line[..hash],
            None => line,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // `Sitemap:` directives apply independently of any `User-agent:`
        // section (unlike Allow/Disallow/Crawl-delay/Request-rate) — every
        // line is scanned unconditionally, with no state/context tracking.
        let Some(colon) = line.find(':') else {
            continue;
        };
        let (name, value) = (line[..colon].trim(), line[colon + 1..].trim());

        if !name.eq_ignore_ascii_case("sitemap") || value.is_empty() {
            continue;
        }

        // The Sitemap-directive convention expects an absolute URL. A
        // relative or otherwise unparseable value is skipped outright —
        // never resolved against a base, never fabricated into an
        // absolute form. Cross-origin absolute URLs are accepted as-is:
        // this module only discovers candidates, it never fetches them,
        // so there is no SSRF surface to guard against here.
        //
        // The value is preserved exactly as declared and is intentionally
        // NOT percent-decoded — unlike `RobotFileParser`'s Allow/Disallow
        // path values (which are decoded for path-matching purposes),
        // percent-encoding in a URL is structural and decoding it would
        // risk misrepresenting the URL the publisher actually declared.
        if url::Url::parse(value).is_ok() {
            sitemaps.push(RobotsSitemapReference {
                url: value.to_string(),
            });
        }
    }

    RobotsSitemapDiscoveryResult { sitemaps }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn sitemaps(text: &str) -> Vec<String> {
        parse(text.as_bytes())
            .await
            .unwrap()
            .sitemaps
            .into_iter()
            .map(|entry| entry.url)
            .collect()
    }

    #[tokio::test]
    async fn single_directive_is_captured() {
        let text =
            "User-agent: *\nDisallow: /private\n\nSitemap: https://example.test/sitemap.xml\n";
        assert_eq!(sitemaps(text).await, ["https://example.test/sitemap.xml"]);
    }

    #[tokio::test]
    async fn multiple_directives_preserve_order_and_duplicates() {
        let text = "Sitemap: https://example.test/sitemap.xml\n\
Sitemap: https://example.test/news.xml\n\
Sitemap: https://example.test/sitemap.xml\n";
        assert_eq!(
            sitemaps(text).await,
            [
                "https://example.test/sitemap.xml",
                "https://example.test/news.xml",
                "https://example.test/sitemap.xml",
            ]
        );
    }

    #[tokio::test]
    async fn empty_text_yields_no_sitemaps() {
        assert!(sitemaps("").await.is_empty());
    }

    #[tokio::test]
    async fn empty_malformed_and_relative_values_are_skipped() {
        let text = "Sitemap:\n\
Sitemap: not a url\n\
Sitemap: /relative/sitemap.xml\n\
Sitemap: https://example.test/real.xml\n";
        assert_eq!(sitemaps(text).await, ["https://example.test/real.xml"]);
    }

    #[tokio::test]
    async fn case_variants_are_recognized() {
        for name in ["Sitemap", "SITEMAP", "sitemap", "SiteMap"] {
            let text = format!("{name}: https://example.test/sitemap.xml");
            assert_eq!(
                sitemaps(&text).await,
                ["https://example.test/sitemap.xml"],
                "{name}"
            );
        }
    }

    #[tokio::test]
    async fn whitespace_around_colon_and_value_is_trimmed() {
        let text = "Sitemap  :   https://example.test/sitemap.xml   \n";
        assert_eq!(sitemaps(text).await, ["https://example.test/sitemap.xml"]);
    }

    #[tokio::test]
    async fn inline_and_leading_comments_and_blank_lines_are_ignored() {
        let text = "# leading comment\n\
\n\
Sitemap: https://example.test/sitemap.xml # primary sitemap\n\
\n\
# another comment\n\
Sitemap: https://example.test/news.xml# no space before comment\n";
        assert_eq!(
            sitemaps(text).await,
            [
                "https://example.test/sitemap.xml",
                "https://example.test/news.xml",
            ]
        );
    }

    #[tokio::test]
    async fn crlf_line_endings_are_handled() {
        let text = "Sitemap: https://example.test/sitemap.xml\r\nSitemap: https://example.test/news.xml\r\n";
        assert_eq!(
            sitemaps(text).await,
            [
                "https://example.test/sitemap.xml",
                "https://example.test/news.xml",
            ]
        );
    }

    #[tokio::test]
    async fn sitemap_recognized_without_any_user_agent_section() {
        // No `User-agent:` line anywhere in the document — Sitemap
        // directives must not be gated behind having seen one.
        let text = "Sitemap: https://example.test/sitemap.xml\n";
        assert_eq!(sitemaps(text).await, ["https://example.test/sitemap.xml"]);
    }

    #[tokio::test]
    async fn cross_origin_absolute_sitemap_is_accepted() {
        let text = "User-agent: *\n\
Disallow: /private\n\
Sitemap: https://cdn.other-example.test/sitemap.xml\n";
        assert_eq!(
            sitemaps(text).await,
            ["https://cdn.other-example.test/sitemap.xml"]
        );
    }

    #[test]
    fn absent_sitemaps_stay_empty() {
        assert!(RobotsSitemapDiscoveryResult::default().sitemaps.is_empty());
    }
}
