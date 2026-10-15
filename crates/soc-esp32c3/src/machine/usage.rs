//! The machine's counts for a resource monitor (design `SP4b`, ADR 0017): where device time went,
//! at which CPU frequency, and the interrupts raised and taken. Each count is touched where time
//! or the event passes already, never per instruction, and nothing the run does reads it.
use super::Esp32C3;
use crate::usage::{ClockUse, LINES, SOURCES, Usage};
use emu_core::Cycles;

/// How the CPU spends a sleep: waiting in `wfi` for an interrupt, or with the chip in deep sleep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Asleep {
    Waiting,
    Deeply,
}

/// What the machine counts, in cycles of the time base for time.
#[derive(Debug)]
pub(super) struct Counters {
    /// The awake time of each frequency used, up to the start of the current segment.
    clocks: Vec<ClockUse>,
    /// The place in `clocks` of the frequency in use.
    current: usize,
    /// The instant the frequency in use took effect.
    segment_start: Cycles,
    /// `deep` at that instant: the deep sleep inside the segment is not awake time.
    deep_at_segment_start: u64,
    /// Deep sleep since power-on.
    deep: u64,
    /// The source levels at the last sampling of the interrupt controller.
    sources: u64,
    raised: [u64; SOURCES],
    taken: [u64; LINES],
    /// The write version of each RAM at the last reset: the SRAM, the SRAM0, the RTC memory.
    memories_at_reset: [u64; 3],
}

impl Counters {
    /// Nothing counted yet, the CPU at `hz` since instant 0; the RAMs at `memories`.
    pub(super) fn new(hz: u64, memories: [u64; 3]) -> Counters {
        Counters {
            clocks: vec![ClockUse {
                hz,
                awake: Cycles::ZERO,
                waiting: Cycles::ZERO,
            }],
            current: 0,
            segment_start: Cycles::ZERO,
            deep_at_segment_start: 0,
            deep: 0,
            sources: 0,
            raised: [0; SOURCES],
            taken: [0; LINES],
            memories_at_reset: memories,
        }
    }

    /// The CPU slept `duration` cycles of the time base, `how`.
    fn slept(&mut self, duration: u64, how: Asleep) {
        match how {
            Asleep::Deeply => self.deep += duration,
            Asleep::Waiting => {
                if let Some(clock) = self.clocks.get_mut(self.current) {
                    clock.waiting = clock.waiting + duration;
                }
            }
        }
    }

    /// The awake time of the segment that began at `segment_start`, up to `now`.
    fn segment(&self, now: Cycles) -> u64 {
        let elapsed = now.get().saturating_sub(self.segment_start.get());
        elapsed.saturating_sub(self.deep - self.deep_at_segment_start)
    }

    /// From `now` on, the CPU runs at `hz`.
    pub(super) fn clock_changed(&mut self, now: Cycles, hz: u64) {
        let awake = self.segment(now);
        if let Some(clock) = self.clocks.get_mut(self.current) {
            clock.awake = clock.awake + awake;
        }
        self.current = self
            .clocks
            .iter()
            .position(|clock| clock.hz == hz)
            .unwrap_or_else(|| {
                self.clocks.push(ClockUse {
                    hz,
                    awake: Cycles::ZERO,
                    waiting: Cycles::ZERO,
                });
                self.clocks.len() - 1
            });
        self.segment_start = now;
        self.deep_at_segment_start = self.deep;
    }

    /// The CPU took an interrupt on `line`.
    pub(super) fn interrupt_taken(&mut self, line: u32) {
        if let Some(count) = usize::try_from(line)
            .ok()
            .and_then(|line| self.taken.get_mut(line))
        {
            *count += 1;
        }
    }

    /// The interrupt controller samples the levels of the sources: each that rose since its
    /// last sampling counts once.
    pub(super) fn sources_sampled(&mut self, levels: u64) {
        let mut rising = levels & !self.sources;
        self.sources = levels;
        while rising != 0 {
            let source = rising.trailing_zeros();
            rising &= rising - 1;
            if let Some(count) = usize::try_from(source)
                .ok()
                .and_then(|source| self.raised.get_mut(source))
            {
                *count += 1;
            }
        }
    }

    /// A reset: the RAM writes since it start at the RAMs' versions `memories`.
    pub(super) fn reset(&mut self, memories: [u64; 3]) {
        self.memories_at_reset = memories;
    }

