# SP1b-4e — CoreMark within 15 % of the published score — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** criterion 5 of §10.3 of the global spec, held by the CI: Espressif's CoreMark example, built like the other reference firmwares and booted by `inkemu run --firmware`, prints a score timed on inkemu's device time within 15 % of the score Espressif publishes for the ESP32-C3; and a ruling on each of the two hand-overs SP1b-4d left (the first tick, `MEIP` in `usb_serial_jtag_echo`).

**Architecture:**

| Where | Owns | Why |
|---|---|---|
| `tools/reference-firmware/coremark_example/` | Espressif's example and component, copied unmodified, with `PROVENANCE.md` | the reference build has no network and no component manager |
| `tools/reference-firmware/{build.sh,in-container.sh}`, `SHA256SUMS.inputs-coremark`, `SHA256SUMS.coremark-sources` | the build of `coremark_example.bin`, its pins, the check of the copied files | as every reference firmware |
| `crates/emu-cli/tests/run_cli.rs` | the test of criterion 5 | an ignored test of the CI job that has the images |
| `.github/workflows/ci.yml` | `INKEMU_COREMARK_APP`, the count 13 → 14 | |

No product logic is added: the machine, the CLI and the libraries do not change, but for a doc comment of `soc_esp32c3::periph::intmatrix` (K8 c) and, at the review, the doc comments of `cpu-rv32/src/timing.rs` (R67; no value changes). Nothing for a GUI to call.

**Tech Stack:** Rust 1.99.0 (edition 2024). No new package. ESP-IDF v5.5.2 in `espressif/idf:v5.5.2` (GCC 14.2.0).

**Spec:** `docs/superpowers/specs/2026-10-15-sp1b-4e-design.md` (decisions K1–K10, binding). Research: `docs/research/coremark-esp32c3.md`. SP1b-4d: its plan (R30–R54), its hand-over note, ADR 0008.

**What was verified, and where.** On the development host: the CoreMark build twice from clean (once by hand, once by `build.sh` with every other reference firmware, every pin checked), QEMU in `espressif/idf:v5.5.2` one container at a time (the stop of `usb_serial_jtag_echo` measured again under the whole table, with a scratch window never versioned); `cargo fmt --all --check`; `tools/ci/clippy.sh` (`-D warnings`) on `emu-cli` and `soc-esp32c3`; rustdoc (`-D warnings`) on `soc-esp32c3`; the test of criterion 5 (passed in 87 s) and its mutation (failed, undone). **Not run on that host**, left to the CI: `cargo test --workspace`, the other `#[ignore]` suites, coverage, the lockstep steps.

## Global Constraints

- The default machine does not change; the pinned trace hashes stay (12 s `a0d221296337b295`, 40 000 001 cycles `fb9ea628bc25b355`, `adc_wait_panic` `b217bc3b287080ae`, `deep_sleep_timer` `80bd1c43d55b5467`); the windows of the lockstep, their counts and usages stay.
- The band is never widened to make a run pass: a score outside it is a fault of the timing model, found and fixed there with a test.
- The CoreMark sources are copied unmodified; no CoreMark binary is committed; what inkemu prints is never presented as a CoreMark score of a device.
- QEMU and ESP-IDF run only in `espressif/idf:v5.5.2`, one container at a time.
- Commits: `type(scope): description`, no trailer.

## Rulings taken while doing it

