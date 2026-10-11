# inkemu SP1b test tooling research: QEMU oracle, bootloader, CoreMark

Date of measurements: 2026-10-11. Host: Linux x86_64, Docker 29. Everything third-party ran inside Docker
(persistent container `oracle`, image `inkemu/esp-idf` = espressif/idf:v5.5.2, mount `/w` = scratch dir
`<scratch>/oracle`).
The repository was not modified. All artefacts below live in that scratch dir.

Practical note: every `docker run inkemu/esp-idf ...` takes about 2 min because the entrypoint re-runs the IDF
activation (python dependency check). Use a persistent container started with `--entrypoint sleep ... infinity`
and `docker exec` after sourcing `export.sh` once (env dumped to `idfenv.sh`); then each call costs ~1 s.

---------------------------------------------------------------------------------------------------

## 1. QEMU oracle

### 1a. Releases, assets, availability

- Latest release of `espressif/qemu`: `esp-develop-9.2.2-20260417` (published 2026-04-19). Previous:
  `esp-develop-9.2.2-20250817`, `...-20250228`, `esp-develop-9.0.0-20240606`, ...
- Assets of the latest release for riscv32 (GitHub publishes NO digest: `digest` is null for all assets):

  | Asset | Size |
  |---|---|
  | `qemu-riscv32-softmmu-esp_develop_9.2.2_20260417-x86_64-linux-gnu.tar.xz` (the one we want) | 16,842,920 B |
  | `...-aarch64-linux-gnu.tar.xz` | 16,325,180 B |
  | `...-x86_64-apple-darwin.tar.xz` / `aarch64-apple-darwin` | 3,763,576 / 3,763,728 B |
  | `...-x86_64-w64-mingw32.tar.xz` | 38,496,020 B |
  | xtensa-softmmu variants (ESP32/S2/S3), x86_64 + aarch64 linux | ~15.6 / 15.1 MB |

  I downloaded the x86_64-linux one into `dl_qemu_latest/` : SHA-256
  `547f03e04701a92cbb699f7f7d015adc1f5b5ef93cbb94c0dd9b7107e2d84e77` (pin this in CI, since GitHub gives none).
  Contents: `qemu/bin/qemu-system-riscv32`, `qemu/lib/x86_64-linux-gnu/` (bundled libs), `qemu/share/qemu/{esp32c3-rom.bin, esp32s3_rev0_rom.bin, esp32-v3-rom*.bin}`, `qemu/include/`.
  Self-contained: it ran inside the IDF container unchanged.
- Release notes of 20260417 relevant to us: "Fixed interrupts being lost or delayed on ESP32-C3 when MIE=0
  (PR 148)", "Fixed GDMA to check descriptor ownership...".
- **No Docker image needed**: the `inkemu/esp-idf` image (IDF v5.5.2) already ships QEMU:
  `/opt/esp/tools/qemu-riscv32/esp_develop_9.2.2_20250817/qemu/bin/qemu-system-riscv32` (on PATH after `export.sh`),
  `QEMU emulator version 9.2.2 (esp_develop_9.2.2_20250817)`, `-machine help` lists `esp32c3  Espressif ESP32-C3 machine`.
  IDF pins this one (older than the latest). `idf_tools.py install qemu-riscv32` is therefore a no-op there.
  Tradeoff: IDF-pinned 20250817 (reproducible via the image) vs 20260417 (interrupt fix). I tested both (see 1c).
- ROM: comes with the QEMU distribution, `share/qemu/esp32c3-rom.bin` (393,216 B), SHA-256
  `0de1e65020e803bea0d7443dca149d61895e01fca3bb9c82d073234eebd73f99`, byte-identical in 20250817 and 20260417.
  QEMU locates it relative to the binary (`../share/qemu`). Its code region is identical to the code of
  `esp32c3_rev3_rom.elf` from `esp-rom-elfs` (Apache-2.0, shipped in `/opt/esp/tools/esp-rom-elfs/`):
  the first 365,968 bytes (.fixed.text/.init.text/.bt_text/.text, extracted with objcopy) match exactly
  (rev0 and rev101 do not match). So the ROM can be re-derived from an Apache-2.0 source if needed (chip rev v0.3).
  The ROM banner is `ESP-ROM:esp32c3-api1-20210207`.
- Plugin interface: **`-plugin help` -> "plugin interface not enabled in this build"** in both the 20250817 and
  20260417 builds. So TCG plugins are not available without building Espressif's fork ourselves
  (`--enable-plugins`).

### 1b. Running a C3 flash image headless

Assembled image (see section 2 for how):
```
qemu-system-riscv32 -nographic -machine esp32c3 -icount 3 \
    -drive file=flash.bin,if=mtd,format=raw
```
- UART0 goes to stdio with `-nographic`; use `-serial file:/path` to capture only the UART and keep stdio free
  (monitor) ; `-serial null` to discard. "Adding SPI flash device" is printed by QEMU itself before the ROM banner.
