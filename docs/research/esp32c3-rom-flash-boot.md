# ESP32-C3 rev3 mask ROM — SPI-flash boot path at register level (banner → 2nd-stage entry)

Scope: everything the rev3 ROM (`esp32c3-api1-20210207`) does with hardware between the end of its banner and the jump to the 2nd-stage bootloader entry point, and the minimal behaviour each register needs for that path to succeed.
Input for the plan that follows SP1b-1. Date of research: 2026-10-11.
Already established and not repeated here: `esp32c3-boot-chain.md` (§C cache/MMU, §D SPI block, §J rows 0–12, §K, §M), `sp1b-test-oracle.md` (§1b, §1c, §2), `esp32c3-svd.md` (register names).

## 0. Method, sources, tags

| Tag | Meaning |
|---|---|
| **M** (MEASURED) | seen in a trace or UART capture produced for this report (QEMU inside Docker, commands in §M) |
| **R** (READ) | read in the disassembly of `esp32c3_rev3_rom.elf` (cited `function@address`) or in a source file (cited `file:line`) |
| **A** (ASSUMED) | inferred, not observed; every A item is repeated in §L (open questions) when it carries risk |

| Source | What | Where |
|---|---|---|
| `ROM3:` | disassembly of `esp32c3_rev3_rom.elf` (esp-rom-elfs `20260528`, sha256 `19ac22e0…fc3c`) | scratch `boot/romelf/dis_rev3.txt`, helper `flashboot/tools/fn.sh` |
| `IDF:` | ESP-IDF v5.5.2 headers shipped in the `inkemu/esp-idf` image (`/opt/esp/idf`) | `components/soc/esp32c3/register/soc/{spi_mem,extmem,system,sensitive,efuse}_reg.h`, `…/include/soc/hwcrypto_reg.h` |
| `QEMU:` | Espressif QEMU 9.2.2 (`esp_develop_9.2.2_20250817`), read for facts only, nothing copied | scratch `boot/qemu/esp32c3_{spi,cache}.c` |
| Trace T1 | `-d exec,nochain` + `-trace memory_region_ops_read/write` + `-trace 'm25p80_*'`, one instruction per TB, image `flash_hello.bin` | `flashboot/out/t1_mmio.txt` (7,628 lines after collapsing repeats) |
| Trace T2 | `-d cpu,nochain`; every load/store decoded from the ROM bytes, address from the pre-state registers, load value from the next state | `flashboot/out/t2_access.txt` (17,891 accesses), `t2_calls.txt` (call trace) |

Validity of the QEMU traces for the real ROM **(M)**: the 365,968 bytes of code (`.fixed.text`…`.text`, `0x40000000–0x4005958F`) of QEMU's `esp32c3-rom.bin` are byte-identical to the rev3 ELF (`cmp` after `objcopy`); the only difference in the whole 384 KiB image is 8 bytes of the `.data_btdm` initial image (Bluetooth, unused here).
T1 and T2 are two independent extractions and agree on all 12,624 peripheral accesses up to the bootloader entry (0 mismatches on kind, address, value).
All instruction indices below are QEMU indices; `+n` means "n instructions after `main+0x64`" (the strap read that selects flash boot, QEMU index 400,840). QEMU reaches that point ≈383,000 instructions later than inkemu because of its USB-Serial-JTAG stub (§I.8); the `+n` offsets are comparable, the absolute indices are not.

## 1. Key results

1. The path is **120,671 instructions** long (`+0` → jump) and makes **7,240 peripheral accesses**: 6,260 to the SHA accelerator, 257 to the MMU table, 414 to the two consoles, 309 to everything else **(M)**.
2. The ROM uses exactly **three flash commands** through SPI1: release-from-power-down `0xAB` (`CMD` bit 20), read-JEDEC-ID `0x9F` (`CMD` bit 28), read-status `0x05` (`CMD` bit 27, twice). **No user-defined (`USR`) transfer, no `ADDR`/`MOSI_DLEN`/`MISO_DLEN`/`USER2` use on SPI1** **(M)**.
3. **Every byte of the bootloader image is read through the D-bus cache window `0x3C000000`**, never through SPI1: 5,266 word reads and 1 byte read **(M)**. The whole image is mapped once (one MMU entry) when it fits in 64 KiB.
4. The ROM blocks on **ten polled conditions** (§C rows 5–12 and 14). Everything else is read-modify-write where the value read only feeds the value written back.
5. Image validation: header gate, chip id, per-segment sanity, 8-bit XOR checksum (fatal), then SHA-256 over the image computed with the **hardware SHA peripheral** (329 blocks) and compared with the 32 bytes that follow the checksum; a SHA mismatch is **not fatal** without secure boot, and the `hash_appended` header byte is never consulted **(M+R)**.
6. Failure modes **(M)**: bad magic / blank flash → `invalid header: 0x%02x%02x%02x%02x` printed forever in a tight loop; every other failure → one message, then `ets_main.c 333 `, then an infinite `j .` at `0x40047ed2`. Recovery on hardware is a watchdog reset, which QEMU does not model.

## A. Call sequence (M, call trace T2; addresses R)

`main@0x40047e3c` after `print_other_reset_reason`:

| `+n` | Call (caller pc) | Arguments seen |
|---|---|---|
| +0 | `main+0x64`: read `GPIO.STRAP`, bit 3 set ⇒ flash boot | `0x8` |
| +4 | `ets_efuse_get_spiconfig@0x40046b3e` | returns 0 |
| +19 | `spi_flash_boot_attach@0x4004dca8(spiconfig=0, a1=0, a2=0)` | |
| +25 | → `SelectSpiFunction@0x4004ce72(0)` (default pads) | |
| +344 | → `SPI_init@0x4004da02(mode=5, div=4)` → `spi_common_set_dummy_output`, `spi_common_set_flash_cs_timing`, `spi_dummy_len_fix`×2, `spi_cache_mode_switch(0)` | |
| +523 | → `esp_rom_spiflash_config_readmode@0x4004dc28(5, 0)` → `spi_cache_mode_switch(0)`. `a2 == 0` ⇒ **no `SPI_WakeUp` here** | |
| +607 | `ets_run_flash_bootloader@0x40049d20` | |
| +683 | → `ROM_Boot_Cache_Init@0x40050ee6` → `Cache_Owner_Init`, `Cache_MMU_Init`, `Cache_Set_Default_Mode` (→ `Cache_Occupy_ICache_MEMORY(1)`, `Cache_Invalidate_ICache_All`), `Cache_Enable_ICache(0)`, `Cache_UnMask_Dram0` | |
| +1170 | → `ets_efuse_get_flash_delay_us` → `ets_delay_us(0)` | |
| +1192 | → `SPI_WakeUp@0x4004d1c8` | |
| +1214 | → `ets_efuse_force_send_resume` (0 ⇒ `esp_rom_spi_flash_send_resume` skipped) | |
| +1227 | → `ets_loader_map_range@0x400499f6(&st, off=0, len=24, 0)` → `Cache_Suspend_ICache`, `Cache_Invalidate_ICache_All`, `Cache_Dbus_MMU_Set(0, 0x3C000000, 0, 64, 1, 0)`, `Cache_Resume_ICache` | returns `0x3C000000` |
| +1444 | → `memcpy(hdr, 0x3C000000, 8)`; header gate | |
| +1482 | → `esp_rom_spi_flash_update_id@0x4004db12` | |
| +1518 | → `memcpy(ext, 0x3C000008, 16)`; chip-id check | |
| +1560 | → `ets_config_flash_by_image_hdr@0x40049b2e(hdr, ext, 0)`: prints `SPIWP`, `SetSpiDrvs`, `esp_rom_spiflash_config_clk(1,0)` and `(1,1)` (each: `Wait_SPI_Idle` → `esp_rom_spiflash_read_status`), `spi_dummy_len_fix`×2, `esp_rom_spiflash_config_readmode(2,0)` → `spi_cache_mode_switch(0x802000)`, prints `mode`, `spi_common_set_dummy_output(2)` → `spi_set_dummy_output(0,1,1)`, `(1,1,1)` | |
| +5808 | → `ets_sha_enable`, `ets_sha_init(ctx, 2)`, `ets_sha_update(ctx, 0x3C000000, 24, 1)` (buffered, no hardware access) | |
| +6074 | → segment loop, 3 times: `ets_loader_map_range(&st, off, 8, 1)` (already mapped ⇒ no cache or MMU access, one eFuse read), 7× `check_condCOUNTER.4107`, `memcpy(seg, ptr, 8)`, `ets_sha_update(ctx, seg, 8, 0)`, print `load:`, checks, `ets_loader_map_range(&st, off, len, 1)`, word-copy loop `@0x4004a186`, `ets_sha_update(ctx, ram_dst, len, 0)` | seg headers at +6074, +36357, +55415 |
| ≈+116580 | → checksum: `ets_loader_map_range(&st, off, 16, 1)`, byte read, compare | |
| +116619 | → `ets_sha_update(ctx, ptr, pad, 0)`, `ets_sha_finish(ctx, digest)` (+116702, → `ets_sha_get_state` +117049), `ets_loader_map_range(&st, off+pad, 32, 1)` (+117202), `memcmp(digest, ptr, 32)` (+117252) | |
| +117321 | → 7× `check_condCOUNTER.4123`, `Cache_Suspend_ICache` (+117453), `Cache_Invalidate_ICache_All`, `Cache_Resume_ICache`, `Cache_MMU_Init` (≈+117560) | |
| +120671 | → print `entry`, `jalr` to the entry | `pc=0x403cbf1a`, `sp=0x3fcde560`, `ra=0x4004a296`, `a0=0x11` (printf return), `a1=0xa` |

