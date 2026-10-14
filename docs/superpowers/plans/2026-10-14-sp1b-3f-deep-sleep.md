# SP1b-3f — Deep sleep and the RTC-timer wake — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** item 7 of §10.2 of the global spec ("RTC_CNTL (reset, deep sleep, wake by timer)"): an ESP-IDF firmware that calls `esp_sleep_enable_timer_wakeup(us)` then `esp_deep_sleep_start()` stops executing at the write of `STATE0.SLEEP_EN`, device time jumps to the alarm of the RTC timer without stepping, and the chip wakes as a core reset of cause 5: the ROM prints `rst:0x5 (DSLEEP),boot:0x8 (SPI_FAST_FLASH_BOOT)`, checks the CRC of the RTC fast memory and calls the wake stub, ESP-IDF reads the wake cause "timer", and the RTC memory, the RTC timer and `STORE0..7` are what they were. Light sleep is refused by name and rejected as the chip rejects it; a GPIO wake is named as a function of sub-project 2. Checked in the CI by `deep_sleep_timer`, an application of this repository.

**Architecture:**

| Crate | Owns | Why a GUI can call it |
|---|---|---|
| `soc-esp32c3` | `periph/rtc_cntl/` split into `mod.rs`, `timer.rs` (RTC timer), `wdt.rs` (RTC watchdog), `swd.rs` (super watchdog), `sleep.rs` (sleep control, wake sources, alarm); `periph/timg/` split into `mod.rs` and `calibration.rs`; the RTC fast-memory CRC unit in `periph/system.rs`, answered by the bus; `ResetCause::DeepSleep`; the four named refusals; `StopReason::DeepSleep`; `Esp32C3::deep_sleep` | the machine says whether it sleeps, since when, until when and on which sources; the CLI only prints it |
| `emu-cli` | `RunEnd::DeepSleep` (exit 0), the end line of a run that ends asleep, the `run_cli` test of `deep_sleep_timer` | printing only |
| `console-script` | the new stop reason in its run loop | — |
| `tools/reference-firmware` | `deep_sleep_timer/` (ours, Apache-2.0) | — |

**Tech Stack:** Rust 1.99.0 (edition 2024); no new crate, no new dependency. ESP-IDF v5.5.2 (`tools/conformance/versions.env`) in the pinned Docker image, which only the CI runs.

**Spec:** `docs/superpowers/specs/2026-10-14-sp1b-3f-design.md` (decisions F1–F13, binding). Global spec `docs/superpowers/specs/2026-10-11-inkemu-design.md` (§3.1 determinism, §7 errors, §10.2 item 7). Facts: `docs/research/esp32c3-deep-sleep.md` (written by Task 1 from the two research reports of this sub-project; **DS** below).

**What was verified, and where.** On the development host, by the rule of the project, only `cargo fmt --all`, `tools/ci/clippy.sh -p <touched crates> --all-targets --all-features -- -D warnings` and narrow `cargo test -p <crate> <filters>` are run. **Not run on that host**, and left to the CI: `cargo test --workspace`, the `#[ignore]` suites (the real ROM, the reference firmwares), coverage, Docker and the ESP-IDF build of `deep_sleep_timer`.

## Values only the CI can produce

| Value | Where it is pinned | Why it is not known here | Step that reads it |
|---|---|---|---|
| `SHA256SUMS.inputs-deep-sleep-timer` (bootloader, partition table, `deep_sleep_timer.bin`) | `tools/reference-firmware/` | a new build: the first one prints the three hashes under `unpinned SHA256SUMS.inputs-deep-sleep-timer:` and stops before writing anything; the job `reference-boot` is red until they are pinned | Task 8, Step 6 |
| `DEEP_SLEEP_TIMER_TRACE` (`trace-hash not pinned yet` until then) | `crates/emu-cli/tests/run_cli.rs` | the first run of the image; the test fails and prints the hash of its run | Task 8, Step 7 |
| The milliseconds `deep_sleep_timer` prints for each sleep | none: the test reads them as a range (2 000 to 2 999) | they include the boot of the next life, which the uncalibrated timing model sets | — |

