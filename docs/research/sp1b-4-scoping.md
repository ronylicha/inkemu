# SP1b-4 scoping — lockstep, CoreMark, replay, PMP, CI hygiene, line-faithful cache

Date 2026-10-14, branch `sp1b-4-scoping` at `44bccc5`. Read-only research; the one command run was
`cargo doc` (below). Tags: **M** = measured or read (file:line / command output), **R** = reasoning or
estimate, to be confirmed by the plan.

## 0. Findings that change the shape of SP1b-4

1. **Lockstep "same initial state" is not reachable as the spec words it.** QEMU and inkemu differ
   structurally inside the ROM and the bootloader, not only in timing: the ROM writes every console
   character to USB-Serial-JTAG and QEMU's `EP1_CONF` reads 0, so the ROM gives up after ~5 000 reads
   (M: `docs/superpowers/notes/2026-10-11-sp1b-2-carryover.md:114`); QEMU boots on the PLL so the
   bootloader skips the PLL bring-up inkemu executes (M: same file :98, :123); eFuse block v1.3 vs
   v1.2 changes the `rtc_init` calibration branch (M: :109); host RNG enters registers from insn
   ≈ 635 927 (M: :112); regi2c reads `0xFFFFFF` vs 0 (M: :99). 22 known rows, plus timing.
   → Criterion 3 needs either a **QEMU-compatibility configuration of the machine** or
   **anchor-based resynchronisation** with a declared whitelist (R). This is a spec decision.
2. **Timing must be aligned, not just whitelisted.** QEMU runs `-icount 3`: every instruction is
   exactly 8 ns, whatever the CPU clock (M: `docs/research/sp1b-test-oracle.md` §1c pitfall 4).
   inkemu's time base is 160 MHz (`crates/soc-esp32c3/src/clock.rs:35`, M), so 8 ns = 1.28 ticks:
   not integral. Without a timing mode whose device time after n instructions ≈ n × 8 ns, the
   FreeRTOS tick and every timer interrupt land on different instructions right after the
   scheduler starts (hello_world: first timer interrupt at insn ≈ 2.42 M, M: oracle §1b) (R).
   UART/USJ TX also take device time in inkemu (347 µs per byte at reset baud, M:
   `crates/soc-esp32c3/src/bus.rs:856`) while QEMU drains at once: console wait loops diverge (R).
