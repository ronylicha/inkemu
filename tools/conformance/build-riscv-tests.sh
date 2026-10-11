#!/usr/bin/env bash
# Rebuilds tests/fixtures/riscv-tests from the pinned riscv-tests commit.
set -euo pipefail
cd "$(dirname "$0")/../.."
source tools/conformance/versions.env
out=tests/fixtures/riscv-tests
docker build -t inkemu/riscv-gcc tools/docker/riscv-gcc
# Build in a scratch dir, then replace only the ELFs and the manifest: LICENSE and README.md stay.
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
mkdir -p "$out"
docker run --rm -u "$(id -u):$(id -g)" -e HOME=/tmp -v "$stage:/out" -w /tmp inkemu/riscv-gcc bash -euo pipefail -c "
  git clone -q https://github.com/riscv-software-src/riscv-tests src
  cd src && git checkout -q $RISCV_TESTS_COMMIT && git submodule update -q --init env
  cd isa
  # Physical-memory (-p) variants only: the virtual-memory (-v) ones need S-mode and a libc.
  printf 'include Makefile\n.PHONY: inkemu-p\ninkemu-p: \$(rv32ui_p_tests) \$(rv32um_p_tests) \$(rv32uc_p_tests) \$(rv32mi_p_tests)\n' > inkemu.mk
  make -s -f inkemu.mk XLEN=32 RISCV_PREFIX=riscv64-unknown-elf- inkemu-p
  for f in rv32ui-p-* rv32um-p-* rv32uc-p-* rv32mi-p-*; do
    # gcc names its temporary object ccXXXXXX.o and that name lands in a FILE symbol: drop it so
    # the committed fixtures are byte-for-byte reproducible (every other symbol is kept).
    case \$f in *.dump) ;; *) riscv64-unknown-elf-objcopy --wildcard --strip-symbol 'cc*.o' \"\$f\" \"/out/\$f\" ;; esac
  done"
find "$out" -maxdepth 1 -type f \( -name 'rv32*' -o -name SHA256SUMS \) -delete
cp "$stage"/rv32* "$out"/
(cd "$out" && LC_ALL=C sha256sum -- rv32* > SHA256SUMS)
echo "built $(wc -l < "$out/SHA256SUMS") fixtures into $out"
