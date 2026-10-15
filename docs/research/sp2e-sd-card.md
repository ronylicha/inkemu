# The SD card of the X4 (SP2e): SPI mode, CrossPoint's driver, the volume it needs

Date 2026-10-15, branch of SP2e from `main` at `4b02a91`. Tags as in `sp2a-external-chips.md`:
**[SPEC]** read in the SD Association's specification, **[SRC]** read in the code of a firmware that
drives the X4 or of the library it builds, **[INF]** reasoning, **[HYP]** chosen here, kept as a
parameter of the model or an `ASSUMPTION:` in the code. Earlier work this report builds on and does not
repeat: `docs/research/sp2d-x4-wiring.md` (§1 items 1 and 10, §2: the pins and the boot decision of
CrossPoint), `docs/research/xteink-hardware.md` §1.1. The design that rules on what follows is
`docs/superpowers/specs/2026-10-15-sp2e-sd-card-design.md`.

## 0. Sources

| Short name | What | Where, pinned |
|---|---|---|
| **PLS** | SD Specifications Part 1, Physical Layer Simplified Specification, Version 9.10 (2023-12-01) | `sdcard.org`, file `Part1PhysicalLayerSimplifiedSpecificationVer9.10Fin_20231201.pdf`, SHA-256 `ee86b20ec02b30797f14ceaf9f760feadb321de12cceb278b917aab7a65380e3`, read on 2026-10-15. Chapter 7 is the SPI mode; §4.3.9 (application commands), §4.3.10 (switch function), §4.5 (CRC), §4.6.2 (timeouts), §5.1–5.3 and §5.6 (registers). The timing tables of SPI mode (§7.5) are removed from the simplified edition. |
| **SF** | SdFat 2.3.1, Bill Greiman, MIT | `greiman/SdFat`, tag `2.3.1`, commit `cda057318bec196183d4cc92b01bc1dd64bbfb02`. Files `src/SdCard/SdSpiCard/SdSpiCard.cpp`, `src/SdCard/SdCardInfo.h`, `src/SdFatConfig.h`, `src/SdFat.h`, `src/SdCard/SdSpiCard/SpiDriver/SdSpiLibDriver.h`, `src/FsLib/FsVolume.cpp`, `src/FatLib/FatPartition.cpp`, `src/FatLib/FatFormatter.cpp`. |
| **FI** | FreeInk SDK, the hardware layer of CrossPoint | `Free-Ink/freeink-sdk` at `111fdcc7f0176c3ee38391a160ee296bf492dbd8` (as in `sp2d-x4-wiring.md`): `libs/hardware/SDCardManager/{library.json, inject_build_flags.py, include/SDCardManager.h, src/SDCardManager.cpp}`. |
| **CP** | CrossPoint Reader 1.6.5 | `crosspoint-reader/crosspoint-reader`, tag `1.6.5`, commit `93e98bb78702e29868a16a13b80c40e6b36ccdff`: `platformio.ini`, `src/main.cpp`, `lib/hal/HalStorage.cpp`, `lib/Serialization/PersistableStore.cpp`, `src/CrossPointState.h`, `src/activities/home/HomeActivity.cpp`, `lib/hal/HalSystem.cpp`. |

Every file was read through `raw.githubusercontent.com` or the publisher's site; nothing was built or run.
Licences: SF, FI and CP are MIT. This report and the code it informs take **facts** from them (command
numbers, field positions, constants, the shape of a sequence), cited by file and line; no code is copied.
The PLS is a published specification: field layouts and protocol rules are facts.

## 1. The SPI mode of an SD card [SPEC]

1. **Mode selection.** A card powers up in SD mode and enters SPI mode when it receives CMD0 with its CS
   line asserted (low); it then answers with an R1 response. The only way back to SD mode is a power
   cycle (PLS §7.2.1). A card that is not in SPI mode does not answer in SPI framing [SPEC]; it does not
   drive its DO (MISO) line in the SPI sense [INF].
2. **Bytes, aligned to CS.** "Every command or data block is built of 8-bit bytes and is byte aligned with
   the CS signal"; "the card starts to count SPI bus clock cycle at the assertion of the CS signal" (PLS
   §7.2). A command is 6 bytes: `01` + 6-bit index, 32-bit argument MSB first, CRC7 and an end bit `1`
   (PLS Table 7-1). The card answers every command in SPI mode (PLS §7.2.8).
