# SP2b — The SoC peripherals of the X4 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** the SoC side of sub-project 2: the GPIO matrix and IO MUX (levels, `GPIO.IN`, GPIO interrupts), GP-SPI2 as a master in CPU mode, I2C0 as a master that hears no target (an X4), the SAR ADC's one-shot reads calibrated from the eFuse, and the neutral signal traits SP2d plugs SP2a's chips into. CrossPoint 1.6.5 passes the ADC wait that stops it today.

**Architecture:**

| Where | Owns | Why |
|---|---|---|
| `crates/emu-core/src/signals.rs` | `Level`, `Nanos`, `GpioInput`, `GpioOutput`, `SpiTarget`, `AnalogSource`, their `Rc<RefCell<_>>` impls | the contract neither side owns (ADR 0010) |
| `crates/soc-esp32c3/src/periph/io/` | `IoMatrix`: GPIO and IO MUX registers, pad resolution, peripheral signals, attachments, GPIO interrupts | one owner of the pads' state |
| `crates/soc-esp32c3/src/periph/spi2.rs` | GP-SPI2 master, CPU mode | the display and the SD card |
| `crates/soc-esp32c3/src/periph/i2c0.rs` | I2C0 master, no target | the X3/X4 probe |
| `crates/soc-esp32c3/src/periph/saradc.rs` | SAR ADC one-shot, eFuse calibration, temperature-sensor registers | the buttons and the battery |
| `crates/soc-esp32c3/src/machine.rs` | `attach_*`, `inputs_changed`, `AttachError` | the board's door |
| `tools/reference-firmware/soc_signals/` | our firmware, built and pinned | the CI proof |
| `crates/boards/tests/reference_signals.rs` | its test with targets attached (2b-R3) | |

**Tech Stack:** Rust 1.99.0 (edition 2024). No new crate dependency (`boards` takes `emu-core` and `sha2`, both in the workspace, as dev-dependencies for its test). ESP-IDF v5.5.2 in `espressif/idf:v5.5.2`.

**Spec:** `docs/superpowers/specs/2026-10-15-sp2b-soc-peripherals-design.md` (2b-D1 to 2b-D14, binding). Research: `docs/research/esp32c3-soc-peripherals-sp2b.md`. ADR 0010.

## Global Constraints

