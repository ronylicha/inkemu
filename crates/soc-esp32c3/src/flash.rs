//! The SPI NOR flash chip behind the MSPI controllers. Read-only in this plan (decision D3):
//! the commands that would change its content are not modelled and are reported by their opcode.
use std::fmt;

/// An erased NOR flash byte.
pub const ERASED: u8 = 0xFF;
pub const MIN_FLASH_BYTES: u32 = 1 << 20;
/// The MMU names flash pages of 64 KiB with 8 bits (report CM §1): 16 MiB at most.
pub const MAX_FLASH_BYTES: u32 = 16 << 20;

/// Release from power-down: the first command of the ROM (`SPI_WakeUp`, FB §B.4).
const OP_RES: u8 = 0xAB;
/// Read JEDEC id (FB §D.1).
const OP_RDID: u8 = 0x9F;
/// Read status register 1 (FB §D.1).
const OP_RDSR: u8 = 0x05;
/// Read SFDP; the bootloader reads one byte at address 0x10 (BA §2.2, SPI1 row).
const OP_SFDP: u8 = 0x5A;
/// Program/erase resume, sent once by the bootloader (BA §2.2, SPI1 row).
const OP_RESUME: u8 = 0x7A;
/// Write enable and write disable (ESP-IDF `spi_flash_defs.h`; BC §D.2 item 6).
const OP_WREN: u8 = 0x06;
const OP_WRDI: u8 = 0x04;
/// Status register 1: bit 0 write-in-progress (always clear: nothing ever writes), bit 1 write-enable latch.
const STATUS_WEL: u8 = 1 << 1;
/// Largest exponent `1 << capacity` fits a `u32`.
const MAX_CAPACITY_EXPONENT: u8 = 31;

/// The three bytes a flash chip answers to a read-JEDEC-id command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JedecId {
    pub manufacturer: u8,
    pub memory_type: u8,
    /// `log2` of the capacity in bytes.
    pub capacity: u8,
}

impl JedecId {
    /// The id the flash of Espressif's QEMU answers, bytes `9D 60 18` (FB §B.6, §D.1). Decision D2:
    /// the 2nd-stage bootloader then takes the vendor branch the oracle was measured on.
    pub const QEMU_ORACLE: JedecId = JedecId {
        manufacturer: 0x9D,
        memory_type: 0x60,
        capacity: 0x18,
    };

    /// From the 24-bit form `manufacturer << 16 | type << 8 | capacity` (the one the ROM stores).
    #[must_use]
    pub const fn from_u24(id: u32) -> Option<JedecId> {
        let [top, manufacturer, memory_type, capacity] = id.to_be_bytes();
        if top != 0 {
            return None;
        }
        Some(JedecId {
            manufacturer,
            memory_type,
            capacity,
        })
    }

    #[must_use]
    pub const fn to_u24(self) -> u32 {
        u32::from_be_bytes([0, self.manufacturer, self.memory_type, self.capacity])
    }

    /// The bytes in the order the chip sends them.
    #[must_use]
    pub const fn bytes(self) -> [u8; 3] {
        [self.manufacturer, self.memory_type, self.capacity]
    }

    /// The size the capacity byte stands for.
    #[must_use]
    pub const fn capacity_bytes(self) -> Option<u32> {
        if self.capacity > MAX_CAPACITY_EXPONENT {
            return None;
        }
        Some(1 << self.capacity)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlashConfig {
    /// Size of the chip in bytes.
    pub size: u32,
    pub jedec_id: JedecId,
}

impl Default for FlashConfig {
    /// 16 MiB (spec §2: every Xteink device) with the id of the QEMU oracle.
    fn default() -> FlashConfig {
        FlashConfig {
            size: MAX_FLASH_BYTES,
            jedec_id: JedecId::QEMU_ORACLE,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FlashImageError {
    #[error(
        "the flash image is {len} bytes, longer than the {size} byte flash of the device; use an image built for this flash size, or a device whose flash is large enough"
    )]
    TooLong { len: usize, size: u32 },
    #[error(
        "a flash of {size} bytes cannot be modelled: the size must be a power of two from 1 MiB to 16 MiB; correct the flash size of the device profile"
    )]
    BadSize { size: u32 },
}

impl FlashConfig {
    /// # Errors
    /// [`FlashImageError::BadSize`] when the size is not a power of two from 1 MiB to 16 MiB.
    pub fn validate(&self) -> Result<(), FlashImageError> {
        let valid =
            self.size.is_power_of_two() && (MIN_FLASH_BYTES..=MAX_FLASH_BYTES).contains(&self.size);
        if valid {
            Ok(())
        } else {
            Err(FlashImageError::BadSize { size: self.size })
        }
    }
}

/// One SPI transaction as the chip sees it.
#[derive(Clone, Copy, Debug)]
pub struct FlashCommand<'a> {
    pub opcode: u8,
    pub address: Option<u32>,
    pub dummy_cycles: u32,
    /// Bytes the controller sends after the address and the dummy cycles.
    pub data_out: &'a [u8],
}

