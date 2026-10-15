//! A machine on a thread of its own, driven by commands (design of sub-project 2c, C3, C4, C6,
//! C10, C14; plan rulings 2c-R4 to 2c-R9).
//!
//! [`power_on`] starts a run as `inkemu run` does, in the same order of refusals, and
//! [`open_replay`] a replay as `inkemu replay` does; each gives a [`Launch`], which
//! [`Emulator::start`] runs on a new thread. The thread builds every machine itself (a machine
//! is not `Send`), serves the commands between two passes and answers each, paces the passes on
//! the host clock in real time, and tells an observer, in order, what the device prints, what
//! the run says, when the panel refreshed, where the run is, and how each life ended.
//!
//! **Lives.** A reset is a new power-on with the flash where the last life left it, as the reset
//! button of a device does. Every life ends the same way: the flash file is written and read
//! again for the next life, a recorded session is written, a replay gives its verdict.
use crate::catalog::{self, Look, ProfileRecipe};
use crate::engine::{
    Engine, EngineError, Halt, Level, LifeEnd, Limits, MachineStatus, Mode, Notice, Output, State,
};
use crate::flash_state::{FlashStateError, Input, RunFlash, Seed};
use crate::launch::{self, FlashRequest, RecordPathError, RunRequest, StartError};
use crate::ports::{Life, PanelFrame, PowerOnError, Recipe, RefreshKind};
use crate::record::{self, Recorder, Verdict};
use crate::rom_store::{self, RomStoreError};
use crate::run::RunEnd;
use crate::session_file::{self, SessionFileError};
use crate::term::sanitize;
use boards::{DeviceProfile, MachineOptions, ProfileError};
use session::{ProfileText, RomIdentity, Session};
use soc_esp32c3::{ConsoleChannel, RomImage, UsbHost};
use std::io::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How fast device time goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Speed {
    /// Device time follows the host clock, when the host is fast enough.
    #[default]
    RealTime,
    /// As fast as the host runs it.
    Fastest,
}

/// How long the thread waits for a command when the machine is where the host clock allows.
const PACE_WAIT: Duration = Duration::from_millis(5);
/// The longest the device may lag behind the host clock before the pace forgets the lag: a host
/// that cannot keep up does not make the device run fast later to catch up.
const MAX_LAG_NANOS: u64 = 50_000_000;
/// The shortest time between two statuses while the machine runs (2c-R6).
const STATUS_EVERY: Duration = Duration::from_millis(100);
/// The time over which the speed of the run is measured.
const SPEED_WINDOW: Duration = Duration::from_secs(1);
/// The shortest time between two console updates while text keeps coming (2c-R15): a firmware
/// that prints on every pass would otherwise send an update per pass, hundreds a second.
const CONSOLE_EVERY: Duration = Duration::from_millis(40);

/// Console text waiting to be sent, joined while it stays on one channel, in the order it came.
#[derive(Debug, Default)]
struct ConsoleBatch {
    texts: Vec<(ConsoleChannel, String)>,
    last_sent: Option<Instant>,
}

impl ConsoleBatch {
    fn push(&mut self, channel: ConsoleChannel, text: String) {
        match self.texts.last_mut() {
            Some((last, held)) if *last == channel => held.push_str(&text),
            _ => self.texts.push((channel, text)),
        }
    }

    /// Whether text waits and the last sending is at least [`CONSOLE_EVERY`] before `now`.
    fn due(&self, now: Instant) -> bool {
        !self.texts.is_empty()
            && self
                .last_sent
                .is_none_or(|last| now.saturating_duration_since(last) >= CONSOLE_EVERY)
    }

    /// The text waiting, sent at `now`.
    fn take(&mut self, now: Instant) -> Vec<(ConsoleChannel, String)> {
        if !self.texts.is_empty() {
            self.last_sent = Some(now);
        }
        std::mem::take(&mut self.texts)
    }
}

/// What the interface asks a run to be: the options of `inkemu run`.
#[derive(Clone, Debug, Default)]
pub struct Setup {
    /// The id of a built-in profile: `x4`, `x3`.
    pub device: String,
    /// A whole flash image (`--flash`).
    pub image: Option<PathBuf>,
    /// An application to compose a flash around (`--firmware`).
    pub firmware: Option<PathBuf>,
    /// The bootloader of that flash (`--bootloader`).
    pub bootloader: Option<PathBuf>,
    /// Where the flash is kept between runs (`--flash-file`).
    pub flash_file: Option<PathBuf>,
    /// Start again from the image and replace the flash file (`--reset-flash`).
    pub reset_flash: bool,
    /// A directory holding the ROM, read only (`--rom-dir`); the per-user cache otherwise.
    pub rom_dir: Option<PathBuf>,
    /// The USB host at every power-on.
    pub usb_host: UsbHost,
    pub strict_mmio: bool,
    /// Record the first life into this file (`--record`).
    pub record: Option<PathBuf>,
    pub limits: Limits,
    /// The console to show first; the profile's otherwise.
    pub console: Option<ConsoleChannel>,
}

