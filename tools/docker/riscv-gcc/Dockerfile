# Test-fixture toolchain only (GPL): never linked into or shipped with inkemu.
FROM ubuntu:24.04
RUN apt-get update \
 && apt-get install -y --no-install-recommends gcc-riscv64-unknown-elf binutils-riscv64-unknown-elf \
      make autoconf git ca-certificates python3 python3-venv \
 && rm -rf /var/lib/apt/lists/*
