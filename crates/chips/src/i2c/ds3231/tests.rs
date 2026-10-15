use emu_core::{I2cTarget, Nanos};

use super::*;
use crate::i2c::MAX_DIAGNOSTICS;
use crate::i2c::bus::{read_register, write};

const MS: u64 = 1_000_000;
const S: u64 = 1_000 * MS;
const NONE: Vec<I2cDiagnostic> = Vec::new();

fn at(nanos: u64) -> Nanos {
    Nanos(nanos)
}

fn time(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> DateTime {
    DateTime::new(year, month, day, hour, minute, second).unwrap()
}

fn clock_at(start: DateTime) -> Ds3231 {
    Ds3231::new(Ds3231Config {
        start,
        ..Ds3231Config::default()
    })
    .unwrap()
}

/// The seven time registers as `CrossPoint` reads them (research `sp3-x3-firmware.md` §5).
fn read_time(chip: &mut Ds3231, when: Nanos) -> Vec<u8> {
    read_register(chip, ADDRESS, 0x00, 7, when).unwrap()
}

#[test]
fn the_probe_of_crosspoint_reads_valid_bcd_seconds() {
    let mut chip = Ds3231::new(Ds3231Config::default()).unwrap();
    let seconds = read_register(&mut chip, ADDRESS, 0x00, 1, at(37 * S)).unwrap()[0];
    assert_eq!(seconds, 0x37);
    assert!(seconds >> 4 <= 5 && seconds & 0x0F <= 9);
    assert_eq!(read_register(&mut chip, 0x69, 0x00, 1, at(0)), None);
}

#[test]
fn crosspoint_begins_the_clock_and_reads_the_time() {
    let mut chip = Ds3231::new(Ds3231Config::default()).unwrap();
    // Rtc::begin: Status must answer, then Control = 0x04 (INTCN, no square wave).
    assert_eq!(
        read_register(&mut chip, ADDRESS, 0x0F, 1, at(0)).unwrap(),
        [0x08]
    );
    assert_eq!(write(&mut chip, ADDRESS, &[0x0E, 0x04], at(0)), [true; 3]);
    // Rtc::now: OSF clear, then seven bytes from 0x00. 2026-01-01 is a Thursday (4).
    assert_eq!(
        read_time(&mut chip, at(0)),
        [0x00, 0x00, 0x00, 0x04, 0x01, 0x01, 0x26]
    );
    assert_eq!(chip.take_diagnostics(), NONE);
}

#[test]
fn the_time_runs_with_device_time_by_whole_seconds() {
    let mut chip = Ds3231::new(Ds3231Config::default()).unwrap();
    assert_eq!(read_time(&mut chip, at(0))[..2], [0x00, 0x00]);
    assert_eq!(
        read_time(&mut chip, at(61 * S + 500 * MS))[..2],
        [0x01, 0x01]
    );
    assert_eq!(chip.now(at(61 * S + 999 * MS)), time(2026, 1, 1, 0, 1, 1));
    assert_eq!(chip.now(at(62 * S)), time(2026, 1, 1, 0, 1, 2));
}

#[test]
fn midnight_rolls_the_date_and_the_day_of_the_week() {
    // 2026-03-31 is a Tuesday (2).
    let mut chip = clock_at(time(2026, 3, 31, 23, 59, 59));
    assert_eq!(
        read_time(&mut chip, at(0)),
        [0x59, 0x59, 0x23, 0x02, 0x31, 0x03, 0x26]
    );
    assert_eq!(
        read_time(&mut chip, at(S)),
        [0x00, 0x00, 0x00, 0x03, 0x01, 0x04, 0x26]
    );
}

#[test]
fn the_century_bit_is_set_from_2100_and_the_clock_wraps_after_2199() {
    let mut chip = clock_at(time(2099, 12, 31, 23, 59, 59));
    assert_eq!(read_time(&mut chip, at(0))[5..], [0x12, 0x99]);
    assert_eq!(read_time(&mut chip, at(S))[4..], [0x01, 0x81, 0x00]);
    let mut late = clock_at(time(2199, 12, 31, 23, 59, 59));
    assert_eq!(late.now(at(S)), time(2000, 1, 1, 0, 0, 0));
    assert_eq!(read_time(&mut late, at(S))[4..], [0x01, 0x01, 0x00]);
}

#[test]
fn a_read_burst_gives_the_time_latched_at_its_start() {
    let mut chip = Ds3231::new(Ds3231Config::default()).unwrap();
    assert!(chip.address(ADDRESS, false, at(900 * MS)));
    assert!(chip.write(0x00, at(900 * MS)));
    assert!(chip.address(ADDRESS, true, at(900 * MS)));
    // The bytes come after the second has turned: the burst still reads 00:00:00.
    assert_eq!(chip.read(true, at(1_100 * MS)), 0x00);
    chip.stop(at(1_100 * MS));
    assert_eq!(
        read_register(&mut chip, ADDRESS, 0x00, 1, at(1_100 * MS)).unwrap(),
        [0x01]
    );
}

#[test]
fn the_pointer_wraps_after_0x12_and_relatches_the_time() {
    let mut chip = Ds3231::new(Ds3231Config::default()).unwrap();
    assert!(chip.address(ADDRESS, false, at(0)));
    assert!(chip.write(0x11, at(0)));
    assert!(chip.address(ADDRESS, true, at(0)));
    assert_eq!(chip.read(true, at(0)), 0x19, "temperature MSB, 25 °C");
    assert_eq!(chip.read(true, at(2 * S)), 0x00, "temperature LSB");
    // The wrap to 0x00 happened at 2 s: the seconds read 02, not the 00 of the START.
    assert_eq!(chip.read(false, at(2 * S)), 0x02);
    chip.stop(at(2 * S));
}

#[test]
fn crosspoint_sets_the_time_and_it_runs_on_from_there() {
    let mut chip = Ds3231::new(Ds3231Config::default()).unwrap();
    // Rtc::set: 24-hour time, weekday with Sunday 7, no century bit (research §5).
    let set = [0x00, 0x30, 0x45, 0x17, 0x07, 0x18, 0x10, 0x26];
    assert_eq!(
        write(&mut chip, ADDRESS, &set, at(10 * S + 300 * MS)),
        [true; 9]
    );
    assert_eq!(
        chip.now(at(10 * S + 300 * MS)),
        time(2026, 10, 18, 17, 45, 30)
    );
    // Writing the seconds began a new second at the write: 2.5 s later, 2 s have passed.
    assert_eq!(
        read_time(&mut chip, at(12 * S + 800 * MS)),
        [0x32, 0x45, 0x17, 0x07, 0x18, 0x10, 0x26]
    );
    assert_eq!(chip.take_diagnostics(), NONE);
}

#[test]
fn a_date_written_before_its_month_is_taken_with_the_whole_burst() {
    let mut chip = clock_at(time(2026, 2, 10, 8, 0, 0));
    let set = [0x00, 0x00, 0x00, 0x12, 0x02, 0x31, 0x03, 0x26];
    write(&mut chip, ADDRESS, &set, at(0));
    assert_eq!(chip.now(at(0)), time(2026, 3, 31, 12, 0, 0));
    assert_eq!(chip.take_diagnostics(), NONE);
}

#[test]
fn a_date_left_past_its_month_is_reported_and_taken_as_the_last_day() {
    let mut chip = clock_at(time(2026, 1, 31, 8, 0, 0));
    write(&mut chip, ADDRESS, &[0x05, 0x02], at(0));
    assert_eq!(chip.now(at(0)), time(2026, 2, 28, 8, 0, 0));
    assert_eq!(
        chip.take_diagnostics()[0].kind,
        I2cDiagnosticKind::Undefined {
            register: 0x04,
            value: 0x31,
        }
    );
}

#[test]
fn a_field_written_alone_keeps_the_sub_second_phase() {
    let mut chip = Ds3231::new(Ds3231Config::default()).unwrap();
    write(&mut chip, ADDRESS, &[0x01, 0x10], at(1_500 * MS));
    assert_eq!(chip.now(at(1_999 * MS)), time(2026, 1, 1, 0, 10, 1));
    assert_eq!(chip.now(at(2 * S)), time(2026, 1, 1, 0, 10, 2));
    write(&mut chip, ADDRESS, &[0x00, 0x30], at(2_500 * MS));
    assert_eq!(chip.now(at(3_400 * MS)), time(2026, 1, 1, 0, 10, 30));
    assert_eq!(chip.now(at(3_500 * MS)), time(2026, 1, 1, 0, 10, 31));
}

#[test]
fn an_undefined_value_is_reported_and_leaves_its_field_alone() {
    let mut chip = Ds3231::new(Ds3231Config::default()).unwrap();
    write(
        &mut chip,
        ADDRESS,
        &[0x00, 0x6A, 0x61, 0x25, 0x08, 0x00, 0x13],
        at(0),
    );
    assert_eq!(chip.now(at(0)), time(2026, 1, 1, 0, 0, 0));
    assert_eq!(chip.register(0x03, at(0)), Some(0x04));
    let refused: Vec<_> = chip
        .take_diagnostics()
        .into_iter()
        .map(|d| d.kind)
        .collect();
    assert_eq!(
        refused,
        [
            (0x00, 0x6A),
            (0x01, 0x61),
            (0x02, 0x25),
            (0x03, 0x08),
            (0x04, 0x00),
            (0x05, 0x13)
        ]
        .map(|(register, value)| I2cDiagnosticKind::Undefined { register, value })
    );
}

#[test]
fn the_twelve_hour_mode_is_kept_from_the_last_hours_write() {
    let mut chip = Ds3231::new(Ds3231Config::default()).unwrap();
    write(&mut chip, ADDRESS, &[0x02, 0x71], at(0)); // 12-hour mode, PM, 11
    assert_eq!(chip.now(at(0)).hour, 23);
    assert_eq!(chip.register(0x02, at(0)), Some(0x71));
    write(&mut chip, ADDRESS, &[0x02, 0x52], at(0)); // 12 AM
    assert_eq!(chip.now(at(0)).hour, 0);
    assert_eq!(chip.register(0x02, at(0)), Some(0x52));
    write(&mut chip, ADDRESS, &[0x02, 0x72], at(0)); // 12 PM
    assert_eq!(chip.now(at(0)).hour, 12);
    write(&mut chip, ADDRESS, &[0x02, 0x05], at(0));
    assert_eq!(
        chip.register(0x02, at(0)),
        Some(0x05),
        "back to 24-hour mode"
    );
}

#[test]
fn the_day_of_the_week_is_a_counter_of_its_own() {
    let mut chip = clock_at(time(2026, 3, 31, 23, 59, 59));
    write(&mut chip, ADDRESS, &[0x03, 0x07], at(0));
    assert_eq!(chip.register(0x03, at(0)), Some(0x07));
    assert_eq!(
        chip.register(0x03, at(S)),
        Some(0x01),
        "7 rolls over to 1 at midnight"
    );
    write(&mut chip, ADDRESS, &[0x04, 0x15], at(S));
    assert_eq!(
        chip.register(0x03, at(S)),
        Some(0x01),
        "a date write leaves it alone"
    );
}

#[test]
fn osf_and_the_alarm_flags_can_only_be_cleared() {
    let mut chip = Ds3231::new(Ds3231Config {
        oscillator_stopped: true,
        ..Ds3231Config::default()
    })
    .unwrap();
    assert_eq!(chip.register(0x0F, at(0)), Some(0x88));
    write(&mut chip, ADDRESS, &[0x0F, 0xFF], at(0));
    assert_eq!(
        chip.register(0x0F, at(0)),
        Some(0x88),
        "a 1 sets nothing, BSY stays clear"
    );
    // CrossPoint reads Status and writes it back with OSF cleared.
    write(&mut chip, ADDRESS, &[0x0F, 0x08], at(0));
    assert_eq!(chip.register(0x0F, at(0)), Some(0x08));
    write(&mut chip, ADDRESS, &[0x0F, 0x80], at(0));
    assert_eq!(
        chip.register(0x0F, at(0)),
        Some(0x00),
        "EN32kHz written 0, OSF not set"
    );
}

#[test]
fn conv_keeps_bsy_and_conv_set_for_200_ms() {
    let mut chip = Ds3231::new(Ds3231Config::default()).unwrap();
    write(&mut chip, ADDRESS, &[0x0E, 0x24], at(S));
    assert_eq!(chip.register(0x0E, at(S)), Some(0x24));
    assert_eq!(chip.register(0x0F, at(S + 199 * MS)), Some(0x0C));
    assert_eq!(chip.register(0x0E, at(S + 200 * MS)), Some(0x04));
    assert_eq!(chip.register(0x0F, at(S + 200 * MS)), Some(0x08));
    assert_eq!(chip.take_diagnostics(), NONE);
}

#[test]
fn the_int_sqw_output_is_reported_once_when_enabled() {
    let mut chip = Ds3231::new(Ds3231Config::default()).unwrap();
    write(&mut chip, ADDRESS, &[0x0E, 0x05], at(0)); // INTCN and A1IE
    write(&mut chip, ADDRESS, &[0x0E, 0x00], at(0)); // square wave
    assert_eq!(
        chip.take_diagnostics()
            .into_iter()
            .map(|d| d.kind)
            .collect::<Vec<_>>(),
        [I2cDiagnosticKind::NotModelled {
            what: "the INT/SQW output, enabled by Control",
            value: 0x05,
        }]
    );
}

#[test]
fn the_temperature_reads_in_quarters_and_refuses_writes() {
    let mut chip = Ds3231::new(Ds3231Config::default()).unwrap();
    assert_eq!(
        read_register(&mut chip, ADDRESS, 0x11, 2, at(0)).unwrap(),
        [0x19, 0x00]
    );
    write(&mut chip, ADDRESS, &[0x11, 0x00], at(0));
    assert_eq!(chip.register(0x11, at(0)), Some(0x19));
    assert_eq!(
        chip.take_diagnostics()[0].kind,
        I2cDiagnosticKind::ReadOnly { register: 0x11 }
    );
    let cold = Ds3231::new(Ds3231Config {
        temperature_quarters: -101,
        ..Ds3231Config::default()
    })
    .unwrap();
    assert_eq!(cold.register(0x11, at(0)), Some(0xE6), "−26");
    assert_eq!(cold.register(0x12, at(0)), Some(0xC0), "+0.75");
}

#[test]
fn the_alarms_and_aging_are_stored() {
    let mut chip = Ds3231::new(Ds3231Config::default()).unwrap();
    let alarms = [0x07, 0x01, 0x02, 0x03, 0x84, 0x05, 0x06, 0x87, 0xF0];
    write(&mut chip, ADDRESS, &alarms, at(0));
    assert_eq!(
        read_register(&mut chip, ADDRESS, 0x07, 8, at(0)).unwrap(),
        alarms[1..]
    );
    write(&mut chip, ADDRESS, &[0x10, 0xF0], at(0));
    assert_eq!(chip.register(0x10, at(0)), Some(0xF0));
    assert_eq!(chip.register(0x13, at(0)), None);
}

#[test]
fn a_pointer_past_0x12_is_reported_and_its_bytes_dropped() {
    let mut chip = Ds3231::new(Ds3231Config::default()).unwrap();
    assert_eq!(write(&mut chip, ADDRESS, &[0x20, 0x55], at(0)), [true; 3]);
    assert_eq!(
        chip.take_diagnostics()[0].kind,
        I2cDiagnosticKind::NoSuchRegister { register: 0x20 }
    );
    assert_eq!(chip.now(at(0)), time(2026, 1, 1, 0, 0, 0));
}

#[test]
fn a_date_time_converts_to_and_from_seconds_since_2000() {
    assert_eq!(DAYS_IN_RANGE, days_since_2000(2200, 1, 1));
    assert_eq!(time(2000, 1, 1, 0, 0, 0).seconds_since_2000(), 0);
    assert_eq!(time(2000, 3, 1, 0, 0, 0).seconds_since_2000(), 60 * 86_400);
    for start in [
        time(2000, 2, 29, 12, 34, 56),
        time(2026, 10, 15, 9, 0, 1),
        time(2100, 2, 28, 23, 59, 59),
        time(2100, 3, 1, 0, 0, 0),
        time(2199, 12, 31, 23, 59, 59),
    ] {
        let seconds = start.seconds_since_2000();
        assert_eq!(DateTime::from_seconds_since_2000(seconds), Some(start));
    }
    assert_eq!(DateTime::from_seconds_since_2000(SECONDS_IN_RANGE), None);
    assert_eq!(time(2024, 1, 1, 0, 0, 0).weekday(), 1, "a Monday");
    assert_eq!(time(2000, 1, 1, 0, 0, 0).weekday(), 6, "a Saturday");
    assert_eq!(time(2026, 1, 4, 0, 0, 0).weekday(), 7, "a Sunday");
}

#[test]
fn an_invalid_date_time_or_config_is_refused() {
    assert_eq!(
        DateTime::new(1999, 1, 1, 0, 0, 0),
        Err(DateTimeError::Year(1999))
    );
    assert_eq!(
        DateTime::new(2026, 13, 1, 0, 0, 0),
        Err(DateTimeError::Month(13))
    );
    assert_eq!(
        DateTime::new(2100, 2, 29, 0, 0, 0),
        Err(DateTimeError::Day {
            year: 2100,
            month: 2,
            day: 29
        })
    );
    assert_eq!(
        DateTime::new(2026, 1, 1, 24, 0, 0),
        Err(DateTimeError::Hour(24))
    );
    assert_eq!(
        DateTime::new(2026, 1, 1, 0, 60, 0),
        Err(DateTimeError::Minute(60))
    );
    assert_eq!(
        DateTime::new(2026, 1, 1, 0, 0, 60),
        Err(DateTimeError::Second(60))
    );
    let bad_start = Ds3231Config {
        start: DateTime {
            day: 0,
            ..Ds3231Config::default().start
        },
        ..Ds3231Config::default()
    };
    assert!(matches!(
        Ds3231::new(bad_start),
        Err(Ds3231ConfigError::Start(DateTimeError::Day { .. }))
    ));
    let too_hot = Ds3231Config {
        temperature_quarters: 512,
        ..Ds3231Config::default()
    };
    assert_eq!(
        Ds3231::new(too_hot).unwrap_err(),
        Ds3231ConfigError::Temperature(512)
    );
}

#[test]
fn the_diagnostics_are_bounded() {
    let mut chip = Ds3231::new(Ds3231Config::default()).unwrap();
    for _ in 0..300 {
        write(&mut chip, ADDRESS, &[0x12, 0x00], at(0));
    }
    assert_eq!(chip.take_diagnostics().len(), MAX_DIAGNOSTICS);
    assert_eq!(chip.dropped_diagnostics(), 300 - 256);
}