/// Why a run or a replay could not start. The messages are those of the command line; the
/// variant says what was refused, for an interface with its own words ([`LaunchError::kind`]).
#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("{0}")]
    Profile(#[from] ProfileError),
    #[error("{0}")]
    RecordPath(#[from] RecordPathError),
    #[error("{0}")]
    FlashFile(#[from] FlashStateError),
    #[error("{0}")]
    Start(#[from] StartError),
    #[error("{0}")]
    Rom(#[from] RomStoreError),
    #[error("{0}")]
    Session(#[from] SessionFileError),
    #[error("{0}")]
    Replay(String),
    #[error("{0}")]
    PowerOn(#[from] PowerOnError),
}

impl LaunchError {
    /// A stable name of what was refused.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            LaunchError::Profile(_) => "profile",
            LaunchError::RecordPath(_) => "recordPath",
            LaunchError::FlashFile(_) => "flashFile",
            LaunchError::Start(StartError::Flash(_)) => "flashImage",
            LaunchError::Start(StartError::Firmware(_)) => "firmware",
            LaunchError::Rom(_) => "rom",
            LaunchError::Session(_) => "session",
            LaunchError::Replay(_) => "replay",
            LaunchError::PowerOn(_) => "powerOn",
        }
    }
}

/// The paths of a run, kept to judge a session path given later.
#[derive(Clone, Debug, Default)]
struct RunPaths {
    device: String,
    image: Option<PathBuf>,
    firmware: Option<PathBuf>,
    bootloader: Option<PathBuf>,
    flash_file: Option<PathBuf>,
    rom_dir: Option<PathBuf>,
}

impl RunPaths {
    fn request(&self) -> FlashRequest<'_> {
        FlashRequest {
            image: self.image.as_deref(),
            firmware: self.firmware.as_deref(),
            bootloader: self.bootloader.as_deref(),
            file: self.flash_file.as_deref(),
            reset: false,
        }
    }
}

/// What a recorded life is recorded with.
#[derive(Clone, Debug)]
pub struct Recording {
    pub rom: RomIdentity,
    pub profile: ProfileText,
    /// The `--strict-mmio` of the run, which a session keeps with its limits.
    pub strict_mmio: bool,
}

/// The flash of a run across its lives: the flash file, locked, and the content of the next
/// power-on (ADR 0004, design C4).
#[derive(Debug)]
pub struct FlashKeeper {
    file: Option<PathBuf>,
    size: u32,
    run_flash: Option<RunFlash>,
    next: Option<Vec<u8>>,
}

impl FlashKeeper {
    /// Takes the flash file `file`, if any, for a device whose flash is `size` bytes, starting
    /// from `seed` unless the file exists ([`RunFlash::begin`]).
    ///
    /// # Errors
    /// [`FlashStateError`] when the flash file cannot be used.
    pub fn begin(
        file: Option<PathBuf>,
        reset: bool,
        seed: Option<Seed>,
        size: u32,
    ) -> Result<FlashKeeper, FlashStateError> {
        let (run_flash, next) = RunFlash::begin(file.as_deref(), reset, seed, size)?;
        Ok(FlashKeeper {
            file,
            size,
            run_flash: Some(run_flash),
            next,
        })
    }

    /// A flash with no file: erased, or `content`.
    #[must_use]
    pub fn without_file(content: Option<Vec<u8>>, size: u32) -> FlashKeeper {
        FlashKeeper {
            file: None,
            size,
            run_flash: None,
            next: content,
        }
    }

    /// The content of the next power-on, taken once. A content kept in memory (the start of the
    /// run, a life that could not begin, a save that failed) is newer than the flash file: with
    /// a file, it replaces the file at the end of the life; otherwise the file is read again.
    fn take_next(&mut self) -> Result<Option<Vec<u8>>, FlashStateError> {
        if self.run_flash.is_some() {
            return Ok(self.next.take());
        }
        let mut seed = self.next.take().map(|content| Seed {
            input: Input::Flash,
            content,
        });
        if seed.is_none() && self.file.is_none() {
            return Ok(None);
        }
        let reset = seed.is_some();
        let begun = RunFlash::begin_keeping(self.file.as_deref(), reset, &mut seed, self.size);
        // A file that cannot be taken now (its directory, its lock) must not lose the content:
        // the next power-on tries again from it.
        self.next = seed.map(|seed| seed.content);
        let (run_flash, content) = begun?;
        self.run_flash = Some(run_flash);
        Ok(content)
    }

    /// Gives back the content of a power-on that did not happen: the next one starts from it.
    fn abandon(&mut self, content: Option<Vec<u8>>) {
        self.run_flash = None;
        self.next = content;
    }

    /// Keeps `content`, the flash of a life that stopped on an internal error: writes it to the
    /// flash file, if the run has one. `false` when the file could not be written.
    fn rescue(&mut self, content: Vec<u8>, log: &mut dyn std::io::Write) -> bool {
        let saved = self
            .run_flash
            .take()
            .is_none_or(|run_flash| run_flash.save_content(&content, log));
        self.next = Some(content);
        saved
    }

    /// Ends a life for its flash: says what it started from and what became of it, writes the
    /// flash file, and keeps the content for the next life. `false` when the file could not be
    /// written: the content is then kept in memory, and written at the end of the next life.
    fn finish(&mut self, ended: &LifeEnd, log: &mut dyn std::io::Write) -> bool {
        let saved = self
            .run_flash
            .take()
            .is_none_or(|run_flash| run_flash.finish(&ended.machine, log));
        // With a file that holds it, the next life reads it again; otherwise it starts from what
        // this one left.
        self.next = if self.file.is_some() && saved {
            None
        } else {
            Some(ended.machine.flash_contents())
        };
        saved
    }
}

/// What a run keeps beside its recipe. One value per emulator, made once: the size of its larger
/// variant costs nothing worth a box.
#[allow(clippy::large_enum_variant)]
enum Plan {
    Run {
        flash: FlashKeeper,
        /// The recording of the first life, then of the life a record command asks for.
        record: Option<PathBuf>,
        recording: Option<Recording>,
        paths: RunPaths,
        limits: Limits,
    },
    Replay {
        session: Box<Session>,
        name: String,
    },
}

/// A run or a replay ready to start on its thread.
pub struct Launch {
    recipe: Box<dyn Recipe>,
    plan: Plan,
    /// What starting it said: the command line's warnings and its first line.
    pub notices: Vec<Notice>,
    /// The panel and the buttons of the device.
    pub look: Look,
    /// The console to show first, and the name of the device.
    pub console: ConsoleChannel,
    pub device_name: String,
}

impl std::fmt::Debug for Launch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Launch")
            .field("notices", &self.notices)
            .field("look", &self.look)
            .field("console", &self.console)
            .field("device_name", &self.device_name)
            .finish_non_exhaustive()
    }
}

impl Launch {
    /// A run of `recipe` with `flash` and `limits`, its first life unrecorded; `recording` lets a
    /// record command record a later life, on the built-in device it names.
    #[must_use]
    pub fn new(
        recipe: Box<dyn Recipe>,
        flash: FlashKeeper,
        limits: Limits,
        look: Look,
        recording: Option<Recording>,
    ) -> Launch {
        let paths = RunPaths {
            device: recording
                .as_ref()
                .map(|recording| recording.profile.id.clone())
                .unwrap_or_default(),
            ..RunPaths::default()
        };
        Launch {
            recipe,
            plan: Plan::Run {
                flash,
                record: None,
                recording,
                paths,
                limits,
            },
            notices: Vec::new(),
            look,
            console: ConsoleChannel::Uart0,
            device_name: String::new(),
        }
    }

    /// The replay of `session`, named `name`, whose machine `recipe` builds.
    #[must_use]
    pub fn replay(recipe: Box<dyn Recipe>, session: Session, name: String, look: Look) -> Launch {
        Launch {
            recipe,
            plan: Plan::Replay {
                session: Box::new(session),
                name,
            },
            notices: Vec::new(),
            look,
            console: ConsoleChannel::Uart0,
            device_name: String::new(),
        }
    }
}

/// Starts a run as `inkemu run` does: the same refusals in the same order, the same lines.
///
/// # Errors
/// [`LaunchError`] for the first refusal.
pub fn power_on(setup: &Setup) -> Result<Launch, LaunchError> {
    let mut log = Vec::new();
    let profile = launch::profile(&setup.device)?;
    let request = RunRequest {
        max_cycles: setup.limits.max_cycles,
        max_device_nanos: setup.limits.max_device_nanos,
        console: setup.console,
    };
    let (options, console_note) = request.resolve(&profile, setup.usb_host);
    let entry = rom_store::rom_entry_for(profile.chip_revision);
    let paths = RunPaths {
        device: setup.device.clone(),
        image: setup.image.clone(),
        firmware: setup.firmware.clone(),
        bootloader: setup.bootloader.clone(),
        flash_file: setup.flash_file.clone(),
        rom_dir: setup.rom_dir.clone(),
    };
    let flash = FlashRequest {
        reset: setup.reset_flash,
        ..paths.request()
    };
    // The text of the profile it gives is the one every recorded life carries (below).
    launch::refuse_record_path(
        setup.record.as_deref(),
        &flash,
        None,
        (setup.rom_dir.as_deref(), entry),
        &setup.device,
    )?;
    launch::refuse_the_images_as_flash_file(&flash)?;
    let fresh = launch::starting_flash(&profile, &flash)?;
    let size = profile.flash.size();
    let keeper = FlashKeeper::begin(
        setup.flash_file.clone(),
        setup.reset_flash,
        fresh.seed,
        size,
    )?;
    for warning in &fresh.warnings {
        log.extend_from_slice(format!("warning: {warning}\n").as_bytes());
    }
    let flash_note = match &keeper.run_flash {
        Some(run_flash) => launch::flash_note(flash.file, run_flash, &fresh.note, size),
        None => format!("flash: {}", fresh.note),
    };
    let rom = launch::load_rom(setup.rom_dir.clone(), entry, &mut log)?;
    let machine_options = MachineOptions {
        strict_mmio: setup.strict_mmio,
        trace_hash: false,
        usb_host: setup.usb_host,
    };
    // The first line names the timing model of a machine of this device.
    let probe = profile.instantiate(rom.clone(), machine_options);
    if let Some(run_flash) = &keeper.run_flash {
        run_flash.accept(&mut log)?;
    }
    let header = launch::header(&profile, entry, &probe, &console_note);
    drop(probe);
    log.extend_from_slice(format!("{header}; {flash_note}\n").as_bytes());
    let recording = Recording {
        rom: RomIdentity::of(entry.rom_revision, &rom),
        profile: ProfileText {
            id: profile.id.clone(),
            text: boards::builtin_toml(&profile.id)?.to_owned(),
        },
        strict_mmio: setup.strict_mmio,
    };
    let look = catalog::look(&profile.id);
    let device_name = sanitize(&profile.name);
    Ok(Launch {
        recipe: Box::new(ProfileRecipe {
            profile,
            rom,
            options: machine_options,
        }),
        plan: Plan::Run {
            flash: keeper,
            record: setup.record.clone(),
            recording: Some(recording),
            paths,
            limits: setup.limits,
        },
        notices: Notice::from_log(&log),
        look,
        console: options.console,
        device_name,
    })
}

/// Powers on the machine of a replay, on the emulator's thread.
struct ReplayRecipe {
    session: Box<Session>,
    rom: RomImage,
    rom_revision: u16,
    profile: DeviceProfile,
}

impl Recipe for ReplayRecipe {
    fn power_on(&mut self, _flash: Option<Vec<u8>>, _recorded: bool) -> Result<Life, PowerOnError> {
        let machine = record::start_replay(&self.session, self.rom.clone(), self.rom_revision)
            .map_err(PowerOnError::Replay)?;
        let (panel, buttons) = catalog::ports_for(&self.profile);
        Ok(Life {
            machine,
            panel,
            buttons,
        })
    }
}

/// Opens the session at `path` for a replay, as `inkemu replay` does: the session is read and
/// refused before the ROM is looked for, and a ROM other than the session's is refused before
/// anything runs.
///
/// # Errors
/// [`LaunchError`] for the first refusal.
pub fn open_replay(
    path: &Path,
    rom_dir: Option<PathBuf>,
    console: Option<ConsoleChannel>,
) -> Result<Launch, LaunchError> {
    let mut log = Vec::new();
    let session = session_file::read_session(path)?;
    let profile = session
        .profile
        .device()
        .map_err(|e| LaunchError::Replay(sanitize(&e.to_string())))?;
    let entry = rom_store::rom_entry_for(profile.chip_revision);
    let rom = launch::load_rom(rom_dir, entry, &mut log)?;
    // Refused here, before the thread, for what a replay cannot start on.
    let probe = record::start_replay(&session, rom.clone(), entry.rom_revision)
        .map_err(LaunchError::Replay)?;
    let (shown, origin) = launch::shown_console(&profile, console);
    let header = launch::header(
        &profile,
        entry,
        &probe,
        &launch::console_note(shown, origin, session.usb_host),
    );
    drop(probe);
    let name = launch::file_name(path);
    log.extend_from_slice(
        format!(
            "{header}; replaying {name}: {} inputs\n",
            session.inputs.len()
        )
        .as_bytes(),
    );
    if let Some(cut) = session.limits.cut {
        log.extend_from_slice(
            format!(
                "inkemu: the session was cut by {} at {} us of device time when it was recorded; its replay stops there\n",
                cut.reason.name(),
                soc_esp32c3::clock::TIME_BASE.cycles_to_nanos(cut.at) / 1_000
            )
            .as_bytes(),
        );
    }
    let look = catalog::look(&profile.id);
    let device_name = sanitize(&profile.name);
    let session = Box::new(session);
    Ok(Launch {
        recipe: Box::new(ReplayRecipe {
            session: session.clone(),
            rom,
            rom_revision: entry.rom_revision,
            profile,
        }),
        plan: Plan::Replay { session, name },
        notices: Notice::from_log(&log),
        look,
        console: shown,
        device_name,
    })
}

/// What an interface asks of a running emulator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Run, or go on after a pause.
    Run,
    Pause,
    /// Pause, then execute one instruction.
    Step,
    /// A new power-on, with the flash where this life leaves it.
    Reset,
    /// End the last life and the thread.
    Stop,
    Speed(Speed),
    Console {
        channel: ConsoleChannel,
        bytes: Vec<u8>,
    },
    UsbHost(UsbHost),
    Button {
        id: String,
        pressed: bool,
    },
    /// A new power-on, recorded into `path` (a session begins at power-on).
    Record {
        path: PathBuf,
    },
}

