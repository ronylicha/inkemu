# Shipping the 2nd-stage bootloader with inkemu, and composing a flash image from an application `.bin`

Scope: the facts SP1b-3c (`--firmware`) needs to ship the stock ESP-IDF 2nd-stage bootloader inside inkemu, chosen from the device profile and replaceable by the user: (A) what is linked into it and under which licences; (B) which header fields matter and how many binaries the profiles need; (C) what it refuses, and CrossPoint 1.6.5 behind it on inkemu; (D) which configuration to ship; (E) the ESP32-S3; (F) partition tables per profile; (G) checks on a user-supplied bootloader.
The decision to ship it (embedded, overridable, selected by profile) is the owner's and is not reopened. Date of research: 2026-10-12/13, `main` at `34e39f9`. No code was changed; this file is the only repository edit.
Already established and not repeated: `flash-writes-and-crosspoint-boot.md` (**FW**) §H–§O, `sp1b-test-oracle.md` (**TO**) §2 and §4, `esp32c3-rom-flash-boot.md` (**RFB**) §F, ADR 0001, `2026-10-12-sp1b-3b-carryover.md`.

## 0. Method, sources, tags

| Tag | Meaning |
|---|---|
| **M** (MEASURED) | produced for this report: a build, a map file, a byte comparison, or an inkemu run |
| **R** (READ) | read in a source file (`file:line`) or a cloned repository |
| **A** (ASSUMED) | inferred; every A item that carries risk is repeated in §H |

| Source | What | Where |
|---|---|---|
| `IDF:` | ESP-IDF `v5.5.2` (`git describe`), commit `30aaf64524299d3bde422ca9a2848090d1bc5d0f`, in `espressif/idf:v5.5.2`; paths relative to `components/` unless they start with `tools/` or `docs/` | read inside the container |
| `TC:` | `riscv32-esp-elf` / `xtensa-esp-elf` GCC `esp-14.2.0_20251107` of the same image | idem |
| `esptool` | v4.11.dev1, `site-packages/esptool/cmds.py` of the image | idem |
| `repos/` | the shallow clones of FW §0 (`crosspoint-reader`, `crosspoint-tools`, `xteink-flasher`), read only | scratch `repos/` |
| Builds | `examples/get-started/hello_world`, `idf.py set-target <t> && idf.py bootloader`, with `tools/reference-firmware/sdkconfig.defaults` or a variant of it (§I) | scratch `r6/out/<tag>/` |
| inkemu runs | `target/release/inkemu` built from `34e39f9`, profile `x4`, ROM rev3 from the cache, tolerant mode | scratch `r6/out/cp_*.{out,err}` |

One container (`inkemu-r6`, `--network none`), removed at the end. The CrossPoint binary was only read as data and run under inkemu.

## 1. Key results

