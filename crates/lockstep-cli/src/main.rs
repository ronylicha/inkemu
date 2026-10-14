//! `inkemu-lockstep`: runs a flash image in Espressif's QEMU, inside the pinned Docker image, and
//! in inkemu, and compares the two after every instruction (spec §4.4, ADR 0008). A development
//! tool: it is not part of the distributed `inkemu`.

mod qemu;

use clap::{Parser, ValueEnum};
use lockstep::{End, Lockstep, TraceEvent, TraceReader, WindowEnd};
use soc_esp32c3::{Esp32C3, RomImage};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The size of the buffer the trace is read through: a block is about 1.1 KB, and the pipe is
/// the bottleneck only when it is read in small pieces.
const READ_BUFFER: usize = 1 << 20;
/// How many events go by between two looks at Ctrl-C.
const INTERRUPT_CHECK_EVENTS: u64 = 4096;
/// How many events go by between two progress lines on standard error: about 13 s of QEMU.
const PROGRESS_EVENTS: u64 = 1 << 20;
/// The name of the ROM inside `--rom-dir`, as `inkemu rom fetch` stores it: QEMU's ROM is the
/// code of the rev3 ROM (oracle §1a).
const ROM_FILE: &str = "esp32c3_rev3_rom.elf";

/// Exit codes.
const EXIT_DIVERGENCE: u8 = 1;
const EXIT_ERROR: u8 = 2;
const EXIT_EXPECTATION: u8 = 3;
const EXIT_STOPPED: u8 = 4;
const EXIT_INTERRUPTED: u8 = 130;

const EXIT_CODES_HELP: &str = "Exit codes: 0 the window ended without a divergence outside \
docs/lockstep-differences.md (and as --expect-* said); 1 divergence (the report is printed); 2 \
error (arguments, files, Docker, a trace that is not QEMU's); 3 the window ended otherwise than \
--expect-compared or --expect-end said; 4 inkemu stopped on its own before QEMU's trace ended; 130 \
interrupted by Ctrl-C.";

/// How the window is expected to end, for `--expect-end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum EndKind {
    /// A reset of both sides after the same instruction.
    Reset,
    /// A reset of one side only.
    OneSidedReset,
    Unemulated,
    Limit,
    TraceEnd,
}

