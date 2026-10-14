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
docs/lockstep-differences.md, every row of it used at every site and bit it declares and \
within them (and as --expect-* said); 1 divergence, a reset of one side included (the report \
is printed); 2 error (arguments, files, Docker, QEMU ending on its own, a trace that is not \
QEMU's, holds no block or ends before the window does); 3 the window ended otherwise than \
--expect-compared, --expect-end or --expect-usage said; 4 inkemu stopped on its own before \
QEMU's trace ended; 5 a row, a site or a bit of the table was never used (unless \
--max-instructions cut the window), or a row was used outside what it declares; 130 \
interrupted by Ctrl-C.";

/// How the window is expected to end, for `--expect-end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum EndKind {
    /// A reset of both sides after the same instruction.
    Reset,
    /// An access to a register QEMU does not have that no row names.
    Absent,
    /// `--max-instructions`.
    Limit,
    /// The window's own bound.
    Bound,
}

impl EndKind {
    /// The kind of a window end the run reports; `None` for the end of the trace, an error.
    fn of(end: &WindowEnd) -> Option<EndKind> {
        match end {
            WindowEnd::Reset => Some(EndKind::Reset),
            WindowEnd::Absent { .. } => Some(EndKind::Absent),
            WindowEnd::Limit => Some(EndKind::Limit),
            WindowEnd::Bound => Some(EndKind::Bound),
            WindowEnd::TraceEnd => None,
        }
    }
}

/// The window `--window` names.
fn window_name(name: &str) -> Result<&'static lockstep::Window, String> {
    lockstep::window(name).ok_or_else(|| {
        let names: Vec<&str> = lockstep::WINDOWS.iter().map(|window| window.name).collect();
        format!("no window {name}; the windows are {}", names.join(", "))
    })
}

#[derive(Debug, Parser)]
#[command(
    name = "inkemu-lockstep",
    about = "Compare inkemu with Espressif's QEMU after every instruction of a flash image",
    after_help = EXIT_CODES_HELP
)]
struct Cli {
    /// The flash image, a whole flash of the profile's size (16 MiB for the X4).
    #[arg(
        long,
        required_unless_present = "firmware",
        conflicts_with = "firmware"
    )]
    flash: Option<PathBuf>,
    /// An application image instead: the flash both sides boot is composed as `inkemu run
    /// --firmware` composes it, behind the bootloader inkemu ships for the device.
    #[arg(long)]
    firmware: Option<PathBuf>,
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
    /// End the window after this many compared instructions. The rows, sites and bits of the
    /// table such a partial window leaves unused are listed, not failed on.
    #[arg(long)]
    max_instructions: Option<u64>,
    /// The window: which rows of docs/lockstep-differences.md hold, how many resets of both
    /// sides the comparison goes on through, and its bound (`hello`, the first one, compares
    /// `flash_hello.bin`).
    #[arg(long, value_parser = window_name)]
    window: &'static lockstep::Window,
    /// Fail (exit 3) unless exactly this many instructions are compared.
    #[arg(long)]
    expect_compared: Option<u64>,
    /// Fail (exit 3) unless the window ends this way.
    #[arg(long, value_enum)]
    expect_end: Option<EndKind>,
    /// Fail (exit 3) unless every row was used exactly as often as this file says, one line
    /// `row N: uses` per row (a row it does not name, never).
    #[arg(long)]
    expect_usage: Option<PathBuf>,
    /// For the test of the tool only: once this many instructions are compared, change bit 0 of
    /// t0 in inkemu, as a fault of the CPU's model would; the run must then diverge (exit 1).
    #[arg(long, hide = true)]
    perturb_after: Option<u64>,
    /// For the test of the tool only: once this many instructions are compared, the next read of
    /// the low word of a SYSTIMER counter gives one count more in inkemu, as a fault of the
    /// SYSTIMER's model would; row 15 holds the next read of that counter to the gap the
    /// faulty one left, and the run must then diverge (exit 1).
    #[arg(long, hide = true)]
    perturb_systimer_after: Option<u64>,
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
    #[error("no flash can be composed from {path}: {detail}")]
    Compose { path: PathBuf, detail: String },
    #[error("give the flash with --flash or an application with --firmware")]
    NoFlash,
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
    #[error(
        "QEMU's trace ended after {0} instructions compared, before the window did: a capture \
         stopped early (QEMU crashed or was cut) ends no window"
    )]
    TraceEnded(u64),
    #[error("{path}: `{line}` is not a line `row N: uses`")]
    Usage { path: PathBuf, line: String },
}

