# SP1b-3e — Two real X4 firmwares start — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** criterion 2 of the spec (§10.3) for the two X4 firmwares the user has: `inkemu run --device x4 --firmware crosspoint-1.6.5-x3-x4.bin --max-time 5` shows, without a console option, the ROM, the bootloader and CrossPoint's own lines, and every first access to a block of a later sub-project (GPIO, I2C0, SPI2, SAR ADC) is a warning that names its register; the stock firmware of the X4 (a dump given to `--flash`) shows its ROM lines and its panics the same way, with no access reported as "undocumented". Checked by two local tests on the user's machine, and in the CI by a firmware of this repository that takes the same path (a wait for the SAR ADC with interrupts masked, the interrupt watchdog's panic, a core dump, a second life).

**Architecture:**

| Crate | Owns | Why a GUI can call it |
|---|---|---|
| `boards` | the `console` key of a profile (`DeviceProfile::console`), its refusals (`ProfileError::MissingConsole`, a bad value at its line) | the console a front end opens first is a fact of the device, read from its profile |
| `soc-esp32c3` | SPI1 `+0x54` (`SPI_MEM_FSM_REG`) in `periph/spi_mem.rs`; the reworded `ASSUMPTION:` of `UsbSerialJtag::reset` | no host I/O, unchanged API |
| `emu-cli` | `--console` optional, the console of the profile otherwise, the first line naming the console and its origin (`RunRequest`, `shown_console`, `console_note`); the local tests of the real firmwares (`tests/real_firmware.rs`); the CI test of `adc_wait_panic` (`tests/run_cli.rs`) | option parsing and printing only |
| `tools/reference-firmware` | `adc_wait_panic/` (ours, Apache-2.0), built and pinned like `usb_serial_jtag_console` | — |

**Tech Stack:** Rust 1.99.0 (edition 2024); no new crate, no new dependency. ESP-IDF v5.5.2 (`tools/conformance/versions.env`) in the pinned Docker image, which only the CI runs.

**Spec:** `docs/superpowers/specs/2026-10-14-sp1b-3e-design.md` (decisions E1–E10, binding). Global spec `docs/superpowers/specs/2026-10-11-inkemu-design.md` (§3.3 profiles, §7 errors, §10.3 criterion 2). Facts: `docs/research/spi1-fsm-and-usj-reset.md` (written by Task 1 from the two research reports of this sub-project), `docs/research/flash-writes-and-crosspoint-boot.md` (**FW**, §K).

**What was verified, and where.** Every task was implemented and committed on this branch, in the order of the tasks; the commit of a task is the reference for its code, and the blocks below are the load-bearing parts of it. On the development host, by the rule of the project, only `cargo fmt --all --check`, `tools/ci/clippy.sh -p soc-esp32c3 -p boards -p emu-cli --all-targets --all-features -- -D warnings` and narrow `cargo test -p <crate> <filters>` were run, with `CARGO_BUILD_JOBS=4`. **Not run on that host**, and left to the CI or to the user's machine: `cargo test --workspace`, the `#[ignore]` suites, coverage, Docker and the ESP-IDF build of `adc_wait_panic`, and the two tests of the real firmwares, which run on the user's machine only.

## Values only the CI or the user's machine can produce

| Value | Where it is pinned | Why it is not known here | Step that reads it |
|---|---|---|---|
| `SHA256SUMS.inputs-adc-wait-panic` (bootloader, partition table, `adc_wait_panic.bin`) | `tools/reference-firmware/` | a new build: the first one prints the three hashes under `unpinned SHA256SUMS.inputs-adc-wait-panic:` and stops before writing anything (README "Adding a firmware"); the job `reference-boot` is red until they are pinned | Task 7, Step 6 |
| `ADC_WAIT_PANIC_TRACE` (`trace-hash not pinned yet` until then) | `crates/emu-cli/tests/run_cli.rs` | the first run of the image; the test fails and prints the hash of its run in its message | Task 7, Step 7 |
| The lines of the two real firmwares | `crates/emu-cli/tests/real_firmware.rs` | measured by the controller on 2026-10-14 (spec §2); the files are on the user's machine only | Task 6, Step 3 |

