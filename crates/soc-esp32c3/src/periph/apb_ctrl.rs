//! `APB_CTRL` (`SYSCON`): the hardware random number generator and two radio memory power registers.
//! Sources: report BA §2 row 88, §2.3 item 1, §3 rows 29 and 34.
use super::{APB_CTRL_BASE, Accepted, Storage};
use crate::config::DEFAULT_RNG_SEED;
use crate::ctx::Ctx;
use emu_core::Peripheral;
use svd_regs::esp32c3::addr;

/// `RNG_DATA_REG` = `SYSCON_RND_DATA_REG` = `WDEV_RND_REG`, `0x600260B0`; the SVD lists it as `RNG.DATA`.
const RND_DATA: u32 = addr("RNG", "DATA") - APB_CTRL_BASE;
/// Multiplier of the xorshift64* generator (Vigna, "An experimental exploration of Marsaglia's
/// xorshift generators, scrambled", 2016).
const XORSHIFT_STAR: u64 = 0x2545_F491_4F6C_DD1D;

const ACCEPTED: [Accepted; 2] = [
    Accepted::svd(
        "APB_CTRL",
        "FRONT_END_MEM_PD",
        "radio front-end memory power",
        "SP5a",
    ),
    Accepted::svd("APB_CTRL", "MEM_POWER_UP", "radio memory power", "SP5a"),
];

pub(crate) fn accepted() -> Vec<Accepted> {
    ACCEPTED.to_vec()
}

/// The `APB_CTRL` block as far as the boot uses it: the random number register, which answers the
/// next value of a seeded sequence on every read, and the two radio memory power registers, which
/// are storage.
#[derive(Clone, Debug)]
pub struct ApbCtrl {
    /// State of the generator; never zero.
    rng: u64,
    power: Storage,
}

impl ApbCtrl {
    /// Decision D8: the random number generator is a deterministic sequence set by `seed`, so two
    /// runs are identical and a session can be replayed. It advances on every read.
    #[must_use]
    pub fn new(seed: u64) -> ApbCtrl {
        ApbCtrl {
            // A xorshift state of 0 stays 0.
            rng: if seed == 0 { DEFAULT_RNG_SEED } else { seed },
            power: Storage::accepted(APB_CTRL_BASE, &ACCEPTED),
        }
    }

    /// A core reset returns the registers to their reset values. The generator is no register: its
    /// sequence goes on, so a boot after a reset does not replay the numbers of the one before.
    pub(crate) fn reset(&mut self) {
        self.power = Storage::accepted(APB_CTRL_BASE, &ACCEPTED);
    }

    fn next_random(&mut self) -> u32 {
        let mut state = self.rng;
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        self.rng = state;
        let [a, b, c, d, ..] = state.wrapping_mul(XORSHIFT_STAR).to_be_bytes();
        u32::from_be_bytes([a, b, c, d])
    }
}

impl Peripheral<Ctx> for ApbCtrl {
    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
        if offset == RND_DATA {
            return Some(self.next_random());
        }
        self.power.read(offset)
    }

    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        // `RND_DATA` is read-only: it is deliberately not handled here, so a write to it is refused
        // and the bus reports it as unmodelled instead of pretending the generator was seeded.
        self.power.write(offset, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_RNG_SEED;

    #[test]
    fn the_generator_is_deterministic_and_never_repeats_a_value_on_the_boot_path() {
        let ctx = &mut Ctx::default();
        let mut first = ApbCtrl::new(DEFAULT_RNG_SEED);
        // Pinned: a recorded session replays only if the sequence never changes.
        assert_eq!(first.read(ctx, RND_DATA), Some(0x2B0E_D4C7));
        assert_eq!(first.read(ctx, RND_DATA), Some(0xC0B1_3AB1));
        assert_eq!(first.read(ctx, RND_DATA), Some(0x73D5_9E4D));
        let mut second = ApbCtrl::new(DEFAULT_RNG_SEED);
        let mut third = ApbCtrl::new(DEFAULT_RNG_SEED ^ 1);
        // 1,604 reads in the measured bootloader (BA §2 row 88).
        let a: Vec<u32> = (0..1604)
            .map(|_| second.read(ctx, RND_DATA).unwrap())
            .collect();
        let b: Vec<u32> = (0..1604)
            .map(|_| third.read(ctx, RND_DATA).unwrap())
            .collect();
        assert_eq!(a[..3], [0x2B0E_D4C7, 0xC0B1_3AB1, 0x73D5_9E4D]);
        assert_ne!(a, b, "the seed matters");
        let mut sorted = a.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), a.len(), "no constant, no short cycle");
    }

    #[test]
    fn an_even_number_of_reads_does_not_cancel_out() {
        // BA §2.3 item 1: each word is the XOR of many reads and the bootloader loops while it is 0.
        let ctx = &mut Ctx::default();
        let mut apb = ApbCtrl::new(DEFAULT_RNG_SEED);
        for reads in [2usize, 802, 1604] {
            let word = (0..reads).fold(0, |acc, _| acc ^ apb.read(ctx, RND_DATA).unwrap());
            assert_ne!(word, 0, "{reads} reads");
        }
    }

    #[test]
    fn a_zero_seed_does_not_lock_the_generator() {
        let ctx = &mut Ctx::default();
        let mut apb = ApbCtrl::new(0);
        assert_ne!(apb.read(ctx, RND_DATA), apb.read(ctx, RND_DATA));
    }

    #[test]
    fn the_generator_is_read_only_and_the_two_power_registers_are_stored() {
        let ctx = &mut Ctx::default();
        let mut apb = ApbCtrl::new(DEFAULT_RNG_SEED);
        assert!(!apb.write(ctx, RND_DATA, 0));
        for (name, reset) in [("FRONT_END_MEM_PD", 0x15), ("MEM_POWER_UP", 0x3F)] {
            let offset = svd_regs::esp32c3::addr("APB_CTRL", name) - APB_CTRL_BASE;
            assert_eq!(apb.read(ctx, offset), Some(reset), "{name}");
            assert!(apb.write(ctx, offset, 1));
            assert_eq!(apb.read(ctx, offset), Some(1));
        }
        let wifi_clk_en = svd_regs::esp32c3::addr("APB_CTRL", "WIFI_CLK_EN") - APB_CTRL_BASE;
        assert_eq!(
            apb.read(ctx, wifi_clk_en),
            None,
            "WIFI_CLK_EN belongs to a later plan"
        );
    }
}
