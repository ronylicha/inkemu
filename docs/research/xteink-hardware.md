> Rapport de recherche du 2026-10-11. Les chemins `R/...` désignent des clones locaux des dépôts cités ; chaque fait porte sa source et un niveau de confiance.

# Xteink X3 / X4 / X4 Pro — hardware research for a firmware-level emulator

Research date: 2026-10-11. Local clones used as primary evidence live in
`/tmp/claude-1000/-home-rony-projects-xteink-emulator/efaa7359-51e8-4dc3-82f3-80b5b3bb7f32/scratchpad/repos/`
(abbreviated `R/` below). Image headers were parsed locally with `scratchpad/tools/imginfo.py` and `appdesc.py`.

Confidence tags: **[HW]** confirmed on hardware / schematic by a project author, **[RE]** reverse-engineered from OEM firmware,
**[SRC]** stated in open-source firmware code, **[WEB]** vendor/press page, **[INF]** my inference, **[?]** unknown / unverified.
Nothing below is guessed silently: anything not found is marked `unknown`.

IMPORTANT naming caveat: there are FOUR Xteink boards, not three. The "X4" (original, ESP32-C3) is different from the
"X4 v2 Classic / X4C" (ESP32-S3, no touch). The X4 Pro is ESP32-S3 too. X4C is summarised at the end of the tables.

---------------------------------------------------------------------------------------------------

## 1. Summary tables

### 1.1 Xteink X4 (original) — ESP32-C3

