//! `inkemu-lockstep`: runs a flash image in Espressif's QEMU, inside the pinned Docker image, and
//! in inkemu, and compares the two after every instruction (spec §4.4, ADR 0008). A development
//! tool: it is not part of the distributed `inkemu`.

#[cfg(unix)]
mod qemu;

use clap::{Parser, ValueEnum};
use lockstep::{End, Lockstep, Target, TraceEvent, TraceReader, WindowEnd};
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
/// The register `--perturb-after` changes in the machine: `t0`, which no row admits a
/// difference of, so the next comparison reports it.
const PERTURBED_REGISTER: u8 = 5;

/// Exit codes.
const EXIT_DIVERGENCE: u8 = 1;
const EXIT_ERROR: u8 = 2;
const EXIT_EXPECTATION: u8 = 3;
const EXIT_STOPPED: u8 = 4;
const EXIT_TABLE: u8 = 5;
const EXIT_INTERRUPTED: u8 = 130;

const EXIT_CODES_HELP: &str = "Exit codes: 0 the window ended without a divergence outside \
docs/lockstep-differences.md, every row of it used within what it declares (and as --expect-* \
said); 1 divergence, a reset of one side included (the report is printed); 2 error (arguments, \
files, Docker, QEMU ending on its own, a trace that is not QEMU's or holds no block); 3 the \
window ended otherwise than --expect-compared or --expect-end said; 4 inkemu stopped on its own \
before QEMU's trace ended; 5 a row of the table was never used, or was used outside what it \
declares; 130 interrupted by Ctrl-C.";

/// How the window is expected to end, for `--expect-end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum EndKind {
    /// A reset of both sides after the same instruction.
    Reset,
    /// An access to a register QEMU does not have that no row names.
    Absent,
    Limit,
    TraceEnd,
}

impl EndKind {
    fn of(end: &WindowEnd) -> EndKind {
        match end {
            WindowEnd::Reset => EndKind::Reset,
            WindowEnd::Absent { .. } => EndKind::Absent,
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
    #[arg(long, default_value = DEFAULT_IMAGE)]
    image: String,
    /// QEMU inside the image.
    #[arg(long, default_value = DEFAULT_QEMU)]
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
    /// For the test of the tool only: once this many instructions are compared, change bit 0 of
    /// t0 in inkemu, as a fault of the model would; the run must then diverge (exit 1).
    #[arg(long, hide = true)]
    perturb_after: Option<u64>,
}

#[cfg(unix)]
const DEFAULT_IMAGE: &str = qemu::DEFAULT_IMAGE;
#[cfg(unix)]
const DEFAULT_QEMU: &str = qemu::DEFAULT_QEMU;
#[cfg(not(unix))]
const DEFAULT_IMAGE: &str = "espressif/idf:v5.5.2";
#[cfg(not(unix))]
const DEFAULT_QEMU: &str = "";

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
    #[cfg(unix)]
    #[error(transparent)]
    Qemu(#[from] qemu::QemuError),
    #[cfg(not(unix))]
    #[error(
        "running QEMU needs a Unix host (a named pipe out of Docker); give its trace with --trace"
    )]
    NotUnix,
    #[error(transparent)]
    Trace(#[from] lockstep::TraceError),
    #[error(
        "QEMU's trace held no block ({0} lines skipped): it is not a trace of `-d cpu,nochain,int`"
    )]
    EmptyTrace(u64),
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
    // QEMU takes a flash of exactly 2, 4, 8 or 16 MiB and refuses any other; inkemu pads a short
    // image. Both must see the same flash: the image is the whole flash of the profile.
    let size = config.flash.size();
    if u32::try_from(image.len()).ok() != Some(size) {
        return Err(Error::Flash {
            path: cli.flash.clone(),
            detail: format!(
                "it holds {} bytes where the profile's flash holds {size}",
                image.len()
            ),
        });
    }
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

/// What a run of [`compare`] is asked to do besides comparing.
struct Options<'a> {
    interrupted: &'a AtomicBool,
    perturb_after: Option<u64>,
}

fn compare(
    reader: &mut TraceReader<impl BufRead>,
    machine: &mut Esp32C3,
    lockstep: &mut Lockstep,
    options: &Options<'_>,
) -> Result<Outcome, Error> {
    let mut events: u64 = 0;
    let mut perturbed = false;
    loop {
        events += 1;
        if events.is_multiple_of(INTERRUPT_CHECK_EVENTS)
            && options.interrupted.load(Ordering::Relaxed)
        {
            return Ok(Outcome::Interrupted);
        }
        let event = match reader.next_event() {
            // A trace cut by Ctrl-C (the producer upstream of `--trace -` dies with it) is an
            // interruption, not a malformed trace.
            Err(_) | Ok(None) if options.interrupted.load(Ordering::Relaxed) => {
                return Ok(Outcome::Interrupted);
            }
            Err(e) => return Err(e.into()),
            Ok(None) => {
                if reader.blocks_read() == 0 {
                    return Err(Error::EmptyTrace(reader.skipped_lines()));
                }
                return Ok(Outcome::Ended(lockstep.finish(machine)));
            }
            Ok(Some(event)) => event,
        };
        if let Some(end) = lockstep.feed(event, machine) {
            return Ok(Outcome::Ended(end));
        }
        if let Some(after) = options.perturb_after
            && !perturbed
            && lockstep.counts().compared >= after
        {
            let t0 = machine.x()[usize::from(PERTURBED_REGISTER)];
            Target::set_x(machine, PERTURBED_REGISTER, t0 ^ 1);
            perturbed = true;
        }
        if matches!(event, TraceEvent::State { .. }) && events.is_multiple_of(PROGRESS_EVENTS) {
            eprintln!(
                "inkemu-lockstep: {} instructions compared",
                lockstep.counts().compared
            );
        }
    }
}

