//! The TI BQ27220 fuel gauge of the X3, at `0x55` (research `sp3-x3-chips.md` §3).
//!
//! The gauge answers the standard commands of its reference manual: an 8-bit command pointer set
//! by the first byte written after the address, 16-bit little-endian values at an even command
//! and the next, the pointer moving on with every acknowledged byte, read or written (§3.1). The
//! values are those of a [`Bq27220Config`] and do not change: a battery that neither drains nor
//! charges, which is sub-project 4's to model.
//!
//! What `CrossPoint` 1.6.5 does with it (research `sp3-x3-firmware.md` §2.1, §4): reads only, of
//! `StateOfCharge()`, `Voltage()` and `Current()`, each a pointer write, a repeated START and two
//! bytes. The writable commands, the `Control()` subcommands `DEVICE_NUMBER` and `FW_VERSION` and
//! the `MACData()` block are kept for the firmwares that use them (`FreeInk`'s data-memory
//! rewrite, §3.4); the data memory itself and the security state machine are not modelled and say
//! so.

use emu_core::{I2cTarget, Nanos};

use super::{Diagnostics, I2cDiagnostic, I2cDiagnosticKind, Phase};

/// The gauge's seven-bit address, fixed (research §3.1).
pub const ADDRESS: u8 = 0x55;

const NAME: &str = "BQ27220";

/// The highest command code: a command byte above it is refused (research §3.1).
const LAST_COMMAND: u8 = 0x6B;

// Standard commands (research §3.2, BQTRM Table 2-1): the low byte of each 16-bit value.
const CONTROL: u8 = 0x00;
const CONTROL_HIGH: u8 = 0x01;
const AT_RATE: u8 = 0x02;
const AT_RATE_HIGH: u8 = 0x03;
const AT_RATE_TIME_TO_EMPTY: u8 = 0x04;
const TEMPERATURE: u8 = 0x06;
const TEMPERATURE_HIGH: u8 = 0x07;
const VOLTAGE: u8 = 0x08;
const BATTERY_STATUS: u8 = 0x0A;
const CURRENT: u8 = 0x0C;
const REMAINING_CAPACITY: u8 = 0x10;
const FULL_CHARGE_CAPACITY: u8 = 0x12;
const AVERAGE_CURRENT: u8 = 0x14;
const TIME_TO_EMPTY: u8 = 0x16;
const TIME_TO_FULL: u8 = 0x18;
const STANDBY_CURRENT: u8 = 0x1A;
const STANDBY_TIME_TO_EMPTY: u8 = 0x1C;
const MAX_LOAD_CURRENT: u8 = 0x1E;
const MAX_LOAD_TIME_TO_EMPTY: u8 = 0x20;
const AVERAGE_POWER: u8 = 0x24;
const INTERNAL_TEMPERATURE: u8 = 0x28;
const CYCLE_COUNT: u8 = 0x2A;
const STATE_OF_CHARGE: u8 = 0x2C;
const STATE_OF_HEALTH: u8 = 0x2E;
const CHARGING_VOLTAGE: u8 = 0x30;
const CHARGING_CURRENT: u8 = 0x32;
const BTP_DISCHARGE_SET: u8 = 0x34;
const BTP_CHARGE_SET_HIGH: u8 = 0x37;
const OPERATION_STATUS: u8 = 0x3A;
const DESIGN_CAPACITY: u8 = 0x3C;
const MAC_CONTROL: u8 = 0x3E;
const MAC_CONTROL_HIGH: u8 = 0x3F;
const MAC_DATA_LEN: u8 = 0x61;

