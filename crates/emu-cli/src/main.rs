//! inkemu command-line interface.
use clap::{Parser, Subcommand};
use cpu_rv32::{HartConfig, MisalignedPolicy};
use emu_cli::rvtest::{RunResult, TestMachine};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const EXIT_FAIL: u8 = 1;
const EXIT_ERROR: u8 = 2;

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
    };
    let machine = std::fs::read(elf)
        .map_err(|e| format!("cannot read {}: {e}", elf.display()))
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
            eprintln!("error: cannot write {}: {e}", path.display());
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
            ExitCode::from(EXIT_FAIL)
        }
        RunResult::Timeout => {
            eprintln!("error: no verdict after {max_steps} steps");
            ExitCode::from(EXIT_ERROR)
        }
    }
}
