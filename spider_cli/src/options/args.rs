use crate::options::sub_command::Commands;
use clap::{ArgAction, Parser};

/// The fastest web crawler CLI. Crawl, scrape, or download any website.
///
/// Authenticate with [Spider Cloud](https://spider.cloud) for remote crawls:
///
/// ```sh
/// spider authenticate YOUR_API_KEY
/// ```
///
/// Sign up at <https://spider.cloud> to get an API key.
#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
#[command(arg_required_else_help = true)]
pub struct Cli {
    /// Build main sub commands
    #[clap(subcommand)]
    pub command: Option<Commands>,
    /// The website URL to crawl.
    #[clap(short, long, default_value = "")]
    pub url: String,
    /// Respect robots.txt file
    #[clap(short, long)]
    pub respect_robots_txt: bool,
    /// Allow sub-domain crawling.
    #[clap(short, long)]
    pub subdomains: bool,
    /// Allow all tlds for domain.
    #[clap(short, long)]
    pub tld: bool,
    #[clap(short = 'H', long)]
    /// Return the headers of the page.  Requires the `headers` flag enabled.
    pub return_headers: bool,
    /// Print page visited on standard output
    #[clap(short, long)]
    pub verbose: bool,
    /// Polite crawling delay in milli seconds
    #[clap(short = 'D', long)]
    pub delay: Option<u64>,
    /// The max pages allowed to crawl.
    #[clap(long)]
    pub limit: Option<u32>,
    /// Comma seperated string list of pages to not crawl or regex with feature enabled
    #[clap(long)]
    pub blacklist_url: Option<String>,
    /// User-Agent
    #[clap(short, long)]
    pub agent: Option<String>,
    /// Crawl Budget preventing extra paths from being crawled. Use commas to split the path followed by the limit ex: "*,1" - to only allow one page.
    #[clap(short = 'B', long)]
    pub budget: Option<String>,
    /// Set external domains to group with crawl.
    #[clap(short = 'E', long)]
    pub external_domains: Option<Vec<String>>,
    #[clap(short = 'b', long)]
    /// Block Images from rendering when using Chrome. Requires the `chrome_intercept` flag enabled.
    pub block_images: bool,
    /// The crawl depth limits.
    #[clap(short, long)]
    pub depth: Option<usize>,
    /// Dangerously accept invalid certficates
    #[clap(long)]
    pub accept_invalid_certs: bool,
    /// Gather all content that relates to the domain like css,jss, and etc.
    #[clap(long)]
    pub full_resources: bool,
    /// Use browser rendering mode (headless) for crawl/scrape/download. Requires the `chrome` feature.
    #[clap(long, conflicts_with = "http")]
    pub headless: bool,
    /// Force HTTP-only mode (no browser rendering), even when built with `chrome`.
    #[clap(long, action = ArgAction::SetTrue, conflicts_with = "headless")]
    pub http: bool,
    /// The proxy url to use.
    #[clap(short, long)]
    pub proxy_url: Option<String>,
    /// Spider Cloud API key. Sign up at https://spider.cloud for an API key.
    /// Falls back to the SPIDER_CLOUD_API_KEY env var, then stored credentials from `spider authenticate`.
    #[clap(long)]
    pub spider_cloud_key: Option<String>,
    /// Partition the cache by an opaque namespace so logically distinct variants of
    /// the same URL (country, proxy pool, tenant, A/B bucket, device profile, …)
    /// never collide on the same cached bytes. Falls back to the
    /// SPIDER_CACHE_NAMESPACE env var.
    #[clap(long)]
    pub cache_namespace: Option<String>,
    /// Spider Cloud mode: api (default), proxy, unblocker, fallback, or smart.
    #[clap(long, default_value = "api")]
    pub spider_cloud_mode: Option<String>,
    /// Use Spider Browser Cloud (remote headless CDP) instead of the HTTP/proxy cloud.
    #[clap(long)]
    pub spider_cloud_browser: bool,
    /// Wait for network request to be idle within a time frame period (500ms no network connections) with an optional timeout in milliseconds.
    #[clap(long)]
    pub wait_for_idle_network: Option<u64>,
    /// Wait for network request with a max timeout (0 connections) with an optional timeout in milliseconds.
    #[clap(long)]
    pub wait_for_idle_network0: Option<u64>,
    /// Wait for network to be almost idle with a max timeout (max 2 connections) with an optional timeout in milliseconds.
    #[clap(long)]
    pub wait_for_almost_idle_network0: Option<u64>,
    /// Wait for idle dom mutations for target element (defaults to "body") with a 30s timeout.
    #[clap(long)]
    pub wait_for_idle_dom: Option<String>,
    /// Wait for a specific CSS selector to appear with a 60s timeout.
    #[clap(long)]
    pub wait_for_selector: Option<String>,
    /// Wait for a fixed delay in milliseconds.
    #[clap(long)]
    pub wait_for_delay: Option<u64>,
    /// Connect to an existing Chrome DevTools Protocol endpoint (e.g. ws://127.0.0.1:9222 or http://127.0.0.1:9222/json/version).
    #[clap(long)]
    pub chrome_connection_url: Option<String>,
    /// Cookie string to inject (e.g. "key=value; key2=value2"). Requires the `cookies` feature on spider.
    #[clap(long)]
    pub cookie: Option<String>,
    /// Enable stealth mode to avoid bot detection.
    #[clap(long)]
    pub stealth: bool,
    /// Write crawled pages to a WARC 1.1 archive file at the given path.
    #[clap(short = 'W', long)]
    pub warc: Option<String>,
    /// Transform output format: markdown (default), raw, commonmark, text, xml.
    /// Requires the `transformations` feature (enabled by default).
    #[clap(long, default_value = "markdown")]
    pub return_format: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// Every derived subcommand's argument definitions are internally
    /// consistent (clap's own structural invariant check).
    #[test]
    fn command_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[cfg(feature = "fetch")]
    #[test]
    fn parses_fetch() {
        let cli = Cli::try_parse_from(["spider", "fetch", "https://example.test"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::FETCH { url }) if url == "https://example.test"
        ));
    }

    #[cfg(feature = "feed")]
    #[test]
    fn parses_feed_with_limit() {
        let cli = Cli::try_parse_from([
            "spider",
            "feed",
            "https://example.test/feed.xml",
            "--limit",
            "5",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::FEED { url, limit: Some(5) }) if url == "https://example.test/feed.xml"
        ));
    }

    #[cfg(feature = "feed")]
    #[test]
    fn parses_feed_without_limit() {
        let cli = Cli::try_parse_from(["spider", "feed", "https://example.test/feed.xml"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::FEED { limit: None, .. })
        ));
    }

    #[cfg(feature = "sitemap")]
    #[test]
    fn parses_sitemap() {
        let cli =
            Cli::try_parse_from(["spider", "sitemap", "https://example.test/sitemap.xml"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::SITEMAP { .. })));
    }

    #[cfg(feature = "news_sitemap")]
    #[test]
    fn parses_news_sitemap_hyphenated_name() {
        let cli = Cli::try_parse_from(["spider", "news-sitemap", "https://example.test/news.xml"])
            .unwrap();
        assert!(matches!(cli.command, Some(Commands::NEWS_SITEMAP { .. })));
    }

    #[cfg(feature = "robots_sitemap")]
    #[test]
    fn parses_robots_sitemap_hyphenated_name() {
        let cli = Cli::try_parse_from([
            "spider",
            "robots-sitemap",
            "https://example.test/robots.txt",
        ])
        .unwrap();
        assert!(matches!(cli.command, Some(Commands::ROBOTS_SITEMAP { .. })));
    }

    #[cfg(feature = "search_searxng")]
    #[test]
    fn parses_search_options_and_default_web_category() {
        use crate::options::sub_command::SearchCategory;

        let cli = Cli::try_parse_from([
            "spider",
            "search",
            "open source intelligence",
            "--provider",
            "searxng",
            "--base-url",
            "http://localhost:8080",
            "--limit",
            "7",
            "--language",
            "sv",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::SEARCH {
                query,
                provider,
                base_url: Some(base_url),
                category: SearchCategory::Web,
                limit: Some(7),
                language: Some(language),
            }) if query == "open source intelligence"
                && provider == "searxng"
                && base_url == "http://localhost:8080"
                && language == "sv"
        ));
    }

    #[cfg(feature = "search_searxng")]
    #[test]
    fn parses_all_search_categories_and_english_language() {
        use crate::options::sub_command::SearchCategory;

        for (name, expected) in [
            ("web", SearchCategory::Web),
            ("news", SearchCategory::News),
            ("image", SearchCategory::Image),
            ("video", SearchCategory::Video),
        ] {
            let cli = Cli::try_parse_from([
                "spider",
                "search",
                "query",
                "--provider",
                "searxng",
                "--base-url",
                "http://localhost:8080",
                "--category",
                name,
                "--language",
                "en",
            ])
            .unwrap();
            assert!(matches!(
                cli.command,
                Some(Commands::SEARCH {
                    category,
                    language: Some(language),
                    ..
                }) if category == expected && language == "en"
            ));
        }
    }

    /// Existing commands remain parseable, unregressed by the new variants.
    /// Argv[0] is deliberately still `"spider"` here — clap only uses it
    /// for shell-completion/error-message purposes, never for validating
    /// which binary is running, so this proves parsing itself is
    /// unaffected by the binary rename in `Cargo.toml`.
    #[test]
    fn existing_commands_remain_parseable() {
        assert!(matches!(
            Cli::try_parse_from(["spider", "--url", "https://example.test", "crawl"])
                .unwrap()
                .command,
            Some(Commands::CRAWL { .. })
        ));
        assert!(matches!(
            Cli::try_parse_from(["spider", "--url", "https://example.test", "scrape"])
                .unwrap()
                .command,
            Some(Commands::SCRAPE { .. })
        ));
        assert!(matches!(
            Cli::try_parse_from(["spider", "--url", "https://example.test", "download"])
                .unwrap()
                .command,
            Some(Commands::DOWNLOAD { .. })
        ));
        assert!(matches!(
            Cli::try_parse_from(["spider", "authenticate", "sk-test"])
                .unwrap()
                .command,
            Some(Commands::AUTHENTICATE { .. })
        ));
    }

    /// Same proof again, but through the literal renamed binary name —
    /// `scorpion crawl ...` etc. all parse identically.
    #[test]
    fn existing_commands_remain_parseable_under_scorpion_argv0() {
        assert!(matches!(
            Cli::try_parse_from(["scorpion", "--url", "https://example.test", "crawl"])
                .unwrap()
                .command,
            Some(Commands::CRAWL { .. })
        ));
        assert!(matches!(
            Cli::try_parse_from(["scorpion", "--url", "https://example.test", "scrape"])
                .unwrap()
                .command,
            Some(Commands::SCRAPE { .. })
        ));
        assert!(matches!(
            Cli::try_parse_from(["scorpion", "--url", "https://example.test", "download"])
                .unwrap()
                .command,
            Some(Commands::DOWNLOAD { .. })
        ));
        assert!(matches!(
            Cli::try_parse_from(["scorpion", "authenticate", "sk-test"])
                .unwrap()
                .command,
            Some(Commands::AUTHENTICATE { .. })
        ));
    }

    #[cfg(feature = "mcp")]
    #[test]
    fn parses_mcp_with_default_log_level() {
        let cli = Cli::try_parse_from(["scorpion", "mcp"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::MCP { ref log_level }) if log_level == "warn"
        ));
    }

    #[cfg(feature = "mcp")]
    #[test]
    fn parses_mcp_with_custom_log_level() {
        let cli = Cli::try_parse_from(["scorpion", "mcp", "--log-level", "debug"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::MCP { ref log_level }) if log_level == "debug"
        ));
    }

    /// `mcp` does not expose any of the irrelevant crawl/discovery flags —
    /// its arg surface is exactly `--log-level`/`-h`.
    #[cfg(feature = "mcp")]
    #[test]
    fn mcp_subcommand_exposes_only_log_level() {
        let command = Cli::command();
        let mcp = command
            .get_subcommands()
            .find(|sub| sub.get_name() == "mcp")
            .expect("mcp subcommand must be registered");
        let arg_names: Vec<&str> = mcp
            .get_arguments()
            .map(|arg| arg.get_id().as_str())
            .filter(|name| *name != "help")
            .collect();
        assert_eq!(arg_names, ["log_level"]);
    }

    /// The full canonical command tree — every command this frontier and
    /// its predecessors established must remain reachable by name.
    #[test]
    fn command_tree_lists_all_canonical_commands() {
        let command = Cli::command();
        let names: Vec<&str> = command
            .get_subcommands()
            .map(|sub| sub.get_name())
            .collect();

        for expected in ["crawl", "scrape", "download", "authenticate"] {
            assert!(names.contains(&expected), "missing {expected}: {names:?}");
        }
        #[cfg(feature = "fetch")]
        assert!(names.contains(&"fetch"), "{names:?}");
        #[cfg(feature = "feed")]
        assert!(names.contains(&"feed"), "{names:?}");
        #[cfg(feature = "sitemap")]
        assert!(names.contains(&"sitemap"), "{names:?}");
        #[cfg(feature = "news_sitemap")]
        assert!(names.contains(&"news-sitemap"), "{names:?}");
        #[cfg(feature = "robots_sitemap")]
        assert!(names.contains(&"robots-sitemap"), "{names:?}");
        #[cfg(feature = "search_searxng")]
        assert!(names.contains(&"search"), "{names:?}");
        #[cfg(feature = "mcp")]
        assert!(names.contains(&"mcp"), "{names:?}");
    }
}
