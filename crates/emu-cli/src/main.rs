//! inkemu command-line interface.
use boards::MachineOptions;
use clap::{Parser, Subcommand};
use console_script::ScriptRun;
use cpu_rv32::MisalignedPolicy;
use emu_cli::exit::{
    EXIT_ERROR, EXIT_SECOND_INTERRUPT, EXIT_TEST_FAILURE, REPLAY_EXIT_CODES_HELP,
    RUN_EXIT_CODES_HELP,
};
use emu_cli::flash_state::RunFlash;
use emu_cli::record::{Recorder, record_session, replay_options, report_replay, start_replay};
use emu_cli::rom_store::{
    self, AnnouncingFetch, ESP_ROM_ELFS, ESP32C3_ROMS, HttpFetch, RomEntry, RomSource,
};
use emu_cli::run::{
    Interrupt, RunExtras, execute_with_script, exit_code_of_run, parse_device_seconds, relay,
};
use emu_cli::rvtest::{RunResult, TestMachine};
use emu_cli::script_file;
use emu_cli::session_file;
use emu_cli::term::sanitize;
use host::launch::{self, FlashRequest, RunRequest};
use session::{ConsoleDigests, ProfileText, RomIdentity};
use sha2::{Digest as _, Sha256};
use soc_esp32c3::{ConsoleChannel, UsbHost};
use std::fmt::Write as _;
use std::io;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

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
    /// Run a bare-metal RISC-V conformance test ELF (riscv-tests or riscv-arch-test) on the
    /// conformance bench: an RV32IMC hart with machine and user modes and a 16-entry PMP.
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
        /// an invalid header, as a blank chip does. The image is only read: what the firmware
        /// writes to the flash is kept by --flash-file, or lost when the run ends.
        #[arg(long)]
        flash: Option<PathBuf>,
        /// Application image to boot (the .bin of an ESP-IDF build or of a release, its first
        /// byte 0xE9): the flash is composed from it, as a device freshly flashed holds it. The
        /// bootloader inkemu ships for the device goes at offset 0, the partition table of the
        /// device profile at 0x8000, the application in the slot the bootloader starts (the
        /// factory application, else the first OTA slot), and every other byte is erased, the
        /// OTA data partition included.
        /// The file is only read. A whole flash image goes to --flash instead.
        #[arg(long, value_name = "FILE", conflicts_with = "flash")]
        firmware: Option<PathBuf>,
        /// Bootloader to compose the flash with instead of the one inkemu ships for the device:
        /// the bootloader.bin of a build. It is checked against the device and written as it is.
        #[arg(long, value_name = "FILE", requires = "firmware")]
        bootloader: Option<PathBuf>,
        /// Keep the flash of the device in this file from one run to the next: a raw image of
        /// exactly the size of the flash. A run starts from the file when it exists (--flash or
        /// --firmware is then refused unless --reset-flash is given) and writes it when it ends:
        /// at a limit, at the first Ctrl-C, at a --strict-mmio stop, when the CPU waits for ever.
        /// The file is replaced in one step, never written while the run goes on, so a run that
        /// is killed leaves the file of before. The new content goes through a temporary file
        /// beside it (.FILE.tmp, the name of FILE after a dot): what is found at that name (an
        /// interrupted save leaves its file there) is removed by the next run that starts, and
        /// by the save, which say so. A lock file
        /// beside it (FILE.lock) refuses a second run on the same file; it is left in place. A
        /// flash holds what the firmware keeps, network credentials among it: on Unix a file
        /// created here is readable and writable by its owner only, one that is replaced keeps
        /// its mode, and one that cannot be written is refused.
        #[arg(long, value_name = "FILE")]
        flash_file: Option<PathBuf>,
        /// Start from the --flash image, or from an erased flash, although the flash file
        /// exists, and replace the file when the run ends. With --firmware, start from the flash
        /// composed from it.
        #[arg(long, requires = "flash_file")]
        reset_flash: bool,
        /// Stop when the CPU has executed this many cycles of its clock. The limit counts executed
        /// cycles, not device time: a sleeping CPU executes none, so a firmware that sleeps
        /// reaches it late, and one whose sleep nothing interrupts never does; --max-time bounds
        /// both.
        #[arg(long)]
        max_cycles: Option<u64>,
        /// Stop when this many seconds of device time have elapsed since power-on (a decimal
        /// number: 30, 2.5, 0.001). Device time goes on while the CPU sleeps and across resets.
        // A negative number is a value, so that it is refused by the parser, with its message.
        #[arg(
            long,
            value_name = "SECONDS",
            value_parser = parse_device_seconds,
            allow_negative_numbers = true
        )]
        max_time: Option<u64>,
        /// Stop at the first access to a register the emulator does not model, 8/16-bit write to a
        /// peripheral register, operation the emulator cannot perform faithfully, access to flash
        /// that is not mapped, or read through a flash window that may differ from the device
        /// (exit code 3).
        #[arg(long)]
        strict_mmio: bool,
        /// Which console of the device goes to standard output. Without it, the console the device
        /// profile names; the first line on standard error says which console is shown and where
        /// the choice came from.
        #[arg(long, value_enum)]
        console: Option<ConsoleArg>,
        /// What the host sends to the consoles, and when: a text file whose first line is
        /// `inkemu-console-script 1`, then one step per line: `send CHANNEL "BYTES"`, `wait 250ms`,
        /// `wait-for CHANNEL "TEXT"` (the next steps are dated from the device time the text
        /// reached the host), `usb-host absent|attached|open`. CHANNEL is uart0, uart1 or
        /// usb-serial-jtag; strings take \r \n \t \\ \" \xHH, and Enter is \r. Every step is dated
        /// in device time, so two runs with the same script are the same run. A script not through
        /// when the run ends makes it exit 5.
        #[arg(long, value_name = "FILE")]
        console_script: Option<PathBuf>,
        /// The USB host on the USB-Serial-JTAG port when the device starts: absent (no cable),
        /// attached (the port closed: frames, nothing read) or open (a program reads and writes
        /// the port).
        #[arg(long, value_enum, default_value_t = UsbHostArg::Open)]
        usb_host: UsbHostArg,
        /// Use the ROM ELF files of this directory and never download.
        #[arg(long)]
        rom_dir: Option<PathBuf>,
        /// Print the execution trace hash at the end (determinism checks).
        #[arg(long)]
        trace_hash: bool,
        /// Record the run in this file, a session that `inkemu replay` replays: the identity of
        /// the ROM (not the ROM), the device profile, the whole flash the run starts from, the
        /// limits, the USB host at power-on, every input the consoles and the USB host received
        /// with its device time, and what the run came to (its end, device time, cycles,
        /// instructions, trace hash, final flash and consoles). The file is written when the run
        /// ends, however it ends but at a second Ctrl-C: a run cut by Ctrl-C or by its reader is
        /// recorded with the device time it stopped at. The path may not be a file the run reads
        /// or writes, even one that does not exist yet (the flash file, its lock and temporary
        /// files, a ROM file), and its directory must exist. A session holds the whole flash: on
        /// Unix the file is readable and writable by its owner only. Recording turns the trace
        /// hash on.
        #[arg(long, value_name = "FILE")]
        record: Option<PathBuf>,
    },
    /// Replay a session recorded by `inkemu run --record` and compare what it gives with what was
    /// recorded.
    #[command(after_help = REPLAY_EXIT_CODES_HELP)]
    Replay {
        /// The session file.
        session: PathBuf,
        /// Which console of the device goes to standard output. Without it, the console the
        /// device profile of the session names.
        #[arg(long, value_enum)]
        console: Option<ConsoleArg>,
        /// Use the ROM ELF files of this directory and never download.
        #[arg(long)]
        rom_dir: Option<PathBuf>,
    },
    /// Print the notices and the licences of the third-party material inkemu embeds as data: the
    /// register descriptions of the chip and the 2nd-stage bootloader it ships (the notices of the
    /// Rust libraries it links are not listed yet).
    Licenses,
    /// Manage the mask-ROM files the emulator needs.
    Rom {
        #[command(subcommand)]
        action: RomAction,
    },
    /// Manage the image files of SD cards.
    Sd {
        #[command(subcommand)]
        action: SdAction,
    },
}

