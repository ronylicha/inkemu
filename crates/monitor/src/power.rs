//! The consumption of the chip the datasheet gives (design `SP4b`, 4b-D8): typical figures for the
//! chip alone, said as such, and only for the states the datasheet has a figure for
//! (`docs/research/sp4b-resource-monitor.md` §3). No figure is extrapolated: the time spent where
//! the datasheet has none is said, and enters no charge.
use serde::Serialize;

/// Where every figure below comes from.
pub const SOURCE: &str = "ESP32-C3 Series Datasheet v2.4 (2026-05-06), tables 5-8 and 5-9";

/// Table 5-8, "Current Consumption in Modem-sleep Mode" (the chip with its Wi-Fi clock gated, as
/// the model has no radio): typical currents in µA, with all peripheral clocks disabled and with
/// all enabled, the CPU running or idle.
struct ModemSleep {
    hz: u64,
    running: [u32; 2],
    idle: [u32; 2],
}

const MODEM_SLEEP: [ModemSleep; 2] = [
    ModemSleep {
        hz: 160_000_000,
        running: [23_000, 28_000],
        idle: [16_000, 21_000],
    },
    ModemSleep {
        hz: 80_000_000,
        running: [17_000, 22_000],
        idle: [13_000, 18_000],
    },
];

/// Table 5-9, "Current Consumption in Low-Power Modes": deep sleep with the RTC timer and the RTC
/// memory, typical, in µA.
pub const DEEP_SLEEP_MICROAMPS: u32 = 5;

/// Nanoseconds in an hour: a charge in µAh is a current in µA times a time in hours.
const NANOS_PER_HOUR: f64 = 3_600_000_000_000.0;

/// A state of the chip the model reaches (a light sleep is refused, a supply cut ends the life).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PowerState {
    /// Awake, the CPU executing.
    Running,
    /// Awake, the CPU waiting in `wfi`.
    Idle,
    DeepSleep,
}

/// The time spent in one state, and what the datasheet makes of it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerRow {
    pub state: PowerState,
    /// The CPU frequency, for an awake state.
    pub hz: Option<u64>,
    pub nanos: u64,
    /// The datasheet's typical current, all peripheral clocks disabled then all enabled; `None`
    /// when it gives none for this state.
    pub microamps: Option<[u32; 2]>,
    /// The charge that current draws over the time, low then high.
    pub microamp_hours: Option<[f64; 2]>,
}

/// The estimate of the chip's consumption since power-on.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerView {
    /// The datasheet and its tables.
    pub source: &'static str,
    /// The states the chip spent time in.
    pub rows: Vec<PowerRow>,
    /// The charge of the rows with a figure, low then high.
    pub microamp_hours: [f64; 2],
    /// The time spent in states the datasheet gives no figure for: the charge leaves it out.
    pub unpriced_nanos: u64,
}

/// The time of one CPU frequency: its `hz`, the nanoseconds executing and waiting.
#[derive(Clone, Copy, Debug)]
pub struct ClockTime {
    pub hz: u64,
    pub running_nanos: u64,
    pub waiting_nanos: u64,
}

/// The charge `microamps` draws over `nanos`.
fn charge(microamps: [u32; 2], nanos: u64) -> [f64; 2] {
    // A count of nanoseconds below 2^52 is exact as `f64`: 52 days of device time.
    #[allow(clippy::cast_precision_loss)]
    let hours = nanos as f64 / NANOS_PER_HOUR;
    microamps.map(|current| f64::from(current) * hours)
}

fn row(state: PowerState, hz: Option<u64>, nanos: u64, microamps: Option<[u32; 2]>) -> PowerRow {
    PowerRow {
        state,
        hz,
        nanos,
        microamps,
        microamp_hours: microamps.map(|current| charge(current, nanos)),
    }
}

