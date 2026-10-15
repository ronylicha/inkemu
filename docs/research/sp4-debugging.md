# Debugging, resource monitor and headless runs (SP4): what is realistic

Date 2026-10-15, branch of SP4 from `main` at `6691a7d` (SP2 done: the X4 wired, CrossPoint 1.6.5 at its home
screen in the desktop application). Tags: **[SRC]** read in a specification or in source code, **[DOC]** stated by
a vendor's documentation, **[INF]** reasoning, **[HYP]** chosen here. The design that rules on what follows is
`docs/superpowers/specs/2026-10-15-sp4a-debugger-design.md`; the split of SP4 is
`docs/superpowers/notes/2026-10-15-sp4-decoupage.md`.

## 0. What the machine already gives a debugger

- `Esp32C3::step` executes one instruction and goes to the boundary of the next (interrupt taken, `wfi` slept
  through): the state read afterwards is the one the next instruction starts from (SP1b-4c, decision H1) **[SRC]**
  `crates/soc-esp32c3/src/machine.rs`.
- `x()`, `set_x()`, `csr()` (reading has no effect), `pc()`, `is_asleep()`, `instructions()`, `cycles()`,
  `device_time_nanos()`, `trace_hash()` **[SRC]** same file.
- `run_within(cycles, device_time)` may be cut into calls anywhere: "the device-time limit is an instant, the same
  whatever the calls a run is cut into" **[SRC]**; the host engine already cuts every run into passes of
  `SLICE_CYCLES` **[SRC]** `crates/host/src/engine.rs`. A stop at an instruction boundary is therefore
  deterministic by construction **[INF]**.
- The decoder (`cpu_rv32::decode`) expands compressed parcels into one `Instr` enum **[SRC]**: a disassembler is a
  formatter over it, no second decoder **[INF]**.
- The host already pauses and steps (`Command::Pause`, `Command::Step`) and shows `pc` **[SRC]**
  `crates/host/src/device.rs`, `app/src/components/toolbar.tsx`.

## 1. GDB remote protocol (RSP)

- Packets a stub needs: `?`, `g`/`G`, `p`/`P`, `m`/`M`, `c`, `s`, `vCont`/`vCont?`, `Z0`–`Z4`/`z0`–`z4` (`Z0`
  software breakpoint, `Z1` hardware, `Z2`/`Z3`/`Z4` write/read/access watchpoints) **[SRC]**
  <https://sourceware.org/gdb/current/onlinedocs/gdb.html/Packets.html>; `qSupported`, `qXfer:features:read`
  **[SRC]** <https://sourceware.org/gdb/current/onlinedocs/gdb.html/General-Query-Packets.html>.
- Stop replies: `S AA` or `T AA n:r;…`; `swbreak`/`hwbreak` only when both sides announce them; `watch`, `rwatch`,
  `awatch` carry the address **[SRC]** <https://sourceware.org/gdb/current/onlinedocs/gdb.html/Stop-Reply-Packets.html>.
- No-ack mode (`QStartNoAckMode`) **[SRC]** <https://sourceware.org/gdb/current/onlinedocs/gdb.html/Packet-Acknowledgment.html>;
  Ctrl-C is a raw `0x03` byte outside a packet **[SRC]** <https://sourceware.org/gdb/current/onlinedocs/gdb.html/Interrupts.html>.
- RISC-V target description: only `org.gnu.gdb.riscv.cpu` (x0–x31, pc) is required; `csr` and `virtual` (`priv`)
  are optional **[SRC]** <https://sourceware.org/gdb/current/onlinedocs/gdb.html/RISC_002dV-Features.html>.
- Crate `gdbstub` 0.7.10 (MIT OR Apache-2.0, 2026-03-10), `no_std` without allocation, breakpoints and
  watchpoints, `GdbStubStateMachine` for an emulator loop; `gdbstub_arch` 0.3.3 has `riscv::Riscv32` with a core
  XML only (CSRs need an overridden XML) **[DOC]** <https://crates.io/crates/gdbstub>,
  <https://github.com/daniel5151/gdbstub>, <https://docs.rs/gdbstub_arch/latest/gdbstub_arch/riscv/index.html>.
  Its README says the API may move before 1.0 **[DOC]**. A codec of our own for the packets above is small; the
  choice belongs to the plan that ships the stub (SP4c) **[INF]**.
