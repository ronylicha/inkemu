//! ESP32-C3 address space. Every value comes from `docs/research/esp32c3-boot-chain.md`, §A.1,
//! which cites the ESP-IDF header named next to each constant, and from `esp32c3-cache-mmu.md`
//! §1 for the flash windows.

/// Mask ROM code, I-bus (ESP-IDF `soc.h` `SOC_IROM_MASK_LOW`).
pub const ROM_BASE: u32 = 0x4000_0000;
/// 384 KiB (`SOC_IROM_MASK_HIGH - SOC_IROM_MASK_LOW`).
pub const ROM_SIZE: u32 = 0x6_0000;

/// Mask ROM data, D-bus (`SOC_DROM_MASK_LOW`).
pub const DROM_BASE: u32 = 0x3FF0_0000;
/// 128 KiB (`SOC_DROM_MASK_HIGH - SOC_DROM_MASK_LOW`).
pub const DROM_SIZE: u32 = 0x2_0000;
/// The D-bus window is an alias of the ROM at this offset: the ROM ELF maps `.rodata` VMA
/// `0x3ff19c00` to LMA `0x40059c00`, i.e. VMA + `0x4004_0000` - `0x3ff0_0000` (report §A.1).
pub const DROM_ROM_OFFSET: u32 = 0x4_0000;

/// SRAM, D-bus view (`SOC_DRAM_LOW`).
pub const SRAM_DBUS_BASE: u32 = 0x3FC8_0000;
/// SRAM, I-bus view of the same cells (`SOC_DIRAM_IRAM_LOW`).
pub const SRAM_IBUS_BASE: u32 = 0x4038_0000;
/// 384 KiB (`SOC_DRAM_HIGH - SOC_DRAM_LOW`).
pub const SRAM_SIZE: u32 = 0x6_0000;

/// SRAM0, I-bus only (`SOC_IRAM_LOW`).
pub const SRAM0_BASE: u32 = 0x4037_C000;
/// 16 KiB (`SOC_CACHE_MEMORY_IBANK_SIZE`).
pub const SRAM0_SIZE: u32 = 0x4000;

/// RTC memory (`SOC_RTC_IRAM_LOW`, `SOC_RTC_DRAM_LOW`).
pub const RTC_RAM_BASE: u32 = 0x5000_0000;
/// 8 KiB (`SOC_RTC_*_HIGH - SOC_RTC_*_LOW`).
pub const RTC_RAM_SIZE: u32 = 0x2000;

/// Peripheral window (`SOC_PERIPHERAL_LOW`).
pub const PERIPH_BASE: u32 = 0x6000_0000;
/// 1 MiB (`SOC_PERIPHERAL_HIGH - SOC_PERIPHERAL_LOW`).
pub const PERIPH_SIZE: u32 = 0x10_0000;

/// Flash cache window, data bus (`SOC_DRAM0_CACHE_ADDRESS_LOW`; report CM §1).
pub const FLASH_DBUS_BASE: u32 = 0x3C00_0000;
/// Flash cache window, instruction bus (`SOC_IRAM0_CACHE_ADDRESS_LOW`). The two windows share the
/// 128 MMU entries: entry `i` maps `base + i * 64 KiB` of both.
pub const FLASH_IBUS_BASE: u32 = 0x4200_0000;
/// 8 MiB each (128 pages of 64 KiB).
pub const FLASH_WINDOW_SIZE: u32 = 0x80_0000;

/// The CPU starts executing at the first byte of the mask ROM (report §A.1, "reset PC").
pub const RESET_PC: u32 = ROM_BASE;
