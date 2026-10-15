//! What a pad connects and what it reads: the parts of the IO matrix that are plain functions of
//! the registers (research P §B).
use emu_core::Level;

/// GPIO0 to GPIO21 (`SOC_GPIO_PIN_COUNT 22`, P §B).
pub const PADS: usize = 22;
/// Every pad, one bit each.
pub(crate) const PAD_MASK: u32 = (1 << PADS) - 1;

/// `IO_MUX_GPIOn` `FUN_WPD`, `FUN_WPU`, `FUN_IE` (`io_mux_reg.h:46-56`). A pull-down reads as a
/// pad nobody drives does (low): only the tests name it.
#[cfg(test)]
pub(crate) const FUN_WPD: u32 = 1 << 7;
pub(crate) const FUN_WPU: u32 = 1 << 8;
pub(crate) const FUN_IE: u32 = 1 << 9;
/// `MCU_SEL` `[14:12]`, the pad's function (`io_mux_reg.h:66`).
const MCU_SEL_SHIFT: u32 = 12;
const MCU_SEL_MASK: u32 = 0x7;

/// `GPIO_PINn` `PAD_DRIVER`: the pad drives low only (open drain) (`gpio_reg.h:206`).
pub(crate) const PAD_DRIVER: u32 = 1 << 2;

/// `GPIO_FUNCn_OUT_SEL_CFG` fields (`gpio_reg.h:3888-3906`).
pub(crate) const OUT_SEL_MASK: u32 = 0xFF;
const OUT_INV_SEL: u32 = 1 << 8;
const OEN_SEL: u32 = 1 << 9;
const OEN_INV_SEL: u32 = 1 << 10;
/// `GPIO_FUNCm_IN_SEL_CFG` fields (`gpio_reg.h:1325-1340`).
const IN_SEL_MASK: u32 = 0x1F;
const IN_INV_SEL: u32 = 1 << 5;
const SIG_IN_SEL: u32 = 1 << 6;
/// `IN_SEL` values that read a constant instead of a pad (`gpio_pins.h:13-14`).
const IN_CONST_HIGH: u32 = 0x1E;
const IN_CONST_LOW: u32 = 0x1F;

/// The output signal index of the plain GPIO output, `GPIO.OUT` (`SIG_GPIO_OUT_IDX`).
pub(crate) const GPIO_OUT_SIGNAL: u32 = 128;

/// A peripheral signal of the matrix, by its index in `gpio_sig_map.h` (P §B).
pub type Signal = u32;
pub(crate) const U0TXD: Signal = 6;
pub(crate) const I2C_SCL: Signal = 53;
pub(crate) const I2C_SDA: Signal = 54;
pub(crate) const FSPICLK: Signal = 63;
pub(crate) const FSPIQ: Signal = 64;
pub(crate) const FSPID: Signal = 65;
pub(crate) const FSPIHD: Signal = 66;
pub(crate) const FSPIWP: Signal = 67;
/// FSPICS0 to FSPICS5 are 68 to 73.
pub(crate) const FSPICS0: Signal = 68;

/// The six hardware chip selects of GP-SPI2, FSPICS0 to FSPICS5.
pub(crate) const FSPI_CS: std::ops::Range<Signal> = FSPICS0..FSPICS0 + 6;

/// How a peripheral output signal the model drives rests: its level and whether it enables the
/// pad's output when `OEN_SEL` leaves the choice to the peripheral; `None` for a signal of a
/// peripheral the model does not have (the plain GPIO output, 128, is not a peripheral signal).
///
/// ASSUMPTION: a peripheral enables the output of a signal it drives whenever the signal is
/// routed, and the levels at rest are those of an idle bus: UART TX high, I2C released (high, an
/// open-drain pad then releasing the line), the SPI clock low (mode 0), MOSI low, HD and WP high,
/// every CS high (inactive). FSPIQ is an input of a master: it never enables the pad. A pad's level
/// while a transfer is under way is not modelled: a transfer is delivered whole (ADR 0012), and a
/// chip watching an SPI clock or MOSI pin sees its rest level.
#[must_use]
pub(crate) fn rest(signal: Signal) -> Option<Drive> {
    let (level, enable) = match signal {
        U0TXD | I2C_SCL | I2C_SDA | FSPIHD | FSPIWP => (Level::High, true),
        FSPICLK | FSPID => (Level::Low, true),
        FSPIQ => (Level::High, false),
        cs if FSPI_CS.contains(&cs) => (Level::High, true),
        _ => return None,
    };
    Some(Drive { level, enable })
}

