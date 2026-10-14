//! The admitted differences between inkemu and QEMU (decision H3), one numbered row each, and the
//! blocks QEMU does not emulate, whose first access ends the window (decision H4).
//!
//! `docs/lockstep-differences.md` is the same table for people, with the source of every row; a
//! test holds the two equal. A difference that is not a row is a divergence.

use crate::state::Field;
use soc_esp32c3::config::QemuCompat;
use soc_esp32c3::{SocConfig, Timing};

/// How a row keeps a difference out of the verdict, in the order of preference of H3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Treatment {
    /// (i) `bits` of `field` are left out of the comparison.
    Mask { field: Field, bits: u32 },
    /// (ii) A load from an address in `first..=last` puts in its register, in inkemu, the `bits`
    /// QEMU loaded: the two model the registers there differently. An address of a block QEMU
    /// does not emulate that such a row covers does not end the window: the row names what the
    /// firmware does there and why QEMU's zeros do not matter to it.
    Inject { first: u32, last: u32, bits: u32 },
    /// (ii) A read of one of these CSRs puts in its register, in inkemu, the `bits` QEMU read.
    InjectCsr { csrs: &'static [u16], bits: u32 },
    /// (iv) The machine takes on a behaviour of QEMU (`soc_esp32c3::QemuCompat`).
    Compat(Compat),
}

/// A setting of `soc_esp32c3::QemuCompat`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compat {
    /// `QemuCompat::systimer_no_alarm_on_stopped_counter`.
    SystimerNoAlarmOnStoppedCounter,
}

impl Compat {
    /// Turns the setting on.
    pub fn apply(self, compat: &mut QemuCompat) {
        match self {
            Compat::SystimerNoAlarmOnStoppedCounter => {
                compat.systimer_no_alarm_on_stopped_counter = true;
            }
        }
    }
}

impl Treatment {
    /// The roman numeral of H3 the treatment is.
    #[must_use]
    pub const fn kind(self) -> &'static str {
        match self {
            Treatment::Mask { .. } => "i",
            Treatment::Inject { .. } | Treatment::InjectCsr { .. } => "ii",
            Treatment::Compat(_) => "iv",
        }
    }
}

/// The configuration of the machine the lockstep compares: `base` under the timing model
/// `qemu-icount` (decision H2), with every setting of `QemuCompat` a row of `rows` names.
#[must_use]
pub fn configure(base: SocConfig, rows: &[Difference]) -> SocConfig {
    let mut config = SocConfig {
        timing: Timing::QemuIcount,
        ..base
    };
    for row in rows {
        for treatment in row.treatments {
            if let Treatment::Compat(setting) = treatment {
                setting.apply(&mut config.qemu_compat);
            }
        }
    }
    config
}

/// One admitted difference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Difference {
    /// The number of the row in `docs/lockstep-differences.md`.
    pub number: u16,
    /// What differs, in a few words.
    pub summary: &'static str,
    /// How the comparison treats it: one treatment or more of the same row.
    pub treatments: &'static [Treatment],
}