| Item | Value | Conf. | Source |
|---|---|---|---|
| SoC | ESP32-C3 (QFN32), no embedded PSRAM | HW | schematic image `R/sunwoods_Xteink-X4/readme-img/05.jpg` (U1 = ESP32-C3); https://github.com/sunwoods/Xteink-X4 ; https://learn.adafruit.com/circuitpython-on-the-xteink-x4-ereader.md?view=all |
| CPU | single-core RISC-V RV32IMC, up to 160 MHz | Espressif datasheet (general chip fact, not re-fetched) | https://www.espressif.com/sites/default/files/documentation/esp32-c3_datasheet_en.pdf ; firmware runs 160 MHz active / 10 MHz "light idle" policy: `R/bigbag_papyrix-reader/docs/device-support-matrix.md` (Papyrix) and `R/crosspoint-reader_crosspoint-reader/lib/hal/HalPowerManager.h` (CrossPoint: LOW_POWER_FREQ 10 MHz, normal = whatever `getCpuFrequencyMhz()` returns at boot) |
| SRAM / PSRAM | 400 KB SRAM (327,680 B usable under PlatformIO per sample author); no PSRAM | SRC | https://learn.adafruit.com/circuitpython-on-the-xteink-x4-ereader.md?view=all ; `R/CidVonHighwind_xteink-x4-sample/README.md` |
| Flash | 16 MB external SPI flash, image header says DIO @ 80 MHz | SRC | `R/CidVonHighwind_xteink-x4-sample/README.md`; `R/crosspoint-reader_crosspoint-reader/platformio.ini` (`board_build.flash_mode = dio`, 16MB); parsed header of `x3-crosspoint.bin` |
| Display | 4.26" GDEQ0426T82 (Good Display), 800x480, ~217-220 PPI, B/W + 4-level grayscale (via custom LUT + 2 RAM planes) | HW/WEB | https://www.good-display.com/product/457.html (cited by sample repo); `R/bigbag_papyrix-reader/docs/x4-specifications.md`; https://techtactician.com/xteink-x4-my-impressions-stock-os-and-crosspoint/ (marketing "4.3 in 480x800", same panel in portrait) |
| Controller | SSD1677 (default). Some newer production runs carry UC8179 instead | SRC | `R/freeink-sdk/libs/hardware/XteinkDetect/include/XteinkDetect.h`, `R/freeink-sdk/libs/hardware/BoardConfig/include/BoardConfig.h` lines ~137-160 |
| Panel bus | 4-wire SPI mode 0, MSB first, write-only (no MISO on panel). SCLK 8, MOSI 10, CS 21, DC 4, RST 5, BUSY 6. BUSY is ACTIVE-HIGH for SSD1677 (Papyrix doc says "LOW = busy" — contradicted by FreeInk `EpdBus.h`: "SSD1677: busy while HIGH"; treat HIGH=busy) | HW | Adafruit pinouts; `R/CidVonHighwind_xteink-x4-sample/README.md`; independent Rust firmware `R/trusty/x4/src/main.rs`; `R/freeink-sdk/libs/display/FreeInkDisplay/src/bus/EpdBus.h` |
| SPI clock | 10 MHz (FreeInk/CrossPoint default); legacy firmware used up to 40 MHz | SRC | `BoardConfig.h` (`XTEINK_DISPLAY_SPI_HZ`, `FREEINK_X4_OVERCLOCK_SPI`); Papyrix x4-specifications says 40 MHz |
| Refresh modes (SSD1677) | FULL: ctrl2 0x22=0xF7, ~1.6-1.8 s. FAST/partial: 0x22=0xFC, ~0.5-0.6 s. HALF: temp reg 0x1A=0x5A + 0x22=0xD7 (~1.7 s, writes both RAM planes). Grayscale: external 111-byte LUT via 0x32 + BW/RED planes. Border 0x3C=0x80 at init, 0xC0 per refresh. Init: SW reset 0x12 + 10 ms delay + wait BUSY, 0x18=0x80 (internal temp), booster 0x0C = AE C7 C3 C0 80, driver output 0x01 (479 gates, SM=1), RAM window via 0x11/0x44/0x45/0x4E/0x4F, auto-fill RAM 0x46/0x47 | SRC | `R/freeink-sdk/libs/display/FreeInkDisplay/src/driver/Ssd1677Driver.cpp` (`ssd1677DefaultConfig`); `R/bigbag_papyrix-reader/docs/ssd1677-driver.md` |
| Framebuffer | 800/8 x 480 = 48,000 B per plane (BW 0x24, RED 0x26) | SRC | Papyrix x4-specifications |
| Buttons | 7 physical keys: Back, Confirm, Left, Right (front), Up, Down (right side), Power (top). Power = GPIO3 active-LOW, INPUT_PULLUP. Other 6 via two ADC resistor ladders: GPIO1 {Back, Confirm, Left, Right}, GPIO2 {Up, Down}. 12-bit ADC, 11 dB atten. Typical raw readings (CrossPoint avg): GPIO1: Back 3512, Confirm 2694, Left 1493, Right ~5, idle >3900; GPIO2: Up 2242, Down ~5, idle >3900. Decision thresholds in code: GPIO1: >3100 Back, 2090-3100 Confirm, 750-2090 Left, <750 Right (idle = >3900); GPIO2: >1120 Up (2205-2242 seen), <=1120 Down | HW/SRC | `R/freeink-sdk/libs/hardware/InputManager/src/InputManager.cpp` lines 34-49, 195-225; `R/CidVonHighwind_xteink-x4-sample/README.md`; Adafruit page lists power on GPIO3, ladder on GPIO1/GPIO2 |
| Reset | separate RESET switch tied to CHIP_EN (pinhole) | HW | schematic 05.jpg |
| SD | microSD in **SPI mode, shared bus with panel**: SCLK 8, MOSI 10, MISO 7, CS 12 (only CS differs from the panel). Driven by SdFat; CrossPoint uses a separate `HSPI` SPIClass object for SD in some paths | HW/SRC | `R/sunwoods_Xteink-X4` schematic (SD slot: CMD=IO10, CLK=IO8, D0=IO7, CD/DAT3→IO12 "wired oddly"); `R/CidVonHighwind_xteink-x4-sample/README.md`; `R/freeink-sdk/libs/hardware/SDCardManager/src/SDCardManager.cpp` |
| USB | USB-C. ESP32-C3 native USB-Serial-JTAG (GPIO18/19 = D-/D+ per chip) used as console/flash (`-DARDUINO_USB_MODE=1 -DARDUINO_USB_CDC_ON_BOOT=1`). No USB mass-storage. Some retail units are "USB-locked" (flashing disabled; recover via OTA spoof) | SRC | `platformio.ini`; Adafruit page ("Serial over JTAG ... no USB drive"); `R/crosspoint-reader_crosspoint-reader/README.md` (USB-locked section) |
| USB/VBUS detect | GPIO20 (= UART0 RXD) reads HIGH when USB present | HW/SRC | CidVonHighwind sample README; Papyrix x4-specifications; Adafruit (GPIO20 "detects charging") |
| Battery | ~650 mAh Li-ion. Voltage on GPIO0 (ADC1_CH0) via 2x10 kOhm divider (reads Vbat/2). Full ~4.2 V, empty ~3.0 V. Percent poly fit used by Papyrix: -144.9390V^3+1655.8629V^2-6158.8520V+7501.3202 | HW/SRC | schematic 05.jpg (R5=R1=10k, node → IO0); `R/freeink-sdk/libs/hardware/BatteryMonitor/src/BatteryMonitor.cpp` (divider multiplier 2.0, ADC_ATTEN_DB_12, 12-bit); Papyrix x4-specifications |
| Charger IC | unknown (no I2C charger/gauge on X4; charging seen only through GPIO20) | ? | — |
| Power latch | GPIO13 drives battery MOSFET gate ("power latch"): firmware drives HIGH at boot, LOW for battery-only deep sleep; HIGH during USB-connected sleep so it keeps charging. Known units self-latch via the power button | SRC | `BoardConfig.h` XTEINK_X4 comment; Papyrix x4-specifications "Power States" |
| Wireless | ESP32-C3 2.4 GHz WiFi (b/g/n) + BLE 5 (no Classic BT) | WEB/SRC | Adafruit; techtactician |
| RTC / IMU / buzzer / frontlight | none found (no RTC, no accelerometer, no frontlight, no buzzer) | WEB/SRC | techtactician ("no touch, no frontlight"); `BoardConfig.h` XTEINK_X4 (`NO_FRONTLIGHT`, `NO_SENSORS`) |

### 1.2 Xteink X3 — ESP32-C3 (same board family as X4)

