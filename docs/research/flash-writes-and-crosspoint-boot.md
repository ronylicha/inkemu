# Flash writes as SPI1 sees them, and booting the CrossPoint firmware from its release `.bin`

Scope: (A) every flash command the mask ROM, the ESP-IDF v5.5.2 bootloader and an ESP-IDF application issue through SPI1, what a NOR model must do with them, and what a first boot writes; (B) the CrossPoint 1.6.5 release images for the Xteink X4, the bootloader and partition table they need, how far the application gets under QEMU, and what a `--firmware` option needs.
Date of research: 2026-10-11. No code was changed; this file is the only repository edit.
Already established and not repeated here: `esp32c3-rom-flash-boot.md` (§D SPI1 on the ROM path, §F image validation, §L), `esp32c3-cache-mmu.md` (§4 ROM cache API, §5, §6), `esp32c3-bootloader-accesses.md` (§2), `xteink-hardware.md`, `sp1b-test-oracle.md`, ADR 0002, `2026-10-11-sp1b-2-carryover.md`.

## 0. Method, sources, tags

| Tag | Meaning |
|---|---|
| **M** (MEASURED) | seen in a trace or UART capture produced for this report (Espressif QEMU `esp_develop_9.2.2_20250817` inside Docker, commands in §Q) |
| **R** (READ) | read in a source file (`file:line`), in the rev3 ROM disassembly (`function@address`), in a datasheet, or in a cloned repository |
| **A** (ASSUMED) | inferred; every A item that carries risk is repeated in §O |

| Source | What | Where |
|---|---|---|
| `IDF:` | ESP-IDF v5.5.2 (`git describe` = `v5.5.2` in `espressif/idf:v5.5.2`), paths relative to `components/` | copied out of the image to scratch `fwx/idfsrc/` |
| `ROM3:` | disassembly of `esp32c3_rev3_rom.elf` (esp-rom-elfs `20260528`) | scratch `boot/romelf/dis_rev3.txt`, symbols `rom/c3rev3.nm` |
| `QEMU:` | Espressif QEMU `hw/ssi/esp32c3_spi.c`, read for facts only, nothing copied | scratch `boot/qemu/` |
| `DS:` | flash datasheets fetched from the vendors (table in §A.6), text extracted with `pdftotext` | scratch `fwx/dl/` |
| `cr/`, `ct/`, `xf/`, `sdk/` | clones of `crosspoint-reader`, `crosspoint-tools`, `xteink-flasher`, `freeink-sdk` (all shallow, depth 1; HEADs `e54c0087` 2026-10-10, `1bd5e06b` 2026-10-10, `2b09cb43` 2026-05-20, `32a265fa` 2026-10-10). Read only; nothing from them was executed | scratch `repos/` |
| Run X1 | QEMU, one instruction per TB, `-d exec,nochain` + `-trace memory_region_ops_*` + `-trace 'm25p80_*'`, image `flash_cp_cpbl.bin` (§K.1), from reset to the first reboot: 77,944,060 instructions | `fwx/out/x1.{flash,mmio,count,tail,uart}`, `x1_app_first.txt` |
| Runs F0/F1, H1, R1 | same without the exec trace (flash commands and MMIO only): CrossPoint with the stock bootloader, CrossPoint to the first reboot, `hello_world` in an OTA layout, CrossPoint with an `otadata` entry in state NEW | `fwx/out/{f0,f1,h1,r1}.*` |
| Runs G1/G2 | X1's image under the gdbstub with two breakpoints that force the ADC wait (§K.5). An experiment that alters execution: its results are marked as such | `fwx/out/{g1,g2}.*` |

Instruction indices are QEMU indices with consecutive duplicate trace entries removed, as in `esp32c3-bootloader-accesses.md`; `+n` means "n instructions after the application entry" (index 31,904,574 in X1). They order events; they are not cycle counts.
Three parts of the reading were delegated to research sub-tasks and returned with `file:line` citations: the ESP-IDF flash driver (§A.2, §A.5, §B, §D), the repositories (§H–§J, §L, §N), the datasheets (§A.6, §C). I re-read and confirmed the lines every conclusion rests on: `spi_flash_chip_generic.c:310-332, 424-481, 64-82`, `esp_flash_api.c:279-296`, `flash_mmap.c:299-326`, `spi_mem_reg.h` (CMD bits, `FLASH_WAITI_CTRL`, `FLASH_SUS_CTRL`, `SUS_STATUS`), `bootloader_utility.c:310-320, 490-503`, the four `LICENSE` files, `flasher.js:1003-1007`; and every register sequence of §A.2–§A.4 is also MEASURED in X1. The datasheet numbers and the remaining repository citations were not re-read by me.

## 1. Key results

1. **The application never uses the ROM flash functions; the bootloader never uses the ESP-IDF flash driver.** Two different register sequences reach the chip for the same opcode: the ROM writes `CMD = SE|PE` (`0x01020000`) for a sector erase, ESP-IDF writes `CMD = SE` (`0x01000000`); both write `CMD = PP` (`0x02000000`) with the byte count in `ADDR[31:24]` **(M)**.
2. Opcodes seen on a complete first boot of CrossPoint (ROM + bootloader + application + panic): `AB 9F 05 5A 7A 04 06 20 02 BB`. Nothing else: no `03`/`0B`/`EB` read, no `D8`/`52`/`C7`/`60` erase, no `01` status write, no `35`, no `75`, no `66`/`99`, no `4B` **(M, X1: 1,632 commands)**.
3. Data reads through SPI1 exist only in the application: `0xBB` (dual I/O), 24-bit address, 4 dummy clocks, **at most 64 bytes per transfer** (`W0..W15`). The bootloader reads everything through the cache window **(M)**.
4. A program is at most 64 bytes per transfer and never crosses a 256-byte page (ESP-IDF), at most 32 bytes (ROM); every program or erase is preceded by `WREN` and followed by `RDSR` polls of WIP **(R+M)**.
5. **WEL must clear when a program or erase completes.** ESP-IDF treats "WIP = 0 and WEL = 1 after an operation" as "command not accepted" (`ESP_ERR_NOT_SUPPORTED`) and issues a `WRDI`. QEMU's flash model keeps WEL set, so every program and erase in the QEMU traces is followed by an extra `RDSR`, `WRDI`, `RDSR`: an artefact a lockstep must expect **(R `spi_flash_chip_generic.c:462-468, 326-330`; M)**.
6. Around every flash operation the application suspends the cache (`Cache_Suspend_ICache`, then `Cache_Resume_ICache`), 192 times in this boot; it invalidates a cache range after a write **only when the written flash page is mapped by `esp_mmu_map`**: 0 times for the NVS and core-dump writes of this boot **(M)**. A live-view window is not observably different from silicon for ESP-IDF code (§D).
7. First-boot writes **(M)**: the bootloader erases and programs `otadata[0]` (32 bytes); the application programs the NVS page header and two entries (`cphw` namespace, `dev_det` key: 5 programs, no erase); a panic writes a 4,292-byte core dump (2 sector erases, 135 programs of 32 bytes). `hello_world` writes nothing.
8. The three CrossPoint 1.6.5 release files are **application-only images**. Only `crosspoint-1.6.5-x3-x4.bin` is for the ESP32-C3 (chip id 5); `-x4c` and `-x4pro` are ESP32-S3 images (chip id 9) **(M)**.
9. The "real CrossPoint C3 bootloader" of the earlier report is `ct/public/firmware/bootloader.bin`: the prebuilt Arduino-ESP32 bootloader (`v5.5.2-729-g87912cd291`, `Feb 11 2026 20:14:10`), log level ERROR, app rollback enabled, validation skipped on deep-sleep wake. The stock ESP-IDF v5.5.2 bootloader boots the same application to the same point **(M)**.
10. **Under QEMU, CrossPoint 1.6.5 runs 1,451,647 instructions past its entry point and stops in the first SAR ADC conversion**: it polls `APB_SARADC.INT_RAW` bit 31 forever inside a critical section, the interrupt watchdog (TIMG1, 300 ms) fires, the panic handler writes a core dump to flash and resets the CPU, and the cycle repeats **(M)**.
11. Before that point the application has touched, in this order: RTC/clock/eFuse setup, SPI1 (flash id), TIMG0 (slow-clock calibration), UART0, the interrupt matrix, SYSTIMER, the temperature sensor registers, IO_MUX, TIMG1, USB-Serial-JTAG, GPIO (power latch on GPIO13), I2C0 (X3/X4 probe), GPIO (panel probe), SPI2 (configuration only), SAR ADC (§K.3). Its own log lines go to USB-Serial-JTAG, which QEMU does not deliver; UART0 shows only ROM lines, Arduino-core error lines and the panic.
12. No licence is stated for any CrossPoint binary. The sources are MIT; the application binary links Arduino-ESP32, ESP-IDF, Espressif radio libraries and wolfSSL. inkemu should take the application from the user and must not embed it (§M, §N).

---

# Part A — flash writes

## A. Flash commands as the controller sees them

### A.1 The two ways to trigger a transfer

`SPI1.CMD` (`0x60002000`) holds one self-clearing bit per hard-wired command plus the user-defined transfer bit **(R `IDF:soc/esp32c3/register/soc/spi_mem_reg.h:18-116`)**. "Opcode" is what QEMU's controller puts on the wire **(R `QEMU:esp32c3_spi.c`; M where a row says so)**; on silicon the opcode of a hard-wired bit is fixed by the controller and is not documented.

| Bit | Name | Opcode | Other registers used | Who writes it |
|---|---|---|---|---|
| 31 | `FLASH_READ` | `03` | `ADDR`, `MISO_DLEN`, `W0..` | nobody (R) |
| 30 | `FLASH_WREN` | `06` (M) | none | ROM `SPI_write_enable`, IDF `spimem_flash_ll_set_write_protect(false)` |
| 29 | `FLASH_WRDI` | `04` (M) | none | ROM `esp_rom_spiflash_write_disable`, IDF `…set_write_protect(true)` |
| 28 | `FLASH_RDID` | `9F` (M) | result in `W0[23:0]` | ROM `esp_rom_spi_flash_update_id` only |
| 27 | `FLASH_RDSR` | `05` (M) | result in `RD_STATUS[7:0]` | ROM `esp_rom_spiflash_read_status` only |
| 26 | `FLASH_WRSR` | `01` | value in `RD_STATUS[15:0]`, width by `CTRL.WRSR_2B` (b22) | ROM `esp_rom_spiflash_write_status` (not on any measured path) |
| 25 | `FLASH_PP` | `02` (M) | `ADDR = len << 24 or addr24`, data in `W0..W15` | ROM `SPI_page_program`, IDF `spimem_flash_ll_program_page` |
| 24 | `FLASH_SE` | `20` (M) | `ADDR = addr24` | ROM (with b17), IDF (alone) |
| 23 | `FLASH_BE` | `D8` | `ADDR = addr24` | ROM (with b17), IDF (alone); not on any measured path |
| 22 | `FLASH_CE` | `60` in QEMU | none | ROM `SPI_chip_erase`; IDF never (R) |
| 21, 20, 19 | `FLASH_DP`, `FLASH_RES`, `FLASH_HPM` | `B9`, `AB` (M), `A3` | none | `RES`: ROM `SPI_WakeUp` |
| 18 | `USR` | `USER2[7:0]` | `USER`, `USER1`, `USER2`, `ADDR`, `MOSI_DLEN`, `MISO_DLEN`, `W0..W15` | bootloader and IDF, for everything else |
| 17 | `FLASH_PE` | none | flag, see §B.1 | ROM with `SE`/`BE`; IDF with `USR` for 4-byte-address commands only |

