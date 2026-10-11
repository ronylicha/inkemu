# riscv-tests fixtures

Pre-built `rv32ui`, `rv32um`, `rv32uc` and `rv32mi` physical-memory (`-p`) test ELFs from
<https://github.com/riscv-software-src/riscv-tests>, pinned at commit
`7b68f7b91f827f8d94cc1c57ff123752c5c8f704` (BSD-3-Clause, see `LICENSE`).

They are test fixtures only: they are never linked into or shipped with inkemu itself.
They are produced by `tools/conformance/build-riscv-tests.sh` (GCC in the
`tools/docker/riscv-gcc` image); `SHA256SUMS` is the manifest the test suite checks.
Do not edit the ELF files by hand: rerun the script.
