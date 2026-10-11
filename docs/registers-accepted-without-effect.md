# Registers accepted without effect

A register listed here **stores what the firmware writes and reads it back**, but the emulator does
not model what the hardware does with the value. Nothing else is silent: any other register is either
modelled or reported by name as unmodelled (spec §5.2), and `--strict-mmio` stops on it.

The table is generated: the lists live next to each peripheral model (`accepted()` in
`crates/soc-esp32c3/src/periph/*.rs`), the storage of each model is built from them, and the test
`crates/soc-esp32c3/tests/accepted_without_effect.rs` fails when this file and the code differ. To
update it, run `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect`: the test
then rewrites the table below, and nothing else; review the diff before committing it.

"Count" is 1 for a single register, or the number of consecutive registers the row stands for.
"Made real by" names the plan that gives the register its effect.

<!-- generated table -->
| Address | Register | Count | Hardware effect not modelled | Made real by |
|---|---|---|---|---|
| `0x6000000c` | `UART0.INT_ENA` | 1 | interrupt enables: nothing is raised | SP1b-3 |
| `0x60000010` | `UART0.INT_CLR` | 1 | interrupt clear: nothing is pending | SP1b-3 |
| `0x60000014` | `UART0.CLKDIV` | 1 | baud rate: bytes leave at once | SP1b-3 |
| `0x60000020` | `UART0.CONF0` | 1 | frame format | SP1b-3 |
| `0x60000024` | `UART0.CONF1` | 1 | FIFO thresholds | SP1b-3 |
| `0x60000078` | `UART0.CLK_CONF` | 1 | clock source and divider | SP1b-3 |
| `0x60002008` | `SPI1.CTRL` | 1 | read mode, dummy output, WP level | SP8 |
| `0x6000200c` | `SPI1.CTRL1` | 1 | CS hold delay after a release-from-power-down | SP8 |
| `0x60002010` | `SPI1.CTRL2` | 1 | CS setup and hold times | SP8 |
| `0x60002014` | `SPI1.CLOCK` | 1 | SPI clock divider: a transfer takes no time | SP8 |
| `0x600020a4` | `SPI1.SUS_STATUS` | 1 | program/erase suspend state | SP1b-3 |
| `0x600020dc` | `SPI1.CLOCK_GATE` | 1 | register clock gate | SP1b-3 |
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
| `0x600030dc` | `SPI0.CLOCK_GATE` | 1 | register clock gate | SP1b-3 |
| `0x60004020` | `GPIO.ENABLE` | 1 | pin output enable | SP2 |
| `0x60004154` | `GPIO.FUNC0_IN_SEL_CFG` | 1 | input signal routing (GPIO matrix) | SP2 |
| `0x60004158` | `GPIO.FUNC1_IN_SEL_CFG` | 1 | input signal routing (GPIO matrix) | SP2 |
| `0x60008084` | `RTC_CNTL.PWC` | 1 | RTC power-domain control | SP1b-3 |
| `0x6000808c` | `RTC_CNTL.DIG_ISO` | 1 | digital pad isolation | SP1b-3 |
| `0x600080ac` | `RTC_CNTL.SWD_CONF` | 1 | super watchdog: auto-feed, reset flag, strobes | SP1b-3 |
| `0x600080d0` | `RTC_CNTL.PAD_HOLD` | 1 | pad hold | SP2 |
| `0x600080d4` | `RTC_CNTL.DIG_PAD_HOLD` | 1 | digital pad hold | SP2 |
| `0x60009000` | `IO_MUX.PIN_CTRL` | 24 | pad function, pulls, drive strength, input enable | SP2 |
| `0x6001000c` | `UART1.INT_ENA` | 1 | interrupt enables: nothing is raised | SP1b-3 |
| `0x60010010` | `UART1.INT_CLR` | 1 | interrupt clear: nothing is pending | SP1b-3 |
| `0x60010014` | `UART1.CLKDIV` | 1 | baud rate: bytes leave at once | SP1b-3 |
| `0x60010020` | `UART1.CONF0` | 1 | frame format | SP1b-3 |
| `0x60010024` | `UART1.CONF1` | 1 | FIFO thresholds | SP1b-3 |
| `0x60010078` | `UART1.CLK_CONF` | 1 | clock source and divider | SP1b-3 |
| `0x6002609c` | `APB_CTRL.FRONT_END_MEM_PD` | 1 | radio front-end memory power | SP5a |
| `0x600260ac` | `APB_CTRL.MEM_POWER_UP` | 1 | radio memory power | SP5a |
| `0x600c0000` | `SYSTEM.CPU_PERI_CLK_EN` | 1 | clock gates of the CPU peripherals | SP1b-3 |
| `0x600c0004` | `SYSTEM.CPU_PERI_RST_EN` | 1 | resets of the CPU peripherals | SP1b-3 |
| `0x600c0008` | `SYSTEM.CPU_PER_CONF` | 1 | PLL_FREQ_SEL and the CPU wait-mode bits (CPUPERIOD_SEL is used by the clock model) | SP8 |
| `0x600c000c` | `SYSTEM.MEM_PD_MASK` | 1 | memory power-down mask in light sleep | SP1b-3 |
| `0x600c0010` | `SYSTEM.PERIP_CLK_EN0` | 1 | peripheral clock gates | SP1b-3 |
| `0x600c0014` | `SYSTEM.PERIP_CLK_EN1` | 1 | peripheral clock gates | SP1b-3 |
| `0x600c0018` | `SYSTEM.PERIP_RST_EN0` | 1 | peripheral reset pulses | SP1b-3 |
| `0x600c001c` | `SYSTEM.PERIP_RST_EN1` | 1 | peripheral reset pulses | SP1b-3 |
| `0x600c0040` | `SYSTEM.CACHE_CONTROL` | 1 | cache clock enable and reset | SP1b-4 |
| `0x600c101c` | `SENSITIVE.INTERNAL_SRAM_USAGE_1` | 1 | SRAM0 handed to the cache as line storage | SP1b-4 |
| `0x600c2000` | `INTERRUPT_CORE0.MAC_INTR_MAP` | 62 | interrupt routing: nothing is delivered to the CPU | SP1b-3 |
| `0x600c2100` | `INTERRUPT_CORE0.CLOCK_GATE` | 1 | interrupt routing: nothing is delivered to the CPU | SP1b-3 |
| `0x600c2104` | `INTERRUPT_CORE0.CPU_INT_ENABLE` | 1 | interrupt routing: nothing is delivered to the CPU | SP1b-3 |
| `0x600c2108` | `INTERRUPT_CORE0.CPU_INT_TYPE` | 1 | interrupt routing: nothing is delivered to the CPU | SP1b-3 |
| `0x600c210c` | `INTERRUPT_CORE0.CPU_INT_CLEAR` | 1 | edge-interrupt acknowledge: nothing is pending | SP1b-3 |
| `0x600c2114` | `INTERRUPT_CORE0.CPU_INT_PRI_0` | 32 | interrupt routing: nothing is delivered to the CPU | SP1b-3 |
| `0x600c2194` | `INTERRUPT_CORE0.CPU_INT_THRESH` | 1 | interrupt routing: nothing is delivered to the CPU | SP1b-3 |
| `0x600c4000` | `EXTMEM.ICACHE_CTRL` | 1 | cache enable: the flash windows are always readable (D5) | SP1b-4 |
| `0x600c4004` | `EXTMEM.ICACHE_CTRL1` | 1 | bus gating of the flash windows (SHUT_IBUS, SHUT_DBUS; D5) | SP1b-4 |
| `0x600c4008` | `EXTMEM.ICACHE_TAG_POWER_CTRL` | 1 | tag memory power | SP8 |
| `0x600c4028` | `EXTMEM.ICACHE_SYNC_CTRL` | 1 | cache operation: completes at once, there are no cache lines | SP1b-4 |
| `0x600c402c` | `EXTMEM.ICACHE_SYNC_ADDR` | 1 | cache operation: completes at once, there are no cache lines | SP1b-4 |
| `0x600c4030` | `EXTMEM.ICACHE_SYNC_SIZE` | 1 | cache operation: completes at once, there are no cache lines | SP1b-4 |
| `0x600c4034` | `EXTMEM.ICACHE_PRELOAD_CTRL` | 1 | cache operation: completes at once, there are no cache lines | SP1b-4 |
| `0x600c4040` | `EXTMEM.ICACHE_AUTOLOAD_CTRL` | 1 | cache operation: completes at once, there are no cache lines | SP1b-4 |
| `0x600c4078` | `EXTMEM.CACHE_ILG_INT_ENA` | 1 | cache error interrupt: the status is latched, no interrupt is raised | SP1b-3 |
| `0x600c4084` | `EXTMEM.CORE0_ACS_CACHE_INT_ENA` | 1 | cache error interrupt: the status is latched, no interrupt is raised | SP1b-3 |
| `0x600c40a8` | `EXTMEM.CACHE_WRAP_AROUND_CTRL` | 1 | wrap-around read mode | SP1b-4 |
| `0x600c40ac` | `EXTMEM.CACHE_MMU_POWER_CTRL` | 1 | MMU memory power | SP8 |
| `0x600c40c4` | `EXTMEM.CACHE_MMU_OWNER` | 1 | MMU ownership bits | SP1b-4 |
| `0x600c40cc` | `EXTMEM.ICACHE_FREEZE` | 1 | cache operation: completes at once, there are no cache lines | SP1b-4 |
| `0x600ce044` | `ASSIST_DEBUG.CORE_0_RCD_EN` | 1 | PC/SP recording at reset | SP1b-3 |
