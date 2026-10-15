# SP4b — The resource monitor — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** in the desktop application, a monitor panel shows the device's time split (executing, waiting in `wfi`, deep sleep), its load, its time per CPU frequency, the RAM pages it wrote, the interrupts and peripheral register accesses it made, the panel refreshes and a datasheet estimate of the chip's consumption, all measured without changing what the machine executes and at no measurable cost while the panel is closed.

**Architecture:**

| Where | Owns | Why |
|---|---|---|
| `crates/emu-core/src/memory.rs` | `Ram::version`, `Ram::page_versions` | 4b-D2, 4b-D5: the versions exist for the decode cache |
| `crates/soc-esp32c3/src/{usage.rs, machine/usage.rs, bus/usage.rs}`, hooks in `machine.rs` and `bus.rs` | the counters, `Esp32C3::usage` | 4b-D2 to 4b-D6 |
| `crates/monitor` (new, no host I/O, wasm) | `MonitorView`, `view`, names, the datasheet table | 4b-D1, 4b-D6 to 4b-D8 |
| `crates/host/src/{resources.rs, device.rs}` | the previous reading, the panel's use, `Handle::monitor` | 4b-D7 |
| `app/src-tauri/src/commands.rs` | `monitor` | 4b-D1 |
| `app/src` | `MonitorPanel`, the toolbar toggle, FR/EN | 4b-D12 |

**Tech Stack:** Rust 1.99.0 (edition 2024), no new third-party crate; the app's stack of SP2c, unchanged.

**Spec:** `docs/superpowers/specs/2026-10-15-sp4b-resource-monitor-design.md` (decisions 4b-D1 to 4b-D13, binding). Research: `docs/research/sp4b-resource-monitor.md`. ADR 0017 (task 8).

## Global Constraints

- `monitor` depends on `soc-esp32c3`, `svd-regs`, `serde`; it does no host I/O and builds for `wasm32-unknown-unknown`; it joins the crates whose `clippy.toml` bans host I/O (`crates/boards/tests/host_io_ban.rs`, nine copies) and the WebAssembly step of the CI.
- SP3 (the X3) works in parallel on `chips`, `soc-esp32c3` (I2C0, `io`), `boards`, and the catalog of `host`: changes to `soc-esp32c3` stay in new files plus one-line hooks in `machine.rs`, `bus.rs`, `lib.rs`, and the heir string of six rows in `periph/rtc_cntl/mod.rs` and `periph/system.rs`.
- `export CARGO_BUILD_JOBS=3`; one Docker container at a time (none is needed).
- Commits `type(scope): description`, no trailer, paths given explicitly to `git commit -- <paths>`; nothing pushed.
- Local checks: `cargo fmt --all --check`, `tools/ci/clippy.sh` on the crates touched, their unit tests, the wasm check of `monitor`, the app's lint, typecheck and Vitest, the shell's clippy and tests. Left to the CI: `cargo test --workspace`, coverage, the suites that need the reference build or the ROM.

## Rulings

| # | Ruling | Why |
|---|---|---|
| 4b-R1 | **Test first, every task**: the failing test is committed with the change that makes it pass, in the same commit; the test names the behaviour. | Repository rule. |
| 4b-R2 | **Observing changes nothing.** `Esp32C3::usage` takes `&self`; no counter is read by the run. The proof compares a run cut into calls with a reading after each with a run in one call (trace hash, cycles, instructions, device time, registers, memory, and the final reading), under both timing models, and a host replay read at every status to "identical". | 4b-D4. |
| 4b-R3 | **No pinned hash moves.** A hash that moves is a regression of 4b-R2, investigated, never re-pinned. | 4b-D4. |
| 4b-R4 | **No count per instruction.** A counter is touched where time or an event already passes (a sleep, a clock change, an interrupt taken, a sampling of the sources, a register access); the executed time is a difference. | 4b-D2. |
| 4b-R5 | **No byte of memory or flash in a view**: the monitor shows which pages were written, never their content. | 4b-D13, ADR 0016 §6. |
| 4b-R6 | **A datasheet figure is said as one**: every current comes with the reference of its table; a state without a figure gives no current and its time is said; nothing is extrapolated. | 4b-D8, spec §6.1. |
| 4b-R7 | **The interface computes nothing about the machine**: shares, windows, names and currents come from `monitor`; the panel formats numbers in its language. | ADR 0011, the user's rule. |
| 4b-R8 | **The cost is measured, not argued**: the bench runs on `main` and on the branch, and the figures go into the hand-over note. | 4b-D11. |

