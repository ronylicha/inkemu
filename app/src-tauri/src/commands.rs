//! The commands of the interface (app/src/ipc/host.ts calls them by these names). Each one turns
//! what the interface sends into the types of `host`, calls it, and turns its answer back; a
//! refusal is an [`ErrorDto`] whose `kind` the interface translates.
use debugger::{DebugView, PointError, Points, PointsRequest, ViewRequest};
use host::catalog::{self, DeviceView, Look};
use host::device::{
    self, Command, CommandError, Emulator, Handle, LaunchError, SdSetup, Setup, Speed, Update,
};
use host::engine::{Limits, Notice};
use serde::{Deserialize, Serialize};
use soc_esp32c3::{ConsoleChannel, UsbHost};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use tauri::State;
use tauri::ipc::{Channel, Response};

/// The languages of the interface (app/src/i18n/i18n.ts), which the shell speaks in its own
/// dialogs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Fr,
    #[default]
    En,
}

/// The title and the text of the dialog that tells, before the window closes, what the last life
/// of the device could not keep; `detail` is the host's own lines, as the journal shows them.
#[must_use]
pub fn closing_dialog(language: Language, detail: &str) -> (String, String) {
    let (title, lead) = match language {
        Language::Fr => (
            "inkemu : l'appareil a été éteint sans tout conserver",
            "L'appareil a été éteint avant la fermeture, mais ceci n'a pas pu être conservé :",
        ),
        Language::En => (
            "inkemu: the device was powered off without keeping everything",
            "The device was powered off before closing, but this could not be kept:",
        ),
    };
    (title.to_owned(), format!("{lead}\n\n{detail}"))
}

/// The emulator the window drives, with the number the interface gave its start: every command
/// names the start it is meant for (2c-R18).
#[derive(Debug)]
struct Running {
    start: u64,
    emulator: Emulator,
}

/// The power-offs under way: an emulator taken from the shell still writes its flash file and
/// its session until its thread ends. The application waits for them before it closes (2c-R17).
#[derive(Clone, Debug, Default)]
struct Stopping(Arc<(Mutex<usize>, Condvar)>);

impl Stopping {
    /// One more power-off under way, until the guard is dropped.
    fn begin(&self) -> StopGuard {
        *self.0.0.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        StopGuard(self.clone())
    }

    fn under_way(&self) -> bool {
        *self.0.0.lock().unwrap_or_else(PoisonError::into_inner) > 0
    }

