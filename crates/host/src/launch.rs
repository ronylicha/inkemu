//! How a run starts, the same for `inkemu run` and for the interface: the device and its console,
//! the flash it starts from, the files it must not confuse, the ROM, and the lines that name them.
//!
//! These decisions lived in the binary of the command line; they are here so that a front end that
//! starts a run makes them the same way, in the same order (design of sub-project 2c, C1). The texts are the
//! command line's, and name its options: a front end with its own words chooses them by the
//! variant of the error, and shows these texts as the detail (C2).
use crate::firmware_file::{self, FirmwareFileError};
use crate::flash_file::{self, FlashFileError};
use crate::flash_state::{self, FlashStateError, Input, RunFlash, Seed};
use crate::rom_store::{self, AnnouncingFetch, ESP_ROM_ELFS, HttpFetch, RomEntry, RomSource};
use crate::run::RunOptions;
use crate::session_file::{self, Claimed, SessionFileError};
use crate::term::sanitize;
use boards::{DeviceProfile, ProfileError};
use firmware::BootloaderWarning;
use session::Limits;
use soc_esp32c3::{ConsoleChannel, Esp32C3, RomImage, UsbHost};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The built-in profile named `id`; an unknown name is echoed with control characters escaped,
/// since it comes from the user.
///
/// # Errors
/// [`ProfileError::UnknownDevice`] for a name inkemu does not know, and any refusal of the built-in
/// profile itself.
pub fn profile(id: &str) -> Result<DeviceProfile, ProfileError> {
    boards::builtin(id).map_err(|e| match e {
        ProfileError::UnknownDevice { id, known } => ProfileError::UnknownDevice {
            id: sanitize(&id),
            known,
        },
        other @ (ProfileError::Parse { .. }
        | ProfileError::MissingFlash { .. }
        | ProfileError::MissingConsole { .. }
        | ProfileError::IdMismatch { .. }
        | ProfileError::Partitions { .. }
        | ProfileError::NoBootloader { .. }) => other,
    })
}

/// What a run was told about the flash of the device.
#[derive(Clone, Copy, Debug, Default)]
pub struct FlashRequest<'a> {
    /// `--flash`: the image a run starts from, never written.
    pub image: Option<&'a Path>,
    /// `--firmware`: the application a flash is composed around, never written.
    pub firmware: Option<&'a Path>,
    /// `--bootloader`: the bootloader of that flash, never written.
    pub bootloader: Option<&'a Path>,
    /// `--flash-file`: where the flash is kept from one run to the next.
    pub file: Option<&'a Path>,
    /// `--reset-flash`: start again from the image and replace the file.
    pub reset: bool,
}

/// What a run was told about how to run: its limits, and the console to show if one was named.
/// The console becomes that of the device profile otherwise ([`shown_console`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct RunRequest {
    pub max_cycles: Option<u64>,
    pub max_device_nanos: Option<u64>,
    /// `--console`, when it was given.
    pub console: Option<ConsoleChannel>,
}

impl RunRequest {
    /// The limits of the session of a run (G3), `--strict-mmio` given as `strict_mmio`.
    #[must_use]
    pub const fn limits(&self, strict_mmio: bool) -> Limits {
        Limits {
            strict_mmio,
            max_cycles: self.max_cycles,
            max_time_nanos: self.max_device_nanos,
            cut: None,
        }
    }

    /// The options of a run of `profile` whose USB host starts as `usb_host`, and the part of its
    /// first line that names the console shown ([`console_note`]).
    #[must_use]
    pub fn resolve(&self, profile: &DeviceProfile, usb_host: UsbHost) -> (RunOptions, String) {
        let (console, origin) = shown_console(profile, self.console);
        let options = RunOptions {
            max_cycles: self.max_cycles,
            max_device_nanos: self.max_device_nanos,
            console,
        };
        (options, console_note(console, origin, usb_host))
    }
}

/// Where the console a run shows comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConsoleOrigin {
    /// `--console` named it (in the interface: the user chose it).
    Option,
    /// The device profile names it, and no other was asked for.
    Profile,
}

