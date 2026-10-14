# SP1b-4c — Lockstep against QEMU, from the ROM to `app_main` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** criterion 3 of §10.3 and item 10 of §10.2 of the global spec, for the reference image `flash_hello.bin`: a development tool runs the image in Espressif's QEMU (inside the pinned Docker image) and in inkemu, compares the PC, `x1`–`x31` and the machine CSRs QEMU prints after every instruction, and finds no divergence outside a numbered, sourced table of admitted differences, from the first instruction of the ROM to `app_main`, the first tick and beyond. A deliberate change of the model is reported with both states and the 64 instructions before it. The CI holds the count of compared instructions and the reason the window ends.

**Architecture:**

| Crate | Owns | Why a GUI can call it |
|---|---|---|
| `cpu-rv32` | `Hart::take_boundary` (what stands before the next instruction, acted on), `Hart::last_instruction`, `QemuIcountTiming` | the debugger of SP2 and the GDB server of SP4 step the same way |
| `soc-esp32c3` | `Esp32C3::step` → `Stepped` (H1), `x`, `set_x`, `csr`; `Timing::{Datasheet, QemuIcount}`, `SocConfig::interrupt_latency_cycles` (H2); `QemuCompat` (H3 iv), off by default | stepping and introspection live in the machine, without host I/O |
| `lockstep` (new) | the reader of QEMU's trace (H5), the comparator and its policy (H3, H4), the table of differences, the report (H8) | no host I/O (its `clippy.toml` bans it), builds for `wasm32-unknown-unknown` |
| `inkemu-lockstep` (new, `crates/lockstep-cli`) | the development binary (H7): the machine, QEMU in Docker (H6), the named pipe, the summary, exit codes | not part of the distributed `inkemu`, `publish = false` |

**Tech Stack:** Rust 1.99.0 (edition 2024). No new third-party package: `nix` (feature `fs`, for `mkfifo`), `clap`, `ctrlc` and `thiserror` are workspace dependencies already.

**Spec:** `docs/superpowers/specs/2026-10-14-sp1b-4c-design.md` (decisions H1–H11, binding). Global spec §4.2, §4.4, §10.2 item 10, §10.3 criterion 3. Scoping: `docs/research/sp1b-4-scoping.md` §0, §1(a). Oracle: `docs/research/sp1b-test-oracle.md` §1. Known differences: `docs/superpowers/notes/2026-10-11-sp1b-2-carryover.md` (table rows 1–22), `docs/research/esp32c3-timers-and-watchdogs.md` §G.

**What was verified, and where.** On the development host (H10): `tools/reference-firmware/build.sh` once, then QEMU in `espressif/idf:v5.5.2`, one container at a time, `nice`; `cargo fmt --all`, `tools/ci/clippy.sh -p <touched crates> --all-targets --all-features -- -D warnings`, `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items -p <touched crates>`, `cargo check --target wasm32-unknown-unknown` of the crates without host I/O, narrow `cargo test -p <crate> <filters>`. **Not run on that host**, left to the CI: `cargo test --workspace`, the `#[ignore]` suites, coverage.

## Values only the CI can produce

The count of compared instructions is measured locally (R10): 3 402 169 instructions, the window ending on a reset of both sides, twice identical. The CI step pins it; if the runner's QEMU (the same image) gives another count, the controller re-pins it from the job log.

## Global Constraints

