# SP1b-3a — Time base, interrupt delivery, timers, watchdogs and reset: the application runs its whole life, restarts, and boots a second time — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `inkemu run --device x4 --flash <image>` runs the reference firmware through its whole life (`Hello world!`, the countdown, `Restarting now.`), through `esp_restart`, and into a second boot that the ROM announces with `rst:0xc (RTC_SW_CPU_RST)` and its `Saved PC:` line; the seconds the application spends in `vTaskDelay` are slept, not executed; in `--strict-mmio` nothing unmodelled is touched and no CPU exception is taken over the whole cycle; without a flash image the hardware watchdogs reset the chip as they do on a real one; a firmware that hangs with interrupts masked is reset by its own watchdog; two identical runs give the same trace hash across the reset.

**Architecture:** One mechanism replaces every side channel between a device model and the rest of the machine: each register access receives a context (`soc_esp32c3::ctx::Ctx`, owned by the bus) through which a peripheral reads device time, drives its interrupt source lines, asks to be called at an instant, and posts what the machine must act on (an unsupported operation, a clock change, a reset request, an event for another device). Device time is counted in cycles of one fixed 160 MHz time base of which every modelled clock is a whole multiple; the CPU adds `cycles × period` after each instruction, and the scheduler of `emu-core` (unchanged, keyed by cycles) holds the wake-ups. A peripheral is never polled: it is called at the instant it asked for and works out what is due then, so a wake-up that outlived a reset or a feed finds nothing to do. Counters (SYSTIMER, timer groups, RTC timer, watchdogs) are a value at an instant and a rate, never ticked. The interrupt controller follows the chip (pending iff enabled, asserted, priority ≠ 0 and ≥ threshold; highest priority, lowest line), presents one line to the hart, which takes it when `mstatus.MIE` is set; `wfi` moves device time to the next wake-up. A reset has three run-time levels (CPU, core, system); each returns to its reset state exactly what the reports tabulate, leaves the memories alone, and records its cause for the ROM to print. The design is the text of ADR 0003, committed by Task 1.

**Tech Stack:** Rust 1.99.0 (edition 2024); one new dependency, `ctrlc` 3.5 (MIT OR Apache-2.0; its Unix dependency `nix` is MIT), in `emu-cli` only; `clap`, `thiserror`, `assert_cmd`/`predicates` (tests); the reference firmware and its pins of plan SP1b-2 (`tools/reference-firmware`, Docker `espressif/idf:v5.5.2`), unchanged.

**Spec:** `docs/superpowers/specs/2026-10-11-inkemu-design.md` (§3.1, §3.2, §4.1, §5.2, §6.4, §7, §8, §10.2 items 5 to 7, §10.3 criterion 4). Hardware facts: `docs/research/esp32c3-interrupts-and-reset.md` (**IR**), `docs/research/esp32c3-timers-and-watchdogs.md` (**TW**), `docs/research/flash-writes-and-crosspoint-boot.md` (**FW**, only for what this plan leaves ready), and the reports of the earlier plans: `esp32c3-boot-chain.md` (**BC**), `esp32c3-bootloader-accesses.md` (**BA**), `esp32c3-cache-mmu.md` (**CM**), `esp32c3-rom-flash-boot.md` (**FB**), `sp1b-test-oracle.md` (**TO**). ADR `docs/adr/0002-flash-model-and-cache-windows.md`. Carried-over points: `docs/superpowers/notes/2026-10-11-sp1b-2-carryover.md`. A value in this plan is followed by the report tag and section it comes from; `ROM3:name@addr` is a symbol of the rev3 mask ROM, `APP:` and `IDF:` a symbol of the reference application and a file of ESP-IDF v5.5.2.

Every code block of this plan was compiled and its tests run in a scratch copy of the repository, in the order of the tasks: after each task the workspace built, `cargo test --workspace` passed and `clippy` was clean; the ignored tests were run against the real ROM and the reference image after Task 2 and after the tasks that say so (8 to 13, 16). The numbers the tests pin (test counts, cycle counts, device times, console lines) are the ones that copy produced.

## Global Constraints

- Toolchain pinned: Rust **1.99.0**, edition **2024**. Workspace lints unchanged: `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `unsafe_code = "forbid"`; `unwrap_used`/`expect_used` denied outside tests (an invariant is documented with `// INVARIANT:`).
- No `TODO`, no placeholder, no `#[allow]` added to hide a lint.
- Coverage ≥ 80 % for the workspace (`cargo llvm-cov --workspace --fail-under-lines 80`).
- Licence Apache-2.0; `cargo deny check` passes; permissive licences only. The one new dependency is added at the version this plan names, in `[workspace.dependencies]` (Task 15).
- Register addresses and names come from `svd-regs` (`svd_regs::esp32c3::addr("PERIPHERAL", "REGISTER")`), never from a literal. A register the SVD does not contain gets **one named constant with its source in a comment**.
- Every user-facing message states **the cause, then the remedy**, separated by a semicolon.
- Untrusted input (anything the firmware writes, a flash image) is bounded and never panics.
- Determinism: same inputs → same trace hash. No wall clock, no host randomness, no `HashMap` iteration on the emulated path. The only thing a run takes from the host is the request to stop (Ctrl-C), read between two slices and never part of a trace.
- `soc-esp32c3` performs **no host I/O** and builds for `wasm32-unknown-unknown`. `emu-core` depends on no workspace crate.
- Third-party material (ROM, firmware, bootloader, QEMU) stays out of the repository and of the binary, and is **never executed on the host**: inkemu is the only thing that executes the reference image.
- Each modelled hardware behaviour cites its source in a comment (report tag + section, ROM symbol, ESP-IDF file and line). For every hardware bit a model gives a meaning to, the comment names one call site in the ROM or in ESP-IDF that sets it. An assumed behaviour is marked `ASSUMPTION:` with what would settle it, and is listed in the hand-over to SP1b-4 (Task 17).
- Unmodelled registers (spec §5.2): read = SVD reset value, write ignored, one deduplicated warning naming peripheral, register, address and PC; `--strict-mmio` stops the machine (exit code 3).
- Source files are changed with the editor (read, then edit the named lines), never by a script that rewrites them.
- Checking a command: redirect its output to a file, then read `$?`; never read an exit code through a pipe.
- Commits: `type(scope): description`, only the paths of the task (`/usr/bin/git commit -- <paths>`), message ending with the line `Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ`.
- Formatting and lints before **every** commit: `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`, all three `exit=0`, whether or not the task lists them. The code blocks of this plan are formatted as `rustfmt` leaves them.
- A task that changes a list of registers accepted without effect regenerates the table with `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect`, reads the diff of `docs/registers-accepted-without-effect.md`, and commits that file with the task.
- The ignored tests need `INKEMU_ROM_ELF_DIR` (the pinned esp-rom-elfs archive, `inkemu rom fetch`) and, for the reference image, `INKEMU_REF_FLASH` (`tools/reference-firmware/build.sh`). A task that says "ignored tests" runs `tools/ci/run-ignored-tests.sh` with the counts `.github/workflows/ci.yml` has at that point; a task that adds an ignored test updates the count in the same commit.
- Documents for people (the ADR, the carry-over note) are in French, with all accents, as `docs/adr/0002-*.md`. README, code, CLI output and this plan are in English.

## Review Focus

- **A firmware that writes to the UART faster than it drains, or resets it mid-line** (a byte written while the 128-byte FIFO is full, a module reset with bytes in flight, a FIFO reset): the byte is dropped as on the chip and named in a diagnostic, strict mode stops, the console never shows a byte twice or out of order → tests `a_byte_written_to_a_full_fifo_is_dropped_and_reported`, `a_reset_loses_the_bytes_that_have_not_left_and_keeps_those_that_have`, `the_transmit_fifo_reset_keeps_only_the_byte_being_sent`, `bytes_leave_one_at_a_time_at_the_baud_rate_in_order_and_only_once` (Task 9).
- **A wake-up that outlives the state it was scheduled for** (a watchdog fed, stopped or reprogrammed, an alarm moved, a peripheral or the whole core reset while its deadline is in the scheduler): the stale call finds nothing to do, nothing fires early, nothing fires twice → tests `a_deadline_keeps_the_earliest_call_and_replaces_a_later_one` (Task 3), `a_wake_up_for_a_state_that_is_gone_finds_nothing_to_do` (Task 7), `a_watchdog_fed_at_every_tick_never_expires`, `the_protection_follows_the_apb_clock_and_stops_when_its_bit_is_cleared` (Task 13), `a_core_reset_does_not_restart_the_watchdog` (Task 10).
- **A watchdog used outside the sequences ESP-IDF runs** (writes while it is locked, a timeout lowered below the present count, a stage whose action is "off", a sleep that covers two stages, a prescaler of 0, the clock gate closed): each has one defined, tested behaviour, and the ones no document settles are `ASSUMPTION:` → tests `a_locked_watchdog_ignores_feeds_and_configuration`, `a_sleep_that_covers_two_stages_runs_both_in_order`, `a_cpu_reset_stage_and_an_off_stage_do_what_their_action_says`, `the_watchdog_clock_gate_stops_the_counter` (Task 13), `a_locked_watchdog_ignores_feeds_and_a_feed_needs_bit_31` (Task 10), `dividers_and_degenerate_rates_never_divide_by_zero` (Task 7).
- **A slow clock the model does not have** (the firmware selects the 32 kHz crystal or the divided fast RC for `RTC_SLOW_CLK`, or calibrates a crystal that is not fitted): the selection is refused by name and the RTC keeps counting the RC oscillator; the calibration ends in `TIMEOUT`, as ESP-IDF expects of a board without the crystal, instead of spinning for ever → tests `a_slow_clock_other_than_the_rc_oscillator_is_refused_by_name` (Task 10), `a_calibration_of_the_missing_32_khz_crystal_times_out_without_a_result` (Task 13).
- **A CPU that goes to sleep and never wakes** (no interrupt can come; or devices keep counting but their interrupt is masked; or a device wakes the machine a hundred thousand times without raising anything; or the user presses Ctrl-C): the run ends with a named reason and exit code 4 when nothing is scheduled, and otherwise always gives control back to its caller, so a limit or Ctrl-C can end it with the usual summary → tests `wfi_with_nothing_to_wake_it_stops_as_idle`, `a_cpu_that_sleeps_while_a_device_counts_gives_control_back_after_a_second`, `a_storm_of_wake_ups_without_an_interrupt_gives_control_back` (Task 8), `a_cpu_asleep_while_a_watchdog_counts_does_not_end_the_run` (Task 13), `an_interrupted_run_ends_with_the_summary_of_a_limit_stop` (Task 15).

---

## Design decisions taken for this plan

E1–E16 are the framing decisions (binding). E17 onward were taken while writing the plan.

| # | Decision | Reason | Cost if wrong |
|---|---|---|---|
| E1 | **One mechanism replaces the side channels.** A peripheral access receives a context through which it reads device time, drives interrupt source lines, schedules an event, posts an effect. `take_unsupported` ×3, `take_clock_change`, `latch_mmu_fault`/`latch_store` are migrated by Tasks 1, 2 and 4, behaviour unchanged, before the first interrupt source. ADR 0003. | SP1b-2 carry-over, "à défaire" items 1 and 2. | Low: the mechanism is six small types (`Ctx`, `Source`, `Event`, `Group`, `Module`, `Deadline`). |
| E2 | Events are scheduled in **device time**, not CPU cycles; device time is monotonic across resets. | SYSTIMER counts at 16 MHz whatever the CPU clock (20 → 80 → 160 MHz). | Medium: see E17 for how it is realised. |
| E3 | Interrupt delivery follows the chip's controller: pending iff enabled, raw asserted, priority ≠ 0 and ≥ threshold; taken when `mstatus.MIE`; highest priority first, ties to the lowest line; vectored `(mtvec & !0xFF) + 4n`, `mcause = 0x8000_0000 \| n`; level lines follow the source, edge lines latch. `mie`/`mip` stay read/write CSRs that gate nothing (`ASSUMPTION:`, IR §7 question 5). The generic hart of `inkemu rvtest` keeps the standard behaviour. | IR §1, §2. | Medium: the SP1b-4 lockstep compares every interrupt. |
| E4 | `wfi` idles by advancing time to the next scheduled event; it ends when a line is pending by the controller's rule, whatever `mstatus.MIE` (`ASSUMPTION:`, IR §7 question 4). With no event and nothing pending the run ends with a named stop reason, exit code 4. | Ten seconds of `vTaskDelay` must cost almost nothing. | Low. |
| E5 | Resets have levels; each resets and keeps exactly what the reports tabulate; the cause is in `RTC_CNTL.RESET_STATE`; `ASSIST_DEBUG` keeps the PC for `Saved PC`. The `OPTIONS0` software-reset bits now reset (D27 of SP1b-2 is undone) and the rest of that write is no longer dropped. | TW §D, IR §4. | Medium: open points are `ASSUMPTION:`. |
| E6 | Watchdogs are real: MWDT0, MWDT1, RWDT and the super watchdog count on their own clocks, with stages, actions, write-protect keys and feeds; the flash-boot protection is armed at power-on by the reset values. Brown-out stays storage. | Spec §10.3 criterion 4. | Medium. |
| E7 | Timers are real: SYSTIMER (two units, three comparators, one-shot and period alarms, the update/valid protocol, interrupts), the general-purpose timer of each group, the RTC slow-clock calibration, the 48-bit RTC counter. The slow clock runs at its nominal frequency (`ASSUMPTION:`, TW §I question 2). | TW §A to §C. | Medium. |
| E8 | The CPU runs at 20 MHz after power-on (TRM §6.2.4.1) and at 40 MHz in the ROM after a software CPU reset. | TW summary item 3, §H.2; IR §4.1. | Low. |
| E9 | Interrupt latency: 4 CPU cycles (TRM §1.6.1; IR §2.7). The timing model stays "uncalibrated". | The TRM figure is definite. | Low: see E22. |
| E10 | UART transmit time is modelled (`FSM_STATUS`, `TXFIFO_CNT` follow), because `esp_restart` waits for the drain. Reception and the USB-Serial-JTAG host side are SP1b-3b. | IR §4.1. | Low. |
| E11 | A run interrupted by Ctrl-C prints the same summary as a limit stop, exit code as for the cycle limit. `--max-time` is **not** added: spec §6.4 and §10.2 name `--max-cycles` only. | Framing. | See E21. |
| E12 | A CPU exception is logged with its PC and the stack pointer, once per distinct cause and PC. | SP1b-1 carry-over. | Nil. |
| E13 | PMP is enforced only if it fits in one task and the measured boots keep passing. **Outcome: it stays stored** (E31). Either way a reset clears it. | Framing. | Low. |
| E14 | The table of registers accepted without effect shrinks, the pinning test proves it, and every remaining row names its heir. | Spec §7. | Nil. |
| E15 | Not in this plan (→ SP1b-3b): flash program/erase/read through SPI1 and the per-device flash file; the third term of `page_version`; `--firmware` and its ADR; the USB-Serial-JTAG host connection; `DeviceProfile` id as a validated type; partition tables in profiles. Cache-error interrupts are in this plan only if their lines are ones the reference application enables: they are (lines 24 to 27, IR §5), see E33. | Framing. | — |
| E16 | Reference firmware, pins, `rom-boot` and `reference-boot` CI jobs are reused; test counts in `ci.yml` follow every task that adds an ignored test. The lockstep cannot cross a reset (IR summary item 9): recorded for SP1b-4. | Framing. | — |
| E17 | Device time is counted in **cycles of a fixed 160 MHz time base** (`clock::TIME_BASE`, 6.25 ns). Every modelled clock is a whole number of them: CPU at 160 MHz (1), 80 MHz (2), the crystal (4) and its divisions (4 × n), SYSTIMER (10); the RC oscillators are exact ratios (136 kHz is 17 counts per 20 000 cycles). `emu_core::Scheduler` stays keyed by `Cycles`; `VirtualClock` is deleted. | Spec §3.1 says time is measured in cycles and events are stamped in cycles: this is the reading of E2 closest to it. A change of CPU clock changes one multiplier; no deadline is re-derived. | Low: a clock that is not a whole multiple would need a finer base, one constant. |
| E18 | The context is a concrete `Ctx` owned by the bus; the trait of `emu-core` becomes `Peripheral<C>` (`read(&mut self, ctx: &mut C, offset)`), so `emu-core` still knows no SoC. The `tick` of spec §3.2 is realised as a call at the instant the peripheral asked for (`Event` + `Peripherals::deliver`), not as a polled method. | A polled `tick` on 20 devices per instruction is the cost the scheduler exists to avoid. | Low. |
| E19 | **A wake-up is a call, not an order.** A device called by a wake-up works out what is due at `ctx.now()` and re-arms; it keeps one `Deadline`. Nothing cancels the wake-ups of a device that is reset: they find nothing to do. | Removes all cancellation bookkeeping from resets and reprogramming. | Low: a spurious call costs one function call. |
| E20 | The run loop compares device time with the earliest wake-up after each instruction (one integer comparison); there is no cycle budget to convert or re-derive. | Simpler than E2's wording and equivalent; exact to the instruction. | Nil. |
| E21 | The CPU cycle counter stands still in `wfi` (TRM register 1.12: "the counter stops when the CPU is in WFI"; TW §H.3). `run_until` therefore returns a new `StopReason::Asleep` after 1 s of device time slept, or 100 000 wake-ups without an interrupt, within one call: the caller regains control, checks Ctrl-C, calls again. | Without it a sleeping CPU never reaches a cycle limit and never returns. | Medium: `--max-cycles` does not bound a firmware that sleeps for ever while a device counts (only Ctrl-C does). SP4's scripted runs need a device-time limit: a spec change request, recorded in the carry-over note. |
| E22 | The 4 cycles of E9 delay a line that **becomes** selected; a line that is masked or released disappears at once (`ASSUMPTION:`). | The TRM gives one figure and no direction. | Low. The SP1b-4 lockstep runs against an oracle with no latency: it needs the constant to be configurable (carry-over). |
| E23 | A reset takes place at the end of the instruction that asked for it. After `esp_restart` the recorded PC is therefore the instruction after the store, in the ROM (`0x40048b92`); a real chip prints an application address (`ASSUMPTION:`, IR §7 question 3). | The diagnosable option: the line is printed, with a value that names the store. | Low: one console line per restart. |
| E24 | Three run-time levels, `ResetLevel::{Cpu, Core, System}`. The fourth of E5, the chip reset of a power-on, is the construction of the machine: nothing asks for one while it runs. | Removes a variant nothing would produce. | Nil. |
| E25 | A reset clears the integer registers (`ASSUMPTION:`), builds new CSRs (PMP entries and their locks cleared, `mpccr` back to 0: TW §H.3) and a new decode cache; the cycle and instruction totals and device time go on. | `--max-cycles`, the trace and the summary stay monotonic. | Low. |
| E26 | A watchdog configuration register acts when it is written; `CONF_UPDATE_EN` reads 0 and changes nothing (`ASSUMPTION:`). | TW §B.3 describes a synchronisation on that bit, but `_timer_ll_reset_register` clears `FLASHBOOT_MOD_EN` with a plain read-modify-write right after a peripheral reset, "to prevent the system from being reset unexpectedly" (`IDF:hal/esp32c3/include/hal/timer_ll.h:50-66`): that code only works if the bit acts at once. | Low. |
| E27 | A watchdog stage expires when its counter is at or past its timeout (`ASSUMPTION:`, TW §I question 5). | A timeout lowered below the count then fires, which is visible; equality would hide it until the next feed. | Low. |
| E28 | The super watchdog is real under three assumptions: it runs once `FIB_SEL` bit 2 is cleared and neither `AUTO_FEED_EN`, `DISABLE` nor `BYPASS_RST` is set; it asks to be fed after 0.9 s and resets the system after 1 s (cause `0x12`). | E6 asks for it; TW §J gives no period. The measured boots keep it auto-fed, so the assumptions cannot change them. | Low. |
| E29 | `RTC_SLOW_CLK` is the RC oscillator at 136 kHz. A write that selects another source is refused by name (`Unsupported::SlowClock`) and the RC keeps counting. A calibration of the 32 kHz crystal ends in `TIMEOUT`: none is fitted (`ASSUMPTION:`). | No silent wrong duration. | Low. |
| E30 | A UART byte leaves after 10 bit times of the programmed divider and clock; the console receives it when its last bit has gone. The FIFO holds 128 bytes plus the one being sent; a byte written beyond that is dropped and reported (`Unsupported::UartOverflow`). A reset of the module loses the bytes still in flight. | E10; the ROM and ESP-IDF both wait for room, so the overflow is a firmware bug worth naming. | Low. |
| E31 | PMP stays **stored, not enforced**; the stack guard of `ASSIST_DEBUG` and the `SENSITIVE` permission registers are storage. Heir: SP1b-4. | Enforcement needs a check on every access that the decode cache must not bypass, and nothing in the milestone can show a fault being delivered: more than one task, and a risk to milestone point 3. | Low. |
| E32 | `Unsupported::SoftwareReset` disappears (the reset happens); `SlowClock` and `UartOverflow` are added. | E5, E29, E30. | Nil. |
| E33 | The two cache-error sources (36 illegal operation, 61 access) are driven by `EXTMEM` from its latched status and its enable registers. | The reference application routes them to line 25 and enables them (IR §5): the condition of E15 is met. | Low. |
| E34 | The word at `EFUSE + 0x18C` reads 0 (`ASSUMPTION:`). | The application reads it at every start (`APP:esp_efuse_utility_check_errors`, the load at `0x4200045c`) although no register is documented there; the strict run found it. | Low. |
| E35 | Ctrl-C is caught with `ctrlc` 3.5; a second Ctrl-C ends the process at once with code 130. | A run that does not come back to a slice boundary must still be stoppable. | Low. |
| E36 | A word fetched through a flash-window page no MMU entry maps is not kept decoded (`Bus::fetch_is_memory`). | SP1b-2 carry-over, "à défaire" item 3: kept, it would be reported and latched once however often it runs. | Nil. |
| E37 | Counters are a value at an instant and a rate (`periph::counter`), shared by SYSTIMER, the timer groups, the RTC timer and the watchdogs. | One arithmetic, tested once; nothing is counted cycle by cycle. | Low. |
| E38 | On the ESP32-C3 hart `mtvec.MODE` reads 1 and `BASE` is bits 31:8; bits 7:2 read 0 (`ASSUMPTION:`). | IR §2.2: the ROM writes `0x40001D00` and reads `0x40001D01`. | Low. |
| E39 | The tests that pinned the end of SP1b-2 ("the strict run ends on `TIMG0.RTCCALICFG`") are replaced by the life-cycle tests in the task that models that register (Task 13), not in a later one. | A test whose premise a task removes is rewritten by that task: the suite passes after every task. | Nil. |
| E40 | Deep sleep, light sleep and the RTC timer wake-up (spec §10.2 item 7) are not in this plan. Heir: **SP1b-3b**, with the first CrossPoint code that sleeps (confirmed by the owner of the roadmap). | Neither the milestone nor the framing of 3a names them; the registers they use stay accepted rows that say so. | Medium: until SP1b-3b a firmware that enters deep or light sleep finds its sleep registers stored and is never put to sleep. |
| E41 | The UART asks to be called when its transmitter goes idle (`Event::Uart`), and at the end of a run the console also receives the bytes still in the UART (`Esp32C3::flush_console`, used by the CLI and by the test helpers). | Two consequences of E10 that the replay of the tasks found: without the first, a CPU in `wfi` with bytes on their way out is declared idle and the bytes never leave; without the second, a run that stops (strict stop, limit, Ctrl-C) shows a console that ends before the place it stopped at, and the SP1b-2 tests that end on a strict stop lose the first line of the application. | Low: the flush ends the transmission, so a machine is not run on after it when output timing matters. |

## Carried-over points of SP1b-2: where they are handled

Source: `docs/superpowers/notes/2026-10-11-sp1b-2-carryover.md`.

| Point | Handled |
|---|---|
| First registers: `TIMG0.RTCCALICFG/1/2` | **Task 13** |
| First registers: `SYSTIMER.UNIT0_OP` and the rest of the block | **Task 7** |
| First registers: RTC timer `TIME_UPDATE`, `TIME_LOW0`, `TIME_HIGH0` | **Task 10** |
| First registers: `SYSTEM.CPU_INTR_FROM_CPU_0` | **Task 8** |
| Flash writes; SPI1 data reads `0x03`/`0x0B`/`0xBB`; per-device flash file; live view against stale cache lines (D4); `otadata` | **SP1b-3b** |
| `--firmware`, and the ADR on the embedded bootloader (spec §3.4) before it | **SP1b-3b** |
| Software reset (`OPTIONS0` bits 31 and 5, causes) | **Tasks 10, 11** |
| A reset clears the PMP (lock bits), keeps the flash, `STORE0..7`, the RTC memory, sets the cause | **Tasks 6, 11** (`STORE0..7` survive a CPU and a core reset; a system reset clears them, `ASSUMPTION:`) |
| Watchdogs: RWDT, MWDT0, MWDT1, super watchdog | **Tasks 10, 13** |
| PMP enforcement and debug triggers | **SP1b-4** (E31) |
| Cache error interrupts (sources 36 and 61) | **Task 4** (E33) |
| eFuse programming registers (`EFUSE.CMD`, `PGM_*`) | **SP1b-3b**: still on no measured path (FW); they stay named unmodelled accesses |
| SP1b-1 leftovers: peripheral time | **Tasks 2, 3** |
| SP1b-1 leftovers: UART transmit time | **Task 9**; reception: **SP1b-3b** |
| SP1b-1 leftovers: interrupt delivery | **Tasks 5, 6, 8** |
| SP1b-1 leftovers: exception log with PC and stack | **Task 14** |
| Rows of the accepted table whose heir was SP1b-3 | **Tasks 4, 5, 9 to 13**: modelled, or given their real heir; Task 17 checks that no row and no comment names SP1b-3 any more |
| À défaire 1: side channels by concrete type | **Tasks 1, 4** |
| À défaire 2: time in the machine, events in cycles | **Tasks 2, 3** |
| À défaire 3: a word fetched through a faulty mapping stays decoded | **Task 4** (E36) |
| À défaire 4: an `OPTIONS0` write with a reset bit is dropped whole | **Task 10** |
| À défaire 5: `NorFlash` returns no content (persistence) | **SP1b-3b** |
| À défaire 6: D4 live view against stale cache lines | **SP1b-3b** (with flash writes) |
| À étendre: `FLASH_SE`/`PP`/`READ` wired with their address; third term of `page_version` | **SP1b-3b** |
| À étendre: an `ICACHE_CTRL` write invalidates all decoded flash code (cost) | **SP8** (timing and performance); unchanged here |
| À étendre: `Peripherals::new` consumes the flash, so a reset cannot rebuild without taking the chip back | **Task 11**: a reset rebuilds the SPI1 registers in place, the chip never leaves `Spi1` |
| À étendre: `DeviceProfile` id as a validated type; partition tables in profiles | **SP1b-3b** |
| À étendre: reset and `VirtualClock` (time must not go back) | **Tasks 2, 11**: `VirtualClock` is gone, device time lives in the context and a reset does not touch it |
| À étendre: size rule of an external `FlashDevice`; the OTA test counts writes | **SP1b-3b** |
| À étendre: literal register offsets in `uart.rs`, `intmatrix.rs`, `assist_debug.rs` | **Tasks 9, 5, 11** |
| À étendre: Ctrl-C prints the summary | **Task 15** |
| Form: `IO_MUX.DATE` row text, `# Errors` of `from_named_toml`, `ProfileError::Parse` repeating its remedy, `13333333 Hz` | Left to the plans that touch those files, as the note asks: the two `boards` points to **SP1b-3b** (it turns the profile id into a type), the `IO_MUX` row to **SP2** (pads). The frequency text is exact as it is: no change |

---

## File Structure

```
Cargo.toml                                 + ctrlc 3.5 (Task 15)
docs/adr/0003-contexte-des-peripheriques.md  the mechanism, in French (Task 1)
docs/registers-accepted-without-effect.md  regenerated by the tasks that change a list
docs/superpowers/notes/2026-10-11-sp1b-3a-carryover.md  hand-over to SP1b-3b and SP1b-4 (Task 17)
crates/emu-core/src/
  peripheral.rs                            Peripheral<C>: every access receives the context (Task 1)
  time.rs                                  - VirtualClock (Task 2)
  bus.rs                                   + Bus::fetch_is_memory (Task 4)
crates/cpu-rv32/src/
  hart.rs                                  InterruptModel, set_external_interrupt, reset, is_waiting (Tasks 4, 6)
  csr.rs                                   the ESP32-C3 mtvec (Task 6)
crates/soc-esp32c3/src/
  ctx.rs                                   Ctx, Source, Event, Group, Module, Deadline (Tasks 1 to 3, then one variant per device)
  reset.rs                                 ResetLevel, ResetCause (Task 3)
  clock.rs                                 TIME_BASE, XTAL_PERIOD, CpuClock, cpu_clock (Task 2)
  bus.rs                                   owns the Ctx; deliver, settle, reset (Tasks 1, 4, 11)
  machine.rs                               the run loop: device time, service, interrupts, sleep, reset (Tasks 2, 6, 8, 9, 11, 13, 14)
  diag.rs                                  Unsupported::{UartOverflow, SlowClock}, Exception.sp (Tasks 9, 10, 14); Unsupported::Reset from Task 10 to Task 11
  test_support.rs                          store_register, stop_watchdogs (Tasks 8, 10)
  periph/mod.rs                            half_of, replace_half; Peripherals::{new, deliver, reset} (Tasks 4 to 13)
  periph/extmem.rs                         fault latches called with the context; sources 36 and 61 (Task 4)
  periph/intmatrix.rs                      the interrupt controller (Task 5)
  periph/counter.rs                        Rate, Counter (Task 7)
  periph/systimer.rs                       SYSTIMER (Task 7)
  periph/system.rs                         software interrupts (Task 8), module resets (Tasks 11, 13)
  periph/uart.rs                           transmit time (Task 9)
  periph/rtc_cntl.rs                       reset cause, software reset, RTC timer, RWDT, super watchdog (Task 10)
  periph/efuse.rs                          wdt_delay_sel (Task 10), the word at +0x18C (Task 12)
  periph/assist_debug.rs                   the recorded PC, the stack guard registers (Task 11)
  periph/sensitive.rs                      the permission registers as storage (Task 11)
  periph/spi_mem.rs, apb_ctrl.rs, usb_serial_jtag.rs   reset in place (Task 11); new accepted rows (Task 12)
  periph/gpio.rs                           new accepted rows (Task 12)
  periph/timg.rs                           timer groups: T0, MWDT, calibration (Task 13)
crates/soc-esp32c3/tests/
  reference_boot.rs                        the life-cycle tests (Task 13)
  real_rom.rs                              the watchdog tests (Task 16)
crates/emu-cli/src/
  run.rs                                   Asleep (Task 8), console flush (Task 9), Ctrl-C (Task 15)
  exit.rs, main.rs                         exit codes and the Ctrl-C handler (Tasks 8, 15)
crates/emu-cli/tests/run_cli.rs            the reference firmware through its restart (Task 13)
.github/workflows/ci.yml                   ignored-test counts (Tasks 13, 16)
README.md                                  status, milestone, limits (Task 17)
```

The order of the tasks is the order of the dependencies: the mechanism (1 to 4), the interrupt path (5, 6), the first timed device and the loop that serves it (7, 8), then one device per task, each complete when it arrives (9, 10), the resets that the watchdogs ask for (11), the last storage rows (12), and the timer groups, which complete the life of the application (13). After every task the workspace builds, `cargo test --workspace` passes and `clippy` is clean; the number of passing tests is given at the end of each task.

---

## Task 1: ADR 0003; every peripheral access receives a context, and refusals go through it

A refactor: no behaviour changes, every existing test passes with its assertions untouched.

**Files:**
- Create: `docs/adr/0003-contexte-des-peripheriques.md`, `crates/soc-esp32c3/src/ctx.rs`
- Modify: `crates/emu-core/src/peripheral.rs`, `crates/soc-esp32c3/src/lib.rs`, `crates/soc-esp32c3/src/bus.rs`, `crates/soc-esp32c3/src/machine.rs`, every file of `crates/soc-esp32c3/src/periph/`

**Interfaces:**
- Consumes: `emu_core::Peripheral { fn read(&mut self, offset: u32) -> Option<u32>; fn write(&mut self, offset: u32, value: u32) -> bool }`; `soc_esp32c3::diag::{Diagnostics, Diagnostic, Unsupported}`; `SocBus { periph, diag: Diagnostics, pc: u32, .. }`, `SocBus::relay_unsupported`; `Peripherals::take_unsupported`, and the `take_unsupported` of `Spi1`, `Sha` and `RtcCntl` (three `VecDeque<Unsupported>` queues).
- Produces: `emu_core::Peripheral<C: ?Sized> { fn read(&mut self, ctx: &mut C, offset: u32) -> Option<u32>; fn write(&mut self, ctx: &mut C, offset: u32, value: u32) -> bool }`; `soc_esp32c3::ctx::Ctx` with `Ctx::new(strict: bool)`, `Default` (not strict), `set_pc(&mut self, pc: u32)`, `pc(&self) -> u32`, `unsupported(&mut self, what: Unsupported)`, the crate-visible field `diag: Diagnostics`, and for tests `take_unsupported(&mut self) -> Option<Unsupported>`; `SocBus.ctx: Ctx` (the fields `diag` and `pc` are gone: `bus.ctx.diag`, `bus.ctx.pc()`); `Peripherals::device(&mut self, addr: u32) -> Option<(&mut dyn Peripheral<Ctx>, u32)>`. The three queues, `SocBus::relay_unsupported` and `Peripherals::take_unsupported` no longer exist.

- [ ] **Step 1: Write the ADR**

Create `docs/adr/0003-contexte-des-peripheriques.md`:

```markdown
# ADR 0003 — Contexte des périphériques, temps de l'appareil et resets

- Date : 2026-10-11
- Statut : accepté (plan SP1b-3a)

## Contexte

Jusqu'au plan SP1b-2, un modèle de périphérique ne voyait que ses registres : `read(offset)` et `write(offset, valeur)`. Tout ce qui dépassait ce cadre passait par un canal propre à un type concret. Trois modèles gardaient une file de refus que le bus vidait après chaque accès (`take_unsupported`) ; `System` levait un drapeau que la machine relisait (`take_clock_change`) ; le bus appelait deux méthodes d'`Extmem` par son nom pour verrouiller une faute (`latch_mmu_fault`, `latch_store`). Le temps appartenait à la machine (`VirtualClock`), hors de portée des périphériques, et l'ordonnanceur d'`emu-core`, daté en cycles du CPU, ne servait à rien : l'horloge du CPU passe de 20 à 80 puis 160 MHz pendant un démarrage, alors que le SYSTIMER compte à 16 MHz quoi qu'il arrive.

Les interruptions, les timers, les watchdogs et les resets demandent tous la même chose : qu'un périphérique lise l'heure, lève une ligne d'interruption, demande à être rappelé à un instant, et dise à la machine ce qu'elle doit faire (changer d'horloge, redémarrer). Ajouter un canal par besoin aurait multiplié par quatre les canaux existants.

## Décision

1. **Un seul mécanisme : le contexte.** Chaque accès à un registre reçoit un contexte (`soc_esp32c3::ctx::Ctx`), que le bus possède. Le trait d'`emu-core` devient `Peripheral<C>` : `emu-core` ne connaît toujours aucun SoC. Par le contexte, et par rien d'autre, un périphérique lit le temps de l'appareil, pilote ses sources d'interruption, demande un appel à un instant donné, et dépose ce que la machine doit traiter : une opération qu'il ne sait pas faire fidèlement, un changement d'horloge, une demande de reset, un événement destiné à un autre périphérique. Les canaux par type concret disparaissent.
2. **Le temps de l'appareil se compte en cycles d'une base de temps fixe à 160 MHz** (6,25 ns). Toutes les horloges modélisées en sont des multiples entiers : le CPU à 160 MHz (1), à 80 MHz (2), le quartz à 40 MHz (4) et ses divisions, le SYSTIMER (10). Les oscillateurs RC, qui ne tombent pas juste, sont des rapports exacts (136 kHz : 17 coups pour 20 000 cycles). Le CPU ajoute « cycles × période » après chaque instruction ; un changement d'horloge change un multiplicateur et aucune échéance. L'ordonnanceur d'`emu-core` reste daté en cycles, ceux de cette base. Le temps ne recule jamais, resets compris.
3. **Un périphérique est appelé, jamais interrogé.** Il demande un appel (`Event`) à l'instant de sa prochaine échéance ; `Peripherals::deliver` l'appelle quand cet instant est atteint. Le `tick` de la spec (§3.2) est cet appel. Un appel est une invitation, pas un ordre : le périphérique recalcule ce qui est dû à l'heure où on l'appelle. Un appel devenu sans objet (watchdog nourri, alarme déplacée, périphérique réinitialisé) ne trouve rien à faire, si bien qu'aucun reset ni aucune reprogrammation n'a d'annulation à tenir à jour.
4. **Un compteur est une valeur à un instant et une cadence.** Rien n'est compté cycle par cycle : la valeur à une date et la date d'une valeur se calculent (`periph::counter`). SYSTIMER, timers généraux, timer RTC et watchdogs partagent cette arithmétique.
5. **Les interruptions suivent le contrôleur de la puce.** Les périphériques pilotent 62 sources ; le contrôleur (`INTERRUPT_CORE0`) en déduit la ligne à prendre : activée, levée, de priorité non nulle et au moins égale au seuil, la plus prioritaire d'abord, la plus petite ligne à égalité. Il présente cette ligne au cœur, qui la prend quand `mstatus.MIE` est à 1. Sur l'ESP32-C3, `mie` et `mip` ne filtrent rien. Le cœur générique d'`inkemu rvtest` garde le comportement standard.
6. **`wfi` fait avancer le temps.** Le CPU qui attend une interruption saute à la prochaine échéance ; son compteur de cycles ne bouge pas. S'il n'y a ni échéance ni ligne en attente, l'exécution s'arrête avec une raison nommée.
7. **Un reset a trois niveaux à l'exécution** — CPU, cœur, système —, le quatrième, la mise sous tension, étant la construction de la machine. Chaque niveau remet à l'état de reset exactement ce que les rapports de recherche tabulent, laisse les mémoires intactes et inscrit sa cause dans `RTC_CNTL.RESET_STATE`. Un reset prend effet à la fin de l'instruction qui l'a demandé.

## Conséquences

- **Moins de code qu'avant pour plus de comportements** : trois files, un drapeau, un relais du bus et l'horloge virtuelle disparaissent ; les six types du contexte (`Ctx`, `Source`, `Event`, `Group`, `Module`, `Deadline`) les remplacent, et servent aussi aux interruptions, aux timers et aux resets.
- **Les tests d'un périphérique se passent de machine** : un contexte par défaut suffit, on y avance le temps et on y lit ce que le périphérique a déposé.
- **Coût par instruction** : une comparaison d'entiers (le temps contre la prochaine échéance) et un test de drapeaux. Dormir dix secondes coûte un millier d'appels, un par tick de 10 ms de l'application.
- **Limite connue** : `--max-cycles` compte les cycles du CPU, qui n'avancent pas pendant `wfi`. Un firmware qui dort pour toujours pendant qu'un périphérique compte n'est borné que par Ctrl-C ; `run_until` rend alors la main à intervalles réguliers (`StopReason::Asleep`). Les scénarios scriptés du sous-projet 4 auront besoin d'une limite en temps de l'appareil, que la spec ne nomme pas encore.
- **Hypothèses** : ce que les documents ne tranchent pas (réveil de `wfi` avec les interruptions masquées, détails des fronts, registres conservés par chaque niveau de reset, valeur de `Saved PC` après un reset logiciel) est marqué `ASSUMPTION:` dans le code et transmis au plan SP1b-4, dont le lockstep ne peut de toute façon pas traverser un reset.
- **Révision de l'ADR** : à rouvrir si une horloge qui n'est pas un multiple de la base de temps devient nécessaire, ou si le coût des appels sans objet devient mesurable.
```

- [ ] **Step 2: Change the trait and create the context**

The test that drives this task is the compiler and the existing suite: nothing may change but the signatures.

```diff
--- a/crates/emu-core/src/peripheral.rs
+++ b/crates/emu-core/src/peripheral.rs
@@ -3,11 +3,14 @@
 /// The `SoC` bus splits sub-word accesses and resolves the device, so a peripheral only ever sees
 /// whole registers at 4-byte-aligned offsets from its own base.
 ///
-/// The `tick` that spec §3.2 lists arrives with the first time-dependent peripherals (SP1b-3).
-pub trait Peripheral: std::fmt::Debug {
+/// `C` is the context the `SoC` hands to every access: through it, and through nothing else, a
+/// peripheral reads device time, drives its interrupt lines, asks to be called back at an instant
+/// and reports what it cannot do (ADR 0003). The `tick` of spec §3.2 is that call-back: a
+/// peripheral is called at the instant it asked for, never polled.
+pub trait Peripheral<C: ?Sized>: std::fmt::Debug {
     /// `None` means this model does not implement the register: the bus then answers with the
     /// documented reset value and reports the access.
-    fn read(&mut self, offset: u32) -> Option<u32>;
+    fn read(&mut self, ctx: &mut C, offset: u32) -> Option<u32>;
     /// `false` means this model does not implement the register: the write is dropped and reported.
-    fn write(&mut self, offset: u32, value: u32) -> bool;
+    fn write(&mut self, ctx: &mut C, offset: u32, value: u32) -> bool;
 }
```

Create `crates/soc-esp32c3/src/ctx.rs`:

```rust
//! What a peripheral reaches beyond its own registers: the one door between a device model and the
//! rest of the machine (ADR 0003).
use crate::diag::{Diagnostic, Diagnostics, Unsupported};

/// The context of a peripheral access. The bus owns it and hands it to every register read and
/// write; the machine reads what the peripherals left in it after each instruction.
#[derive(Debug)]
pub struct Ctx {
    /// What the run has to tell the user. Reached from outside the crate through the machine.
    pub(crate) diag: Diagnostics,
    /// PC of the instruction being executed, for diagnostics.
    pc: u32,
    /// Every refusal in the order it was reported, for the tests of the device models: the
    /// diagnostics list a repeated refusal once, and a device test wants to see each one.
    #[cfg(test)]
    refused: std::collections::VecDeque<Unsupported>,
}

impl Ctx {
    #[must_use]
    pub fn new(strict: bool) -> Ctx {
        Ctx {
            diag: Diagnostics::new(strict),
            pc: 0,
            #[cfg(test)]
            refused: std::collections::VecDeque::new(),
        }
    }

    /// Tells the context which instruction is running, so diagnostics can name it.
    pub fn set_pc(&mut self, pc: u32) {
        self.pc = pc;
    }

    /// PC of the instruction being executed.
    #[must_use]
    pub const fn pc(&self) -> u32 {
        self.pc
    }

    /// Reports an operation the device cannot perform faithfully: it becomes a diagnostic that
    /// names the instruction, and `--strict-mmio` stops on it.
    pub fn unsupported(&mut self, what: Unsupported) {
        #[cfg(test)]
        self.refused.push_back(what);
        self.diag
            .record(Diagnostic::Unsupported { what, pc: self.pc });
    }

    /// The oldest refusal a device reported and no test took yet.
    #[cfg(test)]
    pub(crate) fn take_unsupported(&mut self) -> Option<Unsupported> {
        self.refused.pop_front()
    }
}

impl Default for Ctx {
    /// A context that stops on nothing: what a test of one device needs.
    fn default() -> Ctx {
        Ctx::new(false)
    }
}
```

```diff
--- a/crates/soc-esp32c3/src/lib.rs
+++ b/crates/soc-esp32c3/src/lib.rs
@@ -14,6 +14,7 @@ pub mod bus;
 pub mod chip;
 pub mod clock;
 pub mod config;
+pub mod ctx;
 pub mod diag;
 pub mod flash;
 pub mod machine;
```

- [ ] **Step 3: Give the bus the context and remove its relay**

The bus owns the context, hands it to every access, and no longer drains anything after it: a device reports through `ctx.unsupported`, which records the diagnostic with the PC at once.

```diff
--- a/crates/soc-esp32c3/src/bus.rs
+++ b/crates/soc-esp32c3/src/bus.rs
@@ -1,6 +1,7 @@
 //! The ESP32-C3 address space. Memory map: `docs/research/esp32c3-boot-chain.md`, §A.1.
 use crate::config::SocConfig;
-use crate::diag::{Diagnostic, Diagnostics, Direction, MappingFault};
+use crate::ctx::Ctx;
+use crate::diag::{Diagnostic, Direction, MappingFault};
 use crate::flash::FlashDevice;
 use crate::memmap::{
     DROM_BASE, DROM_ROM_OFFSET, DROM_SIZE, FLASH_DBUS_BASE, FLASH_IBUS_BASE, FLASH_WINDOW_SIZE,
@@ -23,9 +24,8 @@ pub struct SocBus {
     /// Reached from outside the crate only through the bus: a direct write would bypass the relay
     /// of what a device refuses, and so the diagnostics.
     pub(crate) periph: Peripherals,
-    pub(crate) diag: Diagnostics,
-    /// PC of the instruction being executed, for diagnostics.
-    pc: u32,
+    /// The context handed to every peripheral access; it holds the diagnostics of the run.
+    pub(crate) ctx: Ctx,
 }
 
 #[derive(Clone, Copy)]
@@ -107,14 +107,13 @@ impl SocBus {
             sram0: Ram::new(SRAM0_BASE, SRAM0_SIZE as usize),
             rtc: Ram::new(RTC_RAM_BASE, RTC_RAM_SIZE as usize),
             periph: Peripherals::new(config, flash),
-            diag: Diagnostics::new(config.strict_mmio),
-            pc: 0,
+            ctx: Ctx::new(config.strict_mmio),
         }
     }
 
     /// Tells the bus which instruction is running, so diagnostics can name it.
     pub fn set_pc(&mut self, pc: u32) {
-        self.pc = pc;
+        self.ctx.set_pc(pc);
     }
 
     fn rom_read(&self, offset: u32, size: AccessSize) -> u32 {
@@ -130,10 +129,10 @@ impl SocBus {
     fn unmodeled(&mut self, reg_addr: u32, direction: Direction, written: u32) -> u32 {
         let info = svd_regs::esp32c3::lookup(reg_addr);
         let reset = info.map_or(0, |r| r.reset_value);
-        self.diag.record(Diagnostic::UnmodeledMmio {
+        self.ctx.diag.record(Diagnostic::UnmodeledMmio {
             addr: reg_addr,
             direction,
-            pc: self.pc,
+            pc: self.ctx.pc(),
             register: info.map(|r| (r.peripheral, r.name)),
             value: if direction == Direction::Read {
                 reset
@@ -191,12 +190,12 @@ impl SocBus {
                 Err(unbacked) => {
                     // The fault registers and the diagnostic name the address of the access.
                     self.periph.extmem.latch_mmu_fault(vaddr, unbacked.content);
-                    self.diag.record(Diagnostic::FlashMapping {
+                    self.ctx.diag.record(Diagnostic::FlashMapping {
                         vaddr,
                         entry: unbacked.entry,
                         content: unbacked.content,
                         fault: unbacked.fault,
-                        pc: self.pc,
+                        pc: self.ctx.pc(),
                     });
                     reported = Some(unbacked);
                 }
@@ -208,14 +207,6 @@ impl SocBus {
         }
     }
 
-    /// Turns everything a device could not do into diagnostics that name the instruction.
-    fn relay_unsupported(&mut self) {
-        while let Some(what) = self.periph.take_unsupported() {
-            self.diag
-                .record(Diagnostic::Unsupported { what, pc: self.pc });
-        }
-    }
-
     fn mmio_read(&mut self, addr: u32, size: AccessSize) -> Result<ReadOutcome, BusError> {
         if !addr.is_multiple_of(size.bytes()) {
             return Err(BusError::Misaligned { addr, size });
@@ -224,8 +215,7 @@ impl SocBus {
         let modelled = self
             .periph
             .device(reg_addr)
-            .and_then(|(device, base)| device.read(reg_addr - base));
-        self.relay_unsupported();
+            .and_then(|(device, base)| device.read(&mut self.ctx, reg_addr - base));
         let word = match modelled {
             Some(word) => word,
             None => self.unmodeled(reg_addr, Direction::Read, 0),
@@ -244,17 +234,16 @@ impl SocBus {
         let reg_addr = addr & !(REGISTER_BYTES - 1);
         let word = (value & size_mask(size)) << (8 * (addr - reg_addr));
         if size != AccessSize::Word {
-            self.diag.record(Diagnostic::SubWordWrite {
+            self.ctx.diag.record(Diagnostic::SubWordWrite {
                 addr,
                 bytes: size.bytes(),
-                pc: self.pc,
+                pc: self.ctx.pc(),
             });
         }
         let handled = self
             .periph
             .device(reg_addr)
-            .is_some_and(|(device, base)| device.write(reg_addr - base, word));
-        self.relay_unsupported();
+            .is_some_and(|(device, base)| device.write(&mut self.ctx, reg_addr - base, word));
         if !handled {
             self.unmodeled(reg_addr, Direction::Write, word);
         }
```

```diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -104,7 +104,7 @@ impl Esp32C3 {
                 StepOutcome::Waiting => return StopReason::Idle,
                 // An `ecall` is a request to the trap handler, not a fault worth reporting.
                 StepOutcome::Trapped { trap, .. } if trap.cause != Exception::EcallM => {
-                    self.bus.diag.record(Diagnostic::Exception {
+                    self.bus.ctx.diag.record(Diagnostic::Exception {
                         cause: trap.cause,
                         tval: trap.tval,
                         pc,
@@ -118,7 +118,7 @@ impl Esp32C3 {
             if let Some(trace) = &mut self.trace {
                 trace.mix_u32(pc);
             }
-            if let Some(violation) = self.bus.diag.take_violation() {
+            if let Some(violation) = self.bus.ctx.diag.take_violation() {
                 return StopReason::StrictMmio(violation);
             }
         }
@@ -132,7 +132,7 @@ impl Esp32C3 {
         match clock::cpu_frequency(sysclk_conf, cpu_per_conf) {
             Some(frequency) => self.clock.set_frequency(self.hart.cycles, frequency),
             // Decision D22: the previous frequency stays, and the selection is reported.
-            None => self.bus.diag.record(Diagnostic::Unsupported {
+            None => self.bus.ctx.diag.record(Diagnostic::Unsupported {
                 what: Unsupported::ClockSource {
                     sysclk_conf,
                     cpu_per_conf,
@@ -182,21 +182,21 @@ impl Esp32C3 {
 
     /// Diagnostics seen for the first time since the last call.
     pub fn drain_diagnostics(&mut self) -> Vec<Diagnostic> {
-        self.bus.diag.drain()
+        self.bus.ctx.diag.drain()
     }
 
     /// At most `limit` diagnostics that occurred more than once, the most frequent first, with their
     /// number of occurrences: what the firmware keeps doing, such as polling one register.
     #[must_use]
     pub fn most_repeated_diagnostics(&self, limit: usize) -> Vec<(Diagnostic, u64)> {
-        self.bus.diag.most_repeated(limit)
+        self.bus.ctx.diag.most_repeated(limit)
     }
 
     /// Occurrences dropped because more than [`MAX_DISTINCT_DIAGNOSTICS`](crate::diag::MAX_DISTINCT_DIAGNOSTICS) distinct diagnostics were
     /// known: they are counted but never listed.
     #[must_use]
     pub const fn suppressed_diagnostics(&self) -> u64 {
-        self.bus.diag.suppressed()
+        self.bus.ctx.diag.suppressed()
     }
 
     /// The model that prices each instruction, so a front end can say whether timing is calibrated.
```

- [ ] **Step 4: Migrate the device models**

Every `impl Peripheral for X` becomes `impl Peripheral<Ctx> for X` and takes `ctx` (`_ctx` where unused). The three models that queued refusals call `ctx.unsupported(..)` where they pushed, and lose their queue and their `take_unsupported`. The production changes, file by file:

```diff
--- a/crates/soc-esp32c3/src/periph/mod.rs
+++ b/crates/soc-esp32c3/src/periph/mod.rs
@@ -1,6 +1,6 @@
 //! The modelled peripherals of the ESP32-C3 and their place in the address space.
 use crate::config::SocConfig;
-use crate::diag::Unsupported;
+use crate::ctx::Ctx;
 use crate::flash::FlashDevice;
 use emu_core::Peripheral;
 
@@ -184,12 +184,12 @@ impl AcceptedBlock {
     }
 }
 
-impl Peripheral for AcceptedBlock {
-    fn read(&mut self, offset: u32) -> Option<u32> {
+impl Peripheral<Ctx> for AcceptedBlock {
+    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
         self.registers.read(offset)
     }
 
-    fn write(&mut self, offset: u32, value: u32) -> bool {
+    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
         self.registers.write(offset, value)
     }
 }
@@ -396,7 +396,7 @@ impl Peripherals {
     }
 
     /// The device that owns the register at `addr`, with its base address.
-    pub(crate) fn device(&mut self, addr: u32) -> Option<(&mut dyn Peripheral, u32)> {
+    pub(crate) fn device(&mut self, addr: u32) -> Option<(&mut dyn Peripheral<Ctx>, u32)> {
         let within = |block: (u32, u32)| (block.0..block.1).contains(&addr);
         if within(GPIO) {
             Some((&mut self.gpio, GPIO_BASE))
@@ -444,20 +444,12 @@ impl Peripherals {
             None
         }
     }
-
-    /// The oldest thing a device was asked to do and cannot do faithfully, not taken yet: call until
-    /// `None` to get all of them, in order.
-    pub(crate) fn take_unsupported(&mut self) -> Option<Unsupported> {
-        self.spi1
-            .take_unsupported()
-            .or_else(|| self.sha.take_unsupported())
-            .or_else(|| self.rtc_cntl.take_unsupported())
-    }
 }
 
 #[cfg(test)]
 mod tests {
     use super::*;
+    use crate::diag::Unsupported;
 
     #[test]
     fn storage_starts_at_the_svd_reset_values_and_only_answers_its_own_registers() {
```
```diff
--- a/crates/soc-esp32c3/src/periph/spi_mem.rs
+++ b/crates/soc-esp32c3/src/periph/spi_mem.rs
@@ -2,10 +2,10 @@
 //! FB §B and §D). SPI1 runs the commands software asks for; SPI0 refills the cache, which this
 //! emulator does not model (the bus reads the flash directly), so SPI0 is configuration storage.
 use super::{Accepted, SPI0_BASE, SPI1_BASE, Storage};
+use crate::ctx::Ctx;
 use crate::diag::Unsupported;
 use crate::flash::{FlashCommand, FlashDevice};
 use emu_core::Peripheral;
-use std::collections::VecDeque;
 use svd_regs::esp32c3::addr;
 
 const CMD: u32 = addr("SPI1", "CMD") - SPI1_BASE;
@@ -142,15 +142,15 @@ impl Default for Spi0 {
     }
 }
 
-impl Peripheral for Spi0 {
-    fn read(&mut self, offset: u32) -> Option<u32> {
+impl Peripheral<Ctx> for Spi0 {
+    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
         if offset == SPI0_FSM {
             return Some(self.fsm_idle);
         }
         self.config.read(offset)
     }
 
-    fn write(&mut self, offset: u32, value: u32) -> bool {
+    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
         self.config.write(offset, value)
     }
 }
@@ -159,10 +159,6 @@ impl Peripheral for Spi0 {
 pub struct Spi1 {
     registers: Storage,
     flash: Box<dyn FlashDevice>,
-    /// The commands the chip refused, oldest first. One `CMD` write runs at most one command per
-    /// command bit (the hard-wired ones and `USR`), so a write adds at most `HARD_WIRED.len() + 1`
-    /// entries; the bus drains them after every access.
-    unsupported: VecDeque<Unsupported>,
 }
 
 impl Spi1 {
@@ -174,7 +170,6 @@ impl Spi1 {
         Spi1 {
             registers: Storage::for_offsets(SPI1_BASE, &offsets),
             flash,
-            unsupported: VecDeque::new(),
         }
     }
 
@@ -184,24 +179,23 @@ impl Spi1 {
         self.flash.as_ref()
     }
 
-    /// The oldest command the flash model refused and that was not taken yet: call until `None`
-    /// to get every refusal, in the order the commands ran.
-    pub(crate) fn take_unsupported(&mut self) -> Option<Unsupported> {
-        self.unsupported.pop_front()
-    }
-
     fn register(&self, offset: u32) -> u32 {
         self.registers.read(offset).unwrap_or(0)
     }
 
     /// Runs `command`; `Some(bytes)` are the `data_in_len` bytes read, `None` when the chip model
-    /// does not implement the opcode (the refusal is kept for the bus to report).
-    fn transfer(&mut self, command: &FlashCommand<'_>, data_in_len: usize) -> Option<Vec<u8>> {
+    /// does not implement the opcode (the refusal is reported through the context).
+    fn transfer(
+        &mut self,
+        ctx: &mut Ctx,
+        command: &FlashCommand<'_>,
+        data_in_len: usize,
+    ) -> Option<Vec<u8>> {
         let mut data_in = vec![0u8; data_in_len.min(DATA_BYTES)];
         match self.flash.execute(command, &mut data_in) {
             Ok(()) => Some(data_in),
             Err(refused) => {
-                self.unsupported.push_back(Unsupported::FlashCommand {
+                ctx.unsupported(Unsupported::FlashCommand {
                     opcode: refused.opcode,
                 });
                 None
@@ -214,7 +208,7 @@ impl Spi1 {
         (self.register(USER2) & OPCODE_MASK).to_le_bytes()[0]
     }
 
-    fn run_hard_wired(&mut self, command_bit: u32, opcode: u8) {
+    fn run_hard_wired(&mut self, ctx: &mut Ctx, command_bit: u32, opcode: u8) {
         let command = FlashCommand {
             opcode,
             address: None,
@@ -223,13 +217,13 @@ impl Spi1 {
         };
         match command_bit {
             CMD_RDID => {
-                if let Some(id) = self.transfer(&command, JEDEC_ID_BYTES) {
+                if let Some(id) = self.transfer(ctx, &command, JEDEC_ID_BYTES) {
                     // ASSUMPTION: W0[31:24] reads 0 after a read-id; the ROM masks it (FB §L item 7).
                     self.store_data_in(&id);
                 }
             }
             CMD_RDSR => {
-                if let Some(status) = self.transfer(&command, 1) {
+                if let Some(status) = self.transfer(ctx, &command, 1) {
                     // ASSUMPTION: only RD_STATUS[7:0] is replaced (FB §D.1, §L item 7).
                     let low = status.first().copied().map_or(0, u32::from);
                     let kept = self.register(RD_STATUS) & !STATUS_BYTE_MASK;
@@ -237,7 +231,7 @@ impl Spi1 {
                 }
             }
             _ => {
-                let _ = self.transfer(&command, 0);
+                let _ = self.transfer(ctx, &command, 0);
             }
         }
     }
@@ -261,7 +255,7 @@ impl Spi1 {
             .then_some(((user2 >> COMMAND_BITLEN_SHIFT) & COMMAND_BITLEN_MASK) + 1)
     }
 
-    fn run_user(&mut self) {
+    fn run_user(&mut self, ctx: &mut Ctx) {
         let user = self.register(USER);
         let user1 = self.register(USER1);
         // A transfer without a command phase, or whose command is not 8 bits long, has no opcode the
@@ -269,7 +263,7 @@ impl Spi1 {
         // low byte of `USER2` were its opcode.
         let command_bits = Spi1::command_bits(user, self.register(USER2));
         if command_bits != Some(MODELED_COMMAND_BITS) {
-            self.unsupported.push_back(Unsupported::FlashTransfer {
+            ctx.unsupported(Unsupported::FlashTransfer {
                 command_bits,
                 user,
                 user2: self.register(USER2),
@@ -309,7 +303,7 @@ impl Spi1 {
             dummy_cycles,
             data_out: &data_out,
         };
-        if let Some(data_in) = self.transfer(&command, data_in_len) {
+        if let Some(data_in) = self.transfer(ctx, &command, data_in_len) {
             self.store_data_in(&data_in);
         }
     }
@@ -339,20 +333,20 @@ impl Spi1 {
     ///
     /// ASSUMPTION: `FLASH_PE` written without any command bit starts no transfer and self-clears. No
     /// ROM or ESP-IDF v5.5.2 code writes it alone; the SP1b-4 lockstep, or a device, settles it.
-    fn run(&mut self, command_bits: u32) {
+    fn run(&mut self, ctx: &mut Ctx, command_bits: u32) {
         for (bit, opcode) in HARD_WIRED {
             if command_bits & bit != 0 {
-                self.run_hard_wired(bit, opcode);
+                self.run_hard_wired(ctx, bit, opcode);
             }
         }
         if command_bits & CMD_USR != 0 {
-            self.run_user();
+            self.run_user(ctx);
         }
     }
 }
 
-impl Peripheral for Spi1 {
-    fn read(&mut self, offset: u32) -> Option<u32> {
+impl Peripheral<Ctx> for Spi1 {
+    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
         if offset == CMD {
             // Every command bit self-clears and the state nibbles read idle (FB §C rows 9 and 11).
             return Some(0);
@@ -360,9 +354,9 @@ impl Peripheral for Spi1 {
         self.registers.read(offset)
     }
 
-    fn write(&mut self, offset: u32, value: u32) -> bool {
+    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
         if offset == CMD {
-            self.run(value);
+            self.run(ctx, value);
             return true;
         }
         self.registers.write(offset, value)
```
```diff
--- a/crates/soc-esp32c3/src/periph/sha.rs
+++ b/crates/soc-esp32c3/src/periph/sha.rs
@@ -3,9 +3,9 @@
 //! The compression functions come from the `sha1` and `sha2` crates (decisions D7 and D20); only the
 //! register interface is ours.
 use super::SHA_BASE;
+use crate::ctx::Ctx;
 use crate::diag::Unsupported;
 use emu_core::Peripheral;
-use std::collections::VecDeque;
 use svd_regs::esp32c3::addr;
 
 const MODE: u32 = addr("SHA", "MODE") - SHA_BASE;
@@ -83,8 +83,6 @@ pub struct Sha {
     state: [u32; STATE_WORDS],
     /// `M_MEM0..15` as written.
     block: [u32; BLOCK_WORDS],
-    /// Refusals not taken yet, oldest first: a refusal is never overwritten by a later one.
-    unsupported: VecDeque<Unsupported>,
 }
 
 /// Index of `offset` in a block of `count` words starting at `first`.
@@ -99,10 +97,6 @@ impl Sha {
         Sha::default()
     }
 
-    pub(crate) fn take_unsupported(&mut self) -> Option<Unsupported> {
-        self.unsupported.pop_front()
-    }
-
     /// Compresses the block in `M_MEM`; `fresh` starts from the algorithm's initial value.
     ///
     /// ASSUMPTION: `CONTINUE` under a `MODE` other than the one the hash was started with compresses
@@ -110,10 +104,9 @@ impl Sha {
     /// words for SHA-1, the three others being left as they are). The ROM writes the mode before
     /// every block, and the same one for a whole hash (FB §B.7), so no measured path decides it;
     /// the SP1b-4 lockstep, or a device, settles it.
-    fn compress(&mut self, fresh: bool) {
+    fn compress(&mut self, ctx: &mut Ctx, fresh: bool) {
         let Some(algorithm) = Algorithm::from_mode(self.mode) else {
-            self.unsupported
-                .push_back(Unsupported::ShaMode { mode: self.mode });
+            ctx.unsupported(Unsupported::ShaMode { mode: self.mode });
             return;
         };
         // The message bytes are the bytes of the M_MEM words in memory order (FB §F).
@@ -148,8 +141,8 @@ impl Sha {
     }
 }
 
-impl Peripheral for Sha {
-    fn read(&mut self, offset: u32) -> Option<u32> {
+impl Peripheral<Ctx> for Sha {
+    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
         if let Some(index) = word_index(offset, H_MEM0, STATE_WORDS) {
             return self.state.get(index).copied();
         }
@@ -164,7 +157,7 @@ impl Peripheral for Sha {
         }
     }
 
-    fn write(&mut self, offset: u32, value: u32) -> bool {
+    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
         if let Some(slot) =
             word_index(offset, H_MEM0, STATE_WORDS).and_then(|i| self.state.get_mut(i))
         {
@@ -181,13 +174,13 @@ impl Peripheral for Sha {
             MODE => self.mode = value,
             START | CONTINUE => {
                 if value & 1 != 0 {
-                    self.compress(offset == START);
+                    self.compress(ctx, offset == START);
                 }
             }
             // Decision D7: DMA-SHA is not modelled; a write that configures it (the block count) or
             // starts it is reported, never silently dropped.
             DMA_BLOCK_NUM | DMA_START | DMA_CONTINUE => {
-                self.unsupported.push_back(Unsupported::ShaDma);
+                ctx.unsupported(Unsupported::ShaDma);
             }
             _ => return false,
         }
```
```diff
--- a/crates/soc-esp32c3/src/periph/rtc_cntl.rs
+++ b/crates/soc-esp32c3/src/periph/rtc_cntl.rs
@@ -1,10 +1,10 @@
 //! `RTC_CNTL`: reset cause, retention registers, pad, clock, analog and power configuration.
 //! Source: report §E.2. The watchdogs are stored and never expire (decision D11); the RTC timer and
 //! resets arrive in SP1b-3.
+use crate::ctx::Ctx;
 use crate::diag::Unsupported;
 use crate::periph::{Accepted, RTC_CNTL_BASE, Storage};
 use emu_core::Peripheral;
-use std::collections::VecDeque;
 use svd_regs::esp32c3::addr;
 
 const OPTIONS0: u32 = addr("RTC_CNTL", "OPTIONS0") - RTC_CNTL_BASE;
@@ -153,8 +153,6 @@ pub struct RtcCntl {
     reset_state_other: u32,
     store: [u32; STORE_COUNT],
     config: Storage,
-    /// Refused requests not taken yet, oldest first: none may be lost between two relays.
-    unsupported: VecDeque<Unsupported>,
 }
 
 impl RtcCntl {
@@ -167,14 +165,8 @@ impl RtcCntl {
             reset_state_other: documented & !RESET_CAUSE_MASK,
             store: [0; STORE_COUNT],
             config: Storage::accepted(RTC_CNTL_BASE, &ACCEPTED),
-            unsupported: VecDeque::new(),
         }
     }
-
-    /// The oldest request this block refused and nobody took yet.
-    pub(crate) fn take_unsupported(&mut self) -> Option<Unsupported> {
-        self.unsupported.pop_front()
-    }
 }
 
 impl Default for RtcCntl {
@@ -183,8 +175,8 @@ impl Default for RtcCntl {
     }
 }
 
-impl Peripheral for RtcCntl {
-    fn read(&mut self, offset: u32) -> Option<u32> {
+impl Peripheral<Ctx> for RtcCntl {
+    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
         if offset == RESET_STATE {
             return Some(self.reset_state_other | self.reset_cause);
         }
@@ -194,12 +186,11 @@ impl Peripheral for RtcCntl {
         self.config.read(offset)
     }
 
-    fn write(&mut self, offset: u32, value: u32) -> bool {
+    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
         // Decision D27: a software reset is not emulated yet (SP1b-3). Storing the bit would be a
         // reset that silently never happens: the request is reported and dropped instead.
         if offset == OPTIONS0 && value & (SW_SYS_RST | SW_PROCPU_RST) != 0 {
-            self.unsupported
-                .push_back(Unsupported::SoftwareReset { options0: value });
+            ctx.unsupported(Unsupported::SoftwareReset { options0: value });
             return true;
         }
         if let Some(i) = STORE.iter().position(|&o| o == offset) {
```
```diff
--- a/crates/soc-esp32c3/src/periph/extmem.rs
+++ b/crates/soc-esp32c3/src/periph/extmem.rs
@@ -5,6 +5,7 @@
 //! (decision D4). The operations software waits for (invalidate, preload, autoload, freeze)
 //! therefore complete at once, and the registers below only keep what software reads back.
 use super::{Accepted, EXTMEM_BASE, Storage};
+use crate::ctx::Ctx;
 use emu_core::Peripheral;
 use svd_regs::esp32c3::addr;
 
@@ -147,8 +148,8 @@ impl Default for Extmem {
     }
 }
 
-impl Peripheral for Extmem {
-    fn read(&mut self, offset: u32) -> Option<u32> {
+impl Peripheral<Ctx> for Extmem {
+    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
         Some(match offset {
             ICACHE_SYNC_CTRL => SYNC_DONE,
             ICACHE_PRELOAD_CTRL => PRELOAD_DONE | self.preload_order,
@@ -172,7 +173,7 @@ impl Peripheral for Extmem {
         })
     }
 
-    fn write(&mut self, offset: u32, value: u32) -> bool {
+    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
         match offset {
             // The invalidation has nothing to invalidate; `SYNC_DONE` already reads set.
             ICACHE_SYNC_CTRL => {}
```
```diff
--- a/crates/soc-esp32c3/src/periph/system.rs
+++ b/crates/soc-esp32c3/src/periph/system.rs
@@ -1,5 +1,6 @@
 //! `SYSTEM`: clock selection, peripheral clock gates and resets. Source: report §E.1.
 //! The CPU clock selection is read by `crate::clock`; gates and resets are stored.
+use crate::ctx::Ctx;
 use crate::periph::{Accepted, SYSTEM_BASE, Storage};
 use emu_core::Peripheral;
 
@@ -120,15 +121,15 @@ impl Default for System {
     }
 }
 
-impl Peripheral for System {
-    fn read(&mut self, offset: u32) -> Option<u32> {
+impl Peripheral<Ctx> for System {
+    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
         if offset == SYSCLK_CONF {
             return Some(self.clock_registers().0);
         }
         self.registers.read(offset)
     }
 
-    fn write(&mut self, offset: u32, value: u32) -> bool {
+    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
         match offset {
             SYSCLK_CONF => {
                 self.sysclk_writable = value & SYSCLK_WRITABLE;
```
```diff
--- a/crates/soc-esp32c3/src/periph/ana_i2c.rs
+++ b/crates/soc-esp32c3/src/periph/ana_i2c.rs
@@ -5,6 +5,7 @@
 //! The SVD does not describe this window (decision D17). No analog behaviour is modelled: a byte
 //! written to an analog register reads back, nothing more (decision D24).
 use super::Accepted;
+use crate::ctx::Ctx;
 use emu_core::Peripheral;
 use std::collections::BTreeMap;
 
@@ -96,8 +97,8 @@ impl AnaI2c {
     }
 }
 
-impl Peripheral for AnaI2c {
-    fn read(&mut self, offset: u32) -> Option<u32> {
+impl Peripheral<Ctx> for AnaI2c {
+    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
         match offset {
             HOST0_CMD => self.command.first().copied(),
             HOST1_CMD => self.command.get(1).copied(),
@@ -105,7 +106,7 @@ impl Peripheral for AnaI2c {
         }
     }
 
-    fn write(&mut self, offset: u32, value: u32) -> bool {
+    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
         let host = match offset {
             HOST0_CMD => 0,
             HOST1_CMD => 1,
```
```diff
--- a/crates/soc-esp32c3/src/periph/apb_ctrl.rs
+++ b/crates/soc-esp32c3/src/periph/apb_ctrl.rs
@@ -2,6 +2,7 @@
 //! Sources: report BA §2 row 88, §2.3 item 1, §3 rows 29 and 34.
 use super::{APB_CTRL_BASE, Accepted, Storage};
 use crate::config::DEFAULT_RNG_SEED;
+use crate::ctx::Ctx;
 use emu_core::Peripheral;
 use svd_regs::esp32c3::addr;
 
@@ -58,15 +59,15 @@ impl ApbCtrl {
     }
 }
 
-impl Peripheral for ApbCtrl {
-    fn read(&mut self, offset: u32) -> Option<u32> {
+impl Peripheral<Ctx> for ApbCtrl {
+    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
         if offset == RND_DATA {
             return Some(self.next_random());
         }
         self.power.read(offset)
     }
 
-    fn write(&mut self, offset: u32, value: u32) -> bool {
+    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
         // `RND_DATA` is read-only: it is deliberately not handled here, so a write to it is refused
         // and the bus reports it as unmodelled instead of pretending the generator was seeded.
         self.power.write(offset, value)
```
```diff
--- a/crates/soc-esp32c3/src/periph/assist_debug.rs
+++ b/crates/soc-esp32c3/src/periph/assist_debug.rs
@@ -1,6 +1,7 @@
 //! `ASSIST_DEBUG`: the PC recorder the ROM consults to print `Saved PC` after a watchdog reset.
 //! Source: report §J, line 10. Recording a PC at reset arrives with the watchdogs (SP1b-3).
 use super::Accepted;
+use crate::ctx::Ctx;
 use emu_core::Peripheral;
 
 pub(crate) fn accepted() -> Vec<Accepted> {
@@ -30,8 +31,8 @@ impl AssistDebug {
     }
 }
 
-impl Peripheral for AssistDebug {
-    fn read(&mut self, offset: u32) -> Option<u32> {
+impl Peripheral<Ctx> for AssistDebug {
+    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
         match offset {
             CORE_0_RCD_EN => Some(self.record_enable),
             // Nothing was recorded (this is a power-on reset) and no debugger is attached (BA §2.2).
@@ -40,7 +41,7 @@ impl Peripheral for AssistDebug {
         }
     }
 
-    fn write(&mut self, offset: u32, value: u32) -> bool {
+    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
         let is_record_enable = offset == CORE_0_RCD_EN;
         if is_record_enable {
             self.record_enable = value;
```
```diff
--- a/crates/soc-esp32c3/src/periph/efuse.rs
+++ b/crates/soc-esp32c3/src/periph/efuse.rs
@@ -5,6 +5,7 @@
 //! The content is that of a production chip (decision D9); programming registers stay unmodelled
 //! until a firmware needs them (SP1b-3).
 use crate::chip::ChipRevision;
+use crate::ctx::Ctx;
 use crate::periph::EFUSE_BASE;
 use emu_core::Peripheral;
 
@@ -102,15 +103,15 @@ impl Efuse {
     }
 }
 
-impl Peripheral for Efuse {
-    fn read(&mut self, offset: u32) -> Option<u32> {
+impl Peripheral<Ctx> for Efuse {
+    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
         self.read_mirror
             .binary_search_by_key(&offset, |&(o, _)| o)
             .ok()
             .map(|i| self.read_mirror[i].1)
     }
 
-    fn write(&mut self, _offset: u32, _value: u32) -> bool {
+    fn write(&mut self, _ctx: &mut Ctx, _offset: u32, _value: u32) -> bool {
         false
     }
 }
```
```diff
--- a/crates/soc-esp32c3/src/periph/gpio.rs
+++ b/crates/soc-esp32c3/src/periph/gpio.rs
@@ -1,6 +1,7 @@
 //! GPIO block. The strapping register and the registers the ROM's flash-boot path reads back are
 //! modelled; pins arrive with the board wiring (SP2).
 use super::{Accepted, GPIO_BASE, Storage};
+use crate::ctx::Ctx;
 use emu_core::Peripheral;
 
 /// `GPIO_STRAP_REG`: the strapping pins sampled at reset. Bit 3 set selects SPI flash boot (report §D.3).
@@ -44,8 +45,8 @@ impl Gpio {
     }
 }
 
-impl Peripheral for Gpio {
-    fn read(&mut self, offset: u32) -> Option<u32> {
+impl Peripheral<Ctx> for Gpio {
+    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
         if offset == STRAP {
             Some(self.strapping)
         } else {
@@ -53,7 +54,7 @@ impl Peripheral for Gpio {
         }
     }
 
-    fn write(&mut self, offset: u32, value: u32) -> bool {
+    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
         // `STRAP` is not in the list: it stays read-only.
         self.config.write(offset, value)
     }
```
```diff
--- a/crates/soc-esp32c3/src/periph/intmatrix.rs
+++ b/crates/soc-esp32c3/src/periph/intmatrix.rs
@@ -3,6 +3,7 @@
 //! This plan keeps the configuration the software writes; no source is modelled yet, so nothing is
 //! ever pending. Delivery to the CPU arrives in SP1b-3.
 use super::Accepted;
+use crate::ctx::Ctx;
 use emu_core::Peripheral;
 
 /// `<SRC>_MAP` registers: `UART_INTR_MAP` ... `CACHE_CORE0_ACS_INT_MAP` @0xF4 (report §B.3).
@@ -85,8 +86,8 @@ impl Default for IntMatrix {
     }
 }
 
-impl Peripheral for IntMatrix {
-    fn read(&mut self, offset: u32) -> Option<u32> {
+impl Peripheral<Ctx> for IntMatrix {
+    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
         if let Some(source) = index_in(offset, 0, SOURCE_COUNT) {
             return Some(self.map[source]);
         }
@@ -104,7 +105,7 @@ impl Peripheral for IntMatrix {
         })
     }
 
-    fn write(&mut self, offset: u32, value: u32) -> bool {
+    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
         if let Some(source) = index_in(offset, 0, SOURCE_COUNT) {
             self.map[source] = value & MAP_MASK;
             return true;
```
```diff
--- a/crates/soc-esp32c3/src/periph/io_mux.rs
+++ b/crates/soc-esp32c3/src/periph/io_mux.rs
@@ -1,5 +1,6 @@
 //! IO MUX block: pad configuration registers.
 use super::{Accepted, IO_MUX_BASE, Storage};
+use crate::ctx::Ctx;
 use emu_core::Peripheral;
 
 /// Every IO MUX register of the SVD is stored (report BC §A.2), one row per run of consecutive
@@ -54,12 +55,12 @@ impl Default for IoMux {
     }
 }
 
-impl Peripheral for IoMux {
-    fn read(&mut self, offset: u32) -> Option<u32> {
+impl Peripheral<Ctx> for IoMux {
+    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
         self.pads.read(offset)
     }
 
-    fn write(&mut self, offset: u32, value: u32) -> bool {
+    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
         self.pads.write(offset, value)
     }
 }
```
```diff
--- a/crates/soc-esp32c3/src/periph/mmu.rs
+++ b/crates/soc-esp32c3/src/periph/mmu.rs
@@ -1,5 +1,6 @@
 //! The flash MMU table: 128 entries that map 64 KiB pages of the two flash windows to flash pages.
 //! Source: report CM §1 (ESP-IDF `ext_mem_defs.h`, `mmu_ll.h`; ROM `Cache_MMU_Init`).
+use crate::ctx::Ctx;
 use emu_core::Peripheral;
 
 /// `DR_REG_MMU_TABLE` (ESP-IDF `reg_base.h`). The SVD does not describe this block (decision D17).
@@ -84,12 +85,12 @@ impl Default for MmuTable {
     }
 }
 
-impl Peripheral for MmuTable {
-    fn read(&mut self, offset: u32) -> Option<u32> {
+impl Peripheral<Ctx> for MmuTable {
+    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
         Self::index(offset).and_then(|index| self.entry(index))
     }
 
-    fn write(&mut self, offset: u32, value: u32) -> bool {
+    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
         let Some(slot) = Self::index(offset).and_then(|index| self.entries.get_mut(index)) else {
             return false;
         };
```
```diff
--- a/crates/soc-esp32c3/src/periph/uart.rs
+++ b/crates/soc-esp32c3/src/periph/uart.rs
@@ -4,6 +4,7 @@
 //! always reads empty and the baud rate has no effect. Real timing, reception and interrupts arrive
 //! in SP1b-3; until then the emulated boot is shorter than the real one by the time the UART would
 //! take to send its log.
+use crate::ctx::Ctx;
 use crate::periph::{Accepted, Storage};
 use emu_core::Peripheral;
 
@@ -67,8 +68,8 @@ impl Uart {
     }
 }
 
-impl Peripheral for Uart {
-    fn read(&mut self, offset: u32) -> Option<u32> {
+impl Peripheral<Ctx> for Uart {
+    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
         match offset {
             STATUS => Some(self.idle_status),
             // The transmitter is idle: bytes leave at once (decision D7 of SP1b-1).
@@ -78,7 +79,7 @@ impl Peripheral for Uart {
         }
     }
 
-    fn write(&mut self, offset: u32, value: u32) -> bool {
+    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
         match offset {
             FIFO => {
                 self.tx.push(value.to_le_bytes()[0]);
```
```diff
--- a/crates/soc-esp32c3/src/periph/usb_serial_jtag.rs
+++ b/crates/soc-esp32c3/src/periph/usb_serial_jtag.rs
@@ -2,6 +2,7 @@
 //!
 //! The model behaves like a board with a host attached and reading: the IN endpoint always has
 //! room. Reception, interrupts and host attach/detach arrive in SP1b-3.
+use crate::ctx::Ctx;
 use emu_core::Peripheral;
 
 const EP1: u32 = 0x00;
@@ -42,12 +43,12 @@ impl UsbSerialJtag {
     }
 }
 
-impl Peripheral for UsbSerialJtag {
-    fn read(&mut self, offset: u32) -> Option<u32> {
+impl Peripheral<Ctx> for UsbSerialJtag {
+    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
         (offset == EP1_CONF).then_some(SERIAL_IN_EP_DATA_FREE)
     }
 
-    fn write(&mut self, offset: u32, value: u32) -> bool {
+    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
         match offset {
             EP1 => {
                 self.packet.push(value.to_le_bytes()[0]);
```

- [ ] **Step 5: Migrate the tests, without touching an assertion**

428 sites in the test modules of `periph/*.rs` and `bus.rs` change (the tests of `machine.rs` need no change), all by the same six rules; no expected value changes.

**No script rewrites a source file** (Global Constraints), here as elsewhere. Each rule is applied with the editing tool, **one exact pattern, replace-all, one file at a time**: the patterns of rules 1, 4 and 5 are in the table below, file by file, and none of them occurs outside the test module of its file, so a replace-all touches nothing else. After each file, `cargo test --workspace --no-run > /tmp/t1c.log 2>&1; echo "exit=$?"` confirms: the errors that remain in that file are the next rule's. Order: rules 1, 4 and 5, then rule 3, then rule 2, then rule 6. The table is also what the reviewer checks the diff against: the number of changed sites of each file must be the one it gives.

1. A call of `read` or `write` on a device gains `ctx` as its first argument: `x.read(OFFSET)` becomes `x.read(ctx, OFFSET)`, `x.write(OFFSET, v)` becomes `x.write(ctx, OFFSET, v)` (`E0061: this method takes 2 arguments but 1 argument was supplied`, 162 sites; `takes 3 arguments but 2`, 195 sites). The pattern is the receiver with the opening parenthesis: `sha.read(` is replaced by `sha.read(ctx, ` and `sha.write(` by `sha.write(ctx, `, for each receiver the table lists for the file. One call is written on two lines and escapes its pattern: in `efuse.rs`, the helper `word` reads `efuse` on one line and `.read(svd_regs::esp32c3::addr("EFUSE", name) - EFUSE_BASE)` on the next; the compiler names it, and `.read(` becomes `.read(ctx, ` there by hand. Calls whose receiver is not in the table keep their arguments: `Storage::read`/`write` (`storage.read(..)`) and the `read` of the bus (`b.read(addr, size, kind)`).
2. A test function that makes such a call, or that passes `ctx` to a helper of rule 3, starts with `let ctx = &mut Ctx::default();` — **after** any `const` item at the top of its body (`clippy::items_after_statements`; five tests have one: one in `mmu.rs`, one in `mod.rs`, one in `sha.rs`, two in `spi_mem.rs`). 98 test functions get the line. Every test module concerned has `use super::*;`, which brings `Ctx` in: no import is added by this step (the one import `periph/mod.rs` gains is in the diff of Step 4).
3. A helper function of a test module that calls a device takes `ctx: &mut Ctx` as its first parameter, and its callers pass `ctx` first. There are seven: `word(ctx, &mut efuse, name)` and `revision(ctx, &mut efuse)` in `efuse.rs` (12 and 3 callers), `write_block(ctx, &mut sha, block)`, `hash(ctx, mode, message, len)` and `state(ctx, &mut sha)` in `sha.rs` (7, 6 and 8 callers), `user_read(ctx, &mut spi, opcode, miso_bits)` in `spi_mem.rs` (5 callers), `send(ctx, &mut usb, text)` in `usb_serial_jtag.rs` (3 callers). The helpers that call no device (such as `peripherals()`, `spi1()`, `recording_spi1()`, `reg()`, `padded()`, `write_cmd()`) do not change.
4. `<receiver>.take_unsupported()` becomes `ctx.take_unsupported()`: the context keeps, in test builds only, every refusal in the order it was reported (`E0599: no method named 'take_unsupported'`, 42 sites). The patterns: `p.take_unsupported()` in `mod.rs`; `rtc.take_unsupported()` in `rtc_cntl.rs`; `sha.take_unsupported()` and `resumed.take_unsupported()` in `sha.rs`; `spi.take_unsupported()` and `refused.take_unsupported()` in `spi_mem.rs`.
5. In `bus.rs`, `b.diag` becomes `b.ctx.diag` (`E0609: no field 'diag' on type 'bus::SocBus'`, 29 sites: the tests name their bus `b`). The pattern is `b.diag`, not `.diag`: the production code of the file already reads `self.ctx.diag` since Step 3. No test reads the `pc` of the bus.
6. A closure that calls a device uses the `ctx` of its test function: it captures it. Nine closures do (three in `apb_ctrl.rs`, one in `efuse.rs`, four in `sha.rs`, one in `mod.rs`) and need nothing but rule 1. One more needs its binding changed, because it now borrows `ctx` mutably (`E0596: cannot borrow 'page_program' as mutable`): in `spi_mem.rs`, test `an_esp_idf_program_sets_the_flag_with_usr_and_its_opcode_runs_once`, `let page_program = |spi: &mut Spi1| {` becomes `let mut page_program = |spi: &mut Spi1| {`.

Sites per file (rule 1 as `read` + `write` calls; rule 2 as test functions that get the `let ctx` line):

| File | Rule 1: receivers (`read` + `write` sites) | Rule 1 | Rule 4 | Rule 5 | Rule 2 | Rule 3 |
|---|---|---|---|---|---|---|
| `periph/ana_i2c.rs` | `i2c` (9 + 9) | 9 + 9 | — | — | 4 | — |
| `periph/apb_ctrl.rs` | `apb` (6 + 2), `first` (3 + 0), `second` (1 + 0), `third` (1 + 0) | 11 + 2 | — | — | 4 | — |
| `periph/assist_debug.rs` | `dbg` (5 + 3) | 5 + 3 | — | — | 3 | — |
| `periph/efuse.rs` | `efuse` (10 + 1; nine of the ten `read` on one line) | 10 + 1 | — | — | 5 | `word`, `revision` |
| `periph/extmem.rs` | `x` (28 + 18) | 28 + 18 | — | — | 10 | — |
| `periph/gpio.rs` | `gpio` (4 + 2) | 4 + 2 | — | — | 1 | — |
| `periph/intmatrix.rs` | `m` (13 + 9) | 13 + 9 | — | — | 4 | — |
| `periph/mmu.rs` | `mmu` (4 + 8) | 4 + 8 | — | — | 5 | — |
| `periph/mod.rs` | `block` (3 + 2), `device` (2 + 5) | 5 + 7 | 7 | — | 6 | — |
| `periph/rtc_cntl.rs` | `rtc` (14 + 14) | 14 + 14 | 7 | — | 8 | — |
| `periph/sha.rs` | `sha` (10 + 23), `first` (1 + 4), `resumed` (1 + 6), `restarted` (0 + 2) | 12 + 35 | 9 | — | 12 | `write_block`, `hash`, `state` |
| `periph/spi_mem.rs` | `spi` (22 + 65), `spi0` (5 + 3), `read` (1 + 1), `refused` (0 + 1) | 28 + 70 | 19 | — | 18 | `user_read` |
| `periph/system.rs` | `sys` (7 + 6) | 7 + 6 | — | — | 5 | — |
| `periph/uart.rs` | `uart` (8 + 7), `uart1` (1 + 0) | 9 + 7 | — | — | 9 | — |
| `periph/usb_serial_jtag.rs` | `usb` (3 + 4) | 3 + 4 | — | — | 4 | `send` |
| `bus.rs` | — | — | — | 29 | — | — |
| Total | | 162 + 195 | 42 | 29 | 98 | 7 helpers, 44 callers |

One example of rules 1, 2 and 4, from `periph/sha.rs`:

```rust
// before
#[test]
fn dma_start_is_reported() {
    let mut sha = Sha::new();
    for name in ["DMA_START", "DMA_CONTINUE", "DMA_BLOCK_NUM"] {
        let offset = svd_regs::esp32c3::addr("SHA", name) - SHA_BASE;
        assert!(sha.write(offset, 1), "{name}");
        assert_eq!(sha.take_unsupported(), Some(Unsupported::ShaDma), "{name}");
    }
}

// after
#[test]
fn dma_start_is_reported() {
    let ctx = &mut Ctx::default();
    let mut sha = Sha::new();
    for name in ["DMA_START", "DMA_CONTINUE", "DMA_BLOCK_NUM"] {
        let offset = svd_regs::esp32c3::addr("SHA", name) - SHA_BASE;
        assert!(sha.write(ctx, offset, 1), "{name}");
        assert_eq!(ctx.take_unsupported(), Some(Unsupported::ShaDma), "{name}");
    }
}
```

- [ ] **Step 6: Run the suite**

Run: `cargo test --workspace > /tmp/t1.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t1.log`
Expected: `exit=0`, and `591`: the count before the task. No test was added, removed or changed in what it asserts.

Run: `grep -rn "take_unsupported\|relay_unsupported" crates/soc-esp32c3/src > /tmp/g1.log; grep -v "ctx" /tmp/g1.log | wc -l`
Expected: `0`: every remaining mention goes through the context.

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add docs/adr/0003-contexte-des-peripheriques.md crates/emu-core/src/peripheral.rs crates/soc-esp32c3/src
/usr/bin/git commit -m "refactor(soc): a peripheral access receives a context; refusals go through it

ADR 0003. The three refusal queues and the relay of the bus are gone.

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- docs/adr/0003-contexte-des-peripheriques.md crates/emu-core/src/peripheral.rs crates/soc-esp32c3/src
```

---

## Task 2: Device time in cycles of one time base; `VirtualClock` removed

A refactor: device time, which the machine kept in a `VirtualClock`, moves into the context, counted in cycles of a 160 MHz base (decision E17). `Esp32C3::device_time_nanos` returns the same values as before.

**Files:**
- Modify: `crates/soc-esp32c3/src/clock.rs`, `crates/soc-esp32c3/src/ctx.rs`, `crates/soc-esp32c3/src/machine.rs`, `crates/soc-esp32c3/src/periph/system.rs`, `crates/emu-core/src/time.rs`, `crates/emu-core/src/lib.rs`

**Interfaces:**
- Consumes: `emu_core::{Cycles, Frequency}` (`Frequency::new(NonZeroU64)`, `Frequency::cycles_to_nanos(Cycles) -> u64`, `Frequency::hz() -> u64`); `emu_core::VirtualClock`; `clock::cpu_frequency(sysclk_conf: u32, cpu_per_conf: u32) -> Option<Frequency>`; `System::clock_registers(&self) -> (u32, u32)` and `System::take_clock_change(&mut self) -> bool`; `Esp32C3 { clock: VirtualClock, .. }`.
- Produces: `clock::TIME_BASE: Frequency` (160 MHz), `clock::XTAL_PERIOD: u64` (4), `clock::CpuClock { pub cpu_period: u64, pub apb_period: u64 }` with `CpuClock::POWER_ON` and `cpu_frequency(self) -> Frequency`, `clock::cpu_clock(sysclk_conf: u32, cpu_per_conf: u32) -> Option<CpuClock>` (`cpu_frequency` stays, built on it); on `Ctx`: `now(&self) -> Cycles`, `advance(&mut self, cpu_cycles: u64)`, `clock(&self) -> CpuClock`, `set_clock(&mut self, clock: CpuClock)`, `clock_written(&mut self)`, `take_clock_written(&mut self) -> bool`. `emu_core::VirtualClock` and `System::take_clock_change` no longer exist.

- [ ] **Step 1: Write the failing test**

Every selection the firmware can make must be a whole number of time-base cycles, with the APB clock of TRM table 6.2-4 (TW §B.2):

```diff
--- a/crates/soc-esp32c3/src/clock.rs
+++ b/crates/soc-esp32c3/src/clock.rs
@@ -92,6 +148,47 @@ mod tests {
         );
     }
 
+    #[test]
+    fn every_selection_is_a_whole_number_of_time_base_cycles() {
+        // The reason the time base is 160 MHz: no selectable clock needs a fraction of its cycle.
+        assert_eq!(TIME_BASE.hz(), 160_000_000);
+        assert_eq!(
+            cpu_clock(0x2_8001, CPU_PER_RESET),
+            Some(CpuClock::POWER_ON),
+            "power-on: 20 MHz, 8 time-base cycles per CPU and APB cycle"
+        );
+        assert_eq!(CpuClock::POWER_ON.cpu_period, 8);
+        let clock = |sysclk, cpu_per| cpu_clock(sysclk, cpu_per).unwrap();
+        assert_eq!(
+            (
+                clock(0x2_8000, 0xC).cpu_period,
+                clock(0x2_8000, 0xC).apb_period
+            ),
+            (4, 4)
+        );
+        assert_eq!(
+            (
+                clock(0x2_8400, 0xC).cpu_period,
+                clock(0x2_8400, 0xC).apb_period
+            ),
+            (2, 2)
+        );
+        // At 160 MHz the APB clock stays at 80 MHz (TRM table 6.2-4).
+        assert_eq!(
+            (
+                clock(0x2_8400, 0xD).cpu_period,
+                clock(0x2_8400, 0xD).apb_period
+            ),
+            (1, 2)
+        );
+        assert_eq!(clock(0x2_83FF, 0xC).cpu_period, 4 * 1024);
+        assert_eq!(
+            clock(0x2_8002, 0xC).cpu_frequency().hz(),
+            13_333_333,
+            "40 MHz / 3, rounded down"
+        );
+    }
+
     #[test]
     fn the_crystal_divider_covers_its_whole_field() {
         assert_eq!(hz(0x2_83FF, CPU_PER_RESET), Some(40_000_000 / 1024));
```

- [ ] **Step 2: Run it and see it fail**

Run: `cargo test -p soc-esp32c3 --lib clock > /tmp/t2.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, a compile error: `cannot find function 'cpu_clock'`, `cannot find struct 'CpuClock'`.

- [ ] **Step 3: The time base and the clocks as periods**

```diff
--- a/crates/soc-esp32c3/src/clock.rs
+++ b/crates/soc-esp32c3/src/clock.rs
@@ -16,8 +16,6 @@ const fn megahertz(mhz: u32) -> Frequency {
 /// below, and the value `SYSCLK_CONF.CLK_XTAL_FREQ` reads (`periph/system.rs`), both come from it.
 pub const XTAL_MHZ: u32 = 40;
 pub const XTAL: Frequency = megahertz(XTAL_MHZ);
-const PLL_CPU_80: Frequency = megahertz(80);
-const PLL_CPU_160: Frequency = megahertz(160);
 
 /// `SYSCLK_CONF` [9:0] `PRE_DIV_CNT`, [11:10] `SOC_CLK_SEL`.
 const PRE_DIV_MASK: u32 = 0x3FF;
@@ -28,31 +26,89 @@ const SOC_CLK_PLL: u32 = 1;
 /// `CPU_PER_CONF` [1:0] `CPUPERIOD_SEL`: 0 is 80 MHz, 1 is 160 MHz, whichever PLL frequency is selected.
 const CPUPERIOD_SEL_MASK: u32 = 0x3;
 
-/// The CPU clock frequency the two clock registers select, or `None` for a selection this model
-/// has no frequency for (the RC oscillator, or a value ESP-IDF itself aborts on: BA §2.2).
+/// The time base of the machine: device time is counted in cycles of this clock (ADR 0003).
 ///
-/// ASSUMPTION: (decision D21) with the crystal as source the CPU runs at 40 MHz / (`PRE_DIV_CNT` + 1),
-/// the formula of ESP-IDF's `clk_ll_cpu_get_divider`, hence **20 MHz at reset** (`PRE_DIV_CNT` = 1).
-/// The mask ROM assumes 40 MHz (`ets_get_xtal_div` returns 1 unless `CLK_DIV_EN` is set): which one
-/// silicon does is not established (BA §10 item 1). The firmware-visible cycle counter is the same
-/// either way; only this conversion to time depends on it. A timestamped log of a real unit, or the
-/// SP8 calibration bench, settles it.
+/// 160 MHz is the fastest clock software can select, and every clock this model counts with is a
+/// whole number of its cycles: the CPU at 80 MHz (2), the crystal (4) and its divisions (4 × n), the
+/// SYSTIMER counters at 16 MHz (10). One cycle lasts 6.25 ns. An event scheduled at a device time
+/// therefore stays at its instant whatever the CPU clock becomes (decision E2).
+pub const TIME_BASE: Frequency = megahertz(160);
+/// Time-base cycles in one cycle of the 40 MHz crystal.
+pub const XTAL_PERIOD: u64 = 4;
+const PLL_80_PERIOD: u64 = 2;
+const PLL_160_PERIOD: u64 = 1;
+
+/// The two clocks the selection registers decide, each as its period in time-base cycles.
+#[derive(Clone, Copy, Debug, PartialEq, Eq)]
+pub struct CpuClock {
+    /// Time-base cycles in one CPU clock cycle.
+    pub cpu_period: u64,
+    /// Time-base cycles in one `APB_CLK` cycle. `APB_CLK` is 80 MHz when the CPU runs from the PLL
+    /// and equals `CPU_CLK` otherwise (TRM table 6.2-4; report TW §B.2).
+    pub apb_period: u64,
+}
+
+impl CpuClock {
+    /// The clocks after a power-on reset: the crystal divided by 2, 20 MHz (decisions D21 and E8).
+    pub const POWER_ON: CpuClock = CpuClock {
+        cpu_period: 2 * XTAL_PERIOD,
+        apb_period: 2 * XTAL_PERIOD,
+    };
+
+    /// The CPU clock frequency, rounded down to a whole number of hertz.
+    #[must_use]
+    pub fn cpu_frequency(self) -> Frequency {
+        // INVARIANT: `cpu_period` is at most 4 × 1024 (the crystal divider field is 10 bits wide),
+        // so the quotient is at least 39 062 Hz; the fallback is never taken.
+        Frequency::from_hz(TIME_BASE.hz() / self.cpu_period).unwrap_or(TIME_BASE)
+    }
+}
+
+/// The clocks the two clock registers select, or `None` for a selection this model has no
+/// frequency for (the RC oscillator, or a value ESP-IDF itself aborts on: BA §2.2).
+///
+/// With the crystal as source the CPU runs at 40 MHz / (`PRE_DIV_CNT` + 1), the formula of ESP-IDF's
+/// `clk_ll_cpu_get_divider`, hence **20 MHz after a power-on reset** (`PRE_DIV_CNT` = 1): TRM
+/// §6.2.4.1 says the crystal is divided by 2 by default, and a reset-loop log of a real chip excludes
+/// 40 MHz for the clock of the flash-boot watchdog (report TW §1 result 3, §H.2). The mask ROM itself
+/// assumes 40 MHz (`ets_get_xtal_div` returns 1 unless `CLK_DIV_EN` is set, BA §10 item 1): it is
+/// wrong about its own speed after power-on, and right after `esp_restart`, which leaves
+/// `PRE_DIV_CNT` = 0 across the CPU reset (report IR §4.1).
+///
+/// ASSUMPTION: the lower bound measured on silicon (0.955 s between two watchdog resets, TW §H.2) is
+/// compatible with 20 MHz and excludes 40 MHz, but it is one log; the SP8 calibration bench, or a
+/// timestamped boot log of a real unit, settles the exact figure.
 #[must_use]
-pub fn cpu_frequency(sysclk_conf: u32, cpu_per_conf: u32) -> Option<Frequency> {
+pub fn cpu_clock(sysclk_conf: u32, cpu_per_conf: u32) -> Option<CpuClock> {
     match (sysclk_conf >> SOC_CLK_SEL_SHIFT) & SOC_CLK_SEL_MASK {
         SOC_CLK_XTAL => {
-            let divider = u64::from(sysclk_conf & PRE_DIV_MASK) + 1;
-            Frequency::from_hz(XTAL.hz() / divider)
+            let period = XTAL_PERIOD * (u64::from(sysclk_conf & PRE_DIV_MASK) + 1);
+            Some(CpuClock {
+                cpu_period: period,
+                apb_period: period,
+            })
+        }
+        SOC_CLK_PLL => {
+            let cpu_period = match cpu_per_conf & CPUPERIOD_SEL_MASK {
+                0 => PLL_80_PERIOD,
+                1 => PLL_160_PERIOD,
+                _ => return None,
+            };
+            Some(CpuClock {
+                cpu_period,
+                apb_period: PLL_80_PERIOD,
+            })
         }
-        SOC_CLK_PLL => match cpu_per_conf & CPUPERIOD_SEL_MASK {
-            0 => Some(PLL_CPU_80),
-            1 => Some(PLL_CPU_160),
-            _ => None,
-        },
         _ => None,
     }
 }
 
+/// The CPU clock frequency the two clock registers select: [`cpu_clock`] as a frequency.
+#[must_use]
+pub fn cpu_frequency(sysclk_conf: u32, cpu_per_conf: u32) -> Option<Frequency> {
+    cpu_clock(sysclk_conf, cpu_per_conf).map(CpuClock::cpu_frequency)
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
```

- [ ] **Step 4: The context keeps device time and the clock selection**

```diff
--- a/crates/soc-esp32c3/src/ctx.rs
+++ b/crates/soc-esp32c3/src/ctx.rs
@@ -1,6 +1,8 @@
 //! What a peripheral reaches beyond its own registers: the one door between a device model and the
 //! rest of the machine (ADR 0003).
+use crate::clock::CpuClock;
 use crate::diag::{Diagnostic, Diagnostics, Unsupported};
+use emu_core::Cycles;
 
 /// The context of a peripheral access. The bus owns it and hands it to every register read and
 /// write; the machine reads what the peripherals left in it after each instruction.
@@ -10,6 +12,13 @@ pub struct Ctx {
     pub(crate) diag: Diagnostics,
     /// PC of the instruction being executed, for diagnostics.
     pc: u32,
+    /// Device time: cycles of the time base ([`crate::clock::TIME_BASE`]) since power-on. It only
+    /// grows, across resets too.
+    now: Cycles,
+    /// The CPU and APB clocks the firmware selected.
+    clock: CpuClock,
+    /// A clock selection register was written since the machine last looked.
+    clock_written: bool,
     /// Every refusal in the order it was reported, for the tests of the device models: the
     /// diagnostics list a repeated refusal once, and a device test wants to see each one.
     #[cfg(test)]
@@ -22,6 +31,9 @@ impl Ctx {
         Ctx {
             diag: Diagnostics::new(strict),
             pc: 0,
+            now: Cycles::ZERO,
+            clock: CpuClock::POWER_ON,
+            clock_written: false,
             #[cfg(test)]
             refused: std::collections::VecDeque::new(),
         }
@@ -38,6 +50,38 @@ impl Ctx {
         self.pc
     }
 
+    /// Device time: cycles of the time base since power-on.
+    #[must_use]
+    pub const fn now(&self) -> Cycles {
+        self.now
+    }
+
+    /// The CPU has run `cpu_cycles` cycles of its clock: device time moves on by as much.
+    pub fn advance(&mut self, cpu_cycles: u64) {
+        self.now = self.now + cpu_cycles.saturating_mul(self.clock.cpu_period);
+    }
+
+    /// The CPU and APB clocks in use.
+    #[must_use]
+    pub const fn clock(&self) -> CpuClock {
+        self.clock
+    }
+
+    /// From now on a CPU cycle and an APB cycle last what `clock` says.
+    pub fn set_clock(&mut self, clock: CpuClock) {
+        self.clock = clock;
+    }
+
+    /// A device tells that a clock selection register was written.
+    pub fn clock_written(&mut self) {
+        self.clock_written = true;
+    }
+
+    /// Whether a clock selection register was written since the last call.
+    pub fn take_clock_written(&mut self) -> bool {
+        std::mem::take(&mut self.clock_written)
+    }
+
     /// Reports an operation the device cannot perform faithfully: it becomes a diagnostic that
     /// names the instruction, and `--strict-mmio` stops on it.
     pub fn unsupported(&mut self, what: Unsupported) {
```

`SYSTEM` tells the context that a clock register was written, instead of keeping a flag the machine polls:

```diff
--- a/crates/soc-esp32c3/src/periph/system.rs
+++ b/crates/soc-esp32c3/src/periph/system.rs
@@ -85,7 +85,6 @@ const XTAL_FREQ_MHZ: u32 = crate::clock::XTAL_MHZ;
 pub struct System {
     sysclk_writable: u32,
     registers: Storage,
-    clock_changed: bool,
 }
 
 impl System {
@@ -94,7 +93,6 @@ impl System {
         System {
             sysclk_writable: SYSCLK_RESET,
             registers: Storage::accepted(SYSTEM_BASE, &ACCEPTED),
-            clock_changed: false,
         }
     }
 
@@ -108,11 +106,6 @@ impl System {
             self.registers.read(CPU_PER_CONF).unwrap_or(0),
         )
     }
-
-    /// Whether a clock register was written since the last call.
-    pub(crate) fn take_clock_change(&mut self) -> bool {
-        std::mem::take(&mut self.clock_changed)
-    }
 }
 
 impl Default for System {
@@ -129,15 +122,15 @@ impl Peripheral<Ctx> for System {
         self.registers.read(offset)
     }
 
-    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
+    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
         match offset {
             SYSCLK_CONF => {
                 self.sysclk_writable = value & SYSCLK_WRITABLE;
-                self.clock_changed = true;
+                ctx.clock_written();
                 true
             }
             CPU_PER_CONF => {
-                self.clock_changed = true;
+                ctx.clock_written();
                 self.registers.write(offset, value)
             }
             _ => self.registers.write(offset, value),
@@ -202,13 +195,13 @@ mod tests {
         let ctx = &mut Ctx::default();
         let mut sys = System::new();
         assert_eq!(sys.clock_registers(), (0x2_8001, 0xC), "reset: BA §5");
-        assert!(!sys.take_clock_change());
+        assert!(!ctx.take_clock_written());
         let cpu_per_conf = svd_regs::esp32c3::addr("SYSTEM", "CPU_PER_CONF") - SYSTEM_BASE;
         assert!(sys.write(ctx, cpu_per_conf, 0xD));
-        assert!(sys.take_clock_change());
-        assert!(!sys.take_clock_change(), "taken once");
+        assert!(ctx.take_clock_written());
+        assert!(!ctx.take_clock_written(), "taken once");
         sys.write(ctx, SYSCLK_CONF, 0x400);
-        assert!(sys.take_clock_change());
+        assert!(ctx.take_clock_written());
         assert_eq!(sys.clock_registers(), (0x2_8400, 0xD));
         sys.write(
             ctx,
@@ -216,7 +209,7 @@ mod tests {
             0,
         );
         assert!(
-            !sys.take_clock_change(),
+            !ctx.take_clock_written(),
             "a clock gate is not the CPU clock"
         );
     }
```

- [ ] **Step 5: The machine advances the context; `VirtualClock` goes**

```diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -9,7 +9,7 @@ use crate::rom::RomImage;
 use cpu_rv32::{
     DatasheetTiming, Exception, Hart, HartConfig, MisalignedPolicy, StepOutcome, VendorCsrs,
 };
-use emu_core::{Cycles, Frequency, TimingModel, TraceHash, VirtualClock};
+use emu_core::{Cycles, Frequency, TimingModel, TraceHash};
 
 /// The two serial consoles the mask ROM prints on.
 #[derive(Clone, Copy, Debug, PartialEq, Eq)]
@@ -37,8 +37,6 @@ pub struct Esp32C3 {
     hart: Hart,
     bus: SocBus,
     timing: DatasheetTiming,
-    /// Device time: follows the CPU clock the firmware selects.
-    clock: VirtualClock,
     trace: Option<TraceHash>,
 }
 
@@ -79,17 +77,18 @@ impl Esp32C3 {
             misaligned: MisalignedPolicy::Allow,
             vendor_csrs: VendorCsrs::Esp32C3,
         };
-        let bus = SocBus::new(rom, flash, config);
+        let mut bus = SocBus::new(rom, flash, config);
         let (sysclk_conf, cpu_per_conf) = bus.periph.system.clock_registers();
         // INVARIANT: the reset values of the two clock registers select the crystal with a divider
-        // of 2 (decision D21), for which a frequency always exists: 20 MHz. The fallback is never
-        // taken; it is the undivided crystal, 40 MHz, and not the reset frequency.
-        let frequency = clock::cpu_frequency(sysclk_conf, cpu_per_conf).unwrap_or(clock::XTAL);
+        // of 2 (decisions D21 and E8), which is a selection `cpu_clock` knows: the fallback is never
+        // taken, and it is that same power-on clock.
+        bus.ctx.set_clock(
+            clock::cpu_clock(sysclk_conf, cpu_per_conf).unwrap_or(clock::CpuClock::POWER_ON),
+        );
         Esp32C3 {
             hart: Hart::new(RESET_PC, hart_config),
             bus,
             timing: DatasheetTiming,
-            clock: VirtualClock::new(frequency),
             trace: config.trace_hash.then(TraceHash::new),
         }
     }
@@ -100,6 +99,7 @@ impl Esp32C3 {
         while self.hart.cycles.get() < limit.get() {
             let pc = self.hart.pc;
             self.bus.set_pc(pc);
+            let before = self.hart.cycles.get();
             match self.hart.step(&mut self.bus, &self.timing) {
                 StepOutcome::Waiting => return StopReason::Idle,
                 // An `ecall` is a request to the trap handler, not a fault worth reporting.
@@ -112,7 +112,9 @@ impl Esp32C3 {
                 }
                 _ => {}
             }
-            if self.bus.periph.system.take_clock_change() {
+            // Device time follows the CPU: the cycles of this instruction, at the clock it ran on.
+            self.bus.ctx.advance(self.hart.cycles.get() - before);
+            if self.bus.ctx.take_clock_written() {
                 self.retune(pc);
             }
             if let Some(trace) = &mut self.trace {
@@ -129,8 +131,8 @@ impl Esp32C3 {
     /// cycles last what the new selection says.
     fn retune(&mut self, pc: u32) {
         let (sysclk_conf, cpu_per_conf) = self.bus.periph.system.clock_registers();
-        match clock::cpu_frequency(sysclk_conf, cpu_per_conf) {
-            Some(frequency) => self.clock.set_frequency(self.hart.cycles, frequency),
+        match clock::cpu_clock(sysclk_conf, cpu_per_conf) {
+            Some(selected) => self.bus.ctx.set_clock(selected),
             // Decision D22: the previous frequency stays, and the selection is reported.
             None => self.bus.ctx.diag.record(Diagnostic::Unsupported {
                 what: Unsupported::ClockSource {
@@ -144,14 +146,14 @@ impl Esp32C3 {
 
     /// The CPU clock frequency the firmware selected.
     #[must_use]
-    pub const fn cpu_frequency(&self) -> Frequency {
-        self.clock.frequency()
+    pub fn cpu_frequency(&self) -> Frequency {
+        self.bus.ctx.clock().cpu_frequency()
     }
 
-    /// Time elapsed on the device since reset, following every CPU clock switch.
+    /// Time elapsed on the device since power-on, following every CPU clock switch.
     #[must_use]
     pub fn device_time_nanos(&self) -> u64 {
-        self.clock.nanos_at(self.hart.cycles)
+        clock::TIME_BASE.cycles_to_nanos(self.bus.ctx.now())
     }
 
     /// CPU clock cycles elapsed since reset.
```

`VirtualClock` has no user left:

```diff
--- a/crates/emu-core/src/time.rs
+++ b/crates/emu-core/src/time.rs
@@ -1,10 +1,14 @@
-//! Virtual time, measured in `SoC` clock cycles.
+//! Virtual time, measured in clock cycles.
 use std::num::NonZeroU64;
 use std::ops::Add;
 
 const NANOS_PER_SECOND: u128 = 1_000_000_000;
 
-/// A point in virtual time, counted in cycles of the emulated `SoC` clock.
+/// A count of clock cycles: a point in virtual time, or a duration.
+///
+/// Which clock is counted is said where the value lives. A machine keeps its device time in cycles
+/// of one fixed time-base clock, which every clock of the chip divides, so an event stays at its
+/// instant when the CPU clock is switched or stopped; a hart counts the cycles of its own clock.
 #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
 pub struct Cycles(pub u64);
 
@@ -47,9 +51,6 @@ impl Frequency {
     }
 }
 
-const ATTOS_PER_SECOND: u128 = 1_000_000_000_000_000_000;
-const ATTOS_PER_NANO: u128 = 1_000_000_000;
-
 impl Frequency {
     /// A frequency known at compile time.
     #[must_use]
@@ -58,60 +59,6 @@ impl Frequency {
     }
 }
 
-/// Converts the cycle counter of a CPU whose clock frequency changes into elapsed time.
-///
-/// Time is kept in attoseconds, so a cycle that is not a whole number of nanoseconds (6.25 ns at
-/// 160 MHz) accumulates without drift.
-#[derive(Clone, Copy, Debug, PartialEq, Eq)]
-pub struct VirtualClock {
-    frequency: Frequency,
-    /// Cycle counter at the last frequency change.
-    since: Cycles,
-    /// Time elapsed at `since`.
-    elapsed_attos: u128,
-}
-
-impl VirtualClock {
-    #[must_use]
-    pub const fn new(frequency: Frequency) -> VirtualClock {
-        VirtualClock {
-            frequency,
-            since: Cycles::ZERO,
-            elapsed_attos: 0,
-        }
-    }
-
-    #[must_use]
-    pub const fn frequency(&self) -> Frequency {
-        self.frequency
-    }
-
-    fn attos_at(&self, now: Cycles) -> u128 {
-        // `u64::MAX` cycles times 1e18 stays below `u128::MAX` (1.8e37 < 3.4e38).
-        let cycles = u128::from(now.0.saturating_sub(self.since.0));
-        self.elapsed_attos
-            .saturating_add(cycles * ATTOS_PER_SECOND / u128::from(self.frequency.hz()))
-    }
-
-    /// From cycle `now` on, a cycle lasts `1 / frequency`.
-    ///
-    /// The cycle counter only grows, so `now` is never before the previous change. Should a caller
-    /// pass an earlier one, it is taken as the previous change itself: moving the anchor back would
-    /// count the cycles in between a second time.
-    pub fn set_frequency(&mut self, now: Cycles, frequency: Frequency) {
-        let now = Cycles(now.0.max(self.since.0));
-        self.elapsed_attos = self.attos_at(now);
-        self.since = now;
-        self.frequency = frequency;
-    }
-
-    /// Nanoseconds elapsed when the cycle counter reads `now`, saturating at `u64::MAX`.
-    #[must_use]
-    pub fn nanos_at(&self, now: Cycles) -> u64 {
-        u64::try_from(self.attos_at(now) / ATTOS_PER_NANO).unwrap_or(u64::MAX)
-    }
-}
-
 #[cfg(test)]
 mod tests {
     use super::*;
@@ -133,89 +80,6 @@ mod tests {
         assert_eq!(f.cycles_to_nanos(Cycles(u64::MAX)), u64::MAX);
     }
 
-    fn mhz(value: u64) -> Frequency {
-        Frequency::from_hz(value * 1_000_000).unwrap()
-    }
-
-    #[test]
-    fn a_virtual_clock_adds_up_time_across_frequency_switches() {
-        let mut clock = VirtualClock::new(mhz(20));
-        assert_eq!(
-            clock.nanos_at(Cycles(20_000_000)),
-            1_000_000_000,
-            "20 M cycles at 20 MHz"
-        );
-        clock.set_frequency(Cycles(20_000_000), mhz(80));
-        assert_eq!(clock.frequency(), mhz(80));
-        assert_eq!(
-            clock.nanos_at(Cycles(100_000_000)),
-            2_000_000_000,
-            "+80 M cycles at 80 MHz"
-        );
-        clock.set_frequency(Cycles(100_000_000), mhz(160));
-        assert_eq!(
-            clock.nanos_at(Cycles(260_000_000)),
-            3_000_000_000,
-            "+160 M cycles at 160 MHz"
-        );
-    }
-
-    #[test]
-    fn a_cycle_that_is_not_a_whole_nanosecond_does_not_drift() {
-        // 6.25 ns per cycle at 160 MHz: switching on an odd cycle must not lose the fraction.
-        let mut clock = VirtualClock::new(mhz(160));
-        clock.set_frequency(Cycles(1), mhz(160));
-        clock.set_frequency(Cycles(3), mhz(160));
-        assert_eq!(clock.nanos_at(Cycles(4)), 25);
-        // Asked about a cycle before the last switch (cycle 3, 18.75 ns), the clock answers the time
-        // of that switch: it keeps no history, and it never goes back past it.
-        assert_eq!(
-            clock.nanos_at(Cycles(1)),
-            18,
-            "time does not run backwards before the last switch"
-        );
-    }
-
-    #[test]
-    fn a_frequency_change_dated_before_the_last_one_does_not_count_cycles_twice() {
-        // 1 µs per cycle up to cycle 100, then 2 MHz. A change dated at cycle 50 cannot move the
-        // anchor back: the 50 cycles from 50 to 100 are already in the elapsed time.
-        let mut clock = VirtualClock::new(mhz(1));
-        clock.set_frequency(Cycles(100), mhz(2));
-        clock.set_frequency(Cycles(50), mhz(1));
-        assert_eq!(clock.frequency(), mhz(1), "the new frequency applies");
-        assert_eq!(clock.nanos_at(Cycles(100)), 100_000, "from the last change");
-        assert_eq!(clock.nanos_at(Cycles(110)), 110_000);
-    }
-
-    #[test]
-    fn a_period_that_is_not_a_whole_number_of_attoseconds_loses_less_than_one_per_switch() {
-        // 40 MHz / 1024, the slowest crystal division, rounded down to 39 062 Hz: one cycle lasts
-        // 25 600 327 684 194.357… attoseconds. Each switch floors the time elapsed so far, so 1 000
-        // switches lose less than 1 000 attoseconds: far below a nanosecond after a whole second.
-        let slow = Frequency::from_hz(39_062).unwrap();
-        let mut clock = VirtualClock::new(slow);
-        for cycle in 1..=1_000 {
-            clock.set_frequency(Cycles(cycle), slow);
-        }
-        let nanos = clock.nanos_at(Cycles(39_062));
-        assert!(
-            (999_999_999..=1_000_000_000).contains(&nanos),
-            "one second of cycles: {nanos} ns"
-        );
-        assert_eq!(
-            VirtualClock::new(slow).nanos_at(Cycles(39_062)),
-            1_000_000_000,
-            "without a switch the second is exact"
-        );
-    }
-
-    #[test]
-    fn a_virtual_clock_saturates_instead_of_overflowing() {
-        let clock = VirtualClock::new(Frequency::from_hz(1).unwrap());
-        assert_eq!(clock.nanos_at(Cycles(u64::MAX)), u64::MAX);
-    }
-
     #[test]
     fn a_frequency_can_be_a_constant() {
         const ONE: Frequency = Frequency::new(std::num::NonZeroU64::MIN);
```

```diff
--- a/crates/emu-core/src/lib.rs
+++ b/crates/emu-core/src/lib.rs
@@ -11,6 +11,6 @@ pub use bus::{AccessKind, AccessSize, Bus, BusError, ReadOutcome};
 pub use memory::{PAGE_SHIFT, Ram};
 pub use peripheral::Peripheral;
 pub use scheduler::{EventId, Scheduler};
-pub use time::{Cycles, Frequency, VirtualClock};
+pub use time::{Cycles, Frequency};
 pub use timing::{InstrClass, TimingModel};
 pub use trace::TraceHash;
```

- [ ] **Step 6: Run the suite**

Run: `cargo test --workspace > /tmp/t2.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t2.log`
Expected: `exit=0`, `587`: five tests of `VirtualClock` left with it, one was added. The tests of `machine.rs` that pin device time (`device_time_at_the_reset_clock_is_50_ns_per_cycle`, `device_time_follows_the_cpu_clock_the_firmware_selects`) pass unchanged.

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/emu-core/src crates/soc-esp32c3/src
/usr/bin/git commit -m "refactor(soc): device time is counted in cycles of a 160 MHz time base

It lives in the context of the peripherals; VirtualClock is removed.

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/emu-core/src crates/soc-esp32c3/src
```

---

## Task 3: The context in full: wake-ups, signals, interrupt sources, reset requests

The rest of the mechanism of ADR 0003, with nothing using it yet: an addition, behaviour unchanged.

**Files:**
- Create: `crates/soc-esp32c3/src/reset.rs`
- Modify: `crates/soc-esp32c3/src/ctx.rs`, `crates/soc-esp32c3/src/lib.rs`

**Interfaces:**
- Consumes: `emu_core::Scheduler<E>` (`new()`, `schedule(&mut self, at: Cycles, event: E) -> EventId`, `cancel(&mut self, id: EventId)`, `next_due(&self) -> Option<Cycles>`, `pop_due(&mut self, now: Cycles) -> Option<(Cycles, E)>`), `emu_core::EventId`; `periph::INTMATRIX_BASE`; `svd_regs::REGISTER_BYTES`; the `Ctx` of Task 2.
- Produces:
  - `reset::ResetLevel { Cpu, Core, System }` (ordered), `reset::ResetCause { SoftwareSystem, Mwdt0Core, Mwdt1Core, RtcWatchdogCore, Mwdt0Cpu, SoftwareCpu, RtcWatchdogCpu, RtcWatchdogSystem, Mwdt1Cpu, SuperWatchdog }` with `code(self) -> u32` and `level(self) -> ResetLevel`, `reset::CAUSE_POWER_ON: u32` (1).
  - `ctx::Source` with `const fn Source::of(map_register: &str) -> Source` (the source routed by `INTERRUPT_CORE0.<map_register>`; an unknown name fails the build) and `number(self) -> u32`.
  - `ctx::Event { MmuFault { vaddr: u32, content: u32 }, FlashWindowStore { instruction_bus: bool } }`: the later tasks add one variant per device they bring.
  - `ctx::Deadline` (`Default`) with `arm(&mut self, ctx: &mut Ctx, at: Cycles, event: Event)` and `called(&mut self, now: Cycles)`.
  - On `Ctx`: `sleep_until(&mut self, instant: Cycles)`, `schedule(&mut self, at: Cycles, event: Event) -> EventId`, `cancel(&mut self, id: EventId)`, `next_wake(&self) -> Option<Cycles>`, `wake_due(&self) -> bool`, `take_due(&mut self) -> Option<Event>`, `signal(&mut self, event: Event)`, `take_signal(&mut self) -> Option<Event>`, `irq(&mut self, source: Source, asserted: bool)`, `sources(&self) -> u64`, `interrupts_changed(&mut self)`, `take_interrupts_changed(&mut self) -> bool`, `is_flagged(&self) -> bool`, `request_reset(&mut self, cause: ResetCause)`, `take_reset(&mut self) -> Option<ResetCause>`.

- [ ] **Step 1: Write the failing tests**

The codes and levels are TRM table 6.1-1 as tabulated in TW §D.3. `crates/soc-esp32c3/src/reset.rs`, test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn causes_have_the_codes_and_levels_of_the_reference_manual() {
        // TRM table 6.1-1, as tabulated in TW §D.3.
        for (cause, code, level) in [
            (ResetCause::SoftwareSystem, 0x03, ResetLevel::Core),
            (ResetCause::Mwdt0Core, 0x07, ResetLevel::Core),
            (ResetCause::Mwdt1Core, 0x08, ResetLevel::Core),
            (ResetCause::RtcWatchdogCore, 0x09, ResetLevel::Core),
            (ResetCause::Mwdt0Cpu, 0x0B, ResetLevel::Cpu),
            (ResetCause::SoftwareCpu, 0x0C, ResetLevel::Cpu),
            (ResetCause::RtcWatchdogCpu, 0x0D, ResetLevel::Cpu),
            (ResetCause::RtcWatchdogSystem, 0x10, ResetLevel::System),
            (ResetCause::Mwdt1Cpu, 0x11, ResetLevel::Cpu),
            (ResetCause::SuperWatchdog, 0x12, ResetLevel::System),
        ] {
            assert_eq!((cause.code(), cause.level()), (code, level), "{cause:?}");
        }
        assert!(ResetLevel::Cpu < ResetLevel::Core && ResetLevel::Core < ResetLevel::System);
    }
}
```

The test module of `crates/soc-esp32c3/src/ctx.rs` (the file has none yet; it goes at its end). Source 37 is `SYSTIMER_TARGET0`, 50 is `CPU_INTR_FROM_CPU_0` (IR §3.2):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const TICK: Source = Source::of("SYSTIMER_TARGET0_INT_MAP");
    const SOFTWARE: Source = Source::of("CPU_INTR_FROM_CPU_0_MAP");
    // Any event will do: the context does not look inside them.
    const FIRST: Event = Event::FlashWindowStore {
        instruction_bus: true,
    };
    const SECOND: Event = Event::FlashWindowStore {
        instruction_bus: false,
    };
    const THIRD: Event = Event::MmuFault {
        vaddr: 0x4200_0000,
        content: 0x100,
    };

    #[test]
    fn a_source_is_numbered_by_its_map_register() {
        // IR §3.2: SYSTIMER_TARGET0 is source 37, CPU_INTR_FROM_CPU_0 is 50, CACHE_CORE0_ACS the last, 61.
        assert_eq!(TICK.number(), 37);
        assert_eq!(SOFTWARE.number(), 50);
        assert_eq!(Source::of("MAC_INTR_MAP").number(), 0);
        assert_eq!(Source::of("CACHE_CORE0_ACS_INT_MAP").number(), 61);
    }

    #[test]
    fn device_time_follows_the_cpu_clock_and_never_goes_back() {
        let mut ctx = Ctx::default();
        ctx.advance(10);
        assert_eq!(
            ctx.now(),
            Cycles(80),
            "20 MHz: 8 time-base cycles per CPU cycle"
        );
        ctx.set_clock(CpuClock {
            cpu_period: 1,
            apb_period: 2,
        });
        ctx.advance(10);
        assert_eq!(ctx.now(), Cycles(90));
        ctx.sleep_until(Cycles(1_000));
        assert_eq!(ctx.now(), Cycles(1_000));
        ctx.sleep_until(Cycles(500));
        assert_eq!(
            ctx.now(),
            Cycles(1_000),
            "an instant in the past changes nothing"
        );
    }

    #[test]
    fn wake_ups_come_due_in_time_order_and_a_cancelled_one_never_does() {
        let mut ctx = Ctx::default();
        assert_eq!(ctx.next_wake(), None);
        assert!(!ctx.wake_due());
        let late = ctx.schedule(Cycles(300), THIRD);
        ctx.schedule(Cycles(100), FIRST);
        ctx.schedule(Cycles(200), SECOND);
        assert_eq!(ctx.next_wake(), Some(Cycles(100)));
        assert_eq!(ctx.take_due(), None, "nothing is due at time 0");
        ctx.sleep_until(Cycles(250));
        assert!(ctx.wake_due());
        assert_eq!(ctx.take_due(), Some(FIRST));
        assert_eq!(ctx.take_due(), Some(SECOND));
        assert_eq!(ctx.take_due(), None);
        assert_eq!(ctx.next_wake(), Some(Cycles(300)));
        ctx.cancel(late);
        assert_eq!(ctx.next_wake(), None);
        ctx.sleep_until(Cycles(1_000));
        assert!(!ctx.wake_due());
    }

    #[test]
    fn a_deadline_keeps_the_earliest_call_and_replaces_a_later_one() {
        let mut ctx = Ctx::default();
        let mut deadline = Deadline::default();
        deadline.arm(&mut ctx, Cycles(500), FIRST);
        deadline.arm(&mut ctx, Cycles(900), FIRST);
        assert_eq!(
            ctx.next_wake(),
            Some(Cycles(500)),
            "the earlier call is kept"
        );
        deadline.arm(&mut ctx, Cycles(200), FIRST);
        assert_eq!(
            ctx.next_wake(),
            Some(Cycles(200)),
            "an earlier deadline replaces it"
        );
        ctx.sleep_until(Cycles(200));
        assert_eq!(ctx.take_due(), Some(FIRST));
        assert_eq!(ctx.take_due(), None, "the replaced call was withdrawn");
        deadline.called(ctx.now());
        deadline.arm(&mut ctx, Cycles(900), FIRST);
        assert_eq!(
            ctx.next_wake(),
            Some(Cycles(900)),
            "once called, the deadline arms again"
        );
    }

    #[test]
    fn signals_are_delivered_in_posting_order() {
        let mut ctx = Ctx::default();
        ctx.signal(FIRST);
        ctx.signal(SECOND);
        assert_eq!(ctx.take_signal(), Some(FIRST));
        assert_eq!(ctx.take_signal(), Some(SECOND));
        assert_eq!(ctx.take_signal(), None);
    }

    #[test]
    fn source_levels_are_kept_and_a_change_is_flagged_once() {
        let mut ctx = Ctx::default();
        assert!(!ctx.take_interrupts_changed());
        ctx.irq(TICK, true);
        ctx.irq(SOFTWARE, true);
        assert_eq!(ctx.sources(), (1 << 37) | (1 << 50));
        assert!(ctx.take_interrupts_changed());
        ctx.irq(TICK, true);
        assert!(
            !ctx.take_interrupts_changed(),
            "the same level is no change"
        );
        ctx.irq(TICK, false);
        assert_eq!(ctx.sources(), 1 << 50);
        assert!(ctx.take_interrupts_changed());
        ctx.interrupts_changed();
        assert!(
            ctx.take_interrupts_changed(),
            "a controller register was written"
        );
    }

    #[test]
    fn the_first_reset_request_of_an_instruction_wins() {
        let mut ctx = Ctx::default();
        assert_eq!(ctx.take_reset(), None);
        ctx.request_reset(ResetCause::SoftwareCpu);
        ctx.request_reset(ResetCause::Mwdt0Core);
        assert_eq!(ctx.take_reset(), Some(ResetCause::SoftwareCpu));
        assert_eq!(ctx.take_reset(), None);
    }

    #[test]
    fn a_refusal_becomes_a_diagnostic_that_names_the_instruction() {
        let mut ctx = Ctx::new(true);
        ctx.set_pc(0x4000_1234);
        ctx.unsupported(Unsupported::ShaDma);
        assert_eq!(
            ctx.diag.take_violation(),
            Some(Diagnostic::Unsupported {
                what: Unsupported::ShaDma,
                pc: 0x4000_1234
            })
        );
        assert_eq!(ctx.take_unsupported(), Some(Unsupported::ShaDma));
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p soc-esp32c3 --lib > /tmp/t3.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, compile errors in the tests of `ctx.rs`: `cannot find type 'Source'`, `'Deadline'`, `'Event'`, `'ResetCause'`, no method named `next_wake`, `take_due`, `sleep_until`. `reset.rs` is not compiled yet: Step 3 declares the module, and its test needs `ResetCause` and `ResetLevel`.

- [ ] **Step 3: Reset causes and levels**

`crates/soc-esp32c3/src/reset.rs`, above its test module:

```rust
//! Resets: how much of the chip each one returns to its reset state, and the cause it leaves in
//! `RTC_CNTL.RESET_STATE`. Sources: TRM §6.1 (figure 6.1-1, table 6.1-1); reports IR §4.2 and TW §D.

/// How much of the chip a reset returns to its reset state (TRM §6.1.3; TW §D.2).
///
/// The hardware has a fourth level, the chip reset of a power-on: in this model it is the
/// construction of the machine, and nothing asks for one while it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResetLevel {
    /// "Only resets CPU core", and with it the `SENSITIVE` permission registers. Every other
    /// peripheral register survives, the clock selection included.
    Cpu,
    /// "The whole digital system except RTC": the CPU and every digital peripheral. `RTC_CNTL`
    /// survives, with the RTC watchdog and its counter, and so does the recorded PC.
    Core,
    /// "The whole digital system, including RTC". The RTC timer keeps counting (TRM §9.3.3) and
    /// the memories keep their content (TRM §6.1.1).
    System,
}

/// Why the chip was reset: the code `RTC_CNTL.RESET_STATE[5:0]` reads at the next boot (TRM table
/// 6.1-1), which the ROM prints as `rst:0x… (NAME)` (report TW §D.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetCause {
    /// `RTC_CNTL.OPTIONS0` bit 31, `SW_SYS_RST` (`ROM3:software_reset@0x40048b74`): `RTC_SW_SYS_RST`.
    SoftwareSystem,
    /// Main watchdog 0, stage action 3, or its flash-boot protection: `TG0WDT_SYS_RST`.
    Mwdt0Core,
    /// Main watchdog 1, stage action 3: `TG1WDT_SYS_RST`.
    Mwdt1Core,
    /// RTC watchdog, stage action 3: `RTCWDT_SYS_RST`.
    RtcWatchdogCore,
    /// Main watchdog 0, stage action 2: `TG0WDT_CPU_RST`.
    Mwdt0Cpu,
    /// `RTC_CNTL.OPTIONS0` bit 5, `SW_PROCPU_RST` (`ROM3:software_reset_cpu@0x40048b84`), what
    /// `esp_restart` ends with: `RTC_SW_CPU_RST`.
    SoftwareCpu,
    /// RTC watchdog, stage action 2: `RTCWDT_CPU_RST`.
    RtcWatchdogCpu,
    /// RTC watchdog, stage action 4, or its flash-boot protection: `RTCWDT_RTC_RST`.
    RtcWatchdogSystem,
    /// Main watchdog 1, stage action 2: `TG1WDT_CPU_RST`.
    Mwdt1Cpu,
    /// The super watchdog was not fed: `SUPER_WDT_RST`.
    SuperWatchdog,
}

/// `RESET_STATE[5:0]` after a power-on (chip) reset: `POWERON`.
pub const CAUSE_POWER_ON: u32 = 0x01;

impl ResetCause {
    /// The code of TRM table 6.1-1 (ESP-IDF `soc/reset_reasons.h`).
    #[must_use]
    pub const fn code(self) -> u32 {
        match self {
            ResetCause::SoftwareSystem => 0x03,
            ResetCause::Mwdt0Core => 0x07,
            ResetCause::Mwdt1Core => 0x08,
            ResetCause::RtcWatchdogCore => 0x09,
            ResetCause::Mwdt0Cpu => 0x0B,
            ResetCause::SoftwareCpu => 0x0C,
            ResetCause::RtcWatchdogCpu => 0x0D,
            ResetCause::RtcWatchdogSystem => 0x10,
            ResetCause::Mwdt1Cpu => 0x11,
            ResetCause::SuperWatchdog => 0x12,
        }
    }

    /// The level the cause acts at (TRM table 6.1-1).
    #[must_use]
    pub const fn level(self) -> ResetLevel {
        match self {
            ResetCause::Mwdt0Cpu
            | ResetCause::SoftwareCpu
            | ResetCause::RtcWatchdogCpu
            | ResetCause::Mwdt1Cpu => ResetLevel::Cpu,
            ResetCause::SoftwareSystem
            | ResetCause::Mwdt0Core
            | ResetCause::Mwdt1Core
            | ResetCause::RtcWatchdogCore => ResetLevel::Core,
            ResetCause::RtcWatchdogSystem | ResetCause::SuperWatchdog => ResetLevel::System,
        }
    }
}
```

```diff
--- a/crates/soc-esp32c3/src/lib.rs
+++ b/crates/soc-esp32c3/src/lib.rs
@@ -20,6 +20,7 @@ pub mod flash;
 pub mod machine;
 pub mod memmap;
 pub mod periph;
+pub mod reset;
 pub mod rom;
 #[cfg(any(test, feature = "test-support"))]
 pub mod test_support;
```

- [ ] **Step 4: The context**

Replace everything above the test module of `crates/soc-esp32c3/src/ctx.rs` with:

```rust
//! What a peripheral reaches beyond its own registers: the one door between a device model and the
//! rest of the machine (ADR 0003).
use crate::clock::CpuClock;
use crate::diag::{Diagnostic, Diagnostics, Unsupported};
use crate::periph::INTMATRIX_BASE;
use crate::reset::ResetCause;
use emu_core::{Cycles, EventId, Scheduler};
use std::collections::VecDeque;
use svd_regs::REGISTER_BYTES;

/// One of the 62 peripheral interrupt sources: source `n` is the one `INTERRUPT_CORE0` routes with
/// the map register at offset 4 × `n` (report IR §1.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Source(u8);

impl Source {
    /// The source routed by `INTERRUPT_CORE0.<map_register>`. Used in a `const`, an unknown register
    /// name fails the build.
    #[must_use]
    pub const fn of(map_register: &str) -> Source {
        let offset = svd_regs::esp32c3::addr("INTERRUPT_CORE0", map_register) - INTMATRIX_BASE;
        // The 62 map registers end at offset 0xF4: the quotient fits in the 6 bits kept.
        Source(((offset / REGISTER_BYTES) & 0x3F) as u8)
    }

    /// The source number, 0 to 61.
    #[must_use]
    pub const fn number(self) -> u32 {
        self.0 as u32
    }

    const fn mask(self) -> u64 {
        1 << self.0
    }
}

/// What a device is called for, besides register accesses: at the instant it asked for (a wake-up),
/// or because another part of the machine has something to tell it.
///
/// A wake-up is a call, not an order: the device works out what is due at the time it is called.
/// A wake-up that comes early, or for a state that is gone (after a reset, a feed, a reprogrammed
/// alarm), finds nothing to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// To `EXTMEM`: a flash window was accessed through an invalid MMU entry, whose content is given.
    MmuFault { vaddr: u32, content: u32 },
    /// To `EXTMEM`: a store reached a flash window.
    FlashWindowStore { instruction_bus: bool },
}

/// The wake-up a device keeps for its next deadline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Deadline {
    armed: Option<(Cycles, EventId)>,
}

impl Deadline {
    /// Makes sure the device is called with `event` at `at`, or before. A call already due earlier
    /// is kept: the device re-arms when it is called.
    pub fn arm(&mut self, ctx: &mut Ctx, at: Cycles, event: Event) {
        match self.armed {
            Some((due, _)) if due <= at => {}
            previous => {
                if let Some((_, id)) = previous {
                    ctx.cancel(id);
                }
                self.armed = Some((at, ctx.schedule(at, event)));
            }
        }
    }

    /// Forgets a wake-up whose instant has come: the device is being called for it.
    pub fn called(&mut self, now: Cycles) {
        if self.armed.is_some_and(|(due, _)| due <= now) {
            self.armed = None;
        }
    }
}

/// The context of a peripheral access. The bus owns it and hands it to every register read and
/// write; the machine reads what the peripherals left in it after each instruction.
#[derive(Debug)]
pub struct Ctx {
    /// What the run has to tell the user. Reached from outside the crate through the machine.
    pub(crate) diag: Diagnostics,
    /// PC of the instruction being executed, for diagnostics.
    pc: u32,
    /// Device time: cycles of the time base ([`crate::clock::TIME_BASE`]) since power-on. It only
    /// grows, across resets too.
    now: Cycles,
    /// The CPU and APB clocks the firmware selected.
    clock: CpuClock,
    /// A clock selection register was written since the machine last looked.
    clock_written: bool,
    /// Wake-ups the devices asked for, by device time.
    events: Scheduler<Event>,
    /// Device time of the earliest wake-up, `u64::MAX` without one: the run loop compares it with
    /// the time at every instruction.
    next_wake: u64,
    /// Events to deliver before the instruction that posted them ends, oldest first.
    signals: VecDeque<Event>,
    /// Level of each interrupt source, bit `n` for source `n`.
    sources: u64,
    /// A source level or a register of the interrupt controller changed since the machine last looked.
    interrupts_changed: bool,
    /// The reset a device asked for, if any: the first request of an instruction wins.
    reset: Option<ResetCause>,
    /// Every refusal in the order it was reported, for the tests of the device models: the
    /// diagnostics list a repeated refusal once, and a device test wants to see each one.
    #[cfg(test)]
    refused: VecDeque<Unsupported>,
}

impl Ctx {
    #[must_use]
    pub fn new(strict: bool) -> Ctx {
        Ctx {
            diag: Diagnostics::new(strict),
            pc: 0,
            now: Cycles::ZERO,
            clock: CpuClock::POWER_ON,
            clock_written: false,
            events: Scheduler::new(),
            next_wake: u64::MAX,
            signals: VecDeque::new(),
            sources: 0,
            interrupts_changed: false,
            reset: None,
            #[cfg(test)]
            refused: VecDeque::new(),
        }
    }

    /// Tells the context which instruction is running, so diagnostics can name it.
    pub fn set_pc(&mut self, pc: u32) {
        self.pc = pc;
    }

    /// PC of the instruction being executed.
    #[must_use]
    pub const fn pc(&self) -> u32 {
        self.pc
    }

    /// Device time: cycles of the time base since power-on.
    #[must_use]
    pub const fn now(&self) -> Cycles {
        self.now
    }

    /// The CPU has run `cpu_cycles` cycles of its clock: device time moves on by as much.
    pub fn advance(&mut self, cpu_cycles: u64) {
        self.now = self.now + cpu_cycles.saturating_mul(self.clock.cpu_period);
    }

    /// Device time jumps to `instant` while the CPU sleeps. Time never goes back: an instant in the
    /// past changes nothing.
    pub fn sleep_until(&mut self, instant: Cycles) {
        self.now = self.now.max(instant);
    }

    /// The CPU and APB clocks in use.
    #[must_use]
    pub const fn clock(&self) -> CpuClock {
        self.clock
    }

    /// From now on a CPU cycle and an APB cycle last what `clock` says.
    pub fn set_clock(&mut self, clock: CpuClock) {
        self.clock = clock;
    }

    /// A device tells that a clock selection register was written.
    pub fn clock_written(&mut self) {
        self.clock_written = true;
    }

    /// Whether a clock selection register was written since the last call.
    pub fn take_clock_written(&mut self) -> bool {
        std::mem::take(&mut self.clock_written)
    }

    /// Asks for `event` to be delivered at device time `at`; an instant already past is delivered
    /// when the current instruction ends.
    pub fn schedule(&mut self, at: Cycles, event: Event) -> EventId {
        self.next_wake = self.next_wake.min(at.get());
        self.events.schedule(at, event)
    }

    /// Withdraws a wake-up; one that was already delivered or withdrawn is ignored.
    pub fn cancel(&mut self, id: EventId) {
        self.events.cancel(id);
        self.refresh_next_wake();
    }

    fn refresh_next_wake(&mut self) {
        self.next_wake = self.events.next_due().map_or(u64::MAX, Cycles::get);
    }

    /// Device time of the earliest wake-up, if any.
    #[must_use]
    pub fn next_wake(&self) -> Option<Cycles> {
        (self.next_wake != u64::MAX).then_some(Cycles(self.next_wake))
    }

    /// Whether a wake-up is due now. One comparison: the run loop asks after every instruction.
    #[must_use]
    pub const fn wake_due(&self) -> bool {
        self.next_wake <= self.now.get()
    }

    /// The oldest wake-up that is due, if any.
    pub fn take_due(&mut self) -> Option<Event> {
        let (_, event) = self.events.pop_due(self.now)?;
        self.refresh_next_wake();
        Some(event)
    }

    /// Asks for `event` to be delivered before the current instruction ends, after the events
    /// posted before it.
    pub fn signal(&mut self, event: Event) {
        self.signals.push_back(event);
    }

    /// The oldest event posted with [`Ctx::signal`] and not delivered yet.
    pub fn take_signal(&mut self) -> Option<Event> {
        self.signals.pop_front()
    }

    /// Drives the level of an interrupt source.
    pub fn irq(&mut self, source: Source, asserted: bool) {
        let levels = if asserted {
            self.sources | source.mask()
        } else {
            self.sources & !source.mask()
        };
        if levels != self.sources {
            self.sources = levels;
            self.interrupts_changed = true;
        }
    }

    /// The level of every interrupt source, bit `n` for source `n`.
    #[must_use]
    pub const fn sources(&self) -> u64 {
        self.sources
    }

    /// The interrupt controller tells that one of its registers was written.
    pub fn interrupts_changed(&mut self) {
        self.interrupts_changed = true;
    }

    /// Whether a source level or the controller changed since the last call.
    pub fn take_interrupts_changed(&mut self) -> bool {
        std::mem::take(&mut self.interrupts_changed)
    }

    /// Whether a device left something for the machine: a reset request, a clock selection write
    /// or a change of the interrupt lines.
    #[must_use]
    pub const fn is_flagged(&self) -> bool {
        self.clock_written || self.interrupts_changed || self.reset.is_some()
    }

    /// Asks for a reset. It takes place when the current instruction ends; the first request of an
    /// instruction wins.
    pub fn request_reset(&mut self, cause: ResetCause) {
        self.reset.get_or_insert(cause);
    }

    /// The reset asked for since the last call.
    pub fn take_reset(&mut self) -> Option<ResetCause> {
        self.reset.take()
    }

    /// Reports an operation the device cannot perform faithfully: it becomes a diagnostic that
    /// names the instruction, and `--strict-mmio` stops on it.
    pub fn unsupported(&mut self, what: Unsupported) {
        #[cfg(test)]
        self.refused.push_back(what);
        self.diag
            .record(Diagnostic::Unsupported { what, pc: self.pc });
    }

    /// The oldest refusal a device reported and no test took yet.
    #[cfg(test)]
    pub(crate) fn take_unsupported(&mut self) -> Option<Unsupported> {
        self.refused.pop_front()
    }
}

impl Default for Ctx {
    /// A context that stops on nothing: what a test of one device needs.
    fn default() -> Ctx {
        Ctx::new(false)
    }
}
```

- [ ] **Step 5: Run the suite**

Run: `cargo test --workspace > /tmp/t3.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t3.log`
Expected: `exit=0`, `596` (eight tests of the context, one of the reset causes).

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/soc-esp32c3/src/ctx.rs crates/soc-esp32c3/src/reset.rs crates/soc-esp32c3/src/lib.rs
/usr/bin/git commit -m "feat(soc): the context schedules wake-ups, carries interrupt sources and reset requests

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3/src/ctx.rs crates/soc-esp32c3/src/reset.rs crates/soc-esp32c3/src/lib.rs
```

---

## Task 4: Fault latches go through events; cache-error interrupt sources; a faulty fetch is not kept decoded

The last two side channels by concrete type (`latch_mmu_fault`, `latch_store`) become events delivered through one function, `Peripherals::deliver`, which every later device joins. `EXTMEM` drives its two interrupt sources (decision E33). A word fetched through a page no MMU entry maps is decoded again at each execution (decision E36).

**Files:**
- Modify: `crates/emu-core/src/bus.rs`, `crates/cpu-rv32/src/hart.rs`, `crates/soc-esp32c3/src/bus.rs`, `crates/soc-esp32c3/src/periph/mod.rs`, `crates/soc-esp32c3/src/periph/extmem.rs`, `crates/soc-esp32c3/src/machine.rs`, `docs/registers-accepted-without-effect.md`

**Interfaces:**
- Consumes: `Ctx::{signal, take_signal, irq}`, `Event::{MmuFault, FlashWindowStore}`, `Source::of` (Task 3); `Extmem::latch_mmu_fault(&mut self, vaddr: u32, content: u32)`, `Extmem::latch_store(&mut self, instruction_bus: bool)`; `SocBus::flash_offset(&self, window_offset: u32) -> Result<u32, UnbackedByte>`; `emu_core::Bus`; `cpu_rv32::dcache::DecodeCache::insert`.
- Produces: `emu_core::Bus::fetch_is_memory(&self, addr: u32, len: u32) -> bool` (default `true`); `Peripherals::deliver(&mut self, ctx: &mut Ctx, event: Event)` (crate-visible); `SocBus::deliver(&mut self, event: Event)` (crate-visible), and a private `settle` run after every register access; `Extmem::latch_mmu_fault(&mut self, ctx: &mut Ctx, vaddr: u32, content: u32)`, `Extmem::latch_store(&mut self, ctx: &mut Ctx, instruction_bus: bool)`; `EXTMEM.CACHE_ILG_INT_ENA` and `CORE0_ACS_CACHE_INT_ENA` are modelled (they leave the accepted table), sources 36 (`CACHE_IA_INT_MAP`) and 61 (`CACHE_CORE0_ACS_INT_MAP`) follow status AND enable.

- [ ] **Step 1: Write the failing tests**

`EXTMEM`: the application writes `CACHE_ILG_INT_ENA ← 0x23` and `CORE0_ACS_CACHE_INT_ENA ← 0x3F` and routes both sources to CPU line 25 (`esp_cache_err_int_init`, `IDF:esp_system/port/soc/esp32c3/cache_err_int.c`; IR §5):

```diff
--- a/crates/soc-esp32c3/src/periph/extmem.rs
+++ b/crates/soc-esp32c3/src/periph/extmem.rs
@@ -335,8 +364,6 @@ mod tests {
             ("CACHE_WRAP_AROUND_CTRL", 0),
             ("ICACHE_TAG_POWER_CTRL", 5),
             ("CACHE_MMU_POWER_CTRL", 5),
-            ("CACHE_ILG_INT_ENA", 0),
-            ("CORE0_ACS_CACHE_INT_ENA", 0),
         ] {
             assert_eq!(x.read(ctx, reg(name)), Some(reset), "{name}");
             assert!(x.write(ctx, reg(name), 0xF));
@@ -363,7 +390,7 @@ mod tests {
     fn an_invalid_mapping_is_latched_until_software_clears_it() {
         let ctx = &mut Ctx::default();
         let mut x = Extmem::new();
-        x.latch_mmu_fault(0x3C03_0010, 0x100);
+        x.latch_mmu_fault(ctx, 0x3C03_0010, 0x100);
         assert_eq!(
             x.read(ctx, reg("CACHE_ILG_INT_ST")),
             Some(1 << 5),
@@ -390,13 +417,13 @@ mod tests {
     fn a_store_to_a_window_is_latched_per_bus() {
         let ctx = &mut Ctx::default();
         let mut x = Extmem::new();
-        x.latch_store(false);
+        x.latch_store(ctx, false);
         assert_eq!(
             x.read(ctx, reg("CORE0_ACS_CACHE_INT_ST")),
             Some(1 << 5),
             "DBUS_WR_ICACHE_ST"
         );
-        x.latch_store(true);
+        x.latch_store(ctx, true);
         assert_eq!(
             x.read(ctx, reg("CORE0_ACS_CACHE_INT_ST")),
             Some((1 << 5) | (1 << 1)),
```

The machine: a program jumps into the instruction window, whose MMU entries are invalid at reset; the trap handler jumps back. Each fetch must reach the bus:

```diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -667,4 +667,35 @@ mod tests {
             (1, 2)
         );
     }
+
+    /// Where the tests that take traps put their vector table: a page boundary, which `lui` can
+    /// load (`mtvec.BASE` only has to be aligned to 256 bytes).
+    const VECTORS: u32 = ROM_BASE + 0x1000;
+    const MTVEC: u16 = 0x305;
+
+    #[test]
+    fn an_instruction_fetched_through_an_unmapped_flash_page_is_fetched_again_each_time() {
+        // Carried over from SP1b-2: decoded once and kept, such an instruction would be reported
+        // and latched once however often it runs. The handler jumps back into the window.
+        let window = crate::memmap::FLASH_IBUS_BASE;
+        let mut body = vec![
+            lui(T0, VECTORS),
+            cpu_rv32::asm::csrrw(0, MTVEC, T0),
+            lui(T1, window),
+            cpu_rv32::asm::jalr(0, T1, 0),
+        ];
+        body.resize(((VECTORS - ROM_BASE) / 4) as usize, HANG);
+        // Exceptions enter at the base of the table.
+        body.push(cpu_rv32::asm::jalr(0, T1, 0));
+        let mut m = machine(&body);
+        assert_eq!(m.run_until(Cycles(200)), StopReason::CycleBudget);
+        let faults = m
+            .most_repeated_diagnostics(8)
+            .into_iter()
+            .find(|(d, _)| matches!(d, Diagnostic::FlashMapping { vaddr, .. } if *vaddr == window));
+        let Some((_, count)) = faults else {
+            panic!("the fetch through the invalid entry was not reported more than once");
+        };
+        assert!(count > 5, "one report per execution: {count}");
+    }
 }
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p soc-esp32c3 --lib > /tmp/t4.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, compile errors in the tests of `extmem.rs` (`latch_mmu_fault` takes 2 arguments). With those tests set aside, `an_instruction_fetched_through_an_unmapped_flash_page_is_fetched_again_each_time` fails on its last assertion: the fault is reported once.

- [ ] **Step 3: `EXTMEM` latches through the context and drives its sources**

```diff
--- a/crates/soc-esp32c3/src/periph/extmem.rs
+++ b/crates/soc-esp32c3/src/periph/extmem.rs
@@ -5,7 +5,7 @@
 //! (decision D4). The operations software waits for (invalidate, preload, autoload, freeze)
 //! therefore complete at once, and the registers below only keep what software reads back.
 use super::{Accepted, EXTMEM_BASE, Storage};
-use crate::ctx::Ctx;
+use crate::ctx::{Ctx, Source};
 use emu_core::Peripheral;
 use svd_regs::esp32c3::addr;
 
@@ -20,8 +20,10 @@ const ICACHE_PRELOAD_CTRL: u32 = reg("ICACHE_PRELOAD_CTRL");
 const ICACHE_AUTOLOAD_CTRL: u32 = reg("ICACHE_AUTOLOAD_CTRL");
 const ICACHE_FREEZE: u32 = reg("ICACHE_FREEZE");
 const CACHE_STATE: u32 = reg("CACHE_STATE");
+const CACHE_ILG_INT_ENA: u32 = reg("CACHE_ILG_INT_ENA");
 const CACHE_ILG_INT_CLR: u32 = reg("CACHE_ILG_INT_CLR");
 const CACHE_ILG_INT_ST: u32 = reg("CACHE_ILG_INT_ST");
+const CORE0_ACS_CACHE_INT_ENA: u32 = reg("CORE0_ACS_CACHE_INT_ENA");
 const CORE0_ACS_CACHE_INT_CLR: u32 = reg("CORE0_ACS_CACHE_INT_CLR");
 const CORE0_ACS_CACHE_INT_ST: u32 = reg("CORE0_ACS_CACHE_INT_ST");
 const CACHE_MMU_FAULT_CONTENT: u32 = reg("CACHE_MMU_FAULT_CONTENT");
@@ -52,9 +54,18 @@ const FAULT_CODE_CPU: u32 = 1 << 10;
 const IBUS_WRITE: u32 = 1 << 1;
 const DBUS_WRITE: u32 = 1 << 5;
 
+/// The two cache error interrupts: the status bits a line is enabled for drive it (CM §2: "gated
+/// by `ILG_INT_ENA` for the interrupt line (source 36)", and source 61 for the access errors). The
+/// application enables them with `0x23` and `0x3F` and routes both sources to CPU line 25
+/// (`esp_cache_err_int_init`, `IDF:esp_system/port/soc/esp32c3/cache_err_int.c`; report IR §3.2).
+const ILLEGAL_SOURCE: Source = Source::of("CACHE_IA_INT_MAP");
+const ACCESS_SOURCE: Source = Source::of("CACHE_CORE0_ACS_INT_MAP");
+/// The bits of the two enable registers (CM §2).
+const ILLEGAL_BITS: u32 = 0x1A3;
+const ACCESS_BITS: u32 = 0x3F;
+
 const WHY_OPERATION: &str = "cache operation: completes at once, there are no cache lines";
-const WHY_FAULT_IRQ: &str = "cache error interrupt: the status is latched, no interrupt is raised";
-const ACCEPTED: [Accepted; 14] = [
+const ACCEPTED: [Accepted; 12] = [
     Accepted::svd(
         "EXTMEM",
         "ICACHE_CTRL",
@@ -73,8 +84,6 @@ const ACCEPTED: [Accepted; 14] = [
     Accepted::svd("EXTMEM", "ICACHE_SYNC_SIZE", WHY_OPERATION, "SP1b-4"),
     Accepted::svd("EXTMEM", "ICACHE_PRELOAD_CTRL", WHY_OPERATION, "SP1b-4"),
     Accepted::svd("EXTMEM", "ICACHE_AUTOLOAD_CTRL", WHY_OPERATION, "SP1b-4"),
-    Accepted::svd("EXTMEM", "CACHE_ILG_INT_ENA", WHY_FAULT_IRQ, "SP1b-3"),
-    Accepted::svd("EXTMEM", "CORE0_ACS_CACHE_INT_ENA", WHY_FAULT_IRQ, "SP1b-3"),
     Accepted::svd(
         "EXTMEM",
         "CACHE_WRAP_AROUND_CTRL",
@@ -97,7 +106,9 @@ pub struct Extmem {
     autoload: u32,
     freeze: u32,
     illegal_status: u32,
+    illegal_enable: u32,
     access_status: u32,
+    access_enable: u32,
     fault_content: u32,
     fault_vaddr: u32,
     generation: u64,
@@ -112,7 +123,9 @@ impl Extmem {
             autoload: 0,
             freeze: 0,
             illegal_status: 0,
+            illegal_enable: 0,
             access_status: 0,
+            access_enable: 0,
             fault_content: 0,
             fault_vaddr: 0,
             generation: 0,
@@ -126,19 +139,30 @@ impl Extmem {
     }
 
     /// Records an access through an invalid MMU entry (`CACHE_ILG_INT_ST`, `CACHE_MMU_FAULT_*`; CM §6.1).
-    pub fn latch_mmu_fault(&mut self, vaddr: u32, content: u32) {
+    pub(crate) fn latch_mmu_fault(&mut self, ctx: &mut Ctx, vaddr: u32, content: u32) {
         self.illegal_status |= MMU_ENTRY_FAULT;
         self.fault_content = (content & FAULT_CONTENT_MASK) | FAULT_CODE_CPU;
         self.fault_vaddr = vaddr;
+        self.drive(ctx);
     }
 
     /// Records a store to a flash window (`CORE0_ACS_CACHE_INT_ST`; CM §6.1).
-    pub fn latch_store(&mut self, instruction_bus: bool) {
+    pub(crate) fn latch_store(&mut self, ctx: &mut Ctx, instruction_bus: bool) {
         self.access_status |= if instruction_bus {
             IBUS_WRITE
         } else {
             DBUS_WRITE
         };
+        self.drive(ctx);
+    }
+
+    /// Each cache error interrupt is a level: a latched status bit whose enable bit is set.
+    fn drive(&self, ctx: &mut Ctx) {
+        ctx.irq(
+            ILLEGAL_SOURCE,
+            self.illegal_status & self.illegal_enable != 0,
+        );
+        ctx.irq(ACCESS_SOURCE, self.access_status & self.access_enable != 0);
     }
 }
 
@@ -164,7 +188,9 @@ impl Peripheral<Ctx> for Extmem {
             }
             CACHE_STATE => CACHE_IDLE,
             CACHE_ILG_INT_ST => self.illegal_status,
+            CACHE_ILG_INT_ENA => self.illegal_enable,
             CORE0_ACS_CACHE_INT_ST => self.access_status,
+            CORE0_ACS_CACHE_INT_ENA => self.access_enable,
             CACHE_MMU_FAULT_CONTENT => self.fault_content,
             CACHE_MMU_FAULT_VADDR => self.fault_vaddr,
             // Write-only registers read 0.
@@ -173,14 +199,16 @@ impl Peripheral<Ctx> for Extmem {
         })
     }
 
-    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
+    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
         match offset {
             // The invalidation has nothing to invalidate; `SYNC_DONE` already reads set.
             ICACHE_SYNC_CTRL => {}
             ICACHE_PRELOAD_CTRL => self.preload_order = value & PRELOAD_ORDER,
             ICACHE_AUTOLOAD_CTRL => self.autoload = value & AUTOLOAD_CONFIG,
             ICACHE_FREEZE => self.freeze = value & (FREEZE_ENA | FREEZE_MODE),
+            CACHE_ILG_INT_ENA => self.illegal_enable = value & ILLEGAL_BITS,
             CACHE_ILG_INT_CLR => self.illegal_status &= !value,
+            CORE0_ACS_CACHE_INT_ENA => self.access_enable = value & ACCESS_BITS,
             CORE0_ACS_CACHE_INT_CLR => self.access_status &= !value,
             ICACHE_CTRL | ICACHE_CTRL1 => {
                 self.generation += 1;
@@ -194,6 +222,7 @@ impl Peripheral<Ctx> for Extmem {
             | CACHE_MMU_FAULT_VADDR => return false,
             _ => return self.registers.write(offset, value),
         }
+        self.drive(ctx);
         true
     }
 }
```

- [ ] **Step 4: One delivery function, and the bus settles after every access**

```diff
--- a/crates/soc-esp32c3/src/periph/mod.rs
+++ b/crates/soc-esp32c3/src/periph/mod.rs
@@ -1,6 +1,6 @@
 //! The modelled peripherals of the ESP32-C3 and their place in the address space.
 use crate::config::SocConfig;
-use crate::ctx::Ctx;
+use crate::ctx::{Ctx, Event};
 use crate::flash::FlashDevice;
 use emu_core::Peripheral;
 
@@ -395,6 +395,17 @@ impl Peripherals {
         }
     }
 
+    /// Calls the device an event is for: a wake-up whose time has come, or what another part of the
+    /// machine has to tell it (ADR 0003).
+    pub(crate) fn deliver(&mut self, ctx: &mut Ctx, event: Event) {
+        match event {
+            Event::MmuFault { vaddr, content } => self.extmem.latch_mmu_fault(ctx, vaddr, content),
+            Event::FlashWindowStore { instruction_bus } => {
+                self.extmem.latch_store(ctx, instruction_bus);
+            }
+        }
+    }
+
     /// The device that owns the register at `addr`, with its base address.
     pub(crate) fn device(&mut self, addr: u32) -> Option<(&mut dyn Peripheral<Ctx>, u32)> {
         let within = |block: (u32, u32)| (block.0..block.1).contains(&addr);
```

```diff
--- a/crates/soc-esp32c3/src/bus.rs
+++ b/crates/soc-esp32c3/src/bus.rs
@@ -1,6 +1,6 @@
 //! The ESP32-C3 address space. Memory map: `docs/research/esp32c3-boot-chain.md`, §A.1.
 use crate::config::SocConfig;
-use crate::ctx::Ctx;
+use crate::ctx::{Ctx, Event};
 use crate::diag::{Diagnostic, Direction, MappingFault};
 use crate::flash::FlashDevice;
 use crate::memmap::{
@@ -111,6 +111,20 @@ impl SocBus {
         }
     }
 
+    /// Calls the device `event` is for, then every device the call had something to tell.
+    pub(crate) fn deliver(&mut self, event: Event) {
+        self.periph.deliver(&mut self.ctx, event);
+        self.settle();
+    }
+
+    /// Delivers what the devices posted for each other during a register access or a call: such
+    /// an event reaches its device before the instruction ends.
+    fn settle(&mut self) {
+        while let Some(event) = self.ctx.take_signal() {
+            self.periph.deliver(&mut self.ctx, event);
+        }
+    }
+
     /// Tells the bus which instruction is running, so diagnostics can name it.
     pub fn set_pc(&mut self, pc: u32) {
         self.ctx.set_pc(pc);
@@ -189,7 +203,10 @@ impl SocBus {
                 Err(unbacked) if reported == Some(unbacked) => {}
                 Err(unbacked) => {
                     // The fault registers and the diagnostic name the address of the access.
-                    self.periph.extmem.latch_mmu_fault(vaddr, unbacked.content);
+                    self.deliver(Event::MmuFault {
+                        vaddr,
+                        content: unbacked.content,
+                    });
                     self.ctx.diag.record(Diagnostic::FlashMapping {
                         vaddr,
                         entry: unbacked.entry,
@@ -216,6 +233,7 @@ impl SocBus {
             .periph
             .device(reg_addr)
             .and_then(|(device, base)| device.read(&mut self.ctx, reg_addr - base));
+        self.settle();
         let word = match modelled {
             Some(word) => word,
             None => self.unmodeled(reg_addr, Direction::Read, 0),
@@ -244,6 +262,7 @@ impl SocBus {
             .periph
             .device(reg_addr)
             .is_some_and(|(device, base)| device.write(&mut self.ctx, reg_addr - base, word));
+        self.settle();
         if !handled {
             self.unmodeled(reg_addr, Direction::Write, word);
         }
@@ -286,7 +305,7 @@ impl Bus for SocBus {
             Some(Region::Flash {
                 instruction_bus, ..
             }) => {
-                self.periph.extmem.latch_store(instruction_bus);
+                self.deliver(Event::FlashWindowStore { instruction_bus });
                 Err(BusError::Unmapped {
                     addr,
                     kind: AccessKind::Store,
@@ -300,6 +319,18 @@ impl Bus for SocBus {
         }
     }
 
+    fn fetch_is_memory(&self, addr: u32, len: u32) -> bool {
+        match locate(addr, len) {
+            // Decision D23 serves 0 for a flash window byte no flash byte backs: that is no
+            // content to keep, and each such fetch is to be latched and reported.
+            Some(Region::Flash { offset, .. }) => {
+                self.flash_offset(offset).is_ok()
+                    && self.flash_offset(offset + len.saturating_sub(1)).is_ok()
+            }
+            _ => true,
+        }
+    }
+
     fn page_version(&self, addr: u32) -> u64 {
         // The SRAM keeps one version per physical page: code decoded at an instruction-bus address is
         // invalidated by a store through the data bus, because both resolve to the same cell.
```

- [ ] **Step 5: A fetch that is not memory content is not kept decoded**

```diff
--- a/crates/emu-core/src/bus.rs
+++ b/crates/emu-core/src/bus.rs
@@ -71,6 +71,15 @@ pub trait Bus {
     /// Decode caches compare it to detect self-modifying code and DMA writes.
     fn page_version(&self, addr: u32) -> u64;
 
+    /// Whether the `len` bytes a fetch at `addr` returned are memory content, which stays the same
+    /// until [`Bus::page_version`] changes. A `SoC` answers `false` when the fetch was served
+    /// without memory behind it (a flash window page no MMU entry maps): the instruction is then
+    /// decoded again at every execution, so the bus sees, and reports, each faulty fetch.
+    fn fetch_is_memory(&self, addr: u32, len: u32) -> bool {
+        let _ = (addr, len);
+        true
+    }
+
     /// Timing-only hook called when an instruction is served from a decode cache:
     /// the `SoC` charges what the fetch would have cost (e.g. flash-cache hit or miss) without moving data.
     fn fetch_stall(&mut self, addr: u32, len: u32) -> u32 {
```

```diff
--- a/crates/cpu-rv32/src/hart.rs
+++ b/crates/cpu-rv32/src/hart.rs
@@ -136,7 +136,9 @@ impl Hart {
         }
         let (raw, stall) = self.fetch(bus)?;
         let decoded = decode(raw);
-        self.dcache.insert(self.pc, decoded, &*bus);
+        if bus.fetch_is_memory(self.pc, u32::from(decoded.len)) {
+            self.dcache.insert(self.pc, decoded, &*bus);
+        }
         Ok((decoded, stall))
     }
 
```

- [ ] **Step 6: Run the suite, regenerate the table**

Run: `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect > /tmp/bless.log 2>&1; echo "exit=$?"; /usr/bin/git diff --stat docs/registers-accepted-without-effect.md`
Expected: `exit=0`; two rows leave the table (`EXTMEM.CACHE_ILG_INT_ENA`, `EXTMEM.CORE0_ACS_CACHE_INT_ENA`).

Run: `cargo test --workspace > /tmp/t4.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t4.log`
Expected: `exit=0`, `597`.

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/emu-core/src/bus.rs crates/cpu-rv32/src/hart.rs crates/soc-esp32c3/src docs/registers-accepted-without-effect.md
/usr/bin/git commit -m "refactor(soc): fault latches are events; EXTMEM drives its interrupt sources

A word fetched through an unmapped flash page is no longer kept decoded.

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/emu-core/src/bus.rs crates/cpu-rv32/src/hart.rs crates/soc-esp32c3/src docs/registers-accepted-without-effect.md
```

---

## Task 5: The interrupt controller

`INTERRUPT_CORE0` stops being storage: it turns the levels of the 62 sources into the one CPU line to take (decision E3). Nothing calls it yet; Task 8 does.

**Files:**
- Modify: `crates/soc-esp32c3/src/periph/intmatrix.rs` (rewritten), `crates/soc-esp32c3/src/periph/mod.rs`, `docs/registers-accepted-without-effect.md`

**Interfaces:**
- Consumes: `Ctx::{sources, irq, interrupts_changed}`, `Source::of` (Task 3); `periph::{Accepted, INTMATRIX_BASE}`; `svd_regs::esp32c3::addr`.
- Produces: `periph::half_of(value: u64, hi: bool) -> u32` and `periph::replace_half(value: u64, hi: bool, half: u32, hi_mask: u32) -> u64`; on `IntMatrix`: `resolve(&mut self, sources: u64) -> Option<u32>` (the line the CPU must take given the level of every source: pending line of highest priority, lowest line among equals), `claim(&mut self, line: u32)` (the CPU took the line: an edge latched on it may now be acknowledged), `IntMatrix::new()` unchanged; `intmatrix::{SOURCE_COUNT, CPU_LINE_COUNT}`. Registers modelled: the 62 `<SRC>_MAP`, `INTR_STATUS_REG_0/1` (raw source levels), `CPU_INT_ENABLE`, `CPU_INT_TYPE`, `CPU_INT_CLEAR`, `CPU_INT_EIP_STATUS`, `CPU_INT_PRI_0..31`, `CPU_INT_THRESH`; a write to any of them calls `ctx.interrupts_changed()`. Only `CLOCK_GATE` stays accepted without effect (heir SP8).

- [ ] **Step 1: Write the failing tests**

The values are those of the reference application (IR §1.5, §3.2): tick on line 4 and software interrupt on line 3 at priority 1, threshold 1, critical sections at threshold 4... Replace the test module of `crates/soc-esp32c3/src/periph/intmatrix.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::Source;
    use emu_core::Peripheral;

    const TICK: Source = Source::of("SYSTIMER_TARGET0_INT_MAP");
    const SOFTWARE: Source = Source::of("CPU_INTR_FROM_CPU_0_MAP");
    const WATCHDOG: Source = Source::of("TG1_WDT_INT_MAP");
    const TICK_MAP: u32 = reg("SYSTIMER_TARGET0_INT_MAP");
    const SOFTWARE_MAP: u32 = reg("CPU_INTR_FROM_CPU_0_MAP");
    const WATCHDOG_MAP: u32 = reg("TG1_WDT_INT_MAP");

    fn priority(line: u32) -> u32 {
        CPU_INT_PRI_0 + 4 * line
    }

    /// The controller as the reference application leaves it (IR §3.2): the software interrupt on
    /// line 3 and the tick on line 4 at priority 1, the interrupt watchdog on line 24 at priority 4,
    /// threshold 1.
    fn application_setup(ctx: &mut Ctx) -> IntMatrix {
        let mut m = IntMatrix::new();
        for (map, line, level) in [
            (SOFTWARE_MAP, 3, 1),
            (TICK_MAP, 4, 1),
            (WATCHDOG_MAP, 24, 4),
        ] {
            assert!(m.write(ctx, map, line));
            assert!(m.write(ctx, CPU_INT_ENABLE, m.enable | (1 << line)));
            assert!(m.write(ctx, priority(line), level));
        }
        assert!(m.write(ctx, CPU_INT_THRESH, 1));
        m
    }

    #[test]
    fn reset_state_gates_the_clock_on_and_routes_nothing() {
        let ctx = &mut Ctx::default();
        let mut m = IntMatrix::new();
        assert_eq!(m.read(ctx, CLOCK_GATE), Some(1));
        for offset in [
            reg("UART_INTR_MAP"),
            CPU_INT_ENABLE,
            CPU_INT_TYPE,
            CPU_INT_CLEAR,
            CPU_INT_PRI_0,
            priority(5),
            CPU_INT_THRESH,
        ] {
            assert_eq!(m.read(ctx, offset), Some(0), "{offset:#x}");
        }
        ctx.irq(TICK, true);
        assert_eq!(
            m.resolve(ctx.sources()),
            None,
            "an unrouted source reaches no line"
        );
    }

    #[test]
    fn rom_setup_sequence_is_remembered() {
        // BC §B.3: the ROM routes UART0 (source 21) to line 5 with priority 1.
        let ctx = &mut Ctx::default();
        let mut m = IntMatrix::new();
        assert!(m.write(ctx, reg("UART_INTR_MAP"), 5));
        assert!(m.write(ctx, priority(5), 1));
        assert!(m.write(ctx, CPU_INT_THRESH, 1));
        assert!(m.write(ctx, CPU_INT_ENABLE, 1 << 5));
        assert_eq!(
            (
                m.read(ctx, reg("UART_INTR_MAP")),
                m.read(ctx, priority(5)),
                m.read(ctx, CPU_INT_THRESH),
                m.read(ctx, CPU_INT_ENABLE)
            ),
            (Some(5), Some(1), Some(1), Some(1 << 5))
        );
        assert!(
            ctx.take_interrupts_changed(),
            "a controller write is a change"
        );
    }

    #[test]
    fn fields_keep_only_their_defined_bits() {
        let ctx = &mut Ctx::default();
        let mut m = IntMatrix::new();
        m.write(ctx, reg("CACHE_CORE0_ACS_INT_MAP"), 0xFFFF_FFFF);
        m.write(ctx, priority(31), 0xFFFF_FFFF);
        m.write(ctx, CPU_INT_THRESH, 0xFFFF_FFFF);
        m.write(ctx, CPU_INT_ENABLE, 0xFFFF_FFFF);
        m.write(ctx, CPU_INT_TYPE, 0xFFFF_FFFF);
        assert_eq!(
            (
                m.read(ctx, reg("CACHE_CORE0_ACS_INT_MAP")),
                m.read(ctx, priority(31)),
                m.read(ctx, CPU_INT_THRESH),
                m.read(ctx, CPU_INT_ENABLE),
                m.read(ctx, CPU_INT_TYPE)
            ),
            (Some(0x1F), Some(0xF), Some(0xF), Some(!1), Some(!1)),
            "line 0 does not exist"
        );
    }

    #[test]
    fn a_level_line_follows_its_source() {
        // IR §3.3: the software interrupt is taken on line 3, then its handler clears the source.
        let ctx = &mut Ctx::default();
        let mut m = application_setup(ctx);
        assert_eq!(m.resolve(ctx.sources()), None);
        ctx.irq(SOFTWARE, true);
        assert_eq!(m.resolve(ctx.sources()), Some(3));
        assert_eq!(m.read(ctx, CPU_INT_EIP_STATUS), Some(1 << 3));
        assert_eq!(
            m.resolve(ctx.sources()),
            Some(3),
            "taking it changes nothing here"
        );
        ctx.irq(SOFTWARE, false);
        assert_eq!(m.resolve(ctx.sources()), None);
        assert_eq!(m.read(ctx, CPU_INT_EIP_STATUS), Some(0));
    }

    #[test]
    fn the_threshold_masks_lower_priorities_and_priority_zero_never_passes() {
        let ctx = &mut Ctx::default();
        let mut m = application_setup(ctx);
        ctx.irq(TICK, true);
        // The handler raises the threshold to the priority of its line plus one (IR §2.3 step 4).
        m.write(ctx, CPU_INT_THRESH, 2);
        assert_eq!(m.resolve(ctx.sources()), None);
        // "Higher or equal": a threshold equal to the priority lets the line through.
        m.write(ctx, CPU_INT_THRESH, 1);
        assert_eq!(m.resolve(ctx.sources()), Some(4));
        // IR §1.5: `esp_intr_alloc` enables a line before it gives it a priority; priority 0 is
        // masked even with the threshold at 0.
        m.write(ctx, priority(4), 0);
        m.write(ctx, CPU_INT_THRESH, 0);
        assert_eq!(m.resolve(ctx.sources()), None);
    }

    #[test]
    fn a_disabled_line_is_not_pending_and_the_raw_status_ignores_routing_and_masking() {
        let ctx = &mut Ctx::default();
        let mut m = application_setup(ctx);
        ctx.irq(TICK, true);
        ctx.irq(SOFTWARE, true);
        m.write(ctx, CPU_INT_ENABLE, 1 << 3);
        assert_eq!(m.read(ctx, CPU_INT_EIP_STATUS), Some(1 << 3));
        m.write(ctx, CPU_INT_THRESH, 15);
        assert_eq!(m.read(ctx, CPU_INT_EIP_STATUS), Some(0));
        // Sources 37 and 50: bits 5 and 18 of the second status register.
        assert_eq!(m.read(ctx, STATUS_0), Some(0));
        assert_eq!(m.read(ctx, STATUS_1), Some((1 << 5) | (1 << 18)));
        assert!(
            !m.write(ctx, STATUS_1, 0),
            "the status registers are read-only"
        );
        assert!(!m.write(ctx, CPU_INT_EIP_STATUS, 0));
    }

    #[test]
    fn the_highest_priority_wins_and_the_lowest_line_among_equals() {
        let ctx = &mut Ctx::default();
        let mut m = application_setup(ctx);
        ctx.irq(TICK, true);
        ctx.irq(SOFTWARE, true);
        assert_eq!(
            m.resolve(ctx.sources()),
            Some(3),
            "priority 1 on lines 3 and 4"
        );
        ctx.irq(WATCHDOG, true);
        assert_eq!(
            m.resolve(ctx.sources()),
            Some(24),
            "priority 4 beats priority 1"
        );
        // The critical-section threshold of ESP-IDF, 4, leaves the panic lines deliverable (IR §1.4).
        m.write(ctx, CPU_INT_THRESH, 4);
        assert_eq!(m.resolve(ctx.sources()), Some(24));
        assert_eq!(m.read(ctx, CPU_INT_EIP_STATUS), Some(1 << 24));
    }

    #[test]
    fn two_sources_on_one_line_are_ored() {
        // IR §3.2: the application routes both cache-error sources to line 25.
        let ctx = &mut Ctx::default();
        let mut m = IntMatrix::new();
        let (illegal, access) = (
            Source::of("CACHE_IA_INT_MAP"),
            Source::of("CACHE_CORE0_ACS_INT_MAP"),
        );
        m.write(ctx, reg("CACHE_IA_INT_MAP"), 25);
        m.write(ctx, reg("CACHE_CORE0_ACS_INT_MAP"), 25);
        m.write(ctx, CPU_INT_ENABLE, 1 << 25);
        m.write(ctx, priority(25), 4);
        ctx.irq(access, true);
        assert_eq!(m.resolve(ctx.sources()), Some(25));
        ctx.irq(illegal, true);
        ctx.irq(access, false);
        assert_eq!(
            m.resolve(ctx.sources()),
            Some(25),
            "the other source holds the line"
        );
        ctx.irq(illegal, false);
        assert_eq!(m.resolve(ctx.sources()), None);
    }

    #[test]
    fn a_source_mapped_to_line_zero_goes_nowhere() {
        let ctx = &mut Ctx::default();
        let mut m = IntMatrix::new();
        m.write(ctx, CPU_INT_ENABLE, 0xFFFF_FFFF);
        m.write(ctx, CPU_INT_PRI_0, 15);
        ctx.irq(TICK, true);
        assert_eq!(m.resolve(ctx.sources()), None);
        assert_eq!(m.read(ctx, CPU_INT_EIP_STATUS), Some(0));
    }

    #[test]
    fn an_edge_line_latches_until_it_is_taken_and_acknowledged_by_a_toggle() {
        let ctx = &mut Ctx::default();
        let mut m = application_setup(ctx);
        m.write(ctx, CPU_INT_TYPE, 1 << 4);
        ctx.irq(TICK, true);
        assert_eq!(m.resolve(ctx.sources()), Some(4));
        ctx.irq(TICK, false);
        assert_eq!(
            m.resolve(ctx.sources()),
            Some(4),
            "the latch outlives the source"
        );
        // Not taken yet: the toggle does not clear it (TRM §1.5.2 property 5).
        m.write(ctx, CPU_INT_CLEAR, 1 << 4);
        m.write(ctx, CPU_INT_CLEAR, 0);
        assert_eq!(m.resolve(ctx.sources()), Some(4));
        m.claim(4);
        m.write(ctx, CPU_INT_CLEAR, 1 << 4);
        assert_eq!(m.read(ctx, CPU_INT_CLEAR), Some(1 << 4));
        assert_eq!(
            m.resolve(ctx.sources()),
            Some(4),
            "setting the bit is half of the toggle"
        );
        m.write(ctx, CPU_INT_CLEAR, 0);
        assert_eq!(m.resolve(ctx.sources()), None);
        // A source that stayed high is no new edge.
        ctx.irq(TICK, true);
        assert_eq!(m.resolve(ctx.sources()), Some(4));
        m.claim(4);
        m.write(ctx, CPU_INT_CLEAR, 1 << 4);
        m.write(ctx, CPU_INT_CLEAR, 0);
        assert_eq!(m.resolve(ctx.sources()), None, "still high: no rising edge");
    }

    #[test]
    fn an_edge_that_was_never_taken_is_flushed_once_its_line_is_disabled() {
        let ctx = &mut Ctx::default();
        let mut m = application_setup(ctx);
        m.write(ctx, CPU_INT_TYPE, 1 << 4);
        ctx.irq(TICK, true);
        assert_eq!(m.resolve(ctx.sources()), Some(4));
        ctx.irq(TICK, false);
        assert_eq!(m.resolve(ctx.sources()), Some(4));
        let enabled = m.read(ctx, CPU_INT_ENABLE).unwrap();
        m.write(ctx, CPU_INT_ENABLE, enabled & !(1 << 4));
        m.write(ctx, CPU_INT_CLEAR, 1 << 4);
        m.write(ctx, CPU_INT_CLEAR, 0);
        m.write(ctx, CPU_INT_ENABLE, enabled);
        assert_eq!(m.resolve(ctx.sources()), None);
    }

    #[test]
    fn an_edge_on_a_disabled_line_is_not_latched() {
        // Pins the ASSUMPTION of `update`.
        let ctx = &mut Ctx::default();
        let mut m = application_setup(ctx);
        m.write(ctx, CPU_INT_TYPE, 1 << 4);
        let enabled = m.read(ctx, CPU_INT_ENABLE).unwrap();
        m.write(ctx, CPU_INT_ENABLE, enabled & !(1 << 4));
        ctx.irq(TICK, true);
        assert_eq!(m.resolve(ctx.sources()), None);
        m.write(ctx, CPU_INT_ENABLE, enabled);
        assert_eq!(
            m.resolve(ctx.sources()),
            None,
            "the edge passed while the line was disabled"
        );
    }

    #[test]
    fn everything_else_is_unmodeled() {
        let ctx = &mut Ctx::default();
        let mut m = IntMatrix::new();
        assert_eq!(m.read(ctx, reg("INTERRUPT_REG_DATE")), None);
        assert!(!m.write(ctx, reg("INTERRUPT_REG_DATE"), 0));
    }
}
```

And in the test module of `crates/soc-esp32c3/src/periph/mod.rs`:

```diff
--- a/crates/soc-esp32c3/src/periph/mod.rs
+++ b/crates/soc-esp32c3/src/periph/mod.rs
@@ -771,4 +793,19 @@ mod tests {
         assert_eq!(base_of(&mut p, 0x6000_8FFC), Some(EFUSE_BASE));
         assert_eq!(base_of(&mut p, 0x6000_9000), Some(IO_MUX_BASE));
     }
+
+    #[test]
+    fn a_wide_value_is_read_and_written_as_two_halves() {
+        assert_eq!(half_of(0x0012_3456_89AB_CDEF, false), 0x89AB_CDEF);
+        assert_eq!(half_of(0x0012_3456_89AB_CDEF, true), 0x0012_3456);
+        // A 52-bit pair: the high register has 20 bits.
+        assert_eq!(
+            replace_half(0, true, 0xFFFF_FFFF, 0x000F_FFFF),
+            0x000F_FFFF_0000_0000
+        );
+        assert_eq!(
+            replace_half(0x000F_FFFF_0000_0000, false, 0x1234_5678, 0x000F_FFFF),
+            0x000F_FFFF_1234_5678
+        );
+    }
 }
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p soc-esp32c3 --lib > /tmp/t5.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, compile errors: no method `resolve`, no method `claim`, `cannot find function 'half_of'`.

- [ ] **Step 3: The two halves of a wide register**

```diff
--- a/crates/soc-esp32c3/src/periph/mod.rs
+++ b/crates/soc-esp32c3/src/periph/mod.rs
@@ -169,6 +169,28 @@ impl Storage {
     }
 }
 
+/// One 32-bit half of a wide value: bits 63:32 when `hi`, bits 31:0 otherwise. Registers expose
+/// 48- to 62-bit values as such pairs.
+#[must_use]
+pub const fn half_of(value: u64, hi: bool) -> u32 {
+    // Lossless: each operand is reduced to 32 bits before the cast.
+    if hi {
+        (value >> 32) as u32
+    } else {
+        (value & 0xFFFF_FFFF) as u32
+    }
+}
+
+/// `value` with one half replaced by `half`; `hi_mask` keeps the bits the high register has.
+#[must_use]
+pub fn replace_half(value: u64, hi: bool, half: u32, hi_mask: u32) -> u64 {
+    if hi {
+        (value & 0xFFFF_FFFF) | (u64::from(half & hi_mask) << 32)
+    } else {
+        (value & !0xFFFF_FFFF) | u64::from(half)
+    }
+}
+
 /// A block made only of registers accepted without effect.
 #[derive(Clone, Debug)]
 pub struct AcceptedBlock {
```

- [ ] **Step 4: The controller**

Replace everything above the test module of `crates/soc-esp32c3/src/periph/intmatrix.rs` with:

````rust
//! `INTERRUPT_CORE0`: the interrupt controller of the ESP32-C3. It routes the 62 peripheral
//! interrupt sources to the 31 CPU interrupt lines and decides which line the CPU takes.
//! Sources: TRM §1.5 and §8.4, ESP-IDF `interrupt_core0_reg.h`; report IR §1.
//!
//! The rule (IR §1.2, three sources agree):
//!
//! ```text
//! level(n)   = OR of the level of every source s with MAP[s] == n          (n = 1..31)
//! raw(n)     = TYPE[n] ? edge_latch(n) : level(n)
//! pending(n) = ENABLE[n] && raw(n) && PRI[n] != 0 && PRI[n] >= THRESH       (CPU_INT_EIP_STATUS)
//! selected   = the pending line of highest priority, the lowest line number among equals
//! ```
//!
//! `mie` and `mip` play no part: "the ESP32-C3 implements its interrupt mechanism using an
//! interrupt controller instead of RISC-V Privileged ISA specification" (TRM §8.3.2).
use super::{Accepted, INTMATRIX_BASE, half_of};
use crate::ctx::Ctx;
use emu_core::Peripheral;
use svd_regs::esp32c3::addr;

/// `<SRC>_MAP` registers: `MAC_INTR_MAP` (source 0) to `CACHE_CORE0_ACS_INT_MAP` (source 61).
pub const SOURCE_COUNT: usize = SOURCES as usize;
const SOURCES: u32 = 62;
/// `CPU_INT_PRI_0..31`; line 0 does not exist (vector slot 0 is the exception entry) but its
/// register is in the SVD and in ESP-IDF's header.
pub const CPU_LINE_COUNT: usize = LINES as usize;
const LINES: u32 = 32;

const fn reg(name: &str) -> u32 {
    addr("INTERRUPT_CORE0", name) - INTMATRIX_BASE
}

const MAP_0: u32 = reg("MAC_INTR_MAP");
/// Raw status of sources 0..31 and 32..61, whatever their routing and masking (IR §1.1).
const STATUS_0: u32 = reg("INTR_STATUS_REG_0");
const STATUS_1: u32 = reg("INTR_STATUS_REG_1");
const CLOCK_GATE: u32 = reg("CLOCK_GATE");
/// Who writes the per-line registers: `CPU_INT_ENABLE` bit n is set by
/// `ROM3:esprv_intc_int_enable@0x4005294e`, `CPU_INT_TYPE` bit n (1: edge) by
/// `ROM3:esprv_intc_int_set_type@0x40052972`, `CPU_INT_PRI_n` by
/// `ROM3:esprv_intc_int_set_priority@0x40052992`, `CPU_INT_THRESH` by
/// `ROM3:esprv_intc_int_set_threshold@0x400529a2`, and a `<SRC>_MAP` register by
/// `ROM3:intr_matrix_set@0x40047a6a`, which ESP-IDF calls as `esp_rom_route_intr_matrix`
/// (`IDF:esp_hw_support/intr_alloc.c:663`).
const CPU_INT_ENABLE: u32 = reg("CPU_INT_ENABLE");
const CPU_INT_TYPE: u32 = reg("CPU_INT_TYPE");
const CPU_INT_CLEAR: u32 = reg("CPU_INT_CLEAR");
const CPU_INT_EIP_STATUS: u32 = reg("CPU_INT_EIP_STATUS");
const CPU_INT_PRI_0: u32 = reg("CPU_INT_PRI_0");
const CPU_INT_THRESH: u32 = reg("CPU_INT_THRESH");
// The two blocks are consecutive registers: the index arithmetic below relies on it.
const _: () = assert!(
    reg("CACHE_CORE0_ACS_INT_MAP") == MAP_0 + 4 * (SOURCES - 1)
        && reg("CPU_INT_PRI_31") == CPU_INT_PRI_0 + 4 * (LINES - 1)
);

/// Bits [4:0] of a `<SRC>_MAP` register: the CPU line; 0 routes the source nowhere.
const MAP_MASK: u32 = 0x1F;
/// Bits [3:0] of a priority or threshold register.
const PRIORITY_MASK: u32 = 0xF;
/// Bits [31:1] of the per-line registers: line 0 does not exist.
const LINES_MASK: u32 = !1;
/// `CLOCK_GATE.CLK_EN`, bit 0 (reset value 1 in the SVD).
const CLOCK_GATE_MASK: u32 = 1;
const CLOCK_GATE_RESET: u32 = 1;

/// `CLOCK_GATE` only gates the clock of the register file.
pub(crate) fn accepted() -> Vec<Accepted> {
    vec![Accepted::svd(
        "INTERRUPT_CORE0",
        "CLOCK_GATE",
        "clock gate of the controller's registers: they always answer",
        "SP8",
    )]
}

#[derive(Clone, Debug)]
pub struct IntMatrix {
    map: [u32; SOURCE_COUNT],
    clock_gate: u32,
    enable: u32,
    edge_type: u32,
    /// `CPU_INT_CLEAR` as last written: an edge is acknowledged by a bit going from 1 to 0.
    clear: u32,
    priority: [u32; CPU_LINE_COUNT],
    threshold: u32,
    /// `level(n)` at the last evaluation: a rising edge is a line that is high now and was low then.
    level: u32,
    /// Edge latches: bit `n` is set when edge-type line `n` rose and was not acknowledged.
    latched: u32,
    /// Lines the CPU has taken since their latch was set.
    claimed: u32,
}

/// Index of `offset` in a block of `count` 32-bit registers starting at `first`.
fn index_in(offset: u32, first: u32, count: usize) -> Option<usize> {
    let index = usize::try_from(offset.checked_sub(first)? / 4).ok()?;
    (index < count).then_some(index)
}

impl IntMatrix {
    #[must_use]
    pub const fn new() -> IntMatrix {
        IntMatrix {
            map: [0; SOURCE_COUNT],
            clock_gate: CLOCK_GATE_RESET,
            enable: 0,
            edge_type: 0,
            clear: 0,
            priority: [0; CPU_LINE_COUNT],
            threshold: 0,
            level: 0,
            latched: 0,
            claimed: 0,
        }
    }

    /// `level(n)` for every line: the OR of the sources routed to it (TRM §8.3.3.2).
    fn line_levels(&self, sources: u64) -> u32 {
        let mut levels = 0;
        for (source, line) in self.map.iter().enumerate() {
            if sources & (1 << source) != 0 {
                levels |= 1 << line;
            }
        }
        levels & LINES_MASK
    }

    /// Brings the lines up to date with the source levels, latching the rising edges.
    ///
    /// ASSUMPTION: an edge is latched only while its line is enabled: TRM §1.5.2 property 4 speaks
    /// of "an enabled and unmasked interrupt signal" and says no more (report IR §1.3, open
    /// question 8). No ROM or ESP-IDF code on the measured paths uses an edge line; a probe on a
    /// real chip settles it.
    fn update(&mut self, sources: u64) {
        let levels = self.line_levels(sources);
        let rising = levels & !self.level;
        self.latched |= rising & self.edge_type & self.enable;
        self.level = levels;
    }

    /// `pending(n)` for every line: what `CPU_INT_EIP_STATUS` reads.
    fn pending(&self) -> u32 {
        let raw = (self.level & !self.edge_type) | (self.latched & self.edge_type);
        let mut pending = 0;
        for (line, priority) in self.priority.iter().enumerate() {
            // "Only if an interrupt has non-zero priority, higher or equal to the value in the
            // threshold register, will it be reflected in CPU_INT_EIP_STATUS" (TRM register 8.60).
            if *priority != 0 && *priority >= self.threshold {
                pending |= 1 << line;
            }
        }
        pending & raw & self.enable & LINES_MASK
    }

    /// The line the CPU must take given the level of every source, if any: the pending line of
    /// highest priority, and among equals the lowest line number ("statically prioritized by
    /// their IDs, lowest ID having highest priority", TRM §1.5.2).
    pub fn resolve(&mut self, sources: u64) -> Option<u32> {
        self.update(sources);
        let pending = self.pending();
        let mut best: Option<(u32, u32)> = None;
        for (line, priority) in (0u32..).zip(self.priority) {
            if pending & (1 << line) != 0 && best.is_none_or(|(_, top)| priority > top) {
                best = Some((line, priority));
            }
        }
        best.map(|(line, _)| line)
    }

    /// The CPU took `line`: an edge latched on it can now be acknowledged (TRM §1.5.2: the
    /// acknowledge clears "the pending state of claimed edge-type interrupts only").
    pub fn claim(&mut self, line: u32) {
        self.claimed |= self.latched & (1 << (line & 31));
    }

    /// A write to `CPU_INT_CLEAR`: "first setting and then clearing the corresponding bit" clears
    /// the edge latch of a line the CPU has taken; a latched edge that was never taken can only be
    /// flushed once its line is disabled (TRM §1.5.2, properties 4 and 5).
    ///
    /// ASSUMPTION: the latch is cleared when the bit goes from 1 to 0, the end of the toggle. What
    /// a write of 1 alone does is not stated. ESP-IDF's own helper only ever sets the bit
    /// (`rv_utils_intr_edge_ack`, `IDF:riscv/include/esp_private/interrupt_intc.h`; report IR
    /// §1.3), so the firmware does not settle it; a probe on a real chip does.
    fn acknowledge(&mut self, value: u32) {
        let released = self.clear & !value & LINES_MASK;
        let clearable = self.claimed | !self.enable;
        self.latched &= !(released & clearable);
        self.claimed &= self.latched;
        self.clear = value & LINES_MASK;
    }
}

impl Default for IntMatrix {
    fn default() -> Self {
        IntMatrix::new()
    }
}

impl Peripheral<Ctx> for IntMatrix {
    fn read(&mut self, ctx: &mut Ctx, offset: u32) -> Option<u32> {
        if let Some(source) = index_in(offset, MAP_0, SOURCE_COUNT) {
            return Some(self.map[source]);
        }
        if let Some(line) = index_in(offset, CPU_INT_PRI_0, CPU_LINE_COUNT) {
            return Some(self.priority[line]);
        }
        let sources = ctx.sources();
        Some(match offset {
            // The low 32 sources, then the 30 others.
            STATUS_0 => half_of(sources, false),
            STATUS_1 => half_of(sources, true),
            CPU_INT_EIP_STATUS => {
                self.update(sources);
                self.pending()
            }
            CLOCK_GATE => self.clock_gate,
            CPU_INT_ENABLE => self.enable,
            CPU_INT_TYPE => self.edge_type,
            CPU_INT_CLEAR => self.clear,
            CPU_INT_THRESH => self.threshold,
            _ => return None,
        })
    }

    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        // The lines are brought up to date first: a register written now must not turn a level
        // that was already high into a fresh edge.
        self.update(ctx.sources());
        if let Some(source) = index_in(offset, MAP_0, SOURCE_COUNT) {
            self.map[source] = value & MAP_MASK;
        } else if let Some(line) = index_in(offset, CPU_INT_PRI_0, CPU_LINE_COUNT) {
            self.priority[line] = value & PRIORITY_MASK;
        } else {
            match offset {
                CLOCK_GATE => self.clock_gate = value & CLOCK_GATE_MASK,
                CPU_INT_ENABLE => self.enable = value & LINES_MASK,
                CPU_INT_TYPE => self.edge_type = value & LINES_MASK,
                CPU_INT_CLEAR => self.acknowledge(value),
                CPU_INT_THRESH => self.threshold = value & PRIORITY_MASK,
                _ => return false,
            }
        }
        ctx.interrupts_changed();
        true
    }
}
````

- [ ] **Step 5: Run the suite, regenerate the table**

Run: `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect > /tmp/bless.log 2>&1; echo "exit=$?"; /usr/bin/git diff --stat docs/registers-accepted-without-effect.md`
Expected: `exit=0`; the rows of `INTERRUPT_CORE0` leave the table but `CLOCK_GATE`, whose heir becomes SP8.

Run: `cargo test --workspace > /tmp/t5.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t5.log`
Expected: `exit=0`, `607`.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/soc-esp32c3/src/periph/intmatrix.rs crates/soc-esp32c3/src/periph/mod.rs docs/registers-accepted-without-effect.md
/usr/bin/git commit -m "feat(soc): the interrupt controller selects the line to take

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3/src/periph/intmatrix.rs crates/soc-esp32c3/src/periph/mod.rs docs/registers-accepted-without-effect.md
```

---

## Task 6: The hart takes the line a controller presents; `Hart::reset`; the ESP32-C3 `mtvec`

`cpu-rv32` only. The generic hart (`inkemu rvtest`, RISCOF) keeps the standard behaviour: nothing changes unless `InterruptModel::Controller` is selected.

**Files:**
- Modify: `crates/cpu-rv32/src/hart.rs`, `crates/cpu-rv32/src/csr.rs`, `crates/cpu-rv32/src/lib.rs`, `crates/soc-esp32c3/src/machine.rs`

**Interfaces:**
- Consumes: `Hart { x, pc, csr, cycles, instret, config, waiting, dcache }`, `HartConfig { misaligned, vendor_csrs }`, `Hart::step`, `CsrFile::new(vendor: VendorCsrs)`, `DecodeCache::new()`.
- Produces: `cpu_rv32::InterruptModel { Standard, Controller }` (`Default` = `Standard`); `HartConfig.interrupts: InterruptModel`; `Hart::set_external_interrupt(&mut self, line: Option<u32>)`, `Hart::reset(&mut self, reset_pc: u32)`, `Hart::is_waiting(&self) -> bool`. With `Controller`: the presented line is taken when `mstatus.MIE` is set (`mepc` = PC, `mcause = 0x8000_0000 | line`, PC = `(mtvec & !0xFF) + 4 × line`), `wfi` ends when a line is presented whatever `mstatus.MIE`, and `mie`/`mip` gate nothing. With `VendorCsrs::Esp32C3`: `mtvec` reads 1 at reset and a write stores `(value & !0xFF) | 1`.

- [ ] **Step 1: Write the failing tests**

In the test module of `crates/cpu-rv32/src/hart.rs` (the first interrupt of the reference application is line 3: `mcause 0x80000003`, IR §3.3) and of `crates/cpu-rv32/src/csr.rs` (the ROM writes `0x40001D00` and reads `0x40001D01`, IR §2.2):

```diff
--- a/crates/cpu-rv32/src/hart.rs
+++ b/crates/cpu-rv32/src/hart.rs
@@ -920,6 +972,114 @@ mod tests {
         assert_eq!(hart.x[6] - hart.x[5], 2);
     }
 
+    /// A hart whose interrupts come from an outside controller, as on the ESP32-C3.
+    fn controller_machine(program: &[u32]) -> (Hart, Ram) {
+        let (_, ram) = c3_machine(program);
+        let config = HartConfig {
+            vendor_csrs: VendorCsrs::Esp32C3,
+            interrupts: InterruptModel::Controller,
+            ..HartConfig::default()
+        };
+        (Hart::new(0, config), ram)
+    }
+
+    #[test]
+    fn a_line_presented_by_the_controller_is_taken_whatever_mie_and_mip_say() {
+        // IR §2.4: on the ESP32-C3 `mie` gates nothing; §3.3: the first interrupt of the
+        // reference application is line 3, vector `mtvec.BASE + 12`, `mcause` 0x80000003.
+        let (mut h, mut m) = controller_machine(&[addi(1, 0, 1), addi(1, 1, 1)]);
+        assert!(h.csr.write(MTVEC, 0x800));
+        assert!(h.csr.write(MSTATUS, MSTATUS_MIE));
+        assert_eq!(h.csr.mie, 0, "no line is enabled in `mie`");
+        run(&mut h, &mut m, 1);
+        h.set_external_interrupt(Some(3));
+        assert_eq!(
+            h.step(&mut m, &OneCycle),
+            StepOutcome::Interrupted { cause: 3 }
+        );
+        assert_eq!(h.pc, 0x800 + 4 * 3);
+        assert_eq!(h.csr.mcause, 0x8000_0003);
+        assert_eq!(h.csr.mepc, 4);
+        assert_eq!(h.csr.mip, 0, "`mip` is not driven by the controller");
+        // `mstatus.MIE` is cleared on entry: the line stays presented and is not taken again.
+        h.pc = 4;
+        assert!(matches!(
+            h.step(&mut m, &OneCycle),
+            StepOutcome::Retired { .. }
+        ));
+        // Bits of `mip` and `mie` alone raise nothing.
+        h.set_external_interrupt(None);
+        assert!(h.csr.write(MSTATUS, MSTATUS_MIE));
+        assert!(h.csr.write(MIE, 1 << 7));
+        h.set_interrupt_pending(7, true);
+        h.pc = 0;
+        assert!(matches!(
+            h.step(&mut m, &OneCycle),
+            StepOutcome::Retired { .. }
+        ));
+    }
+
+    #[test]
+    fn wfi_ends_when_the_controller_presents_a_line_even_with_interrupts_masked() {
+        // Decision E4; IR §2.6: the instruction after `wfi` runs, no trap is taken.
+        let (mut h, mut m) = controller_machine(&[wfi(), addi(1, 0, 1)]);
+        run(&mut h, &mut m, 1);
+        assert!(h.is_waiting());
+        assert_eq!(h.step(&mut m, &OneCycle), StepOutcome::Waiting);
+        h.set_external_interrupt(Some(4));
+        assert!(matches!(
+            h.step(&mut m, &OneCycle),
+            StepOutcome::Retired { .. }
+        ));
+        assert_eq!(h.x[1], 1);
+        assert!(!h.is_waiting());
+    }
+
+    #[test]
+    fn the_standard_model_ignores_a_line_presented_by_a_controller() {
+        let (mut h, mut m) = machine(&[addi(1, 0, 1)], MisalignedPolicy::Trap);
+        assert!(h.csr.write(MSTATUS, MSTATUS_MIE));
+        h.set_external_interrupt(Some(3));
+        assert!(matches!(
+            h.step(&mut m, &OneCycle),
+            StepOutcome::Retired { .. }
+        ));
+    }
+
+    #[test]
+    fn a_reset_returns_the_hart_to_its_reset_state_and_keeps_the_totals() {
+        const PMPCFG0: u16 = 0x3A0;
+        let (mut h, mut m) = controller_machine(&[addi(1, 0, 1), wfi()]);
+        assert!(h.csr.write(MTVEC, 0x800));
+        assert!(h.csr.write(PMPCFG0, 0x9F), "an entry with its lock bit");
+        run(&mut h, &mut m, 2);
+        assert!(h.is_waiting());
+        h.set_external_interrupt(Some(5));
+        let (cycles, instret) = (h.cycles, h.instret);
+        h.reset(0x40);
+        assert_eq!(h.pc, 0x40);
+        assert_eq!(h.x, [0; 32]);
+        assert!(!h.is_waiting());
+        assert_eq!(
+            h.csr.read(PMPCFG0, h.cycles, h.instret),
+            Some(0),
+            "the PMP entries and their locks are cleared"
+        );
+        assert_eq!(h.csr.mtvec, 1, "the ESP32-C3 reset value");
+        assert_eq!(
+            (h.cycles, h.instret),
+            (cycles, instret),
+            "totals of the run"
+        );
+        // The presented line is forgotten: the controller presents it again if it still selects it.
+        assert!(h.csr.write(MSTATUS, MSTATUS_MIE));
+        h.pc = 0;
+        assert!(matches!(
+            h.step(&mut m, &OneCycle),
+            StepOutcome::Retired { .. }
+        ));
+    }
+
     #[cfg(debug_assertions)]
     #[test]
     #[should_panic(expected = "out of range")]
```

```diff
--- a/crates/cpu-rv32/src/csr.rs
+++ b/crates/cpu-rv32/src/csr.rs
@@ -297,6 +310,37 @@ mod tests {
         assert_eq!(csr.read(MCYCLE, Cycles(5), 1), Some(55));
     }
 
+    #[test]
+    fn the_esp32_c3_mtvec_is_vectored_only_with_a_256_byte_aligned_base() {
+        // IR §2.2: the ROM writes 0x40001D00 and reads back 0x40001D01; the application writes
+        // 0x40380001.
+        let mut csr = CsrFile::new(VendorCsrs::Esp32C3);
+        assert_eq!(
+            csr.read(MTVEC, Cycles::ZERO, 0),
+            Some(1),
+            "vectored at reset"
+        );
+        assert!(csr.write(MTVEC, 0x4000_1D00));
+        assert_eq!(csr.read(MTVEC, Cycles::ZERO, 0), Some(0x4000_1D01));
+        assert!(csr.write(MTVEC, 0x4038_0001));
+        assert_eq!(csr.read(MTVEC, Cycles::ZERO, 0), Some(0x4038_0001));
+        assert!(csr.write(MTVEC, 0x4038_00FE));
+        assert_eq!(
+            csr.read(MTVEC, Cycles::ZERO, 0),
+            Some(0x4038_0001),
+            "the bits under the base read 0, the mode reads 1"
+        );
+        // The generic core keeps the two standard modes.
+        let mut generic = CsrFile::new(VendorCsrs::None);
+        assert_eq!(generic.read(MTVEC, Cycles::ZERO, 0), Some(0));
+        assert!(generic.write(MTVEC, 0x1000));
+        assert_eq!(
+            generic.read(MTVEC, Cycles::ZERO, 0),
+            Some(0x1000),
+            "direct mode"
+        );
+    }
+
     #[test]
     fn vendor_csrs_exist_only_when_selected() {
         let mut generic = CsrFile::new(VendorCsrs::None);
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p cpu-rv32 > /tmp/t6.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, compile errors: `cannot find type 'InterruptModel'`, no method `set_external_interrupt`, `is_waiting`, `reset`.

- [ ] **Step 3: The hart**

```diff
--- a/crates/cpu-rv32/src/hart.rs
+++ b/crates/cpu-rv32/src/hart.rs
@@ -26,11 +26,29 @@ pub enum MisalignedPolicy {
     Allow,
 }
 
+/// Where the hart's interrupts come from.
+#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
+pub enum InterruptModel {
+    /// The privileged specification: a line is taken when its bit is set in both `mip` and `mie`
+    /// (the default).
+    #[default]
+    Standard,
+    /// An interrupt controller outside the hart selects at most one line and presents it with
+    /// [`Hart::set_external_interrupt`]. `mie` and `mip` are plain read/write CSRs that gate
+    /// nothing; `mstatus.MIE` still does.
+    ///
+    /// ASSUMPTION: on the ESP32-C3, `mie` and `mip` are plain registers, and `wfi` ends when a
+    /// line is presented even with `mstatus.MIE` clear (decisions E3 and E4). Neither is
+    /// documented (report IR §7, questions 4 and 5); a probe on a real chip settles both.
+    Controller,
+}
+
 #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
 pub struct HartConfig {
     pub misaligned: MisalignedPolicy,
     /// Which vendor's CSRs the hart implements besides the machine-mode ones.
     pub vendor_csrs: VendorCsrs,
+    pub interrupts: InterruptModel,
 }
 
 #[derive(Clone, Copy, Debug, PartialEq, Eq)]
@@ -54,6 +72,8 @@ pub struct Hart {
     pub instret: u64,
     pub config: HartConfig,
     pub(crate) waiting: bool,
+    /// The line an outside controller presents ([`InterruptModel::Controller`]).
+    pub(crate) external: Option<u32>,
     pub(crate) dcache: DecodeCache,
 }
 
@@ -68,10 +88,42 @@ impl Hart {
             instret: 0,
             config,
             waiting: false,
+            external: None,
             dcache: DecodeCache::new(),
         }
     }
 
+    /// Resets the hart: registers, CSRs (the PMP entries and their lock bits with them) and
+    /// decoded instructions are those of a new hart about to execute `reset_pc`. The cycle and
+    /// instruction totals are measures of the run, not state of the core: they go on.
+    ///
+    /// ASSUMPTION: a reset clears the integer registers. The architecture leaves them undefined;
+    /// the mask ROM writes every register it reads, so no measured boot tells the difference.
+    pub fn reset(&mut self, reset_pc: u32) {
+        self.x = [0; 32];
+        self.pc = reset_pc;
+        self.csr = CsrFile::new(self.config.vendor_csrs);
+        self.waiting = false;
+        self.external = None;
+        self.dcache = DecodeCache::new();
+    }
+
+    /// Presents the line an outside interrupt controller selected, `None` when it selects none.
+    /// Only used with [`InterruptModel::Controller`]; `line` must be lower than 32.
+    pub fn set_external_interrupt(&mut self, line: Option<u32>) {
+        debug_assert!(
+            line.is_none_or(|line| line < 32),
+            "interrupt line out of range"
+        );
+        self.external = line;
+    }
+
+    /// Whether the hart is parked in `wfi`.
+    #[must_use]
+    pub const fn is_waiting(&self) -> bool {
+        self.waiting
+    }
+
     /// `(hits, misses)` of the decoded-instruction cache, for performance reporting.
     #[must_use]
     pub fn decode_cache_stats(&self) -> (u64, u64) {
@@ -89,23 +141,27 @@ impl Hart {
         }
     }
 
-    /// Highest-numbered line that is pending and enabled (`ESP32-C3` priorities are applied by the `SoC` in `SP1b`).
+    /// The line to take: the highest-numbered one that is pending and enabled in the CSRs, or the
+    /// one an outside controller presents.
     fn pending_line(&self) -> Option<u32> {
-        let ready = self.csr.mip & self.csr.mie;
-        ready.checked_ilog2()
+        match self.config.interrupts {
+            InterruptModel::Standard => (self.csr.mip & self.csr.mie).checked_ilog2(),
+            InterruptModel::Controller => self.external,
+        }
     }
 
     /// Executes one instruction, or takes one pending interrupt, or reports that the hart is waiting.
     pub fn step(&mut self, bus: &mut dyn Bus, timing: &dyn TimingModel) -> StepOutcome {
+        let pending = self.pending_line();
         if self.waiting {
-            if self.csr.mip & self.csr.mie == 0 {
+            if pending.is_none() {
                 return StepOutcome::Waiting;
             }
             // wfi resumes on any enabled pending interrupt, even when mstatus.MIE masks it.
             self.waiting = false;
         }
         if self.csr.mstatus & MSTATUS_MIE != 0
-            && let Some(line) = self.pending_line()
+            && let Some(line) = pending
         {
             self.enter_trap(MCAUSE_INTERRUPT | line, 0);
             return StepOutcome::Interrupted { cause: line };
```

```diff
--- a/crates/cpu-rv32/src/lib.rs
+++ b/crates/cpu-rv32/src/lib.rs
@@ -18,6 +18,6 @@ pub mod timing;
 pub mod trap;
 
 pub use csr::VendorCsrs;
-pub use hart::{Hart, HartConfig, MisalignedPolicy, StepOutcome};
+pub use hart::{Hart, HartConfig, InterruptModel, MisalignedPolicy, StepOutcome};
 pub use timing::DatasheetTiming;
 pub use trap::{Exception, Trap};
```

- [ ] **Step 4: The ESP32-C3 `mtvec`**

```diff
--- a/crates/cpu-rv32/src/csr.rs
+++ b/crates/cpu-rv32/src/csr.rs
@@ -29,6 +29,10 @@ pub const MSTATUS_MPIE: u32 = 1 << 7;
 /// MPP is hard-wired to machine mode (0b11): this hart has no lower privilege mode in scope.
 pub const MSTATUS_MPP_M: u32 = 0b11 << 11;
 const MSTATUS_WRITABLE: u32 = MSTATUS_MIE | MSTATUS_MPIE;
+/// `mtvec.MODE` = 1, vectored.
+const MTVEC_VECTORED: u32 = 1;
+/// The bits of `mtvec` below `BASE` on the ESP32-C3 core.
+const MTVEC_C3_LOW: u32 = 0xFF;
 /// MXL=1 (32-bit) | C | I | M.
 const MISA_RV32IMC: u32 = (1 << 30) | (1 << 2) | (1 << 8) | (1 << 12);
 /// CSR addresses whose bits [11:10] are 0b11 are read-only (Privileged spec §2.1).
@@ -76,6 +80,10 @@ impl CsrFile {
     #[must_use]
     pub fn new(vendor: VendorCsrs) -> CsrFile {
         CsrFile {
+            mtvec: match vendor {
+                VendorCsrs::None => 0,
+                VendorCsrs::Esp32C3 => MTVEC_VECTORED,
+            },
             vendor: match vendor {
                 VendorCsrs::None => None,
                 VendorCsrs::Esp32C3 => Some(Esp32C3Csrs::default()),
@@ -131,6 +139,11 @@ impl CsrFile {
             // `mip` pending bits are driven by the interrupt controller, not by software;
             // `misa` is WARL: writes are ignored, the ISA is fixed.
             MIP | MISA => {}
+            // The ESP32-C3 core only has the vectored mode: `MODE` reads 1, `BASE` is bits 31:8
+            // (TRM §1.5.2, register 1.7; report IR §2.1).
+            // ASSUMPTION: bits 7:2 read 0. The TRM draws `BASE` as [31:8] and says nothing of the
+            // bits below it; every value the ROM and ESP-IDF write has them clear.
+            MTVEC if self.vendor.is_some() => self.mtvec = (value & !MTVEC_C3_LOW) | MTVEC_VECTORED,
             MTVEC => self.mtvec = value & !0b10, // modes 0 (direct) and 1 (vectored) only
             MSCRATCH => self.mscratch = value,
             MEPC => self.mepc = value & !1,
```

The machine names its hart configuration field by field; it keeps the default interrupt model until Task 8:

```diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -76,6 +76,7 @@ impl Esp32C3 {
         let hart_config = HartConfig {
             misaligned: MisalignedPolicy::Allow,
             vendor_csrs: VendorCsrs::Esp32C3,
+            ..HartConfig::default()
         };
         let mut bus = SocBus::new(rom, flash, config);
         let (sysclk_conf, cpu_per_conf) = bus.periph.system.clock_registers();
```

- [ ] **Step 5: Run the suite**

Run: `cargo test --workspace > /tmp/t6.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t6.log`
Expected: `exit=0`, `612`. The trap tests of `machine.rs` pass unchanged: their vector addresses are aligned to 256 bytes and take exceptions, which enter at the base in both modes.

Run the conformance suites of the generic hart, which must not have moved: `cargo test -p emu-cli --test rvtest_cli > /tmp/rv.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/cpu-rv32/src crates/soc-esp32c3/src/machine.rs
/usr/bin/git commit -m "feat(cpu): a hart can take its interrupts from an outside controller; reset; ESP32-C3 mtvec

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/cpu-rv32/src crates/soc-esp32c3/src/machine.rs
```

---

## Task 7: Counters as a value at an instant; the SYSTIMER

The first device that asks to be called. It works without a machine: its tests move the time of a context and deliver the wake-ups by hand. Task 8 makes the machine do it.

**Files:**
- Create: `crates/soc-esp32c3/src/periph/counter.rs`, `crates/soc-esp32c3/src/periph/systimer.rs`
- Modify: `crates/soc-esp32c3/src/periph/mod.rs`, `crates/soc-esp32c3/src/ctx.rs`, `crates/soc-esp32c3/src/bus.rs`

**Interfaces:**
- Consumes: `Ctx::{now, sleep_until, schedule, next_wake, take_due, irq, sources}`, `Deadline`, `Source::of`, `Event` (Task 3); `periph::{half_of, replace_half}` (Task 5); `Peripherals::deliver` (Task 4); `Peripherals::new(config: &SocConfig, flash: Box<dyn FlashDevice>)`.
- Produces:
  - `periph::counter::Rate` with `every(period: u64) -> Rate`, `ratio(counts: u64, cycles: u64) -> Rate`, `divided_by(self, divider: u64) -> Rate`, `counts_in(self, cycles: u64) -> u64`, `cycles_for(self, counts: u64) -> u64`; `periph::counter::Counter` with `new(value: u64, now: Cycles, rate: Rate, running: bool) -> Counter`, `is_running(&self) -> bool`, `value(&self, now: Cycles) -> u64`, `set(&mut self, value: u64, instant: Cycles)`, `run(&mut self, running: bool, now: Cycles)`, `retime(&mut self, rate: Rate, now: Cycles)`, `instant_in(&self, ahead: u64, now: Cycles) -> Option<Cycles>`.
  - `Event::Systimer`; `periph::Systimer` with `Systimer::new(now: Cycles) -> Systimer` and the crate-visible `wake(&mut self, ctx: &mut Ctx)`; `periph::SYSTIMER_BASE` (crate-visible).
  - `Peripherals::new(config: &SocConfig, flash: Box<dyn FlashDevice>, ctx: &mut Ctx) -> Peripherals`: the devices are built at the device time of the context, and those that have a first deadline schedule it there. `Peripherals.systimer: Systimer`.
  - Registers of `SYSTIMER` modelled: `CONF`, `UNIT0/1_OP`, `UNIT0/1_LOAD_HI/LO`, `TARGET0..2_HI/LO`, `TARGET0..2_CONF`, `UNIT0/1_VALUE_HI/LO`, `COMP0..2_LOAD`, `UNIT0/1_LOAD`, `INT_ENA`, `INT_RAW`, `INT_CLR`, `INT_ST`, `DATE`. Sources 37 to 39 (`SYSTIMER_TARGET0..2_INT_MAP`) follow `INT_RAW & INT_ENA`.

- [ ] **Step 1: Write the failing tests of the counter**

`crates/soc-esp32c3/src/periph/counter.rs`, test module. 136 kHz is 17 counts per 20 000 cycles of the time base (TW §C.1):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_whole_period_counts_exactly() {
        // A SYSTIMER counter: 16 MHz, one count every 10 cycles of the time base.
        let counter = Counter::new(0, Cycles(100), Rate::every(10), true);
        assert_eq!(counter.value(Cycles(100)), 0);
        assert_eq!(counter.value(Cycles(109)), 0);
        assert_eq!(counter.value(Cycles(110)), 1);
        assert_eq!(counter.value(Cycles(1_600_100)), 160_000);
        assert_eq!(
            counter.instant_in(160_000, Cycles(100)),
            Some(Cycles(1_600_100))
        );
        // 5 cycles into a count, the next count is 5 cycles away, not 10.
        assert_eq!(counter.instant_in(1, Cycles(105)), Some(Cycles(110)));
    }

    #[test]
    fn a_rate_that_is_not_a_whole_number_of_cycles_does_not_drift() {
        // The 136 kHz RC oscillator: 136 000 counts in 160 000 000 cycles, 17 in 20 000.
        let slow = Rate::ratio(17, 20_000);
        let counter = Counter::new(0, Cycles::ZERO, slow, true);
        assert_eq!(counter.value(Cycles(160_000_000)), 136_000, "one second");
        assert_eq!(
            counter.value(Cycles(1_600_000_000)),
            1_360_000,
            "ten seconds"
        );
        // 400 000 counts, the flash-boot timeout of the RTC watchdog: 2.941 s (TW §D.4).
        let at = counter.instant_in(400_000, Cycles::ZERO).unwrap();
        assert_eq!(at, Cycles(470_588_236));
        assert_eq!(counter.value(at), 400_000);
        assert_eq!(counter.value(Cycles(at.get() - 1)), 399_999);
    }

    #[test]
    fn stopping_retiming_and_setting_keep_or_replace_the_value() {
        let mut counter = Counter::new(5, Cycles::ZERO, Rate::every(8), true);
        counter.run(false, Cycles(80));
        assert_eq!(counter.value(Cycles(8_000)), 15, "stopped at 15");
        assert_eq!(
            counter.instant_in(1, Cycles(8_000)),
            None,
            "a stopped counter reaches nothing"
        );
        assert!(!counter.is_running());
        counter.run(true, Cycles(8_000));
        // The APB clock goes from 20 MHz to 80 MHz: 2 cycles of the time base per count.
        counter.retime(Rate::every(2), Cycles(8_080));
        assert_eq!(counter.value(Cycles(8_080)), 25);
        assert_eq!(counter.value(Cycles(8_100)), 35);
        counter.set(0, Cycles(8_100));
        assert_eq!(counter.value(Cycles(8_104)), 2);
    }

    #[test]
    fn confirming_the_state_of_a_counter_loses_no_fraction_of_a_count() {
        // A device re-states "running, at this rate" every time it is called: 9 cycles into a
        // 10-cycle count, the count must still end at cycle 10.
        let mut counter = Counter::new(0, Cycles::ZERO, Rate::every(10), true);
        counter.run(true, Cycles(9));
        counter.retime(Rate::every(10), Cycles(9));
        assert_eq!(counter.value(Cycles(10)), 1);
    }

    #[test]
    fn dividers_and_degenerate_rates_never_divide_by_zero() {
        // The interrupt watchdog: the crystal (4 cycles) divided by 20 000 is 2 kHz (TW §B.3).
        let two_khz = Rate::every(4).divided_by(20_000);
        assert_eq!(two_khz.counts_in(160_000_000), 2_000);
        assert_eq!(two_khz.cycles_for(600), 48_000_000, "600 counts are 300 ms");
        assert_eq!(Rate::every(0), Rate::every(1));
        assert_eq!(Rate::every(4).divided_by(0), Rate::every(4));
        assert_eq!(Rate::every(1).counts_in(u64::MAX), u64::MAX);
        assert_eq!(
            Rate::every(u64::MAX).cycles_for(u64::MAX),
            u64::MAX,
            "saturates"
        );
    }
}
```

- [ ] **Step 2: Write the failing tests of the SYSTIMER**

`crates/soc-esp32c3/src/periph/systimer.rs`, test module. The sequence of `program_the_tick` is the one `vSystimerSetup` writes (TW §A.4): `TARGET0_CONF ← 0xC0027100` is period mode on counter 1 with a period of 160 000 counts, 10 ms; `CONF ← 0xF7000000` starts counter 1 and comparator 0:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const UNIT0_LOAD_LO: u32 = reg("UNIT0_LOAD_LO");
    const UNIT1_LOAD_HI: u32 = reg("UNIT1_LOAD_HI");
    const UNIT0_VALUE_LO: u32 = reg("UNIT0_VALUE_LO");

    /// 10 ms of device time: 160 000 counts, 1 600 000 cycles of the time base.
    const TICK_COUNTS: u32 = 160_000;
    const TICK: u64 = 1_600_000;
    const TICK_SOURCE: u64 = 1 << 37;

    fn after(ctx: &mut Ctx, cycles: u64) {
        ctx.sleep_until(ctx.now() + cycles);
    }

    /// Delivers every wake-up due at the present time, as the machine does.
    fn pump(ctx: &mut Ctx, timer: &mut Systimer) {
        while let Some(event) = ctx.take_due() {
            assert_eq!(event, Event::Systimer);
            timer.wake(ctx);
        }
    }

    fn counter(ctx: &mut Ctx, timer: &mut Systimer, unit: u32) -> u64 {
        assert!(timer.write(ctx, UNIT0_OP + 4 * unit, OP_UPDATE));
        assert_eq!(timer.read(ctx, UNIT0_OP + 4 * unit), Some(OP_VALUE_VALID));
        let lo = timer.read(ctx, UNIT0_VALUE_LO + 8 * unit).unwrap();
        let hi = timer.read(ctx, UNIT0_VALUE_HI + 8 * unit).unwrap();
        (u64::from(hi) << 32) | u64::from(lo)
    }

    /// The writes of `vSystimerSetup` (TW §A.4, values measured in trace M1).
    fn program_the_tick(ctx: &mut Ctx, timer: &mut Systimer) {
        for (offset, value) in [
            (UNIT1_LOAD_HI, 0),
            (UNIT1_LOAD_LO, 0),
            (UNIT1_LOAD, 1),
            (TARGET0_CONF, 0x8000_0000),
            (TARGET0_CONF, 0x8000_0000 | TICK_COUNTS),
            (COMP0_LOAD, 1),
            (CONF, 0xD300_0000),
            (TARGET0_CONF, 0xC000_0000 | TICK_COUNTS),
            (INT_ENA, 1),
            (CONF, 0xF700_0000),
        ] {
            assert!(timer.write(ctx, offset, value), "{offset:#x}");
        }
    }

    #[test]
    fn reset_values_are_those_of_the_header() {
        // TW §A.2.
        let ctx = &mut Ctx::default();
        let mut timer = Systimer::new(ctx.now());
        assert_eq!(timer.read(ctx, CONF), Some(0x4600_0000));
        assert_eq!(timer.read(ctx, DATE), Some(0x0200_6171));
        for offset in [
            UNIT0_OP,
            UNIT1_OP,
            TARGET0_CONF,
            TARGET2_CONF,
            INT_ENA,
            INT_RAW,
            INT_ST,
        ] {
            assert_eq!(timer.read(ctx, offset), Some(0), "{offset:#x}");
        }
        assert_eq!(timer.read(ctx, DATE + 4), None, "nothing past DATE");
        assert_eq!(
            timer.read(ctx, INT_ST + 4),
            None,
            "the C3 has no REAL_TARGET registers"
        );
        assert!(!timer.write(ctx, INT_ST, 1), "read-only");
    }

    #[test]
    fn counter_0_counts_at_16_mhz_from_reset_whatever_the_cpu_clock() {
        let ctx = &mut Ctx::default();
        let mut timer = Systimer::new(ctx.now());
        assert_eq!(
            timer.read(ctx, UNIT0_OP),
            Some(0),
            "no latch yet: not valid"
        );
        // 1 ms: 160 000 time-base cycles, 16 000 counts.
        after(ctx, 160_000);
        assert_eq!(counter(ctx, &mut timer, 0), 16_000);
        // The latch holds until the next update.
        after(ctx, 160_000);
        assert_eq!(timer.read(ctx, UNIT0_VALUE_LO), Some(16_000));
        assert_eq!(counter(ctx, &mut timer, 0), 32_000);
        assert_eq!(counter(ctx, &mut timer, 1), 0, "UNIT1_WORK_EN resets to 0");
    }

    #[test]
    fn a_stopped_counter_keeps_its_value_and_a_load_changes_it_at_once() {
        let ctx = &mut Ctx::default();
        let mut timer = Systimer::new(ctx.now());
        after(ctx, 1_000);
        // UNIT0_WORK_EN <- 0.
        timer.write(ctx, CONF, 0x0600_0000);
        after(ctx, 5_000);
        assert_eq!(counter(ctx, &mut timer, 0), 100);
        timer.write(ctx, CONF, 0x4600_0000);
        after(ctx, 1_000);
        assert_eq!(
            counter(ctx, &mut timer, 0),
            200,
            "it resumes from where it stopped"
        );
        timer.write(ctx, UNIT0_LOAD_HI, 0xFFFF_FFFF);
        timer.write(ctx, UNIT0_LOAD_LO, 0xFFFF_FFFE);
        assert_eq!(timer.read(ctx, UNIT0_LOAD_HI), Some(0x000F_FFFF), "20 bits");
        timer.write(ctx, UNIT0_LOAD, 1);
        assert_eq!(counter(ctx, &mut timer, 0), 0x000F_FFFF_FFFF_FFFE);
        after(ctx, 30);
        assert_eq!(
            counter(ctx, &mut timer, 0),
            1,
            "the counter wraps at 52 bits"
        );
    }

    #[test]
    fn the_freertos_tick_fires_every_160_000_counts_of_counter_1() {
        let ctx = &mut Ctx::default();
        let mut timer = Systimer::new(ctx.now());
        after(ctx, 12_345);
        program_the_tick(ctx, &mut timer);
        assert_eq!(timer.read(ctx, TARGET0_CONF), Some(0xC002_7100));
        // The early alarm of the ASSUMPTION in `evaluate`: target 0 on a counter stopped at 0.
        assert_eq!(timer.read(ctx, INT_ST), Some(1));
        assert_eq!(ctx.sources(), TICK_SOURCE);
        assert!(timer.write(ctx, INT_CLR, 1));
        assert_eq!(
            ctx.sources(),
            0,
            "the level drops when the handler clears it"
        );
        let start = ctx.now();
        assert_eq!(ctx.next_wake(), Some(start + TICK));
        for tick in 1..=3u64 {
            ctx.sleep_until(Cycles(start.get() + tick * TICK - 1));
            pump(ctx, &mut timer);
            assert_eq!(
                timer.read(ctx, INT_RAW),
                Some(0),
                "one cycle before tick {tick}"
            );
            after(ctx, 1);
            pump(ctx, &mut timer);
            assert_eq!(timer.read(ctx, INT_ST), Some(1));
            assert_eq!(ctx.sources(), TICK_SOURCE);
            // What `SysTickIsrHandler` computes: counter 1 divided by the period.
            assert_eq!(counter(ctx, &mut timer, 1) / u64::from(TICK_COUNTS), tick);
            timer.write(ctx, INT_CLR, 1);
        }
    }

    #[test]
    fn a_late_handler_finds_every_period_it_missed_in_the_counter() {
        // TW §J, third trap: the counter must have advanced by the paused time.
        let ctx = &mut Ctx::default();
        let mut timer = Systimer::new(ctx.now());
        program_the_tick(ctx, &mut timer);
        timer.write(ctx, INT_CLR, 1);
        after(ctx, 3 * TICK + 500);
        pump(ctx, &mut timer);
        assert_eq!(
            timer.read(ctx, INT_RAW),
            Some(1),
            "one level, not three interrupts"
        );
        assert_eq!(counter(ctx, &mut timer, 1) / u64::from(TICK_COUNTS), 3);
        timer.write(ctx, INT_CLR, 1);
        assert_eq!(
            ctx.next_wake(),
            Some(Cycles(4 * TICK)),
            "the next alarm is the fourth period"
        );
    }

    #[test]
    fn a_masked_alarm_sets_the_raw_bit_and_drives_nothing() {
        let ctx = &mut Ctx::default();
        let mut timer = Systimer::new(ctx.now());
        program_the_tick(ctx, &mut timer);
        timer.write(ctx, INT_CLR, 1);
        timer.write(ctx, INT_ENA, 0);
        after(ctx, TICK);
        pump(ctx, &mut timer);
        assert_eq!(
            (timer.read(ctx, INT_RAW), timer.read(ctx, INT_ST)),
            (Some(1), Some(0))
        );
        assert_eq!(ctx.sources(), 0);
        timer.write(ctx, INT_ENA, 1);
        assert_eq!(ctx.sources(), TICK_SOURCE, "enabling it raises the line");
    }

    #[test]
    fn a_one_shot_alarm_fires_at_its_target_and_at_once_when_the_target_is_passed() {
        // What `esp_timer` does with comparator 2 on counter 0 (TW §A.4).
        let ctx = &mut Ctx::default();
        let mut timer = Systimer::new(ctx.now());
        let program = |ctx: &mut Ctx, timer: &mut Systimer, target: u32| {
            timer.write(ctx, CONF, 0x4600_0000);
            timer.write(ctx, TARGET2_LO - 4, 0);
            timer.write(ctx, TARGET2_LO, target);
            timer.write(ctx, COMP2_LOAD, 1);
            timer.write(ctx, CONF, 0x4640_0000);
        };
        timer.write(ctx, INT_ENA, 0b100);
        program(ctx, &mut timer, 1_000);
        assert_eq!(ctx.next_wake(), Some(Cycles(10_000)));
        ctx.sleep_until(Cycles(10_000));
        pump(ctx, &mut timer);
        assert_eq!(timer.read(ctx, INT_ST), Some(0b100));
        assert_eq!(ctx.sources(), 1 << 39, "SYSTIMER_TARGET2 is source 39");
        timer.write(ctx, INT_CLR, 0b100);
        after(ctx, 100_000);
        pump(ctx, &mut timer);
        assert_eq!(
            timer.read(ctx, INT_RAW),
            Some(0),
            "one alarm per programming"
        );
        // The counter reads 11 000: a target of 5 000 is already passed.
        program(ctx, &mut timer, 5_000);
        assert_eq!(timer.read(ctx, INT_ST), Some(0b100), "fires immediately");
    }

    #[test]
    fn a_wake_up_for_a_state_that_is_gone_finds_nothing_to_do() {
        let ctx = &mut Ctx::default();
        let mut timer = Systimer::new(ctx.now());
        program_the_tick(ctx, &mut timer);
        timer.write(ctx, INT_CLR, 1);
        // The block is reset while its wake-up is still scheduled.
        timer = Systimer::new(ctx.now());
        after(ctx, TICK);
        pump(ctx, &mut timer);
        assert_eq!(timer.read(ctx, INT_RAW), Some(0));
        assert_eq!(ctx.next_wake(), None);
    }
}
```

- [ ] **Step 3: Run them and see them fail**

Declare the two modules in `periph/mod.rs` (`pub mod counter;`, `pub mod systimer;`) with only their test modules in place.

Run: `cargo test -p soc-esp32c3 --lib > /tmp/t7.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, compile errors: `cannot find type 'Rate'`, `'Counter'`, `'Systimer'`.

- [ ] **Step 4: The counter**

`crates/soc-esp32c3/src/periph/counter.rs`, above its test module:

```rust
//! A hardware counter as a value at an instant. Nothing is counted cycle by cycle: a counter knows
//! what it read at one device time and how fast it runs, which gives its value at any later time
//! and the time at which it will reach a value.
use emu_core::Cycles;

/// How fast a counter runs: `counts` counts every `cycles` cycles of the time base. A rate that is
/// not a whole number of time-base cycles per count (the RC oscillators) stays exact this way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate {
    counts: u64,
    cycles: u64,
}

impl Rate {
    /// One count every `period` cycles of the time base; a period of 0 is taken as 1.
    #[must_use]
    pub const fn every(period: u64) -> Rate {
        Rate {
            counts: 1,
            cycles: if period == 0 { 1 } else { period },
        }
    }

    /// `counts` counts every `cycles` cycles of the time base.
    ///
    /// # Panics
    /// When either is 0: a rate is a constant of the model, checked where it is written.
    #[must_use]
    pub const fn ratio(counts: u64, cycles: u64) -> Rate {
        assert!(counts != 0 && cycles != 0);
        Rate { counts, cycles }
    }

    /// The same clock divided by `divider`; a divider of 0 is taken as 1.
    #[must_use]
    pub const fn divided_by(self, divider: u64) -> Rate {
        Rate {
            counts: self.counts,
            cycles: self
                .cycles
                .saturating_mul(if divider == 0 { 1 } else { divider }),
        }
    }

    /// Whole counts in `cycles` cycles of the time base.
    #[must_use]
    pub fn counts_in(self, cycles: u64) -> u64 {
        let counts = u128::from(cycles) * u128::from(self.counts) / u128::from(self.cycles);
        u64::try_from(counts).unwrap_or(u64::MAX)
    }

    /// Time-base cycles until `counts` counts have gone by, rounded up to a whole cycle.
    #[must_use]
    pub fn cycles_for(self, counts: u64) -> u64 {
        let cycles =
            (u128::from(counts) * u128::from(self.cycles)).div_ceil(u128::from(self.counts));
        u64::try_from(cycles).unwrap_or(u64::MAX)
    }
}

/// A counter that counts up at a [`Rate`] while it runs. Its value wraps at 64 bits; a narrower
/// counter is masked by its owner.
#[derive(Clone, Copy, Debug)]
pub struct Counter {
    /// The value at `since`.
    base: u64,
    since: Cycles,
    rate: Rate,
    running: bool,
}

impl Counter {
    /// A counter that reads `value` at `now`.
    #[must_use]
    pub const fn new(value: u64, now: Cycles, rate: Rate, running: bool) -> Counter {
        Counter {
            base: value,
            since: now,
            rate,
            running,
        }
    }

    /// Whether the counter counts.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.running
    }

    /// Counts gone by since the counter was last set, started, stopped or retimed.
    fn elapsed(&self, now: Cycles) -> u64 {
        if self.running {
            self.rate
                .counts_in(now.get().saturating_sub(self.since.get()))
        } else {
            0
        }
    }

    /// The value at `now`.
    #[must_use]
    pub fn value(&self, now: Cycles) -> u64 {
        self.base.wrapping_add(self.elapsed(now))
    }

    /// The counter reads `value` at `instant`, and counts on from there.
    pub fn set(&mut self, value: u64, instant: Cycles) {
        self.base = value;
        self.since = instant;
    }

    /// Starts or stops the counter at `now`; it keeps its value.
    pub fn run(&mut self, running: bool, now: Cycles) {
        if running == self.running {
            // Nothing changes: the anchor stays, and with it the fraction of a count gone by.
            return;
        }
        self.base = self.value(now);
        self.since = now;
        self.running = running;
    }

    /// From `now` on the counter runs at `rate`; it keeps its value.
    pub fn retime(&mut self, rate: Rate, now: Cycles) {
        if rate == self.rate {
            return;
        }
        self.base = self.value(now);
        self.since = now;
        self.rate = rate;
    }

    /// The device time at which the counter will have counted `ahead` more than it reads at `now`,
    /// `None` when it is stopped.
    #[must_use]
    pub fn instant_in(&self, ahead: u64, now: Cycles) -> Option<Cycles> {
        self.running.then(|| {
            // From the anchor, so that the fraction of a count already gone by is not counted twice.
            self.since
                + self
                    .rate
                    .cycles_for(self.elapsed(now).saturating_add(ahead))
        })
    }
}
```

- [ ] **Step 5: The SYSTIMER**

`crates/soc-esp32c3/src/periph/systimer.rs`, above its test module:

```rust
//! `SYSTIMER`: two 52-bit counters and three comparators, the time base of ESP-IDF (`esp_timer`
//! on counter 0, the `FreeRTOS` tick on counter 1 and comparator 0). Sources: TRM chapter 10, ESP-IDF
//! `systimer_reg.h` and `systimer_ll.h`; report TW §A.
//!
//! The counters count at 16 MHz from the crystal whatever the CPU and APB clocks are (TRM §10.3;
//! TW §A.1): one count is 10 cycles of the time base. Nothing is polled: a counter is a value at an
//! instant, and a comparator asks to be woken at the device time of its next alarm.
use super::SYSTIMER_BASE;
use super::counter::{Counter, Rate};
use super::{half_of, replace_half};
use crate::ctx::{Ctx, Deadline, Event, Source};
use emu_core::{Cycles, Peripheral};
use svd_regs::esp32c3::addr;

const fn reg(name: &str) -> u32 {
    addr("SYSTIMER", name) - SYSTIMER_BASE
}

const CONF: u32 = reg("CONF");
const UNIT0_OP: u32 = reg("UNIT0_OP");
const UNIT1_OP: u32 = reg("UNIT1_OP");
const UNIT0_LOAD_HI: u32 = reg("UNIT0_LOAD_HI");
const UNIT1_LOAD_LO: u32 = reg("UNIT1_LOAD_LO");
const TARGET0_HI: u32 = reg("TARGET0_HI");
const TARGET2_LO: u32 = reg("TARGET2_LO");
const TARGET0_CONF: u32 = reg("TARGET0_CONF");
const TARGET2_CONF: u32 = reg("TARGET2_CONF");
const UNIT0_VALUE_HI: u32 = reg("UNIT0_VALUE_HI");
const UNIT1_VALUE_LO: u32 = reg("UNIT1_VALUE_LO");
const COMP0_LOAD: u32 = reg("COMP0_LOAD");
const COMP2_LOAD: u32 = reg("COMP2_LOAD");
const UNIT0_LOAD: u32 = reg("UNIT0_LOAD");
const UNIT1_LOAD: u32 = reg("UNIT1_LOAD");
const INT_ENA: u32 = reg("INT_ENA");
const INT_RAW: u32 = reg("INT_RAW");
const INT_CLR: u32 = reg("INT_CLR");
const INT_ST: u32 = reg("INT_ST");
const DATE: u32 = reg("DATE");

/// `CNT_CLK` is the crystal divided by 2.5, 16 MHz: one count every 10 cycles of the time base
/// (TRM §10.3).
const COUNT_RATE: Rate = Rate::every(10);
const UNITS: usize = 2;
const COMPARATORS: usize = 3;
/// A counter and a target are 52 bits wide: 20 bits in the `_HI` register (TW §A.2).
const COUNT_MASK: u64 = (1 << 52) - 1;
const HI_MASK: u32 = 0x000F_FFFF;
/// "If t is already passed (0 <= count - t < 2^51 - 1) it fires immediately" (TRM table 10.4-2).
const PASSED_WINDOW: u64 = (1 << 51) - 1;

/// `CONF` (reset `0x46000000`): bit 31 `CLK_EN`, bit 30 `UNIT0_WORK_EN`, bit 29 `UNIT1_WORK_EN`,
/// bits 28..25 the stall enables, bits 24, 23, 22 `TARGET0/1/2_WORK_EN`, bit 0 `SYSTIMER_CLK_FO`.
/// `systimer_ll_enable_counter` sets bit `30 - counter` and `systimer_ll_enable_alarm` bit
/// `24 - alarm` (`IDF:hal/esp32c3/include/hal/systimer_ll.h:70-77, 163-170`). The stall bits stop a
/// counter while the CPU is halted by a debugger (TRM table 10.4-1): there is no debugger, they are
/// stored (`systimer_ll_counter_can_stall_by_cpu`, same file, lines 79-86).
const CONF_WRITABLE: u32 = 0xFFC0_0001;
const UNIT0_WORK_EN: u32 = 1 << 30;
const TARGET0_WORK_EN: u32 = 1 << 24;
/// `UNITn_OP`: bit 30 `UPDATE` latches the counter (`systimer_ll_counter_snapshot`, line 88),
/// bit 29 `VALUE_VALID` says the latch can be read (`systimer_ll_is_counter_value_valid`, line 93).
const OP_UPDATE: u32 = 1 << 30;
const OP_VALUE_VALID: u32 = 1 << 29;
/// `TARGETx_CONF`: bit 31 the counter compared (`systimer_ll_connect_alarm_counter`, line 132),
/// bit 30 `PERIOD_MODE` (`systimer_ll_enable_alarm_period`, line 142), bits [25:0] the period
/// (`systimer_ll_set_alarm_period`, line 147).
const TARGET_CONF_WRITABLE: u32 = 0xC3FF_FFFF;
const TARGET_UNIT_SEL: u32 = 1 << 31;
const TARGET_PERIOD_MODE: u32 = 1 << 30;
const TARGET_PERIOD_MASK: u32 = 0x03FF_FFFF;
/// Bits 2..0 of the four interrupt registers: comparators 2, 1 and 0.
const INT_MASK: u32 = 0b111;
/// Bit 0 of `UNITn_LOAD` and `COMPx_LOAD`, a write-1 trigger (`systimer_ll_apply_counter_value`,
/// line 114; `systimer_ll_apply_alarm_value`, line 158).
const LOAD_TRIGGER: u32 = 1;
const CONF_RESET: u32 = 0x4600_0000;
const DATE_VALUE: u32 = 0x0200_6171;

/// Comparator `x` drives interrupt source `SYSTIMER_TARGETx` (37, 38, 39: report BC §F.1).
const SOURCES: [Source; COMPARATORS] = [
    Source::of("SYSTIMER_TARGET0_INT_MAP"),
    Source::of("SYSTIMER_TARGET1_INT_MAP"),
    Source::of("SYSTIMER_TARGET2_INT_MAP"),
];

#[derive(Clone, Copy, Debug)]
struct Unit {
    counter: Counter,
    /// `UNITn_LOAD_HI/LO`.
    load: u64,
    /// `UNITn_VALUE_HI/LO`: the count at the last `UPDATE`.
    latch: u64,
    valid: bool,
}

impl Unit {
    fn new(now: Cycles, working: bool) -> Unit {
        Unit {
            counter: Counter::new(0, now, COUNT_RATE, working),
            load: 0,
            latch: 0,
            valid: false,
        }
    }

    fn count(&self, now: Cycles) -> u64 {
        self.counter.value(now) & COUNT_MASK
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Comparator {
    /// `TARGETx_HI/LO` and `TARGETx_CONF` as software reads them.
    target: u64,
    conf: u32,
    /// The target and the period the comparator works with: copies made by `COMPx_LOAD`.
    loaded_target: u64,
    loaded_period: u64,
    /// The count of the next alarm, `None` when the comparator has nothing left to signal.
    next: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Systimer {
    conf: u32,
    units: [Unit; UNITS],
    comparators: [Comparator; COMPARATORS],
    int_ena: u32,
    int_raw: u32,
    wake: Deadline,
}

/// Splits a register offset inside a block of `(HI, LO)` pairs into the pair index and whether it
/// is the `HI` half.
fn pair_in(offset: u32, first: u32, last: u32) -> Option<(usize, bool)> {
    if !(first..=last).contains(&offset) {
        return None;
    }
    let word = (offset - first) / 4;
    Some((usize::try_from(word / 2).ok()?, word.is_multiple_of(2)))
}

impl Systimer {
    /// The block after a power-on or a peripheral reset: counter 0 counts from 0 at `now`
    /// (`UNIT0_WORK_EN` resets to 1), counter 1 is stopped, no comparator works.
    ///
    /// ASSUMPTION: a reset of the block (`SYSTEM.PERIP_RST_EN0` bit 29) returns both counters to 0.
    /// The TRM says the registers "will be restored to their default values" (§10.3) and does not
    /// name the counters (report TW §I question 6); a read of counter 0 after
    /// `esp_timer_impl_early_init` on a real chip settles it.
    #[must_use]
    pub fn new(now: Cycles) -> Systimer {
        Systimer {
            conf: CONF_RESET,
            units: [Unit::new(now, true), Unit::new(now, false)],
            comparators: [Comparator::default(); COMPARATORS],
            int_ena: 0,
            int_raw: 0,
            wake: Deadline::default(),
        }
    }

    const fn comparator_works(&self, comparator: usize) -> bool {
        self.conf & (TARGET0_WORK_EN >> comparator) != 0
    }

    fn count(&self, unit: usize, now: Cycles) -> u64 {
        self.units[unit].count(now)
    }

    fn unit_of(&self, comparator: usize) -> usize {
        usize::from(self.comparators[comparator].conf & TARGET_UNIT_SEL != 0)
    }

    /// Works out the next alarm of a comparator that was just enabled, loaded or switched to
    /// another mode: in period mode "alarms at t1 + n x period, t1 being the count at the start"
    /// (TRM §10.4.2); in target mode at the target, at once when it is already passed.
    fn start(&mut self, comparator: usize, now: Cycles) {
        let count = self.count(self.unit_of(comparator), now);
        let c = &mut self.comparators[comparator];
        c.next = if c.conf & TARGET_PERIOD_MODE == 0 {
            Some(c.loaded_target)
        } else if c.loaded_period == 0 {
            // A period of 0 would be an alarm at every count: nothing ESP-IDF programs.
            None
        } else {
            Some(count.wrapping_add(c.loaded_period) & COUNT_MASK)
        };
    }

    /// Raises every alarm that is due at `now`, then asks to be woken for the next one.
    ///
    /// ASSUMPTION: a comparator in target mode compares even when its counter is stopped, so an
    /// alarm whose target the stopped counter has already reached fires at once. This is TRM table
    /// 10.4-2 read literally; with the order in which `vSystimerSetup` programs the tick
    /// (`IDF:freertos/port_systick.c:60-118`) it gives one early tick interrupt that counts no tick
    /// (report TW §A.4, §I question 6). A trace of a real chip settles it.
    fn evaluate(&mut self, ctx: &mut Ctx) {
        let now = ctx.now();
        self.wake.called(now);
        let mut earliest: Option<Cycles> = None;
        for index in 0..COMPARATORS {
            if !self.comparator_works(index) {
                continue;
            }
            let unit = self.unit_of(index);
            let count = self.count(unit, now);
            let comparator = &mut self.comparators[index];
            if let Some(next) = comparator.next {
                let late = count.wrapping_sub(next) & COUNT_MASK;
                if late < PASSED_WINDOW {
                    self.int_raw |= 1 << index;
                    // In period mode the alarms that were missed are one level, not a queue: the
                    // next one is the first multiple of the period after the present count.
                    comparator.next = (comparator.conf & TARGET_PERIOD_MODE != 0
                        && comparator.loaded_period != 0)
                        .then(|| {
                            let periods = late / comparator.loaded_period + 1;
                            next.wrapping_add(periods * comparator.loaded_period) & COUNT_MASK
                        });
                }
            }
            let ahead = comparator
                .next
                .map(|next| next.wrapping_sub(count) & COUNT_MASK);
            // A stopped counter reaches nothing: the comparator waits for it to be started.
            if let Some(at) =
                ahead.and_then(|ahead| self.units[unit].counter.instant_in(ahead, now))
            {
                earliest = Some(earliest.map_or(at, |e| e.min(at)));
            }
        }
        if let Some(at) = earliest {
            self.wake.arm(ctx, at, Event::Systimer);
        }
        self.drive(ctx);
    }

    /// Each comparator drives a level interrupt: `INT_RAW & INT_ENA` (TRM §10.4.4).
    fn drive(&self, ctx: &mut Ctx) {
        for (index, source) in SOURCES.into_iter().enumerate() {
            ctx.irq(source, self.int_raw & self.int_ena & (1 << index) != 0);
        }
    }

    /// The device time of a wake-up has come.
    pub(crate) fn wake(&mut self, ctx: &mut Ctx) {
        self.evaluate(ctx);
    }

    fn write_conf(&mut self, ctx: &mut Ctx, value: u32) {
        let now = ctx.now();
        let previous = self.conf;
        self.conf = value & CONF_WRITABLE;
        // A counter that is stopped or started keeps its count.
        for (unit, work_en) in (0..UNITS).zip([UNIT0_WORK_EN, UNIT0_WORK_EN >> 1]) {
            self.units[unit].counter.run(self.conf & work_en != 0, now);
        }
        for comparator in 0..COMPARATORS {
            let bit = TARGET0_WORK_EN >> comparator;
            if self.conf & bit != 0 && previous & bit == 0 {
                self.start(comparator, now);
            }
        }
        self.evaluate(ctx);
    }
}

impl Peripheral<Ctx> for Systimer {
    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
        if let Some((unit, hi)) = pair_in(offset, UNIT0_LOAD_HI, UNIT1_LOAD_LO) {
            return Some(half_of(self.units[unit].load, hi));
        }
        if let Some((comparator, hi)) = pair_in(offset, TARGET0_HI, TARGET2_LO) {
            return Some(half_of(self.comparators[comparator].target, hi));
        }
        if let Some((unit, hi)) = pair_in(offset, UNIT0_VALUE_HI, UNIT1_VALUE_LO) {
            return Some(half_of(self.units[unit].latch, hi));
        }
        if (TARGET0_CONF..=TARGET2_CONF).contains(&offset) {
            return Some(self.comparators[((offset - TARGET0_CONF) / 4) as usize].conf);
        }
        Some(match offset {
            CONF => self.conf,
            UNIT0_OP | UNIT1_OP => {
                if self.units[((offset - UNIT0_OP) / 4) as usize].valid {
                    OP_VALUE_VALID
                } else {
                    0
                }
            }
            INT_ENA => self.int_ena,
            INT_RAW => self.int_raw,
            INT_ST => self.int_raw & self.int_ena,
            // Write-only triggers read 0.
            INT_CLR | UNIT0_LOAD | UNIT1_LOAD => 0,
            offset if (COMP0_LOAD..=COMP2_LOAD).contains(&offset) => 0,
            DATE => DATE_VALUE,
            _ => return None,
        })
    }

    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        let now = ctx.now();
        if let Some((unit, hi)) = pair_in(offset, UNIT0_LOAD_HI, UNIT1_LOAD_LO) {
            self.units[unit].load = replace_half(self.units[unit].load, hi, value, HI_MASK);
            return true;
        }
        if let Some((comparator, hi)) = pair_in(offset, TARGET0_HI, TARGET2_LO) {
            let target = &mut self.comparators[comparator].target;
            *target = replace_half(*target, hi, value, HI_MASK);
            return true;
        }
        if (TARGET0_CONF..=TARGET2_CONF).contains(&offset) {
            let comparator = ((offset - TARGET0_CONF) / 4) as usize;
            let previous = self.comparators[comparator].conf;
            self.comparators[comparator].conf = value & TARGET_CONF_WRITABLE;
            // The mode and the counter act as soon as they are written: ESP-IDF sets
            // `PERIOD_MODE` after `COMPx_LOAD` (`systimer_hal_select_alarm_mode`). The period
            // itself waits for `COMPx_LOAD`.
            if (previous ^ value) & (TARGET_PERIOD_MODE | TARGET_UNIT_SEL) != 0 {
                self.start(comparator, now);
            }
            self.evaluate(ctx);
            return true;
        }
        if (COMP0_LOAD..=COMP2_LOAD).contains(&offset) {
            if value & LOAD_TRIGGER != 0 {
                let comparator = ((offset - COMP0_LOAD) / 4) as usize;
                let c = &mut self.comparators[comparator];
                c.loaded_target = c.target;
                c.loaded_period = u64::from(c.conf & TARGET_PERIOD_MASK);
                self.start(comparator, now);
                self.evaluate(ctx);
            }
            return true;
        }
        match offset {
            CONF => self.write_conf(ctx, value),
            UNIT0_OP | UNIT1_OP => {
                if value & OP_UPDATE != 0 {
                    let unit = ((offset - UNIT0_OP) / 4) as usize;
                    // ASSUMPTION: the latch is valid at once. The TRM gives no latency for
                    // `VALUE_VALID` (TW §A.3, §I question 6); ESP-IDF polls it without a timeout.
                    self.units[unit].latch = self.count(unit, now);
                    self.units[unit].valid = true;
                }
            }
            UNIT0_LOAD | UNIT1_LOAD => {
                if value & LOAD_TRIGGER != 0 {
                    let unit = ((offset - UNIT0_LOAD) / 4) as usize;
                    // "The current count value will be changed immediately" (TRM §10.5).
                    let load = self.units[unit].load;
                    self.units[unit].counter.set(load, now);
                    self.evaluate(ctx);
                }
            }
            INT_ENA => {
                self.int_ena = value & INT_MASK;
                self.drive(ctx);
            }
            INT_CLR => {
                self.int_raw &= !value;
                self.drive(ctx);
            }
            // Read-only: the write is reported as unmodelled.
            _ => return false,
        }
        true
    }
}
```

- [ ] **Step 6: Wire it: the event, the address range, the construction at a device time**

```diff
--- a/crates/soc-esp32c3/src/ctx.rs
+++ b/crates/soc-esp32c3/src/ctx.rs
@@ -46,6 +46,8 @@ pub enum Event {
     MmuFault { vaddr: u32, content: u32 },
     /// To `EXTMEM`: a store reached a flash window.
     FlashWindowStore { instruction_bus: bool },
+    /// Wake-up of the SYSTIMER: one of its comparators may have an alarm due.
+    Systimer,
 }
 
 /// The wake-up a device keeps for its next deadline.
```

```diff
--- a/crates/soc-esp32c3/src/periph/mod.rs
+++ b/crates/soc-esp32c3/src/periph/mod.rs
@@ -7,6 +7,7 @@ use emu_core::Peripheral;
 pub mod ana_i2c;
 pub mod apb_ctrl;
 pub mod assist_debug;
+pub mod counter;
 pub mod efuse;
 pub mod extmem;
 pub mod gpio;
@@ -17,6 +18,7 @@ pub mod rtc_cntl;
 pub mod sha;
 pub mod spi_mem;
 pub mod system;
+pub mod systimer;
 pub mod uart;
 pub mod usb_serial_jtag;
 
@@ -33,6 +35,7 @@ pub use rtc_cntl::RtcCntl;
 pub use sha::Sha;
 pub use spi_mem::{Spi0, Spi1};
 pub use system::System;
+pub use systimer::Systimer;
 pub use uart::Uart;
 pub use usb_serial_jtag::UsbSerialJtag;
 
@@ -312,6 +315,9 @@ const ANA_I2C: (u32, u32) = (ana_i2c::ANA_I2C_BASE, ana_i2c::ANA_I2C_BASE + 0x10
 /// `DR_REG_TIMERGROUP0_BASE` (ESP-IDF `reg_base.h`; report BC §A.2).
 pub(crate) const TIMG0_BASE: u32 = 0x6001_F000;
 const TIMG0: (u32, u32) = (TIMG0_BASE, 0x6002_0000);
+/// `DR_REG_SYSTIMER_BASE` (ESP-IDF `reg_base.h`; report TW §A.1).
+pub(crate) const SYSTIMER_BASE: u32 = 0x6002_3000;
+const SYSTIMER: (u32, u32) = (SYSTIMER_BASE, 0x6002_4000);
 /// `DR_REG_APB_SARADC_BASE` (ESP-IDF `reg_base.h`; report BC §A.2).
 pub(crate) const APB_SARADC_BASE: u32 = 0x6004_0000;
 const APB_SARADC: (u32, u32) = (APB_SARADC_BASE, 0x6004_1000);
@@ -381,6 +387,7 @@ pub struct Peripherals {
     pub apb_ctrl: ApbCtrl,
     pub ana_i2c: AnaI2c,
     pub timg0: AcceptedBlock,
+    pub systimer: Systimer,
     pub apb_saradc: AcceptedBlock,
     /// Four isolated words: its base is 0, so its offsets are absolute addresses.
     pub radio_power: AcceptedBlock,
@@ -390,9 +397,11 @@ pub struct Peripherals {
 }
 
 impl Peripherals {
+    /// The peripherals after a power-on, at the device time of `ctx`: a device that has a first
+    /// deadline schedules it there.
     #[must_use]
-    pub fn new(config: &SocConfig, flash: Box<dyn FlashDevice>) -> Peripherals {
-        Peripherals {
+    pub fn new(config: &SocConfig, flash: Box<dyn FlashDevice>, ctx: &mut Ctx) -> Peripherals {
+        let mut periph = Peripherals {
             gpio: Gpio::new(config.strapping),
             io_mux: IoMux::new(),
             intmatrix: IntMatrix::new(),
@@ -409,12 +418,15 @@ impl Peripherals {
             apb_ctrl: ApbCtrl::new(config.rng_seed),
             ana_i2c: AnaI2c::new(),
             timg0: AcceptedBlock::new(TIMG0_BASE, &TIMG0_ACCEPTED),
+            systimer: Systimer::new(ctx.now()),
             apb_saradc: AcceptedBlock::new(APB_SARADC_BASE, &APB_SARADC_ACCEPTED),
             radio_power: AcceptedBlock::new(0, &RADIO_POWER_ACCEPTED),
             uart0: Uart::new(UART0_BASE),
             uart1: Uart::new(UART1_BASE),
             usb_serial_jtag: UsbSerialJtag::new(),
-        }
+        };
+        periph.systimer.wake(ctx);
+        periph
     }
 
     /// Calls the device an event is for: a wake-up whose time has come, or what another part of the
@@ -425,6 +437,7 @@ impl Peripherals {
             Event::FlashWindowStore { instruction_bus } => {
                 self.extmem.latch_store(ctx, instruction_bus);
             }
+            Event::Systimer => self.systimer.wake(ctx),
         }
     }
 
@@ -463,6 +476,8 @@ impl Peripherals {
             Some((&mut self.ana_i2c, ana_i2c::ANA_I2C_BASE))
         } else if within(TIMG0) {
             Some((&mut self.timg0, TIMG0_BASE))
+        } else if within(SYSTIMER) {
+            Some((&mut self.systimer, SYSTIMER_BASE))
         } else if within(APB_SARADC) {
             Some((&mut self.apb_saradc, APB_SARADC_BASE))
         } else if within(UART0) {
@@ -595,12 +610,13 @@ mod tests {
         assert_eq!(table.lines().count(), rows.len() + 2);
     }
 
-    fn peripherals() -> Peripherals {
+    fn peripherals(ctx: &mut Ctx) -> Peripherals {
         Peripherals::new(
             &SocConfig::default(),
             Box::new(crate::flash::NorFlash::blank(
                 crate::flash::FlashConfig::default(),
             )),
+            ctx,
         )
     }
 
@@ -610,7 +626,7 @@ mod tests {
     #[test]
     fn every_row_of_the_table_is_routed_to_a_model_that_accepts_a_write_there() {
         let ctx = &mut Ctx::default();
-        let mut p = peripherals();
+        let mut p = peripherals(ctx);
         let rows = accepted_without_effect();
         assert_ne!(rows, Vec::new(), "the table is not empty");
         for row in rows {
@@ -644,12 +660,8 @@ mod tests {
 
     #[test]
     fn the_mmu_table_sits_right_after_the_cache_registers() {
-        let mut p = Peripherals::new(
-            &SocConfig::default(),
-            Box::new(crate::flash::NorFlash::blank(
-                crate::flash::FlashConfig::default(),
-            )),
-        );
+        let ctx = &mut Ctx::default();
+        let mut p = peripherals(ctx);
         let base_of = |p: &mut Peripherals, addr: u32| p.device(addr).map(|(_, base)| base);
         assert_eq!(base_of(&mut p, 0x600C_4FFC), Some(EXTMEM_BASE));
         assert_eq!(base_of(&mut p, 0x600C_5000), Some(mmu::MMU_TABLE_BASE));
@@ -671,12 +683,7 @@ mod tests {
     #[test]
     fn the_sha_block_is_routed_and_its_refusals_reach_the_relay() {
         let ctx = &mut Ctx::default();
-        let mut p = Peripherals::new(
-            &SocConfig::default(),
-            Box::new(crate::flash::NorFlash::blank(
-                crate::flash::FlashConfig::default(),
-            )),
-        );
+        let mut p = peripherals(ctx);
         let dma_start = svd_regs::esp32c3::addr("SHA", "DMA_START");
         let (device, base) = p.device(dma_start).expect("SHA is routed");
         assert_eq!(base, SHA_BASE);
@@ -689,12 +696,7 @@ mod tests {
     #[test]
     fn storage_only_blocks_answer_exactly_their_listed_registers() {
         let ctx = &mut Ctx::default();
-        let mut p = Peripherals::new(
-            &SocConfig::default(),
-            Box::new(crate::flash::NorFlash::blank(
-                crate::flash::FlashConfig::default(),
-            )),
-        );
+        let mut p = peripherals(ctx);
         let mut read = |addr: u32| {
             p.device(addr)
                 .and_then(|(device, base)| device.read(ctx, addr - base))
@@ -738,7 +740,7 @@ mod tests {
     #[test]
     fn storage_only_blocks_read_back_what_was_written_through_the_device_tree() {
         let ctx = &mut Ctx::default();
-        let mut p = peripherals();
+        let mut p = peripherals(ctx);
         let svd = svd_regs::esp32c3::addr;
         for (addr, value) in [
             (svd("TIMG0", "WDTCONFIG0"), 0x8000_0000),
@@ -757,12 +759,7 @@ mod tests {
     #[test]
     fn a_software_reset_request_reaches_the_relay() {
         let ctx = &mut Ctx::default();
-        let mut p = Peripherals::new(
-            &SocConfig::default(),
-            Box::new(crate::flash::NorFlash::blank(
-                crate::flash::FlashConfig::default(),
-            )),
-        );
+        let mut p = peripherals(ctx);
         let options0 = svd_regs::esp32c3::addr("RTC_CNTL", "OPTIONS0");
         let (device, base) = p.device(options0).expect("RTC_CNTL is routed");
         assert!(device.write(ctx, options0 - base, 1 << 31));
@@ -780,12 +777,8 @@ mod tests {
 
     #[test]
     fn the_page_shared_by_rtc_cntl_and_the_efuse_splits_at_the_last_two_kib() {
-        let mut p = Peripherals::new(
-            &SocConfig::default(),
-            Box::new(crate::flash::NorFlash::blank(
-                crate::flash::FlashConfig::default(),
-            )),
-        );
+        let ctx = &mut Ctx::default();
+        let mut p = peripherals(ctx);
         let base_of = |p: &mut Peripherals, addr: u32| p.device(addr).map(|(_, base)| base);
         assert_eq!(base_of(&mut p, 0x6000_8000), Some(RTC_CNTL_BASE));
         assert_eq!(base_of(&mut p, 0x6000_87FC), Some(RTC_CNTL_BASE));
```

```diff
--- a/crates/soc-esp32c3/src/bus.rs
+++ b/crates/soc-esp32c3/src/bus.rs
@@ -101,13 +101,15 @@ const fn size_mask(size: AccessSize) -> u32 {
 impl SocBus {
     #[must_use]
     pub fn new(rom: RomImage, flash: Box<dyn FlashDevice>, config: &SocConfig) -> SocBus {
+        let mut ctx = Ctx::new(config.strict_mmio);
+        let periph = Peripherals::new(config, flash, &mut ctx);
         SocBus {
             rom,
             sram: Ram::new(SRAM_DBUS_BASE, SRAM_SIZE as usize),
             sram0: Ram::new(SRAM0_BASE, SRAM0_SIZE as usize),
             rtc: Ram::new(RTC_RAM_BASE, RTC_RAM_SIZE as usize),
-            periph: Peripherals::new(config, flash),
-            ctx: Ctx::new(config.strict_mmio),
+            periph,
+            ctx,
         }
     }
 
```

- [ ] **Step 7: Run the suite**

Run: `cargo test --workspace > /tmp/t7.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t7.log`
Expected: `exit=0`, `625`.

- [ ] **Step 8: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/soc-esp32c3/src
/usr/bin/git commit -m "feat(soc): the SYSTIMER, on counters that are a value at an instant

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3/src
```

---

## Task 8: Interrupt delivery, software interrupts, and `wfi` that sleeps to the next wake-up

The machine serves the context after each instruction: it delivers the wake-ups that are due, asks the controller which line it selects, presents it to the hart after its latency, and, when the hart waits, moves device time to the next wake-up.

**Files:**
- Modify: `crates/soc-esp32c3/src/machine.rs`, `crates/soc-esp32c3/src/periph/system.rs`, `crates/soc-esp32c3/src/test_support.rs`, `crates/emu-cli/src/run.rs`, `crates/emu-cli/src/exit.rs`

**Interfaces:**
- Consumes: `Ctx::{wake_due, take_due, is_flagged, take_clock_written, take_interrupts_changed, interrupts_changed, sources, next_wake, sleep_until, now, clock}` (Tasks 2, 3); `SocBus::deliver` (Task 4); `IntMatrix::{resolve, claim}` (Task 5); `InterruptModel::Controller`, `Hart::set_external_interrupt`, `StepOutcome::{Waiting, Interrupted { cause }}` (Task 6); `Systimer` (Task 7); `test_support::{T0, T1}`, `cpu_rv32::asm::{lui, addi, sw}`.
- Produces: `StopReason::Asleep`; `machine::{SLEEP_SLICE: u64 = 160_000_000, MAX_FRUITLESS_WAKES: u32 = 100_000, INTERRUPT_LATENCY_CYCLES: u64 = 4}`; `StopReason::Idle` now means that nothing is pending and nothing is scheduled; `SYSTEM.CPU_INTR_FROM_CPU_0..3` are modelled (bit 0 of each is the level of sources 50 to 53); `test_support::store_register(addr: u32, value: u32) -> [u32; 4]`; the CLI goes on after `Asleep` and words exit code 4 as "the CPU waits for an interrupt and none can come".

- [ ] **Step 1: Write the failing tests**

`SYSTEM` (IR §3.3: the application stores 1 then 0 in `CPU_INTR_FROM_CPU_0`):

```diff
--- a/crates/soc-esp32c3/src/periph/system.rs
+++ b/crates/soc-esp32c3/src/periph/system.rs
@@ -213,4 +247,20 @@ mod tests {
             "a clock gate is not the CPU clock"
         );
     }
+
+    #[test]
+    fn a_software_interrupt_register_is_the_level_of_its_source() {
+        // IR §3.3: source 50 is asserted by a store of 1 and cleared by a store of 0.
+        let ctx = &mut Ctx::default();
+        let mut sys = System::new();
+        assert_eq!(sys.read(ctx, CPU_INTR_FROM_CPU_0), Some(0));
+        assert!(sys.write(ctx, CPU_INTR_FROM_CPU_0, 1));
+        assert_eq!(sys.read(ctx, CPU_INTR_FROM_CPU_0), Some(1));
+        assert_eq!(ctx.sources(), 1 << 50);
+        assert!(sys.write(ctx, CPU_INTR_FROM_CPU_0 + 12, 0xFFFF_FFFF));
+        assert_eq!(sys.read(ctx, CPU_INTR_FROM_CPU_0 + 12), Some(1), "one bit");
+        assert_eq!(ctx.sources(), (1 << 50) | (1 << 53));
+        sys.write(ctx, CPU_INTR_FROM_CPU_0, 0);
+        assert_eq!(ctx.sources(), 1 << 53);
+    }
 }
```

A helper for programs that store a 32-bit value in a register:

```diff
--- a/crates/soc-esp32c3/src/test_support.rs
+++ b/crates/soc-esp32c3/src/test_support.rs
@@ -335,3 +335,21 @@ pub fn load_register(addr: u32) -> [u32; 2] {
     let offset = addr.wrapping_sub(page).cast_signed();
     [asm::lui(T0, page), asm::lw(T1, T0, offset)]
 }
+
+/// Stores `value` in the register at `addr`, through `T0` and `T1`.
+#[must_use]
+pub fn store_register(addr: u32, value: u32) -> [u32; 4] {
+    // `addi` and `sw` sign-extend their 12 bits: the upper part is rounded so the sum is the word.
+    let split = |word: u32| {
+        let upper = word.wrapping_add(0x800) & 0xFFFF_F000;
+        (upper, word.wrapping_sub(upper).cast_signed())
+    };
+    let (page, offset) = split(addr);
+    let (upper, lower) = split(value);
+    [
+        asm::lui(T0, page),
+        asm::lui(T1, upper),
+        asm::addi(T1, T1, lower),
+        asm::sw(T1, T0, offset),
+    ]
+}
```

The machine. `route` does what `esp_intr_alloc` does (IR §3.2); the latency is the "4 cycles" of the TRM (IR §2.7); the alarms are the SYSTIMER's (16 MHz: 16 384 counts are 1.024 ms):

```diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -221,7 +337,8 @@ mod tests {
     use crate::memmap::ROM_BASE;
     use crate::memmap::SRAM_DBUS_BASE;
     use crate::test_support::{
-        HANG, T0, T1, flat_rom, load_register, print_program as print, unmodeled_register,
+        HANG, T0, T1, flat_rom, load_register, print_program as print, store_register,
+        unmodeled_register,
     };
     use cpu_rv32::asm::{addi, csrrs, csrrwi, jal, lui, lw, sw, wfi};
     use cpu_rv32::csr_esp32c3::{PCCR_USER, PCER_USER, PCMR_USER};
@@ -440,6 +557,11 @@ mod tests {
     fn wfi_with_nothing_to_wake_it_stops_as_idle() {
         let mut m = machine(&[wfi(), HANG]);
         assert_eq!(m.run_until(Cycles(10_000)), StopReason::Idle);
+        assert_eq!(
+            m.device_time_nanos(),
+            150,
+            "`wfi` alone: 3 cycles at 20 MHz, no jump in time"
+        );
     }
 
     #[test]
@@ -699,4 +821,175 @@ mod tests {
         };
         assert!(count > 5, "one report per execution: {count}");
     }
+
+    /// `addi x0, x0, 0`: one cycle, no effect.
+    const NOP: u32 = 0x0000_0013;
+    const MSTATUS: u16 = 0x300;
+    const MSTATUS_MIE: u8 = 1 << 3;
+
+    fn reg(peripheral: &str, register: &str) -> u32 {
+        svd_regs::esp32c3::addr(peripheral, register)
+    }
+
+    /// `body` at the reset vector, then the vector table at [`VECTORS`]: every entry parks.
+    fn with_vectors(body: &[u32]) -> Vec<u32> {
+        let table_index = ((VECTORS - ROM_BASE) / 4) as usize;
+        assert!(
+            body.len() <= table_index,
+            "the body overlaps the vector table"
+        );
+        let mut program = body.to_vec();
+        program.resize(table_index, HANG);
+        program.extend([HANG; 32]);
+        program
+    }
+
+    /// Routes interrupt source `map_register` to CPU `line` at priority 1 and enables the line,
+    /// as `esp_intr_alloc` does (IR §3.2).
+    fn route(map_register: &str, line: u32) -> Vec<u32> {
+        let mut program = Vec::new();
+        program.extend(store_register(reg("INTERRUPT_CORE0", map_register), line));
+        program.extend(store_register(
+            reg("INTERRUPT_CORE0", "CPU_INT_PRI_0") + 4 * line,
+            1,
+        ));
+        program.extend(store_register(
+            reg("INTERRUPT_CORE0", "CPU_INT_ENABLE"),
+            1 << line,
+        ));
+        program
+    }
+
+    #[test]
+    fn a_software_interrupt_is_taken_four_cpu_cycles_after_it_becomes_pending() {
+        // IR §3.3: the application raises `CPU_INTR_FROM_CPU_0` (source 50) routed to line 3;
+        // IR §2.7: "the interrupt controller ... takes 4 cycles".
+        let mut body = Vec::new();
+        body.extend([lui(T0, VECTORS), cpu_rv32::asm::csrrw(0, MTVEC, T0)]);
+        body.extend(route("CPU_INTR_FROM_CPU_0_MAP", 3));
+        body.push(csrrwi(0, MSTATUS, MSTATUS_MIE));
+        body.extend(store_register(reg("SYSTEM", "CPU_INTR_FROM_CPU_0"), 1));
+        let raised_at = ROM_BASE + 4 * u32::try_from(body.len()).unwrap();
+        body.extend([NOP; 16]);
+        let mut m = machine(&with_vectors(&body));
+        assert_eq!(m.run_until(Cycles(2_000)), StopReason::CycleBudget);
+        assert_eq!(m.hart.csr.mcause, 0x8000_0003);
+        assert_eq!(
+            m.hart.csr.mepc,
+            raised_at + 4 * 4,
+            "four one-cycle instructions ran between the store and the interrupt"
+        );
+        assert_eq!(m.pc(), VECTORS + 4 * 3, "the entry of line 3 parks");
+        assert_eq!(
+            bus_word(&mut m, reg("INTERRUPT_CORE0", "CPU_INT_EIP_STATUS")),
+            1 << 3,
+            "the source is a level: the line stays pending until the handler clears it"
+        );
+    }
+
+    #[test]
+    fn a_line_under_the_threshold_or_disabled_is_not_taken() {
+        let mut body = Vec::new();
+        body.extend([lui(T0, VECTORS), cpu_rv32::asm::csrrw(0, MTVEC, T0)]);
+        body.extend(route("CPU_INTR_FROM_CPU_0_MAP", 3));
+        // What a critical section does: `CPU_INT_THRESH` above the priority of the line (IR §3.3).
+        body.extend(store_register(reg("INTERRUPT_CORE0", "CPU_INT_THRESH"), 2));
+        body.push(csrrwi(0, MSTATUS, MSTATUS_MIE));
+        body.extend(store_register(reg("SYSTEM", "CPU_INTR_FROM_CPU_0"), 1));
+        body.extend([NOP; 16]);
+        body.push(HANG);
+        let hang = ROM_BASE + 4 * u32::try_from(body.len() - 1).unwrap();
+        let mut m = machine(&with_vectors(&body));
+        assert_eq!(m.run_until(Cycles(2_000)), StopReason::CycleBudget);
+        assert_eq!(m.pc(), hang, "masked by the threshold");
+        assert_eq!(m.hart.csr.mcause, 0);
+    }
+
+    #[test]
+    fn wfi_sleeps_to_the_alarm_and_resumes_without_a_trap_when_interrupts_are_masked() {
+        // A one-shot alarm of the SYSTIMER 16 384 counts ahead (1.024 ms at 16 MHz), on line 4.
+        let mut body = Vec::new();
+        body.extend(route("SYSTIMER_TARGET0_INT_MAP", 4));
+        body.extend(store_register(reg("SYSTIMER", "TARGET0_LO"), 16_384));
+        body.extend(store_register(reg("SYSTIMER", "COMP0_LOAD"), 1));
+        body.extend(store_register(reg("SYSTIMER", "INT_ENA"), 1));
+        // `CONF`: its reset value with `TARGET0_WORK_EN`, bit 24 (`systimer_ll_enable_alarm`).
+        body.extend(store_register(
+            reg("SYSTIMER", "CONF"),
+            0x4600_0000 | (1 << 24),
+        ));
+        body.extend([wfi(), addi(1, 0, 1), HANG]);
+        let mut m = machine(&body);
+        assert_eq!(m.run_until(Cycles(2_000)), StopReason::CycleBudget);
+        assert_eq!(
+            m.hart.x[1], 1,
+            "`mstatus.MIE` is 0: execution goes on after `wfi`"
+        );
+        assert_eq!(m.hart.csr.mcause, 0, "no trap");
+        // Awake, a cycle lasts 50 ns; the rest of the time went by in `wfi`, from a few
+        // microseconds after reset to the alarm, 1.024 ms after the counter started.
+        let slept_ns = m.device_time_nanos() - 50 * m.cycles().get();
+        assert!(
+            (1_020_000..=1_024_200).contains(&slept_ns),
+            "slept {slept_ns} ns"
+        );
+        assert!(
+            (2_000..2_003).contains(&m.cycles().get()),
+            "the cycle counter stands still in `wfi`: {}",
+            m.cycles().get()
+        );
+    }
+
+    #[test]
+    fn a_cpu_that_sleeps_while_a_device_counts_gives_control_back_after_a_second() {
+        // A one-shot alarm two seconds ahead (32 000 000 counts at 16 MHz), its interrupt not
+        // enabled: nothing will wake the CPU, and time must still pass.
+        let mut body = Vec::new();
+        body.extend(store_register(reg("SYSTIMER", "TARGET0_LO"), 32_000_000));
+        body.extend(store_register(reg("SYSTIMER", "COMP0_LOAD"), 1));
+        body.extend(store_register(
+            reg("SYSTIMER", "CONF"),
+            0x4600_0000 | (1 << 24),
+        ));
+        body.extend([wfi(), HANG]);
+        let mut m = machine(&body);
+        assert_eq!(m.run_until(Cycles(10_000)), StopReason::Asleep);
+        assert_eq!(
+            m.device_time_nanos(),
+            2_000_000_000,
+            "slept to the alarm in one jump"
+        );
+        assert_eq!(
+            m.run_until(Cycles(10_000)),
+            StopReason::Idle,
+            "the alarm has fired and nothing else is scheduled"
+        );
+    }
+
+    #[test]
+    fn a_storm_of_wake_ups_without_an_interrupt_gives_control_back() {
+        // A period alarm at every count of the 16 MHz counter, its interrupt not enabled
+        // (`TARGET0_CONF`: bit 30 `PERIOD_MODE`, period 1): the SYSTIMER is called 100 000 times
+        // in 6.25 ms of device time and the CPU never wakes.
+        let mut body = Vec::new();
+        body.extend(store_register(
+            reg("SYSTIMER", "TARGET0_CONF"),
+            (1 << 30) | 1,
+        ));
+        body.extend(store_register(reg("SYSTIMER", "COMP0_LOAD"), 1));
+        body.extend(store_register(
+            reg("SYSTIMER", "CONF"),
+            0x4600_0000 | (1 << 24),
+        ));
+        body.extend([wfi(), HANG]);
+        let mut m = machine(&body);
+        assert_eq!(m.run_until(Cycles(10_000)), StopReason::Asleep);
+        let first = m.device_time_nanos();
+        assert!(
+            (6_250_000..6_300_000).contains(&first),
+            "MAX_FRUITLESS_WAKES counts of 62.5 ns: {first} ns"
+        );
+        assert_eq!(m.run_until(Cycles(10_000)), StopReason::Asleep, "and again");
+        assert!(m.device_time_nanos() > first);
+    }
 }
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p soc-esp32c3 --lib > /tmp/t8.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, compile errors: no variant `Asleep`, `cannot find value 'CPU_INTR_FROM_CPU_0'` (the test of `system.rs`). With the variant declared and Step 3 applied, four machine tests fail: `a_software_interrupt_is_taken_four_cpu_cycles_after_it_becomes_pending` (`mcause` is 0: nothing is delivered) and the three sleep tests, which return `Idle`. `a_line_under_the_threshold_or_disabled_is_not_taken` passes already: it asserts that nothing is delivered, and nothing is yet.

- [ ] **Step 3: The software interrupt registers**

```diff
--- a/crates/soc-esp32c3/src/periph/system.rs
+++ b/crates/soc-esp32c3/src/periph/system.rs
@@ -1,11 +1,26 @@
-//! `SYSTEM`: clock selection, peripheral clock gates and resets. Source: report §E.1.
-//! The CPU clock selection is read by `crate::clock`; gates and resets are stored.
-use crate::ctx::Ctx;
+//! `SYSTEM`: clock selection, peripheral clock gates and resets, software interrupts. Sources:
+//! reports BC §E.1, IR §4.1 and §5 (ESP-IDF `system_reg.h`).
+//! The CPU clock selection is read by `crate::clock`; gates and resets are stored; the four
+//! software interrupt registers are interrupt sources.
+use crate::ctx::{Ctx, Source};
 use crate::periph::{Accepted, SYSTEM_BASE, Storage};
 use emu_core::Peripheral;
 
 const SYSCLK_CONF: u32 = svd_regs::esp32c3::addr("SYSTEM", "SYSCLK_CONF") - SYSTEM_BASE;
 const CPU_PER_CONF: u32 = svd_regs::esp32c3::addr("SYSTEM", "CPU_PER_CONF") - SYSTEM_BASE;
+/// `CPU_INTR_FROM_CPU_0..3`: bit 0 of each register is the level of an interrupt source, set and
+/// cleared by software (`esp_crosscore_int_send` stores 1 at `APP@0x40381bc6`, its handler stores
+/// 0 at `APP@0x40381b50`: report IR §3.3, §5).
+const CPU_INTR_FROM_CPU_0: u32 =
+    svd_regs::esp32c3::addr("SYSTEM", "CPU_INTR_FROM_CPU_0") - SYSTEM_BASE;
+const SOFTWARE_INTERRUPTS: usize = 4;
+const SOFTWARE_SOURCES: [Source; SOFTWARE_INTERRUPTS] = [
+    Source::of("CPU_INTR_FROM_CPU_0_MAP"),
+    Source::of("CPU_INTR_FROM_CPU_1_MAP"),
+    Source::of("CPU_INTR_FROM_CPU_2_MAP"),
+    Source::of("CPU_INTR_FROM_CPU_3_MAP"),
+];
+const SOFTWARE_INTERRUPT_LEVEL: u32 = 1;
 /// `CPU_PERI_CLK_EN`, `CPU_PERI_RST_EN`, `PERIP_CLK_EN0/1`, `PERIP_RST_EN0/1`: gates and reset pulses the
 /// ROM toggles around each peripheral it sets up. Nothing acts on them yet. `CPU_PER_CONF` is stored
 /// as software wrote it; only its `CPUPERIOD_SEL` field acts, through `crate::clock`. `MEM_PD_MASK`
@@ -85,6 +100,8 @@ const XTAL_FREQ_MHZ: u32 = crate::clock::XTAL_MHZ;
 pub struct System {
     sysclk_writable: u32,
     registers: Storage,
+    /// `CPU_INTR_FROM_CPU_0..3`.
+    software_interrupts: [u32; SOFTWARE_INTERRUPTS],
 }
 
 impl System {
@@ -93,6 +110,7 @@ impl System {
         System {
             sysclk_writable: SYSCLK_RESET,
             registers: Storage::accepted(SYSTEM_BASE, &ACCEPTED),
+            software_interrupts: [0; SOFTWARE_INTERRUPTS],
         }
     }
 
@@ -108,6 +126,11 @@ impl System {
     }
 }
 
+fn software_interrupt(offset: u32) -> Option<usize> {
+    let index = usize::try_from(offset.checked_sub(CPU_INTR_FROM_CPU_0)? / 4).ok()?;
+    (index < SOFTWARE_INTERRUPTS).then_some(index)
+}
+
 impl Default for System {
     fn default() -> Self {
         System::new()
@@ -119,10 +142,21 @@ impl Peripheral<Ctx> for System {
         if offset == SYSCLK_CONF {
             return Some(self.clock_registers().0);
         }
+        if let Some(index) = software_interrupt(offset) {
+            return Some(self.software_interrupts[index]);
+        }
         self.registers.read(offset)
     }
 
     fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
+        if let Some(index) = software_interrupt(offset) {
+            self.software_interrupts[index] = value & SOFTWARE_INTERRUPT_LEVEL;
+            ctx.irq(
+                SOFTWARE_SOURCES[index],
+                value & SOFTWARE_INTERRUPT_LEVEL != 0,
+            );
+            return true;
+        }
         match offset {
             SYSCLK_CONF => {
                 self.sysclk_writable = value & SYSCLK_WRITABLE;
```

- [ ] **Step 4: The run loop**

```diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -7,7 +7,8 @@ use crate::flash::{FlashDevice, FlashImageError, NorFlash};
 use crate::memmap::RESET_PC;
 use crate::rom::RomImage;
 use cpu_rv32::{
-    DatasheetTiming, Exception, Hart, HartConfig, MisalignedPolicy, StepOutcome, VendorCsrs,
+    DatasheetTiming, Exception, Hart, HartConfig, InterruptModel, MisalignedPolicy, StepOutcome,
+    VendorCsrs,
 };
 use emu_core::{Cycles, Frequency, TimingModel, TraceHash};
 
@@ -26,10 +27,29 @@ pub enum StopReason {
     /// peripheral register with an 8- or 16-bit store, asked for an operation the emulator cannot
     /// perform faithfully, or used flash it had not mapped.
     StrictMmio(Diagnostic),
-    /// The CPU is in `wfi` and nothing can raise an interrupt yet (interrupt sources arrive in SP1b-3).
+    /// The CPU waits for an interrupt and none can come: no interrupt line is pending and no
+    /// device has anything scheduled.
     Idle,
+    /// The CPU has been waiting for an interrupt through [`SLEEP_SLICE`] of device time, or was
+    /// called [`MAX_FRUITLESS_WAKES`] times without one, since the call began. Device time goes
+    /// on while the CPU sleeps and its cycle counter does not: a caller that only watches the
+    /// cycle limit gets control back here, and calls again to go on.
+    Asleep,
 }
 
+/// Device time a call to [`Esp32C3::run_until`] spends asleep before it returns
+/// [`StopReason::Asleep`]: one second, in cycles of the time base.
+pub const SLEEP_SLICE: u64 = 160_000_000;
+/// Wake-ups without an interrupt one call goes through before it returns [`StopReason::Asleep`].
+pub const MAX_FRUITLESS_WAKES: u32 = 100_000;
+/// CPU clock cycles between the instant the controller selects a line and the instant the CPU
+/// can take it: "the interrupt controller ... takes 4 cycles" (TRM §1.6.1; report IR §2.7).
+///
+/// ASSUMPTION: only a line that becomes selectable waits; a line that is masked or released
+/// disappears at once. The TRM gives one figure and no direction; the SP1b-4 lockstep compares
+/// with an oracle that has no latency at all, and needs this to be configurable.
+pub const INTERRUPT_LATENCY_CYCLES: u64 = 4;
+
 /// An ESP32-C3: one hart, the bus with everything it reaches, and the model that prices each
 /// instruction. It performs no host I/O: output is handed over through [`Esp32C3::take_console`].
 #[derive(Debug)]
@@ -38,6 +58,8 @@ pub struct Esp32C3 {
     bus: SocBus,
     timing: DatasheetTiming,
     trace: Option<TraceHash>,
+    /// The line the interrupt controller selects, and the device time from which the CPU sees it.
+    selected: Option<(u32, Cycles)>,
 }
 
 impl Esp32C3 {
@@ -76,7 +98,7 @@ impl Esp32C3 {
         let hart_config = HartConfig {
             misaligned: MisalignedPolicy::Allow,
             vendor_csrs: VendorCsrs::Esp32C3,
-            ..HartConfig::default()
+            interrupts: InterruptModel::Controller,
         };
         let mut bus = SocBus::new(rom, flash, config);
         let (sysclk_conf, cpu_per_conf) = bus.periph.system.clock_registers();
@@ -91,18 +113,41 @@ impl Esp32C3 {
             bus,
             timing: DatasheetTiming,
             trace: config.trace_hash.then(TraceHash::new),
+            selected: None,
         }
     }
 
     /// Runs until the hart's cycle counter reaches `limit` (an absolute [`Esp32C3::cycles`] value) or
     /// the machine stops for another reason.
     pub fn run_until(&mut self, limit: Cycles) -> StopReason {
+        let mut slept: u64 = 0;
+        let mut fruitless_wakes: u32 = 0;
         while self.hart.cycles.get() < limit.get() {
             let pc = self.hart.pc;
             self.bus.set_pc(pc);
             let before = self.hart.cycles.get();
             match self.hart.step(&mut self.bus, &self.timing) {
-                StepOutcome::Waiting => return StopReason::Idle,
+                StepOutcome::Waiting => {
+                    // The CPU idles: device time jumps to the next instant a device asked for.
+                    let Some(duration) = self.sleep() else {
+                        return StopReason::Idle;
+                    };
+                    slept = slept.saturating_add(duration);
+                    fruitless_wakes += 1;
+                    self.service(pc);
+                    if let Some(violation) = self.bus.ctx.diag.take_violation() {
+                        return StopReason::StrictMmio(violation);
+                    }
+                    if slept >= SLEEP_SLICE || fruitless_wakes >= MAX_FRUITLESS_WAKES {
+                        return StopReason::Asleep;
+                    }
+                    continue;
+                }
+                StepOutcome::Interrupted { cause } => {
+                    // The controller forgets the edge it had latched for the line.
+                    self.bus.periph.intmatrix.claim(cause);
+                    self.bus.ctx.interrupts_changed();
+                }
                 // An `ecall` is a request to the trap handler, not a fault worth reporting.
                 StepOutcome::Trapped { trap, .. } if trap.cause != Exception::EcallM => {
                     self.bus.ctx.diag.record(Diagnostic::Exception {
@@ -115,12 +160,12 @@ impl Esp32C3 {
             }
             // Device time follows the CPU: the cycles of this instruction, at the clock it ran on.
             self.bus.ctx.advance(self.hart.cycles.get() - before);
-            if self.bus.ctx.take_clock_written() {
-                self.retune(pc);
-            }
             if let Some(trace) = &mut self.trace {
                 trace.mix_u32(pc);
             }
+            if self.needs_service() {
+                self.service(pc);
+            }
             if let Some(violation) = self.bus.ctx.diag.take_violation() {
                 return StopReason::StrictMmio(violation);
             }
@@ -128,6 +173,77 @@ impl Esp32C3 {
         StopReason::CycleBudget
     }
 
+    /// Whether anything happened that the loop has to act on before the next instruction.
+    fn needs_service(&self) -> bool {
+        let ctx = &self.bus.ctx;
+        ctx.wake_due()
+            || ctx.is_flagged()
+            || self
+                .selected
+                .is_some_and(|(_, visible_at)| visible_at <= ctx.now())
+    }
+
+    /// Acts on what the last instruction, or the passing of time, left in the context: wake-ups
+    /// that are due, a clock change, a change of the interrupt lines.
+    fn service(&mut self, pc: u32) {
+        while let Some(event) = self.bus.ctx.take_due() {
+            self.bus.deliver(event);
+        }
+        if self.bus.ctx.take_clock_written() {
+            self.retune(pc);
+        }
+        if self.bus.ctx.take_interrupts_changed() {
+            self.select_interrupt();
+        }
+        self.present_interrupt();
+    }
+
+    /// Asks the interrupt controller which line it selects now.
+    fn select_interrupt(&mut self) {
+        let sources = self.bus.ctx.sources();
+        let line = self.bus.periph.intmatrix.resolve(sources);
+        self.selected = match (line, self.selected) {
+            (None, _) => None,
+            // The same line as before: the instant it became visible stands.
+            (Some(line), Some((previous, visible_at))) if line == previous => {
+                Some((line, visible_at))
+            }
+            (Some(line), _) => {
+                let latency = INTERRUPT_LATENCY_CYCLES * self.bus.ctx.clock().cpu_period;
+                Some((line, self.bus.ctx.now() + latency))
+            }
+        };
+    }
+
+    /// Shows the CPU the selected line, once its latency has elapsed.
+    fn present_interrupt(&mut self) {
+        let now = self.bus.ctx.now();
+        let visible = self
+            .selected
+            .filter(|&(_, visible_at)| visible_at <= now)
+            .map(|(line, _)| line);
+        self.hart.set_external_interrupt(visible);
+    }
+
+    /// Moves device time to the next instant something is due: a wake-up a device asked for, or
+    /// the end of the latency of the selected interrupt line. Returns the time skipped, `None`
+    /// when nothing is due at any time.
+    ///
+    /// The CPU clock is gated while the CPU waits: its cycle counter does not move (TRM register
+    /// 1.12 `mpccr`, "the counter stops when the CPU is in WFI"; report TW §H.3).
+    fn sleep(&mut self) -> Option<u64> {
+        let now = self.bus.ctx.now();
+        let latency_end = self.selected.map(|(_, visible_at)| visible_at);
+        let wake = match (self.bus.ctx.next_wake(), latency_end) {
+            (Some(a), Some(b)) => a.min(b),
+            (a, b) => a.or(b)?,
+        };
+        // A wake-up already due is delivered without moving time.
+        let target = wake.max(now);
+        self.bus.ctx.sleep_until(target);
+        Some(target.get() - now.get())
+    }
+
     /// Applies a write to the clock registers: from the end of the instruction that wrote them,
     /// cycles last what the new selection says.
     fn retune(&mut self, pc: u32) {
```

- [ ] **Step 5: The CLI goes on while the CPU sleeps**

```diff
--- a/crates/emu-cli/src/run.rs
+++ b/crates/emu-cli/src/run.rs
@@ -140,7 +140,7 @@ pub fn execute(
             (Ok(true), StopReason::Idle) => {
                 let _ = writeln!(
                     log,
-                    "inkemu: the CPU is waiting for an interrupt and nothing can raise one; interrupt sources are not emulated yet"
+                    "inkemu: the CPU waits for an interrupt and none can come: no interrupt line is pending and no device has anything scheduled; check that the firmware enables the interrupt it waits for"
                 );
                 break RunEnd::Idle;
             }
@@ -155,7 +155,8 @@ pub fn execute(
                 );
                 break RunEnd::Budget;
             }
-            (Ok(true), StopReason::CycleBudget) => {}
+            // The CPU sleeps and device time goes on: the next slice continues.
+            (Ok(true), StopReason::CycleBudget | StopReason::Asleep) => {}
         }
     };
     let _ = writeln!(
@@ -410,7 +411,7 @@ mod tests {
         );
         assert_eq!((end, end.exit_code()), (RunEnd::Idle, 4));
         assert!(
-            text(&log).contains("waiting for an interrupt"),
+            text(&log).contains("waits for an interrupt and none can come"),
             "{}",
             text(&log)
         );
```

```diff
--- a/crates/emu-cli/src/exit.rs
+++ b/crates/emu-cli/src/exit.rs
@@ -18,7 +18,8 @@ Exit codes:
   2  error: unknown device, missing or invalid ROM, refused flash image, or standard output cannot be written
   3  stopped by --strict-mmio (an unmodelled register, an 8/16-bit register write, an operation the
      emulator cannot perform faithfully, or flash that is not mapped)
-  4  the CPU is waiting for an interrupt and nothing can raise one (interrupts are not emulated yet)";
+  4  the CPU waits for an interrupt and none can come: no interrupt line is pending and no device
+     has anything scheduled";
 
 #[cfg(test)]
 mod tests {
```

- [ ] **Step 6: Run the suite and the ignored tests**

Run: `cargo test --workspace > /tmp/t8.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t8.log`
Expected: `exit=0`, `631`.

Run the ignored tests with the counts of `.github/workflows/ci.yml` (16 `real_rom`, 5 `reference_boot`, 4 + 1 `run_cli`).
Expected: all pass. The strict run of the reference image still ends on `TIMG0.RTCCALICFG`: the application has not started its scheduler yet at that point.

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/soc-esp32c3/src crates/emu-cli/src/run.rs crates/emu-cli/src/exit.rs
/usr/bin/git commit -m "feat(soc): interrupts are delivered; wfi sleeps to the next wake-up

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3/src crates/emu-cli/src/run.rs crates/emu-cli/src/exit.rs
```

---

## Task 9: UART transmit time

A byte written to the FIFO leaves at the baud rate (decisions E10, E30). `STATUS.TXFIFO_CNT` and `FSM_STATUS.ST_UTX_OUT` follow, which is what `esp_restart` waits on before it resets (IR §4.1). The console receives a byte when its last bit has gone; at the end of a run it also receives what was still in the UART, so that it never stops short of the place the run stopped at.

**Files:**
- Modify: `crates/soc-esp32c3/src/periph/uart.rs` (rewritten), `crates/soc-esp32c3/src/ctx.rs`, `crates/soc-esp32c3/src/diag.rs`, `crates/soc-esp32c3/src/periph/mod.rs`, `crates/soc-esp32c3/src/machine.rs`, `crates/soc-esp32c3/src/bus.rs`, `crates/soc-esp32c3/tests/reference_boot.rs`, `crates/emu-cli/src/run.rs`, `docs/registers-accepted-without-effect.md`

**Interfaces:**
- Consumes: `Ctx::{now, clock, unsupported}`, `Deadline`, `clock::XTAL_PERIOD`, `CpuClock.apb_period` (Tasks 2, 3); `Uart::new(base: u32)`, `Uart::take_tx(&mut self) -> Vec<u8>`; `Esp32C3::take_console`.
- Produces: `Event::Uart(u8)`; `Unsupported::UartOverflow { uart: u8 }`; `Uart::new(index: u8, base: u32) -> Uart`, `Uart::take_tx(&mut self, now: Cycles) -> Vec<u8>` (the bytes that have left by `now`), `Uart::take_written(&mut self) -> Vec<u8>` (those and the ones still on their way; it ends the transmission), `Uart::reset(&mut self, now: Cycles)`, and the crate-visible `wake(&mut self, ctx: &mut Ctx)`; `Esp32C3::flush_console(&mut self, channel: ConsoleChannel) -> Vec<u8>`. `Esp32C3::take_console` keeps its signature and returns the bytes that have left. Registers: `FIFO`, `STATUS` (`TXFIFO_CNT`), `FSM_STATUS` (`ST_UTX_OUT`), `CLKDIV`, `CLK_CONF` and `CONF0.TXFIFO_RST` act; `INT_ENA`, `INT_CLR`, `CONF0`, `CONF1` stay accepted rows whose heir is SP1b-3b.

- [ ] **Step 1: Write the failing tests**

One byte at 115 200 baud is 10 bits of 347 + 3/16 cycles of the 40 MHz crystal: 86.8 µs (`ROM3:uart_div_modify@0x4004c20c` writes `CLKDIV = 0x30015B`; BA §7). The FIFO holds 128 bytes (`ROM3:uart_tx_one_char_uart.part.0@0x4004c0be` waits while `TXFIFO_CNT` ≥ 128). Replace the test module of `crates/soc-esp32c3/src/periph/uart.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    const UART0: u32 = svd_regs::esp32c3::addr("UART0", "FIFO");
    const UART1: u32 = svd_regs::esp32c3::addr("UART1", "FIFO");
    /// What the ROM and the bootloader leave: the crystal undivided, 347 + 3/16 (BA §7).
    const CLKDIV_115200: u32 = 0x0030_015B;
    const CLK_CONF_XTAL: u32 = 0x0370_0000;
    /// One byte at 115 200 baud: 10 x (347 x 16 + 3) x 4 sixteenths, 13 887.5 cycles, 86.8 us.
    const BYTE_SUBCYCLES: u64 = 10 * (347 * 16 + 3) * 4;

    fn console(ctx: &mut Ctx) -> Uart {
        let mut uart = Uart::new(0, UART0);
        assert!(uart.write(ctx, CLK_CONF, CLK_CONF_XTAL));
        assert!(uart.write(ctx, CLKDIV, CLKDIV_115200));
        uart
    }

    fn after_bytes(ctx: &mut Ctx, bytes: u64) {
        ctx.sleep_until(ctx.now() + (bytes * BYTE_SUBCYCLES).div_ceil(SUBCYCLES));
    }

    fn fifo_count(ctx: &mut Ctx, uart: &mut Uart) -> u32 {
        (uart.read(ctx, STATUS).unwrap() >> TXFIFO_CNT_SHIFT) & 0x3FF
    }

    #[test]
    fn bytes_leave_one_at_a_time_at_the_baud_rate_in_order_and_only_once() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        for byte in *b"ESP" {
            assert!(uart.write(ctx, FIFO, u32::from(byte)));
        }
        assert_eq!(uart.take_tx(ctx.now()), [], "nothing has left yet");
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx.now()), b"E");
        assert_eq!(uart.take_tx(ctx.now()), [], "taken once");
        after_bytes(ctx, 2);
        assert_eq!(uart.take_tx(ctx.now()), b"SP");
    }

    #[test]
    fn only_the_low_byte_of_a_fifo_write_is_transmitted() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        uart.write(ctx, FIFO, 0xFFFF_FF41);
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx.now()), b"A");
    }

    #[test]
    fn the_fifo_count_and_the_transmitter_state_follow_the_bytes_out() {
        // BA §7: `uart_tx_flush` waits for TXFIFO_CNT = 0, `uart_tx_wait_idle` for ST_UTX_OUT = 0.
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        assert_eq!(
            uart.read(ctx, STATUS),
            Some(0xE000_C000),
            "idle: the SVD reset value"
        );
        assert_eq!(uart.read(ctx, FSM_STATUS), Some(0));
        for _ in 0..3 {
            uart.write(ctx, FIFO, 0x55);
        }
        assert_eq!(
            fifo_count(ctx, &mut uart),
            2,
            "one byte is in the shift register"
        );
        assert_eq!(uart.read(ctx, FSM_STATUS), Some(ST_UTX_OUT_BUSY));
        after_bytes(ctx, 2);
        assert_eq!(
            fifo_count(ctx, &mut uart),
            0,
            "empty, the last byte still going out"
        );
        assert_eq!(uart.read(ctx, FSM_STATUS), Some(ST_UTX_OUT_BUSY));
        after_bytes(ctx, 1);
        assert_eq!(uart.read(ctx, FSM_STATUS), Some(0));
        assert_eq!(uart.read(ctx, STATUS), Some(0xE000_C000));
        assert!(!uart.write(ctx, FSM_STATUS, 0xFF), "read-only");
        assert!(!uart.write(ctx, STATUS, 0xFFFF_FFFF), "read-only");
    }

    #[test]
    fn a_byte_written_to_an_idle_transmitter_starts_at_once() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        uart.write(ctx, FIFO, u32::from(b'a'));
        after_bytes(ctx, 10);
        uart.write(ctx, FIFO, u32::from(b'b'));
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx.now()), b"ab");
    }

    #[test]
    fn the_baud_rate_follows_the_divider_and_the_source_clock() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        // The crystal divided by 2: a byte takes twice as long.
        uart.write(ctx, CLK_CONF, CLK_CONF_XTAL | (1 << SCLK_DIV_NUM_SHIFT));
        uart.write(ctx, FIFO, u32::from(b'x'));
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx.now()), []);
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx.now()), b"x");
        // APB_CLK as the source, 20 MHz after power-on: twice as long as with the crystal.
        uart.write(ctx, CLK_CONF, CLK_CONF_XTAL & !(0b10 << SCLK_SEL_SHIFT));
        uart.write(ctx, FIFO, u32::from(b'y'));
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx.now()), []);
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx.now()), b"y");
    }

    #[test]
    fn a_byte_written_to_a_full_fifo_is_dropped_and_reported() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        for byte in 0..=128u32 {
            uart.write(ctx, FIFO, byte);
        }
        assert_eq!(
            fifo_count(ctx, &mut uart),
            128,
            "the ROM waits at this count"
        );
        assert_eq!(ctx.take_unsupported(), None);
        assert!(uart.write(ctx, FIFO, 0xEE));
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::UartOverflow { uart: 0 })
        );
        after_bytes(ctx, 130);
        let sent = uart.take_tx(ctx.now());
        assert_eq!(sent.len(), 129);
        assert!(!sent.contains(&0xEE));
    }

    #[test]
    fn a_reset_loses_the_bytes_that_have_not_left_and_keeps_those_that_have() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        for byte in *b"gone lost" {
            uart.write(ctx, FIFO, u32::from(byte));
        }
        after_bytes(ctx, 4);
        uart.reset(ctx.now());
        after_bytes(ctx, 20);
        assert_eq!(uart.take_tx(ctx.now()), b"gone");
        assert_eq!(
            uart.read(ctx, CLKDIV),
            Some(0x2B6),
            "registers back to their reset values"
        );
        assert_eq!(uart.read(ctx, FSM_STATUS), Some(0));
    }

    #[test]
    fn the_transmit_fifo_reset_keeps_only_the_byte_being_sent() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        for byte in *b"abc" {
            uart.write(ctx, FIFO, u32::from(byte));
        }
        // `Uart_Init`: CONF0 with bits 26, 18 and 17 set, then without bits 18 and 17 (BC §G.2).
        uart.write(ctx, CONF0, 0x0406_001C);
        uart.write(ctx, CONF0, 0x0400_001C);
        assert_eq!(fifo_count(ctx, &mut uart), 0);
        after_bytes(ctx, 3);
        assert_eq!(uart.take_tx(ctx.now()), b"a");
    }

    #[test]
    fn line_configuration_is_storage_from_its_reset_value() {
        let ctx = &mut Ctx::default();
        let mut uart = Uart::new(0, UART0);
        for offset in [INT_ENA, CLKDIV, CONF0, CONF1, CLK_CONF] {
            let reset = svd_regs::esp32c3::lookup(UART0 + offset)
                .unwrap()
                .reset_value;
            assert_eq!(uart.read(ctx, offset), Some(reset), "{offset:#x}");
            assert!(uart.write(ctx, offset, 0x15B));
            assert_eq!(uart.read(ctx, offset), Some(0x15B));
        }
    }

    #[test]
    fn interrupt_clear_is_accepted_and_the_rest_is_unmodeled() {
        let ctx = &mut Ctx::default();
        let mut uart = Uart::new(0, UART0);
        assert!(uart.write(ctx, INT_CLR, 0xFFFF_FFFF));
        assert_eq!(
            uart.read(ctx, reg("INT_RAW")),
            None,
            "interrupts are SP1b-3b"
        );
        assert_eq!(uart.read(ctx, reg("INT_ST")), None);
        assert_eq!(
            uart.read(ctx, FIFO),
            None,
            "FIFO is write-only: RX is not modelled"
        );
    }

    #[test]
    fn the_second_uart_has_its_own_reset_values_and_its_own_name() {
        let ctx = &mut Ctx::default();
        // 0xE000_C000 is the SVD reset value of UART0.STATUS and UART1.STATUS; the FIFO counts are
        // zero in it already. This pins the SVD lookup behind `Uart::new`.
        let mut uart1 = Uart::new(1, UART1);
        assert_eq!(uart1.read(ctx, STATUS), Some(0xE000_C000));
        assert_eq!(uart1.read(ctx, CLKDIV), Some(0x2B6));
        for byte in 0..=129u32 {
            uart1.write(ctx, FIFO, byte);
        }
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::UartOverflow { uart: 1 })
        );
    }

    #[test]
    fn the_transmitter_asks_to_be_called_when_its_last_byte_has_left() {
        // Without this a CPU in `wfi` with bytes on their way out would never see them leave.
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        assert_eq!(ctx.next_wake(), None);
        uart.write(ctx, FIFO, u32::from(b'o'));
        uart.write(ctx, FIFO, u32::from(b'k'));
        let first_gone = Cycles(BYTE_SUBCYCLES.div_ceil(SUBCYCLES));
        let last_gone = Cycles((2 * BYTE_SUBCYCLES).div_ceil(SUBCYCLES));
        assert_eq!(
            ctx.next_wake(),
            Some(first_gone),
            "the earliest call is kept"
        );
        ctx.sleep_until(first_gone);
        assert_eq!(ctx.take_due(), Some(Event::Uart(0)));
        uart.wake(ctx);
        assert_eq!(ctx.next_wake(), Some(last_gone));
        ctx.sleep_until(last_gone);
        assert_eq!(ctx.take_due(), Some(Event::Uart(0)));
        uart.wake(ctx);
        assert_eq!(ctx.next_wake(), None, "idle: nothing more to wait for");
        assert_eq!(uart.take_tx(ctx.now()), b"ok");
    }

    #[test]
    fn the_end_of_a_run_takes_the_bytes_that_have_not_left_too() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        for byte in *b"bye" {
            uart.write(ctx, FIFO, u32::from(byte));
        }
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx.now()), b"b");
        assert_eq!(uart.take_written(), b"ye", "in order, each once");
        assert_eq!(uart.take_written(), b"");
        assert_eq!(
            uart.read(ctx, FSM_STATUS),
            Some(0),
            "the transmission ended"
        );
    }
}
```

The diagnostic, the machine, the bus and the CLI (a byte at the reset baud rate and clock takes 6 940 CPU cycles, 347 µs):

```diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -505,6 +512,7 @@ mod tests {
             unsupported(Unsupported::SoftwareReset {
                 options0: 0x8000_0000,
             }),
+            unsupported(Unsupported::UartOverflow { uart: 0 }),
             mapping(3, MappingFault::InvalidEntry),
             mapping(3, MappingFault::BeyondFlash),
         ] {
```

```diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -392,6 +404,12 @@ mod tests {
         let mut m = machine(&program);
         assert_eq!(m.pc(), ROM_BASE);
         assert_eq!(m.run_until(Cycles(500)), StopReason::CycleBudget);
+        assert_eq!(
+            m.take_console(ConsoleChannel::Uart0),
+            b"",
+            "a byte takes 6 940 CPU cycles to leave at the reset baud rate and clock"
+        );
+        assert_eq!(m.run_until(Cycles(15_000)), StopReason::CycleBudget);
         assert_eq!(m.take_console(ConsoleChannel::Uart0), b"Hi");
         assert_eq!(
             m.take_console(ConsoleChannel::Uart0),
```

```diff
--- a/crates/soc-esp32c3/src/bus.rs
+++ b/crates/soc-esp32c3/src/bus.rs
@@ -754,7 +754,10 @@ mod tests {
         b.write(0x6004_3000, Word, u32::from(b'J')).unwrap();
         b.write(0x6004_3004, Word, 1).unwrap();
         b.write(0x6001_0010, Word, 1).unwrap(); // UART1.INT_CLR, written by the ROM's uartAttach
-        assert_eq!(b.periph.uart0.take_tx(), b"U");
+        // A byte takes 10 bit times to leave the UART: 347 us at the reset baud rate (55 520 cycles
+        // of the time base).
+        b.ctx.sleep_until(emu_core::Cycles(60_000));
+        assert_eq!(b.periph.uart0.take_tx(b.ctx.now()), b"U");
         assert_eq!(b.periph.usb_serial_jtag.take_tx(), b"J");
         assert_eq!(b.ctx.diag.drain(), []);
     }
```

```diff
--- a/crates/emu-cli/src/run.rs
+++ b/crates/emu-cli/src/run.rs
@@ -243,7 +248,8 @@ mod tests {
         );
         let (mut console, mut log) = (Vec::new(), Vec::new());
         assert_eq!(
-            execute(&mut m, &options(10_000), &mut console, &mut log),
+            // A byte takes 6 940 CPU cycles to leave the UART at the reset baud rate and clock.
+            execute(&mut m, &options(60_000), &mut console, &mut log),
             RunEnd::Budget
         );
         assert_eq!(console, b"hello\n");
@@ -498,4 +504,20 @@ mod tests {
         assert_eq!((end, end.exit_code()), (RunEnd::ConsoleError, 2));
         assert!(text(&log).contains("disk full"), "{}", text(&log));
     }
+
+    #[test]
+    fn the_console_shows_what_was_written_when_the_run_stopped() {
+        // Five bytes need 34 700 cycles to leave at the reset baud rate; the run stops long
+        // before, on the unmodelled register the firmware reads right after printing.
+        let mut program = printing(UART0, b"hello", &[]);
+        program.extend(reading_unmodeled_register());
+        let mut m = machine(&program, STRICT);
+        let (mut console, mut log) = (Vec::new(), Vec::new());
+        let end = execute(&mut m, &options(10_000), &mut console, &mut log);
+        assert_eq!(end, RunEnd::StrictMmio);
+        assert_eq!(
+            console, b"hello",
+            "the bytes still in the UART are not lost"
+        );
+    }
 }
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p soc-esp32c3 --lib > /tmp/t9.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, compile errors: `Uart::new` takes 1 argument, `take_tx` takes 0 arguments, no variant `UartOverflow`, no variant `Event::Uart`.

- [ ] **Step 3: The event and the diagnostic**

```diff
--- a/crates/soc-esp32c3/src/ctx.rs
+++ b/crates/soc-esp32c3/src/ctx.rs
@@ -48,6 +48,8 @@ pub enum Event {
     FlashWindowStore { instruction_bus: bool },
     /// Wake-up of the SYSTIMER: one of its comparators may have an alarm due.
     Systimer,
+    /// Wake-up of UART 0 or 1: its transmitter may have gone idle.
+    Uart(u8),
 }
 
 /// The wake-up a device keeps for its next deadline.
```

```diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -34,6 +34,8 @@ pub enum Unsupported {
     ClockSource { sysclk_conf: u32, cpu_per_conf: u32 },
     /// A reset requested through `RTC_CNTL.OPTIONS0` (D27).
     SoftwareReset { options0: u32 },
+    /// A byte written to a UART whose transmit FIFO was full: the hardware drops it too.
+    UartOverflow { uart: u8 },
 }
 
 impl Unsupported {
@@ -79,6 +81,10 @@ impl Unsupported {
                 f,
                 "a software reset was requested through RTC_CNTL.OPTIONS0 (value {options0:#010x}, pc {pc:#010x}) but resets are not emulated yet, so the write was ignored; restart inkemu to reset the device"
             ),
+            Unsupported::UartOverflow { uart } => write!(
+                f,
+                "a byte was written to UART{uart} while its 128-byte transmit FIFO was full (pc {pc:#010x}), so the byte was dropped, as on the device; wait for room in the FIFO (UART.STATUS TXFIFO_CNT) before writing"
+            ),
         }
     }
 }
@@ -152,6 +158,7 @@ impl Diagnostic {
                     cpu_per_conf,
                 } => (8, sysclk_conf, cpu_per_conf),
                 Unsupported::SoftwareReset { .. } => (9, 0, 0),
+                Unsupported::UartOverflow { uart } => (12, u32::from(uart), 0),
                 // A command is 1 to 16 bits long, so 0 is free for "no command phase".
                 Unsupported::FlashTransfer { command_bits, .. } => {
                     (11, command_bits.unwrap_or(0), 0)
```

- [ ] **Step 4: The transmitter**

Replace everything above the test module of `crates/soc-esp32c3/src/periph/uart.rs` with:

```rust
//! UART controller, transmit side. Register layout: `uart_reg.h` (reports BC §G.2, BA §7).
//!
//! A byte written to the FIFO leaves at the baud rate: the transmit FIFO fills, `STATUS` and
//! `FSM_STATUS` say so, and the byte reaches the console when its last bit has gone. Software waits
//! on both: the ROM before each character, ESP-IDF before it changes a clock or restarts (report IR
//! §4.1). Reception and the UART interrupts arrive in SP1b-3b.
use crate::clock::XTAL_PERIOD;
use crate::ctx::{Ctx, Deadline, Event};
use crate::diag::Unsupported;
use crate::periph::{Accepted, Storage};
use emu_core::{Cycles, Peripheral};
use std::collections::VecDeque;

/// `INT_CLR` is write-only and accepted; the three others are stored.
pub(crate) fn accepted() -> Vec<Accepted> {
    let mut rows = Vec::new();
    for peripheral in ["UART0", "UART1"] {
        for (register, not_modeled) in [
            ("INT_ENA", "interrupt enables: nothing is raised"),
            ("INT_CLR", "interrupt clear: nothing is pending"),
            (
                "CONF0",
                "frame format: a byte is 10 bit times (the transmit FIFO reset acts)",
            ),
            ("CONF1", "FIFO thresholds"),
        ] {
            rows.push(Accepted::svd(peripheral, register, not_modeled, "SP1b-3b"));
        }
    }
    rows
}

/// The model serves UART0 and UART1, whose register offsets are identical: those of UART0 name them.
const fn reg(name: &str) -> u32 {
    svd_regs::esp32c3::addr("UART0", name) - svd_regs::esp32c3::addr("UART0", "FIFO")
}

const FIFO: u32 = reg("FIFO");
const INT_ENA: u32 = reg("INT_ENA");
const INT_CLR: u32 = reg("INT_CLR");
const CLKDIV: u32 = reg("CLKDIV");
const STATUS: u32 = reg("STATUS");
const FSM_STATUS: u32 = reg("FSM_STATUS");
const CONF0: u32 = reg("CONF0");
const CONF1: u32 = reg("CONF1");
const CLK_CONF: u32 = reg("CLK_CONF");
/// What software writes and reads back. `CLKDIV` and `CLK_CONF` set the baud rate.
const CONFIG: [u32; 5] = [INT_ENA, CLKDIV, CONF0, CONF1, CLK_CONF];
/// `STATUS` bits [9:0] `RXFIFO_CNT` and [25:16] `TXFIFO_CNT`.
const FIFO_COUNTS: u32 = 0x03FF_03FF;
const TXFIFO_CNT_SHIFT: u32 = 16;
/// `FSM_STATUS` bits [7:4] `ST_UTX_OUT`: 0 when the transmitter is idle, the only value software
/// tests (`ROM3:uart_tx_wait_idle@0x4004c4c6`; BA §7).
/// ASSUMPTION: while a byte is being sent the field reads 1. The other states are not documented
/// in the reports and nothing on the measured paths tells them apart.
const ST_UTX_OUT_BUSY: u32 = 1 << 4;
/// The transmit FIFO holds 128 bytes: the ROM waits while `TXFIFO_CNT` is 128 or more
/// (`ROM3:uart_tx_one_char_uart.part.0@0x4004c0be`; BC §G.2).
const FIFO_BYTES: usize = 128;
/// `CLKDIV`: bits [11:0] the integer part of the divider, bits [23:20] its fraction in sixteenths
/// (`ROM3:uart_div_modify@0x4004c20c` writes both: `0x30015B` is 347 + 3/16; BC §G.2, BA §7).
const CLKDIV_INTEGER_MASK: u32 = 0xFFF;
const CLKDIV_FRAG_SHIFT: u32 = 20;
const CLKDIV_FRAG_MASK: u32 = 0xF;
/// `CLK_CONF`: bits [19:12] `SCLK_DIV_NUM`, the divider of the source clock minus one (the
/// bootloader clears it: `bootloader_console_init`, store at `0x403cc8b6` in trace F1), bits
/// [21:20] `SCLK_SEL`, 1 = `APB_CLK`, 3 = the crystal (`ROM3:Uart_Init` sets 3, store at
/// `0x4004c2e2` in trace F1; BA §7: "the UART core clock is the crystal").
const SCLK_DIV_NUM_SHIFT: u32 = 12;
const SCLK_DIV_NUM_MASK: u32 = 0xFF;
const SCLK_SEL_SHIFT: u32 = 20;
const SCLK_SEL_MASK: u32 = 0b11;
const SCLK_SEL_APB: u32 = 1;
/// `CONF0` bit 18 `TXFIFO_RST`: empties the transmit FIFO (`ROM3:Uart_Init@0x4004c306` sets it,
/// then clears it; BC §G.2).
const TXFIFO_RST: u32 = 1 << 18;
/// ASSUMPTION: a byte is 10 bit times on the wire (a start bit, 8 data bits, a stop bit), whatever
/// `CONF0` says. The ROM and ESP-IDF consoles use this format; another one would only change how
/// long a byte takes. SP2 gives the frame format its effect, with the receive side.
const BITS_PER_BYTE: u64 = 10;
/// Device time is kept in sixteenths of a time-base cycle here: a bit lasts a whole number of them.
const SUBCYCLES: u64 = 16;

#[derive(Clone, Debug)]
pub struct Uart {
    /// 0 for UART0, 1 for UART1: named in diagnostics.
    index: u8,
    base: u32,
    /// `STATUS` with both FIFO counts at zero: the line-state bits keep their reset value.
    idle_status: u32,
    config: Storage,
    /// The bytes on their way out, each with the time, in sixteenths of a time-base cycle, at
    /// which its last bit has gone. The first is in the shift register, the others in the FIFO.
    in_flight: VecDeque<(u64, u8)>,
    /// The bytes that have left, for the console.
    tx: Vec<u8>,
    /// The call at the time the transmitter goes idle: a CPU that waits for an interrupt with
    /// bytes still on their way out sleeps until they have left, not for ever.
    wake: Deadline,
}

impl Uart {
    #[must_use]
    pub fn new(index: u8, base: u32) -> Uart {
        let status_reset = svd_regs::esp32c3::lookup(base + STATUS).map_or(0, |r| r.reset_value);
        Uart {
            index,
            base,
            idle_status: status_reset & !FIFO_COUNTS,
            config: Storage::for_offsets(base, &CONFIG),
            in_flight: VecDeque::new(),
            tx: Vec::new(),
            wake: Deadline::default(),
        }
    }

    /// The module is reset (`SYSTEM.PERIP_RST_EN0`, or a core reset): its registers return to their
    /// reset values and the bytes that have not left are lost. Those that left stay for the console.
    pub fn reset(&mut self, now: Cycles) {
        self.settle(now);
        self.config = Storage::for_offsets(self.base, &CONFIG);
        self.in_flight.clear();
    }

    /// The bytes that have left the transmitter by `now`, since the last call.
    pub fn take_tx(&mut self, now: Cycles) -> Vec<u8> {
        self.settle(now);
        std::mem::take(&mut self.tx)
    }

    /// Every byte the firmware wrote and no call has taken, those that have not left yet
    /// included: the transmission ends here. For the end of a run, so that the console shows
    /// what the firmware had written when it stopped.
    pub fn take_written(&mut self) -> Vec<u8> {
        let mut written = std::mem::take(&mut self.tx);
        written.extend(self.in_flight.drain(..).map(|(_, byte)| byte));
        written
    }

    /// The device time of a wake-up has come: the bytes that have gone leave, and the device asks
    /// to be called again when the last one will have.
    pub(crate) fn wake(&mut self, ctx: &mut Ctx) {
        let now = ctx.now();
        self.wake.called(now);
        self.settle(now);
        if let Some(&(gone_at, _)) = self.in_flight.back() {
            self.wake.arm(
                ctx,
                Cycles(gone_at.div_ceil(SUBCYCLES)),
                Event::Uart(self.index),
            );
        }
    }

    /// Moves the bytes whose last bit has gone to the console.
    fn settle(&mut self, now: Cycles) {
        let now = now.get().saturating_mul(SUBCYCLES);
        while let Some(&(gone_at, byte)) = self.in_flight.front() {
            if gone_at > now {
                break;
            }
            self.tx.push(byte);
            self.in_flight.pop_front();
        }
    }

    /// The time one byte takes, in sixteenths of a time-base cycle: 10 bits, each `CLKDIV` (with
    /// its fraction) cycles of the source clock divided by `SCLK_DIV_NUM + 1`.
    ///
    /// ASSUMPTION: a source selection other than `APB_CLK` is clocked by the crystal. 3 is the
    /// crystal; 0 and 2 (the RC fast oscillator, 17.5 MHz) are not modelled and nothing on the
    /// measured paths selects them. The fractional divider of the source clock (`SCLK_DIV_A/B`) is
    /// ignored: it resets to 0 and no measured code writes it.
    fn byte_time(&self, ctx: &Ctx) -> u64 {
        let clkdiv = self.config.read(CLKDIV).unwrap_or(0);
        let clk_conf = self.config.read(CLK_CONF).unwrap_or(0);
        let divider = u64::from(clkdiv & CLKDIV_INTEGER_MASK) * SUBCYCLES
            + u64::from((clkdiv >> CLKDIV_FRAG_SHIFT) & CLKDIV_FRAG_MASK);
        let source_period = if (clk_conf >> SCLK_SEL_SHIFT) & SCLK_SEL_MASK == SCLK_SEL_APB {
            ctx.clock().apb_period
        } else {
            XTAL_PERIOD
        };
        let source_divider = u64::from((clk_conf >> SCLK_DIV_NUM_SHIFT) & SCLK_DIV_NUM_MASK) + 1;
        BITS_PER_BYTE * divider * source_divider * source_period
    }

    fn send(&mut self, ctx: &mut Ctx, byte: u8) {
        // One byte is in the shift register, the FIFO holds the others.
        if self.in_flight.len() > FIFO_BYTES {
            ctx.unsupported(Unsupported::UartOverflow { uart: self.index });
            return;
        }
        let now = ctx.now().get().saturating_mul(SUBCYCLES);
        let starts_at = self.in_flight.back().map_or(now, |&(gone_at, _)| gone_at);
        let gone_at = starts_at.max(now).saturating_add(self.byte_time(ctx));
        self.in_flight.push_back((gone_at, byte));
        self.wake.arm(
            ctx,
            Cycles(gone_at.div_ceil(SUBCYCLES)),
            Event::Uart(self.index),
        );
    }
}

impl Peripheral<Ctx> for Uart {
    fn read(&mut self, ctx: &mut Ctx, offset: u32) -> Option<u32> {
        self.settle(ctx.now());
        match offset {
            STATUS => {
                let in_fifo = u32::try_from(self.in_flight.len().saturating_sub(1)).unwrap_or(0);
                Some(self.idle_status | (in_fifo << TXFIFO_CNT_SHIFT))
            }
            FSM_STATUS => Some(if self.in_flight.is_empty() {
                0
            } else {
                ST_UTX_OUT_BUSY
            }),
            _ => self.config.read(offset),
        }
    }

    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        self.settle(ctx.now());
        match offset {
            FIFO => {
                self.send(ctx, value.to_le_bytes()[0]);
                true
            }
            // No interrupt condition is modelled yet, so there is nothing to clear.
            INT_CLR => true,
            CONF0 => {
                if value & TXFIFO_RST != 0 {
                    // The byte in the shift register is on the wire already: it goes on.
                    self.in_flight.truncate(1);
                }
                self.config.write(offset, value)
            }
            _ => self.config.write(offset, value),
        }
    }
}
```

- [ ] **Step 5: Wire it; the console at the end of a run**

```diff
--- a/crates/soc-esp32c3/src/periph/mod.rs
+++ b/crates/soc-esp32c3/src/periph/mod.rs
@@ -421,8 +421,8 @@ impl Peripherals {
             systimer: Systimer::new(ctx.now()),
             apb_saradc: AcceptedBlock::new(APB_SARADC_BASE, &APB_SARADC_ACCEPTED),
             radio_power: AcceptedBlock::new(0, &RADIO_POWER_ACCEPTED),
-            uart0: Uart::new(UART0_BASE),
-            uart1: Uart::new(UART1_BASE),
+            uart0: Uart::new(0, UART0_BASE),
+            uart1: Uart::new(1, UART1_BASE),
             usb_serial_jtag: UsbSerialJtag::new(),
         };
         periph.systimer.wake(ctx);
@@ -438,6 +438,8 @@ impl Peripherals {
                 self.extmem.latch_store(ctx, instruction_bus);
             }
             Event::Systimer => self.systimer.wake(ctx),
+            Event::Uart(0) => self.uart0.wake(ctx),
+            Event::Uart(_) => self.uart1.wake(ctx),
         }
     }
 
```

```diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -291,10 +291,22 @@ impl Esp32C3 {
         self.hart.pc
     }
 
-    /// The bytes the firmware sent on `channel` since the last call.
+    /// The bytes that left the device on `channel` since the last call: a UART byte counts once
+    /// its last bit has gone.
     pub fn take_console(&mut self, channel: ConsoleChannel) -> Vec<u8> {
         match channel {
-            ConsoleChannel::Uart0 => self.bus.periph.uart0.take_tx(),
+            ConsoleChannel::Uart0 => self.bus.periph.uart0.take_tx(self.bus.ctx.now()),
+            ConsoleChannel::UsbSerialJtag => self.bus.periph.usb_serial_jtag.take_tx(),
+        }
+    }
+
+    /// Everything the firmware wrote on `channel` and no call has taken, the bytes that have not
+    /// left the device yet included. For the end of a run: without them the console would stop
+    /// short of the place the run stopped at. It ends the transmission: do not go on running a
+    /// machine whose console was flushed if the timing of its output matters.
+    pub fn flush_console(&mut self, channel: ConsoleChannel) -> Vec<u8> {
+        match channel {
+            ConsoleChannel::Uart0 => self.bus.periph.uart0.take_written(),
             ConsoleChannel::UsbSerialJtag => self.bus.periph.usb_serial_jtag.take_tx(),
         }
     }
```

```diff
--- a/crates/emu-cli/src/run.rs
+++ b/crates/emu-cli/src/run.rs
@@ -159,6 +159,11 @@ pub fn execute(
             (Ok(true), StopReason::CycleBudget | StopReason::Asleep) => {}
         }
     };
+    // What the firmware had written and the device had not sent yet: without it the console
+    // would end before the place the run stopped at.
+    if !matches!(end, RunEnd::ConsoleClosed | RunEnd::ConsoleError) {
+        let _ = relay(console, &machine.flush_console(options.console));
+    }
     let _ = writeln!(
         log,
         "inkemu: stopped at pc {:#010x} after {} cycles ({} instructions)",
```

The helper of the reference-boot tests collects the console the same way:

```diff
--- a/crates/soc-esp32c3/tests/reference_boot.rs
+++ b/crates/soc-esp32c3/tests/reference_boot.rs
@@ -149,6 +149,8 @@ fn run_image(image: Vec<u8>, config: SocConfig, done: impl Fn(&str, &[Diagnostic
         let _ = machine.take_console(ConsoleChannel::UsbSerialJtag);
         diagnostics.extend(machine.drain_diagnostics());
     }
+    // The run is over: what the firmware wrote and the UART had not sent yet belongs to its console.
+    uart0.extend(machine.flush_console(ConsoleChannel::Uart0));
     Run {
         console: text(&uart0),
         diagnostics,
```

- [ ] **Step 6: Run the suite, regenerate the table, run the ignored tests**

Run: `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect > /tmp/bless.log 2>&1; echo "exit=$?"; /usr/bin/git diff --stat docs/registers-accepted-without-effect.md`
Expected: `exit=0`; `CLKDIV` and `CLK_CONF` of both UARTs leave the table; the four rows that stay for each name SP1b-3b.

Run: `cargo test --workspace > /tmp/t9.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t9.log`
Expected: `exit=0`, `636`.

Run the ignored tests (16 `real_rom`, 5 `reference_boot`, 4 + 1 `run_cli`).
Expected: all pass. Without the flush of Step 5, `strict_run_reaches_the_application_and_ends_on_the_rtc_calibration` and `x4_boots_the_reference_firmware_to_its_first_application_line` fail: the run stops on the unmodelled register while the first line of the application is still in the UART.

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/soc-esp32c3 crates/emu-cli/src/run.rs docs/registers-accepted-without-effect.md
/usr/bin/git commit -m "feat(soc): a UART byte takes its time to leave

The console of a run that stops also shows what was still in the UART.

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3 crates/emu-cli/src/run.rs docs/registers-accepted-without-effect.md
```

---

## Task 10: `RTC_CNTL`: reset cause, software reset requests, RTC timer, RTC watchdog, super watchdog

The block stops being storage with a refusal. It keeps the cause of the last reset, asks for the two software resets (`OPTIONS0` bits 31 and 5: decision D27 of SP1b-2 is undone and the rest of the write is stored), counts the 48-bit RTC timer and the RTC watchdog on the 136 kHz slow clock, and models the super watchdog (decisions E5, E6, E7, E28, E29). The machine does not reset yet: Task 11 does. Until then no request may vanish: the machine takes each one out of the context and reports it by name (`Unsupported::Reset { cause }`, which replaces `SoftwareReset` and also names a watchdog that expired), so a firmware that asks for a reset, or stops feeding a watchdog, gets a diagnostic and stops a strict run at this commit as it did before it. Task 11 removes that refusal when the machine acts on the requests.

**Files:**
- Modify: `crates/soc-esp32c3/src/periph/rtc_cntl.rs` (rewritten), `crates/soc-esp32c3/src/ctx.rs`, `crates/soc-esp32c3/src/diag.rs`, `crates/soc-esp32c3/src/periph/efuse.rs`, `crates/soc-esp32c3/src/periph/mod.rs`, `crates/soc-esp32c3/src/test_support.rs`, `crates/soc-esp32c3/src/machine.rs`, `crates/emu-cli/src/run.rs`, `docs/registers-accepted-without-effect.md`

**Interfaces:**
- Consumes: `Ctx::{now, irq, request_reset, unsupported}`, `Deadline`, `Source::of`, `reset::{ResetCause, CAUSE_POWER_ON}` (Task 3); `counter::{Counter, Rate}`, `periph::half_of` (Tasks 5, 7); `config::STRAP_SPI_FAST_FLASH_BOOT`; `RtcCntl::new()`; `Unsupported::SoftwareReset`.
- Produces: `Event::RtcWatchdog`; `Unsupported::SlowClock { clk_conf: u32 }`; `Unsupported::Reset { cause: ResetCause }` in place of `SoftwareReset { options0 }`, recorded by the machine for every reset request and displayed as `a software reset was requested through RTC_CNTL.OPTIONS0 (reset cause 0x0c, pc 0x…) but resets are not emulated yet, …` or `the RTC watchdog expired (reset cause 0x10, …`, until Task 11 removes it; `Efuse::wdt_delay_sel(&self) -> u32`; `RtcCntl::new(flash_boot: bool, wdt_delay_sel: u32, now: Cycles) -> RtcCntl`, the crate-visible `wake(&mut self, ctx: &mut Ctx)`, and `RtcCntl::reset(&mut self, ctx: &mut Ctx, cause: ResetCause, system: bool)` (a reset took place: the next boot reads `cause`; `system` also returns the registers to their reset values); `test_support::stop_watchdogs() -> Vec<u32>`. A write of `OPTIONS0` with bit 31 calls `ctx.request_reset(ResetCause::SoftwareSystem)`, with bit 5 `ResetCause::SoftwareCpu`. The watchdog requests `RtcWatchdogCpu`, `RtcWatchdogCore`, `RtcWatchdogSystem`; the super watchdog `SuperWatchdog`. Source 27 (`RTC_CORE_INTR_MAP`) follows `INT_RAW & INT_ENA`.

- [ ] **Step 1: Write the failing tests**

Values: the flash-boot protection is 400 000 slow-clock cycles, 2.941 s, a system reset (TW §D.4); the bootloader programs 9 s (`WDTCONFIG1 = 0x956A0`, `WDTCONFIG0 = 0xC007EA00`); `esp_restart_noos` programs `WDTCONFIG1 = 0x109A0` and `WDTCONFIG2 = 0x21340` (TW §C.3); the keys are `0x50D83AA1` and `0x8F1D312A`, which are also the reset values of the two key registers (`IDF:soc/esp32c3/register/soc/rtc_cntl_reg.h:1700`, `:1758`; the SVD says 0). Replace the test module of `crates/soc-esp32c3/src/periph/rtc_cntl.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    const MILLISECOND: u64 = SECOND / 1_000;

    fn rtc(ctx: &mut Ctx) -> RtcCntl {
        let mut rtc = RtcCntl::new(true, 0, ctx.now());
        rtc.wake(ctx);
        rtc
    }

    /// Delivers every wake-up due at the present time, as the machine does.
    fn pump(ctx: &mut Ctx, rtc: &mut RtcCntl) {
        while let Some(event) = ctx.take_due() {
            assert_eq!(event, Event::RtcWatchdog);
            rtc.wake(ctx);
        }
    }

    fn after(ctx: &mut Ctx, rtc: &mut RtcCntl, cycles: u64) {
        ctx.sleep_until(ctx.now() + cycles);
        pump(ctx, rtc);
    }

    /// What `bootloader_config_wdt` writes (TW §C.3): the flash-boot protection off, one stage, a
    /// system reset after 9 s (612 000 cycles written, 1 224 000 counted).
    fn start_the_bootloader_watchdog(ctx: &mut Ctx, rtc: &mut RtcCntl) {
        for (offset, value) in [
            (WDTWPROTECT, WDT_KEY),
            (WDTCONFIG0, 0x0001_2214),
            (WDTCONFIG0, 0x0007_EA00),
            (WDTCONFIG0, 0x4007_EA00),
            (WDTCONFIG1, 0x0009_56A0),
            (WDTFEED, 0x8000_0000),
            (WDTCONFIG0, 0xC007_EA00),
            (WDTWPROTECT, 0),
        ] {
            assert!(rtc.write(ctx, offset, value), "{offset:#x}");
        }
    }

    #[test]
    fn power_on_reset_cause_is_reported() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        let state = rtc.read(ctx, RESET_STATE).unwrap();
        assert_eq!(state & 0x3F, 1, "RESET_CAUSE_PROCPU = POWERON");
        assert_eq!(state & (1 << 20), 0, "no JTAG reset flag");
        assert_eq!(
            state & !0x3F,
            0x3000,
            "the other bits keep their documented reset value"
        );
        assert!(!rtc.write(ctx, RESET_STATE, 0), "read-only");
    }

    #[test]
    fn store_registers_keep_what_the_rom_writes() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        for (i, offset) in STORE.into_iter().enumerate() {
            let value = 0x1000 + u32::try_from(i).unwrap();
            assert_eq!(rtc.read(ctx, offset), Some(0));
            assert!(rtc.write(ctx, offset, value));
            assert_eq!(rtc.read(ctx, offset), Some(value));
        }
    }

    #[test]
    fn configuration_is_stored_from_its_reset_value() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        for (name, reset) in [
            ("OPTIONS0", 0x1C00_A000),
            ("TIMER1", 0x2814_0403),
            ("ANA_CONF", 0x00C4_0000),
            ("CLK_CONF", 0x1158_3218),
            ("RTC_CNTL", 0xA000_0000),
            ("DIG_PWC", 0x0055_5010),
            ("BROWN_OUT", 0x43FF_0010),
        ] {
            assert_eq!(rtc.read(ctx, reg(name)), Some(reset), "{name}");
            assert!(rtc.write(ctx, reg(name), 0x0000_0140));
            assert_eq!(rtc.read(ctx, reg(name)), Some(0x0000_0140), "{name}");
        }
        assert_eq!(ctx.take_unsupported(), None);
        assert_eq!(
            rtc.read(ctx, reg("SLP_TIMER0")),
            None,
            "sleep is not modelled"
        );
        assert_eq!(rtc.read(ctx, reg("STATE0")), None);
    }

    #[test]
    fn a_software_reset_bit_asks_for_its_reset_and_the_rest_of_the_write_is_stored() {
        // IR §4.1: `software_reset_cpu` writes `OPTIONS0 | 0x20`; the other bits must survive.
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        assert!(rtc.write(ctx, OPTIONS0, 0x1C00_8000 | SW_PROCPU_RST));
        assert_eq!(ctx.take_reset(), Some(ResetCause::SoftwareCpu));
        assert_eq!(rtc.read(ctx, OPTIONS0), Some(0x1C00_8000), "bit 5 reads 0");
        assert!(rtc.write(ctx, OPTIONS0, 0x1C00_A000 | SW_SYS_RST));
        assert_eq!(ctx.take_reset(), Some(ResetCause::SoftwareSystem));
        assert_eq!(rtc.read(ctx, OPTIONS0), Some(0x1C00_A000));
        rtc.write(ctx, OPTIONS0, SW_SYS_RST | SW_PROCPU_RST);
        assert_eq!(
            ctx.take_reset(),
            Some(ResetCause::SoftwareSystem),
            "the wider one"
        );
        rtc.write(ctx, OPTIONS0, 0x1C00_A000 & !(1 << 13));
        assert_eq!(
            ctx.take_reset(),
            None,
            "BBPLL power bits are plain configuration"
        );
        assert_eq!(ctx.take_unsupported(), None);
    }

    #[test]
    fn the_cause_of_a_reset_is_what_the_next_boot_reads() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        rtc.write(ctx, STORE[4], 0x0028_0028);
        rtc.reset(ctx, ResetCause::SoftwareCpu, false);
        assert_eq!(rtc.read(ctx, RESET_STATE), Some(0x300C));
        assert_eq!(
            rtc.read(ctx, STORE[4]),
            Some(0x0028_0028),
            "kept by a CPU reset"
        );
        rtc.reset(ctx, ResetCause::RtcWatchdogSystem, true);
        assert_eq!(rtc.read(ctx, RESET_STATE), Some(0x3010));
        assert_eq!(
            rtc.read(ctx, STORE[4]),
            Some(0),
            "cleared by a system reset"
        );
    }

    #[test]
    fn the_rtc_timer_counts_slow_clock_cycles_and_keeps_two_captures() {
        // TW §C.2: 48 bits, 136 kHz, captured by TIME_UPDATE bit 31.
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        assert_eq!(rtc.read(ctx, TIME_LOW0), Some(0));
        after(ctx, &mut rtc, SECOND);
        assert_eq!(rtc.read(ctx, TIME_LOW0), Some(0), "no capture yet");
        assert!(rtc.write(ctx, TIME_UPDATE, 0x8000_0000));
        assert_eq!(rtc.read(ctx, TIME_UPDATE), Some(0), "write-only");
        assert_eq!(rtc.read(ctx, TIME_LOW0), Some(136_000));
        assert_eq!(rtc.read(ctx, TIME_HIGH0), Some(0));
        // 40 000 s more: past 32 bits of slow-clock cycles.
        after(ctx, &mut rtc, 40_000 * SECOND);
        rtc.write(ctx, TIME_UPDATE, 0x8000_0000);
        let cycles = 40_001u64 * 136_000;
        assert_eq!(
            rtc.read(ctx, TIME_LOW0),
            Some((cycles & 0xFFFF_FFFF) as u32)
        );
        assert_eq!(rtc.read(ctx, TIME_HIGH0), Some((cycles >> 32) as u32));
        assert_eq!(
            rtc.read(ctx, TIME_LOW1),
            Some(136_000),
            "the capture before"
        );
        assert_eq!(rtc.read(ctx, TIME_HIGH1), Some(0));
    }

    #[test]
    fn the_rtc_timer_survives_every_reset() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        rtc.write(ctx, WDTWPROTECT, WDT_KEY);
        rtc.write(ctx, WDTCONFIG0, 0);
        after(ctx, &mut rtc, SECOND);
        rtc.reset(ctx, ResetCause::RtcWatchdogSystem, true);
        rtc.write(ctx, TIME_UPDATE, 0x8000_0000);
        assert_eq!(rtc.read(ctx, TIME_LOW0), Some(136_000));
    }

    #[test]
    fn the_flash_boot_protection_resets_the_system_after_400_000_slow_cycles() {
        // TW §D.4: `WDTCONFIG1` = 200 000, doubled by the eFuse rule; 2.941 s at 136 kHz; cause 0x10.
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        assert_eq!(rtc.read(ctx, WDTCONFIG0), Some(0x0001_3214));
        assert_eq!(rtc.read(ctx, WDTCONFIG1), Some(200_000));
        assert_eq!(
            rtc.read(ctx, WDTWPROTECT),
            Some(0x50D8_3AA1),
            "unlocked out of reset"
        );
        assert_eq!(ctx.next_wake(), Some(Cycles(470_588_236)));
        after(ctx, &mut rtc, 470_588_235);
        assert_eq!(ctx.take_reset(), None);
        after(ctx, &mut rtc, 1);
        assert_eq!(ctx.take_reset(), Some(ResetCause::RtcWatchdogSystem));
    }

    #[test]
    fn a_core_reset_does_not_restart_the_watchdog() {
        // TW §D.2: the RTC watchdog keeps counting across a core reset of the main watchdog.
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        after(ctx, &mut rtc, 2 * SECOND);
        rtc.reset(ctx, ResetCause::Mwdt0Core, false);
        after(ctx, &mut rtc, SECOND);
        assert_eq!(
            ctx.take_reset(),
            Some(ResetCause::RtcWatchdogSystem),
            "2.94 s from power-on"
        );
    }

    #[test]
    fn the_bootloader_watchdog_resets_after_9_s_unless_it_is_fed() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        start_the_bootloader_watchdog(ctx, &mut rtc);
        assert_eq!(rtc.read(ctx, WDTCONFIG0), Some(0xC007_EA00), "TW §C.3");
        after(ctx, &mut rtc, 8 * SECOND);
        // `esp_clk_init`: unlock, feed, stage 0 <- 1.6 s (0x1A900), lock.
        rtc.write(ctx, WDTWPROTECT, WDT_KEY);
        rtc.write(ctx, WDTFEED, 0x8000_0000);
        rtc.write(ctx, WDTCONFIG1, 0x0001_A900);
        rtc.write(ctx, WDTWPROTECT, 0);
        after(ctx, &mut rtc, 1_599 * MILLISECOND);
        assert_eq!(ctx.take_reset(), None);
        after(ctx, &mut rtc, 2 * MILLISECOND);
        assert_eq!(ctx.take_reset(), Some(ResetCause::RtcWatchdogSystem));
    }

    #[test]
    fn a_locked_watchdog_ignores_feeds_and_a_feed_needs_bit_31() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        start_the_bootloader_watchdog(ctx, &mut rtc);
        after(ctx, &mut rtc, 5 * SECOND);
        rtc.write(ctx, WDTFEED, 0x8000_0000);
        rtc.write(ctx, WDTCONFIG0, 0);
        assert_eq!(rtc.read(ctx, WDTCONFIG0), Some(0xC007_EA00), "locked");
        rtc.write(ctx, WDTWPROTECT, WDT_KEY);
        rtc.write(ctx, WDTFEED, 1);
        after(ctx, &mut rtc, 4 * SECOND);
        assert_eq!(
            ctx.take_reset(),
            Some(ResetCause::RtcWatchdogSystem),
            "never fed"
        );
    }

    #[test]
    fn the_stages_esp_restart_programs_reset_the_core_then_the_system() {
        // IR §4.1: stage 0 action 3 after 68 000 << 1 cycles (1 s), stage 1 action 4 after 136 000.
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new(false, 0, ctx.now());
        rtc.wake(ctx);
        for (offset, value) in [
            (
                WDTCONFIG0,
                (ACTION_RESET_SYSTEM << 28) | (ACTION_RESET_RTC << 25),
            ),
            (WDTCONFIG1, 0x0001_09A0),
            (WDTCONFIG1 + 4, 0x0002_1340),
            (WDTFEED, 0x8000_0000),
            (
                WDTCONFIG0,
                WDT_EN | (ACTION_RESET_SYSTEM << 28) | (ACTION_RESET_RTC << 25),
            ),
        ] {
            rtc.write(ctx, offset, value);
        }
        after(ctx, &mut rtc, SECOND);
        assert_eq!(ctx.take_reset(), Some(ResetCause::RtcWatchdogCore));
        after(ctx, &mut rtc, SECOND);
        assert_eq!(ctx.take_reset(), Some(ResetCause::RtcWatchdogSystem));
    }

    #[test]
    fn an_interrupt_stage_raises_the_rtc_core_source_until_it_is_cleared() {
        // Source RTC_CORE is 27; the watchdog is bit 3 of the RTC interrupt registers.
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new(false, 0, ctx.now());
        rtc.wake(ctx);
        rtc.write(ctx, INT_ENA_W1TS, INT_WDT);
        rtc.write(ctx, WDTCONFIG1, 68_000);
        rtc.write(
            ctx,
            WDTCONFIG0,
            WDT_EN | (ACTION_INTERRUPT << 28) | (ACTION_RESET_CPU << 25),
        );
        after(ctx, &mut rtc, SECOND);
        assert_eq!(rtc.read(ctx, INT_ST), Some(INT_WDT));
        assert_eq!(ctx.sources(), 1 << 27);
        rtc.write(ctx, INT_CLR, INT_WDT);
        assert_eq!(ctx.sources(), 0);
        rtc.write(ctx, INT_ENA_W1TC, INT_WDT);
        assert_eq!(rtc.read(ctx, INT_ENA), Some(0));
        // Stage 1: 80 000 cycles, a CPU reset.
        after(ctx, &mut rtc, SECOND);
        assert_eq!(ctx.take_reset(), Some(ResetCause::RtcWatchdogCpu));
    }

    #[test]
    fn the_super_watchdog_never_fires_before_software_takes_it_over_nor_once_it_is_auto_fed() {
        // TW §C.4: the ROM phase shows no `rst:0x12`; the bootloader sets AUTO_FEED_EN.
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new(false, 0, ctx.now());
        rtc.wake(ctx);
        assert_eq!(rtc.read(ctx, SWD_CONF), Some(0x04B0_0000));
        assert_eq!(rtc.read(ctx, SWD_WPROTECT), Some(0x8F1D_312A));
        after(ctx, &mut rtc, 10 * SECOND);
        assert_eq!(ctx.take_reset(), None);
        // `bootloader_super_wdt_auto_feed`, then the clearing of FIB_SEL bit 2 and BYPASS_RST.
        rtc.write(ctx, SWD_WPROTECT, SWD_KEY);
        rtc.write(ctx, SWD_CONF, 0x04B0_0000 | SWD_AUTO_FEED_EN);
        rtc.write(ctx, SWD_WPROTECT, 0);
        rtc.write(ctx, FIB_SEL, 0b011);
        after(ctx, &mut rtc, 10 * SECOND);
        assert_eq!(ctx.take_reset(), None);
        rtc.write(ctx, SWD_CONF, 0x04B0_0000);
        assert_eq!(
            rtc.read(ctx, SWD_CONF),
            Some(0x04B0_0000 | SWD_AUTO_FEED_EN),
            "locked: the write is dropped"
        );
    }

    #[test]
    fn an_unfed_super_watchdog_asks_for_a_feed_then_resets_the_system() {
        let ctx = &mut Ctx::default();
        let mut rtc = RtcCntl::new(false, 0, ctx.now());
        rtc.wake(ctx);
        rtc.write(ctx, INT_ENA, INT_SWD);
        rtc.write(ctx, FIB_SEL, 0b011);
        after(ctx, &mut rtc, 900 * MILLISECOND);
        assert_eq!(
            rtc.read(ctx, SWD_CONF).map(|v| v & SWD_FEED_INT),
            Some(SWD_FEED_INT)
        );
        assert_eq!(ctx.sources(), 1 << 27);
        rtc.write(ctx, SWD_CONF, 0x04B0_0000 | SWD_FEED);
        assert_eq!(
            rtc.read(ctx, SWD_CONF),
            Some(0x04B0_0000),
            "FEED is write-only"
        );
        assert_eq!(ctx.sources(), 0);
        after(ctx, &mut rtc, 999 * MILLISECOND);
        assert_eq!(ctx.take_reset(), None, "fed 999 ms ago");
        after(ctx, &mut rtc, MILLISECOND);
        assert_eq!(ctx.take_reset(), Some(ResetCause::SuperWatchdog));
        // The next boot reads the flag, and clears it (`clear_super_wdt_reset_flag`).
        rtc.reset(ctx, ResetCause::SuperWatchdog, true);
        assert_eq!(rtc.read(ctx, RESET_STATE), Some(0x3012));
        assert_eq!(rtc.read(ctx, SWD_CONF), Some(0x04B0_0000 | SWD_RESET_FLAG));
        rtc.write(ctx, SWD_CONF, 0x04B0_0000 | SWD_RST_FLAG_CLR);
        assert_eq!(rtc.read(ctx, SWD_CONF), Some(0x04B0_0000));
    }

    #[test]
    fn a_slow_clock_other_than_the_rc_oscillator_is_refused_by_name() {
        let ctx = &mut Ctx::default();
        let mut rtc = rtc(ctx);
        assert!(rtc.write(ctx, CLK_CONF, 0x1158_3218));
        assert_eq!(ctx.take_unsupported(), None);
        assert!(rtc.write(ctx, CLK_CONF, 0x5158_3218));
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::SlowClock {
                clk_conf: 0x5158_3218
            })
        );
        assert_eq!(
            rtc.read(ctx, CLK_CONF),
            Some(0x5158_3218),
            "stored all the same"
        );
    }
}
```

The diagnostics (the refusal of a reset now names its cause, a software request or a watchdog), the tests that want a machine with nothing scheduled (the reset values now arm a watchdog), and one machine test of what this commit does with a reset request: it reports it by name and a strict run stops on it. Task 11 replaces that test with the reset itself.

```diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -509,8 +531,14 @@ mod tests {
                 sysclk_conf: 0xC00,
                 cpu_per_conf: 0xC,
             }),
-            unsupported(Unsupported::SoftwareReset {
-                options0: 0x8000_0000,
+            unsupported(Unsupported::Reset {
+                cause: ResetCause::SoftwareSystem,
+            }),
+            unsupported(Unsupported::Reset {
+                cause: ResetCause::RtcWatchdogSystem,
+            }),
+            unsupported(Unsupported::SlowClock {
+                clk_conf: 0x4000_0000,
             }),
             unsupported(Unsupported::UartOverflow { uart: 0 }),
             mapping(3, MappingFault::InvalidEntry),
@@ -577,10 +605,22 @@ mod tests {
                 "SYSCLK_CONF 0x00000c00",
             ),
             (
-                unsupported(Unsupported::SoftwareReset {
-                    options0: 0x8000_0000,
+                unsupported(Unsupported::Reset {
+                    cause: ResetCause::SoftwareCpu,
+                }),
+                "a software reset was requested through RTC_CNTL.OPTIONS0 (reset cause 0x0c",
+            ),
+            (
+                unsupported(Unsupported::Reset {
+                    cause: ResetCause::RtcWatchdogSystem,
+                }),
+                "the RTC watchdog expired (reset cause 0x10",
+            ),
+            (
+                unsupported(Unsupported::SlowClock {
+                    clk_conf: 0x4000_0000,
                 }),
-                "software reset",
+                "RTC slow clock",
             ),
             (mapping(3, MappingFault::InvalidEntry), "MMU entry 3"),
             (
```

```diff
--- a/crates/soc-esp32c3/src/periph/mod.rs
+++ b/crates/soc-esp32c3/src/periph/mod.rs
@@ -758,25 +763,6 @@ mod tests {
         assert_eq!(ctx.take_unsupported(), None);
     }
 
-    #[test]
-    fn a_software_reset_request_reaches_the_relay() {
-        let ctx = &mut Ctx::default();
-        let mut p = peripherals(ctx);
-        let options0 = svd_regs::esp32c3::addr("RTC_CNTL", "OPTIONS0");
-        let (device, base) = p.device(options0).expect("RTC_CNTL is routed");
-        assert!(device.write(ctx, options0 - base, 1 << 31));
-        assert!(device.write(ctx, options0 - base, 1 << 5));
-        assert_eq!(
-            ctx.take_unsupported(),
-            Some(Unsupported::SoftwareReset { options0: 1 << 31 })
-        );
-        assert_eq!(
-            ctx.take_unsupported(),
-            Some(Unsupported::SoftwareReset { options0: 1 << 5 })
-        );
-        assert_eq!(ctx.take_unsupported(), None);
-    }
-
     #[test]
     fn the_page_shared_by_rtc_cntl_and_the_efuse_splits_at_the_last_two_kib() {
         let ctx = &mut Ctx::default();
```

```diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -349,8 +349,8 @@ mod tests {
     use crate::memmap::ROM_BASE;
     use crate::memmap::SRAM_DBUS_BASE;
     use crate::test_support::{
-        HANG, T0, T1, flat_rom, load_register, print_program as print, store_register,
-        unmodeled_register,
+        HANG, T0, T1, flat_rom, load_register, print_program as print, stop_watchdogs,
+        store_register, unmodeled_register,
     };
     use cpu_rv32::asm::{addi, csrrs, csrrwi, jal, lui, lw, sw, wfi};
     use cpu_rv32::csr_esp32c3::{PCCR_USER, PCER_USER, PCMR_USER};
@@ -573,7 +573,13 @@ mod tests {
 
     #[test]
     fn wfi_with_nothing_to_wake_it_stops_as_idle() {
-        let mut m = machine(&[wfi(), HANG]);
+        // Strapped for another boot mode, no watchdog runs at reset: no device has anything
+        // scheduled.
+        let config = SocConfig {
+            strapping: 0,
+            ..SocConfig::default()
+        };
+        let mut m = machine_with(&[wfi(), HANG], config);
         assert_eq!(m.run_until(Cycles(10_000)), StopReason::Idle);
         assert_eq!(
             m.device_time_nanos(),
@@ -961,8 +967,9 @@ mod tests {
     #[test]
     fn a_cpu_that_sleeps_while_a_device_counts_gives_control_back_after_a_second() {
         // A one-shot alarm two seconds ahead (32 000 000 counts at 16 MHz), its interrupt not
-        // enabled: nothing will wake the CPU, and time must still pass.
-        let mut body = Vec::new();
+        // enabled: nothing will wake the CPU, and time must still pass. The RTC watchdog the reset
+        // values arm is stopped first: it would reset the chip at 2.94 s.
+        let mut body = stop_watchdogs();
         body.extend(store_register(reg("SYSTIMER", "TARGET0_LO"), 32_000_000));
         body.extend(store_register(reg("SYSTIMER", "COMP0_LOAD"), 1));
         body.extend(store_register(
@@ -1017,4 +1022,49 @@ mod tests {
         assert_eq!(m.run_until(Cycles(10_000)), StopReason::Asleep, "and again");
         assert!(m.device_time_nanos() > first);
     }
+
+    #[test]
+    fn a_reset_request_is_reported_by_name_and_stops_a_strict_run() {
+        use crate::reset::ResetCause;
+        let reported = |m: &mut Esp32C3, cause: ResetCause| {
+            m.drain_diagnostics().into_iter().any(|diagnostic| {
+                matches!(
+                    diagnostic,
+                    Diagnostic::Unsupported { what: Unsupported::Reset { cause: seen }, .. }
+                        if seen == cause
+                )
+            })
+        };
+        // What `esp_restart` ends with: `RTC_CNTL.OPTIONS0` bit 5 (IR §4.1).
+        let mut restart = stop_watchdogs();
+        restart.extend(store_register(reg("RTC_CNTL", "OPTIONS0"), 1 << 5));
+        restart.push(HANG);
+        let mut m = machine(&restart);
+        assert_eq!(m.run_until(Cycles(1_000)), StopReason::CycleBudget);
+        assert!(reported(&mut m, ResetCause::SoftwareCpu));
+        let mut strict = machine_with(
+            &restart,
+            SocConfig {
+                strict_mmio: true,
+                ..SocConfig::default()
+            },
+        );
+        assert!(matches!(
+            strict.run_until(Cycles(1_000)),
+            StopReason::StrictMmio(Diagnostic::Unsupported {
+                what: Unsupported::Reset {
+                    cause: ResetCause::SoftwareCpu
+                },
+                ..
+            })
+        ));
+        // A CPU that only sleeps: the RTC watchdog the reset values arm expires after 400 000
+        // cycles of the 136 kHz slow clock, 2.941 s (TW §D.4).
+        let mut asleep = machine(&[wfi(), HANG]);
+        for _ in 0..3 {
+            assert_eq!(asleep.run_until(Cycles(10_000)), StopReason::Asleep);
+        }
+        assert!(asleep.device_time_nanos() >= 2_941_176_470);
+        assert!(reported(&mut asleep, ResetCause::RtcWatchdogSystem));
+    }
 }
```

```diff
--- a/crates/emu-cli/src/run.rs
+++ b/crates/emu-cli/src/run.rs
@@ -205,7 +205,7 @@ mod tests {
     use boards::MachineOptions;
     use cpu_rv32::asm::{addi, jal, sw, wfi};
     use soc_esp32c3::test_support::{
-        HANG, T0, T1, flat_rom, load_register, print_program, unmodeled_register,
+        HANG, T0, T1, flat_rom, load_register, print_program, stop_watchdogs, unmodeled_register,
     };
     const UART0: u32 = svd_regs::esp32c3::addr("UART0", "FIFO");
     const USB: u32 = svd_regs::esp32c3::addr("USB_DEVICE", "EP1");
@@ -404,7 +404,10 @@ mod tests {
 
     #[test]
     fn idle_cpu_ends_the_run_and_says_why() {
-        let mut m = machine(&[wfi(), HANG], MachineOptions::default());
+        // With its watchdogs stopped, nothing on the device has anything scheduled.
+        let mut program = stop_watchdogs();
+        program.extend([wfi(), HANG]);
+        let mut m = machine(&program, MachineOptions::default());
         let (mut console, mut log) = (Vec::new(), Vec::new());
         let end = execute(
             &mut m,
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p soc-esp32c3 --lib > /tmp/t10.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, compile errors: `RtcCntl::new` takes 0 arguments, no variant `SlowClock`, no variant `Reset`, `cannot find function 'stop_watchdogs'`.

- [ ] **Step 3: The event, the diagnostic, the eFuse field, the fixture**

```diff
--- a/crates/soc-esp32c3/src/ctx.rs
+++ b/crates/soc-esp32c3/src/ctx.rs
@@ -50,6 +50,8 @@ pub enum Event {
     Systimer,
     /// Wake-up of UART 0 or 1: its transmitter may have gone idle.
     Uart(u8),
+    /// Wake-up of `RTC_CNTL`: the RTC watchdog may have reached a stage timeout.
+    RtcWatchdog,
 }
 
 /// The wake-up a device keeps for its next deadline.
```

```diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -1,4 +1,5 @@
 //! What the emulator has to tell the user about the run: accesses it does not model, CPU exceptions.
+use crate::reset::ResetCause;
 use cpu_rv32::Exception;
 use std::collections::BTreeMap;
 use std::fmt;
@@ -32,8 +33,11 @@ pub enum Unsupported {
     ShaMode { mode: u32 },
     /// A clock selection without a known CPU frequency (D22).
     ClockSource { sysclk_conf: u32, cpu_per_conf: u32 },
-    /// A reset requested through `RTC_CNTL.OPTIONS0` (D27).
-    SoftwareReset { options0: u32 },
+    /// A reset the machine cannot perform yet (D27): asked for through `RTC_CNTL.OPTIONS0`, or
+    /// by a watchdog that expired.
+    Reset { cause: ResetCause },
+    /// `RTC_CNTL.CLK_CONF` selects a slow clock other than the RC oscillator, the only one modelled.
+    SlowClock { clk_conf: u32 },
     /// A byte written to a UART whose transmit FIFO was full: the hardware drops it too.
     UartOverflow { uart: u8 },
 }
@@ -77,9 +81,26 @@ impl Unsupported {
                 f,
                 "the clock registers select a CPU clock inkemu cannot model (SYSCLK_CONF {sysclk_conf:#010x}, CPU_PER_CONF {cpu_per_conf:#010x}, pc {pc:#010x}), so emulated time keeps the previous frequency; select the crystal or the PLL at 80 or 160 MHz"
             ),
-            Unsupported::SoftwareReset { options0 } => write!(
+            Unsupported::Reset { cause } => write!(
                 f,
-                "a software reset was requested through RTC_CNTL.OPTIONS0 (value {options0:#010x}, pc {pc:#010x}) but resets are not emulated yet, so the write was ignored; restart inkemu to reset the device"
+                "{} (reset cause {:#04x}, pc {pc:#010x}) but resets are not emulated yet, so nothing was reset; restart inkemu to reset the device",
+                match cause {
+                    ResetCause::SoftwareSystem | ResetCause::SoftwareCpu =>
+                        "a software reset was requested through RTC_CNTL.OPTIONS0",
+                    ResetCause::RtcWatchdogCpu
+                    | ResetCause::RtcWatchdogCore
+                    | ResetCause::RtcWatchdogSystem => "the RTC watchdog expired",
+                    ResetCause::SuperWatchdog => "the super watchdog expired",
+                    ResetCause::Mwdt0Cpu
+                    | ResetCause::Mwdt0Core
+                    | ResetCause::Mwdt1Cpu
+                    | ResetCause::Mwdt1Core => "a main watchdog expired",
+                },
+                cause.code()
+            ),
+            Unsupported::SlowClock { clk_conf } => write!(
+                f,
+                "RTC_CNTL.CLK_CONF selects an RTC slow clock inkemu does not model (value {clk_conf:#010x}, pc {pc:#010x}), so the RTC timer and the RTC watchdog keep counting the 136 kHz RC oscillator; select the RC oscillator (ANA_CLK_RTC_SEL = 0) or expect their durations to differ from the device"
             ),
             Unsupported::UartOverflow { uart } => write!(
                 f,
@@ -157,7 +178,8 @@ impl Diagnostic {
                     sysclk_conf,
                     cpu_per_conf,
                 } => (8, sysclk_conf, cpu_per_conf),
-                Unsupported::SoftwareReset { .. } => (9, 0, 0),
+                Unsupported::Reset { cause } => (13, cause.code(), 0),
+                Unsupported::SlowClock { .. } => (9, 0, 0),
                 Unsupported::UartOverflow { uart } => (12, u32::from(uart), 0),
                 // A command is 1 to 16 bits long, so 0 is free for "no command phase".
                 Unsupported::FlashTransfer { command_bits, .. } => {
```

```diff
--- a/crates/soc-esp32c3/src/periph/efuse.rs
+++ b/crates/soc-esp32c3/src/periph/efuse.rs
@@ -17,6 +17,10 @@ const fn reg(name: &str) -> u32 {
 /// First and last register of the read mirror: `RD_WR_DIS` to `RD_RS_ERR1`.
 const RD_FIRST: u32 = reg("RD_WR_DIS");
 const RD_LAST: u32 = reg("RD_RS_ERR1");
+/// `RD_REPEAT_DATA1` bits [17:16]: `WDT_DELAY_SEL`.
+const RD_REPEAT_DATA1: u32 = reg("RD_REPEAT_DATA1");
+const WDT_DELAY_SEL_SHIFT: u32 = 16;
+const WDT_DELAY_SEL_MASK: u32 = 0b11;
 /// `RD_MAC_SPI_SYS_3` bits [20:18]: `WAFER_VERSION_MINOR_LO`.
 const RD_MAC_SPI_SYS_3: u32 = reg("RD_MAC_SPI_SYS_3");
 const WAFER_MINOR_LO_SHIFT: u32 = 18;
@@ -101,6 +105,18 @@ impl Efuse {
             .collect();
         Efuse { read_mirror }
     }
+
+    /// `WDT_DELAY_SEL`, `RD_REPEAT_DATA1[17:16]`: the multiplier of the stage 0 timeout of the RTC
+    /// watchdog (`IDF:hal/esp32c3/include/hal/rwdt_ll.h:119`; report TW §C.2).
+    #[must_use]
+    pub fn wdt_delay_sel(&self) -> u32 {
+        let word = self
+            .read_mirror
+            .iter()
+            .find(|&&(offset, _)| offset == RD_REPEAT_DATA1)
+            .map_or(0, |&(_, value)| value);
+        (word >> WDT_DELAY_SEL_SHIFT) & WDT_DELAY_SEL_MASK
+    }
 }
 
 impl Peripheral<Ctx> for Efuse {
```

```diff
--- a/crates/soc-esp32c3/src/test_support.rs
+++ b/crates/soc-esp32c3/src/test_support.rs
@@ -353,3 +353,18 @@ pub fn store_register(addr: u32, value: u32) -> [u32; 4] {
         asm::sw(T1, T0, offset),
     ]
 }
+
+/// What a program does first when it must outlive the flash-boot protection: it clears
+/// `WDTCONFIG0` of main watchdog 0 and of the RTC watchdog, as the bootloader does
+/// (`bootloader_config_wdt`). Both are unlocked at reset: their key registers reset to the key.
+#[must_use]
+pub fn stop_watchdogs() -> Vec<u32> {
+    let mut program = Vec::new();
+    for (peripheral, register) in [("TIMG0", "WDTCONFIG0"), ("RTC_CNTL", "WDTCONFIG0")] {
+        program.extend(store_register(
+            svd_regs::esp32c3::addr(peripheral, register),
+            0,
+        ));
+    }
+    program
+}
```

- [ ] **Step 4: The block**

Replace everything above the test module of `crates/soc-esp32c3/src/periph/rtc_cntl.rs` with:

```rust
//! `RTC_CNTL`: the reset cause and the software resets, the retention registers, the RTC timer,
//! the RTC watchdog and the super watchdog, the RTC interrupts, and pad, clock, analog and power
//! configuration. Sources: TRM §6.1, §9.3, §12.2, §12.3, ESP-IDF `rtc_cntl_reg.h`, `rwdt_ll.h`,
//! `rtc_cntl_ll.h`; reports BC §E.2, IR §4, TW §C and §D.
//!
//! This block is in the RTC power domain: a CPU reset and a core reset leave it as it is, RTC
//! watchdog counter included; a system reset returns its registers to their reset values; nothing
//! but a power-on stops the RTC timer (TRM §9.3.3; TW §D.2).
use crate::ctx::{Ctx, Deadline, Event, Source};
use crate::diag::Unsupported;
use crate::periph::counter::{Counter, Rate};
use crate::periph::{Accepted, RTC_CNTL_BASE, Storage, half_of};
use crate::reset::{CAUSE_POWER_ON, ResetCause};
use emu_core::{Cycles, Peripheral};
use svd_regs::esp32c3::addr;

const fn reg(name: &str) -> u32 {
    addr("RTC_CNTL", name) - RTC_CNTL_BASE
}

const OPTIONS0: u32 = reg("OPTIONS0");
/// `OPTIONS0` bit 31 `SW_SYS_RST` (`ROM3:software_reset@0x40048b74`) and bit 5 `SW_PROCPU_RST`
/// (`ROM3:software_reset_cpu@0x40048b84`, the last store of `esp_restart`): each resets when it is
/// written 1 and reads 0 (report IR §4.1, §8 item 14).
const SW_SYS_RST: u32 = 1 << 31;
const SW_PROCPU_RST: u32 = 1 << 5;
const TIME_UPDATE: u32 = reg("TIME_UPDATE");
const TIME_LOW0: u32 = reg("TIME_LOW0");
const TIME_HIGH0: u32 = reg("TIME_HIGH0");
const TIME_LOW1: u32 = reg("TIME_LOW1");
const TIME_HIGH1: u32 = reg("TIME_HIGH1");
/// `TIME_UPDATE` bit 31, write-only: captures the RTC timer (
/// `rtc_cntl_ll_get_rtc_time`, `IDF:hal/esp32c3/include/hal/rtc_cntl_ll.h:74-80`).
const TIME_UPDATE_TRIGGER: u32 = 1 << 31;
/// The RTC timer is 48 bits wide: 16 bits in `TIME_HIGH0` (TRM §9.3.3).
const RTC_TIME_MASK: u64 = (1 << 48) - 1;
const RESET_STATE: u32 = reg("RESET_STATE");
const INT_ENA: u32 = reg("INT_ENA_RTC");
const INT_RAW: u32 = reg("INT_RAW_RTC");
const INT_ST: u32 = reg("INT_ST_RTC");
const INT_CLR: u32 = reg("INT_CLR_RTC");
const INT_ENA_W1TS: u32 = reg("INT_ENA_RTC_W1TS");
const INT_ENA_W1TC: u32 = reg("INT_ENA_RTC_W1TC");
/// `INT_RAW_RTC` bit 3: the RTC watchdog (`rwdt_ll_set_intr_enable`,
/// `IDF:hal/esp32c3/include/hal/rwdt_ll.h:297`); bit 15: the feed request of the super watchdog
/// (`RTC_CNTL_SWD_INT_RAW`, `rtc_cntl_reg.h:639`).
const INT_WDT: u32 = 1 << 3;
const INT_SWD: u32 = 1 << 15;
const CLK_CONF: u32 = reg("CLK_CONF");
/// `CLK_CONF` bits [31:30] `ANA_CLK_RTC_SEL`: the slow clock, 0 = the RC oscillator
/// (`rtc_cntl_reg.h:971`; `clk_ll_rtc_slow_set_src`).
const ANA_CLK_RTC_SEL_SHIFT: u32 = 30;
const WDTCONFIG0: u32 = reg("WDTCONFIG0");
const WDTCONFIG1: u32 = reg("WDTCONFIG1");
const WDTCONFIG4: u32 = reg("WDTCONFIG4");
const WDTFEED: u32 = reg("WDTFEED");
const WDTWPROTECT: u32 = reg("WDTWPROTECT");
const SWD_CONF: u32 = reg("SWD_CONF");
const SWD_WPROTECT: u32 = reg("SWD_WPROTECT");
const FIB_SEL: u32 = reg("FIB_SEL");

/// The RTC slow clock: the 136 kHz RC oscillator, 17 cycles every 20 000 cycles of the time base.
///
/// ASSUMPTION: the oscillator runs at the nominal frequency ESP-IDF computes every timeout from
/// (`SOC_CLK_RC_SLOW_FREQ_APPROX`, `IDF:soc/esp32c3/include/soc/clk_tree_defs.h`), from power-on. A
/// real chip deviates from it, and runs at another, undocumented frequency until the bootloader
/// has written `SCK_DCAP` (TW §C.1, §I question 2). A measurement on a real unit settles it.
const SLOW_CLOCK: Rate = Rate::ratio(17, 20_000);
/// One second of device time, in cycles of the time base.
const SECOND: u64 = 160_000_000;

// Reset values of the RTC watchdog registers (`rtc_cntl_reg.h:1565-1705`; TW §C.3).
/// `FLASHBOOT_MOD_EN` and `PAUSE_IN_SLP` set, reset lengths 1, chip reset width 20.
const WDTCONFIG0_RESET: u32 = 0x0001_3214;
/// Stage timeouts 0 to 3: 200 000, 80 000, 0xFFF, 0xFFF cycles of the slow clock.
const WDT_TIMEOUT_RESET: [u32; WDT_STAGES] = [200_000, 80_000, 0xFFF, 0xFFF];
/// The key that unlocks the watchdog registers, and the reset value of `WDTWPROTECT`
/// (`RTC_CNTL_WDT_WKEY`, default `0x50d83aa1`, `rtc_cntl_reg.h:1700`): unlocked out of reset. The
/// SVD gives 0 for this register and is wrong on this point.
const WDT_KEY: u32 = 0x50D8_3AA1;
const WDT_STAGES: usize = 4;
/// `WDTCONFIG0`: bit 31 `EN` (`rwdt_ll_enable`, `rwdt_ll.h:66`), three bits per stage from
/// [30:28] for stage 0 to [21:19] for stage 3 (`rwdt_ll_config_stage`, line 113), bit 12
/// `FLASHBOOT_MOD_EN` (`rwdt_ll_set_flashboot_en`, line 196). The rest is stored.
const WDT_EN: u32 = 1 << 31;
const WDT_STAGE0_SHIFT: u32 = 28;
const WDT_STAGE_BITS: u32 = 3;
const WDT_STAGE_MASK: u32 = 0b111;
const WDT_FLASHBOOT_MOD_EN: u32 = 1 << 12;
/// `WDTFEED` bit 31 feeds (`rwdt_ll_feed`, line 266).
const WDT_FEED: u32 = 1 << 31;
/// Stage actions (`IDF:hal/include/hal/wdt_types.h`; TW §D.5).
const ACTION_INTERRUPT: u32 = 1;
const ACTION_RESET_CPU: u32 = 2;
const ACTION_RESET_SYSTEM: u32 = 3;
const ACTION_RESET_RTC: u32 = 4;

/// `SWD_CONF` (reset `0x04B00000`): bit 31 `AUTO_FEED_EN`, bit 30 `DISABLE`, bit 29 `FEED` and bit
/// 28 `RST_FLAG_CLR` (write-only), bits [27:18] `SIGNAL_WIDTH`, bit 17 `BYPASS_RST`, bit 1
/// `FEED_INT` and bit 0 `RESET_FLAG` (read-only) (`rtc_cntl_reg.h:1706-1757`). The bootloader
/// clears `BYPASS_RST` and sets `AUTO_FEED_EN` (`IDF:bootloader_support/src/esp32c3/bootloader_soc.c:10-19`,
/// `bootloader_esp32c3.c:79-84`); the ROM reads `RESET_FLAG` and clears it with `RST_FLAG_CLR`
/// (`ROM3:analog_super_wdt_reset_happened@0x40048a26`, `clear_super_wdt_reset_flag@0x40048b58`).
const SWD_CONF_RESET: u32 = 0x04B0_0000;
const SWD_AUTO_FEED_EN: u32 = 1 << 31;
const SWD_DISABLE: u32 = 1 << 30;
const SWD_FEED: u32 = 1 << 29;
const SWD_RST_FLAG_CLR: u32 = 1 << 28;
const SWD_BYPASS_RST: u32 = 1 << 17;
const SWD_FEED_INT: u32 = 1 << 1;
const SWD_RESET_FLAG: u32 = 1 << 0;
const SWD_CONF_STORED: u32 = 0xCFFE_0000;
/// The key of the super watchdog, and the reset value of `SWD_WPROTECT` (`RTC_CNTL_SWD_WKEY`,
/// default `0x8f1d312a`, `rtc_cntl_reg.h:1758`). The SVD gives 0 and is wrong on this point too.
const SWD_KEY: u32 = 0x8F1D_312A;
/// `FIB_SEL` bit 2, `RTC_CNTL_FIB_SUPER_WDT_RST` (`rtc_cntl_reg.h:2341`), reset value 1.
const FIB_SUPER_WDT_RST: u32 = 1 << 2;
/// ASSUMPTION: the super watchdog asks to be fed after 0.9 s and resets after 1 s. The TRM gives
/// "slightly less than one second" for the period and "about 100 ms" before it for the request
/// (§12.3; TW §C.4); the circuit is analog and no figure is published.
const SWD_FEED_REQUEST: u64 = 9 * SECOND / 10;
const SWD_TIMEOUT: u64 = SECOND;

const STORE_COUNT: usize = 8;
/// `RTC_CNTL_STORE0..7`: they survive a reset; the ROM keeps the APB frequency in STORE5.
const STORE: [u32; STORE_COUNT] = [
    addr("RTC_CNTL", "STORE0") - RTC_CNTL_BASE,
    addr("RTC_CNTL", "STORE1") - RTC_CNTL_BASE,
    addr("RTC_CNTL", "STORE2") - RTC_CNTL_BASE,
    addr("RTC_CNTL", "STORE3") - RTC_CNTL_BASE,
    addr("RTC_CNTL", "STORE4") - RTC_CNTL_BASE,
    addr("RTC_CNTL", "STORE5") - RTC_CNTL_BASE,
    addr("RTC_CNTL", "STORE6") - RTC_CNTL_BASE,
    addr("RTC_CNTL", "STORE7") - RTC_CNTL_BASE,
];
/// Configuration the ROM, the bootloader and the application write and read back. Nothing acts on
/// it: there is no analog part, no power domain and no sleep in this model.
const ACCEPTED: [Accepted; 18] = [
    Accepted::svd(
        "RTC_CNTL",
        "OPTIONS0",
        "BBPLL, crystal and I2C power forcing, CPU stall (the two software reset bits are reported)",
        "SP1b-3b",
    ),
    Accepted::svd(
        "RTC_CNTL",
        "TIMER1",
        "clock and PLL wake-up delays",
        "SP1b-3b",
    ),
    Accepted::svd("RTC_CNTL", "TIMER3", "Wi-Fi/BT power-up timing", "SP1b-3b"),
    Accepted::svd("RTC_CNTL", "TIMER4", "digital power-up timing", "SP1b-3b"),
    Accepted::svd("RTC_CNTL", "TIMER5", "minimum sleep time", "SP1b-3b"),
    Accepted::svd(
        "RTC_CNTL",
        "TIMER6",
        "peripheral power-up timing",
        "SP1b-3b",
    ),
    Accepted::svd(
        "RTC_CNTL",
        "ANA_CONF",
        "analog configuration, glitch reset",
        "SP8",
    ),
    Accepted::svd(
        "RTC_CNTL",
        "CLK_CONF",
        "RTC fast clock source and dividers (a slow clock other than the RC oscillator is refused)",
        "SP8",
    ),
    Accepted::svd("RTC_CNTL", "SLOW_CLK_CONF", "RTC slow clock divider", "SP8"),
    Accepted::svd(
        "RTC_CNTL",
        "RTC_CNTL",
        "RTC regulator and digital bias",
        "SP8",
    ),
    Accepted::svd(
        "RTC_CNTL",
        "DIG_PWC",
        "digital power-domain control",
        "SP1b-3b",
    ),
    Accepted::svd(
        "RTC_CNTL",
        "BROWN_OUT",
        "brown-out detector: the supply is always good",
        "SP2",
    ),
    Accepted::svd(
        "RTC_CNTL",
        "FIB_SEL",
        "brown-out and glitch reset selection (the super watchdog bit acts)",
        "SP2",
    ),
    Accepted::svd("RTC_CNTL", "SENSOR_CTRL", "SAR ADC power forcing", "SP2"),
    Accepted::svd("RTC_CNTL", "PWC", "RTC power-domain control", "SP1b-3b"),
    Accepted::svd("RTC_CNTL", "DIG_ISO", "digital pad isolation", "SP1b-3b"),
    Accepted::svd("RTC_CNTL", "PAD_HOLD", "pad hold", "SP2"),
    Accepted::svd("RTC_CNTL", "DIG_PAD_HOLD", "digital pad hold", "SP2"),
];

pub(crate) fn accepted() -> Vec<Accepted> {
    ACCEPTED.to_vec()
}

/// `RESET_CAUSE_PROCPU`, bits [5:0] of `RESET_STATE` (`soc/reset_reasons.h`).
const RESET_CAUSE_MASK: u32 = 0x3F;

#[derive(Clone, Debug)]
pub struct RtcCntl {
    /// Whether the chip boots from flash: the strapping the flash-boot protection depends on.
    flash_boot: bool,
    /// `EFUSE_WDT_DELAY_SEL`: stage 0 of the watchdog lasts `WDTCONFIG1 << (1 + this)` cycles.
    wdt_delay_sel: u32,
    reset_cause: u32,
    /// The bits of `RESET_STATE` other than the cause, at their documented reset value.
    reset_state_other: u32,
    store: [u32; STORE_COUNT],
    config: Storage,
    /// The RTC timer: slow-clock cycles since power-on.
    timer: Counter,
    /// The last capture, and the one before it.
    time: [u64; 2],
    int_ena: u32,
    int_raw: u32,
    wdt_config0: u32,
    wdt_timeout: [u32; WDT_STAGES],
    wdt_key: u32,
    /// Slow-clock cycles since the last feed or the start of the current stage.
    wdt: Counter,
    wdt_stage: usize,
    wdt_enabled: bool,
    swd_conf: u32,
    swd_key: u32,
    /// Device time of the last feed of the super watchdog, while it is armed.
    swd_fed_at: Option<Cycles>,
    wake: Deadline,
}

/// The RTC interrupts share one source, `RTC_CORE` (27: report IR §3.2).
const RTC_CORE: Source = Source::of("RTC_CORE_INTR_MAP");

impl RtcCntl {
    /// The block after a power-on. `flash_boot` says whether the chip was strapped to boot from
    /// flash, `wdt_delay_sel` is the eFuse field of that name.
    ///
    /// The owner calls [`RtcCntl::wake`] once afterwards: the reset values arm the flash-boot
    /// protection of the RTC watchdog, which needs its first deadline.
    #[must_use]
    pub fn new(flash_boot: bool, wdt_delay_sel: u32, now: Cycles) -> RtcCntl {
        let mut rtc = RtcCntl {
            flash_boot,
            wdt_delay_sel,
            reset_cause: CAUSE_POWER_ON,
            reset_state_other: 0,
            store: [0; STORE_COUNT],
            config: Storage::accepted(RTC_CNTL_BASE, &ACCEPTED),
            timer: Counter::new(0, now, SLOW_CLOCK, true),
            time: [0; 2],
            int_ena: 0,
            int_raw: 0,
            wdt_config0: 0,
            wdt_timeout: WDT_TIMEOUT_RESET,
            wdt_key: 0,
            wdt: Counter::new(0, now, SLOW_CLOCK, false),
            wdt_stage: 0,
            wdt_enabled: false,
            swd_conf: 0,
            swd_key: 0,
            swd_fed_at: None,
            wake: Deadline::default(),
        };
        rtc.reset_registers();
        rtc
    }

    /// Returns every register to its reset value. The RTC timer and the reset cause are no
    /// registers of this kind: they stay.
    fn reset_registers(&mut self) {
        let documented =
            svd_regs::esp32c3::lookup(RTC_CNTL_BASE + RESET_STATE).map_or(0, |r| r.reset_value);
        self.reset_state_other = documented & !RESET_CAUSE_MASK;
        // ASSUMPTION: a system reset clears `STORE0..7`. The TRM says it resets "the whole digital
        // system, including RTC" and does not list the retention registers (TW §I question 9); the
        // registers read after a `rst:0x10` on a real chip settle it.
        self.store = [0; STORE_COUNT];
        self.config = Storage::accepted(RTC_CNTL_BASE, &ACCEPTED);
        self.time = [0; 2];
        self.int_ena = 0;
        self.int_raw = 0;
        self.wdt_config0 = WDTCONFIG0_RESET;
        self.wdt_timeout = WDT_TIMEOUT_RESET;
        self.wdt_key = WDT_KEY;
        self.wdt_stage = 0;
        self.wdt_enabled = false;
        self.swd_conf = SWD_CONF_RESET;
        self.swd_key = SWD_KEY;
        self.swd_fed_at = None;
    }

    /// A reset took place: the next boot reads its cause in `RESET_STATE`. A system reset also
    /// returns the registers of this block to their reset values (TW §D.2).
    pub fn reset(&mut self, ctx: &mut Ctx, cause: ResetCause, system: bool) {
        if system {
            let swd_reset = self.swd_conf & SWD_RESET_FLAG;
            self.reset_registers();
            // The flag that says the super watchdog caused the reset is what the next boot reads.
            self.swd_conf |= swd_reset;
        }
        self.reset_cause = cause.code();
        self.wake(ctx);
    }

    fn rtc_time(&self, now: Cycles) -> u64 {
        self.timer.value(now) & RTC_TIME_MASK
    }

    /// Whether the flash-boot protection runs the watchdog (TRM §12.2.2.4; TW §D.4).
    ///
    /// ASSUMPTION: as for the main watchdog, the protection is off when the chip is not strapped to
    /// boot from flash (TW §I question 4).
    const fn flash_boot_protection(&self) -> bool {
        self.flash_boot && self.wdt_config0 & WDT_FLASHBOOT_MOD_EN != 0
    }

    /// The action of a stage.
    ///
    /// ASSUMPTION: under the flash-boot protection stage 0 is a system reset after the timeout
    /// `WDTCONFIG1` holds, whatever `STG0` says. The TRM gives the action (§12.2.2.4); that the
    /// timeout is the programmed one matters after `esp_restart`, which programs 1 s and sets the
    /// protection bit without `EN` (`IDF:esp_system/port/soc/esp32c3/system_internal.c:71-80`; TW
    /// §C.3, §I question 3). A slow second boot on a real chip settles it.
    fn stage_action(&self, stage: usize) -> u32 {
        if stage == 0 && self.flash_boot_protection() {
            return ACTION_RESET_RTC;
        }
        // INVARIANT: `stage` is below `WDT_STAGES` (4), so the shift stays inside the register.
        let shift = WDT_STAGE0_SHIFT - WDT_STAGE_BITS * u32::try_from(stage).unwrap_or(0);
        (self.wdt_config0 >> shift) & WDT_STAGE_MASK
    }

    /// The timeout of a stage, in slow-clock cycles. Stage 0 lasts `WDTCONFIG1 << (1 + delay)`,
    /// which is why `rwdt_ll_config_stage` writes its value shifted the other way (`rwdt_ll.h:119`;
    /// TRM §12.2.2.2; TW §C.3).
    fn stage_timeout(&self, stage: usize) -> u64 {
        let written = u64::from(self.wdt_timeout[stage]);
        if stage == 0 {
            written << (1 + self.wdt_delay_sel)
        } else {
            written
        }
    }

    /// Whether the super watchdog can reset the chip.
    ///
    /// ASSUMPTION: its reset is out of software's hands, and inactive, until `FIB_SEL` bit 2 is
    /// cleared; `BYPASS_RST` and `DISABLE` switch it off, and `AUTO_FEED_EN` makes the controller
    /// feed it for ever. This reading comes from the bootloader code that hands the reset to
    /// software (`bootloader_soc.c:10-19`) and from the absence of any `rst:0x12` in the logs of
    /// chips that never reach the bootloader (TW §C.4, §I question 11).
    fn swd_armed(&self) -> bool {
        self.config.read(FIB_SEL).unwrap_or(FIB_SUPER_WDT_RST) & FIB_SUPER_WDT_RST == 0
            && self.swd_conf & (SWD_AUTO_FEED_EN | SWD_DISABLE | SWD_BYPASS_RST) == 0
    }

    /// Raises what is due at the present time, then asks to be woken for the next deadline.
    fn evaluate(&mut self, ctx: &mut Ctx) {
        let now = ctx.now();
        self.wake.called(now);
        let mut earliest: Option<Cycles> = None;
        let mut sooner = |at: Option<Cycles>| {
            if let Some(at) = at {
                earliest = Some(earliest.map_or(at, |e: Cycles| e.min(at)));
            }
        };

        let enabled = self.wdt_config0 & WDT_EN != 0 || self.flash_boot_protection();
        if enabled && !self.wdt_enabled {
            // ASSUMPTION: a watchdog that is switched on counts its first stage from 0 (see
            // `Timg::retime`).
            self.wdt.set(0, now);
            self.wdt_stage = 0;
        }
        self.wdt_enabled = enabled;
        self.wdt.run(enabled, now);
        // A long sleep can cover several stages: each one starts when the one before it ends.
        while self.wdt.is_running() {
            let timeout = self.stage_timeout(self.wdt_stage);
            let count = self.wdt.value(now);
            // ASSUMPTION: at or past the timeout, as for the main watchdog (see `Timg::evaluate`).
            if count < timeout {
                sooner(self.wdt.instant_in(timeout - count, now));
                break;
            }
            let action = self.stage_action(self.wdt_stage);
            self.wdt.set(count - timeout, now);
            self.wdt_stage = (self.wdt_stage + 1) % WDT_STAGES;
            match action {
                ACTION_INTERRUPT => self.int_raw |= INT_WDT,
                ACTION_RESET_CPU => ctx.request_reset(ResetCause::RtcWatchdogCpu),
                ACTION_RESET_SYSTEM => ctx.request_reset(ResetCause::RtcWatchdogCore),
                ACTION_RESET_RTC => ctx.request_reset(ResetCause::RtcWatchdogSystem),
                // An "off" stage is waited for and does nothing (see `Timg::evaluate`).
                _ => {}
            }
        }

        if self.swd_armed() {
            let fed_at = *self.swd_fed_at.get_or_insert(now);
            let waited = now.get().saturating_sub(fed_at.get());
            if waited >= SWD_TIMEOUT {
                self.swd_conf |= SWD_RESET_FLAG;
                self.swd_fed_at = None;
                ctx.request_reset(ResetCause::SuperWatchdog);
            } else if waited >= SWD_FEED_REQUEST {
                self.int_raw |= INT_SWD;
                sooner(Some(fed_at + SWD_TIMEOUT));
            } else {
                sooner(Some(fed_at + SWD_FEED_REQUEST));
            }
        } else {
            self.swd_fed_at = None;
        }

        if let Some(at) = earliest {
            self.wake.arm(ctx, at, Event::RtcWatchdog);
        }
        self.drive(ctx);
    }

    /// The feed request of the super watchdog reads in `SWD_CONF` while its interrupt is raw.
    fn drive(&self, ctx: &mut Ctx) {
        ctx.irq(RTC_CORE, self.int_raw & self.int_ena != 0);
    }

    /// The device time of a wake-up has come, or the block was just built or reset.
    pub(crate) fn wake(&mut self, ctx: &mut Ctx) {
        self.evaluate(ctx);
    }

    /// A write to a watchdog register other than the key: dropped unless the key is in place.
    fn write_watchdog(&mut self, ctx: &mut Ctx, offset: u32, value: u32) {
        if self.wdt_key != WDT_KEY {
            return;
        }
        match offset {
            WDTCONFIG0 => self.wdt_config0 = value,
            WDTFEED => {
                if value & WDT_FEED != 0 {
                    self.wdt.set(0, ctx.now());
                    self.wdt_stage = 0;
                }
            }
            _ => self.wdt_timeout[((offset - WDTCONFIG1) / 4) as usize] = value,
        }
        self.evaluate(ctx);
    }

    fn write_super_watchdog(&mut self, ctx: &mut Ctx, value: u32) {
        if self.swd_key != SWD_KEY {
            return;
        }
        self.swd_conf = (self.swd_conf & SWD_RESET_FLAG) | (value & SWD_CONF_STORED);
        if value & SWD_RST_FLAG_CLR != 0 {
            self.swd_conf &= !SWD_RESET_FLAG;
        }
        if value & SWD_FEED != 0 {
            self.swd_fed_at = Some(ctx.now());
            self.int_raw &= !INT_SWD;
        }
        self.evaluate(ctx);
    }
}

impl Peripheral<Ctx> for RtcCntl {
    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
        if let Some(i) = STORE.iter().position(|&o| o == offset) {
            return Some(self.store[i]);
        }
        Some(match offset {
            RESET_STATE => self.reset_state_other | self.reset_cause,
            TIME_LOW0 => half_of(self.time[0], false),
            TIME_HIGH0 => half_of(self.time[0], true),
            TIME_LOW1 => half_of(self.time[1], false),
            TIME_HIGH1 => half_of(self.time[1], true),
            INT_ENA => self.int_ena,
            INT_RAW => self.int_raw,
            INT_ST => self.int_raw & self.int_ena,
            // Write-only registers read 0.
            TIME_UPDATE | INT_CLR | INT_ENA_W1TS | INT_ENA_W1TC | WDTFEED => 0,
            WDTCONFIG0 => self.wdt_config0,
            WDTWPROTECT => self.wdt_key,
            SWD_CONF => {
                let feed_request = if self.int_raw & INT_SWD == 0 {
                    0
                } else {
                    SWD_FEED_INT
                };
                self.swd_conf | feed_request
            }
            SWD_WPROTECT => self.swd_key,
            offset if (WDTCONFIG1..=WDTCONFIG4).contains(&offset) => {
                self.wdt_timeout[((offset - WDTCONFIG1) / 4) as usize]
            }
            _ => return self.config.read(offset),
        })
    }

    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        if let Some(i) = STORE.iter().position(|&o| o == offset) {
            self.store[i] = value;
            return true;
        }
        match offset {
            OPTIONS0 => {
                // The system reset is the wider of the two: it wins when both bits are written.
                if value & SW_SYS_RST != 0 {
                    ctx.request_reset(ResetCause::SoftwareSystem);
                } else if value & SW_PROCPU_RST != 0 {
                    ctx.request_reset(ResetCause::SoftwareCpu);
                }
                // The two bits read 0; the rest of the register is stored.
                return self
                    .config
                    .write(offset, value & !(SW_SYS_RST | SW_PROCPU_RST));
            }
            TIME_UPDATE => {
                if value & TIME_UPDATE_TRIGGER != 0 {
                    // "Each trigger moves the previous capture into TIME_LOW1/HIGH1" (TW §C.2).
                    self.time = [self.rtc_time(ctx.now()), self.time[0]];
                }
            }
            INT_ENA => self.int_ena = value,
            INT_ENA_W1TS => self.int_ena |= value,
            INT_ENA_W1TC => self.int_ena &= !value,
            INT_CLR => self.int_raw &= !value,
            WDTWPROTECT => self.wdt_key = value,
            WDTCONFIG0 | WDTFEED => {
                self.write_watchdog(ctx, offset, value);
                return true;
            }
            offset if (WDTCONFIG1..=WDTCONFIG4).contains(&offset) => {
                self.write_watchdog(ctx, offset, value);
                return true;
            }
            SWD_WPROTECT => self.swd_key = value,
            SWD_CONF => {
                self.write_super_watchdog(ctx, value);
                return true;
            }
            CLK_CONF => {
                // The slow clock of this model is the RC oscillator, whatever is selected: a
                // selection of another source is reported, not silently mistimed.
                if value >> ANA_CLK_RTC_SEL_SHIFT != 0 {
                    ctx.unsupported(Unsupported::SlowClock { clk_conf: value });
                }
                return self.config.write(offset, value);
            }
            FIB_SEL => {
                let known = self.config.write(offset, value);
                self.evaluate(ctx);
                return known;
            }
            // Read-only: the write is reported as unmodelled.
            RESET_STATE | TIME_LOW0 | TIME_HIGH0 | TIME_LOW1 | TIME_HIGH1 | INT_RAW | INT_ST => {
                return false;
            }
            _ => return self.config.write(offset, value),
        }
        self.drive(ctx);
        true
    }
}
```

- [ ] **Step 5: Wire it**

```diff
--- a/crates/soc-esp32c3/src/periph/mod.rs
+++ b/crates/soc-esp32c3/src/periph/mod.rs
@@ -1,5 +1,5 @@
 //! The modelled peripherals of the ESP32-C3 and their place in the address space.
-use crate::config::SocConfig;
+use crate::config::{STRAP_SPI_FAST_FLASH_BOOT, SocConfig};
 use crate::ctx::{Ctx, Event};
 use crate::flash::FlashDevice;
 use emu_core::Peripheral;
@@ -398,16 +398,19 @@ pub struct Peripherals {
 
 impl Peripherals {
     /// The peripherals after a power-on, at the device time of `ctx`: a device that has a first
-    /// deadline schedules it there.
+    /// deadline schedules it there. The reset values arm the flash-boot protection of the RTC
+    /// watchdog when the chip is strapped to boot from flash.
     #[must_use]
     pub fn new(config: &SocConfig, flash: Box<dyn FlashDevice>, ctx: &mut Ctx) -> Peripherals {
+        let flash_boot = config.strapping & STRAP_SPI_FAST_FLASH_BOOT != 0;
+        let efuse = Efuse::new(config.chip_revision);
         let mut periph = Peripherals {
             gpio: Gpio::new(config.strapping),
             io_mux: IoMux::new(),
             intmatrix: IntMatrix::new(),
-            rtc_cntl: RtcCntl::new(),
+            rtc_cntl: RtcCntl::new(flash_boot, efuse.wdt_delay_sel(), ctx.now()),
             system: System::new(),
-            efuse: Efuse::new(config.chip_revision),
+            efuse,
             extmem: Extmem::new(),
             mmu: MmuTable::new(),
             sensitive: AcceptedBlock::new(SENSITIVE_BASE, &SENSITIVE_ACCEPTED),
@@ -425,6 +428,7 @@ impl Peripherals {
             uart1: Uart::new(1, UART1_BASE),
             usb_serial_jtag: UsbSerialJtag::new(),
         };
+        periph.rtc_cntl.wake(ctx);
         periph.systimer.wake(ctx);
         periph
     }
@@ -438,6 +442,7 @@ impl Peripherals {
                 self.extmem.latch_store(ctx, instruction_bus);
             }
             Event::Systimer => self.systimer.wake(ctx),
+            Event::RtcWatchdog => self.rtc_cntl.wake(ctx),
             Event::Uart(0) => self.uart0.wake(ctx),
             Event::Uart(_) => self.uart1.wake(ctx),
         }
```

The machine cannot reset before Task 11. Until then it takes every request out of the context and reports it by name, so that a software reset or a watchdog expiry is a diagnostic, and a strict stop, instead of a silence:

```diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -189,6 +189,11 @@ impl Esp32C3 {
         while let Some(event) = self.bus.ctx.take_due() {
             self.bus.deliver(event);
         }
+        // The machine does not reset yet: a request is reported by name, and stops a strict
+        // run, instead of vanishing (decision D27, extended to the watchdogs).
+        if let Some(cause) = self.bus.ctx.take_reset() {
+            self.bus.ctx.unsupported(Unsupported::Reset { cause });
+        }
         if self.bus.ctx.take_clock_written() {
             self.retune(pc);
         }
```

- [ ] **Step 6: Run the suite, regenerate the table, run the ignored tests**

Run: `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect > /tmp/bless.log 2>&1; echo "exit=$?"; /usr/bin/git diff --stat docs/registers-accepted-without-effect.md`
Expected: `exit=0`; the watchdog, timer and interrupt registers of `RTC_CNTL` leave the table; the 18 rows that stay name SP1b-3b (sleep and power domains), SP2 (brown-out, pads) or SP8 (analog and clock trims). The row of `RTC_CNTL.OPTIONS0` reads `… CPU stall (the two software reset bits are reported)`: that is what this commit does with them.

Run: `cargo test --workspace > /tmp/t10.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t10.log`
Expected: `exit=0`, `644`.

Run the ignored tests (16, 5, 4 + 1). Expected: all pass: every strict run among them ends before a watchdog expires, so the new report stops none of them.

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/soc-esp32c3/src crates/emu-cli/src/run.rs docs/registers-accepted-without-effect.md
/usr/bin/git commit -m "feat(soc): RTC_CNTL keeps the reset cause, counts the RTC timer and its watchdogs

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3/src crates/emu-cli/src/run.rs docs/registers-accepted-without-effect.md
```

---

## Task 11: Resets: CPU, core and system levels; module resets; the recorded PC

The machine acts on a reset request at the end of the instruction that posted it (decisions E5, E23 to E25). Each level returns to its reset state what TW §D.2 tabulates; the memories and device time are never touched. `SYSTEM.PERIP_RST_EN0/1` reset one module. `ASSIST_DEBUG` records the PC the ROM prints as `Saved PC`; `SENSITIVE` becomes a whole block of storage that a reset of any level returns to its reset values. The refusal Task 10 kept alive (`Unsupported::Reset`, reported by the machine for every request) is removed here, in the step that makes the machine reset, with its tests and the wording of the `OPTIONS0` row.

**Files:**
- Create: `crates/soc-esp32c3/src/periph/sensitive.rs`
- Modify: `crates/soc-esp32c3/src/periph/assist_debug.rs` (rewritten), `crates/soc-esp32c3/src/diag.rs`, `crates/soc-esp32c3/src/periph/rtc_cntl.rs` (one row text), `crates/soc-esp32c3/src/periph/system.rs`, `crates/soc-esp32c3/src/periph/spi_mem.rs`, `crates/soc-esp32c3/src/periph/apb_ctrl.rs`, `crates/soc-esp32c3/src/periph/usb_serial_jtag.rs`, `crates/soc-esp32c3/src/periph/mod.rs`, `crates/soc-esp32c3/src/ctx.rs`, `crates/soc-esp32c3/src/bus.rs`, `crates/soc-esp32c3/src/machine.rs`, `docs/registers-accepted-without-effect.md`

**Interfaces:**
- Consumes: `Ctx::{take_reset, signal, clock_written, now}`, `reset::{ResetCause, ResetLevel}` (Task 3); `Hart::reset(reset_pc)` (Task 6); `RtcCntl::reset(ctx, cause, system)`, `Unsupported::Reset { cause }` and the machine test `a_reset_request_is_reported_by_name_and_stops_a_strict_run` (Task 10); `Uart::reset(now)` (Task 9); `Systimer::new(now)`, `Systimer::wake` (Task 7); `memmap::RESET_PC`; `Storage::{accepted, for_offsets}`.
- Produces: `ctx::Module { Spi01, Uart0, Uart1, Systimer, Sha }`, `Event::ModuleReset(Module)`, the crate-visible `Ctx::clear_sources(&mut self)`; `periph::Sensitive` (`Sensitive::new()`); on `AssistDebug` the crate-visible `cpu_reset(&mut self, pc: u32, sp: u32)`, `core_reset(&mut self)`, `system_reset(&mut self)`; crate-visible `Spi1::reset(&mut self)`, `ApbCtrl::reset(&mut self)`, `UsbSerialJtag::reset(&mut self)`; `Peripherals::reset(&mut self, ctx: &mut Ctx, cause: ResetCause, pc: u32, sp: u32)` and `SocBus::reset(&mut self, cause: ResetCause, pc: u32, sp: u32)` (crate-visible); `Esp32C3::resets(&self) -> u64`. `Unsupported::Reset` no longer exists: a request is acted on, not reported. `SYSTEM.PERIP_RST_EN0/1`: a bit going from 0 to 1 posts `Event::ModuleReset` for its module (bits 1, 2, 5, 29 of `EN0`, bit 2 of `EN1`).

- [ ] **Step 1: Write the failing tests**

The debug assistant (`ROM3:boot_prepare@0x40047dee` writes `CORE_0_RCD_EN = 3`, IR §4.4; `Saved PC` follows `rst:0x7` and not `rst:0x10`, TW §H.1). Replace the test module of `crates/soc-esp32c3/src/periph/assist_debug.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    #[test]
    fn no_pc_was_recorded_before_a_power_on_reset() {
        let ctx = &mut Ctx::default();
        let mut dbg = AssistDebug::new();
        assert_eq!(
            dbg.read(ctx, CORE_0_RCD_PDEBUGPC),
            Some(0),
            "the ROM prints `Saved PC` only when non-zero"
        );
        assert_eq!(dbg.read(ctx, CORE_0_RCD_PDEBUGSP), Some(0));
    }

    #[test]
    fn no_debugger_is_attached() {
        // BA §2.2: bit 0 set would make the bootloader skip the checksum and hash verification.
        let ctx = &mut Ctx::default();
        let mut dbg = AssistDebug::new();
        assert_eq!(dbg.read(ctx, DEBUG_MODE), Some(0));
        assert!(!dbg.write(ctx, DEBUG_MODE, 1), "read-only");
    }

    #[test]
    fn a_cpu_reset_records_the_pc_when_the_recorder_is_on_and_switches_it_off() {
        let ctx = &mut Ctx::default();
        let mut dbg = AssistDebug::new();
        dbg.cpu_reset(0x4000_1111, 0x3FC8_0000);
        assert_eq!(
            dbg.read(ctx, CORE_0_RCD_PDEBUGPC),
            Some(0),
            "the recorder was off"
        );
        assert!(dbg.write(ctx, CORE_0_RCD_EN, 3));
        assert_eq!(dbg.read(ctx, CORE_0_RCD_EN), Some(3));
        dbg.cpu_reset(0x4004_8b92, 0x3FC9_0000);
        assert_eq!(dbg.read(ctx, CORE_0_RCD_PDEBUGPC), Some(0x4004_8b92));
        assert_eq!(dbg.read(ctx, CORE_0_RCD_PDEBUGSP), Some(0x3FC9_0000));
        assert_eq!(
            dbg.read(ctx, CORE_0_RCD_EN),
            Some(0),
            "RCD_EN resets with the CPU"
        );
        // One of the two bits is not enough.
        dbg.write(ctx, CORE_0_RCD_EN, 1);
        dbg.cpu_reset(0x4200_0000, 0);
        assert_eq!(dbg.read(ctx, CORE_0_RCD_PDEBUGPC), Some(0x4004_8b92));
        assert!(
            !dbg.write(ctx, CORE_0_RCD_PDEBUGPC, 1),
            "the record is read-only"
        );
    }

    #[test]
    fn the_record_survives_a_core_reset_and_not_a_system_reset() {
        // TW §H.1: `Saved PC` follows `rst:0x7`, not `rst:0x10`.
        let ctx = &mut Ctx::default();
        let mut dbg = AssistDebug::new();
        dbg.write(ctx, CORE_0_RCD_EN, 3);
        dbg.write(ctx, reg("CORE_0_SP_MIN"), 0x3FC8_DBCC);
        dbg.cpu_reset(0x4004_D1F8, 0);
        dbg.core_reset();
        assert_eq!(dbg.read(ctx, CORE_0_RCD_PDEBUGPC), Some(0x4004_D1F8));
        assert_eq!(
            dbg.read(ctx, reg("CORE_0_SP_MIN")),
            Some(0),
            "back to its reset value"
        );
        dbg.system_reset();
        assert_eq!(dbg.read(ctx, CORE_0_RCD_PDEBUGPC), Some(0));
    }

    #[test]
    fn the_stack_guard_registers_are_stored_and_the_guard_never_fires() {
        // IR §3.3: MONTR_ENA <- 0, SP_MIN, SP_MAX, MONTR_ENA <- 0x300 at every context switch.
        let ctx = &mut Ctx::default();
        let mut dbg = AssistDebug::new();
        assert_eq!(
            dbg.read(ctx, reg("CORE_0_SP_MAX")),
            Some(0xFFFF_FFFF),
            "SVD reset value"
        );
        for (name, value) in [
            ("CORE_0_INTR_ENA", 0x300),
            ("CORE_0_MONTR_ENA", 0x300),
            ("CORE_0_SP_MIN", 0x3FC8_DBCC),
            ("CORE_0_SP_MAX", 0x3FC8_EBC0),
        ] {
            assert!(dbg.write(ctx, reg(name), value), "{name}");
            assert_eq!(dbg.read(ctx, reg(name)), Some(value), "{name}");
        }
        assert!(dbg.write(ctx, reg("CORE_0_INTR_CLR"), 0x300));
        assert_eq!(dbg.read(ctx, CORE_0_INTR_RAW), Some(0));
        assert_eq!(
            dbg.read(ctx, reg("CORE_0_AREA_PC")),
            None,
            "the rest is unmodelled"
        );
    }
}
```

`crates/soc-esp32c3/src/periph/sensitive.rs`, test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn reg(name: &str) -> u32 {
        svd_regs::esp32c3::addr("SENSITIVE", name) - SENSITIVE_BASE
    }

    #[test]
    fn every_register_of_the_block_is_stored_from_its_reset_value() {
        let ctx = &mut Ctx::default();
        let mut pms = Sensitive::new();
        assert_eq!(
            pms.read(ctx, reg("INTERNAL_SRAM_USAGE_1")),
            Some(0xF),
            "SRAM0 is the cache's at reset (CM §2.1)"
        );
        assert_eq!(pms.read(ctx, reg("CLOCK_GATE")), Some(1));
        // What `esp_mprot_set_split_addr` and `esp_mprot_set_pms_area` write (IR §3.2, trace F1).
        for (name, value) in [
            ("CORE_X_IRAM0_DRAM0_DMA_SPLIT_LINE_CONSTRAIN_1", 0x0016_C03E),
            ("CORE_X_IRAM0_PMS_CONSTRAIN_2", 5),
            ("CORE_0_PIF_PMS_MONITOR_0", 1),
        ] {
            assert!(pms.write(ctx, reg(name), value), "{name}");
            assert_eq!(pms.read(ctx, reg(name)), Some(value), "{name}");
        }
        assert_eq!(
            pms.read(ctx, reg("CLOCK_GATE") + 4),
            None,
            "nothing past the block"
        );
        assert_eq!(pms.read(ctx, reg("DATE")), None);
    }

    #[test]
    fn a_new_block_is_the_reset_state() {
        // What a CPU reset does (TRM §6.1.3).
        let ctx = &mut Ctx::default();
        let mut pms = Sensitive::new();
        pms.write(ctx, reg("INTERNAL_SRAM_USAGE_1"), 0xE);
        pms = Sensitive::new();
        assert_eq!(pms.read(ctx, reg("INTERNAL_SRAM_USAGE_1")), Some(0xF));
    }
}
```

`SYSTEM` (`esp_restart_noos` writes `PERIP_RST_EN0 = 0x20000007` then 0, `PERIP_RST_EN1 = 0x7E` then 0: IR §4.1):

```diff
--- a/crates/soc-esp32c3/src/periph/system.rs
+++ b/crates/soc-esp32c3/src/periph/system.rs
@@ -213,7 +261,7 @@ mod tests {
     fn clock_gates_and_resets_are_storage_from_their_reset_values() {
         let ctx = &mut Ctx::default();
         let mut sys = System::new();
-        for offset in [0x00, 0x04, 0x10, 0x14, 0x18, 0x1C] {
+        for offset in [0x00, 0x04, 0x10, 0x14] {
             let reset = svd_regs::esp32c3::lookup(0x600C_0000 + offset)
                 .unwrap()
                 .reset_value;
@@ -263,4 +311,45 @@ mod tests {
         sys.write(ctx, CPU_INTR_FROM_CPU_0, 0);
         assert_eq!(ctx.sources(), 1 << 53);
     }
+
+    #[test]
+    fn a_peripheral_reset_bit_resets_its_module_when_it_is_set() {
+        // IR §4.1: `esp_restart_noos` writes 0x20000007 then 0, then 0x7E then 0.
+        let ctx = &mut Ctx::default();
+        let mut sys = System::new();
+        let reset0 = sys.read(ctx, PERIP_RST_EN0).unwrap();
+        assert!(sys.write(ctx, PERIP_RST_EN0, reset0 | 0x2000_0007));
+        assert_eq!(sys.read(ctx, PERIP_RST_EN0), Some(reset0 | 0x2000_0007));
+        let mut signalled = Vec::new();
+        while let Some(event) = ctx.take_signal() {
+            signalled.push(event);
+        }
+        assert_eq!(
+            signalled,
+            [
+                Event::ModuleReset(Module::Spi01),
+                Event::ModuleReset(Module::Uart0),
+                Event::ModuleReset(Module::Systimer)
+            ]
+        );
+        sys.write(ctx, PERIP_RST_EN0, reset0 | 0x2000_0007);
+        sys.write(ctx, PERIP_RST_EN0, reset0);
+        assert_eq!(ctx.take_signal(), None, "held, then released: no new reset");
+        sys.write(ctx, PERIP_RST_EN0, reset0 | (1 << 5));
+        // `PERIP_RST_EN1` holds the crypto modules in reset at power-on (reset value 0x1FE): the
+        // bootloader released the SHA accelerator to use it.
+        sys.write(ctx, PERIP_RST_EN1, 0);
+        sys.write(ctx, PERIP_RST_EN1, 0x7E);
+        let mut signalled = Vec::new();
+        while let Some(event) = ctx.take_signal() {
+            signalled.push(event);
+        }
+        assert_eq!(
+            signalled,
+            [
+                Event::ModuleReset(Module::Uart1),
+                Event::ModuleReset(Module::Sha)
+            ]
+        );
+    }
 }
```

The peripherals as a whole, and the machine (`esp_restart` ends with `OPTIONS0 |= 0x20`, cause `0x0C`):

```diff
--- a/crates/soc-esp32c3/src/periph/mod.rs
+++ b/crates/soc-esp32c3/src/periph/mod.rs
@@ -789,4 +859,158 @@ mod tests {
             0x000F_FFFF_1234_5678
         );
     }
+
+    /// Reads and writes through the device tree, as the bus does.
+    fn read(p: &mut Peripherals, ctx: &mut Ctx, addr: u32) -> Option<u32> {
+        let (device, base) = p.device(addr)?;
+        device.read(ctx, addr - base)
+    }
+    fn write(p: &mut Peripherals, ctx: &mut Ctx, addr: u32, value: u32) {
+        let (device, base) = p.device(addr).expect("routed");
+        assert!(device.write(ctx, addr - base, value), "{addr:#010x}");
+    }
+    /// The state the reset tests start from: registers of four blocks away from their reset values.
+    fn running(ctx: &mut Ctx) -> Peripherals {
+        let svd = svd_regs::esp32c3::addr;
+        let mut p = peripherals(ctx);
+        // The clock selection `esp_restart_noos` leaves: the crystal undivided (IR §4.1).
+        write(&mut p, ctx, svd("SYSTEM", "SYSCLK_CONF"), 0x0000_0400);
+        write(&mut p, ctx, svd("RTC_CNTL", "STORE4"), 0x0028_0028);
+        write(&mut p, ctx, svd("SENSITIVE", "INTERNAL_SRAM_USAGE_1"), 0xE);
+        write(&mut p, ctx, svd("ASSIST_DEBUG", "CORE_0_RCD_EN"), 3);
+        write(&mut p, ctx, svd("SPI1", "USER1"), 0x1234_5678);
+        write(&mut p, ctx, svd("SYSTEM", "CPU_INTR_FROM_CPU_0"), 1);
+        assert_ne!(ctx.sources(), 0);
+        p
+    }
+
+    #[test]
+    fn a_cpu_reset_records_the_pc_resets_the_permissions_and_leaves_the_rest() {
+        // TRM §6.1.3; IR §4.2: what survives `esp_restart`.
+        let svd = svd_regs::esp32c3::addr;
+        let ctx = &mut Ctx::default();
+        let mut p = running(ctx);
+        p.reset(ctx, ResetCause::SoftwareCpu, 0x4004_8b92, 0x3FC9_0000);
+        let mut read = |addr| read(&mut p, ctx, addr);
+        assert_eq!(
+            read(svd("RTC_CNTL", "RESET_STATE")).map(|v| v & 0x3F),
+            Some(0x0C)
+        );
+        assert_eq!(
+            read(svd("ASSIST_DEBUG", "CORE_0_RCD_PDEBUGPC")),
+            Some(0x4004_8b92)
+        );
+        assert_eq!(read(svd("ASSIST_DEBUG", "CORE_0_RCD_EN")), Some(0));
+        assert_eq!(read(svd("SENSITIVE", "INTERNAL_SRAM_USAGE_1")), Some(0xF));
+        assert_eq!(
+            read(svd("SYSTEM", "SYSCLK_CONF")).map(|v| v & 0xFFF),
+            Some(0x400)
+        );
+        assert_eq!(read(svd("RTC_CNTL", "STORE4")), Some(0x0028_0028));
+        assert_eq!(read(svd("SPI1", "USER1")), Some(0x1234_5678));
+        assert_eq!(read(svd("SYSTEM", "CPU_INTR_FROM_CPU_0")), Some(1));
+    }
+
+    #[test]
+    fn a_core_reset_rebuilds_the_digital_peripherals_and_keeps_rtc_cntl() {
+        // TW §D.2: after `TG0WDT_SYS_RST` the retention registers and the saved PC are still there.
+        let svd = svd_regs::esp32c3::addr;
+        let ctx = &mut Ctx::default();
+        let mut p = running(ctx);
+        assert!(ctx.take_clock_written());
+        p.reset(ctx, ResetCause::Mwdt0Core, 0x4004_c0d4, 0);
+        assert_eq!(ctx.sources(), 0, "no rebuilt device asserts a source");
+        assert!(
+            ctx.take_clock_written(),
+            "the clock selection is the reset one again"
+        );
+        let mut read = |addr| read(&mut p, ctx, addr);
+        assert_eq!(
+            read(svd("RTC_CNTL", "RESET_STATE")).map(|v| v & 0x3F),
+            Some(0x07)
+        );
+        assert_eq!(read(svd("RTC_CNTL", "STORE4")), Some(0x0028_0028));
+        assert_eq!(
+            read(svd("ASSIST_DEBUG", "CORE_0_RCD_PDEBUGPC")),
+            Some(0x4004_c0d4)
+        );
+        assert_eq!(
+            read(svd("SYSTEM", "SYSCLK_CONF")).map(|v| v & 0xFFF),
+            Some(1),
+            "`PRE_DIV_CNT` 1 and the crystal selected, as after power-on"
+        );
+        assert_eq!(
+            read(svd("SPI1", "USER1")),
+            Some(0x5C00_0007),
+            "its reset value"
+        );
+        assert_eq!(read(svd("SYSTEM", "CPU_INTR_FROM_CPU_0")), Some(0));
+        assert_eq!(
+            read(svd("TIMG0", "WDTCONFIG0")),
+            Some(0x0004_C000),
+            "its reset value, `FLASHBOOT_MOD_EN` set"
+        );
+    }
+
+    #[test]
+    fn a_system_reset_also_resets_rtc_cntl_and_forgets_the_recorded_pc() {
+        // TW §D.2, §H.1: no `Saved PC` after `RTCWDT_RTC_RST`.
+        let svd = svd_regs::esp32c3::addr;
+        let ctx = &mut Ctx::default();
+        let mut p = running(ctx);
+        p.reset(ctx, ResetCause::RtcWatchdogSystem, 0x4004_c0d4, 0);
+        let mut read = |addr| read(&mut p, ctx, addr);
+        assert_eq!(
+            read(svd("RTC_CNTL", "RESET_STATE")).map(|v| v & 0x3F),
+            Some(0x10)
+        );
+        assert_eq!(read(svd("RTC_CNTL", "STORE4")), Some(0));
+        assert_eq!(read(svd("ASSIST_DEBUG", "CORE_0_RCD_PDEBUGPC")), Some(0));
+    }
+
+    #[test]
+    fn a_module_reset_returns_that_module_alone_to_its_reset_values() {
+        // IR §4.1: `esp_restart_noos` resets the SPI controllers through `PERIP_RST_EN0` bit 1.
+        let svd = svd_regs::esp32c3::addr;
+        let ctx = &mut Ctx::default();
+        let mut p = running(ctx);
+        p.deliver(ctx, Event::ModuleReset(Module::Spi01));
+        assert_eq!(read(&mut p, ctx, svd("SPI1", "USER1")), Some(0x5C00_0007));
+        assert_eq!(
+            read(&mut p, ctx, svd("RTC_CNTL", "STORE4")),
+            Some(0x0028_0028)
+        );
+        assert_eq!(
+            p.spi1.flash().size(),
+            crate::flash::FlashConfig::default().size(),
+            "the flash chip is not part of the controller"
+        );
+    }
+
+    #[test]
+    fn a_wake_up_is_delivered_to_the_device_it_names() {
+        // A one-shot alarm of the SYSTIMER 1 600 counts ahead: 100 us, 16 000 cycles of the time base.
+        let svd = svd_regs::esp32c3::addr;
+        let ctx = &mut Ctx::default();
+        let mut p = peripherals(ctx);
+        write(&mut p, ctx, svd("SYSTIMER", "TARGET0_LO"), 1_600);
+        write(&mut p, ctx, svd("SYSTIMER", "COMP0_LOAD"), 1);
+        write(
+            &mut p,
+            ctx,
+            svd("SYSTIMER", "CONF"),
+            0x4600_0000 | (1 << 24),
+        );
+        assert_eq!(ctx.next_wake(), Some(emu_core::Cycles(16_000)));
+        ctx.sleep_until(emu_core::Cycles(16_000));
+        assert_eq!(
+            read(&mut p, ctx, svd("SYSTIMER", "INT_RAW")),
+            Some(0),
+            "not called yet"
+        );
+        while let Some(event) = ctx.take_due() {
+            p.deliver(ctx, event);
+        }
+        assert_eq!(read(&mut p, ctx, svd("SYSTIMER", "INT_RAW")), Some(1));
+    }
 }
```

```diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -1024,47 +1045,51 @@ mod tests {
     }
 
     #[test]
-    fn a_reset_request_is_reported_by_name_and_stops_a_strict_run() {
-        use crate::reset::ResetCause;
-        let reported = |m: &mut Esp32C3, cause: ResetCause| {
-            m.drain_diagnostics().into_iter().any(|diagnostic| {
-                matches!(
-                    diagnostic,
-                    Diagnostic::Unsupported { what: Unsupported::Reset { cause: seen }, .. }
-                        if seen == cause
-                )
-            })
-        };
-        // What `esp_restart` ends with: `RTC_CNTL.OPTIONS0` bit 5 (IR §4.1).
-        let mut restart = stop_watchdogs();
-        restart.extend(store_register(reg("RTC_CNTL", "OPTIONS0"), 1 << 5));
-        restart.push(HANG);
-        let mut m = machine(&restart);
+    fn a_software_cpu_reset_restarts_at_the_reset_vector_and_keeps_memory_and_time() {
+        // What `esp_restart` ends with: `RTC_CNTL.OPTIONS0` bit 5 (IR §4.1). The second pass
+        // finds the cause and parks.
+        const MARKER: u32 = SRAM_DBUS_BASE + 0x100;
+        let marker = load_register(MARKER);
+        // The RAM is zero after power-on: the first pass skips the park of the second.
+        let mut body = vec![marker[0], marker[1], cpu_rv32::asm::beq(T1, 0, 8), HANG];
+        body.extend(stop_watchdogs());
+        body.extend(store_register(MARKER, 0x5AFE_C0DE));
+        body.extend(store_register(reg("ASSIST_DEBUG", "CORE_0_RCD_EN"), 3));
+        body.extend(store_register(reg("RTC_CNTL", "OPTIONS0"), 1 << 5));
+        let after_store = ROM_BASE + 4 * u32::try_from(body.len()).unwrap();
+        body.push(HANG);
+        let mut m = machine(&body);
         assert_eq!(m.run_until(Cycles(1_000)), StopReason::CycleBudget);
-        assert!(reported(&mut m, ResetCause::SoftwareCpu));
-        let mut strict = machine_with(
-            &restart,
-            SocConfig {
-                strict_mmio: true,
-                ..SocConfig::default()
-            },
+        assert_eq!(m.resets(), 1);
+        assert_eq!(m.pc(), ROM_BASE + 4 * 3, "the second pass parked");
+        assert_eq!(
+            bus_word(&mut m, reg("RTC_CNTL", "RESET_STATE")) & 0x3F,
+            0x0C,
+            "RTC_SW_CPU_RST"
+        );
+        assert_eq!(
+            bus_word(&mut m, MARKER),
+            0x5AFE_C0DE,
+            "the RAM keeps its content"
+        );
+        assert_eq!(
+            bus_word(&mut m, reg("ASSIST_DEBUG", "CORE_0_RCD_PDEBUGPC")),
+            after_store,
+            "the PC the reset kept from executing"
+        );
+        assert_eq!(
+            bus_word(&mut m, reg("TIMG0", "WDTCONFIG0")),
+            0,
+            "a CPU reset leaves the peripherals as they are"
+        );
+        assert_eq!(
+            m.cycles().get(),
+            1_000,
+            "the cycle total goes on across the reset"
+        );
+        assert!(
+            m.device_time_nanos() >= 1_000 * 50,
+            "and so does device time"
         );
-        assert!(matches!(
-            strict.run_until(Cycles(1_000)),
-            StopReason::StrictMmio(Diagnostic::Unsupported {
-                what: Unsupported::Reset {
-                    cause: ResetCause::SoftwareCpu
-                },
-                ..
-            })
-        ));
-        // A CPU that only sleeps: the RTC watchdog the reset values arm expires after 400 000
-        // cycles of the 136 kHz slow clock, 2.941 s (TW §D.4).
-        let mut asleep = machine(&[wfi(), HANG]);
-        for _ in 0..3 {
-            assert_eq!(asleep.run_until(Cycles(10_000)), StopReason::Asleep);
-        }
-        assert!(asleep.device_time_nanos() >= 2_941_176_470);
-        assert!(reported(&mut asleep, ResetCause::RtcWatchdogSystem));
     }
 }
```

The diagnostic that named a reset request has nothing left to name:

```diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -531,12 +509,6 @@ mod tests {
                 sysclk_conf: 0xC00,
                 cpu_per_conf: 0xC,
             }),
-            unsupported(Unsupported::Reset {
-                cause: ResetCause::SoftwareSystem,
-            }),
-            unsupported(Unsupported::Reset {
-                cause: ResetCause::RtcWatchdogSystem,
-            }),
             unsupported(Unsupported::SlowClock {
                 clk_conf: 0x4000_0000,
             }),
@@ -604,18 +576,6 @@ mod tests {
                 }),
                 "SYSCLK_CONF 0x00000c00",
             ),
-            (
-                unsupported(Unsupported::Reset {
-                    cause: ResetCause::SoftwareCpu,
-                }),
-                "a software reset was requested through RTC_CNTL.OPTIONS0 (reset cause 0x0c",
-            ),
-            (
-                unsupported(Unsupported::Reset {
-                    cause: ResetCause::RtcWatchdogSystem,
-                }),
-                "the RTC watchdog expired (reset cause 0x10",
-            ),
             (
                 unsupported(Unsupported::SlowClock {
                     clk_conf: 0x4000_0000,
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p soc-esp32c3 --lib > /tmp/t11.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, compile errors: no method `cpu_reset`, `cannot find type 'Sensitive'`, no variant `ModuleReset`, no method `reset` on `Peripherals`, no method `resets`.

- [ ] **Step 3: The module-reset event**

```diff
--- a/crates/soc-esp32c3/src/ctx.rs
+++ b/crates/soc-esp32c3/src/ctx.rs
@@ -34,6 +34,16 @@ impl Source {
     }
 }
 
+/// A hardware module that `SYSTEM.PERIP_RST_EN0` or `PERIP_RST_EN1` resets.
+#[derive(Clone, Copy, Debug, PartialEq, Eq)]
+pub enum Module {
+    Spi01,
+    Uart0,
+    Uart1,
+    Systimer,
+    Sha,
+}
+
 /// What a device is called for, besides register accesses: at the instant it asked for (a wake-up),
 /// or because another part of the machine has something to tell it.
 ///
@@ -52,6 +62,8 @@ pub enum Event {
     Uart(u8),
     /// Wake-up of `RTC_CNTL`: the RTC watchdog may have reached a stage timeout.
     RtcWatchdog,
+    /// To the named module: `SYSTEM.PERIP_RST_EN0/1` put it in reset.
+    ModuleReset(Module),
 }
 
 /// The wake-up a device keeps for its next deadline.
@@ -269,6 +281,12 @@ impl Ctx {
         self.clock_written || self.interrupts_changed || self.reset.is_some()
     }
 
+    /// Every source is released at once: what a reset of the digital peripherals does.
+    pub(crate) fn clear_sources(&mut self) {
+        self.sources = 0;
+        self.interrupts_changed = true;
+    }
+
     /// Asks for a reset. It takes place when the current instruction ends; the first request of an
     /// instruction wins.
     pub fn request_reset(&mut self, cause: ResetCause) {
```

- [ ] **Step 4: The devices**

Replace everything above the test module of `crates/soc-esp32c3/src/periph/assist_debug.rs` with:

```rust
//! `ASSIST_DEBUG`: the debug assistant. It records the PC at a CPU reset, which the ROM prints as
//! `Saved PC`, and guards the stack pointer for ESP-IDF. Sources: TRM chapter 17, ESP-IDF
//! `assist_debug_reg.h`; reports IR §4.4 and §5, TW §D.2 and §D.3.
use super::{ASSIST_DEBUG_BASE, Accepted, Storage};
use crate::ctx::Ctx;
use emu_core::Peripheral;
use svd_regs::esp32c3::addr;

const fn reg(name: &str) -> u32 {
    addr("ASSIST_DEBUG", name) - ASSIST_DEBUG_BASE
}

/// The stack guard of ESP-IDF: the application enables the monitor and its interrupt
/// (`esp_hw_stack_guard_init`, values `0x300`), then rewrites the two bounds at every interrupt
/// entry and exit and at every context switch (report IR §3.3, §5). The stack pointer is not
/// compared with them: a guard that compares must use the bounds exactly, at every instruction.
const WHY_STACK_GUARD: &str = "stack guard: the stack pointer is not compared with the bounds";
const ACCEPTED: [Accepted; 5] = [
    Accepted::svd(
        "ASSIST_DEBUG",
        "CORE_0_MONTR_ENA",
        WHY_STACK_GUARD,
        "SP1b-4",
    ),
    Accepted::svd("ASSIST_DEBUG", "CORE_0_INTR_ENA", WHY_STACK_GUARD, "SP1b-4"),
    Accepted::svd("ASSIST_DEBUG", "CORE_0_INTR_CLR", WHY_STACK_GUARD, "SP1b-4"),
    Accepted::svd("ASSIST_DEBUG", "CORE_0_SP_MIN", WHY_STACK_GUARD, "SP1b-4"),
    Accepted::svd("ASSIST_DEBUG", "CORE_0_SP_MAX", WHY_STACK_GUARD, "SP1b-4"),
];

pub(crate) fn accepted() -> Vec<Accepted> {
    ACCEPTED.to_vec()
}

const CORE_0_INTR_RAW: u32 = reg("CORE_0_INTR_RAW");
const CORE_0_RCD_EN: u32 = reg("CORE_0_RCD_EN");
const CORE_0_RCD_PDEBUGPC: u32 = reg("CORE_0_RCD_PDEBUGPC");
const CORE_0_RCD_PDEBUGSP: u32 = reg("CORE_0_RCD_PDEBUGSP");
const DEBUG_MODE: u32 = reg("C0RE_0_DEBUG_MODE");
/// `CORE_0_RCD_EN`: bit 0 `RCD_RECORDEN`, bit 1 `RCD_PDEBUGEN`. With both set the recorder follows
/// the PC; the ROM writes 3 at every boot (`ROM3:boot_prepare@0x40047dee`; report IR §4.4), and so
/// does the bootloader (`wdt_reset_cpu0_info_enable`).
const RECORDING: u32 = 0b11;

#[derive(Clone, Debug)]
pub struct AssistDebug {
    record_enable: u32,
    /// The PC and the stack pointer at the last CPU or core reset that found the recorder on.
    recorded_pc: u32,
    recorded_sp: u32,
    stack_guard: Storage,
}

impl AssistDebug {
    #[must_use]
    pub fn new() -> AssistDebug {
        AssistDebug {
            record_enable: 0,
            recorded_pc: 0,
            recorded_sp: 0,
            stack_guard: Storage::accepted(ASSIST_DEBUG_BASE, &ACCEPTED),
        }
    }

    /// The CPU is reset while it was about to execute the instruction at `pc`, with the stack
    /// pointer `sp`: "when the CPU resets, `RCD_EN` will reset, while `RCD_PDEBUGPC` will not.
    /// Therefore, the latter will keep the PC value at the CPU reset" (TRM §17.4.2).
    ///
    /// ASSUMPTION: the PC recorded is that of the instruction the reset kept from executing. For a
    /// watchdog this is what real chips print (`Saved PC:0x40047ed2` is the `j .` the ROM hangs
    /// in: TW §D.3). For a software reset the model resets right after the store to
    /// `RTC_CNTL.OPTIONS0`, so the PC is that of the next instruction, in the ROM; logs of real
    /// chips show an application address, which means some instructions run before the reset
    /// takes effect. How many is not established (IR §4.4, §7 question 3).
    pub(crate) fn cpu_reset(&mut self, pc: u32, sp: u32) {
        if self.record_enable & RECORDING == RECORDING {
            self.recorded_pc = pc;
            self.recorded_sp = sp;
        }
        self.record_enable = 0;
    }

    /// A core reset returns the other registers to their reset values; the record stays: a real
    /// chip prints `Saved PC` after a `TG0WDT_SYS_RST` (TW §D.2, §H.1).
    pub(crate) fn core_reset(&mut self) {
        self.stack_guard = Storage::accepted(ASSIST_DEBUG_BASE, &ACCEPTED);
    }

    /// A system reset clears the record: a real chip prints no `Saved PC` after a
    /// `RTCWDT_RTC_RST` (TW §D.2, §H.1).
    pub(crate) fn system_reset(&mut self) {
        self.recorded_pc = 0;
        self.recorded_sp = 0;
    }
}

impl Default for AssistDebug {
    fn default() -> AssistDebug {
        AssistDebug::new()
    }
}

impl Peripheral<Ctx> for AssistDebug {
    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
        match offset {
            CORE_0_RCD_EN => Some(self.record_enable),
            CORE_0_RCD_PDEBUGPC => Some(self.recorded_pc),
            CORE_0_RCD_PDEBUGSP => Some(self.recorded_sp),
            // The stack guard never fires and no debugger is attached (BA §2.2).
            CORE_0_INTR_RAW | DEBUG_MODE => Some(0),
            _ => self.stack_guard.read(offset),
        }
    }

    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        match offset {
            CORE_0_RCD_EN => {
                self.record_enable = value & RECORDING;
                true
            }
            _ => self.stack_guard.write(offset, value),
        }
    }
}
```

`crates/soc-esp32c3/src/periph/sensitive.rs`, above its test module:

```rust
//! `SENSITIVE`: permission management (PMS). The registers split the memory into regions, give
//! each one access rights per privilege level and report violations. Sources: TRM chapter 14,
//! ESP-IDF `sensitive_reg.h`, `esp_memprot.c`; reports CM §2.1, IR §3.2 and §4.2.
//!
//! ESP-IDF programs them at start-up (`esp_mprot_set_prot`, with
//! `CONFIG_ESP_SYSTEM_MEMPROT_FEATURE`, the default) and routes the three violation interrupts to
//! CPU line 26. Nothing here checks an access: the registers are stored. "If CPU is reset, PMS
//! registers will be reset, too" (TRM §6.1.3): a reset of any level returns them to their reset
//! values.
use super::{Accepted, SENSITIVE_BASE, Storage};
use crate::ctx::Ctx;
use emu_core::Peripheral;

/// `ROM_TABLE_LOCK` to `CLOCK_GATE`: the 93 consecutive registers of the block (the SVD also has a
/// `DATE` register at `+0xFFC`, which nothing reads).
const REGISTERS: u32 = 93;
const _: () = assert!(
    svd_regs::esp32c3::addr("SENSITIVE", "CLOCK_GATE")
        == SENSITIVE_BASE + svd_regs::REGISTER_BYTES * (REGISTERS - 1)
);

pub(crate) fn accepted() -> Vec<Accepted> {
    vec![
        Accepted::svd(
            "SENSITIVE",
            "ROM_TABLE_LOCK",
            "permission management: no access is checked, no violation is raised",
            "SP1b-4",
        )
        .block(REGISTERS),
    ]
}

#[derive(Clone, Debug)]
pub struct Sensitive {
    registers: Storage,
}

impl Sensitive {
    #[must_use]
    pub fn new() -> Sensitive {
        let offsets: Vec<u32> = (0..REGISTERS)
            .map(|index| svd_regs::REGISTER_BYTES * index)
            .collect();
        Sensitive {
            registers: Storage::for_offsets(SENSITIVE_BASE, &offsets),
        }
    }
}

impl Default for Sensitive {
    fn default() -> Sensitive {
        Sensitive::new()
    }
}

impl Peripheral<Ctx> for Sensitive {
    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
        self.registers.read(offset)
    }

    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        self.registers.write(offset, value)
    }
}
```

```diff
--- a/crates/soc-esp32c3/src/periph/system.rs
+++ b/crates/soc-esp32c3/src/periph/system.rs
@@ -1,13 +1,34 @@
 //! `SYSTEM`: clock selection, peripheral clock gates and resets, software interrupts. Sources:
 //! reports BC §E.1, IR §4.1 and §5 (ESP-IDF `system_reg.h`).
-//! The CPU clock selection is read by `crate::clock`; gates and resets are stored; the four
-//! software interrupt registers are interrupt sources.
-use crate::ctx::{Ctx, Source};
+//! The CPU clock selection is read by `crate::clock`; the clock gates are stored; a peripheral
+//! reset bit resets its module; the four software interrupt registers are interrupt sources.
+use crate::ctx::{Ctx, Event, Module, Source};
 use crate::periph::{Accepted, SYSTEM_BASE, Storage};
 use emu_core::Peripheral;
 
 const SYSCLK_CONF: u32 = svd_regs::esp32c3::addr("SYSTEM", "SYSCLK_CONF") - SYSTEM_BASE;
 const CPU_PER_CONF: u32 = svd_regs::esp32c3::addr("SYSTEM", "CPU_PER_CONF") - SYSTEM_BASE;
+const PERIP_RST_EN0: u32 = svd_regs::esp32c3::addr("SYSTEM", "PERIP_RST_EN0") - SYSTEM_BASE;
+const PERIP_RST_EN1: u32 = svd_regs::esp32c3::addr("SYSTEM", "PERIP_RST_EN1") - SYSTEM_BASE;
+/// The modules `PERIP_RST_EN0` resets, by bit (`IDF:soc/esp32c3/register/soc/system_reg.h`:
+/// `SYSTEM_SPI01_RST` line 497, `SYSTEM_UART_RST` 491, `SYSTEM_UART1_RST` 479,
+/// `SYSTEM_SYSTIMER_RST` 335). ESP-IDF
+/// sets then clears a bit to reset a module (`_timer_ll_reset_register`,
+/// `IDF:hal/esp32c3/include/hal/timer_ll.h:55-66`; `esp_restart_noos` writes `0x20000007` then 0:
+/// report IR §4.1; a UART through `uart_ll_reset_register`, `uart_ll.h:109`).
+///
+/// ASSUMPTION: bit 0, `SYSTEM_TIMERS_RST`, resets nothing here. `esp_restart_noos` pulses it, and
+/// what it resets is not documented: the timer groups have their own bits (IR §4.3, §7 question
+/// 1; TW §F.1). The other bits belong to modules this plan does not model.
+const RESETS_0: [(u32, Module); 4] = [
+    (1 << 1, Module::Spi01),
+    (1 << 2, Module::Uart0),
+    (1 << 5, Module::Uart1),
+    (1 << 29, Module::Systimer),
+];
+/// `PERIP_RST_EN1` bit 2, `SYSTEM_CRYPTO_SHA_RST` (`system_reg.h:553`): the only crypto module
+/// modelled. `esp_restart_noos` pulses `0x7E` (IR §4.1).
+const RESETS_1: [(u32, Module); 1] = [(1 << 2, Module::Sha)];
 /// `CPU_INTR_FROM_CPU_0..3`: bit 0 of each register is the level of an interrupt source, set and
 /// cleared by software (`esp_crosscore_int_send` stores 1 at `APP@0x40381bc6`, its handler stores
 /// 0 at `APP@0x40381b50`: report IR §3.3, §5).
@@ -21,46 +42,47 @@ const SOFTWARE_SOURCES: [Source; SOFTWARE_INTERRUPTS] = [
     Source::of("CPU_INTR_FROM_CPU_3_MAP"),
 ];
 const SOFTWARE_INTERRUPT_LEVEL: u32 = 1;
-/// `CPU_PERI_CLK_EN`, `CPU_PERI_RST_EN`, `PERIP_CLK_EN0/1`, `PERIP_RST_EN0/1`: gates and reset pulses the
-/// ROM toggles around each peripheral it sets up. Nothing acts on them yet. `CPU_PER_CONF` is stored
-/// as software wrote it; only its `CPUPERIOD_SEL` field acts, through `crate::clock`. `MEM_PD_MASK`
-/// is read by `rtc_init`.
+
+/// `CPU_PERI_CLK_EN`, `CPU_PERI_RST_EN`, `PERIP_CLK_EN0/1`: clock gates and the reset of the CPU
+/// peripherals. Nothing acts on them: a gated module keeps answering. `CPU_PER_CONF` is stored as
+/// software wrote it; only its `CPUPERIOD_SEL` field acts, through `crate::clock`. `MEM_PD_MASK`
+/// is read by `rtc_init`. The two `BT_LPCK_DIV` registers are written by `esp_perip_clk_init`.
 const ACCEPTED: [Accepted; 9] = [
     Accepted::svd(
         "SYSTEM",
         "CPU_PERI_CLK_EN",
-        "clock gates of the CPU peripherals",
-        "SP1b-3",
+        "clock gates of the CPU peripherals: a gated module keeps answering",
+        "SP8",
     ),
     Accepted::svd(
         "SYSTEM",
         "CPU_PERI_RST_EN",
-        "resets of the CPU peripherals",
-        "SP1b-3",
+        "resets of the CPU peripherals (the debug assistant)",
+        "SP1b-4",
     ),
     Accepted::svd(
         "SYSTEM",
         "PERIP_CLK_EN0",
-        "peripheral clock gates",
-        "SP1b-3",
+        "peripheral clock gates: a gated module keeps answering",
+        "SP8",
     ),
     Accepted::svd(
         "SYSTEM",
         "PERIP_CLK_EN1",
-        "peripheral clock gates",
-        "SP1b-3",
+        "peripheral clock gates: a gated module keeps answering",
+        "SP8",
     ),
     Accepted::svd(
         "SYSTEM",
-        "PERIP_RST_EN0",
-        "peripheral reset pulses",
-        "SP1b-3",
+        "BT_LPCK_DIV_INT",
+        "Bluetooth low-power clock divider: there is no radio",
+        "SP5b",
     ),
     Accepted::svd(
         "SYSTEM",
-        "PERIP_RST_EN1",
-        "peripheral reset pulses",
-        "SP1b-3",
+        "BT_LPCK_DIV_FRAC",
+        "Bluetooth low-power clock source: there is no radio",
+        "SP5b",
     ),
     Accepted::svd(
         "SYSTEM",
@@ -78,7 +100,7 @@ const ACCEPTED: [Accepted; 9] = [
         "SYSTEM",
         "MEM_PD_MASK",
         "memory power-down mask in light sleep",
-        "SP1b-3",
+        "SP1b-3b",
     ),
 ];
 
@@ -100,6 +122,8 @@ const XTAL_FREQ_MHZ: u32 = crate::clock::XTAL_MHZ;
 pub struct System {
     sysclk_writable: u32,
     registers: Storage,
+    /// `PERIP_RST_EN0` and `PERIP_RST_EN1` as software reads them.
+    resets: [u32; 2],
     /// `CPU_INTR_FROM_CPU_0..3`.
     software_interrupts: [u32; SOFTWARE_INTERRUPTS],
 }
@@ -110,6 +134,7 @@ impl System {
         System {
             sysclk_writable: SYSCLK_RESET,
             registers: Storage::accepted(SYSTEM_BASE, &ACCEPTED),
+            resets: [reset_value(PERIP_RST_EN0), reset_value(PERIP_RST_EN1)],
             software_interrupts: [0; SOFTWARE_INTERRUPTS],
         }
     }
@@ -126,6 +151,11 @@ impl System {
     }
 }
 
+fn reset_value(offset: u32) -> u32 {
+    svd_regs::esp32c3::lookup(SYSTEM_BASE + offset).map_or(0, |r| r.reset_value)
+}
+
+/// Index of `offset` among the four software interrupt registers.
 fn software_interrupt(offset: u32) -> Option<usize> {
     let index = usize::try_from(offset.checked_sub(CPU_INTR_FROM_CPU_0)? / 4).ok()?;
     (index < SOFTWARE_INTERRUPTS).then_some(index)
@@ -139,13 +169,15 @@ impl Default for System {
 
 impl Peripheral<Ctx> for System {
     fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
-        if offset == SYSCLK_CONF {
-            return Some(self.clock_registers().0);
-        }
         if let Some(index) = software_interrupt(offset) {
             return Some(self.software_interrupts[index]);
         }
-        self.registers.read(offset)
+        match offset {
+            SYSCLK_CONF => Some(self.clock_registers().0),
+            PERIP_RST_EN0 => Some(self.resets[0]),
+            PERIP_RST_EN1 => Some(self.resets[1]),
+            _ => self.registers.read(offset),
+        }
     }
 
     fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
@@ -158,6 +190,22 @@ impl Peripheral<Ctx> for System {
             return true;
         }
         match offset {
+            PERIP_RST_EN0 | PERIP_RST_EN1 => {
+                let (register, modules): (usize, &[(u32, Module)]) = if offset == PERIP_RST_EN0 {
+                    (0, &RESETS_0)
+                } else {
+                    (1, &RESETS_1)
+                };
+                // A module is reset when its bit goes from 0 to 1.
+                let rising = value & !self.resets[register];
+                self.resets[register] = value;
+                for (bit, module) in modules {
+                    if rising & bit != 0 {
+                        ctx.signal(Event::ModuleReset(*module));
+                    }
+                }
+                true
+            }
             SYSCLK_CONF => {
                 self.sysclk_writable = value & SYSCLK_WRITABLE;
                 ctx.clock_written();
```

```diff
--- a/crates/soc-esp32c3/src/periph/spi_mem.rs
+++ b/crates/soc-esp32c3/src/periph/spi_mem.rs
@@ -164,15 +164,25 @@ pub struct Spi1 {
 impl Spi1 {
     #[must_use]
     pub fn new(flash: Box<dyn FlashDevice>) -> Spi1 {
-        let mut offsets: Vec<u32> = SPI1_ACCEPTED.iter().map(|a| a.addr - SPI1_BASE).collect();
-        offsets.extend([ADDR, USER, USER1, USER2, MOSI_DLEN, MISO_DLEN, RD_STATUS]);
-        offsets.extend((0..DATA_WORDS).map(|word| W0 + 4 * word));
         Spi1 {
-            registers: Storage::for_offsets(SPI1_BASE, &offsets),
+            registers: Spi1::reset_registers(),
             flash,
         }
     }
 
+    fn reset_registers() -> Storage {
+        let mut offsets: Vec<u32> = SPI1_ACCEPTED.iter().map(|a| a.addr - SPI1_BASE).collect();
+        offsets.extend([ADDR, USER, USER1, USER2, MOSI_DLEN, MISO_DLEN, RD_STATUS]);
+        offsets.extend((0..DATA_WORDS).map(|word| W0 + 4 * word));
+        Storage::for_offsets(SPI1_BASE, &offsets)
+    }
+
+    /// The controller is reset (`SYSTEM.PERIP_RST_EN0` bit 1, or a core reset): its registers
+    /// return to their reset values. The flash chip is another component: it keeps its content.
+    pub(crate) fn reset(&mut self) {
+        self.registers = Spi1::reset_registers();
+    }
+
     /// The chip, for the cache windows of the bus.
     #[must_use]
     pub fn flash(&self) -> &dyn FlashDevice {
```

```diff
--- a/crates/soc-esp32c3/src/periph/apb_ctrl.rs
+++ b/crates/soc-esp32c3/src/periph/apb_ctrl.rs
@@ -48,6 +48,12 @@ impl ApbCtrl {
         }
     }
 
+    /// A core reset returns the registers to their reset values. The generator is no register: its
+    /// sequence goes on, so a boot after a reset does not replay the numbers of the one before.
+    pub(crate) fn reset(&mut self) {
+        self.power = Storage::accepted(APB_CTRL_BASE, &ACCEPTED);
+    }
+
     fn next_random(&mut self) -> u32 {
         let mut state = self.rng;
         state ^= state >> 12;
```

```diff
--- a/crates/soc-esp32c3/src/periph/usb_serial_jtag.rs
+++ b/crates/soc-esp32c3/src/periph/usb_serial_jtag.rs
@@ -33,6 +33,11 @@ impl UsbSerialJtag {
         }
     }
 
+    /// A core reset empties the packet being filled; what was sent stays for the console.
+    pub(crate) fn reset(&mut self) {
+        self.packet.clear();
+    }
+
     /// The bytes sent to the host since the last call.
     pub fn take_tx(&mut self) -> Vec<u8> {
         std::mem::take(&mut self.tx)
```

- [ ] **Step 5: The three levels**

```diff
--- a/crates/soc-esp32c3/src/periph/mod.rs
+++ b/crates/soc-esp32c3/src/periph/mod.rs
@@ -1,7 +1,8 @@
 //! The modelled peripherals of the ESP32-C3 and their place in the address space.
 use crate::config::{STRAP_SPI_FAST_FLASH_BOOT, SocConfig};
-use crate::ctx::{Ctx, Event};
+use crate::ctx::{Ctx, Event, Module};
 use crate::flash::FlashDevice;
+use crate::reset::{ResetCause, ResetLevel};
 use emu_core::Peripheral;
 
 pub mod ana_i2c;
@@ -15,6 +16,7 @@ pub mod intmatrix;
 pub mod io_mux;
 pub mod mmu;
 pub mod rtc_cntl;
+pub mod sensitive;
 pub mod sha;
 pub mod spi_mem;
 pub mod system;
@@ -32,6 +34,7 @@ pub use intmatrix::IntMatrix;
 pub use io_mux::IoMux;
 pub use mmu::MmuTable;
 pub use rtc_cntl::RtcCntl;
+pub use sensitive::Sensitive;
 pub use sha::Sha;
 pub use spi_mem::{Spi0, Spi1};
 pub use system::System;
@@ -226,7 +229,7 @@ pub fn accepted_without_effect() -> Vec<Accepted> {
         io_mux::accepted(),
         rtc_cntl::accepted(),
         gpio::accepted(),
-        SENSITIVE_ACCEPTED.to_vec(),
+        sensitive::accepted(),
         system::accepted(),
         uart::accepted(),
         intmatrix::accepted(),
@@ -283,14 +286,6 @@ pub(crate) const SYSTEM_BASE: u32 = 0x600C_0000;
 const SYSTEM: (u32, u32) = (SYSTEM_BASE, 0x600C_1000);
 pub(crate) const SENSITIVE_BASE: u32 = 0x600C_1000;
 const SENSITIVE: (u32, u32) = (SENSITIVE_BASE, 0x600C_2000);
-/// `Cache_Occupy_ICache_MEMORY` gives SRAM0 to the cache by clearing bit 0 (CM §2.1). SRAM0 stays
-/// plain RAM here: nothing uses it once the cache is on (CM §9 item 9).
-const SENSITIVE_ACCEPTED: [Accepted; 1] = [Accepted::svd(
-    "SENSITIVE",
-    "INTERNAL_SRAM_USAGE_1",
-    "SRAM0 handed to the cache as line storage",
-    "SP1b-4",
-)];
 pub(crate) const INTMATRIX_BASE: u32 = 0x600C_2000;
 const INTMATRIX: (u32, u32) = (INTMATRIX_BASE, 0x600C_3000);
 pub(crate) const EXTMEM_BASE: u32 = 0x600C_4000;
@@ -371,6 +366,8 @@ const RADIO_POWER_ACCEPTED: [Accepted; 4] = [
 
 #[derive(Debug)]
 pub struct Peripherals {
+    /// The strapping pins of the device, latched at power-on: what a reset rebuilds from.
+    strapping: u32,
     pub gpio: Gpio,
     pub io_mux: IoMux,
     pub intmatrix: IntMatrix,
@@ -379,7 +376,7 @@ pub struct Peripherals {
     pub efuse: Efuse,
     pub extmem: Extmem,
     pub mmu: MmuTable,
-    pub sensitive: AcceptedBlock,
+    pub sensitive: Sensitive,
     pub assist_debug: AssistDebug,
     pub spi0: Spi0,
     pub spi1: Spi1,
@@ -405,6 +402,7 @@ impl Peripherals {
         let flash_boot = config.strapping & STRAP_SPI_FAST_FLASH_BOOT != 0;
         let efuse = Efuse::new(config.chip_revision);
         let mut periph = Peripherals {
+            strapping: config.strapping,
             gpio: Gpio::new(config.strapping),
             io_mux: IoMux::new(),
             intmatrix: IntMatrix::new(),
@@ -413,7 +411,7 @@ impl Peripherals {
             efuse,
             extmem: Extmem::new(),
             mmu: MmuTable::new(),
-            sensitive: AcceptedBlock::new(SENSITIVE_BASE, &SENSITIVE_ACCEPTED),
+            sensitive: Sensitive::new(),
             assist_debug: AssistDebug::new(),
             spi0: Spi0::new(),
             spi1: Spi1::new(flash),
@@ -445,9 +443,81 @@ impl Peripherals {
             Event::RtcWatchdog => self.rtc_cntl.wake(ctx),
             Event::Uart(0) => self.uart0.wake(ctx),
             Event::Uart(_) => self.uart1.wake(ctx),
+            Event::ModuleReset(module) => self.reset_module(ctx, module),
         }
     }
 
+    /// `SYSTEM.PERIP_RST_EN0/1` put a module in reset: its registers return to their reset values.
+    /// What `esp_restart_noos` does to the UARTs, the SPI controllers and the SYSTIMER before it
+    /// resets the CPU (`IDF:esp_system/port/soc/esp32c3/system_internal.c`; report IR §4.1).
+    fn reset_module(&mut self, ctx: &mut Ctx, module: Module) {
+        match module {
+            Module::Spi01 => {
+                self.spi0 = Spi0::new();
+                self.spi1.reset();
+            }
+            Module::Uart0 => self.uart0.reset(ctx.now()),
+            Module::Uart1 => self.uart1.reset(ctx.now()),
+            Module::Systimer => {
+                self.systimer = Systimer::new(ctx.now());
+                self.systimer.wake(ctx);
+            }
+            Module::Sha => self.sha = Sha::new(),
+        }
+    }
+
+    /// Applies a reset to the peripherals (TRM §6.1.3; report TW §D.2). `pc` and `sp` are those of
+    /// the CPU when the reset caught it.
+    ///
+    /// - Every level resets the CPU: the debug assistant records its PC, the permission registers
+    ///   return to their reset values.
+    /// - A core reset rebuilds every digital peripheral. The flash chip, the random sequence and
+    ///   the console bytes that already left are no register: they stay.
+    /// - A system reset also returns `RTC_CNTL` to its reset values and clears the recorded PC.
+    ///
+    /// ASSUMPTION: a CPU reset leaves the interrupt controller as it is. The TRM lists only the PMS
+    /// registers as reset with the CPU (§6.1.3); the ROM clears `CPU_INT_ENABLE` itself at every
+    /// boot (`ROM3:boot_prepare`, trace F1), which hides the difference (TW §D.2).
+    pub(crate) fn reset(&mut self, ctx: &mut Ctx, cause: ResetCause, pc: u32, sp: u32) {
+        let level = cause.level();
+        self.assist_debug.cpu_reset(pc, sp);
+        self.sensitive = Sensitive::new();
+        if level >= ResetLevel::Core {
+            self.core_reset(ctx);
+        }
+        if level == ResetLevel::System {
+            self.assist_debug.system_reset();
+        }
+        // Last: it drives the RTC interrupt source again, after a core reset cleared them all.
+        self.rtc_cntl.reset(ctx, cause, level == ResetLevel::System);
+    }
+
+    fn core_reset(&mut self, ctx: &mut Ctx) {
+        self.gpio = Gpio::new(self.strapping);
+        self.io_mux = IoMux::new();
+        self.intmatrix = IntMatrix::new();
+        self.system = System::new();
+        self.extmem = Extmem::new();
+        self.mmu = MmuTable::new();
+        self.assist_debug.core_reset();
+        self.spi0 = Spi0::new();
+        self.spi1.reset();
+        self.sha = Sha::new();
+        self.apb_ctrl.reset();
+        self.ana_i2c = AnaI2c::new();
+        self.timg0 = AcceptedBlock::new(TIMG0_BASE, &TIMG0_ACCEPTED);
+        self.systimer = Systimer::new(ctx.now());
+        self.apb_saradc = AcceptedBlock::new(APB_SARADC_BASE, &APB_SARADC_ACCEPTED);
+        self.radio_power = AcceptedBlock::new(0, &RADIO_POWER_ACCEPTED);
+        self.uart0.reset(ctx.now());
+        self.uart1.reset(ctx.now());
+        self.usb_serial_jtag.reset();
+        // No rebuilt device asserts an interrupt source, and `SYSTEM` selects the reset clock again.
+        ctx.clear_sources();
+        ctx.clock_written();
+        self.systimer.wake(ctx);
+    }
+
     /// The device that owns the register at `addr`, with its base address.
     pub(crate) fn device(&mut self, addr: u32) -> Option<(&mut dyn Peripheral<Ctx>, u32)> {
         let within = |block: (u32, u32)| (block.0..block.1).contains(&addr);
```

```diff
--- a/crates/soc-esp32c3/src/bus.rs
+++ b/crates/soc-esp32c3/src/bus.rs
@@ -10,6 +10,7 @@ use crate::memmap::{
 };
 use crate::periph::Peripherals;
 use crate::periph::mmu::{MMU_PAGE_BYTES, MmuMiss};
+use crate::reset::ResetCause;
 use crate::rom::RomImage;
 use emu_core::{AccessKind, AccessSize, Bus, BusError, Ram, ReadOutcome};
 use svd_regs::REGISTER_BYTES;
@@ -119,6 +120,12 @@ impl SocBus {
         self.settle();
     }
 
+    /// Applies a reset to the peripherals. The memories keep their content (TRM §6.1.1).
+    pub(crate) fn reset(&mut self, cause: ResetCause, pc: u32, sp: u32) {
+        self.periph.reset(&mut self.ctx, cause, pc, sp);
+        self.settle();
+    }
+
     /// Delivers what the devices posted for each other during a register access or a call: such
     /// an event reaches its device before the instruction ends.
     fn settle(&mut self) {
```

```diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -5,6 +5,7 @@ use crate::config::SocConfig;
 use crate::diag::{Diagnostic, Unsupported};
 use crate::flash::{FlashDevice, FlashImageError, NorFlash};
 use crate::memmap::RESET_PC;
+use crate::reset::ResetCause;
 use crate::rom::RomImage;
 use cpu_rv32::{
     DatasheetTiming, Exception, Hart, HartConfig, InterruptModel, MisalignedPolicy, StepOutcome,
@@ -12,6 +13,9 @@ use cpu_rv32::{
 };
 use emu_core::{Cycles, Frequency, TimingModel, TraceHash};
 
+/// `x2`, the stack pointer of the RISC-V calling convention.
+const STACK_POINTER: usize = 2;
+
 /// The two serial consoles the mask ROM prints on.
 #[derive(Clone, Copy, Debug, PartialEq, Eq)]
 pub enum ConsoleChannel {
@@ -60,6 +64,8 @@ pub struct Esp32C3 {
     trace: Option<TraceHash>,
     /// The line the interrupt controller selects, and the device time from which the CPU sees it.
     selected: Option<(u32, Cycles)>,
+    /// Resets since power-on.
+    resets: u64,
 }
 
 impl Esp32C3 {
@@ -114,6 +120,7 @@ impl Esp32C3 {
             timing: DatasheetTiming,
             trace: config.trace_hash.then(TraceHash::new),
             selected: None,
+            resets: 0,
         }
     }
 
@@ -184,15 +191,13 @@ impl Esp32C3 {
     }
 
     /// Acts on what the last instruction, or the passing of time, left in the context: wake-ups
-    /// that are due, a clock change, a change of the interrupt lines.
+    /// that are due, a reset, a clock change, a change of the interrupt lines.
     fn service(&mut self, pc: u32) {
         while let Some(event) = self.bus.ctx.take_due() {
             self.bus.deliver(event);
         }
-        // The machine does not reset yet: a request is reported by name, and stops a strict
-        // run, instead of vanishing (decision D27, extended to the watchdogs).
         if let Some(cause) = self.bus.ctx.take_reset() {
-            self.bus.ctx.unsupported(Unsupported::Reset { cause });
+            self.reset(cause);
         }
         if self.bus.ctx.take_clock_written() {
             self.retune(pc);
@@ -249,6 +254,16 @@ impl Esp32C3 {
         Some(target.get() - now.get())
     }
 
+    /// Applies a reset (TRM §6.1; ADR 0003): the peripherals of its level return to their reset
+    /// state, the CPU restarts in the ROM. The memories keep their content and device time goes on.
+    fn reset(&mut self, cause: ResetCause) {
+        let (pc, sp) = (self.hart.pc, self.hart.x[STACK_POINTER]);
+        self.bus.reset(cause, pc, sp);
+        self.hart.reset(RESET_PC);
+        self.selected = None;
+        self.resets += 1;
+    }
+
     /// Applies a write to the clock registers: from the end of the instruction that wrote them,
     /// cycles last what the new selection says.
     fn retune(&mut self, pc: u32) {
@@ -266,6 +281,12 @@ impl Esp32C3 {
         }
     }
 
+    /// Resets since power-on: software resets and watchdog resets alike.
+    #[must_use]
+    pub const fn resets(&self) -> u64 {
+        self.resets
+    }
+
     /// The CPU clock frequency the firmware selected.
     #[must_use]
     pub fn cpu_frequency(&self) -> Frequency {
```

The refusal of Task 10 goes, and the row of `OPTIONS0` says what its two bits now do:

```diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -1,5 +1,4 @@
 //! What the emulator has to tell the user about the run: accesses it does not model, CPU exceptions.
-use crate::reset::ResetCause;
 use cpu_rv32::Exception;
 use std::collections::BTreeMap;
 use std::fmt;
@@ -33,9 +32,6 @@ pub enum Unsupported {
     ShaMode { mode: u32 },
     /// A clock selection without a known CPU frequency (D22).
     ClockSource { sysclk_conf: u32, cpu_per_conf: u32 },
-    /// A reset the machine cannot perform yet (D27): asked for through `RTC_CNTL.OPTIONS0`, or
-    /// by a watchdog that expired.
-    Reset { cause: ResetCause },
     /// `RTC_CNTL.CLK_CONF` selects a slow clock other than the RC oscillator, the only one modelled.
     SlowClock { clk_conf: u32 },
     /// A byte written to a UART whose transmit FIFO was full: the hardware drops it too.
@@ -81,23 +77,6 @@ impl Unsupported {
                 f,
                 "the clock registers select a CPU clock inkemu cannot model (SYSCLK_CONF {sysclk_conf:#010x}, CPU_PER_CONF {cpu_per_conf:#010x}, pc {pc:#010x}), so emulated time keeps the previous frequency; select the crystal or the PLL at 80 or 160 MHz"
             ),
-            Unsupported::Reset { cause } => write!(
-                f,
-                "{} (reset cause {:#04x}, pc {pc:#010x}) but resets are not emulated yet, so nothing was reset; restart inkemu to reset the device",
-                match cause {
-                    ResetCause::SoftwareSystem | ResetCause::SoftwareCpu =>
-                        "a software reset was requested through RTC_CNTL.OPTIONS0",
-                    ResetCause::RtcWatchdogCpu
-                    | ResetCause::RtcWatchdogCore
-                    | ResetCause::RtcWatchdogSystem => "the RTC watchdog expired",
-                    ResetCause::SuperWatchdog => "the super watchdog expired",
-                    ResetCause::Mwdt0Cpu
-                    | ResetCause::Mwdt0Core
-                    | ResetCause::Mwdt1Cpu
-                    | ResetCause::Mwdt1Core => "a main watchdog expired",
-                },
-                cause.code()
-            ),
             Unsupported::SlowClock { clk_conf } => write!(
                 f,
                 "RTC_CNTL.CLK_CONF selects an RTC slow clock inkemu does not model (value {clk_conf:#010x}, pc {pc:#010x}), so the RTC timer and the RTC watchdog keep counting the 136 kHz RC oscillator; select the RC oscillator (ANA_CLK_RTC_SEL = 0) or expect their durations to differ from the device"
@@ -178,7 +157,6 @@ impl Diagnostic {
                     sysclk_conf,
                     cpu_per_conf,
                 } => (8, sysclk_conf, cpu_per_conf),
-                Unsupported::Reset { cause } => (13, cause.code(), 0),
                 Unsupported::SlowClock { .. } => (9, 0, 0),
                 Unsupported::UartOverflow { uart } => (12, u32::from(uart), 0),
                 // A command is 1 to 16 bits long, so 0 is free for "no command phase".
```

```diff
--- a/crates/soc-esp32c3/src/periph/rtc_cntl.rs
+++ b/crates/soc-esp32c3/src/periph/rtc_cntl.rs
@@ -139,7 +139,7 @@ const ACCEPTED: [Accepted; 18] = [
     Accepted::svd(
         "RTC_CNTL",
         "OPTIONS0",
-        "BBPLL, crystal and I2C power forcing, CPU stall (the two software reset bits are reported)",
+        "BBPLL, crystal and I2C power forcing, CPU stall (the two software reset bits act)",
         "SP1b-3b",
     ),
     Accepted::svd(
```

- [ ] **Step 6: Run the suite, regenerate the table, run the ignored tests**

Run: `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect > /tmp/bless.log 2>&1; echo "exit=$?"; /usr/bin/git diff --stat docs/registers-accepted-without-effect.md`
Expected: `exit=0`; `SYSTEM.PERIP_RST_EN0/1` and `ASSIST_DEBUG.CORE_0_RCD_EN` leave the table; the 93 registers of `SENSITIVE` are one row (heir SP1b-4), the five stack-guard registers of `ASSIST_DEBUG` five rows (SP1b-4), the clock gates of `SYSTEM` name SP8, two rows appear for the Bluetooth clock dividers the application writes (`esp_perip_clk_init`; heir SP5b), and the row of `RTC_CNTL.OPTIONS0` now ends with `(the two software reset bits act)`.

Run: `cargo test --workspace > /tmp/t11.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t11.log`
Expected: `exit=0`, `654`.

Run the ignored tests (16, 5, 4 + 1). Expected: all pass. `blank_flash_prints_invalid_header_forever` still does: it looks at the first lines, long before the RTC watchdog resets the chip at 2.94 s.

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/soc-esp32c3/src docs/registers-accepted-without-effect.md
/usr/bin/git commit -m "feat(soc): resets at three levels, module resets, and the PC a reset records

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3/src docs/registers-accepted-without-effect.md
```

---

## Task 12: The registers of the application's life outside the timers

Storage rows for what the reference application writes after its first log line and that nothing models: each is a decision recorded in the accepted table with its heir. And one word of the eFuse block that the application reads although no register is documented there (decision E34). The list comes from the MMIO trace of the application on the QEMU oracle (IR §0, trace F1), confirmed by a strict run of the finished plan.

**Files:**
- Modify: `crates/soc-esp32c3/src/periph/spi_mem.rs`, `crates/soc-esp32c3/src/periph/gpio.rs`, `crates/soc-esp32c3/src/periph/apb_ctrl.rs`, `crates/soc-esp32c3/src/periph/efuse.rs`, `docs/registers-accepted-without-effect.md`

**Interfaces:**
- Consumes: `Accepted::svd`, `Storage::accepted` (SP1b-2).
- Produces: accepted rows `SPI1.{MISC, FLASH_WAITI_CTRL, FLASH_SUS_CTRL}`, `SPI0.CORE_CLK_SEL`, `GPIO.FUNC2..5_IN_SEL_CFG`, `APB_CTRL.{WIFI_CLK_EN, WIFI_RST_EN}`; `EFUSE + 0x18C` reads 0. The rows of `SPI0`/`SPI1` whose heir was SP1b-3 name their real one.

- [ ] **Step 1: Write the failing tests**

```diff
--- a/crates/soc-esp32c3/src/periph/spi_mem.rs
+++ b/crates/soc-esp32c3/src/periph/spi_mem.rs
@@ -546,8 +567,8 @@ mod tests {
         assert_eq!(spi.read(ctx, w15), Some(0x9D18_609D));
         assert_eq!(
             spi.read(ctx, w15 + 4),
-            None,
-            "FLASH_WAITI_CTRL is not modelled: nothing was stored past W15"
+            Some(0x14),
+            "FLASH_WAITI_CTRL keeps its reset value: nothing was stored past W15"
         );
     }
 
```

```diff
--- a/crates/soc-esp32c3/src/periph/apb_ctrl.rs
+++ b/crates/soc-esp32c3/src/periph/apb_ctrl.rs
@@ -133,17 +147,22 @@ mod tests {
         let ctx = &mut Ctx::default();
         let mut apb = ApbCtrl::new(DEFAULT_RNG_SEED);
         assert!(!apb.write(ctx, RND_DATA, 0));
-        for (name, reset) in [("FRONT_END_MEM_PD", 0x15), ("MEM_POWER_UP", 0x3F)] {
+        for (name, reset) in [
+            ("FRONT_END_MEM_PD", 0x15),
+            ("MEM_POWER_UP", 0x3F),
+            ("WIFI_CLK_EN", 0xFFFC_E030),
+            ("WIFI_RST_EN", 0),
+        ] {
             let offset = svd_regs::esp32c3::addr("APB_CTRL", name) - APB_CTRL_BASE;
             assert_eq!(apb.read(ctx, offset), Some(reset), "{name}");
             assert!(apb.write(ctx, offset, 1));
             assert_eq!(apb.read(ctx, offset), Some(1));
         }
-        let wifi_clk_en = svd_regs::esp32c3::addr("APB_CTRL", "WIFI_CLK_EN") - APB_CTRL_BASE;
+        let clk_out_en = svd_regs::esp32c3::addr("APB_CTRL", "CLK_OUT_EN") - APB_CTRL_BASE;
         assert_eq!(
-            apb.read(ctx, wifi_clk_en),
+            apb.read(ctx, clk_out_en),
             None,
-            "WIFI_CLK_EN belongs to a later plan"
+            "CLK_OUT_EN is not modelled"
         );
     }
 }
```

```diff
--- a/crates/soc-esp32c3/src/periph/efuse.rs
+++ b/crates/soc-esp32c3/src/periph/efuse.rs
@@ -284,7 +295,11 @@ mod tests {
     fn reserved_holes_between_documented_registers_are_unmodeled() {
         let ctx = &mut Ctx::default();
         let mut efuse = Efuse::new(ChipRevision::V0_3);
-        assert_eq!(efuse.read(ctx, 0x18C), None, "hole");
+        assert_eq!(
+            efuse.read(ctx, 0x18C),
+            Some(0),
+            "the hole ESP-IDF reads as a fifth repeat-error word"
+        );
         assert_eq!(efuse.read(ctx, 0x194), None, "hole");
         let answered = (0x2C..=0x1C4)
             .step_by(4)
@@ -292,12 +307,12 @@ mod tests {
             .count();
         assert_eq!(
             answered,
-            svd_read_offsets().len(),
-            "the mirror answers exactly the documented registers"
+            svd_read_offsets().len() + 1,
+            "the mirror answers exactly the documented registers, and the one hole ESP-IDF reads"
         );
         assert_eq!(
-            answered, 91,
-            "103 words in the window minus 12 reserved holes; a changed count means the SVD changed"
+            answered, 92,
+            "103 words in the window minus 11 unanswered holes; a changed count means the SVD changed"
         );
     }
 
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p soc-esp32c3 --lib > /tmp/t12.log 2>&1; echo "exit=$?"`
Expected: `exit=101`: `FLASH_WAITI_CTRL` reads `None`, `WIFI_CLK_EN` reads `None`, the eFuse word at `0x18C` reads `None`.

- [ ] **Step 3: The rows**

```diff
--- a/crates/soc-esp32c3/src/periph/spi_mem.rs
+++ b/crates/soc-esp32c3/src/periph/spi_mem.rs
@@ -70,7 +70,7 @@ const MODELED_COMMAND_BITS: u32 = 8;
 const DLEN_MASK: u32 = 0x3FF;
 
 /// Configuration of SPI1 that only shapes the electrical transfer, which is instantaneous here.
-const SPI1_ACCEPTED: [Accepted; 6] = [
+const SPI1_ACCEPTED: [Accepted; 9] = [
     Accepted::svd("SPI1", "CTRL", "read mode, dummy output, WP level", "SP8"),
     Accepted::svd(
         "SPI1",
@@ -89,9 +89,23 @@ const SPI1_ACCEPTED: [Accepted; 6] = [
         "SPI1",
         "SUS_STATUS",
         "program/erase suspend state",
-        "SP1b-3",
+        "SP1b-3b",
+    ),
+    Accepted::svd("SPI1", "CLOCK_GATE", "register clock gate", "SP8"),
+    // `spi_flash_hal_device_config` and what it calls (`IDF:hal/spi_flash_hal_common.inc:45`; F1).
+    Accepted::svd("SPI1", "MISC", "chip-select idle level and enables", "SP8"),
+    Accepted::svd(
+        "SPI1",
+        "FLASH_WAITI_CTRL",
+        "automatic wait for the chip to be idle after a program or an erase",
+        "SP1b-3b",
+    ),
+    Accepted::svd(
+        "SPI1",
+        "FLASH_SUS_CTRL",
+        "automatic program/erase suspend and resume",
+        "SP1b-3b",
     ),
-    Accepted::svd("SPI1", "CLOCK_GATE", "register clock gate", "SP1b-3"),
 ];
 
 /// SPI0 `+0x24` and `+0x28`: `MOSI_DLEN` and `MISO_DLEN`. The SVD omits them for SPI0; ESP-IDF
@@ -101,7 +115,7 @@ const SPI0_MISO_DLEN: u32 = SPI0_BASE + 0x28;
 const SPI0_FSM: u32 = addr("SPI0", "FSM") - SPI0_BASE;
 const WHY_SPI0: &str =
     "cache-fill transfer (read command, dummy cycles, clock): the bus reads the flash directly";
-const SPI0_ACCEPTED: [Accepted; 11] = [
+const SPI0_ACCEPTED: [Accepted; 12] = [
     Accepted::svd("SPI0", "CTRL", WHY_SPI0, "SP1b-4"),
     Accepted::svd("SPI0", "CTRL2", WHY_SPI0, "SP1b-4"),
     Accepted::svd("SPI0", "CLOCK", WHY_SPI0, "SP1b-4"),
@@ -112,7 +126,14 @@ const SPI0_ACCEPTED: [Accepted; 11] = [
     Accepted::at("SPI0", "MISO_DLEN", SPI0_MISO_DLEN, WHY_SPI0, "SP1b-4"),
     Accepted::svd("SPI0", "MISC", "chip-select enables", "SP1b-4"),
     Accepted::svd("SPI0", "CACHE_FCTRL", "cache request enable", "SP1b-4"),
-    Accepted::svd("SPI0", "CLOCK_GATE", "register clock gate", "SP1b-3"),
+    Accepted::svd("SPI0", "CLOCK_GATE", "register clock gate", "SP8"),
+    // Read, never written: `spimem_flash_ll_get_source_freq_mhz` (`spimem_flash_ll.h:630`; F1).
+    Accepted::svd(
+        "SPI0",
+        "CORE_CLK_SEL",
+        "MSPI core clock selection: a transfer takes no time",
+        "SP8",
+    ),
 ];
 
 pub(crate) fn accepted() -> Vec<Accepted> {
```

```diff
--- a/crates/soc-esp32c3/src/periph/gpio.rs
+++ b/crates/soc-esp32c3/src/periph/gpio.rs
@@ -9,7 +9,8 @@ const STRAP: u32 = svd_regs::esp32c3::addr("GPIO", "STRAP") - GPIO_BASE;
 
 /// What the ROM's flash-boot path reads back and modifies (FB §B.1): the output enables and the two
 /// input selectors of the flash pads. Pins themselves arrive with the board wiring (SP2).
-const ACCEPTED: [Accepted; 3] = [
+const WHY_IN_SEL: &str = "input signal routing (GPIO matrix)";
+const ACCEPTED: [Accepted; 7] = [
     Accepted::svd("GPIO", "ENABLE", "pin output enable", "SP2"),
     Accepted::svd(
         "GPIO",
@@ -23,6 +24,12 @@ const ACCEPTED: [Accepted; 3] = [
         "input signal routing (GPIO matrix)",
         "SP2",
     ),
+    // The inputs of the flash signals: `esp_restart_noos` returns them to what the ROM expects,
+    // `0x30` in each (`IDF:esp_system/port/soc/esp32c3/system_internal.c:98-103`; trace F1).
+    Accepted::svd("GPIO", "FUNC2_IN_SEL_CFG", WHY_IN_SEL, "SP2"),
+    Accepted::svd("GPIO", "FUNC3_IN_SEL_CFG", WHY_IN_SEL, "SP2"),
+    Accepted::svd("GPIO", "FUNC4_IN_SEL_CFG", WHY_IN_SEL, "SP2"),
+    Accepted::svd("GPIO", "FUNC5_IN_SEL_CFG", WHY_IN_SEL, "SP2"),
 ];
 
 pub(crate) fn accepted() -> Vec<Accepted> {
```

```diff
--- a/crates/soc-esp32c3/src/periph/apb_ctrl.rs
+++ b/crates/soc-esp32c3/src/periph/apb_ctrl.rs
@@ -12,7 +12,7 @@ const RND_DATA: u32 = addr("RNG", "DATA") - APB_CTRL_BASE;
 /// xorshift generators, scrambled", 2016).
 const XORSHIFT_STAR: u64 = 0x2545_F491_4F6C_DD1D;
 
-const ACCEPTED: [Accepted; 2] = [
+const ACCEPTED: [Accepted; 4] = [
     Accepted::svd(
         "APB_CTRL",
         "FRONT_END_MEM_PD",
@@ -20,6 +20,20 @@ const ACCEPTED: [Accepted; 2] = [
         "SP5a",
     ),
     Accepted::svd("APB_CTRL", "MEM_POWER_UP", "radio memory power", "SP5a"),
+    // `esp_perip_clk_init` gates the radio clocks (`IDF:esp_system/port/soc/esp32c3/clk.c:317`);
+    // the radio resets are pulsed by `esp_system_reset_modules_on_exit` at a restart (trace F1).
+    Accepted::svd(
+        "APB_CTRL",
+        "WIFI_CLK_EN",
+        "radio clock gates: there is no radio",
+        "SP5a",
+    ),
+    Accepted::svd(
+        "APB_CTRL",
+        "WIFI_RST_EN",
+        "radio resets: there is no radio",
+        "SP5a",
+    ),
 ];
 
 pub(crate) fn accepted() -> Vec<Accepted> {
```

```diff
--- a/crates/soc-esp32c3/src/periph/efuse.rs
+++ b/crates/soc-esp32c3/src/periph/efuse.rs
@@ -21,6 +21,14 @@ const RD_LAST: u32 = reg("RD_RS_ERR1");
 const RD_REPEAT_DATA1: u32 = reg("RD_REPEAT_DATA1");
 const WDT_DELAY_SEL_SHIFT: u32 = 16;
 const WDT_DELAY_SEL_MASK: u32 = 0b11;
+/// The word between `RD_REPEAT_ERR3` (`+0x188`) and `RD_REPEAT_ERR4` (`+0x190`). No register is
+/// documented there, in the SVD or in ESP-IDF `efuse_reg.h`; the application reads it all the
+/// same, as the fifth word of the repeat-error block, at every start
+/// (`APP:esp_efuse_utility_check_errors@0x420003f8`, the load at `0x4200045c`).
+///
+/// ASSUMPTION: it reads 0, like the error registers around it on a chip without eFuse errors. A
+/// read on a real chip settles it.
+const RD_REPEAT_ERR_GAP: u32 = reg("RD_REPEAT_ERR3") + svd_regs::REGISTER_BYTES;
 /// `RD_MAC_SPI_SYS_3` bits [20:18]: `WAFER_VERSION_MINOR_LO`.
 const RD_MAC_SPI_SYS_3: u32 = reg("RD_MAC_SPI_SYS_3");
 const WAFER_MINOR_LO_SHIFT: u32 = 18;
@@ -121,6 +129,9 @@ impl Efuse {
 
 impl Peripheral<Ctx> for Efuse {
     fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
+        if offset == RD_REPEAT_ERR_GAP {
+            return Some(0);
+        }
         self.read_mirror
             .binary_search_by_key(&offset, |&(o, _)| o)
             .ok()
```

- [ ] **Step 4: Run the suite, regenerate the table, run the ignored tests**

Run: `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect > /tmp/bless.log 2>&1; echo "exit=$?"; /usr/bin/git diff --stat docs/registers-accepted-without-effect.md`
Expected: `exit=0`; ten rows appear, three change their heir.

Run: `cargo test --workspace > /tmp/t12.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t12.log`
Expected: `exit=0`, `654`.

Run the ignored tests (16, 5, 4 + 1). Expected: all pass.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/soc-esp32c3/src/periph docs/registers-accepted-without-effect.md
/usr/bin/git commit -m "feat(soc): accept the configuration registers the application writes after its first line

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3/src/periph docs/registers-accepted-without-effect.md
```

---

## Task 13: Timer groups: timer, main watchdogs, slow-clock calibration — the application lives its whole life

The last device. Each timer group gets its general-purpose timer, its main watchdog (stages, actions, key, feed, clock gate; the flash-boot protection of group 0 armed by the reset values) and, in group 0, the calibration of the slow clock against the crystal (decisions E6, E7, E26, E27, E29). With `TIMG0.RTCCALICFG` modelled the reference application no longer stops where plan SP1b-2 ended: it starts its scheduler, prints, counts down eleven seconds, restarts, and the chip boots again. The tests that pinned the old end are replaced here by the tests of the life cycle (decision E39): milestone points 1, 2, 3 and 6.

**Files:**
- Create: `crates/soc-esp32c3/src/periph/timg.rs`
- Modify: `crates/soc-esp32c3/src/ctx.rs`, `crates/soc-esp32c3/src/periph/mod.rs`, `crates/soc-esp32c3/src/periph/system.rs`, `crates/soc-esp32c3/src/machine.rs`, `crates/soc-esp32c3/tests/reference_boot.rs`, `crates/emu-cli/src/run.rs`, `crates/emu-cli/tests/run_cli.rs`, `.github/workflows/ci.yml`, `docs/registers-accepted-without-effect.md`

**Interfaces:**
- Consumes: `Ctx::{now, clock, irq, request_reset}`, `Deadline`, `Source::of`, `ResetCause::{Mwdt0Cpu, Mwdt0Core, Mwdt1Cpu, Mwdt1Core}` (Task 3); `counter::{Counter, Rate}`, `periph::{half_of, replace_half}` (Tasks 5, 7); `clock::XTAL_PERIOD`, `CpuClock.apb_period` (Task 2); `Peripherals::{deliver, reset}`, `Module`, `Esp32C3::{resets, flush_console}`, `SocBus::deliver` (Tasks 4, 9, 11); `Esp32C3::retune` (SP1b-2).
- Produces: `ctx::Group { Timg0, Timg1 }`, `Event::Timg(Group)`, `Event::ClockChanged`, `Module::{Timg0, Timg1}`; `periph::Timg` with `Timg::new(group: Group, flash_boot: bool, ctx: &Ctx) -> Timg` and the crate-visible `wake(&mut self, ctx: &mut Ctx)`, `clock_changed(&mut self, ctx: &mut Ctx)`; `Peripherals.timg0: Timg`, `Peripherals.timg1: Timg`; `periph::TIMG1_BASE`. Registers of each group: `T0CONFIG`, `T0LO`, `T0HI`, `T0UPDATE`, `T0ALARMLO/HI`, `T0LOADLO/HI`, `T0LOAD`, `WDTCONFIG0..5`, `WDTFEED`, `WDTWPROTECT`, `RTCCALICFG`, `RTCCALICFG1`, `RTCCALICFG2`, `INT_ENA_TIMERS`, `INT_RAW_TIMERS`, `INT_ST_TIMERS`, `INT_CLR_TIMERS`, `NTIMG_DATE`, `REGCLK`. Sources 32/33 (`TG_T0`, `TG_WDT`) and 34/35 (`TG1_T0`, `TG1_WDT`). `SYSTEM.PERIP_RST_EN0` bits 13 and 15 reset the groups. The machine delivers `Event::ClockChanged` after every clock change.

- [ ] **Step 1: Write the failing tests of the device**

Values: the flash-boot protection of group 0 is 26 000 000 cycles of the 20 MHz APB clock, 1.3 s, a core reset (TW §D.4); the interrupt watchdog of the application is `WDTCONFIG1 = 0x4E200000` (prescale 20 000 on the crystal: 2 kHz), `WDTCONFIG0 = 0xB83F8000`, stages 600 and 1 200 (TW §B.3); the calibration of the RC oscillator over 1 024 cycles counts 301 176 crystal cycles, `RTCCALICFG1 = 0x024C3C00`, ready after 7.53 ms (TW §B.4). `crates/soc-esp32c3/src/periph/timg.rs`, test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::CpuClock;

    /// One second of device time.
    const SECOND: u64 = 160_000_000;
    const MILLISECOND: u64 = SECOND / 1_000;

    fn group(ctx: &mut Ctx, group: Group) -> Timg {
        let mut timg = Timg::new(group, true, ctx);
        timg.wake(ctx);
        timg
    }

    /// Delivers every wake-up due at the present time, as the machine does.
    fn pump(ctx: &mut Ctx, timg: &mut Timg) {
        while let Some(event) = ctx.take_due() {
            assert_eq!(event, timg.event());
            timg.wake(ctx);
        }
    }

    fn after(ctx: &mut Ctx, timg: &mut Timg, cycles: u64) {
        ctx.sleep_until(ctx.now() + cycles);
        pump(ctx, timg);
    }

    /// What `wdt_hal_init`, `wdt_hal_config_stage` and `wdt_hal_enable` write for the interrupt
    /// watchdog (TW §B.3; values measured in trace M1, as hardware reads them back).
    fn start_the_interrupt_watchdog(ctx: &mut Ctx, timg: &mut Timg) {
        for (offset, value) in [
            (WDTWPROTECT, WDT_KEY),
            (WDTCONFIG0, 0x003F_8000 | WDT_CONF_UPDATE_EN),
            (INT_CLR, INT_WDT),
            (INT_ENA, INT_WDT),
            (REGCLK, 0x6000_0000),
            (WDTCONFIG1, 20_000 << 16),
            (WDTCONFIG0, 0x203F_8000 | WDT_CONF_UPDATE_EN),
            (WDTCONFIG2, 600),
            (WDTCONFIG0, 0x383F_8000 | WDT_CONF_UPDATE_EN),
            (WDTCONFIG2 + 4, 1_200),
            (WDTFEED, 1),
            (WDTCONFIG0, 0xB83F_8000 | WDT_CONF_UPDATE_EN),
            (WDTWPROTECT, 0),
        ] {
            assert!(timg.write(ctx, offset, value), "{offset:#x}");
        }
    }

    #[test]
    fn reset_values_are_those_of_the_header() {
        // TW §B.1.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        for (offset, value) in [
            (T0CONFIG, 0x6000_2000),
            (WDTCONFIG0, 0x0004_C000),
            (WDTCONFIG1, 0x0001_0000),
            (WDTCONFIG2, 26_000_000),
            (WDTCONFIG2 + 4, 134_217_727),
            (WDTCONFIG2 + 8, 1_048_575),
            (WDTCONFIG5, 1_048_575),
            (WDTWPROTECT, 0x50D8_3AA1),
            (RTCCALICFG, 0x0001_3000),
            (RTCCALICFG1, 0),
            (RTCCALICFG2, 0xFFFF_FF98),
            (INT_ENA, 0),
            (INT_RAW, 0),
            (NTIMG_DATE, 0x0200_6191),
            (REGCLK, 0x6000_0000),
        ] {
            assert_eq!(timg.read(ctx, offset), Some(value), "{offset:#x}");
        }
        assert_eq!(
            timg.read(ctx, T0LOAD + 4),
            None,
            "no second timer on the C3"
        );
        assert!(!timg.write(ctx, INT_ST, 1), "read-only");
        assert_eq!(
            ctx.next_wake(),
            None,
            "group 1 has no flash-boot protection"
        );
    }

    #[test]
    fn the_flash_boot_protection_of_group_0_resets_the_core_after_26_million_apb_cycles() {
        // TW §D.4: 26 000 000 cycles of APB_CLK at 20 MHz, 1.3 s; cause 0x07.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg0);
        assert_eq!(ctx.next_wake(), Some(Cycles(13 * SECOND / 10)));
        after(ctx, &mut timg, 13 * SECOND / 10 - 1);
        assert_eq!(ctx.take_reset(), None);
        after(ctx, &mut timg, 1);
        assert_eq!(ctx.take_reset(), Some(ResetCause::Mwdt0Core));
    }

    #[test]
    fn the_protection_follows_the_apb_clock_and_stops_when_its_bit_is_cleared() {
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg0);
        // Half of the timeout at 20 MHz, then APB at 40 MHz as after `esp_restart` (TW §D.4).
        after(ctx, &mut timg, 65 * SECOND / 100);
        ctx.set_clock(CpuClock {
            cpu_period: 4,
            apb_period: 4,
        });
        timg.clock_changed(ctx);
        assert_eq!(
            ctx.next_wake(),
            Some(Cycles(975 * SECOND / 1_000)),
            "0.65 s + 0.325 s"
        );
        // What `bootloader_config_wdt` writes: unlock, clear the bit, lock (TW §B.3).
        timg.write(ctx, WDTWPROTECT, WDT_KEY);
        timg.write(ctx, WDTCONFIG0, 0x0004_8000 | WDT_CONF_UPDATE_EN);
        timg.write(ctx, WDTWPROTECT, 0);
        assert_eq!(
            timg.read(ctx, WDTCONFIG0),
            Some(0x0004_8000),
            "CONF_UPDATE_EN reads 0"
        );
        after(ctx, &mut timg, 10 * SECOND);
        assert_eq!(ctx.take_reset(), None, "stopped");
    }

    #[test]
    fn the_bit_cleared_without_the_update_strobe_stops_the_protection_too() {
        // `_timer_ll_reset_register`: a plain read-modify-write right after a peripheral reset.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg0);
        timg.write(ctx, WDTCONFIG0, 0x0004_8000);
        after(ctx, &mut timg, 10 * SECOND);
        assert_eq!(ctx.take_reset(), None);
    }

    #[test]
    fn a_chip_that_does_not_boot_from_flash_has_no_protection() {
        let ctx = &mut Ctx::default();
        let mut timg = Timg::new(Group::Timg0, false, ctx);
        timg.wake(ctx);
        assert_eq!(ctx.next_wake(), None);
    }

    #[test]
    fn a_locked_watchdog_ignores_feeds_and_configuration() {
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg0);
        timg.write(ctx, WDTWPROTECT, 0);
        assert!(timg.write(ctx, WDTCONFIG0, 0), "accepted and dropped");
        assert!(timg.write(ctx, WDTFEED, 1));
        assert!(timg.write(ctx, WDTCONFIG2, 5));
        assert_eq!(timg.read(ctx, WDTCONFIG0), Some(0x0004_C000));
        assert_eq!(timg.read(ctx, WDTCONFIG2), Some(26_000_000));
        after(ctx, &mut timg, 13 * SECOND / 10);
        assert_eq!(
            ctx.take_reset(),
            Some(ResetCause::Mwdt0Core),
            "still armed, never fed"
        );
    }

    #[test]
    fn the_interrupt_watchdog_interrupts_after_300_ms_and_resets_the_core_600_ms_later() {
        // TW §B.3, §D.5: stage 0 = interrupt after 600 counts of 500 us, stage 1 = core reset
        // after 1 200 more. Source TG1_WDT is 35.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        start_the_interrupt_watchdog(ctx, &mut timg);
        assert_eq!(timg.read(ctx, WDTCONFIG0), Some(0xB83F_8000));
        after(ctx, &mut timg, 300 * MILLISECOND - 1);
        assert_eq!((timg.read(ctx, INT_RAW), ctx.sources()), (Some(0), 0));
        after(ctx, &mut timg, 1);
        assert_eq!(timg.read(ctx, INT_ST), Some(INT_WDT));
        assert_eq!(ctx.sources(), 1 << 35);
        assert_eq!(ctx.take_reset(), None);
        after(ctx, &mut timg, 600 * MILLISECOND);
        assert_eq!(ctx.take_reset(), Some(ResetCause::Mwdt1Core));
    }

    #[test]
    fn a_watchdog_fed_at_every_tick_never_expires() {
        // The tick hook: unlock, both stages rewritten, feed, lock, every 10 ms (TW §F.3).
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        start_the_interrupt_watchdog(ctx, &mut timg);
        for _ in 0..200 {
            after(ctx, &mut timg, 10 * MILLISECOND);
            for (offset, value) in [
                (WDTWPROTECT, WDT_KEY),
                (WDTCONFIG2, 600),
                (WDTCONFIG0, 0xB83F_8000 | WDT_CONF_UPDATE_EN),
                (WDTCONFIG2 + 4, 1_200),
                (WDTCONFIG0, 0xB83F_8000 | WDT_CONF_UPDATE_EN),
                (WDTFEED, 1),
                (WDTWPROTECT, 0),
            ] {
                timg.write(ctx, offset, value);
            }
        }
        assert_eq!(timg.read(ctx, INT_RAW), Some(0));
        assert_eq!(ctx.take_reset(), None);
        // The ticks stop: the interrupt comes 300 ms after the last feed.
        after(ctx, &mut timg, 300 * MILLISECOND);
        assert_eq!(timg.read(ctx, INT_RAW), Some(INT_WDT));
    }

    #[test]
    fn a_sleep_that_covers_two_stages_runs_both_in_order() {
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        start_the_interrupt_watchdog(ctx, &mut timg);
        ctx.sleep_until(ctx.now() + 2 * SECOND);
        pump(ctx, &mut timg);
        assert_eq!(timg.read(ctx, INT_RAW), Some(INT_WDT), "stage 0");
        assert_eq!(
            ctx.take_reset(),
            Some(ResetCause::Mwdt1Core),
            "then stage 1"
        );
    }

    #[test]
    fn a_cpu_reset_stage_and_an_off_stage_do_what_their_action_says() {
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg0);
        // Stage 0 off (10 counts), stage 1 CPU reset (10 counts), crystal divided by 40 000: 1 ms.
        for (offset, value) in [
            (WDTCONFIG1, 40_000 << 16),
            (WDTCONFIG2, 10),
            (WDTCONFIG2 + 4, 10),
            (WDTFEED, 1),
            (WDTCONFIG0, WDT_EN | WDT_USE_XTAL | (ACTION_RESET_CPU << 27)),
        ] {
            timg.write(ctx, offset, value);
        }
        after(ctx, &mut timg, 10 * MILLISECOND);
        assert_eq!(
            (timg.read(ctx, INT_RAW), ctx.take_reset()),
            (Some(0), None),
            "off"
        );
        after(ctx, &mut timg, 10 * MILLISECOND);
        assert_eq!(ctx.take_reset(), Some(ResetCause::Mwdt0Cpu));
    }

    #[test]
    fn the_watchdog_clock_gate_stops_the_counter() {
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg0);
        after(ctx, &mut timg, SECOND);
        timg.write(ctx, REGCLK, 0x4000_0000);
        after(ctx, &mut timg, 10 * SECOND);
        assert_eq!(ctx.take_reset(), None, "gated");
        timg.write(ctx, REGCLK, 0x6000_0000);
        after(ctx, &mut timg, 3 * SECOND / 10);
        assert_eq!(
            ctx.take_reset(),
            Some(ResetCause::Mwdt0Core),
            "1 s + 0.3 s of counting"
        );
    }

    /// The one-shot measurement of `rtc_clk_cal_internal` (TW §B.4), `cycles` slow-clock cycles of
    /// the clock `clk_sel`.
    fn calibrate(ctx: &mut Ctx, timg: &mut Timg, clk_sel: u32, cycles: u32, shift: u32) {
        // Step 1: the cycling mode is on at reset; a threshold of 1 ends it by a timeout.
        assert_eq!(timg.read(ctx, RTCCALICFG), Some(0x0001_3000));
        assert!(timg.write(ctx, RTCCALICFG2, 0x0000_0098));
        assert_eq!(timg.read(ctx, RTCCALICFG2), Some(0x0000_0099), "TIMEOUT");
        // Steps 2 to 4.
        let config = (cycles << CALI_MAX_SHIFT) | (clk_sel << CALI_CLK_SEL_SHIFT);
        timg.write(ctx, RTCCALICFG, config);
        timg.write(
            ctx,
            RTCCALICFG2,
            ((cycles << shift) << CALI_THRES_SHIFT) | 0x18,
        );
        timg.write(ctx, RTCCALICFG, config);
        timg.write(ctx, RTCCALICFG, config | CALI_START);
    }

    #[test]
    fn the_rc_slow_calibration_is_ready_after_7_53_ms_with_301_176_crystal_cycles() {
        // TW §B.4: 1 024 cycles of the 136 kHz RC; `RTCCALICFG1` reads 0x024C3C00.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg0);
        calibrate(ctx, &mut timg, 0, 1_024, 10);
        assert_eq!(timg.read(ctx, RTCCALICFG).map(|v| v & CALI_RDY), Some(0));
        assert_eq!(
            timg.read(ctx, RTCCALICFG1),
            Some(0),
            "no value before it is ready"
        );
        // 301 176 crystal cycles: 7.5294 ms.
        ctx.sleep_until(Cycles(ctx.now().get() + 301_176 * 4 - 1));
        assert_eq!(timg.read(ctx, RTCCALICFG).map(|v| v & CALI_RDY), Some(0));
        ctx.sleep_until(ctx.now() + 1);
        assert_eq!(timg.read(ctx, RTCCALICFG), Some(0x8400_8000));
        assert_eq!(timg.read(ctx, RTCCALICFG1), Some(0x024C_3C00));
        assert_eq!(
            timg.read(ctx, RTCCALICFG2).map(|v| v & CALI_TIMEOUT),
            Some(0)
        );
        // Step 7: `START` is cleared; the result stays until the next start.
        timg.write(ctx, RTCCALICFG, 0x0400_8000);
        assert_eq!(timg.read(ctx, RTCCALICFG), Some(0x0400_8000));
    }

    #[test]
    fn the_rc_fast_calibration_counts_599_186_crystal_cycles() {
        // TW §B.4: 17.5 MHz / 256; `RTCCALICFG1` reads 0x04924900.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg0);
        calibrate(ctx, &mut timg, 1, 1_024, 12);
        ctx.sleep_until(ctx.now() + 599_186 * 4);
        assert_eq!(timg.read(ctx, RTCCALICFG1), Some(0x0492_4900));
    }

    #[test]
    fn a_calibration_of_the_missing_32_khz_crystal_times_out_without_a_result() {
        // Pins the ASSUMPTION of `Calibration::start`.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg0);
        calibrate(ctx, &mut timg, 2, 1_024, 12);
        assert_eq!(
            timg.read(ctx, RTCCALICFG2).map(|v| v & CALI_TIMEOUT),
            Some(0)
        );
        // The threshold: 1 024 << 12 crystal cycles, 104.9 ms.
        ctx.sleep_until(ctx.now() + (1_024 << 12) * 4);
        assert_eq!(
            timg.read(ctx, RTCCALICFG2).map(|v| v & CALI_TIMEOUT),
            Some(1)
        );
        assert_eq!(timg.read(ctx, RTCCALICFG).map(|v| v & CALI_RDY), Some(0));
        assert_eq!(timg.read(ctx, RTCCALICFG1), Some(0));
    }

    fn t0_value(ctx: &mut Ctx, timg: &mut Timg) -> u64 {
        assert!(timg.write(ctx, T0UPDATE, 1));
        assert_eq!(timg.read(ctx, T0UPDATE), Some(0), "the capture is done");
        (u64::from(timg.read(ctx, T0HI).unwrap()) << 32) | u64::from(timg.read(ctx, T0LO).unwrap())
    }

    #[test]
    fn t0_counts_the_apb_clock_divided_and_keeps_its_value_when_stopped() {
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        // EN, counting up, divider 20: 20 MHz / 20, one count per microsecond.
        timg.write(
            ctx,
            T0CONFIG,
            T0_EN | T0_INCREASE | (20 << T0_DIVIDER_SHIFT),
        );
        after(ctx, &mut timg, MILLISECOND);
        assert_eq!(t0_value(ctx, &mut timg), 1_000);
        timg.write(ctx, T0CONFIG, T0_INCREASE | (20 << T0_DIVIDER_SHIFT));
        after(ctx, &mut timg, MILLISECOND);
        assert_eq!(t0_value(ctx, &mut timg), 1_000, "frozen");
        // The reset divider, 1, divides by 2; the crystal as source: 20 MHz.
        timg.write(
            ctx,
            T0CONFIG,
            T0_EN | T0_INCREASE | (1 << T0_DIVIDER_SHIFT) | T0_USE_XTAL,
        );
        after(ctx, &mut timg, MILLISECOND);
        assert_eq!(t0_value(ctx, &mut timg), 21_000);
    }

    #[test]
    fn t0_counts_down_and_reloads() {
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg1);
        timg.write(ctx, T0LOADLO, 5_000);
        timg.write(ctx, T0LOADHI, 0);
        timg.write(ctx, T0LOAD, 1);
        timg.write(ctx, T0CONFIG, T0_EN | (20 << T0_DIVIDER_SHIFT));
        after(ctx, &mut timg, MILLISECOND);
        assert_eq!(t0_value(ctx, &mut timg), 4_000);
        timg.write(ctx, T0LOAD, 0);
        assert_eq!(
            t0_value(ctx, &mut timg),
            5_000,
            "any write to T0LOAD reloads"
        );
    }

    #[test]
    fn the_t0_alarm_interrupts_clears_its_enable_and_reloads_the_counter() {
        // TW §B.2. Source TG_T0 is 32.
        let ctx = &mut Ctx::default();
        let mut timg = group(ctx, Group::Timg0);
        timg.write(ctx, WDTCONFIG0, 0);
        timg.write(ctx, INT_ENA, INT_T0);
        timg.write(ctx, T0ALARMLO, 1_000);
        timg.write(ctx, T0ALARMHI, 0);
        let config = T0_EN | T0_INCREASE | T0_AUTORELOAD | (20 << T0_DIVIDER_SHIFT);
        timg.write(ctx, T0CONFIG, config | T0_ALARM_EN);
        after(ctx, &mut timg, MILLISECOND - 1);
        assert_eq!(timg.read(ctx, INT_RAW), Some(0));
        after(ctx, &mut timg, 1);
        assert_eq!(timg.read(ctx, INT_ST), Some(INT_T0));
        assert_eq!(ctx.sources(), 1 << 32);
        assert_eq!(
            timg.read(ctx, T0CONFIG),
            Some(config),
            "ALARM_EN cleared by hardware"
        );
        assert_eq!(t0_value(ctx, &mut timg), 0, "reloaded from T0LOAD");
        timg.write(ctx, INT_CLR, INT_T0);
        assert_eq!(ctx.sources(), 0);
        // An alarm value already passed fires at once when the alarm is enabled.
        after(ctx, &mut timg, 2 * MILLISECOND);
        timg.write(ctx, T0CONFIG, config | T0_ALARM_EN);
        assert_eq!(timg.read(ctx, INT_RAW), Some(INT_T0));
    }
}
```

- [ ] **Step 2: Write the failing tests of the life cycle**

The machine, the peripherals as a whole and `SYSTEM`. The sleeping test of Task 8 now meets the wake-up the main watchdog asked for before it was stopped: a call that finds nothing to do (decision E19).

```diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -993,8 +998,8 @@ mod tests {
     #[test]
     fn a_cpu_that_sleeps_while_a_device_counts_gives_control_back_after_a_second() {
         // A one-shot alarm two seconds ahead (32 000 000 counts at 16 MHz), its interrupt not
-        // enabled: nothing will wake the CPU, and time must still pass. The RTC watchdog the reset
-        // values arm is stopped first: it would reset the chip at 2.94 s.
+        // enabled: nothing will wake the CPU, and time must still pass. The watchdogs the reset
+        // values arm are stopped first: they would reset the chip at 1.3 s.
         let mut body = stop_watchdogs();
         body.extend(store_register(reg("SYSTIMER", "TARGET0_LO"), 32_000_000));
         body.extend(store_register(reg("SYSTIMER", "COMP0_LOAD"), 1));
@@ -1004,16 +1009,25 @@ mod tests {
         ));
         body.extend([wfi(), HANG]);
         let mut m = machine(&body);
-        assert_eq!(m.run_until(Cycles(10_000)), StopReason::Asleep);
+        // The wake-up the main watchdog had asked for before it was stopped is still scheduled:
+        // it ends the first sleep at 1.3 s and finds nothing to do (ADR 0003). Every call gives
+        // control back; device time goes on to the alarm.
+        let mut calls = 0;
+        while bus_word(&mut m, reg("SYSTIMER", "INT_RAW")) == 0 {
+            assert_eq!(m.run_until(Cycles(10_000)), StopReason::Asleep);
+            calls += 1;
+            assert!(calls < 10, "device time does not advance");
+        }
         assert_eq!(
-            m.device_time_nanos(),
-            2_000_000_000,
-            "slept to the alarm in one jump"
+            calls, 2,
+            "one call to the stale wake-up, one to the alarm and beyond"
         );
-        assert_eq!(
-            m.run_until(Cycles(10_000)),
-            StopReason::Idle,
-            "the alarm has fired and nothing else is scheduled"
+        assert!(m.device_time_nanos() >= 2_000_000_000);
+        assert_eq!(m.resets(), 0, "no watchdog fired");
+        assert!(
+            m.cycles().get() < 100,
+            "the CPU executed nothing while it slept: {} cycles",
+            m.cycles().get()
         );
     }
 
@@ -1092,4 +1106,24 @@ mod tests {
             "and so does device time"
         );
     }
+
+    #[test]
+    fn the_flash_boot_watchdog_resets_a_cpu_that_only_sleeps() {
+        // The reset values arm main watchdog 0 (TW §D.4): 26 000 000 cycles of the 20 MHz APB
+        // clock, 1.3 s. Its deadline is the first thing scheduled.
+        let mut m = machine(&[wfi(), HANG]);
+        assert_eq!(m.run_until(Cycles(10_000)), StopReason::Asleep);
+        assert_eq!(
+            m.device_time_nanos(),
+            1_300_000_000,
+            "slept to the watchdog deadline"
+        );
+        assert_eq!(m.resets(), 1, "which reset the chip");
+        assert_eq!(
+            bus_word(&mut m, svd_regs::esp32c3::addr("RTC_CNTL", "RESET_STATE")) & 0x3F,
+            0x07,
+            "TG0WDT_SYS_RST"
+        );
+        assert_eq!(m.cycles().get(), 3, "only `wfi` was executed");
+    }
 }
```

```diff
--- a/crates/soc-esp32c3/src/periph/mod.rs
+++ b/crates/soc-esp32c3/src/periph/mod.rs
@@ -779,18 +798,6 @@ mod tests {
                 .and_then(|(device, base)| device.read(ctx, addr - base))
         };
         let svd = svd_regs::esp32c3::addr;
-        assert_eq!(
-            read(svd("TIMG0", "WDTCONFIG0")),
-            Some(0x0004_C000),
-            "FLASHBOOT_MOD_EN set at reset (BC §E.3)"
-        );
-        assert_eq!(read(svd("TIMG0", "WDTWPROTECT")), Some(0x50D8_3AA1));
-        assert_eq!(
-            read(svd("TIMG0", "RTCCALICFG")),
-            None,
-            "the RTC calibration is where SP1b-3 starts"
-        );
-        assert_eq!(read(svd("TIMG0", "T0CONFIG")), None);
         assert_eq!(read(svd("APB_SARADC", "CTRL")), Some(0x4003_8240));
         assert_eq!(read(svd("APB_SARADC", "CLKM_CONF")), Some(4));
         assert_eq!(
@@ -820,7 +827,6 @@ mod tests {
         let mut p = peripherals(ctx);
         let svd = svd_regs::esp32c3::addr;
         for (addr, value) in [
-            (svd("TIMG0", "WDTCONFIG0"), 0x8000_0000),
             (svd("APB_SARADC", "CTRL2"), 0x0000_A5A5),
             (svd("BB", "BBPD_CTRL"), 0x0000_000F),
             (NRX_NRXPD_CTRL, 0x1234_5678),
@@ -948,7 +954,7 @@ mod tests {
         assert_eq!(
             read(svd("TIMG0", "WDTCONFIG0")),
             Some(0x0004_C000),
-            "its reset value, `FLASHBOOT_MOD_EN` set"
+            "the flash-boot protection is armed again"
         );
     }
 
```

```diff
--- a/crates/soc-esp32c3/src/periph/system.rs
+++ b/crates/soc-esp32c3/src/periph/system.rs
@@ -335,7 +337,11 @@ mod tests {
         sys.write(ctx, PERIP_RST_EN0, reset0 | 0x2000_0007);
         sys.write(ctx, PERIP_RST_EN0, reset0);
         assert_eq!(ctx.take_signal(), None, "held, then released: no new reset");
-        sys.write(ctx, PERIP_RST_EN0, reset0 | (1 << 5));
+        sys.write(
+            ctx,
+            PERIP_RST_EN0,
+            reset0 | (1 << 13) | (1 << 15) | (1 << 5),
+        );
         // `PERIP_RST_EN1` holds the crypto modules in reset at power-on (reset value 0x1FE): the
         // bootloader released the SHA accelerator to use it.
         sys.write(ctx, PERIP_RST_EN1, 0);
@@ -348,6 +354,8 @@ mod tests {
             signalled,
             [
                 Event::ModuleReset(Module::Uart1),
+                Event::ModuleReset(Module::Timg0),
+                Event::ModuleReset(Module::Timg1),
                 Event::ModuleReset(Module::Sha)
             ]
         );
```

```diff
--- a/crates/emu-cli/src/run.rs
+++ b/crates/emu-cli/src/run.rs
@@ -523,4 +523,29 @@ mod tests {
             "the bytes still in the UART are not lost"
         );
     }
+
+    #[test]
+    fn a_cpu_asleep_while_a_watchdog_counts_does_not_end_the_run() {
+        // Each life of this firmware is one byte and a `wfi`; main watchdog 0, armed by the reset
+        // values, resets the chip every 1.3 s of device time (TW §D.4). The run goes on through
+        // the sleeps and the resets until its cycle limit.
+        let mut m = machine(
+            &printing(UART0, b"a", &[wfi(), HANG]),
+            MachineOptions::default(),
+        );
+        let (mut console, mut log) = (Vec::new(), Vec::new());
+        let end = execute(&mut m, &options(300), &mut console, &mut log);
+        assert_eq!(end, RunEnd::Budget);
+        assert!(
+            console.len() > 5 && console.iter().all(|&byte| byte == b'a'),
+            "one `a` per life: {console:?}"
+        );
+        // The byte of the life the limit interrupted may not have left the UART yet.
+        let lives = u64::try_from(console.len()).unwrap();
+        assert!(
+            (m.resets()..=m.resets() + 1).contains(&lives),
+            "{lives} bytes"
+        );
+        assert!(m.device_time_nanos() > 5 * 1_300_000_000);
+    }
 }
```

The reference image. The application lines up to the heap size are the QEMU capture of TO §2 line for line; the capture ends there. The differences from the oracle are the two of plan SP1b-2 (`efuse block revision: v1.2`, `SPI Mode : DIO`) and one new: the `Saved PC` line of the second boot, which a real chip prints and QEMU never does (IR §4.4, §6 row 16). Two things are pinned beyond the log. The countdown: the device time at which each of the eleven `Restarting in N seconds...` lines has left the UART is read from the machine, and two consecutive lines are one second apart to one tick of the application's 100 Hz scheduler (measured: the first gap is 0.990 s, the first delay starting inside a tick, the nine others 1.000 s), plus the 1.25 µs resolution of the reading. The reset the determinism test crosses: it must be `rst:0xc (RTC_SW_CPU_RST)`, with the first application line of the second boot behind it, so that a watchdog reset cannot stand in for the restart.

```diff
--- a/crates/soc-esp32c3/tests/reference_boot.rs
+++ b/crates/soc-esp32c3/tests/reference_boot.rs
@@ -1,4 +1,5 @@
-//! The milestone of plan SP1b-2 on the reference firmware (ESP-IDF v5.5.2 bootloader + `hello_world`):
+//! The milestones of plans SP1b-2 and SP1b-3a on the reference firmware (ESP-IDF v5.5.2 bootloader +
+//! `hello_world`): the boot, then the whole life of the application, its restart and a second boot.
 //! `INKEMU_ROM_ELF_DIR=<dir> INKEMU_REF_FLASH=<flash_hello.bin> cargo test -p soc-esp32c3 --test reference_boot -- --ignored`
 //! Build the image with `tools/reference-firmware/build.sh`.
 #![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure
@@ -49,15 +50,71 @@ const BOOTLOADER_LINES: [&str; 21] = [
     "I (#) boot: Disabling RNG early entropy source...",
 ];
 const FIRST_APPLICATION_LINE: &str = "I (#) cpu_start: Unicore app";
-/// `call_start_cpu0` of the application (BA §1).
-const APPLICATION_ENTRY: u32 = 0x4038_0330;
-/// The address ranges of the application's code, from the `esp_image` segment lines of the golden
-/// log: IRAM is segment 2 (`vaddr=40380000 size=088a4h`) followed contiguously by segment 4
-/// (`vaddr=403888a4 size=02c70h`), ending at 0x4038b514; the flash text window is segment 3
-/// (`vaddr=42000020 size=0e6fch`), ending at 0x4200e71c. The bootloader runs at `0x403c_xxxx` and the
-/// ROM below `0x4006_0000`, so neither is inside.
-const APPLICATION_CODE: [std::ops::Range<u32>; 2] =
-    [0x4038_0000..0x4038_b514, 0x4200_0020..0x4200_e71c];
+/// What the application prints from its first line to its restart, timestamps masked. Up to the
+/// heap size it is the QEMU capture of TO §2 line for line; the capture ends there. The eleven
+/// `Restarting in` lines are `hello_world_main.c`'s loop from 10 down to 0.
+const APPLICATION_LINES: [&str; 39] = [
+    FIRST_APPLICATION_LINE,
+    "I (#) cpu_start: GPIO 20 and 21 are used as console UART I/O pins",
+    "I (#) cpu_start: Pro cpu start user code",
+    "I (#) cpu_start: cpu freq: 160000000 Hz",
+    "I (#) app_init: Application information:",
+    "I (#) app_init: Project name:     hello_world",
+    "I (#) app_init: App version:      1",
+    "I (#) app_init: ELF file SHA256:  43d81da8d...",
+    "I (#) app_init: ESP-IDF:          v5.5.2",
+    "I (#) efuse_init: Min chip rev:     v0.3",
+    "I (#) efuse_init: Max chip rev:     v1.99 ",
+    "I (#) efuse_init: Chip rev:         v0.3",
+    "I (#) heap_init: Initializing. RAM available for dynamic allocation:",
+    "I (#) heap_init: At 3FC8D610 len 000329F0 (202 KiB): RAM",
+    "I (#) heap_init: At 3FCC0000 len 0001C710 (113 KiB): Retention RAM",
+    "I (#) heap_init: At 3FCDC710 len 00002950 (10 KiB): Retention RAM",
+    "I (#) heap_init: At 50000020 len 00001FC8 (7 KiB): RTCRAM",
+    "I (#) spi_flash: detected chip: issi",
+    "I (#) spi_flash: flash io: dio",
+    "I (#) sleep_gpio: Configure to isolate all GPIO pins in sleep state",
+    "I (#) sleep_gpio: Enable automatic switching of GPIO sleep configuration",
+    "I (#) main_task: Started on CPU0",
+    "I (#) main_task: Calling app_main()",
+    "Hello world!",
+    "This is esp32c3 chip with 1 CPU core(s), WiFi/BLE, silicon revision v0.3, 16MB external flash",
+    "Minimum free heap size: 332800 bytes",
+    "Restarting in 10 seconds...",
+    "Restarting in 9 seconds...",
+    "Restarting in 8 seconds...",
+    "Restarting in 7 seconds...",
+    "Restarting in 6 seconds...",
+    "Restarting in 5 seconds...",
+    "Restarting in 4 seconds...",
+    "Restarting in 3 seconds...",
+    "Restarting in 2 seconds...",
+    "Restarting in 1 seconds...",
+    "Restarting in 0 seconds...",
+    "Restarting now.",
+    // The second boot starts here: the banner of the ROM.
+    "ESP-ROM:esp32c3-api1-20210207",
+];
+/// The ROM lines of the boot that follows `esp_restart`. Two differences from the first boot: the
+/// cause, `RTC_SW_CPU_RST` (IR §4.3), and the `Saved PC` line a real chip prints after a CPU reset
+/// and the QEMU oracle never does (IR §4.4, §6 row 16).
+///
+/// The address is the instruction after the store that asked for the reset, in the ROM's
+/// `software_reset_cpu`: the model resets at the end of that instruction (`ASSUMPTION:` in
+/// `AssistDebug::cpu_reset`; a real chip shows an application address).
+const SECOND_BOOT_ROM_LINES: [&str; 9] = [
+    "Build:Feb  7 2021",
+    "rst:0xc (RTC_SW_CPU_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)",
+    "Saved PC:0x40048b92",
+    "SPIWP:0xee",
+    "mode:DIO, clock div:1",
+    "load:0x3fcd5820,len:0x15a4",
+    "load:0x403cbf10,len:0xc64",
+    "load:0x403ce710,len:0x2fd0",
+    "entry 0x403cbf1a",
+];
+/// One second of device time, in nanoseconds.
+const SECOND_NS: u64 = 1_000_000_000;
 /// The stock partition table of the Xteink X4 (repository fixture, built from `x4-stock.csv`):
 /// `nvs`, `otadata`, two OTA slots — `app0` at 0x10000, where the reference image has its
 /// application — `spiffs` and `coredump`; no factory application.
@@ -126,6 +183,14 @@ struct Run {
     diagnostics: Vec<Diagnostic>,
     stop: StopReason,
     trace_hash: Option<u64>,
+    resets: u64,
+    cycles: u64,
+    device_time_nanos: u64,
+}
+
+/// How many times the application printed its first line: once per boot that reached it.
+fn application_starts(console: &str) -> usize {
+    console.matches("cpu_start: Unicore app\n").count()
 }
 
 /// Runs the reference image until `done` accepts the console, the machine stops, or the budget ends.
@@ -140,7 +205,8 @@ fn run_image(image: Vec<u8>, config: SocConfig, done: impl Fn(&str, &[Diagnostic
     let (mut uart0, mut diagnostics) = (Vec::new(), Vec::new());
     let mut stop = StopReason::CycleBudget;
     let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace('\r', "");
-    while stop == StopReason::CycleBudget
+    // `Asleep` is the CPU waiting for its next interrupt: device time goes on, the run too.
+    while matches!(stop, StopReason::CycleBudget | StopReason::Asleep)
         && machine.cycles().get() < BUDGET
         && !done(&text(&uart0), &diagnostics)
     {
@@ -156,6 +222,9 @@ fn run_image(image: Vec<u8>, config: SocConfig, done: impl Fn(&str, &[Diagnostic
         diagnostics,
         stop,
         trace_hash: machine.trace_hash(),
+        resets: machine.resets(),
+        cycles: machine.cycles().get(),
+        device_time_nanos: machine.device_time_nanos(),
     }
 }
 
@@ -196,114 +265,140 @@ fn rom_bootloader_and_application_print_the_golden_log() {
     );
 }
 
+/// Milestone point 1 of plan SP1b-3a: the application lives its whole life, restarts, and the
+/// chip boots a second time.
 #[test]
 #[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
-fn strict_run_reaches_the_application_and_ends_on_the_rtc_calibration() {
-    let strict = SocConfig {
-        strict_mmio: true,
-        ..SocConfig::default()
-    };
-    let boot = run(strict, |_| false);
-    // Milestone point 4: nothing unmodelled and no exception from reset to the application entry;
-    // then the run ends on a *named* register, the first one plan SP1b-3 has to model (BA §9).
-    let StopReason::StrictMmio(violation) = &boot.stop else {
-        panic!(
-            "expected a strict stop, got {:?}\nconsole:\n{}",
-            boot.stop, boot.console
-        );
-    };
+fn the_whole_life_cycle_prints_the_golden_log_and_boots_a_second_time() {
+    let life = run(SocConfig::default(), |console| {
+        application_starts(console) == 2
+    });
     assert!(
-        matches!(
-            violation,
-            Diagnostic::UnmodeledMmio {
-                register: Some(("TIMG0", "RTCCALICFG")),
-                ..
-            }
-        ),
-        "the run must end on TIMG0.RTCCALICFG, it ended on: {violation}\nconsole:\n{}",
-        boot.console
+        life.console.starts_with(ROM_LINES),
+        "ROM part:\n{}",
+        life.console
     );
+    let after_rom: Vec<String> = life.console[ROM_LINES.len()..]
+        .lines()
+        .map(mask_timestamp)
+        .collect();
+    let mut expected: Vec<&str> = BOOTLOADER_LINES.to_vec();
+    expected.extend(APPLICATION_LINES);
+    expected.extend(SECOND_BOOT_ROM_LINES);
+    expected.extend(BOOTLOADER_LINES);
+    expected.push(FIRST_APPLICATION_LINE);
     assert_eq!(
-        boot.diagnostics,
-        vec![violation.clone()],
-        "nothing else was reported before it"
+        after_rom
+            .iter()
+            .map(String::as_str)
+            .take(expected.len())
+            .collect::<Vec<_>>(),
+        expected,
+        "diagnostics: {:#?}",
+        life.diagnostics
     );
-    // The first application line is printed by the application's own `system_early_init` (BA §3):
-    // seeing it means the bootloader jumped and the application ran from the flash window.
-    assert!(
-        boot.console
-            .lines()
-            .map(mask_timestamp)
-            .any(|line| line == FIRST_APPLICATION_LINE),
-        "{}",
-        boot.console
+    assert_eq!(life.resets, 1, "one reset: the restart");
+    assert_eq!(
+        life.diagnostics,
+        Vec::new(),
+        "nothing to report in a whole life"
     );
-    let Diagnostic::UnmodeledMmio { pc, .. } = violation else {
-        panic!("expected an unmodelled register access, got: {violation}");
+}
+
+/// Milestone point 3: in strict mode a whole life cycle, from power-on through `esp_restart` to
+/// the first line of the second boot, touches no unmodelled register and takes no CPU exception.
+#[test]
+#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
+fn a_strict_run_crosses_the_restart_without_anything_unmodelled() {
+    let strict = SocConfig {
+        strict_mmio: true,
+        ..SocConfig::default()
     };
+    let life = run(strict, |console| application_starts(console) == 2);
     assert!(
-        APPLICATION_CODE.iter().any(|code| code.contains(pc)),
-        "the access must come from the application's own code {APPLICATION_CODE:x?} (entry {APPLICATION_ENTRY:#010x}), not from the bootloader or the ROM: pc {pc:#010x}"
+        matches!(life.stop, StopReason::CycleBudget | StopReason::Asleep),
+        "nothing stops the run: {:?}\nconsole:\n{}",
+        life.stop,
+        life.console
     );
+    assert_eq!(life.diagnostics, Vec::new());
+    assert_eq!(application_starts(&life.console), 2, "{}", life.console);
+    assert_eq!(life.resets, 1);
 }
 
+/// Milestone point 2: the eleven seconds the application spends in `vTaskDelay` are slept, not
+/// executed. The lines of its countdown are one second of device time apart, and the CPU runs
+/// through its first life in a fraction of the cycles eleven seconds would take at 160 MHz
+/// (1 760 000 000).
 #[test]
 #[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
-fn without_strict_mode_the_wait_on_the_rtc_calibration_is_a_named_warning_not_a_silent_hang() {
-    // Far past the first application line: the application then polls the calibration register.
+fn the_countdown_lines_are_one_device_second_apart_and_the_cpu_sleeps_between_them() {
+    /// CPU cycles between two looks at the console, and the device time they take at the
+    /// 160 MHz the application runs at: the resolution of the times read below.
+    const LOOK: u64 = 200;
+    const LOOK_NS: u64 = 1_250;
+    /// One tick of the application's scheduler (`CONFIG_FREERTOS_HZ` is 100).
+    const TICK_NS: u64 = SECOND_NS / 100;
     let mut machine = Esp32C3::with_flash(rom(), reference_flash(), &SocConfig::default()).unwrap();
-    let stop = machine.run_until(Cycles(60_000_000));
-    let diagnostics = machine.drain_diagnostics();
-    assert_eq!(stop, StopReason::CycleBudget);
+    let mut uart0 = Vec::new();
+    // Device time, read from the machine, at the first look that found each countdown line
+    // printed.
+    let mut seen_at: Vec<u64> = Vec::new();
+    let mut restarting = false;
+    while machine.cycles().get() < BUDGET && !restarting {
+        let stop = machine.run_until(Cycles(machine.cycles().get() + LOOK));
+        assert!(
+            matches!(stop, StopReason::CycleBudget | StopReason::Asleep),
+            "{stop:?}"
+        );
+        let fresh = machine.take_console(ConsoleChannel::Uart0);
+        if fresh.is_empty() {
+            continue;
+        }
+        uart0.extend(fresh);
+        let console = String::from_utf8_lossy(&uart0);
+        if console.matches("seconds...\r\n").count() > seen_at.len() {
+            seen_at.push(machine.device_time_nanos());
+        }
+        restarting = console.contains("Restarting now.");
+    }
+    assert_eq!(seen_at.len(), 11, "{}", String::from_utf8_lossy(&uart0));
+    // `vTaskDelay(1000 / portTICK_PERIOD_MS)` between two lines: one second, less the part of a
+    // tick already spent when the first delay starts. The tolerance is that one tick, plus the
+    // resolution of the reading.
+    for pair in seen_at.windows(2) {
+        let gap = pair[1] - pair[0];
+        assert!(
+            gap.abs_diff(SECOND_NS) <= TICK_NS + LOOK_NS,
+            "two countdown lines are {gap} ns apart: {seen_at:?}"
+        );
+    }
     assert!(
-        diagnostics.iter().any(|d| matches!(
-            d,
-            Diagnostic::UnmodeledMmio {
-                register: Some(("TIMG0", "RTCCALICFG")),
-                ..
-            }
-        )),
-        "{diagnostics:#?}"
+        machine.device_time_nanos() > 11 * SECOND_NS,
+        "{} ns",
+        machine.device_time_nanos()
     );
     assert!(
-        machine
-            .most_repeated_diagnostics(3)
-            .iter()
-            .any(|(d, count)| *count > 1
-                && matches!(
-                    d,
-                    Diagnostic::UnmodeledMmio {
-                        register: Some(("TIMG0", "RTCCALICFG")),
-                        ..
-                    }
-                )),
-        "the end-of-run summary must show TIMG0.RTCCALICFG as what the firmware keeps polling"
+        machine.cycles().get() < 60_000_000,
+        "the CPU executed {} cycles: it did not sleep",
+        machine.cycles().get()
     );
 }
 
 /// Spec §5.1: "Reset → ROM → bootloader → table de partitions → `otadata` → application". The
 /// reference image has a factory application; the target device has none. Behind the stock X4
 /// table the unmodified bootloader finds a blank `otadata`, falls back to OTA slot 0 and records
-/// that choice: the mask ROM's sector-erase and page-program functions run. The flash is read-only
-/// in this plan, so both are refused, each once and by its own opcode, and nothing else is
-/// reported before the application reaches the RTC calibration.
+/// that choice: the mask ROM's sector-erase and page-program functions run. The flash is still
+/// read-only (plan SP1b-3b makes it writable), so both are refused, each once and by its own
+/// opcode, and nothing else is reported before the application says hello.
 #[test]
 #[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
 fn the_stock_x4_ota_layout_boots_and_only_the_otadata_update_is_refused() {
     let mut image = reference_flash();
     image[PARTITION_TABLE_OFFSET..PARTITION_TABLE_OFFSET + X4_STOCK_TABLE.len()]
         .copy_from_slice(X4_STOCK_TABLE);
-    let is_rtc_calibration = |d: &Diagnostic| {
-        matches!(
-            d,
-            Diagnostic::UnmodeledMmio {
-                register: Some(("TIMG0", "RTCCALICFG")),
-                ..
-            }
-        )
-    };
-    let boot = run_image(image, SocConfig::default(), |_, diagnostics| {
-        diagnostics.iter().any(is_rtc_calibration)
+    let boot = run_image(image, SocConfig::default(), |console, _| {
+        console.contains("Hello world!\n")
     });
     assert!(
         boot.console.starts_with(ROM_LINES),
@@ -317,6 +412,7 @@ fn the_stock_x4_ota_layout_boots_and_only_the_otadata_update_is_refused() {
         "I (#) boot: Loaded app from partition at offset 0x10000",
         "I (#) boot: Set actual ota_seq=1 in otadata[0]",
         FIRST_APPLICATION_LINE,
+        "Hello world!",
     ] {
         let at = lines[from..]
             .iter()
@@ -324,64 +420,53 @@ fn the_stock_x4_ota_layout_boots_and_only_the_otadata_update_is_refused() {
             .unwrap_or_else(|| panic!("`{expected}` is missing, or out of order:\n{lines:#?}"));
         from += at + 1;
     }
-    let reached = boot
+    let reported: Vec<(Unsupported, bool)> = boot
         .diagnostics
-        .iter()
-        .position(is_rtc_calibration)
-        .unwrap_or_else(|| {
-            panic!(
-                "the application never reached TIMG0.RTCCALICFG: {:#?}\nconsole:\n{}",
-                boot.diagnostics, boot.console
-            )
-        });
-    let before: Vec<(Unsupported, bool)> = boot.diagnostics[..reached]
         .iter()
         .map(|diagnostic| match diagnostic {
             Diagnostic::Unsupported { what, pc } => (*what, MASK_ROM.contains(pc)),
-            other => panic!("unexpected diagnostic before the RTC calibration: {other}"),
+            other => panic!("unexpected diagnostic: {other}"),
         })
         .collect();
     assert_eq!(
-        before,
+        reported,
         [
             (Unsupported::FlashCommand { opcode: 0x20 }, true),
             (Unsupported::FlashCommand { opcode: 0x02 }, true),
         ],
         "exactly the sector erase and the page program of the otadata update, each run by the mask ROM"
     );
-    assert_eq!(
-        boot.stop,
-        StopReason::CycleBudget,
-        "nothing stops a lenient run"
+    assert!(
+        matches!(boot.stop, StopReason::CycleBudget | StopReason::Asleep),
+        "nothing stops a lenient run: {:?}",
+        boot.stop
     );
 }
 
+/// Milestone point 6: two runs of a whole life cycle, the reset included, are the same run.
 #[test]
 #[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
-fn two_boots_of_the_reference_image_are_identical_down_to_the_trace_hash() {
+fn two_life_cycles_are_identical_down_to_the_trace_hash_across_the_reset() {
     let config = SocConfig {
         trace_hash: true,
         ..SocConfig::default()
     };
-    let boot = || {
-        run(config, |console| {
-            console.contains("cpu_start: Unicore app\n")
-        })
-    };
-    let (first, second) = (boot(), boot());
-    // Two runs that both exhausted the budget before the application would compare equal too.
-    assert!(
-        first
-            .console
-            .lines()
-            .map(mask_timestamp)
-            .any(|line| line == FIRST_APPLICATION_LINE),
-        "the boot must reach the application:\n{}",
-        first.console
-    );
+    let life = || run(config, |console| application_starts(console) == 2);
+    let (first, second) = (life(), life());
+    // Two runs that both exhausted the budget before the second boot would compare equal too,
+    // and so would two runs a watchdog reset: the reset crossed is the one `esp_restart` asks
+    // for, and the second boot went as far as the first line of the application.
+    assert_eq!(application_starts(&first.console), 2, "{}", first.console);
+    assert_eq!(first.resets, 1);
+    let (_, second_boot) = first
+        .console
+        .split_once("rst:0xc (RTC_SW_CPU_RST)")
+        .unwrap_or_else(|| panic!("no software CPU reset in:\n{}", first.console));
+    assert_eq!(application_starts(second_boot), 1, "{}", first.console);
     assert!(first.trace_hash.is_some());
     assert_eq!(
-        first.trace_hash, second.trace_hash,
+        (first.trace_hash, first.cycles, first.device_time_nanos),
+        (second.trace_hash, second.cycles, second.device_time_nanos),
         "spec §8: same session, same trace hash"
     );
     assert_eq!(first.console, second.console);
```

```diff
--- a/crates/emu-cli/tests/run_cli.rs
+++ b/crates/emu-cli/tests/run_cli.rs
@@ -483,46 +483,52 @@ fn x4_without_a_flash_image_loops_on_the_invalid_header_of_an_erased_chip() {
 /// The milestone through the real command: ROM, bootloader, first application line, strict stop.
 #[test]
 #[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
-fn x4_boots_the_reference_firmware_to_its_first_application_line() {
+fn x4_boots_the_reference_firmware_through_its_restart_into_a_second_boot() {
     let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
     let flash = std::env::var("INKEMU_REF_FLASH").expect("set INKEMU_REF_FLASH");
+    // One life of the application is 28 million cycles and 11 s of device time; 40 million reach
+    // the second boot.
     let output = inkemu()
         .args([
             "run",
             "--device",
             "x4",
             "--strict-mmio",
+            "--max-cycles",
+            "40000000",
             "--rom-dir",
             &rom_dir,
             "--flash",
             &flash,
         ])
         .assert()
-        .code(3)
+        .code(0)
         .get_output()
         .clone();
     let console = String::from_utf8_lossy(&output.stdout).replace('\r', "");
-    assert!(console.contains("entry 0x403cbf1a\n"), "{console}");
-    assert!(
-        console.contains(") boot: ESP-IDF v5.5.2 2nd stage bootloader\n"),
-        "{console}"
-    );
-    assert!(
-        console.contains(") boot.esp32c3: SPI Mode       : DIO\n"),
-        "{console}"
-    );
-    assert!(
-        console.contains(") boot: Disabling RNG early entropy source...\n"),
-        "{console}"
-    );
-    assert!(console.contains(") cpu_start: Unicore app\n"), "{console}");
+    let mut from = 0;
+    for expected in [
+        "entry 0x403cbf1a\n",
+        ") boot: ESP-IDF v5.5.2 2nd stage bootloader\n",
+        ") cpu_start: Unicore app\n",
+        "Hello world!\n",
+        "Restarting in 10 seconds...\n",
+        "Restarting in 0 seconds...\n",
+        "Restarting now.\n",
+        "rst:0xc (RTC_SW_CPU_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)\n",
+        "Saved PC:0x40048b92\n",
+        ") boot: ESP-IDF v5.5.2 2nd stage bootloader\n",
+        ") cpu_start: Unicore app\n",
+    ] {
+        let at = console[from..]
+            .find(expected)
+            .unwrap_or_else(|| panic!("`{expected}` is missing, or out of order:\n{console}"));
+        from += at + expected.len();
+    }
     let log = String::from_utf8_lossy(&output.stderr).into_owned();
+    assert!(log.contains("cycle limit reached"), "{log}");
     assert!(
-        log.contains("stopped by --strict-mmio") && log.contains("TIMG0.RTCCALICFG"),
-        "{log}"
-    );
-    assert!(
-        !log.contains("warning:"),
-        "nothing was reported before the stop: {log}"
+        !log.contains("warning:") && !log.contains("stopped by --strict-mmio"),
+        "nothing was reported in a whole life cycle: {log}"
     );
 }
```

- [ ] **Step 3: Run them and see them fail**

Run: `cargo test -p soc-esp32c3 --lib > /tmp/t13.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, compile errors: no variant `Module::Timg0`, no variant `Module::Timg1` (the test of `periph/system.rs`). The test module of `timg.rs` is not compiled yet: Step 6 declares the module, and it then needs `Timg`, `Group` and `Event::Timg`.

Run (with `INKEMU_ROM_ELF_DIR` and `INKEMU_REF_FLASH` set, the device tests set aside): `cargo test --release -p soc-esp32c3 --test reference_boot -- --ignored > /tmp/t13r.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, five tests failing on the application that spins on `TIMG0.RTCCALICFG`: `a_strict_run_crosses_the_restart_without_anything_unmodelled` stops on that register, `the_whole_life_cycle_prints_the_golden_log_and_boots_a_second_time` finds its diagnostics, `the_countdown_lines_are_one_device_second_apart_and_the_cpu_sleeps_between_them` and `the_stock_x4_ota_layout_boots_and_only_the_otadata_update_is_refused` never see `Hello world!`. The run does not last to its budget: the RTC watchdog the bootloader left armed (9 s, Task 10) resets the spinning application, and the console shows `rst:0x10 (RTCWDT_RTC_RST)`. That reset gives `two_life_cycles_are_identical_down_to_the_trace_hash_across_the_reset` a second boot and one reset, and it fails all the same, with `no software CPU reset in:`: it asks for the reset of `esp_restart`. Only `rom_bootloader_and_application_print_the_golden_log` passes.

- [ ] **Step 4: The events**

```diff
--- a/crates/soc-esp32c3/src/ctx.rs
+++ b/crates/soc-esp32c3/src/ctx.rs
@@ -34,12 +34,21 @@ impl Source {
     }
 }
 
+/// The two timer groups.
+#[derive(Clone, Copy, Debug, PartialEq, Eq)]
+pub enum Group {
+    Timg0,
+    Timg1,
+}
+
 /// A hardware module that `SYSTEM.PERIP_RST_EN0` or `PERIP_RST_EN1` resets.
 #[derive(Clone, Copy, Debug, PartialEq, Eq)]
 pub enum Module {
     Spi01,
     Uart0,
     Uart1,
+    Timg0,
+    Timg1,
     Systimer,
     Sha,
 }
@@ -60,8 +69,12 @@ pub enum Event {
     Systimer,
     /// Wake-up of UART 0 or 1: its transmitter may have gone idle.
     Uart(u8),
+    /// Wake-up of a timer group: its timer or its watchdog may have reached a deadline.
+    Timg(Group),
     /// Wake-up of `RTC_CNTL`: the RTC watchdog may have reached a stage timeout.
     RtcWatchdog,
+    /// To every device that counts APB clock cycles: the clock selection changed.
+    ClockChanged,
     /// To the named module: `SYSTEM.PERIP_RST_EN0/1` put it in reset.
     ModuleReset(Module),
 }
```

- [ ] **Step 5: The timer group**

`crates/soc-esp32c3/src/periph/timg.rs`, above its test module:

```rust
//! `TIMG0` and `TIMG1`: each timer group has a general-purpose timer T0, a main watchdog (MWDT)
//! and the registers of the slow-clock calibration. Sources: TRM chapters 11 and 12, ESP-IDF
//! `timer_group_reg.h`, `timer_ll.h`, `mwdt_ll.h`, `rtc_time.c`; report TW §B and §D.
use super::counter::{Counter, Rate};
use super::{TIMG0_BASE, TIMG1_BASE};
use super::{half_of, replace_half};
use crate::clock::XTAL_PERIOD;
use crate::ctx::{Ctx, Deadline, Event, Group, Source};
use crate::reset::ResetCause;
use emu_core::{Cycles, Peripheral};
use svd_regs::esp32c3::addr;

/// The two groups have the same registers at the same offsets: those of `TIMG0` name them.
const fn reg(name: &str) -> u32 {
    addr("TIMG0", name) - TIMG0_BASE
}
const _: () = assert!(addr("TIMG1", "REGCLK") - TIMG1_BASE == reg("REGCLK"));

const T0CONFIG: u32 = reg("T0CONFIG");
const T0LO: u32 = reg("T0LO");
const T0HI: u32 = reg("T0HI");
const T0UPDATE: u32 = reg("T0UPDATE");
const T0ALARMLO: u32 = reg("T0ALARMLO");
const T0ALARMHI: u32 = reg("T0ALARMHI");
const T0LOADLO: u32 = reg("T0LOADLO");
const T0LOADHI: u32 = reg("T0LOADHI");
const T0LOAD: u32 = reg("T0LOAD");
const WDTCONFIG0: u32 = reg("WDTCONFIG0");
const WDTCONFIG1: u32 = reg("WDTCONFIG1");
const WDTCONFIG2: u32 = reg("WDTCONFIG2");
const WDTCONFIG5: u32 = reg("WDTCONFIG5");
const WDTFEED: u32 = reg("WDTFEED");
const WDTWPROTECT: u32 = reg("WDTWPROTECT");
const RTCCALICFG: u32 = reg("RTCCALICFG");
const RTCCALICFG1: u32 = reg("RTCCALICFG1");
const INT_ENA: u32 = reg("INT_ENA_TIMERS");
const INT_RAW: u32 = reg("INT_RAW_TIMERS");
const INT_ST: u32 = reg("INT_ST_TIMERS");
const INT_CLR: u32 = reg("INT_CLR_TIMERS");
const RTCCALICFG2: u32 = reg("RTCCALICFG2");
const NTIMG_DATE: u32 = reg("NTIMG_DATE");
const REGCLK: u32 = reg("REGCLK");

// Reset values: `timer_group_reg.h`, tabulated in TW §B.1.
const T0CONFIG_RESET: u32 = 0x6000_2000;
/// `FLASHBOOT_MOD_EN` set, both reset lengths 1: the flash-boot protection is armed.
const WDTCONFIG0_RESET: u32 = 0x0004_C000;
const WDTCONFIG1_RESET: u32 = 0x0001_0000;
/// Stage timeouts 0 to 3: 26 000 000, 134 217 727, 1 048 575, 1 048 575 counts.
const STAGE_TIMEOUT_RESET: [u32; STAGES] = [26_000_000, 0x07FF_FFFF, 0x000F_FFFF, 0x000F_FFFF];
const RTCCALICFG_RESET: u32 = 0x0001_3000;
const RTCCALICFG2_RESET: u32 = 0xFFFF_FF98;
const NTIMG_DATE_VALUE: u32 = 0x0200_6191;
const REGCLK_RESET: u32 = 0x6000_0000;

/// `T0CONFIG`: bit 31 `EN` (`timer_ll_enable_counter`, `IDF:hal/esp32c3/include/hal/timer_ll.h:175`),
/// bit 30 `INCREASE` (`timer_ll_set_count_direction`, line 161), bit 29 `AUTORELOAD`
/// (`timer_ll_enable_auto_reload`, line 149), bits [28:13] `DIVIDER` and bit 12 `DIVCNT_RST`, a
/// write-1 trigger (`timer_ll_set_clock_prescale`, lines 130-137), bit 10 `ALARM_EN`
/// (`timer_ll_enable_alarm`, line 118), bit 9 `USE_XTAL` (`timer_ll_set_clock_source`, line 79).
const T0_EN: u32 = 1 << 31;
const T0_INCREASE: u32 = 1 << 30;
const T0_AUTORELOAD: u32 = 1 << 29;
const T0_DIVIDER_SHIFT: u32 = 13;
const T0_DIVIDER_MASK: u32 = 0xFFFF;
const T0_ALARM_EN: u32 = 1 << 10;
const T0_USE_XTAL: u32 = 1 << 9;
const T0CONFIG_STORED: u32 = 0xFFFF_E600;
/// The counter, its alarm and its reload value are 54 bits wide: 22 bits in the `HI` registers.
const T0_MASK: u64 = (1 << 54) - 1;
const T0_HI_MASK: u32 = 0x003F_FFFF;
/// "An alarm value already passed (within 2^53) fires immediately" (TRM §11.2; TW §B.2).
const T0_PASSED_WINDOW: u64 = 1 << 53;

/// `WDTCONFIG0`: bit 31 `EN` (`mwdt_ll_enable`, `IDF:hal/esp32c3/include/hal/mwdt_ll.h:65`), two
/// bits per stage from [30:29] for stage 0 to [24:23] for stage 3 (`mwdt_ll_config_stage`, line
/// 102), bit 22 `CONF_UPDATE_EN`, a write-1 trigger that reads 0 (same function), bit 21 `USE_XTAL`
/// (`mwdt_ll_set_clock_source`, line 277), bit 14 `FLASHBOOT_MOD_EN` (`mwdt_ll_set_flashboot_en`,
/// line 194). The reset lengths and the two CPU reset enables are stored.
const WDT_EN: u32 = 1 << 31;
const WDT_STAGE0_SHIFT: u32 = 29;
const WDT_STAGE_BITS: u32 = 2;
const WDT_STAGE_MASK: u32 = 0b11;
const WDT_CONF_UPDATE_EN: u32 = 1 << 22;
const WDT_USE_XTAL: u32 = 1 << 21;
const WDT_FLASHBOOT_MOD_EN: u32 = 1 << 14;
const WDTCONFIG0_STORED: u32 = 0xFFBF_F000;
/// `WDTCONFIG1` bits [31:16] `CLK_PRESCALE` (`mwdt_ll_set_prescaler`, line 207); bit 0 is a
/// write-1 trigger that restarts the prescaler.
const WDT_PRESCALE_SHIFT: u32 = 16;
const WDTCONFIG1_STORED: u32 = 0xFFFF_0000;
/// The key that unlocks every watchdog register (`mwdt_ll_write_protect_disable`, line 245); it
/// is also the reset value of `WDTWPROTECT`: the watchdog is unlocked out of reset (TW §B.3).
const WDT_KEY: u32 = 0x50D8_3AA1;
const STAGES: usize = 4;
/// Stage actions (`IDF:hal/include/hal/wdt_types.h`; TW §D.5).
const ACTION_INTERRUPT: u32 = 1;
const ACTION_RESET_CPU: u32 = 2;
const ACTION_RESET_SYSTEM: u32 = 3;

/// Bit 0 of the interrupt registers is T0 (`timer_ll_enable_intr`, `timer_ll.h:272`), bit 1 the
/// watchdog (`mwdt_ll_set_intr_enable`, `mwdt_ll.h:266`).
const INT_T0: u32 = 1 << 0;
const INT_WDT: u32 = 1 << 1;
const INT_MASK: u32 = INT_T0 | INT_WDT;

/// `REGCLK`: bit 31 `CLK_EN` (`timer_ll_enable_register_clock_always_on`, `timer_ll.h:313`), bit 30
/// `TIMER_CLK_IS_ACTIVE`, bit 29 `WDT_CLK_IS_ACTIVE` (`mwdt_ll_enable_clock`, `mwdt_ll.h:299`): the
/// last two gate the clock of T0 and of the watchdog (TW §B.2, §B.3).
const REGCLK_STORED: u32 = 0xE000_0000;
const TIMER_CLK_IS_ACTIVE: u32 = 1 << 30;
const WDT_CLK_IS_ACTIVE: u32 = 1 << 29;

/// `RTCCALICFG`: bit 31 `START`, bits [30:16] `MAX`, bit 15 `RDY` (read-only), bits [14:13]
/// `CLK_SEL`, bit 12 `START_CYCLING`. `rtc_clk_cal_internal` writes each of them
/// (`IDF:esp_hw_support/port/esp32c3/rtc_time.c:36-128`; TW §B.4).
const CALI_START: u32 = 1 << 31;
const CALI_MAX_SHIFT: u32 = 16;
const CALI_MAX_MASK: u32 = 0x7FFF;
const CALI_RDY: u32 = 1 << 15;
const CALI_CLK_SEL_SHIFT: u32 = 13;
const CALI_CLK_SEL_MASK: u32 = 0b11;
const CALI_START_CYCLING: u32 = 1 << 12;
const RTCCALICFG_STORED: u32 = 0xFFFF_7000;
/// `RTCCALICFG1` bits [31:7] `VALUE`: the crystal cycles counted.
const CALI_VALUE_SHIFT: u32 = 7;
/// `RTCCALICFG2`: bits [31:7] `TIMEOUT_THRES`, bits [6:3] `TIMEOUT_RST_CNT`, bit 0 `TIMEOUT`
/// (read-only).
const CALI_THRES_SHIFT: u32 = 7;
const CALI_TIMEOUT: u32 = 1;
const RTCCALICFG2_STORED: u32 = 0xFFFF_FFF8;

/// The clock a calibration measures, as crystal cycles per slow-clock cycle (a fraction): the
/// nominal frequencies of `IDF:soc/esp32c3/include/soc/clk_tree_defs.h:39-42` (TW §B.4).
///
/// ASSUMPTION: the RC oscillators run at their nominal frequency. A real chip deviates from it, by
/// an amount no source gives (TW §I question 2); the calibration value of a real unit
/// (`RTC_CNTL.STORE1` after boot) settles it, and belongs in the device profile then.
const fn crystal_cycles_per_slow_cycle(clk_sel: u32) -> Option<(u64, u64)> {
    match clk_sel {
        // RC_SLOW, 136 kHz: 40 000 000 / 136 000 = 5 000 / 17.
        0 => Some((5_000, 17)),
        // RC_FAST divided by 256, 17.5 MHz / 256: 40 000 000 * 256 / 17 500 000 = 4 096 / 7.
        1 => Some((4_096, 7)),
        // The 32 kHz crystal: none is fitted (see `Calibration::start`).
        _ => None,
    }
}

/// One measurement of the slow clock against the crystal.
#[derive(Clone, Copy, Debug, Default)]
struct Measurement {
    /// Crystal cycles counted during `MAX` slow-clock cycles.
    value: u32,
    /// Device time from which `RDY` reads 1, if the measurement completes.
    ready_at: Option<Cycles>,
    /// Device time from which `TIMEOUT` reads 1, if it gives up.
    timeout_at: Option<Cycles>,
}

#[derive(Clone, Copy, Debug)]
struct Calibration {
    config: u32,
    config2: u32,
    measurement: Measurement,
}

impl Calibration {
    const fn new() -> Calibration {
        Calibration {
            config: RTCCALICFG_RESET,
            config2: RTCCALICFG2_RESET,
            measurement: Measurement {
                value: 0,
                ready_at: None,
                timeout_at: None,
            },
        }
    }

    const fn max(&self) -> u64 {
        ((self.config >> CALI_MAX_SHIFT) & CALI_MAX_MASK) as u64
    }

    const fn threshold(&self) -> u64 {
        (self.config2 >> CALI_THRES_SHIFT) as u64
    }

    /// Crystal cycles the selected clock takes for `MAX` of its cycles, when the clock exists.
    fn crystal_cycles(&self) -> Option<u64> {
        let (numerator, denominator) =
            crystal_cycles_per_slow_cycle((self.config >> CALI_CLK_SEL_SHIFT) & CALI_CLK_SEL_MASK)?;
        Some(self.max() * numerator / denominator)
    }

    /// `START` went from 0 to 1: a one-shot measurement begins (TW §B.4 step 4).
    ///
    /// It counts crystal cycles during `MAX` cycles of the selected clock: the result is known
    /// from the two frequencies, and it is ready when those `MAX` cycles have gone by. A count
    /// that would pass `TIMEOUT_THRES` first ends in `TIMEOUT` instead, `RDY` staying clear.
    ///
    /// ASSUMPTION: `TIMEOUT_THRES` is in crystal cycles, and no 32 kHz crystal is fitted, so a
    /// measurement of that clock always ends in `TIMEOUT` (ESP-IDF then falls back to the RC
    /// oscillator and says so: `IDF:esp_system/port/soc/esp32c3/clk.c:140-199`). Neither is
    /// documented (TW §I question 7); the Xteink boards carry no such crystal as far as the
    /// hardware report knows. A board that has one needs a profile entry.
    fn start(&mut self, now: Cycles) {
        let threshold = self.threshold();
        self.measurement = match self.crystal_cycles() {
            Some(cycles) if cycles <= threshold => Measurement {
                value: u32::try_from(cycles).unwrap_or(u32::MAX),
                ready_at: Some(now + cycles.saturating_mul(XTAL_PERIOD)),
                timeout_at: None,
            },
            _ => Measurement {
                value: 0,
                ready_at: None,
                timeout_at: Some(now + threshold.saturating_mul(XTAL_PERIOD)),
            },
        };
    }

    fn read_config(&self, now: Cycles) -> u32 {
        let ready = self.measurement.ready_at.is_some_and(|at| at <= now);
        self.config | if ready { CALI_RDY } else { 0 }
    }

    fn read_config2(&self, now: Cycles) -> u32 {
        let timed_out = self.measurement.timeout_at.is_some_and(|at| at <= now);
        self.config2 | if timed_out { CALI_TIMEOUT } else { 0 }
    }

    fn write_config(&mut self, value: u32, now: Cycles) {
        let started = value & CALI_START != 0 && self.config & CALI_START == 0;
        self.config = value & RTCCALICFG_STORED;
        if started {
            self.start(now);
        }
    }

    /// ASSUMPTION: while the cycling mode is on (it is at reset), lowering `TIMEOUT_THRES` below
    /// what the running measurement needs makes `TIMEOUT` read 1 at once. `rtc_clk_cal_internal`
    /// does exactly that to get out of the cycling mode, with a threshold of 1, and then waits
    /// for `RDY` or `TIMEOUT` without a time limit (`rtc_time.c`, comment "accelerate the
    /// generation of timeout"; TW §B.4 step 1, §I question 7). What the cycling mode itself
    /// measures is not modelled: `RTCCALICFG1.CYCLING_DATA_VLD` reads 0.
    fn write_config2(&mut self, value: u32, now: Cycles) {
        self.config2 = value & RTCCALICFG2_STORED;
        let cycling = self.config & CALI_START_CYCLING != 0;
        if cycling
            && self
                .crystal_cycles()
                .is_none_or(|cycles| cycles > self.threshold())
        {
            self.measurement.ready_at = None;
            self.measurement.timeout_at = Some(now);
        }
    }
}

#[derive(Clone, Debug)]
pub struct Timg {
    group: Group,
    /// Whether the chip boots from flash: the strapping the flash-boot protection depends on.
    flash_boot: bool,
    t0_config: u32,
    t0: Counter,
    t0_latch: u64,
    t0_alarm: u64,
    t0_load: u64,
    wdt_config0: u32,
    wdt_config1: u32,
    wdt_timeout: [u32; STAGES],
    wdt_key: u32,
    /// Counts since the last feed or the start of the current stage.
    wdt: Counter,
    wdt_stage: usize,
    /// Whether `EN` or the flash-boot protection had the watchdog on at the last look.
    wdt_enabled: bool,
    calibration: Calibration,
    int_ena: u32,
    int_raw: u32,
    regclk: u32,
    wake: Deadline,
}

impl Timg {
    /// The group after a power-on, a core reset or a peripheral reset. `flash_boot` says whether
    /// the chip was strapped to boot from flash.
    ///
    /// The owner calls [`Timg::wake`] once afterwards: the reset values arm the flash-boot
    /// protection of group 0, which needs its first deadline.
    #[must_use]
    pub fn new(group: Group, flash_boot: bool, ctx: &Ctx) -> Timg {
        let now = ctx.now();
        let mut timg = Timg {
            group,
            flash_boot,
            t0_config: T0CONFIG_RESET,
            t0: Counter::new(0, now, Rate::every(1), false),
            t0_latch: 0,
            t0_alarm: 0,
            t0_load: 0,
            wdt_config0: WDTCONFIG0_RESET,
            wdt_config1: WDTCONFIG1_RESET,
            wdt_timeout: STAGE_TIMEOUT_RESET,
            wdt_key: WDT_KEY,
            wdt: Counter::new(0, now, Rate::every(1), false),
            wdt_stage: 0,
            wdt_enabled: false,
            calibration: Calibration::new(),
            int_ena: 0,
            int_raw: 0,
            regclk: REGCLK_RESET,
            wake: Deadline::default(),
        };
        timg.retime(ctx);
        timg
    }

    const fn event(&self) -> Event {
        Event::Timg(self.group)
    }

    const fn sources(&self) -> (Source, Source) {
        match self.group {
            Group::Timg0 => (Source::of("TG_T0_INT_MAP"), Source::of("TG_WDT_INT_MAP")),
            Group::Timg1 => (Source::of("TG1_T0_INT_MAP"), Source::of("TG1_WDT_INT_MAP")),
        }
    }

    /// T0 counts `APB_CLK` or the crystal, divided by `DIVIDER`: 2 to 65 535 divide by that value,
    /// 0 divides by 65 536 and 1 by 2 (TRM §11.2; TW §B.2).
    fn t0_rate(&self, ctx: &Ctx) -> Rate {
        let source = if self.t0_config & T0_USE_XTAL == 0 {
            ctx.clock().apb_period
        } else {
            XTAL_PERIOD
        };
        let divider = match (self.t0_config >> T0_DIVIDER_SHIFT) & T0_DIVIDER_MASK {
            0 => 65_536,
            1 => 2,
            divider => u64::from(divider),
        };
        Rate::every(source).divided_by(divider)
    }

    /// The watchdog counts `APB_CLK` or the crystal, divided by `CLK_PRESCALE` (TRM §12.2.2.1).
    ///
    /// ASSUMPTION: a prescaler of 0 divides by 65 536, as the 16-bit divider of T0 does. No source
    /// says; ESP-IDF never writes 0. A probe on a real chip settles it.
    fn wdt_rate(&self, ctx: &Ctx) -> Rate {
        let source = if self.wdt_config0 & WDT_USE_XTAL == 0 {
            ctx.clock().apb_period
        } else {
            XTAL_PERIOD
        };
        let prescale = match self.wdt_config1 >> WDT_PRESCALE_SHIFT {
            0 => 65_536,
            prescale => u64::from(prescale),
        };
        Rate::every(source).divided_by(prescale)
    }

    /// Whether the flash-boot protection runs the watchdog: only that of group 0 has one, and only
    /// when the chip boots from flash (TRM §12.2.2.4; TW §D.4).
    ///
    /// ASSUMPTION: the protection is off when the chip is strapped for another boot mode. The ROM
    /// waits in its download modes for ever and has no code that could stop a watchdog (TW §D.4,
    /// §I question 4): the condition is inferred, not documented.
    fn flash_boot_protection(&self) -> bool {
        self.group == Group::Timg0
            && self.flash_boot
            && self.wdt_config0 & WDT_FLASHBOOT_MOD_EN != 0
    }

    fn wdt_enabled(&self) -> bool {
        self.wdt_config0 & WDT_EN != 0 || self.flash_boot_protection()
    }

    fn t0_runs(&self) -> bool {
        self.regclk & TIMER_CLK_IS_ACTIVE != 0 && self.t0_config & T0_EN != 0
    }

    /// The action of a stage: the flash-boot protection makes stage 0 a core reset "whatever EN
    /// and STG0 contain" (TW §B.3).
    fn stage_action(&self, stage: usize) -> u32 {
        if stage == 0 && self.flash_boot_protection() {
            return ACTION_RESET_SYSTEM;
        }
        // INVARIANT: `stage` is below `STAGES` (4), so the shift stays inside the register.
        let shift = WDT_STAGE0_SHIFT - WDT_STAGE_BITS * u32::try_from(stage).unwrap_or(0);
        (self.wdt_config0 >> shift) & WDT_STAGE_MASK
    }

    /// Brings both counters to the clocks and the enables the registers now describe. They keep
    /// their value: only what happens from now on changes.
    fn retime(&mut self, ctx: &Ctx) {
        let now = ctx.now();
        self.t0.retime(self.t0_rate(ctx), now);
        self.t0.run(self.t0_runs(), now);
        self.wdt.retime(self.wdt_rate(ctx), now);
        let enabled = self.wdt_enabled();
        if enabled && !self.wdt_enabled {
            // ASSUMPTION: a watchdog that is switched on counts its first stage from 0. ESP-IDF
            // feeds before it enables (`wdt_hal_enable`, `IDF:hal/wdt_hal_iram.c:146-155`), so the
            // firmware does not tell (TW §I question 5).
            self.wdt.set(0, now);
            self.wdt_stage = 0;
        }
        self.wdt_enabled = enabled;
        // The clock gate freezes the counter without clearing it.
        self.wdt
            .run(enabled && self.regclk & WDT_CLK_IS_ACTIVE != 0, now);
    }

    fn t0_count(&self, now: Cycles) -> u64 {
        let counted = self.t0.value(now);
        if self.t0_config & T0_INCREASE == 0 {
            // Counting down: the counter holds the value it was set to, minus what went by.
            // `set_t0` stores the negation, so that the same up-counter serves both directions.
            counted.wrapping_neg() & T0_MASK
        } else {
            counted & T0_MASK
        }
    }

    fn set_t0(&mut self, value: u64, instant: Cycles) {
        let stored = if self.t0_config & T0_INCREASE == 0 {
            value.wrapping_neg()
        } else {
            value
        };
        self.t0.set(stored & T0_MASK, instant);
    }

    /// Counts until T0 reaches its alarm value in its direction, 0 when it is there or past it.
    fn t0_counts_to_alarm(&self, now: Cycles) -> u64 {
        let count = self.t0_count(now);
        let ahead = if self.t0_config & T0_INCREASE == 0 {
            count.wrapping_sub(self.t0_alarm)
        } else {
            self.t0_alarm.wrapping_sub(count)
        } & T0_MASK;
        if ahead >= T0_PASSED_WINDOW { 0 } else { ahead }
    }

    /// Raises what is due at the present time, then asks to be woken for the next deadline.
    fn evaluate(&mut self, ctx: &mut Ctx) {
        let now = ctx.now();
        self.wake.called(now);
        let mut earliest: Option<Cycles> = None;
        let mut sooner = |at: Option<Cycles>| {
            if let Some(at) = at {
                earliest = Some(earliest.map_or(at, |e: Cycles| e.min(at)));
            }
        };

        if self.t0_config & T0_ALARM_EN != 0 && self.t0.is_running() {
            let ahead = self.t0_counts_to_alarm(now);
            if ahead == 0 {
                // At the alarm the interrupt is raised, `ALARM_EN` is cleared by hardware and,
                // with `AUTORELOAD`, the counter restarts from the reload value (TW §B.2).
                self.int_raw |= INT_T0;
                self.t0_config &= !T0_ALARM_EN;
                if self.t0_config & T0_AUTORELOAD != 0 {
                    self.set_t0(self.t0_load, now);
                }
            } else {
                sooner(self.t0.instant_in(ahead, now));
            }
        }

        // A long sleep can cover several stages: each one starts when the one before it ends.
        while self.wdt.is_running() {
            let timeout = u64::from(self.wdt_timeout[self.wdt_stage]);
            let count = self.wdt.value(now);
            // ASSUMPTION: a stage expires when the counter is at or past its timeout. The hardware
            // may compare for equality, which would let a timeout lowered below the present count
            // never expire until the next feed; QEMU's model says so, no document does (TW §G row
            // 7, §I question 5). At-or-past makes such a watchdog fire, which is visible.
            if count < timeout {
                sooner(self.wdt.instant_in(timeout - count, now));
                break;
            }
            let action = self.stage_action(self.wdt_stage);
            // The counter returns to 0 and the next stage becomes current; stages loop 0 to 3
            // (TRM §12.2.2.2). The counts beyond the timeout belong to the next stage.
            self.wdt.set(count - timeout, now);
            self.wdt_stage = (self.wdt_stage + 1) % STAGES;
            match (action, self.group) {
                (ACTION_INTERRUPT, _) => self.int_raw |= INT_WDT,
                (ACTION_RESET_CPU, Group::Timg0) => ctx.request_reset(ResetCause::Mwdt0Cpu),
                (ACTION_RESET_CPU, Group::Timg1) => ctx.request_reset(ResetCause::Mwdt1Cpu),
                (ACTION_RESET_SYSTEM, Group::Timg0) => ctx.request_reset(ResetCause::Mwdt0Core),
                (ACTION_RESET_SYSTEM, Group::Timg1) => ctx.request_reset(ResetCause::Mwdt1Core),
                // ASSUMPTION: a stage whose action is "off" is waited for like the others and
                // does nothing when it ends (QEMU's reading; TW §B.3, §I question 5).
                _ => {}
            }
        }

        if let Some(at) = earliest {
            self.wake.arm(ctx, at, self.event());
        }
        self.drive(ctx);
    }

    /// Both interrupts are levels: `INT_RAW & INT_ENA` (TW §B.1).
    fn drive(&self, ctx: &mut Ctx) {
        let (t0, wdt) = self.sources();
        let status = self.int_raw & self.int_ena;
        ctx.irq(t0, status & INT_T0 != 0);
        ctx.irq(wdt, status & INT_WDT != 0);
    }

    /// The device time of a wake-up has come, or the group was just built.
    pub(crate) fn wake(&mut self, ctx: &mut Ctx) {
        self.retime(ctx);
        self.evaluate(ctx);
    }

    /// The APB clock changed: the counters keep their value and count on at the new rate.
    pub(crate) fn clock_changed(&mut self, ctx: &mut Ctx) {
        self.retime(ctx);
        self.evaluate(ctx);
    }

    /// A write to a watchdog register other than the key: dropped unless the key is in place,
    /// "feeding or changing its configuration" alike (TRM §12.2.2.3).
    fn write_watchdog(&mut self, ctx: &mut Ctx, offset: u32, value: u32) {
        if self.wdt_key != WDT_KEY {
            return;
        }
        let now = ctx.now();
        match offset {
            // ASSUMPTION: a configuration register acts as soon as it is written, without waiting
            // for `CONF_UPDATE_EN`. The TRM describes a synchronisation on that bit (§12.2; TW
            // §B.3), but `_timer_ll_reset_register` clears `FLASHBOOT_MOD_EN` right after a
            // peripheral reset without writing it, "to prevent the system from being reset
            // unexpectedly" (`IDF:hal/esp32c3/include/hal/timer_ll.h:50-66`): that code only works
            // if the bit acts at once. A register read-back on a real chip settles the others.
            WDTCONFIG0 => self.wdt_config0 = value & WDTCONFIG0_STORED & !WDT_CONF_UPDATE_EN,
            WDTCONFIG1 => self.wdt_config1 = value & WDTCONFIG1_STORED,
            WDTFEED => {
                // Any write feeds: the counter returns to 0 and stage 0 becomes current (TW §B.3).
                self.wdt.set(0, now);
                self.wdt_stage = 0;
            }
            _ => self.wdt_timeout[((offset - WDTCONFIG2) / 4) as usize] = value,
        }
        self.retime(ctx);
        self.evaluate(ctx);
    }
}

impl Peripheral<Ctx> for Timg {
    fn read(&mut self, ctx: &mut Ctx, offset: u32) -> Option<u32> {
        let now = ctx.now();
        Some(match offset {
            T0CONFIG => self.t0_config,
            T0LO => half_of(self.t0_latch, false),
            T0HI => half_of(self.t0_latch, true),
            // "Cleared by hardware when the latch is done" (TW §B.1): the latch is immediate.
            T0UPDATE | T0LOAD | WDTFEED | INT_CLR => 0,
            T0ALARMLO => half_of(self.t0_alarm, false),
            T0ALARMHI => half_of(self.t0_alarm, true),
            T0LOADLO => half_of(self.t0_load, false),
            T0LOADHI => half_of(self.t0_load, true),
            WDTCONFIG0 => self.wdt_config0,
            WDTCONFIG1 => self.wdt_config1,
            WDTWPROTECT => self.wdt_key,
            RTCCALICFG => self.calibration.read_config(now),
            RTCCALICFG1 => {
                if self
                    .calibration
                    .measurement
                    .ready_at
                    .is_some_and(|at| at <= now)
                {
                    self.calibration.measurement.value << CALI_VALUE_SHIFT
                } else {
                    0
                }
            }
            RTCCALICFG2 => self.calibration.read_config2(now),
            INT_ENA => self.int_ena,
            INT_RAW => self.int_raw,
            INT_ST => self.int_raw & self.int_ena,
            NTIMG_DATE => NTIMG_DATE_VALUE,
            REGCLK => self.regclk,
            offset if (WDTCONFIG2..=WDTCONFIG5).contains(&offset) => {
                self.wdt_timeout[((offset - WDTCONFIG2) / 4) as usize]
            }
            _ => return None,
        })
    }

    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        let now = ctx.now();
        match offset {
            T0CONFIG => {
                // The count is read in the old direction and put back in the new one.
                let count = self.t0_count(now);
                self.t0_config = value & T0CONFIG_STORED;
                self.set_t0(count, now);
                self.retime(ctx);
                self.evaluate(ctx);
            }
            // Any write latches the counter (`timer_ll_trigger_soft_capture`, `timer_ll.h:187`).
            T0UPDATE => self.t0_latch = self.t0_count(now),
            T0ALARMLO | T0ALARMHI => {
                self.t0_alarm = replace_half(self.t0_alarm, offset == T0ALARMHI, value, T0_HI_MASK);
                self.evaluate(ctx);
            }
            T0LOADLO | T0LOADHI => {
                self.t0_load = replace_half(self.t0_load, offset == T0LOADHI, value, T0_HI_MASK);
            }
            // Any write reloads (`timer_ll_trigger_soft_reload`, `timer_ll.h:258`).
            T0LOAD => {
                self.set_t0(self.t0_load, now);
                self.evaluate(ctx);
            }
            WDTWPROTECT => self.wdt_key = value,
            WDTCONFIG0 | WDTCONFIG1 | WDTFEED => self.write_watchdog(ctx, offset, value),
            offset if (WDTCONFIG2..=WDTCONFIG5).contains(&offset) => {
                self.write_watchdog(ctx, offset, value);
            }
            RTCCALICFG => self.calibration.write_config(value, now),
            RTCCALICFG2 => self.calibration.write_config2(value, now),
            INT_ENA => {
                self.int_ena = value & INT_MASK;
                self.drive(ctx);
            }
            INT_CLR => {
                self.int_raw &= !value;
                self.drive(ctx);
            }
            REGCLK => {
                self.regclk = value & REGCLK_STORED;
                self.retime(ctx);
                self.evaluate(ctx);
            }
            // Read-only: the write is reported as unmodelled.
            _ => return false,
        }
        true
    }
}
```

- [ ] **Step 6: Wire it**

```diff
--- a/crates/soc-esp32c3/src/periph/mod.rs
+++ b/crates/soc-esp32c3/src/periph/mod.rs
@@ -1,6 +1,6 @@
 //! The modelled peripherals of the ESP32-C3 and their place in the address space.
 use crate::config::{STRAP_SPI_FAST_FLASH_BOOT, SocConfig};
-use crate::ctx::{Ctx, Event, Module};
+use crate::ctx::{Ctx, Event, Group, Module};
 use crate::flash::FlashDevice;
 use crate::reset::{ResetCause, ResetLevel};
 use emu_core::Peripheral;
@@ -21,6 +21,7 @@ pub mod sha;
 pub mod spi_mem;
 pub mod system;
 pub mod systimer;
+pub mod timg;
 pub mod uart;
 pub mod usb_serial_jtag;
 
@@ -39,6 +40,7 @@ pub use sha::Sha;
 pub use spi_mem::{Spi0, Spi1};
 pub use system::System;
 pub use systimer::Systimer;
+pub use timg::Timg;
 pub use uart::Uart;
 pub use usb_serial_jtag::UsbSerialJtag;
 
@@ -238,7 +240,6 @@ pub fn accepted_without_effect() -> Vec<Accepted> {
         spi_mem::accepted(),
         apb_ctrl::accepted(),
         ana_i2c::accepted(),
-        TIMG0_ACCEPTED.to_vec(),
         APB_SARADC_ACCEPTED.to_vec(),
         RADIO_POWER_ACCEPTED.to_vec(),
     ]
@@ -310,6 +311,9 @@ const ANA_I2C: (u32, u32) = (ana_i2c::ANA_I2C_BASE, ana_i2c::ANA_I2C_BASE + 0x10
 /// `DR_REG_TIMERGROUP0_BASE` (ESP-IDF `reg_base.h`; report BC §A.2).
 pub(crate) const TIMG0_BASE: u32 = 0x6001_F000;
 const TIMG0: (u32, u32) = (TIMG0_BASE, 0x6002_0000);
+/// `DR_REG_TIMERGROUP1_BASE` (ESP-IDF `reg_base.h`; report TW §B.1).
+pub(crate) const TIMG1_BASE: u32 = 0x6002_0000;
+const TIMG1: (u32, u32) = (TIMG1_BASE, 0x6002_1000);
 /// `DR_REG_SYSTIMER_BASE` (ESP-IDF `reg_base.h`; report TW §A.1).
 pub(crate) const SYSTIMER_BASE: u32 = 0x6002_3000;
 const SYSTIMER: (u32, u32) = (SYSTIMER_BASE, 0x6002_4000);
@@ -317,22 +321,6 @@ const SYSTIMER: (u32, u32) = (SYSTIMER_BASE, 0x6002_4000);
 pub(crate) const APB_SARADC_BASE: u32 = 0x6004_0000;
 const APB_SARADC: (u32, u32) = (APB_SARADC_BASE, 0x6004_1000);
 
-/// Decision D11: the main watchdog of timer group 0 is stored and never expires. The timers and the
-/// RTC calibration of this block are not modelled here: they are the first thing SP1b-3 adds.
-const TIMG0_ACCEPTED: [Accepted; 2] = [
-    Accepted::svd(
-        "TIMG0",
-        "WDTCONFIG0",
-        "main watchdog 0 enable and stages: it never expires (D11)",
-        "SP1b-3",
-    ),
-    Accepted::svd(
-        "TIMG0",
-        "WDTWPROTECT",
-        "main watchdog 0 write protection",
-        "SP1b-3",
-    ),
-];
 /// The bootloader configures the SAR ADC as an entropy source for the RNG (BA §2 rows 79 to 85).
 const WHY_SARADC: &str = "SAR ADC configuration: no conversion is performed";
 const APB_SARADC_ACCEPTED: [Accepted; 7] = [
@@ -383,7 +371,8 @@ pub struct Peripherals {
     pub sha: Sha,
     pub apb_ctrl: ApbCtrl,
     pub ana_i2c: AnaI2c,
-    pub timg0: AcceptedBlock,
+    pub timg0: Timg,
+    pub timg1: Timg,
     pub systimer: Systimer,
     pub apb_saradc: AcceptedBlock,
     /// Four isolated words: its base is 0, so its offsets are absolute addresses.
@@ -395,8 +384,8 @@ pub struct Peripherals {
 
 impl Peripherals {
     /// The peripherals after a power-on, at the device time of `ctx`: a device that has a first
-    /// deadline schedules it there. The reset values arm the flash-boot protection of the RTC
-    /// watchdog when the chip is strapped to boot from flash.
+    /// deadline schedules it there. The reset values arm the flash-boot protection of main
+    /// watchdog 0 and of the RTC watchdog when the chip is strapped to boot from flash.
     #[must_use]
     pub fn new(config: &SocConfig, flash: Box<dyn FlashDevice>, ctx: &mut Ctx) -> Peripherals {
         let flash_boot = config.strapping & STRAP_SPI_FAST_FLASH_BOOT != 0;
@@ -418,7 +407,8 @@ impl Peripherals {
             sha: Sha::new(),
             apb_ctrl: ApbCtrl::new(config.rng_seed),
             ana_i2c: AnaI2c::new(),
-            timg0: AcceptedBlock::new(TIMG0_BASE, &TIMG0_ACCEPTED),
+            timg0: Timg::new(Group::Timg0, flash_boot, ctx),
+            timg1: Timg::new(Group::Timg1, flash_boot, ctx),
             systimer: Systimer::new(ctx.now()),
             apb_saradc: AcceptedBlock::new(APB_SARADC_BASE, &APB_SARADC_ACCEPTED),
             radio_power: AcceptedBlock::new(0, &RADIO_POWER_ACCEPTED),
@@ -427,10 +417,21 @@ impl Peripherals {
             usb_serial_jtag: UsbSerialJtag::new(),
         };
         periph.rtc_cntl.wake(ctx);
-        periph.systimer.wake(ctx);
+        periph.wake_digital_timers(ctx);
         periph
     }
 
+    const fn flash_boot(&self) -> bool {
+        self.strapping & STRAP_SPI_FAST_FLASH_BOOT != 0
+    }
+
+    /// Gives the timed devices that were just built their first call: each arms its first deadline.
+    fn wake_digital_timers(&mut self, ctx: &mut Ctx) {
+        self.timg0.wake(ctx);
+        self.timg1.wake(ctx);
+        self.systimer.wake(ctx);
+    }
+
     /// Calls the device an event is for: a wake-up whose time has come, or what another part of the
     /// machine has to tell it (ADR 0003).
     pub(crate) fn deliver(&mut self, ctx: &mut Ctx, event: Event) {
@@ -440,7 +441,13 @@ impl Peripherals {
                 self.extmem.latch_store(ctx, instruction_bus);
             }
             Event::Systimer => self.systimer.wake(ctx),
+            Event::Timg(Group::Timg0) => self.timg0.wake(ctx),
+            Event::Timg(Group::Timg1) => self.timg1.wake(ctx),
             Event::RtcWatchdog => self.rtc_cntl.wake(ctx),
+            Event::ClockChanged => {
+                self.timg0.clock_changed(ctx);
+                self.timg1.clock_changed(ctx);
+            }
             Event::Uart(0) => self.uart0.wake(ctx),
             Event::Uart(_) => self.uart1.wake(ctx),
             Event::ModuleReset(module) => self.reset_module(ctx, module),
@@ -448,8 +455,8 @@ impl Peripherals {
     }
 
     /// `SYSTEM.PERIP_RST_EN0/1` put a module in reset: its registers return to their reset values.
-    /// What `esp_restart_noos` does to the UARTs, the SPI controllers and the SYSTIMER before it
-    /// resets the CPU (`IDF:esp_system/port/soc/esp32c3/system_internal.c`; report IR §4.1).
+    /// What `esp_restart_noos` does to the UARTs, the SPI controllers, the timer groups and the
+    /// SYSTIMER before it resets the CPU (`IDF:esp_system/port/soc/esp32c3/system_internal.c`; report IR §4.1).
     fn reset_module(&mut self, ctx: &mut Ctx, module: Module) {
         match module {
             Module::Spi01 => {
@@ -458,6 +465,14 @@ impl Peripherals {
             }
             Module::Uart0 => self.uart0.reset(ctx.now()),
             Module::Uart1 => self.uart1.reset(ctx.now()),
+            Module::Timg0 => {
+                self.timg0 = Timg::new(Group::Timg0, self.flash_boot(), ctx);
+                self.timg0.wake(ctx);
+            }
+            Module::Timg1 => {
+                self.timg1 = Timg::new(Group::Timg1, self.flash_boot(), ctx);
+                self.timg1.wake(ctx);
+            }
             Module::Systimer => {
                 self.systimer = Systimer::new(ctx.now());
                 self.systimer.wake(ctx);
@@ -493,6 +508,7 @@ impl Peripherals {
     }
 
     fn core_reset(&mut self, ctx: &mut Ctx) {
+        let flash_boot = self.flash_boot();
         self.gpio = Gpio::new(self.strapping);
         self.io_mux = IoMux::new();
         self.intmatrix = IntMatrix::new();
@@ -505,7 +521,8 @@ impl Peripherals {
         self.sha = Sha::new();
         self.apb_ctrl.reset();
         self.ana_i2c = AnaI2c::new();
-        self.timg0 = AcceptedBlock::new(TIMG0_BASE, &TIMG0_ACCEPTED);
+        self.timg0 = Timg::new(Group::Timg0, flash_boot, ctx);
+        self.timg1 = Timg::new(Group::Timg1, flash_boot, ctx);
         self.systimer = Systimer::new(ctx.now());
         self.apb_saradc = AcceptedBlock::new(APB_SARADC_BASE, &APB_SARADC_ACCEPTED);
         self.radio_power = AcceptedBlock::new(0, &RADIO_POWER_ACCEPTED);
@@ -515,7 +532,7 @@ impl Peripherals {
         // No rebuilt device asserts an interrupt source, and `SYSTEM` selects the reset clock again.
         ctx.clear_sources();
         ctx.clock_written();
-        self.systimer.wake(ctx);
+        self.wake_digital_timers(ctx);
     }
 
     /// The device that owns the register at `addr`, with its base address.
@@ -553,6 +570,8 @@ impl Peripherals {
             Some((&mut self.ana_i2c, ana_i2c::ANA_I2C_BASE))
         } else if within(TIMG0) {
             Some((&mut self.timg0, TIMG0_BASE))
+        } else if within(TIMG1) {
+            Some((&mut self.timg1, TIMG1_BASE))
         } else if within(SYSTIMER) {
             Some((&mut self.systimer, SYSTIMER_BASE))
         } else if within(APB_SARADC) {
```

```diff
--- a/crates/soc-esp32c3/src/periph/system.rs
+++ b/crates/soc-esp32c3/src/periph/system.rs
@@ -12,7 +12,7 @@ const PERIP_RST_EN0: u32 = svd_regs::esp32c3::addr("SYSTEM", "PERIP_RST_EN0") -
 const PERIP_RST_EN1: u32 = svd_regs::esp32c3::addr("SYSTEM", "PERIP_RST_EN1") - SYSTEM_BASE;
 /// The modules `PERIP_RST_EN0` resets, by bit (`IDF:soc/esp32c3/register/soc/system_reg.h`:
 /// `SYSTEM_SPI01_RST` line 497, `SYSTEM_UART_RST` 491, `SYSTEM_UART1_RST` 479,
-/// `SYSTEM_SYSTIMER_RST` 335). ESP-IDF
+/// `SYSTEM_TIMERGROUP_RST` 431, `SYSTEM_TIMERGROUP1_RST` 419, `SYSTEM_SYSTIMER_RST` 335). ESP-IDF
 /// sets then clears a bit to reset a module (`_timer_ll_reset_register`,
 /// `IDF:hal/esp32c3/include/hal/timer_ll.h:55-66`; `esp_restart_noos` writes `0x20000007` then 0:
 /// report IR §4.1; a UART through `uart_ll_reset_register`, `uart_ll.h:109`).
@@ -20,10 +20,12 @@ const PERIP_RST_EN1: u32 = svd_regs::esp32c3::addr("SYSTEM", "PERIP_RST_EN1") -
 /// ASSUMPTION: bit 0, `SYSTEM_TIMERS_RST`, resets nothing here. `esp_restart_noos` pulses it, and
 /// what it resets is not documented: the timer groups have their own bits (IR §4.3, §7 question
 /// 1; TW §F.1). The other bits belong to modules this plan does not model.
-const RESETS_0: [(u32, Module); 4] = [
+const RESETS_0: [(u32, Module); 6] = [
     (1 << 1, Module::Spi01),
     (1 << 2, Module::Uart0),
     (1 << 5, Module::Uart1),
+    (1 << 13, Module::Timg0),
+    (1 << 15, Module::Timg1),
     (1 << 29, Module::Systimer),
 ];
 /// `PERIP_RST_EN1` bit 2, `SYSTEM_CRYPTO_SHA_RST` (`system_reg.h:553`): the only crypto module
```

```diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -2,6 +2,7 @@
 use crate::bus::SocBus;
 use crate::clock;
 use crate::config::SocConfig;
+use crate::ctx::Event;
 use crate::diag::{Diagnostic, Unsupported};
 use crate::flash::{FlashDevice, FlashImageError, NorFlash};
 use crate::memmap::RESET_PC;
@@ -269,7 +270,11 @@ impl Esp32C3 {
     fn retune(&mut self, pc: u32) {
         let (sysclk_conf, cpu_per_conf) = self.bus.periph.system.clock_registers();
         match clock::cpu_clock(sysclk_conf, cpu_per_conf) {
-            Some(selected) => self.bus.ctx.set_clock(selected),
+            Some(selected) => {
+                self.bus.ctx.set_clock(selected);
+                // The devices that count APB cycles go on at the new rate.
+                self.bus.deliver(Event::ClockChanged);
+            }
             // Decision D22: the previous frequency stays, and the selection is reported.
             None => self.bus.ctx.diag.record(Diagnostic::Unsupported {
                 what: Unsupported::ClockSource {
```

One more ignored test in `reference_boot`:

```diff
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -129,5 +129,5 @@ jobs:
           INKEMU_REF_FLASH: ${{ runner.temp }}/reference-firmware/flash_hello.bin
         # Exact test counts, as in rom-boot: see tools/ci/run-ignored-tests.sh.
         run: |
-          tools/ci/run-ignored-tests.sh 5 --release -p soc-esp32c3 --test reference_boot -- --ignored
+          tools/ci/run-ignored-tests.sh 6 --release -p soc-esp32c3 --test reference_boot -- --ignored
           tools/ci/run-ignored-tests.sh 1 --release -p emu-cli --test run_cli x4_boots_the_reference_firmware -- --ignored
```

- [ ] **Step 7: Run the suite, regenerate the table, run the ignored tests**

Run: `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect > /tmp/bless.log 2>&1; echo "exit=$?"; /usr/bin/git diff --stat docs/registers-accepted-without-effect.md`
Expected: `exit=0`; the two rows of `TIMG0` leave the table. No row names SP1b-3 any more: `grep -c "| SP1b-3 |" docs/registers-accepted-without-effect.md` prints `0`.

Run: `cargo test --workspace > /tmp/t13.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t13.log`
Expected: `exit=0`, `673`.

Run the ignored tests: `tools/ci/run-ignored-tests.sh 16 --release -p soc-esp32c3 --test real_rom -- --ignored`, `tools/ci/run-ignored-tests.sh 6 --release -p soc-esp32c3 --test reference_boot -- --ignored`, `tools/ci/run-ignored-tests.sh 4 --release -p emu-cli --test run_cli -- --ignored --skip x4_boots_the_reference_firmware`, `tools/ci/run-ignored-tests.sh 1 --release -p emu-cli --test run_cli x4_boots_the_reference_firmware -- --ignored`.
Expected: all pass. One life of the application, from power-on to the CPU reset of `esp_restart`, is 28 347 166 CPU cycles and 11.246 s of device time (measured, not pinned: read by stepping the machine until `resets()` is 1. What the tests pin is the milestone: eleven countdown lines one device-second apart to the tick, and a second boot behind `rst:0xc`); the whole `reference_boot` suite runs in about three seconds of host time.

Run the milestone by hand: `cargo run --release -p emu-cli -- run --device x4 --rom-dir "$INKEMU_ROM_ELF_DIR" --flash "$INKEMU_REF_FLASH" --strict-mmio --max-cycles 40000000 > /tmp/life.out 2> /tmp/life.err; echo "exit=$?"; grep -c "Restarting in" /tmp/life.out; grep "rst:0xc\|Saved PC" /tmp/life.out; tail -3 /tmp/life.err`
Expected: `exit=0`, `11`, the lines `rst:0xc (RTC_SW_CPU_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)` and `Saved PC:0x40048b92`, and a summary that ends with `inkemu: 11396706 us of device time; CPU clock at 160 MHz` after `cycle limit reached`.

- [ ] **Step 8: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/soc-esp32c3 crates/emu-cli .github/workflows/ci.yml docs/registers-accepted-without-effect.md
/usr/bin/git commit -m "feat(soc): timer groups; the reference application lives its whole life and restarts

Timer, main watchdogs with the flash-boot protection, slow-clock calibration.

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3 crates/emu-cli .github/workflows/ci.yml docs/registers-accepted-without-effect.md
```

---

## Task 14: An exception is reported with the stack pointer

Decision E12: the line that reports a CPU exception gives the stack pointer the instruction started with, next to its PC. One line per distinct cause and PC, as before: the stack pointer is not part of what makes two reports the same.

**Files:**
- Modify: `crates/soc-esp32c3/src/diag.rs`, `crates/soc-esp32c3/src/machine.rs`

**Interfaces:**
- Consumes: `Diagnostic::Exception { cause: Exception, tval: u32, pc: u32 }`, `Diagnostics::{record, drain}`; `Esp32C3::run_until`; the constant `STACK_POINTER` of `machine.rs` (Task 11).
- Produces: `Diagnostic::Exception { cause: Exception, tval: u32, pc: u32, sp: u32 }`, displayed as `CPU exception <cause> at pc 0x… (tval 0x…, sp 0x…)`.

- [ ] **Step 1: Write the failing tests**

```diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -467,6 +474,7 @@ mod tests {
             cause: cpu_rv32::Exception::IllegalInstr,
             tval: 0,
             pc: 4,
+            sp: 0,
         });
         assert_eq!(
             strict.take_violation(),
@@ -537,6 +545,7 @@ mod tests {
             cause: cpu_rv32::Exception::IllegalInstr,
             tval: 0,
             pc: 4,
+            sp: 0,
         });
         assert_eq!(d.take_violation(), None);
     }
@@ -715,4 +724,37 @@ mod tests {
             "{nowhere}"
         );
     }
+
+    #[test]
+    fn an_exception_names_its_pc_its_tval_and_the_stack_pointer() {
+        let text = Diagnostic::Exception {
+            cause: cpu_rv32::Exception::LoadAccessFault,
+            tval: 0x0000_0010,
+            pc: 0x4200_1234,
+            sp: 0x3FC9_0000,
+        }
+        .to_string();
+        assert_eq!(
+            text,
+            "CPU exception LoadAccessFault at pc 0x42001234 (tval 0x00000010, sp 0x3fc90000)"
+        );
+    }
+
+    #[test]
+    fn the_same_exception_at_the_same_pc_is_listed_once_whatever_the_stack_pointer() {
+        let mut d = Diagnostics::new(false);
+        for sp in [0x3FC9_0000, 0x3FC9_0010] {
+            d.record(Diagnostic::Exception {
+                cause: cpu_rv32::Exception::IllegalInstr,
+                tval: 0,
+                pc: 4,
+                sp,
+            });
+        }
+        assert_eq!(
+            d.drain().len(),
+            1,
+            "one line per cause and PC (decision E12)"
+        );
+    }
 }
```

```diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -570,6 +573,7 @@ mod tests {
                     cause: expected,
                     tval: MISALIGNED,
                     pc: ROM_BASE + 4,
+                    sp: 0,
                 })
             );
         }
@@ -1126,4 +1130,24 @@ mod tests {
         );
         assert_eq!(m.cycles().get(), 3, "only `wfi` was executed");
     }
+
+    #[test]
+    fn an_exception_is_reported_with_the_stack_pointer_of_the_faulting_instruction() {
+        // Decision E12: one line per cause and PC, with what a developer needs to find the frame.
+        let mut m = machine(&[lui(2, 0x3FC9_0000), 0, HANG]);
+        m.run_until(Cycles(20));
+        let exception = m
+            .drain_diagnostics()
+            .into_iter()
+            .find(|d| matches!(d, Diagnostic::Exception { .. }));
+        assert_eq!(
+            exception,
+            Some(Diagnostic::Exception {
+                cause: Exception::IllegalInstr,
+                tval: 0,
+                pc: ROM_BASE + 4,
+                sp: 0x3FC9_0000,
+            })
+        );
+    }
 }
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p soc-esp32c3 --lib > /tmp/t14.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, compile error: `variant 'Diagnostic::Exception' has no field named 'sp'`.

- [ ] **Step 3: Implement**

```diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -125,6 +125,8 @@ pub enum Diagnostic {
         cause: Exception,
         tval: u32,
         pc: u32,
+        /// The stack pointer when the instruction started.
+        sp: u32,
     },
     /// Emitted once when [`MAX_DISTINCT_DIAGNOSTICS`] is reached.
     Suppressed,
@@ -236,9 +238,14 @@ impl fmt::Display for Diagnostic {
                     "access to the flash window at {vaddr:#010x} through MMU entry {entry} (content {content:#05x}, pc {pc:#010x}), which names a page beyond the end of the flash: 0 was returned; check the flash size of the device profile"
                 ),
             },
-            Diagnostic::Exception { cause, tval, pc } => write!(
+            Diagnostic::Exception {
+                cause,
+                tval,
+                pc,
+                sp,
+            } => write!(
                 f,
-                "CPU exception {cause:?} at pc {pc:#010x} (tval {tval:#010x})"
+                "CPU exception {cause:?} at pc {pc:#010x} (tval {tval:#010x}, sp {sp:#010x})"
             ),
             Diagnostic::Suppressed => write!(
                 f,
```

```diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -134,6 +134,8 @@ impl Esp32C3 {
             let pc = self.hart.pc;
             self.bus.set_pc(pc);
             let before = self.hart.cycles.get();
+            // The stack pointer the instruction started with, for the report of an exception.
+            let sp = self.hart.x[STACK_POINTER];
             match self.hart.step(&mut self.bus, &self.timing) {
                 StepOutcome::Waiting => {
                     // The CPU idles: device time jumps to the next instant a device asked for.
@@ -162,6 +164,7 @@ impl Esp32C3 {
                         cause: trap.cause,
                         tval: trap.tval,
                         pc,
+                        sp,
                     });
                 }
                 _ => {}
```

- [ ] **Step 4: Run the suite**

Run: `cargo test --workspace > /tmp/t14.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t14.log`
Expected: `exit=0`, `676`.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/soc-esp32c3/src/diag.rs crates/soc-esp32c3/src/machine.rs
/usr/bin/git commit -m "feat(soc): the report of a CPU exception gives the stack pointer

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3/src/diag.rs crates/soc-esp32c3/src/machine.rs
```

---

## Task 15: Ctrl-C ends a run with its summary

Decisions E11 and E35. A firmware that runs for ever is now the normal case: interrupting it must print what a limit stop prints (where it stopped, device time, repeated diagnostics, the trace hash) and exit with the same code. The request to stop is the only thing the run takes from the host; it is read between two slices and never enters a trace.

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`, `crates/emu-cli/Cargo.toml`, `crates/emu-cli/src/run.rs`, `crates/emu-cli/src/exit.rs`, `crates/emu-cli/src/main.rs`

**Interfaces:**
- Consumes: `run::execute(machine: &mut Esp32C3, options: &RunOptions, console: &mut dyn Write, log: &mut dyn Write) -> RunEnd`; `RunEnd`, `exit::{EXIT_OK, RUN_EXIT_CODES_HELP}`; `StopReason::Asleep` (Task 8).
- Produces: `run::execute(machine: &mut Esp32C3, options: &RunOptions, interrupt: &AtomicBool, console: &mut dyn Write, log: &mut dyn Write) -> RunEnd`; `RunEnd::Interrupted` (exit code 0); `exit::EXIT_SECOND_INTERRUPT: u8 = 130`; the log line `inkemu: interrupted by the user`. Dependency: `ctrlc = "3.5"` (3.5.2 at the time of writing; MIT OR Apache-2.0; on Unix it brings `nix`, MIT).

- [ ] **Step 1: Add the dependency**

In `Cargo.toml`, under `[workspace.dependencies]`, after the `clap` line:

```toml
ctrlc = "3.5"
```

In `crates/emu-cli/Cargo.toml`, under `[dependencies]`, after `clap.workspace = true`:

```toml
ctrlc.workspace = true
```

Run: `cargo deny check > /tmp/deny.log 2>&1; echo "exit=$?"`
Expected: `exit=0` (`advisories ok, bans ok, licenses ok, sources ok`).

- [ ] **Step 2: Write the failing tests**

Every existing call of `execute` in the tests gains `&NOT_INTERRUPTED`; one test sets the flag:

```diff
--- a/crates/emu-cli/src/run.rs
+++ b/crates/emu-cli/src/run.rs
@@ -230,6 +241,8 @@ mod tests {
         strict_mmio: true,
         trace_hash: false,
     };
+    static NOT_INTERRUPTED: AtomicBool = AtomicBool::new(false);
+
     fn options(max_cycles: u64) -> RunOptions {
         RunOptions {
             max_cycles: Some(max_cycles),
@@ -249,7 +262,13 @@ mod tests {
         let (mut console, mut log) = (Vec::new(), Vec::new());
         assert_eq!(
             // A byte takes 6 940 CPU cycles to leave the UART at the reset baud rate and clock.
-            execute(&mut m, &options(60_000), &mut console, &mut log),
+            execute(
+                &mut m,
+                &options(60_000),
+                &NOT_INTERRUPTED,
+                &mut console,
+                &mut log
+            ),
             RunEnd::Budget
         );
         assert_eq!(console, b"hello\n");
@@ -260,7 +279,13 @@ mod tests {
     fn max_cycles_ends_the_run_cleanly() {
         let mut m = machine(&[HANG], MachineOptions::default());
         let (mut console, mut log) = (Vec::new(), Vec::new());
-        let end = execute(&mut m, &options(2_500_000), &mut console, &mut log);
+        let end = execute(
+            &mut m,
+            &options(2_500_000),
+            &NOT_INTERRUPTED,
+            &mut console,
+            &mut log,
+        );
         assert_eq!((end, end.exit_code()), (RunEnd::Budget, 0));
         assert!(
             m.cycles().get() >= 2_500_000 && m.cycles().get() < 2_500_100,
@@ -305,6 +330,7 @@ mod tests {
                 max_cycles: Some(10_000),
                 console: ConsoleChannel::UsbSerialJtag,
             },
+            &NOT_INTERRUPTED,
             &mut console,
             &mut log,
         );
@@ -316,7 +342,13 @@ mod tests {
         let mut m = machine(&reading_unmodeled_register(), MachineOptions::default());
         let (mut console, mut log) = (Vec::new(), Vec::new());
         assert_eq!(
-            execute(&mut m, &options(10_000), &mut console, &mut log),
+            execute(
+                &mut m,
+                &options(10_000),
+                &NOT_INTERRUPTED,
+                &mut console,
+                &mut log
+            ),
             RunEnd::Budget
         );
         let log = text(&log);
@@ -335,7 +367,13 @@ mod tests {
         program.push(jal(0, -4));
         let mut m = machine(&program, MachineOptions::default());
         let (mut console, mut log) = (Vec::new(), Vec::new());
-        execute(&mut m, &options(10_000), &mut console, &mut log);
+        execute(
+            &mut m,
+            &options(10_000),
+            &NOT_INTERRUPTED,
+            &mut console,
+            &mut log,
+        );
         let log = text(&log);
         assert_eq!(log.matches("warning: ").count(), 1, "{log}");
         let repeated: Vec<&str> = log
@@ -357,7 +395,13 @@ mod tests {
     fn strict_mode_ends_with_its_own_exit_code_and_names_the_register() {
         let mut m = machine(&reading_unmodeled_register(), STRICT);
         let (mut console, mut log) = (Vec::new(), Vec::new());
-        let end = execute(&mut m, &options(10_000), &mut console, &mut log);
+        let end = execute(
+            &mut m,
+            &options(10_000),
+            &NOT_INTERRUPTED,
+            &mut console,
+            &mut log,
+        );
         assert_eq!((end, end.exit_code()), (RunEnd::StrictMmio, 3));
         let log = text(&log);
         assert!(
@@ -378,7 +422,13 @@ mod tests {
         let program = printing(UART0, b"x", &reading_unmodeled_register());
         let mut m = machine(&program, STRICT);
         let mut log = Vec::new();
-        let end = execute(&mut m, &options(10_000), &mut Closed, &mut log);
+        let end = execute(
+            &mut m,
+            &options(10_000),
+            &NOT_INTERRUPTED,
+            &mut Closed,
+            &mut log,
+        );
         assert_eq!((end, end.exit_code()), (RunEnd::StrictMmio, 3));
         assert!(
             text(&log).contains("stopped by --strict-mmio"),
@@ -393,7 +443,13 @@ mod tests {
         program.extend(printing(UART0, b"uart", &[HANG]));
         let mut m = machine(&program, MachineOptions::default());
         let (mut console, mut log) = (Vec::new(), Vec::new());
-        execute(&mut m, &options(10_000), &mut console, &mut log);
+        execute(
+            &mut m,
+            &options(10_000),
+            &NOT_INTERRUPTED,
+            &mut console,
+            &mut log,
+        );
         assert_eq!(console, b"uart");
         assert_eq!(
             m.take_console(ConsoleChannel::UsbSerialJtag),
@@ -415,6 +471,7 @@ mod tests {
                 max_cycles: None,
                 console: ConsoleChannel::Uart0,
             },
+            &NOT_INTERRUPTED,
             &mut console,
             &mut log,
         );
@@ -437,7 +494,13 @@ mod tests {
                 },
             );
             let (mut console, mut log) = (Vec::new(), Vec::new());
-            execute(&mut m, &options(5_000), &mut console, &mut log);
+            execute(
+                &mut m,
+                &options(5_000),
+                &NOT_INTERRUPTED,
+                &mut console,
+                &mut log,
+            );
             text(&log)
                 .lines()
                 .find(|l| l.starts_with("trace-hash "))
@@ -472,6 +535,7 @@ mod tests {
                 max_cycles: None,
                 console: ConsoleChannel::Uart0,
             },
+            &NOT_INTERRUPTED,
             &mut Closed,
             &mut log,
         );
@@ -501,6 +565,7 @@ mod tests {
                 max_cycles: None,
                 console: ConsoleChannel::Uart0,
             },
+            &NOT_INTERRUPTED,
             &mut Failing,
             &mut log,
         );
@@ -516,7 +581,13 @@ mod tests {
         program.extend(reading_unmodeled_register());
         let mut m = machine(&program, STRICT);
         let (mut console, mut log) = (Vec::new(), Vec::new());
-        let end = execute(&mut m, &options(10_000), &mut console, &mut log);
+        let end = execute(
+            &mut m,
+            &options(10_000),
+            &NOT_INTERRUPTED,
+            &mut console,
+            &mut log,
+        );
         assert_eq!(end, RunEnd::StrictMmio);
         assert_eq!(
             console, b"hello",
@@ -534,7 +605,13 @@ mod tests {
             MachineOptions::default(),
         );
         let (mut console, mut log) = (Vec::new(), Vec::new());
-        let end = execute(&mut m, &options(300), &mut console, &mut log);
+        let end = execute(
+            &mut m,
+            &options(300),
+            &NOT_INTERRUPTED,
+            &mut console,
+            &mut log,
+        );
         assert_eq!(end, RunEnd::Budget);
         assert!(
             console.len() > 5 && console.iter().all(|&byte| byte == b'a'),
@@ -548,4 +625,29 @@ mod tests {
         );
         assert!(m.device_time_nanos() > 5 * 1_300_000_000);
     }
+
+    #[test]
+    fn an_interrupted_run_ends_with_the_summary_of_a_limit_stop() {
+        // Decision E11: Ctrl-C sets the flag; the run ends at the next slice like a cycle limit.
+        let mut m = machine(&[HANG], MachineOptions::default());
+        let (mut console, mut log) = (Vec::new(), Vec::new());
+        let interrupt = AtomicBool::new(true);
+        let end = execute(
+            &mut m,
+            &RunOptions {
+                max_cycles: None,
+                console: ConsoleChannel::Uart0,
+            },
+            &interrupt,
+            &mut console,
+            &mut log,
+        );
+        assert_eq!((end, end.exit_code()), (RunEnd::Interrupted, 0));
+        let log = text(&log);
+        assert!(log.contains("inkemu: interrupted by the user\n"), "{log}");
+        assert!(
+            log.contains("inkemu: stopped at pc ") && log.contains(" us of device time; "),
+            "the same summary as any other end: {log}"
+        );
+    }
 }
```

```diff
--- a/crates/emu-cli/src/exit.rs
+++ b/crates/emu-cli/src/exit.rs
@@ -27,9 +33,15 @@ mod tests {
 
     #[test]
     fn the_run_help_lists_every_exit_code_a_run_can_end_with() {
-        for code in [EXIT_OK, EXIT_ERROR, EXIT_STRICT_MMIO, EXIT_IDLE] {
+        for code in [
+            EXIT_OK,
+            EXIT_ERROR,
+            EXIT_STRICT_MMIO,
+            EXIT_IDLE,
+            EXIT_SECOND_INTERRUPT,
+        ] {
             assert!(
-                RUN_EXIT_CODES_HELP.contains(&format!("\n  {code}  ")),
+                format!("\n{RUN_EXIT_CODES_HELP}").contains(&format!("\n{code:>3}  ")),
                 "{code} missing from the help"
             );
         }
```

- [ ] **Step 3: Run them and see them fail**

Run: `cargo test -p emu-cli --lib > /tmp/t15.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, compile errors: `execute` takes 4 arguments, no variant `Interrupted`, `cannot find value 'EXIT_SECOND_INTERRUPT'`.

- [ ] **Step 4: The run reads the request between two slices**

```diff
--- a/crates/emu-cli/src/run.rs
+++ b/crates/emu-cli/src/run.rs
@@ -4,6 +4,7 @@ use emu_core::Cycles;
 use soc_esp32c3::diag::MAX_DISTINCT_DIAGNOSTICS;
 use soc_esp32c3::{ConsoleChannel, Esp32C3, StopReason};
 use std::io::{self, Write};
+use std::sync::atomic::{AtomicBool, Ordering};
 
 /// Cycles executed between two flushes of the console and the diagnostics: a fraction of a second
 /// of host time, so output appears as the firmware prints it.
@@ -32,6 +33,8 @@ pub enum RunEnd {
     ConsoleError,
     StrictMmio,
     Idle,
+    /// The user asked the run to stop (Ctrl-C).
+    Interrupted,
 }
 
 impl RunEnd {
@@ -39,7 +42,7 @@ impl RunEnd {
     #[must_use]
     pub const fn exit_code(self) -> u8 {
         match self {
-            RunEnd::Budget | RunEnd::ConsoleClosed => EXIT_OK,
+            RunEnd::Budget | RunEnd::ConsoleClosed | RunEnd::Interrupted => EXIT_OK,
             RunEnd::ConsoleError => EXIT_ERROR,
             RunEnd::StrictMmio => EXIT_STRICT_MMIO,
             RunEnd::Idle => EXIT_IDLE,
@@ -96,14 +99,22 @@ fn summarize_diagnostics(machine: &Esp32C3, log: &mut dyn Write) {
 /// Runs `machine` until it stops, sending the firmware console to `console` and everything else
 /// (warnings, the reason the run ended, the summary) to `log`.
 ///
-/// Output is relayed every [`SLICE_CYCLES`] cycles so a long run shows progress.
+/// Output is relayed every [`SLICE_CYCLES`] cycles so a long run shows progress. `interrupt` is
+/// read between two slices: once it is set (Ctrl-C), the run ends with the same summary as a run
+/// that reached its cycle limit. It is the only thing the run takes from the host, and nothing of
+/// it enters a trace.
 pub fn execute(
     machine: &mut Esp32C3,
     options: &RunOptions,
+    interrupt: &AtomicBool,
     console: &mut dyn Write,
     log: &mut dyn Write,
 ) -> RunEnd {
     let end = loop {
+        if interrupt.load(Ordering::Relaxed) {
+            let _ = writeln!(log, "inkemu: interrupted by the user");
+            break RunEnd::Interrupted;
+        }
         let now = machine.cycles().get();
         let slice_end = now.saturating_add(SLICE_CYCLES);
         let limit = options
```

```diff
--- a/crates/emu-cli/src/exit.rs
+++ b/crates/emu-cli/src/exit.rs
@@ -11,15 +11,21 @@ pub const EXIT_STRICT_MMIO: u8 = 3;
 /// `inkemu run`: the CPU waits for an interrupt and nothing can raise one.
 pub const EXIT_IDLE: u8 = 4;
 
+/// `inkemu run`: a second Ctrl-C ended the process before the first one had ended the run. The
+/// value shells give a process ended by `SIGINT` (128 + 2).
+pub const EXIT_SECOND_INTERRUPT: u8 = 130;
+
 /// The exit codes of `inkemu run`, for its `--help`.
 pub const RUN_EXIT_CODES_HELP: &str = "\
 Exit codes:
-  0  the run ended normally: the cycle limit was reached, or the reader of standard output went away
+  0  the run ended normally: the cycle limit was reached, Ctrl-C was pressed, or the reader of
+     standard output went away
   2  error: unknown device, missing or invalid ROM, refused flash image, or standard output cannot be written
   3  stopped by --strict-mmio (an unmodelled register, an 8/16-bit register write, an operation the
      emulator cannot perform faithfully, or flash that is not mapped)
   4  the CPU waits for an interrupt and none can come: no interrupt line is pending and no device
-     has anything scheduled";
+     has anything scheduled
+130  Ctrl-C was pressed a second time before the run had stopped: no summary was printed";
 
 #[cfg(test)]
 mod tests {
```

- [ ] **Step 5: The binary installs the handler**

A second Ctrl-C ends the process at once, for a run that does not come back to a slice boundary.

```diff
--- a/crates/emu-cli/src/main.rs
+++ b/crates/emu-cli/src/main.rs
@@ -2,7 +2,7 @@
 use boards::{DeviceProfile, MachineOptions, SocFamily};
 use clap::{Parser, Subcommand};
 use cpu_rv32::{HartConfig, MisalignedPolicy};
-use emu_cli::exit::{EXIT_ERROR, EXIT_TEST_FAILURE, RUN_EXIT_CODES_HELP};
+use emu_cli::exit::{EXIT_ERROR, EXIT_SECOND_INTERRUPT, EXIT_TEST_FAILURE, RUN_EXIT_CODES_HELP};
 use emu_cli::flash_file;
 use emu_cli::rom_store::{
     self, AnnouncingFetch, ESP_ROM_ELFS, ESP32C3_ROMS, HttpFetch, RomEntry, RomSource,
@@ -17,6 +17,8 @@ use std::io;
 use std::ops::ControlFlow;
 use std::path::{Path, PathBuf};
 use std::process::ExitCode;
+use std::sync::Arc;
+use std::sync::atomic::{AtomicBool, Ordering};
 
 #[derive(Parser)]
 #[command(
@@ -267,15 +269,33 @@ fn run_device(
         None => profile.instantiate(rom, machine_options),
     };
     eprintln!("{}; {flash_note}", header(&profile, entry, &machine));
+    let interrupt = Arc::new(AtomicBool::new(false));
+    if let Err(e) = watch_ctrl_c(&interrupt) {
+        eprintln!(
+            "warning: cannot watch for Ctrl-C: {e}; the run is not affected, but interrupting it will not print its summary: use --max-cycles to bound it"
+        );
+    }
     let end = execute(
         &mut machine,
         run_options,
+        &interrupt,
         &mut io::stdout().lock(),
         &mut io::stderr().lock(),
     );
     ExitCode::from(end.exit_code())
 }
 
+/// Makes Ctrl-C end the run at the next slice, with its summary. A second Ctrl-C ends the process
+/// at once, for a run that does not come back to the slice boundary.
+fn watch_ctrl_c(interrupt: &Arc<AtomicBool>) -> Result<(), ctrlc::Error> {
+    let requested = Arc::clone(interrupt);
+    ctrlc::set_handler(move || {
+        if requested.swap(true, Ordering::Relaxed) {
+            std::process::exit(i32::from(EXIT_SECOND_INTERRUPT));
+        }
+    })
+}
+
 /// The one line that says what is being emulated and how much to trust its timing.
 fn header(profile: &DeviceProfile, entry: &RomEntry, machine: &soc_esp32c3::Esp32C3) -> String {
     let timing = machine.timing_model();
```

- [ ] **Step 6: Run the suite; try it**

Run: `cargo test --workspace > /tmp/t15.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t15.log`
Expected: `exit=0`, `677`.

Run by hand, with the reference image: `cargo run --release -p emu-cli -- run --device x4 --rom-dir "$INKEMU_ROM_ELF_DIR" --flash "$INKEMU_REF_FLASH" > /tmp/c.out`, press Ctrl-C after the countdown has started.
Expected: the terminal shows `inkemu: interrupted by the user`, then the two lines of the summary (`inkemu: stopped at pc … after … cycles (… instructions)`, `inkemu: … us of device time; CPU clock at … MHz`), and the shell reports exit code 0. Without a terminal: start the same command in the background, `kill -INT` its process after three seconds, and read its exit code with `wait`; two `kill -INT` in a row end it with code 130.

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add Cargo.toml Cargo.lock crates/emu-cli/Cargo.toml crates/emu-cli/src
/usr/bin/git commit -m "feat(cli): Ctrl-C ends a run with its summary

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- Cargo.toml Cargo.lock crates/emu-cli/Cargo.toml crates/emu-cli/src
```

---

## Task 16: The watchdogs reset a chip nothing stops, and a firmware that hangs

Milestone points 4 and 5, on the real mask ROM, with no reference firmware needed: they run in the `rom-boot` job. Nothing is implemented here: the two tests pin what Tasks 10, 11 and 13 built, with the device times the clocks of the model give.

**Files:**
- Modify: `crates/soc-esp32c3/tests/real_rom.rs`, `crates/soc-esp32c3/src/clock.rs`, `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: `Esp32C3::{new, with_flash, run_until, resets, device_time_nanos, take_console, drain_diagnostics, cycles}`, `StopReason::{CycleBudget, Asleep}`; `firmware::builder::{ImageSpec, build_image}`; `cpu_rv32::asm::{lui, addi, sw, jal}`; the helpers of `real_rom.rs` (`elf`, `strict`, `stub_spec`, `STUB_ENTRY`, `FLASH_BOOT_BUDGET`, `INVALID_HEADER_BLANK`).
- Produces: two ignored tests, `a_blank_flash_is_reset_by_the_main_watchdog_then_by_the_rtc_watchdog` and `a_firmware_that_hangs_with_interrupts_masked_is_reset_by_its_own_watchdog`.

- [ ] **Step 1: Write the tests**

A blank chip: main watchdog 0 expires after 26 000 000 cycles of the 20 MHz APB clock, 1.3 s, a core reset the ROM reports as `rst:0x7` with the PC it was caught at; the RTC watchdog, which a core reset does not restart, after 400 000 cycles of the 136 kHz clock, 2.941 s, a system reset reported as `rst:0x10` without a saved PC (TW §D.4, §H.1). A real blank chip prints the same `rst:0x7 (TG0WDT_SYS_RST)` line, and `0x4004c0d4` is one of the saved PCs TW §H.1 collected from such loops: the ROM is caught in its UART routine, as here.

A firmware that hangs: it stops the two watchdogs the reset values arm, starts main watchdog 1 with a 500 ms stage whose action is a core reset, and parks with `mstatus.MIE` clear. The ROM then prints `rst:0x8 (TG1WDT_SYS_RST)` and, as `Saved PC`, the address of the `j .`.

```diff
--- a/crates/soc-esp32c3/tests/real_rom.rs
+++ b/crates/soc-esp32c3/tests/real_rom.rs
@@ -471,3 +471,182 @@ fn two_flash_boots_are_identical_down_to_the_trace_hash() {
     assert_eq!(first.trace_hash, second.trace_hash);
     assert_eq!(first.after_banner, second.after_banner);
 }
+
+/// One second of device time, in nanoseconds.
+const SECOND_NS: u64 = 1_000_000_000;
+/// Cycles between two looks at the machine in the watchdog tests: 50 us at the 20 MHz the ROM
+/// runs at, the resolution the instant of a reset is observed with.
+const WATCHDOG_SLICE: u64 = 1_000;
+const WATCHDOG_SLICE_NS: u64 = 50_000;
+
+/// What a run that is reset by its watchdogs looked like.
+struct WatchdogRun {
+    console: String,
+    /// Device time, in nanoseconds, at the first look that found each reset done.
+    reset_seen_at: Vec<u64>,
+    diagnostics: Vec<Diagnostic>,
+}
+
+/// Runs the rev3 ROM on `flash` until `resets` resets took place and `done` accepts the console.
+fn run_to_resets(flash: Option<Vec<u8>>, resets: u64, done: impl Fn(&str) -> bool) -> WatchdogRun {
+    let rom = RomImage::from_elf(&elf(3)).unwrap();
+    let mut machine = match flash {
+        Some(image) => Esp32C3::with_flash(rom, image, &strict()).unwrap(),
+        None => Esp32C3::new(rom, &strict()),
+    };
+    let (mut uart0, mut reset_seen_at, mut diagnostics) = (Vec::new(), Vec::new(), Vec::new());
+    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace('\r', "");
+    while machine.cycles().get() < FLASH_BOOT_BUDGET * 4
+        && !(machine.resets() >= resets && done(&text(&uart0)))
+    {
+        let stop = machine.run_until(Cycles(machine.cycles().get() + WATCHDOG_SLICE));
+        assert!(
+            matches!(stop, StopReason::CycleBudget | StopReason::Asleep),
+            "{stop:?}"
+        );
+        if machine.resets() > u64::try_from(reset_seen_at.len()).unwrap() {
+            reset_seen_at.push(machine.device_time_nanos());
+        }
+        uart0.extend(machine.take_console(ConsoleChannel::Uart0));
+        let _ = machine.take_console(ConsoleChannel::UsbSerialJtag);
+        diagnostics.extend(machine.drain_diagnostics());
+    }
+    WatchdogRun {
+        console: text(&uart0),
+        reset_seen_at,
+        diagnostics,
+    }
+}
+
+/// Milestone point 4 of plan SP1b-3a. Nothing stops the watchdogs the reset values arm (TW §D.4):
+/// main watchdog 0 after 26 000 000 cycles of the 20 MHz APB clock, 1.3 s, a core reset the ROM
+/// reports as `rst:0x7` with the PC it was caught at; the RTC watchdog, which a core reset does
+/// not restart, after 400 000 cycles of the 136 kHz slow clock, 2.941 s, a system reset reported
+/// as `rst:0x10` without a saved PC.
+#[test]
+#[ignore = "needs INKEMU_ROM_ELF_DIR"]
+fn a_blank_flash_is_reset_by_the_main_watchdog_then_by_the_rtc_watchdog() {
+    // Until the boot that follows the third reset has printed a whole line of its own.
+    let run = run_to_resets(None, 3, |console| {
+        console
+            .split("rst:0x10 (RTCWDT_RTC_RST)")
+            .nth(1)
+            .is_some_and(|rest| rest.contains(INVALID_HEADER_BLANK))
+    });
+    assert_eq!(
+        run.diagnostics,
+        Vec::new(),
+        "strict mode: nothing unmodelled"
+    );
+    // Each reset takes place at the end of the instruction during which its watchdog expired, and
+    // is seen at the next look.
+    for (seen, expired_ns, what) in [
+        (run.reset_seen_at[0], 13 * SECOND_NS / 10, "main watchdog 0"),
+        (
+            run.reset_seen_at[1],
+            26 * SECOND_NS / 10,
+            "main watchdog 0 again",
+        ),
+        // 400 000 slow-clock cycles at 136 kHz: 2.941 176 5 s.
+        (run.reset_seen_at[2], 2_941_176_471, "the RTC watchdog"),
+    ] {
+        assert!(
+            (expired_ns..=expired_ns + 2 * WATCHDOG_SLICE_NS).contains(&seen),
+            "{what} expires at {expired_ns} ns of device time; the reset was seen at {seen} ns"
+        );
+    }
+    let lines: Vec<&str> = run.console.lines().collect();
+    let boots: Vec<usize> = lines
+        .iter()
+        .enumerate()
+        .filter(|(_, line)| line.starts_with("rst:"))
+        .map(|(index, _)| index)
+        .collect();
+    assert_eq!(
+        boots.iter().map(|&i| lines[i]).collect::<Vec<_>>(),
+        [
+            "rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)",
+            "rst:0x7 (TG0WDT_SYS_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)",
+            "rst:0x7 (TG0WDT_SYS_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)",
+            "rst:0x10 (RTCWDT_RTC_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)",
+        ]
+    );
+    // The ROM was printing `invalid header` when it was reset: the PC is in its UART routine
+    // (`ROM3:uart_tx_one_char_uart.part.0@0x4004c0be`, the loop that waits for room in the FIFO).
+    assert_eq!(lines[boots[1] + 1], "Saved PC:0x4004c0d4");
+    assert_eq!(lines[boots[2] + 1], "Saved PC:0x4004c0d4");
+    assert_eq!(
+        lines[boots[3] + 1],
+        INVALID_HEADER_BLANK.trim_end(),
+        "a system reset clears the recorded PC: no `Saved PC` line (TW §D.2)"
+    );
+}
+
+const TIMG1_WDTCONFIG0: u32 = svd_regs::esp32c3::addr("TIMG1", "WDTCONFIG0");
+
+/// `lui`/`addi`/`sw` storing `value` at `addr` through `t0` and `t1`.
+fn store(addr: u32, value: u32) -> [u32; 4] {
+    use cpu_rv32::asm::{addi, lui, sw};
+    const T0: u8 = 5;
+    const T1: u8 = 6;
+    // `addi` sign-extends its 12 bits: the upper part is rounded so the sum is the value.
+    let split = |word: u32| {
+        let upper = word.wrapping_add(0x800) & 0xFFFF_F000;
+        (upper, word.wrapping_sub(upper).cast_signed())
+    };
+    let (page, offset) = split(addr);
+    let (upper, lower) = split(value);
+    [
+        lui(T0, page),
+        lui(T1, upper),
+        addi(T1, T1, lower),
+        sw(T1, T0, offset),
+    ]
+}
+
+/// Milestone point 5 of plan SP1b-3a (spec §10.3 criterion 4). A firmware stops the two
+/// watchdogs the reset values arm, starts main watchdog 1 with a 500 ms stage whose action is a
+/// core reset, and hangs with interrupts masked (`mstatus.MIE` is 0 since reset): its own
+/// watchdog resets it, and the next boot names the watchdog and the instruction it hung in.
+#[test]
+#[ignore = "needs INKEMU_ROM_ELF_DIR"]
+fn a_firmware_that_hangs_with_interrupts_masked_is_reset_by_its_own_watchdog() {
+    use cpu_rv32::asm::jal;
+    let mut program = Vec::new();
+    // `WDTCONFIG0` of both protections: unlocked at reset, 0 stops them (TW §B.3, §C.2).
+    program.extend(store(svd_regs::esp32c3::addr("TIMG0", "WDTCONFIG0"), 0));
+    program.extend(store(svd_regs::esp32c3::addr("RTC_CNTL", "WDTCONFIG0"), 0));
+    // Prescaler 20 000 on the 20 MHz APB clock (`mwdt_ll_set_prescaler`): one count a millisecond.
+    program.extend(store(TIMG1_WDTCONFIG0 + 4, 20_000 << 16));
+    // Stage 0: 500 counts. Enable, stage 0 = reset of the core (`mwdt_ll_config_stage`).
+    program.extend(store(TIMG1_WDTCONFIG0 + 8, 500));
+    program.extend(store(TIMG1_WDTCONFIG0, (1 << 31) | (3 << 29)));
+    program.push(jal(0, 0));
+    let hang_pc = STUB_ENTRY + 4 * u32::try_from(program.len() - 1).unwrap();
+    let code: Vec<u8> = program.iter().flat_map(|word| word.to_le_bytes()).collect();
+    let spec = ImageSpec {
+        segments: vec![(STUB_ENTRY, code)],
+        ..stub_spec()
+    };
+    let run = run_to_resets(Some(build_image(&spec)), 1, |console| {
+        console
+            .split("Saved PC:")
+            .nth(1)
+            .is_some_and(|rest| rest.contains('\n'))
+    });
+    assert_eq!(run.diagnostics, Vec::new());
+    let lines: Vec<&str> = run.console.lines().collect();
+    let reset = lines
+        .iter()
+        .position(|line| line.starts_with("rst:0x8"))
+        .unwrap_or_else(|| panic!("no reset by main watchdog 1:\n{}", run.console));
+    assert_eq!(
+        lines[reset],
+        "rst:0x8 (TG1WDT_SYS_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)"
+    );
+    assert_eq!(
+        lines[reset + 1],
+        format!("Saved PC:{hang_pc:#010x}"),
+        "the `j .` of the firmware"
+    );
+}
```

- [ ] **Step 2: Run them**

Run (with `INKEMU_ROM_ELF_DIR` set): `cargo test --release -p soc-esp32c3 --test real_rom -- --ignored > /tmp/t16.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t16.log`
Expected: `exit=0`, `18 passed`. If the first test fails on a device time, the assertion message gives the expected instant and the one seen: the three instants are the product of the reset values and of the clocks, and a difference means one of them moved.

- [ ] **Step 3: The clock comment becomes a sourced fact; the CI count**

```diff
--- a/crates/soc-esp32c3/src/clock.rs
+++ b/crates/soc-esp32c3/src/clock.rs
@@ -49,7 +49,10 @@ pub struct CpuClock {
 }
 
 impl CpuClock {
-    /// The clocks after a power-on reset: the crystal divided by 2, 20 MHz (decisions D21 and E8).
+    /// The clocks after a power-on reset, and after any core or system reset: the crystal divided
+    /// by 2, 20 MHz (TRM §6.2.4.1; report TW §H.2: the period of the reset loop of a blank chip
+    /// excludes 40 MHz). A CPU reset keeps the selection the firmware made: after `esp_restart`
+    /// the ROM runs on the undivided crystal, 40 MHz (IR §4.1).
     pub const POWER_ON: CpuClock = CpuClock {
         cpu_period: 2 * XTAL_PERIOD,
         apb_period: 2 * XTAL_PERIOD,
```

```diff
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -79,7 +79,7 @@ jobs:
         # Each command must run exactly the expected number of tests, all passing: a renamed or
         # un-ignored test would otherwise leave this job green with nothing proven.
         run: |
-          tools/ci/run-ignored-tests.sh 16 --release -p soc-esp32c3 --test real_rom -- --ignored
+          tools/ci/run-ignored-tests.sh 18 --release -p soc-esp32c3 --test real_rom -- --ignored
           tools/ci/run-ignored-tests.sh 4 --release -p emu-cli --test run_cli -- --ignored --skip x4_boots_the_reference_firmware
   reference-boot:
     # Boots the reference firmware (stock ESP-IDF bootloader + hello_world) from the real mask ROM to
```

- [ ] **Step 4: Run the suite**

Run: `cargo test --workspace > /tmp/t16.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t16.log`
Expected: `exit=0`, `677`. The `test result` lines count 30 ignored tests in all: 18 in `real_rom`, 6 in `reference_boot`, 5 in `run_cli`, and the one the workspace already had.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/soc-esp32c3/tests/real_rom.rs crates/soc-esp32c3/src/clock.rs .github/workflows/ci.yml
/usr/bin/git commit -m "test(soc): the watchdogs reset a blank chip and a firmware that hangs

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3/tests/real_rom.rs crates/soc-esp32c3/src/clock.rs .github/workflows/ci.yml
```

---

## Task 17: Documentation: README, research corrections, the hand-over note

The README stops describing limits that are gone; the earlier research reports receive the corrections the new ones established; the note hands what is left to SP1b-3b and SP1b-4.

**Files:**
- Modify: `crates/cpu-rv32/src/csr_esp32c3.rs`, `crates/soc-esp32c3/src/periph/efuse.rs`, `crates/soc-esp32c3/src/periph/usb_serial_jtag.rs`, `crates/soc-esp32c3/src/periph/mod.rs` (comments and one test string), `README.md`, `docs/registers-accepted-without-effect.md` (the text around the generated table), `tools/conformance/README.md`, `docs/adr/0002-flash-model-and-cache-windows.md` (one sentence), `docs/research/esp32c3-boot-chain.md`, `docs/research/esp32c3-bootloader-accesses.md`, `docs/research/esp32c3-rom-flash-boot.md`, `docs/research/sp1b-test-oracle.md`, `docs/research/esp32c3-timers-and-watchdogs.md`
- Create: `docs/superpowers/notes/2026-10-11-sp1b-3a-carryover.md`

**Interfaces:**
- Consumes: the behaviour of Tasks 1 to 16; the correction lists IR §9 and TW §K.
- Produces: documents, and comments that name the plans still to come.

- [ ] **Step 1: The README, English part**

Replace the status paragraph (the block quote that starts with `> **Status: early development.**`) with:

```markdown
> **Status: early development.** inkemu boots the real ESP32-C3 mask ROM from its reset vector. From a full flash image, the ROM loads the stock ESP-IDF second-stage bootloader, which starts the application; the application runs its whole life — FreeRTOS scheduler, tick interrupt, timers, watchdogs — and when it restarts, the chip boots again. The RV32IMC core is validated against riscv-tests and, through RISCOF, against the Sail reference model. Flash writes and a firmware given on its own come next (sub-project 1b-3b), then the lockstep against QEMU (1b-4). See the [design spec](docs/superpowers/specs/2026-10-11-inkemu-design.md) and the [roadmap](#roadmap).
```

In the console sample that follows `The timestamps are those of the emulated device:`, the eight timestamped lines are no longer what the device prints: since Task 9 the UART takes its time, and the log timestamps with it. Replace the lines from `I (10) boot: ESP-IDF v5.5.2 2nd stage bootloader` to `I (31) cpu_start: Unicore app` (the two `…` lines included) with these, which also say that the console goes on:

```text
I (21) boot: ESP-IDF v5.5.2 2nd stage bootloader
I (21) boot: chip revision: v0.3
I (21) boot: efuse block revision: v1.2
…
I (65) boot: End of partition table
I (68) esp_image: segment 0: paddr=00010020 vaddr=3c010020 size=0656ch ( 25964) map
…
I (116) boot: Loaded app from partition at offset 0x10000
I (118) boot: Disabling RNG early entropy source...
I (134) cpu_start: Unicore app
…
```

Replace the sentence `There is no option yet to boot an application `.bin` on its own: inkemu does not compose a flash from one (planned for sub-project 1b-3).` with:

```markdown
There is no option yet to boot an application `.bin` on its own: inkemu does not compose a flash from one (planned for sub-project 1b-3b).
```

Replace everything from the paragraph that starts with `Today the run goes as far as the first log line of the application.` to the end of the code block that follows `With `--strict-mmio` the run of the reference image stops on the first of its warnings` (the three-line block ending with `inkemu: 32068 us of device time; CPU clock at 80 MHz`) with:

````markdown
The reference image (`hello_world` of ESP-IDF v5.5.2, saved as `flash_hello.bin`) runs its whole life: it prints `Hello world!`, counts down eleven seconds, calls `esp_restart()`, and the ROM boots again, announcing `rst:0xc (RTC_SW_CPU_RST)`. The seconds the application spends waiting are not executed: the CPU sleeps in `wfi` and device time jumps to the next timer event, so the eleven seconds take about one second of host time. With `--max-cycles 50000000`, standard error reads:

```text
inkemu: Xteink X4 (ESP32-C3 v0.3, mask ROM rev3); timing model datasheet-uncalibrated (not calibrated); flash: flash_hello.bin (16777216 bytes in 16 MiB)
inkemu: cycle limit reached; raise --max-cycles to run longer
inkemu: stopped at pc 0x4004c0d4 after 50000000 cycles (33222007 instructions)
inkemu: 11459206 us of device time; CPU clock at 160 MHz
```

With `--strict-mmio` the same run touches no register inkemu does not model and takes no CPU exception, across the restart.

Things worth knowing:

- **`--max-cycles` counts CPU cycles, and a sleeping CPU executes none.** A firmware that sleeps for ever while a timer keeps counting never reaches the limit: stop it with Ctrl-C, which prints the same summary as the limit. A firmware that waits for an interrupt nothing can raise ends the run by itself, with exit code 4.
- **The watchdogs are real.** Without `--flash`, the ROM prints `invalid header: 0xffffffff` until the watchdog the chip arms at reset expires, 1.3 s later, and boots again with `rst:0x7 (TG0WDT_SYS_RST)`, as a blank chip does. A firmware that stops feeding its watchdogs is reset the same way, and the next boot names the watchdog and the address it was caught at (`Saved PC`).
- **The console is as slow as the UART.** A byte leaves at the baud rate the firmware programmed; when a run stops, what was still in the UART is printed too.
- **The flash is read-only.** On the first boot of a layout with OTA slots and a blank `otadata` partition, which is the layout of the Xteink X4, the bootloader records the slot it chose. inkemu reports the refused sector erase and page program (`flash command 0x20 is not modeled`, then `0x02`) and the boot goes on to the application; with `--strict-mmio` the run stops at the first of the two. Flash writes arrive with sub-project 1b-3b.
- **Memory protection is not enforced.** The PMP entries, the permission registers and the stack guard are stored and never raise a fault (sub-project 1b-4).
````

In the paragraph that starts with `The device console goes to standard output.`, the list of operations inkemu reports because it cannot perform them faithfully still ends with a software reset, which Task 10 made real, and lacks the two refusals Tasks 9 and 10 added. Replace `a clock source it cannot turn into a frequency; a software reset)` with:

```markdown
a clock source it cannot turn into a frequency; an RTC slow clock other than the RC oscillator; a byte written to a UART whose transmit FIFO is full)
```

In the roadmap, replace the end of item 1, from `*1a done;` to the end of the line, with:

```markdown
*1a done; 1b-1, the SoC skeleton and the ROM boot, done; 1b-2, flash and the bootloader, done; 1b-3a, interrupts, timers, watchdogs and reset, done; 1b-3b, flash writes and a firmware on its own, is next.*
```

- [ ] **Step 2: The README, French part**

Replace the paragraph that starts with `**État :**` with:

```markdown
**État :** inkemu démarre la vraie ROM masquée de l'ESP32-C3 depuis son vecteur de reset. À partir d'une image flash complète (`--flash`), la ROM charge le bootloader de second niveau, qui lance l'application ; celle-ci vit toute sa vie — ordonnanceur FreeRTOS, interruption de tick, timers, watchdogs — et, quand elle redémarre, la puce démarre de nouveau. Le cœur RV32IMC est validé contre riscv-tests puis, avec RISCOF, contre le modèle de référence Sail ; les commandes `inkemu inspect` et `inkemu rvtest` sont en place. Les écritures en flash et le démarrage d'un firmware fourni seul sont la prochaine étape (sous-projet 1b-3b), puis le lockstep avec QEMU (1b-4).
```

- [ ] **Step 3: Check the README against the binary**

Run: `cargo run --release -p emu-cli -- run --device x4 --rom-dir "$INKEMU_ROM_ELF_DIR" --flash "$INKEMU_REF_FLASH" --max-cycles 50000000 > /tmp/r.out 2> /tmp/r.err; echo "exit=$?"; tail -3 /tmp/r.err`
Expected: `exit=0` and the last three lines of the block quoted in Step 1, character for character (the first line names the image file as it was given).

Run: `grep "boot: ESP-IDF\|chip revision\|efuse block\|End of partition table\|segment 0\|Loaded app\|Disabling RNG\|Unicore app" /tmp/r.out | head -8`
Expected: the eight timestamped lines of the console sample of Step 1, character for character (`I (21) boot: ESP-IDF v5.5.2 2nd stage bootloader` to `I (134) cpu_start: Unicore app`).

Run: `cargo run --release -p emu-cli -- run --device x4 --rom-dir "$INKEMU_ROM_ELF_DIR" --max-cycles 50000000 > /tmp/b.out 2> /tmp/b.err; echo "exit=$?"; grep -c "rst:0x7" /tmp/b.out; tail -1 /tmp/b.err`
Expected: `exit=0`, `1`, `inkemu: 2500000 us of device time; CPU clock at 20 MHz`.

- [ ] **Step 4: Corrections to the earlier research reports**

The two new reports list what they correct in the earlier ones and left those files untouched (IR §9, a list of nine items; TW §K, a table of seven rows). A report known to be wrong is debt: each earlier report receives, as its last section, the entries that name it, **copied word for word** from those two lists, under this heading and sentence:

```markdown
## Corrections (2026-10-11)

Established after this report was written, by `esp32c3-interrupts-and-reset.md` (§9) and `esp32c3-timers-and-watchdogs.md` (§K). The text above is left as it was; where it disagrees with an entry below, the entry is right.
```

The items of IR §9 are copied as the list items they are; the rows of TW §K are copied under the two header lines of that table (`| Place | Statement there | Established here |` and `|---|---|---|`), a blank line between the list and the table.

- `docs/research/esp32c3-boot-chain.md`: the five items of IR §9 that start with `BC`, and the rows of TW §K whose first column names `esp32c3-boot-chain.md` (three rows: §E.3/§K.4, §E.2, §F.2).
- `docs/research/esp32c3-bootloader-accesses.md`: the two items of IR §9 that start with `BA`, and the rows of TW §K whose first column names `esp32c3-bootloader-accesses.md` (four rows: §10.8, §10.1, §6, §9 row 2). The first row of the table names three reports: it is copied in each.
- `docs/research/esp32c3-rom-flash-boot.md`: the rows of TW §K whose first column names `esp32c3-rom-flash-boot.md` (two rows: §L.2, §H).
- `docs/research/sp1b-test-oracle.md`: the two items of IR §9 that start with `TO`.

Then append to `docs/research/esp32c3-timers-and-watchdogs.md`, after its last section:

```markdown
## M. Corrections found while planning SP1b-3a (2026-10-11)

- §B.3 says the configuration registers of a main watchdog take effect when `WDTCONFIG0.CONF_UPDATE_EN` is written. `_timer_ll_reset_register` (`IDF:hal/esp32c3/include/hal/timer_ll.h:50-66`) clears `FLASHBOOT_MOD_EN` with a plain read-modify-write right after a peripheral reset, without that bit, "to prevent the system from being reset unexpectedly": the flash-boot bit at least acts when it is written. The model applies every write at once and reads `CONF_UPDATE_EN` as 0; which registers really wait for the bit is open (list of SP1b-4).
- The SVD gives 0 as the reset value of `RTC_CNTL.WDTWPROTECT` and `RTC_CNTL.SWD_WPROTECT`. ESP-IDF's header gives the keys, `0x50D83AA1` and `0x8F1D312A` (`IDF:soc/esp32c3/register/soc/rtc_cntl_reg.h:1700`, `:1758`): both watchdogs are unlocked at reset, which is why the bootloader can stop the RTC watchdog. The model uses the header's values.
- §H.1 lists `0x4004c0d4` among the saved PCs of blank-chip reset loops. The model reproduces it: with the UART transmit time modelled, the watchdog catches the ROM in `uart_tx_one_char_uart.part.0`, waiting for room in the FIFO.
- The reference application reads `EFUSE + 0x18C` at every start (`esp_efuse_utility_check_errors`, the load at `0x4200045c`), a word between `RD_REPEAT_ERR3` and `RD_REPEAT_ERR4` that neither the SVD nor `efuse_reg.h` documents. The trace on QEMU did not show it as unmodelled because QEMU answers the whole block. The model answers 0.
- The golden log of `sp1b-test-oracle.md` §2 ends at `Minimum free heap size`: the capture was cut before the countdown. The life-cycle test of SP1b-3a pins the rest from the model, not from the oracle.
```

- [ ] **Step 5: The hand-over note**

Create `docs/superpowers/notes/2026-10-11-sp1b-3a-carryover.md`:

```markdown
# SP1b-3a — points reportés

Note de passage du plan `docs/superpowers/plans/2026-10-11-sp1b-3a-interrupts-timers-reset.md` (base de temps, interruptions, timers, watchdogs, reset) aux plans suivants.

## Résultat

L'image de référence (bootloader ESP-IDF v5.5.2 et `hello_world`, mêmes empreintes que pour SP1b-2) vit toute sa vie sur la vraie ROM : `Hello world!`, onze lignes de compte à rebours espacées d'une seconde d'appareil, `Restarting now.`, puis un second démarrage que la ROM annonce par `rst:0xc (RTC_SW_CPU_RST)` et `Saved PC:0x40048b92`. Une vie, de la mise sous tension au reset du CPU par `esp_restart`, représente 28 347 166 cycles de CPU pour 11,246 s d'appareil (mesuré, non figé par un test : les tests figent le jalon, onze lignes de compte à rebours espacées d'une seconde d'appareil au tick près et un second démarrage derrière `rst:0xc`) ; le reste est dormi. En `--strict-mmio`, aucun registre non modélisé et aucune exception sur tout le cycle. Sans image, la puce est réinitialisée par ses watchdogs à 1,3 s, 2,6 s (`rst:0x7`) puis 2,941 s (`rst:0x10`). Un firmware qui se bloque, interruptions masquées, est réinitialisé par son propre watchdog, et le démarrage suivant donne l'adresse où il était bloqué.

Le mécanisme est décrit par l'ADR 0003 : contexte passé à chaque accès, temps de l'appareil en cycles d'une base à 160 MHz, périphériques appelés à l'instant qu'ils ont demandé, compteurs calculés, trois niveaux de reset.

## Décisions prises pendant l'écriture du plan

Le tableau du plan (E17 à E41) fait foi. Celles qui engagent la suite :

- **Pas de `--max-time`** (E11, E21). `--max-cycles` compte les cycles du CPU, qui n'avancent pas pendant `wfi`. Un firmware qui dort pour toujours pendant qu'un périphérique compte n'est borné que par Ctrl-C. `run_until` rend la main par `StopReason::Asleep` après une seconde d'appareil dormie ou 100 000 réveils sans interruption.
- **Appels sans objet** (E19). Un périphérique réinitialisé, nourri ou reprogrammé ne retire pas ses demandes d'appel : elles ne trouvent rien à faire. Un sommeil peut donc s'arrêter à une date où rien ne se passe.
- **Console** : un octet d'UART arrive sur la console quand son dernier bit est parti ; à la fin d'une exécution, la console reçoit aussi ce qui restait dans l'UART (`Esp32C3::flush_console`).
- **La PMP reste mémorisée et n'est pas appliquée** (E31), de même que le garde-pile d'`ASSIST_DEBUG` et les registres de permission de `SENSITIVE`.

## Reporté au plan SP1b-3b (écritures en flash, `--firmware`, CrossPoint)

- Tout ce que le cadrage lui attribue : programmation, effacement et lecture de données par SPI1, fichier de flash par appareil, troisième terme de `Bus::page_version`, vue en direct ou lignes de cache périmées (ADR 0002), `--firmware` et l'ADR sur le bootloader embarqué, côté hôte de l'USB-Serial-JTAG, identifiant de `DeviceProfile` comme type validé, tables de partitions dans les profils, règle de taille d'une `FlashDevice` externe, comptage des écritures dans le test OTA, registres de programmation des eFuse.
- **Réception UART et interruptions UART.** L'émission est temporisée et demande un appel quand l'émetteur devient libre (`Event::Uart`) : l'interruption `TX_DONE` peut s'y brancher. Lignes `INT_ENA`, `INT_CLR`, `CONF0`, `CONF1` de la table.
- **Sommeil profond, sommeil léger et réveil par le timer RTC** (spec §10.2, point 7) : hors du jalon de SP1b-3a ; ils reviennent à SP1b-3b (décision E40, confirmée), avec le premier code de CrossPoint qui dort. Les registres concernés (`TIMER1`, `TIMER3` à `TIMER6`, `DIG_PWC`, `PWC`, `DIG_ISO`, bits d'alimentation d'`OPTIONS0`, `SYSTEM.MEM_PD_MASK`) sont des lignes de la table qui nomment SP1b-3b.
- **Une limite en temps de l'appareil pour les exécutions scriptées.** La spec (§6.4, §10.2) ne nomme que `--max-cycles`. Dès qu'un firmware dort, cette limite ne borne plus rien. À décider avec le sous-projet 4 ; c'est une demande de modification de la spec.
- Retouches de forme laissées à qui touchera `boards` : section `# Errors` de `from_named_toml` (omet `MissingFlash`), `ProfileError::Parse` qui répète le remède.

## Reporté au plan SP1b-4 (lockstep QEMU, CoreMark, rejeu)

- **Le lockstep ne traverse pas un reset** : QEMU réinitialise toute la machine sur `esp_restart`, le silicium et inkemu seulement le CPU ; le second démarrage exécute un autre code (`rtc_clk_init`) et affiche une ligne de plus (`Saved PC`). Comparer jusqu'au reset, puis repartir d'un état commun.
- **Latence d'interruption** : 4 cycles ici (`INTERRUPT_LATENCY_CYCLES`), aucune dans QEMU. La rendre réglable pour le lockstep.
- **Application de la PMP, déclencheurs de débogage, garde-pile, permissions (`SENSITIVE`)** : registres mémorisés, aucune faute levée. Les sources d'interruption 54 (`ASSIST_DEBUG`) et 56 à 58 (PMS) ne sont jamais levées.
- `mstatus.MPP` reste câblé à 3 ; `mie` et `mip` sont de simples registres sur l'ESP32-C3.
- **Hypothèses à trancher**, chacune marquée `ASSUMPTION:` à l'endroit du code indiqué, avec ce qui la trancherait :
  - cœur : réveil de `wfi` par une ligne en attente quand `mstatus.MIE` vaut 0 (`hart.rs`) ; bits 7:2 de `mtvec` lus à 0 (`csr.rs`) ; registres entiers remis à zéro par un reset (`hart.rs`) ;
  - contrôleur d'interruptions (`intmatrix.rs`) : front mémorisé seulement si la ligne est activée ; mémoire de front effacée au passage de 1 à 0 de `CPU_INT_CLEAR` ; une ligne masquée ou relâchée disparaît sans latence (`machine.rs`) ;
  - resets : le contrôleur d'interruptions survit à un reset CPU (`periph/mod.rs`) ; un reset système efface `STORE0..7` (`rtc_cntl.rs`) ; `SYSTEM_TIMERS_RST` (bit 0 de `PERIP_RST_EN0`) ne réinitialise rien (`system.rs`) ; un reset du SYSTIMER remet ses compteurs à zéro (`systimer.rs`) ; valeur de `Saved PC` après un reset logiciel : l'instruction qui suit l'écriture, dans la ROM, alors qu'une puce réelle affiche une adresse de l'application (`assist_debug.rs`) ;
  - watchdogs (`timg.rs`, `rtc_cntl.rs`) : un registre de configuration agit dès son écriture, sans `CONF_UPDATE_EN` ; un étage expire quand le compteur atteint ou dépasse son délai ; un étage « off » est attendu puis ne fait rien ; un prédiviseur à 0 divise par 65 536 ; un watchdog qu'on active compte depuis 0 ; la protection de démarrage est inactive si la puce n'est pas configurée pour démarrer sur la flash ; sous cette protection, l'étage 0 du watchdog RTC est un reset système ; super watchdog : inactif tant que `FIB_SEL` bit 2 n'est pas effacé, demande à être nourri à 0,9 s, réinitialise à 1 s ;
  - horloges : oscillateurs RC à leur fréquence nominale (136 kHz), ce que la borne mesurée sur silicium (0,955 s entre deux resets) ne contredit pas ; pas de quartz 32 kHz, donc sa calibration finit en `TIMEOUT` ; `TIMEOUT_THRES` en cycles du quartz ; abaisser ce seuil en mode cyclique lève `TIMEOUT` aussitôt (`timg.rs`) ;
  - SYSTIMER (`systimer.rs`) : valeur lue valide dès la demande ; un comparateur en mode cible compare même si son compteur est arrêté ;
  - UART (`uart.rs`) : dix temps de bit par octet quel que soit `CONF0` ; une source d'horloge autre que l'APB est le quartz ; `ST_UTX_OUT` lit 1 pendant l'émission ;
  - eFuse : le mot en `+0x18C` lit 0 (`efuse.rs`).

## À prévoir plus tard

- **SP2** : brown-out et la partie de `FIB_SEL` qui le concerne, maintien des pads, `SENSOR_CTRL`, format de trame de l'UART avec sa réception, routage des entrées par la matrice GPIO, texte de la ligne `IO_MUX.DATE` de la table.
- **SP4** : limite en temps de l'appareil et critère de réussite explicite pour une exécution scriptée.
- **SP5a, SP5b** : horloges et resets de la radio (`APB_CTRL.WIFI_CLK_EN`, `WIFI_RST_EN`, `SYSTEM.BT_LPCK_DIV_*`).
- **SP8** : portes d'horloge (un module dont l'horloge est coupée répond toujours), réglages analogiques et de régulateurs, calibration du modèle de temps, coût d'une invalidation de tout le code de flash décodé à chaque écriture d'`ICACHE_CTRL`, coût des appels sans objet s'il devient mesurable.
```

- [ ] **Step 6: Comments that still hand something to "SP1b-3"**

Five comments and one test string name a plan that is now over. Each gets the plan that really inherits the point:

```diff
--- a/crates/cpu-rv32/src/csr_esp32c3.rs
+++ b/crates/cpu-rv32/src/csr_esp32c3.rs
@@ -27,13 +27,13 @@ pub const PCMR_ENABLE: u32 = 1 << 0;
 
 /// Physical memory protection: 16 entries (ESP-IDF `SOC_CPU_PMP_REGION_GRANULARITY`, `cpu_region_protect.c`;
 /// report BC §B.2). The registers are stored and the lock bit is honoured; **the protection is not
-/// enforced** (decision D18): enforcement arrives with the application's memory protection in SP1b-3.
+/// enforced** (decision D18): enforcement arrives with the application's memory protection in SP1b-4.
 ///
 /// ASSUMPTION: a `pmpcfg` byte is stored verbatim. The privileged specification makes its fields
 /// WARL — bits 5 and 6 are reserved, and so is the combination R = 0 with W = 1 — and what this core
 /// keeps of them is not established; nothing reads them back on the measured paths. A readback on a
-/// real chip, or the SP1b-4 lockstep, settles it. A lock is also never cleared, because no hart
-/// reset exists yet: the reset of SP1b-3 must clear the whole PMP state.
+/// real chip, or the SP1b-4 lockstep, settles it. Software cannot clear a lock; a reset does:
+/// `Hart::reset` builds a new CSR file, and the whole PMP state with it.
 pub const PMPCFG0: u16 = 0x3A0;
 pub const PMPADDR0: u16 = 0x3B0;
 pub const PMP_ENTRIES: usize = 16;
```

```diff
--- a/crates/soc-esp32c3/src/periph/efuse.rs
+++ b/crates/soc-esp32c3/src/periph/efuse.rs
@@ -1,9 +1,10 @@
 //! eFuse read mirror of a production chip. Layout: `efuse_reg.h` (report §H).
 //! The mirror answers only the read registers the SVD documents between `RD_WR_DIS` and `RD_RS_ERR1`
-//! (`svd_regs::esp32c3`, peripheral `EFUSE`): the reserved holes between them (such as 0x18C and
-//! 0x194..=0x1BC) are not registers, so they stay unmodelled and surface as diagnostics (spec §5.2).
+//! (`svd_regs::esp32c3`, peripheral `EFUSE`): the reserved holes between them (such as
+//! 0x194..=0x1BC) are not registers, so they stay unmodelled and surface as diagnostics (spec §5.2);
+//! one hole is answered because ESP-IDF reads it, see `RD_REPEAT_ERR_GAP`.
 //! The content is that of a production chip (decision D9); programming registers stay unmodelled
-//! until a firmware needs them (SP1b-3).
+//! until a firmware needs them (SP1b-3b).
 use crate::chip::ChipRevision;
 use crate::ctx::Ctx;
 use crate::periph::EFUSE_BASE;
```

```diff
--- a/crates/soc-esp32c3/src/periph/usb_serial_jtag.rs
+++ b/crates/soc-esp32c3/src/periph/usb_serial_jtag.rs
@@ -1,7 +1,7 @@
 //! USB-Serial-JTAG, device-to-host serial path. Register layout: `usb_serial_jtag_reg.h` (report §G.3).
 //!
 //! The model behaves like a board with a host attached and reading: the IN endpoint always has
-//! room. Reception, interrupts and host attach/detach arrive in SP1b-3.
+//! room. Reception, interrupts and host attach/detach arrive in SP1b-3b.
 use crate::ctx::Ctx;
 use emu_core::Peripheral;
 
@@ -131,7 +131,7 @@ mod tests {
         assert_eq!(
             usb.read(ctx, 0x08),
             None,
-            "INT_RAW arrives with interrupts (SP1b-3)"
+            "INT_RAW arrives with interrupts (SP1b-3b)"
         );
         assert!(!usb.write(ctx, 0x18, 0), "CONF0");
         assert_eq!(usb.read(ctx, 0x00), None, "no host-to-device data yet");
```

```diff
--- a/crates/soc-esp32c3/src/periph/mod.rs
+++ b/crates/soc-esp32c3/src/periph/mod.rs
@@ -750,7 +750,7 @@ mod tests {
     #[should_panic(expected = "stores single registers only")]
     fn a_block_row_given_to_plain_storage_is_a_programming_error() {
         const BLOCK: [Accepted; 1] =
-            [Accepted::svd("INTERRUPT_CORE0", "MAC_INTR_MAP", "routing", "SP1b-3").block(62)];
+            [Accepted::svd("INTERRUPT_CORE0", "MAC_INTR_MAP", "routing", "SP8").block(62)];
         let _ = Storage::accepted(INTMATRIX_BASE, &BLOCK);
     }
 
```

Two documents say what those comments said. The text around the generated table (the bless of the earlier tasks rewrites the table only) and the conformance README:

```diff
--- a/docs/registers-accepted-without-effect.md
+++ b/docs/registers-accepted-without-effect.md
@@ -17,7 +17,7 @@ No other memory-mapped register is silent: it is either modelled or reported by
 The PMP CSRs (`pmpcfg0` to `pmpcfg3`, `pmpaddr0` to `pmpaddr15`) are accepted without effect too,
 and are not in the table because they are CSRs, not memory-mapped registers: on the ESP32-C3 core
 they are stored with their lock rules and **never enforced** — no access is refused because of them
-(decision D18 of plan SP1b-2). Plan SP1b-3 enforces them.
+(decision D18 of plan SP1b-2). Plan SP1b-4 enforces them (decision E31 of plan SP1b-3a).
 
 The table is generated: the lists live next to each peripheral model (`accepted()` in
 `crates/soc-esp32c3/src/periph/*.rs`). A model that is plain storage is built from its list; a model
@@ -31,8 +31,8 @@ then rewrites the table below, and nothing else; review the diff before committi
 
 "Count" is 1 for a single register, or the number of consecutive registers the row stands for.
 "Made real by" names the plan that gives the register its effect. Each plan inherits the rows that
-name it: the carry-over note `docs/superpowers/notes/2026-10-11-sp1b-2-carryover.md` says so under
-SP1b-3, SP1b-4, SP2, SP5a and SP8.
+name it: the carry-over note `docs/superpowers/notes/2026-10-11-sp1b-3a-carryover.md` says so under
+SP1b-3b, SP1b-4, SP2, SP5a, SP5b and SP8.
 
 <!-- generated table -->
 | Address | Register | Count | Hardware effect not modelled | Made real by |
```

```diff
--- a/tools/conformance/README.md
+++ b/tools/conformance/README.md
@@ -25,7 +25,7 @@ group name and expected vs actual).
 - **Excluded: `pmp32`.** RISCOF selects it for any DUT with Zicsr (the tests carry no PMP feature
   check) but the generic hart this bench runs has no PMP CSRs and the ISA YAML does not advertise
   them. The ESP32-C3 hart stores the PMP registers (`pmpcfg0..3`, `pmpaddr0..15`) since SP1b-2 and
-  does not enforce them; enforcement arrives in SP1b-3. To re-enable `pmp32` then: add
+  does not enforce them; enforcement arrives in SP1b-4. To re-enable `pmp32` then: add
   `pmp32=<count>` to `ARCH_TEST_EXPECTED` and update `ARCH_TEST_TOTAL` in `versions.env`; the script
   copies exactly the groups listed there.
 - **No Zicsr group.** riscv-arch-test 3.10.0 has no `Zicsr` directory: CSR instructions are covered
```

ADR 0002 hands the choice between the live view and stale cache lines to the plan that brings flash writes, and names it "SP1b-3": that plan is SP1b-3b. In `docs/adr/0002-flash-model-and-cache-windows.md`, in the item that starts with `- **Écart connu avec le silicium**`, replace

```markdown
(SP1b-3) ; SP1b-3 devra choisir
```

with

```markdown
(SP1b-3b) ; SP1b-3b devra choisir
```

- [ ] **Step 7: Check that the table and the code agree, and that nothing names a plan that is over**

Run: `cargo test -p soc-esp32c3 --test accepted_without_effect > /tmp/acc.log 2>&1; echo "exit=$?"; grep -c "| SP1b-3 |" docs/registers-accepted-without-effect.md; grep -rn "SP1b-3[^ab]" crates --include=*.rs | grep -v "SP1b-3a\|SP1b-3b" | wc -l; grep -n "SP1b-3[^ab]\|1b-3[^ab]" README.md docs/registers-accepted-without-effect.md tools/conformance/README.md docs/adr/0002-flash-model-and-cache-windows.md | wc -l`
Expected: `exit=0`, `0`, `0`, `0`: no row, no comment and none of the four documents still hands something to "SP1b-3".

- [ ] **Step 8: Final checks and commit**

```bash
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
cargo test --workspace > /tmp/t17.log 2>&1; echo "exit=$?"
cargo deny check > /tmp/deny.log 2>&1; echo "exit=$?"
cargo build -p soc-esp32c3 --target wasm32-unknown-unknown > /tmp/wasm.log 2>&1; echo "exit=$?"
cargo llvm-cov --workspace --fail-under-lines 80 > /tmp/cov.log 2>&1; echo "exit=$?"
```

Expected: six times `exit=0`.

```bash
/usr/bin/git add README.md docs/research docs/registers-accepted-without-effect.md tools/conformance/README.md docs/adr/0002-flash-model-and-cache-windows.md docs/superpowers/notes/2026-10-11-sp1b-3a-carryover.md crates/cpu-rv32/src/csr_esp32c3.rs crates/soc-esp32c3/src/periph
/usr/bin/git commit -m "docs: the application lives its whole life; research corrections; hand-over of SP1b-3a

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- README.md docs/research docs/registers-accepted-without-effect.md tools/conformance/README.md docs/adr/0002-flash-model-and-cache-windows.md docs/superpowers/notes/2026-10-11-sp1b-3a-carryover.md crates/cpu-rv32/src/csr_esp32c3.rs crates/soc-esp32c3/src/periph
```
