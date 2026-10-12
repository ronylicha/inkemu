# Registers accepted without effect

A register listed here **accepts a write without a diagnostic**, but the emulator does not model
what the hardware does with the value. Most of them store what the firmware writes and read it back;
some keep only their configuration bits, beside a status bit the model answers
(`EXTMEM.ICACHE_PRELOAD_CTRL`, `ICACHE_AUTOLOAD_CTRL`, `ICACHE_FREEZE`). Two kinds do not store
anything:

- the write-only strobes `UART0.INT_CLR` and `UART1.INT_CLR`: the write is accepted and dropped,
  since nothing is pending; **reading** one is reported as unmodelled;
- `EXTMEM.ICACHE_SYNC_CTRL`: the write is accepted and the register always reads its `SYNC_DONE`
  bit, because the operation it starts completes at once.

No other memory-mapped register is silent: it is either modelled or reported by name as unmodelled
(spec §5.2), and `--strict-mmio` stops on it.

The PMP CSRs (`pmpcfg0` to `pmpcfg3`, `pmpaddr0` to `pmpaddr15`) are accepted without effect too,
and are not in the table because they are CSRs, not memory-mapped registers: on the ESP32-C3 core
they are stored with their lock rules and **never enforced** — no access is refused because of them
(decision D18 of plan SP1b-2). Plan SP1b-4 enforces them (decision E31 of plan SP1b-3a).

The table is generated: the lists live next to each peripheral model (`accepted()` in
`crates/soc-esp32c3/src/periph/*.rs`). A model that is plain storage is built from its list; a model
that keeps its registers in fields has its list beside it. In both cases the test
`every_row_of_the_table_is_routed_to_a_model_that_accepts_a_write_there`
(`crates/soc-esp32c3/src/periph/mod.rs`) writes the first and the last register of every row through
the real peripherals, so a list cannot name a register its model refuses. The test
`crates/soc-esp32c3/tests/accepted_without_effect.rs` fails when this file and the code differ. To
update it, run `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect`: the test
then rewrites the table below, and nothing else; review the diff before committing it.

"Count" is 1 for a single register, or the number of consecutive registers the row stands for.
"Made real by" names the plan that gives the register its effect. Each plan inherits the rows that
name it. The table has eight heirs: SP1b-3b (flash writes), SP1b-3d (the serial consoles in both
directions), SP1b-3f (deep sleep), SP1b-4, SP2, SP5a, SP5b and SP8. The carry-over note
`docs/superpowers/notes/2026-10-11-sp1b-3a-carryover.md` names the rows one by one, as the table
stood at the end of plan SP1b-3a, when one plan, "SP1b-3b", was to take what five plans now share;
its first section says which plan takes what.

