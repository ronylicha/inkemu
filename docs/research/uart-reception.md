# UART reception on the ESP32-C3: registers, readers, interrupts, timing, oracle and a CI firmware

Scope: the receive side of UART0 and UART1 on the ESP32-C3. It covers the registers and bits on the receive path, which code reads them (the mask ROM, ESP-IDF stdin through the VFS with and without the driver, the `uart` driver), the interrupt source and its level semantics, how long a received byte takes and what the receive timeout counts, the frame format and the duration it gives a transmitted byte, what Espressif QEMU can and cannot serve as an oracle for, a stock firmware for a CI test of reception, and whether CrossPoint reads UART0. It ends with recommendations for the plan SP1b-3d ("serial consoles in both directions"). The host side of USB-Serial-JTAG is out of scope.
Date of research: 2026-10-13. No code was changed; this file is the only repository edit.
Earlier reports cited: `esp32c3-boot-chain.md` (**BC**), `esp32c3-interrupts-and-reset.md` (**IR**), `sp1b-test-oracle.md` (**TO**), `flash-writes-and-crosspoint-boot.md` (**FW**), `esp-idf-storage-flash-traffic.md` (**ST**), `xteink-hardware.md` (**XH**).

## 0. Method, sources, tags

| Tag | Meaning |
|---|---|
| **M** (MEASURED) | seen in a QEMU run made for this report (Espressif QEMU `esp_develop_9.2.2_20250817` inside Docker image `espressif/idf:v5.5.2`, commands in §12), in the output of a firmware built for it, or in the QEMU binary itself |
| **R** (READ) | read in a source file or a disassembly (`file:line` or `symbol@address`) |
| **A** (ASSUMED) | inferred; every A item that carries risk is repeated in §11 |

| Prefix | What | Where |
|---|---|---|
| `IDF:` | ESP-IDF v5.5.2 sources, paths relative to `components/` (the subset extracted from the pinned image) | scratch `r3d/idf/components/` |
| `EX:` | ESP-IDF v5.5.2 examples, relative to `examples/` | scratch `r3d/idf/examples/` |
| `ROM3:` | ESP32-C3 rev 3 mask ROM, `objdump -d` of `esp32c3_rev3_rom.elf` and its symbol table | scratch `r3d/rom/rom.dis`, `rom.nm` |
| `SVD:` | `third_party/espressif-svd/esp32c3.svd` (reset values, map register offsets) | repository |
| `INK:` | inkemu at `main` (`0f866ed`), paths relative to `crates/soc-esp32c3/src/` | repository |
| `CP:` | CrossPoint sources | scratch `repos/crosspoint-reader_crosspoint-reader/` |
| Run P1 | `uart_rx_probe` (§12, a 150-line probe written for this report, scratch only) under QEMU with a scripted UART0 input, MMIO trace of the UART registers | `r3d/q/out/p1.{uart,events,uarttrace,seg}` |
| Runs S1, S2, S3 | the stock example `peripherals/uart/uart_select`, built with the reference `sdkconfig.defaults` only, under QEMU: S1 waits for the first timeout line before sending; S2 and S3 send `a` 1 s after `app_main` returns, then `bc\r\n` | `r3d/q/out/s{1,2,3}.*` |

QEMU was given its input on `-serial stdio` by a Python script running in the container (`r3d/q/tools/drive.py`): it waits for a text in the UART output, sleeps, then writes bytes. QEMU source was not read; QEMU facts come from runs and from the strings of its binary. ESP-IDF facts are read in its Apache-2.0 sources; examples are `CC0`/Public Domain. No third-party binary ran on the host.

## 1. Key results

1. **The receive FIFO is read through `FIFO` (offset 0x00) at the same base as everything else**: `0x6000_0000` (UART0) and `0x6001_0000` (UART1). On the C3 the "AHB FIFO" address of ESP-IDF is the same address (`REG_UART_AHB_BASE(i) = 0x60000000 + i*0x10000`); the ROM uses it too. There is no second alias to model **(R+M)**.
2. **Software uses six things on the receive path**: `STATUS.RXFIFO_CNT` (bits 9:0), a read of `FIFO` (byte in bits 7:0), the `CONF0.RXFIFO_RST` pulse (bit 17), `CONF1.RXFIFO_FULL_THRHD` (bits 8:0) and `CONF1.RX_TOUT_EN` (bit 21), `MEM_CONF.RX_TOUT_THRHD` (bits 25:16), and the interrupt registers `INT_ST`/`INT_ENA`/`INT_CLR` with bits 0 (`RXFIFO_FULL`) and 8 (`RXFIFO_TOUT`). Nothing on the measured or read paths reads `MEM_RX_STATUS`, `RXD_CNT`, `ST_URX_OUT` or `STATUS.RXD` **(R+M)**.
3. **The ROM never reads UART0 during a normal (SPI flash) boot.** It configures reception (`CONF1 = 1`, `INT_ENA = 1`, source 21 → CPU line 5, priority 1) but keeps line 5 disabled; its receive functions run only in download mode. In P1, the first read of `UART0.FIFO` is the application's **(R+M)**.
4. **ESP-IDF stdin without the driver polls**: `getchar` reads `STATUS` then one byte from `FIFO` per character and returns `EOF` (errno `EWOULDBLOCK`) at once when the FIFO is empty; a reader must loop. Default line endings: input `\r` becomes `\n` (`CONFIG_LIBC_STDIN_LINE_ENDING_CR`), so `x\r\ny` reads as `x`, `\n`, `\n`, `y` **(R+M)**.
5. **The driver (`uart_driver_install`) is interrupt-driven**: it enables `INT_ENA = 0x00080195` (`RXFIFO_FULL`, `PARITY_ERR`, `RXFIFO_OVF`, `BRK_DET`, `RXFIFO_TOUT`, `WAKEUP`), sets the full threshold to 120 and the timeout to 10 symbols = 100 bit times in 8N1; its ISR reads `INT_ST`, `STATUS`, every byte of `FIFO`, `AT_CMD_CHAR` twice, writes `INT_CLR = 0x101`, and reads `INT_ST` again. When it must wait for room in the transmit FIFO it also enables `TXFIFO_EMPTY` (bit 1) **(R+M)**.
6. **Interrupt**: UART0 is source 21 (`UART_INTR_MAP`, `0x600C2054`), UART1 source 22 (`UART1_INTR_MAP`), both level. `INT_ST = INT_RAW & INT_ENA`; `INT_CLR` bits clear `INT_RAW`. ESP-IDF routed UART0 to CPU line 7, priority 1, in S3. inkemu's SYSTIMER and timer groups already drive a level source as `ctx.irq(source, raw & ena != 0)` after every change; the UART can do the same **(R+M)**.
7. **Timing**: a frame is `1 + (5 + BIT_NUM) + PARITY_EN + stop` bit times, stop = 1, 1.5 or 2 for `STOP_BIT_NUM` 1, 2, 3; one bit is `(CLKDIV + FRAG/16) × (SCLK_DIV_NUM + 1)` source-clock periods. At 115 200 baud, 8N1: 86.8 µs per byte. `RX_TOUT_THRHD` counts bit times (ESP-IDF multiplies symbols by the symbol length). The model's fixed 10 bits per transmitted byte is right for every measured console (8N1) and wrong otherwise **(R)**.
8. **QEMU can feed UART0 bytes, deterministic in content but not in device time**, and its model differs from the chip in ways that matter for a test: bytes arrive all at once (not at the line rate), the FIFO never overflows (back-pressure instead), and **configuring the timeout raises a `RXFIFO_TOUT` with an empty FIFO**, which makes the stock `uart_select` example block in its first `read` until a byte comes **(M)**.
9. **Recommended CI firmware: the stock `peripherals/uart/uart_select`**, unmodified, with the reference `sdkconfig.defaults` and nothing else: UART0, driver, interrupts, `select()` with a 5 s timeout, one log line per received byte. It builds in the reference container (bootloader and partition table are the pinned files) and Espressif's own test pins its two key lines (§9) **(M+R)**.
10. **CrossPoint on the C3 does not read UART0**: its `Serial` is USB-Serial-JTAG (`ARDUINO_USB_CDC_ON_BOOT=1`), and on the X4 the UART0 receive pin GPIO20 is wired to USB/VBUS detect **(R)**.

