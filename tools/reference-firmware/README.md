# Reference firmware of the SP1b tests

`build.sh [output-dir]` builds, inside the pinned `espressif/idf` Docker image and in one container
run, the stock ESP-IDF 2nd-stage bootloader and two unmodified ESP-IDF examples for the ESP32-C3,
checks the binaries of each build against its pinned hashes, merges them into 16 MB flash images
and checks the images against `SHA256SUMS.output`. The ESP-IDF version is `ESP_IDF_VERSION` in
`tools/conformance/versions.env`.

| Image | Application | Partition table | Pins of its build |
|---|---|---|---|
| `flash_hello.bin` | `examples/get-started/hello_world` | the single-app table of the build | `SHA256SUMS.inputs` |
| `flash_nvs.bin` | `examples/storage/nvs/nvs_rw_value_cxx`: it keeps a restart counter in NVS, prints it, and restarts every eleven seconds | the single-app table of the build | `SHA256SUMS.inputs-nvs` |
| `flash_nvs_ota.bin` | the same application | `tests/fixtures/partitions/x4-stock.bin`, the stock table of the X4: `otadata` and two application slots, so the bootloader writes `otadata` on the first boot | `SHA256SUMS.inputs-nvs` |

Nothing built here is committed and nothing is executed on the host: the images are data for inkemu.
`bootloader.nm`, `hello_world.nm` and `nvs_rw_value_cxx.nm` are symbol tables (text) that turn a
program counter printed by inkemu into a function name: the last symbol whose address is not above
the PC.

The examples are built as ESP-IDF ships them, with `sdkconfig.defaults` of this directory as their
only configuration. An example that ships a `sdkconfig.defaults` of its own stops the script before
anything is built: its file would be replaced, and the build would not be the unmodified example.

Run the tests that need the images, those of the machine and those of the `inkemu` command:

    tools/reference-firmware/build.sh
    cargo run --release -p emu-cli -- rom fetch --all --rom-dir /tmp/inkemu-rom
    export INKEMU_ROM_ELF_DIR=/tmp/inkemu-rom \
      INKEMU_REF_FLASH="$PWD/target/reference-firmware/flash_hello.bin" \
      INKEMU_NVS_FLASH="$PWD/target/reference-firmware/flash_nvs.bin" \
      INKEMU_NVS_OTA_FLASH="$PWD/target/reference-firmware/flash_nvs_ota.bin"
    cargo test --release -p soc-esp32c3 --test reference_boot -- --ignored
    cargo test --release -p emu-cli --test run_cli reference_firmware -- --ignored

The paths are absolute because each test binary runs in the directory of its own crate. Every test
checks the SHA-256 of the image it is given before it runs it, and fails by naming the variable or
the file when the image is missing or is not the pinned one.

The builds are reproducible (same hashes from clean builds, from another path and with the compiler
cache off): see `docs/research/sp1b-test-oracle.md`, section 2, and
`docs/research/esp-idf-storage-flash-traffic.md`, section A.2. If the hashes differ, the image, the
ESP-IDF version or `sdkconfig.defaults` changed: fix that, never the pinned values.