/// The bytes the gauge stores as written: every writable command, `0x00` to `0x61`.
const STORE_LEN: usize = 0x62;
/// Where the response of a subcommand starts in the store: `ManufacturerAccessControl()`, then
/// `MACData()` from `0x40`.
const MAC_BLOCK: usize = 0x3E;
/// Offsets in that block: the subcommand echoed (`0x3E`), its data (`MACData()`, `0x40`), the sum
/// (`MACDataSum()`, `0x60`) and the length (`MACDataLen()`, `0x61`).
const MAC_DATA_OFFSET: usize = 0x40 - MAC_BLOCK;
const MAC_SUM_OFFSET: usize = 0x60 - MAC_BLOCK;
const MAC_LEN_OFFSET: usize = 0x61 - MAC_BLOCK;
/// What `MACDataLen()` counts besides the data. ASSUMPTION: TI's convention in its gauge manuals,
/// the two bytes of `ManufacturerAccessControl()`, the sum and the length themselves; a read of a
/// real X3's gauge after `DEVICE_NUMBER` would settle it.
const MAC_LEN_OVERHEAD: usize = 4;

// Control() subcommands (research §3.3, BQTRM Table 2-2).
const CONTROL_STATUS: u16 = 0x0000;
const DEVICE_NUMBER: u16 = 0x0001;
const FW_VERSION: u16 = 0x0002;

/// What `DEVICE_NUMBER` puts in `MACData()`: `0x0220`, in the byte order research §3.3 infers
/// from the manual's example (`01 00 20 02` read from `0x3E`).
const DEVICE_NUMBER_RESPONSE: [u8; 2] = [0x20, 0x02];
/// What `FW_VERSION` puts in `MACData()`, the block `ddDDvvVVbbBBTTzzZZRREE` of BQTRM Table 2-2.
/// ASSUMPTION: no source gives the X3 gauge's firmware; the device number is `DEVICE_NUMBER`'s,
/// the version `01 02` and the build `00 16` are plausible values, the rest zero. A read of a
/// real X3's gauge would settle it.
const FW_VERSION_RESPONSE: [u8; 11] = [
    0x20, 0x02, 0x01, 0x02, 0x00, 0x16, 0x00, 0x00, 0x00, 0x00, 0x00,
];
/// What `Control()` reads once after `DEVICE_NUMBER` or `FW_VERSION`: the response moved to
/// `MACData()` (research §3.3).
const RESPONSE_IN_MAC_DATA: u16 = 0xFFA5;
/// A word written to `ManufacturerAccessControl()` from here up is a data-memory address, below
/// it a subcommand. ASSUMPTION: the subcommands of BQTRM Table 2-2 end at `0x0F00` (`ENTER_ROM`)
/// and the data-memory addresses `FreeInk` writes are `0x929D` and `0x929F` (research §3.4); the
/// manual's data-memory map would place the boundary exactly.
const DATA_MEMORY_FIRST: u16 = 0x4000;

/// A time the gauge cannot give: not discharging (to empty) or not charging (to full).
const NO_TIME: u16 = 65_535;
const MINUTES_PER_HOUR: u32 = 60;
const MILLI: i32 = 1_000;
/// The highest state of charge and of health, in percent.
const FULL_PERCENT: u16 = 100;

/// What the gauge reports. The defaults are those of research §3.5, the plausible idle state of
/// an X3 on battery: profile parameters, not facts read from a device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bq27220Config {
    /// `Voltage()`, mV. ASSUMPTION: 3850, a cell at mid charge.
    pub voltage_mv: u16,
    /// `StateOfCharge()`, %, 0 to 100. ASSUMPTION: 70, matching 455 of 650 mAh.
    pub state_of_charge: u16,
    /// `Current()`, mA, negative while discharging. ASSUMPTION: −15, an idle reader.
    pub current_ma: i16,
    /// `AverageCurrent()`, mA. ASSUMPTION: equal to `Current()`, a steady load.
    pub average_current_ma: i16,
    /// `Temperature()` at power-on and `InternalTemperature()`, 0.1 K. ASSUMPTION: 2982, 25.0 °C.
    pub temperature_dk: u16,
    /// `RemainingCapacity()`, mAh. ASSUMPTION: 455.
    pub remaining_capacity_mah: u16,
    /// `FullChargeCapacity()`, mAh. ASSUMPTION: 650, the X3's cell as `FreeInk` writes it.
    pub full_charge_capacity_mah: u16,
    /// `DesignCapacity()`, mAh. ASSUMPTION: 650; the data-memory default 3000 would make
    /// `CrossPoint` run its data-memory rewrite (research §3.4), which this model does not carry.
    pub design_capacity_mah: u16,
    /// `CycleCount()`. ASSUMPTION: 12.
    pub cycle_count: u16,
    /// `StateOfHealth()`, %, 0 to 100. ASSUMPTION: 100, `FullChargeCapacity()` over
    /// `DesignCapacity()`.
    pub state_of_health: u16,
    /// `StandbyCurrent()`, mA. ASSUMPTION: −10, the manual's default standby load.
    pub standby_current_ma: i16,
    /// `MaxLoadCurrent()`, mA. ASSUMPTION: −500, a full-speed `SoC` with radio.
    pub max_load_current_ma: i16,
    /// `ChargingVoltage()`, mV. ASSUMPTION: 4200, a lithium cell's float voltage.
    pub charging_voltage_mv: u16,
    /// `ChargingCurrent()`, mA. ASSUMPTION: 325, half a C of 650 mAh.
    pub charging_current_ma: u16,
    /// `BatteryStatus()`. ASSUMPTION: `0x0009`, BATTPRES and DSG: a battery, discharging.
    pub battery_status: u16,
    /// `OperationStatus()`. ASSUMPTION: `0x0026`, INITCOMP and sealed.
    pub operation_status: u16,
}

