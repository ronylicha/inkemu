//! inkemu command-line interface.
use boards::{DeviceProfile, MachineOptions, SocFamily};
use clap::{Parser, Subcommand};
use cpu_rv32::{HartConfig, MisalignedPolicy};
use emu_cli::exit::{EXIT_ERROR, EXIT_SECOND_INTERRUPT, EXIT_TEST_FAILURE, RUN_EXIT_CODES_HELP};
use emu_cli::flash_file;
use emu_cli::rom_store::{
    self, AnnouncingFetch, ESP_ROM_ELFS, ESP32C3_ROMS, HttpFetch, RomEntry, RomSource,
};
use emu_cli::run::{RunOptions, execute, parse_device_seconds, relay};
use emu_cli::rvtest::{RunResult, TestMachine};
use emu_cli::term::sanitize;
use firmware::ChipId;
use soc_esp32c3::ConsoleChannel;
use std::fmt::Write as _;
use std::io;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Parser)]
#[command(
    name = "inkemu",
    version,
    about = "Binary-level emulator for embedded e-ink devices"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a bare-metal RISC-V conformance test ELF (riscv-tests or riscv-arch-test).
    Rvtest {
        elf: PathBuf,
        #[arg(long, default_value_t = 50_000_000)]
        max_steps: u64,
        /// Write the arch-test signature (one 8-digit hex word per line) to this file.
        #[arg(long)]
        signature: Option<PathBuf>,
        /// Print the execution trace hash (determinism checks).
        #[arg(long)]
        trace_hash: bool,
        /// Let the core perform misaligned loads and stores instead of trapping.
        #[arg(long)]
        allow_misaligned: bool,
    },
    /// Describe an ESP-IDF application image and say what is wrong with a file it refuses.
    Inspect { file: PathBuf },
    /// Boot a device from its mask ROM and print its console.
    #[command(after_help = RUN_EXIT_CODES_HELP)]
    Run {
        /// Device profile id; an unknown id prints the available ones.
        #[arg(long)]
        device: String,
        /// Full flash image to boot: a dump of a device (`esptool.py read_flash 0 ALL flash.bin`),
        /// or the output of `esptool.py merge_bin`. Its bootloader, partition table and
        /// application run as on the device. Without it the flash is erased and the ROM reports
        /// an invalid header, as a blank chip does.
        #[arg(long)]
        flash: Option<PathBuf>,
        /// Stop when the CPU has executed this many cycles of its clock. A sleeping CPU executes
        /// none: a firmware that sleeps never reaches this limit, use --max-time to bound it.
        #[arg(long)]
        max_cycles: Option<u64>,
        /// Stop when this many seconds of device time have elapsed since power-on (a decimal
        /// number: 30, 2.5, 0.001). Device time goes on while the CPU sleeps and across resets.
        #[arg(long, value_name = "SECONDS", value_parser = parse_device_seconds)]
        max_time: Option<u64>,
        /// Stop at the first access to a register the emulator does not model, 8/16-bit write to a
        /// peripheral register, operation the emulator cannot perform faithfully, or access to flash
        /// that is not mapped (exit code 3).
        #[arg(long)]
        strict_mmio: bool,
        /// Which console of the device goes to standard output.
        #[arg(long, value_enum, default_value_t = ConsoleArg::Uart0)]
        console: ConsoleArg,
        /// Use the ROM ELF files of this directory and never download.
        #[arg(long)]
        rom_dir: Option<PathBuf>,
        /// Print the execution trace hash at the end (determinism checks).
        #[arg(long)]
        trace_hash: bool,
    },
    /// Manage the mask-ROM files the emulator needs.
    Rom {
        #[command(subcommand)]
        action: RomAction,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum ConsoleArg {
    Uart0,
    Uart1,
    UsbSerialJtag,
}

impl From<ConsoleArg> for ConsoleChannel {
    fn from(arg: ConsoleArg) -> ConsoleChannel {
        match arg {
            ConsoleArg::Uart0 => ConsoleChannel::Uart0,
            ConsoleArg::Uart1 => ConsoleChannel::Uart1,
            ConsoleArg::UsbSerialJtag => ConsoleChannel::UsbSerialJtag,
        }
    }
}

#[derive(Subcommand)]
enum RomAction {
    /// Download and verify ROM files into the cache (or into --rom-dir).
    Fetch {
        /// Fetch the ROM this device needs.
        #[arg(long, conflicts_with = "all", required_unless_present = "all")]
        device: Option<String>,
        /// Fetch every ROM revision of the supported chips.
        #[arg(long)]
        all: bool,
        /// Store the files in this directory instead of the default cache.
        #[arg(long)]
        rom_dir: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Rvtest {
            elf,
            max_steps,
            signature,
            trace_hash,
            allow_misaligned,
        } => run_rvtest(
            &elf,
            max_steps,
            signature.as_deref(),
            trace_hash,
            allow_misaligned,
        ),
        Command::Inspect { file } => run_inspect(&file),
        Command::Run {
            device,
            flash,
            max_cycles,
            max_time,
            strict_mmio,
            console,
            rom_dir,
            trace_hash,
        } => run_device(
            &device,
            flash.as_deref(),
            rom_dir,
            MachineOptions {
                strict_mmio,
                trace_hash,
            },
            &RunOptions {
                max_cycles,
                max_device_nanos: max_time,
                console: console.into(),
            },
        ),
        Command::Rom {
            action:
                RomAction::Fetch {
                    device,
                    all,
                    rom_dir,
                },
        } => run_rom_fetch(device.as_deref(), all, rom_dir),
    }
}

/// Writes `line` and a newline to `stream`, standard error or standard output, and goes on if
/// that fails.
///
/// `eprintln!` and `println!` panic when the write fails (`2>/dev/full`, a pipe whose reader
/// left), and the process then ends with the code of a panic, 101, instead of the code that says
/// what happened. Standard error is where failures are reported: when it fails itself there is
/// nowhere left to say so, and the exit code must still be the right one.
fn write_line(mut stream: impl io::Write, line: &dyn std::fmt::Display) {
    let _ = writeln!(stream, "{line}");
}

/// Prints `error: {message}` and yields the input-error exit code.
fn fail(message: &dyn std::fmt::Display) -> ExitCode {
    write_line(io::stderr(), &format_args!("error: {message}"));
    ExitCode::from(EXIT_ERROR)
}

/// Writes `text` to standard output. `Break` carries the exit code to finish with: success when the
/// reader went away (`inkemu inspect fw.bin | head`), an error code for any other write failure.
fn print_stdout(text: &str) -> ControlFlow<ExitCode> {
    match relay(&mut io::stdout().lock(), text.as_bytes()) {
        Ok(true) => ControlFlow::Continue(()),
        Ok(false) => ControlFlow::Break(ExitCode::SUCCESS),
        Err(e) => ControlFlow::Break(fail(&format!(
            "cannot write to standard output: {e}; check where stdout is redirected"
        ))),
    }
}

/// The built-in profile named `id`; an unknown name is echoed with control characters escaped, since
/// it comes straight from the command line.
fn profile_for(id: &str) -> Result<DeviceProfile, boards::ProfileError> {
    boards::builtin(id).map_err(|e| match e {
        boards::ProfileError::UnknownDevice { id, known } => boards::ProfileError::UnknownDevice {
            id: sanitize(&id),
            known,
        },
        other @ (boards::ProfileError::Parse { .. }
        | boards::ProfileError::MissingFlash { .. }
        | boards::ProfileError::IdMismatch { .. }) => other,
    })
}

/// The chip id a firmware image must declare to run on the device of `profile`. The `match` is
/// exhaustive on purpose: a new chip family does not compile until it has its chip id here.
const fn image_chip(profile: &DeviceProfile) -> ChipId {
    match profile.soc {
        SocFamily::Esp32C3 => ChipId::Esp32C3,
    }
}

fn run_device(
    device: &str,
    flash: Option<&Path>,
    rom_dir: Option<PathBuf>,
    machine_options: MachineOptions,
    run_options: &RunOptions,
) -> ExitCode {
    let profile = match profile_for(device) {
        Ok(p) => p,
        Err(e) => return fail(&e),
    };
    // The flash file is checked first: refusing it needs no ROM, and no download should start for a
    // run that cannot happen.
    let flash_image = match flash {
        Some(path) => match flash_file::load(path, image_chip(&profile), profile.flash.size()) {
            Ok(bytes) => Some((path, bytes)),
            Err(e) => return fail(&e),
        },
        None => None,
    };
    let entry = rom_store::rom_entry_for(profile.chip_revision);
    let http = HttpFetch;
    let mut stderr = io::stderr();
    let announcing;
    // An explicit directory is read only; without one the cache downloads on first use.
    let source = match rom_dir {
        Some(dir) => RomSource::Directory(dir),
        None => match rom_store::default_cache_dir() {
            Ok(dir) => {
                announcing = AnnouncingFetch::new(&http, &mut stderr, &ESP_ROM_ELFS, &dir);
                RomSource::Cache {
                    dir,
                    fetch: &announcing,
                    user_named: false,
                }
            }
            Err(e) => return fail(&e),
        },
    };
    let rom = match rom_store::load_rom(&source, &ESP_ROM_ELFS, entry) {
        Ok(rom) => rom,
        Err(e) => return fail(&e),
    };
    let flash_note = match &flash_image {
        Some((path, bytes)) => format!(
            "flash: {} ({} bytes in {} MiB)",
            sanitize(&path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned()
            )),
            bytes.len(),
            profile.flash.size() >> 20
        ),
        None => format!("flash: erased ({} MiB)", profile.flash.size() >> 20),
    };
    let mut machine = match flash_image {
        Some((_, bytes)) => match profile.instantiate_with_flash(rom, bytes, machine_options) {
            Ok(machine) => machine,
            Err(e) => return fail(&e),
        },
        None => profile.instantiate(rom, machine_options),
    };
    write_line(
        io::stderr(),
        &format_args!("{}; {flash_note}", header(&profile, entry, &machine)),
    );
    let interrupt = Arc::new(AtomicBool::new(false));
    if let Err(e) = watch_ctrl_c(&interrupt) {
        write_line(
            io::stderr(),
            &format_args!(
                "warning: cannot watch for Ctrl-C: {e}; the run is not affected, but interrupting it will not print its summary: use --max-cycles or --max-time to bound it"
            ),
        );
    }
    let end = execute(
        &mut machine,
        run_options,
        &interrupt,
        &stdout_reader_gone,
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    );
    ExitCode::from(end.exit_code())
}

