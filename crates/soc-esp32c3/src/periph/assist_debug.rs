//! `ASSIST_DEBUG`: the debug assistant. It records the PC at a CPU reset, which the ROM prints as
//! `Saved PC`, and guards the stack pointer for ESP-IDF. Sources: TRM chapter 17, ESP-IDF
//! `IDF:components/soc/esp32c3/register/soc/assist_debug_reg.h`; reports IR §4.4 and §5, TW §D.2
//! and §D.3.
//!
//! The SVD and that header do not name two of the addresses alike: the SVD calls `+0x000`
//! `CORE_0_MONTR_ENA` and `+0x008` `CORE_0_INTR_ENA`, the header calls them
//! `ASSIST_DEBUG_CORE_0_INTR_ENA_REG` (`assist_debug_reg.h:14`) and
//! `ASSIST_DEBUG_CORE_0_INTR_RLS_REG` (`assist_debug_reg.h:162`). The model follows the SVD names.
use super::{ASSIST_DEBUG_BASE, Accepted, Storage};
use crate::ctx::Ctx;
use emu_core::Peripheral;
use svd_regs::esp32c3::addr;

const fn reg(name: &str) -> u32 {
    addr("ASSIST_DEBUG", name) - ASSIST_DEBUG_BASE
}

/// The stack guard of ESP-IDF: the application enables the monitor and its interrupt
/// (`esp_hw_stack_guard_init`, values `0x300`), then rewrites the two bounds at every interrupt
/// entry and exit and at every context switch (report IR §3.3, §5). The stack pointer is not
/// compared with them: a guard that compares must use the bounds exactly, at every instruction.
const WHY_STACK_GUARD: &str = "stack guard: the stack pointer is not compared with the bounds";
const ACCEPTED: [Accepted; 5] = [
    Accepted::svd(
        "ASSIST_DEBUG",
        "CORE_0_MONTR_ENA",
        WHY_STACK_GUARD,
        "SP1b-4",
    ),
    Accepted::svd("ASSIST_DEBUG", "CORE_0_INTR_ENA", WHY_STACK_GUARD, "SP1b-4"),
    Accepted::svd("ASSIST_DEBUG", "CORE_0_INTR_CLR", WHY_STACK_GUARD, "SP1b-4"),
    Accepted::svd("ASSIST_DEBUG", "CORE_0_SP_MIN", WHY_STACK_GUARD, "SP1b-4"),
    Accepted::svd("ASSIST_DEBUG", "CORE_0_SP_MAX", WHY_STACK_GUARD, "SP1b-4"),
];

pub(crate) fn accepted() -> Vec<Accepted> {
    ACCEPTED.to_vec()
}

const CORE_0_INTR_RAW: u32 = reg("CORE_0_INTR_RAW");
const CORE_0_RCD_EN: u32 = reg("CORE_0_RCD_EN");
const CORE_0_RCD_PDEBUGPC: u32 = reg("CORE_0_RCD_PDEBUGPC");
const CORE_0_RCD_PDEBUGSP: u32 = reg("CORE_0_RCD_PDEBUGSP");
const DEBUG_MODE: u32 = reg("C0RE_0_DEBUG_MODE");
/// `CORE_0_RCD_EN`: bit 0 `RCD_RECORDEN`, bit 1 `RCD_PDEBUGEN`. With both set the recorder follows
/// the PC; the ROM writes 3 at every boot (`ROM3:boot_prepare@0x40047dee`; report IR §4.4), and so
/// does the bootloader (`wdt_reset_cpu0_info_enable`).
const RECORDING: u32 = 0b11;

#[derive(Clone, Debug)]
pub struct AssistDebug {
    record_enable: u32,
    /// The PC and the stack pointer at the last CPU or core reset that found the recorder on.
    recorded_pc: u32,
    recorded_sp: u32,
    stack_guard: Storage,
}

impl AssistDebug {
    #[must_use]
    pub fn new() -> AssistDebug {
        AssistDebug {
            record_enable: 0,
            recorded_pc: 0,
            recorded_sp: 0,
            stack_guard: Storage::accepted(ASSIST_DEBUG_BASE, &ACCEPTED),
        }
    }

    /// The CPU is reset while it was about to execute the instruction at `pc`, with the stack
    /// pointer `sp`: "when the CPU resets, `RCD_EN` will reset, while `RCD_PDEBUGPC` will not.
    /// Therefore, the latter will keep the PC value at the CPU reset" (TRM §17.4.2).
    ///
    /// ASSUMPTION: the PC recorded is that of the instruction the reset kept from executing. For a
    /// watchdog this is what real chips print (`Saved PC:0x40047ed2` is the `j .` the ROM hangs
    /// in: TW §D.3). For a software reset the model resets right after the store to
    /// `RTC_CNTL.OPTIONS0`, so the PC is that of the next instruction, in the ROM; logs of real
    /// chips show an application address, which means some instructions run before the reset
    /// takes effect. How many is not established (IR §4.4, §7 question 3). A real chip's
    /// `Saved PC` after `esp_restart`, compared with the address of the store, settles it.
    pub(crate) fn cpu_reset(&mut self, pc: u32, sp: u32) {
        if self.record_enable & RECORDING == RECORDING {
            self.recorded_pc = pc;
            self.recorded_sp = sp;
        }
        self.record_enable = 0;
    }

    /// A core reset returns the other registers to their reset values; the record stays: a real
    /// chip prints `Saved PC` after a `TG0WDT_SYS_RST` (TW §D.2, §H.1).
    pub(crate) fn core_reset(&mut self) {
        self.stack_guard = Storage::accepted(ASSIST_DEBUG_BASE, &ACCEPTED);
    }

