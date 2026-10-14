//! The admitted differences between inkemu and QEMU (decision H3), one numbered row each, and the
//! peripherals QEMU instantiates, outside which an access ends the window (decision H4) unless a
//! row names the register it reached.
//!
//! `docs/lockstep-differences.md` is the same table for people, with the source of every row; a
//! test holds the two equal, register by register and bit by bit. A difference that is not a row
//! is a divergence, and a row the window does not use, or uses outside what it declares, fails the
//! run ([`crate::Usage::check`]).

use crate::state::Field;
use soc_esp32c3::{CompatActs, QemuCompat, SocConfig, Timing};

/// How a row keeps a difference out of the verdict, in the order of preference of H3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Treatment {
    /// (i) The `bits` of a compared CSR hold `qemu` in QEMU and `inkemu` in inkemu, exactly, and
    /// every other bit is compared; a read of the CSR into a register is held to the same pair,
    /// and the register takes QEMU's bits. With `until_written`, the pair holds only until an
    /// instruction writes the CSR: it is a value both sides start from, not a behaviour.
    Pair {
        field: Field,
        bits: u32,
        qemu: u32,
        inkemu: u32,
        until_written: bool,
    },
    /// (i) `mip.MEIP`, which inkemu's controller does not show the CPU: QEMU's bit must be the line
    /// QEMU's interrupt matrix computes in the state of inkemu's controller
    /// ([`soc_esp32c3::Esp32C3::external_interrupt_as_qemu`]); QEMU may keep it up `lag`
    /// instructions after a store to `CPU_INT_THRESH` has brought that line down.
    ExternalInterrupt { lag: u8 },
    /// (ii) Registers of a peripheral the two model differently, or of one QEMU lacks: an access
    /// to one of them is admitted (it does not end the window), and a word load from one takes
    /// in inkemu QEMU's bits of `mask`, every other bit being compared.
    Registers(&'static [Register]),
    /// (ii) A counter QEMU and inkemu read apart by at most `bound` counts, a residue of the two
    /// models of time: a read takes QEMU's value, a wider gap is a divergence.
    Counter { location: Location, bound: u32 },
    /// (iv) The machine takes on a behaviour of QEMU (`soc_esp32c3::QemuCompat`); the row is used
    /// as many times as the setting acted.
    Compat(Compat),
}

/// A register of a row of treatment (ii).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Register {
    /// Its word address.
    pub address: u32,
    /// The bits a word load takes from QEMU; 0 admits the access and copies nothing.
    pub mask: u32,
    /// The value QEMU must have read for the copy to be made, when QEMU's model of the register
    /// is a constant.
    pub qemu: Option<u32>,
    /// The copy is made only after an SPI1 user command with this opcode (`USR_COMMAND_VALUE`
    /// at the store of `CMD.USR`): the register holds that command's answer.
    pub after_command: Option<u8>,
}

/// What a counter row reads: a peripheral register, or a CSR.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Location {
    Mmio(u32),
    Csr(u16),
}

impl std::fmt::Display for Location {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Location::Mmio(address) => write!(f, "{address:#010x}"),
            Location::Csr(csr) => write!(f, "CSR {csr:#05x}"),
        }
    }
}

/// A setting of `soc_esp32c3::QemuCompat`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compat {
    /// `QemuCompat::systimer_no_alarm_on_stopped_counter`.
    SystimerNoAlarmOnStoppedCounter,
    /// `QemuCompat::systimer_counts_as_qemu`.
    SystimerCountsAsQemu,
    /// `QemuCompat::cycle_counter_as_qemu`.
    CycleCounterAsQemu,
}

impl Compat {
    /// Turns the setting on.
    pub fn apply(self, compat: &mut QemuCompat) {
        match self {
            Compat::SystimerNoAlarmOnStoppedCounter => {
                compat.systimer_no_alarm_on_stopped_counter = true;
            }
            Compat::SystimerCountsAsQemu => compat.systimer_counts_as_qemu = true,
            Compat::CycleCounterAsQemu => compat.cycle_counter_as_qemu = true,
        }
    }

    /// How many times the setting acted, as the machine counted it.
    #[must_use]
    pub const fn acts(self, acts: &CompatActs) -> u64 {
        match self {
            Compat::SystimerNoAlarmOnStoppedCounter => acts.systimer_alarms_withheld,
            Compat::SystimerCountsAsQemu => {
                acts.systimer_resets_ignored + acts.systimer_values_as_qemu
            }
            Compat::CycleCounterAsQemu => acts.cycle_counter_reads,
        }
    }
}