## Global Constraints

- Toolchain pinned: Rust **1.99.0**, edition **2024**. `unsafe_code = "forbid"`; `unwrap_used`/`expect_used` denied outside tests.
- No `TODO`, no placeholder in code but `DEEP_SLEEP_TIMER_TRACE`, named as such, whose test prints the measured value when it fails.
- No third-party package added; `cargo deny check` unchanged.
- Register addresses and names come from `svd-regs`; field positions from ESP-IDF v5.5.2 `rtc_cntl_reg.h` and `system_reg.h`, cited where they are used.
- **No loop or buffer sized by a firmware-written value without a constant bound.** The CRC unit reads `ADDR`/`LEN` (11 bits each) and clamps the range to the 8 KiB of RTC fast memory. The sleep is skipped arithmetically: one jump of device time to the instant the RTC counter reaches its 48-bit target; nothing iterates on the target or the duration. Each step of a deep sleep counts as a sleep of the call (`SLEEP_SLICE`, `MAX_FRUITLESS_WAKES` bound it).
- Determinism (spec §3.1, F10): the instant of the wake is a function of the RTC counter and the target alone; a run cut into slices of 1, 7, 997 and 7 919 cycles gives the same consoles, trace hash, device time and cycles.
- `soc-esp32c3`, `firmware`, `boards`, `console-script`: no host I/O (clippy bans), and they build for `wasm32-unknown-unknown`.
- GUI-first (F9): the sleep state is read from the machine (`Esp32C3::deep_sleep`); `emu-cli` prints it.
- Each hardware or ESP-IDF behaviour a model relies on cites its source in a comment; an assumed one is marked `ASSUMPTION:` and names the experiment that settles it.
- Code, tests, commits, README and CLI cite the spec's decisions F1–F13 at most; the rulings P1–P13 of this plan are cited by this plan and the hand-over note only.
- Checking a command: redirect its output to a file, then read `$?`; never through a pipe.
- Commits: `type(scope): description`, no trailer, no attribution line.
- Documents for people (the hand-over note) are in French; README English then French; code, CLI, research and this plan in English.

## Review Focus

- **The split is a move.** Task 2 changes no behaviour: its commit touches no test assertion, and every test of `rtc_cntl` and `timg` passes unchanged before and after.
- **Deep against light.** `SLEEP_EN` with `DIG_PWC.DG_WRAP_PD_EN` set sleeps; clear, it is refused (`Unsupported::LightSleep`) and rejected (`INT_RAW` bit 1, `SLEEP_EN` back to 0) → `light_sleep_is_refused_by_name_and_rejected_as_the_chip_rejects_it`. Mutation: drop the `DG_WRAP_PD_EN` test → the light-sleep test fails.
- **The instant of the wake.** `target - counter` modulo 2^48, a target already passed (half the range) waking at once → `the_rtc_timer_alarm_wakes_the_chip_at_its_target_as_a_core_reset_of_cause_5`, `an_alarm_already_reached_wakes_at_the_write_of_sleep_en`. Mutation: compare without the mask → the wrap test fails.
- **What survives.** `STORE0..7`, the RTC timer, the RTC memory, `RTC_CNTL` registers; `RESET_STATE` 5, `SLP_WAKEUP_CAUSE` 8 → `a_deep_sleep_wake_keeps_the_rtc_domain`.
- **The digital domain does nothing while asleep.** A main watchdog running at the entry does not reset the sleeping chip → `a_running_main_watchdog_does_not_reset_a_chip_in_deep_sleep` (machine). Mutation: deliver every due wake-up while asleep → it fails.
- **Determinism.** `a_deep_sleep_run_is_the_same_whatever_its_calls` (slices 1, 7, 997, 7 919).
- **The CRC unit.** `the_rtc_memory_crc_unit_answers_at_once_over_the_words_asked` (machine, through the bus). Mutation: drop the clamp → a range past the end panics.
- **The table.** No row has the heir SP1b-3f; `accepted_without_effect` is regenerated with `INKEMU_BLESS=1`; `PLANS_TO_COME` loses SP1b-3f and gains SP4.
- **Counts of the CI.** `reference-boot`'s `run_cli reference_firmware` goes from 10 to 11 (`tools/ci/run-ignored-tests.sh`).