3. **The CrossPoint window is ≈ 32 M instructions, not 50 M**, and it cannot run in CI. Under QEMU
   (CrossPoint's own bootloader) the application entry is at insn 31 904 574 and the first access to
   a peripheral QEMU lacks (temperature sensor / `APB_SARADC`, then I2C0 at +737 216) comes early in
   the application (M: `docs/research/flash-writes-and-crosspoint-boot.md:22,26,41,438`). The
   CrossPoint binary can be neither shipped nor downloaded, so its lockstep is a local, dated run,
   exactly like criterion 2 today (M: tech_debt "Limites de preuve de SP1b-3e"). With inkemu's
   shipped bootloader the count differs (R).
4. **ADR 0002's premise is wrong for the line-faithful cache.** ADR 0002:35 says the line-faithful
   cache comes "with the lockstep that makes it verifiable" (M); but QEMU does not emulate cache
   coherence effects either (M: oracle §1d, "Not emulated: cache coherence effects"). Only
   hardware (SP8) verifies it (R).
5. **CoreMark runs from IRAM** (`espressif/coremark` places the code in IRAM, M: oracle §3), so
   neither the flash cache nor `fetch_stall` affects criterion 5 (R). Criterion 5 depends on
   instruction costs, SRAM wait states and the 160 MHz clock only.
6. **Replay is literally needed for criterion 6** ("two executions of the same *session*", spec
   §10.3 item 6, M: spec line 317) and §10.2 item 9 lists recording and replay (M: spec :306). No
   recording code exists: `grep inkrec|replay` finds nothing outside comments (M).
7. **`cargo doc` remeasured: 93 rustdoc warnings** (98 `^warning` lines minus 5 per-crate totals),
   not 72: `soc-esp32c3` 83, `cpu-rv32` 4, `emu-cli` 3, `firmware` 2, `boards` 1; kinds: 77
   unresolved links (bit ranges such as `[9:0]`, `[11:7]` read as intra-doc links), 13 public docs
   linking a private item, 2 unclosed HTML tags, 1 redundant link target (M:
   `RUSTDOCFLAGS= cargo doc --workspace --no-deps --document-private-items`, exit 0).

## 1. Item by item

### (a) Lockstep against Espressif's QEMU (criterion 3)

**Exists.**
- Research with working commands: `-accel tcg,one-insn-per-tb=on -d cpu,nochain -D /dev/stdout
  -icount 3`, ~133 k insn/s, ~1.1 KB of text per instruction; `-d exec` pc-only at ~300 k insn/s;
  gdbstub ~7 k steps/s; no TCG plugins in released builds; duplicate-entry dedupe rule; reset CSR
  state to mask (M: oracle §1a–§1d, lines 14–212).
- QEMU binary sources: inside the pinned `espressif/idf:v5.5.2` image (build 20250817), or the
  release tarball 20260417 (16.8 MB, SHA-256 `547f03e0…2e84e77`, pinned in the research), which
  fixes "interrupts lost or delayed on ESP32-C3 when MIE=0" (M: oracle §1a). No QEMU runs in CI or
  tests today: QEMU appears only as captured golden logs and command lists in
  `crates/soc-esp32c3/tests/reference_boot.rs:39-114,724,1008` and `run_cli.rs:3865` (M).
- Divergence catalogue: 22 rows (`sp1b-2-carryover.md:87-123`), plus the reset rule, interrupt
  latency (`INTERRUPT_LATENCY_CYCLES = 4`, `crates/soc-esp32c3/src/machine.rs:184`, whose doc
  already asks for it to be configurable for the lockstep, M), `FLASH_CE` `C7` vs `60` (tech_debt
  :63, M).
- Reference flashes built in CI: `flash_hello.bin` (ROM→`app_main` ≈ 2.40 M insns under QEMU,
  M: oracle §1b), `flash_nvs*.bin`, five applications (M: `tools/reference-firmware/build.sh`).

**Missing.**
1. A per-instruction **stepping and introspection API** on `Esp32C3`: today only `run_until` /
   `run_within` are public, the hart is private, and nothing exposes x1..x31, pc or CSRs (M:
   `machine.rs` public fns 231–946; `Hart` fields `x`, `pc`, `csr` are public only on the hart,
   `hart.rs:70-72`). The same API serves SP4 (GDB stepping) and the SP2 debugger panel (R).
2. A **lockstep timing mode**: a `TimingModel` whose device time is n × 8 ns (rational 32 ticks per
   25 instructions, or a time-base change to 4 GHz = lcm(160, 125) MHz — invasive) (R); interrupt
   latency 0; console TX that takes no device time (R).
3. A **QEMU trace reader** (parse `-d cpu` blocks streamed through a pipe, dedupe, `-d int` for async
   traps) and a **comparator** (pc, x1..x31, M-mode CSRs QEMU prints; mask S/H CSRs and QEMU's reset
   values; report first divergence with both states and the 64 previous instructions, spec §4.4).
4. The **divergence policy**: per-row decision among (i) a compat knob in `SocConfig` (CPU on PLL at
   reset, eFuse v1.3 identity, regi2c `0xFFFFFF`, USJ `EP1_CONF` 0), (ii) value injection — at a load
   from a whitelisted MMIO range, adopt QEMU's `rd` from the next block (RNG, timer counters,
   SFDP), (iii) anchor resync (run both sides to the same pc, compare there; e.g. across the PLL
   bring-up). Each whitelist entry must name its table row (R).
5. Stop at the first reset on either side (QEMU resets the whole machine, M: tech_debt :63) and at
   the first access to a peripheral outside QEMU's instantiated list (oracle §1d, M), reported as
   "window end", not as divergence.
6. A CI job (or a step of `reference-boot`, which already holds the flashes in its cache) that
   fetches the pinned tarball and runs the lockstep on `flash_hello.bin` to `app_main` and past the
   first tick; a local recipe and dated result for CrossPoint.

