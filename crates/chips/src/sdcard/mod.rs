//! An SD memory card in SPI mode, at the level of the bytes on its bus (design `SP2e`; PLS 9.10
//! chapter 7, research `sp2e-sd-card.md`).
//!
//! The card does not know the `SoC`: it is selected and deselected, and exchanges one byte for
//! one byte while selected, full duplex: the byte it shifts out was decided before the byte
//! shifted in is read, so the answer to a command comes after it. Time is the device's, an
//! instant in cycles of the time base given at construction, passed with every call; the card
//! never reads a clock and nothing on its lines changes unless the master clocks it, so it is
//! never advanced on its own.
//!
//! What it keeps its blocks in is a [`BlockStorage`]: in memory, or an [`Overlay`] over a source
//! that is only read (an image file of the host, the blocks of a session). [`SdSlot`] holds a card
//! or none: an empty slot never drives MISO.
mod crc;
mod diagnostic;
mod registers;
mod storage;
#[cfg(test)]
mod tests;

use std::collections::VecDeque;

use emu_core::{Cycles, Frequency};

pub use crc::{crc7, crc7_byte, crc16};
pub use diagnostic::{SdDiagnostic, SdDiagnosticKind};
pub use registers::{
    CARD_COMMAND_CLASSES, Capacity, CapacityError, CardKind, Cid, MAX_C_SIZE, MAX_CAPACITY_BYTES,
    SCR, SD_STATUS, SDHC_UNIT_BYTES, SDSC_MAX_BYTES, get as register_bits, ocr, switch_status,
};
pub use storage::{
    BLOCK_BYTES, Block, BlockSource, BlockStorage, Changes, MemoryStorage, Overlay, SparseBlocks,
    StorageError,
};

/// The diagnostics kept until [`SdCard::take_diagnostics`]; past it they are counted, not kept.
pub const MAX_DIAGNOSTICS: usize = 1024;

/// The bits of R1 (PLS Figure 7-9).
const R1_IDLE: u8 = 0x01;
const R1_ERASE_RESET: u8 = 0x02;
const R1_ILLEGAL: u8 = 0x04;
const R1_CRC: u8 = 0x08;
const R1_ERASE_SEQUENCE: u8 = 0x10;
const R1_ADDRESS: u8 = 0x20;
const R1_PARAMETER: u8 = 0x40;

/// The bits of the second byte of R2 (PLS Figure 7-10).
const R2_WP_ERASE_SKIP: u8 = 0x02;
const R2_ERROR: u8 = 0x04;
const R2_WP_VIOLATION: u8 = 0x20;
const R2_OUT_OF_RANGE: u8 = 0x80;

/// The tokens (PLS §7.3.3).
const START_BLOCK: u8 = 0xFE;
const START_MULTIPLE_WRITE: u8 = 0xFC;
const STOP_TRANSMISSION: u8 = 0xFD;
/// Data responses `xxx0sss1`, the three high bits driven high.
const DATA_ACCEPTED: u8 = 0xE5;
const DATA_CRC_ERROR: u8 = 0xEB;
const DATA_WRITE_ERROR: u8 = 0xED;
/// Data error tokens `0000xxxx`.
const ERROR_TOKEN_ERROR: u8 = 0x01;
const ERROR_TOKEN_OUT_OF_RANGE: u8 = 0x08;

/// What the card drives while selected with nothing to send, and while busy.
const IDLE_LINE: u8 = 0xFF;
const BUSY_LINE: u8 = 0x00;

/// The voltage range CMD8 accepts: `0001b`, 2.7–3.6 V.
const VHS_27_36: u32 = 0b0001;

/// A command block: six bytes.
const COMMAND_BYTES: usize = 6;
/// A written block and its CRC16.
const WRITTEN_BYTES: usize = BLOCK_BYTES + 2;
const BLOCK: u64 = BLOCK_BYTES as u64;

/// How long the card takes, in nanoseconds of device time (design 2e-D7). The simplified
/// specification removes the access times of the SPI mode (§7.5.4); these are ASSUMPTION values
/// within the host timeouts of PLS §4.6.2 and of `SdFat`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SdTiming {
    /// From the first ACMD41 to the end of initialization.
    pub initialization_ns: u64,
    /// From the end of a read command, or of a block, to the start token of the next block.
    pub read_access_ns: u64,
    /// Programming a written block: the card is busy that long after its data response.
    pub program_ns: u64,
    /// An erase (CMD38): the card is busy that long after its response.
    pub erase_ns: u64,
}

impl Default for SdTiming {
    fn default() -> SdTiming {
        SdTiming {
            initialization_ns: 50_000_000,
            read_access_ns: 250_000,
            program_ns: 1_000_000,
            erase_ns: 2_000_000,
        }
    }
}

/// What a card is built with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SdCardConfig {
    pub cid: Cid,
    /// The card refuses writes and erases, and its CSD says so (`TMP_WRITE_PROTECT`).
    pub write_protected: bool,
    pub timing: SdTiming,
}

/// Where initialization stands (PLS Figure 7-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Init {
    /// No ACMD41 yet.
    NotStarted,
    /// The first ACMD41 came; the card is ready from `ready_at`.
    Starting {
        ready_at: Cycles,
    },
    Ready,
    /// A high-capacity card asked without CMD8 or with HCS 0: idle until CMD0.
    Stuck,
}