/// The chip model does not implement this opcode: nothing happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnmodeledCommand {
    pub opcode: u8,
}

/// The flash chip as its two users see it: the cache (content) and the SPI1 controller (commands).
pub trait FlashDevice: fmt::Debug {
    fn size(&self) -> u32;
    /// Content at `offset`; bytes the image does not hold, or past the chip, read erased.
    fn read(&self, offset: u32, out: &mut [u8]);
    /// Runs one transaction; `data_in` receives the bytes the chip clocks out.
    /// # Errors
    /// [`UnmodeledCommand`] when the opcode is not implemented; `data_in` is then untouched.
    fn execute(
        &mut self,
        command: &FlashCommand<'_>,
        data_in: &mut [u8],
    ) -> Result<(), UnmodeledCommand>;
}

/// A NOR flash holding the image it was given. The image is kept as supplied: what lies past it is
/// erased, so a blank chip costs no memory (decision D19).
#[derive(Clone)]
pub struct NorFlash {
    image: Vec<u8>,
    config: FlashConfig,
    write_enabled: bool,
}

impl fmt::Debug for NorFlash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NorFlash")
            .field("image_bytes", &self.image.len())
            .field("config", &self.config)
            .field("write_enabled", &self.write_enabled)
            .finish()
    }
}

impl NorFlash {
    /// An erased chip.
    #[must_use]
    pub const fn blank(config: FlashConfig) -> NorFlash {
        NorFlash {
            image: Vec::new(),
            config,
            write_enabled: false,
        }
    }

    /// # Errors
    /// [`FlashImageError::BadSize`] for a size the chip cannot have, [`FlashImageError::TooLong`]
    /// when the image does not fit.
    pub fn with_image(image: Vec<u8>, config: FlashConfig) -> Result<NorFlash, FlashImageError> {
        config.validate()?;
        if u64::try_from(image.len()).unwrap_or(u64::MAX) > u64::from(config.size) {
            return Err(FlashImageError::TooLong {
                len: image.len(),
                size: config.size,
            });
        }
        Ok(NorFlash {
            image,
            config,
            write_enabled: false,
        })
    }
}

impl FlashDevice for NorFlash {
    fn size(&self) -> u32 {
        self.config.size
    }