| Item | Value | Conf. | Source |
|---|---|---|---|
| SoC | ESP32-C3 (shares C3 binary with X4: CrossPoint "x3-x4" image; app image header chip_id=5) | SRC | `R/freeink-sdk/libs/hardware/XteinkDetect/include/XteinkDetect.h` ("two BoardProfiles compiled into one ESP32-C3 binary"); header of `R/crosspoint-reader_crosspoint-tools/public/firmware/x3-crosspoint.bin` (chip_id 5); stock device_type string `ESP32C3_X3` in `crosspoint-tools/worker/index.ts` |
| SRAM/PSRAM/flash | 400 KB / none / 16 MB | INF/SRC | same C3 + `X3_PARTITION_TABLE` in `R/crosspoint-reader_crosspoint-tools/src/lib/flasher.js` ends at 0x1000000 |
| Official page does NOT list MCU/RAM/flash | — | WEB | https://www.xteink.com/blogs/product/xteink-x3-review-pocket-ereader |
| Display | 3.68-3.7" E-Ink, **792 x 528** landscape (528x792 portrait), ~259 PPI, 4-level gray | SRC/WEB | `R/bigbag_papyrix-reader/docs/x3-specifications.md`; xteink.com X3 page (3.7", 259 PPI) |
| Controller | **UC8253** (original) or **UC8279d** (newer runs; same board/glass/pins). Panel module part number: unknown | SRC | `R/freeink-sdk/docs/xteink-x3-uc8279-support.md`; `R/bigbag_papyrix-reader/docs/x3-specifications.md` |
| Panel bus | Same pins as X4: SCLK 8, MOSI 10, CS 21, DC 4, RST 5, BUSY 6. BUSY convention for UC81xx = busy when LOW. SPI 10 MHz (UC8253) / 20 MHz (UC8279d). Four-wire, mode 0, MSB first | SRC | `BoardConfig.h` XTEINK_X3 / XTEINK_X3_UC8279; `EpdBus.h`; Papyrix x3-specifications |
| Framebuffer | 99 B x 528 = 52,272 B per plane | SRC | Papyrix x3-specifications |
| Controller detect at boot | 3-byte VER (0x70) read after reset; byte 2 = 0x66 -> UC8279, 0xFF -> assume UC8253. Also an I2C fingerprint decides X3 vs X4 (see below) | SRC | `XteinkDetect.h`; `xteink-x3-uc8279-support.md` |
| Refresh | UC8253: 5 LUT registers (0x20 VCOM, 0x21 WW, 0x22 BW, 0x23 WB, 0x24 BB; 42 B each, 7 phases) with Full/Half/Turbo/Image/Gray sets; fast=Turbo. UC8279d: external GC (full/half) + DU (fast) waveforms, KW mode (DTM1=old plane, DTM2=new plane), 792x528 partial window to avoid native 800x600 stride | SRC | `R/bigbag_papyrix-reader/docs/x3-lut-waveforms.md`, `x3-uc8279-driver-reference.md`; `R/freeink-sdk/libs/display/FreeInkDisplay/src/driver/Uc8253X3Driver.*`, `Uc8279Driver.*` |
| Buttons | Same ADC ladder scheme as X4: power GPIO3 (active-LOW), ladder GPIO1/GPIO2 (profile identical `XteinkAdcLadder`). Official page says "page-turn buttons"; exact physical layout unknown | SRC/? | `BoardConfig.h` XTEINK_X3 uses identical input tuple as X4 |
| SD | microSD **SPI**: SCLK 8, MOSI 10, MISO 7, CS 12; **GPIO13 = SD power enable, active-HIGH** (HIGH at boot; LOW before deep sleep; ~10 ms rail settle; then drive CS HIGH). 16 GB card bundled | RE/SRC | `BoardConfig.h` XTEINK_X3 comment ("confirmed by X3 factory-firmware RE: setup() does digitalWrite(13,HIGH)"); Papyrix x3-specifications; xteink.com X3 page |
| I2C | **SDA = GPIO20, SCL = GPIO0, 400 kHz** — devices: BQ27220 fuel gauge @0x55, DS3231 RTC @0x68, QMI8658 6-axis IMU @0x6B (alt 0x6A). The probe identifies chips only by I2C ACK at those addresses; exact part identity beyond that is firmware-based, so treat DS3231 vs compatible as medium confidence | SRC/RE | `BoardConfig.h` XTEINK_X3; `XteinkDetect.h`; Papyrix x3-specifications |
| Battery | ~650 mAh (SDK config), measured by BQ27220: voltage reg 0x08 (mV), SoC reg 0x2C (%), current reg 0x0C. No charger IC on I2C. Charging via **magnetic pogo-pin cable** (no USB-C on the product page) | SRC/WEB | `BatteryMonitor.cpp` constants; xteink.com X3 page. Whether the pogo pins carry USB data to the C3: unknown |
| Gyro | yes: "shake to turn page" (QMI8658) | WEB/SRC | xteink.com X3 page; `BoardConfig.h` |
| Frontlight / touch / buzzer | none | WEB | xteink.com X3 page ("no front light, no touchscreen") |
| X3-vs-X4 runtime detect | Two passes probing I2C 0x55/0x68/0x6B/0x6A on SDA=20/SCL=0; >=2 hits in both = X3, 0 hits both = X4; result cached in NVS (CrossPoint: namespace `cphw`, keys `dev_ovr`/`dev_det`; Papyrix: `papyrix_hw`) | SRC | `XteinkDetect.h`; `R/crosspoint-reader_crosspoint-reader/lib/hal/HalGPIO.cpp` |
| Weight / size | 58 g, 97.6 x 63.7 x 5.1 mm | WEB | xteink.com X3 page |

### 1.3 Xteink X4 Pro — ESP32-S3 (NOT the same board as X4)

