//! The waveform of the LUT register and what it does to a pixel (datasheet §6.6–6.7, research §6).
//!
//! A phase holds one source level and one VCOM level for some frames; the pixel sees their
//! difference. Its optical state moves in proportion to that voltage and to the time, and stops at
//! black or white: a phase is a clamped translation `x ↦ min(max(x + a, lo), hi)` of the state, and
//! a sequence of them is again one ([`Transfer`]), so a whole waveform is one translation per LUT.

use super::config::LUT_BYTES;
pub(crate) use crate::glass::Transfer;

/// Groups of the waveform, each of four phases.
pub(crate) const GROUPS: usize = 10;
/// LUTs: LUT0 to LUT3 drive the sources, LUT4 drives VCOM.
const LUTS: usize = 5;
/// The LUT of VCOM.
const VCOM_LUT: usize = 4;

/// VSH1 or VSH2 in millivolts (command `0x04`, bytes A and B): 23h–4Bh from 9 V by 0.2 V, 8Eh–CEh
/// from 2.4 V by 0.1 V. `None` for a code the datasheet does not list.
pub(crate) fn source_millivolts(code: u8) -> Option<i64> {
    match code {
        0x23..=0x4B => Some(9_000 + i64::from(code - 0x23) * 200),
        0x8E..=0xCE => Some(2_400 + i64::from(code - 0x8E) * 100),
        _ => None,
    }
}

/// VSL in millivolts (command `0x04`, byte C): even codes 1Ah–3Ah, from −9 V by −0.5 V.
pub(crate) fn vsl_millivolts(code: u8) -> Option<i64> {
    match code {
        0x1A..=0x3A if code.is_multiple_of(2) => Some(-9_000 - i64::from(code - 0x1A) * 250),
        _ => None,
    }
}

/// DCVCOM in millivolts (command `0x2C`): 00h (its reset value) is 0 V, then multiples of four
/// from 08h, −0.1 V for each four. The datasheet prints 84h as −3.2 V, which its step makes −3.3 V.
pub(crate) fn vcom_millivolts(code: u8) -> Option<i64> {
    match code {
        0x00 => Some(0),
        0x08..=0xA0 if code.is_multiple_of(4) => Some(-i64::from(code / 4) * 100),
        _ => None,
    }
}

/// VGH codes (command `0x03`): 00h is 20 V, 07h–17h from 12 V by 0.5 V. The gate voltage has no
/// effect on the model's optics; the code is only checked.
pub(crate) fn gate_code_is_listed(code: u8) -> bool {
    matches!(code, 0x00 | 0x07..=0x17)
}

/// The voltages a waveform drives with, in millivolts. A code the datasheet does not list reads
/// 0 V; the command that wrote it reported it.
///
/// ASSUMPTION: DCVCOM (byte 109) drives no pixel. It is set to cancel the kickback of the panel's
/// transistors, so a pixel sees the source level minus the swing of VCOM around it (LUT4's VSH1 or
/// VSL). Were it a drive, every VSS phase would move every pixel, while the X4's grey LUT holds its
/// pixels of code 00 with VSS phases only (research §6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Levels {
    vsh1: i64,
    vsh2: i64,
    vsl: i64,
}

impl Levels {
    pub(crate) fn from_setting(setting: &[u8; LUT_BYTES]) -> Levels {
        Levels {
            vsh1: source_millivolts(setting[106]).unwrap_or(0),
            vsh2: source_millivolts(setting[107]).unwrap_or(0),
            vsl: vsl_millivolts(setting[108]).unwrap_or(0),
        }
    }

    /// The source level of a VS code: 00 VSS, 01 VSH1, 10 VSL, 11 VSH2 (datasheet Table 6-6).
    fn source(&self, code: u8) -> i64 {
        match code & 0b11 {
            0b01 => self.vsh1,
            0b10 => self.vsl,
            0b11 => self.vsh2,
            _ => 0,
        }
    }

    /// The swing of VCOM around DCVCOM for a VS code of LUT4: 00 DCVCOM, 01 VSH1 + DCVCOM, 10 VSL
    /// + DCVCOM; 11 is "N/A" and reads DCVCOM (the write of the LUT reported it).
    fn vcom_swing(&self, code: u8) -> i64 {
        match code & 0b11 {
            0b01 => self.vsh1,
            0b10 => self.vsl,
            _ => 0,
        }
    }
}

/// What drives a pixel or the border during the display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Drive {
    /// The phases of LUT0 to LUT3.
    Lut(usize),
    /// One source level, a VS code, for every frame (the border's fixed level, `0x3C` = 01b).
    Fixed(u8),
}

