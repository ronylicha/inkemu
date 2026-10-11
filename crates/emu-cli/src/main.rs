//! inkemu command-line interface.
use boards::{DeviceProfile, MachineOptions};
use clap::{Parser, Subcommand};
use cpu_rv32::{HartConfig, MisalignedPolicy};
use emu_cli::exit::{EXIT_ERROR, EXIT_TEST_FAILURE, RUN_EXIT_CODES_HELP};
use emu_cli::rom_store::{self, ESP_ROM_ELFS, ESP32C3_ROMS, HttpFetch, RomEntry, RomSource};
use emu_cli::run::{RunOptions, execute, relay};
use emu_cli::rvtest::{RunResult, TestMachine};
use emu_cli::term::sanitize;
use soc_esp32c3::ConsoleChannel;
use std::fmt::Write as _;
use std::io;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Size granularity of the download notice: 0.1 MB (decimal megabytes).
const TENTH_OF_A_MEGABYTE: u64 = 100_000;

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
    /// Describe a firmware image and check that inkemu can load it.
    Inspect { file: PathBuf },
    /// Boot a device from its mask ROM and print its console.
    #[command(after_help = RUN_EXIT_CODES_HELP)]
    Run {
        /// Device profile id; an unknown id prints the available ones.
        #[arg(long)]
        device: String,
        /// Stop when the CPU has executed this many clock cycles.
        #[arg(long)]
        max_cycles: Option<u64>,
        /// Stop at the first access to a register the emulator does not model, or the first 8/16-bit
        /// write to a peripheral register (exit code 3).
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
    UsbSerialJtag,
}

impl From<ConsoleArg> for ConsoleChannel {
    fn from(arg: ConsoleArg) -> ConsoleChannel {
        match arg {
            ConsoleArg::Uart0 => ConsoleChannel::Uart0,
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
            max_cycles,
            strict_mmio,
            console,
            rom_dir,
            trace_hash,
        } => run_device(
            &device,
            rom_dir,
            MachineOptions {
                strict_mmio,
                trace_hash,
            },
            &RunOptions {
                max_cycles,
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

/// Prints `error: {message}` and yields the input-error exit code.
fn fail(message: &dyn std::fmt::Display) -> ExitCode {
    eprintln!("error: {message}");
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
        other @ boards::ProfileError::Parse { .. } => other,
    })
}

/// Says on standard error that the archive is about to be downloaded, when the cache lacks at least
/// one of `entries`. It is said once however many files are missing: the archive is downloaded once.
fn announce_download(source: &RomSource<'_>, entries: &[&RomEntry]) {
    let RomSource::Cache { dir, .. } = source else {
        return;
    };
    if entries
        .iter()
        .all(|entry| dir.join(entry.elf_name).exists())
    {
        return;
    }
    // Tenths of a megabyte, in integers: the size is only shown to one decimal.
    let tenths = ESP_ROM_ELFS.size / TENTH_OF_A_MEGABYTE;
    eprintln!(
        "inkemu: downloading the ESP32-C3 mask ROM (esp-rom-elfs {}, {}.{} MB) into {}",
        ESP_ROM_ELFS.release,
        tenths / 10,
        tenths % 10,
        sanitize(&dir.display().to_string())
    );
}

fn run_device(
    device: &str,
    rom_dir: Option<PathBuf>,
    machine_options: MachineOptions,
    run_options: &RunOptions,
) -> ExitCode {
    let profile = match profile_for(device) {
        Ok(p) => p,
        Err(e) => return fail(&e),
    };
    let entry = rom_store::rom_entry_for(profile.chip_revision);
    let fetch = HttpFetch;
    // An explicit directory is read only; without one the cache downloads on first use.
    let source = match rom_dir {
        Some(dir) => Ok(RomSource::Directory(dir)),
        None => rom_store::default_cache_dir().map(|dir| RomSource::Cache { dir, fetch: &fetch }),
    };
    let rom = source.and_then(|source| {
        announce_download(&source, &[entry]);
        rom_store::load_rom(&source, &ESP_ROM_ELFS, entry)
    });
    let rom = match rom {
        Ok(rom) => rom,
        Err(e) => return fail(&e),
    };
    let mut machine = profile.instantiate(rom, machine_options);
    eprintln!("{}", header(&profile, entry, &machine));
    let end = execute(
        &mut machine,
        run_options,
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    );
    ExitCode::from(end.exit_code())
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

fn run_rom_fetch(device: Option<&str>, all: bool, rom_dir: Option<PathBuf>) -> ExitCode {
    let entries: Vec<&RomEntry> = match (device, all) {
        (Some(id), false) => match profile_for(id) {
            Ok(profile) => vec![rom_store::rom_entry_for(profile.chip_revision)],
            Err(e) => return fail(&e),
        },
        (None, true) => ESP32C3_ROMS.iter().collect(),
        // clap already refuses these; refusing again keeps `--all`'s download from being the default.
        (Some(_), true) | (None, false) => return fail(&"pass either --device <id> or --all"),
    };
    let fetch = HttpFetch;
    let dir = match rom_dir.map_or_else(rom_store::default_cache_dir, Ok) {
        Ok(dir) => dir,
        Err(e) => return fail(&e),
    };
    let source = RomSource::Cache { dir, fetch: &fetch };
    announce_download(&source, &entries);
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
            eprintln!("error: cannot read {shown}: {e}; check the path and read permissions");
            return ExitCode::from(EXIT_ERROR);
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
        Err(e) => {
            eprintln!("error: {shown}: {e}");
            ExitCode::from(EXIT_ERROR)
        }
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
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(EXIT_ERROR);
        }
    };
    let result = machine.run(max_steps);
    if trace_hash {
        println!("trace-hash {:016x}", machine.trace_hash());
    }
    if let Some(path) = signature {
        let words = match machine.signature() {
            Ok(w) => w,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::from(EXIT_ERROR);
            }
        };
        let text = words.iter().fold(String::new(), |mut acc, w| {
            // Writing to a String cannot fail.
            let _ = writeln!(acc, "{w:08x}");
            acc
        });
        if let Err(e) = std::fs::write(path, text) {
            eprintln!(
                "error: cannot write {}: {e}; check that the directory exists and is writable",
                path.display()
            );
            return ExitCode::from(EXIT_ERROR);
        }
    }
    match result {
        RunResult::Passed => {
            println!("Passed");
            ExitCode::SUCCESS
        }
        RunResult::Failed { test_number } => {
            println!("FAILED at test #{test_number}");
            ExitCode::from(EXIT_TEST_FAILURE)
        }
        RunResult::Timeout => {
            eprintln!(
                "error: no verdict after {max_steps} steps; raise --max-steps or check that the test writes to tohost"
            );
            ExitCode::from(EXIT_ERROR)
        }
    }
}
