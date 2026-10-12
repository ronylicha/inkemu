//! The SPI NOR flash chip behind the MSPI controllers: its content, its status register, and the
//! commands the boot chain and an ESP-IDF application send it (reports FW §C and §G, SF §B to §D).
//!
//! A program or an erase completes inside the command that asks for it: the write-in-progress bit
//! is never read set (FW §A.5: every measured poll ends on its first read). The durations of a
//! real part (FW §A.6) belong to the plan that calibrates time.
use std::fmt;

/// An erased NOR flash byte.
pub const ERASED: u8 = 0xFF;
/// The unit of a sector erase, and of the "modified" state of the chip.
pub const SECTOR_BYTES: u32 = 4 << 10;
/// The unit of a block erase (`D8`): 64 KiB for the ROM and for ESP-IDF alike (FW §A.1).
pub const BLOCK_BYTES: u32 = 64 << 10;
/// The unit of a page program: a program that reaches the end of its page wraps to its start
/// (FW §C).
pub const PAGE_BYTES: u32 = 256;
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
/// Read SFDP; the bootloader reads one byte at address 0x10, after 8 dummy clocks (BA §2.2, SPI1
/// row).
const OP_SFDP: u8 = 0x5A;
const SFDP_DUMMY_CYCLES: u32 = 8;
/// Program/erase resume, sent once by the bootloader (BA §2.2, SPI1 row).
const OP_RESUME: u8 = 0x7A;
/// Write enable and write disable (ESP-IDF `spi_flash_defs.h`; BC §D.2 item 6).
const OP_WREN: u8 = 0x06;
const OP_WRDI: u8 = 0x04;
/// Write the status register: one data byte (FW §C; ISSI IS25LP128F §8).
const OP_WRSR: u8 = 0x01;
/// Page program (FW §A.2, §A.3: the only opcode a measured boot writes data with).
const OP_PP: u8 = 0x02;
/// Sector erase, 4 KiB (FW §A.2, §A.3).
const OP_SE: u8 = 0x20;
/// Block erase, 64 KiB: what ESP-IDF sends for a step of an erase that covers a whole block
/// (`IDF:spi_flash/esp_flash_api.c:728-742`; SF §D), and the ROM's `esp_rom_spiflash_erase_block`.
const OP_BE: u8 = 0xD8;
/// Chip erase: both opcodes of the datasheet. The ROM's `SPI_chip_erase` is their only sender
/// (FW §A.1, §A.3); ESP-IDF erases a chip block by block.
const OP_CE: u8 = 0xC7;
const OP_CE_ALT: u8 = 0x60;
/// The data reads, each with the dummy clocks it takes after its 24-bit address: slow read,
/// fast read, dual output, dual I/O, quad output, quad I/O
/// (`IDF:spi_flash/include/spi_flash/spi_flash_defs.h:29-39, 60-71`; FW §A.2). The image header
/// chooses the one an application uses; `BB` is the one measured (DIO: FW key result 3, SF §C:
/// `USER1 = 0x5C000003`, 24 address bits and 4 dummy clocks).
const READS: [(u8, u32); 6] = [
    (0x03, 0),
    (0x0B, 8),
    (0x3B, 8),
    (0xBB, 4),
    (0x6B, 8),
    (0xEB, 6),
];
/// Status register (FW §C): bit 0 write-in-progress (never set here), bit 1 the write-enable
/// latch, bits 5:2 block protection `BP3..BP0`, bit 6 quad enable, bit 7 `SRWD`.
const STATUS_WEL: u8 = 1 << 1;
const STATUS_BP: u8 = 0b0011_1100;
/// The bits a status write stores: `SRWD`, `QE` and `BP3..BP0`.
const STATUS_WRITABLE: u8 = 0b1111_1100;
/// The address length of every command that takes an address: three bytes. The 4-byte-address
/// opcodes ESP-IDF sends above 16 MiB (`12`, `21`, `DC`: FW §A.2) are other opcodes, not modelled.
const ADDRESS_BITS: u32 = 24;
/// One bit per sector of the largest chip.
const SECTOR_WORDS: usize = (MAX_FLASH_BYTES / SECTOR_BYTES / u64::BITS) as usize;
/// A sector that holds nothing: what the sectors of an image are compared with.
const ERASED_SECTOR: [u8; SECTOR_BYTES as usize] = [ERASED; SECTOR_BYTES as usize];
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

/// The address phase of a transaction: the value the controller clocks out after the opcode, and
/// on how many bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlashAddress {
    pub value: u32,
    pub bits: u32,
}

/// One SPI transaction as the chip sees it.
#[derive(Clone, Copy, Debug)]
pub struct FlashCommand<'a> {
    pub opcode: u8,
    /// `None` when the transaction has no address phase.
    pub address: Option<FlashAddress>,
    pub dummy_cycles: u32,
    /// Bytes the controller sends after the address and the dummy cycles.
    pub data_out: &'a [u8],
}

/// What a command the model knows takes after its opcode: the one frame the model runs it in.
///
/// A part judges a command on everything it received while its chip select was low: it drops
/// one whose chip select rises off a byte boundary (report FW §C, row "Framing"). What it makes
/// of an opcode followed by bytes its datasheet does not draw for it — a write enable followed
/// by an address, a status read after dummy clocks — is in no report, so the model runs none of
/// them: [`FlashRefusal::Form`]. The frames of [`FlashFrame::of`] are the only ones the ROM,
/// the bootloader and ESP-IDF send.
///
/// ASSUMPTION: the bytes a controller clocks *in* are not part of the frame that is judged. A
/// command that answers nothing (`06`, `04`, `AB`, `7A`, a program, an erase, a status write)
/// runs whatever the length of the data-in phase the controller adds, and leaves those bytes as
/// the controller gave them. On a part those clocks are more bytes of the command. No ROM or
/// ESP-IDF code sends such a transfer (the bootloader and ESP-IDF give a data-in length only to
/// the commands that answer: FW §A.2, §A.3); one read against a device settles it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlashFrame {
    /// Bits of the address phase: 24, or 0 for a command without an address.
    pub address_bits: u32,
    /// Dummy clocks between the address and the data.
    pub dummy_cycles: u32,
    /// The fewest bytes the controller sends after them.
    pub data_out_min: u32,
    /// The most: `u32::MAX` for a page program, of which the chip keeps the last 256.
    pub data_out_max: u32,
}

impl FlashFrame {
    /// An opcode and nothing else on the way out.
    const OPCODE_ALONE: FlashFrame = FlashFrame::addressed(0, 0);

    /// `address_bits` of address, `dummy_cycles` dummy clocks, no data out.
    const fn addressed(address_bits: u32, dummy_cycles: u32) -> FlashFrame {
        FlashFrame {
            address_bits,
            dummy_cycles,
            data_out_min: 0,
            data_out_max: 0,
        }
    }

    /// The frame of `opcode`; `None` for an opcode the model does not implement.
    ///
    /// Each frame is the one a measured boot or the ROM and ESP-IDF sources send:
    /// - the data reads: a 24-bit address, then the dummy clocks each one takes (0 for `03`, 4
    ///   for `BB`, 6 for `EB`, 8 for `0B`, `3B` and `6B`);
    /// - `AB`, `9F`, `05`, `06`, `04`, `C7`/`60`: the opcode alone, as the hard-wired bits of
    ///   `SPI1.CMD` send them (FW §A.1) and as the bootloader and ESP-IDF send `9F`, `05`, `7A`
    ///   and `04` through `USR` (`USER = 0x90000080` or `0x80000000`: no address, no dummy
    ///   clock, no data out; FW §A.2, §A.3, BA §2.2);
    /// - `5A`: a 24-bit address and 8 dummy clocks (`USER1 = 23 << 26 | 7`, BA §2.2);
    /// - `01`: one data byte (FW §C: this part has no second status byte);
    /// - `02`: a 24-bit address and at least one byte; `20`, `D8`: a 24-bit address (FW §A.2,
    ///   §A.3).
    #[must_use]
    pub fn of(opcode: u8) -> Option<FlashFrame> {
        if let Some(&(_, dummy_cycles)) = READS.iter().find(|(read, _)| *read == opcode) {
            return Some(FlashFrame::addressed(ADDRESS_BITS, dummy_cycles));
        }
        match opcode {
            OP_RES | OP_RESUME | OP_RDID | OP_RDSR | OP_WREN | OP_WRDI | OP_CE | OP_CE_ALT => {
                Some(FlashFrame::OPCODE_ALONE)
            }
            OP_SFDP => Some(FlashFrame::addressed(ADDRESS_BITS, SFDP_DUMMY_CYCLES)),
            OP_SE | OP_BE => Some(FlashFrame::addressed(ADDRESS_BITS, 0)),
            OP_WRSR => Some(FlashFrame {
                data_out_min: 1,
                data_out_max: 1,
                ..FlashFrame::OPCODE_ALONE
            }),
            OP_PP => Some(FlashFrame {
                data_out_min: 1,
                data_out_max: u32::MAX,
                ..FlashFrame::addressed(ADDRESS_BITS, 0)
            }),
            _ => None,
        }
    }

    /// Whether `command` has this frame.
    fn admits(self, command: &FlashCommand<'_>) -> bool {
        let data_out = u32::try_from(command.data_out.len()).unwrap_or(u32::MAX);
        command.address.map_or(0, |address| address.bits) == self.address_bits
            && command.dummy_cycles == self.dummy_cycles
            && (self.data_out_min..=self.data_out_max).contains(&data_out)
    }
}

/// The frame in words, for the remedy of a refusal: "a 24-bit address, 4 dummy clocks and no
/// data out".
impl fmt::Display for FlashFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.address_bits {
            0 => write!(f, "no address, ")?,
            bits => write!(f, "a {bits}-bit address, ")?,
        }
        match self.dummy_cycles {
            0 => write!(f, "no dummy clock and ")?,
            cycles => write!(f, "{cycles} dummy clocks and ")?,
        }
        match (self.data_out_min, self.data_out_max) {
            (0, 0) => write!(f, "no data out"),
            (1, 1) => write!(f, "1 byte of data out"),
            (min, max) if min == max => write!(f, "{min} bytes of data out"),
            (min, u32::MAX) => write!(f, "{min} byte of data out or more"),
            (min, max) => write!(f, "{min} to {max} bytes of data out"),
        }
    }
}