If the bootloader ever returns, `ets_run_flash_bootloader` returns 0 and `main` prints `user code done` **(R, `main@0x4004815c`)**.
`check_condCOUNTER.*` are fault-injection guards that only act when secure boot is on; each reads `EFUSE.RD_REPEAT_DATA2` once **(R, `@0x40049930`, `@0x40049972`)**.

## B. Every MMIO access, in execution order

Conventions: "RMW" = read, modify, write back; the value read is not tested. "QEMU" columns are the values measured in T1; where QEMU does not store a register the value it returns is 0 (§I), so the *operation* column is the fact to implement, not the QEMU value. Console accesses (`UART0.STATUS`/`FIFO`, `USB_DEVICE.EP1_CONF`/`EP1`) happen for each of the 136 characters printed on this path and are covered by `esp32c3-boot-chain.md` §G; they are omitted below. All peripheral accesses on this path are 32-bit **(M)**.

### B.1 Boot-mode and pad selection

| Function | Register | Access | Use of the value | Tag |
|---|---|---|---|---|
| `main+0x64` | `GPIO.STRAP` `0x60004038` | R | bit 3 set ⇒ flash boot, clear ⇒ download logic | M+R |
| `ets_efuse_get_spiconfig` (called 3 times: `main`, `SelectSpiFunction`, `ets_config_flash_by_image_hdr`) | `EFUSE.RD_MAC_SPI_SYS_1` `0x60008848`, `RD_MAC_SPI_SYS_2` `0x6000884C` | R, R | `v = (SYS_2 << 16) or (SYS_1 >> 16)`, `x = (SYS_2 >> 16) & 0xF`; returns 0 when both are 0 or both all-ones, else `v & 0x3FFFFFFF`. 0 ⇒ default pads | M+R |
| `gpio_output_disable(12)`, `(13)` | `GPIO.ENABLE` `0x60004020` | RMW | clear bit 12, then bit 13 | M+R |
| `gpio_pad_select_gpio(12)`, `(13)` | `IO_MUX.GPIO12` `0x60009034`, `GPIO13` `0x60009038` | RMW | `MCU_SEL` [14:12] ← 1 (GPIO function) | M+R |
| `gpio_pad_pullup(n)` for 15, 17, 16, 12, 13, 14 | `IO_MUX.GPIOn` (`0x60009004 + 4n`) | RMW ×2 | clear bit 7 (`FUN_WPD`), then set bit 8 (`FUN_WPU`) | M+R |
| `SelectSpiFunction` | `IO_MUX.GPIO15` `0x60009040`, `GPIO17` `0x48`, `GPIO16` `0x44`, `GPIO14` `0x3C` | RMW | `& 0xFFFF8FFF`: `MCU_SEL` ← 0 (SPICLK, SPIQ, SPID, SPICS0) | M+R |
| `gpio_bypass_matrix_in(0)`, `(1)` | `GPIO.FUNC0_IN_SEL_CFG` `0x60004154`, `FUNC1_IN_SEL_CFG` `0x60004158` | RMW | clear bit 6 (`SIG_IN_SEL`): input taken from IO_MUX | M+R |
| `gpio_pad_input_enable(17)`, `(16)` | `IO_MUX.GPIO17`, `GPIO16` | RMW | set bit 9 (`FUN_IE`) | M+R |
| `gpio_pad_unhold(n)` for 15, 17, 16, 12, 13, 14 | `RTC_CNTL.DIG_PAD_HOLD` `0x600080D4` | RMW | clear bit n (pads 0–5 would use `PAD_HOLD` `0x600080D0`) | M+R |

Default pads **(R)**: GPIO12 = SPIHD and GPIO13 = SPIWP are left as pulled-up GPIO inputs (not driven by the controller in DIO mode), GPIO14 = CS0, GPIO15 = CLK, GPIO16 = D, GPIO17 = Q.

### B.2 `SPI_init(5, 4)` and `esp_rom_spiflash_config_readmode(5, 0)`

| Function | Register | Access | Operation / value | Tag |
|---|---|---|---|---|
| `SPI_init` | `SYSTEM.PERIP_CLK_EN0` `0x600C0010` | RMW | set bit 1 (SPI01 clock) | M+R |
| | `SYSTEM.PERIP_RST_EN0` `0x600C0018` | RMW ×2 | set bit 1, then clear bit 1 (reset pulse) | M+R |
| | `SPI1.SUS_STATUS` `0x600020A4` | RMW | clear bit 0 (`FLASH_SUS`) | M+R |
| | `SPI0.MISC` `0x60003034` | RMW ×2 | clear bit 0 (`CS0_DIS`), set bit 1 (`CS1_DIS`) | M+R |
| `spi_common_set_flash_cs_timing` | `SPI0.USER` `0x60003018`, then `SPI1.USER` `0x60002018` | RMW | set bits 7:6 (`CS_SETUP`, `CS_HOLD`) | M+R |
| | `SPI0.CTRL2` `0x60003010`, then `SPI1.CTRL2` `0x60002010` | RMW ×2 | `CS_HOLD_TIME` [9:5] ← 2, then `CS_SETUP_TIME` [4:0] ← 1 (result `0x41`) | M+R |
| `SPI_init` | `SPI1.CLOCK` `0x60002014`, `SPI0.CLOCK` `0x60003014` | W | `0x00030103` (N=3, H=1, L=3: divide by 4); `0x80000000` if div ≤ 1 | M+R |
| | `SPI1.CTRL` `0x60002008` | W | `0x00208000` (`WP` b21, `RESANDRES` b15) or-ed with the mode bits (0 for mode 5) | M+R |
| | `SPI0.CTRL` `0x60003008` | W | `0x00200000` | M+R |
| | SPI0 `+0x28` (`MISO_DLEN`), `+0x24` (`MOSI_DLEN`) | RMW | bits [9:0] ← `0xFF` (256 bits = one 32-byte cache line). Not in the SVD for SPI0; `IDF:spi_mem_reg.h:451,459` defines them for both controllers | M+R |
| | `SPI0.USER2` `0x60003020` | RMW | bits [31:28] ← 7 (8-bit command) | M+R |
| | `SPI0.CACHE_FCTRL` `0x6000303C` | RMW | set bit 0 (`CACHE_REQ_EN`) | M+R |
| `spi_cache_mode_switch(0)` (slow read) | `SPI0.USER` | RMW ×3 | clear b27 (`USR_MOSI`); clear b29 (`USR_DUMMY`, because `dummy_len_plus[0] == 0`); set b30 and b28 (`USR_ADDR`, `USR_MISO`) | M+R |
| | `SPI0.USER1` `0x6000301C` | RMW | `USR_ADDR_BITLEN` [31:26] ← 23 | M+R |
| | `SPI0.USER2` | RMW | command [15:0] ← `0x03` | M+R |
| `SPI_init` | `SPI0.CACHE_FCTRL` | RMW | set bit 2 (`CACHE_FLASH_USR_CMD`) | M+R |
| `esp_rom_spiflash_config_readmode(5,0)` | `SPI1.CTRL`, then `SPI0.CTRL` | RMW ×2 each | `& 0xFE6F9E7F` (clears `FREAD_QIO` b24, `FREAD_DIO` b23, `FREAD_QUAD` b20, `FREAD_DUAL` b14, `FASTRD_MODE` b13, `FCMD_QUAD` b8, `FCMD_DUAL` b7), then or the mode bits (table below) | M+R |
| → `spi_cache_mode_switch(0)` | same SPI0 `USER`/`USER1`/`USER2` sequence as above | | | M |

