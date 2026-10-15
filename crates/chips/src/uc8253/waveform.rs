//! The waveform of the five LUT registers in KW mode and what it does to a pixel (DS79 "LUT",
//! "Data transmission waveform"; research `sp3-x3-chips.md` §2.5).
//!
//! A LUT is seven groups of six bytes: four 2-bit level selects (phase A in bits 7:6), the frame
//! counts of the four phases, and how many times the group runs. The waveform ends after the
//! seventh group, or at the first group that runs no time or has no frame. A pixel's voltage in a
//! phase is its source level less the swing of VCOM in the same phase; the optical state moves in
//! proportion to that voltage and the time (`crate::glass`).

use crate::glass::Transfer;

use super::config::LUT_BYTES;

/// Groups of a LUT in KW mode.
const GROUPS: usize = 7;
/// Bytes of a group.
const GROUP_BYTES: usize = 6;
const _: () = assert!(GROUPS * GROUP_BYTES == LUT_BYTES);
/// Phases of a group.
const PHASES: usize = 4;

/// The LUTs: 0 drives VCOM (`0x20`), 1 to 4 the pixels by their old and new bits.
pub(crate) const LUTS: usize = 5;
/// The LUT of VCOM.
pub(crate) const VCOM_LUT: usize = 0;
/// White stays white (`0x21`).
pub(crate) const LUT_WW: usize = 1;
/// Black to white (`0x22`, LUTKW).
pub(crate) const LUT_KW: usize = 2;
/// White to black (`0x23`, LUTWK).
pub(crate) const LUT_WK: usize = 3;
/// Black stays black (`0x24`, LUTKK).
pub(crate) const LUT_KK: usize = 4;

/// Frames of `VCOM_DC` the controller adds at the end of a waveform before VCOM floats (DS79).
const TAIL_FRAMES: u64 = 2;

/// The supply levels a LUT selects, in millivolts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Levels {
    vdh: i64,
    vdl: i64,
    vdhr: i64,
}

/// `VDH_LVL`, `VDL_LVL` and `VDHR_LVL` of `PWR` (`0x01`, bytes 3 to 5) in millivolts.
///
/// ASSUMPTION: DS79's scale, 2.4 V plus 0.2 V a step, 15 V at `0x3F` (negative for VDL); the
/// X3's controller is a UC8179-class part (research `sp3-x3-chips.md` §1), whose datasheet the
/// X3's bytes decode with. A step above `0x3F` is read as `0x3F`.
fn supply_millivolts(code: u8) -> i64 {
    2_400 + 200 * i64::from(code.min(0x3F))
}

impl Levels {
    pub(crate) fn from_power_setting(power: [u8; 5]) -> Levels {
        Levels {
            vdh: supply_millivolts(power[2] & 0x3F),
            vdl: -supply_millivolts(power[3] & 0x3F),
            vdhr: supply_millivolts(power[4] & 0x3F),
        }
    }

    /// The source level of select `code` for a pixel LUT: `00` GND, `01` VDH, `10` VDL, `11` VDHR.
    fn source(self, code: u8) -> i64 {
        match code & 0b11 {
            0b01 => self.vdh,
            0b10 => self.vdl,
            0b11 => self.vdhr,
            _ => 0,
        }
    }

    /// The swing of VCOM above `VCOM_DC` for select `code` of the VCOM LUT: `00` `VCOM_DC`, `01`
    /// VDH + `VCOM_DC`, `10` VDL + `VCOM_DC`; `11` floats, `None`: no field across the pixel.
    fn vcom_swing(self, code: u8) -> Option<i64> {
        match code & 0b11 {
            0b00 => Some(0),
            0b01 => Some(self.vdh),
            0b10 => Some(self.vdl),
            _ => None,
        }
    }
}

/// One group of a LUT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Group {
    /// The level selects of phases A to D.
    codes: [u8; PHASES],
    frames: [u8; PHASES],
    /// How many times the group runs.
    repeats: u8,
}