/// The console a run of `profile` shows: the one asked for, else the one the profile names, and
/// where it came from.
#[must_use]
pub fn shown_console(
    profile: &DeviceProfile,
    asked: Option<ConsoleChannel>,
) -> (ConsoleChannel, ConsoleOrigin) {
    asked.map_or((profile.console, ConsoleOrigin::Profile), |channel| {
        (channel, ConsoleOrigin::Option)
    })
}

/// Whether nothing reaches `channel` while the USB host is `usb_host`: the device sends nothing
/// to its USB-Serial-JTAG until a host opens the port.
#[must_use]
pub fn console_silenced_by_host(channel: ConsoleChannel, usb_host: UsbHost) -> bool {
    channel == ConsoleChannel::UsbSerialJtag && usb_host != UsbHost::Open
}

/// The part of the first line of a run that names the console shown and where the choice came
/// from: a user who sees nothing knows which console to ask for instead. When the console is
/// USB-Serial-JTAG and the USB host does not start open, the device sends nothing there until a
/// host opens the port, and the note says so and how to see the output.
#[must_use]
pub fn console_note(channel: ConsoleChannel, origin: ConsoleOrigin, usb_host: UsbHost) -> String {
    let mut note = match origin {
        ConsoleOrigin::Option => format!("console: {} (--console)", channel.name()),
        ConsoleOrigin::Profile => format!(
            "console: {} (the default of the device profile; --console shows another)",
            channel.name()
        ),
    };
    if console_silenced_by_host(channel, usb_host) {
        // Writing to a `String` cannot fail.
        let _ = write!(
            note,
            "; the USB host starts {}: nothing reaches this console until a host opens the port, so --usb-host open or --console uart0 shows the output",
            usb_host.name()
        );
    }
    note
}

/// The last component of `path`, for a line that names a file the user gave: escaped, since it
/// comes straight from the user.
#[must_use]
pub fn file_name(path: &Path) -> String {
    sanitize(&path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    ))
}

/// The one line that says what is being emulated, how much to trust its timing and which console
/// is shown (`console_note`). The name of the profile is escaped: a replay takes it from a
/// session, which comes from anywhere.
#[must_use]
pub fn header(
    profile: &DeviceProfile,
    entry: &RomEntry,
    machine: &Esp32C3,
    console_note: &str,
) -> String {
    let timing = machine.timing_model();
    format!(
        "inkemu: {} (ESP32-C3 {}, mask ROM rev{}); timing model {} ({}); {console_note}",
        sanitize(&profile.name),
        profile.chip_revision,
        entry.rom_revision,
        timing.name(),
        if timing.is_calibrated() {
            "calibrated"
        } else {
            "not calibrated"
        }
    )
}

/// The part of the first line of a run that names its flash: the flash file it starts from, or
/// the starting flash (`fresh`) and the flash file it is kept in, if any.
#[must_use]
pub fn flash_note(
    file: Option<&Path>,
    run_flash: &RunFlash,
    fresh: &str,
    flash_size: u32,
) -> String {
    match file {
        Some(path) if run_flash.starts_from_the_file() => format!(
            "flash: {} (flash file, {} MiB)",
            file_name(path),
            flash_size >> 20
        ),
        Some(path) => format!("flash: {fresh}, kept in {}", file_name(path)),
        None => format!("flash: {fresh}"),
    }
}

/// The flash a run starts from when nothing is kept from an earlier run.
#[derive(Debug)]
pub struct Starting {
    /// What the flash holds (`None`: erased).
    pub seed: Option<Seed>,
    /// How the header line of the run names it.
    pub note: String,
    /// What the check of a given bootloader found that does not stop the run, to be said only
    /// once the flash file accepts the seed.
    pub warnings: Vec<BootloaderWarning>,
}

