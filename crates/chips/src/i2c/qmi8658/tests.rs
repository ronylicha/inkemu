use emu_core::Nanos;

use super::*;
use crate::i2c::MAX_DIAGNOSTICS;
use crate::i2c::bus::{read_register, write};

const MS: u64 = 1_000_000;
const S: u64 = 1_000 * MS;
const NONE: Vec<I2cDiagnostic> = Vec::new();

fn at(nanos: u64) -> Nanos {
    Nanos(nanos)
}

fn sensor() -> Qmi8658 {
    Qmi8658::new(Qmi8658Config::default()).unwrap()
}

fn sensor_moving(acceleration_mg: [i32; 3], angular_rate_mdps: [i32; 3]) -> Qmi8658 {
    Qmi8658::new(Qmi8658Config {
        acceleration_mg,
        angular_rate_mdps,
        ..Qmi8658Config::default()
    })
    .unwrap()
}

/// One register written in its own transaction, as `FreeInk`'s `Imu` does.
fn set(chip: &mut Qmi8658, register: u8, value: u8, when: Nanos) {
    assert_eq!(write(chip, ADDRESS, &[register, value], when), [true; 3]);
}

/// `Imu::begin` (research `sp3-x3-firmware.md` §6): CTRL7 off, auto-increment, ±2 g and
/// ±512 dps at ODR code 6, both sensors on.
fn begin_like_crosspoint(chip: &mut Qmi8658, when: Nanos) {
    set(chip, CTRL7, 0x00, when);
    set(chip, CTRL1, 0x40, when);
    set(chip, CTRL2, 0x06, when);
    set(chip, CTRL3, 0x56, when);
    set(chip, CTRL7, 0x03, when);
}

fn axes(bytes: &[u8]) -> [i16; 3] {
    [0, 2, 4].map(|i| i16::from_le_bytes([bytes[i], bytes[i + 1]]))
}

fn kinds(chip: &mut Qmi8658) -> Vec<I2cDiagnosticKind> {
    chip.take_diagnostics()
        .into_iter()
        .map(|d| d.kind)
        .collect()
}

#[test]
fn the_probe_finds_who_am_i_at_0x6b_and_nothing_at_0x6a() {
    let mut chip = sensor();
    assert_eq!(
        read_register(&mut chip, 0x6B, 0x00, 1, at(0)),
        Some(vec![0x05])
    );
    assert_eq!(read_register(&mut chip, 0x6A, 0x00, 1, at(0)), None);
    assert_eq!(chip.register(REVISION_ID, at(0)), 0x68);
}

#[test]
fn the_alternate_address_is_a_profile_choice_and_others_are_refused() {
    let mut chip = Qmi8658::new(Qmi8658Config {
        address: ALTERNATE_ADDRESS,
        ..Qmi8658Config::default()
    })
    .unwrap();
    assert_eq!(
        read_register(&mut chip, 0x6A, 0x00, 1, at(0)),
        Some(vec![0x05])
    );
    assert_eq!(read_register(&mut chip, 0x6B, 0x00, 1, at(0)), None);
    let error = Qmi8658::new(Qmi8658Config {
        address: 0x6C,
        ..Qmi8658Config::default()
    })
    .unwrap_err();
    assert_eq!(error, Qmi8658ConfigError::Address(0x6C));
    assert!(error.to_string().contains("0x6c"));
}

#[test]
fn crosspoint_reads_a_still_device_lying_face_up() {
    let mut chip = sensor();
    begin_like_crosspoint(&mut chip, at(0));
    let gyro = read_register(&mut chip, ADDRESS, GYRO_FIRST, 6, at(50 * MS)).unwrap();
    let accel = read_register(&mut chip, ADDRESS, ACCEL_FIRST, 6, at(50 * MS)).unwrap();
    assert_eq!(gyro, [0; 6]);
    assert_eq!(
        accel,
        [0x00, 0x00, 0x00, 0x00, 0x00, 0x40],
        "AZ = 1 g at ±2 g"
    );
    assert_eq!(chip.take_diagnostics(), NONE);
}

