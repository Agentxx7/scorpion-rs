#!/bin/bash
# SCORPION_CANONICAL_CI_DETERMINISTIC_NETWORK_LOCKDOWN_EXECUTION_001
# (CONTINUE: remote step-10 closure-assurance failure).
#
# TEMPORARY diagnostic only. Prints the execution-identity/environment
# matrix requested by this investigation's own directive (UID/EUID/GID,
# USER/LOGNAME/HOME, PATH/CARGO_HOME/RUSTUP_HOME, TMPDIR/XDG_*, CI/
# GITHUB_ACTIONS/GITHUB_WORKSPACE, RUST_MIN_STACK/RUST_TEST_THREADS, git
# global/system config, umask, repo/target/tmp ownership, CPU count) for
# both the normal runner-user identity and the true
# `sudo ip netns exec spider_ci env PATH=<literal> HOME=<literal>
# CARGO_HOME=<literal> RUSTUP_HOME=<literal>` identity the canonical
# step-10 command actually runs under — then, under the *latter* identity
# only, runs the exact three `git` subcommands closure_harness.rs itself
# invokes against the checked-out repository (`git -C <root> cat-file -t
# <sha>`, `git -C <root> merge-base --is-ancestor <sha> HEAD`, `git -C
# <root> show <commit>:<path>`) to directly test the leading hypothesis:
# that a real-UID/repo-owner mismatch under root triggers git's own
# "detected dubious ownership" safety refusal, which no earlier step in
# this job would ever have surfaced (steps 8/9 run `cargo test` only,
# never `git`, under this same root+namespace identity).
#
# Every line of output is emitted as `::notice::`/`::warning::`
# (GitHub Actions workflow-command syntax) rather than plain stdout —
# a real gap found empirically this round: plain stdout from a
# workflow step is only visible in raw job logs, which return 403
# (admin rights required) for this session; only `::error::`/
# `::warning::`/`::notice::`-prefixed lines are visible via the public
# check-runs annotations API regardless of step outcome. Multi-line
# values are re-emitted one `::notice::` line at a time (the
# `%0A`/newline workflow-command escaping is fragile across runners),
# since annotations are what this diagnostic exists to be read through.
#
# Kept out of the workflow YAML's own `run:` text and marked `gated` via
# the calling step's `if: always()`, same rationale and precedent as
# diagnose-step10-binary.sh and report-test-step-failure.sh.
set -uo pipefail

notice() {
  while IFS= read -r line; do
    echo "::notice::${line}"
  done <<< "$1"
}

root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"

print_identity() {
  notice "id: $(id)"
  notice "whoami: $(whoami 2>&1)"
  notice "USER=${USER:-<unset>} LOGNAME=${LOGNAME:-<unset>} HOME=${HOME:-<unset>}"
  notice "CARGO_HOME=${CARGO_HOME:-<unset>} RUSTUP_HOME=${RUSTUP_HOME:-<unset>}"
  notice "TMPDIR=${TMPDIR:-<unset>} XDG_CACHE_HOME=${XDG_CACHE_HOME:-<unset>} XDG_CONFIG_HOME=${XDG_CONFIG_HOME:-<unset>}"
  notice "CI=${CI:-<unset>} GITHUB_ACTIONS=${GITHUB_ACTIONS:-<unset>} GITHUB_WORKSPACE=${GITHUB_WORKSPACE:-<unset>}"
  notice "RUST_MIN_STACK=${RUST_MIN_STACK:-<unset>} RUST_TEST_THREADS=${RUST_TEST_THREADS:-<unset>}"
  notice "umask: $(umask)"
  notice "nproc: $(nproc 2>&1)"
  notice "repo dir owner: $(stat -c '%U(%u):%G(%g) %n' "${root}" 2>&1)"
  notice "target dir owner: $(stat -c '%U(%u):%G(%g) %n' "${root}/target" 2>&1)"
  notice "/tmp owner/perms: $(stat -c '%U(%u):%G(%g) %a %n' /tmp 2>&1)"
  notice "git --version: $(git --version 2>&1)"
  notice "git config --global --get-all safe.directory: $(git config --global --get-all safe.directory 2>&1 || echo '<none/error>')"
  notice "git config --system --get-all safe.directory: $(git config --system --get-all safe.directory 2>&1 || echo '<none/error>')"
  notice "git -C <root> rev-parse HEAD: $(git -C "${root}" rev-parse HEAD 2>&1)"
}

notice "=== NORMAL USER (no sudo, no namespace) ==="
print_identity

notice "=== TRUE REMOTE IDENTITY: sudo ip netns exec spider_ci env PATH=... HOME=/home/runner CARGO_HOME=/home/runner/.cargo RUSTUP_HOME=/home/runner/.rustup ==="
inner_script="$(mktemp)"
cat > "${inner_script}" <<INNER_EOF
set -uo pipefail
echo "id: \$(id)"
echo "whoami: \$(whoami 2>&1)"
echo "USER=\${USER:-<unset>} LOGNAME=\${LOGNAME:-<unset>} HOME=\${HOME:-<unset>}"
echo "CARGO_HOME=\${CARGO_HOME:-<unset>} RUSTUP_HOME=\${RUSTUP_HOME:-<unset>}"
echo "TMPDIR=\${TMPDIR:-<unset>} XDG_CACHE_HOME=\${XDG_CACHE_HOME:-<unset>} XDG_CONFIG_HOME=\${XDG_CONFIG_HOME:-<unset>}"
echo "CI=\${CI:-<unset>} GITHUB_ACTIONS=\${GITHUB_ACTIONS:-<unset>} GITHUB_WORKSPACE=\${GITHUB_WORKSPACE:-<unset>}"
echo "umask: \$(umask)"
echo "repo dir owner: \$(stat -c '%U(%u):%G(%g) %n' '${root}' 2>&1)"
echo "target dir owner: \$(stat -c '%U(%u):%G(%g) %n' '${root}/target' 2>&1)"
echo "/tmp owner/perms: \$(stat -c '%U(%u):%G(%g) %a %n' /tmp 2>&1)"
echo "git --version: \$(git --version 2>&1)"
echo "git config --global --get-all safe.directory: \$(git config --global --get-all safe.directory 2>&1 || echo '<none/error>')"
echo "git config --system --get-all safe.directory: \$(git config --system --get-all safe.directory 2>&1 || echo '<none/error>')"
echo "[1] git -C <root> cat-file -t HEAD: \$(git -C "${root}" cat-file -t HEAD 2>&1) | exit=\$?"
echo "[2] git -C <root> merge-base --is-ancestor HEAD HEAD: \$(git -C "${root}" merge-base --is-ancestor HEAD HEAD 2>&1) | exit=\$?"
echo "[3] git -C <root> show HEAD:.github/workflows/rust.yml (first line): \$(git -C "${root}" show HEAD:.github/workflows/rust.yml 2>&1 | head -1) | exit=\$?"
echo "[4] git -C <root> rev-parse HEAD: \$(git -C "${root}" rev-parse HEAD 2>&1) | exit=\$?"
INNER_EOF

inner_output="$(sudo ip netns exec spider_ci env PATH=/home/runner/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin HOME=/home/runner CARGO_HOME=/home/runner/.cargo RUSTUP_HOME=/home/runner/.rustup bash "${inner_script}" 2>&1)"
rc=$?
rm -f "${inner_script}"
notice "${inner_output}"
notice "root-identity block exit status: ${rc}"
