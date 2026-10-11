# ESP32-C3 2nd-stage bootloader: hardware accesses from its entry point to the application's first log line

Scope: which hardware the ESP-IDF v5.5.2 bootloader touches, in order, from its entry point to the jump into the application, then what the application touches up to its first log line; what each access needs from an emulator that has flash, cache/MMU, SHA and clock registers but no interrupt delivery, no timer and no watchdog expiry.
Date of measurements: 2026-10-11. The repository was not modified apart from this file.
Already established and not repeated here: `esp32c3-boot-chain.md` (ROM side, register layouts), `sp1b-test-oracle.md` (QEMU oracle, reproducible build), `esp32c3-svd.md` (names).

## 0. Sources, method, conventions

| Tag | Meaning |
|---|---|
| MEASURED | Observed in a QEMU run of the reproducible image (`flash_hello.bin`, sha256 `fc05ef5d…c39bf2`), or computed from that trace |
| READ | Read in ESP-IDF v5.5.2 sources (`IDF:<path>:<line>`, path relative to `/opt/esp/idf/components` in the `inkemu/esp-idf` image) or in a disassembly, cited `symbol@address` (`BTL:` = `bootloader.elf` sha256 `41708944…b0a44d`, `APP:` = `hello_world.elf` sha256 `43d81da8…5448e`, `ROM3:` = `esp32c3_rev3_rom.elf`) |
| ASSUMED | Inference, stated as such |

Method:

1. **MMIO trace (MEASURED).** QEMU `esp_develop_9.2.2_20250817` (the one in the image), third-party binaries run only inside Docker with the artefacts mounted read-only:
   ```
   qemu-system-riscv32 -nographic -machine esp32c3 -icount 3 \
     -drive file=/w/flash_hello.bin,if=mtd,format=raw,readonly=on -serial file:uart.log \
     -accel tcg,one-insn-per-tb=on -d exec,nochain,int,unimp,guest_errors \
     -trace 'memory_region_ops_*' -D /dev/stdout | filter.py 3000000 events.txt pcs.bin
   ```
   The released QEMU build has the `log` trace backend: `-trace 'memory_region_ops_*'` prints every access to a device model **and** to the catch-all `esp32c3.iomem` region (address, value, size, region name). Interleaved with `-d exec` on the same stream, each access follows the pc of the instruction that made it. This needs no plugin and is new with respect to `sp1b-test-oracle.md` §1c.
2. **Symbols.** pc → function with `nm` on the three ELFs. For an access made from ROM code the "caller" is the function of the last executed non-ROM instruction.
3. **Source reading (READ)** wherever QEMU's read values send the code down a path a real chip does not take (§2.1, §5).
4. **No-interrupt experiment (MEASURED)** for the verdict: gdbstub, breakpoint at `xPortStartScheduler`, then 30,000 single steps with the stub's default `qqemu.sstep=0x7` (`NOIRQ|NOTIMER`), which suppresses interrupt delivery (§9).

Conventions:

- **insn** = 1-based count of executed instructions since reset in this trace (consecutive duplicate trace entries removed). Bootloader entry `0x403cbf1a` is insn 521,511 and the application entry `0x40380330` (`call_start_cpu0`) is insn 1,683,222; `sp1b-test-oracle.md` gives 521,517 and 1,683,260 with a different duplicate rule. Use it as an ordering key, not as a cycle count.
- The trace does not show CSR accesses (taken from the disassembly), nor reads through the flash cache windows (plain memory in QEMU).
- "R/W count" = number of reads / writes of the register in the phase.
- Class: **a** plain store (reads return the last write, nothing else happens); **b** a specific read value is needed; **c** real behaviour is needed; **d** belongs to the later plan (interrupt delivery, timer alarm, watchdog expiry). "a→d" = storage is enough now, behaviour comes with the later plan.

## 1. Timeline (MEASURED)

| insn | Event |
|---|---|
| 1 – 521,510 | Mask ROM (covered by `esp32c3-boot-chain.md` §J). ROM banner characters at 391,181–521,473 |
| 521,511 | Bootloader entry `call_start_cpu0@0x403cbf1a` → `bootloader_init` (`IDF:bootloader_support/src/esp32c3/bootloader_esp32c3.c:127`) |
| 521,526 – 521,639 | Chip revision from eFuse, analog reset configuration, super-WDT auto-feed |
| 521,730 – 544,505 | `bootloader_init_mem` (PMP CSRs, no MMIO), BSS clear, `bootloader_clock_configure` (§5); 22,080 instructions are spent in three `ets_delay_us` calls |
| 544,525 – 549,266 | `bootloader_console_init`, banner `I (0) boot: ESP-IDF v5.5.2 2nd stage bootloader` |
| 549,307 – 550,229 | `cache_hal_init`, `mmu_hal_init` (ROM `ROM_Boot_Cache_Init`), `mmu_hal_unmap_all` |
| 550,269 – 550,610 | Flash id (RDID) and SFDP manufacturer byte over SPI1 |
| 550,711 – 573,480 | Bootloader header read through the cache window, `chip revision` / `efuse block revision` lines, SPI pin drive, flash unlock, `SPI Speed/Mode/Flash Size` lines, WRDI |
| 573,529 – 573,830 | `bootloader_check_wdt_reset`, `bootloader_config_wdt` (§6) |
| 573,951 – 579,341 | `Enabling RNG early entropy source...`, `bootloader_random_enable` |
| 579,477 – 621,989 | Partition table (MD5 in ROM software, no hardware), table printed |
| 622,067 – 1,511,325 | `bootloader_load_image`: header, six segments, SHA-256 over the whole image on the SHA peripheral (2,128 blocks), RNG read for the RAM-obfuscation value |
| 1,511,325 – 1,670,930 | De-obfuscation pass over the RAM segments (no MMIO) |
| 1,670,963 – 1,682,019 | `Loaded app from partition at offset 0x10000`, `Disabling RNG early entropy source...` |
| 1,682,124 – 1,683,217 | `bootloader_random_disable`, glitch-reset re-arm, final MMU mapping, cache enable, `bootloader_console_deinit` |
| 1,683,222 | Application entry `call_start_cpu0@0x40380330` |
| 1,688,504 – 1,691,526 | First application log line `I (18) cpu_start: Unicore app` |
| 1,714,177 – 2,184,895 | RTC slow-clock calibration on TIMG0 (470,567 instructions inside `ets_delay_us`) |
| 2,369,140 | `xPortStartScheduler` |
| 2,369,224 | **First interrupt taken** (software interrupt `FROM_CPU_INTR0`, CPU line 3) |
| 2,374,473 | `I (28) main_task: Started on CPU0` |
| 2,396,803 | `Hello world!` |
| 2,422,698 | Second interrupt (yield out of `vTaskDelay`), then periodic SYSTIMER ticks |

`INTERRUPT_CORE0.CPU_INT_ENABLE` (`0x600C2104`) is written 0 by the ROM at insn 6 and 5,688 and not written again before insn 2,194,353: **no interrupt can be delivered during the ROM flash-boot path or the bootloader** (MEASURED). `-d int` logs no interrupt or exception before insn 2,369,224.

## 2. Bootloader: first access to every register, in order (MEASURED)

Values are QEMU's. Where a real chip reads something else it is said in §2.1–§2.3. The 128 MMU entries and the SHA message/digest memories are folded into one row each.

