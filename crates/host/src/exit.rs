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
/// `inkemu run --console-script`: the run ended, by a limit, Ctrl-C or a reader that went away,
/// before the script was through: it waited for a text, or a step is dated after the end.
pub const EXIT_SCRIPT_INCOMPLETE: u8 = 5;
/// `inkemu replay`: the replay reached the end of its session and gave another outcome than the
/// one recorded (G6).
pub const EXIT_DIVERGED: u8 = 6;

/// `inkemu run`: a second Ctrl-C ended the process before the run began its summary; once the
/// summary has begun, Ctrl-C ends nothing. The value shells give a process ended by `SIGINT`
/// (128 + 2).
pub const EXIT_SECOND_INTERRUPT: u8 = 130;

/// The exit codes of `inkemu run`, for its `--help`.
pub const RUN_EXIT_CODES_HELP: &str = "\
Exit codes:
  0  the run ended normally: the cycle limit or the device-time limit was reached, Ctrl-C was
     pressed, the reader of standard output went away, the device went into a deep sleep
     nothing inkemu models can wake it from, or the device turned its own supply off
  2  error: unknown device, missing or invalid ROM, refused flash image, flash file or SD image,
     standard output cannot be written, or the flash file or the SD image could not be written
     when the run ended (what the firmware wrote to them is then lost), or the session of
     --record could not be written
  3  stopped by --strict-mmio (an unmodelled register, an 8/16-bit register write, an operation the
     emulator cannot perform faithfully, flash that is not mapped, or a read through a flash
     window that may differ from the device)
  4  the CPU waits for an interrupt and none can come: no interrupt line is pending and no device
     has anything scheduled
  5  the run would have exited 0, but the --console-script was not through: it was waiting for a
     text, or one of its steps is dated after the end of the run
130  Ctrl-C was pressed a second time before the run began its summary: no summary was printed,
     and the flash file is the one of before. Once the summary has begun, Ctrl-C ends nothing:
     the summary and the save of the flash file complete, with the exit code of the end of the run";

/// The exit codes of `inkemu replay`, for its `--help`.
pub const REPLAY_EXIT_CODES_HELP: &str = "\
Exit codes:
  0  the replay reached the end of the session and gave what was recorded: the same end, device
     time, cycles, instructions, trace hash, final flash, SD card and consoles
  2  error: the session file cannot be read or is refused, the ROM is missing or is not the one
     the session was recorded on, standard output cannot be written, or the replay was cut
     before the end of its session (Ctrl-C, the reader of standard output went away): nothing
     was compared
  6  the replay diverges from the session: the summary names the first quantity that differs,
     with its two values
130  Ctrl-C was pressed a second time before the replay began its summary";

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
            .chain([
                ("a second Ctrl-C".to_owned(), EXIT_SECOND_INTERRUPT),
                (
                    "a console script not through".to_owned(),
                    EXIT_SCRIPT_INCOMPLETE,
                ),
            ]);
        for (end, code) in codes {
            assert!(
                format!("\n{RUN_EXIT_CODES_HELP}").contains(&format!("\n{code:>3}  ")),
                "{code}, the exit code of {end}, is missing from the help"
            );
        }
    }

    #[test]
    fn the_replay_help_lists_every_exit_code_a_replay_can_end_with() {
        for code in [EXIT_OK, EXIT_ERROR, EXIT_DIVERGED, EXIT_SECOND_INTERRUPT] {
            assert!(
                format!("\n{REPLAY_EXIT_CODES_HELP}").contains(&format!("\n{code:>3}  ")),
                "{code} is missing from the help of replay"
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
            RunEnd::DeepSleep => 7,
            RunEnd::PowerOff => 8,
        };
        for (index, end) in RunEnd::ALL.into_iter().enumerate() {
            assert_eq!(place(end), index, "{end:?}");
        }
    }
}
