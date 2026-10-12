//! `SENSITIVE`: permission management (PMS). The registers split the memory into regions, give
//! each one access rights per privilege level and report violations. Sources: TRM chapter 14,
//! ESP-IDF `sensitive_reg.h`, `esp_memprot.c`; reports CM §2.1, IR §3.2 and §4.2.
//!
//! ESP-IDF programs them at start-up (`esp_mprot_set_prot`, with
//! `CONFIG_ESP_SYSTEM_MEMPROT_FEATURE`, the default) and routes the three violation interrupts to
//! CPU line 26. Nothing here checks an access: the registers are stored. "If CPU is reset, PMS
//! registers will be reset, too" (TRM §6.1.3): a reset of any level returns them to their reset
//! values.
//!
//! `Cache_Occupy_ICache_MEMORY` gives SRAM0 to the cache by clearing bit 0 of
//! `INTERNAL_SRAM_USAGE_1` (CM §2.1). SRAM0 stays plain RAM here: nothing uses it once the cache is
//! on (CM §9 item 9).
use super::{Accepted, SENSITIVE_BASE, Storage};
use crate::ctx::Ctx;
use emu_core::Peripheral;

/// `ROM_TABLE_LOCK` to `CLOCK_GATE`: the 93 consecutive registers of the block (the SVD also has a
/// `DATE` register at `+0xFFC`, which nothing reads).
const REGISTERS: u32 = 93;
const _: () = assert!(
    svd_regs::esp32c3::addr("SENSITIVE", "CLOCK_GATE")
        == SENSITIVE_BASE + svd_regs::REGISTER_BYTES * (REGISTERS - 1)
);

pub(crate) fn accepted() -> Vec<Accepted> {
    vec![
        Accepted::svd(
            "SENSITIVE",
            "ROM_TABLE_LOCK",
            "permission management: no access is checked, no violation is raised",
            "SP1b-4",
        )
        .block(REGISTERS),
    ]
}

#[derive(Clone, Debug)]
pub struct Sensitive {
    registers: Storage,
}

impl Sensitive {
    #[must_use]
    pub fn new() -> Sensitive {
        let offsets: Vec<u32> = (0..REGISTERS)
            .map(|index| svd_regs::REGISTER_BYTES * index)
            .collect();
        Sensitive {
            registers: Storage::for_offsets(SENSITIVE_BASE, &offsets),
        }
    }
}

impl Default for Sensitive {
    fn default() -> Sensitive {
        Sensitive::new()
    }
}

impl Peripheral<Ctx> for Sensitive {
    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
        self.registers.read(offset)
    }

    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        self.registers.write(offset, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg(name: &str) -> u32 {
        svd_regs::esp32c3::addr("SENSITIVE", name) - SENSITIVE_BASE
    }

    #[test]
    fn every_register_of_the_block_is_stored_from_its_reset_value() {
        let ctx = &mut Ctx::default();
        let mut pms = Sensitive::new();
        assert_eq!(
            pms.read(ctx, reg("INTERNAL_SRAM_USAGE_1")),
            Some(0xF),
            "SRAM0 is the cache's at reset (CM §2.1)"
        );
        assert_eq!(pms.read(ctx, reg("CLOCK_GATE")), Some(1));
        // What `esp_mprot_set_split_addr` and `esp_mprot_set_pms_area` write (IR §3.2, trace F1).
        for (name, value) in [
            ("CORE_X_IRAM0_DRAM0_DMA_SPLIT_LINE_CONSTRAIN_1", 0x0016_C03E),
            ("CORE_X_IRAM0_PMS_CONSTRAIN_2", 5),
            ("CORE_0_PIF_PMS_MONITOR_0", 1),
        ] {
            assert!(pms.write(ctx, reg(name), value), "{name}");
            assert_eq!(pms.read(ctx, reg(name)), Some(value), "{name}");
        }
        assert_eq!(
            pms.read(ctx, reg("CLOCK_GATE") + 4),
            None,
            "nothing past the block"
        );
        assert_eq!(pms.read(ctx, reg("DATE")), None);
    }

    #[test]
    fn a_new_block_is_the_reset_state() {
        // What a CPU reset does (TRM §6.1.3).
        let ctx = &mut Ctx::default();
        let mut pms = Sensitive::new();
        pms.write(ctx, reg("INTERNAL_SRAM_USAGE_1"), 0xE);
        pms = Sensitive::new();
        assert_eq!(pms.read(ctx, reg("INTERNAL_SRAM_USAGE_1")), Some(0xF));
    }
}
