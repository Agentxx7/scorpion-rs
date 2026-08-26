#!/bin/bash
# SCORPION_CANONICAL_CI_DETERMINISTIC_NETWORK_LOCKDOWN_EXECUTION_001
# (CONTINUE: remote step-10 closure-assurance failure — now step 11,
# CAPTCHA CI-portable evidence, newly reached for the first time ever
# on real remote CI after the fetch-depth:0 fix unblocked step 10).
#
# TEMPORARY diagnostic only. Step 11 ("Cargo test CAPTCHA CI-portable
# evidence (chrome local_paligemma)") combines six real-Chrome test
# binaries into one bare, unwrapped `cargo test` invocation (required
# bare for closure_harness.rs's own CI_ENFORCED grammar, same
# constraint as step 10) and failed remotely (exit 101) with no
# per-test detail available. A local reproduction of this exact
# command (without RUST_MIN_STACK, matching the real step-11 command)
# hit a stack overflow in captcha_browser_oopif_streaming_shipping_real
# specifically — a *hypothesis* this diagnostic exists to confirm or
# reject with real remote evidence, not to assume. This script runs
# each of the six binaries individually under the same true root+
# namespace identity as the canonical step, and separately captures
# each one WITH RUST_MIN_STACK=67108864 set too, so the notice/error
# output directly shows whether that variable is the actual difference
# — same script-hiding rationale/precedent as every other
# diagnose-step10-*.sh in this investigation.
set -uo pipefail

binary_name="$1"
log_file="/tmp/diag11-${binary_name}.log"
log_file_stack="/tmp/diag11-${binary_name}-stack.log"

run_one() {
  local log="$1"
  shift
  sudo ip netns exec spider_ci env PATH=/home/runner/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin HOME=/home/runner CARGO_HOME=/home/runner/.cargo RUSTUP_HOME=/home/runner/.rustup "$@" cargo test -p spider --test "${binary_name}" --features "chrome local_paligemma" > "${log}" 2>&1
  echo $?
}

rc1=$(run_one "${log_file}")
if [ "${rc1}" -eq 0 ]; then
  echo "::notice::${binary_name} (no RUST_MIN_STACK) PASSED"
else
  tail_block="$(tail -12 "${log_file}")"
  echo "::error::${binary_name} (no RUST_MIN_STACK) FAILED exit=${rc1}"
  echo "${tail_block}" | while IFS= read -r line; do
    echo "::error::  ${line}"
  done
fi

rc2=$(run_one "${log_file_stack}" env RUST_MIN_STACK=67108864)
if [ "${rc2}" -eq 0 ]; then
  echo "::notice::${binary_name} (RUST_MIN_STACK=67108864) PASSED"
else
  tail_block2="$(tail -12 "${log_file_stack}")"
  echo "::error::${binary_name} (RUST_MIN_STACK=67108864) FAILED exit=${rc2}"
  echo "${tail_block2}" | while IFS= read -r line; do
    echo "::error::  ${line}"
  done
fi

if [ "${rc1}" -eq 0 ] && [ "${rc2}" -eq 0 ]; then
  exit 0
fi
exit 1
