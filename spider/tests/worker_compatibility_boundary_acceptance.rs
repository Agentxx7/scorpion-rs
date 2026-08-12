use std::{
    fs,
    path::{Path, PathBuf},
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn read(path: &str) -> String {
    fs::read_to_string(root().join(path)).unwrap()
}

#[test]
fn documentation_owns_the_worker_classification() {
    let architecture = read("SCORPION_ARCHITECTURE.md");
    assert!(architecture.contains("`spider_worker` | `UPSTREAM_COMPATIBILITY_BOUNDARY`"));
    assert!(architecture.contains("`COMPATIBILITY_LOCAL_DEFENSE`"));
}

#[test]
fn worker_is_terminal_and_depends_on_spider() {
    let worker = read("spider_worker/Cargo.toml");
    assert!(worker.contains("[dependencies.spider]"));
    assert!(!root().join("spider_worker/src/lib.rs").exists());

    for entry in fs::read_dir(root()).unwrap().flatten() {
        let manifest = entry.path().join("Cargo.toml");
        if !manifest.exists() || entry.file_name() == "spider_worker" {
            continue;
        }
        let contents = fs::read_to_string(&manifest).unwrap();
        assert!(
            !contents.contains("path = \"../spider_worker\""),
            "reverse dependency in {}",
            manifest.display()
        );
    }
}

#[test]
fn worker_owns_no_canonical_seam_or_model() {
    let worker = read("spider_worker/src/main.rs");
    for forbidden in [
        "TransportPolicy",
        "SearchProvider",
        "SearchOptions",
        "SearchError",
        "SourceProvider",
        "EvidenceBundle",
        "ArtifactReference",
        "AgentConfig",
        "WatchState",
        "pub struct Job",
    ] {
        assert!(
            !worker.contains(forbidden),
            "worker must not own {forbidden}"
        );
    }
}

#[test]
fn compatibility_primitives_are_exact_and_tor_is_rejected() {
    let worker = read("spider_worker/src/main.rs");
    for primitive in [
        "configure_http_client()",
        "Page::new_page_streaming(",
        "fetch_page_html_raw(",
    ] {
        assert_eq!(
            worker.matches(primitive).count(),
            1,
            "unexpected use count for {primitive}"
        );
    }
    let website = read("spider/src/website.rs");
    assert!(website.contains("decentralized crawling is not audited for Tor"));
}

#[test]
fn worker_ssrf_defense_is_private_and_compatibility_local() {
    let worker = read("spider_worker/src/main.rs");
    assert!(worker.contains("fn target_host_blocked("));
    assert!(!worker.contains("pub fn target_host_blocked("));
    assert!(!worker.contains("spider_transport::"));
}

#[test]
fn synthetic_negative_cases_are_detectable() {
    let canonical_dependency = "spider_worker = { path = \"../spider_worker\" }";
    let canonical_fallback = "fallback_to_spider_worker(worker_url)";
    let shadow_model = "pub trait SearchProvider {}";
    let ssrf_import = "use spider_worker::target_host_blocked;";
    assert!(canonical_dependency.contains("path = \"../spider_worker\""));
    assert!(canonical_fallback.contains("spider_worker"));
    assert!(shadow_model.contains("trait SearchProvider"));
    assert!(ssrf_import.contains("spider_worker::target_host_blocked"));
}
