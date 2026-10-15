# How CrossPoint 1.6.5 drives the Xteink X3 (SP3 research)

Date 2026-10-15. Tags as in `sp2d-x4-wiring.md`: **[SRC]** read in the code of a firmware that drives
the X3, **[HW]** stated as confirmed on hardware by a project author, **[INF]** reasoning, **[?]**
unknown. Earlier work this report builds on: `docs/research/xteink-hardware.md` §1.2 and §2 (X3
table and C3 pin map), `docs/research/sp2d-x4-wiring.md` (the X4 wiring, same board family). §10
lists what this report confirms or corrects in `xteink-hardware.md`.

## 0. Sources

| Short name | What | Where, pinned |
|---|---|---|
| **FI** | FreeInk SDK, the hardware layer of CrossPoint | `Free-Ink/freeink-sdk` at `111fdcc7f0176c3ee38391a160ee296bf492dbd8`, read file by file through `raw.githubusercontent.com` on 2026-10-15; nothing built or run. |
| **CP** | CrossPoint Reader 1.6.5 | `crosspoint-reader/crosspoint-reader`, tag `1.6.5`, commit `93e98bb78702e29868a16a13b80c40e6b36ccdff`, read the same way. |
| **AE** | Arduino-ESP32 core 3.3.1 (only to settle which I2C driver `Wire` uses) | `espressif/arduino-esp32` tag `3.3.1`. CP builds with pioarduino `55.03.311` (`CP:platformio.ini:10`), whose release notes say "Arduino v3.3.1 … IDF 5.5.1". |

Short file names: in FI `BoardConfig.h` (`libs/hardware/BoardConfig/include/`), `XteinkDetect.cpp` /
`.h` (`libs/hardware/XteinkDetect/`), `EpdBus.cpp` / `.h` (`libs/display/FreeInkDisplay/src/bus/`),
`Uc8253X3Driver.cpp` / `.h`, `Uc8279Driver.cpp` (`…/src/driver/`), `Uc8253X3Luts.h`,
`Uc8279X3Luts.h` (`…/src/lut/`), `FreeInkDisplay.cpp`, `BatteryMonitor.cpp`, `Rtc.cpp`, `Imu.cpp`,
`PowerManager.cpp`, `SDCardManager.cpp`, `InputManager.cpp`, `docs/xteink-x3-uc8279-support.md`;
in CP `main.cpp`, `HalGPIO.cpp` / `.h`, `HalPowerManager.cpp` / `.h`, `HalClock.cpp` / `.h`,
`HalTiltSensor.cpp` / `.h`, `HalDisplay.cpp` (`lib/hal/`), `GfxRenderer.cpp` (`lib/GfxRenderer/`),
`CrossPointSettings.h` (`src/`), `BaseTheme.cpp` (`src/components/themes/`). Facts only are taken
(MIT, both); no code block longer than three lines is copied.

## 1. Board profile (`XTEINK_X3`, `XTEINK_X3_UC8279`)

`XTEINK_X3` is `FI:BoardConfig.h:906-936`; `XTEINK_X3_UC8279` is `:945-968` and differs only in
`board`, `name` and `displayController`. Field order is that of `BoardProfile` (`:715-768`). [SRC]

| Field (struct line) | X3 value | Note |
|---|---|---|
| board, name (`:716-717`) | `Board::XteinkX3` / `"xteink_x3"`; sibling `XteinkX3Uc8279` / `"xteink_x3_uc8279"` | enum `:384-403` |
| inputStyle (`:718`) | `XteinkAdcLadder` | same as X4 |
| displayController (`:719`) | `UC8253` (enum value 2); sibling `UC8279` (6) | enum `:428-437` |
| displayWidth/Height (`:720-721`) | 792 × 528 | native landscape |
| display (`DisplayPins` `:455-463`: sclk, mosi, cs, dc, rst, busy, powerEnable) | `{8, 10, 21, 4, 5, 6, none}` | identical to X4; no panel power switch |
| displaySpiHz (`:723`) | `XTEINK_DISPLAY_SPI_HZ` = 10 000 000 (`:859`) | both X3 profiles; the drivers' 16 MHz default applies only when the profile says 0 (`Uc8253X3Driver.cpp:68-71`, `Uc8279Driver.cpp` `spiHz()`) |
| sd (`SdPins` `:465-478`: sclk, miso, mosi, cs, powerEnable, separateSpi, spiHz, powerActiveHigh) | `{none, 7, none, 12, 13, false, 0}`, powerActiveHigh defaulted `true` | SCLK/MOSI borrowed from the display (8/10); GPIO13 = SD rail, active-HIGH, "confirmed by X3 factory-firmware RE" (`:915-921`) [SRC, RE claim]; spiHz 0 = 40 MHz (`SDCardManager.cpp:130`) |
| input (`InputPins` `:519-529`) | `{0, 1, 2, 3, 4, 5, 3, false}` | first six are decoder indices, power = GPIO3 active-LOW (`INPUT_PULLUP`) |
| batteryAdc, batteryChargeStatus, batteryDividerMultiplier, usbDetect (`:726-729`) | 0, none, 2.0, 20 | copied from X4; unused for the battery (gauge wins, §4) and for USB (§4); `HalPowerManager::begin` still does `pinMode(0, INPUT)` (`CP:HalPowerManager.cpp:26-28`) |
| touch, frontlight, audio, leds | none | |
| orientation (`DisplayOrientation` `:682-685`) | `NO_FLIP` `{false,false}` (`:850`) | the facade does not read it for X3 [INF: no use in `FreeInkDisplay.cpp`] |
| sdmmc | `NO_SDMMC` | SPI card (SdFat) |
| batteryGauge (`BatteryGaugeConfig` `:504-517`: i2cSda, i2cScl, i2cHz, gaugeAddr, chargerAddr, i2cBus, gaugeType) | `{20, 0, 400000, 0x55, 0}` → bus 0, `Bq27220` | no charger IC |
| mic | none | |
| sensors (`SensorsConfig` `:658-676`: i2cSda, i2cScl, i2cHz, rtcAddr, tempHumidityAddr, imuAddr, i2cBus, rtcType, imuType, imuSwapXY/FlipX/FlipY) | `{20, 0, 400000, 0x68, 0, 0x6B, 0, Ds3231, Qmi8658}`, mount flags false | same bus as the gauge |
| uiScale, power, displayControllerVariant, viewableInsets, … | defaults: 1.0, **no latch** (`PowerConfig{}` `:691-702`), 0, insets 9/3/3/3 (`:708-713`) | X3 has no `power.latch0` |