/// What the IO MUX connects to a pad (TRM table 5.12-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Function {
    /// The GPIO matrix: function 1 on every pad, function 0 too on GPIO0-3, GPIO8-11, GPIO18-19.
    Matrix,
    /// An FSPI signal straight from GP-SPI2: function 2 on GPIO2, 4, 5, 6, 7, 10.
    Fspi(Signal),
    /// UART0's TX, function 0 of GPIO21.
    U0Txd,
    /// Anything else: JTAG (function 0 of GPIO4-7), the flash (GPIO12-17), UART0's RX (GPIO20),
    /// functions the pad does not have. The pad is not driven by the model.
    Other,
}

/// The function `IO_MUX_GPIOn` selects for pad `pad`.
#[must_use]
pub(crate) fn function(pad: usize, io_mux: u32) -> Function {
    match ((io_mux >> MCU_SEL_SHIFT) & MCU_SEL_MASK, pad) {
        (1, _) | (0, 0..=3 | 8..=11 | 18 | 19) => Function::Matrix,
        (0, 21) => Function::U0Txd,
        (2, 2) => Function::Fspi(FSPIQ),
        (2, 4) => Function::Fspi(FSPIHD),
        (2, 5) => Function::Fspi(FSPIWP),
        (2, 6) => Function::Fspi(FSPICLK),
        (2, 7) => Function::Fspi(FSPID),
        (2, 10) => Function::Fspi(FSPICS0),
        _ => Function::Other,
    }
}

/// The signal level and output enable a peripheral gives a pad, before `GPIO_FUNCn_OUT_SEL_CFG`'s
/// inversions: what the matrix needs to know of the peripherals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Drive {
    pub level: Level,
    pub enable: bool,
}

/// What the chip drives on a pad, from its function, `GPIO_FUNCn_OUT_SEL_CFG` (`out_sel`), the
/// pad's bits of `GPIO.OUT` and `GPIO.ENABLE`, its `PAD_DRIVER` and what `signal` says of a
/// peripheral signal: `Some(level)` while the output is enabled, `None` when the pad is released
/// (TRM §5.5.2). An open-drain pad drives low only.
#[must_use]
pub(crate) fn driven(
    function: Function,
    out_sel: u32,
    gpio: Drive,
    open_drain: bool,
    signal: impl Fn(Signal) -> Option<Drive>,
) -> Option<Level> {
    let level = match function {
        Function::Matrix => {
            let selected = out_sel & OUT_SEL_MASK;
            let source = if selected == GPIO_OUT_SIGNAL {
                gpio
            } else {
                signal(selected)?
            };
            // `OEN_SEL` set: `GPIO.ENABLE` decides. The plain GPIO output's own enable is that
            // bit too (TRM §5.5.3), so for it both choices agree.
            let enable = if out_sel & OEN_SEL != 0 {
                gpio.enable
            } else {
                source.enable
            } ^ (out_sel & OEN_INV_SEL != 0);
            if !enable {
                return None;
            }
            if out_sel & OUT_INV_SEL != 0 {
                source.level.inverted()
            } else {
                source.level
            }
        }
        Function::Fspi(fspi) => {
            let source = signal(fspi)?;
            if !source.enable {
                return None;
            }
            source.level
        }
        Function::U0Txd => Level::High,
        Function::Other => return None,
    };
    (!(open_drain && level == Level::High)).then_some(level)
}

/// The level of a pad: what the ESP32-C3 drives, else what an external chip drives, else its pull
/// resistor, else low.
///
/// ASSUMPTION: a pad nobody drives and without a pull reads low, and one with both pulls reads
/// high; the TRM says neither (P, open question 2). When the ESP32-C3 and an external chip drive a
/// pad at once, the ESP32-C3's level is read (question 3): on a board that is a fault of the
/// wiring, which the external chip sees as the level the ESP32-C3 drives.
#[must_use]
pub(crate) fn level(driven: Option<Level>, external: Option<Level>, io_mux: u32) -> Level {
    driven.or(external).unwrap_or(if io_mux & FUN_WPU != 0 {
        Level::High
    } else {
        // A pull-down, or nothing.
        Level::Low
    })
}

