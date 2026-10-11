//! `ASSIST_DEBUG`: the PC recorder the ROM consults to print `Saved PC` after a watchdog reset.
//! Source: report §J, line 10. Recording a PC at reset arrives with the watchdogs (SP1b-3).
use emu_core::Peripheral;

const CORE_0_RCD_EN: u32 = 0x44;
const CORE_0_RCD_PDEBUGPC: u32 = 0x48;
const CORE_0_RCD_PDEBUGSP: u32 = 0x4C;

#[derive(Clone, Debug, Default)]
pub struct AssistDebug {
    record_enable: u32,
}

impl AssistDebug {
    #[must_use]
    pub const fn new() -> AssistDebug {
        AssistDebug { record_enable: 0 }
    }
}

impl Peripheral for AssistDebug {
    fn read(&mut self, offset: u32) -> Option<u32> {
        match offset {
            CORE_0_RCD_EN => Some(self.record_enable),
            // Nothing was recorded: this is a power-on reset.
            CORE_0_RCD_PDEBUGPC | CORE_0_RCD_PDEBUGSP => Some(0),
            _ => None,
        }
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        let is_record_enable = offset == CORE_0_RCD_EN;
        if is_record_enable {
            self.record_enable = value;
        }
        is_record_enable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    #[test]
    fn no_pc_was_recorded_before_a_power_on_reset() {
        let mut dbg = AssistDebug::new();
        assert_eq!(
            dbg.read(0x48),
            Some(0),
            "CORE_0_RCD_PDEBUGPC: the ROM prints `Saved PC` only when non-zero"
        );
        assert_eq!(dbg.read(0x4C), Some(0), "CORE_0_RCD_PDEBUGSP");
    }

    #[test]
    fn record_enable_is_remembered_and_the_rest_is_unmodeled() {
        let mut dbg = AssistDebug::new();
        assert!(dbg.write(0x44, 3));
        assert_eq!(dbg.read(0x44), Some(3));
        assert_eq!(dbg.read(0x00), None);
        assert!(!dbg.write(0x48, 1), "the recorded PC is read-only");
    }
}