Mode table `ROM3 .rodata 0x3ff1e13c` **(R)**: QIO `0x01002000`, QOUT `0x00102000`, DIO `0x00802000`, DOUT `0x00006000`, FAST_RD `0x00002000`, SLOW_RD `0`. Modes 0 and 1 additionally call `Enable_QMode` (not on this path).

### B.3 `ROM_Boot_Cache_Init` (`+683`)

| Function | Register | Access | Operation | QEMU value read | Tag |
|---|---|---|---|---|---|
| `ROM_Boot_Cache_Init` | `SYSTEM.CACHE_CONTROL` `0x600C0040` | RMW ×3 | set b0 (`ICACHE_CLK_ON`), set b1 (`ICACHE_RESET`), clear b1 | 0 (not stored) | M+R |
| `Cache_Owner_Init` | `EXTMEM.CACHE_MMU_OWNER` `0x600C40C4` | RMW | or `0xF` | 0 | M+R |
| `Cache_MMU_Init` | MMU table `0x600C5000…0x600C51FC` | W ×128 | every entry ← `0x100` (invalid) | | M+R |
| `Cache_Set_Default_Mode` | `EXTMEM.CACHE_ILG_INT_CLR` `0x600C407C` | W | `0xFFFFFFFF` | | M+R |
| | `EXTMEM.CACHE_WRAP_AROUND_CTRL` `0x600C40A8` | RMW | clear b0 | 0 | M+R |
| `Cache_Occupy_ICache_MEMORY(1)` | `SENSITIVE.INTERNAL_SRAM_USAGE_1` `0x600C101C` | RMW | clear b0 (`CPU_CACHE`; SVD reset `0xF` ⇒ `0xE`) | 0 | M+R |
| `Cache_Invalidate_ICache_Items(0, 0)` | `EXTMEM.ICACHE_SYNC_ADDR` `0x600C402C` | W | 0 | | M+R |
| | `EXTMEM.ICACHE_SYNC_SIZE` `0x600C4030` | RMW | bits [22:0] ← 0 | 0 | M+R |
| | `EXTMEM.ICACHE_SYNC_CTRL` `0x600C4028` | RMW | set b0 (`INVALIDATE_ENA`) | 0, wrote 1 | M+R |
| | `EXTMEM.ICACHE_SYNC_CTRL` | **R, polled** | loop until b1 (`SYNC_DONE`) is set | 2 | M+R |
| `Cache_Enable_ICache(0)` | `EXTMEM.ICACHE_CTRL` `0x600C4000` | RMW | set b0 (`ENABLE`) | 0 | M+R |
| `Cache_UnMask_Dram0` | `EXTMEM.ICACHE_CTRL1` `0x600C4004` | RMW | clear b1 (`SHUT_DBUS`) | 0 | M+R |

This is where inkemu stops today: `ICACHE_SYNC_CTRL` returns its SVD reset value 1 forever (`run.err`, pc `0x40050f4e`).

### B.4 Flash wake-up (`+1170`)

| Function | Register | Access | Operation | QEMU | Tag |
|---|---|---|---|---|---|
| `ets_efuse_get_flash_delay_us` | `EFUSE.RD_REPEAT_DATA2` `0x60008838` | R | `t = bits[31:28]` (`FLASH_TPUW`); 15 ⇒ 30; `ets_delay_us(t × 1000)`; blank ⇒ 0 | 0 | M+R |
| `ets_delay_us(0)` | CSR `0x802` | R ×2 | cycle counter; returns at once for 0 | | M+R |
| `SPI_WakeUp` | `SPI1.CTRL1` `0x6000200C` | RMW | bits [11:2] (`CS_HOLD_DLY_RES`) ← `0x1FF` | read `0xFFC`, wrote `0x7FC` | M+R |
| | `SPI1.SUS_STATUS` | RMW | clear b3 (`FLASH_RES_DLY_128`) | | M+R |
| | `SPI1.CTRL` | RMW | clear b15 (`RESANDRES`) | wrote `0x200000` | M+R |
| | `SPI1.CMD` `0x60002000` | W | `0x00100000` (`FLASH_RES`) → flash sees opcode `0xAB` | | M+R |
| | `SPI1.CMD` | **R, polled** | loop until the **whole register** is 0 | 0 | M+R |
| `ets_efuse_force_send_resume` | `EFUSE.RD_REPEAT_DATA3` `0x6000883C` | R | b13 (`FORCE_SEND_RESUME`); set ⇒ `esp_rom_spi_flash_send_resume` (not analysed) | 0 | M+R |

### B.5 First mapping and header (`+1227`)

| Function | Register | Access | Operation | QEMU | Tag |
|---|---|---|---|---|---|
| `Cache_Suspend_ICache` | `EXTMEM.ICACHE_CTRL1` | RMW | or 3 | read 0 | M+R |
| | `EXTMEM.ICACHE_CTRL` | RMW | clear b0 | read 1 | M+R |
| → `Cache_Suspend_ICache_Autoload` | `EXTMEM.ICACHE_AUTOLOAD_CTRL` `0x600C4040` | R | b2 (`ENA`) set ⇒ clear it and return 4; clear ⇒ return 0 | `0x8` | M+R |
| | `EXTMEM.ICACHE_AUTOLOAD_CTRL` | **R, polled** | loop until b3 (`DONE`) is set | `0x8` | M+R |
| | `EXTMEM.ICACHE_PRELOAD_CTRL` `0x600C4034` | **R, polled** | loop until b1 (`DONE`) is set | `0x2` | M+R |
| → `Cache_Freeze_ICache_Enable(0)` | `EXTMEM.ICACHE_FREEZE` `0x600C40CC` | RMW ×2 | clear b1 (`MODE`), then set b0 (`ENA`) | read 0 | M+R |
| | `EXTMEM.ICACHE_FREEZE` | **R, polled** | loop until b2 (`DONE`) is set | `0x4` | M+R |
| → `Cache_Freeze_ICache_Disable` | `EXTMEM.ICACHE_FREEZE` | RMW | clear b0 | | M+R |
| | `EXTMEM.ICACHE_FREEZE` | **R, polled** | loop until b2 (`DONE`) is clear | 0 | M+R |
| `Cache_Invalidate_ICache_All` | `ICACHE_SYNC_ADDR`, `SYNC_SIZE`, `SYNC_CTRL` | as in B.3 | set b0, poll b1 | read 2, wrote 3, read 2 | M+R |
| `Cache_Dbus_MMU_Set(0, 0x3C000000, 0, 64, 1, 0)` | MMU entry 0 `0x600C5000` | W | `0x0` = flash page 0, valid (entry index `(vaddr >> 16) & 0x7F`, value `paddr >> 16`, +1 per page) | | M+R |
| `Cache_Resume_ICache(saved)` | `EXTMEM.ICACHE_CTRL` | RMW | set b0 | | M+R |
| | `EXTMEM.ICACHE_CTRL1` | RMW | clear b1:0 (both buses open) | | M+R |
| → `Cache_Resume_ICache_Autoload(saved)` | `ICACHE_AUTOLOAD_CTRL` | RMW only if `saved & 4` | set b2 | not executed | R |
| `memcpy(hdr, …, 8)` | flash window `0x3C000000`, `0x3C000004` | R ×2 | image header | `0x4f0203e9`, `0x403cbf1a` | M |
| `ets_efuse_secure_boot_enabled` (many calls) | `EFUSE.RD_REPEAT_DATA2` | R | **bit 20** (`SECURE_BOOT_EN`); must be 0 for this path | 0 | M+R |

