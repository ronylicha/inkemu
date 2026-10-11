#!/usr/bin/env bash
# Builds the reference flash image of the SP1b tests: the stock ESP-IDF 2nd-stage bootloader and the
# hello_world example, merged into a 16 MB image. Everything third-party runs inside Docker; nothing
# built here is committed. The three binaries and the merged image are checked against pinned
# SHA-256 values (docs/research/sp1b-test-oracle.md, section 2): a mismatch stops the script.
#
# Usage: tools/reference-firmware/build.sh [output-dir]      (default: target/reference-firmware)
set -euo pipefail
cd "$(dirname "$0")/../.."
source tools/conformance/versions.env
out="${1:-target/reference-firmware}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
docker build -q -t inkemu/esp-idf --build-arg ESP_IDF_VERSION="$ESP_IDF_VERSION" tools/docker/esp-idf > /dev/null
# No network: the example has no managed component, and the build must not depend on anything that
# is not in the pinned image. The container builds as its own user in its own file system (the
# conditions the pinned hashes were measured in) and hands over the results at the end.
docker run --rm --network none \
  -e IDF_COMPONENT_MANAGER=0 -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
  -v "$PWD/tools/reference-firmware:/ref:ro" -v "$out:/out" \
  inkemu/esp-idf bash /ref/in-container.sh
echo "$out/flash_hello.bin"
