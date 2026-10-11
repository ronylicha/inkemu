# inkemu

**A binary-level emulator for embedded e-ink devices.** inkemu runs the real, unmodified firmware image of a device on a virtual replica of its hardware. You can check how a firmware renders and behaves before you flash it.

> **Status: early development.** inkemu boots the real ESP32-C3 mask ROM from its reset vector and prints its banner. The RV32IMC core is validated against riscv-tests and, through RISCOF, against the Sail reference model. Flash, the second-stage bootloader and the application come next (sub-projects 1b-2 to 1b-4). See the [design spec](docs/superpowers/specs/2026-10-11-inkemu-design.md) and the [roadmap](#roadmap).

## Build & try

```bash
cargo build --release

# Boot a virtual Xteink X4 from its mask ROM and print its console.
target/release/inkemu run --device x4 --max-cycles 5000000
```

```text
ESP-ROM:esp32c3-api1-20210207
Build:Feb  7 2021
rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)
```

The device console goes to standard output. Everything inkemu has to say goes to standard error: the device and timing model in use, then one `warning:` line for each register the firmware touches that inkemu does not model yet, with the peripheral, the register, its address and the program counter. `--strict-mmio` turns the first of them into a stop, for CI.

The first run downloads the ROM (4.9 MB) from Espressif's `esp-rom-elfs` release `20260528` and checks its SHA-256 before using it. To work offline, extract the archive yourself and pass `--rom-dir <directory>`; `inkemu rom fetch --all` fills a directory in advance.

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

1. RV32IMC core, minimal ESP32-C3, firmware loader, CLI: boot a firmware to its console logs. *1a done; 1b-1, the SoC skeleton and the ROM boot, done; 1b-2, flash and the bootloader, is next.*
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

## Français

inkemu est un émulateur **au niveau binaire** d'appareils embarqués à écran e-ink. Il exécute le firmware réel, sans recompilation, sur une réplique virtuelle du matériel : mémoire exacte, modèle de cycles, rendu e-ink fidèle (ghosting, niveaux de gris, durées de rafraîchissement), Wi-Fi avec accès Internet réel, BLE, débogage GDB et tests automatisés. Il tourne sur Linux, macOS, Windows et dans le navigateur. Les liseuses Xteink sont les premières cibles ; d'autres appareils suivront.

**État :** inkemu démarre la vraie ROM masquée de l'ESP32-C3 depuis son vecteur de reset et affiche sa bannière (`inkemu run --device x4`). Le cœur RV32IMC est validé contre riscv-tests puis, avec RISCOF, contre le modèle de référence Sail ; les commandes `inkemu inspect` et `inkemu rvtest` sont en place. La flash, le bootloader de second niveau et l'application sont la prochaine étape (sous-projets 1b-2 à 1b-4).

## Third-party material

- **Mask ROM.** The ESP32-C3 ROM is Espressif's code and is not open source. inkemu never ships it: it downloads the ROM ELF files that Espressif publishes under Apache-2.0 in [`esp-rom-elfs`](https://github.com/espressif/esp-rom-elfs) and verifies them against pinned hashes.
- **Register descriptions.** `third_party/espressif-svd/` holds an unmodified copy of Espressif's `esp32c3.svd` (Apache-2.0), from which `crates/svd-regs` is generated.

See [ADR 0001](docs/adr/0001-espressif-rom-and-svd-licensing.md).

## License

[Apache License 2.0](LICENSE).

Xteink is a trademark of its respective owner. This project is not affiliated with Xteink or Espressif.