3. **CRC.** CRC7, polynomial x⁷ + x³ + 1; CRC16, x¹⁶ + x¹² + x⁵ + 1 (CCITT), both starting from zero
   (PLS §4.5). SPI mode starts with CRC checking **off**, but: CMD0 is received in SD mode and must carry
   a valid CRC (`40 00 00 00 00 95`); CMD8's CRC is **always** checked; after CMD59 with bit 0 set, every
   command and every data block written is checked (PLS §7.2.2). A command whose CRC fails is answered
   with the CRC error bit of R1 and not executed; for CMD8, R1 = `09h` alone (PLS Table 7-5). A data
   block written with a bad CRC16 is answered with the data response "rejected, CRC error" (PLS
   §7.3.3.1). The card always appends a valid CRC16 to the blocks it sends (PLS §7.2.3).
4. **Initialization** (PLS Figures 7-1, 7-2, §7.2.1): CMD0 → R1 `01h` (idle); CMD8 with VHS = `0001b`
   (2.7–3.6 V) and a check pattern → R7 echoing both (a card that does not support CMD8 answers
   "illegal command": a version 1.x card); then ACMD41 (CMD55 followed by CMD41) repeated until R1
   clears its idle bit; then CMD58 reads the OCR, whose bit 30 (CCS) tells SDSC (0) from SDHC/SDXC (1).
   The HCS bit (30) of ACMD41 is taken at the first ACMD41 only; an SDHC/SDXC card stays busy forever if
   CMD8 was not sent before the first ACMD41 or if HCS is 0 (Figure 7-1 note 1). Initialization ends
   within 1 second of the first ACMD41 (PLS §4.2.3).
5. **The idle state** accepts only CMD8, ACMD41, CMD58 and CMD59 (and CMD0, CMD55 to reach ACMD41)
   (PLS §7.2.7); anything else is rejected [SPEC], with the illegal-command bit [INF].
6. **Commands supported in SPI mode** (PLS Table 7-3, 7-4): CMD0, CMD1 (only after a first valid
   ACMD41, for thin cards), CMD6, CMD8, CMD9 (CSD), CMD10 (CID), CMD12 (stop a multiple-block read, R1b),
   CMD13 (status, R2), CMD16 (block length), CMD17/18 (read one / many blocks), CMD24/25 (write one /
   many), CMD27 (program CSD), CMD28–30 (write-protect groups, SDSC only), CMD32/33/38 (erase, R1b),
   CMD42 (lock/unlock), CMD55, CMD56, CMD58 (OCR, R3), CMD59 (CRC on/off); ACMD13 (SD status, R2 + 64
   bytes), ACMD22 (well-written blocks, 4 bytes), ACMD23 (pre-erase count), ACMD41, ACMD42, ACMD51 (SCR,
   8 bytes). "When an ACMD is not defined, the card treats it as regular command" (PLS §4.3.9.1).
7. **Responses** (PLS §7.3.2): R1 is one byte, bit 7 zero; bit 0 in idle state, 1 erase reset, 2 illegal
   command, 3 communication CRC error, 4 erase sequence error, 5 address error, 6 parameter error. R1b
   is R1 followed by busy bytes (`00h`) until the card is ready. R2 (CMD13, ACMD13) adds a second byte:
   bit 0 card locked, 1 WP erase skip / lock-unlock failed, 2 error, 3 CC error, 4 card ECC failed, 5 WP
   violation, 6 erase parameter, 7 out of range / CSD overwrite. R3 = R1 + OCR; R7 = R1 + 4 bytes
   (command version, reserved, voltage accepted, check pattern). A response that signals an illegal
   command or a CRC error is the R1 byte alone (PLS §7.3.2). Error bits are cleared when read (§7.3.4).
8. **Tokens** (PLS §7.3.3): start block `FEh` (single reads and writes, multiple reads), `FCh` (each block
   of a multiple write), stop transmission `FDh` (ends a multiple write); data response `xxx0sss1`, `010`
   accepted, `101` CRC error, `110` write error; data error token `000xxxxx` (bit 0 error, 1 CC error, 2
   ECC failed, 3 out of range) sent instead of a block the card cannot read.
9. **Reads** (PLS §7.2.3): R1, then after the access time a data token: start block, the block, CRC16.
   CMD18 streams blocks until CMD12, which the card accepts while it sends; CMD12 is the only command
   legal during a read (§7.2.8). An SDSC card reads `CMD16` bytes (1–512) from any byte address if
   `READ_BL_PARTIAL` = 1, never across a 512-byte physical block (`READ_BLK_MISALIGN` = 0); an SDHC/SDXC
   card reads 512 bytes at a block address. "SDSC uses byte unit address and SDHC and SDXC use block unit
   address" (Table 7-3 note 10).