- Toolchain pinned: Rust **1.99.0**, edition **2024**. `unsafe_code = "forbid"`; `unwrap_used`/`expect_used` denied outside tests.
- No `TODO`, no placeholder. No third-party package added.
- **No loop or buffer sized by an untrusted value without a constant bound.** QEMU's trace is untrusted text: a line is read through `take(MAX_LINE_BYTES + 1)`, a block has at most `MAX_BLOCK_LINES` lines, a register value is parsed from a bounded token; the history of a report is a ring of `HISTORY` (64) entries.
- **The default behaviour of the machine does not change.** `run_within` keeps its order of operations to the instruction (the refactor shares one turn of the loop between `run_within` and `step`); `Timing::Datasheet`, a latency of 4 cycles and every `QemuCompat` setting off are the defaults; the pinned trace hashes of the reference tests stay valid.
- `soc-esp32c3`, `firmware`, `boards`, `console-script`, `session`, `lockstep`: no host I/O (clippy bans, `crates/boards/tests/host_io_ban.rs` holds the six copies equal) and they build for `wasm32-unknown-unknown`.
- QEMU runs only inside `espressif/idf:v5.5.2`, never on the host; the flash image is mounted read-only (H6).
- Every admitted difference is a numbered row of `docs/lockstep-differences.md` with its source and treatment, and of `lockstep::DIFFERENCES`; a test holds the two equal. No difference is absorbed outside a row.
- Code, tests, commits, README, ADR and CLI cite the spec's decisions H1–H11 at most; the rulings R1–R14 of this plan are cited by this plan and the hand-over note only.
- Checking a command: redirect its output to a file, then read `$?`; never through a pipe.
- Commits: `type(scope): description`, no trailer, no attribution line. Every commit builds and passes clippy on its own.
- Documents for people (ADR 0008, the table, the hand-over note) are in French; README English then French; code, CLI and this plan in English.

## Review Focus

- **The refactor of the run loop changes nothing by default.** `run_within` and a loop of `step` give the same PCs, cycles and device time → `stepping_runs_the_same_instructions_as_a_run`. The existing tests of `machine.rs` pass unchanged.
- **The pre-state contract (R1).** `step` returns at the boundary of the next instruction, the interrupt taken and a `wfi` slept through → `a_step_takes_the_interrupt_due_after_its_instruction`, `a_step_sleeps_through_a_wfi_to_the_interrupt_that_ends_it`.
- **`qemu-icount` (H2, R4).** 25 instructions are exactly 200 ns of device time at any CPU clock; no interrupt latency → `qemu_icount_prices_25_instructions_at_200_ns_whatever_the_clock`, and `a_step_takes_the_interrupt_due_after_its_instruction` (taken right after the store under `qemu-icount`, not yet under the default model).
- **The policy (H3).** Each treatment has a unit test on a synthetic trace; the dedupe and the preempted block (R3) too.
- **Acceptance 2.** `a_changed_instruction_is_reported_with_both_states`: a trace written from a machine whose program has one instruction changed diverges at that instruction, the report shows both values and the 64 instructions before.
- **The table and the tool agree.** `the_table_of_the_documentation_is_the_table_of_the_tool`.
- **CI.** `reference-boot` gains the lockstep step with `--expect-compared 3402169 --expect-end reset`; the WebAssembly step builds `lockstep`.

---

## Design decisions taken for this plan

H1–H11 bind it. These were taken while writing it.

