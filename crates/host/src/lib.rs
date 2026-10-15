//! The host policy of inkemu's front ends (design SP2c, ADR 0010).
//!
//! What a front end does around a machine lives here, so that the command line and the desktop
//! application do the same: the files a run reads and writes and the refusals that guard them, the
//! mask ROM and its cache, the console script, the sessions, and the loop that runs a machine.
//! The modules `term` to `rom_store` came from `emu-cli` unchanged; `emu-cli` re-exports them.
pub mod exit;
pub mod firmware_file;
pub mod flash_file;
pub mod flash_state;
pub mod record;
pub mod rom_store;
pub mod run;
pub mod script_file;
pub mod session_file;
pub mod term;