10. **Writes** (PLS §7.2.4): R1, then the host sends a start token, the block and its CRC16; the card
    answers a data response, then holds DO low (busy) while it programs. "While the card is busy,
    resetting the CS signal will not terminate the programming process. The card will release the DataOut
    line … If the card is reselected before the programming is finished, the DataOut line will be forced
    back to low and all commands will be rejected." A write-protect violation is detected while
    programming: data response "write error", then CMD13 shows WP violation (§7.2.4, §7.3.3.1). CMD0
    terminates programming (§7.2.4).
11. **Timeouts the host should use** (PLS §4.6.2): read 100 ms; write busy 250 ms (SDHC), up to 500 ms for
    SDXC; initialization 1 s. The access times proper (NCR, NAC, NWR, the busy times) are in the
    removed §7.5.4; SF waits 1 to 9 bytes for R1 after discarding one (item 2 of §2 below).
12. **Registers** (PLS §5): OCR — bits 15–23 the 2.7–3.6 V window, bit 30 CCS, bit 31 power-up done. CID
    (128 bits) — MID, OID (2 ASCII), PNM (5 ASCII), PRV (BCD n.m), PSN (32 bits), MDT (year − 2000, month),
    CRC7. CSD version 1.0 (SDSC): capacity = (C_SIZE + 1) · 2^(C_SIZE_MULT + 2) · 2^READ_BL_LEN, at most
    2 GB, `READ_BL_LEN` = 10 only to code 2 GB; `READ_BL_PARTIAL` 1, `WRITE_BL_PARTIAL` choosable.
    CSD version 2.0 (SDHC, SDXC): capacity = (C_SIZE + 1) · 512 KiB, `C_SIZE` 22 bits up to `3FFEFFh`;
    TAAC fixed `0Eh`, NSAC `00h`, `READ_BL_LEN` and `WRITE_BL_LEN` 9, partial and misaligned access 0,
    `ERASE_BLK_EN` 1, `SECTOR_SIZE` `7Fh`, `R2W_FACTOR` `010b`, CCC `x1x1101101x1b`. SDHC is
    2 GB + 8.5 MB to 32 GB − 80 MB, SDXC from 32 GB. `TMP_WRITE_PROTECT` and `PERM_WRITE_PROTECT` are
    bits 12 and 13 of both. SCR (64 bits, ACMD51): `SCR_STRUCTURE`, `SD_SPEC`, `DATA_STAT_AFTER_ERASE`,
    `SD_SECURITY`, `SD_BUS_WIDTHS`, `SD_SPEC3` … (Table 5-17). CMD6's 64-byte status: maximum current,
    the support bits of six function groups ("function 15 and 0 are always enabled"), the selected
    function per group (`Fh` = error) (§4.3.10, Table 4-13).
13. **Write protection.** A card is protected by its CSD bits; the switch of a full-size card is read by
    the host's socket, not by the card; a microSD card has none [SPEC §5.3, INF].

## 2. What CrossPoint 1.6.5 does with the card [SRC]

1. **The driver is SdFat 2.3.1 over SPI.** FreeInk's `SDCardManager` depends on `greiman/SdFat ^2.3.1`
   (`FI:library.json:11-13`); for an SPI board it holds an `SdFat sd` (`FI:SDCardManager.h:124-126`), and
   2.3.1 is the latest release of the 2.3 line (tags of `greiman/SdFat`, 2026-10-15). The SDMMC path
   (`FREEINK_SD_SDMMC`) is the X4 Pro's, not the X4's (`FI:SDCardManager.cpp:17-105`).
2. **Build options.** CrossPoint sets `USE_UTF8_LONG_NAMES=1` and `USE_SPI_ARRAY_TRANSFER=1`
   (`CP:platformio.ini:48-56`); FreeInk forces `USE_UTF8_LONG_NAMES` for the whole build
   (`FI:inject_build_flags.py`). SdFat's defaults on the ESP32 apply otherwise: `SDFAT_FILE_TYPE` 3 (FAT16,
   FAT32 and exFAT, `SF:SdFatConfig.h:113-121`), `ENABLE_DEDICATED_SPI` 1 (`:132-146`), `USE_SD_CRC` 0
   (`:336-338`), `CHECK_FLASH_PROGRAMMING` 1 (`:323-325`), `SD_MAX_INIT_RATE_KHZ` 400 (`:197-199`),
   `FAT12_SUPPORT` 0 (`:371`).
