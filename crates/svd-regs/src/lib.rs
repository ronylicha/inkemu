//! Register tables generated from Espressif's CMSIS-SVD files. No field data: see `docs/research/esp32c3-svd.md`, §5.3.
#![no_std]

/// One 32-bit memory-mapped register.
#[derive(Debug, PartialEq, Eq)]
pub struct RegisterInfo {
    pub addr: u32,
    pub peripheral: &'static str,
    pub name: &'static str,
    /// `0` when the SVD documents no reset value: check `reset_explicit` before trusting it.
    pub reset_value: u32,
    pub reset_explicit: bool,
}

/// Every register of these SVD files is 32 bits wide (the generator rejects any other size).
pub const REGISTER_BYTES: u32 = 4;

fn lookup_in(table: &'static [RegisterInfo], addr: u32) -> Option<&'static RegisterInfo> {
    let index = table.partition_point(|r| r.addr <= addr).checked_sub(1)?;
    let reg = table.get(index)?;
    (addr - reg.addr < REGISTER_BYTES).then_some(reg)
}

// The generated file sits next to `lib.rs` so that `svd-gen` has one flat output path; a
// module nested under `esp32c3` could not reach it without a directory that does not exist.
#[rustfmt::skip]
#[path = "esp32c3.rs"]
mod esp32c3_table;

/// ESP32-C3, from `esp32c3.svd` (espressif/svd release 2024-05-09).
pub mod esp32c3 {
    use crate::RegisterInfo;
    use crate::esp32c3_table::REGISTERS;

    /// The register containing `addr`, if the SVD documents one there.
    #[must_use]
    pub fn lookup(addr: u32) -> Option<&'static RegisterInfo> {
        crate::lookup_in(&REGISTERS, addr)
    }

    /// All documented registers, sorted by address.
    #[must_use]
    pub fn registers() -> &'static [RegisterInfo] {
        &REGISTERS
    }
}

#[cfg(test)]
mod tests;
