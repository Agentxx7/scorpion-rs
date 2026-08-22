// performance reasons jemalloc memory backend for dedicated work and large crawls
#[cfg(all(
    not(windows),
    not(target_os = "android"),
    not(target_env = "musl"),
    feature = "jemalloc",
    not(feature = "mimalloc")
))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

// mimalloc — alternative high-performance allocator with thread-local caches
#[cfg(all(
    not(windows),
    not(target_os = "android"),
    not(target_env = "musl"),
    feature = "mimalloc"
))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

extern crate env_logger;
extern crate serde_json;
extern crate spider;

pub mod build_folders;
#[cfg(any(
    feature = "fetch",
    feature = "feed",
    feature = "sitemap",
    feature = "news_sitemap",
    feature = "robots_sitemap"
))]
pub mod discovery;
#[cfg(feature = "mcp")]
pub mod mcp;
pub mod oauth;
pub mod options;
#[cfg(feature = "research")]
pub mod research;
#[cfg(feature = "search_searxng")]
pub mod search;
pub mod transport;

use clap::Parser;
use options::{Cli, Commands};
use tokio::io::AsyncWriteExt;

use serde_json::{json, Value};

/// Path to the Spider Cloud credentials file (~/.spider/credentials).
fn credentials_path() -> Option<std::path::PathBuf> {
    dirs_home().map(|h| h.join(".spider").join("credentials"))
}

fn dirs_home() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("USERPROFILE").map(std::path::PathBuf::from)
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("HOME").map(std::path::PathBuf::from)
    }
}

/// Read a stored Spider Cloud API key from ~/.spider/credentials.
fn load_api_key() -> Option<String> {
    let path = credentials_path()?;
    std::fs::read_to_string(&path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Persist a Spider Cloud API key to ~/.spider/credentials.
fn save_api_key(key: &str) -> std::io::Result<()> {
    let path = credentials_path().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "cannot determine home directory",
        )
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, key.trim())
}

use spider_transformations::transformation::content::{
    transform_content_input, ReturnFormat, TransformConfig, TransformInput,
};

use spider::client::header::{HeaderMap, HeaderValue};
use spider::features::chrome_common::{
    RequestInterceptConfiguration, WaitForDelay, WaitForIdleNetwork, WaitForSelector,
};
use spider::hashbrown::HashMap;
use spider::page::Page;
use spider::string_concat::{string_concat, string_concat_impl};
use spider::tokio;
use spider::utils::header_utils::header_map_to_hash_map;
use spider::utils::log;
use spider::website::{CrawlStatus, Website};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::build_folders::build_local_path;

/// convert the headers to json
fn headers_to_json(headers: &Option<HeaderMap<HeaderValue>>) -> Value {
    if let Some(headers) = &headers {
        serde_json::to_value(header_map_to_hash_map(headers)).unwrap_or_default()
    } else {
        Value::Null
    }
}

/// handle the headers
#[cfg(feature = "headers")]
fn handle_headers(res: &Page) -> Value {
    headers_to_json(&res.headers)
}

/// handle the headers
#[cfg(not(feature = "headers"))]
fn handle_headers(_res: &Page) -> Value {
    headers_to_json(&None)
}

/// handle the duration elaspsed to milliseconds.
#[cfg(feature = "time")]
fn handle_time(res: &Page, mut json: Value) -> Value {
    json["duration_elapsed_ms"] = json!(res.get_duration_elapsed().as_millis());
    json
}

/// handle the duration elaspsed to milliseconds.
#[cfg(not(feature = "time"))]
fn handle_time(_res: &Page, mut _json: Value) -> Value {
    _json
}

/// handle the HTTP status code.
#[cfg(feature = "status_code")]
fn handle_status_code(res: &Page, mut json: Value) -> Value {
    json["status_code"] = res.status_code.as_u16().into();
    json
}

/// handle the HTTP status code.
#[cfg(not(feature = "status_code"))]
fn handle_status_code(_res: &Page, mut _json: Value) -> Value {
    _json
}

/// handle the remote address.
#[cfg(feature = "remote_addr")]
fn handle_remote_address(res: &Page, mut json: Value) -> Value {
    json["remote_address"] = json!(res.remote_addr);
    json
}

/// handle the remote address.
#[cfg(not(feature = "remote_addr"))]
fn handle_remote_address(_res: &Page, mut _json: Value) -> Value {
    _json
}

