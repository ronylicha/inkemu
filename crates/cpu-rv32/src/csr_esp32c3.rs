//! CSRs specific to the ESP32-C3 core. Source: `docs/research/esp32c3-boot-chain.md`, §B.2.
use emu_core::Cycles;

/// User status. The ROM writes 0 then 1 in `_init`; nothing reads it back.
pub const USTATUS: u16 = 0x000;
/// Performance counter: event select, mode and count, machine addresses (ESP-IDF `rv_utils.h`).
pub const PCER_MACHINE: u16 = 0x7E0;
pub const PCMR_MACHINE: u16 = 0x7E1;
pub const PCCR_MACHINE: u16 = 0x7E2;
/// The same three registers at their user addresses, the ones the ROM uses.
pub const PCER_USER: u16 = 0x800;
pub const PCMR_USER: u16 = 0x801;
pub const PCCR_USER: u16 = 0x802;

/// `ustatus` bits of the RISC-V N extension draft.
/// ASSUMPTION: the silicon keeps only these two; a readback on a real chip would settle it.
pub const USTATUS_UIE: u32 = 1 << 0;
pub const USTATUS_UPIE: u32 = 1 << 4;

/// Event selection: CPU clock cycles (the value the ROM and ESP-IDF write).
pub const PCER_CYCLES: u32 = 1 << 0;
/// Event selection: retired instructions.
/// ASSUMPTION: bit position taken from the ESP32-C3 technical reference manual, not exercised by the ROM.
pub const PCER_INSTRUCTIONS: u32 = 1 << 1;
/// Mode register: global count enable.
pub const PCMR_ENABLE: u32 = 1 << 0;

#[derive(Clone, Debug, Default)]
pub(crate) struct Esp32C3Csrs {
    ustatus: u32,
    pcer: u32,
    pcmr: u32,
    /// Counter value at the snapshot below.
    count: u32,
    snapshot_cycles: u64,
    snapshot_instret: u64,
}

#[allow(clippy::cast_possible_truncation)] // deliberate: the counter is 32 bits wide and wraps
const fn low(v: u64) -> u32 {
    v as u32
}

impl Esp32C3Csrs {
    fn counted(&self, cycles: Cycles, instret: u64) -> u32 {
        if self.pcmr & PCMR_ENABLE == 0 {
            return self.count;
        }
        // Other event selections (hazards, branches, loads...) are not modelled: the counter then stands still.
        let elapsed = if self.pcer & PCER_CYCLES != 0 {
            cycles.get().wrapping_sub(self.snapshot_cycles)
        } else if self.pcer & PCER_INSTRUCTIONS != 0 {
            instret.wrapping_sub(self.snapshot_instret)
        } else {
            0
        };
        self.count.wrapping_add(low(elapsed))
    }

    /// Freezes the value counted so far, so a configuration change only affects what follows.
    fn resnapshot(&mut self, cycles: Cycles, instret: u64) {
        self.count = self.counted(cycles, instret);
        self.snapshot_cycles = cycles.get();
        self.snapshot_instret = instret;
    }

    pub(crate) fn read(&self, addr: u16, cycles: Cycles, instret: u64) -> Option<u32> {
        Some(match addr {
            USTATUS => self.ustatus,
            PCER_MACHINE | PCER_USER => self.pcer,
            PCMR_MACHINE | PCMR_USER => self.pcmr,
            PCCR_MACHINE | PCCR_USER => self.counted(cycles, instret),
            _ => return None,
        })
    }