/// Why no starting flash could be made of the files given.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// The `--flash` image was refused.
    #[error("{0}")]
    Flash(#[from] FlashFileError),
    /// The `--firmware` application, or its `--bootloader`, was refused.
    #[error("{0}")]
    Firmware(#[from] FirmwareFileError),
}

/// The flash a run starts from when nothing is kept from an earlier run: the `--flash` image, the
/// flash composed around `--firmware`, or an erased flash.
///
/// # Errors
/// [`StartError`] when a file given is refused.
pub fn starting_flash(
    profile: &DeviceProfile,
    flash: &FlashRequest<'_>,
) -> Result<Starting, StartError> {
    let flash_size = profile.flash.size();
    let (seed, note, warnings) = match (flash.image, flash.firmware) {
        (Some(path), _) => {
            let bytes = flash_file::load(path, profile.soc.chip_id(), flash_size)?;
            let note = format!(
                "{} ({} bytes in {} MiB)",
                file_name(path),
                bytes.len(),
                flash_size >> 20
            );
            let seed = Seed {
                input: Input::Flash,
                content: bytes,
            };
            (Some(seed), note, Vec::new())
        }
        (None, Some(path)) => {
            let composed = firmware_file::compose(profile, path, flash.bootloader)?;
            let app = &composed.application;
            let note = format!(
                "{} ({} {}, ESP-IDF {}) in {} at {:#x}, behind {}, under the partition table of the profile ({} MiB)",
                file_name(path),
                sanitize(&app.project_name),
                sanitize(&app.version),
                sanitize(&app.idf_ver),
                composed.slot.name,
                composed.slot.offset,
                sanitize(&composed.bootloader.to_string()),
                flash_size >> 20
            );
            let seed = Seed {
                input: Input::Firmware,
                content: composed.bytes,
            };
            (Some(seed), note, composed.warnings)
        }
        (None, None) => (
            None,
            format!("erased ({} MiB)", flash_size >> 20),
            Vec::new(),
        ),
    };
    Ok(Starting {
        seed,
        note,
        warnings,
    })
}

/// Refuses a file the run only reads (`--flash`, `--firmware`, `--bootloader`) that is the flash
/// file, or a file the flash file owns ([`flash_state::refuse_the_image_as_flash_file`]).
///
/// # Errors
/// The [`FlashStateError`] of the first such file.
pub fn refuse_the_images_as_flash_file(flash: &FlashRequest<'_>) -> Result<(), FlashStateError> {
    let Some(file) = flash.file else {
        return Ok(());
    };
    for (input, path) in [
        (Input::Flash, flash.image),
        (Input::Firmware, flash.firmware),
        (Input::Bootloader, flash.bootloader),
    ] {
        if let Some(path) = path {
            flash_state::refuse_the_image_as_flash_file(input, path, file)?;
        }
    }
    Ok(())
}

/// Why a path cannot take the session of a run.
#[derive(Debug, thiserror::Error)]
pub enum RecordPathError {
    /// The path itself is refused.
    #[error("{0}")]
    Path(#[from] SessionFileError),
    /// The text of the profile a session carries is not there.
    #[error("{0}")]
    Profile(#[from] ProfileError),
}

/// Refuses, before anything of the run, a `--record` path a session must not be written to: a
/// path the run reads or writes, existing or not (a session is written over what is at its
/// path): the files given to the run, the flash file and the files beside it, the ROM file of
/// `rom` or, without a directory, the ROM cache. Gives the text of the profile of `device` that
/// a session carries, when the run is recorded.
///
/// # Errors
/// [`RecordPathError`] when the path is refused, or the device has no built-in profile text.
pub fn refuse_record_path(
    record: Option<&Path>,
    flash: &FlashRequest<'_>,
    console_script: Option<&Path>,
    rom: (Option<&Path>, &RomEntry),
    device: &str,
) -> Result<Option<&'static str>, RecordPathError> {
    let Some(path) = record else {
        return Ok(None);
    };
    let claimed = claimed_paths(flash, console_script, Some(rom));
    session_file::refuse_record_path(path, &claimed)?;
    Ok(Some(boards::builtin_toml(device)?))
}

/// The paths a run reads or writes, which nothing it writes besides may replace: the files given
/// to the run, the flash file and the files beside it, and, when `rom` names the ROM of the run,
/// the ROM file of its directory or, without a directory, the ROM cache.
#[must_use]
pub fn claimed_paths(
    flash: &FlashRequest<'_>,
    console_script: Option<&Path>,
    rom: Option<(Option<&Path>, &RomEntry)>,
) -> Vec<Claimed> {
    let mut claimed = Vec::new();
    for (option, input) in [
        ("--flash", flash.image),
        ("--firmware", flash.firmware),
        ("--bootloader", flash.bootloader),
        ("--console-script", console_script),
    ] {
        if let Some(input) = input {
            claimed.push(Claimed::File {
                path: input.to_path_buf(),
                what: format!("the file of {option}, which the run reads"),
            });
        }
    }
    if let Some(file) = flash.file {
        claimed.extend(flash_state::files_of_the_flash_file(file).into_iter().map(
            |(path, what)| Claimed::File {
                path,
                what: what.to_owned(),
            },
        ));
    }
    claimed.extend(rom.and_then(|(rom_dir, entry)| rom_claim(rom_dir, entry)));
    claimed
}

/// The ROM file of `entry` in `rom_dir` or, without a directory, the ROM cache: what a run or a
/// replay reads its ROM from. `None` without a home directory, where there is no cache.
#[must_use]
pub fn rom_claim(rom_dir: Option<&Path>, entry: &RomEntry) -> Option<Claimed> {
    match rom_dir {
        Some(dir) => Some(Claimed::File {
            path: dir.join(entry.elf_name),
            what: "the ROM file of --rom-dir, which the run reads".to_owned(),
        }),
        // Without a home directory there is no cache: loading the ROM says so.
        None => rom_store::default_cache_dir()
            .ok()
            .map(|dir| Claimed::Directory {
                path: dir,
                what: "in the ROM cache, whose files the run reads and downloads".to_owned(),
            }),
    }
}

/// The mask ROM of `entry`, from `rom_dir`, read only, or from the per-user cache, which downloads
/// it on first use and says so in `log` when the download starts.
///
/// # Errors
/// [`rom_store::RomStoreError`] when the ROM cannot be found, downloaded or verified.
pub fn load_rom(
    rom_dir: Option<PathBuf>,
    entry: &RomEntry,
    log: &mut dyn std::io::Write,
) -> Result<RomImage, rom_store::RomStoreError> {
    let http = HttpFetch;
    let announcing;
    // An explicit directory is read only; without one the cache downloads on first use.
    let source = if let Some(dir) = rom_dir {
        RomSource::Directory(dir)
    } else {
        let dir = rom_store::default_cache_dir()?;
        announcing = AnnouncingFetch::new(&http, log, &ESP_ROM_ELFS, &dir);
        RomSource::Cache {
            dir,
            fetch: &announcing,
            user_named: false,
        }
    };
    rom_store::load_rom(&source, &ESP_ROM_ELFS, entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use boards::MachineOptions;
    use soc_esp32c3::test_support::{HANG, flat_rom};

    /// Without `--console` a run shows the console of its profile, and says so; with it, the one
    /// asked for, and says that instead.
    #[test]
    fn the_console_shown_is_the_profile_s_unless_one_is_asked_for() {
        for id in ["x3", "x4"] {
            let profile = profile(id).unwrap();
            let (channel, origin) = shown_console(&profile, None);
            assert_eq!(
                (channel, origin),
                (ConsoleChannel::UsbSerialJtag, ConsoleOrigin::Profile),
                "{id}"
            );
            assert_eq!(
                console_note(channel, origin, UsbHost::Open),
                "console: usb-serial-jtag (the default of the device profile; --console shows another)"
            );
            for asked in ConsoleChannel::ALL {
                let (channel, origin) = shown_console(&profile, Some(asked));
                assert_eq!((channel, origin), (asked, ConsoleOrigin::Option), "{id}");
                assert_eq!(
                    console_note(channel, origin, UsbHost::Open),
                    format!("console: {} (--console)", asked.name())
                );
            }
        }
    }

    /// `resolve` is what a run calls: the console it gives the run and the one its note names are
    /// the profile's without `--console` and the option's with it, and the limits pass through.
    #[test]
    fn resolve_gives_the_run_the_console_its_note_names() {
        let profile = profile("x4").unwrap();
        for (asked, shown, note) in [
            (
                None,
                ConsoleChannel::UsbSerialJtag,
                "console: usb-serial-jtag (the default of the device profile; --console shows another)",
            ),
            (
                Some(ConsoleChannel::Uart0),
                ConsoleChannel::Uart0,
                "console: uart0 (--console)",
            ),
        ] {
            let request = RunRequest {
                max_cycles: Some(7),
                max_device_nanos: Some(11),
                console: asked,
            };
            let (options, resolved_note) = request.resolve(&profile, UsbHost::Open);
            assert_eq!(options.console, shown, "{asked:?}");
            assert_eq!(resolved_note, note, "{asked:?}");
            assert_eq!(
                (options.max_cycles, options.max_device_nanos),
                (Some(7), Some(11))
            );
        }
    }

    /// The first line of a run carries the console note between the timing model and what
    /// follows it.
    #[test]
    fn the_first_line_names_the_console_of_the_profile() {
        let profile = profile("x4").unwrap();
        let entry = rom_store::rom_entry_for(profile.chip_revision);
        let machine = profile.instantiate(flat_rom(&[HANG]), MachineOptions::default());
        let request = RunRequest::default();
        let (_, note) = request.resolve(&profile, UsbHost::Open);
        let line = header(&profile, entry, &machine, &note);
        assert!(line.starts_with("inkemu: Xteink X4 (ESP32-C3 "), "{line}");
        assert!(
            line.ends_with(
                "; console: usb-serial-jtag (the default of the device profile; --console shows another)"
            ),
            "{line}"
        );
    }

    /// A replay takes its profile from a session, which comes from anywhere: a name that would
    /// drive the terminal reaches it escaped.
    #[test]
    fn the_first_line_escapes_the_name_of_the_profile() {
        let text = boards::builtin_toml("x4").unwrap().replacen(
            "name = \"Xteink X4\"",
            "name = \"\\u001b]0;title\\u0007X4\"",
            1,
        );
        let profile = DeviceProfile::from_named_toml("x4", "the profile of the session", &text)
            .expect("a name may hold any character");
        let entry = rom_store::rom_entry_for(profile.chip_revision);
        let machine = profile.instantiate(flat_rom(&[HANG]), MachineOptions::default());
        let line = header(&profile, entry, &machine, "console: uart0 (--console)");
        assert!(
            line.starts_with("inkemu: \\u{1b}]0;title\\u{7}X4 (ESP32-C3 "),
            "{line:?}"
        );
    }

    /// A USB-Serial-JTAG console whose host does not start open shows nothing until a host opens
    /// the port: the note says so and how to see the output. Another console, or an open host,
    /// gets no such clause.
    #[test]
    fn the_console_note_says_when_the_usb_host_keeps_the_console_silent() {
        for host in [UsbHost::Absent, UsbHost::Attached] {
            assert!(console_silenced_by_host(
                ConsoleChannel::UsbSerialJtag,
                host
            ));
            for origin in [ConsoleOrigin::Profile, ConsoleOrigin::Option] {
                let note = console_note(ConsoleChannel::UsbSerialJtag, origin, host);
                assert!(
                    note.ends_with(&format!(
                        "; the USB host starts {}: nothing reaches this console until a host opens the port, so --usb-host open or --console uart0 shows the output",
                        host.name()
                    )),
                    "{note}"
                );
            }
        }
        for host in UsbHost::ALL {
            for channel in [ConsoleChannel::Uart0, ConsoleChannel::Uart1] {
                assert!(!console_silenced_by_host(channel, host));
                let note = console_note(channel, ConsoleOrigin::Option, host);
                assert_eq!(note, format!("console: {} (--console)", channel.name()));
            }
        }
        assert!(
            !console_note(
                ConsoleChannel::UsbSerialJtag,
                ConsoleOrigin::Profile,
                UsbHost::Open
            )
            .contains("USB host")
        );
    }

    /// An unknown device is refused with its name escaped: it comes from the user.
    #[test]
    fn an_unknown_device_is_named_escaped() {
        let Err(ProfileError::UnknownDevice { id, known }) = profile("x\u{1b}9") else {
            panic!("an unknown device is refused")
        };
        assert_eq!(id, "x\\u{1b}9");
        assert!(known.contains("x4"), "{known}");
    }

    /// No file given: the run starts from an erased flash, named with its size.
    #[test]
    fn nothing_given_starts_from_an_erased_flash() {
        let profile = profile("x4").unwrap();
        let starting = starting_flash(&profile, &FlashRequest::default()).unwrap();
        assert!(starting.seed.is_none());
        assert_eq!(starting.note, "erased (16 MiB)");
        assert_eq!(starting.warnings.len(), 0, "{:?}", starting.warnings);
    }

    /// A refusal of the image keeps its own words: the command line prints them after `error: `.
    #[test]
    fn a_refused_image_reads_as_the_refusal_of_the_flash_file_module() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.bin");
        let flash = FlashRequest {
            image: Some(&path),
            ..FlashRequest::default()
        };
        let error = starting_flash(&profile("x4").unwrap(), &flash).unwrap_err();
        assert!(matches!(
            error,
            StartError::Flash(FlashFileError::Read { .. })
        ));
        let direct = flash_file::load(&path, firmware::ChipId::Esp32C3, 16 << 20).unwrap_err();
        assert_eq!(error.to_string(), direct.to_string());
    }

    /// The same file given as the image and as the flash file is refused before it is read.
    #[test]
    fn an_image_that_is_the_flash_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flash.bin");
        std::fs::write(&path, [0xFF; 16]).unwrap();
        let flash = FlashRequest {
            image: Some(&path),
            file: Some(&path),
            ..FlashRequest::default()
        };
        assert!(refuse_the_images_as_flash_file(&flash).is_err());
        let apart = FlashRequest {
            image: Some(&path),
            file: Some(&dir.path().join("kept.bin")),
            ..FlashRequest::default()
        };
        assert!(refuse_the_images_as_flash_file(&apart).is_ok());
    }

    /// A session may not be written over a file the run reads, and an unrecorded run asks for no
    /// profile text.
    #[test]
    fn a_session_path_that_is_a_file_of_the_run_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("flash.bin");
        let flash = FlashRequest {
            image: Some(&image),
            ..FlashRequest::default()
        };
        let entry = rom_store::rom_entry_for(profile("x4").unwrap().chip_revision);
        let rom = (Some(dir.path()), entry);
        assert!(matches!(
            refuse_record_path(None, &flash, None, rom, "x4"),
            Ok(None)
        ));
        assert!(matches!(
            refuse_record_path(Some(&image), &flash, None, rom, "x4"),
            Err(RecordPathError::Path(_))
        ));
        let text = refuse_record_path(
            Some(&dir.path().join("run.inkrec")),
            &flash,
            None,
            rom,
            "x4",
        )
        .unwrap();
        assert_eq!(text, Some(boards::builtin_toml("x4").unwrap()));
    }

    /// A ROM directory without the ROM is refused without a download, and nothing is announced.
    #[test]
    fn a_rom_directory_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let entry = rom_store::rom_entry_for(profile("x4").unwrap().chip_revision);
        let mut log = Vec::new();
        let error = load_rom(Some(dir.path().to_path_buf()), entry, &mut log).unwrap_err();
        assert!(
            matches!(error, rom_store::RomStoreError::NotInDirectory { .. }),
            "{error}"
        );
        assert_eq!(log, b"");
    }

    /// The file name of a path is its last component, escaped.
    #[test]
    fn a_file_name_is_the_last_component_escaped() {
        assert_eq!(file_name(Path::new("/a/b/fw\u{7}.bin")), "fw\\u{7}.bin");
        assert_eq!(file_name(Path::new("/")), "/");
    }
}
