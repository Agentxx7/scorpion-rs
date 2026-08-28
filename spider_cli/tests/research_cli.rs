#![cfg(feature = "research")]

use std::process::Command;

fn scorpion() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_scorpion"));
    for name in [
        "RESEARCH_EVIDENCE_DB",
        "SEARXNG_BASE_URL",
        "OPENAI_COMPAT_BASE_URL",
        "OPENAI_COMPAT_MODEL",
        "OPENAI_COMPAT_API_KEY",
    ] {
        command.env_remove(name);
    }
    command
}

fn temporary_database() -> std::path::PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "scorpion-research-cli-test-{}-{nonce}.sqlite",
        std::process::id(),
    ))
}

fn remove_database(path: &std::path::Path) {
    for candidate in [
        path.to_path_buf(),
        std::path::PathBuf::from(format!("{}-shm", path.display())),
        std::path::PathBuf::from(format!("{}-wal", path.display())),
    ] {
        let _ = std::fs::remove_file(candidate);
    }
}

#[test]
fn run_fails_closed_on_each_missing_configuration_before_networking() {
    let cases = [
        (Vec::<(&str, &str)>::new(), "RESEARCH_EVIDENCE_DB"),
        (
            vec![("RESEARCH_EVIDENCE_DB", "/tmp/research.sqlite")],
            "SEARXNG_BASE_URL",
        ),
        (
            vec![
                ("RESEARCH_EVIDENCE_DB", "/tmp/research.sqlite"),
                ("SEARXNG_BASE_URL", "https://search.invalid"),
            ],
            "OPENAI_COMPAT_BASE_URL",
        ),
        (
            vec![
                ("RESEARCH_EVIDENCE_DB", "/tmp/research.sqlite"),
                ("SEARXNG_BASE_URL", "https://search.invalid"),
                ("OPENAI_COMPAT_BASE_URL", "https://model.invalid/v1"),
            ],
            "OPENAI_COMPAT_MODEL",
        ),
        (
            vec![
                ("RESEARCH_EVIDENCE_DB", "/tmp/research.sqlite"),
                ("SEARXNG_BASE_URL", "https://search.invalid"),
                ("OPENAI_COMPAT_BASE_URL", "https://model.invalid/v1"),
                ("OPENAI_COMPAT_MODEL", "model"),
            ],
            "OPENAI_COMPAT_API_KEY",
        ),
    ];

    for (environment, expected) in cases {
        let mut command = scorpion();
        command.args(["research", "topic"]);
        for (name, value) in environment {
            command.env(name, value);
        }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains(expected));
    }
}