## Global Constraints

- Toolchain pinned: Rust **1.99.0**, edition **2024**. `unsafe_code = "forbid"`; `unwrap_used`/`expect_used` denied outside tests.
- No `TODO`, no placeholder in code but the one value a test cannot know before the CI (`ADC_WAIT_PANIC_TRACE`), which is named as such and whose test prints the measured value when it fails.
- No third-party package added; `cargo deny check` unchanged.
- Register addresses and names come from `svd-regs`; the one exception is SPI1 `+0x54`, which the SVD omits: its offset is that of SPI0 `FSM` (`addr("SPI0", "FSM") - SPI0_BASE`), and the one test that writes `0x6000_2054` out asserts that the SVD has no register there.
- **No loop or buffer sized by a firmware-written value without a constant bound.** The new register stores five bits of a word; nothing iterates on it. The console of a run is chosen from three values.
- Every user-facing message states the cause, then the remedy (`ProfileError::MissingConsole`, the refusal of a bad `console`, the failures of `real_firmware.rs`).
- Determinism: the console shown does not enter the trace; no trace hash moves (E2).
- `soc-esp32c3`, `firmware`, `boards`, `console-script`: no host I/O, and they build for `wasm32-unknown-unknown`.
- GUI-first: the default console is profile data in `boards`; `emu-cli` only picks between it and `--console`.
- Each hardware or ESP-IDF behaviour a model relies on cites its source in a comment; an assumed one is marked `ASSUMPTION:` (106 lines at the base, 107 at the end).
- Code, tests, commits, README and CLI cite the spec's decisions E1–E10 at most; the decisions F1–F8 of this plan are cited by this plan and the hand-over note only.
- Third-party binaries (CrossPoint, the stock dump) are never committed, downloaded or run outside the user's machine; their tests never print a byte of the flash.
- Checking a command: redirect its output to a file, then read `$?`; never through a pipe.
- Commits: `type(scope): description`, no trailer, no attribution line.
- Documents for people (the hand-over note, the corrections of the 3c note) are in French; README English then French; code, CLI, research and this plan in English.

## Review Focus

- **A profile without, or with a wrong, `console`.** Refused at load with the three values; a bad value at its line → `a_profile_without_a_console_is_refused_with_the_values_it_takes`, `an_unknown_console_is_refused_at_its_line_with_the_values_it_takes`. Mutation: `let console = raw.console.ok_or_else(...)?;` → `let console = raw.console.unwrap_or(ConsoleChannel::Uart0);` fails the first.
- **The default and its origin.** `the_console_shown_is_the_profile_s_unless_one_is_asked_for` (main.rs), `the_xteink_devices_are_read_on_usb_serial_jtag` (boards). Mutation: `asked.map_or((profile.console, ...` → `(ConsoleChannel::Uart0, ...` fails it.
- **The help and the README.** `the_readme_shows_run_help_as_the_binary_prints_it` holds the new help of `--console`.
- **SPI1 `FSM`.** Read idle, write keeps [11:7] only, reset gives `0x200`; no diagnostic through the bus in strict mode → `spi1_fsm_reads_idle_and_keeps_only_its_lock_delay`, `spi1_fsm_is_read_idle_without_a_diagnostic_under_strict_mmio`. Mutations: `self.fsm = value & FSM_LOCK_DELAY;` → `self.fsm = value;`; delete the `if offset == SPI1_FSM` branch of `read`.
- **The strict stop of a real firmware is its first unmodelled access.** `real_firmware.rs` compares the strict stop with the first warning of the lenient run (CPU exceptions passed over, since they do not stop a strict run).
- **Counts of the CI.** `reference-boot`'s `run_cli reference_firmware` goes from 9 to 10; `rom-boot`'s `run_cli --skip reference_firmware` stays 28 (the new test has `reference_firmware` in its name); `real_firmware` is not run by any job.

