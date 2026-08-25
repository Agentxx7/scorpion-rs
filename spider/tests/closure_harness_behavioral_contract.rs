//! Independent behavioral contract for `closure_harness.rs`.
//!
//! This is a *different test binary*, invoking the *real* `closure_harness`
//! binary as a real subprocess against deliberately-invalid, known-bad
//! ledger fixtures — not calling any function inside `closure_harness.rs`
//! directly, and not comparing hashes. If someone guts `closure_harness.rs`
//! to unconditional success, these tests fail, because the real subprocess
//! they spawn would then exit 0 against fixtures that must be rejected.
//! `closure_harness_integrity.rs`'s SHA-256 check is explicitly NOT this —
//! it only detects accidental drift between the harness and its own
//! doc-comment description of itself; it provides no behavioral guarantee
//! and must not be described as if it does.
//!
//! Run with:
//! `cargo test -p spider --test closure_harness_behavioral_contract --features "chrome cache cache_request"`

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

/// A real, reproducible race this session found empirically: several
/// fixtures write a throwaway source file directly into `spider/src/`
/// (`ScratchFile`) so their subprocess can exercise `strict = true`
/// AST/cfg logic against a file that structurally looks like part of the
/// `spider` crate. `cargo test` runs this binary's own `#[test]`
/// functions concurrently by default, and each one independently spawns
/// its own `cargo test ... --test closure_harness` subprocess. When one
/// test's scratch file *exists on disk* while a *different* test's
/// subprocess is doing its own build/dependency check against the same
/// `spider` crate and shared `target/` directory, that subprocess
/// occasionally (observed, not hypothetical — a real flake was caught and
/// reproduced during this round) behaves as if a rebuild raced with a
/// stale intermediate state, causing an unrelated test's normally-passing
/// fixture to spuriously fail.
///
/// Every `#[test]` function below acquires this lock exactly once, first
/// thing (via `locked_test`), for its *entire* body — write the scratch
/// file (if any), spawn the subprocess, assert, let the scratch file drop
/// — not merely around the subprocess spawn. This fully serializes the
/// suite (trading this binary's own parallel runtime for eliminating the
/// race entirely) and is deliberately *not* also acquired inside
/// `run_real_verifier_check`: `Mutex` is not reentrant, and a scratch-file
/// test's own thread would deadlock against itself if both the test body
/// and the subprocess helper it calls tried to acquire the same lock.
static SUBPROCESS_SERIALIZATION_LOCK: Mutex<()> = Mutex::new(());

/// Acquires `SUBPROCESS_SERIALIZATION_LOCK` for the duration of `body`.
/// `.lock()` on a poisoned mutex (an earlier test panicked mid-hold)
/// still yields the guard via `unwrap_or_else` — one test's assertion
/// failure must not cascade into every subsequent test being unable to
/// acquire the lock at all.
fn locked_test(body: impl FnOnce()) {
    let _guard = SUBPROCESS_SERIALIZATION_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    body();
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("spider crate must be inside workspace")
        .to_path_buf()
}

/// Builds a temporary ledger directory: the real `LIVE_NETWORK_TESTS.toml`
/// (so that unrelated check doesn't spuriously fail and mask the fixture
/// under test) plus exactly one fixture file, `id`/filename matched.
fn temp_ledger_with_fixture(id: &str, fixture_toml: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let real_registry =
        fs::read_to_string(workspace_root().join("docs/frontier/ledger/LIVE_NETWORK_TESTS.toml"))
            .expect("failed to read the real LIVE_NETWORK_TESTS.toml");
    fs::write(dir.path().join("LIVE_NETWORK_TESTS.toml"), real_registry)
        .expect("failed to write registry copy");
    fs::write(dir.path().join(format!("{id}.toml")), fixture_toml)
        .expect("failed to write fixture");
    dir
}

/// A real, on-disk source file the real verifier subprocess will read via
/// `workspace_root()` (unaffected by `CLOSURE_HARNESS_LEDGER_DIR_OVERRIDE`,
/// which only redirects the *ledger* directory). Used to construct precise,
/// self-contained AST fixtures — a WIRED chain rooted entirely inside one
/// throwaway file — without touching real production code. Not referenced
/// by any `mod` declaration anywhere, so it is inert to the real crate
/// build; removed via `Drop` (including on panic/unwind) so a failing
/// assertion can never leave it behind. Each caller must use a unique
/// `relative_path` — tests run concurrently, and two tests sharing a path
/// would race.
struct ScratchFile {
    path: PathBuf,
}

impl ScratchFile {
    fn write(relative_path: &str, contents: &str) -> Self {
        let path = workspace_root().join(relative_path);
        fs::write(&path, contents)
            .unwrap_or_else(|error| panic!("failed to write scratch file {path:?}: {error}"));
        ScratchFile { path }
    }
}

impl Drop for ScratchFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Spawns the real `closure_harness` test binary as a real subprocess
/// against `ledger_dir`, returning whether it exited successfully.
fn run_real_verifier_against(ledger_dir: &Path) -> bool {
    run_real_verifier_against_with_workflows(ledger_dir, None)
}

/// As above, optionally also overriding the workflows directory the
/// real verifier scans for CI_ENFORCED evidence — lets a fixture exercise
/// workflow-shaped rules (a schedule-only trigger, a gated step) without
/// touching the real `.github/workflows/`.
fn run_real_verifier_against_with_workflows(
    ledger_dir: &Path,
    workflows_dir: Option<&Path>,
) -> bool {
    run_real_verifier_check(ledger_dir, workflows_dir, None)
}

/// Spawns the real `closure_harness` binary as a real subprocess, exactly
/// as above, but — when `only_test` is `Some(name)` — filters the
/// subprocess to run *only* that one `#[test]` function via `--exact`.
/// This is the mechanism that makes causal mutation-proof possible: with
/// the whole binary running, an unrelated internal self-test
/// (`structural_parser_rejects_known_adversarial_fixtures`, which shares
/// several of the same low-level AST/TOML helpers as the ledger-driven
/// checks) can independently fail first and mask which check actually
/// caused the subprocess to reject a given fixture. Filtering to exactly
/// the one rule under test structurally excludes every other test,
/// including that self-test, from ever running at all — so a failure can
/// only mean the named rule specifically accepted (or rejected) the
/// fixture.
fn run_real_verifier_check(
    ledger_dir: &Path,
    workflows_dir: Option<&Path>,
    only_test: Option<&str>,
) -> bool {
    let mut command = Command::new(env!("CARGO"));
    command
        .current_dir(workspace_root())
        .arg("test")
        .arg("-p")
        .arg("spider")
        .arg("--test")
        .arg("closure_harness")
        .arg("--features")
        .arg("chrome cache cache_request");
    if let Some(name) = only_test {
        command.arg(name).arg("--").arg("--exact");
    }
    command
        .env(
            "CLOSURE_HARNESS_LEDGER_DIR_OVERRIDE",
            ledger_dir.as_os_str(),
        )
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Some(workflows_dir) = workflows_dir {
        command.env(
            "CLOSURE_HARNESS_WORKFLOWS_DIR_OVERRIDE",
            workflows_dir.as_os_str(),
        );
    }
    // Deliberately does *not* acquire `SUBPROCESS_SERIALIZATION_LOCK`
    // itself — every calling `#[test]` function acquires it once, first
    // thing, for its whole body (see `locked_test`/the lock's own doc
    // comment). `Mutex` is not reentrant; acquiring it again here would
    // deadlock a scratch-file test against itself.
    command
        .status()
        .expect("failed to spawn cargo test subprocess")
        .success()
}

/// Runs the real subprocess filtered to exactly one inner `#[test]`
/// function via `--exact` (see `run_real_verifier_check`), additionally
/// capturing output and asserting exactly one test actually ran — guarding
/// against a typo'd/renamed `only_test` silently matching zero tests
/// (which `cargo test` reports as a trivial pass, indistinguishable from
/// "correctly rejected" if only the exit code were checked). Panics if the
/// filter didn't match exactly one test.
///
/// This is the actual isolation mechanism for every fixture below that
/// claims to test one specific verifier rule (Codex adversarial review:
/// "do not claim exact one-rule isolation unless the permanent tests
/// actually use the strict single-rule mechanism" — an earlier version of
/// several fixtures called `run_real_verifier_against`, which runs the
/// *entire* `closure_harness` binary; that still correctly rejects each
/// invalid fixture, but does not, by itself, prove *which* rule did the
/// rejecting, and a future regression in an unrelated rule could mask a
/// silent regression in the one actually under test). `--exact` filtering
/// structurally excludes every other `#[test]` — including
/// `structural_parser_rejects_known_adversarial_fixtures`, an internal
/// self-test sharing several of the same low-level AST/TOML helpers —
/// from ever running at all, so a failure here can only mean the named
/// rule specifically accepted (or rejected) the fixture.
fn run_single_verifier_check_strict(
    ledger_dir: &Path,
    workflows_dir: Option<&Path>,
    only_test: &str,
) -> bool {
    let mut command = Command::new(env!("CARGO"));
    command
        .current_dir(workspace_root())
        .arg("test")
        .arg("-p")
        .arg("spider")
        .arg("--test")
        .arg("closure_harness")
        .arg("--features")
        .arg("chrome cache cache_request")
        .arg(only_test)
        .arg("--")
        .arg("--exact")
        .env(
            "CLOSURE_HARNESS_LEDGER_DIR_OVERRIDE",
            ledger_dir.as_os_str(),
        )
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    if let Some(workflows_dir) = workflows_dir {
        command.env(
            "CLOSURE_HARNESS_WORKFLOWS_DIR_OVERRIDE",
            workflows_dir.as_os_str(),
        );
    }
    let output = command
        .output()
        .expect("failed to spawn cargo test subprocess");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let ran_exactly_one = stdout.contains("1 passed") || stdout.contains("1 failed");
    assert!(
        ran_exactly_one,
        "expected the --exact filter {only_test:?} to match exactly one test, but it didn't \
         (0 tests matched would trivially \"pass\" and falsely look like rejection). Subprocess \
         stdout:\n{stdout}"
    );
    output.status.success()
}

/// A scratch `.github/workflows/`-shaped directory containing exactly one
/// workflow file with the given content.
fn temp_workflows_dir(yaml: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    fs::write(dir.path().join("rust.yml"), yaml).expect("failed to write scratch workflow");
    dir
}

/// Two scratch workflow files in the same `.github/workflows/`-shaped
/// directory — `rust.yml` and `other.yml` — for CI workflow provenance
/// fixtures: a matching command may exist in one file but not the other,
/// and the verifier must only credit the file the ledger actually names.
fn temp_workflows_dir_two_files(rust_yml: &str, other_yml: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    fs::write(dir.path().join("rust.yml"), rust_yml).expect("failed to write scratch workflow");
    fs::write(dir.path().join("other.yml"), other_yml).expect("failed to write scratch workflow");
    dir
}

const VALID_BASE: &str = r#"
id = "SCORPION_BEHAVIORAL_FIXTURE_001"
sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
summary = "Behavioral contract fixture."
stage = "WIRED"
required_proof_classes = ["CODE_PROVEN"]

[proof.CODE_PROVEN]
capability_id = "SCORPION_BEHAVIORAL_FIXTURE_001"
commit = "13cbc2dfcc410fa49843b304e45b62102e5012e4"
evidence = ["fixture code fact"]

[stages.DESIGNED]
sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

[stages.IMPLEMENTED]
evidence = ["spider/src/website.rs:crawl_establish"]

[stages.VERIFIED]
test_only = true
evidence = ["spider/tests/architecture_guardrails.rs:no_shadow_credential_aware_cache_policy_in_cli_or_mcp"]
last_verified_command = "cargo test -p spider --lib --test architecture_guardrails --features \"chrome cache cache_request\""
last_verified_result = "1/1"

[stages.WIRED]
"#;