    pub(crate) fn write(&mut self, addr: u16, value: u32, cycles: Cycles, instret: u64) -> bool {
        match addr {
            USTATUS => self.ustatus = value & (USTATUS_UIE | USTATUS_UPIE),
            PCER_MACHINE | PCER_USER => {
                self.resnapshot(cycles, instret);
                self.pcer = value;
            }
            PCMR_MACHINE | PCMR_USER => {
                self.resnapshot(cycles, instret);
                self.pcmr = value;
            }
            PCCR_MACHINE | PCCR_USER => {
                self.resnapshot(cycles, instret);
                self.count = value;
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Cycles;

    #[test]
    fn counter_stays_frozen_until_it_is_enabled() {
        let c = Esp32C3Csrs::default();
        assert_eq!(c.read(PCCR_USER, Cycles(500), 40), Some(0));
    }

    #[test]
    fn counter_counts_cpu_cycles_once_the_cycle_event_and_the_enable_bit_are_set() {
        let mut c = Esp32C3Csrs::default();
        assert!(c.write(PCER_USER, 1, Cycles(100), 10));
        assert!(c.write(PCMR_USER, 1, Cycles(110), 11));
        assert_eq!(c.read(PCCR_USER, Cycles(110), 11), Some(0));
        assert_eq!(c.read(PCCR_USER, Cycles(150), 20), Some(40));
    }

    #[test]
    fn machine_and_user_addresses_are_the_same_registers() {
        let mut c = Esp32C3Csrs::default();
        assert!(c.write(PCER_MACHINE, 1, Cycles(0), 0));
        assert!(c.write(PCMR_MACHINE, 1, Cycles(0), 0));
        assert_eq!(c.read(PCER_USER, Cycles(0), 0), Some(1));
        assert_eq!(c.read(PCMR_USER, Cycles(0), 0), Some(1));
        assert_eq!(
            c.read(PCCR_MACHINE, Cycles(7), 0),
            c.read(PCCR_USER, Cycles(7), 0)
        );
    }

    #[test]
    fn writing_the_counter_sets_its_value_and_counting_resumes_from_there() {
        let mut c = Esp32C3Csrs::default();
        c.write(PCER_USER, 1, Cycles(0), 0);
        c.write(PCMR_USER, 1, Cycles(0), 0);
        assert!(c.write(PCCR_USER, 1000, Cycles(50), 5));
        assert_eq!(c.read(PCCR_USER, Cycles(60), 6), Some(1010));
    }

    #[test]
    fn disabling_freezes_the_value_reached() {
        let mut c = Esp32C3Csrs::default();
        c.write(PCER_USER, 1, Cycles(0), 0);
        c.write(PCMR_USER, 1, Cycles(0), 0);
        c.write(PCMR_USER, 0, Cycles(30), 3);
        assert_eq!(c.read(PCCR_USER, Cycles(900), 90), Some(30));
    }

    #[test]
    fn instruction_event_counts_retired_instructions() {
        let mut c = Esp32C3Csrs::default();
        c.write(PCER_USER, PCER_INSTRUCTIONS, Cycles(0), 0);
        c.write(PCMR_USER, 1, Cycles(0), 0);
        assert_eq!(c.read(PCCR_USER, Cycles(400), 25), Some(25));
    }

    #[test]
    fn counter_wraps_at_32_bits() {
        let mut c = Esp32C3Csrs::default();
        c.write(PCER_USER, 1, Cycles(0), 0);
        c.write(PCMR_USER, 1, Cycles(0), 0);
        c.write(PCCR_USER, u32::MAX, Cycles(0), 0);
        assert_eq!(c.read(PCCR_USER, Cycles(2), 0), Some(1));
    }

    #[test]
    fn ustatus_keeps_only_its_two_defined_bits() {
        let mut c = Esp32C3Csrs::default();
        assert!(c.write(USTATUS, 0xffff_ffff, Cycles(0), 0));
        assert_eq!(
            c.read(USTATUS, Cycles(0), 0),
            Some(USTATUS_UIE | USTATUS_UPIE)
        );
    }

    #[test]
    fn other_addresses_are_not_vendor_csrs() {
        let mut c = Esp32C3Csrs::default();
        assert_eq!(c.read(0x7E3, Cycles(0), 0), None);
        assert!(!c.write(0x803, 1, Cycles(0), 0));
    }
}
