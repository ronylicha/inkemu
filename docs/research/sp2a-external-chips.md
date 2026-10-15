# The external chips of the X4 (SP2a): the SSD1677, the button ladders, the battery

Date 2026-10-15, branch of SP2a from `main` at `194f286`. Tags: **[DS]** read in the datasheet,
**[SRC]** read in the code of a firmware that drives the X4, **[INF]** reasoning, **[HYP]** a value
or behaviour no source gives, chosen here and kept as a parameter or marked `ASSUMPTION:` in the code.
Earlier work this report builds on: `docs/research/xteink-hardware.md` (pins, panel, ADC values, the
contradiction on BUSY). The design that rules on what follows is
`docs/superpowers/specs/2026-10-15-sp2a-chips-design.md`.

## 0. Sources

| Short name | What | Where, pinned |
|---|---|---|
| **DS** | Solomon Systech, *SSD1677 Advance Information*, Rev 1.0, Nov 2018, 47 pages (the PDF holds 46) | Good Display's page "IC Driver SSD1677" (`https://www.good-display.com/companyfile/170.html`), which serves `https://v4.cecdn.yun300.cn/100001_1909185148/SSD1677.pdf`; SHA-256 `4b5cc241f410722e54c4d756424907b90ed988502216ed5ed864dd0df78b7e0c`. Not stored in this repository. Solomon Systech's own page gives the document on request only. |
| **FI** | FreeInk SDK, the hardware layer CrossPoint 1.6.5 builds on | `Free-Ink/freeink-sdk` at `7fcd3b34c108f28c086ade3b433924022dbc0998`, the commit the submodule `freeink-sdk` of `crosspoint-reader/crosspoint-reader` points to (read through the GitHub API on 2026-10-15; CrossPoint's `main` at `bb855ef935826e86c65574b5abbd4eab974d53e0`, latest release 1.6.5). The SSD1677 driver, its bus and the X4 LUTs are identical at FreeInk's head `e91e34e3da3bf87b6c3c896fe1b3d6dda1a8c7c2`; only a LUT of another board differs. |
| **CP** | CrossPoint Reader | `crosspoint-reader/crosspoint-reader` at `bb855ef935826e86c65574b5abbd4eab974d53e0` |
| **PX** | Papyrix Reader, its `docs/ssd1677-driver.md` | `bigbag/papyrix-reader` at `ee071b173e29b1e81fc7e259260077d5ba981b38` |

File paths below are relative to each repository; `FI:Ssd1677Driver.cpp:337` reads "line 337 of
`libs/display/FreeInkDisplay/src/driver/Ssd1677Driver.cpp` in FI".

## 1. Findings

1. **The controller is fully specified at the command level by the datasheet**, except four things
   it leaves out: the meaning of the five frame-rate bytes of the LUT, the waveforms stored in the
   panel's OTP, the durations of BUSY outside the waveform, and what the chip does with a command
   received while BUSY is high (§3, §5). Those four are parameters of the model.
2. **The X4's firmware drives the panel with four update sequences**: `0xF7` (full), `0xFC`
   (fast, display mode 2), `0xD7` (half, a full waveform chosen for a forced 90 °C) and `0xCC`
   (a LUT the firmware wrote itself, for grey levels), plus `0xC0` (power on) and `0x03` (power
   off) (§4). The first three load a waveform from OTP, which no source publishes.
3. **On this panel a positive pixel voltage drives the pixel black.** The factory grey LUT of the
   X4 (FI) only ends on the right colour that way: its black entry ends with VSH1 phases, its white
   entry with VSL phases (§6). The general statement that VSH drives white (PX) is wrong for this
   panel. [INF], from the waveform bytes.
4. **BUSY is active high** (DS: "When Busy is High, … command should not be sent"; FI: "SSD1677:
   busy while HIGH"). PX says low; it is the outlier (`docs/research/xteink-hardware.md` §1.1).
5. **The buttons are two resistor ladders** read at 11 dB (12 dB in ESP-IDF 5) as 12-bit raw codes;
   CP averages Back 3512, Confirm 2694, Left 1493, Right 5 on GPIO1, Up 2242, Down 5 on GPIO2, idle
   above 3900 (§7). **The battery** is read on GPIO0 through a 2:1 divider (§8).

## 2. Interface

- 4-wire SPI, mode 0, MSB first; D/C# low for a command byte, high for data; "the level of D/C#
  should be kept over the whole byte" (DS §6.1.2, Table 6-2). 3-wire (9-bit) mode exists, selected
  by the BS1 pin (DS §6.1.3); the X4 uses 4 wires (FI `EpdBus.cpp`: `cmd` and `data` drive DC).
- Write clock up to 20 MHz, read clock up to 2.5 MHz (DS §2 "Maximum 20MHz for write", §12.1).
  The X4 runs it at 10 MHz (FI `BoardConfig.h:920-922`, `FREEINK_X4_DISPLAY_SPI_HZ`; 40 MHz only
  with an overclock flag). The SPI clock is the bus's, not the controller's: it belongs to SP2d.
- RES# is active low (DS Table 5-2). FI pulses it high 10 ms, low 10 ms, high 10 ms
  (`EpdBus.cpp:202-229`); PX gives 20 / 2 / 20 ms.
- BUSY: see finding 4. FI notes "BUSY assertion can trail MASTER_ACTIVATION by a few microseconds"
  and waits up to 20 ms for it to rise (`EpdBus.cpp:406-419`) [SRC]; no source gives the delay.
  FI also waits on BUSY edges by interrupt (`attachInterrupt(..., CHANGE)`, `EpdBus.cpp:430-450`):
  the model must change BUSY at a known instant, not only answer a poll.

## 3. Datasheet facts the model follows [DS]

### 3.1 RAM

- Two planes of 960×680 bits: B/W (written by `0x24`) and RED (`0x26`). In B/W, 1 is white,
  0 black (Table 7-1, `0x24`). RAM is unaffected by SW reset (`0x12`).
- The LUT used by a pixel is chosen by (RED bit, B/W bit): 00 → LUT0, 01 → LUT1, 10 → LUT2,
  11 → LUT3 (Tables 6-4, 6-5).
- `0x21` (Display Update Control 1) chooses, per plane, normal / "bypass RAM content as 0"
  (`0100`) / inverse (`1000`): A[7:4] for RED, A[3:0] for B/W. POR 00h.
- Window: `0x44` X start/end and `0x45` Y start/end, 10 bits each; X up to 3BFh (959), Y up to
  2A7h (679); "It allows on XEA ≤ XSA" (§8.3–8.4). Counters: `0x4E` X, `0x4F` Y (§8.5).
- Data entry `0x11`: ID[1:0] = Y and X increment (1) or decrement (0), AM = which counter moves
  after each byte (0 = X); POR 03h (§8.2, Table 8-4).
- The X address is a source (pixel) address: the X4 firmware sets 0..799 for 800 pixels, then
  writes 100 bytes per row (FI `Ssd1677Driver.cpp:376-412`; PX: "X addresses are specified in
  pixels, not bytes. The controller does the byte conversion"). One data byte holds 8 pixels, the
  most significant bit first (PX "one bit for each pixel, MSB first"). [SRC]
- Auto write `0x46` (**RED**) and `0x47` (**B/W**): A[7] = the value of the first step, A[6:4] the
  step height (8, 16, 32, 64, 128, 256, 512, 680 gates), A[2:0] the step width (8 … 512, 960
  sources); BUSY high during the fill. FI and PX name them the other way round (`0x46` B/W); both
  send `0xF7` (first value 1, steps the whole RAM) to both, so the inversion does not show. The
  model follows the datasheet.
- §8.5 says "The address counter is not automatically updated when data is read out from the RAM",
  while Table 7-1 (`0x27`) says "Address pointers will advance accordingly". Contradictory.

### 3.2 Gate scan (`0x01`)

MUX[9:0] + 1 gate lines (POR 2A7h, 680), then GD, SM, TB (POR 0). Table 8-2 gives, for 680 MUX,
which RAM row each gate shows: SM=0 GD=0 G*n* → ROW*n*; SM=0 GD=1 swaps pairs; SM=1 GD=0 G0 ROW0,
G1 ROW340, G2 ROW1, G3 ROW341 …; SM=1 GD=1 G0 ROW340, G1 ROW0 …. TB=1 is reserved. The X4 writes
`0xDF 0x01 0x02` (480 lines, SM=1) (FI `Ssd1677Driver.cpp:351-354`; `DRIVER_OUTPUT_SCAN = 0x02`).

### 3.3 Waveform and LUT (§6.6–6.9)

- 10 groups × 4 phases (A–D). TP[nX] = frames of the phase (0 skips it), RP[n] + 1 = repeats of
  the group (1–256).
- VS code per phase: 00 VSS, 01 VSH1, 10 VSL, 11 VSH2 for the source; for VCOM (LUT4) 00 DCVCOM,
  01 VSH1+DCVCOM, 10 VSL+DCVCOM, 11 N/A (Table 6-6).
- Layout of the 112 bytes (Figure 6-6): bytes 0–9 LUT0 (byte *n* = group *n*, phases A–D in bits
  7:6, 5:4, 3:2, 1:0), 10–19 LUT1, 20–29 LUT2, 30–39 LUT3, 40–49 LUT4; 50 + 5*n* .. 54 + 5*n* =
  TP[nA], TP[nB], TP[nC], TP[nD], RP[n]; 100–104 "5 bytes for frame rate"; 105 VGH (`0x03`), 106–108
  VSH1, VSH2, VSL (`0x04`), 109 VCOM (`0x2C`), 110–111 reserved. `0x32` writes bytes 0–104.
- **The frame-rate bytes are not described.** Ten 4-bit fields in five bytes is the obvious reading;
  their order and the frame period of each code are not given. FI's factory LUT uses `0x22` and
  says "50 waveform frames" (`Ssd1677Luts.h:129`); its grey LUT uses `0x8F` (`:36`).
- OTP: 34 waveform settings (WS) and 34 temperature ranges (TR); the LUT is chosen by the
  temperature register and the display mode; the last matching TR wins, the match being
  "Lower < T ≤ Upper"; no match: "display will not be updated" (§6.9). Each WS has a display mode
  1 or 2 (`0x37`). Temperatures are 12-bit two's complement sixteenths of a degree (§6.8.3).

### 3.4 Voltages

| Register | Codes | Value |
|---|---|---|
| `0x03` VGH | 00h = 20 V; 07h–17h = 12 V + 0.5 V per step | no optical effect in the model |
| `0x04` VSH1, VSH2 | 23h–4Bh: 9 V + 0.2 V per step; 8Eh–CEh: 2.4 V + 0.1 V per step (B[7] = 1, "VSH2 … 2.4 V to 8.8 V") | POR 41h (15 V), A8h (5 V) |
| `0x04` VSL | 1Ah–3Ah even: −9 V − 0.5 V per two codes | POR 32h (−15 V) |
| `0x2C` VCOM | 08h–A0h by four: −0.1 V per four codes from −0.2 V | POR 00h; the table prints 84h as −3.2 V where its step gives −3.3 V (a typo, [INF]) |

### 3.5 Display Update Control 2 (`0x22`, POR FFh) and Master Activation (`0x20`)

The datasheet tabulates combinations (80h, 01h, C0h, 03h, 91h, 99h, B1h, B9h, C7h, CFh, F7h, FFh).
They are consistent with one bit per stage, which PX states: 80h clock on, 40h analog on, 20h load
temperature, 10h load LUT, 08h display mode 2 (else 1), 04h display, 02h analog off, 01h clock off.
`0x20` runs the enabled stages; BUSY is high during the operation. [DS] + [SRC] for the bit reading.

### 3.6 Other commands

- `0x10` deep sleep: 00 normal, 11 enter; "BUSY pad will keep output high"; only a hardware reset
  leaves it. FI sends `0x03` (`Ssd1677Driver.cpp:913-914`), PX `0x01`; the SSD1677 datasheet lists
  only 00 and 11. FI: "deep sleep mode 2 (0x03) discards controller RAM" (`:910-912`) [SRC].
- `0x12` SW reset: registers to their reset values except `0x10`; BUSY high during it; RAM kept.
- `0x18`: 48h (POR) external sensor over the controller's own I2C master, 80h internal sensor.
  `0x1A` writes the temperature register (A[11:4], then A[3:0] in the high nibble; POR 7FFh); `0x1B`
  reads it.
- `0x14` HV ready (requires clock and analog; BUSY for at most 10 ms × (n+1) × m), `0x15` VCI
  detection (2.2–2.6 V), `0x28`/`0x29` VCOM sensing ((A[3:0]+1) s), `0x2F` status (bit 5 HV not
  ready, bit 4 VCI low, bit 2 busy, bits 1:0 chip ID = 01).
- `0x3C` border (VBD): A[7:6] 00 follow LUT A[1:0], 01 fixed level A[5:4] (VSS, VSH1, VSL, VSH2),
  10 VCOM, 11 HiZ (POR C0h).
- OTP programming (`0x08`, `0x2A`, `0x30`, `0x36`, `0x39`), OTP load (`0x31`), CRC (`0x34`,
  `0x35`), initial code (`0x09`, `0x0A`), external sensor command (`0x1C`), dithering (`0x25`,
  `0x4D`): described too briefly to model their effect (no CRC polynomial, no dithering input
  format, no OTP image), and never sent by the X4's firmware.
- `0x7F` NOP terminates a RAM write. `0x3A`, `0x3B`: reserved.
- The SSD1677 has no `0x70`/`0x71`; FI's X4-family probe sends them and expects the half-duplex line
  to float (`XteinkDetect.cpp:93-111`) [SRC].

## 4. What the X4's firmware sends [SRC]

From FI `Ssd1677Driver.cpp`, the X4 configuration `ssd1677DefaultConfig` (`:48-76`):

| Step | Bytes | Lines |
|---|---|---|
| Reset | RES# high 10 ms, low 10 ms, high 10 ms | `EpdBus.cpp:202-229` |
| Init | `12`; `delay(10)`; wait BUSY; `18 80`; `0C AE C7 C3 C0 80`; `01 DF 01 02`; `3C 80`; window (below); `46 F7` wait; `47 F7` wait | `:330-374` |
| Window (x, y, w, h) | `11 01` (X increment, Y decrement); `44 x x+w−1` (LE 16-bit each); `45 Y+h−1 Y` with Y = 480 − y − h ("Gates are physically reversed on this panel"); `4E x`; `4F Y+h−1` | `:376-412` |
| Full | `21 40` (RED bypassed as 0); `3C C0`; `22 F7`; `20`; ~1 800 ms | `:419-476`, comment `:53` |
| Fast | `21 00`; `3C C0`; `22 FC`; `20`; ~500 ms; no power-off bits | same |
| Half | `21 40`; `3C C0`; `1A 5A` (90 °C; **one** data byte, where the datasheet lists two: A[11:4], then A[3:0]); `22 D7`; `20` | same, `:468-471` |
| Grey (factory) | `32` + 105 bytes; `03`; `04` ×3; `2C`; `3C C0`; `21 00`; `22 CC`; `20` | `:815-858`, `:871-903` |
| Power on / off | `22 C0`, `20`, wait / `3C 80`, `22 03`, `20`, `delay(200)`, wait | `:554-575` |
| Deep sleep | power off; `10 03` | `:905-915` |

FAST writes the new frame to B/W and the previous one to RED, so LUT1 (RED 0 → B/W 1) is
black-to-white and LUT2 white-to-black: a differential update. FULL and HALF write the frame to both
planes and bypass RED.

Durations given by the sources, all approximate: full ~1.6–1.8 s, fast ~0.5–0.6 s, half ~1.7 s
(FI comments, PX); power on ~100 ms, power off ~200 ms (PX "Timing Specifications"); SW reset
~10 ms is what both firmwares wait, without a source for the chip's own time.

## 5. What no source gives, and what the model does with it

| Unknown | Model | Why |
|---|---|---|
| Frame period of each frame-rate code | a table of 16 periods, all 20 ms by default [HYP] | 50 frames of FI's factory LUT at 20 ms is 1 s, the order of the grey refresh FI and PX describe |
| Order of the ten frame-rate nibbles | byte 100 + *k*: group 2*k* in the high nibble, 2*k*+1 in the low one [HYP] | every LUT of the X4 repeats one byte, so the order cannot show on it |
| OTP waveforms of the GDEQ0426T82 | three reference waveforms built for the model: mode 1 for T ≤ 50 °C (80 frames), mode 1 above (75 frames), mode 2 everywhere (25 frames) [HYP], flagged "approximation" | they reproduce the measured durations of §4 with the stage durations below, and the colours each sequence must produce |
| Durations of the stages | analog on 100 ms, analog off 200 ms (PX); clock on 0, temperature (internal) 5 ms, LUT load 1 ms, SW reset 2 ms, hardware reset 1 ms, auto write 2 ms, VCI detection 1 ms [HYP] | the last ones only need to stay below what the firmwares wait (10 ms after a reset); full = 5 + 1 + 1 600 + 200 = 1 806 ms, fast = 5 + 1 + 500 = 506 ms, half = 1 + 1 500 + 200 = 1 701 ms |
| A command received while BUSY is high | dropped and reported, except `0x2F` (status), executed [HYP] | the datasheet forbids it; a status bit "busy" only makes sense if the status can be read while busy |
| RAM after deep sleep 11 | cleared to 0 [HYP] | FI: discarded; a deterministic value |
| Deep-sleep codes 01, 10 | deep sleep keeping the RAM, reported as a reserved value [HYP] | the SSD1680's mode 1 (PX sends `0x01`) |
| RAM after a hardware reset | kept [HYP] | the datasheet says nothing |
| RAM read: does the counter advance | it advances (Table 7-1) | §8.5 says the opposite; the command table is the more specific |
| Optical response | §6 | |
| Auto write: window or whole RAM | whole RAM [HYP] | the steps are defined on gates and sources, up to 680 and 960 |
| Bit order inside a byte when X decrements | the most significant bit stays at the lowest X of the byte [HYP] | the X4 increments X |
| External temperature sensor of the X4 | none: a temperature load from it keeps the register and is reported [HYP] | the X4's firmware selects the internal sensor (`18 80`) |
| VCOM after `0x28` | unchanged [HYP] | the sensed value depends on the panel |
| `0x1A` with one byte | accepted, A[3:0] kept [HYP] | the X4's half refresh sends one byte and works on the device |
| DCVCOM | drives no pixel [HYP] | §6 |

## 6. Optical model [INF], [HYP]

Each pixel has an optical state between black and white. During each frame of a phase the pixel
sees the voltage of its source minus the swing of VCOM around DCVCOM (LUT4: VSH1, VSL or none);
the state moves in proportion to that voltage and to the duration of the frame, and stops at
black or white. A swing from black to white takes a fixed amount of volt·time, a parameter (6 V·s,
i.e. 6 000 V·ms, by default) [HYP].

DCVCOM itself drives nothing [HYP]: it is set to cancel the kickback of the panel's transistors
(DS §6.4 senses it for that), and were it a drive every VSS phase would move every pixel, while the
grey LUT holds its code-00 pixels with VSS phases only.

The sign follows the X4's factory LUT (`Ssd1677Luts.h:130-147`), whose RAM code 11 is black and 00
white (`Ssd1677Driver.cpp:782-783`): LUT3 (black) runs 21 frames of VSL (groups 0) then 21 of VSH1
(group 1–2), LUT0 (white) 12 of VSH1 then 12 of VSL. Each entry ends on its colour only if a
positive pixel voltage drives black. The grey LUT agrees: its "light grey" entry is 7 frames of
VSH1, applied over a white pixel, its "grey" and "dark grey" entries VSL phases over black ones.

With 20 ms frames, VSH1 15 V and VSL −15 V, a frame moves a pixel by 300 V·ms, 5 % of the default
swing. The overlay grey LUT then darkens white pixels by 35 % ("light grey", 7 frames) and
lightens black ones by 45 % and 30 % ("grey" 9 frames, "dark grey" 6): levels 0.65, 0.45 and 0.30
of white, in the order FI names them. The factory LUT, from white, gives 1.0, 0.6, 0.5 and 0 for
codes 00, 01, 10, 11 (grey levels 255, 153, 128, 0), which the tests pin. That the default swing is
"right" is not established; it is a parameter, and the interface says the panel is an
approximation (spec §5.3).

Each phase is a clamped translation of the state; a sequence of them is again one (`x ↦
min(max(x + a, lo), hi)`), so a whole waveform costs one composition per phase and one evaluation
per pixel, whatever its repeats.

## 7. Buttons [SRC]

- FI `InputManager.cpp:34-49`: recorded raw codes of three devices and their averages: Back 3512,
  Confirm 2694, Left 1493, Right 5 (GPIO1); Up 2242, Down 5 (GPIO2). The decoding thresholds are the
  midpoints: GPIO1 (3900, 3100] Back, (3100, 2090] Confirm, (2090, 750] Left, below Right; GPIO2
  (3900, 1120] Up, below Down; above 3900 no button (`ADC_NO_BUTTON = 3900`, `InputManager.h:472`).
  CP: "The Xteink ladder idles at the ADC full-scale rail (~4095)" (`lib/hal/HalGPIO.cpp:140-142`).
- Read with `analogRead` after `analogSetAttenuation(ADC_11db)` (`InputManager.cpp:109`): raw 12-bit
  codes, uncalibrated, at the attenuation ESP-IDF 5 calls 12 dB.
- Power is a digital key on GPIO3, active low (FI `BoardConfig.h`, X4 input tuple), not on a
  ladder: it is SP2b's GPIO, not this crate's.
- Two keys of one ladder pressed together: no source. A series ladder gives the voltage of the key
  nearest to ground, the lowest code [HYP].

**In volts.** The ladder is an analog source: what it imposes is a voltage, which the SAR ADC (SP2b)
converts according to the attenuation the firmware chose. With the nominal range of the ESP32-C3 at
12 dB, 0–2 500 mV over 0–4095 (ESP-IDF documentation, ADC oneshot, "ESP32-C3") [HYP for the
linearity], the averages above are 2 144 mV (Back), 1 645 (Confirm), 911 (Left), 3 (Right), 1 369
(Up), 3 (Down); idle is the pull-up rail, 3 300 mV, which saturates to 4095. These are the values
the X4 profile is expected to declare (SP2d), each one rounding back to its code under that line.

## 8. Battery [SRC]

`analogReadMilliVolts(GPIO0)` × 2.0 (`BatteryMonitor.cpp:740-747`; X4 `batteryDividerMultiplier`
2.0, `BoardConfig.h:936`), the divider of two 10 kΩ resistors on the schematic
(`docs/research/xteink-hardware.md` §1.1). Full charge ~4.2 V, empty ~3.45 V for 0 % in FI's notch
table (`BatteryMonitor.cpp:862-875`). The model is a cell voltage, set by the user or a script,
and a divider: the pin sees cell × R_bottom / (R_top + R_bottom).