/// A ledger claiming `CLOSED` with a blob SHA as `closed_commit` must be
/// rejected by the real verifier. If `closure_harness.rs` were gutted to
/// unconditional success, this subprocess would exit 0 and this test
/// would fail.
#[test]
fn behavioral_verifier_rejects_blob_sha_as_closed_commit() {
    locked_test(|| {
        let blob_sha = String::from_utf8(
            Command::new("git")
                .current_dir(workspace_root())
                .args(["rev-parse", "HEAD:spider/Cargo.toml"])
                .output()
                .expect("git rev-parse failed")
                .stdout,
        )
        .expect("non-utf8 git output")
        .trim()
        .to_string();
        assert_eq!(
            String::from_utf8(
                Command::new("git")
                    .current_dir(workspace_root())
                    .args(["cat-file", "-t", &blob_sha])
                    .output()
                    .unwrap()
                    .stdout
            )
            .unwrap()
            .trim(),
            "blob",
            "test setup assumption broken: expected a blob SHA"
        );

        let fixture = format!(
            r#"{VALID_BASE}callers = ["spider/src/website.rs:Website::crawl -> spider/src/website.rs:crawl_concurrent -> spider/src/website.rs:crawl_establish"]

    [stages.PRODUCTION_REACHABLE]
    reachability_kind = "binary_default"
    shipping_artifacts = ["spider_cli"]
    feature_requirements = ["sitemap"]
    entry_point_symbols = ["Website::crawl"]
    siblings_enumerated = true
    siblings = []
    siblings_note = "fixture"
    verdict = "MET"
    verdict_evidence = "fixture"

    [stages.CI_ENFORCED]
    ci_workflow_file = ".github/workflows/rust.yml"
    package = "spider"
    lib = false
    test_targets = ["architecture_guardrails", "closure_harness", "closure_harness_integrity", "closure_harness_behavioral_contract"]
    feature_set = "chrome cache cache_request"
    positional_filters = []
    exact = false
    skip = []

    [stages.CLOSED]
    closed_commit = "{blob_sha}"
    "#
        );
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", &fixture);
        assert!(
            !run_single_verifier_check_strict(
                dir.path(),
                None,
                "closed_stage_commit_is_a_real_commit_reachable_from_history"
            ),
            "the real verifier accepted a blob SHA as closed_commit — behavioral contract violated"
        );
    });
}

/// A ledger claiming `PRODUCTION_REACHABLE` via a generic, WIRED-unbound
/// entry-point symbol must be rejected.
#[test]
fn behavioral_verifier_rejects_generic_unbound_entry_point() {
    locked_test(|| {
        let fixture = format!(
            r#"{VALID_BASE}callers = ["spider/src/website.rs:Website::crawl -> spider/src/website.rs:crawl_concurrent -> spider/src/website.rs:crawl_establish"]

    [stages.PRODUCTION_REACHABLE]
    reachability_kind = "binary_default"
    shipping_artifacts = ["spider_cli"]
    feature_requirements = ["sitemap"]
    entry_point_symbols = [".get("]
    siblings_enumerated = true
    siblings = []
    siblings_note = "fixture"
    verdict = "MET"
    verdict_evidence = "fixture"
    "#
        );
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", &fixture);
        assert!(
            !run_single_verifier_check_strict(dir.path(), None, "production_reachable_entry_points_are_bound_to_this_capabilitys_own_wired_roots"),
            "the real verifier accepted a generic, WIRED-unbound entry point — behavioral contract violated"
        );
    });
}

/// A WIRED chain with real symbols but no proven call adjacency must be
/// rejected.
#[test]
fn behavioral_verifier_rejects_adjacency_free_wired_chain() {
    locked_test(|| {
        let fixture = format!(
            r#"{VALID_BASE}callers = ["spider/src/website.rs:Website::crawl -> spider/src/website.rs:crawl_establish_smart"]
    "#
        );
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", &fixture);
        assert!(
            !run_single_verifier_check_strict(
                dir.path(),
                None,
                "wired_stage_chains_prove_real_call_adjacency_rooted_in_production_source"
            ),
            "the real verifier accepted an adjacency-free WIRED chain — behavioral contract violated"
        );
    });
}

/// A genuinely valid fixture (mirroring the mutation-tested one from the
/// prior review round) must still PASS — proving these tests reject bad
/// states specifically, not everything indiscriminately. Deliberately
/// whole-verifier (`run_real_verifier_against`, not
/// `run_single_verifier_check_strict`): a positive control has no single
/// rule to isolate to — it asserts every rule simultaneously accepts a
/// genuinely valid case, which is the point of running the whole binary.
#[test]
fn behavioral_verifier_accepts_a_genuinely_valid_fixture() {
    locked_test(|| {
        let fixture = format!(
            r#"{VALID_BASE}callers = ["spider/src/website.rs:Website::crawl -> spider/src/website.rs:crawl_concurrent -> spider/src/website.rs:crawl_establish"]
    "#
        );
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", &fixture);
        assert!(
            run_real_verifier_against(dir.path()),
            "the real verifier rejected a genuinely valid WIRED-only fixture — false positive"
        );
    });
}

const VALID_WIRED_CHAIN: &str = "spider/src/website.rs:Website::crawl -> spider/src/website.rs:crawl_concurrent -> spider/src/website.rs:crawl_establish";

const VALID_PRODUCTION_REACHABLE: &str = r#"
[stages.PRODUCTION_REACHABLE]
reachability_kind = "binary_default"
shipping_artifacts = ["spider_cli"]
feature_requirements = ["sitemap"]
entry_point_symbols = ["Website::crawl"]
siblings_enumerated = true
siblings = []
siblings_note = "fixture"
verdict = "MET"
verdict_evidence = "fixture"
"#;

/// CLOSED present without any prerequisite stage (PRODUCTION_REACHABLE,
/// ADVERSARIALLY_VERIFIED, CI_ENFORCED all absent) must be rejected.
#[test]
fn behavioral_verifier_rejects_closed_without_prerequisite_stages() {
    locked_test(|| {
        let fixture = format!(
            r#"{VALID_BASE}callers = ["{VALID_WIRED_CHAIN}"]

    [stages.CLOSED]
    closed_commit = "725c2b76e3a6bdbad9b153218953bdb3e88a659d"
    "#
        );
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", &fixture);
        assert!(
            !run_single_verifier_check_strict(dir.path(), None, "later_stages_are_withheld_when_production_reachable_is_not_met"),
            "the real verifier accepted CLOSED with no prerequisite stages — behavioral contract violated"
        );
    });
}

/// IMPLEMENTED evidence citing a symbol that only exists inside a
/// `#[cfg(test)]` module must be rejected — a test-only definition is
/// not a production implementation.
#[test]
fn behavioral_verifier_rejects_test_only_implemented_evidence() {
    locked_test(|| {
        let fixture = VALID_BASE.replace(
            r#"evidence = ["spider/src/website.rs:crawl_establish"]"#,
            r#"evidence = ["spider/src/website.rs:configure_setup_fails_closed_under_tor_policy"]"#,
        );
        let fixture = format!("{fixture}callers = [\"{VALID_WIRED_CHAIN}\"]\n");
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", &fixture);
        assert!(
            !run_single_verifier_check_strict(dir.path(), None, "implemented_stage_evidence_references_real_definitions_not_comments"),
            "the real verifier accepted a test-only symbol as IMPLEMENTED evidence — behavioral contract violated"
        );
    });
}

/// PRODUCTION_REACHABLE claiming MET with a feature that the named
/// shipping artifact's real Cargo.toml does not actually enable must be
/// rejected.
#[test]
fn behavioral_verifier_rejects_fabricated_feature_production_reachable() {
    locked_test(|| {
        let fixture = format!(
            r#"{VALID_BASE}callers = ["{VALID_WIRED_CHAIN}"]

    [stages.PRODUCTION_REACHABLE]
    reachability_kind = "binary_default"
    shipping_artifacts = ["spider_cli"]
    feature_requirements = ["this_feature_does_not_exist_anywhere"]
    entry_point_symbols = ["Website::crawl"]
    siblings_enumerated = true
    siblings = []
    siblings_note = "fixture"
    verdict = "MET"
    verdict_evidence = "fabricated"
    "#
        );
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", &fixture);
        assert!(
            !run_single_verifier_check_strict(dir.path(), None, "production_reachable_claims_are_grep_verified_against_shipping_manifests"),
            "the real verifier accepted a fabricated, unenabled feature as PRODUCTION_REACHABLE proof — behavioral contract violated"
        );
    });
}

/// A `run:` step that isn't a direct `cargo test` invocation
/// (shell-wrapped, non-executing) must never satisfy a declared,
/// otherwise-matching structural CI_ENFORCED command — even when the
/// declared fields describe exactly what the wrapped command *claims* to
/// run.
#[test]
fn behavioral_verifier_rejects_non_executing_ci_evidence() {
    locked_test(|| {
        let fixture = format!(
            r#"{VALID_BASE}callers = ["{VALID_WIRED_CHAIN}"]
    {VALID_PRODUCTION_REACHABLE}
    [stages.CI_ENFORCED]
    ci_workflow_file = ".github/workflows/rust.yml"
    package = "spider"
    lib = true
    test_targets = []
    feature_set = ""
    positional_filters = []
    exact = false
    skip = []
    "#
        );
        let workflow = r#"
    on:
      push:
        branches: [main]
    jobs:
      echo_job:
        runs-on: ubuntu-latest
        steps:
          - name: echo-wrapped, non-executing step
            run: echo cargo test -p spider --lib
    "#;
        let ledger_dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", &fixture);
        let workflows_dir = temp_workflows_dir(workflow);
        assert!(
            !run_single_verifier_check_strict(ledger_dir.path(), Some(workflows_dir.path()), "ci_enforced_commands_are_real_required_non_gated_steps_with_exact_feature_match"),
            "the real verifier accepted a non-executing (echo-wrapped) run: step as CI_ENFORCED evidence — behavioral contract violated"
        );
    });
}

/// CI_ENFORCED.ci_command matching a real step, but one gated behind
/// `if:`, must be rejected.
#[test]
fn behavioral_verifier_rejects_gated_ci_evidence() {
    locked_test(|| {
        let fixture = format!(
            r#"{VALID_BASE}callers = ["{VALID_WIRED_CHAIN}"]
    {VALID_PRODUCTION_REACHABLE}
    [stages.CI_ENFORCED]
    ci_workflow_file = ".github/workflows/rust.yml"
    package = "spider"
    lib = true
    test_targets = []
    feature_set = ""
    positional_filters = []
    exact = false
    skip = []
    "#
        );
        let workflow = r#"
    on:
      push:
        branches: [main]
    jobs:
      gated_job:
        runs-on: ubuntu-latest
        steps:
          - name: gated step
            if: env.RUN_LIVE_TESTS == '1'
            run: cargo test -p spider --lib
    "#;
        let ledger_dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", &fixture);
        let workflows_dir = temp_workflows_dir(workflow);
        assert!(
            !run_single_verifier_check_strict(ledger_dir.path(), Some(workflows_dir.path()), "ci_enforced_commands_are_real_required_non_gated_steps_with_exact_feature_match"),
            "the real verifier accepted a gated (if:-conditional) step as CI_ENFORCED evidence — behavioral contract violated"
        );
    });
}

/// CI_ENFORCED.ci_command matching a real step in a workflow triggered
/// only by `schedule` (never `push`/`pull_request`) must be rejected —
/// such a step is never guaranteed to run on the revision being claimed.
#[test]
fn behavioral_verifier_rejects_schedule_only_ci_evidence() {
    locked_test(|| {
        let fixture = format!(
            r#"{VALID_BASE}callers = ["{VALID_WIRED_CHAIN}"]
    {VALID_PRODUCTION_REACHABLE}
    [stages.CI_ENFORCED]
    ci_workflow_file = ".github/workflows/rust.yml"
    package = "spider"
    lib = true
    test_targets = []
    feature_set = ""
    positional_filters = []
    exact = false
    skip = []
    "#
        );
        let workflow = r#"
    on:
      schedule:
        - cron: "0 0 * * *"
    jobs:
      scheduled_job:
        runs-on: ubuntu-latest
        steps:
          - name: scheduled step
            run: cargo test -p spider --lib
    "#;
        let ledger_dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", &fixture);
        let workflows_dir = temp_workflows_dir(workflow);
        assert!(
            !run_single_verifier_check_strict(ledger_dir.path(), Some(workflows_dir.path()), "ci_enforced_commands_are_real_required_non_gated_steps_with_exact_feature_match"),
            "the real verifier accepted a schedule-only-triggered step as CI_ENFORCED evidence — behavioral contract violated"
        );
    });
}

