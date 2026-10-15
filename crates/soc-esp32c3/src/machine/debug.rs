//! The machine as a debugger drives it (design `SP4a`, ADR 0016): breakpoints and watchpoints
//! that end a call of [`Esp32C3::run_stopping`] without changing what the machine executes, and
//! what a debugger reads of it without effect.
use super::Esp32C3;
use crate::debug::{DebugStop, Peeked, RunStop, Watch, WatchHit};
use cpu_rv32::Privilege;
use emu_core::Cycles;
use std::convert::Infallible;

/// What may end the loop of a run besides the machine's own reasons: nothing for
/// [`Esp32C3::run_within`], the points of a debugger for [`Esp32C3::run_stopping`]. Each hook is
/// called at an instruction boundary and only reads the machine, or takes what the bus noted.
pub(super) trait Stops {
    type Hit;
    /// At the start of a call.
    fn begin(machine: &mut Esp32C3) -> Result<(), Self::Hit>;
    /// Before a turn.
    fn before(machine: &mut Esp32C3) -> Result<(), Self::Hit>;
    /// After a turn that gave no reason to stop.
    fn after(machine: &mut Esp32C3) -> Result<(), Self::Hit>;
}

/// A run without a debugger: an access a watchpoint noted is forgotten, nothing stops.
pub(super) struct NoStops;

impl Stops for NoStops {
    type Hit = Infallible;

    #[inline]
    fn begin(machine: &mut Esp32C3) -> Result<(), Infallible> {
        machine.bus.take_watch_hit();
        Ok(())
    }

    #[inline]
    fn before(_: &mut Esp32C3) -> Result<(), Infallible> {
        Ok(())
    }

    #[inline]
    fn after(_: &mut Esp32C3) -> Result<(), Infallible> {
        Ok(())
    }
}

/// A run with the points of a debugger.
pub(super) struct DebugStops;

impl Stops for DebugStops {
    type Hit = DebugStop;

    /// An access noted before the call is reported first: the turn that made it ended the last
    /// call for a reason of the machine (a strict stop, a console output), which took precedence,
    /// and no instruction has executed since.
    fn begin(machine: &mut Esp32C3) -> Result<(), DebugStop> {
        machine
            .bus
            .take_watch_hit()
            .map_or(Ok(()), |hit| Err(DebugStop::Watchpoint(hit)))
    }

    /// A breakpoint stops the machine before its instruction, when the CPU is about to execute
    /// it: not while the CPU waits in `wfi` (its `pc` is then the instruction after the `wfi`,
    /// reached when an interrupt returns there), nor in a deep sleep or once the supply failed;
    /// and not at the boundary the last call stopped at, which the machine leaves first.
    #[inline]
    fn before(machine: &mut Esp32C3) -> Result<(), DebugStop> {
        if machine.breakpoints.is_empty() {
            return Ok(());
        }
        let pc = machine.hart.pc;
        let executing = !machine.powered_down
            && !machine.hart.is_waiting()
            && machine.bus.periph.io.power_lost().is_none();
        if executing
            && machine.breakpoint_left != Some(pc)
            && machine.breakpoints.binary_search(&pc).is_ok()
        {
            machine.breakpoint_left = Some(pc);
            return Err(DebugStop::Breakpoint { pc });
        }
        Ok(())
    }

    #[inline]
    fn after(machine: &mut Esp32C3) -> Result<(), DebugStop> {
        machine
            .bus
            .take_watch_hit()
            .map_or(Ok(()), |hit| Err(DebugStop::Watchpoint(hit)))
    }
}

impl Esp32C3 {
    /// Stops [`Esp32C3::run_stopping`] before the instruction at each of `pcs` from now on, in
    /// place of the breakpoints before. A breakpoint never changes what the machine executes.
    pub fn set_breakpoints(&mut self, pcs: &[u32]) {
        let mut sorted = pcs.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        self.breakpoints = sorted;
    }

    /// Stops [`Esp32C3::run_stopping`] after an instruction whose load or store touches the bytes
    /// `watches` cover, from now on, in place of the watchpoints before. A watchpoint is a note
    /// the bus takes of an access the CPU makes anyway: it never changes what the machine
    /// executes.
    pub fn set_watchpoints(&mut self, watches: &[Watch]) {
        self.bus.set_watches(watches);
    }