/// Surface truthful acquisition provenance already captured on the page
/// (`spider::utils::evidence::page_provenance`) — never recomputed or
/// reinterpreted here. Gated on `fetch`, the same CLI feature that
/// already enables `spider/evidence` for `spider fetch`.
#[cfg(feature = "fetch")]
fn handle_provenance(res: &Page, mut json: Value) -> Value {
    json["provenance"] = json!(spider::utils::evidence::page_provenance(res));
    json
}

/// No `spider/evidence` available without `fetch` — provenance stays
/// absent rather than fabricated.
#[cfg(not(feature = "fetch"))]
fn handle_provenance(_res: &Page, mut _json: Value) -> Value {
    _json
}

/// Log the website status.
fn log_website_status(website: &Website) {
    use CrawlStatus::*;

    let msg = match website.get_status() {
        FirewallBlocked => "blocked by firewall",
        Blocked => "blocked by the network, firewall, or rate limit",
        ServerError => "server error",
        Empty => "returned no content",
        RateLimited => "rate limited",
        Invalid => "invalid url",
        _ => return,
    };

    let url = website.get_url();
    eprintln!("{url:?} - {msg}.");
}

/// Crawl based on runtime mode selection.
async fn crawl_with_mode(website: &mut Website, headless: bool) {
    #[cfg(feature = "chrome")]
    {
        if headless {
            website.crawl().await;
        } else {
            website.crawl_raw().await;
        }
    }

    #[cfg(not(feature = "chrome"))]
    {
        if headless {
            eprintln!(
                "Warning: --headless requested, but this binary was built without the `chrome` feature; using HTTP mode."
            );
        }
        website.crawl().await;
    }
}

/// Print a discovery/fetch/search command's JSON result to stdout and return, or
/// print a retrieval/config error to stderr and exit non-zero. A parser
/// failure is never a process failure here — it is represented inside the
/// JSON itself (`parse_error`), so it always reaches the `Ok` branch.
#[cfg(any(
    feature = "fetch",
    feature = "feed",
    feature = "sitemap",
    feature = "news_sitemap",
    feature = "robots_sitemap",
    feature = "search_searxng"
))]
async fn print_discovery_result(result: Result<String, String>) {
    match result {
        Ok(json) => println!("{json}"),
        Err(message) => {
            eprintln!("Error: {message}");
            std::process::exit(2);
        }
    }
}

/// Print the canonical fetch evidence even when an attempted acquisition did
/// not observe an HTTP response, while mapping that typed distinction to a
/// truthful nonzero shipping-process exit. The synthetic operational status
/// remains useful JSON data; it is never treated as an observed response.
#[cfg(feature = "fetch")]
async fn print_fetch_result(result: Result<discovery::FetchOutput, String>) {
    match result {
        Ok(output) => {
            println!("{}", output.json);
            if !output.response_observed {
                std::process::exit(2);
            }
        }
        Err(message) => {
            eprintln!("Error: {message}");
            std::process::exit(2);
        }
    }
}

#[cfg(feature = "research")]
fn print_research_result(result: Result<research::CommandOutput, String>) {
    match result {
        Ok(output) => {
            print!("{}", output.stdout);
            if let Some(message) = output.stderr {
                eprintln!("Error: {message}");
            }
            if output.exit_code != 0 {
                std::process::exit(output.exit_code);
            }
        }
        Err(message) => {
            eprintln!("Error: {message}");
            std::process::exit(2);
        }
    }
}

