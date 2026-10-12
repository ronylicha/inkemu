# ESP32-C3 boot chain — hardware facts for inkemu SP1b

Scope: what the mask ROM, the ESP-IDF v5.5 2nd-stage bootloader and the early application need from the SoC, so the plan can list one peripheral model per register block with exact addresses.
Date of research: 2026-10-11.

Corrected on 2026-10-11 from `esp32c3-rom-flash-boot.md` §K, `esp32c3-cache-mmu.md` §0.1 and `esp32c3-bootloader-accesses.md` §11. Where this report and those three disagree, they prevail. Each correction below keeps what was believed, struck through (`~~like this~~`), followed by the established text and the mark "corrected 2026-10-11" with its source.

## 0. Sources, method, licence hygiene

| Tag | What | Where |
|---|---|---|
| `IDF:` | ESP-IDF **v5.5.1** (Apache-2.0). The reference app is built from v5.5.2-729 (Arduino lib-builder, see `docs/research/xteink-hardware.md`); no register-level difference was looked for between 5.5.1 and 5.5.2 (unverified, but headers under `components/soc/esp32c3` are stable). | `https://github.com/espressif/esp-idf/blob/v5.5.1/<path>` |
| `QEMU:` | Espressif QEMU fork, branch `esp-develop` (GPL). Read only to extract facts (addresses, which registers a model must answer, sequencing). No code or comment text reproduced. | `https://github.com/espressif/qemu/blob/esp-develop/<path>` |
| `ROM3:` / `ROM0:` / `ROM101:` | **Disassembly of the real mask ROMs** from `espressif/esp-rom-elfs` release `20260528` (Apache-2.0, already the ROM chosen in ADR 0001). Cited as `symbol@address`. This is the authoritative source for sections G, H (ROM side), J: it shows exactly which registers the ROM touches and in which order. | `https://github.com/espressif/esp-rom-elfs/releases/tag/20260528` (`esp32c3_rev3_rom.elf` sha256 `19ac22e08707df926fb0cf4c54795d4067b983fae8635f396ded173a6d78fc3c`, `rev0` `11d56592…0694`, `rev101` `9495e145…e8336`) |
| `APPNOTE:` | project note | `docs/research/xteink-hardware.md` |

Local copies (scratch, not in the repo): `…/scratchpad/boot/idf/` (IDF files), `…/boot/qemu/` (QEMU files), `…/boot/romelf/dis_rev3.txt` (llvm-objdump of the rev3 ROM), `…/boot/trace_main.txt` (call-graph + constant-propagated MMIO accesses from `main`), `…/boot/tools/{trace,mmio,elfstr}.py`.

**ROM revision selection** (README of `esp-rom-elfs`: "use the next lower one"):

| ELF | `_rom_eco_version` @0x40000014 | Banner (`ESP-ROM:%s`) | Build string | Used for chip revision |
|---|---|---|---|---|
| `esp32c3_rev0_rom.elf` | 0 | `esp32c3-20200918` | `Sep 18 2020` | v0.0–v0.2 |
| `esp32c3_rev3_rom.elf` | 3 | `esp32c3-api1-20210207` | `Feb  7 2021` | v0.3–v1.0 (v0.3/v0.4 are the common retail parts) |
| `esp32c3_rev101_rom.elf` | 7 | `esp32c3-eco7-20230720` | `Jul 20 2023` | v1.1 |

Sources: `ROM3:_rom_eco_version@0x40000014`, banner strings read from `.rodata` (`0x3ff1b86c` in rev3, `0x3ff1b858` rev0, `0x3ff1bb58` rev101); `IDF:components/esp_rom/esp32c3/ld/esp32c3.rom.version.ld` (`_rom_chip_id=0x40000010`, `_rom_eco_version=0x40000014`); the code at `ROM3:_start` reads nothing from them, they are plain data words (`_rom_chip_id` = 5).
Which revision the retail Xteink X4 uses: **unknown** (eFuse of a real unit not available). The rest of this report is written for **rev3** (the QEMU ROM is also "api1-20210207": `IDF:docs/en/security/esp32c3_log.inc`, `examples/security/security_features_app/README.md`).

---

## A. Memory map

### A.1 Address space (all sizes in bytes)

| Region | Bus | Range | Size | Notes / source |
|---|---|---|---|---|
| Mask ROM code | I-bus | `0x4000_0000`–`0x4005_FFFF` | 0x60000 (384 KiB) | `IDF:soc.h SOC_IROM_MASK_LOW/HIGH`; reset PC; QEMU `esp32c3_memmap[IROM]` base 0x40000000 size 0x60000 |
| Mask ROM data | D-bus | `0x3FF0_0000`–`0x3FF1_FFFF` | 0x20000 (128 KiB) | `IDF:soc.h SOC_DROM_MASK_LOW/HIGH`. **Alias of ROM physical offset 0x40000**: `ROM3` program header: `.rodata` VMA `0x3ff19c00` has LMA `0x40059c00` (= VMA + 0x40040000 − 0x3ff00000); QEMU `esp32c3.c` makes DROM an alias of IROM at offset `0x40000` |
| SRAM (D-bus view) | D | `0x3FC8_0000`–`0x3FCD_FFFF` | 0x60000 (384 KiB) | `IDF:soc.h SOC_DRAM_LOW/HIGH` |
| SRAM (I-bus view, same cells) | I | `0x4038_0000`–`0x403D_FFFF` | 0x60000 | `SOC_DIRAM_IRAM_LOW/HIGH`; `SOC_I_D_OFFSET = 0x700000` (bootloader.ld `iram_dram_offset`) |
| SRAM0 (I-bus only) | I | `0x4037_C000`–`0x4037_FFFF` | 0x4000 (16 KiB) | `SOC_IRAM_LOW = 0x4037C000`; QEMU merges it in front of SRAM1 (`ESP32C3_INTERNAL_SRAM0_SIZE`). Used as ICache storage once the cache is on — **inferred** from `SOC_CACHE_MEMORY_IBANK_SIZE 0x4000` + the app IRAM segment starting at `0x40380000` (APPNOTE: entry `0x4038086E`, IRAM seg `0x40380000`) |
| RTC memory | I+D | `0x5000_0000`–`0x5000_1FFF` | 0x2000 (8 KiB) | `SOC_RTC_IRAM/DRAM/DATA_LOW/HIGH`; IDF comment says C3 has only "slow" RTC memory; QEMU `ESP32C3_MEMREGION_RTCFAST` 0x50000000 size 0x2000 |
| Flash cache window, D-bus | D | `0x3C00_0000`–`0x3C7F_FFFF` | 8 MiB | `IDF:ext_mem_defs.h SOC_DRAM0_CACHE_ADDRESS_LOW/HIGH` |
| Flash cache window, I-bus | I | `0x4200_0000`–`0x427F_FFFF` | 8 MiB | `SOC_IRAM0_CACHE_ADDRESS_LOW/HIGH`. **Same MMU entries as the D window** (entry index = `(vaddr & 0x7FFFFF) >> 16`, `mmu_ll_get_entry_id`), QEMU aliases the two windows onto one region |
| Peripherals | — | `0x6000_0000`–`0x600F_FFFF` | 1 MiB | `SOC_PERIPHERAL_LOW/HIGH` |
| Debug region | — | `0x2000_0000`–`0x27FF_FFFF` | 128 MiB | `SOC_DEBUG_LOW/HIGH`; only a PMP entry in IDF (`cpu_region_protect.c`), not a device |

ROM-reserved DRAM (bootloader must not touch): `0x3fccae00–0x3fcdc710` shared buffers (download modes), `0x3fcdc710–0x3fcde710` ROM boot stack (`SOC_ROM_STACK_START = 0x3fcde710`, size `0x2000`), `0x3fcde710–0x3fce0000` ROM `.bss/.data` (`IDF:components/bootloader/subproject/main/ld/esp32c3/bootloader.ld` header comment). The ROM ELF also has a `PT_LOAD` at `0x3fcc9000` (memsz 0x157b8) for its shared buffers + stack.
Bootloader link addresses: IRAM loader seg starts `0x403ce710` (assert in `bootloader.ld`), seen as `load:0x403cc710…` in a real ROM log (`examples/security/security_features_app/README.md`).

**Recommendation (not a fact):** load the ROM ELF `PT_LOAD` segments at their VMAs/LMAs (`.text` 0x40000000–0x40059590, `.rodata` copied from LMA 0x40059c00 to VMA 0x3ff19c00, etc.). The ROM's own `_init` (`ROM3:_init@0x40001e90`) performs the `.data` unpack (`unpackloop@0x40001ef8`) and `.bss` clear (`clearloop@0x40001f2a`), so only the read-only parts must be pre-populated.

### A.2 Peripheral bases (all from `IDF:components/soc/esp32c3/register/soc/reg_base.h`)

"Need": R = touched by the ROM boot path (from `trace_main.txt`), B = IDF 2nd-stage bootloader, A = early app (esp_system init), – = not needed for first boot.

| DR_REG_* | Base | Need | Remarks |
|---|---|---|---|
| UART0 (`DR_REG_UART_BASE`) | `0x6000_0000` | R B A | ROM console, bootloader log (default config) |
| SPI1 (`DR_REG_SPI1_BASE`) | `0x6000_2000` | R B A | flash controller used by user-commands (RDID, RDSR, WREN, PP, SE…) |
| SPI0 (`DR_REG_SPI0_BASE`) | `0x6000_3000` | R B A | cache-read path; ROM and IDF only read-modify-write config regs here |
| GPIO | `0x6000_4000` | R B A | `GPIO_STRAP_REG` = +0x38 |
| RTC_CNTL (`DR_REG_RTCCNTL_BASE`) | `0x6000_8000` | R B A | reset cause, STORE regs, WDT, SWD, pad hold, clocks |
| EFUSE | `0x6000_8800` | R B A | |
| IO_MUX | `0x6000_9000` | R B A | pad config; ROM sets GPIO20/21 and flash pads |
| RTC_I2C / analog I2C master (`DR_REG_RTC_I2C_BASE`) | `0x6000_E000` | R? B A | regi2c: `I2C_MST_ANA_CONF0_REG 0x6000E040`, `ANA_CONFIG_REG 0x6000E044`, `ANA_CONFIG2_REG 0x6000E048` (`regi2c_defs.h`). QEMU answers every read in 0x6000E000–0x6000E0FF with `0xffffff` so `rom_i2c_*` busy-waits end |
| UART1 | `0x6001_0000` | R (INT_CLR only) | |
| TIMG0 | `0x6001_F000` | B A | MWDT0 + RTC calibration |
| TIMG1 | `0x6002_0000` | A | MWDT1 |
| SYSTIMER | `0x6002_3000` | A | OS tick + esp_timer |
| SPI2 (GP-SPI) | `0x6002_4000` | – | ROM `spi_slave_rom_check_conn` reads `0x600240a4` only in download-mode detection |
| SYSCON / APB_CTRL (`DR_REG_SYSCON_BASE`, same address) | `0x6002_6000` | A | `SYSCON_RND_DATA_REG` = +0xB0 = `RNG_DATA_REG` = `WDEV_RND_REG` |
| TWAI | `0x6002_B000` | – | |
| AES / SHA / RSA / DS / HMAC | `0x6003_A000` / `B000` / `C000` / `D000` / `E000` | R (SHA) | SHA is mandatory for the ROM image-hash check |
| GDMA | `0x6003_F000` | – | |
| APB_SARADC | `0x6004_0000` | B | bootloader RNG-entropy setup writes it (write-ignore is enough) |
| USB_SERIAL_JTAG | `0x6004_3000` | R B A | console channel 2 of the ROM, Arduino `Serial` (HWCDC) |
| SYSTEM | `0x600C_0000` | R B A | clocks, resets, CPU_INTR_FROM_CPU |
| SENSITIVE | `0x600C_1000` | A | memprot/PMS (IDF default `ESP_SYSTEM_MEMPROT_FEATURE=y`, `IDF:components/esp_system/Kconfig`) |
| INTERRUPT (intmatrix) | `0x600C_2000` | R B A | |
| EXTMEM (cache regs) | `0x600C_4000` | R B A | |
| MMU table (`DR_REG_MMU_TABLE`) | `0x600C_5000` | R B A | 0x200 bytes |
| XTS_AES (flash encryption) | `0x600C_C000` | – | |
| ASSIST_DEBUG | `0x600C_E000` | R B A | ROM touches +0x44/+0x48; app HW stack guard (`ESP_SYSTEM_HW_STACK_GUARD` default y) |
| DEDICATED_GPIO / WORLD_CNTL | `0x600C_F000` / `0x600D_0000` | – | |

QEMU's minimal ROM→app device set (all it instantiates, `hw/riscv/esp32c3.c`): intmatrix, UART0/1, GPIO (only strap), EXTMEM+MMU, eFuse, SYSTEM (4 CPU_INTR regs, CPU_PER_CONF, SYSCLK_CONF, ext-dev-enc ctrl), TIMG0/1, SYSTIMER, SPI1, RTC_CNTL (OPTIONS0, RESET_STATE, STORE0–7 only), USB-Serial-JTAG (stub), SHA/AES/RSA/HMAC/DS/XTS-AES, GDMA, TWAI. Everything else in `0x60000000–0x600D0FFF` reads 0 and ignores writes (`esp32c3_io_read/write`, iomem size 0xd1000). **SPI0 is not modelled** there: the ROM boots because QEMU fills the cache window when the MMU entry is written.

