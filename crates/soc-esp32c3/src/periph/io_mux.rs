//! IO MUX block: pad configuration registers.
use super::{IO_MUX_BASE, Storage};
use emu_core::Peripheral;

/// Pin configuration is remembered; it only has an effect once boards are wired (SP2).
#[derive(Clone, Debug)]
pub struct IoMux {
    pads: Storage,
}

impl IoMux {
    #[must_use]
    pub fn new() -> IoMux {
        IoMux {
            pads: Storage::for_peripheral("IO_MUX", IO_MUX_BASE),
        }
    }
}

impl Default for IoMux {
    fn default() -> IoMux {
        IoMux::new()
    }
}

impl Peripheral for IoMux {
    fn read(&mut self, offset: u32) -> Option<u32> {
        self.pads.read(offset)
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        self.pads.write(offset, value)
    }
}