**Size: L** (stepping API S–M; timing mode M; reader+comparator M; divergence policy M–L, it is
where the unknowns are). **Risks:** compat knobs leaking into the shipped model (keep them in a
`#[cfg]`-free config struct, default off, named `QemuCompat`) (R); trace volume — 32 M insns ×
1.1 KB ≈ 35 GB streamed, ~4 min of QEMU at 133 k insn/s (R from M figures); QEMU build choice
(20250817 lost-interrupt bug vs 20260417 extra trace re-entries, M: oracle §1c pitfall 2);
self-loop ambiguity of dedupe (M: pitfall 1); the bundled libs of the tarball on
`ubuntu-24.04` untested outside the IDF container (R).
**CI:** yes for reference firmwares: hello_world to `app_main` ≈ 2.4 M insns ≈ 18 s of QEMU
tracing (M: oracle §1c pitfall 7) + inkemu release build already cached → +1–3 min on
`reference-boot` (45 min timeout, `.github/workflows/ci.yml:122`, M). CrossPoint: local only.

### (b) CoreMark within ±15 % (criterion 5)

**Exists.** Recipe and pins: `espressif/coremark 1.1.0~2` (component hash `1f95044d…`), IDF 5.5.2,
GCC 14.2.0, `-O3`, code in IRAM, 6000 iterations; QEMU capture gives the five CRCs (`seedcrc 0xe9f5`
… `crcfinal 0xa14c`) and ≈ 300 k insns per iteration, 1.8 G instructions in all (M: oracle §3).
Published figures: datasheet v2.4 **483.27** (compiler unstated), example README **409.25**
(GCC 12.2) (M: oracle §3). Timing model: ten class costs, all estimates, `is_calibrated() == false`
(M: `crates/cpu-rv32/src/timing.rs:9-41`); every bus access stalls 0 (M:
`crates/soc-esp32c3/src/bus.rs:283,304,374`); `Bus::fetch_stall` defaults to 0 and the SoC does not
override it (M: `crates/emu-core/src/bus.rs:85`; no impl in `soc-esp32c3`). 160 MHz CPU clock is
modelled (M: `clock.rs:26-43`). Licence analysis done: build in CI, do not commit the `.bin`, label
results "CoreMark-derived" (M: oracle §4).

**Missing.** (1) The build: `build.sh` runs with `--network none` and `IDF_COMPONENT_MANAGER=0`
(M: `tools/reference-firmware/build.sh`), so the managed component must be vendored into
`tools/reference-firmware/coremark/` (Apache-2.0 sources + EEMBC agreement in NOTICE) or fetched in
a separate pinned step (R). (2) A test that asserts `Correct operation validated` and the five CRCs
(CPU correctness, timing-free) — cheap and valuable on its own. (3) A **decision on the reference
score** (483.27 or 409.25; ±15 % gives [410.8, 555.8] or [347.9, 470.6]) and on GCC 14.2 vs the
published compilers (R). (4) Calibration of the class costs and SRAM wait states, with the
`is_calibrated` flip and its CLI line (`crates/emu-cli/src/main.rs:683`, M). (5) Optionally the
retired-instruction count per iteration compared to QEMU's (ties (b) to (a), R).

**Size: M.** **Risks:** one benchmark under-determines ten costs: fit few parameters (taken-branch
penalty, load-use, div) and keep the rest documented, or the model overfits CoreMark (R); the
FreeRTOS tick inside the timed loop (R); runtime 1.8 G insns at 25–30 M insn/s ≈ 60–75 s release
(R from M rate, tech_debt :99). **CI:** yes, in `reference-boot` (build cached by hash, run ≈ 1–1.5 min).

### (c) Record and replay `.inkrec`

**Exists.** The input list is fully specified: ADR 0004 point 1 (ROM SHA-256, profile, starting
flash content, `--strict-mmio` / `--max-cycles` / `--max-time`) and ADR 0006 (USB host state at
power-on, dated console inputs and host changes *as the machine received them*, bounds 1 MiB per
console and 16 384 host changes; no paths, no host time; a replay starts from a content and writes
nothing) (M: `docs/adr/0004…md:14-20,38`, `docs/adr/0006…md:28`). The machine already takes dated
inputs: `send_console(.., at)`, `set_usb_host(at, state)` (M: `machine.rs:723,777`); trace hash
exists (`machine.rs:946`, `emu-core/src/trace.rs`) but mixes PCs only (M: tech_debt :69). `flate2`
and `sha2` are workspace deps (M: `Cargo.toml:21,32`).