/// `CLOSED.closed_commit` naming a real, history-reachable ancestor that
/// nonetheless does not contain the exact ledger/harness bytes currently
/// being verified must be rejected — a historical ancestor is not the
/// same as "the revision that actually closed this".
#[test]
fn behavioral_verifier_rejects_unrelated_historical_ancestor_as_closed_commit() {
    locked_test(|| {
        let fixture = format!(
            r#"{VALID_BASE}callers = ["{VALID_WIRED_CHAIN}"]
    {VALID_PRODUCTION_REACHABLE}
    [stages.CI_ENFORCED]
    ci_workflow_file = ".github/workflows/rust.yml"
    package = "spider"
    lib = false
    test_targets = ["architecture_guardrails", "closure_harness", "closure_harness_integrity", "closure_harness_behavioral_contract"]
    feature_set = "chrome cache cache_request"
    positional_filters = []
    exact = false
    skip = []

    [stages.CLOSED]
    closed_commit = "4bb5af0d415493856133135fa8f5661e8b2058e3"
    "#
        );
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", &fixture);
        assert!(
            !run_single_verifier_check_strict(dir.path(), None, "closed_stage_commit_is_a_real_commit_reachable_from_history"),
            "the real verifier accepted an unrelated historical ancestor commit (real, reachable, but never contained this fixture) as closed_commit — behavioral contract violated"
        );
    });
}

/// `ADVERSARIALLY_VERIFIED.reviewed_commit` naming a real, history-
/// reachable ancestor that nonetheless does not contain the exact
/// ledger/harness/evidence bytes currently being verified must be
/// rejected — the same standard `CLOSED.closed_commit` is held to, not a
/// weaker one. No `CI_ENFORCED`/`CLOSED` table is present here, so this
/// isolates `ADVERSARIALLY_VERIFIED`'s own revision-binding rule
/// specifically.
#[test]
fn behavioral_verifier_rejects_stale_ancestor_as_adversarially_verified_reviewed_commit() {
    locked_test(|| {
        let fixture = format!(
            r#"{VALID_BASE}callers = ["{VALID_WIRED_CHAIN}"]
    {VALID_PRODUCTION_REACHABLE}
    [stages.ADVERSARIALLY_VERIFIED]
    capability_id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    reviewed_commit = "4bb5af0d415493856133135fa8f5661e8b2058e3"
    bypass_attempts = ["fixture"]
    "#
        );
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", &fixture);
        assert!(
            !run_single_verifier_check_strict(dir.path(), None, "adversarially_verified_binds_a_real_reviewed_commit_to_this_capability"),
            "the real verifier accepted an unrelated historical ancestor commit as ADVERSARIALLY_VERIFIED.reviewed_commit — behavioral contract violated"
        );
    });
}

/// CI_ENFORCED.ci_command that selects the right binary/target but then
/// explicitly `--skip`s the exact test VERIFIED cites must be rejected.
#[test]
fn behavioral_verifier_rejects_test_selection_that_excludes_verified_evidence() {
    locked_test(|| {
        let fixture = format!(
            r#"{VALID_BASE}callers = ["{VALID_WIRED_CHAIN}"]
    {VALID_PRODUCTION_REACHABLE}
    [stages.CI_ENFORCED]
    ci_workflow_file = ".github/workflows/rust.yml"
    package = "spider"
    lib = false
    test_targets = ["architecture_guardrails"]
    feature_set = ""
    positional_filters = []
    exact = false
    skip = ["no_shadow_credential_aware_cache_policy_in_cli_or_mcp"]
    "#
        );
        let workflow = r#"
    on:
      push:
        branches: [main]
    jobs:
      fixture_job:
        runs-on: ubuntu-latest
        steps:
          - name: fixture step
            run: cargo test -p spider --test architecture_guardrails -- --skip no_shadow_credential_aware_cache_policy_in_cli_or_mcp
    "#;
        let ledger_dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", &fixture);
        let workflows_dir = temp_workflows_dir(workflow);
        assert!(
            !run_single_verifier_check_strict(ledger_dir.path(), Some(workflows_dir.path()), "ci_enforced_commands_are_real_required_non_gated_steps_with_exact_feature_match"),
            "the real verifier accepted a CI command that selects the right binary but --skips the exact VERIFIED test — behavioral contract violated"
        );
    });
}

/// CI_ENFORCED that selects the right binary/target but declares a
/// non-`--exact` positional filter that does not match the exact
/// VERIFIED test's bare name must be rejected — cargo's own substring
/// filter semantics mean that test simply never runs, even though no
/// `--skip`/`--exact` flag is involved at all.
#[test]
fn behavioral_verifier_rejects_non_matching_positional_filter_that_excludes_verified_evidence() {
    locked_test(|| {
        let fixture = format!(
            r#"{VALID_BASE}callers = ["{VALID_WIRED_CHAIN}"]
    {VALID_PRODUCTION_REACHABLE}
    [stages.CI_ENFORCED]
    ci_workflow_file = ".github/workflows/rust.yml"
    package = "spider"
    lib = false
    test_targets = ["architecture_guardrails"]
    feature_set = ""
    positional_filters = ["unrelated_filter_text"]
    exact = false
    skip = []
    "#
        );
        let workflow = r#"
    on:
      push:
        branches: [main]
    jobs:
      fixture_job:
        runs-on: ubuntu-latest
        steps:
          - name: fixture step
            run: cargo test -p spider --test architecture_guardrails unrelated_filter_text
    "#;
        let ledger_dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", &fixture);
        let workflows_dir = temp_workflows_dir(workflow);
        assert!(
            !run_single_verifier_check_strict(ledger_dir.path(), Some(workflows_dir.path()), "ci_enforced_commands_are_real_required_non_gated_steps_with_exact_feature_match"),
            "the real verifier accepted a CI command whose non-exact positional filter does not match the cited VERIFIED test — behavioral contract violated"
        );
    });
}

/// CI workflow provenance (Codex adversarial review, exact demonstrated
/// gap): a structurally matching, non-gated, applicable, executable
/// command existing ONLY in a workflow file other than the ledger's own
/// declared `ci_workflow_file` must not satisfy CI_ENFORCED. Scanning
/// every `.yml` file under `.github/workflows/` and discarding which one
/// actually supplied the match would credit CI enforcement this
/// repository's real CI configuration never actually runs.
#[test]
fn behavioral_verifier_rejects_ci_evidence_matching_only_in_a_different_workflow_file() {
    locked_test(|| {
        let fixture = format!(
            r#"{VALID_BASE}callers = ["{VALID_WIRED_CHAIN}"]
    {VALID_PRODUCTION_REACHABLE}
    [stages.CI_ENFORCED]
    ci_workflow_file = ".github/workflows/rust.yml"
    package = "spider"
    lib = false
    test_targets = ["architecture_guardrails"]
    feature_set = ""
    positional_filters = []
    exact = false
    skip = []
    "#
        );
        // The exact matching command lives only in `other.yml`; the
        // declared `ci_workflow_file` (`rust.yml`) has no matching step
        // at all.
        let rust_yml = r#"
    on:
      push:
        branches: [main]
    jobs:
      fixture_job:
        runs-on: ubuntu-latest
        steps:
          - name: unrelated step
            run: cargo test -p spider --lib
    "#;
        let other_yml = r#"
    on:
      push:
        branches: [main]
    jobs:
      fixture_job:
        runs-on: ubuntu-latest
        steps:
          - name: fixture step
            run: cargo test -p spider --test architecture_guardrails
    "#;
        let ledger_dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", &fixture);
        let workflows_dir = temp_workflows_dir_two_files(rust_yml, other_yml);
        assert!(
            !run_single_verifier_check_strict(ledger_dir.path(), Some(workflows_dir.path()), "ci_enforced_commands_are_real_required_non_gated_steps_with_exact_feature_match"),
            "the real verifier accepted CI_ENFORCED evidence whose matching command lives only in a workflow file other than the declared ci_workflow_file — behavioral contract violated"
        );
    });
}

/// Positive control confirming the fix above is narrowly scoped: the
/// identical matching command, when it genuinely lives in the declared
/// `ci_workflow_file` (alongside an unrelated same-shaped decoy in
/// `other.yml`, which must be ignored), is still accepted.
#[test]
fn behavioral_verifier_accepts_ci_evidence_matching_in_the_declared_workflow_file() {
    locked_test(|| {
        let fixture = format!(
            r#"{VALID_BASE}callers = ["{VALID_WIRED_CHAIN}"]
    {VALID_PRODUCTION_REACHABLE}
    [stages.CI_ENFORCED]
    ci_workflow_file = ".github/workflows/rust.yml"
    package = "spider"
    lib = false
    test_targets = ["architecture_guardrails"]
    feature_set = ""
    positional_filters = []
    exact = false
    skip = []
    "#
        );
        let rust_yml = r#"
    on:
      push:
        branches: [main]
    jobs:
      fixture_job:
        runs-on: ubuntu-latest
        steps:
          - name: fixture step
            run: cargo test -p spider --test architecture_guardrails
    "#;
        let other_yml = r#"
    on:
      push:
        branches: [main]
    jobs:
      fixture_job:
        runs-on: ubuntu-latest
        steps:
          - name: decoy step
            run: cargo test -p spider --lib
    "#;
        let ledger_dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", &fixture);
        let workflows_dir = temp_workflows_dir_two_files(rust_yml, other_yml);
        assert!(
            run_single_verifier_check_strict(ledger_dir.path(), Some(workflows_dir.path()), "ci_enforced_commands_are_real_required_non_gated_steps_with_exact_feature_match"),
            "the real verifier rejected CI_ENFORCED evidence whose matching command genuinely lives in the declared ci_workflow_file — behavioral contract violated"
        );
    });
}

// =====================================================================
// The seven fixtures below close the remaining causal-isolation gap
// (Codex adversarial review): "complete the same causal mutation
// standard already proven for the other rules" for full path:symbol
// WIRED terminal identity, Type::method impl ownership, cfg-active
// coherent WIRED adjacency, test-only intermediate WIRED caller
// exclusion, module-qualified VERIFIED identity, canonical Website
// production ownership, and test-only production consumer exclusion.
// Each was independently mutation-proofed by hand against exactly this
// fixture (mutate the named rule only -> this exact fixture flips to
// accepted -> revert -> rejected again) before being committed here as a
// permanent regression test. Several of these rules are enforced
// redundantly by two independent functions (a defense-in-depth choice
// made earlier in this frontier); the doc comment on each fixture notes
// this where it applies.
// =====================================================================

/// Full `path:symbol` WIRED terminal identity (not bare-name matching): a
/// WIRED chain terminating at a real, non-comment `crawl_establish` in
/// `spider/src/website.rs` must not bind to IMPLEMENTED evidence claiming
/// `crawl_establish` lives in a *different* file
/// (`spider/src/page.rs`, where no such symbol exists) merely because the
/// trailing bare name matches.
#[test]
fn behavioral_verifier_rejects_wired_terminal_bare_name_collision_across_files() {
    locked_test(|| {
        let fixture = r#"
    id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
    summary = "Behavioral contract fixture."
    stage = "WIRED"

    [stages.DESIGNED]
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

    [stages.IMPLEMENTED]
    evidence = ["spider/src/page.rs:crawl_establish"]

    [stages.VERIFIED]
    test_only = true
    evidence = ["spider/tests/architecture_guardrails.rs:no_shadow_credential_aware_cache_policy_in_cli_or_mcp"]
    last_verified_command = "cargo test -p spider --lib --test architecture_guardrails --features \"chrome cache cache_request\""
    last_verified_result = "1/1"

    [stages.WIRED]
    callers = ["spider/src/website.rs:Website::crawl -> spider/src/website.rs:crawl_concurrent -> spider/src/website.rs:crawl_establish"]
    "#;
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", fixture);
        assert!(
            !run_single_verifier_check_strict(dir.path(), None, "wired_stage_chains_prove_real_call_adjacency_rooted_in_production_source"),
            "the real verifier accepted a WIRED terminal binding to IMPLEMENTED evidence in an unrelated file via bare-name collision — behavioral contract violated"
        );
    });
}