| # | insn | Address | Register | 1st access | Function @pc | R/W count | Class |
|---|---|---|---|---|---|---|---|
| 1 | 521,526 | `0x60008858` | EFUSE.RD_MAC_SPI_SYS_5 | R `0x0` | `efuse_hal_get_major_chip_version`@403cedae | 10/0 | b |
| 2 | 521,535 | `0x60008850` | EFUSE.RD_MAC_SPI_SYS_3 | R `0x30c0000` | `efuse_hal_get_minor_chip_version`@403cedc0 | 7/0 | b |
| 3 | 521,555 | `0x6000810c` | RTC_CNTL.FIB_SEL | R `0x0` | `bootloader_ana_super_wdt_reset_config`@403cfee2 | 4/4 | a |
| 4 | 521,560 | `0x600080ac` | RTC_CNTL.SWD_CONF | R `0x0` | `bootloader_ana_super_wdt_reset_config`@403cfef0 | 2/2 | a→d |
| 5 | 521,609 | `0x600080d8` | RTC_CNTL.BROWN_OUT | R `0x0` | `bootloader_init`@403cc006 | 1/1 | a |
| 6 | 521,621 | `0x60008034` | RTC_CNTL.ANA_CONF | R `0x0` | `bootloader_ana_clock_glitch_reset_config`@403cff18 | 3/3 | a |
| 7 | 521,632 | `0x600080b0` | RTC_CNTL.SWD_WPROTECT | W `0x8f1d312a` | `bootloader_init`@403cc042 | 0/2 | a→d |
| 8 | 521,869 | `0x6000001c` | UART0.STATUS | R `0x0` | ROM `uart_tx_flush`@4004c48c ← `bootloader_clock_configure` | 1291/0 | b |
| 9 | 521,881 | `0x6000006c` | UART0.FSM_STATUS | R `0x0` | ROM `uart_tx_wait_idle`@4004c4c6 ← `bootloader_clock_configure` | 3/0 | b |
| 10 | 521,895 | `0x60008038` | RTC_CNTL.RESET_STATE | R `0x1` | ROM `rtc_get_reset_reason`@40048a94 ← `bootloader_clock_configure` | 15/0 | b |
| 11 | 521,902 | `0x60008070` | RTC_CNTL.CLK_CONF | R `0x0` | `rtc_clk_slow_src_get`@403d1144 | 12/9 | a |
| 12 | 521,967 | `0x60008080` | RTC_CNTL.RTC (0x80, SCK_DCAP) | R `0x0` | `rtc_clk_init`@403cc9c8 | 1/1 | a |
| 13 | 522,000 | `0x60008074` | RTC_CNTL.SLOW_CLK_CONF | R `0x0` | `rtc_clk_divider_set`@403d166e | 3/3 | a |
| 14 | 522,042 | `0x6000e044` | ANA_CONFIG | R `0xffffff` | `rtc_clk_init`@403cca20 | 2/7 | a |
| 15 | 522,098 | `0x600080b8` | RTC_CNTL.STORE4 | R `0x0` | `rtc_clk_xtal_freq_update`@403d135c | 3/1 | a |
| 16 | 522,120 | `0x600080bc` | RTC_CNTL.STORE5 | W `0x26252625` | `rtc_clk_apb_freq_update`@403d1392 | 1/2 | a |
| 17 | 522,127 | `0x600c0058` | SYSTEM.SYSCLK_CONF | R `0xa8401` | `rtc_clk_cpu_freq_get_config`@403d1290 | 4/2 | a+b (c) |
| 18 | 522,137 | `0x600c0008` | SYSTEM.CPU_PER_CONF | R `0x0` | `rtc_clk_cpu_freq_get_config`@403d12de | 3/1 | a+b (c) |
| 19 | 522,351 | `0x6000801c` | RTC_CNTL.TIMER1 | R `0x0` | `rtc_clk_8m_enable`@403d107c | 1/1 | a |
| 20 | 544,503 | `0x60008040` | RTC_CNTL.INT_ENA_RTC | W `0x0` | `bootloader_clock_configure`@403cc254 | 1/2 | a |
| 21 | 544,505 | `0x6000804c` | RTC_CNTL.INT_CLR_RTC | W `0xffffffff` | `bootloader_clock_configure`@403cc25a | 1/2 | a |
| 22 | 544,677 | `0x60000014` | UART0.CLKDIV | R `0x30015b` | `bootloader_console_init`@403cc876 | 2/2 | a |
| 23 | 544,697 | `0x60000078` | UART0.CLK_CONF | R `0x15122500` | `bootloader_console_init`@403cc8a8 | 1/1 | a |
| 24 | 544,834 | `0x60043004` | USB_SERIAL_JTAG.EP1_CONF | R `0x0` | ROM `usb_uart_tx_one_char`@4004c8f2 ← `bootloader_print_banner` | 1287/21 | b |
| 25 | 544,867 | `0x60000000` | UART0.FIFO | W `0x49` | ROM `uart_tx_one_char_uart.part.0`@4004c0de ← `bootloader_print_banner` | 0/1287 | c |
| 26 | 549,307 | `0x600c4040` | EXTMEM.ICACHE_AUTOLOAD_CTRL | R `0x8` | `s_cache_hal_init_ctx`@403d0ed2 | 102/0 | b |
| 27 | 549,334 | `0x600c4000` | EXTMEM.ICACHE_CTRL | R `0x1` | ROM `Cache_Enable_ICache`@4005143a ← `cache_hal_init` | 93/93 | a (c) |
| 28 | 549,342 | `0x600c4004` | EXTMEM.ICACHE_CTRL1 | R `0x0` | `cache_hal_init`@403d0f24 | 9/9 | a (c) |
| 29 | 549,362 | `0x600c0040` | SYSTEM.CACHE_CONTROL | R `0x0` | ROM `ROM_Boot_Cache_Init`@40050eea ← `mmu_hal_init` | 3/3 | a |
| 30 | 549,379 | `0x600c40c4` | EXTMEM.CACHE_MMU_OWNER | R `0x0` | ROM `Cache_Owner_Init`@40051642 ← `mmu_hal_init` | 1/1 | a |
| 31 | 549,387 | `0x600c5000–0x600c51fc` | MMU_TABLE[0..127] | W `0x100` | ROM `Cache_MMU_Init`@4005167c ← `mmu_hal_init` | 0/1325 | c |
| 32 | 549,779 | `0x600c407c` | EXTMEM.CACHE_ILG_INT_CLR | W `0xffffffff` | ROM `Cache_Set_Default_Mode`@40050ea6 ← `mmu_hal_init` | 0/1 | a |
| 33 | 549,780 | `0x600c40a8` | EXTMEM.CACHE_WRAP_AROUND_CTRL | R `0x0` | ROM `Cache_Set_Default_Mode`@40050ea8 ← `mmu_hal_init` | 1/1 | a |
| 34 | 549,792 | `0x600c101c` | SENSITIVE.INTERNAL_SRAM_USAGE_1 | R `0x0` | ROM `Cache_Occupy_ICache_MEMORY`@4005165a ← `mmu_hal_init` | 1/1 | a |
| 35 | 549,806 | `0x600c402c` | EXTMEM.ICACHE_SYNC_ADDR | W `0x0` | ROM `Cache_Invalidate_ICache_Items`@40050f2c ← `mmu_hal_init` | 0/50 | a |
| 36 | 549,807 | `0x600c4030` | EXTMEM.ICACHE_SYNC_SIZE | R `0x0` | ROM `Cache_Invalidate_ICache_Items`@40050f2e ← `mmu_hal_init` | 50/50 | a |
| 37 | 549,815 | `0x600c4028` | EXTMEM.ICACHE_SYNC_CTRL | R `0x2` | ROM `Cache_Invalidate_ICache_Items`@40050f42 ← `mmu_hal_init` | 100/50 | b |
| 38 | 550,269 | `0x60002008` | SPI1.CTRL | R `0xac2008` | `bootloader_flash_execute_command_common`@403ce7c0 | 11/16 | a |
| 39 | 550,270 | `0x60002018` | SPI1.USER | R `0x800000c0` | `bootloader_flash_execute_command_common`@403ce7c4 | 33/33 | a |
| 40 | 550,271 | `0x6000201c` | SPI1.USER1 | R `0x5c000007` | `bootloader_flash_execute_command_common`@403ce7c8 | 11/11 | a |
| 41 | 550,272 | `0x60002020` | SPI1.USER2 | R `0x70000000` | `bootloader_flash_execute_command_common`@403ce7cc | 15/15 | a |
| 42 | 550,317 | `0x60002004` | SPI1.ADDR | W `0x0` | `bootloader_flash_execute_command_common`@403ce850 | 0/5 | a |
| 43 | 550,342 | `0x60002024` | SPI1.MOSI_DLEN | R `0x0` | `bootloader_flash_execute_command_common`@403ce8a8 | 5/5 | a |
| 44 | 550,363 | `0x60002028` | SPI1.MISO_DLEN | R `0x0` | `bootloader_flash_execute_command_common`@403ce8f4 | 5/5 | a |
| 45 | 550,367 | `0x60002000` | SPI1.CMD | R `0x0` | `bootloader_flash_execute_command_common`@403ce8fe | 14/7 | c |
| 46 | 550,381 | `0x60002058` | SPI1.W0 | R `0x18609d` | `bootloader_flash_execute_command_common`@403ce962 | 3/0 | c |
| 47 | 550,730 | `0x600c4034` | EXTMEM.ICACHE_PRELOAD_CTRL | R `0x2` | ROM `Cache_Disable_ICache`@40051402 ← `cache_hal_disable` | 49/0 | b |
| 48 | 550,761 | `0x600c40b0` | EXTMEM.CACHE_STATE | R `0x1` | ROM `Cache_Disable_ICache`@40051422 ← `cache_hal_disable` | 49/0 | b |
| 49 | 555,357 | `0x6000886c` | EFUSE.RD_SYS_PART1_DATA4 | R `0x1` | `efuse_hal_blk_version`@403ced54 | 2/0 | b |
| 50 | 559,661 | `0x60008848` | EFUSE.RD_MAC_SPI_SYS_1 | R `0x0` | ROM `ets_efuse_get_spiconfig`@40046b42 ← `bootloader_configure_spi_pins` | 2/0 | b |
| 51 | 559,662 | `0x6000884c` | EFUSE.RD_MAC_SPI_SYS_2 | R `0x0` | ROM `ets_efuse_get_spiconfig`@40046b46 ← `bootloader_configure_spi_pins` | 2/0 | b |
| 52 | 559,707 | `0x60009040` | IO_MUX.GPIO15 | R `0x0` | ROM `gpio_pad_set_drv`@40051b50 ← `bootloader_configure_spi_pins` | 1/1 | a |
| 53 | 559,729 | `0x60009048` | IO_MUX.GPIO17 | R `0x0` | ROM `gpio_pad_set_drv`@40051b50 ← `bootloader_configure_spi_pins` | 1/1 | a |
| 54 | 559,751 | `0x60009044` | IO_MUX.GPIO16 | R `0x0` | ROM `gpio_pad_set_drv`@40051b50 ← `bootloader_configure_spi_pins` | 1/1 | a |
| 55 | 559,774 | `0x6000903c` | IO_MUX.GPIO14 | R `0x0` | ROM `gpio_pad_set_drv`@40051b50 ← `bootloader_configure_spi_pins` | 1/1 | a |
| 56 | 559,797 | `0x60009034` | IO_MUX.GPIO12 | R `0x0` | ROM `gpio_pad_set_drv`@40051b50 ← `bootloader_configure_spi_pins` | 1/1 | a |
| 57 | 559,828 | `0x60009038` | IO_MUX.GPIO13 | R `0x0` | ROM `gpio_pad_set_drv`@40051b50 ← `bootloader_configure_spi_pins` | 1/1 | a |
| 58 | 559,840 | `0x60003008` | SPI0.CTRL | R `0x0` | `bootloader_flash_set_dummy_out`@403cec4a | 2/1 | a |
| 59 | 559,852 | `0x60003018` | SPI0.USER | R `0x0` | `bootloader_flash_cs_timing_config`@403cec08 | 1/1 | a |
| 60 | 559,856 | `0x60003010` | SPI0.CTRL2 | R `0x0` | `bootloader_flash_cs_timing_config`@403cec14 | 2/2 | a |
| 61 | 559,867 | `0x60002010` | SPI1.CTRL2 | R `0x41` | `bootloader_flash_cs_timing_config`@403cec32 | 2/2 | a |
| 62 | 560,056 | `0x60003054` | SPI0.FSM | R `0x0` | ROM `Wait_SPI_Idle`@4004d206 ← `bootloader_init_spi_flash` | 2/0 | b |
| 63 | 560,088 | `0x6000202c` | SPI1.RD_STATUS | R `0x0` | ROM `esp_rom_spiflash_read_status`@4004d188 ← `bootloader_init_spi_flash` | 4/2 | c |
| 64 | 573,536 | `0x600c0000` | SYSTEM.CPU_PERI_CLK_EN | R `0x0` | `bootloader_init`@403cc166 | 1/1 | a |
| 65 | 573,540 | `0x600c0004` | SYSTEM.CPU_PERI_RST_EN | R `0x0` | `bootloader_init`@403cc170 | 1/1 | a |
| 66 | 573,545 | `0x600ce044` | ASSIST_DEBUG.CORE_0_RCD_EN | W `0x3` | `bootloader_init`@403cc17e | 0/1 | a |
| 67 | 573,561 | `0x600080a8` | RTC_CNTL.WDTWPROTECT | W `0x50d83aa1` | `wdt_hal_write_protect_disable`@403d0d54 | 0/6 | a→d |
| 68 | 573,570 | `0x60008090` | RTC_CNTL.WDTCONFIG0 | R `0x0` | `wdt_hal_set_flashboot_en`@403d0da0 | 14/14 | a→d |
| 69 | 573,764 | `0x60008834` | EFUSE.RD_REPEAT_DATA1 | R `0x0` | `wdt_hal_config_stage`@403d0c94 | 1/0 | b |
| 70 | 573,769 | `0x60008094` | RTC_CNTL.WDTCONFIG1 | W `0x956a0` | `wdt_hal_config_stage`@403d0ca0 | 0/1 | a→d |
| 71 | 573,778 | `0x600080a4` | RTC_CNTL.WDTFEED | R `0x0` | `wdt_hal_enable`@403d0d7e | 1/1 | a→d |
| 72 | 573,804 | `0x6001f064` | TIMG0.WDTWPROTECT | W `0x50d83aa1` | `wdt_hal_write_protect_disable`@403d0d5a | 0/2 | a→d |
| 73 | 573,813 | `0x6001f048` | TIMG0.WDTCONFIG0 | R `0x0` | `wdt_hal_set_flashboot_en`@403d0db4 | 2/2 | a→d |
| 74 | 578,405 | `0x6000811c` | RTC_CNTL.SENSOR_CTRL | R `0x0` | `bootloader_random_enable`@403cc26a | 2/2 | a |
| 75 | 578,483 | `0x6000e048` | ANA_CONFIG2 | R `0xffffff` | ROM `rom_get_i2c_hostid`@40038e34 ← `bootloader_random_enable` | 10/10 | a |
| 76 | 578,512 | `0x6000e000` | REGI2C host-0 command reg | W `0x4000769` | ROM `rom_chip_i2c_readReg_org`@40038e78 ← `bootloader_random_enable` | 20/10 | b |
| 77 | 579,238 | `0x600c0010` | SYSTEM.PERIP_CLK_EN0 | R `0x0` | `bootloader_random_enable`@403cc2e4 | 1/1 | a |
| 78 | 579,244 | `0x600c0018` | SYSTEM.PERIP_RST_EN0 | R `0x0` | `bootloader_random_enable`@403cc2f6 | 1/1 | a |
| 79 | 579,250 | `0x60040054` | APB_SARADC.CLKM_CONF | R `0x0` | `bootloader_random_enable`@403cc306 | 4/4 | a |
| 80 | 579,261 | `0x60040000` | APB_SARADC.CTRL | R `0x0` | `bootloader_random_enable`@403cc320 | 7/7 | a |
| 81 | 579,275 | `0x6004000c` | APB_SARADC.FSM_WAIT | R `0x0` | `bootloader_random_enable`@403cc344 | 3/3 | a |
| 82 | 579,307 | `0x60040018` | APB_SARADC.SAR_PATT_TAB1 | R `0x0` | `bootloader_random_enable`@403cc396 | 2/2 | a |
| 83 | 579,314 | `0x6004001c` | APB_SARADC.SAR_PATT_TAB2 | R `0x0` | `bootloader_random_enable`@403cc3a8 | 2/2 | a |
| 84 | 579,319 | `0x60040004` | APB_SARADC.CTRL2 | R `0x0` | `bootloader_random_enable`@403cc3b4 | 4/4 | a |
| 85 | 579,336 | `0x60040050` | APB_SARADC.DMA_CONF | R `0x0` | `bootloader_random_enable`@403cc3e0 | 2/2 | a |
| 86 | 623,618 | `0x600c0014` | SYSTEM.PERIP_CLK_EN1 | R `0x0` | ROM `ets_sha_enable`@400494e0 ← `bootloader_sha256_start` | 1/1 | a |
| 87 | 623,621 | `0x600c001c` | SYSTEM.PERIP_RST_EN1 | R `0x0` | ROM `ets_sha_enable`@400494e8 ← `bootloader_sha256_start` | 1/1 | a |
| 88 | 635,927 | `0x600260b0` | APB_CTRL.RND_DATA | R `0x5b863c83` | `bootloader_fill_random`@403cf456 | 1604/0 | b |
| 89 | 644,218 | `0x6003b000` | SHA.MODE | W `0x2` | ROM `ets_sha_process`@400495da ← `bootloader_sha256_data` | 0/2128 | c |
| 90 | 644,242 | `0x6003b080–0x6003b0bc` | SHA.M_MEM[0..15] | W `0x4f0206e9` | ROM `memcpy`@4005882e ← `bootloader_sha256_data` | 0/34048 | c |
| 91 | 644,295 | `0x6003b018` | SHA.BUSY | R `0x0` | ROM `ets_sha_process`@400495ea ← `bootloader_sha256_data` | 2129/0 | c |
| 92 | 644,304 | `0x6003b010` | SHA.START | W `0x1` | ROM `ets_sha_process`@40049628 ← `bootloader_sha256_data` | 0/1 | c |
| 93 | 644,420 | `0x6003b014` | SHA.CONTINUE | W `0x1` | ROM `ets_sha_process`@40049642 ← `bootloader_sha256_data` | 0/2127 | c |
| 94 | 1,508,620 | `0x600ce098` | ASSIST_DEBUG.C0RE_0_DEBUG_MODE | R `0x0` | `rv_utils_dbgr_is_attached`@403d16d4 | 2/0 | b |
| 95 | 1,511,100 | `0x6003b040–0x6003b05c` | SHA.H_MEM[0..7] | R `0xf39825da` | ROM `memcpy`@4005885c ← `bootloader_sha256_finish` | 8/0 | c |

