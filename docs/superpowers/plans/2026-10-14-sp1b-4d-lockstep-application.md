# SP1b-4d — Lockstep through the application, and CrossPoint in local — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** criterion 3 of §10.3 of the global spec beyond the first life of the reference image: the lockstep goes on through the reset `esp_restart` asks for, compares the second life, covers the other reference firmwares where QEMU covers them, each window pinned in the CI, and compares CrossPoint 1.6.5 once in local, the result dated.

**Architecture:**

| Crate | Owns | Why a GUI can call it |
|---|---|---|
| `lockstep` | `Window` and `WINDOWS` (J3), `Lockstep::for_window`, the crossing of a reset of both sides (J2), the treatments `KeptAtReset`, `CsrKeptAtReset`, `RegistersAfterReset`, the end `WindowEnd::Bound`, the cause `Cause::ReadKeptAtReset`, the rows 25 to 41 | no host I/O, builds for `wasm32-unknown-unknown` |
| `inkemu-lockstep` | `--window`, `--firmware` (the flash composed as `inkemu run --firmware` composes it, written owner-only beside the pipe for QEMU), the local CrossPoint test | a development tool, `publish = false` |

**Tech Stack:** Rust 1.99.0 (edition 2024). No new third-party package (`sha2` and `assert_cmd`-free std `Command` for the local test).

**Spec:** `docs/superpowers/specs/2026-10-14-sp1b-4d-design.md` (decisions J1–J9, binding). SP1b-4c: its spec (H1–H11 and its correction), its plan (R1–R29), ADR 0008, `docs/lockstep-differences.md`, and the hand-over note `2026-10-14-sp1b-4c-carryover.md`.

**What was verified, and where.** On the development host: `tools/reference-firmware/build.sh` once (every image checked against its pinned SHA-256), QEMU in `espressif/idf:v5.5.2`, one container at a time, its traces captured compressed and compared offline as often as needed; `cargo fmt --all --check`, clippy (`-D warnings`) and rustdoc (`-D warnings`) on `lockstep` and `inkemu-lockstep`; `cargo test -p lockstep` and `cargo test -p inkemu-lockstep`; the 12 s trace hash of `flash_hello.bin`. **Not run on that host**, left to the CI: `cargo test --workspace`, the `#[ignore]` suites, coverage, the lockstep steps themselves against the runner's QEMU.

## Global Constraints

- The default machine does not change: nothing in `soc-esp32c3` or `cpu-rv32` changes; the 12 s trace hash of `flash_hello.bin` stays `a0d221296337b295` and the hashes pinned in `run_cli.rs` stay.
- Third-party binaries (CrossPoint, the stock firmware) are never committed, embedded, downloaded or run in the CI. The flash composed from CrossPoint holds the public release asset and erased bytes only.
- No flash byte and no stack dump of a panic is printed.
- QEMU runs only in `espressif/idf:v5.5.2`.
- Every admitted difference is a numbered row, measured on the window that uses it, with exact registers and bits; every window holds its rows to H3 at every run.
- Commits: `type(scope): description`, no trailer.

## Rulings taken while doing it