| # | Decision | Where | Cost if wrong |
|---|---|---|---|
| R1 | The unit of comparison is the instruction boundary. A `-d cpu` block is the state before the instruction at its PC (oracle §1c, validated against the gdbstub); `Esp32C3::step` executes one instruction and returns at the boundary of the next one, the interrupt the CPU takes before it taken and a `wfi` slept through. The power-on state is compared before the first instruction. | Tasks 2, 3 | Low |
| R2 | The compared quantities are the PC, `x1`–`x31` and the eight machine CSRs both sides have that QEMU prints: `mstatus`, `mie`, `mip`, `mtvec`, `mepc`, `mcause`, `mtval`, `mscratch`. QEMU's S- and H-mode CSRs, `mhartid`, `satp`, `mstatush`, `mtval2` are not compared: the chip has none of them beyond `mhartid`, which reads 0 on both. | Task 3 | Low |
| R3 | Two kinds of blocks are dropped and counted. A duplicate: same fingerprint and state as the block before, no trap between, and the instruction the machine executed is not a branch to itself (oracle §1c pitfall 1). A preempted block: a block followed by an interrupt whose `epc` is its own PC, logged when QEMU entered it and taken over by the interrupt before it executed; the machine, which takes the interrupt as part of the boundary, never shows that state. | Task 3 | Medium: a genuine `j .` met by a duplicate counts one iteration too many |
| R4 | `qemu-icount` turns instructions into device time at 8 ns each, exactly: after n instructions (retired, or raising an exception) device time is the whole number of time-base cycles at or below n × 8 ns, 32 cycles every 25 instructions. Taking an interrupt and waiting cost no instruction. Every class costs one CPU cycle (QEMU counts nothing else). The latency of the interrupt controller is the model's (`Timing::interrupt_latency_cycles`: 4, or 0 under `qemu-icount`) unless `SocConfig::interrupt_latency_cycles` sets one. | Task 2 | Low: measured, 1 112 interrupts at the same instructions |
| R5 | QEMU runs with `-icount shift=3,sleep=off`. With `sleep=on`, the default, QEMU's virtual clock follows the host's while the CPU idles: two runs were 21 instructions apart over the first life. With `sleep=off` it jumps to the next deadline, as inkemu's device time does; two runs gave the same count. | Task 4 | Low |
| R6 | Window ends (H4): a reset of either side (QEMU back at `0x40000000`, the machine's reset count), reported with the sides; an access to a block QEMU does not emulate (UHCI0/1, I2C0, RMT, LEDC, BB, SPI2, I2S0, APB_SARADC: the SVD blocks absent from QEMU's `info mtree -f`, each its 4 KiB page); `--max-instructions`; the end of the trace. Blocks of the chip's plumbing that QEMU covers with its catch-all region (IO_MUX, SPI0, APB_CTRL, SENSITIVE, ASSIST_DEBUG, the analog I²C master) are rows (ii), not window ends. A row (ii) may cover registers of an absent block that the firmware programs without depending on them: the bootloader's entropy source writes APB_SARADC, `rtc_sleep_pu` read-modify-writes one BB register. Those accesses are then part of the window, by that row; any other access to the block ends it. | Tasks 3, 4 | Medium: the hello_world window would otherwise end in the bootloader, at instruction 579 249 |
| R7 | Treatment (ii) is a range of registers and a mask of bits: after a load from the range (or a read of a listed CSR) the machine's destination register takes QEMU's bits, only when they differ; each copy is counted for its row. Ranges are the registers measured, not whole blocks, except for blocks QEMU does not model at all. | Task 3 | Low |
| R8 | The one row (iv): QEMU's SYSTIMER compares only a running counter, so the target-mode comparator `vSystimerSetup` enables on a stopped counter raises no early tick (timers report §A.4, §G row 10). A mask cannot hide an interrupt, no register read explains it, and running the extra handler alone to its return leaves `mepc` and `mcause` different until QEMU's next trap. `QemuCompat::systimer_no_alarm_on_stopped_counter`, off by default; `lockstep::table::configure` turns on the settings the rows of the table name. | Tasks 2, 3 | Low |
| R9 | QEMU in Docker: `docker run --rm --name inkemu-lockstep-<pid>-<time> --network none`, the flash bind-mounted read-only and opened with `snapshot=on`, `--entrypoint` QEMU's path in the image, the trace written to a named pipe in a temporary directory (measured: about 80 000 instructions a second, seven times what `-D /dev/stdout` through Docker gives). The container is removed with `docker rm --force` at the end, and the wait for QEMU to open the pipe gives up when the container ends first. | Task 4 | Low |
| R10 | The window the CI holds is the whole first life of `flash_hello.bin`: the ROM, the bootloader, `app_main` (≈ 2.40 M), the first tick and the ten seconds of ticks of the countdown, to `esp_restart`, where both sides reset after the same instruction. 3 402 169 instructions, 1 112 interrupts, measured twice. | Task 5 | Low |
| R11 | Exit codes of `inkemu-lockstep`: 0 window ended as expected; 1 divergence; 2 error; 3 the window ended otherwise than `--expect-compared`/`--expect-end`; 4 the machine stopped on its own; 130 Ctrl-C. `--trace <file>` (or `-`) reads a captured trace instead of running QEMU. | Task 4 | Low |
| R12 | Counting row use: a mask counts each comparison it settles, a copy each value copied, a covered access to an absent block each access, a row (iv) once, when the machine is built with it. The summary prints the rows used with their counts; the test of the table holds the static agreement. | Tasks 3, 4 | Low |
| R13 | `Esp32C3::step` returns a `Stepped` with the instruction executed, the interrupt taken, whether the CPU slept and the stop reason, rather than a `Result`: an instruction can execute and the machine stop after it (a strict-mode violation, a sleep nothing ends). | Task 2 | Low |
| R14 | The report (H8) lists every compared quantity with three columns (before, QEMU, inkemu) and marks a difference outside the masks `<<` and a masked one `(masked)`; the history gives index, PC, instruction bits, interrupt taken and value copied; the nearest row is a mask of a quantity that differs, a copy whose range shares the 4 KiB page of the access, or a copy of the CSR read. | Task 3 | Low |

