# CoreMark on the emulated ESP32-C3 (SP1b-4e), and the two hand-overs of SP1b-4d

Date 2026-10-15, branch of SP1b-4e from `main` at `abbdaeb`. Tags: **M** = measured or read (a
file, a line, a command's output), **R** = reasoning. Everything from Espressif's toolchain ran in
`espressif/idf:v5.5.2` (local tag `inkemu/esp-idf`), one container at a time; nothing third-party
ran on the host. Earlier work this report builds on: `docs/research/sp1b-test-oracle.md` §3–§4
(the recipe, a first QEMU run, the licence reading) and `docs/research/sp1b-4-scoping.md` §1(b).

## 0. Findings

1. **inkemu scores 424.628450** with its default timing model (`datasheet-uncalibrated`): 6000
   iterations in 14 130 ms of device time, the five CRCs of the 2K performance run, "Correct
   operation validated" (M, §4). That is **−12.1 %** from the datasheet's 483.27 and **+3.8 %**
   from the example's 409.25: inside ±15 % of both. Nothing in the timing model had to change.
2. **The published score is the datasheet's**: ESP32-C3 Series Datasheet v2.4 (2026-05-06), page 3,
   "1 core at 160 MHz: 483.27 CoreMark; 3.02 CoreMark/MHz", compiler and flags not stated (M, §1).
3. **The criterion bounds an average, not a model.** On CoreMark's instruction mix the band
   [410.78, 555.76] holds any model whose average cost is between about 0.97 and 1.31 cycles per
   instruction (R from M, §4.3). A model that prices every instruction at one cycle scores about
   537 and passes. The test checks both bounds, but a model that costs at least one cycle per
   instruction cannot reach the upper one: only the lower bound can fail such a model.
4. **The build is offline and reproducible**: the component and the example are copied unmodified
   into `tools/reference-firmware/coremark_example/`, each file checked against the registry's
   `CHECKSUMS.json`; two clean builds gave the same `coremark_example.bin` (M, §3).
5. **Licence**: the EEMBC sources are Apache-2.0; the CoreMark Acceptable Use Agreement governs the
   trademark only, and forbids its use with a modified copy (clause 1.2). Copying the sources
   unmodified into the repository is allowed; inkemu's output is not a CoreMark result of a device
   and is never presented as one (M, §2).
6. **Hand-over of SP1b-4d, `MEIP` in `usb_serial_jtag_echo`: the hypothesis of SP1b-4d is refuted,
   the cause is deduced, not observed.** Established (M): the pinned QEMU's USB-Serial-JTAG has no
   interrupt output at all. Deduced from QEMU's code and the state at the stop, not observed in
   QEMU (R, §6.2): its interrupt matrix keeps a sticky pending bit for a source that rose while the
   CPU could not take it, its CPU leaves `MEIP` up while the matrix reports one, and the `MEIP` QEMU
   shows after `csrs mstatus` would be that stale bit, not a state of inkemu. Observing it in QEMU
   (monitor or gdbstub) is left to sub-project 8. Row 4 of the lockstep table cites a newer branch
   of QEMU than the build the lockstep runs (M); the same reading accounts for the mechanism it
   calls "not identified" (R).
7. **Hand-over of SP1b-4d, the tick two instructions late in QEMU: one of the two is explained by
   reasoning on QEMU's code, not measured.** QEMU anchors the periodic alarm at its virtual clock
   read during the store (M, the code), which QEMU's count of a block's instructions under `-icount`
   makes include the store (R); inkemu anchors at the start of the store. The other instruction is
   not in `esp_systimer.c` (M, §6.1).

## 1. The published scores

| # | Source | Score | Conditions | Status |
|---|---|---|---|---|
| P1 | ESP32-C3 Series Datasheet **v2.4** (2026-05-06), page 3, "CPU and Memory"; PDF `https://www.espressif.com/sites/default/files/documentation/esp32-c3_datasheet_en.pdf`, SHA-256 `833fc000b4b3c3d39c496fcbd597fed5806956503ce7390b19cc8ae82f19f968` (downloaded again on 2026-10-15, the same file as on 2026-10-11) | **483.27** (3.02 /MHz) | one core at 160 MHz; compiler, flags, memory placement and iterations not stated | the chip's score, Espressif's own |
| P2 | `espressif/coremark` 1.1.0~2, `README.md` of the component and of `coremark_example` | **409.249028** | 160 MHz, GCC 12.2.0, global `-Og` then `-O3 -fjump-tables -ftree-switch-conversion` on CoreMark's sources, code in IRAM, 6000 iterations, 14.661 s | an example's output, reported per CoreMark's rules |

The datasheet's revision history says the figure was added in v1.1 (2021-10-26) and **updated** in
v2.1 (2025-04-14): "Updated CPU CoreMark® scode [sic] in Section Product Overview" (M, page of the
revision history). ESP-IDF moved to GCC 14.2.0 with v5.4 (2024-12); the update of 2025-04 comes
after it, and 3.02/MHz is well above the 2.56/MHz of GCC 12.2 in P2. That the datasheet's figure
was measured with GCC 14 is an inference, not a statement of Espressif (R).

What the score is measured on: CoreMark's port times the run with `esp_timer_get_time()`
(`port/core_portme.c`: `GETMYTIME` = `esp_timer_get_time() / 1000`, `EE_TICKS_PER_SEC` 1000), that is
the SYSTIMER on the ESP32-C3. Under inkemu the SYSTIMER counts device time (16 MHz from the time
base, `soc_esp32c3::periph::systimer`): **the score the firmware prints is the one inkemu's device
time gives** (M).

## 2. Licence and what it allows

- `components/coremark/coremark/LICENSE.md` (identical to the component's `LICENSE`, SHA-256
  `9577b9c8…`): the **CoreMark Acceptable Use Agreement** of EEMBC, then the Apache License 2.0. The
  agreement licenses the **trademark** COREMARK (Reg. No. 4,179,307) "solely … in connection with the
  generation of data regarding measurement and analysis … via the Software"; clause 1.2: "Licensee
  shall not use the Trademark in connection with any use of a modified, derivative, or otherwise
  altered copy of the Software"; clause 2.1: display "appropriate notices regarding ownership of the
  Trademark" (M).
- The sources carry "SPDX-License-Identifier: Apache-2.0" (EEMBC 2018; Espressif's port 2023): they
  can be redistributed in source form with their licence (M).
- The README's run and reporting rules (M, `coremark/README.md`, "Run Rules", "Reporting rules"): a
  result is valid only if the run lasts at least 10 s; the benchmark's files are unmodified apart
  from the port; a report gives the iterations per second, the compiler and its flags, and the
  memory placement; the MD5 sums of `coremark.md5` identify the unmodified files.

What follows for this repository (R):

1. The component's files may be **copied unmodified** into `tools/reference-firmware/` with their
   `LICENSE` files: Apache-2.0 allows it; the agreement does not restrict copies, only the mark.
   The copies match `coremark.md5` (M: `md5sum` of the five sources and `coremark.h`).
2. **No CoreMark binary is committed** (`SHA256SUMS.inputs-coremark` pins it; the CI builds it), as
   for every reference firmware but the shipped bootloader.
3. What inkemu prints is the output of an emulated chip: it is **not a CoreMark score of a device**,
   is not reported as one, and no inkemu text presents it as inkemu's "CoreMark score". The mark is
   named with its owner (`PROVENANCE.md`, both READMEs).
4. The root `NOTICE` is printed by `inkemu licenses` and lists what the **product** includes; the
   CoreMark sources are in the repository, not in the product, so the root `NOTICE` does not change.

## 3. The build

Recipe, run on 2026-10-15 in `espressif/idf:v5.5.2` (`ESP_IDF_VERSION` of `versions.env`), without
network and with `IDF_COMPONENT_MANAGER=0`, like every reference firmware:

- **Sources**: the registry release `espressif/coremark` 1.1.0~2 (2025-07-12, component hash
  `1f95044d7fe042d997b7715e0dc9753e65c6724158c2d028997f7816df1329d7`, from
  `espressif/idf-extra-components` at `746e83ddbea0db9c3d24993a87c4c737a60337ae`): archive
  `espressif__coremark-v1.1.0_2.zip`, SHA-256 `b7ba74c9…f7b0af`, downloaded on the host as data, each
  kept file checked against `CHECKSUMS.json` of the release (M: 20 files, all equal).
- **Layout** (`tools/reference-firmware/coremark_example/`): the example's `CMakeLists.txt`,
  `sdkconfig.defaults` (`CONFIG_ESP_TASK_WDT_INIT=n`), `main/`; the component in
  `components/coremark/`, where ESP-IDF finds it as `coremark`, the name the example's
  `PRIV_REQUIRES` gives. Left out: the example's `idf_component.yml` (it asks the manager for
  `espressif/coremark: '*'`, unpinned), documentation and other ports.
- **Configuration**: `tools/reference-firmware/sdkconfig.defaults` (16 MB DIO flash at 80 MHz,
  160 MHz CPU, reproducible build), then the example's own file, as for every example.
- **Result**: GCC 14.2.0 (`esp-14.2.0_20251107`); `coremark_example.bin` 144 752 bytes, SHA-256
  `1de13ef8c2640668640eb52a12f6b015f9efbdd55d3c28109ae640c8f42fbdbf`; `size`: text 111 254, data
  33 376, bss 126 132; bootloader and partition table equal to those of `hello_world` (`601b4845…`,
  `7f00b6c0…`). Built twice from clean, once by hand and once by `build.sh` with every other
  reference firmware: the same hashes (M). 31 s of the build in the container.
- `in-container.sh` checks the copied files against `SHA256SUMS.coremark-sources` before it builds.

The binary differs from the one of `sp1b-test-oracle.md` §3 (`a8f8dcec…`, built through the
component manager): the component's path, and thus its name, differ (`espressif__coremark` there,
`coremark` here); the code is the same (R).