    /// Waits until no power-off is under way.
    fn wait(&self) {
        let (count, ended) = &*self.0;
        let mut count = count.lock().unwrap_or_else(PoisonError::into_inner);
        while *count > 0 {
            count = ended.wait(count).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// A power-off under way ([`Stopping::begin`]); dropped when it ends, however it ends.
#[derive(Debug)]
struct StopGuard(Stopping);

impl Drop for StopGuard {
    fn drop(&mut self) {
        let (count, ended) = &*self.0.0;
        let mut count = count.lock().unwrap_or_else(PoisonError::into_inner);
        *count = count.saturating_sub(1);
        ended.notify_all();
    }
}

/// The emulator the window drives, if one runs, the power-offs under way, and the language the
/// interface speaks.
#[derive(Debug, Default)]
pub struct Shell {
    running: Mutex<Option<Running>>,
    stopping: Stopping,
    language: Mutex<Language>,
    /// A closing of the application is under way ([`Shell::begin_closing`]).
    closing: Arc<AtomicBool>,
    /// A signal powered the device off and asked the application to exit ([`Shell::signalled`]).
    signalled: AtomicBool,
}

/// What the application does when it is asked to exit ([`Shell::on_exit_request`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitDecision {
    /// It exits.
    Exit,
    /// It waits: the device is on, or a closing is under way; a closing powers off first.
    Prevent,
    /// It exits with the status of a signal instead (130): a signal came, and this request (a
    /// window closed afterwards) asked for another status.
    ExitSignalled,
}

/// Refuses a command meant for the emulator of start `asked` when the one held is another (or
/// none): a command queued for an emulator that a new start replaced never reaches the new one.
fn same_start(held: Option<u64>, asked: u64) -> Result<(), ErrorDto> {
    if held == Some(asked) {
        Ok(())
    } else {
        Err(ErrorDto::from(CommandError::Stopped))
    }
}

impl Shell {
    /// Ends the last life of the running emulator, if any, and waits for it and for every other
    /// power-off under way (an emulator a new start replaced).
    ///
    /// # Errors
    /// What the last life could not keep (its flash file, its session), one error per line: the
    /// application tells its user before it closes.
    pub fn power_off(&self) -> Result<(), String> {
        let ended = match self.take() {
            Some((emulator, _under_way)) => emulator.shutdown().map_err(|e| e.to_string()),
            None => Ok(()),
        };
        self.stopping.wait();
        ended
    }

    /// Takes the running emulator, counted as a power-off under way until the guard drops.
    /// Taken and counted under the lock: the shell is never seen off while one writes.
    fn take(&self) -> Option<(Emulator, StopGuard)> {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .map(|running| (running.emulator, self.stopping.begin()))
    }

    /// The language the interface last said it speaks; English until it says.
    #[must_use]
    pub fn language(&self) -> Language {
        *self.language.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether an emulator runs or a power-off is under way: either must end before the
    /// application closes.
    #[must_use]
    pub fn is_on(&self) -> bool {
        // The lock first: a power-off is counted before the lock that takes it is released.
        let running = self
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some();
        running || self.stopping.under_way()
    }

    /// Whether the running emulator holds the flash file at `path`.
    fn holds_flash_file(&self, path: &std::path::Path) -> bool {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|running| running.emulator.holds_flash_file(path))
    }

    /// Whether the running emulator holds the SD image at `path`.
    fn holds_sd_image(&self, path: &std::path::Path) -> bool {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|running| running.emulator.holds_sd_image(path))
    }

    /// Whether the running emulator holds a file `setup` would take, locked: its flash file or
    /// its SD image. That emulator is powered off first, as the user asked for the new run
    /// (H3): a lock held by the same process would refuse the new run otherwise.
    fn holds_a_file_of(&self, setup: &Setup) -> bool {
        setup
            .flash_file
            .as_deref()
            .is_some_and(|file| self.holds_flash_file(file))
            || setup
                .sd
                .as_ref()
                .is_some_and(|sd| self.holds_sd_image(&sd.path))
    }

    /// A handle to the emulator of start `start`, refused when another runs ([`same_start`]).
    fn handle_of(&self, start: u64) -> Result<Handle, ErrorDto> {
        let running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        same_start(running.as_ref().map(|running| running.start), start)?;
        running
            .as_ref()
            .map(|running| running.emulator.handle())
            .ok_or_else(|| ErrorDto::from(CommandError::Stopped))
    }

    /// Keeps, of `emulator` (start `start`) and the one that runs, the one of the later start:
    /// two starts answered out of order leave the one the user asked for last. Says whether
    /// `emulator` is kept, and gives back the other, counted as a power-off under way, which the
    /// caller powers off without the lock.
    fn keep(&self, start: u64, emulator: Emulator) -> (bool, Option<(Emulator, StopGuard)>) {
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        if running.as_ref().is_some_and(|held| held.start > start) {
            return (false, Some((emulator, self.stopping.begin())));
        }
        let replaced = running
            .replace(Running { start, emulator })
            .map(|replaced| (replaced.emulator, self.stopping.begin()));
        (true, replaced)
    }

    /// Gives `command` to the emulator of start `start`, refused when another runs
    /// ([`Shell::handle_of`]).
    fn submit(&self, start: u64, command: Command) -> Result<device::Pending, ErrorDto> {
        Ok(self.handle_of(start)?.submit(command)?)
    }

    /// The last image of the emulator of start `start`, encoded; empty before its first, or when
    /// another runs.
    fn frame_bytes(&self, start: u64) -> Vec<u8> {
        self.handle_of(start)
            .ok()
            .and_then(|handle| handle.frame())
            .map(|frame| frame.encode())
            .unwrap_or_default()
    }

    /// Marks the application as closing: the guard for the first request, which powers off and
    /// closes; `None` for the requests that follow, which only wait for it (2c-R17). The closing
    /// ends when the guard drops, however its holder ends (a panic included).
    #[must_use]
    pub fn begin_closing(&self) -> Option<ClosingGuard> {
        (!self.closing.swap(true, Ordering::SeqCst))
            .then(|| ClosingGuard(Arc::clone(&self.closing)))
    }

    /// Whether a closing is under way.
    #[must_use]
    pub fn is_closing(&self) -> bool {
        self.closing.load(Ordering::SeqCst)
    }

    /// A signal has powered the device off and asks the application to exit: from now on it
    /// exits, with 130, whatever a closing of the window is doing.
    pub fn mark_signalled(&self) {
        self.signalled.store(true, Ordering::SeqCst);
    }

    /// What to do with a request to exit with `code`: once a signal came, exit, with its status;
    /// otherwise wait while the device is on or a closing is under way (2c-R17).
    #[must_use]
    pub fn on_exit_request(&self, code: Option<i32>) -> ExitDecision {
        if self.signalled.load(Ordering::SeqCst) {
            if code == Some(crate::signals::SIGNALLED) {
                ExitDecision::Exit
            } else {
                ExitDecision::ExitSignalled
            }
        } else if self.is_on() || self.is_closing() {
            ExitDecision::Prevent
        } else {
            ExitDecision::Exit
        }
    }
}

/// A closing under way ([`Shell::begin_closing`]): dropped when it ends, however it ends, so
/// that a closing that failed never keeps the application from closing again.
#[derive(Debug)]
pub struct ClosingGuard(Arc<AtomicBool>);

impl Drop for ClosingGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// A refusal, as the interface reads it: what was refused, and the host's words for it.
#[derive(Debug, Serialize)]
pub struct ErrorDto {
    kind: String,
    message: String,
}

impl ErrorDto {
    fn new(kind: &str, message: impl std::fmt::Display) -> ErrorDto {
        ErrorDto {
            kind: kind.to_owned(),
            message: message.to_string(),
        }
    }
}

impl From<LaunchError> for ErrorDto {
    fn from(error: LaunchError) -> ErrorDto {
        ErrorDto::new(error.kind(), &error)
    }
}

impl From<CommandError> for ErrorDto {
    fn from(error: CommandError) -> ErrorDto {
        ErrorDto::new(error.kind(), &error)
    }
}

impl From<tauri::Error> for ErrorDto {
    fn from(error: tauri::Error) -> ErrorDto {
        ErrorDto::new("ipc", error)
    }
}

impl From<PointError> for ErrorDto {
    fn from(error: PointError) -> ErrorDto {
        ErrorDto::from(CommandError::Point(error))
    }
}

/// The points of the debugger a start begins with: the ones the interface kept from the run
/// before (design `SP4a`, 4a-D8), each checked as when it is set alone.
fn points_of(request: Option<PointsRequest>) -> Result<Points, ErrorDto> {
    Ok(Points::from_request(&request.unwrap_or_default())?)
}

/// The form of a run, as app/src/ipc/types.ts writes it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupDto {
    device: String,
    image: Option<PathBuf>,
    firmware: Option<PathBuf>,
    bootloader: Option<PathBuf>,
    flash_file: Option<PathBuf>,
    reset_flash: bool,
    rom_dir: Option<PathBuf>,
    usb_host: String,
    strict_mmio: bool,
    record: Option<PathBuf>,
    max_time: String,
    max_cycles: String,
    console: Option<String>,
    /// A USB cable powers the device from the start (`--usb-power`).
    usb_power: bool,
    /// The SD card in the slot, when the device has one and the form gives a card.
    sd: Option<SdDto>,
}

