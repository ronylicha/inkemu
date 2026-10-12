//! The process exit codes of `inkemu`, defined once for the library and the binary.

/// The command did what was asked.
pub const EXIT_OK: u8 = 0;
/// `inkemu rvtest`: the test program reported a failure.
pub const EXIT_TEST_FAILURE: u8 = 1;
/// Bad input or an environment problem; the message on standard error says which.
pub const EXIT_ERROR: u8 = 2;
/// `inkemu run`: stopped by `--strict-mmio`.
pub const EXIT_STRICT_MMIO: u8 = 3;
/// `inkemu run`: the CPU waits for an interrupt and none can come.
pub const EXIT_IDLE: u8 = 4;

/// `inkemu run`: a second Ctrl-C ended the process before the first one had ended the run; once
/// the run has ended, Ctrl-C ends nothing. The value shells give a process ended by `SIGINT`
/// (128 + 2).
pub const EXIT_SECOND_INTERRUPT: u8 = 130;

/// The exit codes of `inkemu run`, for its `--help`.
pub const RUN_EXIT_CODES_HELP: &str = "\
Exit codes:
  0  the run ended normally: the cycle limit or the device-time limit was reached, Ctrl-C was
     pressed, or the reader of standard output went away
  2  error: unknown device, missing or invalid ROM, refused flash image or flash file, standard
     output cannot be written, or the flash file could not be written when the run ended (what
     the firmware wrote to the flash is then lost)
  3  stopped by --strict-mmio (an unmodelled register, an 8/16-bit register write, an operation the
     emulator cannot perform faithfully, flash that is not mapped, or a read through a flash
     window that may differ from the device)
  4  the CPU waits for an interrupt and none can come: no interrupt line is pending and no device
     has anything scheduled
130  Ctrl-C was pressed a second time before the run had stopped: no summary was printed, and the
     flash file is the one of before. Once the run has stopped, Ctrl-C ends nothing: the summary
     and the save of the flash file complete, with the exit code of the end of the run";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::RunEnd;

    #[test]
    fn the_run_help_lists_every_exit_code_a_run_can_end_with() {
        // The code of every end of a run, and the one the handler of Ctrl-C exits with.
        let codes = RunEnd::ALL
            .iter()
            .map(|end| (format!("{end:?}"), end.exit_code()))
            .chain([("a second Ctrl-C".to_owned(), EXIT_SECOND_INTERRUPT)]);
        for (end, code) in codes {
            assert!(
                format!("\n{RUN_EXIT_CODES_HELP}").contains(&format!("\n{code:>3}  ")),
                "{code}, the exit code of {end}, is missing from the help"
            );
        }
    }

    #[test]
    fn a_run_whose_flash_file_could_not_be_written_exits_2_unless_strict_mmio_stopped_it() {
        for end in RunEnd::ALL {
            let expected = if end == RunEnd::StrictMmio {
                EXIT_STRICT_MMIO
            } else {
                EXIT_ERROR
            };
            assert_eq!(end.exit_code_without_its_flash(), expected, "{end:?}");
        }
    }

    #[test]
    fn every_end_of_a_run_is_listed_once() {
        // The `match` has no wildcard: an end added to the enum does not compile here until
        // someone gives it a place, which is the moment to add it to the list; the place of each
        // end is the one it has there.
        let place = |end: RunEnd| match end {
            RunEnd::Budget => 0,
            RunEnd::TimeLimit => 1,
            RunEnd::ConsoleClosed => 2,
            RunEnd::ConsoleError => 3,
            RunEnd::StrictMmio => 4,
            RunEnd::Idle => 5,
            RunEnd::Interrupted => 6,
        };
        for (index, end) in RunEnd::ALL.into_iter().enumerate() {
            assert_eq!(place(end), index, "{end:?}");
        }
    }
}