Not in the list because never touched between bootloader entry and application entry (MEASURED): GPIO (`0x60004000`, including the strap register), TIMG0/TIMG1 timers and `RTCCALICFG*`, SYSTIMER, the RTC timer (`TIME_UPDATE`/`TIME_LOW0`), the interrupt matrix, UART1, SPI2, I2C, AES/RSA/HMAC/DS, GDMA, `RTC_CNTL.OPTIONS0` (see §2.1), `SYSTEM.CPU_INTR_FROM_CPU_n`.

CSRs used by the bootloader's own code (READ, exhaustive scan of the `BTL:` disassembly): `pmpaddr0..15` and `pmpcfg0..3` (`esp_cpu_configure_region_protection`, called from `bootloader_init_mem`, `IDF:bootloader_support/src/bootloader_mem.c:42`, because `CONFIG_BOOTLOADER_REGION_PROTECTION_ENABLE=y`; values written are never used afterwards), and the cycle counter `0x7E2`: read in `esp_log_early_timestamp@0x403cedd4`, `bootloader_fill_random@0x403cf458/0x403cf464`, `rtc_clk_init@0x403cca9e`, written in `rtc_clk_init@0x403ccab6`. ROM code called by the bootloader reads the user mirror `0x802` (`ROM3:ets_delay_us@0x400462cc`). `mtvec`, `mstatus`, `mie` are not touched: the bootloader runs on the ROM's vector table.

ROM functions entered during the bootloader phase (MEASURED, order of first entry): `memset`, `uart_tx_wait_idle`, `uart_tx_flush`, `rtc_get_reset_reason`, `ets_update_cpu_frequency`, `__udivdi3`, `ets_delay_us`, `ets_install_uart_printf`, `ets_get_cpu_frequency`, `ets_printf`/`ets_vprintf`/`ets_write_char`/`ets_write_char_uart`/`uart_tx_one_char`/`usb_uart_tx_one_char`/`usb_uart_tx_flush`, `Cache_Enable_ICache`, `Cache_Resume_ICache_Autoload`, `ROM_Boot_Cache_Init`, `Cache_Owner_Init`, `Cache_MMU_Init`, `Cache_Set_Default_Mode`, `Cache_Occupy_ICache_MEMORY`, `Cache_Invalidate_ICache_All`/`_Items`, `Cache_UnMask_Dram0`, `memcpy`, `Cache_Disable_ICache`, `Cache_Suspend_ICache_Autoload`, `__umoddi3`, `ets_efuse_get_spiconfig`, `ets_efuse_get_wp_pad`, `gpio_pad_set_drv`, `esp_rom_spiflash_wait_idle`, `Wait_SPI_Idle`, `esp_rom_spiflash_read_status`, `esp_rom_spiflash_config_param`, `rom_i2c_writeReg_Mask`/`rom_i2c_readReg`/`rom_chip_i2c_readReg(_org)`/`rom_get_i2c_hostid`/`rom_i2c_writeReg`/`rom_chip_i2c_writeReg`, `MD5Init/Update/Transform/Final`, `memcmp`, `ets_sha_enable/init/update/process/finish/get_state`. 452,849 of the 1,161,711 bootloader-phase instructions execute in ROM.

### 2.1 What a real chip does that this trace does not contain

| # | Difference | Cause in QEMU | What hardware does | Tag |
|---|---|---|---|---|
| 1 | PLL bring-up is skipped | `SYSTEM.SYSCLK_CONF` resets to `0xA8401` (`SOC_CLK_SEL`=1, already PLL) | With `SOC_CLK_SEL`=0 at reset, `rtc_clk_cpu_freq_set_config` runs `rtc_clk_bbpll_enable` + `rtc_clk_bbpll_configure`: `RTC_CNTL.OPTIONS0`, `I2C_MST_ANA_CONF0`, nine regi2c operations on the BBPLL block. Full sequence in §5 | READ `IDF:esp_hw_support/port/esp32c3/rtc_clk.c:244-249` |
| 2 | Every RTC_CNTL register except `OPTIONS0`, `RESET_STATE`, `STORE0–7` reads 0 and drops writes | not modelled | read-modify-write on real values; no decision depends on them in this path except `CLK_CONF` source fields (reset 0 = RC_SLOW / XTAL_D2) | MEASURED (`CLK_CONF` reads 0 after `0xC80000` was written) |
| 3 | `SPI Mode : SLOW READ` | SPI0 not modelled, `SPI0.CTRL` reads 0 | `bootloader_flash_get_spi_mode` decodes `SPI0.CTRL` bits FREAD_QIO/QUAD/DIO/DUAL/FASTRD; the ROM wrote the DIO mode there, so hardware (and a storage model) prints `DIO` | READ `IDF:bootloader_support/bootloader_flash/src/bootloader_flash.c:1040-1050` |
| 4 | No byte goes to USB-Serial-JTAG | `EP1_CONF` reads 0, the ROM drops the byte | ROM `ets_printf` still mirrors every bootloader and early-app character to `USB_SERIAL_JTAG.EP1` when `EP1_CONF` bit 1 is 1 (1,287 `EP1_CONF` reads = 1,287 UART characters in the bootloader) | MEASURED + `esp32c3-boot-chain.md` §G.3 |
| 5 | Flash is "ISSI" `0x9D6018` | QEMU's M25P80 | Other vendors take other branches of `bootloader_flash_unlock_default` (§2.2 SPI1 row) | READ `…/bootloader_flash.c:730-790` |
| 6 | SFDP command answers nothing | `M25P80: Unknown cmd 5a` | A real flash returns its SFDP table; only byte `0x10` (manufacturer) is looked at | MEASURED |
| 7 | regi2c reads return `0xFF` | window `0x6000E000–0x6000E0FF` reads `0xFFFFFF` | real analog register values; only read-modify-write uses in this path | MEASURED |
| 8 | Log timestamps `I (0)` … `I (18)` | QEMU's counter CSR is ≈0 at the `csrw 0x7e2` of `rtc_clk_init` and then advances 1.28 ticks per instruction whatever the selected CPU frequency | counts CPU clock cycles since reset, rescaled at each frequency switch (§4) | MEASURED (fit of the log timestamps against insn) |
| 9 | RNG values differ from run to run | host randomness (first read `0x5b863c83` in one run, `0x4aab8a06` in another) | true RNG | MEASURED |
| 10 | Chip revision < v0.3 only: two extra regi2c writes (`I2C_ULP_IR_FORCE_XPD_IPH`, `I2C_BIAS_DREG_1P1_PVT`) | QEMU is v0.3 | skipped on v0.3 and later | READ `IDF:bootloader_support/src/esp32c3/bootloader_esp32c3.c:90-93` |

### 2.2 Required behaviour per block