| # | Ruling | Why |
|---|---|---|
| R55 | **The reference is the datasheet's 483.27** (K1). The test's message also gives the gap to the example's 409.25; only the datasheet's band fails the test. The measured score lies in both bands, so the choice does not depend on it. | The chip's published score; the example is an output of an older compiler. |
| R56 | **No cost is fitted** (K3). Measured on 2026-10-15: 424.628450, −12.1 % from 483.27. The score bounds the average cost of CoreMark's instructions between 0.966 and 1.307 cycles (1.264 today); a fit on one benchmark picks one of infinitely many models and moves every pinned trace hash. `is_calibrated()` stays `false`; the fit is sub-project 8's, on a device. | One equation, ten unknowns; "calibrated" means measured on a device (spec §1 point 4). |
| R57 | **The component is copied, not fetched** (K4). The reference build stays without network; the 20 copied files are pinned against the registry's `CHECKSUMS.json` and checked in the container before the build; the copies were made with `cp` (bytes, not an edit). The example's `idf_component.yml` is left out (it asks for `espressif/coremark: '*'`); the component sits in `components/coremark/`, under the name the example requires. | Offline, reproducible, verifiable; CoreMark's run rules ask for its files unmodified. |
| R58 | **The test sits in `emu-cli`'s `run_cli.rs`**, run through the real command like `deep_sleep_timer`'s, with `--max-time 20` (CoreMark returns at 17.5 s). It asserts the four iteration-independent CRCs always, and `crcfinal` 0xa14c only when the run has 6000 iterations (CoreMark chooses its count from its own timing: a model 1.18 to 2.35 times faster runs 11 000). | `crcfinal` depends on the iterations; the others are CoreMark's own validation. |
| R59 | **The test fails on a model out of the band.** Checked: with `ALU` at 2 cycles in `cpu-rv32/src/timing.rs`, the score is 295.53 (−38.8 %) and the test fails on the band; the change was undone. The test checks both bounds; the upper one is out of reach of a model that costs at least one cycle per instruction (one cycle for each scores about 537), so only the lower bound can fail such a model. | A test that cannot fail proves nothing; what it cannot see is said. |
| R60 | **The root `NOTICE` does not change** (K6). It is printed by `inkemu licenses` and says what the product includes; CoreMark is in the repository, not in the product. `PROVENANCE.md` and both READMEs carry the licences and the trademark notice. | No false statement in the binary's notices. |
| R61 | **The first tick is deferred to sub-project 8** (K8 a). Read in the pinned QEMU (tag `esp-develop-9.2.2-20250817`): `esp_systimer_comparator_reprogram` anchors the period at `qemu_clock_get_ns` read during the store, which, reasoning on QEMU's count of a block's instructions under `-icount`, counts the store; inkemu anchors at its start: one instruction, by reasoning, not measured. `TICKS_TO_NS(160000)` is exactly 10 ms, so the other is not in `esp_systimer.c` (candidate: `icount_round` of a deadline after a warp of the clock; not established). The SP1b-4d rule allows a `QemuCompat` setting only for a gap sourced there; inkemu's anchor is an `ASSUMPTION:` only a chip settles. | Sourced as far as the code goes; no setting on a guess. |
| R62 | **`MEIP` in `usb_serial_jtag_echo`: SP1b-4d's hypothesis refuted, the cause deduced, not observed** (K8 b; reworded at the review, R65). Measured again on 2026-10-15 (scratch window under the whole table, never versioned): stop at 2 525 511, `csrs mstatus` at `0x4038025c`, `mcause` 0x80000003, QEMU `mip` 0x800, no trap; inkemu's controller has line 3 = `FROM_CPU_INTR0` at priority 1 and the threshold just stored at 2. Established: QEMU's USB-Serial-JTAG has no interrupt output (`hw/misc/esp32c3_jtag.c`). Deduced from QEMU's code and the state at the stop, not observed in QEMU: its matrix keeps a sticky pending bit and its CPU leaves `MEIP` up while one is reported (`esp32c3_intmatrix.c`, `esp_cpu.c` of the tag). No row is added: a row belongs to a window that uses it (R35), and no composed application has a window before R61 is settled; the treatment goes with R61 to sub-project 8. | The hypothesis of SP1b-4d (a USB source high in QEMU) is refuted by the code. |
| R63 | **Row 4's source is corrected, its measure kept** (K8 c). It cites `esp32c3_intmatrix_line_should_assert` and `esp_cpu_update_parent_irq`, functions of the branch `esp-develop`, not of the tag the lockstep runs; the "three instructions, mechanism not identified" are the stale `MEIP` of R62, up from the interrupt taken to the check after `csrs mstatus`. Dated revisions in `docs/lockstep-differences.md` and ADR 0008, and a paragraph in the doc of `IntMatrix::asserted_as_qemu`; the row's parameters, its uses and the windows' pins do not change. | A row's source must be the oracle's own code (ADR 0008). |
| R64 | **No per-iteration instruction count against QEMU** (K9). | Not asked by the criterion; hours of lockstep; the CLI has no `qemu-icount`. |

### Review round (2026-10-15, PR #14 at `17a9867`, I1–I3 and M1–M9)