    /// [`Esp32C3::run_within`], stopped also by the points of a debugger
    /// ([`Esp32C3::set_breakpoints`], [`Esp32C3::set_watchpoints`]), at an instruction boundary:
    ///
    /// - [`DebugStop::Breakpoint`] before the instruction at a breakpoint, which is not executed;
    ///   the next call goes on past it (a breakpoint the machine comes back to stops it again);
    /// - [`DebugStop::Watchpoint`] after the instruction whose access touched watched bytes,
    ///   once the machine is at the boundary of the next one.
    ///
    /// Stopping changes nothing of the run: calls that go on after each stop execute exactly
    /// what one call without points would have (same instructions, same cycles, same device
    /// time, same trace hash). A reason of the machine and a watchpoint met on the same turn give
    /// the machine's reason; the watchpoint ends the next call, before anything is executed.
    pub fn run_stopping(&mut self, cycles: Cycles, device_time_nanos: Option<u64>) -> RunStop {
        match self.run_loop::<DebugStops>(cycles, device_time_nanos) {
            Ok(reason) => RunStop::Machine(reason),
            Err(hit) => RunStop::Debug(hit),
        }
    }

    /// The access to watched bytes the last [`Esp32C3::step`] made, if it made one (a step does
    /// not stop on a watchpoint: it executes one instruction anyway).
    pub fn take_watch_hit(&mut self) -> Option<WatchHit> {
        self.bus.take_watch_hit()
    }

    /// The `length` bytes from `address` as they are, without the effects of a load: the mask
    /// ROM and the RAMs as bytes, a flash window through its MMU entries, a peripheral register
    /// not read (a read may change it). Nothing is reported and nothing changes.
    #[must_use]
    pub fn peek(&self, address: u32, length: u32) -> Vec<Peeked> {
        (0..length)
            .map(|index| {
                address
                    .checked_add(index)
                    .map_or(Peeked::Unmapped, |at| self.bus.peek_byte(at))
            })
            .collect()
    }