1. **No licence blocker.** The 21,040-byte reference bootloader (`601b4845…afaa3`, rebuilt here bit for bit) contains code from 30 ESP-IDF objects in 8 archives, all Apache-2.0 sources; **no newlib or libgcc byte is linked**: every libc/libgcc symbol it calls (`memcpy`, `memset`, `memcmp`, `__udivdi3`, …) resolves to the mask ROM through the ROM linker scripts **(M)**. Headers compiled into it add BSD-3-Clause macros (`riscv/csr.h`, `riscv/encoding.h`, newlib `sys/cdefs.h`) and GCC headers under the GCC Runtime Library Exception. Nothing GPL/LGPL reaches the binary.
2. ESP-IDF ships **no top-level `NOTICE`** file **(M)**. What inkemu must carry: the Apache-2.0 text, the Espressif copyright line, the BSD-3-Clause notice of the two RISC-V headers, provenance and the statement that only the configuration differs from upstream (§A.3), in the repository and with every distributed binary — the ADR 0001 pattern (`third_party/<name>/{LICENSE,NOTICE,PROVENANCE.md}` + a line in the root `NOTICE`).
3. **The bootloader reads the flash size from its own header** (`bootloader_flash_config_esp32c3.c:123-156, 280`) and hands it to the application, which **aborts** if the chip it detects is smaller (`esp_flash_spi_init.c:580-583`). Builds for 4 MB and 16 MB differ **only** in header byte 3 and the appended SHA-256: rewriting byte 3 of the 16 MB binary and recomputing the digest — what esptool does to any image written at offset 0 (`esptool/cmds.py:263-345`) — gives the 4 MB build byte for byte (`d6421484…109c5`) **(M)**.
4. Hence **one bootloader binary per chip family and flash-mode family** (DIO for every Xteink file seen), not per flash size. A QIO build is different code (22,096 bytes) whose header still says DIO **(M)**. The profile needs to state nothing new to pick it: `soc.family` selects the binary, `flash.size_mib` the header nibble.
5. The reference bootloader requires **chip revision ≥ v0.3** (`min_rev_full = 3`, Kconfig default `ESP32C3_REV_MIN_3`); below that `bootloader_init` fails and the bootloader resets the system **for ever** (`bootloader_start.c:32-34`, `bootloader_utility.c:1160-1166`) **(R)**. Its maximum (v1.99) is not checked for the bootloader itself (`bootloader_common_loader.c:132-137`). inkemu's eFuse model reports v0.3 and eFuse block v1.2 for both profiles **(M)**. A min-rev-0 build differs only in header bytes 14–15 and the digest **(M)**.
6. **CrossPoint 1.6.5 boots on inkemu at `main` behind the stock bootloader**, unmodified flash image of FW §K.1: 27 bootloader lines, `Set actual ota_seq=1 in otadata[0]`, the application's Arduino error lines, then the same interrupt-watchdog panic in the SAR ADC wait as under QEMU (`MEPC 0x4219b962`), reboot, second boot; at 3 s of device time it polls `SPI2.CMD` (228,377,462 instructions, 14 s of host time) **(M)**. With an ERROR-level, rollback-enabled or min-rev-0 build it reaches the same panic **(M)**.
7. **Ship the reference artefact itself** (INFO, no rollback, min rev v0.3): it is already pinned in CI, the golden log and 13 + 4 tests pin its INFO lines and trace, and a user who boots a third-party application sees what the bootloader decided. An ERROR/rollback build is also reproducible (two configurations measured, each rebuilt from another path with the compiler cache off) but would need its own pins and oracle; keep it as a possible second entry, not the default (§D).
8. **The S3 recipe works unchanged** with `CONFIG_IDF_TARGET="esp32s3"`: 20,992 bytes, `53765fcd…a023e`, reproducible, offset `0x0` like the C3 (esptool `BOOTLOADER_FLASH_OFFSET`), min rev v0.0 / max v0.99, licences of the same kinds plus MIT-style Tensilica/Cadence headers **(M)**. inkemu pins no S3 ROM yet (`rom_store.rs` manifest is C3-only, R).
9. **Partition tables**: X4 = the existing fixture (`bd0f7954…f6ce`); the X3 layout is enforced by `xteink-flasher`, which refuses to flash an X3 whose table does not match it (`useEspOperations.ts:72-100`): offsets and sizes are therefore those of real X3 units; names, flags and the MD5 entry remain assumed (§F).

---

## A. What is linked, and the licences

### A.1 Contents of the reference bootloader (M, `r6/out/ref/bootloader.map`, `mapcontrib.py`)

Input sections with a non-zero size in the allocated output sections (`.dram0.*`, `.iram.text`, `.iram_loader.text`), per archive (bytes include `.dram0.bss`, which is not in the file):

| Archive (ESP-IDF component) | Objects contributing | Bytes |
|---|---|---|
| `bootloader_support` | `bootloader_clock_init`, `_common_loader`, `_console`, `_console_loader`, `_esp32c3`, `_flash`, `_flash_config_esp32c3`, `_init`, `_mem`, `_panic`, `_random`, `_random_esp32c3`, `_sha`, `_soc`, `_utility`, `esp_image_format`, `flash_encrypt`, `flash_partitions` | 21,907 |
| `esp_hw_support` | `cpu_region_protect`, `rtc_clk`, `rtc_clk_init` | 2,643 |
| `hal` | `cache_hal`, `efuse_hal`, `mmu_hal`, `wdt_hal_iram` | 1,448 |
| `bootloader` (`subproject/main`, archive `libmain.a`) | `bootloader_start` | 179 |
| `esp_rom` | `esp_rom_sys` | 100 |
| `esp_bootloader_format` | `esp_bootloader_desc` | 90 |
| `log` | `log_timestamp` | 38 |
| `riscv` | `rv_utils` | 14 |

