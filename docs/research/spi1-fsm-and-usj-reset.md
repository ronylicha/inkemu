# SPI1 offset 0x54 and the USB-Serial-JTAG tail across a core reset

Date: 2026-10-14. Two questions raised by the stock X4 firmware (ESP-IDF v4.4.7) under inkemu, and answered
for SP1b-3e (`docs/superpowers/specs/2026-10-14-sp1b-3e-design.md`, decisions E3 and E4):

- **A.** 738 reads of `0x60002054` (SPI1, offset `0x54`) were reported as "no register is documented
  there". What register is it, who reads it, and what does a faithful model return?
- **B.** The last 18 bytes of a v4.4.7 panic (`\r\n\r\nRebooting...\r\n`) appear on UART0 and are missing
  from USB-Serial-JTAG. Is the loss what the chip does, or a defect of inkemu?

Tags: **M** = measured or read in a file (path or upstream file named); **R** = reasoned. Upstream ESP-IDF
files are cited by tag, file and symbol (`https://raw.githubusercontent.com/espressif/esp-idf/<tag>/components/...`);
the fetch gave no line numbers for most of them. Repository files are cited as `path:line` as of commit
5e0e146. TRM = ESP32-C3 Technical Reference Manual v1.4.

## A. SPI1 offset 0x54: `SPI_MEM_FSM_REG`

### A.1 Which register

**`SPI_MEM_FSM_REG(i) = REG_SPI_MEM_BASE(i) + 0x054`**, one macro for SPI0 and SPI1 (`(i)` selects the
controller). **M**: `soc/esp32c3/include/soc/spi_mem_reg.h` @v4.4.7 and
`soc/esp32c3/register/soc/spi_mem_reg.h` @v5.5.2 (identical text; the header moved to `register/soc/` in v5).

| Field (reg.h name / struct.h name) | Bits | Reset | Access | Meaning |
|---|---|---|---|---|
| `SPI_MEM_CSPI_ST` / `spi0_st` | [3:0] | 0 | RO | "current status of SPI0 slave FSM": 0 idle, 1 prep, 2 cmd, 3 addr, 4 wait, 5 read, 6 write, 7 done, 8 read end |
| `SPI_MEM_EM_ST` / `spi0_mst_st` | [6:4] | 0 | RO | "current status of SPI0 master FSM": 0 idle, 1 EM_CACHE_GRANT, 2 P/E suspend, 3 SPI0 read data, 4 wait cache/EDMA data in TX FIFO, 5 SPI0 write data |
| `SPI_MEM_CSPI_LOCK_DELAY_TIME` / `cspi_lock_delay_time` | [11:7] | 4 | R/W | lock delay of the SPI0/1 arbiter after a resume (PER) is sent by SPI1 |
| reserved | [31:12] | 0 | – | |

Reset value of the word: 4 << 7 = **`0x0000_0200`**. **M**: SVD `third_party/espressif-svd/esp32c3.svd:24400-24432`
(`resetValue 0x00000200`), `crates/svd-regs/src/esp32c3.rs:92`. **M**: `spi_mem_struct.h` places `fsm` after
`reserved_40..reserved_50` and before `data_buf[16]` at `0x58`, so the offset is confirmed by the struct too.

The three sources name the same bits differently (reg.h `EM_ST`/`CSPI_ST`, struct.h `spi0_mst_st`/`spi0_st`,
SVD `EM_ST`/`CSPI_ST`), and all three say "SPI0": the state machine is that of SPI0 (the cache side), not the
state of an SPI1 transfer, which lives in `CMD` [7:0] (`MST_ST` [3:0], `SLV_ST`/`MSPI_ST` [7:4]). **M**: reg.h,
SVD `esp32c3.svd:24632-24646`.

