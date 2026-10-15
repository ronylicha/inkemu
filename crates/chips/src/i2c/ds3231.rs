//! The Maxim DS3231 real-time clock of the X3, at `0x68` (research `sp3-x3-chips.md` §4).
//!
//! The clock runs from the configured start by device time: at `at` it reads the start plus the
//! whole seconds elapsed since power-on, never the host's clock, so a run reads the same time
//! wherever and whenever it is replayed. The registers are those of the datasheet's map (§4.1):
//! BCD time and date with a 12/24-hour mode and a century bit, the alarms (stored, not matched),
//! control, status, aging and the temperature. A read is served from a copy of the time taken at
//! each START and when the pointer wraps to `0x00` (§4.2), so the seven time bytes of one burst
//! are consistent.
//!
//! What `CrossPoint` 1.6.5 does with it (research `sp3-x3-firmware.md` §5): reads Status, writes
//! Control `0x04`, refuses the time while OSF is set, reads seven bytes from `0x00`; setting the
//! time writes seven bytes from `0x00`, then clears OSF.

use emu_core::{I2cTarget, Nanos};

use super::{Diagnostics, I2cDiagnostic, I2cDiagnosticKind, Phase};

/// The clock's seven-bit address, fixed (research §4.2).
pub const ADDRESS: u8 = 0x68;

const NAME: &str = "DS3231";

// The register map (research §4.1).
const SECONDS: u8 = 0x00;
const MINUTES: u8 = 0x01;
const HOURS: u8 = 0x02;
const WEEKDAY: u8 = 0x03;
const DATE: u8 = 0x04;
const MONTH: u8 = 0x05;
const YEAR: u8 = 0x06;
const ALARM_FIRST: u8 = 0x07;
const ALARM_LAST: u8 = 0x0D;
const CONTROL: u8 = 0x0E;
const STATUS: u8 = 0x0F;
const AGING: u8 = 0x10;
const TEMPERATURE_MSB: u8 = 0x11;
const TEMPERATURE_LSB: u8 = 0x12;
/// Past it the pointer wraps to `0x00`.
const LAST_REGISTER: u8 = TEMPERATURE_LSB;
const TIME_REGISTERS: usize = 7;
const ALARM_REGISTERS: usize = 7;

// Control (research §4.1): power-on value and bits.
const CONTROL_POWER_ON: u8 = 0x1C;
const CONV: u8 = 0x20;
const INTCN: u8 = 0x04;
const A2IE: u8 = 0x02;
const A1IE: u8 = 0x01;

// Status: bits.
const OSF: u8 = 0x80;
const EN32KHZ: u8 = 0x08;
const BSY: u8 = 0x04;
const A2F: u8 = 0x02;
const A1F: u8 = 0x01;
/// The status bits a write can only clear.
const CLEAR_ONLY: u8 = OSF | A2F | A1F;

// The hours register.
const TWELVE_HOUR: u8 = 0x40;
const PM: u8 = 0x20;
const HOURS_PER_HALF_DAY: u8 = 12;
// The month register.
const CENTURY: u8 = 0x80;
const MONTH_FIELD: u8 = 0x1F;

/// How long a temperature conversion forced by CONV keeps BSY and CONV set: the datasheet gives
/// 125 to 200 ms (`tCONV`, research §4.1); the model takes the longest.
const CONVERSION_NANOS: u64 = 200 * NANOS_PER_MILLISECOND;
const NANOS_PER_MILLISECOND: u64 = 1_000_000;
const NANOS_PER_SECOND: u64 = 1_000_000_000;
const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 3_600;
const SECONDS_PER_DAY: u64 = 86_400;
const DAYS_PER_WEEK: u64 = 7;
const YEARS_PER_CENTURY: u16 = 100;

