//! `SYSTEM`: clock selection, peripheral clock gates and resets, software interrupts. Sources:
//! reports BC §E.1, IR §4.1 and §5 (ESP-IDF `system_reg.h`).
//! The CPU clock selection is read by `crate::clock`; the clock gates are stored; a peripheral
//! reset bit resets its module; the four software interrupt registers are interrupt sources.
use crate::ctx::{Ctx, Event, Module, Source};
use crate::periph::{Accepted, SYSTEM_BASE, Storage, consecutive, index_in};
use emu_core::Peripheral;

const SYSCLK_CONF: u32 = svd_regs::esp32c3::addr("SYSTEM", "SYSCLK_CONF") - SYSTEM_BASE;
const CPU_PER_CONF: u32 = svd_regs::esp32c3::addr("SYSTEM", "CPU_PER_CONF") - SYSTEM_BASE;
const PERIP_RST_EN0: u32 = svd_regs::esp32c3::addr("SYSTEM", "PERIP_RST_EN0") - SYSTEM_BASE;
const PERIP_RST_EN1: u32 = svd_regs::esp32c3::addr("SYSTEM", "PERIP_RST_EN1") - SYSTEM_BASE;
/// The modules `PERIP_RST_EN0` resets, by bit (`IDF:components/soc/esp32c3/register/soc/system_reg.h`:
/// `SYSTEM_SPI01_RST` line 497, `SYSTEM_UART_RST` 491, `SYSTEM_UART1_RST` 479,
/// `SYSTEM_TIMERGROUP_RST` 431, `SYSTEM_TIMERGROUP1_RST` 419, `SYSTEM_SYSTIMER_RST` 335). ESP-IDF
/// resets a module by setting its bit and clearing it again:
/// `esp_system_reset_modules_on_exit`, which `esp_restart_noos` calls, sets `0x20000007` then
/// writes 0 (`IDF:components/esp_system/port/soc/esp32c3/system_internal.c:52-54`; report IR
/// §4.1); the timer groups do it through `_timer_ll_reset_register`
/// (`IDF:components/hal/esp32c3/include/hal/timer_ll.h:58-63`) and a UART through
/// `uart_ll_reset_register` (`IDF:components/hal/esp32c3/include/hal/uart_ll.h:109-124`).
///
/// ASSUMPTION: bit 0, `SYSTEM_TIMERS_RST` (`system_reg.h:503`), resets nothing here.
/// `esp_system_reset_modules_on_exit` pulses it, and what it resets is not documented: the timer
/// groups have their own bits (IR §4.3, §7 question 1; TW §F.1). Bits 13 and 15 reset the timer
/// groups (`SYSTEM_TIMERGROUP_RST`, `SYSTEM_TIMERGROUP1_RST`). A bit of no module listed here is
/// stored and resets nothing; one of them belongs to a module the model has and is not acted on:
/// bit 28, `SYSTEM_APB_SARADC_RST` (`system_reg.h:341`).
///
/// Bit 23, `SYSTEM_USB_DEVICE_RST` (`system_reg.h:371`), resets USB-Serial-JTAG
/// (`usb_serial_jtag_ll_reset_register`, `ll.h:296-311`; report USJ §A), which the startup of
/// ESP-IDF leaves alone (`IDF:esp_system/port/soc/esp32c3/clk.c:289`).
///
/// ASSUMPTION: the pulse resets the block as a core reset does and the host does not see it: it
/// stays attached, with its frames and the packets it is sending (report USJ §R.1). A board whose
/// firmware pulses the bit with a host attached, read from both sides, settles it.
///
/// ASSUMPTION: a module is reset when its bit goes from 0 to 1 and is not held in reset while the
/// bit stays set. Report IR §8 item 16 says a module is in reset while its bit is 1: on silicon a
/// module whose bit is still set (the SHA accelerator at power-on, `PERIP_RST_EN1` resets to
/// `0x1FE`) would not answer, and writes made during a hold would not stick. Writing a register of
/// a module while its reset bit is 1, on a chip, settles it.
const RESETS_0: [(u32, Module); 7] = [
    (1 << 1, Module::Spi01),
    (1 << 2, Module::Uart0),
    (1 << 5, Module::Uart1),
    (1 << 13, Module::Timg0),
    (1 << 15, Module::Timg1),
    (1 << 23, Module::UsbDevice),
    (1 << 29, Module::Systimer),
];
/// `PERIP_RST_EN1` bit 2, `SYSTEM_CRYPTO_SHA_RST` (`system_reg.h:553`): the only crypto module
/// modelled. `esp_system_reset_modules_on_exit` pulses `0x7E` (`system_internal.c:58-60`; report
/// IR §4.1).
const RESETS_1: [(u32, Module); 1] = [(1 << 2, Module::Sha)];
/// `CPU_INTR_FROM_CPU_0..3`: bit 0 of each register is the level of an interrupt source, set and
/// cleared by software (`esp_crosscore_int_send` stores 1 at `APP@0x40381bc6`, its handler stores
/// 0 at `APP@0x40381b50`: report IR §3.3, §5).
///
/// ASSUMPTION: registers 1 to 3 behave as register 0 does, each with its own source (51 to 53).
/// Only register 0 is written on the measured paths; the SVD names the four alike, with a map
/// register each, and the three others are modelled by that analogy. Setting bit 0 of
/// `CPU_INTR_FROM_CPU_1` on a chip, with its source routed to an enabled line, and seeing the
/// interrupt, settles it.
const CPU_INTR_FROM_CPU_0: u32 =
    svd_regs::esp32c3::addr("SYSTEM", "CPU_INTR_FROM_CPU_0") - SYSTEM_BASE;
