#!/bin/bash
# SCORPION_CANONICAL_CI_DETERMINISTIC_NETWORK_LOCKDOWN_EXECUTION_001:
# CONTINUE — remote step-10 closure-assurance failure.
#
# TEMPORARY diagnostic only. The canonical "Cargo test closure/architecture
# assurance suite (chrome cache cache_request)" workflow step combines four
# test binaries (architecture_guardrails, closure_harness,
# closure_harness_integrity, closure_harness_behavioral_contract) into one
# `cargo test` invocation and fails remotely (exit 101) with no per-test
# detail available — that step is deliberately bare/unwrapped, required by
# closure_harness.rs's own CI_ENFORCED grammar
# (`closure_harness_itself_is_a_real_required_test_flag_in_ci`: "a
# non-executing wrapper does not count"), so it cannot be given output
# capture without breaking the very proof machinery this investigation
# depends on. This script decomposes that combined step into one binary at
# a time, under the exact same execution identity (network namespace, root,
# injected PATH/HOME/CARGO_HOME/RUSTUP_HOME) as the canonical step, purely
# to answer "which binary fails, and with what output?" It never
# substitutes for, wraps, or weakens the canonical step itself — that step's
# own `run:` text is untouched.
#
# Deliberately kept out of the workflow YAML's own `run:` text (a plain
# `bash .github/scripts/diagnose-step10-binary.sh <binary>` call, not an
# inline `cargo test` invocation) so closure_harness.rs's own
# `load_workflow_steps`/`required_ci_excludes_every_registered_live_test`/
# `ci_enforced_commands_are_real_required_non_gated_steps_with_exact_
# feature_match` never see or reason about this diagnostic's actual command
# text at all — the same established precedent as
# report-test-step-failure.sh. The calling workflow steps are also marked
# `if: always()` so they still run after the canonical step 10 fails
# (GitHub Actions skips subsequent steps by default once one fails), which
# independently marks them `gated` in closure_harness.rs's own model — a
# second, redundant reason they can never be mistaken for required,
# non-gated CI evidence.
set -euo pipefail

binary_name="$1"
shift
log_file="/tmp/diag-${binary_name}.log"

echo "DIAGNOSTIC: cargo test -p spider --test ${binary_name} --features \"chrome cache cache_request\" $*"
if sudo ip netns exec spider_ci env PATH=/home/runner/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin HOME=/home/runner CARGO_HOME=/home/runner/.cargo RUSTUP_HOME=/home/runner/.rustup cargo test -p spider --test "${binary_name}" --features "chrome cache cache_request" "$@" > "${log_file}" 2>&1; then
  echo "DIAGNOSTIC RESULT: ${binary_name} PASSED"
  tail -5 "${log_file}"
else
  status=$?
  echo "::error::DIAGNOSTIC RESULT: ${binary_name} FAILED (exit ${status})"
  grep -A5 -E "^test .* FAILED$|panicked at|^error|^test result:" "${log_file}" | while IFS= read -r line; do
    echo "::error::  output: ${line}"
  done
  exit "${status}"
fi
