#!/bin/bash
# SCORPION_CANONICAL_CI_DETERMINISTIC_NETWORK_LOCKDOWN_EXECUTION_001
# (CONTINUE: remote step-10 closure-assurance failure).
#
# TEMPORARY diagnostic only. Runs the exact three `git` subcommands
# closure_harness.rs itself invokes against the checked-out repository
# (`git -C <root> cat-file -t <sha>`, `git -C <root> merge-base
# --is-ancestor <sha> HEAD`, `git -C <root> show <commit>:<path>`) under
# the true canonical-step-10 identity (`sudo ip netns exec spider_ci env
# PATH=<literal> HOME=<literal> CARGO_HOME=<literal> RUSTUP_HOME=<literal>`
# — root, HOME pointed at the runner user's own checkout/config) to
# directly test the leading hypothesis: that a real-UID/repo-owner
# mismatch under root triggers git's own "detected dubious ownership"
# safety refusal, which no earlier step in this job (8, 9 — `cargo test`
# only, never `git`) would ever have surfaced. Also captures UID/HOME/
# safe.directory/repo-ownership for both the normal-user and root
# identities, for comparison.
#
# Output is emitted as `::notice::` (GitHub Actions workflow-command
# syntax) rather than plain stdout — raw job logs return 403 (admin
# rights required) for this session, so only `::error::`/`::warning::`/
# `::notice::`-prefixed lines are visible at all via the public
# check-runs annotations API. A second, empirically-found constraint
# (this script's own first two attempts hit it): GitHub truncates to the
# *first 10* notice annotations per step and silently drops the rest —
# this script is deliberately kept to 9 total notice lines, with the
# actual git invocation results (the entire point of this diagnostic)
# emitted first, so truncation can never cut them off even if the count
# crept up again later.
#
# Kept out of the workflow YAML's own `run:` text and marked `gated` via
# the calling step's `if: always()`, same rationale and precedent as
# diagnose-step10-binary.sh and report-test-step-failure.sh.
set -uo pipefail

notice() { echo "::notice::$1"; }

root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"

inner_script="$(mktemp)"
cat > "${inner_script}" <<INNER_EOF
set -uo pipefail
cf_out=\$(git -C "${root}" cat-file -t HEAD 2>&1); cf_rc=\$?
echo "GIT-CATFILE | out=\${cf_out} exit=\${cf_rc}"
mb_out=\$(git -C "${root}" merge-base --is-ancestor HEAD HEAD 2>&1); mb_rc=\$?
echo "GIT-MERGEBASE | out=\${mb_out} exit=\${mb_rc}"
sh_out=\$(git -C "${root}" show HEAD:.github/workflows/rust.yml 2>&1 | head -c 80); sh_rc=\${PIPESTATUS[0]}
echo "GIT-SHOW | out=\${sh_out} exit=\${sh_rc}"
rp_out=\$(git -C "${root}" rev-parse HEAD 2>&1); rp_rc=\$?
echo "GIT-REVPARSE | out=\${rp_out} exit=\${rp_rc}"
echo "ROOT-ID | \$(id) | whoami=\$(whoami 2>&1) HOME=\${HOME:-<unset>}"
echo "ROOT-SAFEDIR | global=\$(git config --global --get-all safe.directory 2>&1 | tr '\n' ',' ) system=\$(git config --system --get-all safe.directory 2>&1 | tr '\n' ',')"
echo "ROOT-OWNER | repo=\$(stat -c '%U(%u)' '${root}' 2>&1) target=\$(stat -c '%U(%u)' '${root}/target' 2>&1)"
INNER_EOF

inner_output="$(sudo ip netns exec spider_ci env PATH=/home/runner/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin HOME=/home/runner CARGO_HOME=/home/runner/.cargo RUSTUP_HOME=/home/runner/.rustup bash "${inner_script}" 2>&1)"
rm -f "${inner_script}"

while IFS= read -r line; do
  [ -n "${line}" ] && notice "${line}"
done <<< "${inner_output}"

notice "NORMAL-ID | $(id) | whoami=$(whoami 2>&1) HOME=${HOME:-<unset>}"
notice "NORMAL-OWNER | repo=$(stat -c '%U(%u)' "${root}" 2>&1) target=$(stat -c '%U(%u)' "${root}/target" 2>&1)"