/// Whether whoever was reading standard output went away (`inkemu run | head`, once `head` has
/// what it wanted). A run that prints learns it from the write that fails; this is for the run
/// whose firmware prints nothing, which would otherwise go on for nobody.
#[cfg(unix)]
fn stdout_reader_gone() -> bool {
    use std::os::fd::AsFd as _;
    emu_cli::reader::is_gone(io::stdout().as_fd())
}

/// On the systems where the question cannot be asked without writing, the answer is always no: a
/// run that prints nothing and whose reader left goes on until `--max-cycles`, `--max-time` or
/// Ctrl-C ends it.
#[cfg(not(unix))]
fn stdout_reader_gone() -> bool {
    false
}

/// Makes Ctrl-C end the run at the next slice, with its summary. A second Ctrl-C ends the process
/// at once, for a run that does not come back to the slice boundary.
///
/// The handler runs on a thread of its own, not in the signal context; it only sets the flag.
fn watch_ctrl_c(interrupt: &Arc<AtomicBool>) -> Result<(), ctrlc::Error> {
    let requested = Arc::clone(interrupt);
    ctrlc::set_handler(move || {
        if second_request(&requested) {
            std::process::exit(i32::from(EXIT_SECOND_INTERRUPT));
        }
    })
}

/// Records in `requested` that the user asked the run to stop, and says whether that had already
/// been asked: the first request is left to the run, which ends with its summary; a second one
/// means that the run has not come back, and the process is ended at once.
fn second_request(requested: &AtomicBool) -> bool {
    requested.swap(true, Ordering::Relaxed)
}