/// Why the chip did not run a command: nothing was read and nothing changed, the write-enable
/// latch included.
///
/// A write (a program, an erase, a status write) is judged in this order:
/// 1. its frame: a frame the model does not know is refused whether or not the latch is set,
///    because the model cannot say what command a chip would see in it;
/// 2. the write-enable latch: without it a chip ignores the write wherever it points and whatever
///    is protected (FW §C), so the model does too, and reports nothing;
/// 3. its range, then the block protection: refused by name, and the latch stays set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlashRefusal {
    /// The model does not implement this opcode.
    Unmodeled { opcode: u8 },
    /// The opcode is known, but not sent in its frame ([`FlashFrame::of`]): an address that is
    /// not as long as the command takes (0 when there is none), other dummy clocks, or a number
    /// of data bytes it does not carry. What a chip does with such a frame is not modelled.
    Form {
        opcode: u8,
        address_bits: u32,
        dummy_cycles: u32,
        data_bytes: u32,
    },
    /// The command addresses a byte past the end of the chip. A real part wraps, or ignores the
    /// address bits it does not have (FW §C); the model does not guess which.
    OutOfRange { opcode: u8, address: u32, len: u32 },
    /// A program or an erase while a block-protect bit of the status register is set. Which
    /// blocks the bits protect depends on the part, and the part of a retail device is not
    /// established (FW §C, §O.2): no protection map is modelled.
    Protected { opcode: u8, status: u8 },
}

/// One operation that wrote the chip: an entry of the journal of a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlashWrite {
    /// `data` was programmed from `address` on: each byte could only clear bits of the content
    /// (`new = old AND data`), and the address wrapped at the end of its 256-byte page.
    Program { address: u32, data: Vec<u8> },
    /// The `len` bytes from `start` were set to `0xFF`: a sector, a block or the whole chip.
    Erase { start: u32, len: u32 },
    /// The status register received `value` (its writable bits).
    Status { value: u8 },
}

/// The most entries the journal of a run holds between two calls of
/// [`Esp32C3::take_flash_writes`](crate::machine::Esp32C3::take_flash_writes); the operations
/// performed while it is full are counted by
/// [`Esp32C3::flash_writes_dropped`](crate::machine::Esp32C3::flash_writes_dropped).
///
/// The journal itself stays inside the crate: the SPI1 controller owns it and records into it,
/// and a front end takes its entries from the machine.
///
/// ```compile_fail
/// // No caller builds or holds a journal: only its entries leave the crate.
/// let journal = soc_esp32c3::flash::FlashJournal::default();
/// ```
pub const JOURNAL_CAPACITY: usize = 1024;

/// The operations that wrote the chip during a run, oldest first. The firmware decides how many
/// there are: the journal keeps the first [`JOURNAL_CAPACITY`] that nobody took and counts the
/// others, so that its memory does not grow with what the firmware does.
///
/// Its one owner is the SPI1 controller ([`crate::periph::spi_mem::Spi1`]); what it holds
/// leaves the crate through the machine.
#[derive(Clone, Debug, Default)]
pub(crate) struct FlashJournal {
    writes: Vec<FlashWrite>,
    dropped: u64,
}

impl FlashJournal {
    pub(crate) fn record(&mut self, write: FlashWrite) {
        if self.writes.len() < JOURNAL_CAPACITY {
            self.writes.push(write);
        } else {
            self.dropped += 1;
        }
    }

    /// The entries recorded since the last call, which leaves the journal empty.
    pub(crate) fn take(&mut self) -> Vec<FlashWrite> {
        std::mem::take(&mut self.writes)
    }

    /// How many operations were not recorded because the journal was full: since power-on.
    pub(crate) const fn dropped(&self) -> u64 {
        self.dropped
    }
}

/// The flash chip as its two users see it: the cache (content) and the SPI1 controller (commands).
pub trait FlashDevice: fmt::Debug {
    fn size(&self) -> u32;
    /// Content at `offset`; bytes the image does not hold, or past the chip, read erased.
    fn read(&self, offset: u32, out: &mut [u8]);
    /// Runs one transaction; `data_in` receives the bytes the chip clocks out. `Ok(Some(_))`
    /// says what a program, an erase or a status write did; `Ok(None)` is every other command,
    /// and a write the chip ignored because its write-enable latch was clear.
    /// # Errors
    /// [`FlashRefusal`] when the model cannot run the command; `data_in` and the chip are then
    /// untouched.
    fn execute(
        &mut self,
        command: &FlashCommand<'_>,
        data_in: &mut [u8],
    ) -> Result<Option<FlashWrite>, FlashRefusal>;
    /// A number that changes whenever a byte of the content does: the version of what was
    /// decoded or read from it.
    fn generation(&self) -> u64;
    /// How many 4 KiB sectors had a byte changed by a program or an erase since the chip was
    /// built. A sector whose bytes were changed and changed back counts (erased, then programmed
    /// with the bytes it held): the count says which sectors were written with other bytes at
    /// some point, not which ones differ from the content the chip started from.
    fn modified_sectors(&self) -> u32;
    /// Whether an erase that reaches the `len` bytes from `start` would change a byte: whether
    /// a 4 KiB sector the range reaches holds a byte other than `0xFF`. A range of no byte
    /// reaches nothing. The answer costs no reading of the content.
    ///
    /// A chip that keeps no account of it answers `true`: the caller then takes every erase
    /// to change all it covers, which is never less than what it changed.
    fn holds_data(&self, _start: u32, _len: u32) -> bool {
        true
    }
}

/// A set of 4 KiB sectors of the chip: one bit per sector of the largest chip, 512 bytes
/// whatever the firmware does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SectorSet([u64; SECTOR_WORDS]);

impl SectorSet {
    const EMPTY: SectorSet = SectorSet([0; SECTOR_WORDS]);

    /// The word and the bit of `sector`; a sector no chip has is in no set.
    fn place(sector: u32) -> (usize, u64) {
        (index(sector / u64::BITS), 1 << (sector % u64::BITS))
    }

    fn insert(&mut self, sector: u32) {
        let (word, bit) = SectorSet::place(sector);
        if let Some(word) = self.0.get_mut(word) {
            *word |= bit;
        }
    }

    /// The bits of the word whose first sector is `base` that the sectors `first..end` cover:
    /// none when the word lies outside the range.
    fn covered(base: u32, first: u32, end: u32) -> u64 {
        let low = first.saturating_sub(base).min(u64::BITS);
        let high = end.saturating_sub(base).min(u64::BITS);
        if low >= high {
            return 0;
        }
        // INVARIANT: `low < high <= 64`, so neither shift is by 64 bits or more.
        (u64::MAX << low) & (u64::MAX >> (u64::BITS - high))
    }

    /// Whether one of the sectors `first..first + count` is in the set. It reads the words the
    /// range reaches and no other: one word for a range of 64 sectors or fewer that starts at
    /// a multiple of 64, whatever the two numbers are.
    fn intersects(&self, first: u32, count: u32) -> bool {
        let end = first.saturating_add(count);
        // The words of the set, as `SECTOR_WORDS` counts them: an index below 64, which times
        // 64 cannot overflow.
        (first / u64::BITS..MAX_FLASH_BYTES / SECTOR_BYTES / u64::BITS)
            .take_while(|word| word * u64::BITS < end)
            .any(|word| {
                let covered = SectorSet::covered(word * u64::BITS, first, end);
                self.0
                    .get(index(word))
                    .is_some_and(|bits| bits & covered != 0)
            })
    }

    /// Takes the sectors `first..first + count` out of the set and hands each one that was in
    /// it to `taken`, in order. It reads the 64 words of the set and calls `taken` for the
    /// sectors found, however long the range is: emptying a range that holds nothing costs
    /// nothing more.
    fn remove_range(&mut self, first: u32, count: u32, mut taken: impl FnMut(u32)) {
        let end = first.saturating_add(count);
        for (base, word) in (0u32..).step_by(64).zip(self.0.iter_mut()) {
            let covered = SectorSet::covered(base, first, end);
            let mut found = *word & covered;
            *word &= !covered;
            while found != 0 {
                taken(base + found.trailing_zeros());
                found &= found - 1;
            }
        }
    }

    #[cfg(test)]
    fn contains(&self, sector: u32) -> bool {
        let (word, bit) = SectorSet::place(sector);
        self.0.get(word).is_some_and(|word| word & bit != 0)
    }

    fn len(&self) -> u32 {
        self.0.iter().map(|word| word.count_ones()).sum()
    }
}

/// A NOR flash holding the image it was given. The image is kept as supplied and grows only as
/// far as a program reaches: what lies past it is erased, so a blank chip costs no memory
/// (decision D19). It never shrinks: an erase leaves it its length, at most the size of the chip.
#[derive(Clone)]
pub struct NorFlash {
    image: Vec<u8>,
    config: FlashConfig,
    /// The status register, write-enable latch included. It is not part of the content: a chip
    /// that is built starts from the factory value `0x00` (FW §C).
    status: u8,
    generation: u64,
    /// The sectors whose content changed.
    modified: SectorSet,
    /// The sectors that hold a byte other than `0xFF`, and only those: what an erase has to
    /// erase. It is computed once from the image the chip is built with, a program that stores
    /// such a byte puts its sector in, and an erase takes it out. An erase then costs the
    /// sectors it changes, not the size of its unit: a firmware that erases the chip in a loop
    /// would otherwise make the host read 16 MiB at every round.
    ///
    /// INVARIANT: a sector that is not in the set holds only `0xFF`, in the image and past it.
    not_blank: SectorSet,
}

impl fmt::Debug for NorFlash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NorFlash")
            .field("image_bytes", &self.image.len())
            .field("config", &self.config)
            .field("status", &self.status)
            .field("generation", &self.generation)
            .field("modified_sectors", &self.modified_sectors())
            .finish_non_exhaustive()
    }
}

/// `offset` as an index; an offset no slice can hold indexes nothing.
fn index(offset: u32) -> usize {
    usize::try_from(offset).unwrap_or(usize::MAX)
}

