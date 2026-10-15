//! The format `inkemu-session 2` (G3, G4; ADR 0013): a session as bytes, and back within constant
//! bounds. Sessions of the format `inkemu-session 1` are read too.
//!
//! Binary, little-endian, the fields in this order: the version line `inkemu-session 2\n`; the
//! ROM revision (`u16`) and the SHA-256 of the ROM image; the profile id (`u8` length) and text
//! (`u32` length) and the SHA-256 of the text; the flash size (`u32`), the SHA-256 of the flash,
//! the length of the deflated flash (`u32`) and its bytes; the SD card at the start (`u8`
//! presence, 0: no card; a card is the format's place for sub-project 2e); `--strict-mmio`
//! (`u8`), `--max-cycles` and `--max-time` (each a `u8` presence, then a `u64` when present); the
//! cut (a `u8` reason, 0 for none, then the `u64` instant); the USB host at power-on (`u8`); the
//! number of inputs (`u32`), then each input: a `u8` tag (0 `uart0`, 1 `usb-serial-jtag`, 2
//! `uart1`, 3 the USB host, 4 the board), the `u64` date, then for a console a `u32` length and
//! the bytes, for the host a `u8` state, for the board a `u16` target and a `u32` value; the
//! outcome: the end (`u8`), device time, cycles, instructions and trace hash (`u64` each), the
//! SHA-256 of the final flash, the SD card at the end (`u8` presence, 0: none), then the SHA-256
//! of each console. Dates and instants are in cycles of the time base. Nothing follows.
//!
//! The format 1 is the same without the two presences of an SD card and without inputs of the
//! board.
//!
//! The reader allocates from no length it has not bounded: every length is checked against a
//! constant bound and against the bytes left in the file before anything is copied or reserved;
//! the inputs are reserved at the count the file gives, no more than [`MAX_INPUTS`] and than the
//! bytes left can hold, then pushed one by one as they are read. The flash is inflated into the size of the device of the
//! profile, checked against the size the file gives and against [`MAX_FLASH_BYTES`] first.
use crate::outcome::{Cut, CutReason, End, Limits, Outcome, hex, place};
use boards::{DeviceProfile, ProfileError};
use emu_core::BoardInput;
use emu_core::Cycles;
use flate2::write::DeflateEncoder;
use flate2::{Compression, Decompress, FlushDecompress, Status};
use sha2::{Digest as _, Sha256};
use soc_esp32c3::flash::{MAX_FLASH_BYTES, MIN_FLASH_BYTES};
use soc_esp32c3::{
    AcceptedInput, ConsoleChannel, InputJournal, MAX_JOURNAL_BOARD_INPUTS,
    MAX_JOURNAL_CONSOLE_BYTES, MAX_JOURNAL_CONSOLE_CHUNKS, MAX_JOURNAL_HOST_CHANGES,
    MAX_JOURNAL_INPUTS, RomImage, UsbHost,
};
use std::fmt;
use std::io::Write as _;

/// The first line of a session, its version: a later format changes the number.
pub const VERSION_LINE: &str = "inkemu-session 2";
/// The bytes a session begins with: [`VERSION_LINE`] and a line feed.
const MAGIC: &[u8] = b"inkemu-session 2\n";
/// The bytes a session of the format 1 begins with, which is still read.
const MAGIC_1: &[u8] = b"inkemu-session 1\n";
/// What every version line begins with: a file that does too is a session of another version.
const MAGIC_PREFIX: &[u8] = b"inkemu-session ";
/// The longest version line read back to name another version.
const MAX_VERSION_LINE_BYTES: usize = 32;

/// The longest profile id: the ids are short words (`x4`).
pub const MAX_PROFILE_ID_BYTES: usize = 64;
/// The longest profile text: a profile is a page of TOML, the X4's about 3 KiB.
pub const MAX_PROFILE_TEXT_BYTES: usize = 64 * 1024;
/// The longest deflated flash: deflate never grows data by more than a few bytes per 64 KiB
/// block it stores as is, far under this margin of one part in a thousand.
pub const MAX_DEFLATED_FLASH_BYTES: usize = MAX_FLASH as usize + MAX_FLASH as usize / 1024 + 4096;
/// The most input on one console channel over the run, each chunk counting for `max(len, 1)`
/// bytes: the bound of the machine's journal, that of ADR 0006 on what is pending.
pub const MAX_CONSOLE_BYTES: usize = MAX_JOURNAL_CONSOLE_BYTES;
/// The most chunks on one console channel over the run: the bound of the machine's journal,
/// which a channel filled with full packets of USB-Serial-JTAG reaches with
/// [`MAX_CONSOLE_BYTES`].
pub const MAX_CONSOLE_CHUNKS: usize = MAX_JOURNAL_CONSOLE_CHUNKS;
/// The most changes of the USB host over the run.
pub const MAX_HOST_CHANGES: usize = MAX_JOURNAL_HOST_CHANGES;
/// The most inputs of the board over the run (ADR 0013).
pub const MAX_BOARD_INPUTS: usize = MAX_JOURNAL_BOARD_INPUTS;
/// The most inputs a session holds: the chunks of the three consoles, the changes of the USB
/// host and the inputs of the board, 81 920.
pub const MAX_INPUTS: usize = MAX_JOURNAL_INPUTS;
/// The most memory the inputs of a session take once read ([`Session::inputs`]): its entries,
/// each of a fixed size, and the bytes of its consoles, each channel at its bound. The
/// allocator adds its own overhead to each chunk that holds bytes, at most [`MAX_INPUTS`] of
/// them: about 7 MiB in all on a 64-bit host, whatever the file.
pub const MAX_INPUTS_MEMORY_BYTES: usize =
    MAX_INPUTS * size_of::<AcceptedInput>() + 3 * MAX_CONSOLE_BYTES;