There is **no USB / charge-detect pin** in use on X3 and **no RTC/IMU interrupt pin** in any profile
field [SRC]. A C3 binary built with both `FREEINK_DEVICE_X4` and `_X3` (all CP C3 envs,
`CP:platformio.ini:230-262`) boots with `DEFAULT_DEVICE = XTEINK_X4` (`BoardConfig.h:1892`) and
switches profile at runtime through `selectDevice()` (`:1905-1999`).

## 2. X3 versus X4 detection

### 2.1 Board fingerprint (I2C)

Entry: `CP:HalGPIO.cpp:116-140` → `detectDeviceTypeWithFingerprint()` (`:78-112`). [SRC]

1. NVS namespace **`cphw`** (Arduino `Preferences`), key **`dev_ovr`** (u8: 0 auto, 1 force X4,
   2 force X3). If 1 or 2, that answer is used, no probe (`:81-85`). Values > 2 read as the
   default (`:59-61`).
2. Key **`dev_det`** (u8: 0 unknown, 1 X4, 2 X3). If 1 or 2, used as is, no probe (`:87-91`).
3. Otherwise `freeink::detectXteinkVerdict()`; `X3Confirmed` writes `dev_det=2`,
   `X4Confirmed` writes `dev_det=1`, `Inconclusive` writes nothing and runs as X4 (`:93-111`).

The probe (`FI:XteinkDetect.cpp:578-669`) uses the **hardware I2C controller through Arduino `Wire`**,
not bit-banging: `Wire.begin(SDA 20, SCL 0, 400 000)`, `Wire.setTimeOut(6)` (6 ms per transaction),
three chip checks, `Wire.end()`, then GPIO20 and GPIO0 back to `INPUT` (`:644-653`). Two passes,
`delay(2)` between (`:657-669`). Verdict: both passes ≥ 2 hits → X3; both 0 → X4; else
Inconclusive (`:666-668`).

A **hit is a plausible register value, not a bare ACK** (`:619-642`), checked in this order:

| Chip | Transactions | Hit condition |
|---|---|---|
| BQ27220 @0x55 | read16 LE of `0x2C` (StateOfCharge), then read16 LE of `0x08` (Voltage) | SoC ≤ 100 **and** 2500 ≤ mV ≤ 5000; the voltage read is skipped if the SoC read fails |
| DS3231 @0x68 | read8 of `0x00` (seconds) | valid BCD: tens ≤ 5, ones ≤ 9 |
| QMI8658 @0x6B, then @0x6A | read8 of `0x00` (WHO_AM_I) | value `0x05` at either address |

Each read is `beginTransmission(addr); write(reg); endTransmission(false); requestFrom(addr, n, true)`
(`:596-617`). In AE 3.3.1 `endTransmission(false)` only marks the transaction non-stop
(`AE:libraries/Wire/src/Wire.cpp:445-476`) and `requestFrom` then issues a single
`i2cWriteReadNonStop` (`:505-519`) → `i2c_master_transmit_receive` (`AE:cores/esp32/esp32-hal-i2c-ng.c:338-371`):
on the wire **START, addr+W, reg, repeated START, addr+R, n bytes (last NACKed), STOP**. [SRC] A
NACK makes `requestFrom` return 0 and the check simply fails (tolerated). No pass writes to any chip.

