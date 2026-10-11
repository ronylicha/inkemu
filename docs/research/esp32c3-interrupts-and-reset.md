# ESP32-C3 interrupts and software reset: what an ESP-IDF v5.5.2 application needs from the CPU, the interrupt controller and the reset logic

Scope: what the CPU and `INTERRUPT_CORE0` must do, at register and CSR level, for an ESP-IDF v5.5.2 (FreeRTOS) application to start its scheduler, run `app_main`, sleep in `vTaskDelay` and restart with `esp_restart()`; and what that restart does to the hardware.
Date of measurements: 2026-10-11. The repository was not modified apart from this file.
Already established and not repeated here: `esp32c3-boot-chain.md` (BC: §B CPU and interrupts, §E resets and watchdogs), `esp32c3-bootloader-accesses.md` (BA: §3, §6, §9), `sp1b-test-oracle.md` (TO: the QEMU oracle). Timer and watchdog *counting* (SYSTIMER, TIMG, RTC timer) is out of scope: only the interrupts they raise are covered.

## 0. Sources, method, conventions

| Tag | Meaning |
|---|---|
| MEASURED | Observed in a QEMU run made for this report, or computed from such a trace. **A MEASURED fact is a fact about QEMU**, not about silicon, unless the text says otherwise |
| READ | Read in a source or a disassembly, cited as below |
| ASSUMED | Inference or recollection, stated as such |

| Source | What | Cited as |
|---|---|---|
| ESP32-C3 Technical Reference Manual v1.4 (PDF built 2026-03-26, 903 pages) | The only hardware documentation: chapter 1 (ESP-RISC-V CPU, its CSRs and its interrupt controller), 6 (reset), 8 (interrupt matrix), 12 (watchdogs), 16 (system registers), 17 (debug assistant) | `TRM §n` |
| ESP-IDF v5.5.2, Apache-2.0, in the `espressif/idf:v5.5.2` image | `/opt/esp/idf/components/<path>` | `IDF:<path>:<line>` |
| Reference application and bootloader | `hello_world.elf` sha256 `43d81da8…5448e`, `flash_hello.bin` sha256 `fc05ef5d…c39bf2` (the SP1b reference image) | `APP:symbol@address` |
| Mask ROM rev3 | `esp32c3_rev3_rom.elf` (Apache-2.0), disassembled | `ROM3:symbol@address` |
| Espressif QEMU, two released builds | `esp_develop_9.2.2_20250817` (in the image, called **Q1**) and `esp_develop_9.2.2_20260417` (**Q2**) | MEASURED |
| Espressif QEMU source, branch `esp-develop` (GPL), fetched 2026-10-11, possibly newer than both binaries | Read for facts only; no code or comment reproduced | `QEMU:<file>` |
| esp-hal v0.20.1 (MIT/Apache-2.0), `esp-riscv-rt` and `esp-hal/src/interrupt/riscv.rs` | An independent implementation that runs on real chips | `ESPHAL:<file>` |

Method:

1. **Full trace F1 (MEASURED, Q1).** Same command as BA §0 (`-icount 3`, `one-insn-per-tb=on`, `-d exec,nochain,int,unimp,guest_errors`, `-trace 'memory_region_ops_*'`), run until 30,000 instructions after the second `Hello world!`: 5,839,461 instructions covering the first boot, eleven seconds of application, `esp_restart()` and the whole second boot. Its first 3,000,001 pcs are identical to the trace of BA.
2. **CSR trace C1 (MEASURED, Q1).** `-d cpu,nochain,int` plus the MMIO trace, reduced to every change of `mstatus`, `mip`, `mie`, `mtvec`, `mepc`, `mcause`, `mtval`, `mscratch`, for the first 3,420,000 instructions (through the reset).
3. **Second build F2 (MEASURED, Q2).** First 2,600,000 instructions, compared with F1.
4. **Probe application (MEASURED, Q1 and Q2).** `hello_world` with a different `app_main`, built in the same image: reads and writes `mie`/`mip`/`mtvec`/`mcause`/`mstatus`, reads `CPU_INT_EIP_STATUS` and `INTR_STATUS_0/1` with a tick pending under each masking condition, programs an edge-type line, executes `wfi` with `mstatus.MIE` = 0, then ends with `abort()`, an illegal instruction, or a store to address 0 (four variants).
5. **Source reading (READ)** for everything QEMU does not model, and for what silicon does where QEMU differs.

Conventions:

- **insn** = 1-based count of executed instructions since reset in trace F1, same numbering as BA (bootloader entry = 521,511; application entry = 1,683,222). QEMU executes one instruction per 8 ns of virtual time (CPI = 1); an insn number is an ordering key, not a cycle count. C1 numbers drift by up to 3 from F1 after insn 2.87 M (different duplicate-entry rule); F1 numbers are used throughout.
- "First life" = power-on to the software reset (insn 1 to 3,402,177). "Second boot" starts at insn 3,402,178.
- "Line" = CPU interrupt line 1..31 (the TRM says "interrupt ID"). "Source" = peripheral interrupt source 0..61.

## 1. The interrupt controller (`INTERRUPT_CORE0`, base `0x600C2000`)

### 1.1 Registers

Hardware columns: READ `TRM §1.5`, `TRM §8.4` (registers 8.52 to 8.60), `IDF:soc/esp32c3/register/soc/interrupt_core0_reg.h`. QEMU column: READ `QEMU:esp32c3_intmatrix.c`, confirmed by the probe where marked (P).

| Offset | Register | Bits | Reset | Hardware | QEMU |
|---|---|---|---|---|---|
| `0x000 + 4·s`, s = 0..61 | `<SRC>_MAP` | [4:0] | 0 | Line that source s drives. 0 = not routed (line 0 does not exist). Several sources may name the same line: the line is the OR of their levels (`TRM §8.3.3.2`) | Same |
| `0x0F8` / `0x0FC` | `INTR_STATUS_0/1` | [31:0] / [29:0] | 0, RO | Raw status of sources 0..31 / 32..61: "if the bit is 1, the corresponding source triggered an interrupt". Independent of routing and masking | **Not decoded: always reads 0** (P: 0 with a tick pending) |
| `0x100` | `CLOCK_GATE` | [0] | 1 | Register clock gate | Not decoded, reads 0 |
| `0x104` | `CPU_INT_ENABLE` | [31:1] | 0 | Bit n enables line n | Same |
| `0x108` | `CPU_INT_TYPE` | [31:1] | 0 | Bit n: 0 = level, 1 = edge (latch on rising edge) | Reads 0; a non-zero write only produces a warning: **edge lines are not supported** |
| `0x10C` | `CPU_INT_CLEAR` | [31:1] | 0, R/W | Edge acknowledge, by *toggling*: write 1 then 0 to bit n (§1.3) | Not decoded: reads 0, writes ignored (P) |
| `0x110` | `CPU_INT_EIP_STATUS` | [31:1] | 0, RO | Pending state of each line, after the enable and priority/threshold masks (§1.2) | **Not decoded: always reads 0** (P) |
| `0x114 + 4·n`, n = 0..31 | `CPU_INT_PRI_n` | [3:0] | 0 | Priority of line n, 1 (lowest) to 15 (highest); 0 = masked. `PRI_0` exists in the header and the SVD but line 0 does not | Decodes `0x118`..`0x18C` only, i.e. **lines 1..30**; `PRI_31` (`0x190`) reads 0 and ignores writes |
| `0x194` | `CPU_INT_THRESH` | [3:0] | 0 | Threshold (§1.2) | Same |
| `0x7FC` | `INTERRUPT_DATE` | | | Version | Not decoded |

The TRM heading of register 8.59 gives `0x0118 + 0x4*n`; its own register summary puts `PRI_16` at `0x0154`, which agrees with the IDF header (`0x114 + 4·n`). The header is right.

### 1.2 The rule that makes a line pending, and taken

READ `TRM §1.5.2`, `TRM §1.5.3.2`, `TRM` register 8.60; the three sources agree.

```
level(n)   = OR of source_level(s) for every source s with MAP[s] == n          (n = 1..31)
raw(n)     = TYPE[n] ? edge_latch(n) : level(n)
pending(n) = ENABLE[n]  &&  raw(n)  &&  PRI[n] != 0  &&  PRI[n] >= THRESH      = bit n of CPU_INT_EIP_STATUS
taken      = mstatus.MIE && some pending(n);  the line taken is the pending one with the highest PRI,
             ties broken by the lowest line number
```

- **The threshold comparison is "greater or equal"**, and priority 0 never passes: "Only if an interrupt has non-zero priority, higher or equal to the value in the threshold register, will it be reflected in `CPU_INT_EIP_STATUS`". The IDF wording agrees ("strictly less than the threshold level are masked", `IDF:riscv/include/esp_private/interrupt_intc.h`).
- **Arbitration** (unknown in BC §B.1) is settled: "Interrupts with same priority are statically prioritized by their IDs, lowest ID having highest priority"; the CPU "acknowledges (claims) the interrupt with the highest priority".
- **A level line follows its source.** Nothing in the controller is changed by taking the interrupt: the handler must clear the source ("Pending state of a level type interrupt is unaffected by [`CPU_INT_CLEAR`] and must be cleared from source"). If the source is still asserted when the line is unmasked again, the interrupt is taken again.
- A pending line can be masked again before it is taken, by lowering its priority, raising the threshold, or clearing its enable bit.
- QEMU implements `ENABLE && level && PRI >= THRESH`: it lets a priority-0 line through when the threshold is 0, and it takes the **lowest-numbered asserted line whatever the priorities** (READ `QEMU:esp_cpu.c`, function selecting the cause).

### 1.3 Edge lines

READ `TRM §1.5.2` (properties 2, 4, 5):

