//! The SPI NOR flash chip behind the MSPI controllers. Read-only in this plan (decision D3):
//! the commands that would change its content are not modelled and are reported by their opcode.
use std::fmt;

/// An erased NOR flash byte.
pub const ERASED: u8 = 0xFF;
/// One mebibyte, the unit device profiles write flash sizes in.
pub const MIB: u32 = 1 << 20;
pub const MIN_FLASH_BYTES: u32 = MIB;
/// The MMU names flash pages of 64 KiB with 8 bits (report CM §1): 16 MiB at most.
pub const MAX_FLASH_BYTES: u32 = 16 * MIB;

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

/// The flash chip of a device: its size and the id it answers. A value of this type always
/// describes a chip the machine can model and whose two fields agree, because
/// [`FlashConfig::new`] is the only way to build one:
///
/// - the size is a power of two from 1 MiB to 16 MiB;
/// - the capacity byte of the JEDEC id is `log2` of the size.
///
/// Known limit of the second rule: it is the one ESP-IDF's generic flash driver assumes
/// (`1 << (id & 0xFF)`), and some real parts do not follow it (Macronix MX25U encodes its capacity
/// as `0x37`, `0x38`). A profile for such a part needs a rule of its own here. The part of a
/// retail Xteink X4 is not established.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlashConfig {
    size: u32,
    jedec_id: JedecId,
}

/// Why a size and a JEDEC id do not describe a flash chip this model can have.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FlashConfigError {
    #[error(
        "a flash of {size} bytes cannot be modelled: the size must be a power of two from 1 MiB to 16 MiB; correct the flash size of the device profile"
    )]
    BadSize { size: u32 },
    #[error(
        "the flash is {size} bytes but the JEDEC id {id:#08x} ends with capacity byte {capacity:#04x}, which stands for another size; make the size and the last byte of the id agree (0x14 = 1 MiB … 0x18 = 16 MiB)"
    )]
    CapacityMismatch { size: u32, id: u32, capacity: u8 },
}

impl FlashConfig {
    /// 16 MiB (spec §2: every Xteink device) with the id of the QEMU oracle.
    pub const QEMU_ORACLE_16_MIB: FlashConfig =
        match FlashConfig::new(MAX_FLASH_BYTES, JedecId::QEMU_ORACLE) {
            Ok(config) => config,
            // INVARIANT: evaluated at compile time; capacity byte 0x18 is log2 of 16 MiB.
            Err(_) => panic!("the default flash configuration is valid"),
        };

    /// # Errors
    /// [`FlashConfigError::BadSize`] when `size` is not a power of two from 1 MiB to 16 MiB;
    /// otherwise [`FlashConfigError::CapacityMismatch`] when the capacity byte of `jedec_id` is not
    /// `log2(size)`.
    pub const fn new(size: u32, jedec_id: JedecId) -> Result<FlashConfig, FlashConfigError> {
        if !FlashConfig::size_is_valid(size) {
            return Err(FlashConfigError::BadSize { size });
        }
        match jedec_id.capacity_bytes() {
            Some(capacity) if capacity == size => Ok(FlashConfig { size, jedec_id }),
            _ => Err(FlashConfigError::CapacityMismatch {
                size,
                id: jedec_id.to_u24(),
                capacity: jedec_id.capacity,
            }),
        }
    }

    /// Whether a chip of `size` bytes can be modelled: a power of two from 1 MiB to 16 MiB. For
    /// callers that judge a size before they have an id to go with it.
    #[must_use]
    pub const fn size_is_valid(size: u32) -> bool {
        size.is_power_of_two() && size >= MIN_FLASH_BYTES && size <= MAX_FLASH_BYTES
    }

    /// Size of the chip in bytes.
    #[must_use]
    pub const fn size(&self) -> u32 {
        self.size
    }

    #[must_use]
    pub const fn jedec_id(&self) -> JedecId {
        self.jedec_id
    }
}

impl Default for FlashConfig {
    fn default() -> FlashConfig {
        FlashConfig::QEMU_ORACLE_16_MIB
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FlashImageError {
    #[error(
        "the flash image is {len} bytes, longer than the {size} byte flash of the device; use an image built for this flash size, or a device whose flash is large enough"
    )]
    TooLong { len: usize, size: u32 },
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