impl Group {
    fn read(lut: &[u8; LUT_BYTES], group: usize) -> Group {
        let at = group * GROUP_BYTES;
        let byte = |offset: usize| lut.get(at + offset).copied().unwrap_or(0);
        let levels = byte(0);
        Group {
            codes: [
                levels >> 6,
                (levels >> 4) & 0b11,
                (levels >> 2) & 0b11,
                levels & 0b11,
            ],
            frames: [byte(1), byte(2), byte(3), byte(4)],
            repeats: byte(5),
        }
    }

    /// The waveform ends here: the group runs no time, or has no frame.
    fn ends(self) -> bool {
        self.repeats == 0 || self.frames.iter().all(|&frames| frames == 0)
    }

    fn frames(self) -> u64 {
        self.frames
            .iter()
            .map(|&frames| u64::from(frames))
            .sum::<u64>()
            * u64::from(self.repeats)
    }
}

/// The five LUTs as a refresh reads them, with the supplies and the frame period.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Waveform {
    groups: [[Group; GROUPS]; LUTS],
    levels: Levels,
    frame_ns: u64,
}

impl Waveform {
    pub(crate) fn new(luts: &[[u8; LUT_BYTES]; LUTS], levels: Levels, frame_ns: u64) -> Waveform {
        Waveform {
            groups: std::array::from_fn(|lut| {
                std::array::from_fn(|group| Group::read(&luts[lut], group))
            }),
            levels,
            frame_ns,
        }
    }

