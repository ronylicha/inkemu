# ESP32-C3 timers and watchdogs: software-visible behaviour, expiry, and what the reference application does with them

Scope: every timer and watchdog of the ESP32-C3 that the rev3 mask ROM, the ESP-IDF v5.5.2 bootloader and an ESP-IDF v5.5.2 application rely on (SYSTIMER, TIMG0/TIMG1 with MWDT and the slow-clock calibration, the RTC timer, RWDT, super watchdog, brownout), what each watchdog does when it expires, and the register sequence measured on the reference `hello_world` image from the application entry to its software restart.
Input for plan SP1b-3. Date of research: 2026-10-11. The repository was not modified apart from this file.
Already established and not repeated here: `esp32c3-boot-chain.md` (§E, §F, §K), `esp32c3-bootloader-accesses.md` (§4, §5, §6, §9, §10), `esp32c3-rom-flash-boot.md` (§H, §L). §K lists what this report corrects in them.

## 0. Sources, method, tags

| Tag | Meaning |
|---|---|
| MEASURED | Observed in a QEMU run made for this report (trace M1, §L), or computed from it. MEASURED (silicon) = read in a serial log of a real chip published in a GitHub issue (§H), i.e. first-hand output of hardware, second-hand for us |
| READ | Read in a source, cited `file:line` or `function@address` |
| ASSUMED | Inference; every ASSUMED item that carries risk is repeated in §I |

| Source | What | Where |
|---|---|---|
| `IDF:` | ESP-IDF v5.5.2 in the `espressif/idf:v5.5.2` image, path relative to `/opt/esp/idf/components` | copies in scratch `timers/idf/` |
| `TRM:` | *ESP32-C3 Technical Reference Manual*, version 1.4 (2026-03-26), sha256 `90ef8256…7423`, `https://www.espressif.com/sites/default/files/documentation/esp32-c3_technical_reference_manual_en.pdf` | scratch `timers/dl/trm/` |
| `ROM3:` | disassembly of `esp32c3_rev3_rom.elf` (esp-rom-elfs `20260528`), `function@address` | scratch `boot/romelf/dis_rev3.txt` |
| `QEMU:` | Espressif QEMU at tag `esp-develop-9.2.2-20250817` (the build in the image), GPL, read for facts only, nothing copied: `hw/timer/esp_timg.c`, `hw/timer/esp_systimer.c`, `hw/timer/esp32c3_systimer.c`, `hw/misc/esp32c3_rtc_cntl.c`, `hw/riscv/esp32c3.c`, `include/hw/timer/esp_timg.h` | scratch `timers/dl/qemu-20250817/` |
| Trace M1 | `-d exec,nochain,int` + `-trace 'memory_region_ops_*'`, one instruction per TB, `-icount 3`, image `flash_hello.bin` (sha256 `fc05ef5d…c39bf2`), 7,500,000 instructions: two complete boots and the start of a third | scratch `timers/out/m1_*` |
| Issue logs | serial logs of real ESP32-C3 chips (ROM banner `esp32c3-api1-20210207`) from seven public GitHub issues | scratch `timers/dl/issues/`, list in §H |

Conventions: **insn** = 1-based instruction index in trace M1 (same indices as `esp32c3-bootloader-accesses.md`: bootloader entry 521,511, application entry 1,683,222). Register names are those of the vendored SVD. "Tick" alone means a FreeRTOS tick (10 ms here, `CONFIG_FREERTOS_HZ=100`); "count" means one increment of a hardware counter.

Reference configuration (READ `sdkconfig` of the reference build): `CONFIG_FREERTOS_HZ=100`, `CONFIG_ESP_DEFAULT_CPU_FREQ_MHZ=160`, `CONFIG_RTC_CLK_SRC_INT_RC`, `CONFIG_RTC_CLK_CAL_CYCLES=1024`, `CONFIG_BOOTLOADER_WDT_ENABLE` with 9000 ms, `CONFIG_ESP_INT_WDT` 300 ms, `CONFIG_ESP_TASK_WDT_INIT` 5 s, `CONFIG_ESP_TIMER_IMPL_SYSTIMER`, `CONFIG_FREERTOS_SYSTICK_USES_SYSTIMER`, `CONFIG_ESP_TIME_FUNCS_USE_ESP_TIMER` and `…_USE_RTC_TIMER`, `CONFIG_ESP_BROWNOUT_DET`.

## 1. Key results

1. **The flash-boot watchdog is hardware, not ROM code.** In the whole rev3 ROM image the only code that touches a TIMG register or the RWDT configuration is `disable_default_watchdog@0x40047c3a`, which clears the two `FLASHBOOT_MOD_EN` bits and has no caller in the ROM; the write-protect keys `0x50D83AA1` and `0x8F1D312A` do not occur in the image; no feed register is ever written (READ, §D.4). The reset values arm both watchdogs: MWDT0 resets the core after `26,000,000` cycles of APB_CLK, RWDT resets the system after `200,000 << 1` slow-clock cycles (READ `TRM:` §12.2.2.4, `IDF:` headers).
2. **Real chips confirm it** (MEASURED (silicon), §H): a blank flash gives `rst:0x7 (TG0WDT_SYS_RST)` + `Saved PC:0x4004d1f8` (= `ROM3:SPI_WakeUp+0x30`), a ROM hang after `ets_main.c 333` gives `rst:0x7` + `Saved PC:0x40047ed2` (= the `j .` in `ROM3:main`), and `rst:0x10 (RTCWDT_RTC_RST)` appears interleaved in the same loops: the RWDT keeps counting across MWDT0 core resets.
3. **The CPU and APB run at 20 MHz after a power-on reset** (READ `TRM:` §6.2.4.1: crystal divided by 2). MWDT0 therefore expires after 1.3 s; a timestamped log of a blank chip gives a lower bound of 0.95 s for the reset period, which excludes 0.65 s (40 MHz) (§H.2). After `esp_restart` the ROM runs at 40 MHz because the application leaves `PRE_DIV_CNT = 0`.
4. **SYSTIMER counts at 16 MHz from the 40 MHz crystal, whatever the CPU and APB frequencies are** (READ `TRM:` §10.3). Counter 1 + comparator 0 in period mode make the FreeRTOS tick; counter 0 is `esp_timer`'s clock; the reference application never programs an `esp_timer` alarm.
5. **The steady state is one interrupt per 10 ms** (MEASURED, 1,100 of them in 11 s): 10 accesses to SYSTIMER, 13 to TIMG1 (the interrupt watchdog is reconfigured and fed from every tick), 3 to TIMG0 (the task watchdog is fed by the idle task), 839 instructions per idle tick, the CPU in `wfi` the rest of the time.
6. **The slow-clock calibration is the only thing an application waits for on a timer block before the scheduler**: one-shot count of crystal cycles during 1,024 slow-clock cycles, 7.53 ms on silicon for the 136 kHz RC, result `301,176` (`RTCCALICFG1 = 0x024C3C00`), stored as the period `0x3AD2CD` (Q13.19 µs) in `RTC_CNTL.STORE1`.
7. **After the bootloader, exactly one watchdog runs**: RWDT, one stage, system reset after 9 s. The application shortens it to 1.6 s around the calibration, restores 9 s, disables it before the scheduler starts, then runs MWDT1 (interrupt watchdog: 300 ms interrupt, +600 ms core reset) and MWDT0 (task watchdog: 5 s interrupt, +10 s core reset).
8. **QEMU has no RWDT, no super watchdog, no RTC timer and no flash-boot protection**; its MWDT expires, but every expiry is a whole-machine reset; its SYSTIMER counters are not reset by anything and its calibration is instantaneous (§G).

## A. SYSTIMER (`0x6002_3000`)

### A.1 Clock

| Fact | Tag, source |
|---|---|
| Counters and comparators are clocked by CNT_CLK = XTAL_CLK ÷ 2.5 = **16 MHz** (alternately ÷3 and ÷2; one count = 62.5 ns = 1/16 µs). The register interface is clocked by APB_CLK | READ `TRM:` §10.2, §10.3 |
| The rate does not follow the CPU or APB frequency: ESP-IDF converts with constant factors (`ticks / 16`, `us * 16`) and never rewrites a step register on the C3 (`SOC_SYSTIMER_FIXED_DIVIDER 1`) | READ `IDF:esp_hw_support/port/esp32c3/systimer.c:14-22`, `IDF:soc/esp32c3/include/soc/soc_caps.h:359`, `IDF:hal/systimer_hal.c:170` |
| Only a 40 MHz crystal exists for this chip in ESP-IDF (`assert(rtc_clk_xtal_freq_get() == SOC_XTAL_FREQ_40M)`) | READ `IDF:esp_system/port/soc/esp32c3/clk.c:77` |
| Gate and reset: `SYSTEM.PERIP_CLK_EN0` bit 29 (reset 1) gates the APB side; `SYSTEM.PERIP_RST_EN0` bit 29 resets the block, "its registers will be restored to their default values" | READ `TRM:` §10.3, `IDF:soc/esp32c3/register/soc/system_reg.h:89, 333` |
| `CONF.CLK_EN` (bit 31) only gates the register file clock. `UNIT0` is enabled out of reset (`UNIT0_WORK_EN` = 1), so counter 0 counts from reset | READ `IDF:soc/esp32c3/register/soc/systimer_reg.h:81-93` |

### A.2 Registers and reset values (READ `IDF:soc/esp32c3/register/soc/systimer_reg.h`)

| Offset | Register | Reset | Fields |
|---|---|---|---|
| 0x00 | `CONF` | `0x46000000` | b31 `CLK_EN`, b30 `UNIT0_WORK_EN` (1), b29 `UNIT1_WORK_EN`, b28 `UNIT0_CORE0_STALL_EN`, b27 `UNIT0_CORE1_STALL_EN`, b26 `UNIT1_CORE0_STALL_EN` (1), b25 `UNIT1_CORE1_STALL_EN` (1), b24/23/22 `TARGET0/1/2_WORK_EN`, b0 `SYSTIMER_CLK_FO` |
| 0x04, 0x08 | `UNIT0_OP`, `UNIT1_OP` | 0 | b30 `UPDATE` (write-1 trigger), b29 `VALUE_VALID` (set by hardware) |
| 0x0C/0x10, 0x14/0x18 | `UNITn_LOAD_HI` (20 bits) / `_LO` | 0 | value loaded by `UNITn_LOAD` |
| 0x1C…0x30 | `TARGETx_HI` (20 bits) / `_LO`, x = 0..2 | 0 | alarm value *t* |
| 0x34, 0x38, 0x3C | `TARGETx_CONF` | 0 | b31 `TIMER_UNIT_SEL`, b30 `PERIOD_MODE`, b25:0 `PERIOD` (δt) |
| 0x40/0x44, 0x48/0x4C | `UNITn_VALUE_HI` / `_LO` | 0 | read-only latch |
| 0x50, 0x54, 0x58 | `COMPx_LOAD` | — | b0, write-1 trigger: synchronises `TARGETx_HI/LO` and `PERIOD` into the comparator |
| 0x5C, 0x60 | `UNITn_LOAD` | — | b0, write-1 trigger: counter ← `UNITn_LOAD_HI/LO` |
| 0x64, 0x68, 0x6C, 0x70 | `INT_ENA`, `INT_RAW`, `INT_CLR`, `INT_ST` | 0 | bits 2:0 = `TARGET2/1/0`; `INT_CLR` write-1; `INT_ST` = `INT_RAW & INT_ENA` |
| 0xFC | `DATE` | `0x02006171` | |

Two 52-bit counters, three 52-bit comparators (`SOC_SYSTIMER_COUNTER_NUM 2`, `ALARM_NUM 3`, READ `soc_caps.h:355-358`). The C3 has no `REAL_TARGETx` registers (0x74…0x88 read 0; READ `QEMU:esp32c3_systimer.c`, absent from the header).

### A.3 Protocols (READ `TRM:` §10.4, §10.5)