**Missing.** A machine-level **journal of accepted inputs** (so a script, a CLI flag or the future
GUI keyboard are recorded alike); a versioned container format (header, input identities, flash
content compressed or SHA-256 + where to find it, profile TOML or id+hash, dated events, expected
outcome: end reason, trace hash, final flash SHA-256); a library crate holding format + driver with
no file I/O (GUI-first: the CLI only reads/writes the file) (R); `inkemu run --record <f>`,
`inkemu replay <f>` (spec §6.4, M) with exit codes; replay verification against the recorded
outcome; the criterion 6 test expressed on a session; extending the trace hash to state (optional).

**Size: M.** **Risks:** a 16 MB flash per recording (deflate shrinks erased flash to a few hundred
KB, R); `send_console` refusal semantics recorded vs replayed (ADR 0006 bounds); Ctrl-C runs are
non-reproducible by ADR 0004 and must be recorded as such (R). **CI:** yes, fully in `check`
(firmware written in instructions) plus one ignored test on a reference application in
`reference-boot`; seconds (R).

### (d) PMP enforcement, debug triggers, ASSIST_DEBUG stack guard, SENSITIVE permissions

**Exists.** PMP CSRs stored, lock bit honoured, not enforced (D18; M:
`crates/cpu-rv32/src/csr_esp32c3.rs:30-137`); `assist_debug.rs` (235 lines) and `sensitive.rs` (115)
store registers; sources 54 and 55–60 never raised (M: tech_debt :61); arch-test `pmp32` excluded
with the re-enable recipe (M: `tools/conformance/README.md:25-30`,
`tools/conformance/run-arch-test.sh:24-29`, `versions.env` `ARCH_TEST_EXPECTED`/`_TOTAL`);
`rv32mi-p-pmpaddr` skipped (M: `riscv-tests-skip.txt:4`). No user mode: `mstatus.MPP` wired to 3
(M: tech_debt :61).

**Missing.** PMP check on fetch/load/store (in M-mode only locked entries apply, R); `rvtest` /
RISCOF DUT on a PMP-capable hart config; `pmp32` count; trigger CSRs (`tselect`/`tdata1-2`);
SP-bounds monitor raising source 54; PMS violation sources 55–60; a test firmware that faults.

**Size: M** for PMP + `pmp32`, **M–L** with the rest. **Risks:** `pmp32` tests probably exercise
U-mode accesses, which the hart lacks → U-mode support would enlarge the scope (R, check the suite);
a wrong enforcement breaks the boot of every reference firmware (ESP-IDF locks PMP entries early,
M: oracle §1b "ignoring pmpcfg write - locked") — the existing CI firmwares catch it (R); ESP-IDF's
memprot (PMS) on C3 is on by default (R), so a wrong PMS model turns working firmware into panics.
**CI:** yes (`arch-test` job, Docker RISCOF; reference-boot regressions).

### (e) CI hygiene

**Exists.** Five jobs; clippy via `tools/ci/clippy.sh`; exact ignored-test counts
(`tools/ci/run-ignored-tests.sh`) (M: `ci.yml`). Known debts: 93 doc warnings (§0.7), static cache
key `esp-rom-elfs-20260528`, actions pinned by tag (`dtolnay/rust-toolchain@master`,
`Swatinem/rust-cache@v2`, `actions/cache@v6`), `espressif/idf` by tag not digest, container
hardening (M: `ci.yml`, tech_debt :70).

**Missing.** Fix the 93 warnings (mostly escaping `\[9:0\]`, R), add `RUSTDOCFLAGS="-D warnings"
cargo doc …` to `check`; pin actions and the IDF image by digest; coverage per crate.
**Size: S.** **Risk:** none notable; `--document-private-items` makes the gate stricter than
needed — keep it, it is what the debt was measured with (R). **CI:** yes, +1–2 min on `check`.

### (f) Line-faithful flash cache and fetch-stall cost (ADR 0002)

**Exists.** Page-granular (64 KiB) stale-window diagnostic on CPU reads, no eviction, silent in five
cases and speaking in ten where silicon would return flash bytes (M: tech_debt :58, ADR 0002:35);
`extmem.rs` 1155 lines, `mmu.rs` 231 (M); `fetch_stall` = 0 (M).

