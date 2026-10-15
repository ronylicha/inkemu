# The SoC peripherals of the X4 (GPIO matrix, IO MUX, GP-SPI2, I2C0, SAR ADC): research for SP2b

Date: 2026-10-15, branch `worktree-agent-a3bea5e036a7b3a47`, from `main` at `194f286`. The design that rules
on this report is `docs/superpowers/specs/2026-10-15-sp2b-soc-peripherals-design.md` (decisions 2b-D1 to
2b-D14); the plan is `docs/superpowers/plans/2026-10-15-sp2b-soc-peripherals.md` (rules 2b-R1 onward).

Tags: **M** = read in a file (path:line, TRM section) or measured; **R** = reasoned.
Abbreviations: `TRM` = ESP32-C3 Technical Reference Manual v1.4 (PDF, text extraction), section numbers given.
ESP-IDF files are those of tag **v5.5.2**, fetched raw from `raw.githubusercontent.com/espressif/esp-idf/v5.5.2/components/`:
`GLL` = `hal/esp32c3/include/hal/gpio_ll.h`; `SIG` = `soc/esp32c3/include/soc/gpio_sig_map.h`;
`IOM` = `soc/esp32c3/register/soc/io_mux_reg.h`; `GREG` = `soc/esp32c3/register/soc/gpio_reg.h`;
`SLL` = `hal/esp32c3/include/hal/spi_ll.h`; `ILL` = `hal/esp32c3/include/hal/i2c_ll.h`;
`IM` = `esp_driver_i2c/i2c_master.c`; `ALL` = `hal/esp32c3/include/hal/adc_ll.h`;
`AOH` = `hal/adc_oneshot_hal.c`; `ASH` = `esp_hw_support/adc_share_hw_ctrl.c`;
`ERC` = `efuse/esp32c3/esp_efuse_rtc_calib.c`; `ETB` = `efuse/esp32c3/esp_efuse_table.csv`;
`ACF` = `esp_adc/adc_cali_curve_fitting.c`; `ACC` = `esp_adc/esp32c3/curve_fitting_coefficients.c`;
`CAPS` = `soc/esp32c3/include/soc/soc_caps.h`; `SYSR` = `soc/esp32c3/register/soc/system_reg.h`.
`SVD` = `third_party/espressif-svd/esp32c3.svd` (fields), `svd-regs` (addresses and reset values).
`FI` = FreeInk SDK (the hardware layer of CrossPoint 1.6.5), `libs/…` at its `HEAD`, read as data;
`CP` = `crosspoint-reader` at its `HEAD`. Neither is the exact source of the 1.6.5 binary: what the binary does
is what Part A measures.

## Part A — What CrossPoint 1.6.5 touches (measured)