/// The one line that says what is being emulated and how much to trust its timing.
fn header(profile: &DeviceProfile, entry: &RomEntry, machine: &soc_esp32c3::Esp32C3) -> String {
    let timing = machine.timing_model();
    format!(
        "inkemu: {} (ESP32-C3 {}, mask ROM rev{}); timing model {} ({})",
        profile.name,
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

/// The ROM entries a `rom fetch` invocation asks for. clap already refuses the two invalid
/// combinations; refusing again keeps `--all`'s download from ever being the default.
fn select_roms(device: Option<&str>, all: bool) -> Result<Vec<&'static RomEntry>, String> {
    match (device, all) {
        (Some(id), false) => profile_for(id)
            .map(|profile| vec![rom_store::rom_entry_for(profile.chip_revision)])
            .map_err(|e| e.to_string()),
        (None, true) => Ok(ESP32C3_ROMS.iter().collect()),
        (Some(_), true) => Err(
            "--device and --all were both given; pass --device <id> to fetch one ROM, or --all to fetch every ROM, not both"
                .to_owned(),
        ),
        (None, false) => Err(
            "neither --device nor --all was given; pass --device <id> to fetch one ROM or --all to fetch every ROM"
                .to_owned(),
        ),
    }
}

fn run_rom_fetch(device: Option<&str>, all: bool, rom_dir: Option<PathBuf>) -> ExitCode {
    let entries = match select_roms(device, all) {
        Ok(entries) => entries,
        Err(message) => return fail(&message),
    };
    let http = HttpFetch;
    let mut stderr = io::stderr();
    let user_named = rom_dir.is_some();
    let dir = match rom_dir.map_or_else(rom_store::default_cache_dir, Ok) {
        Ok(dir) => dir,
        Err(e) => return fail(&e),
    };
    let announcing = AnnouncingFetch::new(&http, &mut stderr, &ESP_ROM_ELFS, &dir);
    let source = RomSource::Cache {
        dir,
        fetch: &announcing,
        user_named,
    };
    if let Err(e) = rom_store::ensure_elfs(&source, &ESP_ROM_ELFS, &entries) {
        return fail(&e);
    }
    for entry in entries {
        // Every file is present now; loading it also checks the hash of the image built from it.
        if let Err(e) = rom_store::load_rom(&source, &ESP_ROM_ELFS, entry) {
            return fail(&e);
        }
        let line = format!(
            "{}\n",
            sanitize(&source.dir().join(entry.elf_name).display().to_string())
        );
        if let ControlFlow::Break(code) = print_stdout(&line) {
            return code;
        }
    }
    ExitCode::SUCCESS
}

fn run_inspect(path: &Path) -> ExitCode {
    let shown = sanitize(&path.display().to_string());
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            return fail(&format_args!(
                "cannot read {shown}: {e}; check the path and read permissions"
            ));
        }
    };
    match firmware::FirmwareImage::parse(&bytes) {
        Ok(img) => {
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            match print_stdout(&emu_cli::inspect::render(&img, &name, bytes.len())) {
                ControlFlow::Continue(()) => ExitCode::SUCCESS,
                ControlFlow::Break(code) => code,
            }
        }
        Err(e) => fail(&format_args!("{shown}: {e}")),
    }
}

