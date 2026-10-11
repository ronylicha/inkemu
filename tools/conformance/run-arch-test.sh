#!/usr/bin/env bash
# Builds inkemu and runs riscv-arch-test (RV32IMC + Zicsr + Zifencei) against Sail through RISCOF.
# Set ARCH_TEST_KEEP=<dir> to keep RISCOF's work dir (ELFs, logs, signatures) for debugging.
set -euo pipefail
cd "$(dirname "$0")/../.."
source tools/conformance/versions.env
cargo build --release -p emu-cli
docker build -q -t inkemu/riscof \
  --build-arg RISCV_ARCH_TEST_TAG="$RISCV_ARCH_TEST_TAG" \
  --build-arg SAIL_RISCV_RELEASE="$SAIL_RISCV_RELEASE" \
  --build-arg RISCOF_VERSION="$RISCOF_VERSION" tools/docker/riscof
keep_args=()
if [[ -n "${ARCH_TEST_KEEP:-}" ]]; then
  mkdir -p "$ARCH_TEST_KEEP"
  keep_args=(-v "$(realpath "$ARCH_TEST_KEEP"):/keep")
fi
docker run --rm -u "$(id -u):$(id -g)" -e HOME=/tmp -e PYTHONDONTWRITEBYTECODE=1 \
  -v "$PWD:/work" "${keep_args[@]}" -w /tmp inkemu/riscof bash -euo pipefail -c '
  # The suite is the arch-test groups that match the declared ISA (RV32IMC + Zicsr + Zifencei).
  # I, M, C and Zifencei are the unprivileged groups; privilege/ holds the M-mode trap tests
  # (ecall, ebreak, misaligned accesses). The A/B/F/D/K/V/... groups are not part of this ISA, and
  # pmp32/ needs PMP CSRs, which this core does not have and the ISA YAML does not advertise.
  # RISCOF selects pmp32/ regardless of the YAML, so it is left out of the suite directory itself.
  # RISCOF deletes and recreates its work dir, so a kept copy is made on exit instead of a mount.
  trap "[[ -d /keep ]] && cp -r /tmp/riscof_work/. /tmp/riscof.log /keep/ || true" EXIT
  src=/opt/riscv-arch-test/riscv-test-suite
  suite=/tmp/suite
  mkdir -p "$suite/rv32i_m"
  cp -r "$src/env" "$suite/env"
  for group in I M C Zifencei privilege; do cp -r "$src/rv32i_m/$group" "$suite/rv32i_m/$group"; done
  # riscof exits 0 on several fatal errors and writes its verdicts to stderr: judge the log, not the status.
  riscof run --config=/work/tools/conformance/riscof/config.ini \
    --suite="$suite" --env="$suite/env" \
    --work-dir=/tmp/riscof_work --no-browser 2>&1 | tee /tmp/riscof.log
  sed -i "s/\x1b\[[0-9;]*m//g" /tmp/riscof.log
  passed=$(grep -cE ": Passed\$" /tmp/riscof.log || true)
  failed=$(grep -cE ": Failed\$" /tmp/riscof.log || true)
  ran=$(sed -nE "s/.*Following ([0-9]+) tests have been run.*/\1/p" /tmp/riscof.log | tail -1)
  echo "arch-test: ${ran:-0} run, $passed passed, $failed failed"
  [[ "$failed" -eq 0 && "$passed" -gt 0 && "$passed" -eq "${ran:-0}" ]] || { echo "arch-test: FAILED" >&2; exit 1; }
'