- `-icount 3` is mandatory for C3 (1 instruction = 2^3 = 8 ns of virtual time = 125 MIPS-equivalent). `-icount 3,sleep=off`
  and `,align=off` also work. Flash image must be exactly 2/4/8/16 MB (16 MB works).
- The flash model is a generic M25P80 (ISSI): QEMU logs (`-d unimp,guest_errors`) `M25P80: Unknown cmd 7a`,
  `Unknown cmd 5a` (SFDP), `Read id (command 0x90/0xAB) is not supported`, and many
  `ignoring pmpcfg write - locked` (same behaviour as hardware locked PMP).
- Confirmed: it boots the hello_world built in section 2, both with the default single-app partition table and with
  an Xteink-like OTA table (blank otadata, "No factory image, trying OTA 0"). Guest reaches `Hello world!` after ~28 ms of
  virtual time, ~108 ms wall; `app_main` is reached after ~2.40 M instructions (ROM 0.52 M -> bootloader entry
  at insn #521,517, app `call_start_cpu0` at #1,683,260, `app_main` at #2,395,993).
- Boot is deterministic: the first 3,000,000 executed-instruction pcs were byte-identical across two runs (same md5
  `d0478d79cbe29132418221e539e142e0`), including the first timer interrupt (~insn 2.42 M) when using `-icount 3`.

### 1c. Per-instruction architectural trace: what works

All four options tried inside Docker on the hello_world flash image.

| Option | Command | Result |
|---|---|---|
| A. `-d cpu` (full register dump at every TB entry) | `-accel tcg,one-insn-per-tb=on -d cpu,nochain -D out.log` | **Works.** One block per instruction, ~1.1 KB each (43 lines). Includes pc, mstatus, mie, mip, mideleg, medeleg, mtvec, mepc, mcause, mtval, mscratch, satp + x0..x31. ~133,000 insn/s measured streaming to a consumer (`-D /dev/stdout`). |
| B. `-d exec` (pc only) | `-accel tcg,one-insn-per-tb=on -d exec,nochain -D out.log` | Works. ~300,000 insn/s. Line: `Trace 0: 0x<host tb addr> [00000000/<pc>/07014003/ff022201]`. No registers. |
| C. `-d in_asm` | | Useless for tracing: logs only at translation time, not per execution. |
| D. gdbstub | `-gdb tcp:127.0.0.1:1234 -S` + RSP `s`,`g`,`p<n>` | Works, exact retire semantics, but ~6,400-7,100 steps/s (2 round trips per step: the stub ignores any packet sent while the CPU is running, so the `g` cannot be pipelined behind the `s`). 2.4 M insns to `app_main` = ~6 min. |
| E. TCG plugin | | **Not available** (plugin interface disabled in both released builds). |

Addendum 2026-10-11 (from `esp32c3-bootloader-accesses.md` §11): MMIO accesses can be traced without plugins with `-trace 'memory_region_ops_*'`, and QEMU's random number generator makes a register-level lockstep diverge inside the bootloader (first read at instruction ≈ 635,927): mask the affected registers and memory, or inject the values.

`-singlestep` is not needed: in QEMU 9.2 the spelling is `-accel tcg,one-insn-per-tb=on`.

Exact working command (recommended, option A):
```
qemu-system-riscv32 -nographic -machine esp32c3 -icount 3 \
  -drive file=flash.bin,if=mtd,format=raw \
  -accel tcg,one-insn-per-tb=on \
  -d cpu,nochain -D /dev/stdout \
  -serial file:uart.log        # keep UART out of the trace stream
```
Stream (`/dev/stdout` or a FIFO) into the Rust comparator; do NOT write to disk: 2.4 M instructions is ~2.9 GB of text.
`-D` file works too. `-d cpu` also prints the "Adding SPI flash device" line at the top.

Log format (first entry = state at reset, BEFORE the instruction at `pc` executes; paste of the real output):
```
 V      =   0
 pc       40000000
 mhartid  00000000
 mstatus  00000000
 mstatush 00000000
 hstatus  00000000
 vsstatus 00000000
 mip      00000000
 mie      00000eee
 mideleg  00001444
 hideleg  00000000
 medeleg  00000000
 hedeleg  00000000
 mtvec    00000000
 stvec    00000000
 vstvec   00000000
 mepc     00000000
 sepc     00000000
 vsepc    00000000
 mcause   00000000
 scause   00000000
 vscause  00000000
 mtval    00000000
 stval    00000000
 htval    00000000
 mtval2   00000000
 mscratch 00000000
 sscratch 00000000
 satp     00000000
 x0/zero  00000000 x1/ra    00000000 x2/sp    00000000 x3/gp    00000000
 x4/tp    00000000 x5/t0    00000000 x6/t1    00000000 x7/t2    00000000
 x8/s0    00000000 x9/s1    00000000 x10/a0   00000000 x11/a1   00000000
 x12/a2   00000000 x13/a3   00000000 x14/a4   00000000 x15/a5   00000000
 x16/a6   00000000 x17/a7   00000000 x18/s2   00000000 x19/s3   00000000
 x20/s4   00000000 x21/s5   00000000 x22/s6   00000000 x23/s7   00000000
 x24/s8   00000000 x25/s9   00000000 x26/s10  00000000 x27/s11  00000000
 x28/t3   00000000 x29/t4   00000000 x30/t5   00000000 x31/t6   00000000
 V      =   0
 pc       40001e90
 ...
```
(The first instruction at 0x40000000 is `j 0x40001e90`; the second block shows pc=40001e90 and the same state.)
`-d exec` sample: `Trace 0: 0x796160000100 [00000000/0000000040000000/07014003/ff022201]`.
Interrupts: add `-d int` to get `riscv_cpu_do_interrupt: hart:0, async:1, cause:0000000b, epc:0x40381bc8, tval:0x00000000, desc=m_external`
(ordered with exec output when sharing the same `-D`).

Validation performed:
1. gdbstub single-step pcs vs `-d exec` pcs: identical for 140,000 steps once consecutive duplicate entries are removed
   (see pitfall 1).
2. gdbstub full state (pc, x1..x31, mstatus 0x300, mtvec 0x305, mepc 0x341, mcause 0x342) after step k vs the `-d cpu` block
   k+1: **2,500/2,500 identical, 0 mismatches** (script `cmp_cpu.py`). So the `-d cpu` block is exactly the PRE-state of the
   instruction at `pc` = POST-state of the previous instruction.
3. gdbstub details if ever needed: register numbers are `66 + csrno` (mstatus 0x300 -> regnum 834 = hex 0x342, i.e. packet `p342`;
   target XML says `<reg name="mstatus" regnum="834">`, mepc regnum 899), `g` returns only x0..x31 + pc (132 bytes), `qSupported` must be sent first,
   `QStartNoAckMode` is rejected (empty reply), acks needed.

Pitfalls:
1. **Duplicate trace entries.** `-d cpu` / `-d exec` log at TB entry. When the icount budget (<=65,536 insns) is exhausted
   the same TB is re-entered and logged twice without executing twice: 50 duplicates in the first 3 M entries of the 20250817
   build, 710 in the 20260417 build (extra ones around interrupt delivery, e.g. a `fence` after an MMIO store).
   Dedupe consecutive entries with identical full state; this cannot distinguish a genuine `j .` self-loop (only gdbstub can),
   so treat a repeated identical entry as retired only if the instruction at pc is a branch-to-self.
2. **Versions diverge in trace shape**: after dedupe, the first 2.4 M pcs of 20250817 and 20260417 are identical (checked); they
   first differ at the interrupt handler where the new build logs an extra re-entry. Pin one version in CI.
3. **gdbstub masks interrupts while stepping**: default `qqemu.sstep` = 0x7 (ENABLE|NOIRQ|NOTIMER). Send `Qqemu.sstep=0x1` to keep
   IRQs/timers, otherwise a step-trace diverges from a free-running run at the first interrupt (~insn 2.42 M for hello_world).
   Not tested over the interrupt; use only for short windows.
4. **Interrupt timing**: deterministic only with `-icount 3` (virtual clock = 8 ns/insn, independent of the host). Without
   icount the machine refuses/behaves free-running. inkemu must therefore model 1 insn = 8 ns (CPI = 1, no flash/cache wait
   states) when diffing against QEMU, or the first timer interrupt lands on a different instruction. SYSTIMER/TIMG values
   read by the firmware derive from that same clock.
5. `-d cpu` omits CSRs we may care about (mcycle, mtvec is shown; PMP, mscratch shown; custom ESP CSRs not shown).
   Use gdb `p<66+csr>` for any other CSR.
6. Reset state is QEMU's: `mstatus=0`, `mie=0x0eee`, `mideleg=0x1444` (S-mode CSRs exist in QEMU's core; hardware C3 is M/U only).
   Mask/ignore these CSRs, or compare only M-mode ones that the firmware actually uses.