impl Default for Bq27220Config {
    fn default() -> Bq27220Config {
        Bq27220Config {
            voltage_mv: 3_850,
            state_of_charge: 70,
            current_ma: -15,
            average_current_ma: -15,
            temperature_dk: 2_982,
            remaining_capacity_mah: 455,
            full_charge_capacity_mah: 650,
            design_capacity_mah: 650,
            cycle_count: 12,
            state_of_health: 100,
            standby_current_ma: -10,
            max_load_current_ma: -500,
            charging_voltage_mv: 4_200,
            charging_current_ma: 325,
            battery_status: 0x0009,
            operation_status: 0x0026,
        }
    }
}

/// Why a [`Bq27220Config`] is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Bq27220ConfigError {
    #[error(
        "a state of charge of {0} % is more than the gauge reports (100 %); give a percentage from \
         0 to 100"
    )]
    StateOfCharge(u16),
    #[error(
        "a state of health of {0} % is more than the gauge reports (100 %); give a percentage from \
         0 to 100"
    )]
    StateOfHealth(u16),
}

/// The BQ27220 on an I2C bus.
#[derive(Clone, Debug)]
pub struct Bq27220 {
    config: Bq27220Config,
    phase: Phase,
    pointer: u8,
    /// The writable commands as last written (`Control()`, `AtRate()`, `Temperature()`, the BTP
    /// thresholds, `ManufacturerAccessControl()` and the `MACData()` block); the other bytes stay
    /// zero.
    store: [u8; STORE_LEN],
    /// `Control()` reads [`RESPONSE_IN_MAC_DATA`] once.
    response_moved: bool,
    diagnostics: Diagnostics,
}

impl Bq27220 {
    /// A gauge reporting `config`, its pointer at `Control()`.
    ///
    /// # Errors
    /// [`Bq27220ConfigError`] when a percentage is above 100.
    pub fn new(config: Bq27220Config) -> Result<Bq27220, Bq27220ConfigError> {
        if config.state_of_charge > FULL_PERCENT {
            return Err(Bq27220ConfigError::StateOfCharge(config.state_of_charge));
        }
        if config.state_of_health > FULL_PERCENT {
            return Err(Bq27220ConfigError::StateOfHealth(config.state_of_health));
        }
        let mut store = [0; STORE_LEN];
        let [low, high] = config.temperature_dk.to_le_bytes();
        if let Some(slot) = store.get_mut(usize::from(TEMPERATURE)) {
            *slot = low;
        }
        if let Some(slot) = store.get_mut(usize::from(TEMPERATURE_HIGH)) {
            *slot = high;
        }
        Ok(Bq27220 {
            config,
            phase: Phase::Released,
            pointer: CONTROL,
            store,
            response_moved: false,
            diagnostics: Diagnostics::new(NAME),
        })
    }