/// A command being received: its bytes so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Frame {
    bytes: [u8; COMMAND_BYTES],
    len: usize,
}

impl Frame {
    fn start(first: u8) -> Frame {
        Frame {
            bytes: [first, 0, 0, 0, 0, 0],
            len: 1,
        }
    }

    /// Adds a byte; the whole command once its sixth byte is in.
    fn push(&mut self, byte: u8) -> Option<Command> {
        if let Some(slot) = self.bytes.get_mut(self.len) {
            *slot = byte;
            self.len += 1;
        }
        (self.len >= COMMAND_BYTES).then(|| Command::from(self.bytes))
    }
}

/// A whole command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Command {
    index: u8,
    argument: u32,
    crc: u8,
    /// The CRC7 and end bit the first five bytes call for.
    expected: u8,
}

impl From<[u8; COMMAND_BYTES]> for Command {
    fn from(bytes: [u8; COMMAND_BYTES]) -> Command {
        let [first, a, b, c, d, crc] = bytes;
        Command {
            index: first & 0x3F,
            argument: u32::from_be_bytes([a, b, c, d]),
            crc,
            expected: crc7_byte(&[first, a, b, c, d]),
        }
    }
}

/// Whether a byte starts a command: `01` then the index.
const fn starts_a_command(byte: u8) -> bool {
    byte & 0xC0 == 0x40
}

/// What a read sends.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Source {
    /// Blocks of the storage from `next`, a block number (SDHC) or a byte address (SDSC), `len`
    /// bytes each.
    Storage { next: u64, len: u32, multiple: bool },
    /// A register or a status, sent once.
    Data(Vec<u8>),
}

/// A read in progress.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Read {
    source: Source,
    /// When the next start token may go out.
    token_at: Cycles,
    /// The packet being sent (token, data, CRC) and how much of it went.
    packet: Vec<u8>,
    sent: usize,
    /// A multiple read that sent an error token sends nothing more until CMD12.
    ended: bool,
    /// A command received while reading.
    side: Option<Frame>,
}

/// What the card is doing with the bytes it receives.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    /// Between commands.
    Idle,
    /// Receiving a command; `while_busy` when it started while the card was busy.
    Command {
        frame: Frame,
        while_busy: bool,
    },
    Read(Read),
    /// A write waits for the start token of its next block; `next` is the block number.
    WriteWait {
        multiple: bool,
        next: u64,
        side: Option<Frame>,
    },
    /// Receiving a written block and its CRC16.
    WriteData {
        multiple: bool,
        block: u64,
        buffer: Box<[u8; WRITTEN_BYTES]>,
        filled: usize,
    },
}

/// The erase addresses received (CMD32, CMD33), as block numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Erase {
    first: Option<u64>,
    last: Option<u64>,
}

/// An SD card in SPI mode.
// Each flag is a separate bit of the card's state in PLS chapter 7 (selection, mode, CMD8,
// ACMD41 seen, APP_CMD, CRC on/off): no two of them make one state an enum would name.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug)]
pub struct SdCard<S: BlockStorage> {
    config: SdCardConfig,
    capacity: Capacity,
    storage: S,
    time_base: Frequency,
    /// The latest instant seen.
    now: Cycles,
    selected: bool,
    /// CMD0 with CS low put the card in SPI mode; only a power cycle takes it back.
    spi: bool,
    init: Init,
    /// CMD8 accepted the voltage since the last CMD0.
    cmd8: bool,
    /// An ACMD41 was accepted since power-on: CMD1 is then taken (PLS §7.2.7).
    acmd41_seen: bool,
    /// CMD55 came: the next command is an application command.
    app: bool,
    crc_on: bool,
    /// CMD16, for a standard-capacity card.
    block_len: u32,
    /// The error bits of the second byte of R2, kept until CMD13 or ACMD13 reads them.
    status: u8,
    erase: Erase,
    /// The blocks accepted by the last write (ACMD22).
    well_written: u32,
    busy_until: Option<Cycles>,
    phase: Phase,
    /// The bytes the card shifts out next, before anything else.
    out: VecDeque<u8>,
    diagnostics: Vec<SdDiagnostic>,
    dropped: u64,
}

impl<S: BlockStorage> SdCard<S> {
    /// A card at power-on, in SD mode, holding `storage`, counting time in cycles of
    /// `time_base`.
    ///
    /// # Errors
    /// [`CapacityError`] when the storage's size is no card's (design 2e-D6).
    pub fn new(
        config: SdCardConfig,
        storage: S,
        time_base: Frequency,
    ) -> Result<SdCard<S>, CapacityError> {
        let bytes = storage
            .block_count()
            .checked_mul(BLOCK)
            .ok_or(CapacityError::TooLarge { bytes: u64::MAX })?;
        let capacity = Capacity::new(bytes)?;
        Ok(SdCard {
            config,
            capacity,
            storage,
            time_base,
            now: Cycles::ZERO,
            selected: false,
            spi: false,
            init: Init::NotStarted,
            cmd8: false,
            acmd41_seen: false,
            app: false,
            crc_on: false,
            block_len: 512,
            status: 0,
            erase: Erase::default(),
            well_written: 0,
            busy_until: None,
            phase: Phase::Idle,
            out: VecDeque::new(),
            diagnostics: Vec::new(),
            dropped: 0,
        })
    }

