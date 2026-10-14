# ESP32-C3 deep sleep and the RTC-timer wake: research for SP1b-3f

Date: 2026-10-14, branch `sp1b-3f-deep-sleep`. Two reports, kept whole: **Part A** reads ESP-IDF
v5.5.2 and the rev3 ROM register by register; **Part B** is the state of inkemu's model at
`ef83410`, what the real firmwares need, and what QEMU can and cannot compare. The design that rules
on them is `docs/superpowers/specs/2026-10-14-sp1b-3f-design.md` (decisions F1–F13); where a
decision departs from a recommendation below, the design and the plan
(`docs/superpowers/plans/2026-10-14-sp1b-3f-deep-sleep.md`) say so. Every reference to inkemu's code,
in both parts (`crates/…`, `rtc_cntl.rs`, `timg.rs` and their line numbers), is as of `ef83410`,
before this branch split `rtc_cntl.rs` into `rtc_cntl/{mod,timer,wdt,swd,sleep}.rs` and `timg.rs`
into `timg/{mod,calibration}.rs`.

# Part A — ESP-IDF v5.5.2 and the ROM, register by register

Tags: **M** = read in a file (path:line, ROM address or TRM section); **R** = reasoned.
Abbreviations (all ESP-IDF files fetched raw from `raw.githubusercontent.com/espressif/esp-idf/v5.5.2/components/`):
`SM` = `esp_hw_support/sleep_modes.c`; `RS` = `esp_hw_support/port/esp32c3/rtc_sleep.c`; `RT` = `esp_hw_support/port/esp32c3/rtc_time.c`;
`RH` = `esp_hw_support/port/esp32c3/include/soc/rtc.h`; `LL` = `hal/esp32c3/include/hal/rtc_cntl_ll.h`; `REG` = `soc/esp32c3/register/soc/rtc_cntl_reg.h`;
`SYS` = `soc/esp32c3/register/soc/system_reg.h`; `ROMH` = `esp_rom/esp32c3/include/esp32c3/rom/rtc.h`; `CAPS` = `soc/esp32c3/include/soc/soc_caps.h`.
`ROM3:sym@addr` = disassembly of `~/.local/share/inkemu/rom/esp32c3_rev3_rom.elf` (llvm-objdump). `TRM` = ESP32-C3 TRM v1.4 (PDF, text extraction), chapter numbers given.
RTC_CNTL base `0x6000_8000`, SYSTEM base `0x600C_0000` (M `soc/esp32c3/register/soc/reg_base.h:6,26`). Offsets below are from the SVD/header names in `REG`.

## 0. Which path a C3 takes (decides everything else)

- C3 caps: `SOC_PM_SUPPORT_DEEPSLEEP_CHECK_STUB_ONLY` absent, `SOC_RTC_FAST_MEM_SUPPORTED 1`, `SOC_PMU_SUPPORTED` absent, `SOC_LP_TIMER_SUPPORTED` absent (M `CAPS:39-40,371-373,440-452`); `ESP_ROM_SUPPORT_DEEP_SLEEP_WAKEUP_STUB 1` (M `esp_rom/esp32c3/esp_rom_caps.h:32`).
- `CONFIG_ESP_SYSTEM_ALLOW_RTC_FAST_MEM_AS_HEAP` defaults to **y** (M `esp_system/Kconfig:118-120`). With it, deep sleep ends in `rtc_deep_sleep_start()` (M `SM:886-895`). With it **n**, ESP-IDF calls ROM `set_rtc_memory_crc()` then `rtc_sleep_start(wakeup, reject, 0)` — the light-sleep function — with a deep-sleep configuration (M `SM:869-874,886-891`). **The emulator must accept both entry functions**; only the configuration (DG_WRAP_PD_EN, §4) distinguishes deep from light (R).
- Timer target goes through `rtc_hal_set_wakeup_timer` → `rtc_cntl_ll_set_wakeup_timer` (M `SM:1820-1822`, `hal/include/hal/rtc_hal.h:94`).

## 1. Call chain and every register touched

### 1.1 `esp_sleep_enable_timer_wakeup(us)` (M `SM:1750-1764`)
- Pure software: `wakeup_triggers |= RTC_TIMER_TRIG_EN (BIT3)`, `sleep_duration = us`. No register access (M). Rejects `us > ((2^48−1)/f)·10^6` with `f = esp_clk_tree_lp_slow_get_freq_hz(APPROX)` = 136 000 for RC_SLOW (M `SM:1752-1756`, `CAPS:61,372-373`, `soc/esp32c3/include/soc/clk_tree_defs.h:40`) → `ESP_ERR_INVALID_ARG`, ≈ 2.07·10^15 µs (R arithmetic).