#[test]
fn the_output_follows_the_full_scale() {
    let mut chip = sensor_moving([3_000, -500, 1_000], [300_000, -45_500, 1]);
    begin_like_crosspoint(&mut chip, at(0));
    let accel = axes(&read_register(&mut chip, ADDRESS, ACCEL_FIRST, 6, at(0)).unwrap());
    assert_eq!(accel, [i16::MAX, -8_192, 16_384], "3 g saturates at ±2 g");
    let gyro = axes(&read_register(&mut chip, ADDRESS, GYRO_FIRST, 6, at(0)).unwrap());
    assert_eq!(gyro, [19_200, -2_912, 0], "64 LSB/dps at ±512 dps");
    set(&mut chip, CTRL2, 0x36, at(0)); // ±16 g
    set(&mut chip, CTRL3, 0x06, at(0)); // ±16 dps
    let accel = axes(&read_register(&mut chip, ADDRESS, ACCEL_FIRST, 6, at(0)).unwrap());
    assert_eq!(accel, [6_144, -1_024, 2_048]);
    let gyro = axes(&read_register(&mut chip, ADDRESS, GYRO_FIRST, 6, at(0)).unwrap());
    assert_eq!(gyro, [i16::MAX, -32_768, 2]);
}

#[test]
fn status0_flags_new_samples_and_clears_when_read() {
    let mut chip = sensor();
    begin_like_crosspoint(&mut chip, at(S));
    let status =
        |chip: &mut Qmi8658, when| read_register(chip, ADDRESS, STATUS0, 1, when).unwrap()[0];
    assert_eq!(status(&mut chip, at(S)), 0x00, "no sample yet");
    assert_eq!(
        status(&mut chip, at(S + 8 * MS)),
        0x03,
        "one sample at 125 Hz"
    );
    assert_eq!(
        status(&mut chip, at(S + 8 * MS)),
        0x00,
        "cleared by the read"
    );
    assert_eq!(status(&mut chip, at(S + 15 * MS)), 0x00);
    assert_eq!(chip.register(STATUS0, at(S + 16 * MS)), 0x03);
    assert_eq!(
        chip.register(STATUS0, at(S + 16 * MS)),
        0x03,
        "a peek clears nothing"
    );
    set(&mut chip, CTRL7, 0x01, at(S + 16 * MS)); // gyroscope off
    assert_eq!(
        status(&mut chip, at(S + 16 * MS)),
        0x03,
        "unread samples stay new"
    );
    assert_eq!(status(&mut chip, at(S + 40 * MS)), 0x01);
}

#[test]
fn the_timestamp_counts_samples_at_the_fastest_enabled_rate() {
    let mut chip = sensor();
    begin_like_crosspoint(&mut chip, at(2 * S));
    let timestamp = |chip: &mut Qmi8658, when| {
        let bytes = read_register(chip, ADDRESS, TIMESTAMP_L, 3, when).unwrap();
        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0])
    };
    assert_eq!(timestamp(&mut chip, at(2 * S)), 0);
    assert_eq!(timestamp(&mut chip, at(3 * S)), 125);
    set(&mut chip, CTRL3, 0x54, at(3 * S)); // gyroscope at code 4, 500 Hz
    assert_eq!(timestamp(&mut chip, at(4 * S)), 625);
    set(&mut chip, CTRL1, 0x41, at(4 * S)); // sensors off: the count holds
    assert_eq!(timestamp(&mut chip, at(9 * S)), 625);
}

#[test]
fn the_timestamp_wraps_at_24_bits() {
    let mut chip = sensor();
    set(&mut chip, CTRL1, 0x40, at(0));
    set(&mut chip, CTRL2, 0x00, at(0)); // 8000 Hz
    set(&mut chip, CTRL7, 0x01, at(0));
    let bytes = read_register(&mut chip, ADDRESS, TIMESTAMP_L, 3, at(2_100 * S)).unwrap();
    let count = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0]);
    assert_eq!(count, 16_800_000 - (1 << 24));
}

#[test]
fn without_auto_increment_a_burst_repeats_its_register() {
    let mut chip = sensor();
    assert_eq!(
        read_register(&mut chip, ADDRESS, WHO_AM_I, 3, at(0)),
        Some(vec![0x05; 3])
    );
    set(&mut chip, CTRL1, 0x40, at(0));
    assert_eq!(
        read_register(&mut chip, ADDRESS, WHO_AM_I, 3, at(0)),
        Some(vec![0x05, 0x68, 0x40])
    );
}