Run on 2026-10-15 at `194f286`, release build, `inkemu run --device x4 --max-time 5 --firmware
crosspoint-1.6.5-x3-x4.bin` (the user's file, SHA-256 pinned in `crates/emu-cli/tests/real_firmware.rs`),
154 s on the host. The diagnostics list each register once per direction (M):

| Block | Registers reported unmodelled | Order of first access |
|---|---|---|
| `APB_SARADC` | `APB_TSENS_CTRL`, `TSENS_CTRL2` (read, write back), then `ARB_CTRL`, `INT_CLR`, `ONETIME_SAMPLE`, `INT_RAW` | first of all (`pc 0x4214917c`), then at the wait |
| `GPIO` | `PIN0..PIN13, PIN20, PIN21`, `FUNCn_OUT_SEL_CFG` (n = 0..8, 10, 12, 13, 20, 21), `FUNC53/54/64_IN_SEL_CFG`, `ENABLE_W1TS/W1TC`, `OUT_W1TS/W1TC`, `STATUS_W1TC`, `IN` | `PIN13` (the power latch) second |
| `I2C0` | every configuration register, `COMD0..COMD6`, `CTR`, `DATA`, `FIFO_CONF`, `INT_ENA`, `INT_CLR`, `SR`, `TO` | during the X3/X4 probe |
| `SPI2` | `CMD`, `CTRL`, `CLOCK`, `USER`, `USER1`, `MS_DLEN`, `MISC`, `SLAVE`, `DMA_CONF`, `CLK_GATE`, `W0..W15` | after the probe |
| `DMA` (GDMA, `0x6003_F000`) | **none** | — |

Console (M, the panic's stack dump left out): the first life probes I2C twice, each read failing with
`i2c_master_transmit_receive failed: [259] ESP_ERR_INVALID_STATE` (`esp32-hal-i2c-ng.c:372`) after the 6 ms
timeout of `Wire.setTimeOut(6)` (no I2C interrupt ever comes); `Xteink probe scores: pass1=0 pass2=0 verdict=0`;
the display-bus probe reads `VER=00 00 00 00 00 FLG=00` (`GPIO.IN` reads 0); then the interrupt watchdog panics
on the wait for `APB_SARADC.INT_RAW` bit 31 with interrupts masked. The second life reads the X4 verdict back from
NVS, says `SD card not detected (err=0x01 … cs=12 sclk=8 miso=7 mosi=10 clk=40000000Hz)`, sends the SSD1677 its
first commands (`CMD_SOFT_RESET (0 ms)`: BUSY reads low) and panics on the same ADC wait.

What it means (R, with the sources below): CrossPoint drives GPIO through Arduino (`pinMode`, `digitalWrite`,
`digitalRead`: `GPIO.PINn`, `FUNCn_OUT_SEL_CFG`, `OUT_W1TS/W1TC`, `ENABLE_W1TS/W1TC`, `IN`); its display bus and
SD card go through Arduino's `SPIClass` on GP-SPI2 in **CPU mode** (no access to GDMA; FI
`libs/display/FreeInkDisplay/src/bus/EpdBus.cpp:179` `SPI.begin(sclk, miso, mosi, cs)`); CS and DC are plain
GPIOs it writes itself (`EpdBus.cpp:84`, `digitalWrite`); the X3/X4 probe is Arduino `Wire` on SDA = GPIO20,
SCL = GPIO0, 400 kHz, two passes over 0x55, 0x68, 0x6B, 0x6A, a chip counted only with a plausible value
(FI `libs/hardware/XteinkDetect/src/XteinkDetect.cpp:577-666`); the buttons and the battery are `analogRead` and
`analogReadMilliVolts` (FI `libs/hardware/InputManager/src/InputManager.cpp:165-216`,
`BatteryMonitor.cpp:724-743`), the ADC one-shot driver of ESP-IDF underneath. FI's `EpdBus` waits for BUSY with a
GPIO interrupt on **any edge** around a refresh (`EpdBus.cpp:437-470`, `attachInterrupt(…, CHANGE)`), and polls it
elsewhere. The display probe bit-bangs the display bus with GPIOs (`XteinkDetect.cpp:52-91`): pull-up input on
MOSI, so a panel that does not answer reads `0xFF`.

## Part B — GPIO matrix and IO MUX

**Blocks.** GPIO at `0x6000_4000`, IO MUX at `0x6000_9000` (`reg_base.h`; `svd-regs`). 22 pads, GPIO0–21 (M
`CAPS:160` `SOC_GPIO_PIN_COUNT 22`).

**GPIO registers** (M `GREG`, `SVD`): `OUT` (`+0x04`, bits 21:0 the output levels; 25:22 invalid, TRM §5.5.3),
`OUT_W1TS`/`OUT_W1TC` (`+0x08`, `+0x0C`), `ENABLE` (`+0x20`) and its `W1TS`/`W1TC` (`+0x24`, `+0x28`), `STRAP`
(`+0x38`), `IN` (`+0x3C`, read-only), `STATUS` (`+0x44`) and its `W1TS`/`W1TC` (`+0x48`, `+0x4C`), `PCPU_INT`
(`+0x5C`, read-only), `PCPU_NMI_INT` (`+0x60`), `CPUSDIO_INT` (`+0x64`), `PINn` (`+0x74 + 4n`), `STATUS_NEXT`
(`+0x14C`), `FUNCm_IN_SEL_CFG` (`+0x154 + 4m`, m = 0..127), `FUNCn_OUT_SEL_CFG` (`+0x554 + 4n`, n = 0..21),
`CLOCK_GATE` (`+0x62C`), `DATE` (`+0x6FC`).

- `PINn` fields (M `GREG:170-206`): `SYNC2_BYPASS` [1:0], `PAD_DRIVER` [2] (open drain), `SYNC1_BYPASS` [4:3],
  `INT_TYPE` [9:7], `WAKEUP_ENABLE` [10], `CONFIG` [12:11], `INT_ENA` [17:13]. `INT_TYPE`: 0 off, 1 rising, 2
  falling, 3 any edge, 4 low level, 5 high level (M TRM register `GPIO_PINn_REG`, `GLL:99-101`). ESP-IDF enables
  the CPU interrupt with `INT_ENA = 1` (bit 13; M `GLL:35,165` `GPIO_LL_PRO_CPU_INTR_ENA BIT(0)`).
- `FUNCm_IN_SEL_CFG` (M `GREG:1325-1340`): `IN_SEL` [4:0], `IN_INV_SEL` [5], `SEL` [6] (1 = through the matrix).
  `IN_SEL` 0x1E reads a constant 1, 0x1F a constant 0 (M `soc/esp32c3/include/soc/gpio_pins.h:13-14`; TRM §5.4.3
  note 3).
- `FUNCn_OUT_SEL_CFG` (M `GREG:3888-3906`): `OUT_SEL` [7:0] (128 = the `OUT` bit, TRM §5.5.3; `SIG:176`
  `SIG_GPIO_OUT_IDX`), `INV_SEL` [8], `OEN_SEL` [9] (1: output enable from `ENABLE`; 0: from the peripheral),
  `OEN_INV_SEL` [10] (TRM §5.11).

**IO MUX pad registers** `IO_MUX_GPIOn` (`+0x04 + 4n`; M `IOM:16-71`): `MCU_OE` [0], `SLP_SEL` [1], `MCU_WPD`
[2], `MCU_WPU` [3], `MCU_IE` [4], `MCU_DRV` [6:5], `FUN_WPD` [7], `FUN_WPU` [8], `FUN_IE` [9], `FUN_DRV` [11:10],
`MCU_SEL` [14:12], `FILTER_EN` [15]. Function 1 is GPIO on every pad (M `IOM:120` `PIN_FUNC_GPIO 1`; TRM §5.5.2).
Function 0 is GPIO too on GPIO0–3 and GPIO8–11, and on GPIO18–19 (USB pins); it is JTAG on GPIO4–7, the flash SPI
on GPIO12–17 and UART0 on GPIO20 (U0RXD) and GPIO21 (U0TXD). Function 2 is FSPI on GPIO2 (FSPIQ), GPIO4 (FSPIHD),
GPIO5 (FSPIWP), GPIO6 (FSPICLK), GPIO7 (FSPID), GPIO10 (FSPICS0) (M TRM table 5.12-1). The SVD gives every pad the
reset value `0xB00`: input enabled, pull-up on, drive strength 2 (M `svd-regs`); table 5.12-1 lists other reset
states for some pads (no pull-up on GPIO2–8 and GPIO10, no input on GPIO0, 1, 11, 18, 19). The model keeps the SVD's
value, which the lockstep's row 11 already compares (an `ASSUMPTION:`, 2b-D4).

**Output path** (M TRM §5.5.2): pad n is driven when its IO MUX function is GPIO and the matrix output is enabled:
`OEN_SEL = 1` takes `ENABLE[n]`, `OEN_SEL = 0` the peripheral's own output enable; `OUT_SEL` picks the level
(`OUT[n]` for 128); `INV_SEL` and `OEN_INV_SEL` invert. Open drain (`PAD_DRIVER`) drives low only. Signal indexes
used here (M `SIG`): U0TXD 6, I2CEXT0_SCL 53, I2CEXT0_SDA 54, FSPICLK 63, FSPIQ 64, FSPID 65, FSPIHD 66, FSPIWP 67,
FSPICS0..5 68..73.

**Input path** (M TRM §5.4): `IN[n]` holds the pad level when `FUN_IE` is set; ESP-IDF says it reads 0 otherwise
(M `GLL:322-334`, `gpio_ll_get_level`). A peripheral input signal m comes from pad `IN_SEL` when `SEL` is set,
else from the pad its IO MUX function connects directly. Synchronisation (`SYNC1/2_BYPASS`) and the glitch filter
(`FILTER_EN`) delay a signal by two APB cycles at most (M TRM §5.4.2, figure 5.4-2) — below what any modelled
device can see (R).

**Interrupts.** Source 16, `GPIO_PROCPU_INTR`, map register `INTERRUPT_CORE0_GPIO_INTERRUPT_PRO_MAP` (M TRM
table 8.3-1). `STATUS` holds a bit per pad, set by the pad's trigger and cleared by `STATUS_W1TC`; `PCPU_INT` is
`STATUS` masked by the pads whose `INT_ENA` bit 0 is set (M `GLL:112-114`, `gpio_ll_get_intr_status` reads it).
The TRM does not say whether a level trigger sets `STATUS` once or keeps setting it while the level holds
(open question 1).

**Pad hold and sleep** (M TRM §5.8, §5.9): `RTC_CNTL.PAD_HOLD` (GPIO0–5) and `DIG_PAD_HOLD` (GPIO6–21) latch a
pad; `SLP_SEL` switches a pad to its `MCU_*` bits in light sleep. CrossPoint releases the hold of the panel's RST
before its probe (`XteinkDetect.cpp:141-147`, `gpio_hold_dis`).

## Part C — GP-SPI2 in master mode

Block at `0x6002_4000` (`svd-regs`, `SPI2`). Source 19, `GPSPI2_INTR_2`, map register `SPI_INTR_2_MAP` (M TRM
table 8.3-1). Module clock and reset: `SYSTEM.PERIP_CLK_EN0` bit 6, `PERIP_RST_EN0` bit 6 `SYSTEM_SPI2_RST`
(M `SYSR:227-229,471-473`).

**Fields** (M `SVD`): `CMD` `UPDATE` [23] (applies the configuration, cleared by hardware; ESP-IDF spins on it,
M `SLL:264-267`), `USR` [24] (starts a transaction, cleared at its end). `USER`: `DOUTDIN` [0] (full duplex),
`CS_HOLD` [6], `CS_SETUP` [7], `CK_OUT_EDGE` [9], `FWRITE_DUAL/QUAD` [12/13], `USR_CONF_NXT` [15], `SIO` [17],
`USR_MISO_HIGHPART` [24], `USR_MOSI_HIGHPART` [25], `USR_MOSI` [27], `USR_MISO` [28], `USR_DUMMY` [29],
`USR_ADDR` [30], `USR_COMMAND` [31]. `USER1`: `USR_DUMMY_CYCLELEN` [7:0], `USR_ADDR_BITLEN` [31:27]. `USER2`:
`USR_COMMAND_VALUE` [15:0], `USR_COMMAND_BITLEN` [31:28]. `MS_DLEN` [17:0] (data bits − 1). `CTRL`: `FREAD_DUAL/QUAD`
[14/15], `FADDR_*`, `FCMD_*`, `RD_BIT_ORDER` [25], `WR_BIT_ORDER` [26]. `MISC`: `CS0_DIS..CS5_DIS` [5:0] (reset
`0x3E`: CS0 only), `MASTER_CS_POL` [12:7], `CK_IDLE_EDGE` [29], `CS_KEEP_ACTIVE` [30]. `CLOCK`: `CLKCNT_L/H/N`,
`CLKDIV_PRE` [21:18], `CLK_EQU_SYSCLK` [31]. `CLK_GATE`: `CLK_EN` [0], `MST_CLK_ACTIVE` [1], `MST_CLK_SEL` [2].
`DMA_CONF`: `DMA_RX_ENA` [27], `DMA_TX_ENA` [28], afifo resets [31:29]. `DMA_INT_RAW/ENA/CLR/ST`: `TRANS_DONE` [12].
`SLAVE`: `MODE` [26] (slave), `USR_CONF` [28] (segmented). `W0..W15`: the 64-byte buffer.

**Transaction** (M TRM §27.5.8.1–27.5.8.4): states PREP (CS setup), CMD, ADDR, DUMMY, DOUT, DIN, DONE (CS hold),
each enabled by its `USR_*` bit. The data length is `MS_DATA_BITLEN + 1` bits, at most 512 in CPU mode. In
CPU-controlled mode the output data is `W0..W15`, byte 0 first, MSB first unless `WR_BIT_ORDER`; received data
lands in the same buffer (M TRM §27.5.8.2). Full duplex (`DOUTDIN`) is 1-bit only and exchanges the data phase
both ways at once (M TRM §27.5.8.3). Half duplex: CMD + [ADDR] + [DUMMY] + [DOUT or DIN] (M TRM §27.5.8.4). The
command value is sent from `USR_COMMAND_VALUE[7:0]` first, then `[15:8]`; the address from
`USR_ADDR_VALUE[31:24]` down (M TRM §27.5.8.2, the bit-order rules). ESP-IDF's `spi_ll_usr_is_done` reads
`DMA_INT_RAW.TRANS_DONE` (M `SLL:277-279`); Arduino's HAL spins on `CMD.USR` (R, `esp32-hal-spi.c` of
Arduino-ESP32 3.x, `while (spi->dev->cmd.usr)`).