---

## 2. The receive path, register by register

### 2.1 Addresses (R+M)

| Fact | Source |
|---|---|
| `DR_REG_UART_BASE = 0x60000000`, `DR_REG_UART1_BASE = 0x60010000` | `IDF:soc/esp32c3/register/soc/reg_base.h:20,29` |
| `REG_UART_BASE(i) = DR_REG_UART_BASE + i*0x10000`; `REG_UART_AHB_BASE(i) = 0x60000000 + i*0x10000`; `UART_FIFO_AHB_REG(i) = REG_UART_AHB_BASE(i) + 0` | `IDF:soc/esp32c3/include/soc/soc.h:20-22` |
| The register struct names offset 0 `ahb_fifo.rw_byte`; `uart_ll_read_rxfifo` reads `hw->ahb_fifo.val` once per byte, and `uart_ll_write_txfifo` writes the same register | `IDF:soc/esp32c3/register/soc/uart_struct.h:13-19`; `IDF:hal/esp32c3/include/hal/uart_ll.h:344-349, 360-366` |
| `UART_RXFIFO_RD_BYTE`: RO, bits [7:0] of `FIFO` | `IDF:soc/esp32c3/register/soc/uart_reg.h:15-20` |
| The ROM computes the base as `(0x6000 + n) << 16` (+`0xE000` for n > 1) and reads offset 0 for the byte | `ROM3:uart_rx_one_char@0x4004c520-0x4004c53c` |
| Every UART access of P1, S1–S3 is at `0x6000_00xx` or `0x6001_00xx`; the application's byte reads are `R 0x60000000` | M |

So the "AHB vs APB" question of the ESP32 does not arise on the C3: one address, write = transmit, read = receive. inkemu maps UART0 at `0x6000_0000` and UART1 at `0x6001_0000`, 4 KiB each (`INK:periph/mod.rs:339-343`), which already covers it.

### 2.2 Registers and fields (R, with the SVD reset values)

| Offset | Register | Field (bits) | Meaning | Reset | Who uses it |
|---|---|---|---|---|---|
| 0x00 | `FIFO` | `RXFIFO_RD_BYTE` [7:0] | read: pops one received byte | — | ROM receive functions, VFS, driver |
| 0x04 | `INT_RAW` | 20 bits, see §2.3 | raw interrupt state, R/WTC/SS | `0x00000002` (`TXFIFO_EMPTY` = 1) | probe only (`uart_ll_get_intraw_mask` exists, `uart_ll.h:293-296`) |
| 0x08 | `INT_ST` | same bits | `INT_RAW & INT_ENA`, RO | 0 | driver ISR, ROM download ISR |
| 0x0C | `INT_ENA` | same bits | enables | 0 | ROM (`|= 1`), driver |
| 0x10 | `INT_CLR` | same bits | write 1 clears the raw bit, WT | — | ROM, driver |
| 0x1C | `STATUS` | `RXFIFO_CNT` [9:0]; `RXD` [15] line level; `TXFIFO_CNT` [25:16] | bytes in the RX FIFO | `0xE000C000` | ROM, VFS, driver |
| 0x20 | `CONF0` | `PARITY` [0], `PARITY_EN` [1], `BIT_NUM` [3:2], `STOP_BIT_NUM` [5:4], `LOOPBACK` [14], `RXFIFO_RST` [17], `TXFIFO_RST` [18], `RXD_INV` [19], `ERR_WR_MASK` [26], `AUTOBAUD_EN` [27], `MEM_CLK_EN` [28] | frame format, FIFO resets | `0x1000001C` (8N1) | ROM, bootloader (no), driver |
| 0x24 | `CONF1` | `RXFIFO_FULL_THRHD` [8:0], `TXFIFO_EMPTY_THRHD` [17:9], `DIS_RX_DAT_OVF` [18], `RX_TOUT_FLOW_DIS` [19], `RX_FLOW_EN` [20], `RX_TOUT_EN` [21] | thresholds, timeout enable | `0x0000C060` (96, 96, timeout off) | ROM (`= 1`), driver |
| 0x48 | `IDLE_CONF` | `RX_IDLE_THRHD` [9:0], `TX_IDLE_NUM` [19:10] | frame-end detection (for DMA), idle between transfers | `0x00040100` (256, 256) | driver writes `TX_IDLE_NUM = 0` |
| 0x4C | `RS485_CONF` | `DL0_EN` [1], `DL1_EN` [2] … | RS485; delay the stop bit by 1 bit each | 0 | driver (read-modify-write, value 0) |
| 0x5C | `AT_CMD_CHAR` | `DATA` [7:0], `CHAR_NUM` [15:8] | pattern detection | `0x0000032B` | driver ISR reads it twice per pass |
| 0x60 | `MEM_CONF` | `RX_SIZE` [3:1], `TX_SIZE` [6:4], `RX_FLOW_THRHD` [15:7], `RX_TOUT_THRHD` [25:16] | FIFO sizes in 128-byte blocks, timeout threshold | `0x000A0012` (1, 1, 0, 10) | driver |
| 0x68 | `MEM_RX_STATUS` | `APB_RX_RADDR` [9:0], `RX_WADDR` [20:11] | read and write offsets in the shared UART RAM ("UART0 is 10'h100. UART1 is 10'h180") | `0x00080100` | nobody (no LL accessor on the C3) |
| 0x6C | `FSM_STATUS` | `ST_URX_OUT` [3:0], `ST_UTX_OUT` [7:4] | receiver and transmitter state | 0 | ROM and VFS read `ST_UTX_OUT` only |