    /// What the gauge reports.
    #[must_use]
    pub const fn config(&self) -> &Bq27220Config {
        &self.config
    }

    /// The diagnostics since the last call, oldest first.
    pub fn take_diagnostics(&mut self) -> Vec<I2cDiagnostic> {
        self.diagnostics.take()
    }

    /// How many diagnostics were dropped because [`super::MAX_DIAGNOSTICS`] were waiting.
    #[must_use]
    pub const fn dropped_diagnostics(&self) -> u64 {
        self.diagnostics.dropped()
    }

    /// The byte a read of `command` gives, without moving the pointer.
    fn byte_at(&self, command: u8) -> u8 {
        if matches!(command, CONTROL | CONTROL_HIGH) {
            let word = if self.response_moved {
                RESPONSE_IN_MAC_DATA
            } else {
                CONTROL_STATUS
            };
            return half(word, command);
        }
        if is_writable(command) {
            return self.stored(command);
        }
        if command < MAC_CONTROL {
            // ASSUMPTION: the undocumented gaps (`0x0E`, `0x26`, `0x38`) read zero, as
            // `standard_word` gives them; research §3.2 and §6 point 5 leave them open.
            return half(self.standard_word(command & !1), command);
        }
        // ASSUMPTION: the gap `0x62`–`0x6B` and the bytes a burst reads past `0x6B` read zero.
        0
    }

    /// The value of the read-only standard command at the even code `command`.
    fn standard_word(&self, command: u8) -> u16 {
        let c = &self.config;
        match command {
            AT_RATE_TIME_TO_EMPTY => time_to_empty(c.remaining_capacity_mah, self.at_rate()),
            VOLTAGE => c.voltage_mv,
            BATTERY_STATUS => c.battery_status,
            CURRENT => as_word(c.current_ma),
            REMAINING_CAPACITY => c.remaining_capacity_mah,
            FULL_CHARGE_CAPACITY => c.full_charge_capacity_mah,
            AVERAGE_CURRENT => as_word(c.average_current_ma),
            TIME_TO_EMPTY => time_to_empty(c.remaining_capacity_mah, c.average_current_ma),
            TIME_TO_FULL => time_to_full(
                c.full_charge_capacity_mah
                    .saturating_sub(c.remaining_capacity_mah),
                c.average_current_ma,
            ),
            STANDBY_CURRENT => as_word(c.standby_current_ma),
            STANDBY_TIME_TO_EMPTY => time_to_empty(c.remaining_capacity_mah, c.standby_current_ma),
            MAX_LOAD_CURRENT => as_word(c.max_load_current_ma),
            MAX_LOAD_TIME_TO_EMPTY => {
                time_to_empty(c.remaining_capacity_mah, c.max_load_current_ma)
            }
            AVERAGE_POWER => as_word(average_power(c.voltage_mv, c.average_current_ma)),
            INTERNAL_TEMPERATURE => c.temperature_dk,
            CYCLE_COUNT => c.cycle_count,
            STATE_OF_CHARGE => c.state_of_charge,
            STATE_OF_HEALTH => c.state_of_health,
            CHARGING_VOLTAGE => c.charging_voltage_mv,
            CHARGING_CURRENT => c.charging_current_ma,
            OPERATION_STATUS => c.operation_status,
            DESIGN_CAPACITY => c.design_capacity_mah,
            // `RawCoulombCount()` (`0x22`). ASSUMPTION: a counter cleared at the gauge's last
            // reset; nothing flows through this model's cell, so it stays zero. And the gaps.
            _ => 0,
        }
    }

    fn stored(&self, command: u8) -> u8 {
        self.store.get(usize::from(command)).copied().unwrap_or(0)
    }

    fn stored_word(&self, low: u8) -> u16 {
        u16::from_le_bytes([self.stored(low), self.stored(low.wrapping_add(1))])
    }

    fn at_rate(&self) -> i16 {
        i16::from_le_bytes(self.stored_word(AT_RATE).to_le_bytes())
    }