---

## Design decisions taken for this plan

E1–E10 bind it. These were taken while writing it.

| # | Decision | Where | Cost if wrong |
|---|---|---|---|
| F1 | The console note goes into the first line between the timing model and the flash note: `…; timing model … (not calibrated); console: usb-serial-jtag (the default of the device profile; --console shows another); flash: …`, or `console: uart0 (--console)`. The flash note stays last because tests read it with its newline. | Task 5 | Low |
| F2 | `console` is a top-level key, read by a deserializer that gives the line of a bad value; its absence is its own refusal (`ProfileError::MissingConsole`), like `[flash]`. The values are listed sorted and quoted as the key is written. | Task 4 | Low |
| F3 | The tests of `run_cli.rs` that boot pass `--console uart0`: inline in the argument lists, and through `on_uart0` for the commands built by `run_of`/`run_with_file`, two of whose callers pass `--console uart1` and `--console usb-serial-jtag` themselves (clap refuses a repeated option). Commands refused before boot are left as they are. | Task 5 | Low |
| F4 | The verified README examples (`hello_world`, NVS) pass `--console uart0` and their `inkemu:` lines say so; "Build & try" and the `--firmware` example show the default. | Task 5 | Low |
| F5 | `real_firmware.rs` needs `INKEMU_ROM_ELF_DIR` like every test of the real ROM, and never downloads. One test per firmware, each with its lenient and its strict run. | Task 6 | Low |
| F6 | The strict stop of each real firmware must equal the first warning of its lenient run: "the first unmodelled access" is then held by the run itself, not by reading a log. | Task 6 | Low |
| F7 | `adc_wait_panic` is run with `--console uart0`: its ESP-IDF console is UART0, USB-Serial-JTAG secondary; the panic handler writes to both. It reads `I2C0.SR` (the SVD reset value `0x0000c000` is what the model returns) and checks four named warnings. `--max-time 10`: the 300 ms of the interrupt watchdog, the core dump and the second life fit with room. | Task 7 | Medium: first CI run |
| F8 | The partition table of `adc_wait_panic` is a copy of `tests/fixtures/partitions/x4-stock.csv` inside the project (`partitions.csv`), since the container copies the project alone; the X4 profile writes the same table at 0x8000 when it boots the application. | Task 7 | Low |

## File Structure

```
.github/workflows/ci.yml                          reference-boot: INKEMU_ADC_WAIT_PANIC_APP, run_cli count 9 → 10 (Task 7)
README.md                                         --console, help, examples, real firmwares, status (Tasks 5, 8)
docs/research/spi1-fsm-and-usj-reset.md           new (Task 1)
docs/research/flash-writes-and-crosspoint-boot.md correction appended, §K (Task 8)
docs/superpowers/notes/2026-10-13-sp1b-3c-carryover.md   correction appended (Task 8)
docs/superpowers/notes/2026-10-14-sp1b-3e-carryover.md   new, French (Task 8)
tools/reference-firmware/                         adc_wait_panic/ (new), build.sh, in-container.sh, README (Task 7)
crates/soc-esp32c3/src/periph/spi_mem.rs          SPI1 FSM (Task 2)
crates/soc-esp32c3/src/machine.rs                 its test through the bus (Task 2)
crates/soc-esp32c3/src/periph/usb_serial_jtag.rs  the ASSUMPTION of reset (Task 3)
crates/boards/src/lib.rs, profiles/x3.toml, profiles/x4.toml   console (Task 4)
crates/emu-cli/src/main.rs                        default console, first line (Task 5)
crates/emu-cli/tests/run_cli.rs                   --console uart0 (Task 5); adc_wait_panic (Task 7)
crates/emu-cli/tests/real_firmware.rs             new (Task 6)
```

---

## Task 1: The research report

**Files:** create `docs/research/spi1-fsm-and-usj-reset.md`.