const SOFTWARE_INTERRUPTS: usize = 4;
// The four registers follow one another: `software_interrupt` indexes them by their offset.
const _: () = assert!(consecutive(
    "SYSTEM",
    &[
        "CPU_INTR_FROM_CPU_0",
        "CPU_INTR_FROM_CPU_1",
        "CPU_INTR_FROM_CPU_2",
        "CPU_INTR_FROM_CPU_3",
    ]
));
const SOFTWARE_SOURCES: [Source; SOFTWARE_INTERRUPTS] = [
    Source::of("CPU_INTR_FROM_CPU_0_MAP"),
    Source::of("CPU_INTR_FROM_CPU_1_MAP"),
    Source::of("CPU_INTR_FROM_CPU_2_MAP"),
    Source::of("CPU_INTR_FROM_CPU_3_MAP"),
];
const SOFTWARE_INTERRUPT_LEVEL: u32 = 1;

/// `CPU_PERI_CLK_EN`, `CPU_PERI_RST_EN`, `PERIP_CLK_EN0/1`: clock gates and the reset of the CPU
/// peripherals. Nothing acts on them: a gated module keeps answering. `CPU_PER_CONF` is stored as
/// software wrote it; only its `CPUPERIOD_SEL` field acts, through `crate::clock`. `MEM_PD_MASK`
/// is read by `rtc_init`. The two `BT_LPCK_DIV` registers are written by `esp_perip_clk_init`
/// (`IDF:components/esp_system/port/soc/esp32c3/clk.c:321-323`).
const ACCEPTED: [Accepted; 9] = [
    Accepted::svd(
        "SYSTEM",
        "CPU_PERI_CLK_EN",
        "clock gates of the CPU peripherals: a gated module keeps answering",
        "SP8",
    ),
    Accepted::svd(
        "SYSTEM",
        "CPU_PERI_RST_EN",
        "resets of the CPU peripherals (the debug assistant)",
        "SP1b-4",
    ),
    Accepted::svd(
        "SYSTEM",
        "PERIP_CLK_EN0",
        "peripheral clock gates: a gated module keeps answering",
        "SP8",
    ),
    Accepted::svd(
        "SYSTEM",
        "PERIP_CLK_EN1",
        "peripheral clock gates: a gated module keeps answering",
        "SP8",
    ),
    Accepted::svd(
        "SYSTEM",
        "BT_LPCK_DIV_INT",
        "Bluetooth low-power clock divider: there is no radio",
        "SP5b",
    ),
    Accepted::svd(
        "SYSTEM",
        "BT_LPCK_DIV_FRAC",
        "Bluetooth low-power clock source: there is no radio",
        "SP5b",
    ),
    Accepted::svd(
        "SYSTEM",
        "CACHE_CONTROL",
        "cache clock enable and reset",
        "SP1b-4",
    ),
    Accepted::svd(
        "SYSTEM",
        "CPU_PER_CONF",
        "PLL_FREQ_SEL and the CPU wait-mode bits (CPUPERIOD_SEL is used by the clock model)",
        "SP8",
    ),
    Accepted::svd(
        "SYSTEM",
        "MEM_PD_MASK",
        "memory power-down mask in light sleep",
        "SP1b-3f",
    ),
];

