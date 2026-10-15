//! The SAR ADC controller (`APB_SARADC`): one-shot conversions of ADC1 (GPIO0 to GPIO4) and ADC2
//! (GPIO5), as ESP-IDF's `adc_oneshot` and Arduino's `analogRead` use it (decision 2b-D10;
//! research P §E).
//!
//! A rising edge of `ONETIME_SAMPLE.ONETIME_START` starts a conversion of the selected unit and
//! channel; 25 cycles of `SAR_CLK` later the IO matrix gives the pad's voltage, the result lands in
//! `SAR1DATA_STATUS` or `SAR2DATA_STATUS` and `INT_RAW` bit 31 or 30 rises. The reading of a voltage
//! is the inverse of ESP-IDF's calibration for the chip's eFuse (`Calibration`, below).
use crate::ctx::{Ctx, Deadline, Event, Source};
use crate::diag::{AdcRefusal, Unsupported};
use crate::periph::efuse::Efuse;
use crate::periph::io::IoMatrix;
use crate::periph::{APB_SARADC_BASE, Accepted, Storage};
use emu_core::Peripheral;

/// The last pad with an ADC channel: GPIO5, channel 0 of ADC2 (TRM table 34.2-1).
pub(crate) const LAST_ADC_PAD: usize = 5;

/// Offset of an `APB_SARADC` register; an unknown name fails the build.
const fn reg(name: &str) -> u32 {
    svd_regs::esp32c3::addr("APB_SARADC", name) - APB_SARADC_BASE
}

const CTRL: u32 = reg("CTRL");
const CTRL2: u32 = reg("CTRL2");
const ONETIME_SAMPLE: u32 = reg("ONETIME_SAMPLE");
const SAR1DATA_STATUS: u32 = reg("SAR1DATA_STATUS");
const SAR2DATA_STATUS: u32 = reg("SAR2DATA_STATUS");
const INT_ENA: u32 = reg("INT_ENA");
const DMA_CONF: u32 = reg("DMA_CONF");
const INT_RAW: u32 = reg("INT_RAW");
const INT_ST: u32 = reg("INT_ST");
const INT_CLR: u32 = reg("INT_CLR");
const CLKM_CONF: u32 = reg("CLKM_CONF");

/// `CTRL`: `SARADC_START_FORCE` `[0]`, `SARADC_START` `[1]`, `SARADC_SAR_CLK_DIV` `[14:7]`
/// (`SVD`).
const START_SCAN: u32 = 0b11;
const SAR_CLK_DIV_SHIFT: u32 = 7;
const SAR_CLK_DIV_MASK: u32 = 0xFF;
/// `CTRL2`: `SARADC_SAR1_INV` `[9]`, `SARADC_SAR2_INV` `[10]`, `SARADC_TIMER_EN` `[24]`.
const SAR1_INV: u32 = 1 << 9;
const SAR2_INV: u32 = 1 << 10;
const TIMER_EN: u32 = 1 << 24;
/// `ONETIME_SAMPLE`: `ONETIME_ATTEN` `[24:23]`, `ONETIME_CHANNEL` `[28:25]`, `ONETIME_START`
/// `[29]`, `ADC2_ONETIME_SAMPLE` `[30]`, `ADC1_ONETIME_SAMPLE` `[31]`.
const ATTEN_SHIFT: u32 = 23;
const ATTEN_MASK: u32 = 0x3;
const CHANNEL_SHIFT: u32 = 25;
const CHANNEL_MASK: u32 = 0xF;
const ONETIME_START: u32 = 1 << 29;
const ADC2_ONETIME: u32 = 1 << 30;
const ADC1_ONETIME: u32 = 1 << 31;
/// `INT_RAW` bits 31 (`ADC1_DONE`) and 30 (`ADC2_DONE`); the register holds bits 26 to 31.
const ADC1_DONE: u32 = 1 << 31;
const ADC2_DONE: u32 = 1 << 30;
const INT_BITS: u32 = 0xFC00_0000;
/// `INT_RAW` bits 26 to 29: the thresholds of a scan's samples (`THRES0/1_HIGH/LOW`).
const THRESHOLD_INTS: u32 = 0x3C00_0000;
/// `DMA_CONF.APB_ADC_TRANS` `[31]`: a scan's samples go to GDMA.
const APB_ADC_TRANS: u32 = 1 << 31;
/// `CLKM_CONF`: `CLKM_DIV_NUM` `[7:0]`, `CLK_SEL` `[22:21]`; 2 selects the APB clock
/// (`adc_ll_digi_clk_sel`, `adc_ll.h:315-318`).
const DIV_NUM_MASK: u32 = 0xFF;
const CLK_SEL_SHIFT: u32 = 21;
const CLK_SEL_MASK: u32 = 0x3;
const CLK_SEL_APB: u32 = 2;
/// `SAR_CLK` cycles in one sample (TRM §34.2.3.4).
const SAR_CLK_PER_SAMPLE: u64 = 25;
/// The largest reading: 12 bits.
const FULL_SCALE: u32 = 4095;