    /// A chip holding `image` from offset 0.
    ///
    /// # Errors
    /// [`FlashImageError::TooLong`] when the image does not fit.
    pub fn with_image(image: Vec<u8>, config: FlashConfig) -> Result<NorFlash, FlashImageError> {
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
            // bootloader only checks that byte 0x10 is not the XMC manufacturer id (BA §2.2). The
            // flash of the QEMU oracle rejects the command (`Unknown cmd 5a`), so the lockstep must
            // mask this read; the SFDP table of the real part, read on a device, settles the bytes.
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
        let small = FlashConfig::new(MIN_FLASH_BYTES, id_with(0x14)).unwrap();
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

    /// A JEDEC id whose capacity byte is `capacity`.
    fn id_with(capacity: u8) -> JedecId {
        JedecId {
            capacity,
            ..JedecId::QEMU_ORACLE
        }
    }

    #[test]
    fn a_config_exists_only_for_a_power_of_two_size_from_1_to_16_mib() {
        for (size, capacity) in [
            (1u32 << 20, 0x14),
            (2 << 20, 0x15),
            (4 << 20, 0x16),
            (8 << 20, 0x17),
            (16 << 20, 0x18),
        ] {
            assert!(FlashConfig::size_is_valid(size), "{size}");
            let config = FlashConfig::new(size, id_with(capacity)).unwrap();
            assert_eq!(
                (config.size(), config.jedec_id()),
                (size, id_with(capacity))
            );
        }
        for size in [0, 1, 512 << 10, 3 << 20, (16 << 20) + 1, 32 << 20, u32::MAX] {
            assert!(!FlashConfig::size_is_valid(size), "{size}");
            // The size is judged first: an id that agrees with no valid size changes nothing.
            for capacity in [0x14, 0x18, 0x19, 0xFF] {
                assert_eq!(
                    FlashConfig::new(size, id_with(capacity)),
                    Err(FlashConfigError::BadSize { size }),
                    "{size} with capacity byte {capacity:#04x}"
                );
            }
        }
    }

    #[test]
    fn a_config_whose_jedec_capacity_byte_contradicts_its_size_cannot_be_built() {
        // 0x18 stands for 16 MiB; 0x13 and 0x19 for sizes no chip of this model has; 0x20 and 0xFF
        // for sizes a `u32` cannot hold.
        for (size, capacity) in [
            (8u32 << 20, 0x18u8),
            (16 << 20, 0x17),
            (1 << 20, 0x13),
            (16 << 20, 0x19),
            (16 << 20, 0x20),
            (4 << 20, 0xFF),
            (4 << 20, 0x00),
        ] {
            assert_eq!(
                FlashConfig::new(size, id_with(capacity)),
                Err(FlashConfigError::CapacityMismatch {
                    size,
                    id: id_with(capacity).to_u24(),
                    capacity
                }),
                "{size} with capacity byte {capacity:#04x}"
            );
        }
    }

    #[test]
    fn the_default_config_is_the_16_mib_chip_of_the_qemu_oracle() {
        let config = FlashConfig::default();
        assert_eq!(config, FlashConfig::QEMU_ORACLE_16_MIB);
        assert_eq!(
            Ok(config),
            FlashConfig::new(MAX_FLASH_BYTES, JedecId::QEMU_ORACLE)
        );
        assert_eq!(config.jedec_id().capacity_bytes(), Some(config.size()));
    }

    #[test]
    fn config_errors_state_the_cause_then_the_remedy() {
        let bad_size = FlashConfigError::BadSize { size: 3 << 20 }.to_string();
        assert!(
            bad_size.contains("3145728 bytes")
                && bad_size.contains("; correct the flash size of the device profile"),
            "{bad_size}"
        );
        let mismatch = FlashConfig::new(8 << 20, JedecId::QEMU_ORACLE)
            .unwrap_err()
            .to_string();
        assert!(
            mismatch.contains("8388608 bytes")
                && mismatch.contains("JEDEC id 0x9d6018")
                && mismatch.contains("capacity byte 0x18")
                && mismatch.contains("; make "),
            "{mismatch}"
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
    fn a_refused_command_leaves_the_receive_buffer_as_it_was() {
        let mut flash = NorFlash::blank(FlashConfig::default());
        let mut data_in = [0x5A, 0xA5, 0x3C];
        assert_eq!(
            run(&mut flash, 0x03, &mut data_in),
            Err(UnmodeledCommand { opcode: 0x03 })
        );
        assert_eq!(data_in, [0x5A, 0xA5, 0x3C], "nothing was clocked in");
    }

    #[test]
    fn a_read_of_no_byte_is_accepted_and_does_nothing() {
        let flash = NorFlash::with_image(vec![1, 2, 3], FlashConfig::default()).unwrap();
        flash.read(1, &mut []);
        flash.read(u32::MAX, &mut []);
        let mut none: [u8; 0] = [];
        let mut chip = flash;
        run(&mut chip, 0x9F, &mut none).unwrap();
        run(&mut chip, 0x05, &mut none).unwrap();
    }

    #[test]
    fn a_read_id_longer_than_the_id_repeats_it() {
        let mut flash = NorFlash::blank(FlashConfig::default());
        let mut id = [0u8; 8];
        run(&mut flash, 0x9F, &mut id).unwrap();
        assert_eq!(id, [0x9D, 0x60, 0x18, 0x9D, 0x60, 0x18, 0x9D, 0x60]);
    }

    #[test]
    fn debug_output_does_not_dump_the_image() {
        let flash = NorFlash::with_image(vec![0xAB; 4096], FlashConfig::default()).unwrap();
        assert!(format!("{flash:?}").len() < 200);
    }
}