/// The SD card of the form (`--sd-image`, `--sd-read-only`, or a blank image made first).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SdDto {
    path: PathBuf,
    read_only: bool,
    /// Make a blank card of this many MiB at `path` first; `null` for an existing image.
    create_mib: Option<u64>,
}

impl From<SdDto> for SdSetup {
    fn from(card: SdDto) -> SdSetup {
        SdSetup {
            path: card.path,
            read_only: card.read_only,
            create_mib: card.create_mib,
        }
    }
}

fn usb_host(name: &str) -> Result<UsbHost, ErrorDto> {
    UsbHost::ALL
        .into_iter()
        .find(|host| host.name() == name)
        .ok_or_else(|| ErrorDto::new("input", format!("no USB host state is named `{name}`")))
}

fn console_channel(name: &str) -> Result<ConsoleChannel, ErrorDto> {
    ConsoleChannel::ALL
        .into_iter()
        .find(|channel| channel.name() == name)
        .ok_or_else(|| ErrorDto::new("input", format!("no console is named `{name}`")))
}

impl SetupDto {
    /// The setup of `host::device::power_on`; the limits are read as `--max-time` and
    /// `--max-cycles` read them.
    fn into_setup(self) -> Result<Setup, ErrorDto> {
        let max_device_nanos = match self.max_time.trim() {
            "" => None,
            text => Some(
                host::run::parse_device_seconds(text).map_err(|e| ErrorDto::new("maxTime", e))?,
            ),
        };
        let max_cycles = match self.max_cycles.trim() {
            "" => None,
            text => Some(text.parse::<u64>().map_err(|e| {
                ErrorDto::new(
                    "maxCycles",
                    format!("`{text}` is not a number of cycles: {e}"),
                )
            })?),
        };
        Ok(Setup {
            device: self.device,
            image: self.image,
            firmware: self.firmware,
            bootloader: self.bootloader,
            flash_file: self.flash_file,
            reset_flash: self.reset_flash,
            rom_dir: self.rom_dir,
            usb_host: usb_host(&self.usb_host)?,
            strict_mmio: self.strict_mmio,
            record: self.record,
            limits: Limits {
                max_cycles,
                max_device_nanos,
            },
            console: self.console.as_deref().map(console_channel).transpose()?,
            usb_power: self.usb_power,
            sd: self.sd.map(SdSetup::from),
        })
    }
}

/// A command, as app/src/ipc/types.ts writes it.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CommandDto {
    Run,
    Pause,
    Step,
    Reset,
    Stop,
    Speed {
        speed: Speed,
    },
    Console {
        channel: String,
        bytes: Vec<u8>,
    },
    UsbHost {
        host: String,
    },
    Button {
        id: String,
        pressed: bool,
    },
    Record {
        path: PathBuf,
    },
    Capture {
        path: PathBuf,
    },
    Breakpoint {
        address: u32,
        set: bool,
    },
    /// `on`: `read`, `write` or `access`.
    Watchpoint {
        address: u32,
        length: u32,
        on: String,
        set: bool,
    },
}

