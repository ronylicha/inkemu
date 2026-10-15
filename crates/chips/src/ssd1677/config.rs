//! The parameters of an SSD1677 and of the panel it drives: what the datasheet leaves open, what
//! belongs to the glass, and what no source gives (research `sp2a-external-chips.md` §5). The
//! defaults are the X4's panel, a GDEQ0426T82; a profile overrides what differs.

use super::reference;
use super::waveform::{source_millivolts, vcom_millivolts, vsl_millivolts};

/// Sources of the controller: the width of each RAM plane, in pixels (datasheet §1).
pub const RAM_SOURCES: u16 = 960;
/// Gates of the controller: the height of each RAM plane, in rows (datasheet §1).
pub const RAM_GATES: u16 = 680;
/// Bytes of a waveform setting: the LUT, the voltages and two reserved bytes (datasheet §6.7).
pub const LUT_BYTES: usize = 112;

const MILLISECOND: u64 = 1_000_000;

/// The gate-scan bits of command `0x01`: SM (no left and right interlacing) and GD (the first
/// output gate is G1). The datasheet's Table 8-2 gives the RAM row each gate shows for each pair.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GateScan {
    pub sm: bool,
    pub gd: bool,
}

/// The glass behind the controller: which sources and gates it uses, and in what order its rows lie.
///
/// A panel's rows are wired to the gates in one order; the firmware of the device chooses the
/// gate-scan setting under which the RAM's rows come out in sequence (`native_scan`), and whether
/// the last RAM row is the top of the glass (`rows_reversed`). Under another setting, the rows
/// come out in the order the datasheet's Table 8-2 gives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelGeometry {
    /// Pixels in a row: the sources the panel uses.
    pub width: u16,
    /// Rows: the gates the panel uses, from G0.
    pub height: u16,
    /// The source of the panel's first column; RAM column *x* drives source *x*.
    pub first_source: u16,
    /// The panel's first column is its last source.
    pub columns_reversed: bool,
    /// The gate-scan setting under which the RAM's rows lie on the glass in sequence.
    pub native_scan: GateScan,
    /// Under the native setting, the last RAM row is the top row of the glass.
    pub rows_reversed: bool,
}

impl PanelGeometry {
    /// The X4's 800×480 GDEQ0426T82: the firmware scans with SM = 1 and writes its top row to the
    /// last RAM row ("Gates are physically reversed on this panel", `FreeInk`).
    #[must_use]
    pub const fn gdeq0426t82() -> PanelGeometry {
        PanelGeometry {
            width: 800,
            height: 480,
            first_source: 0,
            columns_reversed: false,
            native_scan: GateScan {
                sm: true,
                gd: false,
            },
            rows_reversed: true,
        }
    }
}

impl Default for PanelGeometry {
    fn default() -> Self {
        PanelGeometry::gdeq0426t82()
    }
}

/// How the glass answers the waveform (spec §5.3, research §6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Optics {
    /// The drive a pixel needs to go from black to white, in millivolt-nanoseconds (the pixel's
    /// voltage times the time it is applied). ASSUMPTION: 6 V·s by default; not measured.
    pub full_swing: u64,
    /// A positive pixel voltage (source above VCOM) darkens the pixel. True on the X4: its factory
    /// grey LUT only ends on the right colours that way.
    pub positive_drives_black: bool,
    /// The grey level of every pixel and of the border at power-on, 0 black to 255 white.
    pub initial_gray: u8,
}

impl Default for Optics {
    fn default() -> Self {
        Optics {
            full_swing: 6_000 * MILLISECOND * 1_000,
            positive_drives_black: true,
            initial_gray: 255,
        }
    }
}

/// How long each operation keeps BUSY high, in nanoseconds of device time. Every value the
/// datasheet does not give is an ASSUMPTION (research §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ssd1677Timing {
    /// Starting the clock (`0x22` bit 7).
    pub clock_on: u64,
    /// Starting the analog supplies (bit 6): about 100 ms (Papyrix).
    pub analog_on: u64,
    /// Stopping them (bit 1): about 200 ms (Papyrix; `FreeInk` waits 200 ms).
    pub analog_off: u64,
    /// Reading the internal temperature sensor (bit 5, `0x18` = 80h).
    pub temperature_internal: u64,
    /// Reading the external sensor over the controller's I2C (bit 5, `0x18` = 48h).
    pub temperature_external: u64,
    /// Loading a waveform from OTP (bit 4).
    pub lut_load: u64,
    /// The SW reset (`0x12`).
    pub soft_reset: u64,
    /// After RES# rises.
    pub hardware_reset: u64,
    /// The auto write of a plane (`0x46`, `0x47`).
    pub auto_write: u64,
    /// The VCI detection (`0x15`).
    pub vci_detection: u64,
    /// The period of one frame for each of the sixteen frame-rate codes of the LUT. ASSUMPTION:
    /// 20 ms for each, the datasheet does not tabulate them.
    pub frame_period: [u64; 16],
}