### B.6 Flash id and reconfiguration from the image header (`+1482`)

| Function | Register | Access | Operation | QEMU | Tag |
|---|---|---|---|---|---|
| `esp_rom_spi_flash_update_id` | `SPI1.USER` | RMW | clear b29 (`USR_DUMMY`) | `0x800000C0` | M+R |
| | `SPI1.W0` `0x60002058` | W | 0 | | M+R |
| | `SPI1.CMD` | W | `0x10000000` (`FLASH_RDID`) → flash sees `0x9F`, returns 3 bytes | | M+R |
| | `SPI1.CMD` | **R, polled** | until 0 | 0 | M+R |
| | `SPI1.W0` | R | `v & 0xFFFFFF`; bytes 0 and 2 swapped ⇒ `device_id = mfr<<16 or type<<8 or capacity`, stored at `rom_default_spiflash_legacy_data+0` (`0x3fcdf5c0`). Not used again on this path | `0x0018609D` ⇒ `0x9D6018` | M+R |
| `memcpy(ext, …, 16)` | flash window `0x3C000008…14` | R ×4 | extended header | | M |
| `esp_rom_spiflash_config_clk(1, 0)` → `Wait_SPI_Idle` | `SPI0.FSM` `0x60003054` | **R, polled** | loop until bits [6:4] (`EM_ST`) are 0 | 0 | M+R |
| | `SPI1.CMD` | **R, polled** | loop until bits [3:0] (`MST_ST`) are 0 | 0 | M+R |
| → `esp_rom_spiflash_read_status` | `SPI1.USER` | RMW | clear b29 | | M+R |
| | `SPI1.RD_STATUS` `0x6000202C` | RMW | clear bits [15:0] | 0 | M+R |
| | `SPI1.CMD` | W | `0x08000000` (`FLASH_RDSR`) → flash sees `0x05`, returns 1 byte | | M+R |
| | `SPI1.CMD` | **R, polled** | until 0 | 0 | M+R |
| | `SPI1.RD_STATUS` | **R, looped** | `status = value & status_mask` (`0xFFFF`, from the chip struct); **repeat the whole RDSR while bit 0 (WIP) is set** | 0 | M+R |
| `esp_rom_spiflash_config_clk(1, 0)` | `SPI0.CLOCK` | W | `0x80000000` (`CLK_EQU_SYSCLK`, div 1). For div n > 1: `(n-1)<<16 or (n/2-1)<<8 or (n-1)` | | M+R |
| `esp_rom_spiflash_config_clk(1, 1)` | same `Wait_SPI_Idle` + RDSR sequence, then `SPI1.CLOCK` ← `0x80000000` | | | | M+R |
| `esp_rom_spiflash_config_readmode(2, 0)` | `SPI1.CTRL`, `SPI0.CTRL` | RMW ×2 each | `& 0xFE6F9E7F`, then or `0x00802000` (`FREAD_DIO`, `FASTRD_MODE`) | SPI1: `0xA02000` | M+R |
| → `spi_cache_mode_switch(0x802000)` | `SPI0.USER` | RMW ×2 | clear b27; set b30:28 (`USR_ADDR`, `USR_DUMMY`, `USR_MISO`) | | M+R |
| | `SPI0.USER1` | RMW ×2 | addr bitlen ← 23; `USR_DUMMY_CYCLELEN` [5:0] ← `dummy_len_plus[0] + 3` = 3 | | M+R |
| | `SPI0.USER2` | RMW | command ← `0xBB` | | M+R |
| `spi_set_dummy_output(0,1,1)`, then `(1,1,1)` | `SPI0.CTRL`, then `SPI1.CTRL` | RMW ×2 each | set b19:18 (`D_POL`, `Q_POL`); set b3 (`FDUMMY_OUT`) | SPI1: `0xAC2008` | M+R |

`SetSpiDrvs(wp=0xEE, 0, &ext[1])` makes no access when the three drive-strength bytes `ext[1..3]` are 0 **(M+R, `@0x4004cd6e`)**.

Register values at the end of the ROM on a model that stores writes and starts from the SVD reset values (derived from the operations above, **A** for SPI0 since QEMU does not store it): SPI1 `CTRL=0x00AC2008`, `CTRL1=0x7FC`, `CTRL2=0x41`, `CLOCK=0x80000000`, `USER=0x800000C0` (these five match QEMU, **M**); SPI0 `CTRL=0x00AC2008`, `CTRL2=0x41`, `CLOCK=0x80000000`, `USER=0x700000C0`, `USER1=0x5C000003`, `USER2=0x700000BB`, `MOSI_DLEN=MISO_DLEN=0xFF`, `MISC=2`, `CACHE_FCTRL=5`.
The 2nd-stage bootloader reads `SPI0.CTRL` to print `SPI Mode` (`IDF:bootloader_flash.c bootloader_flash_get_spi_mode`), so a storing SPI0 prints `DIO` where QEMU prints `SLOW READ`.

### B.7 SHA, segments, checksum, digest (`+5808`)

| Function | Register | Access | Operation | QEMU | Tag |
|---|---|---|---|---|---|
| `ets_sha_enable` | `SYSTEM.PERIP_CLK_EN1` `0x600C0014` | RMW | set b2 (SHA clock) | | M+R |
| | `SYSTEM.PERIP_RST_EN1` `0x600C001C` | RMW | clear bits 5, 4, 2 (`& ~0x34`) | | M+R |
| segment loop `@0x4004a186` | flash window | R (word) ×5,238 | `w = src[i]; dst[i] = w; ck ^= w`. Destination is the segment load address, including **I-bus SRAM addresses** (`0x403Cxxxx`) | | M+R |
| `ets_sha_process` (329 blocks) | `SHA.MODE` `0x6003B000` | W | 2 (SHA-256), before every block | | M+R |
| | `SHA.M_MEM0…15` `0x6003B080…BC` | W ×16 | the 64-byte block, by `memcpy` (word writes) | | M+R |
| | `SHA.BUSY` `0x6003B018` | **R, polled** | loop **while the value is exactly 1** (after writing the block, before triggering) | 0 | M+R |
| | `SHA.START` `0x6003B010` | W | 1, first block only | | M+R |
| | `SHA.CONTINUE` `0x6003B014` | W | 1, the 328 other blocks | | M+R |
| `ets_sha_get_state` (once, from `ets_sha_finish`) | `SHA.BUSY` | **R, polled** | while == 1 | 0 | M+R |
| | `SHA.H_MEM0…7` `0x6003B040…5C` | R ×8 | digest, by `memcpy` | `0x1c59639a`, … | M+R |
| `ets_run_flash_bootloader+0x228` | flash window | R (byte) | stored checksum | `0x4d` | M+R |
| `memcmp` | flash window | R ×8 | appended digest | | M |

`SHA.H_MEM` is never written on this path (the ROM would write it to restore a saved state only if another user had touched the accelerator) **(M+R, `ets_sha_process@0x400495f0`)**.

### B.8 Hand-over (`+117453`)

`Cache_Suspend_ICache` (same accesses and polls as B.5), `Cache_Invalidate_ICache_All`, `Cache_Resume_ICache`, then `Cache_MMU_Init` (128 writes of `0x100`), `entry` print, `jalr` **(M)**.
State left to the bootloader **(M for QEMU, R for the operations)**: `ICACHE_CTRL.ENABLE = 1`, `ICACHE_CTRL1 = 0`, all 128 MMU entries invalid, SPI0/SPI1 as in B.6, SHA clock on, `mstatus`/interrupt setup unchanged since the banner.

