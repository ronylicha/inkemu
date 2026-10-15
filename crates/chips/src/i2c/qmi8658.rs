//! The QST QMI8658 six-axis motion sensor of the X3, at `0x6B` (`0x6A` with SA0 low; research
//! `sp3-x3-chips.md` §5).
//!
//! The model holds the register map of research §5.2: identification, CTRL1 to CTRL9 as storage,
//! a data-ready status, a sample counter, the temperature and the two sensors' output, scaled by
//! the full scale CTRL2 and CTRL3 select. The device lies still: its acceleration and angular rate
//! are those of a [`Qmi8658Config`]. Samples are counted from device time since the sensors were
//! enabled, at their output data rates, never from a host clock.
//!
//! What `CrossPoint` 1.6.5 does with it (research `sp3-x3-firmware.md` §6): `WHO_AM_I` at `0x6B`,
//! then `0x6A`; CTRL7 `00`, CTRL1 `40`, CTRL2 `06`, CTRL3 `56`, CTRL7 `03`; six bytes from `0x3B`
//! and from `0x35`; sleep with CTRL7 `00` and CTRL1 `41`. The FIFO, the motion engines, the
//! self-test and the CTRL9 commands other than the acknowledgement are kept without effect and
//! reported.

use emu_core::{I2cTarget, Nanos};

use super::{Diagnostics, I2cDiagnostic, I2cDiagnosticKind, Phase};

/// The address with SA0 pulled up, the X3's (research §5.1).
pub const ADDRESS: u8 = 0x6B;
/// The address with SA0 low or unconnected.
pub const ALTERNATE_ADDRESS: u8 = 0x6A;

const NAME: &str = "QMI8658";

// The register map (research §5.2).
const WHO_AM_I: u8 = 0x00;
const REVISION_ID: u8 = 0x01;
const CTRL1: u8 = 0x02;
const CTRL2: u8 = 0x03;
const CTRL3: u8 = 0x04;
const CTRL4: u8 = 0x05;
const CTRL5: u8 = 0x06;
const CTRL6: u8 = 0x07;
const CTRL7: u8 = 0x08;
const CTRL8: u8 = 0x09;
const CTRL9: u8 = 0x0A;
const CAL_FIRST: u8 = 0x0B;
const CAL_LAST: u8 = 0x12;
const FIFO_WTM_TH: u8 = 0x13;
const FIFO_CTRL: u8 = 0x14;
const STATUS0: u8 = 0x2E;
const TIMESTAMP_L: u8 = 0x30;
const TIMESTAMP_H: u8 = 0x32;
const TEMP_L: u8 = 0x33;
const TEMP_H: u8 = 0x34;
const ACCEL_FIRST: u8 = 0x35;
const ACCEL_LAST: u8 = 0x3A;
const GYRO_FIRST: u8 = 0x3B;
const GYRO_LAST: u8 = 0x40;
const RST_RESULT: u8 = 0x4D;
const RESET: u8 = 0x60;
/// The last register of the map: a pointer past it names nothing.
const LAST_REGISTER: u8 = RESET;
const REGISTER_SPACE: usize = 0x61;

const WHO_AM_I_VALUE: u8 = 0x05;
const CTRL1_POWER_ON: u8 = 0x20;
/// CTRL1: address auto-increment, and the sensors' oscillator off.
const AUTO_INCREMENT: u8 = 0x40;
const SENSOR_DISABLE: u8 = 0x01;
/// CTRL2 and CTRL3: the self-test bit, the full scale (bits 6:4) and the output data rate.
const SELF_TEST: u8 = 0x80;
const FULL_SCALE_SHIFT: u8 = 4;
const FULL_SCALE_FIELD: u8 = 0x07;
const ODR_FIELD: u8 = 0x0F;
/// CTRL7: the accelerometer and the gyroscope enables; the magnetometer and the `AttitudeEngine`.
const ACCEL_ENABLE: u8 = 0x01;
const GYRO_ENABLE: u8 = 0x02;
const MAGNETOMETER_OR_ATTITUDE: u8 = 0x0C;
/// STATUS0: new accelerometer data, new gyroscope data.
const ACCEL_READY: u8 = 0x01;
const GYRO_READY: u8 = 0x02;
/// The CTRL9 command the model takes: NOP, and the acknowledgement of the A variant (§5.3).
const CTRL9_NOP: u8 = 0x00;
/// RESET written with this starts a soft reset; `RST_RESULT` then reads [`RESET_DONE`].
const SOFT_RESET: u8 = 0xB0;
const RESET_DONE: u8 = 0x80;

