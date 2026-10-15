//! The view of a reading of the machine an interface shows (design `SP4b`, §4): device time and
//! where it went, the load, the RAM pages written, the interrupts and the register accesses by
//! name, the panel's refreshes and the datasheet estimate of the consumption.
//!
//! It shows counts, never content: no byte of memory or of the flash (ADR 0016 §6).
use crate::names::{block_peripherals, source_name};
use crate::power::{ClockTime, PowerView, estimate};
use emu_core::Cycles;
use serde::Serialize;
use soc_esp32c3::Usage;
use soc_esp32c3::clock::TIME_BASE;
use soc_esp32c3::memmap::{RTC_RAM_BASE, SRAM_DBUS_BASE, SRAM0_BASE};
use soc_esp32c3::usage::{ClockUse, MemoryUse};

/// What the host adds to a reading: what it measures itself.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HostSide {
    /// Device time over host time, measured over about a second while the device runs.
    pub speed_ratio: Option<f64>,
    /// The panel's refreshes in this life, once there was one.
    pub panel: Option<PanelUse>,
}

/// The refreshes of the panel in a life: how many, and the kind of the last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelUse {
    pub refreshes: u64,
    pub last: Refresh,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Refresh {
    Full,
    Partial,
}

/// What the monitor shows of a machine.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorView {
    pub device_time_nanos: u64,
    pub cycles: u64,
    pub instructions: u64,
    pub cpu_hz: u64,
    pub resets: u64,
    pub speed_ratio: Option<f64>,
    /// Where device time went since power-on.
    pub time: TimeView,
    /// The same since the reading before this one, with what happened meanwhile; `None` for the
    /// first reading of a life, or when device time has not moved since.
    pub recent: Option<RecentView>,
    pub clocks: Vec<ClockView>,
    pub memories: Vec<MemoryView>,
    /// The sources raised at least once.
    pub interrupts: Vec<SourceView>,
    /// The lines the CPU took an interrupt on.
    pub lines: Vec<LineView>,
    /// The blocks the CPU reached, in address order.
    pub peripherals: Vec<BlockView>,
    pub power: PowerView,
    pub panel: Option<PanelUse>,
}

/// Where device time went: executing, waiting in `wfi`, in deep sleep; and the load, the share of
/// the awake time spent executing, in thousandths (`None` with no awake time).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeView {
    pub running_nanos: u64,
    pub waiting_nanos: u64,
    pub deep_sleep_nanos: u64,
    pub load_permille: Option<u32>,
}

/// What happened between two readings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentView {
    pub device_nanos: u64,
    pub time: TimeView,
    pub instructions: u64,
    /// Interrupts raised, all sources.
    pub interrupts: u64,
    /// Register accesses, all blocks.
    pub register_accesses: u64,
}

/// The time the chip was awake at one CPU frequency.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClockView {
    pub hz: u64,
    pub running_nanos: u64,
    pub waiting_nanos: u64,
}

/// A RAM of the chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MemoryRegion {
    /// The 384 KiB SRAM, by its data-bus addresses.
    Sram,
    /// The 16 KiB SRAM0.
    Sram0,
    /// The 8 KiB RTC fast memory.
    RtcFast,
}

/// What became of a page of a RAM since power-on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PageState {
    Never,
    /// Written before the last reset, not since.
    Before,
    /// Written since the last reset (power-on included).
    Since,
}

/// The pages of a RAM the CPU wrote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryView {
    pub region: MemoryRegion,
    pub base: u32,
    pub size: u32,
    pub page_bytes: u32,
    pub pages: Vec<PageState>,
    /// The pages written since power-on, and since the last reset.
    pub written: u32,
    pub written_since_reset: u32,
    /// The first byte of the lowest page written, and the byte after the highest.
    pub lowest: Option<u32>,
    pub highest_end: Option<u32>,
    pub writes: u64,
    pub writes_since_reset: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceView {
    pub source: u32,
    pub name: Option<&'static str>,
    pub raised: u64,
    /// Since the reading before, when there is a window.
    pub recent: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineView {
    pub line: u32,
    pub taken: u64,
}

/// The register accesses of the CPU to a block of the peripheral window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockView {
    pub base: u32,
    /// The peripherals of the SVD with a register in the block.
    pub peripherals: Vec<&'static str>,
    pub reads: u64,
    pub writes: u64,
    /// Since the reading before, when there is a window.
    pub recent: Option<u64>,
}

