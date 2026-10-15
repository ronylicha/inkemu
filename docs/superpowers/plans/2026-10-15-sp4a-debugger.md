# SP4a — The debugger in the application — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** in the desktop application, a debugger panel shows the registers, the CSRs, the code around the PC and memory, read without effect, and sets breakpoints and watchpoints that stop the run without changing what the machine executes.

**Architecture:**

| Where | Owns | Why |
|---|---|---|
| `crates/cpu-rv32/src/disasm.rs` | the text of an instruction | 4a-D1: the decoder lives here |
| `crates/soc-esp32c3/src/{debug.rs, bus/debug.rs, machine/debug.rs}`, `run_loop` in `machine.rs` | stops at a boundary, the note of accesses, `peek` | 4a-D2 to 4a-D5 |
| `crates/debugger` (new, no host I/O, wasm) | `Points`, the regions shown, the serialisable views, `inspect` | 4a-D6, 4a-D7, 4a-D11 |
| `crates/host/src/{engine,device}.rs` | points across lives, the stop, inspection, the machine of an ended life | 4a-D8 to 4a-D10 |
| `app/src-tauri/src/commands.rs` | `breakpoint`, `watchpoint`, `inspect` | 4a-D1 |
| `app/src` | `DebugPanel`, the toolbar toggle, FR/EN | 4a-D12 |

**Tech Stack:** Rust 1.99.0 (edition 2024), no new third-party crate; the app's stack of SP2c, unchanged.

**Spec:** `docs/superpowers/specs/2026-10-15-sp4a-debugger-design.md` (decisions 4a-D1 to 4a-D13, binding). Research: `docs/research/sp4-debugging.md`. ADR 0016 (task 8). Split of SP4: `docs/superpowers/notes/2026-10-15-sp4-decoupage.md`.

## Global Constraints

- `debugger` depends on `cpu-rv32`, `soc-esp32c3`, `serde`, `thiserror`; it does no host I/O and builds for `wasm32-unknown-unknown`; it joins the crates whose `clippy.toml` bans host I/O (`crates/boards/tests/host_io_ban.rs`, eight copies) and the WebAssembly step of the CI.
- SP3 (the X3) works in parallel on `chips`, `soc-esp32c3` (I2C0), `boards`, and the catalog and X3 ports of `host`/`app`: changes to `soc-esp32c3` stay in new files plus the minimal hooks in `machine.rs`, `bus.rs`, `lib.rs`.
- `export CARGO_BUILD_JOBS=3`; one Docker container at a time.
- Commits `type(scope): description`, no trailer, paths given explicitly to `git commit -- <paths>`; nothing pushed.
- Local checks: `cargo fmt --all --check`, `tools/ci/clippy.sh` on the crates touched, their unit tests, the wasm check of `debugger`, the app's lint, typecheck and Vitest, the shell's clippy and tests. Left to the CI: `cargo test --workspace`, coverage, the suites that need the reference build or the ROM.

## Rulings