- [ ] **Step 1:** Copy the substance of the two reports of this sub-project (SPI1 offset `0x54`; the USB-Serial-JTAG tail across a core reset) into one English file, sections A and B, keeping their M/R tags and sources.
- [ ] **Step 2:** Add §B.6, the ruling on E4 made by the controller: "undecidable without hardware" → the model is kept, its `ASSUMPTION:` names the experiment that settles it.
- [ ] **Step 3:** Commit: `docs(research): SPI1 +0x54 is SPI_MEM_FSM_REG, read idle by ESP-IDF 4.4, and whether a core reset loses the USB-Serial-JTAG tail is undecidable without a board`.

## Task 2: SPI1 `+0x54` is `SPI_MEM_FSM_REG` (E3)

**Files:** modify `crates/soc-esp32c3/src/periph/spi_mem.rs`, `crates/soc-esp32c3/src/machine.rs`.

**Bounds on untrusted input.** A write stores `value & (0x1F << 7)`; nothing else depends on it.

- [ ] **Step 1: Write the failing tests.** In `spi_mem.rs`, `spi1_fsm_reads_idle_and_keeps_only_its_lock_delay` (reset `0x200`, bits [6:4] 0, idle after a `RDSR`, a write of `0xFFFF_FFFF` reads `0xF80`, a write of `0x7F | 9 << 7` reads `9 << 7`, no diagnostic, `reset` gives `0x200` back). In `machine.rs`, `spi1_fsm_is_read_idle_without_a_diagnostic_under_strict_mmio` (the SVD has no register at `0x6000_2054`; a strict run that loads it runs to its budget with no diagnostic; the bus reads `0x200`).
- [ ] **Step 2: Code.**

```rust
const SPI1_FSM: u32 = SPI0_FSM;          // documented in the commit: spi_mem_reg.h, report §A.1, §A.2
const FSM_LOCK_DELAY: u32 = 0x1F << 7;   // CSPI_LOCK_DELAY_TIME [11:7], the one writable field
// Spi1 gains `fsm: u32`, `fsm_reset_value()` (the SVD value of SPI0 FSM, 0x200), set by `new` and `reset`.
// read:  if offset == SPI1_FSM { return Some(self.fsm); }   // ASSUMPTION: same register as SPI0's
// write: if offset == SPI1_FSM { self.fsm = value & FSM_LOCK_DELAY; return true; }
```

- [ ] **Step 3: Run** `cargo test -p soc-esp32c3 --lib -- spi1_fsm spi_mem` → all pass.
- [ ] **Step 4: Mutations:** `self.fsm = value & FSM_LOCK_DELAY;` → `self.fsm = value;` fails the unit test; delete the read branch → both tests fail (the bus reports an undocumented read).
- [ ] **Step 5: Commit:** `feat(soc-esp32c3): SPI1 +0x54 is SPI_MEM_FSM_REG, read idle with its lock delay, which ESP-IDF 4.4 polls before each status read of the flash`.

## Task 3: The `ASSUMPTION:` of the USB-Serial-JTAG reset (E4)

**Files:** modify `crates/soc-esp32c3/src/periph/usb_serial_jtag.rs` (doc of `UsbSerialJtag::reset`).

- [ ] **Step 1:** Replace the `ASSUMPTION:` text with the one of report §B.6: what a core and a CPU reset do here, TRM §6.1.3 and §30.3.4, the panic handler of ESP-IDF and `esp_restart_noos_dig`, the verdict (undecidable without a chip), and the experiment that settles it. No code changes; `a_core_reset_empties_the_block_and_the_host_goes_on` keeps holding the behaviour.
- [ ] **Step 2: Commit:** `docs(soc-esp32c3): the assumption that a core reset resets USB-Serial-JTAG cites the TRM sentence that leans the other way and the capture that settles it`.

## Task 4: The `console` key of a profile (E1, E9)

**Files:** modify `crates/boards/src/lib.rs`, `crates/boards/profiles/x3.toml`, `crates/boards/profiles/x4.toml`.