Sources: field layout `IDF:soc/esp32c3/register/soc/uart_struct.h:20-45` (`int_raw`), `141-155` (`status`), `157-188` (`conf0`), `189-200` (`conf1`), `264-271` (`idle_conf`), `272-284` (`rs485_conf`), `306-313` (`at_cmd_char`), `314-326` (`mem_conf`), `336-344` (`mem_rx_status`), `345-352` (`fsm_status`); bit positions and access types `IDF:soc/esp32c3/register/soc/uart_reg.h:15-20, 23-156, 614-619, 690-695, 792-797, 1059-1063`; reset values `SVD:` (UART0 block). `RX_TOUT_THRHD` lives in **`MEM_CONF`**, not in `CONF1` as on the ESP32; the enable is `CONF1.RX_TOUT_EN` (`uart_ll_set_rx_tout`, `uart_ll.h:966-975`).

`SOC_UART_FIFO_LEN = 128` (`IDF:soc/esp32c3/include/soc/soc_caps.h:414`); with `RX_SIZE = TX_SIZE = 1` each direction has one 128-byte block of the UART RAM, at offset `0x100` (UART0) or `0x180` (UART1) for reception. The transmit model of inkemu already uses 128 (`INK:periph/uart.rs:57-59`).

### 2.3 Interrupt bits (R; "used by" from §3)

| Bit | Name | Set when (header comment) | Used by |
|---|---|---|---|
| 0 | `RXFIFO_FULL` | "receiver receives more data than what `rxfifo_full_thrhd` specifies" | ROM (`INT_ENA |= 1`, download ISR), driver |
| 1 | `TXFIFO_EMPTY` | "amount of data in Tx-FIFO is less than what `txfifo_empty_thrhd` specifies"; reset value 1 | driver transmit path (§3.3) |
| 2 | `PARITY_ERR` | parity error in the received data | driver (enabled; handler clears it) |
| 3 | `FRM_ERR` | data frame error | driver (handler clears it; not enabled) |
| 4 | `RXFIFO_OVF` | "receiver receives more data than the FIFO can store" | driver (enabled; resets the FIFO) |
| 5, 6 | `DSR_CHG`, `CTS_CHG` | modem lines | nobody |
| 7 | `BRK_DET` | "receiver detects a 0 after the stop bit" | driver (enabled) |
| 8 | `RXFIFO_TOUT` | "receiver takes more time than `rx_tout_thrhd` to receive a byte" | driver |
| 9, 10 | `SW_XON`, `SW_XOFF` | software flow control | nobody by default |
| 11 | `GLITCH_DET` | glitch in the middle of a start bit | nobody |
| 12, 13, 14 | `TX_BRK_DONE`, `TX_BRK_IDLE_DONE`, `TX_DONE` | transmit break, transmitter idle after the FIFO | driver (`TX_DONE` for `uart_wait_tx_done` and RS485) |
| 15–17 | RS485 errors | RS485 mode | driver in RS485 mode |
| 18 | `AT_CMD_CHAR_DET` | pattern detected | driver (pattern API) |
| 19 | `WAKEUP` | RX edges during light sleep | driver (enabled) |

Sources: `IDF:soc/esp32c3/register/soc/uart_struct.h:20-45`; enum `IDF:hal/esp32c3/include/hal/uart_ll.h:44-63`; `UART_LL_INTR_MASK = 0x7ffff` (`:37`). All `INT_RAW` bits are "R/WTC/SS": set by hardware, cleared by writing `INT_CLR` (`uart_reg.h:23-156`). Whether a bit whose condition still holds is set again right after a clear (a level) or only on the next event is not documented (§11, A1).

### 2.4 Overflow (R+A)

The field comments give the event (`RXFIFO_OVF`, bit 4) and a switch to ignore it (`CONF1.DIS_RX_DAT_OVF`, "Disable UART Rx data overflow detect"). They do not say which byte is lost. The driver's answer to an overflow is to reset the whole receive FIFO, clear the bit and post `UART_FIFO_OVF` (`IDF:esp_driver_uart/src/uart.c:1365-1377`), so after it the content of the FIFO does not matter. **A**: the byte that finds 128 bytes in the FIFO is dropped and the 128 stay; `RXFIFO_CNT` does not exceed 128.

---

## 3. Who reads

### 3.1 The mask ROM (R+M)

| Function | What it does | Source |
|---|---|---|
| `uart_rx_one_char@0x4004c50e` | UART number from `UartDev+0x18` (`0x3fcdf07c`); 3 means USB-Serial-JTAG (`usb_uart_rx_one_char`). Otherwise reads `STATUS & 0x3FF`; 0 → returns 1 (no byte); else reads `FIFO`, stores the low byte through its argument, returns 0. No blocking, no interrupt | `ROM3:0x4004c50e-0x4004c544` |
| `uart_rx_one_char_block@0x4004c4d8` | same, but spins on `STATUS & 0x3FF` until non-zero | `ROM3:0x4004c4d8-0x4004c50c` |
| `uart_rx_intr_handler@0x4004c052` | if `INT_ST` bit 0: while `RXFIFO_CNT ≠ 0`, read `FIFO` into a 0x800-byte ring buffer, mark state 2 on `\r` (13); then `INT_CLR = 1`. Otherwise it reads one word at `(0x6001 − n) << 16` and returns | `ROM3:0x4004c052-0x4004c0bc` |
| `uartAttach@0x4004c0e4` | registers `uart_rx_intr_handler` (`0x4004c150`), `UART0.INT_CLR = 1`, `UART1.INT_CLR = 1`, masks CPU line 5 | `ROM3:0x4004c0e4`; BC §J row 4 (`esp32c3-boot-chain.md:421`) |
| `Uart_Init@0x4004c260` (UART0) | `CLK_CONF`, `CLKDIV`, `CONF0` with the `RXFIFO_RST`/`TXFIFO_RST` pulse (`0x0406001C` then `0x0400001C`), **`CONF1 = UartDev+0x28` = 1**, `INT_CLR = 0xFFFFFFFF`, **`INT_ENA |= 1`**, `UART_INTR_MAP = 5`, `CPU_INT_PRI_5` 0 then 1 | `ROM3:0x4004c260-0x4004c35c`; values M (trace P1, ROM segment) |
| Callers of the receive functions | `UartRxString`, `uart_rx_readbuff`, `recv_packet`, `UartGetCmdLn`, `UartConnCheck`, `UartDwnLdProc`: the download protocol, reached only when the boot strap selects download mode (`GPIO_STRAP` bit 3 clear) | `ROM3:rom.nm`, call sites `0x4004c586`, `0x4004c692`…; BC `esp32c3-boot-chain.md:219, 442` |

So during a normal boot the ROM leaves UART0 with `RXFIFO_FULL` enabled at threshold 1 and routed to line 5 at priority 1, but line 5 stays disabled through the ROM and the bootloader (IR §1.4, BC row 4), and the application's `core_intr_matrix_clear` sets `UART_INTR_MAP = 0` before it enables any line (S3: the write of 0 to `0x600C2054` comes before the first non-zero `CPU_INT_ENABLE`) **(M)**. In P1 no `FIFO` read occurs before the application's own (`p1.seg`, ROM and bootloader segments) **(M)**. Bytes that reach the FIFO before `Uart_Init`'s pulse are lost; bytes after it stay in the FIFO for the application.