/// `Type::method` impl ownership: a WIRED chain root claiming
/// `Website::crawl` must not be satisfied by an unrelated type's
/// same-named method, even when that method is the only `crawl` defined
/// anywhere in the claimed file.
#[test]
fn behavioral_verifier_rejects_unrelated_type_impl_ownership_for_wired_root() {
    locked_test(|| {
        let _scratch = ScratchFile::write(
            "spider/src/_mp_bc_rule2.rs",
            r#"pub struct DecoyType;
    impl DecoyType {
        pub async fn crawl(&mut self) {
            self.rule2_next();
        }
        fn rule2_next(&mut self) {}
    }
    "#,
        );
        let fixture = r#"
    id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
    summary = "Behavioral contract fixture."
    stage = "WIRED"

    [stages.DESIGNED]
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

    [stages.IMPLEMENTED]
    evidence = ["spider/src/_mp_bc_rule2.rs:rule2_next"]

    [stages.VERIFIED]
    test_only = true
    evidence = ["spider/tests/architecture_guardrails.rs:no_shadow_credential_aware_cache_policy_in_cli_or_mcp"]
    last_verified_command = "cargo test -p spider --lib --test architecture_guardrails --features \"chrome cache cache_request\""
    last_verified_result = "1/1"

    [stages.WIRED]
    callers = ["spider/src/_mp_bc_rule2.rs:Website::crawl -> spider/src/_mp_bc_rule2.rs:rule2_next"]
    "#;
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", fixture);
        assert!(
            !run_single_verifier_check_strict(dir.path(), None, "wired_stage_chains_prove_real_call_adjacency_rooted_in_production_source"),
            "the real verifier accepted an unrelated type's same-named method as Website::crawl — behavioral contract violated"
        );
    });
}

/// Cfg-active coherent WIRED adjacency: when a hop symbol has multiple
/// `#[cfg(...)]`-gated overloads, only the one active under this
/// capability's declared feature set may supply the call evidence — an
/// inactive overload's body (which happens to call the claimed callee)
/// must not count.
#[test]
fn behavioral_verifier_rejects_inactive_cfg_overload_as_adjacency_proof() {
    locked_test(|| {
        let _scratch = ScratchFile::write(
            "spider/src/_mp_bc_rule3.rs",
            r#"pub struct Website;
    impl Website {
        pub async fn crawl(&mut self) {
            self.rule3_hop();
        }
        #[cfg(feature = "rule3_never_declared_anywhere")]
        fn rule3_hop(&mut self) {
            self.rule3_target();
        }
        fn rule3_hop(&mut self) {}
        fn rule3_target(&mut self) {}
    }
    "#,
        );
        let fixture = r#"
    id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
    summary = "Behavioral contract fixture."
    stage = "WIRED"

    [stages.DESIGNED]
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

    [stages.IMPLEMENTED]
    evidence = ["spider/src/_mp_bc_rule3.rs:rule3_target"]

    [stages.VERIFIED]
    test_only = true
    evidence = ["spider/tests/architecture_guardrails.rs:no_shadow_credential_aware_cache_policy_in_cli_or_mcp"]
    last_verified_command = "cargo test -p spider --lib --test architecture_guardrails --features \"chrome cache cache_request\""
    last_verified_result = "1/1"

    [stages.WIRED]
    callers = ["spider/src/_mp_bc_rule3.rs:Website::crawl -> spider/src/_mp_bc_rule3.rs:rule3_hop -> spider/src/_mp_bc_rule3.rs:rule3_target"]
    "#;
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", fixture);
        assert!(
            !run_single_verifier_check_strict(dir.path(), None, "wired_stage_chains_prove_real_call_adjacency_rooted_in_production_source"),
            "the real verifier accepted an inactive cfg-gated overload's body as WIRED adjacency proof — behavioral contract violated"
        );
    });
}

/// Test-only intermediate WIRED caller exclusion: a hop whose only
/// definition lives inside `#[cfg(test)] mod tests` must not satisfy
/// WIRED adjacency, even though it is a real, non-comment definition that
/// genuinely calls the next hop.
#[test]
fn behavioral_verifier_rejects_test_only_intermediate_wired_caller() {
    locked_test(|| {
        let _scratch = ScratchFile::write(
            "spider/src/_mp_bc_rule4.rs",
            r#"pub struct Website;
    impl Website {
        pub async fn crawl(&mut self) {
            self.rule4_hop();
        }
        fn rule4_target(&mut self) {}
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        impl Website {
            fn rule4_hop(&mut self) {
                self.rule4_target();
            }
        }
    }
    "#,
        );
        let fixture = r#"
    id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
    summary = "Behavioral contract fixture."
    stage = "WIRED"

    [stages.DESIGNED]
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

    [stages.IMPLEMENTED]
    evidence = ["spider/src/_mp_bc_rule4.rs:rule4_target"]

    [stages.VERIFIED]
    test_only = true
    evidence = ["spider/tests/architecture_guardrails.rs:no_shadow_credential_aware_cache_policy_in_cli_or_mcp"]
    last_verified_command = "cargo test -p spider --lib --test architecture_guardrails --features \"chrome cache cache_request\""
    last_verified_result = "1/1"

    [stages.WIRED]
    callers = ["spider/src/_mp_bc_rule4.rs:Website::crawl -> spider/src/_mp_bc_rule4.rs:rule4_hop -> spider/src/_mp_bc_rule4.rs:rule4_target"]
    "#;
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", fixture);
        assert!(
            !run_single_verifier_check_strict(dir.path(), None, "wired_stage_chains_prove_real_call_adjacency_rooted_in_production_source"),
            "the real verifier accepted a #[cfg(test)]-only intermediate caller as WIRED adjacency proof — behavioral contract violated"
        );
    });
}

/// Module-qualified VERIFIED identity: evidence citing
/// `module_a::tests::collision_name` must not be satisfied by an
/// unrelated `collision_name` defined under `module_b::tests` in the same
/// file — the full module chain, not just the trailing bare test name,
/// must resolve.
#[test]
fn behavioral_verifier_rejects_verified_module_path_bare_name_collision() {
    locked_test(|| {
        let _scratch = ScratchFile::write(
            "spider/src/_mp_bc_rule5.rs",
            r#"mod module_b {
        mod tests {
            #[test]
            fn collision_name() {}
        }
    }
    "#,
        );
        let fixture = r#"
    id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
    summary = "Behavioral contract fixture."
    stage = "WIRED"

    [stages.DESIGNED]
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

    [stages.IMPLEMENTED]
    evidence = ["spider/src/website.rs:crawl_establish"]

    [stages.VERIFIED]
    test_only = true
    evidence = ["spider/src/_mp_bc_rule5.rs::module_a::tests::collision_name"]
    last_verified_command = "cargo test -p spider --lib --features \"chrome cache cache_request\""
    last_verified_result = "1/1"

    [stages.WIRED]
    callers = ["spider/src/website.rs:Website::crawl -> spider/src/website.rs:crawl_concurrent -> spider/src/website.rs:crawl_establish"]
    "#;
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", fixture);
        assert!(
            !run_single_verifier_check_strict(dir.path(), None, "verified_stage_evidence_resolves_to_real_test_definitions"),
            "the real verifier accepted a VERIFIED evidence entry naming the wrong module path via bare-test-name collision — behavioral contract violated"
        );
    });
}

/// Canonical `Website` production ownership: a decoy struct also named
/// `Website`, calling a method also named `crawl`, sitting in a real
/// shipping artifact's (`spider_worker`) own `src/` tree must not be
/// credited as that artifact genuinely reaching the WIRED-bound
/// `Website::crawl` entry point.
#[test]
fn behavioral_verifier_rejects_unrelated_website_struct_as_production_reachability() {
    locked_test(|| {
        let _scratch = ScratchFile::write(
            "spider_worker/src/_mp_bc_rule6.rs",
            r#"struct Website;
    impl Website {
        fn crawl(&mut self) {}
    }
    fn mutation_proof_rule6_decoy_caller() {
        let mut w = Website;
        w.crawl();
    }
    "#,
        );
        let fixture = r#"
    id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
    summary = "Behavioral contract fixture."
    stage = "WIRED"

    [stages.DESIGNED]
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

    [stages.IMPLEMENTED]
    evidence = ["spider/src/website.rs:crawl_establish"]

    [stages.VERIFIED]
    test_only = true
    evidence = ["spider/tests/architecture_guardrails.rs:no_shadow_credential_aware_cache_policy_in_cli_or_mcp"]
    last_verified_command = "cargo test -p spider --lib --test architecture_guardrails --features \"chrome cache cache_request\""
    last_verified_result = "1/1"

    [stages.WIRED]
    callers = ["spider/src/website.rs:Website::crawl -> spider/src/website.rs:crawl_concurrent -> spider/src/website.rs:crawl_establish"]

    [stages.PRODUCTION_REACHABLE]
    reachability_kind = "binary_default"
    shipping_artifacts = ["spider_worker"]
    feature_requirements = ["cache"]
    entry_point_symbols = ["Website::crawl"]
    siblings_enumerated = true
    siblings = []
    siblings_note = "fixture"
    verdict = "MET"
    verdict_evidence = "fixture"
    "#;
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", fixture);
        assert!(
            !run_single_verifier_check_strict(dir.path(), None, "production_reachable_claims_are_grep_verified_against_shipping_manifests"),
            "the real verifier credited an unrelated decoy Website struct in spider_worker's own src/ as production reachability — behavioral contract violated"
        );
    });
}

/// Test-only production consumer exclusion: a `#[test]`-attributed
/// function inside a real shipping artifact's (`spider_worker`) own
/// `src/` tree, genuinely constructing and calling the canonical
/// `Website::crawl`, must not be credited as that artifact's *shipped*
/// code reaching the entry point — `cargo test`-only code is never part
/// of the shipped binary.
#[test]
fn behavioral_verifier_rejects_test_only_consumer_as_production_reachability() {
    locked_test(|| {
        let _scratch = ScratchFile::write(
            "spider_worker/src/_mp_bc_rule7.rs",
            r#"struct Website;
    impl Website {
        fn new(_url: &str) -> Self {
            Website
        }
        fn crawl(&mut self) {}
    }
    #[test]
    fn mutation_proof_rule7_test_only_consumer() {
        let mut w = Website::new("https://example.com");
        w.crawl();
    }
    "#,
        );
        let fixture = r#"
    id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
    summary = "Behavioral contract fixture."
    stage = "WIRED"

    [stages.DESIGNED]
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

    [stages.IMPLEMENTED]
    evidence = ["spider/src/website.rs:crawl_establish"]

    [stages.VERIFIED]
    test_only = true
    evidence = ["spider/tests/architecture_guardrails.rs:no_shadow_credential_aware_cache_policy_in_cli_or_mcp"]
    last_verified_command = "cargo test -p spider --lib --test architecture_guardrails --features \"chrome cache cache_request\""
    last_verified_result = "1/1"

    [stages.WIRED]
    callers = ["spider/src/website.rs:Website::crawl -> spider/src/website.rs:crawl_concurrent -> spider/src/website.rs:crawl_establish"]

    [stages.PRODUCTION_REACHABLE]
    reachability_kind = "binary_default"
    shipping_artifacts = ["spider_worker"]
    feature_requirements = ["cache"]
    entry_point_symbols = ["Website::crawl"]
    siblings_enumerated = true
    siblings = []
    siblings_note = "fixture"
    verdict = "MET"
    verdict_evidence = "fixture"
    "#;
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", fixture);
        assert!(
            !run_single_verifier_check_strict(
                dir.path(),
                None,
                "production_reachable_claims_are_grep_verified_against_shipping_manifests"
            ),
            "the real verifier credited a #[test]-only consumer in spider_worker's own src/ as production reachability — behavioral contract violated"
        );
    });
}