The header calls `FLASH_BE` "Block erase enable(32KB)" (`spi_mem_reg.h:71-73`), but both the ROM (`esp_rom_spiflash_erase_block`, unit `0x10000`) and ESP-IDF (`block_erase_size = 64 * 1024`, `spi_flash_chip_issi.c:85`) use it for 64 KiB, and QEMU sends `D8`: the opcode on silicon is `D8` by strong inference **(A)**.

User-defined transfer **(R `IDF:hal/spi_flash_hal_common.inc:180-218`, `hal/esp32c3/include/hal/spimem_flash_ll.h`; M)**: `USER2 = 7 << 28 or opcode` (8-bit command); `USER1[31:26] = address bits − 1`, `USER1[5:0] = dummy clocks − 1`; `USER` bits 31 `USR_COMMAND`, 30 `USR_ADDR`, 29 `USR_DUMMY`, 28 `USR_MISO`, 27 `USR_MOSI`; **`ADDR` holds the address unshifted** (`0x00ff0000` for flash offset `0xFF0000` with 24 address bits: not left-aligned; `spimem_flash_ll_set_usr_address` ignores the bit length); `MOSI_DLEN`/`MISO_DLEN = 8 × bytes − 1`; data in `W0..W15`, first byte in `W0[7:0]`; `CMD = 0x40000`; poll until `CMD == 0`.

### A.2 ESP-IDF application driver (`esp_flash` → ISSI chip driver → `memspi` host → `spi_flash_hal` → `spimem_flash_ll`)

Driver selection **(R)**: probe order issi, gd, mxic, winbond, boya, th, generic (`spi_flash/spi_flash_chip_drivers.c:34-60`); `0x9D6018` matches ISSI (manufacturer `0x9D` and `id & 0xCF00 == 0x4000`, `spi_flash_chip_issi.c:22-37`). Probing sends nothing. Size = `1 << (id & 0xFF)` = 16 MiB (`spi_flash_chip_generic.c:92-114`), then clamped to the image-header size (`esp_flash_spi_init.c:580-594`). `CONFIG_SPI_FLASH_ROM_IMPL` and `CONFIG_SPI_FLASH_AUTO_SUSPEND` default to n (`spi_flash/Kconfig:253-257, 86-88`).

Every bus acquisition (`start()`, 192 in X1) rewrites the configuration **(R `spi_flash_hal_common.inc:45-79`, `spi_flash_hal_iram.c:181-196`; M values, QEMU reads 0 for unstored fields so read-modify-write values are partial)**: `MISC` (`cs0_dis = 0`, `cs1_dis = 1`), `CLOCK = 0x80000000`, `CTRL2 = 0x3E0` (`cs_hold_time = 0x1F`), `FLASH_WAITI_CTRL = 0x14` (`WAITI_CMD = 0x05`), `FLASH_SUS_CTRL`: `pes_en`, `pes_wait_en`, `per_wait_en`, `pes_per_en`, `pes_end_en`, `per_end_en` = 0, `sus_timeout_cnt = 5` (`0x0A000000`).

| Operation | Trigger | Measured register sequence (X1) | Chunking |
|---|---|---|---|
| `RDID` | `USR` | `USER = 0x90000080` (command + MISO, `CS_SETUP`), `USER2 = 0x7000009F`, `MISO_DLEN = 23`, `CMD = 0x40000`, `W0` → `0x0018609D` | twice at init, must agree (`esp_flash_api.c:458-490`); one more from the bootloader-shared code at `+16,912` |
| `RDSR` | `USR` | `ADDR = 0`, `USER = 0x90000080`, `MOSI_DLEN = 0`, `MISO_DLEN = 7`, `CMD = 0x40000`, status = `W0[7:0]` | 1 byte |
| `WREN` / `WRDI` | bit 30 / bit 29 | `CMD = 0x40000000` / `0x20000000`, poll `CMD`, then read `CMD` and `SUS_STATUS` (host status) | |
| Read (DIO) | `USR` | `CTRL = 0x008C2008` (`FREAD_DIO` b23, `D_POL`/`Q_POL` b19:18, `FASTRD_MODE` b13, `FDUMMY_OUT` b3), `USER2 = 0x700000BB`, `USER1 = 0x5C000003` (24 address bits, 4 dummy clocks), `USER = 0xF0000080`, `ADDR = addr`, `MISO_DLEN = 8n − 1`, `MOSI_DLEN = 0`, `CMD = 0x40000`, data from `W0..` | **≤ 64 bytes per transfer** (`memspi_host_driver.c:20, 235-255`); windows of 16,384 bytes per cache-off period (`esp_flash_api.c:40, 955`). Seen in X1: 64 bytes (320 transfers), 32 (5), 4 (1) |
| Page program | bit 25 | `USER1 = 0x5C000003`, `USER = 0xD0000080`, **`ADDR = len << 24 or addr24`** (`0x20009000` for 32 bytes at `0x9000`), `W0..W(n/4−1)` = data, `CMD = 0x02000000` | `min(256 − addr % 256, 64)` bytes (`memspi_host_driver.c:228-232`); above it, 8,192 bytes per cache-off window, or 32 when the source buffer is not in DRAM (`esp_flash_api.c:1138-1146`). Seen in X1: 32 bytes (138 programs), 4 (2) |
| Sector erase | bit 24 | `USER1 = 0x5C000003`, `ADDR = addr24`, `CTRL = 0`, `CMD = 0x01000000` | 4 KiB |
| Block erase | bit 23 | as the sector erase without the `CTRL` write (`spi_flash_hal_iram.c:59-72`) (R) | 64 KiB, chosen by `esp_flash_erase_region` when the range allows it |
| Chip erase | none | `esp_flash_erase_chip` calls `erase_region(0, size)` (`esp_flash_api.c:629-640`) (R) | |

Other modes **(R `spi_flash_chip_generic.c:493-527`)**: slow read `03` (no dummy), fast read `0B`, dual out `3B`, quad out `6B` (8 dummy clocks), quad I/O `EB` (6). The mode comes from the image header (DIO for every Xteink image).
Not issued by the application on the measured path, and by which API they would be **(R)**: `RDSR2 35` and `WRSR 01` only when a quad mode sets QE (ISSI: 8-bit status, QE = bit 6, `spi_flash_chip_issi.c:39-47`); suspend/resume through `FLASH_SUS_CTRL` bits 1/0 only with `CONFIG_SPI_FLASH_SOFTWARE_RESUME`; `66`+`99` reset: no caller; unique id `4B` (no address, 32 dummy clocks, 8 bytes) only from `esp_flash_read_unique_chip_id`; SFDP `5A` only for manufacturer `0x20` in the generic driver; `12`/`21`/`DC` (4-byte address, `USR|PE`) only above 16 MiB.

One write or erase, as issued **(R `spi_flash_chip_generic.c:310-332, 362-379`; M)**:

```
silicon (WEL clears on completion)          QEMU as measured (WEL stays set)
RDSR            wait idle                   RDSR
WREN                                        WREN
RDSR            WEL must read 1             RDSR
RDSR            wait idle (program only)    RDSR
PP or SE                                    PP or SE
RDSR ... RDSR   until WIP = 0               RDSR          WIP = 0, WEL = 1 -> ESP_ERR_NOT_SUPPORTED
                                            RDSR, WRDI, RDSR   "ensure WEL is 0", result ESP_OK
```

Bounds **(R)**: `esp_flash_read/write/erase_region` reject `address > size` or `length > size − address` before any SPI traffic (`esp_flash_api.c:938, 1108-1109, 661-677`); erase needs 4 KiB alignment; a write or erase into the running application or the bootloader region aborts by default (`CONFIG_SPI_FLASH_DANGEROUS_WRITE_ABORTS`, `spi_flash_os_func_app.c:259-273`). The HAL masks the address to 24 bits.

### A.3 Mask ROM functions (what the bootloader uses for writes)

All **R** from `ROM3` unless marked; the two write paths are **M** (X1, `otadata` update at index 31,901,055–31,902,064).

| Function | Sequence |
|---|---|
| `esp_rom_spiflash_read_status@0x4004d13c` | `USER &= ~USR_DUMMY`; loop: `RD_STATUS[15:0] = 0`, `CMD = 0x08000000`, wait `CMD == 0`, `status = RD_STATUS & chip->status_mask`; **repeats while WIP, no timeout** |
| `Wait_SPI_Idle@0x4004d1fe` | wait `SPI0.FSM & 0x70 == 0`, `SPI1.CMD & 0xF == 0`, then `read_status` |
| `SPI_write_enable@0x4004d5be` | `Wait_SPI_Idle`; loop: `CMD = 0x40000000`, wait, `read_status`, **until WEL is set**, no timeout |
| `esp_rom_spiflash_erase_sector@0x4004de3e` → `SPI_sector_erase@0x4004d26e` | `USER`/`USER1` rewritten (address bits = 23); reject a sector index ≥ `chip_size / 4096`; `SPI_write_enable`; `ADDR = addr & 0xFFFFFF` (`0xE000`); **`CMD = 0x01020000`** (`SE` + `PE`); wait `CMD == 0`; `Wait_SPI_Idle` |
| `esp_rom_spiflash_erase_block@0x4004ddc6` → `SPI_block_erase@0x4004d2cc` | same with `CMD = 0x00820000` (`BE` + `PE`), unit `0x10000` |
| `esp_rom_spiflash_write@0x4004deb6` → `SPI_page_program@0x4004d620` | reject `addr + len > chip_size`, `len & 3`; split at 256-byte pages and into ≤ 32-byte chunks (`write_sub_len = 32`); per chunk: `SPI_write_enable`, **`ADDR = len << 24 or addr24`** (`0x2000E000`), `W0..W7`, **`CMD = 0x02000000`** (no `PE`), wait, `Wait_SPI_Idle` |
| `esp_rom_spiflash_read@0x4004df94` → `SPI_read_data@0x4004d018` | `USR` transfer with the opcode of the current read mode, 16 bytes per transfer (`MISO_DLEN = 127`). Not executed in any measured boot |
| `esp_rom_spiflash_write_status@0x4004d34c`, `…_unlock@0x4004db80`, `Enable_QMode@0x4004d770`, `SPI_chip_erase@0x4004d228` | `CMD = 0x04000000` (`WRSR`), `CMD = 0x00400000` (`CE`). Not called by the bootloader |

What the bootloader really does with the chip **(R `IDF:bootloader_support/bootloader_flash/src/bootloader_flash.c:463, 603, 621, 646, 730-797`, `bootloader_flash_config_esp32c3.c:271, 284`; M)**: every boot, through its own `USR` helper: `RDID 9F`, SFDP `5A` (address `0x10`, one byte), resume `7A`, then the unlock for ISSI (`RDSR`; clear bits 7, 5, 4, 3, 2; **only if the status changed**: `WREN`, `WRSR 01` with 8 bits, wait, which never happens with a status of 0), then `WRDI 04` through `USR`. Its only writes are `write_otadata` (`bootloader_utility.c:310-320`: one sector erase, one 32-byte program) and the factory-reset erase. It never reads data through SPI1 (0 read commands in the bootloader phase of X1).

### A.4 Commands of a complete first boot (M, X1, in order of first appearance)

