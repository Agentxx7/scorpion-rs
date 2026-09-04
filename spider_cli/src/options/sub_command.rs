use clap::{Subcommand, ValueEnum};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum SearchCategory {
    Web,
    News,
    Image,
    Video,
}

/// `--transport default|tor` — the CLI-local, clap-friendly mirror of
/// `spider::features::transport::TransportMode`. Kept as its own type
/// (rather than deriving `ValueEnum` on the core crate's enum directly)
/// because `spider` has no reason to depend on `clap` — converted into
/// the core type exactly once, in `crate::transport::resolve`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, ValueEnum)]
pub enum TransportModeArg {
    #[default]
    Default,
    Tor,
}

impl From<TransportModeArg> for spider::features::transport::TransportMode {
    fn from(value: TransportModeArg) -> Self {
        match value {
            TransportModeArg::Default => spider::features::transport::TransportMode::Default,
            TransportModeArg::Tor => spider::features::transport::TransportMode::Tor,
        }
    }
}

/// `--transport`/`--tor-proxy` as one reusable Clap argument group
/// (`#[clap(flatten)]`), rather than eight independent copies of the same
/// two fields. Embedded only in the acquisition commands (crawl, scrape,
/// download, fetch, feed, sitemap, news-sitemap, robots-sitemap) — never
/// on `search` or `mcp`, so those commands reject `--transport`/
/// `--tor-proxy` at the parser level, including when written before the
/// subcommand name (there is no top-level `Cli::transport` to catch them).
/// Every caller still resolves through the one canonical
/// `TransportRequest::into_policy()` seam via `crate::transport::resolve`
/// — this type carries no validation of its own.
#[derive(Clone, Debug, Default, clap::Args)]
pub struct TransportArgs {
    /// Transport to use for this acquisition: default (normal networking)
    /// or tor (fail-closed SOCKS5h). Requires --tor-proxy when set to tor.
    /// Does not change the existing URL syntax — an
    /// http://knownservice.onion URL is used exactly as any other URL.
    #[clap(long, value_enum, default_value_t = TransportModeArg::Default)]
    pub transport: TransportModeArg,
    /// Tor SOCKS5h proxy endpoint, e.g. socks5h://127.0.0.1:9050. Required
    /// when --transport tor is set; rejected otherwise.
    #[clap(long)]
    pub tor_proxy: Option<String>,
}