impl CommandDto {
    fn into_command(self) -> Result<Command, ErrorDto> {
        Ok(match self {
            CommandDto::Run => Command::Run,
            CommandDto::Pause => Command::Pause,
            CommandDto::Step => Command::Step,
            CommandDto::Reset => Command::Reset,
            CommandDto::Stop => Command::Stop,
            CommandDto::Speed { speed } => Command::Speed(speed),
            CommandDto::Console { channel, bytes } => Command::Console {
                channel: console_channel(&channel)?,
                bytes,
            },
            CommandDto::UsbHost { host } => Command::UsbHost(usb_host(&host)?),
            CommandDto::Button { id, pressed } => Command::Button { id, pressed },
            CommandDto::Record { path } => Command::Record { path },
            CommandDto::Capture { path } => Command::Capture { path },
            CommandDto::Breakpoint { address, set } => Command::Breakpoint { address, set },
            CommandDto::Watchpoint {
                address,
                length,
                on,
                set,
            } => Command::Watchpoint {
                watch: debugger::WatchRequest {
                    address,
                    length,
                    on,
                }
                .watch()?,
                set,
            },
        })
    }
}

/// What starting a run or a replay gives back.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartedDto {
    notices: Vec<Notice>,
    look: Look,
    console: &'static str,
    device_name: String,
}

/// Starts `launch` on its thread, its updates sent through `channel`, in place of the emulator
/// that ran before, whose last life ends first.
async fn start(
    shell: &Shell,
    launch: device::Launch,
    speed: Speed,
    number: u64,
    channel: Channel<Update>,
) -> Result<StartedDto, ErrorDto> {
    let started = StartedDto {
        notices: launch.notices.clone(),
        look: launch.look.clone(),
        console: launch.console.name(),
        device_name: launch.device_name.clone(),
    };
    let emulator = Emulator::start(
        launch,
        speed,
        Box::new(move |update| {
            // A window that closed takes no more updates; the emulator goes on until it is
            // powered off.
            let _ = channel.send(update);
        }),
    )
    .map_err(|e| ErrorDto::new("powerOn", e))?;
    let (kept, other) = shell.keep(number, emulator);
    if let Some((other, under_way)) = other {
        // What its last life could not keep reaches the journal through its own updates, which
        // the interface keeps (app/src/state/reducer.ts). Counted until it ends: the
        // application does not close while it writes. Its thread failing to join says nothing
        // of the emulator kept: the start is answered all the same.
        let _ = tauri::async_runtime::spawn_blocking(move || {
            let ended = other.shutdown();
            drop(under_way);
            ended
        })
        .await;
    }
    if kept {
        Ok(started)
    } else {
        // A later start was answered first: this one is powered off, as replaced.
        Err(ErrorDto::from(CommandError::Stopped))
    }
}

/// The language the interface speaks, said when it starts and whenever the user changes it: the
/// shell's own dialogs (the one before the window closes) speak it too.
// Tauri hands a command its state by value: the signature is the framework's.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn set_language(shell: State<'_, Shell>, language: Language) {
    *shell
        .language
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = language;
}

/// The built-in devices.
#[tauri::command]
pub fn devices() -> Result<Vec<DeviceView>, ErrorDto> {
    catalog::devices().map_err(|e| ErrorDto::new("profile", e))
}

/// Powers a device on as `inkemu run` does. The checks and the ROM, which may be downloaded, run
/// off the interface's thread.
///
/// The device that runs goes on running until the new one has passed every check: a refused
/// start leaves it as it was. Only a start on a file it holds, its flash file or its SD image,
/// powers it off first, since its power-off writes that file and releases its lock; and every
/// start waits for the power-offs under way, whose emulators may still hold such a file (R4).
///
/// `points` are the points of the debugger the run begins with, before its first instruction.
#[tauri::command]
pub async fn power_on(
    shell: State<'_, Shell>,
    setup: SetupDto,
    speed: Speed,
    start: u64,
    points: Option<PointsRequest>,
    on_update: Channel<Update>,
) -> Result<StartedDto, ErrorDto> {
    let setup = setup.into_setup()?;
    let points = points_of(points)?;
    let shell = shell.inner();
    if shell.holds_a_file_of(&setup) {
        off_the_main_thread(shell).await?;
    }
    let stopping = shell.stopping.clone();
    let launch = tauri::async_runtime::spawn_blocking(move || {
        after_the_power_offs(&stopping, || device::power_on(&setup))
    })
    .await??;
    self::start(shell, launch.with_points(points), speed, start, on_update).await
}

/// Does `start` once no power-off is under way: an emulator a start replaced, or one powered
/// off for this start, writes its files and holds their locks until its thread ends, and a lock
/// held by this process refuses a run as one held by another would (R4). Blocks: never on the
/// interface's thread.
fn after_the_power_offs<T>(stopping: &Stopping, start: impl FnOnce() -> T) -> T {
    stopping.wait();
    start()
}