impl Default for Ssd1677Timing {
    fn default() -> Self {
        Ssd1677Timing {
            clock_on: 0,
            analog_on: 100 * MILLISECOND,
            analog_off: 200 * MILLISECOND,
            temperature_internal: 5 * MILLISECOND,
            temperature_external: 5 * MILLISECOND,
            lut_load: MILLISECOND,
            soft_reset: 2 * MILLISECOND,
            hardware_reset: MILLISECOND,
            auto_write: 2 * MILLISECOND,
            vci_detection: MILLISECOND,
            frame_period: [20 * MILLISECOND; 16],
        }
    }
}

/// A temperature in the controller's format: twelve bits, two's complement, in sixteenths of a
/// degree Celsius (datasheet §6.8.3), from −128 °C to 127.9375 °C.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Temperature(i16);

impl Temperature {
    /// The lowest value of the format, 800h, −128 °C.
    pub const MIN: Temperature = Temperature(-2048);
    /// The highest value of the format, 7FFh, 127.9375 °C; the temperature register's reset value.
    pub const MAX: Temperature = Temperature(2047);

    /// `sixteenths` of a degree Celsius; `None` outside the twelve bits.
    #[must_use]
    pub const fn from_sixteenths(sixteenths: i16) -> Option<Temperature> {
        if sixteenths >= -2048 && sixteenths <= 2047 {
            Some(Temperature(sixteenths))
        } else {
            None
        }
    }

    /// A whole number of degrees Celsius, saturated to the format.
    #[must_use]
    pub const fn from_celsius(celsius: i8) -> Temperature {
        let sixteenths = celsius as i16 * 16;
        if sixteenths > 2047 {
            Temperature(2047)
        } else {
            Temperature(sixteenths)
        }
    }

    /// The temperature in sixteenths of a degree.
    #[must_use]
    pub const fn sixteenths(self) -> i16 {
        self.0
    }

    /// The twelve-bit register value.
    #[must_use]
    pub fn register(self) -> u16 {
        // The low twelve bits of the two's complement: the mask makes the value non-negative.
        u16::try_from(i32::from(self.0) & 0xFFF).unwrap_or(0)
    }

    /// The temperature a twelve-bit register value stands for; bits above the twelfth are ignored.
    #[must_use]
    pub fn from_register(register: u16) -> Temperature {
        let value = i32::from(register & 0xFFF);
        let signed = if value & 0x800 == 0 {
            value
        } else {
            value - 0x1000
        };
        // Twelve bits, sign-extended: always within an i16.
        Temperature(i16::try_from(signed).unwrap_or(0))
    }
}

/// The two display modes of `0x22` bit 3 and of the waveform settings in OTP (`0x37`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DisplayMode {
    /// Mode 1: the full waveforms on the X4 (`0xF7`, `0xD7`).
    One,
    /// Mode 2: the fast, differential waveform on the X4 (`0xFC`) and the custom-LUT one (`0xCC`).
    Two,
}

/// One waveform setting (WS) of the OTP with its temperature range (TR) and its display mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OtpWaveform {
    pub mode: DisplayMode,
    /// The range is `lower < T ≤ upper` (datasheet §6.9).
    pub lower: Temperature,
    pub upper: Temperature,
    /// The 112 bytes of the setting, laid out as the LUT register (datasheet Figure 6-6).
    pub setting: [u8; LUT_BYTES],
}

/// The waveform the controller loads for `mode` at `temperature`: the last setting of that mode
/// whose range holds it ("The last match will be selected", datasheet §6.9).
pub(crate) fn find_waveform(
    otp: &[OtpWaveform],
    mode: DisplayMode,
    temperature: Temperature,
) -> Option<&OtpWaveform> {
    otp.iter()
        .rev()
        .find(|w| w.mode == mode && w.lower < temperature && temperature <= w.upper)
}