/// The accelerometer's sensitivity at ±2 g, halved at each wider full scale (research §5.2).
const ACCEL_LSB_PER_G: i64 = 16_384;
/// The widest accelerometer full scale, ±16 g; codes 4 to 7 are not in the datasheet.
const ACCEL_WIDEST_SCALE: u8 = 3;
/// The gyroscope's sensitivity at ±16 dps, halved at each wider full scale up to ±2048 dps.
const GYRO_LSB_PER_DPS: i64 = 2_048;
/// The temperature's sensitivity: 256 LSB per degree Celsius.
const TEMP_LSB_PER_DEGREE: i64 = 256;
const MILLI: i64 = 1_000;
const CENTI: i64 = 100;

/// The fastest output data rate, code 0, in millihertz; code `n` up to 8 runs at that over `2^n`.
const FASTEST_ODR_MILLIHERTZ: u64 = 8_000_000;
const LAST_HALVING_ODR_CODE: u8 = 8;
/// The accelerometer's low-power rates, codes `C` to `F` (research §5.2): 128, 21, 11 and 3 Hz.
const LOW_POWER_ODR_MILLIHERTZ: [u64; 4] = [128_000, 21_000, 11_000, 3_000];
const FIRST_LOW_POWER_ODR_CODE: u8 = 0x0C;
/// One second in nanoseconds times one hertz in millihertz: samples = ns × mHz / this.
const NANOS_MILLIHERTZ_PER_SAMPLE: u128 = 1_000_000_000_000;
/// TIMESTAMP is a 24-bit counter that wraps (research §5.2).
const TIMESTAMP_MASK: u64 = 0x00FF_FFFF;

/// What the sensor reports. The defaults describe a reader lying still, face up on a table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Qmi8658Config {
    /// The seven-bit address, `0x6B` (SA0 high, the X3's profile) or `0x6A`.
    pub address: u8,
    /// `REVISION_ID`. ASSUMPTION: `0x68`, the datasheet's Table 20, which its Table 23 contradicts
    /// with `0x79` (research §5.2); a read of a real X3 would settle it.
    pub revision_id: u8,
    /// The acceleration on X, Y and Z, in thousandths of g. ASSUMPTION: (0, 0, 1000), the device
    /// face up with the chip's Z axis pointing up; no source gives how the chip is mounted (research
    /// §5.5).
    pub acceleration_mg: [i32; 3],
    /// The angular rate on X, Y and Z, in thousandths of a degree per second: (0, 0, 0), still.
    pub angular_rate_mdps: [i32; 3],
    /// The temperature, in hundredths of a degree Celsius. ASSUMPTION: 2500, 25.00 °C.
    pub temperature_centi: i32,
}

impl Default for Qmi8658Config {
    fn default() -> Qmi8658Config {
        Qmi8658Config {
            address: ADDRESS,
            revision_id: 0x68,
            acceleration_mg: [0, 0, 1_000],
            angular_rate_mdps: [0, 0, 0],
            temperature_centi: 2_500,
        }
    }
}

/// Why a [`Qmi8658Config`] is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Qmi8658ConfigError {
    #[error(
        "the QMI8658 answers only 0x6A or 0x6B (its SA0 pin low or high), not {0:#04x}; give one \
         of those two"
    )]
    Address(u8),
}

/// A count of samples at a rate that changes: the samples before the last change, and the time
/// since it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SampleCounter {
    /// Samples counted before `since`.
    base: u64,
    /// When the current rate began; `None` while nothing is sampled.
    since: Option<Nanos>,
    millihertz: u64,
}

impl SampleCounter {
    fn count(&self, at: Nanos) -> u64 {
        let Some(since) = self.since else {
            return self.base;
        };
        let elapsed = u128::from(at.get().saturating_sub(since.get()));
        let samples = elapsed * u128::from(self.millihertz) / NANOS_MILLIHERTZ_PER_SAMPLE;
        self.base
            .saturating_add(u64::try_from(samples).unwrap_or(u64::MAX))
    }