pub(crate) fn accepted() -> Vec<Accepted> {
    ACCEPTED.to_vec()
}

/// `SYSCLK_CONF`: bits [9:0] `PRE_DIV_CNT` and [11:10] `SOC_CLK_SEL` are writable; a write keeps
/// bits [11:0] only, because `CLK_XTAL_FREQ` [18:12] and `CLK_DIV_EN` [19] are read-only (report §E.1).
const SYSCLK_WRITABLE: u32 = 0xFFF;
/// Reset: `PRE_DIV_CNT` = 1, `SOC_CLK_SEL` = 0 (the crystal drives the CPU).
const SYSCLK_RESET: u32 = 1;
/// `CLK_XTAL_FREQ`, bits [18:12], read-only: the ROM derives its UART divider from it
/// (`ROM3:ets_get_apb_freq@0x40049866`). Its value is the crystal of the clock tree, in MHz.
const XTAL_FREQ_SHIFT: u32 = 12;
const XTAL_FREQ_MHZ: u32 = crate::clock::XTAL_MHZ;

/// The CRC unit of the RTC fast memory (F3): `RTC_FASTMEM_CONFIG` bit 31 `RTC_MEM_CRC_FINISH`
/// (read-only), bits [30:20] `RTC_MEM_CRC_LEN`, bits [19:9] `RTC_MEM_CRC_ADDR`, bit 8
/// `RTC_MEM_CRC_START`; `RTC_FASTMEM_CRC` holds the result (`system_reg.h:734-766`). ESP-IDF's
/// `rtc_deep_sleep_start` writes `LEN` = 0x7FF and `ADDR` = 0, sets `START`, waits for `FINISH`,
/// clears `START` and keeps the result in `RTC_CNTL.STORE7`; the ROM does the same at the wake to
/// check the memory before it calls the wake stub (`ROM3:cacl_rtc_memory_crc@0x40048ab2`; report
/// DS §A.1.5, §A.3.2).
const RTC_FASTMEM_CONFIG: u32 =
    svd_regs::esp32c3::addr("SYSTEM", "RTC_FASTMEM_CONFIG") - SYSTEM_BASE;
const RTC_FASTMEM_CRC: u32 = svd_regs::esp32c3::addr("SYSTEM", "RTC_FASTMEM_CRC") - SYSTEM_BASE;
const CRC_FINISH: u32 = 1 << 31;
const CRC_LEN_SHIFT: u32 = 20;
const CRC_ADDR_SHIFT: u32 = 9;
const CRC_FIELD_MASK: u32 = 0x7FF;
const CRC_START: u32 = 1 << 8;
const CRC_CONFIG_STORED: u32 = 0x7FFF_FF00;
/// The RTC fast memory holds this many 32-bit words (8 KiB, TRM §9.6): the range of a CRC never
/// goes past it, whatever `ADDR` and `LEN` say.
const RTC_MEMORY_WORDS: usize = crate::memmap::RTC_RAM_SIZE as usize / 4;

#[derive(Clone, Debug)]
pub struct System {
    sysclk_writable: u32,
    registers: Storage,
    /// `PERIP_RST_EN0` and `PERIP_RST_EN1` as software reads them.
    resets: [u32; 2],
    /// `CPU_INTR_FROM_CPU_0..3`.
    software_interrupts: [u32; SOFTWARE_INTERRUPTS],
    /// `RTC_FASTMEM_CONFIG` without its `FINISH` bit.
    crc_config: u32,
    /// Whether the CRC asked for by the last `START` is done.
    crc_finished: bool,
    /// `RTC_FASTMEM_CRC`.
    crc_result: u32,
}

impl System {
    #[must_use]
    pub fn new() -> System {
        System {
            sysclk_writable: SYSCLK_RESET,
            registers: Storage::accepted(SYSTEM_BASE, &ACCEPTED),
            resets: [reset_value(PERIP_RST_EN0), reset_value(PERIP_RST_EN1)],
            software_interrupts: [0; SOFTWARE_INTERRUPTS],
            crc_config: reset_value(RTC_FASTMEM_CONFIG) & CRC_CONFIG_STORED,
            crc_finished: false,
            crc_result: 0,
        }
    }