- With `TYPE[n]` = 1 the controller latches the rising edge of `level(n)`. The latch is what `pending(n)` reports; it stays set after the source has dropped.
- The latch is cleared by **toggling** bit n of `CPU_INT_CLEAR`: "first setting and then clearing the corresponding bit". This "will clear the pending state of claimed edge-type interrupts only" (claimed = already taken by the CPU).
- A latched edge that has not been taken can be flushed by first clearing `ENABLE[n]`, then toggling `CLEAR[n]`.
- Whether the latch captures an edge that arrives while `ENABLE[n]` is 0 is not stated (property 4 speaks of "an enabled and unmasked interrupt signal"): ASSUMED not captured.

Nothing on the measured path uses an edge line (MEASURED: `CPU_INT_TYPE` is only ever written 0, `CPU_INT_CLEAR` is never accessed, §1.5). On the C3 the SYSTIMER interrupts are level (`SOC_SYSTIMER_INT_LEVEL`). The IDF helper for acknowledging an edge, `rv_utils_intr_edge_ack(intr_num)`, executes `REG_SET_BIT(CPU_INT_CLEAR, intr_num)`: it ORs the line *number* into the register as if it were a mask and never writes the bit back to 0 (READ `IDF:riscv/include/esp_private/interrupt_intc.h`, `IDF:esp_hw_support/include/esp_cpu.h:486-496`). It is called only for `ESP_INTR_FLAG_EDGE` allocations. An emulator should implement the TRM semantics and not expect IDF to exercise them.

### 1.4 Reserved lines and allocation

| Line | Use | Source |
|---|---|---|
| 0 | Does not exist: vector slot 0 is the exception entry | `TRM §1.5.2` note; `IDF:riscv/include/esp_private/interrupt_intc.h` (`assert_valid_rv_int_num`) |
| 1 | Reserved for Wi-Fi | `IDF:esp_hw_support/port/esp32c3/esp_cpu_intr.c:11-17` |
| 5 | The **ROM** routes UART0 (source 21) here with priority 1 at every boot, without enabling the line (`ROM3:Uart_Init`, MEASURED insn 5,890 to 5,910); the application's `core_intr_matrix_clear` then unmaps it. Not reserved for IDF | BC §B.3 |
| 6 | "Permanently disabled interrupt" (`INT_MUX_DISABLED_INTNO`), reserved | same file |
| 24 | `ETS_INT_WDT_INUM`: interrupt watchdog (TG1 WDT); vector goes to `_panic_handler` | `IDF:riscv/vectors_intc.S:78`, `IDF:soc/esp32c3/include/soc/soc.h:212-215` |
| 25 | `ETS_CACHEERR_INUM`: cache errors; `_panic_handler` | same |
| 26 | `ETS_MEMPROT_ERR_INUM`: memory-protection violations; `_panic_handler` when `CONFIG_ESP_SYSTEM_MEMPROT_FEATURE` (default) | same |
| 27 | `ETS_ASSIST_DEBUG_INUM`: hardware stack guard; `_panic_handler` when `CONFIG_ESP_SYSTEM_HW_STACK_GUARD` (default) | same |
| 2..5, 7..23, 28..31 | Free, given out by `esp_intr_alloc` lowest number first; a line whose vector does not jump to `_interrupt_handler` is treated as reserved | `IDF:esp_hw_support/include/esp_private/esp_riscv_intr.h:56-70`; MEASURED order 2, 3, 4, 5 |

All free lines are allocated at priority 1 by default (`ESP_INTR_FLAG_LEVEL1`); the four panic lines are set to priority 4. The critical-section threshold is 4 (`RVHAL_EXCM_LEVEL`, MEASURED value written): it masks priorities 1 to 3 and **leaves the four panic lines deliverable inside critical sections**.

### 1.5 What the reference application does with the controller (MEASURED)

Controller accesses: ROM 7 (BC §B.1, §B.3), bootloader 0, application 12,018 in its first life (4,798 reads, 7,220 writes).

| Register | Accesses in the first life of the application | By |
|---|---|---|
| `<SRC>_MAP` | 73 writes, no read: 62 zeroes (`core_intr_matrix_clear`, insn 2,186,120) then 11 routings (§3.2) | ROM `intr_matrix_set@0x40047a6a`, called by IDF |
| `CPU_INT_ENABLE` | 28 reads, 24 writes, always read-modify-write under `mstatus.MIE` = 0 | ROM `esprv_intc_int_enable@0x4005294e` / `…_disable@0x4005295e`, called by `esp_intr_enable_source` / `esp_intr_disable_source` / `esp_intr_noniram_disable` / `…_enable` |
| `CPU_INT_TYPE` | 10 read-modify-writes, value always 0 | ROM `esprv_intc_int_set_type@0x40052972` |
| `CPU_INT_PRI_n` | 10 writes (lines 2, 3, 4, 5, 24, 25, 27 once each, 26 three times); 1,112 reads, one per interrupt taken (`PRI_3` ×12, `PRI_4` ×1,100) | ROM `esprv_intc_int_set_priority@0x40052992`; `_interrupt_handler` |
| `CPU_INT_THRESH` | 3,648 reads, 7,103 writes: critical sections (2,450 entries: read, write 4; 2,428 exits: write the saved value), newlib locks (86 reads), `_interrupt_handler` (1 read, 2 writes per interrupt), `xPortStartScheduler` (1 write of 1, through ROM `esprv_intc_int_set_threshold@0x400529a2`) | |
| `CPU_INT_EIP_STATUS`, `INTR_STATUS_0/1`, `CPU_INT_CLEAR`, `CLOCK_GATE` | **never accessed** | |

Consequences: the application calls the **ROM** to configure the controller, so the ROM routines must work on the emulated registers; the threshold must read back what was written (already noted in BA §3); the status registers and the edge mechanism are not needed for this application, but are part of the hardware.

`esp_intr_alloc` writes in this order: map, **enable**, priority, type (MEASURED, e.g. insn 2,352,136 / 2,352,157 / 2,352,202 / 2,352,218). The line is therefore enabled for about 45 instructions with priority 0: the "priority 0 is masked" rule is what keeps this harmless on hardware.

## 2. The CPU side

### 2.1 CSRs

`TRM §1.4.1` lists: `mvendorid` (`0x612`), `marchid` (`0x80000001`), `mimpid` (`0x1`), `mhartid` (0), `mstatus`, `misa`, `mtvec`, `mscratch`, `mepc`, `mcause`, `mtval`, `pmpcfg0..3`, `pmpaddr0..15`, `tselect`, `tdata1`, `tdata2`, `tcontrol`, `dcsr`, `dpc`, `dscratch0/1`, `mpcer`/`mpcmr`/`mpccr` (`0x7E0..0x7E2`), `cpu_gpio_oen`/`in`/`out` (`0x803..0x805`). "If write/set/clear operation is attempted on any of the CSRs which are read-only […] the CPU will generate illegal instruction exception."
The list is **not exhaustive**: the ROM uses `ustatus` (`0x000`) and `0x800..0x802`, which are not in it (BC §B.2). Absence from the TRM is therefore weak evidence.

CSR and system instructions the application actually executes in F1 (MEASURED, every `csr*`/`wfi`/`mret`/`fence` instruction of the disassembly checked against the pc trace):

| Instruction | Where | Executed (F1, both boots) |
|---|---|---|
| `csrw mtvec` | `call_start_cpu0+0x24` (value `0x40380001`) | 2 |
| `csrrci mstatus,8` / `csrrs mstatus,rs` | `xPortSetInterruptMaskFromISR` (2,634), `esp_intr_enable_source` (20), `esp_intr_disable_source` (12), `esp_intr_noniram_disable`/`enable` (8 each) | critical sections and controller updates |
| `csrrsi mstatus,8` | `xPortStartScheduler+0x2e` | 2 |
| `csrrci mstatus,8` | `esp_restart_noos+0x8` | 1 |
| `csrr mepc`, `csrr mcause`, `csrr mstatus`, `csrs mstatus`, `csrc mstatus`, `csrw mcause`, `csrw mstatus`, `csrw mepc`, `mret`, `fence` ×2 | `_interrupt_handler` | 1,120 (one set per interrupt) |
| `wfi` | `esp_cpu_wait_for_intr+0x14` (`0x40381d16`) | 1,106 |
| `csrr`/`csrw 0x7e2` | log timestamp, `esp_clk_init` | 44 / 2 / 2 |
| `pmpcfg0..3`, `pmpaddr0..15` | `esp_cpu_configure_region_protection` | 2 each |
| `tselect`, `tdata1`, `tdata2`, `tcontrol`; `_panic_handler`'s `csrr mepc/mstatus/mtvec/mhartid/mtval/mcause`; `ebreak` | present in the image, **not executed** on a healthy run | 0 |

**No instruction of the ROM (BC §B.2), the bootloader or the application accesses `mie` or `mip`.**

### 2.2 `mtvec`

READ `TRM` register 1.7 and footnote 2: `MODE` (bits [1:0]) is read-only 1, "only vectored mode 0x1 is available"; `BASE` is bits [31:8], "aligned to 256 bytes"; reset value 0. Bits [7:2] are not described: ASSUMED read as 0.
Exceptions enter at `BASE`; interrupt n enters at `BASE + 4·n` (`TRM` table 1.5-1). Each slot holds one 4-byte jump (`IDF:riscv/vectors_intc.S:44-85`, `.balign 0x100`, `.option norvc`).
Values written: `0x40001D01` by the ROM (`ROM3:_init@0x40001ebe`), `0x40380001` by the application (insn 1,683,239). Both are 256-byte aligned with mode 1, so a model that keeps the written value behaves the same for this firmware.
QEMU (MEASURED, probe): accepts mode 0 or 1 and keeps every other bit (`0` reads back `0`, where silicon would read `0x00000001`); rejects a write with mode 2 or 3 entirely (`0xFFFFFFFF` leaves the old value). Its entry address is `(mtvec & ~3) + 4·n` in mode 1 and `mtvec & ~3` in mode 0, not `(mtvec & ~0xFF) + 4·n` (READ `QEMU:esp_cpu.c`).

### 2.3 Taking an interrupt, and returning

READ `TRM §1.5.2`; MEASURED values from C1.

On taking line n the CPU:

1. sets `mepc` to "the address of the current un-executed instruction";
2. sets `mcause` = `0x80000000 | n` ("the interrupt controller is using up IDs in range 1-31 for all external interrupt sources. This is different from the RISC-V standard", `TRM` register 1.10);
3. copies `MIE` into `MPIE`, clears `MIE` (and, as for any trap from machine mode, `MPP` = 3);
4. jumps to `(mtvec & ~0xFF) + 4·n`.

`mtval` "is not valid for […] interrupts" (QEMU writes 0).
MEASURED first interrupt (insn 2,369,224): `mstatus` `0x00000008` → `0x00001880`, `mepc` = `0x40381bc8`, `mcause` = `0x80000003`, next pc `0x4038000c`. MEASURED tick: `mstatus` `0x00000088` → `0x00001880`, `mepc` = `0x40381d1a` (the instruction after `wfi`), `mcause` = `0x80000004`, next pc `0x40380010`.

`mret`: "copies the state of MPIE back into MIE, and subsequently **clears MPIE**", then jumps to `mepc`. The RISC-V privileged specification sets `MPIE` to 1 on `mret`; the TRM says "clears". QEMU follows the specification: `mstatus` reads `0x00000088` after every `mret` (MEASURED: `MIE` = 1, `MPIE` = 1, `MPP` = 0). Which one silicon does is an open question (§7 item 6) with no effect on IDF: `MPIE` is rewritten by the next trap, and the handler restores `mstatus` from the value it read at entry.

`mstatus` fields on the C3 (READ `TRM` register 1.5): `MIE` (bit 3), `MPIE` (bit 7), `MPP` (bits [12:11], values 0 = user and 3 = machine only: "only lower bit is writable, write to the higher bit is ignored as it is directly tied to the lower bit"), `TW` (bit 21). Reset value: all four 0.
QEMU (MEASURED, probe): `csrw mstatus, 0xFFFFFFF7` reads back `0x807E79A2` (its generic RISC-V mask: `FS`, `SPP`, `SPIE`, `SIE`, `MPRV`, … are writable); `mvendorid`, `marchid`, `mimpid` read 0; `misa` reads `0x401411AD`.

What `_interrupt_handler` does with the CSRs (READ `IDF:riscv/vectors.S:384-513`; MEASURED `APP:_interrupt_handler@0x403801d8`):

| Step | Instruction (first occurrence, insn) | Effect |
|---|---|---|
| 1 | save 29 registers, `csrr t0, mepc` (2,369,256), store `gp`, `sp` | context on the interrupted stack |
| 2 | `call rtos_int_enter` | switches to the ISR stack; reprograms the stack guard (`ASSIST_DEBUG`) once the scheduler runs |
| 3 | `csrr s1, mcause` (2,369,267), `csrr s2, mstatus` (2,369,268) | `s2` = `0x1880` |
| 4 | `lw s3, CPU_INT_THRESH`; `lw t2, CPU_INT_PRI_n` with n = `mcause & 0x1F`; `sw t2+1, CPU_INT_THRESH`; `fence` (2,369,271 to 2,369,281) | software nesting: only lines of strictly higher priority can now be taken |
| 5 | `csrs mstatus, 8` (2,369,283) | `MIE` = 1 inside the handler: `0x1888` |
| 6 | `jal _global_interrupt_handler(sp, n)` | calls the C handler registered for line n |
| 7 | `csrc mstatus, 8` (2,369,393); `sw s3, CPU_INT_THRESH`; `fence` (2,369,396, 2,369,397) | back to `0x1880`, threshold restored |
| 8 | `call rtos_int_exit(s2)` | may switch `sp` to another task's saved context; returns the `mstatus` to restore |
| 9 | `csrw mcause, s1` (2,369,517); `csrw mstatus, a0` (2,369,518); `csrw mepc` from the frame (2,369,520); restore registers; `mret` (2,369,551) | resumes the interrupted task, or another one |

So the core must accept **writes of `mcause`, `mstatus` (with `MPP` = 3) and `mepc`**, execute `fence` as a no-op, and on `mret` resume with `MIE` = `MPIE`.

### 2.4 `mie` and `mip`

What is established:

- They are not in the TRM's CSR list (READ `TRM §1.4.1`), with the caveat of §2.1.
- No ROM, bootloader or application instruction reads or writes them (§2.1), and esp-hal never touches `mie` on the C2/C3 while it writes `mie = 0xFFFFFFFF` on the C6/H2, whose interrupt controller is a different one (READ `ESPHAL:esp-hal/src/interrupt/riscv.rs:217-220`, under `#[cfg(plic)]`); its start-up code clears `mie` and `mip` only under a feature named `has-mie-mip`, "indicate that the device supports `mie` and `mip` instructions", that no chip of v0.20.1 enables (READ `ESPHAL:esp-riscv-rt/src/lib.rs:344-347`, `esp-riscv-rt/Cargo.toml:21-22`).
- Therefore, **on silicon, delivery of lines 1..31 does not depend on any write to `mie`**: software that never writes it gets its interrupts. Whether `mie` exists and reads as some constant, or traps as an illegal instruction, is **not established** (§7 item 5).
- `TRM §8.3.2`: "The ESP32-C3 implements its interrupt mechanism using an interrupt controller instead of RISC-V Privileged ISA specification."

What QEMU does (MEASURED, probe; READ `QEMU:esp_cpu.c`, `QEMU:esp32c3_intmatrix.c`): the 31 lines are ORed into the standard machine external interrupt (`mip` bit 11), delivered by the generic RISC-V code under `mstatus.MIE && mie[11]`, after which `mcause` and the pc are rewritten for the line.

| Probe | Q1 | Q2 |
|---|---|---|
| `csrr mie` in a task | `0x00000EEE` (set by the controller model at reset) | `0x00000EEE` |
| `csrw mie, 0` then read | `0` | same |
| Ticks delivered during 35 ms with `mie` = 0 | **0: `mie` gates delivery** | same |
| `csrw mie, 0xFFFFFFFF` then read | `0xFFFFFEEE` | same |
| `csrr mip` in a task | `0` | `0` |
| `csrr mip` with a tick pending and `mstatus.MIE` = 0 | `0` | `0x00000800` |
| `mip` between interrupt entry and the handler's `csrs mstatus` (C1) | `0x00000800` | not traced |
| `csrr mip` with a tick pending but masked by `THRESH` = 2, or with its line disabled | `0` | `0` |

Recommendation for an emulator: lines 1..31 must not be gated by `mie`. For `mie`/`mip` themselves choose one of two defensible models and mark it as an assumption: (a) illegal instruction, which makes a firmware that depends on them fail loudly; (b) QEMU's (`mie` resets to `0xEEE`, `mip` bit 11 = some line pending). No measured firmware distinguishes them.

### 2.5 Nesting

There is no hardware nesting and no hardware change of the threshold (`SOC_INT_HW_NESTED_SUPPORTED` is not defined for the C3). Nesting is done in software by step 4 and 5 of §2.3; `TRM §1.5.3.2` describes the same scheme and requires the handler to save `mepc`, `mstatus`, `mcause` first.
MEASURED: no nested interrupt occurs in F1. During all 1,112 handlers of the first life no line of priority above 1 is asserted.

### 2.6 `wfi`

- READ `TRM` register 16.8 (`SYSTEM_CPU_PER_CONF`): "after executing the WFI instruction, CPU enters the wait mode, during which the clock gate of CPU is turned off **until any interrupts occur**"; if `CPU_WAIT_MODE_FORCE_ON` (bit 3, reset value 1) is set the clock is not gated. `CPU_WAITI_DELAY_NUM` (bits [7:4], reset 0) is the number of cycles before the gate closes. The application clears `CPU_WAIT_MODE_FORCE_ON` in `rtc_init` (READ `IDF:esp_hw_support/port/esp32c3/rtc_init.c:148`; MEASURED BA §3 row 35).
- READ `TRM` register 1.12: the performance counter's cycle event "does not increment during WFI mode". On hardware `mpccr` (`0x7E2`/`0x802`) therefore stops while the idle task waits; in QEMU it is derived from virtual time and keeps counting.
- The only `wfi` executed is the idle hook's (`IDF:esp_system/freertos_hooks.c:58` → `esp_cpu_wait_for_intr`, `APP@0x40381d02`), with `mstatus.MIE` = 1 and the threshold at 1. Before it, `esp_cpu_wait_for_intr` reads `ASSIST_DEBUG.C0RE_0_DEBUG_MODE` (`0x600CE098`): if a debugger is attached and `CPU_WAIT_MODE_FORCE_ON` is 0 it returns without waiting (READ `IDF:esp_hw_support/cpu.c:52-62`; MEASURED one read of 0 per idle iteration, `SYSTEM.CPU_PER_CONF` not read).
- MEASURED: all 1,106 tick interrupts of F1 are taken on that `wfi`, with `mepc` = `0x40381d1a` = address of `wfi` + 4: execution resumes after the `wfi` when the handler returns. This matches the RISC-V specification (the interrupt is taken on the following instruction).
- **`wfi` with `mstatus.MIE` = 0** (MEASURED, probe variant `wfi`): Q1 never wakes (the interrupt watchdog then resets the machine: `rst:0x8 (TG1WDT_SYS_RST)`); Q2 resumes at the next instruction when the tick becomes pending, 1,598,673 counter ticks later (10 ms), without taking it. The RISC-V specification says Q2's behaviour. Silicon: not established (§7 item 4). IDF does not do this on the measured path.
- What "pending" means for waking (raw source level, or `pending(n)` of §1.2): the TRM says "any interrupts"; ASSUMED `pending(n)` for some n, i.e. an enabled line that passes the threshold, regardless of `mstatus.MIE`.

### 2.7 Latency