#[test]
fn malformed_and_unknown_research_ids_are_nonzero_and_truthful() {
    let path = temporary_database();
    remove_database(&path);

    let malformed = scorpion()
        .args([
            "research",
            "show",
            "not-a-research-id",
            "--database",
            path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!malformed.status.success());
    assert!(malformed.stdout.is_empty());
    assert!(String::from_utf8_lossy(&malformed.stderr).contains("invalid ResearchId"));
    assert!(
        !path.exists(),
        "malformed IDs must fail before opening storage"
    );

    let id = "research_00112233445566778899aabbccddeeff";
    let unknown = scorpion()
        .args(["research", "show", id, "--database", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!unknown.status.success());
    assert!(unknown.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&unknown.stderr);
    assert!(stderr.contains(&format!("Research session not found: {id}")));
    assert!(!stderr.contains("OPENAI"));
    assert!(!stderr.contains("SEARXNG"));

    remove_database(&path);
}

#[test]
fn api_key_has_no_cli_argument_and_show_needs_no_provider_configuration() {
    let rejected = scorpion()
        .args(["research", "topic", "--api-key", "secret"])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(!combined.contains("secret"));
    assert!(combined.contains("unexpected argument '--api-key'"));
}

#[test]
fn show_reopens_and_formats_canonical_result_without_provider_config_or_secret_payloads() {
    use spider::features::domain_persistence::DomainPersistence;
    use spider::features::identity::{EvidenceId, ResearchId};
    use spider::features::research_session::{
        DurableResearchCitation, DurableResearchExtraction, DurableResearchResult,
        DurableResearchSynthesis, DurableResearchTokenUsage, ResearchSession,
        ResearchSessionCounts, ResearchSessionState, ResearchSourceBinding,
    };
    use spider::spider_agent::{FinishReason, ResearchExtraction, ResearchExtractionFact};
    use spider::utils::evidence::{record_evidence, EvidenceBundle, EvidenceRef};

    let path = temporary_database();
    remove_database(&path);
    let research_id = ResearchId::new();
    let evidence_id = EvidenceId::new();
    let evidence_ref = EvidenceRef::new(evidence_id);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let store = DomainPersistence::open(&path).await.unwrap();
        let mut evidence = EvidenceBundle::default();
        evidence.id = Some(evidence_id);
        evidence.requested_url = Some("https://user:secret@example.test/?token=secret".to_string());
        evidence.final_url = Some("https://example.test/final?cookie=secret".to_string());
        evidence.content = Some("SECRET EVIDENCE BODY".to_string());
        record_evidence(&store, evidence).await.unwrap();
        let extraction = DurableResearchExtraction {
            source_number: 1,
            evidence: evidence_ref,
            extracted: ResearchExtraction {
                facts: vec![ResearchExtractionFact {
                    topic: "Runtime".to_string(),
                    finding: "Grounded durable finding".to_string(),
                }],
                missing_evidence: vec!["Source-local gap".to_string()],
            },
            extraction_input_bytes: 999,
            finish_reason: Some(FinishReason::Stop),
        };
        let session = ResearchSession {
            id: research_id,
            topic: "topic".to_string(),
            extraction_instructions: None,
            sources: Vec::new(),
            source_bindings: vec![ResearchSourceBinding {
                source_number: 1,
                evidence: evidence_ref,
            }],
            extraction_diagnostics: Vec::new(),
            synthesis_diagnostic: None,
            counts: ResearchSessionCounts {
                search_results: 1,
                acquisition_attempts: 1,
                durable_sources: 1,
                observed_acquisitions: 1,
                successful_extractions: 1,
            },
            state: ResearchSessionState::CompletedSuccessfully,
            result: Some(DurableResearchResult {
                extractions: vec![extraction],
                synthesis: Some(DurableResearchSynthesis {
                    summary: "Durable summary [Source 1]".to_string(),
                    citations: vec![DurableResearchCitation {
                        source_number: 1,
                        evidence: evidence_ref,
                    }],
                    usage: DurableResearchTokenUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                        total_tokens: 15,
                    },
                }),
            }),
            created_at_unix_ms: 1,
            completed_at_unix_ms: Some(2),
        };
        let payload = serde_json::to_vec(&session).unwrap();
        store
            .write_current(&research_id.to_string(), None, &payload)
            .await
            .unwrap();
    });
    drop(runtime);

    let output = scorpion()
        .args([
            "research",
            "show",
            &research_id.to_string(),
            "--database",
            path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.status);
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(&format!("Research ID: {research_id}")));
    assert!(stdout.contains("State: CompletedSuccessfully"));
    assert!(stdout.contains("Durable summary [Source 1]"));
    assert!(stdout.contains(&format!("[1] Evidence: {evidence_id}")));
    assert!(stdout.contains("Source 1:\n- Source-local gap"));
    assert!(!stdout.contains("SECRET"));
    assert!(!stdout.contains("https://"));
    assert!(!stdout.contains("token="));
    assert!(!stdout.contains("cookie="));

    remove_database(&path);
}

/// A guaranteed-refused loopback endpoint: bound, then immediately dropped,
/// so the port is free but nothing is listening.
fn refused_addr() -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr
}

