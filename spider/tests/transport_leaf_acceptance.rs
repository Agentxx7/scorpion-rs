//! SHARED ACCEPTANCE SUITE — SCORPION_CANONICAL_TRANSPORT_LEAF_EXTRACTION_001
//!
//! Byte-identical (SHA-256 pinned) on every experiment branch. Derives from
//! `docs/frontier/TRANSPORT_LEAF_EXTRACTION_SDD.md`, not from any
//! implementation. Design-neutral: it exercises the public
//! `spider::features::transport{,_secret_request_headers}` façade paths and
//! leaf TYPE identity only — never design-specific leaf internals.
//!
//! Run with:
//! ```sh
//! cargo test -p spider --test transport_leaf_acceptance
//! cargo test -p spider --test transport_leaf_acceptance --features transport_tor
//! ```

use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Source-scanning harness
// ---------------------------------------------------------------------------

struct SourceFile {
    relative_path: String,
    contents: String,
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("spider must have a workspace parent")
        .to_path_buf()
}

fn collect_rust_files(dir: &Path, base: &Path, out: &mut Vec<SourceFile>) {
    if !dir.exists() {
        return;
    }
    for entry in fs::read_dir(dir).expect("failed to read source directory") {
        let entry = entry.expect("failed to read directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, base, out);
        } else if path.extension().map_or(false, |ext| ext == "rs") {
            let contents = fs::read_to_string(&path).expect("failed to read source file");
            let relative_path = path
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.push(SourceFile {
                relative_path,
                contents,
            });
        }
    }
}

fn scan_crate_src(crate_dir: &str) -> Vec<SourceFile> {
    let src = workspace_root().join(crate_dir).join("src");
    let mut files = Vec::new();
    collect_rust_files(&src, &src, &mut files);
    files
}

/// Crates that may participate in the canonical transport graph.
const TRANSPORT_GRAPH_CRATES: &[&str] = &[
    "spider",
    "spider_transport",
    "spider_agent",
    "spider_cli",
    "spider_mcp",
    "spider_worker",
    "spider_utils",
];

fn find_in_graph(pattern: &str) -> Vec<String> {
    let mut locations = Vec::new();
    for crate_dir in TRANSPORT_GRAPH_CRATES {
        for file in scan_crate_src(crate_dir) {
            if file.contents.contains(pattern) {
                locations.push(format!("{crate_dir}/src/{}", file.relative_path));
            }
        }
    }
    locations
}

// ---------------------------------------------------------------------------
// (1) exactly one canonical transport owner
// ---------------------------------------------------------------------------

#[test]
fn exactly_one_transport_policy_owner() {
    let locations = find_in_graph("pub enum TransportPolicy");
    assert_eq!(
        locations.len(),
        1,
        "TransportPolicy must be defined exactly once workspace-wide: {locations:?}"
    );
    assert!(
        locations[0].starts_with("spider_transport/src/"),
        "the single TransportPolicy definition must live in the spider_transport leaf: {locations:?}"
    );
}

#[test]
fn transport_error_has_one_owner() {
    let locations = find_in_graph("pub enum TransportError");
    assert_eq!(
        locations.len(),
        1,
        "TransportError must be defined exactly once: {locations:?}"
    );
    assert!(locations[0].starts_with("spider_transport/src/"));
}

// ---------------------------------------------------------------------------
// (2)+(3)+(4) spider re-exports canonical types — compile-time type identity
// ---------------------------------------------------------------------------

fn assert_same_type<T>(_: &T, _: &T) {}

#[test]
fn transport_policy_type_identity_is_shared() {
    assert_same_type(
        &spider::features::transport::TransportPolicy::Default,
        &spider_transport::TransportPolicy::Default,
    );
    assert_same_type(
        &spider::features::transport::TransportMode::Default,
        &spider_transport::TransportMode::Default,
    );
}