| # | Ruling | Why |
|---|---|---|
| 4a-R1 | **Test first, every task**: the failing test is committed with the change that makes it pass, in the same commit; the test names the behaviour. | Repository rule. |
| 4a-R2 | **A stop changes nothing.** `run_within` and `run_stopping` share one loop; the debugger's hooks read the machine at a boundary or take what the bus noted, nothing else. The proof is a test that compares a run cut at every point with a run in one call (trace hash, cycles, instructions, device time, registers, memory) and a host test that replays a session with breakpoints to "identical". | 4a-D4. |
| 4a-R3 | **No pinned hash moves.** A hash that moves is a regression of 4a-R2, investigated, never re-pinned. | 4a-D4. |
| 4a-R4 | **No flash byte leaves the debugger.** The memory view never holds a byte of a flash window; the disassembly holds decoded text only; a watchpoint on a flash window is refused; nothing the debugger reads is logged, written or put in a session; tests use our own programs only. | 4a-D6, repository rule. |
| 4a-R5 | **No peripheral register is read by the debugger**: `peek` answers `Register` without calling the device. | 4a-D5. |
| 4a-R6 | **Points are the emulator's**: held by the thread, handed to each new engine, kept through resets, accepted during a replay. | 4a-D8. |
| 4a-R7 | **Read-only**: no register, CSR or memory write in SP4a (`Esp32C3::set_x` stays the lockstep's). | 4a-D11. |
| 4a-R8 | **The interface decides nothing about the machine**: what is readable, what is refused and why come from the host; the panel shows them. | ADR 0011. |

## Tasks

### Task 1: The text of an instruction (TDD, `cpu-rv32`)

- [ ] Tests: plain forms as `objdump` writes them, branch and jump targets as absolute addresses, the pseudo-instructions, compressed parcels in their expanded form, CSRs by name or in hexadecimal, an illegal instruction without its bits.
- [ ] `disasm.rs`: `ABI_NAMES`, `register_name`, `csr_name`, `disassemble(&Decoded, pc)`.

### Task 2: Stops and peeks in the machine (TDD, `soc-esp32c3`)

- [ ] Bus tests: a store to watched bytes noted with its value and its instruction; read/write/access; the first access kept; both SRAM views; no overflow at the top of the address space; a faulting access not noted; peeks of ROM, RAMs, registers (not read), unmapped, flash through the MMU with no fault latched, no report and no cache state.
- [ ] Machine tests: a breakpoint stops before its instruction and the next call goes past it; at the reset vector before anything runs; a watchpoint stops after its instruction; **a run cut at every point equals one call**; no stop of a CPU waiting in `wfi`; a watchpoint met with a strict stop ends the next call before anything runs; a step gives its own access only; machine mode at reset.
- [ ] `debug.rs` (types), `bus/debug.rs` (note, `peek_byte`), `machine/debug.rs` (`Stops`, `NoStops`, `DebugStops`, the public methods), `run_loop` shared by `run_within` and `run_stopping`, `breakpoint_left` cleared by every turn.

### Task 3: The `debugger` crate (TDD)

- [ ] Tests: `Points` bounds and named refusals (odd address, no code, too many, length, flash window, nothing mapped, already set); `Region::of`; the memory view shows no flash and no register byte and groups its regions; the disassembly around the PC lands on it, marks the breakpoints and the current line, says unreadable lines; the CSRs of the core in their order; the JSON shape (camelCase, tags).
- [ ] `points.rs`, `region.rs`, `view.rs`, `lib.rs`; `Cargo.toml`, `clippy.toml` (the eighth copy of the ban); workspace member; `host_io_ban.rs`; CI wasm step and its comment.

### Task 4: The debugger in the host (TDD, `host`)

- [ ] Engine tests: a pass stops at a breakpoint and reports it; a step over a watched store reports it; points given to the engine reach the machine.
- [ ] Device tests: a breakpoint pauses the run, says it in the status and the journal; Run goes past it; the points survive a reset; inspection while paused, while running, after the end of a life; refusals named (`point`); **a replay with breakpoints is identical**.
- [ ] `Engine::set_points`, `Output.stop`, `Engine::inspect`; `Command::Breakpoint`, `Command::Watchpoint`, `CommandError::Point`; `Request` as an enum, `Handle::inspect`; `Worker.points`, `Worker.stop`, `Worker.ended`; `Status.stop`.

### Task 5: The shell (TDD, `app/src-tauri`)

- [ ] Tests: the DTOs of the two commands turn into the host's commands, a wrong `on` is refused as `input`; `inspect` answers a view of a running test device.
- [ ] `CommandDto::Breakpoint`, `CommandDto::Watchpoint`, `#[tauri::command] inspect`, registered in `lib.rs`.

### Task 6: The panel (TDD, `app`)

- [ ] Tests: the toolbar toggles the panel (`aria-expanded`); registers, CSRs, disassembly (current line, breakpoint buttons), memory rows and the regions not shown; a click on a line sets and clears a breakpoint; the forms parse hexadecimal and refuse what is not an address; the stop is said; the catalogs stay equal.
- [ ] `ipc/types.ts`, `ipc/host.ts` (`inspect`), `test/fake-host.ts`, `components/debug-panel.tsx` and `components/debug-views.tsx`, `toolbar.tsx`, `app.tsx`, `i18n/{fr,en}.ts`, `styles.css`.

### Task 7: Regression

- [ ] `cargo fmt`, clippy on `cpu-rv32`, `soc-esp32c3`, `debugger`, `host`, `emu-cli`; their tests; the shell's checks; the app's checks; the wasm check.

### Task 8: ADR, documents, hand-over

- [ ] ADR 0016 (stops that change nothing, the read without effect, what the debugger shows of the flash, the points of the emulator).
- [ ] README (English and French): the debugger panel.
- [ ] Hand-over note with "Cartographie".