| Item | Value | Conf. | Source |
|---|---|---|---|
| SoC | **ESP32-S3**, 16 MB flash, **8 MB octal PSRAM**. Vendor does not disclose; CNX marks "TBC" | RE/SRC | image header of `R/crosspoint-reader_crosspoint-tools/public/firmware/x4pro-stock-en.bin` (chip_id 9 = ESP32-S3); `R/freeink-sdk/docs/xteink-x4pro-support.md`; https://www.cnx-software.com/?p=174132 |
| CPU | dual-core Xtensa LX7, up to 240 MHz (firmware active policy 240 MHz / idle 10 MHz) | datasheet / SRC | Papyrix `device-support-matrix.md` |
| Display | 800x480 B/W, ~4.3" (~219 PPI per CNX), same glass as X4 | WEB | CNX link above; `xteink-x4pro-support.md` |
| Controller | SSD1677 (original units), or UC8179 / UC8279 on newer batches. Selected at boot by reading VER (0x70) bytes; X4C table of IDs also documented | SRC | `xteink-x4pro-support.md`; `R/bigbag_papyrix-reader/docs/x4pro-specifications.md` |
| Panel bus (SSD1677 variant, hardware-confirmed by bit-bang sweep) | SCLK 12, MOSI 11, CS 13, DC 18, RST 14, BUSY 6 (active-HIGH). Write-only. 10 MHz (older OEM image 5 MHz). **GPIO1 = master peripheral rail latch (HIGH at boot)** | HW | `xteink-x4pro-support.md` "Confirmed display pinout" |
| Refresh | OEM 7.4.4: FULL ctrl2=0xF7, FAST=0xFC, border 0x3C=0xC0 (plain X4 OTP waveform) | RE | `xteink-x4pro-support.md` |
| Touch | GT911 @0x5D (alt 0x14), I2C SDA 39 / SCL 38 (100-400 kHz), INT = GPIO10, RST = GPIO4, **touch power rail GPIO2 active-LOW** (+ GPIO1 HIGH). Mounted portrait: X 0-480, Y 0-800; swapXY + flipY. Capacitive "Home" key = GT911 key bit 0x10 (status 0x814E & 0x10) | HW | `xteink-x4pro-support.md` |
| Buttons | digital active-LOW (INPUT_PULLUP): Left/prev = GPIO0 (boot strap!), Right/next = GPIO7, Power = GPIO3. (The OEM image still contains an unused ADC ladder matcher; hardware proves it is vestigial.) Reset button also present | HW | `xteink-x4pro-support.md`; CNX (Power, Reset, Previous, Next, Home) |
| Frontlight | dual warm/cool LEDC PWM: cool = GPIO8 (LEDC ch4), warm = GPIO9 (ch5), 25 kHz, 10-bit, active-HIGH | HW/RE | `xteink-x4pro-support.md` |
| SD | **native SDMMC 1-bit**, slot 1: CLK 41, CMD 42, DAT0 40, 40 MHz, internal pull-ups. **GPIO5 = SD power enable, active-LOW**; OEM pulses HIGH 80 ms -> LOW 120 ms before each mount and leaves LOW. SPI-mode CMD0 is silent on this card path | HW | `xteink-x4pro-support.md` |
| I2C bus (SDA 39 / SCL 38) | BM8563 RTC @0x51; CW2017 fuel gauge @0x63 (80-byte BATINFO profile must be loaded; VCELL regs 0x02/0x03 14-bit mV = (raw*5+8)>>4, SoC reg 0x04); GT911 @0x5D | HW | `xteink-x4pro-support.md` |
| Battery / charge | ~1100 mAh; charger STAT on GPIO21 (active-HIGH = charging) - charger IC part unknown; VBUS-detect pin not conclusively identified (GPIO10 is only a candidate in FreeInk, conflicts with GT911 INT, so effectively unknown) | WEB/RE | CNX; `xteink-x4pro-support.md` |
| USB | native USB: GPIO19 = D-, GPIO20 = D+, VID:PID 303A:1001; OEM does USB-MSC "USB Transfer" of the SD card + CDC. Charging connector is pogo pins (no USB-C per CNX) | RE/WEB | `xteink-x4pro-support.md`; Papyrix x4pro-specifications; CNX |
| Wireless | 2.4 GHz WiFi 4 + BLE | WEB | CNX |
| Weight / size | 72 g, 111 x 69 x 5.95 mm, $99 | WEB | CNX |
| Partition table (OEM dump) | nvs 0x9000/0x5000; otadata 0xE000/0x2000; app0 ota_0 0x10000/0x7E0000; app1 ota_1 0x7F0000/0x7E0000; spiffs 0xFD0000/0x14000; coredump 0xFE4000/0x1C000. Factory boots app0; app1 blank on factory device | RE | `xteink-x4pro-support.md`; `R/crosspoint-reader_crosspoint-tools/unlocker-tool/README.md`; `flasher.js` `X4_PRO_PARTITION_TABLE` |
| Bootloader quirk | App-rollback enabled; flashing an OTA slot in state NEW rolls back unless marked VALID | RE | `unlocker-tool/README.md` |

### 1.4 X4 v2 "Classic" (X4C) — for completeness (ESP32-S3, 16 MB flash, 8 MB PSRAM)

800x480; controller SSD1677/UC8179/UC8279 by VER byte2 (0x01 QY4.28 UC8179, 0x40 BOE UC8179, 0x02 UC8279, 0x68/0x69 ZHX UC8279, 0x41/0x42 BOE UC8279). Display pins differ from X4 Pro: SCLK 12, MOSI 11, CS 13, DC 14, RST 10, BUSY 18; GPIO1 power rail; SD enable GPIO6; 8 discrete active-LOW buttons (side L=GPIO0, R=GPIO7, power GPIO3; bottom GPIO5=Left, GPIO2=Right, GPIO8=Confirm, GPIO9=Back); no touch, no frontlight; 1-bit SDMMC; BM8563 + CW2017. Source: `R/freeink-sdk/docs/xteink-x4c-support.md`, `R/bigbag_papyrix-reader/docs/x4-classic-specifications.md`.

---------------------------------------------------------------------------------------------------

## 2. Pin maps (condensed)

