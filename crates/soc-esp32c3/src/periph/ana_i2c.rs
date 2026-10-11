//! The analog I2C master ("regi2c") through which the ROM configures analog blocks: PLL, SAR ADC,
//! bias. Source: report BA §2.2 (ROM `rom_chip_i2c_readReg_org@0x40038e58`,
//! `rom_chip_i2c_writeReg@0x40039170`, `rom_get_i2c_hostid@0x40038e30`; ESP-IDF `regi2c_defs.h`).
//!
//! The SVD does not describe this window (decision D17). No analog behaviour is modelled: a byte
//! written to an analog register reads back, nothing more (decision D24).
use super::Accepted;
use emu_core::Peripheral;
use std::collections::BTreeMap;

/// `DR_REG_RTC_I2C_BASE` (ESP-IDF `reg_base.h`).
pub(crate) const ANA_I2C_BASE: u32 = 0x6000_E000;
/// Command register of I2C host 0, and of host 1 (used for blocks `0x62..=0x64` only).
const HOST0_CMD: u32 = 0x00;
const HOST1_CMD: u32 = 0x04;
/// `I2C_MST_ANA_CONF0_REG`, `ANA_CONFIG_REG`, `ANA_CONFIG2_REG` (`regi2c_defs.h`).
const I2C_MST_ANA_CONF0: u32 = 0x40;
const ANA_CONFIG: u32 = 0x44;
const ANA_CONFIG2: u32 = 0x48;
const HOSTS: usize = 2;
const CONFIG_REGISTERS: usize = 3;

/// Command word: [7:0] block, [15:8] register, [23:16] data, bit 24 write, bit 25 busy.
const CMD_DATA_SHIFT: u32 = 16;
const CMD_DATA_MASK: u32 = 0xFF << CMD_DATA_SHIFT;
const CMD_WRITE: u32 = 1 << 24;
const CMD_BUSY: u32 = 1 << 25;

const WHY: &str =
    "analog register bus: bytes are stored, the analog blocks (PLL, SAR ADC, bias) do nothing";

pub(crate) fn accepted() -> Vec<Accepted> {
    vec![
        Accepted::at("ANA_I2C", "HOST0_CMD", ANA_I2C_BASE + HOST0_CMD, WHY, "SP2"),
        Accepted::at("ANA_I2C", "HOST1_CMD", ANA_I2C_BASE + HOST1_CMD, WHY, "SP2"),
        Accepted::at(
            "ANA_I2C",
            "I2C_MST_ANA_CONF0",
            ANA_I2C_BASE + I2C_MST_ANA_CONF0,
            WHY,
            "SP2",
        ),
        Accepted::at(
            "ANA_I2C",
            "ANA_CONFIG",
            ANA_I2C_BASE + ANA_CONFIG,
            WHY,
            "SP2",
        ),
        Accepted::at(
            "ANA_I2C",
            "ANA_CONFIG2",
            ANA_I2C_BASE + ANA_CONFIG2,
            WHY,
            "SP2",
        ),
    ]
}

#[derive(Clone, Debug, Default)]
pub struct AnaI2c {
    command: [u32; HOSTS],
    /// The analog registers written so far: (host, block, register) to byte. Ordered: deterministic.
    bytes: BTreeMap<(usize, u8, u8), u8>,
    /// ASSUMPTION: (decision D24) these three reset to 0; their reset values are not documented and
    /// the QEMU oracle answers `0xFFFFFF`. Every use on the measured paths is read-modify-write.
    config: [u32; CONFIG_REGISTERS],
}

impl AnaI2c {
    #[must_use]
    pub fn new() -> AnaI2c {
        AnaI2c::default()
    }

    /// Runs the transfer `word` asks for and returns what the command register then reads: never busy.
    fn run(&mut self, host: usize, word: u32) -> u32 {
        let [block, register, data, _] = word.to_le_bytes();
        if word & CMD_WRITE != 0 {
            self.bytes.insert((host, block, register), data);
            return word & !CMD_BUSY;
        }
        // ASSUMPTION: (decision D24) an analog register that was never written reads 0.
        let read = self
            .bytes
            .get(&(host, block, register))
            .copied()
            .unwrap_or(0);
        (word & !(CMD_BUSY | CMD_DATA_MASK)) | (u32::from(read) << CMD_DATA_SHIFT)
    }