const MAX_FLASH: u32 = MAX_FLASH_BYTES;
const SHA256_BYTES: usize = 32;
/// The tag, the date and the length of a console input, before its bytes.
const CONSOLE_INPUT_HEAD: usize = 1 + 8 + 4;
/// The tag, the date and the state of a change of the USB host.
const HOST_INPUT_BYTES: usize = 1 + 8 + 1;
/// The tag, the date, the target and the value of an input of the board.
const BOARD_INPUT_BYTES: usize = 1 + 8 + 2 + 4;
/// The outcome, the presence of an SD card at the end included.
const OUTCOME_BYTES: usize = 1 + 4 * 8 + SHA256_BYTES * 4 + 1;
/// The longest session: every field at its bound. A console input of `n` bytes costs
/// `CONSOLE_INPUT_HEAD + n` bytes of file, one of its channel's chunks and at least `max(n, 1)`
/// of its bytes, so a channel's inputs take at most `CONSOLE_INPUT_HEAD` bytes of file per chunk
/// and one per byte of the bound.
pub const MAX_SESSION_BYTES: usize = MAGIC.len()
    + 2
    + SHA256_BYTES
    + 1
    + MAX_PROFILE_ID_BYTES
    + 4
    + MAX_PROFILE_TEXT_BYTES
    + SHA256_BYTES
    + 4
    + SHA256_BYTES
    + 4
    + MAX_DEFLATED_FLASH_BYTES
    + 1
    + 1
    + 2 * 9
    + 9
    + 1
    + 4
    + 3 * (MAX_CONSOLE_CHUNKS * CONSOLE_INPUT_HEAD + MAX_CONSOLE_BYTES)
    + MAX_HOST_CHANGES * HOST_INPUT_BYTES
    + MAX_BOARD_INPUTS * BOARD_INPUT_BYTES
    + OUTCOME_BYTES;

/// The mask ROM a session was recorded on: never the ROM itself, which its licence keeps out of
/// a file anyone may pass on (ADR 0001).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RomIdentity {
    /// The ROM revision, `major * 100 + minor`.
    pub revision: u16,
    /// The SHA-256 of the ROM image the machine runs ([`RomImage::bytes`]).
    pub sha256: [u8; 32],
}

impl RomIdentity {
    /// The identity of `rom`, of revision `revision`.
    #[must_use]
    pub fn of(revision: u16, rom: &RomImage) -> RomIdentity {
        RomIdentity {
            revision,
            sha256: Sha256::digest(rom.bytes()).into(),
        }
    }
}

/// The device profile of a session: its id and its whole text, which the replay rebuilds the
/// device from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileText {
    pub id: String,
    pub text: String,
}

impl ProfileText {
    /// The device the text describes, checked to carry its id.
    ///
    /// # Errors
    /// [`ProfileError`] when the text is refused.
    pub fn device(&self) -> Result<DeviceProfile, ProfileError> {
        DeviceProfile::from_named_toml(&self.id, "the profile of the session", &self.text)
    }
}

/// A recorded run (G3): what it is made of and what it came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub rom: RomIdentity,
    pub profile: ProfileText,
    /// The whole flash the run started from, erased bytes included.
    pub flash: Vec<u8>,
    pub limits: Limits,
    /// The USB host at power-on.
    pub usb_host: UsbHost,
    /// The inputs the machine accepted, in the order it accepted them.
    pub inputs: Vec<AcceptedInput>,
    pub outcome: Outcome,
}