    /// The mode the CPU executes in.
    #[must_use]
    pub const fn privilege(&self) -> Privilege {
        self.hart.privilege()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StopReason;
    use crate::config::SocConfig;
    use crate::debug::{Access, WatchOn};
    use crate::memmap::{ROM_BASE, SRAM_DBUS_BASE};
    use crate::test_support::{HANG, T0, T1, T2, flat_rom, stop_watchdogs, store_register};
    use cpu_rv32::asm::{addi, jal, lui, lw, sw, wfi};

    /// The address of word `index` of a program the flat ROM holds.
    fn at(index: usize) -> u32 {
        ROM_BASE + 4 * u32::try_from(index).unwrap()
    }

    /// A counter in the SRAM, incremented, stored and loaded back for ever. Returns the program
    /// and the index of its increment, its store and its load.
    fn counting() -> (Vec<u32>, [usize; 3]) {
        let mut program = stop_watchdogs();
        program.extend([lui(T0, SRAM_DBUS_BASE), addi(T1, 0, 0)]);
        let increment = program.len();
        program.extend([addi(T1, T1, 1), sw(T1, T0, 0), lw(T2, T0, 0), jal(0, -12)]);
        (program, [increment, increment + 1, increment + 2])
    }

    fn machine(program: &[u32], config: &SocConfig) -> Esp32C3 {
        Esp32C3::new(flat_rom(program), config)
    }

    fn word(machine: &Esp32C3, address: u32) -> u32 {
        let bytes: Vec<u8> = machine
            .peek(address, 4)
            .into_iter()
            .map(|peeked| match peeked {
                Peeked::Byte(byte) => byte,
                other => panic!("{other:?}"),
            })
            .collect();
        u32::from_le_bytes(bytes.try_into().unwrap())
    }

    const FAR: Cycles = Cycles(u64::MAX);

    #[test]
    fn a_breakpoint_stops_before_its_instruction_and_the_next_call_goes_past_it() {
        let (program, [_, store, _]) = counting();
        let mut m = machine(&program, &SocConfig::default());
        m.set_breakpoints(&[at(store)]);
        for count in 1..=3 {
            assert_eq!(
                m.run_stopping(FAR, None),
                RunStop::Debug(DebugStop::Breakpoint { pc: at(store) })
            );
            assert_eq!(m.pc(), at(store));
            assert_eq!(m.x()[usize::from(T1)], count, "the increment before it ran");
            assert_eq!(word(&m, SRAM_DBUS_BASE), count - 1, "the store has not run");
        }
    }

    #[test]
    fn a_breakpoint_at_the_first_instruction_stops_before_anything_runs() {
        let mut m = machine(&[HANG], &SocConfig::default());
        m.set_breakpoints(&[ROM_BASE]);
        assert_eq!(
            m.run_stopping(FAR, None),
            RunStop::Debug(DebugStop::Breakpoint { pc: ROM_BASE })
        );
        assert_eq!((m.instructions(), m.cycles()), (0, Cycles::ZERO));
    }

    #[test]
    fn a_watchpoint_stops_after_the_instruction_with_its_access() {
        let (program, [_, store, load]) = counting();
        let mut m = machine(&program, &SocConfig::default());
        m.set_watchpoints(&[Watch {
            address: SRAM_DBUS_BASE,
            length: 4,
            on: WatchOn::Write,
        }]);
        assert_eq!(
            m.run_stopping(FAR, None),
            RunStop::Debug(DebugStop::Watchpoint(WatchHit {
                address: SRAM_DBUS_BASE,
                size: 4,
                access: Access::Write,
                value: 1,
                pc: at(store),
            }))
        );
        assert_eq!(m.pc(), at(load), "at the boundary of the next instruction");
        assert_eq!(word(&m, SRAM_DBUS_BASE), 1, "the store has run");
    }

    /// The same run, through calls that stop at every point and go on, and in one call: the
    /// points change nothing of what the machine executes.
    #[test]
    fn stopping_at_points_changes_nothing_of_the_run() {
        let (program, [increment, store, _]) = counting();
        let config = SocConfig {
            trace_hash: true,
            ..SocConfig::default()
        };
        let end = Cycles(20_000);
        let mut plain = machine(&program, &config);
        assert_eq!(plain.run_within(end, None), StopReason::CycleBudget);

        let mut stopped = machine(&program, &config);
        stopped.set_breakpoints(&[at(store), at(increment), at(store)]);
        stopped.set_watchpoints(&[Watch {
            address: SRAM_DBUS_BASE,
            length: 4,
            on: WatchOn::Access,
        }]);
        let mut stops = 0u32;
        loop {
            match stopped.run_stopping(end, None) {
                RunStop::Debug(_) => stops += 1,
                RunStop::Machine(reason) => {
                    assert_eq!(reason, StopReason::CycleBudget);
                    break;
                }
            }
        }
        assert!(stops > 1_000, "stopped {stops} times");
        assert_eq!(stopped.trace_hash(), plain.trace_hash());
        assert_eq!(stopped.cycles(), plain.cycles());
        assert_eq!(stopped.instructions(), plain.instructions());
        assert_eq!(stopped.device_time_nanos(), plain.device_time_nanos());
        assert_eq!(stopped.x(), plain.x());
        assert_eq!(stopped.pc(), plain.pc());
        assert_eq!(word(&stopped, SRAM_DBUS_BASE), word(&plain, SRAM_DBUS_BASE));
    }

    #[test]
    fn a_breakpoint_after_a_wfi_does_not_stop_a_cpu_that_waits() {
        let mut program = stop_watchdogs();
        program.extend([wfi(), HANG]);
        let mut m = machine(&program, &SocConfig::default());
        m.set_breakpoints(&[at(program.len() - 1)]);
        assert_eq!(
            m.run_stopping(FAR, None),
            RunStop::Machine(StopReason::Idle)
        );
        assert!(m.is_asleep());
    }

    #[test]
    fn a_watchpoint_met_with_a_strict_stop_ends_the_next_call_before_anything_runs() {
        let register = crate::test_support::unmodeled_register().addr;
        let mut program = stop_watchdogs();
        program.extend(store_register(register, 7));
        program.push(HANG);
        let config = SocConfig {
            strict_mmio: true,
            ..SocConfig::default()
        };
        let mut m = machine(&program, &config);
        m.set_watchpoints(&[Watch {
            address: register,
            length: 4,
            on: WatchOn::Write,
        }]);
        assert!(matches!(
            m.run_stopping(FAR, None),
            RunStop::Machine(StopReason::StrictMmio(_))
        ));
        let instructions = m.instructions();
        assert!(matches!(
            m.run_stopping(FAR, None),
            RunStop::Debug(DebugStop::Watchpoint(WatchHit { value: 7, .. }))
        ));
        assert_eq!(m.instructions(), instructions, "nothing ran");
    }

    #[test]
    fn a_step_gives_its_own_access_only() {
        let (program, [_, store, _]) = counting();
        let mut m = machine(&program, &SocConfig::default());
        m.set_watchpoints(&[Watch {
            address: SRAM_DBUS_BASE,
            length: 4,
            on: WatchOn::Access,
        }]);
        // A run without stops passes stores and loads: what it noted is not a step's.
        m.run_within(Cycles(4_000), None);
        while m.pc() != at(store) {
            m.step();
        }
        // The last step executed the increment, which reaches no memory.
        assert_eq!(m.take_watch_hit(), None);
        m.step();
        let hit = m.take_watch_hit().unwrap();
        assert_eq!((hit.access, hit.pc), (Access::Write, at(store)));
        assert_eq!(m.take_watch_hit(), None);
    }

    #[test]
    fn the_cpu_starts_in_machine_mode() {
        let m = machine(&[HANG], &SocConfig::default());
        assert_eq!(m.privilege(), Privilege::Machine);
    }
}