impl Treatment {
    /// The roman numeral of H3 the treatment is.
    #[must_use]
    pub const fn kind(self) -> &'static str {
        match self {
            Treatment::Pair { .. } | Treatment::ExternalInterrupt { .. } => "i",
            Treatment::Registers(_) | Treatment::Counter { .. } => "ii",
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
    /// Its number in `docs/lockstep-differences.md`.
    pub number: u16,
    pub summary: &'static str,
    pub treatments: &'static [Treatment],
}

impl Difference {
    /// The registers of the row at `address`, every condition included.
    pub fn registers_at(&self, address: u32) -> impl Iterator<Item = &'static Register> + '_ {
        self.treatments
            .iter()
            .filter_map(|treatment| match treatment {
                Treatment::Registers(registers) => Some(registers.iter()),
                _ => None,
            })
            .flatten()
            .filter(move |register| register.address == address)
    }

    /// The bits the row declares it may take from QEMU at `location`, `None` when it declares
    /// nothing there. A counter declares every bit; a register admitted with no bit, 0.
    #[must_use]
    pub fn declared(&self, location: Location) -> Option<u32> {
        let mut declared = None;
        for treatment in self.treatments {
            let bits = match (*treatment, location) {
                (Treatment::Pair { field, bits, .. }, Location::Csr(csr))
                    if field.csr() == Some(csr) =>
                {
                    Some(bits)
                }
                (Treatment::ExternalInterrupt { .. }, Location::Csr(MIP)) => Some(MEIP),
                (Treatment::Counter { location: l, .. }, _) if l == location => Some(u32::MAX),
                (Treatment::Registers(registers), Location::Mmio(address)) => registers
                    .iter()
                    .filter(|register| register.address == address)
                    .map(|register| register.mask)
                    .reduce(|a, b| a | b),
                _ => None,
            };
            if let Some(bits) = bits {
                declared = Some(declared.unwrap_or(0) | bits);
            }
        }
        declared
    }