/// Opens a session and replays it as `inkemu replay` does, with the points of the debugger
/// `points`: they stop the replay without changing it.
#[tauri::command]
pub async fn open_replay(
    shell: State<'_, Shell>,
    path: PathBuf,
    rom_dir: Option<PathBuf>,
    speed: Speed,
    start: u64,
    points: Option<PointsRequest>,
    on_update: Channel<Update>,
) -> Result<StartedDto, ErrorDto> {
    let points = points_of(points)?;
    // A replay holds no file of a run: the device that runs goes on until the replay is ready.
    let launch =
        tauri::async_runtime::spawn_blocking(move || device::open_replay(&path, rom_dir, None))
            .await??;
    self::start(
        shell.inner(),
        launch.with_points(points),
        speed,
        start,
        on_update,
    )
    .await
}

/// Powers off the emulator that runs, off the interface's thread: its last life writes its files.
async fn off_the_main_thread(shell: &Shell) -> Result<(), ErrorDto> {
    if let Some((emulator, under_way)) = shell.take() {
        // What its last life could not keep reaches the journal through its own updates.
        let _ = tauri::async_runtime::spawn_blocking(move || {
            let ended = emulator.shutdown();
            drop(under_way);
            ended
        })
        .await?;
    }
    Ok(())
}

/// Gives a command to the emulator, then answers through `answer` once it is done.
///
/// The command is given here, on the thread that receives the interface's calls in the order the
/// interface made them, so that the emulator does them in that order (a press before its
/// release); the answer is awaited on another thread, never this one (2c-R16).
// Tauri hands a command its state by value: the signature is the framework's.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn command(
    shell: State<'_, Shell>,
    command: CommandDto,
    start: u64,
    answer: Channel<Option<ErrorDto>>,
) -> Result<(), ErrorDto> {
    let pending = shell.submit(start, command.into_command()?)?;
    tauri::async_runtime::spawn_blocking(move || {
        // A window that closed takes no answer.
        let _ = answer.send(pending.wait().err().map(ErrorDto::from));
    });
    Ok(())
}

/// The last image of the panel of the emulator of start `start`, encoded (`PanelFrame::encode`);
/// empty before the first, or when another emulator runs.
// Tauri hands a command its state by value: the signature is the framework's.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn frame(shell: State<'_, Shell>, start: u64) -> Response {
    Response::new(shell.frame_bytes(start))
}

/// The notices of the material inkemu embeds (`inkemu licenses`).
#[tauri::command]
pub fn notices() -> String {
    boards::notices::notices_text()
}