/// One group of the waveform: four phases, their source codes for each LUT, and its repeats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Group {
    /// `TP[nA]` to `TP[nD]`: frames of each phase, 0 skipping it.
    frames: [u8; 4],
    /// `RP[n]` + 1: how many times the group runs.
    repeats: u16,
    /// The period of a frame, from the group's frame-rate code.
    frame_ns: u64,
    /// VS codes: `codes[lut][phase]`.
    codes: [[u8; 4]; LUTS],
}

/// The waveform of a LUT register, read once at the activation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Waveform {
    groups: [Group; GROUPS],
    levels: Levels,
}

/// The frame-rate code of group `group`: bytes 100 to 104 hold two codes each. ASSUMPTION: the
/// even group in the high nibble; the datasheet only says "5 bytes for frame rate".
fn frame_rate_code(setting: &[u8; LUT_BYTES], group: usize) -> usize {
    let byte = setting[100 + group / 2];
    let code = if group.is_multiple_of(2) {
        byte >> 4
    } else {
        byte & 0x0F
    };
    usize::from(code)
}

impl Waveform {
    /// Reads `setting` (datasheet Figure 6-6) with the frame period of each frame-rate code.
    pub(crate) fn from_setting(setting: &[u8; LUT_BYTES], frame_period: &[u64; 16]) -> Waveform {
        let groups = std::array::from_fn(|group| {
            let timing = &setting[50 + 5 * group..55 + 5 * group];
            let codes = std::array::from_fn(|lut| {
                let byte = setting[10 * lut + group];
                [
                    byte >> 6,
                    (byte >> 4) & 0b11,
                    (byte >> 2) & 0b11,
                    byte & 0b11,
                ]
            });
            Group {
                frames: [timing[0], timing[1], timing[2], timing[3]],
                repeats: u16::from(timing[4]) + 1,
                frame_ns: frame_period[frame_rate_code(setting, group)],
                codes,
            }
        });
        Waveform {
            groups,
            levels: Levels::from_setting(setting),
        }
    }

    /// How long the display runs: every frame of every phase of every repeat.
    pub(crate) fn duration_ns(&self) -> u64 {
        self.groups.iter().fold(0u64, |total, group| {
            let frames: u64 = group.frames.iter().map(|&f| u64::from(f)).sum();
            total.saturating_add(
                frames
                    .saturating_mul(u64::from(group.repeats))
                    .saturating_mul(group.frame_ns),
            )
        })
    }