#[derive(Subcommand)]
enum SdAction {
    /// Create a blank SD image: an empty FAT32 volume, which the file takes on disk only as it is
    /// written. Exit codes: 0 created; 2 refused (size, label, path) or not written.
    Create {
        /// Where to create the image; an existing file is never replaced.
        path: PathBuf,
        /// The size of the card in MiB, from 64 to 2097024 (up to 2 GiB a standard-capacity
        /// card, above a high-capacity one).
        #[arg(long)]
        size: u64,
        /// The volume label: up to 11 letters, digits, spaces and FAT's punctuation.
        #[arg(long, default_value = host::sd_format::DEFAULT_LABEL)]
        label: String,
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

#[derive(Clone, Copy, clap::ValueEnum)]
enum UsbHostArg {
    Absent,
    Attached,
    Open,
}

impl From<UsbHostArg> for UsbHost {
    fn from(arg: UsbHostArg) -> UsbHost {
        match arg {
            UsbHostArg::Absent => UsbHost::Absent,
            UsbHostArg::Attached => UsbHost::Attached,
            UsbHostArg::Open => UsbHost::Open,
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
        Command::Licenses => match print_stdout(&boards::notices::notices_text()) {
            ControlFlow::Continue(()) => ExitCode::SUCCESS,
            ControlFlow::Break(code) => code,
        },
        Command::Run {
            device,
            flash,
            firmware,
            bootloader,
            flash_file,
            reset_flash,
            max_cycles,
            max_time,
            strict_mmio,
            console,
            console_script,
            usb_host,
            rom_dir,
            trace_hash,
            record,
        } => run_device(
            &device,
            &FlashRequest {
                image: flash.as_deref(),
                firmware: firmware.as_deref(),
                bootloader: bootloader.as_deref(),
                file: flash_file.as_deref(),
                reset: reset_flash,
            },
            rom_dir,
            console_script.as_deref(),
            MachineOptions {
                strict_mmio,
                // The outcome of a session holds the trace hash; it changes nothing the machine
                // does.
                trace_hash: trace_hash || record.is_some(),
                usb_host: usb_host.into(),
            },
            &RunRequest {
                max_cycles,
                max_device_nanos: max_time,
                console: console.map(ConsoleChannel::from),
            },
            record.as_deref(),
        ),
        Command::Replay {
            session,
            console,
            rom_dir,
        } => run_replay(&session, console.map(ConsoleChannel::from), rom_dir),
        Command::Rom {
            action:
                RomAction::Fetch {
                    device,
                    all,
                    rom_dir,
                },
        } => run_rom_fetch(device.as_deref(), all, rom_dir),
        Command::Sd {
            action: SdAction::Create { path, size, label },
        } => run_sd_create(&path, size, &label),
    }
}

/// `inkemu sd create`: the image, then a line that says what it is.
fn run_sd_create(path: &Path, mebibytes: u64, label: &str) -> ExitCode {
    // A creation alone: no other file of a run to keep apart from.
    match host::sd_format::create_blank(path, mebibytes, label, &[]) {
        Ok(layout) => {
            let line = format!(
                "{}: {}\n",
                sanitize(&path.display().to_string()),
                layout.describe()
            );
            match print_stdout(&line) {
                ControlFlow::Continue(()) => ExitCode::SUCCESS,
                ControlFlow::Break(code) => code,
            }
        }
        Err(e) => fail(&e),
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

fn run_device(
    device: &str,
    flash: &FlashRequest<'_>,
    rom_dir: Option<PathBuf>,
    console_script: Option<&Path>,
    machine_options: MachineOptions,
    request: &RunRequest,
    record: Option<&Path>,
) -> ExitCode {
    let profile = match launch::profile(device) {
        Ok(p) => p,
        Err(e) => return fail(&e),
    };
    let (run_options, console_note) = request.resolve(&profile, machine_options.usb_host);
    // Read before anything else of the run: a refused script needs no flash and no ROM.
    let script = match console_script.map(script_file::read_script).transpose() {
        Ok(script) => script,
        Err(e) => return fail(&e),
    };
    let entry = rom_store::rom_entry_for(profile.chip_revision);
    let rom = (rom_dir.as_deref(), entry);
    let profile_text = match launch::refuse_record_path(record, flash, console_script, rom, device)
    {
        Ok(text) => text,
        Err(e) => return fail(&e),
    };
    let flash_size = profile.flash.size();
    // The files a run only reads and the flash file are checked first: refusing them needs no
    // ROM, and no download should start for a run that cannot happen.
    if let Err(e) = launch::refuse_the_images_as_flash_file(flash) {
        return fail(&e);
    }
    let fresh = match launch::starting_flash(&profile, flash) {
        Ok(started) => started,
        Err(e) => return fail(&e),
    };
    // From here on the flash file is locked, until the process ends.
    let (run_flash, content) =
        match RunFlash::begin(flash.file, flash.reset, fresh.seed, flash_size) {
            Ok(begun) => begun,
            Err(e) => return fail(&e),
        };
    // Said once the flash file accepts the seed: a run refused for its flash file says only why.
    for warning in &fresh.warnings {
        write_line(io::stderr(), &format_args!("warning: {warning}"));
    }
    let flash_note = launch::flash_note(flash.file, &run_flash, &fresh.note, flash_size);
    let rom = match launch::load_rom(rom_dir, entry, &mut io::stderr()) {
        Ok(rom) => rom,
        Err(e) => return fail(&e),
    };
    let rom_identity = RomIdentity::of(entry.rom_revision, &rom);
    let mut machine = match content {
        Some(bytes) => match profile.instantiate_with_flash(rom, bytes, machine_options) {
            Ok(machine) => machine,
            Err(e) => return fail(&e),
        },
        None => profile.instantiate(rom, machine_options),
    };
    // Nothing refuses the run from here on but its flash file: only now is what an interrupted
    // save left beside it removed. A run refused above left the directory as it found it.
    if let Err(e) = run_flash.accept(&mut io::stderr().lock()) {
        return fail(&e);
    }
    write_line(
        io::stderr(),
        &format_args!(
            "{}; {flash_note}",
            launch::header(&profile, entry, &machine, &console_note)
        ),
    );
    // What the session of the run keeps beside the machine, taken at power-on.
    let mut recorder = profile_text.map(|text| {
        let profile = ProfileText {
            id: profile.id.clone(),
            text: text.to_owned(),
        };
        let limits = request.limits(machine_options.strict_mmio);
        Recorder::begin(rom_identity, profile, &machine, limits)
    });
    // The steps before the first wait are dated now, at power-on.
    let mut script_run = match script
        .as_ref()
        .map(|script| ScriptRun::start(script, &mut machine))
        .transpose()
    {
        Ok(run) => run,
        Err(e) => return fail(&e),
    };
    let interrupt = interrupt_on_ctrl_c(
        "the run is not affected, but interrupting it will not print its summary: use --max-cycles or --max-time to bound it",
    );
    let end = execute_with_script(
        &mut machine,
        &run_options,
        RunExtras {
            script: script_run.as_mut(),
            digests: recorder.as_mut().map(Recorder::digests),
        },
        &interrupt,
        &stdout_reader_gone,
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    );
    let script_status = script_run
        .as_ref()
        .map(|run| run.status(machine.device_time()));
    // The run is over: what follows is no part of it, and of no trace. Since the first line of
    // its summary Ctrl-C ends nothing (`execute` told `interrupt`): the summary, the save of the
    // flash file and the write of the session are bounded (one write of the flash, one of the
    // session, and their flushes to the disk), and they complete; the exit code is the one of
    // the end above.
    let flash_saved = run_flash.finish(&machine, &mut io::stderr().lock());
    let session_saved = recorder.zip(record).is_none_or(|(recorder, path)| {
        record_session(recorder, &machine, end, path, &mut io::stderr().lock())
    });
    ExitCode::from(exit_code_of_run(
        end,
        script_status.as_ref(),
        flash_saved && session_saved,
    ))
}

/// `inkemu replay`: replays the session in `path` as a run shows its console, then compares what
/// it gives with what the session recorded (G6). It reads no path of the recording and writes
/// nothing of the run, not even the flash; without `rom_dir`, the ROM cache downloads the ROM on
/// first use, as for `run`. A session comes from anywhere: what it says reaches the terminal
/// with its control characters escaped.
fn run_replay(path: &Path, console: Option<ConsoleChannel>, rom_dir: Option<PathBuf>) -> ExitCode {
    let session = match session_file::read_session(path) {
        Ok(session) => session,
        Err(e) => return fail(&e),
    };
    // The reader rebuilt it once already: it is the device of the session.
    let profile = match session.profile.device() {
        Ok(profile) => profile,
        Err(e) => return fail(&sanitize(&e.to_string())),
    };
    let entry = rom_store::rom_entry_for(profile.chip_revision);
    let rom = match launch::load_rom(rom_dir, entry, &mut io::stderr()) {
        Ok(rom) => rom,
        Err(e) => return fail(&e),
    };
    let mut machine = match start_replay(&session, rom, entry.rom_revision) {
        Ok(machine) => machine,
        Err(message) => return fail(&message),
    };
    let (shown, origin) = launch::shown_console(&profile, console);
    let options = replay_options(&session, shown);
    let flash_sha256 = emu_cli::term::hex(&Sha256::digest(&session.flash));
    write_line(
        io::stderr(),
        &format_args!(
            "{}; replaying {}: {} inputs, flash at start sha256 {flash_sha256}",
            launch::header(
                &profile,
                entry,
                &machine,
                &launch::console_note(shown, origin, session.usb_host)
            ),
            launch::file_name(path),
            session.inputs.len()
        ),
    );
    let interrupt = interrupt_on_ctrl_c(
        "the replay is not affected, but interrupting it will not print its summary",
    );
    let mut digests = ConsoleDigests::default();
    let end = execute_with_script(
        &mut machine,
        &options,
        RunExtras {
            script: None,
            digests: Some(&mut digests),
        },
        &interrupt,
        &stdout_reader_gone,
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    );
    ExitCode::from(report_replay(
        &session,
        &machine,
        end,
        digests,
        &mut io::stderr().lock(),
    ))
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

/// The request to stop of a run, which Ctrl-C makes ([`watch_ctrl_c`]); when Ctrl-C cannot be
/// watched, a warning says so, with `consequence`.
fn interrupt_on_ctrl_c(consequence: &str) -> Arc<Interrupt> {
    let interrupt = Arc::new(Interrupt::new());
    if let Err(e) = watch_ctrl_c(&interrupt) {
        write_line(
            io::stderr(),
            &format_args!("warning: cannot watch for Ctrl-C: {e}; {consequence}"),
        );
    }
    interrupt
}

/// Makes Ctrl-C end the run at the next slice, with its summary. A second Ctrl-C ends the process
/// at once, for a run that does not come back to the slice boundary, as long as the run has not
/// begun its summary; from then on Ctrl-C ends nothing ([`Interrupt`]).
///
/// The handler runs on a thread of its own, not in the signal context; it only reads and sets
/// the two flags of `interrupt`.
fn watch_ctrl_c(interrupt: &Arc<Interrupt>) -> Result<(), ctrlc::Error> {
    let interrupt = Arc::clone(interrupt);
    ctrlc::set_handler(move || {
        if interrupt.request() {
            std::process::exit(i32::from(EXIT_SECOND_INTERRUPT));
        }
    })
}

/// The ROM entries a `rom fetch` invocation asks for. clap already refuses the two invalid
/// combinations; refusing again keeps `--all`'s download from ever being the default.
fn select_roms(device: Option<&str>, all: bool) -> Result<Vec<&'static RomEntry>, String> {
    match (device, all) {
        (Some(id), false) => launch::profile(id)
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
    let config = emu_cli::rvtest::bench_hart(if allow_misaligned {
        MisalignedPolicy::Allow
    } else {
        MisalignedPolicy::Trap
    });
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

    /// `--console` is optional, and nothing but the profile picks the console when it is absent.
    #[test]
    fn console_is_optional_on_the_command_line() {
        let parsed = Cli::try_parse_from(["inkemu", "run", "--device", "x4"]).unwrap();
        let Command::Run { console, .. } = parsed.command else {
            panic!("`run` parses as the run command")
        };
        assert!(console.is_none());
        let parsed =
            Cli::try_parse_from(["inkemu", "run", "--device", "x4", "--console", "uart0"]).unwrap();
        let Command::Run { console, .. } = parsed.command else {
            panic!("`run` parses as the run command")
        };
        assert!(matches!(console, Some(ConsoleArg::Uart0)));
    }

    #[test]
    fn selecting_all_yields_the_three_roms_and_a_device_yields_one() {
        assert_eq!(select_roms(None, true).map(|roms| roms.len()), Ok(3));
        assert_eq!(select_roms(Some("x4"), false).map(|roms| roms.len()), Ok(1));
    }
}
