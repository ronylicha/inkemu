#!/usr/bin/env bash
# Builds inkemu and runs riscv-arch-test (RV32IMC + Zicsr + Zifencei) against Sail through RISCOF.
# Groups and their expected test counts are pinned in tools/conformance/versions.env.
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
  -e ARCH_TEST_EXPECTED="$ARCH_TEST_EXPECTED" -e ARCH_TEST_TOTAL="$ARCH_TEST_TOTAL" \
  -v "$PWD:/work" "${keep_args[@]}" -w /tmp inkemu/riscof bash -euo pipefail -c '
  # The suite is the arch-test groups listed in ARCH_TEST_EXPECTED. I, M, C and Zifencei are the
  # unprivileged groups; privilege/ holds the M-mode trap tests (ecall, ebreak, misaligned
  # accesses). The A/B/F/D/K/V/... groups are not part of this ISA and RISCOF drops them from the
  # ISA YAML. pmp32/ needs enforced PMP. The generic hart this bench runs has no PMP CSRs and the
  # ISA YAML does not advertise them (the ESP32-C3 hart stores them, unenforced, since SP1b-2), yet
  # RISCOF selects pmp32/ anyway, so it is left out of the suite directory itself.
  groups=""
  for entry in $ARCH_TEST_EXPECTED; do groups="$groups ${entry%%=*}"; done
  echo "arch-test groups:$groups (pmp32 excluded: PMP is stored on the ESP32-C3 hart and not enforced; SP1b-4 enforces it and re-enables pmp32 by adding it back to ARCH_TEST_EXPECTED)"
  # RISCOF deletes and recreates its work dir, so a kept copy is made on exit instead of a mount.
  trap "[[ -d /keep ]] && cp -r /tmp/riscof_work/. /tmp/riscof.log /keep/ || true" EXIT
  src=/opt/riscv-arch-test/riscv-test-suite
  suite=/tmp/suite
  mkdir -p "$suite/rv32i_m"
  cp -r "$src/env" "$suite/env"
  for group in $groups; do cp -r "$src/rv32i_m/$group" "$suite/rv32i_m/$group"; done
  # riscof exits 0 on several fatal errors and writes its verdicts to stderr: judge the log, not the status.
  riscof run --config=/work/tools/conformance/riscof/config.ini \
    --suite="$suite" --env="$suite/env" \
    --work-dir=/tmp/riscof_work --no-browser 2>&1 | tee /tmp/riscof.log
  sed -i "s/\x1b\[[0-9;]*m//g" /tmp/riscof.log
  status=0
  for entry in $ARCH_TEST_EXPECTED; do
    group=${entry%%=*}
    want=${entry#*=}
    ran_g=$(grep -cE "/rv32i_m/$group/.*: (Passed|Failed)\$" /tmp/riscof.log || true)
    passed_g=$(grep -cE "/rv32i_m/$group/.*: Passed\$" /tmp/riscof.log || true)
    echo "arch-test: group $group: expected $want, ran $ran_g, passed $passed_g"
    if [[ "$ran_g" -ne "$want" || "$passed_g" -ne "$want" ]]; then
      echo "arch-test: group $group: expected $want tests passing, got $passed_g of $ran_g run" >&2
      status=1
    fi
  done
  passed=$(grep -cE ": Passed\$" /tmp/riscof.log || true)
  failed=$(grep -cE ": Failed\$" /tmp/riscof.log || true)
  ran=$(sed -nE "s/.*Following ([0-9]+) tests have been run.*/\1/p" /tmp/riscof.log | tail -1)
  echo "arch-test: total expected $ARCH_TEST_TOTAL, ran ${ran:-0}, passed $passed, failed $failed"
  if [[ "$passed" -ne "$ARCH_TEST_TOTAL" || "${ran:-0}" -ne "$ARCH_TEST_TOTAL" || "$failed" -ne 0 ]]; then
    echo "arch-test: total: expected $ARCH_TEST_TOTAL tests passing, got $passed passed, $failed failed, ${ran:-0} run" >&2
    status=1
  fi
  [[ "$status" -eq 0 ]] || { echo "arch-test: FAILED" >&2; exit 1; }
  echo "arch-test: OK"
'