    fn read(&self, offset: u32, out: &mut [u8]) {
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = start
                .checked_add(i)
                .and_then(|index| self.image.get(index))
                .copied()
                .unwrap_or(ERASED);
        }
    }

    fn execute(
        &mut self,
        command: &FlashCommand<'_>,
        data_in: &mut [u8],
    ) -> Result<(), UnmodeledCommand> {
        match command.opcode {
            // Nothing to wake up or resume: the chip is never powered down or suspended.
            OP_RES | OP_RESUME => {}
            OP_RDID => {
                // The id repeats for as long as the controller keeps clocking.
                for (byte, id) in data_in
                    .iter_mut()
                    .zip(self.config.jedec_id.bytes().into_iter().cycle())
                {
                    *byte = id;
                }
            }
            OP_RDSR => data_in.fill(if self.write_enabled { STATUS_WEL } else { 0 }),
            // ASSUMPTION: the modelled part exposes no SFDP table and answers erased bytes. The
            // bootloader only checks that byte 0x10 is not the XMC manufacturer id (BA §2.2). What
            // the QEMU oracle answers is not recorded: the SP1b-4 lockstep settles it.
            OP_SFDP => data_in.fill(ERASED),
            OP_WREN => self.write_enabled = true,
            OP_WRDI => self.write_enabled = false,
            opcode => return Err(UnmodeledCommand { opcode }),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: usize = 1 << 20;

    fn run(flash: &mut NorFlash, opcode: u8, data_in: &mut [u8]) -> Result<(), UnmodeledCommand> {
        flash.execute(
            &FlashCommand {
                opcode,
                address: None,
                dummy_cycles: 0,
                data_out: &[],
            },
            data_in,
        )
    }

    #[test]
    fn a_blank_flash_reads_erased_everywhere_even_past_its_end() {
        let flash = NorFlash::blank(FlashConfig::default());
        assert_eq!(flash.size(), 16 << 20);
        let mut bytes = [0u8; 4];
        flash.read(0, &mut bytes);
        assert_eq!(bytes, [0xFF; 4]);
        flash.read(u32::MAX - 1, &mut bytes);
        assert_eq!(bytes, [0xFF; 4], "no wrap, no panic");
    }

    #[test]
    fn an_image_is_read_back_and_the_rest_of_the_chip_is_erased() {
        let flash = NorFlash::with_image(vec![0xE9, 3, 2, 0x4F], FlashConfig::default()).unwrap();
        let mut bytes = [0u8; 6];
        flash.read(2, &mut bytes);
        assert_eq!(bytes, [2, 0x4F, 0xFF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn an_image_longer_than_the_flash_is_refused_and_one_that_fits_exactly_is_not() {
        let small = FlashConfig {
            size: MIN_FLASH_BYTES,
            ..FlashConfig::default()
        };
        assert!(NorFlash::with_image(vec![0; MIB], small).is_ok());
        let err = NorFlash::with_image(vec![0; MIB + 1], small).unwrap_err();
        assert_eq!(
            err,
            FlashImageError::TooLong {
                len: MIB + 1,
                size: MIN_FLASH_BYTES
            }
        );
        let text = err.to_string();
        assert!(
            text.contains("1048577") && text.contains("1048576") && text.contains("; use"),
            "{text}"
        );
    }

    #[test]
    fn only_power_of_two_sizes_from_1_to_16_mib_are_valid() {
        let with = |size| FlashConfig {
            size,
            ..FlashConfig::default()
        };
        for size in [1u32 << 20, 2 << 20, 4 << 20, 8 << 20, 16 << 20] {
            assert_eq!(with(size).validate(), Ok(()));
        }
        for size in [0, 512 << 10, 3 << 20, 32 << 20] {
            assert_eq!(
                with(size).validate(),
                Err(FlashImageError::BadSize { size }),
                "{size}"
            );
            assert!(NorFlash::with_image(Vec::new(), with(size)).is_err());
        }
        assert!(
            FlashImageError::BadSize { size: 0 }
                .to_string()
                .contains("; correct the flash size")
        );
    }

    #[test]
    fn jedec_id_converts_between_its_number_and_its_wire_bytes() {
        let id = JedecId::from_u24(0x9D_6018).unwrap();
        assert_eq!(id, JedecId::QEMU_ORACLE);
        assert_eq!(id.bytes(), [0x9D, 0x60, 0x18], "FB §D.1: bytes on the wire");
        assert_eq!(id.to_u24(), 0x9D_6018, "FB §B.6: the form the ROM stores");
        assert_eq!(id.capacity_bytes(), Some(16 << 20));
        assert_eq!(JedecId::from_u24(0x0100_0000), None);
        assert_eq!(JedecId { capacity: 40, ..id }.capacity_bytes(), None);
    }

    #[test]
    fn identification_and_status_commands_answer_like_the_measured_chip() {
        let mut flash = NorFlash::blank(FlashConfig::default());
        let mut id = [0u8; 3];
        run(&mut flash, 0x9F, &mut id).unwrap();
        assert_eq!(id, [0x9D, 0x60, 0x18]);
        let mut status = [0xAAu8];
        run(&mut flash, 0x05, &mut status).unwrap();
        assert_eq!(status, [0], "WIP clear: FB §C row 12");
        run(&mut flash, 0xAB, &mut []).unwrap();
        run(&mut flash, 0x7A, &mut []).unwrap();
    }

    #[test]
    fn write_enable_and_disable_only_move_the_latch_bit() {
        let mut flash = NorFlash::blank(FlashConfig::default());
        let mut status = [0u8];
        run(&mut flash, 0x06, &mut []).unwrap();
        run(&mut flash, 0x05, &mut status).unwrap();
        assert_eq!(status, [0x02]);
        run(&mut flash, 0x04, &mut []).unwrap();
        run(&mut flash, 0x05, &mut status).unwrap();
        assert_eq!(status, [0]);
    }

    #[test]
    fn an_sfdp_read_answers_erased_bytes() {
        let mut flash = NorFlash::blank(FlashConfig::default());
        let mut byte = [0u8];
        let command = FlashCommand {
            opcode: 0x5A,
            address: Some(0x10),
            dummy_cycles: 8,
            data_out: &[],
        };
        flash.execute(&command, &mut byte).unwrap();
        assert_eq!(byte, [0xFF], "anything but 0x20 (XMC): BA §2.2");
    }

    #[test]
    fn commands_that_would_change_or_read_the_content_are_not_modeled() {
        let mut flash = NorFlash::with_image(vec![1, 2, 3, 4], FlashConfig::default()).unwrap();
        for opcode in [
            0x02, 0x20, 0xD8, 0xC7, 0x01, 0x31, 0x35, 0x03, 0xBB, 0x00, 0xFF,
        ] {
            assert_eq!(
                run(&mut flash, opcode, &mut [0u8; 4]),
                Err(UnmodeledCommand { opcode })
            );
        }
        let mut bytes = [0u8; 4];
        flash.read(0, &mut bytes);
        assert_eq!(bytes, [1, 2, 3, 4], "the content did not change");
    }

    #[test]
    fn debug_output_does_not_dump_the_image() {
        let flash = NorFlash::with_image(vec![0xAB; 4096], FlashConfig::default()).unwrap();
        assert!(format!("{flash:?}").len() < 200);
    }
}
