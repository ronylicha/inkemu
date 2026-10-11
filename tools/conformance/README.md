# Conformance suites

Test-only tooling: Docker images and third-party suites are never linked into or shipped with inkemu.

## Pinned versions

`versions.env` is the single source of pins: `RISCV_TESTS_COMMIT` (riscv-tests fixtures),
`RISCV_ARCH_TEST_TAG`, `SAIL_RISCV_RELEASE`, `RISCOF_VERSION` (arch-test), and the expected test
count per arch-test group (`ARCH_TEST_EXPECTED`, `ARCH_TEST_TOTAL`). Bump pins in a dedicated PR and
re-count the groups deliberately.

## riscv-arch-test through RISCOF (`run-arch-test.sh`)

```bash
tools/conformance/run-arch-test.sh                       # builds inkemu, builds the image, runs everything
ARCH_TEST_KEEP=/tmp/archtest tools/conformance/run-arch-test.sh   # also keep ELFs, logs and signatures
```

Requires Docker and the Rust toolchain. RISCOF compiles each test, runs it on `inkemu rvtest` and on
Sail (formal reference model), and compares the signatures. The script exits 0 only if every group
ran and passed exactly the pinned number of tests (a missing group or a shrunk group fails with the
group name and expected vs actual).

- Groups run: `I M C Zifencei privilege` (92 tests with riscv-arch-test 3.10.0).
- **Excluded: `pmp32`.** RISCOF selects it for any DUT with Zicsr (the tests carry no PMP feature
  check) but the SP1a core has no PMP CSRs and the ISA YAML does not advertise them. To re-enable it
  in SP1b once PMP exists: add `pmp32=<count>` to `ARCH_TEST_EXPECTED` and update `ARCH_TEST_TOTAL`
  in `versions.env`; the script copies exactly the groups listed there.
- **No Zicsr group.** riscv-arch-test 3.10.0 has no `Zicsr` directory: CSR instructions are covered
  only through the `privilege` tests (ecall, ebreak, misaligned accesses), whose trap handlers use
  csrr/csrw/csrrw and whose signatures record mepc/mcause/mtval.
- **The reference is configured as the same machine as the DUT** (`riscof/sail/inkemu_rv32imc.json`
  applied over Sail's `--rv32` defaults, driven through the adapter `riscof/sail/riscv_sim_RV32`).
  Sail's defaults enable A/F/D/V/B/S/U/H and non-trapping misaligned accesses, which changes the
  trap-handler signatures.

### Misaligned-access policy: three places that must change together

The core traps on misaligned loads/stores by default. That policy is encoded in three places:

1. `riscof/inkemu/inkemu_isa.yaml`: `hw_data_misaligned_support` (selects which variant of the
   `privilege/misalign-*` tests RISCOF builds).
2. `riscof/sail/inkemu_rv32imc.json`: `memory.misaligned.exceptions.load_store`
   (`AlignmentException` = trap, `{"None": null}` = hardware support).
3. The DUT command in `riscof/inkemu/riscof_inkemu.py`: add `--allow-misaligned` to
   `inkemu rvtest` for hardware support.

Flipping only one of them makes the signatures diverge.