| # | Ruling | Why |
|---|---|---|
| R30 | **inkemu's reset is the chip's; nothing in it changes.** `esp_restart` ends with `esp_rom_software_reset_cpu(0)` (read in the pinned image), `SW_PROCPU_RST`, `RTC_SW_CPU_RST` (0x0C), a CPU reset (TRM §6.1); what it resets itself goes through `SYSTEM.PERIP_RST_EN0/1` and `CORE_RST_EN`, modelled since SP1b-3a. QEMU, which resets its whole machine, gives the same cause (oracle §1d). | J1. |
| R31 | **The crossing touches nothing in the machine.** What the lockstep knew of QEMU starts again as at power-on: CSRs written, counters' gaps, the SPI1 command, the `MEIP` lag. The capture of the slow clock stays. | J2: no resync. |
| R32 | **What QEMU keeps at its reset is a pair held until written, not a copy.** Measured: QEMU shows at its reset exactly its registers and `mtvec`, `mepc`, `mstatus.MPIE` from before it; inkemu its reset values (`Hart::reset`, an ASSUMPTION for the integer registers). Each declared quantity that differs is held to exactly those two values until an instruction writes it or a trap changes it. Any read diverges (`Cause::ReadKeptAtReset`), except a word store, which carries the pair to memory, and a word load that gives it back exactly, which holds its destination. Measured on both windows: no instruction computes with such a value. | The strictest rule the firmware passes; a copy of QEMU's value would have hidden any use of it. |
| R33 | **PMP: QEMU's reset keeps the R, W, X bits of every entry** (the privileged specification requires only A and L cleared; inkemu's reset clears all). A read after the reset must give in QEMU inkemu's value with the R, W, X bits inkemu held before the reset; the register takes QEMU's bits within the measured mask. | Measured: `0x89888F88` before, `0x01000700` read by QEMU, 0 by inkemu. |
| R34 | **Registers the two resets leave different are copied only after a reset**, on top of what the rows of the first life take at the same address, a bit going to the first row that declares it. In the first life those bits stay compared. | A row valid in the whole window would have admitted them before the reset too. |
| R35 | **A window has its own rows.** A row keeps its parameters in every window that takes it, and every window that takes it uses it whole; another firmware's bits are another row. | J3; a single table would let one firmware's measure admit another's. |
| R36 | **A window may declare a bound**, which ends it as a whole window (H3 holds); `--max-instructions` still cuts a partial one. No window of this plan needs one. | Kept for windows that end at a measured limit of QEMU. |
| R37 | **The other reference firmwares.** `nvs` (`flash_nvs.bin`) crosses its reset like `hello`, with four rows of its own (SPI1 with `WEL` kept by QEMU's flash, an EXTMEM sync bit, its registers kept at reset, SPI1 after reset) and is in the CI; so is `nvs-ota` (`flash_nvs_ota.bin`, the X4's partition table), with the rows of `nvs` and row 33. The applications composed behind the shipped bootloader are not: each window ends inside the application's start-up at a measured artefact of QEMU's interrupt delivery (below), with no end of their own before it; see the hand-over note. | J5: only what QEMU covers. |
| R38 | **QEMU build: no bump.** `20260417` fixes interrupts lost with `MIE` = 0; no window met that. The two artefacts that stop the applications are of another kind (a SYSTIMER alarm taken two instructions late while the CPU runs; `MEIP` shown by QEMU's matrix while inkemu's controller has the line low in the USB-Serial-JTAG path). | J7: change the oracle only for a measured need. |
| R39 | **CrossPoint** is compared locally by an ignored test of `inkemu-lockstep` that reads `INKEMU_CROSSPOINT_X3_X4`, checks its pinned SHA-256, composes its flash and runs the window `crosspoint`. Its result is dated in the hand-over note and ADR 0008. | J6. |
| R41 | **CrossPoint's window ends where QEMU stops, not at a count.** Measured on 2026-10-14: 31 088 420 instructions compared, the ROM, the bootloader (about 30.7M, `otadata` written) and the start of the application, up to its first access to a register QEMU lacks that no row names, `APB_SARADC.APB_TSENS_CTRL` (`0x60040058`) at `0x4214917C`. No row admits it: QEMU has no temperature sensor, and a copy would feed CrossPoint a value neither side measured. Rows 34 to 41 are measured on this window: where CrossPoint shows fewer registers or bits than `hello` (rows 7, 9, 11, 14, 15, 20, 23), it gets rows of its own instead of a partial use of those (R35); row 33 it uses whole. | J6: the spec's "about 32M" is where QEMU ends, measured; no bound needed (R36). |
| R40 | **Measuring is done on captured traces.** A scratch tool, never versioned, took every differing load and logged it per life and per register; the masks of the rows are those unions. The tool itself prints what each row did, register by register. | As SP1b-4c: the relevé is reproducible from the tool's own output. |

### Review round (2026-10-14, reports P1–P9 and M1–M9)