---

## B. CPU and interrupts

### B.1 Core

| Item | Fact | Source |
|---|---|---|
| Reset PC | `0x4000_0000` (`_start`: `j _init` → `0x40001e90`) | `QEMU:esp32c3.c ESP32C3_RESET_ADDRESS`; `ROM3:_start@0x40000000`; ELF entry |
| ISA | RV32IMC + Zicsr + Zifencei; no A, no F | `IDF:tools/cmake/toolchain-esp32c3.cmake` (`-march=rv32imc_zicsr_zifencei`); `QEMU:target/riscv/esp_cpu.c esp_cpu_init` (`RVI|RVM|RVC`) |
| Privilege | ROM and IDF (non-TEE) run in M-mode only. `U-mode` CSRs exist (ROM writes `ustatus`), but no code drops to U-mode unless `CONFIG_SECURE_ENABLE_TEE` (off by default; `vectors.S` has `ucause/ustatus` variants only under that flag) | `ROM3:_init`; `IDF:components/riscv/vectors.S` |
| mtvec | vectored mode only used: ROM writes `(0x40001d00 | 1)`; IDF writes `vector_table | MTVEC_MODE_CSR(=1)` (`rv_utils_set_mtvec`). "CPU only uses the 24 MSBs of MTVEC": base is 256-byte aligned. Exception → pc = base; interrupt n → pc = base + 4·n | `ROM3:_init@0x40001ebe`; `IDF:components/riscv/vectors_intc.S` (comment), `rv_utils.h rv_utils_set_mtvec`; `QEMU:esp_cpu.c esp_cpu_exec_interrupt` implements `pc = (mtvec>>2<<2) + (vectored ? cause*4 : 0)` |
| mcause for IRQ | `0x80000000 | n`, n = CPU interrupt line 1..31 (not the standard 11/7/3) | `QEMU:esp_cpu.c`; `IDF:vectors.S` masks with `VECTORS_MCAUSE_REASON_MASK` |
| ROM entry state | `mstatus=0; ustatus=0; INTERRUPT.CPU_INT_ENABLE=0; CSR0x800=1; CSR0x801=1; gp=0; mtvec; INTERRUPT.THRESH=1; mstatus=9 (MIE|UIE); ustatus=1; sp=0x3FCDE710` | `ROM3:_init@0x40001e90–0x40001ee0` (sp computed from the `auipc`) |
| WFI | IDF idle uses `wfi` (`rv_utils_wait_for_intr`). Whether C3 wakes with `mstatus.MIE=0` is **unknown** | `IDF:rv_utils.h` |
| Misaligned loads/stores | **Unknown** for silicon (settled, see §M.1). IDF has a Kconfig for "optimized memXXX on misaligned access" and a ROM linker script "suboptimal for misaligned memory", so they appear functional but slow on RAM; `panic_arch.c` still names "Load/Store address misaligned" exceptions. The project spec already says "désalignement en exception" for the RISCOF config; keep that as the conservative default and test with the real firmware | `IDF:components/newlib/Kconfig LIBC_OPTIMIZED_MISALIGNED_ACCESS`; `components/esp_rom/esp32c3/ld/esp32c3.rom.libc-suboptimal_for_misaligned_mem.ld` |
| `mie` / `mip` | Neither the ROM nor IDF ever reads/writes `mie`/`mip` (exhaustive CSR scan of the ROM: below; grep of `components/riscv`). Interrupt delivery is gated by the INTC registers + `mstatus.MIE` only. QEMU force-sets MEIE in `mie` at reset (`riscv_csr_write(CSR_MIE, BIT(12)-1)`). **Silicon behaviour of `mie` is unknown**; recommended model: ignore `mie` for lines 1..31, treat `mie`/`mip` as plain registers | `ROM3` CSR scan; `QEMU:esp32c3_intmatrix.c esp32c3_intmatrix_reset_hold` |
| Interrupt arbitration (several pending) | **Unknown.** QEMU delivers the lowest asserted line number and ignores priority (`esp_cpu_select_irq_cause`). Silicon presumably picks the highest priority; not established here | |

### B.2 CSRs the software actually uses

ROM (rev3), exhaustive scan of the disassembly (`dis_rev3.txt`, `csr*` instructions):

| CSR | Number | Use | Where |
|---|---|---|---|
| mstatus | 0x300 | write 0, write 9; `csrs/csrc 8` (MIE) in interrupt entry/exit and `ets_intr_lock/unlock` | `_init`, `_panic_handler@0x40001d80`, `0x40047a2e/0x40047a52` |
| mtvec, mepc, mcause, mtval | 0x305, 0x341, 0x342, 0x343 | trap path | `_panic_handler@0x40001d80`, `_interrupt_exit@0x40001e2c` |
| ustatus | 0x000 | `csrw 0, zero` then `csrw 0, 1` | `_init@0x40001e94/0x40001ed8` (QEMU registers ustatus/uie/utvec/uscratch/uepc/ucause/utval/uip as no-fault) |
| user perf CSRs | 0x800, 0x801 (write 1), **0x802 (read)** | `0x802` is the cycle counter read by `ets_delay_us@0x400462cc` (busy-wait `cycles < us × g_ticks_per_us`). **Must advance with CPU cycles or the ROM hangs** | `_init@0x40001ea6/0x40001eac`, `0x400462cc/0x400462dc`, and the BT code |
| tselect / tdata1 / tdata2 / 0x7A5 | 0x7A0–0x7A2, 0x7A5 | single-step re-execute inside `ets_fatal_exception_handler@0x40052b0e` only | |

IDF app side (C3, default config):

| CSR | Number | Use | Source |
|---|---|---|---|
| mstatus, mtvec(|1), mepc, mcause, mtval, mhartid | std | vectors/panic | `components/riscv/vectors.S`, `rv_utils.h` |
| pmpcfg0–3, pmpaddr0–15 | 0x3A0–0x3A3, 0x3B0–0x3BF | `esp_cpu_configure_region_protection`: 16 **locked** TOR entries covering debug, DROM-cache, DRAM, mask ROM, IRAM, IROM-cache, RTC, peripherals (`C3 doesn't support overlapping PMP`). Locked entries bind M-mode too, so a model that enforces PMP will fault on holes (e.g. 0x3C800000–0x3FC7FFFF is "none") | `components/esp_hw_support/port/esp32c3/cpu_region_protect.c` (caller not located in this pass) |
| tselect, tdata1, tdata2, tcontrol (0x7A5) | 0x7A0–0x7A2, 0x7A5 | HW breakpoints/watchpoints, 8 each (`SOC_CPU_BREAKPOINTS_NUM`/`WATCHPOINTS_NUM`) — only when software arms them | `rv_utils.h`, `soc_caps.h` |
| PCER_M / PCMR_M / PCCR_M | 0x7E0 / 0x7E1 / 0x7E2 | machine cycle counter (`SOC_CPU_HAS_CSR_PC=1`); `rv_utils_get_cycle_count` reads 0x7E2 | `rv_utils.h`, `soc_caps.h:135` |
| user mirrors | 0x800 / 0x801 / 0x802 | same counter | `rv_utils.h` |
| MEXSTATUS 0x7E1, MHINT 0x7C5, PRV_MODE 0x810 | — | **not C3** (C6/H2 definitions in the shared `csr.h`; 0x7E1 on C3 is PCMR) | `components/riscv/include/riscv/csr.h` |

### B.3 Interrupt controller (`INTERRUPT_CORE0`, base `0x600C2000`)

Source for every row: `IDF:components/soc/esp32c3/register/soc/interrupt_core0_reg.h` unless stated.

| Offset | Register | Reset | Semantics |
|---|---|---|---|
| `0x000 + 4·src` | `<SRC>_MAP` for `src` = 0..61 (62 registers, last `CACHE_CORE0_ACS_INT_MAP` @0xF4) | 0 | bits[4:0] = target CPU interrupt (0 = none; QEMU masks `& 0x1f`). `intr_matrix_route` writes `DR_REG_INTERRUPT_BASE + 4·src` (`IDF:hal/interrupt_intc_ll.h`) |
| 0x0F8 / 0x0FC | `INTR_STATUS_0/1` | RO | raw source status, src 0–31 / 32–61 |
| 0x100 | `CLOCK_GATE` | 1 | bit0 clk_en |
| 0x104 | `CPU_INT_ENABLE` | 0 | bit n enables CPU line n (ROM clears it at reset) |
| 0x108 | `CPU_INT_TYPE` | 0 | bit n: 0 level, 1 edge (`esprv_int_get_type`). QEMU only supports level |
| 0x10C | `CPU_INT_CLEAR` | 0 | write 1 to bit n to clear an edge-pending interrupt (`rv_utils_intr_edge_ack`) |
| 0x110 | `CPU_INT_EIP_STATUS` | RO | pending status per CPU line |
| 0x114 + 4·n | `CPU_INT_PRI_n`, n = 0..31 | 0 | bits[3:0] priority of CPU line n (`INTERRUPT_PRIO_REG(n) = PRI_0 + 4n`; QEMU decodes lines 1..31 from 0x118) |
| 0x194 | `CPU_INT_THRESH` | 0 | bits[3:0] threshold |
| 0x7FC | `INTERRUPT_DATE` | | |

Semantics: "all interrupt priority levels **strictly less than** the threshold level are masked" (`IDF:components/riscv/include/esp_private/interrupt_intc.h`), so a line fires iff `enabled && source_asserted && prio >= thresh` — QEMU implements exactly this. Priorities IDF uses are 1–7 (`RVHAL_INTR_ENABLE_THRESH = 1`). No hardware nesting: `vectors.S` raises `THRESH` to `PRI[mcause]+1` on entry and restores it on exit (`!SOC_INT_HW_NESTED_SUPPORTED`), ROM `_panic_handler` does the same (`ROM3@0x40001dee…0x40001e04`).
Source → line fan-in: several sources may share one line (OR of levels, `esp32c3_get_output_line_level`).
Reserved CPU lines: **0** invalid (`assert_valid_rv_int_num`); **1** Wi-Fi; **6** "permanently disabled" (`INT_MUX_DISABLED_INTNO`) — `esp_cpu_intr_get_desc` rsvd mask `BIT(1)|BIT(6)`; **24** `ETS_INT_WDT_INUM`, **25** cache error, **26** memprot, **27** assist-debug are panic-class vectors (`IDF:vectors_intc.S`, `soc.h:210-216`). ROM uses CPU line **5** for UART0 (`ROM3:Uart_Init@0x4004c334`: `INTERRUPT.UART_INTR_MAP(0x600C2054)=5`, `PRI_5=1`).

Source numbers needed by SP1b (index = `offset/4` in the MAP block, cross-checked with `IDF:soc/interrupts.h`): GPIO 16, SPI1 18, UART0 21, UART1 22, EFUSE 24, USB_SERIAL_JTAG 26, RTC_CORE 27, TG0_T0 32, TG0_WDT 33, TG1_T0 34, TG1_WDT 35, CACHE_IA 36, SYSTIMER_TARGET0/1/2 37/38/39, SPI_MEM_REJECT 40, ICACHE_PRELOAD 41, ICACHE_SYNC 42, SHA 49, FROM_CPU_INTR0–3 50–53, ASSIST_DEBUG 54, CACHE_CORE0_ACS 61.

---

## C. Flash cache and MMU

| Item | Fact | Source |
|---|---|---|
| Cache type | One shared I/D cache ("ICache"), `SOC_SHARED_IDCACHE_SUPPORTED=1` | `IDF:soc_caps.h:127` |
| Geometry | **16 KiB, 8-way, 32-byte lines** → 64 sets, 512 lines. `MAX_ICACHE_SIZE 16384`, `MAX_ICACHE_WAYS 8`, `MIN_CACHE_LINE_SIZE 32`, 1 bank of `0x4000`. (QEMU header also uses 32-byte × 512 blocks) | `IDF:components/esp_rom/esp32c3/include/esp32c3/rom/cache.h`; `QEMU:esp32c3_cache.h` |
| Page size | 64 KiB, fixed (`mmu_ll_get_page_size`) | `IDF:hal/esp32c3/include/hal/mmu_ll.h` |
| Table | `0x600C5000`, **128 entries** (`SOC_MMU_ENTRY_NUM 128`, `0x200` bytes), 8 MiB of virtual space | `reg_base.h DR_REG_MMU_TABLE`; `soc/mmu.h`, `ext_mem_defs.h` (`CACHE_*_MMU_END 0x200`) |
| Entry format | bits[7:0] = flash page number (64 KiB units → ≤16 MiB, `SOC_MMU_MAX_PADDR_PAGE_NUM 256`, `SOC_MMU_VALID_VAL_MASK 0xff`); **bit 8 = invalid** (`SOC_MMU_INVALID = BIT(8)`, `SOC_MMU_VALID = 0`); other bits 0. `mmu_ll_write_entry` stores `val | SOC_MMU_ACCESS_FLASH(0) | SOC_MMU_VALID(0)` | `ext_mem_defs.h`, `mmu_ll.h`; `QEMU:esp32c3_cache.h ESP32C3MMUEntry` |
| Translation | `entry = table[(vaddr & 0x7FFFFF) >> 16]`; flash offset = `entry.page·0x10000 + (vaddr & 0xFFFF)`; same table for 0x3C000000 and 0x42000000 | `mmu_ll_get_entry_id`, `mmu_ll_entry_id_to_paddr_base` |
| Typical layout (reference app) | DROM seg vaddr `0x3C180020` → entry 0x18; IROM seg `0x42000020` → entry 0; the two use disjoint entries | APPNOTE (`x3-crosspoint.bin` segments) |
| State after ROM | ROM calls `Cache_MMU_Init` (writes all 128 entries invalid) right before jumping to the bootloader; bootloader then `mmu_hal_unmap_all()`, maps what it needs, and `set_cache_and_start_app` re-maps DROM/IROM for the app | `ROM3:ets_run_flash_bootloader@0x40049d20` tail (`Cache_MMU_Init@0x40051670`); `IDF:bootloader_utility.c set_cache_and_start_app` |
| ROM flash window | `ets_loader_map_range` maps **at most 1 MiB** at D-bus `0x3C000000` (`Cache_Dbus_MMU_Set(ext=0, vaddr=0x3C000000, paddr, 64 KiB, n pages, fixed=0)`), reads through it | `ROM3:ets_loader_map_range@0x400499f6`, `Cache_Dbus_MMU_Set@0x40051756` |