### 1.2 `esp_deep_sleep_start()` → `deep_sleep_start(false)` (M `SM:1312-1318, 1210-1310`)
In order (ESP-IDF code order; `allow_sleep_rejection = false`, so `reject_triggers = 0`, M `SM:1314,1092`):
1. `esp_sync_timekeeping_timers()` (M `SM:1232`) — esp_timer/RTC bookkeeping (R: SYSTIMER reads, RTC time read; not chased).
2. `s_config.rtc_ticks_at_sleep_start = rtc_time_get()` (M `SM:1242`) — see §2: write `TIME_UPDATE` (0x0C) bit31, read `TIME_LOW0` (0x10), `TIME_HIGH0` (0x14). No busy loop (M `LL:74-80`).
3. Wake stub: `esp_get_deep_sleep_wake_stub()` reads **STORE6** (0xC0) and if not an executable pointer writes `STORE6 = &esp_wake_deep_sleep` (M `SM:1246-1248, 408-433`; `ROMH:64`). On C3 `esp_default_wake_deep_sleep` is `RTC_IRAM_ATTR` (in RTC fast RAM, 0x5000_0000–0x5000_1FFF) and its body is **empty** (only ESP32/S2 branches) (M `SM:437-467`; `soc/esp32c3/include/soc/soc.h:164-169`).
4. `get_power_down_flags()` — software only on C3 (M `SM:2577-2762`).
5. `sleep_low_power_clock_calibration(true)` → `rtc_clk_cal(RTC_CAL_RTC_MUX, 10)` (M `SM:811-833,155`): TIMG0 `RTCCALICFG`/`RTCCALICFG1`/`RTCCALICFG2` — busy-wait on `TIMG_RTC_CALI_RDY` or `TIMEOUT` (two loops: wait for a running cycling calibration, then for the one-off) (M `RT:36-129`). Result → `esp_clk_slowclk_cal_set()` → **STORE1** (0x54) (M `esp_hw_support/esp_clk.c` `esp_clk_slowclk_cal_set`, `hal/esp32c3/include/hal/clk_tree_ll.h:697-711`, `ROMH:59`). Already modelled by inkemu's TIMG (`crates/soc-esp32c3/src/periph/timg.rs:46-64` as of `ef83410`, now `timg/calibration.rs`).
6. `sleep_time_adjustment = DEEP_SLEEP_TIME_OVERHEAD_US = 250 + 100·240/CONFIG_ESP_DEFAULT_CPU_FREQ_MHZ` = **400 µs** at the default 160 MHz (650+… with an external 32 kHz crystal) (M `SM:213-217,1258`; `esp_system/port/soc/esp32c3/Kconfig.cpu:4,21`).
7. `force_pd_flags = PD_DIG | PD_VDDSDIO | PD_INT_8M | PD_XTAL | PD_WIFI | PD_BT` (M `SM:1270-1290`), `get_sleep_flags` adds `RTC_SLEEP_NO_ULTRA_LOW (BIT18)` unless ultra-low mode is requested (M `SM:2764-2790`).
8. `esp_sleep_start(flags, 0, DEEP, false)` (M `SM:980-1152`):
   - `flush_uarts()`: for each UART with clock on and out of reset, `esp_rom_output_tx_wait_idle(i)` — polls UART status until TX FIFO empty and TX idle (M `SM:611-618`; R for the exact UART fields, already answered by inkemu's UART).
   - Deep-sleep PHY hooks only if `CONFIG_ESP_PHY_ENABLED` (M `SM:995-1002`).
   - `esp_clk_utils_mspi_speed_mode_sync_before_cpu_freq_switching` (M `SM:1005-1008`) — C3 has no MSPI timing-tuning caps; R: no register effect (not verified).
   - `rtc_clk_cpu_freq_set_xtal_for_sleep()` → `rtc_clk_cpu_freq_set_xtal()` → CPU to XTAL (SYSTEM `SYSCLK_CONF` source/divider, `esp_rom_set_cpu_ticks_per_us`, APB freq to **STORE5**) **and BBPLL off**: `OPTIONS0 |= BB_I2C_FORCE_PD(6) | BBPLL_I2C_FORCE_PD(8) | BBPLL_FORCE_PD(10)` (M `SM:1019`; `esp_hw_support/port/esp32c3/rtc_clk.c:315-331,338-345`; `clk_tree_ll.h:76-80`). Note: this ignores the USB-Serial-JTAG BBPLL consumer count — the USB PHY clock dies before sleep (R).
   - GPIO wake prepare only if `RTC_GPIO_TRIG_EN` (§5) (M `SM:1036-1040`).
   - `misc_modules_sleep_prepare(…, deep)`: user deep-sleep hooks, then `sar_periph_ctrl_power_disable()` → `RTC_CNTL_SENSOR_CTRL` (0x11C) `FORCE_XPD_SAR[31:30] = 2` (M `SM:706-760`; `esp_hw_support/port/esp32c3/sar_periph_ctrl.c:45-50`; `hal/esp32c3/include/hal/sar_ctrl_ll.h:48-57`).
   - `rtc_sleep_get_default_config` + `rtc_sleep_init` + `rtc_sleep_low_init(cal, true)` (§1.3) (M `SM:1122-1127`).
   - `timer_wakeup_prepare(sleep_duration − 400)` (§1.4) (M `SM:1131-1135`).
   - `esp_sleep_start_safe` (M `SM:852-895`): `esp_sleep_isolate_digital_gpio()` returns at once unless deep-sleep pad hold is on (`DIG_ISO` (0x8C) `DG_PAD_FORCE_UNHOLD`(14)=0 and `DG_PAD_AUTOHOLD_EN`(11)=1) (M `esp_hw_support/sleep_gpio.c:131-140`; `hal/esp32c3/include/hal/gpio_ll.h:398-427`); then `rtc_deep_sleep_start(triggers, 0)` (§1.5).
   - Never returns; if it did, `ESP_INFINITE_LOOP()` (M `SM:1293-1306`).

### 1.3 `rtc_sleep_init` / `rtc_sleep_low_init` writes for the deep configuration (M `RS:63-244,247-252`)
Config: `deep_slp=1, wifi_pd_en=1, bt_pd_en=1, int_8m_pd_en=1, vddsdio_pd_en=1, xtal_fpu=0, deep_slp_reject=1, light_slp_reject=1, lslp_mem_inf_fpu=0`, `cpu_pd_en`/`dig_peri_pd_en` 0 by default (R: flags not set); regulator `rtc_regulator_fpu=1`, `dbg_atten_slp=15` (`NODROP` on chips < rev 0.3), `rtc_dbias_slp=25`, `bias_sleep_slp=1`, `pd_cur_slp=1` (M `RS:63-133,159-167`; `RH:111-128`).

| Register (offset) | Write | Source |
|---|---|---|
| `DIG_ISO` (0x8C) | clear `WIFI_FORCE_NOISO`(29), `BT_FORCE_NOISO`(23) | M `RS:176-186` |
| `DIG_PWC` (0x88) | clear `WIFI_FORCE_PU`(18), `BT_FORCE_PU`(12); set `WIFI_PD_EN`(30), `BT_PD_EN`(27); `CPU_TOP_PD_EN`(29) and `DG_PERI_PD_EN`(28) per config (0); **set `DG_WRAP_PD_EN`(31)**; set `LSLP_MEM_FORCE_PU`(4), `FASTMEM_FORCE_LPU`(16) | M `RS:176-197,211-213,222-224`; bits `REG:1328-1430` |
| `BIAS_CONF` (0x7C) | `DBG_ATTEN_MONITOR[25:22]=0`, `BIAS_SLEEP_MONITOR(17)=0`, `PD_CUR_MONITOR(15)=0`, `DBG_ATTEN_DEEP_SLP[21:18]=15`, `BIAS_SLEEP_DEEP_SLP(16)=1`, `PD_CUR_DEEP_SLP(14)=1` | M `RS:198-209`; `REG:1205-1235` |
| analog I2C (regi2c, via ROM `rom_i2c_writeReg_Mask`) | `I2C_DIG_REG.EXT_RTC_DREG_SLEEP=25`, `EXT_DIG_DREG_SLEEP=0`; `I2C_ULP.IR_FORCE_XPD_CK=0` | M `RS:204-205,212` |
| `ANA_CONF` (0x34) | clear `PLL_I2C_PU`(31), `CKGEN_I2C_PU`(30), `RFRX_PBUS_PU`(28), `TXRF_I2C_PU`(27) | M `RS:214-216`; `REG:383-401` |
| `OPTIONS0` (0x00) | clear `BB_I2C_FORCE_PU`(7); `XTL_FORCE_PU`(13)=0 | M `RS:217,243` |
| `RTC_CNTL_REG` (0x80) | `REGULATOR_FORCE_PU`(31)=1 | M `RS:226`; `REG:1278-1279` |
| `CLK_CONF` (0x70) | clear `CK8M_FORCE_PU`(26), `CK8M_FORCE_NOGATING`(16); `XTAL_GLOBAL_FORCE_NOGATING`(28)=0 | M `RS:227-233,244` |
| `SDIO_CONF` (0x78) | clear `SDIO_FORCE`(22); `SDIO_PD_EN`(21)=1 → **flash VDD_SDIO off in sleep** | M `RS:236-238` |
| `SLP_REJECT_CONF` (0x68) | `DEEP_SLP_REJECT_EN`(31)=1, `LIGHT_SLP_REJECT_EN`(30)=1 | M `RS:240-241` |
| `TIMER1` (0x1C) | `PLL_BUF_WAIT[31:24]=20`, `XTL_BUF_WAIT[23:14]=100`, `CK8M_WAIT[13:6]=20` (deep defaults) | M `RS:247-252`; `RH:91-93` |

`TIMER2..6`, `PWC` (0x84), `SYSTEM.MEM_PD_MASK` (0x600C_000C) and `SYSCON_FRONT_END_MEM_PD` are **not written** on the deep path (M: absent from `RS`/`SM` deep path; `rtc_sleep_pu` only runs when `lslp_mem_inf_fpu`, which is 0, `RS:173-175`).

### 1.4 `timer_wakeup_prepare` (M `SM:1801-1827`; `LL:18-25`)
- `ticks = rtc_time_us_to_slowclk(max(us − 400, 0), cal)`; `target = rtc_ticks_at_sleep_start + ticks` (step 2 above, **not** the time at the write).
- `SLP_TIMER0` (0x04) = `target & 0xFFFF_FFFF`; `SLP_TIMER1` (0x08) = `target >> 32` (field `SLP_VAL_HI[15:0]`); `INT_CLR` (0x4C) `|= MAIN_TIMER_INT_CLR`(10); `SLP_TIMER1 |= MAIN_TIMER_ALARM_EN`(16, documented WO) (M `LL:18-25`; `REG:166-177,755`; TRM §9.7 `RTC_CNTL_MAIN_TIMER_ALARM_EN … (WO)`).
- No lower bound is checked on C3 (the "too short → reject" check is under `SOC_LP_TIMER_SUPPORTED`) (M `SM:1810-1819`).

### 1.5 `rtc_deep_sleep_start(wakeup_opt, reject_opt)` (M `RS:280-355`)
1. `WAKEUP_STATE` (0x3C) `WAKEUP_ENA[31:15] = wakeup_opt` → timer-only is field value `0x8`, register bit 18 (M `RS:282`; `REG:554-559`).
2. `SLP_REJECT_CONF.SLEEP_REJECT_ENA[29:12] = reject_opt` (0 for `esp_deep_sleep_start`) (M `RS:283`; `REG:936-953`).
3. `INT_CLR |= SLP_REJECT_INT_CLR(1) | SLP_WAKEUP_INT_CLR(0)`; `STATE0` (0x18) `|= SLP_REJECT_CAUSE_CLR(1)` (M `RS:285-287`).
4. Inline asm, registers only (stack may be in RTC RAM) (M `RS:290-354`):
   - `SYSTEM_RTC_FASTMEM_CONFIG` (0x600C_0048) = `0x7FF << 20` (LEN=0x7FF, ADDR=0), then `|= RTC_MEM_CRC_START(8)`;
   - **poll** `RTC_MEM_CRC_FINISH` (bit 31, RO) until set; write back with START cleared;
   - read `SYSTEM_RTC_FASTMEM_CRC` (0x600C_004C) → write **STORE7** (0xC4, `RTC_MEMORY_CRC_REG`);
   - `STATE0 |= SLEEP_EN(31)` (read-modify-write);
   - **poll** `INT_RAW` (0x44) `& (SLP_REJECT_INT_RAW(1) | SLP_WAKEUP_INT_RAW(0))` forever ("never finishes if successful").
   - Field layout M `SYS:734-763`. (The asm shifts ADDR by `START_S` instead of `ADDR_S`; harmless because ADDR=0, M `RS:341-342`.)

### 1.6 Polled locations the emulator must answer during entry
TIMG0 `RTCCALICFG.RDY`/`RTCCALICFG2.TIMEOUT` (already modelled); UART status (TX FIFO count/idle) for `flush_uarts`; **SYSTEM 0x600C_0048 bit 31 `RTC_MEM_CRC_FINISH`** (new); **`INT_RAW` bits 1:0** (only if the model lets the CPU keep running after `SLEEP_EN`; a faithful model stops the CPU at the `SLEEP_EN` write instead). No EXTMEM, SPI-mem, GPIO or IO_MUX access on the default deep path (M: none in `SM`/`RS` deep path; GPIO/IO_MUX only with hold or GPIO wake, §5).

## 2. RTC slow clock and RTC timer
- `rtc_time_get()` = `TIME_UPDATE |= bit31` then `TIME_LOW0 | TIME_HIGH0[15:0] << 32` — no wait loop on C3 (M `RT:176-179`, `LL:74-80`). Counter is 48 bits, "incremented by 2 every 2 RTC_SLOW_CLK cycles", not writable, not reset by any reset or sleep except power-up (M `RH:490-497`; TRM §9.3.3 "any reset / sleep other than power-up reset will not stop or reset the RTC timer"). `TIME_LOW1/HIGH1` (0xDC/0xE0) hold the previous capture; triggers include `TIMER_SYS_RST` (end of a digital reset) and `TIMER_XTL_OFF` (state machine powers down) when enabled in `TIME_UPDATE` bits 29/28 (M TRM §9.3.3 Table 9.3-2; `REG:180-200`).
- Calibration value = period of one slow tick in µs, Q13.19 (`RTC_CLK_CAL_FRACT = 19`): `period = ((xtal_cycles << 19) + d/2 − 1) / d`, `d = xtal_MHz × slow_cycles` (M `RH:155`, `RT:146-160`). Stored in **STORE1** (M `ROMH:59`).
- Conversions: `us→ticks = (us << 19) / period` (with an overflow-safe split above 2^45 µs); `ticks→us = (ticks × period) >> 19` (M `RT:162-174`).
- Hence wake tick `= T0 + floor(((us − 400) << 19) / period)` with `T0` read at step 2; with inkemu's nominal 136 kHz (`rtc_cntl.rs:76` as of `ef83410`, now `rtc_cntl/mod.rs`, 17/20000 per 160 MHz cycle) the device sleeps ≈ `us − 400 µs` after `T0` (R). The hardware then spends the power-up waits of `TIMER1` (XTL 100, PLL 20, CK8M 20 slow cycles) before the CPU runs (R; the TRM gives no exact sum — open question).
- ESP-IDF keeps system time across deep sleep through RTC memory (`s_rtc_timer_retain_mem` in `.rtc_timer_data_in_rtc_mem`, checksum-protected), STORE1 and the RTC counter (M `esp_hw_support/esp_clk.c:44-66,96-150`), and boot time in STORE2/3 (M `ROMH:46-51`). All three must survive the wake (R).

## 3. The wake
### 3.1 Reset type and what survives
- Deep-sleep wake = reset code **0x05, "Core Reset: resets the whole digital system except RTC"** (M TRM §6.1 Table 6.1-1 and bullet list; `ROMH:81`; `soc_reset_reason_t RESET_REASON_CORE_DEEP_SLEEP`). Same class as `RTC_SW_SYS_RST` (0x03) and `TG0WDT_SYS_RST` (0x07) — i.e. inkemu's `system = false` path (`crates/soc-esp32c3/src/periph/rtc_cntl.rs:321-332` as of `ef83410`, now `rtc_cntl/mod.rs`) is the right one: STORE0–7 and RTC_CNTL configuration kept.
- In deep sleep only the PMU domain is on: RTC timer, RTC fast memory, always-on registers; CPU, digital system (incl. internal SRAM, ROM power), peripherals, XTAL, PLL, RC_FAST off (M TRM §9.4.1–9.4.2 Table 9.4-1, §9.6). So internal SRAM contents are lost on hardware (R from "digital system OFF"; ESP-IDF does not rely on them).
- Survive: RTC fast RAM `0x5000_0000–0x5000_1FFF` (8 KiB) (M TRM §9.6; `soc.h:164-169`), RTC timer (keeps counting through the sleep), STORE0–7, RTC_CNTL config incl. `WAKEUP_ENA`, `SLP_TIMER*`, `DIG_PWC`, `RTC_CNTL_PAD_HOLD` (R: RTC domain is not reset by a core reset).
- Watchdogs during the sleep: RWDT `WDTCONFIG0.WDT_PAUSE_IN_SLP`(9) defaults to 1 → paused (M `REG:1640`, TRM §9.7); TIMG watchdogs are in the powered-off digital domain (R); super watchdog: bootloader sets `SWD_AUTO_FEED_EN` (M `bootloader_support/src/esp32c3/bootloader_esp32c3.c:79-99,133`).
- After wake, ESP-IDF reads `RTC_CNTL_SLP_WAKEUP_CAUSE` (0xF8) `WAKEUP_CAUSE[16:0]`, bit 3 → `ESP_SLEEP_WAKEUP_TIMER` (M `SM:2275-2290`, `LL:82-85`, `REG:2171-2176`); hardware latches the waking source bit (R; TRM only says "Stores the wakeup cause (RO)", §9.7). It is consulted only when `RESET_STATE[5:0] == 5` (M `SM:2277`). `esp_reset_reason()` maps 5 → `ESP_RST_DEEPSLEEP` (M `esp_system/port/soc/esp32c3/reset_reason.c:36-37`).
- R: `STATE0.SLEEP_EN` self-clears and `INT_RAW.SLP_WAKEUP_INT_RAW` is set by the wake; neither is read by ESP-IDF after a deep-sleep wake (only by the light path, `RS:357-367`).

### 3.2 What the ROM does (M `ROM3:main@0x40047e3c`, `rtc_boot_control@0x40048b1c`, `cacl_rtc_memory_crc@0x40048ab2`)
1. `rtc_get_reset_reason(0)` = `RESET_STATE` (0x38) `& 0x3F` (`@0x40048a8e`).
2. `boot_prepare` prints the banner and `rst:0x%x (%s),boot:0x%x (%s)`; the name table at `0x3ff19da8` has entry 5 → string at `0x3ff1ba08` = **`DSLEEP`** (M: table dump of `.rodata`). With inkemu's strap: **`rst:0x5 (DSLEEP),boot:0x8 (SPI_FAST_FLASH_BOOT)`** (M format, README.md:24 for the strap). `Saved PC:` follows only if `ASSIST_DEBUG 0x600CE048 ≠ 0` (M `boot_prepare@0x40047dd4-0x40047de4`); R: it is 0 after a deep sleep because the CPU/digital domain lost power. No ROM line mentions "wake" (M: `strings` of the ROM ELF has no such format).
3. Reasons {1,3,7–13,15–23} go straight to flash boot; reason 5 alone, **if `GPIO_STRAP` (0x6000_4038) bit 3 is set**, calls `rtc_boot_control(&entry=0x3FCDEF18, 5)` (M `main@0x40047e56-0x40047e98`).
4. `rtc_boot_control`: `entry = STORE6`; if non-zero: `cacl_rtc_memory_crc(0, 2047)` — writes `SYSTEM_RTC_FASTMEM_CONFIG` ADDR=0 (bits 19:9) and LEN=2047 (bits 30:20), sets START(8), **busy-waits until bit 31 (FINISH)**, clears START, reads `SYSTEM_RTC_FASTMEM_CRC`; if `STORE7 ≠ crc` → `entry = 0`; then **`STORE6 = 0`** (M `@0x40048b1c-0x40048b56`, `@0x40048ab2-0x40048b00`).
5. If `entry ≠ 0`: `jalr entry` (executes from RTC fast RAM; the default ESP-IDF stub returns at once), `entry = 0`, then the normal SPI flash boot (`spi_flash_boot_attach`, `ets_run_flash_bootloader`) with the usual `SPIWP:`/`mode:`/`load:`/`entry` lines (M `main@0x40047e8e-0x40047eb2`). The TRM's description places the CRC check "after SPI boot and some initialization" (TRM §9.6 step 5); the ROM code does it before loading the bootloader (M) — follow the code.

### 3.3 Bootloader (2nd stage) on a deep-sleep wake
- Default config (`CONFIG_BOOTLOADER_SKIP_VALIDATE_IN_DEEP_SLEEP = n`, M `bootloader/Kconfig.projbuild:317-323`): full normal path, image validated as usual (M `bootloader/subproject/main/bootloader_start.c:40-62`).
- Differences: factory-reset/test GPIO checks are skipped (M `bootloader_start.c:88`); **RTC fast-memory segments (0x5000_0000–0x5000_2000) are not loaded** (M `bootloader_support/src/esp_image_format.c:884-919`), so their segment line ends with an empty suffix instead of `load`: `I (…) esp_image: segment N: paddr=… vaddr=50000000 size=…h (…) ` (M `esp_image_format.c:613-620`). Skipped lines are `ESP_LOGD` (invisible at default level).
- With `SKIP_VALIDATE_IN_DEEP_SLEEP = y`: `I (…) boot: Fast booting app from partition at offset 0x…` (M `bootloader_support/src/bootloader_utility.c:517-529`), on failure `E … Fast booting is not successful` + `I … Try to load an app as usual with all validations` (M `:548-549`).

### 3.4 Application start after the wake
- RTC `.bss` not cleared (M `esp_system/port/cpu_start.c:443-447`); `esp_deep_sleep_wakeup_io_reset()` releases GPIO wake pins' hold (M `cpu_start.c:787-791`, `sleep_gpio.c:188-218`); clock init recalibrates the slow clock as on any boot (M `esp_system/port/soc/esp32c3/clk.c:191-198`); `rtc_init` skips `cali_ocode` (only on power-on) (M `clk.c:63-70`). `esp_perip_clk_init` treats 5 like a full reset (not a CPU-only reset) (M `clk.c:217-226`).

## 4. Light sleep — how to recognise it at register level
- Same `STATE0.SLEEP_EN` write, but `rtc_sleep_init` with `deep_slp = 0`: **`DIG_PWC.DG_WRAP_PD_EN`(31) cleared**, `BIAS_CONF.DG_VDD_DRV_B_SLP_EN`(8)=1 and `DG_VDD_DRV_B_SLP[7:0]=254` (M `RS:218-221`; `RH:119`). TRM: DG_WRAP_PD_EN is what switches off the digital regulator in sleep (M TRM §9.3, `xpd_dig_reg`).
- Rule: **at the write that sets `SLEEP_EN`, `DG_WRAP_PD_EN = 1` ⇒ deep sleep; `= 0` ⇒ light sleep** (R from the two configurations; valid for both deep entry functions of §0).
- Corroborating differences (R, M sources): light uses `rtc_sleep_start` (no CRC, STORE7 untouched) (M `RS:257-276`); `TIMER1` = `PLL_BUF_WAIT 1`, `CK8M_WAIT 4`, `XTL_BUF_WAIT = us→ticks(250 µs)` (M `RS:247-252`, `RH:100-102`); `SLEEP_REJECT_ENA` usually non-zero (`allow_sleep_rejection=true` + modem triggers) (M `SM:1341,1092-1099`); `WAKEUP_ENA` may contain light-only bits UART0 0x40, UART1 0x80, Wi-Fi 0x20, BT 0x400 (M `RH:664-672`; TRM Table 9.4-2). Before `SLEEP_EN` light sleep also suspends UARTs (force XOFF, poll TX FSM), stops SYSTIMER counters and running TIMG watchdogs, suspends the cache (M `SM:569-589,626-645,1341`) — accesses the emulator must accept before refusing at `SLEEP_EN`. Automatic light sleep (`esp_pm`, tickless idle) uses the same `esp_light_sleep_inner` (R).
- `esp_deep_sleep_try_to_start()` is deep (DG_WRAP_PD_EN=1) but with `SLEEP_REJECT_ENA = triggers & RTC_SLEEP_REJECT_MASK` (M `SM:1320-1326,1092`, `RH:677-685`); hardware may then reject (INT_RAW bit 1, `SLP_REJECT_CAUSE` 0xF0) and the CPU continues, logging `E (…) sleep: Deep sleep request is rejected` (M `SM:1293-1300`).

## 5. GPIO deep-sleep wake (`esp_deep_sleep_enable_gpio_wakeup`) — registers only
- Valid pins GPIO0–5 only (M `CAPS:177-178`); level modes only (`GPIO_INTR_LOW_LEVEL`=4, `HIGH_LEVEL`=5) (M `esp_driver_gpio/src/gpio.c:1029-1049`).
- Enable (`gpio_ll_deepsleep_wakeup_enable`, M `gpio_ll.h:684-695`): `RTC_CNTL_GPIO_WAKEUP` (0x110) `GPIO_PIN_CLK_GATE`(7)=1; `EXT_WAKEUP_CONF` (0x64) `GPIO_WAKEUP_FILTER`(31)=1; `GPIO_PINn_WAKEUP_ENABLE` = bit (31−n); `GPIO_PINn_INT_TYPE[2:0]` at bit (23−3n) (M `REG:2343-2432,928-931`).
- At sleep (`gpio_deep_sleep_wakeup_prepare`, M `SM:2115-2140`): optional pull-up/down per mode, `gpio_hold_en(n)` → `RTC_CNTL_PAD_HOLD` (0xD0) bit n (M `gpio_ll.h:435-440`), then status clear: `GPIO_WAKEUP.GPIO_WAKEUP_STATUS_CLR`(6) set then cleared (M `LL:32-36`). `WAKEUP_ENA` gets `RTC_GPIO_TRIG_EN` = 0x4 (register bit 17) (M `SM:2174`, `RH:664`).
- After wake: `GPIO_WAKEUP_STATUS[5:0]` says which pin (M `LL:27-30`, `SM:2107-2113`); `esp_deep_sleep_wakeup_io_reset` disables the wake bit and releases hold (M `sleep_gpio.c:204-218`).
- Detection for a "later sub-project" refusal: `WAKEUP_ENA` field bit 2 set (or any `GPIO_PINn_WAKEUP_ENABLE`) at `SLEEP_EN` with DG_WRAP_PD_EN=1 (R). Relevant to the X4 (power button) (R).

## Minimal faithful model

**Implement**
1. RTC_CNTL storage for every register of §1.3–1.5 (read back what is written; `MAIN_TIMER_ALARM_EN` and `*_CLR` bits write-only); `WAKEUP_STATE`, `SLP_TIMER0/1`, `SLP_REJECT_CONF`, `SLP_WAKEUP_CAUSE` (RO), `SLP_REJECT_CAUSE` (RO), `SENSOR_CTRL`, `BIAS_CONF`, `ANA_CONF`, `SDIO_CONF`, `DIG_PWC`, `DIG_ISO`, `TIMER1`, `RTC_CNTL_REG`, `GPIO_WAKEUP`, `PAD_HOLD`, `EXT_WAKEUP_CONF` — all surviving a core reset.
2. SYSTEM `RTC_FASTMEM_CONFIG` (0x600C_0048) / `RTC_FASTMEM_CRC` (0x600C_004C): on START, compute a deterministic checksum of RTC fast RAM selected by ADDR/LEN, set FINISH (bit 31) — immediately or after a bounded delay. Both ESP-IDF and the ROM read the same unit, so any deterministic function of the same bytes keeps the stub check consistent (R); the real polynomial is unknown (open question).
3. At the write that sets `STATE0.SLEEP_EN` with `DIG_PWC.DG_WRAP_PD_EN = 1`, `SLEEP_REJECT_ENA = 0`, and `WAKEUP_ENA == 0x8` with `SLP_TIMER1.MAIN_TIMER_ALARM_EN` set: stop the CPU; advance device time (no instruction execution) until the RTC counter reaches the 48-bit target (immediately if already passed, R); then latch `SLP_WAKEUP_CAUSE = 0x8`, and perform a **core reset with cause 5** (`system = false`): RTC fast RAM, RTC timer, STORE0–7 and RTC_CNTL kept; CPU, digital peripherals, SYSTEM, GPIO/IO_MUX, TIMG, cache/MMU reset; RWDT paused during the sleep when `PAUSE_IN_SLP`; RTC timer captures per `TIME_UPDATE` triggers as for other resets. The real ROM, bootloader and app then produce §3.2–3.4 by themselves.
4. Instruction fetch from RTC fast RAM (the ROM jumps to STORE6 there).
5. inkemu's own diagnostic line for the sleep (requested duration, target tick, wake device time) — design choice, not device output.

**Refuse by name**
- `light sleep` — `SLEEP_EN` with `DG_WRAP_PD_EN = 0` (covers `esp_light_sleep_start` and `esp_pm` auto light sleep).
- `deep sleep with GPIO wake` — `WAKEUP_ENA` bit 2 (§5); also `XTAL32K-dead wake` (bit 12), and any other bit than 0x8.
- `deep sleep with sleep rejection` (`SLEEP_REJECT_ENA ≠ 0`, `esp_deep_sleep_try_to_start`) unless rejection is modelled.
- `deep sleep with no wake source` (`WAKEUP_ENA = 0`, or bit 3 without `MAIN_TIMER_ALARM_EN`): the device would sleep until power-cycle; end the run with a named reason rather than spin forever (R).

**Leave as storage (write/read back, no behaviour)**
Regulator/bias fields (`BIAS_CONF`, regi2c `EXT_*_DREG_SLEEP`, `RTC_CNTL_REG`), `ANA_CONF` power bits, `CLK_CONF` CK8M/XTAL gating, `OPTIONS0` force-PU/PD bits (except where inkemu already models clocks), `SDIO_CONF`, `DIG_ISO`, `DIG_PWC` bits other than DG_WRAP_PD_EN, `TIMER1` waits (unless used for wake latency), `SENSOR_CTRL.FORCE_XPD_SAR`, `SLP_REJECT_CONF` reject-enable bits, `SYSTEM.MEM_PD_MASK`.

## Open questions
1. CRC of RTC fast memory: polynomial/initial value, and whether LEN=2047 means 2048 words (8 KiB, inclusive) or bytes. Only matters if something other than the ROM/IDF pair compares STORE7 (R).
2. Wake latency between the RTC counter reaching the target and the first ROM instruction (TIMER1 waits, regulator ramp, `ck8m`/XTAL settling): not quantified in the TRM; ESP-IDF compensates 400 µs total (entry + exit).
3. Whether `Saved PC:` is printed after `rst:0x5` on hardware (R says no: ASSIST_DEBUG loses power) — a hardware capture settles it, as for the other core resets in `docs/research/esp32c3-interrupts-and-reset.md` §4.4.
4. Whether hardware re-arms the RTC watchdog's flash-boot stage on a deep-sleep wake (RTC domain not reset, but `WDT_FLASHBOOT_MOD_EN` is digital-reset-triggered on other resets).
5. Exact values of `STATE0.SLEEP_EN`, `INT_RAW.SLP_WAKEUP_INT_RAW`, `TIME_LOW1/HIGH1` and `MAIN_TIMER_INT_RAW` after the wake (ESP-IDF does not read them on this path).
6. Internal SRAM after wake: lost on hardware (digital domain off); inkemu may keep or poison it — poisoning would expose firmware relying on `.noinit` SRAM across deep sleep.
7. USB-Serial-JTAG: BBPLL is forced off before sleep (§1.2) and the digital domain powers down — the host sees a disconnect and a re-enumeration after the wake; how `--usb-host` should present that is a design question.
8. Whether CrossPoint's Arduino build keeps `CONFIG_ESP_SYSTEM_ALLOW_RTC_FAST_MEM_AS_HEAP=y` (rtc_deep_sleep_start path) or not (ROM `set_rtc_memory_crc` + `rtc_sleep_start` path); both must work (§0).

---

# Part B — the state of inkemu at ef83410, the real firmwares, the QEMU oracle


Tags: M = measured (file:line or URL read in this session), R = recalled / inferred, not verified.
Paths relative to the root of the repository. Nothing was edited or run.

## 1. Current model

### 1.1 RTC_CNTL (crates/soc-esp32c3/src/periph/rtc_cntl.rs, 1292 lines)
- Block doc: RTC domain; CPU and core reset keep it, system reset returns registers to reset values, only power-on stops the RTC timer (rtc_cntl.rs:6-8) M.
- RTC timer: `timer: Counter` at the slow clock, built `RtcCntl::new` rtc_cntl.rs:270-295 (`Counter::new(0, now, SLOW_CLOCK, true)` :278) M. Captured by `TIME_UPDATE` bit 31 only: write handler rtc_cntl.rs:589-594 (`self.time = [rtc_time(now), time[0]]`), `rtc_time()` :334, masks 48 bits (:38) M. TIME_LOW0/HIGH0/LOW1/HIGH1 read :537-540 M. Bits 29:27 of TIME_UPDATE (TIMER_SYS_RST, XTL_OFF, SYS_STALL) are silently dropped (only bit 31 tested) M. Tests: rtc_cntl.rs:781 (counts, two captures), :810 (48-bit wrap), :844 (survives every reset) M.
- Slow clock: `SLOW_CLOCK = Rate::ratio(17, 20_000)` = 136 kHz RC, nominal, ASSUMPTION marked, rtc_cntl.rs:70-78 M. `CLK_CONF` write with ANA_CLK_RTC_SEL != 0 raises `Unsupported::SlowClock` (rtc_cntl.rs:613-620; diag.rs:65) M: this is the existing "refused by name" pattern to copy for light sleep.
- Calibration answer: TIMG0 `RTCCALICFG*` (timg.rs:46-52, 135-289): RC_SLOW => 5000/17 crystal cycles per slow cycle (timg.rs:162); 1024 cycles => 301 176 crystal cycles, ready after 7.53 ms (test timg.rs:1266); RC_FAST/256 => 4096/7; sel 2 (32 kHz crystal) and 3 => TIMEOUT (timg.rs:171-176). The firmware then stores the period in `STORE1` (research esp32c3-timers-and-watchdogs.md:35, :190: `0x3AD2CD` Q13.19 us) M. The model's calibration is therefore exactly consistent with its RTC timer, so a timer wake of N s lands at N s of device time (+ ESP-IDF overhead terms) R.
- STORE0..7: `store: [u32; 8]` rtc_cntl.rs:142-153, 238; read :532-534; write :572-575 M. Cleared by `reset_registers` (:306) only for a SYSTEM-level reset (`reset()` :323-332 calls it only `if system`); ASSUMPTION marked :303-305 M. Core and CPU resets keep them. A deep-sleep reset is a CORE reset (TRM table: deep sleep 0x05 is in the core-reset row, esp32c3-interrupts-and-reset.md:365 M), so STORE survives wake with no new code.
- RESET_STATE: `reset_cause` (u32 code) + `reset_state_other`, read :536 M.
- Registers accepted-without-effect list: `ACCEPTED: [Accepted; 18]` rtc_cntl.rs:156-220 M. Other SVD registers of the block are NOT in it, hence "unmodelled": reads return the SVD reset value and a `UnmodeledMmio` diagnostic is recorded (bus.rs `unmodeled`, bus.rs:~165-180); `--strict-mmio` stops on them M. Unmodelled today, all needed by the sleep path (SVD addresses from crates/svd-regs/src/esp32c3.rs:306-380 M): `SLP_TIMER0` 0x60008004, `SLP_TIMER1` 0x60008008, `STATE0` 0x60008018 (SLEEP_EN, SLP_REJECT_CAUSE_CLR), `TIMER2`, `WAKEUP_STATE` 0x6000803c, `SLP_REJECT_CONF` 0x60008068, `SLP_REJECT_CAUSE` 0x600080f0, `SLP_WAKEUP_CAUSE` 0x600080f8, `GPIO_WAKEUP`, `EXT_WAKEUP_CONF`; `STORE6/7` are stored (loop :532) but not named. INT_RAW/ENA/ST/CLR are modelled (:40-45, 541-543, 598) but only bits 3 (WDT) and 15 (SWD) are produced (:49-50): bits 0 SLP_WAKEUP and 1 SLP_REJECT (SVD names SLP_WAKEUP_INT_RAW/SLP_REJECT_INT_RAW, esp32c3.svd:17870/17877) are not M.
- OPTIONS0: write :577-588; SW_SYS_RST (bit 31) => `ResetCause::SoftwareSystem`, SW_PROCPU_RST (bit 5) => `SoftwareCpu`; other bits stored M.
- Watchdog deadline pattern to copy: `Deadline` (ctx.rs:128-160, `arm`/`cancel`), `Event::RtcWatchdog` armed in `evaluate` rtc_cntl.rs:394-422, `ctx.request_reset(cause)` (ctx.rs:436) M.

### 1.2 TIMG (periph/timg.rs, 1562 lines)
T0, MWDT, calibration (above). `Timg::reset` :366. Nothing sleep related; the planned split (calibration out of timg.rs, timer/RTC-WDT/super-WDT out of rtc_cntl.rs) is carry-over item "Decouper timg.rs / rtc_cntl.rs" (tech_debt.md SP1b-3f section) M.

### 1.3 SYSTEM (periph/system.rs, 402 lines)
`ACCEPTED: [Accepted; 9]` system.rs:89-146; `MEM_PD_MASK` row system.rs:140-145 (comment "read by rtc_init" :88-89) M. `SYSTEM.RTC_FASTMEM_CONFIG` (0x600c0048) and `RTC_FASTMEM_CRC` (0x600c004c) exist in the SVD (esp32c3.rs:2275-2276) but are NOT modelled or accepted anywhere under crates/soc-esp32c3/src (grep empty) M. The ROM reads them on a deep-sleep boot (`rtc_boot_control@0x40048b1c`, `cacl_rtc_memory_crc@0x40048aae`; esp32c3-boot-chain.md:252, "out of scope for SP1b") M. And `rtc_deep_sleep_start` (ESP-IDF) writes `SYSTEM_RTC_FASTMEM_CONFIG_REG` and polls `SYSTEM_RTC_MEM_CRC_FINISH` before SLEEP_EN (rtc_sleep.c, WebFetch, v5.5.2) M. => SP1b-3f must model this CRC unit (algorithm: R, check svd description / ROM disassembly).

### 1.4 Machine loop and time (crates/soc-esp32c3/src/machine.rs, 3692 lines)
- `Esp32C3` struct :187-198; `run_within` :286-363 M. Device time advances per instruction by hart cycles (`ctx.advance` :343), and ONLY a `StepOutcome::Waiting` (WFI) skips time: `self.sleep(deadline)` :305, then `service` :312; returns `Asleep` after 1 s of device sleep or 100 000 sleeps (:319, consts :138, :143) M.
- `sleep()` :443-459: wake = min(`ctx.next_wake()`, interrupt latency end); `NothingDue` => `StopReason::Idle` (:307); deadline cut => `TimeBudget` (:308) M. Cycle counter does not move while asleep (comment :440-442).
- `service()` :391-406: `deliver_due`, `take_reset` => `self.reset(cause)`, clock, interrupts M.
- Resets: `Esp32C3::reset` :468-487 records `Diagnostic::Reset`, calls `bus.reset` (bus.rs:142-145) then `hart.reset(RESET_PC)`, clears `selected`, `resets += 1`; device time and memories untouched M. `SocBus::reset` -> `Peripherals::reset` (periph/mod.rs:569-581): CPU level: assist_debug.cpu_reset + `Sensitive::new()`; Core level adds `core_reset` (mod.rs:583-606: gpio, io_mux, intmatrix, system, extmem, mmu, spi0, spi1.reset, sha, apb_ctrl, ana_i2c, saradc, uart0/1, usb_serial_jtag, `clear_sources`, timg0/1, systimer; NOT rtc_cntl nor flash chip); System adds `assist_debug.system_reset`; always last `rtc_cntl.reset(ctx, cause, level==System)` M.
- ResetCause / levels: crates/soc-esp32c3/src/reset.rs: enum :41-61, `code()` :69-82, `level()` :84-96. No `DeepSleep` variant; code 0x05 absent (ROM main goes to `rtc_boot_control` for reason 5, esp32c3-interrupts-and-reset.md:390) M.
- Missing mechanism: a firmware in deep sleep does not WFI. ESP-IDF `rtc_sleep_start`/`rtc_deep_sleep_start` set SLEEP_EN then POLL `INT_RAW` ("never finishes if successful", rtc_sleep.c M). Without a model-side CPU stall, the 20 s would be executed as a busy loop of ~3.2 G instructions at 160 MHz (R; cf. OPTIONS0 "CPU stall" listed as not modelled, rtc_cntl.rs:160). The loop only skips time on `Waiting` (machine.rs:303). A new "stalled until reset" condition (ctx flag consumed by `run_within`, treated like `Waiting` but wake = the sleep deadline and the end = a reset) is needed.

### 1.5 RTC fast memory
Modelled as `Ram` at 0x5000_0000, 8 KiB (memmap.rs:30-33; bus.rs:24, 125; accessed bus.rs:357, 368) M. Survives every reset level, ASSUMPTION documented bus.rs:138-143; test bus.rs:1553-1567 M. SRAM is also kept across resets (machine.rs:462 doc), which is wrong for deep sleep on silicon (digital domain powered off, TRM) R, harmless for the example because the bootloader reloads everything.

## 2. Accepted-without-effect rows owned by SP1b-3f (exactly 10)
From docs/registers-accepted-without-effect.md:74-79, 84-86, 115 (also grep "SP1b-3f", 10 hits) M:
| Address | Register | Not modelled | Source list |
|---|---|---|---|
| 0x60008000 | RTC_CNTL.OPTIONS0 | BBPLL/crystal/I2C power forcing, CPU stall (the 2 sw reset bits act) | rtc_cntl.rs:157-162 |
| 0x6000801c | RTC_CNTL.TIMER1 | clock/PLL wake-up delays | :163-168 |
| 0x60008024 | RTC_CNTL.TIMER3 | Wi-Fi/BT power-up timing | :169 |
| 0x60008028 | RTC_CNTL.TIMER4 | digital power-up timing | :170 |
| 0x6000802c | RTC_CNTL.TIMER5 | minimum sleep time | :171 |
| 0x60008030 | RTC_CNTL.TIMER6 | peripheral power-up timing | :172-177 |
| 0x60008084 | RTC_CNTL.PWC | RTC power-domain control | :216 |
| 0x60008088 | RTC_CNTL.DIG_PWC | digital power-domain control | :197-202 |
| 0x6000808c | RTC_CNTL.DIG_ISO | digital pad isolation | :217 |
| 0x600c000c | SYSTEM.MEM_PD_MASK | memory power-down mask in light sleep | system.rs:140-145 |
Plus the non-table item: TIME_UPDATE bits 29:27 (tech_debt.md SP1b-3f; handler rtc_cntl.rs:589-594).

Generation: each peripheral exposes `pub(crate) fn accepted() -> Vec<Accepted>` over a `const ACCEPTED: [Accepted; N]` built with `Accepted::svd(periph, reg, why, heir)` (register in SVD, address resolved at compile time; periph/mod.rs:52-91) or `Accepted::at(periph, reg, addr, why, heir)` (register absent from the SVD; used in mod.rs:384-393 for radio/FE/NRX and in spi_mem/extmem). All 13 lists are concatenated and sorted by address in `accepted_without_effect()` periph/mod.rs:271-290; `render_accepted_table` mod.rs:294. rtc_cntl.rs and system.rs use only `Accepted::svd` (grep counts: rtc_cntl 18, system 9) M. Tests: `every_row_of_the_table_is_routed_to_a_model_that_accepts_a_write_there` (mod.rs:796-818, writes first and last register of each row through the real device tree, asserts no diagnostic) and crates/soc-esp32c3/tests/accepted_without_effect.rs (`the_documented_table_is_the_one_the_code_builds` :14-38 rewrites the table below `<!-- generated table -->` only when `INKEMU_BLESS=1`; `every_row_hands_its_register_to_a_plan_that_is_still_to_come` :52-62 with `PLANS_TO_COME` = SP1b-3f, SP1b-4, SP2, SP5a, SP5b, SP8 :50) M. Consequences for SP1b-3f: giving a row its effect means deleting it from the list (and the SVD name constant becomes a modelled register); when SP1b-3f's last row is gone, remove "SP1b-3f" from `PLANS_TO_COME` and update the prose "six heirs" (docs/registers-accepted-without-effect.md:34) and rerun `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect` (do not run it here).

## 3. What the real firmwares need
- CrossPoint 1.6.5 (source at HEAD, R, cr/src/main.cpp:431-655 quoted in docs/research/flash-writes-and-crosspoint-boot.md:479): at boot, wake reason and power-button hold check (power-on with GPIO20 low = power-button wake: goes back to deep sleep unless GPIO3 still held low; GPIO20 high: sleeps at once) M for the doc statement, R for the source. Wake source is GPIO3 LOW: `esp_deep_sleep_enable_gpio_wakeup(GPIO3, LOW)` (xteink-hardware.md:102, :193) M(doc). Power latch GPIO13 driven LOW for battery-only deep sleep (xteink-hardware.md:41). The bootloader shipped with CrossPoint skips image validation on deep-sleep wake (flash-writes doc:39, :331).
- Neither the doc nor tech_debt shows any use of `esp_sleep_enable_timer_wakeup` by CrossPoint or the stock firmware (grep of docs/research and notes for timer_wakeup/enable_timer: none) M(absence in docs), R(in binaries).
- CrossPoint's first sleep comes after its first SAR ADC read (SP2 hardware), so its sleep cannot be the proof (tech_debt.md SP1b-3f; flash-writes doc:492 item 8).
- Conclusion: the real firmwares use GPIO wake (GPIO3 low, `SOC_GPIO_SUPPORT_DEEPSLEEP_WAKEUP`) and need the deep-sleep entry/reset path, not the timer. The spec asks for the timer (§10.2 item 7, spec :301) because it is the one wake source that needs no pins. GPIO wake needs the SP2 GPIO model (input levels, `RTC_CNTL.GPIO_WAKEUP`, GPIO.PINn.WAKEUP_ENABLE) and should be named as a refusal/heir (SP2), like light sleep.

## 4. Reference-firmware candidates (ESP-IDF v5.5.2)
How apps are added: README "Adding a firmware" (tools/reference-firmware/README.md): build.sh -> in-container.sh `build_example <dir> <sums>` (in-container.sh:36-72). Unpinned project: build prints three hashes under `unpinned <file>:`, script stops, CI round 1 fails by design, copy hashes into `SHA256SUMS.inputs-<name>`, round 2 confirms. Build config = `/ref/sdkconfig.defaults` + the project's own sdkconfig.defaults (-D SDKCONFIG_DEFAULTS list; the project's file is never replaced; no third file slot). Application-only outputs are listed in the `for project in ...` loop (in-container.sh:~108-117) M.