    /// The largest gap the row admits at `location`, for a counter.
    #[must_use]
    pub fn bound(&self, location: Location) -> Option<u32> {
        self.treatments
            .iter()
            .find_map(|treatment| match *treatment {
                Treatment::Counter { location: l, bound } if l == location => Some(bound),
                _ => None,
            })
    }
}

/// `mip` and its bit `MEIP`.
pub const MIP: u16 = 0x344;
pub const MEIP: u32 = 1 << 11;
/// `INTERRUPT_CORE0.CPU_INT_THRESH` (the SVD; a test checks it).
pub const CPU_INT_THRESH: u32 = 0x600C_2194;
/// `SPI1.CMD`, its bit `USR` that starts a user command, and `SPI1.USER2`, whose bits 15..0 are
/// `USR_COMMAND_VALUE` (the SVD; a test checks them).
pub const SPI1_CMD: u32 = 0x6000_2000;
pub const SPI1_CMD_USR: u32 = 1 << 18;
pub const SPI1_USER2: u32 = 0x6000_2020;

/// A register admitted with `mask` taken from QEMU at a word load.
const fn copy(address: u32, mask: u32) -> Register {
    Register {
        address,
        mask,
        qemu: None,
        after_command: None,
    }
}

/// A register of a peripheral QEMU lacks whose value both read alike: the access is admitted and
/// nothing is copied.
const fn admit(address: u32) -> Register {
    copy(address, 0)
}

/// The table, in the order of `docs/lockstep-differences.md`. Every mask is the bits measured to
/// differ in the window of `flash_hello.bin` (the document gives the measure and the source of
/// each); a firmware that shows another bit, or reaches another register, fails the run until a
/// row admits it with its evidence.
pub const DIFFERENCES: &[Difference] = &[
    Difference {
        number: 1,
        summary: "mstatus.MPP: QEMU 0 throughout, inkemu wires it to machine mode",
        treatments: &[Treatment::Pair {
            field: Field::Mstatus,
            bits: 0x0000_1800,
            qemu: 0,
            inkemu: 0x0000_1800,
            until_written: false,
        }],
    },
    Difference {
        number: 2,
        summary: "mie at power-on: QEMU 0x0EEE, inkemu 0, until mie is written",
        treatments: &[Treatment::Pair {
            field: Field::Mie,
            bits: 0x0000_0EEE,
            qemu: 0x0000_0EEE,
            inkemu: 0,
            until_written: true,
        }],
    },
    Difference {
        number: 3,
        summary: "mtvec.MODE at power-on: QEMU direct, inkemu vectored, until mtvec is written",
        treatments: &[Treatment::Pair {
            field: Field::Mtvec,
            bits: 0x0000_0001,
            qemu: 0,
            inkemu: 1,
            until_written: true,
        }],
    },
    Difference {
        number: 4,
        summary: "mip.MEIP: QEMU's interrupt-matrix line, three instructions late after a threshold store",
        treatments: &[Treatment::ExternalInterrupt { lag: 3 }],
    },
    Difference {
        number: 5,
        summary: "cycle counter: QEMU's counts 6.25 ns of its clock from the first read",
        treatments: &[
            Treatment::Compat(Compat::CycleCounterAsQemu),
            Treatment::Counter {
                location: Location::Csr(0x7E2),
                bound: 1,
            },
            Treatment::Counter {
                location: Location::Csr(0x802),
                bound: 1,
            },
        ],
    },
    Difference {
        number: 6,
        summary: "UART0: line levels, FIFO count and reset values QEMU does not model",
        treatments: &[Treatment::Registers(&[
            copy(0x6000_001C, 0xE0FF_C000),
            copy(0x6000_0020, 0x1000_001C),
            copy(0x6000_006C, 0x0000_0010),
            copy(0x6000_0078, 0x1672_2500),
        ])],
    },
    Difference {
        number: 7,
        summary: "SPI1: QEMU's flash does not answer SFDP, leaves W0 stale, and lacks reset values",
        treatments: &[Treatment::Registers(&[
            copy(0x6000_2034, 0x0000_0002),
            Register {
                address: 0x6000_2058,
                mask: 0x0018_60FF,
                qemu: None,
                after_command: Some(0x5A),
            },
            Register {
                address: 0x6000_2058,
                mask: 0x0018_6000,
                qemu: None,
                after_command: Some(0x05),
            },
            copy(0x6000_2098, 0x0000_0014),
            copy(0x6000_209C, 0x0A00_2000),
            copy(0x6000_20DC, 0x0000_0001),
        ])],
    },
    Difference {
        number: 8,
        summary: "SPI0, absent in QEMU: the cache's flash configuration",
        treatments: &[Treatment::Registers(&[
            copy(0x6000_3008, 0x00AC_2008),
            copy(0x6000_3010, 0x0000_0061),
            admit(0x6000_3014),
            copy(0x6000_3018, 0x7000_00C0),
            copy(0x6000_301C, 0x5C00_0007),
            copy(0x6000_3020, 0x7000_0003),
            admit(0x6000_3024),
            admit(0x6000_3028),
            admit(0x6000_3034),
            copy(0x6000_303C, 0x0000_0001),
            copy(0x6000_3054, 0x0000_0200),
            copy(0x6000_30DC, 0x0000_0001),
            admit(0x6000_30E0),
        ])],
    },
    Difference {
        number: 9,
        summary: "RTC_CNTL: QEMU models OPTIONS0, RESET_STATE and STORE0-7 only",
        treatments: &[Treatment::Registers(&[
            copy(0x6000_8000, 0x1C00_A000),
            Register {
                address: 0x6000_8010,
                mask: u32::MAX,
                qemu: Some(0),
                after_command: None,
            },
            copy(0x6000_801C, 0x3C14_0543),
            copy(0x6000_8024, 0x0A08_0A09),
            copy(0x6000_8028, 0x1220_0A09),
            copy(0x6000_802C, 0x0000_8000),
            copy(0x6000_8030, 0x0A08_0000),
            copy(0x6000_8034, 0x00D4_0000),
            copy(0x6000_8038, 0x0000_3000),
            copy(0x6000_8040, 0x0000_0200),
            copy(0x6000_8070, 0x31D8_3298),
            copy(0x6000_8074, 0x0040_0000),
            copy(0x6000_8080, 0xA03F_C000),
            copy(0x6000_8088, 0x0057_5010),
            copy(0x6000_808C, 0xBA80_5480),
            copy(0x6000_8090, 0xF807_FA14),
            copy(0x6000_80AC, 0x04B0_0000),
            copy(0x6000_80D8, 0x7BFF_C030),
            copy(0x6000_810C, 0x0000_0007),
            copy(0x6000_811C, 0xC000_0000),
        ])],
    },
    Difference {
        number: 10,
        summary: "eFuse: the words where QEMU's blank eFuses and the X4's differ",
        treatments: &[Treatment::Registers(&[
            copy(0x6000_883C, 0x8000_0000),
            copy(0x6000_8848, 0x0000_58CF),
            copy(0x6000_8850, 0x0100_0000),
            copy(0x6000_8858, 0x0047_D2F2),
            copy(0x6000_886C, 0xD864_EA40),
        ])],
    },
    Difference {
        number: 11,
        summary: "IO_MUX, absent in QEMU: the pad configuration of GPIO0-21",
        treatments: &[Treatment::Registers(&[
            copy(0x6000_9004, 0x0000_0B00),
            copy(0x6000_9008, 0x0000_0B00),
            copy(0x6000_900C, 0x0000_0B00),
            copy(0x6000_9010, 0x0000_0B00),
            copy(0x6000_9014, 0x0000_0B00),
            copy(0x6000_9018, 0x0000_0B00),
            copy(0x6000_901C, 0x0000_0B00),
            copy(0x6000_9020, 0x0000_0B00),
            copy(0x6000_9024, 0x0000_0B00),
            copy(0x6000_9028, 0x0000_0B00),
            copy(0x6000_902C, 0x0000_0B00),
            copy(0x6000_9030, 0x0000_0B00),
            copy(0x6000_9034, 0x0000_1F00),
            copy(0x6000_9038, 0x0000_1F00),
            copy(0x6000_903C, 0x0000_0F00),
            copy(0x6000_9040, 0x0000_0F00),
            copy(0x6000_9044, 0x0000_0F00),
            copy(0x6000_9048, 0x0000_0F00),
            copy(0x6000_904C, 0x0000_0B00),
            copy(0x6000_9050, 0x0000_0B00),
            copy(0x6000_9054, 0x0000_0B00),
            copy(0x6000_9058, 0x0000_0B00),
        ])],
    },
    Difference {
        number: 12,
        summary: "analog I2C master, absent in QEMU: the ROM's regi2c accesses",
        treatments: &[Treatment::Registers(&[
            copy(0x6000_E000, 0x05FF_FF9E),
            copy(0x6000_E044, 0x00FF_FFFF),
            copy(0x6000_E048, 0x00FF_FFFF),
        ])],
    },
    Difference {
        number: 13,
        summary: "radio power-up of rtc_sleep_pu, absent in QEMU: BB, NRX, FE and FE2",
        treatments: &[Treatment::Registers(&[
            admit(0x6000_50F0),
            admit(0x6000_6090),
            admit(0x6001_CCD4),
            admit(0x6001_D054),
        ])],
    },
    Difference {
        number: 14,
        summary: "TIMG0 and TIMG1: watchdog reset values, clock gates and the RTC calibration",
        treatments: &[Treatment::Registers(&[
            copy(0x6001_F048, 0x0004_C000),
            copy(0x6001_F04C, 0x0001_0000),
            copy(0x6001_F068, 0x0000_8000),
            copy(0x6001_F06C, 0x024C_3C00),
            copy(0x6001_F080, 0xFFFF_FF99),
            copy(0x6001_F0FC, 0x6000_0000),
            copy(0x6002_0048, 0x0004_C000),
            copy(0x6002_004C, 0x0001_0000),
            copy(0x6002_00FC, 0x6000_0000),
        ])],
    },
    Difference {
        number: 15,
        summary: "SYSTIMER: QEMU's counters, truncated at each update and never reset by SYSTEM",
        treatments: &[
            Treatment::Compat(Compat::SystimerCountsAsQemu),
            Treatment::Counter {
                location: Location::Mmio(0x6002_3044),
                bound: 2,
            },
        ],
    },
    Difference {
        number: 16,
        summary: "APB_CTRL, absent in QEMU: Wi-Fi clocks and memory power",
        treatments: &[Treatment::Registers(&[
            copy(0x6002_6014, 0xFFFF_FFFF),
            admit(0x6002_6018),
            copy(0x6002_609C, 0x0000_0015),
            copy(0x6002_60AC, 0x0000_003F),
        ])],
    },
    Difference {
        number: 17,
        summary: "RNG: QEMU returns the host's rand()",
        treatments: &[Treatment::Registers(&[copy(0x6002_60B0, u32::MAX)])],
    },
    Difference {
        number: 18,
        summary: "APB_SARADC, absent in QEMU: the seven registers of the bootloader's entropy source",
        treatments: &[Treatment::Registers(&[
            copy(0x6004_0000, 0x5883_82C0),
            copy(0x6004_0004, 0x0106_E1FE),
            copy(0x6004_000C, 0x00FF_080D),
            copy(0x6004_0018, 0x009C_FFFF),
            copy(0x6004_001C, 0x00FF_FFFF),
            copy(0x6004_0050, 0x8000_00FF),
            copy(0x6004_0054, 0x0050_000F),
        ])],
    },
    Difference {
        number: 19,
        summary: "USB-Serial-JTAG: SERIAL_IN_EP_DATA_FREE, which QEMU reads 0",
        treatments: &[Treatment::Registers(&[copy(0x6004_3004, 0x0000_0002)])],
    },
    Difference {
        number: 20,
        summary: "SYSTEM: clock gates, resets and clock selection QEMU's clock model keeps otherwise",
        treatments: &[Treatment::Registers(&[
            copy(0x600C_0000, 0x0000_0040),
            copy(0x600C_0004, 0x0000_00C0),
            copy(0x600C_0008, 0x0000_000C),
            copy(0x600C_000C, 0x0000_0001),
            copy(0x600C_0010, 0xF9C1_E06F),
            copy(0x600C_0014, 0x0000_0204),
            copy(0x600C_0018, 0xA86D_BBEE),
            copy(0x600C_001C, 0x0000_01FE),
            copy(0x600C_0020, 0x0000_00FF),
            copy(0x600C_0024, 0x0200_1001),
            copy(0x600C_0040, 0x0000_0007),
            copy(0x600C_0058, 0x0008_0401),
        ])],
    },
    Difference {
        number: 21,
        summary: "SENSITIVE, absent in QEMU: the memory protection ESP-IDF sets up",
        treatments: &[Treatment::Registers(&[
            copy(0x600C_101C, 0x0000_000F),
            admit(0x600C_1090),
            admit(0x600C_1094),
            admit(0x600C_1098),
            admit(0x600C_109C),
            admit(0x600C_10A0),
            admit(0x600C_10A4),
            admit(0x600C_10A8),
            copy(0x600C_10B0, 0x001C_7FFF),
            admit(0x600C_10B4),
            copy(0x600C_10B8, 0x0000_0003),
            admit(0x600C_10C0),
            copy(0x600C_10C4, 0x0F0F_F0FF),
            admit(0x600C_10C8),
            copy(0x600C_10CC, 0x0000_0003),
            admit(0x600C_10D8),
            copy(0x600C_10FC, 0x003F_FFFF),
            copy(0x600C_1100, 0x0000_0FFF),
            admit(0x600C_1130),
            copy(0x600C_1134, 0x0000_0003),
        ])],
    },
    Difference {
        number: 22,
        summary: "EXTMEM: power, freeze and sync bits QEMU's cache model does not keep",
        treatments: &[Treatment::Registers(&[
            copy(0x600C_4004, 0x0000_0003),
            copy(0x600C_4008, 0x0000_0005),
            copy(0x600C_4028, 0x0000_0002),
            copy(0x600C_40AC, 0x0000_0005),
            copy(0x600C_40C4, 0x0000_000F),
            copy(0x600C_40CC, 0x0000_0001),
        ])],
    },
    Difference {
        number: 23,
        summary: "ASSIST_DEBUG, absent in QEMU: the stack monitor of FreeRTOS",
        treatments: &[Treatment::Registers(&[
            copy(0x600C_E000, 0x0000_0300),
            admit(0x600C_E008),
            admit(0x600C_E038),
            admit(0x600C_E03C),
            admit(0x600C_E044),
            admit(0x600C_E048),
            admit(0x600C_E098),
        ])],
    },
    Difference {
        number: 24,
        summary: "SYSTIMER: no alarm while the counter is stopped",
        treatments: &[Treatment::Compat(Compat::SystimerNoAlarmOnStoppedCounter)],
    },
];

/// A peripheral QEMU instantiates, by its first and last byte: the flattened memory tree of
/// QEMU in the pinned image (`info mtree -f`, measured on 2026-10-14 with `flash_hello.bin`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QemuDevice {
    pub name: &'static str,
    pub first: u32,
    pub last: u32,
}