### ESP32-C3: X4 (X3 shares everything except GPIO0, GPIO13, GPIO20)
```
GPIO0   X4: battery ADC (divider /2)            | X3: I2C SCL (BQ27220 0x55, DS3231 0x68, QMI8658 0x6B/0x6A)
GPIO1   ADC ladder A: Back / Confirm / Left / Right
GPIO2   ADC ladder B: Up / Down
GPIO3   Power button (active-LOW, pull-up)  — also deep-sleep wake
GPIO4   EPD DC        GPIO5   EPD RST       GPIO6   EPD BUSY
GPIO7   SD MISO       GPIO8   SPI SCLK (EPD+SD shared)    GPIO10  SPI MOSI (EPD+SD shared)
GPIO12  SD CS         GPIO21  EPD CS
GPIO13  X4: battery power latch (HIGH=on)       | X3: SD power enable (active-HIGH)
GPIO18/19  native USB-Serial-JTAG D-/D+ (chip fixed)
GPIO20  X4: UART0 RXD = USB/VBUS detect (HIGH)  | X3: I2C SDA
```
Sources: Adafruit pinouts https://learn.adafruit.com/circuitpython-on-the-xteink-x4-ereader/pinouts ; schematic `R/sunwoods_Xteink-X4/readme-img/05.jpg`; https://github.com/CidVonHighwind/xteink-x4-sample ; independent Rust port `R/trusty/x4/src/main.rs` (GPIO8/10/7/12/4/6/5/21/0/20/1/2/3 identical); FreeInk `BoardConfig.h` XTEINK_X4 / XTEINK_X3.
Schematic detail (05.jpg): 24-pin EPD FPC: BUSY=IO6, RST=IO5, D/C=IO4, CS=IO21, SCLK=IO8, SDI=IO10; GPIO3 on a 5-pin FPC = power button (and BAT+/POWER_CTRL lines); GPIO2 / GPIO1 on two 5-pin FPCs for the side and bottom ladders; reset switch -> CHIP_EN.
The Adafruit table lists GPIO2 under the same CircuitPython name as GPIO1 (likely a typo in their doc); FreeInk/CrossPoint code uses GPIO1 and GPIO2.

### ESP32-S3: X4 Pro (SSD1677 variant, all hardware-confirmed except where noted)
```
GPIO0 Left/prev button   GPIO7 Right/next button   GPIO3 Power button   (all active-LOW)
GPIO1 master peripheral rail (HIGH)      GPIO2 touch power rail (LOW = on)
GPIO4 GT911 RST    GPIO10 GT911 INT     GPIO38 I2C SCL   GPIO39 I2C SDA  (GT911 0x5D, BM8563 0x51, CW2017 0x63)
GPIO5 SD power (LOW = on)    GPIO40 SD DAT0   GPIO41 SD CLK   GPIO42 SD CMD   (SDMMC slot 1, 1-bit)
GPIO6 EPD BUSY  GPIO11 EPD MOSI  GPIO12 EPD SCLK  GPIO13 EPD CS  GPIO14 EPD RST  GPIO18 EPD DC
GPIO8 frontlight cool PWM  GPIO9 frontlight warm PWM
GPIO21 charger STAT (HIGH = charging)   GPIO19/20 native USB D-/D+
```
Source: `R/freeink-sdk/docs/xteink-x4pro-support.md`, `BoardConfig.h` XTEINK_X4_PRO. Unknown: panel mount flip, VBUS-detect pin, charger IC.

---------------------------------------------------------------------------------------------------

## 3. Firmware format

### 3.1 What a "firmware .bin" is for each project (verified by parsing headers in `crosspoint-tools/public/firmware/`)

| File | Size | Header facts |
|---|---|---|
| `x3-crosspoint.bin` (CrossPoint X3/X4 C3 build) | 5,867,184 B | **Plain ESP-IDF application image** (magic 0xE9), chip_id 5 (ESP32-C3), 7 segments, entry 0x4038086E, SPI mode DIO (2), flash 16 MB, 80 MHz, hash appended. Segments: DROM 0x3C180020 (4.2 MB), DRAM 0x3FC90600, IROM 0x42000020, DRAM 0x3FC94078, IRAM 0x40380000, RTC 0x50000000. app_desc: project `arduino-lib-builder`, IDF `v5.5.2-729-g87912cd291`, built Feb 11 2026 (Arduino-ESP32 prebuilt libs, pioarduino 55.03.311) |
| `x4pro-stock-en.bin` (OEM X4 Pro 7.0.8, decrypted) | 6,792,544 B | plain app image, chip_id 9 (ESP32-S3), 8 segments, entry 0x403798EC, project `xteink_app`, version 7.0.8, built Jul 14 2026, IDF v6.0.1 |
| `bootloader.bin` (C3) | 18,672 B | second-stage bootloader, chip_id 5, to be written at flash 0x0 |
| `x4pro-bootloader.bin` | 18,736 B | chip_id 9 |

