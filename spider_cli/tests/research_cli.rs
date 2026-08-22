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
            counts: ResearchSessionCounts {
                search_results: 1,
                acquisition_attempts: 1,
                durable_sources: 1,
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