**Clock** (M `SLL:740-830`, `spi_ll_master_cal_clock`): `f = f_src / ((CLKDIV_PRE + 1) × (CLKCNT_N + 1))`, or
`f_src` when `CLK_EQU_SYSCLK`; `MST_CLK_SEL` 0 selects the crystal, 1 the default source (M `SLL:161-171`), which
ESP-IDF and Arduino compute from the APB clock (R). CrossPoint's display bus runs at 10 MHz and its SD probe at
40 MHz (Part A, the SD line).

**CS.** Hardware CS0–5 are the output signals FSPICS0–5; `CS_KEEP_ACTIVE` holds the selected CS low between
transactions (M `SLL:679-695`). CrossPoint uses GPIO CS (R, Part A).

## Part D — I2C0 in master mode

Block at `0x6001_3000`. Source 29, `I2C_EXT0_INTR`, map register `I2C_EXT0_INTR_MAP` (M TRM table 8.3-1).
`PERIP_RST_EN0` bit 7 `SYSTEM_I2C_EXT0_RST` (M `SYSR:465-467`).

**Registers** (M TRM §28.6, registers 28.1–28.34). `CTR` (reset `0x20B`): `SDA_FORCE_OUT` [0], `SCL_FORCE_OUT` [1],
`SAMPLE_SCL_LEVEL` [2], `RX_FULL_ACK_LEVEL` [3], `MS_MODE` [4], `TRANS_START` [5] (write-trigger), `TX_LSB_FIRST`
[6], `RX_LSB_FIRST` [7], `CLK_EN` [8], `ARBITRATION_EN` [9], `FSM_RST` [10] (write-trigger), `CONF_UPGATE` [11]
(write-trigger). `SR`: `RESP_REC` [0] (0 ACK, 1 NACK), `ARB_LOST` [3], `BUS_BUSY` [4], `RXFIFO_CNT` [13:8],
`STRETCH_CAUSE` [15:14] (reset 3), `TXFIFO_CNT` [23:18], `SCL_MAIN_STATE_LAST` [26:24], `SCL_STATE_LAST` [30:28].
`FIFO_CONF`: `NONFIFO_EN` [10], `TX_FIFO_RST` [13], `RX_FIFO_RST` [12] (M `ILL:262-283` sets then clears them).
`DATA` (`+0x1C`): the FIFO port (M `ILL:675-697`). Interrupt bits (M TRM register 28.22): `RXFIFO_WM` 0,
`TXFIFO_WM` 1 (set at reset), `RXFIFO_OVF` 2, `END_DETECT` 3, `BYTE_TRANS_DONE` 4, `ARBITRATION_LOST` 5,
`MST_TXFIFO_UDF` 6, `TRANS_COMPLETE` 7, `TIME_OUT` 8, `TRANS_START` 9, `NACK` 10, `TXFIFO_OVF` 11, `RXFIFO_UDF` 12,
`SCL_ST_TO` 13, `SCL_MAIN_ST_TO` 14, `DET_START` 15, `SLAVE_STRETCH` 16, `GENERAL_CALL` 17. `COMD0..7`: `byte_num`
[7:0], `ack_en` [8], `ack_exp` [9], `ack_val` [10], `op_code` [13:11], `done` [31] (M `ILL:33-49`). Op codes:
RSTART 6, WRITE 1, READ 3, STOP 2, END 4. FIFOs are 32 bytes (M `CAPS:197`).