- CrossPoint releases (v1.6.5, 2026-09-27) ship app-only images: `firmware.bin` / `crosspoint-1.6.5-x3-x4.bin` = 5,589,776 B; `...-x4pro.bin` = 5,632,640 B; `...-x4c.bin` 5,611,520 B. Source: `gh release view -R crosspoint-reader/crosspoint-reader` (not downloaded). **These are NOT merged images**: no bootloader, no partition table. They are written at 0x10000 (`board_upload.offset_address = 0x10000`) or into the inactive OTA slot. Papyrix likewise: `papyrix-xteink-c3.bin` (X3+X4), `papyrix-x4pro.bin`, `papyrix-x4c.bin`, all app-only at 0x10000 with a manifest.json.
- **OEM (stock) X3/X4 firmware**: delivered as a *plain* app image through `https://api-prod.xteink.cc/api/v1/check-update?...&device_type=ESP32C3` (X4) or `ESP32C3_X3` (X3) (see `crosspoint-tools/worker/index.ts` STOCK_CHECK_URLS). Not downloaded in this research.
- **OEM X4 Pro / X4C firmware**: `.xota` = `XOTA` magic(4) + IV(16) + 182-byte encrypted metadata + body, AES-128-CTR over metadata||body, `ota_format: encrypted_v1`; plain image can be had from a device app-slot dump (first `plain_size` bytes hash to server `plain_sha256`). Key is embedded in firmware (`unlocker-tool/crates/unlocker-core/src/xota.rs`, not read here). Source: `R/crosspoint-reader_crosspoint-tools/unlocker-tool/README.md`. If the user's .bin files came from CrossPoint/Papyrix/official X3-X4 downloads, they are plain app images; if they came from the X4 Pro vendor server they may still be `.xota` (encrypted) — check for the `XOTA` magic.
- Full-flash dumps are 16 MB (`esptool read_flash 0x0 0x1000000`) — also used by the flasher tools (full-flash must be exactly 0x1000000 bytes, `flasher.js` line ~843).

### 3.2 Partition tables
| Device | nvs | otadata | ota_0 | ota_1 | spiffs | coredump |
|---|---|---|---|---|---|---|
| X4 stock = CrossPoint/Trusty (`default_16MB.csv`, `partitions.csv`) | 0x9000/0x5000 | 0xE000/0x2000 | 0x10000/0x640000 | 0x650000/0x640000 | 0xC90000/0x360000 | 0xFF0000/0x10000 |
| X3 stock ("Stock X3 layout", flasher.js `X3_PARTITION_TABLE`; CrossPoint requires repartitioning) | 0x9000/0x5000 | 0xE000/0x2000 | 0x10000/0x770000 | 0x780000/0x770000 | 0xEF0000/0x100000 | 0xFF0000/0x10000 |
| X4 Pro stock (OEM dump) | 0x9000/0x5000 | 0xE000/0x2000 | 0x10000/0x7E0000 | 0x7F0000/0x7E0000 | 0xFD0000/0x14000 | 0xFE4000/0x1C000 |
Sources: `R/crosspoint-reader_crosspoint-reader/partitions.csv`, `R/CidVonHighwind_xteink-x4-sample/default_16MB.csv`, `R/trusty/xteink-xt4.csv`, `R/crosspoint-reader_crosspoint-tools/src/lib/flasher.js` lines 232-285, `xteink-x4pro-support.md`.
Boot chain for building a QEMU flash image: bootloader @0x0, partition table @0x8000 (needs generating from CSV with `gen_esp32part.py`; the compiled partition-table binary is not shipped in the clones except Sticky), otadata @0xE000 (blank 0xFF -> bootloader picks first OTA slot = ota_0 [INF, standard ESP-IDF behaviour]), app @0x10000. Pad to 16 MB. CrossPoint also wraps `bootloader_common_check_efuse_blk_validity` to return OK (`src/platform/skip_efuse_blk_check.c`: stock X3 bootloader mis-reads app_desc eFuse block revisions) and builds with `-Wl,--wrap=panic_print_backtrace...`. No flash encryption / secure boot found for X3/X4 (secure_version 0; no mention in any source — [?] eFuse state of real devices unverified).
Useful NVS keys the firmware reads at boot (affect device selection): CrossPoint `cphw`/`dev_ovr`,`dev_det` (0 unknown,1 X4,2 X3); Papyrix `papyrix_hw` keys `dev_ovr`,`dev_det`,`epd_ovr`,`epd_det`,`epd_ver`; OEM X4C/X4 Pro `hw_calib`/`screenType` (u8; 1=UC8179, 2=UC8279, else SSD1677).
Boot-time hardware probes an emulator must satisfy or deliberately fail: X3-vs-X4 I2C fingerprint (SDA20/SCL0 ACKs at 0x55/0x68/0x6B/0x6A — return NACK everywhere to be an X4, ACK >=2 of 3 to be an X3); panel VER (0x70) bit-banged read half-duplex on MOSI after reset (X3: byte 2 = 0xFF assumes UC8253; X4: SSD1677 does not answer; X4-family probe is a two-pass VER/FLG read).

---------------------------------------------------------------------------------------------------

## 4. Open-source firmware / community projects (best pin-map sources)