    /// From `at` on, samples at `millihertz`, or none.
    fn retime(&mut self, at: Nanos, millihertz: Option<u64>) {
        self.base = self.count(at);
        self.since = millihertz.map(|_| at);
        self.millihertz = millihertz.unwrap_or(0);
    }
}

/// One of the two sensors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sensor {
    Accelerometer,
    Gyroscope,
}

/// The QMI8658 on an I2C bus.
#[derive(Clone, Debug)]
pub struct Qmi8658 {
    config: Qmi8658Config,
    phase: Phase,
    pointer: u8,
    /// The writable registers as written (CTRL1 to CTRL9, CAL, the FIFO settings); the others
    /// read from the state below.
    registers: [u8; REGISTER_SPACE],
    accel_samples: SampleCounter,
    gyro_samples: SampleCounter,
    /// TIMESTAMP: samples at the fastest enabled rate.
    timestamp: SampleCounter,
    /// The sample counts at the last read of STATUS0: data is new when the count has moved on.
    accel_seen: u64,
    gyro_seen: u64,
    /// The output a disabled sensor keeps: its last value while it was enabled.
    accel_held: [i16; 3],
    gyro_held: [i16; 3],
    reset_done: bool,
    diagnostics: Diagnostics,
}

impl Qmi8658 {
    /// A sensor at power-on, as `config` describes it.
    ///
    /// # Errors
    /// [`Qmi8658ConfigError`] for an address other than `0x6A` and `0x6B`.
    pub fn new(config: Qmi8658Config) -> Result<Qmi8658, Qmi8658ConfigError> {
        if !matches!(config.address, ADDRESS | ALTERNATE_ADDRESS) {
            return Err(Qmi8658ConfigError::Address(config.address));
        }
        Ok(Qmi8658 {
            config,
            phase: Phase::Released,
            pointer: WHO_AM_I,
            registers: power_on_registers(),
            accel_samples: SampleCounter::default(),
            gyro_samples: SampleCounter::default(),
            timestamp: SampleCounter::default(),
            accel_seen: 0,
            gyro_seen: 0,
            accel_held: [0; 3],
            gyro_held: [0; 3],
            reset_done: false,
            diagnostics: Diagnostics::new(NAME),
        })
    }

    /// What the sensor reports.
    #[must_use]
    pub const fn config(&self) -> &Qmi8658Config {
        &self.config
    }