/// The days from 2000-01-01 to 2200-01-01: the range the two-digit year and the century bit
/// cover, after which the clock wraps to 2000.
const DAYS_IN_RANGE: u64 = 73_049;
const SECONDS_IN_RANGE: u64 = DAYS_IN_RANGE * SECONDS_PER_DAY;
/// The days from 0000-03-01 of the proleptic Gregorian calendar to 2000-01-01, the origin of the
/// civil-date algorithm below (Howard Hinnant's `days_from_civil`).
const DAYS_TO_2000: u64 = 730_425;
const DAYS_PER_400_YEARS: u64 = 146_097;
/// 2000-01-01 was a Saturday, day 6 counting Monday as 1.
const WEEKDAY_OF_2000_OFFSET: u64 = 5;

/// A time of the clock: 2000-01-01T00:00:00 to 2199-12-31T23:59:59, the range of its
/// registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// Why a [`DateTime`] is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DateTimeError {
    #[error(
        "the year {0} is outside the DS3231's range; give a year from 2000 to 2199, what its \
         two-digit year and century bit hold"
    )]
    Year(u16),
    #[error("there is no month {0}; give a month from 1 to 12")]
    Month(u8),
    #[error("{year}-{month:02} has no day {day}; give a day of that month")]
    Day { year: u16, month: u8, day: u8 },
    #[error("there is no hour {0}; give an hour from 0 to 23")]
    Hour(u8),
    #[error("there is no minute {0}; give a minute from 0 to 59")]
    Minute(u8),
    #[error("there is no second {0}; give a second from 0 to 59 (the DS3231 has no leap second)")]
    Second(u8),
}

impl DateTime {
    /// The first year the clock holds.
    pub const FIRST_YEAR: u16 = 2000;
    /// The last year the clock holds.
    pub const LAST_YEAR: u16 = 2199;

    /// A valid time.
    ///
    /// # Errors
    /// [`DateTimeError`] for a field out of its range.
    pub fn new(
        year: u16,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
    ) -> Result<DateTime, DateTimeError> {
        let time = DateTime {
            year,
            month,
            day,
            hour,
            minute,
            second,
        };
        time.validate()?;
        Ok(time)
    }

    /// Whether every field is in its range.
    ///
    /// # Errors
    /// [`DateTimeError`] for the first field out of its range.
    pub fn validate(&self) -> Result<(), DateTimeError> {
        if !(DateTime::FIRST_YEAR..=DateTime::LAST_YEAR).contains(&self.year) {
            return Err(DateTimeError::Year(self.year));
        }
        if !(1..=12).contains(&self.month) {
            return Err(DateTimeError::Month(self.month));
        }
        if self.day == 0 || self.day > days_in_month(self.year, self.month) {
            return Err(DateTimeError::Day {
                year: self.year,
                month: self.month,
                day: self.day,
            });
        }
        if self.hour > 23 {
            return Err(DateTimeError::Hour(self.hour));
        }
        if self.minute > 59 {
            return Err(DateTimeError::Minute(self.minute));
        }
        if self.second > 59 {
            return Err(DateTimeError::Second(self.second));
        }
        Ok(())
    }

    /// The time `seconds` after 2000-01-01T00:00:00, `None` past 2199-12-31T23:59:59.
    #[must_use]
    pub fn from_seconds_since_2000(seconds: u64) -> Option<DateTime> {
        if seconds >= SECONDS_IN_RANGE {
            return None;
        }
        let (year, month, day) = civil_from_days(seconds / SECONDS_PER_DAY)?;
        let in_day = seconds % SECONDS_PER_DAY;
        Some(DateTime {
            year,
            month,
            day,
            hour: u8::try_from(in_day / SECONDS_PER_HOUR).ok()?,
            minute: u8::try_from(in_day % SECONDS_PER_HOUR / SECONDS_PER_MINUTE).ok()?,
            second: u8::try_from(in_day % SECONDS_PER_MINUTE).ok()?,
        })
    }

    /// The seconds since 2000-01-01T00:00:00 of a valid time.
    #[must_use]
    pub fn seconds_since_2000(&self) -> u64 {
        days_since_2000(self.year, self.month, self.day) * SECONDS_PER_DAY
            + u64::from(self.hour) * SECONDS_PER_HOUR
            + u64::from(self.minute) * SECONDS_PER_MINUTE
            + u64::from(self.second)
    }

