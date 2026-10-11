#!/usr/bin/env bash
# Runs inside the espressif/idf container (see build.sh). /ref is this directory, read-only; /out
# receives the results.
set -euo pipefail
project=/tmp/hello_world
rm -rf "$project"
cp -r "$IDF_PATH/examples/get-started/hello_world" "$project"
cp /ref/sdkconfig.defaults "$project/sdkconfig.defaults"
cd "$project"
if ! idf.py set-target esp32c3 > /tmp/set-target.log 2>&1; then
  tail -n 40 /tmp/set-target.log >&2
  echo "error: idf.py set-target failed; see the log above" >&2
  exit 1
fi
if ! idf.py build > /tmp/build.log 2>&1; then
  tail -n 60 /tmp/build.log >&2
  echo "error: idf.py build failed; see the log above" >&2
  exit 1
fi
cd build
if ! sha256sum -c /ref/SHA256SUMS.inputs; then
  sha256sum bootloader/bootloader.bin partition_table/partition-table.bin hello_world.bin >&2
  echo "error: the build is not the pinned one (hashes above); do not edit SHA256SUMS.inputs: check the image tag, the ESP-IDF version and sdkconfig.defaults" >&2
  exit 1
fi
esptool.py --chip esp32c3 merge_bin --fill-flash-size 16MB -o /out/flash_hello.bin \
  --flash_mode dio --flash_freq 80m --flash_size 16MB \
  0x0 bootloader/bootloader.bin 0x8000 partition_table/partition-table.bin 0x10000 hello_world.bin > /tmp/merge.log 2>&1
riscv32-esp-elf-nm -n bootloader/bootloader.elf > /out/bootloader.nm
riscv32-esp-elf-nm -n hello_world.elf > /out/hello_world.nm
(cd /out && sha256sum -c /ref/SHA256SUMS.output)
chown "$HOST_UID:$HOST_GID" /out/flash_hello.bin /out/bootloader.nm /out/hello_world.nm
