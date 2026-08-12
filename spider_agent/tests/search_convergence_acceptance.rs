//! Shared acceptance contract for
//! `SCORPION_SPIDER_AGENT_SEARCH_STACK_CONVERGENCE_001`.
//!
//! Derived from `docs/frontier/SEARCH_STACK_CONVERGENCE_SDD.md` before
//! production implementation. Run with all agent search-provider features.

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone)]
struct SourceFile {
    path: String,
    contents: String,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

fn collect(dir: &Path, base: &Path, files: &mut Vec<SourceFile>) {
    if !dir.exists() {
        return;
    }
    for entry in fs::read_dir(dir).expect("read source directory") {
        let path = entry.expect("read entry").path();
        if path.is_dir() {
            collect(&path, base, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(SourceFile {
                path: path
                    .strip_prefix(base)
                    .expect("relative source path")
                    .to_string_lossy()
                    .replace('\\', "/"),
                contents: fs::read_to_string(path).expect("read source file"),
            });
        }
    }
}

fn crate_sources(crate_name: &str) -> Vec<SourceFile> {
    let base = root().join(crate_name).join("src");
    let mut files = Vec::new();
    collect(&base, &base, &mut files);
    files
}

fn workspace_search_sources() -> Vec<SourceFile> {
    [
        "spider",
        "spider_agent",
        "spider_agent_types",
        "spider_search",
    ]
    .into_iter()
    .flat_map(|crate_name| {
        crate_sources(crate_name).into_iter().map(move |mut file| {
            file.path = format!("{crate_name}/src/{}", file.path);
            file
        })
    })
    .collect()
}

fn locations(files: &[SourceFile], pattern: &str) -> Vec<String> {
    files
        .iter()
        .filter(|file| file.contents.contains(pattern))
        .map(|file| file.path.clone())
        .collect()
}

fn assert_same_type<T>(_: &T, _: &T) {}

#[cfg(feature = "search")]
#[test]
fn consumer_models_and_error_are_type_identical() {
    assert_same_type(
        &spider_agent::SearchOptions::new(),
        &spider::features::search::SearchOptions::new(),
    );
    assert_same_type(
        &spider_agent::TimeRange::Week,
        &spider::features::search::TimeRange::Week,
    );
    assert_same_type(
        &spider_agent::SearchResult::new("title", "https://example.com", 1),
        &spider::features::search::SearchResult::new("title", "https://example.com", 1),
    );
    assert_same_type(
        &spider_agent::SearchResults::new("query"),
        &spider::features::search::SearchResults::new("query"),
    );
    assert_same_type(
        &spider_agent::SearchError::NoProvider,
        &spider::features::search::SearchError::NoProvider,
    );
}

#[cfg(feature = "search_serper")]
#[test]
fn provider_seam_and_concrete_provider_are_type_identical() {
    fn spider_bound<P: spider::features::search::SearchProvider + ?Sized>(_: &P) {}
    fn agent_bound<P: spider_agent::SearchProvider + ?Sized>(_: &P) {}

    let agent = spider_agent::SerperProvider::new("key");
    let spider = spider::features::search_providers::SerperProvider::new("key");
    assert_same_type(&agent, &spider);
    spider_bound(&agent);
    agent_bound(&spider);

    let object: Box<dyn spider_agent::SearchProvider> = Box::new(agent);
    assert_eq!(object.provider_name(), "serper");
}

#[cfg(feature = "search")]
#[test]
fn canonical_model_behavior_is_preserved() {
    let options = spider_agent::SearchOptions::new()
        .with_limit(7)
        .with_country("se")
        .with_language("sv")
        .with_site_filter(vec!["example.com".into()])
        .with_exclude_domains(vec!["excluded.example".into()])
        .with_time_range(spider_agent::TimeRange::Month);
    assert_eq!(options.limit, Some(7));
    assert_eq!(options.country.as_deref(), Some("se"));
    assert_eq!(options.language.as_deref(), Some("sv"));
    assert_eq!(options.time_range, Some(spider_agent::TimeRange::Month));
    assert!(options.include_keywords.is_none());

    let mut results = spider_agent::SearchResults::new("query");
    results.push(
        spider_agent::SearchResult::new("title", "https://example.com", 1)
            .with_snippet("snippet")
            .with_date("2026-01-01")
            .with_score(0.5),
    );
    assert_eq!(results.urls(), vec!["https://example.com"]);
    assert_eq!(results.results[0].score, Some(0.5));
}

#[cfg(feature = "search")]
#[test]
fn canonical_error_semantics_are_preserved() {
    use spider_agent::SearchError;
    assert_eq!(
        SearchError::RequestFailed("timeout".into()).to_string(),
        "Search request failed: timeout"
    );
    assert_eq!(
        SearchError::AuthenticationFailed.to_string(),
        "Search authentication failed"
    );
    assert_eq!(
        SearchError::RateLimited.to_string(),
        "Search rate limit exceeded"
    );
    assert_eq!(
        SearchError::InvalidQuery("empty".into()).to_string(),
        "Invalid search query: empty"
    );
    assert_eq!(
        SearchError::ProviderError("api".into()).to_string(),
        "Search provider error: api"
    );
    assert_eq!(
        SearchError::NoProvider.to_string(),
        "No search provider configured"
    );

    let wrapped: spider_agent::AgentError = SearchError::NoProvider.into();
    assert!(wrapped
        .to_string()
        .contains("No search provider configured"));
    assert!(std::error::Error::source(&wrapped).is_some());
}

#[cfg(feature = "search")]
#[tokio::test]
async fn missing_provider_fails_closed_without_fallback() {
    let agent = spider_agent::Agent::builder().build().expect("build agent");
    let error = agent.search("query").await.expect_err("must fail closed");
    assert!(matches!(
        error,
        spider_agent::AgentError::NotConfigured("search provider")
    ));
}

#[test]
fn search_seam_has_exactly_one_owner() {
    let hits = locations(&workspace_search_sources(), "pub trait SearchProvider");
    assert_eq!(hits, ["spider_search/src/search.rs"]);
}

#[test]
fn search_models_and_error_have_exactly_one_owner() {
    let files = workspace_search_sources();
    for pattern in [
        "pub struct SearchOptions",
        "pub enum TimeRange",
        "pub struct SearchResults",
        "pub struct SearchResult",
        "pub enum SearchError",
    ] {
        let hits = locations(&files, pattern);
        assert_eq!(
            hits,
            ["spider_search/src/search.rs"],
            "unexpected owners for {pattern}"
        );
    }
}

#[test]
fn each_provider_has_exactly_one_implementation() {
    let files = workspace_search_sources();
    for provider in [
        "SerperProvider",
        "BraveProvider",
        "BingProvider",
        "TavilyProvider",
        "SearxngProvider",
    ] {
        let hits = locations(&files, &format!("impl SearchProvider for {provider}"));
        assert_eq!(hits.len(), 1, "duplicate or missing {provider}: {hits:?}");
        assert!(hits[0].starts_with("spider_search/src/providers/"));
    }
}

#[test]
fn providers_use_canonical_transport_without_client_bypass() {
    let files = crate_sources("spider_search");
    let provider_files: Vec<_> = files
        .iter()
        .filter(|file| file.path.starts_with("providers/"))
        .collect();
    assert!(!provider_files.is_empty());

    for file in provider_files {
        for forbidden in [
            "reqwest::Client",
            "ClientBuilder",
            "Client::new",
            "Client::builder",
            "wreq::Client",
            ".send()",
            "client: Option",
            "client: &",
        ] {
            assert!(
                !file.contents.contains(forbidden),
                "provider bypass {forbidden:?} in {}",
                file.path
            );
        }
    }

    let manifest = fs::read_to_string(root().join("spider_search/Cargo.toml"))
        .expect("spider_search manifest");
    assert!(manifest.contains("spider_transport"));
    assert!(!manifest.contains("path = \"../spider\""));
    assert!(!manifest.contains("path = \"../spider_agent\""));
}

#[test]
fn provider_auth_contracts_remain_truthful() {
    let providers = crate_sources("spider_search");
    let joined = providers
        .iter()
        .filter(|file| file.path.starts_with("providers/"))
        .map(|file| file.contents.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for header in [
        "X-API-KEY",
        "X-Subscription-Token",
        "Ocp-Apim-Subscription-Key",
    ] {
        assert!(joined.contains(header), "missing auth header {header}");
    }
    assert!(joined.contains("\"api_key\""), "Tavily body auth missing");
    assert!(joined.contains("AuthenticationFailed"));
    assert!(joined.contains("RateLimited"));
}

#[test]
fn consumer_search_modules_are_implementation_free_facades() {
    for relative in [
        "spider/src/features/search.rs",
        "spider/src/features/search_providers/mod.rs",
        "spider_agent/src/search/mod.rs",
    ] {
        let contents = fs::read_to_string(root().join(relative)).expect("read façade");
        for forbidden in [
            "pub trait SearchProvider",
            "pub struct SearchOptions",
            "pub struct SearchResults",
            "pub enum SearchError",
            "impl SearchProvider for",
            "reqwest::Client",
            ".send()",
        ] {
            assert!(
                !contents.contains(forbidden),
                "implementation {forbidden:?} in façade {relative}"
            );
        }
    }
}

#[test]
fn agent_legacy_provider_files_are_physically_absent() {
    for provider in ["serper.rs", "brave.rs", "bing.rs", "tavily.rs"] {
        assert!(!root()
            .join("spider_agent/src/search")
            .join(provider)
            .exists());
    }
}

#[test]
fn spider_legacy_provider_files_are_physically_absent() {
    for provider in [
        "serper.rs",
        "brave.rs",
        "bing.rs",
        "tavily.rs",
        "searxng.rs",
    ] {
        assert!(!root()
            .join("spider/src/features/search_providers")
            .join(provider)
            .exists());
    }
}

#[test]
fn agent_dispatch_does_not_lend_its_client() {
    let contents = fs::read_to_string(root().join("spider_agent/src/agent.rs")).expect("agent.rs");
    let start = contents
        .find("pub async fn search_with_options")
        .expect("search dispatch");
    let end = (start + 1800).min(contents.len());
    let dispatch = &contents[start..end];
    assert!(!dispatch.contains("self.client"));
    assert!(!dispatch.contains("Client::new"));
    assert!(!dispatch.contains("Client::builder"));
}

#[test]
fn canonical_search_has_no_legacy_dependency_or_silent_fallback() {
    for file in crate_sources("spider_search") {
        assert!(!file.contents.contains("spider_agent::"));
        assert!(!file.contents.contains("spider::features::search"));
        assert!(!file.contents.contains("unwrap_or_else(|_|"));
    }
}

#[test]
fn feature_topology_is_forwarded_and_fail_closed() {
    let canonical = fs::read_to_string(root().join("spider_search/Cargo.toml")).unwrap();
    let spider = fs::read_to_string(root().join("spider/Cargo.toml")).unwrap();
    let agent = fs::read_to_string(root().join("spider_agent/Cargo.toml")).unwrap();
    for feature in [
        "search_serper",
        "search_brave",
        "search_bing",
        "search_tavily",
    ] {
        assert!(canonical.contains(feature));
        assert!(spider.contains(&format!("spider_search/{feature}")));
        assert!(agent.contains(&format!("spider_search/{feature}")));
    }
    assert!(canonical.contains("search_searxng"));
    assert!(spider.contains("spider_search/search_searxng"));
}

#[test]
fn scanner_negative_proofs_detect_every_violation_class() {
    let synthetic = vec![
        SourceFile {
            path: "legacy.rs".into(),
            contents: "pub trait SearchProvider {}\npub struct SearchOptions;".into(),
        },
        SourceFile {
            path: "provider.rs".into(),
            contents: "impl SearchProvider for SerperProvider {}\nreqwest::Client::new().send();"
                .into(),
        },
        SourceFile {
            path: "canonical.rs".into(),
            contents: "use spider_agent::search::SearchProvider;".into(),
        },
    ];
    assert_eq!(
        locations(&synthetic, "pub trait SearchProvider"),
        ["legacy.rs"]
    );
    assert_eq!(
        locations(&synthetic, "pub struct SearchOptions"),
        ["legacy.rs"]
    );
    assert_eq!(
        locations(&synthetic, "impl SearchProvider for SerperProvider"),
        ["provider.rs"]
    );
    assert_eq!(locations(&synthetic, "reqwest::Client"), ["provider.rs"]);
    assert_eq!(locations(&synthetic, ".send()"), ["provider.rs"]);
    assert_eq!(locations(&synthetic, "spider_agent::"), ["canonical.rs"]);
}