**Missing.** A set-associative ICache model (16 KB per spec §4.2; ways and line size from the TRM,
R: 8-way, 32 B), either *tag-only* (hit/miss for timing; content stays the live view of ADR 0002) or
*content-faithful* (stale lines returned) — the latter reverses ADR 0002's live-view decision; the
miss cost (one DIO read of a line at 80 MHz, ≈ 150 SPI clocks ≈ 300 CPU cycles at 160 MHz, R); the
reference firmware that maps, writes and re-reads a partition (tech_debt :59).

**Size:** tag-only + stall **M**; content-faithful **L**. **Risks:** no oracle — QEMU has no cache
(finding 4), CoreMark runs from IRAM (finding 5); only hardware calibrates it; any miss cost
moves every pinned trace-hash-independent timing value (log timestamps, the 2000..3000 ms window of
`deep_sleep_timer`) (R). **CI:** functionally yes, but nothing in CI can prove it right.

## 2. Dependencies

```
(e) CI hygiene ─────────────────────────────── independent
stepping/introspection API ──► (a) lockstep ──► (b) optional: insn count per CoreMark iteration
lockstep timing mode (rational 8 ns) ──► (a)
(b) build + CRC test ── independent; (b) calibration ── after timing-model decisions
(c) replay ── needs only the ADR 0004/0006 inputs (all present) + an input journal in the machine
(d) PMP ── independent; benefits from (a) as arbiter only up to QEMU's own generic PMP (§1d)
(f) cache ── no verifier in SP1; its miss cost would perturb (b) only for flash-resident code
```
(a) and (b) both touch `TimingModel`: do (a)'s lockstep mode first, as a second model beside
`DatasheetTiming`, so (b)'s calibration edits one model without breaking the other (R).

## 3. Recommended split (ordered)

| Plan | Content | Milestone | Acceptance test | Size | CI |
|---|---|---|---|---|---|
| **SP1b-4a** CI and doc hygiene | fix 93 rustdoc warnings; `cargo doc -D warnings` in `check`; pin actions + IDF image by digest; cache keys from versions | the doc gate is green | CI `check` runs `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items` and passes; a test or `grep` fails on an unpinned `uses:` | S | yes |
| **SP1b-4b** Recording and replay | input journal in `Esp32C3`; library crate for `.inkrec` (no I/O); `inkemu run --record`, `inkemu replay`; outcome verification | a scripted console run replays bit for bit | record `uart_select` driven by `--console-script` → `inkemu replay` gives the same trace hash, end reason and final flash SHA-256, twice (criterion 6 on a session); a tampered input is refused by name | M | yes |
| **SP1b-4c** Lockstep foundations | stepping/introspection API; `QemuIcount` timing model (8 ns/insn), latency 0, instant console TX; `-d cpu` reader + comparator + report (64 previous insns); window-end rules | ROM + bootloader of `flash_hello.bin` in lockstep with QEMU | CI: zero unexplained divergence from reset to bootloader entry (insn ≈ 521 517, M: oracle §1b), every masked/injected/resynced point tied to a numbered row; a planted bug (e.g. flip one ALU result) is reported at the right instruction | L | yes (+1–2 min) |
| **SP1b-4d** Lockstep through the application | divergence policy for the app (RNG, timers, USJ, eFuse, regi2c); async-interrupt alignment; CrossPoint local recipe | hello_world to `app_main` and past the first tick; CrossPoint to its window end | CI: `flash_hello.bin` from reset to ≥ 2.5 M insns (past the first timer interrupt) with no unexplained divergence; local dated run: CrossPoint from reset to its first access to an unemulated peripheral (≈ 32 M insns, ≤ 50 M) — criterion 3 | L | hello: yes; CrossPoint: local |
| **SP1b-4e** CoreMark | vendored component build; CRC test; reference-score decision; calibration of a few costs; `is_calibrated` | criterion 5 | CI: `Correct operation validated` + 5 CRCs; score within ±15 % of the chosen published figure; retired insns per iteration equal to QEMU's (if 4c is in) | M | yes (+1–2 min) |
| **SP1b-4f** PMP enforcement | PMP checks (M-mode locked entries); RISCOF DUT with PMP; re-enable `pmp32` | `pmp32` passes | `run-arch-test.sh` with `pmp32=<n>` in `ARCH_TEST_EXPECTED`; all reference-boot firmwares still boot unchanged (trace hashes) | M (L if U-mode) | yes |