Pulled in by the archive scan but contributing **0 bytes** (discarded by `--gc-sections`): `efuse` (5 objects), and from the toolchain `libc.a` (`impure`, `memcmp`, `memcpy`, `memset`) and `libgcc.a` (`_ashldi3`, `_clz`, `_lshrdi3`, `_popcountsi2`, `_udivdi3`). In the ELF, `memcpy` = `0x40000358`, `memset` = `0x40000354`, `memcmp` = `0x40000360`, `__udivdi3` = `0x400008ac`, `_global_impure_ptr` = `0x3fcdffdc`: absolute (`A`) ROM symbols **(M, `riscv32-esp-elf-nm`)**. `micro-ecc` and `spi_flash` are on the link line and contribute nothing. Program headers: three `LOAD` segments, the three image segments **(M)**.
Linker scripts (`-T`): `bootloader.ld`, `bootloader.rom.ld` (one comment line), `esp32c3.peripherals.ld`, `esp32c3.rom{,.api,.bt_funcs,.libc,.libc-suboptimal_for_misaligned_mem,.libgcc,.newlib,.version}.ld`, `rom.api.ld`: address tables; those without an SPDX line are generated files ("THIS FILE WAS AUTOMATICALLY GENERATED") from the ESP-IDF tree.

### A.2 Licence of every file compiled into the contributing objects (M, `ninja -t deps`, first `SPDX-License-Identifier` line or header text)

267 files: 222 carry `Apache-2.0`; the 45 others:

| Files | Licence | Effect on a binary |
|---|---|---|
| `bootloader/subproject/main/bootloader_hooks.h`, `esp_rom/include/esp_rom_crc.h`, `soc/esp32c3/include/soc/uart_pins.h` | Apache-2.0 header text, Espressif, no SPDX line | none beyond Apache-2.0 |
| `riscv/include/riscv/csr.h` ("Copyright (c) 2013, The Regents of the University of California … 2018-2019, The libfemto authors … 2020 Espressif"), `riscv/include/riscv/encoding.h` ("2010-2017, The Regents of the University of California") | **BSD-3-Clause** text in the header | macros expanded into the binary (CSR access, constants): clause 2 asks binary redistributions to reproduce the notice — carry it (A that it applies to expanded macros; costless either way) |
| newlib headers of `TC:` (`string.h`, `sys/reent.h`, `machine/*`, …; `sys/cdefs.h` is SPDX `BSD-3-Clause`) | newlib: BSD-style, `IDF:newlib/COPYING.NEWLIB` | declarations and macros; no newlib code linked (A.1) |
| GCC headers (`stdarg.h`, `stdint.h`, `stddef.h`, `limits.h`, …) | GPL-3.0-or-later **with GCC Runtime Library Exception 3.1** (`stdarg.h:16-17`) | no obligation on the output of an eligible compilation |
| `config/sdkconfig.h`, `project_elf_src_esp32c3.c` | generated by the build | — |

**No GPL, LGPL or other non-permissive code is linked** (M for linkage, R for licences). The documentation page `docs/en/COPYRIGHT.rst:9` states: "All original source code in this repository is Copyright (C) 2015-2023 Espressif Systems … licensed under the Apache License 2.0"; its "ROM Source Code Copyrights" section concerns the mask ROM, which the bootloader calls but does not contain (ADR 0001 keeps the ROM out of the repository).

### A.3 What the repository and the distributed binary must carry

`find $IDF_PATH -maxdepth 3 -iname 'NOTICE*'` returns nothing (the only `NOTICE`-like files are per-component SBOM/COPYING files of components not linked here) **(M)**. Apache-2.0 §4 then asks, for redistribution of the object form: (a) a copy of the licence; (b) prominent notice of modified files — no source file is modified, the configuration is `sdkconfig.defaults` (A: a header rewrite for another flash size, §B, is a modification of the object; say it in the provenance); (c) retain attribution notices; (d) NOTICE — none upstream.
Following ADR 0001 (SVD: `third_party/espressif-svd/{LICENSE,NOTICE,PROVENANCE.md}` and a paragraph in the root `NOTICE`), a shipped bootloader needs:

| Item | Content |
|---|---|
| `third_party/esp-idf-bootloader/LICENSE` | Apache-2.0 text of ESP-IDF `v5.5.2` (`$IDF_PATH/LICENSE`) |
| `…/NOTICE` | "ESP-IDF 2nd-stage bootloader, Copyright Espressif Systems (Shanghai) Co., Ltd., Apache-2.0"; the BSD-3-Clause notice and disclaimer of `riscv/csr.h` and `riscv/encoding.h` (Regents of the University of California; libfemto authors); for the S3 later, the Tensilica/Cadence permission notices (§E) |
| `…/PROVENANCE.md` | image tag, ESP-IDF commit, toolchain, `sdkconfig.defaults` verbatim, build command, SHA-256 of each binary; "unmodified ESP-IDF sources, configuration only"; any header rewrite (§B) |
| root `NOTICE` | one paragraph, as for the SVD |
| distributed `inkemu` binary | the same NOTICE text reachable without the repository (packaged file, or a `licenses` command / GUI "About" page) — the binary embeds the bootloader bytes |

The bootloader prints "ESP-IDF v5.5.2 2nd stage bootloader": a factual statement, no trademark use by inkemu (Apache-2.0 §6).

## B. Header fields, and how many binaries the profiles need

Header of the reference build (M): `E9 03 02 4F`, entry `0x403CBF1A`, `wp_pin 0xEE`, chip id 5, byte 14 (legacy min rev) 3, `min_rev_full` 3, `max_rev_full` 199, `hash_appended` 1; checksum `0x4D` at `0x520F`, digest at `0x5210–0x522F`.

| Field (offset) | Who reads it | Consequence |
|---|---|---|
| chip id (12–13) | ROM (RFB §F step 3) and the bootloader for itself (`bootloader_init.c:64`) | one binary per chip family |
| `min_rev_full` (15–16) | the bootloader, against eFuse, for itself (`bootloader_init.c:50-68`, `bootloader_common_loader.c:35-50`); **not** the ROM (RFB §F) | chip below → `chip revision check failed. Required >= v0.3, found v0.x.`, `bootloader_init` fails, `esp_rom_software_reset_system`, the ROM loads it again: an endless reset loop (R). inkemu should refuse a profile revision below the shipped minimum before boot |
| `max_rev_full` (17–18) | skipped for the bootloader (`bootloader_common_loader.c:132-137`); checked for the **application** unless 0 or 65535 (`:30, 52-61`) | none for the bootloader |
| flash size, byte 3 high nibble | the bootloader: `update_flash_config` → `esp_rom_spiflash_config_param(…, size*1 MiB, …)` (`bootloader_flash_config_esp32c3.c:123-156, 278-281`); the application then compares with the detected chip (`esp_flash_spi_init.c:580-594`) | header > detected → application logs `Probe failed` and fails to start; header < detected → warning, the application uses the header size. **The bootloader header must not exceed the profile's flash** |
| SPI mode (2), frequency (byte 3 low nibble) | the ROM (RFB §F step 1 bounds; the boot banner `mode:DIO, clock div:1`) | DIO / 80 MHz in every Xteink file (FW §H, §I) |
| `hash_appended` (23) + digest | the ROM hashes regardless; a mismatch is not fatal without secure boot (RFB §F step 7) | a header rewrite must recompute it to stay clean |

Measured variants (all `hello_world` bootloader, DIO, 80 MHz unless said):

| Tag | Change to the reference configuration | Size | SHA-256 | Differs from the reference in |
|---|---|---|---|---|
| `ref` | — | 21,040 | `601b48456b0a…87afaa3` | — (= pinned `SHA256SUMS.inputs`) |
| `f4mb` | `FLASHSIZE_4MB` | 21,040 | `d6421484b1ab…4f56109c5` | byte 3 (`4F`→`2F`) + 32 digest bytes (33 bytes) |
| `rev0` | `ESP32C3_REV_MIN_0` | 21,040 | `98f3358a15e4…d134f0372a5` | bytes 14, 15 (`03`→`00`) + digest (34 bytes) |
| `qio` | `FLASHMODE_QIO` | 22,096 | `c874cdf525b2…9103cc58` | code (QIO enable) — header still says **DIO** |

Rewriting byte 3 of `ref` to `0x2F` and recomputing the digest over `[0, 0x5210)` (`r6/tools/resize.py`) gives exactly `f4mb` **(M)**. esptool performs the same rewrite on any image written at offset `0x0` when `--flash_size`/`--flash_mode`/`--flash_freq` are not `keep` (`cmds.py:263-345`: header bytes 2–3, then "recalculate the SHA digest if it was appended"); it never touches an image at another offset. FW §M's "do not rewrite headers" is right for the **application** and too strict for the bootloader.
**Conclusion.** One pinned binary per chip family (and per flash-mode family, DIO now). A profile states what it already states: `soc.family` selects the binary, `flash.size_mib` gives the nibble (16 → `4`); composition either keeps the binary (16 MiB profiles: every current one) or rewrites byte 3 and the digest, as esptool would, and pins the derived hash in a test. A future QIO or octal device needs another build and a field in the profile naming the flash mode.