3. **Mount.** `Storage.begin()` → `SDCardManager::begin()` (`CP:HalStorage.cpp:35`): the X4's SD CS is 12
   and its SCLK and MOSI are the display's (`FI:SDCardManager.cpp:122-129`, `sp2d-x4-wiring.md` §1 item
   1); the display CS is driven high first because the panel shares the bus (`:147-155`); `SPI.begin(8, 7,
   10, 12)`, then one attempt `sd.begin(12, 40 MHz)` (`:170-179`; `spiHz` 0 → 40 000 000, `:130`).
   `SdFat::begin(cs, maxSck)` is `begin(SdSpiConfig(cs, SHARED_SPI, maxSck))` (`SF:SdFat.h:72-73`): the
   card is **shared**, so every sector access is selected, done and deselected.
4. **SdFat's card initialization** (`SF:SdSpiCard.cpp:127-226`): clock 400 kHz; CS high and 10 bytes of
   `FFh` ("min of 74 clock cycles with CS high"); select; CMD0 until R1 = `01h`, within
   `SD_INIT_TIMEOUT` 2 000 ms (`SF:SdCardInfo.h:131`); CMD8 `1AAh`: illegal → SD1, else the fourth byte
   after R1 must be `AAh` → SD2; ACMD41 with HCS = 1 for SD2 until R1 = `00h`, within the same 2 s; for
   SD2, CMD58 and the first OCR byte `& C0h == C0h` → SDHC; then the clock rises to 40 MHz. No CMD9,
   CMD10, CMD16 or CMD59 at mount.
5. **Commands on the wire** (`SF:SdSpiCard.cpp:245-293`): before any command but CMD0 and CMD12, wait for
   `FFh` (`SD_CMD_TIMEOUT` 300 ms); send the command with CRC `95h` for CMD0 and `87h` for **every other
   command** (correct only for CMD8 `1AAh`; the card ignores it with CRC off); "discard first fill byte to
   avoid MISO pull-up problem", then read up to 9 bytes until bit 7 clears. A card that answers on the
   first byte after the command would see its R1 discarded [INF]: the model answers on the second.
6. **Sector reads** (`:461-529`, `:549-561`): with `ENABLE_DEDICATED_SPI`, `readSector` is `readSectors(…,
   1)`, which is CMD18 at the sector (SDHC) or the byte address (SDSC), then per sector: wait for a byte
   that is not `FFh` within `SD_READ_TIMEOUT` 300 ms, require `FEh`, receive 512 bytes in one transfer
   (`USE_SPI_ARRAY_TRANSFER`, `SF:SdSpiLibDriver.h:56-64`: the host sends `FFh`), discard the CRC; then,
   the card being shared, `readStop`: CMD12, its response, and deselect.
7. **Sector writes** (`:663-765`): `writeSector` (shared) is CMD24, `FEh`, 512 bytes, CRC `FFFFh`, the
   data response must be `xxx00101b`; then (`CHECK_FLASH_PROGRAMMING`) wait for `FFh` within
   `SD_WRITE_TIMEOUT` 600 ms and send CMD13, whose two bytes must both be zero. `writeSectors` is CMD25,
   per sector wait for `FFh` then `FCh`, 512 bytes, CRC, data response; `writeStop` waits for `FFh`, sends
   `FDh` and deselects.
8. **Errors the firmware sees**: any timeout or wrong token makes `sd.begin` or the access fail;
   `SDCardManager::begin` returns false and logs `SD card not detected (err=…)`
   (`FI:SDCardManager.cpp:184-191`), and CrossPoint shows "SD card error" (`CP:main.cpp:407-411`).
9. **Volume** (`SF:FsVolume.cpp:28-48`, `SF:FatPartition.cpp:393-505`): exFAT is tried first, then FAT,
   on partition 1 of the MBR: the partition's type must be non-zero and its boot flag `00h` or `80h`; the
   boot sector's bytes per sector must be 512, its FAT count 1 or 2, its sectors per cluster a power of
   two; FAT32 when the cluster count is 65 525 or more, FAT16 from 4 085, FAT12 refused. SdFat's own
   FAT32 formatter (`SF:FatFormatter.cpp:185-255`) follows the SD Association's layout: the partition
   at sector 8 192, the data area on an 8 192-sector boundary, two FATs, partition type `0Ch` (LBA).

## 3. What CrossPoint needs on the volume to reach its home screen [SRC]

1. Nothing beyond a mounted volume. After the mount, `APP_STATE.loadFromFile()`,
   `SETTINGS.loadFromFile()`, `RECENT_BOOKS.loadFromFile()` and the other stores read their JSON files
   (`CP:main.cpp:416-439`); `PersistableStoreBase::readDocFromFile` returns false without error when a
   file is missing ("Expected on first boot — not an error", `CP:PersistableStore.cpp:21-24`).