/// Why a command was not done.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("the emulator has stopped")]
    Stopped,
    #[error("the run has ended; reset the device to power it on again")]
    NoLife,
    #[error("{0}")]
    Engine(#[from] EngineError),
    #[error("{0}")]
    RecordPath(#[from] RecordPathError),
    #[error("a replay cannot be recorded")]
    ReplayNotRecorded,
    #[error("this run cannot be recorded: it was not started from a device profile and its ROM")]
    NotRecordable,
    #[error("{0}")]
    PowerOn(String),
    /// A command given on the emulator's own thread, by its observer: answering it would need
    /// the thread that waits for the answer.
    #[error("a command cannot be given from the emulator's own thread (its observer)")]
    OnItsThread,
}

impl CommandError {
    /// A stable name of what was refused.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            CommandError::Stopped => "stopped",
            CommandError::NoLife => "noLife",
            CommandError::Engine(EngineError::Replaying) => "replaying",
            CommandError::Engine(EngineError::Ended) => "ended",
            CommandError::Engine(EngineError::NothingToStep) => "nothingToStep",
            CommandError::Engine(EngineError::Input(_)) => "input",
            CommandError::Engine(EngineError::Button(_)) => "button",
            CommandError::RecordPath(_) => "recordPath",
            CommandError::ReplayNotRecorded => "replayNotRecorded",
            CommandError::NotRecordable => "notRecordable",
            CommandError::PowerOn(_) => "powerOn",
            CommandError::OnItsThread => "onItsThread",
        }
    }
}

/// Where a run is, for its controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RunState {
    Running,
    Paused,
    /// The machine waits for an input or a reset.
    Halted,
    /// The last life ended; a reset powers the device on again.
    Ended,
}

/// The status of a run, at most ten times a second while it runs.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// The life, from 1 at the first power-on.
    pub life: u32,
    pub run: RunState,
    pub halt: Option<Halt>,
    /// How the last life ended, once it did.
    pub end: Option<RunEnd>,
    pub speed: Speed,
    /// Device time over host time, measured over about a second; `None` until measured.
    pub speed_ratio: Option<f64>,
    /// The file the life is recorded into.
    pub recording: Option<String>,
    /// The session the life replays.
    pub replay: Option<String>,
    pub machine: Option<MachineStatus>,
    /// Nothing reaches the USB-Serial-JTAG console while the USB host is not open.
    pub usb_console_silenced: bool,
}

/// How a replay compared with its session.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "detail")]
pub enum VerdictView {
    Identical,
    Diverged(String),
    /// The host cut the replay: nothing was compared.
    Cut(String),
    /// The machine kept no trace hash.
    Unknown(String),
}

/// What the thread tells its observer, in order.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "event",
    content = "data"
)]
pub enum Update {
    /// Text a console gave: `uart0`, `uart1` or `usb-serial-jtag`.
    Console {
        channel: &'static str,
        text: String,
    },
    Notice(Notice),
    /// The panel completed a refresh; the image is at [`Emulator::frame`].
    Frame {
        generation: u64,
        refresh: RefreshKind,
        life: u32,
    },
    Status(Status),
    LifeEnded {
        life: u32,
        end: RunEnd,
        verdict: Option<VerdictView>,
    },
    /// The thread ended: no more update follows.
    Stopped,
}

/// The receiver of the updates of an emulator, called on its thread, in order.
///
/// It must return soon and must not wait for the emulator: the thread does nothing else while it
/// runs. A command given from it would wait for an answer that only this thread can give, so
/// [`Handle::command`] refuses it ([`CommandError::OnItsThread`]); an observer that needs to
/// command the emulator hands the update to another thread, which commands it.
pub type Observer = Box<dyn FnMut(Update) + Send>;

struct Request {
    command: Command,
    reply: Sender<Result<(), CommandError>>,
}

/// The answer to come of a submitted command ([`Handle::submit`]).
#[derive(Debug)]
#[must_use = "a submitted command is done whether or not its answer is read"]
pub struct Pending {
    answer: Receiver<Result<(), CommandError>>,
}

impl Pending {
    /// Waits for the thread to do the command, and gives its result.
    ///
    /// # Errors
    /// [`CommandError`]: the command was refused, or the thread stopped before doing it.
    pub fn wait(self) -> Result<(), CommandError> {
        self.answer.recv().map_err(|_| CommandError::Stopped)?
    }
}

/// The image of the last refresh, shared between the thread and its handle.
type FrameSlot = Arc<Mutex<Option<Arc<PanelFrame>>>>;

/// What any thread may hold of a running emulator to command it and read its panel: cheap to
/// clone, so that a caller never holds a lock while it waits for an answer.
#[derive(Clone)]
pub struct Handle {
    requests: Sender<Request>,
    frame: FrameSlot,
    rotation: crate::ports::Rotation,
    /// The emulator's thread, on which a command would wait for itself.
    thread: std::thread::ThreadId,
}

impl std::fmt::Debug for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handle").finish_non_exhaustive()
    }
}

impl Handle {
    /// Does `command` between two passes of the machine, and gives its result.
    ///
    /// # Errors
    /// [`CommandError`]: the command was refused, or the thread has stopped;
    /// [`CommandError::OnItsThread`] from the emulator's own thread (see [`Observer`]).
    pub fn command(&self, command: Command) -> Result<(), CommandError> {
        self.submit(command)?.wait()
    }

    /// Gives `command` to the thread without waiting for it: the thread does the commands in the
    /// order they were submitted, whichever thread then waits for which answer. An interface that
    /// must not block where it receives its commands submits there and waits elsewhere.
    ///
    /// # Errors
    /// [`CommandError::Stopped`] when the thread has stopped; [`CommandError::OnItsThread`] from
    /// the emulator's own thread (see [`Observer`]).
    pub fn submit(&self, command: Command) -> Result<Pending, CommandError> {
        if std::thread::current().id() == self.thread {
            return Err(CommandError::OnItsThread);
        }
        let (reply, answer) = mpsc::channel();
        self.requests
            .send(Request { command, reply })
            .map_err(|_| CommandError::Stopped)?;
        Ok(Pending { answer })
    }

    /// The image of the last refresh of the panel.
    #[must_use]
    pub fn frame(&self) -> Option<Arc<PanelFrame>> {
        self.frame
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// A PNG of the last image as the user of the device sees it.
    #[must_use]
    pub fn capture(&self) -> Option<Vec<u8>> {
        self.frame()
            .map(|frame| crate::capture::png(&frame, self.rotation))
    }
}

/// A machine running on its own thread. Dropping it ends its last life and waits for the thread.
pub struct Emulator {
    handle: Handle,
    look: Look,
    console: ConsoleChannel,
    device_name: String,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Emulator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Emulator")
            .field("look", &self.look)
            .field("device_name", &self.device_name)
            .finish_non_exhaustive()
    }
}

impl Emulator {
    /// Starts `launch` on a new thread, running, at `speed`; `observer` receives the updates.
    ///
    /// # Errors
    /// The error of the system when the thread cannot be started.
    pub fn start(launch: Launch, speed: Speed, observer: Observer) -> std::io::Result<Emulator> {
        let (requests, receiver) = mpsc::channel();
        let frame: FrameSlot = Arc::default();
        let Launch {
            recipe,
            plan,
            look,
            console,
            device_name,
            ..
        } = launch;
        let slot = Arc::clone(&frame);
        // The worker holds machines, which stay on the thread: it is built there (2c-R4).
        let thread = std::thread::Builder::new()
            .name("inkemu-device".to_owned())
            .spawn(move || {
                Worker {
                    recipe,
                    plan,
                    engine: None,
                    life: 0,
                    running: true,
                    speed,
                    pace: None,
                    caught_up: false,
                    observer,
                    frame: slot,
                    last_status: None,
                    last_end: None,
                    recording: None,
                    measure: None,
                    speed_ratio: None,
                    console: ConsoleBatch::default(),
                }
                .serve(&receiver);
            })?;
        Ok(Emulator {
            handle: Handle {
                requests,
                frame,
                rotation: look.panel.rotation,
                thread: thread.thread().id(),
            },
            look,
            console,
            device_name,
            thread: Some(thread),
        })
    }

    /// A handle to command the emulator from any thread.
    #[must_use]
    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }

    /// [`Handle::command`].
    ///
    /// # Errors
    /// [`CommandError`]: the command was refused, or the thread has stopped.
    pub fn command(&self, command: Command) -> Result<(), CommandError> {
        self.handle.command(command)
    }

    /// [`Handle::frame`].
    #[must_use]
    pub fn frame(&self) -> Option<Arc<PanelFrame>> {
        self.handle.frame()
    }

    /// [`Handle::capture`].
    #[must_use]
    pub fn capture(&self) -> Option<Vec<u8>> {
        self.handle.capture()
    }

    /// The name of the device, escaped.
    #[must_use]
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    #[must_use]
    pub const fn look(&self) -> &Look {
        &self.look
    }

    /// The console to show first.
    #[must_use]
    pub const fn console(&self) -> ConsoleChannel {
        self.console
    }

    /// Ends the last life and waits for the thread.
    pub fn shutdown(mut self) {
        self.stop_and_join();
    }

    fn stop_and_join(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        let _ = self.command(Command::Stop);
        let _ = thread.join();
    }
}

