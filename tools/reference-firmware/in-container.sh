#!/usr/bin/env bash
# Runs inside the espressif/idf container (see build.sh). /ref is this directory and /partitions the
# partition-table fixtures of the repository, both read-only; /out receives the results. Nothing
# reaches /out until every merged image has passed its pinned hash: the images and the symbol
# tables are produced in a temporary directory, verified, and only then moved.
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

# Builds the ESP-IDF example "$1" (a path under $IDF_PATH/examples), unmodified, with the reference
# configuration as its only one, checks its three binaries against the pinned hashes of "$2", and
# leaves the shell in its build directory. It says how long the build took: the cost of each
# example in a CI job is read in the job log.
build_example() {
  local example="$1" sums="$2"
  local name project started="$SECONDS"
  name="$(basename "$example")"
  project="/tmp/$name"
  rm -rf "$project"
  cp -r "$IDF_PATH/examples/$example" "$project"
  cp /ref/sdkconfig.defaults "$project/sdkconfig.defaults"
  cd "$project"
  step "idf.py set-target ($name)" "/tmp/set-target-$name.log" idf.py set-target esp32c3
  step "idf.py build ($name)" "/tmp/build-$name.log" idf.py build
  cd build
  if ! sha256sum -c "$sums"; then
    sha256sum bootloader/bootloader.bin partition_table/partition-table.bin "$name.bin" >&2
    echo "error: the build of $name is not the pinned one (hashes above); do not edit $(basename "$sums"): check the image tag, the ESP-IDF version and sdkconfig.defaults" >&2
    exit 1
  fi
  echo "$name: built and checked in $((SECONDS - started)) s"
}

# Merges the bootloader of the current build directory, the partition table "$2" and the
# application "$3" into the 16 MB flash image "$stage/$1".
merge() {
  local image="$1" table="$2" application="$3"
  step "esptool merge_bin ($image)" "/tmp/merge-$image.log" \
    esptool.py --chip esp32c3 merge_bin --fill-flash-size 16MB -o "$stage/$image" \
    --flash_mode dio --flash_freq 80m --flash_size 16MB \
    0x0 bootloader/bootloader.bin 0x8000 "$table" 0x10000 "$application"
}

# Writes the symbol table of each ELF file given (without its extension) to "$stage".
symbols() {
  local elf
  for elf in "$@"; do
    if ! riscv32-esp-elf-nm -n "$elf.elf" > "$stage/$(basename "$elf").nm" 2> /tmp/nm.log; then
      tail -n 20 /tmp/nm.log >&2
      echo "error: riscv32-esp-elf-nm failed on $elf.elf; see the log above" >&2
      exit 1
    fi
  done
}

stage="$(mktemp -d)"

build_example get-started/hello_world /ref/SHA256SUMS.inputs
merge flash_hello.bin partition_table/partition-table.bin hello_world.bin
symbols bootloader/bootloader hello_world

# The example that keeps a restart counter in NVS: the same bootloader and the same single-app
# table (its sums pin them again), once as built and once under the stock partition table of the
# X4, which has `otadata` and two application slots. Neither the bootloader nor the application
# embeds the table, so the second image needs no second build.
build_example storage/nvs/nvs_rw_value_cxx /ref/SHA256SUMS.inputs-nvs
merge flash_nvs.bin partition_table/partition-table.bin nvs_rw_value_cxx.bin
merge flash_nvs_ota.bin /partitions/x4-stock.bin nvs_rw_value_cxx.bin
symbols nvs_rw_value_cxx

if ! (cd "$stage" && sha256sum -c /ref/SHA256SUMS.output); then
  (cd "$stage" && sha256sum flash_hello.bin flash_nvs.bin flash_nvs_ota.bin) >&2
  tail -n 20 /tmp/merge-*.log >&2
  echo "error: a merged image is not the pinned one (hashes above); nothing was written to the output directory" >&2
  exit 1
fi
results=(flash_hello.bin flash_nvs.bin flash_nvs_ota.bin bootloader.nm hello_world.nm nvs_rw_value_cxx.nm)
(cd "$stage" && chown "$HOST_UID:$HOST_GID" "${results[@]}" && mv "${results[@]}" /out/)