### 4.1 examples/system/deep_sleep (v5.5.2; sources read via raw.githubusercontent M)
- Files: CMakeLists.txt, README.md, main/, partitions.csv, pytest_deep_sleep.py, sdkconfig.ci.basic, sdkconfig.ci.esp32_singlecore, sdkconfig.defaults. Its sdkconfig.defaults: `CONFIG_LIBC_TIME_SYSCALL_USE_RTC_HRT=y`, `CONFIG_RTC_CLK_SRC_INT_RC=y`, `CONFIG_BOOTLOADER_SKIP_VALIDATE_IN_DEEP_SLEEP=y`, `CONFIG_PARTITION_TABLE_CUSTOM=y`. partitions.csv: nvs 0x4000, otadata 0x2000, factory 600K, ota_0, ota_1 600K (the build would use its own table; the app is cut out anyway; a stock-X4 table via `--firmware` is fine) M.
- Kconfig.projbuild: `EXAMPLE_EXT0_WAKEUP` depends SOC_PM_SUPPORT_EXT0_WAKEUP, `EXAMPLE_EXT1_WAKEUP` depends SOC_PM_SUPPORT_EXT1_WAKEUP; both macros are ABSENT from esp32c3 soc_caps.h (read: only SOC_PM_SUPPORT_WIFI/BT_WAKEUP, CPU_PD ... exist; SOC_GPIO_SUPPORT_DEEPSLEEP_WAKEUP=1, SOC_RTC_FAST_MEM_SUPPORTED=1, SOC_RTC_SLOW_MEM_SUPPORTED absent) so those two are not offered on C3. BUT `EXAMPLE_GPIO_WAKEUP` (depends SOC_GPIO_SUPPORT_DEEPSLEEP_WAKEUP) defaults to `y`, pin `EXAMPLE_GPIO_WAKEUP_PIN` default 0 (range 0-5 on C3), `EXAMPLE_GPIO_WAKEUP_HIGH_LEVEL` default y M. => the unmodified example, built with the pinned reference config, also arms a GPIO0 high-level wake in addition to the timer. Turning it off needs `CONFIG_EXAMPLE_GPIO_WAKEUP=n`, which cannot be added without changing the build rule (no extra defaults file slot) or editing the example (README forbids: "never replaced"). With GPIO0 as a strapping pin and inkemu's pin model (GPIO.IN default levels, SP2) a spurious GPIO wake or an unmodelled GPIO_WAKEUP/PIN0 write is likely R.
- Source behaviour (main/deep_sleep_example_main.c, summarised by WebFetch, quotes exact) M: `app_main` calls `example_deep_sleep_register_rtc_timer_wakeup()` which sets `wakeup_time_sec = 20` and prints `"Enabling timer wakeup, %ds\n"` (=> `Enabling timer wakeup, 20s`), `esp_sleep_enable_timer_wakeup(us)`; then GPIO/EXT0/EXT1 registration under their CONFIG guards; then a task `deep_sleep_task`: prints per wake cause: timer `"Wake up from timer. Time spent in deep sleep: %dms\n"`; GPIO `"Wake up from GPIO %d\n"`-style or `"Wake up from GPIO\n"`; default (first boot) `"Not a deep sleep reset\n"`; then `vTaskDelay(1000 ms)`, `printf("Entering deep sleep\n")`, `gettimeofday(&sleep_enter_time)` into `RTC_SLOW_ATTR static struct timeval sleep_enter_time`, `esp_deep_sleep_start()`. NVS path (`Open NVS done`, `slp_enter_sec`) only when `SOC_RTC_FAST_MEM_SUPPORTED` is unset: not on C3. NO `RTC_DATA_ATTR`, no boot counter: it uses `RTC_SLOW_ATTR` (on C3 there is a single RTC memory, 0x50000000, R which section it lands in).
- Sleep time: 20 s. Upstream pytest (pytest_deep_sleep.py, targets include esp32c3) expects, in order: `boot: ESP-IDF`, `Enabling timer wakeup, 20s`, `Not a deep sleep reset`, `Entering deep sleep`, `boot: Fast booting app from partition` (needs its own bootloader with SKIP_VALIDATE_IN_DEEP_SLEEP; the inkemu-shipped bootloader validates on wake, so that line would differ; the shipped bootloader's lines are the ones inkemu pins), then regex `Wake up from timer. Time spent in deep sleep: 2\d{4}ms` (20000-29999 ms) and host-side 18-22 s M.
- Exact console lines to expect from the example on UART0 (default console), in order, each line ends with `\n`: life 1 `Enabling timer wakeup, 20s` / `Not a deep sleep reset` / `Entering deep sleep`; life 2 `Enabling timer wakeup, 20s` / `Wake up from timer. Time spent in deep sleep: <20000..29999>ms` / `Entering deep sleep`, repeating every ~21 s plus boot. (The order of the first two lines of life 1 is from the pytest and from `app_main` registering before the task runs: M for pytest, R for the exact interleaving.) The printed milliseconds in inkemu are the model's own value (boot time depends on the uncalibrated timing model, tests/ cannot compare to silicon): pin with the regex 2\d{4}, not a number, or pin the measured value after CI.
- ESP-IDF sleep path (read M): `esp_deep_sleep_start` -> `deep_sleep_start` -> `rtc_time_get()` (TIME_UPDATE + TIME_LOW0/HIGH0, matches the model), wake stub default `esp_wake_deep_sleep` stored via `RTC_ENTRY_ADDR_REG` (R: STORE6 on C3), `sleep_low_power_clock_calibration(true)` (TIMG0 RTCCALICFG, 10 cycles; modelled), `esp_sleep_start` -> `timer_wakeup_prepare()` -> `rtc_hal_set_wakeup_timer` (SLP_TIMER0/1, R), `rtc_sleep_init` writes OPTIONS0, TIMER1, DIG_PWC, DIG_ISO, ANA_CONF, CLK_CONF, SDIO_CONF, BIAS_CONF, SLP_REJECT_CONF, plus regi2c writes (I2C_ULP, I2C_DIG_REG; ana_i2c is accepted storage); `rtc_sleep_start`: WAKEUP_STATE.WAKEUP_ENA, SLP_REJECT_CONF.SLEEP_REJECT_ENA, INT_CLR, STATE0.SLP_REJECT_CAUSE_CLR, STATE0.SLEEP_EN, poll INT_RAW for SLP_REJECT|SLP_WAKEUP (rtc_sleep.c M). On wake: reset reason 0x05, ROM `rtc_boot_control` (STORE6/7 + SYSTEM RTC_FASTMEM CRC unit), bootloader (skips validation only if configured), app; `esp_sleep_get_wakeup_cause()` returns UNDEFINED unless `RESET_REASON_CORE_DEEP_SLEEP`, then reads `rtc_cntl_ll_get_wakeup_cause()` (SLP_WAKEUP_CAUSE or WAKEUP_STATE, R) and maps `RTC_TIMER_TRIG_EN` (sleep_modes.c M).

### 4.2 Our own tiny app (like adc_wait_panic / usb_serial_jtag_console: Apache-2.0, project dir in tools/reference-firmware/, sdkconfig.defaults of its own)
- Pros: no GPIO wake (no GPIO0 arming), no 600K x3 table, can count boots with `RTC_DATA_ATTR static uint32_t boots` (tests that RTC fast memory survives the deep-sleep reset and is NOT reloaded by the bootloader, bootloader_utility behaviour on deep-sleep wake: R), can use a short sleep (2 s instead of 20 s, cheaper test), can print the reset reason (`esp_reset_reason()` == `ESP_RST_DEEPSLEEP`, from RESET_STATE 5) and the wake cause, and can read `STORE` survival indirectly. Can refuse-by-name test: a variant (or the same app behind a flag) calling `esp_light_sleep_start()` to show the named refusal.
- Cons: not an Espressif-published example ("unmodified example" evidence value lost: fixed with the example as a second app). Pinning needs two CI rounds, as adc_wait_panic (carry-over note 3e).
- Hand-written lines to define (proposal): `deep-sleep boot %u: reset reason %d`, `wake cause %d`, `entering deep sleep for 2 s`.

## 5. QEMU oracle
- Pin: tools/conformance/versions.env pins only RISCV tests/arch-test/Sail/RISCOF and `ESP_IDF_VERSION=v5.5.2`; it does NOT pin QEMU. QEMU comes with the pinned espressif/idf image: `esp_develop_9.2.2_20250817` (docs/research/sp1b-test-oracle.md:37-39 M); newer 20260417 exists (:18).
- sp1b-test-oracle.md:208-209 M: "Not emulated: ... light/deep sleep (restart path via RTC SW reset works...)".
- espressif/qemu esp-develop hw/misc/esp32c3_rtc_cntl.c (WebFetch M): no sleep model. SLP_TIMER, STATE0/SLEEP_EN, WAKEUP_STATE, SLP_WAKEUP_CAUSE fall to `default:` (reads 0, writes ignored); TIME_UPDATE and the RTC timer are not implemented at all (no timer state: TIME_LOW0 reads 0); RESET_STATE returns `s->reason`; OPTIONS0 SW_SYS_RST/SW_PROCPU_RST raise reset requests; STORE0..7 are scratch; "Other registers are not supported yet" warnings compiled out (RTCCNTL_WARNING 0, silent). Writing SLEEP_EN does nothing, so a deep-sleep firmware polling INT_RAW for SLP_WAKEUP would spin forever in QEMU.
- Can compare: nothing past the point where the firmware writes SLEEP_EN: only life 1 up to "Entering deep sleep" (and the ESP-IDF calibration up to rtc_sleep_init, with QEMU's loose clock regs). Cannot compare: sleep entry, wake time, reset cause 0x05, ROM wake path, CRC unit, TIME registers (QEMU reads 0 so `Time spent in deep sleep` never works there). Oracle for wake is the TRM, the ROM/ESP-IDF sources and a real unit (SP8 hardware).

## 6. CI cost
- Job `reference-boot` timeout 45 min (.github/workflows/ci.yml:103; notes), frees disk (ESP-IDF image 12.5 GB extracted), cache keyed by hash of tools/reference-firmware, Dockerfile, versions.env and x4-stock.bin (integrations.md memory, carry-over 3e) M.
- Per firmware build in the shared container: 34 s for hello_world, +32 s for the NVS example (integrations.md:26 M); `build_example` prints "built and checked in N s" in the job log (in-container.sh:70). So +30 to 40 s per added app. Add one extra CI round for pinning (first run fails by design). Test run cost: the 3e CI shows `real_firmware`-class runs of minutes locally; the 20 s sleep itself costs nothing if the sleep is skipped (device time jumps), plus the boot of two lives under `--release`; hello_world-class tests are already counted in the 13 reference_boot tests (effectifs 19, 28, 13, 10, 5). A new ignored test changes one of these expected counts (tools/ci/run-ignored-tests.sh pins them) R.

## Gaps and risks (for the design)
1. CPU stall during sleep (see 1.4): needed before the 20 s example can run.
2. SYSTEM RTC_FASTMEM CRC engine: unmodelled, hit by ROM on every deep-sleep boot and by `rtc_deep_sleep_start`.
3. New `ResetCause::DeepSleep` (code 0x05, level Core).
4. SLP_TIMER0/1, STATE0, WAKEUP_STATE, SLP_REJECT_CONF, SLP_WAKEUP_CAUSE/REJECT_CAUSE and INT_RAW bits 0/1 must become modelled registers; TIMER2 and GPIO_WAKEUP/EXT_WAKEUP_CONF decisions (accept vs refuse GPIO wake).
5. SRAM is kept across a deep-sleep reset (silicon loses it): decide whether to scramble/clear SRAM at deep-sleep wake (bootloader reloads everything; CrossPoint relies on RTC data only) R.
6. Time-keeping across sleep: ESP-IDF restores time from RTC timer + STORE1 cal (+ STORE2/3 boot time, research timers doc :378); SYSTIMER restarts at 0 on the core reset; consistent with the model if the RTC timer keeps counting during the skip.
7. A GPIO-wake alone is not required by this plan, but the unmodified `deep_sleep` example arms GPIO0 (4.1).

## Recommendation
Do both, in this order, in SP1b-3f:
1. Primary CI proof = our own tiny app `tools/reference-firmware/deep_sleep_timer/` (Apache-2.0): `RTC_DATA_ATTR` boot counter, `esp_sleep_enable_timer_wakeup(2 s)`, prints reset reason and wake cause, then `esp_deep_sleep_start()`; sdkconfig.defaults like the others, pinned in two CI rounds. It proves the whole chain without GPIO wake, checks the RTC fast memory survives, the 0x05 reset cause, STORE/RTC timer survival, and is cheap (2 s).
2. Second app, the unmodified `examples/system/deep_sleep` with its 20 s timer, only if the GPIO0 wake can be disabled by a third `SDKCONFIG_DEFAULTS` file in `build_example` for that project (`CONFIG_EXAMPLE_GPIO_WAKEUP=n`) or if SP1b-3f models GPIO.PIN0 wake as "armed, never raised" (levels stay low). Expected lines: `Enabling timer wakeup, 20s`, `Not a deep sleep reset`, `Entering deep sleep`, then `Enabling timer wakeup, 20s`, `Wake up from timer. Time spent in deep sleep: 2xxxxms` (regex 2\d{4}), `Entering deep sleep`. Run with `--console uart0 --max-time 30`.
3. Model, in order: `ResetCause::DeepSleep` (0x05, Core); registers listed in Gaps 4; wake timer as a `Deadline` armed on SLEEP_EN from SLP_TIMER0/1 compared to the RTC counter; a CPU-stall state in `run_within` (skip time to the deadline, then `request_reset(DeepSleep)`); the SYSTEM RTC_FASTMEM CRC unit; remove the 10 table rows (or keep OPTIONS0/TIMER*/PWC/DIG_*/MEM_PD_MASK as accepted with the heir changed to SP8 if only their wake-up effect is irrelevant: the table test requires every heir to be in `PLANS_TO_COME`); name `Unsupported::LightSleep` (SP1b-3f, like `SlowClock`, diag.rs:65) for light sleep (`STATE0.SLEEP_EN` without the deep-sleep configuration, `esp_light_sleep_start`, in `rtc_sleep_start`) and name GPIO wake as SP2.
4. Do not use QEMU as an oracle for sleep; the oracle is the TRM + ESP-IDF sources + a real unit (SP8). Compare with QEMU only up to `Entering deep sleep`.
5. Do the pure move splits of timg.rs / rtc_cntl.rs first, in their own commit (no behaviour change), then the sleep code, as tech_debt.md asks.