    /// What `register` reads at `at`, without the side effects of a read (STATUS0 is not
    /// cleared); 0 for a register outside the map.
    #[must_use]
    pub fn register(&self, register: u8, at: Nanos) -> u8 {
        match register {
            WHO_AM_I => WHO_AM_I_VALUE,
            REVISION_ID => self.config.revision_id,
            STATUS0 => self.status0(at),
            TIMESTAMP_L..=TIMESTAMP_H => {
                let count = self.timestamp.count(at) & TIMESTAMP_MASK;
                byte_of(count.to_le_bytes(), register - TIMESTAMP_L)
            }
            TEMP_L | TEMP_H => byte_of(self.temperature().to_le_bytes(), register - TEMP_L),
            ACCEL_FIRST..=ACCEL_LAST => {
                axis_byte(self.output(Sensor::Accelerometer), register - ACCEL_FIRST)
            }
            GYRO_FIRST..=GYRO_LAST => {
                axis_byte(self.output(Sensor::Gyroscope), register - GYRO_FIRST)
            }
            RST_RESULT => {
                if self.reset_done {
                    RESET_DONE
                } else {
                    // ASSUMPTION: no reset has completed since power-on; no source gives the
                    // value before the first one.
                    0
                }
            }
            other => self.stored(other),
        }
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

    fn stored(&self, register: u8) -> u8 {
        self.registers
            .get(usize::from(register))
            .copied()
            .unwrap_or(0)
    }

    fn sensors_on(&self) -> bool {
        self.stored(CTRL1) & SENSOR_DISABLE == 0
    }

    fn enabled(&self, sensor: Sensor) -> bool {
        let bit = match sensor {
            Sensor::Accelerometer => ACCEL_ENABLE,
            Sensor::Gyroscope => GYRO_ENABLE,
        };
        self.sensors_on() && self.stored(CTRL7) & bit != 0
    }

    /// The sample rate of `sensor` while it is enabled; `None` while it is not, or at a rate code
    /// the datasheet does not list.
    fn rate(&self, sensor: Sensor) -> Option<u64> {
        if !self.enabled(sensor) {
            return None;
        }
        match sensor {
            Sensor::Accelerometer => odr_millihertz(self.stored(CTRL2) & ODR_FIELD, true),
            Sensor::Gyroscope => odr_millihertz(self.stored(CTRL3) & ODR_FIELD, false),
        }
    }

    /// STATUS0: a sensor's bit is set once it has produced a sample since the last read.
    fn status0(&self, at: Nanos) -> u8 {
        let accel = self.accel_samples.count(at) > self.accel_seen;
        let gyro = self.gyro_samples.count(at) > self.gyro_seen;
        (if accel { ACCEL_READY } else { 0 }) | (if gyro { GYRO_READY } else { 0 })
    }

    /// The output registers of `sensor`: the configured motion at the current full scale while
    /// it is enabled, the last such value otherwise.
    fn output(&self, sensor: Sensor) -> [i16; 3] {
        match (sensor, self.enabled(sensor)) {
            (Sensor::Accelerometer, true) => {
                let scale = ((self.stored(CTRL2) >> FULL_SCALE_SHIFT) & FULL_SCALE_FIELD)
                    .min(ACCEL_WIDEST_SCALE);
                let lsb = ACCEL_LSB_PER_G >> scale;
                self.config.acceleration_mg.map(|mg| scaled(mg, lsb))
            }
            (Sensor::Gyroscope, true) => {
                let scale = (self.stored(CTRL3) >> FULL_SCALE_SHIFT) & FULL_SCALE_FIELD;
                let lsb = GYRO_LSB_PER_DPS >> scale;
                self.config.angular_rate_mdps.map(|mdps| scaled(mdps, lsb))
            }
            (Sensor::Accelerometer, false) => self.accel_held,
            (Sensor::Gyroscope, false) => self.gyro_held,
        }
    }

    /// TEMP: degrees Celsius times 256, `TEMP_H + TEMP_L / 256` (research §5.2). ASSUMPTION: the
    /// temperature reads the configured value at all times; no source says whether it freezes
    /// with the sensors, and it does not change in this model.
    fn temperature(&self) -> i16 {
        saturate(rounded_div(
            i64::from(self.config.temperature_centi) * TEMP_LSB_PER_DEGREE,
            CENTI,
        ))
    }

    /// A read at `register`: [`Qmi8658::register`], plus what a read does to the chip.
    fn read_register(&mut self, register: u8, at: Nanos) -> u8 {
        let value = self.register(register, at);
        match register {
            STATUS0 => {
                self.accel_seen = self.accel_samples.count(at);
                self.gyro_seen = self.gyro_samples.count(at);
            }
            ACCEL_FIRST..=ACCEL_LAST if !self.enabled(Sensor::Accelerometer) => {
                self.report_disabled_read(ACCEL_FIRST, at);
            }
            GYRO_FIRST..=GYRO_LAST if !self.enabled(Sensor::Gyroscope) => {
                self.report_disabled_read(GYRO_FIRST, at);
            }
            _ => {}
        }
        value
    }

    /// ASSUMPTION: a disabled sensor's registers keep their last value (research §5.5 leaves it
    /// open); the read is reported once per sensor, so that a firmware reading stale data is
    /// seen.
    fn report_disabled_read(&mut self, first_register: u8, at: Nanos) {
        self.diagnostics.report_once(
            at,
            I2cDiagnosticKind::NotModelled {
                what: "a disabled sensor read, from register",
                value: u32::from(first_register),
            },
        );
    }

    /// A write at `register`.
    fn write_register(&mut self, register: u8, byte: u8, at: Nanos) {
        match register {
            CTRL1 | CTRL2 | CTRL3 | CTRL7 => self.write_sampling(register, byte, at),
            CTRL4 | CTRL5 | CAL_FIRST..=CAL_LAST => self.store(register, byte),
            CTRL6 | CTRL8 => {
                self.store(register, byte);
                self.report_feature(
                    byte != 0,
                    "the motion features of CTRL6 and CTRL8",
                    byte,
                    at,
                );
            }
            FIFO_WTM_TH | FIFO_CTRL => {
                self.store(register, byte);
                self.report_feature(byte != 0, "the FIFO", byte, at);
            }
            CTRL9 => {
                self.store(register, byte);
                if byte != CTRL9_NOP {
                    self.diagnostics.report_once(
                        at,
                        I2cDiagnosticKind::NotModelled {
                            what: "the CTRL9 command",
                            value: u32::from(byte),
                        },
                    );
                }
            }
            RESET if byte == SOFT_RESET => self.soft_reset(),
            RESET => self.diagnostics.report(
                at,
                I2cDiagnosticKind::Undefined {
                    register,
                    value: byte,
                },
            ),
            // ASSUMPTION: the sensor acknowledges a write to a read-only register, as most
            // register-mapped I2C chips do; the datasheet does not say.
            _ => self
                .diagnostics
                .report(at, I2cDiagnosticKind::ReadOnly { register }),
        }
    }

    /// CTRL1, CTRL2, CTRL3 or CTRL7: what is sampled changes, so the counters are brought to `at`
    /// at the old rates first, and a sensor being disabled keeps its last output.
    fn write_sampling(&mut self, register: u8, byte: u8, at: Nanos) {
        let (accel, gyro) = (
            self.output(Sensor::Accelerometer),
            self.output(Sensor::Gyroscope),
        );
        if self.enabled(Sensor::Accelerometer) {
            self.accel_held = accel;
        }
        if self.enabled(Sensor::Gyroscope) {
            self.gyro_held = gyro;
        }
        self.store(register, byte);
        self.retime(at);
        match register {
            CTRL2 | CTRL3 => {
                self.report_feature(byte & SELF_TEST != 0, "the self-test", byte, at);
                self.check_rate(register, byte, at);
            }
            CTRL7 => self.report_feature(
                byte & MAGNETOMETER_OR_ATTITUDE != 0,
                "the magnetometer and the AttitudeEngine",
                byte,
                at,
            ),
            _ => {}
        }
    }

    /// Reports a full scale or a rate code the datasheet does not list. The model keeps the
    /// value: an unlisted rate samples nothing, an unlisted accelerometer full scale reads as
    /// ±16 g.
    fn check_rate(&mut self, register: u8, byte: u8, at: Nanos) {
        let accelerometer = register == CTRL2;
        let unlisted_scale =
            accelerometer && (byte >> FULL_SCALE_SHIFT) & FULL_SCALE_FIELD > ACCEL_WIDEST_SCALE;
        let unlisted_rate = odr_millihertz(byte & ODR_FIELD, accelerometer).is_none();
        if unlisted_scale || unlisted_rate {
            self.diagnostics.report(
                at,
                I2cDiagnosticKind::Undefined {
                    register,
                    value: byte,
                },
            );
        }
    }

    fn report_feature(&mut self, used: bool, what: &'static str, byte: u8, at: Nanos) {
        if used {
            self.diagnostics.report_once(
                at,
                I2cDiagnosticKind::NotModelled {
                    what,
                    value: u32::from(byte),
                },
            );
        }
    }

    /// Brings the three counters to `at` and sets their rates from the current registers.
    fn retime(&mut self, at: Nanos) {
        let accel = self.rate(Sensor::Accelerometer);
        let gyro = self.rate(Sensor::Gyroscope);
        self.accel_samples.retime(at, accel);
        self.gyro_samples.retime(at, gyro);
        self.timestamp.retime(at, accel.max(gyro));
    }

    fn store(&mut self, register: u8, byte: u8) {
        if let Some(slot) = self.registers.get_mut(usize::from(register)) {
            *slot = byte;
        }
    }

    /// RESET `0xB0`: every register back to its power-on value, the counters and the held
    /// output cleared; `RST_RESULT` reads `0x80` afterwards (research §5.2). ASSUMPTION: the
    /// reset completes at once; the datasheet's 1.75 s turn-on is not modelled, and `SensorLib`
    /// polls `RST_RESULT`, which then answers at the first poll.
    fn soft_reset(&mut self) {
        self.registers = power_on_registers();
        self.accel_samples = SampleCounter::default();
        self.gyro_samples = SampleCounter::default();
        self.timestamp = SampleCounter::default();
        self.accel_seen = 0;
        self.gyro_seen = 0;
        self.accel_held = [0; 3];
        self.gyro_held = [0; 3];
        self.reset_done = true;
    }

    /// The pointer after an access: up by one with auto-increment (CTRL1 bit 6), unchanged
    /// without. ASSUMPTION: without auto-increment a multi-byte read repeats its register; the
    /// datasheet does not say (research §5.1, §6 point 7), and a capture of a burst read on a real
    /// X3 with CTRL1 `0x00` would settle it.
    fn advance(&mut self) {
        if self.stored(CTRL1) & AUTO_INCREMENT != 0 {
            self.pointer = self.pointer.wrapping_add(1);
        }
    }
}

impl I2cTarget for Qmi8658 {
    fn address(&mut self, address: u8, read: bool, _at: Nanos) -> bool {
        if address != self.config.address {
            self.phase = Phase::Released;
            return false;
        }
        self.phase = if read { Phase::Reading } else { Phase::Pointer };
        true
    }