/// A field of the file, as a refusal names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Version,
    RomRevision,
    RomSha256,
    ProfileId,
    ProfileText,
    ProfileSha256,
    FlashSize,
    FlashSha256,
    DeflatedFlash,
    /// The SD card the run started with.
    SdCard,
    StrictMmio,
    MaxCycles,
    MaxTime,
    Cut,
    UsbHost,
    InputCount,
    /// Input `n`, counted from 0.
    Input(usize),
    /// The inputs of a console channel, all of them, by their bytes.
    ConsoleInputs(ConsoleChannel),
    /// The inputs of a console channel, all of them, by their number.
    ConsoleChunks(ConsoleChannel),
    HostChanges,
    /// The inputs of the board, all of them.
    BoardInputs,
    End,
    DeviceTime,
    Cycles,
    Instructions,
    TraceHash,
    FinalFlashSha256,
    /// The SD card the run ended with.
    FinalSdCard,
    ConsoleSha256(ConsoleChannel),
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Field::Version => f.write_str("the version line"),
            Field::RomRevision => f.write_str("the ROM revision"),
            Field::RomSha256 => f.write_str("the SHA-256 of the ROM"),
            Field::ProfileId => f.write_str("the profile id"),
            Field::ProfileText => f.write_str("the profile text"),
            Field::ProfileSha256 => f.write_str("the SHA-256 of the profile"),
            Field::FlashSize => f.write_str("the flash size"),
            Field::FlashSha256 => f.write_str("the SHA-256 of the starting flash"),
            Field::DeflatedFlash => f.write_str("the deflated starting flash"),
            Field::SdCard => f.write_str("the starting SD card"),
            Field::StrictMmio => f.write_str("--strict-mmio"),
            Field::MaxCycles => f.write_str("--max-cycles"),
            Field::MaxTime => f.write_str("--max-time"),
            Field::Cut => f.write_str("the cut"),
            Field::UsbHost => f.write_str("the USB host at power-on"),
            Field::InputCount => f.write_str("the number of inputs"),
            Field::Input(index) => write!(f, "input {index}"),
            Field::ConsoleInputs(channel) => {
                write!(f, "the bytes of the inputs of {}", channel.name())
            }
            Field::ConsoleChunks(channel) => {
                write!(f, "the number of inputs of {}", channel.name())
            }
            Field::HostChanges => f.write_str("the changes of the USB host"),
            Field::BoardInputs => f.write_str("the inputs of the board"),
            Field::End => f.write_str("the end"),
            Field::DeviceTime => f.write_str("the device time"),
            Field::Cycles => f.write_str("the cycles"),
            Field::Instructions => f.write_str("the instructions"),
            Field::TraceHash => f.write_str("the trace hash"),
            Field::FinalFlashSha256 => f.write_str("the SHA-256 of the final flash"),
            Field::FinalSdCard => f.write_str("the final SD card"),
            Field::ConsoleSha256(channel) => {
                write!(f, "the SHA-256 of console {}", channel.name())
            }
        }
    }
}

/// Why bytes are not a session this inkemu reads (G4). Each refusal names the field and the
/// cause; nothing is run.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    #[error(
        "the file holds more than {MAX_SESSION_BYTES} bytes, which no session reaches; it is not a session of inkemu"
    )]
    TooLong,
    #[error("the file does not begin with `{VERSION_LINE}`; it is not a session of inkemu")]
    NotASession,
    #[error(
        "the file is a session of format `{found}`, and this inkemu reads `inkemu-session 1` and `{VERSION_LINE}` only; replay it with the inkemu that recorded it"
    )]
    OtherVersion { found: String },
    #[error("the file ends inside {field}; it was cut short: record the run again")]
    Truncated { field: Field },
    #[error(
        "{field} is {value}, more than {max}; the file is damaged or was not written by inkemu"
    )]
    TooLarge { field: Field, value: u64, max: u64 },
    #[error(
        "{field} is {value}, which is no value of it; the file is damaged or was not written by inkemu"
    )]
    Invalid { field: Field, value: u64 },
    #[error(
        "{field} is not UTF-8 text from its byte {at} on; the file is damaged or was not written by inkemu"
    )]
    NotUtf8 { field: Field, at: usize },
    #[error(
        "the cut is at {at} cycles of the time base of device time, past the 584 years a replay can stop at; the file is damaged or was not written by inkemu",
        at = at.get()
    )]
    CutBeyondReplay { at: Cycles },
    #[error(
        "{field} computes to {computed}, not the {recorded} the session gives; the file was changed or damaged"
    )]
    Sha256Mismatch {
        field: Field,
        recorded: String,
        computed: String,
    },
    #[error("the starting flash cannot be inflated ({detail}); the file is damaged")]
    Inflate { detail: String },
    #[error(
        "{count} bytes follow the end of the deflated starting flash, inside its length; the file is damaged or was not written by inkemu"
    )]
    UnusedDeflatedBytes { count: usize },
    #[error(
        "the starting flash inflates to {found} bytes, not the {expected} the session gives; the file is damaged"
    )]
    InflatedSize { expected: u32, found: usize },
    #[error("{source}")]
    Profile { source: ProfileError },
    #[error(
        "the starting flash holds {flash} bytes, and the profile of the session gives its device {device}; the file is damaged"
    )]
    FlashOfAnotherDevice { flash: u32, device: u32 },
    #[error(
        "the end is {end} but the session {cut}; the file is damaged or was not written by inkemu",
        end = end.name(),
        cut = if *cut { "records a cut" } else { "records no cut" }
    )]
    EndAndCut { end: End, cut: bool },
    #[error(
        "{count} bytes follow the end of the session; the file is damaged or was not written by inkemu"
    )]
    TrailingBytes { count: usize },
}