fn nanos(cycles: u64) -> u64 {
    TIME_BASE.cycles_to_nanos(Cycles(cycles))
}

/// The awake and waiting time of all frequencies, in cycles of the time base.
fn awake_and_waiting(clocks: &[ClockUse]) -> (u64, u64) {
    clocks.iter().fold((0, 0), |(awake, waiting), clock| {
        (awake + clock.awake.get(), waiting + clock.waiting.get())
    })
}

/// The time view of `awake`, `waiting` and `deep` cycles of the time base.
fn time_view(awake: u64, waiting: u64, deep: u64) -> TimeView {
    let running = awake.saturating_sub(waiting);
    TimeView {
        running_nanos: nanos(running),
        waiting_nanos: nanos(waiting),
        deep_sleep_nanos: nanos(deep),
        load_permille: (awake > 0).then(|| {
            let permille = (u128::from(running) * 1000 + u128::from(awake) / 2) / u128::from(awake);
            u32::try_from(permille).unwrap_or(1000)
        }),
    }
}

fn region(base: u32) -> MemoryRegion {
    match base {
        SRAM0_BASE => MemoryRegion::Sram0,
        RTC_RAM_BASE => MemoryRegion::RtcFast,
        // The SRAM, the other one.
        _ => {
            debug_assert_eq!(base, SRAM_DBUS_BASE);
            MemoryRegion::Sram
        }
    }
}

fn memory_view(memory: &MemoryUse) -> MemoryView {
    let pages: Vec<PageState> = memory
        .page_versions
        .iter()
        .map(|&version| {
            if version == 0 {
                PageState::Never
            } else if version >= memory.version_at_reset {
                PageState::Since
            } else {
                PageState::Before
            }
        })
        .collect();
    let count = |wanted: fn(PageState) -> bool| {
        u32::try_from(pages.iter().filter(|page| wanted(**page)).count()).unwrap_or(u32::MAX)
    };
    let page_start = |index: usize| {
        u32::try_from(index)
            .ok()
            .and_then(|index| index.checked_mul(memory.page_bytes))
            .and_then(|offset| memory.base.checked_add(offset))
    };
    let first = pages.iter().position(|page| *page != PageState::Never);
    let last = pages.iter().rposition(|page| *page != PageState::Never);
    MemoryView {
        region: region(memory.base),
        base: memory.base,
        size: memory.size,
        page_bytes: memory.page_bytes,
        written: count(|page| page != PageState::Never),
        written_since_reset: count(|page| page == PageState::Since),
        lowest: first.and_then(page_start),
        highest_end: last.and_then(|last| page_start(last + 1)),
        writes: memory.version.saturating_sub(1),
        writes_since_reset: memory.version.saturating_sub(memory.version_at_reset),
        pages,
    }
}

/// Whether `before` is an earlier reading of the same run as `now`: device time moved on and no
/// count went back.
fn earlier(before: &Usage, now: &Usage) -> bool {
    before.device_time < now.device_time
        && before.instructions <= now.instructions
        && before.cycles <= now.cycles
        && before.resets <= now.resets
        && before.deep_sleep <= now.deep_sleep
}

fn recent_view(before: &Usage, now: &Usage) -> RecentView {
    let (awake_now, waiting_now) = awake_and_waiting(&now.clocks);
    let (awake_before, waiting_before) = awake_and_waiting(&before.clocks);
    let deep = now.deep_sleep.get() - before.deep_sleep.get();
    let raised = |usage: &Usage| usage.raised.iter().sum::<u64>();
    let accesses = |usage: &Usage| {
        usage
            .blocks
            .iter()
            .map(|block| block.reads + block.writes)
            .sum::<u64>()
    };
    RecentView {
        device_nanos: nanos(now.device_time.get() - before.device_time.get()),
        time: time_view(
            awake_now.saturating_sub(awake_before),
            waiting_now.saturating_sub(waiting_before),
            deep,
        ),
        instructions: now.instructions - before.instructions,
        interrupts: raised(now).saturating_sub(raised(before)),
        register_accesses: accesses(now).saturating_sub(accesses(before)),
    }
}