## C. The reads that decide something (complete list for this path)

| # | Register | Condition to continue | Where | What a model must return |
|---|---|---|---|---|
| 1 | `GPIO.STRAP` | bit 3 set | `main+0x64` | already modelled |
| 2 | `EFUSE.RD_MAC_SPI_SYS_1/2` | both 0 ⇒ default pads | `ets_efuse_get_spiconfig` | already modelled (blank) |
| 3 | `EFUSE.RD_REPEAT_DATA2` | bits 31:28 ⇒ delay; **bit 20** must be 0 | `ets_efuse_get_flash_delay_us`, `ets_efuse_secure_boot_enabled` | already modelled (blank) |
| 4 | `EFUSE.RD_REPEAT_DATA3` | bit 13 = 0 (no resume command); bit 1 only consulted on an invalid header | `ets_efuse_force_send_resume`, `ets_run_flash_bootloader+0xd4` | already modelled (blank) |
| 5 | `EXTMEM.ICACHE_SYNC_CTRL` | bit 1 set after bit 0 was written 1 | `Cache_Invalidate_ICache_Items+0x26` | completion: `DONE=1` |
| 6 | `EXTMEM.ICACHE_AUTOLOAD_CTRL` | bit 3 set; bit 2 selects a branch | `Cache_Suspend_ICache_Autoload` | `DONE=1` always, bit 2 as stored |
| 7 | `EXTMEM.ICACHE_PRELOAD_CTRL` | bit 1 set | `Cache_Suspend_ICache+0x30` | `DONE=1` always |
| 8 | `EXTMEM.ICACHE_FREEZE` | bit 2 set after `ENA` set; bit 2 clear after `ENA` cleared | `Cache_Freeze_ICache_Enable/Disable` | `DONE` follows `ENA` |
| 9 | `SPI1.CMD` | whole register 0 after a command bit was written | `SPI_WakeUp`, `update_id`, `read_status` | command bits self-clear; `MST_ST`/`SLV_ST` [7:0] read 0 |
| 10 | `SPI0.FSM` | bits 6:4 = 0 | `Wait_SPI_Idle+0x8` | idle (SVD reset `0x200` satisfies it) |
| 11 | `SPI1.CMD` | bits 3:0 = 0 | `Wait_SPI_Idle+0x14` | idle |
| 12 | `SPI1.RD_STATUS` | bit 0 = 0 after `FLASH_RDSR` | `esp_rom_spiflash_read_status+0x58` | flash status byte with WIP = 0 |
| 13 | `SPI1.W0` | none (stored only) | `esp_rom_spi_flash_update_id` | JEDEC id in bits 23:0 |
| 14 | `SHA.BUSY` | ≠ 1 | `ets_sha_process+0x2c`, `ets_sha_get_state+0x10` | 0 when idle |
| 15 | `SHA.H_MEM0…7` | compared with the image digest (non-fatal) | `ets_sha_get_state` | SHA-256 state |
| 16 | flash window `0x3C000000+` | image content (§F) | `memcpy`, copy loop, `memcmp` | flash bytes through the MMU |
| 17 | CSR `0x802` | advances | `ets_delay_us` | already modelled |

Rows 5–12 and 14 are the hang points if the bit never reaches the expected state (ten conditions: row 8 holds two).

## D. SPI flash protocol as the controller sees it

### D.1 Commands issued (M: `m25p80_*` trace events + T1; R: call sites)

| Order | `+n` | Trigger written to `SPI1.CMD` | Bytes on the wire (QEMU flash model) | Result lands in | Setup registers used |
|---|---|---|---|---|---|
| 1 | +1208 | `0x00100000` `FLASH_RES` (b20) | `0xAB`, then 3 dummy bytes in QEMU | nowhere (`CTRL.RESANDRES` cleared first) | `CTRL1.CS_HOLD_DLY_RES`, `SUS_STATUS.FLASH_RES_DLY_128`, `CTRL.RESANDRES` |
| 2 | +1495 | `0x10000000` `FLASH_RDID` (b28) | `0x9F`, 3 bytes read (`9D 60 18` in QEMU) | `W0[23:0]`, first byte in `W0[7:0]` | `USER.USR_DUMMY` cleared, `W0` zeroed |
| 3 | +3181 | `0x08000000` `FLASH_RDSR` (b27) | `0x05`, 1 byte read (`00`) | `RD_STATUS[7:0]` (field `STATUS` is [15:0]) | `USER.USR_DUMMY` cleared, `RD_STATUS[15:0]` zeroed |
| 4 | +3266 | `0x08000000` `FLASH_RDSR` again | `0x05`, 1 byte | `RD_STATUS[7:0]` | same |

Not used on this path **(M)**: `USR` (b18), `FLASH_READ` (b31), `ADDR`, `USER1`, `USER2`, `MOSI_DLEN`, `MISO_DLEN`, `W1…W15` of SPI1. The ROM's `USR` variant exists (`esp_rom_spiflash_read_user_cmd@0x4004d0cc`: `CTRL &= 0x200000`, `MISO_DLEN = 7`, `USER = 0x90000000`, `USER2 = 0x70000000 or cmd`, `W0 = 0`, `CMD = 0x40000`, poll, result `W0 & 0xFF`, registers restored) but is reached only when `dummy_len_plus[1] ≠ 0`, which `spi_dummy_len_fix` forces to 0 **(R)**.

Self-clearing bits: every command bit of `SPI1.CMD` (b31…b17) is `R/W/SC` (`IDF:spi_mem_reg.h:15-113`); the ROM waits for the whole register, including the read-only FSM nibbles `MST_ST` [3:0] and `SLV_ST` [7:4], to read 0 **(R)**. QEMU completes the transfer inside the write and always reads `CMD` as 0 **(M, `QEMU:esp32c3_spi.c`)**.

### D.2 Smallest SPI1 + flash-chip model that satisfies the ROM

| Element | Minimal behaviour | Tag |
|---|---|---|
| `SPI1.CMD` write | for bits 20, 27, 28: run the command, then the register reads 0. Other bits: not needed by the ROM (needed later by the bootloader: `USR`, and by the app: `WREN`, `PP`, `SE`, …) | M |
| `FLASH_RES` | no effect on registers | M |
| `FLASH_RDID` | `W0` ← `mfr or type<<8 or capacity<<16` | M |
| `FLASH_RDSR` | `RD_STATUS[7:0]` ← flash status register 1, with bit 0 (WIP) = 0 | M |
| `SPI1.CTRL`, `CTRL1`, `CTRL2`, `CLOCK`, `USER`, `RD_STATUS`, `W0`, `SUS_STATUS` | plain read/write storage; reset values from the SVD (`CTRL 0x002CA000`, `CTRL1 0xFFC`, `CLOCK 0x30103`, `USER 0x80000000`) | M (QEMU stores them), R |
| Flash chip | a byte array (16 MiB), a 3-byte JEDEC id, a status byte equal to 0. No state machine is needed for the ROM: the three commands are independent | M |
| SPI0 | storage for `CTRL`, `CTRL2`, `CLOCK`, `USER`, `USER1`, `USER2`, `+0x24`, `+0x28`, `MISC`, `CACHE_FCTRL`; `FSM` reads with bits 6:4 = 0. QEMU proves that "reads 0, writes dropped" is enough for the ROM | M |

What SPI0 is configured to do for cache fills, were the model to honour it **(R, derived)**: opcode `0xBB`, 24-bit address, `USR_DUMMY_CYCLELEN = 3` (4 clocks), 32 bytes per transaction, clock divider 1; before the image header is parsed: opcode `0x03`, no dummy, divider 4.

## E. Cache window and MMU