/// Section H (CRITICAL): `Website::crawl`/`crawl_raw` return `()` — a Tor
/// preflight rejection (incompatible configuration, `.onion` under
/// Default, Tor not compiled, …) never surfaces on its own and must not
/// be allowed to look like "successful command, empty output". Every
/// crawl/scrape/download spawn captures its `JoinHandle` and returns
/// `website.last_transport_error().cloned()` from the task; this is the
/// single place that result is turned into stderr + a nonzero exit. A
/// task panic (`Err`, unusual — not itself a transport concern) and "no
/// transport error" (`Ok(None)`) are both no-ops.
fn exit_on_transport_error(
    result: Result<
        Option<spider::features::transport::TransportError>,
        spider::tokio::task::JoinError,
    >,
) {
    if let Ok(Some(transport_error)) = result {
        eprintln!("Error: {transport_error}");
        std::process::exit(2);
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // `scorpion mcp` owns logger initialization via its own `--log-level`
    // — checked here, before the generic `--verbose` block below, so
    // `--verbose` can never install a competing logger first (a logger
    // can only be initialized once per process; whichever caller gets
    // there first wins, silently locking out the other). Handled early
    // like AUTHENTICATE/discovery/search: it takes no `url`, builds no
    // `Website`, and does not go through `print_discovery_result` — there
    // is no single JSON result to print, stdout belongs to the MCP
    // protocol for the server's entire lifetime, and a normal exit (the
    // client closing the connection) is not an error.
    #[cfg(feature = "mcp")]
    if let Some(Commands::MCP { log_level }) = &cli.command {
        if let Err(message) = mcp::run(log_level).await {
            eprintln!("Error: {message}");
            std::process::exit(2);
        }
        return;
    }

    if cli.verbose {
        use env_logger::Env;
        let env = Env::default()
            .filter_or("RUST_LOG", "info")
            .write_style_or("RUST_LOG_STYLE", "always");

        env_logger::init_from_env(env);
    }

    // Handle the authenticate command early — it doesn't need a URL.
    if let Some(Commands::AUTHENTICATE { api_key, paste }) = &cli.command {
        use std::io::IsTerminal;

        let key = if let Some(k) = api_key {
            k.clone()
        } else if *paste || !std::io::stdin().is_terminal() {
            // Explicit --paste, or a piped key (scripting): read it from stdin.
            eprint!("Enter your Spider Cloud API key: ");
            let mut buf = String::new();
            std::io::stdin()
                .read_line(&mut buf)
                .expect("failed to read API key from stdin");
            buf.trim().to_string()
        } else {
            // Interactive: sign in through the browser and provision a key.
            match oauth::login().await {
                Ok(k) => k,
                Err(e) => {
                    eprintln!("Sign-in failed: {e}");
                    std::process::exit(1);
                }
            }
        };

        if key.is_empty() {
            eprintln!("Error: API key cannot be empty.");
            std::process::exit(1);
        }

        match save_api_key(&key) {
            Ok(()) => {
                if let Some(path) = credentials_path() {
                    println!("API key saved to {}", path.display());
                } else {
                    println!("API key saved.");
                }
            }
            Err(e) => {
                eprintln!("Failed to save API key: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    #[cfg(feature = "research")]
    if let Some(Commands::RESEARCH {
        topic,
        command,
        database,
        searxng_url,
        openai_base_url,
        model,
        extraction_instructions,
        max_pages,
    }) = &cli.command
    {
        use options::sub_command::ResearchCommand;
        let request = match command {
            Some(ResearchCommand::SHOW {
                research_id,
                database,
            }) => research::Request::Show(research::ShowParams {
                research_id: research_id.clone(),
                database: database.clone(),
            }),
            None => match topic.clone() {
                Some(topic) => research::Request::Run(research::RunParams {
                    topic,
                    database: database.clone(),
                    searxng_url: searxng_url.clone(),
                    openai_base_url: openai_base_url.clone(),
                    model: model.clone(),
                    extraction_instructions: extraction_instructions.clone(),
                    max_pages: *max_pages,
                }),
                None => {
                    eprintln!("Error: research requires a topic or the show subcommand");
                    std::process::exit(2);
                }
            },
        };
        print_research_result(research::execute(request, cli.verbose).await);
        return;
    }

    // Canonical discovery/fetch commands — handled early, exactly like
    // AUTHENTICATE: they take their own `url` field, never the shared
    // `--url`/crawl-configuration preamble below, and never build a
    // `Website` (no crawl following, no browser).
    #[cfg(any(
        feature = "fetch",
        feature = "feed",
        feature = "sitemap",
        feature = "news_sitemap",
        feature = "robots_sitemap",
        feature = "search_searxng"
    ))]
    {
        #[cfg(feature = "fetch")]
        if let Some(Commands::FETCH {
            url,
            transport_args,
        }) = &cli.command
        {
            let result = match crate::transport::resolve(
                transport_args.transport,
                transport_args.tor_proxy.clone(),
            ) {
                Ok(policy) => discovery::run_fetch(url, policy).await,
                Err(message) => Err(message),
            };
            return print_fetch_result(result).await;
        }
        #[cfg(feature = "feed")]
        if let Some(Commands::FEED {
            url,
            limit,
            transport_args,
        }) = &cli.command
        {
            let result = match crate::transport::resolve(
                transport_args.transport,
                transport_args.tor_proxy.clone(),
            ) {
                Ok(policy) => discovery::run_feed(url, *limit, policy).await,
                Err(message) => Err(message),
            };
            return print_discovery_result(result).await;
        }
        #[cfg(feature = "sitemap")]
        if let Some(Commands::SITEMAP {
            url,
            limit,
            transport_args,
        }) = &cli.command
        {
            let result = match crate::transport::resolve(
                transport_args.transport,
                transport_args.tor_proxy.clone(),
            ) {
                Ok(policy) => discovery::run_sitemap(url, *limit, policy).await,
                Err(message) => Err(message),
            };
            return print_discovery_result(result).await;
        }
        #[cfg(feature = "news_sitemap")]
        if let Some(Commands::NEWS_SITEMAP {
            url,
            limit,
            transport_args,
        }) = &cli.command
        {
            let result = match crate::transport::resolve(
                transport_args.transport,
                transport_args.tor_proxy.clone(),
            ) {
                Ok(policy) => discovery::run_news_sitemap(url, *limit, policy).await,
                Err(message) => Err(message),
            };
            return print_discovery_result(result).await;
        }
        #[cfg(feature = "robots_sitemap")]
        if let Some(Commands::ROBOTS_SITEMAP {
            url,
            limit,
            transport_args,
        }) = &cli.command
        {
            let result = match crate::transport::resolve(
                transport_args.transport,
                transport_args.tor_proxy.clone(),
            ) {
                Ok(policy) => discovery::run_robots_sitemap(url, *limit, policy).await,
                Err(message) => Err(message),
            };
            return print_discovery_result(result).await;
        }
        #[cfg(feature = "search_searxng")]
        if let Some(Commands::SEARCH {
            query,
            provider,
            base_url,
            category,
            limit,
            language,
        }) = &cli.command
        {
            let params = search::SearchParams {
                query: query.clone(),
                provider: provider.clone(),
                base_url: base_url.clone(),
                category: *category,
                limit: *limit,
                language: language.clone(),
            };
            return print_discovery_result(search::run(params).await).await;
        }
    }

    if cli.url.is_empty() {
        eprintln!("Error: --url is required for crawl/scrape/download commands.");
        std::process::exit(1);
    }

    let url = if cli.url.starts_with("http") {
        cli.url
    } else {
        string_concat!("https://", cli.url)
    };

    let mut website = Website::new(&url);

    website
        .with_respect_robots_txt(cli.respect_robots_txt)
        .with_subdomains(cli.subdomains)
        .with_chrome_intercept(RequestInterceptConfiguration::new(cli.block_images))
        .with_danger_accept_invalid_certs(cli.accept_invalid_certs)
        .with_full_resources(cli.full_resources)
        .with_tld(cli.tld)
        .with_blacklist_url(
            cli.blacklist_url
                .map(|blacklist_url| blacklist_url.split(',').map(|l| l.into()).collect()),
        )
        .with_budget(cli.budget.as_ref().map(|budget| {
            budget
                .split(',')
                .collect::<Vec<_>>()
                .chunks(2)
                .map(|x| (x[0], x[1].parse::<u32>().unwrap_or_default()))
                .collect::<HashMap<&str, u32>>()
        }));

    if let Some(agent) = &cli.agent {
        website.with_user_agent(Some(agent));
    }
    if let Some(delay) = cli.delay {
        website.with_delay(delay);
    }
    if let Some(limit) = cli.limit {
        website.with_limit(limit);
    }
    if let Some(depth) = cli.depth {
        website.with_depth(depth);
    }

    if let Some(proxy_url) = cli.proxy_url {
        if !proxy_url.is_empty() {
            website.with_proxies(Some(vec![proxy_url]));
        }
    }

    if let Some(domains) = cli.external_domains {
        website.with_external_domains(Some(domains.into_iter()));
    }

    if let Some(wait_for_idle_network) = cli.wait_for_idle_network {
        website.with_wait_for_idle_network(Some(WaitForIdleNetwork::new(Some(
            Duration::from_millis(wait_for_idle_network),
        ))));
    }
    if let Some(wait_for_idle_network0) = cli.wait_for_idle_network0 {
        website.with_wait_for_idle_network0(Some(WaitForIdleNetwork::new(Some(
            Duration::from_millis(wait_for_idle_network0),
        ))));
    }
    if let Some(wait_for_almost_idle_network0) = cli.wait_for_almost_idle_network0 {
        website.with_wait_for_almost_idle_network0(Some(WaitForIdleNetwork::new(Some(
            Duration::from_millis(wait_for_almost_idle_network0),
        ))));
    }
    if let Some(selector) = cli.wait_for_idle_dom {
        website.with_wait_for_idle_dom(Some(WaitForSelector::new(
            Some(Duration::from_secs(30)),
            selector,
        )));
    }
    if let Some(selector) = cli.wait_for_selector {
        website.with_wait_for_selector(Some(WaitForSelector::new(
            Some(Duration::from_secs(60)),
            selector,
        )));
    }
    if let Some(wait_for_delay) = cli.wait_for_delay {
        website.with_wait_for_delay(Some(WaitForDelay::new(Some(Duration::from_millis(
            wait_for_delay,
        )))));
    }

    if let Some(cookie) = &cli.cookie {
        website.with_cookies(cookie);
    }

    if let Some(ref chrome_connection_url) = cli.chrome_connection_url {
        website.with_chrome_connection(Some(chrome_connection_url.clone()));
    }

    if cli.stealth {
        website.with_stealth(true);
    }

    #[cfg(feature = "warc")]
    if let Some(ref warc_path) = cli.warc {
        website
            .configuration
            .with_warc(spider::utils::warc::WarcConfig {
                path: warc_path.clone(),
                write_warcinfo: true,
                ..Default::default()
            });
    }

    // Cache namespace (partition key for country / proxy pool / tenant / …).
    // Flag first, then SPIDER_CACHE_NAMESPACE env var.
    {
        let ns = cli
            .cache_namespace
            .clone()
            .or_else(|| std::env::var("SPIDER_CACHE_NAMESPACE").ok())
            .filter(|s| !s.is_empty());
        if let Some(ns) = ns {
            website.with_cache_namespace(Some(ns));
        }
    }

    // Resolve Spider Cloud API key: --spider-cloud-key > SPIDER_CLOUD_API_KEY env > stored credentials.
    {
        let api_key = cli
            .spider_cloud_key
            .or_else(|| std::env::var("SPIDER_CLOUD_API_KEY").ok())
            .or_else(load_api_key);

        if let Some(key) = api_key {
            #[cfg(feature = "spider_cloud")]
            {
                if cli.spider_cloud_browser {
                    website.with_spider_browser(&key);
                } else {
                    use spider::configuration::{SpiderCloudConfig, SpiderCloudMode};

                    let mode = match cli.spider_cloud_mode.as_deref() {
                        Some("proxy") => SpiderCloudMode::Proxy,
                        Some("unblocker") => SpiderCloudMode::Unblocker,
                        Some("fallback") => SpiderCloudMode::Fallback,
                        Some("smart") => SpiderCloudMode::Smart,
                        // api is the default mode.
                        _ => SpiderCloudMode::Api,
                    };

                    let config = SpiderCloudConfig::new(&key).with_mode(mode);
                    website.with_spider_cloud_config(config);
                }
            }
            #[cfg(not(feature = "spider_cloud"))]
            {
                let _ = key;
                eprintln!("Warning: Spider Cloud API key provided but the `spider_cloud` feature is not enabled. Ignoring.");
            }
        }
    }

    let return_headers = cli.return_headers;
    let use_headless = cli.headless && !cli.http;
    let return_format = cli.return_format.clone();

    // `--transport`/`--tor-proxy` are scoped per-command (Section C of the
    // blocker-fix frontier) — CRAWL/SCRAPE/DOWNLOAD carry their own
    // `TransportArgs` (flattened, same as fetch/feed/sitemap/etc.), never a
    // top-level `Cli` field, so `search`/`mcp` reject the flags at the
    // parser level, including when written before the subcommand name.
    // Only CRAWL/SCRAPE/DOWNLOAD/None can still be `cli.command` here —
    // AUTHENTICATE/discovery/search/mcp all `return` above.
    let transport_args = match &cli.command {
        Some(Commands::CRAWL { transport_args, .. }) => transport_args.clone(),
        Some(Commands::SCRAPE { transport_args, .. }) => transport_args.clone(),
        Some(Commands::DOWNLOAD { transport_args, .. }) => transport_args.clone(),
        _ => crate::options::sub_command::TransportArgs::default(),
    };

    // Resolve --transport/--tor-proxy exactly once (Section B/G), before
    // any target networking, and reject incompatible routes up front
    // (Section I) — headless/browser mode, the legacy --proxy-url proxy
    // list, and an active Spider Cloud configuration are all
    // audited-unsafe combinations with Tor. `Website::tor_crawl_preflight`
    // (core) independently re-checks these and more (defense in depth —
    // see the error-propagation block below, which surfaces whatever it
    // rejects too), but failing here gives an immediate, specific message
    // before a `Website` is even built.
    let transport_policy =
        match crate::transport::resolve(transport_args.transport, transport_args.tor_proxy.clone())
        {
            Ok(policy) => policy,
            Err(message) => {
                eprintln!("Error: {message}");
                std::process::exit(2);
            }
        };
    let tor_requested = matches!(
        transport_policy,
        spider::features::transport::TransportPolicy::Tor(_)
    );
    if tor_requested {
        if use_headless {
            eprintln!(
                "Error: --transport tor cannot be combined with --headless — Tor crawling is \
                 HTTP-only; use --http (the default) or omit --headless."
            );
            std::process::exit(2);
        }
        if website.configuration.proxies.is_some() {
            eprintln!(
                "Error: --transport tor cannot be combined with --proxy-url — Tor uses its own \
                 audited SOCKS5h route exclusively."
            );
            std::process::exit(2);
        }
        #[cfg(feature = "spider_cloud")]
        if website.configuration.spider_cloud.is_some() {
            eprintln!(
                "Error: --transport tor cannot be combined with Spider Cloud \
                 (--spider-cloud-key / SPIDER_CLOUD_API_KEY / stored credentials) — Spider \
                 Cloud routing is not audited for Tor."
            );
            std::process::exit(2);
        }
        #[cfg(all(feature = "spider_cloud", feature = "chrome"))]
        if website.configuration.spider_browser.is_some() {
            eprintln!(
                "Error: --transport tor cannot be combined with Spider Browser Cloud — remote \
                 CDP browser routing is not audited for Tor."
            );
            std::process::exit(2);
        }
        website.with_transport(transport_policy);
    }

    match website
        .build()
    {
        Ok(mut website) => {
            let mut rx2 = website.subscribe(0);

            match cli.command {
                Some(Commands::CRAWL {
                    sync,
                    output_links,
                    ..
                }) => {
                    if sync {
                        // remove concurrency
                        website.with_delay(1);
                    }

                    let mut stdout = tokio::io::stdout();

                    // Captured (not fire-and-forget) so a Tor preflight
                    // rejection can never surface as a successful command
                    // with empty output (Section H) — always awaited below,
                    // regardless of --output-links, so the crawl always
                    // actually completes before the process exits.
                    let crawl_task = tokio::spawn(async move {
                        crawl_with_mode(&mut website, use_headless).await;
                        log_website_status(&website);
                        website.last_transport_error().cloned()
                    });

                    if output_links {
                        while let Ok(res) = rx2.recv().await {
                            if return_headers {
                                let headers_json = handle_headers(&res);

                                let _ = stdout
                                    .write_all(format!("{} - {}\n", res.get_url(), headers_json).as_bytes())
                                    .await;
                            } else {
                                let _ = stdout.write_all(string_concat!(res.get_url(), "\n").as_bytes()).await;
                            }
                        }
                    }

                    exit_on_transport_error(crawl_task.await);
                }
                Some(Commands::DOWNLOAD {
                    target_destination, ..
                }) => {
                    let tmp_dir = target_destination
                        .to_owned()
                        .unwrap_or(String::from("./_temp_spider_downloads/"));

                    let tmp_path = Path::new(&tmp_dir);

                    if !Path::new(&tmp_path).exists() {
                        let _ = tokio::fs::create_dir_all(tmp_path).await;
                    }

                    let download_path = PathBuf::from(tmp_path);

                    let crawl_task = tokio::spawn(async move {
                        crawl_with_mode(&mut website, use_headless).await;
                        log_website_status(&website);
                        website.last_transport_error().cloned()
                    });

                    while let Ok(res) = rx2.recv().await {
                        #[allow(unused_mut)]
                        let mut res = res;
                        if let Some(parsed_url) = res.get_url_parsed() {
                            log("Storing", parsed_url);
                            let mut url_path = parsed_url.path().to_string();

                            if url_path.is_empty() {
                                url_path = "/".into();
                            }

                            let final_path = build_local_path(&download_path, &url_path);

                            if let Some(parent) = final_path.parent() {
                                if !parent.exists() {
                                    if let Err(e) = tokio::fs::create_dir_all(parent).await {
                                        eprintln!("Failed to create dirs {:?}: {e}", parent);
                                        continue;
                                    }
                                }
                            }

                            if let Some(bytes) = res.get_bytes() {
                                match tokio::fs::OpenOptions::new()
                                    .write(true)
                                    .create(true)
                                    .truncate(true)
                                    .open(&final_path)
                                    .await
                                {
                                    Ok(mut file) => {
                                        if let Err(e) = file.write_all(bytes).await {
                                            eprintln!("Failed to write {:?}: {e}", final_path);
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!("Unable to open file {:?}: {e}", final_path);
                                    }
                                }
                            }
                        }
                    }

                    exit_on_transport_error(crawl_task.await);
                }
                Some(Commands::SCRAPE {
                    output_html,
                    output_links,
                    ..
                }) => {
                    let mut stdout = tokio::io::stdout();
                    // A failed attempted acquisition is still broadcast as a
                    // truthful synthetic-status Page and must still be
                    // presented. Track the canonical observed-response field
                    // separately so that presentation does not imply process
                    // success when no HTTP response existed.
                    let mut response_observed = false;

                    if output_links {
                        website.configuration.return_page_links = true;
                    }

                    // Scrape returns the page content by default (markdown via
                    // --return-format); --output-html returns the raw HTML instead.
                    let transform_conf = TransformConfig {
                        return_format: if output_html {
                            ReturnFormat::Raw
                        } else {
                            ReturnFormat::from_str(&return_format)
                        },
                        ..Default::default()
                    };

                    let crawl_task = tokio::spawn(async move {
                        crawl_with_mode(&mut website, use_headless).await;
                        log_website_status(&website);
                        website.last_transport_error().cloned()
                    });

                    while let Ok(res) = rx2.recv().await {
                        response_observed |= res.observed_status_code.is_some();
                        let content = {
                            let input = TransformInput {
                                url: res.get_url_parsed_ref().as_ref(),
                                content: res.get_html_bytes_u8(),
                                screenshot_bytes: None,
                                encoding: None,
                                selector_config: None,
                                ignore_tags: None,
                            };
                            transform_content_input(input, &transform_conf)
                        };

                        let page_json = json!({
                            "url": res.get_url(),
                            "content": content,
                            "links": match res.page_links {
                                Some(ref s) => s.iter().map(|i| i.inner().to_string()).collect::<serde_json::Value>(),
                                _ => Default::default()
                            },
                            "headers": if return_headers {
                               handle_headers(&res)
                            } else {
                                Default::default()
                            }
                        });

                        let page_json = handle_time(&res, page_json);
                        let page_json = handle_status_code(&res, page_json);
                        let page_json = handle_remote_address(&res, page_json);
                        let page_json = handle_provenance(&res, page_json);

                        match serde_json::to_string_pretty(&page_json) {
                            Ok(j) => {
                               if let Err(e) = stdout.write_all(string_concat!(j, "\n").as_bytes()).await {
                                    eprintln!("{:?}", e)
                               }
                            }
                            Err(e) =>  eprintln!("{:?}", e)
                        }
                    }

                    exit_on_transport_error(crawl_task.await);
                    if !response_observed {
                        std::process::exit(2);
                    }
                }
                Some(Commands::AUTHENTICATE { .. }) => {},
                // Discovery/fetch commands are handled earlier and always
                // `return` before reaching this match — unreachable here.
                #[cfg(any(
                    feature = "fetch",
                    feature = "feed",
                    feature = "sitemap",
                    feature = "news_sitemap",
                    feature = "robots_sitemap",
                    feature = "search_searxng",
                    feature = "research",
                    feature = "mcp"
                ))]
                Some(_) => unreachable!(
                    "discovery/fetch/mcp commands return early, before this match"
                ),
                None => ()
            }
        }
        _ =>  println!("Invalid website URL passed in. The url should start with http:// or https:// following the website domain ex: https://example.com.")
    }
}