### C.1 EXTMEM registers (base `0x600C4000`) the software touches

Offsets and bit positions from `IDF:components/soc/esp32c3/register/soc/extmem_reg.h` and `QEMU:esp32c3_cache.h`; access sequences from the ROM disassembly.

| Offset | Register | Bits used | Behaviour a model must provide |
|---|---|---|---|
| 0x000 | `ICACHE_CTRL` | b0 ENABLE | `Cache_Enable_ICache` sets b0, `Cache_Suspend` clears, `Cache_Resume` sets. Read back as written |
| 0x004 | `ICACHE_CTRL1` | b0 SHUT_IBUS, b1 SHUT_DBUS | `Cache_Mask_All` sets both (first ROM action); `Cache_UnMask_Dram0` clears b1; suspend sets b0|b1; resume clears them. Plain R/W |
| 0x028 | `ICACHE_SYNC_CTRL` | b0 INVALIDATE_ENA, b1 SYNC_DONE | software sets b0; hardware clears b0 and sets b1 when finished. QEMU does this instantly on the next read |
| 0x034 | `ICACHE_PRELOAD_CTRL` | b0 ENA, b1 DONE, b2 ORDER | **reset value must have DONE=1**: `Cache_Suspend_ICache` spins on `(reg+0x34)&2` (`ROM3:Cache_Suspend_ICache@0x40051446`). Same instant-completion rule |
| 0x040 | `ICACHE_AUTOLOAD_CTRL` | b0 SCT0_ENA, b1 SCT1_ENA, b2 ENA, b3 DONE, b4 ORDER, b6:5 RQST | ~~`ROM_Boot_Cache_Init` sets b0, b1 then clears b1; reset value DONE=1 (QEMU)~~ Only read on the boot path: `Cache_Suspend_ICache_Autoload` polls b3 (`DONE`), which must read 1; b2 (`ENA`) is a stored configuration bit. `ROM_Boot_Cache_Init` does **not** write this register (it writes `SYSTEM.CACHE_CONTROL`) (corrected 2026-10-11, plan SP1b-2: see `esp32c3-cache-mmu.md` §0.1 and `esp32c3-rom-flash-boot.md` §K) |
| 0x0B0 | `CACHE_STATE` | b11:0 ICACHE_STATE | QEMU returns 1 (idle) |
| 0x0CC | `ICACHE_FREEZE` | b0 ENA, b1 MODE, b2 DONE | `Cache_Freeze_ICache_Enable` → set DONE when ENA set; clear DONE when ENA cleared. This is required: `Cache_Suspend_ICache` spins on it (corrected 2026-10-11, plan SP1b-2: confirmed and completed by `esp32c3-cache-mmu.md` §0.1) |
| 0x078–0x08C | `CACHE_ILG_INT_*`, `CORE0_ACS_CACHE_INT_*` | | IDF `cache_ll_l1_*_intr`; model as R/W + `INT_ST = RAW & ENA`; never raised in a happy-path boot |
| 0x0DC–0x0FC | `IBUS/DBUS_PMS_TBL_*` | | `Cache_Ibus_Pms_*` (app, memprot) — plain R/W |
| 0x054–0x060 | `IBUS/DBUS_TO_FLASH_START/END_VADDR` | | plain R/W |
| 0x100 | `CLOCK_GATE` | | plain R/W |

Timing hook for the planned cache-miss cost: a miss = one 32-byte line fill over SPI0 in the configured read mode; nothing in the sources gives a cycle count (**unknown**; the spec already says "calculated from DIO 80 MHz").

ROM cache API relevant to a model (all in the ROM jump table, `IDF:components/esp_rom/esp32c3/ld/esp32c3.rom.ld`): `Cache_MMU_Init@0x4000055c`, `Cache_Ibus_MMU_Set@0x40000560`, `Cache_Dbus_MMU_Set@0x40000564`, `Cache_Enable_ICache@0x40000520`, `Cache_Disable_ICache@0x4000051c`, `Cache_Suspend_ICache@0x40000524`, `Cache_Resume_ICache@0x40000528`, `Cache_Freeze_ICache_Enable/Disable@0x4000052c/0x40000530`, `Cache_Invalidate_Addr@0x400004d4`, `Cache_Invalidate_ICache_All@0x400004d8`, `ROM_Boot_Cache_Init@0x400004c8`. These are executed by the real ROM; a model that reproduces the register behaviour above makes them work unmodified. They are jump-table entries (`__call_*: j <impl>`), so HLE-hooking by address is possible if needed.

---

## D. SPI flash controller (MSPI) and boot-mode selection

### D.1 Register block (SPI1 `0x60002000`, SPI0 `0x60003000`; offsets identical, `IDF:…/register/soc/spi_mem_reg.h`)

`CMD 0x00` · `ADDR 0x04` · `CTRL 0x08` · `CTRL1 0x0C` · `CTRL2 0x10` · `CLOCK 0x14` · `USER 0x18` · `USER1 0x1C` · `USER2 0x20` · `MOSI_DLEN 0x24` · `MISO_DLEN 0x28` · `RD_STATUS 0x2C` · `MISC 0x34` · `TX_CRC 0x38` · `CACHE_FCTRL 0x3C` · `FSM 0x54` · `W0..W15 0x58..0x94` · `FLASH_WAITI_CTRL 0x98` · `FLASH_SUS_CTRL 0x9C` · `FLASH_SUS_CMD 0xA0` · `SUS_STATUS 0xA4` · `TIMING_CALI 0xA8` · `DIN_MODE 0xAC` · `DIN_NUM 0xB0` · `DOUT_MODE 0xB4` · `INT_ENA/CLR/RAW/ST 0xC0..0xCC` · `CLOCK_GATE 0xDC` · `CORE_CLK_SEL 0xE0` · `DATE 0x3FC`.

`CMD` bits (SPI1): b31 FLASH_READ, b30 WREN, b29 WRDI, b28 RDID, b27 RDSR, b26 WRSR, b25 PP, b24 SE, b23 BE, b22 CE, b21 DP, b20 RES, b19 HPM, **b18 USR**, b17 PE, b1 PES, b0 PER. All are self-clearing: software triggers by writing a bit and polls `CMD == 0` (`spimem_flash_ll_host_idle`: `dev->cmd.val == 0`).

### D.2 What the ROM does (from `trace_main.txt`)

1. `spi_flash_boot_attach@0x4004dca8`: `SelectSpiFunction` (IO_MUX 0x6000903C/40/44/48 RMW, GPIO matrix `0x60004020/24`), `SPI_init` (SYSTEM `PERIP_CLK_EN0 b1` + `PERIP_RST_EN0 b1` pulse = SPI01), SPI0/SPI1 `CLOCK`, `CTRL`, `MISC(0x34)`, `USER/USER1/USER2`, `CACHE_FCTRL(0x3C)`, CS timing (`0x…10/18`), `esp_rom_spiflash_config_readmode`, ~~`SPI_WakeUp@0x4004d1c8` (write `CMD`, poll)~~. With `a2 == 0` the attach does **not** wake the flash up: `SPI_WakeUp@0x4004d1c8` is called later, from `ets_run_flash_bootloader` (corrected 2026-10-11, plan SP1b-2: see `esp32c3-rom-flash-boot.md` §K).
2. `Wait_SPI_Idle@0x4004d1fe` polls **SPI0 `FSM(0x54)` bits[6:4] == 0** and **SPI1 `CMD` bits[3:0] == 0**, ~~then reads status (RDSR via `esp_rom_spiflash_read_user_cmd@0x4004d0cc`: `USER=0x90000000` (b31 command, b28 miso), `USER2=0x70000000|cmd`, `MISO_DLEN=7`, `W0` cleared, `CMD=0x40000` (USR), poll, result in `W0 & 0xFF`)~~. The status is read with the hard-wired command: `CMD = 0x08000000` (`FLASH_RDSR`), result in `RD_STATUS`. This happens twice, inside `ets_config_flash_by_image_hdr`, not in the attach. The `USR` variant `esp_rom_spiflash_read_user_cmd` is not reached on this path (corrected 2026-10-11, plan SP1b-2: see `esp32c3-rom-flash-boot.md` §K and §D.1).
3. Flash ID: `esp_rom_spi_flash_update_id@0x4004db12` → `CMD=0x10000000` (b28 RDID), poll, `W0 & 0xFFFFFF` = bytes [manufacturer, type, capacity] little-endian (manufacturer in `W0[7:0]`); ROM then rotates to `0xMMTTCC` form (`(id&0xFF00)|(id>>16&0xFF)|(id<<16&0xFF0000)`) and stores it.
4. Bootloader read: `ROM_Boot_Cache_Init@0x40050ee6` (~~EXTMEM `AUTOLOAD_CTRL`, `Cache_MMU_Init`, `Cache_Enable_ICache`, `Cache_UnMask_Dram0`~~ `SYSTEM.CACHE_CONTROL` ×3, `CACHE_MMU_OWNER |= 0xF`, `Cache_MMU_Init`, `CACHE_ILG_INT_CLR`, `CACHE_WRAP_AROUND_CTRL`, `SENSITIVE.INTERNAL_SRAM_USAGE_1 &= ~1`, invalidate-all, `Cache_Enable_ICache`, `Cache_UnMask_Dram0`; corrected 2026-10-11, plan SP1b-2: see `esp32c3-cache-mmu.md` §0.1 and §5.1) then `ets_loader_map_range` — **the bootloader image is read through the MMU/cache window, not through SPI1 user commands**. The data read path (SPI0 cache refill) therefore only needs to return the flash image bytes; the SPI0 register file only needs R/W storage.
5. `ets_config_flash_by_image_hdr@0x40049b2e` re-configures clock divider / read mode from the image header (see I) and may touch IO_MUX 0x60009034/38 (WP/HD pads) and the GPIO matrix.
6. Opcodes (IDF `components/spi_flash/include/spi_flash/spi_flash_defs.h`): RDID 0x9F, RDSR 0x05, RDSR2 0x35, WRSR 0x01, WRSR2 0x31, WREN 0x06, WRDI 0x04, READ 0x03, FAST_READ 0x0B, DUAL 0x3B, QUAD 0x6B, DIO 0xBB, QIO 0xEB, SE 0x20, BE64K 0xD8, CE 0xC7, PP 0x02, suspend 0x75/resume 0x7A, reset-enable/reset 0x66/0x99, SFDP 0x5A, HPM 0xA3, release-from-power-down 0xAB (`SPI_WakeUp`). The QEMU SPI1 model implements `FLASH_READ`, `WREN`, `WRDI`, `RDID`, `RDSR`, `WRSR`, `PP`, `SE`, `BE`, `CE`, `DP`, `RES`, `HPM` and the generic `USR` path (command/address/dummy/data from `USER*`, `MOSI/MISO_DLEN`, `W0..W15`), enough for esptool and IDF (`QEMU:esp32c3_spi.c`).

Flash expectations: `bootloader_flash_xmc_startup` runs only for XMC parts (manufacturer 0x20) — avoid that vendor. IDF has dedicated chip drivers for GD (0xC8, e.g. `0xC84018` 16 MiB), Winbond (0xEF, `0xEF4018`), ISSI, MXIC, TH, Boya, plus a generic fallback (`components/spi_flash/spi_flash_chip_{gd,winbond,…}.c`). The real X4 flash vendor is **unknown** (APPNOTE: 16 MB, DIO 80 MHz); recommended default `0xC84018` or `0xEF4018`. eFuse SPI pad config blank ⇒ default pads (`ets_efuse_get_spiconfig` returns 0, `ROM3@0x40046b3e`).

### D.3 Boot mode selection