### 2.2 Panel controller probe (X3 only)

After `selectDevice(X3)`, `CP:HalGPIO.cpp:125` calls `freeink::applyXteinkDisplayController()`
(`FI:XteinkDetect.cpp:415-538`). It first reads NVS namespace **`hw_calib`** key **`screenType`** (u8)
read-only, for a log line only (`:366-375`, `:421-429`), then probes the bus with the X3 protocol
(`detectXteinkDisplayController` routes X3 profiles to `probeX3DisplayController`, `:343-353`), and
on a hit promotes `UC8253 → UC8279` (`:530-534`); CP then selects `XTEINK_X3_UC8279`
(`CP:HalGPIO.cpp:126-128`). `probeX3DisplayController` (`FI:XteinkDetect.cpp:182-250`), "stock X3
V6.3.15" protocol, bit-banged on the display pins before SPI owns them: [SRC]

- CS output HIGH, SCLK output LOW, D/C output HIGH, MOSI output; BUSY input, no pull (`:186-194`).
- RST: `gpio_hold_dis`, HIGH 10 ms, LOW 50 ms, HIGH, then 50 ms (`:196-204`).
- Wait for **BUSY HIGH** (idle), 1 ms polls, give up after 300 ms; a timeout only sets a
  diagnostic flag, the read still happens (`:205-215`).
- CS LOW, D/C LOW, byte `0x70` (VER) written MSB first: per bit, set MOSI, 1 µs, SCLK HIGH, 1 µs,
  SCLK LOW (`epdWriteByte` `:52-61`; about 500 kHz, `:50`).
- D/C HIGH, 1 µs, MOSI to `INPUT` **without pull-up**, 2 µs (`:221-225`).
- **3 bytes**, 8 bits each: SCLK LOW, 1 µs, SCLK HIGH, 1 µs, **sample MOSI while SCLK is HIGH**
  (after the rising edge), MSB first; SCLK LOW + 1 µs after each byte (`:226-236`). No dummy clock.
- CS HIGH, pins released: SCLK, MOSI, D/C, RST to `INPUT`, CS `INPUT_PULLUP` (`:237-239`, `:163-171`).
- Decision on byte 2 only: `0x66` → UC8279; `0xFF` → UC8253 ("assumed"); anything else →
  Inconclusive, UC8253 kept (`:242-246`). FLG (0x71) and MTP are **not** read on X3
  (`XteinkDetect.h:44-59`, `docs/xteink-x3-uc8279-support.md`). [SRC]

For an emulated UC8253: answer byte 2 with anything but `0x66` (a floating line reads `0xFF` only
if the emulator models a pull-up, which the firmware does not enable) [INF]; BUSY must read HIGH
within 300 ms of the end of reset, or boot loses 300 ms but continues.

## 3. Display drivers and bus

### 3.1 Bus (`EpdBus`)

- SPI mode 0, MSB first, at the driver's `spiHz()` = 10 MHz on both X3 profiles
  (`EpdBus.cpp:75`, §1). `SPI.begin(8, -1, 10, 21)` in `EpdBus::begin` (`:95`); CP already did
  `SPI.begin(8, 7, 10, 21)` (`CP:HalGPIO.cpp:130`). CS 21 and D/C 4 are GPIOs driven by software.
- BUSY pin mode `INPUT` (no pull) for `X3TwoPhase` (`:108`); RST hold released, output (`:104-107`).
- Reset: RST HIGH 10 ms, LOW 10 ms, HIGH 10 ms, plus `extraSettleMs` (`:117-145`); both X3 drivers
  pass 50 (`Uc8253X3Driver.cpp:160`, `Uc8279Driver.cpp:112-118`).
- `cmd`: D/C LOW, CS LOW, one byte, CS HIGH (`:147-154`); `data`: same with D/C HIGH (`:156-172`);
  `cmdData`: CS LOW, D/C LOW, command, D/C HIGH, data, CS HIGH in one selection (`:174-185`).
- `sendPlaneFlipped(cmd, plane, h, wb)`: the command in its own CS pulse, then **one CS-low burst
  of all rows, last framebuffer row first** (`:396-403`). `sendPlaneFlippedInverted` streams
  `~byte`, also rows reversed (`:405-426`). `fillPlane`: command, then h × wb copies of a byte
  (`:428-444`).
- **No SPI read** from the controller after the probe [SRC: no read in either X3 driver].
- BUSY `X3TwoPhase` (`EpdBus.h:16-21`, `EpdBus.cpp:285-307`): wait up to **1000 ms for BUSY LOW**;
  if it went LOW, wait for HIGH (ceiling 30 s). BUSY_N convention: LOW = busy, HIGH = idle.
  `waitRefreshComplete` (`:314-394`): with no host slice hook, attaches a CHANGE interrupt on BUSY;
  if BUSY is not LOW, waits ≤ 20 ms for an edge, then sleeps until BUSY reaches HIGH (30 s
  ceiling). After DRF the drivers also spin ≤ 50 ms for BUSY LOW before returning
  (`Uc8253X3Driver.cpp:249-254`). An emulator must lower BUSY_N on PON, DRF and POF and raise it
  later; otherwise every PON/POF wait costs 1 s [INF].

