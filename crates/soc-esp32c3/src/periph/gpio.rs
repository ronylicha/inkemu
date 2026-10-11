//! GPIO block. Only the strapping register is modelled so far; pins arrive with the board wiring (SP2).
use emu_core::Peripheral;

/// `GPIO_STRAP_REG`: the strapping pins sampled at reset. Bit 3 set selects SPI flash boot (report §D.3).
const STRAP: u32 = 0x38;

#[derive(Clone, Debug)]
pub struct Gpio {
    strapping: u32,
}

impl Gpio {
    #[must_use]
    pub const fn new(strapping: u32) -> Gpio {
        Gpio { strapping }
    }
}

impl Peripheral for Gpio {
    fn read(&mut self, offset: u32) -> Option<u32> {
        (offset == STRAP).then_some(self.strapping)
    }
    fn write(&mut self, _offset: u32, _value: u32) -> bool {
        false
    }
}