    /// The day of the week, Monday 1 to Sunday 7: the convention `CrossPoint` writes (research
    /// `sp3-x3-firmware.md` §5).
    #[must_use]
    pub fn weekday(&self) -> u8 {
        weekday_of_day(
            days_since_2000(self.year, self.month, self.day),
            WEEKDAY_OF_2000_OFFSET,
        )
    }
}

/// What the clock holds at power-on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ds3231Config {
    /// The time at power-on (device time 0). ASSUMPTION: 2026-01-01T00:00:00, a clock already set
    /// by its owner; the profile or the front end gives the real one.
    pub start: DateTime,
    /// Whether the oscillator stopped before power-on (OSF set, Status `0x88`): a clock never
    /// set, or whose backup battery ran out. ASSUMPTION: `false`, a battery-backed clock already
    /// set (Status `0x08`, research §4.4).
    pub oscillator_stopped: bool,
    /// The temperature registers, in quarters of a degree Celsius, −512 to 511. ASSUMPTION: 100,
    /// 25.00 °C (research §4.4).
    pub temperature_quarters: i16,
}

impl Default for Ds3231Config {
    fn default() -> Ds3231Config {
        Ds3231Config {
            start: DateTime {
                year: 2026,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
            },
            oscillator_stopped: false,
            temperature_quarters: 100,
        }
    }
}

/// Why a [`Ds3231Config`] is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Ds3231ConfigError {
    #[error("the start time is invalid: {0}")]
    Start(#[from] DateTimeError),
    #[error(
        "{0} quarters of a degree do not fit the DS3231's 10-bit temperature; give a temperature \
         from −512 to 511 quarters (−128.00 to 127.75 °C)"
    )]
    Temperature(i16),
}

/// The lowest and highest temperatures the two registers hold, in quarters of a degree.
const TEMPERATURE_RANGE: std::ops::RangeInclusive<i16> = -512..=511;

/// The running clock: the time at an instant whose sub-second phase is zero, and the offset that
/// gives the day-of-week counter from the day count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Clock {
    /// Seconds since 2000 at `base_at`.
    base_seconds: u64,
    /// An instant at which a second of the clock began.
    base_at: Nanos,
    /// The day-of-week register is a counter of its own, incremented at midnight (research §4.1):
    /// its value is `(days since 2000 + offset) mod 7 + 1`.
    weekday_offset: u64,
}

impl Clock {
    fn seconds(&self, at: Nanos) -> u64 {
        let elapsed = at.get().saturating_sub(self.base_at.get()) / NANOS_PER_SECOND;
        (self.base_seconds % SECONDS_IN_RANGE + elapsed % SECONDS_IN_RANGE) % SECONDS_IN_RANGE
    }

    fn now(&self, at: Nanos) -> DateTime {
        DateTime::from_seconds_since_2000(self.seconds(at)).unwrap_or(DateTime {
            year: DateTime::FIRST_YEAR,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
        })
    }

    fn weekday(&self, at: Nanos) -> u8 {
        weekday_of_day(self.seconds(at) / SECONDS_PER_DAY, self.weekday_offset)
    }

    /// The instant the second running at `at` began.
    fn second_start(&self, at: Nanos) -> Nanos {
        let into_second = at.get().saturating_sub(self.base_at.get()) % NANOS_PER_SECOND;
        Nanos(at.get() - into_second)
    }

    /// Sets the clock to `time`, beginning a second at `base_at`, with `weekday` that day.
    fn set(&mut self, time: DateTime, weekday: u8, base_at: Nanos) {
        let days = days_since_2000(time.year, time.month, time.day);
        let weekday = u64::from(weekday.clamp(1, 7) - 1);
        self.base_seconds = time.seconds_since_2000();
        self.base_at = base_at;
        self.weekday_offset = (weekday + DAYS_PER_WEEK - days % DAYS_PER_WEEK) % DAYS_PER_WEEK;
    }
}

