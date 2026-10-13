# USB-Serial-JTAG of the ESP32-C3, both directions, and what makes firmware believe a host is attached

Scope: the USB-Serial-JTAG controller (USJ) of the ESP32-C3 as its consumers see it — the register map and its reset values; what the mask ROM, ESP-IDF v5.5.2 (console paths, the `usb_serial_jtag` driver, its connection monitor, the panic handler) and Arduino-ESP32's `HWCDC` read and write; the interrupt source; what a USB host attached and reading, attached with the port closed, absent, detaching and re-attaching looks like at register level and in device time; oracles; reference firmwares for CI; what CrossPoint needs from USJ for its own log to appear.
Date of research: 2026-10-13. No code was changed and nothing was built or run; this file is the only repository edit. UART reception is covered by another report.
Extends `esp32c3-boot-chain.md` (**BC**) §G.3, `esp32c3-bootloader-accesses.md` (**BA**), `flash-writes-and-crosspoint-boot.md` (**FW**) §K and open question #4, `esp-idf-storage-flash-traffic.md` (**NV**), `esp32c3-interrupts-and-reset.md` (**IR**), ADR 0003 and ADR 0004.

## 0. Method, sources, tags

| Tag | Meaning |
|---|---|
| **M** (MEASURED) | seen in an artefact produced by an earlier build or run: the `sdkconfig` and the linker map of the reference `hello_world` built for `sp1b-test-oracle.md` (scratch `oracle/hello/`, `sdkconfig.defaults` byte-identical to `tools/reference-firmware/sdkconfig.defaults`), or a measurement quoted from BA or FW with its location |
| **R** (READ) | read in a source file (`file:line`) or in a disassembly (`ROM:address`) |
| **A** (ASSUMED) | inferred; every A item that carries risk is repeated in §O |

| Prefix | Source | Where |
|---|---|---|
| `IDF:` | ESP-IDF v5.5.2 subset, paths relative to `components/` | scratch `r3d/idf/components/` |
| `IDFEX:` | ESP-IDF v5.5.2 examples subset, relative to `examples/` | scratch `r3d/idf/examples/` |
| `IDFGH:` | ESP-IDF files fetched for this report from `raw.githubusercontent.com/espressif/esp-idf`, paths relative to `components/`: at tag `v5.5.2` `bootloader_support/src/bootloader_console.c`, `freertos/port_systick.c`, `freertos/config/include/freertos/FreeRTOSConfig.h`, `esp_system/freertos_hooks.c`; at tag `v5.5.5` `esp_driver_usb_serial_jtag/src/usb_serial_jtag_connection_monitor.c` (byte-identical to v5.5.2) | scratch `usj-dl/` |
| `ROM:` | ESP32-C3 rev 3 mask ROM, `rom.dis` / `rom.nm` (addresses) | scratch `r3d/rom/` |
| `ARD:` | arduino-esp32 tag `3.3.11` (commit `189089bb76e7`), `cores/esp32/HWCDC.cpp` (743 lines) and `HWCDC.h` (117 lines) | scratch `usj-dl/` |
| `LB:` | esp32-arduino-lib-builder `master`, `configs/defconfig.common` (the `release/v5.5` branch name returned 404) | scratch `usj-dl/` |
| `CP:` | CrossPoint reader, commit `e54c00873189` (2026-10-11, `version = 1.6.5`), relative to the repository root | scratch `repos/crosspoint-reader_crosspoint-reader/` |
| `FSDK:` | freeink-sdk at the submodule commit CrossPoint pins (`32a265fa8649`), `libs/hardware/BoardConfig/include/BoardConfig.h` (the submodule is empty in the local clone) | scratch `usj-dl/BoardConfig.h` |
| `MAP:` | `oracle/hello/build/hello_world.map` and `oracle/hello/sdkconfig` (reference `hello_world`, ESP-IDF 5.5.2) | scratch |
| `INK:` | inkemu at `main` (`0f866ed`), relative to `crates/` | repository |

The QEMU stub was not re-read; BC §G.3 and FW §K.4 are quoted. No real ESP32-C3 was available.

## 1. Key results

