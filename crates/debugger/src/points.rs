//! The breakpoints and watchpoints of a debugger, and their bounds (4a-D2, 4a-D3, 4a-D6, 4a-D7).
//!
//! They belong to the emulator, not to one life of its machine (4a-D8): [`Points::apply`] hands
//! them to each machine. Setting a point that is set already, or clearing one that is not, changes
//! nothing and is no error: a click and the view it came from may cross.
use crate::region::{Region, overlaps_flash};
use soc_esp32c3::{Esp32C3, Watch, WatchOn};
use std::collections::BTreeSet;

/// Breakpoints a debugger may set at once.
pub const MAX_BREAKPOINTS: usize = 64;
/// Watchpoints a debugger may set at once.
pub const MAX_WATCHPOINTS: usize = 16;
/// The bytes one watchpoint may cover.
pub const MAX_WATCH_LENGTH: u32 = 4096;

/// Why a point was refused: the cause and the remedy, in words a user reads.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PointError {
    #[error(
        "a breakpoint goes on the address of an instruction, which is even: {address:#010x} is odd; give an even address"
    )]
    OddAddress { address: u32 },
    #[error(
        "no instruction can be fetched at {address:#010x}, which is {}; set a breakpoint in the ROM, a RAM or the flash",
        region.describe()
    )]
    NoCode { address: u32, region: Region },
    #[error("{MAX_BREAKPOINTS} breakpoints are set already; clear one first")]
    TooManyBreakpoints,
    #[error("{MAX_WATCHPOINTS} watchpoints are set already; clear one first")]
    TooManyWatchpoints,
    #[error("a watchpoint covers 1 to {MAX_WATCH_LENGTH} bytes, not {length}")]
    WatchLength { length: u32 },
    #[error(
        "the watchpoint at {address:#010x} would end past the end of the address space; shorten it"
    )]
    PastTheEnd { address: u32 },
    #[error(
        "the debugger does not watch the flash windows (the flash may hold personal data, and a store there faults anyway); watch the RAM or the registers"
    )]
    FlashWindow,
    #[error(
        "nothing is mapped at {address:#010x}: no access can reach it; watch the RAM or the registers"
    )]
    Unmapped { address: u32 },
    #[error("a watchpoint stops on `read`, `write` or `access`, not `{name}`")]
    UnknownAccess { name: String },
}

/// The points a debugger set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Points {
    breakpoints: BTreeSet<u32>,
    watchpoints: Vec<Watch>,
}

/// Points as an interface sends them, to start a run with them: an interface keeps the points
/// of its user from one run to the next, and a run starts with them before its first
/// instruction.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PointsRequest {
    #[serde(default)]
    pub breakpoints: Vec<u32>,
    #[serde(default)]
    pub watchpoints: Vec<WatchRequest>,
}

/// A watchpoint as an interface sends it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WatchRequest {
    pub address: u32,
    pub length: u32,
    /// `read`, `write` or `access`.
    pub on: String,
}

impl WatchRequest {
    /// The watchpoint.
    ///
    /// # Errors
    /// [`PointError::UnknownAccess`] for an `on` that names no access.
    pub fn watch(&self) -> Result<Watch, PointError> {
        let on = watch_on(&self.on).ok_or_else(|| PointError::UnknownAccess {
            name: self.on.clone(),
        })?;
        Ok(Watch {
            address: self.address,
            length: self.length,
            on,
        })
    }
}

impl Points {
    /// The points `request` gives, each checked as when it is set alone.
    ///
    /// # Errors
    /// The first refusal, as [`Points::set_breakpoint`] and [`Points::set_watchpoint`] give it.
    pub fn from_request(request: &PointsRequest) -> Result<Points, PointError> {
        let mut points = Points::default();
        for &address in &request.breakpoints {
            points.set_breakpoint(address)?;
        }
        for watch in &request.watchpoints {
            points.set_watchpoint(watch.watch()?)?;
        }
        Ok(points)
    }

    /// Sets a breakpoint before the instruction at `address`.
    ///
    /// # Errors
    /// [`PointError::OddAddress`], [`PointError::NoCode`] (registers, nothing),
    /// [`PointError::TooManyBreakpoints`].
    pub fn set_breakpoint(&mut self, address: u32) -> Result<(), PointError> {
        if !address.is_multiple_of(2) {
            return Err(PointError::OddAddress { address });
        }
        let region = Region::of(address);
        if !region.holds_code() {
            return Err(PointError::NoCode { address, region });
        }
        if !self.breakpoints.contains(&address) && self.breakpoints.len() >= MAX_BREAKPOINTS {
            return Err(PointError::TooManyBreakpoints);
        }
        self.breakpoints.insert(address);
        Ok(())
    }

    /// Clears the breakpoint at `address`, if one is set.
    pub fn clear_breakpoint(&mut self, address: u32) {
        self.breakpoints.remove(&address);
    }