    /// A system reset clears the record: a real chip prints no `Saved PC` after a
    /// `RTCWDT_RTC_RST` (TW §D.2, §H.1).
    pub(crate) fn system_reset(&mut self) {
        self.recorded_pc = 0;
        self.recorded_sp = 0;
    }
}

impl Default for AssistDebug {
    fn default() -> AssistDebug {
        AssistDebug::new()
    }
}

impl Peripheral<Ctx> for AssistDebug {
    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
        match offset {
            CORE_0_RCD_EN => Some(self.record_enable),
            CORE_0_RCD_PDEBUGPC => Some(self.recorded_pc),
            CORE_0_RCD_PDEBUGSP => Some(self.recorded_sp),
            // The stack guard never fires and no debugger is attached (BA §2.2).
            CORE_0_INTR_RAW | DEBUG_MODE => Some(0),
            _ => self.stack_guard.read(offset),
        }
    }

    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        match offset {
            CORE_0_RCD_EN => {
                self.record_enable = value & RECORDING;
                true
            }
            _ => self.stack_guard.write(offset, value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    #[test]
    fn no_pc_was_recorded_before_a_power_on_reset() {
        let ctx = &mut Ctx::default();
        let mut dbg = AssistDebug::new();
        assert_eq!(
            dbg.read(ctx, CORE_0_RCD_PDEBUGPC),
            Some(0),
            "the ROM prints `Saved PC` only when non-zero"
        );
        assert_eq!(dbg.read(ctx, CORE_0_RCD_PDEBUGSP), Some(0));
    }

    #[test]
    fn no_debugger_is_attached() {
        // BA §2.2: bit 0 set would make the bootloader skip the checksum and hash verification.
        let ctx = &mut Ctx::default();
        let mut dbg = AssistDebug::new();
        assert_eq!(dbg.read(ctx, DEBUG_MODE), Some(0));
        assert!(!dbg.write(ctx, DEBUG_MODE, 1), "read-only");
    }

    #[test]
    fn a_cpu_reset_records_the_pc_when_the_recorder_is_on_and_switches_it_off() {
        let ctx = &mut Ctx::default();
        let mut dbg = AssistDebug::new();
        dbg.cpu_reset(0x4000_1111, 0x3FC8_0000);
        assert_eq!(
            dbg.read(ctx, CORE_0_RCD_PDEBUGPC),
            Some(0),
            "the recorder was off"
        );
        assert!(dbg.write(ctx, CORE_0_RCD_EN, 3));
        assert_eq!(dbg.read(ctx, CORE_0_RCD_EN), Some(3));
        dbg.cpu_reset(0x4004_8b92, 0x3FC9_0000);
        assert_eq!(dbg.read(ctx, CORE_0_RCD_PDEBUGPC), Some(0x4004_8b92));
        assert_eq!(dbg.read(ctx, CORE_0_RCD_PDEBUGSP), Some(0x3FC9_0000));
        assert_eq!(
            dbg.read(ctx, CORE_0_RCD_EN),
            Some(0),
            "RCD_EN resets with the CPU"
        );
        // One of the two bits is not enough.
        dbg.write(ctx, CORE_0_RCD_EN, 1);
        dbg.cpu_reset(0x4200_0000, 0);
        assert_eq!(dbg.read(ctx, CORE_0_RCD_PDEBUGPC), Some(0x4004_8b92));
        assert!(
            !dbg.write(ctx, CORE_0_RCD_PDEBUGPC, 1),
            "the record is read-only"
        );
    }

    #[test]
    fn the_record_survives_a_core_reset_and_not_a_system_reset() {
        // TW §H.1: `Saved PC` follows `rst:0x7`, not `rst:0x10`.
        let ctx = &mut Ctx::default();
        let mut dbg = AssistDebug::new();
        dbg.write(ctx, CORE_0_RCD_EN, 3);
        dbg.write(ctx, reg("CORE_0_SP_MIN"), 0x3FC8_DBCC);
        dbg.cpu_reset(0x4004_D1F8, 0);
        dbg.core_reset();
        assert_eq!(dbg.read(ctx, CORE_0_RCD_PDEBUGPC), Some(0x4004_D1F8));
        assert_eq!(
            dbg.read(ctx, reg("CORE_0_SP_MIN")),
            Some(0),
            "back to its reset value"
        );
        dbg.system_reset();
        assert_eq!(dbg.read(ctx, CORE_0_RCD_PDEBUGPC), Some(0));
    }

    #[test]
    fn the_stack_guard_registers_are_stored_and_the_guard_never_fires() {
        // IR §3.3: MONTR_ENA <- 0, SP_MIN, SP_MAX, MONTR_ENA <- 0x300 at every context switch.
        let ctx = &mut Ctx::default();
        let mut dbg = AssistDebug::new();
        assert_eq!(
            dbg.read(ctx, reg("CORE_0_SP_MAX")),
            Some(0xFFFF_FFFF),
            "SVD reset value"
        );
        for (name, value) in [
            ("CORE_0_INTR_ENA", 0x300),
            ("CORE_0_MONTR_ENA", 0x300),
            ("CORE_0_SP_MIN", 0x3FC8_DBCC),
            ("CORE_0_SP_MAX", 0x3FC8_EBC0),
        ] {
            assert!(dbg.write(ctx, reg(name), value), "{name}");
            assert_eq!(dbg.read(ctx, reg(name)), Some(value), "{name}");
        }
        assert!(dbg.write(ctx, reg("CORE_0_INTR_CLR"), 0x300));
        assert_eq!(dbg.read(ctx, CORE_0_INTR_RAW), Some(0));
        assert_eq!(
            dbg.read(ctx, reg("CORE_0_AREA_PC")),
            None,
            "the rest is unmodelled"
        );
    }
}