impl Drop for Emulator {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

/// The thread's side.
struct Worker {
    recipe: Box<dyn Recipe>,
    plan: Plan,
    engine: Option<Engine>,
    life: u32,
    /// The user wants the machine to run (not paused).
    running: bool,
    speed: Speed,
    /// The host instant and the device time the pace counts from.
    pace: Option<(Instant, u64)>,
    caught_up: bool,
    observer: Observer,
    frame: FrameSlot,
    last_status: Option<Instant>,
    last_end: Option<RunEnd>,
    /// The file the current life is recorded into.
    recording: Option<PathBuf>,
    /// The host instant and the device time the speed is measured from.
    measure: Option<(Instant, u64)>,
    speed_ratio: Option<f64>,
    console: ConsoleBatch,
}

impl Worker {
    fn serve(mut self, requests: &Receiver<Request>) {
        let first = match &mut self.plan {
            Plan::Run { record, .. } => record.take(),
            Plan::Replay { .. } => None,
        };
        // A panic on the thread (a fault of the emulator, of a port, of the observer) must not
        // lose the flash nor leave the interface waiting: every pass and every command runs
        // under a guard, and a panic ends the run by its rescue.
        let begun = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = self.begin_life(first) {
                self.notice(Level::Error, format!("error: {e}"));
            }
            self.send_status(true);
        }));
        if let Err(panic) = begun {
            self.rescue(&*panic);
            return;
        }
        loop {
            let live = self.wants_to_run();
            let request = if live && !self.caught_up {
                match requests.try_recv() {
                    Ok(request) => Some(request),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => break,
                }
            } else if live {
                match requests.recv_timeout(PACE_WAIT) {
                    Ok(request) => Some(request),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            } else {
                // Nothing runs until a command comes: the text waiting is sent now.
                self.flush_console(true);
                match requests.recv() {
                    Ok(request) => Some(request),
                    Err(_) => break,
                }
            };
            if let Some(Request { command, reply }) = request {
                let stop = command == Command::Stop;
                let done = catch_unwind(AssertUnwindSafe(|| {
                    let result = self.handle(command);
                    self.send_status(true);
                    result
                }));
                match done {
                    Ok(result) => {
                        let _ = reply.send(result);
                    }
                    Err(panic) => {
                        self.rescue(&*panic);
                        let _ = reply.send(Err(CommandError::Stopped));
                        return;
                    }
                }
                if stop {
                    self.tell(Update::Stopped);
                    return;
                }
                continue;
            }
            if self.wants_to_run()
                && let Err(panic) = catch_unwind(AssertUnwindSafe(|| self.run_pass()))
            {
                self.rescue(&*panic);
                return;
            }
        }
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| self.end_life())) {
            self.rescue(&*panic);
            return;
        }
        self.tell(Update::Stopped);
    }

    /// Ends a run that panicked: writes the flash the device left, says why the emulator
    /// stopped, and sends [`Update::Stopped`]. The console text of the passes that completed is
    /// sent; what else the life held (its session, its end) is not trusted after a panic and is
    /// dropped.
    fn rescue(&mut self, panic: &(dyn std::any::Any + Send)) {
        let mut log = Vec::new();
        // The flash first: telling the observer may panic again.
        if let Some(engine) = self.engine.take() {
            let content = catch_unwind(AssertUnwindSafe(|| engine.machine().flash_contents()));
            match (content, &mut self.plan) {
                (Ok(content), Plan::Run { flash, .. }) => {
                    let _ = catch_unwind(AssertUnwindSafe(|| flash.rescue(content, &mut log)));
                }
                (Err(_), Plan::Run { .. }) => {
                    let _ = writeln!(
                        log,
                        "error: the flash of the device could not be read after the internal error: what the last life wrote to it is lost"
                    );
                }
                (_, Plan::Replay { .. }) => {}
            }
            // A machine whose drop panics must not keep the interface from hearing of it.
            let _ = catch_unwind(AssertUnwindSafe(move || drop(engine)));
        }
        let text = panic
            .downcast_ref::<&str>()
            .map(|text| (*text).to_owned())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "no message".to_owned());
        let _ = catch_unwind(AssertUnwindSafe(|| {
            self.notice(
                Level::Error,
                format!(
                    "error: the emulator stopped on an internal error: {}",
                    sanitize(&text)
                ),
            );
            for notice in Notice::from_log(&log) {
                self.tell(Update::Notice(notice));
            }
            self.tell(Update::Stopped);
        }));
    }

    /// Sends `update` after the console text waiting, so that the observer hears everything in
    /// the order it happened.
    fn tell(&mut self, update: Update) {
        self.flush_console(true);
        (self.observer)(update);
    }

    /// Sends the console text waiting: now when `now`, else only when the last console update is
    /// old enough ([`CONSOLE_EVERY`]).
    fn flush_console(&mut self, now: bool) {
        let at = Instant::now();
        if !now && !self.console.due(at) {
            return;
        }
        for (channel, text) in self.console.take(at) {
            (self.observer)(Update::Console {
                channel: channel.name(),
                text,
            });
        }
    }

    fn wants_to_run(&self) -> bool {
        self.running
            && self
                .engine
                .as_ref()
                .is_some_and(|engine| engine.state() == State::Live)
    }

    fn notice(&mut self, level: Level, text: String) {
        self.tell(Update::Notice(Notice { level, text }));
    }

    /// Sends what a call of the engine produced; its console text joins the text waiting, which
    /// goes at most every [`CONSOLE_EVERY`] (2c-R15).
    fn emit(&mut self, output: Output) {
        for (channel, text) in output.console {
            self.console.push(channel, text);
        }
        for notice in output.notices {
            self.tell(Update::Notice(notice));
        }
        if let Some(frame) = output.frame {
            let (generation, refresh) = (frame.generation, frame.refresh);
            *self.frame.lock().unwrap_or_else(PoisonError::into_inner) = Some(frame);
            self.tell(Update::Frame {
                generation,
                refresh,
                life: self.life,
            });
        }
        self.flush_console(false);
    }

    /// Starts the pace and the measure of speed from now.
    fn rebase(&mut self) {
        let device = self
            .engine
            .as_ref()
            .map_or(0, |engine| engine.machine().device_time_nanos());
        self.pace = Some((Instant::now(), device));
        self.measure = Some((Instant::now(), device));
        self.caught_up = false;
    }

    /// The device time the next pass may reach.
    fn pace(&mut self) -> Option<u64> {
        if self.speed == Speed::Fastest {
            return None;
        }
        let device = self
            .engine
            .as_ref()
            .map_or(0, |engine| engine.machine().device_time_nanos());
        let (since, base) = *self.pace.get_or_insert((Instant::now(), device));
        let elapsed = u64::try_from(since.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let target = base.saturating_add(elapsed);
        if target.saturating_sub(device) > MAX_LAG_NANOS {
            // The host fell behind: forget the lag rather than run fast to catch it up.
            self.pace = Some((Instant::now(), device));
            return Some(device.saturating_add(MAX_LAG_NANOS));
        }
        Some(target)
    }

    fn run_pass(&mut self) {
        let pace = self.pace();
        let Some(engine) = self.engine.as_mut() else {
            return;
        };
        let output = engine.pass(pace);
        self.caught_up = output.caught_up;
        let state = engine.state();
        self.emit(output);
        match state {
            State::Ended(_) => {
                self.end_life();
                self.send_status(true);
            }
            State::Halted(_) => self.send_status(true),
            State::Live => self.send_status(false),
        }
    }

    fn handle(&mut self, command: Command) -> Result<(), CommandError> {
        match command {
            Command::Run => {
                self.running = true;
                self.rebase();
                self.engine.as_ref().map(|_| ()).ok_or(CommandError::NoLife)
            }
            Command::Pause => {
                self.running = false;
                Ok(())
            }
            Command::Step => {
                let engine = self.engine.as_mut().ok_or(CommandError::NoLife)?;
                let output = engine.step()?;
                // A step pauses the run; a refused one changes nothing.
                self.running = false;
                let ended = matches!(engine.state(), State::Ended(_));
                self.emit(output);
                if ended {
                    self.end_life();
                }
                Ok(())
            }
            Command::Reset => {
                self.end_life();
                self.begin_life(None)
            }
            Command::Stop => {
                self.end_life();
                Ok(())
            }
            Command::Speed(speed) => {
                self.speed = speed;
                self.rebase();
                Ok(())
            }
            Command::Console { channel, bytes } => {
                let output = self.live_engine()?.send_console(channel, &bytes)?;
                self.after_input(output);
                Ok(())
            }
            Command::UsbHost(host) => {
                let output = self.live_engine()?.set_usb_host(host)?;
                self.after_input(output);
                Ok(())
            }
            Command::Button { id, pressed } => {
                let output = self.live_engine()?.button(&id, pressed)?;
                self.after_input(output);
                Ok(())
            }
            Command::Record { path } => self.record(path),
        }
    }

    fn live_engine(&mut self) -> Result<&mut Engine, CommandError> {
        self.engine.as_mut().ok_or(CommandError::NoLife)
    }

    fn after_input(&mut self, output: Output) {
        self.emit(output);
        // A halted machine went on: its pace starts from where it waited.
        self.rebase();
    }

    fn record(&mut self, path: PathBuf) -> Result<(), CommandError> {
        let Plan::Run {
            paths, recording, ..
        } = &self.plan
        else {
            return Err(CommandError::ReplayNotRecorded);
        };
        if recording.is_none() {
            return Err(CommandError::NotRecordable);
        }
        let rom = match self.recipe_rom_entry() {
            Some(entry) => (paths.rom_dir.as_deref(), entry),
            None => return Err(CommandError::NotRecordable),
        };
        launch::refuse_record_path(Some(&path), &paths.request(), None, rom, &paths.device)?;
        self.end_life();
        self.begin_life(Some(path))
    }

    /// The ROM entry of the device of the run, to name the ROM file a session must not replace.
    fn recipe_rom_entry(&self) -> Option<&'static rom_store::RomEntry> {
        let Plan::Run { paths, .. } = &self.plan else {
            return None;
        };
        let profile = launch::profile(&paths.device).ok()?;
        Some(rom_store::rom_entry_for(profile.chip_revision))
    }

    /// Powers the device on: a new life, recorded into `record` when given.
    fn begin_life(&mut self, record: Option<PathBuf>) -> Result<(), CommandError> {
        let (content, mode_of, limits) = match &mut self.plan {
            Plan::Run {
                flash,
                recording,
                limits,
                ..
            } => {
                let content = flash
                    .take_next()
                    .map_err(|e| CommandError::PowerOn(e.to_string()))?;
                let recording = record.as_ref().and(recording.clone());
                (content, Some(recording), *limits)
            }
            Plan::Replay { session, .. } => {
                let (max_cycles, max_device_nanos) = session.limits.run_limits();
                (
                    None,
                    None,
                    Limits {
                        max_cycles,
                        max_device_nanos,
                    },
                )
            }
        };
        let recorded = matches!(mode_of, Some(Some(_)));
        // A power-on that fails must not lose the content: the next one starts from it.
        let kept = content.clone();
        let life = match self.recipe.power_on(content, recorded) {
            Ok(life) => life,
            Err(e) => {
                if let Plan::Run { flash, .. } = &mut self.plan {
                    flash.abandon(kept);
                }
                return Err(CommandError::PowerOn(e.to_string()));
            }
        };
        drop(kept);
        let mode = match (mode_of, &self.plan) {
            (Some(Some(recording)), _) => {
                let session_limits = session::Limits {
                    strict_mmio: recording.strict_mmio,
                    max_cycles: limits.max_cycles,
                    max_time_nanos: limits.max_device_nanos,
                    cut: None,
                };
                Mode::Run {
                    recorder: Some(Recorder::begin(
                        recording.rom,
                        recording.profile,
                        &life.machine,
                        session_limits,
                    )),
                }
            }
            (None, Plan::Replay { session, .. }) => Mode::Replay {
                session: session.clone(),
                digests: session::ConsoleDigests::default(),
            },
            (Some(None) | None, _) => Mode::Run { recorder: None },
        };
        self.recording = if recorded { record } else { None };
        self.life += 1;
        self.last_end = None;
        let mut engine = Engine::new(life, mode, limits);
        let output = engine.begin();
        self.engine = Some(engine);
        self.emit(output);
        self.rebase();
        Ok(())
    }

    /// Ends the life, if there is one: its last console text and summary, its session or
    /// verdict, its flash.
    fn end_life(&mut self) {
        let Some(engine) = self.engine.take() else {
            return;
        };
        let mut ended = engine.end();
        let output = std::mem::take(&mut ended.output);
        self.emit(output);
        let mut log = Vec::new();
        if let (Some(result), Some(path)) = (ended.session.take(), self.recording.take()) {
            write_session(result, &path, &mut log);
        }
        let verdict = ended
            .verdict
            .take()
            .map(|result| verdict_view(result, &mut log));
        if let Plan::Run { flash, .. } = &mut self.plan {
            flash.finish(&ended, &mut log);
        }
        for notice in Notice::from_log(&log) {
            self.tell(Update::Notice(notice));
        }
        self.last_end = Some(ended.end);
        self.tell(Update::LifeEnded {
            life: self.life,
            end: ended.end,
            verdict,
        });
    }

    fn status(&mut self) -> Status {
        let replay = match &self.plan {
            Plan::Replay { name, .. } => Some(name.clone()),
            Plan::Run { .. } => None,
        };
        let recording = self.recording.as_deref().map(launch::file_name);
        let (run, halt, machine) = match self.engine.as_mut() {
            Some(engine) => {
                let (run, halt) = match engine.state() {
                    State::Live if self.running => (RunState::Running, None),
                    State::Live => (RunState::Paused, None),
                    State::Halted(halt) => (RunState::Halted, Some(halt)),
                    State::Ended(_) => (RunState::Ended, None),
                };
                (run, halt, Some(engine.status()))
            }
            None => (RunState::Ended, None, None),
        };
        let usb_console_silenced = machine
            .as_ref()
            .is_some_and(|machine| machine.usb_host != UsbHost::Open.name());
        Status {
            life: self.life,
            run,
            halt,
            end: self.last_end,
            speed: self.speed,
            speed_ratio: self.speed_ratio,
            recording,
            replay,
            machine,
            usb_console_silenced,
        }
    }

    /// Sends the status: at once when `now`, else when the last one is old enough.
    fn send_status(&mut self, now: bool) {
        let due = self
            .last_status
            .is_none_or(|last| last.elapsed() >= STATUS_EVERY);
        if !now && !due {
            return;
        }
        self.measure_speed();
        let status = self.status();
        self.last_status = Some(Instant::now());
        self.tell(Update::Status(status));
    }

    fn measure_speed(&mut self) {
        // A machine that does not run (paused, stepped, waiting for an input, ended) has no
        // speed: a window across a pause would measure the pause. The measure starts again when
        // it runs.
        if !self.wants_to_run() {
            self.measure = None;
            self.speed_ratio = None;
            return;
        }
        let Some(engine) = self.engine.as_ref() else {
            return;
        };
        let device = engine.machine().device_time_nanos();
        let Some((since, base)) = self.measure else {
            self.measure = Some((Instant::now(), device));
            return;
        };
        let elapsed = since.elapsed();
        if elapsed >= SPEED_WINDOW {
            // Nanosecond counts below 2^52 are exact as `f64`: about 52 days of device time
            // within one window.
            #[allow(clippy::cast_precision_loss)]
            let ratio = device.saturating_sub(base) as f64 / elapsed.as_nanos() as f64;
            self.speed_ratio = Some(ratio);
            self.measure = Some((Instant::now(), device));
        }
    }
}

