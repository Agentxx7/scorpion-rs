//! Research-readable content materialization.

use spider_agent_types::ContentAnalysis;

struct ReadableProduct {
    content: String,
    text: String,
}

struct FallbackCandidate {
    markdown: String,
    section_headings: Vec<String>,
    substantive_heading_count: usize,
    prose_chars: usize,
    dom_index: usize,
}

/// Deterministic failures while deriving research-readable content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResearchReadableError {
    /// The source URL was not valid.
    InvalidUrl,
    /// Readability could not identify a main-content representation.
    ReadabilityFailed,
    /// The derived Markdown did not contain enough substantive text.
    InsufficientContent,
}

impl std::fmt::Display for ResearchReadableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUrl => f.write_str("source URL is invalid"),
            Self::ReadabilityFailed => f.write_str("readability extraction failed"),
            Self::InsufficientContent => {
                f.write_str("derived research content is empty or too thin")
            }
        }
    }
}

impl std::error::Error for ResearchReadableError {}

/// Select main content, convert it to Markdown, and reject thin output.
///
/// The source HTML is borrowed and never modified. Readability failure is
/// fail-closed: raw HTML is not returned as an equivalent fallback.
pub fn materialize_research_markdown(
    html: &str,
    source_url: &str,
) -> Result<String, ResearchReadableError> {
    materialize_with_selector(html, source_url, |input, url| {
        llm_readability::extractor::extract(&mut input.as_bytes(), url)
            .map(|product| ReadableProduct {
                content: product.content,
                text: product.text,
            })
            .map_err(|_| ResearchReadableError::ReadabilityFailed)
    })
}

fn materialize_with_selector(
    html: &str,
    source_url: &str,
    select: impl FnOnce(&str, &url::Url) -> Result<ReadableProduct, ResearchReadableError>,
) -> Result<String, ResearchReadableError> {
    let url = url::Url::parse(source_url).map_err(|_| ResearchReadableError::InvalidUrl)?;
    let readable = select(html, &url)?;
    let markdown =
        html2md::rewrite_html_custom_with_url(&readable.content, &None, false, &Some(url.clone()));

    let fallback = select_fallback_candidate(html, &url);
    if is_substantive(&readable.text) && is_substantive(&markdown) {
        let omitted_article_sections = fallback.as_ref().is_some_and(|candidate| {
            candidate.section_headings.len() >= 2
                && candidate.section_headings.iter().any(|heading| {
                    !readable
                        .text
                        .to_ascii_lowercase()
                        .contains(&heading.to_ascii_lowercase())
                })
        });
        if !omitted_article_sections {
            return Ok(markdown);
        }
    }

    fallback
        .map(|candidate| candidate.markdown)
        .ok_or(ResearchReadableError::InsufficientContent)
}

fn is_substantive(content: &str) -> bool {
    !ContentAnalysis::analyze(content).is_thin_content
}

fn select_fallback_candidate(html: &str, url: &url::Url) -> Option<FallbackCandidate> {
    use scraper::{Html, Selector};

    let document = Html::parse_document(html);
    let structured =
        Selector::parse("article, main, section, div").expect("static selector must parse");
    best_candidate(document.select(&structured), url)
}

fn best_candidate<'a>(
    candidates: impl Iterator<Item = scraper::ElementRef<'a>>,
    url: &url::Url,
) -> Option<FallbackCandidate> {
    use scraper::Selector;

    let heading_selector =
        Selector::parse("h1, h2, h3").expect("static heading selector must parse");
    let prose_selector =
        Selector::parse("p, pre, blockquote").expect("static prose selector must parse");
    candidates
        .enumerate()
        .filter_map(|(dom_index, candidate)| {
            let tag = candidate.value().name();
            let visible_text = candidate.text().collect::<Vec<_>>().join(" ");
            if !is_substantive(&visible_text) {
                return None;
            }

            let markdown = html2md::rewrite_html_custom_with_url(
                &candidate.html(),
                &None,
                false,
                &Some(url.clone()),
            );
            if !is_substantive(&markdown) {
                return None;
            }

            let section_headings = candidate
                .select(&heading_selector)
                .map(|heading| heading.text().collect::<Vec<_>>().join(" "))
                .filter(|heading| !heading.trim().is_empty())
                .collect::<Vec<_>>();
            let prose = candidate
                .select(&prose_selector)
                .flat_map(|element| element.text())
                .collect::<Vec<_>>()
                .join(" ");
            let prose_chars = prose.chars().count();
            let generic_container = tag == "section" || tag == "div";
            if generic_container && (section_headings.len() < 2 || !is_substantive(&prose)) {
                return None;
            }

            let substantive_heading_count = candidate
                .select(&heading_selector)
                .filter(|heading| heading_has_following_prose(*heading))
                .count();
            Some(FallbackCandidate {
                markdown,
                section_headings,
                substantive_heading_count,
                prose_chars,
                dom_index,
            })
        })
        .max_by(compare_candidates)
}