| Project | What it is | Use for emulator |
|---|---|---|
| CrossPoint Reader — https://github.com/crosspoint-reader/crosspoint-reader (v1.6.5, PlatformIO, Arduino-ESP32 via pioarduino 55.03.311, board `esp32-c3-devkitm-1`) | Main community firmware for X3/X4/X4 Pro/X4C (+Sticky, PaperMono). HAL in `lib/hal/` | HAL files (`HalGPIO.cpp`, `HalPowerManager.cpp`, `HalDisplay`, `HalStorage`) + `freeink-sdk` submodule |
| **FreeInk SDK** — https://github.com/Free-Ink/freeink-sdk (MIT) | Hardware abstraction used by CrossPoint 1.6.5; `BoardConfig.h` holds every profile (pins, controller, input style, gauge, RTC) and `docs/xteink-*-support.md` hold RE notes | **Best single source of truth**; `libs/display/FreeInkDisplay/src/driver/{Ssd1677,Uc8253X3,Uc8279,Uc8179}Driver.*` give exact command sequences + LUTs |
| Papyrix Reader — https://github.com/bigbag/papyrix-reader | Alternative firmware (X3, X4, X4 Pro, X4C); docs include `x4-specifications.md`, `x3-specifications.md`, `x4pro-specifications.md`, `ssd1677-driver.md`, `x3-uc8279-driver-reference.md` | cross-checks; per-device pin summaries |
| xteink-x4-sample — https://github.com/CidVonHighwind/xteink-x4-sample | Minimal GxEPD2 sample; first public pin map and ADC thresholds; origin of OpenX4 community-sdk | ADC levels, pins |
| sunwoods/Xteink-X4 — https://github.com/sunwoods/Xteink-X4 | Teardown photos + **schematic fragment (sch.jpg/05.jpg)** | ground truth for X4 pins |
| TrustyReader — https://github.com/HookedBehemoth/TrustyReader | Rust (esp-hal, `no_std`) X4 firmware + desktop SDL build + `wokwi.toml` | independent pin confirmation; shows no Wokwi model of EPD |
| crosspoint-tools — https://github.com/crosspoint-reader/crosspoint-tools | Flasher/unlocker/stock-firmware proxy, partition tables, X4 Pro `.xota` format | partition tables, firmware formats |
| Adafruit CircuitPython board `xteink_x4` — https://circuitpython.org/board/xteink_x4/ , learn page https://learn.adafruit.com/circuitpython-on-the-xteink-x4-ereader/pinouts | CircuitPython port | pin names, 16 MB flash dump command |
| Others (not read in depth): ngxson/esphome-component-xteink (ESPHome), hansmrtn/pulp-os (Rust), andrewjiang/flowe-os (X3 firmware), viticci/muse-pocket (X4 Pro), maddiedreese/xteink-terminal, penk/X4Term, CrossPoint-CJK, InkPointX | — |

---------------------------------------------------------------------------------------------------

## 5. Emulator prior art

| Item | Finding | Source |
|---|---|---|
| Existing Xteink-specific **binary-level** emulator | **None found.** Searched GitHub (`gh search repos xteink`), web search for "xteink emulator/QEMU/Wokwi/Renode". | search results this session |
| Native "run the source on desktop" emulators (NOT running the .bin) | `jonmooreai/Crosspoint-Emulator` (SDL2 800x480, directory-backed SD, keyboard; recompiles CrossPoint sources with a HAL shim; WiFi stubbed); TrustyReader desktop build; Meta Muse "UI simulator" (SDL, explicitly not CPU-emulating); FreeInk host tests (`docs/testing.md`) | https://github.com/jonmooreai/Crosspoint-Emulator ; R/trusty/desktop ; R/viticci_muse-pocket/esp32/simulator/README.md |
| Wokwi | Supports ESP32-C3 and ESP32-S3 boards and loads custom ESP-IDF firmware. TrustyReader ships `wokwi.toml` + `diagram.json` using only `board-esp32-c3-devkitm-1` (16 MB flash) with serial — **no EPD/SD/button parts modelled**. Per-peripheral C3 coverage is not documented in sources I found. Closed-source/server-side, so unsuitable as an embeddable reference. | https://docs.wokwi.com/guides/esp32 ; R/trusty/wokwi.toml,diagram.json |
| Espressif QEMU fork (`espressif/qemu`, branch `esp-develop`, latest release tag `esp-develop-9.2.2-20260417`) | **ESP32-C3 machine `esp32c3`** (riscv32-softmmu), **ESP32-S3 machine `esp32s3`** (xtensa-softmmu), ESP32 too. Flash image sizes 2/4/8/16 MB accepted (16 MB OK). Needs ROM image `esp32c3-rom.bin` (found via `qemu_find_file`, shipped in release tarballs). `-icount 3` mandatory for C3 (free-running unsupported; = 8 ns/insn = 125 MHz). No Secure Boot. Strapping via `-global driver=esp32c3.gpio,property=strap_mode,value=0x08` (default SPI flash boot). Flash encryption supported. Run line: `qemu-system-riscv32 -nographic -icount 3 -machine esp32c3 -drive file=flash_image.bin,if=mtd,format=raw`. Merge image with `esptool.py --chip esp32c3 merge_bin --fill-flash-size 16MB -o flash_image.bin @flash_args` | https://github.com/espressif/esp-toolchain-docs/tree/main/qemu/esp32c3 ; R/esp-toolchain-docs/qemu/esp32c3/README.md ; `gh api repos/espressif/qemu` |
| QEMU C3 devices actually instantiated (read from `hw/riscv/esp32c3.c`, esp-develop) | UART0/1, GPIO (with strap), interrupt matrix, EXTMEM/cache, eFuse, clock, SHA, AES, RSA, HMAC, Digital Signature, XTS-AES (flash enc), GDMA, TIMG0/1, SYSTIMER, **SPI1 (flash controller only)**, RTC_CNTL, **USB-Serial-JTAG**, TWAI, OpenCores Ethernet (framebuffer region for RGB demo at 0x20000000). Unmapped IO reads return 0 (RTC_I2C range returns 0xffffff); **no GP-SPI2, no I2C0, no SAR-ADC (APB_SARADC), no LEDC, no RMT, no WiFi/BT, no PSRAM** found by grep of `esp32c3.c` (I did not exhaustively diff DR_REG maps — [INF] but strong) | `esp32c3.c` fetched from `espressif/qemu` via `gh api` |
| QEMU S3 machine | has: UART, GPIO, intmatrix, RTC_CNTL, TWAI, GDMA, SHA/AES/RSA/HMAC/DS/XTS, TIMG, SYSTIMER, SPI1 + **SPI PSRAM model (`ssi_psram`)**, **DWC SDMMC controller (`hw/sd/dwc_sdmmc.h`)**, USB-JTAG. RMT, IO_MUX only unimp stubs; no I2C/GP-SPI/ADC/LEDC model found in `esp32s3.c` | `esp32s3.c` fetched via `gh api` |
| Consequence for Xteink | Using Espressif QEMU as the CPU/ROM/peripheral base for C3 gets bootloader + app execution, UART/USB-JTAG console, timers, eFuse, flash; you must **add**: GP-SPI2 (FSPI: panel + SD on one bus, DMA via GDMA), SAR-ADC for button ladder + battery (X4), I2C0 for X3 (BQ27220/DS3231/QMI8658), GPIO matrix wiring for BUSY/CS/DC. For X4 Pro S3 you additionally need GP-SPI2/3, I2C (GT911/BM8563/CW2017), LEDC; QEMU already has SDMMC (1-bit mode path) + PSRAM. | [INF] |
| Renode | No ESP32-C3 platform found (`gh search code` in renode/renode & renode-infrastructure: 0 hits; an Antmicro tlib PR adds Xtensa ESP32 core support only) | gh api search; https://github.com/antmicro/tlib/pull/13 |
| Arduino-ESP32 SPI default on C3 | `SPI` = FSPI = GP-SPI2 (my knowledge of Arduino-ESP32; not verified in these sources) and SD is on same bus [INF] | EpdBus.cpp uses global `SPI.begin(sclk,miso,mosi,cs)` (`R/freeink-sdk/libs/display/FreeInkDisplay/src/bus/EpdBus.cpp:179`) |

