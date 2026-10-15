//! The host policy of inkemu's front ends (design of sub-project 2c, ADR 0011).
//!
//! What a front end does around a machine lives here, so that the command line and the desktop
//! application do the same: the files a run reads and writes and the refusals that guard them, the
//! mask ROM and its cache, the console script, the sessions, and the loop that runs a machine.
//! The modules `term` to `rom_store` came from `emu-cli` unchanged; `emu-cli` re-exports them.
//! [`launch`] holds how a run starts, which the binary of the command line decided before.
//!
//! The rest serves the interface: [`engine`] makes one machine live without host I/O,
//! [`device`] runs it on a thread of its own and takes commands, [`ports`] are where the panel
//! image and the buttons of a device plug in, with a test pattern ([`pattern`]) until the display
//! is wired, and [`catalog`] chooses them per device. [`utf8`] decodes the consoles as they speak
//! and [`capture`] makes a PNG of the panel.
//!
//! The SD card of a device (design `SP2e`, ADR 0014) is kept in an image file: [`sd_image`] opens,
//! locks and writes it through a journal, and [`sd_format`] creates a blank FAT32 one;
//! [`sd_run`] puts its card in the slot of the wired device for each life and commits what the
//! life changed (design `SP2d`).
pub mod capture;
pub mod catalog;
pub mod device;
pub mod engine;
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
pub mod sd_format;
pub mod sd_image;
pub mod sd_run;
pub mod session_file;
pub mod term;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod utf8;