    fn write(&mut self, byte: u8, at: Nanos) -> bool {
        match self.phase {
            Phase::Pointer => {
                if byte > LAST_REGISTER {
                    // ASSUMPTION: acknowledged (the datasheet describes no NACK), the bytes that
                    // follow dropped. Bit 7 of the address, the second auto-increment of the C
                    // datasheet (research §5.1), is not taken.
                    self.diagnostics
                        .report(at, I2cDiagnosticKind::NoSuchRegister { register: byte });
                    self.phase = Phase::Ignoring;
                } else {
                    self.pointer = byte;
                    self.phase = Phase::Writing;
                }
                true
            }
            Phase::Writing => {
                self.write_register(self.pointer, byte, at);
                self.advance();
                true
            }
            Phase::Ignoring => true,
            Phase::Released | Phase::Reading => false,
        }
    }

    fn read(&mut self, _acknowledged: bool, at: Nanos) -> u8 {
        if self.phase != Phase::Reading {
            // Not addressed for a read: the chip leaves SDA to its pull-up.
            return u8::MAX;
        }
        let value = self.read_register(self.pointer, at);
        self.advance();
        value
    }

    fn stop(&mut self, _at: Nanos) {
        self.phase = Phase::Released;
    }
}

/// The registers at power-on and after a soft reset: zero but CTRL1 (research §5.2).
fn power_on_registers() -> [u8; REGISTER_SPACE] {
    let mut registers = [0; REGISTER_SPACE];
    if let Some(slot) = registers.get_mut(usize::from(CTRL1)) {
        *slot = CTRL1_POWER_ON;
    }
    registers
}

/// The rate of an output data rate code, in millihertz (research §5.2): codes 0 to 8 run at
/// 8000 Hz halved at each step down to 31.25 Hz; `C` to `F` are the accelerometer's low-power
/// rates. `None` for a code the datasheet does not list for that sensor.
fn odr_millihertz(code: u8, accelerometer: bool) -> Option<u64> {
    if code <= LAST_HALVING_ODR_CODE {
        return Some(FASTEST_ODR_MILLIHERTZ >> code);
    }
    if !accelerometer {
        return None;
    }
    let low_power = code.checked_sub(FIRST_LOW_POWER_ODR_CODE)?;
    LOW_POWER_ODR_MILLIHERTZ
        .get(usize::from(low_power))
        .copied()
}

/// `milli` thousandths of a unit at `lsb_per_unit`, rounded to the nearest and held in 16 bits.
fn scaled(milli: i32, lsb_per_unit: i64) -> i16 {
    saturate(rounded_div(i64::from(milli) * lsb_per_unit, MILLI))
}

/// `numerator / denominator` rounded to the nearest, halves away from zero; `denominator` > 0.
fn rounded_div(numerator: i64, denominator: i64) -> i64 {
    let half = denominator / 2;
    if numerator >= 0 {
        (numerator + half) / denominator
    } else {
        (numerator - half) / denominator
    }
}

/// `value` clamped to what a 16-bit output register holds.
fn saturate(value: i64) -> i16 {
    i16::try_from(value).unwrap_or(if value < 0 { i16::MIN } else { i16::MAX })
}

/// Byte `index` of `bytes`, 0 past the end.
fn byte_of<const N: usize>(bytes: [u8; N], index: u8) -> u8 {
    bytes.get(usize::from(index)).copied().unwrap_or(0)
}

/// The byte at `offset` of three little-endian axes: X low, X high, Y low, and so on.
fn axis_byte(axes: [i16; 3], offset: u8) -> u8 {
    axes.get(usize::from(offset / 2))
        .map_or(0, |axis| byte_of(axis.to_le_bytes(), offset % 2))
}

#[cfg(test)]
mod tests;