- [ ] **Step 1: Write the failing tests** (in `lib.rs`): `the_xteink_devices_are_read_on_usb_serial_jtag`, `each_console_of_the_chip_can_be_named_by_a_profile`, `a_profile_without_a_console_is_refused_with_the_values_it_takes` (exact message: `demo.toml: the profile has no console key; add one before the first table, the console shown when none is asked for: console = "uart0", "uart1" or "usb-serial-jtag"`), `an_unknown_console_is_refused_at_its_line_with_the_values_it_takes` (`"uart2"`, `"USB-Serial-JTAG"`, `""` at line 3 with the values; `0` with the general remedy). `VALID` gains `console = "uart0"` in place of its blank third line, so the line numbers the other tests cite stay.
- [ ] **Step 2: Code.** `DeviceProfile::console: ConsoleChannel`; `RawProfile::console: Option<ConsoleChannel>` with `#[serde(default, deserialize_with = "console")]`; `fn console` maps a string to `ConsoleChannel::ALL` by `name()`, else `the device has no console `…`; write console = "uart0", "uart1" or "usb-serial-jtag"`; `ProfileError::MissingConsole { origin }`; `from_toml` refuses a profile without the key after `[flash]`. The two profiles: `console = "usb-serial-jtag"`, with a comment citing FW §K.2.
- [ ] **Step 3: Run** `cargo test -p boards` → all pass.
- [ ] **Step 4: Commit:** `feat(boards): a profile names the console its device is read on, required and checked at load, usb-serial-jtag for the X3 and the X4`.

## Task 5: The default console of `inkemu run` (E1, E2, E9)

**Files:** modify `crates/emu-cli/src/main.rs`, `crates/emu-cli/tests/run_cli.rs`, `README.md`.