/// Serves exactly one SearXNG-shaped `/search` JSON response (matching the
/// same real HTTP contract `search_cli.rs`'s own fake server proves), then
/// stops. `result_urls` become the candidate URLs the real research
/// acquisition loop attempts next.
fn fake_searxng(result_urls: Vec<String>) -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        let results: Vec<serde_json::Value> = result_urls
            .iter()
            .enumerate()
            .map(|(index, url)| {
                serde_json::json!({ "title": format!("Result {}", index + 1), "url": url })
            })
            .collect();
        let body = serde_json::to_string(&serde_json::json!({ "results": results })).unwrap();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    (address, handle)
}

/// SCORPION_CLI_RESEARCH_TOTAL_ACQUISITION_FAILURE_EXIT_SEMANTICS_001.
/// Real end-to-end process-level proof: the actual `scorpion` binary,
/// driven through its actual `research` command, against a real (local,
/// deterministic) SearXNG-shaped provider whose candidates are all
/// genuinely refused loopback endpoints — no mocked state, no synthetic
/// string matching. The OpenAI-compatible endpoint is never dialed (every
/// acquisition fails before extraction is ever attempted), so a
/// syntactically-valid but unreachable placeholder is sufficient.
#[test]
fn subprocess_total_acquisition_failure_exits_nonzero_with_truthful_state() {
    let path = temporary_database();
    remove_database(&path);

    let refused_one = refused_addr();
    let refused_two = refused_addr();
    let (search_addr, search_handle) = fake_searxng(vec![
        format!("http://{refused_one}/a"),
        format!("http://{refused_two}/b"),
    ]);

    let output = scorpion()
        .args(["research", "total acquisition failure topic"])
        .env("RESEARCH_EVIDENCE_DB", path.to_str().unwrap())
        .env("SEARXNG_BASE_URL", format!("http://{search_addr}"))
        .env("OPENAI_COMPAT_BASE_URL", "http://127.0.0.1:1/v1")
        .env("OPENAI_COMPAT_MODEL", "unused-model")
        .env("OPENAI_COMPAT_API_KEY", "unused-key")
        .output()
        .unwrap();

    search_handle.join().unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "total acquisition failure must not exit 0: stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains("State: CompletedNoObservedAcquisitions"),
        "{stdout}"
    );
    assert!(stdout.contains("No observed acquisitions"), "{stdout}");
    assert!(stderr.contains("total acquisition failure"), "{stderr}");

    remove_database(&path);
}

/// Required positive control for the same frontier: a real, genuinely
/// observed acquisition (a real local HTTP response, not a refusal) whose
/// content the research pipeline legitimately never turns into an
/// extraction must still exit 0 — proving the fix does not convert every
/// no-extraction run into a failure.
#[test]
fn subprocess_acquired_but_unextracted_still_exits_zero() {
    use std::io::{Read, Write};
    let path = temporary_database();
    remove_database(&path);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let fixture_addr = listener.local_addr().unwrap();
    let fixture_handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        let body: &[u8] = b"not extractable content";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
    });

    let (search_addr, search_handle) =
        fake_searxng(vec![format!("http://{fixture_addr}/unsupported")]);

    let output = scorpion()
        .args(["research", "acquired but unextracted topic"])
        .env("RESEARCH_EVIDENCE_DB", path.to_str().unwrap())
        .env("SEARXNG_BASE_URL", format!("http://{search_addr}"))
        .env("OPENAI_COMPAT_BASE_URL", "http://127.0.0.1:1/v1")
        .env("OPENAI_COMPAT_MODEL", "unused-model")
        .env("OPENAI_COMPAT_API_KEY", "unused-key")
        .output()
        .unwrap();

    search_handle.join().unwrap();
    fixture_handle.join().unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "a real observed acquisition with no usable extraction must still exit 0: \
         stdout={stdout} stderr={stderr}"
    );
    assert!(stdout.contains("State: CompletedNoExtractions"), "{stdout}");
    assert!(stdout.contains("No supported extractions"), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");

    remove_database(&path);
}
