#!/bin/bash
# SCORPION_CANONICAL_CI_DETERMINISTIC_NETWORK_LOCKDOWN_EXECUTION_001:
# called from .github/workflows/rust.yml's spider_core job on a failing
# `cargo test` step, after that step's own combined stdout+stderr has
# already been redirected to a log file. Filters that log down to the
# lines that actually matter (failed test names, panics, compile
# errors, the final summary line) and echoes each as its own
# `::error::` annotation, visible via the public check-runs
# annotations API even though raw job logs require admin auth this
# session doesn't have.
#
# Deliberately a separate file, not inlined into the workflow YAML's
# own `run:` text: closure_harness.rs's `load_workflow_steps` only
# reads `.github/workflows/*.yml`, so this file's own use of grep's
# `-E` flag and ordinary English words (which previously, inlined,
# either broke `shell_text_is_unambiguous` on a raw backslash/hash, or
# silently corrupted `required_ci_excludes_every_registered_live_test`'s
# unrelated positional-filter parsing via an accidental test-name
# substring collision — `in` inside `website::crawl_invalid`, `do`
# inside `website::test_crawl_subdomains`) is invisible to that parser
# entirely.
set -euo pipefail

log_file="$1"
description="$2"

echo "::error::TIMED OUT (600s) OR FAILED: ${description}"
grep -E "^test .* FAILED$|panicked at|^error|^test result:" "${log_file}" | while IFS= read -r line; do
  echo "::error::  output: ${line}"
done