/// Reads QEMU's trace from where `cli` says and compares it.
fn comparison(
    cli: &Cli,
    machine: &mut Esp32C3,
    lockstep: &mut Lockstep,
    options: &Options<'_>,
) -> Result<(Outcome, u64), Error> {
    match &cli.trace {
        Some(path) if path.as_os_str() == "-" => {
            let mut reader = TraceReader::new(BufReader::with_capacity(
                READ_BUFFER,
                std::io::stdin().lock(),
            ));
            let outcome = compare(&mut reader, machine, lockstep, options)?;
            Ok((outcome, reader.skipped_lines()))
        }
        Some(path) => {
            let file = std::fs::File::open(path).map_err(|source| Error::Read {
                path: path.clone(),
                source,
            })?;
            let mut reader = TraceReader::new(BufReader::with_capacity(READ_BUFFER, file));
            let outcome = compare(&mut reader, machine, lockstep, options)?;
            Ok((outcome, reader.skipped_lines()))
        }
        None => run_qemu(cli, machine, lockstep, options),
    }
}

#[cfg(unix)]
fn run_qemu(
    cli: &Cli,
    machine: &mut Esp32C3,
    lockstep: &mut Lockstep,
    options: &Options<'_>,
) -> Result<(Outcome, u64), Error> {
    let flash = std::fs::canonicalize(&cli.flash).map_err(|source| Error::Read {
        path: cli.flash.clone(),
        source,
    })?;
    eprintln!(
        "inkemu-lockstep: starting QEMU in {} on {}",
        cli.image,
        flash.display()
    );
    let (mut qemu, pipe) =
        match qemu::Qemu::start(&flash, &cli.image, &cli.qemu, options.interrupted)? {
            qemu::Started::Running(qemu, pipe) => (qemu, pipe),
            qemu::Started::Interrupted => return Ok((Outcome::Interrupted, 0)),
        };
    let mut reader = TraceReader::new(BufReader::with_capacity(READ_BUFFER, pipe));
    let outcome = compare(&mut reader, machine, lockstep, options)?;
    // The trace ends only when QEMU exits, which a running firmware never asks for: an end of
    // the trace is QEMU failing, and the run with it.
    if matches!(outcome, Outcome::Ended(End::Window(WindowEnd::TraceEnd))) {
        qemu.ended()?;
    }
    // Dropping `qemu` removes the container and the pipe.
    Ok((outcome, reader.skipped_lines()))
}

#[cfg(not(unix))]
fn run_qemu(
    _cli: &Cli,
    _machine: &mut Esp32C3,
    _lockstep: &mut Lockstep,
    _options: &Options<'_>,
) -> Result<(Outcome, u64), Error> {
    Err(Error::NotUnix)
}

fn run(cli: &Cli, interrupted: &AtomicBool) -> Result<u8, Error> {
    let mut machine = machine(cli)?;
    let mut lockstep = Lockstep::new(&machine, cli.max_instructions);
    let options = Options {
        interrupted,
        perturb_after: cli.perturb_after,
    };
    let (outcome, skipped) = comparison(cli, &mut machine, &mut lockstep, &options)?;
    let counts = lockstep.counts();
    let usage = lockstep.usage(&machine);
    let summary = format!(
        "instructions compared: {}\nduplicate blocks dropped: {}\nblocks preempted by an interrupt: {}\ninterrupts taken: {}\ntrace lines skipped: {skipped}\nuse of the rows of docs/lockstep-differences.md:\n{usage}",
        counts.compared, counts.duplicates, counts.preempted, counts.interrupts,
    );
    let end = match outcome {
        Outcome::Ended(end) => end,
        Outcome::Interrupted => {
            println!("{summary}interrupted by Ctrl-C");
            return Ok(EXIT_INTERRUPTED);
        }
    };
    match end {
        End::Window(end) => {
            println!("{summary}window end: {end}");
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
            // Decision H3: a row the window did not need, or used outside its declaration, is a
            // table that no longer says what the comparison does.
            if let Err(misuses) = usage.check(lockstep::DIFFERENCES) {
                for misuse in misuses {
                    eprintln!("inkemu-lockstep: {misuse}");
                }
                if code == 0 {
                    code = EXIT_TABLE;
                }
            }
            Ok(code)
        }
        End::Divergence(report) => {
            println!("{report}\n{summary}");
            Ok(EXIT_DIVERGENCE)
        }
        End::Stopped { reason, pc } => {
            println!("{summary}inkemu stopped at {pc:#010x}: {reason:?}");
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
        Err(_) if interrupted.load(Ordering::Relaxed) => ExitCode::from(EXIT_INTERRUPTED),
        Err(e) => {
            eprintln!("inkemu-lockstep: {e}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}