**Command execution** (M TRM §28.4.9): commands run from `COMD0`; each sets its `done` bit; a STOP ends the
sequence, an END suspends it with SCL held low (`END_DETECT`); restarting runs from `COMD0` again. A WRITE whose
received ACK differs from `ack_exp` while `ack_en` is set raises `NACK_INT` **and generates a STOP**.

**Timing** (M `ILL:93-117,172-194`): the source clock is the crystal (`SCLK_SEL` 0) or RC_FAST (1), divided by
`SCLK_DIV_NUM + 1` (fractional part 0 in `i2c_master`, `IM:695`); ESP-IDF chooses `scl_low − 1`, `scl_high` and
`scl_wait_high` so that one SCL period is `(SCL_LOW_PERIOD + 1) + SCL_HIGH_PERIOD + SCL_WAIT_HIGH_PERIOD` source
cycles, "absolutely accurate" by its own measurement (`ILL:175-179`). At 400 kHz from the 40 MHz crystal: 100
cycles, 2.5 µs.

**The driver** (M `IM:531-614,765-800`): `i2c_master_transmit_receive` fills the command list, starts it and waits
for an event from its interrupt handler: `NACK` → status `ACK_ERROR`; `TIME_OUT` or `ARBITRATION_LOST` →
`TIMEOUT`; `TRANS_COMPLETE` → done. After a NACK it writes a STOP into `COMD0`, starts it, and waits for
`SR.BUS_BUSY` to fall (`IM:560-568,592-603`). Any status but DONE returns `ESP_ERR_INVALID_STATE` (`IM:707-711`):
**on a real X4 every probe read fails with error 259 as it does today in inkemu, but after one address byte**
instead of after the 6 ms software timeout (R). The verdict is X4 either way: a read that "succeeds" with the
bus's idle `0xFF` is rejected as implausible by every probe (`XteinkDetect.cpp:618-639`).