| # | Ruling | Why |
|---|---|---|
| R65 | **What is established is told apart from what is deduced** (I1, M8). Established: QEMU's USB-Serial-JTAG has no interrupt output, which refutes SP1b-4d's hypothesis. Deduced from QEMU's code and the state at the stop, not observed in QEMU: the sticky pending bit behind `MEIP`, and the three instructions of row 4. Explained by reasoning on the code, not measured: one of the tick's two instructions. Reworded in ADR 0008, spec K8, the table, the report, R61, R62 and the hand-over note. No row is added; observing the bit in QEMU (monitor or gdbstub) goes to sub-project 8. | A reading of code is not a measure. |
| R66 | **The table's QEMU sources are the tag's** (I2). The image holds QEMU's binary only (`--version`: `esp_develop_9.2.2_20250817`); the sources are read at the tag. Rows 9, 17, 20, 27, 28, 30, 32, 37: files identical in tag and branch; 15, 24, 39: `esp_systimer.c` differs by debug traces only; 5: its functions identical; 2: cited `esp_cpu_has_work` (branch), the reset value comes from `esp32c3_intmatrix_reset_hold` in the tag; 4: R63. Dated revisions; no measure, parameter, use or pin changes. | The oracle's own code is the source (ADR 0008). |
| R67 | **`cpu-rv32/src/timing.rs` says what ADR 0009 says** (I3): uncalibrated estimates, held on average by CoreMark, fitted only on a device in sub-project 8. Doc comments only, by Edit; every value unchanged. | The code contradicted the decision. |
| R68 | **The score is held to the band as soon as its line is read** (M1), in `coremark_score_within_criterion_5`, and appears in every later failure message; both bounds are checked (M7: the upper one is out of reach of a model at one cycle per instruction or more, about 537). | A failure elsewhere must not hide the verdict on criterion 5. |
| R69 | **A table of known final CRCs** (M2): `COREMARK_FINAL_CRCS` lists 6000 → 0xa14c, from Espressif's output and QEMU; any other number of iterations fails, saying that a CRC enters the table only from such a source, never from the run. Checked by a mutation (6000 → 6001: fails with that message; undone). | A count CoreMark chose from a moved timing must be investigated, not pinned. |
| R70 | **inkemu's own clock is checked** (M3): the summary line `CPU clock at 160 MHz`, beside the firmware's `CPU frequency: 160 MHz` (its configuration). | The second only says what the firmware asked for. |
| R71 | **`coremark_example/` holds the pinned files and `PROVENANCE.md`, nothing else** (M4): `in-container.sh` compares the directory's list with `SHA256SUMS.coremark-sources` before the hashes. Checked in `espressif/idf:v5.5.2`: an added file, or a pinned file removed, stops the build. | An added header or source could change the build unseen. |
| R72 | **Durations are the measured ones** (M5): the test 68 s in the CI (`finished in 68.26s`), 87 and 102 s on the development host; the run alone 110 s there. The report shows the `Compiler flags` and `CoreMark 1.0` lines, saying what is shortened (M6); the README gives GCC 14.2.0, `-O3`, IRAM beside the score (M9). | Numbers say where they were measured. |

## Tasks

### Task 1: Research and sources

- [x] **Step 1:** Download the registry release `espressif/coremark` 1.1.0~2 (archive and `CHECKSUMS.json`) on the host as data; check every file kept; read the licence, the run and reporting rules, the port (`esp_timer`), the iteration choice of `core_main.c`.
- [x] **Step 2:** Download the datasheet again (v2.4, same SHA-256 as on 2026-10-11) and read its CoreMark line and revision history.
- [x] **Step 3:** Read the QEMU sources of the tag `esp-develop-9.2.2-20250817` for the two hand-overs (`esp_systimer.c`, `esp32c3_intmatrix.c`, `esp_cpu.c`, `esp32c3_jtag.c`, `icount-common.c`), and the branch's `esp32c3_intmatrix.c` that row 4 cites.
- [x] **Step 4:** Measure `usb_serial_jtag_echo` again under the whole table (a scratch window, removed) and inkemu's interrupt matrix writes (a scratch print, removed).
- [x] **Step 5:** `docs/research/coremark-esp32c3.md`. Commit `docs(research): …`.

### Task 2: The reference firmware

**Files:** `tools/reference-firmware/coremark_example/` (copied files and `PROVENANCE.md`), `SHA256SUMS.coremark-sources`, `SHA256SUMS.inputs-coremark`, `build.sh`, `in-container.sh`, `README.md`.

- [x] **Step 1:** Copy the component and the example with `cp`; pin the 20 files (`SHA256SUMS.coremark-sources`, the registry's hashes).
- [x] **Step 2:** Build once in the container by hand: `coremark_example.bin` `1de13ef8…`, bootloader and partition table equal to `hello_world`'s; pin `SHA256SUMS.inputs-coremark`.
- [x] **Step 3:** `in-container.sh`: check the copied files, then build the project with the applications; `build.sh`: header and output list; README: table, configuration, licence, variables, a command to run it.
- [x] **Step 4:** `build.sh` whole from clean: every pin passes, CoreMark's included (the second build).

### Task 3: The test (TDD)

**Files:** `crates/emu-cli/tests/run_cli.rs`, `.github/workflows/ci.yml`.

- [x] **Step 1:** Run the firmware by hand: 424.628450, 6000 iterations, 14 130 ms, CRCs right, no warning, `trace-hash bfb20978732b673f` at `--max-time 20`.
- [x] **Step 2:** Write `x4_runs_the_coremark_reference_firmware_within_15_percent_of_the_published_score` (R58); `cargo fmt`, clippy.
- [x] **Step 3:** Run it: passed (87 s). Mutation R59: failed on the band; undone.
- [x] **Step 4:** CI: `INKEMU_COREMARK_APP`, `run_cli reference_firmware` 13 → 14, the job's comment.

### Task 4: The hand-overs of SP1b-4d

- [x] **Step 1:** R61, R62, R63 in the research report, the hand-over note, `docs/lockstep-differences.md` (dated revisions under the paragraph of the composed applications and under row 4), ADR 0008 (dated section), the doc of `IntMatrix::asserted_as_qemu`; clippy and rustdoc on `soc-esp32c3`.

### Task 5: Documents

- [x] **Step 1:** ADR 0009; spec; this plan; README (EN and FR: the timing model's line, the roadmap, the licences); hand-over note `2026-10-15-sp1b-4e-carryover.md`; the project's cartography.