| Item | Fact | Source |
|---|---|---|
| Strap register | `GPIO_STRAP_REG = 0x6000_4038`, RO, bits[15:0] `GPIO_STRAPPING` | `IDF:gpio_reg.h:106` |
| Decision in the ROM | **bit 3 set ⇒ SPI flash boot** (`main`: `lw 0x60004038; andi 8`); bit 3 clear ⇒ download-mode logic (UART/USB/SPI). `boot:0x%x` prints the **whole raw register**, the text is chosen from the low bits: bit3 ⇒ `SPI_FAST_FLASH_BOOT`; low nibble 2 ⇒ `UART0_BOOT`; 0 ⇒ `USB_BOOT`; `(v&12)==4` ⇒ `DOWNLOAD(USB/UART0/1)`; 1 ⇒ `SPI_DOWNLOAD_BOOT`; 3 ⇒ assert | `ROM3:main@0x40047e3c`, `boot_prepare@0x40047c5c` |
| Values seen in real logs | `0x8` (security README, QEMU default `ESP32C3_STRAP_MODE_FLASH_BOOT 0x8`), `0x9`, `0xf`, `0x2c` | `IDF:examples/security/security_features_app/README.md`, `examples/system/ulp/ulp_riscv/*/README.md`, `docs/en/security/esp32c3_log.inc`, `examples/system/deep_sleep_wake_stub/README.md`; `QEMU:include/hw/gpio/esp32c3_gpio.h` |
| Which GPIO is which bit | **unknown** from these sources (not asserted). Irrelevant for flash boot: any value with bit 3 set works. Choose `0x8` (matches QEMU and the security README) or `0xC` (the value quoted in the task statement; also bit3-set) |
| Related eFuses | `DIS_DOWNLOAD_MODE` (RD_REPEAT_DATA3 b0) etc. — see H | |

---

## E. Clocks, resets, RTC, watchdogs

### E.1 Clock registers

| Register | Address | Fields | Notes / source |
|---|---|---|---|
| `SYSTEM_CPU_PER_CONF_REG` | `0x600C0008` | b1:0 `CPUPERIOD_SEL` (0 = 80 MHz, 1 = 160 MHz), b2 `PLL_FREQ_SEL` (default 1 = 480 MHz PLL, 0 = 320 MHz), b3 `CPU_WAIT_MODE_FORCE_ON`, b7:4 `CPU_WAITI_DELAY_NUM` | `IDF:system_reg.h:42-66`; `QEMU:esp32c3_clk_defs.h` (QEMU resets it to 80 MHz/480 MHz); use of `PLL_FREQ_SEL`: `hal/clk_tree_ll.h` lines ~269-292 |
| `SYSTEM_SYSCLK_CONF_REG` | `0x600C0058` | b9:0 `PRE_DIV_CNT` (reset 1), b11:10 `SOC_CLK_SEL` (0 XTAL, 1 PLL, ROM also decodes 2, 3 as 8 MHz / 16 MHz), b18:12 `CLK_XTAL_FREQ` (**read by the ROM: must read 40**), b19 `CLK_DIV_EN` | `ROM3:ets_get_apb_freq@0x40049866`, `ets_get_xtal_freq@0x4004984c`, `ets_get_xtal_div@0x4004981c`; `IDF:system_reg.h:790-815` (header says RO default 0; QEMU resets it with XTAL=40, PLL select, PRE_DIV 1) |
| Perip clock/reset | `SYSTEM_PERIP_CLK_EN0/1` `0x600C0010/14`, `PERIP_RST_EN0/1` `0x600C0018/1C`; `CPU_PERI_CLK_EN/RST_EN` `0x600C0000/04` | bit positions in `QEMU:esp32c3_clk_defs.h` (UART_CLK_EN b2, SPI01 b1, TIMERGROUP b13, SYSTIMER b29, APB_SARADC b28, EFUSE b14…); ROM toggles UART b2, SPI01 b1; `ets_sha_enable@0x400494dc` sets `CLK_EN1 b2` (SHA) and clears RST bits in `RST_EN1`; `assist_debug_clock_enable@0x40047c1a` sets `CPU_PERI_CLK_EN b6`, clears `CPU_PERI_RST_EN b6` | plain R/W (reset pulses are write-then-clear) |
| `SYSTEM_CPU_INTR_FROM_CPU_0..3` | `0x600C0028..34` | b0 level → intmatrix sources 50–53 | software IRQs; QEMU: write sets/clears the level |
| `SYSTEM_EXTERNAL_DEVICE_ENCRYPT_DECRYPT_CONTROL` | `0x600C0044` | | flash-encryption enable (R/W; blank = off) |
| RTC fast/slow source, CK8M, XTAL32K | `RTC_CNTL_CLK_CONF 0x60008070`, `SLOW_CLK_CONF 0x60008074`, `EXT_XTL_CONF 0x60008060`, `ANA_CONF 0x60008034`, `TIMER1 0x6000801C`, `OPTIONS0 0x60008000` (BBPLL/XTAL force bits) | `IDF:hal/esp32c3/include/hal/clk_tree_ll.h`, `rtc_cntl_reg.h` | plain R/W; `RTC_CNTL_SLOW_CLK_NEXT_EDGE` (SLOW_CLK_CONF) is self-clearing (`rtc_clk_wait_for_slow_cycle`) |
| Analog (BBPLL) via regi2c | `rom_i2c_writeReg(_Mask)@0x4000195c`, `rom_i2c_readReg@0x40001954`; hardware window `0x6000E000`, `ANA_CONFIG`, `I2C_MST_ANA_CONF0 b2/b3 BBPLL_STOP_FORCE` | `regi2c_ctrl_ll.h`; the bootloader calls `rtc_clk_cpu_freq_set_config` with `CPU_CLK_FREQ_MHZ_BTLD = 80` (`soc.h:139`) unless the previous reset was a CPU SW reset at ≥80 MHz | Satisfy the ROM's I2C-master busy-wait (QEMU: read = `0xffffff`); no analog behaviour needed |

Frequencies: reset state XTAL 40 MHz, CPU = XTAL (`SOC_CLK_SEL=0`; "when ROM bootloader is running at 40 MHz" comment in `bootloader_clock_init.c`), APB = 40 MHz → UART divider for 115200 ⇒ `CLKDIV` integer 347 frag 3 (`ROM3:Uart_Init` computes `40e6·16/115200 = 5555`). Bootloader: 80 MHz via PLL; app: `CONFIG_ESP_DEFAULT_CPU_FREQ_MHZ` (reference app: 160 MHz per spec; APB always 80 MHz from PLL, `rtc_clk_apb_freq_update(80 MHz)`).

### E.2 Reset reason and RTC_CNTL

| Item | Fact | Source |
|---|---|---|
| Reset reason register | `RTC_CNTL_RESET_STATE_REG` `0x6000_8038`, `RESET_CAUSE_PROCPU` bits[5:0]; ROM `rtc_get_reset_reason(cpu)` = `(reg >> (cpu?6:0)) & 0x3F` | `ROM3:rtc_get_reset_reason@0x40048a8e`; `rtc_cntl_reg.h:547` |
| Values | 0x01 POWERON (also brown-out/super-WDT chip), 0x03 SW system reset, 0x05 deep sleep, 0x07/0x08 TG0/TG1 WDT system, 0x09 RTC WDT system, 0x0B TG0 WDT CPU, 0x0C SW CPU, 0x0D RTC WDT CPU, 0x0F brown-out, 0x10 RTC WDT RTC, 0x11 TG1 WDT CPU, 0x12 super WDT, 0x13 glitch, 0x14 eFuse CRC, 0x15 USB-UART, 0x16 USB-JTAG, 0x17 power glitch | `IDF:soc/reset_reasons.h`; names printed by the ROM: table at `ROM3 .rodata 0x3ff19da8` (`POWERON`, `RTC_SW_SYS_RST`, `DSLEEP`, `TG0WDT_SYS_RST`, `TG1WDT_SYS_RST`, `RTCWDT_SYS_RST`, `INTRUSION_RST`, `TG0WDT_CPU_RST`, `RTC_SW_CPU_RST`, `RTCWDT_CPU_RST`, `BROWNOUT_RST`, `RTCWDT_RTC_RST`, `TG1WDT_CPU_RST`, `SUPER_WDT_RST`, `GLITCH_RTC_RST`, `EFUSE_RST`, `USB_UART_CHIP_RESET`, `USB_JTAG_CHIP_RESET`, `POWER_GLITCH_RESET`; unused slots print `N/A`) |
| Software reset | `OPTIONS0` (`0x60008000`): **bit 31 `SW_SYS_RST`** (`ROM3:software_reset@0x40048b74`) → reason 3; **bit 5 `SW_PROCPU_RST`** (`software_reset_cpu(0)@0x40048b84`; bit 4 for APPCPU) → reason 0xC. IDF `esp_restart` ends with `esp_rom_software_reset_cpu(0)` after disabling TG0/TG1 WDT, enabling a 1 s RWDT with flashboot protection, disabling the cache and pulsing SYSTEM resets (`IDF:components/esp_system/port/soc/esp32c3/system_internal.c esp_restart_noos`). QEMU: both bits are self-clearing, trigger a whole-machine reset, and latch `reason` (3 / 0xC) | |
| What survives a reset | RTC_CNTL regs `STORE0..7`, RTC memory (0x50000000), reset cause. `STORE0 0x50 / 1 0x54 / 2 0x58 / 3 0x5C / 4 0xB8 / 5 0xBC / 6 0xC0 / 7 0xC4` with roles: 1 = RTC slow-clk calibration, 2/3 = boot time, **4 = XTAL freq (low and high halves identical; bit 0/16 = "disable ROM log")**, **5 = APB freq (`apb>>12` in both halves; ROM writes it @`boot_prepare+0x4c`)**, 6 = RTC entry address / reset cause, 7 = RTC memory CRC | `IDF:esp_rom/esp32c3/include/esp32c3/rom/rtc.h:48-68`; `hal/clk_tree_ll.h clk_ll_xtal_store_freq_mhz` |
| Other RTC_CNTL registers hit before the app | `PWC 0x84` b21, `DIG_ISO 0x8C` b15/b10 (pads unhold in `rtc_unhold_all_pads@0x40047bea`), `PAD_HOLD 0xD0`, `DIG_PAD_HOLD 0xD4` (`gpio_pad_unhold`), `SWD_CONF 0xAC`, `FIB_SEL 0x10C`, `ANA_CONF 0x34`, `BROWN_OUT 0xD8`, `TIME_UPDATE 0x0C` / `TIME_LOW0 0x10` / `TIME_HIGH0 0x14` | |
| RTC timer protocol | write `TIME_UPDATE` bit 31, then read `TIME_LOW0`/`TIME_HIGH0` (no valid-poll on C3: `rtc_cntl_ll_get_rtc_time`). The 48-bit counter ticks at the RTC slow clock (RC ≈ 150 kHz) | `IDF:hal/esp32c3/include/hal/rtc_cntl_ll.h:74` |
| Deep sleep / wake | `rtc_boot_control@0x40048b1c` (reads `STORE6/7`, checks CRC of RTC memory by programming the RTC CRC unit at `SYSTEM_RTC_FASTMEM_CRC 0x600C004C` / `RTC_FASTMEM_CONFIG 0x600C0048`) only runs when the reset reason is deep-sleep. Not needed for first boot; out of scope for SP1b | `ROM3:main`, `cacl_rtc_memory_crc@0x40048aae` |

### E.3 Watchdogs

| WDT | Registers | Key | Defaults / behaviour | Source |
|---|---|---|---|---|
| RTC WDT | `WDTCONFIG0 0x60008090` (b31 EN, b30:28 STG0 …, b18:16 CPU_RESET_LENGTH, b15:13 SYS_RESET_LENGTH, **b12 FLASHBOOT_MOD_EN default 1**, b11 PROCPU_RESET_EN, b9 PAUSE_IN_SLP, b8 CHIP_RESET_EN), `CONFIG1..4 0x94..0xA0` (STG0 hold default 200000, STG1 80000, STG2/3 0xFFF), `FEED 0xA4` (b31), `WPROTECT 0xA8` | **`0x50D83AA1`** (reset value of the key register; any other value locks the registers) | EN reset 0, but the hardware flash-boot protection arms it automatically; 2nd-stage bootloader comment: "the flashboot protection of RWDT and MWDT0 will have been automatically enabled" | `rtc_cntl_reg.h:1565-1710`; `IDF:bootloader_support/src/bootloader_init.c bootloader_config_wdt` |
| Super WDT | `SWD_CONF 0x600080AC` (b31 AUTO_FEED_EN, b30 DISABLE, b29 FEED, b28 RST_FLAG_CLR, b27:18 SIGNAL_WIDTH default 300, b17 BYPASS_RST, b1 FEED_INT, **b0 RESET_FLAG**), `SWD_WPROTECT 0x600080B0` | **`0x8F1D312A`** | Bootloader sets AUTO_FEED_EN with key unlock (`bootloader_super_wdt_auto_feed`). ROM reads b0 (`analog_super_wdt_reset_happened@0x40048a26`) and clears via b28 (`clear_super_wdt_reset_flag@0x40048b58`): reads 0 ⇒ no message | `rtc_cntl_reg.h:1706-1760`; `bootloader_esp32c3.c` |
| MWDT0/1 | `TIMG_WDTCONFIG0 +0x48` (b31 EN, b30:29 STG0, … b22 CONF_UPDATE_EN, b21 USE_XTAL, b20:18 CPU_RESET_LEN, b17:15 SYS_RESET_LEN, **b14 FLASHBOOT_MOD_EN default 1**, b13 PROCPU_RESET_EN), `CONFIG1 +0x4C` (clock divider), `CONFIG2..5 +0x50..5C`, `FEED +0x60`, `WPROTECT +0x64` | **`0x50D83AA1`** (`TIMG_WDT_WKEY` default 1356348065) | TG0 WDT: flashboot-armed at reset; bootloader clears `FLASHBOOT_MOD_EN` on MWDT0 (and RWDT) after init, ~~optionally starts RWDT (`CONFIG_BOOTLOADER_WDT_ENABLE`)~~ starts RWDT in the default configuration: one stage, whole-chip reset after 9 s, left running into the application (corrected 2026-10-11, plan SP1b-2: see `esp32c3-bootloader-accesses.md` §11 and §6); app re-inits MWDT0/1 (task WDT, interrupt WDT) | `timer_group_reg.h:177-360`; `bootloader_init.c` |
| Does the ROM feed/disable them? | The ROM path `main → … → ets_run_flash_bootloader` **never touches TIMG0/1 or RWDT** (`trace_main.txt` has no 0x6000_8090–0xA8 or 0x6001F/0x60020 accesses). Hence the hardware flash-boot auto-protection (reset system if it expires, ≈ seconds) is real and the bootloader must clear it in time. Exact hardware timeout/stage action of the auto-armed WDT: **unknown** | |
| Brown-out | `RTC_CNTL_BROWN_OUT_REG 0x600080D8`; bootloader `brownout_ll_ana_reset_enable(true)` for rev ≥ 2 and `FIB_SEL`/`ANA_CONF` glitch bits | `bootloader_esp32c3.c bootloader_ana_reset_config` | plain R/W, never fires |