/// Source 43, `vDIGTAL_ADC_INTR` (TRM table 8.3-1).
const SOURCE: Source = Source::of("APB_ADC_INT_MAP");

/// The registers stored without effect: the scan's configuration (a scan software could see is
/// refused by name, one whose samples only go to GDMA samples nothing), the filters and
/// thresholds that act on the scan's samples, the arbiter, the calibration register of the analog
/// side, the temperature sensor, the date.
const WHY_SCAN: &str = "multi-channel scan configuration: a scan software could see is refused by name, one whose samples only go to GDMA samples nothing";
const ACCEPTED: [Accepted; 16] = [
    Accepted::svd("APB_SARADC", "FILTER_CTRL1", WHY_SCAN, "SP2d"),
    Accepted::svd("APB_SARADC", "FSM_WAIT", WHY_SCAN, "SP2d"),
    Accepted::svd("APB_SARADC", "SAR1_STATUS", WHY_SCAN, "SP2d"),
    Accepted::svd("APB_SARADC", "SAR2_STATUS", WHY_SCAN, "SP2d"),
    Accepted::svd("APB_SARADC", "SAR_PATT_TAB1", WHY_SCAN, "SP2d"),
    Accepted::svd("APB_SARADC", "SAR_PATT_TAB2", WHY_SCAN, "SP2d"),
    Accepted::svd("APB_SARADC", "FILTER_CTRL0", WHY_SCAN, "SP2d"),
    Accepted::svd("APB_SARADC", "THRES0_CTRL", WHY_SCAN, "SP2d"),
    Accepted::svd("APB_SARADC", "THRES1_CTRL", WHY_SCAN, "SP2d"),
    Accepted::svd("APB_SARADC", "THRES_CTRL", WHY_SCAN, "SP2d"),
    Accepted::svd("APB_SARADC", "DMA_CONF", WHY_SCAN, "SP2d"),
    Accepted::svd(
        "APB_SARADC",
        "ARB_CTRL",
        "ADC2 arbiter: no Wi-Fi contends for ADC2",
        "SP5a",
    ),
    Accepted::svd(
        "APB_SARADC",
        "APB_TSENS_CTRL",
        "temperature sensor: no temperature is measured, TSENS_OUT reads what is stored",
        "SP2d",
    ),
    Accepted::svd(
        "APB_SARADC",
        "TSENS_CTRL2",
        "temperature sensor clock",
        "SP2d",
    ),
    Accepted::svd(
        "APB_SARADC",
        "CALI",
        "analog self-calibration selection: a reading follows the eFuse calibration",
        "SP2d",
    ),
    Accepted::svd(
        "APB_SARADC",
        "CTRL_DATE",
        "none: the version date of the block, stored as written",
        "SP2d",
    ),
];

pub(crate) fn accepted() -> Vec<Accepted> {
    ACCEPTED.to_vec()
}

/// The error polynomial ESP-IDF subtracts from the line of the eFuse, per attenuation: up to five
/// terms of a coefficient and a divisor, and their signs (`esp_adc/esp32c3/curve_fitting_coefficients.c:23-49`,
/// v5.5.2). The divisors are `1e15` and `1e16` written as `uint64_t`.
const E15: u64 = 1_000_000_000_000_000;
const E16: u64 = 10_000_000_000_000_000;
const ERROR_COEFFICIENTS: [[(u64, u64); 5]; 4] = [
    [
        (225_966_470_500_043, E15),
        (7_265_418_501_948, E16),
        (109_410_402_681, E16),
        (0, 0),
        (0, 0),
    ],
    [
        (4_229_623_392_600_516, E16),
        (731_527_490_903, E16),
        (88_166_562_521, E16),
        (0, 0),
        (0, 0),
    ],
    [
        (1_017_859_239_236_435, E15),
        (97_159_265_299_153, E16),
        (149_794_028_038, E16),
        (0, 0),
        (0, 0),
    ],
    [
        (14_912_262_772_850_453, E16),
        (228_549_975_564_099, E16),
        (356_391_935_717, E16),
        (179_964_582, E16),
        (42_046, E16),
    ],
];
const ERROR_SIGNS: [[i32; 5]; 4] = [
    [-1, -1, 1, 0, 0],
    [1, -1, 1, 0, 0],
    [-1, -1, 1, 0, 0],
    [-1, -1, 1, -1, 1],
];
/// The reference voltages of the eFuse points, per attenuation (`esp_efuse_rtc_calib.c:68-80`).
const CAL_VOLTAGE_MV: [u32; 4] = [400, 550, 750, 1370];
/// BLOCK2 bits of `ADC1_CAL_VOL_ATTEN0` (10 bits each, the next attenuation 10 bits further) and of
/// `BLK_VERSION_MAJOR` (2 bits) (`esp_efuse_table.csv:179-189`).
const CAL_VOL_BIT: u32 = 188;
const BLK_VERSION_MAJOR_BIT: u32 = 128;
/// The full scale of each attenuation by the TRM's nominal transfer function, `Vref / k` with
/// `Vref` 1100 mV and k 100 %, 75 %, 50 %, 25 % (TRM §34.2.3.2): for a chip without calibration.
const NOMINAL_FULL_SCALE_MV: [u32; 4] = [1_100, 1_467, 2_200, 4_400];

