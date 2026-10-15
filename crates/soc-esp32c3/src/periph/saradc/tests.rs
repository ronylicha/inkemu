use super::*;
use crate::chip::ChipRevision;
use crate::clock::CpuClock;
use crate::periph::{GPIO_BASE, IO_MUX_BASE, pump};
use emu_core::{AnalogSource, Cycles, Nanos};

const SOURCE_BIT: u64 = 1 << 43;

fn efuse() -> Efuse {
    Efuse::new(ChipRevision::V0_3)
}

#[test]
fn the_calibration_of_the_production_chip_is_read_from_its_block2() {
    // The eFuse of decision D9: BLK_VERSION_MAJOR 1, ADC1_CAL_VOL_ATTEN3 = 0x247 (P §E).
    let block = efuse().system_data();
    assert_eq!(field(&block, BLK_VERSION_MAJOR_BIT, 2), 1);
    assert_eq!(
        field(&block, CAL_VOL_BIT + 30, 10),
        0x247,
        "straddles two words"
    );
    // At 1370 mV the eFuse point reads 2000 − 71 = 1929; the line of the calibration gives
    // 1929 × 46544 / 65536 = 1369 mV there, less the error polynomial.
    let calibration = Calibration::from_block2(&block);
    let v1 = 1929 * (65_536 * 1370 / 1929) / 65_536;
    assert_eq!(v1, 1369);
    assert_eq!(
        calibration.millivolts(3, 1929),
        1369 - reading_error(1369, 3)
    );
}

/// ESP-IDF's error polynomial in floating point, for each attenuation: what the integer code of
/// `reading_error` must agree with within a millivolt or two of truncations.
fn error_in_floats(v1: f64, atten: usize) -> f64 {
    let terms = if atten == 3 { 5 } else { 3 };
    (0..terms)
        .map(|i| {
            let (k, divisor) = ERROR_COEFFICIENTS[atten][i];
            #[allow(clippy::cast_precision_loss)]
            let k = k as f64 / divisor as f64;
            f64::from(ERROR_SIGNS[atten][i]) * k * v1.powi(i32::try_from(i).unwrap())
        })
        .sum()
}

#[test]
fn the_integer_error_polynomial_agrees_with_the_coefficients_in_floating_point() {
    for atten in 0..4 {
        for v1 in [1u32, 100, 500, 1000, 2000, 2800] {
            let exact = error_in_floats(f64::from(v1), atten);
            let integer = f64::from(reading_error(u64::from(v1), atten));
            assert!(
                (exact - integer).abs() <= 3.0,
                "atten {atten}, v1 {v1}: {exact} against {integer}"
            );
        }
    }
}

#[test]
fn a_voltage_reads_back_through_esp_idf_calibration_within_a_count() {
    let calibration = Calibration::from_block2(&efuse().system_data());
    // The ranges ESP-IDF documents for the C3 per attenuation, inside the calibrated band.
    for (atten, top) in [(0, 750), (1, 1050), (2, 1300), (3, 2500)] {
        for millivolts in (100..=top).step_by(25) {
            let raw = calibration.reading(atten, millivolts);
            let back = calibration.millivolts(atten, raw);
            assert!(
                (back - i32::try_from(millivolts).unwrap()).abs() <= 2,
                "atten {atten}: {millivolts} mV reads {raw}, which calibrates to {back} mV"
            );
        }
    }
    assert_eq!(calibration.reading(3, 0), 0);
    assert_eq!(calibration.reading(3, 3300), FULL_SCALE, "beyond the range");
}

#[test]
fn a_chip_without_calibration_reads_by_the_nominal_line() {
    let calibration = Calibration::from_block2(&[0; 8]);
    assert_eq!(calibration.reading(3, 2200), 2048);
    assert_eq!(calibration.millivolts(0, 4095), 1100);
}

/// A voltage the test sets.
#[derive(Debug)]
struct Voltage(u32);

impl AnalogSource for Voltage {
    fn millivolts(&mut self, _at: Nanos) -> u32 {
        self.0
    }
}

struct Bench {
    ctx: Ctx,
    io: IoMatrix,
    adc: SarAdc,
}

fn bench() -> Bench {
    let mut ctx = Ctx::default();
    ctx.set_clock(CpuClock::new(1, 2));
    let mut io = IoMatrix::new(0x8);
    io.attach_analog(1, Box::new(Voltage(1650))).unwrap();
    io.attach_analog(5, Box::new(Voltage(500))).unwrap();
    let mut adc = SarAdc::new(&efuse());
    // As `adc_oneshot_hal_setup` leaves it: the APB clock divided by 16, SAR_CLK_DIV 1.
    assert!(adc.write(&mut ctx, CLKM_CONF, 15 | (2 << CLK_SEL_SHIFT)));
    assert!(adc.write(&mut ctx, CTRL, 0x4000_0040 | (1 << SAR_CLK_DIV_SHIFT)));
    Bench { ctx, io, adc }
}

impl Bench {
    fn write(&mut self, offset: u32, value: u32) {
        assert!(self.adc.write(&mut self.ctx, offset, value), "{offset:#x}");
    }

    fn read(&mut self, offset: u32) -> u32 {
        self.adc.read(&mut self.ctx, offset).unwrap()
    }