    /// What `drive` does to a state within `[0, swing]` over the first `until_ns` nanoseconds of
    /// the display (all of it with `None`): whole frames only, a frame cut short does nothing.
    pub(crate) fn transfer(
        &self,
        drive: Drive,
        swing: i64,
        positive_drives_black: bool,
        until_ns: Option<u64>,
    ) -> Transfer {
        let mut transfer = Transfer::identity(swing);
        let mut elapsed: u64 = 0;
        for group in &self.groups {
            for _ in 0..group.repeats {
                for phase in 0..4 {
                    let frames = u64::from(group.frames[phase]);
                    if frames == 0 {
                        continue;
                    }
                    let length = frames.saturating_mul(group.frame_ns);
                    let run = match until_ns {
                        Some(until) if elapsed.saturating_add(length) > until => {
                            // The frames of this phase completed before `until`; then stop.
                            until.saturating_sub(elapsed) / group.frame_ns
                        }
                        _ => frames,
                    };
                    let source = match drive {
                        Drive::Lut(lut) => group.codes[lut][phase],
                        Drive::Fixed(code) => code,
                    };
                    let volts = self.levels.source(source)
                        - self.levels.vcom_swing(group.codes[VCOM_LUT][phase]);
                    // mV × ns: at most 38 V × 255 frames × a frame period, far below i64::MAX for
                    // any period under a minute.
                    let drive_amount = volts
                        .saturating_mul(i64::try_from(run).unwrap_or(i64::MAX))
                        .saturating_mul(i64::try_from(group.frame_ns).unwrap_or(i64::MAX));
                    let whiter = if positive_drives_black {
                        drive_amount.saturating_neg()
                    } else {
                        drive_amount
                    };
                    transfer = transfer.then(Transfer::shift(whiter, swing));
                    if run < frames {
                        return transfer;
                    }
                    elapsed = elapsed.saturating_add(length);
                }
            }
        }
        transfer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A millisecond, in nanoseconds of a duration.
    const MS: u64 = 1_000_000;
    /// A millisecond, in the drives' unit (mV·ns per mV).
    const DRIVE_MS: i64 = 1_000_000;

    #[test]
    fn source_voltages_follow_the_datasheet_tables() {
        assert_eq!(source_millivolts(0x23), Some(9_000));
        assert_eq!(source_millivolts(0x41), Some(15_000), "POR VSH1");
        assert_eq!(source_millivolts(0x4B), Some(17_000));
        assert_eq!(source_millivolts(0x8E), Some(2_400));
        assert_eq!(source_millivolts(0xA8), Some(5_000), "POR VSH2");
        assert_eq!(source_millivolts(0xCE), Some(8_800));
        assert_eq!(source_millivolts(0x22), None);
        assert_eq!(source_millivolts(0x4C), None);
        assert_eq!(source_millivolts(0x8D), None);
        assert_eq!(source_millivolts(0xCF), None);
        assert_eq!(vsl_millivolts(0x1A), Some(-9_000));
        assert_eq!(vsl_millivolts(0x32), Some(-15_000), "POR VSL");
        assert_eq!(vsl_millivolts(0x3A), Some(-17_000));
        assert_eq!(vsl_millivolts(0x1B), None);
        assert_eq!(vsl_millivolts(0x3C), None);
        assert_eq!(vcom_millivolts(0x00), Some(0));
        assert_eq!(vcom_millivolts(0x08), Some(-200));
        assert_eq!(vcom_millivolts(0x30), Some(-1_200));
        assert_eq!(vcom_millivolts(0x84), Some(-3_300));
        assert_eq!(vcom_millivolts(0xA0), Some(-4_000));
        assert_eq!(vcom_millivolts(0x04), None);
        assert_eq!(vcom_millivolts(0x09), None);
        assert_eq!(vcom_millivolts(0xA4), None);
        assert!(
            gate_code_is_listed(0x00) && gate_code_is_listed(0x07) && gate_code_is_listed(0x17)
        );
        assert!(!gate_code_is_listed(0x06) && !gate_code_is_listed(0x18));
    }

    /// A setting with one phase in group 0: `frames` frames of `code` on LUT1, VCOM at DCVCOM.
    fn one_phase(code: u8, frames: u8) -> [u8; LUT_BYTES] {
        let mut s = [0u8; LUT_BYTES];
        s[10] = code << 6;
        s[50] = frames;
        s[106] = 0x41; // VSH1 15 V
        s[107] = 0xA8; // VSH2 5 V
        s[108] = 0x32; // VSL -15 V
        s[109] = 0x30; // VCOM -1.2 V
        s
    }

    #[test]
    fn a_phase_moves_the_state_by_voltage_times_time() {
        let setting = one_phase(0b01, 10);
        let w = Waveform::from_setting(&setting, &[20 * MS; 16]);
        assert_eq!(w.duration_ns(), 200 * MS);
        // VSH1 = 15 V for 10 frames of 20 ms: 3 000 V·ms toward black; DCVCOM (−1.2 V) drives
        // nothing.
        let swing = 6_000_000 * DRIVE_MS;
        let t = w.transfer(Drive::Lut(1), swing, true, None);
        assert_eq!(t.apply(swing), swing - 15_000 * 200 * DRIVE_MS);
        // The other sign convention moves it toward white instead.
        let t = w.transfer(Drive::Lut(1), swing, false, None);
        assert_eq!(t.apply(0), 15_000 * 200 * DRIVE_MS);
        // LUT0 holds VSS: nothing moves.
        let t = w.transfer(Drive::Lut(0), swing, true, None);
        assert_eq!(t.apply(swing / 2), swing / 2);
    }

    #[test]
    fn repeats_frame_rates_and_cut_display_are_counted() {
        let mut setting = one_phase(0b10, 3);
        setting[54] = 4; // RP[0]: five runs
        setting[100] = 0x10; // group 0 frame-rate code 1, group 1 code 0
        let mut periods = [20 * MS; 16];
        periods[1] = 10 * MS;
        let w = Waveform::from_setting(&setting, &periods);
        assert_eq!(w.duration_ns(), 15 * 10 * MS);
        let swing = i64::MAX / 4;
        // VSL = −15 V: toward white with a positive-darkens panel.
        let full = w.transfer(Drive::Lut(1), swing, true, None);
        assert_eq!(full.apply(0), 15_000 * 150 * DRIVE_MS);
        // Cut at 75 ms: 7 whole frames, the eighth is not counted.
        let cut = w.transfer(Drive::Lut(1), swing, true, Some(75 * MS));
        assert_eq!(cut.apply(0), 15_000 * 70 * DRIVE_MS);
        // Cut at 0: nothing.
        assert_eq!(w.transfer(Drive::Lut(1), swing, true, Some(0)).apply(5), 5);
    }

    #[test]
    fn a_fixed_drive_holds_one_source_level_through_the_vcom_phases() {
        let mut setting = one_phase(0b00, 2);
        // LUT4: VSL + DCVCOM on phase A of group 0.
        setting[40] = 0b10 << 6;
        let w = Waveform::from_setting(&setting, &[20 * MS; 16]);
        let swing = i64::MAX / 4;
        // VSH2 (5 V) against VCOM swung to VSL (−15 V): 20 V for 40 ms, toward black.
        let t = w.transfer(Drive::Fixed(0b11), swing, true, None);
        assert_eq!(t.apply(swing), swing - 20_000 * 40 * DRIVE_MS);
        // VCOM swung to VSH1 against VSS: −15 V, toward white.
        setting[40] = 0b01 << 6;
        let w = Waveform::from_setting(&setting, &[20 * MS; 16]);
        let t = w.transfer(Drive::Fixed(0b00), swing, true, None);
        assert_eq!(t.apply(0), 15_000 * 40 * DRIVE_MS);
        // VCOM code 11 reads DCVCOM: no swing, VSS drives nothing.
        setting[40] = 0b11 << 6;
        let w = Waveform::from_setting(&setting, &[20 * MS; 16]);
        let t = w.transfer(Drive::Fixed(0b00), swing, true, None);
        assert_eq!(t.apply(swing / 3), swing / 3);
    }

    #[test]
    fn an_unlisted_voltage_code_reads_zero_volts() {
        let mut setting = one_phase(0b01, 1);
        setting[106] = 0x00;
        setting[109] = 0x01;
        let w = Waveform::from_setting(&setting, &[20 * MS; 16]);
        assert_eq!(w.transfer(Drive::Lut(1), 1_000, true, None).apply(500), 500);
    }

    #[test]
    fn the_longest_waveform_saturates_instead_of_overflowing() {
        let mut setting = [0xFFu8; LUT_BYTES];
        setting[106] = 0x4B;
        setting[107] = 0x4B;
        setting[108] = 0x3A;
        setting[109] = 0xA0;
        let w = Waveform::from_setting(&setting, &[u64::MAX; 16]);
        assert_eq!(w.duration_ns(), u64::MAX);
        // Every phase of LUT3 is VSH2 (17 V) against no VCOM swing (code 11): a saturated drive
        // toward black, whatever the start.
        let t = w.transfer(Drive::Lut(3), i64::MAX, true, None);
        assert_eq!(t.apply(i64::MAX), 0);
        assert_eq!(t.apply(0), 0);
        // The other sign: saturated toward white.
        let t = w.transfer(Drive::Lut(3), i64::MAX, false, None);
        assert_eq!(t.apply(0), i64::MAX);
        // A cut before the first frame of u64::MAX ns ends: a frame cut short does nothing.
        // (`Some(u64::MAX)` would not cut: every phase's length saturates to it.)
        let t = w.transfer(Drive::Lut(3), i64::MAX, true, Some(u64::MAX - 1));
        assert_eq!(t.apply(i64::MAX), i64::MAX);
        assert_eq!(t.apply(0), 0);
        assert_eq!(t.apply(i64::MAX / 2), i64::MAX / 2);
    }

    #[test]
    fn a_cut_after_the_first_of_very_long_frames_runs_exactly_one_frame() {
        let mut setting = [0xFFu8; LUT_BYTES];
        setting[106] = 0x4B;
        setting[107] = 0x4B;
        setting[108] = 0x3A;
        setting[109] = 0xA0;
        // 255 frames of a phase overflow u64 at this period, one frame does not.
        let frame = u64::MAX / 2 + 1;
        let w = Waveform::from_setting(&setting, &[frame; 16]);
        assert_eq!(w.duration_ns(), u64::MAX);
        // One frame of 17 V over `frame` ns saturates the drive: black from white, and the other
        // sign white from black.
        let t = w.transfer(Drive::Lut(3), i64::MAX, true, Some(frame));
        assert_eq!(t.apply(i64::MAX), 0);
        assert_eq!(t.apply(0), 0);
        let t = w.transfer(Drive::Lut(3), i64::MAX, false, Some(frame));
        assert_eq!(t.apply(0), i64::MAX);
        // One nanosecond earlier the first frame has not completed: nothing moves.
        let t = w.transfer(Drive::Lut(3), i64::MAX, true, Some(frame - 1));
        assert_eq!(t.apply(i64::MAX), i64::MAX);
    }
}