READ `TRM §1.5.3.1`: "In steady state operation, the Interrupt Controller has a fixed latency of **4 cycles** […] any interrupt that is asserted to the controller will take exactly 4 cycles before the CPU starts processing the interrupt. This further implies that CPU may execute **up to 5 instructions** before the preemption happens." After a write to any controller register it is in a "transient state, which may take up to 4 cycles", during which "the ordering of interrupts may not be predictable". The advised sequence is: clear `MIE`, modify the registers, execute `fence`, restore `MIE`.
IDF is written for this latency: `vPortYield` raises the software interrupt then spins on `while (port_xSchedulerRunning && port_uxCriticalNesting == 0 && CPU_INTR_FROM_CPU_0 != 0) {}` ("3-4 instructions of latency"), and `vPortClearInterruptMaskFromISR` ends with three `nop` ("up to three machine cycles/instructions can be executed") (READ `IDF:freertos/FreeRTOS-Kernel/portable/riscv/port.c`, functions `vPortYield`, `vPortClearInterruptMaskFromISR`).
QEMU (MEASURED): **zero**. All 12 software interrupts of the first life are taken immediately after the `sw` that sets `CPU_INTR_FROM_CPU_0` (`0x40381bc6`, `mepc` = `0x40381bc8`); the wait loop then reads the register once, as 0 (11 reads at `0x40383f94`).
An emulator may deliver at the next instruction boundary (QEMU-compatible, and correct for IDF) or after a few cycles (hardware-like). The first choice is required for a pc-level lockstep against QEMU.

### 2.8 Exceptions on the panic path

All exceptions enter at `mtvec` base (slot 0, `_panic_handler`). `mcause` values on this core (READ `TRM` register 1.10): 1 PMP instruction access fault, 2 illegal instruction, 3 breakpoint/watchpoint/`ebreak`, 5 PMP load access fault, 7 PMP store access fault, 8 `ecall` from U, 11 `ecall` from M; no instruction-misaligned exception. `mtval` holds the faulting address (1, 5, 7) or the faulting opcode (2).
MEASURED (probe, Q1): `abort()` ends in `panic_abort`, which executes `unimp` (`0x0000` at `APP-probe@0x40382f6c`; READ `IDF:esp_system/panic.c:491`): `mcause` = 2, `mepc` = the `unimp`, `mstatus` = `0x00001880`. A store to address 0 gives `mcause` = 7, `mtval` = 0 (the locked PMP entries of `esp_cpu_configure_region_protection` forbid it; inkemu stores PMP without enforcing it and will not raise this fault).

## 3. The measured sequence (trace F1, QEMU Q1)

### 3.1 Timeline

| insn | Event |
|---|---|
| 22 | ROM `_init`: `mstatus` = `0x8` (`MIE` = 1). **`MIE` stays 1 through the ROM, the bootloader and the application start-up**; nothing is delivered because `CPU_INT_ENABLE` is 0 |
| 1,683,239 | `csrw mtvec, 0x40380001` |
| 2,186,120 | `core_intr_matrix_clear`: the 62 map registers ← 0 |
| 2,194,350 to 2,388,696 | Lines set up (§3.2) |
| 2,356,724 / 2,359,524 | `vTaskStartScheduler` / `xPortStartScheduler` entered |
| 2,368,922 to 2,369,122 | `vSystimerSetup`: tick alarm programmed, `SYSTIMER.INT_ENA` = 1 (2,369,110), counter enabled (2,369,122) |
| 2,369,140 | `CPU_INT_THRESH` ← 1 |
| 2,369,224 | **Interrupt 1**: software interrupt, line 3 → first task (§3.3) |
| 2,369,551 | `mret` to `vPortTaskWrapper@0x40383ce2`; `main_task` at 2,369,558 |
| 2,374,473 | `I (28) main_task: Started on CPU0` |
| 2,388,614 | task watchdog: `TG_WDT_INT_MAP` ← 5 |
| 2,395,944 / 2,396,803 | `app_main` / `Hello world!` |
| 2,419,952 | `Restarting in 10 seconds...` |
| 2,422,426 / 2,422,698 | `vTaskDelay` / **interrupt 2**: software interrupt, line 3 → idle task |
| 2,423,330 | **Interrupt 3**: first tick, line 4, taken on `wfi` (§3.4) |
| 2,423,330 to 2,507,086 | 100 ticks, one every 839 instructions; the 100th wakes `main_task` from its handler |
| 2,508,658 | `Restarting in 9 seconds...`, then `vTaskDelay`: software interrupt at 2,511,332 |
| … | the same second repeats: software interrupts at 2,599,976, 2,688,620, 2,777,264, 2,865,908, 2,954,552, 3,043,196, 3,131,837, 3,220,481, 3,309,124 |
| 3,392,829 | Last tick of the first life |
| 3,394,472 | `Restarting now.` |
| 3,396,588 / 3,401,606 | `esp_restart` / `esp_restart_noos` (§4.1) |
| 3,402,177 | `RTC_CNTL.OPTIONS0` ← `0x20`; next pc `0x40000000` |
| 3,797,997 | `rst:0xc (RTC_SW_CPU_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)` |
| 5,779,677 / 5,808,510 | First interrupt of the second boot / `Hello world!` |

Interrupts taken in the first life: **1,112 = 12 on line 3 + 1,100 on line 4**. No other line fires. `-d int` logs no exception.
Distance between two idle ticks: 839 instructions (1,086 of 1,099 intervals); the OS-tick counter read by the handler advances by 160,000 ± 200 between ticks (10 ms at 16 MHz).

### 3.2 Lines set up before and after the scheduler starts

| insn (map write) | Source (number) | Line | Priority | Type | Enabled | Set by | Vector |
|---|---|---|---|---|---|---|---|
| 2,194,371 / 2,194,383 | `CACHE_IA` (36), `CACHE_CORE0_ACS` (61) | 25 | 4 | level | 2,194,441 | `esp_cache_err_int_init` | `_panic_handler` |
| 2,194,745 / 2,194,844 / 2,194,946 | `CORE_0_IRAM0_PMS_MONITOR_VIOLATE` (56), `…DRAM0…` (57), `…PIF…` (58) | 26 | 4 | level | 2,194,787 (disabled and re-enabled around each map write) | `esp_mprot_set_intr_matrix` | `_panic_handler` |
| 2,302,990 | `RTC_CORE` (27) | 2 | 1 | level | 2,303,011 | `esp_brownout_init` → `esp_intr_alloc` | `_interrupt_handler` |
| 2,319,686 | `ASSIST_DEBUG` (54) | 27 | 4 | level | 2,319,732 | `esp_hw_stack_guard_init` | `_panic_handler` |
| 2,343,343 | `TG1_WDT` (35) | 24 | 4 | level | 2,343,385 | `esp_int_wdt_cpu_init` | `_panic_handler` |
| 2,352,136 | `CPU_INTR_FROM_CPU_0` (50) | 3 | 1 | level | 2,352,157 | `esp_crosscore_int_init` | `_interrupt_handler` → `esp_crosscore_isr` |
| 2,368,368 | `SYSTIMER_TARGET0` (37) | 4 | 1 | level | 2,368,389 | `vSystimerSetup` | `_interrupt_handler` → `SysTickIsrHandler` |
| 2,388,614 | `TG_WDT` (33, timer group 0) | 5 | 1 | level | 2,388,635 | `esp_task_wdt_init` (in `main_task`) | `_interrupt_handler` → `task_wdt_isr` |

Final state: `CPU_INT_ENABLE` = `0x0F00003C`, `CPU_INT_TYPE` = 0, `CPU_INT_THRESH` = 1, priorities as above (and `PRI_5` = 1 already from the ROM).
Around flash operations `esp_intr_noniram_disable`/`enable` rewrite `CPU_INT_ENABLE` (four pairs, insn 2,308,035 to 2,319,090): every line enabled at that time is IRAM-safe, so the value does not change.

### 3.3 Interrupt 1: the scheduler starts

| insn | pc | Access or CSR | Notes |
|---|---|---|---|
| 2,369,140 | ROM `0x400529a6` | W `CPU_INT_THRESH` = 1 | `xPortStartScheduler` |
| 2,369,142 | `0x40383fc8` | `csrrsi mstatus, 8` | `MIE` already 1 |
| 2,369,166 to 2,369,213 | `esp_crosscore_int_send` | `csrrci`; R `THRESH` (1); W `THRESH` = 4; `csrrs`; …; W `THRESH` = 1 | critical section around the reason word |
| 2,369,224 | `0x40381bc6` | **W `SYSTEM.CPU_INTR_FROM_CPU_0` (`0x600C0028`) = 1** | source 50 asserted |
| (same) | | `mepc` = `0x40381bc8`, `mcause` = `0x80000003`, `mstatus` `0x8` → `0x1880` | taken before the next instruction |
| 2,369,225 / 2,369,226 | `0x4038000c` / `0x403801d8` | vector slot 3: `j _interrupt_handler` | |
| 2,369,256 to 2,369,268 | | `csrr mepc`, `csrr mcause`, `csrr mstatus` | no `ASSIST_DEBUG` access yet: the scheduler is not running |
| 2,369,271 / 2,369,278 / 2,369,280 | `0x40380236` … | R `THRESH` (1); R `CPU_INT_PRI_3` (1); W `THRESH` = 2; `fence` | |
| 2,369,283 | `0x4038025c` | `csrs mstatus, 8` → `0x1888` | QEMU logs `mip` `0x800` → 0 here: the line was masked by the threshold write three instructions earlier |
| 2,369,314 | `0x40381b50` | **W `CPU_INTR_FROM_CPU_0` = 0** | `esp_crosscore_isr`: source cleared, then `vPortYieldFromISR` sets `port_xSchedulerRunning` |
| 2,369,319 to 2,369,361 | | `THRESH` 2 → 4 → 2 | a critical section inside the handler |
| 2,369,393 / 2,369,396 | `0x40380270` / `0x4038027c` | `csrc mstatus, 8`; W `THRESH` = 1; `fence` | |
| 2,369,499 to 2,369,513 | `restore_stack_pointer` | `ASSIST_DEBUG.CORE_0_MONTR_ENA` ← 0; `CORE_0_SP_MIN` ← `0x3FC8DBCC`; `CORE_0_SP_MAX` ← `0x3FC8EBC0`; `MONTR_ENA` ← `0x300` | stack guard set to the new task's stack; repeated at every context switch and, from now on, at every interrupt entry |
| 2,369,517 to 2,369,520 | | `csrw mcause`; `csrw mstatus` (`0x1880`); `csrw mepc` = `0x40383ce2` | |
| 2,369,551 | `0x403802d6` | `mret` → pc `0x40383ce2` (`vPortTaskWrapper`), `mstatus` = `0x88` | the main task runs |