/// Everything an SSD1677 model is built from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ssd1677Config {
    pub panel: PanelGeometry,
    pub optics: Optics,
    pub timing: Ssd1677Timing,
    /// The waveform settings in the panel's OTP. The default ones are approximations (A9).
    pub otp: Vec<OtpWaveform>,
    /// BUSY is high while busy (the SSD1677 does so, datasheet Table 5-2); a board that inverts
    /// the line sets it false.
    pub busy_active_high: bool,
    /// What the internal sensor reads.
    pub ambient: Temperature,
    /// What a sensor on the controller's own I2C reads; `None` when nothing is connected there.
    pub external_sensor: Option<Temperature>,
    /// The supply VCI, for the VCI detection (`0x15`).
    pub vci_millivolts: u32,
    /// The user ID of the OTP, read by `0x2E` and loaded into its register at reset.
    pub user_id: [u8; 10],
}

impl Ssd1677Config {
    /// The X4's panel, GDEQ0426T82, at 25 °C on a 3.3 V supply.
    #[must_use]
    pub fn gdeq0426t82() -> Ssd1677Config {
        Ssd1677Config {
            panel: PanelGeometry::gdeq0426t82(),
            optics: Optics::default(),
            timing: Ssd1677Timing::default(),
            otp: reference::gdeq0426t82_otp(),
            busy_active_high: true,
            ambient: Temperature::from_celsius(25),
            external_sensor: None,
            vci_millivolts: 3_300,
            user_id: [0; 10],
        }
    }

    /// The parameters a model cannot be built on.
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        let panel = &self.panel;
        if panel.width == 0 || panel.height == 0 {
            return Err(ConfigError::EmptyPanel);
        }
        if u32::from(panel.first_source) + u32::from(panel.width) > u32::from(RAM_SOURCES)
            || panel.height > RAM_GATES
        {
            return Err(ConfigError::PanelOutsideRam {
                first_source: panel.first_source,
                width: panel.width,
                height: panel.height,
            });
        }
        if panel.native_scan.gd && !panel.height.is_multiple_of(2) {
            return Err(ConfigError::OddRowsWithGd {
                height: panel.height,
            });
        }
        if self.optics.full_swing == 0 {
            return Err(ConfigError::ZeroSwing);
        }
        if let Some(code) = self.timing.frame_period.iter().position(|&p| p == 0) {
            return Err(ConfigError::ZeroFramePeriod { code });
        }
        for (index, waveform) in self.otp.iter().enumerate() {
            let s = &waveform.setting;
            let valid = source_millivolts(s[106]).is_some()
                && source_millivolts(s[107]).is_some()
                && vsl_millivolts(s[108]).is_some()
                && vcom_millivolts(s[109]).is_some();
            if !valid {
                return Err(ConfigError::OtpVoltage { index });
            }
        }
        Ok(())
    }
}

impl Default for Ssd1677Config {
    fn default() -> Self {
        Ssd1677Config::gdeq0426t82()
    }
}