- `export CARGO_BUILD_JOBS=3`; locally fmt, targeted clippy and narrow tests only; the full suites run in CI.
- One Docker container at a time; never while another `espressif/idf` container runs.
- No flash byte and no stack dump is ever printed (a device's NVS may hold Wi-Fi credentials).
- No third-party binary in the repository or the CI; no TODO.
- Pinned trace hashes move only as 2b-D14 says, with the reason written next to the new value.
- Sources are edited with the editor only, never by script. Commits: `type(scope): description`, no trailer.

## Rulings taken while doing it

| # | Ruling | Why |
|---|---|---|
| 2b-R1 | **Tests first** for every model: a failing unit test of the register behaviour, then the code. | TDD. |
| 2b-R2 | **The timed scan of the SAR ADC that ESP-IDF's bootloader runs is accepted, not refused.** `CTRL2.SARADC_TIMER_EN` is refused only when software could see the samples: `DMA_CONF.APB_ADC_TRANS` clear (they stay in the controller) or a threshold interrupt enabled; checked at each write of `CTRL2`, `DMA_CONF` and `INT_ENA`. A scan whose samples only go to GDMA samples nothing in the model. | Measured: every boot through the shipped bootloader runs `bootloader_random_enable` (`bootloader_random_esp32c3.c:16-56`, PC `0x403cc3ea`), so 2b-D10's blanket refusal warned on every reference firmware and on CrossPoint. Nothing in the chip changes for software (the RNG mixes the noise); a firmware that links a GDMA channel touches GDMA, which is unmodelled and says so. |
| 2b-R3 | **The test of `soc_signals` lives in `crates/boards/tests/reference_signals.rs`**, not in `soc-esp32c3`; it also probes I2C0. | It composes the flash as `inkemu run --firmware` does (`boards::Profile::compose_flash`); `boards` depends on `soc-esp32c3`, and a dev-dependency the other way would build a second `soc_esp32c3` whose types differ from those the machine is made of. The probe puts I2C0's NACK in the CI, where only CrossPoint (local) had it. |
| 2b-R4 | **`adc_wait_panic` waits a second time, for ADC2, which nothing starts**, and its binary is pinned again. | With the ADC modelled its first wait ends; the test was the only one in the CI of the interrupt watchdog's panic, the core dump in flash and the CPU reset `rst:0xc`. A wait for a conversion nothing started never ends on a chip either, so the second life and its trace stay faithful. Its trace hash moves, the reason written beside it (2b-D14). |
| 2b-R5 | **`inputs_changed` is needed only for an interrupt or a wake-up**: a read of `GPIO.IN` (or `STATUS`) works the pads out again at that instant and sees a chip's new level without it. | The pads are worked out again on each write of the matrix and each read of `IN` and `STATUS` (`IoMatrix::settle`); the call exists for a change that must raise the GPIO interrupt while the CPU reads nothing (a button pressed during `WFI`). |
| 2b-R6 | **A pulse of a chip select without a frame is the driver's, and a test accepts it.** `soc_signals`' panel sees its chip select low for 531 ns before its first frame. | `spicommon_cs_initialize` calls `gpio_output_enable`, which routes `GPIO_OUT` (low) and selects the GPIO function, before it connects `FSPICS0` (ESP-IDF v5.5.2 `spi_common.c:756-768`, `gpio.c:215-222`): a chip sees the same pulse. A chip model must not take a select without a frame for a command. |
| 2b-R7 | **Measured outcome, local, release build, 2026-10-15.** CrossPoint 1.6.5: no warning in 5 s of device time (59 s host), and under `--strict-mmio` 60 s of device time without a stop (30 s host); the console reaches the idle loop (`[MEM] Free` every 10 s). Stock firmware 3.0.2: deep sleep at 637 053 us, the GPIO wake (`WAKEUP_ENA` `0x4`) its one warning. `soc_signals`: 2.7 s host; trace hash `0x08d63d647e5a8a29`. `adc_wait_panic`: reading `0xfff` (GPIO1 pulled up), trace hash `42e50c75581ae020`. The reference suites (`reference_boot` 13, `run_cli reference_firmware` 13 of 14, CoreMark left to the CI; `console-script` 5) pass; the three lockstep windows of the CI and the local `crosspoint` window give their pinned counts, ends and usages. | Criterion 4 of the spec: CrossPoint's first unmodelled access is past the end of its boot, none in 60 s. |

## Tasks

### Task 1: Research and design

- [x] Run CrossPoint 1.6.5 and the reference firmwares at `194f286`; read the TRM, the SVD, ESP-IDF v5.5.2 and QEMU's tag (research P).
- [x] Spec, ADR 0010, this plan.

### Task 2: The signal traits (`emu-core`)

- [x] Tests: `Level` conversions; a `Rc<RefCell<T>>` forwards each call to `T`; defaults (`next_change` is `None`).
- [x] `signals.rs`, re-exported from `lib.rs`; rustdoc for each item.

### Task 3: The IO matrix

- [x] Tests: register storage and reset values (the six input selectors and `ENABLE` read as before); `W1TS/W1TC`; pad resolution (GPIO output, open drain, pulls, floating, `FUN_IE`, function 0/2); attached inputs and outputs; interrupts of the five types, `STATUS`, `PCPU_INT`, the source; `next_change` wake-up; unmodelled signal and NMI refused by name.
- [x] `periph/io/`; routing of both blocks in `Peripherals`; `Event::Pins`; diagnostics.

### Task 4: GP-SPI2

- [x] Tests: `UPDATE`; a full-duplex transfer to a selected target, at the end instant; half duplex; command and address phases; bit order; hardware CS and `CS_KEEP_ACTIVE`; DC; `TRANS_DONE` and source 19; each refusal by name; module reset.

### Task 5: I2C0

- [x] Tests: FIFO through `DATA`; a probe sequence (RSTART, WRITE with `ack_en`) raises NACK at the address, a STOP command then completes; without `ack_en` a READ gets `0xFF`; END; timing; source 29; refusals.

### Task 6: SAR ADC

- [x] Tests: the eFuse calibration decoded; raw for a voltage inverts ESP-IDF's curve fitting within one count; a conversion's duration and `INT_RAW`; ADC2; inversion; unattached pin; scan refused, the bootloader's timed scan accepted (2b-R2); source 43.

### Task 7: The machine's door

- [x] `attach_*`, `inputs_changed`, `AttachError`, module resets, core reset; tests through `Esp32C3`.

### Task 8: Reference firmware and its test

- [x] `tools/reference-firmware/soc_signals/`, `build.sh`, `in-container.sh`, pins; test `reference_signals.rs`; `adc_wait_panic`'s test; CI.

### Task 9: CrossPoint locally, documents

- [x] Run CrossPoint 1.6.5; update `real_firmware.rs`; README (EN, FR), reference-firmware README, accepted-register table, carry-over note with its "Cartographie" section.
