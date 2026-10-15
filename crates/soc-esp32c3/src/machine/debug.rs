//! The machine as a debugger drives it (design `SP4a`, ADR 0016): breakpoints and watchpoints
//! that end a call of [`Esp32C3::run_stopping`] without changing what the machine executes, and
//! what a debugger reads of it without effect.
use super::{Esp32C3, Stepped};
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
    /// it ([`cpu_rv32::Hart::executes_next`]): not while the CPU waits in `wfi` (its `pc` is then
    /// the instruction after the `wfi`), nor when an interrupt is taken first (the machine stops
    /// when the handler returns there), nor in a deep sleep or once the supply failed; and not
    /// at the boundary the last call stopped at or the last step reached, which the machine
    /// leaves first, as a debugger steps over the breakpoint it resumes from.
    #[inline]
    fn before(machine: &mut Esp32C3) -> Result<(), DebugStop> {
        if machine.breakpoints.is_empty() {
            return Ok(());
        }
        let pc = machine.hart.pc;
        let executing = !machine.powered_down
            && machine.hart.executes_next()
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

    /// [`Esp32C3::step`] for a debugger: an access to watched bytes noted and not reported yet
    /// is reported first, as [`Esp32C3::run_stopping`] would report it, and nothing executes;
    /// otherwise one step. The turn that made the access ended the last call for a reason of the
    /// machine, which took precedence; [`Esp32C3::step`] alone would forget the access.
    ///
    /// # Errors
    /// [`DebugStop::Watchpoint`] with the access not reported yet.
    pub fn step_stopping(&mut self) -> Result<Stepped, DebugStop> {
        if let Some(hit) = self.bus.take_watch_hit() {
            return Err(DebugStop::Watchpoint(hit));
        }
        Ok(self.step())
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
    use crate::config::{SocConfig, Timing};
    use crate::debug::{Access, WatchOn};
    use crate::memmap::{ROM_BASE, SRAM_DBUS_BASE};
    use crate::test_support::{HANG, T0, T1, T2, flat_rom, stop_watchdogs, store_register};
    use cpu_rv32::asm::{addi, bne, csrrci, csrrsi, csrrw, jal, lui, lw, mret, sw, wfi};

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

    /// A step that ends on a breakpoint leaves it, as a stop at it does: the run that follows
    /// executes it and stops there the next time round.
    #[test]
    fn a_run_after_a_step_onto_a_breakpoint_goes_past_it() {
        let (program, [_, store, _]) = counting();
        let mut m = machine(&program, &SocConfig::default());
        m.set_breakpoints(&[at(store)]);
        while m.pc() != at(store) {
            m.step();
        }
        assert_eq!(m.x()[usize::from(T1)], 1);
        assert_eq!(
            m.run_stopping(FAR, None),
            RunStop::Debug(DebugStop::Breakpoint { pc: at(store) })
        );
        assert_eq!(m.x()[usize::from(T1)], 2, "one turn of the loop, not none");
        assert_eq!(word(&m, SRAM_DBUS_BASE), 1);
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

    /// Where the interrupting program puts its vector table, and how its parts are laid out.
    const VECTORS: u32 = ROM_BASE + 0x1000;
    /// The CPU line the SYSTIMER alarm is routed to: its vector is entry 4 of the table.
    const TIMER_LINE: u32 = 4;
    /// The handler, after the 32 entries of the table.
    const HANDLER: u32 = VECTORS + 4 * 32;
    const MSTATUS: u16 = 0x300;
    const MTVEC: u16 = 0x305;
    const MEPC: u16 = 0x341;
    const MSTATUS_MIE: u8 = 1 << 3;
    /// The alarm's period, in counts of the 16 MHz SYSTIMER: 25 µs, about 500 CPU cycles.
    const ALARM_PERIOD: u32 = 400;
    /// The passes of the loop between two `wfi`s.
    const PASSES: i32 = 16;
    const S1: u8 = 9;
    const A3: u8 = 13;
    const A4: u8 = 14;
    const A5: u8 = 15;

    fn reg(peripheral: &str, register: &str) -> u32 {
        svd_regs::esp32c3::addr(peripheral, register)
    }

    /// `sw` of the word in `value` to `addr`, through `base`: `addr` split as `lui` and a signed
    /// 12-bit offset, so the handler keeps off the registers the loop uses.
    fn store_through(base: u8, value: u8, addr: u32) -> [u32; 2] {
        let page = addr.wrapping_add(0x800) & 0xFFFF_F000;
        [
            lui(base, page),
            sw(value, base, addr.wrapping_sub(page).cast_signed()),
        ]
    }

    /// A periodic SYSTIMER alarm on line 4, taken while a loop counts in the first word of the
    /// SRAM; every 16 passes the loop masks interrupts and waits in `wfi`, which the masked line
    /// ends, then unmasks them and the interrupt is taken at once. The handler clears the alarm
    /// and counts itself in the second word. Returns the program, the addresses of the loop's
    /// instructions, and the instruction after the `wfi`'s unmasking.
    fn interrupting() -> (Vec<u32>, Vec<u32>, u32) {
        let mut body = stop_watchdogs();
        body.extend([lui(T0, VECTORS), csrrw(0, MTVEC, T0)]);
        body.extend(store_register(
            reg("INTERRUPT_CORE0", "SYSTIMER_TARGET0_INT_MAP"),
            TIMER_LINE,
        ));
        body.extend(store_register(
            reg("INTERRUPT_CORE0", "CPU_INT_PRI_0") + 4 * TIMER_LINE,
            1,
        ));
        body.extend(store_register(
            reg("INTERRUPT_CORE0", "CPU_INT_ENABLE"),
            1 << TIMER_LINE,
        ));
        body.extend(store_register(reg("SYSTIMER", "INT_ENA"), 1));
        // `TARGET0_CONF`: bit 30 `PERIOD_MODE`, and the period.
        body.extend(store_register(
            reg("SYSTIMER", "TARGET0_CONF"),
            (1 << 30) | ALARM_PERIOD,
        ));
        body.extend(store_register(reg("SYSTIMER", "COMP0_LOAD"), 1));
        // `CONF`: its reset value with `TARGET0_WORK_EN`, bit 24.
        body.extend(store_register(
            reg("SYSTIMER", "CONF"),
            0x4600_0000 | (1 << 24),
        ));
        body.extend([
            csrrsi(0, MSTATUS, MSTATUS_MIE),
            lui(T0, SRAM_DBUS_BASE),
            addi(T1, 0, 0),
            addi(S1, 0, PASSES),
        ]);
        let first = body.len();
        body.extend([
            addi(T1, T1, 1),
            sw(T1, T0, 0),
            lw(T2, T0, 0),
            addi(S1, S1, -1),
            bne(S1, 0, -16),
            csrrci(0, MSTATUS, MSTATUS_MIE),
            wfi(),
            csrrsi(0, MSTATUS, MSTATUS_MIE),
            addi(S1, 0, PASSES),
            jal(0, -36),
        ]);
        let loop_at: Vec<u32> = (first..body.len()).map(at).collect();
        let after_unmasking = at(first + 8);
        let table = usize::try_from((VECTORS - ROM_BASE) / 4).unwrap();
        assert!(body.len() <= table, "the body overlaps the vector table");
        body.resize(table, HANG);
        let mut vectors = [HANG; 32];
        vectors[TIMER_LINE as usize] = jal(0, (HANDLER - (VECTORS + 4 * TIMER_LINE)).cast_signed());
        body.extend(vectors);
        body.push(addi(A4, 0, 1));
        body.extend(store_through(A5, A4, reg("SYSTIMER", "INT_CLR")));
        body.extend([
            lui(A3, SRAM_DBUS_BASE),
            lw(A4, A3, 4),
            addi(A4, A4, 1),
            sw(A4, A3, 4),
            mret(),
        ]);
        (body, loop_at, after_unmasking)
    }

    /// What two runs must agree on to be the same run.
    fn outcome(m: &Esp32C3) -> (Option<u64>, Cycles, u64, u64, [u32; 32], u32, [u32; 2]) {
        (
            m.trace_hash(),
            m.cycles(),
            m.instructions(),
            m.device_time_nanos(),
            m.x(),
            m.pc(),
            [word(m, SRAM_DBUS_BASE), word(m, SRAM_DBUS_BASE + 4)],
        )
    }

    /// A run with interrupts, `wfi`s a masked line ends, and a device-time limit, stopped at a
    /// breakpoint on the handler's entry, on every instruction of the loop (the interrupted one
    /// among them) and after the unmasking, and at two watchpoints, is the run made in calls
    /// without points, under both timing models: the points change nothing of what executes.
    #[test]
    fn stopping_with_interrupts_wfi_and_a_time_limit_changes_nothing_of_the_run() {
        let (program, loop_at, after_unmasking) = interrupting();
        let limit = Some(1_000_000);
        for timing in [Timing::Datasheet, Timing::QemuIcount] {
            let config = SocConfig {
                trace_hash: true,
                timing,
                ..SocConfig::default()
            };
            let mut plain = machine(&program, &config);
            let plain_end = loop {
                match plain.run_within(FAR, limit) {
                    StopReason::Asleep => {}
                    reason => break reason,
                }
            };
            assert_eq!(plain_end, StopReason::TimeBudget, "{timing:?}");

            let mut stopped = machine(&program, &config);
            let mut breakpoints = loop_at.clone();
            breakpoints.push(HANDLER);
            stopped.set_breakpoints(&breakpoints);
            stopped.set_watchpoints(&[
                Watch {
                    address: SRAM_DBUS_BASE,
                    length: 4,
                    on: WatchOn::Access,
                },
                Watch {
                    address: SRAM_DBUS_BASE + 4,
                    length: 4,
                    on: WatchOn::Write,
                },
            ]);
            let (mut handler_entries, mut watched) = (0u32, 0u32);
            let mut interrupted: Vec<u32> = Vec::new();
            let mut resumed_at_interrupted = 0u32;
            let mut after_wfi = 0u32;
            let stopped_end = loop {
                match stopped.run_stopping(FAR, limit) {
                    RunStop::Debug(DebugStop::Breakpoint { pc }) if pc == HANDLER => {
                        handler_entries += 1;
                        interrupted.push(stopped.csr(MEPC).unwrap());
                    }
                    RunStop::Debug(DebugStop::Breakpoint { pc }) => {
                        assert!(breakpoints.contains(&pc), "{pc:#x}");
                        resumed_at_interrupted += u32::from(interrupted.last() == Some(&pc));
                        after_wfi += u32::from(pc == after_unmasking);
                    }
                    RunStop::Debug(DebugStop::Watchpoint(_)) => watched += 1,
                    RunStop::Machine(StopReason::Asleep) => {}
                    RunStop::Machine(reason) => break reason,
                }
            };
            assert_eq!(stopped_end, plain_end, "{timing:?}");
            assert_eq!(outcome(&stopped), outcome(&plain), "{timing:?}");
            let handled = word(&plain, SRAM_DBUS_BASE + 4);
            assert!(handled > 10, "{timing:?}: {handled} interrupts");
            assert_eq!(handler_entries, handled, "{timing:?}: one stop per entry");
            assert!(
                interrupted.iter().all(|pc| loop_at.contains(pc)),
                "{timing:?}: {interrupted:x?}"
            );
            assert!(resumed_at_interrupted > 0, "{timing:?}");
            assert!(after_wfi > 0, "{timing:?}: the loop went past its wfi");
            assert!(watched > handled, "{timing:?}: {watched} accesses");
        }
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

    /// A watchpoint met on the turn of a strict stop is pending: a step for a debugger reports it
    /// first, executing nothing, then steps.
    #[test]
    fn a_step_for_a_debugger_reports_the_pending_access_before_it_executes() {
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
            m.step_stopping(),
            Err(DebugStop::Watchpoint(WatchHit { value: 7, .. }))
        ));
        assert_eq!(m.instructions(), instructions, "nothing ran");
        assert!(m.step_stopping().is_ok());
        assert_eq!(m.instructions(), instructions + 1);
    }

    #[test]
    fn the_cpu_starts_in_machine_mode() {
        let m = machine(&[HANG], &SocConfig::default());
        assert_eq!(m.privilege(), Privilege::Machine);
    }
}