const fn device(name: &'static str, first: u32, last: u32) -> QemuDevice {
    QemuDevice { name, first, last }
}

/// QEMU's peripherals. The rest of [`PERIPHERALS`] is QEMU's catch-all `esp32c3.iomem`, which
/// reads 0 and drops writes but for two registers: the RNG, a row of the table, and
/// `SYSCON_ORIGIN` (`0x600263F8`), which reads "QEMU" and ends the window like any other.
pub const QEMU_DEVICES: &[QemuDevice] = &[
    device("UART0", 0x6000_0000, 0x6000_007B),
    device("SPI1", 0x6000_2000, 0x6000_23FF),
    device("GPIO", 0x6000_4000, 0x6000_4FFF),
    device("RTC_CNTL", 0x6000_8000, 0x6000_81FF),
    device("EFUSE", 0x6000_8800, 0x6000_8A03),
    device("UART1", 0x6001_0000, 0x6001_007B),
    device("TIMG0", 0x6001_F000, 0x6001_F0FF),
    device("TIMG1", 0x6002_0000, 0x6002_00FF),
    device("SYSTIMER", 0x6002_3000, 0x6002_30FF),
    device("TWAI", 0x6002_B000, 0x6002_B1FF),
    device("AES", 0x6003_A000, 0x6003_A0BB),
    device("SHA", 0x6003_B000, 0x6003_B0FF),
    device("RSA", 0x6003_C000, 0x6003_C833),
    device("DS", 0x6003_D000, 0x6003_DE23),
    device("HMAC", 0x6003_E000, 0x6003_E0FE),
    device("DMA", 0x6003_F000, 0x6003_F283),
    device("USB_DEVICE", 0x6004_3000, 0x6004_3083),
    device("SYSTEM", 0x600C_0000, 0x600C_009B),
    device("INTERRUPT_CORE0", 0x600C_2000, 0x600C_27FF),
    device("EXTMEM", 0x600C_4000, 0x600C_51FF),
    device("XTS_AES", 0x600C_C000, 0x600C_C05F),
    device("open_eth", 0x600C_D000, 0x600C_D053),
    device("open_eth", 0x600C_D400, 0x600C_D7FF),
];