| # | Ruling | Why |
|---|---|---|
| R42 | **`mstatus` is the chip's** (M3). TRM §1.4.2 and register 1.5: reset 0, `MPP` WARL {0, 3} through its low bit, `TW` stored. Fixed in `cpu-rv32` for the ESP32-C3 core only (a plain hart keeps `MPP` wired to machine mode); a trap sets `MPP` to machine mode, `mret` to user mode; execution in user mode stays in SP1b-4f. Row 1 is retired, its number kept (`table::RETIRED`): a number always names one difference. Pinned trace hashes unchanged (PC sequence). | Fidelity first; the row charged QEMU with inkemu's defect. |
| R43 | **Holds end at the second life's write** (P1–P3). `KeptAtReset` holds only the measured bits (`Kept { field, bits, reset }`: of `mstatus`, `MPIE`) and only from inkemu's declared reset value; `CsrKeptAtReset` until a whole write (`csrrw`): a `csrrs`/`csrrc` writes back the value it read, which the row admitted, and carries the reset's difference (measured: the bootloader's `csrrs` on `pmpcfg0`); `RegistersAfterReset` until the first store to the register. | A hold must not admit what the second life itself wrote. |
| R44 | **A module reset QEMU ignores also arms the copies after a reset** until the next store (`table::MODULE_RESETS`, edge of the bit in `PERIP_RST_EN0/1`). QEMU's `SYSTEM` reads 0 and ignores writes there (`hw/riscv/esp32c3_clk.c`); inkemu resets SPI0/1, UART0/1, TIMG0/1, USB-Serial-JTAG, SYSTIMER, SHA. Registers QEMU does not store at all are no reset difference: `0x600080AC` bit 31 goes to row 9, `PERIP_RST_EN0/1` to row 20, with the bits measured in both lives. | Measured per register on the second life of `hello` (every load and store logged): every difference after a store follows a module pulse, or is at those two addresses. |
| R45 | **SPI1 after the reset is at its reset values in inkemu** (P4, M9): `USER` `0x80000000`, `USER2` `0x70000000`, `CTRL2` and `MISO_DLEN` 0, the SVD's. The difference is QEMU's: `esp32c3_spi_reset_hold` deposits fields into the current values without clearing them (`hw/ssi/esp32c3_spi.c`). Row 30's source is `esp32c3_cache_read` returning 0 for `ICACHE_SYNC_SIZE`. | Every row has a real source. |
| R46 | **Hypotheses named, causes not established** (M1, M2, M5, M6): `SYSTEM_TIMERS_RST` without effect (TIMG kept), `mepc` reset to 0, the SYSTIMER alarm anchored at the start of the `CONF` store (`ASSUMPTION:` in `systimer.rs`); the two stops of the composed applications are "cause not established", not QEMU's. QEMU's counters go on through its reset. | Say only what is modelled or measured. |
| R47 | **Tests and pins** (P5–P9): `Lockstep::new` removed (a table belongs to a window; tests use `with_rows`); a usage file naming a row outside its window is refused before the run (exit 2); a test holds the documentation's counts to the pins (CI, usage files, `CrossPoint`'s test); the one-owner test covers copies after a reset, counters and clocks; negative tests for every new limit; a CI step puts the SYSTIMER fault after the reset (instruction 5 700 683, row 15). | Every limit has its negative test. |

## Tasks

- [x] **1. Crossing a reset.** `Lockstep` goes on through the resets its window declares (`Counts::resets_crossed`), what it knew of QEMU starting again. Tests: `a_reset_of_both_sides_ends_a_window_that_crosses_none_and_is_crossed_otherwise`.
- [x] **2. What the reset leaves.** `Treatment::KeptAtReset`, its hold, the carry through memory, `Cause::ReadKeptAtReset`; `Treatment::CsrKeptAtReset` with `KeptCsr` and the snapshot of the CSRs before every step; `Treatment::RegistersAfterReset` and the attribution of bits across rows. Tests in `crates/lockstep/tests/reset.rs`.
- [x] **3. Windows.** `lockstep::window` (`Window`, `WINDOWS`, `window`), `Lockstep::for_window`, `WindowEnd::Bound`; the rows as named constants, `HELLO`, `NVS` and the catalogue `DIFFERENCES`. Tests: the windows' rows are the table's, a register is named by one row of a window, an after-reset entry adds bits only.
- [x] **4. The tool.** `--window` (required), `--firmware`, `--expect-end bound`, the usage file checked over the window's rows.
- [x] **5. Measure** `hello` and `nvs` through their reset (6 813 949 and 7 680 555 instructions), rows 25 to 32, usage files.
- [x] **6. Documentation**: `docs/lockstep-differences.md` (the windows, the crossing, the treatments, rows 25 to 32), README, and its test of the windows.
- [x] **7. CI**: the two windows pinned, and the fault in `t0` put after the reset.
- [x] **8. The other reference firmwares** measured and their stop recorded (R37).
- [x] **9. CrossPoint**: the local test, one run, dated (R39).
- [x] **10. ADR 0008** (dated section), the hand-over note.
