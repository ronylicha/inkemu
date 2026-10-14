#!/usr/bin/env bash
# Builds the reference flash images of the SP1b tests, each a 16 MB image whose bootloader is the
# stock ESP-IDF 2nd-stage bootloader:
#   flash_hello.bin    the hello_world example;
#   flash_nvs.bin      the nvs_rw_value_cxx example, which keeps a restart counter in NVS;
#   flash_nvs_ota.bin  the same application under the stock partition table of the X4
#                      (tests/fixtures/partitions/x4-stock.bin), whose bootloader writes otadata;
# and four applications alone, which the tests boot behind the bootloader inkemu ships:
# uart_select.bin, usb_serial_jtag_echo.bin and usb_serial_jtag_console.bin (the serial consoles),
# and adc_wait_panic.bin (the wait for the SAR ADC that stops CrossPoint, its panic and core dump).
# Everything third-party runs inside Docker, in one container run. One artefact of this build is
# committed: its bootloader, the one inkemu ships (third_party/esp-idf-bootloader/, the same bytes);
# nothing else built here is.
# The binaries and the merged images are checked against pinned SHA-256 values
# (docs/research/sp1b-test-oracle.md, section 2; docs/research/esp-idf-storage-flash-traffic.md,
# sections A and G): a mismatch stops the script.
#
# Usage: tools/reference-firmware/build.sh [output-dir]      (default: target/reference-firmware)
set -euo pipefail
cd "$(dirname "$0")/../.."
source tools/conformance/versions.env
out="${1:-target/reference-firmware}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
docker build -q -t inkemu/esp-idf --build-arg ESP_IDF_VERSION="$ESP_IDF_VERSION" tools/docker/esp-idf > /dev/null
# No network: the examples have no managed component, and the build must not depend on anything
# that is not in the pinned image. The container builds as its own user in its own file system (the
# conditions the pinned hashes were measured in) and hands over the results at the end.
docker run --rm --network none \
  -e IDF_COMPONENT_MANAGER=0 -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
  -v "$PWD/tools/reference-firmware:/ref:ro" \
  -v "$PWD/tests/fixtures/partitions:/partitions:ro" \
  -v "$out:/out" \
  inkemu/esp-idf bash /ref/in-container.sh
echo "$out/flash_hello.bin"
echo "$out/flash_nvs.bin"
echo "$out/flash_nvs_ota.bin"
echo "$out/uart_select.bin"
echo "$out/usb_serial_jtag_echo.bin"
echo "$out/usb_serial_jtag_console.bin"
echo "$out/adc_wait_panic.bin"