/// Why a session cannot be written: what it holds is beyond what a reader takes, or the run it
/// would record is not whole.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WriteError {
    #[error(
        "the run received more input than a session holds ({MAX_CONSOLE_BYTES} bytes or {MAX_CONSOLE_CHUNKS} inputs on one console, {MAX_HOST_CHANGES} changes of the USB host or {MAX_BOARD_INPUTS} inputs of the board), so the machine's journal of it is incomplete; record a run that sends less, in fewer pieces"
    )]
    IncompleteJournal,
    #[error("the machine was told to keep no journal of its inputs, so its run cannot be recorded")]
    ForgoneJournal,
    #[error(
        "the run was cut at {at} cycles of the time base of device time, past the 584 years a replay can stop at; a session cannot hold it",
        at = at.get()
    )]
    CutBeyondReplay { at: Cycles },
    #[error("{field} is {value}, more than {max}, which a session cannot hold")]
    TooLarge { field: Field, value: u64, max: u64 },
    #[error(
        "the flash holds {flash} bytes, and the profile `{id}` gives its device {device}; record the run with its own profile"
    )]
    FlashOfAnotherDevice {
        id: String,
        flash: usize,
        device: u32,
    },
    #[error("{source}")]
    Profile { source: ProfileError },
    #[error(
        "the end is {end} but the limits {cut}; a cut run ends with its cut, and a run with a cut is cut",
        end = end.name(),
        cut = if *cut { "hold a cut" } else { "hold no cut" }
    )]
    EndAndCut { end: End, cut: bool },
    #[error("the starting flash cannot be deflated: {detail}")]
    Deflate { detail: String },
}

/// What the inputs of a session take of its bounds, as they are read or written.
#[derive(Debug, Default)]
struct InputBudget {
    console: [usize; 3],
    chunks: [usize; 3],
    host_changes: usize,
    board_inputs: usize,
}

impl InputBudget {
    /// Counts a console input of `len` bytes on `channel`, or names the bound it passes.
    fn console(&mut self, channel: ConsoleChannel, len: usize) -> Result<(), (Field, u64, u64)> {
        let at = place(channel);
        let chunks = self.chunks[at] + 1;
        if chunks > MAX_CONSOLE_CHUNKS {
            return Err((
                Field::ConsoleChunks(channel),
                chunks as u64,
                MAX_CONSOLE_CHUNKS as u64,
            ));
        }
        let bytes = self.console[at].saturating_add(len.max(1));
        if bytes > MAX_CONSOLE_BYTES {
            return Err((
                Field::ConsoleInputs(channel),
                bytes as u64,
                MAX_CONSOLE_BYTES as u64,
            ));
        }
        self.chunks[at] = chunks;
        self.console[at] = bytes;
        Ok(())
    }

    /// Counts a change of the USB host, or names the bound it passes.
    fn host_change(&mut self) -> Result<(), (Field, u64, u64)> {
        if self.host_changes >= MAX_HOST_CHANGES {
            return Err((
                Field::HostChanges,
                self.host_changes as u64 + 1,
                MAX_HOST_CHANGES as u64,
            ));
        }
        self.host_changes += 1;
        Ok(())
    }

    /// Counts an input of the board, or names the bound it passes.
    fn board_input(&mut self) -> Result<(), (Field, u64, u64)> {
        if self.board_inputs >= MAX_BOARD_INPUTS {
            return Err((
                Field::BoardInputs,
                self.board_inputs as u64 + 1,
                MAX_BOARD_INPUTS as u64,
            ));
        }
        self.board_inputs += 1;
        Ok(())
    }
}

const fn channel_tag(channel: ConsoleChannel) -> u8 {
    match channel {
        ConsoleChannel::Uart0 => 0,
        ConsoleChannel::UsbSerialJtag => 1,
        ConsoleChannel::Uart1 => 2,
    }
}
const HOST_TAG: u8 = 3;
const BOARD_TAG: u8 = 4;

const fn host_code(state: UsbHost) -> u8 {
    match state {
        UsbHost::Absent => 0,
        UsbHost::Attached => 1,
        UsbHost::Open => 2,
    }
}

const fn end_code(end: End) -> u8 {
    match end {
        End::CycleLimit => 0,
        End::TimeLimit => 1,
        End::Idle => 2,
        End::DeepSleep => 3,
        End::StrictMmio => 4,
        End::Cut => 5,
        End::PowerOff => 6,
    }
}

const fn cut_code(reason: CutReason) -> u8 {
    match reason {
        CutReason::Interrupted => 1,
        CutReason::ReaderGone => 2,
        CutReason::ConsoleError => 3,
    }
}

impl Session {
    /// The session of a run: `rom`, `profile` and `flash` it started from, `limits` that cut it,
    /// the inputs `journal` holds and the `outcome` it came to.
    ///
    /// # Errors
    /// [`WriteError::IncompleteJournal`] when the journal passed one of its bounds,
    /// [`WriteError::ForgoneJournal`] when the machine kept none, and the errors of
    /// [`Session::to_bytes`] for what a reader would refuse.
    pub fn record(
        rom: RomIdentity,
        profile: ProfileText,
        flash: Vec<u8>,
        limits: Limits,
        journal: &InputJournal,
        outcome: Outcome,
    ) -> Result<Session, WriteError> {
        if journal.is_forgone() {
            return Err(WriteError::ForgoneJournal);
        }
        if !journal.is_complete() {
            return Err(WriteError::IncompleteJournal);
        }
        let session = Session {
            rom,
            profile,
            flash,
            limits,
            usb_host: journal.usb_host_at_power_on(),
            inputs: journal.inputs().to_vec(),
            outcome,
        };
        session.check()?;
        Ok(session)
    }

