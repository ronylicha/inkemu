use emu_core::{I2cTarget, Nanos};

use super::*;
use crate::i2c::MAX_DIAGNOSTICS;
use crate::i2c::bus::{quick_read, read_register, write};

const T: Nanos = Nanos(1_000_000);
const NONE: Vec<I2cDiagnostic> = Vec::new();

fn gauge() -> Bq27220 {
    Bq27220::new(Bq27220Config::default()).unwrap()
}

/// A 16-bit little-endian read as `FreeInk` makes it: pointer, repeated START, two bytes.
fn read16(chip: &mut Bq27220, command: u8) -> u16 {
    let bytes = read_register(chip, ADDRESS, command, 2, T).unwrap();
    u16::from_le_bytes([bytes[0], bytes[1]])
}

#[test]
fn the_probe_of_crosspoint_reads_a_plausible_charge_and_voltage() {
    // Research sp3-x3-firmware.md §2.1: SoC ≤ 100, then 2500 ≤ mV ≤ 5000.
    let mut chip = gauge();
    let soc = read16(&mut chip, 0x2C);
    let millivolts = read16(&mut chip, 0x08);
    assert_eq!(soc, 70);
    assert_eq!(millivolts, 3_850);
    assert!(soc <= 100 && (2_500..=5_000).contains(&millivolts));
    assert_eq!(chip.take_diagnostics(), NONE);
}

#[test]
fn only_its_own_address_is_acknowledged() {
    let mut chip = gauge();
    assert!(!chip.address(0x68, false, T));
    assert!(
        !chip.write(0x08, T),
        "bytes for another chip are not acknowledged"
    );
    assert!(chip.address(0x55, true, T));
    chip.stop(T);
    assert_eq!(read_register(&mut chip, 0x56, 0x08, 2, T), None);
}

#[test]
fn the_signed_and_derived_commands_give_the_profile_values() {
    let mut chip = gauge();
    assert_eq!(read16(&mut chip, 0x0C), 0xFFF1, "Current() −15 mA");
    assert_eq!(read16(&mut chip, 0x14), 0xFFF1, "AverageCurrent()");
    assert_eq!(read16(&mut chip, 0x0A), 0x0009, "BatteryStatus()");
    assert_eq!(read16(&mut chip, 0x06), 2_982, "Temperature()");
    assert_eq!(read16(&mut chip, 0x10), 455);
    assert_eq!(read16(&mut chip, 0x12), 650);
    assert_eq!(
        read16(&mut chip, 0x16),
        1_820,
        "TimeToEmpty() = 455 mAh at 15 mA"
    );
    assert_eq!(
        read16(&mut chip, 0x18),
        65_535,
        "TimeToFull(): not charging"
    );
    assert_eq!(
        read16(&mut chip, 0x04),
        65_535,
        "AtRateTimeToEmpty() with AtRate 0"
    );
    assert_eq!(read16(&mut chip, 0x1A), 0xFFF6, "StandbyCurrent() −10 mA");
    assert_eq!(read16(&mut chip, 0x1C), 2_730, "StandbyTimeToEmpty()");
    assert_eq!(read16(&mut chip, 0x1E), as_word(-500));
    assert_eq!(read16(&mut chip, 0x20), 54, "MaxLoadTimeToEmpty()");
    assert_eq!(
        read16(&mut chip, 0x24),
        as_word(-57),
        "AveragePower() 3.85 V × −15 mA"
    );
    assert_eq!(read16(&mut chip, 0x28), 2_982, "InternalTemperature()");
    assert_eq!(read16(&mut chip, 0x2A), 12);
    assert_eq!(read16(&mut chip, 0x2E), 100);
    assert_eq!(read16(&mut chip, 0x30), 4_200);
    assert_eq!(read16(&mut chip, 0x32), 325);
    assert_eq!(read16(&mut chip, 0x3A), 0x0026, "OperationStatus()");
    assert_eq!(read16(&mut chip, 0x3C), 650, "DesignCapacity(), not 3000");
    assert_eq!(read16(&mut chip, 0x00), 0x0000, "CONTROL_STATUS");
    assert_eq!(read16(&mut chip, 0x0E), 0, "an undocumented gap reads zero");
}