### 3.2 UC8253 (`Uc8253X3Driver`)

Command constants `Uc8253X3Driver.cpp:11-35`. **Init** (`initController` `:115-155`), each command
then its data bytes: [SRC]

| Cmd | Name | Data |
|---|---|---|
| 0x00 | PSR | `3F 0A` |
| 0x61 | TRES | `03 18 02 58` (= 792 × **600**) |
| 0x65 | GSST | `00 00 00 00` |
| 0x03 | PFS | `20` |
| 0x01 | PWR | `07 17 3F 3F 17` |
| 0x82 | VDCS | `24` |
| 0x06 | BTST | `25 25 3C 37` |
| 0x30 | PLL | `09` |
| 0xE1 | LVSEL | `02` |
| 0x10 | DTM1 | 528 × 99 bytes of `0xFF` (`fillPlane`), then 0x11 DSP |
| 0x13 | DTM2 | 528 × 99 bytes of `0xFF`, then 0x11 DSP |

`begin` = reset(50) then init; one forced full sync armed (`:159-183`). TRES announces 600 lines
while only 528 rows of 99 bytes are ever streamed [SRC]; how the controller places them is [?].
CDI (0x50) always takes two bytes: `29 07` (fast, full, GC, absolute) or `A9 07` (half, normal,
pre-BW) (`:84-86`, `:217-236`). A **LUT bank** = 0x20 VCOM, 0x21 WW, 0x22 BW, 0x23 WB, 0x24 BB,
**42 bytes each** (arrays are 43 bytes, `:52`, `:76-82`); banks in `Uc8253X3Luts.h`: normal, half,
fast, full, gc, aa_pre_bw_mid (`:17-155`), plus a direct-gray bank.

- **PON** 0x04 + wait only when the rails are off; **POF** 0x02 + wait when `turnOff`
  (`triggerRefresh` `:88-102`, `:240-246`).
- **Full** (or first paint after `begin`, or after `requestResync`): CDI `29 07` + full bank, DTM1
  filled `0xFF` + DSP, DTM2 = frame (flipped) (`:222-227`); PON if needed; DRF 0x12; wait. Then
  `delay(200)` (`:279`); on the initial sync one **condition pass**: CDI `A9 07` + normal bank, then
  0x91 PTIN, 0x90 PTL `00 00 03 17 00 00 02 0F 01`, DTM2 frame, 0x92 PTOUT, DRF (`:282-300`). Then
  DTM1 = frame + DSP (`:302-304`), then a settle: CDI `29 07` + fast bank, DTM2 frame, DRF (POF if
  asked), DTM1 frame + DSP (`:314-320`).
- **Half**: CDI `A9 07` + half bank, DTM2 frame, PON?, DRF; finish: `delay(200)`, DTM1 frame + DSP
  (`:228-231`). A wake transition (screen off, no turnOff) is promoted to Half (`:197-199`). CP
  requests one condition pass before every X3 Half (`CP:HalDisplay.cpp:63-65`).
- **Fast**: CDI `29 07` + fast bank, DTM2 frame (DTM1 still holds the previous frame), PON?, DRF;
  finish: DTM1 frame + DSP, no delay (`:232-236`, `:274`).
- **Grayscale** (`:329-597`): LSB plane → DTM1, MSB → DTM2 (both flipped, + DSP), then CDI `29 07` +
  gc bank, DRF; revert = both planes `0xFF`, CDI `A9 07` + half bank, DRF (`:568-584`). Partial
  windows use **gate-flipped Y**: `ystart = 527 − yEnd`, x aligned to 8 (`:388-425`, `:452-482`).
  Direct gray writes PSR `3F 0A` again and uses PTIN/PTL full window.
- **Deep sleep** (`:599-610`): POF + wait if on, then 0x07 DSLP with data `A5`.

### 3.3 UC8279d (`Uc8279Driver`, newer X3 runs)

Same `X3TwoPhase` BUSY and 10 MHz (`Uc8279Driver.h:36-37`). **Init** script `kUc8279X3_Init`
(`Uc8279X3Luts.h:124-134`, replayed by `sendScript`): 0x00 PSR `3F 4A`; 0x91 PTIN; 0x90 PTL
`00 00 03 17 00 00 02 0F 01` (x 0–791, y 0–527, PT_SCAN); 0x03 PFS `20`; 0x01 PWR
`43 00 78 78 17`; 0x82 VDCS `24`; 0x06 BTST `25 25 3C`; 0x30 PLL `0F`; 0xE1 `02`. **No TRES**; the
window replaces it "to avoid the controller's native 800×600 stride" (doc). No plane seed at init;
first two content paints forced to GC (`Uc8279Driver.cpp:101-110`). [SRC]