    /// The parameters the card was built with.
    #[must_use]
    pub fn config(&self) -> &SdCardConfig {
        &self.config
    }

    /// Its capacity and kind.
    #[must_use]
    pub fn capacity(&self) -> Capacity {
        self.capacity
    }

    /// What it keeps its blocks in.
    #[must_use]
    pub fn storage(&self) -> &S {
        &self.storage
    }

    /// What it keeps its blocks in, to read or change from outside a run.
    pub fn storage_mut(&mut self) -> &mut S {
        &mut self.storage
    }

    /// What it keeps its blocks in; the card is gone.
    pub fn into_storage(self) -> S {
        self.storage
    }

    /// Whether the card is selected.
    #[must_use]
    pub fn is_selected(&self) -> bool {
        self.selected
    }

    /// Whether the card is in SPI mode (CMD0 with CS low came).
    #[must_use]
    pub fn is_in_spi_mode(&self) -> bool {
        self.spi
    }

    /// Whether the card is busy at `now`: programming a block or erasing.
    #[must_use]
    pub fn is_busy(&self, now: Cycles) -> bool {
        self.busy_until
            .is_some_and(|until| now.max(self.now) < until)
    }

    /// The diagnostics since the last call, oldest first.
    pub fn take_diagnostics(&mut self) -> Vec<SdDiagnostic> {
        std::mem::take(&mut self.diagnostics)
    }

    /// How many diagnostics were dropped because [`MAX_DIAGNOSTICS`] were waiting.
    #[must_use]
    pub fn dropped_diagnostics(&self) -> u64 {
        self.dropped
    }

    /// The chip-select line changed at `now`: `selected` when it became active (low).
    /// Deselecting drops what is not yet shifted out, a command in the middle, a block in the
    /// middle; a multiple read ends; programming goes on (design 2e-D8).
    pub fn select(&mut self, now: Cycles, selected: bool) {
        self.advance(now);
        if selected == self.selected {
            return;
        }
        self.selected = selected;
        if selected {
            return;
        }
        self.out.clear();
        let phase = std::mem::replace(&mut self.phase, Phase::Idle);
        self.phase = match phase {
            Phase::Command { .. } => {
                self.report(SdDiagnosticKind::CommandCut);
                Phase::Idle
            }
            Phase::Read(read) => {
                if read.sent > 0 && read.sent < read.packet.len() {
                    self.report(SdDiagnosticKind::TransferCut);
                }
                if read.side.is_some() {
                    self.report(SdDiagnosticKind::CommandCut);
                }
                Phase::Idle
            }
            Phase::WriteData {
                multiple, block, ..
            } => {
                self.report(SdDiagnosticKind::TransferCut);
                if multiple {
                    Phase::WriteWait {
                        multiple,
                        next: block,
                        side: None,
                    }
                } else {
                    Phase::Idle
                }
            }
            Phase::WriteWait {
                multiple,
                next,
                side,
            } => {
                if side.is_some() {
                    self.report(SdDiagnosticKind::CommandCut);
                }
                Phase::WriteWait {
                    multiple,
                    next,
                    side: None,
                }
            }
            Phase::Idle => Phase::Idle,
        };
    }

    /// One byte each way at `now`: `mosi` is the byte the master sent; the result is the byte the
    /// card drove on MISO, or `None` when it drives nothing (deselected, or not yet in SPI mode)
    /// and the pad reads its pull.
    pub fn exchange(&mut self, now: Cycles, mosi: u8) -> Option<u8> {
        self.advance(now);
        if !self.selected {
            return None;
        }
        let miso = self.spi.then(|| self.shift_out());
        self.shift_in(mosi);
        miso
    }

    /// The bytes of `mosi`, one [`SdCard::exchange`] each at `now`; each byte the card drives
    /// overwrites its place in `miso`, which holds what the pad reads otherwise. `miso` shorter
    /// than `mosi` keeps the bytes it has room for.
    pub fn transfer(&mut self, now: Cycles, mosi: &[u8], miso: &mut [u8]) {
        for (index, &byte) in mosi.iter().enumerate() {
            if let Some(driven) = self.exchange(now, byte)
                && let Some(place) = miso.get_mut(index)
            {
                *place = driven;
            }
        }
    }

    fn advance(&mut self, now: Cycles) {
        self.now = self.now.max(now);
        if self.busy_until.is_some_and(|until| until <= self.now) {
            self.busy_until = None;
        }
        if let Init::Starting { ready_at } = self.init
            && ready_at <= self.now
        {
            self.init = Init::Ready;
        }
    }

    /// The instant `nanos` after now.
    fn after(&self, nanos: u64) -> Cycles {
        let cycles = self.time_base.nanos_to_cycles(u128::from(nanos));
        Cycles(self.now.get().saturating_add(cycles.get()))
    }