Recommended minimum for SP1b: implement the WDT register files (key protection, feed, flashboot bits) as storage and **never fire**; add real expiry later if hang detection is wanted.

---

## F. Timers

### F.1 SYSTIMER (`0x6002_3000`, `IDF:…/register/soc/systimer_reg.h`)

| Offset | Register | Notes |
|---|---|---|
| 0x00 | `CONF` | b31 CLK_EN (reset 0; set by `systimer_ll_enable_clock`), b30 UNIT0_WORK_EN (reset **1**), b29 UNIT1_WORK_EN (reset 0), b28 UNIT0_CORE0_STALL_EN (0), b27 UNIT0_CORE1_STALL_EN (0), b26 UNIT1_CORE0_STALL_EN (reset **1**), b25 UNIT1_CORE1_STALL_EN (reset **1**), b24/23/22 TARGET0/1/2_WORK_EN, b0 SYSTIMER_CLK_FO. QEMU C3 override on read: bit 27 forced 0, bit 25 forced 1 (single core) |
| 0x04 / 0x08 | `UNIT0_OP` / `UNIT1_OP` | write b30 UPDATE to latch the counter into VALUE regs; hardware sets b29 VALUE_VALID when latched (poll it) |
| 0x0C/0x10, 0x14/0x18 | `UNITn_LOAD_HI/LO` | 20-bit hi + 32-bit lo = 52-bit counter. Write then pulse `UNITn_LOAD` (0x5C/0x60 = 1) |
| 0x1C..0x30 | `TARGETn_HI/LO` (n=0..2) | 52-bit alarm value |
| 0x34/0x38/0x3C | `TARGETn_CONF` | b31 TIMER_UNIT_SEL (which counter), b30 PERIOD_MODE (0 one-shot, 1 periodic), b25:0 PERIOD. Pulse `COMPn_LOAD` (0x50/54/58) to apply the period/target |
| 0x40/0x44, 0x48/0x4C | `UNITn_VALUE_HI/LO` | latched counter |
| 0x64/0x68/0x6C/0x70 | `INT_ENA/RAW/CLR/ST` | bits 2:0 = TARGET2/1/0; **level** interrupts on C3 (`SOC_SYSTIMER_INT_LEVEL 1`); `ST = RAW & ENA`; alarm "miss compensation": if target already in the past the interrupt fires immediately (`SOC_SYSTIMER_ALARM_MISS_COMPENSATE 1`) |
| 0xFC | `DATE` | |

Tick rate: **16 MHz** = XTAL 40 MHz / 2.5 (fixed divider; `systimer_ticks_to_us(t) = t/16`, `IDF:esp_hw_support/port/esp32c3/systimer.c`, `SOC_SYSTIMER_FIXED_DIVIDER 1`). 2 counters, 3 alarms (`SOC_SYSTIMER_COUNTER_NUM 2`, `ALARM_NUM 3`).
Use by IDF (`components/esp_hw_support/include/esp_private/systimer.h`): `SYSTIMER_COUNTER_ESPTIMER=0`, `SYSTIMER_COUNTER_OS_TICK=1`, `SYSTIMER_ALARM_OS_TICK_CORE0=0`, `SYSTIMER_ALARM_ESPTIMER=2`. FreeRTOS tick: alarm 0 on counter 1, **periodic**, period `1_000_000/CONFIG_FREERTOS_HZ` µs-ticks (Arduino defconfig: `CONFIG_FREERTOS_HZ=1000`, `esp32-arduino-lib-builder/configs/defconfig.common`), interrupt source `ETS_SYSTIMER_TARGET0_INTR_SOURCE` (37) (`components/freertos/port_systick.c`). esp_timer: counter 0, alarm 2, one-shot, source 39 (`esp_timer_impl_systimer.c`).
Interrupt-matrix wiring in QEMU: TARGET0/1/2 → sources 37/38/39 (`ETS_SYSTIMER_TARGET0_EDGE_INTR_SOURCE + i`).

### F.2 TIMG0 (`0x6001_F000`) / TIMG1 (`0x6002_0000`)

`T0CONFIG 0x00` (b31 EN, b30 INCREASE, b29 AUTORELOAD, b28:13 DIVIDER default 1, b12 DIVCNT_RST, b10 ALARM_EN, b9 USE_XTAL), `T0LO/HI 0x04/0x08` (54-bit), `T0UPDATE 0x0C` (b31, self-clearing: latch counter), `T0ALARMLO/HI 0x10/0x14`, `T0LOADLO/HI 0x18/0x1C`, `T0LOAD 0x20`, WDT block `0x48–0x64` (E.3), `RTCCALICFG 0x68`, `RTCCALICFG1 0x6C`, `INT_ENA/RAW/ST/CLR 0x70/74/78/7C` (b0 T0, b1 WDT), `RTCCALICFG2 0x80`, `NTIMERS_DATE 0xF8`, `REGCLK 0xFC`. Interrupt sources TG0_T0 32, TG0_WDT 33, TG1_T0 34, TG1_WDT 35. **App-boot dependency: RTC slow-clock calibration** (`rtc_clk_cal_internal`, `IDF:esp_hw_support/port/esp32c3/rtc_time.c`): set `RTCCALICFG` CLK_SEL/MAX, pulse START, wait `esp_rom_delay_us`, then poll `TIMG_RTC_CALI_RDY` (in `RTCCALICFG`) and read the count from `RTCCALICFG1`; also `RTCCALICFG2.TIMEOUT` as an escape. QEMU completes it instantly with `value = xtal_cycles` derived from the nominal slow-clock frequency (150 kHz RC / 8 MHz÷256 / 32.768 kHz). A model must return `RDY=1` and a plausible count or the app spins.

---

## G. Console

### G.1 ROM console selection (from `ROM3:boot_prepare`, `uart_usb_print_status_update@0x4004c39c`, `uart_tx_one_char@0x4004c3ec`)

* Two enable flags in ROM RAM at `0x3fcdffd0` (USB print) and `0x3fcdffd1` (UART print), set once at boot:
  * UART flag := `1` if download mode (strap bit3 = 0), else `(RTC_CNTL.STORE4 bit0 == 0) && ets_is_print_boot()` where `ets_is_print_boot` depends on eFuse `UART_PRINT_CONTROL` (RD_REPEAT_DATA3 b7:6: 0 ⇒ always print, 1 ⇒ print if strap bit2 = 0, 2 ⇒ print if strap bit2 = 1, 3 ⇒ never).
  * USB flag := `1` if (download mode and USB download not disabled) or (flash boot and STORE4 bit0 == 0 and `!ets_efuse_usb_print_is_disabled`), where `usb_print_is_disabled = eFuse DIS_USB_SERIAL_JTAG (RD_REPEAT_DATA0 b11) || DIS_USB_SERIAL_JTAG_ROM_PRINT (RD_REPEAT_DATA3 b2)`.
  * Blank eFuses ⇒ **both channels print**: every character goes to UART0 *and* to USB-Serial-JTAG. IDF confirms: "ROM printf by default also prints to USB-Serial-JTAG on channel 2" (`bootloader_console.c`, USB-Serial-JTAG branch). The ROM printf channel number stored at `0x3fcdf648` is 0 (UART0) because `ets_efuse_get_uart_print_channel` returns 0 (`ROM3@0x40046c36`).
* `ets_write_char_uart@0x400481e2` expands `\n` to `\r\n`.

### G.2 UART0 (ROM and IDF)

`FIFO 0x00` (write = TX byte, read = RX byte), `INT_RAW 0x04`, `INT_ST 0x08`, `INT_ENA 0x0C`, `INT_CLR 0x10`, `CLKDIV 0x14` (b11:0 integer, b23:20 fraction), `STATUS 0x1C` (**b25:16 TXFIFO_CNT**, b9:0 RXFIFO_CNT), `CONF0 0x20`, `CONF1 0x24`, `MEM_CONF 0x60`, `FSM_STATUS 0x6C` (b7:4 `ST_UTX_OUT`), `CLK_CONF 0x78` (b24 TX_SCLK_EN, b25 RX_SCLK_EN, b23 RST_CORE, b21:20 SCLK_SEL), `DATE 0x7C`, `ID 0x80` (`IDF:uart_reg.h`, offsets verified by `REG_UART_BASE(i)+off`).
* ROM TX per char: spin while `(STATUS >> 16) & 0x380 != 0` (i.e. TXFIFO_CNT ≥ 128), then store the byte to `FIFO` (`ROM3:uart_tx_one_char_uart.part.0@0x4004c0be`). `uart_tx_flush`/`uart_tx_wait_idle` spin while `STATUS & 0x03FF0000` (`ROM3:uart_tx_flush@0x4004c46c`), the IDF `esp_rom_output_tx_wait_idle(0)` in `bootloader_clock_configure` uses the same wait ⇒ **TXFIFO_CNT must drop to 0**; the QEMU model drains the FIFO instantly when a chardev is connected, else drops.
* `Uart_Init@0x4004c260` (called with 40 MHz): UART0 pins GPIO20/21 (`IO_MUX +0x54/+0x58`, function field cleared, pull-ups, `gpio_pad_unhold`), `CLKDIV` for 115200 (`uart_div_modify@0x4004c20c`), `CONF0 (0x20)` RMW (sets b26, b18, b17 = `0x04060000`, then clears b18/b17: TX/RX FIFO reset pulse; `ROM3:Uart_Init@0x4004c306–0x4004c318`), `CONF1` from ROM data, `INT_CLR = 0xFFFFFFFF`, `INT_ENA |= 1`, then INTC map/priority (B.3). UART1 gets only `INT_CLR = 1` in `uartAttach@0x4004c0e4`.
* IDF app default: `CONFIG_ESP_CONSOLE_UART_DEFAULT` = UART0 on GPIO20/21, 115200 (`IDF:components/esp_system/Kconfig` choice `ESP_CONSOLE_UART`, default `ESP_CONSOLE_UART_DEFAULT`); the Arduino lib-builder defconfigs for C3 do not override the console (`configs/defconfig.common`, `defconfig.esp32c3` checked: no `ESP_CONSOLE*` line).

### G.3 USB-Serial-JTAG (`0x6004_3000`, `IDF:usb_serial_jtag_reg.h`, `usb_serial_jtag_ll.h`)

| Offset | Register | Fields |
|---|---|---|
| 0x00 | `EP1` | write = enqueue TX byte, read = pop RX byte |
| 0x04 | `EP1_CONF` | b0 `WR_DONE` (write 1 = flush/commit the IN packet), **b1 `SERIAL_IN_EP_DATA_FREE` (RO, reset 1)**, b2 `SERIAL_OUT_EP_DATA_AVAIL` (RO, reset 0) |
| 0x08/0x0C/0x10/0x14 | `INT_RAW/ST/ENA/CLR` | b0 JTAG_IN_FLUSH, b1 SOF, b2 SERIAL_OUT_RECV_PKT, b3 SERIAL_IN_EMPTY (reset 1), b4 PID_ERR … |
| 0x18 | `CONF0` | PHY/pad config |
| 0x20 | `JFIFO_ST` | b2 IN_FIFO_EMPTY (1), b3 IN_FIFO_FULL, b6 OUT_FIFO_EMPTY (1), b7 OUT_FIFO_FULL, b8/b9 resets |
| 0x24 | `FRAM_NUM`; 0x44 `MISC_CONF`; 0x48 `MEM_CONF`; 0x80 `DATE` | |