Refresh (`displayStart` `:125-200`, `displayFinish` `:203-231`): PTIN; DTM1 filled `0xFF` + DSP only
when no old plane is valid; DTM2 = frame (flipped) + DSP; CDI **one byte** `97` (first refresh) or
`D7`; LUT bank GC (`kUc8279X3_BwGc`) or DU (`kUc8279X3_BwDu`), each 5 rows of 43 bytes = register
byte + 42 data (`:53-60`); PON if off; DRF. Finish: wait, CDI `D7`, DTM1 = frame + DSP **inside**
the window, PTOUT, POF if asked. Grayscale banks XtfAa / Xth4 are 5 × 49 bytes written to
0x20–0x24 in remapped orders (`:62-71`); the AA pre-pass adds 0xE0 `02` and 0xE5 `5A`
(`Uc8279X3Luts.h:139-140`). Deep sleep: POF if on, DSLP `A5` (`:247-258`).

### 3.4 Framebuffer layout

- 792 px per row, **99 bytes per row, 528 rows, 52 272 bytes per plane**
  (`FreeInkDisplay.h:83-86`, `Uc8253X3Driver.cpp:63-66`).
- **MSB = leftmost pixel**; **bit 1 = white, bit 0 = black** (`GfxRenderer.cpp:638-644`;
  buffers cleared to `0xFF`, `FreeInkDisplay.cpp:227-230`).
- Every plane write sends **framebuffer row 527 first, row 0 last** (§3.1); controller RAM row *r*
  therefore holds framebuffer row 527 − *r* [SRC]. Both drivers do this; the facade applies no
  mirror (`NO_FLIP`).
- KW mode: DTM1 (0x10) = "old" frame, DTM2 (0x13) = "new" frame; the LUT applied to a pixel is
  chosen by the (old, new) pair (WW, BW, WB, BB). In grayscale DTM1 = LSB, DTM2 = MSB. [SRC
  comments `Uc8253X3Driver.h:8-14`, `Uc8279Driver.cpp` header]

## 4. BQ27220 fuel gauge

Reads only; **no write, no control subcommand, no unseal, no config** anywhere [SRC: no
`writeReg` call on the Bq27220 path in `BatteryMonitor.cpp`; detection is read-only]. All reads are
the §2.1 pattern (register pointer, repeated START, 2 bytes LE, STOP) through `Wire`, begun
lazily with the profile's 20/0/400 kHz (`BatteryMonitor.cpp:30-59`).

| Register | Meaning | Use |
|---|---|---|
| `0x2C` StateOfCharge, u16 % | battery level; clamped to 100 (`:209-251`) | `HalPowerManager::getBatteryPercentage`, cached **1500 ms** (`CP:HalPowerManager.cpp:136-151`, `.h:34`); on failure the last value is kept |
| `0x0C` Current, i16 mA | > 0 = charging (`:283-312`) | X3 "USB connected" = Current > 0, 2 attempts 2 ms apart (`CP:HalGPIO.cpp:245-257`, constants `HalGPIO.h:25-29`); called by `HalGPIO::update()` **every `loop()`** (`:142-147`), by `getWakeupReason`, and by the battery icon |
| `0x08` Voltage, u16 mV | true cell mV, no divider (`:253-270`, `:483-490`) | `readStatus()/readMillivolts()` and the detection probe |

UI: battery outline filled `percentage × width / 100 + 1`, percentage text unless hidden, and a
lightning bolt when `isUsbConnected()` (`CP:BaseTheme.cpp:76-115`, `:323-332`). With no charger IC
there is no external-power reading on X3 (`BatteryMonitor.cpp:315-340`).

## 5. DS3231 RTC

Used: `halClock.begin()` at boot (`CP:main.cpp:397`, `HalClock.cpp:10-13`) → `Rtc::begin`
(`FI:Rtc.cpp:91-120`): read `0x0F` (status, 1 byte) — must ACK or the clock is "absent"; then
write `0x0E` = `0x04` (INTCN, square wave off). OSF is **not** checked in `begin`. [SRC]

Reading (`Rtc::now` `:153-171`): read `0x0F`; if bit 7 (**OSF**) is set → returns false (time
unreliable, nothing shown, `HalClock::localTime` returns false while no time was ever cached,
`HalClock.cpp:37-53`). Else read 7 bytes from `0x00`: seconds `&0x7F`, minutes `&0x7F`, hours
(bit 6 = 12 h mode: `&0x1F`, bit 5 = PM, 12 → 0; else `&0x3F` 24 h), day-of-week `&0x07` taken
`% 7` (7 = Sunday → 0), date `&0x3F`, month `&0x1F` (**century bit 7 ignored**), year = 2000 + BCD.
Polled at most every **10 s** (`HalClock.h:19`); the value is UTC, converted with the POSIX TZ.