/// How the chip reads a voltage at each attenuation: a table of the voltage ESP-IDF's
/// calibration gives each reading, whose inverse is the reading of a voltage.
///
/// ASSUMPTION: the emulated chip is the typical chip of ESP-IDF's curve fitting for its eFuse: its
/// reading of a voltage is the one the calibration maps back closest to it, so that
/// `adc_cali_raw_to_voltage` (Arduino's `analogReadMilliVolts`) gives back the input within a
/// count (P, open question 7). The reading does not depend on the initial code software writes
/// through regi2c, nor on `RTC_CNTL.SENSOR_CTRL` (question 8). A chip without calibration in its
/// eFuse reads by the TRM's nominal line.
#[derive(Clone, Debug)]
pub(crate) struct Calibration {
    /// `volts[atten][raw]`: the millivolts ESP-IDF computes for each reading.
    volts: Vec<[i32; 4096]>,
}

/// `bits` bits of BLOCK2 from bit `first` on.
fn field(block: &[u32; 8], first: u32, bits: u32) -> u32 {
    let word = usize::try_from(first / 32).unwrap_or(0);
    let low = u64::from(block[word]);
    let high = block.get(word + 1).map_or(0, |&w| u64::from(w));
    let both = (high << 32) | low;
    u32::try_from((both >> (first % 32)) & ((1 << bits) - 1)).unwrap_or(0)
}

/// ESP-IDF's `get_reading_error`, in its integer arithmetic (`adc_cali_curve_fitting.c:181-215`).
fn reading_error(v1: u64, atten: usize) -> i32 {
    if v1 == 0 {
        return 0;
    }
    let terms = if atten == 3 { 5 } else { 3 };
    let coefficients = &ERROR_COEFFICIENTS[atten];
    let signs = &ERROR_SIGNS[atten];
    // A term is a few hundred millivolts at most, far inside `i32`, as in the C code's cast.
    let as_i32 = |term: u64| i32::try_from(term).unwrap_or(i32::MAX);
    let mut variable = 1u64;
    let mut error = as_i32(variable.wrapping_mul(coefficients[0].0) / coefficients[0].1) * signs[0];
    for i in 1..terms {
        variable = variable.wrapping_mul(v1);
        let term = variable.wrapping_mul(coefficients[i].0) / coefficients[i].1;
        error += as_i32(term) * signs[i];
    }
    error
}

impl Calibration {
    /// The calibration of the chip whose BLOCK2 is `block`.
    #[must_use]
    pub(crate) fn from_block2(block: &[u32; 8]) -> Calibration {
        let calibrated = field(block, BLK_VERSION_MAJOR_BIT, 2) == 1;
        let volts = (0..4)
            .map(|atten| {
                let mut table = [0i32; 4096];
                for (raw, volts) in table.iter_mut().enumerate() {
                    let raw = u64::try_from(raw).unwrap_or(0);
                    *volts = if calibrated {
                        let vol = field(
                            block,
                            CAL_VOL_BIT + 10 * u32::try_from(atten).unwrap_or(0),
                            10,
                        );
                        // `esp_efuse_rtc_calib_get_cal_voltage`: bit 9 is the sign.
                        let digi = if vol & (1 << 9) != 0 {
                            2000 - (vol & 0x1FF)
                        } else {
                            2000 + vol
                        };
                        let coeff_a = 65_536 * u64::from(CAL_VOLTAGE_MV[atten]) / u64::from(digi);
                        let v1 = raw * coeff_a / 65_536;
                        i32::try_from(v1).unwrap_or(i32::MAX) - reading_error(v1, atten)
                    } else {
                        let full = u64::from(NOMINAL_FULL_SCALE_MV[atten]);
                        i32::try_from(raw * full / u64::from(FULL_SCALE)).unwrap_or(i32::MAX)
                    };
                }
                table
            })
            .collect();
        Calibration { volts }
    }

