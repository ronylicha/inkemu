# Reference firmware of the SP1b tests

`build.sh [output-dir]` builds, inside the pinned `espressif/idf` Docker image and in one container
run, the stock ESP-IDF 2nd-stage bootloader and four unmodified ESP-IDF examples for the ESP32-C3,
and one application of this directory, checks the binaries of each build against its pinned
hashes, merges the first two into 16 MB flash images and checks the images against
`SHA256SUMS.output`. The ESP-IDF version is `ESP_IDF_VERSION` in
`tools/conformance/versions.env`.

| Image | Application | Partition table | Pins of its build |
|---|---|---|---|
| `flash_hello.bin` | `examples/get-started/hello_world` | the single-app table of the build | `SHA256SUMS.inputs` |
| `flash_nvs.bin` | `examples/storage/nvs/nvs_rw_value_cxx`: it keeps a restart counter in NVS, prints it, and restarts every eleven seconds | the single-app table of the build | `SHA256SUMS.inputs-nvs` |
| `flash_nvs_ota.bin` | the same application | `tests/fixtures/partitions/x4-stock.bin`, the stock table of the X4: `otadata` and two application slots, so the bootloader writes `otadata` on the first boot | `SHA256SUMS.inputs-nvs` |

One artefact of this build is committed: its bootloader, the one inkemu ships, under
`third_party/esp-idf-bootloader/` (the same bytes, checked by the CI). Nothing else built here is
committed, and nothing is executed on the host: the images are data for inkemu.
`bootloader.nm`, `hello_world.nm` and `nvs_rw_value_cxx.nm` are symbol tables (text) that turn a
program counter printed by inkemu into a function name: the last symbol whose address is not above
the PC.

Three applications are built next to them and leave the container alone, without a merged image:
the tests of the serial consoles (`crates/console-script/tests/reference_firmware.rs`) boot them
behind the bootloader inkemu ships, as `inkemu run --firmware` does.

| Application | Source | What its tests type into it | Pins of its build |
|---|---|---|---|
| `uart_select.bin` | `examples/peripherals/uart/uart_select`: UART0 through the driver, `select()` with a 5 s timeout, one log line per byte read | `a`, then `bc\r\n` | `SHA256SUMS.inputs-uart-select` |
| `usb_serial_jtag_echo.bin` | `examples/peripherals/usb_serial_jtag/usb_serial_jtag_echo`: what the host sends to USB-Serial-JTAG is echoed there and dumped on UART0 | a line, then 90 bytes | `SHA256SUMS.inputs-usj-echo` |
| `usb_serial_jtag_console.bin` | `usb_serial_jtag_console/` of this directory, ours (Apache-2.0): USB-Serial-JTAG is the console, each line read is echoed; it needs a USB host that reads the port | `hello\r` | `SHA256SUMS.inputs-usj-console` |

The examples are built as ESP-IDF ships them. Their configuration is `sdkconfig.defaults` of this
directory, then the example's own `sdkconfig.defaults` when it ships one (`usb_serial_jtag_echo`
does: `CONFIG_ESP_CONSOLE_SECONDARY_NONE=y`), merged by ESP-IDF in that order, so that the example
keeps every setting it chose. The example's file is never replaced. `usb_serial_jtag_console` is
configured the same way, its own file adding `CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y`.

## Adding a firmware

A project whose sums file does not exist yet is built, its hashes are printed after a line
`unpinned <file>:`, and the script stops before anything reaches the output directory. Pin a new
firmware from two clean builds: run the build (the CI job `reference-boot` does when its cache key
changes), copy the three lines printed under `unpinned <file>:` into `<file>` here, and commit: the
next build is the second one, and fails if it differs from the first.

Run the tests that need the images, those of the machine and those of the `inkemu` command:

    tools/reference-firmware/build.sh
    cargo run --release -p emu-cli -- rom fetch --all --rom-dir /tmp/inkemu-rom
    export INKEMU_ROM_ELF_DIR=/tmp/inkemu-rom \
      INKEMU_REF_FLASH="$PWD/target/reference-firmware/flash_hello.bin" \
      INKEMU_NVS_FLASH="$PWD/target/reference-firmware/flash_nvs.bin" \
      INKEMU_NVS_OTA_FLASH="$PWD/target/reference-firmware/flash_nvs_ota.bin" \
      INKEMU_UART_SELECT_APP="$PWD/target/reference-firmware/uart_select.bin" \
      INKEMU_USJ_ECHO_APP="$PWD/target/reference-firmware/usb_serial_jtag_echo.bin" \
      INKEMU_USJ_CONSOLE_APP="$PWD/target/reference-firmware/usb_serial_jtag_console.bin"
    cargo test --release -p soc-esp32c3 --test reference_boot -- --ignored
    cargo test --release -p emu-cli --test run_cli reference_firmware -- --ignored
    cargo test --release -p console-script --test reference_firmware -- --ignored

The tests of `inkemu run --firmware` boot the application images alone: they take them out of the
merged images, from 0x10000, and check them against `SHA256SUMS.inputs` and `SHA256SUMS.inputs-nvs`
before they run them. To boot one by hand, cut it out the same way (`hello_world.bin` is the 136,160
bytes from 65,536 of `flash_hello.bin`, `nvs_rw_value_cxx.bin` the 161,312 bytes from 65,536 of
`flash_nvs.bin`):

    dd if=target/reference-firmware/flash_hello.bin of=hello_world.bin bs=4096 skip=65536 count=136160 iflag=skip_bytes,count_bytes
    cargo run --release -p emu-cli -- run --device x4 --firmware hello_world.bin --max-time 12

The bootloader of this build is the one inkemu ships (`third_party/esp-idf-bootloader/`): a test
compares the first bytes of `flash_hello.bin` with it.

The paths are absolute because each test binary runs in the directory of its own crate. Every test
checks the SHA-256 of the image it is given before it runs it, and fails by naming the variable or
the file when the image is missing or is not the pinned one.

The builds are reproducible (same hashes from clean builds, from another path and with the compiler
cache off): see `docs/research/sp1b-test-oracle.md`, section 2, and
`docs/research/esp-idf-storage-flash-traffic.md`, section A.2. If the hashes differ, the image, the
ESP-IDF version or `sdkconfig.defaults` changed: fix that, never the pinned values.