What the repository had at 5e0e146:
- The SVD lists `FSM` for **SPI0 only** (`esp32c3.svd:24400`, inside SPI0, which starts at `:23978`; SPI1 starts at
  `:24617`, has no `FSM`, and its address block is `0xA8` long). `crates/svd-regs` therefore has no
  `("SPI1", "FSM")`, and `Spi1::read` fell through to `self.registers.read(offset)`, which answers `None` for an
  offset it does not store: the bus returned 0 and warned "no register is documented there". **M**. It is an
  omission of the SVD, not another register: ESP-IDF defines `FSM` once for both controllers. **R** (same IP
  block `spi_mem`); the value read through the SPI1 alias on silicon is not measured.
- `crates/soc-esp32c3/src/periph/spi_mem.rs:178` (`SPI0_FSM`), `:209-218` (`fsm_idle`, the SVD reset value
  `0x200`, state bits clear) and `:230-233` (returned on read) model the SPI0 copy. **M**.
- `docs/research/esp32c3-boot-chain.md:199` lists `FSM 0x54` in the SPI0/1 layout (correct); `:206` and
  `esp32c3-rom-flash-boot.md:257` describe the ROM's `Wait_SPI_Idle`, which polls SPI0 `FSM & 0x70 == 0` and
  SPI1 `CMD & 0xF == 0`. **M**.

The TRM was not read for this register (PDF not reachable by the fetch tool); the ESP-IDF reg.h comments are
generated from the TRM's register descriptions, with the same wording. **R**.

### A.2 Who reads it

**ESP-IDF v4.4.7**, `hal/esp32c3/include/hal/spimem_flash_ll.h` (**M**):

```c
static inline bool spimem_flash_ll_host_idle(const spi_mem_dev_t *dev)
{
    return dev->fsm.spi0_mst_st == 0;
}
```

It is the only reader of `fsm` in that header. On the C3, `spi_flash_ll_host_idle(dev)` maps to it (**M**:
v4.4.7 `hal/esp32c3/include/hal/spi_flash_ll.h`, the branch without `GPSPI_BUILD`), and it is called by
`spi_flash_hal_check_status()` (**M**: v4.4.7 `hal/spi_flash_hal_iram.c`), which returns the driver's
`host_status` (0 busy, 1 idle, 2 suspended), polled by `spi_flash_chip_generic_wait_idle` before it reads the
status register of the chip (**M**: v4.4.7 `spi_flash/spi_flash_chip_generic.c`). The firmware's
`pc 0x4038e89c` in IRAM is therefore `spi_flash_hal_check_status` with `host_idle` inlined, and the 738 reads
come from as many waits after a program, an erase or a status read. **R** (not checked against the firmware's
symbols).

- Idle is **bits [6:4] = 0**; the rest of the word is ignored. A read of 0 makes `host_idle` true, so the
  answer inkemu gave (0, with a warning) was already right for the firmware: only the warning and the missing
  documentation were wrong. **R**.
- v4.4.7 reads the SPI0 master field through the SPI1 device (`dev` = `&SPIMEM1` = `0x60002000` when the chip
  is on SPI1): the read lands at SPI1 + `0x54`. **R**.
- **ESP-IDF v5.5.2** (**M**: `hal/esp32c3/include/hal/spimem_flash_ll.h`): `spimem_flash_ll_host_idle` is
  `return dev->cmd.mst_st == 0;`, i.e. `CMD` [3:0], and no function of that header reads `fsm` any more. The
  only access left is a write, `SPIMEM0.fsm.cspi_lock_delay_time = lock_time;` in
  `spimem_flash_ll_sus_set_spi0_lock_trans`, on SPI0. So **only v4.4.x firmware reads SPI1 + 0x54**: the stock
  X4 firmware does, CrossPoint 1.6.5 (ESP-IDF 5.5) does not. The v5.5.2 struct still has `fsm` at `0x54` with the
  same three fields (**M**: `register/soc/spi_mem_struct.h`).
