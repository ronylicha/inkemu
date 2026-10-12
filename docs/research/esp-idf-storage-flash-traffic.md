# Flash traffic of an ESP-IDF storage example: an NVS restart counter under QEMU, and a CI firmware for flash writes

Scope: the stock ESP-IDF v5.5.2 example that keeps a restart counter in NVS, built like the reference `hello_world`; every flash command and every byte it writes over three lives under Espressif QEMU; the read path; when an erase becomes a 64 KiB block erase; what an application writes to `otadata`; the registers it adds to the reference life; what inkemu at `main` does with the image; and the firmware, pins and assertions recommended for the plan "flash writes + persistent flash file".
Date of research: 2026-10-12. No code was changed; this file is the only repository edit.
Extends `flash-writes-and-crosspoint-boot.md` (**FW**) Part A and does not repeat it: command encodings and register sequences (FW §A), `FLASH_PE` and the cache (FW §B), NOR rules (FW §C), stale cache (FW §D), the minimal model (FW §G). Other reports cited: `sp1b-test-oracle.md` (**TO**) §2, `esp32c3-interrupts-and-reset.md` (**IR**).

## 0. Method, sources, tags

| Tag | Meaning |
|---|---|
| **M** (MEASURED) | seen in a trace, a UART capture or a flash image produced for this report (QEMU `esp_develop_9.2.2_20250817` inside Docker, commands in §Q), or printed by inkemu at `main` where a row says "inkemu" |
| **R** (READ) | read in a source file (`file:line`) |
| **A** (ASSUMED) | inferred; every A item that carries risk is repeated in §O |

| Source | What | Where |
|---|---|---|
| `IDF:` | ESP-IDF v5.5.2 (`git describe` = `v5.5.2`, commit `30aaf645`, in `espressif/idf:v5.5.2`, image id `afd00992497f`), paths relative to `components/` | scratch `fwx/idfsrc/` |
| `EX:` | the same tree, paths relative to `examples/storage/nvs/` | scratch `r4/idfsrc/examples_storage_nvs/` |
| `INK:` | inkemu at `main` (`3229474`), paths relative to `crates/soc-esp32c3/src/`; binary `target/release/inkemu` built from that commit | repository |
| Run P1 | `flash_nvs.bin` (§A.3), plain QEMU boot, 42 s of host time, UART only | `r4/out/p1.uart` |
| Run N1 | `flash_nvs.bin`, FW §Q tracing (one instruction per TB, `-d exec,nochain`, `memory_region_ops_*`, `m25p80_*`), from power-on to the third software reset: 11,522,874 instructions | `r4/out/n1.{flash,mmio,count,tail,uart,img}` |
| Runs N1b1, N1b2, N1f | N1 stopped at the first and at the second reset (flash image after boots 1 and 2); N1 with every MMIO event kept (about 444,000 events) | `n1b1.*`, `n1b2.*`, `n1f.*` |
| Run P2 | a new QEMU process on the image N1b1 left, one life | `p2.*` |
| Run N2 | `flash_nvs_ota.bin` (X4 layout), three lives: 11,549,692 instructions | `n2.*` |
| Runs H2, H2f | the pinned reference `flash_hello.bin` (`fc05ef5d…`), one life, same tracing: 3,402,181 instructions (application entry at 1,683,222, the index of the earlier reports) | `h2.*`, `h2f.*` |
| Run E1 | a 31-line probe application written for this report (`r4/src/erase_ota/main/main.c`, scratch only) in the X4 layout, one life | `e1.*` |
| Runs I1–I4 | inkemu at `main` on both images, tolerant (`--max-time 24`, `34.5`) and `--strict-mmio` | `ink_*.out`, `ink_*.err` |

Instruction indices are QEMU indices as in FW; they order events and are not reproducible to the unit (the third reset of N1 fell at 11,522,871, 11,522,874 and 11,519,918 in three repeats). The flash commands, the written bytes, the flash image and the first boot's UART bytes were identical in every repeat **(M)**.
QEMU source was not read for this report; QEMU facts quoted from FW keep FW's tag.

## 1. Key results