| Block | Registers touched | Class | What the model must provide | Source |
|---|---|---|---|---|
| eFuse | `RD_MAC_SPI_SYS_5` (wafer major, minor-hi), `RD_MAC_SPI_SYS_3` (minor-lo, block minor), `RD_SYS_PART1_DATA4` (block major), `RD_MAC_SPI_SYS_1/2` (SPI pads), `RD_REPEAT_DATA1` (`WDT_DELAY_SEL`) | b | Values of §8. Chip revision must be ≥ v0.3: both image headers carry `min_chip_rev_full = 3`, `max_chip_rev_full = 199` (bootloader.bin bytes 15–18 = `03 00 c7 00`), and an all-zero eFuse (v0.0) makes `bootloader_common_check_chip_validity` return `ESP_FAIL` on the bootloader's own header (the maximum is checked for the application only) | MEASURED reads; READ `IDF:bootloader_support/src/bootloader_common_loader.c:117-135`, `bootloader_init.c:50-61` |
| RTC_CNTL analog/reset config | `FIB_SEL`, `BROWN_OUT`, `ANA_CONF`, `SENSOR_CTRL`, `RTC (0x80)`, `SLOW_CLK_CONF`, `TIMER1`, `INT_ENA_RTC`, `INT_CLR_RTC`, `CLK_CONF` | a | Storage. `CLK_CONF[31:30]` = 3 is treated as invalid and replaced by RC_SLOW, so any value works | READ `IDF:bootloader_support/src/bootloader_clock_init.c:62-72` |
| RTC_CNTL retention | `STORE4` (XTAL MHz in both halves, bit 0/16 = ROM log off), `STORE5` (APB Hz >> 12 in both halves) | a | Storage that survives a CPU reset. If `STORE4` reads 0 the code assumes 40 MHz after a warning. `STORE5` is read back by `bootloader_console_init` | READ `IDF:hal/esp32c3/include/hal/clk_tree_ll.h:626-687` |
| RTC_CNTL reset cause | `RESET_STATE[5:0]` | b | 1 (POWERON). 15 reads: clock decision, `bootloader_check_wdt_reset`, `should_load` (RTC segment not loaded after deep sleep) | MEASURED |
| SYSTEM clock | `SYSCLK_CONF`, `CPU_PER_CONF` | a + b | Storage with the reset values of §5; `SOC_CLK_SEL` 3, or PLL with `CPUPERIOD_SEL` ≥ 2, ends in `abort()`. Class c only if the emulator scales time with the selected frequency | READ `…/rtc_clk.c:259-300` |
| SYSTEM gates | `CPU_PERI_CLK_EN/RST_EN`, `PERIP_CLK_EN0/1`, `PERIP_RST_EN0/1`, `CACHE_CONTROL (0x40)` | a | Storage | MEASURED |
| regi2c (analog I2C master) | `ANA_CONFIG 0x6000E044`, `ANA_CONFIG2 0x6000E048`, command register `0x6000E000` (host 0; host 1 would be `0x6000E004`, used only for blocks `0x62–0x64`) | a, b | Command register: **bit 25 = busy must read 0**, bits [23:16] = data byte of a read. ROM writes `0x04000000 | reg<<8 | block` for a read, `0x05000000 | data<<16 | reg<<8 | block` for a write, then spins while bit 25 is set. Any data byte is acceptable here (all uses are read-modify-write) | READ `ROM3:rom_chip_i2c_readReg_org@0x40038e58`, `rom_chip_i2c_writeReg@0x40039170`, `rom_get_i2c_hostid@0x40038e30` (settles `esp32c3-boot-chain.md` §K.7) |
| UART0 | `STATUS`, `FSM_STATUS`, `FIFO`, `CLKDIV`, `CLK_CONF` | b, c, a | `STATUS[25:16]` (TXFIFO_CNT) must fall to 0 (`uart_tx_flush`) and stay < 128 (`uart_tx_one_char`); `FSM_STATUS[7:4]` must read 0 (`ROM3:uart_tx_wait_idle@0x4004c4c6`); `FIFO` writes are the console. `CLKDIV` is rewritten to the same `0x30015B` (40 MHz XTAL / 115200: the UART core clock is the crystal, `CONFIG_ESP_ROM_UART_CLK_IS_XTAL=y`, so the CPU frequency switch does not change the baud divider) | MEASURED + READ |
| USB-Serial-JTAG | `EP1_CONF` (R per character, W 1 per line), `EP1` if bit 1 reads 1 | b, c | Either value works; bit 1 = 1 duplicates the console on USB | MEASURED |
| EXTMEM | `ICACHE_CTRL`, `ICACHE_CTRL1`, `ICACHE_SYNC_ADDR/SIZE/CTRL`, `ICACHE_PRELOAD_CTRL`, `ICACHE_AUTOLOAD_CTRL`, `CACHE_STATE`, `CACHE_MMU_OWNER`, `CACHE_ILG_INT_CLR`, `CACHE_WRAP_AROUND_CTRL` | a, b | `ICACHE_SYNC_CTRL` bit 1 (done) must read 1 after bit 0 is written (`ROM3:Cache_Invalidate_ICache_Items@0x40050f4e`); `ICACHE_PRELOAD_CTRL` bit 1 = 1 (`Cache_Disable_ICache@0x40051402`); `ICACHE_AUTOLOAD_CTRL` bit 3 = 1 (`Cache_Suspend_ICache_Autoload@0x40051000`); `CACHE_STATE[11:0]` = 1 (`Cache_Disable_ICache@0x40051422`: loops until `(reg & 0xFFF) == 1`). The cache is disabled/enabled 49 times and invalidated 50 times: an emulator with a real cache model must drop its lines on `ICACHE_SYNC_CTRL`, a transparent one can ignore it | READ ROM disassembly + MEASURED counts |
| MMU table | entries 0, 1, 127 used; 9 × `mmu_hal_unmap_all` (128 writes of `0x100`) | c | `bootloader_mmap` maps at entry 0 upward (D-bus `0x3C000000`), `bootloader_flash_read` uses **entry 127** (`0x3C7F0000`) as a one-page window; values written are flash page numbers 0–3. Final state at the jump: `[1]=1` (DROM `0x3C010000` ← flash `0x10000`), `[127]=1` ("map page for app to find the boot partition"), `[0]=2` (IROM `0x42000000` ← flash `0x20000`), all others `0x100`. No entry is ever read back. **All flash content is read through the cache window; SPI1 carries only the commands below** | MEASURED; READ `…/bootloader_flash.c:321-399, 496-531`, `IDF:bootloader_support/src/bootloader_utility.c:1036` ff. (`set_cache_and_start_app`) |
| SPI1 | `CTRL`, `USER`, `USER1`, `USER2`, `ADDR`, `MOSI_DLEN`, `MISO_DLEN`, `CMD`, `W0`, `RD_STATUS`, `CTRL2` | a, c | Configuration registers are saved, overwritten, restored: storage. `CMD` bit 18 (USR) and bit 27 (RDSR) must execute and self-clear. Commands issued, in order: **RDID `0x9F`** (24 bits in → `W0`), **SFDP `0x5A`** address `0x10`, 8 dummy bits, 8 bits in, **RESUME `0x7A`**, ROM RDSR (`CMD` bit 27, result in `RD_STATUS`) twice, **RDSR `0x05`** (8 bits in), **WRDI `0x04`**. Required answers: a JEDEC id whose manufacturer is not `0x20` (XMC start-up flow otherwise), SFDP byte `0x10` ≠ `0x20`, status 0. For a GD-style part the unlock also sends RDSR2 `0x35`; with a non-zero protection field it sends WREN `0x06` + WRSR `0x01`/`0x31` | MEASURED; READ `…/bootloader_flash.c:730-790, 954-962` |
| SPI0 | `CTRL`, `USER`, `CTRL2`, `FSM` | a, b | Storage; `FSM[6:4]` must read 0 (`ROM3:Wait_SPI_Idle@0x4004d206`) | READ |
| IO_MUX | `GPIO12..17` (drive strength) | a | Storage | MEASURED |
| ASSIST_DEBUG | `CORE_0_RCD_EN` (W 3), `C0RE_0_DEBUG_MODE` (R) | a, b | `DEBUG_MODE` bit 0 must read 0: 1 means "debugger attached" and the image checksum/hash verification is skipped | READ `IDF:bootloader_support/src/esp_image_format.c:188-208` |
| Watchdogs | `SWD_CONF`, `SWD_WPROTECT`, `WDTWPROTECT`, `WDTCONFIG0/1`, `WDTFEED`, `TIMG0.WDTWPROTECT`, `TIMG0.WDTCONFIG0` | a→d | Storage now (§6) | MEASURED |
| RNG entropy source | `APB_SARADC.*` (7 registers), `SENSOR_CTRL`, regi2c SAR block | a | Storage | MEASURED |
| RNG | `APB_CTRL.RND_DATA` (`0x600260B0`), 1,604 reads | b | **Must return varying non-zero data** (§2.3 item 1) | READ |
| SHA | `MODE`, `M_MEM`, `START`, `CONTINUE`, `BUSY`, `H_MEM` | c | §7 | MEASURED + READ |

### 2.3 Where a "reads 0, ignores writes" stub hangs or aborts

Ranked by how early they bite. All are in the bootloader.

1. **RNG reading 0** → endless loop. `process_segment_data` runs `while (ram_obfs_value[0] == 0 || ram_obfs_value[1] == 0) bootloader_fill_random(...)` (READ `IDF:bootloader_support/src/esp_image_format.c:735-736`), right after the `segment 0` line. A constant non-zero value is not safe either: each 32-bit word is the XOR of 1 + N reads (N = 801 in QEMU: 1,604 reads for the two words; N depends on the cycle counter model), and an even number of equal values XORs to 0. Use a deterministic PRNG.
2. **Cycle counter not advancing** → `bootloader_fill_random` waits `now - start >= 5120` cycles on CSR `0x7E2` per word (READ `IDF:bootloader_support/src/bootloader_random.c:73-78`), and `ets_delay_us` waits on CSR `0x802`.
3. **eFuse all zero** → chip revision v0.0, below the image's minimum v0.3: boot refused (READ, not run).
4. **Done/idle bits**: `EXTMEM.ICACHE_SYNC_CTRL` bit 1, `ICACHE_PRELOAD_CTRL` bit 1, `ICACHE_AUTOLOAD_CTRL` bit 3, `CACHE_STATE` = 1 (§2.2). Zero spins in ROM.
5. **`SPI1.CMD` not self-clearing**, or flash status with WIP set → spins.
6. Stubs that read 0 and are fine: regi2c busy bit, `UART0.STATUS`, `UART0.FSM_STATUS`, `SPI0.FSM`, `SHA.BUSY`, `ASSIST_DEBUG` debug mode, `RD_MAC_SPI_SYS_1/2`, `RD_REPEAT_DATA1`.

## 3. Application: entry to the first log line (MEASURED)

`call_start_cpu0` → cache/MMU HAL init → `esp_rtc_init`/`rtc_init` → `sar_periph_ctrl_init` → flash id → `system_early_init` prints `I (18) cpu_start: Unicore app`. 8,305 instructions, 396 accesses, 52 registers. Code already executes from the flash cache window (`rtc_init@0x420032bc`), so the I-bus mapping set up by the bootloader must work for instruction fetch.