## Part E — SAR ADC, one-shot reads and their calibration

Block `APB_SARADC` at `0x6004_0000`. Source 43, `APB_ADC_INT_MAP` (M TRM table 8.3-1). `PERIP_RST_EN0` bit 28
`SYSTEM_APB_SARADC_RST` (M `SYSR:339-341`). Channels (M TRM table 34.2-1): ADC1 channel c on GPIOc (c = 0..4), ADC2
channel 0 on GPIO5 (M `CAPS:92`).

**One-shot conversion** (M TRM §34.2.3.3): set `ADC1_ONETIME_SAMPLE` [31] or `ADC2_ONETIME_SAMPLE` [30],
`ONETIME_CHANNEL` [28:25], `ONETIME_ATTEN` [24:23], then `ONETIME_START` [29]; at the end `ADCx_DONE_INT_RAW`
(`INT_RAW` bit 31 for ADC1, 30 for ADC2), the result in `SAR1DATA_STATUS`/`SAR2DATA_STATUS` [16:0]. ESP-IDF masks
the low 12 bits (M `ALL:905-911`) and, for ADC2, rejects a result whose bits [16:13] exceed 9 (`ALL:926-934`). The
driver writes `ONETIME_START` 0, waits, writes 1 (`AOH:116-118`), clears the event, enables the unit and spins on
`INT_RAW` (`AOH:147-156`) inside `portENTER_CRITICAL` (the wait that stops CrossPoint today).