1. **The restart-counter example of v5.5.2 is `examples/storage/nvs/nvs_rw_value_cxx`** (C++). The C example `nvs_rw_value` was rewritten in April 2025: it no longer counts restarts and never restarts; only its README still says so **(R)**.
2. Built with the reference `sdkconfig.defaults` **unchanged**, without network, it is reproducible: three clean builds (three paths, with and without ccache, one through the reference script's environment) give the same `nvs_rw_value_cxx.bin`, and `bootloader.bin` and `partition-table.bin` are byte-identical to the two files inkemu already pins **(M)**.
3. Three lives in the single-app layout send `AB 9F 05 5A 7A 04 BB 06 02` and nothing else: **no erase of any kind**, no `03`, no `01`, no `D8`/`52`, no chip erase. Every write is a page program in flash sector 9 (`0x9000`): 5 programs on the first boot (104 bytes), 3 on each later boot (40 bytes) **(M, N1: 1,223 commands)**.
4. The written bytes are deterministic and listed in §B.2. A boot appends one 32-byte entry and rewrites one 4-byte bitmap word twice; an old entry is "erased" by clearing two bits, its bytes stay **(M)**.
5. The console of boot 1 says `The value is not initialized yet!`; boot *n* says `Restart counter = n−1`. That line is the only console evidence that a write survived a reset **(M)**.
6. **inkemu at `main` prints the complete first-boot console of QEMU while refusing every data command** (`0xbb`, `0x02`, and an erase `0x20` that QEMU never sees), then prints `The value is not initialized yet!` again at boots 2 and 3. A test that pins the first boot's console proves nothing about flash writes **(M, I1–I2)**.
7. Data reads: `BB`, 24 address bits, 4 dummy clocks, at most 64 bytes, through `USR`, exactly as FW measured for Arduino. NVS reads every blank page in full at every boot: 24,768 bytes (390 transfers) on the first boot, 21,284 bytes (346) afterwards **(M)**.
8. One life is 11.02 s of device time under QEMU: the boot (`app_main` is called at log time 30 ms) and eleven one-second delays **(M)**.
9. ESP-IDF erases a 64 KiB block (`CMD = 0x00800000`, opcode `D8`) for each step of an erase where at least 64 KiB remain and the address is a multiple of 64 KiB, a 4 KiB sector (`CMD = 0x01000000`, `20`) otherwise: one call gave `20`, `D8`, `20`. `nvs_flash_erase` is five or six `20` in the two layouts; `52` is never sent **(R+M, E1)**.
10. `esp_ota_set_boot_partition` erases and programs the **other** `otadata` sector (32 bytes, state `FFFFFFFF` when rollback is off); `esp_ota_mark_app_valid_cancel_rollback` writes only if the active entry is not already VALID, which the stock bootloader makes it on a first boot **(R+M, E1)**.
11. Registers: against the reference `hello_world` life the example adds exactly **SPI1 `W1`–`W15`** as new addresses; in the application phase also a write of `W0`, a read of `SPI1.SUS_STATUS`, a read of MMU entry 0 and a write of MMU entry 3. Boots 2 and 3 touch nothing boot 1 does not. inkemu at `main` already stores or accepts all of them: its only named warnings are flash commands **(M)**.
12. At `main` the SPI1 controller hands a hard-wired program or erase to the chip without `ADDR` and without `W0..` (`INK:periph/spi_mem.rs:244-250`), and a comment and a test there describe ESP-IDF programming through `USR | FLASH_PE`, a path it takes only above 16 MiB (FW §B.1): the measured program is `CMD = 0x02000000` with `ADDR = len << 24 | addr` **(R+M)**.
13. Recommended CI firmware: this example, unmodified, reference config, both layouts; pins and assertions in §G.

---

## A. The example and its build

### A.1 Which example (R)

| Fact | Source |
|---|---|
| `EX:nvs_rw_value_cxx/main/nvs_value_example_main.cpp` (89 lines, `Unlicense OR CC0-1.0`): `nvs_flash_init` (and `nvs_flash_erase` + retry only on `ESP_ERR_NVS_NO_FREE_PAGES` or `ESP_ERR_NVS_NEW_VERSION_FOUND`); `nvs::open_nvs_handle("storage", NVS_READWRITE)`; `get_item("restart_counter", int32_t)`; increment; `set_item`; `commit`; eleven `printf("Restarting in %d seconds...")` from 10 to 0, each followed by `vTaskDelay(1000 / portTICK_PERIOD_MS)`; `esp_restart()` | `:29-36, 42, 51, 66-67, 75, 82-88` |
| Project files: `CMakeLists.txt` (`MINIMAL_BUILD ON`, project `nvs_rw_value_cxx`), `main/CMakeLists.txt` (`PRIV_REQUIRES nvs_flash`), an empty `sdkconfig.ci`, no `sdkconfig.defaults`, no managed component | directory listing |
| Espressif's own test expects, four times in a row, `Opening … Done`, `Reading restart counter from NVS ... The value is not initialized yet!` (first loop) or `... Done` and `Restart counter = i`, `Updating … Done`, `Committing … Done`, `Restarting in 10 seconds...`, on `esp32` and `esp32c3` | `EX:nvs_rw_value_cxx/pytest_nvs_rw_value_cxx.py:11-23` |
| `EX:nvs_rw_value/main/nvs_value_example_main.c` (161 lines) writes `counter = 42` and a string, lists the keys, erases the key, commits and **returns from `app_main`**: no counter of restarts, no restart. Its README still describes the old behaviour | `:83-85, 107, 140, 150, 159`; `README.md:10`; `git log`: `24065c6bbd` (2025-04-03, "update nvs_rw_blob and nvs_rw_value examples"), moved under `storage/nvs/` by `e4e56a4687` |
| No other example of `examples/storage` or `examples/system` both counts restarts in NVS and restarts by itself | `grep -rn "restart_counter\|esp_restart" examples` |

### A.2 Build and reproducibility (M)

Discipline of `tools/reference-firmware`: the example copied to the container's own file system, the reference `sdkconfig.defaults` (six lines) as its only configuration, `idf.py set-target esp32c3`, `idf.py build`, `IDF_COMPONENT_MANAGER=0`, container without network. Nothing had to differ.

| Build | Path, environment | `nvs_rw_value_cxx.bin` |
|---|---|---|
| c1 | `/tmp/b1/nvs_rw_value_cxx`, exported environment, ccache as the image sets it | `44523a2fd8f7dc0a3eaad17a8991c647ab663ad993dc0db2e31235806b516ff2` |
| c2 | `/tmp/other/proj_y`, `IDF_CCACHE_ENABLE=0`, `CCACHE_DISABLE=1` | identical (`cmp`), `sdkconfig` identical |
| c3 | `/tmp/nvs_rw_value_cxx`, through the image entrypoint as `in-container.sh` runs | identical |

| File | Size | SHA-256 |
|---|---|---|
| `bootloader/bootloader.bin` | 21,040 | the pinned `601b48456b0a9d5c9f1beac166135e06686d3c5ed0309dd16840ab39887afaa3` (`sha256sum -c SHA256SUMS.inputs`: OK) |
| `partition_table/partition-table.bin` (single app: `nvs` `0x9000`/`0x6000`, `phy_init` `0xF000`, `factory` `0x10000`/1 MiB) | 3,072 | the pinned `7f00b6c042a89b15b0cac534f82ed988caf29278ff5700b0c511eb1b5bb7c820` (OK) |
| `nvs_rw_value_cxx.bin` | 161,312 | `44523a2fd8f7dc0a3eaad17a8991c647ab663ad993dc0db2e31235806b516ff2` |

Application image (`fwx/tools/imgdump.py`): entry `0x40380330` (the same address as `hello_world`), 6 segments, DROM `0x3C020020` (28,536 bytes), IROM `0x42000020` (79,608 bytes, file offset `0x10018`, so flash page 2), DIO, 16 MB, 80 MHz, chip id 5, revisions 3 to 199, checksum and appended digest valid, `app_desc`: project `nvs_rw_value_cxx`, version `1`, IDF `v5.5.2`, no build date, ELF SHA-256 `27fa7af2…`.
Resulting `sdkconfig`: log level INFO, console on UART0 at 115,200, `CONFIG_PARTITION_TABLE_SINGLE_APP`, app rollback off, `CONFIG_SPI_FLASH_ROM_IMPL` and `CONFIG_SPI_FLASH_AUTO_SUSPEND` off, `CONFIG_FREERTOS_HZ=100`, C++ exceptions and RTTI off, interrupt watchdog 300 ms, task watchdog 5 s.

### A.3 Flash images (M; `esptool.py v4.11.dev1 merge_bin --fill-flash-size 16MB --flash_mode dio --flash_freq 80m --flash_size 16MB`, the reference command)

| Image | `0x0` | `0x8000` | `0x10000` | SHA-256 |
|---|---|---|---|---|
| `flash_nvs.bin` | pinned bootloader | pinned single-app table | `nvs_rw_value_cxx.bin` | `c3f0eed129d85de6c0f959d481bb99101d14d18ec6fab8bfd11c541128ec757f` (also the output of build c3) |
| `flash_nvs_ota.bin` | same | `tests/fixtures/partitions/x4-stock.bin` (`bd0f7954…fff6ce`) | same | `87832c6d5d15672fb3892bfad281688fc7486b360a25b495bdb6a27d04f28fec` |

The X4 variant needs no second build: neither the bootloader nor the application embeds the table (TO §2); `nvs` is `0x9000`/`0x5000` there, five pages instead of six.

## B. Three lives under QEMU

### B.1 Flash commands per boot and phase (M, N1 and N2; counts as QEMU issues them)

| Phase | Boot 1 (erased NVS) | Boot 2 | Boot 3 |
|---|---|---|---|
| ROM | `AB` (1), `9F` (1), `05` (2) | same | same |
| Bootloader, single app | `9F` (1), `5A` (1), `7A` (1), `05` (3), `04` (1) | same | same |
| Bootloader, X4 layout | the same plus `05` (10), `06` (2), `20` (1, `0xE000`), `02` (1, `0xE000`, 32 bytes): the `otadata` update of FW §E, same bytes | as single app: no write | as single app |
| Application, single app | `9F` (3), `BB` (390), then 5 × program: 438 commands | `9F` (3), `BB` (346), 3 × program: 376 | identical counts: 376 |
| Application, X4 layout | `9F` (3), `BB` (325), 5 × program: 373 | `9F` (3), `BB` (281), 3 × program: 311 | 311 |

One program is, in QEMU, `05`, `06`, `05`, `05`, `02`, `05`, `05`, `04`, `05`: the last three commands are the artefact FW key result 5 names (WEL still 1, so ESP-IDF "ensures WEL is 0"). With a chip that clears WEL at completion and answers WIP = 0 at the first poll, the same program is `05`, `06`, `05`, `05`, `02`, `05` **(A, from FW §A.2)**: 423 commands instead of 438 for the first boot's application phase, 367 instead of 376 afterwards. Further artefacts **(M)**: `5A` and `7A` are unknown commands for QEMU's chip (FW §F row 3); in the X4 layout the application's first status polls read `0x02`, because QEMU left WEL set after the bootloader's program; QEMU resets its flash model at each software reset (`m25p80_reset_done` in the trace); a CPU reset does not reach a real chip (A).
The program trigger is at `0x4038671c` (`spimem_flash_ll_program_page+0x6a`), the read trigger at `0x40386ac4` (`spi_flash_hal_read+0x66`), both in IRAM. `SPI1.CMD` takes only these values in the application: `0x00040000` (`USR`), `0x02000000` (`PP`), `0x40000000` (`WREN`), `0x20000000` (`WRDI`) **(M, N1f, every write)**. Flash windows (bus acquisitions, each with a cache suspend and resume, FW §B.3): 63, 73 and 73 in the three boots, against 4 in `hello_world`.

### B.2 Exact bytes written (M; identical in N1, N1f, N1b1, N1b2, P2 and, for the NVS rows, N2)

| Boot | # | Address | Len | `ADDR` register | Data (first byte first) | Meaning (R `IDF:nvs_flash/src/nvs_page.cpp:800-845`, `private_include/nvs_constants.h:23-66`) |
|---|---|---|---|---|---|---|
| 1 | 1 | `0x9000` | 32 | `0x20009000` | `FE FF FF FF 00 00 00 00 FE` then 19 × `FF` then `84 2D BA B9` | page header: state ACTIVE, sequence 0, version `FE`, CRC |
| 1 | 2 | `0x9040` | 32 | `0x20009040` | `00 01 01 FF 09 A9 50 07` `73 74 6F 72 61 67 65` 9 × `00` `01` 7 × `FF` | entry 0: namespace `storage` gets index 1 (type u8) |
| 1 | 3 | `0x9020` | 4 | `0x04009020` | `FE FF FF FF` | bitmap: entry 0 WRITTEN |
| 1 | 4 | `0x9060` | 32 | `0x20009060` | `01 14 01 FF 81 A8 FF 42` `72 65 73 74 61 72 74 5F 63 6F 75 6E 74 65 72 00` `01 00 00 00 FF FF FF FF` | entry 1: `restart_counter`, i32 = 1 |
| 1 | 5 | `0x9020` | 4 | `0x04009020` | `FA FF FF FF` | bitmap: entry 1 WRITTEN |
| 2 | 1 | `0x9080` | 32 | `0x20009080` | as entry 1 with CRC `62 AF 70 CC` and value `02 00 00 00` | entry 2: i32 = 2 |
| 2 | 2 | `0x9020` | 4 | | `EA FF FF FF` | bitmap: entry 2 WRITTEN |
| 2 | 3 | `0x9020` | 4 | | `E2 FF FF FF` (after a 32-byte `BB` read of `0x9060`) | bitmap: entry 1 ERASED |
| 3 | 1 | `0x90A0` | 32 | `0x200090A0` | CRC `FC AF DA 00`, value `03 00 00 00` | entry 3: i32 = 3 |
| 3 | 2, 3 | `0x9020` | 4, 4 | | `A2 FF FF FF`, then `82 FF FF FF` (after a read of `0x9080`) | entry 3 WRITTEN, entry 2 ERASED |

Flash content after each boot, single-app image (**M**: the images N1b1, N1b2 and N1 left; a replay of the programs with AND gives the same bytes):

```
after boot 1  sha256 cfab4ffe5187eeea8ec2ee6e9794886289282f0242ce56aad2842144a6ffb17b
0x009000  fe ff ff ff 00 00 00 00 fe ff ff ff ff ff ff ff  ff ff ff ff ff ff ff ff ff ff ff ff 84 2d ba b9
0x009020  fa ff ff ff ff ff ff ff ff ff ff ff ff ff ff ff  ff ff ff ff ff ff ff ff ff ff ff ff ff ff ff ff
0x009040  00 01 01 ff 09 a9 50 07 73 74 6f 72 61 67 65 00  00 00 00 00 00 00 00 00 01 ff ff ff ff ff ff ff
0x009060  01 14 01 ff 81 a8 ff 42 72 65 73 74 61 72 74 5f  63 6f 75 6e 74 65 72 00 01 00 00 00 ff ff ff ff
after boot 2  sha256 7ad0acf62e5a4e20aa6d47be0f1d922a7b89cb204941feac1ab2fea71363b709   0x9020 = e2 ff ff ff, and
0x009080  01 14 01 ff 62 af 70 cc 72 65 73 74 61 72 74 5f  63 6f 75 6e 74 65 72 00 02 00 00 00 ff ff ff ff
after boot 3  sha256 fc23c7ecb700e966c0ef9db3becc6b73e37324b5a21e5380519fe597542143f5   0x9020 = 82 ff ff ff, and
0x0090a0  01 14 01 ff fc af da 00 72 65 73 74 61 72 74 5f  63 6f 75 6e 74 65 72 00 03 00 00 00 ff ff ff ff
```

After boot 3, 115 bytes differ from `flash_nvs.bin`, all in these six rows. X4 layout: the same rows plus `0x00E000 = 01 00 00 00`, 20 × `FF`, `02 00 00 00 9A 98 43 47` from the bootloader on boot 1; whole-image SHA-256 after boots 1, 2, 3: `a9709319…f4043e`, `1baf0e66…981cb6`, `6c668205970fc21e7330b840b44e47ca1a2a245f22d93ef1cb0f239a3b89de5b` (the last is N2's image, the first two are replayed).
When the bytes are written **(M, N1f, UART writes and program triggers on one time line)**: on boot 1, programs 1–3 happen inside `open_nvs_handle` (before `Opening … Done` is printed), 4–5 inside `set_item`; `commit` writes nothing; `nvs_flash_init` writes nothing. On later boots the three programs happen inside `set_item`. The last write is 11 s before the reset.
**Cold start on a written image (M, P2):** a new QEMU process on the image boot 1 left prints `rst:0x1 (POWERON)`, then `Reading restart counter from NVS ... Done` and `Restart counter = 1`, sends the three programs of "boot 2" and leaves the image `7ad0acf6…`: a power cycle and a software reset are the same to the example.
No NVS erase can occur in a short test **(R, count A)**: a page holds 126 entries, a life consumes one, and `PageManager::requestNewPage` erases a page only when a single free page is left (`IDF:nvs_flash/src/nvs_pagemanager.cpp:146-187`, `nvs_page.cpp:1061-1063`), that is after roughly 125 lives per page times the number of pages minus one.

### B.3 Console (M; full captures `n1.uart`, `n2.uart`; `\r\n` line ends)

Boot 1 is 71 lines: the 9 ROM lines and 44 `I (…)` lines of the reference boot (TO §2; they differ from `hello_world` only in the six segment lines, `Project name:     nvs_rw_value_cxx`, `ELF file SHA256:  27fa7af26...`, `heap_init: At 3FC8DF30 len 000320D0 (200 KiB): RAM`, and timestamps up to 2 ms later), ending with `I (30) main_task: Calling app_main()`, then:

```
(empty line)
Opening Non-Volatile Storage (NVS) handle... Done
Reading restart counter from NVS ... The value is not initialized yet!
Updating restart counter in NVS ... Done
Committing updates in NVS ... Done
(empty line)
Restarting in 10 seconds...        (eleven lines, 10 down to 0)
Restarting now.
```

Boots 2 and 3 are 72 lines each: `rst:0xc (RTC_SW_CPU_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)` in the ROM banner, and the third application line replaced by `Reading restart counter from NVS ... Done` and `Restart counter = 1` (boot 2) or `Restart counter = 2` (boot 3).
X4 layout, boot 1 is 76 lines: six partition rows instead of three, `I (2) boot: No factory image, trying OTA 0`, and `I (21) boot: Set actual ota_seq=1 in otadata[0]` after `Loaded app from partition at offset 0x10000`. **Both lines are printed on boot 1 only**: boots 2 and 3 (75 lines each) find a valid `otadata` and print neither.
Timestamps: those of boot 1 are identical in every run (`I (0)` … `I (30)`; SHA-256 of the 71 lines without `\r`: `870f0be8…76b45`). After a restart they start at `I (22070)` and `I (12523)` and move by 1 ms between runs: QEMU's cycle counter is not reset with the CPU (IR §7 question 7), so they are not device facts and cannot be pinned.

### B.4 Device time (M)

`SYSTIMER` unit 0 (16 MHz, not reset by QEMU's CPU reset) reads `0x0A8287CE`, `0x15051AFE`, `0x1F87AA57` just before the three resets: 11.0204 s, 22.0409 s, 33.0615 s, so successive lives last 11.0205 s and 11.0206 s. **One life is 11.02 s**: the boot (`app_main` at log time 30 ms), a few milliseconds of NVS work, and eleven delays of 100 ticks of 10 ms, the first one cut at a tick boundary. On hardware the delays are the same eleven seconds and the boot part is whatever the device takes **(A)**. inkemu at `main` takes 11.247 s of device time from power-on to the second ROM banner (found by bisection on `--max-time`; its log calls `app_main` at `I (224)`), and runs three lives (`--max-time 34.5`) in 3.5 s of host time.

## C. The read path, and the commands that never occur

| Item | Value | Tag |
|---|---|---|
| Opcode and trigger | `BB` through `USR` (`CMD = 0x00040000`); `USER2 = 0x700000BB`, `USER1 = 0x5C000003` (24 address bits, 4 dummy clocks), `USER = 0xF0000080`, `CTRL = 0x008C2008`, `ADDR` = flash offset unshifted (`0x00009000`), `MISO_DLEN` = `0x1FF`, `0xFF`, `0x1F` for 64, 32, 4 bytes | M (N1f); same as FW §A.2 |
| Maximum length | 64 bytes: a 100-byte `esp_partition_read` is `BB` 64 + `BB` 36, not split at the 256-byte page | M (E1) |
| Mode | from the image header (`I (29) spi_flash: flash io: dio`) | M |
| What NVS reads at start | per page a 32-byte header (`nvs_page.cpp:39`); a page whose state is UNINITIALIZED is then read in full, eight 512-byte blocks, to check it is blank (`:44-67`): 64 transfers of 64 bytes; the active page: header, 32-byte bitmap at `+0x20`, then entries of 32 bytes | R+M |
| Totals, single app | boot 1: 6 × (32 + 4,096) = 24,768 bytes, 390 transfers; later boots: 5 × 4,128 + 644 = 21,284 bytes (320 of 64, 25 of 32, 1 of 4), one of them between two programs | M |
| Image validation by an application (`esp_ota_set_boot_partition`) | headers through `BB` (24 bytes at `0x10000`, 8 at each of the six segment headers and at the checksum, 32 at the appended digest), segment data through the cache window | M (E1) |
| Hard-wired `FLASH_READ` (`CMD` b31, `03`) | never written, in any phase of N1, N2, E1, H2 | M |
| Status write `01` (`CMD` b26 or `USR`) | never. The bootloader's unlock reads a status of 0 and skips it (FW §A.3) | M |
| `D8` | never in the example (no erase at all); only in the probe E1 | M |
| `52` (32 KiB erase) | no definition and no caller in ESP-IDF's flash driver (`IDF:spi_flash/include/spi_flash/spi_flash_defs.h:42-46` has `C7`, `20`, `21`, `D8`, `DC` only) | R |
| Chip erase (`CMD` b22) | never; `esp_flash_erase_chip` is `esp_flash_erase_region(0, size)` (`IDF:spi_flash/esp_flash_api.c:629-640`) | M, R |
| The example's only erase path | `nvs_flash_erase()` when `nvs_flash_init` returns `NO_FREE_PAGES` or `NEW_VERSION_FOUND` (`EX:…main.cpp:30-35`): `esp_partition_erase_range(nvs, 0, size)` (`IDF:nvs_flash/src/nvs_api.cpp:206, 234-236`), six `20` in the single-app layout (the partition lies inside one 64 KiB block), five in the X4 layout (M, E1) | R, M |

## D. Sector or block: what `esp_flash_erase_region` chooses

All three entry points end in the same loop **(R)**: `nvs_flash_erase` → `esp_partition_erase_range` (`IDF:esp_partition/partition_target.c:123-142`: offset and size multiples of 4,096, inside the partition) → `esp_flash_erase_region(chip, start, len)`.

| Step | Fact | Source |
|---|---|---|
| Sizes | `sector_size` 4 KiB, `block_erase_size` 64 KiB for the ISSI driver, as for the generic one | `IDF:spi_flash/spi_flash_chip_issi.c:82-85`, `spi_flash_chip_generic.c:725-728` |
| Rule, evaluated again at every step | `if (block_erase_size > 0 && len_remain >= block_erase_size && (erase_addr % block_erase_size) == 0)` erase a block and advance 64 KiB, else erase a sector and advance 4 KiB. `CONFIG_SPI_FLASH_BYPASS_BLOCK_ERASE` (default off) removes the first branch | `IDF:spi_flash/esp_flash_api.c:728-742` |
| Around each step | one bus acquisition per sector or block (`start` … `end`, so the cache is resumed between two erases); `WREN`, status polls, erase, polls, as for a program | `:713-762`; `spi_flash_chip_generic.c:212-262` |
| Register level | sector: `ADDR = addr24`, **`CTRL = 0`**, `CMD = 0x01000000`; block: `ADDR = addr24`, `CMD = 0x00800000`, no `CTRL` write; neither sets `FLASH_PE` | `IDF:hal/esp32c3/include/hal/spimem_flash_ll.h:81-95`, `hal/spi_flash_hal_iram.c:42-72`; M (E1) |

Measured with the probe (E1, X4 layout, `spiffs` at `0xC90000`) **(M)**:

| Call | Commands at the chip |
|---|---|
| `esp_partition_erase_range(spiffs, 0xF000, 0x12000)` | `20` `0xC9F000`, `D8` `0xCA0000`, `20` `0xCB0000` |
| `nvs_flash_erase()` | `20` at `0x9000`, `0xA000`, `0xB000`, `0xC000`, `0xD000` |
| `esp_flash_erase_region(NULL, 0xCC0000, 0x20000)` | `D8` `0xCC0000`, `D8` `0xCD0000` |
| `esp_partition_write(spiffs, 0x100F0, ram, 100)` | `02` 16 bytes at `0xCA00F0`, 64 at `0xCA0100`, 20 at `0xCA0140` |
| the same from a `const` array in flash-mapped rodata, at `0x110F0` | `02` 16, 16, 32, 32, 4 bytes: 32-byte pieces through a RAM buffer, the first one split at the page boundary (FW §A.2 "32 when the source buffer is not in DRAM") |
| `esp_partition_read(spiffs, 0x100F0, back, 100)` | `BB` 64, `BB` 36; the data read back is the data written |

`SPI1.CMD` values of that run's application phase: `0x01000000` × 8, `0x00800000` × 3, `0x02000000` × 9, `0x40000000` × 20, `0x20000000` × 20 (artefact), `0x00040000` × 134. `USER1` reads `0x5C000007` before these erases and `0x5C000003` before N1's programs: its low field is whatever the previous `USR` transfer left and is not part of a hard-wired command **(M)**.

## E. An application that writes `otadata`

By reading (`IDF:app_update/esp_ota_ops.c`):

| Call | Sequence | Lines |
|---|---|---|
| `read_otadata` (start of every call below) | `esp_partition_mmap` of the whole `otadata` partition, two `memcpy` of 32 bytes, `munmap`: mapped reads, no `BB` | `:75-96` |
| `esp_ota_set_boot_partition(p)`, `p` an OTA slot | `image_validate(p)`; pick the **inactive** entry (`(~active) & 1`, entry 0 if none is valid); `ota_state` = `ESP_OTA_IMG_NEW` (0) with `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE`, else `ESP_OTA_IMG_UNDEFINED` (`0xFFFFFFFF`); smallest `seq` ≥ the active one that selects the slot; then `rewrite_ota_seq` | `:599-640, 552-597, 113-121` |
| `rewrite_ota_seq` | `crc = crc32 of ota_seq only`; `esp_partition_erase_range(otadata, sec_id * 4096, 4096)` (one `20`); `esp_partition_write(…, 32 bytes)` (one `02`) | `:526-539`; entry layout `IDF:bootloader_support/include/esp_flash_partitions.h:79-82` |
| `esp_ota_set_boot_partition(factory)` | erases the whole `otadata` partition: two `20` | `:609-617` |
| `esp_ota_mark_app_valid_cancel_rollback()` | if the active entry's state is not `ESP_OTA_IMG_VALID` (whatever it is): state = VALID, `rewrite_ota_seq` of the **active** sector with the same `seq`; else nothing | `:904-921, 944-947` |
| `esp_ota_mark_app_invalid_rollback()` | state = INVALID in the active sector, same erase + program | `:922-934` |

Measured (E1; the bootloader had written `otadata[0]` = seq 1, state VALID on that same boot) **(M)**:
- `esp_ota_mark_app_valid_cancel_rollback()` returns `ESP_OK` and sends no program and no erase.
- `esp_ota_set_boot_partition(esp_ota_get_running_partition())` returns `ESP_OK`: `20` at `0xF000`, then `02` of 32 bytes at `0xF000`: `01 00 00 00`, 24 × `FF`, `9A 98 43 47` (seq 1, state UNDEFINED, same CRC as entry 0 since the CRC covers `seq` only). Both sectors then hold seq 1.
The rollback states (NEW → PENDING_VERIFY → VALID) need a bootloader built with app rollback, which is not the pinned one: FW §E measured them with CrossPoint's bootloader (run R1); not repeated here.

## F. Registers: what the example adds to the reference life

### F.1 Difference of the access lists (M; N1, three lives, against H2, one life of `flash_hello.bin`)

| Comparison | (address, kind) pairs | New ones |
|---|---|---|
| ROM + bootloader, N1 boot 1 against H2 | 361 against 361 | none |
| Application, N1 (any boot) against H2 | 420 against 394 | 26, table below |
| N1 boots 2 and 3 against N1 boot 1, all phases | 647 against 651 | none (boot 1 has 4 more: `RTC_CNTL.SLOW_CLK_CONF` read and written, `RTC_CNTL.STORE4` written, `0x6000E044` read) |
| N2 against N1, all phases | 651 against 651 | none; in the bootloader phase only, writes of `SPI1.W0`–`W7` (the ROM's `otadata` program) |
| Addresses in N1 and never in H2, all phases | 474 against 459 | 15: `SPI1.W1` … `W15` |

| Block | Register | Access the example adds | Counts in N1 | inkemu at `main` |
|---|---|---|---|---|
| SPI1 | `W1`–`W15` (`0x6000205C`–`0x60002094`) | read after every `BB` (ROM `memcpy` at `0x4005885c`); `W1`–`W7` written for a program | 1,080 reads each (`W8`–`W15`: 1,024), 5 writes | stored (`INK:periph/spi_mem.rs:198-199`); filled only when the chip accepts the opcode, and `BB` is refused (`INK:flash.rs:295`) |
| SPI1 | `W0` (`0x60002058`) | written by the application (program data); the reference life only reads it | 11 writes | stored |
| SPI1 | `SUS_STATUS` (`0x600020A4`) | read by the application's host-status check (`spi_flash_hal_check_status`); the ROM already touches it | 44 reads, always 0 | accepted without effect, heir SP1b-3b (`docs/registers-accepted-without-effect.md:53`) |
| MMU | entry 0 (`0x600C5000`) | **read**, twice per boot, before the first write: `esp_partition_main_flash_region_safe` → `esp_ota_get_running_partition` → `spi_flash_cache2phys` (`IDF:esp_partition/partition_target.c:222-235`, `app_update/esp_ota_ops.c:714`, `hal/mmu_hal.c:120-135`); reads `0x2` | 6 reads | modelled, reads back (`INK:periph/mmu.rs:89-100`) |
| MMU | entry 3 (`0x600C500C`) | **written** by the application: `esp_mmu_map` of flash page 0 at `0x3C030000` to load the partition table, then unmapped (`0x0`, later `0x100`) | 6 writes | modelled |

The same registers with values the reference life never writes **(M, N1f against H2f, every event)**: `SPI1.CMD` `0x02000000`, `0x40000000`, `0x20000000`; `SPI1.ADDR` with a length in bits 31:24; `SPI1.USER` `0xC0000080`, `0xD0000080`, `0xE0000080`, `0xF0000080`; `USER1 = 0x5C000003`; `USER2 = 0x700000BB`; `CTRL = 0x008C2008` and three transient values; `MISO_DLEN` `0x1F`, `0xFF`, `0x1FF`; `EXTMEM.ICACHE_SYNC_ADDR = 0x3C030000` with `ICACHE_SYNC_SIZE = 0x800` (the invalidation `esp_mmu_map` does, once per boot). The other differences of that comparison are data (SHA input, RNG, SYSTIMER values, the image's own layout) or belong to the restart, which H2 stops at.
Flash page 0, which holds the partition table and the whole NVS partition, is mapped and unmapped by the application before its first write; no invalidation follows a write (no `ICACHE_SYNC` event between the unmap and the restart): FW §D holds for this example **(M)**.

### F.2 The image on inkemu at `main` (M, tolerant mode, `--device x4`)

| Run | Result |
|---|---|
| `flash_nvs.bin`, `--max-time 24` | exit 0. Boot 1 prints the same lines as QEMU (timestamps differ: `I (21) boot: …`, `I (224) main_task: Calling app_main()`), including `Opening … Done`, `The value is not initialized yet!`, `Updating … Done`, `Committing … Done`. Boots 2 and 3 print `The value is not initialized yet!` again where QEMU prints `Restart counter = 1`, `= 2` |
| Named warnings | `flash command 0xbb is not modeled (pc 0x40386ac4)`, `flash command 0x20 is not modeled (pc 0x40386b9c)` (`spi_flash_hal_erase_sector`), `flash command 0x02 is not modeled (pc 0x4038671c)`; over the three boots that run starts: `0xbb` 18 times (six per boot), `0x02` 15, `0x20` 3. **No register warning**; one `reset by the firmware, a CPU reset (rst:0xc)` entry |
| Where it stops being like QEMU | at the first `BB` (the 32-byte header read of `0x9000`): nothing is stored in `W0..`. Per boot it then sends six `BB` instead of 390, **one sector erase** that QEMU never sees, and five programs (M). Reading: each header read returns the stale content of `W0..`, NVS takes a page for corrupt, erases it and goes on as with a blank partition (A) |
| `--strict-mmio` | exit 3 at that first `BB`: `stopped at pc 0x40386ac8`, 231,172 µs of device time |
| `flash_nvs_ota.bin`, `--max-time 24` | warnings `0x20` (pc `0x4004d29a`) and `0x02` (pc `0x4004d6d0`) from the ROM, then `0xbb`; a warning is grouped by opcode under the pc of its first occurrence, so the application's own `0x02` and `0x20` are counted there (per boot: `0x20` twice, `0x02` six times, `0xbb` five). `No factory image, trying OTA 0` and `Set actual ota_seq=1 in otadata[0]` are printed **at every boot** (QEMU: boot 1 only). Strict: exit 3 at the bootloader's `0x20`, 142,361 µs |

What the plan has to add, read against these measurements **(R)**: in the chip, `BB`, `02`, `20`, `D8` (`INK:flash.rs:270-296` knows `AB 9F 05 5A 7A 06 04`), with WEL cleared at completion (FW §C); in the controller, `ADDR[23:0]`, the length `ADDR[31:24]` and `W0..` for the hard-wired `PP`, and `ADDR[23:0]` for `SE` and `BE`, which `run_hard_wired` sends today with `address: None` and `data_out: &[]` (`INK:periph/spi_mem.rs:244-250`). The `USR` read path already carries what `BB` needs (address width, dummy count, 64-byte limit, `W0..` filling: `:291-356`). The comment at `:362-363` ("ORs in `0x60000` … for every program or erase") and the test `an_esp_idf_program_sets_the_flag_with_usr_and_its_opcode_runs_once` (`:780-797`) describe a transfer ESP-IDF v5.5.2 issues only for 4-byte addresses (`IDF:spi_flash/spi_flash_chip_generic.c:118-151`, FW §B.1); no measured boot contains it.

## G. Recommended CI firmware, pins and assertions

| Item | Recommendation |
|---|---|
| Firmware | `$IDF_PATH/examples/storage/nvs/nvs_rw_value_cxx` of ESP-IDF v5.5.2, **unmodified**, built by `tools/reference-firmware` next to `hello_world` in the same container run: copy to `/tmp/nvs_rw_value_cxx`, the same `sdkconfig.defaults`, `idf.py set-target esp32c3`, `idf.py build`. No network, no new configuration line, one to two more minutes of build |
| Why this one | it is Espressif's, it is public domain, its own pytest expects the lines below on an ESP32-C3, it writes on every boot, reads its write back after a reset, and needs no input. Its bootloader and table are the pinned ones, so the two images differ from `flash_hello.bin` only from `0x10000` on |
| Pins, inputs | `nvs_rw_value_cxx.bin` `44523a2fd8f7dc0a3eaad17a8991c647ab663ad993dc0db2e31235806b516ff2` (161,312 bytes); `bootloader.bin` and `partition-table.bin` already pinned |
| Pins, outputs | `flash_nvs.bin` `c3f0eed129d85de6c0f959d481bb99101d14d18ec6fab8bfd11c541128ec757f`; `flash_nvs_ota.bin` (with `tests/fixtures/partitions/x4-stock.bin`) `87832c6d5d15672fb3892bfad281688fc7486b360a25b495bdb6a27d04f28fec`; the symbol table `nvs_rw_value_cxx.nm` |
| Run length | three lives end at 33.1 s of device time under QEMU and near 33.8 s on inkemu at `main`; stop on a console predicate (the third `Restarting now.`) under a bound of 40 s |

Console lines to expect, timestamps masked as the existing tests do (`I (#)`), in this order (§B.3):

| Boot | Lines that identify it |
|---|---|
| 1 | `rst:0x1 (POWERON)`; `Opening Non-Volatile Storage (NVS) handle... Done`; `Reading restart counter from NVS ... The value is not initialized yet!`; `Updating restart counter in NVS ... Done`; `Committing updates in NVS ... Done`; `Restarting in 10 seconds...` … `Restarting in 0 seconds...`; `Restarting now.` |
| 2 | `rst:0xc (RTC_SW_CPU_RST)`; `Reading restart counter from NVS ... Done`; `Restart counter = 1`; the other lines as boot 1 |
| 3 | `rst:0xc (RTC_SW_CPU_RST)`; `Reading restart counter from NVS ... Done`; `Restart counter = 2` |
| X4 layout | on boot 1 only: `I (#) boot: No factory image, trying OTA 0` and `I (#) boot: Set actual ota_seq=1 in otadata[0]`; boots 2 and 3 have neither |

Smallest set of assertions that pins the writes:

1. **Program list of three lives, in order, as (address, bytes)**: the eleven rows of §B.2; in the X4 layout preceded by one sector erase at `0xE000` and one 32-byte program at `0xE000`. No other program, no erase in the single-app layout. Do not count `05` or `04`: their number depends on the chip model (§B.1).
2. **Content**: after boot 3, the six 32-byte rows `0x9000`–`0x90BF` of §B.2 and every other byte equal to the input image (equivalently the SHA-256 `fc23c7ec…2143f5`; X4: `6c668205…89de5b`). After boot 1 alone: `cfab4ffe…ffb17b`.
3. **NOR rule**, free with 1 and 2: `0x9020` goes `FF` → `FE` → `FA` → `EA` → `E2` → `A2` → `82` by programs of an already programmed word, with no erase.
4. **Persistence across processes** (the flash file): one life, stop, start again from the saved file: `rst:0x1 (POWERON)` followed by `Restart counter = 1`, and the file after that life has the "after boot 2" content (P2).
5. **Strict mode** runs three lives of both images without a refusal (today: exit 3 at the first `BB`, or at the bootloader's `0x20`).
6. **A read after a write inside one boot**: covered by `Restart counter = n` only across a reset; within a boot the example reads `0x9060` between its second and third program (boot 2), which a model that returned stale data would not show on the console. Pin it through assertion 1's order or leave it to the lockstep.

Not covered by this firmware, and where to get it: an erase from the application and a block erase (`D8`): the probe of §D, or a unit test of the controller with `CMD = 0x01000000` and `0x00800000`; programs of 64 bytes and the page-boundary split: the same probe (§D rows 4–5), or a unit test; `otadata` written by an application: §E.

## H. Relation to FW

| FW statement | Here |
|---|---|
| Key result 3, §A.2: `BB`, 24 bits, 4 dummy clocks, ≤ 64 bytes, application only | confirmed for a pure ESP-IDF application, with the same register values |
| Key result 4, §A.2: a program is ≤ 64 bytes, never crosses a page, 32-byte pieces from a buffer outside DRAM | confirmed and measured in both forms (§D) |
| Key result 5: the `RDSR`, `WRDI`, `RDSR` artefact after every program or erase | confirmed: 9 commands per program in QEMU; also seen after erases (E1: 20 `WRDI` for 20 operations) |
| §A.2 "Block erase … chosen by `esp_flash_erase_region` when the range allows it" (R) | the rule is quoted (§D) and measured: `CMD = 0x00800000`, `D8` |
| §A.2 sector erase "`USER1 = 0x5C000003`" | the value before an erase was `0x5C000007` in E1: a leftover of the previous transfer, not a property of the erase. A refinement, not a contradiction |
| §E "`nvs_flash_init` reads the five pages and writes nothing; the first write comes with the first key" | confirmed; the first write here comes with the first namespace (`open_nvs_handle` in read-write mode) |
| §E first row: the bootloader's `otadata[0]` bytes | identical bytes with this application (N2) |
| §E "Application, same case" (A: the Arduino start-up code marks the image valid) | by reading, any state other than VALID makes `esp_ota_mark_app_valid_cancel_rollback` rewrite the active sector (§E): consistent with FW's measurement |
| §D: invalidation after a write only for pages mapped by `esp_mmu_map` | confirmed in a case where the written flash page had been mapped and unmapped by the application itself (§F.1) |

Nothing measured here contradicts FW. Two statements outside FW are contradicted: the premise that the stock restart-counter example is `nvs_rw_value` (§A.1), and the comment and test in `INK:periph/spi_mem.rs` about `USR | FLASH_PE` programs (§F.2). The carry-over note of SP1b-3a lists data reads "`0x03`, `0x0B`, `0xBB`": only `BB` occurs with images whose header says DIO.

## O. Open questions

| # | Question | Why it matters | Status |
|---|---|---|---|
| 1 | Every M of §B–§E is QEMU's chip: WEL never clears by itself, every command completes at once. The command lists "without artefacts" (§B.1) are derived, not measured | a test that counts `05`/`04` would pin QEMU, not a chip; a model with real durations sends more `05` | assumed from FW §A.2, §C; written bytes do not depend on it |
| 2 | Console timestamps and the duration of a life on a device | only the eleven one-second delays are certain | QEMU 11.02 s; hardware not measured |
| 3 | The first NVS page change and the first NVS erase (after about 125 lives, and about 625 in the single-app layout) | long-running persistence tests would meet a 4-byte state program at `0x9000`, a new header at `0xA000`, later a `20` | read only; counts assumed |
| 4 | Rollback states written by an application (NEW, PENDING_VERIFY, VALID, INVALID) with a rollback-enabled bootloader | CrossPoint's bootloader has it enabled (FW §I) | read (§E); FW R1 measured one case; not repeated |
| 5 | The opcode the controller sends for `FLASH_BE` on silicon | FW §A.1: `D8` by inference | assumed |
| 6 | Reproducibility of the build on another host | measured three times on one host, one Docker engine | TO §2 measured the same property for `hello_world` |
| 7 | Whether inkemu, once it models these commands, touches registers this report did not see (a different timing changes interleavings, not code paths) | the tolerant run at `main` diverges at the first `BB`, so its register list after that point is not the example's | QEMU's list (§F.1) is the reference |

## Q. Artefacts and reproduction (scratch, not in the repository)

Directory `…/scratchpad/r4/`: `tools/` (scripts written for this report), `img/` (composed images), `out/` (builds under `out/bld/`, traces, UART captures, flash images after each run, inkemu outputs), `src/erase_ota/` (the probe), `idfsrc/` (the two example directories). FW's tools are reused from `…/scratchpad/fwx/tools/` (`imgdump.py`, `qboot.sh`, `first_access.py`); `trace_filter2.py` is FW's `trace_filter.py` with the full program data and a kill of QEMU at the reset that ends a run (QEMU does not stop when its trace reader does; an earlier attempt left it running and its image and UART file were discarded); `regdiff.py`, `phasediff.py` and `valdiff.py` compare the per-phase lists `first_access.py` prints.
One container for the whole session, removed at the end: `docker run -d --name inkemu-r4 --network none --entrypoint sleep -v …/oracle:/w:ro -v …/rom/x:/rom:ro -v …/fwx/tools:/fwtools:ro -v …/r4/tools:/tools:ro -v …/r4/src:/src:ro -v …/r4/img:/img -v …/r4/out:/out -v …/r4/idfsrc:/idfsrc -v <repo>/tools/reference-firmware:/ref:ro -v <repo>/tests/fixtures/partitions:/parts:ro espressif/idf:v5.5.2 infinity`. No network was needed at any step. Nothing built was executed on the host; the host ran Python (`-I`) on text and images, and inkemu itself.

| Step | Command (`docker exec inkemu-r4 bash …` unless "host") |
|---|---|
| Builds c1, c2 | `/tools/build.sh /opt/esp/idf/examples/storage/nvs/nvs_rw_value_cxx nvs_rw_value_cxx c1 /tmp/b1/nvs_rw_value_cxx 1`; `… c2 /tmp/other/proj_y 0` |
| Build c3 | `docker exec -e IDF_COMPONENT_MANAGER=0 inkemu-r4 /opt/esp/entrypoint.sh bash /tools/build_like_ref.sh` |
| Compose | `/tools/compose.sh c1 nvs_rw_value_cxx flash_nvs.bin`; `… flash_nvs_ota.bin /parts/x4-stock.bin` |
| P1 | `/fwtools/qboot.sh flash_nvs.bin p1 42` |
| N1, N2, H2 | `/tools/qtrace2.sh flash_nvs.bin n1 300 40380330 3 1 3`; `… flash_nvs_ota.bin n2 …`; `… flash_hello.bin h2 300 40380330 1 1 3` (about 50 s for three lives) |
| N1f, H2f, N1b1, N1b2, P2 | the same with a last argument of `100000000`; with 1 or 2 resets; with `flash_nvs_after1.bin` (a copy of `out/n1b1.img`) and 1 reset |
| Probe E1 | `/tools/build.sh /src/erase_ota erase_ota e1 /tmp/e1/erase_ota 1`; `/tools/compose.sh e1 erase_ota flash_eo.bin /parts/x4-stock.bin`; `/tools/qtrace2.sh flash_eo.bin e1 200 40380330 1 1 100000000` |
| Reduce (host) | `python3 -I …/fwx/tools/first_access.py out/n1.count <svd> b0p2` (279 registers; the same on `out/h2.count`: 261; the 18 names only in the first list are `SPI1.W1`–`W15`, `SPI1.SUS_STATUS`, `MMU[0]`, `MMU[3]`); `python3 -I tools/flashlog.py out/n1.flash --replay img/flash_nvs.bin out/n1.img`; `tools/states.py out/n1.flash img/flash_nvs.bin check:0=out/n1b1.img check:1=out/n1b2.img check:2=out/n1.img`; `tools/regdiff.py out/n1.count out/h2.count <repo>/third_party/espressif-svd/esp32c3.svd`; `tools/phasediff.py out/n1.count 'b.p2' out/h2.count 'b0p2' <svd>`; `tools/valdiff.py out/n1f.mmio out/h2f.mmio <svd>`; `tools/sym.py out/bld/c1/nvs_rw_value_cxx.nm <pc>…` |
| inkemu (host) | `INKEMU_ROM_ELF_DIR=…/rom/x target/release/inkemu run --device x4 --flash …/r4/img/flash_nvs.bin --max-time 24` (also `34.5`, `--strict-mmio`, and `flash_nvs_ota.bin`), output and exit status to files |

Checksums (SHA-256): `out/n1.uart` `bb522daf037e…8cc8`, `out/n2.uart` `dc65fa6a845b…4ed6`, `out/p1.uart` `e5c8bf3deb42…516c`, `out/e1.uart` `192b5e7bceca…4c05`, `out/n1.img` `fc23c7ecb700…43f5`, `out/n2.img` `6c668205970f…de5b`, `out/e1.img` `27bc15d42d2d…126c`, `out/n1.flash` `93f652477e05…6a78`, `out/n2.flash` `c93b4649d028…84f9`, `out/e1.flash` `5aac53c690ac…ba83`, `out/n1.count` `5d0a8bba6274…5532`, `out/h2.count` `b357d3d90e81…3a67`, `out/n1_app_first.txt` `8a4870d29deb…a347`, `out/h2_app_first.txt` `e894312abb5a…c025`, `img/flash_eo.bin` `81f7ad80fa19…3232`, `out/bld/e1/erase_ota.bin` `b25bd0ad754d…264d`, `src/erase_ota/main/main.c` `23cbff975bb7…a28b`. The `.flash`, `.count` and `.mmio` files carry instruction indices and a host pointer and change from run to run; the images and the first boot of each UART capture do not.
Licence hygiene: ESP-IDF facts come from its Apache-2.0 sources; the example is `Unlicense OR CC0-1.0`; console lines, hashes and a few quoted expressions are reproduced, no code. QEMU was used as a tool and its source was not read.
