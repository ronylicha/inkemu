//! What a debugger asks of the machine (design `SP4a`, ADR 0016): to stop before the instruction
//! at an address (a breakpoint), to stop after an instruction that read or wrote some bytes (a
//! watchpoint), and to read memory without the effects of a load.
//!
//! None of them changes what the machine executes. A breakpoint ends a call of
//! [`Esp32C3::run_stopping`](crate::Esp32C3::run_stopping) at an instruction boundary, as a cycle
//! limit ends one; the next call goes on from the same state, past the breakpoint it stopped at.
//! A watchpoint is a note the bus takes of an access the CPU makes anyway. A peek reads the
//! memories as they are and does not reach a peripheral, whose registers a read may change.
//!
//! The policy (how many points, which regions a user may watch or see) belongs to the front end:
//! the `debugger` crate.
use crate::machine::StopReason;

/// Which accesses of the CPU a watchpoint stops on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchOn {
    /// Loads.
    Read,
    /// Stores.
    Write,
    /// Loads and stores.
    Access,
}

/// An access of the CPU to memory or to a peripheral register: a load or a store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

/// A watchpoint: the `length` bytes from `address`, watched for the accesses `on` says.
///
/// The SRAM is one memory seen through two buses: a watchpoint on an address of either view
/// watches the cells, through whichever view the CPU reaches them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Watch {
    pub address: u32,
    pub length: u32,
    pub on: WatchOn,
}

impl Watch {
    /// Whether an access of `size` bytes at `address` (both as the bus sees them, the SRAM by its
    /// data-bus address) touches the watched bytes and is of a kind the watchpoint stops on.
    #[must_use]
    pub(crate) fn covers(&self, address: u32, size: u32, access: Access) -> bool {
        let wanted = match (self.on, access) {
            (WatchOn::Access, _)
            | (WatchOn::Read, Access::Read)
            | (WatchOn::Write, Access::Write) => true,
            (WatchOn::Read, Access::Write) | (WatchOn::Write, Access::Read) => false,
        };
        let start = u64::from(crate::bus::canonical(self.address));
        let end = start + u64::from(self.length);
        let first = u64::from(address);
        let last = first + u64::from(size);
        wanted && first < end && start < last
    }
}

/// The access a watchpoint stopped on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatchHit {
    /// The address of the access, as the instruction gave it.
    pub address: u32,
    /// Its size in bytes: 1, 2 or 4.
    pub size: u8,
    pub access: Access,
    /// The value the load read, or the store wrote, right-aligned to the size.
    pub value: u32,
    /// The address of the instruction that made the access.
    pub pc: u32,
}

/// Why a call of [`Esp32C3::run_stopping`](crate::Esp32C3::run_stopping) stopped for a debugger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugStop {
    /// The CPU is about to execute the instruction at `pc`, a breakpoint: nothing of it is done.
    Breakpoint { pc: u32 },
    /// The instruction before the boundary the machine is at made this access to watched bytes.
    Watchpoint(WatchHit),
}

/// What ended a call of [`Esp32C3::run_stopping`](crate::Esp32C3::run_stopping): a reason the
/// machine stops for on its own, or a point of the debugger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunStop {
    Machine(StopReason),
    Debug(DebugStop),
}

/// One byte of the address space, read without effect ([`Esp32C3::peek`](crate::Esp32C3::peek)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Peeked {
    /// A byte of the mask ROM or of a RAM.
    Byte(u8),
    /// A byte of the flash, through a cache window and its MMU entry.
    Flash(u8),
    /// A byte of a flash window that no flash byte backs: its MMU entry is invalid, or maps a
    /// page past the end of the chip.
    Unbacked,
    /// A byte of a peripheral register: not read, since reading a register may change it.
    Register,
    /// No memory nor device is mapped at this address.
    Unmapped,
}