- `docs/research/esp32c3-boot-chain.md:201` says `spimem_flash_ll_host_idle` is `dev->cmd.val == 0`; that is
  `spimem_flash_ll_cmd_is_done` (**M**, both versions). `host_idle` is `fsm.spi0_mst_st == 0` (v4.4.7) and
  `cmd.mst_st == 0` (v5.5.2). That report is a dated record and is not edited; this line corrects it.

### A.3 What a faithful model returns

- **No transfer in flight, SPI0 idle (the cache is not modelled):** the read-only fields [6:0] are 0 (both
  machines idle, "0 when idle" in the header), [11:7] the stored lock delay, 4 at reset: the word is `0x200`
  at reset, the value the SPI0 model already returns (`spi_mem.rs:218,231`). **R**.
- **While a transfer is in flight:** an SPI1 transfer drives the SPI1 nibbles of `CMD`, not `FSM`. `FSM` [6:4]
  leaves 0 only while SPI0 has a cache fill or write in flight, after a suspend (value 2), or when the arbiter
  has granted the cache (value 1). On silicon an SPI1 command may hold [6:4] non-zero while SPI0 is locked out,
  but no header says so and inkemu models no arbiter. **R**, not measured.
- **inkemu:** transfers complete at once. A write of `CMD` runs the command inside that write
  (`spi_mem.rs:669-708`), `Spi1::read(CMD)` answers 0 (`spi_mem.rs:714-717`), a suspend or resume is refused
  (`:722-740`), and the automatic wait (`FLASH_WAITI_CTRL`) is accepted as storage because "an operation
  completes at once" (`:99-134`). No window "in flight" is ever visible to the firmware, so `FSM` [6:4] is
  idle at every read. Returning a busy value would contradict `CMD` = 0 and could hang `wait_idle`. **M** for
  the code, **R** for the conclusion.
- **Write:** [11:7] is R/W, the rest read-only. v4.4.7 and v5.5.2 write the field on SPI0 only, and only on the
  suspend path that inkemu refuses (`FlashSuspend`); a write through SPI1 is not expected and is kept harmless:
  bits [11:7] are stored, the others ignored, nothing else happens. **R** (access column of the header).

### A.4 Recommendation (followed by SP1b-3e)

- SPI1 answers offset `0x54` as `SPI_MEM_FSM_REG`, reset value `0x200` (the SVD value of SPI0 `FSM`). A read
  returns the stored lock delay in [11:7] and 0 in every other bit, without a warning; a write stores [11:7]
  only. The register returns to `0x200` with the other registers of SPI1 at a reset of the controller.
- The value read through the SPI1 alias on silicon is not measured: `ASSUMPTION:` in the code, settled by a
  read of `0x60002054` on a chip at rest (expected `0x200`).
- Effect on a v4.4.7 firmware: `spimem_flash_ll_host_idle` is true, `spi_flash_hal_check_status` returns 1,
  `wait_idle` goes straight to the status read, as before, without the warning.

## B. The USB-Serial-JTAG tail lost across a core reset

### B.1 What ESP-IDF does (v4.4.7 and v5.5.2)

1. The panic handler writes the USB-Serial-JTAG IN FIFO one byte at a time and never commits.
   `panic_print_char_usb_serial_jtag` waits (100 µs steps, 50 ms in all) until
   `usb_serial_jtag_ll_txfifo_writable()`, then `usb_serial_jtag_ll_write_txfifo(&c, 1)`; there is no
   `usb_serial_jtag_ll_txfifo_flush()` (`WR_DONE`) in the function. **M**: v4.4.7 `esp_system/panic.c:93-111`,
   v5.5.2 `panic.c:109-127`. It is compiled in with `CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG` or
   `CONFIG_ESP_CONSOLE_SECONDARY_USB_SERIAL_JTAG`.
2. `panic_print_char` calls the UART writer and the USB-Serial-JTAG writer for each byte. The v4.4.7 UART writer
   fills UART0's FIFO directly (`uart_hal_write_txfifo`), not through the ROM, so the ROM's flush at `\n` does
   not apply to panic output. **M**: v4.4.7 `panic.c:74-80, 115-125`.