| Operation | Protocol | Notes |
|---|---|---|
| Read a counter | write `UNITn_OP.UPDATE` = 1; poll `UNITn_OP.VALUE_VALID`; read `UNITn_VALUE_LO`, `_HI`. The latch keeps its value until the next update | `systimer_hal_get_counter_value` writes the update bit (read-modify-write: reads `0x20000000`, writes `0x60000000` in M1), **spins while `VALUE_VALID` is 0, with no timeout**, then reads LO, HI, LO and loops until the two LO reads agree (`IDF:hal/systimer_hal.c:46-70`). The latency of `VALUE_VALID` on silicon is not documented (a few CNT_CLK cycles, ASSUMED) |
| Load a counter | write `UNITn_LOAD_HI/LO`, then `UNITn_LOAD` = 1: "the current count value will be changed immediately"; a working counter continues from the new value | |
| Stop/start a counter | `CONF.UNITn_WORK_EN`. A stopped counter keeps its value | |
| Stall | with `UNITn_CORE0_STALL_EN` = 1 the counter stops while the CPU is **stalled** (debugger halt, `RTC_CNTL.OPTIONS0.SW_STALL_PROCPU`) and resumes afterwards. `wfi` is not a stall: the tick wakes the CPU from it (MEASURED: every tick interrupt is taken in `esp_cpu_wait_for_intr`) | READ `TRM:` Table 10.4-1; ASSUMED for the exact list of stall causes |
| Target (one-shot) alarm | write `TARGETx_HI/LO`, `TARGETx_CONF` (`PERIOD_MODE` = 0, unit), `COMPx_LOAD` = 1, set `CONF.TARGETx_WORK_EN`. The alarm fires when count = *t*; **if *t* is already passed (0 ≤ count − *t* < 2^51 − 1) it fires immediately**; if count − *t* ≥ 2^51 − 1 it fires after the counter wraps | READ `TRM:` Table 10.4-2; `SOC_SYSTIMER_ALARM_MISS_COMPENSATE 1` |
| Period alarm | `PERIOD_MODE` = 1, `PERIOD` = δt, `COMPx_LOAD` = 1, `TARGETx_WORK_EN`. With *t1* the count at the start, alarms at *t1* + n·δt, n = 1, 2, 3… No software re-arm | READ `TRM:` §10.4.2 |
| Interrupt | each comparator drives a **level** interrupt: `INT_RAW.TARGETx` is set by the alarm and stays set until `INT_CLR.TARGETx` is written 1; the line is `INT_RAW & INT_ENA` | READ `TRM:` §10.4.4, `soc_caps.h:360` |

Interrupt sources: `SYSTIMER_TARGET0/1/2` = 37/38/39 (`esp32c3-boot-chain.md` §F.1).

### A.4 What ESP-IDF programs

| User | Counter, comparator | Sequence (READ source; MEASURED insn in M1) |
|---|---|---|
| `esp_timer` (`esp_timer_impl_early_init`, init function at priority 101) | counter 0, comparator 2, one-shot, source 39 | `SYSTEM.PERIP_CLK_EN0` \|= b29; **`PERIP_RST_EN0` b29 pulsed: the block is reset, counter 0 restarts from 0** (ASSUMED for the counter value, §I.6); `CONF.CLK_EN` ← 1 (W `0xC6000000`); `UNIT0_WORK_EN` ← 1; `TARGET2_CONF`: one-shot, unit 0; `UNIT0_CORE0_STALL_EN` ← 1 (W `0xD6000000`). `IDF:esp_timer/src/esp_timer_impl_systimer.c:150-182`; insn 2,292,318–2,292,479 |
| `esp_timer_get_time()` | counter 0 | `systimer_hal_get_counter_value(0) / 16` (`esp_timer_impl_systimer.c:68-75`) |
| `esp_timer` alarm | comparator 2 | `systimer_hal_set_alarm_target`: `TARGET2_WORK_EN` ← 0, `TARGET2_HI/LO`, `COMP2_LOAD`, `TARGET2_WORK_EN` ← 1 (`IDF:hal/systimer_hal.c:78-87`); ISR writes `INT_CLR` bit 2. **Not exercised by the reference application**: `esp_timer_impl_init` is never called, `INT_ENA` bit 2 stays 0, no access to `TARGET2_*`/`COMP2_LOAD` after the early init (MEASURED) |
| FreeRTOS tick (`vSystimerSetup`, from `vPortSetupTimer` just before the scheduler starts) | counter 1, comparator 0, period, source 37 → CPU line 4, priority 1 | `UNIT1_LOAD_HI/LO` ← 0, `UNIT1_LOAD`; `UNIT1_CORE0_STALL_EN` ← 0; `TARGET0_CONF`: one-shot, unit 1 (W `0x80000000`); `TARGET0_WORK_EN` ← 0; `PERIOD` ← 160,000 (W `0x80027100`); `COMP0_LOAD`; `TARGET0_WORK_EN` ← 1 (W `0xD3000000`); `PERIOD_MODE` ← 1 (W `0xC0027100`); `UNIT1_CORE0_STALL_EN` ← 1; `INT_ENA` ← 1; `UNIT1_WORK_EN` ← 1 (W `0xF7000000`). `IDF:freertos/port_systick.c:60-118`; insn 2,368,922–2,369,122 |
| Tick ISR (`SysTickIsrHandler`) | counter 1 | `INT_CLR` bit 0 (read-modify-write); `ticks = counter1 / PERIOD − handled`, with `PERIOD` read back from `TARGET0_CONF`; calls `xPortSysTickHandler` `ticks` times; loops while `INT_ST` bit 0 is set (`port_systick.c:126-156`). **The counter value is used, not only the interrupt**: a counter that does not advance by 160,000 per interrupt gives zero or several ticks |

Remark on the tick set-up order (READ + ASSUMED): comparator 0 is enabled in target mode with target 0 while counter 1 holds 0 and is stopped, 22 instructions before `PERIOD_MODE` is set. By `TRM:` Table 10.4-2 (count = target) the alarm condition holds at that moment, so `INT_RAW` bit 0 is probably already set on silicon when the scheduler starts and one early tick interrupt is taken with `ticks = 0`. Harmless for the firmware, visible in a trace. QEMU does not do it (§G).

## B. Timer groups `TIMG0` (`0x6001_F000`) and `TIMG1` (`0x6002_0000`)

Each group has one general-purpose timer T0, one main watchdog (MWDT) and the calibration registers (used in TIMG0 only). Clock gate and reset: `SYSTEM.PERIP_CLK_EN0` bits 13 (TIMG0) and 15 (TIMG1), both 1 at reset; `PERIP_RST_EN0` bits 13 and 15 (READ `system_reg.h:173-187`).

### B.1 Registers and reset values (READ `IDF:soc/esp32c3/register/soc/timer_group_reg.h`)

| Offset | Register | Reset | Fields |
|---|---|---|---|
| 0x00 | `T0CONFIG` | `0x60002000` | b31 `EN`, b30 `INCREASE` (1), b29 `AUTORELOAD` (1), b28:13 `DIVIDER` (1), b12 `DIVCNT_RST` (WT), b10 `ALARM_EN` (cleared by hardware at alarm), b9 `USE_XTAL` |
| 0x04, 0x08 | `T0LO`, `T0HI` (22 bits) | 0 | read-only latch |
| 0x0C | `T0UPDATE` | 0 | b31: written to latch; **cleared by hardware when the latch is done** |
| 0x10, 0x14 | `T0ALARMLO`, `T0ALARMHI` | 0 | |
| 0x18, 0x1C, 0x20 | `T0LOADLO`, `T0LOADHI`, `T0LOAD` | 0 | any write to `T0LOAD` reloads |
| 0x48 | `WDTCONFIG0` | **`0x0004C000`** | b31 `EN`, b30:29 `STG0`, b28:27 `STG1`, b26:25 `STG2`, b24:23 `STG3` (0 off, 1 interrupt, 2 reset CPU, 3 reset system), b22 `CONF_UPDATE_EN` (WT), b21 `USE_XTAL`, b20:18 `CPU_RESET_LENGTH` (1), b17:15 `SYS_RESET_LENGTH` (1), **b14 `FLASHBOOT_MOD_EN` (1)**, b13 `PROCPU_RESET_EN`, b12 `APPCPU_RESET_EN` |
| 0x4C | `WDTCONFIG1` | `0x00010000` | b31:16 `CLK_PRESCALE` (1), b0 `DIVCNT_RST` (WT) |
| 0x50…0x5C | `WDTCONFIG2…5` | 26,000,000; 134,217,727; 1,048,575; 1,048,575 | stage 0…3 timeout, in MWDT clock cycles |
| 0x60 | `WDTFEED` | — | any write feeds |
| 0x64 | `WDTWPROTECT` | `0x50D83AA1` | key |
| 0x68 | `RTCCALICFG` | `0x00013000` | b31 `START`, b30:16 `MAX` (1), b15 `RDY` (RO), b14:13 `CLK_SEL` (1), b12 `START_CYCLING` (1) |
| 0x6C | `RTCCALICFG1` | 0 | b31:7 `VALUE` (RO), b0 `CYCLING_DATA_VLD` (RO) |
| 0x70, 0x74, 0x78, 0x7C | `INT_ENA_TIMERS`, `INT_RAW_TIMERS`, `INT_ST_TIMERS`, `INT_CLR_TIMERS` | 0 | b0 T0, b1 WDT; level; `INT_CLR` write-1 |
| 0x80 | `RTCCALICFG2` | **`0xFFFFFF98`** | b31:7 `TIMEOUT_THRES` (`0x1FFFFFF`), b6:3 `TIMEOUT_RST_CNT` (3), b0 `TIMEOUT` (RO) |
| 0xF8 | `NTIMERS_DATE` | `0x02006191` | |
| 0xFC | `REGCLK` | `0x60000000` | b31 `CLK_EN`, b30 `TIMER_CLK_IS_ACTIVE` (1), b29 `WDT_CLK_IS_ACTIVE` (1) |

Interrupt sources: `TG_T0` 32, `TG_WDT` 33, `TG1_T0` 34, `TG1_WDT` 35.

### B.2 General-purpose timer T0 (READ `TRM:` §11.2; `IDF:hal/esp32c3/include/hal/timer_ll.h`)

| Item | Fact |
|---|---|
| Clock | APB_CLK (`USE_XTAL` = 0) or XTAL_CLK (40 MHz), gated by `REGCLK.TIMER_CLK_IS_ACTIVE`, divided by a 16-bit prescaler: `DIVIDER` 2…65535 divides by that value, **0 divides by 65536, 1 divides by 2**. APB_CLK is 80 MHz when the CPU runs from the PLL and equals CPU_CLK otherwise (`TRM:` Table 6.2-4) |
| Counter | 54 bits, up (`INCREASE` = 1) or down, frozen when `EN` = 0; the direction can change while it runs |
| Read | write any value to `T0UPDATE`, wait until the register reads 0 (`timer_ll_trigger_soft_capture` spins on it, `timer_ll.h:186-194`), read `T0LO`/`T0HI`. The latch holds until the next update |
| Load | write `T0LOADLO/HI`, then any value to `T0LOAD` |
| Alarm | compare with `T0ALARMLO/HI` once `ALARM_EN` is set; an alarm value already passed (within 2^53) fires immediately; at the alarm `INT_RAW` bit 0 is set, **`ALARM_EN` is cleared by hardware**, and with `AUTORELOAD` the counter is reloaded from `T0LOADLO/HI` |
| Use by the reference application | **none**: no access to `T0CONFIG`…`T0LOAD` of either group in M1 (MEASURED). ESP-IDF uses T0 only through the `gptimer` driver |

### B.3 Main watchdog MWDT (READ `TRM:` §12.2; `IDF:hal/esp32c3/include/hal/mwdt_ll.h`, `IDF:hal/wdt_hal_iram.c`)

| Item | Fact |
|---|---|
| Clock | APB_CLK (`USE_XTAL` = 0, the reset value) or XTAL_CLK, gated by `REGCLK.WDT_CLK_IS_ACTIVE`, divided by `CLK_PRESCALE` (16 bits). ESP-IDF selects **XTAL** (`MWDT_CLK_SRC_DEFAULT`, `IDF:soc/esp32c3/include/soc/clk_tree_defs.h:356`) with prescaler 20,000 → 2 kHz, one count = 500 µs (`mwdt_ll.h:27`, `wdt_hal_iram.c:80-83`) |
| Counter and stages | one 32-bit counter. When it reaches the timeout of the current stage, the stage action is executed, the counter returns to 0 and the next stage becomes current; stages loop 0 → 3 → 0 (`TRM:` §12.2.2.1-2). A stage whose action is "off" has no effect when it expires (whether its timeout is still waited for is not stated; QEMU waits for it, §G) |
| Stage actions | 0 off, 1 interrupt (`INT_RAW` bit 1, source 33/35), 2 CPU reset, 3 "system reset" in ESP-IDF's naming = **core reset** in the TRM's naming (`mwdt_ll.h:29-33`; §D.1) |
| Feed | any write to `WDTFEED`: counter ← 0, stage ← 0 |
| Write protection | every MWDT register other than `WDTWPROTECT` ignores writes unless `WDTWPROTECT` holds `0x50D83AA1`; this includes the feed (`TRM:` §12.2.2.3: "feeding or changing its configuration"). The key register resets to the key: **the watchdog is unlocked out of reset**. ESP-IDF writes the key, operates, then writes 0 |
| Configuration update | `WDTCONFIG0`…`5` are synchronised into the watchdog clock domain when `CONF_UPDATE_EN` (b22, write-1, reads 0) is written; every `mwdt_ll` setter does it (`mwdt_ll.h:102-214`) |
| Enable | `WDTCONFIG0.EN`; `wdt_hal_enable` feeds first, then sets `EN` (`wdt_hal_iram.c:146-155`) |
| Flash-boot protection | while `FLASHBOOT_MOD_EN` is 1 and the chip boots from flash, MWDT0 runs with stage 0 = core reset, whatever `EN` and `STG0` contain; clearing the bit stops it (`TRM:` §12.2.2.4; §D.4). The peripheral reset of a timer group brings the bit back to 1, which is why `_timer_ll_reset_register` clears it right after the reset pulse, for both groups (`timer_ll.h:55-66`) |

