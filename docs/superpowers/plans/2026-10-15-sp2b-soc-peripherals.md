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
| `crates/soc-esp32c3/tests/reference_signals.rs` | its test with targets attached | |

**Tech Stack:** Rust 1.99.0 (edition 2024). No new crate dependency. ESP-IDF v5.5.2 in `espressif/idf:v5.5.2`.

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

## Tasks

### Task 1: Research and design

- [x] Run CrossPoint 1.6.5 and the reference firmwares at `194f286`; read the TRM, the SVD, ESP-IDF v5.5.2 and QEMU's tag (research P).
- [x] Spec, ADR 0010, this plan.

### Task 2: The signal traits (`emu-core`)

- [ ] Tests: `Level` conversions; a `Rc<RefCell<T>>` forwards each call to `T`; defaults (`next_change` is `None`).
- [ ] `signals.rs`, re-exported from `lib.rs`; rustdoc for each item.

### Task 3: The IO matrix

- [ ] Tests: register storage and reset values (the six input selectors and `ENABLE` read as before); `W1TS/W1TC`; pad resolution (GPIO output, open drain, pulls, floating, `FUN_IE`, function 0/2); attached inputs and outputs; interrupts of the five types, `STATUS`, `PCPU_INT`, the source; `next_change` wake-up; unmodelled signal and NMI refused by name.
- [ ] `periph/io/`; routing of both blocks in `Peripherals`; `Event::Pins`; diagnostics.

### Task 4: GP-SPI2

- [ ] Tests: `UPDATE`; a full-duplex transfer to a selected target, at the end instant; half duplex; command and address phases; bit order; hardware CS and `CS_KEEP_ACTIVE`; DC; `TRANS_DONE` and source 19; each refusal by name; module reset.

### Task 5: I2C0

- [ ] Tests: FIFO through `DATA`; a probe sequence (RSTART, WRITE with `ack_en`) raises NACK at the address, a STOP command then completes; without `ack_en` a READ gets `0xFF`; END; timing; source 29; refusals.

### Task 6: SAR ADC

- [ ] Tests: the eFuse calibration decoded; raw for a voltage inverts ESP-IDF's curve fitting within one count; a conversion's duration and `INT_RAW`; ADC2; inversion; unattached pin; scan refused; source 43.

### Task 7: The machine's door

- [ ] `attach_*`, `inputs_changed`, `AttachError`, module resets, core reset; tests through `Esp32C3`.

### Task 8: Reference firmware and its test

- [ ] `tools/reference-firmware/soc_signals/`, `build.sh`, `in-container.sh`, pins; test `reference_signals.rs`; `adc_wait_panic`'s test; CI.

### Task 9: CrossPoint locally, documents

- [ ] Run CrossPoint 1.6.5; update `real_firmware.rs`; README (EN, FR), reference-firmware README, accepted-register table, carry-over note with its "Cartographie" section.
