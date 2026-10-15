# The wiring of the X4 (SP2d): pins, polarities, ladders, power, orientation

Date 2026-10-15, branch of SP2d from `main` at `4b02a91`. Tags as in `sp2a-external-chips.md`:
**[SRC]** read in the code of a firmware that drives the X4, **[HW]** stated as confirmed on hardware by a
project author, **[INF]** reasoning, **[HYP]** chosen here, kept as a profile parameter or an
`ASSUMPTION:` in the code. Earlier work this report builds on and does not repeat:
`docs/research/xteink-hardware.md` (§1.1 and §2: pins, panel, ladders, power latch),
`docs/research/sp2a-external-chips.md` (the SSD1677, the ladders and the battery in themselves),
`docs/research/esp32c3-deep-sleep.md` §5 (the registers of the GPIO deep-sleep wake),
`docs/research/flash-writes-and-crosspoint-boot.md` §K (what CrossPoint 1.6.5 does before its first
screen). The design that rules on what follows is `docs/superpowers/specs/2026-10-15-sp2d-x4-wiring-design.md`.

## 0. Sources

| Short name | What | Where, pinned |
|---|---|---|
| **FI** | FreeInk SDK, the hardware layer of CrossPoint | `Free-Ink/freeink-sdk` at `111fdcc7f0176c3ee38391a160ee296bf492dbd8`, the commit the release CrossPoint 1.6.5 builds on (`sp2a-external-chips.md` §0). Read file by file through `raw.githubusercontent.com` on 2026-10-15; no clone, nothing built or run. Line numbers below are those of `111fdcc7` (SP2a's were those of `7fcd3b34`). |
| **CP** | CrossPoint Reader 1.6.5 | `crosspoint-reader/crosspoint-reader`, tag `1.6.5`, commit `93e98bb78702e29868a16a13b80c40e6b36ccdff`, read the same way. Its `.gitmodules` names FreeInk as the submodule `freeink-sdk`. |
| **SB** | The SP2b hand-over | branch of PR #18 (not merged): `crates/emu-core/src/signals.rs`, `docs/adr/0012-signaux-vers-les-puces-externes.md`, `docs/superpowers/notes/2026-10-15-sp2b-carryover.md` |

Paths are relative to each repository: `FI:BoardConfig.h:869` reads "line 869 of
`libs/hardware/BoardConfig/include/BoardConfig.h` in FI". Short file names used below:
`BoardConfig.h`, `InputManager.cpp` / `.h` (`libs/hardware/InputManager/`), `XteinkDetect.cpp`
(`libs/hardware/XteinkDetect/src/`), `EpdBus.cpp` (`libs/display/FreeInkDisplay/src/bus/`),
`PowerManager.cpp` (`libs/hardware/PowerManager/src/`); in CP `main.cpp` (`src/`), `HalGPIO.cpp`,
`HalGPIO.h`, `HalPowerManager.cpp` (`lib/hal/`), `GfxRenderer.cpp` (`lib/GfxRenderer/`),
`CrossPointSettings.h` (`src/`), `ButtonRemapActivity.cpp` (`src/activities/settings/`).

Licences: FI and CP are MIT (`sp2a-external-chips.md` §9). This report and the code it informs take
**facts** from them (pin numbers, levels, orders, thresholds, the shape of a boot sequence), cited by file
and line; nothing is copied.

## 1. Findings

1. **The pins of SP2a's table are those of the release.** `XTEINK_X4` (`BoardConfig.h:869-899`):
   display `{sclk 8, mosi 10, cs 21, dc 4, rst 5, busy 6, powerEnable none}` (struct order
   `BoardConfig.h:455-463`); SD `{sclk none, miso 7, mosi none, cs 12, powerEnable none, separateSpi
   false, spiHz 0}` (`:465-480`: the card shares the display's SCLK and MOSI); inputs `{back 0,
   confirm 1, left 2, right 3, up 4, down 5, power 3, powerActiveHigh false}` (`:519-528`: the first
   six are indices of the ladder decoder, not pins; `power` is GPIO3); battery ADC GPIO0, divider
   multiplier 2.0; `usbDetect` GPIO20; orientation `NO_FLIP` (`:682-686`, `:850`); power latch
   `latch0` GPIO13 (`:893-899`). [SRC]
2. **The display bus is SPI mode 0, MSB first, 10 MHz, with a chip select and D/C written by
   software around each transfer** (`EpdBus.cpp:75`, `:148-184`): `dc` then `cs` low, `SPI.transfer`
   or `SPI.writeBytes`, `cs` high. A command and its data may share one selection, D/C changing
   between the two transfers (`:175-184`). [SRC]
3. **The display probe bit-bangs the bus** before SPI owns it (`XteinkDetect.cpp:50-90`, called from
   `HalGPIO.cpp:125`): SCLK, MOSI, D/C and CS as GPIO outputs, about 500 kHz (`:50`); a byte is
   written MSB first, MOSI set, SCLK high, SCLK low (`:52-61`); a read releases MOSI to an input with
   pull-up, D/C high, and samples it while SCLK is low before each rising edge, "the controller shifts
   the next bit out on the SCL falling edge" (`:63-90`). It sends `0x71` then `0x70` and reads 1 and 5
   bytes; an SSD1677 answers neither, the line floats to the pull-up, all bytes read alike, and the
   panel is taken for an SSD1677 (`:97-120`). The pins are released afterwards: SCLK, MOSI, D/C and
   RST as inputs ("RST_N has an internal pull-up"), CS as input with pull-up (`:163-170`). [SRC]
4. **RST is pulsed and held.** The probe pulses it high 2 ms, low `rstLowMs`, high (`XteinkDetect.cpp:146-152`);
   `EpdBus` pulses it 10/10/10 ms (SP2a §2). Before a deep sleep FI drives it high and holds it
   (`gpio_hold_en`), the X4 having no display power switch (`PowerManager.cpp:96-108`); on wake the
   probe and the bus release the hold before pulsing it (`XteinkDetect.cpp:139-146`). [SRC]
5. **The buttons**: GPIO1 and GPIO2 are analog inputs (`pinMode INPUT`, `InputManager.cpp:93-95`),
   read with `analogRead` at `ADC_11db` (12 dB in ESP-IDF 5), decoded against the thresholds of SP2a §7
   (`InputManager.cpp:29-44`, `ADC_NO_BUTTON` 3900 at `InputManager.h:440`). The power key is GPIO3,
   `INPUT_PULLUP`, pressed when low (`InputManager.cpp:96`, `powerActiveHigh false`). The names, in
   decoder order: `Back, Confirm, Left, Right, Up, Down, Power` (`InputManager.cpp:45`). [SRC]
6. **The front keys lie, from left to right, Back, Confirm, Left, Right**: "This mirrors the
   on-device front button order: Back, Confirm, Left, Right" (`ButtonRemapActivity.cpp:140`); the
   default layout `BACK_CONFIRM_LEFT_RIGHT` is the hardware order (`CrossPointSettings.h:76-95`). This
   settles the first `ASSUMPTION:` of `crates/host/src/catalog.rs`. [SRC] The two side keys are Up and
   Down on the right edge (`xteink-hardware.md` §1.1, [HW/SRC]); Up above Down is read from their names
   and their roles, "Up = Previous, Down = Next" (`CrossPointSettings.h:97`) [INF].
7. **The panel is turned 90° clockwise for its user.** CrossPoint's default orientation is
   `PORTRAIT`, "480x800 logical coordinates" (`CrossPointSettings.h:68-69`); its renderer maps a
   logical portrait pixel *(x, y)* to the frame buffer's *(X, Y) = (y, 479 − x)*, "Rotation: 90
   degrees clockwise" (`GfxRenderer.cpp:359-367`). The frame buffer is the panel's native 800×480
   frame: FreeInk declares `NO_FLIP` ("native scan") for the X4 and its driver writes row *y* of the
   buffer so that it shows as row *y* of the glass (SP2a §4, the window with Y reversed); SP2a's model
   publishes it upright (`crates/chips/tests/x4_driver.rs`,
   `a_full_refresh_shows_the_frame_buffer_upright_in_about_two_seconds`). Inverting the mapping, the
   user sees native pixel *(X, Y)* at *(479 − Y, X)*: the native top row becomes the right edge, a
   clockwise quarter turn. Since CrossPoint shows upright on real units, the rotation is that one,
   given the firmware's own frame. This settles the second `ASSUMPTION:` of `catalog.rs` for the X4
   (the X3 keeps it until SP3). [SRC] + [INF]
8. **USB presence is GPIO20, read high when a cable powers the device** (`HalGPIO.cpp:245-260`,
   `usbDetect` 20, `HalGPIO.h:18` "Used for USB connection detection"), configured `INPUT` without
   pull (`HalGPIO.cpp:131-134`). [SRC] What drives it low without a cable is not given; a divider on
   VBUS is the usual circuit [HYP].
9. **The power topology** (`BoardConfig.h:893-899`, comment of the X4 profile): GPIO13 gates the
   battery MOSFET; "known units self-latch through a pull once the power button bridges the rail",
   one revision "stays powered only while the button is held"; "driving it LOW is the battery
   power-off". At boot FI drives every latch high (`holdPowerRails`, `BoardConfig.h:2045-2060`). [SRC]
10. **CrossPoint's boot decision depends on the reset reason, GPIO20 and GPIO3**
    (`HalGPIO.cpp:270-303`, `main.cpp:382-387`, `:451-474`):

    | Reset, wake cause | GPIO20 | Reason | What follows |
    |---|---|---|---|
    | deep sleep, GPIO | any | `PowerButton` | boots if GPIO3 still reads low 10 ms after the first sample (`verifyPowerButtonWakeup`, `HalGPIO.cpp:225-243`), else sleeps again |
    | power-on | low | `PowerButton` (an Xteink can only cold-boot on battery with the key held, `HalGPIO.cpp:270-280`) | same |
    | power-on | high | `AfterUSBPower` | the X4 sleeps at once ("plugging USB into an off device should charge-sleep") |
    | unknown | high | `AfterFlash` | boots |

    The SD card is mounted **before** that decision: without a card, CrossPoint shows "SD card error"
    and returns from `setup()` whatever the reason (`main.cpp:407-411`). [SRC]
11. **CrossPoint's deep sleep** (`HalPowerManager.cpp:69-134`, `PowerManager.cpp:56-144`): drive
    GPIO13 low and hold it (the battery power-off, `:79-89`); hold RST high (`PowerManager.cpp:108`);
    wait until GPIO3 reads high, polling every 50 ms (`:67-76`); set GPIO3 `INPUT_PULLUP` and arm
    `esp_deep_sleep_enable_gpio_wakeup(1 << 3, ESP_GPIO_WAKEUP_GPIO_LOW)` (`:34-62`); then
    `esp_sleep_config_gpio_isolate()`, `gpio_deep_sleep_hold_en()`, `esp_deep_sleep_start()`
    (`:118-122`). On battery the latch cuts the rail as soon as the key is released: the device is off,
    and the next press is a cold boot, the second row of the table. With a cable the rail stays and
    the GPIO wake ends the sleep: the first row. [SRC] + [INF]
12. **The stock firmware 3.0.2** goes back to deep sleep 637 ms after a power-on without its power key
    held, with the GPIO wake alone armed (`WAKEUP_ENA 0x4`) (SB, hand-over note). Whether it drives
    GPIO13 is not known (no source; its binary is the user's). [SRC for what SB measured]

## 2. The pins of the X4, as SP2d wires them

| Pin | Function | Direction seen from the SoC | Level, polarity | Chip of SP2a / board element | Source |
|---|---|---|---|---|---|
| GPIO0 | battery | analog in (ADC1 ch 0) | cell × 10 kΩ / (10 kΩ + 10 kΩ) | `Battery` | §1 item 1; `xteink-hardware.md` §1.1 |
| GPIO1 | ladder A: Back, Confirm, Left, Right | analog in (ch 1) | idle at the 3.3 V rail, keys below | `ButtonLadder` | §1 item 5 |
| GPIO2 | ladder B: Up, Down | analog in (ch 2) | same | `ButtonLadder` | §1 item 5 |
| GPIO3 | power key | digital in | low when pressed; released: the pad's pull-up | push button | §1 item 5 |
| GPIO4 | display D/C | out | low command, high data | SSD1677 | §1 item 2 |
| GPIO5 | display RES# | out | low = reset; released: the controller's pull-up | SSD1677 | §1 items 3, 4 |
| GPIO6 | display BUSY | in | high while busy (SP2a) | SSD1677 | SP2a §2 |
| GPIO7 | SD MISO | in | — | none in SP2d (SP2e) | §1 item 1 |
| GPIO8 | SCLK, display and SD | out | mode 0 | SSD1677 | §1 items 1-3 |
| GPIO10 | MOSI, display and SD; the display's half-duplex SDA | out; in during a probe read | — | SSD1677 | §1 item 3 |
| GPIO12 | SD CS | out | low selects | none in SP2d (SP2e) | §1 item 1 |
| GPIO13 | power latch | out | high keeps the battery rail, low cuts it | board power | §1 item 9 |
| GPIO20 | USB presence (also UART0 RX) | in | high with a cable | board power | §1 item 8 |
| GPIO21 | display CS | out | low selects; released: deselected | SSD1677 | §1 items 2, 3 |

## 3. The voltages of the ladders

SP2a §7 converted the averages of three devices (raw 12-bit codes at 12 dB) to millivolts with a straight
line, 0–2 500 mV over 0–4 095 [HYP]. SP2b's SAR ADC does not use a straight line: it converts a voltage to
the reading whose voltage, by ESP-IDF's calibration curve for the eFuse of the emulated chip, is the
closest (SB, `saradc.rs`, `Calibration::reading`). A level declared by the straight line would therefore
not come back to the code it was taken from. The levels SP2d declares are computed the other way: for
each recorded average, the millivolts ESP-IDF's curve gives that code at 12 dB on the X4's eFuse; a test
converts each level back through the SoC's model and requires the code FreeInk's thresholds decode as
that key, within the spread of the three devices. Idle is the rail, 3 300 mV, which saturates to 4 095
(above `ADC_NO_BUTTON`). [INF], the exact values to be computed in the plan's task on the ladders.

## 4. What the sources leave open, and what SP2d does with it

| Open point | SP2d | Why |
|---|---|---|
| A released RST, CS, D/C, SCLK or MOSI | RST released reads high (FreeInk: internal pull-up); CS released deselects; a released D/C, SCLK or MOSI clocks nothing [HYP] | the chip side of a released line; the pad's own pull is the SoC's |
| What drives GPIO20 low without a cable | the board drives it: high with a cable, low without [HYP] | a divider on VBUS; CrossPoint configures it without a pull |
| Whether the X4 is on a cable | a run input, absent by default (on battery) [HYP] | the power key then turns the X4 on, as a user does |
| A latch released (input) | keeps the rail (self-latch) [HYP, from FI's comment] | "known units self-latch" |
| How long a user holds the key to turn the X4 on | a profile value, 1 000 ms [HYP] | CrossPoint verifies 10 ms; the stock firmware's minimum is not known |
| The pulls of a pad during deep sleep | those of its IO MUX register (`FUN_WPU/WPD`): read in ESP-IDF v5.5.2 on 2026-10-15, `gpio_deep_sleep_wakeup_prepare` (`esp_hw_support/sleep_modes.c:2115-2140`) enables the pull-up of a low-level wake pin (`gpio_pullup_en`, with `CONFIG_ESP_SLEEP_GPIO_ENABLE_INTERNAL_RESISTORS`) and holds the pad (`gpio_hold_en` → `RTC_CNTL_PAD_HOLD` bit *n* for GPIO0–5, `DIG_PAD_HOLD` `GPIO_HOLD_MASK[n] = BIT(n)` above: `hal/esp32c3/include/hal/gpio_ll.h:435-457`, `soc/esp32c3/gpio_periph.c:36-59`); `esp_sleep_config_gpio_isolate` (`sleep_gpio.c:57-87`) writes only the sleep-mode settings (`gpio_sleep_set_direction`, `gpio_sleep_set_pull_mode`), which act when a pad's sleep select is on, which a deep sleep of the C3 does not turn on; the automatic hold (`DIG_ISO.DG_PAD_AUTOHOLD_EN`, `gpio_ll.h:398-411`) is not modelled [SRC] + [HYP for the last point] | 2d-R7 |
| The battery's cell voltage | a profile value, 3 900 mV [HYP] | a charged cell between FI's 4.2 V full and 3.45 V empty |
| Up above Down | Up first along the right edge [INF] | names and roles |
| The SD card | not wired in SP2d; CS 12 and MISO 7 left to the pads | CrossPoint's home screen needs a card (§1 item 10): sub-project 2e |

## 5. Verdicts on the hypotheses of `crates/host/src/catalog.rs`

| `ASSUMPTION:` | Verdict |
|---|---|
| "the order of the four front keys from left to right is not established" (X4) | **settled**: Back, Confirm, Left, Right (§1 item 6) |
| "the rotation of both panels in their device (90°…) is not established" | **settled for the X4**: 90° clockwise (§1 item 7); the X3 keeps it until SP3 |
| "its physical layout is not known; the X4's is used" (X3) | unchanged, SP3 |