Without delivery, `vPortYield` returns (the wait loop is skipped while `port_xSchedulerRunning` is 0) and the CPU ends in the `j .` of `start_cpu0_default` (BA §9). With hardware-like latency the interrupt is simply taken a few instructions later, anywhere before or inside that loop.

### 3.4 A tick, and the idle loop

First tick (the others are identical up to counter values):

| insn | Access or CSR | By |
|---|---|---|
| 2,423,325 | R `ASSIST_DEBUG.C0RE_0_DEBUG_MODE` = 0 | `esp_cpu_wait_for_intr` |
| 2,423,330 | `wfi` at `0x40381d16`; **interrupt on line 4**: `mepc` = `0x40381d1a`, `mcause` = `0x80000004`, `mstatus` `0x88` → `0x1880` | |
| 2,423,379 to 2,423,396 | `ASSIST_DEBUG`: `MONTR_ENA` ← 0, `SP_MIN`/`SP_MAX` ← ISR stack (`0x3FC8CB80`/`0x3FC8D180`), `MONTR_ENA` ← `0x300` | `rtos_int_enter` |
| 2,423,403 to 2,423,415 | R `THRESH` (1); R `CPU_INT_PRI_4` (1); W `THRESH` = 2; `fence`; `csrs mstatus` | `_interrupt_handler` |
| 2,423,447 / 2,423,449 | R / **W `SYSTIMER.INT_CLR` = 1** | `SysTickIsrHandler`: clears the source, so the level line drops |
| 2,423,456 to 2,423,484 | `SYSTIMER.UNIT1_OP` update and poll, `UNIT1_VALUE_LO/HI`, R `TARGET0_CONF` | counts the ticks elapsed (more than one is processed if the handler was late) |
| 2,423,584 to 2,423,662 | `TIMG1`: unlock, stage 0 ← 600, stage 1 ← 1,200, feed, lock | interrupt-watchdog tick hook (BA §6) |
| 2,423,742 to 2,423,788 | `THRESH` 2 → 4 → 2 | critical section |
| 2,423,803 | R `SYSTIMER.INT_ST` = 0 | loop exit test |
| 2,423,815 / 2,423,818 | `csrc mstatus`; W `THRESH` = 1; `fence` | |
| 2,423,841 to 2,423,855 | `ASSIST_DEBUG` back to the task's stack | `restore_stack_pointer` |
| 2,423,893 | `mret` → `0x40381d1a`, `mstatus` = `0x88` | |
| 2,423,954 to 2,424,074 | critical section; `TIMG0`: unlock, feed, lock | idle hook feeding the task watchdog |
| 2,424,166 / 2,424,171 | R `C0RE_0_DEBUG_MODE`; `wfi` → next tick | |

The yield out of `vTaskDelay` (insn 2,422,698) is §3.3 again with `rtos_int_enter` reprogramming the stack guard, and with the wait loop reading `CPU_INTR_FROM_CPU_0` = 0 once after the handler returns.
Because the machine is idle, **every tick of F1 arrives during `wfi`**: no interrupt preempts running code at an arbitrary instruction in this trace.

## 4. `esp_restart()` and the software reset

### 4.1 Register sequence (MEASURED F1; READ `IDF:esp_system/esp_system.c:47-64`, `IDF:esp_system/port/soc/esp32c3/system_internal.c:31-116`)

| insn | Step | Accesses (QEMU values) | On hardware |
|---|---|---|---|
| 3,396,588 | `esp_restart`: shutdown handlers | `RTC_CNTL.STORE2/3` read and written twice (boot-time bookkeeping, last value `0x00A7DB82`/`0`), R `STORE1`, RTC timer read (`TIME_UPDATE` bit 31, `TIME_LOW0`, `TIME_HIGH0`), SYSTIMER unit 0 reads | same |
| | `vTaskSuspendAll` | critical sections | |
| 3,401,606 / 3,401,610 | `esp_restart_noos`: `csrrci mstatus, 8` | `mstatus` `0x88` → `0x80` | interrupts off for good |
| 3,401,638 to 3,401,713 | `wdt_hal_init(RWDT)` | `WDTWPROTECT` ← `0x50D83AA1`; `WDTCONFIG0` cleared field by field then `PROCPU_RESET_EN`, `PAUSE_IN_SLP`, reset lengths 7/7; `INT_CLR_RTC` ← 8; `INT_ENA_RTC` bit cleared; `WDTWPROTECT` ← 0 | **the RWDT is disabled here** (`EN` = 0) |
| 3,401,808 to 3,401,893 | RWDT stages | unlock; stage 0 action 3 (`WDTCONFIG0[30:28]`), R `EFUSE.RD_REPEAT_DATA1` (`WDT_DELAY_SEL`), `WDTCONFIG1` ← `0x109A0` (68,000); stage 1 action 4 (`[27:25]`), `WDTCONFIG2` ← `0x21340` (136,000); **`FLASHBOOT_MOD_EN` (bit 12) ← 1**; lock | Two stages of 1 s at the 136 kHz slow clock: stage 0 = core reset, stage 1 = system reset. `EN` is not set: the watchdog is armed only by the flash-boot protection (BC §E.3, `TRM §12.2.2.4`) |
| 3,401,908 to 3,401,959 | MWDT0 then MWDT1 | unlock; `WDTCONFIG0` `0xB83F8000` → `0x383F8000` (`EN` cleared); lock | same |
| 3,401,965 to 3,402,015 | ROM `Cache_Disable_ICache` | `EXTMEM.ICACHE_CTRL` 1 → 0; autoload and preload reads; invalidate all (`ICACHE_SYNC_ADDR` ← 0, `SYNC_SIZE` ← 0, `SYNC_CTRL` bit 0, poll bit 1); R `CACHE_STATE` | same. From here only IRAM and ROM code runs |
| 3,402,025 to 3,402,030 | SPI flash pins back to ROM defaults | `GPIO.FUNC0..5_IN_SEL_CFG` ← `0x30` | same |
| 3,402,042 to 3,402,062 | `esp_system_reset_modules_on_exit`: is each UART enabled? | R `SYSTEM.PERIP_RST_EN0`, R `PERIP_CLK_EN0`, twice; QEMU reads 0, so **no wait** | UART0's clock bit is set: `esp_rom_output_tx_wait_idle(0)` polls `UART0.STATUS` until the TX FIFO is empty and the transmitter idle. **An emulated UART must reach that state** |
| 3,402,069 to 3,402,074 | radio reset pulse | `0x60026018` (`SYSTEM_CORE_RST_EN`, SVD name `APB_CTRL.WIFI_RST_EN`): R, W `0x3FAF`, W 0 | same |
| 3,402,076 to 3,402,079 | UART0 core reset | `UART0.CLK_CONF` ← old \| bit 23 (`RST_CORE`), left set: "rom will clear this bit" | same |
| 3,402,081 to 3,402,086 | peripheral reset pulse | `PERIP_RST_EN0`: R, W `0x20000007` (`TIMERS` bit 0, `SPI01` bit 1, `UART` bit 2, `SYSTIMER` bit 29), W 0 | **these modules are reset by this pulse, not by the CPU reset** |
| 3,402,088 to 3,402,091 | crypto and DMA reset pulse | `PERIP_RST_EN1`: R, W `0x7E` (AES, SHA, RSA, DS, HMAC, DMA), W 0 | same |
| 3,402,103 to 3,402,159 | `rtc_clk_cpu_set_to_default_config` | R `STORE4`; `SYSCLK_CONF`: `PRE_DIV_CNT` ← 0 (twice), `SOC_CLK_SEL` ← 0 (crystal); `STORE5` ← `0x26252625` (40 MHz); `ets_update_cpu_frequency(40)` | CPU on the 40 MHz crystal **with divider 1** (`PRE_DIV_CNT` = 0); the PLL is left running. QEMU does not store `SYSCLK_CONF` (reads back `0xA8401`) |
| 3,402,174 / 3,402,177 | ROM `software_reset_cpu(0)@0x40048b84` | R `RTC_CNTL.OPTIONS0`; **W `OPTIONS0` = old \| `0x20`** (`SW_PROCPU_RST`, bit 5) at `0x40048b90` | |
| 3,402,178 | | pc = `0x40000000` | QEMU: immediate. Hardware: after the store the ROM routine returns and IDF spins in `ESP_INFINITE_LOOP()`; how many instructions run before the reset is not established (§4.4) |

`software_reset@0x40048b74` (bit 31, `SW_SYS_RST`, reason 3) is the same two instructions with the other bit; IDF does not use it on this path. `OPTIONS0`'s other bits must survive the read-modify-write.

### 4.2 What a "CPU reset" is, and what survives

READ `TRM §6.1` (figure 6.1-1, table 6.1-1):

| Reset level | Scope | Sources (code in `RTC_CNTL.RESET_STATE[5:0]`) |
|---|---|---|
| CPU reset | "Only resets CPU core". "If CPU is reset, PMS registers will be reset, too" | MWDT0 CPU (`0x0B`), **software CPU (`0x0C`)**, RWDT CPU (`0x0D`), MWDT1 CPU (`0x11`) |
| Core reset | "The whole digital system except RTC, including CPU, peripherals, Wi-Fi, Bluetooth LE, and digital GPIOs" | software system (`0x03`), deep sleep (`0x05`), MWDT0 (`0x07`), MWDT1 (`0x08`), RWDT core (`0x09`), eFuse CRC (`0x14`), USB-UART (`0x15`), USB-JTAG (`0x16`), power glitch (`0x17`) |
| System reset | "The whole digital system, including RTC" | RWDT system (`0x10`), super watchdog (`0x12`), clock glitch (`0x13`); brown-out (`0x0F`) by configuration |
| Chip reset | "The whole chip" | power-on (`0x01`), brown-out (`0x0F`) |