    /// The CRC unit computes the CRC `START` asked for, over `memory`, the RTC fast memory: the
    /// bus calls it when the signal of the write that set `START` is delivered, so `FINISH` reads
    /// 1 from the next instruction on.
    ///
    /// ASSUMPTION: the unit takes no time, counts `ADDR` and `LEN` in 32-bit words, covers the
    /// words `ADDR` to `ADDR + LEN` (so the 0x7FF that ESP-IDF and the ROM write covers the 8 KiB),
    /// and computes CRC-32/ISO-HDLC (the reflected polynomial 0xEDB88320, initial value and final
    /// XOR 0xFFFFFFFF) over their bytes in memory order. No source gives the polynomial, the unit
    /// of the fields or the duration (DS §A, open question 1); the ROM and ESP-IDF compare a result
    /// of the unit with another result of the unit, so any function of the same bytes keeps the
    /// wake-stub check of the ROM right (F3). Writing a known pattern to the RTC fast memory of a
    /// real chip, running the unit over a few words and comparing `RTC_FASTMEM_CRC` with the
    /// candidates settles it.
    pub(crate) fn finish_rtc_memory_crc(&mut self, memory: &[u8]) {
        let field = |shift: u32| (self.crc_config >> shift) & CRC_FIELD_MASK;
        // Each field is 11 bits: the arithmetic stays far inside `usize`, and the range is
        // clamped to the memory, so a firmware cannot make the unit read past it.
        let first = (field(CRC_ADDR_SHIFT) as usize).min(RTC_MEMORY_WORDS);
        let end = (first + field(CRC_LEN_SHIFT) as usize + 1).min(RTC_MEMORY_WORDS);
        let bytes = memory.get(4 * first..4 * end).unwrap_or_default();
        self.crc_result = crc32_iso_hdlc(bytes);
        self.crc_finished = true;
    }

    /// `SYSCLK_CONF` and `CPU_PER_CONF` as software reads them: the inputs of [`crate::clock::cpu_frequency`].
    #[must_use]
    pub fn clock_registers(&self) -> (u32, u32) {
        (
            self.sysclk_writable | (XTAL_FREQ_MHZ << XTAL_FREQ_SHIFT),
            // INVARIANT: `CPU_PER_CONF` is in `ACCEPTED`, which `registers` is built from, so the
            // read always answers and the fallback is never taken.
            self.registers.read(CPU_PER_CONF).unwrap_or(0),
        )
    }
}

fn reset_value(offset: u32) -> u32 {
    svd_regs::esp32c3::lookup(SYSTEM_BASE + offset).map_or(0, |r| r.reset_value)
}

/// CRC-32/ISO-HDLC, bit by bit: the CRC unit reads at most 8 KiB, twice per sleep, so a table
/// would buy nothing.
fn crc32_iso_hdlc(bytes: &[u8]) -> u32 {
    const POLYNOMIAL: u32 = 0xEDB8_8320;
    !bytes.iter().fold(u32::MAX, |crc, &byte| {
        (0..8).fold(crc ^ u32::from(byte), |crc, _| {
            if crc & 1 == 0 {
                crc >> 1
            } else {
                (crc >> 1) ^ POLYNOMIAL
            }
        })
    })
}

/// Index of `offset` among the four software interrupt registers.
fn software_interrupt(offset: u32) -> Option<usize> {
    index_in(offset, CPU_INTR_FROM_CPU_0, SOFTWARE_INTERRUPTS)
}

impl Default for System {
    fn default() -> Self {
        System::new()
    }
}