| Fact | Tag |
|---|---|
| `ets_loader_map_range(st, off, len, flag)@0x400499f6` requires `off % 4 == 0`, `len % 4 == 0`, `len ≤ 0x100000`, else prints `ets_loader.c 77/78/79 ` and hangs in `j .` | R |
| It keeps `st = {base, size}`; if `[off, off+len)` lies inside the current mapping it returns `0x3C000000 + off − base` **without touching the cache or the MMU** (with `flag = 1` it still reads the secure-boot eFuse bit). The caller zeroes `st` before the header read of each retry, so the invalid-header loop remaps every time | R, M (one mapping for the whole 21 KiB image; one remap per `invalid header` iteration) |
| Otherwise `base = off & 0xFFFF0000`, `size = (off − base + len + 0xFFFF) & 0xFFFF0000`, then `Cache_Suspend_ICache`, `Cache_Invalidate_ICache_All`, `Cache_Dbus_MMU_Set(0, 0x3C000000, base, 64, size >> 16, 0)`, `Cache_Resume_ICache`. A failure of `Cache_Dbus_MMU_Set` prints `ets_loader.c 98 ` and hangs | R |
| `Cache_Dbus_MMU_Set@0x40051756` writes `table[((vaddr >> 16) & 0x7F) + i] = (paddr >> 16) + i` for `i < n`; rejects a page size other than 64 (returns 3), unaligned addresses (2), a virtual address outside `0x3C000000–0x3C7FFFFF` (4) | R |
| MMU table accesses on this path: 257 writes (128 + 1 + 128), **no read** | M |
| With secure boot off the mapping never targets anything but entry 0 upward from `0x3C000000`; a bootloader larger than 64 KiB (or a range crossing the current window) remaps, starting at the 64 KiB page that contains `off` | R (not exercised: both test bootloaders fit in one page) |
| Reads through the window happen with `ICACHE_CTRL.ENABLE = 1`, `ICACHE_CTRL1 = 0`, entry valid | M |
| QEMU copies the whole 64 KiB flash page into the window RAM when the MMU entry is written (`QEMU:esp32c3_cache.c:73-83`); a read costs nothing | R |

Bytes read through the window, in order **(M, T2)**: header `0x00–0x07` (`+1457`), extended header `0x08–0x17` (`+1531`), header again `0x00–0x17` (SHA buffer, `+6007`), then per segment its 8-byte header followed by its data as words (`0x18…`), then the checksum byte (file offset `0x520F`), the last 8 bytes before it (`0x5208–0x520F`, SHA padding block), and the 32-byte digest (`0x5210–0x522F`).

## F. What the ROM validates in the image at flash offset 0

All **R** from `ets_run_flash_bootloader@0x40049d20`, each confirmed by a QEMU run (**M**, §H) unless noted.

| Step | Check | On failure |
|---|---|---|
| 1 | `hdr[0] == 0xE9`; `hdr[1]` (segment count) ≤ 32; `hdr[2]` (SPI mode) ≤ 10; `hdr[3] & 0xF` (speed) ≤ 2 or == 15 | direct-boot test (below), then `invalid header`, then **retry from `SPI_WakeUp`, forever** |
| 2 | segment count ≤ 16 | `Invalid image block count: %d (max %d)`, return 1 |
| 3 | extended header bytes 4–5 (`chip_id`, flash offset 12) == 5 | `Invalid chip id…`, return 1 |
| 4 | per segment (8-byte header `load_addr`, `len`): `len % 4 == 0`, `load_addr % 4 == 0`, `len ≤ 0x100000` | `Invalid image block, can't boot.`, return 1 |
| 5 | per segment: `load_addr + len ≤ sp − 1024` **or** `load_addr ≥ 0x3FCDF664`; with `sp = 0x3FCDE560` the forbidden range is `0x3FCDE160–0x3FCDF663` (ROM stack and data) | `region … overlaps …, can't load`, return 1 |
| 6 | checksum: seed `0xEF`, XOR of every data word of every segment, folded to one byte (`b0^b1^b2^b3`); stored byte is the last byte of the 16-byte-aligned block after the last segment (`end + (16 − end % 16) − 1`) | `Checksum failure…`, return 1 |
| 7 | SHA-256 of `[0, end + pad)` (`pad = 16 − end % 16`, checksum byte included) against the 32 bytes that follow | prints 4 lines and **continues** (aborts with a software reset only if secure boot is enabled) |

Not checked **(R; M for the last one)**: the header's flash-size nibble (`hdr[3] >> 4`), `min_chip_rev`/`max_chip_rev`, the entry address, and the `hash_appended` byte (extended header byte 15). An image with `hash_appended = 0` and a stale digest is still hashed and compared (`nohashflag` case, §H).
A segment whose `load_addr` has bit 31 set is a "command block" (`ets_common_command`, `cmd len %d`) and takes a different branch; not analysed, never produced by esptool for a bootloader **(R, `@0x40049fc4`)**.
Direct boot **(R+M)**: on an invalid header, if secure boot is off and `EFUSE.RD_REPEAT_DATA3` bit 1 (`DIS_DIRECT_BOOT`) is 0 and the first two flash words are both `0xAEDB041D`, `ets_run_direct_boot@0x40049c50` maps 128 pages on both buses and jumps to `0x42000008`.

Hashing: **hardware SHA peripheral**, not software **(M: 329 `MODE`/`START or CONTINUE` sequences; R: `ets_sha_process@0x400495be`)**. Details a model needs:

| Item | Fact | Tag |
|---|---|---|
| Block input | 16 word writes to `M_MEM0…15`; the bytes of the message are the bytes of the words in memory order (first block starts with `0x4f0203e9` = header bytes `e9 03 02 4f`), so SHA word `W[i] = bswap32(M_MEM[i])` | M |
| Trigger | `START = 1`: compress from the SHA-256 initial value; `CONTINUE = 1`: compress from the current `H` | A (usual accelerator semantics; register addresses `IDF:hwcrypto_reg.h:49-50`), M (QEMU's result equals `hashlib.sha256` of the same bytes) |
| Digest output | `H_MEM0…7` read as words give the digest bytes in memory order: `H_MEM[i] = bswap32(H[i])` (read `0x1c59639a` for digest `9a 63 59 1c…`) | M |
| Busy | written block first, then poll `BUSY != 1`, then trigger; no wait after the trigger | R |
| Data source | header and padding blocks come from a RAM buffer; **segment data is hashed from its RAM copy after the copy**, not from the flash window | M+R |
| Length | image bytes + `0x80` + zeros + 64-bit big-endian bit count, built in software (`ets_sha_finish@0x40049756`); 21,008 bytes ⇒ 329 blocks | M+R |

## G. Lines printed on this path

`ets_printf` expands `\n` to `\r\n` **(M: characters `0x0d`, `0x0a`)**. 136 characters for the reference image.

| Line (format, `ROM3 .rodata` address) | Printed by, when | Value source | Tag |
|---|---|---|---|
| `SPIWP:0x%02x\n` (`0x3ff1bef0`) | `ets_config_flash_by_image_hdr+0x32`, after RDID, before the clock is changed | SPI-pad eFuse blank ⇒ extended header byte 0 (`wp_pin`, flash offset 8; `0xee`). Otherwise `ets_efuse_get_wp_pad()`, preceded by `configspi:0x%08x\n` | M+R |
| `mode:%s, clock div:%d\n` (`0x3ff1bf00`) | same function, after both `CLOCK` writes and the read-mode switch | `%s` = name table `0x3ff1c14c` indexed by `hdr[2]`: `QIO`, `QOUT`, `DIO`, `DOUT`, `FAST_RD`, `SLOW_RD`, `OPI_STR`, `OPI_DTR`, `OOUT`, `OIO_STR`, `OIO_DTR`. `%d` = `(hdr[3] & 0xF) + 2` when that nibble is 0, 1 or 2, else 1 | M+R |
| `load:0x%08x,len:0x%x\n` (`0x3ff1c024`) | once per segment, **before** the segment checks and the copy | segment header words 0 and 1 | M+R |
| `entry 0x%08x\n` (`0x3ff1c12c`) | after `Cache_MMU_Init`, immediately before the jump | header bytes 4–7 | M+R |

Measured for the reference `flash_hello.bin` and for the real CrossPoint C3 `bootloader.bin` (18,672 B, sha256 `6e6b0d38…9fb6`, from `crosspoint-tools/public/firmware/`) **(M)**:

```
SPIWP:0xee                         SPIWP:0xee
mode:DIO, clock div:1              mode:DIO, clock div:1
load:0x3fcd5820,len:0x15a4         load:0x3fcd5820,len:0x1010
load:0x403cbf10,len:0xc64          load:0x403cbf10,len:0x9f4
load:0x403ce710,len:0x2fd0         load:0x403ce710,len:0x2e94
entry 0x403cbf1a                   entry 0x403cbf10
```

`flashboot/tools/romcheck.py` re-implements §F and §G from the disassembly and its output is identical to the QEMU UART capture for 15 of the 16 images of this report (the two above and every row of §H); the exception is the direct-boot image, a branch the script does not implement **(M)**.

## H. Failure paths (all M: QEMU UART captures, 2 MiB images built by `flashboot/tools/mkimg.py`; R: branch targets)

| Image | Output after the three banner lines | Then |
|---|---|---|
| all `0xFF` | `invalid header: 0xffffffff` | repeated without delay (125,760 lines in 6 s of QEMU). One iteration is 4,317 instructions in QEMU (trace `t3_empty_mmio.txt`): `SPI_WakeUp` (`0xAB`), `RD_REPEAT_DATA3` read, a **full remap** (the loop zeroes the mapping state first: cache suspend with its four polls, invalidate, MMU entry 0 ← 0, resume), header read at `0x3C000000`, `RD_REPEAT_DATA2` read, `RD_REPEAT_DATA3` read (direct-boot bit), the print. No RDID, no SPIWP line. Never exits |
| all `0x00` | `invalid header: 0x00000000` | same loop |
| byte 0 ← `0x00` | `invalid header: 0x4f020300` | same loop (bytes printed as `hdr[3] hdr[2] hdr[1] hdr[0]`) |
| `hdr[2]` ← 11 | `invalid header: 0x4f0b03e9` | same loop |
| segment count ← 33 | `invalid header: 0x4f0221e9` | same loop |
| segment count ← 17 | `Invalid image block count: 17 (max 16)` / `ets_main.c 333 ` | hang |
| `chip_id` ← 9 | `Invalid chip id. Expected 5 read 9. Bootloader for wrong chip?` / `ets_main.c 333 ` | hang (after RDID, before SPIWP) |
| segment 0 `len` ← `0x15a6` | `SPIWP…`, `mode…`, `load:0x3fcd5820,len:0x15a6`, `Invalid image block, can't boot.`, `ets_main.c 333 ` | hang |
| segment 0 address ← `0x3fcde000` | …, `load:0x3fcde000,len:0x15a4`, `region 0x3fcde000-0x3fcdf5a4 overlaps 0x3fcde160-0x3fcdf664, can't load`, `ets_main.c 333 ` | hang |
| one data bit flipped | 3 `load:` lines, `Checksum failure. Calculated 0x4c stored 0x4d`, `ets_main.c 333 ` | hang |
| checksum byte flipped | 3 `load:` lines, `Checksum failure. Calculated 0x4d stored 0xb2`, `ets_main.c 333 ` | hang |
| digest byte flipped | 3 `load:` lines, `SHA-256 comparison failed:`, `Calculated: <64 hex>`, `Expected: <64 hex>`, `Attempting to boot anyway...`, `entry 0x403cbf1a` | **boots normally** |
| `hash_appended` ← 0, digest untouched | same four SHA lines (digest of the modified header differs), `entry …` | boots normally |
| first 8 bytes ← `1d 04 db ae` ×2 | (nothing from the ROM) | jumps to `0x42000008` (QEMU then reports an illegal instruction there) |

"hang" **(R)**: `ets_run_flash_bootloader` returns 1, `main@0x40047eba` prints `%s %u \n` with `ets_main.c`, 333 and executes `j .` at **`0x40047ed2`**. The `ets_loader.c NN ` variants hang inside `ets_loader_map_range` the same way.
Bad checksum is detected only after all segments were copied to RAM and all `load:` lines printed.

Hardware behaviour after the hang or the endless loop: QEMU never resets (**M**: one banner in 45 s of wall time for the checksum case). On real chips a watchdog resets the system: several forum logs of blank ESP32-C3 parts show `rst:0x7 (TG0WDT_SYS_RST)` followed by `Saved PC:0x4004d1f8` and repeated `invalid header: 0xffffffff` (search-engine summaries of esp32.com posts `p=87827`, `p=151330`, `p=154723`; the pages themselves could not be fetched — **R, second-hand**). `0x4004d1f8` is the `SPI1.CMD` poll of `SPI_WakeUp`, which matches the loop above and shows that the `0xAB` command dominates the loop time on hardware.

## I. Where QEMU differs from hardware on this path

| # | QEMU behaviour (M unless noted) | Hardware / recommended model |
|---|---|---|
| 1 | SPI0 (`0x60003000`) is not a device: reads 0, writes dropped | storage (R: registers are R/W); visible later as `SPI Mode : SLOW READ` instead of `DIO` |
| 2 | IO_MUX, `SENSITIVE`, `ASSIST_DEBUG` read 0, writes dropped; `SYSTEM.CACHE_CONTROL` and `PERIP_CLK_EN0/1`, `PERIP_RST_EN0/1` read 0 after being written | storage |
| 3 | `EXTMEM.ICACHE_CTRL1` resets to 0 | SVD and `IDF:extmem_reg.h:23-29`: 3 |
| 4 | `ICACHE_SYNC_CTRL` reads 0 at reset; `DONE` appears on the read after the write | `IDF:extmem_reg.h:147-153`: `INVALIDATE_ENA` default 1, `SYNC_DONE` default 0; state after the reset-time invalidation unknown (A: done) |
| 5 | SPI1 transfers complete inside the `CMD` write; flash is a generic m25p80 with id `9D 60 18`, status 0; `FLASH_RES` clocks `0xAB` + 3 bytes and logs "Read id (command 0x90/0xAB) is not supported" | transfer takes real time; with `RESANDRES = 0` only `0xAB` is expected on the wire (A); id of the real part unknown |
| 6 | flash page copied into the window when the MMU entry is written; no cache, no miss cost | 32-byte line fills through SPI0 |
| 7 | SHA result available at once; `BUSY` always 0 | compression takes time (duration unknown) |
| 8 | USB-Serial-JTAG stub: `EP1_CONF` reads 0, so `usb_uart_tx_one_char@0x4004c8e2` polls bit 1 5,000 times (5,236 reads measured), then clears its "ready" flag and never writes `EP1` again; ≈383,000 extra instructions before the end of the banner | inkemu already models the FIFO as always free, so it writes every character to `EP1` |
| 9 | no flash-boot watchdog: a hung ROM stays hung | TG0 watchdog reset (second-hand evidence, §H) |
| 10 | 8 bytes of `.data_btdm` differ from the rev3 ELF | irrelevant here |

## J. Minimal model per block, and what inkemu lacks today

"Today" = `crates/soc-esp32c3/src/periph` at commit `2a1fd22` and the 176 warnings of `run.err`.

| Block | Needed for the ROM flash path | inkemu today |
|---|---|---|
| GPIO | `ENABLE` (`+0x20`), `FUNC0_IN_SEL_CFG`, `FUNC1_IN_SEL_CFG` (`+0x154`, `+0x158`): storage | only `STRAP` |
| IO_MUX | `GPIO12…17`: storage | present |
| RTC_CNTL | `DIG_PAD_HOLD`: storage | present (no warning) |
| SYSTEM | `CACHE_CONTROL` (`+0x40`): storage; `PERIP_CLK_EN1`, `PERIP_RST_EN1` | `+0x40` missing; gates present |
| SENSITIVE | `INTERNAL_SRAM_USAGE_1` (`0x600C101C`): storage | missing (new block) |
| SPI1 | §D.2 | missing (new block) |
| SPI0 | §D.2 | missing (new block) |
| EXTMEM | `ICACHE_CTRL`, `SYNC_ADDR`, `SYNC_SIZE`, `CACHE_WRAP_AROUND_CTRL`, `CACHE_MMU_OWNER`: storage; `CACHE_ILG_INT_CLR`: write-only; `SYNC_CTRL`, `PRELOAD_CTRL`, `AUTOLOAD_CTRL`, `FREEZE`: the completion rules of §C rows 5–8 | only `ICACHE_CTRL1` |
| MMU table | 128 words at `0x600C5000`, write (reads not needed by the ROM) | missing |
| Flash window | D-bus `0x3C000000–0x3C7FFFFF`: word and byte reads translated through the table; the I-bus window `0x42000000` is not needed before the bootloader | missing |
| Flash image | backing store for the window and for SPI1 | missing |
| SHA | `MODE`, `START`, `CONTINUE`, `BUSY`, `H_MEM0…7`, `M_MEM0…15`; SHA-256 compression (mode 2) | missing (new block; needs a SHA-256 compression function) |
| SRAM | data writes through the I-bus view `0x4038_0000+` (segments 1 and 2 are stored there by `sw`) | to verify in the bus tests |
| eFuse | blank values of `RD_REPEAT_DATA2/3`, `RD_MAC_SPI_SYS_1/2` | present |

Expected instruction budget in inkemu (**A**, derived): banner at 17,932 instructions (boot-chain §M.3) + ≈70 before `main+0x64` + ≈120,700 ⇒ the bootloader entry near instruction 139,000 for the reference image; the per-character console path differs from QEMU (USB FIFO writes), so this is an order of magnitude, not an oracle.

## K. Corrections to `esp32c3-boot-chain.md` (that file is left untouched)

| Place | Statement there | Established here |
|---|---|---|
| §D.2 step 1–2, §J row 14 | `spi_flash_boot_attach` ends with `SPI_WakeUp`, `Wait_SPI_Idle` and a RDSR through the `USR` path | `a2 == 0`: no wake-up, no status read in the attach. `SPI_WakeUp` is called from `ets_run_flash_bootloader`; RDSR uses `CMD.FLASH_RDSR` → `RD_STATUS` and happens twice, inside `ets_config_flash_by_image_hdr` (M) |
| §J row 15 | `ROM_Boot_Cache_Init` read-modify-writes `EXTMEM.AUTOLOAD_CTRL` | it read-modify-writes `SYSTEM.CACHE_CONTROL`; `AUTOLOAD_CTRL` is only read (M) |
| §J rows 17, 22 | cache suspend/invalidate/resume and MMU rewrite for every segment | once per window change; once in total for an image below 64 KiB (M) |
| §H, §J row 16 | `SECURE_BOOT_EN` = `RD_REPEAT_DATA2` b16, `FLASH_TPUW` = b21:18 | b20 and b31:28 (R: `ets_efuse_secure_boot_enabled@0x40046d12`, `ets_efuse_get_flash_delay_us@0x40046d74`, `IDF:efuse_reg.h:411,432`) |
| §I.1 | "segment count ≤ 16", "optional appended SHA-256" | ≤ 32 in the header gate, then ≤ 16; the digest is always compared, mismatch non-fatal without secure boot (M) |

## L. Open questions, ranked by risk

| # | Question | Why it matters | Status |
|---|---|---|---|
| 1 | Durations on hardware: SPI1 commands (the two IDF descriptions of the post-`RES` delay disagree: `CS_HOLD_DLY_RES × 512` SPI clocks in `spi_mem_reg.h:256`, `× 4` when `FLASH_RES_DLY_128 = 0` in `:910`), a cache-line fill, a SHA block, and the MSPI clock source while the CPU still runs from the crystal | cycle accuracy of the ROM phase and of every later flash access; nothing here was measured on silicon, QEMU is instantaneous | unknown |
| 2 | Flash-boot watchdog: which timer, timeout, and reset reason, when the ROM hangs or loops | needed to reproduce the reset loop of a blank or corrupt flash (`rst:0x7`, `Saved PC`), and it bounds how slow the modelled ROM path may be before the bootloader disables it | second-hand evidence only (§H) |
| 3 | JEDEC id and power-up status byte of the real X4 flash | the ROM only stores the id, but the bootloader and `esp_flash` branch on it; WIP = 0 assumed | unknown (already boot-chain §K.1) |
| 4 | What a D-bus access to `0x3C000000` does when the cache is disabled, the bus is shut or the MMU entry is invalid | not on the happy path; decides the diagnostic inkemu should raise instead of returning data | not investigated |
| 5 | Hardware state of the EXTMEM completion bits right after reset and their latency after a request; idle value of `SPI0.FSM` and of the `SPI1.CMD` status nibbles | the model assumes "done/idle at once", which the ROM accepts; a wrong reset value is a hang (as today with `ICACHE_SYNC_CTRL`) | assumed |
| 6 | Bootloader larger than 64 KiB or a range leaving the mapped window | remap branch of `ets_loader_map_range` read but not executed | read only |
| 7 | `W0[31:24]` after RDID and `RD_STATUS[15:8]` after RDSR | masked or ignored by the ROM; the bootloader may look at them | unknown |
| 8 | Paths not analysed: non-default SPI pads from eFuse (GPIO matrix), QIO/QOUT (`Enable_QMode`), OPI modes, `FORCE_SEND_RESUME`, non-zero drive strengths in `SetSpiDrvs`, command blocks, secure boot, the body of direct boot | none is reachable with a blank eFuse and a DIO image | out of scope |
| 9 | rev0 and rev101 ROMs | this report is rev3 only; addresses differ and the behaviour was not compared | not done |

## M. Artefacts and reproduction (scratch, not in the repository)

Directory `…/scratchpad/flashboot/`: `tools/` (scripts written for this report), `img/` (2 MiB test images), `out/` (traces and UART captures).
Container: `docker run -d --name inkemu-flashboot --network none --entrypoint sleep -v …/oracle:/w:ro -v …/rom/x:/rom:ro -v …/flashboot/tools:/tools:ro -v …/flashboot/img:/img:ro -v …/flashboot/out:/out inkemu/esp-idf infinity` (everything third-party is mounted read-only; only `/out` is writable).

| Step | Command (inside the container after `. /w/idfenv.sh`, unless "host") |
|---|---|
| T1 | `qemu-system-riscv32 -nographic -machine esp32c3 -icount 3 -drive file=/w/flash_hello.bin,if=mtd,format=raw,readonly=on -accel tcg,one-insn-per-tb=on -d exec,nochain -trace memory_region_ops_read -trace memory_region_ops_write -trace 'm25p80_*' -D /out/t1.log -serial file:/out/t1_uart.log -monitor none` (stop after a few seconds), then host: `python3 -I tools/mmio_filter.py out/t1.log out/rev3.nm <repo>/crates/svd-regs/src/esp32c3.rs --stop-pc 403cbf1a` |
| T2 | same QEMU line with `-d cpu,nochain -D /dev/stdout` piped into `python3 /tools/cpu_filter.py <qemu share>/esp32c3-rom.bin /out/rev3.nm 403cbf1a /out/t2_calls.txt /out/t2_access.txt main` (≈5 min) |
| T3 (one `invalid header` iteration) | T1 command line with `/img/empty.bin`, filtered the same way → `out/t3_empty_mmio.txt` |
| Failure cases | host: `python3 -I tools/mkimg.py …/oracle/flash_hello.bin img`; container: `/tools/qfail.sh <name> <seconds>` → `/out/fail_<name>.uart` |
| Static prediction | host: `python3 -I tools/romcheck.py <image>` |
| Disassembly of one ROM function | host: `tools/fn.sh <symbol>…`; strings and tables: `python3 -I tools/rstr.py <elf> <addr>[:wN or :bN]` |

Checksums: `out/t1_mmio.txt` sha256 `7a4723b7…463a`, `out/t2_access.txt` `7a07a8c4…a2e0`, `out/t2_calls.txt` `7cfff674…16b1`.
Licence hygiene: QEMU source was read for behaviour only (two files, cited by line); no code or comment was copied. ROM facts come from the Apache-2.0 `esp-rom-elfs` ELF; register names and bit positions from the Apache-2.0 ESP-IDF headers and the vendored SVD.