"All reset types mentioned above (except Chip Reset) maintain the data stored in internal memory."

So after `esp_restart()` on hardware (reason `0x0C`):

| State | After the software CPU reset | Source |
|---|---|---|
| pc, `mstatus`, `mtvec`, PMP (lock bits included), trigger CSRs | reset: pc = `0x40000000`, `mstatus` = 0 | `TRM §6.1.3`; register reset values of `TRM §1.4.2`. That a hart reset clears PMP lock bits is the RISC-V rule: ASSUMED for this core |
| `SENSITIVE` (PMS) registers | reset | `TRM §6.1.3` note |
| `ASSIST_DEBUG.CORE_0_RCD_EN` | reset; **`CORE_0_RCD_PDEBUGPC` keeps "the PC value at the CPU reset"** | `TRM §17.4.2` |
| Internal SRAM, RTC memory | kept (the ROM then re-initialises its own `.data`/`.bss`) | `TRM §6.1.1` |
| Every other peripheral register: `SYSTEM` (clock selection, `PERIP_CLK_EN0/1`), `EXTMEM` and the MMU table, GPIO, IO MUX, UART, SPI0/1, SYSTIMER, TIMG, SHA, USB-Serial-JTAG, eFuse mirror | **kept**, except what the pulses of §4.1 reset explicitly. IDF relies on this: "For reason that only reset CPU, do not disable the clocks that have been enabled before reset" | `TRM §6.1.3`; `IDF:esp_system/port/soc/esp32c3/clk.c:219-230` |
| `INTERRUPT_CORE0` | not established: the TRM describes the controller in the CPU chapter, its registers in the interrupt-matrix block. The software does not depend on it: the ROM clears `CPU_INT_ENABLE` as its sixth instruction and sets `CPU_INT_THRESH`, IDF clears the 62 map registers and rewrites each priority it uses | `ROM3:_init@0x40001ea0`; MEASURED |
| `RTC_CNTL` entirely: `STORE0..7`, RWDT configuration, `OPTIONS0`, clocks | kept | RTC is outside even a core reset |
| `RTC_CNTL.RESET_STATE[5:0]` | `0x0C` | table 6.1-1 |
| Flash content, eFuse | kept | |
| `mpccr` (`0x7E2`), `mscratch`, `mepc`, `mcause` | not established; ASSUMED reset with the core | |

`RESET_STATE` also holds `STAT_VECTOR_SEL_PROCPU`/`APPCPU` (bits 13 and 12, reset value 1), `JTAG_RESET_FLAG_PROCPU` (bit 20, cleared by writing bit 22): on hardware the register reads `0x300C`-like values, not `0x0C` (READ `IDF:soc/esp32c3/register/soc/rtc_cntl_reg.h:456-535`). Readers mask with `0x3F`.

### 4.3 The second boot

What the ROM does with the reason (READ `ROM3:main@0x40047e3c`): reasons 1, 3, 7 to 13, 15 to 23 take the normal boot path; reason 5 (deep sleep) goes through `rtc_boot_control`; any other value takes an error path. Reason `0x0C` is therefore booted exactly like power-on. `print_other_reset_reason@0x40047e0a` reads `SWD_CONF` only when the reason is 1, and always reads `RESET_STATE` bit 20 (JTAG reset flag).

MEASURED differences between the two boots in F1 (53,591 and 53,511 register accesses up to `Hello world!`, console and SPI1 values aside), with what hardware does (READ):