/// What the debugger shows of the emulator of start `start` (design `SP4a`): its CPU, its code,
/// the memory `view` asks for, its points. The emulator's thread reads it between two passes;
/// the answer is awaited off the interface's thread.
#[tauri::command]
pub async fn inspect(
    shell: State<'_, Shell>,
    start: u64,
    view: ViewRequest,
) -> Result<DebugView, ErrorDto> {
    let shell = shell.inner();
    let handle = shell.handle_of(start)?;
    tauri::async_runtime::spawn_blocking(move || handle.inspect(view))
        .await?
        .map_err(ErrorDto::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{self, Receiver};
    use std::time::{Duration, Instant};

    const WAIT: Duration = Duration::from_secs(60);

    /// A real emulator of `host`'s looping test device, whose power-off lasts `slow_end`, and
    /// its updates.
    fn emulator(slow_end: Duration) -> (Emulator, Receiver<Update>) {
        let (sender, updates) = mpsc::channel();
        let emulator = Emulator::start(
            host::test_support::looping_device(slow_end),
            Speed::Fastest,
            Box::new(move |update| {
                let _ = sender.send(update);
            }),
        )
        .unwrap();
        (emulator, updates)
    }

    fn first_frame(updates: &Receiver<Update>) {
        let asked = Instant::now();
        while asked.elapsed() < WAIT {
            if let Ok(Update::Frame { .. }) = updates.recv_timeout(WAIT) {
                return;
            }
        }
        panic!("no frame");
    }

    /// With a real emulator: commands and images go only to the start they name, and the shell
    /// stays on while the emulator's power-off (slow here) writes its files, which a power-off of
    /// the shell waits for.
    #[test]
    fn the_shell_drives_one_start_and_stays_on_while_it_powers_off() {
        let shell = Arc::new(Shell::default());
        let (running, updates) = emulator(Duration::from_millis(400));
        let (kept, replaced) = shell.keep(1, running);
        assert!(kept && replaced.is_none());
        first_frame(&updates);
        // An earlier start's command is refused; the running one's is done.
        assert_eq!(shell.submit(0, Command::Pause).unwrap_err().kind, "stopped");
        shell.submit(1, Command::Pause).unwrap().wait().unwrap();
        assert_ne!(shell.frame_bytes(1), Vec::<u8>::new());
        assert_eq!(shell.frame_bytes(0), Vec::<u8>::new());
        let off = Arc::clone(&shell);
        let asked = Instant::now();
        let powering_off = std::thread::spawn(move || off.power_off());
        std::thread::sleep(Duration::from_millis(150));
        assert!(shell.is_on(), "on while the power-off writes");
        assert_eq!(powering_off.join().unwrap(), Ok(()));
        assert!(asked.elapsed() >= Duration::from_millis(350));
        assert!(!shell.is_on());
    }

    /// Two starts answered out of order: the later start's emulator is kept, the other powered
    /// off, and its start is refused.
    #[test]
    fn the_later_start_is_kept_whatever_the_order_of_the_answers() {
        let shell = Shell::default();
        let (later, _later_updates) = emulator(Duration::ZERO);
        let (earlier, _earlier_updates) = emulator(Duration::ZERO);
        let (kept, other) = shell.keep(2, later);
        assert!(kept && other.is_none());
        let (kept, other) = shell.keep(1, earlier);
        assert!(!kept);
        let (earlier, under_way) = other.unwrap();
        assert!(shell.is_on());
        earlier.shutdown().unwrap();
        drop(under_way);
        shell.submit(2, Command::Pause).unwrap().wait().unwrap();
        assert_eq!(shell.submit(1, Command::Pause).unwrap_err().kind, "stopped");
        assert_eq!(shell.power_off(), Ok(()));
    }

    /// H3: a run that holds the SD image a new start asks for is powered off first, as one
    /// holding its flash file is: the image is locked by the run, and the new one could not
    /// open it.
    #[test]
    fn a_start_on_the_card_the_running_device_holds_powers_that_device_off_first() {
        // Removed however the test ends.
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        let (shell, card) = shell_on_a_card(dir);
        let on_card = |path: PathBuf| Setup {
            device: "x4".into(),
            sd: Some(device::SdSetup {
                path,
                read_only: false,
                create_mib: None,
            }),
            ..Setup::default()
        };
        assert!(shell.holds_a_file_of(&on_card(card.clone())));
        // The same file by another spelling of its path is the same card.
        assert!(shell.holds_a_file_of(&on_card(dir.join(".").join("card.img"))));
        assert!(!shell.holds_a_file_of(&on_card(dir.join("other.img"))));
        assert!(!shell.holds_a_file_of(&Setup::default()));
        assert_eq!(shell.power_off(), Ok(()));
        assert!(!shell.holds_a_file_of(&on_card(card)));
    }

    /// A shell whose emulator runs the looping test device with a blank image of 64 MiB in
    /// `dir` in its slot, as start 1; and the path of that image.
    fn shell_on_a_card(dir: &std::path::Path) -> (Shell, PathBuf) {
        let card = dir.join("card.img");
        std::fs::File::create(&card)
            .unwrap()
            .set_len(64 << 20)
            .unwrap();
        let shell = Shell::default();
        let (sender, _updates) = mpsc::channel();
        let running = Emulator::start(
            host::test_support::looping_device_with_sd(&card),
            Speed::Fastest,
            Box::new(move |update| {
                let _ = sender.send(update);
            }),
        )
        .unwrap();
        let (kept, _) = shell.keep(1, running);
        assert!(kept);
        (shell, card)
    }

    /// R4: an emulator powering off holds its card until its thread ends, out of the shell's
    /// sight: a start on that card waits for the power-off, where opening the card at once is
    /// refused as in use.
    #[test]
    fn a_start_waits_for_a_power_off_that_still_holds_its_card() {
        use host::sd_image::{Access, SdImage, SdImageError};
        let dir = tempfile::tempdir().unwrap();
        let (shell, card) = shell_on_a_card(dir.path());
        let (emulator, under_way) = shell.take().unwrap();
        assert!(!shell.holds_a_file_of(&Setup {
            sd: Some(device::SdSetup {
                path: card.clone(),
                read_only: false,
                create_mib: None,
            }),
            ..Setup::default()
        }));
        let open = || SdImage::open(&card, Access::ReadWrite, &[], &mut Vec::new());
        assert!(matches!(open(), Err(SdImageError::InUse { .. })));
        let slow = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            let ended = emulator.shutdown();
            drop(under_way);
            ended
        });
        if let Err(e) = after_the_power_offs(&shell.stopping, open) {
            panic!("{e}");
        }
        assert!(slow.join().unwrap().is_ok());
    }

    /// The first request to close powers off; those that follow only wait for it.
    #[test]
    fn a_second_request_to_close_waits_for_the_first() {
        let shell = Shell::default();
        assert!(!shell.is_closing());
        let first = shell.begin_closing();
        assert!(first.is_some());
        assert!(shell.begin_closing().is_none());
        assert!(shell.is_closing());
        drop(first);
        assert!(!shell.is_closing());
        assert!(shell.begin_closing().is_some());
    }

    /// A request to exit waits while the device is on or a closing is under way, until a signal
    /// came: then the signal's exit goes through even during a closing, and a later request (a
    /// window closed afterwards) exits with the signal's status.
    #[test]
    fn a_signal_exits_even_during_a_closing() {
        let shell = Shell::default();
        assert_eq!(shell.on_exit_request(None), ExitDecision::Exit);
        assert_eq!(shell.on_exit_request(Some(0)), ExitDecision::Exit);
        let closing = shell.begin_closing().unwrap();
        assert_eq!(shell.on_exit_request(None), ExitDecision::Prevent);
        assert_eq!(shell.on_exit_request(Some(130)), ExitDecision::Prevent);
        shell.mark_signalled();
        assert_eq!(shell.on_exit_request(Some(130)), ExitDecision::Exit);
        assert_eq!(shell.on_exit_request(None), ExitDecision::ExitSignalled);
        drop(closing);
        assert_eq!(shell.on_exit_request(Some(0)), ExitDecision::ExitSignalled);
        assert_eq!(shell.on_exit_request(Some(130)), ExitDecision::Exit);
    }

    /// A closing whose power-off panics still ends: the guard lets the application close again.
    #[test]
    fn a_closing_that_panics_still_ends() {
        let shell = Shell::default();
        let closing = shell.begin_closing().unwrap();
        let broke = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _closing = closing;
            panic!("a power-off broke");
        }));
        assert!(broke.is_err());
        assert!(!shell.is_closing());
        assert!(shell.begin_closing().is_some());
    }

    #[test]
    fn a_command_of_the_interface_reads_as_a_command_of_the_host() {
        let read = |json: &str| {
            serde_json::from_str::<CommandDto>(json)
                .unwrap()
                .into_command()
                .unwrap()
        };
        assert_eq!(read(r#"{"kind":"pause"}"#), Command::Pause);
        assert_eq!(
            read(r#"{"kind":"speed","speed":"fastest"}"#),
            Command::Speed(Speed::Fastest)
        );
        assert_eq!(
            read(r#"{"kind":"console","channel":"usb-serial-jtag","bytes":[104,105]}"#),
            Command::Console {
                channel: ConsoleChannel::UsbSerialJtag,
                bytes: b"hi".to_vec()
            }
        );
        assert_eq!(
            read(r#"{"kind":"usbHost","host":"attached"}"#),
            Command::UsbHost(UsbHost::Attached)
        );
        assert_eq!(
            read(r#"{"kind":"button","id":"power","pressed":true}"#),
            Command::Button {
                id: "power".into(),
                pressed: true
            }
        );
        let unknown = serde_json::from_str::<CommandDto>(r#"{"kind":"usbHost","host":"x"}"#)
            .unwrap()
            .into_command()
            .unwrap_err();
        assert_eq!(unknown.kind, "input");
    }

    /// The points of the debugger, as the interface writes them (design `SP4a`).
    #[test]
    fn a_point_of_the_interface_reads_as_a_point_of_the_host() {
        let read = |json: &str| {
            serde_json::from_str::<CommandDto>(json)
                .unwrap()
                .into_command()
        };
        assert_eq!(
            read(r#"{"kind":"breakpoint","address":1073741828,"set":true}"#).unwrap(),
            Command::Breakpoint {
                address: 0x4000_0004,
                set: true
            }
        );
        assert_eq!(
            read(
                r#"{"kind":"watchpoint","address":1070071808,"length":4,"on":"write","set":false}"#
            )
            .unwrap(),
            Command::Watchpoint {
                watch: soc_esp32c3::Watch {
                    address: 0x3FC8_0000,
                    length: 4,
                    on: soc_esp32c3::WatchOn::Write
                },
                set: false
            }
        );
        let refused = read(r#"{"kind":"watchpoint","address":0,"length":4,"on":"run","set":true}"#)
            .unwrap_err();
        assert_eq!(refused.kind, "point");
        assert!(refused.message.contains("`run`"), "{}", refused.message);
    }

    /// A start begins with the points the interface kept, each checked; none is no point.
    #[test]
    fn the_points_a_start_begins_with_are_checked() {
        assert_eq!(points_of(None).unwrap(), Points::default());
        let kept: PointsRequest =
            serde_json::from_str(r#"{"breakpoints":[1073741824],"watchpoints":[]}"#).unwrap();
        assert_eq!(points_of(Some(kept)).unwrap().breakpoints().count(), 1);
        let odd: PointsRequest = serde_json::from_str(r#"{"breakpoints":[1073741825]}"#).unwrap();
        assert_eq!(points_of(Some(odd)).unwrap_err().kind, "point");
    }

    /// The debugger reads the emulator of the start it names, stopped at the breakpoint it began
    /// with; another start's is refused.
    #[test]
    fn the_debugger_reads_the_emulator_of_its_start() {
        let shell = Shell::default();
        let mut points = Points::default();
        points
            .set_breakpoint(host::test_support::counting_increment())
            .unwrap();
        let (sender, updates) = mpsc::channel();
        let running = Emulator::start(
            host::test_support::counting_device().with_points(points),
            Speed::Fastest,
            Box::new(move |update| {
                let _ = sender.send(update);
            }),
        )
        .unwrap();
        let (kept, _) = shell.keep(1, running);
        assert!(kept);
        let asked = Instant::now();
        loop {
            assert!(asked.elapsed() < WAIT, "no stop");
            if let Ok(Update::Status(status)) = updates.recv_timeout(WAIT)
                && status.stop.is_some()
            {
                break;
            }
        }
        let view = shell
            .handle_of(1)
            .unwrap()
            .inspect(ViewRequest::default())
            .unwrap();
        assert_eq!(view.cpu.pc, host::test_support::counting_increment());
        assert_eq!(view.points.breakpoints, [view.cpu.pc]);
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["cpu"]["registers"][1]["name"], "ra");
        assert_eq!(shell.handle_of(0).unwrap_err().kind, "stopped");
        assert_eq!(shell.power_off(), Ok(()));
    }

    fn form(max_time: &str, max_cycles: &str) -> SetupDto {
        serde_json::from_value(serde_json::json!({
            "device": "x4",
            "image": null,
            "firmware": "/tmp/app.bin",
            "bootloader": null,
            "flashFile": null,
            "resetFlash": false,
            "romDir": null,
            "usbHost": "open",
            "strictMmio": false,
            "record": null,
            "maxTime": max_time,
            "maxCycles": max_cycles,
            "console": "uart0",
            "usbPower": true,
            "sd": { "path": "/tmp/card.img", "readOnly": false, "createMib": 1024 },
        }))
        .unwrap()
    }

    #[test]
    fn the_form_reads_as_the_options_of_inkemu_run() {
        let setup = form("2.5", "1000").into_setup().unwrap();
        assert_eq!(setup.limits.max_device_nanos, Some(2_500_000_000));
        assert_eq!(setup.limits.max_cycles, Some(1000));
        assert_eq!(setup.console, Some(ConsoleChannel::Uart0));
        assert_eq!(setup.firmware, Some(PathBuf::from("/tmp/app.bin")));
        assert!(setup.usb_power);
        assert_eq!(
            setup.sd,
            Some(SdSetup {
                path: PathBuf::from("/tmp/card.img"),
                read_only: false,
                create_mib: Some(1024),
            })
        );
        let none = form(" ", "").into_setup().unwrap();
        assert_eq!(none.limits, Limits::default());
        assert_eq!(form("-1", "").into_setup().unwrap_err().kind, "maxTime");
        assert_eq!(form("", "ten").into_setup().unwrap_err().kind, "maxCycles");
    }

    #[test]
    fn a_shell_without_an_emulator_says_it_has_stopped() {
        let shell = Shell::default();
        assert_eq!(shell.handle_of(1).unwrap_err().kind, "stopped");
        assert!(!shell.is_on());
        assert!(!shell.holds_flash_file(std::path::Path::new("flash.bin")));
        assert_eq!(shell.power_off(), Ok(()));
    }

    /// The dialog before closing speaks the language the interface last said, English before it
    /// says one; the host's lines follow as they are.
    #[test]
    fn the_closing_dialog_speaks_the_language_of_the_interface() {
        let shell = Shell::default();
        assert_eq!(shell.language(), Language::En);
        *shell
            .language
            .lock()
            .unwrap_or_else(PoisonError::into_inner) =
            serde_json::from_str::<Language>(r#""fr""#).unwrap();
        assert_eq!(shell.language(), Language::Fr);
        let (title, text) = closing_dialog(shell.language(), "error: cannot write flash.bin");
        assert!(title.contains("éteint sans tout conserver"), "{title}");
        assert!(text.starts_with("L'appareil a été éteint"), "{text}");
        assert!(text.ends_with("error: cannot write flash.bin"), "{text}");
        let (title, text) = closing_dialog(Language::En, "x");
        assert!(
            title.contains("powered off without keeping everything"),
            "{title}"
        );
        assert!(text.ends_with("\n\nx"), "{text}");
        assert!(serde_json::from_str::<Language>(r#""de""#).is_err());
    }

    /// A power-off under way (a slow one: its flash file is still being written) keeps the
    /// shell on, and a power-off of the shell waits for it: the application cannot close
    /// before it ends.
    #[test]
    fn a_power_off_under_way_keeps_the_shell_on_until_it_ends() {
        let shell = Shell::default();
        let under_way = shell.stopping.begin();
        assert!(shell.is_on());
        let slow = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            drop(under_way);
        });
        let asked = std::time::Instant::now();
        assert_eq!(shell.power_off(), Ok(()));
        assert!(asked.elapsed() >= std::time::Duration::from_millis(250));
        assert!(!shell.is_on());
        slow.join().unwrap();
    }

    /// A command is for the emulator of one start: refused as stopped when another runs, or none.
    #[test]
    fn a_command_for_a_replaced_emulator_is_refused() {
        assert!(same_start(Some(2), 2).is_ok());
        assert_eq!(same_start(Some(3), 2).unwrap_err().kind, "stopped");
        assert_eq!(same_start(None, 2).unwrap_err().kind, "stopped");
        assert_eq!(Shell::default().handle_of(1).unwrap_err().kind, "stopped");
    }

    #[test]
    fn a_capture_of_the_interface_is_a_command_of_the_host() {
        let read = serde_json::from_str::<CommandDto>(r#"{"kind":"capture","path":"/tmp/a.png"}"#)
            .unwrap()
            .into_command()
            .unwrap();
        assert_eq!(
            read,
            Command::Capture {
                path: PathBuf::from("/tmp/a.png")
            }
        );
    }
}
