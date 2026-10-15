# Conformance tooling only (GPL/BSD tools): never linked into or shipped with inkemu.
FROM ubuntu:24.04
ARG RISCV_ARCH_TEST_TAG
ARG SAIL_RISCV_RELEASE
ARG RISCOF_VERSION
RUN apt-get update \
 && apt-get install -y --no-install-recommends gcc-riscv64-unknown-elf binutils-riscv64-unknown-elf \
      make git ca-certificates curl python3 python3-venv patch \
 && rm -rf /var/lib/apt/lists/*
# setuptools provides the `distutils` shim that the arch-test Sail plugin imports (removed from Python 3.12).
RUN python3 -m venv /opt/venv && /opt/venv/bin/pip install --no-cache-dir setuptools "riscof==${RISCOF_VERSION}"
ENV PATH="/opt/venv/bin:/opt/sail/bin:${PATH}"
RUN git clone -q --depth 1 --branch "${RISCV_ARCH_TEST_TAG}" https://github.com/riscv-non-isa/riscv-arch-test /opt/riscv-arch-test \
 && git config --system --add safe.directory /opt/riscv-arch-test
# The Sail plugin calls riscv32-unknown-elf-*; Ubuntu ships one riscv64 toolchain that targets both widths.
RUN for t in gcc objdump; do ln -s "$(command -v riscv64-unknown-elf-$t)" /usr/local/bin/riscv32-unknown-elf-$t; done
RUN mkdir -p /opt/sail \
 && curl -fsSL "https://github.com/riscv/sail-riscv/releases/download/${SAIL_RISCV_RELEASE}/sail-riscv-Linux-x86_64.tar.gz" \
    | tar -xz -C /opt/sail --strip-components=1
