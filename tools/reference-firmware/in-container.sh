#!/usr/bin/env bash
# Runs inside the espressif/idf container (see build.sh). /ref is this directory, read-only; /out
# receives the results. Nothing reaches /out until the merged image has passed its pinned hash: the
# image and the symbol tables are produced in a temporary directory, verified, and only then moved.
set -euo pipefail

# Runs one step with its output in a log file; on failure prints the tail of that log and stops.
step() {
  local name="$1" log="$2"
  shift 2
  if ! "$@" > "$log" 2>&1; then
    tail -n 60 "$log" >&2
    echo "error: $name failed; see the log above" >&2
    exit 1
  fi
}

project=/tmp/hello_world
stage="$(mktemp -d)"
rm -rf "$project"
cp -r "$IDF_PATH/examples/get-started/hello_world" "$project"
cp /ref/sdkconfig.defaults "$project/sdkconfig.defaults"
cd "$project"
step "idf.py set-target" /tmp/set-target.log idf.py set-target esp32c3
step "idf.py build" /tmp/build.log idf.py build
cd build
if ! sha256sum -c /ref/SHA256SUMS.inputs; then
  sha256sum bootloader/bootloader.bin partition_table/partition-table.bin hello_world.bin >&2
  echo "error: the build is not the pinned one (hashes above); do not edit SHA256SUMS.inputs: check the image tag, the ESP-IDF version and sdkconfig.defaults" >&2
  exit 1
fi
step "esptool merge_bin" /tmp/merge.log \
  esptool.py --chip esp32c3 merge_bin --fill-flash-size 16MB -o "$stage/flash_hello.bin" \
  --flash_mode dio --flash_freq 80m --flash_size 16MB \
  0x0 bootloader/bootloader.bin 0x8000 partition_table/partition-table.bin 0x10000 hello_world.bin
for elf in bootloader/bootloader hello_world; do
  if ! riscv32-esp-elf-nm -n "$elf.elf" > "$stage/$(basename "$elf").nm" 2> /tmp/nm.log; then
    tail -n 20 /tmp/nm.log >&2
    echo "error: riscv32-esp-elf-nm failed on $elf.elf; see the log above" >&2
    exit 1
  fi
done
if ! (cd "$stage" && sha256sum -c /ref/SHA256SUMS.output); then
  (cd "$stage" && sha256sum flash_hello.bin) >&2
  tail -n 20 /tmp/merge.log >&2
  echo "error: the merged image is not the pinned one (hash above); nothing was written to the output directory" >&2
  exit 1
fi
chown "$HOST_UID:$HOST_GID" "$stage/flash_hello.bin" "$stage/bootloader.nm" "$stage/hello_world.nm"
mv "$stage/flash_hello.bin" "$stage/bootloader.nm" "$stage/hello_world.nm" /out/