/// The table. Every row is used by the tool; the tool uses no other.
pub const DIFFERENCES: &[Difference] = &[
    Difference {
        number: 1,
        summary: "mstatus.MPP reads machine mode in inkemu, holds what was written in QEMU",
        treatments: &[
            Treatment::Mask {
                field: Field::Mstatus,
                bits: MSTATUS_MPP,
            },
            Treatment::InjectCsr {
                csrs: &[MSTATUS],
                bits: MSTATUS_MPP,
            },
        ],
    },
    Difference {
        number: 2,
        summary: "mie resets to 0x0eee in QEMU's generic core, to 0 in inkemu",
        treatments: &[Treatment::Mask {
            field: Field::Mie,
            bits: 0x0000_0EEE,
        }],
    },
    Difference {
        number: 3,
        summary: "mtvec.MODE is hard-wired to vectored in inkemu, resets to 0 in QEMU",
        treatments: &[Treatment::Mask {
            field: Field::Mtvec,
            bits: 0x0000_0001,
        }],
    },
    Difference {
        number: 4,
        summary: "mip.MEIP is set by QEMU's core while a line is presented; mip is a plain register in inkemu",
        treatments: &[Treatment::Mask {
            field: Field::Mip,
            bits: 0x0000_0800,
        }],
    },
    Difference {
        number: 5,
        summary: "the performance counter counts inkemu's cycles and QEMU's own count",
        treatments: &[Treatment::InjectCsr {
            csrs: &[0x7E2, 0x802],
            bits: ALL,
        }],
    },
    Difference {
        number: 6,
        summary: "UART0: STATUS, CONF0, FSM_STATUS and CLK_CONF, which QEMU's UART does not model as the chip has them",
        treatments: &[
            registers(0x6000_001C, 0x6000_0020),
            registers(0x6000_006C, 0x6000_006C),
            registers(0x6000_0078, 0x6000_0078),
        ],
    },
    Difference {
        number: 7,
        summary: "SPI1: MISC, FLASH_WAITI_CTRL, FLASH_SUS_CTRL and CLOCK_GATE read 0 in QEMU, and W0 after the SFDP read QEMU's flash rejects",
        treatments: &[
            registers(0x6000_2034, 0x6000_2034),
            registers(0x6000_2058, 0x6000_2058),
            registers(0x6000_2098, 0x6000_209C),
            registers(0x6000_20DC, 0x6000_20DC),
        ],
    },
    Difference {
        number: 8,
        summary: "SPI0 is not a device in QEMU: its registers read 0",
        treatments: &[registers(0x6000_3000, 0x6000_33FC)],
    },
    Difference {
        number: 9,
        summary: "RTC_CNTL: QEMU models OPTIONS0, RESET_STATE (the cause alone) and STORE0-7 and reads the rest as 0",
        treatments: &[
            registers(0x6000_8000, 0x6000_804C),
            registers(0x6000_8060, 0x6000_80B4),
            registers(0x6000_80C8, 0x6000_81FC),
        ],
    },
    Difference {
        number: 10,
        summary: "eFuse: QEMU's chip has block revision v1.3 and neither MAC nor calibration; inkemu's is a production v0.3 of block v1.2",
        treatments: &[registers(0x6000_8800, 0x6000_89FC)],
    },
    Difference {
        number: 11,
        summary: "IO_MUX is not a device in QEMU: its registers read 0",
        treatments: &[registers(0x6000_9000, 0x6000_90FC)],
    },
    Difference {
        number: 12,
        summary: "the analog I2C master (regi2c) reads 0xFFFFFF in QEMU",
        treatments: &[registers(0x6000_E000, 0x6000_E0FC)],
    },
    Difference {
        number: 13,
        summary: "BB, which QEMU does not emulate, read-modify-written by rtc_sleep_pu at application start",
        treatments: &[registers(0x6001_D054, 0x6001_D054)],
    },
    Difference {
        number: 14,
        summary: "TIMG0/1: watchdog configuration, calibration and REGCLK, which QEMU resets or computes otherwise",
        treatments: &[
            registers(0x6001_F048, 0x6001_F04C),
            registers(0x6001_F068, 0x6001_F06C),
            registers(0x6001_F080, 0x6001_F080),
            registers(0x6001_F0FC, 0x6001_F0FC),
            registers(0x6002_0048, 0x6002_004C),
            registers(0x6002_00FC, 0x6002_00FC),
        ],
    },
    Difference {
        number: 15,
        summary: "SYSTIMER: VALUE_VALID always reads 1 in QEMU, and its counters count from the start of the machine",
        treatments: &[
            registers(0x6002_3004, 0x6002_3008),
            registers(0x6002_3040, 0x6002_304C),
        ],
    },
    Difference {
        number: 16,
        summary: "APB_CTRL is not a device in QEMU: WIFI_CLK_EN and the memory power registers read 0",
        treatments: &[
            registers(0x6002_6014, 0x6002_6014),
            registers(0x6002_609C, 0x6002_60AC),
        ],
    },
    Difference {
        number: 17,
        summary: "RNG.DATA: QEMU draws from the host, inkemu from a fixed seed",
        treatments: &[registers(0x6002_60B0, 0x6002_60B0)],
    },
    Difference {
        number: 18,
        summary: "APB_SARADC, which QEMU does not emulate, programmed by the bootloader's entropy source",
        treatments: &[registers(0x6004_0000, 0x6004_0054)],
    },
    Difference {
        number: 19,
        summary: "USB_DEVICE.EP1_CONF reads 0 in QEMU: the ROM waits for room that never comes, then gives up",
        treatments: &[registers(0x6004_3004, 0x6004_3004)],
    },
    Difference {
        number: 20,
        summary: "SYSTEM: QEMU reads the clock gates and resets as 0 and SYSCLK_CONF on the PLL",
        treatments: &[
            registers(0x600C_0000, 0x600C_0024),
            registers(0x600C_0040, 0x600C_0040),
            registers(0x600C_0058, 0x600C_0058),
        ],
    },
    Difference {
        number: 21,
        summary: "SENSITIVE is not a device in QEMU: its registers read 0",
        treatments: &[registers(0x600C_1000, 0x600C_1FFC)],
    },
    Difference {
        number: 22,
        summary: "EXTMEM: the cache control, power, sync and freeze registers of QEMU's cache read otherwise",
        treatments: &[
            registers(0x600C_4004, 0x600C_4008),
            registers(0x600C_4028, 0x600C_4028),
            registers(0x600C_40AC, 0x600C_40AC),
            registers(0x600C_40C4, 0x600C_40C4),
            registers(0x600C_40CC, 0x600C_40CC),
        ],
    },
    Difference {
        number: 23,
        summary: "ASSIST_DEBUG is not a device in QEMU: the stack monitor's CORE_0_MONTR_ENA reads 0",
        treatments: &[registers(0x600C_E000, 0x600C_E000)],
    },
    Difference {
        number: 24,
        summary: "QEMU's SYSTIMER compares only a running counter: no early tick when the tick is set up",
        treatments: &[Treatment::Compat(Compat::SystimerNoAlarmOnStoppedCounter)],
    },
];