* ROM TX (`usb_uart_tx_one_char@0x4004c8e2`): keeps a "pending" flag in RAM (`0x3fcdf060`). If no packet is pending: if `EP1_CONF.b1` (data free) is 1 → set pending, write the byte to `EP1`; if 0 → **drop silently** (return 0). If pending: retry up to 904 times with a 1 µs `ets_delay` while `b1` is 0; on timeout clear pending and drop. On `\n`, `usb_uart_tx_flush@0x4004c95a` writes `EP1_CONF = 1` (WR_DONE). ⇒ A model that always reports `b1 = 1` and treats `EP1` writes as console output works; a model that stalls the host side makes the ROM burn ~0.9 ms per character, then drop (this is also what happens on a real board with no host).
* IDF bootloader with USB-Serial-JTAG console: `esp_rom_output_switch_buffer` + `esp_rom_output_set_as_console(ESP_ROM_USB_SERIAL_DEVICE_NUM)`; app side `usb_serial_jtag_ll_write_txfifo` loops on `b1` then `usb_serial_jtag_ll_txfifo_flush` (`WR_DONE`). The Arduino `HWCDC` driver (the `Serial` of `ARDUINO_USB_CDC_ON_BOOT=1`) additionally relies on the interrupt bits above and on SOF to detect a host (**not read in this pass; the driver source was not fetched — unverified**). QEMU has only a stub (all reads 0), i.e. it models **no** USB console and relies on UART0.

**What the reference firmware prints where — unknown, two plausible answers.** `ARDUINO_USB_CDC_ON_BOOT=1` + `-DARDUINO_USB_MODE=1` (APPNOTE) makes `Serial` = HWCDC on USB-Serial-JTAG. The ESP-IDF log/`printf` path follows `CONFIG_ESP_CONSOLE_*`, which is the IDF default (UART0) unless the lib-builder overrides it (no override found). So expect: ROM + bootloader logs on **both** UART0 and USB-Serial-JTAG; app `ESP_LOGx` on UART0; Arduino `Serial.print` on USB-Serial-JTAG. Verify by checking the app binary for `HWCDC` strings and the bootloader build config of the lib-builder.

---

## H. eFuse (`0x6000_8800`)

Layout (`IDF:efuse_reg.h`; the read registers are the already-loaded mirror, there is no per-read command):

| Offset | Register | Fields the boot chain reads |
|---|---|---|
| 0x2C | `RD_WR_DIS` | |
| 0x30 | `RD_REPEAT_DATA0` | b6:0 RD_DIS, b7 DIS_RTC_RAM_BOOT, b8 DIS_ICACHE, b9 DIS_USB_JTAG, b10 DIS_DOWNLOAD_ICACHE, **b11 DIS_USB_SERIAL_JTAG** (ROM: `ets_efuse_usb_device_disabled@0x40046c3e` reads `0x60008830`), b12 DIS_FORCE_DOWNLOAD, b13 DIS_TWAI, b14 JTAG_SEL_ENABLE, b17:15 SOFT_DIS_JTAG, b18 DIS_PAD_JTAG, b19 DIS_DOWNLOAD_MANUAL_ENCRYPT, b21:20/b23:22 USB_DREFH/L, b24 USB_EXCHG_PINS, b25 VDD_SPI_AS_GPIO, b27:26 BTLC_GPIO_ENABLE, b28 POWERGLITCH_EN, b30:29 POWER_GLITCH_DSENSE |
| 0x34 | `RD_REPEAT_DATA1` | ~~b1:0 WDT_DELAY_SEL, **b4:2 SPI_BOOT_CRYPT_CNT** (flash encryption), b5/6/7 SECURE_BOOT_KEY_REVOKE0/1/2, b11:8 KEY_PURPOSE_0, b15:12 KEY_PURPOSE_1~~ b17:16 WDT_DELAY_SEL, **b20:18 SPI_BOOT_CRYPT_CNT** (flash encryption), b21/22/23 SECURE_BOOT_KEY_REVOKE0/1/2, b27:24 KEY_PURPOSE_0, b31:28 KEY_PURPOSE_1; b15:0 reserved (corrected 2026-10-11, plan SP1b-2: see `esp32c3-bootloader-accesses.md` §11 for `WDT_DELAY_SEL` and `SPI_BOOT_CRYPT_CNT`; the three other fields, which moved with them, were corrected against `IDF:soc/esp32c3/register/soc/efuse_reg.h:312-370`) |
| 0x38 | `RD_REPEAT_DATA2` | b3:0 KEY_PURPOSE_2 … b15:12 KEY_PURPOSE_5, ~~**b16 SECURE_BOOT_EN**~~ **b20 SECURE_BOOT_EN** (`ets_efuse_secure_boot_enabled@0x40046d0a` reads `0x60008838`), ~~b17 SECURE_BOOT_AGGRESSIVE_REVOKE~~ b21 SECURE_BOOT_AGGRESSIVE_REVOKE, ~~**b21:18 FLASH_TPUW**~~ **b31:28 FLASH_TPUW** (`ets_efuse_get_flash_delay_us@0x40046d6a`) (corrected 2026-10-11, plan SP1b-2: see `esp32c3-rom-flash-boot.md` §K for `SECURE_BOOT_EN` and `FLASH_TPUW`; `SECURE_BOOT_AGGRESSIVE_REVOKE`, which sits between them, was corrected against `IDF:soc/esp32c3/register/soc/efuse_reg.h:417-424`) |
| 0x3C | `RD_REPEAT_DATA3` | **b0 DIS_DOWNLOAD_MODE**, b1 DIS_DIRECT_BOOT (ROM `ets_run_flash_bootloader` checks `& 2` before the direct-boot jump), **b2 DIS_USB_SERIAL_JTAG_ROM_PRINT**, b3 FLASH_ECC_MODE, **b4 DIS_USB_SERIAL_JTAG_DOWNLOAD_MODE**, **b5 ENABLE_SECURITY_DOWNLOAD**, **b7:6 UART_PRINT_CONTROL**, b8 PIN_POWER_SELECTION, b9 FLASH_TYPE, b11:10 FLASH_PAGE_SIZE, b12 FLASH_ECC_EN, **b13 FORCE_SEND_RESUME**, b29:14 SECURE_VERSION, ~~b30 ERR_RST_ENABLE~~ b31 ERR_RST_ENABLE (corrected 2026-10-11, plan SP1b-2: see `esp32c3-bootloader-accesses.md` §11) |
| 0x40 | `RD_REPEAT_DATA4` | b0 DISABLE_WAFER_VERSION_MAJOR, b1 DISABLE_BLK_VERSION_MAJOR |
| 0x44 | `RD_MAC_SPI_SYS_0` | MAC[31:0] |
| 0x48 | `RD_MAC_SPI_SYS_1` | b15:0 MAC[47:32], then **SPI pad config CLK[21:16], Q[27:22], D[31:28]** (ROM `ets_efuse_get_spiconfig` reads 0x48 and 0x4C) |
| 0x4C | `RD_MAC_SPI_SYS_2` | SPI pad D_1/CS/HD/WP/DQS/D4 |
| 0x50 | `RD_MAC_SPI_SYS_3` | b5:0 D5 pad … **b20:18 WAFER_VERSION_MINOR_LO**, b23:21 PKG_VERSION, **b26:24 BLK_VERSION_MINOR**, b29:27 FLASH_CAP, b31:30 FLASH_TEMP |
| 0x54 | `RD_MAC_SPI_SYS_4` | b2:0 FLASH_VENDOR … |
| 0x58 | `RD_MAC_SPI_SYS_5` | **b23 WAFER_VERSION_MINOR_HI, b25:24 WAFER_VERSION_MAJOR** |
| 0x5C–0x68 | `RD_SYS_PART1_DATA0..3` | OPTIONAL_UNIQUE_ID |
| 0x6C | `RD_SYS_PART1_DATA4` | **b1:0 BLK_VERSION_MAJOR**, temp/ADC calibration fields |
| 0x7C.. / 0x9C.. | USR_DATA0..7 / KEY0..5 | |
| 0x17C / 0x1C0 | `RD_REPEAT_ERR0..4` / `RD_RS_ERR0..1` | coding-error flags; blank = 0 |
| 0x1C8 CLK, 0x1CC CONF, 0x1D0 STATUS, **0x1D4 CMD** (b0 READ_CMD, b1 PGM_CMD, b5:2 BLK_NUM), 0x1D8 INT_RAW, 0x1E8 DAC_CONF, 0x1EC RD_TIM_CONF, 0x1FC DATE | | `efuse_hal_read`: write CONF op-code 0x5A5A, set `READ_CMD`, spin until it clears (twice); `efuse_hal_set_timing` writes DAC/TIM regs — all plain R/W with `CMD` self-clearing |

Chip revision (`IDF:hal/esp32c3/include/hal/efuse_ll.h`, `efuse_hal.c`): `major = RD_MAC_SPI_SYS_5.WAFER_VERSION_MAJOR`; `minor = (SYS_5.WAFER_VERSION_MINOR_HI << 3) | SYS_3.WAFER_VERSION_MINOR_LO`; `efuse_hal_chip_revision() = major*100 + minor`; block revision = `BLK_VERSION_MAJOR*100 + BLK_VERSION_MINOR`. Bootloader prints `chip revision: v%d.%d` and `efuse block revision: v%d.%d` (`bootloader_init.c bootloader_check_bootloader_validity`).
Values used by QEMU for a blank virtual chip: revision **v0.3** (`WAFER_VER_MINOR_LO = 3` @0x50 b20:18), eFuse block revision **v1.3** (`BLK_VERSION_MAJOR = 1` @0x6C, `BLK_VERSION_MINOR = 3` @0x50 b26:24); everything else 0 ⇒ no secure boot, no flash encryption, download modes allowed, both consoles enabled, default SPI pads, no MAC. A v0.4 part needs `WAFER_VERSION_MINOR_LO = 4`. For ROM selection the revision matters (rev3 ROM for ≤ v1.0).
Open point: `esp_efuse_mac_get_default` with an all-zero MAC (blank) — IDF source not located in this pass; the project note says CrossPoint wraps `bootloader_common_check_efuse_blk_validity` because the stock bootloader mis-reads the block revision. **Recommendation:** fill `RD_MAC_SPI_SYS_0/1` with a locally-administered MAC and set `BLK_VERSION` to a value the reference bootloader accepts; verify against the real first-boot log.

Bootloader eFuse checks seen: secure-boot / flash-encryption bits are read in `bootloader_utility.c` ("Checking flash encryption…", "Checking secure boot…") — both are no-ops when the bits are 0.

---

## I. Expected boot log (milestones)

### I.1 ROM (rev3), format strings read from `ROM3 .rodata` and the call sites in `boot_prepare` / `ets_run_flash_bootloader` / `ets_config_flash_by_image_hdr`

```
ESP-ROM:esp32c3-api1-20210207
Build:Feb  7 2021
rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)
SPIWP:0xee
mode:DIO, clock div:1
load:0x3fcd5990,len:0x3b94
load:0x403cc710,len:0xb9c
load:0x403ce710,len:0x5ba8
entry 0x403cc71a
```
(Real example: `IDF:examples/security/security_features_app/README.md`.) Format strings: `ESP-ROM:%s\n`, `Build:%s\n`, `rst:0x%x (%s),boot:0x%x (%s)\n` (printed value = raw `GPIO_STRAP_REG`), `Saved PC:0x%08x\n` (only after WDT-type resets, from `ASSIST_DEBUG +0x48`), `SPIWP:0x%02x\n`, `mode:%s, clock div:%d\n`, `load:0x%08x,len:0x%x\n`, `entry 0x%08x\n`, plus failure strings (`invalid header: 0x%02x%02x%02x%02x`, `Invalid chip id. Expected %u read %u. Bootloader for wrong chip?`, `Checksum failure. Calculated 0x%02x stored 0x%02x`, `SHA-256 comparison failed:`+`Calculated`/`Expected`+`Attempting to boot anyway...` or `Aborting`, `Invalid image block, can't boot.`). All at `ROM3 .rodata 0x3ff1b86c–0x3ff1c14c`.
Details derivable from the ROM code:
* `SPIWP:0xee`: value comes from the **image's extended header byte 0** (`wp_pin`, default 0xEE = "disabled") when the SPI-pad eFuses are blank (`ets_config_flash_by_image_hdr@0x40049b2e`: `lbu s0, 0(s3)`).
* `mode:` is the string indexed by image-header byte 2 (`QIO`, `QOUT`, `DIO`, `DOUT`, …) and `clock div` is `(hdr[3] & 0xF) + 2` for values 0..2, else 1 (hdr[3]&0xF = 0 ⇒ div 2/40 MHz, 1 ⇒ 3/26 MHz, 2 ⇒ 4/20 MHz, 0xF ⇒ 1/80 MHz). The reference image header says DIO @ 80 MHz ⇒ **`mode:DIO, clock div:1`**.
* Header validation (`ets_run_flash_bootloader@0x40049d20`): magic `0xE9`, ~~segment count ≤ 16~~ segment count ≤ 32 in the header gate, then ≤ 16, byte 2 ≤ 10, `hdr[3]&0xF` ≤ 2 or == 0xF; extended-header chip id must be **5** (ESP32-C3); data checksum seed `0xEF` (XOR of all segment words); ~~optional appended SHA-256 (sha accelerator) compared against the digest stored after the checksum~~ the SHA-256 that follows the checksum is always computed (sha accelerator) and compared; a mismatch is not fatal without secure boot, and `hash_appended` is never consulted (corrected 2026-10-11, plan SP1b-2: see `esp32c3-rom-flash-boot.md` §K and §F); each load segment copied word-by-word from the MMU window into SRAM; 4-byte alignment and `len ≤ 0x100000`, and a no-overlap check against the ROM stack/data region.
* RTC STORE4/5, MMU entries and cache as in C/E.

### I.2 2nd-stage bootloader (IDF 5.5.x, log level INFO) — strings from `components/bootloader_support`