## Tasks

### Task 1: Page versions of a RAM (TDD, `emu-core`)

- [ ] Tests: a new RAM has version 1 and every page at 0; a write sets its pages to the version it took and moves the version on; a load counts as a write.
- [ ] `Ram::version`, `Ram::page_versions`.

### Task 2: The counters of the machine (TDD, `soc-esp32c3`)

- [ ] Tests: a program that waits in `wfi` has waiting time and executed time that add up to its device time; a deep sleep is counted apart and its time is in no clock; time per frequency after a clock switch; interrupts raised per source and taken per line; register accesses per block, reads and writes apart; RAM pages written before and since a reset; **a run read after every cut of a call equals a run in one call, final reading included**, under both timing models.
- [ ] `usage.rs` (types), `machine/usage.rs` (`Counters`, `Esp32C3::usage`), `bus/usage.rs` (blocks, memories); hooks at the four `sleep_until`, in `retune`, in `turn` (interrupt taken), in `select_interrupt`, in `reset`, in `mmio_read`/`mmio_write`. The interrupting program of SP4a's tests moves to `machine/test_programs.rs`, shared.

### Task 3: The `monitor` crate (TDD)

- [ ] Tests: load per mille; recent window only after an earlier reading of the same run, and none when time has not moved or a counter went back; block and source names; the power rows at 160 and 80 MHz, the unpriced time at other frequencies, the deep-sleep row, the totals; page states; the JSON shape (camelCase, tags).
- [ ] `lib.rs`, `view.rs`, `names.rs`, `power.rs`; `Cargo.toml`, `clippy.toml` (ninth copy of the ban); workspace member; `host_io_ban.rs`; CI wasm step and its comment.

### Task 4: The monitor in the host (TDD, `host`)

- [ ] Tests: a view of a running life, of a paused one, of an ended one until the next powers on; the recent window between two readings, none across lives; the panel's refreshes counted; **a replay read at every status is identical**.
- [ ] `resources.rs` (`Resources`), `Request::Monitor`, `Handle::monitor`, `Emulator::monitor`, the frame hook.

### Task 5: The shell (TDD, `app/src-tauri`)

- [ ] Test: `monitor` answers a view of the emulator of its start.
- [ ] `#[tauri::command] monitor`, registered in `lib.rs`.

### Task 6: The panel (TDD, `app`)

- [ ] Tests: the toolbar toggles the panel (`aria-expanded`); sections and tables; percentages, times, frequencies and charges formatted in FR and EN; the "datasheet" mention; the page map's text; read every second while running and never while closed; the catalogs stay equal.
- [ ] `ipc/types.ts`, `ipc/host.ts`, `test/fake-host.ts`, `components/monitor-panel.tsx`, `components/monitor-format.ts`, `toolbar.tsx`, `app.tsx`, `i18n/{fr,en}.ts`, `styles.css`.

### Task 7: The bench and the heirs

- [ ] `crates/soc-esp32c3/tests/throughput.rs` (ignored); measured on `main` and on the branch.
- [ ] The six power rows: heir SP8; `docs/registers-accepted-without-effect.md` regenerated.

### Task 8: Documentation

- [ ] ADR 0017; README (English and French); the hand-over note with its "Cartographie" section.

### Task 9: Regression

- [ ] `cargo fmt`, clippy on `emu-core`, `soc-esp32c3`, `monitor`, `host`, `debugger`; their tests; the shell's checks; the app's checks; the wasm check.
