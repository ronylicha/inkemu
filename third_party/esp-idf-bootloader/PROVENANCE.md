# Provenance

| File | Size | SHA-256 |
|---|---|---|
| `esp32c3/bootloader.bin` | 21,040 bytes | `601b48456b0a9d5c9f1beac166135e06686d3c5ed0309dd16840ab39887afaa3` |

`esp32c3/bootloader.bin` is the 2nd-stage bootloader that `tools/reference-firmware/build.sh`
builds, byte for byte (the build checks it against the same SHA-256, `SHA256SUMS.inputs`, and the
CI job `reference-boot` compares this file with the bootloader of the merged image it builds).

| Item | Value |
|---|---|
| Build image | `espressif/idf:v5.5.2` (Docker Hub) |
| ESP-IDF | tag `v5.5.2`, commit `30aaf64524299d3bde422ca9a2848090d1bc5d0f` |
| Toolchain | `riscv32-esp-elf-gcc` `esp-14.2.0_20251107` (GCC 14.2.0), from the image |
| Project | `$IDF_PATH/examples/get-started/hello_world`, unmodified; the bootloader does not depend on the application |
| Command | `idf.py set-target esp32c3 && idf.py build`, in the container, network off |
| Output | `build/bootloader/bootloader.bin` |

The configuration, `tools/reference-firmware/sdkconfig.defaults`, is the only input that is not
ESP-IDF's own:

```
CONFIG_IDF_TARGET="esp32c3"
CONFIG_ESPTOOLPY_FLASHSIZE_16MB=y
CONFIG_ESPTOOLPY_FLASHMODE_DIO=y
CONFIG_ESPTOOLPY_FLASHFREQ_80M=y
CONFIG_ESP_DEFAULT_CPU_FREQ_MHZ_160=y
CONFIG_APP_REPRODUCIBLE_BUILD=y
```

Every other option keeps the default of ESP-IDF v5.5.2: log level INFO, no application rollback,
minimum chip revision v0.3, no secure boot, no flash encryption. The build is reproducible: clean
builds, a build from another path and a build with the compiler cache off give the same file
(`docs/research/sp1b-test-oracle.md`, section 2; `docs/research/shipped-bootloader-and-firmware-composition.md`,
section D).

**Unmodified ESP-IDF sources, configuration only.** No source file was changed.

**What inkemu changes when it writes the file into a flash.** For a device whose flash is not
16 MiB, inkemu writes the image with the flash-size nibble of its header (byte 3, high nibble) set
to the size of that flash and its appended SHA-256 computed again, as `esptool.py` does to any image
written at offset 0 (`esptool/cmds.py` of esptool v4.11). That is a modification of the object
form (Apache License 2.0, section 4 b). This file is never changed: the rewrite happens in memory,
in `firmware::bootloader::ShippedBootloader::for_flash`.
