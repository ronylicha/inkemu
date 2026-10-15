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

/// The emulator the window drives, if one runs.
#[derive(Debug, Default)]
pub struct Shell {
    emulator: Mutex<Option<Emulator>>,
}

impl Shell {
    /// Ends the last life of the running emulator, if any, and waits for it.
    pub fn power_off(&self) {
        let running = self
            .emulator
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(emulator) = running {
            emulator.shutdown();
        }
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
        tauri::async_runtime::spawn_blocking(move || replaced.shutdown()).await?;
    }
    Ok(started)
}

/// The built-in devices.
#[tauri::command]
pub fn devices() -> Result<Vec<DeviceView>, ErrorDto> {
    catalog::devices().map_err(|e| ErrorDto::new("profile", e))
}

/// Powers a device on as `inkemu run` does. The checks and the ROM, which may be downloaded, run
/// off the interface's thread.
#[tauri::command]
pub async fn power_on(
    shell: State<'_, Shell>,
    setup: SetupDto,
    speed: Speed,
    on_update: Channel<Update>,
) -> Result<StartedDto, ErrorDto> {
    let setup = setup.into_setup()?;
    let previous = shell.inner();
    off_the_main_thread(previous).await?;
    let launch = tauri::async_runtime::spawn_blocking(move || device::power_on(&setup)).await??;
    start(shell.inner(), launch, speed, on_update).await
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
    off_the_main_thread(shell.inner()).await?;
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
        tauri::async_runtime::spawn_blocking(move || emulator.shutdown()).await?;
    }
    Ok(())
}

/// Sends a command to the emulator and waits for its answer, off the interface's thread.
#[tauri::command]
pub async fn command(shell: State<'_, Shell>, command: CommandDto) -> Result<(), ErrorDto> {
    let command = command.into_command()?;
    let handle = shell.handle()?;
    tauri::async_runtime::spawn_blocking(move || handle.command(command))
        .await?
        .map_err(ErrorDto::from)
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

/// Writes a PNG of the screen as the user sees it to `path`, chosen in the system's dialog.
#[tauri::command]
pub async fn save_capture(shell: State<'_, Shell>, path: PathBuf) -> Result<(), ErrorDto> {
    let png = shell
        .handle()?
        .capture()
        .ok_or_else(|| ErrorDto::new("noImage", "the panel has shown no image yet"))?;
    tauri::async_runtime::spawn_blocking(move || std::fs::write(&path, png))
        .await?
        .map_err(|e| ErrorDto::new("capture", e))
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
        shell.power_off();
    }
}