    /// What a reader would refuse in this session, the content of the flash aside.
    fn check(&self) -> Result<(), WriteError> {
        let too_large = |field, value: usize, max: usize| WriteError::TooLarge {
            field,
            value: value as u64,
            max: max as u64,
        };
        if self.profile.id.len() > MAX_PROFILE_ID_BYTES {
            return Err(too_large(
                Field::ProfileId,
                self.profile.id.len(),
                MAX_PROFILE_ID_BYTES,
            ));
        }
        if self.profile.text.len() > MAX_PROFILE_TEXT_BYTES {
            return Err(too_large(
                Field::ProfileText,
                self.profile.text.len(),
                MAX_PROFILE_TEXT_BYTES,
            ));
        }
        let device = self
            .profile
            .device()
            .map_err(|source| WriteError::Profile { source })?;
        if self.flash.len() != device.flash.size() as usize {
            return Err(WriteError::FlashOfAnotherDevice {
                id: self.profile.id.clone(),
                flash: self.flash.len(),
                device: device.flash.size(),
            });
        }
        if (self.outcome.end == End::Cut) != self.limits.cut.is_some() {
            return Err(WriteError::EndAndCut {
                end: self.outcome.end,
                cut: self.limits.cut.is_some(),
            });
        }
        if let Some(cut) = self.limits.cut.filter(|cut| !cut.is_replayable()) {
            return Err(WriteError::CutBeyondReplay { at: cut.at });
        }
        let mut budget = InputBudget::default();
        for input in &self.inputs {
            let counted = match input {
                AcceptedInput::Console { channel, bytes, .. } => {
                    budget.console(*channel, bytes.len())
                }
                AcceptedInput::UsbHost { .. } => budget.host_change(),
                AcceptedInput::Board { .. } => budget.board_input(),
            };
            counted.map_err(|(field, value, max)| WriteError::TooLarge { field, value, max })?;
        }
        Ok(())
    }

    /// The session as the bytes of a file.
    ///
    /// # Errors
    /// [`WriteError`] when the session holds what a reader would refuse: a profile or a flash
    /// beyond their bounds or not of the same device, more inputs than a session holds, an end
    /// that does not agree with the cut, a cut no replay can stop at.
    pub fn to_bytes(&self) -> Result<Vec<u8>, WriteError> {
        self.check()?;
        self.encode()
    }