**Duration** (M TRM §34.2.3.4): 25 cycles of SAR_CLK per sample; SAR_CLK is the controller clock divided by
`CTRL.SAR_CLK_DIV` [14:7]; the controller clock is the selected source divided by `CLKM_CONF.CLKM_DIV_NUM` [7:0]
(+ `DIV_B/DIV_A`). ESP-IDF selects the source with `CLK_SEL = 2` (M `ALL:315-318`) and divides it by 15, B 1, A 0
(`ALL:59-61`); the one-shot clock source is the APB clock (R, `esp_adc/adc_oneshot.c:113` `ADC_DIGI_CLK_SRC_DEFAULT`).

**Transfer function** (M TRM §34.2.3.2): `Vdata = Vref / k × data / 4095`, `Vref` ≈ 1100 mV, k ≈ 100 %, 75 %, 50 %,
25 % for attenuations 0–3.

**Calibration in eFuse** (M `ETB:179-189`): `BLK_VERSION_MAJOR` (BLOCK2 bits 128–129; 1 = with calibration),
`ADC1_INIT_CODE_ATTENa` (bits 148 + 10a, 10 bits), `ADC1_CAL_VOL_ATTENa` (bits 188 + 10a, 10 bits, bit 9 the sign).
ESP-IDF reads them only when `BLK_VERSION_MAJOR == 1` (`ERC:13-22`): the reference point is `digi = 2000 ± vol`
at 400, 550, 750 and 1370 mV for attenuations 0–3 (`ERC:47-87`), the initial code `init + 1000` (`ERC:24-45`).
Otherwise it self-calibrates the initial code by converting the internal ground (`ASH:61-97`). inkemu's eFuse
holds the dump of a production v0.3 chip (`crates/soc-esp32c3/src/periph/efuse.rs`, decision D9): its
`BLK_VERSION_MAJOR` is 1, so the eFuse path is taken. Decoded (R): attenuation 3, `ADC1_CAL_VOL_ATTEN3` = 0x247
(bit 9 set: −71), `digi` = 1929 at 1370 mV.