/// Nested operations under the durable research product surface.
#[cfg(feature = "research")]
#[derive(Subcommand)]
pub enum ResearchCommand {
    /// Reopen and display one existing durable research session.
    SHOW {
        /// Canonical research invocation identity.
        research_id: String,
        /// Canonical evidence/session database path. Falls back to
        /// RESEARCH_EVIDENCE_DB.
        #[clap(long)]
        database: Option<std::path::PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum Commands {
    /// Run durable canonical research, or reopen a prior durable session.
    #[cfg(feature = "research")]
    RESEARCH {
        /// Research question. The reserved word `show` selects the nested
        /// reopen command instead of being accepted as a topic.
        topic: Option<String>,
        /// Reopen operation, when `show` is supplied.
        #[clap(subcommand)]
        command: Option<ResearchCommand>,
        /// Canonical evidence/session database path. Falls back to
        /// RESEARCH_EVIDENCE_DB.
        #[clap(long)]
        database: Option<std::path::PathBuf>,
        /// SearXNG base URL. Falls back to SEARXNG_BASE_URL.
        #[clap(long)]
        searxng_url: Option<String>,
        /// OpenAI-compatible API base URL. Falls back to
        /// OPENAI_COMPAT_BASE_URL.
        #[clap(long)]
        openai_base_url: Option<String>,
        /// OpenAI-compatible model name. Falls back to OPENAI_COMPAT_MODEL.
        #[clap(long)]
        model: Option<String>,
        /// Caller-specific extraction instructions.
        #[clap(long)]
        extraction_instructions: Option<String>,
        /// Maximum discovered pages selected for acquisition.
        #[clap(long)]
        max_pages: Option<usize>,
    },
    /// Crawl the website extracting links.
    CRAWL {
        /// sequentially one by one crawl pages
        #[clap(short, long)]
        sync: bool,
        /// stdout all links crawled
        #[clap(short, long)]
        output_links: bool,
        #[clap(flatten)]
        transport_args: TransportArgs,
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
        #[clap(flatten)]
        transport_args: TransportArgs,
    },
    /// Download html markup to destination.
    DOWNLOAD {
        /// store files at target destination
        #[clap(short, long)]
        target_destination: Option<String>,
        #[clap(flatten)]
        transport_args: TransportArgs,
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
        #[clap(flatten)]
        transport_args: TransportArgs,
    },
    /// Run Scorpion's canonical deterministic page audit and print the
    /// resulting Findings, technology markers, and evidence identity as
    /// JSON. Calls the same canonical audit engine spider_audit_page
    /// (MCP) and POST /api/audit (Web Console) call — one audit engine,
    /// three peer adapters, never a duplicated rule/analyzer.
    #[cfg(feature = "audit")]
    AUDIT {
        /// The URL to audit.
        url: String,
        /// Canonical evidence/domain database path. Falls back to
        /// SCORPION_DOMAIN_DB, then RESEARCH_EVIDENCE_DB.
        #[clap(long)]
        database: Option<std::path::PathBuf>,
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
        #[clap(flatten)]
        transport_args: TransportArgs,
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
        #[clap(flatten)]
        transport_args: TransportArgs,
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
        #[clap(flatten)]
        transport_args: TransportArgs,
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
        #[clap(flatten)]
        transport_args: TransportArgs,
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
    /// Discover exactly one Hugging Face model repository's files through
    /// the canonical Hugging Face artifact-discovery provider and print
    /// them as JSON. Read-only: no artifact is downloaded, no file is
    /// written. `resolved_revision` and `download_url` may legitimately be
    /// null; declared identities are provider-recorded claims, never
    /// locally verified checksums.
    #[cfg(feature = "hugging_face_artifacts")]
    #[clap(name = "hugging-face-artifacts")]
    #[allow(non_camel_case_types)]
    HUGGING_FACE_ARTIFACTS {
        /// Provider-native Hugging Face repository identity, e.g.
        /// "owner/model".
        repository_id: String,
        /// Caller-requested branch, tag, or commit. Omitted means the Hub's
        /// own default (`main`) is used server-side, but this remains
        /// recorded as absent — never presented as an immutable resolution.
        #[clap(long)]
        revision: Option<String>,
        /// Maximum file artifacts retained from the single response
        /// (`1..=100`).
        #[clap(long)]
        limit: Option<usize>,
    },
    /// Download exactly one artifact through the canonical artifact-
    /// download binding/execution seam, from a serialized
    /// `ArtifactReference` JSON file to an exact, operator-chosen
    /// destination path. CLI-only surface (no MCP/Web exposure); no
    /// provider filename is ever automatically written, no existing
    /// destination is overwritten, and no archive is ever extracted.
    /// `--max-bytes` is required: there is no unbounded download.
    #[cfg(feature = "artifact_download")]
    #[clap(name = "artifact-download")]
    #[allow(non_camel_case_types)]
    ARTIFACT_DOWNLOAD {
        /// Path to a JSON file holding exactly one serialized
        /// `ArtifactReference` — e.g. one element of the `"artifacts"`
        /// array `scorpion hugging-face-artifacts` prints, saved verbatim.
        #[clap(long)]
        reference_file: String,
        /// Exact destination file path. Never a directory; never derived
        /// from provider-declared metadata. Fails closed if it already
        /// exists.
        #[clap(long)]
        destination: String,
        /// Maximum bytes this download may stream before it fails closed,
        /// mid-stream, before the excess bytes are written to disk.
        /// Required.
        #[clap(long)]
        max_bytes: u64,
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
