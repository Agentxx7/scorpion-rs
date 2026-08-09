use clap::{Subcommand, ValueEnum};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum SearchCategory {
    Web,
    News,
    Image,
    Video,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Crawl the website extracting links.
    CRAWL {
        /// sequentially one by one crawl pages
        #[clap(short, long)]
        sync: bool,
        /// stdout all links crawled
        #[clap(short, long)]
        output_links: bool,
    },
    /// Scrape a page, returning its content as markdown (jsonl). Use --return-format
    /// for another format, or --output-html for the raw HTML.
    SCRAPE {
        /// Include the page links in the output.
        #[clap(short, long)]
        output_links: bool,
        /// Return the raw HTML instead of the transformed (markdown) content.
        #[clap(long)]
        output_html: bool,
    },
    /// Download html markup to destination.
    DOWNLOAD {
        /// store files at target destination
        #[clap(short, long)]
        target_destination: Option<String>,
    },
    /// Authenticate with the Spider Cloud service. Stores your API key locally for remote crawls.
    /// With no arguments it signs you in through your browser (OAuth) and provisions a key.
    /// Sign up at https://spider.cloud to get started.
    #[clap(alias = "auth", alias = "login")]
    AUTHENTICATE {
        /// Your Spider Cloud API key (e.g. sk-...). If omitted, sign in via the browser.
        api_key: Option<String>,
        /// Paste/read the API key from stdin instead of opening the browser.
        #[clap(long)]
        paste: bool,
    },
    /// Fetch exactly one resource and print its retrieval evidence
    /// (hashes, status, timestamps) as JSON. HTTP-only: no crawl following,
    /// no browser, no content transformation.
    #[cfg(feature = "fetch")]
    FETCH {
        /// The URL to fetch.
        url: String,
    },
    /// Read exactly one RSS or Atom feed and print its normalized entries
    /// plus retrieval evidence as JSON. Does not fetch any entry's URL.
    #[cfg(feature = "feed")]
    FEED {
        /// The feed URL to read.
        url: String,
        /// Return only the first N entries in source order.
        #[clap(long)]
        limit: Option<usize>,
    },
    /// Read exactly one standard sitemap (urlset or sitemapindex) and print
    /// its discovery candidates plus retrieval evidence as JSON. Does not
    /// fetch any discovered URL.
    #[cfg(feature = "sitemap")]
    SITEMAP {
        /// The sitemap URL to read.
        url: String,
        /// Return only the first N candidates in source order.
        #[clap(long)]
        limit: Option<usize>,
    },
    /// Read exactly one Google News Sitemap and print its News-aware
    /// entries plus retrieval evidence as JSON. Does not fetch any
    /// discovered URL.
    #[cfg(feature = "news_sitemap")]
    #[clap(name = "news-sitemap")]
    #[allow(non_camel_case_types)]
    NEWS_SITEMAP {
        /// The News Sitemap URL to read.
        url: String,
        /// Return only the first N entries in source order.
        #[clap(long)]
        limit: Option<usize>,
    },
    /// Read exactly one robots.txt and print its declared `Sitemap:` URLs
    /// plus retrieval evidence as JSON. Does not fetch any declared URL.
    #[cfg(feature = "robots_sitemap")]
    #[clap(name = "robots-sitemap")]
    #[allow(non_camel_case_types)]
    ROBOTS_SITEMAP {
        /// The robots.txt URL to read.
        url: String,
        /// Return only the first N declared sitemap URLs in source order.
        #[clap(long)]
        limit: Option<usize>,
    },
    /// Search an operator-provided SearXNG instance and return discovery
    /// candidates as JSON. Result URLs are not fetched.
    #[cfg(feature = "search_searxng")]
    SEARCH {
        /// Search query.
        query: String,
        /// Search provider. Currently supported: searxng.
        #[clap(long)]
        provider: String,
        /// Base URL of the operator-provided SearXNG instance.
        #[clap(long)]
        base_url: Option<String>,
        /// Search category. Defaults to ordinary web results.
        #[clap(long, value_enum, default_value_t = SearchCategory::Web)]
        category: SearchCategory,
        /// Return only the first N results in provider order.
        #[clap(long)]
        limit: Option<usize>,
        /// Language code passed through to SearXNG.
        #[clap(long)]
        language: Option<String>,
    },
    /// Launch the canonical Spider MCP server over stdio — the same
    /// server implementation the standalone `spider-mcp` binary runs.
    /// Stdout is reserved for MCP protocol traffic; logs go to stderr.
    #[cfg(feature = "mcp")]
    MCP {
        /// Log level (default: warn). Logs go to stderr.
        #[clap(long, default_value = "warn")]
        log_level: String,
    },
}