Writing: only `HalClock::syncFromNTP()` (WiFi + SNTP, `HalClock.cpp:79-128`, from
`ClockSyncActivity.cpp:67`): `Rtc::set` (`Rtc.cpp:235-257`) writes from `0x00` seven bytes —
BCD sec, min, hour (24 h, bit 6 clear), weekday 1–7 (Sunday = 7), date, month (no century bit),
year % 100 — then reads `0x0F` and writes it back with bit 7 cleared. [SRC]

Display: header clock off by default (`clockShowInHeader = 0`, `CrossPointSettings.h:252`),
status-bar clock off by default (`statusBarClock = STATUS_BAR_CLOCK_HIDE`, `:239`); shown when
enabled and `halClock.isAvailable()` (`BaseTheme.cpp:368-369`, `:863-865`). 24 h default (`:245`).

## 6. QMI8658 IMU

Used only for **tilt page turn**, off by default (`tiltPageTurn = TILT_OFF`,
`CrossPointSettings.h:350`). No interrupt pin; polling only. [SRC]

`Imu::begin` (`FI:Imu.cpp:119-169`, from `CP:main.cpp:396` via `HalTiltSensor::begin`
`HalTiltSensor.cpp:16-30`): WHO_AM_I `0x00` must read `0x05` at 0x6B, else at 0x6A (`:137-148`);
then writes CTRL7 (`0x08`) = `00`, CTRL1 (`0x02`) = `40` (address auto-increment, little endian),
CTRL2 (`0x03`) = `06` (±2 g, ODR code 6), CTRL3 (`0x04`) = `56` (±512 dps, ODR code 6),
CTRL7 = `03` (accel + gyro on) (`:150-154`, constants `:31-56`). On failure: CTRL7 `00`, CTRL1 `41`.
No soft reset (`0x60`) and no REVISION read. CP immediately puts it to sleep: CTRL7 `00`, CTRL1
`41` (sensor disable) (`HalTiltSensor.cpp:21-25`, `Imu.cpp:108-115`, `:224-237`). Wake: CTRL1 `40`,
CTRL7 `03` (`:240-254`).

Read (`Imu.cpp:171-222`): 6 bytes from `0x3B` (gyro X/Y/Z, i16 LE, 1/64 dps per LSB) then 6 bytes
from `0x35` (accel, 1/16384 g). Tilt logic (`HalTiltSensor.cpp:65-144`, `.h:33-37`): only in the
reader, 20 Hz poll, 300 ms after wake ignored; gyro X (portrait) or Y (landscape) above
**270 dps** = page forward/back, 600 ms cooldown, must fall below 50 dps before the next trigger.

## 7. I2C layer

- API: Arduino `Wire` (bus 0; the C3 has one controller, `BatteryMonitor.cpp:30-37`). With IDF
  5.5.1 ≥ 5.4, AE 3.3.1 compiles `esp32-hal-i2c-ng.c` on the **new `driver/i2c_master.h`**
  (`ng.c:17-29`); the legacy `driver/i2c.h` file is compiled only for IDF < 5.4
  (`esp32-hal-i2c.c:19`). Bus config: default clock source, glitch filter 7, **internal pull-ups
  enabled** (`ng.c:118-131`). [SRC]
- Frequency 400 kHz everywhere (§1). Timeout: 6 ms during detection (`XteinkDetect.cpp:646`),
  else Wire's default 50 ms (`Wire.cpp:44`).
- NACKs are tolerated everywhere: each caller returns false and the feature is reported absent
  (probe score, `Rtc::begin`, `Imu::begin`, gauge reads keep the last value).
- Before `setup()`: nothing. The first I2C traffic is the detection probe in `gpio.begin()`
  (only when `dev_det`/`dev_ovr` are unset); on a cached boot it is `isUsbConnected()` inside
  `getWakeupReason()` (`CP:main.cpp:382`) — at that moment **`Wire` is not begun** (the probe ended
  with `Wire.end()`, nothing else begins it before `halTiltSensor.begin()`), so `endTransmission`
  fails on its NULL buffer (`Wire.cpp:452-455`) and the X3 boots as "no USB" [INF from SRC].
- [INF, unverified] On the **uncached first boot** that same failed call leaves Wire's HAL mutex
  taken: `beginTransmission` takes it (`Wire.cpp:419-429`), `endTransmission` returns 4 before
  giving it back, and the next `Wire.begin()` (in `Imu::begin`) takes the same non-recursive
  mutex with `portMAX_DELAY` (`Wire.cpp:286-291`). `CONFIG_DISABLE_HAL_LOCKS` defaults to `n`
  (`AE:Kconfig.projbuild:160-162`). If the lib-builder sdkconfig keeps locks on, a first X3 boot
  with empty `cphw` would hang in `halTiltSensor.begin()`; a cached boot would not. An emulator
  can pre-seed `cphw/dev_det = 2` to avoid depending on this.

## 8. Boot sequence on an X3 (CP 1.6.5)

`CP:main.cpp:349-582`, in order: [SRC unless tagged]