/// Writes the session of a recorded life to `path`, and says so in `log` as the command line
/// does.
fn write_session(
    result: Result<Session, record::RecordError>,
    path: &Path,
    log: &mut dyn std::io::Write,
) {
    let shown = sanitize(&path.display().to_string());
    let bytes = result.map_err(|e| e.to_string()).and_then(|session| {
        let inputs = session.inputs.len();
        session
            .to_bytes()
            .map(|bytes| (bytes, inputs))
            .map_err(|e| e.to_string())
    });
    match bytes {
        Ok((bytes, inputs)) => match session_file::write_session(path, &bytes) {
            Ok(()) => {
                let _ = writeln!(
                    log,
                    "inkemu: session recorded in {} ({inputs} inputs); `inkemu replay` replays it",
                    launch::file_name(path)
                );
            }
            Err(e) => {
                let _ = writeln!(log, "error: {e}");
            }
        },
        Err(cause) => {
            let _ = writeln!(log, "error: the run is not recorded in {shown}: {cause}");
        }
    }
}

/// What a replay's verdict says, in `log` as the command line says it.
fn verdict_view(
    result: Result<Verdict, session::NoTraceHash>,
    log: &mut dyn std::io::Write,
) -> VerdictView {
    match result {
        Ok(Verdict::Identical) => {
            let _ = writeln!(
                log,
                "inkemu: the replay gives what the session recorded: the same end, device time, cycles, instructions, trace hash, final flash and consoles"
            );
            VerdictView::Identical
        }
        Ok(Verdict::Diverged(divergence)) => {
            let text = sanitize(&divergence.to_string());
            let _ = writeln!(log, "inkemu: the replay diverges from the session: {text}");
            VerdictView::Diverged(text)
        }
        Ok(Verdict::Cut(reason)) => {
            let _ = writeln!(
                log,
                "error: the replay was cut by {} before the end of its session: nothing was compared; let it run to its end",
                reason.name()
            );
            VerdictView::Cut(reason.name().to_owned())
        }
        Err(e) => {
            let _ = writeln!(log, "error: {e}");
            VerdictView::Unknown(e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::fixtures::{echo, idle, life, options};
    use crate::pattern::{self, fixtures::BUTTONS, fixtures::PANEL};
    use soc_esp32c3::test_support::{HANG, flash_program, flat_rom, stop_watchdogs};
    use soc_esp32c3::{Esp32C3, FlashConfig, JedecId, SocConfig};
    use std::sync::mpsc::Receiver;

    const WAIT: Duration = Duration::from_secs(60);

    /// A recipe from a closure.
    struct FnRecipe<F>(F);

    impl<F> Recipe for FnRecipe<F>
    where
        F: FnMut(Option<Vec<u8>>, bool) -> Life + Send,
    {
        fn power_on(
            &mut self,
            flash: Option<Vec<u8>>,
            recorded: bool,
        ) -> Result<Life, PowerOnError> {
            Ok((self.0)(flash, recorded))
        }
    }

    const LOOK: Look = Look {
        panel: PANEL,
        buttons: &BUTTONS,
    };

    /// The X4 on `rom`, with the pattern.
    fn x4(rom: RomImage) -> Box<dyn Recipe> {
        Box::new(FnRecipe(move |_flash: Option<Vec<u8>>, recorded: bool| {
            life(rom.clone(), recorded)
        }))
    }

    fn x4_recording() -> Recording {
        Recording {
            rom: RomIdentity::of(3, &echo()),
            profile: ProfileText {
                id: "x4".to_owned(),
                text: boards::builtin_toml("x4").unwrap().to_owned(),
            },
            strict_mmio: false,
        }
    }

    fn started(launch: Launch, speed: Speed) -> (Emulator, Receiver<Update>) {
        let (sender, updates) = mpsc::channel();
        let emulator = Emulator::start(
            launch,
            speed,
            Box::new(move |update| {
                let _ = sender.send(update);
            }),
        )
        .unwrap();
        (emulator, updates)
    }

    /// The updates up to the first one `wanted` accepts, that one included.
    fn until(updates: &Receiver<Update>, mut wanted: impl FnMut(&Update) -> bool) -> Vec<Update> {
        let mut seen = Vec::new();
        loop {
            let update = updates
                .recv_timeout(WAIT)
                .unwrap_or_else(|_| panic!("no awaited update after {WAIT:?}: {seen:#?}"));
            let done = wanted(&update);
            seen.push(update);
            if done {
                return seen;
            }
        }
    }

    /// Waits until `channel` has shown `text` since the call.
    fn shows(updates: &Receiver<Update>, channel: ConsoleChannel, text: &str) {
        let mut shown = String::new();
        until(updates, |update| {
            if let Update::Console {
                channel: one,
                text: more,
            } = update
                && *one == channel.name()
            {
                shown.push_str(more);
            }
            shown.contains(text)
        });
    }

    fn status(updates: &Receiver<Update>, wanted: impl Fn(&Status) -> bool) -> Status {
        let seen = until(
            updates,
            |update| matches!(update, Update::Status(status) if wanted(status)),
        );
        match seen.last() {
            Some(Update::Status(status)) => status.clone(),
            _ => unreachable!("until ends on the update it accepts"),
        }
    }

    const UART0: ConsoleChannel = ConsoleChannel::Uart0;

    /// The life of a run on its thread: its console both ways, a pause, a step, a reset that is
    /// a new life, and a stop that ends the thread.
    #[test]
    fn an_emulator_runs_on_its_thread_and_obeys_its_commands() {
        let launch = Launch::new(
            x4(echo()),
            FlashKeeper::without_file(None, 16 << 20),
            Limits::default(),
            LOOK,
            None,
        );
        let (emulator, updates) = started(launch, Speed::Fastest);
        let first = until(&updates, |u| matches!(u, Update::Frame { .. }));
        assert!(
            first.iter().any(|u| matches!(
                u,
                Update::Frame {
                    generation: 1,
                    life: 1,
                    ..
                }
            )),
            "{first:?}"
        );
        assert_eq!(emulator.frame().map(|f| f.generation), Some(1));
        assert!(
            emulator
                .capture()
                .is_some_and(|png| png.starts_with(b"\x89PNG"))
        );
        shows(&updates, UART0, "ready");
        emulator
            .command(Command::Console {
                channel: UART0,
                bytes: b"hi\r".to_vec(),
            })
            .unwrap();
        shows(&updates, UART0, "hi\r");

        emulator.command(Command::Pause).unwrap();
        let paused = status(&updates, |s| s.run == RunState::Paused);
        let before = paused.machine.unwrap().instructions;
        emulator.command(Command::Step).unwrap();
        let stepped = status(&updates, |s| s.run == RunState::Paused);
        assert_eq!(stepped.machine.unwrap().instructions, before + 1);

        emulator.command(Command::Reset).unwrap();
        let reset = until(&updates, |u| matches!(u, Update::Status(s) if s.life == 2));
        assert!(
            reset.iter().any(|u| matches!(
                u,
                Update::LifeEnded {
                    life: 1,
                    end: RunEnd::Interrupted,
                    verdict: None
                }
            )),
            "{reset:?}"
        );
        assert!(
            reset.iter().any(
                |u| matches!(u, Update::Notice(n) if n.text.starts_with("inkemu: stopped at pc "))
            ),
            "the summary of the life is said"
        );
        emulator.command(Command::Run).unwrap();
        shows(&updates, UART0, "ready");

        emulator.command(Command::Stop).unwrap();
        let end = until(&updates, |u| *u == Update::Stopped);
        assert!(
            end.iter()
                .any(|u| matches!(u, Update::LifeEnded { life: 2, .. }))
        );
        assert!(matches!(
            emulator.command(Command::Run),
            Err(CommandError::Stopped)
        ));
    }

    /// A machine that nothing can wake halts and says so; an input sets it going again.
    #[test]
    fn a_halted_device_waits_for_an_input() {
        let launch = Launch::new(
            x4(idle()),
            FlashKeeper::without_file(None, 16 << 20),
            Limits::default(),
            LOOK,
            None,
        );
        let (emulator, updates) = started(launch, Speed::Fastest);
        let halted = status(&updates, |s| s.run == RunState::Halted);
        assert_eq!(halted.halt, Some(Halt::Idle));
        assert!(matches!(
            emulator.command(Command::Step),
            Err(CommandError::Engine(EngineError::NothingToStep))
        ));
        emulator.command(Command::UsbHost(UsbHost::Absent)).unwrap();
        // The input sets the machine going; nothing wakes the CPU, and it halts again.
        status(&updates, |s| s.run == RunState::Running);
        let again = status(&updates, |s| s.run == RunState::Halted);
        assert_eq!((again.life, again.halt), (1, Some(Halt::Idle)));
        let machine = again.machine.unwrap();
        assert!(
            machine.usb_host == "absent" || machine.usb_host_changes_pending == 1,
            "the change is made, or waits for the CPU to wake: {machine:?}"
        );
        emulator.shutdown();
    }

    /// A limit ends a life on its own; the run then waits for a reset, and refuses inputs.
    #[test]
    fn a_life_that_reaches_its_limit_ends_and_a_reset_starts_another() {
        let launch = Launch::new(
            x4(echo()),
            FlashKeeper::without_file(None, 16 << 20),
            Limits {
                max_cycles: Some(5_000),
                max_device_nanos: None,
            },
            LOOK,
            None,
        );
        let (emulator, updates) = started(launch, Speed::Fastest);
        until(&updates, |u| {
            matches!(
                u,
                Update::LifeEnded {
                    life: 1,
                    end: RunEnd::Budget,
                    ..
                }
            )
        });
        let ended = status(&updates, |s| s.run == RunState::Ended);
        assert_eq!(ended.end, Some(RunEnd::Budget));
        assert!(matches!(
            emulator.command(Command::Console {
                channel: UART0,
                bytes: b"x".to_vec()
            }),
            Err(CommandError::NoLife)
        ));
        emulator.command(Command::Reset).unwrap();
        until(&updates, |u| matches!(u, Update::LifeEnded { life: 2, .. }));
    }

    /// Real time paces the device on the host clock: a firmware that sleeps two seconds does
    /// not wake within a few hundred milliseconds of the host.
    #[test]
    fn real_time_paces_device_time_on_the_host_clock() {
        let launch = Launch::new(
            x4(echo()),
            FlashKeeper::without_file(None, 16 << 20),
            Limits::default(),
            LOOK,
            None,
        );
        let (emulator, updates) = started(launch, Speed::RealTime);
        std::thread::sleep(Duration::from_millis(300));
        emulator.command(Command::Pause).unwrap();
        let paused = status(&updates, |s| s.run == RunState::Paused);
        let device = paused.machine.unwrap().device_time_nanos;
        assert!(
            device < 2_000_000_000,
            "{device} ns of device time in about 0.3 s of host time"
        );
        assert_eq!(paused.speed, Speed::RealTime);
        emulator.command(Command::Speed(Speed::Fastest)).unwrap();
        let fastest = status(&updates, |s| s.speed == Speed::Fastest);
        assert_eq!(fastest.run, RunState::Paused, "a speed is no run");
    }

    /// A recorded life writes its session when it ends, and the session replays identically on
    /// a thread of its own, which takes no input.
    #[test]
    fn a_recorded_life_replays_identically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.inkrec");
        let launch = Launch::new(
            x4(echo()),
            FlashKeeper::without_file(None, 16 << 20),
            Limits {
                max_cycles: None,
                max_device_nanos: Some(20_000_000),
            },
            LOOK,
            Some(x4_recording()),
        );
        let (emulator, updates) = started(launch, Speed::Fastest);
        until(&updates, |u| matches!(u, Update::LifeEnded { life: 1, .. }));
        emulator
            .command(Command::Record { path: path.clone() })
            .unwrap();
        let recorded = until(&updates, |u| matches!(u, Update::LifeEnded { life: 2, .. }));
        assert!(
            recorded.iter().any(|u| matches!(
                u,
                Update::Notice(n) if n.text.starts_with("inkemu: session recorded in run.inkrec")
            )),
            "{recorded:?}"
        );
        emulator.shutdown();

        let session = session_file::read_session(&path).unwrap();
        let replay_of = session.clone();
        let recipe = Box::new(FnRecipe(move |_: Option<Vec<u8>>, _: bool| {
            let machine = session::Replay::start(&replay_of, echo())
                .unwrap()
                .into_machine();
            let (panel, buttons) = pattern::ports(PANEL, &BUTTONS);
            Life {
                machine,
                panel,
                buttons,
            }
        }));
        let (replay, updates) = started(
            Launch::replay(recipe, session, "run.inkrec".to_owned(), LOOK),
            Speed::Fastest,
        );
        let replayed = until(&updates, |u| matches!(u, Update::LifeEnded { .. }));
        assert!(
            replayed.iter().any(|u| matches!(
                u,
                Update::LifeEnded {
                    verdict: Some(VerdictView::Identical),
                    ..
                }
            )),
            "{replayed:?}"
        );
        assert!(matches!(
            replay.command(Command::Record { path }),
            Err(CommandError::ReplayNotRecorded)
        ));
    }

    /// A session path that is a file the run uses is refused, and the life goes on.
    #[test]
    fn a_session_path_of_the_run_is_refused() {
        let launch = Launch::new(
            x4(echo()),
            FlashKeeper::without_file(None, 16 << 20),
            Limits::default(),
            LOOK,
            None,
        );
        let (emulator, _updates) = started(launch, Speed::Fastest);
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            emulator.command(Command::Record {
                path: dir.path().join("a.inkrec")
            }),
            Err(CommandError::NotRecordable)
        ));
        let launch = Launch::new(
            x4(echo()),
            FlashKeeper::without_file(None, 16 << 20),
            Limits::default(),
            LOOK,
            Some(x4_recording()),
        );
        let (emulator, _updates) = started(launch, Speed::Fastest);
        assert!(matches!(
            emulator.command(Command::Record {
                path: dir.path().to_path_buf()
            }),
            Err(CommandError::RecordPath(_))
        ));
        emulator.command(Command::Pause).unwrap();
    }

    /// The flash of a 1 MiB device that writes `byte` at 0x3000, then waits for ever.
    fn writing_device(byte: u8, powered: Arc<Mutex<Vec<Option<Vec<u8>>>>>) -> Box<dyn Recipe> {
        Box::new(FnRecipe(move |flash: Option<Vec<u8>>, _: bool| {
            powered
                .lock()
                .unwrap()
                .push(flash.as_ref().map(|f| f[0x3000..0x3001].to_vec()));
            let config = SocConfig {
                flash: FlashConfig::new(1 << 20, JedecId::from_u24(0x9D_6014).unwrap()).unwrap(),
                ..SocConfig::default()
            };
            let mut program = stop_watchdogs();
            program.extend(flash_program(0x3000, &[byte]));
            program.push(HANG);
            let rom = flat_rom(&program);
            let machine = match flash {
                Some(content) => Esp32C3::with_flash(rom, content, &config).unwrap(),
                None => Esp32C3::new(rom, &config),
            };
            let (panel, buttons) = pattern::ports(PANEL, &BUTTONS);
            Life {
                machine,
                panel,
                buttons,
            }
        }))
    }

    /// The flash file is written at the end of each life and read again for the next one.
    #[test]
    fn the_flash_file_is_kept_from_one_life_to_the_next() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("flash.bin");
        let powered = Arc::new(Mutex::new(Vec::new()));
        let keeper = FlashKeeper::begin(Some(file.clone()), false, None, 1 << 20).unwrap();
        let launch = Launch::new(
            writing_device(0x00, Arc::clone(&powered)),
            keeper,
            Limits {
                max_cycles: Some(50_000),
                max_device_nanos: None,
            },
            LOOK,
            None,
        );
        let (emulator, updates) = started(launch, Speed::Fastest);
        let first = until(&updates, |u| matches!(u, Update::LifeEnded { life: 1, .. }));
        assert!(
            first.iter().any(|u| matches!(
                u,
                Update::Notice(n) if n.text.contains("; saved to ") && n.text.ends_with("flash.bin")
            )),
            "{first:?}"
        );
        assert_eq!(std::fs::read(&file).unwrap()[0x3000], 0x00);
        emulator.command(Command::Reset).unwrap();
        until(&updates, |u| matches!(u, Update::LifeEnded { life: 2, .. }));
        emulator.shutdown();
        let powered = powered.lock().unwrap();
        assert_eq!(powered[0], None, "the first life starts erased");
        assert_eq!(
            powered[1],
            Some(vec![0x00]),
            "the second starts from the file"
        );
    }

    /// `power_on` refuses as `inkemu run` does, in its order, and names what it refused.
    #[test]
    fn a_start_is_refused_as_the_command_line_refuses_it() {
        let dir = tempfile::tempdir().unwrap();
        let unknown = power_on(&Setup {
            device: "x9".into(),
            ..Setup::default()
        })
        .unwrap_err();
        assert_eq!(unknown.kind(), "profile");
        let missing = power_on(&Setup {
            device: "x4".into(),
            image: Some(dir.path().join("absent.bin")),
            rom_dir: Some(dir.path().to_path_buf()),
            ..Setup::default()
        })
        .unwrap_err();
        assert_eq!(missing.kind(), "flashImage", "{missing}");
        let no_rom = power_on(&Setup {
            device: "x4".into(),
            rom_dir: Some(dir.path().to_path_buf()),
            ..Setup::default()
        })
        .unwrap_err();
        assert_eq!(no_rom.kind(), "rom", "{no_rom}");
        let flash = dir.path().join("flash.bin");
        std::fs::write(&flash, [0u8; 8]).unwrap();
        let same = power_on(&Setup {
            device: "x4".into(),
            image: Some(flash.clone()),
            flash_file: Some(flash.clone()),
            rom_dir: Some(dir.path().to_path_buf()),
            ..Setup::default()
        })
        .unwrap_err();
        assert_eq!(same.kind(), "flashFile", "{same}");
        let record = power_on(&Setup {
            device: "x4".into(),
            image: Some(flash.clone()),
            record: Some(flash),
            rom_dir: Some(dir.path().to_path_buf()),
            ..Setup::default()
        })
        .unwrap_err();
        assert_eq!(record.kind(), "recordPath", "{record}");
    }

    /// A replay of a file that is no session is refused before any ROM is looked for.
    #[test]
    fn a_replay_of_what_is_no_session_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not.inkrec");
        std::fs::write(&path, b"hello").unwrap();
        let error = open_replay(&path, Some(dir.path().to_path_buf()), None).unwrap_err();
        assert_eq!(error.kind(), "session", "{error}");
    }

    /// A recipe that fails its `fail_at`th power-on, and only that one.
    struct Flaky {
        inner: Box<dyn Recipe>,
        calls: u32,
        fail_at: u32,
    }

    impl Recipe for Flaky {
        fn power_on(
            &mut self,
            flash: Option<Vec<u8>>,
            recorded: bool,
        ) -> Result<Life, PowerOnError> {
            self.calls += 1;
            if self.calls == self.fail_at {
                return Err(PowerOnError::Replay("this power-on fails".into()));
            }
            self.inner.power_on(flash, recorded)
        }
    }

    /// A power-on that fails keeps the flash for the next one: nothing the last life wrote is
    /// lost, with or without a flash file.
    #[test]
    fn a_failed_power_on_keeps_the_flash_for_the_next() {
        for with_file in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let powered = Arc::new(Mutex::new(Vec::new()));
            let keeper = if with_file {
                FlashKeeper::begin(Some(dir.path().join("flash.bin")), false, None, 1 << 20)
                    .unwrap()
            } else {
                FlashKeeper::without_file(None, 1 << 20)
            };
            let recipe = Box::new(Flaky {
                inner: writing_device(0x00, Arc::clone(&powered)),
                calls: 0,
                fail_at: 2,
            });
            let launch = Launch::new(
                recipe,
                keeper,
                Limits {
                    max_cycles: Some(50_000),
                    max_device_nanos: None,
                },
                LOOK,
                None,
            );
            let (emulator, updates) = started(launch, Speed::Fastest);
            until(&updates, |u| matches!(u, Update::LifeEnded { life: 1, .. }));
            assert!(matches!(
                emulator.command(Command::Reset),
                Err(CommandError::PowerOn(_))
            ));
            emulator.command(Command::Reset).unwrap();
            until(&updates, |u| matches!(u, Update::LifeEnded { life: 2, .. }));
            emulator.shutdown();
            let powered = powered.lock().unwrap();
            assert_eq!(
                *powered,
                [None, Some(vec![0x00])],
                "with a file: {with_file}"
            );
        }
    }

    /// A flash file that cannot be written keeps the content in memory: the next life starts
    /// from it, and writes the file once it can.
    #[cfg(unix)]
    #[test]
    fn a_failed_save_keeps_the_flash_for_the_next_life() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let place = dir.path().join("kept");
        std::fs::create_dir(&place).unwrap();
        let file = place.join("flash.bin");
        let powered = Arc::new(Mutex::new(Vec::new()));
        let keeper = FlashKeeper::begin(Some(file.clone()), false, None, 1 << 20).unwrap();
        let launch = Launch::new(
            writing_device(0x00, Arc::clone(&powered)),
            keeper,
            Limits {
                max_cycles: Some(50_000),
                max_device_nanos: None,
            },
            LOOK,
            None,
        );
        // The directory takes no new file: the save of the first life fails.
        std::fs::set_permissions(&place, std::fs::Permissions::from_mode(0o500)).unwrap();
        let (emulator, updates) = started(launch, Speed::Fastest);
        let first = until(&updates, |u| matches!(u, Update::LifeEnded { life: 1, .. }));
        std::fs::set_permissions(&place, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            first
                .iter()
                .any(|u| matches!(u, Update::Notice(n) if n.level == Level::Error)),
            "the failed save is said: {first:?}"
        );
        assert!(!file.exists());
        emulator.command(Command::Reset).unwrap();
        until(&updates, |u| matches!(u, Update::LifeEnded { life: 2, .. }));
        emulator.shutdown();
        assert_eq!(*powered.lock().unwrap(), [None, Some(vec![0x00])]);
        assert_eq!(std::fs::read(&file).unwrap()[0x3000], 0x00);
    }

    /// A power-on refused because the flash file cannot be taken keeps the content in memory:
    /// a later power-on starts from it, and writes it.
    #[cfg(unix)]
    #[test]
    fn a_flash_file_that_cannot_be_taken_keeps_the_flash() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let place = dir.path().join("kept");
        std::fs::create_dir(&place).unwrap();
        let file = place.join("flash.bin");
        let powered = Arc::new(Mutex::new(Vec::new()));
        let keeper = FlashKeeper::begin(Some(file.clone()), false, None, 1 << 20).unwrap();
        let launch = Launch::new(
            writing_device(0x00, Arc::clone(&powered)),
            keeper,
            Limits {
                max_cycles: Some(50_000),
                max_device_nanos: None,
            },
            LOOK,
            None,
        );
        std::fs::set_permissions(&place, std::fs::Permissions::from_mode(0o500)).unwrap();
        let (emulator, updates) = started(launch, Speed::Fastest);
        until(&updates, |u| matches!(u, Update::LifeEnded { life: 1, .. }));
        // The directory still takes no file: the lock of the next life cannot be made.
        let refused = emulator.command(Command::Reset);
        std::fs::set_permissions(&place, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            matches!(refused, Err(CommandError::PowerOn(_))),
            "{refused:?}"
        );
        emulator.command(Command::Reset).unwrap();
        until(&updates, |u| matches!(u, Update::LifeEnded { life: 2, .. }));
        emulator.shutdown();
        assert_eq!(*powered.lock().unwrap(), [None, Some(vec![0x00])]);
        assert_eq!(std::fs::read(&file).unwrap()[0x3000], 0x00);
    }

    /// A panel that panics once the firmware has written its flash.
    struct PanickingPanel(Box<dyn crate::ports::PanelSource>);

    impl crate::ports::PanelSource for PanickingPanel {
        fn describe(&self) -> crate::ports::PanelDescription {
            self.0.describe()
        }

        fn poll(&mut self, machine: &mut Esp32C3) -> Option<PanelFrame> {
            assert!(machine.flash_modified_sectors() == 0, "a panel port broke");
            self.0.poll(machine)
        }
    }

    /// Buttons that panic when one is pressed.
    struct PanickingButtons(Box<dyn crate::ports::ButtonPort>);

    impl crate::ports::ButtonPort for PanickingButtons {
        fn buttons(&self) -> &[crate::ports::ButtonSpec] {
            self.0.buttons()
        }

        fn reaches_firmware(&self) -> bool {
            false
        }

        fn set(
            &mut self,
            _: &mut Esp32C3,
            _: &str,
            _: bool,
        ) -> Result<(), crate::ports::ButtonError> {
            panic!("a button port broke")
        }
    }

    /// [`writing_device`] whose buttons panic, and its panel too when `panel` breaks.
    fn breaking_device(panel_breaks: bool) -> Box<dyn Recipe> {
        let mut inner = writing_device(0x00, Arc::default());
        Box::new(FnRecipe(move |flash: Option<Vec<u8>>, recorded: bool| {
            let life = inner.power_on(flash, recorded).unwrap();
            Life {
                machine: life.machine,
                panel: if panel_breaks {
                    Box::new(PanickingPanel(life.panel))
                } else {
                    life.panel
                },
                buttons: Box::new(PanickingButtons(life.buttons)),
            }
        }))
    }

    /// A port that panics during a pass stops the emulator: the flash the device wrote is in
    /// its file, the panic is said, and the thread says it stopped.
    #[test]
    fn a_panic_during_a_pass_keeps_the_flash_and_stops() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("flash.bin");
        let keeper = FlashKeeper::begin(Some(file.clone()), false, None, 1 << 20).unwrap();
        let launch = Launch::new(breaking_device(true), keeper, Limits::default(), LOOK, None);
        let (emulator, updates) = started(launch, Speed::Fastest);
        let seen = until(&updates, |u| matches!(u, Update::Stopped));
        assert!(
            seen.iter().any(|u| matches!(u, Update::Notice(n)
                if n.level == Level::Error && n.text.contains("a panel port broke"))),
            "{seen:#?}"
        );
        assert_eq!(std::fs::read(&file).unwrap()[0x3000], 0x00);
        assert!(matches!(
            emulator.command(Command::Run),
            Err(CommandError::Stopped)
        ));
        emulator.shutdown();
    }

    /// A port that panics during a command answers it as stopped, and the thread says so.
    #[test]
    fn a_panic_during_a_command_answers_and_stops() {
        let launch = Launch::new(
            breaking_device(false),
            FlashKeeper::without_file(None, 1 << 20),
            Limits::default(),
            LOOK,
            None,
        );
        let (emulator, updates) = started(launch, Speed::Fastest);
        emulator.command(Command::Pause).unwrap();
        let pressed = emulator.command(Command::Button {
            id: BUTTONS[0].id.to_owned(),
            pressed: true,
        });
        assert!(matches!(pressed, Err(CommandError::Stopped)), "{pressed:?}");
        let seen = until(&updates, |u| matches!(u, Update::Stopped));
        assert!(
            seen.iter().any(|u| matches!(u, Update::Notice(n)
                if n.text.contains("a button port broke"))),
            "{seen:#?}"
        );
        emulator.shutdown();
    }

    /// Console text is joined while it stays on one channel, sent at once the first time, then at
    /// most every [`CONSOLE_EVERY`].
    #[test]
    fn console_text_is_sent_in_batches() {
        let mut batch = ConsoleBatch::default();
        let start = Instant::now();
        assert!(!batch.due(start), "nothing waits");
        batch.push(UART0, "a".into());
        batch.push(UART0, "b".into());
        batch.push(ConsoleChannel::UsbSerialJtag, "c".into());
        batch.push(UART0, "d".into());
        assert!(batch.due(start));
        assert_eq!(
            batch.take(start),
            [
                (UART0, "ab".to_owned()),
                (ConsoleChannel::UsbSerialJtag, "c".to_owned()),
                (UART0, "d".to_owned())
            ]
        );
        batch.push(UART0, "e".into());
        assert!(!batch.due(start + CONSOLE_EVERY / 2));
        assert!(batch.due(start + CONSOLE_EVERY));
    }

    /// A firmware that prints on every pass reaches the observer in few updates, all its text in
    /// order.
    #[test]
    fn a_chatty_firmware_sends_few_console_updates() {
        // Prints `x` on UART0 for ever, from every pass.
        let mut program = stop_watchdogs();
        program.extend(soc_esp32c3::test_support::print_program(0x6000_0000, b"x"));
        program.push(cpu_rv32::asm::jal(0, -4));
        let launch = Launch::new(
            x4(flat_rom(&program)),
            FlashKeeper::without_file(None, 16 << 20),
            Limits::default(),
            LOOK,
            None,
        );
        let (emulator, updates) = started(launch, Speed::Fastest);
        let watched = Instant::now();
        let (mut sent, mut shown) = (0_usize, 0_usize);
        while watched.elapsed() < Duration::from_millis(600) {
            if let Ok(Update::Console { text, .. }) = updates.recv_timeout(WAIT) {
                assert!(text.bytes().all(|byte| byte == b'x'), "{text:?}");
                sent += 1;
                shown += text.len();
            }
        }
        emulator.shutdown();
        assert!(shown > 100, "the firmware printed {shown} bytes");
        // At most one batch per 40 ms, one per status (every 100 ms), and the first: 30 at most
        // in 600 ms, where a pass per update would have sent hundreds.
        assert!(sent <= 30, "{sent} console updates for {shown} bytes");
    }

    /// Commands submitted one after the other are done in that order, whatever waits for their
    /// answers and in whichever order.
    #[test]
    fn submitted_commands_are_done_in_their_order() {
        let launch = Launch::new(
            x4(echo()),
            FlashKeeper::without_file(None, 16 << 20),
            Limits::default(),
            LOOK,
            None,
        );
        let (emulator, updates) = started(launch, Speed::Fastest);
        shows(&updates, UART0, "ready");
        let handle = emulator.handle();
        let pending: Vec<Pending> = [&b"a"[..], b"b", b"c\r"]
            .into_iter()
            .map(|bytes| {
                handle
                    .submit(Command::Console {
                        channel: UART0,
                        bytes: bytes.to_vec(),
                    })
                    .unwrap()
            })
            .collect();
        // Waited for last to first: the order of the answers is not the order of the work.
        for answer in pending.into_iter().rev() {
            answer.wait().unwrap();
        }
        shows(&updates, UART0, "abc\r");
        emulator.shutdown();
    }

    /// The speed is measured only while the machine runs: a pause or a step forgets it, and
    /// running again measures it afresh.
    #[test]
    fn the_speed_is_forgotten_across_a_pause_and_a_step() {
        let launch = Launch::new(
            writing_device(0x00, Arc::default()),
            FlashKeeper::without_file(None, 1 << 20),
            Limits::default(),
            LOOK,
            None,
        );
        let (emulator, updates) = started(launch, Speed::Fastest);
        let measured =
            |u: &Update| matches!(u, Update::Status(s) if s.speed_ratio.is_some_and(|r| r > 0.0));
        until(&updates, measured);
        for command in [Command::Pause, Command::Step] {
            emulator.command(command.clone()).unwrap();
            let seen = until(
                &updates,
                |u| matches!(u, Update::Status(s) if s.run == RunState::Paused),
            );
            let Some(Update::Status(status)) = seen.last() else {
                unreachable!()
            };
            assert_eq!(status.speed_ratio, None, "after {command:?}");
        }
        emulator.command(Command::Run).unwrap();
        until(&updates, measured);
        emulator.shutdown();
    }

    /// A command from the observer, on the emulator's thread, is refused instead of waiting for
    /// ever.
    #[test]
    fn a_command_from_the_observer_is_refused() {
        let handle: Arc<Mutex<Option<Handle>>> = Arc::default();
        let (sender, answers) = mpsc::channel();
        let shared = Arc::clone(&handle);
        let launch = Launch::new(
            x4(echo()),
            FlashKeeper::without_file(None, 1 << 22),
            Limits::default(),
            LOOK,
            None,
        );
        let emulator = Emulator::start(
            launch,
            Speed::RealTime,
            Box::new(move |update| {
                if matches!(update, Update::Status(_))
                    && let Some(handle) = shared.lock().unwrap().as_ref()
                {
                    let _ = sender.send(handle.command(Command::Pause));
                }
            }),
        )
        .unwrap();
        *handle.lock().unwrap() = Some(emulator.handle());
        let answer = answers.recv_timeout(WAIT).unwrap();
        assert!(
            matches!(answer, Err(CommandError::OnItsThread)),
            "{answer:?}"
        );
        drop(handle.lock().unwrap().take());
        emulator.shutdown();
    }

    /// A machine powered on with the recorder's options keeps its trace hash.
    #[test]
    fn a_recorded_life_keeps_its_trace_hash() {
        let recorded = life(echo(), true);
        assert!(recorded.machine.trace_hash().is_some());
        assert!(!options(false).trace_hash);
    }

    #[test]
    fn the_updates_read_as_tagged_camel_case_json() {
        let update = Update::LifeEnded {
            life: 2,
            end: RunEnd::TimeLimit,
            verdict: Some(VerdictView::Diverged("x".into())),
        };
        let json = serde_json::to_string(&update).unwrap();
        assert_eq!(
            json,
            r#"{"event":"lifeEnded","data":{"life":2,"end":"timeLimit","verdict":{"kind":"diverged","detail":"x"}}}"#
        );
        let console = Update::Console {
            channel: ConsoleChannel::UsbSerialJtag.name(),
            text: "a".into(),
        };
        assert_eq!(
            serde_json::to_string(&console).unwrap(),
            r#"{"event":"console","data":{"channel":"usb-serial-jtag","text":"a"}}"#
        );
    }
}
