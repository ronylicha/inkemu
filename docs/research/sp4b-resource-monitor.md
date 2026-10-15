# The resource monitor (SP4b): what the machine can measure, and what the datasheet gives

Date 2026-10-15, branch of SP4b from `main` at `5a28566` (SP4a merged: the debugger in the application). Tags as in
`docs/research/sp4-debugging.md`: **[SRC]** read in a specification or in source code, **[DOC]** stated by a vendor's
documentation, **[INF]** reasoning, **[HYP]** chosen here. Global spec §6.1: "the monitor shows measured values only;
when a value cannot be measured, it says so instead of showing an estimate". The design that rules on what follows is
`docs/superpowers/specs/2026-10-15-sp4b-resource-monitor-design.md`.

## 1. What the machine already counts, for free

- Device time moves in exactly five places: an instruction (`Ctx::advance`, or `advance_time_base` under
  `qemu-icount`), and four calls of `Ctx::sleep_until` — two in `Esp32C3::sleep` (a `wfi`: to the next wake-up, or to the
  limit of the call), two in `Esp32C3::sleep_deeply` (a deep sleep: likewise) **[SRC]**
  `crates/soc-esp32c3/src/machine.rs`. The time the CPU waited and the time the chip slept deeply are therefore known
  where they pass, without a test per instruction; the time it executed is the rest **[INF]**.
- The CPU clock changes in one place, `Esp32C3::retune`, after a write to `SYSCLK_CONF` or `CPU_PER_CONF` **[SRC]**
  same file, `periph/system.rs`. Time per frequency is closed there **[INF]**.
- An interrupt taken is one turn of the loop (`StepOutcome::Interrupted { cause }`), and the levels of the 62 sources
  are sampled by the controller only in `select_interrupt`, at an instruction boundary, when one changed **[SRC]**
  `machine.rs`, ADR 0003: a source raised and lowered within one instruction is lost to the controller; counting rises
  there counts what the controller sees **[INF]**.
- Every RAM block keeps a write version per 4 KiB page, for the decode cache: `Ram::write` sets the page's version to a
  counter that grows by one per write **[SRC]** `crates/emu-core/src/memory.rs`. A page whose version is 0 was never
  written since the machine was built; a page whose version is at or above a mark taken at a reset was written since
  that reset; the counter is the number of writes. The memories keep their content through every reset, and nothing
  but the CPU writes them (no DMA into the SRAM in the model) **[SRC]** `bus.rs`. The RAM footprint costs nothing new
  **[INF]**.
- A peripheral register access goes through `SocBus::mmio_read` or `mmio_write` **[SRC]** `bus.rs`. The SVD places the
  blocks at 2 KiB or more from each other, except a few registers of a block inside another's page (`RNG` in
  `APB_CTRL`, `GPIO_SD` in the upper half of `GPIO`'s page, `EFUSE` in the upper half of `RTC_CNTL`'s) **[SRC]**
  `crates/svd-regs/src/esp32c3.rs`: a counter per 2 KiB of the 1 MiB peripheral window (512 counters per direction) tells
  the blocks apart, `APB_CTRL` and `RNG` together **[INF]**.

## 2. What cannot be measured without the firmware's structures

- The heap and the FreeRTOS tasks (list, stack of each, high-water mark of each) need the ELF and
  `FreeRTOS_openocd_params` **[SRC]** `docs/research/sp4-debugging.md` §4: SP4d.
- "Occupation" of a RAM region in the sense of an allocator cannot be read without the heap. What the bus can say is
  which pages were written, since power-on and since the last reset, at the granularity of the page **[INF]**.
- The battery level and the cable are inputs of the board fixed at power-on (ADR 0013); they become live inputs in
  SP4c **[SRC]** `docs/superpowers/notes/2026-10-15-sp4-decoupage.md`.

## 3. The consumption the datasheet gives

ESP32-C3 Series Datasheet v2.4, 2026-05-06, §5.6.2 "Current Consumption in Other Modes", page 57
(<https://www.espressif.com/sites/default/files/documentation/esp32-c3_datasheet_en.pdf>, the PDF read on 2026-10-15,
SHA-256 `833fc000b4b3c3d39c496fcbd597fed5806956503ce7390b19cc8ae82f19f968`) **[DOC]**:

| Table | Mode | CPU frequency | CPU | Typ., all peripheral clocks disabled | Typ., all enabled |
|---|---|---|---|---|---|
| 5-8 | Modem-sleep | 160 MHz | running | 23 mA | 28 mA |
| 5-8 | Modem-sleep | 160 MHz | idle | 16 mA | 21 mA |
| 5-8 | Modem-sleep | 80 MHz | running | 17 mA | 22 mA |
| 5-8 | Modem-sleep | 80 MHz | idle | 13 mA | 18 mA |
| 5-9 | Light-sleep | — | — | 130 µA | |
| 5-9 | Deep-sleep | RTC timer + RTC memory | — | 5 µA | |
| 5-9 | Power off | `CHIP_EN` low | — | 1 µA | |

- Notes of table 5-8: "the current consumption might be different depending on which peripherals are enabled"; in
  Modem-sleep "Wi-Fi is clock gated"; "the consumption might be higher when accessing flash. For a flash rated at
  80 Mbit/s, in SPI 2-line mode the consumption is 10 mA" **[DOC]**.
- Section 5.6.1 gives its conditions as "3.3 V supply at 25 °C" **[DOC]**; 5.6.2 states none of its own.
- No figure at 40 MHz or 20 MHz (the crystal and the clock after a power-on reset, `crates/soc-esp32c3/src/clock.rs`):
  the monitor has no current to give then, and says so **[DOC]**, **[INF]**.
- The model has no radio (Modem-sleep is the state of a chip whose Wi-Fi is gated), refuses a light sleep by name, and
  ends a life when the board cuts its supply: the states it reaches are running and idle (`wfi`) at a CPU frequency,
  and deep sleep **[SRC]** `machine.rs` (`light_sleep_stops_a_strict_run`), ADR 0013 **[INF]**.
- A per-domain figure (RTC memory powered or not in deep sleep, `VDD_SDIO` off, memories powered down in light sleep)
  is not in the datasheet: the registers that choose them (`RTC_CNTL.OPTIONS0`, `PWC`, `DIG_PWC`, `BIAS_CONF`,
  `SDIO_CONF`, `SYSTEM.MEM_PD_MASK`) change no figure the monitor could give. Only a measurement on hardware can give
  them an effect **[INF]**: the calibration bench, SP8 (global spec §9).

## 4. Conclusions

1. Time split, load, time per frequency, interrupts raised and taken, peripheral register accesses and the RAM pages
   written are measurable without the firmware's knowledge, at almost no cost: no test per instruction, one
   increment per register access, nothing new per RAM write **[INF]**.
2. The consumption is an estimate from typical datasheet figures for the chip alone, given as a range (all peripheral
   clocks disabled to all enabled) and only where the datasheet has a figure **[INF]**.
3. Tasks, heap, battery and cable belong to later plans (SP4d, SP4c) **[INF]**.