fn heading_has_following_prose(heading: scraper::ElementRef<'_>) -> bool {
    use scraper::ElementRef;

    let mut sibling = heading.next_sibling();
    while let Some(node) = sibling {
        sibling = node.next_sibling();
        let Some(element) = ElementRef::wrap(node) else {
            continue;
        };
        let tag = element.value().name();
        if matches!(tag, "h1" | "h2" | "h3") {
            return false;
        }
        if matches!(tag, "p" | "pre" | "blockquote") {
            let prose = element.text().collect::<Vec<_>>().join(" ");
            return prose
                .chars()
                .filter(|character| !character.is_whitespace())
                .count()
                >= 40;
        }
    }
    false
}

fn compare_candidates(left: &FallbackCandidate, right: &FallbackCandidate) -> std::cmp::Ordering {
    left.substantive_heading_count
        .cmp(&right.substantive_heading_count)
        // A smaller Markdown subtree preserving equivalent coverage wins.
        .then_with(|| right.markdown.len().cmp(&left.markdown.len()))
        .then_with(|| left.prose_chars.cmp(&right.prose_chars))
        // Earlier DOM order wins the final tie. `max_by` otherwise keeps the
        // later equal element, so reverse the index comparison explicitly.
        .then_with(|| right.dom_index.cmp(&left.dom_index))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn substantive_article() -> &'static str {
        "<article><h1>Async runtimes</h1><p>Tokio and async-std provide executors, networking, timers, synchronization primitives, and task scheduling for asynchronous Rust applications. Their APIs differ in ecosystem breadth and compatibility.</p><ul><li>Tokio has broad library support.</li><li>async-std follows familiar standard-library naming.</li></ul><pre><code>runtime.block_on(task);</code></pre></article>"
    }

    fn injected_product(content: &str, text: &str) -> ReadableProduct {
        ReadableProduct {
            content: content.to_string(),
            text: text.to_string(),
        }
    }

    #[test]
    fn selects_late_article_and_preserves_markdown_structure() {
        let boilerplate = format!(
            "<header><nav>{}</nav></header>",
            "<a href='/menu'>menu</a>".repeat(600)
        );
        assert!(boilerplate.len() > 10_000);
        let html = format!(
            "<html><body>{boilerplate}{}</body></html>",
            substantive_article()
        );

        let markdown = materialize_research_markdown(&html, "https://example.test/article")
            .expect("late substantive article should be readable");

        assert!(markdown.contains("Async runtimes"));
        assert!(markdown.contains("Tokio and async-std"));
        assert!(markdown.contains("Tokio has broad library support"));
        assert!(markdown.contains("runtime.block\\_on(task);"));
        assert!(!markdown.contains("menu\nmenu\nmenu"));
        assert!(markdown.len() < 10_000);
    }

    #[test]
    fn rejects_empty_whitespace_and_thin_shells() {
        for html in [
            "",
            "   \n\t",
            "<html><body></body></html>",
            "<html><body><header>Sign in</header><main>Enable JavaScript</main></body></html>",
        ] {
            assert!(materialize_research_markdown(html, "https://example.test").is_err());
        }
    }

    #[test]
    fn admits_short_but_substantive_article() {
        let markdown =
            materialize_research_markdown(substantive_article(), "https://example.test/article")
                .expect("substantive article should be admitted");

        assert!(markdown.contains("Async runtimes"));
    }

    #[test]
    fn readability_failure_does_not_return_the_raw_shell() {
        let raw = "not html ".repeat(2_000);
        assert_eq!(
            materialize_with_selector(&raw, "https://example.test", |_, _| {
                Err(ResearchReadableError::ReadabilityFailed)
            }),
            Err(ResearchReadableError::ReadabilityFailed)
        );
    }

    #[test]
    fn substantive_primary_readability_remains_authoritative() {
        let primary = substantive_article();
        let html = format!(
            "<article><h1>Fallback must not win</h1><p>{}</p></article>",
            "Fallback material is substantive but should remain unused while the primary readability selection is itself substantive. ".repeat(3)
        );

        let markdown = materialize_with_selector(&html, "https://example.test/article", |_, _| {
            Ok(injected_product(primary, primary))
        })
        .unwrap();

        assert!(markdown.contains("Async runtimes"));
        assert!(!markdown.contains("Fallback must not win"));
    }

    #[test]
    fn structured_fallback_recovers_body_outside_article_and_main() {
        let boilerplate = "<nav><a href='/menu'>menu</a></nav>".repeat(400);
        assert!(boilerplate.len() > 10_000);
        let html = format!(
            "<html><body>{boilerplate}<article><h1>Metadata article</h1><p>{}</p></article><main><h1>Related runtime links</h1><p>{}</p></main><div class='huge-layout'><h2>Navigation heading without prose</h2><div class='content-fragment'><h2>Which Crates Require Which Runtime</h2><p>{}</p><h2>When Should You Choose Each Runtime</h2><p>{}</p></div></div></body></html>",
            "The metadata article contains publication details, author information, image captions, and a sufficiently long summary to pass basic content admission. ".repeat(2),
            "The main container lists related runtime resources, documentation links, author profiles, publication metadata, and other supplementary material. ".repeat(2),
            "Runtime compatibility depends on the libraries and network stack selected by an application. ".repeat(3),
            "Choose a runtime by evaluating ecosystem requirements, scheduling needs, and interoperability constraints. ".repeat(3)
        );

        let markdown =
            materialize_with_selector(&html, "https://example.test/rustify-shaped", |_, _| {
                Ok(injected_product(
                    "<h1>Runtime comparison</h1><p>This overview describes a detailed comparison of Tokio and async-std, including their APIs, ecosystem position, scheduling facilities, networking support, timers, synchronization primitives, and general runtime selection considerations for Rust applications.</p>",
                    "Runtime comparison. This overview describes a detailed comparison of Tokio and async-std, including their APIs, ecosystem position, scheduling facilities, networking support, timers, synchronization primitives, and general runtime selection considerations for Rust applications.",
                ))
            })
            .unwrap();

        assert!(is_substantive(
            "Runtime comparison. This overview describes a detailed comparison of Tokio and async-std, including their APIs, ecosystem position, scheduling facilities, networking support, timers, synchronization primitives, and general runtime selection considerations for Rust applications."
        ));
        assert!(markdown.contains("Which Crates Require Which Runtime"));
        assert!(markdown.contains("When Should You Choose Each Runtime"));
        assert!(!markdown.contains("Metadata article"));
        assert!(!markdown.contains("Related runtime links"));
        assert!(!markdown.contains("Navigation heading without prose"));
        assert!(!markdown.contains("menu\nmenu\nmenu"));
    }

    #[test]
    fn equivalent_nested_candidates_choose_smallest_substantive_container() {
        let html = format!(
            "<div class='outer'><p>{}</p><div class='inner'><h2>First owned section</h2><p>{}</p><h2>Second owned section</h2><p>{}</p></div><p>{}</p></div>",
            "Outer boilerplate introduction that should not survive selection. ".repeat(4),
            "The first section contains substantive source prose about runtime scheduling and compatibility. ".repeat(3),
            "The second section contains substantive source prose about networking and synchronization. ".repeat(3),
            "Outer boilerplate footer that should not survive selection. ".repeat(4)
        );

        let markdown = materialize_with_selector(&html, "https://example.test/nested", |_, _| {
            Ok(injected_product("<p>Thin summary</p>", "Thin summary"))
        })
        .unwrap();

        assert!(markdown.contains("First owned section"));
        assert!(!markdown.contains("Outer boilerplate introduction"));
        assert!(!markdown.contains("Outer boilerplate footer"));
    }

    #[test]
    fn dasroot_shaped_primary_with_many_sections_remains_authoritative() {
        let primary = format!(
            "<article><h1>Async programming patterns</h1>{}</article>",
            (1..=8)
                .map(|index| format!("<h2>Section {index}</h2><p>Substantive source discussion of runtime executors, scheduling, networking, synchronization, timers, cancellation, and ecosystem interoperability for section {index}. Additional factual detail keeps every section useful.</p>"))
                .collect::<String>()
        );
        let html = format!(
            "<main>{primary}</main><div><h2>Unrelated section one</h2><p>{}</p><h2>Unrelated section two</h2><p>{}</p></div>",
            "Unrelated fallback material. ".repeat(10),
            "More unrelated fallback material. ".repeat(10)
        );

        let markdown = materialize_with_selector(&html, "https://example.test/dasroot", |_, _| {
            Ok(injected_product(&primary, &primary))
        })
        .unwrap();

        assert!(markdown.contains("Async programming patterns"));
        assert!(!markdown.contains("Unrelated section one"));
    }

    #[test]
    fn largest_substantive_article_wins_over_cards_and_ctas() {
        let html = format!(
            "<html><body><article><p>Short card</p></article><article><h1>Full article</h1><p>{}</p></article><article><p>Subscribe now</p></article></body></html>",
            "The full source article contains detailed discussion of runtime selection, compatibility, scheduling, networking, timers, synchronization, and ecosystem support. ".repeat(3)
        );

        let markdown = materialize_with_selector(&html, "https://example.test/multiple", |_, _| {
            Ok(injected_product("<p>Thin summary</p>", "Thin summary"))
        })
        .unwrap();

        assert!(markdown.contains("Full article"));
        assert!(!markdown.contains("Short card"));
        assert!(!markdown.contains("Subscribe now"));
    }

    #[test]
    fn substantive_main_is_used_when_articles_are_not_admissible() {
        let html = format!(
            "<html><body><article>Card</article><main><h1>Main report</h1><p>{}</p></main></body></html>",
            "The main report provides substantive discussion of executor design, task scheduling, networking, synchronization primitives, timers, cancellation, and interoperability. ".repeat(3)
        );

        let markdown = materialize_with_selector(&html, "https://example.test/main", |_, _| {
            Ok(injected_product("<p>Thin summary</p>", "Thin summary"))
        })
        .unwrap();

        assert!(markdown.contains("Main report"));
    }

    #[test]
    fn large_raw_shell_without_substantive_candidates_is_rejected() {
        let html = format!(
            "<html><body><div>{}</div><main>Enable JavaScript</main></body></html>",
            "<a href='/menu'>menu</a>".repeat(10_000)
        );

        assert_eq!(
            materialize_with_selector(&html, "https://example.test/shell", |_, _| {
                Ok(injected_product("<h1>Shell</h1>", "Shell"))
            }),
            Err(ResearchReadableError::InsufficientContent)
        );
    }

    #[test]
    fn fallback_markdown_is_complete_before_agent_side_truncation() {
        let long_body = format!(
            "{}<p>TAIL MARKER AFTER TEN THOUSAND BYTES</p>",
            "<p>Substantive article paragraph about runtime compatibility, scheduling, networking, and synchronization.</p>".repeat(140)
        );
        let html = format!("<article><h1>Long fallback article</h1>{long_body}</article>");

        let markdown = materialize_with_selector(&html, "https://example.test/long", |_, _| {
            Ok(injected_product("<p>Thin summary</p>", "Thin summary"))
        })
        .unwrap();

        assert!(markdown.len() > 10_000);
        assert!(markdown.contains("TAIL MARKER AFTER TEN THOUSAND BYTES"));
    }
}