    /// The bytes of the session, its bounds unchecked: the tests of the reader write with it
    /// what a writer would refuse.
    fn encode(&self) -> Result<Vec<u8>, WriteError> {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        let deflated = encoder
            .write_all(&self.flash)
            .and_then(|()| encoder.finish())
            .map_err(|e| WriteError::Deflate {
                detail: e.to_string(),
            })?;
        if deflated.len() > MAX_DEFLATED_FLASH_BYTES {
            return Err(WriteError::TooLarge {
                field: Field::DeflatedFlash,
                value: deflated.len() as u64,
                max: MAX_DEFLATED_FLASH_BYTES as u64,
            });
        }
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.rom.revision.to_le_bytes());
        out.extend_from_slice(&self.rom.sha256);
        // The bounds were checked above: the lengths fit their fields.
        let id = self.profile.id.as_bytes();
        out.push(u8::try_from(id.len()).unwrap_or(u8::MAX));
        out.extend_from_slice(id);
        let text = self.profile.text.as_bytes();
        out.extend_from_slice(&u32::try_from(text.len()).unwrap_or(u32::MAX).to_le_bytes());
        out.extend_from_slice(text);
        out.extend_from_slice(&Sha256::digest(text));
        let flash_size = u32::try_from(self.flash.len()).unwrap_or(u32::MAX);
        out.extend_from_slice(&flash_size.to_le_bytes());
        out.extend_from_slice(&Sha256::digest(&self.flash));
        let deflated_len = u32::try_from(deflated.len()).unwrap_or(u32::MAX);
        out.extend_from_slice(&deflated_len.to_le_bytes());
        out.extend_from_slice(&deflated);
        // No SD card: the place the card of sub-project 2e takes in the format.
        out.push(0);
        out.push(u8::from(self.limits.strict_mmio));
        for limit in [self.limits.max_cycles, self.limits.max_time_nanos] {
            match limit {
                Some(value) => {
                    out.push(1);
                    out.extend_from_slice(&value.to_le_bytes());
                }
                None => out.push(0),
            }
        }
        match self.limits.cut {
            Some(cut) => {
                out.push(cut_code(cut.reason));
                out.extend_from_slice(&cut.at.get().to_le_bytes());
            }
            None => out.push(0),
        }
        out.push(host_code(self.usb_host));
        let count = u32::try_from(self.inputs.len()).unwrap_or(u32::MAX);
        out.extend_from_slice(&count.to_le_bytes());
        for input in &self.inputs {
            match input {
                AcceptedInput::Console { channel, at, bytes } => {
                    out.push(channel_tag(*channel));
                    out.extend_from_slice(&at.get().to_le_bytes());
                    let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
                    out.extend_from_slice(&len.to_le_bytes());
                    out.extend_from_slice(bytes);
                }
                AcceptedInput::UsbHost { at, state } => {
                    out.push(HOST_TAG);
                    out.extend_from_slice(&at.get().to_le_bytes());
                    out.push(host_code(*state));
                }
                AcceptedInput::Board { at, input } => {
                    out.push(BOARD_TAG);
                    out.extend_from_slice(&at.get().to_le_bytes());
                    out.extend_from_slice(&input.target.to_le_bytes());
                    out.extend_from_slice(&input.value.to_le_bytes());
                }
            }
        }
        let outcome = &self.outcome;
        out.push(end_code(outcome.end));
        for value in [
            outcome.device_time.get(),
            outcome.cycles,
            outcome.instructions,
            outcome.trace_hash,
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&outcome.flash_sha256);
        // No SD card at the end either.
        out.push(0);
        for digest in &outcome.consoles {
            out.extend_from_slice(digest);
        }
        Ok(out)
    }

    /// The session these bytes hold.
    ///
    /// # Errors
    /// [`ReadError`], naming the field and the cause: bytes of another version or of no
    /// session, cut short, a length past its bound, a value no field takes, a SHA-256 that does
    /// not match, a flash that does not inflate to its size or is not the size of its device, a
    /// profile refused, bytes after the end.
    pub fn from_bytes(bytes: &[u8]) -> Result<Session, ReadError> {
        if bytes.len() > MAX_SESSION_BYTES {
            return Err(ReadError::TooLong);
        }
        let mut reader = Reader {
            bytes,
            at: 0,
            format: 2,
        };
        reader.version()?;
        let rom = RomIdentity {
            revision: reader.u16(Field::RomRevision)?,
            sha256: reader.sha256(Field::RomSha256)?,
        };
        let profile = reader.profile()?;
        // The device first: the flash is inflated into its size, and no other.
        let device = profile
            .device()
            .map_err(|source| ReadError::Profile { source })?;
        let flash = reader.flash(device.flash.size())?;
        reader.no_sd_card(Field::SdCard)?;
        let strict_mmio = match reader.u8(Field::StrictMmio)? {
            0 => false,
            1 => true,
            other => {
                return Err(ReadError::Invalid {
                    field: Field::StrictMmio,
                    value: other.into(),
                });
            }
        };
        let max_cycles = reader.optional_u64(Field::MaxCycles)?;
        let max_time_nanos = reader.optional_u64(Field::MaxTime)?;
        let cut = reader.cut()?;
        let limits = Limits {
            strict_mmio,
            max_cycles,
            max_time_nanos,
            cut,
        };
        let usb_host = reader.usb_host(Field::UsbHost)?;
        let inputs = reader.inputs()?;
        let outcome = reader.outcome()?;
        if (outcome.end == End::Cut) != cut.is_some() {
            return Err(ReadError::EndAndCut {
                end: outcome.end,
                cut: cut.is_some(),
            });
        }
        let left = bytes.len() - reader.at;
        if left > 0 {
            return Err(ReadError::TrailingBytes { count: left });
        }
        Ok(Session {
            rom,
            profile,
            flash,
            limits,
            usb_host,
            inputs,
            outcome,
        })
    }
}

/// The bytes of a file, read field by field.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    /// The format of the file, 1 or 2, once its version line is read.
    format: u8,
}