<!-- generated table -->
| Address | Register | Count | Hardware effect not modelled | Made real by |
|---|---|---|---|---|
| `0x6000000c` | `UART0.INT_ENA` | 1 | interrupt enables: nothing is raised | SP1b-3d |
| `0x60000010` | `UART0.INT_CLR` | 1 | interrupt clear: nothing is pending | SP1b-3d |
| `0x60000020` | `UART0.CONF0` | 1 | frame format: a byte is 10 bit times (the transmit FIFO reset acts) | SP1b-3d |
| `0x60000024` | `UART0.CONF1` | 1 | FIFO thresholds | SP1b-3d |
| `0x60002008` | `SPI1.CTRL` | 1 | read mode, dummy output, WP level | SP8 |
| `0x6000200c` | `SPI1.CTRL1` | 1 | CS hold delay after a release-from-power-down | SP8 |
| `0x60002010` | `SPI1.CTRL2` | 1 | CS setup and hold times | SP8 |
| `0x60002014` | `SPI1.CLOCK` | 1 | SPI clock divider: a transfer takes no time | SP8 |
| `0x60002034` | `SPI1.MISC` | 1 | chip-select idle level and enables | SP8 |
| `0x60002098` | `SPI1.FLASH_WAITI_CTRL` | 1 | automatic wait for the chip to be idle after a program or an erase | SP1b-3b |
| `0x6000209c` | `SPI1.FLASH_SUS_CTRL` | 1 | automatic program/erase suspend and resume | SP1b-3b |
| `0x600020a4` | `SPI1.SUS_STATUS` | 1 | program/erase suspend state | SP1b-3b |
| `0x600020dc` | `SPI1.CLOCK_GATE` | 1 | register clock gate | SP8 |
| `0x60003008` | `SPI0.CTRL` | 1 | cache-fill transfer (read command, dummy cycles, clock): the bus reads the flash directly | SP1b-4 |
| `0x60003010` | `SPI0.CTRL2` | 1 | cache-fill transfer (read command, dummy cycles, clock): the bus reads the flash directly | SP1b-4 |
| `0x60003014` | `SPI0.CLOCK` | 1 | cache-fill transfer (read command, dummy cycles, clock): the bus reads the flash directly | SP1b-4 |
| `0x60003018` | `SPI0.USER` | 1 | cache-fill transfer (read command, dummy cycles, clock): the bus reads the flash directly | SP1b-4 |
| `0x6000301c` | `SPI0.USER1` | 1 | cache-fill transfer (read command, dummy cycles, clock): the bus reads the flash directly | SP1b-4 |
| `0x60003020` | `SPI0.USER2` | 1 | cache-fill transfer (read command, dummy cycles, clock): the bus reads the flash directly | SP1b-4 |
| `0x60003024` | `SPI0.MOSI_DLEN` | 1 | cache-fill transfer (read command, dummy cycles, clock): the bus reads the flash directly | SP1b-4 |
| `0x60003028` | `SPI0.MISO_DLEN` | 1 | cache-fill transfer (read command, dummy cycles, clock): the bus reads the flash directly | SP1b-4 |
| `0x60003034` | `SPI0.MISC` | 1 | chip-select enables | SP1b-4 |
| `0x6000303c` | `SPI0.CACHE_FCTRL` | 1 | cache request enable | SP1b-4 |
| `0x600030dc` | `SPI0.CLOCK_GATE` | 1 | register clock gate | SP8 |
| `0x600030e0` | `SPI0.CORE_CLK_SEL` | 1 | MSPI core clock selection: a transfer takes no time | SP8 |
| `0x60004020` | `GPIO.ENABLE` | 1 | pin output enable | SP2 |
| `0x60004154` | `GPIO.FUNC0_IN_SEL_CFG` | 1 | input signal routing (GPIO matrix) | SP2 |
| `0x60004158` | `GPIO.FUNC1_IN_SEL_CFG` | 1 | input signal routing (GPIO matrix) | SP2 |
| `0x6000415c` | `GPIO.FUNC2_IN_SEL_CFG` | 1 | input signal routing (GPIO matrix) | SP2 |
| `0x60004160` | `GPIO.FUNC3_IN_SEL_CFG` | 1 | input signal routing (GPIO matrix) | SP2 |
| `0x60004164` | `GPIO.FUNC4_IN_SEL_CFG` | 1 | input signal routing (GPIO matrix) | SP2 |
| `0x60004168` | `GPIO.FUNC5_IN_SEL_CFG` | 1 | input signal routing (GPIO matrix) | SP2 |
| `0x600050f0` | `FE2.FE2_TX_INTERP_CTRL` | 1 | radio memory power: there is no radio | SP5a |
| `0x60006090` | `FE.FE_GEN_CTRL` | 1 | radio memory power: there is no radio | SP5a |
| `0x60008000` | `RTC_CNTL.OPTIONS0` | 1 | BBPLL, crystal and I2C power forcing, CPU stall (the two software reset bits act) | SP1b-3f |
| `0x6000801c` | `RTC_CNTL.TIMER1` | 1 | clock and PLL wake-up delays | SP1b-3f |
| `0x60008024` | `RTC_CNTL.TIMER3` | 1 | Wi-Fi/BT power-up timing | SP1b-3f |
| `0x60008028` | `RTC_CNTL.TIMER4` | 1 | digital power-up timing | SP1b-3f |
| `0x6000802c` | `RTC_CNTL.TIMER5` | 1 | minimum sleep time | SP1b-3f |
| `0x60008030` | `RTC_CNTL.TIMER6` | 1 | peripheral power-up timing | SP1b-3f |
| `0x60008034` | `RTC_CNTL.ANA_CONF` | 1 | analog configuration, glitch reset | SP8 |
| `0x60008070` | `RTC_CNTL.CLK_CONF` | 1 | RTC fast clock source and dividers (a slow clock other than the RC oscillator is refused) | SP8 |
| `0x60008074` | `RTC_CNTL.SLOW_CLK_CONF` | 1 | RTC slow clock divider | SP8 |
| `0x60008080` | `RTC_CNTL.RTC_CNTL` | 1 | RTC regulator and digital bias | SP8 |
| `0x60008084` | `RTC_CNTL.PWC` | 1 | RTC power-domain control | SP1b-3f |
| `0x60008088` | `RTC_CNTL.DIG_PWC` | 1 | digital power-domain control | SP1b-3f |
| `0x6000808c` | `RTC_CNTL.DIG_ISO` | 1 | digital pad isolation | SP1b-3f |
| `0x600080d0` | `RTC_CNTL.PAD_HOLD` | 1 | pad hold | SP2 |
| `0x600080d4` | `RTC_CNTL.DIG_PAD_HOLD` | 1 | digital pad hold | SP2 |
| `0x600080d8` | `RTC_CNTL.BROWN_OUT` | 1 | brown-out detector: the supply is always good | SP2 |
| `0x6000810c` | `RTC_CNTL.FIB_SEL` | 1 | brown-out and glitch reset selection (the super watchdog bit acts) | SP2 |
| `0x6000811c` | `RTC_CNTL.SENSOR_CTRL` | 1 | SAR ADC power forcing | SP2 |
| `0x60009000` | `IO_MUX.PIN_CTRL` | 23 | pad function, pulls, drive strength, input enable | SP2 |
| `0x600090fc` | `IO_MUX.DATE` | 1 | none: the version date of the block, stored as written | SP2 |
| `0x6000e000` | `ANA_I2C.HOST0_CMD` | 1 | analog register bus: bytes are stored, the analog blocks (PLL, SAR ADC, bias) do nothing | SP2 |
| `0x6000e004` | `ANA_I2C.HOST1_CMD` | 1 | analog register bus: bytes are stored, the analog blocks (PLL, SAR ADC, bias) do nothing | SP2 |
| `0x6000e040` | `ANA_I2C.I2C_MST_ANA_CONF0` | 1 | analog register bus: bytes are stored, the analog blocks (PLL, SAR ADC, bias) do nothing | SP2 |
| `0x6000e044` | `ANA_I2C.ANA_CONFIG` | 1 | analog register bus: bytes are stored, the analog blocks (PLL, SAR ADC, bias) do nothing | SP2 |
| `0x6000e048` | `ANA_I2C.ANA_CONFIG2` | 1 | analog register bus: bytes are stored, the analog blocks (PLL, SAR ADC, bias) do nothing | SP2 |
| `0x6001000c` | `UART1.INT_ENA` | 1 | interrupt enables: nothing is raised | SP1b-3d |
| `0x60010010` | `UART1.INT_CLR` | 1 | interrupt clear: nothing is pending | SP1b-3d |
| `0x60010020` | `UART1.CONF0` | 1 | frame format: a byte is 10 bit times (the transmit FIFO reset acts) | SP1b-3d |
| `0x60010024` | `UART1.CONF1` | 1 | FIFO thresholds | SP1b-3d |
| `0x6001ccd4` | `NRX.NRXPD_CTRL` | 1 | radio memory power: there is no radio | SP5a |
| `0x6001d054` | `BB.BBPD_CTRL` | 1 | radio memory power: there is no radio | SP5a |
| `0x60026014` | `APB_CTRL.WIFI_CLK_EN` | 1 | radio clock gates: there is no radio | SP5a |
| `0x60026018` | `APB_CTRL.WIFI_RST_EN` | 1 | radio resets: there is no radio | SP5a |
| `0x6002609c` | `APB_CTRL.FRONT_END_MEM_PD` | 1 | radio front-end memory power | SP5a |
| `0x600260ac` | `APB_CTRL.MEM_POWER_UP` | 1 | radio memory power | SP5a |
| `0x60040000` | `APB_SARADC.CTRL` | 1 | SAR ADC configuration: no conversion is performed | SP2 |
| `0x60040004` | `APB_SARADC.CTRL2` | 1 | SAR ADC configuration: no conversion is performed | SP2 |
| `0x6004000c` | `APB_SARADC.FSM_WAIT` | 1 | SAR ADC configuration: no conversion is performed | SP2 |
| `0x60040018` | `APB_SARADC.SAR_PATT_TAB1` | 1 | SAR ADC configuration: no conversion is performed | SP2 |
| `0x6004001c` | `APB_SARADC.SAR_PATT_TAB2` | 1 | SAR ADC configuration: no conversion is performed | SP2 |
| `0x60040050` | `APB_SARADC.DMA_CONF` | 1 | SAR ADC configuration: no conversion is performed | SP2 |
| `0x60040054` | `APB_SARADC.CLKM_CONF` | 1 | SAR ADC configuration: no conversion is performed | SP2 |
| `0x600c0000` | `SYSTEM.CPU_PERI_CLK_EN` | 1 | clock gates of the CPU peripherals: a gated module keeps answering | SP8 |
| `0x600c0004` | `SYSTEM.CPU_PERI_RST_EN` | 1 | resets of the CPU peripherals (the debug assistant) | SP1b-4 |
| `0x600c0008` | `SYSTEM.CPU_PER_CONF` | 1 | PLL_FREQ_SEL and the CPU wait-mode bits (CPUPERIOD_SEL is used by the clock model) | SP8 |
| `0x600c000c` | `SYSTEM.MEM_PD_MASK` | 1 | memory power-down mask in light sleep | SP1b-3f |
| `0x600c0010` | `SYSTEM.PERIP_CLK_EN0` | 1 | peripheral clock gates: a gated module keeps answering | SP8 |
| `0x600c0014` | `SYSTEM.PERIP_CLK_EN1` | 1 | peripheral clock gates: a gated module keeps answering | SP8 |
| `0x600c0020` | `SYSTEM.BT_LPCK_DIV_INT` | 1 | Bluetooth low-power clock divider: there is no radio | SP5b |
| `0x600c0024` | `SYSTEM.BT_LPCK_DIV_FRAC` | 1 | Bluetooth low-power clock source: there is no radio | SP5b |
| `0x600c0040` | `SYSTEM.CACHE_CONTROL` | 1 | cache clock enable and reset | SP1b-4 |
| `0x600c1000` | `SENSITIVE.ROM_TABLE_LOCK` | 93 | permission management: no access is checked, no violation is raised | SP1b-4 |
| `0x600c2100` | `INTERRUPT_CORE0.CLOCK_GATE` | 1 | clock gate of the controller's registers: they always answer | SP8 |
| `0x600c4000` | `EXTMEM.ICACHE_CTRL` | 1 | cache enable: the flash windows are always readable (D5) | SP1b-4 |
| `0x600c4004` | `EXTMEM.ICACHE_CTRL1` | 1 | bus gating of the flash windows (SHUT_IBUS, SHUT_DBUS; D5) | SP1b-4 |
| `0x600c4008` | `EXTMEM.ICACHE_TAG_POWER_CTRL` | 1 | tag memory power | SP8 |
| `0x600c4028` | `EXTMEM.ICACHE_SYNC_CTRL` | 1 | cache operation: completes at once, there are no cache lines | SP1b-4 |
| `0x600c402c` | `EXTMEM.ICACHE_SYNC_ADDR` | 1 | cache operation: completes at once, there are no cache lines | SP1b-4 |
| `0x600c4030` | `EXTMEM.ICACHE_SYNC_SIZE` | 1 | cache operation: completes at once, there are no cache lines | SP1b-4 |
| `0x600c4034` | `EXTMEM.ICACHE_PRELOAD_CTRL` | 1 | cache operation: completes at once, there are no cache lines | SP1b-4 |
| `0x600c4040` | `EXTMEM.ICACHE_AUTOLOAD_CTRL` | 1 | cache operation: completes at once, there are no cache lines | SP1b-4 |
| `0x600c40a8` | `EXTMEM.CACHE_WRAP_AROUND_CTRL` | 1 | wrap-around read mode | SP1b-4 |
| `0x600c40ac` | `EXTMEM.CACHE_MMU_POWER_CTRL` | 1 | MMU memory power | SP8 |
| `0x600c40c4` | `EXTMEM.CACHE_MMU_OWNER` | 1 | MMU ownership bits | SP1b-4 |
| `0x600c40cc` | `EXTMEM.ICACHE_FREEZE` | 1 | cache operation: completes at once, there are no cache lines | SP1b-4 |
| `0x600ce000` | `ASSIST_DEBUG.CORE_0_MONTR_ENA` | 1 | stack guard: the stack pointer is not compared with the bounds | SP1b-4 |
| `0x600ce008` | `ASSIST_DEBUG.CORE_0_INTR_ENA` | 1 | stack guard: the stack pointer is not compared with the bounds | SP1b-4 |
| `0x600ce00c` | `ASSIST_DEBUG.CORE_0_INTR_CLR` | 1 | stack guard: the stack pointer is not compared with the bounds | SP1b-4 |
| `0x600ce038` | `ASSIST_DEBUG.CORE_0_SP_MIN` | 1 | stack guard: the stack pointer is not compared with the bounds | SP1b-4 |
| `0x600ce03c` | `ASSIST_DEBUG.CORE_0_SP_MAX` | 1 | stack guard: the stack pointer is not compared with the bounds | SP1b-4 |