2. CrossPoint **writes** to the card: every store's `writeDocToFile` creates `/.crosspoint` first
   (`CP:PersistableStore.cpp:10-19`); its state file is `/.crosspoint/state.json`
   (`CP:CrossPointState.h:27`), and a splash-less wake saves it before painting (`CP:main.cpp:510-511`).
   A panic dump goes to `/crash_report.txt` (`CP:HalSystem.cpp:110`). The home screen looks for cover
   thumbnails of recent books only (`CP:HomeActivity.cpp:123-180`); with no recent book it reads nothing
   more.
3. Hence: an **empty FAT32 volume** (FAT16 or exFAT would do as well) is what CrossPoint's home screen
   needs; the card must accept writes, single (CMD24 + CMD13) and multiple (CMD25 + `FDh`). [INF]

## 4. The bus of the X4, seen from a card that only sees selections [INF]

The card sits on GP-SPI2 with the display: SCLK GPIO8, MOSI GPIO10, MISO GPIO7, CS GPIO12
(`sp2d-x4-wiring.md` §2). Under SP2b's `SpiTarget` (ADR 0012), a chip receives the transfers made while
its chip select is active, nothing else. Two consequences:

1. SdFat's power-up clocks (10 bytes with CS high) reach no model; a card model cannot require them, and
   accepts CMD0 without them. A real card that never got them may not answer [HYP: real cards tolerate
   it in practice; not the model's concern].
2. Transfers to the display (CS 21) never reach the card (CS 12 high), and a deselected card leaves MISO
   to the pad (`SpiTarget::transfer`'s `miso` prefill). With no card in the slot, MISO reads what the
   pad's pull gives on every byte: `FFh` with a pull-up; SdFat then times out on CMD0 after 2 s and
   CrossPoint shows "SD card error", which is SP2d's result.

## 5. What the sources leave open, and what SP2e does with it

| Open point | SP2e | Why |
|---|---|---|
| Access times (NCR, NAC, busy) — removed from the simplified edition | response on the second byte after a command; read access 250 µs; programming 1 ms per block; erase 2 ms; initialization 50 ms from the first ACMD41; busy 250 µs after the stop token of a multiple write (PLS Figure 7-7 draws busy after it) and after CMD12 ending a multiple write, none after CMD12 ending a read ("The card may become busy after receiving these commands based on its state prior to the command reception", PLS §4.9.2: a read leaves nothing to program) — all parameters of the model [HYP] | within the host timeouts of PLS §4.6.2 and SdFat's; short enough for a fast emulation |
| What DO carries while selected with nothing to send | `FFh` [HYP] | SdFat waits for `FFh` as "ready" (`SF:SdSpiCard.cpp:613-621`) |
| What a deselection does to a transfer in progress | a partial command or block is dropped and reported; a multiple-block read ends; programming goes on (PLS §7.2.4) [HYP for reads] | the spec says it for writes only |
| Card identity (CID) | MID `00h`, OID `"IE"`, PNM `"INKEM"`, PRV 1.0, PSN 1, MDT 2026-10 [HYP] | no manufacturer to imitate; a fixed identity keeps runs deterministic |
| Card type by capacity | SDSC (CSD 1.0, byte addresses) up to 2 GiB; SDHC/SDXC (CSD 2.0, block addresses) above [INF from §1 item 12] | the 2 GB + 8.5 MB minimum of SDHC is not enforced: a 2 GiB + 1 MiB image is presented as SDHC [HYP] |
| Card lock (class 7), write-protect groups, CPRM, CMD27, CMD56 | not modelled: illegal command and a named diagnostic; CCC declares classes 0, 2, 4, 5, 8, 10 (`535h`), SCR `SD_SECURITY` 0 [HYP] | no X4 firmware known to use them; declaring class 7 would announce what is not there |
| CMD6 | the status of a card with the default function only in each group; a switch to anything else answers `Fh` [HYP] | lets a driver that asks (ESP-IDF's SD/MMC layer, when the CSD declares class 10) go on at default speed |
| Write protection | `TMP_WRITE_PROTECT` set in the CSD; writes answered "write error", CMD13 shows WP violation; erase skips with "WP erase skip" | §1 items 10, 13 |
| Content of the card after an erase | zeros; SCR `DATA_STAT_AFTER_ERASE` 0 | PLS §4.3.5 lets the card choose 0 or 1 |
| SD status (ACMD13) | 64 zero bytes (1-bit bus, regular card, no class) [HYP] | nothing on the X4 reads it |