    /// The millivolts ESP-IDF's calibration gives reading `raw` at attenuation `atten`: what the
    /// tests check a reading against.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn millivolts(&self, atten: usize, raw: u32) -> i32 {
        self.volts[atten & 3][usize::try_from(raw.min(FULL_SCALE)).unwrap_or(0)]
    }

    /// The reading of `millivolts` at attenuation `atten`: the one whose calibrated voltage is
    /// closest, the lowest on a tie.
    #[must_use]
    pub(crate) fn reading(&self, atten: usize, millivolts: u32) -> u32 {
        let target = i64::from(millivolts);
        let table = &self.volts[atten & 3];
        let (mut best, mut best_gap) = (0, i64::MAX);
        for (raw, &volts) in table.iter().enumerate() {
            let gap = (i64::from(volts) - target).abs();
            if gap < best_gap {
                best = raw;
                best_gap = gap;
            }
        }
        u32::try_from(best).unwrap_or(FULL_SCALE)
    }
}

/// A conversion under way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Conversion {
    adc2: bool,
    channel: u32,
    atten: usize,
}

/// The SAR ADC controller.
#[derive(Debug)]
pub struct SarAdc {
    regs: Storage,
    calibration: Calibration,
    converting: Option<Conversion>,
    wake: Deadline,
}

impl SarAdc {
    /// The controller of a chip whose eFuse is `efuse`.
    #[must_use]
    pub fn new(efuse: &Efuse) -> SarAdc {
        SarAdc {
            regs: Storage::for_peripheral("APB_SARADC", APB_SARADC_BASE),
            calibration: Calibration::from_block2(&efuse.system_data()),
            converting: None,
            wake: Deadline::default(),
        }
    }

    fn get(&self, offset: u32) -> u32 {
        self.regs.read(offset).unwrap_or(0)
    }

    fn set(&mut self, offset: u32, value: u32) {
        self.regs.write(offset, value);
    }

    /// `SYSTEM.PERIP_RST_EN0` bit 28, or a core reset: the registers return to their reset values
    /// and a conversion under way is lost. The calibration is the chip's.
    pub(crate) fn reset(&mut self, ctx: &mut Ctx) {
        self.wake.cancel(ctx);
        self.regs = Storage::for_peripheral("APB_SARADC", APB_SARADC_BASE);
        self.converting = None;
        ctx.irq(SOURCE, false);
    }

    /// The wake-up of the end of a conversion: the pad's voltage is read and converted.
    pub(crate) fn wake(&mut self, ctx: &mut Ctx, io: &mut IoMatrix) {
        self.wake.called(ctx.now());
        if self.wake.due().is_some() {
            return;
        }
        let Some(conversion) = self.converting.take() else {
            return;
        };
        let pad = if conversion.adc2 {
            (conversion.channel == 0).then_some(LAST_ADC_PAD)
        } else {
            usize::try_from(conversion.channel)
                .ok()
                .filter(|&c| c < LAST_ADC_PAD)
        };
        // ASSUMPTION: a channel the unit does not have reads 0 V.
        let millivolts = pad.map_or(0, |pad| io.millivolts(ctx, pad));
        let mut raw = self.calibration.reading(conversion.atten, millivolts);
        let (data, done, inverted) = if conversion.adc2 {
            (SAR2DATA_STATUS, ADC2_DONE, SAR2_INV)
        } else {
            (SAR1DATA_STATUS, ADC1_DONE, SAR1_INV)
        };
        if self.get(CTRL2) & inverted != 0 {
            // ASSUMPTION: an inverted output is the complement of the 12-bit reading.
            raw = FULL_SCALE - raw;
        }
        self.set(data, raw);
        self.set(INT_RAW, self.get(INT_RAW) | done);
        self.drive_source(ctx);
    }

    fn drive_source(&self, ctx: &mut Ctx) {
        ctx.irq(SOURCE, self.get(INT_RAW) & self.get(INT_ENA) != 0);
    }

