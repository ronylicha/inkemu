# SP1b-3d — Serial consoles in both directions — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**As executed (2026-10-13).** The hand-over note, `docs/superpowers/notes/2026-10-13-sp1b-3d-carryover.md` (dated the day it was written, not 2026-10-14 as Task 11 says), records what differed from this plan. In short: the eleven task commits were cherry-picked from the scratch tree the architect built and checked them in, unchanged; four domain reviews, their fixers and the CI then added thirty-six commits (UART clocks and refusals, same-instant order and resets of USB-Serial-JTAG, the timing of `wait-for` and the bounds of the inputs, the console driver, the watchdog test, the CI steps), each with the test that holds it, and five more `ASSUMPTION:` lines (106 at the end, not 101); the values only the CI can produce were read from its logs and pinned there (the two trace hashes, the three `SHA256SUMS.inputs-*`, the README summaries), the watchdog test now accepts the whole UART wait loop of the ROM, and milestone 2 checks USB-Serial-JTAG as UART0 with the echo in it, since the ROM mirrors the application's console there. The rest of this plan is left as it was written.

**Goal:** a firmware that reads its console works under inkemu, on UART0, UART1 and USB-Serial-JTAG, with input that is part of the run: dated in device time, the same on every host, and independent of how the run is cut into calls. The USB host is modelled well enough that a firmware which checks for one (ESP-IDF's connection monitor, Arduino `HWCDC`, hence CrossPoint in SP1b-3e) sees it. Milestones, checked by tests (the heavy ones in the GitHub CI): (1) the stock ESP-IDF example `uart_select` under `--strict-mmio` prints its timeout line 5 s of device time after `app_main` returns, `a` typed after it gives exactly `Received: a`, and `bc\r\n` typed in one burst gives its four lines in order; (2) the stock `usb_serial_jtag_echo`, host open, echoes `hi, espressif\n` on USB-Serial-JTAG, dumps it on UART0, and reads 90 bytes as 64 + 26; (3) a small application of this repository, whose console is USB-Serial-JTAG, prints `ready` and echoes `hello\r` with the host open and nothing at all with the host absent; (4) the reference `hello_world`, host open, carries the same bytes on USB-Serial-JTAG as on UART0; (5) two runs with the same inputs give the same consoles and trace hash, whatever slices they are cut into.

**Architecture:**

| Crate | Owns | Why a GUI can call it |
|---|---|---|
| `soc-esp32c3` | the UART receive path, its interrupts and its frame timing (`periph/uart.rs`); USB-Serial-JTAG in both directions, its interrupts, SOF and the USB host (`periph/usb_serial_jtag.rs`, `UsbHost`); the inputs of a run as methods of `Esp32C3` (`send_console`, `set_usb_host`, `watch_console`, `device_time`, `InputError`, `StopReason::ConsoleOutput`); the console driver of the tests (`test_support::drive`) | no host I/O, typed errors, inputs are values dated in device time |
| `console-script` (new) | the console script: its format and its refusals (`Script::parse`, `ScriptError`), and its run against a machine (`ScriptRun`: `start`, `observe`, `status`) | no host I/O (its `clippy.toml` bans it), builds for `wasm32-unknown-unknown`; a GUI builds the same steps or feeds the same machine methods |
| `boards` | `MachineOptions::usb_host`, the USB host at power-on | the profile turns options into a `SocConfig` |
| `emu-cli` | `--console-script` (read with the bounded reader of SP1b-3c, `script_file.rs`), `--usb-host`, the summary lines and exit code 5 | options, file reading, printing only |

Input enters the machine as dated chunks the peripherals keep in queues and wake up for (ADR 0003 `Deadline`), never as host time. A console script dates its steps from what the machine sent: the channels it waits on are watched, so each byte they send ends a call of the machine at the instruction boundary it was sent at (at its very instant while the CPU sleeps), and the steps after a `wait-for` are dated from that instant and handed over at once (C21). The slicing of the CLI therefore never reaches the device time of an input.

**Tech Stack:** Rust 1.99.0 (edition 2024); one new workspace crate, `console-script`, with dependencies already in `Cargo.lock` (`emu-core`, `soc-esp32c3`, `thiserror`; tests: `boards`, `sha2`, `cpu-rv32`, `svd-regs`); no new third-party package. The reference firmware tooling of `tools/reference-firmware` and the Docker image `espressif/idf:v5.5.2`, which only the CI runs.

**Spec:** `docs/superpowers/specs/2026-10-11-inkemu-design.md` (§3.1 stamped inputs, §6.4 scripts, §7 debug console, SP1 items 8–9). Facts: `docs/research/uart-reception.md` (**UR**), `docs/research/usb-serial-jtag-host.md` (**USJ**), `docs/research/esp32c3-interrupts-and-reset.md` (**IR**). ADR 0003 (peripheral context), ADR 0004 (inputs of a run), ADR 0006 (written by Task 11). Framing decisions C1–C12 (below), given to this plan.

**What was verified, and where.** Every code block of this plan was compiled and its tests run in a scratch work tree outside the repository, one commit per task, in the order of the tasks. On that host, by the rule of this plan (C11), only `cargo fmt --all --check`, `cargo clippy` on the touched crates (`--all-targets --all-features -- -D warnings`) and narrow `cargo test -p <crate> <filters>` were run, with `CARGO_BUILD_JOBS=4`: the expected outputs of the steps are those runs. Each mutation in the tables was applied to the scratch tree and the named test failed. The blocks of each task, applied in order to the commit before, rebuild the commit of that task exactly (`Cargo.lock` aside, which cargo updates): this was checked mechanically for the 11 tasks. **Not run on that host**, and left to the CI: `cargo test --workspace`, the `#[ignore]` suites (real ROM, reference firmware), coverage, Docker builds. The values only the CI can produce are listed in the next section, with the step that reads each one.

**How to read the code blocks.** A block headed `--- a/… +++ b/…` is a unified diff against the file as the previous step left it: apply it with the editor, hunk by hunk (the unmarked lines are context to find the place). "Create `path`:" introduces the whole content of a new file. "Create `path` with its tests only:" introduces the `#[cfg(test)] mod tests` of a new module, and "Put this above the tests of `path`:" the code that goes in front of it, in the same file, a blank line between them. A step "Write the failing tests" gives test modules, test files and the test hunks of existing modules; the step after it gives the code. `Cargo.lock` is never shown: cargo updates it.

## Values only the CI can produce

None of these is invented by this plan; each is read from a CI log by the step named, then committed with the reason.

| Value | Where it is pinned | Why it changes | Step that reads it |
|---|---|---|---|
| Trace hash `10174cc2726c3d77`, device time `11_396_706` us, `26562395` instructions | `crates/emu-cli/tests/run_cli.rs`, `x4_runs_the_reference_firmware_along_its_pinned_trace` | the ROM mirrors its console to USB-Serial-JTAG; the IN endpoint now waits for the host to take each packet (C5) | Task 5, Step 7 |
| Trace hash `c6b24a1cc226ed4a`, `11_403_054` us, `26637404` instructions | same file, `x4_runs_the_reference_firmware_from_its_application_image_along_its_pinned_trace` | same | Task 5, Step 7 |
| `ROM_UART_WAIT_PC` (`0x4004_c0d4`) and the device-time windows of the watchdog tests | `crates/soc-esp32c3/tests/real_rom.rs` | the ROM spends device time on USB-Serial-JTAG before its UART wait; the PC a watchdog finds may move | Task 5, Step 7 (only if a test of the `rom-boot` job fails) |
| `SHA256SUMS.inputs-uart-select`, `SHA256SUMS.inputs-usj-echo`, `SHA256SUMS.inputs-usj-console` | `tools/reference-firmware/` | three new builds (C9) | Task 10, Step 6 |
| The window 4,990–5,020 ms of milestone 1, the 7 dump rows of milestone 2 | `crates/console-script/tests/reference_firmware.rs` | the first runs of the firmwares under the model | Task 10, Step 7 |

The two notes that quote the old hashes, `docs/superpowers/notes/2026-10-12-sp1b-3b-carryover.md` (lines 13, 101) and `docs/superpowers/notes/2026-10-13-sp1b-3c-carryover.md` (line 8), are dated records and are not edited; the hand-over note of this plan says which values changed and why (Task 11).

## Global Constraints

- Toolchain pinned: Rust **1.99.0**, edition **2024**. Workspace lints unchanged: `unsafe_code = "forbid"`; `unwrap_used`/`expect_used` denied outside tests (an invariant is documented with `// INVARIANT:`).
- No `TODO`, no placeholder, no `#[allow]` added to hide a lint.
- Coverage ≥ 80 % for the workspace (`cargo llvm-cov --workspace --fail-under-lines 80`, in the CI).
- Licence Apache-2.0; `cargo deny check` passes; **no third-party package is added**. The new crate `console-script` depends on workspace crates and on `thiserror`, already in `Cargo.lock`.
- Register addresses and names come from `svd-regs` (`svd_regs::esp32c3::addr("PERIPHERAL", "REGISTER")`, or the `reg(name)` helper of each peripheral), never from a literal.
- Every user-facing message states **the cause, then the remedy**, separated by a semicolon; each new error enum has a test that every variant does.
- **Untrusted input never panics and is bounded.** The firmware's register values and the console script are both untrusted. **No loop and no allocation is sized by a value the firmware or the script supplies without a constant bound**: the receive FIFO holds 128 bytes; a channel holds at most `MAX_PENDING_HOST_BYTES` bytes of the host and the machine `MAX_PENDING_HOST_CHANGES` changes of the host; a script is bounded by the `MAX_*` constants of `console-script/src/parse.rs`. Each task that takes such a value says where its bound is.
- Determinism: same inputs → same consoles, same trace hash. No wall clock, no host randomness, no `HashMap` iteration on the emulated path. **The device time of an input is fixed by the inputs alone, never by the slicing of the run** (C6), and a test of each layer proves it (Tasks 6, 7, 10).
- `soc-esp32c3`, `firmware`, `boards` and `console-script` perform **no host I/O**: their `clippy.toml` (four equal copies, held by `crates/boards/tests/host_io_ban.rs`) bans it, and they build for `wasm32-unknown-unknown` (a step of the CI job `check`).
- **GUI-first.** No logic in `emu-cli` beyond option parsing, file reading and printing: the script's format, its refusals and its run belong to `console-script`; the inputs and the host state to `soc-esp32c3`.
- Each hardware or ESP-IDF behaviour a model relies on cites its source in a comment (report tag + section). An assumed behaviour is marked `ASSUMPTION:` with what would settle it, and is listed in the hand-over note (Task 11): 87 lines in `.rs` files at the base, 101 at the end of this plan.
- **A test proves only what a mutation makes fail.** Each task ends with a table of mutations: apply one, run the command, see the named test fail, undo it, and `touch` the file you restored (cargo judges a source by its modification time). Do not commit a task whose mutation survives.
- **No test bounded by wall-clock time when the work can be counted.** The only time bound added is `within` (an existing helper of `test_support`) around a call that must return, so that a broken driver fails instead of hanging the suite (Task 9).
- Code, tests, commit messages, the README, the ADR and the CLI never cite the decisions table of this plan, a review or a finding: they cite reports, ADRs, the spec and the code. Only the plan refers to the table (by its `C` numbers).
- Source files are changed with the editor (read, then edit the named lines), never by a script that rewrites them.
- Checking a command: redirect its output to a file, then read `$?`; never read an exit code through a pipe. Use `/usr/bin/git`. Run Python with `-I`.
- Commits: `type(scope): description`, only the paths of the task (`/usr/bin/git commit -- <paths>`, after `/usr/bin/git add` of the new files). **A commit message carries no trailer and no attribution line.**
- **No heavy acceptance on the development host** (C11). Locally: `cargo fmt --all --check`, `cargo clippy -p <touched crates> --all-targets --all-features -- -D warnings`, `cargo check`, and narrow `cargo test -p <crate> <filters>`, with `CARGO_BUILD_JOBS=4`. Never `cargo test --workspace`, the `#[ignore]` suites, coverage, Docker or QEMU on that host: push the branch, open the pull request early, and read the CI logs (not only the status) after each push. The CI clippy step is `tools/ci/clippy.sh --workspace --all-targets --all-features -- -D warnings`.
- Formatting and lints before **every** commit: `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy -p <crates of the task> --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`, all three `exit=0`. The code blocks of this plan are formatted as `rustfmt` leaves them.
- The ignored tests need `INKEMU_ROM_ELF_DIR` (the pinned esp-rom-elfs archive) and, for the reference images, the variables of `tools/reference-firmware/README.md` (three new ones from Task 10: `INKEMU_UART_SELECT_APP`, `INKEMU_USJ_ECHO_APP`, `INKEMU_USJ_CONSOLE_APP`), as absolute paths. A task that adds an ignored test updates the count of `.github/workflows/ci.yml` in the same commit.
- **Exit codes of `inkemu run`:** 0, 2, 3, 4 and 130, and 5 from Task 8 (C25); no other task adds one.
- Documents for people (ADR 0006, the hand-over note) are in French, with all accents. README (English, then French), code, CLI output and this plan are in English.

## Review Focus

The input classes most likely to bite, each with the test that holds it and the mutation a final reviewer should try (apply, run the command of the task, see the test fail, undo, `touch` the file):

- **Hostile register values from the firmware.** A frame with no stop bit (`STOP_BIT_NUM` = 0, defined by no source), loopback, an inverted receive line, autobaud, a FIFO memory of another size, RS485: each is refused by name and the model goes on with a defined behaviour → `a_frame_without_a_stop_bit_is_refused_and_timed_with_one`, `loopback_an_inverted_receive_line_autobaud_and_no_stop_bit_are_refused_by_name` (Task 1), `the_registers_of_the_driver_are_storage_and_rs485_or_another_fifo_size_are_refused` (Task 2). A read of an empty FIFO, a write to a full IN endpoint: 0 or nothing, and a named refusal → `a_read_of_an_empty_receive_fifo_returns_0_and_is_refused_by_name` (Task 2), `a_byte_written_while_a_packet_waits_for_the_host_is_refused_and_lost` (Task 5). An interrupt enable with bits above the 19 of USB-Serial-JTAG → `hwcdc_begin_takes_its_first_interrupt_from_the_reset_value_of_serial_in_empty` (Task 4). Mutations: `if (conf0 >> STOP_BIT_NUM_SHIFT) & TWO_BITS == 0 {` → `if false {` (uart.rs); `self.int_ena = value & INTERRUPT_BITS;` → `self.int_ena = value;` (usb_serial_jtag.rs).
- **Hostile script files.** A script is untrusted text: every limit (file, line, steps, send, text, total) is held at its value and one past it; a byte that is not UTF-8, an escape that is not one, a duration that overflows device time, a trailing word: refused at their line and column, cause then remedy → `each_limit_is_held`, `every_refusal_names_its_line_and_column`, `every_refusal_of_a_string_or_a_duration_names_its_line_and_column`, `a_text_that_is_not_utf8_is_refused_where_it_stops_being_utf8` (Task 7); through the binary, before any ROM is read → `a_refused_console_script_exits_2_at_its_line_and_column_before_any_rom_is_read` (Task 8). Mutations: `if bytes.len() > limit {` → `if bytes.len() > limit + 1 {`; `if total > MAX_TOTAL_BYTES {` → `+ 1` (parse.rs).
- **Inputs dated in the past.** A chunk or a change of host dated before the device time is refused with both instants and changes nothing; changes given out of order apply in the order of their dates → `an_input_dated_before_the_device_time_is_refused_and_changes_nothing` (Task 6), `changes_of_the_host_given_out_of_order_apply_in_the_order_of_their_dates` (Task 5). A script never dates in the past: its step time is the device time of the byte it waited for. Mutations: `if at < now {` → `if at < now && false {` (machine.rs); `.partition_point(|&(other, _)| other <= date)` → `.len()` (usb_serial_jtag.rs).
- **Slicing dependence.** The same inputs cut into calls of 1, 7, 997, 7,919 or a million cycles, watched or not, give the same consoles, trace hash, device time and instruction count → `a_run_with_inputs_is_the_same_whatever_its_calls_and_whether_it_is_watched` (Task 6), `the_steps_are_dated_the_same_whatever_the_calls_the_run_is_cut_into` (Task 7), `uart_select_reference_firmware_typed_into_is_the_same_run_whatever_the_slices` (Task 10, CI). The exact instant a watched byte reaches the host → `a_watched_channel_ends_the_call_at_the_instant_each_byte_reaches_the_host`, `a_watched_byte_ends_the_call_at_the_first_instruction_boundary_after_it_leaves` (Task 6). Mutations: `.filter(|_| self.watched);` → `.filter(|_| false);` (uart.rs); delete `self.step_time = now;` (console-script run.rs); delete `machine.watch_console(channel, true);` (same).
- **Resets in the middle of a transfer.** A core reset empties the receive FIFO, the endpoint FIFOs and the interrupt state; the host goes on sending what it had not sent, and its state is kept → `a_reset_empties_the_receive_fifo_and_the_host_goes_on_sending` (Task 2), `a_core_reset_empties_the_block_and_the_host_goes_on_sending` (Tasks 4, 5), `uart1_drives_source_22_and_a_reset_lowers_it` (Task 3); bytes dropped by a FIFO reset wake nothing → `the_wake_up_of_bytes_dropped_by_a_fifo_reset_finds_nothing_to_do` (Task 1). Mutation: `let host = std::mem::take(&mut self.host);` → `let host = VecDeque::new();` (usb_serial_jtag.rs).
- **Input without end.** A front end that feeds a channel faster than the firmware reads it gets an error at one mebibyte, and a zero-length packet counts as a byte → `a_channel_holds_a_megabyte_of_the_host_at_most` (Task 6). Mutations: `if pending > MAX_PENDING_HOST_BYTES {` → `+ 1`; `.saturating_add(bytes.len().max(1));` → `.saturating_add(bytes.len());`.

---

## Design decisions taken for this plan

C1–C12 were given to this plan and bind it; they are summarised here with what the plan does with each. C13 onward were taken while writing it.

| # | Decision | Where | Cost if wrong |
|---|---|---|---|
| C1 | Scope: UART reception, UART interrupts, frame timing both ways, USB-Serial-JTAG both ways with interrupts, SOF and a host model, dated input from a script file, the shared console driver of the tests and their access to `test_support`, the UART rows of the accepted-without-effect table. Out: typing from the terminal, scenarios, record and replay, CrossPoint, JTAG; autobaud, RS485, loopback, an inverted line are refused by name. | Tasks 1–11 | — |
| C2 | UART registers: `FIFO` pops (empty → 0 and a named diagnostic), `STATUS.RXFIFO_CNT`, `INT_RAW`/`INT_ST`/`INT_ENA`/`INT_CLR` with effect on bits 0, 1, 4, 8, 14; `CONF0` (receive FIFO reset, frame format, refusals); `CONF1`; `MEM_CONF` (`RX_TOUT_THRHD`, one block only); storage for `IDLE_CONF`, `RS485_CONF` (non-zero refused), `AT_CMD_CHAR`; `MEM_RX_STATUS` at its reset value. The UART rows leave the table. | Tasks 1–3 | Low |
| C3 | Sources 21 and 22, level, driven after every change; a condition that still holds after `INT_CLR` raises its bit again (`ASSUMPTION:`, UR A1); overflow drops the new byte and is reported (UR A2); `RXFIFO_TOUT` needs a byte in the FIFO and an idle line, once per idle period (UR A3). | Task 3 | Low |
| C4 | A frame lasts `1 + data + parity + stop` bit times both ways, half stop bits kept exact; bytes of a chunk are sent back to back from its date and land one frame after their start; lost while the receiver's clock is off or its FIFO in reset; no gap from `TX_IDLE_NUM`, so 8N1 transmit timing and the UART-only trace hashes do not change. | Tasks 1, 2 | Low |
| C5 | USB-Serial-JTAG: `EP1` both ways, `EP1_CONF`, the interrupt registers, source 26, bits SOF, `SERIAL_OUT_RECV_PKT`, `SERIAL_IN_EMPTY` (reset 1), `USB_BUS_RESET`, `OUT_EP1_ZERO_PAYLOAD`; storage registers at their reset values; `FRAM_NUM`; SOF computed from device time, an event at a frame start only while `INT_ENA.SOF` is set; a committed IN packet is taken by an open host 50 us later (`HOST_LATENCY`); host packets of at most 64 bytes; the core reset checked against IR §4 (C20). Trace hashes re-pinned from the CI. | Tasks 4, 5 | Medium: the re-pinned hashes |
| C6 | Two new inputs, the USB host and dated console input, entered by methods of `Esp32C3`; a date in the past refused by a typed error; the device time of an input independent of the slicing, tested; ADR 0006. | Tasks 6, 11 | Low |
| C7 | A script file read by a library crate; no stdin forwarding; `--usb-host`; the summary prints the host and the script's progress. | Tasks 7, 8 | Low |
| C8 | The script format (`send`, `wait`, `wait-for`, `usb-host`), escapes, no translation, limits as tested constants, line-and-column refusals, a version line, a distinct exit code. | Tasks 7, 8 | Low |
| C9 | Reference firmwares: `uart_select`, `usb_serial_jtag_echo` (the build script merges an example's own `sdkconfig.defaults` after the reference one), our `usb_serial_jtag_console`; applications booted behind the shipped bootloader; hashes pinned after two clean builds in the CI; new variables. | Task 10 | Medium: first CI builds |
| C10 | Unit tests per register sequence; parser and resolver tests; slicing tests; CLI tests; one console driver shared by the integration tests, which reach `test_support`. | Tasks 1–10 | Low |
| C11 | The constraints of the SP1b-3c plan, and no heavy acceptance on the development host. | All | — |
| C12 | Hand-over note, ADR 0006, README, cartography. | Task 11 | — |
| C13 | **UART time is kept in 1/32 of a time-base cycle** (`SUBCYCLES`): a half bit at 115,200 baud on the 40 MHz crystal is 22,220/32 cycles, so 1.5 stop bits and every bit are exact over a whole line, and an 8N1 byte rounds to the 13,888 cycles of before. | Task 1 | Low |
| C14 | **The host of a UART sends at the UART's own rate and frame** (`ASSUMPTION:`): a mismatched host would need corrupted bytes and framing errors, which no milestone needs. | Task 2 | Low |
| C15 | **Overflow, a read of an empty FIFO (UART and USB) and a write to a full IN endpoint are `Unsupported` diagnostics**: a warning in a lenient run, a stop under `--strict-mmio`, so that a CI run notices a firmware that loses bytes. | Tasks 2, 4, 5 | Low |
| C16 | **`TX_DONE` is raised once, when the last byte leaves**, and not again while idle; `RXFIFO_FULL` and `TXFIFO_EMPTY` are levels (C3). A timeout is armed only while `RX_TOUT_EN` is set; at the instant a byte starts and a timeout is due, the timeout comes first. | Task 3 | Low |
| C17 | **The USB host at power-on is a field of `SocConfig`** (`usb_host`, default `Open`), attached at device time 0 without a bus reset (`ASSUMPTION:`): a host plugged before power-on enumerates the device as it starts. | Task 5 | Low |
| C18 | **Host packets wait while the host is not open**; `WR_DONE` while a committed packet waits for the host does nothing (`ASSUMPTION:`). | Tasks 4, 5 | Low |
| C19 | **At one instant, a change of host comes first, then the host takes an IN packet, then an OUT packet loads.** | Task 5 | Low |
| C20 | **A core reset resets USB-Serial-JTAG** (FIFOs, packets, interrupt state), and the host keeps its state and goes on sending. C5 asked to check the block against IR §4; see "Corrections" below. | Tasks 4, 5 | Low |
| C21 | **The exact instant of a `wait-for` comes from watched channels** (`Esp32C3::watch_console`, `StopReason::ConsoleOutput`): a byte sent on a watched channel ends the call at the instruction boundary it left at (at its instant while the CPU sleeps). A UART schedules an event per byte only while watched; a USB packet is already an event. Rejected: dating past inputs at "now" (a clamped input could overtake a packet the host takes at the same instant, so the result would depend on the slicing). | Task 6 | Low |
| C22 | **Bounds of pending input:** `MAX_PENDING_HOST_BYTES` = 1 MiB per channel (a zero-length packet counts as one), `MAX_PENDING_HOST_CHANGES` = 16,384; a script holds at most 512 KiB of sends and 10,000 steps, below both, checked by `const` assertions. | Tasks 6, 7 | Low |
| C23 | **The script is a new library crate, `console-script`**, not part of `soc-esp32c3` (which knows devices, not scripts), `boards` (profiles) or `emu-cli` (a binary with host I/O, which SP2 does not link). It has the host-I/O ban of the model crates and builds for WebAssembly. | Task 7 | Low |
| C24 | **Step times are in cycles of the time base**, durations rounded up (`ceil(ns × 160 / 10⁹)`); a `wait-for` searches what its channel sent from the time of its step on; the steps up to the next `wait-for` are handed to the machine at once; a channel stays watched only while a `wait-for` is to come. | Task 7 | Low |
| C25 | **Exit code 5**: the run would have exited 0 and its script was not through. Any other code is kept (a strict stop, a CPU waiting for ever, a console or a flash file that could not be written say more). | Task 8 | Low |
| C26 | **Summary lines:** `inkemu: USB host <state>` in every run; with a script, `inkemu: console script through (N steps)` or `not through: line L (step) …` with a remedy. | Task 8 | Low |
| C27 | **The integration tests of `soc-esp32c3` reach `test_support` through a dependency of the crate on itself with the feature.** The 3b note (line 40) warned that an example compiled under `--workspace` and not under `-p`: with the self dependency both commands turn the feature on, and a test (`test_support_is_test_only.rs`) refuses `test_support` and `from_flat` in the documentation examples, which would otherwise pass in tests and fail for a user. | Task 9 | Low |
| C28 | **The shared driver is `test_support::drive`** with `Consoles`, `Bounds`, `Driven`; the four console loops of `real_rom.rs` and `reference_boot.rs` (`boot`, `flash_boot`, `run_to_resets`, `run_image`) and the countdown loop use it; the step-by-step loop that seeks a PC is not a console loop and stays. | Task 9 | Low |
| C29 | **The milestone tests live in `crates/console-script/tests/reference_firmware.rs`** (they need every console, the script run and the machine, and two slicings), and read the pinned hashes from the `SHA256SUMS` files of the build: one place per hash. The applications leave the container alone; a firmware whose sums file does not exist is built, its hashes printed under `unpinned <file>:`, and the script stops before writing anything. | Task 10 | Low |
| C30 | **A project without a `sdkconfig.defaults` of its own is built exactly as before** (the reference file copied in); only a project with one gets the list `reference;own`. The hashes pinned before this plan cannot move. | Task 10 | Low |

### Corrections to C1–C12

- **C5, the core reset of USB-Serial-JTAG.** C5 points to USJ O.12, which reads IR §4 as "a core reset keeps the block". IR §4.2's row that keeps the USB-Serial-JTAG state is that of the CPU reset (`esp_restart`, `rst:0xc`), not the core reset; the TRM says a core reset resets "the whole digital system" but the RTC domain. inkemu calls `UsbSerialJtag::reset` on the core reset only, so the block is reset with it (C20), with an `ASSUMPTION:` line and the test `a_core_reset_empties_the_block_and_the_host_goes_on_sending`. Nothing to fix in the code; the reading of O.12 is what changes.
- **UR §3.3, the register values of the QEMU runs.** `MEM_CONF` = `0x00640000` and `CONF1` = `0x00200001`/`0x00200078` are read back from QEMU, which returns 0 for bits it does not store; the driver writes `0x0064_0012` and `0x0020_C078`, which is what the tests of Task 3 use. A model that copied the QEMU values would refuse the FIFO size (`MEM_CONF` bits [3:1] = 0) of every ESP-IDF driver.
- **C8, the exit code of a script not through "when the run ends (budget, idle, Ctrl-C)".** A CPU that waits for ever keeps 4 and a strict stop 3 (C25): they say more about the run than "the script was waiting".

## File Structure

```
.github/workflows/ci.yml                                  WebAssembly check of console-script (Task 7); counts and variables (Tasks 8, 10)
Cargo.toml                                                the member and dependency console-script (Task 7)
README.md                                                 run help and exit code 5 (Task 8); reference firmware (Task 10); typing into the consoles, EN then FR (Task 11)
docs/adr/0006-entrees-des-consoles.md                     new, French (Task 11)
docs/adr/0004-entrees-d-une-execution-et-fichier-de-flash.md   points to 0006 (Task 11)
docs/registers-accepted-without-effect.md                 the UART rows leave (Tasks 1, 3)
docs/superpowers/notes/2026-10-14-sp1b-3d-carryover.md    new, French (Task 11)
tools/ci/clippy.sh                                        message for four copies (Task 7)
tools/reference-firmware/                                 build rule, three applications, README (Task 10)
  usb_serial_jtag_console/                                new: our application (Apache-2.0)
crates/soc-esp32c3/
  Cargo.toml                    itself as a dev-dependency with test-support (Task 9)
  clippy.toml                   the comment names console-script (Task 7)
  src/diag.rs                   UartFeature, UartRxOverflow, UartEmptyRead (Tasks 1, 2); UsbEmptyRead, UsbInFull (Tasks 4, 5)
  src/periph/uart.rs            frame timing (Task 1); reception (Task 2); interrupts (Task 3); watch (Task 6)
  src/periph/usb_serial_jtag.rs both directions and interrupts (Task 4); host, SOF, drains (Task 5); has_left (Task 6)
  src/periph/mod.rs, src/ctx.rs, src/bus.rs, src/config.rs, src/lib.rs   plumbing (Tasks 2–6)
  src/machine.rs                take_tx (Tasks 2, 4); inputs, watch, ConsoleOutput (Task 6); index (Task 9)
  src/test_support.rs           Consoles, Bounds, Driven, drive (Task 9)
  tests/accepted_without_effect.rs   PLANS_TO_COME loses SP1b-3d (Task 3)
  tests/real_rom.rs, tests/reference_boot.rs   the shared driver (Task 9)
  tests/test_support_is_test_only.rs           new (Task 9)
crates/console-script/          new (Task 7): Cargo.toml, clippy.toml, src/{lib,parse,run}.rs; tests/reference_firmware.rs (Task 10)
crates/boards/
  clippy.toml, tests/host_io_ban.rs   four copies (Task 7)
  src/lib.rs                    MachineOptions::usb_host (Task 8)
crates/firmware/clippy.toml     same comment (Task 7)
crates/emu-cli/
  Cargo.toml                    console-script (Task 8)
  src/run.rs                    ConsoleOutput continues (Task 6); execute_with_script, exit_code_with_script, summary (Task 8)
  src/main.rs, src/exit.rs, src/lib.rs, src/script_file.rs (new)   options, code 5, reading (Task 8)
  tests/run_cli.rs              options through the binary (Task 8); re-pins (Task 5)
```

---
## Task 1: A UART byte lasts the frame `CONF0` sets, and the modes the model lacks are refused by name

Until now every UART byte lasted 10 bit times. A byte lasts its start bit, its 5 to 8 data bits (`BIT_NUM`), its parity bit when `PARITY_EN` is set, and 1, 1.5 or 2 stop bits (`STOP_BIT_NUM`), in both directions (UR §6, C4). Half a stop bit is kept exact over a whole line: the UART counts its time in 1/32 of a cycle of the time base (C13), so 8N1 at 115,200 baud is still 13,888 cycles a byte and no pinned trace moves. A change of `CONF0` applies to the bytes written after it. The modes the model does not have are refused by name when set (`Unsupported::UartFeature`): loopback, an inverted receive line, autobaud, and no stop bit (`STOP_BIT_NUM` = 0, defined by no source: timed with one, `ASSUMPTION:`). `CONF0` leaves the accepted-without-effect table: its effect is modelled.

**Files:**
- Modify: `crates/soc-esp32c3/src/diag.rs`, `crates/soc-esp32c3/src/periph/uart.rs`, `docs/registers-accepted-without-effect.md`

**Interfaces:**
- Consumes: `Ctx::report`, `Unsupported`, the `CLKDIV`/`CLK_CONF` arithmetic of `uart.rs` (`DIVIDER_FRACTION_BITS`).
- Produces: `pub enum UartFeature { Loopback, InvertedRx, Autobaud, NoStopBit, FifoSize, Rs485 }` (`FifoSize` and `Rs485` are used from Task 2); `Unsupported::UartFeature { uart: u8, feature: UartFeature }`, keyed once per UART and feature (key 23); `fn frame_half_bits(conf0: u32) -> u64` and `const SUBCYCLES: u64 = 32` (private to `uart.rs`).

**Bounds on untrusted input.** `CONF0` is a firmware value: `frame_half_bits` reads three 2-bit fields and one bit, at most 24 half bits; no loop depends on it.

- [ ] **Step 1: Write the failing tests**

```diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -759,6 +759,10 @@
                 uart: 1,
                 sclk_sel: 2,
             }),
+            unsupported(Unsupported::UartFeature {
+                uart: 0,
+                feature: UartFeature::Loopback,
+            }),
             unsupported(Unsupported::FlashProgramLength { addr: 0x4100_9000 }),
             unsupported(Unsupported::FlashCommandBits { cmd: 0x0300_0000 }),
             unsupported(Unsupported::FlashSuspend { sus_ctrl: 0x20 }),
@@ -909,6 +913,24 @@
                 "UART{uart}, SCLK_SEL {sclk_sel}"
             );
         }
+    }
+
+    #[test]
+    fn a_refused_uart_feature_is_listed_once_per_uart_and_feature() {
+        let feature = |uart, feature, pc| Diagnostic::Unsupported {
+            what: Unsupported::UartFeature { uart, feature },
+            pc,
+        };
+        let mut d = Diagnostics::new(false);
+        d.record(feature(0, UartFeature::Loopback, 0x4200_0000));
+        d.record(feature(0, UartFeature::Loopback, 0x4200_0010));
+        d.record(feature(1, UartFeature::Loopback, 0x4200_0000));
+        d.record(feature(0, UartFeature::Autobaud, 0x4200_0000));
+        assert_eq!(d.distinct(), 3, "per UART and feature, whatever the pc");
+        assert_eq!(
+            d.occurrences(&feature(0, UartFeature::Loopback, 0x4200_0020)),
+            2
+        );
     }
 
     #[test]
@@ -1125,13 +1147,54 @@
             ),
         ];
         for (diagnostic, needle) in texts {
-            let text = diagnostic.to_string();
-            assert!(text.contains(needle), "{text}");
-            assert!(text.contains("pc 0x40"), "{text}");
-            let (cause, remedy) = text
-                .split_once("; ")
-                .unwrap_or_else(|| panic!("no remedy in: {text}"));
-            assert!(!cause.is_empty() && !remedy.is_empty(), "{text}");
+            assert_cause_then_remedy(&diagnostic, needle);
+        }
+    }
+
+    fn assert_cause_then_remedy(diagnostic: &Diagnostic, needle: &str) {
+        let text = diagnostic.to_string();
+        assert!(text.contains(needle), "{text}");
+        assert!(text.contains("pc 0x40"), "{text}");
+        let (cause, remedy) = text
+            .split_once("; ")
+            .unwrap_or_else(|| panic!("no remedy in: {text}"));
+        assert!(!cause.is_empty() && !remedy.is_empty(), "{text}");
+    }
+
+    #[test]
+    fn the_refusals_of_the_serial_ports_state_the_cause_then_the_remedy() {
+        let texts = [
+            (
+                Unsupported::UartFeature {
+                    uart: 1,
+                    feature: UartFeature::Loopback,
+                },
+                "UART1 was set to loop its transmitter back to its receiver (CONF0.LOOPBACK, pc 0x40381000), which inkemu does not model",
+            ),
+            (
+                Unsupported::UartFeature {
+                    uart: 0,
+                    feature: UartFeature::InvertedRx,
+                },
+                "(CONF0.RXD_INV, pc 0x40381000)",
+            ),
+            (
+                Unsupported::UartFeature {
+                    uart: 0,
+                    feature: UartFeature::Autobaud,
+                },
+                "(CONF0.AUTOBAUD_EN, pc 0x40381000)",
+            ),
+            (
+                Unsupported::UartFeature {
+                    uart: 0,
+                    feature: UartFeature::NoStopBit,
+                },
+                "(CONF0.STOP_BIT_NUM = 0, pc 0x40381000)",
+            ),
+        ];
+        for (what, needle) in texts {
+            assert_cause_then_remedy(&unsupported(what), needle);
         }
     }
 
```

```diff
--- a/crates/soc-esp32c3/src/periph/uart.rs
+++ b/crates/soc-esp32c3/src/periph/uart.rs
@@ -295,8 +295,9 @@
     /// What the ROM and the bootloader leave: the crystal undivided, 347 + 3/16 (BA §7).
     const CLKDIV_115200: u32 = 0x0030_015B;
     const CLK_CONF_XTAL: u32 = 0x0370_0000;
-    /// One byte at 115 200 baud: 10 x (347 x 16 + 3) x 4 sixteenths, 13 887.5 cycles, 86.8 us.
-    const BYTE_SUBCYCLES: u64 = 10 * (347 * 16 + 3) * 4;
+    /// One byte at 115 200 baud, 8N1: 20 half bits x (347 x 16 + 3) x 4 thirty-seconds,
+    /// 13 887.5 cycles, 86.8 us.
+    const BYTE_SUBCYCLES: u64 = 20 * (347 * 16 + 3) * 4;
 
     fn console(ctx: &mut Ctx) -> Uart {
         let mut uart = Uart::new(0, UART0);
@@ -692,6 +693,108 @@
         assert_eq!(uart.take_tx(ctx.now()), b"b");
     }
 
+    /// The instant, in cycles, at which the first byte written now to an idle transmitter has
+    /// left, for a frame of `half_bits` half bit times at 115 200 baud on the crystal.
+    fn gone_after(half_bits: u64) -> Cycles {
+        Cycles((half_bits * (347 * 16 + 3) * 4).div_ceil(SUBCYCLES))
+    }
+
+    #[test]
+    fn a_byte_lasts_its_start_bit_its_data_bits_its_parity_bit_and_its_stop_bits() {
+        // UR §5.1, §6: start + (5 + BIT_NUM) + PARITY_EN + stop (1, 1.5 or 2 for STOP_BIT_NUM 1,
+        // 2, 3). The ROM's `0x0400001C` and the reset value are 8N1: 10 bit times.
+        for (conf0, half_bits, format) in [
+            (0x0400_001C, 20, "8N1"),
+            (0x1000_001C, 20, "8N1, the reset value"),
+            (0x0400_001E, 22, "8E1"),
+            (0x0400_001F, 22, "8O1"),
+            (0x0400_003C, 22, "8N2"),
+            (0x0400_003E, 24, "8E2"),
+            (0x0400_0010, 14, "5N1"),
+            (0x0400_001A, 20, "7E1"),
+            (0x0400_002C, 21, "8N1.5"),
+        ] {
+            let ctx = &mut Ctx::default();
+            let mut uart = console(ctx);
+            assert!(uart.write(ctx, CONF0, conf0));
+            uart.write(ctx, FIFO, u32::from(b'f'));
+            assert_eq!(ctx.next_wake(), Some(gone_after(half_bits)), "{format}");
+            assert_eq!(ctx.take_unsupported(), None, "{format}");
+        }
+    }
+
+    #[test]
+    fn half_a_stop_bit_is_kept_over_a_line_of_bytes() {
+        // 8N1.5 at 115 200 baud: 10.5 bit times a byte; 128 bytes end at 128 x 10.5 bit times,
+        // not at 128 x 10 or 128 x 11.
+        let ctx = &mut Ctx::default();
+        let mut uart = console(ctx);
+        uart.write(ctx, CONF0, 0x0400_002C);
+        for byte in 0..128u32 {
+            uart.write(ctx, FIFO, byte);
+        }
+        ctx.sleep_until(gone_after(128 * 21));
+        assert_eq!(uart.take_tx(ctx.now()).len(), 128);
+        assert_eq!(
+            ctx.next_wake(),
+            Some(gone_after(21)),
+            "the first wake-up is kept"
+        );
+    }
+
+    #[test]
+    fn a_change_of_frame_format_applies_to_the_bytes_written_after_it() {
+        let ctx = &mut Ctx::default();
+        let mut uart = console(ctx);
+        uart.write(ctx, FIFO, u32::from(b'a'));
+        uart.write(ctx, CONF0, 0x0400_003E);
+        uart.write(ctx, FIFO, u32::from(b'b'));
+        ctx.sleep_until(gone_after(20));
+        assert_eq!(uart.take_tx(ctx.now()), b"a", "8N1, as written");
+        ctx.sleep_until(gone_after(20 + 23));
+        assert_eq!(uart.take_tx(ctx.now()), []);
+        ctx.sleep_until(gone_after(20 + 24));
+        assert_eq!(uart.take_tx(ctx.now()), b"b", "8E2: 12 bit times");
+    }
+
+    #[test]
+    fn loopback_an_inverted_receive_line_autobaud_and_no_stop_bit_are_refused_by_name() {
+        // UR §10: refused when set; the value is stored all the same.
+        for (bits, feature) in [
+            (1 << 14, UartFeature::Loopback),
+            (1 << 19, UartFeature::InvertedRx),
+            (1 << 27, UartFeature::Autobaud),
+        ] {
+            let ctx = &mut Ctx::default();
+            let mut uart = Uart::new(1, UART1);
+            assert!(uart.write(ctx, CONF0, 0x0400_001C | bits));
+            assert_eq!(
+                ctx.take_unsupported(),
+                Some(Unsupported::UartFeature { uart: 1, feature })
+            );
+            assert_eq!(uart.read(ctx, CONF0), Some(0x0400_001C | bits));
+            assert!(uart.write(ctx, CONF0, 0x0400_001C));
+            assert_eq!(ctx.take_unsupported(), None, "cleared: nothing to refuse");
+        }
+    }
+
+    #[test]
+    fn a_frame_without_a_stop_bit_is_refused_and_timed_with_one() {
+        // UR A4: STOP_BIT_NUM = 0 has no documented frame.
+        let ctx = &mut Ctx::default();
+        let mut uart = console(ctx);
+        uart.write(ctx, CONF0, 0x0400_000C);
+        assert_eq!(
+            ctx.take_unsupported(),
+            Some(Unsupported::UartFeature {
+                uart: 0,
+                feature: UartFeature::NoStopBit
+            })
+        );
+        uart.write(ctx, FIFO, u32::from(b's'));
+        assert_eq!(ctx.next_wake(), Some(gone_after(20)));
+    }
+
     #[test]
     fn the_wake_up_of_bytes_dropped_by_a_fifo_reset_finds_nothing_to_do() {
         let ctx = &mut Ctx::default();
@@ -706,8 +809,9 @@
         assert_eq!(ctx.take_due(), Some(Event::Uart(0)));
         uart.wake(ctx);
         assert_eq!(ctx.next_wake(), Some(last_gone));
-        uart.write(ctx, CONF0, TXFIFO_RST);
-        uart.write(ctx, CONF0, 0);
+        // The pulse of `Uart_Init`, with an 8N1 frame around it.
+        uart.write(ctx, CONF0, 0x0400_001C | TXFIFO_RST);
+        uart.write(ctx, CONF0, 0x0400_001C);
         ctx.sleep_until(last_gone);
         assert_eq!(ctx.take_due(), Some(Event::Uart(0)));
         uart.wake(ctx);
```

- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p soc-esp32c3 --lib -- periph::uart:: diag:: > /tmp/t1-red.log 2>&1; echo "exit=$?"; grep -E "^error" /tmp/t1-red.log | sort | uniq -c
```

Expected: `exit=101`; the test build stops with `could not compile soc-esp32c3 (lib test) due to 22 previous errors`: 14 × `error[E0433]: cannot find type UartFeature in this scope` and 8 × `error[E0599]: no variant named UartFeature found for enum diag::Unsupported`.

- [ ] **Step 3: The frame format, and the refusals**

```diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -72,6 +72,51 @@
     /// frequency for: the RC fast clock (2), or no documented source (0). The byte is timed with
     /// the crystal.
     UartClock { uart: u8, sclk_sel: u32 },
+    /// A UART field set to a mode the model does not have (report UR §10): the register keeps
+    /// the value, and the UART goes on as if the field were clear.
+    UartFeature { uart: u8, feature: UartFeature },
+}
+
+/// A UART mode the model refuses by name when software sets it (report UR §10, A4).
+#[derive(Clone, Copy, Debug, PartialEq, Eq)]
+pub enum UartFeature {
+    /// `CONF0.LOOPBACK`: the transmitter drives the receiver.
+    Loopback,
+    /// `CONF0.RXD_INV`: the receive line is inverted.
+    InvertedRx,
+    /// `CONF0.AUTOBAUD_EN`: the baud rate is detected from the line.
+    Autobaud,
+    /// `CONF0.STOP_BIT_NUM` = 0: a frame with no documented stop bit (report UR A4).
+    NoStopBit,
+}
+
+impl UartFeature {
+    /// The register field, then what it asks for, then what the model does instead and the
+    /// remedy, as the message says them.
+    const fn describe(self) -> (&'static str, &'static str, &'static str) {
+        match self {
+            UartFeature::Loopback => (
+                "CONF0.LOOPBACK",
+                "to loop its transmitter back to its receiver",
+                "its bytes still go to the host and its receiver hears only the host; clear CONF0.LOOPBACK",
+            ),
+            UartFeature::InvertedRx => (
+                "CONF0.RXD_INV",
+                "to invert its receive line",
+                "the host's bytes are received as they were sent; clear CONF0.RXD_INV",
+            ),
+            UartFeature::Autobaud => (
+                "CONF0.AUTOBAUD_EN",
+                "to detect its baud rate from the line",
+                "the baud rate stays the one CLKDIV sets; set the baud rate with CLKDIV",
+            ),
+            UartFeature::NoStopBit => (
+                "CONF0.STOP_BIT_NUM = 0",
+                "for a frame without a stop bit",
+                "its bytes are timed with one stop bit; write STOP_BIT_NUM 1, 2 or 3 (1, 1.5 or 2 stop bits)",
+            ),
+        }
+    }
 }
 
 impl From<FlashRefusal> for Unsupported {
@@ -200,6 +245,15 @@
                 f,
                 "RTC_CNTL.CLK_CONF selects an RTC slow clock inkemu does not model (value {clk_conf:#010x}, pc {pc:#010x}), so the RTC timer and the RTC watchdog keep counting the 136 kHz RC oscillator; select the RC oscillator (ANA_CLK_RTC_SEL = 0) or expect their durations to differ from the device"
             ),
+            Unsupported::UartOverflow { .. }
+            | Unsupported::UartClock { .. }
+            | Unsupported::UartFeature { .. } => self.describe_serial(pc, f),
+        }
+    }
+
+    /// The messages of the refusals of the serial ports, for [`Unsupported::describe`].
+    fn describe_serial(self, pc: u32, f: &mut fmt::Formatter<'_>) -> fmt::Result {
+        match self {
             Unsupported::UartOverflow { uart } => write!(
                 f,
                 "a byte was written to UART{uart} while its 128-byte transmit FIFO was full (pc {pc:#010x}), so inkemu dropped the byte; wait for room in the FIFO (UART.STATUS TXFIFO_CNT) before writing"
@@ -208,6 +262,15 @@
                 f,
                 "UART{uart} sent a byte with a source clock inkemu does not model (CLK_CONF.SCLK_SEL = {sclk_sel}, pc {pc:#010x}), so the byte was timed with the 40 MHz crystal and its duration differs from the device; select the crystal (SCLK_SEL = 3) or APB_CLK (SCLK_SEL = 1)"
             ),
+            Unsupported::UartFeature { uart, feature } => {
+                let (field, asked, instead) = feature.describe();
+                write!(
+                    f,
+                    "UART{uart} was set {asked} ({field}, pc {pc:#010x}), which inkemu does not model, so {instead}"
+                )
+            }
+            // INVARIANT: `describe` hands only the refusals above to this function.
+            _ => Ok(()),
         }
     }
 }
@@ -308,6 +371,7 @@
                 Unsupported::SlowClock { .. } => (9, 0, 0),
                 Unsupported::UartOverflow { uart } => (12, u32::from(uart), 0),
                 Unsupported::UartClock { uart, sclk_sel } => (13, u32::from(uart), sclk_sel),
+                Unsupported::UartFeature { uart, feature } => (23, u32::from(uart), feature as u32),
                 // A command is 1 to 16 bits long, so 0 is free for "no command phase".
                 Unsupported::FlashTransfer { command_bits, .. } => {
                     (11, command_bits.unwrap_or(0), 0)
```

```diff
--- a/crates/soc-esp32c3/src/periph/uart.rs
+++ b/crates/soc-esp32c3/src/periph/uart.rs
@@ -6,22 +6,18 @@
 //! §4.1). Reception and the UART interrupts arrive in SP1b-3d.
 use crate::clock::XTAL_PERIOD;
 use crate::ctx::{Ctx, Deadline, Event};
-use crate::diag::Unsupported;
+use crate::diag::{UartFeature, Unsupported};
 use crate::periph::{Accepted, Storage};
 use emu_core::{Cycles, Peripheral};
 use std::collections::VecDeque;
 
-/// `INT_CLR` is write-only and accepted; the three others are stored.
+/// `INT_CLR` is write-only and accepted; the two others are stored.
 pub(crate) fn accepted() -> Vec<Accepted> {
     let mut rows = Vec::new();
     for peripheral in ["UART0", "UART1"] {
         for (register, not_modeled) in [
             ("INT_ENA", "interrupt enables: nothing is raised"),
             ("INT_CLR", "interrupt clear: nothing is pending"),
-            (
-                "CONF0",
-                "frame format: a byte is 10 bit times (the transmit FIFO reset acts)",
-            ),
             ("CONF1", "FIFO thresholds"),
         ] {
             rows.push(Accepted::svd(peripheral, register, not_modeled, "SP1b-3d"));
@@ -77,14 +73,40 @@
 /// `CONF0` bit 18 `TXFIFO_RST`: empties the transmit FIFO (`ROM3:Uart_Init@0x4004c306` sets it,
 /// then clears it; BC §G.2).
 const TXFIFO_RST: u32 = 1 << 18;
-/// ASSUMPTION: a byte is 10 bit times on the wire (a start bit, 8 data bits, a stop bit), whatever
-/// `CONF0` says. The ROM and ESP-IDF consoles use this format; another one would only change how
-/// long a byte takes. SP1b-3d gives the frame format its effect, with the receive side; a byte
-/// timed on a chip configured for another format (two stop bits, a parity bit) settles what each
-/// field adds.
-const BITS_PER_BYTE: u64 = 10;
-/// Device time is kept in sixteenths of a time-base cycle here: a bit lasts a whole number of them.
-const SUBCYCLES: u64 = 16;
+/// The frame format of `CONF0` (report UR §5.1, from `uart_ll_set_parity`,
+/// `uart_ll_set_data_bit_num` and `uart_ll_set_stop_bits`, `IDF:hal/esp32c3/include/hal/uart_ll.h`):
+/// bit 1 `PARITY_EN` adds a parity bit; bits [3:2] `BIT_NUM` give 5 + `BIT_NUM` data bits;
+/// bits [5:4] `STOP_BIT_NUM` give 1, 1.5 or 2 stop bits for 1, 2 or 3.
+const PARITY_EN: u32 = 1 << 1;
+const BIT_NUM_SHIFT: u32 = 2;
+const STOP_BIT_NUM_SHIFT: u32 = 4;
+const TWO_BITS: u32 = 0b11;
+/// The modes of `CONF0` the model refuses by name when they are set (report UR §10).
+const CONF0_REFUSED: [(u32, UartFeature); 3] = [
+    (1 << 14, UartFeature::Loopback),
+    (1 << 19, UartFeature::InvertedRx),
+    (1 << 27, UartFeature::Autobaud),
+];
+/// Device time is kept in thirty-seconds of a time-base cycle here: a bit lasts a whole number of
+/// sixteenths (the fraction of `CLKDIV`), and a frame may end on half a bit (1.5 stop bits).
+const SUBCYCLES: u64 = 32;
+
+/// The length of a frame in half bit times: a start bit, the data bits, the parity bit, the stop
+/// bits (report UR §5.1, §6). `RS485_CONF.DL0_EN`/`DL1_EN` would each add a bit; the model
+/// refuses RS485 mode instead.
+///
+/// ASSUMPTION: `STOP_BIT_NUM` = 0, which no source defines (report UR A4), lasts one stop bit;
+/// it is refused by name when written. A frame timed on a chip set that way settles it.
+fn frame_half_bits(conf0: u32) -> u64 {
+    let data_bits = 5 + u64::from((conf0 >> BIT_NUM_SHIFT) & TWO_BITS);
+    let parity_bits = u64::from((conf0 & PARITY_EN) >> 1);
+    let stop_half_bits = match (conf0 >> STOP_BIT_NUM_SHIFT) & TWO_BITS {
+        2 => 3,
+        3 => 4,
+        _ => 2,
+    };
+    2 * (1 + data_bits + parity_bits) + stop_half_bits
+}
 
 #[derive(Debug)]
 pub struct Uart {
@@ -202,10 +224,11 @@
         }
     }
 
-    /// The time one byte takes, in sixteenths of a time-base cycle: 10 bits, each `CLKDIV` (with
-    /// its fraction) cycles of the source clock divided by `SCLK_DIV_NUM + 1`. The fractional
-    /// divider of the source clock (`SCLK_DIV_A/B`) is ignored: it resets to 0 and no measured
-    /// code writes it.
+    /// The time one byte takes, in thirty-seconds of a time-base cycle: the bits of a frame
+    /// ([`frame_half_bits`]), each `CLKDIV` (with its fraction) cycles of the source clock divided
+    /// by `SCLK_DIV_NUM + 1`. The fractional divider of the source clock (`SCLK_DIV_A/B`) is
+    /// ignored: it resets to 0 and no measured code writes it. The 10 bit times of 8N1 give the
+    /// durations the model gave every byte before it read the frame format.
     ///
     /// ASSUMPTION: the divider is what the register holds, down to 0. An integer part of 0 with
     /// a fraction divides by that fraction of a cycle, and a `CLKDIV` of 0 gives a byte no
@@ -215,10 +238,31 @@
     fn byte_time(&self, source_period: u64) -> u64 {
         let clkdiv = self.config.read(CLKDIV).unwrap_or(0);
         let clk_conf = self.config.read(CLK_CONF).unwrap_or(0);
-        let divider = u64::from(clkdiv & CLKDIV_INTEGER_MASK) * SUBCYCLES
+        let conf0 = self.config.read(CONF0).unwrap_or(0);
+        // The divider in sixteenths of a source-clock cycle, a frame in half bits: the product
+        // is in thirty-seconds.
+        let divider = u64::from(clkdiv & CLKDIV_INTEGER_MASK) * 16
             + u64::from((clkdiv >> CLKDIV_FRAG_SHIFT) & CLKDIV_FRAG_MASK);
         let source_divider = u64::from((clk_conf >> SCLK_DIV_NUM_SHIFT) & SCLK_DIV_NUM_MASK) + 1;
-        BITS_PER_BYTE * divider * source_divider * source_period
+        frame_half_bits(conf0) * divider * source_divider * source_period
+    }
+
+    /// Refuses by name the modes of `conf0` the model does not have (report UR §10, A4).
+    fn refuse_modes(&self, ctx: &mut Ctx, conf0: u32) {
+        for (bit, feature) in CONF0_REFUSED {
+            if conf0 & bit != 0 {
+                ctx.unsupported(Unsupported::UartFeature {
+                    uart: self.index,
+                    feature,
+                });
+            }
+        }
+        if (conf0 >> STOP_BIT_NUM_SHIFT) & TWO_BITS == 0 {
+            ctx.unsupported(Unsupported::UartFeature {
+                uart: self.index,
+                feature: UartFeature::NoStopBit,
+            });
+        }
     }
 
     fn send(&mut self, ctx: &mut Ctx, byte: u8) {
@@ -278,6 +322,7 @@
                     // The byte in the shift register is on the wire already: it goes on.
                     self.in_flight.truncate(1);
                 }
+                self.refuse_modes(ctx, value);
                 self.config.write(offset, value)
             }
             _ => self.config.write(offset, value),
```

```diff
--- a/docs/registers-accepted-without-effect.md
+++ b/docs/registers-accepted-without-effect.md
@@ -42,7 +42,6 @@
 |---|---|---|---|---|
 | `0x6000000c` | `UART0.INT_ENA` | 1 | interrupt enables: nothing is raised | SP1b-3d |
 | `0x60000010` | `UART0.INT_CLR` | 1 | interrupt clear: nothing is pending | SP1b-3d |
-| `0x60000020` | `UART0.CONF0` | 1 | frame format: a byte is 10 bit times (the transmit FIFO reset acts) | SP1b-3d |
 | `0x60000024` | `UART0.CONF1` | 1 | FIFO thresholds | SP1b-3d |
 | `0x60002008` | `SPI1.CTRL` | 1 | read mode, dummy output, WP level (the two-byte status write bit acts) | SP8 |
 | `0x6000200c` | `SPI1.CTRL1` | 1 | CS hold delay after a release-from-power-down | SP8 |
@@ -101,7 +100,6 @@
 | `0x6000e048` | `ANA_I2C.ANA_CONFIG2` | 1 | analog register bus: bytes are stored, the analog blocks (PLL, SAR ADC, bias) do nothing | SP2 |
 | `0x6001000c` | `UART1.INT_ENA` | 1 | interrupt enables: nothing is raised | SP1b-3d |
 | `0x60010010` | `UART1.INT_CLR` | 1 | interrupt clear: nothing is pending | SP1b-3d |
-| `0x60010020` | `UART1.CONF0` | 1 | frame format: a byte is 10 bit times (the transmit FIFO reset acts) | SP1b-3d |
 | `0x60010024` | `UART1.CONF1` | 1 | FIFO thresholds | SP1b-3d |
 | `0x6001ccd4` | `NRX.NRXPD_CTRL` | 1 | radio memory power: there is no radio | SP5a |
 | `0x6001d054` | `BB.BBPD_CTRL` | 1 | radio memory power: there is no radio | SP5a |
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p soc-esp32c3 --lib -- periph::uart:: diag:: > /tmp/t1.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t1.log
```

Expected: `exit=0` and `test result: ok. 55 passed; 0 failed; 0 ignored; 0 measured; 469 filtered out`.

- [ ] **Step 5: Mutations**

Each row: apply the change, run the command of Step 4, see the test fail, undo, `touch` the file.

| # | Mutation (`crates/soc-esp32c3/src/periph/uart.rs`) | Test that fails |
|---|---|---|
| 1 | delete `self.refuse_modes(ctx, value);` in `write` | `loopback_an_inverted_receive_line_autobaud_and_no_stop_bit_are_refused_by_name`, `a_frame_without_a_stop_bit_is_refused_and_timed_with_one` |
| 2 | `let parity_bits = u64::from((conf0 & PARITY_EN) >> 1);` → `let parity_bits = 0;` | `a_byte_lasts_its_start_bit_its_data_bits_its_parity_bit_and_its_stop_bits`, `a_change_of_frame_format_applies_to_the_bytes_written_after_it` |
| 3 | `if (conf0 >> STOP_BIT_NUM_SHIFT) & TWO_BITS == 0 {` → `if false {` | `a_frame_without_a_stop_bit_is_refused_and_timed_with_one` |

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy -p soc-esp32c3 --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git commit -m "feat(soc-esp32c3): a UART byte lasts the frame CONF0 sets, and the modes the model lacks are refused by name" -- crates/soc-esp32c3/src/diag.rs crates/soc-esp32c3/src/periph/uart.rs docs/registers-accepted-without-effect.md
```

Expected: three `exit=0`, then the commit.

---

## Task 2: The host's bytes land in a UART's receive FIFO one frame after they start, and the FIFO is read

The receive path of the UART (UR §2–§5, C2, C4). `Uart::receive(ctx, at, bytes)` queues the bytes of the host dated `at`; they are sent back to back from `max(at, the end of the previous byte)` at the UART's rate and in its frame (C14), each landing in the 128-byte receive FIFO one frame after its start bit, the frame read when the byte starts. The FIFO is computed lazily from device time; the UART asks to be called at each landing (`Deadline`). `FIFO` reads pop (empty → 0 and `Unsupported::UartEmptyRead`); `STATUS.RXFIFO_CNT` counts; a byte that finds 128 is lost and reported (`UartRxOverflow`, UR A2); a byte that lands while `CLK_CONF.RX_SCLK_EN` is clear or while `CONF0.RXFIFO_RST` is held is lost (UR A8); a core reset empties the FIFO and the host goes on sending what it had not sent. The driver's registers are storage (`IDLE_CONF`, `AT_CMD_CHAR`, `MEM_CONF`), `MEM_RX_STATUS` reads its reset value, and a non-zero `RS485_CONF` or another FIFO size than one block is refused by name. `reset` and `take_tx` take the context, which the receive path needs.

**Files:**
- Modify: `crates/soc-esp32c3/src/diag.rs`, `crates/soc-esp32c3/src/periph/uart.rs`, `crates/soc-esp32c3/src/periph/mod.rs`, `crates/soc-esp32c3/src/machine.rs`, `crates/soc-esp32c3/src/bus.rs`

**Interfaces:**
- Consumes: `UartFeature::{FifoSize, Rs485}` (Task 1), `Deadline::arm`, `Event::Uart(i)`.
- Produces: `pub fn Uart::receive(&mut self, ctx: &mut Ctx, at: Cycles, bytes: &[u8])`; `pub fn Uart::host_pending(&self) -> usize`; `pub fn Uart::reset(&mut self, ctx: &mut Ctx)` and `pub fn Uart::take_tx(&mut self, ctx: &mut Ctx) -> Vec<u8>` (they took no context before); `Unsupported::UartRxOverflow { uart: u8 }` (key 24), `Unsupported::UartEmptyRead { uart: u8 }` (key 25).

**Bounds on untrusted input.** The receive FIFO holds at most `FIFO_BYTES` (128); the host queue is bounded by the machine (Task 6, `MAX_PENDING_HOST_BYTES`). Settling lands at most the bytes queued, each once.

- [ ] **Step 1: Write the failing tests**

```diff
--- a/crates/soc-esp32c3/src/bus.rs
+++ b/crates/soc-esp32c3/src/bus.rs
@@ -835,7 +835,7 @@
         // A byte takes 10 bit times to leave the UART: 347 us at the reset baud rate (55 520 cycles
         // of the time base).
         b.ctx.sleep_until(emu_core::Cycles(60_000));
-        assert_eq!(b.periph.uart0.take_tx(b.ctx.now()), b"U");
+        assert_eq!(b.periph.uart0.take_tx(&mut b.ctx), b"U");
         assert_eq!(b.periph.usb_serial_jtag.take_tx(), b"J");
         assert_eq!(b.ctx.diag.drain(), []);
     }
```

```diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -1256,6 +1256,28 @@
                 },
                 "(CONF0.STOP_BIT_NUM = 0, pc 0x40381000)",
             ),
+            (
+                Unsupported::UartFeature {
+                    uart: 0,
+                    feature: UartFeature::FifoSize,
+                },
+                "(MEM_CONF.RX_SIZE or TX_SIZE other than 1, pc 0x40381000)",
+            ),
+            (
+                Unsupported::UartFeature {
+                    uart: 0,
+                    feature: UartFeature::Rs485,
+                },
+                "UART0 was set to RS485 mode (RS485_CONF other than 0, pc 0x40381000)",
+            ),
+            (
+                Unsupported::UartRxOverflow { uart: 1 },
+                "a byte the host sent found the 128-byte receive FIFO of UART1 full (pc 0x40381000)",
+            ),
+            (
+                Unsupported::UartEmptyRead { uart: 0 },
+                "UART0.FIFO was read while the receive FIFO was empty (pc 0x40381000), so 0 was returned",
+            ),
         ];
         for (what, needle) in texts {
             assert_cause_then_remedy(&unsupported(what), needle);
```

```diff
--- a/crates/soc-esp32c3/src/periph/uart.rs
+++ b/crates/soc-esp32c3/src/periph/uart.rs
@@ -366,12 +366,12 @@
         for byte in *b"ESP" {
             assert!(uart.write(ctx, FIFO, u32::from(byte)));
         }
-        assert_eq!(uart.take_tx(ctx.now()), [], "nothing has left yet");
+        assert_eq!(uart.take_tx(ctx), [], "nothing has left yet");
         after_bytes(ctx, 1);
-        assert_eq!(uart.take_tx(ctx.now()), b"E");
-        assert_eq!(uart.take_tx(ctx.now()), [], "taken once");
+        assert_eq!(uart.take_tx(ctx), b"E");
+        assert_eq!(uart.take_tx(ctx), [], "taken once");
         after_bytes(ctx, 2);
-        assert_eq!(uart.take_tx(ctx.now()), b"SP");
+        assert_eq!(uart.take_tx(ctx), b"SP");
     }
 
     #[test]
@@ -380,7 +380,7 @@
         let mut uart = console(ctx);
         uart.write(ctx, FIFO, 0xFFFF_FF41);
         after_bytes(ctx, 1);
-        assert_eq!(uart.take_tx(ctx.now()), b"A");
+        assert_eq!(uart.take_tx(ctx), b"A");
     }
 
     #[test]
@@ -425,7 +425,7 @@
         after_bytes(ctx, 10);
         uart.write(ctx, FIFO, u32::from(b'b'));
         after_bytes(ctx, 1);
-        assert_eq!(uart.take_tx(ctx.now()), b"ab");
+        assert_eq!(uart.take_tx(ctx), b"ab");
     }
 
     #[test]
@@ -436,16 +436,16 @@
         uart.write(ctx, CLK_CONF, CLK_CONF_XTAL | (1 << SCLK_DIV_NUM_SHIFT));
         uart.write(ctx, FIFO, u32::from(b'x'));
         after_bytes(ctx, 1);
-        assert_eq!(uart.take_tx(ctx.now()), []);
+        assert_eq!(uart.take_tx(ctx), []);
         after_bytes(ctx, 1);
-        assert_eq!(uart.take_tx(ctx.now()), b"x");
+        assert_eq!(uart.take_tx(ctx), b"x");
         // APB_CLK as the source, 20 MHz after power-on: twice as long as with the crystal.
         uart.write(ctx, CLK_CONF, CLK_CONF_XTAL & !(0b10 << SCLK_SEL_SHIFT));
         uart.write(ctx, FIFO, u32::from(b'y'));
         after_bytes(ctx, 1);
-        assert_eq!(uart.take_tx(ctx.now()), []);
+        assert_eq!(uart.take_tx(ctx), []);
         after_bytes(ctx, 1);
-        assert_eq!(uart.take_tx(ctx.now()), b"y");
+        assert_eq!(uart.take_tx(ctx), b"y");
     }
 
     #[test]
@@ -467,7 +467,7 @@
             Some(Unsupported::UartOverflow { uart: 0 })
         );
         after_bytes(ctx, 130);
-        let sent = uart.take_tx(ctx.now());
+        let sent = uart.take_tx(ctx);
         assert_eq!(sent.len(), 129);
         assert!(!sent.contains(&0xEE));
     }
@@ -480,9 +480,9 @@
             uart.write(ctx, FIFO, u32::from(byte));
         }
         after_bytes(ctx, 4);
-        uart.reset(ctx.now());
+        uart.reset(ctx);
         after_bytes(ctx, 20);
-        assert_eq!(uart.take_tx(ctx.now()), b"gone");
+        assert_eq!(uart.take_tx(ctx), b"gone");
         assert_eq!(
             uart.read(ctx, CLKDIV),
             Some(0x2B6),
@@ -503,7 +503,7 @@
         uart.write(ctx, CONF0, 0x0400_001C);
         assert_eq!(fifo_count(ctx, &mut uart), 0);
         after_bytes(ctx, 3);
-        assert_eq!(uart.take_tx(ctx.now()), b"a");
+        assert_eq!(uart.take_tx(ctx), b"a");
     }
 
     #[test]
@@ -521,21 +521,230 @@
     }
 
     #[test]
-    fn interrupt_clear_is_accepted_and_the_rest_is_unmodeled() {
+    fn interrupt_clear_is_accepted_and_the_interrupt_state_is_unmodeled() {
         let ctx = &mut Ctx::default();
         let mut uart = Uart::new(0, UART0);
         assert!(uart.write(ctx, INT_CLR, 0xFFFF_FFFF));
         assert_eq!(
             uart.read(ctx, reg("INT_RAW")),
             None,
-            "interrupts are SP1b-3d"
+            "interrupts are not modelled yet"
         );
         assert_eq!(uart.read(ctx, reg("INT_ST")), None);
-        assert_eq!(
-            uart.read(ctx, FIFO),
-            None,
-            "FIFO is write-only: RX is not modelled"
-        );
+    }
+
+    /// The instant, in cycles, at which the `n`th byte (from 1) the host sends at 0 has landed,
+    /// at 115 200 baud 8N1 on the crystal.
+    fn landed(n: u64) -> Cycles {
+        gone_after(20 * n)
+    }
+
+    fn rx_count(ctx: &mut Ctx, uart: &mut Uart) -> u32 {
+        uart.read(ctx, STATUS).unwrap() & 0x3FF
+    }
+
+    #[test]
+    fn a_byte_of_the_host_lands_one_frame_after_it_starts_and_is_read_once() {
+        // UR §5.4: one frame after the start bit (A7); `STATUS.RXFIFO_CNT`, then a read of `FIFO`.
+        let ctx = &mut Ctx::default();
+        let mut uart = console(ctx);
+        uart.receive(ctx, Cycles::ZERO, b"a");
+        ctx.sleep_until(Cycles(landed(1).get() - 1));
+        assert_eq!(rx_count(ctx, &mut uart), 0, "the stop bit is not over");
+        ctx.sleep_until(landed(1));
+        assert_eq!(rx_count(ctx, &mut uart), 1);
+        assert_eq!(
+            uart.read(ctx, STATUS),
+            Some(0xE000_C001),
+            "the line bits stay"
+        );
+        assert_eq!(uart.read(ctx, FIFO), Some(u32::from(b'a')));
+        assert_eq!(rx_count(ctx, &mut uart), 0);
+        assert_eq!(ctx.take_unsupported(), None);
+    }
+
+    #[test]
+    fn the_bytes_of_an_input_follow_each_other_and_those_sent_before() {
+        let ctx = &mut Ctx::default();
+        let mut uart = console(ctx);
+        uart.receive(ctx, Cycles::ZERO, b"ab");
+        uart.receive(ctx, Cycles::ZERO, b"c");
+        ctx.sleep_until(landed(2));
+        assert_eq!(rx_count(ctx, &mut uart), 2);
+        ctx.sleep_until(landed(3));
+        assert_eq!(rx_count(ctx, &mut uart), 3, "c waits for the line");
+        // An input dated later starts at its date: the line was idle.
+        let later = Cycles(landed(3).get() + 1_000_000);
+        uart.receive(ctx, later, b"d");
+        ctx.sleep_until(Cycles(later.get() + landed(1).get() - 1));
+        assert_eq!(rx_count(ctx, &mut uart), 3);
+        ctx.sleep_until(Cycles(later.get() + landed(1).get()));
+        assert_eq!(rx_count(ctx, &mut uart), 4);
+        let read: Vec<u32> = (0..4).map(|_| uart.read(ctx, FIFO).unwrap()).collect();
+        assert_eq!(read, [97, 98, 99, 100]);
+    }
+
+    #[test]
+    fn the_uart_asks_to_be_called_at_the_instant_each_byte_lands() {
+        // A CPU that waits for an interrupt must see the byte arrive at its instant.
+        let ctx = &mut Ctx::default();
+        let mut uart = console(ctx);
+        uart.receive(ctx, Cycles::ZERO, b"hi");
+        assert_eq!(ctx.next_wake(), Some(landed(1)));
+        ctx.sleep_until(landed(1));
+        assert_eq!(ctx.take_due(), Some(Event::Uart(0)));
+        uart.wake(ctx);
+        assert_eq!(ctx.next_wake(), Some(landed(2)));
+        ctx.sleep_until(landed(2));
+        assert_eq!(ctx.take_due(), Some(Event::Uart(0)));
+        uart.wake(ctx);
+        assert_eq!(ctx.next_wake(), None, "nothing more to land");
+        assert_eq!(uart.host_pending(), 0);
+        assert_eq!(rx_count(ctx, &mut uart), 2);
+    }
+
+    #[test]
+    fn a_byte_that_finds_the_fifo_full_is_dropped_and_reported() {
+        // UR A2: the new byte is lost, the 128 stay.
+        let ctx = &mut Ctx::default();
+        let mut uart = console(ctx);
+        let sent: Vec<u8> = (0..=128).collect();
+        uart.receive(ctx, Cycles::ZERO, &sent);
+        ctx.sleep_until(landed(128));
+        assert_eq!(rx_count(ctx, &mut uart), 128);
+        assert_eq!(ctx.take_unsupported(), None);
+        ctx.sleep_until(landed(129));
+        assert_eq!(rx_count(ctx, &mut uart), 128);
+        assert_eq!(
+            ctx.take_unsupported(),
+            Some(Unsupported::UartRxOverflow { uart: 0 })
+        );
+        let read: Vec<u32> = (0..128).map(|_| uart.read(ctx, FIFO).unwrap()).collect();
+        assert_eq!(read, (0..128).collect::<Vec<u32>>());
+    }
+
+    #[test]
+    fn a_byte_that_lands_while_the_receiver_is_off_or_its_fifo_in_reset_is_lost() {
+        // UR A8: `CLK_CONF.RX_SCLK_EN` clear, or `CONF0.RXFIFO_RST` held.
+        for (register, value) in [
+            (CLK_CONF, CLK_CONF_XTAL & !RX_SCLK_EN),
+            (CONF0, 0x0400_001C | RXFIFO_RST),
+        ] {
+            let ctx = &mut Ctx::default();
+            let mut uart = console(ctx);
+            uart.write(ctx, register, value);
+            uart.receive(ctx, Cycles::ZERO, b"x");
+            ctx.sleep_until(landed(1));
+            assert_eq!(rx_count(ctx, &mut uart), 0, "{register:#x}");
+            assert_eq!(uart.host_pending(), 0, "the byte arrived, and was lost");
+        }
+    }
+
+    #[test]
+    fn the_receive_fifo_reset_empties_the_fifo() {
+        // `uart_param_config` pulses it (UR §3.3).
+        let ctx = &mut Ctx::default();
+        let mut uart = console(ctx);
+        uart.receive(ctx, Cycles::ZERO, b"xyz");
+        ctx.sleep_until(landed(2));
+        uart.write(ctx, CONF0, 0x0400_001C | RXFIFO_RST);
+        uart.write(ctx, CONF0, 0x0400_001C);
+        assert_eq!(rx_count(ctx, &mut uart), 0);
+        ctx.sleep_until(landed(3));
+        assert_eq!(uart.read(ctx, FIFO), Some(u32::from(b'z')));
+    }
+
+    #[test]
+    fn a_read_of_an_empty_receive_fifo_returns_0_and_is_refused_by_name() {
+        // UR A6.
+        let ctx = &mut Ctx::default();
+        let mut uart = Uart::new(1, UART1);
+        assert_eq!(uart.read(ctx, FIFO), Some(0));
+        assert_eq!(
+            ctx.take_unsupported(),
+            Some(Unsupported::UartEmptyRead { uart: 1 })
+        );
+    }
+
+    #[test]
+    fn a_received_byte_keeps_the_frame_set_when_it_started() {
+        let ctx = &mut Ctx::default();
+        let mut uart = console(ctx);
+        uart.write(ctx, CONF0, 0x0400_003E); // 8E2: 12 bit times
+        uart.receive(ctx, Cycles::ZERO, b"pq");
+        ctx.sleep_until(Cycles(1));
+        uart.write(ctx, CONF0, 0x0400_001C); // 8N1 from now on
+        ctx.sleep_until(Cycles(gone_after(24).get() - 1));
+        assert_eq!(rx_count(ctx, &mut uart), 0);
+        ctx.sleep_until(gone_after(24));
+        assert_eq!(rx_count(ctx, &mut uart), 1, "p, 8E2");
+        ctx.sleep_until(gone_after(24 + 20));
+        assert_eq!(
+            rx_count(ctx, &mut uart),
+            2,
+            "q started after the change: 8N1"
+        );
+    }
+
+    #[test]
+    fn a_reset_empties_the_receive_fifo_and_the_host_goes_on_sending() {
+        let ctx = &mut Ctx::default();
+        let mut uart = console(ctx);
+        uart.receive(ctx, Cycles::ZERO, b"012");
+        ctx.sleep_until(Cycles(landed(2).get() - 1));
+        assert_eq!(rx_count(ctx, &mut uart), 1);
+        uart.reset(ctx);
+        assert_eq!(rx_count(ctx, &mut uart), 0);
+        ctx.sleep_until(landed(2));
+        assert_eq!(rx_count(ctx, &mut uart), 1, "the byte on the line lands");
+        // The next byte starts after the reset, at the reset values: `CLKDIV` 694 on the crystal
+        // divided by 2 (`SCLK_DIV_NUM` 1).
+        let third = (2 * 20 * (347 * 16 + 3) * 4 + 20 * (694 * 16) * 2 * 4_u64).div_ceil(SUBCYCLES);
+        ctx.sleep_until(Cycles(third - 1));
+        assert_eq!(rx_count(ctx, &mut uart), 1);
+        ctx.sleep_until(Cycles(third));
+        assert_eq!(rx_count(ctx, &mut uart), 2);
+        assert_eq!(uart.read(ctx, FIFO), Some(u32::from(b'1')));
+    }
+
+    #[test]
+    fn the_registers_of_the_driver_are_storage_and_rs485_or_another_fifo_size_are_refused() {
+        // UR §3.3: the driver writes IDLE_CONF and RS485_CONF, reads AT_CMD_CHAR in its handler.
+        let ctx = &mut Ctx::default();
+        let mut uart = Uart::new(0, UART0);
+        for offset in [IDLE_CONF, RS485_CONF, AT_CMD_CHAR, MEM_CONF] {
+            let reset = svd_regs::esp32c3::lookup(UART0 + offset)
+                .unwrap()
+                .reset_value;
+            assert_eq!(uart.read(ctx, offset), Some(reset), "{offset:#x}");
+        }
+        assert!(uart.write(ctx, IDLE_CONF, 0));
+        assert!(
+            uart.write(ctx, MEM_CONF, 0x0064_0012),
+            "a timeout of 100, one block each"
+        );
+        assert!(uart.write(ctx, RS485_CONF, 0));
+        assert_eq!(ctx.take_unsupported(), None);
+        assert_eq!(uart.read(ctx, MEM_CONF), Some(0x0064_0012));
+        uart.write(ctx, RS485_CONF, 1);
+        assert_eq!(
+            ctx.take_unsupported(),
+            Some(Unsupported::UartFeature {
+                uart: 0,
+                feature: UartFeature::Rs485
+            })
+        );
+        uart.write(ctx, MEM_CONF, 0x000A_0014);
+        assert_eq!(
+            ctx.take_unsupported(),
+            Some(Unsupported::UartFeature {
+                uart: 0,
+                feature: UartFeature::FifoSize
+            })
+        );
+        assert_eq!(uart.read(ctx, MEM_RX_STATUS), Some(0x0008_0100));
+        assert!(!uart.write(ctx, MEM_RX_STATUS, 0), "read-only");
+        assert_eq!(uart.read(ctx, FSM_STATUS), Some(0), "ST_URX_OUT stays 0");
     }
 
     #[test]
@@ -578,7 +787,7 @@
         assert_eq!(ctx.take_due(), Some(Event::Uart(0)));
         uart.wake(ctx);
         assert_eq!(ctx.next_wake(), None, "idle: nothing more to wait for");
-        assert_eq!(uart.take_tx(ctx.now()), b"ok");
+        assert_eq!(uart.take_tx(ctx), b"ok");
     }
 
     #[test]
@@ -589,7 +798,7 @@
             uart.write(ctx, FIFO, u32::from(byte));
         }
         after_bytes(ctx, 1);
-        assert_eq!(uart.take_tx(ctx.now()), b"b");
+        assert_eq!(uart.take_tx(ctx), b"b");
         assert_eq!(uart.take_written(), b"ye", "in order, each once");
         assert_eq!(uart.take_written(), b"");
         assert_eq!(
@@ -618,12 +827,12 @@
         let every_byte = collect(&|ctx, uart, out| {
             for _ in text {
                 after_bytes(ctx, 1);
-                out.extend(uart.take_tx(ctx.now()));
+                out.extend(uart.take_tx(ctx));
             }
         });
         let once_midway = collect(&|ctx, uart, out| {
             after_bytes(ctx, 2);
-            out.extend(uart.take_tx(ctx.now()));
+            out.extend(uart.take_tx(ctx));
         });
         assert_eq!(every_byte, text);
         assert_eq!(once_midway, text);
@@ -639,10 +848,10 @@
         uart.write(ctx, FIFO, u32::from(b'f'));
         assert_eq!(ctx.next_wake(), Some(Cycles(8)), "7.5 cycles, rounded up");
         ctx.sleep_until(Cycles(7));
-        assert_eq!(uart.take_tx(ctx.now()), []);
+        assert_eq!(uart.take_tx(ctx), []);
         assert_eq!(uart.read(ctx, FSM_STATUS), Some(ST_UTX_OUT_BUSY));
         ctx.sleep_until(Cycles(8));
-        assert_eq!(uart.take_tx(ctx.now()), b"f");
+        assert_eq!(uart.take_tx(ctx), b"f");
         assert_eq!(uart.read(ctx, FSM_STATUS), Some(0));
     }
 
@@ -652,7 +861,7 @@
         let mut uart = console(ctx);
         uart.write(ctx, CLKDIV, 0);
         uart.write(ctx, FIFO, u32::from(b'z'));
-        assert_eq!(uart.take_tx(ctx.now()), b"z");
+        assert_eq!(uart.take_tx(ctx), b"z");
         assert_eq!(uart.read(ctx, FSM_STATUS), Some(0));
     }
 
@@ -682,9 +891,9 @@
                     sclk_sel: selection
                 })
             );
-            assert_eq!(uart.take_tx(ctx.now()), [], "SCLK_SEL {selection}");
+            assert_eq!(uart.take_tx(ctx), [], "SCLK_SEL {selection}");
             after_bytes(ctx, 1);
-            assert_eq!(uart.take_tx(ctx.now()), b"r", "SCLK_SEL {selection}");
+            assert_eq!(uart.take_tx(ctx), b"r", "SCLK_SEL {selection}");
             uart.write(ctx, FIFO, u32::from(b's'));
             assert!(ctx.take_unsupported().is_some(), "every byte is reported");
         }
@@ -731,11 +940,11 @@
         uart.write(ctx, CLK_CONF, CLK_CONF_XTAL | (1 << SCLK_DIV_NUM_SHIFT));
         uart.write(ctx, FIFO, u32::from(b'b'));
         after_bytes(ctx, 1);
-        assert_eq!(uart.take_tx(ctx.now()), b"a", "a was timed at 115 200 baud");
+        assert_eq!(uart.take_tx(ctx), b"a", "a was timed at 115 200 baud");
         after_bytes(ctx, 1);
-        assert_eq!(uart.take_tx(ctx.now()), [], "b takes two byte times");
+        assert_eq!(uart.take_tx(ctx), [], "b takes two byte times");
         after_bytes(ctx, 2);
-        assert_eq!(uart.take_tx(ctx.now()), b"b");
+        assert_eq!(uart.take_tx(ctx), b"b");
     }
 
     /// The instant, in cycles, at which the first byte written now to an idle transmitter has
@@ -779,7 +988,7 @@
             uart.write(ctx, FIFO, byte);
         }
         ctx.sleep_until(gone_after(128 * 21));
-        assert_eq!(uart.take_tx(ctx.now()).len(), 128);
+        assert_eq!(uart.take_tx(ctx).len(), 128);
         assert_eq!(
             ctx.next_wake(),
             Some(gone_after(21)),
@@ -795,11 +1004,11 @@
         uart.write(ctx, CONF0, 0x0400_003E);
         uart.write(ctx, FIFO, u32::from(b'b'));
         ctx.sleep_until(gone_after(20));
-        assert_eq!(uart.take_tx(ctx.now()), b"a", "8N1, as written");
+        assert_eq!(uart.take_tx(ctx), b"a", "8N1, as written");
         ctx.sleep_until(gone_after(20 + 23));
-        assert_eq!(uart.take_tx(ctx.now()), []);
+        assert_eq!(uart.take_tx(ctx), []);
         ctx.sleep_until(gone_after(20 + 24));
-        assert_eq!(uart.take_tx(ctx.now()), b"b", "8E2: 12 bit times");
+        assert_eq!(uart.take_tx(ctx), b"b", "8E2: 12 bit times");
     }
 
     #[test]
@@ -861,7 +1070,7 @@
         assert_eq!(ctx.take_due(), Some(Event::Uart(0)));
         uart.wake(ctx);
         assert_eq!(ctx.next_wake(), None, "nothing is left to wait for");
-        assert_eq!(uart.take_tx(ctx.now()), b"ab", "c was dropped");
+        assert_eq!(uart.take_tx(ctx), b"ab", "c was dropped");
         assert_eq!(uart.read(ctx, FSM_STATUS), Some(0));
         assert_eq!(ctx.take_unsupported(), None);
     }
```

- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p soc-esp32c3 --lib -- periph::uart:: diag:: bus:: > /tmp/t2-red.log 2>&1; echo "exit=$?"; grep -E "^error" /tmp/t2-red.log | sort | uniq -c
```

Expected: `exit=101`; `could not compile soc-esp32c3 (lib test) due to 68 previous errors`: the new registers and bits (`error[E0425]: cannot find value IDLE_CONF`, `MEM_CONF`, `MEM_RX_STATUS`, `RS485_CONF`, `AT_CMD_CHAR`, `RXFIFO_RST`, `RX_SCLK_EN`), `no method named receive` and `host_pending` for `uart::Uart`, `UartFeature::FifoSize`, and 33 × `error[E0308]: mismatched types` where the tests pass the context to `reset` and `take_tx`.

- [ ] **Step 3: The receive path**

```diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -75,6 +75,12 @@
     /// A UART field set to a mode the model does not have (report UR §10): the register keeps
     /// the value, and the UART goes on as if the field were clear.
     UartFeature { uart: u8, feature: UartFeature },
+    /// A byte of the host that found the 128-byte receive FIFO of a UART full: dropped.
+    /// ASSUMPTION: the new byte is lost and the 128 stay (report UR A2); 200 bytes sent to a FIFO
+    /// nobody reads, on a chip, settle it.
+    UartRxOverflow { uart: u8 },
+    /// A read of the receive FIFO of a UART while it was empty: 0 was returned (report UR A6).
+    UartEmptyRead { uart: u8 },
 }
 
 /// A UART mode the model refuses by name when software sets it (report UR §10, A4).
@@ -88,6 +94,10 @@
     Autobaud,
     /// `CONF0.STOP_BIT_NUM` = 0: a frame with no documented stop bit (report UR A4).
     NoStopBit,
+    /// `MEM_CONF.RX_SIZE` or `TX_SIZE` other than one 128-byte block.
+    FifoSize,
+    /// `RS485_CONF` other than 0.
+    Rs485,
 }
 
 impl UartFeature {
@@ -114,6 +124,16 @@
                 "CONF0.STOP_BIT_NUM = 0",
                 "for a frame without a stop bit",
                 "its bytes are timed with one stop bit; write STOP_BIT_NUM 1, 2 or 3 (1, 1.5 or 2 stop bits)",
+            ),
+            UartFeature::FifoSize => (
+                "MEM_CONF.RX_SIZE or TX_SIZE other than 1",
+                "to give its FIFOs another share of the UART memory",
+                "each FIFO keeps 128 bytes; leave RX_SIZE and TX_SIZE at 1",
+            ),
+            UartFeature::Rs485 => (
+                "RS485_CONF other than 0",
+                "to RS485 mode",
+                "it sends and receives as a plain UART; write RS485_CONF = 0",
             ),
         }
     }
@@ -247,7 +267,9 @@
             ),
             Unsupported::UartOverflow { .. }
             | Unsupported::UartClock { .. }
-            | Unsupported::UartFeature { .. } => self.describe_serial(pc, f),
+            | Unsupported::UartFeature { .. }
+            | Unsupported::UartRxOverflow { .. }
+            | Unsupported::UartEmptyRead { .. } => self.describe_serial(pc, f),
         }
     }
 
@@ -269,6 +291,14 @@
                     "UART{uart} was set {asked} ({field}, pc {pc:#010x}), which inkemu does not model, so {instead}"
                 )
             }
+            Unsupported::UartRxOverflow { uart } => write!(
+                f,
+                "a byte the host sent found the 128-byte receive FIFO of UART{uart} full (pc {pc:#010x}), so it was dropped and the 128 bytes stay; read the FIFO sooner, or send less at once"
+            ),
+            Unsupported::UartEmptyRead { uart } => write!(
+                f,
+                "UART{uart}.FIFO was read while the receive FIFO was empty (pc {pc:#010x}), so 0 was returned; read only the number of bytes UART.STATUS RXFIFO_CNT gives"
+            ),
             // INVARIANT: `describe` hands only the refusals above to this function.
             _ => Ok(()),
         }
@@ -372,6 +402,8 @@
                 Unsupported::UartOverflow { uart } => (12, u32::from(uart), 0),
                 Unsupported::UartClock { uart, sclk_sel } => (13, u32::from(uart), sclk_sel),
                 Unsupported::UartFeature { uart, feature } => (23, u32::from(uart), feature as u32),
+                Unsupported::UartRxOverflow { uart } => (24, u32::from(uart), 0),
+                Unsupported::UartEmptyRead { uart } => (25, u32::from(uart), 0),
                 // A command is 1 to 16 bits long, so 0 is free for "no command phase".
                 Unsupported::FlashTransfer { command_bits, .. } => {
                     (11, command_bits.unwrap_or(0), 0)
```

```diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -469,9 +469,9 @@
     /// of a run whose firmware writes without end.
     pub fn take_console(&mut self, channel: ConsoleChannel) -> Vec<u8> {
         match channel {
-            ConsoleChannel::Uart0 => self.bus.periph.uart0.take_tx(self.bus.ctx.now()),
+            ConsoleChannel::Uart0 => self.bus.periph.uart0.take_tx(&mut self.bus.ctx),
             ConsoleChannel::UsbSerialJtag => self.bus.periph.usb_serial_jtag.take_tx(),
-            ConsoleChannel::Uart1 => self.bus.periph.uart1.take_tx(self.bus.ctx.now()),
+            ConsoleChannel::Uart1 => self.bus.periph.uart1.take_tx(&mut self.bus.ctx),
         }
     }
 
```

```diff
--- a/crates/soc-esp32c3/src/periph/mod.rs
+++ b/crates/soc-esp32c3/src/periph/mod.rs
@@ -521,8 +521,8 @@
                 self.spi0 = Spi0::new();
                 self.spi1.reset();
             }
-            Module::Uart0 => self.uart0.reset(ctx.now()),
-            Module::Uart1 => self.uart1.reset(ctx.now()),
+            Module::Uart0 => self.uart0.reset(ctx),
+            Module::Uart1 => self.uart1.reset(ctx),
             Module::Timg0 => self.timg0.reset(ctx),
             Module::Timg1 => self.timg1.reset(ctx),
             Module::Systimer => self.systimer.reset(ctx),
@@ -584,8 +584,8 @@
         self.ana_i2c.reset();
         self.apb_saradc = AcceptedBlock::new(APB_SARADC_BASE, &APB_SARADC_ACCEPTED);
         self.radio_power = AcceptedBlock::new(0, &RADIO_POWER_ACCEPTED);
-        self.uart0.reset(ctx.now());
-        self.uart1.reset(ctx.now());
+        self.uart0.reset(ctx);
+        self.uart1.reset(ctx);
         self.usb_serial_jtag.reset();
         // No rebuilt device asserts an interrupt source, and `SYSTEM` selects the reset clock again.
         // The timer groups and the SYSTIMER go last: each withdraws its wake-up, so that none of
```

```diff
--- a/crates/soc-esp32c3/src/periph/uart.rs
+++ b/crates/soc-esp32c3/src/periph/uart.rs
@@ -1,9 +1,15 @@
-//! UART controller, transmit side. Register layout: `uart_reg.h` (reports BC §G.2, BA §7).
+//! UART controller. Register layout: `uart_reg.h` (reports BC §G.2, BA §7, UR §2).
 //!
 //! A byte written to the FIFO leaves at the baud rate: the transmit FIFO fills, `STATUS` and
 //! `FSM_STATUS` say so, and the byte reaches the console when its last bit has gone. Software waits
 //! on both: the ROM before each character, ESP-IDF before it changes a clock or restarts (report IR
-//! §4.1). Reception and the UART interrupts arrive in SP1b-3d.
+//! §4.1).
+//!
+//! A byte the host sends lands in the receive FIFO one frame after its start bit (report UR
+//! §5.4): the bytes of one input go out back to back from the device time they are given, at the
+//! rate and in the frame the UART is set to. `STATUS` and a read of `FIFO` work the count out from
+//! device time when they are accessed (ADR 0003 §4), and the UART asks to be called at the
+//! instant each byte lands. The UART interrupts arrive with the next part of this plan.
 use crate::clock::XTAL_PERIOD;
 use crate::ctx::{Ctx, Deadline, Event};
 use crate::diag::{UartFeature, Unsupported};
@@ -40,11 +46,40 @@
 const CONF0: u32 = reg("CONF0");
 const CONF1: u32 = reg("CONF1");
 const CLK_CONF: u32 = reg("CLK_CONF");
-/// What software writes and reads back. `CLKDIV` and `CLK_CONF` set the baud rate.
-const CONFIG: [u32; 5] = [INT_ENA, CLKDIV, CONF0, CONF1, CLK_CONF];
+const IDLE_CONF: u32 = reg("IDLE_CONF");
+const RS485_CONF: u32 = reg("RS485_CONF");
+const AT_CMD_CHAR: u32 = reg("AT_CMD_CHAR");
+const MEM_CONF: u32 = reg("MEM_CONF");
+const MEM_RX_STATUS: u32 = reg("MEM_RX_STATUS");
+/// What software writes and reads back. `CLKDIV` and `CLK_CONF` set the baud rate. The ESP-IDF
+/// driver writes `IDLE_CONF` and `RS485_CONF` when it configures the port and reads
+/// `AT_CMD_CHAR` twice in each pass of its interrupt handler (report UR §3.3): stored, with no
+/// effect of their own (`RS485_CONF` other than 0 is refused).
+const CONFIG: [u32; 9] = [
+    INT_ENA,
+    CLKDIV,
+    CONF0,
+    CONF1,
+    CLK_CONF,
+    IDLE_CONF,
+    RS485_CONF,
+    AT_CMD_CHAR,
+    MEM_CONF,
+];
 /// `STATUS` bits [9:0] `RXFIFO_CNT` and [25:16] `TXFIFO_CNT`.
 const FIFO_COUNTS: u32 = 0x03FF_03FF;
 const TXFIFO_CNT_SHIFT: u32 = 16;
+/// `CONF0` bit 17 `RXFIFO_RST`: empties the receive FIFO; a byte that lands while it is set is
+/// lost (`ROM3:Uart_Init` and `uart_param_config` pulse it; report UR §3.1, §3.3, A8).
+const RXFIFO_RST: u32 = 1 << 17;
+/// `CLK_CONF` bit 25 `RX_SCLK_EN`: the receiver's clock; a byte that lands while it is clear is
+/// lost (report UR §5.4 point 5, A8).
+const RX_SCLK_EN: u32 = 1 << 25;
+/// `MEM_CONF` bits [3:1] `RX_SIZE` and [6:4] `TX_SIZE`: the 128-byte blocks of the UART memory
+/// each FIFO has. Both are 1 at reset and in every measured configuration (report UR §2.2); the
+/// model has one block each and refuses another size.
+const MEM_SIZES: u32 = 0b111_1110;
+const ONE_BLOCK_EACH: u32 = 0b001_0010;
 /// `FSM_STATUS` bits [7:4] `ST_UTX_OUT`: 0 when the transmitter is idle, the only value software
 /// tests (`ROM3:uart_tx_wait_idle@0x4004c4c6`; BA §7).
 /// ASSUMPTION: while a byte is being sent the field reads 1. The other states are not documented
@@ -116,43 +151,85 @@
     /// `STATUS` with both FIFO counts at zero: the line-state bits keep their reset value.
     idle_status: u32,
     config: Storage,
-    /// The bytes on their way out, each with the time, in sixteenths of a time-base cycle, at
+    /// `MEM_RX_STATUS`, served at its reset value: nothing reads it (report UR §2.2).
+    mem_rx_status: u32,
+    /// The bytes on their way out, each with the time, in thirty-seconds of a time-base cycle, at
     /// which its last bit has gone. The first is in the shift register, the others in the FIFO.
     in_flight: VecDeque<(u64, u8)>,
     /// The bytes that have left, for the console.
     tx: Vec<u8>,
-    /// The call at the time the transmitter goes idle: a CPU that waits for an interrupt with
-    /// bytes still on their way out sleeps until they have left, not for ever.
+    /// The bytes the host sends and that have not started: each with the device time, in
+    /// thirty-seconds, before which it does not start, the date of the input it came in.
+    host: VecDeque<(u64, u8)>,
+    /// The byte on the line: the instant its frame ends, when it lands, and the byte.
+    receiving: Option<(u64, u8)>,
+    /// When the line is free for the next byte: the end of the last frame that started.
+    line_free_at: u64,
+    /// The receive FIFO.
+    rx: VecDeque<u8>,
+    /// The call at the next instant the device has something to do: a byte of its transmitter
+    /// leaves (so that a CPU that waits for an interrupt with bytes on their way out sleeps
+    /// until they have left, not for ever), or a byte of the host lands.
     wake: Deadline,
 }
 
 impl Uart {
     #[must_use]
     pub fn new(index: u8, base: u32) -> Uart {
-        let status_reset = svd_regs::esp32c3::lookup(base + STATUS).map_or(0, |r| r.reset_value);
+        let reset =
+            |offset: u32| svd_regs::esp32c3::lookup(base + offset).map_or(0, |r| r.reset_value);
         Uart {
             index,
             base,
-            idle_status: status_reset & !FIFO_COUNTS,
+            idle_status: reset(STATUS) & !FIFO_COUNTS,
             config: Storage::for_offsets(base, &CONFIG),
+            mem_rx_status: reset(MEM_RX_STATUS),
             in_flight: VecDeque::new(),
             tx: Vec::new(),
+            host: VecDeque::new(),
+            receiving: None,
+            line_free_at: 0,
+            rx: VecDeque::new(),
             wake: Deadline::default(),
         }
     }
 
     /// The module is reset (`SYSTEM.PERIP_RST_EN0`, or a core reset): its registers return to their
-    /// reset values and the bytes that have not left are lost. Those that left stay for the console.
-    pub fn reset(&mut self, now: Cycles) {
-        self.settle(now);
+    /// reset values, the bytes that have not left are lost, and so are those in the receive FIFO.
+    /// Those that left stay for the console. The host is no part of the chip: the bytes it is
+    /// sending go on arriving, and land in the FIFO of the reset UART.
+    pub fn reset(&mut self, ctx: &mut Ctx) {
+        self.settle(ctx);
         self.config = Storage::for_offsets(self.base, &CONFIG);
         self.in_flight.clear();
-    }
-
-    /// The bytes that have left the transmitter by `now`, since the last call.
-    pub fn take_tx(&mut self, now: Cycles) -> Vec<u8> {
-        self.settle(now);
+        self.rx.clear();
+        self.arm(ctx);
+    }
+
+    /// The bytes that have left the transmitter by now, since the last call.
+    pub fn take_tx(&mut self, ctx: &mut Ctx) -> Vec<u8> {
+        self.settle(ctx);
         std::mem::take(&mut self.tx)
+    }
+
+    /// The host sends `bytes`, the first starting at device time `at` (now or later), the others
+    /// right after it, and after the bytes it sent before. The caller bounds how many it hands
+    /// over ([`Uart::host_pending`]).
+    ///
+    /// ASSUMPTION: the host sends at the baud rate and in the frame the UART is set to, as a
+    /// terminal set to match it does; a host at another rate would give framing errors, which the
+    /// model does not produce. Every console measured is 8N1 at 115 200 baud (report UR §6).
+    pub fn receive(&mut self, ctx: &mut Ctx, at: Cycles, bytes: &[u8]) {
+        self.settle(ctx);
+        let date = at.max(ctx.now()).get().saturating_mul(SUBCYCLES);
+        self.host.extend(bytes.iter().map(|&byte| (date, byte)));
+        self.arm(ctx);
+    }
+
+    /// How many bytes the host has sent that have not landed yet.
+    #[must_use]
+    pub fn host_pending(&self) -> usize {
+        self.host.len() + usize::from(self.receiving.is_some())
     }
 
     /// Every byte the firmware wrote and no call has taken, those that have not left yet
@@ -171,24 +248,46 @@
         self.tx.len()
     }
 
-    /// The device time of a wake-up has come: the bytes that have gone leave, and the device asks
-    /// to be called again when the last one will have.
+    /// The device time of a wake-up has come: the bytes that have gone leave, those of the host
+    /// that have arrived land, and the device asks to be called again at the next such instant.
     pub(crate) fn wake(&mut self, ctx: &mut Ctx) {
-        let now = ctx.now();
-        self.wake.called(now);
-        self.settle(now);
-        if let Some(&(gone_at, _)) = self.in_flight.back() {
+        self.wake.called(ctx.now());
+        self.settle(ctx);
+        self.arm(ctx);
+    }
+
+    /// Asks to be called when the last byte of the transmitter will have left, and when the next
+    /// byte of the host lands.
+    fn arm(&mut self, ctx: &mut Ctx) {
+        let next_landing = self.next_landing(ctx);
+        let last_gone = self.in_flight.back().map(|&(gone_at, _)| gone_at);
+        for instant in [last_gone, next_landing].into_iter().flatten() {
             self.wake.arm(
                 ctx,
-                Cycles(gone_at.div_ceil(SUBCYCLES)),
+                Cycles(instant.div_ceil(SUBCYCLES)),
                 Event::Uart(self.index),
             );
         }
     }
 
-    /// Moves the bytes whose last bit has gone to the console.
-    fn settle(&mut self, now: Cycles) {
-        let now = now.get().saturating_mul(SUBCYCLES);
+    /// The instant the next byte of the host lands: the one on the line, or the next to start,
+    /// whose frame is timed with the settings of now (they hold until a register write, which
+    /// settles first).
+    fn next_landing(&self, ctx: &mut Ctx) -> Option<u64> {
+        if let Some((lands_at, _)) = self.receiving {
+            return Some(lands_at);
+        }
+        let &(date, _) = self.host.front()?;
+        Some(
+            date.max(self.line_free_at)
+                .saturating_add(self.byte_time(self.source_period(ctx))),
+        )
+    }
+
+    /// Brings the device to now: the bytes whose last bit has gone move to the console, the bytes
+    /// of the host whose frame has ended land, in the order of their instants.
+    fn settle(&mut self, ctx: &mut Ctx) {
+        let now = ctx.now().get().saturating_mul(SUBCYCLES);
         while let Some(&(gone_at, byte)) = self.in_flight.front() {
             if gone_at > now {
                 break;
@@ -196,6 +295,64 @@
             self.tx.push(byte);
             self.in_flight.pop_front();
         }
+        // INVARIANT: each pass lands a byte or ends the loop, and the bytes are those the caller
+        // handed over within its bound (`host_pending`).
+        loop {
+            if self.receiving.is_none() {
+                let Some(&(date, byte)) = self.host.front() else {
+                    break;
+                };
+                let starts_at = date.max(self.line_free_at);
+                if starts_at > now {
+                    break;
+                }
+                // The frame of a byte is the one the UART is set to when it starts: a register
+                // write settles before it changes the setting.
+                let lands_at = starts_at.saturating_add(self.byte_time(self.source_period(ctx)));
+                self.host.pop_front();
+                self.receiving = Some((lands_at, byte));
+                self.line_free_at = lands_at;
+            }
+            match self.receiving {
+                Some((lands_at, byte)) if lands_at <= now => {
+                    self.receiving = None;
+                    self.land(ctx, byte);
+                }
+                _ => break,
+            }
+        }
+    }
+
+    /// A byte of the host has been received whole: it enters the FIFO.
+    ///
+    /// ASSUMPTION: a byte that lands while the receiver's clock is off (`CLK_CONF.RX_SCLK_EN`
+    /// clear) or while `CONF0.RXFIFO_RST` is set is lost (report UR A8), and a byte that finds
+    /// 128 bytes in the FIFO is dropped while the 128 stay (report UR A2). Input sent to a chip
+    /// during its boot, and 200 bytes sent to a FIFO nobody reads, settle them.
+    fn land(&mut self, ctx: &mut Ctx, byte: u8) {
+        let clk_conf = self.config.read(CLK_CONF).unwrap_or(0);
+        let conf0 = self.config.read(CONF0).unwrap_or(0);
+        if clk_conf & RX_SCLK_EN == 0 || conf0 & RXFIFO_RST != 0 {
+            return;
+        }
+        if self.rx.len() >= FIFO_BYTES {
+            ctx.unsupported(Unsupported::UartRxOverflow { uart: self.index });
+            return;
+        }
+        self.rx.push_back(byte);
+    }
+
+    /// A read of `FIFO`: the oldest byte received.
+    ///
+    /// ASSUMPTION: a read of an empty receive FIFO returns 0 and changes nothing (report UR A6);
+    /// it is refused by name. Only firmware that reads without looking at `RXFIFO_CNT` does it;
+    /// a chip settles it.
+    fn pop(&mut self, ctx: &mut Ctx) -> u32 {
+        if let Some(byte) = self.rx.pop_front() {
+            return u32::from(byte);
+        }
+        ctx.unsupported(Unsupported::UartEmptyRead { uart: self.index });
+        0
     }
 
     /// The period of the source clock a byte sent now is timed with, in time-base cycles.
@@ -293,24 +450,29 @@
 
 impl Peripheral<Ctx> for Uart {
     fn read(&mut self, ctx: &mut Ctx, offset: u32) -> Option<u32> {
-        self.settle(ctx.now());
+        self.settle(ctx);
         match offset {
+            FIFO => Some(self.pop(ctx)),
             STATUS => {
                 let in_fifo = u32::try_from(self.in_flight.len().saturating_sub(1)).unwrap_or(0);
-                Some(self.idle_status | (in_fifo << TXFIFO_CNT_SHIFT))
+                // INVARIANT: the receive FIFO holds at most `FIFO_BYTES` bytes (`land`).
+                let received = u32::try_from(self.rx.len()).unwrap_or(0);
+                Some(self.idle_status | (in_fifo << TXFIFO_CNT_SHIFT) | received)
             }
+            // `ST_URX_OUT` (bits [3:0]) stays 0: nothing reads it (report UR §2.2).
             FSM_STATUS => Some(if self.in_flight.is_empty() {
                 0
             } else {
                 ST_UTX_OUT_BUSY
             }),
+            MEM_RX_STATUS => Some(self.mem_rx_status),
             _ => self.config.read(offset),
         }
     }
 
     fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
-        self.settle(ctx.now());
-        match offset {
+        self.settle(ctx);
+        let accepted = match offset {
             FIFO => {
                 self.send(ctx, value.to_le_bytes()[0]);
                 true
@@ -322,11 +484,35 @@
                     // The byte in the shift register is on the wire already: it goes on.
                     self.in_flight.truncate(1);
                 }
+                if value & RXFIFO_RST != 0 {
+                    self.rx.clear();
+                }
                 self.refuse_modes(ctx, value);
                 self.config.write(offset, value)
             }
+            MEM_CONF => {
+                if value & MEM_SIZES != ONE_BLOCK_EACH {
+                    ctx.unsupported(Unsupported::UartFeature {
+                        uart: self.index,
+                        feature: UartFeature::FifoSize,
+                    });
+                }
+                self.config.write(offset, value)
+            }
+            RS485_CONF => {
+                if value != 0 {
+                    ctx.unsupported(Unsupported::UartFeature {
+                        uart: self.index,
+                        feature: UartFeature::Rs485,
+                    });
+                }
+                self.config.write(offset, value)
+            }
             _ => self.config.write(offset, value),
-        }
+        };
+        // The baud rate or the frame may have changed: so has the instant the next byte lands.
+        self.arm(ctx);
+        accepted
     }
 }
 
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p soc-esp32c3 --lib -- periph::uart:: diag:: bus:: > /tmp/t2.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t2.log
```

Expected: `exit=0` and `test result: ok. 113 passed; 0 failed; 0 ignored; 0 measured; 421 filtered out`.

- [ ] **Step 5: Mutations**

| # | Mutation (`crates/soc-esp32c3/src/periph/uart.rs`) | Test that fails |
|---|---|---|
| 1 | `if self.rx.len() >= FIFO_BYTES {` → `if self.rx.len() > FIFO_BYTES {` | `a_byte_that_finds_the_fifo_full_is_dropped_and_reported` |
| 2 | delete `if value & RXFIFO_RST != 0 { self.rx.clear(); }` in the `CONF0` arm | `the_receive_fifo_reset_empties_the_fifo` |
| 3 | `if clk_conf & RX_SCLK_EN == 0 \|\| ` → `if ` | `a_byte_that_lands_while_the_receiver_is_off_or_its_fifo_in_reset_is_lost` |
| 4 | `let starts_at = date.max(self.line_free_at);` → `let starts_at = date;` | `the_bytes_of_an_input_follow_each_other_and_those_sent_before` and four others |
| 5 | `for instant in [last_gone, next_landing]` → `for instant in [last_gone]` | `the_uart_asks_to_be_called_at_the_instant_each_byte_lands` |

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy -p soc-esp32c3 --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git commit -m "feat(soc-esp32c3): the host's bytes land in a UART's receive FIFO one frame after they start, and the FIFO is read" -- crates/soc-esp32c3/src
```

Expected: three `exit=0`, then the commit.

---

## Task 3: The UART interrupts drive sources 21 and 22

`INT_RAW`, `INT_ST`, `INT_ENA` and `INT_CLR` take effect on the five bits a driver waits on (C2, C3, C16): `RXFIFO_FULL` (bit 0) while the FIFO holds more than `CONF1.RXFIFO_FULL_THRHD`; `TXFIFO_EMPTY` (bit 1, its reset value) while the transmit FIFO holds fewer than `CONF1.TXFIFO_EMPTY_THRHD`, raised at the instant it falls below; `RXFIFO_OVF` (bit 4) when a byte finds the FIFO full, unless `CONF1.DIS_RX_DAT_OVF`; `RXFIFO_TOUT` (bit 8) when `CONF1.RX_TOUT_EN` is set, a byte is in the FIFO, and the line has been idle `MEM_CONF.RX_TOUT_THRHD` bit times since the last landing, once per idle period (UR A3); `TX_DONE` (bit 14) when the last byte has left. A condition that still holds after `INT_CLR` raises its bit again (`ASSUMPTION:`, UR A1). UART0 drives source 21, UART1 source 22, as levels (`raw & ena != 0`), after every change; a core reset lowers them. Every UART row leaves the accepted-without-effect table, and with them `SP1b-3d` leaves `PLANS_TO_COME`.

**Files:**
- Modify: `crates/soc-esp32c3/src/periph/uart.rs`, `crates/soc-esp32c3/src/periph/mod.rs`, `crates/soc-esp32c3/tests/accepted_without_effect.rs`, `docs/registers-accepted-without-effect.md`

**Interfaces:**
- Consumes: `Ctx::irq(source, level)`, `Source::{UART_INTR_MAP, UART1_INTR_MAP}`, the receive path (Task 2).
- Produces: no new public item; `uart::accepted()` is removed (nothing of the UART is accepted without effect any more).

**Bounds on untrusted input.** `RX_TOUT_THRHD` is a 10-bit field (at most 1,023 bit times); the settle loop of the UART advances by one event (a landing, a departure, the timeout) per pass, and each event consumes a byte or the idle period.

- [ ] **Step 1: Write the failing tests**

```diff
--- a/crates/soc-esp32c3/src/periph/uart.rs
+++ b/crates/soc-esp32c3/src/periph/uart.rs
@@ -707,22 +707,301 @@
     }
 
     #[test]
-    fn interrupt_clear_is_accepted_and_the_interrupt_state_is_unmodeled() {
+    fn the_raw_state_resets_to_txfifo_empty_and_int_st_is_the_enabled_part_of_it() {
+        // SVD: INT_RAW resets to 0x00000002; UR §4: INT_ST = INT_RAW & INT_ENA.
         let ctx = &mut Ctx::default();
         let mut uart = Uart::new(0, UART0);
-        assert!(uart.write(ctx, INT_CLR, 0xFFFF_FFFF));
-        assert_eq!(
-            uart.read(ctx, reg("INT_RAW")),
-            None,
-            "interrupts are not modelled yet"
-        );
-        assert_eq!(uart.read(ctx, reg("INT_ST")), None);
+        assert_eq!(uart.read(ctx, INT_RAW), Some(TXFIFO_EMPTY));
+        assert_eq!(uart.read(ctx, INT_ST), Some(0));
+        assert_eq!(ctx.sources(), 0);
+        uart.write(ctx, INT_ENA, TXFIFO_EMPTY);
+        assert_eq!(uart.read(ctx, INT_ST), Some(TXFIFO_EMPTY));
+        assert_eq!(ctx.sources(), 1 << 21, "source 21, UART_INTR_MAP");
+        uart.write(ctx, INT_CLR, 0xFFFF_FFFF);
+        assert_eq!(
+            uart.read(ctx, INT_RAW),
+            Some(TXFIFO_EMPTY),
+            "a level set again at once: the FIFO is still below its threshold"
+        );
+        uart.write(ctx, INT_ENA, 0);
+        assert_eq!(ctx.sources(), 0, "disabled: the source falls");
+        assert!(
+            !uart.write(ctx, INT_RAW, 0),
+            "set by the device, cleared through INT_CLR"
+        );
+        assert!(!uart.write(ctx, INT_ST, 0), "read-only");
+    }
+
+    /// What `uart_driver_install` leaves (UR §3.3, run S2, as the chip reads back): the timeout at
+    /// 100 bit times and enabled, the full threshold at 120, the transmit threshold at its reset
+    /// value 96, `INT_ENA` = `0x00080195`.
+    fn driver(ctx: &mut Ctx) -> Uart {
+        let mut uart = console(ctx);
+        uart.write(ctx, INT_ENA, 0);
+        uart.write(ctx, INT_CLR, 0x7FFFF);
+        uart.write(ctx, MEM_CONF, 0x0064_0012);
+        uart.write(ctx, CONF1, 0x0020_C078);
+        uart.write(ctx, INT_ENA, 0x0008_0195);
+        assert_eq!(ctx.take_unsupported(), None);
+        uart
+    }
+
+    /// The instant of the timeout after the `n`th byte sent at 0 lands: 100 bit times later.
+    fn timed_out_after(n: u64) -> Cycles {
+        Cycles((n * 20 * (347 * 16 + 3) * 4 + 100 * 2 * (347 * 16 + 3) * 4).div_ceil(SUBCYCLES))
+    }
+
+    #[test]
+    fn a_short_burst_raises_the_timeout_100_bit_times_after_its_last_byte_and_the_handler_reads_it()
+    {
+        // UR §9.2: `bc\r\n` in one burst; UR §3.3: the handler reads INT_ST, STATUS, each byte,
+        // AT_CMD_CHAR twice, writes INT_CLR = 0x101 and reads INT_ST again.
+        let ctx = &mut Ctx::default();
+        let mut uart = driver(ctx);
+        uart.receive(ctx, Cycles::ZERO, b"bc\r\n");
+        run_to(ctx, &mut uart, landed(4));
+        assert_eq!(
+            uart.read(ctx, INT_ST),
+            Some(0),
+            "four bytes: under the threshold of 120"
+        );
+        assert_eq!(
+            ctx.next_wake(),
+            Some(timed_out_after(4)),
+            "the call at the timeout"
+        );
+        ctx.sleep_until(Cycles(timed_out_after(4).get() - 1));
+        assert_eq!(uart.read(ctx, INT_ST), Some(0));
+        ctx.sleep_until(timed_out_after(4));
+        assert_eq!(ctx.take_due(), Some(Event::Uart(0)));
+        uart.wake(ctx);
+        assert_eq!(
+            ctx.sources(),
+            1 << 21,
+            "raised at its instant, before any access"
+        );
+        assert_eq!(uart.read(ctx, INT_ST), Some(RXFIFO_TOUT));
+        assert_eq!(uart.read(ctx, STATUS).unwrap() & 0x3FF, 4);
+        let read: Vec<u32> = (0..4).map(|_| uart.read(ctx, FIFO).unwrap()).collect();
+        assert_eq!(read, [98, 99, 13, 10]);
+        assert_eq!(uart.read(ctx, AT_CMD_CHAR), Some(0x32B));
+        uart.write(ctx, INT_CLR, RXFIFO_TOUT | RXFIFO_FULL);
+        assert_eq!(uart.read(ctx, INT_ST), Some(0));
+        assert_eq!(ctx.sources(), 0);
+        assert_eq!(ctx.take_unsupported(), None);
+    }
+
+    #[test]
+    fn the_timeout_comes_once_per_idle_period_and_never_with_an_empty_fifo() {
+        // UR A3: QEMU raises it as soon as the timeout is configured; the chip needs data.
+        let ctx = &mut Ctx::default();
+        let mut uart = driver(ctx);
+        ctx.sleep_until(timed_out_after(10));
+        assert_eq!(uart.read(ctx, INT_RAW).unwrap() & RXFIFO_TOUT, 0, "no data");
+        assert_eq!(ctx.next_wake(), None);
+        // A byte, read before the timeout: the FIFO is empty when it comes.
+        uart.receive(ctx, Cycles::ZERO, b"a");
+        let start = ctx.now();
+        ctx.sleep_until(start + landed(1).get());
+        uart.read(ctx, FIFO);
+        ctx.sleep_until(start + timed_out_after(1).get());
+        assert_eq!(
+            uart.read(ctx, INT_RAW).unwrap() & RXFIFO_TOUT,
+            0,
+            "empty FIFO"
+        );
+        // Two bytes, one left in the FIFO: the timeout comes, once.
+        let start = ctx.now();
+        uart.receive(ctx, start, b"xy");
+        ctx.sleep_until(start + landed(2).get());
+        uart.read(ctx, FIFO);
+        ctx.sleep_until(start + timed_out_after(2).get());
+        assert_eq!(
+            uart.read(ctx, INT_RAW).unwrap() & RXFIFO_TOUT,
+            RXFIFO_TOUT,
+            "a read does not start the count again"
+        );
+        uart.write(ctx, INT_CLR, RXFIFO_TOUT);
+        ctx.sleep_until(start + timed_out_after(20).get());
+        assert_eq!(
+            uart.read(ctx, INT_RAW).unwrap() & RXFIFO_TOUT,
+            0,
+            "once per idle period"
+        );
+    }
+
+    #[test]
+    fn a_byte_that_starts_before_the_timeout_comes_puts_it_off() {
+        // A start bit ends the idle line: a byte that follows within 100 bit times of the last one
+        // puts the timeout 100 bit times after itself.
+        let ctx = &mut Ctx::default();
+        let mut uart = driver(ctx);
+        uart.receive(ctx, Cycles::ZERO, b"1");
+        let second_at = Cycles(timed_out_after(1).get() - 1_000);
+        uart.receive(ctx, second_at, b"2");
+        run_to(ctx, &mut uart, timed_out_after(1));
+        assert_eq!(uart.read(ctx, INT_RAW).unwrap() & RXFIFO_TOUT, 0);
+        ctx.sleep_until(second_at + timed_out_after(1).get());
+        assert_eq!(uart.read(ctx, INT_RAW).unwrap() & RXFIFO_TOUT, RXFIFO_TOUT);
+    }
+
+    #[test]
+    fn a_byte_that_starts_at_the_instant_the_timeout_comes_finds_it_come() {
+        // The line was idle until then. A timeout of 2 bit times after one byte ends on a whole
+        // cycle: 444 400 + 2 x 44 440 thirty-seconds, 16 665 cycles.
+        let ctx = &mut Ctx::default();
+        let mut uart = driver(ctx);
+        uart.write(ctx, MEM_CONF, 0x0002_0012);
+        uart.receive(ctx, Cycles::ZERO, b"1");
+        uart.receive(ctx, Cycles(16_665), b"2");
+        run_to(ctx, &mut uart, Cycles(16_665));
+        assert_eq!(uart.read(ctx, INT_RAW).unwrap() & RXFIFO_TOUT, RXFIFO_TOUT);
+    }
+
+    #[test]
+    fn more_bytes_than_the_full_threshold_raise_rxfifo_full_and_a_clear_while_they_stay_does_not_lower_it()
+     {
+        // UR A1: count > threshold, and a level.
+        let ctx = &mut Ctx::default();
+        let mut uart = console(ctx);
+        uart.write(ctx, CONF1, 0x0000_C004); // full threshold 4
+        uart.write(ctx, INT_ENA, RXFIFO_FULL);
+        uart.receive(ctx, Cycles::ZERO, b"12345");
+        run_to(ctx, &mut uart, landed(4));
+        assert_eq!(
+            uart.read(ctx, INT_ST),
+            Some(0),
+            "four bytes: not more than 4"
+        );
+        assert_eq!(ctx.next_wake(), Some(landed(5)));
+        ctx.sleep_until(landed(5));
+        assert_eq!(ctx.take_due(), Some(Event::Uart(0)));
+        uart.wake(ctx);
+        assert_eq!(ctx.sources(), 1 << 21);
+        uart.write(ctx, INT_CLR, RXFIFO_FULL);
+        assert_eq!(
+            uart.read(ctx, INT_ST),
+            Some(RXFIFO_FULL),
+            "five bytes are still there"
+        );
+        uart.read(ctx, FIFO);
+        assert_eq!(
+            uart.read(ctx, INT_ST),
+            Some(RXFIFO_FULL),
+            "set until cleared"
+        );
+        uart.write(ctx, INT_CLR, RXFIFO_FULL);
+        assert_eq!(
+            uart.read(ctx, INT_ST),
+            Some(0),
+            "four left: cleared for good"
+        );
+        assert_eq!(ctx.sources(), 0);
+    }
+
+    #[test]
+    fn a_byte_that_finds_the_fifo_full_raises_rxfifo_ovf_unless_detection_is_off() {
+        for (conf1, raised) in [(0x0000_C060, RXFIFO_OVF), (0x0004_C060, 0)] {
+            let ctx = &mut Ctx::default();
+            let mut uart = console(ctx);
+            uart.write(ctx, CONF1, conf1);
+            let sent: Vec<u8> = (0..=128).collect();
+            uart.receive(ctx, Cycles::ZERO, &sent);
+            ctx.sleep_until(landed(128));
+            assert_eq!(uart.read(ctx, INT_RAW).unwrap() & RXFIFO_OVF, 0);
+            ctx.sleep_until(landed(129));
+            assert_eq!(
+                uart.read(ctx, INT_RAW).unwrap() & RXFIFO_OVF,
+                raised,
+                "{conf1:#x}"
+            );
+            assert_eq!(
+                ctx.take_unsupported(),
+                Some(Unsupported::UartRxOverflow { uart: 0 }),
+                "dropped and counted either way"
+            );
+        }
+    }
+
+    #[test]
+    fn the_transmit_fifo_falling_below_its_threshold_raises_txfifo_empty_at_that_instant() {
+        // UR §3.3: with no transmit buffer, the driver enables TXFIFO_EMPTY at threshold 10 and
+        // waits for it; without it the first long burst blocks for ever (UR A10).
+        let ctx = &mut Ctx::default();
+        let mut uart = console(ctx);
+        uart.write(ctx, CONF1, 10 << TXFIFO_EMPTY_THRHD_SHIFT);
+        for byte in 0..20u32 {
+            uart.write(ctx, FIFO, byte);
+        }
+        uart.write(ctx, INT_CLR, TXFIFO_EMPTY);
+        uart.write(ctx, INT_ENA, TXFIFO_EMPTY);
+        assert_eq!(
+            uart.read(ctx, INT_ST),
+            Some(0),
+            "19 in the FIFO, 1 on the wire"
+        );
+        // Ten bytes have to leave: the FIFO then holds 9.
+        let below = Cycles((10 * BYTE_SUBCYCLES).div_ceil(SUBCYCLES));
+        run_to(ctx, &mut uart, Cycles(below.get() - 1));
+        assert_eq!(uart.read(ctx, INT_ST), Some(0));
+        assert_eq!(ctx.next_wake(), Some(below), "the call at the crossing");
+        ctx.sleep_until(below);
+        assert_eq!(ctx.take_due(), Some(Event::Uart(0)));
+        uart.wake(ctx);
+        assert_eq!(ctx.sources(), 1 << 21);
+        assert_eq!(uart.read(ctx, INT_ST), Some(TXFIFO_EMPTY));
+    }
+
+    #[test]
+    fn tx_done_is_raised_when_the_last_byte_has_left_and_not_again_while_idle() {
+        let ctx = &mut Ctx::default();
+        let mut uart = console(ctx);
+        uart.write(ctx, INT_ENA, TX_DONE);
+        uart.write(ctx, FIFO, u32::from(b'k'));
+        uart.write(ctx, FIFO, u32::from(b'o'));
+        after_bytes(ctx, 1);
+        assert_eq!(uart.read(ctx, INT_ST), Some(0));
+        after_bytes(ctx, 1);
+        assert_eq!(uart.read(ctx, INT_ST), Some(TX_DONE));
+        uart.write(ctx, INT_CLR, TX_DONE);
+        after_bytes(ctx, 5);
+        assert_eq!(uart.read(ctx, INT_ST), Some(0), "an event, not a level");
+    }
+
+    #[test]
+    fn uart1_drives_source_22_and_a_reset_lowers_it() {
+        let ctx = &mut Ctx::default();
+        let mut uart1 = Uart::new(1, UART1);
+        uart1.write(ctx, INT_ENA, TXFIFO_EMPTY);
+        assert_eq!(ctx.sources(), 1 << 22, "UART1_INTR_MAP");
+        uart1.reset(ctx);
+        assert_eq!(ctx.sources(), 0, "INT_ENA back to 0");
+        assert_eq!(uart1.read(ctx, INT_RAW), Some(TXFIFO_EMPTY));
     }
 
     /// The instant, in cycles, at which the `n`th byte (from 1) the host sends at 0 has landed,
     /// at 115 200 baud 8N1 on the crystal.
     fn landed(n: u64) -> Cycles {
         gone_after(20 * n)
+    }
+
+    /// Moves device time to `instant` as the machine would: each call the UART asked for is
+    /// delivered at its own instant on the way. A UART that asks for a call at the instant it is
+    /// called again and again fails the test instead of hanging it.
+    fn run_to(ctx: &mut Ctx, uart: &mut Uart, instant: Cycles) {
+        let mut calls = 0;
+        while let Some(next) = ctx.next_wake().filter(|&next| next <= instant) {
+            ctx.sleep_until(next);
+            while ctx.take_due().is_some() {
+                calls += 1;
+                assert!(
+                    calls < 1_000,
+                    "the UART asks for a call at {next:?} for ever"
+                );
+                uart.wake(ctx);
+            }
+        }
+        ctx.sleep_until(instant);
     }
 
     fn rx_count(ctx: &mut Ctx, uart: &mut Uart) -> u32 {
```

```diff
--- a/crates/soc-esp32c3/tests/accepted_without_effect.rs
+++ b/crates/soc-esp32c3/tests/accepted_without_effect.rs
@@ -42,7 +42,7 @@
 /// before it ends, and its name is removed from this list by its last task. A plan to come that
 /// owns no row (SP1b-3e today) is not in the list: it is added with its first row.
 /// The list holds the table only: a comment or a document that names an heir is not read here.
-const PLANS_TO_COME: [&str; 7] = ["SP1b-3d", "SP1b-3f", "SP1b-4", "SP2", "SP5a", "SP5b", "SP8"];
+const PLANS_TO_COME: [&str; 6] = ["SP1b-3f", "SP1b-4", "SP2", "SP5a", "SP5b", "SP8"];
 
 #[test]
 fn every_row_hands_its_register_to_a_plan_that_is_still_to_come() {
```

- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p soc-esp32c3 --lib -- periph::uart:: > /tmp/t3-red.log 2>&1; echo "exit=$?"; grep -E "^error|FAILED|^test result" /tmp/t3-red.log | sort | uniq -c
```

Expected: `exit=101`; `could not compile soc-esp32c3 (lib test) due to 65 previous errors`, all `error[E0425]: cannot find value …`: `INT_ST`, `INT_RAW`, `RXFIFO_TOUT`, `TXFIFO_EMPTY`, `RXFIFO_FULL`, `TX_DONE`, `RXFIFO_OVF`, `TXFIFO_EMPTY_THRHD_SHIFT`, ….

- [ ] **Step 3: The interrupts**

```diff
--- a/crates/soc-esp32c3/src/periph/mod.rs
+++ b/crates/soc-esp32c3/src/periph/mod.rs
@@ -275,7 +275,6 @@
         gpio::accepted(),
         sensitive::accepted(),
         system::accepted(),
-        uart::accepted(),
         intmatrix::accepted(),
         assist_debug::accepted(),
         extmem::accepted(),
```

```diff
--- a/crates/soc-esp32c3/src/periph/uart.rs
+++ b/crates/soc-esp32c3/src/periph/uart.rs
@@ -8,29 +8,22 @@
 //! A byte the host sends lands in the receive FIFO one frame after its start bit (report UR
 //! §5.4): the bytes of one input go out back to back from the device time they are given, at the
 //! rate and in the frame the UART is set to. `STATUS` and a read of `FIFO` work the count out from
-//! device time when they are accessed (ADR 0003 §4), and the UART asks to be called at the
-//! instant each byte lands. The UART interrupts arrive with the next part of this plan.
+//! device time when they are accessed (ADR 0003 §4), and the UART asks to be called at each
+//! instant a byte lands or an interrupt condition begins.
+//!
+//! The interrupt (report UR §4): sources 21 (`UART_INTR_MAP`) and 22 (`UART1_INTR_MAP`), level,
+//! driven as `INT_RAW & INT_ENA != 0` after every change, as the SYSTIMER drives its own. Five
+//! bits of `INT_RAW` have an effect: `RXFIFO_FULL`, `TXFIFO_EMPTY`, `RXFIFO_OVF`, `RXFIFO_TOUT`
+//! and `TX_DONE` (report UR §10); the others are never set.
 use crate::clock::XTAL_PERIOD;
-use crate::ctx::{Ctx, Deadline, Event};
+use crate::ctx::{Ctx, Deadline, Event, Source};
 use crate::diag::{UartFeature, Unsupported};
-use crate::periph::{Accepted, Storage};
+use crate::periph::Storage;
 use emu_core::{Cycles, Peripheral};
 use std::collections::VecDeque;
 
-/// `INT_CLR` is write-only and accepted; the two others are stored.
-pub(crate) fn accepted() -> Vec<Accepted> {
-    let mut rows = Vec::new();
-    for peripheral in ["UART0", "UART1"] {
-        for (register, not_modeled) in [
-            ("INT_ENA", "interrupt enables: nothing is raised"),
-            ("INT_CLR", "interrupt clear: nothing is pending"),
-            ("CONF1", "FIFO thresholds"),
-        ] {
-            rows.push(Accepted::svd(peripheral, register, not_modeled, "SP1b-3d"));
-        }
-    }
-    rows
-}
+/// The interrupt source of each UART (report UR §4).
+const SOURCES: [Source; 2] = [Source::of("UART_INTR_MAP"), Source::of("UART1_INTR_MAP")];
 
 /// The model serves UART0 and UART1, whose register offsets are identical: those of UART0 name them.
 const fn reg(name: &str) -> u32 {
@@ -38,6 +31,8 @@
 }
 
 const FIFO: u32 = reg("FIFO");
+const INT_RAW: u32 = reg("INT_RAW");
+const INT_ST: u32 = reg("INT_ST");
 const INT_ENA: u32 = reg("INT_ENA");
 const INT_CLR: u32 = reg("INT_CLR");
 const CLKDIV: u32 = reg("CLKDIV");
@@ -80,6 +75,21 @@
 /// model has one block each and refuses another size.
 const MEM_SIZES: u32 = 0b111_1110;
 const ONE_BLOCK_EACH: u32 = 0b001_0010;
+/// `MEM_CONF` bits [25:16] `RX_TOUT_THRHD`: the receive timeout, in bit times (report UR §5.2).
+const RX_TOUT_THRHD_SHIFT: u32 = 16;
+const TEN_BITS: u32 = 0x3FF;
+/// `CONF1` (report UR §2.2): bits [8:0] `RXFIFO_FULL_THRHD`, [17:9] `TXFIFO_EMPTY_THRHD`, bit 18
+/// `DIS_RX_DAT_OVF` ("Disable UART Rx data overflow detect"), bit 21 `RX_TOUT_EN`.
+const NINE_BITS: u32 = 0x1FF;
+const TXFIFO_EMPTY_THRHD_SHIFT: u32 = 9;
+const DIS_RX_DAT_OVF: u32 = 1 << 18;
+const RX_TOUT_EN: u32 = 1 << 21;
+/// The bits of `INT_RAW` the model sets (`uart_struct.h:20-45`, report UR §2.3).
+const RXFIFO_FULL: u32 = 1 << 0;
+const TXFIFO_EMPTY: u32 = 1 << 1;
+const RXFIFO_OVF: u32 = 1 << 4;
+const RXFIFO_TOUT: u32 = 1 << 8;
+const TX_DONE: u32 = 1 << 14;
 /// `FSM_STATUS` bits [7:4] `ST_UTX_OUT`: 0 when the transmitter is idle, the only value software
 /// tests (`ROM3:uart_tx_wait_idle@0x4004c4c6`; BA §7).
 /// ASSUMPTION: while a byte is being sent the field reads 1. The other states are not documented
@@ -167,9 +177,15 @@
     line_free_at: u64,
     /// The receive FIFO.
     rx: VecDeque<u8>,
+    /// `INT_RAW`.
+    int_raw: u32,
+    /// The instant the last byte landed, while the receive timeout of that idle period has not
+    /// come: `None` once it came, and from the start bit of the next byte to its landing.
+    idle_since: Option<u64>,
     /// The call at the next instant the device has something to do: a byte of its transmitter
     /// leaves (so that a CPU that waits for an interrupt with bytes on their way out sleeps
-    /// until they have left, not for ever), or a byte of the host lands.
+    /// until they have left, not for ever), a byte of the host lands, the receive timeout comes,
+    /// or the transmit FIFO falls below its threshold.
     wake: Deadline,
 }
 
@@ -190,6 +206,10 @@
             receiving: None,
             line_free_at: 0,
             rx: VecDeque::new(),
+            // `TXFIFO_EMPTY` at reset: the SVD's `0x00000002`, the condition with an empty FIFO
+            // and the reset threshold, 96.
+            int_raw: TXFIFO_EMPTY,
+            idle_since: None,
             wake: Deadline::default(),
         }
     }
@@ -203,6 +223,10 @@
         self.config = Storage::for_offsets(self.base, &CONFIG);
         self.in_flight.clear();
         self.rx.clear();
+        self.int_raw = 0;
+        self.idle_since = None;
+        self.assert_levels();
+        self.drive(ctx);
         self.arm(ctx);
     }
 
@@ -256,12 +280,22 @@
         self.arm(ctx);
     }
 
-    /// Asks to be called when the last byte of the transmitter will have left, and when the next
-    /// byte of the host lands.
+    /// Asks to be called at the next instant an interrupt condition can begin: the last byte of
+    /// the transmitter leaves (`TX_DONE`), the transmit FIFO falls below its threshold
+    /// (`TXFIFO_EMPTY`), the next byte of the host lands (`RXFIFO_FULL`, `RXFIFO_OVF`), the
+    /// receive timeout comes (`RXFIFO_TOUT`). A landing that changes no bit is a call all the
+    /// same: one per byte received, the price of being right whatever the firmware enables.
     fn arm(&mut self, ctx: &mut Ctx) {
         let next_landing = self.next_landing(ctx);
         let last_gone = self.in_flight.back().map(|&(gone_at, _)| gone_at);
-        for instant in [last_gone, next_landing].into_iter().flatten() {
+        // A timeout that is not enabled raises nothing at its instant: no call.
+        let timeout_enabled = self.config.read(CONF1).unwrap_or(0) & RX_TOUT_EN != 0;
+        let timeout = self.timeout_at(ctx).filter(|_| timeout_enabled);
+        let below_threshold = self.tx_below_threshold_at();
+        for instant in [last_gone, next_landing, timeout, below_threshold]
+            .into_iter()
+            .flatten()
+        {
             self.wake.arm(
                 ctx,
                 Cycles(instant.div_ceil(SUBCYCLES)),
@@ -284,10 +318,38 @@
         )
     }
 
+    /// The instant the transmit FIFO count falls below `TXFIFO_EMPTY_THRHD`, when it is not below
+    /// it yet: once `n − threshold` of the `n` bytes on their way out have left (the first is in
+    /// the shift register, so the count is `n − 1`). A threshold of 0 is never crossed.
+    fn tx_below_threshold_at(&self) -> Option<u64> {
+        let threshold = usize::try_from(self.tx_empty_threshold()).unwrap_or(usize::MAX);
+        let on_the_way = self.in_flight.len();
+        if threshold == 0 || on_the_way <= threshold {
+            return None;
+        }
+        self.in_flight
+            .get(on_the_way - threshold - 1)
+            .map(|&(gone_at, _)| gone_at)
+    }
+
+    fn tx_empty_threshold(&self) -> u32 {
+        (self.config.read(CONF1).unwrap_or(0) >> TXFIFO_EMPTY_THRHD_SHIFT) & NINE_BITS
+    }
+
+    /// The instant the receive timeout of the current idle period comes: `RX_TOUT_THRHD` bit
+    /// times after the last byte landed (report UR §5.2), timed with the settings of now.
+    fn timeout_at(&self, ctx: &mut Ctx) -> Option<u64> {
+        let since = self.idle_since?;
+        let threshold = (self.config.read(MEM_CONF).unwrap_or(0) >> RX_TOUT_THRHD_SHIFT) & TEN_BITS;
+        Some(since.saturating_add(u64::from(threshold) * self.bit_time(self.source_period(ctx))))
+    }
+
     /// Brings the device to now: the bytes whose last bit has gone move to the console, the bytes
-    /// of the host whose frame has ended land, in the order of their instants.
+    /// of the host whose frame has ended land, and the receive timeout comes, in the order of
+    /// their instants; the interrupt source follows.
     fn settle(&mut self, ctx: &mut Ctx) {
         let now = ctx.now().get().saturating_mul(SUBCYCLES);
+        let was_sending = !self.in_flight.is_empty();
         while let Some(&(gone_at, byte)) = self.in_flight.front() {
             if gone_at > now {
                 break;
@@ -295,32 +357,103 @@
             self.tx.push(byte);
             self.in_flight.pop_front();
         }
-        // INVARIANT: each pass lands a byte or ends the loop, and the bytes are those the caller
-        // handed over within its bound (`host_pending`).
+        if was_sending && self.in_flight.is_empty() {
+            self.int_raw |= TX_DONE;
+        }
+        // INVARIANT: each pass lands a byte, starts one, ends the timeout or ends the loop, and
+        // the bytes are those the caller handed over within its bound (`host_pending`).
         loop {
+            let timeout = self.timeout_at(ctx).filter(|&at| at <= now);
             if self.receiving.is_none() {
                 let Some(&(date, byte)) = self.host.front() else {
+                    if timeout.is_some() {
+                        self.time_out();
+                        continue;
+                    }
                     break;
                 };
                 let starts_at = date.max(self.line_free_at);
+                // A start bit at the instant the timeout comes finds the line idle until then:
+                // the timeout comes first.
+                if timeout.is_some_and(|at| at <= starts_at) {
+                    self.time_out();
+                    continue;
+                }
                 if starts_at > now {
                     break;
                 }
                 // The frame of a byte is the one the UART is set to when it starts: a register
-                // write settles before it changes the setting.
+                // write settles before it changes the setting. A start bit ends the idle line:
+                // the timeout it put off is not asked for again (`arm`), and the landing starts
+                // the count again.
                 let lands_at = starts_at.saturating_add(self.byte_time(self.source_period(ctx)));
                 self.host.pop_front();
                 self.receiving = Some((lands_at, byte));
                 self.line_free_at = lands_at;
+                self.idle_since = None;
             }
             match self.receiving {
                 Some((lands_at, byte)) if lands_at <= now => {
                     self.receiving = None;
                     self.land(ctx, byte);
+                    self.idle_since = Some(lands_at);
                 }
                 _ => break,
             }
         }
+        self.assert_levels();
+        self.drive(ctx);
+    }
+
+    /// The receive timeout came: it is raised when it is enabled and the FIFO holds data, and
+    /// comes once per idle period.
+    ///
+    /// ASSUMPTION: `RXFIFO_TOUT` needs a byte in the FIFO and an idle line for `RX_TOUT_THRHD`
+    /// bit times after the last byte landed; a read of the FIFO does not start the count again;
+    /// it comes once per idle period (report UR A3). QEMU raises it with an empty FIFO; ESP-IDF
+    /// leaves a byte in the FIFO "in order to trigger `uart_intr_rxfifo_tout`" (`uart.c:1275-1277`).
+    /// A logic analyser on the receive line of a chip, with a GPIO toggled by the handler,
+    /// settles it.
+    fn time_out(&mut self) {
+        self.idle_since = None;
+        let conf1 = self.config.read(CONF1).unwrap_or(0);
+        if conf1 & RX_TOUT_EN != 0 && !self.rx.is_empty() {
+            self.int_raw |= RXFIFO_TOUT;
+        }
+    }
+
+    /// Sets the bits whose condition holds now.
+    ///
+    /// ASSUMPTION: `RXFIFO_FULL` (the FIFO holds more than `RXFIFO_FULL_THRHD` bytes) and
+    /// `TXFIFO_EMPTY` (the transmit FIFO holds fewer than `TXFIFO_EMPTY_THRHD`) are levels: a
+    /// write of `INT_CLR` while the condition holds sets the bit again at once, and a set bit
+    /// stays until it is cleared, after the condition ends too; "more than" is strict (report UR
+    /// A1, from "receives more data than"). The ESP-IDF driver works with either reading (it
+    /// drains the FIFO before it clears `RXFIFO_FULL`, and disables `TXFIFO_EMPTY` before it
+    /// clears it), and so does the ROM's download handler. Clearing the bit on a chip with the
+    /// FIFO above the threshold, and sending exactly `RXFIFO_FULL_THRHD` bytes, settles it.
+    fn assert_levels(&mut self) {
+        let conf1 = self.config.read(CONF1).unwrap_or(0);
+        let full_threshold = usize::try_from(conf1 & NINE_BITS).unwrap_or(usize::MAX);
+        if self.rx.len() > full_threshold {
+            self.int_raw |= RXFIFO_FULL;
+        }
+        let in_fifo = self.in_flight.len().saturating_sub(1);
+        if in_fifo < usize::try_from(self.tx_empty_threshold()).unwrap_or(usize::MAX) {
+            self.int_raw |= TXFIFO_EMPTY;
+        }
+    }
+
+    /// The level of the source: `INT_ST`, `INT_RAW & INT_ENA`, other than 0.
+    fn drive(&self, ctx: &mut Ctx) {
+        // INVARIANT: the machine builds UART 0 and UART 1 only.
+        if let Some(&source) = SOURCES.get(usize::from(self.index)) {
+            ctx.irq(source, self.int_status() != 0);
+        }
+    }
+
+    fn int_status(&self) -> u32 {
+        self.int_raw & self.config.read(INT_ENA).unwrap_or(0)
     }
 
     /// A byte of the host has been received whole: it enters the FIFO.
@@ -337,6 +470,9 @@
         }
         if self.rx.len() >= FIFO_BYTES {
             ctx.unsupported(Unsupported::UartRxOverflow { uart: self.index });
+            if self.config.read(CONF1).unwrap_or(0) & DIS_RX_DAT_OVF == 0 {
+                self.int_raw |= RXFIFO_OVF;
+            }
             return;
         }
         self.rx.push_back(byte);
@@ -393,15 +529,24 @@
     /// says what the chip does below a divider of 1 (ESP-IDF never programs one); a byte timed on
     /// a chip with `CLKDIV` = 0 settles it.
     fn byte_time(&self, source_period: u64) -> u64 {
+        let conf0 = self.config.read(CONF0).unwrap_or(0);
+        frame_half_bits(conf0) * self.half_bit_time(source_period)
+    }
+
+    /// The time of one bit, in thirty-seconds of a time-base cycle: what `RX_TOUT_THRHD` counts.
+    fn bit_time(&self, source_period: u64) -> u64 {
+        2 * self.half_bit_time(source_period)
+    }
+
+    /// Half a bit time, in thirty-seconds of a time-base cycle: the divider in sixteenths of a
+    /// source-clock cycle.
+    fn half_bit_time(&self, source_period: u64) -> u64 {
         let clkdiv = self.config.read(CLKDIV).unwrap_or(0);
         let clk_conf = self.config.read(CLK_CONF).unwrap_or(0);
-        let conf0 = self.config.read(CONF0).unwrap_or(0);
-        // The divider in sixteenths of a source-clock cycle, a frame in half bits: the product
-        // is in thirty-seconds.
         let divider = u64::from(clkdiv & CLKDIV_INTEGER_MASK) * 16
             + u64::from((clkdiv >> CLKDIV_FRAG_SHIFT) & CLKDIV_FRAG_MASK);
         let source_divider = u64::from((clk_conf >> SCLK_DIV_NUM_SHIFT) & SCLK_DIV_NUM_MASK) + 1;
-        frame_half_bits(conf0) * divider * source_divider * source_period
+        divider * source_divider * source_period
     }
 
     /// Refuses by name the modes of `conf0` the model does not have (report UR §10, A4).
@@ -466,6 +611,8 @@
                 ST_UTX_OUT_BUSY
             }),
             MEM_RX_STATUS => Some(self.mem_rx_status),
+            INT_RAW => Some(self.int_raw),
+            INT_ST => Some(self.int_status()),
             _ => self.config.read(offset),
         }
     }
@@ -477,8 +624,12 @@
                 self.send(ctx, value.to_le_bytes()[0]);
                 true
             }
-            // No interrupt condition is modelled yet, so there is nothing to clear.
-            INT_CLR => true,
+            // Write 1 to clear (`uart_reg.h:422-530`); a level whose condition holds is set again
+            // below (`assert_levels`).
+            INT_CLR => {
+                self.int_raw &= !value;
+                true
+            }
             CONF0 => {
                 if value & TXFIFO_RST != 0 {
                     // The byte in the shift register is on the wire already: it goes on.
@@ -510,7 +661,10 @@
             }
             _ => self.config.write(offset, value),
         };
-        // The baud rate or the frame may have changed: so has the instant the next byte lands.
+        // A threshold, an enable or a clear may have changed a bit or the level; the baud rate
+        // or the frame may have changed the instant the next byte lands.
+        self.assert_levels();
+        self.drive(ctx);
         self.arm(ctx);
         accepted
     }
```

```diff
--- a/docs/registers-accepted-without-effect.md
+++ b/docs/registers-accepted-without-effect.md
@@ -31,8 +31,8 @@
 
 "Count" is 1 for a single register, or the number of consecutive registers the row stands for.
 "Made real by" names the plan that gives the register its effect. Each plan inherits the rows that
-name it. The table has seven heirs: SP1b-3d (the serial consoles in both directions), SP1b-3f
-(deep sleep), SP1b-4, SP2, SP5a, SP5b and SP8. The carry-over note
+name it. The table has six heirs: SP1b-3f (deep sleep), SP1b-4, SP2, SP5a, SP5b and SP8. The
+UART rows SP1b-3d inherited left the table when it gave them their effect. The carry-over note
 `docs/superpowers/notes/2026-10-11-sp1b-3a-carryover.md` names the rows one by one, as the table
 stood at the end of plan SP1b-3a, when one plan, "SP1b-3b", was to take what five plans now share;
 its first section says which plan takes what.
@@ -40,9 +40,6 @@
 <!-- generated table -->
 | Address | Register | Count | Hardware effect not modelled | Made real by |
 |---|---|---|---|---|
-| `0x6000000c` | `UART0.INT_ENA` | 1 | interrupt enables: nothing is raised | SP1b-3d |
-| `0x60000010` | `UART0.INT_CLR` | 1 | interrupt clear: nothing is pending | SP1b-3d |
-| `0x60000024` | `UART0.CONF1` | 1 | FIFO thresholds | SP1b-3d |
 | `0x60002008` | `SPI1.CTRL` | 1 | read mode, dummy output, WP level (the two-byte status write bit acts) | SP8 |
 | `0x6000200c` | `SPI1.CTRL1` | 1 | CS hold delay after a release-from-power-down | SP8 |
 | `0x60002010` | `SPI1.CTRL2` | 1 | CS setup and hold times | SP8 |
@@ -98,9 +95,6 @@
 | `0x6000e040` | `ANA_I2C.I2C_MST_ANA_CONF0` | 1 | analog register bus: bytes are stored, the analog blocks (PLL, SAR ADC, bias) do nothing | SP2 |
 | `0x6000e044` | `ANA_I2C.ANA_CONFIG` | 1 | analog register bus: bytes are stored, the analog blocks (PLL, SAR ADC, bias) do nothing | SP2 |
 | `0x6000e048` | `ANA_I2C.ANA_CONFIG2` | 1 | analog register bus: bytes are stored, the analog blocks (PLL, SAR ADC, bias) do nothing | SP2 |
-| `0x6001000c` | `UART1.INT_ENA` | 1 | interrupt enables: nothing is raised | SP1b-3d |
-| `0x60010010` | `UART1.INT_CLR` | 1 | interrupt clear: nothing is pending | SP1b-3d |
-| `0x60010024` | `UART1.CONF1` | 1 | FIFO thresholds | SP1b-3d |
 | `0x6001ccd4` | `NRX.NRXPD_CTRL` | 1 | radio memory power: there is no radio | SP5a |
 | `0x6001d054` | `BB.BBPD_CTRL` | 1 | radio memory power: there is no radio | SP5a |
 | `0x60026014` | `APB_CTRL.WIFI_CLK_EN` | 1 | radio clock gates: there is no radio | SP5a |
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p soc-esp32c3 --lib -- periph::uart:: > /tmp/t3.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t3.log
cargo test -p soc-esp32c3 --test accepted_without_effect > /tmp/t3-a.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t3-a.log
```

Expected: `exit=0` and `test result: ok. 45 passed; 0 failed; 0 ignored; 0 measured; 498 filtered out`; then `exit=0` and `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.

- [ ] **Step 5: Mutations**

| # | Mutation (`crates/soc-esp32c3/src/periph/uart.rs`) | Test that fails |
|---|---|---|
| 1 | `if self.rx.len() > full_threshold {` → `>=` | `more_bytes_than_the_full_threshold_raise_rxfifo_full_and_a_clear_while_they_stay_does_not_lower_it` |
| 2 | `if conf1 & RX_TOUT_EN != 0 && !self.rx.is_empty() {` → `if conf1 & RX_TOUT_EN != 0 {` | `the_timeout_comes_once_per_idle_period_and_never_with_an_empty_fifo` |
| 3 | delete `self.idle_since = None;` after `self.line_free_at = lands_at;` | `a_byte_that_starts_before_the_timeout_comes_puts_it_off` |
| 4 | `if timeout.is_some_and(\|at\| at <= starts_at)` → `at < starts_at` | `a_byte_that_starts_at_the_instant_the_timeout_comes_finds_it_come` |
| 5 | delete `self.int_raw &= !value;` in the `INT_CLR` arm | `the_raw_state_resets_to_txfifo_empty_and_int_st_is_the_enabled_part_of_it` and two others |
| 6 | `for instant in [last_gone, next_landing, timeout, below_threshold]` → without `below_threshold` | `the_transmit_fifo_falling_below_its_threshold_raises_txfifo_empty_at_that_instant` |
| 7 | `if self.config.read(CONF1).unwrap_or(0) & DIS_RX_DAT_OVF == 0 {` → `if true {` | `a_byte_that_finds_the_fifo_full_raises_rxfifo_ovf_unless_detection_is_off` |
| 8 | `let timeout = self.timeout_at(ctx).filter(\|_\| timeout_enabled);` → `let timeout = self.timeout_at(ctx);` | `the_uart_asks_to_be_called_at_the_instant_each_byte_lands` |

Mutation 3 makes the UART ask, again and again, to be woken at a timeout instant already past: `run_to`, the helper of these tests, delivers at most 1,000 wake-ups, so the test fails instead of hanging.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy -p soc-esp32c3 --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git commit -m "feat(soc-esp32c3): the UART interrupts: RXFIFO_FULL, TXFIFO_EMPTY, RXFIFO_OVF, RXFIFO_TOUT and TX_DONE drive sources 21 and 22" -- crates/soc-esp32c3 docs/registers-accepted-without-effect.md
```

Expected: three `exit=0`, then the commit.

---

## Task 4: USB-Serial-JTAG receives the host's packets on EP1 and raises its interrupts on source 26

USB-Serial-JTAG in both directions (USJ §R.1, §R.3, C5). The IN endpoint: 64 bytes, written through `EP1`, committed by `EP1_CONF.WR_DONE` (a commit of nothing is a zero-length packet) or by the 64th byte; `EP1_CONF.SERIAL_IN_EP_DATA_FREE` is 0 while a packet waits; the bytes reach the console when the packet is committed (the host model comes in Task 5). The OUT endpoint: packets of at most 64 bytes from `UsbSerialJtag::receive` (a chunk of no byte is one zero-length packet), each loaded `HOST_LATENCY` after its date or after the previous one was read, raising `SERIAL_OUT_RECV_PKT` (and `OUT_EP1_ZERO_PAYLOAD` for a zero-length one); `EP1` reads pop (empty → 0 and `Unsupported::UsbEmptyRead`). `INT_RAW`, `INT_ENA`, `INT_CLR`, `INT_ST` with source 26 (`USB_INTR_MAP`) as a level; `SERIAL_IN_EMPTY` starts at 1, which the first interrupt of Arduino's `HWCDC::begin` relies on. `CONF0`, `TEST`, `MISC_CONF`, `MEM_CONF`, `DATE` are storage at their reset values; the endpoint state registers read their reset values (`ASSUMPTION:`, no consumer reads them); `FRAM_NUM` is served by the model. A core reset resets the block (C20).

**Files:**
- Modify: `crates/soc-esp32c3/src/periph/usb_serial_jtag.rs`, `crates/soc-esp32c3/src/diag.rs`, `crates/soc-esp32c3/src/ctx.rs`, `crates/soc-esp32c3/src/periph/mod.rs`, `crates/soc-esp32c3/src/machine.rs`, `crates/soc-esp32c3/src/bus.rs`

**Interfaces:**
- Consumes: `Deadline`, `Ctx::irq`, `Source::USB_INTR_MAP`.
- Produces: `pub const PACKET_BYTES: usize = 64`; `pub const HOST_LATENCY: u64` (50 us in cycles of the time base); `Event::UsbSerialJtag`; `pub fn UsbSerialJtag::new() -> UsbSerialJtag`; `pub(crate) fn reset(&mut self, ctx: &mut Ctx)`; `pub fn take_tx(&mut self, ctx: &mut Ctx) -> Vec<u8>`; `pub fn take_written(&mut self) -> Vec<u8>`; `pub fn receive(&mut self, ctx: &mut Ctx, at: Cycles, bytes: &[u8])`; `pub fn host_pending(&self) -> usize`; `pub(crate) fn wake(&mut self, ctx: &mut Ctx)`; `Unsupported::UsbEmptyRead` (key 26).

**Bounds on untrusted input.** The IN FIFO holds at most 64 bytes; a host chunk is split into packets of at most `PACKET_BYTES`; the packets queued are bounded by the machine (Task 6).

- [ ] **Step 1: Write the failing tests**

```diff
--- a/crates/soc-esp32c3/src/bus.rs
+++ b/crates/soc-esp32c3/src/bus.rs
@@ -836,7 +836,7 @@
         // of the time base).
         b.ctx.sleep_until(emu_core::Cycles(60_000));
         assert_eq!(b.periph.uart0.take_tx(&mut b.ctx), b"U");
-        assert_eq!(b.periph.usb_serial_jtag.take_tx(), b"J");
+        assert_eq!(b.periph.usb_serial_jtag.take_tx(&mut b.ctx), b"J");
         assert_eq!(b.ctx.diag.drain(), []);
     }
 
```

```diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -1310,6 +1310,10 @@
                 Unsupported::UartEmptyRead { uart: 0 },
                 "UART0.FIFO was read while the receive FIFO was empty (pc 0x40381000), so 0 was returned",
             ),
+            (
+                Unsupported::UsbEmptyRead,
+                "USB_DEVICE.EP1 was read while the OUT FIFO was empty (pc 0x40381000)",
+            ),
         ];
         for (what, needle) in texts {
             assert_cause_then_remedy(&unsupported(what), needle);
```

```diff
--- a/crates/soc-esp32c3/src/periph/usb_serial_jtag.rs
+++ b/crates/soc-esp32c3/src/periph/usb_serial_jtag.rs
@@ -78,62 +78,238 @@
     use super::*;
     use emu_core::Peripheral;
 
+    const USB_SOURCE: u64 = 1 << 26;
+
     fn send(ctx: &mut Ctx, usb: &mut UsbSerialJtag, text: &[u8]) {
         for &byte in text {
-            assert!(usb.write(ctx, 0x00, u32::from(byte)));
-        }
-    }
-
-    #[test]
-    fn endpoint_always_has_room_and_no_received_data() {
-        let ctx = &mut Ctx::default();
-        let mut usb = UsbSerialJtag::new();
-        let conf = usb.read(ctx, 0x04).unwrap();
-        assert_eq!(
-            conf & 0b010,
-            0b010,
-            "SERIAL_IN_EP_DATA_FREE: the ROM drops the byte otherwise"
-        );
-        assert_eq!(
-            conf & 0b101,
-            0,
-            "WR_DONE reads back clear, no data from the host"
-        );
+            assert!(usb.write(ctx, EP1, u32::from(byte)));
+        }
+    }
+
+    /// The bytes the firmware reads from `EP1` while `SERIAL_OUT_EP_DATA_AVAIL` says there are
+    /// some, as the driver's handler does (report USJ §B.4).
+    fn read_packet(ctx: &mut Ctx, usb: &mut UsbSerialJtag) -> Vec<u8> {
+        let mut bytes = Vec::new();
+        while usb.read(ctx, EP1_CONF).unwrap() & SERIAL_OUT_EP_DATA_AVAIL != 0 {
+            bytes.push(u8::try_from(usb.read(ctx, EP1).unwrap()).unwrap());
+        }
+        bytes
+    }
+
+    /// Moves device time to `instant`, delivering each call the block asked for on the way.
+    fn run_to(ctx: &mut Ctx, usb: &mut UsbSerialJtag, instant: Cycles) {
+        let mut calls = 0;
+        while let Some(next) = ctx.next_wake().filter(|&next| next <= instant) {
+            ctx.sleep_until(next);
+            while ctx.take_due().is_some() {
+                calls += 1;
+                assert!(
+                    calls < 1_000,
+                    "the block asks for a call at {next:?} for ever"
+                );
+                usb.wake(ctx);
+            }
+        }
+        ctx.sleep_until(instant);
+    }
+
+    #[test]
+    fn the_registers_start_at_their_svd_reset_values() {
+        // Report USJ §A.
+        let ctx = &mut Ctx::default();
+        let mut usb = UsbSerialJtag::new();
+        for (name, reset) in [
+            ("EP1_CONF", 0x2),
+            ("INT_RAW", 0x8),
+            ("INT_ST", 0),
+            ("INT_ENA", 0),
+            ("CONF0", 0x4200),
+            ("TEST", 0),
+            ("JFIFO_ST", 0x44),
+            ("FRAM_NUM", 0),
+            ("IN_EP0_ST", 1),
+            ("IN_EP3_ST", 1),
+            ("OUT_EP1_ST", 0),
+            ("MISC_CONF", 0),
+            ("MEM_CONF", 0x2),
+            ("DATE", 0x0200_7300),
+        ] {
+            assert_eq!(usb.read(ctx, reg(name)), Some(reset), "{name}");
+        }
+        assert!(usb.write(ctx, reg("CONF0"), 0x4204));
+        assert_eq!(usb.read(ctx, reg("CONF0")), Some(0x4204), "stored");
+        assert!(!usb.write(ctx, reg("JFIFO_ST"), 0), "read-only");
+        assert!(!usb.write(ctx, reg("IN_EP1_ST"), 0), "read-only");
+        assert_eq!(usb.read(ctx, 0x7C), None, "no register there");
+        assert_eq!(ctx.sources(), 0);
     }
 
     #[test]
     fn bytes_reach_the_host_when_the_packet_is_committed() {
-        let ctx = &mut Ctx::default();
-        let mut usb = UsbSerialJtag::new();
-        send(ctx, &mut usb, b"boot\n");
-        assert_eq!(usb.take_tx(), [], "nothing is sent before WR_DONE");
-        assert!(usb.write(ctx, 0x04, 1));
-        assert_eq!(usb.take_tx(), b"boot\n");
-        assert_eq!(usb.take_tx(), []);
-    }
-
-    #[test]
-    fn full_packet_is_sent_without_waiting_for_a_commit() {
+        // The ROM: read EP1_CONF bit 1, write the byte, WR_DONE after `\n` (report USJ §B.1).
+        let ctx = &mut Ctx::default();
+        let mut usb = UsbSerialJtag::new();
+        usb.write(ctx, INT_CLR, SERIAL_IN_EMPTY);
+        for &byte in b"boot\n" {
+            assert_eq!(usb.read(ctx, EP1_CONF).unwrap() & SERIAL_IN_EP_DATA_FREE, 2);
+            usb.write(ctx, EP1, u32::from(byte));
+        }
+        assert_eq!(usb.take_tx(ctx), [], "nothing is sent before WR_DONE");
+        assert!(usb.write(ctx, EP1_CONF, WR_DONE));
+        assert_eq!(usb.take_tx(ctx), b"boot\n");
+        assert_eq!(usb.take_tx(ctx), []);
+        assert_eq!(
+            usb.read(ctx, INT_RAW),
+            Some(SERIAL_IN_EMPTY),
+            "the host took the packet"
+        );
+        assert_eq!(
+            usb.read(ctx, EP1_CONF),
+            Some(SERIAL_IN_EP_DATA_FREE),
+            "WR_DONE reads 0"
+        );
+    }
+
+    #[test]
+    fn a_full_packet_is_sent_without_waiting_for_a_commit() {
         let ctx = &mut Ctx::default();
         let mut usb = UsbSerialJtag::new();
         send(ctx, &mut usb, &[b'x'; 64]);
-        assert_eq!(usb.take_tx().len(), 64);
+        assert_eq!(usb.take_tx(ctx).len(), 64);
         send(ctx, &mut usb, b"y");
-        assert_eq!(usb.take_tx(), []);
-    }
-
-    #[test]
-    fn committing_an_empty_packet_sends_nothing_and_other_registers_are_unmodeled() {
-        let ctx = &mut Ctx::default();
-        let mut usb = UsbSerialJtag::new();
-        assert!(usb.write(ctx, 0x04, 1));
-        assert_eq!(usb.take_tx(), []);
-        assert_eq!(
-            usb.read(ctx, 0x08),
-            None,
-            "INT_RAW arrives with interrupts (SP1b-3d)"
-        );
-        assert!(!usb.write(ctx, 0x18, 0), "CONF0");
-        assert_eq!(usb.read(ctx, 0x00), None, "no host-to-device data yet");
+        assert_eq!(usb.take_tx(ctx), []);
+        assert_eq!(usb.take_written(), b"y", "the end of a run takes the rest");
+    }
+
+    #[test]
+    fn a_commit_of_nothing_is_a_zero_length_packet_the_host_takes() {
+        // The driver ends a transfer of 64 bytes with one (report USJ §B.4).
+        let ctx = &mut Ctx::default();
+        let mut usb = UsbSerialJtag::new();
+        usb.write(ctx, INT_CLR, SERIAL_IN_EMPTY);
+        assert!(usb.write(ctx, EP1_CONF, WR_DONE));
+        assert_eq!(usb.take_tx(ctx), []);
+        assert_eq!(usb.read(ctx, INT_RAW), Some(SERIAL_IN_EMPTY));
+    }
+
+    #[test]
+    fn hwcdc_begin_takes_its_first_interrupt_from_the_reset_value_of_serial_in_empty() {
+        // Report USJ §B.5: `begin` clears the 19 enables, sets bits 2, 3 and 9, and never writes
+        // INT_CLR: the reset value of SERIAL_IN_EMPTY runs the handler at once.
+        let ctx = &mut Ctx::default();
+        let mut usb = UsbSerialJtag::new();
+        usb.write(ctx, reg("CONF0"), 0x4200);
+        let ena = usb.read(ctx, INT_ENA).unwrap();
+        usb.write(ctx, INT_ENA, ena & !INTERRUPT_BITS);
+        let ena = usb.read(ctx, INT_ENA).unwrap();
+        usb.write(
+            ctx,
+            INT_ENA,
+            ena | SERIAL_OUT_RECV_PKT | SERIAL_IN_EMPTY | (1 << 9),
+        );
+        assert_eq!(usb.read(ctx, INT_ST), Some(SERIAL_IN_EMPTY));
+        assert_eq!(ctx.sources(), USB_SOURCE, "source 26, USB_INTR_MAP");
+        usb.write(ctx, INT_CLR, SERIAL_IN_EMPTY);
+        assert_eq!(ctx.sources(), 0);
+        assert_eq!(usb.read(ctx, INT_ENA), Some(0x20C));
+        usb.write(ctx, INT_ENA, 0xFFFF_FFFF);
+        assert_eq!(usb.read(ctx, INT_ENA), Some(0x7FFFF), "19 bits stored");
+    }
+
+    #[test]
+    fn a_packet_of_the_host_reaches_the_fifo_after_the_host_latency_and_raises_its_interrupt() {
+        // The driver at install enables SERIAL_OUT_RECV_PKT only; its handler clears it, then
+        // reads while SERIAL_OUT_EP_DATA_AVAIL (report USJ §B.4, §F.1).
+        let ctx = &mut Ctx::default();
+        let mut usb = UsbSerialJtag::new();
+        usb.write(ctx, INT_ENA, SERIAL_OUT_RECV_PKT);
+        usb.receive(ctx, Cycles(1_000), b"hi, espressif\n");
+        assert_eq!(ctx.next_wake(), Some(Cycles(1_000 + HOST_LATENCY)));
+        run_to(ctx, &mut usb, Cycles(1_000 + HOST_LATENCY - 1));
+        assert_eq!(
+            usb.read(ctx, EP1_CONF).unwrap() & SERIAL_OUT_EP_DATA_AVAIL,
+            0
+        );
+        assert_eq!(ctx.sources(), 0);
+        run_to(ctx, &mut usb, Cycles(1_000 + HOST_LATENCY));
+        assert_eq!(ctx.sources(), USB_SOURCE, "raised at its instant");
+        assert_eq!(usb.read(ctx, INT_ST), Some(SERIAL_OUT_RECV_PKT));
+        usb.write(ctx, INT_CLR, SERIAL_OUT_RECV_PKT);
+        assert_eq!(read_packet(ctx, &mut usb), b"hi, espressif\n");
+        assert_eq!(ctx.sources(), 0);
+        assert_eq!(usb.host_pending(), 0);
+        assert_eq!(ctx.take_unsupported(), None);
+    }
+
+    #[test]
+    fn a_long_input_arrives_in_packets_of_64_each_after_the_previous_one_was_read() {
+        // Report USJ §F.1: a 90-byte input is read as 64 + 26.
+        let ctx = &mut Ctx::default();
+        let mut usb = UsbSerialJtag::new();
+        let input: Vec<u8> = (0..90).collect();
+        usb.receive(ctx, Cycles::ZERO, &input);
+        assert_eq!(usb.host_pending(), 90);
+        run_to(ctx, &mut usb, Cycles(HOST_LATENCY));
+        let first = read_packet(ctx, &mut usb);
+        assert_eq!(first, &input[..64]);
+        let read_at = ctx.now();
+        run_to(ctx, &mut usb, read_at + (HOST_LATENCY - 1));
+        assert_eq!(
+            read_packet(ctx, &mut usb),
+            [],
+            "the host waits for the FIFO to be read"
+        );
+        run_to(ctx, &mut usb, read_at + HOST_LATENCY);
+        assert_eq!(read_packet(ctx, &mut usb), &input[64..]);
+    }
+
+    #[test]
+    fn an_input_of_no_byte_is_a_zero_length_packet() {
+        let ctx = &mut Ctx::default();
+        let mut usb = UsbSerialJtag::new();
+        usb.receive(ctx, Cycles::ZERO, b"");
+        usb.receive(ctx, Cycles::ZERO, b"z");
+        assert_eq!(usb.host_pending(), 2);
+        run_to(ctx, &mut usb, Cycles(HOST_LATENCY));
+        assert_eq!(
+            usb.read(ctx, INT_RAW).unwrap() & OUT_EP1_ZERO_PAYLOAD,
+            OUT_EP1_ZERO_PAYLOAD
+        );
+        assert_eq!(usb.read(ctx, INT_RAW).unwrap() & SERIAL_OUT_RECV_PKT, 0);
+        run_to(ctx, &mut usb, Cycles(2 * HOST_LATENCY));
+        assert_eq!(read_packet(ctx, &mut usb), b"z");
+    }
+
+    #[test]
+    fn a_read_of_an_empty_out_fifo_returns_0_and_is_refused_by_name() {
+        let ctx = &mut Ctx::default();
+        let mut usb = UsbSerialJtag::new();
+        assert_eq!(usb.read(ctx, EP1), Some(0));
+        assert_eq!(ctx.take_unsupported(), Some(Unsupported::UsbEmptyRead));
+    }
+
+    #[test]
+    fn a_core_reset_empties_the_block_and_the_host_goes_on_sending() {
+        let ctx = &mut Ctx::default();
+        let mut usb = UsbSerialJtag::new();
+        send(ctx, &mut usb, b"sent\n");
+        usb.write(ctx, EP1_CONF, WR_DONE);
+        send(ctx, &mut usb, b"lost");
+        usb.write(ctx, INT_ENA, SERIAL_IN_EMPTY);
+        usb.receive(ctx, Cycles::ZERO, b"in");
+        usb.receive(ctx, Cycles(10 * HOST_LATENCY), b"later");
+        run_to(ctx, &mut usb, Cycles(HOST_LATENCY));
+        usb.reset(ctx);
+        assert_eq!(ctx.sources(), 0, "INT_ENA back to 0");
+        assert_eq!(usb.read(ctx, INT_RAW), Some(SERIAL_IN_EMPTY));
+        assert_eq!(
+            read_packet(ctx, &mut usb),
+            [],
+            "the packet in the FIFO is lost"
+        );
+        assert_eq!(usb.take_tx(ctx), b"sent\n", "what the host took stays");
+        assert_eq!(usb.take_written(), [], "the packet being filled is lost");
+        run_to(ctx, &mut usb, Cycles(11 * HOST_LATENCY));
+        assert_eq!(read_packet(ctx, &mut usb), b"later");
     }
 }
```

- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p soc-esp32c3 --lib -- periph::usb_serial_jtag:: diag:: machine:: > /tmp/t4-red.log 2>&1; echo "exit=$?"; grep -E "^error" /tmp/t4-red.log | sort | uniq -c
```

Expected: `exit=101`; `could not compile soc-esp32c3 (lib test) due to 94 previous errors`: the new constants (`HOST_LATENCY`, `SERIAL_IN_EMPTY`, `INT_ENA`, `SERIAL_OUT_RECV_PKT`, the `reg` helper, `Cycles` not imported by the module), `no method named receive`, and 9 × `error[E0061]: this method takes 0 arguments but 1 argument was supplied` (`take_tx` with the context).

- [ ] **Step 3: Both endpoints and the interrupts**

```diff
--- a/crates/soc-esp32c3/src/ctx.rs
+++ b/crates/soc-esp32c3/src/ctx.rs
@@ -103,8 +103,10 @@
     CacheInvalidated,
     /// Wake-up of the SYSTIMER: one of its comparators may have an alarm due.
     Systimer,
-    /// Wake-up of UART 0 or 1: its transmitter may have gone idle.
+    /// Wake-up of UART 0 or 1: a byte may have left or landed, or an interrupt condition begun.
     Uart(u8),
+    /// Wake-up of USB-Serial-JTAG: a packet of the host may have reached the OUT FIFO.
+    UsbSerialJtag,
     /// Wake-up of a timer group: its timer or its watchdog may have reached a deadline.
     Timg(Group),
     /// Wake-up of `RTC_CNTL`: the RTC watchdog may have reached a stage timeout.
```

```diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -81,6 +81,8 @@
     UartRxOverflow { uart: u8 },
     /// A read of the receive FIFO of a UART while it was empty: 0 was returned (report UR A6).
     UartEmptyRead { uart: u8 },
+    /// A read of `USB_DEVICE.EP1` while the OUT FIFO was empty: 0 was returned (report USJ O.6).
+    UsbEmptyRead,
 }
 
 /// A UART mode the model refuses by name when software sets it (report UR §10, A4).
@@ -269,7 +271,8 @@
             | Unsupported::UartClock { .. }
             | Unsupported::UartFeature { .. }
             | Unsupported::UartRxOverflow { .. }
-            | Unsupported::UartEmptyRead { .. } => self.describe_serial(pc, f),
+            | Unsupported::UartEmptyRead { .. }
+            | Unsupported::UsbEmptyRead => self.describe_serial(pc, f),
         }
     }
 
@@ -298,6 +301,10 @@
             Unsupported::UartEmptyRead { uart } => write!(
                 f,
                 "UART{uart}.FIFO was read while the receive FIFO was empty (pc {pc:#010x}), so 0 was returned; read only the number of bytes UART.STATUS RXFIFO_CNT gives"
+            ),
+            Unsupported::UsbEmptyRead => write!(
+                f,
+                "USB_DEVICE.EP1 was read while the OUT FIFO was empty (pc {pc:#010x}), so 0 was returned; read EP1 only while EP1_CONF.SERIAL_OUT_EP_DATA_AVAIL is set"
             ),
             // INVARIANT: `describe` hands only the refusals above to this function.
             _ => Ok(()),
@@ -404,6 +411,7 @@
                 Unsupported::UartFeature { uart, feature } => (23, u32::from(uart), feature as u32),
                 Unsupported::UartRxOverflow { uart } => (24, u32::from(uart), 0),
                 Unsupported::UartEmptyRead { uart } => (25, u32::from(uart), 0),
+                Unsupported::UsbEmptyRead => (26, 0, 0),
                 // A command is 1 to 16 bits long, so 0 is free for "no command phase".
                 Unsupported::FlashTransfer { command_bits, .. } => {
                     (11, command_bits.unwrap_or(0), 0)
```

```diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -470,7 +470,9 @@
     pub fn take_console(&mut self, channel: ConsoleChannel) -> Vec<u8> {
         match channel {
             ConsoleChannel::Uart0 => self.bus.periph.uart0.take_tx(&mut self.bus.ctx),
-            ConsoleChannel::UsbSerialJtag => self.bus.periph.usb_serial_jtag.take_tx(),
+            ConsoleChannel::UsbSerialJtag => {
+                self.bus.periph.usb_serial_jtag.take_tx(&mut self.bus.ctx)
+            }
             ConsoleChannel::Uart1 => self.bus.periph.uart1.take_tx(&mut self.bus.ctx),
         }
     }
@@ -482,7 +484,7 @@
     pub fn flush_console(&mut self, channel: ConsoleChannel) -> Vec<u8> {
         match channel {
             ConsoleChannel::Uart0 => self.bus.periph.uart0.take_written(),
-            ConsoleChannel::UsbSerialJtag => self.bus.periph.usb_serial_jtag.take_tx(),
+            ConsoleChannel::UsbSerialJtag => self.bus.periph.usb_serial_jtag.take_written(),
             ConsoleChannel::Uart1 => self.bus.periph.uart1.take_written(),
         }
     }
```

```diff
--- a/crates/soc-esp32c3/src/periph/mod.rs
+++ b/crates/soc-esp32c3/src/periph/mod.rs
@@ -503,6 +503,7 @@
             }
             Event::Uart(0) => self.uart0.wake(ctx),
             Event::Uart(_) => self.uart1.wake(ctx),
+            Event::UsbSerialJtag => self.usb_serial_jtag.wake(ctx),
             Event::ModuleReset(module) => self.reset_module(ctx, module),
         }
     }
@@ -585,7 +586,7 @@
         self.radio_power = AcceptedBlock::new(0, &RADIO_POWER_ACCEPTED);
         self.uart0.reset(ctx);
         self.uart1.reset(ctx);
-        self.usb_serial_jtag.reset();
+        self.usb_serial_jtag.reset(ctx);
         // No rebuilt device asserts an interrupt source, and `SYSTEM` selects the reset clock again.
         // The timer groups and the SYSTIMER go last: each withdraws its wake-up, so that none of
         // the replaced devices leaves a call behind, and drives its sources low.
```

```diff
--- a/crates/soc-esp32c3/src/periph/usb_serial_jtag.rs
+++ b/crates/soc-esp32c3/src/periph/usb_serial_jtag.rs
@@ -1,65 +1,322 @@
-//! USB-Serial-JTAG, device-to-host serial path. Register layout: `usb_serial_jtag_reg.h` (report §G.3).
+//! USB-Serial-JTAG, the serial endpoint in both directions. Register layout:
+//! `usb_serial_jtag_reg.h` and `usb_serial_jtag_struct.h` (report USJ §A).
 //!
-//! The model behaves like a board with a host attached and reading: the IN endpoint always has
-//! room. Reception, interrupts and host attach/detach arrive in SP1b-3d.
-use crate::ctx::Ctx;
-use emu_core::Peripheral;
-
-const EP1: u32 = 0x00;
-const EP1_CONF: u32 = 0x04;
-/// `EP1_CONF` bit 0, write 1: commit the bytes written so far as one IN packet. A write to `EP1_CONF`
-/// acts on this bit only: bits 1 and 2 are read-only (report §G.3).
+//! The firmware writes the bytes for the host into the IN FIFO of `EP1` and commits them as a
+//! packet with `EP1_CONF.WR_DONE`, or by filling the 64 bytes of a packet. The host's bytes come
+//! as OUT packets of at most 64 bytes, one in the FIFO at a time, read from `EP1` (report USJ
+//! §R.1, §R.5). The interrupt is source 26 (`USB_INTR_MAP`), level, driven as
+//! `INT_RAW & INT_ENA != 0` after every change (report USJ §C).
+//!
+//! The model behaves like a host attached and reading: a committed packet is taken at once.
+use crate::clock::SECOND;
+use crate::ctx::{Ctx, Deadline, Event, Source};
+use crate::diag::Unsupported;
+use crate::periph::Storage;
+use emu_core::{Cycles, Peripheral};
+use std::collections::VecDeque;
+
+/// The offset of a register of the block.
+const fn reg(name: &str) -> u32 {
+    svd_regs::esp32c3::addr("USB_DEVICE", name) - svd_regs::esp32c3::addr("USB_DEVICE", "EP1")
+}
+
+const EP1: u32 = reg("EP1");
+const EP1_CONF: u32 = reg("EP1_CONF");
+const INT_RAW: u32 = reg("INT_RAW");
+const INT_ST: u32 = reg("INT_ST");
+const INT_ENA: u32 = reg("INT_ENA");
+const INT_CLR: u32 = reg("INT_CLR");
+/// Written by the driver and by Arduino's `HWCDC` (`CONF0`), or by nobody on the paths read
+/// (report USJ §A): stored, from their reset values, with no effect.
+const STORED: [u32; 5] = [
+    reg("CONF0"),
+    reg("TEST"),
+    reg("MISC_CONF"),
+    reg("MEM_CONF"),
+    reg("DATE"),
+];
+/// The state of the JTAG FIFO and of the endpoints, and the frame number: read-only, served at
+/// their reset values.
+///
+/// ASSUMPTION: nobody reads them (report USJ §A: no consumer on the paths read touches them; the
+/// LL header has no accessor for `FRAM_NUM`), so the model does not derive them from its FIFOs.
+/// A firmware that reads them, or a register dump of a chip with a packet in each FIFO,
+/// settles what they hold.
+const AT_RESET: [u32; 9] = [
+    reg("JFIFO_ST"),
+    reg("FRAM_NUM"),
+    reg("IN_EP0_ST"),
+    reg("IN_EP1_ST"),
+    reg("IN_EP2_ST"),
+    reg("IN_EP3_ST"),
+    reg("OUT_EP0_ST"),
+    reg("OUT_EP1_ST"),
+    reg("OUT_EP2_ST"),
+];
+/// `EP1_CONF` bit 0, write 1: commit the bytes written so far as one IN packet, none included (a
+/// zero-length packet). It reads 0. A write to `EP1_CONF` acts on this bit only: bits 1 and 2 are
+/// read-only (`usb_serial_jtag_struct.h:24-26`).
 const WR_DONE: u32 = 1 << 0;
-/// `EP1_CONF` bit 1, read-only: the IN endpoint can take a byte.
+/// `EP1_CONF` bit 1, read-only: the IN endpoint takes a byte (no committed packet waits for the
+/// host, and the FIFO holds fewer than 64 bytes).
 const SERIAL_IN_EP_DATA_FREE: u32 = 1 << 1;
+/// `EP1_CONF` bit 2, read-only: the OUT FIFO holds a byte of the host.
+const SERIAL_OUT_EP_DATA_AVAIL: u32 = 1 << 2;
+/// The bits of `INT_RAW` the model sets (`usb_serial_jtag_reg.h:554-660`, report USJ §R.1):
+/// `SERIAL_OUT_RECV_PKT` (a packet of the host is in the OUT FIFO), `SERIAL_IN_EMPTY` (the host
+/// took an IN packet; 1 at reset), `OUT_EP1_ZERO_PAYLOAD` (the host sent a zero-length packet).
+const SERIAL_OUT_RECV_PKT: u32 = 1 << 2;
+const SERIAL_IN_EMPTY: u32 = 1 << 3;
+const OUT_EP1_ZERO_PAYLOAD: u32 = 1 << 10;
+/// `INT_ENA` and `INT_CLR` take the 19 bits of `USB_SERIAL_JTAG_LL_INTR_MASK` (`ll.h:19`).
+const INTERRUPT_BITS: u32 = 0x7FFFF;
 /// Size of a full-speed bulk packet: the hardware sends a full FIFO without waiting for `WR_DONE`.
 /// Source: ESP-IDF v5.5.2 `components/hal/esp32c3/include/hal/usb_serial_jtag_ll.h`, note on
 /// `usb_serial_jtag_ll_txfifo_flush`: "When fifo is full (with 64 byte), HW will flush the buffer
 /// automatically".
-const PACKET_BYTES: usize = 64;
-
-#[derive(Clone, Debug, Default)]
+pub const PACKET_BYTES: usize = 64;
+/// The interrupt source of the block (report USJ §C).
+const SOURCE: Source = Source::of("USB_INTR_MAP");
+/// The device time the host takes to move a packet: an OUT packet of the host reaches the FIFO
+/// this long after it is sent, or after the previous one has been read whole.
+///
+/// ASSUMPTION: 50 us, about one full-speed bulk transaction of 64 bytes with its token and its
+/// handshake (report USJ §D.2, §O.5). No source gives a figure; a board with a host writing to
+/// it settles it.
+pub const HOST_LATENCY: u64 = 50 * SECOND / 1_000_000;
+
+#[derive(Debug)]
 pub struct UsbSerialJtag {
-    packet: Vec<u8>,
+    stored: Storage,
+    /// The bytes of the IN packet being filled.
+    in_fifo: Vec<u8>,
+    /// The bytes the host took, for the console.
     tx: Vec<u8>,
+    /// The packets the host sends and that have not reached the FIFO, each with the device
+    /// time, in cycles, before which it is not sent; a zero-length packet is empty.
+    host: VecDeque<(u64, Vec<u8>)>,
+    /// The packet of the host in the OUT FIFO, what the firmware has not read of it.
+    out_fifo: VecDeque<u8>,
+    /// When the OUT FIFO was last left empty: the host sends its next packet from then on.
+    out_free_since: u64,
+    int_raw: u32,
+    int_ena: u32,
+    /// The call at the instant the next packet of the host reaches the FIFO.
+    wake: Deadline,
+}
+
+impl Default for UsbSerialJtag {
+    fn default() -> UsbSerialJtag {
+        UsbSerialJtag::new()
+    }
 }
 
 impl UsbSerialJtag {
     #[must_use]
-    pub const fn new() -> UsbSerialJtag {
+    pub fn new() -> UsbSerialJtag {
+        let base = svd_regs::esp32c3::addr("USB_DEVICE", "EP1");
         UsbSerialJtag {
-            packet: Vec::new(),
+            stored: Storage::for_offsets(base, &STORED),
+            in_fifo: Vec::new(),
             tx: Vec::new(),
-        }
-    }
-
-    /// A core reset empties the packet being filled; what was sent stays for the console.
-    pub(crate) fn reset(&mut self) {
-        self.packet.clear();
-    }
-
-    /// The bytes sent to the host since the last call.
-    pub fn take_tx(&mut self) -> Vec<u8> {
+            host: VecDeque::new(),
+            out_fifo: VecDeque::new(),
+            out_free_since: 0,
+            // The SVD's reset value: `SERIAL_IN_EMPTY`.
+            int_raw: SERIAL_IN_EMPTY,
+            int_ena: 0,
+            wake: Deadline::default(),
+        }
+    }
+
+    /// A core reset returns the block to its reset values and empties both FIFOs; what the host
+    /// took stays for the console. The host is no part of the chip: the packets it is sending go
+    /// on arriving.
+    ///
+    /// ASSUMPTION: a core reset resets the block, as it resets "the whole digital system"
+    /// (TRM §6.1.3, report IR §4.2); a CPU reset (`esp_restart` on the ESP32-C3) keeps it, as
+    /// report IR §4.2 lists for the other peripherals, and does not reach this function. The
+    /// reset table of the TRM, or a chip whose USB-Serial-JTAG is read after a watchdog reset,
+    /// settles it.
+    pub(crate) fn reset(&mut self, ctx: &mut Ctx) {
+        self.settle(ctx);
+        let host = std::mem::take(&mut self.host);
+        let tx = std::mem::take(&mut self.tx);
+        *self = UsbSerialJtag {
+            host,
+            tx,
+            out_free_since: ctx.now().get(),
+            ..UsbSerialJtag::new()
+        };
+        self.drive(ctx);
+        self.arm(ctx);
+    }
+
+    /// The bytes the host took since the last call.
+    pub fn take_tx(&mut self, ctx: &mut Ctx) -> Vec<u8> {
+        self.settle(ctx);
         std::mem::take(&mut self.tx)
     }
 
+    /// Every byte the firmware wrote and no call has taken, the packet being filled included:
+    /// for the end of a run, so that the console shows what the firmware had written when it
+    /// stopped.
+    pub fn take_written(&mut self) -> Vec<u8> {
+        let mut written = std::mem::take(&mut self.tx);
+        written.append(&mut self.in_fifo);
+        written
+    }
+
+    /// The host sends `bytes` from device time `at` (now or later), after what it sent before:
+    /// packets of at most 64 bytes; no byte at all is one zero-length packet. The caller bounds
+    /// how many it hands over ([`UsbSerialJtag::host_pending`]).
+    pub fn receive(&mut self, ctx: &mut Ctx, at: Cycles, bytes: &[u8]) {
+        self.settle(ctx);
+        let date = at.max(ctx.now()).get();
+        if bytes.is_empty() {
+            self.host.push_back((date, Vec::new()));
+        }
+        for packet in bytes.chunks(PACKET_BYTES) {
+            self.host.push_back((date, packet.to_vec()));
+        }
+        self.arm(ctx);
+    }
+
+    /// How many bytes the host has sent that the firmware has not read, its packets counted as
+    /// one byte each so that zero-length ones count too.
+    #[must_use]
+    pub fn host_pending(&self) -> usize {
+        self.out_fifo.len()
+            + self
+                .host
+                .iter()
+                .map(|(_, packet)| packet.len().max(1))
+                .sum::<usize>()
+    }
+
+    /// The device time of a wake-up has come.
+    pub(crate) fn wake(&mut self, ctx: &mut Ctx) {
+        self.wake.called(ctx.now());
+        self.settle(ctx);
+        self.arm(ctx);
+    }
+
+    /// The instant the next packet of the host reaches the FIFO: once the FIFO is empty, the
+    /// host latency after the packet is sent or the FIFO was left empty, whichever is later.
+    fn next_packet_at(&self) -> Option<u64> {
+        if !self.out_fifo.is_empty() {
+            return None;
+        }
+        let &(date, _) = self.host.front()?;
+        Some(date.max(self.out_free_since).saturating_add(HOST_LATENCY))
+    }
+
+    fn arm(&mut self, ctx: &mut Ctx) {
+        if let Some(at) = self.next_packet_at() {
+            self.wake.arm(ctx, Cycles(at), Event::UsbSerialJtag);
+        }
+    }
+
+    /// Brings the block to now: the packets of the host that have arrived enter the FIFO, one at
+    /// a time; the interrupt source follows.
+    fn settle(&mut self, ctx: &mut Ctx) {
+        let now = ctx.now().get();
+        // INVARIANT: each pass takes one packet of those the caller handed over within its bound.
+        while let Some(at) = self.next_packet_at().filter(|&at| at <= now) {
+            let Some((_, packet)) = self.host.pop_front() else {
+                break;
+            };
+            if packet.is_empty() {
+                // A zero-length packet leaves the FIFO empty: the next one can follow.
+                self.int_raw |= OUT_EP1_ZERO_PAYLOAD;
+                self.out_free_since = at;
+            } else {
+                self.out_fifo.extend(packet);
+                self.int_raw |= SERIAL_OUT_RECV_PKT;
+            }
+        }
+        self.drive(ctx);
+    }
+
+    /// The bytes written so far leave as one packet: the host takes it at once, and
+    /// `SERIAL_IN_EMPTY` says so.
     fn commit(&mut self) {
-        self.tx.append(&mut self.packet);
+        self.tx.append(&mut self.in_fifo);
+        self.int_raw |= SERIAL_IN_EMPTY;
+    }
+
+    /// A read of `EP1`: the oldest byte of the host.
+    ///
+    /// ASSUMPTION: a read of an empty OUT FIFO returns 0 and changes nothing (report USJ §R.1,
+    /// O.6); it is refused by name. Every consumer read looks at `SERIAL_OUT_EP_DATA_AVAIL`
+    /// first; the TRM settles it.
+    fn pop(&mut self, ctx: &mut Ctx) -> u32 {
+        let Some(byte) = self.out_fifo.pop_front() else {
+            ctx.unsupported(Unsupported::UsbEmptyRead);
+            return 0;
+        };
+        if self.out_fifo.is_empty() {
+            self.out_free_since = ctx.now().get();
+        }
+        u32::from(byte)
+    }
+
+    /// A write of `EP1`: a byte for the host. The FIFO never holds 64 bytes: the 64th commits.
+    fn push(&mut self, byte: u8) {
+        self.in_fifo.push(byte);
+        if self.in_fifo.len() == PACKET_BYTES {
+            self.commit();
+        }
+    }
+
+    fn ep1_conf(&self) -> u32 {
+        let free = if self.in_fifo.len() < PACKET_BYTES {
+            SERIAL_IN_EP_DATA_FREE
+        } else {
+            0
+        };
+        let avail = if self.out_fifo.is_empty() {
+            0
+        } else {
+            SERIAL_OUT_EP_DATA_AVAIL
+        };
+        free | avail
+    }
+
+    fn int_status(&self) -> u32 {
+        self.int_raw & self.int_ena
+    }
+
+    fn drive(&self, ctx: &mut Ctx) {
+        ctx.irq(SOURCE, self.int_status() != 0);
     }
 }
 
 impl Peripheral<Ctx> for UsbSerialJtag {
-    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
-        (offset == EP1_CONF).then_some(SERIAL_IN_EP_DATA_FREE)
-    }
-
-    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
-        match offset {
+    fn read(&mut self, ctx: &mut Ctx, offset: u32) -> Option<u32> {
+        self.settle(ctx);
+        let value = match offset {
+            EP1 => self.pop(ctx),
+            EP1_CONF => self.ep1_conf(),
+            INT_RAW => self.int_raw,
+            INT_ST => self.int_status(),
+            INT_ENA => self.int_ena,
+            _ if AT_RESET.contains(&offset) => {
+                svd_regs::esp32c3::lookup(svd_regs::esp32c3::addr("USB_DEVICE", "EP1") + offset)
+                    .map_or(0, |register| register.reset_value)
+            }
+            _ => return self.stored.read(offset),
+        };
+        self.drive(ctx);
+        self.arm(ctx);
+        Some(value)
+    }
+
+    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
+        self.settle(ctx);
+        let accepted = match offset {
             EP1 => {
-                self.packet.push(value.to_le_bytes()[0]);
-                if self.packet.len() == PACKET_BYTES {
-                    self.commit();
-                }
+                self.push(value.to_le_bytes()[0]);
                 true
             }
             EP1_CONF => {
@@ -68,8 +325,18 @@
                 }
                 true
             }
-            _ => false,
-        }
+            INT_ENA => {
+                self.int_ena = value & INTERRUPT_BITS;
+                true
+            }
+            INT_CLR => {
+                self.int_raw &= !value;
+                true
+            }
+            _ => self.stored.write(offset, value),
+        };
+        self.drive(ctx);
+        accepted
     }
 }
 
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p soc-esp32c3 --lib -- periph::usb_serial_jtag:: diag:: machine:: > /tmp/t4.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t4.log
```

Expected: `exit=0` and `test result: ok. 101 passed; 0 failed; 0 ignored; 0 measured; 448 filtered out`.

- [ ] **Step 5: Mutations**

| # | Mutation (`crates/soc-esp32c3/src/periph/usb_serial_jtag.rs`) | Test that fails |
|---|---|---|
| 1 | `Some(date.max(self.out_free_since).saturating_add(HOST_LATENCY))` → `Some(date.saturating_add(HOST_LATENCY))` | `a_long_input_arrives_in_packets_of_64_each_after_the_previous_one_was_read`, `an_input_of_no_byte_is_a_zero_length_packet` |
| 2 | delete `if !self.out_fifo.is_empty() { return None; }` | `a_long_input_arrives_in_packets_of_64_each_after_the_previous_one_was_read` |
| 3 | delete `self.int_raw \|= SERIAL_OUT_RECV_PKT;` | `a_packet_of_the_host_reaches_the_fifo_after_the_host_latency_and_raises_its_interrupt` |
| 4 | delete the last `self.int_raw \|= SERIAL_IN_EMPTY;` of the commit | `a_commit_of_nothing_is_a_zero_length_packet_the_host_takes`, `bytes_reach_the_host_when_the_packet_is_committed` |
| 5 | `self.int_ena = value & INTERRUPT_BITS;` → `self.int_ena = value;` | `hwcdc_begin_takes_its_first_interrupt_from_the_reset_value_of_serial_in_empty` |
| 6 | `let host = std::mem::take(&mut self.host);` → `let host = VecDeque::new();` | `a_core_reset_empties_the_block_and_the_host_goes_on_sending` |

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy -p soc-esp32c3 --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git commit -m "feat(soc-esp32c3): USB-Serial-JTAG receives the host's packets on EP1 and raises its interrupts on source 26" -- crates/soc-esp32c3/src
```

Expected: three `exit=0`, then the commit.

---

## Task 5: The USB host is absent, attached or open

The host model (USJ §D, C5, C17–C20). `UsbHost::{Absent, Attached, Open}`, the state at power-on in `SocConfig::usb_host` (default `Open`, attached at device time 0 without a bus reset), and dated changes through `UsbSerialJtag::set_host`, applied in the order of their dates. An attached or open host starts a frame every millisecond (`FRAME`): SOF (bit 1) and `FRAM_NUM` are computed from device time, never one event per frame; an event is scheduled at the next frame start only while `INT_ENA.SOF` is set, which the connection monitor of ESP-IDF needs at 100 Hz and at 1 kHz. An attach from `Absent` raises `USB_BUS_RESET` and starts the frames at its instant; a detach stops them. Only an open host takes a committed IN packet, `HOST_LATENCY` after its commit (or after it opened); its bytes then reach the console, `SERIAL_IN_EP_DATA_FREE` comes back and `SERIAL_IN_EMPTY` is raised. A byte written while a packet waits is ignored and refused by name (`UsbInFull`), `WR_DONE` then does nothing (C18), and host packets wait while the host is not open. At one instant, a change of host comes first, then a drain, then an OUT packet (C19).

The ROM mirrors its console to USB-Serial-JTAG: it now waits for the host to take each packet (with an open host, 50 us), so the pinned trace hashes, device times and instruction counts of the reference runs change (Step 7).

**Files:**
- Modify: `crates/soc-esp32c3/src/periph/usb_serial_jtag.rs`, `crates/soc-esp32c3/src/config.rs`, `crates/soc-esp32c3/src/diag.rs`, `crates/soc-esp32c3/src/lib.rs`, `crates/soc-esp32c3/src/machine.rs`, `crates/soc-esp32c3/src/periph/mod.rs`; Step 7: `crates/emu-cli/tests/run_cli.rs` and, if the `rom-boot` job says so, `crates/soc-esp32c3/tests/real_rom.rs`

**Interfaces:**
- Consumes: Task 4.
- Produces: `pub enum UsbHost { Absent, Attached, #[default] Open }` with `pub const fn name(self) -> &'static str` (`"absent"`, `"attached"`, `"open"`) and `pub const ALL: [UsbHost; 3]`; `pub const FRAME: u64` (1 ms); `pub fn UsbSerialJtag::new(host: UsbHost) -> UsbSerialJtag` (it took nothing in Task 4); `pub fn set_host(&mut self, ctx: &mut Ctx, at: Cycles, state: UsbHost)`; `pub fn host(&mut self, ctx: &mut Ctx) -> UsbHost`; `pub fn host_changes_pending(&self) -> usize`; `SocConfig::usb_host: UsbHost`; `soc_esp32c3::UsbHost`; `Unsupported::UsbInFull` (key 27).

**Bounds on untrusted input.** The changes of host queued are bounded by the machine (Task 6, `MAX_PENDING_HOST_CHANGES`); frames are counted, not iterated (`frames_by` is a division).

- [ ] **Step 1: Write the failing tests**

```diff
--- a/crates/soc-esp32c3/src/config.rs
+++ b/crates/soc-esp32c3/src/config.rs
@@ -52,5 +52,6 @@
         assert_eq!(config.flash, crate::flash::FlashConfig::default());
         assert_eq!(config.rng_seed, DEFAULT_RNG_SEED);
         assert_ne!(DEFAULT_RNG_SEED, 0);
+        assert_eq!(config.usb_host, UsbHost::Open, "report USJ §R.2");
     }
 }
```

```diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -1322,6 +1322,10 @@
                 Unsupported::UsbEmptyRead,
                 "USB_DEVICE.EP1 was read while the OUT FIFO was empty (pc 0x40381000)",
             ),
+            (
+                Unsupported::UsbInFull,
+                "a byte was written to USB_DEVICE.EP1 while the IN endpoint had no room (pc 0x40381000)",
+            ),
         ];
         for (what, needle) in texts {
             assert_cause_then_remedy(&unsupported(what), needle);
```

```diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -790,7 +790,11 @@
         let mut program = print(USB, b"ok");
         program.extend([addi(T1, 0, 1), sw(T1, T0, 4), HANG]);
         let mut m = machine(&program);
+        // The host takes the packet 50 us after `WR_DONE`: 1 000 cycles at the 20 MHz of
+        // power-on.
         m.run_until(Cycles(500));
+        assert_eq!(m.take_console(ConsoleChannel::UsbSerialJtag), b"");
+        m.run_until(Cycles(1_500));
         assert_eq!(m.take_console(ConsoleChannel::UsbSerialJtag), b"ok");
         assert_eq!(
             m.take_console(ConsoleChannel::Uart0),
```

```diff
--- a/crates/soc-esp32c3/src/periph/usb_serial_jtag.rs
+++ b/crates/soc-esp32c3/src/periph/usb_serial_jtag.rs
@@ -363,7 +363,8 @@
         bytes
     }
 
-    /// Moves device time to `instant`, delivering each call the block asked for on the way.
+    /// Moves device time to `instant`, delivering each call the block asked for on the way. A
+    /// block that asks for a call at the instant it is called again and again fails the test.
     fn run_to(ctx: &mut Ctx, usb: &mut UsbSerialJtag, instant: Cycles) {
         let mut calls = 0;
         while let Some(next) = ctx.next_wake().filter(|&next| next <= instant) {
@@ -380,11 +381,15 @@
         ctx.sleep_until(instant);
     }
 
+    fn open() -> UsbSerialJtag {
+        UsbSerialJtag::new(UsbHost::Open)
+    }
+
     #[test]
     fn the_registers_start_at_their_svd_reset_values() {
         // Report USJ §A.
         let ctx = &mut Ctx::default();
-        let mut usb = UsbSerialJtag::new();
+        let mut usb = UsbSerialJtag::new(UsbHost::Absent);
         for (name, reset) in [
             ("EP1_CONF", 0x2),
             ("INT_RAW", 0x8),
@@ -406,16 +411,16 @@
         assert!(usb.write(ctx, reg("CONF0"), 0x4204));
         assert_eq!(usb.read(ctx, reg("CONF0")), Some(0x4204), "stored");
         assert!(!usb.write(ctx, reg("JFIFO_ST"), 0), "read-only");
-        assert!(!usb.write(ctx, reg("IN_EP1_ST"), 0), "read-only");
+        assert!(!usb.write(ctx, reg("FRAM_NUM"), 0), "read-only");
         assert_eq!(usb.read(ctx, 0x7C), None, "no register there");
         assert_eq!(ctx.sources(), 0);
     }
 
     #[test]
-    fn bytes_reach_the_host_when_the_packet_is_committed() {
+    fn an_open_host_takes_a_committed_packet_after_the_host_latency() {
         // The ROM: read EP1_CONF bit 1, write the byte, WR_DONE after `\n` (report USJ §B.1).
         let ctx = &mut Ctx::default();
-        let mut usb = UsbSerialJtag::new();
+        let mut usb = open();
         usb.write(ctx, INT_CLR, SERIAL_IN_EMPTY);
         for &byte in b"boot\n" {
             assert_eq!(usb.read(ctx, EP1_CONF).unwrap() & SERIAL_IN_EP_DATA_FREE, 2);
@@ -423,25 +428,30 @@
         }
         assert_eq!(usb.take_tx(ctx), [], "nothing is sent before WR_DONE");
         assert!(usb.write(ctx, EP1_CONF, WR_DONE));
+        assert_eq!(
+            usb.read(ctx, EP1_CONF),
+            Some(0),
+            "no room until the host takes it"
+        );
+        assert_eq!(ctx.next_wake(), Some(Cycles(HOST_LATENCY)));
+        run_to(ctx, &mut usb, Cycles(HOST_LATENCY - 1));
+        assert_eq!(usb.take_tx(ctx), []);
+        run_to(ctx, &mut usb, Cycles(HOST_LATENCY));
         assert_eq!(usb.take_tx(ctx), b"boot\n");
-        assert_eq!(usb.take_tx(ctx), []);
-        assert_eq!(
-            usb.read(ctx, INT_RAW),
-            Some(SERIAL_IN_EMPTY),
-            "the host took the packet"
-        );
-        assert_eq!(
-            usb.read(ctx, EP1_CONF),
-            Some(SERIAL_IN_EP_DATA_FREE),
-            "WR_DONE reads 0"
-        );
-    }
-
-    #[test]
-    fn a_full_packet_is_sent_without_waiting_for_a_commit() {
-        let ctx = &mut Ctx::default();
-        let mut usb = UsbSerialJtag::new();
+        assert_eq!(
+            usb.read(ctx, INT_RAW).unwrap() & SERIAL_IN_EMPTY,
+            SERIAL_IN_EMPTY
+        );
+        assert_eq!(usb.read(ctx, EP1_CONF), Some(SERIAL_IN_EP_DATA_FREE));
+    }
+
+    #[test]
+    fn a_full_packet_is_committed_without_waiting_for_wr_done() {
+        let ctx = &mut Ctx::default();
+        let mut usb = open();
         send(ctx, &mut usb, &[b'x'; 64]);
+        assert_eq!(usb.read(ctx, EP1_CONF), Some(0));
+        run_to(ctx, &mut usb, Cycles(HOST_LATENCY));
         assert_eq!(usb.take_tx(ctx).len(), 64);
         send(ctx, &mut usb, b"y");
         assert_eq!(usb.take_tx(ctx), []);
@@ -449,14 +459,36 @@
     }
 
     #[test]
+    fn a_byte_written_while_a_packet_waits_for_the_host_is_refused_and_lost() {
+        let ctx = &mut Ctx::default();
+        let mut usb = open();
+        send(ctx, &mut usb, b"a");
+        usb.write(ctx, EP1_CONF, WR_DONE);
+        assert!(usb.write(ctx, EP1, u32::from(b'b')));
+        assert_eq!(ctx.take_unsupported(), Some(Unsupported::UsbInFull));
+        usb.write(ctx, EP1_CONF, WR_DONE);
+        run_to(ctx, &mut usb, Cycles(10 * HOST_LATENCY));
+        assert_eq!(usb.take_tx(ctx), b"a");
+        assert_eq!(
+            usb.read(ctx, EP1_CONF),
+            Some(SERIAL_IN_EP_DATA_FREE),
+            "the second WR_DONE did nothing"
+        );
+    }
+
+    #[test]
     fn a_commit_of_nothing_is_a_zero_length_packet_the_host_takes() {
         // The driver ends a transfer of 64 bytes with one (report USJ §B.4).
         let ctx = &mut Ctx::default();
-        let mut usb = UsbSerialJtag::new();
+        let mut usb = open();
         usb.write(ctx, INT_CLR, SERIAL_IN_EMPTY);
         assert!(usb.write(ctx, EP1_CONF, WR_DONE));
+        run_to(ctx, &mut usb, Cycles(HOST_LATENCY));
         assert_eq!(usb.take_tx(ctx), []);
-        assert_eq!(usb.read(ctx, INT_RAW), Some(SERIAL_IN_EMPTY));
+        assert_eq!(
+            usb.read(ctx, INT_RAW).unwrap() & SERIAL_IN_EMPTY,
+            SERIAL_IN_EMPTY
+        );
     }
 
     #[test]
@@ -464,7 +496,7 @@
         // Report USJ §B.5: `begin` clears the 19 enables, sets bits 2, 3 and 9, and never writes
         // INT_CLR: the reset value of SERIAL_IN_EMPTY runs the handler at once.
         let ctx = &mut Ctx::default();
-        let mut usb = UsbSerialJtag::new();
+        let mut usb = open();
         usb.write(ctx, reg("CONF0"), 0x4200);
         let ena = usb.read(ctx, INT_ENA).unwrap();
         usb.write(ctx, INT_ENA, ena & !INTERRUPT_BITS);
@@ -472,7 +504,7 @@
         usb.write(
             ctx,
             INT_ENA,
-            ena | SERIAL_OUT_RECV_PKT | SERIAL_IN_EMPTY | (1 << 9),
+            ena | SERIAL_OUT_RECV_PKT | SERIAL_IN_EMPTY | USB_BUS_RESET,
         );
         assert_eq!(usb.read(ctx, INT_ST), Some(SERIAL_IN_EMPTY));
         assert_eq!(ctx.sources(), USB_SOURCE, "source 26, USB_INTR_MAP");
@@ -488,16 +520,16 @@
         // The driver at install enables SERIAL_OUT_RECV_PKT only; its handler clears it, then
         // reads while SERIAL_OUT_EP_DATA_AVAIL (report USJ §B.4, §F.1).
         let ctx = &mut Ctx::default();
-        let mut usb = UsbSerialJtag::new();
+        let mut usb = open();
         usb.write(ctx, INT_ENA, SERIAL_OUT_RECV_PKT);
         usb.receive(ctx, Cycles(1_000), b"hi, espressif\n");
-        assert_eq!(ctx.next_wake(), Some(Cycles(1_000 + HOST_LATENCY)));
         run_to(ctx, &mut usb, Cycles(1_000 + HOST_LATENCY - 1));
         assert_eq!(
             usb.read(ctx, EP1_CONF).unwrap() & SERIAL_OUT_EP_DATA_AVAIL,
             0
         );
         assert_eq!(ctx.sources(), 0);
+        assert_eq!(ctx.next_wake(), Some(Cycles(1_000 + HOST_LATENCY)));
         run_to(ctx, &mut usb, Cycles(1_000 + HOST_LATENCY));
         assert_eq!(ctx.sources(), USB_SOURCE, "raised at its instant");
         assert_eq!(usb.read(ctx, INT_ST), Some(SERIAL_OUT_RECV_PKT));
@@ -512,13 +544,12 @@
     fn a_long_input_arrives_in_packets_of_64_each_after_the_previous_one_was_read() {
         // Report USJ §F.1: a 90-byte input is read as 64 + 26.
         let ctx = &mut Ctx::default();
-        let mut usb = UsbSerialJtag::new();
+        let mut usb = open();
         let input: Vec<u8> = (0..90).collect();
         usb.receive(ctx, Cycles::ZERO, &input);
         assert_eq!(usb.host_pending(), 90);
         run_to(ctx, &mut usb, Cycles(HOST_LATENCY));
-        let first = read_packet(ctx, &mut usb);
-        assert_eq!(first, &input[..64]);
+        assert_eq!(read_packet(ctx, &mut usb), &input[..64]);
         let read_at = ctx.now();
         run_to(ctx, &mut usb, read_at + (HOST_LATENCY - 1));
         assert_eq!(
@@ -533,7 +564,7 @@
     #[test]
     fn an_input_of_no_byte_is_a_zero_length_packet() {
         let ctx = &mut Ctx::default();
-        let mut usb = UsbSerialJtag::new();
+        let mut usb = open();
         usb.receive(ctx, Cycles::ZERO, b"");
         usb.receive(ctx, Cycles::ZERO, b"z");
         assert_eq!(usb.host_pending(), 2);
@@ -550,25 +581,25 @@
     #[test]
     fn a_read_of_an_empty_out_fifo_returns_0_and_is_refused_by_name() {
         let ctx = &mut Ctx::default();
-        let mut usb = UsbSerialJtag::new();
+        let mut usb = open();
         assert_eq!(usb.read(ctx, EP1), Some(0));
         assert_eq!(ctx.take_unsupported(), Some(Unsupported::UsbEmptyRead));
     }
 
     #[test]
-    fn a_core_reset_empties_the_block_and_the_host_goes_on_sending() {
-        let ctx = &mut Ctx::default();
-        let mut usb = UsbSerialJtag::new();
+    fn a_core_reset_empties_the_block_and_the_host_goes_on() {
+        let ctx = &mut Ctx::default();
+        let mut usb = open();
         send(ctx, &mut usb, b"sent\n");
         usb.write(ctx, EP1_CONF, WR_DONE);
-        send(ctx, &mut usb, b"lost");
         usb.write(ctx, INT_ENA, SERIAL_IN_EMPTY);
         usb.receive(ctx, Cycles::ZERO, b"in");
         usb.receive(ctx, Cycles(10 * HOST_LATENCY), b"later");
         run_to(ctx, &mut usb, Cycles(HOST_LATENCY));
+        send(ctx, &mut usb, b"lost");
         usb.reset(ctx);
         assert_eq!(ctx.sources(), 0, "INT_ENA back to 0");
-        assert_eq!(usb.read(ctx, INT_RAW), Some(SERIAL_IN_EMPTY));
+        assert_eq!(usb.read(ctx, INT_RAW).unwrap() & !SOF, SERIAL_IN_EMPTY);
         assert_eq!(
             read_packet(ctx, &mut usb),
             [],
@@ -578,5 +609,167 @@
         assert_eq!(usb.take_written(), [], "the packet being filled is lost");
         run_to(ctx, &mut usb, Cycles(11 * HOST_LATENCY));
         assert_eq!(read_packet(ctx, &mut usb), b"later");
+        assert_eq!(usb.host(ctx), UsbHost::Open);
+    }
+
+    #[test]
+    fn an_attached_host_starts_a_frame_every_millisecond_and_the_connection_monitor_sees_each() {
+        // Report USJ §B.3: the tick hook reads INT_RAW & SOF and writes INT_CLR = SOF, at 100 Hz
+        // in the reference build; it must see a SOF at every tick, or it declares the host gone.
+        let ctx = &mut Ctx::default();
+        let mut usb = UsbSerialJtag::new(UsbHost::Attached);
+        for tick in 1..=50 {
+            ctx.sleep_until(Cycles(tick * 10 * FRAME));
+            let sof = usb.read(ctx, INT_RAW).unwrap() & SOF;
+            assert_eq!(sof, SOF, "tick {tick}");
+            usb.write(ctx, INT_CLR, SOF);
+            assert_eq!(
+                usb.read(ctx, INT_RAW).unwrap() & SOF,
+                0,
+                "cleared until the next frame"
+            );
+        }
+        assert_eq!(
+            ctx.next_wake(),
+            None,
+            "no call per frame while SOF is not enabled"
+        );
+        // A 1 kHz tick too, a little after each frame start.
+        for tick in 501..=520 {
+            ctx.sleep_until(Cycles(tick * FRAME + 7));
+            assert_eq!(usb.read(ctx, INT_RAW).unwrap() & SOF, SOF, "tick {tick}");
+            usb.write(ctx, INT_CLR, SOF);
+        }
+        assert_eq!(
+            usb.read(ctx, FRAM_NUM),
+            Some(520),
+            "the frame of the last SOF"
+        );
+        ctx.sleep_until(Cycles(2048 * FRAME + 3 * FRAME));
+        assert_eq!(usb.read(ctx, FRAM_NUM), Some(3), "11 bits");
+    }
+
+    #[test]
+    fn an_enabled_sof_asks_for_a_call_at_each_frame_start() {
+        let ctx = &mut Ctx::default();
+        let mut usb = UsbSerialJtag::new(UsbHost::Attached);
+        usb.write(ctx, INT_CLR, SOF);
+        usb.write(ctx, INT_ENA, SOF);
+        assert_eq!(ctx.sources(), 0);
+        assert_eq!(ctx.next_wake(), Some(Cycles(FRAME)));
+        ctx.sleep_until(Cycles(FRAME));
+        assert!(ctx.take_due().is_some());
+        usb.wake(ctx);
+        assert_eq!(ctx.sources(), USB_SOURCE);
+    }
+
+    #[test]
+    fn an_absent_host_starts_no_frame_and_takes_nothing() {
+        // Report USJ §B.1, §F.3: the ROM's first packet stays, and the endpoint has no room after.
+        let ctx = &mut Ctx::default();
+        let mut usb = UsbSerialJtag::new(UsbHost::Absent);
+        send(ctx, &mut usb, b"ESP-ROM\n");
+        usb.write(ctx, EP1_CONF, WR_DONE);
+        usb.receive(ctx, Cycles::ZERO, b"x");
+        run_to(ctx, &mut usb, Cycles(1_000 * FRAME));
+        assert_eq!(usb.read(ctx, INT_RAW).unwrap() & SOF, 0);
+        assert_eq!(usb.take_tx(ctx), []);
+        assert_eq!(
+            usb.read(ctx, EP1_CONF),
+            Some(0),
+            "no room, nothing received"
+        );
+        assert_eq!(usb.take_written(), b"ESP-ROM\n");
+    }
+
+    #[test]
+    fn a_host_with_its_port_closed_starts_frames_but_takes_and_sends_nothing() {
+        let ctx = &mut Ctx::default();
+        let mut usb = UsbSerialJtag::new(UsbHost::Attached);
+        send(ctx, &mut usb, b"line\n");
+        usb.write(ctx, EP1_CONF, WR_DONE);
+        usb.receive(ctx, Cycles::ZERO, b"in");
+        run_to(ctx, &mut usb, Cycles(100 * FRAME));
+        assert_eq!(usb.read(ctx, INT_RAW).unwrap() & SOF, SOF);
+        assert_eq!(usb.take_tx(ctx), []);
+        assert_eq!(read_packet(ctx, &mut usb), []);
+        // The port opens: the packet waiting since its commit leaves the host latency after,
+        // and the input comes as long after.
+        let opened = Cycles(200 * FRAME);
+        usb.set_host(ctx, opened, UsbHost::Open);
+        run_to(ctx, &mut usb, opened + (HOST_LATENCY - 1));
+        assert_eq!(usb.take_tx(ctx), []);
+        assert_eq!(read_packet(ctx, &mut usb), []);
+        run_to(ctx, &mut usb, opened + HOST_LATENCY);
+        assert_eq!(usb.take_tx(ctx), b"line\n");
+        assert_eq!(read_packet(ctx, &mut usb), b"in");
+        // The port closes: a packet committed then waits again.
+        let closed = ctx.now() + 1;
+        usb.set_host(ctx, closed, UsbHost::Attached);
+        run_to(ctx, &mut usb, closed);
+        send(ctx, &mut usb, b"again\n");
+        usb.write(ctx, EP1_CONF, WR_DONE);
+        run_to(ctx, &mut usb, closed + 10 * HOST_LATENCY);
+        assert_eq!(usb.take_tx(ctx), []);
+    }
+
+    #[test]
+    fn an_attach_raises_usb_bus_reset_and_starts_the_frames_from_its_instant_and_a_detach_raises_nothing()
+     {
+        let ctx = &mut Ctx::default();
+        let mut usb = UsbSerialJtag::new(UsbHost::Absent);
+        let attach = Cycles(5 * FRAME + 123);
+        usb.set_host(ctx, attach, UsbHost::Open);
+        assert_eq!(ctx.next_wake(), Some(attach));
+        run_to(ctx, &mut usb, attach);
+        assert_eq!(usb.host(ctx), UsbHost::Open);
+        assert_eq!(
+            usb.read(ctx, INT_RAW),
+            Some(SERIAL_IN_EMPTY | SOF | USB_BUS_RESET)
+        );
+        assert_eq!(usb.read(ctx, FRAM_NUM), Some(0));
+        usb.write(ctx, INT_CLR, INTERRUPT_BITS);
+        ctx.sleep_until(attach + (FRAME - 1));
+        assert_eq!(
+            usb.read(ctx, INT_RAW),
+            Some(0),
+            "frame 1 starts a millisecond later"
+        );
+        ctx.sleep_until(attach + FRAME);
+        assert_eq!(usb.read(ctx, INT_RAW), Some(SOF));
+        let detach = attach + 3 * FRAME;
+        usb.set_host(ctx, detach, UsbHost::Absent);
+        usb.write(ctx, INT_CLR, INTERRUPT_BITS);
+        run_to(ctx, &mut usb, detach + 10 * FRAME);
+        assert_eq!(
+            usb.read(ctx, INT_RAW),
+            Some(SOF),
+            "the frame of the detach instant, no more"
+        );
+        assert_eq!(usb.read(ctx, FRAM_NUM), Some(3));
+        usb.write(ctx, INT_CLR, INTERRUPT_BITS);
+        ctx.sleep_until(detach + 20 * FRAME);
+        assert_eq!(usb.read(ctx, INT_RAW), Some(0));
+    }
+
+    #[test]
+    fn changes_of_the_host_given_out_of_order_apply_in_the_order_of_their_dates() {
+        let ctx = &mut Ctx::default();
+        let mut usb = UsbSerialJtag::new(UsbHost::Absent);
+        usb.set_host(ctx, Cycles(2 * FRAME), UsbHost::Absent);
+        usb.set_host(ctx, Cycles(FRAME), UsbHost::Attached);
+        assert_eq!(usb.host_changes_pending(), 2);
+        run_to(ctx, &mut usb, Cycles(FRAME));
+        assert_eq!(usb.host(ctx), UsbHost::Attached);
+        run_to(ctx, &mut usb, Cycles(2 * FRAME));
+        assert_eq!(usb.host(ctx), UsbHost::Absent);
+        assert_eq!(usb.host_changes_pending(), 0);
+    }
+
+    #[test]
+    fn the_host_names_its_states_as_the_command_line_does() {
+        let names: Vec<&str> = UsbHost::ALL.iter().map(|host| host.name()).collect();
+        assert_eq!(names, ["absent", "attached", "open"]);
+        assert_eq!(UsbHost::default(), UsbHost::Open, "report USJ §R.2");
     }
 }
```

- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p soc-esp32c3 --lib -- periph::usb_serial_jtag:: config:: machine:: > /tmp/t5-red.log 2>&1; echo "exit=$?"; grep -E "^error" /tmp/t5-red.log | sort | uniq -c
```

Expected: `exit=101`; `could not compile soc-esp32c3 (lib test) due to 86 previous errors`: `error[E0433]: cannot find type UsbHost`, the constants `FRAME`, `SOF`, `FRAM_NUM`, `no method named set_host` and `host`, `UsbInFull`, and 8 × `error[E0061]: this function takes 0 arguments but 1 argument was supplied` (`UsbSerialJtag::new(host)`).

- [ ] **Step 3: The host model**

```diff
--- a/crates/soc-esp32c3/src/config.rs
+++ b/crates/soc-esp32c3/src/config.rs
@@ -1,6 +1,7 @@
 //! Run-time configuration of the virtual `SoC`.
 use crate::chip::ChipRevision;
 use crate::flash::FlashConfig;
+use crate::periph::UsbHost;
 
 /// Seed of the hardware random number generator model (decision D8): fixed, so two runs are identical.
 /// The bytes spell "inkemu"; any non-zero value would do.
@@ -24,6 +25,9 @@
     pub flash: FlashConfig,
     /// Seed of the random number generator (`APB_CTRL.RND_DATA`).
     pub rng_seed: u64,
+    /// The USB host on the USB-Serial-JTAG port when the chip starts (ADR 0006): an input of
+    /// the run, open by default.
+    pub usb_host: UsbHost,
 }
 
 impl Default for SocConfig {
@@ -35,6 +39,7 @@
             trace_hash: false,
             flash: FlashConfig::default(),
             rng_seed: DEFAULT_RNG_SEED,
+            usb_host: UsbHost::Open,
         }
     }
 }
```

```diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -83,6 +83,9 @@
     UartEmptyRead { uart: u8 },
     /// A read of `USB_DEVICE.EP1` while the OUT FIFO was empty: 0 was returned (report USJ O.6).
     UsbEmptyRead,
+    /// A write of `USB_DEVICE.EP1` while the IN endpoint had no room: the byte was ignored
+    /// (report USJ O.6).
+    UsbInFull,
 }
 
 /// A UART mode the model refuses by name when software sets it (report UR §10, A4).
@@ -272,7 +275,8 @@
             | Unsupported::UartFeature { .. }
             | Unsupported::UartRxOverflow { .. }
             | Unsupported::UartEmptyRead { .. }
-            | Unsupported::UsbEmptyRead => self.describe_serial(pc, f),
+            | Unsupported::UsbEmptyRead
+            | Unsupported::UsbInFull => self.describe_serial(pc, f),
         }
     }
 
@@ -305,6 +309,10 @@
             Unsupported::UsbEmptyRead => write!(
                 f,
                 "USB_DEVICE.EP1 was read while the OUT FIFO was empty (pc {pc:#010x}), so 0 was returned; read EP1 only while EP1_CONF.SERIAL_OUT_EP_DATA_AVAIL is set"
+            ),
+            Unsupported::UsbInFull => write!(
+                f,
+                "a byte was written to USB_DEVICE.EP1 while the IN endpoint had no room (pc {pc:#010x}), so it was ignored; write EP1 only while EP1_CONF.SERIAL_IN_EP_DATA_FREE is set"
             ),
             // INVARIANT: `describe` hands only the refusals above to this function.
             _ => Ok(()),
@@ -412,6 +420,7 @@
                 Unsupported::UartRxOverflow { uart } => (24, u32::from(uart), 0),
                 Unsupported::UartEmptyRead { uart } => (25, u32::from(uart), 0),
                 Unsupported::UsbEmptyRead => (26, 0, 0),
+                Unsupported::UsbInFull => (27, 0, 0),
                 // A command is 1 to 16 bits long, so 0 is free for "no command phase".
                 Unsupported::FlashTransfer { command_bits, .. } => {
                     (11, command_bits.unwrap_or(0), 0)
```

```diff
--- a/crates/soc-esp32c3/src/lib.rs
+++ b/crates/soc-esp32c3/src/lib.rs
@@ -32,4 +32,5 @@
     FlashConfig, FlashConfigError, FlashDevice, FlashImageError, FlashWrite, JedecId, NorFlash,
 };
 pub use machine::{ConsoleChannel, Esp32C3, StopReason};
+pub use periph::UsbHost;
 pub use rom::{RomError, RomImage};
```

```diff
--- a/crates/soc-esp32c3/src/periph/mod.rs
+++ b/crates/soc-esp32c3/src/periph/mod.rs
@@ -43,7 +43,7 @@
 pub use systimer::Systimer;
 pub use timg::Timg;
 pub use uart::Uart;
-pub use usb_serial_jtag::UsbSerialJtag;
+pub use usb_serial_jtag::{UsbHost, UsbSerialJtag};
 
 /// A register whose value the emulator stores although the hardware effect is not modelled
 /// (decision D12). The lists of these are what the storage of each model is built from, and what
@@ -471,7 +471,7 @@
             radio_power: AcceptedBlock::new(0, &RADIO_POWER_ACCEPTED),
             uart0: Uart::new(0, UART0_BASE),
             uart1: Uart::new(1, UART1_BASE),
-            usb_serial_jtag: UsbSerialJtag::new(),
+            usb_serial_jtag: UsbSerialJtag::new(config.usb_host),
         };
         periph.rtc_cntl.wake(ctx);
         periph.timg0.wake(ctx);
```

```diff
--- a/crates/soc-esp32c3/src/periph/usb_serial_jtag.rs
+++ b/crates/soc-esp32c3/src/periph/usb_serial_jtag.rs
@@ -7,7 +7,11 @@
 //! §R.1, §R.5). The interrupt is source 26 (`USB_INTR_MAP`), level, driven as
 //! `INT_RAW & INT_ENA != 0` after every change (report USJ §C).
 //!
-//! The model behaves like a host attached and reading: a committed packet is taken at once.
+//! The host is part of the run (ADR 0006): absent, attached with its port closed, or open
+//! ([`UsbHost`]), and its changes are dated in device time. While attached it starts a frame
+//! every millisecond, which sets `SOF`; while open it takes each committed packet
+//! [`HOST_LATENCY`] after it is committed, and sends its own (report USJ §D, §R.2 to §R.5).
+//! Frames are counted from device time when the block is accessed, never one call per frame.
 use crate::clock::SECOND;
 use crate::ctx::{Ctx, Deadline, Event, Source};
 use crate::diag::Unsupported;
@@ -26,6 +30,7 @@
 const INT_ST: u32 = reg("INT_ST");
 const INT_ENA: u32 = reg("INT_ENA");
 const INT_CLR: u32 = reg("INT_CLR");
+const FRAM_NUM: u32 = reg("FRAM_NUM");
 /// Written by the driver and by Arduino's `HWCDC` (`CONF0`), or by nobody on the paths read
 /// (report USJ §A): stored, from their reset values, with no effect.
 const STORED: [u32; 5] = [
@@ -35,16 +40,13 @@
     reg("MEM_CONF"),
     reg("DATE"),
 ];
-/// The state of the JTAG FIFO and of the endpoints, and the frame number: read-only, served at
-/// their reset values.
+/// The state of the JTAG FIFO and of the endpoints: read-only, served at their reset values.
 ///
-/// ASSUMPTION: nobody reads them (report USJ §A: no consumer on the paths read touches them; the
-/// LL header has no accessor for `FRAM_NUM`), so the model does not derive them from its FIFOs.
-/// A firmware that reads them, or a register dump of a chip with a packet in each FIFO,
-/// settles what they hold.
-const AT_RESET: [u32; 9] = [
+/// ASSUMPTION: nobody reads them (report USJ §A: no consumer on the paths read touches them), so
+/// the model does not derive them from its FIFOs. A firmware that reads them, or a register dump
+/// of a chip with a packet in each FIFO, settles what they hold.
+const AT_RESET: [u32; 8] = [
     reg("JFIFO_ST"),
-    reg("FRAM_NUM"),
     reg("IN_EP0_ST"),
     reg("IN_EP1_ST"),
     reg("IN_EP2_ST"),
@@ -57,19 +59,25 @@
 /// zero-length packet). It reads 0. A write to `EP1_CONF` acts on this bit only: bits 1 and 2 are
 /// read-only (`usb_serial_jtag_struct.h:24-26`).
 const WR_DONE: u32 = 1 << 0;
-/// `EP1_CONF` bit 1, read-only: the IN endpoint takes a byte (no committed packet waits for the
-/// host, and the FIFO holds fewer than 64 bytes).
+/// `EP1_CONF` bit 1, read-only: the IN endpoint takes a byte: no committed packet waits for the
+/// host, and the FIFO holds fewer than 64 bytes ("After writing `WR_DONE`, this will be 1'b0
+/// until the data is sent to the USB Host", `usb_serial_jtag_struct.h:25`).
 const SERIAL_IN_EP_DATA_FREE: u32 = 1 << 1;
 /// `EP1_CONF` bit 2, read-only: the OUT FIFO holds a byte of the host.
 const SERIAL_OUT_EP_DATA_AVAIL: u32 = 1 << 2;
 /// The bits of `INT_RAW` the model sets (`usb_serial_jtag_reg.h:554-660`, report USJ §R.1):
-/// `SERIAL_OUT_RECV_PKT` (a packet of the host is in the OUT FIFO), `SERIAL_IN_EMPTY` (the host
-/// took an IN packet; 1 at reset), `OUT_EP1_ZERO_PAYLOAD` (the host sent a zero-length packet).
+/// `SOF` (a frame began), `SERIAL_OUT_RECV_PKT` (a packet of the host is in the OUT FIFO),
+/// `SERIAL_IN_EMPTY` (the host took an IN packet; 1 at reset), `USB_BUS_RESET` (the host
+/// attached), `OUT_EP1_ZERO_PAYLOAD` (the host sent a zero-length packet).
+const SOF: u32 = 1 << 1;
 const SERIAL_OUT_RECV_PKT: u32 = 1 << 2;
 const SERIAL_IN_EMPTY: u32 = 1 << 3;
+const USB_BUS_RESET: u32 = 1 << 9;
 const OUT_EP1_ZERO_PAYLOAD: u32 = 1 << 10;
 /// `INT_ENA` and `INT_CLR` take the 19 bits of `USB_SERIAL_JTAG_LL_INTR_MASK` (`ll.h:19`).
 const INTERRUPT_BITS: u32 = 0x7FFFF;
+/// `FRAM_NUM` holds the 11-bit number of the last frame (`SOF_FRAME_INDEX` [10:0]).
+const FRAME_NUMBERS: u64 = 2048;
 /// Size of a full-speed bulk packet: the hardware sends a full FIFO without waiting for `WR_DONE`.
 /// Source: ESP-IDF v5.5.2 `components/hal/esp32c3/include/hal/usb_serial_jtag_ll.h`, note on
 /// `usb_serial_jtag_ll_txfifo_flush`: "When fifo is full (with 64 byte), HW will flush the buffer
@@ -77,51 +85,115 @@
 pub const PACKET_BYTES: usize = 64;
 /// The interrupt source of the block (report USJ §C).
 const SOURCE: Source = Source::of("USB_INTR_MAP");
-/// The device time the host takes to move a packet: an OUT packet of the host reaches the FIFO
-/// this long after it is sent, or after the previous one has been read whole.
+/// One full-speed frame: "SOF packet is sent by the HOST every 1ms on a full speed bus"
+/// (`IDF:esp_driver_usb_serial_jtag/src/usb_serial_jtag_connection_monitor.c:39-41`).
+pub const FRAME: u64 = SECOND / 1_000;
+/// The device time the host takes to move a packet: an open host takes a committed IN packet
+/// this long after it is committed, and its own OUT packet reaches the FIFO this long after it
+/// is sent, or after the previous one has been read whole.
 ///
 /// ASSUMPTION: 50 us, about one full-speed bulk transaction of 64 bytes with its token and its
-/// handshake (report USJ §D.2, §O.5). No source gives a figure; a board with a host writing to
-/// it settles it.
+/// handshake, independent of the frames (report USJ §D.2, O.5). No source gives a figure; a
+/// board with a host reading and writing it settles it.
 pub const HOST_LATENCY: u64 = 50 * SECOND / 1_000_000;
+
+/// The USB host on the other side of the cable (report USJ §D.1, §R.2).
+///
+/// ASSUMPTION: a host whose port is closed starts frames but takes nothing (Linux `cdc-acm`
+/// submits its bulk transfers when the tty is opened); an attach from `Absent` raises
+/// `USB_BUS_RESET`, a detach raises nothing (report USJ O.3). A capture on a board settles it.
+#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
+pub enum UsbHost {
+    /// No cable, or a charger: no frame, nothing taken, nothing sent.
+    Absent,
+    /// Enumerated, no program has the port open: a frame every millisecond, nothing taken,
+    /// nothing sent.
+    Attached,
+    /// A program has the port open: frames, every packet taken, the host's bytes sent.
+    #[default]
+    Open,
+}
+
+impl UsbHost {
+    /// The state as the command line and a front end name it.
+    #[must_use]
+    pub const fn name(self) -> &'static str {
+        match self {
+            UsbHost::Absent => "absent",
+            UsbHost::Attached => "attached",
+            UsbHost::Open => "open",
+        }
+    }
+
+    /// Every state, in the order a front end lists them.
+    pub const ALL: [UsbHost; 3] = [UsbHost::Absent, UsbHost::Attached, UsbHost::Open];
+}
 
 #[derive(Debug)]
 pub struct UsbSerialJtag {
     stored: Storage,
     /// The bytes of the IN packet being filled.
     in_fifo: Vec<u8>,
+    /// The committed IN packet the host has not taken, with the device time it was committed.
+    committed: Option<(u64, Vec<u8>)>,
     /// The bytes the host took, for the console.
     tx: Vec<u8>,
     /// The packets the host sends and that have not reached the FIFO, each with the device
     /// time, in cycles, before which it is not sent; a zero-length packet is empty.
-    host: VecDeque<(u64, Vec<u8>)>,
+    sent: VecDeque<(u64, Vec<u8>)>,
     /// The packet of the host in the OUT FIFO, what the firmware has not read of it.
     out_fifo: VecDeque<u8>,
     /// When the OUT FIFO was last left empty: the host sends its next packet from then on.
     out_free_since: u64,
+    /// The state of the host as of `settled_to`.
+    host: UsbHost,
+    /// The changes of the host still to come, by date.
+    host_changes: VecDeque<(u64, UsbHost)>,
+    /// The device time the host attached: frames start then and every [`FRAME`] after it.
+    attached_at: Option<u64>,
+    /// The device time the host opened its port.
+    open_since: Option<u64>,
+    /// The frames of the current attachment whose `SOF` has been raised.
+    frames_raised: u64,
+    /// `FRAM_NUM`: the number of the last frame.
+    frame_number: u32,
     int_raw: u32,
     int_ena: u32,
-    /// The call at the instant the next packet of the host reaches the FIFO.
+    /// The call at the next instant the block has something to do.
     wake: Deadline,
 }
 
 impl Default for UsbSerialJtag {
     fn default() -> UsbSerialJtag {
-        UsbSerialJtag::new()
+        UsbSerialJtag::new(UsbHost::default())
     }
 }
 
 impl UsbSerialJtag {
+    /// The block at power-on, with `host` as the host is when the chip starts.
+    ///
+    /// ASSUMPTION: a host attached at power-on starts its frames at device time 0 and raises no
+    /// `USB_BUS_RESET`: `INT_RAW` reads its reset value, `0x8`, until something happens. The bus
+    /// reset of an enumeration after power-on is not modelled; a capture of `INT_RAW` on a board
+    /// plugged in before power-on settles it.
     #[must_use]
-    pub fn new() -> UsbSerialJtag {
+    pub fn new(host: UsbHost) -> UsbSerialJtag {
         let base = svd_regs::esp32c3::addr("USB_DEVICE", "EP1");
+        let attached = host != UsbHost::Absent;
         UsbSerialJtag {
             stored: Storage::for_offsets(base, &STORED),
             in_fifo: Vec::new(),
+            committed: None,
             tx: Vec::new(),
-            host: VecDeque::new(),
+            sent: VecDeque::new(),
             out_fifo: VecDeque::new(),
             out_free_since: 0,
+            host,
+            host_changes: VecDeque::new(),
+            attached_at: attached.then_some(0),
+            open_since: (host == UsbHost::Open).then_some(0),
+            frames_raised: 0,
+            frame_number: 0,
             // The SVD's reset value: `SERIAL_IN_EMPTY`.
             int_raw: SERIAL_IN_EMPTY,
             int_ena: 0,
@@ -129,25 +201,24 @@
         }
     }
 
-    /// A core reset returns the block to its reset values and empties both FIFOs; what the host
-    /// took stays for the console. The host is no part of the chip: the packets it is sending go
-    /// on arriving.
+    /// A core reset returns the block to its reset values and empties both FIFOs and the
+    /// committed packet; what the host took stays for the console. The host is no part of the
+    /// chip: its state, its frames and the packets it is sending go on.
     ///
     /// ASSUMPTION: a core reset resets the block, as it resets "the whole digital system"
-    /// (TRM §6.1.3, report IR §4.2); a CPU reset (`esp_restart` on the ESP32-C3) keeps it, as
-    /// report IR §4.2 lists for the other peripherals, and does not reach this function. The
-    /// reset table of the TRM, or a chip whose USB-Serial-JTAG is read after a watchdog reset,
-    /// settles it.
+    /// (TRM §6.1.3, report IR §4.2), without the host seeing a detach; a CPU reset
+    /// (`esp_restart` on the ESP32-C3) keeps the block, as report IR §4.2 lists for the other
+    /// peripherals, and does not reach this function. The reset table of the TRM, or a chip
+    /// whose USB-Serial-JTAG is read after a watchdog reset, settles it.
     pub(crate) fn reset(&mut self, ctx: &mut Ctx) {
         self.settle(ctx);
-        let host = std::mem::take(&mut self.host);
-        let tx = std::mem::take(&mut self.tx);
-        *self = UsbSerialJtag {
-            host,
-            tx,
-            out_free_since: ctx.now().get(),
-            ..UsbSerialJtag::new()
-        };
+        self.stored = UsbSerialJtag::new(self.host).stored;
+        self.in_fifo.clear();
+        self.committed = None;
+        self.out_fifo.clear();
+        self.out_free_since = ctx.now().get();
+        self.int_raw = SERIAL_IN_EMPTY;
+        self.int_ena = 0;
         self.drive(ctx);
         self.arm(ctx);
     }
@@ -158,40 +229,69 @@
         std::mem::take(&mut self.tx)
     }
 
-    /// Every byte the firmware wrote and no call has taken, the packet being filled included:
-    /// for the end of a run, so that the console shows what the firmware had written when it
-    /// stopped.
+    /// Every byte the firmware wrote and no call has taken, the committed packet and the packet
+    /// being filled included: for the end of a run, so that the console shows what the firmware
+    /// had written when it stopped.
     pub fn take_written(&mut self) -> Vec<u8> {
         let mut written = std::mem::take(&mut self.tx);
+        if let Some((_, packet)) = self.committed.take() {
+            written.extend(packet);
+        }
         written.append(&mut self.in_fifo);
         written
     }
 
     /// The host sends `bytes` from device time `at` (now or later), after what it sent before:
-    /// packets of at most 64 bytes; no byte at all is one zero-length packet. The caller bounds
-    /// how many it hands over ([`UsbSerialJtag::host_pending`]).
+    /// packets of at most 64 bytes; no byte at all is one zero-length packet. A packet waits for
+    /// the host to have its port open. The caller bounds how many it hands over
+    /// ([`UsbSerialJtag::host_pending`]).
     pub fn receive(&mut self, ctx: &mut Ctx, at: Cycles, bytes: &[u8]) {
         self.settle(ctx);
         let date = at.max(ctx.now()).get();
         if bytes.is_empty() {
-            self.host.push_back((date, Vec::new()));
+            self.sent.push_back((date, Vec::new()));
         }
         for packet in bytes.chunks(PACKET_BYTES) {
-            self.host.push_back((date, packet.to_vec()));
+            self.sent.push_back((date, packet.to_vec()));
         }
         self.arm(ctx);
     }
 
+    /// From device time `at` (now or later) the host is `state`. Changes given for the same
+    /// instant follow each other in the order given. The caller bounds how many it hands over
+    /// ([`UsbSerialJtag::host_changes_pending`]).
+    pub fn set_host(&mut self, ctx: &mut Ctx, at: Cycles, state: UsbHost) {
+        self.settle(ctx);
+        let date = at.max(ctx.now()).get();
+        let place = self
+            .host_changes
+            .partition_point(|&(other, _)| other <= date);
+        self.host_changes.insert(place, (date, state));
+        self.arm(ctx);
+    }
+
+    /// The state of the host now, the changes due by now applied.
+    pub fn host(&mut self, ctx: &mut Ctx) -> UsbHost {
+        self.settle(ctx);
+        self.host
+    }
+
     /// How many bytes the host has sent that the firmware has not read, its packets counted as
-    /// one byte each so that zero-length ones count too.
+    /// one byte each at least, so that zero-length ones count too.
     #[must_use]
     pub fn host_pending(&self) -> usize {
         self.out_fifo.len()
             + self
-                .host
+                .sent
                 .iter()
                 .map(|(_, packet)| packet.len().max(1))
                 .sum::<usize>()
+    }
+
+    /// How many changes of the host are still to come.
+    #[must_use]
+    pub fn host_changes_pending(&self) -> usize {
+        self.host_changes.len()
     }
 
     /// The device time of a wake-up has come.
@@ -201,48 +301,152 @@
         self.arm(ctx);
     }
 
-    /// The instant the next packet of the host reaches the FIFO: once the FIFO is empty, the
-    /// host latency after the packet is sent or the FIFO was left empty, whichever is later.
+    /// The instant an open host takes the committed packet.
+    fn drain_at(&self) -> Option<u64> {
+        let (committed_at, _) = self.committed.as_ref()?;
+        let open_since = self.open_since?;
+        Some((*committed_at).max(open_since).saturating_add(HOST_LATENCY))
+    }
+
+    /// The instant the next packet of an open host reaches the FIFO: once the FIFO is empty, the
+    /// host latency after the packet is sent, the FIFO was left empty or the port opened,
+    /// whichever is latest.
     fn next_packet_at(&self) -> Option<u64> {
         if !self.out_fifo.is_empty() {
             return None;
         }
-        let &(date, _) = self.host.front()?;
-        Some(date.max(self.out_free_since).saturating_add(HOST_LATENCY))
-    }
-
+        let open_since = self.open_since?;
+        let &(date, _) = self.sent.front()?;
+        Some(
+            date.max(self.out_free_since)
+                .max(open_since)
+                .saturating_add(HOST_LATENCY),
+        )
+    }
+
+    /// The frames started from `attached_at` to `instant`, the one starting at `instant`
+    /// included.
+    fn frames_by(&self, instant: u64) -> u64 {
+        self.attached_at
+            .filter(|&attached_at| attached_at <= instant)
+            .map_or(0, |attached_at| (instant - attached_at) / FRAME + 1)
+    }
+
+    /// Raises `SOF` for the frames started by `instant`.
+    fn raise_frames(&mut self, instant: u64) {
+        let frames = self.frames_by(instant);
+        if frames > self.frames_raised {
+            self.frames_raised = frames;
+            self.int_raw |= SOF;
+            // INVARIANT: the remainder is below 2048.
+            self.frame_number = u32::try_from((frames - 1) % FRAME_NUMBERS).unwrap_or(0);
+        }
+    }
+
+    /// Asks to be called at the next instant something happens that the firmware can see: the
+    /// host takes the committed packet, a packet of the host arrives, the host changes, and,
+    /// only while `SOF` is enabled, the next frame starts.
     fn arm(&mut self, ctx: &mut Ctx) {
-        if let Some(at) = self.next_packet_at() {
-            self.wake.arm(ctx, Cycles(at), Event::UsbSerialJtag);
-        }
-    }
-
-    /// Brings the block to now: the packets of the host that have arrived enter the FIFO, one at
-    /// a time; the interrupt source follows.
+        let next_frame = (self.int_ena & SOF != 0)
+            .then(|| {
+                self.attached_at
+                    .map(|attached_at| attached_at.saturating_add(self.frames_raised * FRAME))
+            })
+            .flatten();
+        let next_change = self.host_changes.front().map(|&(at, _)| at);
+        for instant in [
+            self.drain_at(),
+            self.next_packet_at(),
+            next_change,
+            next_frame,
+        ]
+        .into_iter()
+        .flatten()
+        {
+            self.wake.arm(ctx, Cycles(instant), Event::UsbSerialJtag);
+        }
+    }
+
+    /// Brings the block to now: the host changes, takes the committed packet and sends its own,
+    /// in the order of their instants (a change of the host first at an instant they share); the
+    /// frames started raise `SOF`; the interrupt source follows.
     fn settle(&mut self, ctx: &mut Ctx) {
         let now = ctx.now().get();
-        // INVARIANT: each pass takes one packet of those the caller handed over within its bound.
-        while let Some(at) = self.next_packet_at().filter(|&at| at <= now) {
-            let Some((_, packet)) = self.host.pop_front() else {
+        // INVARIANT: each pass applies a change, takes the committed packet or takes a packet of
+        // the host, all of them bounded by the caller (`host_pending`, `host_changes_pending`).
+        loop {
+            let change = self.host_changes.front().map(|&(at, _)| at);
+            let next = [change, self.drain_at(), self.next_packet_at()]
+                .into_iter()
+                .flatten()
+                .min()
+                .filter(|&at| at <= now);
+            let Some(at) = next else {
                 break;
             };
-            if packet.is_empty() {
-                // A zero-length packet leaves the FIFO empty: the next one can follow.
-                self.int_raw |= OUT_EP1_ZERO_PAYLOAD;
-                self.out_free_since = at;
+            if change == Some(at) {
+                self.change_host(at);
+            } else if self.drain_at() == Some(at) {
+                if let Some((_, packet)) = self.committed.take() {
+                    self.tx.extend(packet);
+                    self.int_raw |= SERIAL_IN_EMPTY;
+                }
             } else {
-                self.out_fifo.extend(packet);
-                self.int_raw |= SERIAL_OUT_RECV_PKT;
+                self.take_packet(at);
             }
         }
+        self.raise_frames(now);
         self.drive(ctx);
     }
 
-    /// The bytes written so far leave as one packet: the host takes it at once, and
-    /// `SERIAL_IN_EMPTY` says so.
-    fn commit(&mut self) {
-        self.tx.append(&mut self.in_fifo);
-        self.int_raw |= SERIAL_IN_EMPTY;
+    /// The host becomes what its first pending change says, at `at`.
+    fn change_host(&mut self, at: u64) {
+        let Some((_, state)) = self.host_changes.pop_front() else {
+            return;
+        };
+        self.raise_frames(at);
+        if self.host == UsbHost::Absent && state != UsbHost::Absent {
+            self.attached_at = Some(at);
+            self.frames_raised = 0;
+            self.int_raw |= USB_BUS_RESET;
+            self.raise_frames(at);
+        }
+        if state == UsbHost::Absent {
+            self.attached_at = None;
+        }
+        self.open_since = match (self.open_since, state) {
+            (Some(since), UsbHost::Open) => Some(since),
+            (None, UsbHost::Open) => Some(at),
+            (_, UsbHost::Absent | UsbHost::Attached) => None,
+        };
+        self.host = state;
+    }
+
+    /// The next packet of the host reaches the OUT FIFO at `at`.
+    fn take_packet(&mut self, at: u64) {
+        let Some((_, packet)) = self.sent.pop_front() else {
+            return;
+        };
+        if packet.is_empty() {
+            // A zero-length packet leaves the FIFO empty: the next one can follow.
+            self.int_raw |= OUT_EP1_ZERO_PAYLOAD;
+            self.out_free_since = at;
+        } else {
+            self.out_fifo.extend(packet);
+            self.int_raw |= SERIAL_OUT_RECV_PKT;
+        }
+    }
+
+    /// The bytes written so far leave the FIFO as one packet, for the host to take.
+    ///
+    /// ASSUMPTION: `WR_DONE` while a committed packet waits for the host does nothing: the
+    /// endpoint holds one packet (report USJ §R.1). `HWCDC` writes it each time it finds itself
+    /// not connected (report USJ §B.5); the TRM settles it.
+    fn commit(&mut self, now: u64) {
+        if self.committed.is_some() {
+            return;
+        }
+        self.committed = Some((now, std::mem::take(&mut self.in_fifo)));
     }
 
     /// A read of `EP1`: the oldest byte of the host.
@@ -261,16 +465,24 @@
         u32::from(byte)
     }
 
-    /// A write of `EP1`: a byte for the host. The FIFO never holds 64 bytes: the 64th commits.
-    fn push(&mut self, byte: u8) {
+    /// A write of `EP1`: a byte for the host. The 64th commits the packet.
+    ///
+    /// ASSUMPTION: a byte written while the endpoint has no room is ignored (report USJ §R.1,
+    /// O.6), and refused by name. Every consumer read looks at `SERIAL_IN_EP_DATA_FREE` first;
+    /// the TRM settles it.
+    fn push(&mut self, ctx: &mut Ctx, byte: u8) {
+        if self.ep1_conf() & SERIAL_IN_EP_DATA_FREE == 0 {
+            ctx.unsupported(Unsupported::UsbInFull);
+            return;
+        }
         self.in_fifo.push(byte);
         if self.in_fifo.len() == PACKET_BYTES {
-            self.commit();
+            self.commit(ctx.now().get());
         }
     }
 
     fn ep1_conf(&self) -> u32 {
-        let free = if self.in_fifo.len() < PACKET_BYTES {
+        let free = if self.committed.is_none() && self.in_fifo.len() < PACKET_BYTES {
             SERIAL_IN_EP_DATA_FREE
         } else {
             0
@@ -301,6 +513,7 @@
             INT_RAW => self.int_raw,
             INT_ST => self.int_status(),
             INT_ENA => self.int_ena,
+            FRAM_NUM => self.frame_number,
             _ if AT_RESET.contains(&offset) => {
                 svd_regs::esp32c3::lookup(svd_regs::esp32c3::addr("USB_DEVICE", "EP1") + offset)
                     .map_or(0, |register| register.reset_value)
@@ -316,12 +529,12 @@
         self.settle(ctx);
         let accepted = match offset {
             EP1 => {
-                self.push(value.to_le_bytes()[0]);
+                self.push(ctx, value.to_le_bytes()[0]);
                 true
             }
             EP1_CONF => {
                 if value & WR_DONE != 0 {
-                    self.commit();
+                    self.commit(ctx.now().get());
                 }
                 true
             }
@@ -336,6 +549,7 @@
             _ => self.stored.write(offset, value),
         };
         self.drive(ctx);
+        self.arm(ctx);
         accepted
     }
 }
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p soc-esp32c3 --lib -- periph::usb_serial_jtag:: config:: machine:: > /tmp/t5.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t5.log
```

Expected: `exit=0` and `test result: ok. 81 passed; 0 failed; 0 ignored; 0 measured; 476 filtered out`.

- [ ] **Step 5: Mutations**

| # | Mutation (`crates/soc-esp32c3/src/periph/usb_serial_jtag.rs`) | Test that fails |
|---|---|---|
| 1 | `.map_or(0, \|attached_at\| (instant - attached_at) / FRAME + 1)` → without `+ 1` | `an_attached_host_starts_a_frame_every_millisecond_and_the_connection_monitor_sees_each`, `an_enabled_sof_asks_for_a_call_at_each_frame_start`, `an_attach_raises_usb_bus_reset_…` |
| 2 | delete `self.int_raw \|= USB_BUS_RESET;` | `an_attach_raises_usb_bus_reset_and_starts_the_frames_from_its_instant_and_a_detach_raises_nothing` |
| 3 | `Some((*committed_at).max(open_since).saturating_add(HOST_LATENCY))` → `Some((*committed_at).saturating_add(HOST_LATENCY))` | `a_host_with_its_port_closed_starts_frames_but_takes_and_sends_nothing` |
| 4 | delete `if self.committed.is_some() { return; }` in `commit` | `a_byte_written_while_a_packet_waits_for_the_host_is_refused_and_lost` |
| 5 | `let free = if self.committed.is_none() && self` → `let free = if self` | `a_byte_written_while_a_packet_waits_for_the_host_is_refused_and_lost` and three others |
| 6 | `let next_frame = (self.int_ena & SOF != 0)` → `== 0` | `an_attached_host_starts_a_frame_every_millisecond_and_the_connection_monitor_sees_each` |
| 7 | `(_, UsbHost::Absent \| UsbHost::Attached) => None,` → `=> self.open_since,` | `a_host_with_its_port_closed_starts_frames_but_takes_and_sends_nothing` |
| 8 | `.partition_point(\|&(other, _)\| other <= date);` → `.len();` | `changes_of_the_host_given_out_of_order_apply_in_the_order_of_their_dates` |

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy -p soc-esp32c3 --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git commit -m "feat(soc-esp32c3): the USB host is absent, attached or open: frames set SOF every millisecond and an open host takes each IN packet 50 us after its commit" -m "The IN endpoint is no longer always free: the ROM mirrors every console byte to USB-Serial-JTAG and now waits for the host to take each packet, so the pinned trace hashes, device times and instruction counts of the reference runs change." -- crates/soc-esp32c3/src
```

Expected: three `exit=0`, then the commit.

- [ ] **Step 7: Re-pin, from the CI, what the host model moves**

These values come from the real ROM and the reference images, which only the CI runs (C11). Push the branch after this task (or with the next ones) and read the logs of the jobs `reference-boot` and `rom-boot`.

1. In the log of the step "Boot the reference firmware" of `reference-boot`, the two tests `x4_runs_the_reference_firmware_along_its_pinned_trace` and `x4_runs_the_reference_firmware_from_its_application_image_along_its_pinned_trace` fail on their first changed assertion: the trace hash first (`left: Some("trace-hash …")`, `right: Some("trace-hash 10174cc2726c3d77")`, and `c6b24a1cc226ed4a` for the second). Write each `left` value in place of the `right` one in `crates/emu-cli/tests/run_cli.rs`, push, and do the same for the next assertions as they fail: `device_micros(&log)` (`11_396_706` and `11_403_054`), then the line `after 40000001 cycles (… instructions)` (`26562395` and `26637404`). Find each literal with `/usr/bin/git grep -n` rather than by line number.
2. Check that the change is the expected one, and only it: the console of both tests, which they compare line by line, must not change; the device time must grow (the ROM now waits up to 50 us per packet of its USB copy), by less than 1 ms per 64 bytes the ROM printed.
3. In the log of `rom-boot`, `real_rom` must still run its 18 tests. If a watchdog test fails because the PC it finds moved (`ROM_UART_WAIT_PC`, `0x4004_c0d4`, in `ROM3:uart_tx_one_char_uart.part.0@0x4004c0be` by the comment beside it), the ROM now spends part of its print in the USB copy: take the PC the failure prints, name the ROM function it falls in (the symbol table of the ELF the test loaded, `INKEMU_ROM_ELF_DIR/esp32c3_rev3_rom.elf`), update the constant and its comment, and keep the test's claim (a watchdog catches the ROM in a wait of its console routine). A window of device time that fails moves by what step 2 measured; re-pin it the same way, never wider than it was.
4. Commit: `/usr/bin/git commit -m "test: re-pin the reference traces: the ROM waits for the USB host to take its console" -- crates/emu-cli/tests/run_cli.rs` (and `crates/soc-esp32c3/tests/real_rom.rs` when step 3 applied), with the old and new values in the body. The hand-over note lists them (Task 11).

---

## Task 6: The machine takes dated console input and USB host changes, and stops where a watched console speaks

The inputs of a run (C6, C21, C22, ADR 0006). `Esp32C3::send_console(channel, at, bytes)` and `set_usb_host(at, state)` hand dated inputs to the peripherals; a date before the device time is refused with both instants (`InputError::InThePast`), a channel holding `MAX_PENDING_HOST_BYTES` refuses more (`TooMuchPending`, a zero-length packet counting one), and the machine keeps at most `MAX_PENDING_HOST_CHANGES` changes of host (`TooManyHostChanges`); a refused input changes nothing. `watch_console(channel, true)` makes every byte that channel hands to the host end the call of `run_within` with `StopReason::ConsoleOutput`, at the instruction boundary it left at, or at its very instant while the CPU sleeps: `device_time()` is then the time it reached the host. A watched UART schedules an event at each byte's departure; a USB packet is already one. The CLI's run loop goes on after `ConsoleOutput`.

**Files:**
- Modify: `crates/soc-esp32c3/src/machine.rs`, `crates/soc-esp32c3/src/lib.rs`, `crates/soc-esp32c3/src/periph/uart.rs`, `crates/soc-esp32c3/src/periph/usb_serial_jtag.rs`, `crates/emu-cli/src/run.rs`

**Interfaces:**
- Consumes: `Uart::receive`/`host_pending` (Task 2), `UsbSerialJtag::receive`/`set_host`/`host`/`host_changes_pending` (Tasks 4, 5).
- Produces: `pub const fn ConsoleChannel::name(self) -> &'static str`; `pub const MAX_PENDING_HOST_BYTES: usize = 1 << 20`; `pub const MAX_PENDING_HOST_CHANGES: usize = 16_384`; `pub enum InputError { InThePast { at_nanos: u64, now_nanos: u64 }, TooMuchPending { channel: ConsoleChannel, pending: usize }, TooManyHostChanges }`; `StopReason::ConsoleOutput`; on `Esp32C3`: `pub const fn device_time(&self) -> Cycles`, `pub fn send_console(&mut self, channel: ConsoleChannel, at: Cycles, bytes: &[u8]) -> Result<(), InputError>`, `pub fn input_pending(&self, channel: ConsoleChannel) -> usize`, `pub fn set_usb_host(&mut self, at: Cycles, state: UsbHost) -> Result<(), InputError>`, `pub fn usb_host(&mut self) -> UsbHost`, `pub fn usb_host_changes_pending(&self) -> usize`, `pub fn watch_console(&mut self, channel: ConsoleChannel, watched: bool)`; `pub(crate) fn Uart::watch(&mut self, ctx: &mut Ctx, watched: bool)`, `pub(crate) fn has_left(&self) -> bool` on both peripherals.

**Bounds on untrusted input.** The two constants above; `send_console` checks the bound before it queues anything.

- [ ] **Step 1: Write the failing tests**

```diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -609,7 +609,7 @@
         HANG, T0, T1, flash_erase_sector, flash_program, flat_rom, load_register,
         print_program as print, stop_watchdogs, store_register, unmodeled_register, within,
     };
-    use cpu_rv32::asm::{addi, csrrs, csrrwi, jal, lui, lw, sw, wfi};
+    use cpu_rv32::asm::{addi, beq, csrrs, csrrwi, jal, lbu, lui, lw, sw, wfi};
     use cpu_rv32::csr_esp32c3::{PCCR_USER, PCER_USER, PCMR_USER};
     use emu_core::{AccessKind, AccessSize, Bus};
 
@@ -982,6 +982,247 @@
         };
         assert!(diagnostic.to_string().contains("LEDC"), "{diagnostic}");
         assert!(m.cycles().get() < 100, "stopped right after the access");
+    }
+
+    /// A program that echoes on UART0 every byte it receives: it polls `STATUS.RXFIFO_CNT` (its
+    /// low byte), reads `FIFO`, writes the byte back.
+    fn echo_uart0() -> Vec<u32> {
+        vec![
+            lui(T0, UART0),
+            lbu(T1, T0, 0x1C),
+            beq(T1, 0, -4),
+            lw(T1, T0, 0),
+            sw(T1, T0, 0),
+            jal(0, -16),
+        ]
+    }
+
+    /// A byte at the reset settings of UART0: `CLKDIV` 694 on the crystal divided by 2, 10 bit
+    /// times: 55 520 cycles of the time base, 347 us.
+    const RESET_BYTE: u64 = 55_520;
+
+    #[test]
+    fn an_input_dated_before_the_device_time_is_refused_and_changes_nothing() {
+        let mut m = machine(&echo_uart0());
+        m.run_until(Cycles(100));
+        let now = m.device_time();
+        assert_eq!(
+            m.send_console(ConsoleChannel::Uart0, Cycles(now.get() - 1), b"x"),
+            Err(InputError::InThePast {
+                at_nanos: clock::TIME_BASE.cycles_to_nanos(Cycles(now.get() - 1)),
+                now_nanos: m.device_time_nanos(),
+            })
+        );
+        assert_eq!(m.input_pending(ConsoleChannel::Uart0), 0);
+        assert_eq!(
+            m.set_usb_host(Cycles::ZERO, UsbHost::Absent),
+            Err(InputError::InThePast {
+                at_nanos: 0,
+                now_nanos: m.device_time_nanos(),
+            })
+        );
+        assert_eq!(m.usb_host(), UsbHost::Open);
+        assert_eq!(m.send_console(ConsoleChannel::Uart0, now, b"x"), Ok(()));
+        assert_eq!(m.input_pending(ConsoleChannel::Uart0), 1);
+        let message = InputError::InThePast {
+            at_nanos: 5,
+            now_nanos: 10,
+        }
+        .to_string();
+        assert!(
+            message.ends_with("; date it at 10 ns or later"),
+            "{message}"
+        );
+    }
+
+    #[test]
+    fn a_channel_holds_a_megabyte_of_the_host_at_most() {
+        let mut m = machine(&[HANG]);
+        let megabyte = vec![b'.'; MAX_PENDING_HOST_BYTES];
+        assert_eq!(
+            m.send_console(ConsoleChannel::Uart1, Cycles::ZERO, &megabyte),
+            Ok(())
+        );
+        assert_eq!(
+            m.send_console(ConsoleChannel::Uart1, Cycles::ZERO, b"!"),
+            Err(InputError::TooMuchPending {
+                channel: ConsoleChannel::Uart1,
+                pending: MAX_PENDING_HOST_BYTES + 1,
+            })
+        );
+        assert_eq!(
+            m.send_console(ConsoleChannel::UsbSerialJtag, Cycles::ZERO, &megabyte),
+            Ok(())
+        );
+        assert_eq!(
+            m.send_console(ConsoleChannel::UsbSerialJtag, Cycles::ZERO, b""),
+            Err(InputError::TooMuchPending {
+                channel: ConsoleChannel::UsbSerialJtag,
+                pending: MAX_PENDING_HOST_BYTES + 1,
+            }),
+            "a zero-length packet counts"
+        );
+        for _ in 0..MAX_PENDING_HOST_CHANGES {
+            assert_eq!(m.set_usb_host(Cycles(1_000), UsbHost::Open), Ok(()));
+        }
+        assert_eq!(
+            m.set_usb_host(Cycles(1_000), UsbHost::Open),
+            Err(InputError::TooManyHostChanges)
+        );
+        assert!(
+            InputError::TooManyHostChanges
+                .to_string()
+                .contains("; give the next ones")
+        );
+    }
+
+    #[test]
+    fn a_firmware_receives_the_bytes_of_the_host_at_their_dates() {
+        let mut m = machine(&echo_uart0());
+        let sent_at = Cycles(1_000_000);
+        m.send_console(ConsoleChannel::Uart0, sent_at, b"hi")
+            .unwrap();
+        let landed_ns = clock::TIME_BASE.cycles_to_nanos(sent_at + RESET_BYTE);
+        m.run_within(Cycles(u64::MAX), Some(landed_ns - 1_000));
+        assert_eq!(
+            m.input_pending(ConsoleChannel::Uart0),
+            2,
+            "h is on the line"
+        );
+        assert_eq!(m.take_console(ConsoleChannel::Uart0), b"");
+        m.run_within(Cycles(u64::MAX), Some(landed_ns + 3 * 347_000));
+        assert_eq!(m.take_console(ConsoleChannel::Uart0), b"hi");
+        assert_eq!(m.input_pending(ConsoleChannel::Uart0), 0);
+        assert_eq!(m.drain_diagnostics(), Vec::new());
+    }
+
+    #[test]
+    fn a_watched_channel_ends_the_call_at_the_instant_each_byte_reaches_the_host() {
+        // The CPU sleeps after the writes: each byte is seen at its very instant.
+        let mut program = print(UART0, b"abc");
+        program.extend([wfi(), HANG]);
+        let mut m = machine(&program);
+        m.watch_console(ConsoleChannel::Uart0, true);
+        assert_eq!(m.run_until(Cycles(1_000_000)), StopReason::ConsoleOutput);
+        assert_eq!(m.take_console(ConsoleChannel::Uart0), b"a");
+        let first = m.device_time();
+        for (byte, after) in [(b"b", 1), (b"c", 2)] {
+            assert_eq!(m.run_until(Cycles(1_000_000)), StopReason::ConsoleOutput);
+            assert_eq!(m.take_console(ConsoleChannel::Uart0), byte);
+            assert_eq!(
+                m.device_time(),
+                first + after * RESET_BYTE,
+                "back to back, to the cycle"
+            );
+        }
+        m.watch_console(ConsoleChannel::Uart0, false);
+        assert_ne!(m.run_until(Cycles(1_000_000)), StopReason::ConsoleOutput);
+    }
+
+    #[test]
+    fn a_watched_byte_ends_the_call_at_the_first_instruction_boundary_after_it_leaves() {
+        // While the CPU executes: the boundary is the one a caller taking the console after
+        // every instruction first finds the byte at.
+        let mut program = print(UART0, b"a");
+        program.push(HANG);
+        let mut stepped = machine(&program);
+        let seen_at = loop {
+            stepped.run_until(stepped.cycles() + 1);
+            if stepped.take_console(ConsoleChannel::Uart0) == b"a" {
+                break stepped.device_time();
+            }
+        };
+        let mut watched = machine(&program);
+        watched.watch_console(ConsoleChannel::Uart0, true);
+        assert_eq!(
+            watched.run_until(Cycles(1_000_000)),
+            StopReason::ConsoleOutput
+        );
+        assert_eq!(watched.take_console(ConsoleChannel::Uart0), b"a");
+        assert_eq!(watched.device_time(), seen_at);
+    }
+
+    /// Everything a run shows of itself: its consoles, its trace hash, its device time, its
+    /// cycles and instructions.
+    type EchoRun = (Vec<u8>, Vec<u8>, Option<u64>, Cycles, Cycles, u64);
+
+    /// The echo firmware with three inputs and a change of the USB host, run to 40 ms of device
+    /// time in calls of `slice` cycles, watching UART0 or not.
+    fn echo_run(slice: u64, watched: bool) -> EchoRun {
+        let mut m = machine_with(
+            &echo_uart0(),
+            SocConfig {
+                trace_hash: true,
+                ..SocConfig::default()
+            },
+        );
+        m.watch_console(ConsoleChannel::Uart0, watched);
+        m.send_console(ConsoleChannel::Uart0, Cycles(800_000), b"ab")
+            .unwrap();
+        m.send_console(ConsoleChannel::Uart0, Cycles(800_001), b"c")
+            .unwrap();
+        m.send_console(ConsoleChannel::Uart0, Cycles(3_000_000), b"\r\n")
+            .unwrap();
+        m.set_usb_host(Cycles(2_000_000), UsbHost::Absent).unwrap();
+        let (mut uart0, mut usb) = (Vec::new(), Vec::new());
+        let limit_ns = 40_000_000;
+        loop {
+            let stop = m.run_within(m.cycles() + slice, Some(limit_ns));
+            uart0.extend(m.take_console(ConsoleChannel::Uart0));
+            usb.extend(m.take_console(ConsoleChannel::UsbSerialJtag));
+            match stop {
+                StopReason::TimeBudget => break,
+                StopReason::CycleBudget | StopReason::Asleep | StopReason::ConsoleOutput => {}
+                other => panic!("{other:?}"),
+            }
+        }
+        let trace = m.trace_hash();
+        (
+            uart0,
+            usb,
+            trace,
+            m.device_time(),
+            m.cycles(),
+            m.instructions(),
+        )
+    }
+
+    #[test]
+    fn a_run_with_inputs_is_the_same_whatever_its_calls_and_whether_it_is_watched() {
+        // ADR 0006: the device time of an input is fixed by the inputs alone.
+        let whole = echo_run(u64::MAX, false);
+        assert_eq!(whole.0, b"abc\r\n");
+        assert!(whole.2.is_some());
+        for (slice, watched) in [(1, false), (997, false), (1, true), (1_000_000, true)] {
+            assert_eq!(
+                echo_run(slice, watched),
+                whole,
+                "slice {slice}, watched {watched}"
+            );
+        }
+    }
+
+    #[test]
+    fn the_usb_host_changes_at_its_date() {
+        let mut m = machine(&[HANG]);
+        m.set_usb_host(Cycles(1_000), UsbHost::Attached).unwrap();
+        assert_eq!(m.usb_host(), UsbHost::Open);
+        assert_eq!(m.usb_host_changes_pending(), 1);
+        m.run_within(
+            Cycles(u64::MAX),
+            Some(clock::TIME_BASE.cycles_to_nanos(Cycles(1_000))),
+        );
+        assert_eq!(m.usb_host(), UsbHost::Attached);
+        assert_eq!(m.usb_host_changes_pending(), 0);
+    }
+
+    #[test]
+    fn the_channels_are_named_as_the_command_line_names_them() {
+        let names: Vec<&str> = ConsoleChannel::ALL.iter().map(|c| c.name()).collect();
+        assert_eq!(names, ["uart0", "usb-serial-jtag", "uart1"]);
+        for (place, channel) in ConsoleChannel::ALL.into_iter().enumerate() {
+            assert_eq!(channel.index(), place);
+        }
     }
 
     #[test]
```

- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p soc-esp32c3 --lib -- machine:: > /tmp/t6-red.log 2>&1; echo "exit=$?"; grep -E "^error" /tmp/t6-red.log | sort | uniq -c
```

Expected: `exit=101`; `could not compile soc-esp32c3 (lib test) due to 60 previous errors`: `no method named send_console`, `device_time`, `set_usb_host`, `watch_console`, `input_pending` for `Esp32C3`, `cannot find type InputError`, `UsbHost`, `no variant … named ConsoleOutput`.

- [ ] **Step 3: The inputs, the watch, and the run loop of the CLI**

```diff
--- a/crates/emu-cli/src/run.rs
+++ b/crates/emu-cli/src/run.rs
@@ -73,7 +73,10 @@
         // The end of a slice, or a CPU that sleeps while device time goes on: the next pass
         // continues. A sleep that ended on the very instant of the device-time limit comes
         // back as `Asleep` too; the next call then runs nothing and says `TimeBudget`.
-        (Ok(true), StopReason::CycleBudget | StopReason::Asleep) => None,
+        // A watched console spoke: the console script, if any, reads it, and the run goes on.
+        (Ok(true), StopReason::CycleBudget | StopReason::Asleep | StopReason::ConsoleOutput) => {
+            None
+        }
     }
 }
 
```

```diff
--- a/crates/soc-esp32c3/src/lib.rs
+++ b/crates/soc-esp32c3/src/lib.rs
@@ -31,6 +31,9 @@
 pub use flash::{
     FlashConfig, FlashConfigError, FlashDevice, FlashImageError, FlashWrite, JedecId, NorFlash,
 };
-pub use machine::{ConsoleChannel, Esp32C3, StopReason};
+pub use machine::{
+    ConsoleChannel, Esp32C3, InputError, MAX_PENDING_HOST_BYTES, MAX_PENDING_HOST_CHANGES,
+    StopReason,
+};
 pub use periph::UsbHost;
 pub use rom::{RomError, RomImage};
```

```diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -6,6 +6,7 @@
 use crate::diag::{Diagnostic, Unsupported};
 use crate::flash::{FlashDevice, FlashImageError, FlashWrite, NorFlash};
 use crate::memmap::RESET_PC;
+use crate::periph::UsbHost;
 use crate::reset::ResetCause;
 use crate::rom::RomImage;
 use cpu_rv32::{
@@ -33,6 +34,58 @@
         ConsoleChannel::UsbSerialJtag,
         ConsoleChannel::Uart1,
     ];
+
+    /// The channel as the command line and a front end name it.
+    #[must_use]
+    pub const fn name(self) -> &'static str {
+        match self {
+            ConsoleChannel::Uart0 => "uart0",
+            ConsoleChannel::UsbSerialJtag => "usb-serial-jtag",
+            ConsoleChannel::Uart1 => "uart1",
+        }
+    }
+
+    /// The place of the channel in [`ConsoleChannel::ALL`].
+    const fn index(self) -> usize {
+        match self {
+            ConsoleChannel::Uart0 => 0,
+            ConsoleChannel::UsbSerialJtag => 1,
+            ConsoleChannel::Uart1 => 2,
+        }
+    }
+}
+
+/// The most bytes of the host a channel holds that the firmware has not read yet: what a front
+/// end may hand over ahead of the firmware ([`Esp32C3::send_console`]). A megabyte, a thousand
+/// times the 1 KiB receive buffer of the ESP-IDF drivers.
+pub const MAX_PENDING_HOST_BYTES: usize = 1 << 20;
+/// The most changes of the USB host still to come ([`Esp32C3::set_usb_host`]).
+pub const MAX_PENDING_HOST_CHANGES: usize = 16_384;
+
+/// Why the machine refused an input of the run ([`Esp32C3::send_console`],
+/// [`Esp32C3::set_usb_host`]).
+#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
+pub enum InputError {
+    /// The input is dated before the device time the machine has reached: what the firmware
+    /// did since cannot have seen it.
+    #[error(
+        "the input is dated {at_nanos} ns of device time, but the run is at {now_nanos} ns already; date it at {now_nanos} ns or later"
+    )]
+    InThePast { at_nanos: u64, now_nanos: u64 },
+    /// The channel already holds as many bytes of the host as it may.
+    #[error(
+        "{channel} would hold {pending} bytes of the host that the firmware has not read, more than {MAX_PENDING_HOST_BYTES}; send the rest once the firmware has read some",
+        channel = channel.name()
+    )]
+    TooMuchPending {
+        channel: ConsoleChannel,
+        pending: usize,
+    },
+    /// The changes of the USB host still to come are as many as they may be.
+    #[error(
+        "{MAX_PENDING_HOST_CHANGES} changes of the USB host are still to come; give the next ones once the run has reached some"
+    )]
+    TooManyHostChanges,
 }
 
 #[derive(Clone, Debug, PartialEq, Eq)]
@@ -61,6 +114,11 @@
     /// sleeping CPU returns `Asleep` although the CPU is then awake, about to execute the first
     /// instruction of the ROM. [`Esp32C3::is_asleep`] gives the state.
     Asleep,
+    /// A channel [`Esp32C3::watch_console`] watches handed bytes to the host: the call returns
+    /// at the instruction boundary at which they went (at their very instant while the CPU
+    /// sleeps), so that [`Esp32C3::device_time`] is the device time they reached the host. The
+    /// caller takes them with [`Esp32C3::take_console`] and calls again.
+    ConsoleOutput,
 }
 
 /// Device time one call to [`Esp32C3::run_within`] sleeps through, all its sleeps added, before
@@ -130,6 +188,8 @@
     selected: Option<(u32, Cycles)>,
     /// Resets since power-on.
     resets: u64,
+    /// The channels [`Esp32C3::watch_console`] watches, by their place in [`ConsoleChannel::ALL`].
+    watched: [bool; 3],
 }
 
 impl Esp32C3 {
@@ -185,6 +245,7 @@
             trace: config.trace_hash.then(TraceHash::new),
             selected: None,
             resets: 0,
+            watched: [false; 3],
         }
     }
 
@@ -247,6 +308,9 @@
                     if let Some(violation) = self.bus.ctx.diag.take_violation() {
                         return StopReason::StrictMmio(violation);
                     }
+                    if self.watched_output() {
+                        return StopReason::ConsoleOutput;
+                    }
                     if slept >= SLEEP_SLICE || sleeps >= MAX_FRUITLESS_WAKES {
                         return StopReason::Asleep;
                     }
@@ -277,12 +341,34 @@
             }
             if self.needs_service() {
                 self.service(pc);
+                // A watched byte leaves at an instant its device asked to be called at: the
+                // service of that call is the boundary it is seen at.
+                if self.watched_output() {
+                    if let Some(violation) = self.bus.ctx.diag.take_violation() {
+                        return StopReason::StrictMmio(violation);
+                    }
+                    return StopReason::ConsoleOutput;
+                }
             }
             if let Some(violation) = self.bus.ctx.diag.take_violation() {
                 return StopReason::StrictMmio(violation);
             }
         }
         StopReason::CycleBudget
+    }
+
+    /// Whether a watched channel has handed bytes to the host that no call has taken.
+    fn watched_output(&self) -> bool {
+        let periph = &self.bus.periph;
+        let left = [
+            periph.uart0.has_left(),
+            periph.usb_serial_jtag.has_left(),
+            periph.uart1.has_left(),
+        ];
+        self.watched
+            .iter()
+            .zip(left)
+            .any(|(&watched, left)| watched && left)
     }
 
     /// Whether anything happened that the loop has to act on before the next instruction.
@@ -487,6 +573,113 @@
             ConsoleChannel::UsbSerialJtag => self.bus.periph.usb_serial_jtag.take_written(),
             ConsoleChannel::Uart1 => self.bus.periph.uart1.take_written(),
         }
+    }
+
+    /// Device time since power-on, in cycles of the time base ([`clock::TIME_BASE`]): the unit
+    /// the inputs of a run are dated in.
+    #[must_use]
+    pub const fn device_time(&self) -> Cycles {
+        self.bus.ctx.now()
+    }
+
+    /// The host sends `bytes` on `channel`, the first from device time `at`, the others after
+    /// it and after what it sent before (ADR 0006): a UART receives them back to back at its
+    /// line rate (report UR §5.4), USB-Serial-JTAG in packets of at most 64 bytes, no byte at all
+    /// being one zero-length packet (report USJ §R.5). An input is part of the run: two runs
+    /// given the same inputs at the same dates are the same run, however they are cut into
+    /// calls.
+    ///
+    /// # Errors
+    /// [`InputError::InThePast`] when `at` is before [`Esp32C3::device_time`];
+    /// [`InputError::TooMuchPending`] when the channel would hold more than
+    /// [`MAX_PENDING_HOST_BYTES`] bytes of the host the firmware has not read. Nothing is sent
+    /// then.
+    pub fn send_console(
+        &mut self,
+        channel: ConsoleChannel,
+        at: Cycles,
+        bytes: &[u8],
+    ) -> Result<(), InputError> {
+        self.check_date(at)?;
+        let pending = self
+            .input_pending(channel)
+            .saturating_add(bytes.len().max(1));
+        if pending > MAX_PENDING_HOST_BYTES {
+            return Err(InputError::TooMuchPending { channel, pending });
+        }
+        let (periph, ctx) = (&mut self.bus.periph, &mut self.bus.ctx);
+        match channel {
+            ConsoleChannel::Uart0 => periph.uart0.receive(ctx, at, bytes),
+            ConsoleChannel::UsbSerialJtag => periph.usb_serial_jtag.receive(ctx, at, bytes),
+            ConsoleChannel::Uart1 => periph.uart1.receive(ctx, at, bytes),
+        }
+        Ok(())
+    }
+
+    /// The bytes of the host on `channel` the firmware has not read yet, a zero-length packet of
+    /// USB-Serial-JTAG counting as one: a front end that ends a run says when some are left.
+    #[must_use]
+    pub fn input_pending(&self, channel: ConsoleChannel) -> usize {
+        let periph = &self.bus.periph;
+        match channel {
+            ConsoleChannel::Uart0 => periph.uart0.host_pending(),
+            ConsoleChannel::UsbSerialJtag => periph.usb_serial_jtag.host_pending(),
+            ConsoleChannel::Uart1 => periph.uart1.host_pending(),
+        }
+    }
+
+    /// From device time `at` the USB host is `state` (ADR 0006; report USJ §R.2).
+    ///
+    /// # Errors
+    /// [`InputError::InThePast`] when `at` is before [`Esp32C3::device_time`];
+    /// [`InputError::TooManyHostChanges`] when [`MAX_PENDING_HOST_CHANGES`] changes are still to
+    /// come. Nothing changes then.
+    pub fn set_usb_host(&mut self, at: Cycles, state: UsbHost) -> Result<(), InputError> {
+        self.check_date(at)?;
+        let usb = &mut self.bus.periph.usb_serial_jtag;
+        if usb.host_changes_pending() >= MAX_PENDING_HOST_CHANGES {
+            return Err(InputError::TooManyHostChanges);
+        }
+        usb.set_host(&mut self.bus.ctx, at, state);
+        Ok(())
+    }
+
+    /// The USB host now, the changes due by now applied.
+    pub fn usb_host(&mut self) -> UsbHost {
+        self.bus.periph.usb_serial_jtag.host(&mut self.bus.ctx)
+    }
+
+    /// The changes of the USB host still to come.
+    #[must_use]
+    pub fn usb_host_changes_pending(&self) -> usize {
+        self.bus.periph.usb_serial_jtag.host_changes_pending()
+    }
+
+    /// Watches `channel` or stops watching it. While a channel is watched, each byte it hands to
+    /// the host ends the call of [`Esp32C3::run_within`] with [`StopReason::ConsoleOutput`], at
+    /// the instruction boundary it went at, or at its very instant while the CPU sleeps. A front
+    /// end that waits for a text then knows the device time it came at, whatever its slices.
+    /// Watching changes nothing the firmware can see.
+    pub fn watch_console(&mut self, channel: ConsoleChannel, watched: bool) {
+        self.watched[channel.index()] = watched;
+        let (periph, ctx) = (&mut self.bus.periph, &mut self.bus.ctx);
+        match channel {
+            ConsoleChannel::Uart0 => periph.uart0.watch(ctx, watched),
+            // The host takes each packet at an instant the block asked to be called at.
+            ConsoleChannel::UsbSerialJtag => {}
+            ConsoleChannel::Uart1 => periph.uart1.watch(ctx, watched),
+        }
+    }
+
+    fn check_date(&self, at: Cycles) -> Result<(), InputError> {
+        let now = self.device_time();
+        if at < now {
+            return Err(InputError::InThePast {
+                at_nanos: clock::TIME_BASE.cycles_to_nanos(at),
+                now_nanos: clock::TIME_BASE.cycles_to_nanos(now),
+            });
+        }
+        Ok(())
     }
 
     /// The programs, erases and status writes the flash chip performed since the last call,
```

```diff
--- a/crates/soc-esp32c3/src/periph/uart.rs
+++ b/crates/soc-esp32c3/src/periph/uart.rs
@@ -187,6 +187,9 @@
     /// until they have left, not for ever), a byte of the host lands, the receive timeout comes,
     /// or the transmit FIFO falls below its threshold.
     wake: Deadline,
+    /// The machine watches what this UART sends: each byte asks for a call at the instant it has
+    /// left, so that the machine sees it go at that instant ([`Uart::watch`]).
+    watched: bool,
 }
 
 impl Uart {
@@ -211,7 +214,23 @@
             int_raw: TXFIFO_EMPTY,
             idle_since: None,
             wake: Deadline::default(),
-        }
+            watched: false,
+        }
+    }
+
+    /// While `watched`, each byte asks for a call at the instant its last bit has gone, not only
+    /// the last one: the machine then sees each byte leave at its own instant, and a front end
+    /// that waits for a text knows when it came (ADR 0006). A call changes nothing the firmware
+    /// can see.
+    pub(crate) fn watch(&mut self, ctx: &mut Ctx, watched: bool) {
+        self.watched = watched;
+        self.arm(ctx);
+    }
+
+    /// Whether bytes have left that no call has taken.
+    #[must_use]
+    pub(crate) fn has_left(&self) -> bool {
+        !self.tx.is_empty()
     }
 
     /// The module is reset (`SYSTEM.PERIP_RST_EN0`, or a core reset): its registers return to their
@@ -292,9 +311,20 @@
         let timeout_enabled = self.config.read(CONF1).unwrap_or(0) & RX_TOUT_EN != 0;
         let timeout = self.timeout_at(ctx).filter(|_| timeout_enabled);
         let below_threshold = self.tx_below_threshold_at();
-        for instant in [last_gone, next_landing, timeout, below_threshold]
-            .into_iter()
-            .flatten()
+        let first_gone = self
+            .in_flight
+            .front()
+            .map(|&(gone_at, _)| gone_at)
+            .filter(|_| self.watched);
+        for instant in [
+            last_gone,
+            next_landing,
+            timeout,
+            below_threshold,
+            first_gone,
+        ]
+        .into_iter()
+        .flatten()
         {
             self.wake.arm(
                 ctx,
```

```diff
--- a/crates/soc-esp32c3/src/periph/usb_serial_jtag.rs
+++ b/crates/soc-esp32c3/src/periph/usb_serial_jtag.rs
@@ -288,6 +288,13 @@
                 .sum::<usize>()
     }
 
+    /// Whether the host took bytes that no call has taken. The host takes a packet at an instant
+    /// the block asked to be called at: a front end that watches this channel sees it then.
+    #[must_use]
+    pub(crate) fn has_left(&self) -> bool {
+        !self.tx.is_empty()
+    }
+
     /// How many changes of the host are still to come.
     #[must_use]
     pub fn host_changes_pending(&self) -> usize {
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p soc-esp32c3 --lib -- machine:: > /tmp/t6.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t6.log
cargo test -p emu-cli --lib -- run:: > /tmp/t6-cli.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t6-cli.log
```

Expected: `exit=0` and `test result: ok. 70 passed; 0 failed; 0 ignored; 0 measured; 495 filtered out`; then `exit=0` and `test result: ok. 47 passed; 0 failed; 0 ignored; 0 measured; 114 filtered out`.

- [ ] **Step 5: Mutations**

| # | Mutation | Test that fails |
|---|---|---|
| 1 | `machine.rs`: in the execute branch of `run_within`, `if self.watched_output() {` → `if false {` | `a_watched_byte_ends_the_call_at_the_first_instruction_boundary_after_it_leaves` |
| 2 | `machine.rs`: in the sleep branch, `if self.watched_output() {` → `if false {` | `a_watched_channel_ends_the_call_at_the_instant_each_byte_reaches_the_host` |
| 3 | `machine.rs`: `if at < now {` → `if at < now && false {` | `an_input_dated_before_the_device_time_is_refused_and_changes_nothing` |
| 4 | `machine.rs`: `if pending > MAX_PENDING_HOST_BYTES {` → `if pending > MAX_PENDING_HOST_BYTES + 1 {` | `a_channel_holds_a_megabyte_of_the_host_at_most` |
| 5 | `machine.rs`: `.saturating_add(bytes.len().max(1));` → `.saturating_add(bytes.len());` | `a_channel_holds_a_megabyte_of_the_host_at_most` |
| 6 | `periph/uart.rs`: `.filter(\|_\| self.watched);` → `.filter(\|_\| false);` | `a_watched_channel_ends_the_call_at_the_instant_each_byte_reaches_the_host` |

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy -p soc-esp32c3 -p emu-cli --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git commit -m "feat(soc-esp32c3): the machine takes dated console input and USB host changes, and stops where a watched console speaks" -- crates/soc-esp32c3/src crates/emu-cli/src/run.rs
```

Expected: three `exit=0`, then the commit.

---
## Task 7: The console script: its format, its refusals, and its run against a machine

A new library crate, `console-script` (C7, C8, C23, C24). `Script::parse(bytes)` reads the text: the version line `inkemu-console-script 1`, then one step per line (`send CHANNEL "BYTES"`, `wait DURATION`, `wait-for CHANNEL "TEXT"`, `usb-host STATE`), `#` comments, escapes `\r \n \t \\ \" \xHH`, every other character as its UTF-8 bytes, no translation. Every value is bounded by a constant before anything is allocated for it (`MAX_SCRIPT_BYTES` 1 MiB, `MAX_LINE_BYTES` 8,192, `MAX_STEPS` 10,000, `MAX_SEND_BYTES` 1,024, `MAX_TEXT_BYTES` 256, `MAX_TOTAL_BYTES` 512 KiB, with `const` assertions that a parsed script stays below the bounds of the machine, C22), and every refusal names its line and its column, then the cause and the remedy (`ScriptError`, `ScriptErrorKind`). `ScriptRun::start(&script, &mut machine)` watches every channel a `wait-for` names and hands the machine the steps before the first `wait-for`, dated from its device time; `observe(&mut machine, &taken)`, called after each call of the machine with what every channel sent, searches the text of the current `wait-for` in what its channel sent from the time of the step on, and when it is complete dates the next steps from that instant and hands them over; `status(now)` says whether the script is through, waits for a text, or has a step dated after `now`. A duration is rounded up to a cycle of the time base. The crate performs no host I/O: its `clippy.toml` is the fourth copy of the ban, held equal to the others by `crates/boards/tests/host_io_ban.rs`, and the CI builds it for WebAssembly.

**Files:**
- Create: `crates/console-script/Cargo.toml`, `crates/console-script/clippy.toml`, `crates/console-script/src/lib.rs`, `crates/console-script/src/parse.rs`, `crates/console-script/src/run.rs`
- Modify: `Cargo.toml`, `crates/soc-esp32c3/clippy.toml`, `crates/firmware/clippy.toml`, `crates/boards/clippy.toml`, `crates/boards/tests/host_io_ban.rs`, `tools/ci/clippy.sh`, `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: `Esp32C3::{send_console, set_usb_host, watch_console, device_time}`, `InputError`, `ConsoleChannel::{ALL, name}`, `UsbHost::{ALL, name}` (Tasks 5, 6), `soc_esp32c3::clock::TIME_BASE`.
- Produces: `pub const VERSION_LINE`, `MAX_SCRIPT_BYTES`, `MAX_LINE_BYTES`, `MAX_STEPS`, `MAX_SEND_BYTES`, `MAX_TEXT_BYTES`, `MAX_TOTAL_BYTES`; `pub enum Action { Send { channel, bytes }, Wait { nanos }, WaitFor { channel, text }, UsbHost(UsbHost) }`; `pub struct Step { pub line: usize, pub action: Action }` with `Display` as the script writes it; `pub struct Script` with `pub fn parse(bytes: &[u8]) -> Result<Script, ScriptError>` and `pub fn steps(&self) -> &[Step]`; `pub struct ScriptError { pub line, pub column, pub kind: ScriptErrorKind }`; `pub struct RunError { pub line: usize, pub source: InputError }`; `pub enum ScriptStatus { Complete, WaitingFor { step }, Ahead { step, at: Cycles } }`; `pub struct ScriptRun` with `pub fn start(script: &Script, machine: &mut Esp32C3) -> Result<ScriptRun, RunError>`, `pub fn observe(&mut self, machine: &mut Esp32C3, taken: &[(ConsoleChannel, Vec<u8>)]) -> Result<(), RunError>`, `pub fn step_count(&self) -> usize`, `pub fn status(&self, now: Cycles) -> ScriptStatus`.

**Bounds on untrusted input.** The constants above; a duration is parsed with checked arithmetic (`DurationTooLong`); the search of a `wait-for` keeps at most the length of its text.

- [ ] **Step 1: Write the failing tests**

The tests need a crate to live in: the workspace entry, the manifest and the library root come first, then `parse.rs` and `run.rs` with their tests only, and the fourth copy in the test of the ban.

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -1,6 +1,6 @@
 [workspace]
 resolver = "3"
-members = ["crates/emu-core", "crates/cpu-rv32", "crates/firmware", "crates/emu-cli", "crates/svd-gen", "crates/svd-regs", "crates/soc-esp32c3", "crates/boards"]
+members = ["crates/emu-core", "crates/cpu-rv32", "crates/firmware", "crates/emu-cli", "crates/svd-gen", "crates/svd-regs", "crates/soc-esp32c3", "crates/boards", "crates/console-script"]
 
 [workspace.package]
 version = "0.1.0"
@@ -16,6 +16,7 @@
 svd-regs = { path = "crates/svd-regs", version = "0.1.0" }
 soc-esp32c3 = { path = "crates/soc-esp32c3", version = "0.1.0" }
 boards = { path = "crates/boards", version = "0.1.0" }
+console-script = { path = "crates/console-script", version = "0.1.0" }
 thiserror = "2"
 sha2 = "0.11"
 sha1 = "0.11"
```

Create `crates/console-script/Cargo.toml`:

```toml
[package]
name = "console-script"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[lints]
workspace = true

[dependencies]
emu-core.workspace = true
soc-esp32c3.workspace = true
thiserror.workspace = true

[dev-dependencies]
cpu-rv32 = { workspace = true, features = ["test-support"] }
soc-esp32c3 = { workspace = true, features = ["test-support"] }
svd-regs.workspace = true
```

Create `crates/console-script/src/lib.rs`:

````rust
//! Console scripts: what the host sends to the serial consoles of a run, and when, written as a
//! small text (ADR 0006).
//!
//! ```text
//! inkemu-console-script 1
//! # Wait for the prompt, then type a command.
//! wait-for uart0 "ready"
//! wait 10ms
//! send uart0 "help\r"
//! usb-host absent
//! ```
//!
//! [`Script::parse`] reads the text, every refusal naming its line and its column;
//! [`ScriptRun`] hands the steps to a machine as dated inputs while it runs, and says how far the
//! script got. A front end (the command line, a graphical one) reads the file and drives the
//! machine; this crate performs no host I/O. The format is versioned by its first line, so that the
//! scenarios of a later plan can supersede it without ambiguity.
#![cfg_attr(test, allow(clippy::unwrap_used))]

mod parse;
mod run;

pub use parse::{
    Action, MAX_LINE_BYTES, MAX_SCRIPT_BYTES, MAX_SEND_BYTES, MAX_STEPS, MAX_TEXT_BYTES,
    MAX_TOTAL_BYTES, Script, ScriptError, ScriptErrorKind, Step, VERSION_LINE,
};
pub use run::{RunError, ScriptRun, ScriptStatus};
````

```diff
--- a/crates/boards/tests/host_io_ban.rs
+++ b/crates/boards/tests/host_io_ban.rs
@@ -1,5 +1,5 @@
-//! The ban on host I/O of `firmware`, `boards` and `soc-esp32c3` lives in one `clippy.toml` per
-//! crate, because clippy reads the one nearest to a crate and no other. Three copies can drift, and
+//! The ban on host I/O of `firmware`, `boards`, `soc-esp32c3` and `console-script` lives in one
+//! `clippy.toml` per crate, because clippy reads the one nearest to a crate and no other. Copies can drift, and
 //! an entry removed from all of them is silent: clippy then accepts the call. This test holds the
 //! copies equal, holds the workspace settings in each, and names every call the ban must refuse.
 #![allow(clippy::disallowed_methods)] // reads the clippy.toml files: it is a test of the build
@@ -7,7 +7,7 @@
 use std::path::PathBuf;
 
 /// The crates that do no host I/O: a graphical front end and the WebAssembly build use them.
-const CRATES: [&str; 3] = ["firmware", "boards", "soc-esp32c3"];
+const CRATES: [&str; 4] = ["firmware", "boards", "soc-esp32c3", "console-script"];
 
 /// Every path the ban refuses, by kind. A path missing from a `clippy.toml` is a call that
 /// compiles again; a new one is added here and to each copy.
@@ -117,7 +117,7 @@
 }
 
 #[test]
-fn the_three_copies_of_the_ban_are_the_same_file() {
+fn the_copies_of_the_ban_are_the_same_file() {
     let texts: Vec<(String, String)> = CRATES
         .iter()
         .map(|name| {
```

Create `crates/console-script/src/parse.rs` with its tests only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn script(body: &str) -> Result<Script, ScriptError> {
        Script::parse(format!("{VERSION_LINE}\n{body}").as_bytes())
    }

    fn refusal(body: &str) -> (usize, usize, ScriptErrorKind) {
        let error = script(body).unwrap_err();
        (error.line, error.column, error.kind)
    }

    #[test]
    fn the_four_steps_are_read_in_order_with_their_lines() {
        let read = script(
            "# a comment\n\nsend uart0 \"a\"\n  wait 250ms   # half of it\nwait-for usb-serial-jtag \"ready\"\nusb-host attached\nsend uart1 \"\"\n",
        )
        .unwrap();
        assert_eq!(
            read.steps(),
            [
                Step {
                    line: 4,
                    action: Action::Send {
                        channel: ConsoleChannel::Uart0,
                        bytes: b"a".to_vec()
                    }
                },
                Step {
                    line: 5,
                    action: Action::Wait { nanos: 250_000_000 }
                },
                Step {
                    line: 6,
                    action: Action::WaitFor {
                        channel: ConsoleChannel::UsbSerialJtag,
                        text: b"ready".to_vec()
                    }
                },
                Step {
                    line: 7,
                    action: Action::UsbHost(UsbHost::Attached)
                },
                Step {
                    line: 8,
                    action: Action::Send {
                        channel: ConsoleChannel::Uart1,
                        bytes: Vec::new()
                    }
                },
            ]
        );
    }

    #[test]
    fn escapes_give_their_bytes_and_other_characters_their_utf8_bytes() {
        let read = script("send uart0 \"\\r\\n\\t\\\\\\\"\\x00\\xFf é\"\n").unwrap();
        assert_eq!(
            read.steps()[0].action,
            Action::Send {
                channel: ConsoleChannel::Uart0,
                bytes: b"\r\n\t\\\"\x00\xff \xc3\xa9".to_vec()
            }
        );
        assert_eq!(
            read.steps()[0].to_string(),
            "send uart0 \"\\r\\n\\t\\\\\\\"\\x00\\xff \\xc3\\xa9\""
        );
    }

    #[test]
    fn durations_take_their_units_and_carriage_returns_end_lines() {
        let read = script("wait 7ns\r\nwait 3us\r\nwait 2ms\r\nwait 5s\r\n").unwrap();
        let nanos: Vec<Action> = read.steps().iter().map(|s| s.action.clone()).collect();
        assert_eq!(
            nanos,
            [7, 3_000, 2_000_000, 5_000_000_000].map(|nanos| Action::Wait { nanos })
        );
    }

    #[test]
    fn the_version_line_must_come_first() {
        let first = |text: &str| Script::parse(text.as_bytes()).unwrap_err().kind;
        assert_eq!(first(""), ScriptErrorKind::NoVersionLine);
        assert_eq!(
            first("# comment\ninkemu-console-script 1\n"),
            ScriptErrorKind::NoVersionLine
        );
        assert_eq!(
            first("inkemu-console-script 2\n"),
            ScriptErrorKind::UnknownVersion {
                found: "2".to_owned()
            }
        );
        assert_eq!(
            Script::parse(b"inkemu-console-script 1").unwrap(),
            Script::default()
        );
        assert_eq!(
            Script::parse(b"inkemu-console-script 1\r\n").unwrap(),
            Script::default()
        );
    }

    #[test]
    fn every_refusal_names_its_line_and_column() {
        let channel = |word: &str| ScriptErrorKind::UnknownChannel {
            word: word.to_owned(),
        };
        for (body, line, column, kind) in [
            (
                "jump 3s",
                2,
                1,
                ScriptErrorKind::UnknownStep {
                    word: "jump".to_owned(),
                },
            ),
            ("send uart2 \"x\"", 2, 6, channel("uart2")),
            (
                "send",
                2,
                5,
                ScriptErrorKind::Missing {
                    step: "send",
                    what: "a channel (uart0, uart1 or usb-serial-jtag)",
                },
            ),
            (
                "send uart0   # nothing",
                2,
                14,
                ScriptErrorKind::Missing {
                    step: "send",
                    what: "a string between double quotes",
                },
            ),
            (
                "usb-host closed",
                2,
                10,
                ScriptErrorKind::UnknownHostState {
                    word: "closed".to_owned(),
                },
            ),
            (
                "usb-host open now",
                2,
                15,
                ScriptErrorKind::Trailing {
                    text: "now".to_owned(),
                },
            ),
            (
                "\n\nsend uart0 \"a\" \"b\"",
                4,
                16,
                ScriptErrorKind::Trailing {
                    text: "\"b\"".to_owned(),
                },
            ),
        ] {
            assert_eq!(refusal(body), (line, column, kind), "{body:?}");
        }
    }

    #[test]
    fn every_refusal_of_a_string_or_a_duration_names_its_line_and_column() {
        for (body, line, column, kind) in [
            ("send uart0 x", 2, 12, ScriptErrorKind::NotAString),
            ("send uart0 \"x", 2, 12, ScriptErrorKind::UnclosedString),
            (
                "send uart0 \"é\\q\"",
                2,
                14,
                ScriptErrorKind::BadEscape {
                    escape: "\\q\"".to_owned(),
                },
            ),
            (
                "send uart0 \"\\x4\"",
                2,
                13,
                ScriptErrorKind::BadEscape {
                    escape: "\\x4\"".to_owned(),
                },
            ),
            (
                "wait 3",
                2,
                6,
                ScriptErrorKind::BadDuration {
                    word: "3".to_owned(),
                },
            ),
            (
                "wait ms",
                2,
                6,
                ScriptErrorKind::BadDuration {
                    word: "ms".to_owned(),
                },
            ),
            (
                "wait 1.5s",
                2,
                6,
                ScriptErrorKind::BadDuration {
                    word: "1.5s".to_owned(),
                },
            ),
            (
                "wait 18446744074s",
                2,
                6,
                ScriptErrorKind::DurationTooLong {
                    word: "18446744074s".to_owned(),
                },
            ),
            (
                "wait",
                2,
                5,
                ScriptErrorKind::Missing {
                    step: "wait",
                    what: "a duration (a whole number and ns, us, ms or s)",
                },
            ),
            ("wait-for uart0 \"\"", 2, 16, ScriptErrorKind::EmptyText),
        ] {
            assert_eq!(refusal(body), (line, column, kind), "{body:?}");
        }
    }

    #[test]
    fn a_text_that_is_not_utf8_is_refused_where_it_stops_being_utf8() {
        let mut text = format!("{VERSION_LINE}\nsend uart0 \"é").into_bytes();
        text.push(0xFF);
        let error = Script::parse(&text).unwrap_err();
        assert_eq!((error.line, error.column), (2, 14));
        assert_eq!(error.kind, ScriptErrorKind::NotUtf8);
    }

    #[test]
    fn each_limit_is_held() {
        let long = "x".repeat(MAX_SEND_BYTES);
        assert!(script(&format!("send uart0 \"{long}\"")).is_ok());
        assert_eq!(
            refusal(&format!("send uart0 \"{long}x\"")),
            (2, 12, ScriptErrorKind::SendTooLong)
        );
        let escaped = "\\x41".repeat(MAX_SEND_BYTES);
        let read = script(&format!("send uart0 \"{escaped}\"")).unwrap();
        assert_eq!(
            read.steps()[0].action,
            Action::Send {
                channel: ConsoleChannel::Uart0,
                bytes: vec![b'A'; MAX_SEND_BYTES]
            },
            "an escape is one byte"
        );
        let text = "t".repeat(MAX_TEXT_BYTES);
        assert!(script(&format!("wait-for uart0 \"{text}\"")).is_ok());
        assert_eq!(
            refusal(&format!("wait-for uart0 \"{text}t\"")),
            (2, 16, ScriptErrorKind::TextTooLong)
        );
        let line = format!("# {}", "c".repeat(MAX_LINE_BYTES - 2));
        assert!(script(&line).is_ok());
        assert_eq!(
            refusal(&format!("{line}c")),
            (
                2,
                1,
                ScriptErrorKind::LineTooLong {
                    len: MAX_LINE_BYTES + 1
                }
            )
        );
        let steps = "wait 1ns\n".repeat(MAX_STEPS);
        assert_eq!(script(&steps).unwrap().steps().len(), MAX_STEPS);
        assert_eq!(
            refusal(&format!("{steps}wait 1ns\n")),
            (MAX_STEPS + 2, 1, ScriptErrorKind::TooManySteps)
        );
        let send = format!("send uart0 \"{long}\"\n");
        let sends = send.repeat(MAX_TOTAL_BYTES / MAX_SEND_BYTES);
        assert!(script(&sends).is_ok());
        let over = MAX_TOTAL_BYTES / MAX_SEND_BYTES + 2;
        assert_eq!(
            refusal(&format!("{sends}send uart0 \"x\"\n")),
            (over, 1, ScriptErrorKind::TooManyBytes)
        );
        let file = vec![b'#'; MAX_SCRIPT_BYTES + 1];
        assert_eq!(
            Script::parse(&file).unwrap_err(),
            at(
                1,
                1,
                ScriptErrorKind::TooLong {
                    len: MAX_SCRIPT_BYTES + 1
                }
            )
        );
    }

    #[test]
    fn every_message_states_the_cause_then_the_remedy() {
        let kinds = [
            ScriptErrorKind::TooLong { len: 9 },
            ScriptErrorKind::NotUtf8,
            ScriptErrorKind::NoVersionLine,
            ScriptErrorKind::UnknownVersion { found: "2".into() },
            ScriptErrorKind::LineTooLong { len: 9 },
            ScriptErrorKind::TooManySteps,
            ScriptErrorKind::UnknownStep { word: "x".into() },
            ScriptErrorKind::Missing {
                step: "send",
                what: "a channel",
            },
            ScriptErrorKind::UnknownChannel { word: "x".into() },
            ScriptErrorKind::UnknownHostState { word: "x".into() },
            ScriptErrorKind::BadDuration { word: "x".into() },
            ScriptErrorKind::DurationTooLong { word: "x".into() },
            ScriptErrorKind::NotAString,
            ScriptErrorKind::UnclosedString,
            ScriptErrorKind::BadEscape {
                escape: "\\q".into(),
            },
            ScriptErrorKind::SendTooLong,
            ScriptErrorKind::TextTooLong,
            ScriptErrorKind::EmptyText,
            ScriptErrorKind::TooManyBytes,
            ScriptErrorKind::Trailing { text: "x".into() },
        ];
        for kind in kinds {
            let text = at(3, 7, kind).to_string();
            let (cause, remedy) = text.split_once("; ").unwrap_or_else(|| panic!("{text}"));
            assert!(cause.starts_with("line 3, column 7: "), "{text}");
            assert!(!remedy.is_empty(), "{text}");
        }
    }
}
```

Create `crates/console-script/src/run.rs` with its tests only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use cpu_rv32::asm::{beq, jal, lbu, lw, sw, wfi};
    use soc_esp32c3::test_support::{HANG, T0, T1, flat_rom, print_program};
    use soc_esp32c3::{SocConfig, StopReason, UsbHost};

    const UART0: u32 = svd_regs::esp32c3::addr("UART0", "FIFO");
    /// A byte at the reset settings of UART0, in cycles of the time base (`machine.rs`).
    const RESET_BYTE: u64 = 55_520;

    fn script(body: &str) -> Script {
        Script::parse(format!("{}\n{body}", crate::VERSION_LINE).as_bytes()).unwrap()
    }

    fn machine(program: &[u32]) -> Esp32C3 {
        Esp32C3::new(
            flat_rom(program),
            &SocConfig {
                trace_hash: true,
                ..SocConfig::default()
            },
        )
    }

    /// Prints `ready` on UART0, then echoes what UART0 receives.
    fn ready_then_echo() -> Vec<u32> {
        let mut program = print_program(UART0, b"ready");
        program.extend([
            lbu(T1, T0, 0x1C),
            beq(T1, 0, -4),
            lw(T1, T0, 0),
            sw(T1, T0, 0),
            jal(0, -16),
        ]);
        program
    }

    /// Runs `machine` with `script` to `limit_ns` of device time in calls of `slice` cycles, as a
    /// front end does: after each call it takes every console and shows it to the script.
    fn run(
        machine: &mut Esp32C3,
        script: &Script,
        slice: u64,
        limit_ns: u64,
    ) -> (Vec<u8>, ScriptRun) {
        let mut run = ScriptRun::start(script, machine).unwrap();
        let mut uart0 = Vec::new();
        loop {
            let stop = machine.run_within(machine.cycles() + slice, Some(limit_ns));
            let taken: Vec<(ConsoleChannel, Vec<u8>)> = ConsoleChannel::ALL
                .into_iter()
                .map(|channel| (channel, machine.take_console(channel)))
                .collect();
            uart0.extend(&taken[0].1);
            run.observe(machine, &taken).unwrap();
            match stop {
                StopReason::TimeBudget | StopReason::Idle => break,
                StopReason::CycleBudget | StopReason::Asleep | StopReason::ConsoleOutput => {}
                StopReason::StrictMmio(diagnostic) => panic!("{diagnostic}"),
            }
        }
        (uart0, run)
    }

    #[test]
    fn a_wait_for_dates_the_next_steps_from_the_instant_its_text_reached_the_host() {
        let mut m = machine(&ready_then_echo());
        let (console, run) = run(
            &mut m,
            &script("wait-for uart0 \"dy\"\nsend uart0 \"ok\"\n"),
            u64::MAX,
            20_000_000,
        );
        assert_eq!(console, b"readyok");
        assert_eq!(run.status(m.device_time()), ScriptStatus::Complete);
        assert_eq!(run.step_count(), 2);
    }

    #[test]
    fn the_steps_are_dated_the_same_whatever_the_calls_the_run_is_cut_into() {
        // ADR 0006: the device time of every input is fixed by the inputs alone.
        let steps = script(
            "wait-for uart0 \"ready\"\nwait 1ms\nsend uart0 \"a\"\nwait-for uart0 \"a\"\nsend uart0 \"bc\"\nusb-host attached\nwait 2ms\nsend uart0 \"\\r\"\n",
        );
        let outcome = |slice: u64| {
            let mut m = machine(&ready_then_echo());
            let (console, run) = run(&mut m, &steps, slice, 30_000_000);
            assert_eq!(
                run.status(m.device_time()),
                ScriptStatus::Complete,
                "slice {slice}"
            );
            (console, m.trace_hash(), m.device_time(), m.instructions())
        };
        let whole = outcome(u64::MAX);
        assert_eq!(whole.0, b"readyabc\r");
        for slice in [1, 7, 1_000, 1_000_000] {
            assert_eq!(outcome(slice), whole, "slice {slice}");
        }
    }

    #[test]
    fn the_text_of_a_wait_for_is_searched_only_from_the_time_of_its_step() {
        // `ready` is sent before the wait ends: the wait-for after it never sees it.
        let mut m = machine(&ready_then_echo());
        let (_, run) = run(
            &mut m,
            &script("wait 20ms\nwait-for uart0 \"ready\"\n"),
            1_000,
            40_000_000,
        );
        assert_eq!(
            run.status(m.device_time()),
            ScriptStatus::WaitingFor {
                step: Step {
                    line: 3,
                    action: Action::WaitFor {
                        channel: ConsoleChannel::Uart0,
                        text: b"ready".to_vec()
                    }
                }
            }
        );
    }

    #[test]
    fn a_text_found_where_its_bytes_cross_the_calls_and_after_a_false_start_is_found() {
        let mut m = machine(&{
            let mut program = print_program(UART0, b"rereadyX");
            program.extend([wfi(), HANG]);
            program
        });
        let (_, run) = run(&mut m, &script("wait-for uart0 \"ready\"\n"), 3, 20_000_000);
        assert_eq!(run.status(m.device_time()), ScriptStatus::Complete);
    }

    #[test]
    fn a_wait_for_ends_at_the_instant_the_last_byte_of_its_text_reached_the_host() {
        // The program sleeps after its writes: the bytes reach the host at their very instants,
        // and the `y` of `ready` leaves the fifth byte time after the first. The send after the
        // wait-for is dated then, and its echo leaves after it lands and is sent back.
        let mut program = print_program(UART0, b"ready");
        program.extend([wfi(), HANG]);
        let mut m = machine(&program);
        let mut run_with_dates =
            ScriptRun::start(&script("wait-for uart0 \"y\"\nsend uart1 \"z\"\n"), &mut m).unwrap();
        let first = loop {
            assert_eq!(m.run_until(Cycles(1_000_000)), StopReason::ConsoleOutput);
            let bytes = m.take_console(ConsoleChannel::Uart0);
            if bytes == b"r" {
                break m.device_time();
            }
        };
        let found = loop {
            m.run_until(Cycles(1_000_000));
            let taken = vec![(ConsoleChannel::Uart0, m.take_console(ConsoleChannel::Uart0))];
            run_with_dates.observe(&mut m, &taken).unwrap();
            if taken[0].1 == b"y" {
                break m.device_time();
            }
        };
        assert_eq!(found, first + 4 * RESET_BYTE);
        assert_eq!(
            m.input_pending(ConsoleChannel::Uart1),
            1,
            "sent at {found:?}"
        );
        assert_eq!(run_with_dates.status(found), ScriptStatus::Complete);
    }

    #[test]
    fn a_run_that_ends_before_the_date_of_its_last_step_says_so() {
        let mut m = machine(&[HANG]);
        let (_, run) = run(
            &mut m,
            &script("wait 1s\nusb-host absent\n"),
            1_000_000,
            10_000_000,
        );
        assert_eq!(
            run.status(m.device_time()),
            ScriptStatus::Ahead {
                step: Step {
                    line: 3,
                    action: Action::UsbHost(UsbHost::Absent)
                },
                at: Cycles(TIME_BASE.hz()),
            }
        );
        assert_eq!(m.usb_host_changes_pending(), 1);
    }

    #[test]
    fn a_wait_is_rounded_up_to_a_cycle_of_the_time_base() {
        // 160 cycles a microsecond: 25 ns is 4 cycles, 1 ns is one.
        assert_eq!(cycles_of(25), 4);
        assert_eq!(cycles_of(1), 1);
        assert_eq!(cycles_of(0), 0);
        assert_eq!(cycles_of(1_000_000_000), TIME_BASE.hz());
        // 0.16 cycle a nanosecond: the longest wait, rounded up, without overflow.
        assert_eq!(cycles_of(u64::MAX), 2_951_479_051_793_528_259);
    }

    #[test]
    fn a_refused_input_names_the_line_of_its_step() {
        let mut m = machine(&[HANG]);
        m.run_until(Cycles(10));
        let mut run = ScriptRun {
            steps: script("send uart0 \"x\"\n").steps().to_vec(),
            next: 0,
            step_time: Cycles::ZERO,
            seen: Vec::new(),
            last_dated: None,
            watched: Vec::new(),
        };
        let error = run.advance(&mut m).unwrap_err();
        assert_eq!(error.line, 2);
        assert!(
            error
                .to_string()
                .starts_with("console script, line 2: the input is dated 0 ns")
        );
    }
}
```

- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p console-script > /tmp/t7-red.log 2>&1; echo "exit=$?"; grep -E "^error" /tmp/t7-red.log | sort | uniq -c | head
```

Expected: `exit=101`; `could not compile console-script (lib) due to 2 previous errors`: `error[E0432]: unresolved imports parse::Action, …` and `run::RunError, …`: the library root names what Step 3 writes.

- [ ] **Step 3: The crate, and the fourth copy of the host-I/O ban**

```diff
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -26,9 +26,10 @@
       # The model crates, and the crates a front end composes a flash with, must go on building for
       # `wasm32-unknown-unknown`, the target of the web version: this step notices a crate that does
       # not build there. It cannot notice host I/O, which std compiles there and fails at run time:
-      # the clippy step above refuses it in firmware, boards and soc-esp32c3 (their clippy.toml).
+      # the clippy step above refuses it in firmware, boards, soc-esp32c3 and console-script (their
+      # clippy.toml).
       - name: Build the model for WebAssembly
-        run: cargo check --target wasm32-unknown-unknown -p emu-core -p svd-regs -p cpu-rv32 -p soc-esp32c3 -p firmware -p boards
+        run: cargo check --target wasm32-unknown-unknown -p emu-core -p svd-regs -p cpu-rv32 -p soc-esp32c3 -p firmware -p boards -p console-script
       - uses: EmbarkStudios/cargo-deny-action@v2
   coverage:
     runs-on: ubuntu-24.04
```

```diff
--- a/crates/boards/clippy.toml
+++ b/crates/boards/clippy.toml
@@ -5,11 +5,11 @@
 allow-unwrap-in-tests = true
 allow-expect-in-tests = true
 
-# firmware, boards and soc-esp32c3 do no host I/O: a graphical front end and the WebAssembly build
-# use them, and std on wasm32-unknown-unknown compiles these calls only to fail at run time. Files,
-# the environment, the clock, processes, threads, the console and the network belong to the front end
-# (emu-cli). Test code that reads a fixture allows it on the item that does
-# (`#[allow(clippy::disallowed_methods)]`). `crates/boards/tests/host_io_ban.rs` holds the three
+# firmware, boards, soc-esp32c3 and console-script do no host I/O: a graphical front end and the
+# WebAssembly build use them, and std on wasm32-unknown-unknown compiles these calls only to fail at
+# run time. Files, the environment, the clock, processes, threads, the console and the network belong
+# to the front end (emu-cli). Test code that reads a fixture allows it on the item that does
+# (`#[allow(clippy::disallowed_methods)]`). `crates/boards/tests/host_io_ban.rs` holds the four
 # copies of this file equal and the list below complete: change them together.
 disallowed-methods = [
     { path = "std::fs::read", reason = "no host I/O in this crate: take bytes, let the front end read files" },
```

Create `crates/console-script/clippy.toml`:

```toml
# Clippy reads the clippy.toml nearest to the crate and no other: the workspace settings of
# ../../clippy.toml are repeated here, then the host I/O this crate must not do.

# Tests may unwrap/expect: a panic is a test failure, which is the intent.
allow-unwrap-in-tests = true
allow-expect-in-tests = true

# firmware, boards, soc-esp32c3 and console-script do no host I/O: a graphical front end and the
# WebAssembly build use them, and std on wasm32-unknown-unknown compiles these calls only to fail at
# run time. Files, the environment, the clock, processes, threads, the console and the network belong
# to the front end (emu-cli). Test code that reads a fixture allows it on the item that does
# (`#[allow(clippy::disallowed_methods)]`). `crates/boards/tests/host_io_ban.rs` holds the four
# copies of this file equal and the list below complete: change them together.
disallowed-methods = [
    { path = "std::fs::read", reason = "no host I/O in this crate: take bytes, let the front end read files" },
    { path = "std::fs::read_to_string", reason = "no host I/O in this crate: take text, let the front end read files" },
    { path = "std::fs::write", reason = "no host I/O in this crate: return bytes, let the front end write files" },
    { path = "std::fs::read_dir", reason = "no host I/O in this crate" },
    { path = "std::fs::metadata", reason = "no host I/O in this crate" },
    { path = "std::fs::symlink_metadata", reason = "no host I/O in this crate" },
    { path = "std::fs::exists", reason = "no host I/O in this crate" },
    { path = "std::fs::create_dir", reason = "no host I/O in this crate" },
    { path = "std::fs::create_dir_all", reason = "no host I/O in this crate" },
    { path = "std::fs::remove_file", reason = "no host I/O in this crate" },
    { path = "std::fs::remove_dir", reason = "no host I/O in this crate" },
    { path = "std::fs::remove_dir_all", reason = "no host I/O in this crate" },
    { path = "std::fs::rename", reason = "no host I/O in this crate" },
    { path = "std::fs::copy", reason = "no host I/O in this crate" },
    { path = "std::fs::hard_link", reason = "no host I/O in this crate" },
    { path = "std::fs::canonicalize", reason = "no host I/O in this crate" },
    { path = "std::fs::read_link", reason = "no host I/O in this crate" },
    { path = "std::fs::set_permissions", reason = "no host I/O in this crate" },
    { path = "std::env::var", reason = "no host environment in this crate: take the value as an argument" },
    { path = "std::env::var_os", reason = "no host environment in this crate: take the value as an argument" },
    { path = "std::env::vars", reason = "no host environment in this crate" },
    { path = "std::env::vars_os", reason = "no host environment in this crate" },
    { path = "std::env::args", reason = "no host environment in this crate" },
    { path = "std::env::args_os", reason = "no host environment in this crate" },
    { path = "std::env::current_dir", reason = "no host environment in this crate" },
    { path = "std::env::set_current_dir", reason = "no host environment in this crate" },
    { path = "std::env::current_exe", reason = "no host environment in this crate" },
    { path = "std::env::home_dir", reason = "no host environment in this crate" },
    { path = "std::env::temp_dir", reason = "no host environment in this crate" },
    { path = "std::env::set_var", reason = "no host environment in this crate" },
    { path = "std::env::remove_var", reason = "no host environment in this crate" },
    { path = "std::time::SystemTime::now", reason = "no host clock in this crate: time is the device's, counted in cycles" },
    { path = "std::time::Instant::now", reason = "no host clock in this crate: time is the device's, counted in cycles" },
    { path = "std::process::exit", reason = "no process control in this crate: return an error" },
    { path = "std::process::abort", reason = "no process control in this crate: return an error" },
    { path = "std::process::id", reason = "no process in this crate" },
    { path = "std::process::Command::new", reason = "no process in this crate" },
    { path = "std::net::ToSocketAddrs::to_socket_addrs", reason = "no network in this crate" },
    { path = "std::path::Path::exists", reason = "no host I/O in this crate: a path is text here, the front end looks at the file system" },
    { path = "std::path::Path::try_exists", reason = "no host I/O in this crate" },
    { path = "std::path::Path::is_file", reason = "no host I/O in this crate" },
    { path = "std::path::Path::is_dir", reason = "no host I/O in this crate" },
    { path = "std::path::Path::is_symlink", reason = "no host I/O in this crate" },
    { path = "std::path::Path::metadata", reason = "no host I/O in this crate" },
    { path = "std::path::Path::symlink_metadata", reason = "no host I/O in this crate" },
    { path = "std::path::Path::canonicalize", reason = "no host I/O in this crate" },
    { path = "std::path::Path::read_dir", reason = "no host I/O in this crate" },
    { path = "std::path::Path::read_link", reason = "no host I/O in this crate" },
    { path = "std::io::stdin", reason = "no console in this crate: the front end reads the user's input" },
    { path = "std::io::stdout", reason = "no console in this crate: return the text, the front end prints it" },
    { path = "std::io::stderr", reason = "no console in this crate: return the text, the front end prints it" },
    { path = "std::thread::spawn", reason = "no host threads in this crate: std on wasm32-unknown-unknown cannot start one" },
    { path = "std::thread::sleep", reason = "no host clock in this crate: time is the device's, counted in cycles" },
    { path = "std::thread::scope", reason = "no host threads in this crate: std on wasm32-unknown-unknown cannot start one" },
    { path = "std::thread::Builder::spawn", reason = "no host threads in this crate: std on wasm32-unknown-unknown cannot start one" },
]
disallowed-macros = [
    { path = "std::print", reason = "no console in this crate: return the text, the front end prints it" },
    { path = "std::println", reason = "no console in this crate: return the text, the front end prints it" },
    { path = "std::eprint", reason = "no console in this crate: return the text, the front end prints it" },
    { path = "std::eprintln", reason = "no console in this crate: return the text, the front end prints it" },
    { path = "std::dbg", reason = "no console in this crate: remove the debugging output" },
]
disallowed-types = [
    { path = "std::fs::File", reason = "no host I/O in this crate: take bytes, let the front end read files" },
    { path = "std::fs::OpenOptions", reason = "no host I/O in this crate" },
    { path = "std::fs::ReadDir", reason = "no host I/O in this crate" },
    { path = "std::fs::DirEntry", reason = "no host I/O in this crate" },
    { path = "std::time::SystemTime", reason = "no host clock in this crate: time is the device's, counted in cycles" },
    { path = "std::time::Instant", reason = "no host clock in this crate: time is the device's, counted in cycles" },
    { path = "std::process::Command", reason = "no process in this crate" },
    { path = "std::process::Child", reason = "no process in this crate" },
    { path = "std::net::TcpStream", reason = "no network in this crate" },
    { path = "std::net::TcpListener", reason = "no network in this crate" },
    { path = "std::net::UdpSocket", reason = "no network in this crate" },
]
```

Put this above the tests of `crates/console-script/src/parse.rs`:

```rust
//! The text of a console script, and the steps it is made of.
//!
//! One step per line, in order; `#` begins a comment; blank lines are ignored. The first line is
//! [`VERSION_LINE`]. Strings are between double quotes, with the escapes `\r`, `\n`, `\t`, `\\`,
//! `\"` and `\xHH`; every other character is sent as its UTF-8 bytes. Nothing is translated: Enter
//! is `\r`, as a terminal sends it (report UR §10). Every value read from the text is bounded by a
//! constant before anything is allocated for it.
use soc_esp32c3::{ConsoleChannel, UsbHost};
use std::fmt;

/// The first line of every console script: the format and its version.
pub const VERSION_LINE: &str = "inkemu-console-script 1";
/// The longest script file, in bytes: a front end reads no more.
pub const MAX_SCRIPT_BYTES: usize = 1 << 20;
/// The longest line, in bytes: a send of [`MAX_SEND_BYTES`] written as `\xHH` escapes fits.
pub const MAX_LINE_BYTES: usize = 8192;
/// The most steps a script holds.
pub const MAX_STEPS: usize = 10_000;
/// The most bytes one `send` holds, its escapes resolved.
pub const MAX_SEND_BYTES: usize = 1024;
const _: () = assert!(4 * MAX_SEND_BYTES + "send usb-serial-jtag \"\"".len() < MAX_LINE_BYTES);
/// The longest text a `wait-for` waits for, in bytes.
pub const MAX_TEXT_BYTES: usize = 256;
/// The most bytes all the sends of a script hold together: with [`MAX_STEPS`] zero-length
/// packets, less than the most a channel of the machine holds
/// ([`soc_esp32c3::MAX_PENDING_HOST_BYTES`]), so that no send of a script is refused for it.
pub const MAX_TOTAL_BYTES: usize = 512 * 1024;
const _: () = assert!(MAX_TOTAL_BYTES + MAX_STEPS < soc_esp32c3::MAX_PENDING_HOST_BYTES);
// Nor is a change of the USB host refused: a script holds fewer than the machine keeps.
const _: () = assert!(MAX_STEPS < soc_esp32c3::MAX_PENDING_HOST_CHANGES);

/// What one step does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// `send <channel> "<bytes>"`: the host sends the bytes at the time of the step.
    Send {
        channel: ConsoleChannel,
        bytes: Vec<u8>,
    },
    /// `wait <duration>`: the time of the next steps is this many nanoseconds later.
    Wait { nanos: u64 },
    /// `wait-for <channel> "<text>"`: the time of the next steps is the device time at which the
    /// last byte of the first occurrence of the text reached the host on the channel, searched in
    /// what the channel sent from the time of the step on.
    WaitFor {
        channel: ConsoleChannel,
        text: Vec<u8>,
    },
    /// `usb-host absent|attached|open`: the USB host is in that state from the time of the step.
    UsbHost(UsbHost),
}

/// A step and the line it was written on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub line: usize,
    pub action: Action,
}

/// A console script, read and checked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Script {
    steps: Vec<Step>,
}

/// A refusal of a script: where it is, then what is wrong and what to write instead.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("line {line}, column {column}: {kind}")]
pub struct ScriptError {
    pub line: usize,
    pub column: usize,
    pub kind: ScriptErrorKind,
}

/// Why a script is refused. Each message gives the cause, then the remedy.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ScriptErrorKind {
    #[error("the script is {len} bytes long, more than {MAX_SCRIPT_BYTES}; split it, or send less")]
    TooLong { len: usize },
    #[error(
        "the text is not UTF-8; save the script as UTF-8, and write other bytes as \\xHH between double quotes"
    )]
    NotUtf8,
    #[error("the first line is not `{VERSION_LINE}`; make it the first line of the script")]
    NoVersionLine,
    #[error(
        "the script is written for version `{found}` of the format, and inkemu reads version 1; write it for version 1"
    )]
    UnknownVersion { found: String },
    #[error("the line is {len} bytes long, more than {MAX_LINE_BYTES}; split the step in several")]
    LineTooLong { len: usize },
    #[error("the script has more than {MAX_STEPS} steps; split it in several scripts")]
    TooManySteps,
    #[error("`{word}` is no step; write send, wait, wait-for or usb-host")]
    UnknownStep { word: String },
    #[error("`{step}` needs {what}; write it after `{step}`")]
    Missing {
        step: &'static str,
        what: &'static str,
    },
    #[error("`{word}` is no channel; write uart0, uart1 or usb-serial-jtag")]
    UnknownChannel { word: String },
    #[error("`{word}` is no state of the USB host; write absent, attached or open")]
    UnknownHostState { word: String },
    #[error(
        "`{word}` is no duration; write a whole number and its unit, ns, us, ms or s, like 250ms"
    )]
    BadDuration { word: String },
    #[error(
        "`{word}` is more nanoseconds than device time can count (584 years); give a shorter wait"
    )]
    DurationTooLong { word: String },
    #[error("a string is expected here; write the bytes between double quotes")]
    NotAString,
    #[error("the string has no closing double quote; close it on the same line")]
    UnclosedString,
    #[error("`{escape}` is no escape; write \\r, \\n, \\t, \\\\, \\\" or \\x and two hex digits")]
    BadEscape { escape: String },
    #[error("the string holds more than {MAX_SEND_BYTES} bytes; split it in several sends")]
    SendTooLong,
    #[error("the text holds more than {MAX_TEXT_BYTES} bytes; wait for a shorter part of it")]
    TextTooLong,
    #[error("an empty text is found at once; give the text to wait for")]
    EmptyText,
    #[error(
        "the sends of the script hold more than {MAX_TOTAL_BYTES} bytes; send less, or split the script"
    )]
    TooManyBytes,
    #[error("`{text}` follows the step; remove it, or begin a comment with #")]
    Trailing { text: String },
}

impl Script {
    /// Reads a console script.
    ///
    /// # Errors
    /// The first thing wrong in the text, at its line and column ([`ScriptError`]).
    pub fn parse(bytes: &[u8]) -> Result<Script, ScriptError> {
        if bytes.len() > MAX_SCRIPT_BYTES {
            return Err(at(1, 1, ScriptErrorKind::TooLong { len: bytes.len() }));
        }
        let text = std::str::from_utf8(bytes).map_err(|e| {
            let (before, _) = bytes.split_at(e.valid_up_to());
            // INVARIANT: `before` is the valid UTF-8 prefix the error reports.
            let before = std::str::from_utf8(before).unwrap_or_default();
            let line = before.matches('\n').count() + 1;
            let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
            at(
                line,
                before[line_start..].chars().count() + 1,
                ScriptErrorKind::NotUtf8,
            )
        })?;
        let mut lines = text
            .split('\n')
            .enumerate()
            .map(|(index, line)| (index + 1, line.strip_suffix('\r').unwrap_or(line)));
        match lines.next() {
            Some((_, VERSION_LINE)) => {}
            Some((_, first)) => {
                let kind = match first.strip_prefix("inkemu-console-script ") {
                    Some(found) => ScriptErrorKind::UnknownVersion {
                        found: found.trim().to_owned(),
                    },
                    None => ScriptErrorKind::NoVersionLine,
                };
                return Err(at(1, 1, kind));
            }
            None => return Err(at(1, 1, ScriptErrorKind::NoVersionLine)),
        }
        let mut steps = Vec::new();
        let mut total = 0usize;
        for (number, line) in lines {
            if line.len() > MAX_LINE_BYTES {
                return Err(at(
                    number,
                    1,
                    ScriptErrorKind::LineTooLong { len: line.len() },
                ));
            }
            let mut cursor = Cursor {
                line,
                pos: 0,
                number,
            };
            let Some(action) = cursor.step()? else {
                continue;
            };
            if steps.len() == MAX_STEPS {
                return Err(at(number, 1, ScriptErrorKind::TooManySteps));
            }
            if let Action::Send { bytes, .. } = &action {
                total += bytes.len();
                if total > MAX_TOTAL_BYTES {
                    return Err(at(number, 1, ScriptErrorKind::TooManyBytes));
                }
            }
            steps.push(Step {
                line: number,
                action,
            });
        }
        Ok(Script { steps })
    }

    /// The steps, in the order they run.
    #[must_use]
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }
}

fn at(line: usize, column: usize, kind: ScriptErrorKind) -> ScriptError {
    ScriptError { line, column, kind }
}

/// Reads one line.
struct Cursor<'a> {
    line: &'a str,
    /// Byte offset of the next character.
    pos: usize,
    number: usize,
}

impl<'a> Cursor<'a> {
    /// The column of byte offset `pos`, counted in characters from 1.
    fn column(&self, pos: usize) -> usize {
        self.line[..pos].chars().count() + 1
    }

    fn refuse<T>(&self, pos: usize, kind: ScriptErrorKind) -> Result<T, ScriptError> {
        Err(at(self.number, self.column(pos), kind))
    }

    fn skip_spaces(&mut self) {
        let rest = &self.line[self.pos..];
        self.pos += rest.len() - rest.trim_start().len();
    }

    /// The next word and its offset, if the line has one before a comment.
    fn word(&mut self) -> Option<(usize, &'a str)> {
        self.skip_spaces();
        let line = self.line;
        let rest = &line[self.pos..];
        if rest.is_empty() || rest.starts_with('#') {
            return None;
        }
        let len = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let start = self.pos;
        self.pos += len;
        Some((start, &line[start..start + len]))
    }

    /// The step of the line, `None` for a blank or comment line.
    fn step(&mut self) -> Result<Option<Action>, ScriptError> {
        let Some((start, word)) = self.word() else {
            return Ok(None);
        };
        let action = match word {
            "send" => {
                let channel = self.channel("send")?;
                let bytes = self.string("send", MAX_SEND_BYTES, ScriptErrorKind::SendTooLong)?;
                Action::Send { channel, bytes }
            }
            "wait" => Action::Wait {
                nanos: self.duration()?,
            },
            "wait-for" => {
                let channel = self.channel("wait-for")?;
                self.skip_spaces();
                let string_at = self.pos;
                let text = self.string("wait-for", MAX_TEXT_BYTES, ScriptErrorKind::TextTooLong)?;
                if text.is_empty() {
                    return self.refuse(string_at, ScriptErrorKind::EmptyText);
                }
                Action::WaitFor { channel, text }
            }
            "usb-host" => Action::UsbHost(self.host_state()?),
            other => {
                return self.refuse(
                    start,
                    ScriptErrorKind::UnknownStep {
                        word: other.to_owned(),
                    },
                );
            }
        };
        self.skip_spaces();
        let rest = &self.line[self.pos..];
        if !rest.is_empty() && !rest.starts_with('#') {
            return self.refuse(
                self.pos,
                ScriptErrorKind::Trailing {
                    text: rest.to_owned(),
                },
            );
        }
        Ok(Some(action))
    }

    fn channel(&mut self, step: &'static str) -> Result<ConsoleChannel, ScriptError> {
        let Some((start, word)) = self.word() else {
            return self.refuse(
                self.pos,
                ScriptErrorKind::Missing {
                    step,
                    what: "a channel (uart0, uart1 or usb-serial-jtag)",
                },
            );
        };
        let found = ConsoleChannel::ALL
            .into_iter()
            .find(|channel| channel.name() == word);
        match found {
            Some(channel) => Ok(channel),
            None => self.refuse(
                start,
                ScriptErrorKind::UnknownChannel {
                    word: word.to_owned(),
                },
            ),
        }
    }

    fn host_state(&mut self) -> Result<UsbHost, ScriptError> {
        let Some((start, word)) = self.word() else {
            return self.refuse(
                self.pos,
                ScriptErrorKind::Missing {
                    step: "usb-host",
                    what: "a state (absent, attached or open)",
                },
            );
        };
        match UsbHost::ALL.into_iter().find(|state| state.name() == word) {
            Some(state) => Ok(state),
            None => self.refuse(
                start,
                ScriptErrorKind::UnknownHostState {
                    word: word.to_owned(),
                },
            ),
        }
    }

    /// `<digits><unit>`, in nanoseconds.
    fn duration(&mut self) -> Result<u64, ScriptError> {
        let Some((start, word)) = self.word() else {
            return self.refuse(
                self.pos,
                ScriptErrorKind::Missing {
                    step: "wait",
                    what: "a duration (a whole number and ns, us, ms or s)",
                },
            );
        };
        let digits = word.len() - word.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let (number, unit) = word.split_at(digits);
        let scale: u64 = match unit {
            "ns" => 1,
            "us" => 1_000,
            "ms" => 1_000_000,
            "s" => 1_000_000_000,
            _ => 0,
        };
        let bad = || ScriptErrorKind::BadDuration {
            word: word.to_owned(),
        };
        if number.is_empty() || scale == 0 {
            return self.refuse(start, bad());
        }
        // The digits are ASCII: the fold only overflows.
        number
            .bytes()
            .try_fold(0u64, |value, digit| {
                value.checked_mul(10)?.checked_add(u64::from(digit - b'0'))
            })
            .and_then(|value| value.checked_mul(scale))
            .map_or_else(
                || {
                    self.refuse(
                        start,
                        ScriptErrorKind::DurationTooLong {
                            word: word.to_owned(),
                        },
                    )
                },
                Ok,
            )
    }

    /// A string between double quotes, its escapes resolved, at most `limit` bytes.
    fn string(
        &mut self,
        step: &'static str,
        limit: usize,
        too_long: ScriptErrorKind,
    ) -> Result<Vec<u8>, ScriptError> {
        self.skip_spaces();
        let open = self.pos;
        let rest = &self.line[open..];
        if rest.is_empty() || rest.starts_with('#') {
            return self.refuse(
                open,
                ScriptErrorKind::Missing {
                    step,
                    what: "a string between double quotes",
                },
            );
        }
        if !rest.starts_with('"') {
            return self.refuse(open, ScriptErrorKind::NotAString);
        }
        let mut bytes = Vec::new();
        let mut chars = rest.char_indices().skip(1);
        loop {
            let Some((offset, c)) = chars.next() else {
                return self.refuse(open, ScriptErrorKind::UnclosedString);
            };
            let start = open + offset;
            match c {
                '"' => {
                    self.pos = start + 1;
                    return Ok(bytes);
                }
                '\\' => {
                    let escaped = match chars.next() {
                        Some((_, 'r')) => b'\r',
                        Some((_, 'n')) => b'\n',
                        Some((_, 't')) => b'\t',
                        Some((_, '\\')) => b'\\',
                        Some((_, '"')) => b'"',
                        Some((_, 'x')) => {
                            let high = chars.next().and_then(|(_, c)| c.to_digit(16));
                            let low = chars.next().and_then(|(_, c)| c.to_digit(16));
                            match high
                                .zip(low)
                                .and_then(|(h, l)| u8::try_from(h * 16 + l).ok())
                            {
                                Some(byte) => byte,
                                None => return self.bad_escape(start),
                            }
                        }
                        _ => return self.bad_escape(start),
                    };
                    bytes.push(escaped);
                }
                other => {
                    let mut buffer = [0; 4];
                    bytes.extend_from_slice(other.encode_utf8(&mut buffer).as_bytes());
                }
            }
            if bytes.len() > limit {
                return self.refuse(open, too_long);
            }
        }
    }

    /// Refuses the escape that begins at byte offset `start`: its backslash and what follows it,
    /// at most three characters.
    fn bad_escape<T>(&self, start: usize) -> Result<T, ScriptError> {
        let escape: String = self.line[start..].chars().take(4).collect();
        self.refuse(start, ScriptErrorKind::BadEscape { escape })
    }
}

impl fmt::Display for Step {
    /// The step as it is written in a script, for a front end that says where a script stopped.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.action {
            Action::Send { channel, bytes } => {
                write!(f, "send {} \"{}\"", channel.name(), quoted(bytes))
            }
            Action::Wait { nanos } => write!(f, "wait {nanos}ns"),
            Action::WaitFor { channel, text } => {
                write!(f, "wait-for {} \"{}\"", channel.name(), quoted(text))
            }
            Action::UsbHost(state) => write!(f, "usb-host {}", state.name()),
        }
    }
}

/// `bytes` as a script writes them between double quotes.
fn quoted(bytes: &[u8]) -> String {
    let mut text = String::new();
    for &byte in bytes {
        match byte {
            b'\r' => text.push_str("\\r"),
            b'\n' => text.push_str("\\n"),
            b'\t' => text.push_str("\\t"),
            b'\\' => text.push_str("\\\\"),
            b'"' => text.push_str("\\\""),
            0x20..=0x7E => text.push(char::from(byte)),
            other => {
                text.push_str("\\x");
                for nibble in [other >> 4, other & 0xF] {
                    // INVARIANT: a nibble is a hex digit: the fallback is never taken.
                    text.push(char::from_digit(u32::from(nibble), 16).unwrap_or('0'));
                }
            }
        }
    }
    text
}
```

Put this above the tests of `crates/console-script/src/run.rs`:

```rust
//! A console script while a machine runs: its steps become dated inputs of the machine, and its
//! waits read what the consoles of the machine send (ADR 0006).
//!
//! The time of a step is fixed by the script and the run alone. A `wait-for` ends at the device
//! time at which the last byte of its text reached the host: the channels the script waits on are
//! watched ([`Esp32C3::watch_console`]), so each byte they send ends a call of the machine at the
//! instruction boundary it was sent at, and the bytes a caller takes after a call reached the
//! host at [`Esp32C3::device_time`]. The steps that follow are dated from that instant and handed
//! over at once, before the machine runs on. However a front end cuts the run into calls, the
//! machine gets the same inputs at the same dates.
use crate::parse::{Action, Script, Step};
use emu_core::Cycles;
use soc_esp32c3::clock::TIME_BASE;
use soc_esp32c3::{ConsoleChannel, Esp32C3, InputError};

const NANOS_PER_SECOND: u128 = 1_000_000_000;

/// A duration of `nanos` nanoseconds in cycles of the time base, rounded up: a wait never ends
/// before the time asked.
fn cycles_of(nanos: u64) -> u64 {
    let cycles = (u128::from(nanos) * u128::from(TIME_BASE.hz())).div_ceil(NANOS_PER_SECOND);
    // INVARIANT: a second has more nanoseconds than cycles of the time base, so the quotient is
    // at most `nanos`: the fallback, that same bound, is never taken.
    u64::try_from(cycles).unwrap_or(nanos)
}

/// An input the machine refused, and the line of the step that gave it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("console script, line {line}: {source}")]
pub struct RunError {
    pub line: usize,
    #[source]
    pub source: InputError,
}

/// How far a script got.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptStatus {
    /// Every step was handed to the machine, and the run reached the time of the last one.
    Complete,
    /// The run waits for the text of this `wait-for`.
    WaitingFor { step: Step },
    /// Every step was handed to the machine, but this one is dated at `at`, which the run has not
    /// reached.
    Ahead { step: Step, at: Cycles },
}

/// A console script handed to a machine as it runs.
#[derive(Debug)]
pub struct ScriptRun {
    steps: Vec<Step>,
    /// The next step to hand to the machine.
    next: usize,
    /// The device time of the next step.
    step_time: Cycles,
    /// What the channel of the current `wait-for` sent from the step time on: the last bytes,
    /// as many as its text has at most.
    seen: Vec<u8>,
    /// The step handed over last that is dated, and its date.
    last_dated: Option<(usize, Cycles)>,
    /// The channels the script waits on and still watches.
    watched: Vec<ConsoleChannel>,
}

impl ScriptRun {
    /// Starts `script` on `machine` at its device time: watches every channel a `wait-for` of the
    /// script names, and hands over the steps before the first `wait-for`.
    ///
    /// # Errors
    /// [`RunError`] when the machine refuses an input; a script that [`Script::parse`] accepted
    /// stays within the bounds of the machine, so this does not happen from a script file.
    pub fn start(script: &Script, machine: &mut Esp32C3) -> Result<ScriptRun, RunError> {
        let mut watched = Vec::new();
        for step in script.steps() {
            if let Action::WaitFor { channel, .. } = step.action
                && !watched.contains(&channel)
            {
                watched.push(channel);
                machine.watch_console(channel, true);
            }
        }
        let mut run = ScriptRun {
            steps: script.steps().to_vec(),
            next: 0,
            step_time: machine.device_time(),
            seen: Vec::new(),
            last_dated: None,
            watched,
        };
        run.advance(machine)?;
        Ok(run)
    }

    /// The machine handed `taken` to the host since the last call, on each channel, at its device
    /// time now: the bytes a front end took from every channel after a call of
    /// [`Esp32C3::run_within`]. The current `wait-for` reads those of its channel; when its text
    /// is complete, the steps that follow are dated from now and handed over, and the next
    /// `wait-for` reads what remains of the same instant on its own channel.
    ///
    /// # Errors
    /// [`RunError`] as [`ScriptRun::start`] says.
    pub fn observe(
        &mut self,
        machine: &mut Esp32C3,
        taken: &[(ConsoleChannel, Vec<u8>)],
    ) -> Result<(), RunError> {
        let now = machine.device_time();
        if now < self.step_time {
            // Sent before the time of the current step: not searched.
            return Ok(());
        }
        let mut rest: Vec<(ConsoleChannel, &[u8])> = taken
            .iter()
            .map(|(channel, bytes)| (*channel, bytes.as_slice()))
            .collect();
        // INVARIANT: each pass consumes a byte or a step, or ends the loop.
        while let Some((channel, text)) = self.waiting_for() {
            let Some(entry) = rest
                .iter_mut()
                .find(|(c, bytes)| *c == channel && !bytes.is_empty())
            else {
                break;
            };
            let Some((&byte, after)) = entry.1.split_first() else {
                break;
            };
            entry.1 = after;
            self.seen.push(byte);
            if self.seen.len() > text.len() {
                self.seen.remove(0);
            }
            if self.seen == text {
                self.seen.clear();
                self.step_time = now;
                self.last_dated = Some((self.next, now));
                self.next += 1;
                self.advance(machine)?;
            }
        }
        Ok(())
    }

    /// The channel and the text of the current `wait-for`.
    fn waiting_for(&self) -> Option<(ConsoleChannel, Vec<u8>)> {
        match &self.steps.get(self.next)?.action {
            Action::WaitFor { channel, text } => Some((*channel, text.clone())),
            _ => None,
        }
    }

    /// Hands the machine every step up to the next `wait-for`.
    fn advance(&mut self, machine: &mut Esp32C3) -> Result<(), RunError> {
        while let Some(step) = self.steps.get(self.next) {
            let refused = |source| RunError {
                line: step.line,
                source,
            };
            match &step.action {
                Action::Send { channel, bytes } => machine
                    .send_console(*channel, self.step_time, bytes)
                    .map_err(refused)?,
                Action::UsbHost(state) => machine
                    .set_usb_host(self.step_time, *state)
                    .map_err(refused)?,
                Action::Wait { nanos } => {
                    self.step_time = self.step_time + cycles_of(*nanos);
                }
                Action::WaitFor { .. } => return Ok(()),
            }
            self.last_dated = Some((self.next, self.step_time));
            self.next += 1;
        }
        // Nothing is waited for any more: the consoles go back to their pace.
        for channel in self.watched.drain(..) {
            machine.watch_console(channel, false);
        }
        Ok(())
    }

    /// How many steps the script has.
    #[must_use]
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    /// How far the script got, the run being at device time `now`.
    #[must_use]
    pub fn status(&self, now: Cycles) -> ScriptStatus {
        if let Some(step) = self.steps.get(self.next) {
            return ScriptStatus::WaitingFor { step: step.clone() };
        }
        match self.last_dated {
            Some((index, at)) if at > now => ScriptStatus::Ahead {
                step: self.steps[index].clone(),
                at,
            },
            _ => ScriptStatus::Complete,
        }
    }
}
```

```diff
--- a/crates/firmware/clippy.toml
+++ b/crates/firmware/clippy.toml
@@ -5,11 +5,11 @@
 allow-unwrap-in-tests = true
 allow-expect-in-tests = true
 
-# firmware, boards and soc-esp32c3 do no host I/O: a graphical front end and the WebAssembly build
-# use them, and std on wasm32-unknown-unknown compiles these calls only to fail at run time. Files,
-# the environment, the clock, processes, threads, the console and the network belong to the front end
-# (emu-cli). Test code that reads a fixture allows it on the item that does
-# (`#[allow(clippy::disallowed_methods)]`). `crates/boards/tests/host_io_ban.rs` holds the three
+# firmware, boards, soc-esp32c3 and console-script do no host I/O: a graphical front end and the
+# WebAssembly build use them, and std on wasm32-unknown-unknown compiles these calls only to fail at
+# run time. Files, the environment, the clock, processes, threads, the console and the network belong
+# to the front end (emu-cli). Test code that reads a fixture allows it on the item that does
+# (`#[allow(clippy::disallowed_methods)]`). `crates/boards/tests/host_io_ban.rs` holds the four
 # copies of this file equal and the list below complete: change them together.
 disallowed-methods = [
     { path = "std::fs::read", reason = "no host I/O in this crate: take bytes, let the front end read files" },
```

```diff
--- a/crates/soc-esp32c3/clippy.toml
+++ b/crates/soc-esp32c3/clippy.toml
@@ -5,11 +5,11 @@
 allow-unwrap-in-tests = true
 allow-expect-in-tests = true
 
-# firmware, boards and soc-esp32c3 do no host I/O: a graphical front end and the WebAssembly build
-# use them, and std on wasm32-unknown-unknown compiles these calls only to fail at run time. Files,
-# the environment, the clock, processes, threads, the console and the network belong to the front end
-# (emu-cli). Test code that reads a fixture allows it on the item that does
-# (`#[allow(clippy::disallowed_methods)]`). `crates/boards/tests/host_io_ban.rs` holds the three
+# firmware, boards, soc-esp32c3 and console-script do no host I/O: a graphical front end and the
+# WebAssembly build use them, and std on wasm32-unknown-unknown compiles these calls only to fail at
+# run time. Files, the environment, the clock, processes, threads, the console and the network belong
+# to the front end (emu-cli). Test code that reads a fixture allows it on the item that does
+# (`#[allow(clippy::disallowed_methods)]`). `crates/boards/tests/host_io_ban.rs` holds the four
 # copies of this file equal and the list below complete: change them together.
 disallowed-methods = [
     { path = "std::fs::read", reason = "no host I/O in this crate: take bytes, let the front end read files" },
```

```diff
--- a/tools/ci/clippy.sh
+++ b/tools/ci/clippy.sh
@@ -25,6 +25,6 @@
   exit 1
 fi
 if grep -q 'does not refer to' "$log"; then
-  echo "error: a path banned by a clippy.toml names nothing (the lines that say \"does not refer to\" above), so the call it was meant to refuse compiles; correct the path in the three clippy.toml and in crates/boards/tests/host_io_ban.rs" >&2
+  echo "error: a path banned by a clippy.toml names nothing (the lines that say \"does not refer to\" above), so the call it was meant to refuse compiles; correct the path in every clippy.toml that bans host I/O and in crates/boards/tests/host_io_ban.rs" >&2
   exit 1
 fi
```

`crates/console-script/clippy.toml` holds the same bytes as the three other copies once their comment is changed; `the_copies_of_the_ban_are_the_same_file` holds the four equal.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p console-script > /tmp/t7.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t7.log
cargo test -p boards --test host_io_ban > /tmp/t7-ban.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t7-ban.log
cargo check --target wasm32-unknown-unknown -p console-script > /tmp/t7-wasm.log 2>&1; echo "exit=$?"
```

Expected: `exit=0` and `test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (the doc-tests line after it runs none); then `exit=0` and `3 passed`; then `exit=0`.

- [ ] **Step 5: Mutations**

| # | Mutation | Test that fails |
|---|---|---|
| 1 | `run.rs`: delete `if now < self.step_time { return Ok(()); }` (with its comment) in `observe` | `the_text_of_a_wait_for_is_searched_only_from_the_time_of_its_step` |
| 2 | `run.rs`: delete `self.step_time = now;` in `observe` | `a_wait_for_ends_at_the_instant_the_last_byte_of_its_text_reached_the_host`, `the_steps_are_dated_the_same_whatever_the_calls_the_run_is_cut_into`, `a_wait_for_dates_the_next_steps_…` |
| 3 | `run.rs`: delete `if self.seen.len() > text.len() { self.seen.remove(0); }` | `a_text_found_where_its_bytes_cross_the_calls_and_after_a_false_start_is_found` and two others |
| 4 | `run.rs`: `Some((index, at)) if at > now =>` → `Some((index, at)) if at > now && false =>` | `a_run_that_ends_before_the_date_of_its_last_step_says_so` |
| 5 | `run.rs`: `.div_ceil(NANOS_PER_SECOND)` → `/ NANOS_PER_SECOND` | `a_wait_is_rounded_up_to_a_cycle_of_the_time_base` |
| 6 | `run.rs`: delete `machine.watch_console(channel, true);` in `start` | `a_wait_for_ends_at_the_instant_the_last_byte_of_its_text_reached_the_host` and two others |
| 7 | `parse.rs`: `if bytes.len() > limit {` → `if bytes.len() > limit + 1 {` | `each_limit_is_held` |
| 8 | `parse.rs`: `if total > MAX_TOTAL_BYTES {` → `if total > MAX_TOTAL_BYTES + 1 {` | `each_limit_is_held` |
| 9 | `parse.rs`: `if steps.len() == MAX_STEPS {` → `if steps.len() == MAX_STEPS + 1 {` | `each_limit_is_held` |

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy -p console-script -p boards --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/console-script
/usr/bin/git commit -m "feat(console-script): a console script is read with every refusal at its line and column, and handed to a machine as dated inputs while it runs" -- Cargo.toml Cargo.lock crates/console-script crates/boards/tests/host_io_ban.rs crates/firmware/clippy.toml crates/boards/clippy.toml crates/soc-esp32c3/clippy.toml tools/ci/clippy.sh .github/workflows/ci.yml
```

Expected: three `exit=0`, then the commit.

---

## Task 8: `inkemu run --console-script` and `--usb-host`

The command line (C7, C25, C26). `--console-script <FILE>` is read before the ROM, with the bounded reader of SP1b-3c (`flash_file::read_regular_file`, limit `MAX_SCRIPT_BYTES`), and parsed by `console-script`; a refusal exits 2 with the file, the line and the column. `--usb-host absent|attached|open` sets `MachineOptions::usb_host`. The run loop becomes `execute_with_script`: after each call of the machine it shows what every console sent to the script; `execute` is that loop without a script. The summary prints `inkemu: USB host <state>` and, with a script, how far it got; `exit_code_with_script` turns an exit 0 into 5 when the script is not through, and keeps every other code. `run_device` loses its ROM loading to a function of its own (`load_rom`), which keeps it within the lines clippy allows.

**Files:**
- Create: `crates/emu-cli/src/script_file.rs`
- Modify: `crates/boards/src/lib.rs`, `crates/emu-cli/Cargo.toml`, `crates/emu-cli/src/{exit,lib,main,run}.rs`, `crates/emu-cli/tests/run_cli.rs`, `README.md` (the run help and the exit code), `.github/workflows/ci.yml` (`rom-boot` runs 27 tests of `run_cli`, two more)

**Interfaces:**
- Consumes: Task 7; `flash_file::{ReadFailure, read_regular_file}`, `term::sanitize`.
- Produces: `boards::MachineOptions::usb_host: UsbHost`; `pub const EXIT_SCRIPT_INCOMPLETE: u8 = 5`; `pub fn emu_cli::run::execute_with_script(machine: &mut Esp32C3, options: &RunOptions, script: Option<&mut ScriptRun>, interrupt: &Interrupt, reader_gone: &dyn Fn() -> bool, console: &mut dyn Write, log: &mut dyn Write) -> RunEnd`; `pub fn exit_code_with_script(code: u8, script: Option<&ScriptStatus>) -> u8`; `pub enum emu_cli::script_file::ScriptFileError`; `pub fn read_script(path: &Path) -> Result<Script, ScriptFileError>`.

**Bounds on untrusted input.** The script file: at most `MAX_SCRIPT_BYTES`, judged on its metadata before it is read, and never more than one byte past it (the bounded reader).

- [ ] **Step 1: Write the failing tests**

```diff
--- a/crates/boards/src/lib.rs
+++ b/crates/boards/src/lib.rs
@@ -1049,10 +1049,12 @@
         let config = profile.soc_config(MachineOptions {
             strict_mmio: true,
             trace_hash: true,
+            usb_host: UsbHost::Attached,
         });
         assert_eq!(config.chip_revision.to_string(), "v1.1");
         assert_eq!(config.flash, profile.flash);
         assert!(config.strict_mmio && config.trace_hash);
+        assert_eq!(config.usb_host, UsbHost::Attached);
         assert_eq!(
             profile.soc_config(MachineOptions::default()),
             SocConfig {
```

```diff
--- a/crates/emu-cli/src/exit.rs
+++ b/crates/emu-cli/src/exit.rs
@@ -44,7 +44,13 @@
         let codes = RunEnd::ALL
             .iter()
             .map(|end| (format!("{end:?}"), end.exit_code()))
-            .chain([("a second Ctrl-C".to_owned(), EXIT_SECOND_INTERRUPT)]);
+            .chain([
+                ("a second Ctrl-C".to_owned(), EXIT_SECOND_INTERRUPT),
+                (
+                    "a console script not through".to_owned(),
+                    EXIT_SCRIPT_INCOMPLETE,
+                ),
+            ]);
         for (end, code) in codes {
             assert!(
                 format!("\n{RUN_EXIT_CODES_HELP}").contains(&format!("\n{code:>3}  ")),
```

```diff
--- a/crates/emu-cli/src/run.rs
+++ b/crates/emu-cli/src/run.rs
@@ -532,7 +532,9 @@
 mod tests {
     use super::*;
     use boards::MachineOptions;
-    use cpu_rv32::asm::{addi, bne, jal, lui, sw, wfi};
+    use console_script::{Script, VERSION_LINE};
+    use cpu_rv32::asm::{addi, beq, bne, jal, lbu, lui, lw, sw, wfi};
+    use soc_esp32c3::UsbHost;
     use soc_esp32c3::test_support::{
         HANG, T0, T1, flat_rom, load_register, print_program, stop_watchdogs, store_register,
         unmodeled_register, within,
@@ -561,6 +563,7 @@
     const STRICT: MachineOptions = MachineOptions {
         strict_mmio: true,
         trace_hash: false,
+        usb_host: UsbHost::Open,
     };
     static NOT_INTERRUPTED: Interrupt = Interrupt::new();
     /// Ctrl-C pressed once before the run starts.
@@ -641,6 +644,134 @@
             &mut log,
         );
         log.lines
+    }
+
+    /// Prints `ready` on UART0, then echoes what UART0 receives.
+    fn ready_then_echo() -> Vec<u32> {
+        printing(
+            UART0,
+            b"ready",
+            &[
+                lbu(T1, T0, 0x1C),
+                beq(T1, 0, -4),
+                lw(T1, T0, 0),
+                sw(T1, T0, 0),
+                jal(0, -16),
+            ],
+        )
+    }
+
+    /// Runs `program` with the console script `body` to 20 ms of device time: the console, the
+    /// log, and the exit code of the run.
+    fn scripted(program: &[u32], body: &str) -> (String, String, u8) {
+        let mut m = machine(program, MachineOptions::default());
+        let script = Script::parse(format!("{VERSION_LINE}\n{body}").as_bytes()).unwrap();
+        let mut run = ScriptRun::start(&script, &mut m).unwrap();
+        let (mut console, mut log) = (Vec::new(), Vec::new());
+        let options = RunOptions {
+            max_device_nanos: Some(20_000_000),
+            ..UNLIMITED
+        };
+        let end = execute_with_script(
+            &mut m,
+            &options,
+            Some(&mut run),
+            &NOT_INTERRUPTED,
+            &reader_stays,
+            &mut console,
+            &mut log,
+        );
+        let status = run.status(m.device_time());
+        let code = exit_code_with_script(end.exit_code(), Some(&status));
+        (text(&console), text(&log), code)
+    }
+
+    #[test]
+    fn a_console_script_reads_the_consoles_and_its_summary_line_says_it_went_through() {
+        let (console, log, code) = scripted(
+            &ready_then_echo(),
+            "wait-for uart0 \"ready\"\nsend uart0 \"ok\\r\"\n",
+        );
+        assert_eq!(console, "readyok\r");
+        assert!(
+            log.contains("inkemu: USB host open\ninkemu: console script through (2 steps)\n"),
+            "{log}"
+        );
+        assert_eq!(code, EXIT_OK);
+    }
+
+    #[test]
+    fn a_console_script_not_through_turns_an_exit_0_into_5_and_says_where_it_stopped() {
+        let (_, log, code) = scripted(&ready_then_echo(), "wait-for uart0 \"never\"\n");
+        assert!(
+            log.contains(
+                "inkemu: console script not through: line 2 (wait-for uart0 \"never\") was still waiting; raise --max-time or --max-cycles, or check that the firmware prints that text on that console\n"
+            ),
+            "{log}"
+        );
+        assert_eq!(code, EXIT_SCRIPT_INCOMPLETE);
+        let (_, log, code) = scripted(&ready_then_echo(), "wait 1s\nsend uart0 \"late\"\n");
+        assert!(
+            log.contains(
+                "inkemu: console script not through: line 3 (send uart0 \"late\") is dated at 1000000 us of device time, after the end of the run; raise --max-time or --max-cycles\n"
+            ),
+            "{log}"
+        );
+        assert_eq!(code, EXIT_SCRIPT_INCOMPLETE);
+    }
+
+    #[test]
+    fn a_script_not_through_changes_no_exit_code_but_0() {
+        let waiting = ScriptStatus::Ahead {
+            step: console_script::Step {
+                line: 2,
+                action: console_script::Action::Wait { nanos: 1 },
+            },
+            at: Cycles(1),
+        };
+        for end in RunEnd::ALL {
+            let code = end.exit_code();
+            let expected = if code == EXIT_OK {
+                EXIT_SCRIPT_INCOMPLETE
+            } else {
+                code
+            };
+            assert_eq!(
+                exit_code_with_script(code, Some(&waiting)),
+                expected,
+                "{end:?}"
+            );
+            assert_eq!(
+                exit_code_with_script(code, Some(&ScriptStatus::Complete)),
+                code
+            );
+            assert_eq!(exit_code_with_script(code, None), code);
+        }
+    }
+
+    #[test]
+    fn the_summary_says_the_usb_host_at_the_end_of_the_run() {
+        let mut m = machine(
+            &[HANG],
+            MachineOptions {
+                usb_host: UsbHost::Attached,
+                ..MachineOptions::default()
+            },
+        );
+        let mut log = Vec::new();
+        execute(
+            &mut m,
+            &options(1_000),
+            &NOT_INTERRUPTED,
+            &reader_stays,
+            &mut Vec::new(),
+            &mut log,
+        );
+        assert!(
+            text(&log).contains("inkemu: USB host attached\n"),
+            "{}",
+            text(&log)
+        );
     }
 
     #[test]
@@ -1048,6 +1179,7 @@
                 MachineOptions {
                     strict_mmio: false,
                     trace_hash: true,
+                    usb_host: UsbHost::Open,
                 },
             );
             let (mut console, mut log) = (Vec::new(), Vec::new());
@@ -1556,6 +1688,7 @@
                 MachineOptions {
                     strict_mmio: false,
                     trace_hash: true,
+                    usb_host: UsbHost::Open,
                 },
             );
             let (mut console, mut log) = (Vec::new(), Vec::new());
@@ -2043,6 +2176,7 @@
         const TRACED: MachineOptions = MachineOptions {
             strict_mmio: false,
             trace_hash: true,
+            usb_host: UsbHost::Open,
         };
         let program = printing(UART0, b"x", &[HANG]);
         let hash_line = |log: &[u8]| {
```

Create `crates/emu-cli/src/script_file.rs` with its tests only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use console_script::VERSION_LINE;

    #[test]
    fn a_script_file_is_read_and_checked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("steps.txt");
        std::fs::write(&path, format!("{VERSION_LINE}\nsend uart0 \"x\"\n")).unwrap();
        assert_eq!(read_script(&path).unwrap().steps().len(), 1);
    }

    #[test]
    fn each_refusal_names_the_file_and_states_its_remedy() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.txt");
        std::fs::write(&bad, format!("{VERSION_LINE}\nsend uart9 \"x\"\n")).unwrap();
        let refused = read_script(&bad).unwrap_err().to_string();
        assert!(
            refused.ends_with(
                "bad.txt, line 2, column 6: `uart9` is no channel; write uart0, uart1 or usb-serial-jtag"
            ),
            "{refused}"
        );
        let long = dir.path().join("long.txt");
        std::fs::File::create(&long)
            .unwrap()
            .set_len(u64::try_from(MAX_SCRIPT_BYTES).unwrap() + 1)
            .unwrap();
        let too_long = read_script(&long).unwrap_err().to_string();
        assert!(
            too_long.contains("is 1048577 bytes long, more than 1048576; "),
            "{too_long}"
        );
        let directory = read_script(dir.path()).unwrap_err().to_string();
        assert!(directory.contains("is not a regular file; "), "{directory}");
        let missing = read_script(&dir.path().join("missing.txt"))
            .unwrap_err()
            .to_string();
        assert!(
            missing.starts_with("cannot read the console script "),
            "{missing}"
        );
        assert!(
            missing.ends_with("; check the path and its read permissions"),
            "{missing}"
        );
    }
}
```

```diff
--- a/crates/emu-cli/tests/run_cli.rs
+++ b/crates/emu-cli/tests/run_cli.rs
@@ -180,6 +180,69 @@
                 .and(contains("uart1"))
                 .and(contains("usb-serial-jtag")),
         );
+}
+
+#[test]
+fn a_usb_host_state_the_port_does_not_have_exits_2_and_lists_the_three_it_has() {
+    inkemu()
+        .args(["run", "--device", "x4", "--usb-host", "closed"])
+        .assert()
+        .code(2)
+        .stderr(
+            contains("'--usb-host <USB_HOST>'")
+                .and(contains("absent"))
+                .and(contains("attached"))
+                .and(contains("open")),
+        );
+}
+
+/// A console script with `body` after its version line, in a directory that lives as long as
+/// the value.
+fn console_script(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
+    let dir = tempfile::tempdir().unwrap();
+    let path = dir.path().join("steps.txt");
+    std::fs::write(&path, format!("inkemu-console-script 1\n{body}")).unwrap();
+    (dir, path)
+}
+
+#[test]
+fn a_refused_console_script_exits_2_at_its_line_and_column_before_any_rom_is_read() {
+    let (dir, script) = console_script("wait 3\n");
+    let empty_rom_dir = dir.path().join("roms");
+    std::fs::create_dir(&empty_rom_dir).unwrap();
+    inkemu()
+        .args(["run", "--device", "x4", "--rom-dir"])
+        .arg(&empty_rom_dir)
+        .arg("--console-script")
+        .arg(&script)
+        .assert()
+        .code(2)
+        .stderr(
+            contains("steps.txt, line 2, column 6: `3` is no duration; write a whole number")
+                .and(contains("rom").not()),
+        );
+    inkemu()
+        .args(["run", "--device", "x4", "--rom-dir"])
+        .arg(&empty_rom_dir)
+        .arg("--console-script")
+        .arg(dir.path())
+        .assert()
+        .code(2)
+        .stderr(contains(
+            "is not a regular file; give the path of the file itself",
+        ));
+}
+
+#[test]
+fn run_help_documents_the_console_script_and_the_usb_host() {
+    inkemu().args(["run", "--help"]).assert().code(0).stdout(
+        contains("--console-script <FILE>")
+            .and(contains("inkemu-console-script 1"))
+            .and(contains("wait-for CHANNEL"))
+            .and(contains("--usb-host <USB_HOST>"))
+            .and(contains("[default: open]"))
+            .and(contains("\n  5  ")),
+    );
 }
 
 #[test]
@@ -838,6 +901,75 @@
     assert_eq!(
         trace_hash_line(&log),
         trace_hash_line(&String::from_utf8_lossy(&second.stderr))
+    );
+}
+
+/// A second stage that stops the watchdogs, prints `ready` on UART0, then echoes on UART0 what
+/// it receives, polling `STATUS.RXFIFO_CNT`.
+fn ready_then_echo() -> Vec<u32> {
+    use cpu_rv32::asm::{beq, jal, lbu, lw, sw};
+    use soc_esp32c3::test_support::{T0, T1, print_program};
+    let uart0 = svd_regs::esp32c3::addr("UART0", "FIFO");
+    let mut program = stop_watchdogs();
+    program.extend(print_program(uart0, b"ready"));
+    program.extend([
+        lbu(T1, T0, 0x1C),
+        beq(T1, 0, -4),
+        lw(T1, T0, 0),
+        sw(T1, T0, 0),
+        jal(0, -16),
+    ]);
+    program
+}
+
+/// The run of the echo firmware with the console script `body`, to 50 ms of device time.
+fn scripted_run(rom_dir: &str, body: &str) -> std::process::Output {
+    let (_flash_dir, flash) = flash_image_of(&ready_then_echo());
+    let (_script_dir, script) = console_script(body);
+    inkemu()
+        .args([
+            "run",
+            "--device",
+            "x4",
+            "--max-time",
+            "0.05",
+            "--rom-dir",
+            rom_dir,
+        ])
+        .arg("--flash")
+        .arg(&flash)
+        .arg("--console-script")
+        .arg(&script)
+        .timeout(HANG_LIMIT)
+        .output()
+        .unwrap()
+}
+
+#[test]
+#[ignore = "needs INKEMU_ROM_ELF_DIR"]
+fn a_console_script_types_into_the_firmware_after_its_prompt() {
+    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
+    let output = scripted_run(&rom_dir, "wait-for uart0 \"ready\"\nsend uart0 \"hi\\r\"\n");
+    let log = String::from_utf8_lossy(&output.stderr);
+    assert_eq!(output.status.code(), Some(0), "{log}");
+    let console = String::from_utf8_lossy(&output.stdout);
+    assert!(console.ends_with("readyhi\r"), "{console:?}");
+    assert!(
+        log.contains("inkemu: USB host open\ninkemu: console script through (2 steps)\n"),
+        "{log}"
+    );
+}
+
+#[test]
+#[ignore = "needs INKEMU_ROM_ELF_DIR"]
+fn a_console_script_still_waiting_when_the_run_ends_exits_5() {
+    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
+    let output = scripted_run(&rom_dir, "wait-for uart0 \"login:\"\n");
+    let log = String::from_utf8_lossy(&output.stderr);
+    assert_eq!(output.status.code(), Some(5), "{log}");
+    assert!(
+        log.contains("inkemu: console script not through: line 2 (wait-for uart0 \"login:\") was still waiting; "),
+        "{log}"
     );
 }
 
```

- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p emu-cli --lib > /tmp/t8-red.log 2>&1; echo "exit=$?"; grep -E "^error" /tmp/t8-red.log | sort | uniq -c | head
```

Expected: `exit=101`; `could not compile emu-cli (lib test) due to 20 previous errors`: `error[E0560]: struct MachineOptions has no field named usb_host`, `cannot find value EXIT_SCRIPT_INCOMPLETE`, `cannot find function exit_code_with_script` and `execute_with_script`, `unresolved import console_script` (not a dependency yet).

- [ ] **Step 3: The options, the summary and exit code 5**

```diff
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -95,7 +95,7 @@
         # un-ignored test would otherwise leave this job green with nothing proven.
         run: |
           tools/ci/run-ignored-tests.sh 18 --release -p soc-esp32c3 --test real_rom -- --ignored
-          tools/ci/run-ignored-tests.sh 25 --release -p emu-cli --test run_cli -- --ignored --skip reference_firmware
+          tools/ci/run-ignored-tests.sh 27 --release -p emu-cli --test run_cli -- --ignored --skip reference_firmware
   reference-boot:
     # Boots the reference firmware (stock ESP-IDF bootloader + hello_world) from the real mask ROM
     # and runs it through a whole life: the application, its countdown, the restart it asks for
```

```diff
--- a/README.md
+++ b/README.md
@@ -163,6 +163,8 @@
       --max-time <SECONDS>       Stop when this many seconds of device time have elapsed since power-on (a decimal number: 30, 2.5, 0.001). Device time goes on while the CPU sleeps and across resets
       --strict-mmio              Stop at the first access to a register the emulator does not model, 8/16-bit write to a peripheral register, operation the emulator cannot perform faithfully, access to flash that is not mapped, or read through a flash window that may differ from the device (exit code 3)
       --console <CONSOLE>        Which console of the device goes to standard output [default: uart0] [possible values: uart0, uart1, usb-serial-jtag]
+      --console-script <FILE>    What the host sends to the consoles, and when: a text file whose first line is `inkemu-console-script 1`, then one step per line: `send CHANNEL "BYTES"`, `wait 250ms`, `wait-for CHANNEL "TEXT"` (the next steps are dated from the device time the text reached the host), `usb-host absent|attached|open`. CHANNEL is uart0, uart1 or usb-serial-jtag; strings take \r \n \t \\ \" \xHH, and Enter is \r. Every step is dated in device time, so two runs with the same script are the same run. A script not through when the run ends makes it exit 5
+      --usb-host <USB_HOST>      The USB host on the USB-Serial-JTAG port when the device starts: absent (no cable), attached (the port closed: frames, nothing read) or open (a program reads and writes the port) [default: open] [possible values: absent, attached, open]
       --rom-dir <ROM_DIR>        Use the ROM ELF files of this directory and never download
       --trace-hash               Print the execution trace hash at the end (determinism checks)
   -h, --help                     Print help
@@ -178,6 +180,8 @@
      window that may differ from the device)
   4  the CPU waits for an interrupt and none can come: no interrupt line is pending and no device
      has anything scheduled
+  5  the run would have exited 0, but the --console-script was not through: it was waiting for a
+     text, or one of its steps is dated after the end of the run
 130  Ctrl-C was pressed a second time before the run began its summary: no summary was printed,
      and the flash file is the one of before. Once the summary has begun, Ctrl-C ends nothing:
      the summary and the save of the flash file complete, with the exit code of the end of the run
```

```diff
--- a/crates/boards/src/lib.rs
+++ b/crates/boards/src/lib.rs
@@ -13,7 +13,7 @@
     FlashConfig, FlashConfigError, FlashImageError, JedecId, MAX_FLASH_BYTES, MIB, MIN_FLASH_BYTES,
 };
 use soc_esp32c3::periph::Efuse;
-use soc_esp32c3::{ChipRevision, Esp32C3, RomImage, SocConfig};
+use soc_esp32c3::{ChipRevision, Esp32C3, RomImage, SocConfig, UsbHost};
 use std::fmt;
 
 /// How to write the `jedec_id` key, for the messages that refuse one.
@@ -96,6 +96,8 @@
 pub struct MachineOptions {
     pub strict_mmio: bool,
     pub trace_hash: bool,
+    /// The USB host on the USB-Serial-JTAG port at power-on (ADR 0006).
+    pub usb_host: UsbHost,
 }
 
 #[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
@@ -549,6 +551,7 @@
             flash: self.flash,
             strict_mmio: options.strict_mmio,
             trace_hash: options.trace_hash,
+            usb_host: options.usb_host,
             ..SocConfig::default()
         }
     }
```

```diff
--- a/crates/emu-cli/Cargo.toml
+++ b/crates/emu-cli/Cargo.toml
@@ -22,6 +22,7 @@
 thiserror.workspace = true
 soc-esp32c3.workspace = true
 boards.workspace = true
+console-script.workspace = true
 sha2.workspace = true
 ureq.workspace = true
 flate2.workspace = true
```

```diff
--- a/crates/emu-cli/src/exit.rs
+++ b/crates/emu-cli/src/exit.rs
@@ -10,6 +10,9 @@
 pub const EXIT_STRICT_MMIO: u8 = 3;
 /// `inkemu run`: the CPU waits for an interrupt and none can come.
 pub const EXIT_IDLE: u8 = 4;
+/// `inkemu run --console-script`: the run ended, by a limit, Ctrl-C or a reader that went away,
+/// before the script was through: it waited for a text, or a step is dated after the end.
+pub const EXIT_SCRIPT_INCOMPLETE: u8 = 5;
 
 /// `inkemu run`: a second Ctrl-C ended the process before the run began its summary; once the
 /// summary has begun, Ctrl-C ends nothing. The value shells give a process ended by `SIGINT`
@@ -29,6 +32,8 @@
      window that may differ from the device)
   4  the CPU waits for an interrupt and none can come: no interrupt line is pending and no device
      has anything scheduled
+  5  the run would have exited 0, but the --console-script was not through: it was waiting for a
+     text, or one of its steps is dated after the end of the run
 130  Ctrl-C was pressed a second time before the run began its summary: no summary was printed,
      and the flash file is the one of before. Once the summary has begun, Ctrl-C ends nothing:
      the summary and the save of the flash file complete, with the exit code of the end of the run";
```

```diff
--- a/crates/emu-cli/src/lib.rs
+++ b/crates/emu-cli/src/lib.rs
@@ -8,4 +8,5 @@
 pub mod rom_store;
 pub mod run;
 pub mod rvtest;
+pub mod script_file;
 pub mod term;
```

```diff
--- a/crates/emu-cli/src/main.rs
+++ b/crates/emu-cli/src/main.rs
@@ -1,6 +1,7 @@
 //! inkemu command-line interface.
 use boards::{DeviceProfile, MachineOptions};
 use clap::{Parser, Subcommand};
+use console_script::ScriptRun;
 use cpu_rv32::{HartConfig, MisalignedPolicy};
 use emu_cli::exit::{EXIT_ERROR, EXIT_SECOND_INTERRUPT, EXIT_TEST_FAILURE, RUN_EXIT_CODES_HELP};
 use emu_cli::firmware_file;
@@ -9,11 +10,14 @@
 use emu_cli::rom_store::{
     self, AnnouncingFetch, ESP_ROM_ELFS, ESP32C3_ROMS, HttpFetch, RomEntry, RomSource,
 };
-use emu_cli::run::{Interrupt, RunOptions, execute, parse_device_seconds, relay};
+use emu_cli::run::{
+    Interrupt, RunOptions, execute_with_script, exit_code_with_script, parse_device_seconds, relay,
+};
 use emu_cli::rvtest::{RunResult, TestMachine};
+use emu_cli::script_file;
 use emu_cli::term::sanitize;
 use firmware::BootloaderWarning;
-use soc_esp32c3::ConsoleChannel;
+use soc_esp32c3::{ConsoleChannel, RomImage, UsbHost};
 use std::fmt::Write as _;
 use std::io;
 use std::ops::ControlFlow;
@@ -122,6 +126,20 @@
         /// Which console of the device goes to standard output.
         #[arg(long, value_enum, default_value_t = ConsoleArg::Uart0)]
         console: ConsoleArg,
+        /// What the host sends to the consoles, and when: a text file whose first line is
+        /// `inkemu-console-script 1`, then one step per line: `send CHANNEL "BYTES"`, `wait 250ms`,
+        /// `wait-for CHANNEL "TEXT"` (the next steps are dated from the device time the text
+        /// reached the host), `usb-host absent|attached|open`. CHANNEL is uart0, uart1 or
+        /// usb-serial-jtag; strings take \r \n \t \\ \" \xHH, and Enter is \r. Every step is dated
+        /// in device time, so two runs with the same script are the same run. A script not through
+        /// when the run ends makes it exit 5.
+        #[arg(long, value_name = "FILE")]
+        console_script: Option<PathBuf>,
+        /// The USB host on the USB-Serial-JTAG port when the device starts: absent (no cable),
+        /// attached (the port closed: frames, nothing read) or open (a program reads and writes
+        /// the port).
+        #[arg(long, value_enum, default_value_t = UsbHostArg::Open)]
+        usb_host: UsbHostArg,
         /// Use the ROM ELF files of this directory and never download.
         #[arg(long)]
         rom_dir: Option<PathBuf>,
@@ -153,6 +171,23 @@
             ConsoleArg::Uart0 => ConsoleChannel::Uart0,
             ConsoleArg::Uart1 => ConsoleChannel::Uart1,
             ConsoleArg::UsbSerialJtag => ConsoleChannel::UsbSerialJtag,
+        }
+    }
+}
+
+#[derive(Clone, Copy, clap::ValueEnum)]
+enum UsbHostArg {
+    Absent,
+    Attached,
+    Open,
+}
+
+impl From<UsbHostArg> for UsbHost {
+    fn from(arg: UsbHostArg) -> UsbHost {
+        match arg {
+            UsbHostArg::Absent => UsbHost::Absent,
+            UsbHostArg::Attached => UsbHost::Attached,
+            UsbHostArg::Open => UsbHost::Open,
         }
     }
 }
@@ -204,6 +239,8 @@
             max_time,
             strict_mmio,
             console,
+            console_script,
+            usb_host,
             rom_dir,
             trace_hash,
         } => run_device(
@@ -216,9 +253,11 @@
                 reset: reset_flash,
             },
             rom_dir,
+            console_script.as_deref(),
             MachineOptions {
                 strict_mmio,
                 trace_hash,
+                usb_host: usb_host.into(),
             },
             &RunOptions {
                 max_cycles,
@@ -309,11 +348,17 @@
     device: &str,
     flash: &FlashOptions<'_>,
     rom_dir: Option<PathBuf>,
+    console_script: Option<&Path>,
     machine_options: MachineOptions,
     run_options: &RunOptions,
 ) -> ExitCode {
     let profile = match profile_for(device) {
         Ok(p) => p,
+        Err(e) => return fail(&e),
+    };
+    // Read before anything else of the run: a refused script needs no flash and no ROM.
+    let script = match console_script.map(script_file::read_script).transpose() {
+        Ok(script) => script,
         Err(e) => return fail(&e),
     };
     let flash_size = profile.flash.size();
@@ -356,6 +401,71 @@
         None => format!("flash: {}", fresh.note),
     };
     let entry = rom_store::rom_entry_for(profile.chip_revision);
+    let rom = match load_rom(rom_dir, entry) {
+        Ok(rom) => rom,
+        Err(code) => return code,
+    };
+    let mut machine = match content {
+        Some(bytes) => match profile.instantiate_with_flash(rom, bytes, machine_options) {
+            Ok(machine) => machine,
+            Err(e) => return fail(&e),
+        },
+        None => profile.instantiate(rom, machine_options),
+    };
+    // Nothing refuses the run from here on but its flash file: only now is what an interrupted
+    // save left beside it removed. A run refused above left the directory as it found it.
+    if let Err(e) = run_flash.accept(&mut io::stderr().lock()) {
+        return fail(&e);
+    }
+    write_line(
+        io::stderr(),
+        &format_args!("{}; {flash_note}", header(&profile, entry, &machine)),
+    );
+    // The steps before the first wait are dated now, at power-on.
+    let mut script_run = match script
+        .as_ref()
+        .map(|script| ScriptRun::start(script, &mut machine))
+        .transpose()
+    {
+        Ok(run) => run,
+        Err(e) => return fail(&e),
+    };
+    let interrupt = Arc::new(Interrupt::new());
+    if let Err(e) = watch_ctrl_c(&interrupt) {
+        write_line(
+            io::stderr(),
+            &format_args!(
+                "warning: cannot watch for Ctrl-C: {e}; the run is not affected, but interrupting it will not print its summary: use --max-cycles or --max-time to bound it"
+            ),
+        );
+    }
+    let end = execute_with_script(
+        &mut machine,
+        run_options,
+        script_run.as_mut(),
+        &interrupt,
+        &stdout_reader_gone,
+        &mut io::stdout().lock(),
+        &mut io::stderr().lock(),
+    );
+    let script_status = script_run
+        .as_ref()
+        .map(|run| run.status(machine.device_time()));
+    // The run is over: what follows is no part of it, and of no trace. Since the first line of
+    // its summary Ctrl-C ends nothing (`execute` told `interrupt`): the summary and the save of
+    // the flash file are bounded (one write of the flash and one flush to the disk), and they
+    // complete; the exit code is the one of the end above.
+    let code = if run_flash.finish(&machine, &mut io::stderr().lock()) {
+        exit_code_with_script(end.exit_code(), script_status.as_ref())
+    } else {
+        end.exit_code_without_its_flash()
+    };
+    ExitCode::from(code)
+}
+
+/// The mask ROM of `entry`, from `rom_dir`, read only, or from the cache, which downloads it on
+/// first use. A refusal is printed, and its exit code given back.
+fn load_rom(rom_dir: Option<PathBuf>, entry: &RomEntry) -> Result<RomImage, ExitCode> {
     let http = HttpFetch;
     let mut stderr = io::stderr();
     let announcing;
@@ -371,56 +481,10 @@
                     user_named: false,
                 }
             }
-            Err(e) => return fail(&e),
+            Err(e) => return Err(fail(&e)),
         },
     };
-    let rom = match rom_store::load_rom(&source, &ESP_ROM_ELFS, entry) {
-        Ok(rom) => rom,
-        Err(e) => return fail(&e),
-    };
-    let mut machine = match content {
-        Some(bytes) => match profile.instantiate_with_flash(rom, bytes, machine_options) {
-            Ok(machine) => machine,
-            Err(e) => return fail(&e),
-        },
-        None => profile.instantiate(rom, machine_options),
-    };
-    // Nothing refuses the run from here on but its flash file: only now is what an interrupted
-    // save left beside it removed. A run refused above left the directory as it found it.
-    if let Err(e) = run_flash.accept(&mut io::stderr().lock()) {
-        return fail(&e);
-    }
-    write_line(
-        io::stderr(),
-        &format_args!("{}; {flash_note}", header(&profile, entry, &machine)),
-    );
-    let interrupt = Arc::new(Interrupt::new());
-    if let Err(e) = watch_ctrl_c(&interrupt) {
-        write_line(
-            io::stderr(),
-            &format_args!(
-                "warning: cannot watch for Ctrl-C: {e}; the run is not affected, but interrupting it will not print its summary: use --max-cycles or --max-time to bound it"
-            ),
-        );
-    }
-    let end = execute(
-        &mut machine,
-        run_options,
-        &interrupt,
-        &stdout_reader_gone,
-        &mut io::stdout().lock(),
-        &mut io::stderr().lock(),
-    );
-    // The run is over: what follows is no part of it, and of no trace. Since the first line of
-    // its summary Ctrl-C ends nothing (`execute` told `interrupt`): the summary and the save of
-    // the flash file are bounded (one write of the flash and one flush to the disk), and they
-    // complete; the exit code is the one of the end above.
-    let code = if run_flash.finish(&machine, &mut io::stderr().lock()) {
-        end.exit_code()
-    } else {
-        end.exit_code_without_its_flash()
-    };
-    ExitCode::from(code)
+    rom_store::load_rom(&source, &ESP_ROM_ELFS, entry).map_err(|e| fail(&e))
 }
 
 /// The flash a run starts from when nothing is kept from an earlier run.
```

```diff
--- a/crates/emu-cli/src/run.rs
+++ b/crates/emu-cli/src/run.rs
@@ -1,5 +1,6 @@
 //! `inkemu run`: drives a machine and relays its console and diagnostics.
-use crate::exit::{EXIT_ERROR, EXIT_IDLE, EXIT_OK, EXIT_STRICT_MMIO};
+use crate::exit::{EXIT_ERROR, EXIT_IDLE, EXIT_OK, EXIT_SCRIPT_INCOMPLETE, EXIT_STRICT_MMIO};
+use console_script::{ScriptRun, ScriptStatus};
 use emu_core::Cycles;
 use soc_esp32c3::diag::MAX_DISTINCT_DIAGNOSTICS;
 use soc_esp32c3::{ConsoleChannel, Diagnostic, Esp32C3, StopReason};
@@ -333,15 +334,18 @@
 /// selected console has nothing to write, `reader_gone` is asked instead, once, if `watch` says
 /// this pass is one that asks: a firmware that prints nothing would otherwise run on for a
 /// reader that left.
+///
+/// The bytes of every channel are given back, for a console script to read.
 fn relay_selected_console(
     machine: &mut Esp32C3,
     selected: ConsoleChannel,
     console: &mut dyn Write,
     watch: &mut ReaderWatch,
     reader_gone: &dyn Fn() -> bool,
-) -> io::Result<bool> {
+) -> (io::Result<bool>, Vec<(ConsoleChannel, Vec<u8>)>) {
     let executed = machine.cycles().get();
     let mut outcome = Ok(true);
+    let mut taken = Vec::with_capacity(ConsoleChannel::ALL.len());
     for channel in ConsoleChannel::ALL {
         let bytes = machine.take_console(channel);
         if channel == selected {
@@ -351,8 +355,43 @@
                 relay(console, &bytes)
             };
         }
-    }
-    outcome
+        taken.push((channel, bytes));
+    }
+    (outcome, taken)
+}
+
+/// The exit code of a run that ended with `code` and ran a console script whose status is
+/// `script`: a run that would exit 0 exits [`EXIT_SCRIPT_INCOMPLETE`] when the script is not
+/// through. Any other code is kept: it says more about the run (a `--strict-mmio` stop, a CPU
+/// that waits for ever, a console or a flash file that could not be written).
+#[must_use]
+pub fn exit_code_with_script(code: u8, script: Option<&ScriptStatus>) -> u8 {
+    match script {
+        Some(status) if code == EXIT_OK && *status != ScriptStatus::Complete => {
+            EXIT_SCRIPT_INCOMPLETE
+        }
+        _ => code,
+    }
+}
+
+/// The line of the summary that says how far the console script got.
+fn say_script(log: &mut dyn Write, status: &ScriptStatus, steps: usize) {
+    let _ = match status {
+        ScriptStatus::Complete => {
+            writeln!(log, "inkemu: console script through ({steps} steps)")
+        }
+        ScriptStatus::WaitingFor { step } => writeln!(
+            log,
+            "inkemu: console script not through: line {} ({step}) was still waiting; raise --max-time or --max-cycles, or check that the firmware prints that text on that console",
+            step.line
+        ),
+        ScriptStatus::Ahead { step, at } => writeln!(
+            log,
+            "inkemu: console script not through: line {} ({step}) is dated at {} us of device time, after the end of the run; raise --max-time or --max-cycles",
+            step.line,
+            soc_esp32c3::clock::TIME_BASE.cycles_to_nanos(*at) / 1_000
+        ),
+    };
 }
 
 /// Says in `log` why a run ended as [`RunEnd::ConsoleClosed`].
@@ -423,6 +462,25 @@
     console: &mut dyn Write,
     log: &mut dyn Write,
 ) -> RunEnd {
+    execute_with_script(machine, options, None, interrupt, reader_gone, console, log)
+}
+
+/// [`execute`], with a console script that started on `machine` ([`ScriptRun::start`]): after
+/// each call of the machine the bytes of every console are shown to it, so that its steps are
+/// handed to the machine at their dates; the summary says how far it got. A step the machine
+/// refuses is said in `log` and ends the script there; a script that [`console_script`] read
+/// stays within the bounds of the machine, so this does not happen from a file.
+pub fn execute_with_script(
+    machine: &mut Esp32C3,
+    options: &RunOptions,
+    mut script: Option<&mut ScriptRun>,
+    interrupt: &Interrupt,
+    reader_gone: &dyn Fn() -> bool,
+    console: &mut dyn Write,
+    log: &mut dyn Write,
+) -> RunEnd {
+    // Whether the script still reads the consoles: not after a step the machine refused.
+    let mut observing = true;
     // Whether the console has taken everything it was given. A console that failed, or whose
     // reader went away, is not written to again: a strict stop goes on to its own end after it,
     // and must not report the same failure a second time.
@@ -440,8 +498,15 @@
             .max_cycles
             .map_or(slice_end, |max| max.min(slice_end));
         let stop = machine.run_within(Cycles(limit), options.max_device_nanos);
-        let console_open =
+        let (console_open, taken) =
             relay_selected_console(machine, options.console, console, &mut reader, reader_gone);
+        if observing
+            && let Some(run) = script.as_deref_mut()
+            && let Err(e) = run.observe(machine, &taken)
+        {
+            let _ = writeln!(log, "error: {e}");
+            observing = false;
+        }
         console_usable = matches!(console_open, Ok(true));
         // The stop line carries the violation: listing it as a warning too would print it twice.
         let violation = match &stop {
@@ -504,6 +569,10 @@
         machine.device_time_nanos() / 1_000,
         frequency_text(machine.cpu_frequency().hz())
     );
+    let _ = writeln!(log, "inkemu: USB host {}", machine.usb_host().name());
+    if let Some(run) = script {
+        say_script(log, &run.status(machine.device_time()), run.step_count());
+    }
     summarize_diagnostics(machine, log);
     if let Some(hash) = machine.trace_hash() {
         let _ = writeln!(log, "trace-hash {hash:016x}");
```

Put this above the tests of `crates/emu-cli/src/script_file.rs`:

```rust
//! Reads the file given to `inkemu run --console-script`. The format, its checks and the run of
//! its steps belong to the `console-script` crate, so that a front end gets the same answers;
//! this module only reads the file, within the size the format allows, and names it.
use crate::flash_file::{ReadFailure, read_regular_file};
use crate::term::sanitize;
use console_script::{MAX_SCRIPT_BYTES, Script, ScriptError};
use std::path::Path;

/// Why the console script cannot be used. The messages state the cause, then the remedy; the
/// paths they echo have their control characters escaped.
#[derive(Debug, thiserror::Error)]
pub enum ScriptFileError {
    #[error(
        "cannot read the console script {path}: {detail}; check the path and its read permissions"
    )]
    Read { path: String, detail: String },
    #[error("the console script {path} is not a regular file; give the path of the file itself")]
    NotAFile { path: String },
    #[error(
        "the console script {path} is {len} bytes long, more than {MAX_SCRIPT_BYTES}; split it, or send less"
    )]
    TooLong { path: String, len: u64 },
    #[error(
        "the console script {path} held more than {MAX_SCRIPT_BYTES} bytes when it was read (it grew while it was being read); give a file that nothing else writes"
    )]
    Grew { path: String },
    #[error("the console script {path}, {source}")]
    Refused {
        path: String,
        #[source]
        source: ScriptError,
    },
}

/// The console script in the file `path`, read whole (at most [`MAX_SCRIPT_BYTES`] bytes, of
/// which no more than one more byte is read) and checked.
///
/// # Errors
/// [`ScriptFileError`]: the file cannot be read, or what it holds is no console script.
pub fn read_script(path: &Path) -> Result<Script, ScriptFileError> {
    let shown = || sanitize(&path.display().to_string());
    // INVARIANT: a mebibyte fits a `u32`.
    let limit = u32::try_from(MAX_SCRIPT_BYTES).unwrap_or(u32::MAX);
    let bytes = read_regular_file(path, limit).map_err(|failure| match failure {
        ReadFailure::Io(e) => ScriptFileError::Read {
            path: shown(),
            detail: sanitize(&e.to_string()),
        },
        ReadFailure::NotAFile => ScriptFileError::NotAFile { path: shown() },
        ReadFailure::TooLong(len) => ScriptFileError::TooLong { path: shown(), len },
        ReadFailure::Grew(_) => ScriptFileError::Grew { path: shown() },
    })?;
    Script::parse(&bytes).map_err(|source| ScriptFileError::Refused {
        path: shown(),
        source,
    })
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p emu-cli --lib > /tmp/t8.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t8.log
cargo test -p emu-cli --test run_cli -- usb_host_state refused_console_script run_help readme > /tmp/t8-cli.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t8-cli.log
cargo test -p boards --lib -- options_and_profile > /tmp/t8-b.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t8-b.log
```

Expected: `exit=0` and `test result: ok. 167 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`; then `exit=0` and `9 passed; 0 failed; 0 ignored; 0 measured; 81 filtered out`; then `exit=0` and `1 passed; 0 failed; 0 ignored; 0 measured; 35 filtered out`.

The two ignored tests of this task (`a_console_script_types_into_the_firmware_after_its_prompt`, `a_console_script_still_waiting_when_the_run_ends_exits_5`) need the ROM: the CI runs them in `rom-boot`, whose count goes from 25 to 27.

- [ ] **Step 5: Mutations**

| # | Mutation (`crates/emu-cli/src/run.rs`) | Test that fails |
|---|---|---|
| 1 | `Some(status) if code == EXIT_OK && *status != ScriptStatus::Complete =>` → `Some(status) if *status != ScriptStatus::Complete =>` | `a_script_not_through_changes_no_exit_code_but_0` |
| 2 | `&& let Err(e) = run.observe(machine, &taken)` → `&& let Err(e) = Ok::<(), console_script::RunError>(())` | `a_console_script_reads_the_consoles_and_its_summary_line_says_it_went_through` |

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy -p emu-cli -p boards -p console-script --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/emu-cli/src/script_file.rs
/usr/bin/git commit -m "feat(emu-cli): inkemu run takes --console-script and --usb-host, says how far the script got, and exits 5 when it is not through" -- Cargo.lock README.md .github/workflows/ci.yml crates/boards/src/lib.rs crates/emu-cli
```

Expected: three `exit=0`, then the commit.

---

## Task 9: One console driver for the tests that drive a machine

`soc_esp32c3::test_support` gains the driver the integration tests had copied five times (C10, C28): `Consoles` keeps what every channel sent, the diagnostics and the flash writes, taken after each call (`take` also gives back what this call took, which a console script reads); `drive(machine, bounds, consoles, done)` calls the machine in slices until `done` accepts, the machine stops (`Driven::Stopped`: a strict stop, a CPU waiting for ever) or the cycles or device time of `Bounds` run out (`Driven::OutOfBounds`). The four loops of `real_rom.rs` and `reference_boot.rs` and the countdown loop use it. The integration tests of `soc-esp32c3` reach `test_support` through a dependency of the crate on itself with the feature (C27), and a new test refuses `test_support` and `from_flat` in the documentation examples. `ConsoleChannel::index` becomes `pub(crate)`.

**Files:**
- Create: `crates/soc-esp32c3/tests/test_support_is_test_only.rs`
- Modify: `crates/soc-esp32c3/Cargo.toml`, `crates/soc-esp32c3/src/machine.rs`, `crates/soc-esp32c3/src/test_support.rs`, `crates/soc-esp32c3/tests/real_rom.rs`, `crates/soc-esp32c3/tests/reference_boot.rs`

**Interfaces:**
- Consumes: `Esp32C3::{run_within, take_console, drain_diagnostics, take_flash_writes}`.
- Produces (behind `test-support`): `pub struct Consoles { pub diagnostics, pub flash_writes, … }` with `bytes(channel) -> &[u8]`, `text(channel) -> String` (carriage returns dropped), `take(&mut self, machine) -> Vec<(ConsoleChannel, Vec<u8>)>`; `pub struct Bounds { pub slice: u64, pub max_cycles: u64, pub max_nanos: u64 }`; `pub enum Driven { Done, Stopped(StopReason), OutOfBounds }`; `pub fn drive(machine: &mut Esp32C3, bounds: Bounds, consoles: &mut Consoles, done: impl FnMut(&mut Esp32C3, &[(ConsoleChannel, Vec<u8>)], &Consoles) -> bool) -> Driven`.

**Bounds on untrusted input.** None new: `Bounds` is chosen by the test.

- [ ] **Step 1: Write the failing tests**

The loops of the two integration test files move to the driver, and the guard on the documentation examples is new. The driver's own unit tests come with it in Step 3 (they are in the same hunk of `test_support.rs`, which has no test module before this task).

```diff
--- a/crates/soc-esp32c3/tests/real_rom.rs
+++ b/crates/soc-esp32c3/tests/real_rom.rs
@@ -8,6 +8,7 @@
 use firmware::builder::{ImageSpec, build_image};
 use sha2::{Digest, Sha256};
 use soc_esp32c3::reset::ResetCause;
+use soc_esp32c3::test_support::{Bounds, Consoles, Driven, drive};
 use soc_esp32c3::{
     ChipRevision, ConsoleChannel, Diagnostic, Esp32C3, RomImage, SocConfig, StopReason,
 };
@@ -90,7 +91,7 @@
     uart0: String,
     usb: String,
     diagnostics: Vec<Diagnostic>,
-    stop: StopReason,
+    driven: Driven,
     trace_hash: Option<u64>,
     cycles: u64,
 }
@@ -99,23 +100,20 @@
 /// Carriage returns are dropped: the ROM expands `\n` to `\r\n` on the UART.
 fn boot(revision: u16, config: SocConfig) -> Boot {
     let mut machine = Esp32C3::new(RomImage::from_elf(&elf(revision)).unwrap(), &config);
-    let (mut uart0, mut usb, mut diagnostics) = (Vec::new(), Vec::new(), Vec::new());
-    let mut stop = StopReason::CycleBudget;
-    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace('\r', "");
-    while stop == StopReason::CycleBudget
-        && machine.cycles().get() < BANNER_BUDGET
-        && !text(&uart0).contains(BOOT_LINE)
-    {
-        stop = machine.run_until(Cycles(machine.cycles().get() + SLICE));
-        uart0.extend(machine.take_console(ConsoleChannel::Uart0));
-        usb.extend(machine.take_console(ConsoleChannel::UsbSerialJtag));
-        diagnostics.extend(machine.drain_diagnostics());
-    }
+    let mut consoles = Consoles::default();
+    let bounds = Bounds {
+        slice: SLICE,
+        max_cycles: BANNER_BUDGET,
+        max_nanos: u64::MAX,
+    };
+    let driven = drive(&mut machine, bounds, &mut consoles, |_, _, seen| {
+        seen.text(ConsoleChannel::Uart0).contains(BOOT_LINE)
+    });
     Boot {
-        uart0: text(&uart0),
-        usb: text(&usb),
-        diagnostics,
-        stop,
+        uart0: consoles.text(ConsoleChannel::Uart0),
+        usb: consoles.text(ConsoleChannel::UsbSerialJtag),
+        diagnostics: consoles.diagnostics,
+        driven,
         trace_hash: machine.trace_hash(),
         cycles: machine.cycles().get(),
     }
@@ -179,7 +177,7 @@
     assert!(
         boot.uart0.starts_with(banner_text),
         "rev{rom_revision} stopped early: {:?}\nUART0: {:?}",
-        boot.stop,
+        boot.driven,
         boot.uart0
     );
     assert!(
@@ -250,7 +248,7 @@
     /// UART0 output after the three banner lines, carriage returns dropped.
     after_banner: String,
     diagnostics: Vec<Diagnostic>,
-    stop: StopReason,
+    driven: Driven,
     pc: u32,
     trace_hash: Option<u64>,
 }
@@ -264,31 +262,34 @@
         None => Esp32C3::new(rom, &config),
     };
     let banner_text = banner("esp32c3-api1-20210207", "Feb  7 2021");
-    let (mut uart0, mut diagnostics) = (Vec::new(), Vec::new());
-    let mut stop = StopReason::CycleBudget;
-    let mut extra_slices = 1;
-    let after = |bytes: &[u8]| {
-        let text = String::from_utf8_lossy(bytes).replace('\r', "");
-        text.strip_prefix(&banner_text)
+    let after = |consoles: &Consoles| {
+        consoles
+            .text(ConsoleChannel::Uart0)
+            .strip_prefix(&banner_text)
             .map(str::to_owned)
             .unwrap_or_default()
     };
-    while stop == StopReason::CycleBudget && machine.cycles().get() < FLASH_BOOT_BUDGET {
-        if done(&after(&uart0)) {
-            if extra_slices == 0 {
-                break;
-            }
-            extra_slices -= 1;
+    let mut consoles = Consoles::default();
+    let bounds = Bounds {
+        slice: SLICE,
+        max_cycles: FLASH_BOOT_BUDGET,
+        max_nanos: u64::MAX,
+    };
+    let mut extra_slices = 1;
+    let driven = drive(&mut machine, bounds, &mut consoles, |_, _, seen| {
+        if !done(&after(seen)) {
+            return false;
         }
-        stop = machine.run_until(Cycles(machine.cycles().get() + SLICE));
-        uart0.extend(machine.take_console(ConsoleChannel::Uart0));
-        let _ = machine.take_console(ConsoleChannel::UsbSerialJtag);
-        diagnostics.extend(machine.drain_diagnostics());
-    }
+        if extra_slices == 0 {
+            return true;
+        }
+        extra_slices -= 1;
+        false
+    });
     FlashBoot {
-        after_banner: after(&uart0),
-        diagnostics,
-        stop,
+        after_banner: after(&consoles),
+        diagnostics: consoles.diagnostics,
+        driven,
         pc: machine.pc(),
         trace_hash: machine.trace_hash(),
     }
@@ -327,7 +328,7 @@
         Vec::new(),
         "nothing unmodelled, no exception, on the whole flash path"
     );
-    assert_eq!(boot.stop, StopReason::CycleBudget);
+    assert_eq!(boot.driven, Driven::Done);
     assert_eq!(boot.pc, STUB_ENTRY, "the ROM jumped into the image");
 }
 
@@ -352,7 +353,7 @@
         boot.after_banner
     );
     assert_eq!(boot.diagnostics, Vec::new());
-    assert_eq!(boot.stop, StopReason::CycleBudget, "a loop, not a stop");
+    assert_eq!(boot.driven, Driven::Done, "a loop, not a stop");
 }
 
 #[test]
@@ -530,35 +531,33 @@
     limit_ns: u64,
     done: impl Fn(&str) -> bool,
 ) -> WatchdogRun {
-    let (mut uart0, mut reset_seen_at, mut diagnostics) = (Vec::new(), Vec::new(), Vec::new());
-    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace('\r', "");
-    while !(machine.resets() >= resets && done(&text(&uart0))) {
-        assert!(
-            machine.cycles().get() < FLASH_BOOT_BUDGET * 4
-                && machine.device_time_nanos() < limit_ns,
-            "budget exhausted after {} resets, at {} ns of device time and {} cycles (limits: {limit_ns} ns, {} cycles); console:\n{}",
-            machine.resets(),
-            machine.device_time_nanos(),
-            machine.cycles().get(),
-            FLASH_BOOT_BUDGET * 4,
-            text(&uart0)
-        );
-        let stop = machine.run_until(Cycles(machine.cycles().get() + WATCHDOG_SLICE));
-        assert!(
-            matches!(stop, StopReason::CycleBudget | StopReason::Asleep),
-            "{stop:?}"
-        );
+    let mut reset_seen_at = Vec::new();
+    let mut consoles = Consoles::default();
+    let bounds = Bounds {
+        slice: WATCHDOG_SLICE,
+        max_cycles: FLASH_BOOT_BUDGET * 4,
+        max_nanos: limit_ns,
+    };
+    let driven = drive(&mut machine, bounds, &mut consoles, |machine, _, seen| {
         if machine.resets() > u64::try_from(reset_seen_at.len()).unwrap() {
             reset_seen_at.push(machine.device_time_nanos());
         }
-        uart0.extend(machine.take_console(ConsoleChannel::Uart0));
-        let _ = machine.take_console(ConsoleChannel::UsbSerialJtag);
-        diagnostics.extend(machine.drain_diagnostics());
-    }
+        machine.resets() >= resets && done(&seen.text(ConsoleChannel::Uart0))
+    });
+    assert_eq!(
+        driven,
+        Driven::Done,
+        "after {} resets, at {} ns of device time and {} cycles (limits: {limit_ns} ns, {} cycles); console:\n{}",
+        machine.resets(),
+        machine.device_time_nanos(),
+        machine.cycles().get(),
+        FLASH_BOOT_BUDGET * 4,
+        consoles.text(ConsoleChannel::Uart0)
+    );
     WatchdogRun {
-        console: text(&uart0),
+        console: consoles.text(ConsoleChannel::Uart0),
         reset_seen_at,
-        diagnostics,
+        diagnostics: consoles.diagnostics,
         repeated: machine.most_repeated_diagnostics(8),
     }
 }
```

```diff
--- a/crates/soc-esp32c3/tests/reference_boot.rs
+++ b/crates/soc-esp32c3/tests/reference_boot.rs
@@ -14,13 +14,11 @@
 
 mod common;
 
-use emu_core::Cycles;
 use sha2::{Digest, Sha256};
 use soc_esp32c3::memmap::{SRAM_DBUS_BASE, SRAM_SIZE};
 use soc_esp32c3::reset::ResetCause;
-use soc_esp32c3::{
-    ConsoleChannel, Diagnostic, Esp32C3, FlashWrite, RomImage, SocConfig, StopReason,
-};
+use soc_esp32c3::test_support::{Bounds, Consoles, Driven, drive};
+use soc_esp32c3::{ConsoleChannel, Diagnostic, Esp32C3, FlashWrite, RomImage, SocConfig};
 
 /// SHA-256 of the merged reference image (`docs/research/sp1b-test-oracle.md`, §2).
 const REFERENCE_SHA256: &str = "fc05ef5dbe85372e1da52abdec3bfcbbb110738a60163734666095fcd8c39bf2";
@@ -214,7 +212,9 @@
     flash_modified_sectors: u32,
     /// How many flash commands of each opcode were sent to the chip, indexed by opcode.
     flash_commands: Vec<u64>,
-    stop: StopReason,
+    /// How the drive ended: a run that stopped (a strict stop, a CPU waiting for ever) did not
+    /// reach what it waited for.
+    driven: Driven,
     trace_hash: Option<u64>,
     resets: u64,
     cycles: u64,
@@ -259,46 +259,41 @@
 /// stops, or the budget ends.
 fn run_image(image: Vec<u8>, config: SocConfig, done: impl Fn(&str, &[Diagnostic]) -> bool) -> Run {
     let mut machine = Esp32C3::with_flash(rom(), image, &config).unwrap();
-    let (mut uart0, mut diagnostics) = (Vec::new(), Vec::new());
-    let mut flash_writes = Vec::new();
-    let mut stop = StopReason::CycleBudget;
-    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace('\r', "");
-    // `Asleep` is the CPU waiting for its next interrupt: device time goes on, the run too.
-    while matches!(stop, StopReason::CycleBudget | StopReason::Asleep)
-        && machine.cycles().get() < BUDGET
-        && machine.device_time_nanos() < DEVICE_TIME_BUDGET_NS
-        && !done(&text(&uart0), &diagnostics)
-    {
-        stop = machine.run_until(Cycles(machine.cycles().get() + SLICE));
-        uart0.extend(machine.take_console(ConsoleChannel::Uart0));
-        let _ = machine.take_console(ConsoleChannel::UsbSerialJtag);
-        diagnostics.extend(machine.drain_diagnostics());
-        flash_writes.extend(machine.take_flash_writes());
-    }
+    let mut consoles = Consoles::default();
+    let bounds = Bounds {
+        slice: SLICE,
+        max_cycles: BUDGET,
+        max_nanos: DEVICE_TIME_BUDGET_NS,
+    };
+    let driven = drive(&mut machine, bounds, &mut consoles, |_, _, seen| {
+        done(&seen.text(ConsoleChannel::Uart0), &seen.diagnostics)
+    });
     assert_eq!(
         machine.flash_writes_dropped(),
         0,
         "the journal was taken at every slice: it never filled"
     );
     assert!(
-        machine.device_time_nanos() < DEVICE_TIME_BUDGET_NS || done(&text(&uart0), &diagnostics),
+        machine.device_time_nanos() < DEVICE_TIME_BUDGET_NS || driven == Driven::Done,
         "device time ran out ({} ns) after {} CPU cycles before the run was done: the CPU sleeps and nothing wakes it?\nconsole:\n{}",
         machine.device_time_nanos(),
         machine.cycles().get(),
-        text(&uart0)
+        consoles.text(ConsoleChannel::Uart0)
     );
     // The run is over: what the firmware wrote and the UART had not sent yet belongs to its console.
-    uart0.extend(machine.flush_console(ConsoleChannel::Uart0));
+    let unsent = machine.flush_console(ConsoleChannel::Uart0);
+    let mut console = consoles.bytes(ConsoleChannel::Uart0).to_vec();
+    console.extend(unsent);
     Run {
-        console: text(&uart0),
-        diagnostics,
-        flash_writes,
+        console: String::from_utf8_lossy(&console).replace('\r', ""),
+        diagnostics: consoles.diagnostics,
+        flash_writes: consoles.flash_writes,
         flash: machine.flash_contents(),
         flash_modified_sectors: machine.flash_modified_sectors(),
         flash_commands: (0..=u8::MAX)
             .map(|opcode| machine.flash_commands_sent(opcode))
             .collect(),
-        stop,
+        driven,
         trace_hash: machine.trace_hash(),
         resets: machine.resets(),
         cycles: machine.cycles().get(),
@@ -395,9 +390,9 @@
     };
     let life = run(strict, |console| application_starts(console) == 2);
     assert!(
-        matches!(life.stop, StopReason::CycleBudget | StopReason::Asleep),
+        !matches!(life.driven, Driven::Stopped(_)),
         "nothing stops the run: {:?}\nconsole:\n{}",
-        life.stop,
+        life.driven,
         life.console
     );
     // The restart is an entry of the journal, not a violation: strict mode lets it pass and
@@ -430,32 +425,37 @@
     /// One tick of the application's scheduler (`CONFIG_FREERTOS_HZ` is 100).
     const TICK_NS: u64 = SECOND_NS / 100;
     let mut machine = Esp32C3::with_flash(rom(), reference_flash(), &SocConfig::default()).unwrap();
-    let mut uart0 = Vec::new();
+    let mut consoles = Consoles::default();
     // Device time, read from the machine, at the first look that found each countdown line
     // printed.
     let mut seen_at: Vec<u64> = Vec::new();
-    let mut restarting = false;
-    while machine.cycles().get() < BUDGET
-        && machine.device_time_nanos() < DEVICE_TIME_BUDGET_NS
-        && !restarting
-    {
-        let stop = machine.run_until(Cycles(machine.cycles().get() + LOOK));
-        assert!(
-            matches!(stop, StopReason::CycleBudget | StopReason::Asleep),
-            "{stop:?}"
-        );
-        let fresh = machine.take_console(ConsoleChannel::Uart0);
-        if fresh.is_empty() {
-            continue;
-        }
-        uart0.extend(fresh);
-        let console = String::from_utf8_lossy(&uart0);
-        if console.matches("seconds...\r\n").count() > seen_at.len() {
-            seen_at.push(machine.device_time_nanos());
-        }
-        restarting = console.contains("Restarting now.");
-    }
-    assert_eq!(seen_at.len(), 11, "{}", String::from_utf8_lossy(&uart0));
+    let bounds = Bounds {
+        slice: LOOK,
+        max_cycles: BUDGET,
+        max_nanos: DEVICE_TIME_BUDGET_NS,
+    };
+    let driven = drive(
+        &mut machine,
+        bounds,
+        &mut consoles,
+        |machine, taken, seen| {
+            if taken[0].1.is_empty() {
+                return false;
+            }
+            let console = seen.text(ConsoleChannel::Uart0);
+            if console.matches("seconds...\n").count() > seen_at.len() {
+                seen_at.push(machine.device_time_nanos());
+            }
+            console.contains("Restarting now.")
+        },
+    );
+    assert_eq!(driven, Driven::Done);
+    assert_eq!(
+        seen_at.len(),
+        11,
+        "{}",
+        consoles.text(ConsoleChannel::Uart0)
+    );
     // `vTaskDelay(1000 / portTICK_PERIOD_MS)` waits 100 ticks counted from the next tick edge, so
     // the first delay, which starts inside a tick, lasts one second less the part of that tick
     // already spent: 0 to 10 ms shorter than a second, never longer. Which tick the first line is
@@ -612,9 +612,9 @@
     assert_eq!(life.flash_modified_sectors, 1, "the otadata sector");
     assert_only_the_restart_is_reported(&life.diagnostics);
     assert!(
-        matches!(life.stop, StopReason::CycleBudget | StopReason::Asleep),
+        !matches!(life.driven, Driven::Stopped(_)),
         "nothing stops a lenient run: {:?}",
-        life.stop
+        life.driven
     );
 }
 
@@ -633,9 +633,9 @@
         |console, _| console.contains("Hello world!\n"),
     );
     assert!(
-        matches!(boot.stop, StopReason::CycleBudget | StopReason::Asleep),
+        !matches!(boot.driven, Driven::Stopped(_)),
         "a strict run of this layout does not stop: {:?}\nconsole:\n{}",
-        boot.stop,
+        boot.driven,
         boot.console
     );
     assert_eq!(boot.diagnostics, [], "nothing unmodelled, no exception");
@@ -897,9 +897,9 @@
 fn the_nvs_example_counts_its_restarts_over_three_lives_and_programs_the_measured_bytes() {
     let run = run_image(nvs_flash(), strict(), lives(3));
     assert!(
-        matches!(run.stop, StopReason::CycleBudget | StopReason::Asleep),
+        !matches!(run.driven, Driven::Stopped(_)),
         "a strict run of the example does not stop: {:?}\nconsole:\n{}",
-        run.stop,
+        run.driven,
         run.console
     );
     assert_the_console_of_three_lives(&run.console, &[]);
@@ -927,9 +927,9 @@
 fn the_nvs_example_behind_the_stock_x4_table_writes_otadata_once_and_counts_its_restarts() {
     let run = run_image(nvs_ota_flash(), strict(), lives(3));
     assert!(
-        matches!(run.stop, StopReason::CycleBudget | StopReason::Asleep),
+        !matches!(run.driven, Driven::Stopped(_)),
         "a strict run of the example does not stop: {:?}\nconsole:\n{}",
-        run.stop,
+        run.driven,
         run.console
     );
     assert_the_console_of_three_lives(&run.console, &[NO_FACTORY_IMAGE, OTADATA_SET]);
```

Create `crates/soc-esp32c3/tests/test_support_is_test_only.rs`:

````rust
//! Every test build of this crate turns on its `test-support` feature (the crate is its own
//! dev-dependency, `Cargo.toml`), and so do the documentation examples, which are compiled as
//! tests. An example that called `test_support` or `RomImage::from_flat` would therefore pass
//! `cargo test` and fail in the hands of a user, who has no such feature: the examples must take
//! what they need as parameters. This test reads the examples and refuses those two names in them.
#![allow(clippy::disallowed_methods)] // reads the sources: it is a test of the build
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

/// The names an example must not use: they exist only under the feature.
const TEST_ONLY: [&str; 2] = ["test_support", "from_flat"];

fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

/// The lines of the documentation examples of `text`: between the fences of `///` and `//!`
/// comments.
fn example_lines(text: &str) -> Vec<(usize, &str)> {
    let mut inside = false;
    let mut lines = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let Some(doc) = line
            .trim_start()
            .strip_prefix("///")
            .or_else(|| line.trim_start().strip_prefix("//!"))
        else {
            inside = false;
            continue;
        };
        if doc.trim_start().starts_with("```") {
            inside = !inside;
        } else if inside {
            lines.push((number + 1, doc));
        }
    }
    lines
}

#[test]
fn no_documentation_example_uses_what_only_tests_have() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    assert!(
        files.len() > 10,
        "the sources were not found under {}",
        src.display()
    );
    let mut examples = 0;
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        for (number, line) in example_lines(&text) {
            examples += 1;
            for name in TEST_ONLY {
                assert!(
                    !line.contains(name),
                    "{}:{number}: a documentation example uses `{name}`, which only test builds have; take what it needs as a parameter",
                    file.display()
                );
            }
        }
    }
    assert!(
        examples > 0,
        "no documentation example was read: the reader is broken"
    );
}

#[test]
fn the_reader_finds_the_lines_of_an_example_and_only_them() {
    let text = "/// Text.\n///\n/// ```\n/// let rom = RomImage::from_flat(&[]);\n/// ```\n/// After.\nfn f() {}\n//! ```ignore\n//! test_support::HANG\n//! ```\n";
    assert_eq!(
        example_lines(text),
        [
            (4, " let rom = RomImage::from_flat(&[]);"),
            (9, " test_support::HANG")
        ]
    );
}
````

- [ ] **Step 2: Run them and see them fail**

```bash
cargo check -p soc-esp32c3 --tests > /tmp/t9-red.log 2>&1; echo "exit=$?"; grep -E "^error" /tmp/t9-red.log | sort | uniq -c | head
```

Expected: `exit=101`: `error[E0432]: unresolved import soc_esp32c3::test_support`, once in `real_rom` and once in `reference_boot` (an integration test does not see a module behind a feature its crate does not turn on).

- [ ] **Step 3: The driver, the self dependency, and the loops that use it**

```diff
--- a/crates/soc-esp32c3/Cargo.toml
+++ b/crates/soc-esp32c3/Cargo.toml
@@ -24,3 +24,10 @@
 [dev-dependencies]
 cpu-rv32 = { workspace = true, features = ["test-support"] }
 firmware = { workspace = true, features = ["test-support"] }
+# The integration tests of this crate drive machines with `test_support` (its console driver
+# among it), which they can only reach through the feature. A crate that depends on itself for
+# its tests turns the feature on for every test build of it, `cargo test -p soc-esp32c3` as
+# `cargo test --workspace`: the two agree, where the documentation examples of SP1b-3b differed.
+# Those examples keep taking their `RomImage` as a parameter, so that none of them leans on the
+# feature (`crates/soc-esp32c3/tests/test_support_is_test_only.rs`).
+soc-esp32c3 = { path = ".", features = ["test-support"] }
```

```diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -46,7 +46,7 @@
     }
 
     /// The place of the channel in [`ConsoleChannel::ALL`].
-    const fn index(self) -> usize {
+    pub(crate) const fn index(self) -> usize {
         match self {
             ConsoleChannel::Uart0 => 0,
             ConsoleChannel::UsbSerialJtag => 1,
```

```diff
--- a/crates/soc-esp32c3/src/test_support.rs
+++ b/crates/soc-esp32c3/src/test_support.rs
@@ -4,7 +4,9 @@
 
 use crate::memmap::ROM_BASE;
 use crate::rom::{ROM_LEN, RomImage};
+use crate::{ConsoleChannel, Diagnostic, Esp32C3, FlashWrite, StopReason};
 use cpu_rv32::asm;
+use emu_core::Cycles;
 
 /// A minimal ELF32 little-endian writer: enough structure for `RomImage::from_elf`, nothing more.
 #[derive(Clone, Debug, Default)]
@@ -468,3 +470,201 @@
         },
     }
 }
+
+/// What the consoles of a machine sent since it was driven, and its diagnostics, taken after
+/// every call: the one record the tests that drive a machine through its consoles keep.
+#[derive(Clone, Debug, Default, PartialEq, Eq)]
+pub struct Consoles {
+    sent: [Vec<u8>; 3],
+    pub diagnostics: Vec<Diagnostic>,
+    pub flash_writes: Vec<FlashWrite>,
+}
+
+impl Consoles {
+    /// Everything `channel` sent.
+    #[must_use]
+    pub fn bytes(&self, channel: ConsoleChannel) -> &[u8] {
+        &self.sent[channel.index()]
+    }
+
+    /// Everything `channel` sent, as text, carriage returns dropped: the ROM and ESP-IDF expand
+    /// `\n` to `\r\n` on their consoles.
+    #[must_use]
+    pub fn text(&self, channel: ConsoleChannel) -> String {
+        String::from_utf8_lossy(self.bytes(channel)).replace('\r', "")
+    }
+
+    /// Takes what every channel of `machine` sent, its diagnostics and its flash writes, keeps
+    /// them, and gives back what the channels sent in this take, in [`ConsoleChannel::ALL`]
+    /// order: what a console script reads.
+    pub fn take(&mut self, machine: &mut Esp32C3) -> Vec<(ConsoleChannel, Vec<u8>)> {
+        self.diagnostics.extend(machine.drain_diagnostics());
+        self.flash_writes.extend(machine.take_flash_writes());
+        ConsoleChannel::ALL
+            .into_iter()
+            .map(|channel| {
+                let taken = machine.take_console(channel);
+                self.sent[channel.index()].extend_from_slice(&taken);
+                (channel, taken)
+            })
+            .collect()
+    }
+}
+
+/// The bounds of [`drive`]: the CPU cycles of each call, and the cycles and device time of the
+/// whole run. Both totals are needed: the cycle counter stands still while the CPU sleeps, so a
+/// machine that sleeps for ever never exhausts the first.
+#[derive(Clone, Copy, Debug, PartialEq, Eq)]
+pub struct Bounds {
+    pub slice: u64,
+    pub max_cycles: u64,
+    pub max_nanos: u64,
+}
+
+/// How [`drive`] ended.
+#[derive(Clone, Debug, PartialEq, Eq)]
+pub enum Driven {
+    /// The caller's check accepted what the machine did.
+    Done,
+    /// The machine stopped for this reason, which no further call changes: a strict stop, or a
+    /// CPU that waits for ever.
+    Stopped(StopReason),
+    /// The cycles or the device time of [`Bounds`] ran out first.
+    OutOfBounds,
+}
+
+/// Drives `machine` in calls of `bounds.slice` cycles, taking its consoles into `consoles`
+/// after each call, until `done` accepts them, the machine stops, or the bounds run out. `done`
+/// sees the machine (to feed it input dated from what it sent), what this call took
+/// ([`Consoles::take`]) and everything taken so far. A call that ends because a watched console
+/// spoke, or because the CPU slept a while, is followed by the next.
+pub fn drive(
+    machine: &mut Esp32C3,
+    bounds: Bounds,
+    consoles: &mut Consoles,
+    mut done: impl FnMut(&mut Esp32C3, &[(ConsoleChannel, Vec<u8>)], &Consoles) -> bool,
+) -> Driven {
+    loop {
+        let cycles = machine.cycles().get();
+        if cycles >= bounds.max_cycles || machine.device_time_nanos() >= bounds.max_nanos {
+            return Driven::OutOfBounds;
+        }
+        let limit = cycles.saturating_add(bounds.slice).min(bounds.max_cycles);
+        let stop = machine.run_within(Cycles(limit), Some(bounds.max_nanos));
+        let taken = consoles.take(machine);
+        if done(machine, &taken, consoles) {
+            return Driven::Done;
+        }
+        match stop {
+            StopReason::CycleBudget
+            | StopReason::TimeBudget
+            | StopReason::Asleep
+            | StopReason::ConsoleOutput => {}
+            StopReason::StrictMmio(_) | StopReason::Idle => return Driven::Stopped(stop),
+        }
+    }
+}
+
+#[cfg(test)]
+mod tests {
+    use super::*;
+    use crate::SocConfig;
+    use asm::wfi;
+
+    const UART0: u32 = 0x6000_0000;
+
+    fn machine(program: &[u32]) -> Esp32C3 {
+        Esp32C3::new(flat_rom(program), &SocConfig::default())
+    }
+
+    const WIDE: Bounds = Bounds {
+        slice: 1_000,
+        max_cycles: 10_000_000,
+        max_nanos: 1_000_000_000,
+    };
+
+    #[test]
+    fn drive_keeps_every_channel_and_ends_when_the_check_accepts() {
+        let mut m = machine(&print_program(UART0, b"ok"));
+        let mut consoles = Consoles::default();
+        let mut calls = 0;
+        let end = drive(&mut m, WIDE, &mut consoles, |_, _, seen| {
+            calls += 1;
+            seen.text(ConsoleChannel::Uart0) == "ok"
+        });
+        assert_eq!(end, Driven::Done);
+        assert_eq!(consoles.bytes(ConsoleChannel::Uart0), b"ok");
+        assert_eq!(consoles.bytes(ConsoleChannel::UsbSerialJtag), b"");
+        assert!(calls > 1, "the check is called after every call");
+    }
+
+    #[test]
+    fn drive_ends_on_a_stop_and_when_its_bounds_run_out() {
+        // Strapped for another boot mode, no watchdog runs at reset: nothing wakes the CPU, and
+        // device time stands still, so a driver that called again would never return.
+        let end = within(
+            std::time::Duration::from_secs(60),
+            "drive called an idle machine again and again",
+            || {
+                let mut idle = Esp32C3::new(
+                    flat_rom(&[wfi(), HANG]),
+                    &SocConfig {
+                        strapping: 0,
+                        ..SocConfig::default()
+                    },
+                );
+                drive(&mut idle, WIDE, &mut Consoles::default(), |_, _, _| false)
+            },
+        );
+        assert_eq!(end, Driven::Stopped(StopReason::Idle));
+        let mut spinning = machine(&[HANG]);
+        let tight = Bounds {
+            max_cycles: 5_500,
+            ..WIDE
+        };
+        let end = drive(&mut spinning, tight, &mut Consoles::default(), |_, _, _| {
+            false
+        });
+        assert_eq!(end, Driven::OutOfBounds);
+        // The last call stops at the bound, not a slice later: the instruction that crosses it
+        // completes (a jump is 2 cycles).
+        assert!(
+            (5_500..5_510).contains(&spinning.cycles().get()),
+            "{:?}",
+            spinning.cycles()
+        );
+        let short = Bounds {
+            max_nanos: 10_000,
+            ..WIDE
+        };
+        let mut spinning = machine(&[HANG]);
+        let end = drive(&mut spinning, short, &mut Consoles::default(), |_, _, _| {
+            false
+        });
+        assert_eq!(end, Driven::OutOfBounds);
+        // Ends within an instruction of the bound (a jump at 20 MHz: 100 ns).
+        assert!(
+            (10_000..10_100).contains(&spinning.device_time_nanos()),
+            "{}",
+            spinning.device_time_nanos()
+        );
+    }
+
+    #[test]
+    fn take_gives_back_what_each_channel_sent_in_this_take() {
+        let mut m = machine(&print_program(UART0, b"x"));
+        let mut consoles = Consoles::default();
+        m.run_until(Cycles(1_000_000));
+        let taken = consoles.take(&mut m);
+        assert_eq!(
+            taken,
+            [
+                (ConsoleChannel::Uart0, b"x".to_vec()),
+                (ConsoleChannel::UsbSerialJtag, Vec::new()),
+                (ConsoleChannel::Uart1, Vec::new()),
+            ]
+        );
+        assert_eq!(consoles.take(&mut m)[0].1, b"", "taken once");
+        assert_eq!(consoles.bytes(ConsoleChannel::Uart0), b"x");
+    }
+}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p soc-esp32c3 --lib -- test_support:: > /tmp/t9.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t9.log
cargo test -p soc-esp32c3 --test test_support_is_test_only > /tmp/t9-doc.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t9-doc.log
cargo check -p soc-esp32c3 --tests > /tmp/t9-check.log 2>&1; echo "exit=$?"
```

Expected: `exit=0` and `test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 565 filtered out`; then `exit=0` and `2 passed`; then `exit=0`.

The ignored tests that now use the driver run in the CI with their counts unchanged: `real_rom` 18 (`rom-boot`), `reference_boot` 13 (`reference-boot`).

- [ ] **Step 5: Mutations**

| # | Mutation (`crates/soc-esp32c3/src/test_support.rs`) | Test that fails |
|---|---|---|
| 1 | `StopReason::StrictMmio(_) \| StopReason::Idle => return Driven::Stopped(stop),` → `StopReason::StrictMmio(_) \| StopReason::Idle => {}` | `drive_ends_on_a_stop_and_when_its_bounds_run_out` (after the 60 s of `within`: the driver calls an idle machine for ever) |
| 2 | in `Consoles::take`, delete `self.sent[channel.index()].extend_from_slice(&taken);` | `drive_keeps_every_channel_and_ends_when_the_check_accepts`, `take_gives_back_what_each_channel_sent_in_this_take` |
| 3 | `tests/test_support_is_test_only.rs`: `inside = !inside;` → `inside = false;` (run `cargo test -p soc-esp32c3 --test test_support_is_test_only`) | `the_reader_finds_the_lines_of_an_example_and_only_them`, `no_documentation_example_uses_what_only_tests_have` |

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy -p soc-esp32c3 --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/soc-esp32c3/tests/test_support_is_test_only.rs
/usr/bin/git commit -m "test(soc-esp32c3): one console driver for the tests that drive a machine, and the integration tests reach test_support" -- Cargo.lock crates/soc-esp32c3
```

Expected: three `exit=0`, then the commit.

---

## Task 10: Three reference firmwares read their consoles

The milestones on firmwares built with ESP-IDF (C9, C29, C30). The build script builds three more projects in the same container run, after the two of before: the stock `peripherals/uart/uart_select`, the stock `peripherals/usb_serial_jtag/usb_serial_jtag_echo` (which ships its own `sdkconfig.defaults`: the rule changes from refusing such an example to merging its file after the reference one, `SDKCONFIG_DEFAULTS="<reference>;<own>"`, and a project without one is built exactly as before), and `usb_serial_jtag_console`, a 15-line application of this repository under Apache-2.0 (USJ §F.2) whose console is USB-Serial-JTAG. Their applications leave the container alone (`uart_select.bin`, `usb_serial_jtag_echo.bin`, `usb_serial_jtag_console.bin`, and their symbol tables); the tests compose each behind the bootloader inkemu ships, as `inkemu run --firmware` does. A project whose sums file does not exist is built, its hashes printed after `unpinned <file>:`, and the script stops before writing anything: that is how the three are pinned (Step 6). The five tests of `crates/console-script/tests/reference_firmware.rs` are ignored and run by the `reference-boot` job.

**Files:**
- Create: `tools/reference-firmware/usb_serial_jtag_console/{CMakeLists.txt, sdkconfig.defaults, main/CMakeLists.txt, main/usb_serial_jtag_console_main.c}`, `crates/console-script/tests/reference_firmware.rs`; Step 6: `tools/reference-firmware/SHA256SUMS.inputs-uart-select`, `SHA256SUMS.inputs-usj-echo`, `SHA256SUMS.inputs-usj-console`
- Modify: `tools/reference-firmware/{in-container.sh, build.sh, README.md}`, `crates/console-script/Cargo.toml`, `.github/workflows/ci.yml`, `README.md`

**Interfaces:**
- Consumes: `test_support::{drive, Consoles, Bounds, Driven}` (Task 9), `ScriptRun` (Task 7), `boards::builtin`, `DeviceProfile::{compose_flash, instantiate_with_flash}`, `MachineOptions::usb_host` (Task 8).
- Produces: the variables `INKEMU_UART_SELECT_APP`, `INKEMU_USJ_ECHO_APP`, `INKEMU_USJ_CONSOLE_APP`; the CI line `tools/ci/run-ignored-tests.sh 5 --release -p console-script --test reference_firmware -- --ignored`.

**Bounds on untrusted input.** The images are checked against their pinned SHA-256 before they run; the runs are bounded by `Bounds` (2·10⁹ cycles, 3 to 30 s of device time).

- [ ] **Step 1: Write the tests**

They are ignored: they need the ROM and the images, which only the CI has.

Create `crates/console-script/tests/reference_firmware.rs`:

```rust
//! The milestones of plan SP1b-3d on firmwares built with ESP-IDF, booted from the real mask ROM:
//! console scripts typed into `uart_select` (UART0, driver, `select`), `usb_serial_jtag_echo`
//! (USB-Serial-JTAG, driver), this repository's `usb_serial_jtag_console` (a console that needs a
//! USB host), and the reference `hello_world`, whose console the ROM mirrors on USB-Serial-JTAG.
//!
//! Each image is named by its own variable and checked against the hash pinned beside its build,
//! `tools/reference-firmware/SHA256SUMS.inputs-*`. The whole suite:
//! `INKEMU_ROM_ELF_DIR=<dir> INKEMU_REF_FLASH=<flash_hello.bin> INKEMU_UART_SELECT_APP=<uart_select.bin> INKEMU_USJ_ECHO_APP=<usb_serial_jtag_echo.bin> INKEMU_USJ_CONSOLE_APP=<usb_serial_jtag_console.bin> cargo test --release -p console-script --test reference_firmware -- --ignored`
//! Build the images with `tools/reference-firmware/build.sh`, which prints their paths.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure
#![allow(clippy::disallowed_methods)] // reads the images and the ROM the environment names

use boards::MachineOptions;
use console_script::{Script, ScriptRun, ScriptStatus, VERSION_LINE};
use sha2::{Digest, Sha256};
use soc_esp32c3::test_support::{Bounds, Consoles, Driven, drive};
use soc_esp32c3::{ConsoleChannel, Esp32C3, RomImage, UsbHost};
use std::fmt::Write as _;
use std::path::Path;

const SECOND_NS: u64 = 1_000_000_000;
/// The CPU cycles a run may spend: far more than any of these scripts needs.
const BUDGET: u64 = 2_000_000_000;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

fn rom() -> RomImage {
    let dir = std::env::var("INKEMU_ROM_ELF_DIR").expect(
        "set INKEMU_ROM_ELF_DIR: fetch the ROMs with `inkemu rom fetch --all --rom-dir <dir>`",
    );
    let elf = std::fs::read(Path::new(&dir).join("esp32c3_rev3_rom.elf")).unwrap();
    RomImage::from_elf(&elf).unwrap()
}

/// The SHA-256 pinned for `file` in `tools/reference-firmware/<sums>`, the file the build checks:
/// the one place a hash is written.
fn pinned_sha256(sums: &str, file: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/reference-firmware")
        .join(sums);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}; the image is not pinned yet: see tools/reference-firmware/README.md",
            path.display()
        )
    });
    text.lines()
        .find_map(|line| {
            let (hash, name) = line.split_once("  ")?;
            (name == file || name.ends_with(&format!("/{file}"))).then(|| hash.to_owned())
        })
        .unwrap_or_else(|| panic!("{}: no line for {file}", path.display()))
}

/// The image the variable `variable` names, checked against its pinned hash.
fn pinned_image(variable: &str, sums: &str, file: &str) -> Vec<u8> {
    let path = std::env::var(variable).unwrap_or_else(|_| {
        panic!("set {variable} to the {file} of tools/reference-firmware/build.sh")
    });
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert_eq!(
        hex(&Sha256::digest(&bytes)),
        pinned_sha256(sums, file),
        "{path} is not the pinned {file}; rebuild it with tools/reference-firmware/build.sh"
    );
    bytes
}

/// The X4 with `application` behind the bootloader inkemu ships, as `inkemu run --firmware`
/// composes it.
fn x4_with(application: &[u8], options: MachineOptions) -> Esp32C3 {
    let profile = boards::builtin("x4").unwrap();
    let flash = profile.compose_flash(application, None).unwrap();
    profile
        .instantiate_with_flash(rom(), flash.bytes, options)
        .unwrap()
}

fn script(body: &str) -> Script {
    Script::parse(format!("{VERSION_LINE}\n{body}").as_bytes()).unwrap()
}

/// Runs `machine` with `script` in calls of `slice` cycles until the script is through, the
/// machine stops, or `max_nanos` of device time have passed.
fn scripted(
    machine: &mut Esp32C3,
    script: &Script,
    slice: u64,
    max_nanos: u64,
) -> (Consoles, ScriptStatus, Driven) {
    let mut run = ScriptRun::start(script, machine).unwrap();
    let mut consoles = Consoles::default();
    let bounds = Bounds {
        slice,
        max_cycles: BUDGET,
        max_nanos,
    };
    let driven = drive(machine, bounds, &mut consoles, |machine, taken, _| {
        run.observe(machine, taken).unwrap();
        run.status(machine.device_time()) == ScriptStatus::Complete
    });
    (consoles, run.status(machine.device_time()), driven)
}

/// The milliseconds an ESP-IDF log line begins with: `I (5321) tag: ...`.
fn log_millis(line: &str) -> u64 {
    let rest = line.split_once(" (").unwrap().1;
    rest.split_once(')').unwrap().0.parse().unwrap()
}

const TIMEOUT_LINE: &str =
    "uart_select_example: Timeout has been reached and nothing has been received";
const UART_SELECT_SCRIPT: &str = "\
wait-for uart0 \"Timeout has been reached and nothing has been received\\r\\n\"
send uart0 \"a\"
wait-for uart0 \"Received: a\\r\\n\"
wait 500ms
send uart0 \"bc\\r\\n\"
wait-for uart0 \"Timeout has been reached and nothing has been received\\r\\n\"
";

fn uart_select(trace_hash: bool) -> Esp32C3 {
    let application = pinned_image(
        "INKEMU_UART_SELECT_APP",
        "SHA256SUMS.inputs-uart-select",
        "uart_select.bin",
    );
    x4_with(
        &application,
        MachineOptions {
            strict_mmio: true,
            trace_hash,
            usb_host: UsbHost::Open,
        },
    )
}

/// Milestone 1 (UR §9.2): the timeout line comes 5 s of device time after `app_main` returns,
/// `a` typed after it gives exactly `Received: a`, and `bc\r\n` typed in one burst gives the four
/// lines in order, each byte its own line: the 174 bytes they make are more than the transmit
/// FIFO holds, so the writer waits for `TXFIFO_EMPTY`. `--strict-mmio` stops at nothing.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_UART_SELECT_APP"]
fn uart_select_reference_firmware_times_out_then_reads_each_byte_typed() {
    let mut machine = uart_select(false);
    let (consoles, status, driven) = scripted(
        &mut machine,
        &script(UART_SELECT_SCRIPT),
        1_000_000,
        30 * SECOND_NS,
    );
    let console = consoles.text(ConsoleChannel::Uart0);
    assert_eq!(driven, Driven::Done, "{console}");
    assert_eq!(status, ScriptStatus::Complete);
    let lines: Vec<&str> = console.lines().collect();
    let returned = lines
        .iter()
        .position(|line| line.contains("main_task: Returned from app_main()"))
        .unwrap_or_else(|| panic!("{console}"));
    let after: Vec<&str> = lines[returned + 1..]
        .iter()
        .map(|line| line.split_once(") ").map_or(*line, |(_, text)| text))
        .collect();
    assert_eq!(
        after,
        [
            TIMEOUT_LINE,
            "uart_select_example: Received: a",
            "uart_select_example: Received: b",
            "uart_select_example: Received: c",
            "uart_select_example: Received: ",
            "",
            "uart_select_example: Received: ",
            "",
            TIMEOUT_LINE,
        ],
        "{console}"
    );
    let waited = log_millis(lines[returned + 1]) - log_millis(lines[returned]);
    assert!(
        (4_990..=5_020).contains(&waited),
        "the first timeout came {waited} ms after app_main returned:\n{console}"
    );
}

/// Milestone 5: the same script gives the same run, to the trace hash, whatever the calls the
/// run is cut into.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_UART_SELECT_APP"]
fn uart_select_reference_firmware_typed_into_is_the_same_run_whatever_the_slices() {
    let outcome = |slice: u64| {
        let mut machine = uart_select(true);
        let (consoles, status, driven) = scripted(
            &mut machine,
            &script(UART_SELECT_SCRIPT),
            slice,
            30 * SECOND_NS,
        );
        assert_eq!((status, driven), (ScriptStatus::Complete, Driven::Done));
        (
            consoles.bytes(ConsoleChannel::Uart0).to_vec(),
            consoles.bytes(ConsoleChannel::UsbSerialJtag).to_vec(),
            machine.trace_hash(),
            machine.device_time(),
            machine.instructions(),
        )
    };
    assert_eq!(outcome(1_000_000), outcome(7_919));
}

/// The address of the buffer an `ESP_LOG_BUFFER_HEXDUMP` row dumps: `... Recv str: : 0x3fc8a1b0   68 69 ...`.
fn dump_address(line: &str) -> &str {
    line.split_once(": 0x")
        .unwrap()
        .1
        .split_whitespace()
        .next()
        .unwrap()
}

/// Milestone 2 (USJ §F.1): with the host open, a line sent after the init line is echoed on
/// USB-Serial-JTAG and dumped on UART0; 90 bytes arrive as two packets, 64 and 26, which the
/// example reads in two calls: the second dump starts again at the first address of the buffer.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_USJ_ECHO_APP"]
fn usb_serial_jtag_echo_reference_firmware_echoes_on_usb_and_dumps_on_uart0() {
    let application = pinned_image(
        "INKEMU_USJ_ECHO_APP",
        "SHA256SUMS.inputs-usj-echo",
        "usb_serial_jtag_echo.bin",
    );
    let mut machine = x4_with(&application, MachineOptions::default());
    let ninety = "0123456789".repeat(9);
    let body = format!(
        "wait-for uart0 \"USB_SERIAL_JTAG init done\"\nsend usb-serial-jtag \"hi, espressif\\n\"\nwait-for usb-serial-jtag \"hi, espressif\\n\"\nsend usb-serial-jtag \"{ninety}\"\nwait-for usb-serial-jtag \"{ninety}\"\nwait 100ms\n"
    );
    let (consoles, status, driven) =
        scripted(&mut machine, &script(&body), 1_000_000, 20 * SECOND_NS);
    let uart0 = consoles.text(ConsoleChannel::Uart0);
    assert_eq!(
        (status, driven),
        (ScriptStatus::Complete, Driven::Done),
        "{uart0}"
    );
    let usb = consoles.bytes(ConsoleChannel::UsbSerialJtag);
    assert!(
        usb.ends_with(format!("hi, espressif\n{ninety}").as_bytes()),
        "{}",
        String::from_utf8_lossy(usb)
    );
    let dumps: Vec<&str> = uart0
        .lines()
        .filter(|line| line.contains("Recv str: "))
        .collect();
    assert_eq!(
        dumps.len(),
        7,
        "one row for the first line, 4 + 2 for the 90 bytes:\n{uart0}"
    );
    assert!(
        dumps[0].ends_with("68 69 2c 20 65 73 70 72  65 73 73 69 66 0a        |hi, espressif.|"),
        "{uart0}"
    );
    assert_eq!(dump_address(dumps[1]), dump_address(dumps[0]));
    assert_eq!(
        dump_address(dumps[5]),
        dump_address(dumps[0]),
        "26 bytes read on their own:\n{uart0}"
    );
}

fn usb_serial_jtag_console(host: UsbHost) -> Esp32C3 {
    let application = pinned_image(
        "INKEMU_USJ_CONSOLE_APP",
        "SHA256SUMS.inputs-usj-console",
        "usb_serial_jtag_console.bin",
    );
    x4_with(
        &application,
        MachineOptions {
            usb_host: host,
            ..MachineOptions::default()
        },
    )
}

/// Milestone 3 (USJ §F.2): the console of this firmware works only with a host that reads the
/// port. Open: `ready`, then the line typed, echoed. Absent: no start-of-frame ever comes, the
/// console discards what is printed, and USB-Serial-JTAG carries nothing.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_USJ_CONSOLE_APP"]
fn usb_serial_jtag_console_reference_firmware_needs_an_open_host() {
    let mut open = usb_serial_jtag_console(UsbHost::Open);
    let body = "wait-for usb-serial-jtag \"ready\\r\\n\"\nsend usb-serial-jtag \"hello\\r\"\nwait-for usb-serial-jtag \"echo: hello\\r\\n\"\n";
    let (consoles, status, driven) = scripted(&mut open, &script(body), 1_000_000, 10 * SECOND_NS);
    let usb = consoles.text(ConsoleChannel::UsbSerialJtag);
    assert_eq!(
        (status, driven),
        (ScriptStatus::Complete, Driven::Done),
        "{usb}"
    );
    assert!(usb.ends_with("ready\necho: hello\n"), "{usb:?}");

    let mut absent = usb_serial_jtag_console(UsbHost::Absent);
    let (consoles, _, driven) =
        scripted(&mut absent, &script("wait 2s\n"), 1_000_000, 3 * SECOND_NS);
    assert_eq!(driven, Driven::Done);
    assert_eq!(
        consoles.text(ConsoleChannel::UsbSerialJtag),
        "",
        "UART0:\n{}",
        consoles.text(ConsoleChannel::Uart0)
    );
}

/// Milestone 4 (USJ §F.3): with the host open, the ROM mirrors on USB-Serial-JTAG every byte the
/// reference `hello_world` prints on UART0: the two consoles carry the same bytes.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn hello_world_reference_firmware_prints_the_same_bytes_on_usb_serial_jtag_as_on_uart0() {
    let flash = pinned_image("INKEMU_REF_FLASH", "SHA256SUMS.output", "flash_hello.bin");
    let profile = boards::builtin("x4").unwrap();
    let mut machine = profile
        .instantiate_with_flash(rom(), flash, MachineOptions::default())
        .unwrap();
    let body = "wait-for uart0 \"Restarting in 5 seconds...\"\n";
    let (consoles, status, driven) =
        scripted(&mut machine, &script(body), 1_000_000, 20 * SECOND_NS);
    assert_eq!((status, driven), (ScriptStatus::Complete, Driven::Done));
    // What was written and not yet sent belongs to each console.
    let mut uart0 = consoles.bytes(ConsoleChannel::Uart0).to_vec();
    uart0.extend(machine.flush_console(ConsoleChannel::Uart0));
    let mut usb = consoles.bytes(ConsoleChannel::UsbSerialJtag).to_vec();
    usb.extend(machine.flush_console(ConsoleChannel::UsbSerialJtag));
    assert_eq!(
        String::from_utf8_lossy(&usb),
        String::from_utf8_lossy(&uart0)
    );
}
```

- [ ] **Step 2: See them fail to build**

The crate does not depend on `boards` and `sha2` yet.

```bash
cargo test -p console-script --test reference_firmware > /tmp/t10-red.log 2>&1; echo "exit=$?"; grep -E "^error|^test result" /tmp/t10-red.log
```

Expected: `exit=101`; `could not compile console-script (test "reference_firmware") due to 4 previous errors`: `unresolved import boards`, `unresolved import sha2`.

- [ ] **Step 3: The application, the build rule and the CI wiring**

```diff
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -149,7 +149,11 @@
           INKEMU_REF_FLASH: ${{ runner.temp }}/reference-firmware/flash_hello.bin
           INKEMU_NVS_FLASH: ${{ runner.temp }}/reference-firmware/flash_nvs.bin
           INKEMU_NVS_OTA_FLASH: ${{ runner.temp }}/reference-firmware/flash_nvs_ota.bin
+          INKEMU_UART_SELECT_APP: ${{ runner.temp }}/reference-firmware/uart_select.bin
+          INKEMU_USJ_ECHO_APP: ${{ runner.temp }}/reference-firmware/usb_serial_jtag_echo.bin
+          INKEMU_USJ_CONSOLE_APP: ${{ runner.temp }}/reference-firmware/usb_serial_jtag_console.bin
         # Exact test counts, as in rom-boot: see tools/ci/run-ignored-tests.sh.
         run: |
           tools/ci/run-ignored-tests.sh 13 --release -p soc-esp32c3 --test reference_boot -- --ignored
           tools/ci/run-ignored-tests.sh 8 --release -p emu-cli --test run_cli reference_firmware -- --ignored
+          tools/ci/run-ignored-tests.sh 5 --release -p console-script --test reference_firmware -- --ignored
```

```diff
--- a/README.md
+++ b/README.md
@@ -322,12 +322,12 @@
 
 ### Matériel tiers et licence
 
-La ROM masquée de l'ESP32-C3 est le code d'Espressif et n'est pas libre : inkemu ne la distribue jamais, il télécharge les fichiers ELF qu'Espressif publie sous Apache-2.0 dans `esp-rom-elfs` et les vérifie contre des empreintes épinglées. Le firmware de référence (bootloader ESP-IDF, Apache-2.0, et deux exemples ESP-IDF non modifiés, `hello_world` et `nvs_rw_value_cxx`, dans le domaine public) est construit dans Docker et vérifié de même. Le seul binaire de firmware tiers du dépôt est le bootloader qu'inkemu embarque, `third_party/esp-idf-bootloader/` (ESP-IDF v5.5.2, Apache-2.0, avec deux avis BSD-3-Clause, sa licence et sa provenance) : la CI vérifie qu'il est celui de la construction de référence, et son avis dit la seule modification qu'inkemu lui apporte (pour une flash qui ne fait pas 16 Mio, la taille de flash de son en-tête et son empreinte ajoutée) ; aucun autre binaire de firmware n'est stocké. `third_party/espressif-svd/` contient une copie non modifiée du `esp32c3.svd` d'Espressif (Apache-2.0), dont `crates/svd-regs` est généré. Voir les ADR [0001](docs/adr/0001-espressif-rom-and-svd-licensing.md) et [0005](docs/adr/0005-bootloader-embarque.md) ; `inkemu licenses` imprime les avis. inkemu est sous [licence Apache 2.0](LICENSE). Xteink est une marque de son propriétaire ; ce projet n'est affilié ni à Xteink ni à Espressif.
+La ROM masquée de l'ESP32-C3 est le code d'Espressif et n'est pas libre : inkemu ne la distribue jamais, il télécharge les fichiers ELF qu'Espressif publie sous Apache-2.0 dans `esp-rom-elfs` et les vérifie contre des empreintes épinglées. Le firmware de référence (bootloader ESP-IDF, Apache-2.0, quatre exemples ESP-IDF non modifiés, `hello_world`, `nvs_rw_value_cxx`, `uart_select` et `usb_serial_jtag_echo`, dans le domaine public, et `usb_serial_jtag_console`, une petite application de ce dépôt sous Apache-2.0) est construit dans Docker et vérifié de même. Le seul binaire de firmware tiers du dépôt est le bootloader qu'inkemu embarque, `third_party/esp-idf-bootloader/` (ESP-IDF v5.5.2, Apache-2.0, avec deux avis BSD-3-Clause, sa licence et sa provenance) : la CI vérifie qu'il est celui de la construction de référence, et son avis dit la seule modification qu'inkemu lui apporte (pour une flash qui ne fait pas 16 Mio, la taille de flash de son en-tête et son empreinte ajoutée) ; aucun autre binaire de firmware n'est stocké. `third_party/espressif-svd/` contient une copie non modifiée du `esp32c3.svd` d'Espressif (Apache-2.0), dont `crates/svd-regs` est généré. Voir les ADR [0001](docs/adr/0001-espressif-rom-and-svd-licensing.md) et [0005](docs/adr/0005-bootloader-embarque.md) ; `inkemu licenses` imprime les avis. inkemu est sous [licence Apache 2.0](LICENSE). Xteink est une marque de son propriétaire ; ce projet n'est affilié ni à Xteink ni à Espressif.
 
 ## Third-party material
 
 - **Mask ROM.** The ESP32-C3 ROM is Espressif's code and is not open source. inkemu never ships it: it downloads the ROM ELF files that Espressif publishes under Apache-2.0 in [`esp-rom-elfs`](https://github.com/espressif/esp-rom-elfs) and verifies them against pinned hashes.
-- **Reference firmware.** The tests boot the stock ESP-IDF bootloader (Apache-2.0) and two unmodified ESP-IDF examples, `hello_world` and `nvs_rw_value_cxx` (public domain, `Unlicense OR CC0-1.0`). They are built in Docker by `tools/reference-firmware/build.sh` and checked against pinned hashes; no firmware binary is stored in this repository but the one below.
+- **Reference firmware.** The tests boot the stock ESP-IDF bootloader (Apache-2.0), four unmodified ESP-IDF examples, `hello_world`, `nvs_rw_value_cxx`, `uart_select` and `usb_serial_jtag_echo` (public domain, `Unlicense OR CC0-1.0`), and `usb_serial_jtag_console`, a small application of this repository (Apache-2.0, `tools/reference-firmware/usb_serial_jtag_console/`). They are built in Docker by `tools/reference-firmware/build.sh` and checked against pinned hashes; no firmware binary is stored in this repository but the one below.
 - **The bootloader inkemu ships.** `third_party/esp-idf-bootloader/` holds the ESP-IDF v5.5.2 2nd-stage bootloader of the reference build (Apache-2.0, with two BSD-3-Clause notices), with its licence, its notice and its provenance; it is embedded in the binary, and the CI checks that it is the one the reference build produces. Its notice also states the one change inkemu makes to it: for a flash other than 16 MiB, the flash size in its header and its appended digest. `inkemu licenses` prints the notices.
 - **Register descriptions.** `third_party/espressif-svd/` holds an unmodified copy of Espressif's `esp32c3.svd` (Apache-2.0), from which `crates/svd-regs` is generated.
 
```

```diff
--- a/crates/console-script/Cargo.toml
+++ b/crates/console-script/Cargo.toml
@@ -17,3 +17,7 @@
 cpu-rv32 = { workspace = true, features = ["test-support"] }
 soc-esp32c3 = { workspace = true, features = ["test-support"] }
 svd-regs.workspace = true
+# The tests on the reference firmwares compose their flash as `inkemu run --firmware` does, and
+# check the images they are given against their pinned SHA-256.
+boards.workspace = true
+sha2.workspace = true
```

```diff
--- a/tools/reference-firmware/README.md
+++ b/tools/reference-firmware/README.md
@@ -1,9 +1,10 @@
 # Reference firmware of the SP1b tests
 
 `build.sh [output-dir]` builds, inside the pinned `espressif/idf` Docker image and in one container
-run, the stock ESP-IDF 2nd-stage bootloader and two unmodified ESP-IDF examples for the ESP32-C3,
-checks the binaries of each build against its pinned hashes, merges them into 16 MB flash images
-and checks the images against `SHA256SUMS.output`. The ESP-IDF version is `ESP_IDF_VERSION` in
+run, the stock ESP-IDF 2nd-stage bootloader and four unmodified ESP-IDF examples for the ESP32-C3,
+and one application of this directory, checks the binaries of each build against its pinned
+hashes, merges the first two into 16 MB flash images and checks the images against
+`SHA256SUMS.output`. The ESP-IDF version is `ESP_IDF_VERSION` in
 `tools/conformance/versions.env`.
 
 | Image | Application | Partition table | Pins of its build |
@@ -19,9 +20,29 @@
 program counter printed by inkemu into a function name: the last symbol whose address is not above
 the PC.
 
-The examples are built as ESP-IDF ships them, with `sdkconfig.defaults` of this directory as their
-only configuration. An example that ships a `sdkconfig.defaults` of its own stops the script before
-anything is built: its file would be replaced, and the build would not be the unmodified example.
+Three applications are built next to them and leave the container alone, without a merged image:
+the tests of the serial consoles (`crates/console-script/tests/reference_firmware.rs`) boot them
+behind the bootloader inkemu ships, as `inkemu run --firmware` does.
+
+| Application | Source | What its tests type into it | Pins of its build |
+|---|---|---|---|
+| `uart_select.bin` | `examples/peripherals/uart/uart_select`: UART0 through the driver, `select()` with a 5 s timeout, one log line per byte read | `a`, then `bc\r\n` | `SHA256SUMS.inputs-uart-select` |
+| `usb_serial_jtag_echo.bin` | `examples/peripherals/usb_serial_jtag/usb_serial_jtag_echo`: what the host sends to USB-Serial-JTAG is echoed there and dumped on UART0 | a line, then 90 bytes | `SHA256SUMS.inputs-usj-echo` |
+| `usb_serial_jtag_console.bin` | `usb_serial_jtag_console/` of this directory, ours (Apache-2.0): USB-Serial-JTAG is the console, each line read is echoed; it needs a USB host that reads the port | `hello\r` | `SHA256SUMS.inputs-usj-console` |
+
+The examples are built as ESP-IDF ships them. Their configuration is `sdkconfig.defaults` of this
+directory, then the example's own `sdkconfig.defaults` when it ships one (`usb_serial_jtag_echo`
+does: `CONFIG_ESP_CONSOLE_SECONDARY_NONE=y`), merged by ESP-IDF in that order, so that the example
+keeps every setting it chose. The example's file is never replaced. `usb_serial_jtag_console` is
+configured the same way, its own file adding `CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y`.
+
+## Adding a firmware
+
+A project whose sums file does not exist yet is built, its hashes are printed after a line
+`unpinned <file>:`, and the script stops before anything reaches the output directory. Pin a new
+firmware from two clean builds: run the build (the CI job `reference-boot` does when its cache key
+changes), copy the three lines printed under `unpinned <file>:` into `<file>` here, and commit: the
+next build is the second one, and fails if it differs from the first.
 
 Run the tests that need the images, those of the machine and those of the `inkemu` command:
 
@@ -30,9 +51,13 @@
     export INKEMU_ROM_ELF_DIR=/tmp/inkemu-rom \
       INKEMU_REF_FLASH="$PWD/target/reference-firmware/flash_hello.bin" \
       INKEMU_NVS_FLASH="$PWD/target/reference-firmware/flash_nvs.bin" \
-      INKEMU_NVS_OTA_FLASH="$PWD/target/reference-firmware/flash_nvs_ota.bin"
+      INKEMU_NVS_OTA_FLASH="$PWD/target/reference-firmware/flash_nvs_ota.bin" \
+      INKEMU_UART_SELECT_APP="$PWD/target/reference-firmware/uart_select.bin" \
+      INKEMU_USJ_ECHO_APP="$PWD/target/reference-firmware/usb_serial_jtag_echo.bin" \
+      INKEMU_USJ_CONSOLE_APP="$PWD/target/reference-firmware/usb_serial_jtag_console.bin"
     cargo test --release -p soc-esp32c3 --test reference_boot -- --ignored
     cargo test --release -p emu-cli --test run_cli reference_firmware -- --ignored
+    cargo test --release -p console-script --test reference_firmware -- --ignored
 
 The tests of `inkemu run --firmware` boot the application images alone: they take them out of the
 merged images, from 0x10000, and check them against `SHA256SUMS.inputs` and `SHA256SUMS.inputs-nvs`
```

```diff
--- a/tools/reference-firmware/build.sh
+++ b/tools/reference-firmware/build.sh
@@ -4,7 +4,9 @@
 #   flash_hello.bin    the hello_world example;
 #   flash_nvs.bin      the nvs_rw_value_cxx example, which keeps a restart counter in NVS;
 #   flash_nvs_ota.bin  the same application under the stock partition table of the X4
-#                      (tests/fixtures/partitions/x4-stock.bin), whose bootloader writes otadata.
+#                      (tests/fixtures/partitions/x4-stock.bin), whose bootloader writes otadata;
+# and three applications alone, which the tests of the serial consoles boot behind the bootloader
+# inkemu ships: uart_select.bin, usb_serial_jtag_echo.bin and usb_serial_jtag_console.bin.
 # Everything third-party runs inside Docker, in one container run. One artefact of this build is
 # committed: its bootloader, the one inkemu ships (third_party/esp-idf-bootloader/, the same bytes);
 # nothing else built here is.
@@ -32,3 +34,6 @@
 echo "$out/flash_hello.bin"
 echo "$out/flash_nvs.bin"
 echo "$out/flash_nvs_ota.bin"
+echo "$out/uart_select.bin"
+echo "$out/usb_serial_jtag_echo.bin"
+echo "$out/usb_serial_jtag_console.bin"
```

```diff
--- a/tools/reference-firmware/in-container.sh
+++ b/tools/reference-firmware/in-container.sh
@@ -16,32 +16,49 @@
   fi
 }
 
-# Builds the ESP-IDF example "$1" (a path under $IDF_PATH/examples), unmodified, with the reference
-# configuration as its only one, checks its three binaries against the pinned hashes of "$2", and
-# leaves the shell in its build directory. It says how long the build took: the cost of each
-# example in a CI job is read in the job log.
+# Builds the ESP-IDF project in the directory "$1" (an example under $IDF_PATH/examples, or a
+# reference firmware of this directory), unmodified, checks its three binaries against the pinned
+# hashes of "$2", and leaves the shell in its build directory. It says how long the build took: the
+# cost of each project in a CI job is read in the job log.
 #
-# The reference configuration is added to the example, it replaces nothing: an example that ships
-# a sdkconfig.defaults of its own stops the script before anything is copied. Its file would be
-# overwritten, and what is built would no longer be the example as ESP-IDF configures it.
+# The configuration is the reference one, then the project's own sdkconfig.defaults when it ships
+# one: ESP-IDF reads the list in order, a later file overriding an earlier one, so the project
+# keeps every setting it chose and the reference settings fill in the rest. The project's file is
+# never replaced: what is built is the project as it is configured, on top of the reference. A
+# project without a file of its own gets the reference file copied in, as before this rule existed,
+# so that the builds pinned before it do not change.
+#
+# A project whose sums file "$2" does not exist yet is built, its hashes printed and its name added
+# to `unpinned`; the script goes on with the others and stops before handing anything over: that is
+# how a new reference firmware is pinned, every new one in the same run (README.md, "Adding a
+# firmware").
+unpinned=()
 build_example() {
-  local example="$1" sums="$2"
+  local source="$1" sums="$2"
   local name project started="$SECONDS"
-  name="$(basename "$example")"
+  name="$(basename "$source")"
   project="/tmp/$name"
-  if [ -e "$IDF_PATH/examples/$example/sdkconfig.defaults" ]; then
-    echo "error: the example $example ships its own sdkconfig.defaults, which the reference configuration would replace: the build would not be the unmodified example; choose an example without one, or decide how the two configurations combine and pin the result of that build" >&2
-    exit 1
+  rm -rf "$project"
+  cp -r "$source" "$project"
+  cd "$project"
+  local defaults=()
+  if [ -e sdkconfig.defaults ]; then
+    defaults=(-D "SDKCONFIG_DEFAULTS=/ref/sdkconfig.defaults;$project/sdkconfig.defaults")
+  else
+    cp /ref/sdkconfig.defaults sdkconfig.defaults
   fi
-  rm -rf "$project"
-  cp -r "$IDF_PATH/examples/$example" "$project"
-  cp /ref/sdkconfig.defaults "$project/sdkconfig.defaults"
-  cd "$project"
-  step "idf.py set-target ($name)" "/tmp/set-target-$name.log" idf.py set-target esp32c3
-  step "idf.py build ($name)" "/tmp/build-$name.log" idf.py build
+  step "idf.py set-target ($name)" "/tmp/set-target-$name.log" idf.py "${defaults[@]}" set-target esp32c3
+  step "idf.py build ($name)" "/tmp/build-$name.log" idf.py "${defaults[@]}" build
   cd build
+  local binaries=(bootloader/bootloader.bin partition_table/partition-table.bin "$name.bin")
+  if [ ! -e "$sums" ]; then
+    echo "unpinned $(basename "$sums"):"
+    sha256sum "${binaries[@]}"
+    unpinned+=("$name")
+    return 0
+  fi
   if ! sha256sum -c "$sums"; then
-    sha256sum bootloader/bootloader.bin partition_table/partition-table.bin "$name.bin" >&2
+    sha256sum "${binaries[@]}" >&2
     echo "error: the build of $name is not the pinned one (hashes above); do not edit $(basename "$sums"): check the image tag, the ESP-IDF version and sdkconfig.defaults" >&2
     exit 1
   fi
@@ -72,7 +89,7 @@
 
 stage="$(mktemp -d)"
 
-build_example get-started/hello_world /ref/SHA256SUMS.inputs
+build_example "$IDF_PATH/examples/get-started/hello_world" /ref/SHA256SUMS.inputs
 merge flash_hello.bin partition_table/partition-table.bin hello_world.bin
 symbols bootloader/bootloader hello_world
 
@@ -80,10 +97,33 @@
 # table (its sums pin them again), once as built and once under the stock partition table of the
 # X4, which has `otadata` and two application slots. Neither the bootloader nor the application
 # embeds the table, so the second image needs no second build.
-build_example storage/nvs/nvs_rw_value_cxx /ref/SHA256SUMS.inputs-nvs
+build_example "$IDF_PATH/examples/storage/nvs/nvs_rw_value_cxx" /ref/SHA256SUMS.inputs-nvs
 merge flash_nvs.bin partition_table/partition-table.bin nvs_rw_value_cxx.bin
 merge flash_nvs_ota.bin /partitions/x4-stock.bin nvs_rw_value_cxx.bin
 symbols nvs_rw_value_cxx
+
+# The firmwares of the serial consoles. Their tests boot the application alone, behind the
+# bootloader inkemu ships (`inkemu run --firmware`): only the application leaves the container.
+# uart_select reads UART0 through the driver and select(); usb_serial_jtag_echo, which ships a
+# sdkconfig.defaults of its own, echoes what the host sends to USB-Serial-JTAG; the third is this
+# directory's own, a console on USB-Serial-JTAG that needs a host reading the port.
+applications=()
+for project in \
+  "$IDF_PATH/examples/peripherals/uart/uart_select:SHA256SUMS.inputs-uart-select" \
+  "$IDF_PATH/examples/peripherals/usb_serial_jtag/usb_serial_jtag_echo:SHA256SUMS.inputs-usj-echo" \
+  "/ref/usb_serial_jtag_console:SHA256SUMS.inputs-usj-console"; do
+  source="${project%%:*}"
+  name="$(basename "$source")"
+  build_example "$source" "/ref/${project##*:}"
+  cp "$name.bin" "$stage/$name.bin"
+  symbols "$name"
+  applications+=("$name.bin" "$name.nm")
+done
+
+if [ "${#unpinned[@]}" -ne 0 ]; then
+  echo "error: ${unpinned[*]}: no pinned hashes yet (the lines after \"unpinned\" above give those of this build); build a second time from a clean cache, and when both builds give the same hashes write each block to the file it names, in tools/reference-firmware/; nothing was written to the output directory" >&2
+  exit 1
+fi
 
 if ! (cd "$stage" && sha256sum -c /ref/SHA256SUMS.output); then
   (cd "$stage" && sha256sum flash_hello.bin flash_nvs.bin flash_nvs_ota.bin) >&2
@@ -91,5 +131,5 @@
   echo "error: a merged image is not the pinned one (hashes above); nothing was written to the output directory" >&2
   exit 1
 fi
-results=(flash_hello.bin flash_nvs.bin flash_nvs_ota.bin bootloader.nm hello_world.nm nvs_rw_value_cxx.nm)
+results=(flash_hello.bin flash_nvs.bin flash_nvs_ota.bin bootloader.nm hello_world.nm nvs_rw_value_cxx.nm "${applications[@]}")
 (cd "$stage" && chown "$HOST_UID:$HOST_GID" "${results[@]}" && mv "${results[@]}" /out/)
```

Create `tools/reference-firmware/usb_serial_jtag_console/CMakeLists.txt`:

```
# SPDX-License-Identifier: Apache-2.0
# A reference firmware of inkemu: see main/usb_serial_jtag_console_main.c.
cmake_minimum_required(VERSION 3.16)
include($ENV{IDF_PATH}/tools/cmake/project.cmake)
project(usb_serial_jtag_console)
```

Create `tools/reference-firmware/usb_serial_jtag_console/main/CMakeLists.txt`:

```
# SPDX-License-Identifier: Apache-2.0
idf_component_register(SRCS "usb_serial_jtag_console_main.c"
                       PRIV_REQUIRES esp_driver_usb_serial_jtag vfs)
```

Create `tools/reference-firmware/usb_serial_jtag_console/main/usb_serial_jtag_console_main.c`:

```c
/*
 * SPDX-License-Identifier: Apache-2.0
 *
 * A reference firmware of inkemu, written for its tests (no ESP-IDF example reads stdin over the
 * USB-Serial-JTAG console with a small footprint): USB-Serial-JTAG is the primary console, stdin
 * and stdout go through its driver, and every line read is written back after "echo: ". The
 * console only works while a USB host reads the port: without the start-of-frame interrupts of a
 * host the output is discarded and stdin reads end, which is what its tests check.
 */
#include <stdio.h>
#include "esp_err.h"
#include "driver/usb_serial_jtag.h"
#include "driver/usb_serial_jtag_vfs.h"

void app_main(void)
{
    usb_serial_jtag_driver_config_t config = USB_SERIAL_JTAG_DRIVER_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(usb_serial_jtag_driver_install(&config));
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

Create `tools/reference-firmware/usb_serial_jtag_console/sdkconfig.defaults`:

```
CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y
```

- [ ] **Step 4: Check the scripts**

```bash
bash -n tools/reference-firmware/in-container.sh > /tmp/t10-sh.log 2>&1; echo "exit=$?"
bash -n tools/reference-firmware/build.sh > /tmp/t10-sh2.log 2>&1; echo "exit=$?"
cargo test -p console-script --test reference_firmware > /tmp/t10.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t10.log
```

Expected: `exit=0`, `exit=0`, then `exit=0` and `test result: ok. 0 passed; 0 failed; 5 ignored; 0 measured; 0 filtered out`.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy -p console-script --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add tools/reference-firmware/usb_serial_jtag_console crates/console-script/tests/reference_firmware.rs
/usr/bin/git commit -m "test: three reference firmwares read their consoles, and the build merges an example's own configuration after the reference one" -- Cargo.lock README.md .github/workflows/ci.yml crates/console-script tools/reference-firmware
```

Expected: three `exit=0`, then the commit.

- [ ] **Step 6: Pin the three builds, from the CI**

Push. The cache key of `reference-boot` covers `tools/reference-firmware/**`, so the job builds. Its step "Build the reference firmware" fails with `error: uart_select usb_serial_jtag_echo usb_serial_jtag_console: no pinned hashes yet …`; above it, each project printed a line `unpinned SHA256SUMS.inputs-…:` followed by three `sha256sum` lines (`bootloader/bootloader.bin`, `partition_table/partition-table.bin`, `<name>.bin`). The `bootloader.bin` and `partition-table.bin` of `uart_select` must be `601b4845…` and `7f00b6c0…`, the pinned ones (UR §9.1 measured them in the same container); if not, stop and find out why before pinning anything. Write each block of three lines to the file it names, in `tools/reference-firmware/`, exactly as printed, and commit: `/usr/bin/git add tools/reference-firmware/SHA256SUMS.inputs-uart-select tools/reference-firmware/SHA256SUMS.inputs-usj-echo tools/reference-firmware/SHA256SUMS.inputs-usj-console; /usr/bin/git commit -m "test: pin the builds of uart_select, usb_serial_jtag_echo and usb_serial_jtag_console" -- tools/reference-firmware`. The key changed again, so the next run is a second clean build: it must pass `sha256sum -c` for all five projects. If it does not, the build is not reproducible: say so in the hand-over note and fix the cause (UR §9.1 measured one build of `uart_select` only), never the pinned values.

- [ ] **Step 7: Read the milestones in the CI**

In the same job, the step "Boot the reference firmware" runs the five tests. A failure prints the console it saw: compare it with the report before changing anything. For milestone 1 the expected lines are UR §9.2 and the window of the first timeout (4,990 to 5,020 ms after `main_task: Returned from app_main()`, by the timestamps of the two lines) is UR's "5 s of device time"; for milestone 2 the 7 rows of `Recv str:` and the address of the fifth are USJ §F.1. A difference is a finding about the model or about the report: fix the model with a unit test that fails before the fix, or correct the expectation with the reason in the hand-over note; never widen a window to make it pass.

---

## Task 11: ADR 0006, the README, and the hand-over

ADR 0006 records the two new inputs of a run and amends ADR 0004 (C6, C12). The README says how to type into the consoles, in English then in French, and changes what it said of a console that was output only. The hand-over note records what this plan did, its assumptions, and what it hands on. The cartography of the project (outside the repository) is updated.

**Files:**
- Create: `docs/adr/0006-entrees-des-consoles.md`, `docs/superpowers/notes/2026-10-14-sp1b-3d-carryover.md` (dated the day it is written)
- Modify: `docs/adr/0004-entrees-d-une-execution-et-fichier-de-flash.md`, `README.md`

- [ ] **Step 1: The ADR and the README**

````diff
--- a/README.md
+++ b/README.md
@@ -138,7 +138,7 @@
 - **Two limits, two clocks.** `--max-cycles N` stops the run when the CPU has executed N cycles of its clock; `--max-time S` stops it when S seconds of device time have elapsed since power-on (a decimal number: `30`, `2.5`, `0.001`). `--max-cycles` counts the cycles the CPU executes, not device time: a sleeping CPU executes none while device time goes on, as it does across resets. A firmware that sleeps therefore reaches that limit late (the reference image waits on a timer for eleven seconds and still stops at `--max-cycles 50000000`), and one whose sleep nothing interrupts never does; `--max-time` bounds both. Both may be given; the first one reached ends the run, with exit code 0. The stop is exact and the same on every host: the first instruction boundary at or after the limit, or the limit itself when the CPU is asleep. A firmware that waits for an interrupt nothing can raise ends the run by itself, with exit code 4.
 - **Ctrl-C and `kill`.** Without a limit a run ends with Ctrl-C, which prints the same summary as a limit, with exit code 0. A second Ctrl-C before the run begins its summary exits at once, with code 130 and no summary, and the flash file is the one of before. Once the summary has begun (its first line says why the run ended; the warnings above it are not part of it), Ctrl-C ends nothing: the summary is printed to its end, the flash file is saved, and the exit code is that of the end of the run (0 when the first Ctrl-C ended it). A plain `kill` is not handled and ends the process without a summary either. With `--trace-hash`, the hash of an interrupted run covers what ran up to the cut, which depends on host timing: it is not reproducible; the hash of a run ended by a limit is.
 - **The watchdogs are real.** Without `--flash`, the ROM prints `invalid header: 0xffffffff` until the watchdog the chip arms at reset expires, 1.3 s later, and boots again with `rst:0x7 (TG0WDT_SYS_RST)`, as a blank chip does. A firmware that stops feeding its watchdogs is reset the same way, and the next boot names the watchdog and the address it was caught at (`Saved PC`). inkemu's own `reset by …` line gives the cause, the program counter and the stack pointer of every reset, also after a system reset, where the ROM prints no `Saved PC`.
-- **The console is as slow as the UART, and output only.** A byte leaves at the baud rate the firmware programmed; when a run stops, what was still in the UART is printed too. `--console` chooses which serial output of the device goes to standard output: `uart0` (the default), `uart1` or `usb-serial-jtag`; the ROM prints on the first and on the last. Nothing can be sent to the device yet: there is no UART reception and no host side of USB-Serial-JTAG (sub-project 1b-3d).
+- **The console is as slow as the line.** A UART byte lasts the frame the firmware programmed (start bit, data bits, parity, stop bits) at its baud rate; a USB-Serial-JTAG packet reaches the host 50 µs after the firmware commits it, if a host reads the port. When a run stops, what was still on its way is printed too. `--console` chooses which serial output of the device goes to standard output: `uart0` (the default), `uart1` or `usb-serial-jtag`; the ROM prints on the first and on the last. What the device receives comes from a console script (below), never from the terminal: typing makes sense only when device time follows the clock on the wall, which inkemu does not do yet.
 - **Pipes and redirections.** `inkemu run … | head -5` ends when `head` has its lines: a run whose reader went away says so and ends with exit code 0. On Unix this also holds when the firmware prints nothing more: every so often, after a slice that had nothing to print, inkemu asks whether its reader is still there, and a reader that left is noticed after 64 such slices at most (a few milliseconds to half a second on the machine it was measured on, depending on what the firmware does). A run that ends by itself in the meantime ends as what it is, with the exit code of that end. On other systems such a silent run goes on to its limit or to Ctrl-C. When standard output cannot be written for another reason (a full disk), the run ends with exit code 2; a `--strict-mmio` stop keeps its code whatever happened to the console. A closed standard output (`>&-`) cannot be told from `/dev/null`, because the Rust runtime reopens it on the null device before the program starts: it behaves like `/dev/null`, the console is dropped and the run ends as it would have, with exit code 0.
 - **What is not emulated yet.** Deep sleep and light sleep: a firmware that asks for either keeps running. Sub-project 1b-3f models deep sleep and the wake-up by the RTC timer, and refuses light sleep by name. There is no radio, no display and no button yet. The line of standard error that names the device says `timing model datasheet-uncalibrated (not calibrated)`: the cycle cost of each instruction comes from documents, not from a measurement on a device, so a device time is plausible, not exact (the calibration bench is sub-project 8).
 - **The flash is written as a NOR chip is.** A program can only clear bits (each byte becomes the AND of the old one and the new one) and wraps to the start of its 256-byte page; an erase sets a 4 KiB sector, a 64 KiB block or the whole chip to `0xFF`; both need the write-enable latch, which the chip clears when the operation completes. On the first boot of a layout with OTA slots and a blank `otadata` partition, which is the layout of the Xteink X4, the bootloader erases and programs `otadata`, and the next boots find it. Every operation completes at once: an erase that takes a tenth of a second on a device takes no time here (real durations come with the calibration bench, sub-project 8). What the model does not do is refused by name, never guessed. The chip refuses a command it does not have, a command in another frame than its own (each command has one: its address width, its dummy clocks and its data, so a write enable sent with an address is refused as well), an address past the end of the chip, and a program or an erase while a block-protect bit of the status register is set. The controller refuses a transfer whose data phase is longer than 64 bytes or is not a whole number of bytes, a hard-wired program of no byte or of more than 64, a suspend request, and several command bits in one write of `SPI1.CMD`. A refused command reads nothing and changes nothing. The status register is not part of the flash file: it starts from its factory value at every run.
@@ -192,7 +192,7 @@
 - first, what was done before the run could start, when something was: the download of a ROM, the removal of what sat at the temporary name of the flash file;
 - then the device, the timing model and the flash in use;
 - as the run goes, one `warning:` line for each thing it cannot do faithfully or that the firmware got wrong — an unmodelled register read or write (with the peripheral, the register, its address and the program counter), an 8- or 16-bit write to a peripheral register, an operation it cannot perform faithfully (a flash command the chip model does not have, or one it has, sent in another frame than its own or at an address past the end of the chip; a program or an erase while a block-protect bit is set; a flash suspend request; several flash command bits written at once; an SPI flash transfer whose command is not 8 bits long, or whose data phase is longer than 64 bytes or not a whole number of bytes; a hard-wired program of no byte or of more than 64; SHA through DMA; a SHA mode the chip does not have; a clock source it cannot turn into a frequency; an RTC slow clock other than the RC oscillator; a byte written to a UART whose transmit FIFO is full; a UART byte sent on a source clock it has no frequency for, such as the RC fast clock), an access to flash that no MMU entry maps, a read through a flash window that may differ from the device, a CPU exception (named in words, with its program counter and stack pointer) — and one `inkemu:` line for each reset, which is an event of the run and not a warning;
-- at the end, why the run stopped, where (program counter, cycles, instructions), the device time elapsed and the CPU clock, followed by `; the CPU is asleep, waiting for an interrupt` when it is, then the most repeated lines with their count, the trace hash when it was asked for, and the flash: the SHA-256 of the content the run started from, then what became of it. With a flash file: saved, with its SHA-256 and the number of 4 KiB sectors modified; left as it was, when the run started from the file and modified nothing; or written again, when that file was removed while the run went on. Without one: the number of sectors modified and not kept, and no line at all when nothing was modified.
+- at the end, why the run stopped, where (program counter, cycles, instructions), the device time elapsed and the CPU clock, followed by `; the CPU is asleep, waiting for an interrupt` when it is, then the state of the USB host and, with a console script, how far it got, then the most repeated lines with their count, the trace hash when it was asked for, and the flash: the SHA-256 of the content the run started from, then what became of it. With a flash file: saved, with its SHA-256 and the number of 4 KiB sectors modified; left as it was, when the run started from the file and modified nothing; or written again, when that file was removed while the run went on. Without one: the number of sectors modified and not kept, and no line at all when nothing was modified.
 
 Each distinct warning and each distinct reset is listed once, however often it comes. `--strict-mmio` turns the first warning into a stop with exit code 3, for CI; CPU exceptions and resets are excepted, because they are what the firmware does on the device. Registers that inkemu stores without modelling their effect are listed in [docs/registers-accepted-without-effect.md](docs/registers-accepted-without-effect.md).
 
@@ -201,6 +201,32 @@
 The first run downloads the ROM (4.9 MB) from Espressif's `esp-rom-elfs` release `20260528` and checks its SHA-256 before using it. To work offline, extract the archive yourself and pass `--rom-dir <directory>`. `inkemu run` reads only the ROM of its device's chip revision from that directory: `esp32c3_rev3_rom.elf` for `--device x4` and `--device x3`. `inkemu rom fetch --all [--rom-dir <directory>]` fills a directory in advance, with a single download, with the three ROMs of the supported chips (`esp32c3_rev0_rom.elf`, `esp32c3_rev3_rom.elf` and `esp32c3_rev101_rom.elf`); `inkemu rom fetch --device x4` fetches just the one that device needs.
 
 Other commands: `inkemu inspect <app.bin>` describes an ESP-IDF application image and explains what is wrong when it refuses a file; `inkemu licenses` prints the notices of the third-party material the binary embeds as data; `inkemu rvtest <elf>` runs a bare-metal RISC-V conformance test. For the pinned conformance suites and how to run riscv-arch-test against Sail, see [tools/conformance/README.md](tools/conformance/README.md).
+
+### Typing into the consoles
+
+A console script says what the host sends to the device, and when, in device time: two runs with the same script are the same run, to the trace hash.
+
+```text
+inkemu-console-script 1
+# Wait for the prompt of the firmware, then type a command and Enter.
+wait-for uart0 "login: "
+send uart0 "root\r"
+wait 2s
+# Pull the USB cable, then plug it back and open the port.
+usb-host absent
+wait 100ms
+usb-host open
+wait-for usb-serial-jtag "ready"
+```
+
+```sh
+inkemu run --device x4 --firmware app.bin --console-script steps.txt --max-time 30
+```
+
+- **Steps**, one per line, in order: `send CHANNEL "BYTES"` sends at the time of the step; `wait 250ms` (units `ns`, `us`, `ms`, `s`) moves that time on; `wait-for CHANNEL "TEXT"` moves it to the device time at which the last byte of `TEXT` reached the host, searched in what the channel sent from the time of the step on; `usb-host absent|attached|open` changes the USB host at that time. `CHANNEL` is `uart0`, `uart1` or `usb-serial-jtag`. Strings take `\r`, `\n`, `\t`, `\\`, `\"` and `\xHH`; any other character is sent as its UTF-8 bytes, and nothing is translated: Enter is `\r`, as a terminal sends it. `#` begins a comment.
+- **The host** of the UARTs sends at the rate and in the frame the UART is set to. On USB-Serial-JTAG it sends 64-byte packets; it is `open` unless `--usb-host` says otherwise: `attached` is a cable without a program reading the port (the device sees frames, its output is not read), `absent` no cable.
+- **Refusals**: a script is read whole before the run starts, and every refusal names its line and its column, then what to write instead. The limits are those of `crates/console-script/src/parse.rs` (1 MiB of script, 10,000 steps, 1,024 bytes a send, 256 bytes a text to wait for).
+- **The end**: the summary says how far the script got. A run that would exit 0 exits 5 when its script was not through: it was still waiting for a text, or one of its steps is dated after the end of the run.
 
 ## Why
 
@@ -247,6 +273,7 @@
 - [ADR 0003](docs/adr/0003-contexte-des-peripheriques.md): the context every peripheral access receives, device time, and the three levels of reset (French)
 - [ADR 0004](docs/adr/0004-entrees-d-une-execution-et-fichier-de-flash.md): what an input of a run is, and the life of the flash file (French)
 - [ADR 0005](docs/adr/0005-bootloader-embarque.md): the 2nd-stage bootloader inkemu ships, chosen by the device profile and replaceable (French)
+- [ADR 0006](docs/adr/0006-entrees-des-consoles.md): what the host sends to the consoles and the USB host, inputs of a run dated in device time (French)
 
 ## Français
 
@@ -280,17 +307,26 @@
 - **Deux limites, deux horloges.** `--max-cycles N` arrête l'exécution quand le CPU a exécuté N cycles de son horloge ; `--max-time S` l'arrête quand S secondes du temps de l'appareil se sont écoulées depuis la mise sous tension (un nombre décimal : `30`, `2.5`, `0.001`). `--max-cycles` compte les cycles que le CPU exécute, non le temps de l'appareil : un CPU qui dort n'en exécute aucun alors que le temps de l'appareil continue, comme à travers les resets. Un firmware qui dort atteint donc cette limite tard (l'image de référence attend un timer pendant onze secondes et s'arrête tout de même à `--max-cycles 50000000`), et celui dont rien n'interrompt le sommeil ne l'atteint jamais ; `--max-time` borne les deux. Les deux peuvent être données ; la première atteinte termine l'exécution, avec le code de sortie 0. L'arrêt est exact et le même sur tout hôte : la première frontière d'instruction à la limite ou après, ou la limite elle-même quand le CPU dort. Un firmware qui attend une interruption que rien ne peut lever termine l'exécution de lui-même, avec le code 4.
 - **Ctrl-C et `kill`.** Sans limite, une exécution se termine par Ctrl-C, qui affiche le même résumé qu'une limite, avec le code 0. Un second Ctrl-C avant le début du résumé sort aussitôt, avec le code 130 et sans résumé, et le fichier de flash reste celui d'avant. Une fois le résumé commencé (sa première ligne dit pourquoi l'exécution s'est terminée ; les avertissements au-dessus d'elle n'en font pas partie), Ctrl-C ne termine plus rien : le résumé est affiché jusqu'au bout, le fichier de flash est enregistré, et le code de sortie est celui de la fin de l'exécution (0 quand c'est le premier Ctrl-C qui l'a terminée). Un simple `kill` n'est pas géré et termine le processus sans résumé non plus. Avec `--trace-hash`, l'empreinte d'une exécution interrompue couvre ce qui a tourné jusqu'à la coupure, qui dépend de l'hôte : elle n'est pas reproductible ; celle d'une exécution terminée par une limite l'est.
 - **Les watchdogs sont réels.** Sans `--flash`, la ROM affiche `invalid header: 0xffffffff` jusqu'à l'expiration du watchdog que la puce arme au reset, 1,3 s plus tard, et redémarre avec `rst:0x7 (TG0WDT_SYS_RST)`, comme une puce vierge. Un firmware qui cesse de nourrir ses watchdogs est réinitialisé de la même façon, et le démarrage suivant nomme le watchdog et l'adresse où il a été pris (`Saved PC`). La ligne `reset by …` d'inkemu donne la cause, le compteur de programme et le pointeur de pile de chaque reset, y compris après un reset système, où la ROM n'affiche pas de `Saved PC`.
-- **La console est aussi lente que l'UART, et en sortie seulement.** Un octet part au débit que le firmware a programmé ; quand une exécution s'arrête, ce qui restait dans l'UART est affiché aussi. `--console` choisit quelle sortie série de l'appareil va sur la sortie standard : `uart0` (par défaut), `uart1` ou `usb-serial-jtag` ; la ROM écrit sur la première et sur la dernière. Rien ne peut encore être envoyé à l'appareil : ni réception UART, ni côté hôte de l'USB-Serial-JTAG (sous-projet 1b-3d).
+- **La console est aussi lente que la ligne.** Un octet UART dure la trame que le firmware a programmée (bit de start, bits de données, parité, bits de stop) à son débit ; un paquet USB-Serial-JTAG atteint l'hôte 50 µs après que le firmware l'a validé, si un hôte lit le port. Quand une exécution s'arrête, ce qui était encore en route est affiché aussi. `--console` choisit quelle sortie série de l'appareil va sur la sortie standard : `uart0` (par défaut), `uart1` ou `usb-serial-jtag` ; la ROM écrit sur la première et sur la dernière. Ce que l'appareil reçoit vient d'un script de console (ci-dessous), jamais du terminal : taper n'a de sens que si le temps d'appareil suit l'horloge murale, ce qu'inkemu ne fait pas encore.
 - **Tubes et redirections.** `inkemu run … | head -5` se termine quand `head` a ses lignes : une exécution dont le lecteur est parti le dit et finit avec le code 0. Sous Unix, c'est vrai aussi quand le firmware n'affiche plus rien : de temps en temps, après une tranche qui n'avait rien à afficher, inkemu demande si son lecteur est encore là, et un lecteur parti est remarqué après 64 de ces tranches au plus (de quelques millisecondes à une demi-seconde sur la machine où cela a été mesuré, selon ce que fait le firmware). Une exécution qui finit d'elle-même entre-temps finit comme telle, avec le code de sortie de cette fin. Ailleurs, une telle exécution muette continue jusqu'à sa limite ou jusqu'à Ctrl-C. Quand la sortie standard ne peut pas être écrite pour une autre raison (un disque plein), l'exécution finit avec le code 2 ; un arrêt par `--strict-mmio` garde son code quoi qu'il soit arrivé à la console. Une sortie standard fermée (`>&-`) ne se distingue pas de `/dev/null`, parce que l'exécutable Rust la rouvre sur le périphérique nul avant que le programme ne commence : elle se comporte comme `/dev/null`, la console est abandonnée et l'exécution finit comme elle l'aurait fait, avec le code 0.
 - **Ce qui n'est pas encore émulé.** Le sommeil profond et le sommeil léger : un firmware qui demande l'un ou l'autre continue de tourner. Le sous-projet 1b-3f modélise le sommeil profond et le réveil par le timer RTC, et refuse le sommeil léger en le nommant. Il n'y a encore ni radio, ni écran, ni bouton. La ligne de l'erreur standard qui nomme l'appareil dit `timing model datasheet-uncalibrated (not calibrated)` : le coût en cycles de chaque instruction vient de documents, pas d'une mesure sur un appareil, donc un temps d'appareil est plausible, pas exact (le banc de calibration est le sous-projet 8).
 - **La flash s'écrit comme une puce NOR.** Une programmation ne peut que mettre des bits à 0 (chaque octet devient l'ET de l'ancien et du nouveau) et reboucle au début de sa page de 256 octets ; un effacement met à `0xFF` un secteur de 4 Kio, un bloc de 64 Kio ou toute la puce ; les deux exigent le verrou d'écriture, que la puce fait retomber à la fin de l'opération. Au premier démarrage d'une disposition à emplacements OTA dont la partition `otadata` est vierge, celle du Xteink X4, le bootloader efface puis programme `otadata`, et les démarrages suivants la retrouvent. Chaque opération s'achève aussitôt : un effacement qui prend un dixième de seconde sur un appareil ne prend ici aucun temps (les durées réelles viennent avec le banc de calibration, sous-projet 8). Ce que le modèle ne fait pas est refusé en le nommant, jamais deviné. La puce refuse une commande qu'elle n'a pas, une commande dans une autre trame que la sienne (chaque commande en a une : sa largeur d'adresse, ses cycles d'attente et ses données ; une autorisation d'écriture envoyée avec une adresse est donc refusée elle aussi), une adresse au-delà de la fin de la puce, et une programmation ou un effacement quand un bit de protection de bloc du registre d'état est à 1. Le contrôleur refuse un transfert dont la phase de données dépasse 64 octets ou n'est pas un nombre entier d'octets, une programmation câblée de zéro octet ou de plus de 64, une demande de suspension, et plusieurs bits de commande dans une même écriture de `SPI1.CMD`. Une commande refusée ne lit rien et ne change rien. Le registre d'état n'est pas dans le fichier de flash : il repart de sa valeur d'usine à chaque exécution.
 - **Une lecture qui peut différer de l'appareil est signalée.** inkemu lit la flash à chaque accès par les fenêtres du cache ; un appareil lit son cache, qui garde les octets qu'il a chargés jusqu'à ce que le firmware l'invalide. Quand un firmware lit de la flash par une fenêtre, la programme ou l'efface, puis la relit sans invalider le cache, inkemu renvoie les nouveaux octets et avertit qu'un appareil peut encore renvoyer les anciens (`read of the flash window at … may differ from the device`) ; `--strict-mmio` s'y arrête. L'avertissement se prononce par page de flash de 64 Kio, non par ligne de cache : c'est pourquoi il dit « peut ». Aucun chemin lu ou mesuré de la ROM, du bootloader ou d'ESP-IDF ne le déclenche, et les images de référence (`hello_world`, et l'exemple NVS sous sa propre table de partitions et sous celle du X4) s'exécutent sans lui en mode strict ; l'invalidation que fait ESP-IDF après une écriture dans de la flash projetée n'est tenue que par des tests unitaires, aucune image de référence ne l'exerce.
 - **La protection mémoire n'est pas appliquée.** Les entrées PMP, les registres de permission et le garde-pile sont mémorisés et ne lèvent jamais de faute (sous-projet 1b-4).
-- **Codes de sortie** : 0 quand l'exécution s'est terminée normalement (limite de cycles ou limite en temps de l'appareil atteinte, Ctrl-C, ou lecteur de la sortie standard parti) ; 2 pour une erreur (appareil inconnu, ROM absente ou invalide, image flash ou fichier de flash refusé, sortie standard impossible à écrire, fichier de flash impossible à écrire à la fin de l'exécution) ; 3 pour un arrêt par `--strict-mmio` ; 4 quand le CPU attend une interruption et qu'aucune ne peut venir ; 130 pour un second Ctrl-C pressé avant le début du résumé (une fois le résumé commencé, Ctrl-C ne termine plus rien).
-
-La console de l'appareil va sur la sortie standard. Tout ce qu'inkemu a à dire va sur l'erreur standard : d'abord ce qui a été fait avant que l'exécution puisse commencer, quand quelque chose l'a été (le téléchargement d'une ROM, la suppression de ce qui occupait le nom temporaire du fichier de flash) ; puis l'appareil, le modèle de temps et la flash utilisée ; au fil de l'exécution, une ligne `warning:` pour chaque chose qu'il ne sait pas faire fidèlement ou que le firmware a mal faite (lecture ou écriture d'un registre non modélisé, écriture de 8 ou 16 bits dans un registre de périphérique, opération qu'il ne peut pas exécuter fidèlement, accès à de la flash qu'aucune entrée MMU ne projette, lecture par une fenêtre de flash qui peut différer de l'appareil, exception CPU) et une ligne `inkemu:` pour chaque reset, qui est un événement de l'exécution et non un avertissement ; à la fin, la raison de l'arrêt, l'endroit, le temps d'appareil écoulé et l'horloge du CPU, puis les lignes les plus répétées, l'empreinte de trace si elle est demandée, et la flash : le SHA-256 du contenu de départ, puis ce qu'il est devenu. Avec un fichier de flash : enregistré, avec son SHA-256 et le nombre de secteurs de 4 Kio modifiés ; laissé tel quel, quand l'exécution est partie du fichier et n'a rien modifié ; ou réécrit, quand ce fichier a été supprimé pendant l'exécution. Sans fichier : le nombre de secteurs modifiés et non conservés, et aucune ligne quand rien n'a été modifié. Chaque avertissement et chaque reset distinct n'est listé qu'une fois. `--strict-mmio` fait du premier avertissement un arrêt avec le code 3, pour la CI ; les exceptions CPU et les resets font exception, parce qu'ils sont ce que le firmware fait sur l'appareil. Les registres mémorisés sans que leur effet soit modélisé sont listés dans [docs/registers-accepted-without-effect.md](docs/registers-accepted-without-effect.md).
+- **Codes de sortie** : 0 quand l'exécution s'est terminée normalement (limite de cycles ou limite en temps de l'appareil atteinte, Ctrl-C, ou lecteur de la sortie standard parti) ; 2 pour une erreur (appareil inconnu, ROM absente ou invalide, image flash ou fichier de flash refusé, sortie standard impossible à écrire, fichier de flash impossible à écrire à la fin de l'exécution) ; 3 pour un arrêt par `--strict-mmio` ; 4 quand le CPU attend une interruption et qu'aucune ne peut venir ; 5 quand l'exécution aurait fini avec 0 mais que son script de console n'est pas allé au bout ; 130 pour un second Ctrl-C pressé avant le début du résumé (une fois le résumé commencé, Ctrl-C ne termine plus rien).
+
+La console de l'appareil va sur la sortie standard. Tout ce qu'inkemu a à dire va sur l'erreur standard : d'abord ce qui a été fait avant que l'exécution puisse commencer, quand quelque chose l'a été (le téléchargement d'une ROM, la suppression de ce qui occupait le nom temporaire du fichier de flash) ; puis l'appareil, le modèle de temps et la flash utilisée ; au fil de l'exécution, une ligne `warning:` pour chaque chose qu'il ne sait pas faire fidèlement ou que le firmware a mal faite (lecture ou écriture d'un registre non modélisé, écriture de 8 ou 16 bits dans un registre de périphérique, opération qu'il ne peut pas exécuter fidèlement, accès à de la flash qu'aucune entrée MMU ne projette, lecture par une fenêtre de flash qui peut différer de l'appareil, exception CPU) et une ligne `inkemu:` pour chaque reset, qui est un événement de l'exécution et non un avertissement ; à la fin, la raison de l'arrêt, l'endroit, le temps d'appareil écoulé et l'horloge du CPU, puis l'état de l'hôte USB et, avec un script de console, jusqu'où il est allé, puis les lignes les plus répétées, l'empreinte de trace si elle est demandée, et la flash : le SHA-256 du contenu de départ, puis ce qu'il est devenu. Avec un fichier de flash : enregistré, avec son SHA-256 et le nombre de secteurs de 4 Kio modifiés ; laissé tel quel, quand l'exécution est partie du fichier et n'a rien modifié ; ou réécrit, quand ce fichier a été supprimé pendant l'exécution. Sans fichier : le nombre de secteurs modifiés et non conservés, et aucune ligne quand rien n'a été modifié. Chaque avertissement et chaque reset distinct n'est listé qu'une fois. `--strict-mmio` fait du premier avertissement un arrêt avec le code 3, pour la CI ; les exceptions CPU et les resets font exception, parce qu'ils sont ce que le firmware fait sur l'appareil. Les registres mémorisés sans que leur effet soit modélisé sont listés dans [docs/registers-accepted-without-effect.md](docs/registers-accepted-without-effect.md).
 
 Pour construire soi-même les images de référence (il faut Docker ; rien n'est exécuté sur l'hôte), voir [tools/reference-firmware/README.md](tools/reference-firmware/README.md). Les tests (`cargo test --workspace`) se lancent sous un utilisateur ordinaire, pas sous `root` : certains ont besoin d'un fichier ou d'un répertoire que leur utilisateur ne peut pas écrire, et échouent sous `root` avec un message qui le dit. Le premier lancement télécharge la ROM (4,9 Mo) depuis la version `20260528` d'`esp-rom-elfs` d'Espressif et vérifie son SHA-256 avant de s'en servir ; pour travailler hors ligne, extraire l'archive soi-même et passer `--rom-dir <répertoire>`, ou remplir un répertoire à l'avance avec `inkemu rom fetch --all [--rom-dir <répertoire>]`. Autres commandes : `inkemu inspect <app.bin>` décrit une image d'application ESP-IDF et explique ce qui ne va pas quand il refuse un fichier ; `inkemu rvtest <elf>` exécute un test de conformité RISC-V sans système ([tools/conformance/README.md](tools/conformance/README.md)).
+
+### Taper dans les consoles
+
+Un script de console dit ce que l'hôte envoie à l'appareil, et quand, en temps d'appareil : deux exécutions avec le même script sont la même exécution, jusqu'à l'empreinte de trace. Le format est celui de l'exemple de la partie anglaise, ci-dessus, et s'utilise avec `inkemu run … --console-script <fichier>`.
+
+- **Les étapes**, une par ligne, dans l'ordre : `send CANAL "OCTETS"` envoie à l'instant de l'étape ; `wait 250ms` (unités `ns`, `us`, `ms`, `s`) avance cet instant ; `wait-for CANAL "TEXTE"` le porte à l'instant d'appareil où le dernier octet de `TEXTE` a atteint l'hôte, cherché dans ce que le canal a envoyé depuis l'instant de l'étape ; `usb-host absent|attached|open` change l'hôte USB à cet instant. `CANAL` vaut `uart0`, `uart1` ou `usb-serial-jtag`. Les chaînes acceptent `\r`, `\n`, `\t`, `\\`, `\"` et `\xHH` ; tout autre caractère est envoyé en ses octets UTF-8, et rien n'est traduit : Entrée s'écrit `\r`, comme un terminal l'envoie. `#` commence un commentaire. La première ligne est `inkemu-console-script 1`.
+- **L'hôte** des UART envoie au débit et dans la trame de l'UART. Sur l'USB-Serial-JTAG, il envoie des paquets de 64 octets ; il est `open` sauf si `--usb-host` dit autre chose : `attached` est un câble sans programme qui lit le port (l'appareil voit les trames, sa sortie n'est pas lue), `absent` l'absence de câble.
+- **Les refus** : un script est lu en entier avant le départ, et chaque refus donne sa ligne et sa colonne, puis ce qu'il faut écrire. Les limites sont celles de `crates/console-script/src/parse.rs` (1 Mio de script, 10 000 étapes, 1 024 octets par envoi, 256 octets par texte attendu).
+- **La fin** : le résumé dit jusqu'où le script est allé. Une exécution qui sortirait avec 0 sort avec 5 quand son script n'est pas allé au bout : il attendait encore un texte, ou l'une de ses étapes est datée après la fin de l'exécution.
 
 ### Pourquoi
 
@@ -318,7 +354,7 @@
 
 ### Documentation
 
-La [spécification](docs/superpowers/specs/2026-10-11-inkemu-design.md), les ADR [0002](docs/adr/0002-flash-model-and-cache-windows.md) (modèle de la flash et fenêtres de cache) [0003](docs/adr/0003-contexte-des-peripheriques.md) (contexte des périphériques, temps de l'appareil et resets) [0004](docs/adr/0004-entrees-d-une-execution-et-fichier-de-flash.md) (entrées d'une exécution et fichier de flash) et [0005](docs/adr/0005-bootloader-embarque.md) (bootloader embarqué) sont en français ; la [recherche sur le matériel Xteink](docs/research/xteink-hardware.md) et la [table des registres acceptés sans effet](docs/registers-accepted-without-effect.md) sont en anglais.
+La [spécification](docs/superpowers/specs/2026-10-11-inkemu-design.md), les ADR [0002](docs/adr/0002-flash-model-and-cache-windows.md) (modèle de la flash et fenêtres de cache) [0003](docs/adr/0003-contexte-des-peripheriques.md) (contexte des périphériques, temps de l'appareil et resets) [0004](docs/adr/0004-entrees-d-une-execution-et-fichier-de-flash.md) (entrées d'une exécution et fichier de flash), [0005](docs/adr/0005-bootloader-embarque.md) (bootloader embarqué) et [0006](docs/adr/0006-entrees-des-consoles.md) (entrées des consoles et hôte USB) sont en français ; la [recherche sur le matériel Xteink](docs/research/xteink-hardware.md) et la [table des registres acceptés sans effet](docs/registers-accepted-without-effect.md) sont en anglais.
 
 ### Matériel tiers et licence
 
````

```diff
--- a/docs/adr/0004-entrees-d-une-execution-et-fichier-de-flash.md
+++ b/docs/adr/0004-entrees-d-une-execution-et-fichier-de-flash.md
@@ -18,7 +18,7 @@
    - Les options qui changent ce que la machine fait ou l'endroit où l'exécution est coupée : `--strict-mmio`, `--max-cycles`, `--max-time`.
 
    Rien d'autre n'entre dans une exécution : ni l'heure de l'hôte, ni le chemin ou l'existence du fichier de flash, ni son écriture, ni la console relayée (`--console` choisit ce qui est montré, pas ce qui est exécuté). Ctrl-C et le départ du lecteur de la sortie coupent l'exécution à un endroit qui dépend de l'hôte : une telle exécution n'est pas reproductible, ce que disait déjà `RunEnd::Interrupted`.
-   Les entrées que le firmware ne peut pas encore recevoir (octets reçus sur une console, boutons, carte SD) s'ajouteront à cette liste avec les plans qui les apportent (SP1b-3d et suivants), datées en temps d'appareil.
+   Les entrées que le firmware ne peut pas encore recevoir (octets reçus sur une console, boutons, carte SD) s'ajouteront à cette liste avec les plans qui les apportent (SP1b-3d et suivants), datées en temps d'appareil. SP1b-3d en a ajouté deux, l'hôte USB et les octets envoyés aux consoles : voir l'[ADR 0006](0006-entrees-des-consoles.md).
 2. **Le contenu de la flash au départ est nommé par son SHA-256, que le résumé de l'exécution imprime.** L'empreinte de trace est le chemin du CPU (la suite des adresses exécutées), pas ses données : deux exécutions parties de deux contenus différents ont la même empreinte de trace quand le firmware suit le même chemin (un test le montre avec un compteur de démarrages). L'empreinte de trace et le SHA-256 du contenu de départ identifient une exécution ensemble, pas séparément. Le contenu final est une fonction des entrées : son SHA-256 est imprimé quand un fichier est écrit, et un test vérifie que deux exécutions parties du même fichier laissent le même fichier.
 3. **Le fichier de flash est demandé, brut et exact.** `--flash-file <chemin>` le désigne ; sans cette option rien n'est conservé, et le résumé dit combien de secteurs de 4 Kio ont été modifiés et perdus. Le fichier est l'image de la puce et rien d'autre : pas d'en-tête, exactement la taille de la flash du profil, donc lisible par `esptool.py` et comparable par `sha256sum`. Un fichier d'une autre taille est refusé avec les deux tailles ; il n'est jamais complété ni tronqué, même sous `--reset-flash` : un fichier d'une autre taille n'est pas la flash de cet appareil, et une image de firmware donnée par erreur à la mauvaise option doit survivre. Ce qui se trouve au chemin sans être un fichier ordinaire (répertoire, lien symbolique) est refusé, et un lien n'est pas suivi.
 4. **L'image de `--flash` n'est jamais écrite.** Elle est l'image de départ : ce dont part une exécution quand aucun fichier de flash n'existe encore. Donnée avec un fichier de flash qui existe, elle est refusée avant le démarrage, car l'exécution ne peut pas partir de deux flashs ; `--reset-flash` lève le refus : l'exécution repart de l'image de départ (ou d'une flash effacée) et le fichier est remplacé à la fin. Les deux options ne peuvent pas désigner le même fichier, puisque l'une ne fait que lire et que l'autre remplace : l'exécution est alors refusée avant le démarrage, avant toute lecture de l'image et toute création de fichier, avec ou sans `--reset-flash`, que le fichier soit atteint par le même chemin, par une autre écriture du chemin, par un lien symbolique ou par un lien matériel (même périphérique et même inode sous Unix ; chemins canoniques ailleurs, où un lien matériel n'est pas vu). Sans ce refus, `--reset-flash` remplacerait l'image par ce que l'exécution laisse. Le remède proposé n'est jamais `--reset-flash` : retirer `--flash` pour continuer depuis le fichier, ou donner un autre chemin à `--flash-file` pour garder l'image telle quelle. Pour la même raison, l'image ne peut pas être le fichier temporaire du fichier de flash (`.<nom>.tmp`) ni son fichier de verrou (`<nom>.lock`), par aucune de ces voies : une exécution acceptée supprime ce qui se trouve au nom temporaire puis y écrit, et toute exécution ouvre et verrouille le fichier de verrou. Le nom temporaire est justement celui sous lequel un enregistrement interrompu peut laisser la flash entière, le fichier qu'un utilisateur donnerait à `--flash` pour la retrouver ; sans ce refus, l'exécution supprimait cette image avant sa première instruction. Le refus vient au même moment que celui du même fichier, avant toute création, et propose de copier l'image sous un autre nom.
```

Create `docs/adr/0006-entrees-des-consoles.md`:

```markdown
# ADR 0006 — Entrées des consoles et hôte USB

- Date : 2026-10-13
- Statut : accepté (plan SP1b-3d)
- Amende : ADR 0004, point 1 (les entrées d'une exécution)

## Contexte

Jusqu'à SP1b-3c, les consoles de l'appareil n'allaient que vers l'hôte. Un firmware qui lit sa console (un invite, un menu, la console de CrossPoint) attendait pour toujours : l'UART n'avait pas de réception, l'USB-Serial-JTAG pas d'hôte. Or l'hôte USB fait partie de ce que voit le firmware : sans trames de début (SOF), le moniteur de connexion d'ESP-IDF et le `HWCDC` d'Arduino jugent qu'aucun hôte n'est là et jettent leur sortie (rapport `docs/research/usb-serial-jtag-host.md`, §B.3 et §B.5).
L'ADR 0004 a dit ce qui entre dans une exécution et annoncé que les octets reçus sur une console s'y ajouteraient, datés en temps d'appareil. Ce qu'un hôte envoie dépend souvent de ce que l'appareil a dit (attendre l'invite avant de taper) : une date fixée d'avance ne suffit pas, et une date lue sur l'horloge de l'hôte ferait dépendre l'exécution de la vitesse de la machine qui la fait tourner.

## Décision

1. **Deux entrées s'ajoutent aux quatre de l'ADR 0004.**
   - **L'hôte USB** de l'USB-Serial-JTAG : `absent` (pas de câble), `attached` (un câble, le port fermé : des trames, rien n'est lu) ou `open` (un programme lit et écrit le port), à la mise sous tension (`--usb-host`, `open` par défaut), puis ses changements datés.
   - **Les octets envoyés** à une console (`uart0`, `uart1`, `usb-serial-jtag`), par morceaux datés.

   Les deux entrent dans la machine par des méthodes d'`Esp32C3` (`send_console`, `set_usb_host`), qui les rangent dans les périphériques comme des événements datés : le modèle ne lit rien sur l'hôte. Une date antérieure au temps d'appareil est refusée par une erreur typée qui donne les deux instants ; rien n'est alors rangé. Une console qui garde déjà un mébioctet en attente refuse le morceau suivant, et la machine garde au plus 16 384 changements d'hôte à venir : une interface qui fournit des entrées sans fin reçoit une erreur et non une mémoire qui grossit.
2. **La date d'une entrée est fixée par les entrées seules, jamais par la façon dont l'exécution est découpée.** Un script de console (`--console-script`) peut dater une étape de l'instant où un texte a atteint l'hôte (`wait-for`). Pour que cet instant soit exact, les canaux qu'un script attend sont « observés » (`watch_console`) : chaque octet qui atteint l'hôte sur un canal observé arrête l'appel de la machine à la frontière d'instruction où il est parti (à son instant même quand le CPU dort), et le temps d'appareil est alors celui de l'octet. Les étapes suivantes sont datées de cet instant et remises toutes à la machine avant qu'elle reprenne. Un test exécute le même script en tranches de 1, de 7 919 et d'un million de cycles, et compare les consoles, l'empreinte de trace et le temps d'appareil.
3. **Le terminal n'est pas une entrée.** Taper au clavier n'a de sens que si le temps d'appareil suit l'horloge murale, ce qu'inkemu ne fait pas : la commande ne relaie pas l'entrée standard. L'interface graphique (SP2) fournira ses octets par la même méthode, datés de l'instant présent de la machine, et dira qu'une telle exécution ne se rejoue qu'avec son enregistrement (SP1b-4).
4. **La console relayée reste une sortie.** `--console` choisit ce qui est montré, pas ce qui est reçu ni exécuté (ADR 0004, point 1).
5. **Un script dit jusqu'où il est allé.** Le résumé de l'exécution imprime l'état de l'hôte USB à la fin et la progression du script ; une exécution qui sortirait avec 0 sort avec 5 quand son script n'est pas allé au bout (un texte encore attendu, une étape datée après la fin). Les autres codes sont gardés : ils en disent plus sur l'exécution.

## Conséquences

- **Enregistrement et rejeu (SP1b-4)** : un enregistrement porte, en plus des entrées de l'ADR 0004, l'état de l'hôte USB à la mise sous tension et la liste datée des entrées des consoles et des changements d'hôte, telle que la machine les a reçues (pas le script, dont les dates dépendent de l'exécution). Un rejeu les remet à la machine d'un coup, au départ.
- **Le script n'est pas le format des scénarios.** Sa première ligne, `inkemu-console-script 1`, le désigne ; les scénarios YAML du sous-projet 4 (boutons, écran, consoles) pourront le remplacer sans ambiguïté.
- **L'hôte d'une UART envoie au débit et dans la trame de l'UART.** Un hôte réel peut se tromper de débit ; ce cas (octets corrompus, erreurs de trame) n'est pas modélisé, et l'hypothèse est écrite dans le code.
- **Les empreintes de trace des exécutions qui écrivent sur l'USB-Serial-JTAG changent.** La ROM recopie sa console sur l'USB-Serial-JTAG ; avant SP1b-3d, l'extrémité IN était toujours libre, elle attend désormais que l'hôte prenne le paquet. Les empreintes épinglées par les tests ont été remesurées avec ce changement (note de passation de SP1b-3d).
- **Numérotation** : l'ADR suivant prend le numéro 0007.
```

- [ ] **Step 2: Check the README against the binary**

```bash
cargo test -p emu-cli --test run_cli -- readme > /tmp/t11.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t11.log
```

Expected: `exit=0` and `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 89 filtered out`: the help block of the README is the one Task 8 wrote.

- [ ] **Step 3: The hand-over note**

Create `docs/superpowers/notes/2026-10-14-sp1b-3d-carryover.md` (the date of the day it is written, in its name and its first line) with the sections below. The values marked « relevé dans la CI » are those Steps 7 of Task 5 and 6–7 of Task 10 read: write them, with the job and step of the log they come from.

```markdown
# SP1b-3d — consoles série dans les deux sens — note de passation

Date : 2026-10-14. Plan : `docs/superpowers/plans/2026-10-13-sp1b-3d-serial-consoles.md`. ADR 0006.

## Résultat

- Un firmware lit sa console sur UART0, UART1 et USB-Serial-JTAG : réception UART (FIFO de 128 octets, débordement signalé), interruptions UART (sources 21 et 22) et USB-Serial-JTAG (source 26), durée d'une trame selon `CONF0` dans les deux sens, hôte USB `absent`, `attached` ou `open` avec SOF à chaque milliseconde.
- Les entrées d'une exécution comptent l'hôte USB et les octets envoyés aux consoles, datés en temps d'appareil (ADR 0006). Un script de console (`--console-script`, crate `console-script`) les fournit ; un `wait-for` date les étapes suivantes de l'instant exact où son texte a atteint l'hôte, quel que soit le découpage de l'exécution.
- `inkemu run` gagne `--console-script`, `--usb-host` et le code de sortie 5.
- Jalons dans la CI : `uart_select`, `usb_serial_jtag_echo`, `usb_serial_jtag_console`, `hello_world` (copie USB de la console), déterminisme sous deux découpages (`crates/console-script/tests/reference_firmware.rs`).

## Valeurs remesurées

- Empreintes de trace des deux exécutions épinglées du firmware de référence : `10174cc2726c3d77` → (relevé dans la CI) et `c6b24a1cc226ed4a` → (relevé dans la CI), avec leurs temps d'appareil et nombres d'instructions. Cause : la ROM recopie sa console sur l'USB-Serial-JTAG et attend désormais que l'hôte prenne chaque paquet (50 µs avec un hôte ouvert). Les notes de SP1b-3b et SP1b-3c citent les anciennes valeurs : ce sont des documents datés.
- Les empreintes des trois nouvelles constructions (`SHA256SUMS.inputs-uart-select`, `-usj-echo`, `-usj-console`), relevées dans la CI sur deux constructions propres.

## Hypothèses (`ASSUMPTION:`)

Quatorze lignes nouvelles (87 → 101 dans les fichiers `.rs`) :
- UART : l'octet qui trouve la FIFO pleine est perdu, les 128 restent (UR A2) ; `STOP_BIT_NUM` = 0 dure un bit de stop (UR A4) ; l'hôte envoie au débit et dans la trame de l'UART ; `RXFIFO_TOUT` demande un octet dans la FIFO et une ligne au repos, une fois par période de repos (UR A3) ; une condition qui tient encore après `INT_CLR` relève son bit (UR A1) ; un octet qui arrive horloge de réception coupée ou FIFO en remise à zéro est perdu (UR A8) ; une lecture de la FIFO vide rend 0 (UR A6).
- USB-Serial-JTAG : les registres d'état des terminaisons valent leur valeur de reset, personne ne les lit (USJ §A) ; l'hôte prend un paquet 50 µs après sa validation (USJ O.5) ; un hôte au port fermé émet des trames et ne prend rien ; un hôte branché à la mise sous tension commence ses trames à 0 sans reset de bus ; un reset du cœur remet le bloc à zéro (voir ci-dessous) ; `WR_DONE` pendant qu'un paquet attend ne fait rien ; une lecture de la FIFO OUT vide rend 0 ; un octet écrit sans place est ignoré.

## Décisions prises pendant l'écriture du plan

- Le reset du cœur remet l'USB-Serial-JTAG à zéro. Le rapport USJ (O.12) lisait le tableau de IR §4 comme « le bloc est conservé » ; la ligne qui le conserve est celle du reset du CPU (`esp_restart`, `rst:0xc`), et inkemu n'appelle `UsbSerialJtag::reset` qu'au reset du cœur.
- Les autres : voir le tableau des décisions du plan (C13 à C30).

## Décisions prises pendant l'exécution

(Ce que l'exécution a changé au plan, avec le test qui le tient ; « aucune » s'il n'y en a pas.)

## Reporté, par héritier

- **SP1b-3e (CrossPoint)** : le modèle d'hôte USB sert tel quel au `HWCDC` d'Arduino ; vérifier sur CrossPoint que la première interruption vient de `SERIAL_IN_EMPTY` à sa valeur de reset et que le moniteur voit les SOF.
- **SP2 (interface)** : une console interactive (taper au clavier) avec un temps d'appareil cadencé sur l'horloge murale, par `Esp32C3::send_console` à l'instant présent de la machine ; un interrupteur d'hôte USB (`set_usb_host`) ; l'affichage de la progression d'un script (`ScriptRun::status`). Les règles de `flash_state.rs` restent à déplacer (note de SP1b-3c).
- **SP1b-4 (enregistrement et rejeu)** : porter l'état de l'hôte à la mise sous tension et la liste datée des entrées des consoles et des changements d'hôte, telle que la machine les a reçues (ADR 0006) ; la liste blanche du lockstep contre QEMU (UR §10 : QEMU livre les octets d'un coup, sans débordement, et lève un `RXFIFO_TOUT` sur FIFO vide).
- **SP4 (scénarios)** : les scénarios YAML remplacent le format `inkemu-console-script 1`, qu'ils désignent par sa ligne de version.
- **SP8 (temps)** : un hôte UART à un autre débit (octets corrompus, erreurs de trame) ; la latence de l'hôte USB mesurée sur un vrai hôte.
```

- [ ] **Step 4: The cartography**

Update the cartography files of the project (`~/.claude/projects/-home-rony-projects-xteink-emulator/memory/`, outside the repository, not committed): `architecture_overview.md` (the two new inputs, `watch_console`/`ConsoleOutput`, the new crate), `backend_models.md` (`UsbHost`, `InputError`, `UartFeature`, the script types, `Consoles`/`Bounds`/`Driven`), `backend_services.md` (UART reception and interrupts, USB-Serial-JTAG and its host, `console-script`), `backend_controllers.md` and `api_routes.md` (`--console-script`, `--usb-host`, exit code 5, the new ignored tests and their counts: `real_rom` 18, `run_cli` 27 and 8, `reference_boot` 13, `reference_firmware` 5), `integrations.md` (the three reference firmwares and their variables), `tech_debt.md` (the heirs above), `MEMORY.md` (the index lines).

- [ ] **Step 5: Commit**

```bash
/usr/bin/git add docs/adr/0006-entrees-des-consoles.md docs/superpowers/notes/2026-10-14-sp1b-3d-carryover.md
/usr/bin/git commit -m "docs: ADR 0006 makes the console input and the USB host inputs of a run, the README says how to type into the consoles, and the hand-over of SP1b-3d" -- docs/adr README.md docs/superpowers/notes
```

Expected: the commit.