#[test]
fn a_burst_reads_across_register_pairs() {
    let mut chip = gauge();
    let bytes = read_register(&mut chip, ADDRESS, 0x08, 4, T).unwrap();
    assert_eq!(
        bytes,
        [0x0A, 0x0F, 0x09, 0x00],
        "Voltage() then BatteryStatus()"
    );
}

#[test]
fn a_quick_read_continues_from_the_last_acknowledged_byte() {
    let mut chip = gauge();
    // The master acknowledges 0x2C and NACKs 0x2D: the pointer stays on 0x2D.
    read16(&mut chip, 0x2C);
    let bytes = quick_read(&mut chip, ADDRESS, 3, T).unwrap();
    assert_eq!(
        bytes,
        [0x00, 100, 0x00],
        "high byte of SoC, then StateOfHealth()"
    );
}

#[test]
fn a_write_to_a_read_only_command_is_refused_and_reported() {
    let mut chip = gauge();
    let acks = write(&mut chip, ADDRESS, &[0x08, 0x34, 0x12], T);
    assert_eq!(acks, [true, true, false, false]);
    assert_eq!(read16(&mut chip, 0x08), 3_850, "unchanged");
    let diagnostics = chip.take_diagnostics();
    assert_eq!(diagnostics.len(), 2, "each refused byte is reported");
    assert_eq!(
        diagnostics[0].kind,
        I2cDiagnosticKind::ReadOnly { register: 0x08 }
    );
    assert_eq!(diagnostics[0].chip, "BQ27220");
    assert_eq!(diagnostics[0].at, T);
}

#[test]
fn a_command_byte_above_0x6b_is_refused_with_what_follows() {
    let mut chip = gauge();
    let acks = write(&mut chip, ADDRESS, &[0x79, 0x00], T);
    assert_eq!(acks, [true, false, false]);
    assert_eq!(read_register(&mut chip, ADDRESS, 0x6C, 2, T), None);
    let diagnostics = chip.take_diagnostics();
    assert_eq!(
        diagnostics
            .iter()
            .map(|d| d.kind.clone())
            .collect::<Vec<_>>(),
        [
            I2cDiagnosticKind::NoSuchRegister { register: 0x79 },
            I2cDiagnosticKind::NoSuchRegister { register: 0x6C },
        ]
    );
    assert_eq!(read16(&mut chip, 0x6A), 0, "the last gap still reads");
}

#[test]
fn device_number_answers_through_mac_data_and_control_reads_ffa5_once() {
    let mut chip = gauge();
    // FreeInk writes a 16-bit value as one three-byte transaction (research §3.1).
    assert_eq!(
        write(&mut chip, ADDRESS, &[0x00, 0x01, 0x00], T),
        [true, true, true, true]
    );
    assert_eq!(read16(&mut chip, 0x00), 0xFFA5);
    assert_eq!(read16(&mut chip, 0x00), 0x0000, "once only");
    let block = read_register(&mut chip, ADDRESS, 0x3E, 4, T).unwrap();
    assert_eq!(block, [0x01, 0x00, 0x20, 0x02]);
    let tail = read_register(&mut chip, ADDRESS, 0x60, 2, T).unwrap();
    assert_eq!(
        tail,
        [!0x23, 6],
        "sum of 01 00 20 02 complemented, length 2 + 4"
    );
    assert_eq!(chip.take_diagnostics(), NONE);
}

#[test]
fn fw_version_fills_its_block() {
    let mut chip = gauge();
    write(&mut chip, ADDRESS, &[0x00, 0x02, 0x00], T);
    let block = read_register(&mut chip, ADDRESS, 0x3E, 13, T).unwrap();
    assert_eq!(&block[..2], [0x02, 0x00]);
    assert_eq!(&block[2..], FW_VERSION_RESPONSE);
    assert_eq!(read16(&mut chip, 0x00), 0xFFA5);
}