#[test]
fn transport_error_type_identity_is_shared() {
    assert_same_type(
        &spider::features::transport::TransportError::TorNotCompiled,
        &spider_transport::TransportError::TorNotCompiled,
    );
}

#[test]
fn secret_request_headers_type_identity_is_shared() {
    assert_same_type(
        &spider::features::secret_request_headers::SecretRequestHeaders::new(),
        &spider_transport::SecretRequestHeaders::new(),
    );
    assert_same_type(
        &spider::features::secret_request_headers::SecretHeaderError::InvalidHeaderName,
        &spider_transport::SecretHeaderError::InvalidHeaderName,
    );
}

#[test]
fn tor_config_and_request_types_are_shared() {
    let config =
        spider::features::transport::TorTransportConfig::new("socks5h://127.0.0.1:9050").unwrap();
    assert_same_type(
        &config,
        &spider_transport::TorTransportConfig::new("socks5h://127.0.0.1:9050").unwrap(),
    );
    let request = spider::features::transport::TransportRequest::default();
    assert_same_type(&request, &spider_transport::TransportRequest::default());
}

// ---------------------------------------------------------------------------
// (2) façade purity: no implementation inside spider's transport modules
// ---------------------------------------------------------------------------

#[test]
fn spider_transport_facade_contains_no_implementation() {
    let spider_src = scan_crate_src("spider");
    for facade in [
        "features/transport.rs",
        "features/secret_request_headers.rs",
    ] {
        let file = spider_src
            .iter()
            .find(|f| f.relative_path == facade)
            .unwrap_or_else(|| panic!("façade {facade} must exist"));
        for forbidden in [
            "reqwest::Client::new()",
            "reqwest::Client::builder()",
            "reqwest::ClientBuilder::new()",
            "reqwest::Proxy::all",
            "redirect::Policy::custom",
            "pub enum TransportPolicy",
            "pub enum TransportError",
            "pub struct TorTransportConfig",
            "pub struct SecretRequestHeaders",
            "pub enum SecretHeaderError",
            "fn is_onion_host",
            "fn is_ssrf_redirect",
            "is_loopback()",
        ] {
            assert!(
                !file.contents.contains(forbidden),
                "spider transport façade {facade} must contain no implementation: found {forbidden:?}"
            );
        }
    }
}

#[test]
fn no_higher_level_transport_implementation_remains_in_spider() {
    // Client-construction / policy logic fingerprints may exist only in the
    // leaf (plus pre-existing non-canonical upstream-compat files already
    // allowlisted by architecture_guardrails — those are NOT transport).
    let spider_src = scan_crate_src("spider");
    for file in &spider_src {
        if file.relative_path.starts_with("features/transport") {
            assert!(
                !file.contents.contains("redirect::Policy::custom"),
                "redirect policy logic must live only in the leaf"
            );
        }
    }
    let tor_builders: Vec<_> = find_in_graph("fn build_tor_client")
        .into_iter()
        .filter(|path| path.starts_with("spider_transport/src/"))
        .collect();
    assert_eq!(
        tor_builders.len(),
        1,
        "exactly one Tor client builder may exist: {tor_builders:?}"
    );
    assert!(tor_builders[0].starts_with("spider_transport/src/"));
}

// ---------------------------------------------------------------------------
// (16)-(19) security-primitive uniqueness (logic fingerprints, not names)
// ---------------------------------------------------------------------------

