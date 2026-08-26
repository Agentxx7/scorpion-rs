#!/bin/bash
# SCORPION_CANONICAL_CI_DETERMINISTIC_NETWORK_LOCKDOWN_EXECUTION_001
# (CONTINUE: remote step-10 closure-assurance failure).
#
# TEMPORARY diagnostic only. Narrower than diagnose-step10-binary.sh:
# runs exactly one named `#[test]` function, under the true canonical
# root+namespace identity, with `--exact --nocapture --test-threads=1`,
# and emits only the actual panic/assertion detail (not the whole test
# binary's output) as a small, fixed number of `::error::` lines —
# empirically found this round (diagnose-step10-binary.sh's own `-A5`
# grep) to still be too many lines for GitHub's 10-annotation-per-step
# cap once combined with the surrounding "test ... FAILED"/separator
# noise, cutting the real assertion detail off before it was ever
# visible.
#
# Kept out of the workflow YAML's own `run:` text and marked `gated` via
# the calling step's `if: always()`, same rationale and precedent as
# the other diagnose-step10-*.sh scripts.
set -uo pipefail

binary_name="$1"
test_name="$2"
log_file="/tmp/diag-exact-${binary_name}-${test_name}.log"

sudo ip netns exec spider_ci env PATH=/home/runner/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin HOME=/home/runner CARGO_HOME=/home/runner/.cargo RUSTUP_HOME=/home/runner/.rustup cargo test -p spider --test "${binary_name}" --features "chrome cache cache_request" "${test_name}" -- --exact --nocapture --test-threads=1 > "${log_file}" 2>&1
rc=$?

if [ "${rc}" -eq 0 ]; then
  echo "::notice::EXACT ${binary_name}::${test_name} PASSED under true root+namespace identity"
else
  # Only the panic message + up to 6 following lines (the actual
  # assertion left/right detail lives there) — no "test ... FAILED"
  # banner, no "test result:" summary, no separator lines, so the
  # 10-annotation-per-step budget is spent entirely on real content.
  panic_block="$(grep -A6 "panicked at" "${log_file}" | head -12)"
  if [ -z "${panic_block}" ]; then
    # No "panicked at" line found at all (e.g. the subprocess itself
    # never ran, or failed before reaching the assertion) — fall back
    # to the last 12 lines of the log so there is still *something*.
    panic_block="$(tail -12 "${log_file}")"
  fi
  echo "::error::EXACT ${binary_name}::${test_name} FAILED (exit ${rc}) under true root+namespace identity"
  echo "${panic_block}" | while IFS= read -r line; do
    echo "::error::  ${line}"
  done
fi
exit "${rc}"