1. `holdPowerRails()` with the **X4** default profile: `gpio_hold_dis(13)`, GPIO13 output HIGH
   (`main.cpp:350`, `BoardConfig.h:2045-2076`). On an X3 this powers the SD rail. [SRC + INF]
2. `Serial.begin` (log builds), `HalSystem::begin()`.
3. `gpio.begin()`: NVS `cphw` / I2C fingerprint (§2.1); `selectDevice(XteinkX3)` (re-asserts latches:
   none on X3); NVS `hw_calib/screenType` read; bit-banged VER probe (§2.2), possibly
   `XteinkX3Uc8279`; `SPI.begin(8, 7, 10, 21)`; `InputManager::begin`: GPIO1/2 `INPUT`, GPIO3
   `INPUT_PULLUP`, ADC attenuation 11 dB (`InputManager.cpp:92-99`). Needs: answers of §2 or a
   pre-seeded NVS.
4. `powerManager.begin()`: `pinMode(0, INPUT)` (X3 SCL), CPU frequency noted.
5. `getWakeupReason()` (`HalGPIO.cpp:282-303`): deep-sleep reset + GPIO wake → PowerButton;
   POWERON + no USB → PowerButton (`coldBootImpliesPowerButton` true for Xteink, `:270-280`);
   USB is read as absent at this point on X3 (§7), so **every X3 cold boot is classified
   PowerButton** [INF]. Then `verifyPowerButtonWakeup()`: GPIO3 must be LOW at the first sample and
   still LOW after ≥ 10 ms of debounced polling (`:225-243`).
6. Recovery check (Up + power), `halTiltSensor.begin()` (§6), `halClock.begin()` (§5): both
   failures tolerated (log only).
7. `Storage.begin()` → `SDCardManager::begin` (`SDCardManager.cpp:108-195`): GPIO13
   `gpio_hold_dis`, HIGH, **10 ms**; display CS 21 HIGH; `SPI.begin(8, 7, 10, 12)`;
   `sd.begin(CS 12, 40 MHz)`, one attempt. **Failure** → display brought up, "SD card error" screen,
   `setup()` returns (`main.cpp:407-412`): no home screen.
8. State/settings loaded from SD. If the wake was PowerButton but not held and `shortPwrBtn` is not
   SLEEP (default `IGNORE`, `CrossPointSettings.h:267`) → deep sleep (`main.cpp:452-461`).
   `AfterUSBPower` (not reachable on X3 per step 5 [INF]) also sleeps on C3 (`:462-477`).
9. `setupDisplayAndFonts()` (`:499`) → `HalDisplay::begin` → `setDisplayX3()`
   (`FreeInkDisplay.cpp:90-103`) → `FreeInkDisplay::begin` (`:193-233`): bus begin, reset
   10/10/10 + 50 ms, controller init (§3). `HalDisplay::begin` calls `getWakeupReason()` again (now
   with Wire up) to decide a resync (`CP:HalDisplay.cpp:30-35`).
10. Splash (`goToBoot`, `:525`) unless resuming, then `goHome` (`:554`) or the reader. Home shows
    the battery from `0x2C`/`0x0C` (§4).

**No hold-to-boot timer** beyond step 5's ~10 ms check; the 400 ms value is the hold-to-sleep
duration in `loop()` (`CrossPointSettings.h:389-390`, `main.cpp:743-749`). **No power latch on X3**
(profile `power` empty). Power-off = deep sleep only (`enterDeepSleep` `main.cpp:262-301`):
IMU sleep, panel POF + DSLP `A5`, SD unmount, then `HalPowerManager::startDeepSleep`
(`HalPowerManager.cpp:69-134`): GPIO13 driven **LOW and held** (the comment there calls it the
"battery MOSFET on both Xteink C3 boards", which contradicts `BoardConfig.h:915-921` calling it the
X3 SD rail — the code path is the same either way); `powerDownRailsForSleep` holds RST (GPIO5)
**HIGH** (no panel power switch) and GPIO13 LOW (`PowerManager.cpp:93-116`); then
`deepSleepUntilPowerButton` (`:140-144`): wait for GPIO3 release (50 ms polls), GPIO3
`INPUT_PULLUP`, `esp_deep_sleep_enable_gpio_wakeup(1<<3, LOW)` (`:34-55`),
`esp_sleep_config_gpio_isolate`, `gpio_deep_sleep_hold_en`, `esp_deep_sleep_start` (`:118-139`). Whether the X3 hardware also cuts the battery is [?].

## 9. Buttons and orientation

