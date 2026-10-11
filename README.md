# inkemu

**A binary-level emulator for embedded e-ink devices.** inkemu runs the real, unmodified firmware image of a device on a virtual replica of its hardware. You can check how a firmware renders and behaves before you flash it.

> **Status: early development.** inkemu boots the real ESP32-C3 mask ROM from its reset vector. From a full flash image, the ROM loads the stock ESP-IDF second-stage bootloader, which runs and starts the application up to its first log line. The RV32IMC core is validated against riscv-tests and, through RISCOF, against the Sail reference model. Interrupts, timers, watchdogs and reset come next (sub-projects 1b-3 and 1b-4). See the [design spec](docs/superpowers/specs/2026-10-11-inkemu-design.md) and the [roadmap](#roadmap).

## Build & try

```bash
cargo build --release

# Boot a virtual Xteink X4 from a full flash image: mask ROM, 2nd-stage bootloader, application.
target/release/inkemu run --device x4 --flash flash.bin --max-cycles 50000000
```

This is the console of the reference image (the stock ESP-IDF v5.5.2 bootloader and its `hello_world` example, see [Third-party material](#third-party-material)). The timestamps are those of the emulated device:

```text
ESP-ROM:esp32c3-api1-20210207
Build:Feb  7 2021
rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)
SPIWP:0xee
mode:DIO, clock div:1
load:0x3fcd5820,len:0x15a4
load:0x403cbf10,len:0xc64
load:0x403ce710,len:0x2fd0
entry 0x403cbf1a
I (10) boot: ESP-IDF v5.5.2 2nd stage bootloader
I (10) boot: chip revision: v0.3
I (11) boot: efuse block revision: v1.2
…
I (12) boot: End of partition table
I (12) esp_image: segment 0: paddr=00010020 vaddr=3c010020 size=0656ch ( 25964) map
…
I (31) boot: Loaded app from partition at offset 0x10000
I (31) boot: Disabling RNG early entropy source...
I (31) cpu_start: Unicore app
```

`--flash` takes a **full flash image**: a flash dump, or the output of `esptool.py merge_bin` (bootloader at offset 0, partition table at `0x8000`, application). The file may be shorter than the flash of the device; the rest reads as erased. inkemu refuses, before anything runs, a file that cannot be the flash of the device (exit code 2, with the cause and the remedy): an encrypted `.xota`, an application image alone, an image for another chip, an image that declares more flash than the device has, a file longer than the flash, an empty file. For example:

```text
$ target/release/inkemu run --device x4 --flash app.bin
error: app.bin: the file starts with an image but holds no partition table at offset 0x8000, so it is an application or a bootloader alone, not a full flash image; use a flash dump or the output of `esptool.py merge_bin` (bootloader, partition table and application)
```

Without `--flash` the flash is erased and the ROM prints `invalid header: 0xffffffff` in a loop, as a blank chip does. There is no option yet to boot an application `.bin` on its own: inkemu does not compose a flash from one (planned for sub-project 1b-3).

Today the run goes as far as the first log line of the application. The application then waits for the calibration of the slow RTC clock, which needs a timer block inkemu does not emulate yet (`TIMG0.RTCCALICFG`). The wait shows as named warnings and the run only ends at `--max-cycles`. For the reference image, saved as `flash_hello.bin`, standard error reads:

```text
inkemu: Xteink X4 (ESP32-C3 v0.3, mask ROM rev3); timing model datasheet-uncalibrated (not calibrated); flash: flash_hello.bin (16777216 bytes in 16 MiB)
warning: unmodeled register read: TIMG0.RTCCALICFG at 0x6001f068 (pc 0x403829fa); returning its documented reset value 0x00013000
warning: unmodeled register read: TIMG0.RTCCALICFG2 at 0x6001f080 (pc 0x40382a08); returning its documented reset value 0xffffff98
warning: unmodeled register write: TIMG0.RTCCALICFG2 at 0x6001f080 (pc 0x40382a14); value 0x00000098 ignored
inkemu: cycle limit reached; raise --max-cycles to run longer
inkemu: stopped at pc 0x40382a1c after 50000000 cycles (39808633 instructions)
inkemu: 632854 us of device time; CPU clock at 80 MHz
```

With `--strict-mmio` the same run stops on the first of these, with exit code 3. It is the first unmodelled access since the reset, and no CPU exception came before it:

```text
inkemu: stopped by --strict-mmio: unmodeled register read: TIMG0.RTCCALICFG at 0x6001f068 (pc 0x403829fa); returning its documented reset value 0x00013000
inkemu: stopped at pc 0x403829fc after 1937072 cycles (1358289 instructions)
inkemu: 32068 us of device time; CPU clock at 80 MHz
```

The device console goes to standard output. Everything inkemu has to say goes to standard error: the device, the timing model and the flash in use, then one `warning:` line for each thing it cannot do faithfully — an unmodelled register read or write (with the peripheral, the register, its address and the program counter), an 8- or 16-bit write to a peripheral register, an operation it cannot perform faithfully (a flash command the read-only flash does not implement, SHA through DMA, a clock source it cannot turn into a frequency, a software reset), an access to flash that no MMU entry maps, a CPU exception — and, at the end, the most repeated ones, the device time elapsed and the CPU clock. `--strict-mmio` turns the first of these (CPU exceptions excepted) into a stop with exit code 3, for CI. Registers that inkemu stores without modelling their effect are listed in [docs/registers-accepted-without-effect.md](docs/registers-accepted-without-effect.md).

To build the reference image yourself (it needs Docker; nothing is executed on the host), see [tools/reference-firmware/README.md](tools/reference-firmware/README.md).

The first run downloads the ROM (4.9 MB) from Espressif's `esp-rom-elfs` release `20260528` and checks its SHA-256 before using it. To work offline, extract the archive yourself and pass `--rom-dir <directory>`. `inkemu run` reads only the ROM of its device's chip revision from that directory: `esp32c3_rev3_rom.elf` for `--device x4` and `--device x3`. `inkemu rom fetch --all [--rom-dir <directory>]` fills a directory in advance, with a single download, with the three ROMs of the supported chips (`esp32c3_rev0_rom.elf`, `esp32c3_rev3_rom.elf` and `esp32c3_rev101_rom.elf`); `inkemu rom fetch --device x4` fetches just the one that device needs.

Other commands: `inkemu inspect <app.bin>` describes an ESP-IDF application image and explains what is wrong when it refuses a file; `inkemu rvtest <elf>` runs a bare-metal RISC-V conformance test. For the pinned conformance suites and how to run riscv-arch-test against Sail, see [tools/conformance/README.md](tools/conformance/README.md).

## Why

Existing simulators for small e-ink devices recompile the firmware sources against a fake display driver. That only works when you have the sources, and it hides the bugs that live in the real hardware path. inkemu takes the opposite approach. It emulates the SoC, the bus and every chip on the board, then boots the vendor's binary through its real boot chain.

## Goals

- **Any firmware binary**: vendor firmware, community firmware, or your own. No sources or symbols required.
- **Hardware fidelity**: the device's exact memory map and RAM size, a cycle-level timing model, watchdogs, deep sleep.
- **Realistic e-ink rendering**: display controllers are emulated command by command, and each pixel has an optical state driven by the waveform LUTs. Full-refresh flashing, ghosting, grayscale and refresh durations come out of that model naturally.
- **Devices as data**: a device is a declarative TOML profile listing its SoC, external chips, wiring and button layout. Adding one does not touch the core.
- **Deterministic**: every input is a timestamped event, so any session can be recorded and replayed bit-for-bit. That includes network traffic.
- **Tooling**: serial console, GDB remote server, resource monitor (RAM, CPU, battery), headless CLI with input scripts and visual regression for CI.
- **Connectivity**: emulated Wi-Fi with real Internet access through a virtual access point, and BLE with virtual devices or bridged to the host adapter.
- **Everywhere**: one Rust core, compiled natively for Linux, macOS and Windows (Tauri app) and to WebAssembly for the browser. UI in English and French.

## Supported hardware (planned)

| Family | SoC / ISA | Devices |
|---|---|---|
| Espressif ESP32-C3 | RISC-V RV32IMC | Xteink X4, Xteink X3 |
| Espressif ESP32-S3 | Xtensa LX7 (dual core) | Xteink X4 Pro, Xteink X4C |

Xteink devices are the first targets. The architecture is not tied to them, and more devices and SoCs will follow.

## Roadmap

1. RV32IMC core, minimal ESP32-C3, firmware loader, CLI: boot a firmware to its console logs. *1a done; 1b-1, the SoC skeleton and the ROM boot, done; 1b-2, flash and the bootloader, done; 1b-3, interrupts, timers, watchdogs and reset, is next.*
2. X4 peripherals, desktop UI, e-ink rendering: **first visual milestone**
3. X3 support (UC8253 panel, I2C sensors)
4. Debugging, resource monitor, headless test runner
5. Wi-Fi (real Internet access) and BLE
6. WebAssembly build and web UI
7. Xtensa LX7 core and ESP32-S3: X4 Pro (touch, PSRAM, SDMMC) and X4C
8. Timing calibration bench
9. Signed release packages

## Documentation

- [Design specification](docs/superpowers/specs/2026-10-11-inkemu-design.md) (French)
- [Xteink hardware research](docs/research/xteink-hardware.md), with a source for every fact
- [Registers accepted without effect](docs/registers-accepted-without-effect.md): what is stored but not modelled, and the plan that makes each real
- [ADR 0002](docs/adr/0002-flash-model-and-cache-windows.md): the flash model and the cache windows

## Français

inkemu est un émulateur **au niveau binaire** d'appareils embarqués à écran e-ink. Il exécute le firmware réel, sans recompilation, sur une réplique virtuelle du matériel : mémoire exacte, modèle de cycles, rendu e-ink fidèle (ghosting, niveaux de gris, durées de rafraîchissement), Wi-Fi avec accès Internet réel, BLE, débogage GDB et tests automatisés. Il tourne sur Linux, macOS, Windows et dans le navigateur. Les liseuses Xteink sont les premières cibles ; d'autres appareils suivront.

**État :** inkemu démarre la vraie ROM masquée de l'ESP32-C3 depuis son vecteur de reset et affiche sa bannière (`inkemu run --device x4`). Le cœur RV32IMC est validé contre riscv-tests puis, avec RISCOF, contre le modèle de référence Sail ; les commandes `inkemu inspect` et `inkemu rvtest` sont en place. À partir d'une image flash complète (`--flash`), la ROM charge le bootloader de second niveau, qui s'exécute et lance l'application jusqu'à sa première ligne de journal ; les interruptions, les timers et les watchdogs sont la prochaine étape (sous-projets 1b-3 et 1b-4).

## Third-party material

- **Mask ROM.** The ESP32-C3 ROM is Espressif's code and is not open source. inkemu never ships it: it downloads the ROM ELF files that Espressif publishes under Apache-2.0 in [`esp-rom-elfs`](https://github.com/espressif/esp-rom-elfs) and verifies them against pinned hashes.
- **Reference firmware.** The tests boot the stock ESP-IDF bootloader and `hello_world` example (Apache-2.0). They are built in Docker by `tools/reference-firmware/build.sh` and checked against pinned hashes; no firmware binary is stored in this repository.
- **Register descriptions.** `third_party/espressif-svd/` holds an unmodified copy of Espressif's `esp32c3.svd` (Apache-2.0), from which `crates/svd-regs` is generated.

See [ADR 0001](docs/adr/0001-espressif-rom-and-svd-licensing.md).

## License

[Apache License 2.0](LICENSE).

Xteink is a trademark of its respective owner. This project is not affiliated with Xteink or Espressif.