    fn report(&mut self, kind: SdDiagnosticKind) {
        if self.diagnostics.len() < MAX_DIAGNOSTICS {
            self.diagnostics.push(SdDiagnostic { at: self.now, kind });
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    // ---------------------------------------------------------------------------------------
    // What goes out

    fn shift_out(&mut self) -> u8 {
        if let Some(byte) = self.out.pop_front() {
            return byte;
        }
        if self.busy_until.is_some() {
            return BUSY_LINE;
        }
        // Only a read sends anything of its own.
        if !matches!(self.phase, Phase::Read(_)) {
            return IDLE_LINE;
        }
        let Phase::Read(mut read) = std::mem::replace(&mut self.phase, Phase::Idle) else {
            return IDLE_LINE;
        };
        let byte = self.read_byte(&mut read);
        let single_done = !matches!(read.source, Source::Storage { multiple: true, .. })
            && read.sent >= read.packet.len()
            && !read.packet.is_empty();
        self.phase = if single_done {
            Phase::Idle
        } else {
            Phase::Read(read)
        };
        byte
    }

    /// The next byte of a read: the packet in progress, or a new one once its token is due.
    fn read_byte(&mut self, read: &mut Read) -> u8 {
        if read.sent >= read.packet.len() {
            let finished_one = !read.packet.is_empty();
            if finished_one {
                // A multiple read waits again before its next block.
                read.packet.clear();
                read.sent = 0;
                read.token_at = self.after(self.config.timing.read_access_ns);
                return IDLE_LINE;
            }
            if read.ended || self.now < read.token_at {
                return IDLE_LINE;
            }
            read.packet = self.next_packet(read);
            read.sent = 0;
        }
        let byte = read.packet.get(read.sent).copied().unwrap_or(IDLE_LINE);
        read.sent += 1;
        byte
    }

    /// The packet of the next block of a read: start token, data, CRC16; or a data error token.
    fn next_packet(&mut self, read: &mut Read) -> Vec<u8> {
        let data = match &mut read.source {
            Source::Data(data) => std::mem::take(data),
            Source::Storage {
                next,
                len,
                multiple,
            } => {
                let address = *next;
                let len = *len;
                let multiple = *multiple;
                match self.read_storage(address, len) {
                    Ok(data) => {
                        *next = match self.capacity.kind() {
                            CardKind::Sdhc => address.saturating_add(1),
                            CardKind::Sdsc => address.saturating_add(u64::from(len)),
                        };
                        data
                    }
                    Err(token) => {
                        read.ended = multiple;
                        return vec![token];
                    }
                }
            }
        };
        let crc = crc16(&data);
        let mut packet = Vec::with_capacity(data.len() + 3);
        packet.push(START_BLOCK);
        packet.extend_from_slice(&data);
        packet.extend_from_slice(&crc.to_be_bytes());
        packet
    }

    /// `len` bytes at `address` (a block number for SDHC, a byte address for SDSC), or the data
    /// error token that replaces them.
    fn read_storage(&mut self, address: u64, len: u32) -> Result<Vec<u8>, u8> {
        let (block, offset) = match self.capacity.kind() {
            CardKind::Sdhc => (address, 0),
            CardKind::Sdsc => (address / BLOCK, address % BLOCK),
        };
        if block >= self.capacity.blocks() {
            self.status |= R2_OUT_OF_RANGE;
            self.report(SdDiagnosticKind::ReadPastEnd { block });
            return Err(ERROR_TOKEN_OUT_OF_RANGE);
        }
        let end = offset.saturating_add(u64::from(len));
        if end > BLOCK {
            self.status |= R2_ERROR;
            self.report(SdDiagnosticKind::MisalignedAddress {
                address: u32::try_from(address).unwrap_or(u32::MAX),
                length: len,
            });
            return Err(ERROR_TOKEN_ERROR);
        }
        let mut data = [0u8; BLOCK_BYTES];
        if let Err(error) = self.storage.read_block(block, &mut data) {
            self.status |= R2_ERROR;
            self.report(SdDiagnosticKind::StorageFailed {
                block,
                detail: error.to_string(),
            });
            return Err(ERROR_TOKEN_ERROR);
        }
        // INVARIANT: `offset` < 512 and `end` <= 512.
        let range = usize::try_from(offset).unwrap_or(0)..usize::try_from(end).unwrap_or(0);
        Ok(data.get(range).map(<[u8]>::to_vec).unwrap_or_default())
    }

    // ---------------------------------------------------------------------------------------
    // What comes in

    fn shift_in(&mut self, byte: u8) {
        let phase = std::mem::replace(&mut self.phase, Phase::Idle);
        self.phase = match phase {
            Phase::Idle => self.idle_byte(byte),
            Phase::Command {
                mut frame,
                while_busy,
            } => match frame.push(byte) {
                Some(command) => self.command(command, while_busy),
                None => Phase::Command { frame, while_busy },
            },
            Phase::Read(mut read) => match read.side.as_mut().map(|frame| frame.push(byte)) {
                Some(Some(command)) => {
                    read.side = None;
                    self.side_command(command, Phase::Read(read))
                }
                Some(None) => Phase::Read(read),
                None => {
                    if starts_a_command(byte) {
                        read.side = Some(Frame::start(byte));
                    }
                    Phase::Read(read)
                }
            },
            Phase::WriteWait {
                multiple,
                next,
                mut side,
            } => match side.as_mut().map(|frame| frame.push(byte)) {
                Some(Some(command)) => self.side_command(
                    command,
                    Phase::WriteWait {
                        multiple,
                        next,
                        side: None,
                    },
                ),
                Some(None) => Phase::WriteWait {
                    multiple,
                    next,
                    side,
                },
                None => self.write_token(byte, multiple, next),
            },
            Phase::WriteData {
                multiple,
                block,
                mut buffer,
                filled,
            } => {
                if let Some(slot) = buffer.get_mut(filled) {
                    *slot = byte;
                }
                let filled = filled + 1;
                if filled >= WRITTEN_BYTES {
                    self.written(multiple, block, &buffer)
                } else {
                    Phase::WriteData {
                        multiple,
                        block,
                        buffer,
                        filled,
                    }
                }
            }
        };
    }

    /// A byte between commands.
    fn idle_byte(&mut self, byte: u8) -> Phase {
        if starts_a_command(byte) {
            // A host that starts a command no longer reads what was left of an answer.
            self.out.clear();
            return Phase::Command {
                frame: Frame::start(byte),
                while_busy: self.busy_until.is_some(),
            };
        }
        if byte != IDLE_LINE && self.spi && self.busy_until.is_none() {
            self.report(SdDiagnosticKind::StrayByte { byte });
        }
        Phase::Idle
    }

    /// A byte while a write waits for its next block: a token, `FFh`, or the start of a command.
    fn write_token(&mut self, byte: u8, multiple: bool, next: u64) -> Phase {
        let waiting = Phase::WriteWait {
            multiple,
            next,
            side: None,
        };
        if starts_a_command(byte) {
            return Phase::WriteWait {
                multiple,
                next,
                side: Some(Frame::start(byte)),
            };
        }
        if byte == IDLE_LINE || self.busy_until.is_some() {
            return waiting;
        }
        let expected = if multiple {
            START_MULTIPLE_WRITE
        } else {
            START_BLOCK
        };
        if byte == expected {
            return Phase::WriteData {
                multiple,
                block: next,
                buffer: Box::new([0; WRITTEN_BYTES]),
                filled: 0,
            };
        }
        if multiple && byte == STOP_TRANSMISSION {
            return Phase::Idle;
        }
        self.report(SdDiagnosticKind::UnexpectedToken { token: byte });
        waiting
    }

    /// A block and its CRC16 received: checked, written, answered.
    fn written(&mut self, multiple: bool, block: u64, buffer: &[u8; WRITTEN_BYTES]) -> Phase {
        let (data, crc) = buffer.split_at(BLOCK_BYTES);
        let received = match crc {
            [high, low] => u16::from_be_bytes([*high, *low]),
            _ => 0,
        };
        let expected = crc16(data);
        let response = if self.crc_on && received != expected {
            self.report(SdDiagnosticKind::DataCrc {
                block,
                crc: received,
                expected,
            });
            DATA_CRC_ERROR
        } else if self.config.write_protected {
            self.status |= R2_WP_VIOLATION;
            self.report(SdDiagnosticKind::WriteProtected { block });
            DATA_WRITE_ERROR
        } else if block >= self.capacity.blocks() {
            self.status |= R2_OUT_OF_RANGE;
            self.report(SdDiagnosticKind::AddressOutOfRange {
                address: u32::try_from(block).unwrap_or(u32::MAX),
            });
            DATA_WRITE_ERROR
        } else {
            let mut whole = [0u8; BLOCK_BYTES];
            whole.copy_from_slice(data);
            match self.storage.write_block(block, &whole) {
                Ok(()) => {
                    self.well_written = self.well_written.saturating_add(1);
                    self.busy_until = Some(self.after(self.config.timing.program_ns));
                    DATA_ACCEPTED
                }
                Err(error) => {
                    self.status |= R2_ERROR;
                    self.report(SdDiagnosticKind::StorageFailed {
                        block,
                        detail: error.to_string(),
                    });
                    DATA_WRITE_ERROR
                }
            }
        };
        self.out.push_back(response);
        if multiple {
            Phase::WriteWait {
                multiple,
                next: block.saturating_add(1),
                side: None,
            }
        } else {
            Phase::Idle
        }
    }

    /// A command received while the card reads or waits for a written block: CMD0 resets, CMD12
    /// stops, anything else is refused (PLS §7.2.8).
    fn side_command(&mut self, command: Command, during: Phase) -> Phase {
        match command.index {
            0 => self.command(command, false),
            12 => {
                if self.crc_on && command.crc != command.expected {
                    return self.crc_refusal(command);
                }
                self.respond(0, &[]);
                Phase::Idle
            }
            index => {
                self.report(SdDiagnosticKind::CommandDuringTransfer { index });
                during
            }
        }
    }

    /// Queues an answer: one `FFh`, then R1 with the idle bit of now and `flags`, then `extra`.
    fn respond(&mut self, flags: u8, extra: &[u8]) {
        let idle = if self.init == Init::Ready { 0 } else { R1_IDLE };
        self.out.clear();
        self.out.push_back(IDLE_LINE);
        self.out.push_back(flags | idle);
        self.out.extend(extra.iter().copied());
    }

    fn crc_refusal(&mut self, command: Command) -> Phase {
        self.report(SdDiagnosticKind::CommandCrc {
            index: command.index,
            crc: command.crc,
            expected: command.expected,
        });
        self.respond(R1_CRC, &[]);
        Phase::Idle
    }

    /// A whole command between commands.
    fn command(&mut self, command: Command, while_busy: bool) -> Phase {
        if !self.spi {
            return self.command_in_sd_mode(command);
        }
        let index = command.index;
        if while_busy && index != 0 {
            self.report(SdDiagnosticKind::CommandWhileBusy { index });
            return Phase::Idle;
        }
        if (index == 8 || self.crc_on) && command.crc != command.expected {
            self.app = false;
            return self.crc_refusal(command);
        }
        let app = std::mem::take(&mut self.app) && is_application_command(index);
        let erase_reset =
            if !app && self.erase != Erase::default() && !matches!(index, 13 | 32 | 33 | 38) {
                self.erase = Erase::default();
                R1_ERASE_RESET
            } else {
                0
            };
        if self.init != Init::Ready && !allowed_in_idle(index, app) {
            self.report(SdDiagnosticKind::IllegalInIdle { index, app });
            self.respond(R1_ILLEGAL | erase_reset, &[]);
            return Phase::Idle;
        }
        if app {
            self.application_command(command, erase_reset)
        } else {
            self.regular_command(command, erase_reset)
        }
    }

    /// Before SPI mode, only CMD0 with a valid CRC is heard (PLS §7.2.1, §7.2.2).
    fn command_in_sd_mode(&mut self, command: Command) -> Phase {
        if command.index != 0 {
            self.report(SdDiagnosticKind::CommandInSdMode {
                index: command.index,
            });
            return Phase::Idle;
        }
        if command.crc != command.expected {
            self.report(SdDiagnosticKind::Cmd0CrcInSdMode { crc: command.crc });
            return Phase::Idle;
        }
        self.spi = true;
        self.reset();
        self.respond(0, &[]);
        Phase::Idle
    }

    /// What CMD0 does in SPI mode: the idle state, the block length of 512, no erase sequence,
    /// no programming. The CRC setting of CMD59 stays (PLS §7.2.2).
    fn reset(&mut self) {
        self.init = Init::NotStarted;
        self.cmd8 = false;
        self.app = false;
        self.block_len = 512;
        self.erase = Erase::default();
        self.busy_until = None;
        self.status = 0;
    }

    fn regular_command(&mut self, command: Command, erase_reset: u8) -> Phase {
        let argument = command.argument;
        match command.index {
            0 => {
                self.reset();
                self.respond(0, &[]);
                Phase::Idle
            }
            1 => {
                if self.acmd41_seen {
                    self.send_op_cond(argument, erase_reset)
                } else {
                    self.report(SdDiagnosticKind::Cmd1BeforeAcmd41);
                    self.respond(R1_ILLEGAL | erase_reset, &[]);
                    Phase::Idle
                }
            }
            6 => {
                self.respond(erase_reset, &[]);
                self.data_read(switch_status(argument).to_vec())
            }
            8 => {
                let voltage = argument >> 8 & 0xF;
                let accepted = voltage == VHS_27_36;
                self.cmd8 = accepted;
                let pattern = argument.to_be_bytes()[3];
                let vca = u8::from(accepted);
                self.respond(erase_reset, &[0x00, 0x00, vca, pattern]);
                Phase::Idle
            }
            9 => {
                let csd = self.capacity.csd(self.config.write_protected);
                self.respond(erase_reset, &[]);
                self.data_read(csd.to_vec())
            }
            10 => {
                let cid = self.config.cid.register();
                self.respond(erase_reset, &[]);
                self.data_read(cid.to_vec())
            }
            12 => {
                self.respond(erase_reset, &[]);
                Phase::Idle
            }
            13 => {
                let status = std::mem::take(&mut self.status);
                self.respond(erase_reset, &[status]);
                Phase::Idle
            }
            16 => {
                if (1..=512).contains(&argument) {
                    self.block_len = argument;
                    self.respond(erase_reset, &[]);
                } else {
                    self.report(SdDiagnosticKind::BlockLength { length: argument });
                    self.respond(R1_PARAMETER | erase_reset, &[]);
                }
                Phase::Idle
            }
            17 | 18 => self.read_command(argument, command.index == 18, erase_reset),
            24 | 25 => self.write_command(argument, command.index == 25, erase_reset),
            32 | 33 => self.erase_address(argument, command.index == 32, erase_reset),
            38 => self.erase_command(erase_reset),
            55 => {
                self.app = true;
                self.respond(erase_reset, &[]);
                Phase::Idle
            }
            58 => {
                let ocr = ocr(self.init == Init::Ready, self.capacity.kind());
                self.respond(erase_reset, &ocr.to_be_bytes());
                Phase::Idle
            }
            59 => {
                self.crc_on = argument & 1 == 1;
                self.respond(erase_reset, &[]);
                Phase::Idle
            }
            index => {
                let kind = match unsupported_name(index, false) {
                    Some(name) => SdDiagnosticKind::UnsupportedCommand {
                        index,
                        app: false,
                        name,
                    },
                    None => SdDiagnosticKind::UnknownCommand { index, app: false },
                };
                self.report(kind);
                self.respond(R1_ILLEGAL | erase_reset, &[]);
                Phase::Idle
            }
        }
    }

    fn application_command(&mut self, command: Command, erase_reset: u8) -> Phase {
        match command.index {
            13 => {
                let status = std::mem::take(&mut self.status);
                self.respond(erase_reset, &[status]);
                self.data_read(SD_STATUS.to_vec())
            }
            22 => {
                let count = self.well_written.to_be_bytes().to_vec();
                self.respond(erase_reset, &[]);
                self.data_read(count)
            }
            23 | 42 => {
                // ACMD23 (pre-erase count) only speeds a real card up; ACMD42 (the pull-up on
                // CS) changes nothing a model sees.
                self.respond(erase_reset, &[]);
                Phase::Idle
            }
            41 => {
                self.acmd41_seen = true;
                self.send_op_cond(command.argument, erase_reset)
            }
            51 => {
                self.respond(erase_reset, &[]);
                self.data_read(SCR.to_vec())
            }
            index => {
                let name = unsupported_name(index, true).unwrap_or("security command");
                self.report(SdDiagnosticKind::UnsupportedCommand {
                    index,
                    app: true,
                    name,
                });
                self.respond(R1_ILLEGAL | erase_reset, &[]);
                Phase::Idle
            }
        }
    }

    /// ACMD41 (and CMD1 once allowed): starts initialization at the first, and answers whether it
    /// ended. A high-capacity card needs CMD8 and HCS at the first (PLS Figure 7-1, note 1).
    fn send_op_cond(&mut self, argument: u32, erase_reset: u8) -> Phase {
        if self.init == Init::NotStarted {
            let hcs = argument & 1 << 30 != 0;
            self.init = if self.capacity.kind() == CardKind::Sdhc && !(self.cmd8 && hcs) {
                self.report(SdDiagnosticKind::HighCapacityWithoutHcs { cmd8: self.cmd8 });
                Init::Stuck
            } else {
                Init::Starting {
                    ready_at: self.after(self.config.timing.initialization_ns),
                }
            };
            self.advance(self.now);
        }
        self.respond(erase_reset, &[]);
        Phase::Idle
    }

    /// A register or a status sent as a data block, right after the response.
    fn data_read(&mut self, data: Vec<u8>) -> Phase {
        Phase::Read(Read {
            source: Source::Data(data),
            token_at: self.now,
            packet: Vec::new(),
            sent: 0,
            ended: false,
            side: None,
        })
    }

    /// The block number of a block address, or the R1 bits that refuse it. A standard-capacity
    /// card takes a byte address on a 512-byte block, with a length of `len`.
    fn block_of(&mut self, address: u32, len: u32, write: bool) -> Result<u64, u8> {
        let address64 = u64::from(address);
        match self.capacity.kind() {
            CardKind::Sdhc => {
                if address64 >= self.capacity.blocks() {
                    self.report(SdDiagnosticKind::AddressOutOfRange { address });
                    return Err(R1_PARAMETER);
                }
                Ok(address64)
            }
            CardKind::Sdsc => {
                if write && len != 512 {
                    self.report(SdDiagnosticKind::BlockLength { length: len });
                    return Err(R1_PARAMETER);
                }
                if address64.saturating_add(u64::from(len)) > self.capacity.bytes() {
                    self.report(SdDiagnosticKind::AddressOutOfRange { address });
                    return Err(R1_PARAMETER);
                }
                if address64 % BLOCK + u64::from(len) > BLOCK {
                    self.report(SdDiagnosticKind::MisalignedAddress {
                        address,
                        length: len,
                    });
                    return Err(R1_ADDRESS);
                }
                Ok(address64 / BLOCK)
            }
        }
    }

    fn read_command(&mut self, address: u32, multiple: bool, erase_reset: u8) -> Phase {
        let len = match self.capacity.kind() {
            CardKind::Sdhc => 512,
            CardKind::Sdsc => self.block_len,
        };
        if let Err(flags) = self.block_of(address, len, false) {
            self.respond(flags | erase_reset, &[]);
            return Phase::Idle;
        }
        self.respond(erase_reset, &[]);
        Phase::Read(Read {
            source: Source::Storage {
                next: u64::from(address),
                len,
                multiple,
            },
            token_at: self.after(self.config.timing.read_access_ns),
            packet: Vec::new(),
            sent: 0,
            ended: false,
            side: None,
        })
    }

    fn write_command(&mut self, address: u32, multiple: bool, erase_reset: u8) -> Phase {
        let len = match self.capacity.kind() {
            CardKind::Sdhc => 512,
            CardKind::Sdsc => self.block_len,
        };
        match self.block_of(address, len, true) {
            Err(flags) => {
                self.respond(flags | erase_reset, &[]);
                Phase::Idle
            }
            Ok(block) => {
                self.well_written = 0;
                self.respond(erase_reset, &[]);
                Phase::WriteWait {
                    multiple,
                    next: block,
                    side: None,
                }
            }
        }
    }

    /// CMD32 or CMD33: the first or last block of an erase.
    fn erase_address(&mut self, address: u32, first: bool, erase_reset: u8) -> Phase {
        let block = match self.capacity.kind() {
            CardKind::Sdhc => u64::from(address),
            CardKind::Sdsc => u64::from(address) / BLOCK,
        };
        if block >= self.capacity.blocks() {
            self.report(SdDiagnosticKind::AddressOutOfRange { address });
            self.respond(R1_PARAMETER | erase_reset, &[]);
            return Phase::Idle;
        }
        if first {
            self.erase = Erase {
                first: Some(block),
                last: None,
            };
            self.respond(erase_reset, &[]);
        } else if self.erase.first.is_some() {
            self.erase.last = Some(block);
            self.respond(erase_reset, &[]);
        } else {
            self.report(SdDiagnosticKind::EraseSequence);
            self.respond(R1_ERASE_SEQUENCE | erase_reset, &[]);
        }
        Phase::Idle
    }

    /// CMD38: erases the blocks from the first to the last address, then is busy.
    fn erase_command(&mut self, erase_reset: u8) -> Phase {
        let erase = std::mem::take(&mut self.erase);
        let (Some(first), Some(last)) = (erase.first, erase.last) else {
            self.report(SdDiagnosticKind::EraseSequence);
            self.respond(R1_ERASE_SEQUENCE | erase_reset, &[]);
            return Phase::Idle;
        };
        self.respond(erase_reset, &[]);
        if self.config.write_protected {
            self.status |= R2_WP_ERASE_SKIP;
            self.report(SdDiagnosticKind::EraseProtected);
            return Phase::Idle;
        }
        let (low, high) = (first.min(last), first.max(last));
        let count = high - low + 1;
        if let Err(error) = self.storage.erase(low, count) {
            self.status |= R2_ERROR;
            self.report(SdDiagnosticKind::StorageFailed {
                block: low,
                detail: error.to_string(),
            });
        }
        self.busy_until = Some(self.after(self.config.timing.erase_ns));
        Phase::Idle
    }
}

/// Whether `index` after CMD55 is an application command; any other is the regular command of
/// the same number (PLS §4.3.9.1).
const fn is_application_command(index: u8) -> bool {
    matches!(index, 13 | 18 | 22 | 23 | 25 | 26 | 38 | 41..=49 | 51)
}

/// What the idle state takes (PLS §7.2.7): CMD0, CMD1, CMD8, CMD55, CMD58, CMD59 and ACMD41.
const fn allowed_in_idle(index: u8, app: bool) -> bool {
    if app {
        index == 41
    } else {
        matches!(index, 0 | 1 | 8 | 55 | 58 | 59)
    }
}

/// The commands of the specification the model does not carry out (design 2e-D5).
const fn unsupported_name(index: u8, app: bool) -> Option<&'static str> {
    if app {
        return match index {
            18 | 25 | 26 | 38 | 43..=49 => Some("security command"),
            _ => None,
        };
    }
    match index {
        27 => Some("PROGRAM_CSD"),
        28 => Some("SET_WRITE_PROT"),
        29 => Some("CLR_WRITE_PROT"),
        30 => Some("SEND_WRITE_PROT"),
        42 => Some("LOCK_UNLOCK"),
        56 => Some("GEN_CMD"),
        _ => None,
    }
}

/// A slot that holds a card or none (design 2e-D11). An empty slot never drives MISO.
#[derive(Debug)]
pub struct SdSlot<S: BlockStorage> {
    card: Option<SdCard<S>>,
}

impl<S: BlockStorage> SdSlot<S> {
    /// A slot without a card.
    #[must_use]
    pub fn empty() -> SdSlot<S> {
        SdSlot { card: None }
    }