```
I (xx) boot: ESP-IDF v5.5.x 2nd stage bootloader
I (xx) boot: compile time <HH:MM:SS>          (only if CONFIG_BOOTLOADER_COMPILE_TIME_DATE)
I (xx) boot: chip revision: v0.4
I (xx) boot: efuse block revision: v1.3
I (xx) boot.esp32c3: SPI Speed      : 80MHz
I (xx) boot.esp32c3: SPI Mode       : DIO
I (xx) boot.esp32c3: SPI Flash Size : 16MB
I (xx) boot: Enabling RNG early entropy source...
I (xx) boot: Partition Table:
I (xx) boot: ## Label            Usage          Type ST Offset   Length
I (xx) boot:  0 nvs              WiFi data        01 02 0000a000 00006000
...
I (xx) boot: End of partition table
I (xx) esp_image: segment 0: paddr=… vaddr=0x3c… size=… ( …) map
...
I (xx) boot: Loaded app from partition at offset 0x10000
I (xx) boot: Disabling RNG early entropy source...
```
Sources: `bootloader_init.c` (`ESP-IDF %s 2nd stage bootloader`, `compile time %s`, `chip revision: v%d.%d`, `efuse block revision: v%d.%d`, `Enabling RNG early entropy source...`), `bootloader_flash_config_esp32c3.c` (`SPI Speed      : %s`, `SPI Mode       : %s`, `SPI Flash Size : %s`), `bootloader_utility.c` (`Partition Table:`, `## Label            Usage          Type ST Offset   Length`, row format `"%2d %-16s %-16s %02x %02x %08x %08x"`, `End of partition table`, `Loaded app from partition at offset 0x%x`, `Checking flash encryption...`/`Checking secure boot...` only when enabled). The first `I (n)` timestamps in a real log are ≈ 10–30 ms (`IDF:docs/en/security/esp32c3_log.inc`). The line `Multicore bootloader` is **not** printed on C3 (single core: `bootloader_print_banner`). `Disabling RNG early entropy source...` is emitted by the loader just before jumping (seen in `examples/security/security_features_app/README.md`).
Console routing: bootloader output goes through `esp_rom_printf` channels ⇒ UART0 and USB-Serial-JTAG (G.1).

### I.3 App milestones (IDF early init, from the app image — not log-verified here)
`I (xx) cpu_start: Unicore app`, `I (xx) cpu_start: Pro cpu start user code`, `I (xx) cpu_start: cpu freq: 160000000 Hz`, `I (xx) app_init: Application information:` …, then `main_task: Calling app_main()`. **Not read from IDF sources in this pass** (listed as expected markers; the exact text depends on log level / Arduino build — treat as unknown until the first real run).

---

## J. First peripheral accesses of the ROM, in order (rev3)

Derived by walking the call graph from `main@0x40047e3c` with constant-propagated `lui/addi` addresses (`trace_main.txt`, `tools/trace.py`). Branch outcomes are assumed for a **power-on, flash-boot, blank-eFuse** run (download-mode and secure-boot paths omitted). R = read, W = write, RMW = read-modify-write. Implementation-order recommendation at the end.

| # | Phase (ROM symbol @ address) | Accesses (address → meaning) |
|---|---|---|
| 0 | `_start@0x40000000 → _init@0x40001e90` | CSR setup (B.1); **W `INTERRUPT.CPU_INT_ENABLE 0x600C2104 = 0`**; CSR 0x800/0x801 = 1; **W `INTERRUPT.CPU_INT_THRESH 0x600C2194 = 1`**; copy `.data` from ROM image to `0x3fccae00…`, zero `.bss`; `j main` |
| 1 | `main` → `rtc_get_reset_reason(0)@0x40048a8e` | **R `RTC_CNTL.RESET_STATE 0x60008038`** (expects 1 on power-on) |
| 2 | `boot_prepare@0x40047c5c` | `memcpy` of the 24-entry reset-reason name table (RAM only) → **RMW `EXTMEM.ICACHE_CTRL1 0x600C4004 |= 3`** (`Cache_Mask_All`) |
| 3 | `rtc_unhold_all_pads@0x40047bea` | **RMW `RTC_CNTL.PWC 0x60008084` (clear b21)**, **RMW `RTC_CNTL.DIG_ISO 0x6000808C` (clear b15, set b10)** |
| 4 | `uartAttach(0)@0x4004c0e4` | ROM RAM struct init; **W `UART0.INT_CLR 0x60000010 = 1`**, **W `UART1.INT_CLR 0x60010010 = 1`**; `ets_isr_mask(0x20)` ⇒ **RMW `INTERRUPT.CPU_INT_ENABLE 0x600C2104` (clear b5)**; `ets_isr_attach` (RAM handler table) |
| 5 | `ets_get_apb_freq@0x40049866` (+ xtal freq/div) | **R `SYSTEM.SYSCLK_CONF 0x600C0058`** (×3: SOC_CLK_SEL, CLK_XTAL_FREQ=40, CLK_DIV_EN/PRE_DIV_CNT) ⇒ APB 40 MHz |
| 6 | `ets_update_cpu_frequency(40)` + store | RAM `g_ticks_per_us=40`; **W `RTC_CNTL.STORE5 0x600080BC = (apb>>12)|(apb>>12)<<16`** |
| 7 | `ets_efuse_get_uart_print_channel` → 0; `Uart_Init(0, 40 MHz)@0x4004c260` | **RMW `SYSTEM.PERIP_CLK_EN0 0x600C0010 |= 4`**; **RMW `SYSTEM.PERIP_RST_EN0 0x600C0018` set b2 then clear b2**; **RMW `IO_MUX 0x60009054`/`0x60009058`** (GPIO20/21: clear function bits [14:12], pulls) via `gpio_pad_pullup`/`gpio_pad_unhold` ⇒ **RMW `RTC_CNTL.PAD_HOLD 0x600080D0`, `DIG_PAD_HOLD 0x600080D4`**; **W `UART0.CLKDIV 0x60000014`**, **RMW `UART0.CONF0 0x60000020`** (FIFO reset pulse), **W `UART0.CONF1 0x60000024`**, **W `UART0.INT_CLR 0x60000010 = 0xFFFFFFFF`**, **RMW `UART0.INT_ENA 0x6000000C |= 1`**, **W `INTERRUPT.UART_INTR_MAP 0x600C2054 = 5`**, **W `INTERRUPT.CPU_INT_PRI_5 0x600C2128`** (0 then 1) |
| 8 | `uart_tx_switch(0)`; `uart_usb_print_status_update@0x4004c39c` | **R `GPIO.STRAP 0x60004038`**, **R `RTC_CNTL.STORE4 0x600080B8`** (bit0), **R `EFUSE.RD_REPEAT_DATA0 0x60008830`** (b11), **R `EFUSE.RD_REPEAT_DATA3 0x6000883C`** (b0, b2, b4, b7:6) ⇒ set the two print-enable flags (G.1) |
| 9 | banner via `ets_printf` | **R `GPIO.STRAP 0x60004038`** (several, text selection); **W `UART0.FIFO 0x60000000` per char** after polling **R `UART0.STATUS 0x6000001C`**; **R `USB_SERIAL_JTAG.EP1_CONF 0x60043004`** + **W `EP1 0x60043000`** per char; **W `EP1_CONF 0x60043004 = 1`** on each `\n` |
| 10 | `assist_debug_clock_enable@0x40047c1a`; rest of `boot_prepare` | **RMW `SYSTEM.CPU_PERI_CLK_EN 0x600C0000 |= 0x40`**, **RMW `SYSTEM.CPU_PERI_RST_EN 0x600C0004 &= ~0x40`**; **R `ASSIST_DEBUG 0x600CE048`** (non-zero ⇒ prints `Saved PC:0x%08x`); **W `ASSIST_DEBUG 0x600CE044 = 3`**; `fatal_handlers_attach` (RAM) |
| 11 | `print_other_reset_reason@0x40047e0a` | **R `RTC_CNTL.SWD_CONF 0x600080AC`** (b0; must be 0), **R `RTC_CNTL.RESET_STATE 0x60008038`** (JTAG flag b20; must be 0), `clear_super_wdt_reset_flag` re-reads `SWD_CONF` |
| 12 | `main`: reset-reason bit-mask test; deep-sleep case only → `rtc_boot_control` | **R `GPIO.STRAP 0x60004038`**; (not on power-on) `STORE6/7`, `SYSTEM.RTC_FASTMEM_CRC/CONFIG 0x600C004C/48` |
| 13 | flash path: `ets_efuse_get_spiconfig@0x40046b3e` | **R `EFUSE.RD_MAC_SPI_SYS_1 0x60008848`, `RD_MAC_SPI_SYS_2 0x6000884C`** ⇒ 0 ⇒ default pads |
| 14 | `spi_flash_boot_attach@0x4004dca8`: `SelectSpiFunction`, `SPI_init`, `esp_rom_spiflash_config_readmode` ~~, `SPI_WakeUp`~~ (no `SPI_WakeUp` and no status read here) (corrected 2026-10-11, plan SP1b-2: see `esp32c3-rom-flash-boot.md` §K) | **RMW `GPIO 0x60004020`** (output-disable), `0x60004024` (matrix out), GPIO matrix in/out regs; **RMW `IO_MUX 0x6000903C/40/44/48`**; **RMW `SYSTEM 0x600C0010/18`** (SPI01 b1 clock + reset pulse); **RMW SPI1 `0x600020A4`, SPI0 `0x60003034`**; **RMW SPI0/SPI1 `0x…10, 0x…18` (CTRL2/USER)**; **W `0x60002014` / `0x60003014` (CLOCK)**, **W `0x60002008` / `0x60003008` (CTRL)**; **RMW SPI0 `0x60003020/24/28/3C/18/1C`** (cache-mode switch); **R SPI0 `0x60003054` (FSM) until `&0x70==0`**, **R SPI1 `0x60002000` (CMD) until `&0xF==0`**; ~~RDSR via user command (SPI1 `USER/USER2/MISO_DLEN/W0/CMD`); **W SPI1 `CMD` (release-from-power-down)** and poll~~ (the status read and the release from power-down are not in the attach: they come later, `SPI_WakeUp` from `ets_run_flash_bootloader` and the two status reads inside `ets_config_flash_by_image_hdr`; corrected 2026-10-11, plan SP1b-2: see `esp32c3-rom-flash-boot.md` §K) |
| 15 | `ets_run_flash_bootloader@0x40049d20` → `ROM_Boot_Cache_Init@0x40050ee6` | ~~**RMW `EXTMEM.AUTOLOAD_CTRL 0x600C4040`** (b0, b1, clear b1); **`Cache_MMU_Init`: W `0x600C5000…` 128 entries ← invalid**; **RMW `EXTMEM.ICACHE_CTRL 0x600C4000 \|= 1`**; **RMW `EXTMEM.AUTOLOAD_CTRL \|= 4`** (via `Cache_Resume_ICache_Autoload`); **RMW `EXTMEM.ICACHE_CTRL1 0x600C4004 &= ~2`**~~ **RMW `SYSTEM.CACHE_CONTROL 0x600C0040`** (set b0, set b1, clear b1); **RMW `EXTMEM.CACHE_MMU_OWNER \|= 0xF`**; **`Cache_MMU_Init`: W `0x600C5000…` 128 entries ← invalid**; **W `EXTMEM.CACHE_ILG_INT_CLR` = `0xFFFFFFFF`**; **RMW `CACHE_WRAP_AROUND_CTRL &= ~1`**; **RMW `SENSITIVE.INTERNAL_SRAM_USAGE_1 &= ~1`**; invalidate-all (`SYNC_CTRL` set b0, poll b1); **RMW `EXTMEM.ICACHE_CTRL 0x600C4000 \|= 1`**; **RMW `EXTMEM.ICACHE_CTRL1 0x600C4004 &= ~2`**. `AUTOLOAD_CTRL` is not written: `Cache_Resume_ICache_Autoload` does nothing for the argument `0` of `Cache_Enable_ICache(0)` (corrected 2026-10-11, plan SP1b-2: see `esp32c3-cache-mmu.md` §0.1, §4.1 and §5.1) |
| 16 | `ets_efuse_get_flash_delay_us`, `SPI_WakeUp`, `ets_efuse_force_send_resume` | **R `EFUSE.RD_REPEAT_DATA2 0x60008838`** (~~b21:18~~ b31:28; corrected 2026-10-11, plan SP1b-2: see `esp32c3-rom-flash-boot.md` §K), **R `EFUSE.RD_REPEAT_DATA3 0x6000883C`** (b13); `SPI_WakeUp` ~~again~~ (its first call: row 14 has none; corrected 2026-10-11, plan SP1b-2: see `esp32c3-rom-flash-boot.md` §K); `esp_rom_spi_flash_send_resume` only if FORCE_SEND_RESUME |
| 17 | `ets_loader_map_range(0, 24)@0x400499f6` | **R `EFUSE.RD_REPEAT_DATA2`** (secure boot bit); `Cache_Suspend_ICache` (**RMW `EXTMEM.CTRL1 \|=3`, `CTRL &=~1`, poll `EXTMEM+0x34 & 2`**), then `Cache_Suspend_ICache_Autoload` (poll `AUTOLOAD_CTRL` b3) before the preload poll, and after it a freeze enable + freeze disable pair on `ICACHE_FREEZE`, each with its `DONE` poll (corrected 2026-10-11, plan SP1b-2: see `esp32c3-cache-mmu.md` §0.1 and §4.1); `Cache_Invalidate_ICache_All` (**`SYNC_CTRL` set b0, poll b1**); **W MMU entry `0x600C5000` = flash page 0** (`Cache_Dbus_MMU_Set`); `Cache_Resume_ICache`; then **R 24 bytes at `0x3C000000`** (image header + extended header) |
| 18 | `ets_efuse_secure_boot_enabled`, `ets_loader_is_direct_boot_jump` | **R `EFUSE.RD_REPEAT_DATA2`, `RD_REPEAT_DATA3` (b1)** |
| 19 | `esp_rom_spi_flash_update_id@0x4004db12` | SPI1: **RMW `USER 0x60002018`**, **W `W0 0x60002058 = 0`**, **W `CMD 0x60002000 = 0x10000000`**, poll `CMD`, **R `W0`** (JEDEC id), **W `0x6000201C`** |
| 20 | `ets_config_flash_by_image_hdr@0x40049b2e` | prints `SPIWP:0x%02x`; if SPI pads from eFuse ≠ 0 uses IO_MUX; `esp_rom_spiflash_config_clk` (CLOCK regs, `Wait_SPI_Idle`), `spi_dummy_len_fix`, `esp_rom_spiflash_config_readmode`, `spi_common_set_dummy_output`; prints `mode:%s, clock div:%d` |
| 21 | `ets_sha_enable@0x400494dc`, `ets_sha_init/update/finish` | **RMW `SYSTEM.PERIP_CLK_EN1 0x600C0014 |= 4`**, **RMW `SYSTEM.PERIP_RST_EN1 0x600C001C &= ~0x34`**; per 64-byte block: **W `SHA.MODE 0x6003B000 = 2`** (SHA-256), **memcpy block → `SHA.TEXT 0x6003B080…`**, **W `SHA.START 0x6003B010 = 1`** (first block) or **`CONTINUE 0x6003B014 = 1`**, **poll `SHA.BUSY 0x6003B018` while == 1**; state read-back from `H 0x6003B040…` (`ets_sha_get_state`) |
| 22 | segment loop (per segment) | `ets_loader_map_range` for the 8-byte segment header and for the data (~~MMU entries rewritten, cache suspend/invalidate/resume each time~~ the mapping changes only when the wanted range leaves the current window: once in total for an image below 64 KiB; corrected 2026-10-11, plan SP1b-2: see `esp32c3-rom-flash-boot.md` §K; up to 1 MiB window at `0x3C000000`), `ets_common_command`/`SPI_Common_Command@0x400001f0` only if the header requests it; `load:0x%08x,len:0x%x`; word copy to SRAM; SHA update; checksum XOR; final `ets_sha_finish`, compare appended SHA-256 |
| 23 | exit | `Cache_Suspend_ICache` (the full sequence of row 17, with `Cache_Suspend_ICache_Autoload` and the freeze pair; corrected 2026-10-11, plan SP1b-2: see `esp32c3-cache-mmu.md` §0.1), `Cache_Invalidate_ICache_All`, `Cache_Resume_ICache`, **`Cache_MMU_Init` (all 128 entries invalid again)**, print `entry 0x%08x`, `jalr` to the entry (`0x403cc71a` in the example) with no argument |