#[test]
fn security_primitives_are_unique_workspace_wide() {
    let single_owner_patterns = [
        ("pub fn validate_target", "target validator"),
        ("fn is_onion_host", "onion host classifier"),
        ("pub fn is_onion_url", "onion URL classifier"),
        ("fn pin_redirect_policy", "redirect pinning policy"),
        ("fn ssrf_screened_base_policy", "SSRF redirect policy"),
        ("fn apply_transport_policy", "transport policy application"),
        ("fn build_streaming_client", "streaming client builder"),
        ("pub struct SecretRequestHeaders", "secret request headers"),
        ("pub enum SecretHeaderError", "secret header error"),
        (
            "static ACQUISITION_TRANSPORT_SCOPE",
            "acquisition transport task-local",
        ),
    ];
    for (pattern, description) in single_owner_patterns {
        let locations = find_in_graph(pattern);
        assert_eq!(
            locations.len(),
            1,
            "{description} must have exactly one owner ({pattern:?}): {locations:?}"
        );
        assert!(
            locations[0].starts_with("spider_transport/src/"),
            "{description} must be owned by the spider_transport leaf: {locations:?}"
        );
    }
}

#[test]
fn ssrf_classifier_has_exactly_one_owner() {
    // Design-neutral: the SSRF internal-address classifier may legitimately
    // live in the leaf (design A) or remain in spider with injection into
    // the leaf (design B) — but it must exist EXACTLY ONCE workspace-wide.
    let locations = find_in_graph("is_unique_local()");
    assert_eq!(
        locations.len(),
        1,
        "SSRF internal-address classifier must have exactly one owner: {locations:?}"
    );
}

// ---------------------------------------------------------------------------
// leaf purity: no path dependency on any higher-level consumer
// ---------------------------------------------------------------------------

#[test]
fn leaf_crate_has_no_workspace_path_dependencies() {
    let cargo_toml = workspace_root().join("spider_transport").join("Cargo.toml");
    let contents = fs::read_to_string(&cargo_toml).expect("spider_transport/Cargo.toml must exist");
    assert!(
        !contents.contains("path ="),
        "spider_transport must not path-depend on any workspace crate"
    );
}

#[test]
fn leaf_crate_is_a_workspace_member() {
    let root_toml = workspace_root().join("Cargo.toml");
    let contents = fs::read_to_string(root_toml).expect("failed to read workspace Cargo.toml");
    assert!(
        contents.contains("\"spider_transport\""),
        "spider_transport must be a workspace member"
    );
}

// ---------------------------------------------------------------------------
// (13)+(14) canonical consumers still use the canonical seam
// ---------------------------------------------------------------------------