## C. What the stock bootloader refuses, against inkemu's eFuse; CrossPoint behind it

| Check (stock v5.5.2, reference configuration) | Source | inkemu `x4` | CrossPoint 1.6.5 C3 image |
|---|---|---|---|
| bootloader min chip rev v0.3 | §B | v0.3 (`efuse.rs:224-231`, boot line `chip revision: v0.3`, M) | — |
| application chip id = 5 | `esp_image_format.c:375` → `bootloader_common_check_chip_validity` | — | 5 ✓ |
| application chip rev in `[min, max]` (each ignored when 0 or 65535) | `bootloader_common_loader.c:35-63` | v0.3 | min 0, max 65535: unset ✓ |
| application eFuse block rev in `[min, max]` | `esp_image_format.c:752-757`, `bootloader_common_loader.c:93-114` | v1.2 (`efuse.rs:233-238`, M) | 0–199 ✓ |
| application `secure_version` ≥ eFuse | only under `CONFIG_BOOTLOADER_APP_ANTI_ROLLBACK` (`esp_image_format.c:265-270`, `bootloader_utility.c:323-333`), not set | eFuse secure version 0 | 0 |
| app rollback (`NEW` → `PENDING_VERIFY`, abort a pending image) | only under `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE` (`bootloader_utility.c:446-452`), not set | — | a blank `otadata` is written with seq 1 |
| image checksum and appended SHA-256 | `esp_image_format.c` | — | valid (FW §H) |

The stock bootloader refuses nothing a CrossPoint/Arduino bootloader would accept for this image: the differences are behavioural (rollback state handling, validation skipped on deep-sleep wake, log level; FW §I). `skip_efuse_blk_check.c` in CrossPoint wraps the eFuse-block check **in the application** (OTA validation; comment: "The X3's validation code misreads … garbage eFuse block revision values"), not in any bootloader **(R, `cr/src/platform/skip_efuse_blk_check.c`, `cr/platformio.ini:110`)**.

**On inkemu (M).** `inkemu run --device x4 --flash flash_cp_stockbl.bin --max-time 3` (image `f5a4065a…5a9616` of FW §K.1, stock bootloader + X4 table + `crosspoint-1.6.5-x3-x4.bin`), exit code 0, 14 s on the host:

- ROM banner, `load:` ×3, `entry 0x403cbf1a`, then **27** bootloader lines, those of QEMU's run F0 (`fwx/out/f0.uart`, timestamps aside) but for two: `efuse block revision` (QEMU v1.3, inkemu v1.2 from its production dump) and `SPI Mode` (QEMU `SLOW READ`, inkemu `DIO`) (`chip revision: v0.3`, `efuse block revision: v1.2`, the six partitions, `No factory image, trying OTA 0`, seven `esp_image` segment lines, `Loaded app from partition at offset 0x10000`, `Set actual ota_seq=1 in otadata[0]`);
- Arduino-core lines on UART0: `nvs_open failed: NOT_FOUND` ×2, then ten pairs `i2cWriteReadNonStop … ESP_ERR_INVALID_STATE` (the X3/X4 I2C probe);
- `Guru Meditation Error: Core 0 panic'ed (Interrupt wdt timeout on CPU0)`, `MEPC 0x4219b962` (FW §K.4's ADC poll), core dump, `Rebooting...`, `rst:0xc (RTC_SW_CPU_RST)`, a second bootloader pass (no `otadata` write this time);
- at the limit: `stopped at pc 0x4213c9e4 after 342248071 cycles (228377462 instructions)`; repeated unmodelled reads `APB_SARADC.INT_RAW` (9,577,669) and `SPI2.CMD` (4,504,182). The second life polls SPI2 rather than panicking again in the ADC wait (QEMU repeated the ADC panic, FW key result 10): not investigated, SP1b-3e. Four 4 KiB sectors written (`otadata`, NVS, core dump).

The same run with the bootloader region replaced by the `err`, `ard` (ERROR + rollback + skip-on-wake) and `rev0` builds (`r6/tools/swapbl.py`, `--max-time 1.5`) reaches the same panic; the ERROR builds print **nothing** between the ROM's 9 lines and the application's first line **(M)**.

## D. Which configuration to ship

| | Reference artefact (`601b4845…`) | ERROR + rollback + skip-on-wake (`ard`, `8346c569…`) | CrossPoint's own (Arduino prebuilt, FW §I) |
|---|---|---|---|
| Console before the application | 27 INFO lines (this table and image) | none | none |
| Rollback | off: `NEW`/`PENDING_VERIFY` entries boot as they are | on: `NEW` → `PENDING_VERIFY` (an `otadata` erase+program per such boot), a pending image not confirmed is aborted | on |
| Deep-sleep wake | full validation (SHA over the whole application) | validation skipped | skipped |
| Min chip rev | v0.3 | v0.3 | v0.0 (max 65535) |
| Pinned and tested today | `SHA256SUMS.inputs`, `SHA256SUMS.output`, golden log (TO §2, 57 lines), `reference_boot.rs:46-65, 562-594`, `run_cli.rs:1094-1102`, trace hash `10174cc2726c3d77` (carry-over note) | nothing | nothing (not embeddable, FW §N) |
| Reproducible | yes (TO §2) | yes: `ard`, `ard2` (other path), `ard3` (other path, `IDF_CCACHE_ENABLE=0`) all `8346c569…767c`; `err` = `err2` `8e2ca1f1…5ed68` | — |
| Size | 21,040 | 18,816 | 18,672 |

Facts for the choice: both configurations are reproducible, so both could be pinned; two configurations double the pins, the oracle (QEMU golden logs) and the CI build time (≈7–10 s per extra bootloader in a warm container, M). The INFO lines are the only place a user sees the bootloader's partition choice and the `otadata` write on the console. No configuration reproduces the stock Xteink bootloader, which is unknown (FW §O.1). Recommendation: ship the reference artefact as the default for every C3 profile; if a "quiet" bootloader is wanted later, add it as a second pinned entry that a profile or a GUI can select, not as a replacement.

## E. ESP32-S3 later

| Item | Fact | Tag |
|---|---|---|
| Recipe | same script with `CONFIG_IDF_TARGET="esp32s3"`: `idf.py set-target esp32s3`; Xtensa toolchain in the same image | M |
| Result | 20,992 bytes, `53765fcd83f6…a023e`; rebuilt with cache off: identical; header DIO, 16 MB, 80 MHz, chip id 9, min rev v0.0, max v0.99, entry `0x403C8920` | M |
| Offset | `0x0` for C3 and S3, `0x1000` only for the ESP32 (`esptool.targets`, `BOOTLOADER_FLASH_OFFSET`) | M |
| Linked | same components; libgcc symbols are ROM symbols (`__udivdi3` = `0x40002544` …); 297 files compiled: 235 Apache-2.0, plus Tensilica/Cadence MIT-style permission headers (`xtensa/esp32s3/include/xtensa/config/core-isa.h`, `xtensa/include/xtensa/hal.h`, …), `esp_rom/esp32s3/include/esp32s3/rom/usb/cdc_acm.h` BSD-3-Clause, two MIT `xtensa_*.h` | M |
| Flash size default | Kconfig default 2 MB, as for the C3; the reference configuration sets 16 MB explicitly, so nothing to change | R |
| ROM | `esp32s3_rev0_rom.elf` is in the pinned archive (`esp32c3-rom-elfs.md:29`) but not in inkemu's manifest | R |
| Does not generalise | the S3 bootloader's min rev differs (v0.0); octal flash or PSRAM boards need other builds (the CrossPoint S3 images say DIO 16 MB, FW §H, so A that X4c/X4 Pro are quad-capable DIO) | A |

## F. Partition tables per profile

| Profile | Table | Evidence | Recommendation |
|---|---|---|---|
| `x4` | `tests/fixtures/partitions/x4-stock.{csv,bin}`, `firmware::partition::xteink_x4_stock()`, `bd0f7954…f6ce` = `gen_esp32part.py` output = CrossPoint's (FW §J) | layout used by both flashers for X4 detection | profile states this table |
| `x3` | `nvs 0x9000/0x5000`, `otadata 0xE000/0x2000`, `ota_0 0x10000/0x770000`, `ota_1 0x780000/0x770000`, `spiffs 0xEF0000/0x100000`, `coredump 0xFF0000/0x10000` | identical in `ct/src/lib/flasher.js:243-250` and `xf/src/esp/useEspOperations.ts:31-38`; `xteink-flasher` **refuses to flash** a device whose table does not match (`useEspOperations.ts:72-100`, compares type, subtype, offset, size only): the layout is that of real X3 units (R). Names, flags, MD5 entry: unknown | profile states this layout; compile it with inkemu's writer under the X4 names (A), candidate `334c9553e279…ae7d5e` (`gen_esp32part.py`, names `nvs otadata app0 app1 spiffs coredump`, M); mark names/MD5 as assumptions until a dump |
| any | where the application goes | first `ota_0`, else `factory`: what the bootloader picks with blank `otadata` (FW §J) | derive from the table, not a separate field |

The CrossPoint 1.6.5 application (5,589,776 bytes) fits both `0x640000` (85 %) and `0x770000` (71 %).

## G. A bootloader supplied by the user

Plausible inputs: the `bootloader.bin` of the user's own PlatformIO/ESP-IDF build; the first `0x8000` bytes of a full dump (contains the stock Xteink bootloader, then erased bytes); Arduino's prebuilt bootloader from a tools repository (accepted from the user, never embedded or fetched, FW §N); by mistake, an application image or a whole merged image. Checks inkemu can make before boot, with the reference values (M on `ref`, the Arduino file and the CrossPoint application):

| # | Check | Bytes | Refusal or warning |
|---|---|---|---|
| 1 | length ≤ `0x8000`; if a partition table parses at `0x8000` the file is a dump or merged image | `has_partition_table` (exists) | refuse: "use --flash" |
| 2 | magic | `[0] == 0xE9` | refuse |
| 3 | segment count ≤ 16; mode ≤ 3; speed nibble valid | `[1]`, `[2]`, `[3] & 0xF` (ROM limits, RFB §F steps 1–2) | refuse |
| 4 | chip id = profile family | `[12..14]` LE = 5 (C3) / 9 (S3) | refuse |
| 5 | `min_rev_full` ≤ profile revision | `[15..17]` LE (ref 3, Arduino 0) | refuse: it would reset for ever (§B) |
| 6 | flash size ≤ profile flash | `[3] >> 4` (ref `4` = 16 MB) | refuse (the application aborts, §B); a smaller size is legal, warn |
| 7 | segments: `len % 4 == 0`, `addr % 4 == 0`, `len ≤ 0x100000`, not over `0x3FCDE160–0x3FCDF663`, and in RAM (`0x3FC8…`/`0x403C…`), never flash-mapped `0x3C…`/`0x42…` | 8-byte headers from `0x18` (RFB §F steps 4–5) | refuse; flash-mapped segments mean an application was given |
| 8 | it is a bootloader, not an application | first segment data at file `0x20`: `esp_bootloader_desc_t` magic `0x50`, version, `idf_ver` at `0x28`, `date_time` at `0x48` (`esp_bootloader_desc.h:20-36`); an application has `esp_app_desc_t` magic `0xABCD5432` at `0x20` | refuse an application; show `idf_ver`/date of a bootloader |
| 9 | XOR checksum | seed `0xEF` over segment data, at the byte with `pos % 16 == 15` after the last segment (ref `0x520F`) | refuse (the ROM would) |
| 10 | appended SHA-256 when `[23] == 1` | 32 bytes after the checksum (ref `0x5210`) | warn: the ROM continues without secure boot (RFB §F step 7) |
| 11 | known hashes | the shipped one; Arduino `6e6b0d38…9fb6` | name it in the summary |

What inkemu cannot check: the configuration of a foreign bootloader (rollback, log level, secure boot) beyond its strings.

## H. Open questions

| # | Question | Why it matters | Status |
|---|---|---|---|
| 1 | The stock Xteink bootloader and its configuration | the behaviour a real X3/X4 has; only a dump brings it | unknown (FW §O.1) |
| 2 | Real X3 table names, flags, MD5 entry | byte identity with a device; not boot behaviour | assumed X4 names |
| 3 | Whether BSD-3-Clause clause 2 applies to macros expanded from `csr.h`/`encoding.h` | carry the notice anyway | not decided legally (engineering reading, not legal advice) |
| 4 | CrossPoint's second life on inkemu polls SPI2 where QEMU repeats the ADC panic | the path after the first reboot differs between the two | measured, not explained; SP1b-3e |
| 5 | Retail chip revision and flash part (v0.3, DIO, 16 MB assumed) | below v0.3 the shipped bootloader loops; QIO parts need another build | profile assumptions, unchanged |
| 6 | S3 Xteink flash type (quad vs octal) and PSRAM | another S3 build if octal | assumed quad DIO |

## I. Artefacts and reproduction (scratch `…/scratchpad/r6/`, not in the repository)

Container: `docker run -d --name inkemu-r6 --network none --entrypoint sleep -e IDF_COMPONENT_MANAGER=0 -v $S/r6:/r6 -v $REPO/tools/reference-firmware:/ref:ro -v $REPO/tests/fixtures/partitions:/partitions:ro -v $S/fw:/fw:ro -v $S/fwx/img:/fwimg:ro espressif/idf:v5.5.2 infinity`; removed with `docker rm -f inkemu-r6`.

| Step | Command |
|---|---|
| Build one bootloader | `docker exec inkemu-r6 bash /r6/tools/bl.sh <tag> /r6/src/<tag>.defaults <esp32c3\|esp32s3>` (copies `hello_world`, `idf.py set-target`, `idf.py bootloader`, keeps `.bin .map .elf sdkconfig`) |
| Variants | `src/<tag>.defaults` = the reference file plus: `err` `CONFIG_BOOTLOADER_LOG_LEVEL_ERROR=y`; `ard` that + `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE=y` + `CONFIG_BOOTLOADER_SKIP_VALIDATE_IN_DEEP_SLEEP=y`; `rev0` `CONFIG_ESP32C3_REV_MIN_0=y`; `f4mb`/`qio`/`s3` edit one line |
| Contributions | host: `python3 -I tools/mapcontrib.py out/ref/bootloader.map` |
| Licences | `docker exec inkemu-r6 bash /r6/tools/deps.sh ref` → `out/ref/deps-spdx.txt`, `linkscripts-spdx.txt` |
| Header rewrite | host: `python3 -I tools/resize.py out/ref/bootloader.bin 2 out/img/ref_as_4mb.bin` |
| Bootloader swap | host: `python3 -I tools/swapbl.py $S/fwx/img/flash_cp_stockbl.bin out/<tag>/bootloader.bin out/img/flash_cp_<tag>bl.bin` |
| inkemu | `target/release/inkemu run --device x4 --flash <image> --max-time 3 > out/cp_inkemu_uart0.out 2> out/cp_inkemu_uart0.err; echo $?` |
| X3 table | `gen_esp32part.py -q --flash-size 16MB out/pt/x3-stock-assumed-names.csv out/pt/x3.bin` |

| File | SHA-256 |
|---|---|
| `out/{ref,f4mb,rev0,qio}/bootloader.bin` | `601b48456b0a…87afaa3`, `d6421484b1ab…4f56109c5`, `98f3358a15e4…d134f0372a5`, `c874cdf525b2…9103cc58` |
| `out/{err,err2,rev0err}/bootloader.bin` | `8e2ca1f1fb1e4b775524d68ad46e16503efeb686e78966cab7dd278b9805ed68` ×2, `74589b3466dcc7be4dcc80610b4a75cea0ae68350a2adbe8cf9376d46dc011c2` |
| `out/{ard,ard2,ard3}/bootloader.bin` | `8346c5691db13e2b94f6017b4062fef666a99b624a1921ff9c059cb4b024767c` ×3 |
| `out/{s3,s3b}/bootloader.bin` | `53765fcd83f60ce93f280f857999bf2c05d6ce52e6063164ef64301ecd8a023e` ×2 |
| `out/ref/bootloader.map`, `contrib.txt`, `deps-spdx.txt` | `dcbee5f35dda…3f37`, `2a0a354ea4f9…a8ea`, `9e5cb535e12d…90d5` |
| `out/s3/bootloader.map`, `deps-spdx.txt` | `abae1a7f0b65…4288`, `a5ef08289e26…8d6d` |
| `out/cp_inkemu_uart0.out` (3 s, stock bootloader) | `4d630230710ee18c8a20b4f90e28fd8dbc3d096cdb1c2ed32f31eebe8b2bdaf2` |
| `out/img/flash_cp_{err,ard,rev0}bl.bin` | `e72cb267…6a90b`, `f73e297b…0c0cca`, `9f19d154…f496` |
| `out/pt/x3.bin` | `334c9553e2799107ed33eb9057f8661c6dd757c662a661edac6c73bb99ae7d5e` |

Licence hygiene: ESP-IDF, esptool and toolchain files were read inside the container for facts; nothing was copied into the repository. CrossPoint material is cited by path and short quotation only.