### 3.2 ESP-IDF stdin without the driver (R+M)

The console is UART0 (`CONFIG_ESP_CONSOLE_UART=y`, `CONFIG_ESP_CONSOLE_UART_NUM=0` in both builds); the secondary USB-Serial-JTAG console "is only used for output" (`IDF:esp_vfs_console/vfs_console.c:26-29`, `61-62`). Until `uart_vfs_dev_use_driver` is called, `/dev/uart/0` uses `uart_rx_char` and `uart_get_avail_data_len` (`IDF:esp_driver_uart/src/uart_vfs.c:1124-1133`):

| Step | Code | Source |
|---|---|---|
| one character | `uart_rx_char`: `STATUS.RXFIFO_CNT == 0` → `NONE`; else one `FIFO` read | `uart_vfs.c:203-213` |
| available count | `STATUS.RXFIFO_CNT` | `uart_vfs.c:170-176` |
| `read(fd, buf, n)` | in blocking mode it first calls the (non-blocking) character function, adds the FIFO count, reads at most that many bytes, converts line endings, and returns `-1`/`EWOULDBLOCK` when it got nothing | `uart_vfs.c:273-344` |
| line endings | `\r` → `\n` in `CR` mode (default), `\r\n` → `\n` in `CRLF` mode (with one-byte look-ahead), none in `LF` mode | `uart_vfs.c:38-43, 302-326`; default `IDF:newlib/Kconfig:51-73` (`default LIBC_STDIN_LINE_ENDING_CR`) |

So `getchar()` never blocks on an empty FIFO: newlib sees `read` fail and returns `EOF`. P1 phase A had to loop on `EOF` with `clearerr(stdin)` and `vTaskDelay(1)`; it made 63 `STATUS` reads while waiting 0.3 s, then read `ab\r` as `STATUS=3`, `FIFO=61`, `STATUS=2` (twice), `FIFO=62`, `STATUS=1`, `FIFO=0d`, and printed `A line [ab] 2`. Input `x\r\ny` gave `120, 10, 10, 121` **(M)**. No interrupt register is touched on this path **(M)**.

### 3.3 The `uart` driver (R+M)

`uart_driver_install` (`IDF:esp_driver_uart/src/uart.c:1932`): `INT_ENA = 0` and `INT_CLR = 0x7FFFF` (`:1989-1990`), `esp_intr_alloc(ETS_UART0_INTR_SOURCE, …, uart_rx_intr_handler_default)` (`:1992-1994`), then `uart_intr_config` with `UART_INTR_CONFIG_FLAG`, full threshold `UART_FULL_THRESH_DEFAULT = 120`, timeout `UART_TOUT_THRESH_DEFAULT = 10` symbols (`:2027-2033`, `:67-73`, `:85-96`). `uart_intr_config` clears all, sets the timeout (or disables it), the full threshold, and ORs the mask into `INT_ENA` (`:1087-1106`). `uart_hal_set_rx_timeout` writes `symbol_length × tout` (`IDF:hal/uart_hal.c:152-156`), the symbol length being start + data + stop (1.5 counted as 2) + parity (`:137-150`): 10 × 10 = **100 bit times** in 8N1.

`uart_param_config` (`uart.c:991-1050`) adds: source clock and baud rate (`CLK_CONF`, `CLKDIV`), `uart_hal_init` (mode normal via `RS485_CONF` and `CONF0`, parity off, 8 bits, 1 stop bit, `TX_IDLE_NUM = 0`, flow control off), and the `RXFIFO_RST` then `TXFIFO_RST` pulses (`:1046-1050`).

The ISR loops while `INT_ST ≠ 0` (`uart.c:1145-1160`). For `RXFIFO_TOUT` or `RXFIFO_FULL` it reads `RXFIFO_CNT` bytes (one fewer with `rx_always_timeout`, "leave one byte in the fifo in order to trigger uart_intr_rxfifo_tout"), reads `AT_CMD_CHAR`, clears `TOUT | FULL`, and pushes the bytes into the ring buffer (`:1265-1300`); a full ring buffer disables both bits (`:1305, 1355-1357`). `RXFIFO_OVF`: FIFO reset (`:1365-1377`). `BRK_DET`, `FRM_ERR`, `PARITY_ERR`: clear and post an event (`:1378-1397`). `TXFIFO_EMPTY`: disable, clear, give the transmit semaphore (`:1166-1185`). `TX_DONE`: clear, disable (`:1434-1445`).

**Transmit with the driver**: with no transmit buffer, `uart_write_bytes` writes what fits in the FIFO and, if anything is left, enables `TXFIFO_EMPTY` at threshold 10 and waits on a semaphore that only the ISR gives (`uart.c:1597-1606`, `686-700`). Once a firmware has called `uart_vfs_dev_use_driver`, every `printf` and log character goes this way (`uart_vfs.c:197-201`). **So a model with timed transmission must raise `TXFIFO_EMPTY`, or the first burst of output larger than the free room in the FIFO blocks the task for ever** (A, from the code; QEMU never exercises it because its transmission is instantaneous).

Measured sequences (S2; UART0; `R`/`W`, offset, value; `STATUS` reads of the transmit loop omitted) **(M)**:

| Phase | Accesses |
|---|---|
| install | `R 0C 1`, `W 0C 0`, `W 10 7FFFF` ×2, `R 20` ×14 (symbol length, mode), `R/W 60` → `00640000` (timeout 100), `R/W 24` → `00200001` then `00200078` (timeout on, threshold 120), `R 0C 0`, `W 0C 00080195` |
| `uart_param_config` | `R/W 4C 0` ×3, `W 20 0400001C` ×11, `R/W 48 0` ×2, `R/W 78` (clock), `W 14 003002B6` then `007002B6` (APB 80 MHz: 694 + 7/16), `W 20 0402001C` / `0400001C` (RX reset pulse), `W 20 0404001C` / `0400001C` (TX reset pulse) |
| ISR, `a` | `R 08 100`, `R 1C 1`, `R 00 61`, `R 5C 0` ×2, `W 10 101`, `R 08 0` |
| ISR, `bc\r\n` | `R 08 100`, `R 1C 4`, `R 00` ×4 (`62 63 0d 0a`), `R 5C 0` ×2, `W 10 101`, `R 08 0` |

Interrupt routing in S3: `UART_INTR_MAP = 7`, `CPU_INT_PRI_7 = 1`, `CPU_INT_ENABLE` bit 7 set (`0x0F0000BC`) **(M)**. The line is chosen by `esp_intr_alloc` at run time; nothing pins 7.

### 3.4 The console REPL and linenoise (R)

`esp_console_new_repl_uart` installs the driver with a 256-byte receive buffer, sets `CR` line endings and switches the VFS to the driver (`IDF:console/esp_console_repl_chip.c:204, 235, 241`). `linenoiseProbe` switches stdin to `O_NONBLOCK`, prints `ESC [5n` and reads for at most 500 ms in 10 ms steps, expecting `ESC [0n` (`IDF:console/linenoise/linenoise.c:1100-1140`); without an answer the REPL falls back to "dumb" mode. A scripted host can answer or stay silent: either is deterministic in device time.