/// The time registers written in the current transaction, applied when it ends.
///
/// The datasheet applies each byte on its acknowledge (research §4.2), but a firmware setting the
/// time writes the date before the month: applying `31` while the clock is still in February
/// would be refused, or would make an illogical date, for a time that is valid once the burst is
/// complete. The burst is therefore taken whole, at its STOP or at the next START: no read can
/// see the difference, since a read needs a new START.
#[derive(Clone, Copy, Debug)]
struct PendingTime {
    /// The time as written so far; its day may not fit its month until the burst ends.
    time: DateTime,
    weekday: u8,
    /// The instant of the burst's first byte: the time it writes is the time at that instant.
    first_at: Nanos,
    /// Where the seconds register was written: "writing seconds resets the countdown chain"
    /// (research §4.2), so the next second begins there.
    seconds_at: Option<Nanos>,
}

/// A field of the time, decoded from a register write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Second(u8),
    Minute(u8),
    Hour { hour: u8, twelve_hour: bool },
    Weekday(u8),
    Day(u8),
    Month { month: u8, century: bool },
    Year(u8),
}

/// The DS3231 on an I2C bus.
#[derive(Clone, Debug)]
pub struct Ds3231 {
    phase: Phase,
    pointer: u8,
    clock: Clock,
    /// The mode bit of the hours register, as last written (research §4.1).
    twelve_hour: bool,
    /// The time registers as copied at the last START or pointer wrap (research §4.2).
    latched: [u8; TIME_REGISTERS],
    pending: Option<PendingTime>,
    alarms: [u8; ALARM_REGISTERS],
    /// Control without CONV, which reads set while a conversion runs.
    control: u8,
    /// Status without BSY, which reads set while a conversion runs.
    status: u8,
    conversion_end: Option<Nanos>,
    aging: u8,
    temperature_quarters: i16,
    int_sqw_reported: bool,
    diagnostics: Diagnostics,
}

impl Ds3231 {
    /// A clock at `config.start` at device time 0, its pointer at `0x00`.
    ///
    /// # Errors
    /// [`Ds3231ConfigError`] for an invalid start time or a temperature out of range.
    pub fn new(config: Ds3231Config) -> Result<Ds3231, Ds3231ConfigError> {
        config.start.validate()?;
        if !TEMPERATURE_RANGE.contains(&config.temperature_quarters) {
            return Err(Ds3231ConfigError::Temperature(config.temperature_quarters));
        }
        let mut clock = Clock {
            base_seconds: 0,
            base_at: Nanos(0),
            weekday_offset: WEEKDAY_OF_2000_OFFSET,
        };
        clock.set(config.start, config.start.weekday(), Nanos(0));
        let osf = if config.oscillator_stopped { OSF } else { 0 };
        let mut chip = Ds3231 {
            phase: Phase::Released,
            pointer: SECONDS,
            clock,
            twelve_hour: false,
            latched: [0; TIME_REGISTERS],
            pending: None,
            alarms: [0; ALARM_REGISTERS],
            control: CONTROL_POWER_ON,
            status: osf | EN32KHZ,
            conversion_end: None,
            aging: 0,
            temperature_quarters: config.temperature_quarters,
            int_sqw_reported: false,
            diagnostics: Diagnostics::new(NAME),
        };
        chip.latch(Nanos(0));
        Ok(chip)
    }

    /// The time at `at`.
    #[must_use]
    pub fn now(&self, at: Nanos) -> DateTime {
        self.clock.now(at)
    }