/// What the monitor shows of the reading `now`, with what happened since `before`, an earlier
/// reading of the same life (no window when it is not one), and what the host measured.
#[must_use]
pub fn view(now: &Usage, before: Option<&Usage>, host: HostSide) -> MonitorView {
    let before = before.filter(|before| earlier(before, now));
    let (awake, waiting) = awake_and_waiting(&now.clocks);
    let clocks: Vec<ClockView> = now
        .clocks
        .iter()
        .map(|clock| ClockView {
            hz: clock.hz,
            running_nanos: nanos(clock.awake.get().saturating_sub(clock.waiting.get())),
            waiting_nanos: nanos(clock.waiting.get()),
        })
        .collect();
    let times: Vec<ClockTime> = clocks
        .iter()
        .map(|clock| ClockTime {
            hz: clock.hz,
            running_nanos: clock.running_nanos,
            waiting_nanos: clock.waiting_nanos,
        })
        .collect();
    let interrupts = now
        .raised
        .iter()
        .zip(0u32..)
        .filter(|(raised, _)| **raised > 0)
        .map(|(&raised, source)| SourceView {
            source,
            name: source_name(source),
            raised,
            recent: before.map(|before| {
                let earlier = usize::try_from(source)
                    .ok()
                    .and_then(|source| before.raised.get(source))
                    .copied()
                    .unwrap_or(0);
                raised.saturating_sub(earlier)
            }),
        })
        .collect();
    let lines = now
        .taken
        .iter()
        .zip(0u32..)
        .filter(|(taken, _)| **taken > 0)
        .map(|(&taken, line)| LineView { line, taken })
        .collect();
    let peripherals = now
        .blocks
        .iter()
        .map(|block| BlockView {
            base: block.base,
            peripherals: block_peripherals(block.base),
            reads: block.reads,
            writes: block.writes,
            recent: before.map(|before| {
                let earlier = before
                    .blocks
                    .iter()
                    .find(|earlier| earlier.base == block.base)
                    .map_or(0, |earlier| earlier.reads + earlier.writes);
                (block.reads + block.writes).saturating_sub(earlier)
            }),
        })
        .collect();
    MonitorView {
        device_time_nanos: nanos(now.device_time.get()),
        cycles: now.cycles,
        instructions: now.instructions,
        cpu_hz: now.cpu_hz,
        resets: now.resets,
        speed_ratio: host.speed_ratio,
        time: time_view(awake, waiting, now.deep_sleep.get()),
        recent: before.map(|before| recent_view(before, now)),
        power: estimate(&times, nanos(now.deep_sleep.get())),
        clocks,
        memories: now.memories.iter().map(memory_view).collect(),
        interrupts,
        lines,
        peripherals,
        panel: host.panel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soc_esp32c3::usage::BlockUse;

    /// 160 time-base cycles: one microsecond.
    const MICROSECOND: u64 = 160;

    fn usage() -> Usage {
        Usage {
            device_time: Cycles(1_000 * MICROSECOND),
            cycles: 50_000,
            instructions: 40_000,
            cpu_hz: 160_000_000,
            resets: 1,
            deep_sleep: Cycles(200 * MICROSECOND),
            clocks: vec![
                ClockUse {
                    hz: 20_000_000,
                    awake: Cycles(100 * MICROSECOND),
                    waiting: Cycles::ZERO,
                },
                ClockUse {
                    hz: 160_000_000,
                    awake: Cycles(700 * MICROSECOND),
                    waiting: Cycles(300 * MICROSECOND),
                },
            ],
            raised: {
                let mut raised = vec![0; 62];
                raised[37] = 12;
                raised
            },
            taken: {
                let mut taken = vec![0; 32];
                taken[4] = 11;
                taken
            },
            blocks: vec![BlockUse {
                base: 0x6002_3000,
                reads: 30,
                writes: 12,
            }],
            memories: vec![MemoryUse {
                base: SRAM_DBUS_BASE,
                size: 4 * 4096,
                page_bytes: 4096,
                page_versions: vec![0, 3, 0, 9],
                version: 10,
                version_at_reset: 5,
            }],
        }
    }

    #[test]
    fn device_time_is_split_into_running_waiting_and_deep_sleep_with_the_load() {
        let view = view(&usage(), None, HostSide::default());
        assert_eq!(view.device_time_nanos, 1_000_000);
        assert_eq!(
            view.time,
            TimeView {
                running_nanos: 500_000,
                waiting_nanos: 300_000,
                deep_sleep_nanos: 200_000,
                load_permille: Some(625),
            }
        );
        assert_eq!(
            view.clocks,
            [
                ClockView {
                    hz: 20_000_000,
                    running_nanos: 100_000,
                    waiting_nanos: 0
                },
                ClockView {
                    hz: 160_000_000,
                    running_nanos: 400_000,
                    waiting_nanos: 300_000
                },
            ]
        );
        assert_eq!(view.recent, None);
    }

    #[test]
    fn no_awake_time_has_no_load() {
        let mut usage = usage();
        usage.clocks = vec![ClockUse {
            hz: 20_000_000,
            awake: Cycles::ZERO,
            waiting: Cycles::ZERO,
        }];
        assert_eq!(
            view(&usage, None, HostSide::default()).time.load_permille,
            None
        );
    }

    #[test]
    fn the_pages_of_a_ram_are_never_written_written_before_or_since_the_last_reset() {
        let view = view(&usage(), None, HostSide::default());
        let [sram] = view.memories.as_slice() else {
            panic!()
        };
        assert_eq!(sram.region, MemoryRegion::Sram);
        assert_eq!(
            sram.pages,
            [
                PageState::Never,
                PageState::Before,
                PageState::Never,
                PageState::Since
            ]
        );
        assert_eq!((sram.written, sram.written_since_reset), (2, 1));
        assert_eq!(sram.lowest, Some(SRAM_DBUS_BASE + 4096));
        assert_eq!(sram.highest_end, Some(SRAM_DBUS_BASE + 4 * 4096));
        assert_eq!((sram.writes, sram.writes_since_reset), (9, 5));
    }

    #[test]
    fn a_ram_never_written_has_no_bounds() {
        let mut usage = usage();
        usage.memories[0].page_versions = vec![0; 4];
        usage.memories[0].version = 1;
        usage.memories[0].version_at_reset = 1;
        let view = view(&usage, None, HostSide::default());
        let sram = &view.memories[0];
        assert_eq!(
            (sram.lowest, sram.highest_end, sram.written),
            (None, None, 0)
        );
    }

    #[test]
    fn sources_lines_and_blocks_are_named_and_only_those_used_are_listed() {
        let view = view(&usage(), None, HostSide::default());
        assert_eq!(
            view.interrupts,
            [SourceView {
                source: 37,
                name: Some("SYSTIMER_TARGET0_INT"),
                raised: 12,
                recent: None
            }]
        );
        assert_eq!(view.lines, [LineView { line: 4, taken: 11 }]);
        assert_eq!(
            view.peripherals,
            [BlockView {
                base: 0x6002_3000,
                peripherals: vec!["SYSTIMER"],
                reads: 30,
                writes: 12,
                recent: None
            }]
        );
    }

    #[test]
    fn the_window_is_what_happened_since_an_earlier_reading() {
        let before = usage();
        let mut now = usage();
        now.device_time = Cycles(2_000 * MICROSECOND);
        now.instructions += 1_000;
        now.clocks[1].awake = Cycles(1_600 * MICROSECOND);
        now.clocks[1].waiting = Cycles(400 * MICROSECOND);
        now.deep_sleep = Cycles(300 * MICROSECOND);
        now.raised[37] = 20;
        now.blocks[0].reads = 40;
        let view = view(&now, Some(&before), HostSide::default());
        let recent = view.recent.unwrap();
        assert_eq!(recent.device_nanos, 1_000_000);
        assert_eq!(
            recent.time,
            TimeView {
                running_nanos: 800_000,
                waiting_nanos: 100_000,
                deep_sleep_nanos: 100_000,
                load_permille: Some(889),
            }
        );
        assert_eq!(
            (
                recent.instructions,
                recent.interrupts,
                recent.register_accesses
            ),
            (1_000, 8, 10)
        );
        assert_eq!(view.interrupts[0].recent, Some(8));
        assert_eq!(view.peripherals[0].recent, Some(10));
    }

    #[test]
    fn there_is_no_window_without_time_or_from_a_reading_of_another_run() {
        let now = usage();
        assert_eq!(view(&now, Some(&now), HostSide::default()).recent, None);
        let mut later = usage();
        later.device_time = Cycles(10_000 * MICROSECOND);
        later.instructions = 10;
        assert_eq!(view(&now, Some(&later), HostSide::default()).recent, None);
        let mut other = usage();
        other.device_time = Cycles(10);
        other.resets = 5;
        let view = view(&now, Some(&other), HostSide::default());
        assert_eq!(view.recent, None);
        assert_eq!(view.interrupts[0].recent, None);
    }

    #[test]
    fn the_power_estimate_prices_the_time_at_160_mhz_and_in_deep_sleep_only() {
        let view = view(&usage(), None, HostSide::default());
        assert_eq!(view.power.unpriced_nanos, 100_000, "the time at 20 MHz");
        assert_eq!(view.power.rows.len(), 4);
    }

    #[test]
    fn the_host_adds_the_speed_ratio_and_the_panel() {
        let host = HostSide {
            speed_ratio: Some(0.5),
            panel: Some(PanelUse {
                refreshes: 3,
                last: Refresh::Partial,
            }),
        };
        let view = view(&usage(), None, host);
        assert_eq!(view.speed_ratio, Some(0.5));
        assert_eq!(view.panel, host.panel);
    }

    #[test]
    fn a_machine_that_ran_shows_its_three_rams_and_a_load() {
        use soc_esp32c3::test_support::{T0, T1, flat_rom};
        use soc_esp32c3::{Esp32C3, SocConfig};
        // Stores in the first page of the SRAM, for ever.
        let program = [
            cpu_rv32::asm::lui(T0, SRAM_DBUS_BASE),
            cpu_rv32::asm::addi(T1, T1, 1),
            cpu_rv32::asm::sw(T1, T0, 0),
            cpu_rv32::asm::jal(0, -8),
        ];
        let mut machine = Esp32C3::new(flat_rom(&program), &SocConfig::default());
        machine.run_until(Cycles(1_000));
        let view = view(&machine.usage(), None, HostSide::default());
        let regions: Vec<MemoryRegion> = view.memories.iter().map(|memory| memory.region).collect();
        assert_eq!(
            regions,
            [
                MemoryRegion::Sram,
                MemoryRegion::Sram0,
                MemoryRegion::RtcFast
            ]
        );
        assert_eq!(view.memories[0].pages.len(), 96);
        assert_eq!(view.memories[0].pages[0], PageState::Since);
        assert_eq!(view.memories[0].written, 1);
        assert_eq!(view.time.load_permille, Some(1000), "it never waits");
        assert_eq!(view.cpu_hz, 20_000_000);
    }

    #[test]
    fn the_view_reads_in_camel_case_with_its_enumerations_as_words() {
        let host = HostSide {
            speed_ratio: None,
            panel: Some(PanelUse {
                refreshes: 1,
                last: Refresh::Full,
            }),
        };
        let json = serde_json::to_value(view(&usage(), None, host)).unwrap();
        assert_eq!(json["deviceTimeNanos"], 1_000_000);
        assert_eq!(json["time"]["loadPermille"], 625);
        assert_eq!(json["memories"][0]["region"], "sram");
        assert_eq!(json["memories"][0]["pages"][1], "before");
        assert_eq!(json["memories"][0]["highestEnd"], SRAM_DBUS_BASE + 4 * 4096);
        assert_eq!(json["interrupts"][0]["name"], "SYSTIMER_TARGET0_INT");
        assert_eq!(json["peripherals"][0]["peripherals"][0], "SYSTIMER");
        assert_eq!(json["power"]["rows"][0]["state"], "running");
        assert_eq!(json["power"]["unpricedNanos"], 100_000);
        assert_eq!(json["panel"]["last"], "full");
        assert!(json["recent"].is_null());
        assert!(json["speedRatio"].is_null());
    }
}