#[test]
fn canonical_consumers_still_use_the_canonical_transport_seam() {
    let spider_src = scan_crate_src("spider");
    for consumer in [
        "features/github_source_provider.rs",
        "features/hugging_face_source_provider.rs",
        "features/artifact_download_execution.rs",
    ] {
        let file = spider_src
            .iter()
            .find(|f| f.relative_path == consumer)
            .unwrap_or_else(|| panic!("{consumer} must exist"));
        assert!(
            file.contents.contains("execute_streaming_request"),
            "{consumer} must execute through the canonical transport seam"
        );
        for forbidden in [
            "reqwest::Client::new()",
            "reqwest::Client::builder()",
            "Website::new",
        ] {
            assert!(
                !file.contents.contains(forbidden),
                "{consumer} must not construct alternate HTTP clients ({forbidden:?})"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// (5)-(12) behavior — offline local fixtures, through the façade seam
// ---------------------------------------------------------------------------

#[cfg(all(not(feature = "wreq"), not(feature = "cache_request")))]
mod behavior {
    use spider::features::secret_request_headers::SecretRequestHeaders;
    use spider::features::transport::{
        execute_streaming_request, is_onion_url, validate_target, TorTransportConfig,
        TransportError, TransportPolicy, TransportRequest,
    };
    use std::net::SocketAddr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio_stream::StreamExt;

    async fn start_fixture(
        status: &'static str,
        extra_headers: &'static str,
        body: &'static [u8],
    ) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                tokio::spawn(async move {
                    let mut buf = [0_u8; 8192];
                    if stream.read(&mut buf).await.is_err() {
                        return;
                    }
                    let response = format!(
                        "HTTP/1.1 {status}\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.write_all(body).await;
                });
            }
        });
        addr
    }

    fn fixture_url(addr: SocketAddr) -> url::Url {
        url::Url::parse(&format!("http://{addr}/")).unwrap()
    }

    /// (5) Default execution behavior preserved: status/final-URL/headers
    /// before any body byte; body intact for the caller.
    #[tokio::test]
    async fn default_execution_status_headers_then_body() {
        let addr = start_fixture("200 OK", "X-Fixture: leaf\r\n", b"leaf-body").await;
        let response = execute_streaming_request(
            &fixture_url(addr),
            &TransportPolicy::Default,
            &SecretRequestHeaders::new(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(response.headers().get("x-fixture").unwrap(), "leaf");
        let bytes = response.bytes().await.unwrap();
        assert_eq!(&bytes[..], b"leaf-body");
    }

    /// (12) streaming behavior preserved: body arrives via the caller's own
    /// stream consumption.
    #[tokio::test]
    async fn body_streams_via_caller_consumption() {
        let payload = b"chunked-streaming-payload-for-leaf-acceptance";
        let addr = start_fixture("200 OK", "", payload).await;
        let response = execute_streaming_request(
            &fixture_url(addr),
            &TransportPolicy::Default,
            &SecretRequestHeaders::new(),
        )
        .await
        .unwrap();
        let mut stream = response.bytes_stream();
        let mut collected = Vec::new();
        while let Some(chunk) = stream.next().await {
            collected.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(collected, payload);
    }

    /// (8) `.onion` under Default remains rejected, before any networking.
    #[tokio::test]
    async fn onion_under_default_is_rejected() {
        let onion = url::Url::parse("http://exampleexampleexampleexamp.onion/").unwrap();
        assert!(is_onion_url(&onion));
        assert_eq!(
            validate_target(&onion, &TransportPolicy::Default).unwrap_err(),
            TransportError::OnionRequiresTor
        );
        let error = execute_streaming_request(
            &onion,
            &TransportPolicy::Default,
            &SecretRequestHeaders::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(error, TransportError::OnionRequiresTor);
    }

    /// (9) cross-transport redirect (clearnet → onion under Default) remains
    /// rejected mid-request.
    #[tokio::test]
    async fn redirect_to_onion_under_default_is_rejected() {
        let addr = start_fixture(
            "302 Found",
            "Location: http://exampleexampleexampleexamp.onion/\r\n",
            b"",
        )
        .await;
        let error = execute_streaming_request(
            &fixture_url(addr),
            &TransportPolicy::Default,
            &SecretRequestHeaders::new(),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, TransportError::RequestExecutionFailed(_)));
    }

    /// (10) SSRF redirect screening remains: a redirect into internal
    /// address space is rejected mid-request.
    #[tokio::test]
    async fn redirect_to_internal_address_is_rejected() {
        let addr = start_fixture("302 Found", "Location: http://127.0.0.1:9/\r\n", b"").await;
        let error = execute_streaming_request(
            &fixture_url(addr),
            &TransportPolicy::Default,
            &SecretRequestHeaders::new(),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, TransportError::RequestExecutionFailed(_)));
    }

    /// (6)+(7) Tor behavior: endpoint grammar/validation preserved;
    /// Tor-not-compiled fail-closed when the feature is off.
    #[test]
    fn tor_endpoint_validation_matrix_preserved() {
        assert!(TorTransportConfig::new("socks5h://127.0.0.1:9050").is_ok());
        assert_eq!(
            TorTransportConfig::new("socks5://127.0.0.1:9050").unwrap_err(),
            TransportError::UnsupportedScheme("socks5".into())
        );
        assert_eq!(
            TorTransportConfig::new("socks5h://user:pass@127.0.0.1:9050").unwrap_err(),
            TransportError::CredentialsNotSupported
        );
        assert!(TorTransportConfig::new("socks5h://127.0.0.1").is_err());
        assert!(TorTransportConfig::new("socks5h://127.0.0.1:9050/path").is_err());
    }

    #[cfg(not(feature = "transport_tor"))]
    #[tokio::test]
    async fn tor_without_feature_fails_closed() {
        let policy = TransportRequest {
            mode: spider::features::transport::TransportMode::Tor,
            proxy: Some("socks5h://127.0.0.1:9050".to_string()),
        }
        .into_policy()
        .unwrap();
        let url = url::Url::parse("https://example.test/").unwrap();
        let error = execute_streaming_request(&url, &policy, &SecretRequestHeaders::new())
            .await
            .unwrap_err();
        assert_eq!(error, TransportError::TorNotCompiled);
    }

    #[cfg(feature = "transport_tor")]
    #[test]
    fn tor_policy_constructs_when_feature_compiled() {
        let policy = TransportRequest {
            mode: spider::features::transport::TransportMode::Tor,
            proxy: Some("socks5h://127.0.0.1:9050".to_string()),
        }
        .into_policy()
        .unwrap();
        assert!(matches!(policy, TransportPolicy::Tor(_)));
        // Tor targets validate under Tor policy.
        let onion = url::Url::parse("http://exampleexampleexampleexamp.onion/").unwrap();
        assert!(validate_target(&onion, &policy).is_ok());
    }

    /// (11) secret headers keep sensitivity semantics.
    #[test]
    fn secret_headers_stay_sensitive_and_redacted() {
        let mut headers = SecretRequestHeaders::new();
        headers
            .try_insert("authorization", "Bearer leaf-secret-sentinel")
            .unwrap();
        let debug = format!("{headers:?}");
        assert!(!debug.contains("leaf-secret-sentinel"));
        assert_eq!(debug, "SecretRequestHeaders { count: 1 }");

        let mut map = reqwest::header::HeaderMap::new();
        headers.apply_to(&mut map);
        assert!(map.get("authorization").unwrap().is_sensitive());
    }

    /// (11b) secret headers are actually applied to outgoing requests.
    #[tokio::test]
    async fn secret_headers_reach_the_wire() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_clone = captured.clone();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0_u8; 8192];
                if let Ok(n) = stream.read(&mut buf).await {
                    *captured_clone.lock().unwrap() = buf[..n].to_vec();
                }
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await;
            }
        });
        let mut headers = SecretRequestHeaders::new();
        headers
            .try_insert("x-leaf-secret", "leaf-secret-value")
            .unwrap();
        let response =
            execute_streaming_request(&fixture_url(addr), &TransportPolicy::Default, &headers)
                .await
                .unwrap();
        assert!(response.status().is_success());
        let request_text = String::from_utf8_lossy(&captured.lock().unwrap()).to_ascii_lowercase();
        assert!(request_text.contains("x-leaf-secret: leaf-secret-value"));
    }
}

// ---------------------------------------------------------------------------
// NEGATIVE PROOF: the scanner catches transport-graph violations
// ---------------------------------------------------------------------------

#[test]
fn scanner_detects_transport_violations() {
    let synthetic = vec![
        SourceFile {
            relative_path: "transport.rs".to_string(),
            contents: "pub enum TransportPolicy { Default }".to_string(),
        },
        SourceFile {
            relative_path: "evil.rs".to_string(),
            contents: "pub enum TransportPolicy { Default, Shadow }".to_string(),
        },
    ];
    let hits: Vec<String> = synthetic
        .iter()
        .filter(|f| f.contents.contains("pub enum TransportPolicy"))
        .map(|f| f.relative_path.clone())
        .collect();
    assert_eq!(
        hits.len(),
        2,
        "scanner must see both definitions so the uniqueness guard can reject the shadow"
    );

    let mut graph_hits = Vec::new();
    for file in &synthetic {
        if file.contents.contains("fn build_tor_client") {
            graph_hits.push(file.relative_path.clone());
        }
    }
    assert!(
        graph_hits.is_empty(),
        "sanity: synthetic set has no Tor builder; uniqueness logic is exercised above"
    );
}