/// Canonical `Website` provenance, exact demonstrated bypass (Codex
/// adversarial review): a `Website` struct nested inside an *unrelated
/// module* — never top-level — combined with an inline, fully-qualified
/// construction and type annotation that never goes through a `use`
/// import at all, sitting in a real shipping artifact's (`spider_worker`)
/// own `src/` tree, must not be credited as that artifact genuinely
/// reaching the WIRED-bound `Website::crawl` entry point.
#[test]
fn behavioral_verifier_rejects_nested_module_qualified_website_as_production_reachability() {
    locked_test(|| {
        let _scratch = ScratchFile::write(
            "spider_worker/src/_mp_bc_rule_website_provenance.rs",
            r#"mod unrelated {
        pub struct Website;
        impl Website {
            pub fn new() -> Self {
                Website
            }
            pub fn crawl(&self) {}
        }
    }
    fn mutation_proof_rule_website_provenance_decoy_caller() {
        let site: unrelated::Website = unrelated::Website::new();
        site.crawl();
    }
    "#,
        );
        let fixture = r#"
    id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
    summary = "Behavioral contract fixture."
    stage = "WIRED"

    [stages.DESIGNED]
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

    [stages.IMPLEMENTED]
    evidence = ["spider/src/website.rs:crawl_establish"]

    [stages.VERIFIED]
    test_only = true
    evidence = ["spider/tests/architecture_guardrails.rs:no_shadow_credential_aware_cache_policy_in_cli_or_mcp"]
    last_verified_command = "cargo test -p spider --lib --test architecture_guardrails --features \"chrome cache cache_request\""
    last_verified_result = "1/1"

    [stages.WIRED]
    callers = ["spider/src/website.rs:Website::crawl -> spider/src/website.rs:crawl_concurrent -> spider/src/website.rs:crawl_establish"]

    [stages.PRODUCTION_REACHABLE]
    reachability_kind = "binary_default"
    shipping_artifacts = ["spider_worker"]
    feature_requirements = ["cache"]
    entry_point_symbols = ["Website::crawl"]
    siblings_enumerated = true
    siblings = []
    siblings_note = "fixture"
    verdict = "MET"
    verdict_evidence = "fixture"
    "#;
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", fixture);
        assert!(
            !run_single_verifier_check_strict(dir.path(), None, "production_reachable_claims_are_grep_verified_against_shipping_manifests"),
            "the real verifier credited a nested-module, inline-qualified unrelated Website (never `use`-imported) as production reachability — behavioral contract violated"
        );
    });
}

/// Shared impl-ownership identity, round 3 (Codex adversarial review,
/// exact demonstrated reproducer): a WIRED root/adjacency check must not
/// be satisfiable by a decoy `Website` nested in a crate-local module and
/// referenced only through a qualified `impl` self_ty
/// (`impl decoy::Website { ... }`) — before the shared identity fix, this
/// self_ty's *final* path segment alone ("Website") was indistinguishable
/// from a genuine bare `impl Website`.
#[test]
fn behavioral_verifier_rejects_qualified_decoy_impl_self_ty_as_wired_root() {
    locked_test(|| {
        let _scratch = ScratchFile::write(
            "spider/src/_mp_bc_rule_impl_identity.rs",
            r#"mod decoy {
        pub struct Website;
    }
    impl decoy::Website {
        pub fn crawl(&mut self) {
            self.mutation_proof_rule_impl_identity_target();
        }
        fn mutation_proof_rule_impl_identity_target(&mut self) {}
    }
    "#,
        );
        let fixture = r#"
    id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
    summary = "Behavioral contract fixture."
    stage = "WIRED"

    [stages.DESIGNED]
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

    [stages.IMPLEMENTED]
    evidence = ["spider/src/_mp_bc_rule_impl_identity.rs:mutation_proof_rule_impl_identity_target"]

    [stages.VERIFIED]
    test_only = true
    evidence = ["spider/tests/architecture_guardrails.rs:no_shadow_credential_aware_cache_policy_in_cli_or_mcp"]
    last_verified_command = "cargo test -p spider --lib --test architecture_guardrails --features \"chrome cache cache_request\""
    last_verified_result = "1/1"

    [stages.WIRED]
    callers = ["spider/src/_mp_bc_rule_impl_identity.rs:Website::crawl -> spider/src/_mp_bc_rule_impl_identity.rs:mutation_proof_rule_impl_identity_target"]
    "#;
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", fixture);
        assert!(
            !run_single_verifier_check_strict(dir.path(), None, "wired_stage_chains_prove_real_call_adjacency_rooted_in_production_source"),
            "the real verifier accepted a decoy Website nested in a crate-local module, referenced only via a qualified impl self_ty, as a genuine WIRED root/adjacency — behavioral contract violated"
        );
    });
}

/// PRODUCTION_REACHABLE self-receiver path, round 3 (Codex adversarial
/// review, exact demonstrated reproducer): a fully-qualified `impl`
/// self_ty referencing an unrelated `Website` — no local shadow struct,
/// no `use` import anywhere in the file, so the file-level trust gate
/// alone would have passed it — must not let `self.crawl()` establish
/// reachability in a real shipping artifact's own `src/` tree.
#[test]
fn behavioral_verifier_rejects_qualified_decoy_impl_self_receiver_as_production_reachability() {
    locked_test(|| {
        let _scratch = ScratchFile::write(
            "spider_worker/src/_mp_bc_rule_impl_identity2.rs",
            r#"impl crate::decoy::Website {
        pub fn mutation_proof_rule_impl_identity2_decoy_caller(&mut self) {
            self.crawl();
        }
        pub fn crawl(&mut self) {}
    }
    "#,
        );
        let fixture = r#"
    id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
    summary = "Behavioral contract fixture."
    stage = "WIRED"

    [stages.DESIGNED]
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

    [stages.IMPLEMENTED]
    evidence = ["spider/src/website.rs:crawl_establish"]

    [stages.VERIFIED]
    test_only = true
    evidence = ["spider/tests/architecture_guardrails.rs:no_shadow_credential_aware_cache_policy_in_cli_or_mcp"]
    last_verified_command = "cargo test -p spider --lib --test architecture_guardrails --features \"chrome cache cache_request\""
    last_verified_result = "1/1"

    [stages.WIRED]
    callers = ["spider/src/website.rs:Website::crawl -> spider/src/website.rs:crawl_concurrent -> spider/src/website.rs:crawl_establish"]

    [stages.PRODUCTION_REACHABLE]
    reachability_kind = "binary_default"
    shipping_artifacts = ["spider_worker"]
    feature_requirements = ["cache"]
    entry_point_symbols = ["Website::crawl"]
    siblings_enumerated = true
    siblings = []
    siblings_note = "fixture"
    verdict = "MET"
    verdict_evidence = "fixture"
    "#;
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", fixture);
        assert!(
            !run_single_verifier_check_strict(dir.path(), None, "production_reachable_claims_are_grep_verified_against_shipping_manifests"),
            "the real verifier credited self.crawl() inside a qualified decoy impl (crate::decoy::Website, no local shadow, no use import) as production reachability — behavioral contract violated"
        );
    });
}

/// Canonical identity, round 5 (Codex adversarial review, exact
/// demonstrated reproducer A): a bare, same-file `struct Website; impl
/// Website { ... }` — no canonical import, not the real definition site
/// — must not prove WIRED. Same identifier is never the same canonical
/// symbol; only affirmative provenance (the real definition site, or an
/// exact canonical import/qualified path) does.
#[test]
fn behavioral_verifier_rejects_bare_local_struct_and_impl_website_as_wired_root() {
    locked_test(|| {
        let _scratch = ScratchFile::write(
            "spider/src/_mp_bc_rule_canonical_identity_a.rs",
            r#"struct Website;
    impl Website {
        fn crawl(&mut self) {
            self.mutation_proof_rule_canonical_identity_a_target();
        }
        fn mutation_proof_rule_canonical_identity_a_target(&mut self) {}
    }
    "#,
        );
        let fixture = r#"
    id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
    summary = "Behavioral contract fixture."
    stage = "WIRED"

    [stages.DESIGNED]
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

    [stages.IMPLEMENTED]
    evidence = ["spider/src/_mp_bc_rule_canonical_identity_a.rs:mutation_proof_rule_canonical_identity_a_target"]

    [stages.VERIFIED]
    test_only = true
    evidence = ["spider/tests/architecture_guardrails.rs:no_shadow_credential_aware_cache_policy_in_cli_or_mcp"]
    last_verified_command = "cargo test -p spider --lib --test architecture_guardrails --features \"chrome cache cache_request\""
    last_verified_result = "1/1"

    [stages.WIRED]
    callers = ["spider/src/_mp_bc_rule_canonical_identity_a.rs:Website::crawl -> spider/src/_mp_bc_rule_canonical_identity_a.rs:mutation_proof_rule_canonical_identity_a_target"]
    "#;
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", fixture);
        assert!(
            !run_single_verifier_check_strict(dir.path(), None, "wired_stage_chains_prove_real_call_adjacency_rooted_in_production_source"),
            "the real verifier accepted a bare same-file struct Website + impl Website (no canonical import, not the definition site) as a genuine WIRED root — behavioral contract violated"
        );
    });
}

/// Canonical identity, round 5 (Codex adversarial review, exact
/// demonstrated reproducer B): a crate-local (but not canonical)
/// imported decoy — `use crate::decoy::Website;` — must not prove WIRED.
/// Crate-local is not the same thing as canonical.
#[test]
fn behavioral_verifier_rejects_crate_local_imported_decoy_website_as_wired_root() {
    locked_test(|| {
        let _scratch = ScratchFile::write(
            "spider/src/_mp_bc_rule_canonical_identity_b.rs",
            r#"use crate::decoy::Website;
    impl Website {
        fn crawl(&mut self) {
            self.mutation_proof_rule_canonical_identity_b_target();
        }
        fn mutation_proof_rule_canonical_identity_b_target(&mut self) {}
    }
    "#,
        );
        let fixture = r#"
    id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
    summary = "Behavioral contract fixture."
    stage = "WIRED"

    [stages.DESIGNED]
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

    [stages.IMPLEMENTED]
    evidence = ["spider/src/_mp_bc_rule_canonical_identity_b.rs:mutation_proof_rule_canonical_identity_b_target"]

    [stages.VERIFIED]
    test_only = true
    evidence = ["spider/tests/architecture_guardrails.rs:no_shadow_credential_aware_cache_policy_in_cli_or_mcp"]
    last_verified_command = "cargo test -p spider --lib --test architecture_guardrails --features \"chrome cache cache_request\""
    last_verified_result = "1/1"

    [stages.WIRED]
    callers = ["spider/src/_mp_bc_rule_canonical_identity_b.rs:Website::crawl -> spider/src/_mp_bc_rule_canonical_identity_b.rs:mutation_proof_rule_canonical_identity_b_target"]
    "#;
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", fixture);
        assert!(
            !run_single_verifier_check_strict(dir.path(), None, "wired_stage_chains_prove_real_call_adjacency_rooted_in_production_source"),
            "the real verifier accepted a crate-local (but non-canonical) imported decoy Website as a genuine WIRED root — behavioral contract violated"
        );
    });
}

