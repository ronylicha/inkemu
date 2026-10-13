#!/usr/bin/env bash
# Runs `cargo clippy <arguments>` and fails when clippy fails, or when one of the paths a
# clippy.toml bans names nothing. Why: clippy reports a banned path it cannot resolve (a typo:
# `std::thread::sleeep`) as a warning about its configuration, which `-D warnings` does not turn
# into an error; the exit code stays 0 and the call the entry was meant to refuse compiles.
#
# Usage: tools/ci/clippy.sh <cargo clippy arguments...>
# Example, the call of the CI job `check`:
#   tools/ci/clippy.sh --workspace --all-targets --all-features -- -D warnings
set -euo pipefail
export CARGO_TERM_COLOR=never
if [ "$#" -lt 1 ]; then
  echo "usage: $0 <cargo clippy arguments...>" >&2
  exit 2
fi
log="$(mktemp)"
trap 'rm -f "$log"' EXIT

# The exit code is kept in a variable (never read through a pipe) and the output goes to a file.
code=0
cargo clippy "$@" > "$log" 2>&1 || code=$?
cat "$log"
if [ "$code" -ne 0 ]; then
  echo "error: cargo clippy $* exited with code $code; the output is above" >&2
  exit 1
fi
if grep -q 'does not refer to' "$log"; then
  echo "error: a path banned by a clippy.toml names nothing (the lines that say \"does not refer to\" above), so the call it was meant to refuse compiles; correct the path in every clippy.toml that bans host I/O and in crates/boards/tests/host_io_ban.rs" >&2
  exit 1
fi