    /// What `register` holds at `at`, as a read would give it if a START came now; `None` past
    /// `0x12`. Bytes written in a transaction not yet ended are not counted.
    #[must_use]
    pub fn register(&self, register: u8, at: Nanos) -> Option<u8> {
        match register {
            SECONDS..=YEAR => self.time_registers(at).get(usize::from(register)).copied(),
            _ => self.other_register(register, at),
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

    /// The seven time registers at `at`, BCD, in the hours mode last written.
    fn time_registers(&self, at: Nanos) -> [u8; TIME_REGISTERS] {
        let now = self.clock.now(at);
        let century = if now.year >= DateTime::FIRST_YEAR + YEARS_PER_CENTURY {
            CENTURY
        } else {
            0
        };
        let two_digits = u8::try_from(now.year % YEARS_PER_CENTURY).unwrap_or(0);
        [
            to_bcd(now.second),
            to_bcd(now.minute),
            self.encode_hour(now.hour),
            self.clock.weekday(at),
            to_bcd(now.day),
            to_bcd(now.month) | century,
            to_bcd(two_digits),
        ]
    }

    fn encode_hour(&self, hour: u8) -> u8 {
        if !self.twelve_hour {
            return to_bcd(hour);
        }
        let pm = if hour >= HOURS_PER_HALF_DAY { PM } else { 0 };
        let on_dial = match hour % HOURS_PER_HALF_DAY {
            0 => HOURS_PER_HALF_DAY,
            other => other,
        };
        TWELVE_HOUR | pm | to_bcd(on_dial)
    }

    /// A register outside the time: alarms, control, status, aging and temperature.
    fn other_register(&self, register: u8, at: Nanos) -> Option<u8> {
        let converting = self.converting(at);
        match register {
            ALARM_FIRST..=ALARM_LAST => self
                .alarms
                .get(usize::from(register - ALARM_FIRST))
                .copied(),
            CONTROL => Some(self.control | if converting { CONV } else { 0 }),
            STATUS => Some(self.status | if converting { BSY } else { 0 }),
            AGING => Some(self.aging),
            // Two's complement, the integer part in the MSB and the quarters in bits 7:6 of the
            // LSB (research §4.1): −25.25 °C is `E6 C0`, −26 + 0.75.
            TEMPERATURE_MSB => Some((self.temperature_quarters >> 2).to_le_bytes()[0]),
            TEMPERATURE_LSB => Some(self.temperature_quarters.to_le_bytes()[0] << 6),
            _ => None,
        }
    }

    fn converting(&self, at: Nanos) -> bool {
        self.conversion_end.is_some_and(|end| at < end)
    }

    /// Copies the time registers into the read buffer (research §4.2).
    fn latch(&mut self, at: Nanos) {
        self.latched = self.time_registers(at);
    }

    /// The byte a read at `register` gives: the latched time, or the live register.
    fn read_register(&self, register: u8, at: Nanos) -> u8 {
        match register {
            SECONDS..=YEAR => self.latched.get(usize::from(register)).copied(),
            _ => self.other_register(register, at),
        }
        .unwrap_or(0)
    }

    /// A data byte written at the pointer.
    fn write_register(&mut self, register: u8, byte: u8, at: Nanos) {
        match register {
            SECONDS..=YEAR => self.write_time(register, byte, at),
            ALARM_FIRST..=ALARM_LAST => {
                if let Some(slot) = self.alarms.get_mut(usize::from(register - ALARM_FIRST)) {
                    *slot = byte;
                }
            }
            CONTROL => self.write_control(byte, at),
            STATUS => {
                // OSF, A2F and A1F can only be cleared; EN32kHz takes the bit; BSY is read only.
                self.status = (self.status & byte & CLEAR_ONLY) | (byte & EN32KHZ);
            }
            AGING => self.aging = byte,
            _ => self
                .diagnostics
                .report(at, I2cDiagnosticKind::ReadOnly { register }),
        }
    }

    fn write_control(&mut self, byte: u8, at: Nanos) {
        self.control = byte & !CONV;
        if byte & CONV != 0 && !self.converting(at) {
            self.conversion_end = Some(Nanos(at.get().saturating_add(CONVERSION_NANOS)));
        }
        let alarm_interrupt = byte & INTCN != 0 && byte & (A1IE | A2IE) != 0;
        let square_wave = byte & INTCN == 0;
        if (alarm_interrupt || square_wave) && !self.int_sqw_reported {
            // Alarm matching and the pin are not modelled; nothing on the X3 is known to use them
            // (research §4.4).
            self.int_sqw_reported = true;
            self.diagnostics.report(
                at,
                I2cDiagnosticKind::NotModelled {
                    what: "the INT/SQW output, enabled by Control",
                    value: u32::from(byte),
                },
            );
        }
    }

    /// A byte of the time registers: decoded, then kept in the pending burst; a value the
    /// datasheet does not define is reported and leaves its field alone.
    fn write_time(&mut self, register: u8, byte: u8, at: Nanos) {
        let Some(field) = decode(register, byte) else {
            self.diagnostics.report(
                at,
                I2cDiagnosticKind::Undefined {
                    register,
                    value: byte,
                },
            );
            return;
        };
        let clock = self.clock;
        let pending = self.pending.get_or_insert_with(|| PendingTime {
            time: clock.now(at),
            weekday: clock.weekday(at),
            first_at: at,
            seconds_at: None,
        });
        let time = &mut pending.time;
        match field {
            Field::Second(second) => {
                time.second = second;
                pending.seconds_at = Some(at);
            }
            Field::Minute(minute) => time.minute = minute,
            Field::Hour { hour, twelve_hour } => {
                time.hour = hour;
                self.twelve_hour = twelve_hour;
            }
            Field::Weekday(weekday) => pending.weekday = weekday,
            Field::Day(day) => time.day = day,
            Field::Month { month, century } => {
                time.month = month;
                time.year = year_of(century, time.year % YEARS_PER_CENTURY);
            }
            Field::Year(two_digits) => {
                let century = time.year >= DateTime::FIRST_YEAR + YEARS_PER_CENTURY;
                time.year = year_of(century, u16::from(two_digits));
            }
        }
    }

    /// Applies the time written in the transaction that ends at `at`. A day past the end of its
    /// month ("illogical", research §4.1) is reported and taken as the month's last day.
    /// ASSUMPTION: the datasheet leaves that case undefined; clamping keeps a valid clock.
    fn commit(&mut self, at: Nanos) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let mut time = pending.time;
        let last_day = days_in_month(time.year, time.month);
        if time.day > last_day {
            self.diagnostics.report(
                at,
                I2cDiagnosticKind::Undefined {
                    register: DATE,
                    value: to_bcd(time.day),
                },
            );
            time.day = last_day;
        }
        let base_at = pending
            .seconds_at
            .unwrap_or_else(|| self.clock.second_start(pending.first_at));
        self.clock.set(time, pending.weekday, base_at);
    }

    /// The pointer after `register`: up by one, wrapping past `0x12` (research §4.2).
    const fn next(register: u8) -> u8 {
        if register >= LAST_REGISTER {
            SECONDS
        } else {
            register + 1
        }
    }
}

impl I2cTarget for Ds3231 {
    fn address(&mut self, address: u8, read: bool, at: Nanos) -> bool {
        // Every START ends the transaction before it and refreshes the read buffer, whoever it
        // addresses ("on any START", research §4.2).
        self.commit(at);
        self.latch(at);
        if address != ADDRESS {
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
                    // ASSUMPTION: the clock acknowledges every byte (the datasheet describes no
                    // NACK) and a pointer past its map moves nothing; the bytes that follow are
                    // dropped.
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
                let register = self.pointer;
                self.write_register(register, byte, at);
                self.pointer = Ds3231::next(register);
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
        let register = self.pointer;
        let value = self.read_register(register, at);
        self.pointer = Ds3231::next(register);
        if self.pointer == SECONDS {
            self.latch(at);
        }
        value
    }

    fn stop(&mut self, at: Nanos) {
        self.commit(at);
        self.phase = Phase::Released;
    }
}

/// The field a byte written to a time register sets, `None` for a value the datasheet does not
/// define: a BCD digit above 9, a bit its map fixes at 0, a value out of the field's range.
fn decode(register: u8, byte: u8) -> Option<Field> {
    match register {
        SECONDS => from_bcd(byte).filter(|s| *s <= 59).map(Field::Second),
        MINUTES => from_bcd(byte).filter(|m| *m <= 59).map(Field::Minute),
        HOURS => decode_hour(byte),
        WEEKDAY => (1..=7).contains(&byte).then_some(Field::Weekday(byte)),
        DATE => from_bcd(byte)
            .filter(|d| (1..=31).contains(d))
            .map(Field::Day),
        MONTH => from_bcd(byte & MONTH_FIELD)
            .filter(|m| (1..=12).contains(m) && byte & !(CENTURY | MONTH_FIELD) == 0)
            .map(|month| Field::Month {
                month,
                century: byte & CENTURY != 0,
            }),
        YEAR => from_bcd(byte).map(Field::Year),
        _ => None,
    }
}

/// The hours register: bit 6 selects 12-hour mode, where bit 5 is PM and the hour runs 1 to 12;
/// in 24-hour mode bits 5:0 hold 00 to 23 (research §4.1).
fn decode_hour(byte: u8) -> Option<Field> {
    if byte & TWELVE_HOUR == 0 {
        return from_bcd(byte).filter(|h| *h <= 23).map(|hour| Field::Hour {
            hour,
            twelve_hour: false,
        });
    }
    let on_dial = from_bcd(byte & !(TWELVE_HOUR | PM)).filter(|h| (1..=12).contains(h))?;
    let afternoon = if byte & PM == 0 {
        0
    } else {
        HOURS_PER_HALF_DAY
    };
    Some(Field::Hour {
        hour: on_dial % HOURS_PER_HALF_DAY + afternoon,
        twelve_hour: true,
    })
}

/// The year of a two-digit year and the century bit.
fn year_of(century: bool, two_digits: u16) -> u16 {
    DateTime::FIRST_YEAR + if century { YEARS_PER_CENTURY } else { 0 } + two_digits
}

/// `value`, 0 to 99, in two BCD digits.
const fn to_bcd(value: u8) -> u8 {
    ((value / 10) << 4) | (value % 10)
}

/// The value of two BCD digits, `None` when a digit is above 9.
const fn from_bcd(byte: u8) -> Option<u8> {
    let (tens, ones) = (byte >> 4, byte & 0x0F);
    if tens > 9 || ones > 9 {
        None
    } else {
        Some(tens * 10 + ones)
    }
}

const fn is_leap(year: u16) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// The days from 2000-01-01 to `year-month-day` (Howard Hinnant's `days_from_civil`, on a year
/// that starts in March so that the leap day ends it). The date is in the clock's range.
fn days_since_2000(year: u16, month: u8, day: u8) -> u64 {
    let (year, month) = (u64::from(year), u64::from(month));
    let (march_year, march_month) = if month <= 2 {
        (year - 1, month + 9)
    } else {
        (year, month - 3)
    };
    let day_of_year = (153 * march_month + 2) / 5 + u64::from(day) - 1;
    let days =
        march_year * 365 + march_year / 4 - march_year / 100 + march_year / 400 + day_of_year;
    days - DAYS_TO_2000
}

/// The date `days` after 2000-01-01 (Hinnant's `civil_from_days`).
fn civil_from_days(days: u64) -> Option<(u16, u8, u8)> {
    let since_origin = days + DAYS_TO_2000;
    let era = since_origin / DAYS_PER_400_YEARS;
    let day_of_era = since_origin % DAYS_PER_400_YEARS;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * march_month + 2) / 5 + 1;
    let month = if march_month < 10 {
        march_month + 3
    } else {
        march_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    Some((
        u16::try_from(year).ok()?,
        u8::try_from(month).ok()?,
        u8::try_from(day).ok()?,
    ))
}

/// The day of the week, 1 to 7, of the day `days` after 2000-01-01, for a counter at `offset`.
fn weekday_of_day(days: u64, offset: u64) -> u8 {
    u8::try_from((days + offset) % DAYS_PER_WEEK).map_or(1, |day| day + 1)
}

#[cfg(test)]
mod tests;
