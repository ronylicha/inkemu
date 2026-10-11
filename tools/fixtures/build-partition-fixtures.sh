#!/usr/bin/env bash
# Regenerates partition-table reference binaries with ESP-IDF's own gen_esp32part.py.
set -euo pipefail
cd "$(dirname "$0")/../.."
source tools/conformance/versions.env
docker build -q -t inkemu/esp-idf --build-arg ESP_IDF_VERSION="$ESP_IDF_VERSION" tools/docker/esp-idf
for csv in tests/fixtures/partitions/*.csv; do
  docker run --rm -u "$(id -u):$(id -g)" -v "$PWD:/w" inkemu/esp-idf \
    python /opt/esp/idf/components/partition_table/gen_esp32part.py --flash-size 16MB "/w/$csv" "/w/${csv%.csv}.bin"
done