/// The peripheral window of the ESP32-C3 (TRM §3.3.5): an access in it that falls in no
/// [`QemuDevice`] reaches a register QEMU does not have.
pub const PERIPHERALS: (u32, u32) = (0x6000_0000, 0x600F_FFFF);

/// Whether an access to `address` reaches a register QEMU does not have.
#[must_use]
pub fn absent_in_qemu(address: u32) -> bool {
    (PERIPHERALS.0..=PERIPHERALS.1).contains(&address)
        && !QEMU_DEVICES
            .iter()
            .any(|device| (device.first..=device.last).contains(&address))
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
    fn the_addresses_named_here_are_those_of_the_svd() {
        use svd_regs::esp32c3::addr;
        assert_eq!(CPU_INT_THRESH, addr("INTERRUPT_CORE0", "CPU_INT_THRESH"));
        assert_eq!(SPI1_CMD, addr("SPI1", "CMD"));
        assert_eq!(SPI1_USER2, addr("SPI1", "USER2"));
    }

    #[test]
    fn a_register_listed_twice_differs_by_its_condition() {
        for row in DIFFERENCES {
            let mut seen = std::collections::BTreeSet::new();
            for treatment in row.treatments {
                if let Treatment::Registers(registers) = treatment {
                    for register in *registers {
                        assert!(
                            seen.insert((register.address, register.after_command)),
                            "row {}: {:#010x}",
                            row.number,
                            register.address
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn every_register_is_word_aligned_and_in_one_row() {
        let mut owner = std::collections::BTreeMap::new();
        for row in DIFFERENCES {
            for treatment in row.treatments {
                if let Treatment::Registers(registers) = treatment {
                    for register in *registers {
                        assert_eq!(register.address % 4, 0, "{:#010x}", register.address);
                        let first = *owner.entry(register.address).or_insert(row.number);
                        assert_eq!(first, row.number, "{:#010x}", register.address);
                    }
                }
            }
        }
    }

    #[test]
    fn qemu_has_uart0_and_spi1_but_not_spi0_nor_the_radio() {
        assert!(!absent_in_qemu(0x6000_0000));
        assert!(!absent_in_qemu(0x6000_2058));
        assert!(absent_in_qemu(0x6000_3008));
        assert!(absent_in_qemu(0x6000_6090));
        assert!(absent_in_qemu(0x6000_8200), "past QEMU's RTC_CNTL");
        assert!(!absent_in_qemu(0x3FC8_0000), "memory is not a peripheral");
        assert!(absent_in_qemu(0x6004_002C), "APB_SARADC.SAR1DATA_STATUS");
    }

    #[test]
    fn the_devices_are_ordered_and_disjoint() {
        for pair in QEMU_DEVICES.windows(2) {
            assert!(pair[0].last < pair[1].first, "{pair:?}");
        }
    }
}