- [ ] **Step 1: Write the failing tests** (`main.rs`): `the_console_shown_is_the_profile_s_unless_one_is_asked_for` (x3, x4: `UsbSerialJtag` from the profile, then each channel from the option, with the exact notes), `console_is_optional_on_the_command_line`.
- [ ] **Step 2: Code.** `--console` becomes `Option<ConsoleArg>` with no default; its help says the profile names the console otherwise and that the first line says which. `RunRequest { max_cycles, max_device_nanos, console: Option<ConsoleChannel> }` with `resolve(&profile) -> (RunOptions, String)`; `shown_console`, `ConsoleOrigin`, `console_note`; `header` takes the note; `flash_note` becomes a function (the body of `run_device` stays under clippy's 100 lines). `profile_for` passes `MissingConsole` through.
- [ ] **Step 3: Tests that read UART0 say so** (F3): every booting command of `run_cli.rs` without `--console` gets `--console uart0`; `on_uart0` for the commands of `run_of`/`run_with_file` that read their console. README: the help block, the `--console` paragraph (EN, FR), the description of the first line, the verified examples (F4) with their `inkemu:` lines, and the `EXAMPLES` of `the_readme_shows_what_the_binary_prints_for_the_reference_firmware`.
- [ ] **Step 4: Run** `cargo test -p emu-cli --bin inkemu`, `cargo test -p emu-cli --test run_cli -- readme help console device`, `cargo test -p emu-cli --lib firmware_file` → all pass; `run_cli -- --ignored --list` counts 28 outside `reference_firmware`.
- [ ] **Step 5: Commit:** `feat(emu-cli): without --console a run shows the console its profile names and its first line says which and why, and the tests and README examples that read UART0 ask for it`.

## Task 6: The local tests of the two real firmwares (E5–E7)

**Files:** create `crates/emu-cli/tests/real_firmware.rs`.

- [ ] **Step 1:** `pinned_file(variable, example, sha256)`: a missing variable or file fails with the variable and the command that sets it; another SHA-256 fails with both hashes and "nothing was run". `rom_dir()` from `INKEMU_ROM_ELF_DIR`. `run(input, file, more)`: `inkemu run --device x4 --max-time 5 --rom-dir … <input> <file>`, no console option.
- [ ] **Step 2:** `Run` with `assert_in_order`, `assert_named_warning(peripheral)` (a `warning: unmodeled register …: P.` line), `assert_nothing_undocumented`, `first_warning` (CPU exceptions passed over); `assert_strict_stop(input, file, lenient, register, address)`: code 3, `inkemu: stopped by --strict-mmio: unmodeled register …: REGISTER at ADDRESS `, equal to the first warning of the lenient run (F6).
- [ ] **Step 3:** `crosspoint_1_6_5_x3_x4_starts_and_names_what_the_model_lacks` (`INKEMU_CROSSPOINT_X3_X4`, `--firmware`): the console note of the profile; in order the ROM banner, `rst:0x1`, the shipped bootloader, `[INF] [MAIN] Hardware detect: X4`, the panic of the interrupt watchdog, `rst:0xc`, `[SD] SD card not detected (`, `Wait complete:  CMD_SOFT_RESET (0 ms)`; the CPU reset in the log; warnings for GPIO, I2C0, SPI2, APB_SARADC; nothing undocumented; strict stop at `APB_SARADC.APB_TSENS_CTRL` `0x60040058`. `the_stock_x4_firmware_starts_and_names_what_the_model_lacks` (`INKEMU_STOCK_X4_FLASH`, `--flash`): banner, `rst:0x1`, panic, `rst:0x3`, panic again; the core reset in the log; warnings for GPIO and APB_SARADC; nothing undocumented; strict stop at `GPIO.PIN13` `0x600040a8`. The header says how to run them; they are in no job of the CI and in no count of `run-ignored-tests.sh`.
- [ ] **Step 4: Run** `cargo test -p emu-cli --test real_firmware` → `0 passed; 2 ignored` (they compile). On the user's machine: `cargo test --release -p emu-cli --test real_firmware -- --ignored` → 2 passed.
- [ ] **Step 5: Commit:** `test(emu-cli): CrossPoint 1.6.5 and the stock firmware of the X4, files of the user's checked by their SHA-256, start as the device shows them and name each register the model lacks, a strict run stopping at the first`.

## Task 7: `adc_wait_panic`, the same path in the CI (E8)

**Files:** create `tools/reference-firmware/adc_wait_panic/{CMakeLists.txt, sdkconfig.defaults, partitions.csv, main/CMakeLists.txt, main/adc_wait_panic_main.c}`; modify `tools/reference-firmware/{build.sh, in-container.sh, README.md}`, `.github/workflows/ci.yml`, `crates/emu-cli/tests/run_cli.rs`.

- [ ] **Step 1: The application.** `app_main`: `esp_core_dump_image_check()` and `esp_core_dump_image_get()` → `adc_wait_panic: a core dump is in flash at 0x…, … bytes` and return; else `adc_wait_panic: no core dump in flash (<err>)`, `adc_wait_panic: I2C0 SR 0x…` (`REG_READ(I2C_SR_REG(0))`), `REG_WRITE(APB_SARADC_INT_CLR_REG, APB_SARADC_ADC1_DONE_INT_CLR)`, `adc_wait_panic: waiting for the SAR ADC, interrupts masked`, then under `portENTER_CRITICAL`: `ONETIME_SAMPLE` = `APB_SARADC1_ONETIME_SAMPLE | 1 << ONETIME_CHANNEL_S | 3 << ONETIME_ATTEN_S`, then with `APB_SARADC_ONETIME_START`, and the loop on `APB_SARADC_INT_RAW_REG & APB_SARADC_ADC1_DONE_INT_RAW`. Names checked against the v5.5.2 headers (`soc/esp32c3/register/soc/apb_saradc_reg.h`, `i2c_reg.h`, `soc/esp32c3/include/soc/soc.h`, `espcoredump/include/esp_core_dump.h`); the critical section is that of `esp_adc/adc_oneshot.c`.
- [ ] **Step 2: Configuration** (F8): `sdkconfig.defaults` = `CONFIG_PARTITION_TABLE_CUSTOM=y`, `CONFIG_PARTITION_TABLE_CUSTOM_FILENAME="partitions.csv"`, `CONFIG_ESP_COREDUMP_ENABLE_TO_FLASH=y`, merged after the reference file by the existing rule; `partitions.csv` = the X4 table; `main/CMakeLists.txt` `PRIV_REQUIRES espcoredump`.
- [ ] **Step 3: Build wiring.** `in-container.sh` adds `/ref/adc_wait_panic:SHA256SUMS.inputs-adc-wait-panic` to the application loop; `build.sh` its comment and its output line; the README of `tools/reference-firmware` its row, its configuration, its variable, and where the real firmwares are tested. No `SHA256SUMS.inputs-adc-wait-panic` is committed.
- [ ] **Step 4: The test** `x4_runs_the_adc_wait_panic_reference_firmware_through_its_panic_into_a_second_life` (ignored, `INKEMU_ROM_ELF_DIR`, `INKEMU_ADC_WAIT_PANIC_APP`): the image checked against `SHA256SUMS.inputs-adc-wait-panic` (`pinned_in_sums`); `--console uart0 --max-time 10 --trace-hash --firmware` (F7); in order `rst:0x1`, `no core dump in flash (`, `I2C0 SR 0x0000c000`, `waiting for the SAR ADC`, the panic, `Rebooting...`, `rst:0xc`, `a core dump is in flash at 0x`; one panic; the CPU reset in the log; warnings `I2C0.SR` read, `APB_SARADC.INT_CLR` and `ONETIME_SAMPLE` writes, `APB_SARADC.INT_RAW` read; nothing undocumented; the trace hash against `ADC_WAIT_PANIC_TRACE`, the failure printing the measured one.
- [ ] **Step 5: CI.** `reference-boot`: `INKEMU_ADC_WAIT_PANIC_APP`, `run_cli reference_firmware` count 9 → 10, the comment of the job.
- [ ] **Step 6 (CI):** the first build prints `unpinned SHA256SUMS.inputs-adc-wait-panic:` and three lines: write them to that file, push, and check that the next (clean) build passes `sha256sum -c`.
- [ ] **Step 7 (CI):** the test then fails on `ADC_WAIT_PANIC_TRACE` only, printing the hash of the run: check the console and the warnings in the failure message are as stated, pin the hash, push.
- [ ] **Step 8: Commit** (Steps 1–5): `feat(tools): adc_wait_panic, ours, waits for the SAR ADC with interrupts masked as CrossPoint does until the interrupt watchdog panics, writes a core dump and finds it in its second life, and run_cli holds its two lives in the CI`.

## Task 8: Documents (E10)

**Files:** modify `README.md`, `docs/superpowers/notes/2026-10-13-sp1b-3c-carryover.md` (appended), `docs/research/flash-writes-and-crosspoint-boot.md` (appended); create `docs/superpowers/notes/2026-10-14-sp1b-3e-carryover.md`.

- [ ] **Step 1:** README: section "Real firmwares of the X4" (EN) and paragraph "Firmwares réels du X4" (FR): verified locally on 2026-10-14, the two SHA-256, the command, what the console and standard error show, the tests and how they protect a dump; the status line and the roadmap.
- [ ] **Step 2:** Append "Correction du 2026-10-14" to the 3c note and "Correction of 2026-10-14, to §K" to the FW report: the second life is explained (the verdict X4 stored in NVS survives the CPU reset of the panic within the run), and nothing measured under QEMU contradicts it (its trace stops at the first reboot, its capture reads UART0 only). The existing text is not rewritten.
- [ ] **Step 3:** The hand-over note (French): results, the measurements of 2026-10-14, the values the CI must pin, the assumptions (106 → 107), F1–F8, what is left to each heir, the limits of proof.
- [ ] **Step 4: Commit:** `docs: the README says CrossPoint 1.6.5 and the stock X4 firmware start as the device shows them, verified on 2026-10-14, the 3c note and FW §K are corrected on the second life, and the hand-over of SP1b-3e`.