**Curve fitting** (M `ACF:127-215`, `ACC:23-49`): `v1 = raw × coeff_a / 65536` with `coeff_a = 65536 × V / digi`
(integer), then `voltage = v1 − error(v1)`, `error` a polynomial of degree 2 (attenuations 0–2) or 4 (attenuation
3) with fixed coefficients and signs, computed in `uint64_t` with truncations. The reading a chip gives for an input
voltage is not documented; the calibration is the function Espressif fits so that the reported voltage is the
input (R).

**Temperature sensor** (M `SVD`): `APB_TSENS_CTRL` (`TSENS_OUT` [7:0] read-only, `TSENS_PU` [22]) and
`TSENS_CTRL2`. CrossPoint reads and writes them back first of all (Part A); nothing reads a temperature (R).

## Part F — GDMA

GDMA (`0x6003_F000`, three channel pairs, M `CAPS:154-155`) feeds GP-SPI2 when `DMA_TX_ENA`/`DMA_RX_ENA` are set
(M TRM §27.5.6). CrossPoint 1.6.5 never touches it (Part A): Arduino's `SPIClass` moves data through `W0..W15`.

## Part G — What QEMU can compare

QEMU's ESP32-C3 machine at the pinned tag `esp-develop-9.2.2-20250817` (M `hw/riscv/esp32c3.c`, fetched raw from
`espressif/qemu`): GPIO is `esp32c3.gpio`, whose read answers `STRAP` and **0 for every other register**, and whose
write does nothing (M `hw/gpio/esp32_gpio.c:23-44`, `esp32c3_gpio.c:23-36`); IO MUX, SPI2, I2C0 and APB_SARADC are
not devices of QEMU: its catch-all `esp32c3.iomem` reads 0 and drops writes (M `esp32c3.c:144-175`;
`docs/lockstep-differences.md`, list of QEMU's devices). GDMA is a device of QEMU (`hw/dma/esp32c3_gdma.c`), unused
by the X4's firmware (Part F). So **no lockstep window can compare GPIO, the matrix, SPI2, I2C0 or the ADC**:
QEMU's values would be zeros against a modelled pad, and the existing windows (hello, nvs, nvs-ota) read no GPIO
register this work changes (M: under `194f286` they report no unmodelled GPIO access, Part H).

## Part H — The reference firmwares under `194f286`

Run on 2026-10-15 (M, each image of `tools/reference-firmware/build.sh`): `flash_hello`, `flash_nvs`,
`flash_nvs_ota`, `uart_select`, `usb_serial_jtag_echo`, `usb_serial_jtag_console` and `deep_sleep_timer` report no
unmodelled access to GPIO, IO MUX, SPI2, I2C0 or the ADC; they touch only the GPIO registers already stored
(`ENABLE`, `FUNC0..5_IN_SEL_CFG`). `adc_wait_panic` reports `I2C0.SR` (read), `APB_SARADC.INT_CLR`,
`ONETIME_SAMPLE` (write) and `INT_RAW` (read): the model of the ADC changes its path (its wait now ends).

## Open questions (each an `ASSUMPTION:` in the code)

1. Whether a level-triggered GPIO interrupt sets `STATUS` again after software clears it while the level holds.
2. What a pad reads with neither a driver nor a pull (floating), and with both pulls on.
3. What a pad reads when the chip drives it and something outside drives the other level (contention).
4. The CS setup and hold times of a GP-SPI2 transaction and their effect on its duration.
5. Whether a NACK with `ack_en` also raises `TRANS_COMPLETE` (the TRM says a STOP is generated).
6. The level MISO idles at when no target drives it (pull-up of the pad, or the target's high impedance).
7. The ADC's reading of a voltage: taken here as the inverse of ESP-IDF's curve fitting for the chip's eFuse.
8. Whether the initial code written through regi2c, or `RTC_CNTL.SENSOR_CTRL`'s power forcing, change a reading.