/// Every bit of a register.
const ALL: u32 = u32::MAX;
/// `mstatus` and its field `MPP`, bits 12 and 11.
const MSTATUS: u16 = 0x300;
const MSTATUS_MPP: u32 = 0x0000_1800;

/// Treatment (ii) for the registers from `first` to `last`, both word addresses, every bit.
const fn registers(first: u32, last: u32) -> Treatment {
    Treatment::Inject {
        first,
        last: last + 3,
        bits: ALL,
    }
}

/// A block of the ESP32-C3 that Espressif's QEMU does not emulate at all: its registers fall in
/// QEMU's catch-all region, which reads 0 and drops writes (`info mtree -f`, oracle §1d).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unemulated {
    pub name: &'static str,
    pub first: u32,
    pub last: u32,
}

/// The blocks whose first access ends the window (H4): the function blocks oracle §1d lists as
/// missing, each its 4 KiB page of the peripheral window (the addresses of the SVD, TRM §3.3.5).
/// The catch-all region also covers blocks of the chip's own plumbing that the boot programs
/// (`IO_MUX`, `SPI0`, `APB_CTRL`, `SENSITIVE`, the analog I²C master): those are rows of
/// [`DIFFERENCES`], not window ends, because QEMU's zeros are a model of them that the boot
/// accepts, where a function block that reads 0 is a device that is absent.
pub const UNEMULATED: &[Unemulated] = &[
    Unemulated {
        name: "UHCI1",
        first: 0x6000_C000,
        last: 0x6000_CFFF,
    },
    Unemulated {
        name: "I2C0",
        first: 0x6001_3000,
        last: 0x6001_3FFF,
    },
    Unemulated {
        name: "UHCI0",
        first: 0x6001_4000,
        last: 0x6001_4FFF,
    },
    Unemulated {
        name: "RMT",
        first: 0x6001_6000,
        last: 0x6001_6FFF,
    },
    Unemulated {
        name: "LEDC",
        first: 0x6001_9000,
        last: 0x6001_9FFF,
    },
    Unemulated {
        name: "BB",
        first: 0x6001_D000,
        last: 0x6001_DFFF,
    },
    Unemulated {
        name: "SPI2",
        first: 0x6002_4000,
        last: 0x6002_4FFF,
    },
    Unemulated {
        name: "I2S0",
        first: 0x6002_D000,
        last: 0x6002_DFFF,
    },
    Unemulated {
        name: "APB_SARADC",
        first: 0x6004_0000,
        last: 0x6004_0FFF,
    },
];

/// The block an address falls in, when it is one QEMU does not emulate.
#[must_use]
pub fn unemulated(address: u32) -> Option<&'static Unemulated> {
    UNEMULATED
        .iter()
        .find(|block| (block.first..=block.last).contains(&address))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rows_are_numbered_once_each() {
        let mut numbers: Vec<u16> = DIFFERENCES.iter().map(|row| row.number).collect();
        numbers.sort_unstable();
        numbers.dedup();
        assert_eq!(numbers.len(), DIFFERENCES.len());
    }

    #[test]
    fn every_unemulated_block_holds_the_registers_the_svd_gives_it() {
        for block in UNEMULATED {
            let registers: Vec<_> = svd_regs::esp32c3::registers()
                .iter()
                .filter(|register| register.peripheral == block.name)
                .collect();
            assert!(
                !registers.is_empty(),
                "{} is not a block of the SVD",
                block.name
            );
            for register in registers {
                assert_eq!(
                    unemulated(register.addr).map(|b| b.name),
                    Some(block.name),
                    "{}.{}",
                    block.name,
                    register.name
                );
            }
        }
        assert_eq!(unemulated(0x6000_0000), None, "UART0 is emulated");
    }
}