1. **The register block is at `0x6004_3000`, interrupt source 26 (`INTERRUPT_CORE0_USB_INTR_MAP` at `0x600C_2068`), level** **(R)**. The firmware-visible data path is `EP1` (byte FIFO, both directions), `EP1_CONF` (`WR_DONE`, `SERIAL_IN_EP_DATA_FREE`, `SERIAL_OUT_EP_DATA_AVAIL`) and `INT_RAW/ST/ENA/CLR`; every consumer found uses only those, `CONF0`, and the `SOF` bit. Nobody on the paths read here touches `FRAM_NUM`, the `IN_EPn_ST`/`OUT_EPn_ST` state registers, `JFIFO_ST`, `TEST`, `MISC_CONF`, `MEM_CONF` or `DATE`. **There is no `CHIP_RST` register and no DTR/RTS line-state register on the ESP32-C3**: the firmware cannot see whether the host opened the port **(R)**.
2. **The firmware's only sign of an attached host is the SOF bit (`INT_RAW` bit 1), which ESP-IDF's connection monitor polls from the FreeRTOS tick and clears** (`IDF:esp_driver_usb_serial_jtag/src/usb_serial_jtag_connection_monitor.c:37-72`). `usb_serial_jtag_is_connected()` starts true and turns false after `pdMS_TO_TICKS(3) + 1` ticks without a SOF: **one tick (10 ms) at the reference `CONFIG_FREERTOS_HZ=100`, four ticks (4 ms) at Arduino's 1000 Hz** **(R)**. The SOF *interrupt* is never enabled by anyone (Arduino commented it out: `ARD:HWCDC.cpp:251-255, 458-461`) **(R)**.
3. **The only sign that a host is *reading* is the IN endpoint being drained**: after `WR_DONE` (or a 64th byte), `SERIAL_IN_EP_DATA_FREE` stays 0 until the host has taken the packet; then `SERIAL_IN_EMPTY` (`INT_RAW` bit 3, reset value 1) is set **(R: field comments, `IDF:soc/esp32c3/register/soc/usb_serial_jtag_struct.h:24-25, 36`)**. Attached-but-port-closed = SOF present, IN never drained **(A for the host side, §O.3)**.
4. **The ROM writes every console character to USJ as well as UART0, on both boot paths, through `usb_uart_tx_one_char` (`ROM:0x4004c8e2`)**. It drops a character silently when the endpoint is full and it has not yet seen a free endpoint; once it has, it waits up to **5,000** iterations of a 1 µs delay before giving up (**BC §G.3 says 904: the immediate of `addi` only; `lui s0,0x1` adds 4,096**) **(R)**. A normal SPI boot never reads `EP1`: ROM reception from USJ (`usb_uart_rx_one_char`, `ROM:0x4004c976`) is reached only through `uart_rx_one_char` when the ROM console buffer is channel 3, i.e. in download mode or after an ESP-IDF bootloader built with the USJ console **(R)**.
5. **The reference `hello_world` and `nvs_rw_value_cxx` already put their whole console on USJ, through the ROM, not through ESP-IDF's "secondary console"**: they are `MINIMAL_BUILD`s without VFS, so `printf` goes `_write_r_console` → `esp_system_console_put_char` → ROM `uart_tx_one_char`, which mirrors to USJ because ESP32-C3 has `ESP_ROM_CONSOLE_OUTPUT_SECONDARY` and the bootloader sets `g_usb_print = true` **(R + M: the map has no object of `esp_driver_usb_serial_jtag` and discards `esp_register_freertos_tick_hook`)**. `CONFIG_ESP_CONSOLE_SECONDARY_USB_SERIAL_JTAG=y` is the default on ESP32-C3 and is set in the reference `sdkconfig` **(R + M)**, but in these two builds it changes nothing. Nothing in them reads `INT_RAW`, so the SOF question does not arise for them.
6. **A full (non-minimal) ESP-IDF build gates its USJ VFS output on `usb_serial_jtag_is_connected()`**: without SOF, `write()` returns -1 and the bytes never reach the FIFO (`IDF:esp_driver_usb_serial_jtag/src/usb_serial_jtag_vfs.c:183-188`). With SOF but no reader, each character waits until 50 ms have passed since the last successful one, then everything is dropped until the endpoint frees (`:72-81, 148-171`) **(R)**.
7. **Arduino `HWCDC` (CrossPoint's `Serial`, core 3.3.11)**: `isPlugged()` *is* `usb_serial_jtag_is_connected()` (`ARD:HWCDC.h:73-77`); `connected` is set by the ISR on `SERIAL_IN_EMPTY` or `SERIAL_OUT_RECV_PKT` and cleared on `USB_BUS_RESET` or when `isPlugged()` is false (`ARD:HWCDC.cpp:137-291`). `begin()` writes `CONF0` and enables exactly `SERIAL_IN_EMPTY | SERIAL_OUT_RECV_PKT | USB_BUS_RESET` (`:449-457`) **(R)**.
8. **Bytes written while not connected are not dropped: they go into a 256-byte TX ring with a drop-oldest policy** (`flushTXBuffer`, `ARD:HWCDC.cpp:293-343`), and the ISR keeps draining that ring into the FIFO whenever `SERIAL_IN_EMPTY` fires. A host that starts reading after boot receives the last ≤256 (+≤64 in the FIFO, +≤64 stashed) bytes, then the live log **(R)**.
9. **With the host attached and the port closed, CrossPoint stalls about 20 ms per log line**: the "connected" path blocks `xRingbufferSend` for `tx_timeout_ms` (CrossPoint sets 1 ms, `CP:src/main.cpp:441`) up to 20 times before returning a short write (`ARD:HWCDC.cpp:581-604`) **(R)**.
10. **For CrossPoint's log to appear, the emulator must offer**: `CONF0`/`INT_ENA` storage, `INT_ST = INT_RAW & INT_ENA` driving source 26, `INT_CLR`, `SERIAL_IN_EMPTY` set at reset and after each drained IN packet, a host that drains committed packets, and a SOF bit set every millisecond (§G). Draining alone delivers the log through the drop-oldest path; SOF alone delivers nothing **(R)**. Under QEMU and at inkemu `main` neither holds (`INT_RAW` reads 0 or is unmodelled), which explains why FW §K saw no CrossPoint line on USJ **(R + FW §K.4)**.
11. **FW §K.3 already measured the monitor**: the `USB_DEVICE` `INT_RAW`, `INT_CLR` accesses from `+588,725` and the 10,744 `USB_DEVICE` reads of the application phase are the 1 kHz tick hook reading and clearing SOF; `CONF0`, `INT_ENA` then `EP1_CONF` at `+691,716` is `HWCDC::begin` (§G.2) **(R, matching M of FW)**.
12. **No executable oracle exists** for the host side (QEMU's model is a stub that reads 0; no other open emulator models USJ). Validation rests on the register contract of the headers (already in `svd-regs`), driver behaviour read from source, and reference firmwares whose output is fixed: the ROM banner on USJ (an existing test), the reference `hello_world` (USJ stream = UART0 stream, §F.3), and the stock `usb_serial_jtag_echo` example (§F.1) **(R)**.
13. **Recommended host model** (§R): one input of the run, independent of `--console` (ADR 0004 point 1): `absent | attached (port closed) | open` with **`open` as the default**, which is today's behaviour and keeps the pinned USB banner test true; SOF computed lazily from device time (1 ms, `FRAM_NUM` modulo 2048); a committed IN packet drained a fixed device-time latency after commit; host-to-device bytes as dated inputs split into ≤64-byte OUT packets, one packet in the FIFO at a time.

---

## A. Register map (R: `IDF:soc/esp32c3/register/soc/usb_serial_jtag_reg.h`, `usb_serial_jtag_struct.h`; base `IDF:soc/esp32c3/register/soc/reg_base.h:46`)

Reset values are the header's defaults; `INK:svd-regs/src/esp32c3.rs` carries the same ones (`EP1_CONF 0x2`, `INT_RAW 0x8`, `CONF0 0x4200`, `JFIFO_ST 0x44`, `IN_EPn_ST 0x1`, `MEM_CONF 0x2`, `DATE 0x0200_7300`).

| Off | Register | Reset | Fields (bit) | Read by | Written by |
|---|---|---|---|---|---|
| `0x00` | `EP1` | 0 | `RDWR_BYTE` [7:0]: write = push a byte into the IN FIFO; read = pop a byte from the OUT FIFO. Struct comment: "When `SERIAL_IN_EMPTY_INT` is set then user can write data (up to 64 bytes)…; when `SERIAL_OUT_RECV_PKT_INT` is set, user can check `OUT_EP1_WR_ADDR` and `OUT_EP0_RD_ADDR` to know how many data is received" (`struct.h:18`) | ROM rx (`0x4004c96e`, `0x4004c980`); `usb_serial_jtag_ll_read_rxfifo` (`IDF:hal/esp32c3/include/hal/usb_serial_jtag_ll.h:115-123`): driver ISR, VFS no-driver rx, HWCDC ISR | ROM tx (`0x4004c8fc`, `0x4004c944`); `usb_serial_jtag_ll_write_txfifo` (`ll.h:134-142`): VFS tx, driver ISR, panic, HWCDC ISR |
| `0x04` | `EP1_CONF` | `0x2` | b0 `WR_DONE` (WT): "Set this bit to indicate writing byte data to UART Tx FIFO is done. This bit then stays 0 until data in UART Tx FIFO is read by the USB Host" (`struct.h:24`); b1 `SERIAL_IN_EP_DATA_FREE` (RO, 1): "Tx FIFO is not full and data can be written … After writing `WR_DONE`, this will be 1'b0 until the data is sent to the USB Host" (`:25`); b2 `SERIAL_OUT_EP_DATA_AVAIL` (RO, 0): "there is data in UART Rx FIFO" (`:26`). The FIFO commits itself at 64 bytes (`ll.h:168-175`) | everyone, per byte | `WR_DONE`: ROM per `\n`, VFS per `\n` and in `fsync`, driver ISR after each fill and once more to send a zero-length packet, HWCDC (ISR, `isCDC_Connected`, `flushTXBuffer`, `cdc0_write_char`, `flush`) |
| `0x08` | `INT_RAW` | `0x8` | b0 `JTAG_IN_FLUSH`, **b1 `SOF`** ("when a SOF frame is received"), **b2 `SERIAL_OUT_RECV_PKT`** ("the Serial Port OUT Endpoint received one packet"), **b3 `SERIAL_IN_EMPTY`** ("the Serial Port IN Endpoint is empty", reset **1**), b4 `PID_ERR`, b5 `CRC5_ERR`, b6 `CRC16_ERR`, b7 `STUFF_ERR`, b8 `IN_TOKEN_REC_IN_EP1`, **b9 `USB_BUS_RESET`**, b10 `OUT_EP1_ZERO_PAYLOAD`, b11 `OUT_EP2_ZERO_PAYLOAD`; access R/WTC/SS (`reg.h:554-660`, `struct.h:33-45`) | connection monitor (bit 1, every tick) | — (cleared through `INT_CLR`) |
| `0x0C` | `INT_ST` | 0 | `INT_RAW & INT_ENA`, same bits | driver ISR, HWCDC ISR (`usb_serial_jtag_ll_get_intsts_mask`, `ll.h:70-73`) | — |
| `0x10` | `INT_ENA` | 0 | same bits | `ll.h:102-105` (unused by the consumers read) | read-modify-write: driver (bit 2 at install, bit 3 per write, cleared in the ISR), HWCDC (all cleared, then bits 2, 3, 9 at `begin`; bit 3 armed and disarmed constantly) |
| `0x14` | `INT_CLR` | 0 | same bits, write 1 to clear | — | monitor (bit 1 per tick), driver ISR (bits 2, 3), HWCDC ISR (bits 2, 3, 9), `HWCDC::end` (all 19 bits of `USB_SERIAL_JTAG_LL_INTR_MASK = 0x7ffff`) |
| `0x18` | `CONF0` | `0x4200` | b0 `PHY_SEL`, b1 `EXCHG_PINS_OVERRIDE`, b2 `EXCHG_PINS`, `VREFH`/`VREFL` (2+2 bits; **`reg.h` puts `VREFL` at bit 3 and `VREFH` at bit 5, the struct the other way round**), b7 `VREF_OVERRIDE`, b8 `PAD_PULL_OVERRIDE`, b9 `DP_PULLUP` (1), b10 `DP_PULLDOWN`, b11 `DM_PULLUP`, b12 `DM_PULLDOWN`, b13 `PULLUP_VALUE`, b14 `USB_PAD_ENABLE` (1) | — | `usb_serial_jtag_ll_phy_set_defaults` (`PHY_SEL=0`, `USB_PAD_ENABLE=1`, `ll.h:194-199`): driver install (`IDF:esp_driver_usb_serial_jtag/src/usb_serial_jtag.c:216`), `usb_serial_jtag_hal_init`; HWCDC `begin` (`PHY_SEL=0`, `PAD_PULL_OVERRIDE=0`, `DP_PULLUP=1`, `USB_PAD_ENABLE=1`, `ARD:HWCDC.cpp:449-455`); startup when USJ is disabled in Kconfig (`USB_PAD_ENABLE=0`, `IDF:esp_system/port/soc/esp32c3/clk.c:260-267`) |
| `0x1C` | `TEST` | 0 | pad test drive (`TEST_ENABLE`, `TEST_USB_OE`, `TEST_TX_DP`, `TEST_TX_DM`) | — | — |
| `0x20` | `JFIFO_ST` | `0x44` | JTAG FIFO counters, empty/full, resets (`reg.h:252-320`) | — | — |
| `0x24` | `FRAM_NUM` | 0 | `SOF_FRAME_INDEX` [10:0] ("Frame index of received SOF frame") | — (no `ll` accessor on ESP32-C3) | — |
| `0x28`–`0x34` | `IN_EP0_ST`…`IN_EP3_ST` | `0x1` | `STATE` [1:0], `WR_ADDR` [8:2], `RD_ADDR` [15:9] | — | — |
| `0x38`–`0x40` | `OUT_EP0_ST`…`OUT_EP2_ST` | 0 | `STATE`, `WR_ADDR` ("there are `OUT_EP1_WR_ADDR`-2 bytes data in OUT EP1" after `SERIAL_OUT_RECV_PKT`), `RD_ADDR`; EP1 adds `REC_DATA_CNT` [22:16] | — | — |
| `0x44` | `MISC_CONF` | 0 | b0 `CLK_EN` (force register clock) | — | — |
| `0x48` | `MEM_CONF` | `0x2` | b0 `USB_MEM_PD`, b1 `USB_MEM_CLK_EN` | — | — |
| `0x80` | `DATE` | `0x0200_7300` | version | — | — |

Outside the block: `SYSTEM.PERIP_CLK_EN0.USB_DEVICE_CLK_EN` and `PERIP_RST_EN0.USB_DEVICE_RST` (`ll.h:296-311`); ESP-IDF's startup leaves the USB device out of the peripheral reset it performs, "for non-CPU-reset cases" (`IDF:esp_system/port/soc/esp32c3/clk.c:289`). IR §4 lists USJ among the registers a core reset keeps; reset reasons `0x15` (USB-UART) and `0x16` (USB-JTAG) are host-initiated chip resets **(R, IR)**.

Not on ESP32-C3 (asked for): `CHIP_RST`, a serial line-state (DTR/RTS) register, a SOF-frame accessor in the LL header **(R: absent from `reg.h`, `struct.h`, `ll.h`)**.

`USB_SERIAL_JTAG_LL_INTR_MASK = 0x7ffff` (19 bits, `ll.h:19`) is wider than the 12 bits the header documents; the comment says "the hardware has more interrupts, but they're only useful for debugging" (`ll.h:22-24`) **(R)**.

---

## B. Consumers and their exact accesses

### B.1 Mask ROM (R, rev 3 ROM; addresses `ROM:`)

The ROM touches the block only in four functions (every reference to `0x60043000` in `rom.dis`): `usb_uart_tx_one_char` `0x4004c8e2`, `usb_uart_tx_flush` `0x4004c95a`, `usb_uart_rx_one_char_block` `0x4004c964`, `usb_uart_rx_one_char` `0x4004c976`.

| Function | Accesses | Logic |
|---|---|---|
| `usb_uart_tx_one_char(c)` | R `EP1_CONF`, W `EP1` | RAM flag `usb_uart_connected` (`0x3fcdf060`). Flag 0: if `EP1_CONF` bit 1 → set flag, write `c`; else **drop `c`, return at once**. Flag 1: loop: if bit 1 → write `c`; else call `*ets_ops_table_ptr` (`0x3fcdfffc`) with `a0 = 1` (a 1 µs delay, **A**) and retry; after **5,000** failed polls (`lui s0,0x1; addi s0,s0,904` = `0x1388`) clear the flag and drop `c` |
| `usb_uart_tx_flush` | W `EP1_CONF = 1` | `WR_DONE` |
| `uart_tx_one_char(c)` `0x4004c3ec` (= `esp_rom_output_tx_one_char`) | — | if `g_usb_print` (`0x3fcdffd0`): `usb_uart_tx_one_char(c)`, and `usb_uart_tx_flush` when `c == '\n'`; if the printf channel is 3, stop there; else if `g_uart_print` (`0x3fcdffd1`), send `c` to the UART of the channel |
| `uart_usb_print_status_update` `0x4004c39c` (from `boot_prepare` `0x40047ccc`) | R `GPIO.STRAP`, R `RTC_CNTL.STORE4` | strap bit 3 clear (download boot): `g_uart_print = 1`, `g_usb_print = !ets_efuse_usb_download_mode_disabled()`; strap bit 3 set (SPI boot): nothing if `STORE4` bit 0 (ROM log disabled), else `g_uart_print = ets_is_print_boot()`, `g_usb_print = !ets_efuse_usb_print_is_disabled()` |
| `usb_uart_rx_one_char(&c)` | R `EP1_CONF`, R `EP1` | bit 2 set → pop one byte, return 0; else return 1 |
| `usb_uart_rx_one_char_block` | same | spin on bit 2 |
| `uart_rx_one_char` `0x4004c50e`, `uart_rx_one_char_block` `0x4004c4d8` | — | go to the USB functions only when `UartDev.buff_uart_no` (`0x3fcdf07c`) is 3 |

So on a normal SPI boot with blank eFuses: **every ROM-printf character goes to USJ, `EP1` is never read**, and `EP1_CONF` is read once per character and written once per line. BA measured exactly that under QEMU: 1,287 `EP1_CONF` reads and 21 writes for the bootloader's 1,287 characters (`BA:95`), and the same from the early application (`BA:279`) **(M, BA)**.
Consequences: a host that never drains costs the ROM one ~5 ms wait (the first character after the first committed packet), then silence until the endpoint frees; a host that drains within a few µs costs nothing. QEMU's stub (bit 1 reads 0) makes the ROM drop everything without waiting **(R)**.

### B.2 ESP-IDF v5.5.2 console on ESP32-C3

**Kconfig defaults (R, M)**: primary console `ESP_CONSOLE_UART_DEFAULT`; secondary console choice `default ESP_CONSOLE_SECONDARY_USB_SERIAL_JTAG` (`IDF:esp_system/Kconfig:296-315`), which selects `ESP_CONSOLE_USB_SERIAL_JTAG_ENABLED` and `USJ_ENABLE_USB_SERIAL_JTAG` (`:318-322`; `IDF:esp_driver_usb_serial_jtag/Kconfig:4-6`, default y). The reference `sdkconfig` has `CONFIG_ESP_CONSOLE_SECONDARY_USB_SERIAL_JTAG=y`, `CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG_ENABLED=y`, `CONFIG_ESP_ROM_CONSOLE_OUTPUT_SECONDARY=y`, `CONFIG_FREERTOS_HZ=100`, `# CONFIG_FREERTOS_USE_TICK_HOOK is not set` (`MAP: oracle/hello/sdkconfig:435, 747-748, 800, 808`) **(M)**.

Three output paths reach USJ, chosen by what the application links:

| Path | When | USJ accesses | No host / host not reading |
|---|---|---|---|
| **P1, ROM** | always for `esp_rom_printf`, early logs, the bootloader; for `printf`/`ESP_LOG` when VFS is absent (`_write_r_console` → `esp_system_console_put_char` → `esp_rom_output_tx_one_char` = ROM `uart_tx_one_char`, `IDF:newlib/src/reent_syscalls.c:31-46`, `IDF:esp_system/esp_system_console.c:12-18`) | §B.1 | §B.1 |
| **P2, USJ VFS without driver** | `CONFIG_VFS_SUPPORT_IO` and the USJ console (primary: `/dev/usbserjtag`; secondary: `/dev/secondary`, `IDF:esp_driver_usb_serial_jtag/src/usb_serial_jtag_vfs.c:240-255`); the console VFS writes to primary then secondary (`IDF:esp_vfs_console/vfs_console.c:80-88`) | per byte: R `EP1_CONF`, W `EP1`; W `WR_DONE` per `\n` (`:148-171`); `fsync`: `WR_DONE`, wait for bit 1, `WR_DONE` again (`:339-356`) | `write`, `read`, `fsync` return -1 at once when `!usb_serial_jtag_is_connected()` (`:183-188, 231-236, 358-363`, "TODO: IDF-14303"); otherwise a byte waits while less than `TX_FLUSH_TIMEOUT_US = 50 ms` has passed since the last byte accepted, then is dropped, and so are all the following ones until bit 1 returns (`:72-81, 158-169`) |
| **P3, USJ VFS over the driver** | after `usb_serial_jtag_driver_install` + `usb_serial_jtag_vfs_use_driver` | §B.4 | tx: one blocking attempt of 50 ms per character (`:272-288`), then non-blocking; rx blocks unless `O_NONBLOCK` (`:261-270`) |

Notes:
* With ESP32-C3's `ESP_ROM_CONSOLE_OUTPUT_SECONDARY = 1` (`IDF:esp_rom/esp32c3/esp_rom_caps.h:33`), `esp_system_console_put_char` does **not** add its own USJ copy, because the ROM already mirrors (`esp_system_console.c:15-17`) **(R)**.
* `g_usb_print` is forced true by `esp_rom_install_uart_printf` (`IDF:esp_rom/patches/esp_rom_sys.c:72-87`, ESP32-C3 has `ESP_ROM_HAS_ETS_PRINTF_BUG`, `esp_rom_caps.h:23`), which the UART-console bootloader calls (`IDFGH:bootloader_support/src/bootloader_console.c:48-55`). The application on ESP32-C3 (single core) does not set the ROM console again: the only calls in `cpu_start.c` are in `call_start_cpu1` (`IDF:esp_system/port/cpu_start.c:203-241`). **The application inherits the ROM console from the bootloader** **(R)**: an application built with `CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y` behind the UART-console bootloader inkemu ships keeps ROM channel 0, so its P1 output goes to UART0 *and* USJ, and its P1 input (`esp_rom_output_rx_one_char`) comes from **UART0**. With its own bootloader (USJ console) the ROM channel is 3 and channel 2 is removed (`bootloader_console.c:133-148`): P1 output on USJ only, P1 input from USJ.
* **Reference `hello_world` (and `nvs_rw_value_cxx`)**: `MINIMAL_BUILD ON` (`oracle/hello/CMakeLists.txt:7`; NV §A.1); the map resolves `_write_r_console` at `0x42004b6c`, `esp_system_console_put_char` at `0x4200679a`, `esp_rom_output_tx_one_char = uart_tx_one_char` at `0x40000068`, contains no object of `libesp_driver_usb_serial_jtag.a`, no `usb_serial_jtag` VFS, and lists `.text.esp_register_freertos_tick_hook` under "Discarded input sections" (`MAP:4199, 8145, 11867-11870, 12024-12026`) **(M)**. So both reference images use P1 only: their USJ stream is their UART0 stream, and they never read `INT_RAW`. The NVS map was not inspected: same build style, **A**.
* **Panic** (`IDF:esp_system/panic.c:109-127`): per character, while bit 1 is 0 and a cumulative counter is below 500, wait 100 µs; write only if bit 1 is 1; the counter resets after each success. So a panic with no reader stalls 50 ms once, then drops. No `WR_DONE` per character (the 64-byte auto-commit and later output flush it) **(R)**.
* **stdin over USJ**: P2 reads non-blocking from the FIFO (`usb_serial_jtag_rx_char_no_driver`, `:173-181`); P3 blocks on the driver's ring. `CONFIG_LIBC_STDIN_LINE_ENDING_CR` is the default, so the host's `\r` becomes `\n`; stdout default `CRLF` (`IDF:newlib/Kconfig:26-74`) **(R)**.

### B.3 Connection monitor (R, `IDF:esp_driver_usb_serial_jtag/src/usb_serial_jtag_connection_monitor.c`, identical in v5.5.5)

* Linked when `CONFIG_USJ_NO_AUTO_LS_ON_CONNECTION` or `CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG_ENABLED` and the component is in the build (`-u usb_serial_jtag_connection_monitor_include`, `IDF:esp_driver_usb_serial_jtag/CMakeLists.txt:30-34`), or when anything references `usb_serial_jtag_is_connected` (HWCDC does). Its `ESP_SYSTEM_INIT_FN(…, SECONDARY, BIT(0), 230)` sets `s_usb_serial_jtag_conn_status = true`, adds a BBPLL consumer (`USB_SERIAL_JTAG_LL_PHY_DEPENDS_ON_BBPLL`) and registers the tick hook (`:74-90`).
* The tick hook runs from every SysTick interrupt, unconditionally (`IDFGH:freertos/port_systick.c:190-199` calls `esp_vApplicationTickHook`, which walks the registered callbacks, `IDFGH:esp_system/freertos_hooks.c:30-39`; it does not depend on `CONFIG_FREERTOS_USE_TICK_HOOK`).
* Each tick: `sof = INT_RAW & SOF`; `INT_CLR = SOF`; a change from connected to "no SOF" consumes one of `ALLOWED_NO_SOF_TICKS = pdMS_TO_TICKS(3)` credits, then declares disconnected (and drops the BBPLL consumer); a SOF while disconnected reconnects at once and refills the credits (`:22-26, 37-72`).
* `pdMS_TO_TICKS(3)` is 0 at 100 Hz and 3 at 1000 Hz (integer division), so "disconnected" comes **one tick after the scheduler starts** (10 ms) at the reference rate, and after four ticks at Arduino's rate. The monitor assumes SOF is raised at least once per tick ("SOF packet is sent by the HOST every 1ms on a full speed bus … SOF intr bit must have be raised at every tick hook", `:39-41`).

### B.4 `usb_serial_jtag` driver (R, `IDF:esp_driver_usb_serial_jtag/src/usb_serial_jtag.c`)

* `usb_serial_jtag_driver_install` (`:162-240`): ring buffers (RX size must exceed 64), bus clock on, `phy_set_defaults` (`CONF0`), **does not clear `INT_RAW`** ("The output routine needs `SERIAL_IN_EMPTY` set because it needs the ISR to trigger as soon as data is sent", `:219-222`), enables **`SERIAL_OUT_RECV_PKT` only** (`:226`), allocates source 26 (`ETS_USB_SERIAL_JTAG_INTR_SOURCE`, `:231`).
* `usb_serial_jtag_write_bytes` (`:265-289`): pushes into the TX ring with the caller's timeout, then enables `SERIAL_IN_EMPTY` — "If this was disabled, this will immediately trigger the ISR".
* ISR (`:61-160`): on `SERIAL_IN_EMPTY`: clear it; if bit 1: take ≤64 bytes from the stash or the ring, write the FIFO, `WR_DONE`, stash what did not fit ("ROM print routines … snuck in a full buffer"); if nothing to send: `WR_DONE` again (zero-length packet ending a 64-byte transfer), disable `SERIAL_IN_EMPTY`, re-enable it if the ring refilled meanwhile, else give `tx_idle_sem`. On `SERIAL_OUT_RECV_PKT`: clear it, read up to 64 bytes while bit 2, push to the RX ring.
* `usb_serial_jtag_wait_tx_done` waits for `tx_idle_sem` (`:291-318`).
* The driver itself never looks at SOF; without a draining host its TX ring fills and `write_bytes` times out.

### B.5 Arduino `HWCDC` (R, `ARD:`, core 3.3.11)

| Item | Behaviour |
|---|---|
| `begin()` `:410-472` | creates the RX queue (256) and TX ring (256); claims GPIO18/19 in the peripheral manager; `CONF0`: `PHY_SEL=0`, `PAD_PULL_OVERRIDE=0`, `DP_PULLUP=1`, `USB_PAD_ENABLE=1`; `INT_ENA &= ~0x7ffff`; `INT_ENA |= SERIAL_IN_EMPTY | SERIAL_OUT_RECV_PKT | USB_BUS_RESET`; `esp_intr_alloc(source 26)`. **No `INT_CLR`**: the reset value of `SERIAL_IN_EMPTY` (1) makes the ISR run as soon as the interrupt is enabled |
| ISR `:137-260` | `SERIAL_IN_EMPTY`: **`connected = true`**; if bit 1: clear, move ≤64 bytes (stash first, else ring) into the FIFO, `WR_DONE`, re-arm; if empty: `WR_DONE` (ZLP), disarm, re-arm if data arrived; if bit 1 is 0: just clear. `SERIAL_OUT_RECV_PKT`: clear, read ≤64 bytes into the RX queue (stop when full), **`connected = true`**. `USB_BUS_RESET`: clear, drop the stash, **`connected = false`**. SOF: commented out ("causing esptool to be unable to upload firmware") |
| `isPlugged()` (`HWCDC.h:73-77`) | `usb_serial_jtag_is_connected()`: the IDF tick-hook monitor of §B.3 |
| `isCDC_Connected()` `:268-291` (also `operator bool`, `isConnected`) | not plugged → `connected = false`, false; `connected` → true; else arm `SERIAL_IN_EMPTY`, write `WR_DONE`, return false. Called on every `write` and every `if (Serial)` |
| `write()` `:540-623` | connected: blocking `xRingbufferSend` with `tx_timeout_ms`, arm `SERIAL_IN_EMPTY` per chunk; after **20** consecutive time-outs return a short count, and mark disconnected (pushing the rest through the drop-oldest policy) only if `!isPlugged()`. Not connected: `flushTXBuffer` (drop the oldest bytes to make room, keep at most the ring size, `WR_DONE`) + arm `SERIAL_IN_EMPTY`; never blocks |
| `flush()` `:629-662` | connected: wait 1 ms per round while the ring does not shrink, `tx_timeout_ms` rounds, then declare disconnected and empty the ring |
| `tx_timeout_ms` | 100 by default (`:105`); CrossPoint sets 1 |
| RX | `available()` = queue count; `read()` non-blocking (`:683-722`); the queue holds 256 bytes; excess bytes of a packet are dropped (the ISR stops at a full queue, and has already popped them from the FIFO) |
| `end()` / `deinit()` `:387-408, 483-502` | disable and clear all interrupts, free, then drive GPIO18 (D−) and GPIO19 (D+) low open-drain "to force the host to re-enumerate" |
| `setDebugOutput(true)` `:728-736` | routes ROM `ets_printf` channel 2 to `cdc0_write_char` and removes channel 1 (UART). CrossPoint does not call it |

**Bytes written while "not connected" are buffered (last 256), not dropped**, and they leave as soon as `SERIAL_IN_EMPTY` fires, whatever `isPlugged()` says. A host that opens the port after boot therefore first receives the tail of the earlier log **(R)**.

---

## C. Interrupt (R)

* Source **26** = `ETS_USB_SERIAL_JTAG_INTR_SOURCE` ("interrupt of USJ, level", `IDF:soc/esp32c3/include/soc/interrupts.h:44`, the 27th entry of an enum starting at 0); map register `INTERRUPT_CORE0_USB_INTR_MAP_REG = INTERRUPT_CORE0_BASE + 0x068` (`IDF:soc/esp32c3/register/soc/interrupt_core0_reg.h:225`), `USB_INTR_MAP` in `INK:svd-regs/src/esp32c3.rs:2417`; the SVD calls the peripheral `USB_DEVICE`, IRQ 26 (`esp32c3-svd.md:103`).
* Level semantics: the line is the OR of `INT_ST = INT_RAW & INT_ENA` **(A: standard for Espressif peripherals; consistent with the drivers, which clear in the ISR and re-arm through `INT_ENA`)**. Enabling `SERIAL_IN_EMPTY` while its raw bit is set raises the line at once; both drivers rely on it (`usb_serial_jtag.c:282-286`, `HWCDC.cpp:282-289`).
* `SERIAL_IN_EMPTY` behaves as a latched event, not a level that follows the FIFO: the driver clears it "so we won't be called until the next transfer finishes" while the FIFO stays empty (`usb_serial_jtag.c:68`), and HWCDC sends a ZLP "so the next IN token will generate the interrupt" (`HWCDC.cpp:282-289`). It is set at reset and each time the host completes an IN transfer, a zero-length one included **(R + A on the exact trigger, §O.4)**.
* Bits enabled by the consumers: driver `SERIAL_OUT_RECV_PKT` permanently, `SERIAL_IN_EMPTY` while sending; HWCDC `SERIAL_IN_EMPTY`, `SERIAL_OUT_RECV_PKT`, `USB_BUS_RESET`; nobody enables `SOF`, `IN_TOKEN_REC_IN_EP1`, the error bits or `JTAG_IN_FLUSH` **(R)**. IR §494 already expects source 26 in CrossPoint **(R)**.

---

## D. What the host looks like at register level, and in device time

### D.1 Host states

| State | SOF (`INT_RAW` b1, `FRAM_NUM`) | IN endpoint | OUT endpoint | Events |
|---|---|---|---|---|
| **Absent** (no cable, or a charger) | never | the first committed packet (or 64 bytes) stays; bit 1 stays 0 after it | nothing arrives | — |
| **Attached, port closed** (enumerated, no program has the tty open) | every 1 ms | **not drained** (A: Linux `cdc-acm` submits its bulk-in URBs only when the tty is opened; same expectation for other hosts, §O.3) | nothing arrives | at attach: `USB_BUS_RESET` (A) |
| **Open** (a program reads) | every 1 ms | a committed packet is taken within one full-speed transaction time; then bit 1 = 1 and `SERIAL_IN_EMPTY` set; a ZLP is taken the same way | the host's writes arrive in packets of ≤64 bytes; each sets bit 2 and `SERIAL_OUT_RECV_PKT`, and `OUT_EP1_ST.REC_DATA_CNT` | — |
| **Detach** (cable pulled) | stops | stops | stops | none seen by the firmware (A: the device's D+ pull-up keeps the line idle, so no SE0 reset) |
| **Re-attach** | resumes after enumeration | resumes when a program opens the port; the packet pending since detach is delivered first (A) | | `USB_BUS_RESET` (A); HWCDC drops its stash and `connected` |

What each consumer then does (R, from §B):

| Consumer | Absent | Attached, port closed | Open |
|---|---|---|---|
| ROM (P1) | first packet accepted, one ~5 ms wait, then drops until the endpoint frees | same | everything delivered |
| IDF VFS (P2), full build | disconnected one tick after the scheduler starts (100 Hz) or four (1000 Hz): output discarded, no wait | connected: first stall 50 ms, then drops | delivered |
| IDF driver (P3) | ring fills, `write_bytes` times out | same | delivered |
| HWCDC | not plugged: drop-oldest 256-byte ring, no wait; nothing leaves | plugged and `connected` (set by the reset-value `SERIAL_IN_EMPTY` ISR): **~20 × `tx_timeout_ms` per write call once the ring is full**, then short writes | delivered |
| Panic handler | one 50 ms stall, then drops | same | delivered |

### D.2 Timing in device time

* **SOF**: one per 1 ms frame on a full-speed bus (`connection_monitor.c:39`); `FRAM_NUM` is the 11-bit frame number of the last SOF (modulo 2048) **(R)**. SOF starts once the host has enabled the port after its bus reset, before the port is opened (**A**, standard USB behaviour).
* **IN drain latency**: no figure in the sources. Full-speed bulk moves at most 64 bytes per transaction, about 50 µs for a full packet including token and handshake, and a host keeps polling a bulk-in endpoint throughout the frame while it has a read pending (**A**, §O.5). The consumers tolerate anything below 5 ms (ROM) or 50 ms (VFS); throughput matters only for bulk transfers such as CrossPoint's `CMD:SCREENSHOT` reply (§G.3). Tying the drain to SOF would cap throughput at 64 bytes/ms (a 48 KB framebuffer would take 0.75 s instead of tens of ms).
* **64-byte packets and ZLP**: "a 64-byte packet will be interpreted as a not-complete USB transaction, you need to transfer either more data or a zero-length packet for the data to actually end up at the program listening" (`ll.h:168-175`) **(R)**. This is host-OS buffering; an emulator that hands each drained packet to its console reproduces what a terminal shows once the ZLP follows, which both drivers send.
* **OUT**: one packet occupies the OUT FIFO until the firmware has read it (bit 2 back to 0); the host's next OUT transaction is NAKed meanwhile (**A**, standard for a single-buffered endpoint). The driver and HWCDC read the whole packet in the ISR; the VFS without driver reads byte by byte when the program asks.
* **Determinism**: everything above can be expressed in device time — SOF as a function of time since attach, drain and OUT delivery at fixed latencies after the event that allows them. No host time enters.

---

## E. Oracle

* **QEMU** (Espressif `esp_develop_9.2.2`): USJ is a stub, all reads 0 (BC §G.3, FW §K.4). It contradicts the reset values (bit 1 of `EP1_CONF` and bit 3 of `INT_RAW` are 1 on silicon) and models no host: it is not an oracle for any state, not even "absent" **(R)**.
* No other open emulator found that models ESP32-C3 USJ (Renode has no ESP32-C3 USJ model; Wokwi simulates the console but is closed and not scriptable in CI) **(A)**.
* **What validates the model instead**:
  1. the register contract: offsets, reset values and access types of §A, already in `svd-regs` (any mismatch is a test failure);
  2. the consumers' code paths of §B: unit tests that replay the exact register sequences (ROM drop/wait, monitor SOF/clear, driver ISR, HWCDC `begin` and ISR) against the model, with the expected values derived from the sources quoted;
  3. firmwares with fixed output: the ROM banner on USJ (`INK:soc-esp32c3/tests/real_rom.rs:141-150`, already pinned), the reference `hello_world` (§F.3), the stock echo example (§F.1, Espressif's pytest gives the expected strings);
  4. eventually a real ESP32-C3 board (a devkit or an X4 with a host) to capture drain latency and the attach/detach events of §D.1: the only true oracle; not available for this report.

---

## F. Reference firmwares for CI

The build discipline of `tools/reference-firmware/`: unmodified examples, its six-line `sdkconfig.defaults` as only configuration, an example that ships its own `sdkconfig.defaults` stops the script (`README.md`). Nothing below was built.

### F.1 `examples/peripherals/usb_serial_jtag/usb_serial_jtag_echo` (stock; driver, interrupts, both directions; no SOF dependence) — recommended

* Source (`IDFEX:peripherals/usb_serial_jtag/usb_serial_jtag_echo/main/usb_serial_echo_main.c:18-52`): a task installs the driver (RX and TX rings 1,024), logs `USB_SERIAL_JTAG init done`, then loops `len = usb_serial_jtag_read_bytes(data, 1023, 20 ms)`; if `len`, `usb_serial_jtag_write_bytes(data, len, 20 ms)` and `ESP_LOG_BUFFER_HEXDUMP("Recv str: ", data, len, ESP_LOG_INFO)`. `MINIMAL_BUILD ON`, `PRIV_REQUIRES esp_driver_usb_serial_jtag`. Licence `Unlicense OR CC0-1.0`.
* It ships `sdkconfig.defaults` = `CONFIG_ESP_CONSOLE_SECONDARY_NONE=y`. To stay unmodified, the build must merge instead of replace: `idf.py -D SDKCONFIG_DEFAULTS="<ref>/sdkconfig.defaults;sdkconfig.defaults" build` (ESP-IDF accepts a `;`-separated list) — a change to the script's rule, to be made explicitly. Resulting lines:

```
CONFIG_IDF_TARGET="esp32c3"
CONFIG_ESPTOOLPY_FLASHSIZE_16MB=y
CONFIG_ESPTOOLPY_FLASHMODE_DIO=y
CONFIG_ESPTOOLPY_FLASHFREQ_80M=y
CONFIG_ESP_DEFAULT_CPU_FREQ_MHZ_160=y
CONFIG_APP_REPRODUCIBLE_BUILD=y
CONFIG_ESP_CONSOLE_SECONDARY_NONE=y
```

* What it exercises: `CONF0` at install, `INT_ENA` RMW (bit 2 permanently, bit 3 per write), source 26, `INT_ST`, `INT_CLR` bits 2 and 3, OUT packets (bit 2, `EP1` reads), IN packets with `WR_DONE` and the trailing ZLP, `SERIAL_IN_EMPTY` from its reset value. The connection monitor is not linked (`ESP_CONSOLE_USB_SERIAL_JTAG_ENABLED` becomes n and nothing references it, **A**), so SOF does not matter: a clean test of the data path.
* Expected output (host open; input `hi, espressif\n` sent as one write after the init line):
  * UART0: the bootloader lines, `I (<t>) usb_serial_jtag echo: USB_SERIAL_JTAG init done`, then `I (<t>) Recv str: : 0x3fc…   68 69 2c 20 65 73 70 72  65 73 73 69 66 0a        |hi, espressif.|`. The address and times are deterministic in the emulator for a fixed input date, so the test can pin the whole text once observed.
  * USJ: the ROM banner and every P1 line (bootloader and `ESP_LOG`, mirrored by the ROM because `g_usb_print` stays true, §B.2), and the echoed bytes `hi, espressif\n`, interleaved at packet boundaries.
  * Espressif's own test (`pytest_usj_echo_example.py`, run on ESP32-C6 hardware) expects the echo for `hi, espressif\n`, `See you again!\n`, a 90-byte line split by the hexdump into 16-byte rows, and a 66-byte line — a ready list of inputs, the last two crossing the 64-byte packet size. The README sample shows an 88-byte input read as 64 + 24 bytes (one read per OUT packet). That split holds only while the task waits in `usb_serial_jtag_read_bytes` when the first packet comes: the handler's yield wakes it, and it takes the 64 bytes before the host's next packet. A line sent while the task still prints the dump of the previous one may reach the driver's ring whole, and be read in one call: seen in the emulator, where the second line went in the instant the first one's echo reached the host, and an assumption on a chip (§O.13).
  * USJ carries the console of the application because the ROM copies it, not because of ESP-IDF's secondary console, which `CONFIG_ESP_CONSOLE_SECONDARY_NONE` turns off: the image is a `MINIMAL_BUILD` without VFS (§O.9), so its `printf` and `ESP_LOG` take P1 (§B.2), whose ROM `uart_tx_one_char` sends each character to USJ while `g_usb_print` is set, as the bootloader leaves it (§B.1, §B.2).

### F.2 USJ as primary console with stdin (SOF dependence) — no stock example fits; scratch application proposed

No stock v5.5.2 example in the subset reads stdin over a USJ primary console with a small footprint; `examples/system/console/basic` (REPL, linenoise terminal probe, Wi-Fi commands) does with `CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y` but is heavy and was not read **(A)**. A 15-line application (built like NV's probe `E1`, scratch source; a decision for the owner since it departs from "unmodified examples"):

```c
#include <stdio.h>
#include "esp_err.h"
#include "driver/usb_serial_jtag.h"
#include "driver/usb_serial_jtag_vfs.h"

void app_main(void)
{
    usb_serial_jtag_driver_config_t cfg = USB_SERIAL_JTAG_DRIVER_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(usb_serial_jtag_driver_install(&cfg));
    usb_serial_jtag_vfs_use_driver();
    setvbuf(stdin, NULL, _IONBF, 0);
    printf("ready\n");
    char line[64];
    while (fgets(line, sizeof line, stdin)) {
        printf("echo: %s", line);
    }
    printf("stdin closed\n");
}
```

`main/CMakeLists.txt`: `PRIV_REQUIRES esp_driver_usb_serial_jtag vfs`; no `MINIMAL_BUILD` (the VFS is needed). `sdkconfig.defaults`: the six reference lines plus `CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y`. Consequences: its own bootloader prints on USJ only and differs from the shipped one (a new pin); behind the shipped bootloader (`--firmware`) the ROM channel stays UART0 (§B.2) but stdout/stdin go through the USJ VFS anyway. Expected on USJ with the host open and the input `hello\r`: `ready\r\n`, `echo: hello\r\n`. With SOF absent: nothing at all after the bootloader, because the VFS discards output and `read` returns -1 one tick after the scheduler starts (`fgets` ends; `stdin closed` is discarded too). With the host attached and the port closed: `ready` waits 50 ms then is dropped. This firmware is the one that proves the SOF model.

### F.3 The reference `hello_world`, as it is

Since its USJ stream is its UART0 stream through P1 (§B.2, **M** for the link), a cheap test on the existing pinned image: with the host open, the bytes taken from USJ equal the bytes taken from UART0, `\r\n` included (both go through `uart_tx_one_char`) **(A until run)**. A second assertion with the host absent: USJ carries the first committed packet only, `ESP-ROM:esp32c3-api1-20210207\r\n`, and the boot is about 5 ms longer in device time than with the host open (§B.1).

---

## G. CrossPoint

### G.1 What the source says (R)

* Platform `pioarduino/platform-espressif32 55.03.311` (`CP:platformio.ini:10`), whose release is "Arduino Release v3.3.11 based on ESP-IDF v5.5.5" (GitHub release `55.03.311`, 2026-07-24); `platform.json` points at `esp32-core-3.3.11`. The ESP32-C3 environments rebuild the core with `custom_sdkconfig` (`[firmware_tuned]`, `[firmware_tuned_c3]`, `CP:platformio.ini:179-264`): the IDF version is **5.5.5, not the 5.5.2 of the reference firmwares**; the connection monitor is byte-identical (§0). `LB:defconfig.common:45` sets `CONFIG_FREERTOS_HZ=1000` and no console line **(R; that the C3 rebuild keeps them is A)**.
* `-DARDUINO_USB_MODE=1 -DARDUINO_USB_CDC_ON_BOOT=1` (`CP:platformio.ini:39-40`): `Serial` is `HWCDCSerial` (`ARD:HWCDC.cpp:738-741`).
* Logging: X4/X3 select `FREEINK_LOG_TRANSPORT_SERIAL` (`FSDK:BoardConfig.h:370-381`), so `logPrintf` formats `[<ms>] [<LVL>] [<origin>] …` and calls `logSerial.print(buf)` — `logSerial` is the HWCDC — "even when `logSerial` reports disconnected" (`CP:lib/Logging/Logging.cpp:39-75`, `Logging.h:34-36`). `ENABLE_SERIAL_LOG` is set in every C3 environment except `slim`; release builds use `LOG_LEVEL=1` (`CP:platformio.ini:278-286`), the default developer environment `LOG_LEVEL=2` and `CROSSPOINT_WAIT_FOR_USB_SERIAL` (`:266-275`).
* `setup()` (`CP:src/main.cpp:430-442`): `BoardConfig::holdPowerRails()`; developer builds only: `delay(250)`; `Serial.begin(115200)`; `logSerial.setTxTimeoutMs(1)` ("load-bearing 1"). No `setDebugOutput`.
* `loop()`: the memory line is printed only `if (Serial …)`, i.e. only when `isCDC_Connected()` is true (`:713`); `logSerial.available()` → `readStringUntil('\n')`; a line `CMD:SCREENSHOT` makes it print `SCREENSHOT_START:<size>\n`, write the raw framebuffer (`display.getBufferSize()` bytes), then `SCREENSHOT_END\n` (`:725-740`).
* Deep sleep: `logSerial.end()` drives D−/D+ low (`CP:lib/hal/HalPowerManager.cpp:83-89`, §B.5).

### G.2 Its USJ register life (R, matched to FW §K.3 M)

1. ROM banner and bootloader: P1 (§B.1).
2. From the scheduler start: the IDF connection monitor, every 1 ms: R `INT_RAW`, W `INT_CLR = 2` — FW's `+588,725` `INT_RAW`, `INT_CLR` and most of the 10,744 `USB_DEVICE` reads.
3. `Serial.begin` at `+691,716`: W `CONF0`, `INT_ENA` (clear 19 bits, set bits 2, 3, 9), map source 26; the reset-value `SERIAL_IN_EMPTY` fires the ISR at once → `connected = true`, ring empty → `WR_DONE` (ZLP), disarm.
4. Each log line: `isCDC_Connected()` (→ `isPlugged()`), then the connected or the drop-oldest path; `EP1_CONF` from `+708,954`.
5. Each `loop()`: `if (Serial)` → `isCDC_Connected()`, which re-arms `SERIAL_IN_EMPTY` and writes `WR_DONE` whenever it returns false.

### G.3 What the emulator must offer for the log to appear

| Offered | Result |
|---|---|
| Nothing (QEMU, inkemu `main`: `INT_RAW` 0 or unmodelled, `INT_ENA` unmodelled) | no line on USJ: monitor sees no SOF → not plugged → ring only; no interrupt ever drains it (FW §K.2/§K.4) |
| SOF only, no drain | `isPlugged()` true, `connected` true after `begin`; the first packet leaves into a FIFO nobody drains; then **every write blocks ~20 ms** and returns short. No line visible, and the boot is slower |
| Drain only, no SOF | lines appear through the drop-oldest path: a burst longer than 256 bytes between two drains loses its head; never blocks |
| **SOF every 1 ms + drain + interrupts** (the "open" host) | every line appears, in order, with flow control; `if (Serial)` is true, so the periodic `MEM` line appears too; `CMD:SCREENSHOT` works if host input is offered |

Minimum set: `INT_ENA` storage, `INT_ST`, `INT_CLR`, source 26 asserted from `INT_ST`, `SERIAL_IN_EMPTY` (reset 1, set on each drained packet), `EP1_CONF` bits 1 and `WR_DONE` semantics, `SOF` bit set at each frame boundary, `CONF0` storage; for input, bit 2, `SERIAL_OUT_RECV_PKT`, `EP1` reads. `USB_BUS_RESET` at attach is optional (it only resets `connected`, which the next `SERIAL_IN_EMPTY` restores).
Side conditions that are not USJ: the monitor needs the 1 kHz SysTick and its interrupt (SP1b-3 has them); CrossPoint stops at its first SAR ADC read before most of its log (FW §K.4) — USJ makes its first lines visible, not more. A board-level USB-presence input (FW §K.5 mentions a GPIO that reads "USB absent" at 0) should agree with the host state; that pin belongs to the board profile (SP2), not to this report **(A)**.

---

## H. Relation to earlier reports (those files are left untouched)

| Report | Statement | Correction or completion |
|---|---|---|
| BC §G.3 | ROM "retry up to 904 times with a 1 µs `ets_delay`" | **5,000** polls (`0x1388`): `lui s0,0x1` then `addi s0,s0,904` (`ROM:0x4004c922, 0x4004c934`). The delay is a call through `ets_ops_table_ptr` with argument 1 (**A** that it is `ets_delay_us`) |
| BC §G.3 | HWCDC "relies on the interrupt bits above and on SOF to detect a host (not read in this pass — unverified)" | §B.5: interrupts 2, 3, 9 via its own ISR; SOF only through ESP-IDF's tick-hook monitor, not through an interrupt |
| BC §G.3, "two plausible answers" | app `ESP_LOGx` on UART0, `Serial.print` on USJ | completed by §B.2: `ESP_LOGx` also reaches USJ, through the ROM mirror (minimal builds) or the secondary VFS gated by SOF (full builds) |
| FW line 406, §O.4 | the firmware's log reaches USJ "only when a USB host has the port open"; whether a boot without host loses or buffers the first lines | §G: delivery needs drained IN packets (and SOF for flow control); without a host the last 256 bytes are **buffered** and appear first when a host starts reading; with the port closed but the cable in, each line costs ~20 ms |
| FW §K.3 `+588,725` "USB-Serial-JTAG driver" | | the ESP-IDF connection monitor (tick hook), not the driver |
| FW `cr/platformio.ini:39-40` | | at `e54c008` the same lines; the core is arduino-esp32 3.3.11 on ESP-IDF 5.5.5 (§G.1) |

---

## R. Recommendations for SP1b-3d

1. **Registers and bits to model** (all at `0x6004_3000`, reset values of §A):
   * `EP1`: write pushes into a 64-byte IN FIFO, ignored when bit 1 is 0 (**A**, §O.6); read pops the OUT FIFO (0 when empty, **A**).
   * `EP1_CONF`: `WR_DONE` commits the IN FIFO as a packet, an empty one included (a ZLP); a 64th byte commits by itself; bit 1 = "no committed packet pending and FIFO count < 64"; bit 2 = OUT FIFO not empty.
   * `INT_RAW` (bits 0–11 stored, writes ignored), `INT_ENA` (bits 0–18 stored), `INT_CLR` (write-1-clears `INT_RAW`), `INT_ST = INT_RAW & INT_ENA`; `ctx.irq(source 26, INT_ST != 0)` on every change (ADR 0003). Bits produced: `SOF` (1), `SERIAL_OUT_RECV_PKT` (2), `SERIAL_IN_EMPTY` (3, reset 1), `USB_BUS_RESET` (9), `OUT_EP1_ZERO_PAYLOAD` (10) for an empty host write; `IN_TOKEN_REC_IN_EP1` (8) may be set with each drain at no cost. Error bits and `JTAG_IN_FLUSH` never.
   * `CONF0`, `TEST`, `MISC_CONF`, `MEM_CONF`, `DATE`: storage with reset values. `FRAM_NUM`: frame index. `IN_EP1_ST`/`OUT_EP1_ST`: derive `WR_ADDR`/`RD_ADDR`/`REC_DATA_CNT` from the FIFOs if cheap, else read-only reset values with an `ASSUMPTION:` — no consumer reads them. `JFIFO_ST`, `IN_EP0/2/3_ST`, `OUT_EP0/2_ST`: reset values (JTAG is out of scope).
   * Leave `SYSTEM.PERIP_RST_EN0.USB_DEVICE_RST` handling explicit: a pulse resets the block (FIFOs, `INT_*`), not the host state.
2. **Host model** — an input of the run (ADR 0004 point 1), not a consequence of `--console`:
   * states `absent`, `attached` (port closed: SOF, no drain, no input), `open` (SOF, drain, input); **default `open`**, which keeps today's output (the ROM banner test, `hello_world` on USJ) and is what a user watching the USB console expects;
   * attach at a device time (default 0 for `attached`/`open`): set `USB_BUS_RESET`, start frames; detach: stop frames, drains and input; a later attach repeats the reset (all dated, so SP2's "USB cable" toggle and SP1b-4's replay can drive them);
   * the host state survives every reset of the chip (core, system, `esp_restart`); the block's own state follows IR §4 (kept by a core reset) — today's `UsbSerialJtag::reset` empties the packet being filled on a core reset, which contradicts IR: check against the reset table before keeping it.
3. **SOF timing**: lazy, no event per frame. Frame index `n = floor((now - attach) / 1 ms)`; `FRAM_NUM = n mod 2048`; `INT_RAW.SOF` is set when a read (or an `INT_ST` evaluation) finds `n` greater than the frame of the last clear. Schedule a `ctx` call at the next frame boundary only while `INT_ENA.SOF` is set (nobody does today). The monitor's tolerance (3 ticks at 1 kHz, 0 at 100 Hz) means SOF must be visible at *every* tick: with exactly 1 ms frames and 1 ms (or 10 ms) ticks this holds; add a test that a 100 Hz tick hook never misses one.
4. **IN drain**: in `open`, a committed packet (ZLP included) is taken at `commit + D` with one `ctx.schedule` per packet; then bit 1 = 1, `SERIAL_IN_EMPTY` set, the bytes appended to the USJ console stream. Pick D as one full-speed bulk transaction (≈50 µs, **A**) and name it as a model constant with an `ASSUMPTION:` until a board measures it. In `absent`/`attached`, committed packets stay; on a later `open`, the pending packet leaves first.
5. **Host input to the OUT endpoint**: dated byte chunks (`at`, bytes); each chunk is split into packets of ≤64 bytes, a zero-length chunk is one zero-length packet. A packet is loaded into the OUT FIFO at `max(its date, previous packet fully read) + D`; loading sets bit 2, `SERIAL_OUT_RECV_PKT` and `REC_DATA_CNT`. Inputs arriving while not `open` wait (or are refused by name: decide in the plan). These chunks are the dated inputs ADR 0004 announced for SP1b-3d.
6. **CLI implications**: a flag for the host state (e.g. `--usb-host absent|attached|open`, default `open`) printed in the run summary next to the other inputs; a reproducible input source for the OUT endpoint (a file of dated chunks); interactive stdin forwarding to USJ only as a convenience that records the device time of each chunk (otherwise not reproducible, like Ctrl-C); `--console usb-serial-jtag` keeps choosing what is shown, never the host state. Keep the logic in `soc-esp32c3` (the GUI-first rule): the CLI only parses and forwards.
7. **Tests**: register-sequence unit tests for the ROM path (drop when never free; 5,000-poll give-up), the monitor (SOF set and cleared per tick; disconnect after one 100 Hz tick without SOF), the driver ISR, HWCDC `begin`/ISR (`connected` from the reset value of `SERIAL_IN_EMPTY`); then §F.3 on the pinned `hello_world` (USJ = UART0 when `open`; first packet only when `absent`); §F.1 as the new CI firmware for both directions and interrupts; §F.2 if the owner accepts a scratch application, as the SOF proof. CrossPoint (SP1b-3e) checks the whole in tolerant mode: its first `[INF]` lines on USJ with the default host.

---

## O. Risky assumptions

| # | Assumption | Where it matters | What would settle it |
|---|---|---|---|
| 1 | `INT_ST = INT_RAW & INT_ENA` and source 26 is the OR of `INT_ST` | interrupt delivery | TRM interrupt chapter; behaviour of the drivers is consistent |
| 2 | `WR_DONE` on an empty FIFO commits a ZLP that blocks bit 1 until the host takes it | HWCDC calls it whenever not connected; with no host it would block the endpoint for good | real board, or TRM USJ chapter |
| 3 | A host with the port closed sends SOF but never drains bulk-in; detach produces no `USB_BUS_RESET`; attach produces one | the three host states and CrossPoint's 20 ms stalls | Linux `cdc-acm` source (URB submission on open), a capture on a board |
| 4 | `SERIAL_IN_EMPTY` is set exactly once per completed IN transfer (ZLP included), not while the FIFO is empty | ISR re-entry; `connected` in HWCDC | real board |
| 5 | Drain latency ≈ 50 µs per packet, independent of SOF | throughput, `CMD:SCREENSHOT` duration, boot time with the host open | real board with a host reading |
| 6 | `EP1` write while bit 1 is 0 is ignored; `EP1` read with an empty OUT FIFO returns 0 | only firmware that does not poll (none found) | TRM |
| 7 | The ROM's retry delay is 1 µs per poll (`ets_ops_table_ptr[0](1)` = `ets_delay_us`) | the ~5 ms wait of §B.1 | dump `ets_ops_table` from the ROM data |
| 8 | CrossPoint's C3 core rebuild keeps `CONFIG_FREERTOS_HZ=1000` and the secondary USJ console of the lib-builder defaults | monitor tolerance, IDF logs on USJ | the `sdkconfig` of a CrossPoint build |
| 9 | The `usb_serial_jtag_echo` build without the secondary console links neither the connection monitor nor the VFS | that F.1 is independent of SOF | its linker map after the first build |
| 10 | `nvs_rw_value_cxx` uses the ROM path only, like `hello_world` | §F.3 on the NVS image | its linker map |
| 11 | The X4 wires GPIO18/19 to its USB-C connector and has a USB-presence GPIO that must agree with the host state | coherence of the board profile | X4 schematic or board probing (SP2) |
| 12 | Core reset keeps the USJ block (IR §4) while inkemu's `UsbSerialJtag::reset` clears the packet being filled | output across `esp_restart` | the reset table of the TRM |
| 13 | A line longer than a packet, sent while the `usb_serial_jtag_echo` task still prints the dump of the line before, reaches the driver's ring whole and is read in one call (seen in the emulator only; on a chip the dump, about 95 characters, fits in the 128-byte UART FIFO and may be done before the second OUT packet) | the script of milestone 2, which waits for that dump before it sends | an echo on a board, a line sent the moment the echo of the line before arrives |