impl Peripheral<Ctx> for System {
    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
        if let Some(index) = software_interrupt(offset) {
            return Some(self.software_interrupts[index]);
        }
        match offset {
            SYSCLK_CONF => Some(self.clock_registers().0),
            PERIP_RST_EN0 => Some(self.resets[0]),
            PERIP_RST_EN1 => Some(self.resets[1]),
            RTC_FASTMEM_CONFIG => {
                Some(self.crc_config | if self.crc_finished { CRC_FINISH } else { 0 })
            }
            RTC_FASTMEM_CRC => Some(self.crc_result),
            _ => self.registers.read(offset),
        }
    }

    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        if let Some(index) = software_interrupt(offset) {
            self.software_interrupts[index] = value & SOFTWARE_INTERRUPT_LEVEL;
            ctx.irq(
                SOFTWARE_SOURCES[index],
                value & SOFTWARE_INTERRUPT_LEVEL != 0,
            );
            return true;
        }
        match offset {
            PERIP_RST_EN0 | PERIP_RST_EN1 => {
                let (register, modules): (usize, &[(u32, Module)]) = if offset == PERIP_RST_EN0 {
                    (0, &RESETS_0)
                } else {
                    (1, &RESETS_1)
                };
                // A module is reset when its bit goes from 0 to 1.
                let rising = value & !self.resets[register];
                self.resets[register] = value;
                for (bit, module) in modules {
                    if rising & bit != 0 {
                        ctx.signal(Event::ModuleReset(*module));
                    }
                }
                true
            }
            SYSCLK_CONF => {
                self.sysclk_writable = value & SYSCLK_WRITABLE;
                ctx.clock_written();
                true
            }
            CPU_PER_CONF => {
                ctx.clock_written();
                self.registers.write(offset, value)
            }
            RTC_FASTMEM_CONFIG => {
                // ASSUMPTION: `FINISH` reads 1 from the write that sets `START` (the CRC is done
                // before the next instruction) until a write clears `START`: both programs wait
                // for it after setting `START`, then clear `START` (DS §A.1.5, §A.3.2). A real
                // chip read in a loop across the two writes settles it.
                let starting = value & CRC_START != 0 && self.crc_config & CRC_START == 0;
                self.crc_config = value & CRC_CONFIG_STORED;
                if value & CRC_START == 0 {
                    self.crc_finished = false;
                }
                if starting {
                    self.crc_finished = false;
                    ctx.signal(Event::RtcMemoryCrc);
                }
                true
            }
            // Read-only: the write is reported as unmodelled.
            RTC_FASTMEM_CRC => false,
            _ => self.registers.write(offset, value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    #[test]
    fn the_crc_is_crc_32_iso_hdlc() {
        // The check value of the catalogue of parametrised CRCs for CRC-32/ISO-HDLC.
        assert_eq!(crc32_iso_hdlc(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32_iso_hdlc(b""), 0);
    }

    #[test]
    fn the_crc_unit_starts_on_the_rising_start_bit_and_finishes_over_the_words_asked() {
        // What `rtc_deep_sleep_start` writes (DS §A.1.5): LEN 0x7FF, ADDR 0, then START.
        let ctx = &mut Ctx::default();
        let mut sys = System::new();
        assert_eq!(
            sys.read(ctx, RTC_FASTMEM_CONFIG),
            Some(0x7FF0_0000),
            "SVD reset"
        );
        assert!(sys.write(ctx, RTC_FASTMEM_CONFIG, 0x7FF << 20));
        assert_eq!(ctx.take_signal(), None);
        assert!(sys.write(ctx, RTC_FASTMEM_CONFIG, 0x7FF << 20 | CRC_START));
        assert_eq!(ctx.take_signal(), Some(Event::RtcMemoryCrc));
        assert_eq!(
            sys.read(ctx, RTC_FASTMEM_CONFIG).map(|v| v & CRC_FINISH),
            Some(0),
            "until the bus answers"
        );
        let mut memory = vec![0u8; RTC_MEMORY_WORDS * 4];
        memory[..9].copy_from_slice(b"123456789");
        sys.finish_rtc_memory_crc(&memory);
        assert_eq!(
            sys.read(ctx, RTC_FASTMEM_CONFIG),
            Some(0x7FF0_0000 | CRC_START | CRC_FINISH)
        );
        assert_eq!(
            sys.read(ctx, RTC_FASTMEM_CRC),
            Some(crc32_iso_hdlc(&memory))
        );
        // Setting START again while it is set starts nothing; clearing it clears FINISH and keeps
        // the result.
        assert!(sys.write(ctx, RTC_FASTMEM_CONFIG, 0x7FF << 20 | CRC_START));
        assert_eq!(ctx.take_signal(), None);
        assert!(sys.write(ctx, RTC_FASTMEM_CONFIG, 0x7FF << 20));
        assert_eq!(sys.read(ctx, RTC_FASTMEM_CONFIG), Some(0x7FF0_0000));
        assert_eq!(
            sys.read(ctx, RTC_FASTMEM_CRC),
            Some(crc32_iso_hdlc(&memory))
        );
        assert!(!sys.write(ctx, RTC_FASTMEM_CRC, 0), "read-only");
    }

    #[test]
    fn the_crc_unit_covers_addr_to_addr_plus_len_and_never_reads_past_the_memory() {
        let mut memory = vec![0u8; RTC_MEMORY_WORDS * 4];
        memory[8..16].copy_from_slice(b"abcdefgh");
        let ctx = &mut Ctx::default();
        let mut sys = System::new();
        // Words 2 and 3: ADDR 2, LEN 1.
        sys.write(ctx, RTC_FASTMEM_CONFIG, 1 << 20 | 2 << 9 | CRC_START);
        sys.finish_rtc_memory_crc(&memory);
        assert_eq!(
            sys.read(ctx, RTC_FASTMEM_CRC),
            Some(crc32_iso_hdlc(b"abcdefgh"))
        );
        // ADDR 0x7FF and LEN 0x7FF ask for 2 048 words from the last one: only the last is read.
        sys.write(ctx, RTC_FASTMEM_CONFIG, 0);
        sys.write(
            ctx,
            RTC_FASTMEM_CONFIG,
            0x7FF << 20 | 0x7FF << 9 | CRC_START,
        );
        memory[RTC_MEMORY_WORDS * 4 - 4..].copy_from_slice(b"last");
        sys.finish_rtc_memory_crc(&memory);
        assert_eq!(
            sys.read(ctx, RTC_FASTMEM_CRC),
            Some(crc32_iso_hdlc(b"last"))
        );
        // A memory shorter than the RTC memory (none is) gives the CRC of what there is.
        sys.finish_rtc_memory_crc(&memory[..4]);
        assert_eq!(sys.read(ctx, RTC_FASTMEM_CRC), Some(0));
    }

    #[test]
    fn sysclk_conf_reports_a_40_mhz_crystal_selected_undivided() {
        let ctx = &mut Ctx::default();
        let mut sys = System::new();
        let conf = sys.read(ctx, 0x58).unwrap();
        assert_eq!((conf >> 12) & 0x7F, 40, "CLK_XTAL_FREQ");
        assert_eq!((conf >> 10) & 3, 0, "SOC_CLK_SEL = XTAL");
        assert_eq!(conf & 0x3FF, 1, "PRE_DIV_CNT");
    }

    #[test]
    fn cache_control_is_storage_from_its_reset_value() {
        // CM §2.1: reset 5; ROM_Boot_Cache_Init does |=1, |=2, &=~2.
        let ctx = &mut Ctx::default();
        let mut sys = System::new();
        let offset = svd_regs::esp32c3::addr("SYSTEM", "CACHE_CONTROL") - SYSTEM_BASE;
        assert_eq!(sys.read(ctx, offset), Some(5));
        assert!(sys.write(ctx, offset, 7));
        assert_eq!(sys.read(ctx, offset), Some(7));
    }

    #[test]
    fn crystal_frequency_is_read_only() {
        let ctx = &mut Ctx::default();
        let mut sys = System::new();
        assert!(sys.write(ctx, 0x58, 0xFFFF_FFFF));
        let conf = sys.read(ctx, 0x58).unwrap();
        assert_eq!((conf >> 12) & 0x7F, 40);
        assert_eq!(conf & 0xFFF, 0xFFF, "divider and clock select are writable");
        assert_eq!(conf >> 19, 0, "CLK_DIV_EN and above stay clear");
    }

    #[test]
    fn clock_gates_and_resets_are_storage_from_their_reset_values() {
        let ctx = &mut Ctx::default();
        let mut sys = System::new();
        for offset in [0x00, 0x04, 0x10, 0x14] {
            let reset = svd_regs::esp32c3::lookup(0x600C_0000 + offset)
                .unwrap()
                .reset_value;
            assert_eq!(sys.read(ctx, offset), Some(reset), "{offset:#x}");
            assert!(sys.write(ctx, offset, reset | 4));
            assert_eq!(sys.read(ctx, offset), Some(reset | 4));
        }
        assert_eq!(sys.read(ctx, 0x08), Some(0xC), "CPU_PER_CONF reset");
    }

    #[test]
    fn clock_writes_are_flagged_once_and_the_registers_read_as_software_sees_them() {
        let ctx = &mut Ctx::default();
        let mut sys = System::new();
        assert_eq!(sys.clock_registers(), (0x2_8001, 0xC), "reset: BA §5");
        assert!(!ctx.take_clock_written());
        let cpu_per_conf = svd_regs::esp32c3::addr("SYSTEM", "CPU_PER_CONF") - SYSTEM_BASE;
        assert!(sys.write(ctx, cpu_per_conf, 0xD));
        assert!(ctx.take_clock_written());
        assert!(!ctx.take_clock_written(), "taken once");
        sys.write(ctx, SYSCLK_CONF, 0x400);
        assert!(ctx.take_clock_written());
        assert_eq!(sys.clock_registers(), (0x2_8400, 0xD));
        sys.write(
            ctx,
            svd_regs::esp32c3::addr("SYSTEM", "PERIP_CLK_EN0") - SYSTEM_BASE,
            0,
        );
        assert!(
            !ctx.take_clock_written(),
            "a clock gate is not the CPU clock"
        );
    }

    #[test]
    fn a_software_interrupt_register_is_the_level_of_its_source() {
        // IR §3.3: source 50 is asserted by a store of 1 and cleared by a store of 0.
        let ctx = &mut Ctx::default();
        let mut sys = System::new();
        assert_eq!(sys.read(ctx, CPU_INTR_FROM_CPU_0), Some(0));
        assert!(sys.write(ctx, CPU_INTR_FROM_CPU_0, 1));
        assert_eq!(sys.read(ctx, CPU_INTR_FROM_CPU_0), Some(1));
        assert_eq!(ctx.sources(), 1 << 50);
        assert!(sys.write(ctx, CPU_INTR_FROM_CPU_0 + 12, 0xFFFF_FFFF));
        assert_eq!(sys.read(ctx, CPU_INTR_FROM_CPU_0 + 12), Some(1), "one bit");
        assert_eq!(ctx.sources(), (1 << 50) | (1 << 53));
        sys.write(ctx, CPU_INTR_FROM_CPU_0, 0);
        assert_eq!(ctx.sources(), 1 << 53);
    }

    #[test]
    fn a_peripheral_reset_bit_resets_its_module_when_it_is_set() {
        // IR §4.1: `esp_system_reset_modules_on_exit` writes 0x20000007 then 0, then 0x7E then 0.
        let ctx = &mut Ctx::default();
        let mut sys = System::new();
        let reset0 = sys.read(ctx, PERIP_RST_EN0).unwrap();
        assert!(sys.write(ctx, PERIP_RST_EN0, reset0 | 0x2000_0007));
        assert_eq!(sys.read(ctx, PERIP_RST_EN0), Some(reset0 | 0x2000_0007));
        let mut signalled = Vec::new();
        while let Some(event) = ctx.take_signal() {
            signalled.push(event);
        }
        assert_eq!(
            signalled,
            [
                Event::ModuleReset(Module::Spi01),
                Event::ModuleReset(Module::Uart0),
                Event::ModuleReset(Module::Systimer)
            ]
        );
        sys.write(ctx, PERIP_RST_EN0, reset0 | 0x2000_0007);
        sys.write(ctx, PERIP_RST_EN0, reset0);
        assert_eq!(ctx.take_signal(), None, "held, then released: no new reset");
        sys.write(
            ctx,
            PERIP_RST_EN0,
            reset0 | (1 << 13) | (1 << 15) | (1 << 5) | (1 << 23),
        );
        // `PERIP_RST_EN1` holds the crypto modules in reset at power-on (reset value 0x1FE): the
        // bootloader released the SHA accelerator to use it.
        sys.write(ctx, PERIP_RST_EN1, 0);
        sys.write(ctx, PERIP_RST_EN1, 0x7E);
        let mut signalled = Vec::new();
        while let Some(event) = ctx.take_signal() {
            signalled.push(event);
        }
        assert_eq!(
            signalled,
            [
                Event::ModuleReset(Module::Uart1),
                Event::ModuleReset(Module::Timg0),
                Event::ModuleReset(Module::Timg1),
                Event::ModuleReset(Module::UsbDevice),
                Event::ModuleReset(Module::Sha)
            ]
        );
    }
}