    /// The time of each frequency up to `now`, the current segment included.
    fn clocks_at(&self, now: Cycles) -> Vec<ClockUse> {
        let mut clocks = self.clocks.clone();
        let awake = self.segment(now);
        if let Some(clock) = clocks.get_mut(self.current) {
            clock.awake = clock.awake + awake;
        }
        clocks
    }
}

impl Esp32C3 {
    /// Device time jumps to `instant` while the CPU sleeps `how` (a `wfi`, or a deep sleep), and
    /// the time it moved by is counted.
    pub(super) fn sleep_to(&mut self, instant: Cycles, how: Asleep) {
        let before = self.bus.ctx.now();
        self.bus.ctx.sleep_until(instant);
        let after = self.bus.ctx.now();
        self.counters.slept(after.get() - before.get(), how);
    }

    /// What the machine counted of its use since power-on, for a resource monitor (design
    /// `SP4b`): the time awake at each CPU frequency and the part of it spent waiting in `wfi`,
    /// the time in deep sleep, the interrupts raised by each source and taken on each line, the
    /// register accesses to each block of the peripheral window, and the write versions of the
    /// RAM pages. Reading it changes nothing, and the counts do not depend on the calls a run is
    /// cut into.
    #[must_use]
    pub fn usage(&self) -> Usage {
        let now = self.bus.ctx.now();
        Usage {
            device_time: now,
            cycles: self.hart.cycles.get(),
            instructions: self.hart.instret,
            cpu_hz: self.cpu_frequency().hz(),
            resets: self.resets,
            deep_sleep: Cycles(self.counters.deep),
            clocks: self.counters.clocks_at(now),
            raised: self.counters.raised.to_vec(),
            taken: self.counters.taken.to_vec(),
            blocks: self.bus.block_uses(),
            memories: self.bus.memory_uses(self.counters.memories_at_reset),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StopReason;
    use crate::config::{SocConfig, Timing};
    use crate::ctx::Source;
    use crate::machine::test_programs::{S2, TIMER_LINE, interrupting, reg};
    use crate::memmap::{PERIPH_BASE, SRAM_DBUS_BASE};
    use crate::reset::ResetCause;
    use crate::test_support::{
        HANG, T0, T1, TIMER_WAKE_STATE, flat_rom, load_register, sleep_on_the_rtc_timer,
        stop_watchdogs, store_register,
    };
    use crate::usage::{BLOCK_BYTES, BlockUse};
    use cpu_rv32::asm::{addi, bne, lui, sw};
    use emu_core::{AccessSize, Bus};

    const FAR: Cycles = Cycles(u64::MAX);

    fn machine(program: &[u32], config: &SocConfig) -> Esp32C3 {
        Esp32C3::new(flat_rom(program), config)
    }

    /// The awake time of every frequency and the deep sleep make up device time.
    fn assert_time_adds_up(usage: &Usage) {
        let awake: u64 = usage.clocks.iter().map(|clock| clock.awake.get()).sum();
        assert_eq!(awake + usage.deep_sleep.get(), usage.device_time.get());
        assert!(
            usage
                .clocks
                .iter()
                .all(|clock| clock.waiting.get() <= clock.awake.get())
        );
    }

    /// Runs `m` until device time reaches `nanos`, through the bounds of a sleep.
    fn run_for(m: &mut Esp32C3, nanos: u64) -> StopReason {
        loop {
            match m.run_within(FAR, Some(nanos)) {
                StopReason::Asleep => {}
                reason => return reason,
            }
        }
    }

    #[test]
    fn a_new_machine_has_counted_nothing_at_its_power_on_clock() {
        let m = machine(&[HANG], &SocConfig::default());
        let usage = m.usage();
        assert_eq!(usage.device_time, Cycles::ZERO);
        assert_eq!(
            usage.clocks,
            [ClockUse {
                hz: 20_000_000,
                awake: Cycles::ZERO,
                waiting: Cycles::ZERO
            }]
        );
        assert!(usage.raised.iter().chain(&usage.taken).all(|n| *n == 0));
        assert_eq!(usage.raised.len(), 62);
        assert_eq!(usage.taken.len(), 32);
        assert_eq!(usage.blocks, Vec::new());
        assert_eq!(usage.memories.len(), 3);
    }

    #[test]
    fn time_waiting_in_wfi_and_time_executing_make_up_the_awake_time() {
        let (program, _, _) = interrupting();
        let mut m = machine(&program, &SocConfig::default());
        assert_eq!(run_for(&mut m, 1_000_000), StopReason::TimeBudget);
        let usage = m.usage();
        assert_time_adds_up(&usage);
        assert_eq!(usage.deep_sleep, Cycles::ZERO);
        let [clock] = usage.clocks.as_slice() else {
            panic!("{:?}", usage.clocks)
        };
        assert!(clock.waiting > Cycles::ZERO, "the loop waits in wfi");
        assert!(clock.waiting < clock.awake, "and executes");
    }

    #[test]
    fn interrupts_are_counted_raised_by_their_source_and_taken_on_their_line() {
        const ALARM: Source = Source::of("SYSTIMER_TARGET0_INT_MAP");
        let (program, _, _) = interrupting();
        let mut m = machine(&program, &SocConfig::default());
        run_for(&mut m, 1_000_000);
        let usage = m.usage();
        let handled = u64::from(m.x()[usize::from(S2)]);
        assert!(handled > 10, "{handled}");
        let source = ALARM.number() as usize;
        assert_eq!(usage.taken[TIMER_LINE as usize], handled);
        // Raised once per alarm: the handler clears it, the next alarm raises it again. The
        // last one may be raised and not taken yet.
        assert!(
            (handled..=handled + 1).contains(&usage.raised[source]),
            "{} raised, {handled} taken",
            usage.raised[source]
        );
        let others: u64 = usage
            .raised
            .iter()
            .enumerate()
            .filter(|(n, _)| *n != source)
            .map(|(_, count)| count)
            .sum();
        assert_eq!(others, 0);
        assert_eq!(usage.taken.iter().sum::<u64>(), handled);
    }

    #[test]
    fn register_accesses_are_counted_by_block_reads_and_writes_apart() {
        let systimer = reg("SYSTIMER", "UNIT0_VALUE_LO");
        let mut program = stop_watchdogs();
        // Three loads of a SYSTIMER register, then park.
        for _ in 0..3 {
            program.extend(load_register(systimer));
        }
        program.push(HANG);
        let mut m = machine(&program, &SocConfig::default());
        m.run_until(Cycles(10_000));
        let usage = m.usage();
        let block = |address: u32| {
            usage
                .blocks
                .iter()
                .find(|block| block.base == (address & !(BLOCK_BYTES - 1)))
                .copied()
        };
        assert_eq!(
            block(systimer),
            Some(BlockUse {
                base: systimer & !(BLOCK_BYTES - 1),
                reads: 3,
                writes: 0
            })
        );
        // `stop_watchdogs` writes the watchdogs' registers: those blocks hold writes.
        let writes: u64 = usage.blocks.iter().map(|block| block.writes).sum();
        assert!(writes > 0);
        assert!(usage.blocks.windows(2).all(|two| two[0].base < two[1].base));
        assert!(usage.blocks.iter().all(|block| block.base >= PERIPH_BASE));
    }

    #[test]
    fn ram_pages_are_written_before_or_since_the_last_reset() {
        let mut m = machine(&[HANG], &SocConfig::default());
        m.bus
            .write(SRAM_DBUS_BASE + 2 * 4096, AccessSize::Word, 1)
            .unwrap();
        m.reset(ResetCause::SoftwareCpu);
        m.bus
            .write(SRAM_DBUS_BASE + 5 * 4096 + 8, AccessSize::Byte, 1)
            .unwrap();
        let usage = m.usage();
        let sram = &usage.memories[0];
        assert_eq!(
            (sram.base, sram.size, sram.page_bytes),
            (SRAM_DBUS_BASE, 0x6_0000, 4096)
        );
        assert_eq!(sram.page_versions.len(), 96);
        let written: Vec<usize> = (0..96).filter(|&p| sram.page_versions[p] != 0).collect();
        assert_eq!(written, [2, 5]);
        let since: Vec<usize> = (0..96)
            .filter(|&p| sram.page_versions[p] >= sram.version_at_reset)
            .collect();
        assert_eq!(since, [5]);
        assert_eq!(sram.version, 3, "two writes since power-on");
    }

    #[test]
    fn a_deep_sleep_is_counted_apart_and_in_no_clock() {
        let mut program = stop_watchdogs();
        program.extend(sleep_on_the_rtc_timer(0x1000, TIMER_WAKE_STATE));
        let mut m = machine(&program, &SocConfig::default());
        assert_eq!(run_for(&mut m, 500_000_000), StopReason::TimeBudget);
        let usage = m.usage();
        assert!(usage.resets >= 2, "{} wakes", usage.resets);
        assert!(usage.deep_sleep > Cycles::ZERO);
        assert_time_adds_up(&usage);
        let awake: u64 = usage.clocks.iter().map(|clock| clock.awake.get()).sum();
        assert!(awake < usage.deep_sleep.get(), "the chip mostly sleeps");
    }

    #[test]
    fn time_is_counted_at_each_frequency_the_firmware_selects() {
        let sysclk = reg("SYSTEM", "SYSCLK_CONF");
        let per = reg("SYSTEM", "CPU_PER_CONF");
        let mut program = stop_watchdogs();
        program.extend(store_register(sysclk, 0x400)); // PLL, 80 MHz
        // A loop of 200 turns at 80 MHz, then 160 MHz.
        program.extend([addi(T1, 0, 200), addi(T1, T1, -1), bne(T1, 0, -4)]);
        program.extend(store_register(per, 0xD));
        program.push(HANG);
        let mut m = machine(&program, &SocConfig::default());
        m.run_until(Cycles(50_000));
        let usage = m.usage();
        let hz: Vec<u64> = usage.clocks.iter().map(|clock| clock.hz).collect();
        assert_eq!(hz, [20_000_000, 80_000_000, 160_000_000]);
        assert!(usage.clocks.iter().all(|clock| clock.awake > Cycles::ZERO));
        assert_eq!(usage.cpu_hz, 160_000_000);
        assert_time_adds_up(&usage);
    }

    /// What two runs must agree on to be the same run, and the machine's reading.
    fn outcome(m: &Esp32C3) -> (Option<u64>, Cycles, u64, u64, [u32; 32], u32, Usage) {
        (
            m.trace_hash(),
            m.cycles(),
            m.instructions(),
            m.device_time_nanos(),
            m.x(),
            m.pc(),
            m.usage(),
        )
    }

    /// A run with interrupts, `wfi`s and a deep sleep, read after every cut of a call, is the run
    /// made in one call, and its last reading is the same: observing changes nothing of what
    /// executes, and the counts do not depend on where a run is cut.
    #[test]
    fn reading_the_usage_after_every_cut_changes_nothing_of_the_run_nor_of_the_counts() {
        let (interrupting, _, _) = interrupting();
        // Each life writes the SRAM, then sleeps deeply until the RTC timer wakes it.
        let mut sleeping = stop_watchdogs();
        sleeping.extend([lui(T0, SRAM_DBUS_BASE), addi(T1, 0, 1), sw(T1, T0, 0)]);
        sleeping.extend(sleep_on_the_rtc_timer(0x1000, TIMER_WAKE_STATE));
        // A call is cut every `step` cycles: the sleeping program executes few instructions.
        for (program, limit, step) in [(interrupting, 2_000_000, 137), (sleeping, 300_000_000, 3)] {
            for timing in [Timing::Datasheet, Timing::QemuIcount] {
                let config = SocConfig {
                    trace_hash: true,
                    timing,
                    ..SocConfig::default()
                };
                let mut whole = machine(&program, &config);
                let whole_end = run_for(&mut whole, limit);

                let mut cut = machine(&program, &config);
                let mut readings = 0u32;
                let mut at = Cycles::ZERO;
                let cut_end = loop {
                    at = at + step;
                    let reason = cut.run_within(at, Some(limit));
                    let _ = cut.usage();
                    readings += 1;
                    match reason {
                        StopReason::CycleBudget | StopReason::Asleep => {}
                        reason => break reason,
                    }
                };
                assert!(readings > 10, "{timing:?}: {readings}");
                // Both programs sleep: one waits in `wfi`, the other sleeps deeply.
                let usage = whole.usage();
                let waiting: u64 = usage.clocks.iter().map(|clock| clock.waiting.get()).sum();
                assert!(waiting + usage.deep_sleep.get() > 0, "{timing:?}");
                assert_eq!(cut_end, whole_end, "{timing:?}");
                assert_eq!(outcome(&cut), outcome(&whole), "{timing:?}");
            }
        }
    }
}