## File Structure

```
Cargo.toml                                                  members and dependency `lockstep` (Tasks 3, 4)
.github/workflows/ci.yml                                    wasm32 step + lockstep; lockstep step in reference-boot (Task 5)
README.md                                                   developer section: the lockstep (Task 6)
docs/lockstep-differences.md                                new, French: the table (Task 3)
docs/adr/0008-lockstep-et-differences-admises.md            new, French (Task 6)
docs/superpowers/notes/2026-10-14-sp1b-4c-carryover.md      new, French (Task 6)
crates/cpu-rv32/src/{hart,timing,lib}.rs                    take_boundary, last_instruction, QemuIcountTiming (Task 2)
crates/soc-esp32c3/src/{machine,config,ctx,lib}.rs          step, Stepped, Executed, x, set_x, csr; Timing; latency; QemuCompat (Task 2)
crates/soc-esp32c3/src/periph/{systimer,mod}.rs             the setting of row 24 (Task 2)
crates/boards/tests/host_io_ban.rs                          six crates (Task 3)
crates/lockstep/{Cargo.toml,clippy.toml,src/{lib,state,trace,table,lockstep,report}.rs,tests/{policy,table}.rs}   new (Task 3)
crates/lockstep-cli/{Cargo.toml,src/{main,qemu}.rs}        new: inkemu-lockstep (Task 4)
crates/lockstep-cli/tests/readme.rs                         the README's command and the CI's counts agree (Task 6)
```

---

## Task 1: This plan

- [x] **Step 1:** Commit: `docs(plan): SP1b-4c, lockstep against QEMU from the ROM to app_main`.

## Task 2: Stepping, introspection and the timing of QEMU (H1, H2; R1, R4, R8, R13)

**Files:** `crates/cpu-rv32/src/{hart,timing,lib}.rs`; `crates/soc-esp32c3/src/{machine,config,ctx,lib}.rs`, `periph/{systimer,mod}.rs`.

- [x] **Step 1: Write the failing tests.** `hart.rs`: `take_boundary` takes a pending enabled interrupt, ends a `wfi` on a line even with `MIE` clear, and is `Ready` otherwise; `last_instruction` is the instruction just executed and `None` after a fetch fault. `timing.rs`: `QemuIcountTiming` prices every class at 1. `config.rs`: defaults (`Datasheet`, latency 4, `QemuCompat` off); `QemuIcount` has latency 0 unless set. `machine.rs`: stepping runs the same instructions as a run; a step returns at the next boundary, takes the interrupt due after its instruction, sleeps through a `wfi`; `x`, `set_x` (not `x0`), `csr`; 25 instructions under `qemu-icount` are 200 ns at 20 MHz and at 160 MHz; a selected line is visible at once under `qemu-icount`, after 4 cycles by default. `systimer.rs`: with the setting, a target-mode comparator on a stopped counter raises nothing, and raises once the counter runs.
- [x] **Step 2: Code.** Split `Hart::step` into `take_boundary` and the execution; one private `turn` of the run loop shared by `run_within` and `step`; `Pricing` (datasheet, or 8 ns per instruction with the remainder in 25ths); `Ctx::advance_time_base`; `Timing`, `SocConfig::{timing, interrupt_latency_cycles, qemu_compat}`; `Systimer::with_qemu_compat`.
- [x] **Step 3: Run** `cargo test -p cpu-rv32 --lib hart timing`, `cargo test -p soc-esp32c3 --lib machine config systimer` → pass.
- [x] **Step 4: Commit** (two): `feat(cpu-rv32): the boundary before an instruction, the last instruction, and the timing of QEMU's icount`; `feat(soc-esp32c3): stepping and introspection, the timing model qemu-icount, a configurable interrupt latency and QEMU's SYSTIMER for the lockstep`.