impl NorFlash {
    /// An erased chip.
    #[must_use]
    pub const fn blank(config: FlashConfig) -> NorFlash {
        NorFlash {
            image: Vec::new(),
            config,
            status: 0,
            generation: 0,
            modified: SectorSet::EMPTY,
            not_blank: SectorSet::EMPTY,
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
        // The image is read once, here: it is no longer than the chip, so the loop has one
        // round per sector of the chip at most (4096), whatever the caller supplies.
        let mut not_blank = SectorSet::EMPTY;
        for (sector, bytes) in (0u32..).zip(image.chunks(ERASED_SECTOR.len())) {
            if *bytes != ERASED_SECTOR[..bytes.len()] {
                not_blank.insert(sector);
            }
        }
        Ok(NorFlash {
            image,
            not_blank,
            ..NorFlash::blank(config)
        })
    }

    fn form(command: &FlashCommand<'_>) -> FlashRefusal {
        FlashRefusal::Form {
            opcode: command.opcode,
            address_bits: command.address.map_or(0, |address| address.bits),
            dummy_cycles: command.dummy_cycles,
            data_bytes: u32::try_from(command.data_out.len()).unwrap_or(u32::MAX),
        }
    }

    /// Refuses a write the model cannot place or cannot judge: past the end of the chip, or
    /// under a protection it has no map of. Called once the latch is known to be set: a chip
    /// ignores a write without it, wherever it points (the order is the one of
    /// [`FlashRefusal`]).
    fn writable(&self, opcode: u8, address: u32, len: u32) -> Result<(), FlashRefusal> {
        if address >= self.config.size {
            return Err(FlashRefusal::OutOfRange {
                opcode,
                address,
                len,
            });
        }
        if self.status & STATUS_BP != 0 {
            return Err(FlashRefusal::Protected {
                opcode,
                status: self.status,
            });
        }
        Ok(())
    }

    /// Records that the sector holding `offset` changed.
    fn mark_modified(&mut self, offset: u32) {
        self.modified.insert(offset / SECTOR_BYTES);
        self.generation += 1;
    }

    /// `02`: ANDs the data into the page of the address (FW §C: a program only clears bits),
    /// wrapping at the end of the page; of more than a page of data the last 256 bytes are kept.
    fn program(
        &mut self,
        address: u32,
        data_out: &[u8],
    ) -> Result<Option<FlashWrite>, FlashRefusal> {
        // Of more than a page of data the chip keeps the last 256 bytes (FW §C): the first
        // `skip` bytes are dropped, and the loop below is bounded by `PAGE_BYTES` whatever the
        // caller sends. The SPI1 controller sends 64 bytes at most.
        let skip = data_out.len().saturating_sub(index(PAGE_BYTES));
        let data = &data_out[skip..];
        if self.status & STATUS_WEL == 0 {
            return Ok(None);
        }
        let len = u32::try_from(data.len()).unwrap_or(PAGE_BYTES);
        self.writable(OP_PP, address, len)?;
        let page = address & !(PAGE_BYTES - 1);
        // Where the first byte kept lands: the page counter after the bytes that were dropped.
        let dropped = u32::try_from(skip % index(PAGE_BYTES)).unwrap_or(0);
        let first = (address - page + dropped) % PAGE_BYTES;
        // The image grows to the end of the page it is programmed in, never past the chip:
        // `writable` checked that the page starts inside it, and the size is a multiple of a page.
        let page_end = index(page + PAGE_BYTES);
        if self.image.len() < page_end {
            self.image.resize(page_end, ERASED);
        }
        let mut changed = false;
        for (position, &byte) in (first..).zip(data) {
            let at = index(page + position % PAGE_BYTES);
            if let Some(cell) = self.image.get_mut(at) {
                let programmed = *cell & byte;
                changed |= programmed != *cell;
                *cell = programmed;
            }
        }
        if changed {
            // A program only clears bits: a byte it changed is no longer `0xFF`, and the
            // sector of the page (a page lies inside one sector) now has something to erase.
            self.not_blank.insert(page / SECTOR_BYTES);
            self.mark_modified(page);
        }
        // "After the completion of any Write Non-Volatile Register, Program and Erase operation"
        // the latch is clear (FW §C); ESP-IDF takes a latch still set for a command the chip
        // did not accept (`IDF:spi_flash/spi_flash_chip_generic.c:462-468`; FW key result 5).
        self.status &= !STATUS_WEL;
        Ok(Some(FlashWrite::Program {
            address: page + first,
            data: data.to_vec(),
        }))
    }

    /// Sets `len` bytes from `start` to `0xFF` and records the sectors whose content changed.
    /// `start` and `len` are multiples of a sector.
    ///
    /// Only the sectors that hold something to erase are visited, and each of them changes: a
    /// sector of `0xFF` is left alone and is no modification. The work is bounded by the
    /// sectors of the largest chip (4096 of 4 KiB) and is, for a range that holds nothing, the
    /// reading of the set.
    fn fill_erased(&mut self, start: u32, len: u32) {
        let (image, modified, generation) =
            (&mut self.image, &mut self.modified, &mut self.generation);
        self.not_blank
            .remove_range(start / SECTOR_BYTES, len / SECTOR_BYTES, |sector| {
                // A sector that holds a byte other than `0xFF` starts inside the image; the
                // image may end inside it, and what lies past the image is erased already.
                let from = index(sector.saturating_mul(SECTOR_BYTES));
                let to = from.saturating_add(ERASED_SECTOR.len()).min(image.len());
                if let Some(bytes) = image.get_mut(from..to) {
                    bytes.fill(ERASED);
                }
                modified.insert(sector);
                *generation += 1;
            });
    }

    /// `20` and `D8`: erases the sector or the block that holds the address, whose low bits are
    /// ignored (FW §G).
    fn erase(
        &mut self,
        opcode: u8,
        address: u32,
        unit: u32,
    ) -> Result<Option<FlashWrite>, FlashRefusal> {
        if self.status & STATUS_WEL == 0 {
            return Ok(None);
        }
        self.writable(opcode, address, unit)?;
        // INVARIANT: the unit that holds an address inside the chip ends inside it too:
        // `writable` checked `address < size`, and the size is a power of two from 1 MiB
        // (`FlashConfig::new`), so a multiple of the 4 KiB and of the 64 KiB unit. The range
        // the journal names is therefore the range that was erased.
        let start = address & !(unit - 1);
        self.fill_erased(start, unit);
        self.status &= !STATUS_WEL;
        Ok(Some(FlashWrite::Erase { start, len: unit }))
    }

    /// `C7` and `60`: erases the whole chip.
    fn erase_chip(&mut self, opcode: u8) -> Result<Option<FlashWrite>, FlashRefusal> {
        if self.status & STATUS_WEL == 0 {
            return Ok(None);
        }
        self.writable(opcode, 0, self.config.size)?;
        // The image keeps its length: emptied, the next program in a high page would fill it
        // again up to that page, 16 MiB at every round of a firmware that erases in a loop.
        self.fill_erased(0, self.config.size);
        self.status &= !STATUS_WEL;
        Ok(Some(FlashWrite::Erase {
            start: 0,
            len: self.config.size,
        }))
    }

    /// A data read: the content from the address on, as many bytes as the controller clocks in.
    ///
    /// A read that would reach past the end of the chip is refused whole: the part of the
    /// datasheet rolls its address counter over to 0 there (FW §C), a smaller part ignores the
    /// address bits it does not have, and neither the ROM nor ESP-IDF ever asks (both reject the
    /// range first: FW §A.2, §A.3).
    ///
    /// ASSUMPTION: the quad reads (`6B`, `EB`) answer whatever the quad-enable bit of the status
    /// register holds. On the part they need it set (FW §C), and ESP-IDF sets it before it
    /// selects a quad mode (`IDF:spi_flash/spi_flash_chip_issi.c:39-47`); what the chip clocks
    /// out without it is not documented. An image whose header says QIO or QOUT, read against a
    /// device, settles it.
    fn read_data(
        &self,
        opcode: u8,
        address: u32,
        data_in: &mut [u8],
    ) -> Result<Option<FlashWrite>, FlashRefusal> {
        let len = u32::try_from(data_in.len()).unwrap_or(u32::MAX);
        // A read of no byte at the end of the chip reaches no byte past it, and is refused all
        // the same: an address is inside the chip or it is not, whatever is read from it.
        if address >= self.config.size || len > self.config.size - address {
            return Err(FlashRefusal::OutOfRange {
                opcode,
                address,
                len,
            });
        }
        self.read(address, data_in);
        Ok(None)
    }

    /// `01`: stores the writable bits of the one status byte this part has (FW §C: no status
    /// register 2). The stored bits last as long as the chip model: they are not in an image.
    ///
    /// ASSUMPTION: `SRWD` is stored and locks nothing. On the part it makes the register
    /// read-only while the `WP#` pin is low, and the level of that pin on a device is not
    /// established; the pin model of the board (SP2), or a schematic, settles it.
    fn write_status(
        &mut self,
        command: &FlashCommand<'_>,
    ) -> Result<Option<FlashWrite>, FlashRefusal> {
        // The frame of `01` is one data byte: `execute` let no other number through.
        let [value] = *command.data_out else {
            return Err(NorFlash::form(command));
        };
        if self.status & STATUS_WEL == 0 {
            return Ok(None);
        }
        // Storing the byte without its latch bit is the latch cleared at completion.
        self.status = value & STATUS_WRITABLE;
        Ok(Some(FlashWrite::Status { value: self.status }))
    }
}

impl FlashDevice for NorFlash {
    fn size(&self) -> u32 {
        self.config.size
    }

    fn generation(&self) -> u64 {
        self.generation
    }

    fn modified_sectors(&self) -> u32 {
        self.modified.len()
    }

    fn holds_data(&self, start: u32, len: u32) -> bool {
        // The sectors from the one that holds the first byte to the one that holds the last.
        let Some(last) = len.checked_sub(1).map(|span| start.saturating_add(span)) else {
            return false;
        };
        let first = start / SECTOR_BYTES;
        self.not_blank
            .intersects(first, last / SECTOR_BYTES - first + 1)
    }

    fn read(&self, offset: u32, out: &mut [u8]) {
        // The part of `out` the image holds is copied in one piece (the whole chip is read this
        // way when its content leaves the machine); the rest reads erased.
        let held = usize::try_from(offset)
            .ok()
            .and_then(|start| self.image.get(start..))
            .unwrap_or_default();
        let (copied, erased) = out.split_at_mut(held.len().min(out.len()));
        // INVARIANT: `copied` is no longer than `held`, by the `min` above.
        copied.copy_from_slice(&held[..copied.len()]);
        erased.fill(ERASED);
    }

