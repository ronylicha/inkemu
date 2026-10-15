//! Library side of the inkemu CLI, so integration tests can drive the same code as the binary.
//!
//! The host policy of a run (files, ROM, sessions, the run loop) lives in the `host` crate, shared
//! with the desktop application; it is re-exported here under the paths it always had.
pub use host::{
    exit, firmware_file, flash_file, flash_state, record, rom_store, run, script_file,
    session_file, term,
};
pub mod inspect;
pub mod reader;
pub mod rvtest;
