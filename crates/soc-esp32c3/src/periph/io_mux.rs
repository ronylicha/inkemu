//! IO MUX block: pad configuration registers.
use super::{Accepted, IO_MUX_BASE, Storage};
use crate::ctx::Ctx;
use emu_core::Peripheral;

/// Every IO MUX register of the SVD is stored (report BC §A.2), one row per run of consecutive
/// registers: `PIN_CTRL` and the 22 pads, then `DATE`, which sits apart at `+0xFC`. A row stands
/// for `count` *consecutive* registers, so the whole block cannot be one row.
pub(crate) fn accepted() -> Vec<Accepted> {
    let mut registers: Vec<_> = svd_regs::esp32c3::registers()
        .iter()
        .filter(|r| r.peripheral == "IO_MUX")
        .collect();
    registers.sort_by_key(|r| r.addr);
    let mut rows: Vec<Accepted> = Vec::new();
    for register in registers {
        match rows.last_mut() {
            Some(row) if row.addr + svd_regs::REGISTER_BYTES * row.count == register.addr => {
                row.count += 1;
            }
            _ => rows.push(Accepted::at(
                "IO_MUX",
                register.name,
                register.addr,
                if register.name == "DATE" {
                    "none: the version date of the block, stored as written"
                } else {
                    "pad function, pulls, drive strength, input enable"
                },
                "SP2",
            )),
        }
    }
    rows
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

impl Peripheral<Ctx> for IoMux {
    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
        self.pads.read(offset)
    }

    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        self.pads.write(offset, value)
    }
}