/// Canonical identity, round 5 (Codex adversarial review, exact
/// demonstrated reproducer D): `mod Website { pub fn crawl() {} }` — a
/// *module*, not the canonical struct at all — followed by
/// `Website::crawl()` must not prove reachability merely because the
/// raw call text matches; the callee path's type prefix is
/// independently resolved through the shared canonical-identity model.
#[test]
fn behavioral_verifier_rejects_module_masquerading_as_type_as_production_reachability() {
    locked_test(|| {
        let _scratch = ScratchFile::write(
            "spider_worker/src/_mp_bc_rule_canonical_identity_d.rs",
            r#"mod Website {
        pub fn crawl() {}
    }
    fn mutation_proof_rule_canonical_identity_d_shipping_consumer() {
        Website::crawl();
    }
    "#,
        );
        let fixture = r#"
    id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
    summary = "Behavioral contract fixture."
    stage = "WIRED"

    [stages.DESIGNED]
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

    [stages.IMPLEMENTED]
    evidence = ["spider/src/website.rs:crawl_establish"]

    [stages.VERIFIED]
    test_only = true
    evidence = ["spider/tests/architecture_guardrails.rs:no_shadow_credential_aware_cache_policy_in_cli_or_mcp"]
    last_verified_command = "cargo test -p spider --lib --test architecture_guardrails --features \"chrome cache cache_request\""
    last_verified_result = "1/1"

    [stages.WIRED]
    callers = ["spider/src/website.rs:Website::crawl -> spider/src/website.rs:crawl_concurrent -> spider/src/website.rs:crawl_establish"]

    [stages.PRODUCTION_REACHABLE]
    reachability_kind = "binary_default"
    shipping_artifacts = ["spider_worker"]
    feature_requirements = ["cache"]
    entry_point_symbols = ["Website::crawl"]
    siblings_enumerated = true
    siblings = []
    siblings_note = "fixture"
    verdict = "MET"
    verdict_evidence = "fixture"
    "#;
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", fixture);
        assert!(
            !run_single_verifier_check_strict(dir.path(), None, "production_reachable_claims_are_grep_verified_against_shipping_manifests"),
            "the real verifier credited Website::crawl() as production reachability where `Website` was a decoy *module*, not the canonical struct — behavioral contract violated"
        );
    });
}

/// Macro adjacency, round 5 (Codex adversarial review, exact demonstrated
/// reproducer E): a locally-defined `macro_rules! join` shadowing the
/// real `tokio::join!` under the identical bare name must not establish
/// WIRED adjacency — this harness no longer credits any macro invocation
/// at all, precisely because it cannot tell a real `tokio::join!` from a
/// same-named local shadow.
#[test]
fn behavioral_verifier_rejects_locally_shadowed_join_macro_as_wired_call_adjacency() {
    locked_test(|| {
        let _scratch = ScratchFile::write(
            "spider/src/_mp_bc_rule_canonical_identity_e.rs",
            r#"macro_rules! join {
        ($($tokens:tt)*) => {};
    }
    struct Website;
    impl Website {
        async fn crawl(&mut self) {
            join!(self.mutation_proof_rule_canonical_identity_e_target());
        }
        async fn mutation_proof_rule_canonical_identity_e_target(&mut self) {}
    }
    "#,
        );
        let fixture = r#"
    id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
    summary = "Behavioral contract fixture."
    stage = "WIRED"

    [stages.DESIGNED]
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

    [stages.IMPLEMENTED]
    evidence = ["spider/src/_mp_bc_rule_canonical_identity_e.rs:mutation_proof_rule_canonical_identity_e_target"]

    [stages.VERIFIED]
    test_only = true
    evidence = ["spider/tests/architecture_guardrails.rs:no_shadow_credential_aware_cache_policy_in_cli_or_mcp"]
    last_verified_command = "cargo test -p spider --lib --test architecture_guardrails --features \"chrome cache cache_request\""
    last_verified_result = "1/1"

    [stages.WIRED]
    callers = ["spider/src/_mp_bc_rule_canonical_identity_e.rs:Website::crawl -> spider/src/_mp_bc_rule_canonical_identity_e.rs:mutation_proof_rule_canonical_identity_e_target"]
    "#;
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", fixture);
        assert!(
            !run_single_verifier_check_strict(dir.path(), None, "wired_stage_chains_prove_real_call_adjacency_rooted_in_production_source"),
            "the real verifier accepted a call inside a locally-shadowed `join!` macro as WIRED adjacency proof — behavioral contract violated"
        );
    });
}

/// Canonical identity, round 5, positive control (F): genuine canonical
/// Website evidence — reached in a real shipping artifact via a real
/// `use spider::website::Website;` import, exactly as
/// `spider_cli`/`spider_mcp`/`spider_worker`'s own real source does —
/// remains provable as PRODUCTION_REACHABLE. The hardened, affirmative-
/// provenance model does not fail closed on the one shape it exists to
/// keep working.
#[test]
fn behavioral_verifier_accepts_genuine_external_use_import_as_production_reachability() {
    locked_test(|| {
        let _scratch = ScratchFile::write(
            "spider_worker/src/_mp_bc_rule_canonical_identity_f.rs",
            r#"use spider::website::Website;
    fn mutation_proof_rule_canonical_identity_f_shipping_consumer() {
        let mut w = Website::new("https://example.com");
        w.crawl();
    }
    "#,
        );
        let fixture = r#"
    id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
    summary = "Behavioral contract fixture."
    stage = "WIRED"

    [stages.DESIGNED]
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

    [stages.IMPLEMENTED]
    evidence = ["spider/src/website.rs:crawl_establish"]

    [stages.VERIFIED]
    test_only = true
    evidence = ["spider/tests/architecture_guardrails.rs:no_shadow_credential_aware_cache_policy_in_cli_or_mcp"]
    last_verified_command = "cargo test -p spider --lib --test architecture_guardrails --features \"chrome cache cache_request\""
    last_verified_result = "1/1"

    [stages.WIRED]
    callers = ["spider/src/website.rs:Website::crawl -> spider/src/website.rs:crawl_concurrent -> spider/src/website.rs:crawl_establish"]

    [stages.PRODUCTION_REACHABLE]
    reachability_kind = "binary_default"
    shipping_artifacts = ["spider_worker"]
    feature_requirements = ["cache"]
    entry_point_symbols = ["Website::crawl"]
    siblings_enumerated = true
    siblings = []
    siblings_note = "fixture"
    verdict = "MET"
    verdict_evidence = "fixture"
    "#;
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", fixture);
        assert!(
            run_single_verifier_check_strict(dir.path(), None, "production_reachable_claims_are_grep_verified_against_shipping_manifests"),
            "the real verifier rejected genuine canonical Website evidence reached via a real `use spider::website::Website;` import — behavioral contract violated"
        );
    });
}

/// Macro adjacency (Codex adversarial review, exact demonstrated bypass):
/// a WIRED chain hop whose caller body only reaches its callee through a
/// non-allowlisted macro (`stringify!`, which never even evaluates its
/// argument) must not be credited as real call adjacency.
#[test]
fn behavioral_verifier_rejects_stringify_macro_argument_as_wired_call_adjacency() {
    locked_test(|| {
        let _scratch = ScratchFile::write(
            "spider/src/_mp_bc_rule_macro_adjacency.rs",
            r#"struct Website;
    impl Website {
        fn crawl(&mut self) {
            stringify!(mutation_proof_rule_macro_adjacency_target());
        }
    }
    fn mutation_proof_rule_macro_adjacency_target() {}
    "#,
        );
        let fixture = r#"
    id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
    summary = "Behavioral contract fixture."
    stage = "WIRED"

    [stages.DESIGNED]
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

    [stages.IMPLEMENTED]
    evidence = ["spider/src/_mp_bc_rule_macro_adjacency.rs:mutation_proof_rule_macro_adjacency_target"]

    [stages.VERIFIED]
    test_only = true
    evidence = ["spider/tests/architecture_guardrails.rs:no_shadow_credential_aware_cache_policy_in_cli_or_mcp"]
    last_verified_command = "cargo test -p spider --lib --test architecture_guardrails --features \"chrome cache cache_request\""
    last_verified_result = "1/1"

    [stages.WIRED]
    callers = ["spider/src/_mp_bc_rule_macro_adjacency.rs:Website::crawl -> spider/src/_mp_bc_rule_macro_adjacency.rs:mutation_proof_rule_macro_adjacency_target"]
    "#;
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", fixture);
        assert!(
            !run_single_verifier_check_strict(dir.path(), None, "wired_stage_chains_prove_real_call_adjacency_rooted_in_production_source"),
            "the real verifier accepted a stringify!()-argument as WIRED call adjacency proof — behavioral contract violated"
        );
    });
}

/// Cargo/libtest value-taking option validation (Codex adversarial
/// review, exact demonstrated bypass): a CI_ENFORCED evidence check must
/// never construct a `TestSelection` from a `run:` command real Cargo
/// itself would reject — `cargo test --test --lib` is not a request for
/// a test target literally named `--lib`; real Cargo parses `--lib` as
/// its own flag and rejects the command for `--test` having no value at
/// all.
#[test]
fn behavioral_verifier_rejects_ci_command_where_value_taking_option_consumes_another_option() {
    locked_test(|| {
        let fixture = format!(
            r#"{VALID_BASE}callers = ["{VALID_WIRED_CHAIN}"]
    {VALID_PRODUCTION_REACHABLE}
    [stages.CI_ENFORCED]
    ci_workflow_file = ".github/workflows/rust.yml"
    package = "spider"
    lib = false
    test_targets = ["--lib"]
    feature_set = ""
    positional_filters = []
    exact = false
    skip = []
    "#
        );
        // Before this fix, `--test` would greedily consume the following
        // `--lib` token as its test-target *value* rather than
        // recognizing it as its own flag — producing exactly the
        // (nonsensical, but structurally self-consistent) declared
        // selection above, and falsely satisfying CI_ENFORCED for a
        // command real Cargo itself would refuse to run at all.
        let workflow = r#"
    on:
      push:
        branches: [main]
    jobs:
      fixture_job:
        runs-on: ubuntu-latest
        steps:
          - name: fixture step
            run: cargo test -p spider --test --lib
    "#;
        let ledger_dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", &fixture);
        let workflows_dir = temp_workflows_dir(workflow);
        assert!(
            !run_single_verifier_check_strict(ledger_dir.path(), Some(workflows_dir.path()), "ci_enforced_commands_are_real_required_non_gated_steps_with_exact_feature_match"),
            "the real verifier accepted a CI command where `--test` consumed `--lib` as its value instead of rejecting the malformed command — behavioral contract violated"
        );
    });
}

// =====================================================================
// CI execution-semantics fail-closed fixtures (Codex adversarial
// review): CI_ENFORCED means the cited VERIFIED evidence actually
// executes — compile-only, list-only, wrong-selection, or
// unmodeled-feature-mode commands must never qualify, even when they
// otherwise structurally match the declared package/target/feature
// fields.
// =====================================================================

const CI_EXEC_SEMANTICS_BASE: &str = r#"
id = "SCORPION_BEHAVIORAL_FIXTURE_001"
sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
summary = "Behavioral contract fixture."
stage = "WIRED"

[stages.DESIGNED]
sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

[stages.IMPLEMENTED]
evidence = ["spider/src/website.rs:crawl_establish"]

[stages.VERIFIED]
test_only = true
evidence = ["spider/tests/architecture_guardrails.rs:no_shadow_credential_aware_cache_policy_in_cli_or_mcp"]
last_verified_command = "cargo test -p spider --lib --test architecture_guardrails --features \"chrome cache cache_request\""
last_verified_result = "1/1"

[stages.WIRED]
callers = ["spider/src/website.rs:Website::crawl -> spider/src/website.rs:crawl_concurrent -> spider/src/website.rs:crawl_establish"]

[stages.PRODUCTION_REACHABLE]
reachability_kind = "binary_default"
shipping_artifacts = ["spider_cli"]
feature_requirements = ["sitemap"]
entry_point_symbols = ["Website::crawl"]
siblings_enumerated = true
siblings = []
siblings_note = "fixture"
verdict = "MET"
verdict_evidence = "fixture"

[stages.CI_ENFORCED]
ci_workflow_file = ".github/workflows/rust.yml"
package = "spider"
lib = false
test_targets = ["architecture_guardrails"]
feature_set = ""
positional_filters = []
exact = false
skip = []
"#;

/// `--no-run` compiles the declared target but never executes it — the
/// cited VERIFIED evidence never runs. Must be rejected even though the
/// declared structural fields (package/target) otherwise match.
#[test]
fn behavioral_verifier_rejects_no_run_ci_evidence() {
    locked_test(|| {
        let workflow = r#"
    on:
      push:
        branches: [main]
    jobs:
      fixture_job:
        runs-on: ubuntu-latest
        steps:
          - name: fixture step
            run: cargo test -p spider --test architecture_guardrails --no-run
    "#;
        let ledger_dir =
            temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", CI_EXEC_SEMANTICS_BASE);
        let workflows_dir = temp_workflows_dir(workflow);
        assert!(
            !run_single_verifier_check_strict(ledger_dir.path(), Some(workflows_dir.path()), "ci_enforced_commands_are_real_required_non_gated_steps_with_exact_feature_match"),
            "the real verifier accepted a --no-run (compile-only) step as CI_ENFORCED evidence — behavioral contract violated"
        );
    });
}