| Phase | Second boot in QEMU | Hardware |
|---|---|---|
| ROM | `RESET_STATE` reads `0x0C`; `STORE4` reads `0x00280028` (kept); `SWD_CONF` not read; banner `rst:0xc (RTC_SW_CPU_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)`. Otherwise the same accesses at the same instruction numbers until the banner | same, **plus one more line, `Saved PC:0x…`** (§4.4). `STORE5` is rewritten by `boot_prepare` from the current clock: 40 MHz → `0x26252625` |
| ROM | `SPI1.USER`, `CTRL2`, `USER2`, `MISO_DLEN` read the values the first boot left (QEMU does not reset its SPI1 registers) | reset by the `SPI01` pulse |
| Bootloader, `bootloader_clock_configure` | reason is `RESET_REASON_CPU0_SW` and `STORE5` says 80 MHz (QEMU's `SYSCLK_CONF` always reads PLL): **`rtc_clk_init` is skipped** (2 accesses instead of 51) | `STORE5` says 40 MHz, below `APB_CLK_FREQ`: **`rtc_clk_init` runs as at power-on** (READ `IDF:bootloader_support/src/bootloader_clock_init.c`, `bootloader_clock_configure`: `if (esp_rom_get_reset_reason(0) != RESET_REASON_CPU0_SW || rtc_clk_apb_freq_get() < APB_CLK_FREQ)`). The two machines execute different code here |
| Bootloader | log timestamps start at `I (22065)`: the cycle counter is not reset | not established (§7 item 7) |
| Application, `rtc_init` | 30 accesses fewer: the OCode calibration (`set_ocode_by_efuse`, regi2c) runs only after a power-on reset (`IDF:esp_system/port/soc/esp32c3/clk.c:63-72`) | same |
| Application, `rtc_clk_cal_internal` | `TIMG0.RTCCALICFG` reads `0x04000000` instead of `0x13000` (QEMU's timer-group state after its reset) | `TIMG` is not reset by the CPU reset; bit 0 of `PERIP_RST_EN0` (`SYSTEM_TIMERS_RST`) was pulsed. Not established which registers that pulse resets |
| Application, `esp_clk_slowclk_cal_get` | `STORE1` reads `0x003AD2CD` (kept) | same |
| Application, `esp_perip_clk_init` | reads `PERIP_CLK_EN0/1` and `WIFI_CLK_EN` and keeps the enabled clocks (reason is a CPU reset) instead of writing the power-on set | same |
| Application, SYSTIMER | unit 0 counter continues (`0x0A86E892` where the first boot read `0x0004A36F`) | reset by the `SYSTIMER` pulse: ASSUMED restarts from 0 |
| Interrupts | same lines, same order; first interrupt at second-boot instruction 2,377,500 instead of 2,369,224 | |

Instructions from reset to `Hello world!`: 2,396,803 (first boot), 2,406,333 (second).

### 4.4 `Saved PC`

READ `ROM3:boot_prepare@0x40047dd2-0x40047dee`: after printing the `rst:` line the ROM enables the debug-assistant clock, reads `ASSIST_DEBUG.CORE_0_RCD_PDEBUGPC` (`0x600CE048`), prints `Saved PC:0x%08x\n` (string at `0x3ff1b8bc`) **if the value is not 0**, then writes 3 to `CORE_0_RCD_EN` (`0x600CE044`: `RCD_RECORDEN` | `RCD_PDEBUGEN`).
READ `TRM §17.4.2`: with both bits set, `RCD_PDEBUGPC` samples the CPU's pc continuously; "when the CPU resets, `RCD_EN` will reset, while `RCD_PDEBUGPC` will not. Therefore, the latter will keep the PC value at the CPU reset."
The application never writes `RCD_EN` (MEASURED), so recording stays on from the ROM to the reset. Hence on hardware every boot that follows a CPU reset prints, between the `rst:` line and `SPIWP:`, a line `Saved PC:0x<pc at the reset>`. After power-on the register is 0 and the line is absent.
QEMU reads 0 at `0x600CE048`: the line is never printed (MEASURED second-boot log).
Which pc is recorded for `esp_restart()` (the `sw` at `0x40048b90`, the `ret` at `0x40048b92`, or the `j .` of `esp_restart_noos` a few instructions later) is not established. ASSUMED from boot logs published by users: an application address for `RTC_SW_CPU_RST`, and `0x40048b82` (the `ret` of `software_reset`) for `RTC_SW_SYS_RST`.

### 4.5 Panic and abort (MEASURED, probe on Q1)

| insn (probe trace) | Step |
|---|---|
| 12,539,923 | `unimp` in `panic_abort`: exception, `mcause` = 2, vector slot 0 → `_panic_handler` → `panic_from_exception` |
| 12,540,031 to 12,540,438 | `esp_panic_handler`: feed MWDT0 and MWDT1; reconfigure the RWDT (one stage, action 4, `WDTCONFIG1` ← `0xA6040`, **enabled**: `WDTFEED` bit 31, `WDTCONFIG0` bit 31); disable MWDT0 and MWDT1 |
| 12,540,447 to 12,540,482 | R `ASSIST_DEBUG.C0RE_0_DEBUG_MODE`; R `EXTMEM.CORE0_ACS_CACHE_INT_ST`, `CACHE_ILG_INT_ST` (is a cache error the real cause?) |
| 18,802,845 to 19,170,654 | `abort() was called at PC 0x… on core 0`, register dump, stack dump, `ELF file SHA256`, `Rebooting...`; the three watchdog enable bits are polled between characters |
| 19,170,736 onwards | `esp_restart_noos`: **the same sequence as §4.1**, ending with `OPTIONS0` ← old \| `0x20` |
| next boot | `rst:0xc (RTC_SW_CPU_RST)` |

The illegal-instruction and store-fault variants differ only by the first line (`Guru Meditation Error: Core  0 panic'ed (Illegal instruction)` / `(Store access fault)`) and the register dump. A panic is thus, for the hardware, an exception followed by a software CPU reset: no new mechanism.

### 4.6 Reset cause as seen by the application

`esp_reset_reason()` combines `RESET_STATE[5:0]` with a **hint** stored in `RTC_CNTL.STORE6` (`0x600080C0`): value `0x80000000 | hint << 16 | hint`, written by the panic handler (`ESP_RST_PANIC`, `ESP_RST_INT_WDT`, `ESP_RST_TASK_WDT`) or the brown-out handler before the reset, read by a constructor at the next start, then cleared (READ `IDF:esp_system/port/soc/esp32c3/reset_reason.c:66-116`, `IDF:esp_system/panic.c:450-458`). The hint is honoured only for reasons `0x0C` and `0x03`.
In `hello_world` the hint functions are weak empty stubs (nothing calls `esp_reset_reason`): `STORE6` is never accessed in F1 nor in the probe. A firmware that calls `esp_reset_reason()` (Arduino does) needs `STORE6` to survive the reset.

## 5. Sources an early application enables

Peripheral-side enables are MEASURED writes; "fires" refers to a healthy run of the reference application.

| Source (number) | Line, priority | Type | Peripheral-side enable | Handler | Fires on a healthy run? |
|---|---|---|---|---|---|
| `CPU_INTR_FROM_CPU_0` (50) | 3, 1 | level | none: the source is bit 0 of `SYSTEM.CPU_INTR_FROM_CPU_0` (`0x600C0028`), set by software and cleared by the handler | `esp_crosscore_isr` (yield, and other reasons on demand) | **Yes**: once to start the scheduler, then at every blocking call (12 in 11 s) |
| `CPU_INTR_FROM_CPU_1..3` (51..53) | not routed | | `0x600C002C`, `0x30`, `0x34`: never accessed | | No |
| `SYSTIMER_TARGET0` (37) | 4, 1 | level | `SYSTIMER.INT_ENA` bit 0; cleared by `INT_CLR` bit 0 | `SysTickIsrHandler` | **Yes**: every 10 ms (`CONFIG_FREERTOS_HZ` = 100), 1,100 times |
| `SYSTIMER_TARGET2` (39), `esp_timer` | not routed in this application (`esp_timer` is not linked into the minimal build) | level | | `esp_timer` ISR | An application using `esp_timer` (Arduino does) allocates it on the next free line |
| `TG_WDT` (33, MWDT0) | 5, 1 | level | `TIMG0.INT_ENA_TIMERS` bit 1 | `task_wdt_isr` | No: the idle task feeds MWDT0 at every tick. Fires after 5 s if a task starves the idle task (stage 0) |
| `TG1_WDT` (35, MWDT1) | 24, 4 | level | `TIMG1.INT_ENA_TIMERS` bit 1 | `_panic_handler` ("Interrupt wdt timeout on CPU0") | No: the tick handler feeds MWDT1. Fires 300 ms after ticks stop being served; system reset 600 ms later (BA §6) |
| `RTC_CORE` (27) | 2, 1 | level | `RTC_CNTL.INT_ENA_RTC` bit 9 (brown-out); detector configured in `RTC_CNTL.BROWN_OUT` | `rtc_isr` → brown-out handler (prints, then resets) | No |
| `CACHE_IA` (36), `CACHE_CORE0_ACS` (61) | 25, 4 | level | `EXTMEM.CACHE_ILG_INT_ENA` ← `0x23`, `CORE0_ACS_CACHE_INT_ENA` ← `0x3F` | `_panic_handler` ("Cache error") | No. Raised by an access through an invalid MMU entry or with the cache disabled |
| PMS violations (56, 57, 58) | 26, 4 | level | `SENSITIVE` monitor registers | `_panic_handler` ("Memory protection fault") | No |
| `ASSIST_DEBUG` (54) | 27, 4 | level | `ASSIST_DEBUG.CORE_0_INTR_ENA` ← `0x300`, `CORE_0_MONTR_ENA` ← `0x300` (SP below `SP_MIN` or above `SP_MAX`) | `_panic_handler` ("Stack protection fault") | No. **But `SP_MIN`/`SP_MAX` are rewritten at every interrupt entry and exit**: a model that compares `sp` must use them exactly, or not compare at all |
| UART0 (21) | 5 by the ROM, never enabled; unmapped by IDF | | | | No (the console transmits by polling) |

On a healthy run only two sources ever assert: the software interrupt and the tick.

## 6. Where QEMU differs from hardware

"Mask" = what a lockstep against QEMU must ignore or reproduce.

| # | Topic | Hardware (source) | QEMU (how known) | Lockstep |
|---|---|---|---|---|
| 1 | `mie` | Not needed for delivery; existence unknown (§2.4) | Resets to `0xEEE`; writing 0 blocks every line (MEASURED) | No measured firmware touches it. Mask the `mie`/`mip` columns of `-d cpu` |
| 2 | `mip` | Unknown | Bit 11 while a line is asserted; Q1 does not show it while `mstatus.MIE` = 0, Q2 does (MEASURED) | Mask |
| 3 | Arbitration | Highest priority, then lowest line (`TRM §1.5.2`) | Lowest asserted line, priorities ignored (READ) | Differs only when two lines of different priority are pending together: never in F1 |
| 4 | Priority 0 | Masked whatever the threshold | Passes when the threshold is 0 (READ) | Never reached: the ROM sets the threshold to 1 at insn 19 |
| 5 | `CPU_INT_EIP_STATUS`, `INTR_STATUS_0/1` | Live status | Always 0 (MEASURED) | Reads differ for any firmware that polls them; none measured |
| 6 | Edge lines, `CPU_INT_TYPE`, `CPU_INT_CLEAR` | Latch and toggle-to-clear | Not supported; `TYPE` reads 0 (READ, MEASURED) | Not on the measured path |
| 7 | `CPU_INT_PRI_31`, line 31 | Exists | Register not decoded (READ) | Not used |
| 8 | Latency | 4 cycles, up to 5 instructions (`TRM §1.5.3.1`) | 0 instructions (MEASURED) | Deliver at the next instruction boundary in lockstep mode |
| 9 | `wfi` with `MIE` = 0 | Unknown; specification says it resumes | Q1: sleeps for ever; Q2: resumes (MEASURED) | Not on the measured path |
| 10 | `mtvec` | `MODE` read-only 1, `BASE` = bits [31:8] | Mode 0 or 1 stored, all upper bits kept (MEASURED) | Identical for `0x40001D01` and `0x40380001` |
| 11 | `mstatus` | `MIE`, `MPIE`, `MPP` (0 or 3), `TW` | Generic mask (`0x807E79A2` writable); `MPIE` = 1 and `MPP` = 0 after `mret` (MEASURED) | inkemu today reads `MPP` as 3 always: either model `MPP` or mask bits [12:11] |
| 12 | `mvendorid`, `marchid`, `mimpid`, `misa` | `0x612`, `0x80000001`, `0x1`, (`misa` hard-wired) | 0, 0, 0, `0x401411AD` (MEASURED) | Not read by the firmware |
| 13 | Cycle counter during `wfi` | Stops (`TRM` register 1.12) | Runs on virtual time | Affects values read from `0x7E2`/`0x802` after the first idle period |
| 14 | Software CPU reset | CPU core only (§4.2) | Whole-machine reset; `RTC_CNTL` reason and `STORE0..7` kept (READ `QEMU:esp32c3_rtc_cntl.c`, `QEMU:esp32c3.c`); SPI1 registers and SYSTIMER counter kept, `SYSCLK_CONF` reads `0xA8401` (PLL) before and after (MEASURED) | **The second boot's bootloader takes a different branch** (§4.3): a pc-level lockstep cannot cross a reset unless inkemu reproduces QEMU's clock registers |
| 15 | Reset timing | Not established | Immediate after the store (MEASURED) | |
| 16 | `Saved PC` line | Printed after a CPU reset (§4.4) | Never (`0x600CE048` reads 0) | Console comparison of the second boot differs by one line |
| 17 | UART flush before reset | `esp_rom_output_tx_wait_idle(0)` runs | Skipped (`PERIP_CLK_EN0` reads 0) | Extra UART status reads on hardware |
| 18 | Register read-back | `RTC_CNTL.WDTCONFIG0`, `SYSCLK_CONF`, `PERIP_CLK_EN0`, `INT_ENA_RTC` hold what was written | Read 0 or a constant | Written values in read-modify-write sequences differ (BA §5, §6) |
| 19 | `RESET_STATE` | Reason plus bits 12, 13 (and flags) | Reason only | Masked by readers |
| 20 | Watchdog expiry | RWDT, MWDT0, MWDT1, super watchdog | MWDT0/1 modelled, including the interrupt and the reset (`rst:0x8` MEASURED in the `wfi` probe on Q1); the RWDT register file is not stored | |
| 21 | Q1 against Q2 on the reference image | | First 2,472,463 instructions identical, including the first 61 interrupt entries; then inside the handler of the 59th tick Q2 processes one more tick (timer model, not delivery) | Pin one build (TO §1c). The Q1 "MIE = 0" bug fixed by Q2 is not exercised by `hello_world` |

## 7. Open questions, ranked by risk

1. **Peripheral state across a CPU reset.** The TRM gives the scope in one sentence ("only resets CPU core", plus PMS and `RCD_EN`). Which registers the `PERIP_RST_EN0/1` pulses reset (in particular bit 0, `SYSTEM_TIMERS_RST`, against the two timer groups and their calibration registers), and whether `INTERRUPT_CORE0` keeps its content, are not documented. Risk: a second boot that hangs or misbehaves in the emulator on state the real ROM or bootloader happens to tolerate, or the reverse. QEMU cannot arbitrate: it resets nearly everything.
2. **CPU clock after a reset.** `esp_restart_noos` leaves `PRE_DIV_CNT` = 0 (crystal ÷ 1, 40 MHz) and its comment says "same as hard reset", while the IDF header gives `PRE_DIV_CNT` = 1 at power-on (BA §10 item 1, decision D21 of SP1b-2: 20 MHz). After a software reset the answer is unambiguous (40 MHz); the power-on value is still open, and this comment is a hint towards 40 MHz.
3. **Reset latency and the `Saved PC` value.** How many instructions execute after the store to `OPTIONS0`, hence which pc the next boot prints. Affects one console line of every restart. A boot log of a real device after `esp_restart()` settles it.
4. **`wfi` wake condition on silicon** when `mstatus.MIE` = 0, or when the only asserted line is masked by the threshold or disabled. Not exercised by IDF's idle path; matters for light sleep and for other firmware.
5. **`mie` and `mip` on silicon**: trap, or readable constants. No known firmware depends on it.
6. **`MPIE` after `mret`**: the TRM says cleared, the RISC-V specification says set. Visible only in a read of `mstatus` outside a handler.
7. **`mpccr` across a CPU reset.** QEMU keeps counting (second-boot timestamps start at 22 s); on hardware early log timestamps after a restart are small in every log seen (ASSUMED), which fits a counter reset with the core.
8. **Edge latch details**: capture while the line is disabled or masked, and the exact effect of the toggle on an edge that arrives between the 1 and the 0. No IDF component of the measured path uses edge lines; IDF's own acknowledge helper looks wrong (§1.3).
9. **Transient state of the controller** (`TRM §1.5.3.1`: unpredictable ordering for up to 4 cycles after a register write). IDF always modifies the controller with `MIE` = 0 followed by `fence`; modelling it is not needed.
10. **The Xteink firmware** (Arduino core, CrossPoint) allocates more lines than `hello_world`: `esp_timer` (source 39), UART or USB-Serial-JTAG interrupts (21, 26), GPIO (16), SPI2 (19), possibly task-watchdog and brown-out variants. The mechanism is the same; the set of sources is not measured here.

## 8. Minimal model: what an emulator must implement

For the reference application to reach `Hello world!`, sleep, and restart.

**Controller (`INTERRUPT_CORE0`)**

1. Storage with read-back for the 62 map registers ([4:0]), `CPU_INT_ENABLE`, `CPU_INT_TYPE`, `CPU_INT_PRI_0..31` ([3:0]), `CPU_INT_THRESH` ([3:0]), `CLOCK_GATE` (reset 1). Already present in inkemu.
2. One input level per source 0..61, driven by the peripheral models. Needed now: source 50 (`SYSTEM.CPU_INTR_FROM_CPU_0` bit 0, plain read/write register whose bit *is* the level; same for 51..53) and source 37 (`SYSTIMER`: `INT_ST` bit 0 = `INT_RAW & INT_ENA`, cleared by `INT_CLR`).
3. `pending(n)` exactly as in §1.2, recomputed whenever a source level, a map register, `ENABLE`, `TYPE`, a priority or the threshold changes. `CPU_INT_EIP_STATUS` = the `pending` bits; `INTR_STATUS_0/1` = the raw source levels.
4. Selection: highest priority, then lowest line.
5. Edge lines: a latch per line set on a rising edge of `level(n)` when `TYPE[n]` = 1, cleared when bit n of `CPU_INT_CLEAR` goes from 1 to 0 (or, simpler and sufficient for IDF's broken helper, on any write of 1). Can be deferred: nothing measured uses it; if deferred, a non-zero write to `CPU_INT_TYPE` must be reported, not silently accepted.

**CPU**

6. Before each instruction: if `mstatus.MIE` and a line n is selected, take it: `mepc` = pc of the instruction not executed, `mcause` = `0x80000000 | n`, `MPIE` = `MIE`, `MIE` = 0, pc = `(mtvec & ~0xFF) + 4·n` (`mtvec & ~3` gives the same result for both values the firmware writes). **No dependence on `mie`**: inkemu's current `mip & mie` gate (`crates/cpu-rv32/src/hart.rs`) would never deliver, since nothing writes `mie`; its "highest-numbered line" choice must be replaced by the controller's selection.
7. Exceptions: pc = `mtvec` base, slot 0.
8. `mret`: `MIE` = `MPIE`, pc = `mepc`. Writes of `mstatus`, `mepc`, `mcause` by `csrw` must be accepted. `fence` is a no-op.
9. `wfi`: stop executing until some `pending(n)` is true, then continue at the next instruction; if `MIE` is 1 the interrupt is then taken with `mepc` = address after the `wfi`. Device time must keep advancing while waiting (the tick comes from the SYSTIMER); whether the cycle counter CSR advances is §6 row 13.
10. Delivery latency: next instruction boundary (0), which IDF tolerates and QEMU does.

**Peripherals the path touches around interrupts** (storage unless stated)

11. `ASSIST_DEBUG`: `CORE_0_MONTR_ENA`, `CORE_0_INTR_ENA`, `CORE_0_SP_MIN`, `CORE_0_SP_MAX` are rewritten at every interrupt and context switch; `C0RE_0_DEBUG_MODE` must read 0 (no debugger). No stack-guard interrupt needs to be raised for a healthy run.
12. `TIMG0`/`TIMG1` watchdog registers: unlocked, fed and locked at every tick; never expiring is enough (BA §6).
13. `SYSTIMER`: the tick alarm and its three interrupt registers (timer report).

**Reset**

14. A write to `RTC_CNTL.OPTIONS0` with bit 5 set: keep the other bits (bit 5 itself reads 0), set `RESET_STATE[5:0]` = `0x0C`, and reset the hart: pc = `0x40000000`, `mstatus` = 0, PMP cleared with its lock bits (the carry-over note of SP1b-2 already requires this), `waiting` cleared. With bit 31: reason `0x03`, and a core reset (every peripheral except `RTC_CNTL`).
15. Keep across a CPU reset: RAM, RTC memory, `RTC_CNTL` (all of it, `STORE0..7` included), flash, eFuse, and the peripheral registers (§4.2). Reset `SENSITIVE` and `ASSIST_DEBUG.CORE_0_RCD_EN`.
16. `SYSTEM.PERIP_RST_EN0/1`: while a bit is 1 the module is in reset; on the pulse, return the module's registers to their reset values. Needed for the restart path: SYSTIMER (bit 29), SPI0/1 (bit 1), UART (bit 2), `TIMERS` (bit 0), and SHA, AES, RSA, DS, HMAC, DMA in `PERIP_RST_EN1`. If the SYSTIMER is not reset, its periodic alarm stays asserted on source 37 through the second boot: harmless (the line is disabled, then the map is cleared), but a wrong state.
17. `ASSIST_DEBUG.CORE_0_RCD_PDEBUGPC` (`0x600CE048`): the pc at the last CPU reset while `RCD_EN` = 3, 0 after power-on. With it the ROM prints `Saved PC:`; reading 0 always reproduces QEMU instead. Decide which console the golden log follows.
18. The UART must report an empty TX FIFO and an idle transmitter some time after the last byte, or `esp_restart` never reaches the reset on hardware-like register values.
19. The RWDT is left configured (two 1 s stages, flash-boot mode) across the reset and the bootloader reprograms it; with watchdogs that never expire this is storage.
20. The device clock must not go backwards at a reset (carry-over note): the reset happens at a device time, and the second boot continues from it.

**Not needed for this application**: `mie`/`mip`, nesting beyond what §2.3 gives for free, edge lines, the status registers, lines other than 3 and 4, trigger CSRs, PMP enforcement (needed only to reproduce the store-fault panic), watchdog expiry.

## 9. Corrections to earlier reports

- BC §B.1 "Interrupt arbitration (several pending): Unknown": settled by `TRM §1.5.2`, highest priority then lowest line (§1.2). QEMU's lowest-line rule is a divergence.
- BC §B.3: the delivery condition also requires a non-zero priority (§1.2); `CPU_INT_CLEAR` is toggled (1 then 0), not written once; QEMU decodes the priority registers of lines 1 to 30 only (in the current `esp-develop` source), not 1 to 31.
- BC §B.1 "`mie`/`mip` […] recommended model: ignore `mie` for lines 1..31, treat `mie`/`mip` as plain registers": the first half stands; the second is an assumption among two (§2.4). QEMU does **not** ignore `mie`: writing 0 blocks delivery.
- BC §B.1 "WFI […] Whether C3 wakes with `mstatus.MIE=0` is unknown": still unknown for silicon; the two QEMU builds disagree (§2.6).
- BC §E.2 "enabling a 1 s RWDT with flashboot protection": the RWDT is configured with two 1 s stages and `FLASHBOOT_MOD_EN`, but `wdt_hal_init` has cleared its enable bit and nothing sets it again (§4.1). BC §K.4 (hardware flash-boot watchdog timeout): after an `esp_restart` the stage values are these.
- BA §1 and §9 give insn 2,369,140 for `xPortStartScheduler`: that is the threshold write inside it; the function is entered at 2,359,524.
- BA §10 item 9: the 20260417 build was measured here; it is identical to 20250817 for the first 2,472,463 instructions (§6 row 21).
- TO §1c pitfall 2 ("they first differ at the interrupt handler where the new build logs an extra re-entry"): with duplicate entries removed, the first real divergence is at insn 2,472,464, in the tick handler.
- TO §1d "restart path via RTC SW reset works": it works because QEMU resets the whole machine; on hardware only the CPU is reset and the second boot differs (§4.3, §4.4).

## Files (scratch, not in the repository)

`<scratch>/intr/tools/`: `filter2.py` (exec + MMIO stream → events and pc list, stop on a console marker), `filter3.py` (`-d cpu` stream → CSR changes), `run_trace.sh`, `run_cpu.sh`, `run_plain.sh`, `lib.py` (symbols, SVD names), `timeline.py` (console lines, interrupt list with vector slot), `window.py` (accesses in an instruction window), `intc_state.py` (controller writes, statistics, final state), `bootdiff.py` (first boot against second boot).
`<scratch>/intr/out/`: `F1_events.txt` (191,861 events), `F1_pcs.bin` (5,839,461 pcs), `F1_ints.txt` (1,120 interrupts), `F1_intc_app.txt`, `F1_intc_state.txt`, `F1_uart_lines.txt`, `C1_events.txt` (11,676 CSR changes among 135,587 events), `F2_*` (20260417), `P_abort_*`, `P_ill_*`, `P_store_*` (probe traces), `pa1`, `pa2`, `pw1`, `pw2`, `pi1`, `ps1` `_uart.log` (probe consoles).
`<scratch>/intr/probe/`: `src/main.c`, `src/in-container.sh`, `out/flash_probe_{abort,ill,store,wfi}.bin` (sha256 `064e9bfd…`, `62fe0cb7…`, `8da03a19…`, `95465ce2…`), their ELF and symbol files.
`<scratch>/intr/txt/trm_raw.txt`: text of the TRM used for the quotations. `<scratch>/intr/dl/esp-riscv-rt/`: the esp-hal files read.
