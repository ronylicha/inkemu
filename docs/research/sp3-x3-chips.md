# The external chips of the X3 (SP3): the UC8253 and UC8279 panel controllers, the BQ27220, the DS3231, the QMI8658

Date 2026-10-15, branch from `main` at `6691a7d`. Tags: **[DS]** read in a datasheet, **[SRC]** read in
the code or documentation of a firmware that drives the X3 (repository, commit, file, line), **[INF]**
reasoning, **[?]** unknown: no source found; a model must make it a parameter or an `ASSUMPTION:`.
Earlier work this report builds on: `docs/research/xteink-hardware.md` §1.2 (X3 pins, I2C bus, the two
controller batches) and `docs/research/sp2a-external-chips.md` (conventions, the X4's SSD1677).

## 0. Sources

| Short name | What | Where, pinned, how obtained |
|---|---|---|
| **DS53** | UltraChip, *UC8253 All-in-one driver IC w/ Timing Controller*, "UC8253c_A0.6 (for TFT_module_Use_Only)", Rev 0.6, 13 Oct 2020, 75 pages | `https://www.elecrow.com/download/product/DIE01237S/UC8253_Datasheet.pdf` (the URL FreeInk and Papyrix cite), downloaded 2026-10-15, SHA-256 `bee25538177b8cc6f67c1ffe696d8fd09bda7842b42e021fb9bd41a3da91c125`, text by `pdftotext -layout`. Not stored here. |
| **DS79** | UltraChip, *UC8179 …*, "UC8179c_C0.6 (for_TFT_Module_Use_Only)", Rev 0.6, 2019, 81 pages | `https://www.crystalfontz.com/controllers/uploaded/UC8179.pdf` (Crystalfontz's UltraChip page), downloaded 2026-10-15, SHA-256 `bf1dcef76e0d02a5683814e5f2b525bb7079e57509bf9a0754235b24fa587f50`. Used because the X3's controllers speak its command layout (§1, finding 1). The Orient Display copy answered 403. |
| — | UC8279 / UC8279d datasheet | **Not public.** A web search on 2026-10-15 found none. Papyrix cites "UC8279d_B datasheet 0.1 (December 2025)" without a link; its facts reach this report only through PX/FI below, tagged [SRC]. |
| **BQDS** | TI, *bq27220 Single-Cell CEDV Fuel Gauge*, SLUSCB7A, Mar 2016 rev. Apr 2016, 24 pages | `https://www.ti.com/lit/ds/symlink/bq27220.pdf`, 2026-10-15, SHA-256 `9fffe0b632cd287c9210aa1d2e71af1816b31764c11f1b62425de3a4a7cfe5f4` |
| **BQTRM** | TI, *BQ27220 Technical Reference Manual*, SLUUBD4A, Apr 2016 rev. Nov 2022, 91 pages | `https://www.ti.com/lit/ug/sluubd4a/sluubd4a.pdf`, 2026-10-15, SHA-256 `44779f229423d649e410439d1d1a3a65026d5919cdd43112ee3a3d8d9811ead9` |
| **DS31** | Maxim, *DS3231 Extremely Accurate I2C-Integrated RTC/TCXO/Crystal*, 19-5170 Rev 10, 3/15, 20 pages | Adafruit's mirror `https://cdn-shop.adafruit.com/product-files/3013/DS3231.pdf`, 2026-10-15, SHA-256 `5ead8440e491059cf9fa20d0c1bd00793dcefef014d7cbdd55577417012d4c82`. The analog.com URL timed out twice. |
| **QMC** | QST, *QMI8658C Datasheet*, Rev 0.6, 13 Jan 2021, 63 pages ("Advance information") | Waveshare's mirror `https://files.waveshare.com/upload/5/5f/QMI8658C.pdf`, 2026-10-15, SHA-256 `5518afa3ec3a5c3b05148c9d16c26bebfc34658b01f2099e7c80fb36ac30a305`. No QMI8658A datasheet was found in public. |
| **FI** | FreeInk SDK (hardware layer of CrossPoint) | `Free-Ink/freeink-sdk` at `3af4166a36aa51a743e26a528d9c570106753a98` (head, committed 2026-10-15T03:42:56Z), tarball through the code host's API. Paths below are relative to `libs/`. Not checked against the FreeInk commit CrossPoint 1.6.5 builds on (`111fdcc7`, sp2a §0): the X3 path of the release may differ. |
| **PX** | Papyrix Reader | `bigbag/papyrix-reader` at `ee071b173e29b1e81fc7e259260077d5ba981b38` (2026-10-07), the commit sp2a read. |
| **SL** | SensorLib (Lewis He), QMI8658 driver | `lewisxhe/SensorLib` at `477fc682e9ed30ced39774f3b7e93a1504968884` (2026-09-28), files `src/sensor/imu/qmi8658/SensorQMI8658_Reg.hpp` and `SensorQMI8658.cpp`. Used for the QMI8658A details the C datasheet lacks. |

`FI:display/FreeInkDisplay/src/driver/Uc8253X3Driver.cpp:119` reads "line 119 of that file in FI".

## 1. Findings

1. **The X3's "UC8253" is not the UC8253 of the only public datasheet.** DS53 describes a 240 source ×
   480 gate KWR controller (PSR `RES[1:0]` up to 480×240, 3-byte TRES with `HRES[7:3]`, `VRES[8:0]`,
   7-byte PTL, 43/57-byte LUT commands) [DS]. The X3 firmware sends a 4-byte TRES `03 18 02 58`
   (HRES 792, VRES 600), a 9-byte PTL with 10-bit fields, a 4-byte BTST and a 2-byte CDI
   (`FI:display/FreeInkDisplay/src/driver/Uc8253X3Driver.cpp:115-147`) [SRC]: the layout of the UC8179
   (800×600 max, DS79 command table) [DS]. 792×528 fits only that larger family [INF]. **The model of
   both X3 controllers follows DS79's command layout**, with the differences §2.4 lists.
2. **BUSY_N is active low on both controllers**: "L: Driver is Busy. H: Host side can send command/data"
   (DS53 pin table; same in DS79) [DS]; PX and FI agree (`PX:docs/x3-specifications.md`, "LOW = busy";
   `FI:display/FreeInkDisplay/src/bus/EpdBus.cpp:359-381`, `X3TwoPhase`: wait for the LOW edge, then HIGH)
   [SRC]. This is the opposite of the X4's SSD1677.
3. **Identification by REV (0x70).** The stock X3 firmware and FreeInk read **three** bytes after `0x70`
   with SDA released, and select the UC8279 when the **third byte is `0x66`**, the UC8253 when it is
   `0xFF` (`FI:hardware/XteinkDetect/src/XteinkDetect.cpp:182-250`) [SRC]. The X3's UC8253 does not drive
   SDA on reads: everything reads `0xFF` through the pull-up, "field-confirmed" (`PX:docs/x3-uc8279-driver-reference.md`)
   [SRC]. The datasheet default of REV is 5 bytes on DS53 (reserved, `CHIP_REV` `09`, `LUT_REV` `FF FF FF`;
   the reserved byte is `00` in its command table, `FF` in its description) and 7 bytes on DS79 (`FF`×6,
   `CHIP_REV` `0C`) [DS]; neither is what the X3 sees.
4. **The four I2C chips** sit on SDA GPIO20 / SCL GPIO0 at 400 kHz: BQ27220 `0x55`, DS3231 `0x68`,
   QMI8658 `0x6B` (alternate `0x6A`) (`FI:hardware/BoardConfig/include/BoardConfig.h:989-991`) [SRC]. The
   X3/X4 decision at boot reads one plausible value from each and wants at least two of three on two
   passes (§3.4, §4.4, §5.4).
5. **Pull-ups of the board** (added 2026-10-15, review of PR #22). *I2C:* no schematic, board
   photograph or firmware source found gives the resistors on SCL and SDA [?]. Every firmware also
   turns the pads' own pull-ups on (Arduino `Wire`, `esp32-hal-i2c-ng.c:131`
   `enable_internal_pullup = 1`; the stock firmware is not readable), which proves nothing about the
   board. An I2C bus at 400 kHz needs pull-ups of a few kΩ to rise within the I2C specification's
   300 ns, which the pads' ~45 kΩ do not give (ESP32-C3 datasheet, DC characteristics) [INF]: the
   profile models board pull-ups on both lines as an **ASSUMPTION** (`[wiring.i2c] pull_up`).
   *Display data line:* the field note of point 3 ("uniform `0xFF` through the pull-up") is read as a
   pull-up on the board: FreeInk's probe turns the pin into an input without a pull of its own
   (`FI:XteinkDetect.cpp:221-225`), and a line that floats low would read `00` [INF from SRC]; the
   resistor's value is not known (`[wiring.display] mosi_pull_up`).

## 2. UC8253 (X3, first batch) and UC8279d (X3, later batch)

### 2.1 Interface [DS79, DS53 unless tagged]

- 4-wire SPI selected by `BS` low; D/C low = command byte, high = data; 8-bit units; data written MSB
  first on the rising SCL edge; "the serial counter is reset at the rising edge of CSB" (DS79 "Host
  interfaces"). "Commands are processed on the 'stop' condition of the interface" (DS79/DS53 table note 3).
- **Excess parameters are ignored**: "When too many parameters are input, the chip accepts only defined
  parameters, and ignores undefined ones" (DS79). The X3 sends a second PSR byte that DS79 does not
  define (§2.4).
- **Reads (4-wire)**: "The MSB bit of data will be output at SDA pin after the CSB falling edge, if DC pin
  is High" (DS79); SDA is half-duplex, the X3 has no display MISO. The stock X3 probe writes `0x70` with
  DC low, sets DC high, releases SDA without pull-up, then clocks 24 bits and samples after each SCLK
  rising edge, about 500 kHz bit-banged (`FI:…/XteinkDetect.cpp:219-237`; `PX:docs/x3-uc8279-driver-reference.md`) [SRC].
- **Clock**: "Clock rate up to 20 MHz" (DS53/DS79 features); DS79 AC table: write cycle ≥ 100 ns, read
  cycle ≥ 150 ns. The X3 profiles run 10 MHz for both batches (`FI:…/BoardConfig.h:969,1008`); PX says
  UC8253 10 MHz, UC8279d 20 MHz (`PX:docs/x3-specifications.md`) [SRC]. Bus speed belongs to the SPI
  model, not to the controller.
- **Reset**: `RST_N` low resets every register to default; minimum low width 50 µs (DS53 pins). After
  `RST_N` rises, >1 ms of internal initialisation during which "any commands … will be ignored" (DS79
  power management). FI pulses high 10 / low 10 / high 10 ms then settles 50 ms
  (`FI:display/FreeInkDisplay/src/bus/EpdBus.cpp:202`, `Uc8253X3Driver.cpp:160`, `Uc8279Driver.cpp:115`);
  the stock probe high 10 / low 50 / settle 50 ms and waits ≤300 ms for BUSY high (`XteinkDetect.cpp:195-215`) [SRC].
- **Deep sleep** (`0x07` + check code `0xA5`; executed only if the code is `0xA5`); "Deep Sleep Mode will
  return to Standby Mode by hardware reset" — `RST_N` low is the only wake (DS53, DS79).

### 2.2 Command table (DS79 layout; defaults DS79; DS53 differences in the last column)

| Code | Name | Data bytes (DS79) | Default | DS53 (small UC8253) | X3 sends [SRC] |
|---|---|---|---|---|---|
| 00 | PSR | 1: `-,-,REG,KW/R,UD,SHL,SHD_N,RST_N` | 0F | 2: `RES1,RES0,REG,KW/R,UD,SHL,SHD_N,RST_N` (0F), `VCMZ,TS_AUTO,TIEG,NORG,VCM_LUTZ` (8D) | UC8253 `3F 0A`; UC8279 `3F 4A` |
| 01 | PWR | 5: `BD_EN,VSR_EN,VS_EN,VG_EN`; `VCOM_SLEW,VG_LVL[2:0]`; `VDH_LVL[5:0]`; `VDL_LVL[5:0]`; `VDHR_LVL[5:0]` | 07 17 3A 3A 03 | 5, other fields | UC8253 `07 17 3F 3F 17`; UC8279 `43 00 78 78 17` |
| 02 | POF | — | | same | |
| 03 | PFS | 1: `T_VDS_OFF[1:0]` at bits 5:4 (1–4 frames) | 00 | same | `20` |
| 04 | PON | — | | same | |
| 05 | PMES | — (bandgap on until next POF) | | same | |
| 06 | BTST | 4: phase A, B, C1, `PHC2EN`+C2 | 17 17 17 17 | 3 bytes | UC8253 `25 25 3C 37`; UC8279 `25 25 3C` |
| 07 | DSLP | 1: check code A5 | | same | `A5` |
| 10 | DTM1 | n pixel bytes: KW mode OLD plane, KWR mode K/W plane | | same | |
| 11 | DSP | read 1: `data_flag` in bit 7 | 00 | same | after each plane |
| 12 | DRF | — | | same | |
| 13 | DTM2 | n pixel bytes: KW mode NEW plane, KWR mode RED plane | | same | |
| 15 | DUSPI | 1: `MM_EN, DUSPI_EN` | 00 | absent | not sent |
| 17 | AUTO | 1: `A5` = PON→DRF→POF, `A7` = PON→DRF→POF→DSLP | | `A5` only | not sent |
| 20 | LUTC (VCOM) | 60 (10 groups × 6); KW mode uses 7 groups | — | 57/43 bytes, other format | 42 |
| 21 | LUTWW | 42 (7 × 6) | — | | 42 |
| 22 | LUTKW / LUTR (K→W) | 60; KW mode 7 groups | — | | 42 |
| 23 | LUTWK / LUTW (W→K) | 60; KW mode 7 groups | — | | 42 |
| 24 | LUTKK / LUTK | 60; KW mode 7 groups | — | | 42 |
| 25 | LUTBD (border) | 42 | — | absent | not sent |
| 2A | LUTOPT | 2: `STATE_XON[9:0]` (all-gate-on per state) | 00 00 | 5 bytes | not sent |
| 2B | KWOPT | 3: `ATRED,NORED`; `KWE[9:0]` (KWR mode only) | 00 00 00 | absent | not sent |
| 30 | PLL | 1: `FRS[3:0]`: 0=5 Hz … 9=80 Hz … F=200 Hz | 06 (50 Hz) | `FRS[4:0]`: 0=5 Hz, step 5 Hz, 9=50 Hz … 1F=200 Hz | UC8253 `09`; UC8279 `0F` |
| 40 | TSC | read 2: temperature `D[10:3]`, `D[2:0]` | | same | |
| 41 | TSE | 1: `TSE, TO[3:0]` | 00 | same | |
| 42 | TSW | 3: external sensor write | | same | |
| 43 | TSR | read 2 | | same | |
| 44 | PBC | read 1: `PSTA` | | same | |
| 50 | CDI | 2: `BDZ,-,BDV[1:0],N2OCP,-,DDX[1:0]`; `CDI[3:0]` | 31 07 | 1: `VBD[1:0],DDX[1:0],CDI[3:0]` (D7) | UC8253 `29 07` / `A9 07`; UC8279 one byte `97` first refresh, `D7` after |
| 51 | LPD | read 1 | 01 | same | |
| 52 | EVS | 1: `VCEND, BDEND[1:0]` | 02 | absent | |
| 60 | TCON | 1: `S2G[3:0], G2S[3:0]` | 22 | same | |
| 61 | TRES | 4: `HRES[9:8]`, `HRES[7:3]`, `VRES[9:8]`, `VRES[7:0]` | 03 20 02 58 (800×600) | 3 bytes | UC8253 `03 18 02 58` (792×600); UC8279 none |
| 65 | GSST | 4: `HST[9:3]`, `VST[9:0]` | 00 00 00 00 | 3 bytes | UC8253 `00 00 00 00` |
| 70 | REV | read 7: `PROD_REV[23:0]`, `LUT_REV[23:0]`, `CHIP_REV` | FF×6, 0C | read 5: reserved FF, `CHIP_REV` 09, `LUT_REV` 3 bytes FF (LSB first) | probe only (§2.6) |
| 71 | FLG | read 1 (bits below) | 13 | same | PX probe |
| 72 | CRC | read 2 | | DS53 only | |
| 80 | AMV | 1 | 10 | same | |
| 81 | VV | read 1: `VV[6:0]` | | same | |
| 82 | VDCS | 1: `VDCS[6:0]` | 00 | same | `24` |
| 90 | PTL | 9: `HRST[9:8]`,`HRST[7:3]`; `HRED[9:8]`,`HRED[7:3]` (low 3 bits written 111); `VRST[9:0]`; `VRED[9:0]`; `PT_SCAN` | 00 00 03 1F 00 00 02 57 01 | 7 bytes, 8-bit H, 9-bit V | `00 00 03 17 00 00 02 0F 01` (0–791 × 0–527) |
| 91 | PTIN | — | | same | |
| 92 | PTOUT | — | | same | |
| A0 / A1 / A2 | PGM / APG / ROTP | ROTP: read, dummy byte then OTP from 0x000 | | same | PX probe reads 0xA2 |
| E0 | CCSET | 1: `TSFIX, CCEN` | 00 | same | UC8279 `02` (grey pre-pass only) |
| E1 | — | not in DS79 or DS53 ("all other register addresses are invalid") | | | both send `02`; FI names it "LV selection" / "gate scan" [?] |
| E3 | PWS | 1: `VCOM_W[3:0], SD_W[3:0]` | 00 | same | |
| E4 | LVSEL | 1: `LVD_SEL[1:0]` | 03 | same | |
| E5 | TSSET | 1: forced temperature | 00 | same | UC8279 `5A` (grey pre-pass only) |
| E7 | TSBDRY | 1 | 00 | absent | |

X3 sources: `FI:display/FreeInkDisplay/src/driver/Uc8253X3Driver.cpp:115-147,224-235` and
`FI:display/FreeInkDisplay/src/lut/Uc8279X3Luts.h:124-140`; PX's script is identical
(`PX:docs/x3-uc8279-driver-reference.md`). The UC8279 bytes of PWR, PSR byte 2 and BTST do not decode with
DS79 (`43` sets bit 6, undefined there): its register layout is unknown [?].

**FLG (0x71)** bits (DS79, DS53): 6 `PTL_flag`, 5 `I2C_ERR`, 4 `I2C_BUSYN` (low active), 3 `data_flag`,
2 `PON`, 1 `POF`, 0 `BUSY_N` (1 = idle). Default `0x13` (I2C idle, powered off, not busy) [DS]. After PON a
plausible value is `0x15` [INF]; in partial mode bit 6 is set [DS].

### 2.3 BUSY_N: which commands drive it, for how long

- DS79 "BUSY_N signal" table: commands that pull BUSY_N low: **POF, PON, PMES, DSLP, AUTO, DSP, DRF,
  TSC, LPD, AMV, APG**; every other command leaves it alone [DS]. PON: "When all voltages are ready, the
  BUSY_N signal will return to high" [DS]. PSR soft reset (`RST_N=0`): BUSY_N low ≥ 50 µs, commands ignored
  (DS53) [DS].
- **While a refresh runs, only REV, FLG and VV are accepted; every other command is ignored** (DS79 table,
  "Refresh Restriction: V accepted, X ignored") [DS]. PX/FI: writing RAM during a waveform "corrupts the
  frame" (`PX:docs/x3-uc8279-driver-reference.md`) [SRC].
- Durations, none in a datasheet: PON ≈ 127 ms, DRF = waveform length (UC8253 full ≈ 472 ms, turbo ≈ 382 ms,
  image ≈ 908 ms, grey ≈ 127 ms) (`PX:docs/x3-lut-waveforms.md`) [SRC]; POF ≈ `T_VDS_OFF` + a few frames [?].
  FI waits ≤ 50 ms after DRF for the LOW edge, ≤ 1 s for it in `X3TwoPhase`, then ≤ 30 s for HIGH
  (`Uc8253X3Driver.cpp:245-254`, `EpdBus.cpp:359-376`) [SRC].
- **A PON sent while already powered gives no BUSY low edge** on the X3's UC8253 (FI comment,
  `Uc8253X3Driver.cpp:238-239`) [SRC]: the driver skips it, and a model must not invent a pulse there.
- DSP is in the BUSY table and, per the datasheet, "after DTM1/DSP and when data_flag = 1, the refreshing of
  panel starts" (DS79 DSP) [DS]. Every X3 driver sends DSP after each plane and then DRF itself, without
  waiting on BUSY after DSP [SRC]. Whether DSP really starts a refresh in register-LUT KW mode is [?]; the
  model treats DSP as "set `data_flag`, no refresh", warns once, and keeps the DS sentence as an open point.

### 2.4 Pixels, planes and memory

- **Bit order**: in a DTM byte, D7 is pixel 1 and D0 pixel 8 (DS79/DS53 DTM tables): MSB = first pixel in
  source order; with `SHL=1` source order is S0 → Sn-1 [DS].
- **Polarity**: decided by CDI `DDX`. In KW mode with `DDX[1]=0` (old/new), DS79 maps `{NEW,OLD}` to a LUT:
  `DDX=00`: 00→WW, 01→KW, 10→WK, 11→KK; `DDX=01` (default): 00→KK, 01→WK, 10→KW, 11→WW [DS]. Every X3 CDI
  value has `DDX=01` (`0x29`/`0xA9` in DS79 layout, `0x97`/`0xD7` in DS53 layout) [SRC]→ **1 = white,
  0 = black** on both planes, which PX states (`x3-uc8279-driver-reference.md`, "1 = white. 0 = black") [SRC].
- **Old/new**: in KW mode DTM1 writes OLD, DTM2 writes NEW (DS79/DS53) [DS]. **DRF picks, per pixel, the
  LUT of its old→new transition**: register 0x21 WW (white stays white), 0x22 KW (black→white), 0x23 WK
  (white→black), 0x24 KK (black stays black); 0x20 drives VCOM [DS]. PX calls 0x22 "BW" and 0x23 "WB" with
  the same meaning [SRC].
- **No automatic copy**: CDI bit 3 `N2OCP` copies NEW to OLD after a refresh when 1 (DS79) [DS]; the X3
  values leave it 0 [SRC] and the drivers rewrite DTM1 themselves after each refresh
  (`Uc8253X3Driver.cpp:302-304`) [SRC]. The model copies only when `N2OCP=1`.
- **Geometry**: DS79 is 800 source × 600 gate, `800×600×2 bits` SRAM [DS]. TRES sets the active area from
  S0/G0 (`HRES[9:3]*8` sources, `VRES` gates), GSST moves its origin [DS].
  - X3 UC8253: TRES 792×600, GSST 0; the driver writes 528 rows of 99 bytes (52 272 bytes per plane), **rows
    in reverse order** (row 527 first: `EpdBus::sendPlaneFlipped`, `EpdBus.cpp:470`); "logical row y lives at
    gate H-1-y" (`Uc8253X3Driver.cpp:395-396`) [SRC]. The data counter then expects 600 rows; the last 72
    gate rows of the plane are never written [INF].
  - X3 UC8279: no TRES; init sets **PTIN + PTL 0–791 × 0–527** and every plane write and refresh stays in
    that window "to avoid the controller's native 800×600 stride" (`FI:../docs/xteink-x3-uc8279-support.md`;
    `Uc8279Driver.cpp:73-80`) [SRC]. Inside a window, a DTM stream fills the window row by row, 99 bytes
    per row [INF from FI's comment "keeps plane writes/refresh at 99-byte rows"].
- **PTL** horizontal resolution is 8 pixels: `HRST` low 3 bits written 000, `HRED` low 3 bits 111 (DS79);
  FI rounds a window outward (`x & ~7`, `x | 7`, `Uc8253X3Driver.cpp:407-408`) [SRC]. `PT_SCAN=1` scans
  gates inside and outside the window, 0 inside only [DS].

### 2.5 LUT format and timing

- DS79 (LUTWW and every LUT in KW mode): 7 groups of 6 bytes. Byte 0: four 2-bit level selects, bits 7:6
  first; bytes 1–4: frame counts 0–255 of the four sub-phases; byte 5: times to repeat 0–255 [DS]. Levels for
  WW/KW/WK/KK: `00` GND, `01` VDH, `10` VDL, `11` VDHR; for LUTC: `00` VCOM_DC, `01` VDH+VCOM_DC, `10`
  VDL+VCOM_DC, `11` floating [DS]. PX's "`11` = VCOM" is not what DS79 says [SRC vs DS].
- The waveform ends after the 7 groups, or at the first group whose repeat is 0, or whose four frame counts
  are 0; VCOM then gets 2 frames of VCOM_DC and floats (DS79 "Data transmission waveform") [DS].
- Duration = Σ over groups (Σ four frame counts) × repeat, in frames at the PLL frame rate [INF from DS79].
  Panel convention on the X3: VDH drives toward black, VDL toward white (`PX:docs/x3-lut-waveforms.md`) [SRC].
- **Frame rate is not settled**: X3 UC8253 PLL `0x09` is 80 Hz in DS79's table and 50 Hz in DS53's; PX
  measured "≈18.2 ms per frame group" (≈55 Hz) [DS, SRC]. UC8279 PLL `0x0F` is 200 Hz (DS79) or 80 Hz (DS53)
  [DS]. A parameter of the model [?].
- UC8279 B/W banks (GC, DU, grey pre-pass) are 5 × 42 data bytes in this format; the grey banks (XTF_AA,
  XTH4) are **5 × 49 bytes** in a format no source describes (`Uc8279Driver.cpp:53-70`) [SRC, ?].

### 2.6 Identification reads [SRC unless tagged]

| Board | REV `0x70`, 3 bytes as the stock probe samples them | FLG `0x71` | RMTP `0xA2` |
|---|---|---|---|
| X3 UC8253 | `FF FF FF` (SDA not driven) | floating `00`/`FF` or driven idle | floats, uniform `FF` (PX: "the UC8253 has no RMTP command") |
| X3 UC8279d | byte 2 = **`0x66`** (FI, stock V6.3.15 `4200fb12`) | `0x13` | 1 dummy byte, then MTP: not `A5` at 0, zeros, LUT version stamp at `0x1A`, repeatable |

PX documents the UC8279 VER as 5 bytes "reserved `0x00`, `CHIP_VER` (default `0x03`), `LUT_VER[23:0]`" and
reports field modules reading `FF FF FF FF FF`; FI reads `0x66` in byte 2 on the same modules. The two
contradict [?]. `00 03 66 xx xx` satisfies FI/stock (byte 2) and PX's "structured" rule (FLG driven idle,
VER not uniform and equal on two passes, `PX:lib/EInkDisplay/src/X3DisplayProbeClassifier.cpp:17-20`) [INF].
FI's probe in `XteinkDetect.cpp:242-245` treats any other byte 2 as inconclusive and keeps the UC8253.

### 2.7 Emulation notes

- Must answer: every command of the "X3 sends" column with its byte count; DTM1/DTM2 streams of 52 272 bytes;
  BUSY_N low during PON/DRF/POF (and DSLP), high otherwise; RST_N; DSLP + `A5` (then ignore everything until
  RST_N low); REV/FLG reads with the board's identity (§2.6). A UC8253 model drives no read data.
- Accept and record without effect, with a named warning on first use: PWR values, BTST, VDCS, PFS, TCON,
  `0xE1`, CCSET, TSSET, PLL (kept only to time the waveform), border bits of CDI (border not drawn).
- Refuse with a named warning: OTP programming (`A0`/`A1`), `AUTO`, KWR mode (`KW/R=0`), `REG=0` (LUT from
  OTP: the X3 OTP/MTP is blank or unknown; PX: a UC8279 driven with `REG=0` "leaves the panel dark"),
  DUSPI, commands received while BUSY_N is low other than REV/FLG/VV (ignored, as the datasheet says).
- The visible result of DRF in B/W: each pixel ends at the colour of NEW when its LUT's last drive is the
  matching voltage; the simple model "pixel := NEW" is right for every X3 B/W bank [INF]. Grey banks need the
  waveform-level model of SP2a (§6 there), or a named "grey waveform not simulated" warning for the 49-byte
  UC8279 banks.

## 3. TI BQ27220 fuel gauge

### 3.1 Protocol [BQDS §7.3.1]

- 7-bit address fixed `1010101` = **0x55** (`0xAA` write, `0xAB` read).
- Supported: 1-byte write, incremental write, 1-byte read (pointer write, **repeated start**, read), incremental
  read, quick read (read without pointer write returns "data at the address indicated by the address pointer").
  "The address pointer … increments whenever data is acknowledged by the fuel gauge or the I2C master."
- **Not supported**: writing a read-only address (NACK after the data byte); reading an address above
  `0x6B` (NACK on the command byte) [DS]. BQTRM nevertheless lists `0x79`–`0x7F` (AnalogCount, RawCurrent,
  RawVoltage, RawIntTemp) [DS vs DS, ?].
- Standard commands are 16-bit little endian at an even address and the next one (`0x08`/`0x09` …) [DS].
- 400 kHz max; t(BUF) ≥ 66 µs between packets; above 100 kHz TI asks for 1-byte writes for subcommands;
  clock stretching ≤ 100 µs in SLEEP, ≤ 4 ms in INITIALIZATION/NORMAL; the bus is released if held low 2 s;
  no more than two standard commands per second or the gauge's watchdog may reset it (BQDS §7.3.1.2–4) [DS].
- FI writes every 16-bit value as one 3-byte transaction ("the X3's BQ27220 ignores a Control() subcommand
  split in two"), 66 µs apart, and reads with repeated start
  (`FI:hardware/BatteryMonitor/src/BatteryMonitor.cpp:50-95`) [SRC]: the opposite of TI's advice at 400 kHz.

### 3.2 Standard commands [BQTRM Table 2-1, §2.3–2.34]

| Code | Name | Unit / meaning | Access |
|---|---|---|---|
| 00/01 | Control() / CONTROL_STATUS() | write: subcommand; read: status (low byte: CCA bit 5, BCA 4, SNOOZE 3, BATT_ID[2:0]) | RW |
| 02/03 | AtRate() | mA, signed, default 0 | RW |
| 04/05 | AtRateTimeToEmpty() | min; 65535 when AtRate = 0 | R |
| 06/07 | Temperature() | 0.1 K | RW (write used only if WRTEMP) |
| 08/09 | Voltage() | mV, 0–6000 | R |
| 0A/0B | BatteryStatus() | high: FD, OCVCOMP, OCVFAIL, SLEEP, OTC, OTD, FC, CHGINH; low: RSVD, TCA, OCVGD, AUTH_GD, BATTPRES, TDA, SYSDWN, DSG | R |
| 0C/0D | Current() | mA signed, instantaneous, updated each second (Table 2-1 prints "mAh", §2.8 says mA) | R |
| 10/11 | RemainingCapacity() | mAh | R |
| 12/13 | FullChargeCapacity() | mAh | R |
| 14/15 | AverageCurrent() | mA | R |
| 16/17 | TimeToEmpty() | min; 65535 when not discharging | R |
| 18/19 | TimeToFull() | min; 65535 when not charging | R |
| 1A/1B | StandbyCurrent() | mA signed | R |
| 1C/1D | StandbyTimeToEmpty() | min | R |
| 1E/1F | MaxLoadCurrent() | mA signed | R |
| 20/21 | MaxLoadTimeToEmpty() | min | R |
| 22/23 | RawCoulombCount() | mAh | R |
| 24/25 | AveragePower() | mW signed, negative when discharging | R |
| 28/29 | InternalTemperature() | 0.1 K | R |
| 2A/2B | CycleCount() | count | R |
| 2C/2D | StateOfCharge() (RelativeStateOfCharge) | %, 0–100, RM ÷ FCC rounded up | R |
| 2E/2F | StateOfHealth() | %, FCC ÷ DesignCapacity rounded up | R |
| 30/31 | ChargingVoltage() | mV; 65535 = max | R |
| 32/33 | ChargingCurrent() | mA; 65535 = max | R |
| 34/35, 36/37 | BTPDischargeSet(), BTPChargeSet() | mAh | RW |
| 3A/3B | OperationStatus() | high bit 2 CFGUPDATE; low: BTPINT, SMTH, INITCOMP, VDQ, EDV2, SEC1, SEC0, CALMD; SEC 11 sealed, 10 unsealed, 01 full access | R |
| 3C/3D | DesignCapacity() | mAh (Design Capacity, data memory default 3000) | R |
| 3E/3F | ManufacturerAccessControl() | subcommand, or data-memory address (low byte at 0x3E, high at 0x3F) | RW |
| 40–5F | MACData() | 32-byte block: subcommand response or data memory (**big endian** values) | RW |
| 60 | MACDataSum() | checksum = 255 − (sum of 0x3E/0x3F and MACData bytes) mod 256 | RW |
| 61 | MACDataLen() | bytes in the block; a block write executes when 0x60 and 0x61 are written together | RW |

Gaps (`0x0E`/`0x0F`, `0x26`/`0x27`, `0x38`/`0x39`, `0x62`–`0x6B`): what they return is not documented [?].

### 3.3 Control() subcommands [BQTRM Table 2-2]

`0x0000` CONTROL_STATUS (ignored); **`0x0001` DEVICE_NUMBER → `0x0220` in MACData()**; `0x0002` FW_VERSION
(block `ddDDvvVVbbBBTTzzZZRREE`); `0x0003` HW_VERSION; `0x0009` BOARD_OFFSET; `0x000A`/`0x000B` CC_OFFSET(_SAVE);
`0x000C` OCV_CMD; `0x000D`/`0x000E` BAT_INSERT/REMOVE; `0x0013`/`0x0014` SET/CLEAR_SNOOZE; `0x0015`–`0x001A`
SET_PROFILE_1–6; `0x002D` CAL_TOGGLE; **`0x0030` SEALED**; `0x0041` RESET (unsealed only); `0x0054`
OPERATION_STATUS; `0x0056` GaugingStatus; `0x0080`/`0x0081` EXIT/ENTER_CAL; **`0x0090` ENTER_CFG_UPDATE**,
**`0x0091` EXIT_CFG_UPDATE_REINIT**, **`0x0092` EXIT_CFG_UPDATE**; `0x0F00` ENTER_ROM. After DEVICE_NUMBER or
FW_VERSION, Control() reads `0xFFA5` once (response moved to MACData). §2.2.2 says DEVICE_NUMBER returns
`0x0220`, so 4 bytes read at `0x3E` should be `01 00 20 02` [INF]; the manual's own example prints
`0x01 0x00 0x20 0x03` and its table "for example: 0x0320", apparently copied from a sibling gauge [DS, ?]. Unseal keys `0x0414` then `0x3672`; full access
`0xFFFF` twice; "the BQ27220 boots up in UNSEAL, but not in FULL ACCESS" (BQTRM §6.1) [DS].

### 3.4 What the X3 firmwares do [SRC]

- Read Voltage `0x08`, Current `0x0C`, StateOfCharge `0x2C`, one second apart at least
  (`FI:…/BatteryMonitor.cpp:25-27`; `PX:docs/x3-specifications.md`). Positive current = charging = external
  power (PX "USB detection").
- X3/X4 probe: SoC ≤ 100 and Voltage within 2500–5000 mV (`FI:hardware/XteinkDetect/src/XteinkDetect.cpp:618-624`,
  `PX:lib/BoardSupport/src/BoardSelector.cpp:134-142`).
- FI rewrites Design Capacity (650 mAh) and a too-high Learned FCC when Design Capacity reads 3000: unseal
  (`0x0414`, `0x3672`, 1.5 s apart), full access (`0xFFFF` ×2), `0x0090`, poll OperationStatus bit 10, write
  `0x3E` = `0x929D` then `0x929F`, read `0x40` and `0x60`, write `0x40` and `0x60` (sum word, length in the
  high byte), `0x0091` or `0x0092`, then `0x0030` (`BatteryMonitor.cpp:98-200`). It notes the X3's gauge
  "moves to the next block" after reading MACDataSum.

### 3.5 Emulation notes

- Implement: address `0x55`; 8-bit pointer with auto-increment on each acknowledged byte, read across a
  register pair (and beyond) in one burst; NACK of a write to a read-only command and of a command byte
  above `0x6B`; repeated-start reads and quick reads.
- Plausible idle values (parameters of the profile, not facts): Voltage 3850 (`0F0A`), SoC 70, Current and
  AverageCurrent −15 mA (`FFF1`), FCC 650, RemainingCapacity 455, DesignCapacity 650 (a value of 3000 makes
  CrossPoint run the whole data-memory sequence), Temperature 2982 (25.0 °C), BatteryStatus `0x0009`
  (BATTPRES, DSG), OperationStatus `0x0026` (INITCOMP, sealed), CycleCount 12, StateOfHealth 100,
  TimeToEmpty 1820, TimeToFull 65535, CONTROL_STATUS 0 [INF].
- Charging (cable in): Current and AverageCurrent positive, DSG clear, TimeToEmpty 65535 [INF].
- Control(): DEVICE_NUMBER/FW_VERSION answer through MACData; the security state machine (keys, CFGUPDATE,
  SEALED) and MACData read/write with checksum are needed only if Design Capacity reads 3000; otherwise
  accept and warn ("BQ27220 data memory not modelled"). Refuse RESET/ENTER_ROM/calibration with a warning.

## 4. Maxim DS3231 real-time clock

### 4.1 Register map [DS31 Figure 1]

| Addr | Bits 7…0 | Range |
|---|---|---|
| 00 | `0`, 10 s [6:4], s [3:0] | 00–59 BCD |
| 01 | `0`, 10 min, min | 00–59 |
| 02 | `0`, 12/24 (1 = 12 h), AM/PM or 20 h, 10 h, h | 1–12 + AM/PM or 00–23 |
| 03 | `00000`, day [2:0] | 1–7 (meaning user-defined, sequential) |
| 04 | `00`, 10 date, date | 01–31 |
| 05 | Century, `00`, 10 month, month | 01–12 + century |
| 06 | 10 year, year | 00–99 |
| 07–0A | Alarm 1: A1M1 + s, A1M2 + min, A1M3 + hours, A1M4 + DY/DT + day/date | |
| 0B–0D | Alarm 2: A2M2 + min, A2M3 + hours, A2M4 + DY/DT + day/date | |
| 0E | Control: EOSC, BBSQW, CONV, RS2, RS1, INTCN, A2IE, A1IE | POR `0x1C` |
| 0F | Status: OSF, 0, 0, 0, EN32kHz, BSY, A2F, A1F | POR `1000 1XXX` |
| 10 | Aging offset, two's complement | POR `0x00` |
| 11 | Temperature MSB, two's complement integer °C | POR 0 |
| 12 | Temperature LSB: bits 7:6 = 0.25 °C steps, bits 5:0 = 0 | POR 0 |

- Century bit toggles when the year overflows 99→00; day-of-week increments at midnight. "Illogical time and
  date entries result in undefined operation." "Unless otherwise specified, the registers' state is not
  defined when power is first applied" [DS].
- Control: EOSC 0 = oscillator runs (stops on VBAT only when 1); CONV forces a temperature conversion (CONV
  and BSY return to 0 at the end, 125–200 ms `tCONV`); RS = 1 Hz / 1.024 / 4.096 / 8.192 kHz; INTCN 1 = alarm
  interrupt, 0 = square wave; A1IE/A2IE alarm interrupt enables [DS].
- Status: **OSF = 1 at first power-up** and whenever the oscillator stopped; stays 1 until written 0. A1F/A2F
  set on a match, writable to 0 only. BSY = TCXO conversion running. EN32kHz 1 at power-up [DS].
- Temperature: 10-bit, 0.25 °C; example `00011001 01` = +25.25 °C; converted at power-up and every 64 s [DS].

### 4.2 Protocol [DS31 pp. 11, 15–17]

- 7-bit address `1101000` = **0x68**; standard and fast mode, **400 kHz max**.
- Write: address+W, word address (sets the register pointer), then data bytes, the pointer incrementing after
  each byte. "Write transfers occur on the acknowledge from the DS3231." Writing seconds resets the countdown
  chain; the other time registers must follow within 1 s.
- Read: address+R starts at the register pointer; "if the register pointer is not written to before the
  initiation of a read mode, the first address that is read is the last one stored in the register pointer".
  The usual form is pointer write, repeated start, read; the master ends with NACK.
- **Wrap**: past `0x12` the pointer returns to `0x00`.
- **Latching**: "the user buffers are synchronised to the internal registers on any START and when the
  register pointer rolls over to zero"; time is read from these secondary registers while the clock runs.
- tOSF 100 ms; tREC (recovery at power-up) 250–300 ms [DS].

### 4.3 What the X3 firmwares do [SRC]

- FI: reads Status `0x0F`, writes Control `0x0E` = `0x04` (INTCN, no square wave), refuses the time when OSF is
  set, reads 7 bytes from `0x00`, decodes the 12-hour bit; on a time set writes 7 bytes from `0x00` with the
  century bit set for years < 2000 (clear = 20xx), then clears OSF
  (`FI:hardware/Rtc/src/Rtc.cpp:28-33,128-130,173-179,221,256-276`).
- Probe: register `0x00` must hold valid BCD seconds (`XteinkDetect.cpp:626-632`, `BoardSelector.cpp:144-148`).

### 4.4 Emulation notes

- Implement: the 19 registers, pointer with increment and wrap 0x12→0x00, latch of time on START and on
  wrap, clock driven by the emulated device time (deterministic: from the session start time, not the host
  clock while running), BCD encoding, 12/24-hour mode, century bit, OSF/A1F/A2F write-0-only semantics.
- Defaults: a first power gives OSF = 1 and Control `0x1C`, Status `0x88`; a "battery-backed, already set"
  profile gives Status `0x08` and a valid time; Temperature `0x19 0x00` (25.00 °C) [INF].
- Alarm matching and the INT/SQW output can wait; until wired, a write enabling A1IE/A2IE with INTCN, or a
  square wave (INTCN = 0), warns "DS3231 INT/SQW not wired". Invalid BCD writes are stored as written and warn
  once (the datasheet leaves the behaviour undefined) [INF].

## 5. QST QMI8658 6-axis IMU

### 5.1 Address and protocol

- 7-bit address **0x6A** with SA0 unconnected (internal 200 kΩ pull-down), **0x6B** with SA0 pulled up
  (QMC §12.2) [DS]. The X3 profile names `0x6B`, alternate `0x6A`; both firmwares try both [SRC].
- Host I2C up to 400 kHz; SPI 15 MHz; I3C (QMC Table 13) [DS].
- Auto-increment: QMC Rev 0.6 gives two mechanisms, CTRL1 bit 6 `SPI_AI` ("serial interface (SPI or I2C)
  address auto increment", default 0) and, in the I2C section, bit 7 of the register address [DS]. SL defines
  only CTRL1 bit 6 (`MASK_ADDR_AI`, `SensorQMI8658_Reg.hpp:64`) [SRC]. FI writes CTRL1 = `0x40` and reads 6 bytes
  from `0x35` and `0x3B` with plain addresses (`FI:hardware/Imu/src/Imu.cpp:42-45,193,254-255`) [SRC]. Without
  increment, what a multi-byte read returns is not stated [?].
- Endianness: data registers are `_L` then `_H` (little endian) [DS]; CTRL1 bit 5 `SPI_BE` (default 1 in QMC)
  selects big-endian *SPI* reads [DS]; whether it changes I2C reads is [?]. FI clears it and assembles little
  endian [SRC].

### 5.2 Register map [QMC Table 20; SL for the A variant]

| Addr | Name | Default | Notes |
|---|---|---|---|
| 00 | WHO_AM_I | **0x05** | [DS] |
| 01 | REVISION_ID | 0x68 (Table 20) / 0x79 (Table 23) | the datasheet contradicts itself [DS] |
| 02 | CTRL1 | 0x20 | 7 SIM (3-wire SPI), 6 auto-increment, 5 SPI big endian, 0 SensorDisable (1 = 2 MHz oscillator off); SL adds bit 4 INT2 / bit 3 INT1 enables, bit 2 I2C watchdog [DS, SRC] |
| 03 | CTRL2 | 0x00 | 7 aST self-test; 6:4 aFS 0=±2 g, 1=±4, 2=±8, 3=±16; 3:0 aODR 0=8000 Hz … 8=31.25 Hz, C–F low-power 128/21/11/3 Hz |
| 04 | CTRL3 | 0x00 | 7 gST; 6:4 gFS 0=±16 … 7=±2048 dps; 3:0 gODR 0=8000 … 8=31.25 Hz |
| 05 | CTRL4 | 0x00 | magnetometer (C) / reserved |
| 06 | CTRL5 | 0x00 | 6:5 gLPF_MODE, 4 gLPF_EN, 2:1 aLPF_MODE, 0 aLPF_EN |
| 07 | CTRL6 | 0x00 | AttitudeEngine ODR, motion on demand (C); motion-feature enables (A, SL) |
| 08 | CTRL7 | 0x00 | 7 syncSmpl, 6 sys_hs, 4 gSN snooze, 3 sEN (C), 2 mEN (C), **1 gEN, 0 aEN** |
| 09 | CTRL8 | 0x00 | reserved (C); motion/tap control (A) |
| 0A | CTRL9 | 0x00 | host command (§5.3) |
| 0B–12 | CAL1_L … CAL4_H | 0x00 | command parameters/results |
| 13–17 | FIFO_WTM_TH, FIFO_CTRL, FIFO_SMPL_CNT, FIFO_STATUS, FIFO_DATA | 0x00 | |
| 2C | I2CM_STATUS | 0x00 | C only |
| 2D | STATUSINT | 0x00 | 1 Locked, 0 Avail (syncSmpl); SL: **bit 7 CTRL9 done** (A) |
| 2E | STATUS0 | 0x00 | 3 sDA, 2 mDA, **1 gDA, 0 aDA** (new data since last read) |
| 2F | STATUS1 | 0x00 | C: bit 0 CmdDone, cleared on read; A (SL): motion events (tap, WoM, pedometer, any/no/significant motion) |
| 30–32 | TIMESTAMP L/M/H | 0x00 | 24-bit sample counter, wraps |
| 33–34 | TEMP_L, TEMP_H | 0x00 | 256 LSB/°C; **°C = TEMP_H + TEMP_L / 256** (SL `SensorQMI8658.cpp:355-356`); refreshed at 8 Hz [DS] |
| 35–3A | AX, AY, AZ (L, H) | 0x00 | 16-bit two's complement |
| 3B–40 | GX, GY, GZ (L, H) | 0x00 | 16-bit two's complement |
| 41–46 | MX, MY, MZ (C) / 46 COD_STATUS (A, SL) | | |
| 49–56 | dQW…dQZ, dVX…dVZ | | AttitudeEngine (C) / self-test data (A) |
| 4D | RST_RESULT (A, SL) | | reads `0x80` when a soft reset has completed (`SensorQMI8658_Reg.hpp:41,404-407`) [SRC] |
| 60 | RESET | — | write **`0xB0`** = soft reset (SL `:40,398-401`; not in QMC Rev 0.6) [SRC] |

Sensitivity [QMC Tables 7–8, DS]: accelerometer 16 384 / 8 192 / 4 096 / 2 048 LSB/g at ±2 / 4 / 8 / 16 g;
gyroscope 2048 / 1024 / 512 / 256 / 128 / 64 / 32 / 16 LSB/dps at ±16 … ±2048 dps.
Timing [DS]: system turn-on 1.75 s from power or software reset; accelerometer on 3 ms + 3/ODR; SL polls
`0x4D` every 10 ms after `0xB0` (`SensorQMI8658.cpp:302-311`) [SRC].

### 5.3 CTRL9 handshake

- QMC: host writes the command to `0x0A`; the device executes it, raises INT1 and sets STATUS1 bit 0 CmdDone;
  the host reads STATUS1, which clears the bit and drops INT1 [DS]. Commands: `00` NOP, `01` GYRO_BIAS, `03`
  REQ_SDI, `04` RST_FIFO, `05` REQ_FIFO, `06` I2CM_WRITE, `08` WOM setting, `09`/`0A` host delta offsets, `10`
  COPY_USID, `11` SET_RPU [DS].
- SL (QMI8658A): waits for **STATUSINT bit 7**, then writes `0x00` (CTRL_CMD_ACK) to CTRL9 and waits for bit 7
  to clear (`SensorQMI8658.cpp:276-292`); adds `0C` tap, `0D` pedometer, `0E` motion, `0F` reset pedometer,
  `12` AHB clock gating, `A2` on-demand calibration, `AA` gyro gains (`SensorQMI8658_Reg.hpp:212-227`) [SRC].
  Which variant the X3 carries is [?]; the A handshake is the one current drivers expect [INF].

### 5.4 What the X3 firmwares do [SRC]

FI: probe WHO_AM_I at `0x6B` then `0x6A`; configure CTRL7 = 0, CTRL1 = `0x40`, CTRL2 = `0x06` (±2 g), CTRL3 =
`0x56` (±512 dps), CTRL7 = `0x03`; read 6 bytes at `0x3B` and `0x35`; scale 1/16384 g and 1/64 dps; sleep
writes CTRL7 = 0 and CTRL1 = `0x41` (`FI:hardware/Imu/src/Imu.cpp:32-56,150-151,184-196,254-274`). PX only
probes WHO_AM_I (`BoardSelector.cpp:109-153`). Neither uses CTRL9, the FIFO, the temperature or RESET.

### 5.5 Emulation notes

- Implement: address `0x6B` (profile parameter, `0x6A` alternative), WHO_AM_I `0x05`, REVISION_ID (parameter,
  `0x68`), CTRL1–CTRL9 as plain storage, pointer with auto-increment when CTRL1 bit 6 (or address bit 7) is set,
  STATUS0 aDA/gDA set while the sensors are enabled, TIMESTAMP counting at the highest enabled ODR, RESET `0xB0`
  (defaults back, RST_RESULT `0x80`), TEMP.
- Idle device flat on a table, screen up: accel (0, 0, +1 g) → at ±2 g AX = AY = 0, AZ = `0x4000` (bytes
  `00 40`); gyro 0; TEMP_H `0x19`, TEMP_L `0x00` (25.0 °C) [INF]. The sign of Z depends on the chip's mounting,
  which no source gives; FI carries swap/flip flags per board [?]. Scale values with CTRL2/CTRL3.
- With aEN/gEN clear, or SensorDisable set, the data registers keep their last value and STATUS0 stays 0 [?];
  the model freezes them and warns once if they are read.
- CTRL9 commands: answer NOP/ACK with the handshake of the configured variant; refuse the others with a named
  warning ("QMI8658 CTRL9 command 0x.. not modelled"). FIFO, wake-on-motion, tap/pedometer, magnetometer,
  AttitudeEngine and self-test: refuse with a named warning; INT1/INT2 are not wired on the X3 as far as any
  source says [?].

## 6. Open points

1. The exact controller of the first X3 batch: a large-format UC8253 variant whose datasheet is not public, or a
   UC8179-class part sold under that name. Its PSR second byte, `0xE1`, its PLL table and REV bytes are unknown.
2. UC8279d: datasheet not public; layout of PWR (`43 00 78 78 17`), PSR byte 2 (`4A`), the 49-byte grey LUT
   format, the frame rate at PLL `0x0F`, and the REV bytes (`0x66` in byte 2 per FI/stock against
   `FF FF FF FF FF` per PX).
3. Whether DSP (`0x11`) starts a refresh when `data_flag = 1` in register-LUT KW mode (DS79 says so; no X3
   driver relies on it).
4. BUSY_N durations of PON, POF and DSLP, and the delay between DRF and the BUSY_N low edge.
5. BQ27220: what the reserved command codes return; whether the OEM seals the gauge; the X3's real Design
   Capacity; the behaviour FI reports (block select moving after reading MACDataSum, keys ignored back to back).
6. DS3231 vs a compatible part: the probe recognises the chip only by an ACK and valid BCD seconds.
7. QMI8658 variant (C or A), its REVISION_ID, the I2C auto-increment rule, the mounting orientation, and what
   disabled sensors return.