/// Where an input signal of a peripheral comes from (TRM §5.4.3): a pad, a constant, or the pad
/// the IO MUX connects to it directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputSource {
    Pad { pad: usize, inverted: bool },
    Constant(Level),
}

/// The source of input signal `signal` from its `GPIO_FUNCm_IN_SEL_CFG` (`in_sel`) and the pads'
/// functions (`direct`: the pad whose IO MUX function connects the signal straight, if any).
///
/// ASSUMPTION: a signal taken straight from the IO MUX (`SEL` clear) whose pad is in another
/// function reads low; the TRM does not say what it reads (P §B).
#[must_use]
pub(crate) fn input_source(in_sel: u32, direct: Option<usize>) -> InputSource {
    let inverted = in_sel & IN_INV_SEL != 0;
    if in_sel & SIG_IN_SEL == 0 {
        return direct.map_or(InputSource::Constant(Level::Low), |pad| InputSource::Pad {
            pad,
            inverted,
        });
    }
    match in_sel & IN_SEL_MASK {
        IN_CONST_HIGH => InputSource::Constant(Level::from_bool(!inverted)),
        IN_CONST_LOW => InputSource::Constant(Level::from_bool(inverted)),
        // `IN_SEL` 22 to 29 names no pad. ASSUMPTION: it reads low, as a pad that is not there.
        pad if usize::try_from(pad).is_ok_and(|p| p < PADS) => InputSource::Pad {
            pad: usize::try_from(pad).unwrap_or(0),
            inverted,
        },
        _ => InputSource::Constant(Level::from_bool(inverted)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GPIO_FUNCTION: u32 = 1 << MCU_SEL_SHIFT;
    const HIGH_ENABLED: Drive = Drive {
        level: Level::High,
        enable: true,
    };
    const LOW_DISABLED: Drive = Drive {
        level: Level::Low,
        enable: false,
    };

    fn no_signal(_: Signal) -> Option<Drive> {
        None
    }

    #[test]
    fn the_io_mux_function_of_each_pad_is_that_of_table_5_12_1() {
        for pad in [0, 1, 2, 3, 8, 9, 10, 11, 18, 19] {
            assert_eq!(function(pad, 0), Function::Matrix, "GPIO{pad} function 0");
        }
        for pad in [4, 5, 6, 7, 12, 13, 14, 15, 16, 17, 20] {
            assert_eq!(function(pad, 0), Function::Other, "GPIO{pad} function 0");
        }
        assert_eq!(function(21, 0), Function::U0Txd);
        for pad in 0..PADS {
            assert_eq!(
                function(pad, GPIO_FUNCTION),
                Function::Matrix,
                "GPIO{pad} function 1"
            );
        }
        let fspi = 2 << MCU_SEL_SHIFT;
        assert_eq!(function(6, fspi), Function::Fspi(FSPICLK));
        assert_eq!(function(7, fspi), Function::Fspi(FSPID));
        assert_eq!(function(10, fspi), Function::Fspi(FSPICS0));
        assert_eq!(function(2, fspi), Function::Fspi(FSPIQ));
        assert_eq!(function(4, fspi), Function::Fspi(FSPIHD));
        assert_eq!(function(5, fspi), Function::Fspi(FSPIWP));
        assert_eq!(
            function(8, fspi),
            Function::Other,
            "GPIO8 has no function 2"
        );
        assert_eq!(function(8, 3 << MCU_SEL_SHIFT), Function::Other);
    }

    #[test]
    fn a_plain_gpio_output_follows_out_and_enable() {
        // `gpio_ll_matrix_out_default` writes 128 alone; `ENABLE` turns the output on.
        let out = GPIO_OUT_SIGNAL;
        assert_eq!(
            driven(Function::Matrix, out, HIGH_ENABLED, false, no_signal),
            Some(Level::High)
        );
        assert_eq!(
            driven(Function::Matrix, out, LOW_DISABLED, false, no_signal),
            None
        );
        assert_eq!(
            driven(
                Function::Matrix,
                out | OEN_SEL,
                HIGH_ENABLED,
                false,
                no_signal
            ),
            Some(Level::High)
        );
        assert_eq!(
            driven(
                Function::Matrix,
                out | OUT_INV_SEL,
                HIGH_ENABLED,
                false,
                no_signal
            ),
            Some(Level::Low)
        );
        assert_eq!(
            driven(
                Function::Matrix,
                out | OEN_INV_SEL,
                HIGH_ENABLED,
                false,
                no_signal
            ),
            None,
            "the enable inverted"
        );
    }

    #[test]
    fn an_open_drain_pad_drives_low_only() {
        let out = GPIO_OUT_SIGNAL;
        assert_eq!(
            driven(Function::Matrix, out, HIGH_ENABLED, true, no_signal),
            None
        );
        let low = Drive {
            level: Level::Low,
            enable: true,
        };
        assert_eq!(
            driven(Function::Matrix, out, low, true, no_signal),
            Some(Level::Low)
        );
    }

    #[test]
    fn a_peripheral_signal_takes_its_level_and_enable_unless_oen_sel_says_enable() {
        let cs = |signal: Signal| {
            (signal == FSPICS0).then_some(Drive {
                level: Level::Low,
                enable: true,
            })
        };
        assert_eq!(
            driven(Function::Matrix, FSPICS0, LOW_DISABLED, false, cs),
            Some(Level::Low)
        );
        assert_eq!(
            driven(Function::Matrix, FSPICS0 | OEN_SEL, LOW_DISABLED, false, cs),
            None,
            "OEN_SEL: GPIO.ENABLE, clear here, decides"
        );
        assert_eq!(
            driven(Function::Matrix, 200, HIGH_ENABLED, false, cs),
            None,
            "a signal the model does not drive"
        );
        assert_eq!(
            driven(Function::Fspi(FSPICS0), 0, LOW_DISABLED, false, cs),
            Some(Level::Low)
        );
        assert_eq!(
            driven(Function::U0Txd, 0, LOW_DISABLED, false, no_signal),
            Some(Level::High)
        );
        assert_eq!(
            driven(Function::Other, 0, HIGH_ENABLED, false, no_signal),
            None
        );
    }

    #[test]
    fn a_pad_reads_the_soc_then_a_chip_then_its_pull() {
        assert_eq!(
            level(Some(Level::Low), Some(Level::High), FUN_WPU),
            Level::Low
        );
        assert_eq!(level(None, Some(Level::Low), FUN_WPU), Level::Low);
        assert_eq!(level(None, None, FUN_WPU), Level::High);
        assert_eq!(level(None, None, FUN_WPD), Level::Low);
        assert_eq!(level(None, None, FUN_WPU | FUN_WPD), Level::High);
        assert_eq!(level(None, None, 0), Level::Low, "floating");
    }

    #[test]
    fn an_input_signal_comes_from_a_pad_a_constant_or_the_direct_pad() {
        assert_eq!(
            input_source(SIG_IN_SEL | 7, None),
            InputSource::Pad {
                pad: 7,
                inverted: false
            }
        );
        assert_eq!(
            input_source(SIG_IN_SEL | IN_INV_SEL | 7, None),
            InputSource::Pad {
                pad: 7,
                inverted: true
            }
        );
        assert_eq!(
            input_source(SIG_IN_SEL | IN_CONST_HIGH, None),
            InputSource::Constant(Level::High)
        );
        assert_eq!(
            input_source(SIG_IN_SEL | IN_CONST_LOW, None),
            InputSource::Constant(Level::Low)
        );
        assert_eq!(
            input_source(SIG_IN_SEL | 0x19, None),
            InputSource::Constant(Level::Low)
        );
        assert_eq!(
            input_source(0, Some(2)),
            InputSource::Pad {
                pad: 2,
                inverted: false
            }
        );
        assert_eq!(input_source(0, None), InputSource::Constant(Level::Low));
    }

    #[test]
    fn the_modelled_outputs_are_the_uart_i2c_and_fspi_signals_at_an_idle_bus() {
        let high = Some(Drive {
            level: Level::High,
            enable: true,
        });
        let low = Some(Drive {
            level: Level::Low,
            enable: true,
        });
        for signal in [U0TXD, I2C_SCL, I2C_SDA, FSPIHD, FSPIWP] {
            assert_eq!(rest(signal), high, "{signal}");
        }
        for cs in FSPI_CS {
            assert_eq!(rest(cs), high, "{cs}");
        }
        assert_eq!(rest(FSPICLK), low);
        assert_eq!(rest(FSPID), low);
        assert_eq!(
            rest(FSPIQ),
            Some(Drive {
                level: Level::High,
                enable: false
            }),
            "an input of a master"
        );
        for signal in [0, 9, 45, 74, 127, GPIO_OUT_SIGNAL] {
            assert_eq!(rest(signal), None, "{signal}");
        }
    }
}