    /// A slot holding `card`.
    #[must_use]
    pub fn holding(card: SdCard<S>) -> SdSlot<S> {
        SdSlot { card: Some(card) }
    }

    /// The card, if there is one.
    #[must_use]
    pub fn card(&self) -> Option<&SdCard<S>> {
        self.card.as_ref()
    }

    /// The card, if there is one.
    pub fn card_mut(&mut self) -> Option<&mut SdCard<S>> {
        self.card.as_mut()
    }

    /// Takes the card out; the slot is empty.
    pub fn take(&mut self) -> Option<SdCard<S>> {
        self.card.take()
    }

    /// Puts `card` in, and gives back the one that was there.
    pub fn insert(&mut self, card: SdCard<S>) -> Option<SdCard<S>> {
        self.card.replace(card)
    }

    /// [`SdCard::select`] of the card, if there is one.
    pub fn select(&mut self, now: Cycles, selected: bool) {
        if let Some(card) = &mut self.card {
            card.select(now, selected);
        }
    }

    /// [`SdCard::exchange`] of the card; `None` without one.
    pub fn exchange(&mut self, now: Cycles, mosi: u8) -> Option<u8> {
        self.card.as_mut()?.exchange(now, mosi)
    }

    /// [`SdCard::transfer`] of the card; `miso` is left as it is without one.
    pub fn transfer(&mut self, now: Cycles, mosi: &[u8], miso: &mut [u8]) {
        if let Some(card) = &mut self.card {
            card.transfer(now, mosi, miso);
        }
    }
}