impl<'a> Reader<'a> {
    /// The next `len` bytes, or the field they would end in.
    fn take(&mut self, len: usize, field: Field) -> Result<&'a [u8], ReadError> {
        let end = self
            .at
            .checked_add(len)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(ReadError::Truncated { field })?;
        let taken = &self.bytes[self.at..end];
        self.at = end;
        Ok(taken)
    }

    fn array<const N: usize>(&mut self, field: Field) -> Result<[u8; N], ReadError> {
        let mut array = [0u8; N];
        array.copy_from_slice(self.take(N, field)?);
        Ok(array)
    }

    fn u8(&mut self, field: Field) -> Result<u8, ReadError> {
        Ok(self.array::<1>(field)?[0])
    }

    fn u16(&mut self, field: Field) -> Result<u16, ReadError> {
        self.array(field).map(u16::from_le_bytes)
    }

    fn u32(&mut self, field: Field) -> Result<u32, ReadError> {
        self.array(field).map(u32::from_le_bytes)
    }

    fn u64(&mut self, field: Field) -> Result<u64, ReadError> {
        self.array(field).map(u64::from_le_bytes)
    }

    fn sha256(&mut self, field: Field) -> Result<[u8; 32], ReadError> {
        self.array(field)
    }

    /// A length of `field`, at most `max`.
    fn length(&mut self, field: Field, max: usize) -> Result<usize, ReadError> {
        let value = self.u32(field)?;
        let len = usize::try_from(value).unwrap_or(usize::MAX);
        if len > max {
            return Err(ReadError::TooLarge {
                field,
                value: value.into(),
                max: max as u64,
            });
        }
        Ok(len)
    }

    fn version(&mut self) -> Result<(), ReadError> {
        for (magic, format) in [(MAGIC, 2), (MAGIC_1, 1)] {
            if self.bytes.starts_with(magic) {
                self.at = magic.len();
                self.format = format;
                return Ok(());
            }
        }
        if MAGIC.starts_with(self.bytes) || MAGIC_1.starts_with(self.bytes) {
            return Err(ReadError::Truncated {
                field: Field::Version,
            });
        }
        if self.bytes.starts_with(MAGIC_PREFIX) {
            // Another version: its line is shown, at most a few bytes of it, as text.
            let line = self.bytes[..self.bytes.len().min(MAX_VERSION_LINE_BYTES)]
                .split(|&b| b == b'\n')
                .next()
                .unwrap_or_default();
            return Err(ReadError::OtherVersion {
                found: String::from_utf8_lossy(line)
                    .chars()
                    .map(|c| if c.is_control() { '?' } else { c })
                    .collect(),
            });
        }
        Err(ReadError::NotASession)
    }

    /// The presence of an SD card in a session of the format 2: this inkemu holds none, so 0 is
    /// the one value the field takes. A session of the format 1 has no such field.
    fn no_sd_card(&mut self, field: Field) -> Result<(), ReadError> {
        if self.format < 2 {
            return Ok(());
        }
        match self.u8(field)? {
            0 => Ok(()),
            other => Err(ReadError::Invalid {
                field,
                value: other.into(),
            }),
        }
    }

    fn text(&mut self, len: usize, field: Field) -> Result<String, ReadError> {
        let bytes = self.take(len, field)?;
        String::from_utf8(bytes.to_vec()).map_err(|e| ReadError::NotUtf8 {
            field,
            at: e.utf8_error().valid_up_to(),
        })
    }

    fn profile(&mut self) -> Result<ProfileText, ReadError> {
        let id_len = usize::from(self.u8(Field::ProfileId)?);
        if id_len > MAX_PROFILE_ID_BYTES {
            return Err(ReadError::TooLarge {
                field: Field::ProfileId,
                value: id_len as u64,
                max: MAX_PROFILE_ID_BYTES as u64,
            });
        }
        let id = self.text(id_len, Field::ProfileId)?;
        let text_len = self.length(Field::ProfileText, MAX_PROFILE_TEXT_BYTES)?;
        let text = self.text(text_len, Field::ProfileText)?;
        let recorded = self.sha256(Field::ProfileSha256)?;
        let computed: [u8; 32] = Sha256::digest(text.as_bytes()).into();
        if computed != recorded {
            // The text and its digest disagree; which of the two was damaged, nothing tells.
            return Err(ReadError::Sha256Mismatch {
                field: Field::ProfileSha256,
                recorded: hex(&recorded),
                computed: hex(&computed),
            });
        }
        Ok(ProfileText { id, text })
    }

    /// The starting flash, which the profile gives `device_size` bytes.
    fn flash(&mut self, device_size: u32) -> Result<Vec<u8>, ReadError> {
        let size = self.u32(Field::FlashSize)?;
        if size > MAX_FLASH_BYTES {
            return Err(ReadError::TooLarge {
                field: Field::FlashSize,
                value: size.into(),
                max: MAX_FLASH_BYTES.into(),
            });
        }
        if size < MIN_FLASH_BYTES {
            return Err(ReadError::Invalid {
                field: Field::FlashSize,
                value: size.into(),
            });
        }
        if size != device_size {
            return Err(ReadError::FlashOfAnotherDevice {
                flash: size,
                device: device_size,
            });
        }
        let recorded = self.sha256(Field::FlashSha256)?;
        let deflated_len = self.length(Field::DeflatedFlash, MAX_DEFLATED_FLASH_BYTES)?;
        let deflated = self.take(deflated_len, Field::DeflatedFlash)?;
        let flash = inflate(deflated, size)?;
        let computed: [u8; 32] = Sha256::digest(&flash).into();
        if computed != recorded {
            return Err(ReadError::Sha256Mismatch {
                field: Field::FlashSha256,
                recorded: hex(&recorded),
                computed: hex(&computed),
            });
        }
        Ok(flash)
    }

    fn optional_u64(&mut self, field: Field) -> Result<Option<u64>, ReadError> {
        match self.u8(field)? {
            0 => Ok(None),
            1 => self.u64(field).map(Some),
            other => Err(ReadError::Invalid {
                field,
                value: other.into(),
            }),
        }
    }

    fn cut(&mut self) -> Result<Option<Cut>, ReadError> {
        let reason = match self.u8(Field::Cut)? {
            0 => return Ok(None),
            1 => CutReason::Interrupted,
            2 => CutReason::ReaderGone,
            3 => CutReason::ConsoleError,
            other => {
                return Err(ReadError::Invalid {
                    field: Field::Cut,
                    value: other.into(),
                });
            }
        };
        let cut = Cut {
            at: Cycles(self.u64(Field::Cut)?),
            reason,
        };
        if !cut.is_replayable() {
            return Err(ReadError::CutBeyondReplay { at: cut.at });
        }
        Ok(Some(cut))
    }

    fn usb_host(&mut self, field: Field) -> Result<UsbHost, ReadError> {
        let code = self.u8(field)?;
        UsbHost::ALL
            .into_iter()
            .find(|state| host_code(*state) == code)
            .ok_or(ReadError::Invalid {
                field,
                value: code.into(),
            })
    }

    fn inputs(&mut self) -> Result<Vec<AcceptedInput>, ReadError> {
        let count = self.length(Field::InputCount, MAX_INPUTS)?;
        let mut budget = InputBudget::default();
        // As many entries as the count says, bounded by MAX_INPUTS and by what the bytes left can
        // hold (an input takes ten bytes at least): the vector never grows past what it holds.
        let room = (self.bytes.len() - self.at) / HOST_INPUT_BYTES;
        let mut inputs = Vec::with_capacity(count.min(room));
        // At most MAX_INPUTS turns; each takes ten bytes of the file at least, so a count larger
        // than the file allows ends early, on the input it would be cut in.
        for index in 0..count {
            let field = Field::Input(index);
            let tag = self.u8(field)?;
            let at = Cycles(self.u64(field)?);
            let input = if tag == HOST_TAG {
                budget.host_change().map_err(too_large)?;
                let state = self.usb_host(field)?;
                AcceptedInput::UsbHost { at, state }
            } else if tag == BOARD_TAG && self.format >= 2 {
                budget.board_input().map_err(too_large)?;
                let target = self.u16(field)?;
                let value = self.u32(field)?;
                AcceptedInput::Board {
                    at,
                    input: BoardInput { target, value },
                }
            } else {
                let channel = ConsoleChannel::ALL
                    .into_iter()
                    .find(|channel| channel_tag(*channel) == tag)
                    .ok_or(ReadError::Invalid {
                        field,
                        value: tag.into(),
                    })?;
                let len = self.length(field, MAX_CONSOLE_BYTES)?;
                budget.console(channel, len).map_err(too_large)?;
                let bytes = self.take(len, field)?.to_vec();
                AcceptedInput::Console { channel, at, bytes }
            };
            inputs.push(input);
        }
        Ok(inputs)
    }

    fn outcome(&mut self) -> Result<Outcome, ReadError> {
        let code = self.u8(Field::End)?;
        let end = End::ALL
            .into_iter()
            .find(|end| end_code(*end) == code)
            .ok_or(ReadError::Invalid {
                field: Field::End,
                value: code.into(),
            })?;
        let device_time = Cycles(self.u64(Field::DeviceTime)?);
        let cycles = self.u64(Field::Cycles)?;
        let instructions = self.u64(Field::Instructions)?;
        let trace_hash = self.u64(Field::TraceHash)?;
        let flash_sha256 = self.sha256(Field::FinalFlashSha256)?;
        self.no_sd_card(Field::FinalSdCard)?;
        let mut consoles = [[0u8; 32]; 3];
        for channel in ConsoleChannel::ALL {
            consoles[place(channel)] = self.sha256(Field::ConsoleSha256(channel))?;
        }
        Ok(Outcome {
            end,
            device_time,
            cycles,
            instructions,
            trace_hash,
            flash_sha256,
            consoles,
        })
    }
}