| # | insn | Address | Register | 1st access | Function @pc | R/W count | Class |
|---|---|---|---|---|---|---|---|
| 1 | 1,683,229 | `0x600ce098` | ASSIST_DEBUG.C0RE_0_DEBUG_MODE | R `0x0` | `rv_utils_dbgr_is_attached`@40389934 | 1/0 | b |
| 2 | 1,683,246 | `0x60008038` | RTC_CNTL.RESET_STATE | R `0x1` | ROM `rtc_get_reset_reason`@40048a94 ← `call_start_cpu0` | 2/0 | b |
| 3 | 1,684,661 | `0x600c4040` | EXTMEM.ICACHE_AUTOLOAD_CTRL | R `0x8` | `s_cache_hal_init_ctx`@4038590c | 4/0 | b |
| 4 | 1,684,701 | `0x600c4000` | EXTMEM.ICACHE_CTRL | R `0x1` | ROM `Cache_Enable_ICache`@4005143a ← `cache_hal_init` | 1/1 | a (c) |
| 5 | 1,684,709 | `0x600c4004` | EXTMEM.ICACHE_CTRL1 | R `0x0` | `cache_hal_init`@40385996 | 4/4 | a (c) |
| 6 | 1,684,783 | `0x60008088` | RTC_CNTL.DIG_PWC | R `0x0` | `rtc_init`@420032bc | 10/10 | a |
| 7 | 1,684,789 | `0x6000808c` | RTC_CNTL.DIG_ISO | R `0x0` | `rtc_init`@420032ce | 10/10 | a |
| 8 | 1,684,827 | `0x600c2194` | INTERRUPT_CORE0.CPU_INT_THRESH | R `0x1` | `xPortSetInterruptMaskFromISR`@40383e98 | 8/16 | a→d |
| 9 | 1,684,913 | `0x6000e048` | ANA_CONFIG2 | R `0xffffff` | ROM `rom_get_i2c_hostid`@40038e34 ← `regi2c_ctrl_write_reg_mask` | 14/14 | a |
| 10 | 1,684,933 | `0x6000e044` | ANA_CONFIG | W `0xffffbfff` | ROM `rom_chip_i2c_readReg_org`@40038e60 ← `regi2c_ctrl_write_reg_mask` | 0/7 | a |
| 11 | 1,684,942 | `0x6000e000` | REGI2C host-0 command reg | W `0x4000d6d` | ROM `rom_chip_i2c_readReg_org`@40038e78 ← `regi2c_ctrl_write_reg_mask` | 28/14 | b |
| 12 | 1,685,397 | `0x60008034` | RTC_CNTL.ANA_CONF | R `0x0` | `rtc_init`@4200330e | 4/4 | a |
| 13 | 1,685,403 | `0x6000801c` | RTC_CNTL.TIMER1 | R `0x0` | `rtc_init`@4200331e | 2/2 | a |
| 14 | 1,685,420 | `0x6000802c` | RTC_CNTL.TIMER5 | R `0x0` | `rtc_init`@4200334a | 1/1 | a |
| 15 | 1,685,435 | `0x60008024` | RTC_CNTL.TIMER3 | R `0x0` | `rtc_init`@42003376 | 4/4 | a |
| 16 | 1,685,468 | `0x60008028` | RTC_CNTL.TIMER4 | R `0x0` | `rtc_init`@420033c8 | 4/4 | a |
| 17 | 1,685,496 | `0x60008030` | RTC_CNTL.TIMER6 | R `0x0` | `rtc_init`@4200340a | 2/2 | a |
| 18 | 1,685,514 | `0x6000886c` | EFUSE.RD_SYS_PART1_DATA4 | R `0x1` | `rtc_init`@42003438 | 2/0 | b |
| 19 | 1,686,146 | `0x60008858` | EFUSE.RD_MAC_SPI_SYS_5 | R `0x0` | `efuse_hal_get_major_chip_version`@40380508 | 3/0 | b |
| 20 | 1,686,159 | `0x60008850` | EFUSE.RD_MAC_SPI_SYS_3 | R `0x30c0000` | `efuse_hal_get_minor_chip_version`@40380520 | 1/0 | b |
| 21 | 1,686,800 | `0x600c40ac` | EXTMEM.CACHE_MMU_POWER_CTRL | R `0x0` | `rtc_init`@42003454 | 1/1 | a |
| 22 | 1,686,804 | `0x600c4008` | EXTMEM.ICACHE_TAG_POWER_CTRL | R `0x0` | `rtc_init`@42003462 | 1/1 | a |
| 23 | 1,686,808 | `0x600030dc` | SPI0.CLOCK_GATE | R `0x0` | `rtc_init`@4200346c | 1/1 | a |
| 24 | 1,686,812 | `0x600020dc` | SPI1.CLOCK_GATE | R `0x0` | `rtc_init`@4200347a | 1/1 | a |
| 25 | 1,686,818 | `0x60008070` | RTC_CNTL.CLK_CONF | R `0x0` | `rtc_init`@42003490 | 1/1 | a |
| 26 | 1,686,830 | `0x60008000` | RTC_CNTL.OPTIONS0 | R `0x0` | `rtc_init`@420034b2 | 4/4 | a |
| 27 | 1,686,865 | `0x60008080` | RTC_CNTL.RTC (0x80, SCK_DCAP) | R `0x0` | `rtc_init`@4200350e | 3/3 | a |
| 28 | 1,686,883 | `0x600c000c` | SYSTEM.MEM_PD_MASK | R `0x0` | `rtc_init`@4200354c | 1/1 | a |
| 29 | 1,686,906 | `0x6002609c` | APB_CTRL.FRONT_END_MEM_PD | R `0x0` | `rtc_sleep_pu`@40382808 | 3/3 | a |
| 30 | 1,686,923 | `0x6001d054` | BB.BBPD_CTRL | R `0x0` | `rtc_sleep_pu`@4038283e | 2/2 | a |
| 31 | 1,686,936 | `0x6001ccd4` | NRX.NRXPD_CTRL | R `0x0` | `rtc_sleep_pu`@4038285e | 3/3 | a |
| 32 | 1,686,954 | `0x60006090` | FE.FE_GEN_CTRL | R `0x0` | `rtc_sleep_pu`@40382898 | 1/1 | a |
| 33 | 1,686,960 | `0x600050f0` | FE2.FE2_TX_INTERP_CTRL | R `0x0` | `rtc_sleep_pu`@403828ae | 1/1 | a |
| 34 | 1,686,968 | `0x600260ac` | APB_CTRL.MEM_POWER_UP | R `0x0` | `rtc_sleep_pu`@403828f0 | 2/2 | a |
| 35 | 1,687,034 | `0x600c0008` | SYSTEM.CPU_PER_CONF | R `0x0` | `rtc_init`@4200360c | 1/1 | a+b (c) |
| 36 | 1,687,068 | `0x60008040` | RTC_CNTL.INT_ENA_RTC | W `0x0` | `rtc_init`@42003678 | 0/1 | a |
| 37 | 1,687,070 | `0x6000804c` | RTC_CNTL.INT_CLR_RTC | W `0xffffffff` | `rtc_init`@4200367e | 0/1 | a |
| 38 | 1,687,375 | `0x6000811c` | RTC_CNTL.SENSOR_CTRL | R `0x0` | `sar_periph_ctrl_init`@4200d432 | 1/1 | a |
| 39 | 1,687,443 | `0x60002008` | SPI1.CTRL | R `0xac2008` | `bootloader_flash_execute_command_common`@403812ca | 2/3 | a |
| 40 | 1,687,444 | `0x60002018` | SPI1.USER | R `0x800000c0` | `bootloader_flash_execute_command_common`@403812ce | 6/6 | a |
| 41 | 1,687,445 | `0x6000201c` | SPI1.USER1 | R `0x5c000007` | `bootloader_flash_execute_command_common`@403812d2 | 2/2 | a |
| 42 | 1,687,446 | `0x60002020` | SPI1.USER2 | R `0x70000000` | `bootloader_flash_execute_command_common`@403812d6 | 3/3 | a |
| 43 | 1,687,492 | `0x60002004` | SPI1.ADDR | W `0x0` | `bootloader_flash_execute_command_common`@40381362 | 0/1 | a |
| 44 | 1,687,525 | `0x60002024` | SPI1.MOSI_DLEN | R `0x0` | `bootloader_flash_execute_command_common`@403813dc | 1/1 | a |
| 45 | 1,687,551 | `0x60002028` | SPI1.MISO_DLEN | R `0x0` | `bootloader_flash_execute_command_common`@403814a8 | 1/1 | a |
| 46 | 1,687,555 | `0x60002000` | SPI1.CMD | R `0x0` | `bootloader_flash_execute_command_common`@403814b2 | 2/1 | c |
| 47 | 1,687,587 | `0x60002058` | SPI1.W0 | R `0x18609d` | `bootloader_flash_execute_command_common`@4038154a | 1/0 | c |
| 48 | 1,688,014 | `0x60008848` | EFUSE.RD_MAC_SPI_SYS_1 | R `0x0` | ROM `ets_efuse_get_spiconfig`@40046b42 ← `esp_mspi_get_io` | 6/0 | b |
| 49 | 1,688,015 | `0x6000884c` | EFUSE.RD_MAC_SPI_SYS_2 | R `0x0` | ROM `ets_efuse_get_spiconfig`@40046b46 ← `esp_mspi_get_io` | 6/0 | b |
| 50 | 1,688,471 | `0x60043004` | USB_SERIAL_JTAG.EP1_CONF | R `0x0` | ROM `usb_uart_tx_one_char`@4004c8f2 ← `system_early_init` | 31/1 | b |
| 51 | 1,688,500 | `0x6000001c` | UART0.STATUS | R `0x0` | ROM `uart_tx_one_char_uart.part.0`@4004c0d4 ← `system_early_init` | 31/0 | b |
| 52 | 1,688,504 | `0x60000000` | UART0.FIFO | W `0x49` | ROM `uart_tx_one_char_uart.part.0`@4004c0de ← `system_early_init` | 0/31 | c |

Notes on this phase:

- **No timer, SYSTIMER, TIMG, RTC-timer, RNG, GPIO or SHA access, and `CPU_INT_ENABLE` is still 0** (MEASURED). The only "time" source is the cycle-counter CSR (log timestamp).
- `INTERRUPT_CORE0.CPU_INT_THRESH` is the critical-section mask: `xPortSetInterruptMaskFromISR` reads it, writes 4, and `vPortClearInterruptMaskFromISR` writes the saved value back. It must read back what was written (MEASURED: 8 read / 16 write).
- `rtc_init` depends on the eFuse block version (READ `IDF:esp_hw_support/port/esp32c3/rtc_init.c:67, 174-219`): `BLK_VERSION_MAJOR` = 1 → `set_ocode_by_efuse` (`assert(calib_version == 1)`); = 0 → `calibrate_ocode()`, whose loop ends only when two regi2c flags read 1 or the RTC timer has advanced, i.e. never with regi2c data reading 0 and a frozen RTC timer; ≥ 2 → the assert fails. For chip revision ≥ v0.3 `set_rtc_dig_dbias` then reads the voltage calibration fields (`rtc_init.c:264-292`); all-zero values take the "not burnt" branch (QEMU's case).
- `0x6001D054`, `0x6001CCD4`, `0x60006090`, `0x600050F0`, `APB_CTRL.FRONT_END_MEM_PD`, `APB_CTRL.MEM_POWER_UP` are radio memory power bits set by `rtc_sleep_pu` (names READ `IDF:soc/esp32c3/include/soc/bb_reg.h:21`, `fe_reg.h:23,33`, `register/soc/nrx_reg.h:15`). Storage; they are outside every block inkemu models today.
- Flash: one RDID (`0x9F`) over SPI1, same protocol as the bootloader (the application links the same `bootloader_flash_execute_command_common`).

## 4. Time and delays

| Mechanism | Implementation | Hardware | Who uses it before the first app log line | Tag |
|---|---|---|---|---|
| `esp_rom_delay_us(us)` | `start = csrr 0x802; while (csrr 0x802 - start < us * s_ticks_per_us);` with `s_ticks_per_us` in ROM RAM at `0x3FCDF64C` | CSR `0x802` (user mirror of the cycle counter) | Bootloader, 3 calls: 50 µs (`rtc_clk_8m_enable`), 3 µs (`rtc_clk_fast_src_set`), 300 µs (`rtc_clk_slow_src_set`) = 3,131 + 194 + 18,755 instructions in QEMU | READ `ROM3:ets_delay_us@0x400462cc`, `IDF:esp_hw_support/port/esp32c3/include/soc/rtc.h:77-79`; MEASURED counts |
| `s_ticks_per_us` | written by `ets_update_cpu_frequency@0x400462e8` | RAM only | ROM boot sets 40; bootloader sets 80 in `rtc_clk_cpu_freq_to_pll_mhz` (insn 522,265); app sets 160 | READ + MEASURED |
| Log timestamp | `esp_log_early_timestamp` = `csrr 0x7e2 / (ets_get_cpu_frequency() * 1000)` (a `divu`) | CSR `0x7E2` | every log line (21 in the bootloader) | READ `BTL:esp_log_early_timestamp@0x403cedce` |
| `esp_cpu_get_cycle_count` / `esp_cpu_set_cycle_count` | `csrr 0x7e2` / `csrw 0x7e2` | CSR `0x7E2` | `rtc_clk_init` rescales the counter: `count * 80 / freq_before` (64-bit, ROM `__udivdi3`) | READ `IDF:esp_hw_support/port/esp32c3/rtc_clk_init.c:66`, `BTL:rtc_clk_init@0x403cca9e/0x403ccab6` |
| RNG pacing | `do { random ^= REG_READ(WDEV_RND_REG); now = ccount; } while (now - start < 5120);` once per 32-bit word | CSR `0x7E2`, `0x600260B0` | `bootloader_fill_random` (8 bytes, once per boot) | READ `IDF:bootloader_support/src/bootloader_random.c:35, 73-78` |
| SYSTIMER | — | — | not before insn 2,292,400 (`esp_timer_impl_early_init`) | MEASURED |
| TIMG `RTCCALICFG` (slow-clock calibration) | — | — | not before insn 1,714,191 (`rtc_clk_cal_internal`), i.e. just after the first app log line | MEASURED |
| RTC timer (`TIME_UPDATE`, `TIME_LOW0/HIGH0`) | — | — | not before insn 2,185,053 (`rtc_time_get`) | MEASURED |

**What an emulator without timers must provide for the bootloader:** only the cycle-counter CSR, readable at `0x7E2` and `0x802` (same counter), advancing with executed cycles, and writable through `0x7E2`. inkemu already has it (`crates/cpu-rv32/src/csr_esp32c3.rs`). No time-related MMIO register is read by the bootloader.

Division by zero: there is none to fear in practice. `esp_log_early_timestamp` divides by `s_ticks_per_us * 1000`, non-zero since the ROM stored 40. The 64-bit rescale divides by `freq_before`, which is `xtal / (PRE_DIV_CNT + 1)` for the crystal source and 80 or 160 for the PLL (§5); it would be 0 only if `PRE_DIV_CNT` read ≥ 40. RISC-V `div`/`divu` by zero does not trap, so the worst case is a wrong timestamp, not a hang (ASSUMED for the ROM's `__udivdi3`, not tested).

Consequences for comparisons against QEMU (MEASURED): QEMU's bootloader timestamps fit `ms = (insn − 522,270) / 64,000`, i.e. the counter is ≈0 when `rtc_clk_init` rewrites it and then advances 1.28 ticks per instruction (160 ticks/µs at 8 ns per instruction) while `s_ticks_per_us` is 80. A hardware-like counter gives different `I (n)` values and a different number of RNG reads (1,604 in QEMU). Mask timestamps and do not compare MMIO access counts inside timed loops.

## 5. Clock setup

Reset state on hardware (READ `IDF:soc/esp32c3/register/soc/system_reg.h:790-811`, `esp32c3-boot-chain.md` §E.1): `SYSCLK_CONF`: `PRE_DIV_CNT` = 1, `SOC_CLK_SEL` = 0 (crystal), `CLK_XTAL_FREQ` = 40, `CLK_DIV_EN` = 0. `CPU_PER_CONF`: `CPUPERIOD_SEL` = 0, `PLL_FREQ_SEL` = 1 (480 MHz).

Sequence of `bootloader_clock_configure` (READ `IDF:bootloader_support/src/bootloader_clock_init.c:27-84`, `IDF:esp_hw_support/port/esp32c3/rtc_clk_init.c:24-81`, `rtc_clk.c`, `IDF:hal/esp32c3/include/hal/clk_tree_ll.h`, `regi2c_ctrl_ll.h`); the "QEMU" column is what the trace shows (R = read, W = written value; QEMU reads 0 back from unmodelled RTC_CNTL registers, so written values show only the field being set).

| Step | Function | Register operations | QEMU (MEASURED) | Read-back needed |
|---|---|---|---|---|
| 1 | `esp_rom_output_tx_wait_idle(0)` | `UART0.STATUS` until TXFIFO_CNT = 0, `UART0.FSM_STATUS[7:4]` = 0 | insn 521,869–521,881 | b (§2.2) |
| 2 | reset-reason test | `RTC_CNTL.RESET_STATE`; `STORE5` only if the reason is CPU software reset | R `0x1` | b |
| 3 | `rtc_clk_slow_src_get`, `rtc_clk_fast_src_get` | R `RTC_CNTL.CLK_CONF` [31:30], [29] | R 0 | none |
| 4 | tuning | `RTC (0x60008080)` field SCK_DCAP; `CLK_CONF` field CK8M_DFREQ | W `0x3FC000`; W `0xC80000` | none |
| 5 | `rtc_clk_divider_set`, `rtc_clk_8m_divider_set` | 3 RMW on `SLOW_CLK_CONF` (last sets bit 22), 3 RMW on `CLK_CONF` (last sets bit 3) | W `0`, `0`, `0x400000`; W `0`, `0`, `0x8` | none |
| 6 | `regi2c_ctrl_ll_i2c_reset`, `…_i2c_bbpll_enable` | `ANA_CONFIG (0x6000E044)` |= `0x3FF << 8`, then clear bit 17 | R `0xFFFFFF`, W `0xFFFFFF`, W `0xFDFFFF` | none |
| 7 | `rtc_clk_xtal_freq_update(40)`, `rtc_clk_apb_freq_update(40 MHz)` | R `STORE4` (bit 0), W `STORE4 = 0x00280028`, W `STORE5 = 0x26252625` | same | retained |
| 8 | `rtc_clk_cpu_freq_get_config` | R `SYSCLK_CONF` (`SOC_CLK_SEL`; `PRE_DIV_CNT` if crystal), R `CPU_PER_CONF` ×2 if PLL | R `0xA8401` → PLL, 80 MHz | **b**: hardware gives crystal, divider `PRE_DIV_CNT + 1` = 2, `freq_before` = 20 |
| 9 | `rtc_clk_bbpll_enable` — **hardware only** | `RTC_CNTL.OPTIONS0`: clear `BB_I2C_FORCE_PD`, `BBPLL_FORCE_PD`, `BBPLL_I2C_FORCE_PD` | skipped | none |
| 10 | `rtc_clk_bbpll_configure(40, 480)` — **hardware only** | `CPU_PER_CONF.PLL_FREQ_SEL` ← 1; `I2C_MST_ANA_CONF0 (0x6000E040)`: clear bit 2, set bit 3; then regi2c on block `I2C_BBPLL`: write `MODE_HF = 0x6B`, write `OC_REF_DIV`, write `OC_DIV_7_0 = 8`, masked write `OC_DR1`, masked write `OC_DR3`, write `OC_DCUR`, masked writes `OC_VCO_DBIAS`, `OC_DHREF_SEL`, `OC_DLREF_SEL` | skipped | regi2c busy bit only. **There is no lock or calibration-done wait on the C3** (`regi2c_ctrl_ll.h` only has `calibration_start`) |
| 11 | `rtc_clk_cpu_freq_to_pll_mhz(80)` | `CPU_PER_CONF.CPUPERIOD_SEL` ← 0; `SYSCLK_CONF.PRE_DIV_CNT` ← 0; `SYSCLK_CONF.SOC_CLK_SEL` ← 1; W `STORE5 = 0x4C4B4C4B`; `ets_update_cpu_frequency(80)` | W `0`; W `0xA8400`; W `0xA8401`; same | retained (the app re-reads them) |
| 12 | cycle-count rescale | `csrr 0x7e2`, `__udivdi3`, `csrw 0x7e2` | ×80/80 | — |
| 13 | `rtc_clk_8m_enable(true, false)` | RMW `CLK_CONF`, RMW `TIMER1` (CK8M_WAIT), **delay 50 µs**, RMW `CLK_CONF` | W `0`; W `0x140`; W `0x80` | none |
| 14 | `rtc_clk_fast_src_set`, `rtc_clk_slow_src_set` | RMW `CLK_CONF` + **3 µs**; 2 RMW `CLK_CONF` + **300 µs** | W `0` | none |
| 15 | end of `bootloader_clock_configure` | W `INT_ENA_RTC = 0`, W `INT_CLR_RTC = 0xFFFFFFFF` | same | none |
| 16 | `bootloader_console_init` | R `STORE5`, R `STORE4`, RMW `UART0.CLKDIV` twice (`0x30015B` = 347 + 3/16), RMW `UART0.CLK_CONF` (clears `SCLK_DIV_NUM`) | R `0x4C4B4C4B`, `0x280028`; W `0x30015B`; `0x15122500` → `0x15100500` | none |

Each regi2c operation is done by ROM code (`esp_rom_regi2c_write` = `rom_i2c_writeReg@0x4000195c`, `…_write_mask` = `0x40001960`): RMW `ANA_CONFIG2 (0x6000E048)`, for reads W `ANA_CONFIG (0x6000E044)`, W command to `0x6000E000`, poll bit 25, read bits [23:16] (§2.2).

Application, 80 → 160 MHz (MEASURED insn 2,185,376–2,185,555; READ `IDF:esp_system/port/soc/esp32c3/clk.c:119-137`): R `SYSCLK_CONF`, R `CPU_PER_CONF` ×2, UART idle wait, `CPU_PER_CONF.CPUPERIOD_SEL` ← 1 (W `0x1`), `PRE_DIV_CNT` ← 0, `SOC_CLK_SEL` ← 1, W `STORE5 = 0x4C4B4C4B`, `ets_update_cpu_frequency(160)`, cycle count × 160/80, then UART0 `CLKDIV`/`CLK_CONF` rewritten in `system_early_init`. The PLL stays at 480 MHz (÷6 → ÷3), no regi2c.

Summary: the whole clock tree is **class a** (storage with correct reset values), plus the regi2c busy bit. It becomes class c only for an emulator that converts cycles to time: the cycle period changes at step 11 and again in the application. The firmware keeps the counter coherent by itself (steps 12 and the ×2 in the app); an emulator only has to keep counting cycles.

## 6. Watchdogs

What the bootloader does (MEASURED writes; READ `IDF:bootloader_support/src/bootloader_init.c:70-100`, `esp32c3/bootloader_esp32c3.c:79-84`, `IDF:hal/esp32c3/include/hal/rwdt_ll.h:118-119`):

| WDT | insn | Operations | State left when the application starts |
|---|---|---|---|
| Super WDT | 521,560–521,639 | `SWD_CONF`: reset-enable setup (`bootloader_ana_super_wdt_reset_config`); `SWD_WPROTECT = 0x8F1D312A`, set `SWD_CONF` bit 31 (`AUTO_FEED_EN`), `SWD_WPROTECT = 0` | Auto-fed by hardware: never expires. Left like this for good |
| RTC WDT | 573,561–573,790 | Unlock `WDTWPROTECT = 0x50D83AA1`; clear `FLASHBOOT_MOD_EN`; `wdt_hal_init` (disable, clear stages, CPU/SYS reset length 7, `PROCPU_RESET_EN`, `PAUSE_IN_SLP`); stage 0 = `RESET_RTC` (`WDTCONFIG0[30:28]` = 4); `WDTCONFIG1 = 0x956A0`; feed (`WDTFEED` bit 31); enable (`WDTCONFIG0` bit 31); lock | **Running.** One stage, whole-chip RTC reset after **9 s** (`CONFIG_BOOTLOADER_WDT_TIME_MS=9000`: 9000 × 136,000 Hz / 1000 = 1,224,000 slow-clock ticks, register = that `>> (1 + WDT_DELAY_SEL)` = 612,000 = `0x956A0`). Counted from this feed, not from the application start. `WDTCONFIG0` on hardware should then read about `0xC007EA00` (sum of the fields written; QEMU does not store it) |
| MWDT0 (TIMG0) | 573,804–573,830 | Unlock `0x50D83AA1`; clear `FLASHBOOT_MOD_EN` (bit 14), set `CONF_UPDATE_EN` (W `0x400000`); lock | **Stopped** (`EN` = 0, flash-boot protection off) |
| MWDT1 (TIMG1) | — | untouched | reset state, not running |

The bootloader never feeds a watchdog after that, and uses no watchdog interrupt.

What the application then does before and around the scheduler start (MEASURED; READ `IDF:esp_system/port/soc/esp32c3/clk.c:90-115`, `IDF:esp_system/int_wdt.c:36-37, 118-120`):

| insn | Function | Effect |
|---|---|---|
| 1,695,147 | `esp_clk_init` | RWDT fed, stage 0 shortened to 1.6 s (`WDTCONFIG1 = 0x1A900`) for the slow-clock selection |
| 2,185,310 | `esp_clk_init` | RWDT fed, stage 0 back to 9 s (`0x956A0`) |
| 2,342,799 | `__esp_system_init_fn_init_disable_rtc_wdt` | **RWDT disabled** (`CONFIG_BOOTLOADER_WDT_DISABLE_IN_USER_CODE` not set) |
| 2,342,906 | `esp_int_wdt_init` | MWDT1 = interrupt watchdog: prescaler 20000 (a 500 µs tick according to `int_wdt.c`), stage 0 interrupt after 10,000 ticks (5 s), stage 1 system reset after another 10,000; enabled. Interrupt source `TG1_WDT` → CPU line 24, priority 4 |
| 2,388,614 | `esp_task_wdt_init` (main task) | MWDT0 = task watchdog: stage 0 interrupt after 10,000 ticks (5 s), stage 1 system reset after a further 20,000 (10 s); source `TG_WDT` → CPU line 5 |
| 2,423,584 | first tick ISR | MWDT1 re-armed to its run-time values: stage 0 = 600 ticks (300 ms, `CONFIG_ESP_INT_WDT_TIMEOUT_MS`), stage 1 = a further 1,200 ticks (600 ms); then fed from every tick |

Decision for the plans:

- With watchdog registers as storage that never fire, nothing needs a watchdog: an application can run for any duration. The RWDT's 9 s budget covers the bootloader and the application's early init, which hardware finishes in well under a second.
- Watchdog **expiry** must not be modelled before the SYSTIMER tick interrupt works: MWDT1 is fed only from the tick ISR and MWDT0 only from tasks. Without ticks an expiring MWDT1 fires its interrupt after 5 s and resets the SoC 5 s later (0.3 s, then 0.6 s later, once it has been re-armed).
- The hardware flash-boot protection that arms RWDT and MWDT0 before the bootloader runs is still of unknown timeout (`esp32c3-boot-chain.md` §K.4); the bootloader's first RWDT/MWDT0 access is at insn 573,561, 52,050 instructions after its entry (QEMU count; SHA-free part of the boot, but it includes about 22,000 instructions of delay loops and the banner).

## 7. SHA

The bootloader verifies the appended SHA-256 of the application image with the **hardware SHA peripheral at `0x6003B000`, driven by ROM code** (READ `IDF:bootloader_support/src/bootloader_sha.c:24-43`: `ets_sha_enable(); ets_sha_init(&ctx, SHA2_256);` then `ets_sha_update(&ctx, data, len, false)`, then `ets_sha_finish(&ctx, digest)`). No software SHA is linked. The comparison itself is a `memcmp` against the 32 bytes read from flash (`esp_image_format.c:955`, `verify_simple_hash`). `CONFIG_BOOTLOADER_SKIP_VALIDATE_*` are off, so this happens on every boot. The partition table check uses MD5 in ROM software, no hardware.

Register protocol (READ `ROM3:ets_sha_enable@0x400494dc`, `ets_sha_process@0x400495be`; MEASURED counts):

| Step | Access | Detail |
|---|---|---|
| Enable (once) | RMW `SYSTEM.PERIP_CLK_EN1 0x600C0014` |= 4, RMW `SYSTEM.PERIP_RST_EN1 0x600C001C` (clear the SHA reset bits) | storage |
| Per 64-byte block, 1 | W `SHA.MODE 0x6003B000 = 2` | 2 = SHA-256; written before every block (2,128 times) |
| 2 | 16 word writes to `SHA.M_MEM 0x6003B080…0x6003B0BC` | ROM `memcpy`, always 4-byte stores (34,048 writes). Message bytes in memory order: the first word written is `0x4F0206E9`, i.e. bytes `E9 06 02 4F`, the start of the image header |
| 3 | R `SHA.BUSY 0x6003B018`, loop while it reads 1 | polled **before** the trigger: it waits for the previous block. 2,129 reads for 2,128 blocks, all 0 in QEMU |
| 4 | (not taken here) copy a saved state into `SHA.H_MEM` | only for a context restored from software state |
| 5 | first block: W `SHA.START 0x6003B010 = 1`; every other block: W `SHA.CONTINUE 0x6003B014 = 1` | 1 START, 2,127 CONTINUE. START = load the SHA-256 initial state then compress; CONTINUE = compress with the state left in `H_MEM` |
| Finish | ROM pads in software (one more block through the same steps), polls `BUSY`, then 8 word reads of `SHA.H_MEM 0x6003B040…0x6003B05C` | `H_MEM` holds the digest in byte order: the first word read is `0xF39825DA` and the digest stored in the image starts `da 25 98 f3` |

So `H_MEM` and `M_MEM` are byte-addressed big-endian views: each 32-bit SHA word is the byte-swap of the little-endian register word. Data hashed: the whole image except its last 32 bytes, 136,128 bytes = 2,127 blocks, plus one padding block. No DMA, no SHA interrupt. A model that runs the compression function on the `START`/`CONTINUE` write and always reads `BUSY` = 0 is sufficient. The application does not touch the SHA peripheral before `Hello world!` (MEASURED).

## 8. eFuse

Fields read (MEASURED; all are reads of the mirror registers, no `EFUSE.CMD` access):

| Register | Field | Reader | Phase |
|---|---|---|---|
| `RD_MAC_SPI_SYS_5 0x60008858` | `WAFER_VERSION_MAJOR` [25:24], `WAFER_VERSION_MINOR_HI` [23] | `efuse_hal_get_major/minor_chip_version` (5 times in the bootloader) | bootloader, app |
| `RD_MAC_SPI_SYS_3 0x60008850` | `WAFER_VERSION_MINOR_LO` [20:18], `BLK_VERSION_MINOR` [26:24] | same, `efuse_hal_blk_version` | bootloader, app |
| `RD_SYS_PART1_DATA4 0x6000886C` | `BLK_VERSION_MAJOR` [1:0] | `efuse_hal_blk_version`; app `rtc_init`, `set_ocode_by_efuse` | bootloader, app |
| `RD_MAC_SPI_SYS_1/2 0x60008848/4C` | SPI pad configuration | ROM `ets_efuse_get_spiconfig` ← `bootloader_configure_spi_pins`; app `esp_mspi_get_io` | bootloader, app |
| `RD_REPEAT_DATA1 0x60008834` | `WDT_DELAY_SEL` [17:16] | `wdt_hal_config_stage` (RWDT only) | bootloader, app |
| `RD_REPEAT_DATA3 0x6000883C` | `ERR_RST_ENABLE` [31] | app `esp_efuse_utility_check_errors` (insn 2,196,231) | app, after the first log line |
| `RD_REPEAT_DATA4 0x60008840` | `DISABLE_WAFER_VERSION_MAJOR` [0] | app `efuse_init` log lines | app, after the first log line |
| `RD_MAC_SPI_SYS_4/5` calibration fields | `DIG_DBIAS_HVT`, `K_RTC_LDO`, `K_DIG_LDO`, `V_RTC_DBIAS20`, `V_DIG_DBIAS20`, OCODE in `RD_SYS_PART1_DATA*` | app `rtc_init` (`get_dig_dbias_by_efuse` reads `SYS_5`) | app |

The bootloader built with this configuration reads **no** MAC, flash-encryption or secure-boot field: `SPI_BOOT_CRYPT_CNT`, `SECURE_BOOT_EN` and the download-mode flags are read by the ROM only (`RD_REPEAT_DATA0/2/3`, before the bootloader entry). The MAC is not read before `Hello world!`.

Values of a real production chip, revision v0.3 (READ: Espressif's own dump in `/opt/esp/idf/docs/en/api-reference/system/inc/espefuse_summary_ESP32-C3_dump.rst` and `espefuse_summary_ESP32-C3.rst`, Apache-2.0; one sample chip):

| Block | Registers | Raw words | Decoded |
|---|---|---|---|
| BLOCK0 | `RD_WR_DIS`, `RD_REPEAT_DATA0..4` (`0x2C..0x40`) | `00000000 00000000 00000000 00000000 80000000 00000000` | everything 0 except `ERR_RST_ENABLE` = 1: no secure boot, no flash encryption, download modes allowed, UART and USB print enabled, `WDT_DELAY_SEL` = 0, `FLASH_TPUW` = 0 |
| BLOCK1 | `RD_MAC_SPI_SYS_0..5` (`0x44..0x58`) | `790f968c 000058cf 00000000 020c0000 715424e0 0047d2f2` | MAC `58:cf:79:0f:96:8c`; SPI pads 0 (default); `WAFER_VERSION_MINOR_LO` = 3, `PKG_VERSION` = 0, `BLK_VERSION_MINOR` = 2; `WAFER_VERSION_MAJOR` = 0, `MINOR_HI` = 0 → **chip v0.3**; `K_RTC_LDO` −36, `K_DIG_LDO` −64, `V_RTC_DBIAS20` −40, `V_DIG_DBIAS20` −76, `DIG_DBIAS_HVT` −28, `THRES_HVT` 2000 |
| BLOCK2 | `RD_SYS_PART1_DATA0..7` (`0x5C..0x78`) | `96046025 6f41fdc3 512cedbe 217ee31d d864ea41 5aba3a86 1e260363 00000009` | unique id; `BLK_VERSION_MAJOR` = 1 → **eFuse block v1.2**; temperature and ADC calibration, `OCODE` = 78 |
| BLOCK3–10 | user data, keys | all 0 | no key burnt |
| Error registers | `RD_REPEAT_ERR0..4`, `RD_RS_ERR0/1` | all 0 | |

Compared with QEMU (v0.3, block v1.3, everything else 0): the real chip has a MAC, calibration data and `ERR_RST_ENABLE` set. With `ERR_RST_ENABLE` = 1 the application checks the error registers, which must read 0. Using this dump (with a locally administered MAC if preferred) is a sound default for inkemu; the eFuse content of a real Xteink X4 is still unknown.

## 9. Verdict

**The bootloader needs no interrupt, no timer and no watchdog expiry at all.** With flash content readable through the cache/MMU window, the SPI1 command path, the SHA peripheral, clock registers as storage and the read values of §2.2, it runs from its entry to the jump: `CPU_INT_ENABLE` is 0 for its whole duration, it touches no TIMG timer, no SYSTIMER, no RTC timer, and its only time source is the cycle-counter CSR (MEASURED). Last bootloader line: `I (18) boot: Disabling RNG early entropy source...`.

How far the application then gets depends on which read-value stubs exist, in this order:

| Model | Furthest log line | What stops it | Tag |
|---|---|---|---|
| Strictly no timer block (TIMG0 reads 0) | `I (18) cpu_start: Unicore app` (the first application line) | `rtc_clk_cal_internal` waits for `TIMG0.RTCCALICFG` bit 15 (`RTC_CALI_RDY`) or `RTCCALICFG2` bit 0 (timeout) forever; and a calibration result of 0 makes `select_rtc_slow_clk` retry forever (`do … while (cal_val == 0)`) | READ `IDF:esp_hw_support/port/esp32c3/rtc_time.c:69-76, 99-112`, `IDF:esp_system/port/soc/esp32c3/clk.c:150-195`; not run |
| + calibration answered as a register stub (`RDY` = 1, `RTCCALICFG1[31:7]` = 301,176 for 1,024 cycles of a 136 kHz clock against 40 MHz, i.e. register value `0x024C3C00`), `SYSTIMER.UNIT0_OP` bit 29 (value valid) reading 1 after bit 30 is written, `EXTMEM.ICACHE_FREEZE` bit 2 following bit 0, flash RDID through `esp_flash` | **`I (27) sleep_gpio: Enable automatic switching of GPIO sleep configuration`** | the scheduler start | MEASURED on QEMU with interrupt delivery suppressed; that plain stubs are enough in place of QEMU's timer models is ASSUMED (§10 item 6) |
| + delivery of one software interrupt | `Hello world!`, the two lines after it and `Restarting in 10 seconds...` | `vTaskDelay`: the SYSTIMER tick alarm | MEASURED (exactly one interrupt is taken before these lines) |

**First thing that truly requires interrupts:** `xPortStartScheduler` (`IDF:freertos/FreeRTOS-Kernel/portable/riscv/port.c:130`) dispatches the first task by calling `vPortYield`, which writes 1 to `SYSTEM.CPU_INTR_FROM_CPU_0` (`0x600C0028`, insn 2,369,224). That is interrupt source 50, routed by `CPU_INTR_FROM_CPU_0_MAP = 3` to CPU line 3 (priority 1, threshold 1, `CPU_INT_ENABLE` bit 3, `mstatus.MIE` set). The handler reads `CPU_INT_THRESH` and `CPU_INT_PRI_3`, raises the threshold, `esp_crosscore_isr` writes 0 to `CPU_INTR_FROM_CPU_0`, and the exit path switches to the main task.

Without delivery (MEASURED, gdbstub stepping with interrupts suppressed from `xPortStartScheduler`): `vPortYield` returns at once because `port_xSchedulerRunning` is still 0 (it is set by the ISR path, `port.c:649, 657`), `vTaskStartScheduler` and `esp_startup_start_app` return, and the CPU ends in the `j .` at `0x420005ec` (`ESP_INFINITE_LOOP()` in `start_cpu0_default`, `IDF:esp_system/startup.c:222-224`) 9,727 instructions later. No output after the `sleep_gpio` line, no panic: a silent hang.

**First thing that requires a timer:** the periodic SYSTIMER alarm (counter 1, alarm 0, source 37 → CPU line 4), programmed in `vPortSetupTimer` just before the scheduler starts and first needed by `vTaskDelay` in `app_main`.

**Watchdog expiry** is required by nothing on this path (§6).

What the application touches between its first log line and the scheduler start, for the boundary of the next plans (MEASURED):

| insn | Function | Registers | Class |
|---|---|---|---|
| 1,691,647 | `esp_clk_init` → `rtc_clk_8m_enable`, `rtc_clk_fast_src_set`, `rtc_clk_slow_src_set` | `RTC_CNTL.CLK_CONF`, `TIMER1`, RWDT feed/config | a |
| 1,714,191 | `rtc_clk_cal_internal` | `TIMG0.RTCCALICFG` (reset `0x13000`), `RTCCALICFG2`, `RTCCALICFG1`; 7,529 µs `ets_delay_us` | **b** |
| 2,185,038 | `esp_clk_slowclk_cal_set`, `rtc_time_get` | `RTC_CNTL.STORE1` (W `0x3AD2CD`), `TIME_UPDATE` (W bit 31), `TIME_LOW0`, `TIME_HIGH0` (read 0 in QEMU) | a |
| 2,185,376 | 80 → 160 MHz | §5 | a |
| 2,185,657 | `esp_perip_clk_init` | `SYSTEM.PERIP_CLK_EN0/1`, `PERIP_RST_EN0/1`, `APB_CTRL.WIFI_CLK_EN`, `WIFI_RST_EN`, `SYSTEM.BT_LPCK_DIV_*` | a |
| 2,186,120 | `core_intr_matrix_clear` | 62 `INTERRUPT_CORE0.*_MAP` ← 0 | a→d |
| 2,187,117 | `system_early_init` | UART0 `CLKDIV`, `CLK_CONF`, console | a, c |
| 2,194,350 | cache-error interrupt, memory protection | `INTERRUPT_CORE0` map/priority/type/enable (lines 25, 26), `EXTMEM.*_INT_ENA/CLR`, `SENSITIVE.*` registers (72 accesses) | a→d |
| 2,196,231 | `esp_efuse_utility_check_errors` | `EFUSE.RD_REPEAT_DATA3` | b |
| 2,292,318 | `esp_timer_impl_early_init`, `systimer_hal_get_counter_value` (several calls) | `SYSTIMER.CONF` (reset `0x46000000`), `TARGET2_CONF`, `UNIT0_OP` (W bit 30, poll bit 29), `UNIT0_VALUE_LO/HI` | **b** (value valid); a counter value is only read, not waited on |
| 2,294,174 | `esp_brownout_init` | `RTC_CNTL.BROWN_OUT`, `FIB_SEL`, regi2c, `INT_ENA_RTC`, `RTC_CORE_INTR_MAP = 2` | a→d |
| 2,307,642 | `esp_flash_init_default_chip` | SPI1 configuration, two RDID (`0x9F`) user commands, `SPI1.FLASH_SUS_CTRL`, `FLASH_WAITI_CTRL`, `SPI0.CORE_CLK_SEL`; `EXTMEM.ICACHE_FREEZE` (W bit 0 → bit 2 must read 1; W 0 → 0) around each flash operation | c, **b** |
| 2,319,492 | hardware stack guard | `ASSIST_DEBUG.CORE_0_INTR_ENA`, later `CORE_0_MONTR_ENA`, `CORE_0_SP_MIN/MAX` | a→d |
| 2,326,387 | `sleep_gpio` | `IO_MUX` pad registers (220 accesses, read-modify-write) | a |
| 2,342,799 | watchdogs, cross-core interrupt, tick timer | §6; `CPU_INTR_FROM_CPU_0_MAP = 3`; `SYSTIMER_TARGET0_INT_MAP = 4`, `SYSTIMER.UNIT1_LOAD*`, `TARGET0_CONF`, `COMP0_LOAD`, `INT_ENA` | a→d |

## 10. Open questions, ranked by risk

1. **CPU frequency during the ROM and `freq_before`.** The IDF header gives `PRE_DIV_CNT` = 1 at reset and `clk_ll_cpu_get_divider()` returns `PRE_DIV_CNT + 1` = 2, so `rtc_clk_init` computes 20 MHz and multiplies the cycle count by 4. The ROM's `ets_get_xtal_div@0x4004981c` returns 1 unless `CLK_DIV_EN` (bit 19) is set, so the ROM believes 40 MHz, and an IDF comment says "ROM bootloader is running at 40 MHz". Which one silicon does is not established. Effect: bootloader log timestamps (×2), and whether ROM delays last twice their nominal time. No hang either way. inkemu currently resets `SYSCLK_CONF` to `PRE_DIV_CNT` = 1 (`crates/soc-esp32c3/src/periph/system.rs`), so the bootloader will take the 20 MHz branch.
2. **The hardware-only PLL path (§5 steps 9–10) was read, not run.** QEMU cannot exercise it. The risk is a missed dependency in the nine regi2c operations on the BBPLL block; the ROM routines themselves were exercised on the SAR block.
3. **Flash vendor of the real device.** RDID decides the unlock branch (RDSR only, RDSR + RDSR2, or 16-bit status), the XMC start-up flow, and whether WREN/WRSR are issued. Only the ISSI branch was measured.
4. **eFuse of a real Xteink X4.** `BLK_VERSION_MAJOR` must be 1 for the application's `rtc_init` to avoid the calibration loop or the assert; the note about CrossPoint wrapping `bootloader_common_check_efuse_blk_validity` suggests the real value is unusual. §8 gives a real v0.3 chip, not that device.
5. **Lockstep against QEMU breaks at the RNG.** QEMU's RNG is not reproducible between runs, its values enter registers and RAM at insn ≈ 635,927 (`ram_obfs_value`), and the number of reads depends on the cycle-counter model. The pc sequence stays identical; register and memory comparison needs a mask, or the RNG values must be injected.
6. **SYSTIMER before the scheduler.** Whether a constant counter value is enough between `esp_timer_impl_early_init` and the scheduler start was not tested (only the value-valid bit is waited on).
7. **Console duplication on USB-Serial-JTAG** if `EP1_CONF` bit 1 reads 1: every ROM-printf character of the bootloader and early application goes to both channels. QEMU cannot confirm the byte stream.
8. **Hardware flash-boot watchdog timeout** (unchanged from `esp32c3-boot-chain.md` §K.4). Irrelevant while watchdogs never fire.
9. **Exactly one interrupt before `Hello world!`** was measured on QEMU 20250817; the 20260417 build changed interrupt latency when `MIE` = 0 and was not re-measured.
10. **The Xteink bootloader is not this one** (18,672 bytes, CrossPoint patches, possibly another log level and console). The register list should carry over, the instruction counts and line texts will not.

## 11. Corrections to earlier reports

- `esp32c3-boot-chain.md` §H: in `RD_REPEAT_DATA1` (`0x34`), ESP-IDF v5.5.2 places `WDT_DELAY_SEL` at bits [17:16] and `SPI_BOOT_CRYPT_CNT` at [20:18], bits [15:0] being reserved (READ `IDF:soc/esp32c3/register/soc/efuse_reg.h:312-335`); the report says [1:0] and [4:2]. In `RD_REPEAT_DATA3`, `ERR_RST_ENABLE` is bit 31, not 30 (`efuse_reg.h:546`).
- `esp32c3-boot-chain.md` §K.7 (regi2c busy-wait "inferred from QEMU") is settled by the ROM disassembly: bit 25 of the command register, §2.2.
- `esp32c3-boot-chain.md` §E.3 says the bootloader "optionally starts RWDT": in the default configuration it does, with 9 s, and leaves it running into the application (§6).
- `sp1b-test-oracle.md` §1c: MMIO tracing is possible without plugins (`-trace 'memory_region_ops_*'`), and QEMU's RNG makes register-level lockstep diverge inside the bootloader (§10 item 5).

## Files (scratch, not in the repository)

`<scratch>/btlacc/tools/`: `filter.py` (reduces the QEMU stream to events + pc list), `analyze.py` (per-phase first-access and full-access tables, UART line index), `groups.py`, `mktable.py`, `romcalls.py` (ROM functions entered, `ets_delay_us` accounting), `nointr.py` (gdbstub experiment).
`<scratch>/btlacc/out/`: `t1_events.txt` (100,782 events), `t1_pcs.bin` (3,000,001 pcs), `a_first_{rom,btl,app1,app2}.tsv`, `a_all_*.tsv`, `a_uart_lines.txt`, `a_other_events.txt` (interrupts, QEMU warnings), `dis_{btl,app,rom}.txt`, `nm_*.txt`, `nointr_pcs.txt`, `nointr_uart.log`.

## Corrections (2026-10-11)

Established after this report was written, by `esp32c3-interrupts-and-reset.md` (§9) and `esp32c3-timers-and-watchdogs.md` (§K). The text above is left as it was; where it disagrees with an entry below, the entry is right.

- BA §1 and §9 give insn 2,369,140 for `xPortStartScheduler`: that is the threshold write inside it; the function is entered at 2,359,524.
- BA §10 item 9: the 20260417 build was measured here; it is identical to 20250817 for the first 2,472,463 instructions (§6 row 21).

| Place | Statement there | Established here |
|---|---|---|
| `esp32c3-boot-chain.md` §E.3, §K.4; `esp32c3-rom-flash-boot.md` §L.2; `esp32c3-bootloader-accesses.md` §10.8 | "Exact hardware timeout/stage action of the auto-armed WDT: unknown" | MWDT0: 26,000,000 APB cycles, core reset, cause `0x07`; RWDT: 400,000 slow-clock cycles, system reset, cause `0x10`; proven absent from the ROM; both seen on real chips (§D.4, §H) |
| `esp32c3-bootloader-accesses.md` §10.1 | CPU frequency during the ROM (20 or 40 MHz) "not established" | `TRM:` v1.4 §6.2.4.1: 20 MHz by default; a reset-loop log excludes 40 MHz for APB (§H.2). After an `esp_restart` the ROM runs at 40 MHz (`PRE_DIV_CNT` = 0 survives the CPU reset) |
| `esp32c3-bootloader-accesses.md` §6 | interrupt watchdog "prescaler 20000 (a 500 µs tick according to `int_wdt.c`)" | 500 µs because ESP-IDF clocks the MWDT from the 40 MHz crystal (`USE_XTAL` = 1), not from APB |
| `esp32c3-bootloader-accesses.md` §9, row 2 | a calibration stub with `RDY` = 1 and `SYSTIMER.UNIT0_OP` valid is enough up to the scheduler | still true; add that the tick ISR needs counter 1 to advance by one period per interrupt (§A.4), and that QEMU's own counter 1 starts 1.93 periods ahead (§G.9) |