    /// The groups of LUT `lut` that run, up to its end.
    fn running(&self, lut: usize) -> impl Iterator<Item = (usize, Group)> + '_ {
        self.groups
            .get(lut)
            .into_iter()
            .flat_map(|groups| groups.iter().copied().enumerate())
            .take_while(|(_, group)| !group.ends())
    }

    /// How long the refresh drives the glass: the longest of the five LUTs, then the two frames
    /// of `VCOM_DC`.
    pub(crate) fn duration_ns(&self) -> u64 {
        let frames = (0..LUTS)
            .map(|lut| {
                self.running(lut)
                    .map(|(_, group)| group.frames())
                    .sum::<u64>()
            })
            .max()
            .unwrap_or(0);
        frames
            .saturating_add(TAIL_FRAMES)
            .saturating_mul(self.frame_ns)
    }

    /// The VCOM swing in phase `phase` of group `group`, `VCOM_DC` once the VCOM LUT has ended.
    fn vcom(&self, group: usize, phase: usize) -> Option<i64> {
        let vcom = self
            .running(VCOM_LUT)
            .find(|(index, _)| *index == group)
            .map_or(0, |(_, group)| group.codes[phase]);
        self.levels.vcom_swing(vcom)
    }

    /// What pixel LUT `lut` does to a state within `[0, swing]` over the first `until_ns`
    /// nanoseconds of the refresh (all of it with `None`): whole frames only, a frame cut short
    /// does nothing.
    ///
    /// ASSUMPTION: the five LUTs share their timing, as every X3 bank has them (the VCOM level of
    /// a phase is read in the same group and phase of LUT `0x20`); DS79 does not say what a
    /// controller does when they differ.
    pub(crate) fn transfer(
        &self,
        lut: usize,
        swing: i64,
        positive_drives_black: bool,
        until_ns: Option<u64>,
    ) -> Transfer {
        let mut transfer = Transfer::identity(swing);
        let mut elapsed: u64 = 0;
        for (index, group) in self.running(lut) {
            for _ in 0..group.repeats {
                for phase in 0..PHASES {
                    let frames = u64::from(group.frames[phase]);
                    if frames == 0 {
                        continue;
                    }
                    let length = frames.saturating_mul(self.frame_ns);
                    let run = match until_ns {
                        Some(until) if elapsed.saturating_add(length) > until => {
                            until.saturating_sub(elapsed) / self.frame_ns
                        }
                        _ => frames,
                    };
                    if let Some(vcom) = self.vcom(index, phase) {
                        let volts = self.levels.source(group.codes[phase]) - vcom;
                        // mV × ns: at most 30 V × 255 frames × a frame period of 200 ms, far below
                        // i64::MAX.
                        let drive = volts
                            .saturating_mul(i64::try_from(run).unwrap_or(i64::MAX))
                            .saturating_mul(i64::try_from(self.frame_ns).unwrap_or(i64::MAX));
                        let whiter = if positive_drives_black {
                            drive.saturating_neg()
                        } else {
                            drive
                        };
                        transfer = transfer.then(Transfer::shift(whiter, swing));
                    }
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

    const FRAME: u64 = 20_000_000;

    /// A LUT of one group: `levels` for its four phases, `frames` each, run once.
    fn lut(levels: u8, frames: [u8; 4]) -> [u8; LUT_BYTES] {
        let mut lut = [0; LUT_BYTES];
        lut[0] = levels;
        lut[1..5].copy_from_slice(&frames);
        lut[5] = 1;
        lut
    }

    fn levels() -> Levels {
        Levels::from_power_setting([0x07, 0x17, 0x3F, 0x3F, 0x17])
    }

    #[test]
    fn the_x3_supplies_are_15_volts_and_7() {
        let levels = levels();
        assert_eq!(
            (levels.vdh, levels.vdl, levels.vdhr),
            (15_000, -15_000, 7_000)
        );
        assert_eq!(supply_millivolts(0), 2_400);
    }

    #[test]
    fn a_lut_drives_toward_black_with_vdh_and_white_with_vdl() {
        let vcom = lut(0x00, [4, 0, 0, 0]);
        let to_black = lut(0x40, [4, 0, 0, 0]);
        let to_white = lut(0x80, [4, 0, 0, 0]);
        let luts = [vcom, to_white, to_white, to_black, to_black];
        let waveform = Waveform::new(&luts, levels(), FRAME);
        let swing = 15_000 * 4 * i64::try_from(FRAME).unwrap();
        let white = waveform.transfer(LUT_KW, swing, true, None);
        let black = waveform.transfer(LUT_WK, swing, true, None);
        assert_eq!(white.apply(0), swing, "four frames of VDL: black to white");
        assert_eq!(black.apply(swing), 0, "four frames of VDH: white to black");
        assert_eq!(
            waveform.duration_ns(),
            (4 + 2) * FRAME,
            "plus two frames of VCOM_DC"
        );
    }

    #[test]
    fn a_waveform_ends_at_its_first_empty_group_and_counts_its_repeats() {
        let mut long = lut(0x40, [1, 1, 0, 0]);
        long[5] = 3;
        long[6] = 0x40;
        long[7] = 9;
        long[11] = 1;
        let mut cut = long;
        cut[11] = 0;
        let flat = lut(0, [0, 0, 0, 0]);
        let waveform = Waveform::new(&[flat, long, flat, flat, flat], levels(), FRAME);
        assert_eq!(waveform.duration_ns(), (6 + 9 + 2) * FRAME);
        let waveform = Waveform::new(&[flat, cut, flat, flat, flat], levels(), FRAME);
        assert_eq!(
            waveform.duration_ns(),
            (6 + 2) * FRAME,
            "a group run no time ends it"
        );
    }

    #[test]
    fn a_floating_vcom_moves_no_pixel_and_a_cut_counts_whole_frames() {
        let floating = lut(0xC0, [4, 0, 0, 0]);
        let to_black = lut(0x40, [4, 0, 0, 0]);
        let swing = 1_000_000_000_000;
        let waveform = Waveform::new(
            &[floating, to_black, to_black, to_black, to_black],
            levels(),
            FRAME,
        );
        assert_eq!(
            waveform.transfer(LUT_WK, swing, true, None).apply(swing),
            swing
        );
        let driven = Waveform::new(
            &[lut(0, [4, 0, 0, 0]), to_black, to_black, to_black, to_black],
            levels(),
            FRAME,
        );
        let one_frame = 15_000 * i64::try_from(FRAME).unwrap();
        let cut = driven.transfer(LUT_WK, swing, true, Some(FRAME + FRAME / 2));
        assert_eq!(
            cut.apply(swing),
            swing - one_frame,
            "one whole frame by 1.5"
        );
    }
}