#[test]
fn a_disabled_sensor_keeps_its_last_output_and_its_read_is_reported_once() {
    let mut chip = sensor();
    set(&mut chip, CTRL1, 0x40, at(0));
    assert_eq!(
        read_register(&mut chip, ADDRESS, ACCEL_FIRST, 6, at(0)).unwrap(),
        [0; 6]
    );
    begin_like_crosspoint(&mut chip, at(0));
    // CrossPoint's sleep: CTRL7 off, then CTRL1 with SensorDisable.
    set(&mut chip, CTRL7, 0x00, at(S));
    set(&mut chip, CTRL1, 0x41, at(S));
    set(&mut chip, CTRL2, 0x36, at(S)); // a new full scale does not touch a held value
    let held = read_register(&mut chip, ADDRESS, ACCEL_FIRST, 6, at(2 * S)).unwrap();
    assert_eq!(held, [0, 0, 0, 0, 0x00, 0x40]);
    assert_eq!(
        kinds(&mut chip),
        [I2cDiagnosticKind::NotModelled {
            what: "a disabled sensor read, from register",
            value: 0x35,
        }]
    );
    read_register(&mut chip, ADDRESS, GYRO_FIRST, 6, at(2 * S));
    assert_eq!(kinds(&mut chip).len(), 1, "once for the gyroscope too");
}

/// `SPI_BE`, set at power-on, is not taken over I2C: the outputs read little endian, and the
/// first read of an output with the bit set is reported once; with the bit cleared, as every X3
/// firmware clears it, nothing is.
#[test]
fn an_output_read_with_the_big_endian_bit_set_is_little_endian_and_reported_once() {
    let mut chip = sensor_moving([0, 0, 1000], [0; 3]);
    set(&mut chip, CTRL2, 0x06, at(0));
    set(&mut chip, CTRL7, 0x01, at(0));
    assert_eq!(chip.register(CTRL1, at(0)) & 0x20, 0x20, "power-on value");
    // Z, 1 g at ±2 g: 16384, its low byte at `ACCEL_FIRST + 4`, its high byte after it.
    let low = read_register(&mut chip, ADDRESS, ACCEL_FIRST + 4, 1, at(S)).unwrap();
    let high = read_register(&mut chip, ADDRESS, ACCEL_FIRST + 5, 1, at(S)).unwrap();
    assert_eq!([low[0], high[0]], [0x00, 0x40], "low byte first");
    assert_eq!(
        kinds(&mut chip),
        [I2cDiagnosticKind::NotModelled {
            what: "an output read with CTRL1's big-endian bit (SPI_BE) set, read little endian over I2C; CTRL1",
            value: 0x20,
        }]
    );
    let _ = read_register(&mut chip, ADDRESS, TEMP_L, 2, at(S));
    assert_eq!(chip.take_diagnostics(), NONE, "once");
    let mut cleared = sensor();
    begin_like_crosspoint(&mut cleared, at(0));
    let _ = read_register(&mut cleared, ADDRESS, ACCEL_FIRST, 6, at(S));
    assert_eq!(cleared.take_diagnostics(), NONE);
}

#[test]
fn a_soft_reset_brings_every_register_back_and_reports_completion() {
    let mut chip = sensor();
    begin_like_crosspoint(&mut chip, at(0));
    assert_eq!(chip.register(RST_RESULT, at(0)), 0x00);
    set(&mut chip, RESET, 0xB0, at(S));
    assert_eq!(chip.register(CTRL1, at(S)), 0x20);
    assert_eq!(chip.register(CTRL7, at(S)), 0x00);
    assert_eq!(chip.register(TIMESTAMP_L, at(2 * S)), 0x00);
    assert_eq!(chip.register(ACCEL_LAST, at(2 * S)), 0x00);
    assert_eq!(chip.register(RST_RESULT, at(S)), 0x80);
    set(&mut chip, RESET, 0x01, at(S));
    assert_eq!(
        kinds(&mut chip),
        [I2cDiagnosticKind::Undefined {
            register: RESET,
            value: 0x01,
        }]
    );
}

