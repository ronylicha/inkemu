//! The process exit codes of `inkemu`, defined once for the library and the binary.

/// The command did what was asked.
pub const EXIT_OK: u8 = 0;
/// `inkemu rvtest`: the test program reported a failure.
pub const EXIT_TEST_FAILURE: u8 = 1;
/// Bad input or an environment problem; the message on standard error says which.
pub const EXIT_ERROR: u8 = 2;
/// `inkemu run`: stopped by `--strict-mmio`.
pub const EXIT_STRICT_MMIO: u8 = 3;
/// `inkemu run`: the CPU waits for an interrupt and nothing can raise one.
pub const EXIT_IDLE: u8 = 4;

/// The exit codes of `inkemu run`, for its `--help`.
pub const RUN_EXIT_CODES_HELP: &str = "\
Exit codes:
  0  the run ended normally: the cycle limit was reached, or the reader of standard output went away
  2  error: unknown device, missing or invalid ROM, refused flash image, or standard output cannot be written
  3  stopped by --strict-mmio (an unmodelled register, an 8/16-bit register write, an operation the
     emulator cannot perform faithfully, or flash that is not mapped)
  4  the CPU is waiting for an interrupt and nothing can raise one (interrupts are not emulated yet)";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_run_help_lists_every_exit_code_a_run_can_end_with() {
        for code in [EXIT_OK, EXIT_ERROR, EXIT_STRICT_MMIO, EXIT_IDLE] {
            assert!(
                RUN_EXIT_CODES_HELP.contains(&format!("\n  {code}  ")),
                "{code} missing from the help"
            );
        }
    }
}