/// `-- --list` prints test names and runs nothing. Must be rejected.
#[test]
fn behavioral_verifier_rejects_list_only_ci_evidence() {
    locked_test(|| {
        let workflow = r#"
    on:
      push:
        branches: [main]
    jobs:
      fixture_job:
        runs-on: ubuntu-latest
        steps:
          - name: fixture step
            run: cargo test -p spider --test architecture_guardrails -- --list
    "#;
        let ledger_dir =
            temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", CI_EXEC_SEMANTICS_BASE);
        let workflows_dir = temp_workflows_dir(workflow);
        assert!(
            !run_single_verifier_check_strict(ledger_dir.path(), Some(workflows_dir.path()), "ci_enforced_commands_are_real_required_non_gated_steps_with_exact_feature_match"),
            "the real verifier accepted a --list (list-only) step as CI_ENFORCED evidence — behavioral contract violated"
        );
    });
}

/// `-- --ignored` inverts normal selection to run only `#[ignore]`-
/// attributed tests. The cited VERIFIED evidence (not `#[ignore]`d) never
/// runs under this flag. Must be rejected.
#[test]
fn behavioral_verifier_rejects_ignored_only_ci_evidence() {
    locked_test(|| {
        let workflow = r#"
    on:
      push:
        branches: [main]
    jobs:
      fixture_job:
        runs-on: ubuntu-latest
        steps:
          - name: fixture step
            run: cargo test -p spider --test architecture_guardrails -- --ignored
    "#;
        let ledger_dir =
            temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", CI_EXEC_SEMANTICS_BASE);
        let workflows_dir = temp_workflows_dir(workflow);
        assert!(
            !run_single_verifier_check_strict(ledger_dir.path(), Some(workflows_dir.path()), "ci_enforced_commands_are_real_required_non_gated_steps_with_exact_feature_match"),
            "the real verifier accepted a --ignored (inverted-selection) step as CI_ENFORCED evidence — behavioral contract violated"
        );
    });
}

/// `--all-features` changes the active feature set independent of any
/// declared `--features` flag — this model does not represent that
/// effect, so it must fail closed rather than silently ignore the flag.
#[test]
fn behavioral_verifier_rejects_unmodeled_feature_mode_ci_evidence() {
    locked_test(|| {
        let workflow = r#"
    on:
      push:
        branches: [main]
    jobs:
      fixture_job:
        runs-on: ubuntu-latest
        steps:
          - name: fixture step
            run: cargo test -p spider --test architecture_guardrails --all-features
    "#;
        let ledger_dir =
            temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", CI_EXEC_SEMANTICS_BASE);
        let workflows_dir = temp_workflows_dir(workflow);
        assert!(
            !run_single_verifier_check_strict(ledger_dir.path(), Some(workflows_dir.path()), "ci_enforced_commands_are_real_required_non_gated_steps_with_exact_feature_match"),
            "the real verifier accepted an unmodeled feature-mode flag (--all-features) as CI_ENFORCED evidence — behavioral contract violated"
        );
    });
}

/// Canonical Website ownership, imported form: a real shipping artifact's
/// own `src/` tree importing something named `Website` from an unrelated
/// path (not this repository's own `website` module) and calling
/// `.crawl()` on a genuine-looking construction must not be credited as
/// that artifact reaching the canonical, WIRED-bound entry point — the
/// same file-trust guarantee already proven for a *locally defined*
/// decoy `Website`, exercised here for the *imported* bypass form
/// explicitly named in this round's review.
#[test]
fn behavioral_verifier_rejects_imported_unrelated_website_as_production_reachability() {
    locked_test(|| {
        let _scratch = ScratchFile::write(
            "spider_worker/src/_mp_bc_rule11_import.rs",
            r#"use some_other_crate::Website;
    fn mutation_proof_rule11_import_decoy_caller() {
        let mut w = Website::new("https://example.com");
        w.crawl();
    }
    "#,
        );
        let fixture = r#"
    id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
    summary = "Behavioral contract fixture."
    stage = "WIRED"

    [stages.DESIGNED]
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

    [stages.IMPLEMENTED]
    evidence = ["spider/src/website.rs:crawl_establish"]

    [stages.VERIFIED]
    test_only = true
    evidence = ["spider/tests/architecture_guardrails.rs:no_shadow_credential_aware_cache_policy_in_cli_or_mcp"]
    last_verified_command = "cargo test -p spider --lib --test architecture_guardrails --features \"chrome cache cache_request\""
    last_verified_result = "1/1"

    [stages.WIRED]
    callers = ["spider/src/website.rs:Website::crawl -> spider/src/website.rs:crawl_concurrent -> spider/src/website.rs:crawl_establish"]

    [stages.PRODUCTION_REACHABLE]
    reachability_kind = "binary_default"
    shipping_artifacts = ["spider_worker"]
    feature_requirements = ["cache"]
    entry_point_symbols = ["Website::crawl"]
    siblings_enumerated = true
    siblings = []
    siblings_note = "fixture"
    verdict = "MET"
    verdict_evidence = "fixture"
    "#;
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", fixture);
        assert!(
            !run_single_verifier_check_strict(dir.path(), None, "production_reachable_claims_are_grep_verified_against_shipping_manifests"),
            "the real verifier credited an imported, unrelated `Website` type in spider_worker's own src/ as production reachability — behavioral contract violated"
        );
    });
}

/// Module-qualified WIRED identity: a WIRED chain terminal whose bare
/// name exists in *two different modules* of the same file must not
/// silently resolve to whichever one the walk happens to visit first —
/// `real::hop` and `unrelated::hop` must remain distinct.
#[test]
fn behavioral_verifier_rejects_module_collision_wired_terminal() {
    locked_test(|| {
        let _scratch = ScratchFile::write(
            "spider/src/_mp_bc_rule_module_collision.rs",
            r#"pub struct Website;
    impl Website {
        pub async fn crawl(&mut self) {
            module_collision_target();
        }
    }
    mod real {
        pub fn module_collision_target() {}
    }
    mod unrelated {
        pub fn module_collision_target() {}
    }
    "#,
        );
        let fixture = r#"
    id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
    summary = "Behavioral contract fixture."
    stage = "WIRED"

    [stages.DESIGNED]
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

    [stages.IMPLEMENTED]
    evidence = ["spider/src/_mp_bc_rule_module_collision.rs:module_collision_target"]

    [stages.VERIFIED]
    test_only = true
    evidence = ["spider/tests/architecture_guardrails.rs:no_shadow_credential_aware_cache_policy_in_cli_or_mcp"]
    last_verified_command = "cargo test -p spider --lib --test architecture_guardrails --features \"chrome cache cache_request\""
    last_verified_result = "1/1"

    [stages.WIRED]
    callers = ["spider/src/_mp_bc_rule_module_collision.rs:Website::crawl -> spider/src/_mp_bc_rule_module_collision.rs:module_collision_target"]
    "#;
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", fixture);
        assert!(
            !run_single_verifier_check_strict(
                dir.path(),
                None,
                "wired_stage_chains_prove_real_call_adjacency_rooted_in_production_source"
            ),
            "the real verifier accepted a WIRED terminal whose bare name is ambiguous across two different modules — behavioral contract violated"
        );
    });
}

fn proof_fixture(required: &str, proof: &str) -> String {
    format!(
        r#"id = "SCORPION_BEHAVIORAL_FIXTURE_001"
sdd = "SCORPION_ARCHITECTURE.md"
summary = "Proof-class behavioral fixture."
stage = "DESIGNED"
required_proof_classes = [{required}]
{proof}
[stages.DESIGNED]
sdd = "SCORPION_ARCHITECTURE.md"
"#
    )
}

fn proof_fixture_is_rejected(fixture: String, verifier: &str) {
    let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", &fixture);
    assert!(
        !run_single_verifier_check_strict(dir.path(), None, verifier),
        "the real verifier accepted an invalid proof-class fixture"
    );
}

#[test]
fn behavioral_ci_enforced_cannot_substitute_for_ci_proven() {
    locked_test(|| {
        proof_fixture_is_rejected(
        proof_fixture("\"CI_PROVEN\"", "[stages.CLOSED]\nclosed_commit = \"13cbc2dfcc410fa49843b304e45b62102e5012e4\"\n[stages.CI_ENFORCED]\ncommand_identity = \"configured-only\""),
        "closed_requires_every_declared_proof_class_independently",
    )
    });
}

#[test]
fn behavioral_ci_proven_requires_run_identity() {
    locked_test(|| {
        proof_fixture_is_rejected(
        proof_fixture("\"CI_PROVEN\"", "[proof.CI_PROVEN]\ncapability_id = \"SCORPION_BEHAVIORAL_FIXTURE_001\"\ncommit = \"13cbc2dfcc410fa49843b304e45b62102e5012e4\"\nworkflow = \".github/workflows/rust.yml\"\nrun_id = \"\"\nrun_url = \"\"\njob = \"job\"\nstep = \"step\"\nconclusion = \"success\"\ncommand_identity = \"command\""),
        "proof_class_records_are_typed_bound_and_non_substitutable",
    )
    });
}

#[test]
fn behavioral_ci_proven_rejects_wrong_commit() {
    locked_test(|| {
        proof_fixture_is_rejected(
        proof_fixture("\"CI_PROVEN\"", "[proof.CI_PROVEN]\ncapability_id = \"SCORPION_BEHAVIORAL_FIXTURE_001\"\ncommit = \"13cbc2dfcc410fa49843b304e45b62102e5012e4\"\nrun_commit = \"0000000000000000000000000000000000000000\"\nworkflow = \".github/workflows/rust.yml\"\nrun_id = \"1\"\nrun_url = \"https://github.com/owner/repo/actions/runs/1\"\njob = \"job\"\nstep = \"step\"\nconclusion = \"success\"\ncommand_identity = \"command\""),
        "proof_class_records_are_typed_bound_and_non_substitutable",
    )
    });
}

#[test]
fn behavioral_operator_observation_requires_concrete_result() {
    locked_test(|| {
        proof_fixture_is_rejected(
        proof_fixture("\"OPERATOR_OBSERVED\"", "[[proof.OPERATOR_OBSERVED]]\ncapability_id = \"SCORPION_BEHAVIORAL_FIXTURE_001\"\ncommit = \"13cbc2dfcc410fa49843b304e45b62102e5012e4\"\ncommand = \"scorpion research topic\"\npurpose = \"acceptance\"\nresult = \"\""),
        "proof_class_records_are_typed_bound_and_non_substitutable",
    )
    });
}

#[test]
fn behavioral_live_classification_is_not_observation() {
    locked_test(|| {
        proof_fixture_is_rejected(
        proof_fixture("\"LIVE_ENVIRONMENT_DEPENDENT\"", "[proof.LIVE_ENVIRONMENT_DEPENDENT]\ncapability_id = \"SCORPION_BEHAVIORAL_FIXTURE_001\"\nrequirements = [\"real provider\"]\nobserved = true"),
        "proof_class_records_are_typed_bound_and_non_substitutable",
    )
    });
}

#[test]
fn behavioral_unproven_cannot_satisfy_required_proof() {
    locked_test(|| {
        proof_fixture_is_rejected(
        proof_fixture("\"CODE_PROVEN\"", "[proof.UNPROVEN]\ncapability_id = \"SCORPION_BEHAVIORAL_FIXTURE_001\"\nmissing = [\"CODE_PROVEN\"]\nreason = \"missing\"\n[stages.CLOSED]\nclosed_commit = \"13cbc2dfcc410fa49843b304e45b62102e5012e4\""),
        "closed_requires_every_declared_proof_class_independently",
    )
    });
}

