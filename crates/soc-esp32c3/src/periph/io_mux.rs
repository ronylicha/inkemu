//! IO MUX block: pad configuration registers.
use super::{Accepted, IO_MUX_BASE, Storage};
use emu_core::Peripheral;

/// One row for the whole block: every pad register is stored (report BC §A.2).
pub(crate) fn accepted() -> Vec<Accepted> {
    let pads: Vec<_> = svd_regs::esp32c3::registers()
        .iter()
        .filter(|r| r.peripheral == "IO_MUX")
        .collect();
    pads.first()
        .map(|first| {
            // The block is far smaller than `u32::MAX` registers.
            let count = u32::try_from(pads.len()).unwrap_or(u32::MAX);
            Accepted::at(
                "IO_MUX",
                first.name,
                first.addr,
                "pad function, pulls, drive strength, input enable",
                "SP2",
            )
            .block(count)
        })
        .into_iter()
        .collect()
}

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