---

## 4. The interrupt

| Fact | Tag | Source |
|---|---|---|
| `ETS_UART0_INTR_SOURCE` and `ETS_UART1_INTR_SOURCE` are entries 21 and 22 of the source table, "level" | R | `IDF:soc/esp32c3/include/soc/interrupts.h:16-40` |
| Their map registers are `UART_INTR_MAP` (`0x600C2054`, offset 0x54 = 21 × 4) and `UART1_INTR_MAP` (`0x600C2058`) | M | `SVD:` INTERRUPT_CORE0 |
| `INT_ST` bit n = `INT_RAW` bit n and `INT_ENA` bit n ("This is the status bit for … when … ena is set to 1") | R | `uart_struct.h:48-67` |
| `INT_CLR`: write-only, a 1 clears the raw bit ("WT"); the driver writes the exact mask it handled | R | `uart_reg.h:422-530`; `uart_ll_clr_intsts_mask`, `uart_ll.h:318-321` |
| The source into the matrix is the OR of `INT_ST`; it stays high until software clears the raw bits or disables them | A (the usual Espressif structure; the driver's loop "while `INT_ST ≠ 0`" relies on it) | `uart.c:1153-1160` |

inkemu today: the UART stores `INT_ENA`, accepts `INT_CLR` with no effect, and does not serve `INT_RAW`, `INT_ST` or a read of `FIFO` (`INK:periph/uart.rs:14-31, 267-285`; its test pins those `None`, `:477-493`). The two existing level sources are written the same way and can be copied:

- SYSTIMER: `fn drive(&self, ctx)` calls `ctx.irq(source, int_raw & int_ena & bit != 0)` for each comparator, after an alarm, a write of `INT_ENA` and a write of `INT_CLR` (`INK:periph/systimer.rs:291-296, 415-422`); `INT_ST` reads `int_raw & int_ena` (`:352`).
- Timer groups: `ctx.irq(t0, status & INT_T0 != 0)` (`INK:periph/timg.rs:601-606`).
- The sources are `const` items built by `Source::of("<NAME>_MAP")` (`INK:ctx.rs:23-52`); the UART's are `Source::of("UART_INTR_MAP")` and `Source::of("UART1_INTR_MAP")`. `Ctx::irq` only flips a bit and marks the interrupts changed (`INK:ctx.rs:384-394`); the controller samples between instructions (`INK:periph/intmatrix.rs:17-24`, ADR 0003 §5).

A UART that receives while the CPU is in `wfi` must ask to be called at the instant a raw bit becomes true (a byte that crosses the threshold, the end of a timeout), like the transmitter already asks for the instant its last byte has left (`INK:periph/uart.rs:152-165`, `Event::Uart(n)`, `INK:periph/mod.rs:505-506`). One `Deadline` per UART is enough if it is armed at the earliest of "next transmit settle", "next receive event".

---

## 5. Timing

### 5.1 Bit time and frame (R)

- Bit time = `(CLKDIV.DIV_INT + CLKDIV.DIV_FRAG / 16) × (CLK_CONF.SCLK_DIV_NUM + 1)` periods of the source clock (`uart_ll_set_baudrate`/`get_baudrate`, `uart_ll.h:223-258`). The same divider times transmission and reception (one `CLKDIV` per UART). inkemu computes it for transmission already (`INK:periph/uart.rs:205-222`).
- Frame = start bit + `5 + BIT_NUM` data bits + `PARITY_EN` parity bit + stop bits (`STOP_BIT_NUM` 1 → 1, 2 → 1.5, 3 → 2). Sources: `uart_ll_set_data_bit_num` writes `UART_DATA_5_BITS = 0` … `UART_DATA_8_BITS = 3` (`uart_ll.h:650-653`, `IDF:hal/include/hal/uart_types.h:57-60`); `uart_ll_set_stop_bits` writes `UART_STOP_BITS_1 = 1`, `_1_5 = 2`, `_2 = 3` (`uart_ll.h:428-431`, `uart_types.h:68-70`); `uart_ll_set_parity` sets `PARITY_EN` and `PARITY` (`uart_ll.h:454-460`). `RS485_CONF.DL0_EN` and `DL1_EN` each "delay the stop bit by 1 bit" (`uart_struct.h:275-276`).
- At the measured settings: the ROM's 40 MHz crystal, `CLKDIV = 347 + 3/16`: 8.680 µs per bit; the driver's APB 80 MHz, `694 + 7/16`: 8.681 µs per bit. 8N1: **86.8 µs per byte** at either.

### 5.2 When a received byte lands, and the timeout

- A byte enters the FIFO when its frame has been received: one frame time after its start bit (A; the chip may store it when it samples the stop bit, half a bit earlier; nothing measured tells them apart).
- `RXFIFO_FULL` rises when `RXFIFO_CNT` becomes greater than `RXFIFO_FULL_THRHD` ("receives more data than", `uart_struct.h:191`; A1 for "greater" against "greater or equal").
- `RX_TOUT_THRHD` counts **bit times**: ESP-IDF documents its parameter in symbols ("tout_thresh = 1 … transmission time of one symbol (~11 bit)", `IDF:esp_driver_uart/include/driver/uart.h:749-762`) and writes symbols × bits per symbol (§3.3). The reset value 10 is one 8N1 symbol. Under QEMU, with `RX_TOUT_THRHD = 100` at 115 200 baud (868 µs), `RXFIFO_TOUT` rose 810 µs after the probe first saw the ten bytes, which had arrived a little earlier (P1 phase C) **(M)**.
- The timeout counts idle line time since the last byte **while the FIFO holds data** (A; ESP-IDF keeps one byte in the FIFO "in order to trigger uart_intr_rxfifo_tout", `uart.c:1275-1277`, and `uart.h:857-861`). Whether a read of the FIFO restarts the count is not documented (A3).

### 5.3 What software observes

- The driver hands data over in chunks decided by the two interrupts: a burst of fewer than 121 bytes arrives as one event when the timeout fires (100 bit times after its last byte); a longer burst arrives as ~121 bytes at the threshold, then the rest at the timeout (P1 phase B under QEMU gave 5 bytes `tout=1`, then 128 `tout=0` and 22 `tout=1`, because QEMU delivers 128 bytes at once) **(M+R)**.
- `uart_read_bytes(…, 20 ms)` loops (`uart_echo`), `select()` timeouts (5 s in `uart_select`), the 500 ms `linenoiseProbe`, and a polling reader with `vTaskDelay(1)` all react to the device time at which bytes land.

### 5.4 What a deterministic model needs

1. The host's input is a list of bytes per UART, each with the device time at which the host starts sending it (ADR 0004 already plans such dated inputs: "les entrées que le firmware ne peut pas encore recevoir … datées en temps d'appareil", carry-over note SP1b-3c).
2. Bytes go out back to back: byte k lands at `max(t_k, land_{k−1}) + frame_time`, the frame time computed from `CLKDIV`, `CLK_CONF`, `CONF0` at the instant the byte starts (the transmitter's rule, `INK:periph/uart.rs:231-237`).
3. The FIFO count at any instant is the landed bytes minus the bytes read: computed when a register is accessed (`settle`, as for transmission), not cycle by cycle (ADR 0003 §4).
4. The UART asks to be called at the next instant a raw bit can change: the landing of byte number `RXFIFO_FULL_THRHD + 1`, the landing of byte 129 (overflow), the end of the timeout. Bytes that change nothing need no call.
5. A byte that lands while `CLK_CONF.RX_SCLK_EN` (bit 25) is clear, or before `Uart_Init`, is lost (A); a `RXFIFO_RST` pulse or a module reset empties the FIFO and cancels pending interrupt deadlines.

---

## 6. Frame format on transmit

The model sends every byte in 10 bit times (`INK:periph/uart.rs:80-85`, `BITS_PER_BYTE`). With §5.1, the duration of a transmitted byte is

```
bits = 1 + (5 + CONF0.BIT_NUM) + CONF0.PARITY_EN + stop(CONF0.STOP_BIT_NUM) [+ RS485_CONF.DL0_EN + DL1_EN]
stop(1) = 1, stop(2) = 1.5, stop(3) = 2
byte_time = bits × (DIV_INT + DIV_FRAG/16) × (SCLK_DIV_NUM + 1) × source_period
```

So yes, `CONF0` changes the duration: 8N1 = 10 (the ROM's `0x0400001C` and the driver's), 7E1 = 10, 8E1 = 11, 8N2 = 11, 8E2 = 12, 5N1 = 7, 8N1.5 = 10.5 (a half bit: the existing sixteenths of a cycle cannot hold every case; use thirty-seconds or keep bit counts in halves). `STOP_BIT_NUM = 0` has no documented meaning (A4). Every console measured so far is 8N1 (ROM `CONF0 = 0x0400001C`, `uart_hal_init` sets 8 bits, 1 stop, no parity, `IDF:hal/uart_hal.c:121-135`) **(M+R)**, so the fixed 10 is right on every measured path.

Not covered by the formula: `IDLE_CONF.TX_IDLE_NUM` ("the duration time between transfers", `uart_struct.h:267`) resets to 256 bit times; the driver sets it to 0 (`uart.c:1046`, `UART_TX_IDLE_NUM_DEFAULT = 0`), but the ROM, the bootloader and a firmware without the driver never write `IDLE_CONF` (P1, S2: only the driver path touches offset `0x48`) **(M)**. If the chip inserted 256 idle bits between transfers, the ROM banner would be visibly slow; when it applies is not documented (A5, the riskiest timing assumption of this report).

---

## 7. QEMU as an oracle

### 7.1 Feeding bytes (M)

| Question | Answer | Tag |
|---|---|---|
| Can a host feed UART0? | Yes: `-serial stdio` (used here), `-chardev pipe`, `socket`, `pty`, and `file` with `input-path=` (the parser accepts it; a bogus option is refused) | M |
| Deterministic content? | Yes: S2 and S3 give the same console modulo timestamps | M |
| Deterministic device time? | No. Bytes enter when QEMU's main loop reads them, at host pace: `Received: a` was logged at 932 ms in S2 and 982 ms in S3 for the same script. `-icount` does not tie host input to virtual time | M |
| Line rate? | No: ten bytes sent in one write were all in the FIFO at the probe's first observation (`cnt=10` at t = 0) | M |
| A way to make one run replayable | QEMU's record/replay (`-icount …,rr=record`) records character-device input | A (not tried) |

### 7.2 What its UART model implements (M)

The binary names `hw/char/esp32_uart.c` and `hw/char/esp32c3_uart.c`, functions `esp32_uart_set_rx_timeout`, `esp32_uart_update_irq`, and register fields `RXFIFO_FULL`, `TXFIFO_EMPTY`, `TX_DONE`, `RXFIFO_TOUT` (raw), `CONF1` thresholds, `RX_TOUT_EN`, `MEM_CONF.RX_TOUT_THRHD`, `STATUS` counts, `MEM_RX_STATUS.WR_ADDR`. No string names `RXFIFO_OVF`, `FRM_ERR`, `PARITY_ERR`, `BRK_DET` or `GLITCH_DET`.

| Behaviour (P1 unless stated) | QEMU | Chip (A unless R) |
|---|---|---|
| `RXFIFO_FULL` after `INT_CLR` while the FIFO is still above the threshold | stays set (`RAW 00004103` → `00004003`) | unknown (A1) |
| `RXFIFO_FULL` after draining, without a clear | gone (`00004002`) | unknown (A1) |
| `RXFIFO_TOUT` | set ~one timeout after the bytes, stays until cleared | same (R: "R/WTC/SS") |
| 200 bytes sent to a FIFO nobody reads | 128 in the FIFO, the other 72 delivered as room appears: 200 read in total, `RXFIFO_OVF` never set | 72 lost, `RXFIFO_OVF` set (A2) |
| Timeout configured with an empty FIFO | **raises `RXFIFO_TOUT`**: right after `uart_driver_install`, the ISR saw `INT_ST = 0x100` with `RXFIFO_CNT = 0` (P1, S1, S2, S3) | no timeout without data (A, §5.2) |
| `STATUS` idle | `0x00000000` | `0xE000C000` (SVD; line bits) |
| `INT_RAW` idle | `0x00004002` (`TX_DONE` and `TXFIFO_EMPTY`) | `0x00000002` at reset (SVD) |
| `MEM_CONF` read back | 0 before the write; `0x00014000` after writing `0x00640000` | the written value (A) |
| `MEM_RX_STATUS` | 0 | `0x00080100` at reset |
| Transmission | instantaneous: the driver never waits for room, `TXFIFO_EMPTY` is never enabled in S2 | timed (inkemu models it) |

Consequence of the spurious timeout: in S1, `uart_select`'s first `select()` returned "readable" at once, its `read()` then blocked for want of data, and the expected first line `Timeout has been reached and nothing has been received` never appeared in 150 s; sending `a` unblocked it (S2) **(M)**. So QEMU cannot run the example's own test as written (§9), and a lockstep against QEMU must expect one extra `RXFIFO_TOUT` after each timeout configuration.

---

## 8. CrossPoint (R)

CrossPoint's C3 build (`board = esp32-c3-devkitm-1`) sets `-DARDUINO_USB_MODE=1 -DARDUINO_USB_CDC_ON_BOOT=1` (`CP:platformio.ini:11, 39-40`), so `logSerial` is the `HWCDC` USB-Serial-JTAG port (`CP:lib/Logging/Logging.h:34-39`); its only serial input, the `CMD:` lines of the main loop (`CP:src/main.cpp:727-735`, e.g. `CMD:SCREENSHOT`), comes from that port. No file of `src/` or `lib/` uses `Serial0`, `Serial1`, `HardwareSerial` reads or `uart_read_bytes`; the other log transport, `esp_rom_printf` to UART0, only writes, and its comment ties it to the "Sticky" board's UART0 bridge (`CP:lib/Logging/Logging.cpp:65-67`). Under QEMU, UART0 carries only ROM lines, Arduino-core errors and panics (FW §K.2). On the X4 the UART0 receive pin GPIO20 is the USB/VBUS detect input ("reads HIGH when USB present"), and on the X3 it is I2C SDA (`XH:38, 60, 108`): host bytes on UART0 have no meaning for CrossPoint. If the pin reads low without USB (A, XH gives only the high level), the receiver of a firmware that keeps UART0 on GPIO20 would see a line held low, a continuous break; that belongs with the GPIO and pin model, not SP1b-3d.

---

## 9. Reference firmware for a CI test of reception

### 9.1 Choice: `EX:peripherals/uart/uart_select`, unmodified

| Property | Evidence |
|---|---|
| Stock, `CC0`, `MINIMAL_BUILD ON`, `REQUIRES esp_driver_uart vfs`, no `sdkconfig.defaults`, no `idf_component.yml` (so `tools/reference-firmware/in-container.sh` accepts it and needs no network) | `EX:peripherals/uart/uart_select/{CMakeLists.txt,main/CMakeLists.txt}`, directory listing (R) |
| UART0 at 115 200 8N1, driver with a 2 KiB receive buffer, `select()` on `/dev/uart/0` with a 5 s timeout, `read(fd, &c, 1)`, `ESP_LOGI(TAG, "Received: %c", c)` | `EX:…/main/uart_select_example_main.c` (R) |
| Built here with the six-line reference `sdkconfig.defaults` and nothing else: 62 s; `bootloader.bin` = `601b4845…afaa3` and `partition-table.bin` = `7f00b6c0…7c820`, the pinned files; `uart_select.bin` `879fed8f…4b48` (184,448 bytes; one build, reproducibility not measured) | M |
| Espressif's test: expect `uart_select_example: Timeout has been reached and nothing has been received`, write `a`, expect `uart_select_example: Received: a` | `EX:…/pytest_uart_select.py:9-13` (R) |
| It exercises: driver install, interrupt routing (any line), `RXFIFO_TOUT` (short bursts), `select()`, the VFS read in driver mode with `CR` conversion, FreeRTOS timeouts in device time, and, under a timed transmitter, `TXFIFO_EMPTY`: the four lines logged for `bc\r\n` total about 174 bytes, more than the 128-byte FIFO | R + arithmetic |

Why not the others: `system/console/basic` ships its own `sdkconfig.defaults` and a managed component (refused by the build script); `uart_echo` defaults to UART1 on GPIO 4/5 and needs three Kconfig changes to use UART0, and its 20 ms read windows make chunking depend on timing; `uart_repl`, `uart_events`, `nmea0183_parser` need a second UART or a peer. A non-stock "getchar echo" (§12 probe phase A) would cover the polling path, which `uart_select` does not; recommend it only if the plan wants both paths.

### 9.2 Expected console (M under QEMU, lines after `main_task: Returned from app_main()`, `\r\n` endings; timestamps `#` vary under QEMU and are fixed under inkemu for a fixed input schedule)

Input `a`, then 0.5 s later `bc\r\n` (S2, S3):

```
I (#) uart_select_example: Received: a
I (#) uart_select_example: Received: b
I (#) uart_select_example: Received: c
I (#) uart_select_example: Received: 

I (#) uart_select_example: Received: 

I (#) uart_select_example: Timeout has been reached and nothing has been received
```

Both `\r` and `\n` print as a newline (the `\r` is converted to `\n` by the `CR` mode, then each `\n` goes out as `\r\n`), so each gives `Received: ` followed by an empty line. On a chip, and on a model without QEMU's spurious timeout, the console first shows the timeout line every 5 s of device time from the first `select()` until the first byte; the input `a` can then be sent after that line, as Espressif's test does. Recommended assertions for SP1b-3d: (1) the timeout line appears 5 s of device time after `app_main` returns, with no input; (2) `a` sent after it gives exactly `Received: a`; (3) `bc\r\n` in one burst gives the four lines above in order, which also proves that the transmitter's `TXFIFO_EMPTY` wakes the writer; (4) `--strict-mmio` runs without a refusal.

---

## 10. Recommendations for SP1b-3d

**Registers (per UART, offsets as UART0).** Model: `FIFO` read (pop, low byte; reading an empty FIFO: return 0 and report it by name, A6), `STATUS.RXFIFO_CNT`, `INT_RAW`/`INT_ST`/`INT_ENA`/`INT_CLR` (bits 0, 1, 4, 8, 14 with effect; the others stored, never set), `CONF0` (`RXFIFO_RST` pulse, frame format; `LOOPBACK`, `RXD_INV`, `AUTOBAUD_EN` refused by name when set), `CONF1` (`RXFIFO_FULL_THRHD`, `TXFIFO_EMPTY_THRHD`, `DIS_RX_DAT_OVF`, `RX_TOUT_EN`), `MEM_CONF` (`RX_TOUT_THRHD`; sizes other than 1 block refused). Store as plain registers, because the driver reads and writes them: `IDLE_CONF`, `RS485_CONF` (value 0; non-zero refused), `AT_CMD_CHAR` (read by every ISR pass). Serve the reset value of `MEM_RX_STATUS` and keep `FSM_STATUS.ST_URX_OUT` at 0: nothing reads them. The eight table rows deferred to SP1b-3d (`INT_ENA`, `INT_CLR`, `CONF0`, `CONF1` of both UARTs, `INK:periph/uart.rs:15-31`, `docs/superpowers/notes/2026-10-11-sp1b-3a-carryover.md:15`) become modelled registers; `MEM_CONF`, `IDLE_CONF`, `RS485_CONF`, `AT_CMD_CHAR`, `INT_RAW`, `INT_ST` are new.

**Interrupt bits.** `RXFIFO_FULL` (count > threshold), `RXFIFO_TOUT` (timeout with data), `RXFIFO_OVF` (byte 129), `TXFIFO_EMPTY` (transmit count < threshold; reset value 1), `TX_DONE` (transmitter goes idle). Drive `Source::of("UART_INTR_MAP")` / `Source::of("UART1_INTR_MAP")` as `raw & ena != 0` after every change, like `systimer.rs:291-296`. Pick one rule for conditions that still hold after `INT_CLR` (A1) and write it once; the ESP-IDF driver works with either (it drains before clearing `RXFIFO_FULL` and disables `TXFIFO_EMPTY` before clearing it), the ROM download ISR too.

**Timing.** Frame time from §6 for both directions (replace `BITS_PER_BYTE`; keep half bits for 1.5 stop bits); receive as §5.4: dated host bytes, sent back to back, landing one frame later, the FIFO count computed lazily, one call per instant at which a raw bit changes. No timeout with an empty FIFO. Overflow drops the byte and sets the bit (named diagnostic, not a stop under the lenient default). Keep transmitting bytes with the duration they were given (current rule).

**Host input and CLI.**
- Inputs: a new run input per ADR 0004, "bytes for UART n at device time t". Two producers: a script (literal bytes with escapes; a step "after the console printed TEXT, wait Δ, send BYTES" is resolved during the run into device times, which stays deterministic because the console is), and an interactive terminal (bytes stamped with the device time at which the machine takes them; not reproducible unless recorded, like Ctrl-C today).
- Send `\r` for Enter (ESP-IDF's default stdin mode maps it to `\n`, `linenoise` expects it); pass bytes raw, never translate on the host.
- A script that sends before the firmware listens loses bytes on a chip too (`Uart_Init`'s FIFO reset, the driver's `RXFIFO_RST`); the CLI should not hide it, but a "wait for text" step avoids it.
- No QEMU-style back-pressure: a paste larger than the FIFO overflows a firmware that does not read fast enough, as on a chip. A diagnostic that counts dropped bytes keeps that visible.
- The relay that prints UART0 (`ConsoleChannel`, `INK:` via `emu-cli/src/run.rs:178`) gains an input side per channel; the same input type serves USB-Serial-JTAG (other report) and the GUI of SP2.

**Test firmware.** Add `peripherals/uart/uart_select` to `tools/reference-firmware/in-container.sh` as built (no extra option), pin its three hashes after two clean builds, merge it like `flash_hello.bin`, and assert §9.2 (1)–(4) under inkemu. Do not use QEMU's console of this example as a golden file: its first timeout line is missing (§7.2). Optionally a second, non-stock firmware for the polling stdin path (§12 probe phase A, 20 lines).

**Lockstep against QEMU (SP1b-4).** Expected differences to whitelist: one `RXFIFO_TOUT` after each timeout configuration, bytes landing all at once, no overflow, `STATUS`/`INT_RAW`/`MEM_CONF`/`MEM_RX_STATUS` read-backs (§7.2), and QEMU's input time being host-dependent: drive both from the same recorded input or compare only content.

## 11. Risky assumptions

| # | Assumption | Why it matters | What settles it |
|---|---|---|---|
| A1 | Whether `RXFIFO_FULL` and `TXFIFO_EMPTY` are set again right after `INT_CLR` while their condition holds (level) or only on the next crossing (edge); and "count > threshold" rather than "≥" | an interrupt storm or a missed interrupt in firmware that clears without draining; off-by-one in the byte count at which the driver's ISR runs | a chip: clear with the FIFO above the threshold and read `INT_RAW`; send exactly `THRHD` bytes |
| A2 | On overflow the new byte is dropped, the FIFO keeps its 128 bytes, `RXFIFO_CNT` stays 128 | what a too-fast paste leaves in a polling reader | a chip: send 200 bytes to an unread FIFO, read it out |
| A3 | `RXFIFO_TOUT` needs a non-empty FIFO and an idle line for `RX_TOUT_THRHD` bit times after the last landed byte; reading the FIFO does not restart the count; it fires once per idle period | when the driver's last chunk arrives; QEMU disagrees (spurious timeout) | a chip with a logic analyser on RX and a GPIO toggled in the ISR |
| A4 | `STOP_BIT_NUM = 0` and `BIT_NUM` beyond 3 have no defined frame | a firmware writing them | TRM register description or a chip |
| A5 | `IDLE_CONF.TX_IDLE_NUM` (256 at reset, never written by the ROM, the bootloader or a driver-less application) does not slow back-to-back transmission | every transmit duration of the ROM, the bootloader and `hello_world` | a capture of the ROM banner's byte spacing on a chip |
| A6 | A read of an empty receive FIFO returns 0 and changes nothing | only buggy firmware does it; the model reports it | a chip |
| A7 | A byte lands one full frame after its start bit (not at the stop-bit sample, half a bit earlier) | sub-bit timing of `RXFIFO_FULL`/`RXFIFO_TOUT` | a chip with a logic analyser |
| A8 | A byte that arrives while `CLK_CONF.RX_SCLK_EN` is clear, or during a `RXFIFO_RST` pulse, is lost | input sent during boot | a chip |
| A9 | QEMU's spurious timeout comes from the timeout configuration itself (it appears right after `uart_driver_install` writes `MEM_CONF`/`CONF1`, with an empty FIFO, in four runs) | the lockstep whitelist | reading `hw/char/esp32_uart.c` |
| A10 | The driver deadlocks without `TXFIFO_EMPTY` once a timed transmit FIFO fills | whether SP1b-3d can postpone transmit interrupts | running `uart_select` with input `bc\r\n` on inkemu |

## 12. Artefacts and reproduction (scratch, not in the repository)

Directory `…/scratchpad/r3d/q/`: `src/uart_rx_probe/` (the probe: phase A stdin without the driver, phase C registers by hand with threshold 4 and timeout 100, phase C5 200 unread bytes, phase B the driver with an event queue, then `fgets` through the driver), `tools/` (`build.sh`, `drive.py`, `uartseg.py`, `probe.script`, `select.script`, `select2.script`), `img/` (merged 16 MB images), `out/` (builds, UART captures, event logs, UART-only traces and their segment summaries). One container for the whole session, `--network none`, removed at the end: `docker run -d --name inkemu-r3d --network none --entrypoint sleep -v …/q/tools:/w:ro -v …/q/tools:/tools:ro -v …/q/src:/src:ro -v …/q/img:/img -v …/q/out:/out -v <repo>/tools/reference-firmware:/ref:ro espressif/idf:v5.5.2 infinity`.

| Step | Command (`docker exec inkemu-r3d bash …`) |
|---|---|
| Builds | `/w/build.sh /src/uart_rx_probe uart_rx_probe probe`; `/w/build.sh /opt/esp/idf/examples/peripherals/uart/uart_select uart_select select` (copy, reference `sdkconfig.defaults`, `idf.py set-target esp32c3`, `idf.py build`, `esptool.py merge_bin` as the reference script) |
| P1 | `python3 -I /tools/drive.py /img/probe.bin p1 /tools/probe.script 1` (QEMU `-machine esp32c3 -icount 3 -display none -serial stdio -monitor none -trace 'memory_region_ops_*'`) |
| S1, S2, S3 | `… /img/select.bin s1 /tools/select.script 1`; `… s2 /tools/select2.script 1`; `… s3 /tools/select2.script 1` |
| Reduce (host) | `grep "uart'" <tag>.trace > <tag>.uarttrace`; `python3 -I tools/uartseg.py out/<tag>.uarttrace` (UART accesses grouped by the last line the firmware printed) |

Checksums (SHA-256, abbreviated): `uart_select.bin` `879fed8fb43f…4b48`, `img/select.bin` `64a73c8c4160…202f`, `uart_rx_probe.bin` `2adac4bc68f9…0d9c`, `img/probe.bin` `99be58fac04c…fc91`, `main.c` of the probe `5cef2774008e…7a32`, `out/p1.uart` `92dc85e4de4c…2098`, `out/s1.uart` `e0831c079496…98d0`, `out/s2.uart` `7f64c48d039d…a23b`, `out/s3.uart` `5b58e5a9f5fa…6f5a`. The raw traces (0.5 GB for P1) were deleted after reduction; the UART-only extracts remain.