/// Parameters an SSD1677 model cannot be built on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("the panel has no pixel: its width and height must not be 0")]
    EmptyPanel,
    #[error(
        "the panel ({width}×{height} from source {first_source}) does not fit the controller's \
         960 sources and 680 gates"
    )]
    PanelOutsideRam {
        first_source: u16,
        width: u16,
        height: u16,
    },
    #[error(
        "a native gate scan with GD = 1 swaps gates in pairs, which a panel of {height} rows (odd) \
         cannot hold"
    )]
    OddRowsWithGd { height: u16 },
    #[error("the drive of a full swing is 0: every pixel would jump between black and white")]
    ZeroSwing,
    #[error("the frame period of frame-rate code {code} is 0")]
    ZeroFramePeriod { code: usize },
    #[error(
        "waveform {index} of the OTP holds a source or VCOM voltage the datasheet does not list \
         (bytes 106 to 109)"
    )]
    OtpVoltage { index: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temperatures_convert_to_and_from_the_twelve_bit_register() {
        // Datasheet Table 6-7.
        for (sixteenths, register) in [
            (2047, 0x7FF),
            (1600, 0x640),
            (400, 0x190),
            (4, 0x004),
            (0, 0x000),
            (-4, 0xFFC),
            (-400, 0xE70),
            (-880, 0xC90),
            (-2048, 0x800),
        ] {
            let t = Temperature::from_sixteenths(sixteenths).unwrap();
            assert_eq!(t.register(), register, "{sixteenths}");
            assert_eq!(Temperature::from_register(register), t, "{register:#x}");
        }
        assert_eq!(Temperature::from_register(0xF190), Temperature(400));
        assert_eq!(Temperature::from_sixteenths(2048), None);
        assert_eq!(Temperature::from_sixteenths(-2049), None);
        assert_eq!(Temperature::from_celsius(90).register(), 0x5A0);
        assert_eq!(Temperature::from_celsius(127), Temperature(2032));
        assert_eq!(Temperature::from_celsius(-128), Temperature::MIN);
        assert_eq!(Temperature::MAX.sixteenths(), 2047);
    }

    fn waveform(mode: DisplayMode, lower: i8, upper: i8, tag: u8) -> OtpWaveform {
        let mut setting = reference::gdeq0426t82_otp()[0].setting;
        setting[0] = tag;
        OtpWaveform {
            mode,
            lower: Temperature::from_celsius(lower),
            upper: Temperature::from_celsius(upper),
            setting,
        }
    }

    #[test]
    fn the_last_matching_waveform_of_the_mode_is_chosen_lower_bound_excluded() {
        // Datasheet Table 6-8: 34 °C lies in TR6 and TR7; TR7, the last, is chosen.
        let otp = vec![
            waveform(DisplayMode::One, 30, 35, 6),
            waveform(DisplayMode::One, 33, 127, 7),
            waveform(DisplayMode::Two, 0, 127, 9),
        ];
        let at = |c| Temperature::from_celsius(c);
        let tag = |w: Option<&OtpWaveform>| w.map(|w| w.setting[0]);
        assert_eq!(tag(find_waveform(&otp, DisplayMode::One, at(34))), Some(7));
        assert_eq!(tag(find_waveform(&otp, DisplayMode::One, at(31))), Some(6));
        assert_eq!(tag(find_waveform(&otp, DisplayMode::One, at(30))), None);
        assert_eq!(tag(find_waveform(&otp, DisplayMode::One, at(35))), Some(7));
        assert_eq!(tag(find_waveform(&otp, DisplayMode::Two, at(34))), Some(9));
        assert_eq!(tag(find_waveform(&otp, DisplayMode::Two, at(0))), None);
    }

    #[test]
    fn the_default_is_the_x4_panel_and_validates() {
        let config = Ssd1677Config::default();
        assert_eq!(config.panel, PanelGeometry::gdeq0426t82());
        assert_eq!((config.panel.width, config.panel.height), (800, 480));
        assert_eq!(config.validate(), Ok(()));
        assert_eq!(PanelGeometry::default(), PanelGeometry::gdeq0426t82());
    }

    #[test]
    fn parameters_a_model_cannot_be_built_on_are_refused() {
        let base = Ssd1677Config::default();
        let refused = |change: &dyn Fn(&mut Ssd1677Config)| {
            let mut c = base.clone();
            change(&mut c);
            c.validate().unwrap_err()
        };
        assert_eq!(refused(&|c| c.panel.width = 0), ConfigError::EmptyPanel);
        assert_eq!(refused(&|c| c.panel.height = 0), ConfigError::EmptyPanel);
        assert_eq!(
            refused(&|c| c.panel.first_source = 161),
            ConfigError::PanelOutsideRam {
                first_source: 161,
                width: 800,
                height: 480
            }
        );
        assert!(matches!(
            refused(&|c| c.panel.height = 681),
            ConfigError::PanelOutsideRam { .. }
        ));
        assert_eq!(
            refused(&|c| {
                c.panel.native_scan.gd = true;
                c.panel.height = 479;
            }),
            ConfigError::OddRowsWithGd { height: 479 }
        );
        assert_eq!(
            refused(&|c| c.optics.full_swing = 0),
            ConfigError::ZeroSwing
        );
        assert_eq!(
            refused(&|c| c.timing.frame_period[3] = 0),
            ConfigError::ZeroFramePeriod { code: 3 }
        );
        assert_eq!(
            refused(&|c| c.otp[1].setting[108] = 0x33),
            ConfigError::OtpVoltage { index: 1 }
        );
        // The widest panel that fits: every source and every gate.
        let mut c = base.clone();
        c.panel.width = 960;
        c.panel.height = 680;
        assert_eq!(c.validate(), Ok(()));
    }
}