What the firmware does with the two MWDTs (READ sources; MEASURED values in M1):

| Who | Watchdog | Configuration | Fed by |
|---|---|---|---|
| Bootloader `bootloader_config_wdt` | MWDT0 | unlock, `FLASHBOOT_MOD_EN` ← 0 (+ `CONF_UPDATE_EN`), lock: stopped (`IDF:bootloader_support/src/bootloader_init.c:95-99`; insn 573,804) | — |
| `enable_timer_group0_for_calibration` (a constructor: runs with the global constructors, after the calibration it is named for) | TIMG0 | clock gate on, peripheral reset pulse, `FLASHBOOT_MOD_EN` ← 0 (`IDF:esp_hw_support/port/esp32c3/rtc_time.c:199-214`; insn 2,319,492) | — |
| `esp_int_wdt_init` | MWDT1 = interrupt watchdog | TIMG1 gate on + reset + flash-boot bit cleared; `wdt_hal_init` (XTAL, prescaler 20,000: `WDTCONFIG1` = `0x4E200000`; reset lengths 7; interrupt enabled); stage 0 = interrupt after 10,000 counts (5 s), stage 1 = core reset after 10,000 more; enabled: `WDTCONFIG0` = `0xB83F8000`. Source 35 → CPU line 24, priority 4 (`IDF:esp_system/int_wdt.c:143-161`; insn 2,342,906–2,343,385) | — |
| Tick hook of the interrupt watchdog | MWDT1 | on **every tick**: unlock, stage 0 ← 600 counts (300 ms), stage 1 ← 1,200 counts (600 ms more), feed, lock (`int_wdt.c:129-139`) | the tick ISR |
| `esp_task_wdt_init` | MWDT0 = task watchdog | `wdt_hal_init` as above; stage 0 = interrupt after 10,000 counts (5 s), stage 1 = core reset after 20,000 more (10 s); `WDTCONFIG0` = `0xB83F8000` once enabled. Source 33 → CPU line 5, priority 1 (`IDF:esp_system/task_wdt/task_wdt_impl_timergroup.c:97-138`; insn 2,388,614–2,390,114) | the idle task, once per loop: unlock, `WDTFEED`, lock (MEASURED 1,099 feeds for 1,100 ticks) |
| `esp_restart_noos` | both | unlock, `EN` ← 0, lock (`IDF:esp_system/port/soc/esp32c3/system_internal.c:82-91`) | — |

### B.4 Slow-clock calibration (`RTCCALICFG`, `RTCCALICFG1`, `RTCCALICFG2` of TIMG0)

Principle (READ `TRM:` §11.2.5, `rtc_time.c:18-28`): a counter of XTAL_CLK cycles and a counter of the selected slow clock start together; when the slow counter reaches `MAX` both stop; `VALUE` is the crystal count. One-shot mode is started by `START` and reports `RDY`; cycling mode (`START_CYCLING`, **on at reset**) repeats continuously.

`CLK_SEL`: 0 = RC_SLOW (the "150 k" RC, 136 kHz nominal), 1 = RC_FAST ÷ 256, 2 = XTAL32K (`timer_group_reg.h:369`; `rtc_cal_sel_t`, `IDF:esp_hw_support/port/esp32c3/include/soc/rtc.h:164-167`). `RTC_CAL_RTC_MUX` (0) is redirected by software to the source currently selected as slow clock (`rtc_time.c:43-52`).

Protocol of `rtc_clk_cal_internal(cal_clk, cycles)` (READ `rtc_time.c:36-128`; MEASURED column = M1, first boot):

| Step | Register operations | MEASURED (QEMU) |
|---|---|---|
| 0 | enable the measured clock if it is the 32 kHz crystal or RC_FAST ÷ 256 (`RTC_CNTL.CLK_CONF`) | not taken (RC) |
| 1 | if `RTCCALICFG.START_CYCLING` is set: `RTCCALICFG2.TIMEOUT_THRES` ← 1, then **spin until `RTCCALICFG.RDY` or `RTCCALICFG2.TIMEOUT`** (no software timeout). On silicon the running cycling measurement overruns the threshold of 1 at once (ASSUMED from the source comment "accelerate the generation of timeout"; the written value is `0x00000098`) | R `0x13000`; W `RTCCALICFG2 = 0x80`; R `RTCCALICFG2 = 0x81` → exit |
| 2 | `CLK_SEL` ← source; `START_CYCLING` ← 0; `MAX` ← cycles | W `0x11000`, W `0x18000`, W `0x04000000` |
| 3 | `TIMEOUT_THRES` ← cycles << 10 for the RC, cycles << 12 for the two others (`soc/rtc.h:54-56`) | W `RTCCALICFG2 = 0x08000000` |
| 4 | `START` ← 0, then `START` ← 1 | W `0x04000000`, W `0x84000000` |
| 5 | `esp_rom_delay_us(cycles × 1,000,000 / nominal frequency)` | 7,529 µs = 470,567 instructions |
| 6 | spin: if `RDY` → result = `RTCCALICFG1.VALUE`; else if `TIMEOUT` → result = 0 | R `RTCCALICFG = 0x84008000`, R `RTCCALICFG1 = 0x024C3C00` |
| 7 | `START` ← 0; restore the clock enables | W `0x04008000` |

Values for a 40 MHz crystal and `cycles` = 1,024 (computed from the nominal frequencies of `IDF:soc/esp32c3/include/soc/clk_tree_defs.h:39-42`; ASSUMED for silicon, whose RC oscillators deviate from nominal, §I.2):

| Slow clock | Nominal frequency | Duration of the measurement | `VALUE` (crystal cycles) | `RTCCALICFG1` | `rtc_clk_cal` result (period, Q13.19 µs) | Timeout threshold |
|---|---|---|---|---|---|---|
| RC_SLOW | 136,000 Hz | 7.529 ms | 301,176 | `0x024C3C00` | 3,855,053 = `0x3AD2CD` (7.3529 µs) | 1,048,576 |
| XTAL32K | 32,768 Hz | 31.250 ms | 1,250,000 | `0x09896800` | 16,000,000 = `0xF42400` (30.5176 µs) | 4,194,304 |
| RC_FAST ÷ 256 | 68,359.4 Hz (17.5 MHz ÷ 256) | 14.980 ms | 599,186 | `0x04924900` | 7,669,581 = `0x75074D` (14.6286 µs) | 4,194,304 |

What is computed from it:

| Function | Computation | Source |
|---|---|---|
| `rtc_clk_cal(clk, cycles)` | `period = ((xtal_cycles << 19) + divider/2 − 1) / divider` with `divider = xtal_MHz × cycles` = 40,960: the slow-clock period in µs, Q13.19. For the 32 kHz crystal the count must be within ±0.05 % of `xtal_Hz × cycles >> 15` (1,250,000 ± 625), otherwise the result is 0 | `rtc_time.c:139-160` |
| `rtc_clk_cal_ratio` | `(xtal_cycles << 19) / cycles` | `rtc_time.c:130-137` |
| `select_rtc_slow_clk` | selects the configured source, then `do { cal = rtc_clk_cal(RTC_CAL_RTC_MUX, 1024); } while (cal == 0);` — **a result of 0 loops forever**; for the 32 kHz crystal three failed attempts fall back to the RC with the warning `32 kHz XTAL not found, switching to internal 150 kHz oscillator` | `IDF:esp_system/port/soc/esp32c3/clk.c:140-199` |
| `esp_clk_slowclk_cal_set(cal)` | first calls `esp_rtc_get_time_us()` (which reads the RTC timer and, on the first power-up, zeroes its retained structure because the stored calibration is 0), then stores `cal` in `RTC_CNTL.STORE1` | `IDF:esp_hw_support/esp_clk.c:101-192`; MEASURED W `STORE1 = 0x3AD2CD` at insn 2,185,201 |
| `rtc_time_slowclk_to_us(n, period)` / `rtc_time_us_to_slowclk` | `(n × period) >> 19` / `(us << 19) / period` (asserts `period != 0`) | `rtc_time.c:162-174` |
| `calibrate_ocode` (only when the eFuse block version is 0) | `rtc_clk_cal(clk, 100)`, then waits on regi2c flags with the RTC timer as its 10 ms time-out | `IDF:esp_hw_support/port/esp32c3/rtc_init.c:201-225` |

After the calibration the RWDT is fed and its stage 0 put back to 9 s (`clk.c:109-116`).

## C. `RTC_CNTL` (`0x6000_8000`): RTC timer, RWDT, super watchdog, brownout

### C.1 Slow clock

| Fact | Tag, source |
|---|---|
| RTC_SLOW_CLK is RC_SLOW (0), XTAL32K (1) or RC_FAST ÷ 256 (2), selected by `CLK_CONF.ANA_CLK_RTC_SEL` [31:30], reset 0 | READ `IDF:soc/esp32c3/register/soc/rtc_cntl_reg.h:971`, `TRM:` §6.2.4.4 |
| ESP-IDF's nominal frequencies: 136,000, 32,768 and 68,359 Hz (`rtc_clk_slow_freq_get_hz`); every watchdog timeout is computed from the **nominal** value, not from the calibration | READ `IDF:esp_hw_support/port/esp32c3/rtc_clk.c:137-145` |
| 136 kHz is the RC frequency once the bootloader has written `SCK_DCAP` = 255 (`RTC_CNTL_SCK_DCAP_DEFAULT`); the field resets to 0, so the RC runs at another, undocumented frequency between reset and the bootloader's clock set-up | READ `soc/rtc.h:97`, `rtc_cntl_reg.h:1305`; frequency ASSUMED higher (§I.2) |

### C.2 RTC timer

| Item | Fact | Tag, source |
|---|---|---|
| Counter | 48 bits, counts RTC_SLOW_CLK cycles | READ `TRM:` §9.3.3 |
| Read | write `TIME_UPDATE` (0x0C) bit 31 (write-only), read `TIME_LOW0` (0x10) and `TIME_HIGH0` (0x14, 16 bits). No "valid" bit is polled on the C3. Each trigger moves the previous capture into `TIME_LOW1/HIGH1` (0xDC/0xE0) | READ `IDF:hal/esp32c3/include/hal/rtc_cntl_ll.h:74-80`, `TRM:` §9.3.3 |
| Other capture triggers | `TIME_UPDATE` bits 29 (`TIMER_SYS_RST`: end of a digital reset), 28 (`TIMER_XTL_OFF`), 27 (`TIMER_SYS_STALL`), all 0 at reset | READ `rtc_cntl_reg.h:180-200`, `TRM:` Table 9.3-2 |
| Reset | "any reset / sleep other than power-up reset will not stop or reset the RTC timer" | READ `TRM:` §9.3.3 |
| Alarm | `SLP_TIMER0` (0x04), `SLP_TIMER1` (0x08: b15:0 high part, b16 `MAIN_TIMER_ALARM_EN`), interrupt `INT_RAW_RTC` bit 10: wake-up from sleep only; not used by the reference application | READ `rtc_cntl_reg.h` |
| Users | `rtc_time_get()` ← `esp_rtc_get_time_us()` = accumulated `(Δticks × cal) >> 19` kept in a structure in RTC memory; called by `esp_clk_slowclk_cal_set`, for `g_startup_time` (`IDF:esp_system/port/cpu_start.c:761`), by `esp_timer_impl_init_system_time`, `esp_set_time_from_rtc`, and at shutdown by `esp_sync_timekeeping_timers` | READ `esp_clk.c:101-153`; MEASURED five reads in the first boot (insn 2,185,053; 2,185,952; 2,292,543; 2,303,922; 3,400,614) |

Nothing in the reference path waits for the RTC timer to advance. With eFuse block version 0, `calibrate_ocode` uses it as a time-out (§B.4).

### C.3 RTC watchdog RWDT (READ `rtc_cntl_reg.h:1565-1705`, `IDF:hal/esp32c3/include/hal/rwdt_ll.h`, `TRM:` §12.2)