    /// The command byte after the address: the pointer, or a refusal above `0x6B`.
    fn write_command(&mut self, command: u8, at: Nanos) -> bool {
        if command > LAST_COMMAND {
            self.diagnostics
                .report(at, I2cDiagnosticKind::NoSuchRegister { register: command });
            self.phase = Phase::Ignoring;
            return false;
        }
        self.pointer = command;
        self.phase = Phase::Writing;
        true
    }

    /// A data byte at the pointer: stored and acknowledged where the command is writable,
    /// refused elsewhere (research §3.1). "Write transfers occur on the acknowledge": a
    /// subcommand runs once its high byte is in.
    fn write_data(&mut self, byte: u8, at: Nanos) -> bool {
        let command = self.pointer;
        if !is_writable(command) {
            self.diagnostics
                .report(at, I2cDiagnosticKind::ReadOnly { register: command });
            return false;
        }
        if let Some(slot) = self.store.get_mut(usize::from(command)) {
            *slot = byte;
        }
        self.pointer = command.wrapping_add(1);
        match command {
            CONTROL_HIGH => {
                if self.execute(self.stored_word(CONTROL), at) {
                    self.response_moved = true;
                }
            }
            MAC_CONTROL_HIGH => self.manufacturer_access(at),
            MAC_DATA_LEN => self.diagnostics.report_once(
                at,
                I2cDiagnosticKind::NotModelled {
                    what: "a MACData() block write at data-memory address",
                    value: u32::from(self.stored_word(MAC_CONTROL)),
                },
            ),
            _ => {}
        }
        true
    }

    /// A word written to `ManufacturerAccessControl()`: a subcommand, or the address of a
    /// data-memory block, which is not modelled.
    fn manufacturer_access(&mut self, at: Nanos) {
        let word = self.stored_word(MAC_CONTROL);
        if word >= DATA_MEMORY_FIRST {
            self.diagnostics.report_once(
                at,
                I2cDiagnosticKind::NotModelled {
                    what: "data memory",
                    value: u32::from(word),
                },
            );
        } else {
            // Through this command the response is read from `MACData()` directly: `Control()`
            // keeps reading its status.
            self.execute(word, at);
        }
    }

    /// Runs `subcommand`: the two identification subcommands answer through `MACData()`,
    /// `CONTROL_STATUS` does nothing, every other one is reported and has no effect. Whether the
    /// subcommand put a response in `MACData()`.
    fn execute(&mut self, subcommand: u16, at: Nanos) -> bool {
        match subcommand {
            CONTROL_STATUS => false,
            DEVICE_NUMBER => {
                self.respond(subcommand, &DEVICE_NUMBER_RESPONSE);
                true
            }
            FW_VERSION => {
                self.respond(subcommand, &FW_VERSION_RESPONSE);
                true
            }
            other => {
                self.diagnostics.report_once(
                    at,
                    I2cDiagnosticKind::NotModelled {
                        what: "a Control() subcommand",
                        value: u32::from(other),
                    },
                );
                false
            }
        }
    }

    /// Fills the `MACData()` block with the response to `subcommand`: the subcommand echoed in
    /// `ManufacturerAccessControl()`, `data`, zeros, the sum and the length (research §3.2).
    fn respond(&mut self, subcommand: u16, data: &[u8]) {
        let Some(block) = self.store.get_mut(MAC_BLOCK..) else {
            return;
        };
        block.fill(0);
        let echo = subcommand.to_le_bytes();
        for (slot, byte) in block.iter_mut().zip(echo) {
            *slot = byte;
        }
        for (slot, byte) in block.iter_mut().skip(MAC_DATA_OFFSET).zip(data) {
            *slot = *byte;
        }
        let sum = echo
            .iter()
            .chain(data)
            .fold(0_u8, |sum, byte| sum.wrapping_add(*byte));
        if let Some(slot) = block.get_mut(MAC_SUM_OFFSET) {
            // 255 − (sum mod 256): the one's complement of the byte sum (research §3.2).
            *slot = !sum;
        }
        if let Some(slot) = block.get_mut(MAC_LEN_OFFSET) {
            *slot = u8::try_from(data.len() + MAC_LEN_OVERHEAD).unwrap_or(u8::MAX);
        }
    }