3. No flush before the restart in either version: `panic.c` ends with
   `panic_print_str("Rebooting...\r\n"); panic_restart();` (v4.4.7 `:383-384`, v5.5.2 `:463-464`); the only
   flushes in the file are those of app trace and SEGGER. **M**.
4. The tail of a v4.4.7 dump: `"\r\nELF file SHA256: "`, 16 hex digits, `"\r\n"`, `"\r\n"`, then
   `"Rebooting...\r\n"`. **M**: `panic.c:317-323, 383`. After the last digit come exactly **18 bytes**
   (`\r\n\r\nRebooting...\r\n`): the loss seen on the stock firmware.
5. v4.4.7 `panic_restart()` calls `esp_restart_noos_dig()` when memory protection is enabled or locked, else
   `esp_restart_noos()` (**M**: `port/panic_handler.c:222-249`). `esp_restart_noos_dig` ends with
   `SET_PERI_REG_MASK(RTC_CNTL_OPTIONS0_REG, RTC_CNTL_SW_SYS_RST)`, a **core reset, `rst:0x3`** (**M**:
   `esp_system/esp_system.c:29-52`). `CONFIG_ESP_SYSTEM_MEMPROT_FEATURE` and its `_LOCK` default to `y` on 4.4
   (**M**: `esp_system/Kconfig:118-135`). Espressif confirms that `esp_restart()` gives `RTC_SW_SYS_RST` or
   `RTC_SW_CPU_RST` depending on that option (**M**: esp-idf issue #10247). This matches the stock X4
   (`rst:0x3`, Saved PC `0x403819da`). **R**.
6. `esp_restart_noos_dig` waits only for `esp_rom_uart_tx_wait_idle(CONFIG_ESP_CONSOLE_UART_NUM)`, then sets the
   CPU clock to the crystal, then resets. No wait or flush of USB-Serial-JTAG. **M**: `esp_system.c:31-33`.
7. v5.5.2 `panic_restart()` always calls `esp_restart_noos()` (the `_dig` variant only for cache errors of the
   classic ESP32) (**M**: `port/panic_handler.c:312-321`), which ends with `esp_rom_software_reset_cpu(0)`, a
   **CPU reset, `rst:0xc`** (**M**: v5.5.2 `port/soc/esp32c3/system_internal.c:113`). Its list of modules reset
   on exit (`esp_system_reset_modules_on_exit`: timers, SPI0/1, UART, SYSTIMER, DMA, crypto) does not pulse
   `USB_DEVICE_RST` (**M**: `system_internal.c:31-61`; the same list without crypto in 4.4.7,
   `system_internal.c:96-114`), and its wait covers the UARTs only (**M**: `:34-38`). The comment "keep BBPLL on
   so that USB Serial JTAG can log at 1st stage bootloader" (`:107`) shows the restart is meant to leave the
   block running. **M**.
8. ESP-IDF documents the partial-packet hazard: "In rare cases, data sent from ESP32-C3 to the host gets 'stuck'
   … needs to be flushed to the host manually. The non-blocking (default) driver and the VFS implementation will
   flush automatically after a newline." Nothing on resets or the panic handler. **M**: v5.5.2
   `api-guides/usb-serial-jtag-console.html`. The note of `usb_serial_jtag_ll_txfifo_flush` says the hardware
   flushes by itself at 64 bytes. **M**: v5.5.2 `hal/esp32c3/include/hal/usb_serial_jtag_ll.h:164-177`.
9. TRM §30.3.2: "Writing the buffer doesn't immediately trigger sending data to the host … A flush can be
   triggered in two ways: after the 64th byte is written …, or … writing a one to USB_REG_SERIAL_WR_DONE." No
   flush on a timer. **M**.

Consequence (**R**): at the end of a panic, the bytes written since the last 64-byte boundary sit in the IN
FIFO, uncommitted. Whether they reach the host depends only on whether the block keeps its FIFO across the reset
and whether later output pushes them out.

### B.2 What a reset does to the block on silicon

- **CPU reset (`rst:0xc`)**: "Only resets CPU core" (and the PMS registers). **M**: TRM §6.1.3. The block keeps
  its FIFO; the ROM then prints through the same FIFO (`usb_uart_tx_one_char`, `WR_DONE` at `\n`), so the held
  bytes leave with the banner and `Rebooting...` appears on USB-Serial-JTAG. That is what CrossPoint shows.
  **R** (ROM behaviour from `docs/research/usb-serial-jtag-host.md` §B.1, measured on the ROM's disassembly).
- **Core reset (`rst:0x3`)**: "Resets the whole digital system except RTC, including CPU, peripherals, Wi-Fi,
  Bluetooth LE, and digital GPIOs". **M**: TRM §6.1.3. USB-Serial-JTAG is not named, and figure 6.1-1 (the
  scope of each reset) could not be read from the text of the PDF. **Gap.**
- Evidence that the controller is outside the digital reset:
  - TRM §30.3.4: "SRST is connected to the reset logic of the digital circuitry in the SoC and a high level on
    this line will cause a digital system reset. Note that the USB Serial/JTAG Controller itself is not
    affected by SRST." **M**.
  - Reset sources `0x15` and `0x16` are core resets triggered by the USB host, through the RTS/DTR sequence of
    the CDC-ACM port or through JTAG (TRM table 6.1-1; §30.3.2 table 30.3-2, `RTS=1, DTR=0` "Reset ESP32-C3").
    **M**. If a core reset also reset the controller, every esptool or monitor reset of a C3 would drop the
    USB device and the host's session; the port is commonly seen to survive them. **R** (not sourced).
  - Together these make "a core reset leaves the controller alone" plausible; the TRM does not say it for
    `SW_SYS_RST` (`0x03`). **R**.
- The software pulse `SYSTEM_USB_DEVICE_RST` (`PERIP_RST_EN0` bit 23) does reset the block ("Set this bit to
  reset USB DEVICE"). **M**: TRM, `SYSTEM` registers. No restart path of ESP-IDF pulses it (B.1 item 7).
- Field reports: a C3 with a USB-CDC console under Arduino core 3.3.7 (ESP-IDF 5.x, CPU reset) shows
  `Rebooting...` before the next boot log (**M** for the log, **R** for the console type: arduino-esp32 issue
  #12403). No capture of a core-reset panic over USB-Serial-JTAG, with or without the tail, was found. **Gap.**

### B.3 How inkemu models it, and why exactly these bytes are lost

- `UsbSerialJtag` keeps `in_fifo` (bytes written, not committed) and `committed` (a packet waiting for the
  host). **M**: `crates/soc-esp32c3/src/periph/usb_serial_jtag.rs:132-139`.
- `push` appends to `in_fifo`; the 64th byte and `EP1_CONF.WR_DONE` call `commit`; nothing else commits (no
  timer, as the TRM says). **M**: `:472-500, 568-575`.
- An open host takes a committed packet `HOST_LATENCY` = 50 µs after the commit. **M**: `:95-98, 317-322,
  405-409`.
- `UsbSerialJtag::reset` empties `in_fifo` and the committed packet, clears the FIFOs, sets
  `INT_RAW = SERIAL_IN_EMPTY` and `INT_ENA = 0`; what the host took stays. A unit test pins that "the packet
  being filled is lost" (`a_core_reset_empties_the_block_and_the_host_goes_on`, `:851-878`). **M**.
- `Peripherals::reset` calls it on a core reset (level ≥ Core) and on the pulse of `PERIP_RST_EN0` bit 23; a
  CPU reset leaves the block alone. `ResetCause::SoftwareSystem` is level Core, `SoftwareCpu` level Cpu. **M**:
  `crates/soc-esp32c3/src/periph/mod.rs:540, 569-601`, `crates/soc-esp32c3/src/reset.rs:83-95`.

The stock X4 (`rst:0x3`), step by step (**R**, from the code above):
1. The panic bytes reach `EP1` through `push`; each 64-byte packet is committed and taken 50 µs later while the
   handler waits for room, so nothing is lost there.
2. After the last 64-byte boundary the 18-byte tail stays in `in_fifo`: the panic path writes no `WR_DONE`.
3. `esp_restart_noos_dig` then waits for UART0 to drain: at least 1.6 ms for 18 bytes at 115,200 baud, far
   more than 50 µs, so a committed packet would have been taken; the case "committed, not yet taken" does not
   arise.
4. `SW_SYS_RST` → level Core → `UsbSerialJtag::reset` → `in_fifo.clear()`: the 18 bytes are dropped. The ROM's
   banner goes into an empty FIFO, which is why the stream jumps from the last hex digit to `ESP-ROM:` with no
   newline.
5. For CrossPoint (`rst:0xc`) `reset` is not called, `in_fifo` survives, and the ROM's first `\n` commits it with
   its banner: `Rebooting...` appears on USB-Serial-JTAG.

The tail ending exactly on the last hex digit means the last 64-byte boundary fell there, a 1-in-64 alignment
unless earlier flushes lined up. A one-line diagnostic in `reset` (`in_fifo.len()`, `committed.is_some()`)
would confirm `18` and `false`; it was not run.

### B.4 What is and is not established

| Question | Status |
|---|---|
| ESP-IDF commits nothing and waits for nothing on USB-Serial-JTAG before the panic restart (both versions) | **M** |
| The 4.4.7 path is a core reset (`rst:0x3`, `esp_restart_noos_dig`); the 5.5.2 path a CPU reset (`rst:0xc`) | **M** |
| inkemu drops the uncommitted IN bytes on a core reset and keeps them on a CPU reset | **M** |
| A CPU reset keeps the FIFO on silicon | **R** (TRM "Only resets CPU core"; `USB_DEVICE_RST` not pulsed) |
| A core reset (`SW_SYS_RST`) empties the IN FIFO on silicon | **Not established**: the TRM is silent for the core reset, and its one sentence on a digital-system reset (SRST, §30.3.4) says the controller is not affected |
| Silicon prints `Rebooting...` on USB-Serial-JTAG with the stock firmware | **Not established** (no capture) |

### B.5 Verdict

**Undecidable without hardware, leaning towards "too aggressive".** The loss is real ESP-IDF behaviour (an
uncommitted partial packet, no flush), and inkemu is faithful to it on a CPU reset. On a core reset the model
assumes the controller is reset; the only TRM sentence on a digital-system reset says it is not, and hosts
trigger core resets without losing the port. Nothing readable proves it either way for `SW_SYS_RST`.

### B.6 Ruling for SP1b-3e (decision E4)

The model is kept, and its `ASSUMPTION:` line in `usb_serial_jtag.rs` is replaced by one that gives these
sources and names the experiment that settles it: on a real X4 (or any C3 running a 4.4 image with memory
protection), capture USB-Serial-JTAG across a panic; or run a ten-line image that writes `abc` to `EP1` without
`WR_DONE`, sets `RTC_CNTL_SW_SYS_RST`, and reads the host side after the ROM's banner. `abc` present: the block
survives a core reset; absent: it is reset.

If the capture shows that the block survives, the change is in `Peripherals::core_reset`
(`periph/mod.rs:600`): stop calling `usb_serial_jtag.reset(ctx)` there and keep it for
`reset_module(Module::UsbDevice)` only; then decide which registers a core reset restores (`CONF0`, `INT_ENA`,
`INT_RAW`), and update `a_core_reset_empties_the_block_and_the_host_goes_on`. Assumption 12 of
`docs/research/usb-serial-jtag-host.md` ("Core reset keeps the USJ block") is to be read with §B.2 above: the
row of IR §4.2 that keeps the block is that of the CPU reset.