| Register | Reset | Fields |
|---|---|---|
| `WDTCONFIG0` 0x90 | **`0x00013214`** | b31 `EN`, b30:28 `STG0`, b27:25 `STG1`, b24:22 `STG2`, b21:19 `STG3` (0 off, 1 interrupt, 2 reset CPU, 3 "reset system" = core reset, 4 "reset RTC" = system reset), b18:16 `CPU_RESET_LENGTH` (1), b15:13 `SYS_RESET_LENGTH` (1), **b12 `FLASHBOOT_MOD_EN` (1)**, b11 `PROCPU_RESET_EN`, b10 `APPCPU_RESET_EN`, b9 `PAUSE_IN_SLP` (1), b8 `CHIP_RESET_EN`, b7:0 `CHIP_RESET_WIDTH` (20) |
| `WDTCONFIG1…4` 0x94…0xA0 | 200,000; 80,000; `0xFFF`; `0xFFF` | stage 0…3 timeout in slow-clock cycles |
| `WDTFEED` 0xA4 | — | b31 write-1 |
| `WDTWPROTECT` 0xA8 | `0x50D83AA1` | key, same rule as the MWDT |

| Item | Fact |
|---|---|
| Clock | RTC_SLOW_CLK directly, no prescaler (`TRM:` §12.2.2.1) |
| Stage 0 timeout | **`WDTCONFIG1 << (EFUSE_WDT_DELAY_SEL + 1)`** slow-clock cycles; the eFuse field (`RD_REPEAT_DATA1` [17:16]) is 0 on a blank chip, so the register holds half the wanted count. `rwdt_ll_config_stage` writes `timeout >> (1 + delay_sel)` for stage 0 only (`TRM:` §12.2.2.2, `rwdt_ll.h:95-136`) |
| Interrupt | `INT_RAW_RTC` bit 3, through the RTC core interrupt (source `RTC_CORE`, routed to CPU line 2 by the reference application for the brownout) |
| Flash-boot protection | as for MWDT0, with stage 0 = system reset (`TRM:` §12.2.2.4) |
| Reset domain | RTC: not reset by a CPU or core reset (§D.2) |

Life of the RWDT in the reference boot (READ sources, MEASURED writes; QEMU stores nothing, so every read-back in M1 is 0):

| Moment | Operation | State |
|---|---|---|
| Reset | hardware | flash-boot protection: system reset after 400,000 slow-clock cycles (2.9 s at 136 kHz) |
| Bootloader, insn 573,561–573,790 | unlock; `FLASHBOOT_MOD_EN` ← 0; `wdt_hal_init` (off, stages off, reset lengths 7, `PROCPU_RESET_EN`, `PAUSE_IN_SLP`); stage 0 = action 4, `WDTCONFIG1` = 612,000 = `0x956A0`; feed; `EN`; lock (`bootloader_init.c:70-93`) | **running**: one stage, system reset after 9 s. `WDTCONFIG0` should read `0xC007EA00` on silicon |
| `esp_clk_init`, insn 1,695,147 | unlock, feed, stage 0 ← 1.6 s (`0x1A900`), lock (`clk.c:84-97`) | 1.6 s during the slow-clock selection |
| `esp_clk_init`, insn 2,185,310 | unlock, feed, stage 0 ← 9 s, lock | 9 s |
| `init_disable_rtc_wdt`, insn 2,342,799 | unlock, `EN` ← 0, lock | **stopped** for the rest of the run |
| `esp_restart_noos`, insn 3,401,638–3,401,893 | `wdt_hal_init`; stage 0 = action 3 after 1 s (`WDTCONFIG1` = `0x109A0`), stage 1 = action 4 after 1 s (`WDTCONFIG2` = `0x21340`); **`FLASHBOOT_MOD_EN` ← 1**; lock. `EN` is not set: the flash-boot mode arms the watchdog for the next boot (`system_internal.c:71-80`) | the next boot has 1 s (stage 0 as configured) to reach `bootloader_config_wdt` (ASSUMED that the flash-boot mode uses the configured stage 0 timeout; the action in that mode is system reset by `TRM:` §12.2.2.4) |

### C.4 Super watchdog SWD (READ `TRM:` §12.3, `rtc_cntl_reg.h:1706-1760`)

| Item | Fact |
|---|---|
| Nature | analog circuit; must be fed at least once per period "slightly less than one second"; about 100 ms before the end it raises a feed request (`SWD_CONF.SWD_FEED_INT` b1, interrupt `INT_RAW_RTC` bit 15); on time-out it resets the whole digital system, reset cause `0x12` |
| Registers | `SWD_CONF` 0xAC: b31 `AUTO_FEED_EN`, b30 `DISABLE`, b29 `FEED` (WO), b28 `RST_FLAG_CLR` (WO), b27:18 `SIGNAL_WIDTH` (300), b17 `BYPASS_RST`, b1 `FEED_INT` (RO), b0 `RESET_FLAG` (RO); reset `0x04B00000`. `SWD_WPROTECT` 0xB0: key `0x8F1D312A`, which is also its reset value (unlocked out of reset) |
| Reset default | its reset output is not under register control (ASSUMED from the wording of the analogous brownout code, `brownout_ll.h:111`: "give … control permission to the software") until `FIB_SEL` bit 2 (`RTC_CNTL_FIB_SUPER_WDT_RST`, register 0x10C, reset 7) is cleared; the bootloader clears it, clears `BYPASS_RST`, then sets `AUTO_FEED_EN` under the key (`IDF:bootloader_support/src/esp32c3/bootloader_soc.c:10-19`, `bootloader_esp32c3.c:79-84, 96-133`; insn 521,560–521,639) |
| ROM | reads `SWD_CONF` bit 0 and, if set, writes bit 28 and prints `analog super wdt reset` (`ROM3:analog_super_wdt_reset_happened@0x40048a26`, `clear_super_wdt_reset_flag@0x40048b58`, string at `0x3ff1b9bc`) |
| Consequence | once `AUTO_FEED_EN` is set the controller feeds the circuit itself: it never expires again. No real log of a blank chip shows `0x12`, so it does not fire during the ROM phase either (MEASURED (silicon) by absence, §H) |

### C.5 Brownout detector (READ `rtc_cntl_reg.h:2030-2070`, `IDF:hal/esp32c3/include/hal/brownout_ll.h`, `TRM:` §9.3.4.3)

`BROWN_OUT` 0xD8: b31 `DET` (RO, detector output, 0 when the supply is good), b30 `ENA` (reset 1), b29 `CNT_CLR`, b28 `ANA_RST_EN`, b27 `RST_SEL` (1 = chip reset, 0 = system reset), b26 `RST_ENA`, b25:16 `RST_WAIT` (`0x3FF`), b15 `PD_RF_ENA`, b14 `CLOSE_FLASH_ENA`, b13:4 `INT_WAIT` (1); reset `0x43FF0010`. Interrupt `INT_RAW_RTC` bit 9.
The bootloader enables the analog reset for chip revision ≥ v0.2 (`brownout_ll_ana_reset_enable`: clears a `FIB_SEL` bit, sets `ANA_RST_EN`). The application's `esp_brownout_init` writes the fields above (insn 2,294,177–2,294,548), sets the threshold through regi2c, clears and enables interrupt bit 9, and routes the RTC core interrupt to CPU line 2, priority 1 (insn 2,302,990).
For an emulator: storage, `DET` reads 0, the interrupt never fires.

## D. Expiry

### D.1 Three vocabularies for the same four reset levels

| TRM (`TRM:` §6.1) | ESP-IDF HAL action (`IDF:hal/include/hal/wdt_types.h`) | ESP-IDF reason prefix (`IDF:soc/esp32c3/include/soc/reset_reasons.h`) | ROM name suffix |
|---|---|---|---|
| CPU reset | `WDT_STAGE_ACTION_RESET_CPU` (2) | `RESET_REASON_CPU0_…` | `…_CPU_RST` |
| Core reset: the whole digital system except RTC | `WDT_STAGE_ACTION_RESET_SYSTEM` (3) | `RESET_REASON_CORE_…` | `…_SYS_RST` |
| System reset: digital system and RTC | `WDT_STAGE_ACTION_RESET_RTC` (4, RWDT only) | `RESET_REASON_SYS_…` | `RTCWDT_RTC_RST`, `SUPER_WDT_RST`, … |
| Chip reset | — | `RESET_REASON_CHIP_…` | `POWERON` |

### D.2 What each level resets