    /// The byte at the pointer for the master; the pointer moves on when the master
    /// acknowledges it ("increments whenever data is acknowledged", research §3.1).
    fn read_data(&mut self, acknowledged: bool) -> u8 {
        let command = self.pointer;
        let value = self.byte_at(command);
        if command == CONTROL_HIGH {
            // The relocated response is read once: the word ends with its high byte.
            self.response_moved = false;
        }
        if acknowledged {
            self.pointer = command.wrapping_add(1);
        }
        value
    }
}

impl I2cTarget for Bq27220 {
    fn address(&mut self, address: u8, read: bool, _at: Nanos) -> bool {
        if address != ADDRESS {
            self.phase = Phase::Released;
            return false;
        }
        self.phase = if read { Phase::Reading } else { Phase::Pointer };
        true
    }

    fn write(&mut self, byte: u8, at: Nanos) -> bool {
        match self.phase {
            Phase::Pointer => self.write_command(byte, at),
            Phase::Writing => self.write_data(byte, at),
            Phase::Released | Phase::Reading | Phase::Ignoring => false,
        }
    }

    fn read(&mut self, acknowledged: bool, _at: Nanos) -> u8 {
        if self.phase == Phase::Reading {
            self.read_data(acknowledged)
        } else {
            // Not addressed for a read: the chip leaves SDA to its pull-up.
            u8::MAX
        }
    }

    fn stop(&mut self, _at: Nanos) {
        self.phase = Phase::Released;
    }
}

/// The commands a master may write (research §3.2): `Control()`, `AtRate()`, `Temperature()`
/// (ASSUMPTION: the gauge's WRTEMP option is set, so the written temperature is kept and read
/// back), the BTP thresholds and the `ManufacturerAccessControl()`/`MACData()` block.
const fn is_writable(command: u8) -> bool {
    matches!(
        command,
        CONTROL..=AT_RATE_HIGH
            | TEMPERATURE..=TEMPERATURE_HIGH
            | BTP_DISCHARGE_SET..=BTP_CHARGE_SET_HIGH
            | MAC_CONTROL..=MAC_DATA_LEN
    )
}

/// The low byte of `word` at an even command, the high byte at an odd one.
const fn half(word: u16, command: u8) -> u8 {
    let [low, high] = word.to_le_bytes();
    if command.is_multiple_of(2) { low } else { high }
}

/// A signed value as the gauge sends it: its two's complement bits.
const fn as_word(value: i16) -> u16 {
    u16::from_le_bytes(value.to_le_bytes())
}

/// Minutes until `capacity_mah` is spent at `current_ma`; [`NO_TIME`] when the current does not
/// discharge (research §3.2, `TimeToEmpty()`). 455 mAh at −15 mA gives the 1820 of research §3.5.
fn time_to_empty(capacity_mah: u16, current_ma: i16) -> u16 {
    if current_ma >= 0 {
        return NO_TIME;
    }
    minutes(capacity_mah, current_ma.unsigned_abs())
}

/// Minutes until `missing_mah` is charged at `current_ma`; [`NO_TIME`] when not charging.
fn time_to_full(missing_mah: u16, current_ma: i16) -> u16 {
    if current_ma <= 0 {
        return NO_TIME;
    }
    minutes(missing_mah, current_ma.unsigned_abs())
}

/// `capacity_mah` over `current_ma`, in minutes, below [`NO_TIME`], which means "no time".
fn minutes(capacity_mah: u16, current_ma: u16) -> u16 {
    let minutes = u32::from(capacity_mah) * MINUTES_PER_HOUR / u32::from(current_ma.max(1));
    u16::try_from(minutes).map_or(NO_TIME - 1, |m| m.min(NO_TIME - 1))
}

/// `AveragePower()`, mW, negative while discharging: voltage times average current.
fn average_power(voltage_mv: u16, current_ma: i16) -> i16 {
    let milliwatts = i32::from(voltage_mv) * i32::from(current_ma) / MILLI;
    i16::try_from(milliwatts).unwrap_or(if milliwatts < 0 { i16::MIN } else { i16::MAX })
}

#[cfg(test)]
mod tests;
