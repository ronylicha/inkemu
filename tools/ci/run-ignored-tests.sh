#!/usr/bin/env bash
# Runs `cargo test <arguments>` and fails unless it ran exactly the expected number of tests, all
# passing. Why: `cargo test ... -- --ignored` exits 0 with "running 0 tests" when a test is renamed
# or filtered out, and silently skips a test that lost its #[ignore]; a green job would then prove
# nothing. The expected count is deliberate: adding or removing a test is an edit of the call site.
#
# Usage: tools/ci/run-ignored-tests.sh <expected-passed> <cargo test arguments...>
# Example, for a test binary with three ignored tests:
#   tools/ci/run-ignored-tests.sh 3 --release -p <crate> --test <binary> -- --ignored
# The calls that are run, with their counts, are in .github/workflows/ci.yml.
set -euo pipefail
# The result line is matched as plain text below: colour codes around it would defeat the match,
# so colour is switched off here rather than left to the caller's environment.
export CARGO_TERM_COLOR=never
if [ "$#" -lt 2 ]; then
  echo "usage: $0 <expected-passed> <cargo test arguments...>" >&2
  exit 2
fi
expected="$1"
shift
log="$(mktemp)"
trap 'rm -f "$log"' EXIT

# The exit code is kept in a variable (never read through a pipe) and the output goes to a file.
code=0
cargo test "$@" > "$log" 2>&1 || code=$?
cat "$log"
if [ "$code" -ne 0 ]; then
  echo "error: cargo test $* exited with code $code; the output is above" >&2
  exit 1
fi

# One test binary, so exactly one result line is expected.
results="$(grep '^test result:' "$log" || true)"
want="test result: ok. $expected passed; 0 failed; 0 ignored; 0 measured;"
if [[ "$results" == *$'\n'* || "$results" != "$want"* ]]; then
  echo "error: cargo test $* must report \"$want ...\" exactly once; it reported:" >&2
  echo "${results:-<no result line>}" >&2
  echo "cause: a test was renamed, filtered out, lost its #[ignore] or was added; if the change is deliberate, edit the expected count in .github/workflows/ci.yml" >&2
  exit 1
fi