    /// A scan the timer runs (`CTRL2.SARADC_TIMER_EN`) is refused when software could see its
    /// samples: they stay in the controller (`DMA_CONF.APB_ADC_TRANS` clear) or a threshold
    /// interrupt is enabled. A scan that only sends them to GDMA is the one ESP-IDF's bootloader
    /// runs to feed the random number generator (`bootloader_random_enable`,
    /// `bootloader_random_esp32c3.c:16-56`): nothing in the chip changes for software, and a
    /// firmware that links a GDMA channel to it touches GDMA, which is not modelled and says so.
    fn refuse_a_visible_scan(&self, ctx: &mut Ctx) {
        let to_gdma_only =
            self.get(DMA_CONF) & APB_ADC_TRANS != 0 && self.get(INT_ENA) & THRESHOLD_INTS == 0;
        if self.get(CTRL2) & TIMER_EN != 0 && !to_gdma_only {
            ctx.unsupported(Unsupported::Adc {
                refusal: AdcRefusal::Scan,
            });
        }
    }

    /// One `SAR_CLK` cycle, in cycles of the time base: the APB clock divided by `CLKM_DIV_NUM + 1`
    /// and by `SAR_CLK_DIV`.
    ///
    /// ASSUMPTION: the fractional part `DIV_B / DIV_A` adds nothing, and `SAR_CLK_DIV` divides by
    /// itself (1 at least); ESP-IDF's continuous mode counts the controller's divider as
    /// `DIV_NUM + 1` (`adc_hal_digi`, P §E).
    fn sar_clk_period(&self, ctx: &mut Ctx) -> u64 {
        let clkm = self.get(CLKM_CONF);
        let clk_sel = (clkm >> CLK_SEL_SHIFT) & CLK_SEL_MASK;
        if clk_sel != CLK_SEL_APB {
            ctx.unsupported(Unsupported::Adc {
                refusal: AdcRefusal::Clock { clk_sel },
            });
        }
        let div_num = u64::from(clkm & DIV_NUM_MASK) + 1;
        let sar_div = u64::from((self.get(CTRL) >> SAR_CLK_DIV_SHIFT) & SAR_CLK_DIV_MASK).max(1);
        ctx.clock().apb_period() * div_num * sar_div
    }

    /// `ONETIME_START` rose: a conversion starts.
    fn start(&mut self, ctx: &mut Ctx, sample: u32) {
        let (adc1, adc2) = (sample & ADC1_ONETIME != 0, sample & ADC2_ONETIME != 0);
        if adc1 && adc2 {
            ctx.unsupported(Unsupported::Adc {
                refusal: AdcRefusal::BothUnits,
            });
            return;
        }
        if !adc1 && !adc2 {
            // ASSUMPTION: a start with no unit selected converts nothing.
            return;
        }
        self.converting = Some(Conversion {
            adc2,
            channel: (sample >> CHANNEL_SHIFT) & CHANNEL_MASK,
            atten: usize::try_from((sample >> ATTEN_SHIFT) & ATTEN_MASK).unwrap_or(0),
        });
        let end = ctx.now() + SAR_CLK_PER_SAMPLE * self.sar_clk_period(ctx);
        self.wake.arm(ctx, end, Event::SarAdc);
    }
}

impl Peripheral<Ctx> for SarAdc {
    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
        match offset {
            INT_ST => Some(self.get(INT_RAW) & self.get(INT_ENA)),
            INT_CLR => Some(0),
            _ => self.regs.read(offset),
        }
    }

    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        match offset {
            ONETIME_SAMPLE => {
                let rising =
                    value & ONETIME_START != 0 && self.get(ONETIME_SAMPLE) & ONETIME_START == 0;
                self.set(ONETIME_SAMPLE, value);
                if rising {
                    self.start(ctx, value);
                }
            }
            INT_CLR => {
                self.set(INT_RAW, self.get(INT_RAW) & !value);
                self.drive_source(ctx);
            }
            INT_ENA => {
                self.set(INT_ENA, value & INT_BITS);
                self.drive_source(ctx);
                self.refuse_a_visible_scan(ctx);
            }
            DMA_CONF => {
                self.set(DMA_CONF, value);
                self.refuse_a_visible_scan(ctx);
            }
            // Read-only: the results and the interrupt state.
            INT_RAW | INT_ST | SAR1DATA_STATUS | SAR2DATA_STATUS => return false,
            CTRL => {
                if value & START_SCAN != 0 {
                    ctx.unsupported(Unsupported::Adc {
                        refusal: AdcRefusal::Scan,
                    });
                }
                self.set(CTRL, value);
            }
            CTRL2 => {
                self.set(CTRL2, value);
                self.refuse_a_visible_scan(ctx);
            }
            _ => return self.regs.write(offset, value),
        }
        true
    }
}

#[cfg(test)]
mod tests;