#[test]
fn ctrl9_acknowledges_nop_and_reports_the_other_commands_once() {
    let mut chip = sensor();
    set(&mut chip, CTRL9, 0x00, at(0));
    assert_eq!(chip.take_diagnostics(), NONE);
    set(&mut chip, CTRL9, 0x04, at(0)); // RST_FIFO
    set(&mut chip, CTRL9, 0x04, at(0));
    assert_eq!(chip.register(CTRL9, at(0)), 0x04, "stored");
    assert_eq!(
        kinds(&mut chip),
        [I2cDiagnosticKind::NotModelled {
            what: "the CTRL9 command",
            value: 0x04,
        }]
    );
}

#[test]
fn the_fifo_the_motion_features_and_the_self_test_are_stored_and_reported() {
    let mut chip = sensor();
    set(&mut chip, FIFO_CTRL, 0x0A, at(0));
    set(&mut chip, CTRL6, 0x01, at(0));
    set(&mut chip, CTRL8, 0x00, at(0));
    set(&mut chip, CTRL2, 0x86, at(0));
    set(&mut chip, CTRL7, 0x04, at(0));
    assert_eq!(chip.register(FIFO_CTRL, at(0)), 0x0A);
    let reported = kinds(&mut chip);
    let whats: Vec<_> = reported
        .iter()
        .filter_map(|kind| match kind {
            I2cDiagnosticKind::NotModelled { what, .. } => Some(*what),
            _ => None,
        })
        .collect();
    assert_eq!(
        whats,
        [
            "the FIFO",
            "the motion features of CTRL6 and CTRL8",
            "the self-test",
            "the magnetometer and the AttitudeEngine",
        ]
    );
}

#[test]
fn a_rate_or_scale_the_datasheet_does_not_list_is_reported() {
    let mut chip = sensor();
    set(&mut chip, CTRL3, 0x0C, at(0)); // low-power code on the gyroscope
    set(&mut chip, CTRL2, 0x46, at(0)); // accelerometer full scale 4
    set(&mut chip, CTRL2, 0x0D, at(0)); // a low-power accelerometer rate: listed
    assert_eq!(
        kinds(&mut chip),
        [
            I2cDiagnosticKind::Undefined {
                register: CTRL3,
                value: 0x0C,
            },
            I2cDiagnosticKind::Undefined {
                register: CTRL2,
                value: 0x46,
            },
        ]
    );
}

#[test]
fn read_only_registers_and_pointers_past_the_map_are_reported() {
    let mut chip = sensor();
    set(&mut chip, WHO_AM_I, 0x00, at(0));
    assert_eq!(chip.register(WHO_AM_I, at(0)), 0x05);
    assert_eq!(write(&mut chip, ADDRESS, &[0x61, 0x00], at(0)), [true; 3]);
    assert_eq!(
        kinds(&mut chip),
        [
            I2cDiagnosticKind::ReadOnly { register: 0x00 },
            I2cDiagnosticKind::NoSuchRegister { register: 0x61 },
        ]
    );
}

#[test]
fn the_temperature_reads_in_256ths_of_a_degree() {
    let mut chip = sensor();
    assert_eq!(
        read_register(&mut chip, ADDRESS, TEMP_L, 1, at(0)),
        Some(vec![0x00])
    );
    assert_eq!(chip.register(TEMP_H, at(0)), 0x19);
    let cold = Qmi8658::new(Qmi8658Config {
        temperature_centi: -1_250,
        ..Qmi8658Config::default()
    })
    .unwrap();
    assert_eq!(
        [cold.register(TEMP_L, at(0)), cold.register(TEMP_H, at(0))],
        [0x80, 0xF3]
    );
    assert_eq!(cold.config().temperature_centi, -1_250);
}

#[test]
fn the_diagnostics_are_bounded() {
    let mut chip = sensor();
    for _ in 0..300 {
        set(&mut chip, RESET, 0x00, at(0));
    }
    assert_eq!(chip.take_diagnostics().len(), MAX_DIAGNOSTICS);
    assert_eq!(chip.dropped_diagnostics(), 300 - 256);
}