| Level | Reset | Survives | Tag, source |
|---|---|---|---|
| CPU reset | the CPU core; execution restarts at the reset vector `0x40000000`. "If CPU is reset, PMS registers will be reset, too" (the `SENSITIVE` permission registers) | every peripheral register, including `SYSTEM` (the clock selection stays as it was), TIMG, SYSTIMER, the interrupt matrix, the cache and MMU configuration; RTC; RAM | READ `TRM:` §6.1.3. Corroborated by `esp_restart_noos`, which resets by hand what a CPU reset leaves (watchdogs, cache, flash pins, UART/SPI/SYSTIMER/crypto peripheral resets, CPU clock back to the crystal) before `software_reset_cpu` (`system_internal.c:67-116`) |
| Core reset | CPU, all digital peripherals (TIMG with both MWDTs, SYSTIMER, `SYSTEM`, GPIO, radio): back to reset values, so `FLASHBOOT_MOD_EN` of MWDT0 is 1 again and the CPU clock is the crystal ÷ 2 | `RTC_CNTL` (RWDT configuration **and counter**, `STORE0..7`, slow-clock selection, super-watchdog configuration), the RTC timer, RTC memory; RAM contents | READ `TRM:` §6.1.3, §12.2.2.2 ("the main system (which includes MWDTs, CPU, and all peripherals) will be reset. The power management unit and RTC peripheral will not be reset"); RWDT counter: inferred from the logs of §H.1 (a watchdog with the longer period can only expire if the shorter one's resets do not restart it) |
| System reset | the above and the RTC registers (`STORE0..7` back to 0 is ASSUMED, §I.9) | the RTC timer (`TRM:` §9.3.3); RAM contents ("all reset types … except Chip Reset maintain the data stored in internal memory", `TRM:` §6.1.1) | READ |
| Chip reset | everything | nothing | READ |

One register behaves differently from its block (MEASURED (silicon), §H.1): `ASSIST_DEBUG.CORE_0_RCD_PDEBUGPC` (`0x600CE048`) still holds the recorded PC after a CPU reset and after a core reset, and reads 0 after a system reset or a power-on.

### D.3 Reset cause register, codes, and the ROM line

`RTC_CNTL.RESET_STATE` (0x38): b5:0 `RESET_CAUSE_PROCPU` (RO), b11:6 `RESET_CAUSE_APPCPU`, b13/b12 `STAT_VECTOR_SEL_*` (reset 1), flags in b14…b25 (`rtc_cntl_reg.h:456-552`). The ROM and ESP-IDF read it through `rtc_get_reset_reason(0)` = register & `0x3F` (`ROM3:rtc_get_reset_reason@0x40048a8e`). The value of the upper bits on silicon is not established (§I.8).

| Code | TRM source and level (`TRM:` Table 6.1-1) | ESP-IDF name | Name printed by the ROM (table at `ROM3 .rodata 0x3ff19da8`) | Produced by |
|---|---|---|---|---|
| `0x01` | chip reset (power-on, or brownout with chip reset selected) | `CHIP_POWER_ON` | `POWERON` | power-up |
| `0x03` | software system reset — core | `CORE_SW` | `RTC_SW_SYS_RST` | `OPTIONS0` bit 31 (`ROM3:software_reset@0x40048b74`) |
| `0x05` | deep-sleep reset — core | `CORE_DEEP_SLEEP` | `DSLEEP` | wake-up from deep sleep |
| `0x07` | MWDT0 core reset | `CORE_MWDT0` | `TG0WDT_SYS_RST` | MWDT0 stage action 3; **MWDT0 flash-boot protection** |
| `0x08` | MWDT1 core reset | `CORE_MWDT1` | `TG1WDT_SYS_RST` | MWDT1 stage action 3 |
| `0x09` | RWDT core reset | `CORE_RTC_WDT` | `RTCWDT_SYS_RST` | RWDT stage action 3 |
| `0x0B` | MWDT0 CPU reset | `CPU0_MWDT0` | `TG0WDT_CPU_RST` | MWDT0 stage action 2 |
| `0x0C` | software CPU reset | `CPU0_SW` | `RTC_SW_CPU_RST` | `OPTIONS0` bit 5 (`ROM3:software_reset_cpu@0x40048b84`); **`esp_restart`** |
| `0x0D` | RWDT CPU reset | `CPU0_RTC_WDT` | `RTCWDT_CPU_RST` | RWDT stage action 2 |
| `0x0F` | brownout — system (or chip) reset | `SYS_BROWN_OUT` | `BROWNOUT_RST` | brownout detector |
| `0x10` | RWDT system reset | `SYS_RTC_WDT` | `RTCWDT_RTC_RST` | RWDT stage action 4; **RWDT flash-boot protection**; the bootloader's 9 s stage |
| `0x11` | MWDT1 CPU reset | `CPU0_MWDT1` | `TG1WDT_CPU_RST` | MWDT1 stage action 2 |
| `0x12` | super watchdog — system reset | `SYS_SUPER_WDT` | `SUPER_WDT_RST` | super watchdog |
| `0x13` | clock glitch — system reset | `SYS_CLK_GLITCH` | `GLITCH_RTC_RST` | glitch detector |
| `0x14`…`0x17` | eFuse CRC, USB-UART, USB-JTAG, power glitch — core reset | `CORE_EFUSE_CRC`, `CORE_USB_UART`, `CORE_USB_JTAG`, `CORE_PWR_GLITCH` | `EFUSE_RST`, `USB_UART_CHIP_RESET`, `USB_JTAG_CHIP_RESET`, `POWER_GLITCH_RESET` | — |
| 0, 2, 4, 6, 14 | — | — | `N/A`; `0x0A` prints `INTRUSION_RST`; above 23 prints `unk` | — |

What the ROM does with the cause (READ `ROM3:main@0x40047e3c`, `boot_prepare@0x40047c5c`):

1. `boot_prepare` prints `ESP-ROM:%s`, `Build:%s`, then **`rst:0x%x (%s),boot:0x%x (%s)`** (format at `0x3ff1b89c`) with the cause, its name, `GPIO.STRAP` and the boot-mode name.
2. It enables the debug-assist clock, reads `ASSIST_DEBUG.CORE_0_RCD_PDEBUGPC` (`0x600CE048`) and, **if the value is not 0, prints `Saved PC:0x%08x`** (`0x3ff1b8bc`); then writes 3 to `CORE_0_RCD_EN` (`0x600CE044`), which re-arms the recording for the next reset (`boot_prepare+0x178…+0x192`). The bootloader re-arms it too (`wdt_reset_cpu0_info_enable`, `bootloader_esp32c3.c:49-54`).
3. `print_other_reset_reason@0x40047e0a` prints `analog super wdt reset` or `pro cpu reset by JTAG` when the matching flag is set.
4. `main` accepts the causes of the mask `0x00FFBF8A` (1, 3, 7–13, 15–23) and boots normally for all of them; 5 goes through `rtc_boot_control`; any other value (0, 2, 4, 6, 14, or above 23) prints `invalid reset` (`0x3ff1b99c`) and ends in `software_reset` (cause 3). **A watchdog reset is therefore followed by an ordinary boot**; nothing in the ROM depends on which watchdog fired.

Lines on real chips (MEASURED (silicon), §H): `rst:0x7 (TG0WDT_SYS_RST),boot:0xc (SPI_FAST_FLASH_BOOT)` followed by `Saved PC:0x4004d1f8`; `rst:0x10 (RTCWDT_RTC_RST),boot:0xc (SPI_FAST_FLASH_BOOT)` with no `Saved PC` line; `rst:0xc (RTC_SW_CPU_RST),boot:0xc (SPI_FAST_FLASH_BOOT)` followed by `Saved PC:0x403806fc`. The saved PC is the address of the instruction being executed when the reset struck: `0x40047ed2` is exactly the `j .` of `ROM3:main`.

What ESP-IDF does with the cause: `bootloader_check_wdt_reset` logs `PRO CPU has been reset by WDT.` for causes 7, 8, 9, 0x0B, 0x0D, 0x11 (`bootloader_esp32c3.c:63-77`); `esp_reset_reason()` maps 7 → `ESP_RST_TASK_WDT`, 8 → `ESP_RST_INT_WDT`, 9/0x10/0x12/0x0B/0x0D/0x11 → `ESP_RST_WDT`, and for 3/0x0C uses the hint a panic left in `RTC_CNTL.STORE6` (`IDF:esp_system/port/soc/esp32c3/reset_reason.c:17-117`); `esp_rtc_init` re-runs an analog calibration only after cause 1 (`clk.c:59-73`).

### D.4 The flash-boot watchdog: which one, its timeout, and the proof

**Proof that the ROM arms nothing and feeds nothing** (READ, tools `romscan.py` and `elfwords.py`, §L):

| Check on `esp32c3_rev3_rom.elf` | Result |
|---|---|
| Every `lui` forming the page of TIMG0 (`0x6001F`) or TIMG1 (`0x60020`), in the whole disassembly | one, at `0x40047c4c`, in `disable_default_watchdog` |
| Every 32-bit word in the range `0x6001F000…0x60020FFF` in any loadable segment (a pointer in a table or literal pool) | one hit, which is the encoding of that same `lui` |
| Every load or store at `0x60008090…0x600080B0` (RWDT, super watchdog) through a `lui 0x60008` base | `disable_default_watchdog` (`WDTCONFIG0`), `analog_super_wdt_reset_happened` and `clear_super_wdt_reset_flag` (`SWD_CONF`); **no access to `WDTFEED`, `WDTWPROTECT` or `WDTCONFIG1…4`** |
| The key `0x50D83AA1` as a data word or as the `lui` of either possible split | absent |
| The key `0x8F1D312A`, same search | absent |
| Callers of `disable_default_watchdog@0x40047c3a`: a `jal` to it, or a word holding its address or the address of its entry in the jump table (`0x400000A4`) | none: only the jump-table entry itself, for code outside the ROM |
| What `disable_default_watchdog` does | `RTC_CNTL.WDTCONFIG0 &= 0xFFFFEFFF` (bit 12) and `TIMG0.WDTCONFIG0 &= 0xFFFFBFFF` (bit 14): it clears the two `FLASHBOOT_MOD_EN` bits and nothing else |
| MMIO trace of the ROM on the flash-boot path, power-on to bootloader entry (MEASURED, M1 and the earlier T1) | no access to TIMG0, TIMG1, SYSTIMER, `RTC_CNTL.WDT*`; two reads of `SWD_CONF` |

Since the ROM cannot enable, configure or feed a watchdog, the resets seen on real chips come from the reset values of the registers: the flash-boot protection of `TRM:` §12.2.2.4.

| | MWDT0 | RWDT |
|---|---|---|
| Armed by | `TIMG0.WDTCONFIG0.FLASHBOOT_MOD_EN` = 1 (reset value) | `RTC_CNTL.WDTCONFIG0.FLASHBOOT_MOD_EN` = 1 (reset value; set again by `esp_restart`) |
| Timeout register | `WDTCONFIG2` = 26,000,000, prescaler 1, clock APB_CLK (`USE_XTAL` = 0) | `WDTCONFIG1` = 200,000, doubled by the eFuse rule: 400,000 slow-clock cycles |
| Timeout | **1.3 s** at APB = 20 MHz (power-on, or any core/system reset); 0.65 s at 40 MHz (after an `esp_restart`, whose CPU reset leaves `PRE_DIV_CNT` = 0). READ + ASSUMED: the product of the reset values and of the clock of `TRM:` §6.2.4.1; the only measurement is the lower bound of §H.2 | 2.94 s if the RC ran at 136 kHz; shorter in fact, since `SCK_DCAP` is 0 until the bootloader (ASSUMED, §I.2). 1 s after an `esp_restart` |
| Action | core reset, cause `0x07` | system reset, cause `0x10` |
| Stopped by | the bootloader, 52,000 instructions after its entry (`bootloader_config_wdt`), or `disable_default_watchdog` | idem |
| After its own reset | re-armed (TIMG0 is reset by a core reset) | re-armed |
| After the other's reset | re-armed, counter 0 (a system reset resets TIMG0) | **keeps counting** across an MWDT0 core reset |

Resulting behaviour of a chip that never reaches the bootloader (blank flash, or the `j .` after `ets_main.c 333`): `rst:0x7` every 1.3 s, and `rst:0x10` each time the RWDT's own period elapses, which restarts both (two `0x7` then one `0x10` with the nominal figures). The logs of §H.1 show both causes in the same loop; the exact ratio depends on the two open frequencies.

The protection must be conditional on the boot mode sampled from the strapping pins (ASSUMED, §I.4): the ROM stays in its download modes indefinitely, and nothing in it could stop the watchdogs there.

### D.5 Stage actions, summary

| Watchdog | Action | Effect | Cause |
|---|---|---|---|
| MWDT0 / MWDT1 | 1 interrupt | `TIMG.INT_RAW_TIMERS` bit 1 ← 1 (level, source 33 / 35), next stage starts | — |
| | 2 reset CPU | CPU reset (D.2). Whether `PROCPU_RESET_EN` (reset 0, never set by ESP-IDF's MWDT code) must be 1 is not established (§I.5); ESP-IDF never uses this action | `0x0B` / `0x11` |
| | 3 reset system | core reset | `0x07` / `0x08` |
| RWDT | 1 interrupt | `RTC_CNTL.INT_RAW_RTC` bit 3 ← 1 | — |
| | 2 reset CPU | CPU reset (ESP-IDF sets `PROCPU_RESET_EN`) | `0x0D` |
| | 3 reset system | core reset | `0x09` |
| | 4 reset RTC | system reset | `0x10` |
| Super watchdog | — | system reset; `SWD_CONF.RESET_FLAG` ← 1 | `0x12` |

Reference application, if its ticks stopped (READ): MWDT1 raises its interrupt after 300 ms (panic handler when it can run), then core-resets 600 ms later → `rst:0x8 (TG1WDT_SYS_RST)`. If a task starved the idle task: MWDT0 interrupt after 5 s (the handler prints the task-watchdog report and feeds; no panic, `CONFIG_ESP_TASK_WDT_PANIC` is off), core reset only if the interrupt itself is not served for 10 s more → `rst:0x7`.

## E. Time sources an application reads

| API | What it reads | Source |
|---|---|---|
| `esp_rom_delay_us(us)` | busy loop on the CPU cycle counter, CSR `0x802` (user mirror of `0x7E2`), scaled by the ROM variable `s_ticks_per_us` (40 after ROM boot, 80 in the bootloader, 160 in the application). No timer peripheral | `esp32c3-bootloader-accesses.md` §4 (`ROM3:ets_delay_us@0x400462cc`) |
| `esp_cpu_get_cycle_count()` | CSR `0x7E2` | idem |
| `esp_log_early_timestamp()` (all bootloader lines; application lines before the scheduler) | `cycle count / (ticks_per_us × 1000)` | `IDF:log/src/os/log_timestamp.c:28-37` |
| `esp_log_timestamp()` after the scheduler starts | `base + xTaskGetTickCount() × 10` ms, where `base` is the early timestamp sampled at the first call: the cycle counter is no longer read | `log_timestamp.c:14-25` |
| `xTaskGetTickCount()` | the RAM variable `xTickCount`, incremented by the tick ISR, i.e. by SYSTIMER counter 1 + comparator 0 (§A.4) | `IDF:freertos/port_systick.c` |
| `vTaskDelay`, every FreeRTOS time-out | the same tick | |
| `esp_timer_get_time()` | SYSTIMER counter 0 ÷ 16: µs since `esp_timer_impl_early_init` reset the block | `esp_timer_impl_systimer.c:68-75` |
| `esp_system_get_time()` | `esp_timer_get_time() + s_correction_us`, the correction being `esp_rtc_get_time_us() − g_startup_time − esp_timer_get_time()` at early init | `IDF:esp_timer/src/system_time.c:27-41` |
| `rtc_time_get()` / `esp_rtc_get_time_us()` / `esp_clk_rtc_time()` | the RTC timer (§C.2), scaled by the calibration in `STORE1` | `rtc_time.c:176-179`, `esp_clk.c:101-153` |
| `gettimeofday()`, `time()` | boot time (`RTC_CNTL.STORE2/STORE3`, 0 after power-on, hence 1970) + `s_microseconds_offset` + `esp_system_get_time()`, i.e. **SYSTIMER counter 0**; the RTC timer is read once at start-up (`esp_set_time_from_rtc`) to compute the offset and once at shutdown (`esp_sync_timekeeping_timers`) to save the boot time | `IDF:newlib/src/port/esp_time_impl.c:39-110`, `IDF:newlib/src/time.c:167-175` |
| `clock_gettime(CLOCK_MONOTONIC)` | `esp_system_get_time()` → SYSTIMER counter 0 | `newlib/src/time.c:281` |

## F. Measured sequence in the reference application (trace M1, QEMU)

### F.1 Timeline

| insn | Event |
|---|---|
| 573,561–573,830 | bootloader: RWDT started (9 s), MWDT0 flash-boot bit cleared (§C.3, §B.3) |
| 1,683,222 | application entry |
| 1,683,246 | R `RESET_STATE` = 1 |
| 1,695,147–1,695,201 | RWDT fed, 1.6 s |
| 1,714,191–2,184,895 | calibration (§B.4); 470,567 instructions of delay |
| 2,185,038–2,185,201 | R `STORE1` = 0; first RTC timer read (`TIME_UPDATE` W bit 31, `TIME_LOW0` = 0, `TIME_HIGH0` = 0); W `STORE1 = 0x3AD2CD` |
| 2,185,310–2,185,364 | RWDT fed, 9 s |
| 2,185,952 | RTC timer read for `g_startup_time` |
| 2,292,318–2,292,479 | `esp_timer_impl_early_init` (§A.4); first read of `SYSTIMER.CONF` = `0x46000000` |
| 2,292,543; 2,292,711 | RTC timer read, then first read of counter 0 (`UNIT0_OP` R `0x20000000`, W `0x60000000`, R `0x20000000`; `UNIT0_VALUE_LO` = `0x48C1C`, `_HI` = 0, `_LO` again) |
| 2,294,177–2,303,832 | brownout set-up (§C.5) |
| 2,303,922–2,319,271 | RTC timer read (`esp_set_time_from_rtc`) and five more reads of counter 0 (`gettimeofday` during start-up) |
| 2,319,492–2,319,510 | TIMG0 gate, reset pulse, R/W `TIMG0.WDTCONFIG0` (flash-boot bit) |
| 2,342,799–2,342,819 | RWDT disabled |
| 2,342,906–2,343,385 | MWDT1 = interrupt watchdog (§B.3) |
| 2,368,368–2,369,122 | tick timer (§A.4) |
| 2,369,224 | scheduler start: first interrupt (software interrupt, CPU line 3) |
| 2,388,614–2,390,114 | MWDT0 = task watchdog (§B.3) |
| 2,396,803 | `Hello world!` |
| 2,422,698 | yield out of the first `vTaskDelay` |
| 2,423,330 | first tick interrupt; then one every 839 instructions while idle |
| 2,419,952 … 3,394,472 | eleven `Restarting in N seconds...` lines and `Restarting now.`, 100 ticks apart (1.0000 s ± 0.0001 s of counter 1) |
| 3,399,071–3,401,387 | shutdown handler `esp_sync_timekeeping_timers`: three reads of counter 0, one of the RTC timer, `STORE2/STORE3` (boot time) rewritten twice |
| 3,401,638–3,402,177 | `esp_restart_noos` (§C.3, §B.3): RWDT for the next boot; both MWDT disabled; `PERIP_RST_EN0` \|= `0x20000007` then 0 (bit 29 = SYSTIMER; **the timer groups are not reset**); `SYSCLK_CONF`: `PRE_DIV_CNT` ← 0, `SOC_CLK_SEL` ← 0; `STORE5 = 0x26252625`; `OPTIONS0` \|= `0x20` |
| 3,402,183 | first instruction after the CPU reset (`_init`) |
| 3,407,701 | R `RESET_STATE` = `0xC`; the ROM prints `rst:0xc (RTC_SW_CPU_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)` |
| 5,085,802 … 6,806,179 | second boot, identical accesses (differences in read values: §G) |

### F.2 First access to each timer or watchdog register, application only (first boot)

| insn | Register | Access | Function |
|---|---|---|---|
| 1,695,147 | `RTC_CNTL.WDTWPROTECT` | W `0x50D83AA1` | `wdt_hal_write_protect_disable` ← `esp_clk_init` |
| 1,695,155 | `RTC_CNTL.WDTFEED` | R 0, W `0x80000000` | `wdt_hal_feed` |
| 1,695,175 | `RTC_CNTL.WDTCONFIG0` | R 0, W `0x40000000` | `wdt_hal_config_stage` |
| 1,695,190 | `RTC_CNTL.WDTCONFIG1` | W `0x1A900` | `wdt_hal_config_stage` |
| 1,714,191 | `TIMG0.RTCCALICFG` | R `0x13000` | `rtc_clk_cal_internal` |
| 1,714,195 | `TIMG0.RTCCALICFG2` | R 0, W `0x80` | `rtc_clk_cal_internal` |
| 2,184,888 | `TIMG0.RTCCALICFG1` | R `0x024C3C00` | `rtc_clk_cal_internal` |
| 2,185,038 | `RTC_CNTL.STORE1` | R 0 | `esp_clk_slowclk_cal_get` |
| 2,185,053 | `RTC_CNTL.TIME_UPDATE` | R 0, W `0x80000000` | `rtc_time_get` |
| 2,185,058 | `RTC_CNTL.TIME_LOW0`, `TIME_HIGH0` | R 0, R 0 | `rtc_time_get` |
| 2,292,400 | `SYSTIMER.CONF` | R `0x46000000`, W `0xC6000000` | `systimer_hal_init` |
| 2,292,437 | `SYSTIMER.TARGET2_CONF` | R 0, W 0 | `systimer_hal_select_alarm_mode` |
| 2,292,711 | `SYSTIMER.UNIT0_OP` | R `0x20000000`, W `0x60000000` | `systimer_hal_get_counter_value` |
| 2,292,725 | `SYSTIMER.UNIT0_VALUE_LO`, `_HI` | R `0x48C1C`, R 0 | `systimer_hal_get_counter_value` |
| 2,294,177 | `RTC_CNTL.BROWN_OUT` | R 0 | `brownout_hal_config` |
| 2,319,506 | `TIMG0.WDTCONFIG0` | R 0, W 0 | `enable_timer_group0_for_calibration` |
| 2,342,920 | `TIMG1.WDTCONFIG0` | R 0, W 0 | `esp_int_wdt_init` |
| 2,343,012 | `TIMG1.WDTWPROTECT` | W `0x50D83AA1` | `wdt_hal_init` |
| 2,343,057 | `TIMG1.INT_CLR_TIMERS` | R 0, W 2 | `wdt_hal_init` |
| 2,343,061 | `TIMG1.INT_ENA_TIMERS` | R 0, W 2 | `wdt_hal_init` |
| 2,343,089 | `TIMG1.REGCLK` | R 0, W `0x20000000` | `wdt_hal_init` |
| 2,343,094 | `TIMG1.WDTCONFIG1` | R 0, W `0x4E200000` | `wdt_hal_init` |
| 2,343,146 | `TIMG1.WDTCONFIG2` | W `0x2710` | `wdt_hal_config_stage` |
| 2,343,180 | `TIMG1.WDTCONFIG3` | W `0x2710` | `wdt_hal_config_stage` |
| 2,343,196 | `TIMG1.WDTFEED` | W 1 | `wdt_hal_enable` |
| 2,368,942 | `SYSTIMER.UNIT1_LOAD_HI`, `_LO`, `UNIT1_LOAD` | R 0, W 0, W 0, W 1 | `vSystimerSetup` |
| 2,368,979 | `SYSTIMER.TARGET0_CONF` | R 0 | `systimer_hal_select_alarm_mode` |
| 2,369,057 | `SYSTIMER.COMP0_LOAD` | W 1 | `systimer_hal_set_alarm_period` |
| 2,369,106 | `SYSTIMER.INT_ENA` | R 0, W 1 | `systimer_hal_enable_alarm_int` |
| 2,388,893 | `TIMG0.WDTWPROTECT` | W `0x50D83AA1` | `wdt_hal_init` ← `esp_task_wdt_init` |
| 2,388,938…2,389,061 | `TIMG0.INT_CLR_TIMERS`, `INT_ENA_TIMERS`, `REGCLK`, `WDTCONFIG1`, `WDTCONFIG2` (`0x2710`), `WDTCONFIG3` (`0x4E20`) | as for TIMG1 | `wdt_hal_init`, `wdt_hal_config_stage` |
| 2,389,893 | `TIMG0.WDTFEED` | W 1 | `wdt_hal_feed` |
| 2,423,447 | `SYSTIMER.INT_CLR` | R 0, W 1 | `SysTickIsrHandler` |
| 2,423,456 | `SYSTIMER.UNIT1_OP` | R `0x20000000`, W `0x60000000` | `systimer_hal_get_counter_value` |
| 2,423,470 | `SYSTIMER.UNIT1_VALUE_LO`, `_HI` | R `0x72827`, R 0 | `systimer_hal_get_counter_value` |
| 2,423,803 | `SYSTIMER.INT_ST` | R 0 | `SysTickIsrHandler` |

Interrupt routing written by the application before the first tick (MEASURED): RTC core (brownout) → CPU line 2, priority 1; `CPU_INTR_FROM_CPU_0` → 3, priority 1; `SYSTIMER_TARGET0` → 4, priority 1; `TG_WDT` (TIMG0) → 5, priority 1; `TG1_WDT` → 24, priority 4. `SYSTIMER_TARGET2`, `TG_T0`, `TG1_T0` are never routed.

Registers the application never touches in this run: T0 of both groups, `SYSTIMER.TARGET1_*`, `TARGET2_HI/LO`, `COMP1_LOAD`, `COMP2_LOAD`, `UNIT0_LOAD*`, `INT_RAW`, `RTC_CNTL.SLP_TIMER*`, `TIME_LOW1/HIGH1`, `WDTCONFIG3/4`, `SWD_CONF`, `TIMG1.RTCCALI*`.

### F.3 Steady state: one tick (10 ms; 839 instructions when idle)

Interrupt taken while the idle task sits in `wfi` (`esp_cpu_wait_for_intr`), CPU line 4:

| Order | Register | Access | Function |
|---|---|---|---|
| 1 | `ASSIST_DEBUG.CORE_0_MONTR_ENA`, `CORE_0_SP_MIN`, `CORE_0_SP_MAX` | stack-guard switch (R/W, W, W, R/W) | `rtos_int_enter` |
| 2 | `INTERRUPT_CORE0.CPU_INT_THRESH`, `CPU_INT_PRI_4` | R 1, R 1, W `THRESH` = 2 | `_tee_interrupt_handler` |
| 3 | `SYSTIMER.INT_CLR` | R 0, W 1 | `SysTickIsrHandler` |
| 4 | `SYSTIMER.UNIT1_OP` | R `0x20000000`, W `0x60000000`, R `0x20000000` | `systimer_hal_get_counter_value` |
| 5 | `SYSTIMER.UNIT1_VALUE_LO`, `_HI`, `_LO` | R, R, R | idem |
| 6 | `SYSTIMER.TARGET0_CONF` | R `0xC0027100` | `SysTickIsrHandler` |
| 7 | `TIMG1.WDTWPROTECT` | W `0x50D83AA1` | interrupt-watchdog tick hook |
| 8 | `TIMG1.WDTCONFIG0`, `WDTCONFIG2` | R `0xB83F8000`, W same, W `WDTCONFIG2 = 0x258`, R, W `0xB87F8000` | `wdt_hal_config_stage` (stage 0) |
| 9 | `TIMG1.WDTCONFIG0`, `WDTCONFIG3` | R, W, W `WDTCONFIG3 = 0x4B0`, R, W `0xB87F8000` | `wdt_hal_config_stage` (stage 1) |
| 10 | `TIMG1.WDTFEED`, `WDTWPROTECT` | W 1, W 0 | `wdt_hal_feed` |
| 11 | `CPU_INT_THRESH` | R, W 4, W 2 | critical section in `xTaskIncrementTick` |
| 12 | `SYSTIMER.INT_ST` | R 0 | loop condition |
| 13 | `CPU_INT_THRESH`; stack guard | W 1; as step 1 | exit |
| 14 | `CPU_INT_THRESH`; `TIMG0.WDTWPROTECT` W key, `TIMG0.WDTFEED` W 1, `TIMG0.WDTWPROTECT` W 0 | | idle task: `esp_task_wdt_reset` |
| 15 | `ASSIST_DEBUG.C0RE_0_DEBUG_MODE` | R 0 | `rv_utils_dbgr_is_attached`, then `wfi` |

Totals between the first tick and `Restarting now.` (MEASURED): 1,110 interrupts = 1,100 ticks + 10 software interrupts (one yield per `vTaskDelay`); SYSTIMER 2,200 writes and 8,800 reads; TIMG1 9,900 writes and 4,400 reads; TIMG0 3,297 writes; no access to `RTC_CNTL`, to counter 0 or to comparator 2.
Once per second: the main task wakes, prints a line through `UART0.FIFO`, calls `vTaskDelay(100)`; 5,583 instructions instead of 839 in that tick.

## G. Where QEMU differs from the hardware

| # | QEMU `esp-develop-9.2.2-20250817` (MEASURED in M1 or READ `QEMU:`) | Hardware / what a lockstep must mask |
|---|---|---|
| 1 | No flash-boot protection: `TIMG.WDTCONFIG0` resets to 0, the RWDT does not exist. A hung ROM stays hung | MWDT0 and RWDT armed at reset (§D.4) |
| 2 | `RTC_CNTL` models `OPTIONS0`, `RESET_STATE` and `STORE0..7` only. RWDT, super watchdog, brownout, RTC timer, clock configuration: reads 0, writes dropped. `TIME_LOW0` is always 0 | storage at least; a counting RTC timer; RWDT expiry |
| 3 | MWDT: expiry is implemented. Actions 2 and 3 both request a **whole-machine reset** with cause 7 (TIMG0) or 8 (TIMG1); the causes `0x0B`/`0x11` are never produced. A machine option `wdt_disable` exists | CPU reset vs core reset (§D.2) |
| 4 | MWDT write protection covers `WDTCONFIG0` only; `WDTFEED`, `WDTCONFIG1…5` are accepted while locked | all registers protected (`TRM:` §12.2.2.3) |
| 5 | `WDTCONFIG1…5` read 0 until written (hardware: `0x00010000`, 26,000,000, …); the written values take effect at the next `WDTCONFIG0` write carrying `CONF_UPDATE_EN`; `WDTCONFIG0` reads back without bits 10:0 and bit 22 | reset values of §B.1 |
| 6 | MWDT clock: constant 80 MHz (APB) or 40 MHz (crystal) ÷ prescaler, whatever `SYSCLK_CONF` says | APB = CPU clock when the CPU runs from the crystal |
| 7 | A stage with action "off" is still waited for, without clearing the counter. Lowering the current stage's timeout below the current count stops the watchdog until it is fed (stated in the model as the behaviour of the real hardware) | unverified on silicon (§I.5) |
| 8 | Calibration: completes inside the register write; `RDY` is set by `START` **and by `START_CYCLING`**; nominal frequencies 136,000, 17,500,000 ÷ 256 = 68,359 and **32,000 Hz for the 32 kHz crystal**, which gives 1,280,000, outside ESP-IDF's ±0.05 % window: an application configured for the crystal falls back to the RC in QEMU. `TIMEOUT` is a heuristic on `TIMEOUT_RST_CNT`; `RTCCALICFG2` resets to 0 (hardware `0xFFFFFF98`) | 7.5 to 31 ms; result varies with the chip |
| 9 | SYSTIMER: `VALUE_VALID` always reads 1. Counter values come from the virtual clock with truncation at every read (jitter of some hundred counts between ticks, MEASURED). **Counter 0 counts from the start of the machine and is reset neither by `PERIP_RST_EN0` nor by a machine reset** (first read `0x48C1C` = 18.6 ms; `0xA86D506` at the second boot). **Counter 1 starts with the virtual time since the start of the machine as offset**: the first tick ISR reads 469,031 (2.93 periods) and accounts for 2 ticks, where hardware gives one period plus the interrupt latency | counters reset with the block; counter 1 = n × 160,000 + latency at each tick |
| 10 | Period alarms are scheduled relative to the instant of programming, not to counter multiples: the ISR reads counter 1 ≡ 148,000 mod 160,000. The target-mode alarm that the tick set-up arms on a stopped counter does not fire (§A.4) | alarms at multiples of the period after the load; one probable early interrupt |
| 11 | SYSTIMER stall bits are stored without effect; `CONF` bit 25 always reads 1, bit 27 always 0 | stall only matters under a debugger |
| 12 | `SYSTEM.PERIP_CLK_EN0`/`PERIP_RST_EN0` read 0 and reset nothing: `timer_ll_reset_register`, `systimer_ll_reset_register` have no effect. `SYSCLK_CONF` reads `0xA8401` whatever was written | peripheral resets restore reset values |
| 13 | `RESET_STATE` reads the cause alone (1, then `0xC`). `ASSIST_DEBUG` is not modelled: **no `Saved PC` line**. The cycle counter (CSR `0x7E2`) is not reset by the machine reset: the bootloader of the second boot starts at `I (22065)` (third boot: `I (12508)`, the 32-bit counter having wrapped) where a real chip prints `I (24)` after the same reset (§H.3). TIMG calibration registers keep their values across the reset | see §D.2, §D.3 |
| 14 | `-icount 3`: every instruction lasts 8 ns of virtual time, at any CPU frequency; `wfi` jumps to the next timer deadline | cycle-accurate time is inkemu's own |

To mask in a register-level lockstep: every value read from `UNITn_VALUE_LO/HI`, `RTCCALICFG*`, `TIME_LOW0/HIGH0`, `WDTCONFIG*`, `RESET_STATE` above bit 5, `STORE1`…`STORE3`; the instants of interrupt delivery; the number of ticks accounted by the first tick ISR; log timestamps. The **sequence** of accesses (§F) is comparable.

## H. Evidence and timing from real chips

Logs published in GitHub issues, all with the banner `ESP-ROM:esp32c3-api1-20210207` (rev3 ROM): `mocleiri/tensorflow-micropython-examples#103`, `espressif/esp-cryptoauthlib#17`, `dlktdr/BTWifiModule#16`, `shmuelzon/esp32-ble2mqtt#212`, `wled/WLED#5068`, `TheRealMoeder/Energy2Shelly_ESP#98` and `#116`. The esp32.com threads cited in `esp32c3-rom-flash-boot.md` §H still cannot be fetched.

### H.1 Reset causes observed

| Situation | Lines | Issue |
|---|---|---|
| Erased flash, `invalid header: 0xffffffff` loop | `rst:0x7 (TG0WDT_SYS_RST),boot:0xc (SPI_FAST_FLASH_BOOT)` / `Saved PC:0x4004d1f8`; in the same capture `rst:0x10 (RTCWDT_RTC_RST),boot:0xc (SPI_FAST_FLASH_BOOT)` with **no** `Saved PC` line, twice, alternating with `0x7` (excerpts separated by "…") | `mocleiri#103`; same pair in `esp-cryptoauthlib#17` |
| Image rejected by the ROM (`Invalid image block, can't boot.` / `ets_main.c 333`) | `rst:0x7 (TG0WDT_SYS_RST)` / `Saved PC:0x40047ed2`, repeated | `mocleiri#103` |
| Other saved PCs in `invalid header` loops | `0x4004d1fa` (`SPI_WakeUp+0x32`), `0x4004c0d4` (`uart_tx_one_char_uart.part.0+0x16`, the UART FIFO poll), `0x4005885e` (`memcpy+0xa4`) | `ble2mqtt#212`, `WLED#5068` |
| `esp_restart` / panic reboot | `rst:0xc (RTC_SW_CPU_RST)` / `Saved PC:0x403806fc` (or `0x4038128a`) | `mocleiri#103`, `BTWifiModule#16` |

What follows: both flash-boot watchdogs are live on silicon; MWDT0 fires first; the RWDT is not restarted by MWDT0's core reset; the recorded PC survives a core reset and is cleared by a system reset.

### H.2 Lower bound of the MWDT0 flash-boot period

`BTWifiModule#16` has host timestamps with a resolution of 1 s: 184 `invalid header` lines stamped `23:16:00`, then 223 stamped `23:16:01`, then the banner with `rst:0x7`. So at least **407 iterations ran between two resets**. Each iteration prints 27 characters on UART0 at 115,200 baud, and the ROM waits for room in the transmit FIFO (the saved PC `0x4004c0d4` above is in that wait), so at most 426 lines per second leave the chip: 407 lines take at least **0.955 s**. That excludes a 0.65 s period (APB at 40 MHz) and agrees with 1.3 s (APB at 20 MHz, at most 554 lines). ASSUMED in this reasoning: the chip also drives UART0 on that board, whose console is USB-Serial-JTAG; the ROM prints on both channels by default.

### H.3 Durations in a boot log

`BTWifiModule#16`, chip v0.4, ESP-IDF 5.4.0, a 919 kB Wi-Fi/BLE application, boot after `rst:0xc` (so the ROM ran at 40 MHz), timestamps from the cycle counter:

| Line | Timestamp |
|---|---|
| `boot: ESP-IDF 5.4.0 2nd stage bootloader` | 24 ms |
| `boot: Loaded app from partition at offset 0x10000` | 242 ms (image load and SHA: 75 → 242 ms) |
| `cpu_start: Unicore app` | 254 ms |
| `cpu_start: Pro cpu start user code` | 262 ms |
| `main_task: Started on CPU0` | 375 ms |

The 8 ms between the two `cpu_start` lines contain the RTC watchdog writes and the calibration: consistent with 7.53 ms for 1,024 cycles of a 136 kHz clock. The timestamp restarts near 0 after a CPU reset: the cycle counter is reset by it.

| Quantity | Value | Tag |
|---|---|---|
| Calibration, RC, 1,024 cycles | 7.53 ms nominal; ≤ 8 ms observed | READ, MEASURED (silicon) |
| Super watchdog period | slightly less than 1 s; feed request about 100 ms before | READ `TRM:` §12.3 |
| MWDT0 flash-boot period | 1.3 s; ≥ 0.955 s observed | ASSUMED, MEASURED (silicon) |
| Tick | 10 ms exactly (160,000 counts of a crystal-derived clock); the ISR runs after the interrupt latency; jitter on silicon not measured | READ |
| ROM + bootloader up to its banner, after a CPU reset | 24 ms | MEASURED (silicon) |
| QEMU, for comparison | `I (18) cpu_start: Unicore app`, `I (28) main_task: Started on CPU0`; calibration 0 s of device time plus the 7,529 µs software delay | MEASURED |

## I. Open questions, ranked by risk

| # | Question | Why it matters | Status |
|---|---|---|---|
| 1 | Exact MWDT0 flash-boot timeout, and whether APB really is 20 MHz for the watchdog prescaler at power-on | sets the period of the blank-flash reset loop and the budget of the modelled ROM path (120,671 instructions on the flash path is far below it at any plausible speed) | 1.3 s inferred from reset values and `TRM:` §6.2.4.1; lower bound 0.955 s from one log |
| 2 | Real RC_SLOW frequency: at reset (`SCK_DCAP` = 0), and per chip once set to 255; hence the RWDT flash-boot timeout, the ratio of `0x10` to `0x7` in the loop, the calibration value a real chip returns and its spread | the model returns nominal values; a lockstep against silicon would differ in `STORE1` and in every RTC-time conversion | no measurement found; nominal 136 kHz only |
| 3 | In flash-boot mode, which timeout and action the RWDT uses after `esp_restart` (stage 0 was reprogrammed to 1 s / core reset; the TRM says the mode forces a system reset) | decides whether a slow second boot is reset with cause `0x09` or `0x10`, and after how long | source comment + TRM, not observed |
| 4 | The condition that gates the flash-boot protection (boot mode latched from the straps; download modes exempt) | inkemu only boots from flash today; matters when download mode is modelled | inferred from the absence of any watchdog code in the ROM |
| 5 | MWDT details never exercised by ESP-IDF: `PROCPU_RESET_EN` for action 2; whether an "off" stage is waited for; the effect of rewriting the current stage's timeout below the count; whether the counter restarts when `EN` goes 0 → 1 without a feed | only matter for firmware that programs the watchdog itself | QEMU's choices (§G.7) are the only description |
| 6 | SYSTIMER: latency of `VALUE_VALID`; whether `PERIP_RST_EN0` bit 29 zeroes the counters; whether the target-mode comparator fires on a stopped counter equal to its target (the early tick interrupt of §A.4) | the first changes nothing functionally; the second shifts `esp_timer_get_time()` by the boot time; the third adds one interrupt before the first real tick | read in the TRM, not observed |
| 7 | Calibration corner cases: does `RDY` ever assert in cycling mode; when does `TIMEOUT` clear; meaning of `TIMEOUT_RST_CNT`; unit of `TIMEOUT_THRES` (crystal cycles assumed) | the pre-wait of step 1 must terminate; a stale `TIMEOUT` is harmless because `RDY` is tested first | assumed |
| 8 | `RESET_STATE` bits above the cause on silicon (bits 13:12 reset to 1); exact semantics of `ASSIST_DEBUG.CORE_0_RCD_PDEBUGPC` (when it is latched, what clears it) | the ROM masks the cause; the `Saved PC` line needs the record register | inferred from logs |
| 9 | What a system reset does to `STORE0..7` and to RTC memory | after `rst:0x10` the bootloader and application read `STORE1`, `STORE4`, `STORE6` | assumed cleared |
| 10 | Whether the CPU cycle counter is reset by a core reset as it is by a CPU reset | log timestamps after a watchdog reset | observed for the CPU reset only |
| 11 | Super watchdog between reset and the bootloader's auto-feed: inactive because of `FIB_SEL`, or simply slower than the ROM | none while the model never fires it | inferred |
| 12 | The retail Xteink firmware: tick rate (Arduino builds use 1,000 Hz), use of `esp_timer` alarms (comparator 2) and of T0 | this report measured the 100 Hz `hello_world` only | not measured |

## J. Minimal model per block

Time base: device time, with the crystal as reference. One SYSTIMER count is exactly 62.5 ns; it does not depend on the CPU frequency, so events must not be scheduled in CPU cycles (carry-over note, decision 2).

| Block | State | Behaviour needed now | Can wait |
|---|---|---|---|
| **TIMG0 calibration** | `RTCCALICFG`, `RTCCALICFG1`, `RTCCALICFG2` with the reset values of §B.1; the slow-clock frequency of each source | On `START` 0 → 1: result = `floor(40,000,000 × MAX / f(CLK_SEL))`, placed in `RTCCALICFG1[31:7]`; `RDY` ← 1 after `MAX / f` of device time (or at once); `TIMEOUT` ← 0. While `START_CYCLING` = 1: report `TIMEOUT` = 1 (or `RDY` = 1) as soon as `RTCCALICFG2` is written, so that step 1 of §B.4 ends. `RDY` is read-only and cleared by a write with `START` = 0 followed by a new start. Never return 0 for the RC | an absent 32 kHz crystal (result 0 / `TIMEOUT` when the count exceeds `TIMEOUT_THRES`); per-device frequency error |
| **SYSTIMER** | two 52-bit counters (value, enabled), per-counter latch and load value, three comparators (target, period, unit, mode, enabled, loaded copies), `INT_ENA`, `INT_RAW`, `CONF` | 16 MHz counting when `UNITn_WORK_EN`; `UPDATE` latches and sets `VALUE_VALID`; `UNITn_LOAD`; `COMPx_LOAD` copies target and period; period mode: alarm every `PERIOD` counts from the count at which the comparator starts; target mode: alarm at equality, immediately if already passed; alarm sets `INT_RAW`; level output `INT_RAW & INT_ENA` to sources 37–39; `INT_CLR` write-1; reset values; reset by `PERIP_RST_EN0` bit 29 | stall bits; the 2^51 wrap rule; `VALUE_VALID` latency |
| **RTC timer** | 48-bit counter of slow-clock cycles, two capture pairs | `TIME_UPDATE` bit 31 copies the count into `TIME_LOW0/HIGH0` (previous pair into `LOW1/HIGH1`); counts at the selected slow-clock frequency from power-on; survives every reset but power-on | alarm (`SLP_TIMER*`), the other capture triggers |
| **MWDT0, MWDT1** | registers of §B.1, 32-bit counter, current stage, shadow configuration | key check on every write; `CONF_UPDATE_EN` commits; clock = (crystal or APB) ÷ prescaler; `EN`; feed; per-stage action: interrupt (`INT_RAW` bit 1, sources 33/35), CPU reset, core reset; flash-boot mode for MWDT0 (stage 0 = core reset after `WDTCONFIG2` counts while bit 14 is set); reset values restored by `PERIP_RST_EN0` bits 13/15 and by a core reset | T0 (no user in the reference path): counter, divider with the 0 → 65536 and 1 → 2 rules, `T0UPDATE` self-clear, alarm with `ALARM_EN` auto-clear, auto-reload |
| **RWDT** | registers of §C.3, 32-bit counter, current stage | key; `EN`; feed (bit 31); stage 0 timeout = `WDTCONFIG1 << (1 + WDT_DELAY_SEL)`; actions interrupt, CPU, core, system reset; flash-boot mode (stage 0 = system reset while bit 12 is set); clock = slow clock; **not reset by CPU or core resets** | `PAUSE_IN_SLP`, chip reset |
| **Super watchdog** | `SWD_CONF`, `SWD_WPROTECT` as storage; bit 0 reads 0 | never fires | everything else |
| **Brownout** | `BROWN_OUT` as storage; bit 31 reads 0 | never fires | |
| **Reset machinery** | reset cause; `ASSIST_DEBUG.CORE_0_RCD_EN`, `CORE_0_RCD_PDEBUGPC` | four levels with the scopes of §D.2; cause codes of §D.3 in `RESET_STATE[5:0]`; `OPTIONS0` bit 31 → core reset, cause 3; bit 5 → CPU reset, cause `0xC`; on a CPU or core reset with `RCD_EN` = 3, `RCD_PDEBUGPC` ← PC of the interrupted instruction; cleared by system and chip resets; CPU cycle counter ← 0; the CPU restarts at `0x40000000` | deep-sleep cause 5; glitch, eFuse and USB causes |

Suggested order (each step is observable on the reference image): calibration → `Unicore app` is passed and `cpu freq: 160000000 Hz` appears; SYSTIMER counter 0 → the start-up runs to the scheduler; software interrupt (other plan) → `Hello world!`; SYSTIMER comparator 0 → the countdown; `OPTIONS0` bit 5 and the CPU reset → `rst:0xc (RTC_SW_CPU_RST)` and a second boot; MWDT and RWDT expiry with the flash-boot mode → the blank-flash loop `rst:0x7` / `rst:0x10` with `Saved PC:0x4004d1f8`.

Three traps for the implementation:

- A CPU reset must leave `SYSTEM.SYSCLK_CONF`, TIMG and `RTC_CNTL` untouched: `esp_restart` relies on the RWDT it has just programmed and on the clock it has just selected.
- A core reset must bring `TIMG0.WDTCONFIG0` back to `0x0004C000` (flash-boot protection armed again) but must not touch the RWDT counter.
- The tick ISR divides counter 1 by the period: after a pause of the emulation the counter must have advanced by the paused device time, or ticks are lost or replayed in bursts.

## K. Corrections to earlier reports (those files are left untouched)

| Place | Statement there | Established here |
|---|---|---|
| `esp32c3-boot-chain.md` §E.3, §K.4; `esp32c3-rom-flash-boot.md` §L.2; `esp32c3-bootloader-accesses.md` §10.8 | "Exact hardware timeout/stage action of the auto-armed WDT: unknown" | MWDT0: 26,000,000 APB cycles, core reset, cause `0x07`; RWDT: 400,000 slow-clock cycles, system reset, cause `0x10`; proven absent from the ROM; both seen on real chips (§D.4, §H) |
| `esp32c3-rom-flash-boot.md` §H | real-chip behaviour known "second-hand" from search-engine summaries | first-hand logs in seven GitHub issues; `rst:0x10 (RTCWDT_RTC_RST)` also occurs in the loop; the `j .` hang gives `Saved PC:0x40047ed2` (§H.1) |
| `esp32c3-bootloader-accesses.md` §10.1 | CPU frequency during the ROM (20 or 40 MHz) "not established" | `TRM:` v1.4 §6.2.4.1: 20 MHz by default; a reset-loop log excludes 40 MHz for APB (§H.2). After an `esp_restart` the ROM runs at 40 MHz (`PRE_DIV_CNT` = 0 survives the CPU reset) |
| `esp32c3-boot-chain.md` §E.2 | RTC timer "ticks at the RTC slow clock (RC ≈ 150 kHz)" | 136 kHz nominal once the bootloader has set `SCK_DCAP` = 255 (`SOC_CLK_RC_SLOW_FREQ_APPROX`) |
| `esp32c3-boot-chain.md` §F.2 | "`RTCCALICFG2.TIMEOUT` as an escape" | also the exit of a first wait that runs when `START_CYCLING` is set, which is the reset state (§B.4 step 1); `RTCCALICFG2` resets to `0xFFFFFF98` |
| `esp32c3-bootloader-accesses.md` §6 | interrupt watchdog "prescaler 20000 (a 500 µs tick according to `int_wdt.c`)" | 500 µs because ESP-IDF clocks the MWDT from the 40 MHz crystal (`USE_XTAL` = 1), not from APB |
| `esp32c3-bootloader-accesses.md` §9, row 2 | a calibration stub with `RDY` = 1 and `SYSTIMER.UNIT0_OP` valid is enough up to the scheduler | still true; add that the tick ISR needs counter 1 to advance by one period per interrupt (§A.4), and that QEMU's own counter 1 starts 1.93 periods ahead (§G.9) |

## L. Artefacts and reproduction (scratch, not in the repository)

Directory `…/scratchpad/timers/`: `tools/` (scripts written for this report), `out/` (traces), `dl/` (downloads: TRM, QEMU sources at the tag, issue logs), `idf/` (ESP-IDF files copied out of the image).
Container: `docker run -d --name inkemu-timers --network none --entrypoint sleep -v …/oracle:/w:ro -v …/rom/x:/rom:ro -v …/t16/fw:/fw:ro -v …/timers/tools:/tools:ro -v …/timers/out:/out espressif/idf:v5.5.2 infinity` (third-party binaries read-only, only `/out` writable, no network).

| Step | Command |
|---|---|
| Trace M1 | container: `/tools/run_trace.sh /w/flash_hello.bin 7500000 /out/m1` (QEMU line of `esp32c3-bootloader-accesses.md` §0 piped into `tfilter.py`, which keeps the accesses to TIMG0/1, SYSTIMER, `RTC_CNTL`, `SYSTEM`, `INTERRUPT_CORE0`, `ASSIST_DEBUG`, the interrupts, and the UART lines with their instruction index) → `out/m1_events.txt` (114,361 events, sha256 `7d8c7140…1647a`), `out/m1_uart.txt` |
| Names | host: `python3 -I tools/svdnames.py third_party/espressif-svd/esp32c3.svd > out/names.tsv`; `python3 -I tools/ann.py out/m1_events.txt out/names.tsv out/m1_ann.txt <hello_world.nm> <bootloader.nm> <c3rev3.nm>` → `out/m1_ann.txt` (sha256 `4a9a62c1…7035`) |
| ROM proof (§D.4) | host: `python3 -I tools/romscan.py boot/romelf/dis_rev3.txt 6001f000 0 ff` (and `60020000 0 ff`, `60008000 90 b0`, `60023000 0 ff`); `python3 -I tools/elfwords.py rom/x/esp32c3_rev3_rom.elf 6001f000 60020fff` (and the two keys, and `40047c3a`, `400000a4`); `flashboot/tools/fn.sh disable_default_watchdog boot_prepare main` |
| Register summaries | host: `python3 -I tools/regsum.py idf/soc/esp32c3/register/soc/<block>_reg.h [regex]` |
| TRM text | host: `pdftotext -layout dl/trm/trm.pdf out/trm.txt` |
| Issue logs | host: `gh api repos/<owner>/<repo>/issues/<n>` and `…/comments` → `dl/issues/*.txt` |

Licence hygiene: QEMU sources (GPL) were read for behaviour only; no code or comment was copied, and the facts taken from them are restated in original wording. Register names, bit positions and reset values come from the Apache-2.0 ESP-IDF headers and the vendored SVD; ROM facts from the Apache-2.0 `esp-rom-elfs` ELF; the TRM is cited by section.

## M. Corrections found while planning SP1b-3a (2026-10-11)

- §B.3 says the configuration registers of a main watchdog take effect when `WDTCONFIG0.CONF_UPDATE_EN` is written. `_timer_ll_reset_register` (`IDF:hal/esp32c3/include/hal/timer_ll.h:50-66`) clears `FLASHBOOT_MOD_EN` with a plain read-modify-write right after a peripheral reset, without that bit, "to prevent the system from being reset unexpectedly": the flash-boot bit at least acts when it is written. The model applies every write at once and reads `CONF_UPDATE_EN` as 0; which registers really wait for the bit is open (list of SP1b-4).
- The SVD gives 0 as the reset value of `RTC_CNTL.WDTWPROTECT` and `RTC_CNTL.SWD_WPROTECT`. ESP-IDF's header gives the keys, `0x50D83AA1` and `0x8F1D312A` (`IDF:soc/esp32c3/register/soc/rtc_cntl_reg.h:1700`, `:1758`): both watchdogs are unlocked at reset, which is why the bootloader can stop the RTC watchdog. The model uses the header's values.
- §H.1 lists `0x4004c0d4` among the saved PCs of blank-chip reset loops. The model reproduces it: with the UART transmit time modelled, the watchdog catches the ROM in `uart_tx_one_char_uart.part.0`, waiting for room in the FIFO.
- The reference application reads `EFUSE + 0x18C` at every start (`esp_efuse_utility_check_errors`, the load at `0x4200045c`), a word between `RD_REPEAT_ERR3` and `RD_REPEAT_ERR4` that neither the SVD nor `efuse_reg.h` documents. The trace on QEMU did not show it as unmodelled because QEMU answers the whole block. The model answers 0.
- The golden log of `sp1b-test-oracle.md` §2 ends at `Minimum free heap size`: the capture was cut before the countdown. The life-cycle test of SP1b-3a pins the rest from the model, not from the oracle.