#[test]
fn another_subcommand_is_reported_and_has_no_effect() {
    let mut chip = gauge();
    write(&mut chip, ADDRESS, &[0x00, 0x41, 0x00], T); // RESET
    write(&mut chip, ADDRESS, &[0x00, 0x41, 0x00], T);
    assert_eq!(read16(&mut chip, 0x00), 0x0000);
    assert_eq!(
        chip.take_diagnostics()
            .into_iter()
            .map(|d| d.kind)
            .collect::<Vec<_>>(),
        [I2cDiagnosticKind::NotModelled {
            what: "a Control() subcommand",
            value: 0x41,
        }],
        "reported once"
    );
}

#[test]
fn at_rate_temperature_and_the_btp_thresholds_keep_what_is_written() {
    let mut chip = gauge();
    let minus_100 = as_word(-100).to_le_bytes();
    write(&mut chip, ADDRESS, &[0x02, minus_100[0], minus_100[1]], T);
    assert_eq!(read16(&mut chip, 0x02), as_word(-100));
    assert_eq!(read16(&mut chip, 0x04), 273, "455 mAh at 100 mA");
    write(&mut chip, ADDRESS, &[0x06, 0xD0, 0x0B], T);
    assert_eq!(read16(&mut chip, 0x06), 3_024, "WRTEMP: the written value");
    assert_eq!(
        read16(&mut chip, 0x28),
        2_982,
        "the internal sensor is not written"
    );
    write(&mut chip, ADDRESS, &[0x34, 0x96, 0x00, 0x2C, 0x01], T);
    assert_eq!(read16(&mut chip, 0x34), 150);
    assert_eq!(read16(&mut chip, 0x36), 300);
    assert_eq!(chip.take_diagnostics(), NONE);
}

#[test]
fn the_data_memory_and_block_writes_are_kept_without_effect() {
    let mut chip = gauge();
    // FreeInk's data-memory rewrite (research §3.4): the block address, then a block write.
    assert_eq!(write(&mut chip, ADDRESS, &[0x3E, 0x9D, 0x92], T), [true; 4]);
    write(&mut chip, ADDRESS, &[0x40, 0x02, 0x8A], T);
    write(&mut chip, ADDRESS, &[0x60, 0x55, 0x24], T);
    assert_eq!(
        read_register(&mut chip, ADDRESS, 0x40, 2, T).unwrap(),
        [0x02, 0x8A]
    );
    assert_eq!(read16(&mut chip, 0x3C), 650, "DesignCapacity() unchanged");
    assert_eq!(
        chip.take_diagnostics()
            .into_iter()
            .map(|d| d.kind)
            .collect::<Vec<_>>(),
        [
            I2cDiagnosticKind::NotModelled {
                what: "data memory",
                value: 0x929D,
            },
            I2cDiagnosticKind::NotModelled {
                what: "a MACData() block write at data-memory address",
                value: 0x929D,
            },
        ]
    );
}

#[test]
fn a_percentage_above_100_is_refused() {
    let too_full = Bq27220Config {
        state_of_charge: 101,
        ..Bq27220Config::default()
    };
    assert_eq!(
        Bq27220::new(too_full).unwrap_err(),
        Bq27220ConfigError::StateOfCharge(101)
    );
    let too_healthy = Bq27220Config {
        state_of_health: 120,
        ..Bq27220Config::default()
    };
    let error = Bq27220::new(too_healthy).unwrap_err();
    assert_eq!(error, Bq27220ConfigError::StateOfHealth(120));
    assert!(error.to_string().contains("from 0 to 100"));
    assert_eq!(gauge().config(), &Bq27220Config::default());
}

#[test]
fn a_charging_battery_has_a_time_to_full_and_none_to_empty() {
    let charging = Bq27220Config {
        current_ma: 300,
        average_current_ma: 300,
        ..Bq27220Config::default()
    };
    let mut chip = Bq27220::new(charging).unwrap();
    assert_eq!(read16(&mut chip, 0x16), 65_535);
    assert_eq!(read16(&mut chip, 0x18), 39, "195 mAh at 300 mA");
}

#[test]
fn the_diagnostics_are_bounded() {
    let mut chip = gauge();
    for _ in 0..300 {
        write(&mut chip, ADDRESS, &[0x08, 0x00], T);
    }
    assert_eq!(chip.take_diagnostics().len(), MAX_DIAGNOSTICS);
    assert_eq!(chip.dropped_diagnostics(), 300 - 256);
}
