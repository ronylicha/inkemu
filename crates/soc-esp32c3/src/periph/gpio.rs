//! GPIO block. The strapping register and the registers the ROM's flash-boot path reads back are
//! modelled; pins arrive with the board wiring (SP2).
use super::{Accepted, GPIO_BASE, Storage};
use emu_core::Peripheral;

/// `GPIO_STRAP_REG`: the strapping pins sampled at reset. Bit 3 set selects SPI flash boot (report §D.3).
const STRAP: u32 = svd_regs::esp32c3::addr("GPIO", "STRAP") - GPIO_BASE;

/// What the ROM's flash-boot path reads back and modifies (FB §B.1): the output enables and the two
/// input selectors of the flash pads. Pins themselves arrive with the board wiring (SP2).
const ACCEPTED: [Accepted; 3] = [
    Accepted::svd("GPIO", "ENABLE", "pin output enable", "SP2"),
    Accepted::svd(
        "GPIO",
        "FUNC0_IN_SEL_CFG",
        "input signal routing (GPIO matrix)",
        "SP2",
    ),
    Accepted::svd(
        "GPIO",
        "FUNC1_IN_SEL_CFG",
        "input signal routing (GPIO matrix)",
        "SP2",
    ),
];

pub(crate) fn accepted() -> Vec<Accepted> {
    ACCEPTED.to_vec()
}

#[derive(Clone, Debug)]
pub struct Gpio {
    strapping: u32,
    config: Storage,
}

impl Gpio {
    #[must_use]
    pub fn new(strapping: u32) -> Gpio {
        Gpio {
            strapping,
            config: Storage::accepted(GPIO_BASE, &ACCEPTED),
        }
    }
}

impl Peripheral for Gpio {
    fn read(&mut self, offset: u32) -> Option<u32> {
        if offset == STRAP {
            Some(self.strapping)
        } else {
            self.config.read(offset)
        }
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        // `STRAP` is not in the list: it stays read-only.
        self.config.write(offset, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    #[test]
    fn output_enable_and_the_two_input_selectors_are_stored() {
        // FB §B.1: gpio_output_disable clears bits of ENABLE; gpio_bypass_matrix_in clears bit 6 of FUNCn_IN_SEL_CFG.
        let mut gpio = Gpio::new(0x8);
        for name in ["ENABLE", "FUNC0_IN_SEL_CFG", "FUNC1_IN_SEL_CFG"] {
            let offset = svd_regs::esp32c3::addr("GPIO", name) - crate::periph::GPIO_BASE;
            assert_eq!(gpio.read(offset), Some(0), "{name}");
            assert!(gpio.write(offset, 0x40));
            assert_eq!(gpio.read(offset), Some(0x40));
        }
        assert_eq!(gpio.read(STRAP), Some(0x8), "STRAP is unchanged");
        assert!(!gpio.write(STRAP, 0), "STRAP is read-only");
        let out = svd_regs::esp32c3::addr("GPIO", "OUT") - crate::periph::GPIO_BASE;
        assert_eq!(gpio.read(out), None, "OUT is not modelled");
    }
}