- VS Code: Cortex-Debug `servertype: external` + `gdbTarget`
  (<https://github.com/Marus/cortex-debug/blob/master/debug_attributes.md>), or `cppdbg` with
  `miDebuggerServerAddress` (<https://code.visualstudio.com/docs/cpp/launch-json-reference>); the ESP-IDF extension
  runs OpenOCD and serves GDB on port 3333 **[DOC]**
  <https://docs.espressif.com/projects/vscode-esp-idf-extension/en/latest/debugproject.html>.

## 2. Disassembly

- ABI register names: zero, ra, sp, gp, tp, t0–t2, s0/fp, s1, a0–a7, s2–s11, t3–t6 **[SRC]**
  <https://github.com/riscv-non-isa/riscv-elf-psabi-doc/blob/master/riscv-cc.adoc>.
- Pseudo-instructions (`li`, `mv`, `nop`, `ret`, `j`, `jr`, `beqz`, `bnez`, `bltz`, `bgez`, `bgtz`, `blez`, `not`,
  `neg`, `seqz`, `snez`, `csrr`, `csrw`, `csrs`, `csrc` and immediate forms) **[SRC]**
  <https://github.com/riscv-non-isa/riscv-asm-manual> (`src/asm-manual.adoc`, no release yet), and the
  "RISC-V Assembly Programmer's Handbook" chapter of the ISA manual 20191213
  (<https://github.com/riscv/riscv-isa-manual/releases/download/Ratified-IMAFDQC/riscv-spec-20191213.pdf>).
  `c.mv` expands to `add rd, x0, rs2`, written `mv` **[SRC]** same manual, C extension.
- Going backwards from a `pc` is ambiguous with 16-bit parcels: a start is chosen so that decoding forward lands
  exactly on `pc`, as debuggers do; the lines before `pc` are a best guess **[INF]**.

## 3. Debug hardware of the ESP32-C3 (the firmware's own debugging)

- 8 trigger units, `tselect` 0–7 (a larger index reads back the maximum), `tdata1` = `mcontrol` (type 2),
  `tdata2` = address, NAPOT ranges, `action` writable from debug mode only (so a breakpoint exception by default),
  `hit` cleared by software, `tcontrol` against re-triggering in a trap handler **[DOC]** TRM v1.4 §1.7
  "Hardware Trigger", p. 51, <https://documentation.espressif.com/esp32-c3_technical_reference_manual_en.pdf>; ESP-IDF
  advertises 8 hardware breakpoints and 8 watchpoints **[DOC]**
  <https://docs.espressif.com/projects/esp-idf/en/latest/esp32c3/api-guides/jtag-debugging/tips-and-quirks.html>.
  Debug specification 1.0 (2025-02-21) <https://github.com/riscv/riscv-debug-spec/releases/tag/1.0>; the TRM cites
  0.13.
- `ASSIST_DEBUG`: two data-bus and two peripheral-bus region monitors, stack-pointer bounds with recorded PC and
  interrupt, PC at the last reset, bus logging **[DOC]** TRM chapter 17, p. 446. PMS (`SENSITIVE`) **[DOC]** chapter 14,
  p. 315; ESP-IDF headers `sensitive_reg.h`, `assist_debug_reg.h` at v5.5.2.
- These are the **firmware's** features (ESP-IDF's stack guard and watchpoint API use them). They are not what an
  emulator's debugger needs: the emulator stops the machine from outside, without a trigger, which the firmware
  cannot see **[INF]**. They stay a separate plan (SP4f), with `rv32mi-p-breakpoint` of the conformance suite.

## 4. Resource monitor and FreeRTOS tasks

- OpenOCD lists FreeRTOS tasks from `pxCurrentTCB`, `pxReadyTasksLists`, `xDelayedTaskList1/2`,
  `pxDelayedTaskList`, `pxOverflowDelayedTaskList`, `xPendingReadyList`, `xTasksWaitingTermination`,
  `xSuspendedTaskList`, `uxCurrentNumberOfTasks`, `uxTopUsedPriority`, `xSchedulerRunning` **[SRC]**
  <https://github.com/openocd-org/openocd/blob/master/src/rtos/freertos.c>. Espressif's fork adds `pxCurrentTCBs`
  (ESP-IDF ≥ 5.0), `uxTaskNumber`, `port_interruptNesting`, `port_xSchedulerRunning` and `FreeRTOS_openocd_params`,
  a table of the kernel version and of the TCB and list offsets **[SRC]**
  <https://github.com/espressif/openocd-esp32/blob/master/src/rtos/freertos.c> lines 386–421; ESP-IDF v5.5.2
  defines it in `components/freertos/esp_additions/freertos_tasks_c_additions.h`:1129 and sets
  `configRECORD_STACK_HIGH_ADDRESS` to 1, so a task's stack margin reads from `pxStack` and `pxEndOfStack`.
- All of it needs symbols: an ELF given with the `.bin`. With `FreeRTOS_openocd_params` the offsets come from the
  firmware itself, not from a version table of ours: a task list is feasible "without depending on a given firmware"
  as long as the firmware is ESP-IDF ≥ 5.0 built with its default configuration and its ELF is given **[INF]**.
- The heap (`registered_heaps`, TLSF since ESP-IDF 5.x) depends on the TLSF version and on
  `CONFIG_HEAP_POISONING_*`, and regions can be added at run time **[SRC]**
  <https://docs.espressif.com/projects/esp-idf/en/latest/esp32c3/api-reference/system/mem_alloc.html>,
  `components/heap/heap_private.h`. Spec §6.1 already says the heap is shown "only if its ESP-IDF structures are
  recognized": the monitor shows it then and says it cannot otherwise **[SRC]** global spec.
- Measured without symbols: CPU frequency, load as the share of device time outside `wfi`, speed ratio (exists),
  the high-water mark of each RAM region (bytes ever written, counted by the bus as it writes) **[INF]**.

## 5. Headless scenarios

- Renode's Robot keywords: `Wait For Line On Uart`, `Wait For Prompt On Uart`, `Should Not Be On Uart`, timeouts,
  `Register Failing Uart String` **[DOC]**
  <https://github.com/renode/renode/blob/master/src/Renode/RobotFrameworkEngine/UartKeywords.cs>.
- Wokwi's scenario YAML: `name`, `version`, `steps` of `delay`, `set-control`, `wait-serial`, `write-serial`,
  `expect-pin`, `take-screenshot` (`save-to` or `compare-with`); CLI `--scenario`, `--expect-text`, `--timeout`,
  `--gdb-server-port` (starts paused) **[DOC]** <https://docs.wokwi.com/wokwi-ci/automation-scenarios>,
  <https://github.com/wokwi/wokwi-cli>.
- What inkemu has: the `inkemu-console-script 1` format (`console-script`) with waits on console text, sends,
  `press`/`release`, device-time waits; the exit code 5 when a script does not finish; the global spec asks YAML
  scenarios with buttons, gestures, conditional waits (stable screen, console text, virtual delay), captures and
  comparisons **[SRC]** spec §6.4. A capture is exact (logical state) and deterministic **[SRC]** spec §5.3.

## 6. Conclusions for the split

1. The emulator's own debugger (pause, step, registers, CSRs, memory, disassembly, breakpoints on a PC, watchpoints
   on an address) needs nothing the machine lacks but a stop at a boundary, a note of the accesses and a read
   without effect: it is the cheapest and the most useful to the application, and everything after builds on it
   (the GDB stub drives the same stops) **[INF]**.
2. Symbols and the GDB stub go together: both read an ELF, and the stub is the debugger driven by another client
   **[INF]**.
3. The monitor's measured values need a few counters in the machine; tasks and heap need the ELF of 2 **[INF]**.
4. Scenarios replace the console-script format, reuse the capture and the session, and are what the command line
   and the CI run headless **[INF]**.
5. The ESP32-C3's debug hardware is the firmware's: a plan of its own, with its conformance test **[INF]**.