/// The rows of the time spent at each frequency and in deep sleep (rows without time are left
/// out), and their charge.
#[must_use]
pub fn estimate(clocks: &[ClockTime], deep_sleep_nanos: u64) -> PowerView {
    let mut rows = Vec::new();
    for clock in clocks {
        let figures = MODEM_SLEEP.iter().find(|figure| figure.hz == clock.hz);
        for (state, nanos, current) in [
            (
                PowerState::Running,
                clock.running_nanos,
                figures.map(|figure| figure.running),
            ),
            (
                PowerState::Idle,
                clock.waiting_nanos,
                figures.map(|figure| figure.idle),
            ),
        ] {
            if nanos > 0 {
                rows.push(row(state, Some(clock.hz), nanos, current));
            }
        }
    }
    if deep_sleep_nanos > 0 {
        rows.push(row(
            PowerState::DeepSleep,
            None,
            deep_sleep_nanos,
            Some([DEEP_SLEEP_MICROAMPS; 2]),
        ));
    }
    let microamp_hours = rows
        .iter()
        .filter_map(|row| row.microamp_hours)
        .fold([0.0; 2], |[low, high], [a, b]| [low + a, high + b]);
    let unpriced_nanos = rows
        .iter()
        .filter(|row| row.microamps.is_none())
        .map(|row| row.nanos)
        .sum();
    PowerView {
        source: SOURCE,
        rows,
        microamp_hours,
        unpriced_nanos,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: u64 = 3_600_000_000_000;

    fn clock(hz: u64, running_nanos: u64, waiting_nanos: u64) -> ClockTime {
        ClockTime {
            hz,
            running_nanos,
            waiting_nanos,
        }
    }

    #[test]
    fn an_hour_at_160_mhz_draws_the_datasheet_current_running_and_idle() {
        let view = estimate(&[clock(160_000_000, HOUR, HOUR)], 0);
        assert_eq!(view.source, SOURCE);
        assert_eq!(view.rows.len(), 2);
        assert_eq!(view.rows[0].state, PowerState::Running);
        assert_eq!(view.rows[0].microamps, Some([23_000, 28_000]));
        assert_eq!(view.rows[0].microamp_hours, Some([23_000.0, 28_000.0]));
        assert_eq!(view.rows[1].state, PowerState::Idle);
        assert_eq!(view.rows[1].microamps, Some([16_000, 21_000]));
        assert_eq!(view.microamp_hours, [39_000.0, 49_000.0]);
        assert_eq!(view.unpriced_nanos, 0);
    }

    #[test]
    fn the_80_mhz_figures_are_those_of_table_5_8() {
        let view = estimate(&[clock(80_000_000, HOUR / 2, HOUR / 2)], 0);
        assert_eq!(view.rows[0].microamps, Some([17_000, 22_000]));
        assert_eq!(view.rows[1].microamps, Some([13_000, 18_000]));
        assert_eq!(view.microamp_hours, [15_000.0, 20_000.0]);
    }

    #[test]
    fn a_frequency_without_a_figure_has_no_current_and_its_time_is_said() {
        let view = estimate(
            &[clock(20_000_000, 3_000, 1_000), clock(160_000_000, HOUR, 0)],
            0,
        );
        let rows: Vec<(PowerState, Option<u64>, Option<[u32; 2]>)> = view
            .rows
            .iter()
            .map(|row| (row.state, row.hz, row.microamps))
            .collect();
        assert_eq!(
            rows,
            [
                (PowerState::Running, Some(20_000_000), None),
                (PowerState::Idle, Some(20_000_000), None),
                (
                    PowerState::Running,
                    Some(160_000_000),
                    Some([23_000, 28_000])
                ),
            ]
        );
        assert_eq!(view.unpriced_nanos, 4_000);
        assert_eq!(view.microamp_hours, [23_000.0, 28_000.0]);
    }

    #[test]
    fn a_deep_sleep_draws_5_microamps() {
        let view = estimate(&[], 2 * HOUR);
        assert_eq!(view.rows.len(), 1);
        assert_eq!(view.rows[0].state, PowerState::DeepSleep);
        assert_eq!(view.rows[0].hz, None);
        assert_eq!(view.rows[0].microamps, Some([5, 5]));
        assert_eq!(view.microamp_hours, [10.0, 10.0]);
    }

    #[test]
    fn no_time_is_no_row_and_no_charge() {
        let view = estimate(&[clock(160_000_000, 0, 0)], 0);
        assert_eq!(view.rows, Vec::new());
        assert_eq!(view.microamp_hours, [0.0, 0.0]);
    }
}
