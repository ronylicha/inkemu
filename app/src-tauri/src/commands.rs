//! The commands of the interface (app/src/ipc/host.ts calls them by these names). Each one turns
//! what the interface sends into the types of `host`, calls it, and turns its answer back; a
//! refusal is an [`ErrorDto`] whose `kind` the interface translates.
use host::catalog::{self, DeviceView, Look};
use host::device::{
    self, Command, CommandError, Emulator, Handle, LaunchError, Setup, Speed, Update,
};
use host::engine::{Limits, Notice};
use serde::{Deserialize, Serialize};
use soc_esp32c3::{ConsoleChannel, UsbHost};
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
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

/// The emulator the window drives, if one runs, and the language the interface speaks.
#[derive(Debug, Default)]
pub struct Shell {
    emulator: Mutex<Option<Emulator>>,
    language: Mutex<Language>,
}

impl Shell {
    /// Ends the last life of the running emulator, if any, and waits for it.
    ///
    /// # Errors
    /// What the last life could not keep (its flash file, its session), one error per line: the
    /// application tells its user before it closes.
    pub fn power_off(&self) -> Result<(), String> {
        let running = self
            .emulator
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        match running {
            Some(emulator) => emulator.shutdown().map_err(|e| e.to_string()),
            None => Ok(()),
        }
    }

    /// The language the interface last said it speaks; English until it says.
    #[must_use]
    pub fn language(&self) -> Language {
        *self.language.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether an emulator runs, which a power-off must end before the application closes.
    pub fn is_on(&self) -> bool {
        self.emulator
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// Whether the running emulator holds the flash file at `path`.
    fn holds_flash_file(&self, path: &std::path::Path) -> bool {
        self.emulator
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|emulator| emulator.holds_flash_file(path))
    }

    fn handle(&self) -> Result<Handle, ErrorDto> {
        self.emulator
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(Emulator::handle)
            .ok_or_else(|| ErrorDto::from(CommandError::Stopped))
    }

    /// Keeps `emulator`, and gives back the one it replaces (two starts at once), which the caller
    /// powers off without holding the lock.
    fn keep(&self, emulator: Emulator) -> Option<Emulator> {
        self.emulator
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(emulator)
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
    Speed { speed: Speed },
    Console { channel: String, bytes: Vec<u8> },
    UsbHost { host: String },
    Button { id: String, pressed: bool },
    Record { path: PathBuf },
    Capture { path: PathBuf },
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
    channel: Channel<Update>,
) -> Result<StartedDto, ErrorDto> {
    let started = StartedDto {
        notices: launch.notices.clone(),
        look: launch.look,
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
    if let Some(replaced) = shell.keep(emulator) {
        // What its last life could not keep reaches the journal through its own updates, which
        // the interface keeps (app/src/state/reducer.ts).
        let _ = tauri::async_runtime::spawn_blocking(move || replaced.shutdown()).await?;
    }
    Ok(started)
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
/// start leaves it as it was. Only a start on the flash file it holds powers it off first, since
/// its power-off writes that file and releases its lock.
#[tauri::command]
pub async fn power_on(
    shell: State<'_, Shell>,
    setup: SetupDto,
    speed: Speed,
    on_update: Channel<Update>,
) -> Result<StartedDto, ErrorDto> {
    let setup = setup.into_setup()?;
    let shell = shell.inner();
    if setup
        .flash_file
        .as_deref()
        .is_some_and(|file| shell.holds_flash_file(file))
    {
        off_the_main_thread(shell).await?;
    }
    let launch = tauri::async_runtime::spawn_blocking(move || device::power_on(&setup)).await??;
    start(shell, launch, speed, on_update).await
}

/// Opens a session and replays it as `inkemu replay` does.
#[tauri::command]
pub async fn open_replay(
    shell: State<'_, Shell>,
    path: PathBuf,
    rom_dir: Option<PathBuf>,
    speed: Speed,
    on_update: Channel<Update>,
) -> Result<StartedDto, ErrorDto> {
    // A replay holds no file of a run: the device that runs goes on until the replay is ready.
    let launch =
        tauri::async_runtime::spawn_blocking(move || device::open_replay(&path, rom_dir, None))
            .await??;
    start(shell.inner(), launch, speed, on_update).await
}

/// Powers off the emulator that runs, off the interface's thread: its last life writes its files.
async fn off_the_main_thread(shell: &Shell) -> Result<(), ErrorDto> {
    let running = shell
        .emulator
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    if let Some(emulator) = running {
        // What its last life could not keep reaches the journal through its own updates.
        let _ = tauri::async_runtime::spawn_blocking(move || emulator.shutdown()).await?;
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
    answer: Channel<Option<ErrorDto>>,
) -> Result<(), ErrorDto> {
    let pending = shell.handle()?.submit(command.into_command()?)?;
    tauri::async_runtime::spawn_blocking(move || {
        // A window that closed takes no answer.
        let _ = answer.send(pending.wait().err().map(ErrorDto::from));
    });
    Ok(())
}

/// The last image of the panel, encoded (`PanelFrame::encode`); empty before the first.
// Tauri hands a command its state by value: the signature is the framework's.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn frame(shell: State<'_, Shell>) -> Response {
    let bytes = shell
        .handle()
        .ok()
        .and_then(|handle| handle.frame())
        .map(|frame| frame.encode())
        .unwrap_or_default();
    Response::new(bytes)
}

/// The notices of the material inkemu embeds (`inkemu licenses`).
#[tauri::command]
pub fn notices() -> String {
    boards::notices::notices_text()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let none = form(" ", "").into_setup().unwrap();
        assert_eq!(none.limits, Limits::default());
        assert_eq!(form("-1", "").into_setup().unwrap_err().kind, "maxTime");
        assert_eq!(form("", "ten").into_setup().unwrap_err().kind, "maxCycles");
    }

    #[test]
    fn a_shell_without_an_emulator_says_it_has_stopped() {
        let shell = Shell::default();
        assert_eq!(shell.handle().unwrap_err().kind, "stopped");
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