    /// Sets a watchpoint.
    ///
    /// # Errors
    /// [`PointError::WatchLength`], [`PointError::PastTheEnd`], [`PointError::FlashWindow`]
    /// (4a-D6), [`PointError::Unmapped`] (nothing mapped at its first byte),
    /// [`PointError::TooManyWatchpoints`].
    pub fn set_watchpoint(&mut self, watch: Watch) -> Result<(), PointError> {
        if watch.length == 0 || watch.length > MAX_WATCH_LENGTH {
            return Err(PointError::WatchLength {
                length: watch.length,
            });
        }
        if u64::from(watch.address) + u64::from(watch.length) > 1 << 32 {
            return Err(PointError::PastTheEnd {
                address: watch.address,
            });
        }
        if overlaps_flash(watch.address, watch.length) {
            return Err(PointError::FlashWindow);
        }
        if Region::of(watch.address) == Region::Unmapped {
            return Err(PointError::Unmapped {
                address: watch.address,
            });
        }
        if self.watchpoints.contains(&watch) {
            return Ok(());
        }
        if self.watchpoints.len() >= MAX_WATCHPOINTS {
            return Err(PointError::TooManyWatchpoints);
        }
        self.watchpoints.push(watch);
        Ok(())
    }

    /// Clears `watch`, if it is set.
    pub fn clear_watchpoint(&mut self, watch: Watch) {
        self.watchpoints.retain(|set| *set != watch);
    }

    /// The breakpoints, in increasing order.
    pub fn breakpoints(&self) -> impl Iterator<Item = u32> + '_ {
        self.breakpoints.iter().copied()
    }

    #[must_use]
    pub fn has_breakpoint(&self, address: u32) -> bool {
        self.breakpoints.contains(&address)
    }

    /// The watchpoints, in the order they were set.
    #[must_use]
    pub fn watchpoints(&self) -> &[Watch] {
        &self.watchpoints
    }

    /// Gives the points to `machine`, in place of the ones it had.
    pub fn apply(&self, machine: &mut Esp32C3) {
        let breakpoints: Vec<u32> = self.breakpoints().collect();
        machine.set_breakpoints(&breakpoints);
        machine.set_watchpoints(&self.watchpoints);
    }

    /// The points as an interface shows them.
    #[must_use]
    pub fn view(&self) -> PointsView {
        PointsView {
            breakpoints: self.breakpoints().collect(),
            watchpoints: self
                .watchpoints
                .iter()
                .map(|watch| WatchView {
                    address: watch.address,
                    length: watch.length,
                    on: watch_on_name(watch.on),
                })
                .collect(),
        }
    }
}

/// The name of `on` in the interface: `read`, `write` or `access`.
#[must_use]
pub const fn watch_on_name(on: WatchOn) -> &'static str {
    match on {
        WatchOn::Read => "read",
        WatchOn::Write => "write",
        WatchOn::Access => "access",
    }
}

/// The [`WatchOn`] named `name` in the interface.
#[must_use]
pub fn watch_on(name: &str) -> Option<WatchOn> {
    [WatchOn::Read, WatchOn::Write, WatchOn::Access]
        .into_iter()
        .find(|on| watch_on_name(*on) == name)
}

/// The points, for an interface.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PointsView {
    /// In increasing order.
    pub breakpoints: Vec<u32>,
    pub watchpoints: Vec<WatchView>,
}