fn run_rvtest(
    elf: &Path,
    max_steps: u64,
    signature: Option<&Path>,
    trace_hash: bool,
    allow_misaligned: bool,
) -> ExitCode {
    let config = HartConfig {
        misaligned: if allow_misaligned {
            MisalignedPolicy::Allow
        } else {
            MisalignedPolicy::Trap
        },
        ..HartConfig::default()
    };
    let machine = std::fs::read(elf)
        .map_err(|e| {
            format!(
                "cannot read {}: {e}; check the path and read permissions",
                elf.display()
            )
        })
        .and_then(|bytes| TestMachine::from_elf(&bytes, config).map_err(|e| e.to_string()));
    let mut machine = match machine {
        Ok(m) => m,
        Err(e) => return fail(&e),
    };
    let result = machine.run(max_steps);
    // What goes to standard output here is for a person: the verdict of the test is the exit
    // code, which must not depend on who reads the lines.
    if trace_hash {
        write_line(
            io::stdout(),
            &format_args!("trace-hash {:016x}", machine.trace_hash()),
        );
    }
    if let Some(path) = signature {
        let words = match machine.signature() {
            Ok(w) => w,
            Err(e) => return fail(&e),
        };
        let text = words.iter().fold(String::new(), |mut acc, w| {
            // Writing to a String cannot fail.
            let _ = writeln!(acc, "{w:08x}");
            acc
        });
        if let Err(e) = std::fs::write(path, text) {
            return fail(&format_args!(
                "cannot write {}: {e}; check that the directory exists and is writable",
                path.display()
            ));
        }
    }
    match result {
        RunResult::Passed => {
            write_line(io::stdout(), &"Passed");
            ExitCode::SUCCESS
        }
        RunResult::Failed { test_number } => {
            write_line(io::stdout(), &format_args!("FAILED at test #{test_number}"));
            ExitCode::from(EXIT_TEST_FAILURE)
        }
        RunResult::Timeout => fail(&format_args!(
            "no verdict after {max_steps} steps; raise --max-steps or check that the test writes to tohost"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selecting_no_rom_states_the_cause_then_the_remedy() {
        let Err(message) = select_roms(None, false) else {
            panic!("no selection must be refused")
        };
        assert_eq!(
            message,
            "neither --device nor --all was given; pass --device <id> to fetch one ROM or --all to fetch every ROM"
        );
    }

    #[test]
    fn selecting_a_device_and_all_states_the_cause_then_the_remedy() {
        let Err(message) = select_roms(Some("x4"), true) else {
            panic!("a double selection must be refused")
        };
        let (cause, remedy) = message.split_once("; ").expect("cause; remedy");
        assert!(cause.contains("--device and --all"), "{message}");
        assert!(
            remedy.contains("--device <id>") && remedy.contains("--all"),
            "{message}"
        );
    }

    #[test]
    fn the_first_request_to_stop_is_left_to_the_run_and_the_second_ends_the_process() {
        let requested = AtomicBool::new(false);
        assert!(!second_request(&requested));
        assert!(
            requested.load(Ordering::Relaxed),
            "the run reads the request at its next pass"
        );
        assert!(second_request(&requested));
        assert!(second_request(&requested), "and so does every one after it");
    }

    #[test]
    fn every_console_of_the_device_can_be_asked_for() {
        use clap::ValueEnum as _;
        for channel in ConsoleChannel::ALL {
            assert!(
                ConsoleArg::value_variants()
                    .iter()
                    .any(|arg| ConsoleChannel::from(*arg) == channel),
                "no value of --console selects {channel:?}"
            );
        }
    }

    #[test]
    fn selecting_all_yields_the_three_roms_and_a_device_yields_one() {
        assert_eq!(select_roms(None, true).map(|roms| roms.len()), Ok(3));
        assert_eq!(select_roms(Some("x4"), false).map(|roms| roms.len()), Ok(1));
    }
}