    fn execute(
        &mut self,
        command: &FlashCommand<'_>,
        data_in: &mut [u8],
    ) -> Result<Option<FlashWrite>, FlashRefusal> {
        // The frame is judged first, for every command: see [`FlashFrame`] and the order of
        // [`FlashRefusal`].
        let Some(frame) = FlashFrame::of(command.opcode) else {
            return Err(FlashRefusal::Unmodeled {
                opcode: command.opcode,
            });
        };
        if !frame.admits(command) {
            return Err(NorFlash::form(command));
        }
        let address = command.address.map_or(0, |address| address.value);
        if READS.iter().any(|(read, _)| *read == command.opcode) {
            return self.read_data(command.opcode, address, data_in);
        }
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
            OP_RDSR => data_in.fill(self.status),
            // ASSUMPTION: the modelled part exposes no SFDP table and answers erased bytes. The
            // bootloader only checks that byte 0x10 is not the XMC manufacturer id (BA §2.2). The
            // flash of the QEMU oracle rejects the command (`Unknown cmd 5a`), so the lockstep must
            // mask this read; the SFDP table of the real part, read on a device, settles the bytes.
            OP_SFDP => data_in.fill(ERASED),
            OP_WREN => self.status |= STATUS_WEL,
            OP_WRDI => self.status &= !STATUS_WEL,
            OP_WRSR => return self.write_status(command),
            OP_PP => return self.program(address, command.data_out),
            OP_SE => return self.erase(OP_SE, address, SECTOR_BYTES),
            OP_BE => return self.erase(OP_BE, address, BLOCK_BYTES),
            opcode @ (OP_CE | OP_CE_ALT) => return self.erase_chip(opcode),
            // No opcode with a frame is left: this arm keeps the match total.
            opcode => return Err(FlashRefusal::Unmodeled { opcode }),
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: usize = 1 << 20;

    /// What [`FlashDevice::execute`] returns.
    type Done = Result<Option<FlashWrite>, FlashRefusal>;

    fn run(flash: &mut NorFlash, opcode: u8, data_in: &mut [u8]) -> Done {
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

    /// A command with a 24-bit address and data, no dummy clock: the form of a program and of
    /// an erase.
    fn send(flash: &mut NorFlash, opcode: u8, address: u32, data: &[u8]) -> Done {
        flash.execute(
            &FlashCommand {
                opcode,
                address: Some(FlashAddress {
                    value: address,
                    bits: 24,
                }),
                dummy_cycles: 0,
                data_out: data,
            },
            &mut [],
        )
    }

    /// Write enable, then the command: how the ROM and ESP-IDF send every program and erase.
    fn write(flash: &mut NorFlash, opcode: u8, address: u32, data: &[u8]) -> Done {
        run(flash, 0x06, &mut []).unwrap();
        send(flash, opcode, address, data)
    }

    /// A command without an address that carries `data`: the status write.
    fn send_data(flash: &mut NorFlash, opcode: u8, data: &[u8]) -> Done {
        flash.execute(
            &FlashCommand {
                opcode,
                address: None,
                dummy_cycles: 0,
                data_out: data,
            },
            &mut [],
        )
    }

    fn content(flash: &NorFlash, offset: u32, len: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; len];
        flash.read(offset, &mut bytes);
        bytes
    }

    fn status(flash: &mut NorFlash) -> u8 {
        let mut status = [0u8];
        run(flash, 0x05, &mut status).unwrap();
        status[0]
    }

    /// The bytes written as hexadecimal pairs; spaces are ignored.
    fn hex(text: &str) -> Vec<u8> {
        let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        digits
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
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
    fn a_read_of_any_range_is_the_image_where_it_has_bytes_and_erased_elsewhere() {
        let image: Vec<u8> = (0..=9).collect();
        let flash = NorFlash::with_image(image.clone(), FlashConfig::default()).unwrap();
        // (offset, length): inside the image, straddling its end, at its end, far past it, and
        // the offsets no chip has.
        for (offset, len) in [
            (0u32, 10usize),
            (0, 3),
            (7, 3),
            (7, 8),
            (10, 4),
            (11, 4),
            (0, 0),
            (10, 0),
            (1 << 24, 4),
            (u32::MAX, 4),
        ] {
            let mut bytes = vec![0x55u8; len];
            flash.read(offset, &mut bytes);
            let expected: Vec<u8> = (u64::from(offset)..)
                .take(len)
                .map(|at| usize::try_from(at).ok().and_then(|at| image.get(at)))
                .map(|byte| byte.copied().unwrap_or(ERASED))
                .collect();
            assert_eq!(bytes, expected, "{offset:#x} + {len}");
        }
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
            address: Some(FlashAddress {
                value: 0x10,
                bits: 24,
            }),
            dummy_cycles: 8,
            data_out: &[],
        };
        flash.execute(&command, &mut byte).unwrap();
        assert_eq!(byte, [0xFF], "anything but 0x20 (XMC): BA §2.2");
    }

    #[test]
    fn a_command_without_address_or_data_is_refused_in_any_frame_but_its_own() {
        // `06` now opens every write: what a chip does with a write enable followed by an
        // address or a byte is not established, so the model does not set the latch on one.
        let at = |bits| Some(FlashAddress { value: 0x10, bits });
        let frames: [(Option<FlashAddress>, u32, &[u8]); 5] = [
            (at(24), 0, &[]),
            (at(32), 0, &[]),
            (None, 8, &[]),
            (None, 0, &[0x00]),
            (at(32), 8, &[1, 2, 3, 4]),
        ];
        for opcode in [0x06u8, 0x04, 0x05, 0x9F, 0xAB, 0x7A] {
            for latch in [false, true] {
                let mut flash = NorFlash::blank(FlashConfig::default());
                if latch {
                    run(&mut flash, 0x06, &mut []).unwrap();
                }
                for (address, dummy_cycles, data_out) in frames {
                    let command = FlashCommand {
                        opcode,
                        address,
                        dummy_cycles,
                        data_out,
                    };
                    let mut data_in = [0xA5u8; 3];
                    assert_eq!(
                        flash.execute(&command, &mut data_in),
                        Err(FlashRefusal::Form {
                            opcode,
                            address_bits: address.map_or(0, |a| a.bits),
                            dummy_cycles,
                            data_bytes: u32::try_from(data_out.len()).unwrap(),
                        }),
                        "{command:?}"
                    );
                    assert_eq!(data_in, [0xA5; 3], "nothing was clocked in: {command:?}");
                    assert_eq!(
                        status(&mut flash),
                        if latch { 0x02 } else { 0x00 },
                        "the latch is as it was: {command:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_sfdp_read_is_refused_in_any_frame_but_the_measured_one() {
        // BA §2.2: a 24-bit address, 8 dummy clocks, nothing sent after them.
        let mut flash = NorFlash::blank(FlashConfig::default());
        let at = |bits| Some(FlashAddress { value: 0x10, bits });
        let frames: [(Option<FlashAddress>, u32, &[u8]); 5] = [
            (None, 8, &[]),
            (at(32), 8, &[]),
            (at(24), 0, &[]),
            (at(24), 4, &[]),
            (at(24), 8, &[0]),
        ];
        for (address, dummy_cycles, data_out) in frames {
            let command = FlashCommand {
                opcode: 0x5A,
                address,
                dummy_cycles,
                data_out,
            };
            let mut byte = [0xA5u8];
            assert_eq!(
                flash.execute(&command, &mut byte),
                Err(FlashRefusal::Form {
                    opcode: 0x5A,
                    address_bits: address.map_or(0, |a| a.bits),
                    dummy_cycles,
                    data_bytes: u32::try_from(data_out.len()).unwrap(),
                }),
                "{command:?}"
            );
            assert_eq!(byte, [0xA5]);
        }
    }

    #[test]
    fn every_opcode_the_model_runs_has_one_frame_and_the_others_have_none() {
        let frame = |address_bits, dummy_cycles, data_out_min, data_out_max| {
            Some(FlashFrame {
                address_bits,
                dummy_cycles,
                data_out_min,
                data_out_max,
            })
        };
        let known = [
            (
                0xABu8,
                frame(0, 0, 0, 0),
                "no address, no dummy clock and no data out",
            ),
            (0x7A, frame(0, 0, 0, 0), ""),
            (0x9F, frame(0, 0, 0, 0), ""),
            (0x05, frame(0, 0, 0, 0), ""),
            (0x06, frame(0, 0, 0, 0), ""),
            (0x04, frame(0, 0, 0, 0), ""),
            (0xC7, frame(0, 0, 0, 0), ""),
            (0x60, frame(0, 0, 0, 0), ""),
            (
                0x5A,
                frame(24, 8, 0, 0),
                "a 24-bit address, 8 dummy clocks and no data out",
            ),
            (
                0x01,
                frame(0, 0, 1, 1),
                "no address, no dummy clock and 1 byte of data out",
            ),
            (
                0x02,
                frame(24, 0, 1, u32::MAX),
                "a 24-bit address, no dummy clock and 1 byte of data out or more",
            ),
            (
                0x20,
                frame(24, 0, 0, 0),
                "a 24-bit address, no dummy clock and no data out",
            ),
            (0xD8, frame(24, 0, 0, 0), ""),
            (0x03, frame(24, 0, 0, 0), ""),
            (0x0B, frame(24, 8, 0, 0), ""),
            (0x3B, frame(24, 8, 0, 0), ""),
            (
                0xBB,
                frame(24, 4, 0, 0),
                "a 24-bit address, 4 dummy clocks and no data out",
            ),
            (0x6B, frame(24, 8, 0, 0), ""),
            (
                0xEB,
                frame(24, 6, 0, 0),
                "a 24-bit address, 6 dummy clocks and no data out",
            ),
        ];
        for opcode in 0..=u8::MAX {
            let expected = known.iter().find(|(known, ..)| *known == opcode);
            assert_eq!(
                FlashFrame::of(opcode),
                expected.and_then(|&(_, frame, _)| frame),
                "{opcode:#04x}"
            );
            if let Some(&(_, Some(frame), words)) = expected
                && !words.is_empty()
            {
                assert_eq!(frame.to_string(), words, "{opcode:#04x}");
            }
            // A command is run exactly when it has a frame: the chip refuses the others by
            // their opcode, whatever comes with them.
            let mut flash = NorFlash::blank(FlashConfig::default());
            let refused_by_opcode =
                run(&mut flash, opcode, &mut []) == Err(FlashRefusal::Unmodeled { opcode });
            assert_eq!(refused_by_opcode, expected.is_none(), "{opcode:#04x}");
        }
    }

    #[test]
    fn opcodes_the_model_does_not_know_are_refused_by_opcode() {
        // What no measured boot sends: a second status register (`31`, `35`), a 32 KiB erase
        // (`52`: no caller in ESP-IDF, SF §C), reset (`66`, `99`), suspend (`75`), deep
        // power-down (`B9`), and the commands that take a 4-byte address (`12`, `13`, `0C`,
        // `BC`, `21`, `DC`: ESP-IDF sends them above 16 MiB, FW §A.2).
        let mut flash = NorFlash::with_image(vec![1, 2, 3, 4], FlashConfig::default()).unwrap();
        run(&mut flash, 0x06, &mut []).unwrap();
        for opcode in [
            0x31, 0x35, 0x52, 0x66, 0x99, 0x75, 0xB9, 0x12, 0x13, 0x0C, 0xBC, 0x21, 0xDC, 0x00,
            0xFF,
        ] {
            assert_eq!(
                run(&mut flash, opcode, &mut [0u8; 4]),
                Err(FlashRefusal::Unmodeled { opcode })
            );
            assert_eq!(
                send(&mut flash, opcode, 0, &[0]),
                Err(FlashRefusal::Unmodeled { opcode })
            );
        }
        assert_eq!(
            content(&flash, 0, 4),
            [1, 2, 3, 4],
            "the content did not change"
        );
        assert_eq!(status(&mut flash), 0x02, "nor did the latch");
    }

    #[test]
    fn a_refused_command_leaves_the_receive_buffer_as_it_was() {
        let mut flash = NorFlash::blank(FlashConfig::default());
        let mut data_in = [0x5A, 0xA5, 0x3C];
        assert_eq!(
            run(&mut flash, 0x35, &mut data_in),
            Err(FlashRefusal::Unmodeled { opcode: 0x35 })
        );
        assert_eq!(data_in, [0x5A, 0xA5, 0x3C], "nothing was clocked in");
    }

    /// A data read: `opcode` with a 24-bit address and `dummy_cycles`.
    fn read_at(
        flash: &mut NorFlash,
        opcode: u8,
        address: u32,
        dummy_cycles: u32,
        data_in: &mut [u8],
    ) -> Done {
        flash.execute(
            &FlashCommand {
                opcode,
                address: Some(FlashAddress {
                    value: address,
                    bits: 24,
                }),
                dummy_cycles,
                data_out: &[],
            },
            data_in,
        )
    }

    #[test]
    fn every_data_read_returns_the_content_from_its_address_with_its_own_dummy_clocks() {
        let image: Vec<u8> = (0u8..=255).cycle().take(0x400).collect();
        let mut flash = NorFlash::with_image(image.clone(), FlashConfig::default()).unwrap();
        for (opcode, dummy_cycles) in [
            (0x03u8, 0u32),
            (0x0B, 8),
            (0x3B, 8),
            (0xBB, 4),
            (0x6B, 8),
            (0xEB, 6),
        ] {
            let mut data = [0u8; 64];
            assert_eq!(
                read_at(&mut flash, opcode, 0x123, dummy_cycles, &mut data),
                Ok(None),
                "{opcode:#04x}"
            );
            assert_eq!(data, image[0x123..0x163], "{opcode:#04x}");
            // Another number of dummy clocks is another frame: the model does not shift the
            // data by guessing.
            let mut untouched = [0xA5u8; 4];
            assert_eq!(
                read_at(&mut flash, opcode, 0x123, dummy_cycles + 2, &mut untouched),
                Err(FlashRefusal::Form {
                    opcode,
                    address_bits: 24,
                    dummy_cycles: dummy_cycles + 2,
                    data_bytes: 0
                })
            );
            assert_eq!(untouched, [0xA5; 4]);
        }
        // What the image does not hold reads erased, inside the chip.
        let mut erased = [0u8; 4];
        read_at(&mut flash, 0xBB, 0x3FE, 4, &mut erased).unwrap();
        assert_eq!(erased, [0xFE, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn a_read_sees_what_a_program_wrote_before_it() {
        // SF §G item 6: on its second boot the application reads 0x9060 between two programs.
        let mut flash = NorFlash::blank(FlashConfig::default());
        write(&mut flash, 0x02, 0x9060, &[0x01, 0x14, 0x01]).unwrap();
        let mut entry = [0u8; 4];
        read_at(&mut flash, 0xBB, 0x9060, 4, &mut entry).unwrap();
        assert_eq!(entry, [0x01, 0x14, 0x01, 0xFF]);
    }

    #[test]
    fn a_read_that_reaches_past_the_end_of_the_chip_is_refused_and_reads_nothing() {
        let mut flash = NorFlash::with_image(vec![0x11; 16 << 20], FlashConfig::default()).unwrap();
        let mut last = [0u8; 2];
        assert_eq!(read_at(&mut flash, 0xBB, 0xFF_FFFE, 4, &mut last), Ok(None));
        assert_eq!(last, [0x11; 2], "the last two bytes of the chip");
        let mut past = [0xA5u8; 3];
        assert_eq!(
            read_at(&mut flash, 0xBB, 0xFF_FFFE, 4, &mut past),
            Err(FlashRefusal::OutOfRange {
                opcode: 0xBB,
                address: 0xFF_FFFE,
                len: 3
            }),
            "no roll-over to address 0 is guessed"
        );
        assert_eq!(past, [0xA5; 3]);
        // A smaller chip does not have the upper address bits.
        let small = FlashConfig::new(MIN_FLASH_BYTES, id_with(0x14)).unwrap();
        let mut flash = NorFlash::blank(small);
        for address in [0x10_0000, 0xFF_FFFF] {
            assert_eq!(
                read_at(&mut flash, 0x03, address, 0, &mut [0u8; 1]),
                Err(FlashRefusal::OutOfRange {
                    opcode: 0x03,
                    address,
                    len: 1
                })
            );
        }
        assert_eq!(
            read_at(&mut flash, 0x03, 0x0F_FFFF, 0, &mut [0u8; 1]),
            Ok(None)
        );
    }

    #[test]
    fn a_read_of_no_byte_is_judged_on_its_address_like_any_other() {
        // A transfer without a data-in phase reads no byte. Its address is inside the chip or
        // it is not: the first address past the end is refused, the last one inside is not.
        let small = FlashConfig::new(MIN_FLASH_BYTES, id_with(0x14)).unwrap();
        let mut flash = NorFlash::blank(small);
        assert_eq!(read_at(&mut flash, 0xBB, 0x0F_FFFF, 4, &mut []), Ok(None));
        assert_eq!(
            read_at(&mut flash, 0xBB, 0x10_0000, 4, &mut []),
            Err(FlashRefusal::OutOfRange {
                opcode: 0xBB,
                address: 0x10_0000,
                len: 0
            })
        );
        // One byte at the last address is the last byte; two reach past the end.
        assert_eq!(
            read_at(&mut flash, 0xBB, 0x0F_FFFF, 4, &mut [0u8; 1]),
            Ok(None)
        );
        assert_eq!(
            read_at(&mut flash, 0xBB, 0x0F_FFFF, 4, &mut [0u8; 2]),
            Err(FlashRefusal::OutOfRange {
                opcode: 0xBB,
                address: 0x0F_FFFF,
                len: 2
            })
        );
    }

    #[test]
    fn a_data_read_in_a_form_the_chip_does_not_know_is_refused_by_name() {
        let mut flash = NorFlash::with_image(vec![0x22; 64], FlashConfig::default()).unwrap();
        let at = |bits| Some(FlashAddress { value: 0, bits });
        // (address, data sent): no address, a 4-byte address, a 2-byte address, data after it.
        let frames: [(Option<FlashAddress>, &[u8]); 4] =
            [(None, &[]), (at(32), &[]), (at(16), &[]), (at(24), &[0])];
        for (address, data_out) in frames {
            let command = FlashCommand {
                opcode: 0xBB,
                address,
                dummy_cycles: 4,
                data_out,
            };
            let mut data_in = [0xA5u8; 4];
            assert_eq!(
                flash.execute(&command, &mut data_in),
                Err(FlashRefusal::Form {
                    opcode: 0xBB,
                    address_bits: address.map_or(0, |a| a.bits),
                    dummy_cycles: 4,
                    data_bytes: u32::try_from(data_out.len()).unwrap(),
                }),
                "{command:?}"
            );
            assert_eq!(data_in, [0xA5; 4]);
        }
    }

    #[test]
    fn a_program_needs_the_write_enable_latch_and_clears_it_when_it_completes() {
        let mut flash = NorFlash::blank(FlashConfig::default());
        // Without the latch a program is ignored, as the datasheet says (FW §C).
        assert_eq!(send(&mut flash, 0x02, 0x9000, &[0x12]), Ok(None));
        assert_eq!(content(&flash, 0x9000, 1), [0xFF]);
        assert_eq!(
            write(&mut flash, 0x02, 0x9000, &[0x12]),
            Ok(Some(FlashWrite::Program {
                address: 0x9000,
                data: vec![0x12]
            }))
        );
        assert_eq!(content(&flash, 0x9000, 1), [0x12]);
        assert_eq!(
            status(&mut flash),
            0,
            "WEL reads 0 after a program (FW key result 5)"
        );
        // The latch served once: a second program needs a second write enable.
        assert_eq!(send(&mut flash, 0x02, 0x9001, &[0x34]), Ok(None));
        assert_eq!(content(&flash, 0x9001, 1), [0xFF]);
    }

    #[test]
    fn a_program_only_clears_bits() {
        // NVS rewrites its bitmap word in place: `FE`, then `FA` (SF §B.2).
        let mut flash = NorFlash::blank(FlashConfig::default());
        write(&mut flash, 0x02, 0x9020, &[0xFE, 0xFF, 0xFF, 0xFF]).unwrap();
        write(&mut flash, 0x02, 0x9020, &[0xFA, 0xFF, 0xFF, 0xFF]).unwrap();
        assert_eq!(content(&flash, 0x9020, 4), [0xFA, 0xFF, 0xFF, 0xFF]);
        // No program sets a bit again: `FA AND FF`, then `FA AND 0F`.
        write(&mut flash, 0x02, 0x9020, &[0xFF]).unwrap();
        assert_eq!(content(&flash, 0x9020, 1), [0xFA]);
        write(&mut flash, 0x02, 0x9020, &[0x0F]).unwrap();
        assert_eq!(content(&flash, 0x9020, 1), [0x0A]);
    }

    #[test]
    fn a_program_wraps_inside_its_256_byte_page() {
        let mut flash = NorFlash::blank(FlashConfig::default());
        write(&mut flash, 0x02, 0x10FE, &[1, 2, 3, 4]).unwrap();
        assert_eq!(content(&flash, 0x10FE, 2), [1, 2]);
        assert_eq!(
            content(&flash, 0x1000, 2),
            [3, 4],
            "the page counter wraps to the start of the page"
        );
        assert_eq!(
            content(&flash, 0x1100, 2),
            [0xFF, 0xFF],
            "the next page is not reached"
        );
    }

    #[test]
    fn of_more_than_a_page_of_data_the_last_256_bytes_are_programmed() {
        let mut flash = NorFlash::blank(FlashConfig::default());
        // 300 bytes that cycle through 0 to 254, so none of them is `FF`: 44 are dropped.
        let data: Vec<u8> = (0u8..=254).cycle().take(300).collect();
        assert_eq!(
            write(&mut flash, 0x02, 0x2010, &data),
            Ok(Some(FlashWrite::Program {
                address: 0x203C,
                data: data[44..].to_vec()
            })),
            "the first byte kept, byte 44, lands at 0x10 + 44"
        );
        assert_eq!(content(&flash, 0x203C, 1), [data[44]]);
        assert_eq!(
            content(&flash, 0x2000, 1),
            [data[240]],
            "0x10 + 240 wraps to 0"
        );
        assert_eq!(
            content(&flash, 0x203B, 1),
            [data[299]],
            "the last byte ends the page turn"
        );
        assert!(
            content(&flash, 0x2000, 256)
                .iter()
                .all(|&byte| byte != 0xFF),
            "every position of the page was programmed once"
        );
        assert_eq!(content(&flash, 0x2100, 1), [0xFF]);
    }

    /// The `otadata` entry the stock bootloader writes on the first boot of an OTA layout
    /// (FW §E, first row): sequence 1, a blank label, state valid, CRC.
    fn otadata_entry() -> Vec<u8> {
        let mut entry = vec![0xFF; 32];
        entry[..4].copy_from_slice(&[1, 0, 0, 0]);
        entry[24..28].copy_from_slice(&[2, 0, 0, 0]);
        entry[28..].copy_from_slice(&[0x9A, 0x98, 0x43, 0x47]);
        entry
    }

    #[test]
    fn the_otadata_update_of_the_bootloader_leaves_the_measured_bytes() {
        // FW §E: the ROM's sector erase at 0xE000, then its 32-byte program at 0xE000.
        let mut flash = NorFlash::with_image(vec![0x55; 0x1_0000], FlashConfig::default()).unwrap();
        assert_eq!(
            write(&mut flash, 0x20, 0xE000, &[]),
            Ok(Some(FlashWrite::Erase {
                start: 0xE000,
                len: 0x1000
            }))
        );
        assert_eq!(
            write(&mut flash, 0x02, 0xE000, &otadata_entry()),
            Ok(Some(FlashWrite::Program {
                address: 0xE000,
                data: otadata_entry()
            }))
        );
        assert_eq!(content(&flash, 0xE000, 32), otadata_entry());
        assert_eq!(
            content(&flash, 0xE020, 0xFE0),
            vec![0xFF; 0xFE0],
            "the rest of the sector is erased"
        );
        assert_eq!(content(&flash, 0xDFFC, 4), [0x55; 4], "the sector before");
        assert_eq!(content(&flash, 0xF000, 4), [0x55; 4], "the sector after");
        assert_eq!(flash.modified_sectors(), 1);
    }

    #[test]
    fn the_nvs_programs_of_a_first_boot_leave_the_measured_rows() {
        // SF §B.2, boot 1: five programs on an erased partition, no erase. The bitmap word at
        // 0x9020 is programmed twice.
        let header = format!("feffffff 00000000 fe {} 842dbab9", "ff".repeat(19));
        let namespace = "000101ff 09a95007 73746f72 61676500 00000000 00000000 01ffffff ffffffff";
        let counter = "011401ff 81a8ff42 72657374 6172745f 636f756e 74657200 01000000 ffffffff";
        let programs = [
            (0x9000, hex(&header)),
            (0x9040, hex(namespace)),
            (0x9020, hex("feffffff")),
            (0x9060, hex(counter)),
            (0x9020, hex("faffffff")),
        ];
        let mut flash = NorFlash::blank(FlashConfig::default());
        for (address, data) in &programs {
            assert_eq!(
                write(&mut flash, 0x02, *address, data),
                Ok(Some(FlashWrite::Program {
                    address: *address,
                    data: data.clone()
                }))
            );
        }
        let bitmap = format!("faffffff {}", "ff".repeat(28));
        let rows = [hex(&header), hex(&bitmap), hex(namespace), hex(counter)].concat();
        assert_eq!(
            content(&flash, 0x9000, 0x80),
            rows,
            "the four rows of SF §B.2"
        );
        assert_eq!(content(&flash, 0x9080, 0x80), vec![0xFF; 0x80]);
        assert_eq!(flash.modified_sectors(), 1, "all in flash sector 9");
    }

    #[test]
    fn a_sector_erase_and_a_block_erase_set_their_whole_unit_and_nothing_else() {
        let mut flash = NorFlash::with_image(vec![0u8; 0x4_0000], FlashConfig::default()).unwrap();
        // The low address bits are ignored (FW §G): 0x12345 is in the sector of 0x12000.
        assert_eq!(
            write(&mut flash, 0x20, 0x1_2345, &[]),
            Ok(Some(FlashWrite::Erase {
                start: 0x1_2000,
                len: 0x1000
            }))
        );
        assert_eq!(content(&flash, 0x1_1FFF, 1), [0]);
        assert_eq!(content(&flash, 0x1_2000, 0x1000), vec![0xFF; 0x1000]);
        assert_eq!(content(&flash, 0x1_3000, 1), [0]);
        assert_eq!(flash.modified_sectors(), 1);
        assert_eq!(
            write(&mut flash, 0xD8, 0x2_FFFF, &[]),
            Ok(Some(FlashWrite::Erase {
                start: 0x2_0000,
                len: 0x1_0000
            }))
        );
        assert_eq!(content(&flash, 0x1_FFFF, 1), [0]);
        assert_eq!(content(&flash, 0x2_0000, 0x1_0000), vec![0xFF; 0x1_0000]);
        assert_eq!(content(&flash, 0x3_0000, 1), [0]);
        assert_eq!(
            flash.modified_sectors(),
            17,
            "one sector, then the sixteen of a block"
        );
        assert_eq!(status(&mut flash), 0, "the latch is clear after an erase");
    }

    #[test]
    fn a_chip_erase_erases_everything_under_either_opcode() {
        for opcode in [0xC7, 0x60] {
            let mut flash =
                NorFlash::with_image(vec![0u8; 0x2800], FlashConfig::default()).unwrap();
            run(&mut flash, 0x06, &mut []).unwrap();
            assert_eq!(
                run(&mut flash, opcode, &mut []),
                Ok(Some(FlashWrite::Erase {
                    start: 0,
                    len: 16 << 20
                })),
                "{opcode:#04x}"
            );
            assert_eq!(content(&flash, 0, 0x2800), vec![0xFF; 0x2800]);
            assert_eq!(
                flash.modified_sectors(),
                3,
                "the three sectors that held something"
            );
            assert_eq!(status(&mut flash), 0);
        }
    }

    #[test]
    fn an_erase_without_the_latch_changes_nothing() {
        let mut flash = NorFlash::with_image(vec![0u8; 0x2_0000], FlashConfig::default()).unwrap();
        assert_eq!(send(&mut flash, 0x20, 0, &[]), Ok(None));
        assert_eq!(send(&mut flash, 0xD8, 0x1_0000, &[]), Ok(None));
        assert_eq!(run(&mut flash, 0xC7, &mut []), Ok(None));
        assert_eq!(run(&mut flash, 0x60, &mut []), Ok(None));
        assert_eq!(content(&flash, 0, 0x2_0000), vec![0u8; 0x2_0000]);
        assert_eq!((flash.generation(), flash.modified_sectors()), (0, 0));
    }

    #[test]
    fn an_operation_that_changes_no_byte_is_performed_and_modifies_nothing() {
        // An erase of an erased sector and a program of `FF` are operations of the journal; the
        // content, its generation and the modified sectors do not move.
        let mut flash = NorFlash::blank(FlashConfig::default());
        assert!(matches!(
            write(&mut flash, 0x20, 0x9000, &[]),
            Ok(Some(FlashWrite::Erase { .. }))
        ));
        assert!(matches!(
            write(&mut flash, 0x02, 0x9000, &[0xFF, 0xFF]),
            Ok(Some(FlashWrite::Program { .. }))
        ));
        assert_eq!((flash.generation(), flash.modified_sectors()), (0, 0));
        write(&mut flash, 0x02, 0x9000, &[0xFE]).unwrap();
        let after_a_change = flash.generation();
        assert_ne!(after_a_change, 0);
        assert_eq!(flash.modified_sectors(), 1);
        // The same bytes again: nothing changes.
        write(&mut flash, 0x02, 0x9000, &[0xFE]).unwrap();
        assert_eq!(flash.generation(), after_a_change);
        write(&mut flash, 0x20, 0x9000, &[]).unwrap();
        assert_ne!(
            flash.generation(),
            after_a_change,
            "the erase changed a byte"
        );
        assert_eq!(flash.modified_sectors(), 1, "the same sector");
        // The image now reaches 0x9100, and what it holds below sector 9 is erased: erasing
        // that again, or sector 9 a second time, changes no byte.
        let erased = flash.generation();
        write(&mut flash, 0x20, 0x8000, &[]).unwrap();
        write(&mut flash, 0x20, 0x9000, &[]).unwrap();
        write(&mut flash, 0xD8, 0x0000, &[]).unwrap();
        assert_eq!((flash.generation(), flash.modified_sectors()), (erased, 1));
    }

    #[test]
    fn the_chip_says_which_ranges_hold_something_an_erase_would_change() {
        let size = FlashConfig::default().size();
        let mut flash = NorFlash::blank(FlashConfig::default());
        assert!(!flash.holds_data(0, size), "a blank chip");
        // One bit cleared at 0x12345, in sector 0x12: the answer is by sector.
        write(&mut flash, 0x02, 0x1_2345, &[0xFE]).unwrap();
        for (start, len, holds) in [
            (0x1_2345, 1, true),
            (0x1_2000, 1, true),          // the first byte of the sector
            (0x1_2FFF, 1, true),          // its last byte
            (0x1_1FFF, 1, false),         // the sector before
            (0x1_3000, 0x1000, false),    // the sector after
            (0x1_1FFF, 2, true),          // reaches the sector by its last byte
            (0x1_0000, 0x1_0000, true),   // its 64 KiB block
            (0, 0x1_0000, false),         // the block before
            (0x2_0000, 0xFE_0000, false), // everything after its block
            (0, size, true),
            (0, u32::MAX, true),
            (0x1_2345, 0, false), // no byte
            // Numbers no chip has reach no sector.
            (size, 1, false),
            (u32::MAX, u32::MAX, false),
            (u32::MAX, 0, false),
        ] {
            assert_eq!(flash.holds_data(start, len), holds, "{start:#x} {len:#x}");
        }
        // Erased, the sector holds nothing again.
        write(&mut flash, 0x20, 0x1_2000, &[]).unwrap();
        assert!(!flash.holds_data(0, size));
        // The last byte of the chip, and a program that changes nothing.
        write(&mut flash, 0x02, 0xFF_FFFF, &[0x7F]).unwrap();
        write(&mut flash, 0x02, 0x80_0000, &[0xFF]).unwrap();
        assert!(flash.holds_data(0xFF_F000, 0x1000));
        assert!(flash.holds_data(0xFF_0000, 0x1_0000));
        assert!(
            !flash.holds_data(0, 0xFF_F000),
            "everything below the last sector"
        );
        // A chip built from an image knows what the image holds: sectors 63 and 64 are in two
        // words of the set.
        let mut image = vec![0xFF; 0x4_1000];
        image[0x3_F800] = 0;
        image[0x4_0800] = 0;
        let flash = NorFlash::with_image(image, FlashConfig::default()).unwrap();
        for (start, len, holds) in [
            (0x3_F000, 0x1000, true),
            (0x4_0000, 0x1000, true),
            (0x3_E000, 0x1000, false),
            (0x4_1000, 0x1000, false),
            (0x3_E000, 0x2000, true),   // ends in sector 63
            (0x4_0000, 0x2000, true),   // starts in sector 64
            (0x3_0000, 0x1_0000, true), // block 3
            (0, 0x3_F000, false),
            (0x4_1000, size - 0x4_1000, false),
        ] {
            assert_eq!(flash.holds_data(start, len), holds, "{start:#x} {len:#x}");
        }
    }

    #[test]
    fn a_range_taken_out_of_a_set_of_sectors_is_that_range_in_order_and_nothing_else() {
        const SECTORS: u32 = 4096;
        let mut n = crate::test_support::Lcg::new(0x5EC7_0125);
        for round in 0..3000 {
            let mut set = SectorSet::EMPTY;
            let mut plain = vec![false; SECTORS as usize];
            for _ in 0..n.below(300) {
                let sector = n.below(SECTORS);
                set.insert(sector);
                plain[sector as usize] = true;
            }
            // Every shape of range: empty, one sector, inside a word, across words, the whole
            // set, and ranges that start or end past the last sector.
            let first = n.below(SECTORS + 100);
            let count = match n.below(5) {
                0 => 0,
                1 => 1,
                2 => n.below(70),
                3 => n.below(SECTORS + 200),
                _ => SECTORS - first.min(SECTORS),
            };
            let range = first..first.saturating_add(count);
            let mut taken = Vec::new();
            set.remove_range(first, count, |sector| taken.push(sector));
            let expected: Vec<u32> = range
                .clone()
                .filter(|&sector| plain.get(sector as usize) == Some(&true))
                .collect();
            assert_eq!(taken, expected, "round {round}: {range:?}");
            for sector in 0..SECTORS {
                assert_eq!(
                    set.contains(sector),
                    plain[sector as usize] && !range.contains(&sector),
                    "round {round}: sector {sector} after {range:?}"
                );
            }
            let left = plain.iter().filter(|&&sector| sector).count() - expected.len();
            assert_eq!(set.len() as usize, left, "round {round}");
        }
        // A sector no chip has is in no set, and numbers no erase produces do not overflow.
        let mut set = SectorSet::EMPTY;
        set.insert(SECTORS);
        set.insert(u32::MAX);
        assert_eq!(set, SectorSet::EMPTY);
        set.insert(SECTORS - 1);
        set.remove_range(u32::MAX, u32::MAX, |_| panic!("nothing is there"));
        set.remove_range(SECTORS, u32::MAX, |_| panic!("nothing is there"));
        assert!(set.contains(SECTORS - 1) && !set.contains(SECTORS));
        let mut taken = Vec::new();
        set.remove_range(0, u32::MAX, |sector| taken.push(sector));
        assert_eq!((taken, set), (vec![SECTORS - 1], SectorSet::EMPTY));
    }

    /// A sector of erased bytes, to compare a sector with.
    const BLANK_SECTOR: [u8; 4096] = [0xFF; 4096];

    /// The chip written the plain way, from the rules of report FW §C alone: every byte of the
    /// chip is held, and an erase looks at every byte of its unit. What [`NorFlash`] must hold
    /// and count after any sequence of writes.
    struct PlainChip {
        content: Vec<u8>,
        modified: Vec<bool>,
        generation: u64,
    }

    impl PlainChip {
        fn new(image: &[u8], size: usize) -> PlainChip {
            let mut content = vec![0xFF; size];
            content[..image.len()].copy_from_slice(image);
            PlainChip {
                content,
                modified: vec![false; size / 4096],
                generation: 0,
            }
        }

        fn program(&mut self, address: usize, data: &[u8]) -> FlashWrite {
            let page = address / 256 * 256;
            let dropped = data.len().saturating_sub(256);
            let first = (address - page + dropped) % 256;
            let mut changed = false;
            for (position, &byte) in (first..).zip(&data[dropped..]) {
                let cell = &mut self.content[page + position % 256];
                changed |= *cell & byte != *cell;
                *cell &= byte;
            }
            if changed {
                self.modified[page / 4096] = true;
                self.generation += 1;
            }
            FlashWrite::Program {
                address: u32::try_from(page + first).unwrap(),
                data: data[dropped..].to_vec(),
            }
        }

        fn erase(&mut self, address: usize, unit: usize) -> FlashWrite {
            let start = address / unit * unit;
            for sector in (start..start + unit).step_by(4096) {
                let bytes = &mut self.content[sector..sector + 4096];
                if bytes != BLANK_SECTOR {
                    bytes.fill(0xFF);
                    self.modified[sector / 4096] = true;
                    self.generation += 1;
                }
            }
            FlashWrite::Erase {
                start: u32::try_from(start).unwrap(),
                len: u32::try_from(unit).unwrap(),
            }
        }
    }

    /// Asserts that `flash` holds, counts and knows of its sectors what `plain` does.
    fn assert_same_chip(flash: &NorFlash, plain: &PlainChip, held: &mut Vec<u8>, step: &str) {
        held.resize(plain.content.len(), 0);
        flash.read(0, held);
        if *held != plain.content {
            let at = held.iter().zip(&plain.content).position(|(a, b)| a != b);
            panic!("{step}: the content differs at {at:x?}");
        }
        assert_eq!(flash.generation(), plain.generation, "{step}");
        let modified = plain.modified.iter().filter(|&&sector| sector).count();
        assert_eq!(
            usize::try_from(flash.modified_sectors()).unwrap(),
            modified,
            "{step}"
        );
        // The set of sectors an erase visits: exactly the ones that hold a byte to erase.
        for (sector, bytes) in (0u32..).zip(plain.content.chunks(4096)) {
            assert_eq!(
                flash.not_blank.contains(sector),
                bytes != BLANK_SECTOR,
                "{step}: sector {sector:#x}"
            );
        }
    }

    #[test]
    fn a_long_random_sequence_of_writes_leaves_what_the_plain_chip_holds_and_counts() {
        const SIZE: usize = MIB;
        let small = FlashConfig::new(MIN_FLASH_BYTES, id_with(0x14)).unwrap();
        let mut n = crate::test_support::Lcg::new(0xF1A5_0001);
        // An image that ends inside a sector: blank sectors, sectors of zeroes, sectors with a
        // single byte to erase, sectors of anything.
        let mut image = vec![0xFF; 150 * 4096 + 1234];
        for sector in image.chunks_mut(4096) {
            match n.below(4) {
                0 => {}
                1 => sector.fill(0x00),
                2 => {
                    let at = n.below(u32::try_from(sector.len()).unwrap()) as usize;
                    sector[at] = 0x7F;
                }
                _ => sector.fill_with(|| n.next_u8()),
            }
        }
        let mut plain = PlainChip::new(&image, SIZE);
        let mut flash = NorFlash::with_image(image, small).unwrap();
        let mut held = Vec::new();
        assert_same_chip(&flash, &plain, &mut held, "the image");
        for step in 0..4000 {
            // Three addresses out of four fall in eight places of the chip, so that programs
            // and erases meet; the fourth is anywhere, the end of the chip included.
            let address = if n.below(4) == 0 {
                n.below(MIN_FLASH_BYTES)
            } else {
                (n.below(8) * 0x1_F000 + n.below(0x2400)) % MIN_FLASH_BYTES
            };
            let at = address as usize;
            let (opcode, data) = match n.below(20) {
                0 => (0xC7u8, Vec::new()),
                1..=2 => (0xD8, Vec::new()),
                3..=7 => (0x20, Vec::new()),
                // One program in four is of `FF` only: it changes nothing.
                kind => {
                    let len = 1 + n.below(300) as usize;
                    let mut data = vec![0xFF; len];
                    if kind % 4 != 0 {
                        data.fill_with(|| n.next_u8() | n.next_u8());
                    }
                    (0x02, data)
                }
            };
            // One command in sixteen comes without its write enable, and is ignored.
            let enabled = n.below(16) != 0;
            if enabled {
                run(&mut flash, 0x06, &mut []).unwrap();
            }
            let done = match opcode {
                0xC7 => run(&mut flash, 0xC7, &mut []),
                opcode => send(&mut flash, opcode, address, &data),
            };
            let expected = enabled.then(|| match opcode {
                0xC7 => plain.erase(0, SIZE),
                0xD8 => plain.erase(at, 0x1_0000),
                0x20 => plain.erase(at, 0x1000),
                _ => plain.program(at, &data),
            });
            let what = format!("step {step}: {opcode:#04x} at {address:#x}");
            assert_eq!(done, Ok(expected), "{what}");
            assert_same_chip(&flash, &plain, &mut held, &what);
        }
        assert!(plain.generation > 1000, "the sequence changed the content");
    }

    #[test]
    fn an_erase_costs_what_it_erases_and_not_the_size_of_the_chip() {
        // A firmware loop of two commands: one byte programmed in the last page of a 16 MiB
        // chip, then the chip erased. Each round changes one byte and changes it back; it must
        // not read or fill 16 MiB to do so. With that cost, a thousand rounds take minutes in
        // this build; they are given thirty seconds.
        let (generation, modified) = crate::test_support::within(
            std::time::Duration::from_secs(30),
            "1,000 rounds of a one-byte program and a chip erase did not end in 30 s: an erase costs the size of the chip",
            || {
                let mut flash = NorFlash::blank(FlashConfig::default());
                for _ in 0..1000 {
                    write(&mut flash, 0x02, 0xFF_FFFF, &[0x00]).unwrap();
                    run(&mut flash, 0x06, &mut []).unwrap();
                    run(&mut flash, 0xC7, &mut []).unwrap();
                }
                assert_eq!(content(&flash, 0xFF_FF00, 256), vec![0xFF; 256]);
                // The image keeps the length the first program gave it: emptied by the erase,
                // it would be filled again, 16 MiB of it, by the program of every round.
                assert!(
                    format!("{flash:?}").contains("image_bytes: 16777216"),
                    "{flash:?}"
                );
                (flash.generation(), flash.modified_sectors())
            },
        );
        assert_eq!(
            (generation, modified),
            (2000, 1),
            "each program and each erase changed one byte of the last sector"
        );
    }

    #[test]
    fn a_sector_changed_and_changed_back_still_counts_as_modified() {
        let image = vec![0x12, 0x34, 0x56];
        let mut flash = NorFlash::with_image(image.clone(), FlashConfig::default()).unwrap();
        write(&mut flash, 0x20, 0, &[]).unwrap();
        write(&mut flash, 0x02, 0, &image).unwrap();
        assert_eq!(content(&flash, 0, 3), image, "the content it started from");
        assert_eq!(
            flash.modified_sectors(),
            1,
            "written with other bytes at some point"
        );
        assert_eq!(
            flash.generation(),
            2,
            "an erase and a program changed bytes"
        );
    }

    #[test]
    fn the_status_register_stores_its_writable_bits_and_its_write_needs_and_clears_the_latch() {
        let mut flash = NorFlash::blank(FlashConfig::default());
        assert_eq!(send_data(&mut flash, 0x01, &[0x40]), Ok(None), "no latch");
        assert_eq!(status(&mut flash), 0);
        run(&mut flash, 0x06, &mut []).unwrap();
        assert_eq!(
            send_data(&mut flash, 0x01, &[0xFF]),
            Ok(Some(FlashWrite::Status { value: 0xFC })),
            "WIP and WEL are not stored"
        );
        assert_eq!(
            status(&mut flash),
            0xFC,
            "the latch is clear, the rest reads back"
        );
        run(&mut flash, 0x06, &mut []).unwrap();
        assert_eq!(status(&mut flash), 0xFE);
        send_data(&mut flash, 0x01, &[0x00]).unwrap();
        assert_eq!(status(&mut flash), 0);
        // The status register is not content: the four status writes changed no byte of it.
        assert_eq!((flash.generation(), flash.modified_sectors()), (0, 0));
    }

    #[test]
    fn a_program_or_an_erase_under_a_block_protect_bit_is_refused_by_name() {
        for bp in [0x04u8, 0x08, 0x10, 0x20, 0x3C] {
            let mut flash = NorFlash::with_image(vec![0x0F; 16], FlashConfig::default()).unwrap();
            run(&mut flash, 0x06, &mut []).unwrap();
            send_data(&mut flash, 0x01, &[bp]).unwrap();
            let protected = |opcode| {
                Err(FlashRefusal::Protected {
                    opcode,
                    status: bp | 0x02,
                })
            };
            // Without the latch a chip ignores the write, protected or not: nothing to report.
            assert_eq!(status(&mut flash), bp, "the status write cleared the latch");
            assert_eq!(send(&mut flash, 0x02, 0, &[0x00]), Ok(None));
            assert_eq!(send(&mut flash, 0x20, 0, &[]), Ok(None));
            assert_eq!(run(&mut flash, 0xC7, &mut []), Ok(None));
            // One write enable for the five commands: a refusal changes nothing, the latch
            // included, so each of them finds it set.
            run(&mut flash, 0x06, &mut []).unwrap();
            for (opcode, data) in [(0x02u8, &[0x00u8][..]), (0x20, &[]), (0xD8, &[])] {
                assert_eq!(send(&mut flash, opcode, 0, data), protected(opcode));
                assert_eq!(
                    status(&mut flash),
                    bp | 0x02,
                    "{opcode:#04x} kept the latch"
                );
            }
            for opcode in [0xC7, 0x60] {
                assert_eq!(run(&mut flash, opcode, &mut []), protected(opcode));
                assert_eq!(
                    status(&mut flash),
                    bp | 0x02,
                    "{opcode:#04x} kept the latch"
                );
            }
            assert_eq!(content(&flash, 0, 16), [0x0F; 16], "status {bp:#04x}");
            assert_eq!((flash.generation(), flash.modified_sectors()), (0, 0));
            // The status write is how the protection is lifted: it is not protected itself.
            send_data(&mut flash, 0x01, &[0x00]).unwrap();
            assert!(write(&mut flash, 0x02, 0, &[0x00]).unwrap().is_some());
            assert_eq!(content(&flash, 0, 1), [0x00]);
        }
        // Quad enable and SRWD protect nothing.
        let mut flash = NorFlash::blank(FlashConfig::default());
        run(&mut flash, 0x06, &mut []).unwrap();
        send_data(&mut flash, 0x01, &[0xC0]).unwrap();
        assert!(write(&mut flash, 0x02, 0, &[0x00]).unwrap().is_some());
    }

    #[test]
    fn a_write_past_the_end_of_a_smaller_chip_is_refused_by_name() {
        let small = FlashConfig::new(MIN_FLASH_BYTES, id_with(0x14)).unwrap();
        let mut flash = NorFlash::blank(small);
        for (opcode, data, len) in [
            (0x02u8, &[0u8, 1][..], 2u32),
            (0x20, &[][..], 0x1000),
            (0xD8, &[][..], 0x1_0000),
        ] {
            for address in [0x10_0000, 0xFF_FFFF] {
                assert_eq!(
                    write(&mut flash, opcode, address, data),
                    Err(FlashRefusal::OutOfRange {
                        opcode,
                        address,
                        len
                    })
                );
                assert_eq!(status(&mut flash), 0x02, "a refusal keeps the latch");
            }
        }
        // The latch a refusal kept is the one the next command finds: no write enable here.
        assert_eq!(
            send(&mut flash, 0x20, 0x10_0000, &[]),
            Err(FlashRefusal::OutOfRange {
                opcode: 0x20,
                address: 0x10_0000,
                len: 0x1000
            })
        );
        assert_eq!((flash.generation(), flash.modified_sectors()), (0, 0));
        run(&mut flash, 0x04, &mut []).unwrap();
        // The latch is judged before the range, for a program and for an erase alike.
        for (opcode, data) in [(0x02u8, &[0u8][..]), (0x20, &[]), (0xD8, &[])] {
            assert_eq!(
                send(&mut flash, opcode, 0x10_0000, data),
                Ok(None),
                "{opcode:#04x}: without the latch a chip ignores the command wherever it points"
            );
        }
        // The last byte of the chip is inside it: the image grows to the size of the chip and
        // no further.
        write(&mut flash, 0x02, 0x0F_FFFF, &[0x00, 0x11]).unwrap();
        assert_eq!(content(&flash, 0x0F_FFFF, 1), [0x00]);
        assert_eq!(
            content(&flash, 0x0F_FF00, 1),
            [0x11],
            "wrapped inside the last page"
        );
        assert!(
            format!("{flash:?}").contains("image_bytes: 1048576"),
            "{flash:?}"
        );
    }

    #[test]
    fn a_write_command_in_a_form_the_chip_does_not_know_is_refused_by_name() {
        let at = |bits| Some(FlashAddress { value: 0, bits });
        // (opcode, address, dummy clocks, data): each differs from the form of its command.
        let frames: [(u8, Option<FlashAddress>, u32, &[u8]); 17] = [
            (0x02, None, 0, &[0]),    // a program without an address
            (0x02, at(32), 0, &[0]),  // with a 4-byte address
            (0x02, at(16), 0, &[0]),  // with a 2-byte address
            (0x02, at(24), 8, &[0]),  // with dummy clocks
            (0x02, at(24), 0, &[]),   // without a byte
            (0x20, None, 0, &[]),     // an erase without an address
            (0x20, at(24), 0, &[0]),  // with data
            (0x20, at(24), 8, &[]),   // with dummy clocks
            (0xD8, at(32), 0, &[]),   // a block erase with a 4-byte address
            (0xD8, at(24), 0, &[0]),  // with data
            (0xC7, at(24), 0, &[]),   // a chip erase with an address
            (0xC7, None, 8, &[]),     // with dummy clocks
            (0x60, None, 0, &[0]),    // with data
            (0x01, None, 0, &[0, 0]), // a status write of two bytes
            (0x01, None, 0, &[]),     // of no byte
            (0x01, at(24), 0, &[0]),  // with an address
            (0x01, None, 8, &[0]),    // with dummy clocks
        ];
        // The frame is judged before the write-enable latch: a frame the model does not know is
        // refused with the latch set and without it, and the latch stays as it was.
        for latch in [0x02u8, 0x00] {
            let mut flash = NorFlash::with_image(vec![0x0F; 16], FlashConfig::default()).unwrap();
            if latch != 0 {
                run(&mut flash, 0x06, &mut []).unwrap();
            }
            for (opcode, address, dummy_cycles, data_out) in frames {
                let command = FlashCommand {
                    opcode,
                    address,
                    dummy_cycles,
                    data_out,
                };
                assert_eq!(
                    flash.execute(&command, &mut []),
                    Err(FlashRefusal::Form {
                        opcode,
                        address_bits: address.map_or(0, |a| a.bits),
                        dummy_cycles,
                        data_bytes: u32::try_from(data_out.len()).unwrap(),
                    }),
                    "{command:?} with status {latch:#04x}"
                );
                assert_eq!(status(&mut flash), latch, "the latch is as it was");
            }
            assert_eq!(content(&flash, 0, 16), [0x0F; 16]);
            assert_eq!((flash.generation(), flash.modified_sectors()), (0, 0));
        }
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
        assert!(format!("{flash:?}").len() < 300);
    }

    #[test]
    fn the_journal_keeps_the_first_entries_nobody_took_and_counts_the_others() {
        let mut journal = FlashJournal::default();
        let entry = |start| FlashWrite::Erase {
            start,
            len: SECTOR_BYTES,
        };
        for start in 0..u32::try_from(JOURNAL_CAPACITY).unwrap() + 6 {
            journal.record(entry(start));
        }
        assert_eq!(journal.dropped(), 6);
        let taken = journal.take();
        assert_eq!(taken.len(), JOURNAL_CAPACITY);
        assert_eq!(
            (taken.first(), taken.last()),
            (Some(&entry(0)), Some(&entry(1023))),
            "the oldest are kept"
        );
        // Once taken, there is room again; the count of what was lost stays.
        journal.record(entry(7));
        assert_eq!(journal.take(), [entry(7)]);
        assert_eq!(journal.take(), []);
        assert_eq!(journal.dropped(), 6);
    }
}