---------------------------------------------------------------------------------------------------

## 6. Emulator-design relevant specifics (collected)

- ESP32-C3 timing anchors: CPU 160 MHz (stock Arduino default at boot = 160; light idle 10 MHz via `setCpuFrequencyMhz`), APB 80 MHz; SPI to panel at 10 MHz -> full-frame transfer ~42 ms (UC8253 52,272 B at 10 MHz) or ~21 ms at 20 MHz (UC8279d); SSD1677 full-plane 48,000 B at 10 MHz ~38 ms per plane (computed). BUSY durations to model: SSD1677 FULL ~1.6-1.8 s, FAST ~0.5-0.6 s, HALF ~1.7 s; soft-reset BUSY ~10 ms (firmware also inserts delay(10)); deep sleep cmd 0x10; reset pulse sequences in `EpdBus::reset` (`EpdBus.cpp:202`).
- Panel orientation: native 800x480 landscape; X4 firmware renders portrait 480x800 (rotation 3 in GxEPD2 sample).
- Buttons: emulate as ADC values (12-bit, 11 dB, attenuation ADC1 ch1/ch2 = GPIO1/GPIO2) idle ~4095 (>3900 treated as "no button"); use CrossPoint averages above. Power button on GPIO3 is also the deep-sleep wake (`esp_deep_sleep_enable_gpio_wakeup(GPIO3, LOW)`), so RTC/deep-sleep wake semantics matter; X4 sample also latches GPIO13.
- X4 battery ADC: 12-bit, 12 dB attenuation, `esp_adc_cal` (default Vref 1100) or `analogReadMilliVolts`; mV = reading * 2 (divider). Charge/VBUS: GPIO20 level.
- Heap: C3 has no PSRAM; CrossPoint uses single framebuffer (`EINK_DISPLAY_SINGLE_BUFFER_MODE=1`) and is memory-tight (~50 KB free during reading per platformio.ini comment).
- Console: USB-Serial-JTAG CDC (`ARDUINO_USB_CDC_ON_BOOT=1`), 115200 monitor.
- WiFi stack (wolfSSL TLS 1.3, HTTP server, OTA) will touch the WiFi MAC/RF registers; QEMU C3 has no WiFi — firmware paths will fail/stub unless faked at the ESP-IDF API layer.

---------------------------------------------------------------------------------------------------

## 7. Unknown / unverified (explicit)

1. X4 and X3 charger IC part number — unknown. X4 Pro charger IC — unknown (only a STAT line on GPIO21).
2. X3 panel module part number — unknown (only controller UC8253 / UC8279d known).
3. Whether X3 pogo-pin connector carries USB D+/D- (flashing path) — unknown.
4. Real eFuse state of retail devices (USB-JTAG disabled on "USB-locked" units? secure boot / flash encryption?) — not documented in any source read. "USB-locked" is described only as a firmware/OTA-level lock (`crosspoint-reader README`), mechanism unknown.
5. Stock X4/X3 firmware images were NOT downloaded/analysed (would need explicit approval); stock X3 partition layout comes from labels in `flasher.js`.
6. SD slot of X4 Pro is confirmed SDMMC; X3/X4 SD is SPI — one source (Adafruit) lists only CS/MISO.
7. Frontlight (X4 Pro) LUT mixing details and capacitive Home key behaviour not needed for CPU emulation.
8. DS3231 identity on X3 — inferred from I2C address 0x68 + firmware RE.
9. Papyrix doc lists SSD1677 BUSY as "LOW = busy" and "40 MHz SPI"; FreeInk code says HIGH = busy and 10 MHz. Used FreeInk (HIGH=busy) plus hardware-confirmed Rust sample; unresolved only in documentation.
10. QEMU peripheral coverage was determined from `esp32c3.c`/`esp32s3.c` machine files only; if the user builds a recent tag, re-check `hw/ssi`, `hw/i2c`, `hw/adc` directories (a top-level `hw/adc` and `hw/i2c` directory exist in the fork but the C3 machine does not instantiate any ESP-specific one).