fn machine(cli: &Cli) -> Result<(Esp32C3, Flash), Error> {
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
    let profile = boards::builtin(&cli.device)?;
    let config = lockstep::table::configure(
        profile.soc_config(boards::MachineOptions::default()),
        cli.window.rows,
    );
    let flash = flash(cli, &profile)?;
    // QEMU takes a flash of exactly 2, 4, 8 or 16 MiB and refuses any other; inkemu pads a short
    // image. Both must see the same flash: the image is the whole flash of the profile.
    let size = config.flash.size();
    if u32::try_from(flash.bytes.len()).ok() != Some(size) {
        return Err(Error::Flash {
            path: flash.path.clone(),
            detail: format!(
                "it holds {} bytes where the profile's flash holds {size}",
                flash.bytes.len()
            ),
        });
    }
    let machine =
        Esp32C3::with_flash(rom, flash.bytes.clone(), &config).map_err(|e| Error::Flash {
            path: flash.path.clone(),
            detail: e.to_string(),
        })?;
    Ok((machine, flash))
}

/// The flash both sides boot: the image of `--flash`, or the one composed from `--firmware`.
#[derive(Debug)]
struct Flash {
    /// The file it was read or composed from.
    path: PathBuf,
    bytes: Vec<u8>,
    /// Whether the tool composed it, so that QEMU is given the composed bytes, not the file.
    composed: bool,
}