    fn config_index(offset: u32) -> Option<usize> {
        [I2C_MST_ANA_CONF0, ANA_CONFIG, ANA_CONFIG2]
            .iter()
            .position(|&o| o == offset)
    }
}

impl Peripheral for AnaI2c {
    fn read(&mut self, offset: u32) -> Option<u32> {
        match offset {
            HOST0_CMD => self.command.first().copied(),
            HOST1_CMD => self.command.get(1).copied(),
            _ => Self::config_index(offset).and_then(|i| self.config.get(i).copied()),
        }
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        let host = match offset {
            HOST0_CMD => 0,
            HOST1_CMD => 1,
            _ => {
                let Some(slot) = Self::config_index(offset).and_then(|i| self.config.get_mut(i))
                else {
                    return false;
                };
                *slot = value;
                return true;
            }
        };
        let answer = self.run(host, value);
        if let Some(slot) = self.command.get_mut(host) {
            *slot = answer;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUSY: u32 = 1 << 25;
    fn write_cmd(block: u32, register: u32, data: u32) -> u32 {
        0x0500_0000 | (data << 16) | (register << 8) | block
    }
    fn read_cmd(block: u32, register: u32) -> u32 {
        0x0400_0000 | (register << 8) | block
    }

    #[test]
    fn a_command_is_never_busy_and_a_read_returns_the_byte_written_before() {
        // BA §2.2: the ROM writes the command, spins while bit 25 is set, reads bits [23:16].
        let mut i2c = AnaI2c::new();
        assert!(i2c.write(HOST0_CMD, write_cmd(0x69, 0x07, 0x5A)));
        assert_eq!(i2c.read(HOST0_CMD).map(|v| v & BUSY), Some(0));
        assert!(i2c.write(HOST0_CMD, read_cmd(0x69, 0x07))); // BA §2 row 76: 0x4000769
        assert_eq!(i2c.read(HOST0_CMD), Some(0x0400_0769 | (0x5A << 16)));
    }

    #[test]
    fn a_register_never_written_reads_zero_and_the_two_hosts_are_separate() {
        let mut i2c = AnaI2c::new();
        i2c.write(HOST0_CMD, read_cmd(0x66, 0x02));
        assert_eq!(
            i2c.read(HOST0_CMD).map(|v| (v >> 16) & 0xFF),
            Some(0),
            "decision D24"
        );
        i2c.write(HOST0_CMD, write_cmd(0x66, 0x02, 0x6B)); // BBPLL MODE_HF = 0x6B (BA §5 step 10)
        i2c.write(HOST1_CMD, read_cmd(0x66, 0x02));
        assert_eq!(i2c.read(HOST1_CMD).map(|v| (v >> 16) & 0xFF), Some(0));
        i2c.write(HOST0_CMD, read_cmd(0x66, 0x02));
        assert_eq!(i2c.read(HOST0_CMD).map(|v| (v >> 16) & 0xFF), Some(0x6B));
    }

    #[test]
    fn a_busy_bit_written_by_software_is_not_kept() {
        let mut i2c = AnaI2c::new();
        i2c.write(HOST0_CMD, read_cmd(0x69, 0x07) | BUSY);
        assert_eq!(i2c.read(HOST0_CMD).map(|v| v & BUSY), Some(0));
    }

    #[test]
    fn the_three_configuration_registers_are_stored_from_zero() {
        let mut i2c = AnaI2c::new();
        for offset in [I2C_MST_ANA_CONF0, ANA_CONFIG, ANA_CONFIG2] {
            assert_eq!(i2c.read(offset), Some(0), "decision D24");
            assert!(i2c.write(offset, 0xFFFF_BFFF)); // BA §3 row 10
            assert_eq!(i2c.read(offset), Some(0xFFFF_BFFF));
        }
        assert_eq!(i2c.read(0x08), None);
        assert!(!i2c.write(0x4C, 0));
    }
}