impl EndKind {
    fn of(end: &WindowEnd) -> EndKind {
        match end {
            WindowEnd::Reset {
                qemu: true,
                inkemu: true,
            } => EndKind::Reset,
            WindowEnd::Reset { .. } => EndKind::OneSidedReset,
            WindowEnd::Unemulated { .. } => EndKind::Unemulated,
            WindowEnd::Limit => EndKind::Limit,
            WindowEnd::TraceEnd => EndKind::TraceEnd,
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "inkemu-lockstep",
    about = "Compare inkemu with Espressif's QEMU after every instruction of a flash image",
    after_help = EXIT_CODES_HELP
)]
struct Cli {
    /// The flash image, a whole flash of the profile's size (16 MiB for the X4).
    #[arg(long)]
    flash: PathBuf,
    /// The directory of the mask ROMs (`inkemu rom fetch --rom-dir <dir>`).
    #[arg(long)]
    rom_dir: PathBuf,
    /// The device profile whose flash and chip revision inkemu takes.
    #[arg(long, default_value = "x4")]
    device: String,
    /// Read QEMU's trace from this file instead of running QEMU (`-` for standard input).
    #[arg(long)]
    trace: Option<PathBuf>,
    /// The Docker image QEMU runs in.
    #[arg(long, default_value = qemu::DEFAULT_IMAGE)]
    image: String,
    /// QEMU inside the image.
    #[arg(long, default_value = qemu::DEFAULT_QEMU)]
    qemu: String,
    /// End the window after this many compared instructions.
    #[arg(long)]
    max_instructions: Option<u64>,
    /// Fail (exit 3) unless exactly this many instructions are compared.
    #[arg(long)]
    expect_compared: Option<u64>,
    /// Fail (exit 3) unless the window ends this way.
    #[arg(long, value_enum)]
    expect_end: Option<EndKind>,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{0}")]
    Rom(String),
    #[error("{0}")]
    Profile(#[from] boards::ProfileError),
    #[error("the flash image {path} does not fit the profile's flash: {detail}")]
    Flash { path: PathBuf, detail: String },
    #[error(transparent)]
    Qemu(#[from] qemu::QemuError),
    #[error(transparent)]
    Trace(#[from] lockstep::TraceError),
}

fn machine(cli: &Cli) -> Result<Esp32C3, Error> {
    let rom_path = cli.rom_dir.join(ROM_FILE);
    let rom = std::fs::read(&rom_path).map_err(|source| Error::Read {
        path: rom_path.clone(),
        source,
    })?;
    let rom = RomImage::from_elf(&rom).map_err(|e| {
        Error::Rom(format!(
            "{} is not a usable ESP32-C3 ROM ({e}); fetch it again with `inkemu rom fetch --all --rom-dir {}`",
            rom_path.display(),
            cli.rom_dir.display()
        ))
    })?;
    let image = std::fs::read(&cli.flash).map_err(|source| Error::Read {
        path: cli.flash.clone(),
        source,
    })?;
    let profile = boards::builtin(&cli.device)?;
    let config = lockstep::table::configure(
        profile.soc_config(boards::MachineOptions::default()),
        lockstep::DIFFERENCES,
    );
    Esp32C3::with_flash(rom, image, &config).map_err(|e| Error::Flash {
        path: cli.flash.clone(),
        detail: e.to_string(),
    })
}

/// How the comparison ended.
enum Outcome {
    Ended(End),
    /// Ctrl-C: the comparison stopped where it was.
    Interrupted,
}

fn compare(
    reader: &mut TraceReader<impl BufRead>,
    machine: &mut Esp32C3,
    lockstep: &mut Lockstep,
    interrupted: &AtomicBool,
) -> Result<Outcome, Error> {
    let mut events: u64 = 0;
    loop {
        events += 1;
        if events.is_multiple_of(INTERRUPT_CHECK_EVENTS) && interrupted.load(Ordering::Relaxed) {
            return Ok(Outcome::Interrupted);
        }
        let Some(event) = reader.next_event()? else {
            return Ok(Outcome::Ended(lockstep.finish(machine)));
        };
        if let Some(end) = lockstep.feed(event, machine) {
            return Ok(Outcome::Ended(end));
        }
        if matches!(event, TraceEvent::State { .. }) && events.is_multiple_of(PROGRESS_EVENTS) {
            eprintln!(
                "inkemu-lockstep: {} instructions compared",
                lockstep.counts().compared
            );
        }
    }
}

fn run(cli: &Cli, interrupted: &AtomicBool) -> Result<u8, Error> {
    let mut machine = machine(cli)?;
    let mut lockstep = Lockstep::new(&machine, cli.max_instructions);
    let outcome = match &cli.trace {
        Some(path) if path.as_os_str() == "-" => {
            let mut reader = TraceReader::new(BufReader::with_capacity(
                READ_BUFFER,
                std::io::stdin().lock(),
            ));
            compare(&mut reader, &mut machine, &mut lockstep, interrupted)?
        }
        Some(path) => {
            let file = std::fs::File::open(path).map_err(|source| Error::Read {
                path: path.clone(),
                source,
            })?;
            let mut reader = TraceReader::new(BufReader::with_capacity(READ_BUFFER, file));
            compare(&mut reader, &mut machine, &mut lockstep, interrupted)?
        }
        None => {
            let flash = std::fs::canonicalize(&cli.flash).map_err(|source| Error::Read {
                path: cli.flash.clone(),
                source,
            })?;
            eprintln!(
                "inkemu-lockstep: starting QEMU in {} on {}",
                cli.image,
                flash.display()
            );
            let (qemu, pipe) = qemu::Qemu::start(&flash, &cli.image, &cli.qemu)?;
            let mut reader = TraceReader::new(BufReader::with_capacity(READ_BUFFER, pipe));
            let outcome = compare(&mut reader, &mut machine, &mut lockstep, interrupted);
            drop(reader);
            qemu.stop();
            outcome?
        }
    };
    let counts = lockstep.counts();
    let rows: Vec<String> = counts
        .rows
        .iter()
        .map(|(row, uses)| format!("{row} ({uses})"))
        .collect();
    let summary = format!(
        "instructions compared: {}\nduplicate blocks dropped: {}\nblocks preempted by an interrupt: {}\ninterrupts taken: {}\nrows used: {}",
        counts.compared,
        counts.duplicates,
        counts.preempted,
        counts.interrupts,
        if rows.is_empty() {
            "none".to_owned()
        } else {
            rows.join(", ")
        }
    );
    let end = match outcome {
        Outcome::Ended(end) => end,
        Outcome::Interrupted => {
            println!("{summary}\ninterrupted by Ctrl-C");
            return Ok(EXIT_INTERRUPTED);
        }
    };
    match end {
        End::Window(end) => {
            println!("{summary}\nwindow end: {end}");
            let mut code = 0;
            if let Some(expected) = cli.expect_compared
                && expected != counts.compared
            {
                eprintln!(
                    "inkemu-lockstep: {} instructions were compared where {expected} were expected",
                    counts.compared
                );
                code = EXIT_EXPECTATION;
            }
            if let Some(expected) = cli.expect_end
                && expected != EndKind::of(&end)
            {
                eprintln!(
                    "inkemu-lockstep: the window ended by {end} where {expected:?} was expected"
                );
                code = EXIT_EXPECTATION;
            }
            Ok(code)
        }
        End::Divergence(report) => {
            println!("{report}\n{summary}");
            Ok(EXIT_DIVERGENCE)
        }
        End::Stopped { reason, pc } => {
            println!("{summary}\ninkemu stopped at {pc:#010x}: {reason:?}");
            Ok(EXIT_STOPPED)
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let interrupted = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&interrupted);
    if let Err(e) = ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed)) {
        eprintln!("inkemu-lockstep: cannot catch Ctrl-C: {e}");
        return ExitCode::from(EXIT_ERROR);
    }
    match run(&cli, &interrupted) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("inkemu-lockstep: {e}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}