## 4. Measurement

### 4.1 The run

`inkemu run --device x4 --console uart0 --max-time 20 --trace-hash --firmware coremark_example.bin`
(release build of this branch, ROM rev3):

```text
Running coremark...
2K performance run parameters for coremark.
CoreMark Size    : 666
Total ticks      : 14130
Total time (secs): 14.130000
Iterations/Sec   : 424.628450
Iterations       : 6000
Compiler version : GCC14.2.0
Compiler flags   : -ffunction-sections -fdata-sections -gdwarf-4 -ggdb -nostartfiles -nostartfiles -Og -fno-shrink-wrap [35 × -fdebug-prefix-map=…] -fstrict-volatile-bitfields -fno-jump-tables -fno-tree-switch-conversion -std=gnu17 -O3 -fjump-tables -ftree-switch-conversion
Memory location  : IRAM
seedcrc          : 0xe9f5
[0]crclist       : 0xe714
[0]crcmatrix     : 0x1fd7
[0]crcstate      : 0x8e3a
[0]crcfinal      : 0xa14c
Correct operation validated. See README.md for run and reporting rules.
CoreMark 1.0 : 424.628450 / GCC14.2.0 -ffunction-sections -fdata-sections -gdwarf-4 -ggdb -nostartfiles -nostartfiles -Og -fno-shrink-wrap [35 × -fdebug-prefix-map=…] -fstrict-volatile-bitfields -fno-jump-tables -fno-tree-switch-conversion -std=gnu17 -O3 -fjump-tables -ftree-switch-conversion / IRAM
CPU frequency: 160 MHz
I (17532) main_task: Returned from app_main()
```