- **Ladder identical to X4**: same pins and thresholds, no X3 branch. Decode
  `ranges[i+1] < raw ≤ ranges[i]` → button *i* (`InputManager.cpp:136-144`) with
  `ADC_RANGES_1 = {3900, 3100, 2090, 750, INT32_MIN}` and `ADC_RANGES_2 = {3900, 1120, INT32_MIN}`
  (`:43-44`, `ADC_NO_BUTTON = 3900` `InputManager.h:440`): GPIO1 (3100, 3900] Back, (2090, 3100]
  Confirm, (750, 2090] Left, ≤ 750 Right; GPIO2 (1120, 3900] Up, ≤ 1120 Down; > 3900 = none
  (measured means Back 3512, Confirm 2694, Left 1493, Right 5, Up 2242, Down 5, `:36-37`). "X3 and
  X4 share this pinout" (`InputManager.h:256-260`). Power GPIO3 active-LOW. [SRC]
- **Physical layout differs**: `hasEdgeSideButtons()` is true for X3 (`HalGPIO.cpp:212-217`); the
  side-button hints draw **Up on the left edge and Down on the right edge**, at y = 155 in
  portrait (`BaseTheme.cpp:248-270`), against "both stacked on the right side" for the X4. Front
  Back/Confirm/Left/Right keep the X4 mapping and remap settings (`CrossPointSettings.h:278-286`).
  Exact positions of the front keys on the X3 shell are [?].
- **Orientation**: panel native 792 × 528 landscape. The UI themes render in **Portrait**
  (`GfxRenderer.cpp:1384`); reader default `orientation = PORTRAIT` (`CrossPointSettings.h:276`).
  Portrait maps logical (x, y) to physical `(phyX = y, phyY = 527 − x)` (`GfxRenderer.cpp:359-389`):
  logical 528 wide × 792 tall, logical top-left lands at framebuffer column 0, row 527. Landscape
  CCW is the identity, Landscape CW is 180°, Portrait-inverted is `(791 − y, x)`. Combined with the
  row reversal of §3.4, controller RAM row *r* = framebuffer row 527 − *r* = logical portrait column
  *r* [INF]. To show the glass upright in portrait, an emulator rotates the framebuffer image
  90° counter-clockwise (framebuffer column → screen row top-down, framebuffer row 527 → screen
  left) [INF]; how the glass's gate/source scan maps controller RAM to physical position (PSR
  `UD`/`SHL` bits of `3F`) is [?].

## 10. What this confirms or corrects in `xteink-hardware.md`

- Confirmed: SDA 20 / SCL 0 / 400 kHz; 0x55 / 0x68 / 0x6B (alt 0x6A); two passes, ≥ 2 hits both
  → X3; NVS `cphw` `dev_ovr`/`dev_det` 0/1/2; GPIO13 SD power active-HIGH; 99 B × 528; VER byte 2
  `0x66` → UC8279, `0xFF` → UC8253; 42-byte UC8253 LUTs on 0x20–0x24; mode 0 MSB first.
- Precision: a probe "hit" needs a plausible value (§2.1), not an ACK; the probe uses hardware
  I2C (`Wire`), not bit-banging; any VER byte 2 other than `0x66` keeps UC8253.
- Correction: the **UC8279d also runs at 10 MHz** in CrossPoint (both X3 profiles set
  `displaySpiHz` 10 MHz); 16 MHz is only the drivers' fallback, never 20 MHz.
- Correction: the X3 VER read samples **while SCLK is HIGH** (after the rising edge), 3 bytes, no
  pull-up — unlike the X4-family probe that samples while SCLK is LOW with a pull-up.
- Addition: the X3 profile carries X4 leftovers (`batteryAdc` 0, `usbDetect` 20) that the code
  bypasses; USB/charging on X3 = BQ27220 Current > 0.
- Battery capacity "~650 mAh" is not in the pinned sources read here [?].

## 11. Open points

1. Whether the UC8253 answers VER at all, and with which bytes; the firmware only needs byte 2 ≠
   `0x66` and BUSY HIGH within 300 ms. Real UC8253 BUSY timings after PON/DRF/POF are not in the
   sources.
2. TRES `792 × 600` with 528 streamed rows: where the 528 rows land in UC8253 RAM, and the meaning
   of PSR's second byte (`0A`, UC8279 `4A`); the scan direction of the X3 glass.
3. The first-boot Wire mutex hang of §7 depends on the lib-builder `sdkconfig`
   (`CONFIG_DISABLE_HAL_LOCKS`) and is unverified; check by running the image or reading the
   pioarduino framework's `sdkconfig` for the C3.
4. X3 power topology: is there a hardware self-latch, and what GPIO13 LOW cuts (SD rail per
   BoardConfig, battery per the HalPowerManager comment). Behaviour of POWERON with USB present
   (classified PowerButton per §8 step 5, [INF]).
5. Physical placement of the front keys on the X3 shell; the X3 bezel insets (profile keeps the
   X4 default 9/3/3/3).
6. QMI8658 axis mounting on the X3 PCB beyond the comment "X axis = left/right in portrait"
   (`HalTiltSensor.cpp:101`); the BQ27220's initial state on a real unit (SoC, current sign at rest).
7. The OEM `hw_calib/screenType` key is only logged on X3; its value on real X3 units is unknown.
