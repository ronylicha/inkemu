# Reference firmware of the SP1b tests

`build.sh [output-dir]` builds, inside the pinned `espressif/idf` Docker image, the stock ESP-IDF
2nd-stage bootloader and the `hello_world` example for the ESP32-C3, checks the three binaries
against `SHA256SUMS.inputs`, merges them into a 16 MB flash image and checks it against
`SHA256SUMS.output`. The ESP-IDF version is `ESP_IDF_VERSION` in `tools/conformance/versions.env`.

Nothing built here is committed and nothing is executed on the host: the image is data for inkemu.
`bootloader.nm` and `hello_world.nm` are symbol tables (text) that turn a program counter printed by
inkemu into a function name: the last symbol whose address is not above the PC.

Run the tests that need the image:

    tools/reference-firmware/build.sh
    cargo run --release -p emu-cli -- rom fetch --all --rom-dir /tmp/inkemu-rom
    INKEMU_ROM_ELF_DIR=/tmp/inkemu-rom INKEMU_REF_FLASH=target/reference-firmware/flash_hello.bin \
      cargo test --release -p soc-esp32c3 --test reference_boot -- --ignored

The build is reproducible (same hashes from two clean builds and from another path): see
`docs/research/sp1b-test-oracle.md`, section 2. If the hashes differ, the image, the ESP-IDF version
or `sdkconfig.defaults` changed: fix that, never the pinned values.