7. Throughput: `-d cpu` ~133k insn/s => ROM->`app_main` window (2.4 M insn) ~18 s; the whole CoreMark run (1.8 G insn) is not
   traceable, compare checkpoints instead (see 3).

Recommendation: **Option A (`-d cpu,nochain` + `one-insn-per-tb=on`, streamed) as the lockstep oracle for windows up to a few million
instructions (ROM -> bootloader -> app_main); gdbstub (script `gdbtrace.py`, ~7k steps/s) only as ground truth for ambiguous
points (self-loops, interrupt entry, CSR not in the dump); `-d exec` for fast pc-only first-divergence search over longer windows.**
Beyond that, compare periodic checkpoints (UART output, final CoreMark crc values). Avoid TCG plugins (unavailable).

### 1d. Legitimate divergences between QEMU esp32c3 and real hardware (and therefore in lockstep)

Instantiated devices (from `info mtree -f`): UART0/UART1 (0x60000000/0x60010000), SPI1 flash controller only (0x60002000),
GPIO (0x60004000), RTC_CNTL (0x60008000), eFuse (0x60008800), TIMG0/1 (0x6001f000/0x60020000), SYSTIMER (0x60023000),
TWAI, AES/SHA/RSA/DS/HMAC, GDMA (0x6003f000), USB-Serial-JTAG (0x60043000), SYSTEM/clk (0x600c0000), interrupt matrix
(0x600c2000), EXTMEM/cache (0x600c4000), XTS-AES, OpenCores Ethernet, an RGB framebuffer at 0x20000000/0x21000000
(QEMU-only extras), ROM, 400 KB SRAM, RTC RAM.
Everything else in the peripheral window is generic `esp32c3.iomem`: **reads return 0, writes silently ignored, nothing logged**
(verified: SPI2 0x60024000, I2C0 0x60013004, LEDC 0x60019000, SAR-ADC 0x60040000 all read 0). Missing: GP-SPI2, I2C0, LEDC, RMT,
APB_SARADC, Wi-Fi/BT/PHY, PSRAM.
Consequences for a lockstep with inkemu:
- Any inkemu peripheral that returns non-zero where QEMU returns 0 will diverge (e.g. inkemu's SPI2/ADC for the e-paper/buttons).
  The oracle comparison is therefore valid only for firmware that does not touch those, or inkemu must mask them.
- Flash: QEMU uses a generic M25P80 (reports ISSI via IDF's "detected chip: issi"), rejects SFDP (0x5A) and RDID variants; the Xteink
  flash is a different vendor, and `esp_flash` logs differ. Flash reads cost zero extra time in QEMU, while real hardware (and any cycle-accurate
  inkemu) has cache misses, wait states (SPI Mode shows "SLOW READ" in the bootloader log).
- Timing: QEMU is CPI=1 (8 ns/insn). Real C3 at 160 MHz is faster per ns but not CPI=1 (CoreMark: see section 3). Timestamps in
  `I (n)` log lines are QEMU virtual ms, not hardware-faithful.
- eFuse/chip identity is QEMU's: chip rev v0.3, efuse block rev v1.3, MAC and flash-crypt/secure-boot fuses defaults; ESP32 `rst:0x1 (POWERON)` banner and `SPIWP:0xee`.
  Clock/RTC_CNTL registers are modelled loosely (calibration values, XTAL frequency detection).
- Interrupt controller: ESP interrupt matrix is QEMU's own implementation; interrupt priority/threshold edge cases and the delivery
  latency when `mstatus.MIE=0` changed between builds (fix in 20260417).
- QEMU CPU model has S-mode CSRs and PMP behaviour of its generic riscv core; hardware does not have the same CSR set.
- Not emulated: cache coherence effects, flash/cache MMU faults timing, brown-out, light/deep sleep (restart path via RTC SW reset works: the
  hello_world's `esp_restart` reboots through the ROM with `rst:0xc (RTC_SW_CPU_RST)`).

---------------------------------------------------------------------------------------------------

## 2. Reproducible ESP-IDF bootloader

Built in `inkemu/esp-idf` (IDF v5.5.2, GCC `riscv32-esp-elf-gcc (crosstool-NG esp-14.2.0_20251107) 14.2.0`, esptool v4.11.dev1).
Project: `$IDF_PATH/examples/get-started/hello_world` copied to scratch, plus `sdkconfig.defaults`:
```
CONFIG_IDF_TARGET="esp32c3"
CONFIG_ESPTOOLPY_FLASHSIZE_16MB=y
CONFIG_ESPTOOLPY_FLASHMODE_DIO=y
CONFIG_ESPTOOLPY_FLASHFREQ_80M=y
CONFIG_ESP_DEFAULT_CPU_FREQ_MHZ_160=y
CONFIG_APP_REPRODUCIBLE_BUILD=y
```
Commands (inside the container after `. export.sh`): `idf.py set-target esp32c3 && idf.py build` (script `rebuild.sh` removes `build` and
`sdkconfig` first, builds, copies artefacts).

### Artefacts (canonical, reproducible config; dir `r1/`)

| File | Size | SHA-256 |
|---|---|---|
| `build/bootloader/bootloader.bin` | 21,040 B | `601b48456b0a9d5c9f1beac166135e06686d3c5ed0309dd16840ab39887afaa3` |
| `build/partition_table/partition-table.bin` (default single-app table: nvs 0x9000/0x6000, phy_init 0xf000, factory 0x10000/1 MB) | 3,072 B | `7f00b6c042a89b15b0cac534f82ed988caf29278ff5700b0c511eb1b5bb7c820` |
| `build/hello_world.bin` | 136,160 B | `4b82d73e37d10969d660eba95984cbe7f053212d39306cc54bfce6c366d0f04b` |
| merged 16 MB `flash_hello.bin` | 16,777,216 B | `fc05ef5dbe85372e1da52abdec3bfcbbb110738a60163734666095fcd8c39bf2` |

Xteink-like OTA layout variant (partition CSV = CrossPoint `default_16MB.csv`: nvs 0x9000/0x5000, otadata 0xe000/0x2000, app0 0x10000/0x640000,
app1 0x650000/0x640000, spiffs 0xc90000/0x360000, coredump 0xff0000/0x10000): `partition-table.bin` SHA-256
`bd0f7954aca2ef7d925ee21aaa1f3dc8822d1d6ce5cbbd26a135e5886bfff6ce`; bootloader.bin and hello_world.bin are **unchanged**
(same hashes, neither embeds the partition table); merged `flash_hello_ota.bin` SHA-256 `9ddacb60b8b7338731b52edee1ecae47c7f45470d8db6fa8d1fbc5a20011bc89`
(boots, "No factory image, trying OTA 0").

Because the bootloader is app-independent for a given sdkconfig, the CoreMark build (section 3) produced the **same**
`bootloader.bin` hash 601b4845...afaa3.

### Reproducibility (measured)

| Build | bootloader.bin | hello_world.bin |
|---|---|---|
| default config (APP_REPRODUCIBLE_BUILD off), clean build 1 vs clean build 2 | **differ** (36 bytes, e.g. byte 89; sha `df2b96c0...` vs `acfa4b5b...`) | differ (68 bytes) |
| `CONFIG_APP_REPRODUCIBLE_BUILD=y`, two clean builds ~1 min apart, same path | **identical** | **identical** |
| same, built from a different project path (`/w/other/proj_x`) | **identical** | **identical** |

So: set `CONFIG_APP_REPRODUCIBLE_BUILD=y` (removes build timestamps/paths from the bootloader and app descriptors; IDF also remaps debug prefixes). The partition
table was byte-identical in all builds. The toolchain and IDF version must be pinned (the Docker image tag). The sizes above (21,040 / 136,160) are the reproducible ones;
the non-reproducible default config gave 21,104 / 136,272.

### merge_bin command (16 MB image, matches Xteink flash layout)

```
esptool.py --chip esp32c3 merge_bin --fill-flash-size 16MB -o flash_hello.bin \
  --flash_mode dio --flash_freq 80m --flash_size 16MB \
  0x0 bootloader.bin 0x8000 partition-table.bin 0x10000 hello_world.bin
```
(or `... @flash_args` from inside `build/` ; esptool prints "SHA digest in image updated" but the merged image contains byte-identical copies of the three inputs and
0xFF elsewhere: verified.) Note: `flash_args` in a flat copy references `bootloader/bootloader.bin` paths; run it from `build/`.

### Golden boot log (QEMU 20250817, `-icount 3`, UART0), `golden_hello_boot.log` (57 lines), SHA-256 `2bf0d70ebb20e9626710ab28fd7d4c62bc10ef15cb2bb863eee0b33c4a755d6b`
Deterministic: the first 68 lines were identical across runs. Timestamps are QEMU virtual milliseconds.
```
Adding SPI flash device
ESP-ROM:esp32c3-api1-20210207
Build:Feb  7 2021
rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)
SPIWP:0xee
mode:DIO, clock div:1
load:0x3fcd5820,len:0x15a4
load:0x403cbf10,len:0xc64
load:0x403ce710,len:0x2fd0
entry 0x403cbf1a
I (0) boot: ESP-IDF v5.5.2 2nd stage bootloader
I (0) boot: chip revision: v0.3
I (0) boot: efuse block revision: v1.3
I (0) boot.esp32c3: SPI Speed      : 80MHz
I (0) boot.esp32c3: SPI Mode       : SLOW READ
I (0) boot.esp32c3: SPI Flash Size : 16MB
I (0) boot: Enabling RNG early entropy source...
I (0) boot: Partition Table:
I (0) boot: ## Label            Usage          Type ST Offset   Length
I (1) boot:  0 nvs              WiFi data        01 02 00009000 00006000
I (1) boot:  1 phy_init         RF data          01 01 0000f000 00001000
I (1) boot:  2 factory          factory app      00 00 00010000 00100000
I (1) boot: End of partition table
I (1) esp_image: segment 0: paddr=00010020 vaddr=3c010020 size=0656ch ( 25964) map
I (4) esp_image: segment 1: paddr=00016594 vaddr=3fc8b600 size=011d8h (  4568) load
I (4) esp_image: segment 2: paddr=00017774 vaddr=40380000 size=088a4h ( 34980) load
I (8) esp_image: segment 3: paddr=00020020 vaddr=42000020 size=0e6fch ( 59132) map
I (14) esp_image: segment 4: paddr=0002e724 vaddr=403888a4 size=02c70h ( 11376) load
I (15) esp_image: segment 5: paddr=0003139c vaddr=50000000 size=00020h (    32) load
I (18) boot: Loaded app from partition at offset 0x10000
I (18) boot: Disabling RNG early entropy source...
I (18) cpu_start: Unicore app
I (26) cpu_start: GPIO 20 and 21 are used as console UART I/O pins
I (26) cpu_start: Pro cpu start user code
I (26) cpu_start: cpu freq: 160000000 Hz
I (26) app_init: Application information:
I (26) app_init: Project name:     hello_world
I (26) app_init: App version:      1
I (26) app_init: ELF file SHA256:  43d81da8d...
I (26) app_init: ESP-IDF:          v5.5.2
I (26) efuse_init: Min chip rev:     v0.3
I (27) efuse_init: Max chip rev:     v1.99 
I (27) efuse_init: Chip rev:         v0.3
I (27) heap_init: Initializing. RAM available for dynamic allocation:
I (27) heap_init: At 3FC8D610 len 000329F0 (202 KiB): RAM
I (27) heap_init: At 3FCC0000 len 0001C710 (113 KiB): Retention RAM
I (27) heap_init: At 3FCDC710 len 00002950 (10 KiB): Retention RAM
I (27) heap_init: At 50000020 len 00001FC8 (7 KiB): RTCRAM
I (27) spi_flash: detected chip: issi
I (27) spi_flash: flash io: dio
I (27) sleep_gpio: Configure to isolate all GPIO pins in sleep state
I (27) sleep_gpio: Enable automatic switching of GPIO sleep configuration
I (28) main_task: Started on CPU0
I (28) main_task: Calling app_main()
Hello world!
This is esp32c3 chip with 1 CPU core(s), WiFi/BLE, silicon revision v0.3, 16MB external flash
Minimum free heap size: 332800 bytes
```
(`golden_hello_boot.log` was captured from the canonical `flash_hello.bin` above. The ELF SHA256 prefix `43d81da8d` is specific to this reproducible build; mask it in a golden comparison if the toolchain may change.)
OTA-layout variant log: `golden_hello_ota_boot.log` (62 lines, SHA-256 `b0bd4fdc6305f96b5f4c33b6ba527e953871657f92daa1519b142f4d02ba4b2c`).
The app restarts every 10 s ("Restarting now." then ROM banner `rst:0xc (RTC_SW_CPU_RST)`), useful as a reset-path test.

---------------------------------------------------------------------------------------------------

## 3. CoreMark

### Maintained port
Espressif component **`espressif/coremark` 1.1.0~2** (registry, 2025-07-12; component hash `1f95044d7fe042d997b7715e0dc9753e65c6724158c2d028997f7816df1329d7`),
source `https://github.com/espressif/idf-extra-components/tree/master/coremark` (embeds `eembc/coremark` as submodule + `port/core_portme.c`), requires IDF >= 5.0.
It enables `-O3 -fjump-tables -ftree-switch-conversion` for CoreMark sources and places the code in IRAM (linker.lf). Its `coremark_example` is the ready-made app.

### Build recipe (built and run here)
```
idf.py create-project-from-example "espressif/coremark:coremark_example"   # then pin the dependency
# main/idf_component.yml : version: "1.1.0~2"   (default is '*', unpinned)
# sdkconfig.defaults (example ships CONFIG_ESP_TASK_WDT_INIT=n) + :
#   CONFIG_ESPTOOLPY_FLASHSIZE_16MB=y  CONFIG_ESPTOOLPY_FLASHMODE_DIO=y  CONFIG_ESPTOOLPY_FLASHFREQ_80M=y
#   CONFIG_ESP_DEFAULT_CPU_FREQ_MHZ_160=y  CONFIG_APP_REPRODUCIBLE_BUILD=y
idf.py set-target esp32c3 && idf.py build
esptool.py --chip esp32c3 merge_bin --fill-flash-size 16MB -o flash_coremark.bin --flash_mode dio --flash_freq 80m --flash_size 16MB \
  0x0 bootloader.bin 0x8000 partition-table.bin 0x10000 coremark_example.bin
```
Results: `coremark_example.bin` 144,736 B, SHA-256 `a8f8dcec9c254b709fd108a5e29b8062c8e551cf75e9b28e5bf5bd120efc04dc`;
`riscv32-esp-elf-size`: text 111,216 / data 33,404 / bss 126,170; merged `flash_coremark.bin` SHA-256
`6df776cc90901ee65fa52286c0a77d0357469511cb8a11f1a1b5bf9ff39ba4ce`. bootloader.bin and partition-table.bin identical to hello_world's (same hashes).
`dependencies.lock` records `espressif/coremark 1.1.0~2`, IDF 5.5.2.

### Output format printed (UART0), real capture from QEMU
```
Running coremark...
2K performance run parameters for coremark.
CoreMark Size    : 666
Total ticks      : 14399
Total time (secs): 14.399000
Iterations/Sec   : 416.695604
Iterations       : 6000
Compiler version : GCC14.2.0
Compiler flags   : -ffunction-sections ... -O3 -fjump-tables -ftree-switch-conversion
Memory location  : IRAM
seedcrc          : 0xe9f5
[0]crclist       : 0xe714
[0]crcmatrix     : 0x1fd7
[0]crcstate      : 0x8e3a
[0]crcfinal      : 0xa14c
Correct operation validated. See README.md for run and reporting rules.
CoreMark 1.0 : 416.695604 / GCC14.2.0 <flags> / IRAM
CPU frequency: 160 MHz
```
Parse with the regex `^CoreMark 1\.0 : ([0-9.]+) / ` and require the line `Correct operation validated.`; the CRCs (`seedcrc 0xe9f5`, `crclist 0xe714`, `crcmatrix 0x1fd7`, `crcstate 0x8e3a`,
`crcfinal 0xa14c`) are identical to Espressif's published example output (function-correctness oracle for the CPU emulation; independent of timing). 6000 iterations (>10 s required by the run rules).

### Published reference numbers (ESP32-C3, 160 MHz)
1. **ESP32-C3 Series Datasheet v2.4 (2026-05-06)**, p. 3, "CPU and Memory": "CoreMark score – 1 core at 160 MHz: **483.27 CoreMark; 3.02 CoreMark/MHz**".
   (PDF downloaded to `dl_datasheet/`, SHA-256 `833fc000b4b3c3d39c496fcbd597fed5806956503ce7390b19cc8ae82f19f968`; URL
   `https://www.espressif.com/sites/default/files/documentation/esp32-c3_datasheet_en.pdf`.) The revision history notes the score was *updated* in v2.1 (2025-04-14); compiler/flags/memory
   are NOT stated in the datasheet. The figure was added in v1.1 (2021-10-26) so older datasheets/third-party pages quote a lower number; always cite the datasheet version.
2. **`espressif/coremark` example README** (readme of `coremark_example`, 1.1.0~2): ESP32-C3 at 160 MHz, **409.249028 CoreMark** (14.661 s, 6000 iterations) = 2.558 CoreMark/MHz,
   GCC 12.2.0, `-O3 -fjump-tables -ftree-switch-conversion`, IRAM. URL
   `https://components-file.espressif.com/components/espressif/coremark/1.1.0~2/examples/coremark_example/readme.md` (also in `idf-extra-components/coremark/README.md`).
3. Measured here in QEMU (GCC 14.2.0, same flags, IRAM): **416.696** (14.399 s of *virtual* time, 8 ns/insn) = 2.60 per 160 MHz; wall time 2.4 s. Implied instruction count
   14.399 s / 8 ns = 1.80 G insns = ~300 k instructions per iteration, so CPI = 1 gives an upper bound of 160 M / 300 k = 533 it/s (3.33 CoreMark/MHz); the datasheet 483 implies hardware CPI ~ 1.10.
   QEMU's CoreMark number is NOT a hardware reference (it is a pure function of instruction count at 8 ns).

Suggested inkemu assertions: (a) `Correct operation validated` + the five CRCs; (b) retired-instruction count per iteration equal to QEMU's (deterministic, same binary, ~300 k) ;
(c) with its cycle model, a score within a tolerance band around 483 (datasheet) / 409 (Espressif example, older GCC), e.g. +-10 %, documented as a model-quality check not a pass/fail on exactness.

---------------------------------------------------------------------------------------------------

## 4. Licences

| Item | Licence | Source / note |
|---|---|---|
| Espressif QEMU fork (`espressif/qemu`) | GPL-2.0 (`COPYING`; GitHub reports NOASSERTION because of mixed upstream files; some files GPL-2.0-or-later / LGPL / BSD) | Used as an external binary only; never linked or shipped in inkemu; do not vendor its binary in the repo, fetch in CI (pin SHA-256 `547f03e0...2e84e77` for the x86_64-linux tarball). |
| `esp32c3-rom.bin` (in QEMU `pc-bios` and release tarball) | No licence file; Espressif ROM. Same code available in `esp-rom-elfs` (Apache-2.0) | Do not commit the .bin; extract in CI from the QEMU tarball or from `esp-rom-elfs` rev3. Note for SP1b: inkemu itself likely needs a ROM to boot, the Apache-2.0 `esp-rom-elfs` is the cleanest source. |
| ESP-IDF v5.5.2 (bootloader source, hello_world) | Apache-2.0 | A built `bootloader.bin`/`hello_world.bin` is a derived binary of Apache-2.0 sources: redistributable with the licence/NOTICE (IDF also bundles third-party components with their own licences in the app; the bootloader links mostly Espressif Apache-2.0 code). Safe to commit as a small fixture with a `THIRD_PARTY`/NOTICE entry, but building in CI from the pinned image is cleaner and verifiable thanks to reproducibility. |
| esptool 4.x | GPL-2.0-or-later | Test-time tool only. |
| GCC `riscv32-esp-elf` 14.2.0 | GPL-3.0 with GCC Runtime Library Exception | Output binaries are not GPL-encumbered (runtime exception covers libgcc/newlib parts linked in). |
| `espressif/coremark` component / `idf-extra-components` | Apache-2.0 (repo) ; registry shows "Custom" = the CoreMark Acceptable Use Agreement | See below. |
| EEMBC CoreMark sources | Source files carry "Licensed under the Apache License, Version 2.0 (Copyright EEMBC)"; `LICENSE.md` of `eembc/coremark` (and the component's `LICENSE`) is the **CoreMark Acceptable Use Agreement** governing the registered trademark COREMARK. | Clause 1.2: the licensee shall not use the Trademark in connection with a **modified, derivative or altered copy** of the Software. Reporting rules (README): runs must last >= 10 s, unmodified sources apart from the port files, memory/compiler/flags reported. |

CoreMark fixture decision: the Apache-2.0 source licence permits redistributing the compiled object code (keep licence + notices), so a committed `coremark_example.bin` (144 KB) is *legally* possible, but the
Acceptable Use Agreement forbids using the "CoreMark" mark for modified copies and for results that do not follow the run rules. Recommendation: **do not commit the CoreMark .bin; build it in CI from the pinned
image (`espressif/coremark 1.1.0~2`, IDF 5.5.2, reproducible), keep the `coremark` string out of marketing claims, and label inkemu results "CoreMark-derived benchmark, not a certified CoreMark score".**
If a committed fixture is wanted anyway, add Apache-2.0 text + EEMBC copyright notice in `THIRD_PARTY`. (This is an engineering reading, not legal advice.)

---------------------------------------------------------------------------------------------------

## 5. Blockers / open points

1. No TCG plugin support in released QEMU builds; per-instruction register traces rely on `-d cpu` text parsing (~133 k insn/s, ~1.1 KB/insn) or gdbstub (~7 k steps/s). Building our own plugin-enabled fork would be a separate, heavy task and is not needed for ROM->`app_main` windows.
2. Duplicate trace entries at icount-budget refills (and extra ones around interrupts in the 20260417 build) must be deduplicated; IDF-pinned 20250817 is the more predictable choice (both give identical pc sequences after dedupe up to the first interrupt).
3. gdbstub steps mask interrupts by default (`Qqemu.sstep=0x1` to re-enable) and the over-the-interrupt behaviour was not tested.
4. QEMU lacks SPI2/I2C0/SAR-ADC/LEDC (read as 0): lockstep is meaningful for hello_world/CoreMark/boot, not for full Xteink firmware peripherals.
5. QEMU's flash model is not the Xteink vendor chip; SFDP/RDID divergences are expected.
6. The reproducible build depends on keeping the image tag, IDF version, and `sdkconfig.defaults` fixed; an unpinned `espressif/coremark: '*'` in the example would drift.
7. `docker run inkemu/esp-idf` startup is 2 min per invocation; use a persistent container + `idfenv.sh`.
8. The Xteink bootloader is NOT the stock IDF one (CrossPoint wraps `bootloader_common_check_efuse_blk_validity`); our reproducible stock bootloader is a test fixture, not a byte-for-byte Xteink bootloader (the real `bootloader.bin` is 18,672 B per docs/research/xteink-hardware.md).

## Files in the scratch dir (`.../scratchpad/oracle/`)
`r1/` canonical artefacts (+ elf), `r2/ r3/` repro rebuilds (same hashes), `out1/ out2/` non-reproducible builds, `r_ota/` OTA-layout build, `cm1/` CoreMark build, `flash_hello.bin`, `flash_hello_ota.bin`, `flash_coremark.bin`,
`golden_hello_boot.log`, `golden_hello_ota_boot.log`, `coremark_run.log`, `trace_a.sample` (3 MB sample of `-d cpu`), `gdbtrace.py` (RSP stepper), `cmp_cpu.py` (validator), `det_A1.txt`/`det_B1.txt` (3 M pc traces of both QEMU builds),
`mtree.txt`, `dl_qemu_latest/` (downloaded QEMU 20260417), `dl_datasheet/`, `dl_license/`, helper scripts `rebuild.sh gdbrun.sh dettest.sh thr.sh qrun.sh`.
