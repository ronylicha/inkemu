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

# Builds the ESP-IDF project in the directory "$1" (an example under $IDF_PATH/examples, or a
# reference firmware of this directory), unmodified, checks its three binaries against the pinned
# hashes of "$2", and leaves the shell in its build directory. It says how long the build took: the
# cost of each project in a CI job is read in the job log.
#
# The configuration is the reference one, then the project's own sdkconfig.defaults when it ships
# one: ESP-IDF reads the list in order, a later file overriding an earlier one, so the project
# keeps every setting it chose and the reference settings fill in the rest. The project's file is
# never replaced: what is built is the project as it is configured, on top of the reference. A
# project without a file of its own gets the reference file copied in, as before this rule existed,
# so that the builds pinned before it do not change.
#
# A project whose sums file "$2" does not exist yet is built, its hashes printed and its name added
# to `unpinned`; the script goes on with the others and stops before handing anything over: that is
# how a new reference firmware is pinned, every new one in the same run (README.md, "Adding a
# firmware").
unpinned=()
build_example() {
  local source="$1" sums="$2"
  local name project started="$SECONDS"
  name="$(basename "$source")"
  project="/tmp/$name"
  rm -rf "$project"
  cp -r "$source" "$project"
  cd "$project"
  local defaults=()
  if [ -e sdkconfig.defaults ]; then
    defaults=(-D "SDKCONFIG_DEFAULTS=/ref/sdkconfig.defaults;$project/sdkconfig.defaults")
  else
    cp /ref/sdkconfig.defaults sdkconfig.defaults
  fi
  step "idf.py set-target ($name)" "/tmp/set-target-$name.log" idf.py "${defaults[@]}" set-target esp32c3
  step "idf.py build ($name)" "/tmp/build-$name.log" idf.py "${defaults[@]}" build
  cd build
  local binaries=(bootloader/bootloader.bin partition_table/partition-table.bin "$name.bin")
  if [ ! -e "$sums" ]; then
    echo "unpinned $(basename "$sums"):"
    sha256sum "${binaries[@]}"
    unpinned+=("$name")
    return 0
  fi
  if ! sha256sum -c "$sums"; then
    sha256sum "${binaries[@]}" >&2
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

build_example "$IDF_PATH/examples/get-started/hello_world" /ref/SHA256SUMS.inputs
merge flash_hello.bin partition_table/partition-table.bin hello_world.bin
symbols bootloader/bootloader hello_world

# The example that keeps a restart counter in NVS: the same bootloader and the same single-app
# table (its sums pin them again), once as built and once under the stock partition table of the
# X4, which has `otadata` and two application slots. Neither the bootloader nor the application
# embeds the table, so the second image needs no second build.
build_example "$IDF_PATH/examples/storage/nvs/nvs_rw_value_cxx" /ref/SHA256SUMS.inputs-nvs
merge flash_nvs.bin partition_table/partition-table.bin nvs_rw_value_cxx.bin
merge flash_nvs_ota.bin /partitions/x4-stock.bin nvs_rw_value_cxx.bin
symbols nvs_rw_value_cxx

# The firmwares of the serial consoles. Their tests boot the application alone, behind the
# bootloader inkemu ships (`inkemu run --firmware`): only the application leaves the container.
# uart_select reads UART0 through the driver and select(); usb_serial_jtag_echo, which ships a
# sdkconfig.defaults of its own, echoes what the host sends to USB-Serial-JTAG; the third is this
# directory's own, a console on USB-Serial-JTAG that needs a host reading the port. The last two are
# this directory's too: adc_wait_panic waits for the SAR ADC with interrupts masked, as CrossPoint
# does, until the interrupt watchdog panics, and its next life finds the core dump the panic wrote;
# deep_sleep_timer counts its lives in RTC memory through three deep sleeps on the RTC timer and a
# fourth with no wake source. The last one, coremark_example, is Espressif's CoreMark example and
# component, copied unmodified into this directory (coremark_example/PROVENANCE.md): the build has no
# network and no component manager. Its copied files are checked first, against their pinned hashes;
# the CoreMark run rules ask for the benchmark's files unmodified.
if ! (cd /ref/coremark_example && sha256sum --quiet -c /ref/SHA256SUMS.coremark-sources); then
  echo "error: a file of coremark_example is not the copy of espressif/coremark 1.1.0~2 its hash pins (see above); restore it from the release (coremark_example/PROVENANCE.md), never edit the sums" >&2
  exit 1
fi
applications=()
for project in \
  "$IDF_PATH/examples/peripherals/uart/uart_select:SHA256SUMS.inputs-uart-select" \
  "$IDF_PATH/examples/peripherals/usb_serial_jtag/usb_serial_jtag_echo:SHA256SUMS.inputs-usj-echo" \
  "/ref/usb_serial_jtag_console:SHA256SUMS.inputs-usj-console" \
  "/ref/adc_wait_panic:SHA256SUMS.inputs-adc-wait-panic" \
  "/ref/deep_sleep_timer:SHA256SUMS.inputs-deep-sleep-timer" \
  "/ref/coremark_example:SHA256SUMS.inputs-coremark"; do
  source="${project%%:*}"
  name="$(basename "$source")"
  build_example "$source" "/ref/${project##*:}"
  cp "$name.bin" "$stage/$name.bin"
  symbols "$name"
  applications+=("$name.bin" "$name.nm")
done

if [ "${#unpinned[@]}" -ne 0 ]; then
  echo "error: ${unpinned[*]}: no pinned hashes yet (the lines after \"unpinned\" above give those of this build); build a second time from a clean cache, and when both builds give the same hashes write each block to the file it names, in tools/reference-firmware/; nothing was written to the output directory" >&2
  exit 1
fi

if ! (cd "$stage" && sha256sum -c /ref/SHA256SUMS.output); then
  (cd "$stage" && sha256sum flash_hello.bin flash_nvs.bin flash_nvs_ota.bin) >&2
  tail -n 20 /tmp/merge-*.log >&2
  echo "error: a merged image is not the pinned one (hashes above); nothing was written to the output directory" >&2
  exit 1
fi
results=(flash_hello.bin flash_nvs.bin flash_nvs_ota.bin bootloader.nm hello_world.nm nvs_rw_value_cxx.nm "${applications[@]}")
(cd "$stage" && chown "$HOST_UID:$HOST_GID" "${results[@]}" && mv "${results[@]}" /out/)