/// A watchpoint, for an interface.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchView {
    pub address: u32,
    pub length: u32,
    /// `read`, `write` or `access`.
    pub on: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use soc_esp32c3::SocConfig;
    use soc_esp32c3::memmap::{FLASH_DBUS_BASE, FLASH_IBUS_BASE, ROM_BASE, SRAM_DBUS_BASE};
    use soc_esp32c3::test_support::flat_rom;

    fn watch(address: u32, length: u32) -> Watch {
        Watch {
            address,
            length,
            on: WatchOn::Write,
        }
    }

    #[test]
    fn a_breakpoint_goes_on_an_even_address_of_memory() {
        let mut points = Points::default();
        assert_eq!(
            points.set_breakpoint(ROM_BASE + 1),
            Err(PointError::OddAddress {
                address: ROM_BASE + 1
            })
        );
        assert_eq!(
            points.set_breakpoint(0x6000_0000),
            Err(PointError::NoCode {
                address: 0x6000_0000,
                region: Region::Registers
            })
        );
        assert!(matches!(
            points.set_breakpoint(0x10),
            Err(PointError::NoCode { .. })
        ));
        points.set_breakpoint(FLASH_IBUS_BASE + 0x20).unwrap();
        points.set_breakpoint(ROM_BASE + 2).unwrap();
        points.set_breakpoint(ROM_BASE + 2).unwrap();
        assert_eq!(
            points.breakpoints().collect::<Vec<_>>(),
            [ROM_BASE + 2, FLASH_IBUS_BASE + 0x20],
            "in increasing order, without repetition"
        );
        points.clear_breakpoint(ROM_BASE + 2);
        points.clear_breakpoint(ROM_BASE + 2);
        assert!(!points.has_breakpoint(ROM_BASE + 2));
    }

    #[test]
    fn breakpoints_are_bounded_and_one_set_already_is_no_new_one() {
        let mut points = Points::default();
        for index in 0..MAX_BREAKPOINTS {
            points
                .set_breakpoint(ROM_BASE + 4 * u32::try_from(index).unwrap())
                .unwrap();
        }
        assert_eq!(
            points.set_breakpoint(SRAM_DBUS_BASE),
            Err(PointError::TooManyBreakpoints)
        );
        assert_eq!(points.set_breakpoint(ROM_BASE), Ok(()), "set already");
        let message = PointError::TooManyBreakpoints.to_string();
        assert!(
            message.contains("64") && message.contains("clear one"),
            "{message}"
        );
    }

    #[test]
    fn a_watchpoint_covers_memory_or_registers_and_never_the_flash() {
        let mut points = Points::default();
        assert_eq!(
            points.set_watchpoint(watch(SRAM_DBUS_BASE, 0)),
            Err(PointError::WatchLength { length: 0 })
        );
        assert_eq!(
            points.set_watchpoint(watch(SRAM_DBUS_BASE, MAX_WATCH_LENGTH + 1)),
            Err(PointError::WatchLength { length: 4097 })
        );
        assert_eq!(
            points.set_watchpoint(watch(u32::MAX, 2)),
            Err(PointError::PastTheEnd { address: u32::MAX })
        );
        assert_eq!(
            points.set_watchpoint(watch(FLASH_DBUS_BASE + 8, 4)),
            Err(PointError::FlashWindow)
        );
        assert_eq!(
            points.set_watchpoint(watch(FLASH_DBUS_BASE - 2, 4)),
            Err(PointError::FlashWindow),
            "a range that reaches into a window"
        );
        assert_eq!(
            points.set_watchpoint(watch(0x100, 4)),
            Err(PointError::Unmapped { address: 0x100 })
        );
        points.set_watchpoint(watch(SRAM_DBUS_BASE, 4)).unwrap();
        points.set_watchpoint(watch(0x6000_0000, 4)).unwrap();
        points.set_watchpoint(watch(SRAM_DBUS_BASE, 4)).unwrap();
        assert_eq!(points.watchpoints().len(), 2, "without repetition");
        points.clear_watchpoint(watch(SRAM_DBUS_BASE, 4));
        assert_eq!(points.watchpoints(), [watch(0x6000_0000, 4)]);
        assert!(
            PointError::FlashWindow
                .to_string()
                .contains("personal data"),
            "the message says why"
        );
    }

    #[test]
    fn watchpoints_are_bounded() {
        let mut points = Points::default();
        for index in 0..MAX_WATCHPOINTS {
            points
                .set_watchpoint(watch(SRAM_DBUS_BASE + 4 * u32::try_from(index).unwrap(), 4))
                .unwrap();
        }
        assert_eq!(
            points.set_watchpoint(watch(SRAM_DBUS_BASE + 0x1000, 4)),
            Err(PointError::TooManyWatchpoints)
        );
    }

    #[test]
    fn the_points_reach_the_machine() {
        let mut machine = Esp32C3::new(flat_rom(&[0x0000_006f]), &SocConfig::default());
        let mut points = Points::default();
        points.set_breakpoint(ROM_BASE).unwrap();
        points.apply(&mut machine);
        assert_eq!(
            machine.run_stopping(emu_core::Cycles(1_000), None),
            soc_esp32c3::RunStop::Debug(soc_esp32c3::DebugStop::Breakpoint { pc: ROM_BASE })
        );
    }

    #[test]
    fn the_points_an_interface_sends_are_checked_as_when_set_alone() {
        let request: PointsRequest = serde_json::from_value(serde_json::json!({
            "breakpoints": [ROM_BASE, ROM_BASE + 4],
            "watchpoints": [{ "address": SRAM_DBUS_BASE, "length": 4, "on": "access" }]
        }))
        .unwrap();
        let points = Points::from_request(&request).unwrap();
        assert_eq!(points.breakpoints().count(), 2);
        assert_eq!(
            points.watchpoints(),
            [Watch {
                address: SRAM_DBUS_BASE,
                length: 4,
                on: WatchOn::Access
            }]
        );
        let odd = PointsRequest {
            breakpoints: vec![ROM_BASE + 1],
            watchpoints: Vec::new(),
        };
        assert!(matches!(
            Points::from_request(&odd),
            Err(PointError::OddAddress { .. })
        ));
        let unknown = PointsRequest {
            breakpoints: Vec::new(),
            watchpoints: vec![WatchRequest {
                address: SRAM_DBUS_BASE,
                length: 4,
                on: "execute".to_owned(),
            }],
        };
        assert_eq!(
            Points::from_request(&unknown),
            Err(PointError::UnknownAccess {
                name: "execute".to_owned()
            })
        );
        assert_eq!(
            Points::from_request(&PointsRequest::default()),
            Ok(Points::default())
        );
    }

    #[test]
    fn the_names_of_the_accesses_go_both_ways() {
        for on in [WatchOn::Read, WatchOn::Write, WatchOn::Access] {
            assert_eq!(watch_on(watch_on_name(on)), Some(on));
        }
        assert_eq!(watch_on("execute"), None);
    }
}