| Phase | Commands (count) |
|---|---|
| ROM | `AB` (1), `9F` (1), `05` (2) |
| Bootloader (CrossPoint's) | `9F` (1), `5A` (1), `7A` (1), `05` (13), `04` (1), `06` (2), `20` (1, `0xE000`), `02` (1, `0xE000`, 32 bytes) |
| Application up to the hang | `9F` (3), `BB` (326: 4 bytes at `0xFF0000`, then the five NVS pages in 64-byte transfers), then 5 × [`06`, `02`] at `0x9000` (32), `0x9040` (32), `0x9020` (4), `0x9060` (32), `0x9020` (4) |
| Panic handler | 2 × [`06`, `20`] at `0xFF0000`, `0xFF1000`; 135 × [`06`, `02`] of 32 bytes from `0xFF0000` |
| Totals for the application phase | `BB` 326, `06` 142, `04` 142 (QEMU artefact, §A.2), `05` 852, `02` 140, `20` 2 |

### A.5 Busy polling and time-outs

| Layer | Mechanism | Tag |
|---|---|---|
| Controller | software spins on `CMD == 0` after every trigger (`spi_flash_hal_common.inc:38-43`; ROM: every function above) | R+M |
| ESP-IDF `wait_idle` | first waits for the host (`CMD` FSM bits idle and `SUS_STATUS.FLASH_SUS == 0`) in 1 µs steps, then `RDSR`; while WIP: `delay_us(20)` and subtract 20 µs from the budget (`spi_flash_chip_generic.c:424-481`) | R |
| ESP-IDF budgets | idle 200 ms, page program 500 ms, sector erase 600 ms, block erase 4,100 ms, chip erase 4,000 ms (`spi_flash_chip_generic.c:64-82`). The budget is the sum of the 20 µs delays, not wall-clock time: a sector erase tolerates at most 30,000 polls | R |
| ESP-IDF erase yield | `vTaskDelay(1)` after 20 ms of bus hold (`spi_flash_os_func_app.c:428-441`) | R |
| ROM | `RDSR` in a loop with no time-out; a chip that never clears WIP or never sets WEL hangs the bootloader | R |

A model that completes every command at once (WIP never seen set) satisfies all of it, as QEMU shows. A model that holds WIP for a real duration needs `esp_rom_delay_us` to make time pass.

### A.6 Durations of a real part (R, datasheets; typical / maximum)

| Part (JEDEC id) | Page program ms | 4K erase ms | 32K erase s | 64K erase s | Chip erase s | Status write ms | Suspend latency µs |
|---|---|---|---|---|---|---|---|
| ISSI IS25LP128F (`9D 60 18`), Rev. A10 | 0.2 / 0.8 | 100 / 300 | 0.14 / 0.5 | 0.17 / 1.0 | 35 / 90 | 2 / 15 | 100 |
| ISSI IS25LP128 (`9D 60 18`), Rev. L11 | 0.2 / 0.8 | 70 / 300 | 0.1 / 0.5 | 0.15 / 1.0 | 30 / 90 | 2 / 15 | 100 |
| Winbond W25Q128JV (`EF 40 18`), Rev. I | 0.4 / 3 | 45 / 400 | 0.12 / 1.6 | 0.15 / 2.0 | 40 / 200 | 10 / 15 | 20 |
| GigaDevice GD25Q127C (`C8 40 18`), Rev 2.3 | 0.5 / 2.4 | 50 / 400 | 0.16 / 0.8 | 0.3 / 1.2 | 50 / 120 | 5 / 30 | 20 |
| GigaDevice GD25Q128E (`C8 40 18`), Rev 1.4 | 0.5 / 2.4 | 45 / 300 | 0.15 / 1.2 | 0.25 / 1.6 | 50 / 100 | 5 / 30 | 20 |
| XMC XM25QH128C (`20 40 18`), Rev 1.3 | 0.5 / 3 | 40 / 400 | 0.12 / 0.9 | 0.25 / 1.8 | 55 / 100 | 1 / 50 | 22 |
| Zbit ZB25VQ128E (`5E 40 18`), Rev D | 0.25 / 2 | 25 / 300 | 0.1 / 0.8 | 0.14 / 1 | 20 / 150 | 3 / 15 | 22 |

Sources: `https://www.issi.com/WW/pdf/25LP-WP128F.pdf` (§9.6), `https://www.issi.com/WW/pdf/IS25LP128.pdf`, `https://www.winbond.com/resource-files/W25Q128JV%20RevI%2008232021%20Plus.pdf`, GigaDevice documents from distributor mirrors, `https://www.xmcwh.com/uploads/435/XM25QH128C.pdf`, `https://www.zbitsemi.com/upload/2026-08/178788118356422000.pdf`. ISSI also gives: byte program 8 / 40 µs, release from power-down 3 µs, software reset 35 µs (128F) or 100 µs (128), power-up ≥ 300 µs. The chip of a real X4 is unknown (§O.2); both ISSI parts answer `9D 60 18`.

## B. `FLASH_PE`, suspend/resume, and the cache during a flash operation

### B.1 `SPI1.CMD` bit 17

| Fact | Tag |
|---|---|
| Definition: "In user mode it is set to indicate that program/erase operation will be triggered. The bit is combined with spi_mem_usr bit. The bit will be cleared once the operation done." Type `R/W/SC` | R `spi_mem_reg.h:113-116` |
| ROM: set together with `FLASH_SE` and `FLASH_BE`; not with `FLASH_PP`, not with `FLASH_CE` | R `SPI_sector_erase@0x4004d29a`, `SPI_block_erase`, `SPI_page_program@0x4004d6d0`; M (`0x01020000`, `0x02000000`) |
| ESP-IDF: `spimem_flash_ll_user_start(dev, pe_ops)` writes `USR` or `USR` + `PE` (`0x60000`); `pe_ops` is true only for the 4-byte-address program and erase commands (above 16 MiB). Hard-wired `SE`, `BE`, `PP` are written without it | R `spimem_flash_ll.h:391-395`, `spi_flash_chip_generic.c:118-151`; M (`0x01000000`) |
| QEMU masks bits below 19 before decoding a hard-wired command: `PE` is ignored | R `QEMU:esp32c3_spi.c` |
| What the hardware does differently when the flag is set (most likely: it marks the transfer as one that auto-suspend may interrupt and that `SPI0` must wait for) | A, §O.5 |

So the same erase is observed as `SE|PE` (ROM) and as `SE` alone (ESP-IDF): the flag does not change the command. The carry-over decision R14 (flag, not command; exactly one erase per write of `CMD`) is confirmed by both paths.

### B.2 Suspend/resume machinery (all R, `spi_mem_reg.h:763-937`, `spimem_flash_ll.h:102-115, 219-286`)

| Register | Fields | Use |
|---|---|---|
| `FLASH_WAITI_CTRL` (`+0x98`, reset `0x14`) | `WAITI_CMD` [9:2] = `0x05`, `WAITI_DUMMY` b1, `WAITI_DUMMY_CYCLELEN` [15:10] | the command the controller itself sends to wait for the flash to be idle |
| `FLASH_SUS_CTRL` (`+0x9C`) | `FLASH_PER` b0 and `FLASH_PES` b1 (self-clearing: send resume / suspend), `FLASH_PER_WAIT_EN` b2, `FLASH_PES_WAIT_EN` b3, `PES_PER_EN` b4, `FLASH_PES_EN` b5 (**enable auto-suspend**), `PESR_END_MSK` [21:6] (reset `0x80`), `FMEM_RD_SUS_2B` b22, `PER_END_EN` b23, `PES_END_EN` b24, `SUS_TIMEOUT_CNT` [31:25] | |
| `FLASH_SUS_CMD` (`+0xA0`) | `FLASH_PER_COMMAND` [7:0] (reset `0x7A`), `FLASH_PES_COMMAND` [15:8] (reset `0x75`), `WAIT_PESR_COMMAND` [31:16] (reset `0x05`) | opcodes the controller sends |
| `SUS_STATUS` (`+0xA4`) | `FLASH_SUS` b0 ("the status of flash suspend"), `WAIT_PESR_CMD_2B` b1, `FLASH_HPM/RES/DP/PER/PES_DLY_128` b2–b6, `SPI0_LOCK_EN` b7 | read by ESP-IDF's host-status check after every command (568 reads in X1, always 0) |

With auto-suspend **off** (the ESP-IDF default, and what CrossPoint 1.6.5 runs: `FLASH_SUS_CTRL` is written with every enable bit 0, **M**), the controller never suspends anything and `FLASH_SUS` stays 0. With auto-suspend **on**, a cache miss during a program or erase makes SPI0 send the suspend command, read the flash, and resume; ESP-IDF then also sets `SUS_STATUS` b5, b6, b7 and time fields (`spi_flash_hal_iram.c:145-179`). ESP-IDF refuses to enable it on this chip: the ISSI driver has no suspend capability, `esp_flash_suspend_cmd_init` returns `ESP_ERR_NOT_SUPPORTED` (`esp_flash_api.c:1592-1595`). The only suspend-related traffic of a boot is the bootloader's `7A` (resume) through `USR`.

### B.3 The cache during a flash operation

| Step | What runs | Registers | Tag |
|---|---|---|---|
| Start (`spi_flash_disable_interrupts_caches_and_other_cpu`, unicore) | `vTaskSuspendAll`, `esp_intr_noniram_disable`, `cache_hal_suspend` → ROM `Cache_Suspend_ICache@0x40051446` | `ICACHE_CTRL1 or= 3`, `ICACHE_CTRL &= ~1`, poll `ICACHE_AUTOLOAD_CTRL` b3, poll `ICACHE_PRELOAD_CTRL` b1, then **a freeze-enable and a freeze-disable** on `ICACHE_FREEZE` (poll b2 set, then clear). No invalidation | R `spi_flash/cache_utils.c:295-300, 340-347`, `hal/esp32c3/include/hal/cache_ll.h:103-106`; M |
| End | `Cache_Resume_ICache@0x40051496` | `ICACHE_CTRL or= 1`, `ICACHE_CTRL1 &= ~3` | R+M |
| Counts in X1, application phase | 192 windows: `SPI1.CLOCK` written 192 times, `ICACHE_CTRL` 386, `ICACHE_FREEZE` 588 writes and 980 reads, `ICACHE_PRELOAD_CTRL` 193 reads | | M |
| `esp_mmu_map` / unmap | cache freeze (not suspend), MMU write, invalidate the range | 2 maps and 2 unmaps of entry 85 in X1 | M, `esp32c3-cache-mmu.md` §1.2 |
| Bootloader | no suspend around its ROM write calls; its reads use `Cache_Disable_ICache` (which invalidates everything) + MMU write + `Cache_Enable_ICache` | | R `bootloader_flash.c:560-617`, `esp32c3-cache-mmu.md` §4.1 |

All code that runs while the cache is suspended is in IRAM or in the ROM (the measured program trigger is at `0x4038bcfe`). A model that never gates the windows on `ICACHE_CTRL`/`CTRL1` (ADR 0002, point 2) is therefore unaffected; the carry-over note that every `ICACHE_CTRL` write drops the decoded flash code means 386 drops in this boot.

## C. NOR semantics a model must get right

Datasheet facts are for the ISSI parts (**R `DS:` IS25LP128F Rev. A10 §6.1, §8.1–8.23, IS25LP128 Rev. L11**); "software" says who depends on it.

| Rule | Fact | Depends on it |
|---|---|---|
| Program | only changes bits 1 → 0: new = old AND data. Erase sets every bit of the sector or block to 1 | NVS (state words and bitmaps are rewritten in place: `0x9020` gets `FFFFFFFE` then `FFFFFFFA`, M), `otadata` |
| Page | 256 bytes. A program that reaches the page end **wraps to the start of the same page**; with more than 256 bytes "the previously latched data are discarded, and the last 256 bytes are kept" | nothing measured: ROM and ESP-IDF both split at page boundaries and send ≤ 64 bytes |
| WEL required | program, erase and status writes are ignored when WEL = 0 (§7.1 note) | both stacks check WEL after `WREN` |
| WEL cleared | at power-up, by `WRDI`, and "after the completion of any Write Non-Volatile Register, Program and Erase operation" | **ESP-IDF requires it** (§1 point 5). QEMU does not clear it (M) |
| WEL after a rejected command (protected area, or no WEL) | not stated | ESP-IDF sends `WRDI` when it sees WEL = 1 after an operation |
| Status register | b7 SRWD, b6 QE, b5–b2 BP3–BP0, b1 WEL, b0 WIP; factory value `0x00` (QE preset only on ordering options Q/P); read `05`, write `01` (one byte, needs WEL). **No status register 2**: `35` is "enter QPI" on ISSI | bootloader unlock clears b7 and b5–b2 only if set (never with `0x00`) |
| Block protection | BP3–0: `0000` none; `0001`–`1000` protect 1, 2, 4, …, 128 blocks of 64 KiB, from the top (TBS = 0) or the bottom (TBS = 1, an OTP bit of the function register `48`/`42`); `1001`–`1111` all. A program or erase into a protected block "will be ignored"; chip erase needs BP = 0 | nothing measured (status stays 0) |
| While WIP = 1 | only `RDSR`, suspend and reset are accepted; array reads "are ignored"; **no datasheet says what data such a read returns** | ESP-IDF never reads while busy; with auto-suspend off the cache is off for the whole operation |
| Address past the end | reads: "the address counter will roll over to the 000000h address". With a 24-bit address on a 16 MiB part every address is valid; a smaller part ignores the high address bits (A, standard NOR) | ROM and ESP-IDF reject `addr + len > chip_size` before any traffic (§A.2, §A.3) |
| Framing | a command is dropped if chip select rises off a byte boundary | carry-over hypothesis 12 (whole bytes only) |
| Other commands the part has | suspend `75`/`B0`, resume `7A`/`30`, `90` (id), `AB` (release, id `17`), unique id `4B` (16 bytes), SFDP `5A` (IS25LP128F: signature `SFDP`, revision 1.6, one header; the byte at `0x10` that the bootloader reads is not defined by it), reset `66` then `99`, deep power-down `B9` | bootloader sends `5A` and `7A` every boot |

## D. Stale cache after a write

| Question | Answer | Tag |
|---|---|---|
| On silicon, does a mapped read see old data after a program or erase? | The cache is a read-only cache with 32-byte lines; SPI1 writes do not go through it. A line fetched before the write stays valid until it is invalidated or evicted, so yes, old data can be returned. No source states it more precisely than ESP-IDF's own design: it flushes because of this | R `esp32c3-cache-mmu.md` §3; A for "can" (not measured on silicon) |
| What ESP-IDF does after each write | `esp_flash_write` and `esp_flash_erase_region` end with `flash_end_flush_cache` → `spi_flash_check_and_flush_cache(address, length)`, **while the cache is still suspended**: for each 64 KiB flash page of the range, if `esp_mmu_map` has it mapped, `cache_hal_invalidate_addr(vaddr, 64 KiB)` | R `esp_flash_api.c:279-296, 764, 1206`, `flash_mmap.c:299-326` |
| Measured | 3 invalidations in the whole application phase: two from `esp_mmu_map` of entry 85 (`SYNC_ADDR = 0x3C550000`, `SYNC_SIZE = 0x800`), one `(0, 0)` at restart. **None** after the 5 NVS programs or the 137 core-dump operations: those pages were not mapped | M (X1) |
| New mappings | `esp_mmu_map` invalidates the range it maps, so a page mapped after a write is read fresh | R `esp_mm/esp_mmu_map.c:436-458` |
| Bootloader | writes `otadata` with ROM calls and no flush; it does not read `otadata` again, and its next `bootloader_munmap`/`bootloader_flash_read` disables the cache, which invalidates everything | R, `esp32c3-cache-mmu.md` §4.1 |
| Gaps in ESP-IDF's own flush | (1) the boot-time IROM/DROM mappings are reserved, not tracked, so never flushed; writes there abort by default. (2) a flash page mapped at two virtual addresses gets only the first one invalidated | R `esp_mmu_map.c:135-217`, `hal/mmu_hal.c:139-158` |

Conclusion: for ROM, bootloader and ESP-IDF application code, **a live-view window and a silicon-accurate cache give the same data on every path read here**: wherever stale lines could exist, ESP-IDF invalidates them before the next mapped read. The two models differ only for (a) software that calls the ROM or the registers directly and reads a mapping without invalidating, (b) the two gaps above, (c) QEMU, which snapshots a page when the MMU entry is written and never refreshes it. The choice left open by ADR 0002 can therefore stay "live view" for the firmware in scope; what a silicon-accurate model would add is not reachable by a healthy ESP-IDF firmware.

## E. First-boot writes (M unless marked)

| Who | When | Commands | Content |
|---|---|---|---|
| Bootloader, OTA layout with no factory app and blank `otadata` | after loading the application, before jumping (`set_actual_ota_seq`, `bootloader_utility.c:490-503`) | ROM `SE` at `0xE000`, ROM `PP` 32 bytes at `0xE000` | `01 00 00 00` (ota_seq 1), 20 × `FF` (label), `02 00 00 00` (`ESP_OTA_IMG_VALID`), CRC `9A 98 43 47`. Stock v5.5.2 prints `Set actual ota_seq=1 in otadata[0]`; CrossPoint's bootloader prints nothing. `otadata[1]` (`0xF000`) stays blank |
| Bootloader with app rollback (CrossPoint's), `otadata` entry in state NEW (what the web flasher writes, §J) | same place | `SE` + `PP` at `0xE000` | the rewrite is M (R1); the new state is pending-verify by the source (`ESP_OTA_IMG_NEW` → `ESP_OTA_IMG_PENDING_VERIFY`, R `bootloader_support/include/esp_flash_partitions.h:68`), not read back |
| Application, same case | Arduino start-up, before any NVS access | `SE` + `PP` at `0xE000` through `esp_flash` | M (R1): final content identical to the first row, state `02` (valid). The Arduino start-up code marking the running image valid is the likely author (A) |
| Application (CrossPoint), blank `nvs` | `nvs_flash_init` reads the five pages (`BB`, 64 bytes at a time) and **writes nothing**; the first write comes with the first key, at `+1,212,928` | 5 × `PP`: `0x9000` (32) page header `FE FF FF FF 00 00 00 00 FE FF …` + CRC; `0x9040` (32) namespace entry `cphw`; `0x9020` (4) bitmap `FE FF FF FF`; `0x9060` (32) entry `dev_det` (u8 = 1, X4); `0x9020` (4) bitmap `FA FF FF FF` | 59 bytes of sector 9 change; no erase |
| Application, panic | core dump to the `coredump` partition | `SE` `0xFF0000`, `SE` `0xFF1000`, 135 × `PP` of 32 bytes | 4,292 bytes, header `C4 10 00 00 02 01 05 00 …` |
| Application, start-up | reads 4 bytes at `0xFF0000` (`BB`) to look for an earlier core dump (`+429,136`) | read only | |
| `hello_world` (ESP-IDF example, OTA layout with a factory app) | 25 s, three restarts | **no write command at all**, flash image unchanged (H1) | |
| PHY calibration (not measurable: QEMU has no radio) | first Wi-Fi or Bluetooth start, `CONFIG_ESP_PHY_CALIBRATION_AND_DATA_STORAGE` default y | NVS namespace `phy`: blob `cal_data` (1,904 bytes: version 4, MAC 6, opaque 1,894), blob `cal_mac` (6), u32 `cal_version`, then `nvs_commit` | R `IDF:esp_phy/src/phy_init.c:692-836`, `esp_phy/include/esp_phy_init.h:48-50` |

CrossPoint does not start Wi-Fi in `setup()` (**R** `cr/src/main.cpp`, HEAD), so the PHY write is not a first-boot write for it. On the second boot it writes nothing before the hang (the cached `dev_det` is read back, no I2C probe, no `Preferences` error) **(M)**.

## F. Where QEMU differs from a real chip on these paths

| # | QEMU (M unless marked) | Real part / recommended model |
|---|---|---|
| 1 | WEL stays 1 after `PP`/`SE`; cleared only by `WRDI` | cleared at completion (§C); the extra `RDSR`, `WRDI`, `RDSR` per operation disappear |
| 2 | every command completes inside the `CMD` write; WIP is never seen set | real durations (§A.6) |
| 3 | `5A` and `7A` logged as `M25P80: Unknown cmd`; SFDP data reads `00` | SFDP table of the part; resume is a no-op when nothing is suspended |
| 4 | `SPI1.USER1` reads `0xFC000003`/`0xFC000007` after an address length of 0 was written, `FLASH_SUS_CTRL`, `FLASH_WAITI_CTRL`, `MISC`, `CLOCK_GATE`, `SUS_STATUS` read 0 whatever was written | storage |
| 5 | `W0` after a 1-byte `RDSR` reads `0xFFFFFF02`: bytes 1–3 keep stale content | unknown (carry-over hypothesis 14: 0) |
| 6 | `PE` (b17) ignored (R) | unknown effect (§O.5) |
| 7 | the cache window is a snapshot taken when the MMU entry is written (R) | §D |
| 8 | all GPIO inputs read 0, SAR ADC / I2C0 / SPI2 / USB-Serial-JTAG data path absent | §K |

## G. Minimal model for flash writes

**Chip (`NorFlash`)**

| Element | Behaviour |
|---|---|
| State | content, status byte (`WIP` b0, `WEL` b1, `BP3..0` b5:2, `QE` b6, `SRWD` b7), a "modified" flag for persistence (carry-over point 5), a content generation for `Bus::page_version` |
| `06` / `04` | set / clear WEL |
| `05` | status byte |
| `02` addr + n bytes | if WEL = 0 or the page is protected: no change. Else for each byte: `flash[page or ((offset + i) mod 256)] &= data[i]`. Then WEL = 0. n ≤ 64 from this controller |
| `20`, `D8` addr | if WEL = 0 or protected: no change. Else fill the 4 KiB sector / 64 KiB block that contains the address with `FF` (low address bits ignored). Then WEL = 0 |
| `C7`/`60` | whole chip if BP = 0 and WEL = 1; then WEL = 0. Reached only through the ROM's `SPI_chip_erase` |
| `01` + 1 byte | needs WEL; stores b7–b2; WEL = 0. Only reached if a status with protection bits is ever loaded |
| `03`, `0B`, `3B`, `BB`, `6B`, `EB` | read from the address, wrapping at the end of the array; `BB` is the one that matters (24-bit address, 4 dummy clocks, ≤ 64 bytes) |
| `7A`, `75`, `66`, `99`, `B9`, `AB` | accepted, no effect on content (`AB` already there); `66`+`99` clears WEL (A) |
| `9F`, `5A` | as today |
| WIP | first version: never set (every measured poll then ends on its first read). Timing version: set for the §A.6 duration, cleared by the scheduler; reads other than `05` while WIP is set are a diagnostic, not a data path |
| Out of range | cannot happen with 24-bit addresses on 16 MiB; for a smaller profile, mask the address to the chip size |
| Power cycle / reset | content kept, WEL = 0, WIP = 0 |

**Controller (`SPI1`)**

| Element | Behaviour |
|---|---|
| `CMD` b30, b29 | `06`, `04` |
| `CMD` b25 | `02`: address = `ADDR[23:0]`, length = `ADDR[31:24]` bytes, data = `W0..` little-endian words. Length 0 and length > 64: not produced by the ROM or ESP-IDF; treat as a refusal with a diagnostic (A) |
| `CMD` b24, b23, b22 | `20`, `D8` with address `ADDR[23:0]`; chip erase |
| `CMD` b26 | `01` with `RD_STATUS[7:0]` (16 bits if `CTRL.WRSR_2B`) |
| `CMD` b31 | `03`: not used by any measured code; refuse with a diagnostic until a firmware needs it |
| `CMD` b17 | a flag; never a command, alone or combined (R14 stands) |
| `USR` with a data-in phase and an address | the read opcodes: address = low `USER1.USR_ADDR_BITLEN + 1` bits of `ADDR`, unshifted (confirms hypothesis 13 for 24 bits); `USR_DUMMY` clocks skipped; up to 64 bytes into `W0..W15` |
| Completion | every command bit reads 0 after the write; the low byte of `CMD` (FSM) reads 0; `SUS_STATUS.FLASH_SUS` reads 0 |
| Storage | `MISC`, `CLOCK`, `CTRL2`, `FLASH_WAITI_CTRL` (reset `0x14`), `FLASH_SUS_CTRL`, `FLASH_SUS_CMD` (reset `0x7A`/`0x75`), `SUS_STATUS`, `CLOCK_GATE`; `SPI0.CORE_CLK_SEL`, `SPI0.CLOCK_GATE` |
| Auto-suspend | not modelled; `FLASH_SUS_CTRL.FLASH_PES_EN = 1` is a diagnostic (no supported firmware sets it) |

**Around it**

- Cache windows stay a live view (§D); a write bumps the content generation so decoded flash code is dropped (only matters if code pages are rewritten, which ESP-IDF forbids by default).
- A per-device flash file: without it `cphw/dev_det` and `otadata` are rewritten at every run and a core dump never survives.
- The reset bit `RTC_CNTL.OPTIONS0` b5 (`SW_PROCPU_RST`) is how the panic path restarts (`ROM3:software_reset_cpu`, write of `0x20` at `0x40048b90`, M); it must keep the flash content.
- Tests the measurements give directly: the `otadata` bytes of §E after a boot of the X4 layout; the NVS bytes of §E; "WEL reads 0 after a program"; "a second program of `FE` then `FA` at the same address gives `FA`"; the command lists of §A.4 without the `04` artefacts.

---

# Part B — CrossPoint and the Xteink flash layout

## H. The release binaries (M: parsed with `fwx/tools/imgdump.py`; nothing executed on the host)

| | `crosspoint-1.6.5-x3-x4.bin` | `-x4c.bin` | `-x4pro.bin` |
|---|---|---|---|
| Size | 5,589,776 | 5,611,520 | 5,632,640 |
| SHA-256 | `8eccf614b6d6f82284d3ec775a383fd421ec1880481ff863cf44a831e0385764` | `51e17d9e…be9bb2b` | `9ebd6ef1…6667eab` |
| Chip id | **5 (ESP32-C3)** | 9 (ESP32-S3) | 9 (ESP32-S3) |
| Header | magic `E9`, 7 segments, DIO, 16 MB, 80 MHz (`02 4F`), `wp_pin 0xEE`, min rev 0, max rev 65535, hash appended | same | same |
| Entry | `0x40380994` | `0x403760E0` | `0x403760F8` |
| `app_desc` | project `crosspoint-reader`, version `1.6.5`, IDF `5.5.5`, built `Sep 27 2026 09:27:39`, secure version 0, eFuse block rev 0–199 | project `arduino-lib-builder`, version `ee57070`, IDF `v5.5.5`, `Jul 20 2026` | same as x4c |
| Checksum, appended SHA-256 | valid (`0x41`; digest `539ac715…f8a815`) | valid | valid |
| Trailing bytes | none: no signature block, no padding | none | none |

Segments of the C3 image: DROM `0x3C210020` (3,362,384 bytes, file `0x18`), DRAM `0x3FC90800` (20,072), IRAM `0x40380000` (25,392), IROM `0x42000020` (2,139,628, file `0x340018`), IRAM `0x40386330` (42,128), RTC `0x50000000` (20), RTC `0x500015AC` (32).
They are application images and nothing else: the image starts at byte 0 with `E9`, carries an `app_desc`, and has no partition table (the bytes at `0x8000` are application data, `75 00 00 00`). Release assets (`gh release view`, tag `1.6.5`, 2026-09-27): the three files above, `-papermono`, `-sticky`, and `firmware*.bin` duplicates of the same sizes; **no ELF, no bootloader, no partition table**.
Other strings in the C3 image: `3.3.11` (Arduino-ESP32 core version, A), build paths `/home/runner/.platformio/packages/framework-arduinoespressif32/…`, 72 wolfSSL strings. The repository HEAD pins pioarduino `55.03.311`, board `esp32-c3-devkitm-1`, `flash_mode = dio`, `flash_size = 16MB`, `partitions = partitions.csv`, upload offset `0x10000` (`cr/platformio.ini:10-12, 34-36, 118-120`), and rebuilds the Arduino core libraries with `custom_sdkconfig` (`:188, 253`).

## I. The bootloader

| Item | `ct/public/firmware/bootloader.bin` (the one the earlier report booted) | Stock ESP-IDF v5.5.2 (inkemu's reference, first 21,040 bytes of `flash_hello.bin`) |
|---|---|---|
| Size, SHA-256 | 18,672 bytes, `6e6b0d386095f0783e9f0513ea29ea2ce413790e7f7153a06a2b04e2b5f59fb6` | 21,040 bytes, `601b48456b0a9d5c9f1beac166135e06686d3c5ed0309dd16840ab39887afaa3` |
| Version (`esp_bootloader_desc`) | `v5.5.2-729-g87912cd291`, `Feb 11 2026 20:14:10` | `v5.5.2`, no date (reproducible build) |
| Header | DIO, 16 MB, 80 MHz, chip id 5, **min rev 0, max rev 65535** | DIO, 16 MB, 80 MHz, chip id 5, min rev 3, max rev 199 |
| Segments | `0x3FCD5820` (`0x1010`), `0x403CBF10` (`0x9F4`), `0x403CE710` (`0x2E94`); entry `0x403CBF10` | same addresses, `0x15A4`, `0xC64`, `0x2FD0`; entry `0x403CBF1A` |
| Log level | **ERROR**: only `E (%lu)` strings. No banner, no partition table print, no `esp_image` lines | INFO (27 lines with this application and table) |
| App rollback | **enabled** (M: with an `otadata` entry in state NEW it erases and reprograms the entry before starting the application, run R1) | not enabled (`# CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE is not set` in the reference `sdkconfig`, R) |
| Deep-sleep wake | image validation skipped (string `Fast booting is not successful`, emitted only under `CONFIG_BOOTLOADER_SKIP_VALIDATE_IN_DEEP_SLEEP`, `bootloader_utility.c:508-551`) | validated at every boot |
| Image verification on a normal boot | hardware SHA over the whole application: 87,340 blocks | same |
| RTC watchdog | same setup: `WDTCONFIG1 = 0x956A0` (9 s), `WDTCONFIG0` enabled | same |
| Secure boot, flash encryption | no trace (no strings; blank eFuse path) | none |
| Flash traffic | identical: `9F`, `5A`, `7A`, `05`, `04`, then the `otadata` update | identical |

Where it comes from **(R)**: the only statement is the comment at `ct/src/lib/flasher.js:1005-1007`, "Bundled ESP32-C3 2nd-stage bootloader (from the CrossPoint PlatformIO build), served as a static asset. Used by the boot-region repair flow to restore offset 0x0 after an accidental overwrite." Its version string and build date equal those of the Arduino core in `ct/public/firmware/x3-crosspoint.bin` (`arduino-lib-builder`, `v5.5.2-729-g87912cd291`, built 19:57:14 the same day): it is the bootloader prebuilt with the Arduino-ESP32 libraries, with its header patched for 16 MB DIO by PlatformIO (**A**), not a dump of a device. It is older than the 1.6.5 application (IDF `5.5.5`). The commit that added it cannot be read from the shallow clone.
`crosspoint-reader` has no custom bootloader and no bootloader configuration; a build produces the framework's bootloader. Releases ship the application only; release-candidate workflow artefacts also contain `bootloader.bin` and `partitions.bin` (`cr/.github/workflows/release_candidate.yml:87-96`).
The **stock Xteink bootloader** (the one on a retail X4) is in none of the repositories.

## J. Partition table, `otadata`, and what the flashers write

`cr/partitions.csv` (identical to inkemu's `tests/fixtures/partitions/x4-stock.csv`):

```
nvs,      data, nvs,     0x9000,  0x5000,
otadata,  data, ota,     0xe000,  0x2000,
app0,     app,  ota_0,   0x10000, 0x640000,
app1,     app,  ota_1,   0x650000,0x640000,
spiffs,   data, spiffs,  0xc90000,0x360000,
coredump, data, coredump,0xFF0000,0x10000,
```

| Fact | Tag |
|---|---|
| `gen_esp32part.py` (ESP-IDF v5.5.2) on that CSV gives 3,072 bytes, SHA-256 `bd0f7954aca2ef7d925ee21aaa1f3dc8822d1d6ce5cbbd26a135e5886bfff6ce`: **byte-identical to inkemu's `x4-stock.bin` and to `ct/public/firmware/sticky-partitions.bin`**, the only compiled table in the clones | M |
| The flashers hold the X4 table as data (`ct/src/lib/flasher.js:234-241`, `xf/src/esp/useEspOperations.ts:23-28`) and build the binary themselves (`flasher.js:450-470`) | R |
| Stock **X3** layout: `ota_0` `0x10000`/`0x770000`, `ota_1` `0x780000`/`0x770000`, `spiffs` `0xEF0000`/`0x100000`, same `nvs`, `otadata`, `coredump` (`flasher.js:243-250`); no compiled copy anywhere | R |
| The normal web-flasher path for X3/X4 **writes no bootloader and no partition table**: it reads the device's table, writes the application into the inactive OTA slot, and rewrites the other `otadata` sector with `seq = active + 1`, **state NEW (0)**, CRC = `crc32_le(0xFFFFFFFF, seq)` (`flasher.js:321-359, 720-785`). On blank `otadata` it writes seq 2 into sector 0, which boots `app1` | R |
| A bootloader is written at `0x0` only by the debug "repair boot region" flow (bundled `bootloader.bin` or a user file, table, blank `otadata`, NVS kept by default) and by a full 16 MiB restore (`ct/src/pages/DebugPage.jsx:576-626`, `flasher.js:841-1000`) | R |
| The manual instructions write only `0x10000` (`cr/README.md:119`) | R |
| The application is linked for any slot: its segments are mapped by the bootloader from wherever the image sits (`app1` at `0x650000`: MMU values in `esp32c3-cache-mmu.md` §1.1, computed, not run) | A |
| `spiffs` is not mounted by the firmware (no SPIFFS/LittleFS code at HEAD); `coredump` is written by the panic handler (§E) | R, M |

## K. Booting the C3 application under QEMU

### K.1 Images (composed inside Docker with `esptool.py v4.11.dev1 merge_bin --fill-flash-size 16MB`, flash settings kept)

| Image | `0x0` | `0x8000` | `0x10000` | SHA-256 |
|---|---|---|---|---|
| `flash_cp_cpbl.bin` | `ct/…/bootloader.bin` | table of §J | `crosspoint-1.6.5-x3-x4.bin` | `b24671eb…e3e43a` |
| `flash_cp_stockbl.bin` | stock v5.5.2 bootloader | same | same | `f5a4065a…5a9616` |

`otadata`, `nvs`, `app1`, `spiffs`, `coredump` are `FF`. QEMU opens a writable copy.

### K.2 UART0 capture (M; identical in X1, F1 and the untraced runs, timestamps ±1 ms)

```
ESP-ROM:esp32c3-api1-20210207
Build:Feb  7 2021
rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)
SPIWP:0xee
mode:DIO, clock div:1
load:0x3fcd5820,len:0x1010
load:0x403cbf10,len:0x9f4
load:0x403ce710,len:0x2e94
entry 0x403cbf10
[   261][E][Preferences.cpp:47] begin(): nvs_open failed: NOT_FOUND
[   261][E][Preferences.cpp:47] begin(): nvs_open failed: NOT_FOUND
[   267][E][esp32-hal-i2c-ng.c:372] i2cWriteReadNonStop(): i2c_master_transmit_receive failed: [259] ESP_ERR_INVALID_STATE
[   267][E][Wire.cpp:532] requestFrom(): i2cWriteReadNonStop returned Error 259
   ... the same pair 7 more times, at 273, 279, 285, 293, 299, 305, 311 ms
Guru Meditation Error: Core  0 panic'ed (Interrupt wdt timeout on CPU0).
Core  0 register dump:
MEPC    : 0x4219b964  RA      : 0x4219b958  SP      : 0x3fca8140  ...  S0/FP   : 0x60040000 ...
   ... stack dump
ELF file SHA256: 342869986
Rebooting...
ESP-ROM:esp32c3-api1-20210207
Build:Feb  7 2021
rst:0xc (RTC_SW_CPU_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)
   ... same ROM lines, then directly "Guru Meditation Error" again (no Preferences or I2C lines), forever
```

With the stock bootloader 27 bootloader lines appear between `entry 0x403cbf1a` and the first application line (partition table, `No factory image, trying OTA 0`, seven `esp_image: segment` lines, `Loaded app from partition at offset 0x10000`, `Set actual ota_seq=1 in otadata[0]`); everything after is the same, 10 ms earlier.
What is **not** on UART0: the firmware's own log (`[<ms>] [INF] [MAIN] Hardware detect: X4` and the like). It is written to `Serial`, which is the USB-Serial-JTAG device (`-DARDUINO_USB_MODE=1 -DARDUINO_USB_CDC_ON_BOOT=1`, `cr/platformio.ini:39-40`), and only when a USB host has the port open; QEMU's USB-Serial-JTAG is a stub. The lines above come from the ROM, from the Arduino core's `log_e` (ESP-IDF console, UART0) and from the panic handler.

### K.3 First access to every peripheral register, application entry to the hang (M, X1; 381 registers, grouped by block in order of first access)

`+n` = instructions after the application entry `0x40380994`. The complete list (one line per register, with pc, first value and counts) is `fwx/out/x1_app_first.txt`. Values read from blocks QEMU does not model are 0.

| `+n` | Block | Registers first touched | What it is |
|---|---|---|---|
| +7 | ASSIST_DEBUG, RTC_CNTL | `C0RE_0_DEBUG_MODE`, `RESET_STATE` | start-up, reset reason |
| +14,260 | EXTMEM | `ICACHE_AUTOLOAD_CTRL`, `ICACHE_CTRL`, `ICACHE_CTRL1` | cache HAL init |
| +14,365 | RTC_CNTL, INTERRUPT_CORE0, regi2c (`0x6000E000`, `…44`, `…48`), EFUSE, EXTMEM, SPI0/SPI1 `CLOCK_GATE`, SYSTEM `MEM_PD_MASK`, APB_CTRL `FRONT_END_MEM_PD` / `MEM_POWER_UP`, BB `BBPD_CTRL`, `0x6001CCD4`, `0x60006090`, `0x600050F0` | `DIG_PWC`, `DIG_ISO`, `ANA_CONF`, `TIMER1/3/4/5/6`, `CLK_CONF`, `OPTIONS0`, `RTC_CNTL`, `INT_ENA_RTC`, `INT_CLR_RTC`, `SENSOR_CTRL`, `CPU_PER_CONF`, eFuse `RD_SYS_PART1_DATA4`, `RD_MAC_SPI_SYS_3/5` | `rtc_init`, same family as `hello_world` (`esp32c3-bootloader-accesses.md` §3) |
| +16,811 | SPI1 | `CTRL`, `USER`, `USER1`, `USER2`, `ADDR`, `MOSI_DLEN`, `MISO_DLEN`, `CMD`, `W0` | flash id (`9F`) |
| +17,721 | RTC_CNTL, EFUSE | `STORE4`, `WDTWPROTECT`, `WDTFEED`, `WDTCONFIG0`, `WDTCONFIG1`, `RD_REPEAT_DATA1` | RTC watchdog |
| +40,241 | TIMG0 | `RTCCALICFG`, `RTCCALICFG2`, `RTCCALICFG1` | **slow-clock calibration** (265,000 instructions of waiting) |
| +305,226 | RTC_CNTL, SYSTEM, UART0 | `STORE1`, `TIME_UPDATE`, `TIME_LOW0`, `TIME_HIGH0`, `SYSCLK_CONF`, `STATUS`, `FSM_STATUS`, `STORE5`, `PERIP_CLK_EN0/1`, `PERIP_RST_EN0/1`, `WIFI_CLK_EN`, `BT_LPCK_DIV_INT/FRAC`, `WIFI_RST_EN` | RTC time, clocks, console |
| +306,206 | INTERRUPT_CORE0 | all 62 source maps, `CPU_INT_ENABLE`, `CPU_INT_TYPE`, priorities | interrupt matrix reset |
| +307,229 | UART0, EXTMEM, SENSITIVE (19 registers) | `CLKDIV`, `CLK_CONF`; cache error interrupt enables; memory-protection split lines and monitors | console clock, cache error interrupts, PMS |
| +317,405 | SYSTIMER, RTC_CNTL | `CONF`, `TARGET2_CONF`, `UNIT0_OP`, `UNIT0_VALUE_LO/HI`; `FIB_SEL`, `BROWN_OUT` | `esp_timer`, brown-out |
| +344,821 | SPI0 `CORE_CLK_SEL`, EXTMEM `ICACHE_PRELOAD_CTRL`, `ICACHE_FREEZE`, SPI1 `MISC`, `CLOCK`, `CTRL2`, `FLASH_WAITI_CTRL`, `FLASH_SUS_CTRL` | | `esp_flash_init_default_chip` (two `9F`) |
| +358,740 | APB_SARADC, EFUSE | `APB_TSENS_CTRL`, `TSENS_CTRL2`; `RD_SYS_PART1_DATA5` | temperature-sensor power-down |
| +369,304 | TIMG0, RTC_CNTL, RNG | `WDTCONFIG0`, `STORE6`, `RNG.DATA` | |
| +399,564 | SYSTIMER, SYSTEM, ASSIST_DEBUG | `INT_ENA`, `CPU_PERI_CLK_EN`, `CPU_PERI_RST_EN`, `CORE_0_INTR_ENA` | |
| +399,887 | IO_MUX | `GPIO0` … `GPIO21` (all 22) | sleep configuration of every pad |
| +413,488 | MMU, EXTMEM | entry 85; `ICACHE_SYNC_ADDR`, `SYNC_SIZE`, `SYNC_CTRL` | `esp_mmu_map` of flash page 0 (partition table) |
| +435,206 | TIMG1 | `WDTCONFIG0–3`, `WDTWPROTECT`, `WDTFEED`, `INT_CLR/ENA_TIMERS`, `REGCLK` | **interrupt watchdog** |
| +463,438 | SYSTIMER, SYSTEM | `UNIT1_LOAD*`, `TARGET0_CONF`, `COMP0_LOAD`; `CPU_INTR_FROM_CPU_0` | tick timer; scheduler start (first software interrupt) |
| +463,997 | ASSIST_DEBUG, TIMG0 | stack monitor `CORE_0_MONTR_ENA`, `SP_MIN`, `SP_MAX`; TIMG0 watchdog registers | stack guard, task watchdog |
| +477,161 | MMU, SPI1 | entry 20 (a read, value `0x49`); `W1` … `W15` (+499,259) | address translation check; first 64-byte flash reads (NVS) |
| +588,725 | SYSTIMER, USB_DEVICE | `INT_CLR`, `UNIT1_OP`, `UNIT1_VALUE_LO/HI`, `INT_ST`; `INT_RAW`, `INT_CLR` | tick interrupts; USB-Serial-JTAG driver |
| +686,383 | RTC_CNTL, GPIO | `DIG_PAD_HOLD`; `PIN13`, `FUNC13_OUT_SEL_CFG`, `ENABLE_W1TS`, `STATUS_W1TC`, `OUT_W1TS` | **power latch: GPIO13 unhold, output, high** (first board-specific action) |
| +691,716 | USB_DEVICE | `CONF0`, `INT_ENA`, then `EP1_CONF` (+708,954) | `Serial.begin` |
| +708,987 | UART0 | `FIFO` | first character of the first `Preferences` error line |
| +737,216 | I2C0 (25 registers), GPIO | `CLK_CONF`, `CTR`, `FIFO_CONF`, `INT_CLR`, `FILTER_CFG`, `SR`, timing registers, `TO`, `INT_ENA`, `COMD0–6`, `DATA`, `SCL_SP_CONF`; `PIN20`, `FUNC20_OUT_SEL_CFG`, `FUNC54_IN_SEL_CFG`, `PIN0`, `FUNC0_OUT_SEL_CFG`, `FUNC53_IN_SEL_CFG` | **X3/X4 probe**: I2C on SDA = GPIO20, SCL = GPIO0, two passes over four addresses (`COMD` writes: start, write 1 or 2 bytes, restart, read, stop) |
| +1,211,363 | SPI1 | `SUS_STATUS`, then the five NVS programs (+1,212,928 … +1,249,291) | verdict X4 stored in NVS |
| +1,255,547 | GPIO, RTC_CNTL | `PIN21`, `PIN8`, `PIN4`, `PIN10`, `PIN6`, `PIN5` and their `FUNCn_OUT_SEL_CFG`, `OUT_W1TC`, `PAD_HOLD`, `IN` | **panel probe**, bit-banged on CS 21, SCLK 8, DC 4, MOSI 10, BUSY 6, RST 5 |
| +1,427,407 | SPI2 (24 registers) | `SLAVE`, `MISC`, `USER`, `USER1`, `CTRL`, `CLK_GATE`, `DMA_CONF`, `CLOCK`, `W0–W15` | `SPI.begin(8, 7, 10, 21)`: configuration only, **no transfer** (`CMD` never written) |
| +1,432,054 | GPIO | `PIN7`, `FUNC7_OUT_SEL_CFG`, `FUNC64_IN_SEL_CFG`, `PIN1`, `PIN2`, `PIN3` and their `FUNCn_OUT_SEL_CFG` | MISO, the two button ladders, the power button |
| +1,448,355 | APB_SARADC | `CTRL`, `CLKM_CONF`, `ONETIME_SAMPLE`, `ARB_CTRL`, `INT_CLR`, **`INT_RAW`** | first ADC conversion: attenuation 3 (`0x01800000`), **channel 1 = GPIO1** (`0x02000000`), start |
| **+1,451,647** | APB_SARADC | `INT_RAW` read **12,472,498 times** at `0x4219b962` | **the hang** |
| +38,869,696 | EXTMEM, RTC_CNTL, GPIO, SYSTEM, UART0 | cache error status, `WDTCONFIG2`, `CACHE_STATE`, `FUNC0–5_IN_SEL_CFG`, peripheral resets | panic handler, core dump (+45,450,027 first erase), restart |
| +46,039,485 | RTC_CNTL | `OPTIONS0` ← `0x20` from `ROM3:software_reset_cpu` | CPU reset; ROM runs again at index 77,944,060 |

Totals for the application phase: SPI1 31,052 reads / 24,649 writes; INTERRUPT_CORE0 2,609 / 4,063; EXTMEM 2,360 / 1,381; SYSTIMER 5,233 / 1,112; UART0 10,162 / 5,952; USB_DEVICE 10,744 / 144; I2C0 918 / 776; GPIO 429 / 658; IO_MUX 355 / 355; TIMG1 512 / 1,121; SPI2 15 / 40.
Not touched before the hang: LEDC, RMT, TWAI, I2S, UART1, GDMA, the Wi-Fi/Bluetooth MAC and baseband beyond the power and clock gates listed (`WIFI_CLK_EN`, `WIFI_RST_EN`, `BBPD_CTRL`, `FRONT_END_MEM_PD`), the eFuse programming registers, AES/RSA/HMAC/DS. SHA is used by the bootloader only.

### K.4 What it waits on, and why it stops

| Wait | Where | In QEMU |
|---|---|---|
| Slow-clock calibration (`TIMG0.RTCCALICFG*`) | +40,241 | answered |
| SYSTIMER tick, software interrupt, interrupt delivery | from +463,438 | answered |
| Flash operations (192 bus windows) | from +344,821 | answered |
| USB-Serial-JTAG (`EP1_CONF`, driver interrupts) | from +588,842 | stub: no host, firmware log lines are dropped |
| I2C0 transactions | +737,216 | not modelled: the driver fails each transaction with `ESP_ERR_INVALID_STATE` after about 6 ms; zero devices answer on both passes, which the firmware takes as "X4" and stores (`cphw/dev_det = 1`) |
| Panel probe on GPIO | +1,255,547 | inputs read 0; the probe returns, result unknown from UART0 |
| **SAR ADC conversion done (`APB_SARADC.INT_RAW` bit 31)** | **+1,451,647** | **never set: QEMU has no SAR ADC. This is where it stops** |

The stop, instruction by instruction **(R: disassembly of the IROM segment at `0x4219b8f0–0x4219b9a0`, `S0 = 0x60040000`; M)**: clear `INT_CLR` bit 31; write `ONETIME_SAMPLE` (attenuation, channel, bit 31 = ADC1 one-shot, then bit 29 = start); loop `lw a5, 0x44(s0); and a5, s3, a5; beqz` with `s3 = 0x80000000`; afterwards the result would be read from `SAR1DATA_STATUS` (`+0x2C`, low 12 bits). The loop runs with interrupts masked, so the tick cannot run; after 300 ms (37.4 M instructions at 8 ns) the TIMG1 interrupt watchdog fires, the panic handler prints the dump, writes the core dump to flash and resets the CPU. Each later boot reaches the same loop.

So the plain statement: **under QEMU the firmware prints two `Preferences` errors and sixteen I2C error lines on UART0, identifies the board as an X4, writes that to NVS, configures SPI2, and dies in its first button-ladder ADC read, 1.45 M instructions after its entry point. It never reaches the SD card or the display, and none of its own log lines are visible.**

### K.5 One step further: forcing the ADC wait (M, but an altered execution)

Runs G1/G2: the same image under the gdbstub, a breakpoint on the `beqz` that skips the loop and one after the result load that sets the raw value to 4095 ("no button"; QEMU's GPIO inputs read 0, which is "power button held" and "USB absent").

| Observation | Tag |
|---|---|
| No panic; 22 conversions in the first 75 s of wall time, 1,575 over the longer run | M |
| New registers after the ADC: `SAR1DATA_STATUS`; `GPIO.PIN12`, `FUNC12_OUT_SEL_CFG` (SD chip select); `SPI2.CLOCK` (three values, last `0x0025300A`), `SPI2.CMD` (`0x00800000` update, `0x01000000` user transfer), `SPI2.MS_DLEN` = 7, `SPI2.W0` | M |
| More than 1,000,000 single-byte SPI2 transfers (2,000,587 `CMD` writes): the SD-card initialisation of SdFat, each byte answered `00` by QEMU's missing device | M |
| Nothing new on UART0. No display register sequence appears (no further GPIO or SPI2 register is touched) | M |
| Later in the long run QEMU logs `Invalid read at addr 0x0, size 2` repeatedly (the guest fetches from address 0) with no panic text. Not investigated: the experiment patches execution and every unmodelled register reads 0 | M, §O.9 |

From the source at HEAD **(R, `cr/src/main.cpp:431-655`, not verified against the 1.6.5 binary)** the order after the ADC is: wake reason and power-button hold check (a power-on reset with GPIO20 low is treated as a power-button wake and **the firmware goes back to deep sleep unless GPIO3 is still held low**; with GPIO20 high it sleeps at once), `Storage.begin()` (SdFat on the shared SPI bus, chip select GPIO12, one attempt), then with no card: `display.begin()` and an "SD card error" screen; with a card: settings from the card, fonts, splash, home screen.

### K.6 The milestone "boot logs of a real firmware"

What inkemu needs to show at least what QEMU shows (§K.2), in the order the firmware asks for it:

1. Everything SP1b-3 already lists (calibration, SYSTIMER, interrupt delivery, RTC time).
2. Flash writes (§G): `otadata` by the bootloader, then `BB` reads, then the NVS programs. With a read-only flash the bootloader's refused write is harmless (it does not read the entry back), but the application's NVS write is lost; what the firmware does after that was not measured.
3. TIMG1 as storage at least; expiry only matters once something hangs.
4. USB-Serial-JTAG registers `INT_RAW`, `INT_CLR`, `INT_ENA`, `CONF0`, `EP1_CONF` (already partly modelled for the ROM). To show the firmware's own log lines, the device must look connected to a host (§O.4).
5. GPIO output and matrix registers as storage (`PINn`, `FUNCn_OUT_SEL_CFG`, `FUNCn_IN_SEL_CFG`, `ENABLE_W1TS/W1TC`, `OUT_W1TS/W1TC`, `STATUS_W1TC`, `IN`), `RTC_CNTL.PAD_HOLD`/`DIG_PAD_HOLD`.
6. I2C0: either storage that makes the driver fail as in QEMU (the firmware then decides "X4" and logs 16 error lines), or a master model that reports NACK (the same verdict without the error lines; what a real X4 does). The second is the faithful one.
7. SAR ADC one-shot: `INT_RAW` bit 31 set after a start, `SAR1DATA_STATUS` = the ladder value (idle above 3,900 on GPIO1 and GPIO2). Without it the boot ends exactly as in QEMU.
8. To go past the QEMU point: GPIO3 held low at boot (or the firmware sleeps), then SPI2 with an SD card or a "no card" answer (`FF`), then the panel.

## L. What a user realistically has

| Input | Format | Where it comes from | Usable as |
|---|---|---|---|
| CrossPoint release | application image, `E9` at byte 0, 5.5 MB, chip id 5 | GitHub release asset `crosspoint-<ver>-x3-x4.bin` (or `firmware.bin`) | `--firmware`: needs a bootloader and a table (§M) |
| Other community firmware (Papyrix, …) | application image at `0x10000` | their releases | same |
| Self-built firmware | `firmware.bin` + `bootloader.bin` + `partitions.bin` from PlatformIO (`.pio/build/<env>/`) | the user's build; CrossPoint RC artefacts | all three, or a merged image |
| Full dump | exactly `0x1000000` bytes, `esptool read_flash 0 0x1000000`, or the web tool's `flash.bin` (`ct/src/lib/flasher.js:789-865`) | the user's device; contains the stock bootloader, table, NVS (Wi-Fi credentials), both slots | `--flash` as it is |
| Partition dumps | `app0.bin`, `app1.bin`, `otadata.bin` from the web debug page (`ct/src/pages/DebugPage.jsx:663, 688`) | the user's device | `app*.bin` is an application image followed by the rest of the slot (A) |
| Stock X3/X4 firmware | **plain application image** | not in any repository; downloaded at run time from `https://api-prod.xteink.{cn,cc}/api/v1/check-update?…&device_type=ESP32C3` (X4) or `ESP32C3_X3` (X3) (`ct/worker/index.ts:1074-1096`, `xf/src/remote/firmwareFetcher.ts:26-66`); no licence or terms stated anywhere | `--firmware`, by a user who obtained it; inkemu must not fetch or ship it |
| `.xota` | `XOTA` (4) + IV (16) + AES-128-CTR metadata (182) + body | vendor updates for the **X4 Pro and X4C only** (ESP32-S3), `ct/unlocker-tool/crates/unlocker-core/src/xota.rs:11-78` | not relevant to the C3 devices; recognise the magic and say so |

No flash encryption or secure boot is mentioned for X3/X4 in any source; the eFuse state of retail units is unverified (`xteink-hardware.md` §7.4).

## M. What `--firmware` needs

| Piece | Offset | Source | Licence | Recommendation |
|---|---|---|---|---|
| Application | `0x10000` (`app0`); must fit `0x640000` (the 1.6.5 image uses 85 %) | the user's file | CrossPoint: no licence stated for the binary (§N) | path given by the user; never embedded, never downloaded by default |
| 2nd-stage bootloader | `0x0` | (a) the stock ESP-IDF v5.5.2 bootloader inkemu already builds reproducibly (`tools/reference-firmware/build.sh`); (b) `ct/public/firmware/bootloader.bin`; (c) the user's own (`bootloader.bin` of a build, or the first `0x8000` bytes of a dump) | (a) ESP-IDF, `LICENSE`: "Apache License Version 2.0", SPDX `Apache-2.0`; (b) repository `LICENSE` MIT, no statement for the file, contents are ESP-IDF code built by the Arduino project; (c) the user's | (a) is the only one inkemu can state a licence for, and it boots CrossPoint 1.6.5 to the same point (M). Embedding it or building/fetching it on demand is the ADR the carry-over note asks for; this report adds no reason against (a) and one caution: it prints 27 INFO lines that the Arduino bootloader does not, has no rollback, and validates on deep-sleep wake (what the stock Xteink bootloader does is unknown, §O.1). Never embed (b); accept (c) through an option |
| Partition table | `0x8000` | device profile | facts from `cr/partitions.csv` (repository MIT), compiled by ESP-IDF's `gen_esp32part.py` (Apache-2.0); already in the repository as `tests/fixtures/partitions/x4-stock.{csv,bin}` | profile data; an X3 "stock layout" profile needs its own table (§J) |
| `otadata` | `0xE000`, `0x2000` | generated: all `FF` | none | blank: the bootloader chooses `app0` and writes the entry (needs flash writes, §E). Do not pre-write an entry: the two bootloaders treat states differently |
| `nvs`, `app1`, `spiffs`, `coredump`, padding | rest of 16 MiB | generated: `FF` | none | the first run writes `cphw/dev_det`; a per-device flash file keeps it |
| Image-header settings | bytes 2–3 of bootloader and application | already `02 4F` (DIO, 16 MB, 80 MHz) in every file here | | do not rewrite headers (it would break the appended SHA-256); refuse an application whose chip id is not 5 (`-x4c`, `-x4pro`) by name |

Checks worth making before composing **(R §F of `esp32c3-rom-flash-boot.md`, M here)**: magic `E9`; chip id 5; XOR checksum and appended SHA-256 (the bootloader refuses a corrupt image with `Image hash failed - image is corrupt`); size ≤ slot; `XOTA` magic → explain; a 16 MiB input → it is a dump, use `--flash`.

## N. Licence findings

| Item | File and exact first lines | SPDX | Note |
|---|---|---|---|
| `crosspoint-reader` | `LICENSE`: "MIT License" / "Copyright (c) 2025 CrossPoint Reader organization" | `MIT` | sources only |
| `crosspoint-tools` | `LICENSE`: "MIT License" / "Copyright (c) 2025 SoFriendly" | `MIT` | `unlocker-tool/Cargo.toml:13` `license = "MIT"` |
| `xteink-flasher` (archived) | `LICENSE`: "MIT License" / "Copyright (c) 2025 Dave Allie" | `MIT` | |
| `freeink-sdk` | `LICENSE`: "MIT License" / "Copyright (c) 2026 FreeInk"; `NOTICE`: derived from the OpenX4 E-Paper Community SDK, MIT, "Copyright (c) 2025 Open X4 E-Paper Contributors" | `MIT` | submodule of `crosspoint-reader`, pinned at `32a265fa…` |
| CrossPoint release binaries | **no licence statement**: the release workflow only renames `firmware.bin` (`cr/.github/workflows/release.yml:97-141`); the README has no licence section; the repository has no third-party notice | none stated | the binary links the MIT sources with Arduino-ESP32 (LGPL-2.1, my knowledge, not stated in the repository), ESP-IDF (`Apache-2.0`), Espressif Wi-Fi, PHY, Bluetooth and coexistence libraries (each directory carries an Apache-2.0 `LICENSE` in the ESP-IDF image, R), **wolfSSL** (`wolfssl/Arduino-wolfSSL @ 5.7.2`, `cr/platformio.ini:164-170`; 72 strings in the binary; GPL, my knowledge), `links2004/WebSockets`, ArduinoJson, SdFat, fonts under SIL OFL 1.1 (`cr/lib/EpdFont/builtinFonts/source/*/OFL.txt`), expat, miniz, uzlib |
| `ct/public/firmware/bootloader.bin` | no statement beyond the code comment quoted in §I | none stated | ESP-IDF bootloader code (Apache-2.0) built by the Arduino-ESP32 project |
| `ct/public/firmware/x3-crosspoint.bin`, `x4pro-stock-en.bin` | none; the second is a decrypted vendor image ("pinned stock fallback V7.0.8", `ct/worker/index.ts:1138-1148`) | none stated | vendor firmware: proprietary, no terms found |
| `sticky-partitions.bin` | none | none stated | byte-identical to the output of `gen_esp32part.py` on `cr/partitions.csv` |
| Stock X3/X4 firmware | not redistributed in any repository; no licence or terms | proprietary (A) | |

Consequences for inkemu:

1. **CrossPoint application binaries: download by the user, never embedded, never committed, never fetched by default.** Nobody states the terms of the binary, and it contains at least one GPL component and LGPL ones: redistribution has obligations inkemu should not take on. Tests that need it must fetch it outside the repository, as the ROM is fetched (ADR 0001).
2. The bundled Arduino bootloader and the stock firmware: same rule, stronger for the stock firmware.
3. What inkemu can hold: the partition table (facts, already a fixture) and a bootloader it builds from ESP-IDF sources under Apache-2.0.

## O. Open questions, ranked by risk

| # | Question | Why it matters | Status |
|---|---|---|---|
| 1 | **The stock Xteink bootloader**: version, log level, rollback, and what it does with an `otadata` entry in state NEW or a blank one. A user's full dump contains it; no repository does | a dump is the one input that brings its own bootloader; CrossPoint wraps an eFuse-revision check because of that bootloader (`xteink-hardware.md` §3.2) | unknown; needs a dump from a device |
| 2 | **The flash part of a real X4** (JEDEC id, status default, SFDP) | the id selects the ESP-IDF chip driver (ISSI here: 8-bit status, no suspend) and the bootloader unlock branch; the durations of §A.6 differ by a factor of 4 between vendors | unknown (`esptool flash_id` on a device) |
| 3 | **Does the controller wait for the flash by itself after a hard-wired `PP`/`SE`/`BE`** (the `FLASH_WAITI_CTRL` machinery) before it clears the `CMD` bit, or does the bit clear when the bytes are sent? | decides where a timing model puts the delay: in the `CMD` poll or in the `RDSR` polls. Software works either way | unknown; QEMU completes at once |
| 4 | **USB-Serial-JTAG as seen by Arduino's `HWCDC`**: what makes `Serial` consider a host connected, and whether a boot without one loses or buffers the first lines | the firmware's own log (the actual "boot log of a real firmware") only exists on that port; UART0 shows errors and panics only | not investigated; QEMU's stub delivers nothing |
| 5 | Hardware meaning of `FLASH_PE` beyond the one-line description; whether `SE` without it (ESP-IDF) behaves differently from `SE` + `PE` (ROM) when auto-suspend is on | none while auto-suspend is off | assumed: a flag |
| 6 | Stale cache lines on silicon after a write (§D): never measured | nil for ESP-IDF firmware; matters for software that bypasses `esp_flash` | assumed |
| 7 | WEL after a rejected program or erase; data returned by an array read while WIP = 1; `W0[31:8]` after a 1-byte read | ESP-IDF recovers from the first with `WRDI`; the others are not reachable from the measured code | datasheets silent |
| 8 | The 1.6.5 binary was matched to the source at HEAD (two weeks newer) only through its strings; the order of `setup()` after the ADC read (§K.5) is read from HEAD | the list of what comes after the QEMU stop | read, not measured |
| 9 | The fetch from address 0 in the forced-ADC run | may be an artefact of the experiment, or the first real fault after the SD stage with every device reading 0 | not investigated |
| 10 | The X3 path: stock X3 table, the I2C devices that must answer, a different panel | out of the X4 milestone | read only (§J) |
| 11 | Flash-encrypted or secure-boot devices | no source mentions any for X3/X4 | unverified |

## P. Corrections and additions to earlier reports (those files are left untouched)

| Place | Statement there | Established here |
|---|---|---|
| `xteink-hardware.md` §3.1 | the 1.6.5 release files were listed by size, "not downloaded" | parsed: only `-x3-x4` is an ESP32-C3 image; its DROM segment is at `0x3C210020`, project `crosspoint-reader`, IDF string `5.5.5` |
| `xteink-hardware.md` §3.2 | "otadata blank → bootloader picks first OTA slot [INF]" | measured, and the bootloader **writes** `otadata[0]` (seq 1, state valid) on that boot |
| `xteink-hardware.md` §6 | console = USB-Serial-JTAG | true for the firmware's `Serial`; the Arduino core's error log and the panic handler write to UART0 as well |
| `2026-10-11-sp1b-2-carryover.md`, "À étendre" | the ROM passes `addr or len << 24` for a program | confirmed for the ROM and **also true for ESP-IDF's own driver**; ESP-IDF erases with `SE` alone, the ROM with `SE` + `PE` |
| same, hypothesis 13 | the `USR` address is the low field of `ADDR`, unshifted | confirmed by the `BB` reads (`ADDR = 0x00FF0000` for offset `0xFF0000`) |
| `esp32c3-bootloader-accesses.md` §9 | "flash RDID through `esp_flash`" as the application's flash need up to `main_task` | an Arduino application adds `BB` reads of whole NVS pages before `setup()`, and writes on first use |
| `esp32c3-cache-mmu.md` §9 point 2 | staleness model "needed for OTA/NVS flows" | not needed for them: ESP-IDF invalidates what it has mapped (§D) |

## Q. Artefacts and reproduction (scratch, not in the repository)

Directory `…/scratchpad/fwx/`: `tools/` (scripts written for this report), `img/` (composed images, extracted segments), `out/` (traces, UART captures, flash images after each run), `idfsrc/` (copy of the ESP-IDF components read), `dl/` (datasheets).
Container: `docker run -d --name inkemu-fwx --network none --entrypoint sleep -v …/fw:/fw:ro -v …/repos/crosspoint-reader_crosspoint-tools/public/firmware:/cpfw:ro -v …/oracle:/w:ro -v …/rom/x:/rom:ro -v …/fwx/tools:/tools:ro -v …/fwx/img:/img -v …/fwx/out:/out espressif/idf:v5.5.2 infinity`. Third-party binaries are mounted read-only and run only under QEMU inside this container, without network.

| Step | Command (inside the container after `. /w/idfenv.sh`, unless "host") |
|---|---|
| Parse an image | host: `python3 -I tools/imgdump.py <file>`; segments: `python3 -I tools/segx.py <file> <dir>` |
| Partition table | `gen_esp32part.py -q --flash-size 16MB /img/cp_partitions.csv /img/cp_partitions.bin` |
| Compose | `esptool.py --chip esp32c3 merge_bin --fill-flash-size 16MB --flash_mode keep --flash_freq keep --flash_size keep -o /img/flash_cp_cpbl.bin 0x0 /cpfw/bootloader.bin 0x8000 /img/cp_partitions.bin 0x10000 /fw/crosspoint-1.6.5-x3-x4.bin` |
| Plain boot | `/tools/qboot.sh flash_cp_cpbl.bin cp_cpbl 40` → `/out/cp_cpbl.uart`, `.flash` |
| X1 | `/tools/qtrace.sh flash_cp_cpbl.bin x1 2400 40380994 1 1 3` (about 4 minutes; stops at the first reboot) → `x1.flash` (one line per flash command), `x1.mmio` (at most 3 events per pc and register), `x1.count`, `x1.tail`; host: `python3 -I tools/first_access.py out/x1.count <repo>/third_party/espressif-svd/esp32c3.svd b0p2` |
| F0, F1, H1, R1 | `/tools/qtrace.sh <image> <tag> <seconds> <entry> 9 0 2` with `flash_cp_stockbl.bin`, `flash_cp_cpbl.bin`, `flash_hello_ota.bin` (copy of `oracle/flash_hello_ota.bin`), `flash_cp_cpbl_new.bin` (`otadata[0]` = seq 1, state 0, CRC `9A 98 43 47`) |
| G1, G2 | `/tools/qgdb.sh flash_cp_cpbl.bin g1 75 /tools/adc.gdb`, `/tools/qgdb2.sh … g2 420 …` |
| Disassemble part of the application | `/tools/dis.sh /img/cpseg/seg_42000020.bin 42000020 4219b8f0 4219b9a0` |

Checksums: `out/x1.flash` sha256 `af591cda09fd…a38c`, `out/x1.mmio` `7302cfaed19c…e2c5`, `out/x1.count` `bbe9a5d77f80…af71`, `out/x1_app_first.txt` `4fb4a4a90863…192d`, `out/x1.uart` `6468afac260c…f2e5`.
Licence hygiene: QEMU source was read for behaviour only (one file, cited by name); no code or comment was copied. ESP-IDF facts come from its Apache-2.0 sources, ROM facts from the Apache-2.0 `esp-rom-elfs` ELF. Nothing from the CrossPoint repositories or binaries is reproduced here beyond the partition CSV (six lines of facts), short quoted comments, log lines and hashes.

## Correction of 2026-10-14, to §K (SP1b-3e)

§K.2 says that after the first panic every later boot goes "directly" to the same Guru Meditation, and the hand-over note of SP1b-3c called the second life inkemu showed (a boot that reaches `SPI2.CMD`) "not explained". It is explained (`docs/superpowers/specs/2026-10-14-sp1b-3e-design.md`, §2): the first life stores its verdict `X4` in NVS (§K.3, +1,211,363, `cphw/dev_det`); that write stays in the flash of the device across the CPU reset of the panic, within the same run; the second life reads the cached verdict, skips the I2C probe, and reaches the SD card (SPI2) and the panel before the same wait for the SAR ADC. Its own log lines (`[SD] SD card not detected`, the commands of the SSD1677) go to USB-Serial-JTAG only (§K.2): a UART0 capture cannot show them, and the trace X1 stops at the first reboot (§K.3), so nothing measured under QEMU contradicts this. **R** from the spec's measurement under inkemu (2026-10-14), **M** for the inkemu console. The sections above are left as they were.