## Task 3: The crate `lockstep` (H3–H5, H8; R2, R3, R6, R7, R12, R14)

**Files:** create `crates/lockstep/`, `docs/lockstep-differences.md`; modify `Cargo.toml`, `crates/boards/tests/host_io_ban.rs`.

- [x] **Step 1: Write the failing tests.** `trace.rs`: a real block and a trap line are read; a rendered block reads back; identical blocks share a fingerprint; a line one byte past the bound, a block cut short, without a quantity, too long, or with a value that is not hex is refused with its line. `table.rs`: rows numbered once; every absent block holds its SVD registers. `tests/policy.rs`, on traces written from a machine running a program (`render_block`): the same program compares to the end of the trace; a changed instruction diverges at it with both states and the history (acceptance 2); a duplicate block is dropped, a branch to itself is not; a preempted block is dropped; a reset of QEMU ends the window; an access to an absent block ends it, one a row covers does not; a load from a row's range takes QEMU's value and counts the row; a mask settles its bits only; the limit ends the window. `tests/table.rs`: the documentation's table and `DIFFERENCES` have the same rows and treatments.
- [x] **Step 2: Code.** `state` (fields, `CpuState`), `trace` (`TraceReader`, `render_block`), `table` (`Treatment`, `Compat`, `configure`, `DIFFERENCES`, `UNEMULATED`), `lockstep` (`Target` for `Esp32C3`, `Lockstep::feed`/`finish`, `End`, `WindowEnd`, `Counts`), `report`. `clippy.toml`, the sixth copy of the ban.
- [x] **Step 3: Run** `cargo test -p lockstep`, `cargo test -p boards --test host_io_ban`, `cargo check --target wasm32-unknown-unknown -p lockstep` → pass.
- [x] **Step 4: Commit:** `feat(lockstep): QEMU's trace, the comparison after every instruction under a numbered table of admitted differences, and the report of a divergence`.

## Task 4: `inkemu-lockstep` (H6, H7; R5, R9, R11)

**Files:** create `crates/lockstep-cli/`; modify `Cargo.toml`.

- [x] **Step 1: Write the failing tests** (`qemu.rs`): QEMU's arguments carry `-icount shift=3,sleep=off`, one instruction per block, `-d cpu,nochain,int`, the read-only drive with `snapshot=on` and the pipe.
- [x] **Step 2: Code.** The CLI (`--flash`, `--rom-dir`, `--device`, `--trace`, `--image`, `--qemu`, `--max-instructions`, `--expect-compared`, `--expect-end`), the machine of the profile under `configure`, QEMU in Docker through a named pipe, the summary and the exit codes.
- [x] **Step 3: Run** `cargo test -p inkemu-lockstep`; locally, `inkemu-lockstep --flash target/reference-firmware/flash_hello.bin --rom-dir <dir> --expect-compared 3402169 --expect-end reset` → exit 0, twice.
- [x] **Step 4: Commit:** `feat(inkemu-lockstep): the development binary that runs QEMU in the pinned image and compares it with inkemu`.

## Task 5: The CI (H9; R10)

**Files:** `.github/workflows/ci.yml`.

- [x] **Step 1:** `check`: the WebAssembly step builds `lockstep`. `reference-boot`: the disk is freed and the image pulled whatever the cache of the firmware; a step runs `cargo run --release -p inkemu-lockstep -- --flash … --rom-dir … --image espressif/idf:$ESP_IDF_VERSION --expect-compared 3402169 --expect-end reset`.
- [x] **Step 2: Commit:** `ci: the reference-boot job compares flash_hello.bin with QEMU over its first life, and the WebAssembly step builds lockstep`.