/// Reads `--flash`, or composes the flash of `--firmware` as `inkemu run --firmware` does.
fn flash(cli: &Cli, profile: &boards::DeviceProfile) -> Result<Flash, Error> {
    let (path, composed) = match (&cli.flash, &cli.firmware) {
        (Some(path), _) => (path, false),
        (None, Some(path)) => (path, true),
        // clap requires one of the two.
        (None, None) => return Err(Error::NoFlash),
    };
    let bytes = std::fs::read(path).map_err(|source| Error::Read {
        path: path.clone(),
        source,
    })?;
    let bytes = if composed {
        profile
            .compose_flash(&bytes, None)
            .map_err(|e| Error::Compose {
                path: path.clone(),
                detail: e.to_string(),
            })?
            .bytes
    } else {
        bytes
    };
    Ok(Flash {
        path: path.clone(),
        bytes,
        composed,
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
    perturb_systimer_after: Option<u64>,
}

/// The low words of the two SYSTIMER counters, which `--perturb-systimer-after` changes.
const SYSTIMER_VALUE_LO: [u32; 2] = [
    lockstep::table::SYSTIMER_UNIT0_VALUE_LO,
    lockstep::table::SYSTIMER_UNIT1_VALUE_LO,
];

fn compare(
    reader: &mut TraceReader<impl BufRead>,
    machine: &mut Esp32C3,
    lockstep: &mut Lockstep,
    options: &Options<'_>,
) -> Result<Outcome, Error> {
    let mut events: u64 = 0;
    let mut perturbed = false;
    let mut systimer_perturbed = false;
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
        if let Some(after) = options.perturb_systimer_after
            && !systimer_perturbed
            && lockstep.counts().compared >= after
            && let Some((rd, address)) = lockstep.pending_load()
            && SYSTIMER_VALUE_LO.contains(&address)
        {
            let value = machine.x()[usize::from(rd)];
            Target::set_x(machine, rd, value.wrapping_add(1));
            systimer_perturbed = true;
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
    flash: &Flash,
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
        None => run_qemu(cli, flash, machine, lockstep, options),
    }
}

#[cfg(unix)]
fn run_qemu(
    cli: &Cli,
    flash: &Flash,
    machine: &mut Esp32C3,
    lockstep: &mut Lockstep,
    options: &Options<'_>,
) -> Result<(Outcome, u64), Error> {
    let file = std::fs::canonicalize(&flash.path).map_err(|source| Error::Read {
        path: flash.path.clone(),
        source,
    })?;
    eprintln!(
        "inkemu-lockstep: starting QEMU in {} on {}{}",
        cli.image,
        if flash.composed {
            "the flash composed from "
        } else {
            ""
        },
        file.display()
    );
    let given = if flash.composed {
        qemu::Flash::Composed(&flash.bytes)
    } else {
        qemu::Flash::File(&file)
    };
    let (mut qemu, pipe) =
        match qemu::Qemu::start(given, &cli.image, &cli.qemu, options.interrupted)? {
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
    _flash: &Flash,
    _machine: &mut Esp32C3,
    _lockstep: &mut Lockstep,
    _options: &Options<'_>,
) -> Result<(Outcome, u64), Error> {
    Err(Error::NotUnix)
}

/// Whether every row was used exactly as often as `path` says, one line `row N: X uses` each
/// (as the run prints them; `#` starts a comment), the rows it does not name never. Prints every
/// row that differs.
fn usage_as_expected(
    path: &std::path::Path,
    window: &lockstep::Window,
    usage: &lockstep::Usage,
) -> Result<bool, Error> {
    let text = std::fs::read_to_string(path).map_err(|source| Error::Read {
        path: path.to_owned(),
        source,
    })?;
    let mut expected = std::collections::BTreeMap::new();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let parsed = line
            .strip_prefix("row ")
            .and_then(|rest| rest.split_once(':'))
            .and_then(|(row, uses)| {
                let uses = uses.trim().strip_suffix("uses").unwrap_or(uses).trim();
                Some((row.trim().parse::<u16>().ok()?, uses.parse::<u64>().ok()?))
            });
        let Some((row, uses)) = parsed else {
            return Err(Error::Usage {
                path: path.to_owned(),
                line: line.to_owned(),
            });
        };
        expected.insert(row, uses);
    }
    let mut as_expected = true;
    for row in window.rows.iter().map(|row| row.number) {
        let (want, got) = (expected.get(&row).copied().unwrap_or(0), usage.uses(row));
        if want != got {
            eprintln!("inkemu-lockstep: row {row} was used {got} times where {want} were expected");
            as_expected = false;
        }
    }
    Ok(as_expected)
}

fn run(cli: &Cli, interrupted: &AtomicBool) -> Result<u8, Error> {
    let (mut machine, flash) = machine(cli)?;
    let mut lockstep = Lockstep::for_window(&machine, cli.window, cli.max_instructions);
    let options = Options {
        interrupted,
        perturb_after: cli.perturb_after,
        perturb_systimer_after: cli.perturb_systimer_after,
    };
    let (outcome, skipped) = comparison(cli, &flash, &mut machine, &mut lockstep, &options)?;
    let counts = lockstep.counts();
    let usage = lockstep.usage(&machine);
    let summary = format!(
        "instructions compared: {}\nduplicate blocks dropped: {}\nblocks preempted by an interrupt: {}\ninterrupts taken: {}\nresets crossed: {}\ntrace lines skipped: {skipped}\nuse of the rows of docs/lockstep-differences.md:\n{usage}",
        counts.compared,
        counts.duplicates,
        counts.preempted,
        counts.interrupts,
        counts.resets_crossed,
    );
    let end = match outcome {
        Outcome::Ended(end) => end,
        Outcome::Interrupted => {
            println!("{summary}interrupted by Ctrl-C");
            return Ok(EXIT_INTERRUPTED);
        }
    };
    match end {
        // A firmware's run never ends QEMU: a trace that stops before the window has ended is a
        // capture cut short, whether it is a file or QEMU's own pipe.
        End::Window(WindowEnd::TraceEnd) => {
            println!("{summary}");
            Err(Error::TraceEnded(counts.compared))
        }
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
                && Some(expected) != EndKind::of(&end)
            {
                eprintln!(
                    "inkemu-lockstep: the window ended by {end} where {expected:?} was expected"
                );
                code = EXIT_EXPECTATION;
            }
            if let Some(path) = &cli.expect_usage
                && !usage_as_expected(path, cli.window, &usage)?
                && code == 0
            {
                code = EXIT_EXPECTATION;
            }
            // Decision H3: a row the window did not need, or used outside its declaration, is a
            // table that no longer says what the comparison does. A window cut at
            // --max-instructions reports what it left unused without failing on it.
            let check = lockstep.check_usage(&machine, &end);
            for unused in &check.unused_in_partial_window {
                println!("unused in this partial window: {unused}");
            }
            for misuse in &check.failures {
                eprintln!("inkemu-lockstep: {misuse}");
            }
            if !check.passes() && code == 0 {
                code = EXIT_TABLE;
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