Skipped (only on other paths): `uart_baudrate_detect`, `ets_uart_download`, `spi_slave_download`, `UartConnCheck` (UART/USB/SPI download), secure-boot signature check (`ets_secure_boot_verify_stage_bootloader`), `ets_run_direct_boot`.

**Suggested implementation order** (each step unblocks the next ROM stage):
1. Memory map + ROM image (A) and CPU incl. CSRs 0x000/0x800–0x802/0x3A0–0x3BF/0x7A0–0x7A5, vectored trap with `mcause = 0x80000000|n` (B).
2. INTERRUPT_CORE0 register file (`CPU_INT_ENABLE`, `THRESH`, MAP, PRI) — the very first ROM writes.
3. `GPIO.STRAP` (bit 3 set), `RTC_CNTL` (RESET_STATE, OPTIONS0 software resets, STORE0–7, SWD_CONF read-as-0, PAD_HOLD/PWC/DIG_ISO R/W), `SYSTEM` (CLK_EN/RST_EN R/W, `SYSCLK_CONF` with XTAL=40, `CPU_PER_CONF`), `EFUSE` mirror.
4. UART0 (+ UART1 stub) and USB-Serial-JTAG (EP1 + EP1_CONF, `b1` always 1) → banner appears.
5. IO_MUX/GPIO matrix as R/W storage; SPI1 (user commands: RDID, RDSR, WREN, …) and SPI0 as R/W storage with `CMD` self-clearing, `FSM` reads 0.
6. EXTMEM + MMU table + flash cache windows (C) → bootloader loads via `0x3C000000`.
7. SHA accelerator (mode 2 / SHA-256, START/CONTINUE/BUSY, TEXT/H memories) → image hash check.
8. Then the bootloader: SYSTEM/RTC clock programming (accept regi2c window), TIMG0/1 + RWDT/SWD (storage only), APB_SARADC (storage), RNG (`0x600260B0`, any value).
9. App: SYSTIMER (F), TIMG RTC calibration (F.2), RTC_CNTL timer, intmatrix delivery, USB-Serial-JTAG interrupts, SPI1 write/erase for NVS/OTA (`PP`, `SE`, `BE`, WREN/WRDI, WIP bit in RDSR).

---

## K. Uncertainties and open questions (ranked)

1. **Retail chip revision / ROM revision** of the Xteink X4 (rev3 vs rev101) and its **flash JEDEC id** — not in any source; drives the `chip revision:` line, the ROM ELF, and `bootloader_flash_xmc_startup`.
2. **`mie`/`mip` and interrupt arbitration** on the C3 core (QEMU forces MEIE, picks lowest line; ROM/IDF never touch `mie`). Priority/threshold semantics are solid (IDF + ROM + QEMU agree).
3. **Misaligned access** behaviour (trap vs hardware split; settled, see §M.1) — IDF hints "works but slow"; the spec's RISCOF config says "exception". Needs a decision plus a test against the real firmware.
4. **Hardware WDT auto-arm in flash-boot mode** (timeout and stage action unknown); ROM never touches WDTs, bootloader disables flashboot protection.
5. **Which console the reference firmware uses** after the app starts (UART0 via ESP-IDF defaults vs USB-Serial-JTAG/HWCDC) and the HWCDC driver's expectations (SOF, interrupts) — driver source not read.
6. USB-Serial-JTAG register reset values beyond `EP1_CONF`/`INT_RAW`/`JFIFO_ST` (taken from the header defaults only).
7. RTC_I2C/regi2c window (`0x6000E000…`): ~~the ROM busy-wait condition was inferred from QEMU's `0xffffff` stub, not disassembled~~ settled: the ROM spins while bit 25 of the command register `0x6000E000` is set and reads the data byte in bits [23:16] (corrected 2026-10-11, plan SP1b-2: see `esp32c3-bootloader-accesses.md` §11 and §2.2).
8. App early-boot log lines (I.3) and exact caller of `esp_cpu_configure_region_protection` (PMP locking) — not read in this pass.
9. SPI0 `FSM(0x54)` bits other than "== 0 means idle", cache-refill timing, MWDT/RWDT clocks (150 kHz RC ± calibration).
10. eFuse MAC/blk-version acceptance by the reference bootloader (see H).

## L. Licence notes

* QEMU (GPL): only facts were extracted (addresses, reset values, sequencing, which registers matter); wording here is original. Do not copy QEMU source or comments into inkemu.
* QEMU's bundled `pc-bios/esp32c3-rom.bin` has no stated licence (ADR 0001 already excludes it); this report uses the Apache-2.0 ROM ELFs from `espressif/esp-rom-elfs` for all ROM-side behaviour.
* ESP-IDF headers (Apache-2.0) are the preferred origin for register names, offsets and bit positions in the Rust models (or the already-vendored SVD files).

## M. Addendum, 2026-10-11 — facts established while booting the real ROMs (plan SP1b-1)

These supersede the matching "unknown" entries of sections B.1, G.3 and K.

1. **Misaligned loads and stores are performed by the silicon, not trapped** (settles B.1 "Misaligned loads/stores" and K.3).
   Evidence: the shipped rev0 mask ROM executes `lw t0,0(a5)` at pc `0x40001e00` (in `_init`, the `.data` unpack loop) with the address `0x40058c3e`, which is 2 modulo 4. That address is the first byte after `.wifi0iram.3`, where the linker placed the initial `.data` image. No trap handler able to emulate the access is installed at that point, and rev0 chips boot. With a trapping core the emulated rev0 ROM never reaches its banner; with misaligned accesses performed it prints it with no diagnostic.
   Consequence in inkemu: the ESP32-C3 machine runs with `MisalignedPolicy::Allow`. The generic conformance harness (`inkemu rvtest`, RISCOF/Sail) keeps trapping by default: it tests a generic hart.
   Still unknown: the extra cycles such an access costs on silicon, and what a misaligned access to a peripheral register does (inkemu reports a bus error).
2. **USB-Serial-JTAG sends a full 64-byte IN packet without waiting for `WR_DONE`** (completes G.3).
   Source: ESP-IDF v5.5.2, `components/hal/esp32c3/include/hal/usb_serial_jtag_ll.h`, note on `usb_serial_jtag_ll_txfifo_flush`: "When fifo is full (with 64 byte), HW will flush the buffer automatically". The same note adds that the host treats a 64-byte packet as an incomplete transfer until more data or a zero-length packet follows.
3. **Measured ROM path to the banner** (strict mode, nothing unmodelled, no CPU exception): the banner completes after 25,637 CPU cycles / 17,196 instructions (rev0), 26,681 / 17,932 (rev3), 26,775 / 17,994 (rev101), with the uncalibrated timing model and an UART that transmits instantly. These three counts were measured with the default v0.3 eFuse image for every ROM; the test suite now boots rev0 on a v0.0 chip and rev101 on a v1.1 chip, and the counts were not measured again on those. The first register the rev3 ROM touches after its banner that inkemu does not model is `GPIO.ENABLE` (`0x60004020`) at pc `0x40051b08` (`gpio_output_disable`, flash-boot path, section J row 14).
4. **One register beyond section J rows 0–12 is needed before the banner**: `EXTMEM.ICACHE_CTRL1` (`0x600C4004`), read-modify-written by `Cache_Mask_All` (row 2 lists it; it is plain read/write with reset value 3).
5. **eFuse read mirror**: the SVD documents 91 registers in `0x2C..=0x1C4`; the 12 other offsets of that window (`0x18C`, `0x194`–`0x1BC`) are reserved holes.

## Corrections (2026-10-11)

Established after this report was written, by `esp32c3-interrupts-and-reset.md` (§9) and `esp32c3-timers-and-watchdogs.md` (§K). The text above is left as it was; where it disagrees with an entry below, the entry is right.

- BC §B.1 "Interrupt arbitration (several pending): Unknown": settled by `TRM §1.5.2`, highest priority then lowest line (§1.2). QEMU's lowest-line rule is a divergence.
- BC §B.3: the delivery condition also requires a non-zero priority (§1.2); `CPU_INT_CLEAR` is toggled (1 then 0), not written once; QEMU decodes the priority registers of lines 1 to 30 only (in the current `esp-develop` source), not 1 to 31.
- BC §B.1 "`mie`/`mip` […] recommended model: ignore `mie` for lines 1..31, treat `mie`/`mip` as plain registers": the first half stands; the second is an assumption among two (§2.4). QEMU does **not** ignore `mie`: writing 0 blocks delivery.
- BC §B.1 "WFI […] Whether C3 wakes with `mstatus.MIE=0` is unknown": still unknown for silicon; the two QEMU builds disagree (§2.6).
- BC §E.2 "enabling a 1 s RWDT with flashboot protection": the RWDT is configured with two 1 s stages and `FLASHBOOT_MOD_EN`, but `wdt_hal_init` has cleared its enable bit and nothing sets it again (§4.1). BC §K.4 (hardware flash-boot watchdog timeout): after an `esp_restart` the stage values are these.

| Place | Statement there | Established here |
|---|---|---|
| `esp32c3-boot-chain.md` §E.3, §K.4; `esp32c3-rom-flash-boot.md` §L.2; `esp32c3-bootloader-accesses.md` §10.8 | "Exact hardware timeout/stage action of the auto-armed WDT: unknown" | MWDT0: 26,000,000 APB cycles, core reset, cause `0x07`; RWDT: 400,000 slow-clock cycles, system reset, cause `0x10`; proven absent from the ROM; both seen on real chips (§D.4, §H) |
| `esp32c3-boot-chain.md` §E.2 | RTC timer "ticks at the RTC slow clock (RC ≈ 150 kHz)" | 136 kHz nominal once the bootloader has set `SCK_DCAP` = 255 (`SOC_CLK_RC_SLOW_FREQ_APPROX`) |
| `esp32c3-boot-chain.md` §F.2 | "`RTCCALICFG2.TIMEOUT` as an escape" | also the exit of a first wait that runs when `START_CYCLING` is set, which is the reset state (§B.4 step 1); `RTCCALICFG2` resets to `0xFFFFFF98` |
