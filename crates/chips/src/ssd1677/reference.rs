//! Reference waveforms for the OTP of the X4's panel, a GDEQ0426T82.
//!
//! No source publishes the waveforms the panel's maker stored in its OTP. These three are built for
//! the model (decision A9 of the `SP2a` design; research `sp2a-external-chips.md` §5): they
//! reproduce the refresh durations the X4's firmwares report with the default stage durations of
//! [`Ssd1677Timing`](super::Ssd1677Timing), and the colours each sequence must give. They are an
//! **approximation**, and an interface showing the panel must say so (spec §5.3).
//!
//! | Setting | Mode | Range | Frames | What it does |
//! |---|---|---|---|---|
//! | 0 | 1 | −128 °C < T ≤ 50 °C | 4 × 20 | full: black and white twice, ending on the target (`0xF7` ≈ 1.8 s) |
//! | 1 | 1 | 50 °C < T ≤ 127.9 °C | 3 × 25 | full, the one the forced 90 °C of `0xD7` selects (≈ 1.7 s) |
//! | 2 | 2 | whole range | 19 + 6 | differential: only LUT1 (to white) and LUT2 (to black) drive, 95 % of a swing, then rest (`0xFC` ≈ 0.5 s) |
//!
//! Every phase of setting 0 is exactly one swing of the default optics (15 V for 400 ms); setting 2
//! stops short of a full swing, so partial refreshes leave a trace that only a full one clears, as
//! on the glass.

use super::config::{DisplayMode, LUT_BYTES, OtpWaveform, Temperature};

/// VS codes.
const VSS: u8 = 0b00;
const VSH1: u8 = 0b01;
const VSL: u8 = 0b10;

/// The voltages of every setting: VGH 20 V (17h), VSH1 15 V (41h), VSH2 5 V (A8h), VSL −15 V (32h),
/// DCVCOM −1.2 V (30h), the voltage block of the X4's factory grey LUT (`FreeInk`
/// `lut_factory_quality`). ASSUMPTION: the panel's OTP holds the same.
const VOLTAGES: [u8; 5] = [0x17, 0x41, 0xA8, 0x32, 0x30];

/// The frame-rate bytes: code 2 for every group, as the X4's factory LUT (all codes take the same
/// period by default).
const FRAME_RATE: u8 = 0x22;

/// A setting whose group `g` runs phase A for `frames[g]` frames, then phase B for `rest[g]`
/// frames at VSS, with `codes[lut][g]` as phase A's source code of LUT0 to LUT3.
fn setting(frames: &[u8], rest: &[u8], codes: [&[u8]; 4]) -> [u8; LUT_BYTES] {
    let mut s = [0u8; LUT_BYTES];
    for (lut, lut_codes) in codes.iter().enumerate() {
        for (group, &code) in lut_codes.iter().enumerate() {
            s[10 * lut + group] = code << 6;
        }
    }
    for (group, &count) in frames.iter().enumerate() {
        s[50 + 5 * group] = count;
        s[51 + 5 * group] = rest.get(group).copied().unwrap_or(0);
    }
    s[100..105].fill(FRAME_RATE);
    s[105..110].copy_from_slice(&VOLTAGES);
    s
}

/// The three reference settings of the GDEQ0426T82, in the order the module's table gives.
#[must_use]
pub fn gdeq0426t82_otp() -> Vec<OtpWaveform> {
    let black_4 = [VSL, VSH1, VSL, VSH1];
    let white_4 = [VSH1, VSL, VSH1, VSL];
    let black_3 = [VSH1, VSL, VSH1];
    let white_3 = [VSL, VSH1, VSL];
    let fifty = Temperature::from_celsius(50);
    vec![
        OtpWaveform {
            mode: DisplayMode::One,
            lower: Temperature::MIN,
            upper: fifty,
            setting: setting(&[20; 4], &[], [&black_4, &white_4, &black_4, &white_4]),
        },
        OtpWaveform {
            mode: DisplayMode::One,
            lower: fifty,
            upper: Temperature::MAX,
            setting: setting(&[25; 3], &[], [&black_3, &white_3, &black_3, &white_3]),
        },
        OtpWaveform {
            mode: DisplayMode::Two,
            lower: Temperature::MIN,
            upper: Temperature::MAX,
            setting: setting(&[19], &[6], [&[VSS], &[VSL], &[VSH1], &[VSS]]),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::super::config::Ssd1677Timing;
    use super::super::waveform::{Drive, Waveform};
    use super::*;

    const MS: u64 = 1_000_000;
    /// The default swing of [`Optics`](super::super::Optics): 6 000 V·ms in mV·ns.
    const SWING: i64 = 6_000_000_000_000;

    fn waveform(index: usize) -> Waveform {
        Waveform::from_setting(
            &gdeq0426t82_otp()[index].setting,
            &Ssd1677Timing::default().frame_period,
        )
    }

    #[test]
    fn the_reference_settings_last_as_long_as_the_x4_refreshes() {
        assert_eq!(waveform(0).duration_ns(), 1_600 * MS);
        assert_eq!(waveform(1).duration_ns(), 1_500 * MS);
        assert_eq!(waveform(2).duration_ns(), 500 * MS);
    }

    #[test]
    fn the_full_settings_end_every_pixel_on_its_target_from_anywhere() {
        let swing = SWING;
        for index in [0, 1] {
            let w = waveform(index);
            for start in [0, swing / 3, swing] {
                for lut in [0, 2] {
                    assert_eq!(
                        w.transfer(Drive::Lut(lut), swing, true, None).apply(start),
                        0
                    );
                }
                for lut in [1, 3] {
                    assert_eq!(
                        w.transfer(Drive::Lut(lut), swing, true, None).apply(start),
                        swing
                    );
                }
            }
        }
    }

    #[test]
    fn the_differential_setting_drives_only_the_pixels_that_change_and_short_of_a_swing() {
        let swing = SWING;
        let w = waveform(2);
        let to = |lut, start| w.transfer(Drive::Lut(lut), swing, true, None).apply(start);
        assert_eq!(to(0, 0), 0);
        assert_eq!(to(3, swing), swing);
        // 15 V for 380 ms: 95 % of a swing.
        assert_eq!(to(1, 0), swing / 20 * 19);
        assert_eq!(to(2, swing), swing / 20);
    }

    #[test]
    fn the_ranges_split_at_fifty_degrees() {
        let otp = gdeq0426t82_otp();
        assert_eq!(otp.len(), 3);
        assert_eq!(otp[0].upper, otp[1].lower);
        assert_eq!(otp[0].upper.register(), 0x320);
        assert!(otp.iter().all(|w| w.setting[105..110] == VOLTAGES));
    }
}
