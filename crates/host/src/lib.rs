//! The host policy of inkemu's front ends (design of sub-project 2c, ADR 0010).
//!
//! What a front end does around a machine lives here, so that the command line and the desktop
//! application do the same: the files a run reads and writes and the refusals that guard them, the
//! mask ROM and its cache, the console script, the sessions, and the loop that runs a machine.
//! The modules `term` to `rom_store` came from `emu-cli` unchanged; `emu-cli` re-exports them.
//! [`launch`] holds how a run starts, which the binary of the command line decided before.
//!
//! The rest serves the interface: [`ports`] are where the panel image and the buttons of a device
//! plug in, with a test pattern ([`pattern`]) until the display is wired, and [`catalog`] chooses
//! them per device.
pub mod capture;
pub mod catalog;
pub mod exit;
pub mod firmware_file;
pub mod flash_file;
pub mod flash_state;
pub mod launch;
pub mod pattern;
pub mod ports;
pub mod record;
pub mod rom_store;
pub mod run;
pub mod script_file;
pub mod session_file;
pub mod term;
pub mod utf8;
