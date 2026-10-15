# Reference firmware of the SP1b tests

`build.sh [output-dir]` builds, inside the pinned `espressif/idf` Docker image and in one container
run, the stock ESP-IDF 2nd-stage bootloader and four unmodified ESP-IDF examples for the ESP32-C3,
three applications of this directory, and Espressif's CoreMark example (copied into this directory,
unmodified), checks the binaries of each build against its pinned
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

Six applications are built next to them and leave the container alone, without a merged image:
the tests of the serial consoles (`crates/console-script/tests/reference_firmware.rs`) and of the
`inkemu` command (`crates/emu-cli/tests/run_cli.rs`) boot them behind the bootloader inkemu ships,
as `inkemu run --firmware` does.

| Application | Source | What its tests do with it | Pins of its build |
|---|---|---|---|
| `uart_select.bin` | `examples/peripherals/uart/uart_select`: UART0 through the driver, `select()` with a 5 s timeout, one log line per byte read | type `a`, then `bc\r\n` | `SHA256SUMS.inputs-uart-select` |
| `usb_serial_jtag_echo.bin` | `examples/peripherals/usb_serial_jtag/usb_serial_jtag_echo`: what the host sends to USB-Serial-JTAG is echoed there and dumped on UART0 | type a line, then 90 bytes | `SHA256SUMS.inputs-usj-echo` |
| `usb_serial_jtag_console.bin` | `usb_serial_jtag_console/` of this directory, ours (Apache-2.0): USB-Serial-JTAG is the console, each line read is echoed; it needs a USB host that reads the port | type `hello\r` | `SHA256SUMS.inputs-usj-console` |
| `adc_wait_panic.bin` | `adc_wait_panic/` of this directory, ours (Apache-2.0): it reads a register of I2C0, starts a conversion of the SAR ADC and waits for its end with interrupts masked, as CrossPoint 1.6.5 does, until the interrupt watchdog panics; the panic writes a core dump to the `coredump` partition, and the next life says it found it | read its two lives, its warnings and its trace hash | `SHA256SUMS.inputs-adc-wait-panic` |
| `deep_sleep_timer.bin` | `deep_sleep_timer/` of this directory, ours (Apache-2.0): it counts its lives in RTC memory (`RTC_DATA_ATTR`) and prints, at each, its reset reason, its wake cause, the runs of its own wake stub (`RTC_IRAM_ATTR esp_wake_deep_sleep`, which the ROM calls only when the CRC of the RTC fast memory matches) and the milliseconds it slept; lives 1 to 3 sleep 2 s on the RTC timer, life 4 sleeps with no wake source | read its four lives, the three wakes (`rst:0x5 (DSLEEP)`), the stub run once per wake, the end of the run with the device asleep, and its trace hash | `SHA256SUMS.inputs-deep-sleep-timer` |
| `coremark_example.bin` | `coremark_example/` of this directory: Espressif's `espressif/coremark` 1.1.0~2, its example and its component (the EEMBC sources, Apache-2.0 with the CoreMark Acceptable Use Agreement; Espressif's port, Apache-2.0; the example, CC0-1.0), copied unmodified (`coremark_example/PROVENANCE.md`); CoreMark's sources at `-O3 -fjump-tables -ftree-switch-conversion`, its code in IRAM, timed by `esp_timer` | read the CRCs of the 2K performance run, `Correct operation validated`, the score (within 15 % of the 483.27 the ESP32-C3 datasheet publishes: criterion 5, ADR 0009) and its trace hash | `SHA256SUMS.inputs-coremark` |

The examples are built as ESP-IDF ships them. Their configuration is `sdkconfig.defaults` of this
directory, then the example's own `sdkconfig.defaults` when it ships one (`usb_serial_jtag_echo`
does: `CONFIG_ESP_CONSOLE_SECONDARY_NONE=y`), merged by ESP-IDF in that order, so that the example
keeps every setting it chose. The example's file is never replaced. `usb_serial_jtag_console` is
configured the same way, its own file adding `CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y`, and so is
`adc_wait_panic`, whose file adds the stock partition table of the X4 (`partitions.csv`, a copy of
`tests/fixtures/partitions/x4-stock.csv`) and `CONFIG_ESP_COREDUMP_ENABLE_TO_FLASH=y`.
`deep_sleep_timer` ships no file of its own: it is built with the reference configuration alone, in
which `CONFIG_ESP_SYSTEM_ALLOW_RTC_FAST_MEM_AS_HEAP` keeps its default (`y`), so that ESP-IDF enters
deep sleep through `rtc_deep_sleep_start` and the CRC unit of the RTC fast memory.
`coremark_example` is configured the same way, its own file being the example's
(`CONFIG_ESP_TASK_WDT_INIT=n`). The reference build has no network and no component manager: the
example asks the manager for `espressif/coremark`, so the component is copied into
`coremark_example/components/coremark/`, where ESP-IDF finds it under the name the example requires.
Every copied file is pinned in `SHA256SUMS.coremark-sources` (the hashes of the registry's
`CHECKSUMS.json` for the release), and `in-container.sh` checks them before it builds, together with
the list of the directory's entries, which must be the regular files of the sums plus
`PROVENANCE.md` (a file or a symbolic link added stops the build): CoreMark's run rules ask for its files unmodified. CoreMark is a trademark of EEMBC; what the firmware prints under
inkemu is the output of an emulated chip, not a result of a device.

The firmwares of real units (CrossPoint 1.6.5 and the stock firmware of the X4) are not built here:
the user who has them runs their tests, `crates/emu-cli/tests/real_firmware.rs`, whose header says
how. They run nowhere in the CI.

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
      INKEMU_USJ_CONSOLE_APP="$PWD/target/reference-firmware/usb_serial_jtag_console.bin" \
      INKEMU_ADC_WAIT_PANIC_APP="$PWD/target/reference-firmware/adc_wait_panic.bin" \
      INKEMU_DEEP_SLEEP_TIMER_APP="$PWD/target/reference-firmware/deep_sleep_timer.bin" \
      INKEMU_COREMARK_APP="$PWD/target/reference-firmware/coremark_example.bin"
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

CoreMark is an application already; it prints its score after about 17.5 s of device time
(about 2.2 billion instructions; the test that runs it took 68 s in the CI on 2026-10-15, and 87 to
102 s on the development host):

    cargo run --release -p emu-cli -- run --device x4 --console uart0 --max-time 20 \
      --firmware target/reference-firmware/coremark_example.bin

The bootloader of this build is the one inkemu ships (`third_party/esp-idf-bootloader/`): a test
compares the first bytes of `flash_hello.bin` with it.

The paths are absolute because each test binary runs in the directory of its own crate. Every test
checks the SHA-256 of the image it is given before it runs it, and fails by naming the variable or
the file when the image is missing or is not the pinned one.

The builds are reproducible (same hashes from clean builds, from another path and with the compiler
cache off): see `docs/research/sp1b-test-oracle.md`, section 2, and
`docs/research/esp-idf-storage-flash-traffic.md`, section A.2. If the hashes differ, the image, the
ESP-IDF version or `sdkconfig.defaults` changed: fix that, never the pinned values.