#[test]
fn behavioral_closed_rejects_missing_required_proof() {
    locked_test(|| {
        proof_fixture_is_rejected(
            proof_fixture(
                "\"OPERATOR_OBSERVED\"",
                "[stages.CLOSED]\nclosed_commit = \"13cbc2dfcc410fa49843b304e45b62102e5012e4\"",
            ),
            "closed_requires_every_declared_proof_class_independently",
        )
    });
}

#[test]
fn behavioral_proof_cannot_attach_to_wrong_capability() {
    locked_test(|| {
        proof_fixture_is_rejected(
        proof_fixture("\"CODE_PROVEN\"", "[proof.CODE_PROVEN]\ncapability_id = \"SCORPION_OTHER_001\"\ncommit = \"13cbc2dfcc410fa49843b304e45b62102e5012e4\"\nevidence = [\"fact\"]"),
        "proof_class_records_are_typed_bound_and_non_substitutable",
    )
    });
}

#[test]
fn behavioral_deterministic_tests_alone_cannot_close() {
    locked_test(|| {
        proof_fixture_is_rejected(
        proof_fixture("\"CODE_PROVEN\", \"CI_PROVEN\"", "[proof.CODE_PROVEN]\ncapability_id = \"SCORPION_BEHAVIORAL_FIXTURE_001\"\ncommit = \"13cbc2dfcc410fa49843b304e45b62102e5012e4\"\nevidence = [\"tests passed\"]\n[stages.CLOSED]\nclosed_commit = \"13cbc2dfcc410fa49843b304e45b62102e5012e4\""),
        "closed_requires_every_declared_proof_class_independently",
    )
    });
}

#[test]
fn behavioral_workflow_presence_is_not_successful_ci() {
    locked_test(|| {
        proof_fixture_is_rejected(
        proof_fixture("\"CI_PROVEN\"", "[stages.CI_ENFORCED]\ncommand_identity = \"present-in-yaml\"\n[stages.CLOSED]\nclosed_commit = \"13cbc2dfcc410fa49843b304e45b62102e5012e4\""),
        "closed_requires_every_declared_proof_class_independently",
    )
    });
}

#[test]
fn behavioral_ci_proven_accepts_exact_run_and_configured_step_identity() {
    locked_test(|| {
        let fixture = proof_fixture(
            "\"CI_PROVEN\"",
            "[proof.CI_PROVEN]\ncapability_id = \"SCORPION_BEHAVIORAL_FIXTURE_001\"\ncommit = \"13cbc2dfcc410fa49843b304e45b62102e5012e4\"\nrun_commit = \"13cbc2dfcc410fa49843b304e45b62102e5012e4\"\nworkflow = \".github/workflows/rust.yml\"\nrun_id = \"12345\"\nrun_url = \"https://github.com/owner/repo/actions/runs/12345\"\njob = \"assurance\"\nstep = \"Exact assurance\"\nconclusion = \"success\"\ncommand_identity = \"package=spider;lib=false;tests=architecture_guardrails;features=;filters=;exact=false;skip=\"\n[stages.CI_ENFORCED]\nci_workflow_file = \".github/workflows/rust.yml\"\npackage = \"spider\"\nlib = false\ntest_targets = [\"architecture_guardrails\"]\nfeature_set = \"\"\npositional_filters = []\nexact = false\nskip = []",
        );
        let workflow = r#"
on:
  push:
    branches: [main]
jobs:
  assurance:
    runs-on: ubuntu-latest
    steps:
      - name: Exact assurance
        run: cargo test -p spider --test architecture_guardrails
"#;
        let ledger_dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", &fixture);
        let workflows_dir = temp_workflows_dir(workflow);
        assert!(
            run_single_verifier_check_strict(
                ledger_dir.path(),
                Some(workflows_dir.path()),
                "proof_class_records_are_typed_bound_and_non_substitutable"
            ),
            "the real verifier rejected a complete exact CI_PROVEN record"
        );
    });
}

// =====================================================================
// SCORPION_CANONICAL_CAPTCHA_MACHINE_READABLE_CAPABILITY_COVERAGE_001 —
// capability-specific negative proofs. Every generic verifier rule above
// is already proven correct against synthetic/decoy fixtures unrelated to
// any specific capability; the five tests below instead mutate fixtures
// built from this CAPTCHA capability's own *real* production symbols
// (spider/src/features/captcha_browser.rs, browser_challenge_detection.rs,
// solvers.rs) to prove the generic rules correctly reject tampering with
// THIS capability's own real evidence, not just an unrelated decoy's.
// =====================================================================

/// Removing/renaming the real OOPIF browser-action production symbol must
/// be rejected: IMPLEMENTED evidence naming a plausible but nonexistent
/// sibling of the real `execute_browser_captcha_attempt_in_frame` symbol.
#[test]
fn behavioral_verifier_rejects_missing_oopif_action_production_symbol() {
    locked_test(|| {
        let fixture = r#"
    id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
    summary = "Behavioral contract fixture."
    stage = "IMPLEMENTED"

    [stages.DESIGNED]
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

    [stages.IMPLEMENTED]
    evidence = ["spider/src/features/captcha_browser.rs:execute_browser_captcha_attempt_in_oopif_v2"]
    "#;
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", fixture);
        assert!(
            !run_single_verifier_check_strict(
                dir.path(),
                None,
                "implemented_stage_evidence_references_real_definitions_not_comments"
            ),
            "the real verifier accepted a nonexistent OOPIF action symbol as IMPLEMENTED \
             evidence — behavioral contract violated"
        );
    });
}

/// A production symbol name that exists only inside a comment (never a
/// real definition) in a real, on-disk CAPTCHA source file must be
/// rejected as IMPLEMENTED evidence.
#[test]
fn behavioral_verifier_rejects_comment_only_captcha_symbol() {
    locked_test(|| {
        let _scratch = ScratchFile::write(
            "spider/src/features/_mp_bc_captcha_comment.rs",
            r#"// The real dispatcher used to be called
// execute_browser_captcha_attempt_in_oopif_comment_only — renamed before
// shipping. This mention is prose, not a definition.
pub fn unrelated_helper() {}
"#,
        );
        let fixture = r#"
    id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
    summary = "Behavioral contract fixture."
    stage = "IMPLEMENTED"

    [stages.DESIGNED]
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

    [stages.IMPLEMENTED]
    evidence = ["spider/src/features/_mp_bc_captcha_comment.rs:execute_browser_captcha_attempt_in_oopif_comment_only"]
    "#;
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", fixture);
        assert!(
            !run_single_verifier_check_strict(
                dir.path(),
                None,
                "implemented_stage_evidence_references_real_definitions_not_comments"
            ),
            "the real verifier accepted a comment-only mention of a CAPTCHA symbol as \
             IMPLEMENTED evidence — behavioral contract violated"
        );
    });
}

/// VERIFIED evidence naming a plausible but nonexistent PaliGemma test
/// (a name that could easily be typo'd from the real
/// `real_browser_snapshot_paligemma_inference_and_exact_action`) must be
/// rejected.
#[test]
fn behavioral_verifier_rejects_evidence_pointing_to_nonexistent_paligemma_test() {
    locked_test(|| {
        let fixture = r#"
    id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
    summary = "Behavioral contract fixture."
    stage = "VERIFIED"

    [stages.DESIGNED]
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

    [stages.IMPLEMENTED]
    evidence = ["spider/src/features/captcha_browser.rs:execute_browser_captcha_attempt_in_frame"]

    [stages.VERIFIED]
    test_only = true
    evidence = ["spider/tests/captcha_browser_paligemma_real.rs:real_browser_snapshot_paligemma_inference_and_exact_action_v2"]
    last_verified_command = "cargo test -p spider --test captcha_browser_paligemma_real --features \"chrome local_paligemma local_paligemma_cuda\""
    last_verified_result = "fixture"
    "#;
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", fixture);
        assert!(
            !run_single_verifier_check_strict(
                dir.path(),
                None,
                "verified_stage_evidence_resolves_to_real_test_definitions"
            ),
            "the real verifier accepted a nonexistent PaliGemma test name as VERIFIED evidence \
             — behavioral contract violated"
        );
    });
}

/// `spider_worker` genuinely does not call `Website::crawl` in its own
/// source (its request handlers use `Page::new_page_streaming` directly —
/// confirmed in this same capability's real ledger entry's own
/// `siblings_note`). Claiming `PRODUCTION_REACHABLE.verdict = "MET"` with
/// `spider_worker` in `shipping_artifacts` must be rejected — a stage
/// must not be marked production-reachable without a valid production
/// caller.
#[test]
fn behavioral_verifier_rejects_production_reachable_claim_for_a_non_calling_artifact() {
    locked_test(|| {
        let fixture = format!(
            r#"{VALID_BASE}callers = ["{VALID_WIRED_CHAIN}"]

    [stages.PRODUCTION_REACHABLE]
    reachability_kind = "binary_optional_flag"
    shipping_artifacts = ["spider_worker"]
    feature_requirements = ["chrome"]
    entry_point_symbols = ["Website::crawl"]
    siblings_enumerated = true
    siblings = []
    siblings_note = "fixture"
    verdict = "MET"
    verdict_evidence = "fixture"
    "#
        );
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", &fixture);
        assert!(
            !run_single_verifier_check_strict(
                dir.path(),
                None,
                "production_reachable_claims_are_grep_verified_against_shipping_manifests"
            ),
            "the real verifier accepted spider_worker as production-reachable despite its own \
             source never calling Website::crawl — behavioral contract violated"
        );
    });
}

/// The real PaliGemma provider-construction call
/// (`register_browser_challenge_providers!`'s expansion, invoked from
/// `route_detected_browser_challenge`'s body) genuinely happens only
/// inside a macro invocation this harness never credits as call
/// adjacency (`SCORPION_CANONICAL_CAPTCHA_MACHINE_READABLE_CAPABILITY_
/// COVERAGE_001`'s own SDD, section 3). A WIRED chain claiming
/// `route_detected_browser_challenge` calls `resolve_paligemma_provider`
/// must be rejected — the real repository has no such direct adjacency.
#[test]
fn behavioral_verifier_rejects_macro_shielded_paligemma_routing_hop_as_wired() {
    locked_test(|| {
        let fixture = r#"
    id = "SCORPION_BEHAVIORAL_FIXTURE_001"
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"
    summary = "Behavioral contract fixture."
    stage = "WIRED"

    [stages.DESIGNED]
    sdd = "docs/frontier/CANONICAL_CLOSURE_AND_PRODUCTION_REALITY_HARNESS_SDD.md"

    [stages.IMPLEMENTED]
    evidence = ["spider/src/features/solvers.rs:resolve_paligemma_provider"]
    additional_cfg_features = ["local_paligemma"]

    [stages.VERIFIED]
    test_only = true
    evidence = ["spider/tests/architecture_guardrails.rs:no_shadow_credential_aware_cache_policy_in_cli_or_mcp"]
    last_verified_command = "cargo test -p spider --lib --test architecture_guardrails --features \"chrome cache cache_request\""
    last_verified_result = "1/1"

    [stages.WIRED]
    additional_cfg_features = ["local_paligemma", "fs"]
    callers = ["spider/src/website.rs:Website::crawl -> spider/src/website.rs:crawl_concurrent -> spider/src/website.rs:crawl_establish -> spider/src/page.rs:Page::new_streaming -> spider/src/page.rs:Page::new_base -> spider/src/utils/mod.rs:fetch_page_html -> spider/src/utils/mod.rs:fetch_page_html_chrome_base -> spider/src/utils/mod.rs:fetch_page_html_chrome_base_inner -> spider/src/features/browser_challenge_detection.rs:detect_browser_challenge -> spider/src/features/solvers.rs:resolve_paligemma_provider"]
    "#;
        let dir = temp_ledger_with_fixture("SCORPION_BEHAVIORAL_FIXTURE_001", fixture);
        assert!(
            !run_single_verifier_check_strict(
                dir.path(),
                None,
                "wired_stage_chains_prove_real_call_adjacency_rooted_in_production_source"
            ),
            "the real verifier accepted a WIRED chain through the macro-shielded PaliGemma \
             provider-construction hop — behavioral contract violated"
        );
    });
}