Order rationale: 4a is a quick, independent win and makes later docs gate-checked; 4b closes a
spec item with no unknowns and gives SP2 its session file early (GUI-first: recording an
interactive session is an SP2 feature); 4c/4d carry the main risk and should start before
calibration so that `TimingModel` grows a second implementation first; 4e after 4c; 4f last, it
is the most deferrable (R).

## 4. What could leave SP1, judged against §10.3

| Item | Defer to | Justification |
|---|---|---|
| (f) line-faithful cache content + eviction | **SP8** (time and performance) | No acceptance criterion names it; QEMU cannot verify it (finding 4); hardware can. ADR 0002 should be amended: its "verified by lockstep" premise is false (M: ADR 0002:35 vs oracle §1d). |
| (f) fetch-stall cost (tag-only cache) | **SP8** | Spec §4.2 asks for it, but criterion 5 is insensitive to it (CoreMark in IRAM, finding 5) and no oracle calibrates it in SP1; adding it would move pinned device-time values for no checkable gain (R). |
| (d) ASSIST_DEBUG stack guard, PMS/`SENSITIVE`, debug triggers | **SP4** (triggers belong with GDB) / **SP2** (stack guard and PMS once CrossPoint runs further and can overflow) | Criterion 1 is RV32IMC + Zicsr (92 tests pass, M: `versions.env`); none of these is in a criterion; they only change buggy-firmware behaviour. |
| (d) PMP enforcement + `pmp32` | keep in SP1 if no U-mode needed, else **SP4** | PMP is privileged-spec, arguably outside "RV32IMC + Zicsr"; §10.2 item 5 asks for "CSR et mécanisme d'interruption spécifiques au C3", which storage already meets (R). |
| CrossPoint lockstep in CI | none (local, dated) | binary not distributable (M); same precedent as criterion 2. |

Not deferrable: (a) criterion 3 and §10.2 item 10; (b) criterion 5 and item 11; (c) item 9 and
criterion 6 ("same session"); (e) is cheap.

## 5. Decisions to put to the user / spec owner

1. **Criterion 3 wording**: accept "lockstep under a declared QEMU-compatibility configuration and a
   numbered whitelist (mask / inject / resync), window ending at the first reset or the first
   access to a peripheral QEMU does not instantiate" — or require literal same-state lockstep, which
   finding 1 makes impossible without making inkemu boot like QEMU (PLL at reset, no USJ console).
2. **Which published CoreMark score** (datasheet 483.27, compiler unstated; or example 409.25,
   GCC 12.2) and whether GCC 14.2 from IDF 5.5.2 is acceptable.
3. **Where compat knobs live**: a `QemuCompat` part of `SocConfig` in the library (usable by a GUI
   "oracle" view) vs only in the dev-tool crate via injection (R: the first is simpler, the second
   keeps the shipped model clean).
4. **QEMU build**: tarball 20260417 (C3 interrupt fix, more duplicate re-entries) vs the
   IDF-pinned 20250817 (no extra download, lost-interrupt bug when `MIE=0`).
5. **Deferrals of §4** and the amendment of ADR 0002.
6. **`.inkrec` and the flash**: embed the compressed content always, or SHA-256 + locator (ADR 0004
   allows both); recommended: embed (self-contained sessions for bug reports, R).

## 6. Fit with the post-SP1 goal (X4 SP2, X3 SP3, S3 SP7, everything from the GUI)

- The stepping/introspection API of 4c is the GUI's pause/step/registers panel and SP4's GDB stub
  base: put it in `soc-esp32c3` as a library API, not in the lockstep tool (R).
- `.inkrec` format + driver in a library crate; the CLI only opens files — the SP2 host crate then
  adds "record this session" / "replay" buttons without moving code (memory `feedback_gui_first`).
- The lockstep comparator stays a dev tool (spec §3.4: QEMU is test-only, GPL, never linked) (M:
  spec :119); keep the QEMU trace parser out of the distributed binary.
- SP7 (S3) has no QEMU riscv32 counterpart (S3 is Xtensa, QEMU's xtensa build exists, M: oracle
  §1a): design the comparator around a target-neutral register-file trait so it can be reused (R).
- Visual emulation (SP2) needs none of (d)/(f); deferring them does not delay the X4 screen (R).