fn too_large((field, value, max): (Field, u64, u64)) -> ReadError {
    ReadError::TooLarge { field, value, max }
}

/// The flash the deflated stream `deflated` holds, `size` bytes exactly, the whole stream used: a
/// stream that does not inflate to `size` bytes, that stops before its last block, or that is
/// followed by more bytes is refused.
fn inflate(deflated: &[u8], size: u32) -> Result<Vec<u8>, ReadError> {
    let expected = size as usize;
    // `size` was checked against `MAX_FLASH_BYTES` and the device of the profile before: the
    // capacity is bounded by a constant. The byte past it tells a stream that inflates further.
    let mut flash = Vec::with_capacity(expected + 1);
    let mut decompress = Decompress::new(false);
    let status = decompress
        .decompress_vec(deflated, &mut flash, FlushDecompress::Finish)
        .map_err(|e| ReadError::Inflate {
            detail: e.to_string(),
        })?;
    if flash.len() != expected {
        return Err(ReadError::InflatedSize {
            expected: size,
            found: flash.len(),
        });
    }
    if status != Status::StreamEnd {
        return Err(ReadError::Inflate {
            detail: "the deflated stream stops before its last block".to_owned(),
        });
    }
    let used = usize::try_from(decompress.total_in()).unwrap_or(usize::MAX);
    if used < deflated.len() {
        return Err(ReadError::UnusedDeflatedBytes {
            count: deflated.len() - used,
        });
    }
    Ok(flash)
}

#[cfg(test)]
mod tests;