---

## Design decisions taken for this plan

F1–F13 bind it. These were taken while writing it.

| # | Decision | Where | Cost if wrong |
|---|---|---|---|
| P1 | `Event::RtcWatchdog` becomes `Event::RtcCntl`: the one wake-up of the block now serves its watchdog, its super watchdog and the alarm of its timer, worked out together by `evaluate`. | Task 3 | Low |
| P2 | While the chip is in deep sleep only `RTC_CNTL` is called: the machine jumps to the instant of its next call and calls it alone. The wake-ups of the digital devices that fall due meanwhile are delivered after the core reset of the wake, to rebuilt devices that find nothing to do (the contract of `Event`). A reset `RTC_CNTL` asks for while the chip sleeps (the RTC watchdog left running without `PAUSE_IN_SLP`, the super watchdog) ends the sleep as that reset. | Task 5 | Medium |
| P3 | A deep sleep nothing the model has can end returns `StopReason::DeepSleep` at once, device time not moved (as `Idle`); the CLI ends with `RunEnd::DeepSleep`, exit code 0 (the firmware turned the device off, which is no failure), and a line that says what the firmware waits for. A run that ends on a limit while the chip sleeps says so too, with the instant of the wake. | Tasks 5, 7 | Low |
| P4 | `MAIN_TIMER_ALARM_EN` is write-only (`rtc_cntl_reg.h:167`): the alarm stays enabled until the next write of `SLP_TIMER1`, raises `MAIN_TIMER_INT_RAW` (bit 10) once when the counter reaches the target, and an alarm already reached when `SLEEP_EN` is written wakes the chip at once (DS §5, spec §5). | Task 4 | Low |
| P5 | The CRC unit is CRC-32/ISO-HDLC over the words `ADDR..=ADDR+LEN` of RTC fast memory, little-endian, clamped to its 2 048 words, computed at the write that sets `START`; `FINISH` reads 1 from then until a write clears `START`. `ASSUMPTION:` (F3) with the experiment that settles it. | Task 4 | Low: the ROM and ESP-IDF read the same unit |
| P6 | Wake sources other than the RTC timer are named at the entry (`Unsupported::SleepWakeSource`, GPIO named as sub-project 2's); the sleep goes on, with the timer as its only possible wake. | Task 4 | Low |
| P7 | `SLP_REJECT_CONF` is accepted storage, heir SP2: rejection needs a reject source (GPIO, UART), and the model has none to raise. | Task 6 | Low |
| P8 | Heirs of the ten SP1b-3f rows: `OPTIONS0`, `PWC`, `SYSTEM.MEM_PD_MASK` → SP4 (no observable effect without a power model, or part of the light sleep the model refuses); `TIMER1`, `TIMER4`, `TIMER5`, `TIMER6` → SP8 (wake-up latency and minimum sleep, a matter of measurement); `TIMER3` → SP5a (radio); `DIG_ISO` → SP2 (pads); `DIG_PWC` leaves the table (`DG_WRAP_PD_EN` acts). New rows written on the deep-sleep path: `BIAS_CONF`, `SDIO_CONF` (SP4), `GPIO_WAKEUP`, `EXT_WAKEUP_CONF`, `SLP_REJECT_CONF` (SP2). SP4 joins `PLANS_TO_COME`, SP1b-3f leaves it. | Task 6 | Low |
| P9 | `STATE0` bits `SW_CPU_INT`, `SLP_WAKEUP`, `SLP_REJECT` written 1 are refused by name (`Unsupported::SleepSignal`); `TIME_UPDATE` bits 29:27 are R/W (`rtc_cntl_reg.h:187-204`), stored and read back, and a 1 in any is refused by name (`Unsupported::RtcTimerTrigger`, F7). | Task 4 | Low |
| P10 | The wake clears the PC `ASSIST_DEBUG` recorded (the digital domain had no power, DS §3.2); internal SRAM keeps its bytes. Both are `ASSUMPTION:`s with their experiments. | Task 5 | Low |
| P11 | The RTC watchdog pauses while the chip sleeps when `WDTCONFIG0.PAUSE_IN_SLP` (bit 9, reset 1) is set (`rtc_cntl_reg.h:1640`; TRM §9.7). | Task 4 | Low |
| P12 | `deep_sleep_timer` is run with `--console uart0 --max-time 30` behind the shipped bootloader (`--firmware`, device `x4`): three lives of 2 s and a fourth that sleeps with no wake source end the run as `DeepSleep` long before 30 s. | Task 8 | Medium: first CI run |
| P13 | A UART byte still in flight at the entry leaves at the wake's reset, where the chip's powered-down UART would lose it; ESP-IDF waits for the UARTs to be idle before it sleeps (`flush_uarts`). `ASSUMPTION:` on the sleep. | Task 5 | Low |

## File Structure

```
.github/workflows/ci.yml                               reference-boot: INKEMU_DEEP_SLEEP_TIMER_APP, run_cli count 10 → 11 (Task 8)
tools/ci/run-ignored-tests.sh                          the count (Task 8)
README.md                                              deep sleep, status, roadmap, exit codes (Task 9)
docs/research/esp32c3-deep-sleep.md                    new (Task 1)
docs/registers-accepted-without-effect.md              regenerated, prose (Task 6)
docs/superpowers/notes/2026-10-14-sp1b-3f-carryover.md new, French (Task 9)
tools/reference-firmware/                              deep_sleep_timer/ (new), build.sh, in-container.sh, README (Task 8)
crates/soc-esp32c3/src/periph/rtc_cntl/{mod,timer,wdt,swd,sleep}.rs   split (Task 2), sleep (Task 4)
crates/soc-esp32c3/src/periph/timg/{mod,calibration}.rs               split (Task 2)
crates/soc-esp32c3/src/periph/system.rs                CRC unit (Task 4)
crates/soc-esp32c3/src/periph/mod.rs                   deep-sleep reset, CRC event (Tasks 4, 5)
crates/soc-esp32c3/src/{ctx,reset,diag,bus,machine,lib}.rs   (Tasks 3–5)
crates/soc-esp32c3/tests/accepted_without_effect.rs    PLANS_TO_COME (Task 6)
crates/console-script/src/run.rs, crates/soc-esp32c3/src/test_support.rs   the new stop reason (Task 5)
crates/emu-cli/src/{run,exit}.rs                       RunEnd::DeepSleep, the end line (Task 7)
crates/emu-cli/tests/run_cli.rs                        deep_sleep_timer (Task 8)
```

---

## Task 1: The research report

**Files:** create `docs/research/esp32c3-deep-sleep.md`.

- [ ] **Step 1:** Copy the substance of the two reports of this sub-project (ESP-IDF v5.5.2 and ROM rev3, register by register; the state of inkemu's model, the real firmwares, the QEMU oracle) into one English file, parts A and B, keeping their M/R tags and sources.
- [ ] **Step 2:** Commit: `docs(research): ESP32-C3 deep sleep under ESP-IDF v5.5.2 and the ROM, register by register, and what inkemu's model lacked`.

## Task 2: Split `rtc_cntl.rs` and `timg.rs` (F8), a pure move

**Files:** `crates/soc-esp32c3/src/periph/rtc_cntl.rs` → `rtc_cntl/{mod,timer,wdt,swd}.rs`; `timg.rs` → `timg/{mod,calibration}.rs`.

- [ ] **Step 1:** `git mv` both files to `mod.rs` of their directory. Move, without changing a line of their bodies: the RTC timer (`rtc_time`, the `TIME_*` handling) to `timer.rs`; the RTC watchdog (`flash_boot_protection`, `stage_action`, `stage_timeout`, `walk_stages`, `write_watchdog` and their constants) to `wdt.rs`; the super watchdog (`swd_armed`, `step_super_watchdog`, `write_super_watchdog`, its constants) to `swd.rs`; the slow-clock calibration (`crystal_cycles_per_slow_cycle`, `Measurement`, `Calibration`, the `CALI_*` constants) to `timg/calibration.rs`. Child modules reach the private fields of the parent's struct; constants become `pub(super)`. The tests stay in `mod.rs`.
- [ ] **Step 2:** Run `cargo test -p soc-esp32c3 --lib -- rtc_cntl timg` → the same counts as before the move.
- [ ] **Step 3:** Commit: `refactor(soc-esp32c3): RTC_CNTL is split into its timer, its watchdog and its super watchdog, and the slow-clock calibration leaves timg.rs, without a change of behaviour`.

## Task 3: The wake-up of `RTC_CNTL`, the cause 5, the refusals (P1, P9)

**Files:** `ctx.rs`, `reset.rs`, `diag.rs`, `periph/mod.rs`, `periph/rtc_cntl/mod.rs`.

- [ ] **Step 1:** `Event::RtcWatchdog` → `Event::RtcCntl`. `ResetCause::DeepSleep` (code 0x05, level Core, `DSLEEP` for the ROM; TRM table 6.1-1). `Diagnostic::Reset` of that cause reads `woken from deep sleep by the RTC timer (rst:0x5), a core reset; the CPU had stopped at pc …, sp …` (F2). `Unsupported::LightSleep { dig_pwc }`, `SleepWakeSource { wakeup_ena }`, `SleepSignal { state0 }`, `RtcTimerTrigger { time_update }`: keys, messages (cause, then remedy), the lists of the diagnostics tests.
- [ ] **Step 2:** `Ctx::enter_deep_sleep`/`take_deep_sleep_entered`, part of `is_flagged`.
- [ ] **Step 3:** Run the tests of `reset`, `diag`, `ctx` → pass.

## Task 4: The sleep of `RTC_CNTL` and the CRC unit (F1–F5, F7, F12; P4–P6, P9, P11)

**Files:** create `periph/rtc_cntl/sleep.rs`; modify `rtc_cntl/{mod,timer,wdt}.rs`, `periph/system.rs`, `periph/mod.rs`, `bus.rs`.

- [ ] **Step 1: Write the failing tests** (in `sleep.rs`): the alarm wakes the sleeping chip at its target as a reset of cause 5, with `SLP_WAKEUP_CAUSE` 8 and `INT_RAW` bits 0 and 10; an alarm already reached wakes at the write; light sleep refused and rejected; a GPIO source named and the sleep entered without a wake; no source, no wake; `TIME_UPDATE` 29:27 stored and refused; the `STATE0` signals refused; the RTC watchdog paused in sleep and not without `PAUSE_IN_SLP`; the alarm raises bit 10 once while awake; every register reads back what was written, write-only bits 0.
- [ ] **Step 2: Code.** `SLP_TIMER0/1`, `STATE0`, `WAKEUP_STATE`, `SLP_WAKEUP_CAUSE`, `SLP_REJECT_CAUSE`, `DIG_PWC` in `sleep.rs`; `evaluate` adds the alarm to the instants it works out; `RtcCntl::reset` ends any sleep; `deep_sleep()` and `next_call()` for the machine. `Deadline::due`.
- [ ] **Step 3:** The CRC unit: `SYSTEM.RTC_FASTMEM_CONFIG`/`RTC_FASTMEM_CRC`; a write that sets `START` posts `Event::RtcMemoryCrc`, which the bus answers from the RTC memory (`settle_signals`).
- [ ] **Step 4: Run** `cargo test -p soc-esp32c3 --lib -- rtc_cntl system` → pass.

## Task 5: The machine asleep (F1, F2, F5, F9, F10; P2, P3, P10, P13)

**Files:** `machine.rs`, `periph/mod.rs`, `lib.rs`, `test_support.rs`, `crates/console-script/src/run.rs`.

- [ ] **Step 1: Write the failing tests** (`machine.rs`): a program that counts its lives in `STORE0`, prints the count, and sleeps on the RTC timer three times then with no source; `a_deep_sleep_run_is_the_same_whatever_its_calls` (one call, then slices of 1, 7, 997, 7 919 cycles: consoles, trace hash, device time, cycles); `a_deep_sleep_nothing_can_end_stops_the_run` (`StopReason::DeepSleep`, device time not moved); `a_deep_sleep_cut_by_the_time_limit_is_still_asleep` (`TimeBudget`, `deep_sleep()` gives the instant of the wake); `a_running_main_watchdog_does_not_reset_a_chip_in_deep_sleep`; `light_sleep_stops_a_strict_run`; `the_rtc_memory_crc_unit_answers_at_once_over_the_words_asked`.
- [ ] **Step 2: Code.** `powered_down` in the machine; `sleep_deeply`; `reset` of cause 5 clears `ASSIST_DEBUG`'s record; `Esp32C3::deep_sleep() -> Option<DeepSleep>`; `StopReason::DeepSleep` in `drive` and in the run loop of `console-script`.
- [ ] **Step 3: Run** `cargo test -p soc-esp32c3 --lib -- machine::tests::a_deep machine::tests::light machine::tests::the_rtc_memory`, `cargo test -p console-script --lib` → pass.
- [ ] **Step 4: Commit** Tasks 3–5: `feat(soc-esp32c3): deep sleep: SLEEP_EN with the digital domain powered down stops the CPU and device time jumps to the RTC timer's alarm, the chip wakes as a core reset of cause 5 with the RTC domain kept, the RTC memory CRC unit answers, light sleep and wake sources other than the timer are refused by name`.

## Task 6: The table of registers accepted without effect (F6; P7, P8)

**Files:** `periph/rtc_cntl/mod.rs`, `periph/system.rs`, `tests/accepted_without_effect.rs`, `docs/registers-accepted-without-effect.md`.

- [ ] **Step 1:** Rows as P8 says; `PLANS_TO_COME` loses SP1b-3f and gains SP4; the prose of the document names the heirs.
- [ ] **Step 2:** `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect`, then without the variable, and `cargo test -p soc-esp32c3 --lib -- every_row_of_the_table` → pass; `grep -c SP1b-3f docs/registers-accepted-without-effect.md` → 0.
- [ ] **Step 3: Commit:** `feat(soc-esp32c3): no register is handed to SP1b-3f any more: the power and timing rows go to SP4, SP5a and SP8, the pads and wake configuration to SP2, and DIG_PWC leaves the table`.

## Task 7: The CLI (F2, F5, F9; P3)

**Files:** `crates/emu-cli/src/run.rs`, `exit.rs`.

- [ ] **Step 1: Write the failing tests:** `RunEnd::DeepSleep` exit 0 and in `ALL`; the end line of a run stopped asleep names the sources it waits for; a run that ends on its limit while asleep says when the timer wakes the chip.
- [ ] **Step 2: Code** and **Run** `cargo test -p emu-cli --lib run` → pass.
- [ ] **Step 3: Commit:** `feat(emu-cli): a run that ends with the device in deep sleep says since when and what it waits for, and one nothing can wake ends with exit code 0`.

## Task 8: `deep_sleep_timer`, the proof in the CI (F11; P12)

**Files:** create `tools/reference-firmware/deep_sleep_timer/{CMakeLists.txt, main/CMakeLists.txt, main/deep_sleep_timer_main.c}`; modify `tools/reference-firmware/{build.sh, in-container.sh, README.md}`, `.github/workflows/ci.yml`, `tools/ci/run-ignored-tests.sh`, `crates/emu-cli/tests/run_cli.rs`.

- [ ] **Step 1: The application.** `RTC_DATA_ATTR` life counter and time of entry; at each life: the life, the reset reason and the wake cause by name, the milliseconds slept (`gettimeofday`, kept across the sleep by ESP-IDF); lives 1 to 3 sleep 2 s on the RTC timer, life 4 sleeps with no wake source.
- [ ] **Step 2:** No `sdkconfig.defaults` of its own: the reference one; `CONFIG_ESP_SYSTEM_ALLOW_RTC_FAST_MEM_AS_HEAP` keeps its default `y`, so ESP-IDF enters through `rtc_deep_sleep_start` and the CRC unit.
- [ ] **Step 3: Build wiring.** `in-container.sh` adds `/ref/deep_sleep_timer:SHA256SUMS.inputs-deep-sleep-timer` to the application loop; `build.sh` its comment and its output line; the README of `tools/reference-firmware` its row and its variable. No `SHA256SUMS.inputs-deep-sleep-timer` is committed.
- [ ] **Step 4: The test** `x4_runs_the_deep_sleep_timer_reference_firmware_through_three_wakes_into_a_sleep_without_end` (ignored, `INKEMU_ROM_ELF_DIR`, `INKEMU_DEEP_SLEEP_TIMER_APP`): `--console uart0 --max-time 30 --trace-hash --firmware`; exit 0; in order `rst:0x1`, life 1, three times `rst:0x5 (DSLEEP),boot:0x8 (SPI_FAST_FLASH_BOOT)` then the life, the deep-sleep reset reason, the timer and between 2 000 and 2 999 ms slept, then `sleeping with no wake source`; exactly four `rst:` lines; the journal's wake line; the end line; nothing undocumented, no warning; the trace hash against `DEEP_SLEEP_TIMER_TRACE`.
- [ ] **Step 5: CI.** `reference-boot`: `INKEMU_DEEP_SLEEP_TIMER_APP`, `run_cli reference_firmware` 10 → 11.
- [ ] **Step 6 (CI):** the first build prints `unpinned SHA256SUMS.inputs-deep-sleep-timer:` and three lines: write them to that file, push, and check that the next (clean) build passes `sha256sum -c`.
- [ ] **Step 7 (CI):** the test then fails on `DEEP_SLEEP_TIMER_TRACE` only, printing the hash of the run: check the console and the log in the failure message, pin the hash, push.
- [ ] **Step 8: Commit** (Steps 1–5): `feat(tools): deep_sleep_timer, ours, counts its lives in RTC memory through three deep sleeps on the RTC timer and a fourth without a wake source, and run_cli holds them in the CI`.

## Task 9: Documents (F13)

**Files:** `README.md`; create `docs/superpowers/notes/2026-10-14-sp1b-3f-carryover.md`.

- [ ] **Step 1:** README (EN, then FR): deep sleep and the wake by the RTC timer, light sleep and the other wake sources refused, the end line and exit code of a run that ends asleep; the status and the roadmap.
- [ ] **Step 2:** The hand-over note (French): results, the values the CI must pin, the assumptions added, P1–P13, what is left to each heir (SP2: GPIO wake, rejection, pad isolation; SP4: power; SP8: the latency of the wake, the CRC polynomial, `Saved PC`), the limits of proof.
- [ ] **Step 3: Commit:** `docs: the README says how inkemu sleeps and wakes, and the hand-over of SP1b-3f`.