## Task 6: Documents (H11)

**Files:** `README.md`; create `docs/adr/0008-lockstep-et-differences-admises.md`, `docs/superpowers/notes/2026-10-14-sp1b-4c-carryover.md`.

- [x] **Step 1:** ADR 0008 (French): the unit of comparison, the policy and its order of preference, the window, QEMU in Docker, the timing of QEMU, what the lockstep proves and what it does not.
- [x] **Step 2:** README (EN, then FR): the developer section on `inkemu-lockstep`.
- [x] **Step 3:** The hand-over note (French): results, rulings R1–R14, what is left to SP1b-4d and later, limits of proof.
- [x] **Step 4: Commit:** `docs: ADR 0008, the lockstep and its admitted differences, the README of inkemu-lockstep, and the hand-over of SP1b-4c`.

## Fix round (2026-10-14)

Two reviews (policy: 3 blocking, 11 important, 8 minor; machine: 5 medium, 7 low) found that the lockstep
could pass with a wrong model. The controller's rulings and how they are implemented:

| # | Ruling | Implementation |
|---|---|---|
| R15 | A copy row never replaces a value blindly: exact registers and bits, equality outside the mask, every adoption counted; rows 7, 9, 10 and 18 narrowed. | `Treatment::Registers` of `Register { address, mask, qemu, after_command }`; masks are the bits measured to differ; `W0` conditioned on the SPI1 opcode at the store of `CMD.USR`; `TIME_LOW0` requires QEMU's 0. |
| R16 | Rows 2 and 4 narrowed to their bits and, where possible, their window. | `Treatment::Pair` with `until_written`; `Treatment::ExternalInterrupt { lag: 3 }` against `Esp32C3::external_interrupt_as_qemu`. |
| R17 | SYSTIMER (and any timer): exact equality under `qemu-icount`, else a bounded, sourced, counted delta; never a full copy. | `QemuCompat::systimer_counts_as_qemu` and `cycle_counter_as_qemu` (QEMU's models ported from its source); `Treatment::Counter` bounds 2 and 1, measured. |
| R18 | Accesses to undocumented addresses end the window or are an explicit row; a one-sided reset is a divergence. | `absent_in_qemu` from QEMU's `info mtree -f` to the byte; `WindowEnd::Absent`; `Cause::OneSidedReset`. |
| R19 | Per-row usage recorded and checked; unused or out-of-declaration rows fail the run; row 24 counts only when it acts; CI prints it. | `lockstep::Usage`, `Usage::check`, exit 5; `CompatActs` from the machine. |
| R20 | Duplicates bounded and cross-checked against the trace. | `MAX_REPRINTS` = 1; `loops_on_itself` on raw bits and QEMU's state. |
| R21 | Machine and tool: Ctrl-C, Drop, empty trace, long sleeps, Windows, image tag, CI timeouts, faulting accesses, wider equivalence test, negative test against QEMU. | See the commits below and the hand-over note. |

- [x] **Step 1:** `fix(cpu-rv32)`: the QEMU cycle counter, off by default.
- [x] **Step 2:** `fix(soc-esp32c3)`: `step` resumes at its boundary, `qemu-icount` refunds faulting accesses, the
  SYSTIMER and cycle-counter settings with their act counters, `external_interrupt_as_qemu`; tests.
- [x] **Step 3:** `fix(lockstep)`: the table of exact registers, pairs and bounded counters; `Usage`; the window
  by QEMU's devices; one-sided reset; reprints; reader bounds; `inkemu-lockstep` (Drop, process group, timeouts,
  exit codes 2/5/130, `--perturb-after`, Unix-only QEMU mode, `versions.env`); `docs/lockstep-differences.md`.
- [x] **Step 4:** `ci`: timeouts, `--qemu` from `versions.env`, the run with a fault that must diverge.
- [x] **Step 5:** `docs`: ADR 0008, the correction of the spec, the README, this section, the hand-over note.

Measured after the fixes: 3 402 169 instructions compared (unchanged), window end at the reset of both sides, all
24 rows used within their declarations, the same usage live and on the captured trace.