The extract starts at `Running coremark...` (the boot before it is left out) and is the console
otherwise whole, but for one shortening, marked `[35 × -fdebug-prefix-map=…]` in the two lines that
carry the flags: the 35 options `-fdebug-prefix-map=<path>=<name>` the reproducible build adds (from
`/opt/esp/idf=/IDF` to the toolchain's `esp-14.2.0_20251107` directory), which only rename paths
in the debug information. The CoreMark sources are compiled at `-O3` (the last `-O` wins over the
global `-Og`), with jump tables, code in IRAM.

Standard error: no warning; `stopped … after 2792650730 cycles (2208654520 instructions)`, 20 s of
device time, `CPU clock at 160 MHz`, `trace-hash bfb20978732b673f`. The same run to 40 s gave the
same score (M). Durations, measured: the run alone 110 s of wall time on the development host
(about 20 M instructions/s); the test that runs it 87 s and 102 s on that host (two runs, load
differing), and **68 s in the CI** (job `reference-boot` of PR #14, "finished in 68.26s").

The CRCs are those Espressif's example prints (P2), and `crcfinal` 0xa14c is that of 6000
iterations. CoreMark chooses its iterations from its own timing (`core_main.c`: 1, 10, 100, 1000
until a second has passed, then `1000 × (1 + 10 / seconds)`): 1000 iterations take about 2.35 s
here, as on the chip, hence 6000. A model 1.18 to 2.35 times faster (1000 iterations in 1 to 2 s)
would run 11 000 iterations, with another `crcfinal` (R).

### 4.2 The gap

| Reference | Score | inkemu | Gap | ±15 % band |
|---|---|---|---|---|
| P1, datasheet | 483.27 | 424.628 | −12.1 % | [410.78, 555.76] |
| P2, example | 409.25 | 424.628 | +3.8 % | [347.86, 470.64] |

### 4.3 What the band constrains

Over the run, inkemu's model costs 2 792 650 730 cycles for 2 208 654 520 instructions: **1.264
cycles per instruction**, almost all of it CoreMark (M; R for "almost all": the boot is about 3 M
instructions). With the instructions fixed by the binary, the score is inversely proportional to the
average cost, so the datasheet's band is an average between **0.966 and 1.307** cycles per
instruction on this mix, and the datasheet's own figure corresponds to 1.11 (R). The upper bound is
out of reach of a single-issue core; a model at one cycle per instruction (`qemu-icount`'s costs at
160 MHz) scores about 537 and passes. The QEMU run of `sp1b-test-oracle.md` §3 implies about
300 000 instructions per iteration with GCC 14.2 (M there), consistent with 2.2 G instructions here.

A mutation shows the lower bound is a real constraint: with `ALU` at 2 cycles instead of 1 in
`cpu-rv32/src/timing.rs`, the score falls to 295.53 (−38.8 %) and the test fails on the band
(M, the change undone at once).

### 4.4 Why no cost was fitted

The scoping report foresaw "a calibration of a few costs" (`sp1b-4-scoping.md` §3). The score is
inside the band without one, and one benchmark gives one equation for ten class costs: a fit would
choose among infinitely many models on no evidence but the datasheet's figure, whose compiler is
unknown, and it would move every pinned trace hash for no checkable gain. The fit belongs to the
calibration bench (sub-project 8), with a device (R; ADR 0009).

## 5. What the test holds

`crates/emu-cli/tests/run_cli.rs`,
`x4_runs_the_coremark_reference_firmware_within_15_percent_of_the_published_score` (ignored; job
`reference-boot`): the pinned SHA-256; exit 0; in order the run's header, size 666, IRAM, the four
CRCs that do not depend on the iterations, "Correct operation validated", 160 MHz, the return of
`app_main`; no `ERROR!`; `crcfinal` 0xa14c when the run has 6000 iterations; at least 10 s; the
score of the `CoreMark 1.0` line equal to `Iterations/Sec` and to iterations × 1000 / ticks; the
score within 15 % of 483.27 (the message gives both gaps); no warning; the trace hash.

## 6. The two hand-overs of SP1b-4d, read in the pinned QEMU

Sources downloaded on 2026-10-15 from `espressif/qemu`, tag **`esp-develop-9.2.2-20250817`**, the
source of the build the lockstep runs (`QEMU_ESP_BUILD=esp_develop_9.2.2_20250817`): SHA-256 of each
file, `hw/timer/esp_systimer.c` `f653119f…`, `hw/timer/esp32c3_systimer.c` `6f47915e…`,
`hw/riscv/esp32c3_intmatrix.c` `db5bf9e5…`, `target/riscv/esp_cpu.c` `59c3d76d…`,
`hw/misc/esp32c3_jtag.c` `0c62c7e2…`, `accel/tcg/icount-common.c` `d72c1515…`. For comparison,
`hw/riscv/esp32c3_intmatrix.c` of the branch `esp-develop` (newer than the tag).

### 6.1 The first tick, two instructions later in QEMU

- `esp_systimer_conf_set` (lines 374–427): a comparator already enabled whose counter is started
  is reprogrammed (`esp_systimer_comparator_reprogram`, lines 105–154): `now =
  qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL)`, and in period mode `target_ns = now +
  TICKS_TO_NS(period)` with `TICKS_TO_NS(t) = (t / 16) * 1000` (line 25): for the tick's 160 000
  counts, exactly 10 000 000 ns (M).
- Under `-icount`, the virtual clock at an I/O access counts the instructions of the current block,
  decremented at its start (`icount_get_executed`, lines 71–75; `icount_get_raw_locked`, 105–126),
  and an I/O access is the last instruction of its block: `now` includes the store (M for the code;
  R for "includes", from QEMU's block accounting). inkemu anchors the alarm at the device time the
  store starts (`Systimer::write_conf`, an `ASSUMPTION:`): **one instruction, by reasoning, not
  measured**.
- The other is not in `esp_systimer.c`: 10 ms is an exact number of 8 ns instructions, and the
  alarm is not rounded to a count of the counter (M). Candidates left: the deadline rounded up to
  an instruction by `icount_round` (lines 224–228) when the clock's bias is not a multiple of 8 ns,
  which a warp of the clock to another timer's deadline during a `wfi` (`sleep=off`) can leave; or
  something else (R). The SP1b-4d rule stands: a setting of `QemuCompat` only for a gap sourced in
  `esp_systimer.c`.

### 6.2 `MEIP` after `csrs mstatus` in `usb_serial_jtag_echo`

Measured on 2026-10-15 with a scratch window (not versioned) holding every row of the table, on
the application composed behind the shipped bootloader: divergence after 2 525 511 compared
instructions, at `csrs mstatus, t0` (`0x4038025c`) of `_interrupt_handler`, `mcause` 0x80000003
(line 3), QEMU `mip` 0x800, inkemu 0, "QEMU took no trap after it" (M; the count of SP1b-4d).
inkemu's controller at that point (relevé of its map writes, scratch, not versioned): line 3 is the
software interrupt (`FROM_CPU_INTR0`, source 50), priority 1; the handler has just stored the
threshold, priority of line 3 plus 1, so 2 (`lw t2` of `CPU_INT_PRI_3`, `addi t2, 1`, `sw` to
`CPU_INT_THRESH`, M from the 64 instructions before).

- `hw/misc/esp32c3_jtag.c` has **no interrupt output** (no `sysbus_init_irq`; reads 0, ignores
  writes): QEMU's USB-Serial-JTAG source is always low (M). The hypothesis of the hand-over note
  and of the table's paragraph ("a source high in QEMU and low in inkemu") is refuted.
- `target/riscv/esp_cpu.c`: every line of the matrix drives one parent interrupt, `IRQ_M_EXT`
  (line 304), i.e. `mip.MEIP`; it is raised when a line is taken (`esp_cpu_irq_handler`, 156–166)
  and **not lowered when the interrupt is accepted** (`esp_cpu_exec_interrupt`, 172–222); it is
  lowered only by an interrupt check with `MIE` set, when the matrix's callback says nothing is
  pending (178–201) (M).
- `hw/riscv/esp32c3_intmatrix.c` of the tag: a line is **pulsed** (33–36); a source that rises
  while the CPU cannot take it, or below the threshold, sets a **pending** bit (107–117); that bit
  is cleared only when the source falls while mapped to the line, or the line is disabled (59–69,
  118–120, 186–189), not when the interrupt is taken (171–173). The callback run when `MIE` comes
  back (`esp32c3_intmatrix_mie_enabled`, 209–216) pulses the highest pending line whose priority
  reaches the threshold, and **returns whether any pending bit is set**, whatever its priority (M).

Reading of the stop (R): the software interrupt of line 3 was requested while the CPU could not
take it (inside another handler: the run took one interrupt before and had one block preempted),
so its pending bit is set; QEMU takes line 3 when `MIE` returns, keeps the bit (the source stays
high until the handler of line 3 clears it, later), and at `csrs mstatus` the callback finds line 3
pending below the new threshold: no pulse, no trap, but "pending", so `MEIP` stays up. inkemu's
controller does not show its lines through `mip` (row 4) and shows 0. On this reading the bit comes
from how the pinned QEMU funnels its lines into `MEIP` and keeps its pending bits, not from a state
of inkemu. **This is deduced from QEMU's code and the state at the stop, not observed in QEMU**: the
pending bit was not read out of QEMU (its monitor or gdbstub could; left to sub-project 8). The only
established fact of this section is the refutation of the USB-Serial-JTAG hypothesis.

Row 4 of `docs/lockstep-differences.md` describes QEMU's `MEIP` as the level of a line enabled,
high and at or above the threshold, citing `esp32c3_intmatrix_line_should_assert` and
`esp_cpu_update_parent_irq`: those functions are in the branch `esp-develop`, **not in the tag the
lockstep runs** (M). Its "three instructions after a store of the threshold that brings the line
down, mechanism not identified" are, on the same reading, that stale `MEIP`: up from the interrupt taken until the
check after `csrs mstatus` (store of the threshold, `fence`, `li`, `csrs`) (R, consistent with the
1 112 × 3 measured). The row stays as it is — its measure holds in the three windows of the CI —
and its source is corrected by a dated revision.

### 6.3 The table's QEMU sources against the tag (review of SP1b-4e, 2026-10-15)

The table said its QEMU sources were those of the branch `esp-develop`. The image
`espressif/idf:v5.5.2` holds QEMU's binary, headers, libraries and data, not its sources (M: the
directory `/opt/esp/tools/qemu-riscv32/esp_develop_9.2.2_20250817/qemu/` has `bin`, `include`,
`lib`, `share`); its binary says `QEMU emulator version 9.2.2 (esp_develop_9.2.2_20250817)` (M,
`--version` run in the image). The sources are read at the tag of that name and compared with the
branch, file by file (SHA-256, first 16 hex digits):

| File | Rows | Tag | Branch |
|---|---|---|---|
| `hw/misc/esp32c3_rtc_cntl.c` | 9, 37 | `3e8f80499b29ac02` | identical |
| `hw/riscv/esp32c3.c` | 17 | `77cac6c5da7b3933` | identical |
| `hw/riscv/esp32c3_clk.c` | 20, 27 | `b78618b8425926e5` | identical |
| `hw/ssi/esp32c3_spi.c` | 28, 32 | `89a425929b4191cb` | identical |
| `hw/misc/esp32c3_cache.c` | 30 | `a29978da537389a4` | identical |
| `hw/timer/esp32c3_systimer.c` | 15, 24, 39 | `6f47915ee122adf6` | identical |
| `hw/timer/esp_systimer.c` | 15, 24, 39 | `f653119f127f212b` | debug traces only (`SYSTIMER_DEBUG`) |
| `target/riscv/esp_cpu.c` | 2, 4, 5 | `59c3d76d415e3c75` | differs (§6.2) |
| `hw/riscv/esp32c3_intmatrix.c` | 2, 4 | `db5bf9e542be64ce` | differs (§6.2) |

Every QEMU function a row's section names is in the tag (M: a search of each), but two:
`esp_cpu_has_work` (row 2) and the two functions of row 4, which are the branch's. Row 2's value
at reset comes, in the tag, from `esp32c3_intmatrix_reset_hold` (lines 345–346: `mie` written
`BIT(12) - 1`); the 0x0EEE QEMU shows is the measure, and that it is what the generic RISC-V core
keeps of 0xFFF was not read. Row 5's functions (`esp_cpu_get_cycles`, `esp_cpu_csr_write`) are the
same in both. No measure, parameter, use or pin changes; the sources of rows 2 and 4 are corrected
by dated revisions.