    /// `adc_oneshot_hal_convert`: start low, start high.
    fn convert(&mut self, unit: u32, channel: u32, atten: u32) {
        let sample = unit | (channel << CHANNEL_SHIFT) | (atten << ATTEN_SHIFT);
        self.write(ONETIME_SAMPLE, sample);
        self.write(ONETIME_SAMPLE, sample | ONETIME_START);
    }

    fn run_to(&mut self, instant: u64) {
        self.ctx.sleep_until(Cycles(instant));
        let Bench { ctx, io, adc } = self;
        pump(ctx, |ctx, event| {
            assert_eq!(event, Event::SarAdc);
            adc.wake(ctx, io);
        });
    }
}

#[test]
fn a_one_shot_conversion_of_adc1_ends_after_25_sar_clock_cycles_with_the_pin_s_reading() {
    let mut b = bench();
    b.write(INT_ENA, ADC1_DONE);
    b.convert(ADC1_ONETIME, 1, 3);
    // 25 cycles of 80 MHz / 16: 5 µs, 800 cycles of the time base.
    assert_eq!(b.ctx.next_wake(), Some(Cycles(800)));
    b.run_to(799);
    assert_eq!(b.read(INT_RAW), 0, "not yet");
    b.run_to(800);
    assert_eq!(b.read(INT_RAW), ADC1_DONE);
    assert_eq!(b.read(INT_ST), ADC1_DONE);
    assert_eq!(b.ctx.sources() & SOURCE_BIT, SOURCE_BIT);
    let raw = b.read(SAR1DATA_STATUS);
    let back = b.adc.calibration.millivolts(3, raw);
    assert!((back - 1650).abs() <= 2, "{raw} calibrates to {back} mV");
    b.write(INT_CLR, ADC1_DONE);
    assert_eq!(b.read(INT_RAW), 0);
    assert_eq!(b.ctx.sources() & SOURCE_BIT, 0);
}

#[test]
fn adc2_converts_gpio5_and_an_inverted_output_is_complemented() {
    let mut b = bench();
    b.convert(ADC2_ONETIME, 0, 0);
    b.run_to(800);
    assert_eq!(b.read(INT_RAW), ADC2_DONE);
    let raw = b.read(SAR2DATA_STATUS);
    assert!((b.adc.calibration.millivolts(0, raw) - 500).abs() <= 2);
    b.write(CTRL2, SAR1_INV);
    b.convert(ADC1_ONETIME, 1, 3);
    b.run_to(1_600);
    let inverted = b.read(SAR1DATA_STATUS);
    assert_eq!(
        inverted,
        FULL_SCALE - b.adc.calibration.reading(3, 1650),
        "the complement"
    );
}

#[test]
fn a_pin_without_an_analog_chip_reads_its_level() {
    let mut b = bench();
    b.convert(ADC1_ONETIME, 2, 3);
    b.run_to(800);
    assert_eq!(
        b.read(SAR1DATA_STATUS),
        FULL_SCALE,
        "pulled up, as at reset: 3.3 V"
    );
    // The analog configuration of TRM §5.7: no pull, no input.
    let pad2 = IO_MUX_BASE + 4 + 4 * 2 - GPIO_BASE;
    assert!(b.io.write(&mut b.ctx, pad2, 1 << 12));
    b.convert(ADC1_ONETIME, 2, 3);
    b.run_to(1_600);
    assert_eq!(b.read(SAR1DATA_STATUS), 0, "a floating pad: 0 V");
    b.convert(ADC1_ONETIME, 9, 3);
    b.run_to(2_400);
    assert_eq!(b.read(SAR1DATA_STATUS), 0, "no such channel");
}

#[test]
fn a_scan_both_units_and_a_foreign_clock_are_refused_by_name() {
    let mut b = bench();
    b.write(CTRL, 1 << 1);
    assert_eq!(
        b.ctx.take_unsupported(),
        Some(Unsupported::Adc {
            refusal: AdcRefusal::Scan
        })
    );
    b.write(CTRL2, TIMER_EN);
    assert_eq!(
        b.ctx.take_unsupported(),
        Some(Unsupported::Adc {
            refusal: AdcRefusal::Scan
        })
    );
    b.convert(ADC1_ONETIME | ADC2_ONETIME, 1, 3);
    assert_eq!(
        b.ctx.take_unsupported(),
        Some(Unsupported::Adc {
            refusal: AdcRefusal::BothUnits
        })
    );
    assert_eq!(b.ctx.next_wake(), None);
    b.write(CLKM_CONF, 15 | (1 << CLK_SEL_SHIFT));
    b.convert(ADC1_ONETIME, 1, 3);
    assert_eq!(
        b.ctx.take_unsupported(),
        Some(Unsupported::Adc {
            refusal: AdcRefusal::Clock { clk_sel: 1 }
        })
    );
    assert!(!b.adc.write(&mut b.ctx, SAR1DATA_STATUS, 0), "read-only");
}

#[test]
fn a_reset_loses_the_conversion_and_restores_the_reset_values() {
    let mut b = bench();
    b.convert(ADC1_ONETIME, 1, 3);
    b.adc.reset(&mut b.ctx);
    assert_eq!(b.read(ONETIME_SAMPLE), 0x1A00_0000, "the SVD's reset value");
    b.run_to(1_000);
    assert_eq!(b.read(INT_RAW), 0);
}
