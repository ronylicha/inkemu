# Conformance suites

Test-only tooling: Docker images and third-party suites are never linked into or shipped with inkemu.

## Pinned versions

`versions.env` is the single source of pins: `RISCV_TESTS_COMMIT` (riscv-tests fixtures),
`RISCV_ARCH_TEST_TAG`, `SAIL_RISCV_RELEASE`, `RISCOF_VERSION` (arch-test), the expected test
count per arch-test group (`ARCH_TEST_EXPECTED`, `ARCH_TEST_TOTAL`), and the riscv-tests the bench runs
and skips (`RISCV_TESTS_RAN`, `RISCV_TESTS_SKIPPED`, held by `crates/emu-cli/tests/riscv_tests.rs`,
which runs in the `check` job's `cargo test --workspace`). Bump pins in a dedicated PR and re-count
deliberately.

## riscv-arch-test through RISCOF (`run-arch-test.sh`)

```bash
tools/conformance/run-arch-test.sh                       # builds inkemu, builds the image, runs everything
ARCH_TEST_KEEP=/tmp/archtest tools/conformance/run-arch-test.sh   # also keep ELFs, logs and signatures
ARCH_TEST_ONLY=pmp32 tools/conformance/run-arch-test.sh            # only these groups, with their pinned counts
```

Requires Docker and the Rust toolchain. RISCOF compiles each test, runs it on `inkemu rvtest` and on
Sail (formal reference model), and compares the signatures. The script exits 0 only if every group
ran and passed exactly the pinned number of tests (a missing group or a shrunk group fails with the
group name and expected vs actual).

- Groups run: `I M C Zifencei privilege pmp32` (133 tests with riscv-arch-test 3.10.0, 41 of them
  in `pmp32`). The script copies exactly the groups listed in `ARCH_TEST_EXPECTED`.
- **The hart under test** is inkemu's conformance bench (`cpu_rv32::Core::MachineUser`, the hart
  `inkemu rvtest` runs): RV32IMC + Zicsr + Zifencei with machine and user modes, as the ESP32-C3
  (TRM register 1.6), and a 16-entry PMP with a grain of 4 bytes. Where the privileged
  specification leaves a choice, the bench makes Sail's, and the ESP32-C3 core of inkemu keeps its
  own (the TRM's PMP has no static priority): `docs/research/esp32c3-pmp-and-user-mode.md`, ADR 0010.
- **`pmp32` runs with a patched environment.** 40 of its 41 tests declare an S-mode trap routine,
  and the arch-test environment then runs S-mode code that a hart without S-mode cannot run (it
  loops for ever, on Sail as on inkemu). `run-arch-test.sh` applies
  `riscof/arch-test-env-no-smode.patch` to the copy of `env/arch_test.h` both models compile with,
  after checking the original's SHA-256: the S prolog and epilog and the `medeleg` accesses are
  compiled out, nothing of the tests is touched, and their "S-mode" parts run in user mode on both
  models. The patch says what each hunk does.
- **No Zicsr group.** riscv-arch-test 3.10.0 has no `Zicsr` directory: CSR instructions are covered
  only through the `privilege` tests (ecall, ebreak, misaligned accesses), whose trap handlers use
  csrr/csrw/csrrw and whose signatures record mepc/mcause/mtval.
- **The reference is configured as the same machine as the DUT** (`riscof/sail/inkemu_rv32imc.json`
  applied over Sail's `--rv32` defaults, driven through the adapter `riscof/sail/riscv_sim_RV32`).
  Sail's defaults enable A/F/D/V/B/S/H and non-trapping misaligned accesses, which changes the
  trap-handler signatures; the override keeps U, makes a `pmpcfg` write of R = 0 with W = 1 leave
  the entry as it was (`PMP_Ignore`, as inkemu and QEMU), and lets `wfi` wait in user mode unless
  `mstatus.TW` is set. Sail's default PMP (16 entries, grain 0) is the bench's.
- **No Zicntr, on either side.** The ISA declares none and Sail is configured with
  `"Zicntr": {"supported": false}`: Sail 0.15 then makes `cycle`, `time`, `instret` and their high
  halves illegal in every mode (`model/extensions/Zicntr/zicntr_control.sail`) and keeps machine
  mode's `mcycle` and `minstret` (`model/core/counters.sail`). The bench does the same;
  `mcounteren` exists, every bit writable, and gates nothing. Declaring Zicntr instead would need a
  `time` the bench does not have. The riscv-test `rv32mi-p-zicntr` is skipped for it, and
  `crates/emu-cli/tests/riscv_tests.rs` pins that it fails at its first read of `cycle`.

### Misaligned-access policy: three places that must change together

The core traps on misaligned loads/stores by default. That policy is encoded in three places:

1. `riscof/inkemu/inkemu_isa.yaml`: `hw_data_misaligned_support` (selects which variant of the
   `privilege/misalign-*` tests RISCOF builds).
2. `riscof/sail/inkemu_rv32imc.json`: `memory.misaligned.exceptions.load_store`
   (`AlignmentException` = trap, `{"None": null}` = hardware support).
3. The DUT command in `riscof/inkemu/riscof_inkemu.py`: add `--allow-misaligned` to
   `inkemu rvtest` for hardware support.

Flipping only one of them makes the signatures diverge.
