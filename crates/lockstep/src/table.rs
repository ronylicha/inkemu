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
    /// (i) What a reset of both sides leaves different in the compared state: inkemu's CPU takes
    /// its reset values (`cpu_rv32::Hart::reset`), and QEMU leaves these quantities as they were
    /// before the reset. Each [`Kept`] is held, in its bits, to exactly those two values,
    /// QEMU's from before the reset and inkemu's from its reset, which must be the reset value
    /// the row declares, until an instruction writes it or a trap changes it; an instruction
    /// that reads one while it is held is a divergence, since the two sides would compute from
    /// different values.
    KeptAtReset(&'static [Kept]),
    /// (ii) CSRs QEMU does not print, some bits of which its reset keeps where inkemu's CPU reset
    /// clears them: after a reset of both sides, a read into a register gives in QEMU inkemu's
    /// value with those of the kept bits inkemu held before the reset set again ([`KeptCsr`]),
    /// or inkemu's value itself; the register takes QEMU's bits, within the mask. A bit an
    /// instruction has written since the reset is no longer kept (every bit for `csrrw` and
    /// `csrrwi`, the operand for `csrrs`, `csrrc` and their immediate forms): it is compared.
    CsrKeptAtReset(&'static [KeptCsr]),
    /// (ii) Registers the two resets leave in different states: the same copy as
    /// [`Treatment::Registers`], made only after a reset that left the register in two states
    /// (a reset of both sides, or one of its module the machine made at a pulse QEMU ignores) and
    /// until the first store to it since, on top of what the entries of `Registers` at the same
    /// address take. The copy is made only when the side that reset shows the value the entry
    /// declares ([`AfterReset`]); any other value is a divergence.
    RegistersAfterReset(&'static [AfterReset]),
    /// (ii) Registers of a peripheral the two model differently, or of one QEMU lacks: an access
    /// to one of them is admitted (it does not end the window), and a word load from one takes
    /// in inkemu QEMU's bits of `mask`, every other bit being compared.
    Registers(&'static [Register]),
    /// (ii) A counter both sides read, each its own model's, from origins that differ: the gap
    /// between them (QEMU's value minus inkemu's) may only move as `drift` says, a residue of the
    /// two models of time; a read within it takes QEMU's value, a wider move is a divergence. The
    /// first read after any of `origins` (a CSR write to it, or a word store to that address),
    /// and the first read of all, fix the gap the next reads are held to.
    Counter {
        location: Location,
        drift: Drift,
        origins: &'static [Location],
    },
    /// (ii) A register of the RTC timer, which QEMU does not model and reads `qemu`: inkemu's
    /// value must be the slow-clock cycles of `hz` at the device time of the last store of bit
    /// `trigger` to `capture`, within `bound`; a read within it takes QEMU's value.
    SlowClock {
        register: u32,
        capture: u32,
        trigger: u32,
        hz: u64,
        bound: u32,
        qemu: u32,
    },
    /// (iv) The machine takes on a behaviour of QEMU (`soc_esp32c3::QemuCompat`); the row is used
    /// as many times as the setting acted.
    Compat(Compat),
}

/// How far the gap of a [`Treatment::Counter`] may move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drift {
    /// Within `n` counts of the gap at the origin, either way.
    Within(u32),
    /// From one read to the next, down by at most one count for every update QEMU may have made
    /// of its counter in between, and never up: QEMU drops what is left of a count at every
    /// update, which follows a store to the block `first..=last` (a flush, a load, a reprogram)
    /// or an alarm (an interrupt taken).
    Truncated { first: u32, last: u32 },
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

/// A register of [`Treatment::RegistersAfterReset`], with the values the side that reset it must
/// show, within the mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AfterReset {
    pub register: Register,
    /// inkemu's value when the machine reset the register's module since the last store to it
    /// (`esp_restart`'s pulses, or the firmware's): its reset value in the SVD.
    pub inkemu: Option<u32>,
    /// QEMU's value otherwise, when the reset of both sides left inkemu's register as the life
    /// before set it: the value QEMU's reset gives it, as measured.
    pub qemu: Option<u32>,
}

/// An entry of [`Treatment::RegistersAfterReset`] at `address`, copying `mask`.
const fn after_reset(
    address: u32,
    mask: u32,
    inkemu: Option<u32>,
    qemu: Option<u32>,
) -> AfterReset {
    AfterReset {
        register: copy(address, mask),
        inkemu,
        qemu,
    }
}

/// A quantity of [`Treatment::KeptAtReset`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kept {
    pub field: Field,
    /// The bits held: those measured to differ after the reset; every other bit is compared.
    pub bits: u32,
    /// inkemu's value after a reset (`cpu_rv32::Hart::reset`): a hold starts only when inkemu
    /// shows it, so that a reset of inkemu that leaves another value is a divergence.
    pub reset: u32,
}

/// A quantity held whole, whose reset value in inkemu is `reset`.
#[must_use]
pub const fn kept(field: Field, reset: u32) -> Kept {
    Kept {
        field,
        bits: u32::MAX,
        reset,
    }
}

/// A row number no longer in use, kept so that a number always names the same difference
/// (`docs/lockstep-differences.md`, where its section says why it went).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retired {
    pub number: u16,
    pub summary: &'static str,
}

/// The retired rows.
pub const RETIRED: &[Retired] = &[Retired {
    number: 1,
    summary: "mstatus.MPP: retired on 2026-10-14, inkemu's MPP is the chip's (TRM register 1.5)",
}];

/// A CSR of [`Treatment::CsrKeptAtReset`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeptCsr {
    pub csr: u16,
    /// The bits QEMU's reset keeps.
    pub kept: u32,
    /// The bits a read may take from QEMU: those measured to differ.
    pub mask: u32,
}

/// Where a row acts: a peripheral register, a CSR, or an integer register.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Location {
    Mmio(u32),
    Csr(u16),
    /// `x1` to `x31`.
    Integer(u8),
}

impl Location {
    /// Where a compared quantity is; `None` for the PC.
    #[must_use]
    pub fn of(field: Field) -> Option<Location> {
        match field {
            Field::Pc => None,
            Field::X(reg) => Some(Location::Integer(reg)),
            csr => csr.csr().map(Location::Csr),
        }
    }
}

impl std::fmt::Display for Location {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Location::Mmio(address) => write!(f, "{address:#010x}"),
            Location::Csr(csr) => write!(f, "CSR {csr:#05x}"),
            Location::Integer(reg) => write!(f, "x{reg}"),
        }
    }
}

/// A setting of `soc_esp32c3::QemuCompat`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compat {
    /// `QemuCompat::systimer_no_alarm_on_stopped_counter`.
    SystimerNoAlarmOnStoppedCounter,
    /// `QemuCompat::cycle_counter_on_time_base`.
    CycleCounterOnTimeBase,
}

impl Compat {
    /// Turns the setting on.
    pub fn apply(self, compat: &mut QemuCompat) {
        match self {
            Compat::SystimerNoAlarmOnStoppedCounter => {
                compat.systimer_no_alarm_on_stopped_counter = true;
            }
            Compat::CycleCounterOnTimeBase => compat.cycle_counter_on_time_base = true,
        }
    }

    /// How many times the setting acted, as the machine counted it.
    #[must_use]
    pub const fn acts(self, acts: &CompatActs) -> u64 {
        match self {
            Compat::SystimerNoAlarmOnStoppedCounter => acts.systimer_alarms_withheld,
            Compat::CycleCounterOnTimeBase => acts.cycle_counter_reads,
        }
    }
}

impl Treatment {
    /// The registers a treatment of registers names, after a reset or not; none for another.
    pub fn registers(&self) -> Box<dyn Iterator<Item = &'static Register>> {
        match *self {
            Treatment::Registers(registers) => Box::new(registers.iter()),
            Treatment::RegistersAfterReset(entries) => {
                Box::new(entries.iter().map(|entry| &entry.register))
            }
            _ => Box::new(std::iter::empty()),
        }
    }

    /// The roman numeral of H3 the treatment is.
    #[must_use]
    pub const fn kind(self) -> &'static str {
        match self {
            Treatment::Pair { .. }
            | Treatment::ExternalInterrupt { .. }
            | Treatment::KeptAtReset(_) => "i",
            Treatment::Registers(_)
            | Treatment::RegistersAfterReset(_)
            | Treatment::CsrKeptAtReset(_)
            | Treatment::Counter { .. }
            | Treatment::SlowClock { .. } => "ii",
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
    /// The registers of the row at `address`, every condition included, those of
    /// [`Treatment::RegistersAfterReset`] when `after_reset` (a reset of both sides was crossed).
    pub fn registers_at(
        &self,
        address: u32,
        after_reset: bool,
    ) -> impl Iterator<Item = &'static Register> + '_ {
        self.treatments
            .iter()
            .flat_map(
                move |treatment| -> Box<dyn Iterator<Item = &'static Register>> {
                    match *treatment {
                        Treatment::Registers(registers) => Box::new(registers.iter()),
                        Treatment::RegistersAfterReset(entries) if after_reset => {
                            Box::new(entries.iter().map(|entry| &entry.register))
                        }
                        _ => Box::new(std::iter::empty()),
                    }
                },
            )
            .filter(move |register| register.address == address)
    }

    /// The entries of [`Treatment::RegistersAfterReset`] of the row at `address`.
    pub fn after_reset_at(&self, address: u32) -> impl Iterator<Item = &'static AfterReset> + '_ {
        self.treatments
            .iter()
            .flat_map(|treatment| match *treatment {
                Treatment::RegistersAfterReset(entries) => entries,
                _ => &[],
            })
            .filter(move |entry| entry.register.address == address)
    }

    /// Every register entry of the row, before a reset and after one.
    pub fn registers_at_all(&self) -> impl Iterator<Item = &'static Register> + '_ {
        self.treatments.iter().flat_map(Treatment::registers)
    }

    /// Every site the row declares, with the bits a run must take from QEMU there for the
    /// declaration to be used whole (decision H3): a pair's bits, `MEIP`, a register's mask. A
    /// register admitted with no bit, a counter and a clock are used by a read.
    pub fn sites(&self) -> impl Iterator<Item = (Site, u32)> + '_ {
        self.treatments
            .iter()
            .flat_map(|treatment| -> Box<dyn Iterator<Item = (Site, u32)>> {
                match *treatment {
                    Treatment::Pair { field, bits, .. } => Box::new(
                        field
                            .csr()
                            .map(|csr| (Site::at(Location::Csr(csr)), bits))
                            .into_iter(),
                    ),
                    Treatment::ExternalInterrupt { .. } => {
                        Box::new(std::iter::once((Site::at(Location::Csr(MIP)), MEIP)))
                    }
                    Treatment::KeptAtReset(kept) => Box::new(
                        kept.iter()
                            .filter_map(|kept| Location::of(kept.field))
                            .map(|location| (Site::at(location), 0)),
                    ),
                    Treatment::Registers(_) | Treatment::RegistersAfterReset(_) => {
                        Box::new(treatment.registers().map(|register| {
                            (
                                Site {
                                    location: Location::Mmio(register.address),
                                    after_command: register.after_command,
                                },
                                register.mask,
                            )
                        }))
                    }
                    Treatment::CsrKeptAtReset(csrs) => Box::new(
                        csrs.iter()
                            .map(|kept| (Site::at(Location::Csr(kept.csr)), kept.mask)),
                    ),
                    Treatment::Counter { location, .. } => {
                        Box::new(std::iter::once((Site::at(location), 0)))
                    }
                    Treatment::SlowClock { register, .. } => {
                        Box::new(std::iter::once((Site::at(Location::Mmio(register)), 0)))
                    }
                    Treatment::Compat(_) => Box::new(std::iter::empty()),
                }
            })
    }

    /// The bits the row allows a run to take from QEMU at `site`, `None` when it declares nothing
    /// there. A counter and a clock allow every bit; a register admitted with no bit, none.
    #[must_use]
    pub fn declared(&self, site: Site) -> Option<u32> {
        let allowed = |treatment: &Treatment| match *treatment {
            Treatment::Counter { location, .. } => location == site.location,
            Treatment::SlowClock { register, .. } => Location::Mmio(register) == site.location,
            _ => false,
        };
        if self.treatments.iter().any(allowed) {
            return Some(u32::MAX);
        }
        self.sites()
            .filter(|(declared, _)| *declared == site)
            .map(|(_, bits)| bits)
            .reduce(|a, b| a | b)
    }

    /// The largest move of the gap the row admits at `location`, for a counter held within a
    /// constant of its origin.
    #[must_use]
    pub fn bound(&self, location: Location) -> Option<u32> {
        self.treatments
            .iter()
            .find_map(|treatment| match *treatment {
                Treatment::Counter {
                    location: l,
                    drift: Drift::Within(bound),
                    ..
                } if l == location => Some(bound),
                Treatment::SlowClock {
                    register, bound, ..
                } if Location::Mmio(register) == location => Some(bound),
                _ => None,
            })
    }
}

/// A place a row acts at: a location and, for a register entry, the SPI1 command it waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Site {
    pub location: Location,
    pub after_command: Option<u8>,
}

impl Site {
    /// A site without a condition.
    #[must_use]
    pub const fn at(location: Location) -> Site {
        Site {
            location,
            after_command: None,
        }
    }
}

impl std::fmt::Display for Site {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.location)?;
        if let Some(command) = self.after_command {
            write!(f, " after {command:#04x}")?;
        }
        Ok(())
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
/// A module the machine resets when a store sets its bit of `SYSTEM.PERIP_RST_EN0/1`, and QEMU,
/// whose `SYSTEM` reads 0 there and ignores the store (`hw/riscv/esp32c3_clk.c`,
/// `esp32c3_clock_read`/`esp32c3_clock_write`, default case), does not: `register` and `bit`, and
/// the registers it resets, `first..=last`. These are the modules of `RESETS_0` and `RESETS_1` in
/// `soc_esp32c3::periph::system`, at their addresses in the SVD.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuleReset {
    pub register: u32,
    pub bit: u32,
    pub first: u32,
    pub last: u32,
}

/// `SYSTEM.PERIP_RST_EN1` (the SVD; a test checks it).
pub const SYSTEM_PERIP_RST_EN1: u32 = 0x600C_001C;
/// The values of `PERIP_RST_EN0` and `PERIP_RST_EN1` at power-on (the SVD), which a CPU reset
/// keeps.
pub const PERIP_RST_EN_AT_POWER_ON: [u32; 2] = [0, 0x0000_01FE];

/// The modules the machine resets at a pulse of their bit.
pub const MODULE_RESETS: &[ModuleReset] = &[
    // SPI01 resets SPI0 and SPI1.
    module_reset(SYSTEM_PERIP_RST_EN0, 1, 0x6000_2000, 0x6000_23FF),
    module_reset(SYSTEM_PERIP_RST_EN0, 1, 0x6000_3000, 0x6000_33FF),
    module_reset(SYSTEM_PERIP_RST_EN0, 2, 0x6000_0000, 0x6000_00FF),
    module_reset(SYSTEM_PERIP_RST_EN0, 5, 0x6001_0000, 0x6001_00FF),
    module_reset(SYSTEM_PERIP_RST_EN0, 13, 0x6001_F000, 0x6001_F0FF),
    module_reset(SYSTEM_PERIP_RST_EN0, 15, 0x6002_0000, 0x6002_00FF),
    module_reset(SYSTEM_PERIP_RST_EN0, 23, 0x6004_3000, 0x6004_30FF),
    module_reset(SYSTEM_PERIP_RST_EN0, 29, 0x6002_3000, 0x6002_30FF),
    module_reset(SYSTEM_PERIP_RST_EN1, 2, 0x6003_B000, 0x6003_B0FF),
];

const fn module_reset(register: u32, bit: u32, first: u32, last: u32) -> ModuleReset {
    ModuleReset {
        register,
        bit: 1 << bit,
        first,
        last,
    }
}

/// The cycle counter at its machine and user addresses (`mpccr`, `upccr`).
pub const MPCCR: u16 = 0x7E2;
pub const UPCCR: u16 = 0x802;
/// `RTC_CNTL.TIME_UPDATE`, whose bit 31 captures the RTC timer, and `TIME_LOW0`, the low word of
/// the capture (the SVD; a test checks them).
pub const RTC_TIME_UPDATE: u32 = 0x6000_800C;
pub const RTC_TIME_LOW0: u32 = 0x6000_8010;
/// The SYSTIMER block as QEMU maps it, its `CONF`, the low words of its two counters, and
/// `SYSTEM.PERIP_RST_EN0`, whose bit 29 resets the block (the SVD; a test checks them).
pub const SYSTIMER: (u32, u32) = (0x6002_3000, 0x6002_30FF);
pub const SYSTIMER_CONF: u32 = 0x6002_3000;
pub const SYSTIMER_UNIT0_VALUE_LO: u32 = 0x6002_3044;
pub const SYSTIMER_UNIT1_VALUE_LO: u32 = 0x6002_304C;
pub const SYSTEM_PERIP_RST_EN0: u32 = 0x600C_0018;

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

/// Row 2. Rows 2 to 24 were measured on the first life of `flash_hello.bin` (SP1b-4c): every mask
/// is the bits measured to differ there (`docs/lockstep-differences.md` gives the measure and the
/// source of each). A window that shows another bit, or reaches another register, fails until a
/// row of its own admits it with its evidence. Row 1 is retired ([`RETIRED`]).
pub const ROW_MIE_AT_POWER_ON: Difference = Difference {
    number: 2,
    summary: "mie at power-on: QEMU 0x0EEE, inkemu 0, until mie is written",
    treatments: &[Treatment::Pair {
        field: Field::Mie,
        bits: 0x0000_0EEE,
        qemu: 0x0000_0EEE,
        inkemu: 0,
        until_written: true,
    }],
};

/// Row 3.
pub const ROW_MTVEC_MODE_AT_POWER_ON: Difference = Difference {
    number: 3,
    summary: "mtvec.MODE at power-on: QEMU direct, inkemu vectored, until mtvec is written",
    treatments: &[Treatment::Pair {
        field: Field::Mtvec,
        bits: 0x0000_0001,
        qemu: 0,
        inkemu: 1,
        until_written: true,
    }],
};

/// Row 4.
pub const ROW_MEIP: Difference = Difference {
    number: 4,
    summary: "mip.MEIP: QEMU's interrupt-matrix line, three instructions late after a threshold store",
    treatments: &[Treatment::ExternalInterrupt { lag: 3 }],
};

/// Row 5.
pub const ROW_CYCLE_COUNTER: Difference = Difference {
    number: 5,
    summary: "cycle counter: both count 6.25 ns of device time, QEMU's from its first read",
    treatments: &[
        Treatment::Compat(Compat::CycleCounterOnTimeBase),
        Treatment::Counter {
            location: Location::Csr(MPCCR),
            drift: Drift::Within(1),
            origins: &[Location::Csr(MPCCR), Location::Csr(UPCCR)],
        },
        Treatment::Counter {
            location: Location::Csr(UPCCR),
            drift: Drift::Within(1),
            origins: &[Location::Csr(MPCCR), Location::Csr(UPCCR)],
        },
    ],
};

/// Row 6.
pub const ROW_UART0: Difference = Difference {
    number: 6,
    summary: "UART0: line levels, FIFO count and reset values QEMU does not model",
    treatments: &[Treatment::Registers(&[
        copy(0x6000_001C, 0xE0FF_C000),
        copy(0x6000_0020, 0x1000_001C),
        copy(0x6000_006C, 0x0000_0010),
        copy(0x6000_0078, 0x1672_2500),
    ])],
};

/// Row 7.
pub const ROW_SPI1: Difference = Difference {
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
};

/// Row 8.
pub const ROW_SPI0: Difference = Difference {
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
};

/// Row 9.
pub const ROW_RTC_CNTL: Difference = Difference {
    number: 9,
    summary: "RTC_CNTL: QEMU models OPTIONS0, RESET_STATE and STORE0-7 only",
    treatments: &[
        Treatment::SlowClock {
            register: RTC_TIME_LOW0,
            capture: RTC_TIME_UPDATE,
            trigger: 1 << 31,
            hz: 136_000,
            bound: 1,
            qemu: 0,
        },
        Treatment::Registers(&[
            copy(0x6000_8000, 0x1C00_A000),
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
            copy(0x6000_80AC, 0x84B0_0000),
            copy(0x6000_80D8, 0x7BFF_C030),
            copy(0x6000_810C, 0x0000_0007),
            copy(0x6000_811C, 0xC000_0000),
        ]),
    ],
};

/// Row 10.
pub const ROW_EFUSE: Difference = Difference {
    number: 10,
    summary: "eFuse: the words where QEMU's blank eFuses and the X4's differ",
    treatments: &[Treatment::Registers(&[
        copy(0x6000_883C, 0x8000_0000),
        copy(0x6000_8848, 0x0000_58CF),
        copy(0x6000_8850, 0x0100_0000),
        copy(0x6000_8858, 0x0047_D2F2),
        copy(0x6000_886C, 0xD864_EA40),
    ])],
};

/// Row 11.
pub const ROW_IO_MUX: Difference = Difference {
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
};

/// Row 12.
pub const ROW_REGI2C: Difference = Difference {
    number: 12,
    summary: "analog I2C master, absent in QEMU: the ROM's regi2c accesses",
    treatments: &[Treatment::Registers(&[
        copy(0x6000_E000, 0x05FF_FF9E),
        copy(0x6000_E044, 0x00FF_FFFF),
        copy(0x6000_E048, 0x00FF_FFFF),
    ])],
};

/// Row 13.
pub const ROW_RADIO: Difference = Difference {
    number: 13,
    summary: "radio power-up of rtc_sleep_pu, absent in QEMU: BB, NRX, FE and FE2",
    treatments: &[Treatment::Registers(&[
        admit(0x6000_50F0),
        admit(0x6000_6090),
        admit(0x6001_CCD4),
        admit(0x6001_D054),
    ])],
};

/// Row 14.
pub const ROW_TIMG: Difference = Difference {
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
};

/// Row 15.
pub const ROW_SYSTIMER: Difference = Difference {
    number: 15,
    summary: "SYSTIMER: QEMU's counters drop a fraction of a count at each update and always read valid",
    treatments: &[
        Treatment::Registers(&[
            copy(0x6002_3004, 0x2000_0000),
            copy(0x6002_3008, 0x2000_0000),
        ]),
        Treatment::Counter {
            location: Location::Mmio(SYSTIMER_UNIT0_VALUE_LO),
            drift: Drift::Truncated {
                first: SYSTIMER.0,
                last: SYSTIMER.1,
            },
            origins: &[
                Location::Mmio(SYSTEM_PERIP_RST_EN0),
                Location::Mmio(SYSTIMER_CONF),
            ],
        },
        Treatment::Counter {
            location: Location::Mmio(SYSTIMER_UNIT1_VALUE_LO),
            drift: Drift::Truncated {
                first: SYSTIMER.0,
                last: SYSTIMER.1,
            },
            origins: &[
                Location::Mmio(SYSTEM_PERIP_RST_EN0),
                Location::Mmio(SYSTIMER_CONF),
            ],
        },
    ],
};

/// Row 16.
pub const ROW_APB_CTRL: Difference = Difference {
    number: 16,
    summary: "APB_CTRL, absent in QEMU: Wi-Fi clocks and memory power",
    treatments: &[Treatment::Registers(&[
        copy(0x6002_6014, 0xFFFF_FFFF),
        admit(0x6002_6018),
        copy(0x6002_609C, 0x0000_0015),
        copy(0x6002_60AC, 0x0000_003F),
    ])],
};

/// Row 17.
pub const ROW_RNG: Difference = Difference {
    number: 17,
    summary: "RNG: QEMU returns the host's rand()",
    treatments: &[Treatment::Registers(&[copy(0x6002_60B0, u32::MAX)])],
};

/// Row 18.
pub const ROW_APB_SARADC: Difference = Difference {
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
};

/// Row 19.
pub const ROW_USB_SERIAL_JTAG: Difference = Difference {
    number: 19,
    summary: "USB-Serial-JTAG: SERIAL_IN_EP_DATA_FREE, which QEMU reads 0",
    treatments: &[Treatment::Registers(&[copy(0x6004_3004, 0x0000_0002)])],
};

/// Row 20.
pub const ROW_SYSTEM: Difference = Difference {
    number: 20,
    summary: "SYSTEM: clock gates, resets and clock selection QEMU's clock model keeps otherwise",
    treatments: &[Treatment::Registers(&[
        copy(0x600C_0000, 0x0000_0040),
        copy(0x600C_0004, 0x0000_00C0),
        copy(0x600C_0008, 0x0000_000C),
        copy(0x600C_000C, 0x0000_0001),
        copy(0x600C_0010, 0xF9C1_E06F),
        copy(0x600C_0014, 0x0000_0204),
        copy(0x600C_0018, 0xFF7F_FFFF),
        copy(0x600C_001C, 0xFFFF_FFFF),
        copy(0x600C_0020, 0x0000_00FF),
        copy(0x600C_0024, 0x0200_1001),
        copy(0x600C_0040, 0x0000_0007),
        copy(0x600C_0058, 0x0008_0401),
    ])],
};

/// Row 21.
pub const ROW_SENSITIVE: Difference = Difference {
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
};

/// Row 22.
pub const ROW_EXTMEM: Difference = Difference {
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
};

/// Row 23.
pub const ROW_ASSIST_DEBUG: Difference = Difference {
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
};

/// Row 24.
pub const ROW_SYSTIMER_STOPPED: Difference = Difference {
    number: 24,
    summary: "SYSTIMER: no alarm while the counter is stopped",
    treatments: &[Treatment::Compat(Compat::SystimerNoAlarmOnStoppedCounter)],
};

/// Row 25. Rows 25 to 27 were measured on the second life of `flash_hello.bin` (SP1b-4d), after
/// the reset `esp_restart` asks for.
pub const ROW_HELLO_KEPT_AT_RESET: Difference = Difference {
    number: 25,
    summary: "flash_hello.bin, at its reset: QEMU keeps registers and CSRs the chip's CPU reset sets",
    treatments: &[Treatment::KeptAtReset(&[
        kept(Field::X(1), 0),
        kept(Field::X(2), 0),
        kept(Field::X(3), 0),
        kept(Field::X(4), 0),
        kept(Field::X(5), 0),
        kept(Field::X(8), 0),
        kept(Field::X(9), 0),
        kept(Field::X(11), 0),
        kept(Field::X(12), 0),
        kept(Field::X(13), 0),
        kept(Field::X(14), 0),
        kept(Field::X(15), 0),
        kept(Field::X(16), 0),
        kept(Field::X(17), 0),
        kept(Field::X(28), 0),
        MSTATUS_MPIE_KEPT,
        kept(Field::Mtvec, MTVEC_AT_RESET),
        kept(Field::Mepc, 0),
    ])],
};

/// inkemu's `mtvec` after a reset: vectored, base 0 (`cpu_rv32::csr`, the ESP32-C3 core).
const MTVEC_AT_RESET: u32 = 1;

/// `mstatus.MPIE`, the one bit of `mstatus` QEMU keeps at its reset where inkemu's reset, as the
/// chip's (TRM §1.4.2), leaves 0.
const MSTATUS_MPIE_KEPT: Kept = Kept {
    field: Field::Mstatus,
    bits: 1 << 7,
    reset: 0,
};

/// The R, W and X bits of the four entries of a `pmpcfg` register (RISC-V privileged
/// specification, §3.7.1): a reset must clear A and L, and leaves these unspecified.
pub const PMP_RWX: u32 = 0x0707_0707;

/// Row 26.
pub const ROW_PMP_KEPT_AT_RESET: Difference = Difference {
    number: 26,
    summary: "PMP configuration: QEMU's reset keeps the R, W, X bits of every entry, the chip's CPU reset clears them",
    treatments: &[Treatment::CsrKeptAtReset(&[
        KeptCsr {
            csr: 0x3A0,
            kept: PMP_RWX,
            mask: 0x0100_0700,
        },
        KeptCsr {
            csr: 0x3A1,
            kept: PMP_RWX,
            mask: 0x0005_0103,
        },
        KeptCsr {
            csr: 0x3A2,
            kept: PMP_RWX,
            mask: 0x0700_0507,
        },
        KeptCsr {
            csr: 0x3A3,
            kept: PMP_RWX,
            mask: 0x0000_0300,
        },
    ])],
};

/// Row 27.
pub const ROW_AFTER_RESET: Difference = Difference {
    number: 27,
    summary: "after esp_restart: registers the chip's reset and QEMU's leave in different states",
    treatments: &[Treatment::RegistersAfterReset(&[
        // Modules the machine resets (UART0, SPI0/1, TIMG0/1): inkemu shows its SVD reset value
        // in the mask (CLKDIV 0x2B6, CLK_CONF 0x3701000, WDTCONFIG0 0x4C000, WDTCONFIG1
        // 0x10000, RTCCALICFG 0x13000, the SPI1 registers 0); the others: QEMU reads 0.
        after_reset(0x6000_0014, 0x0030_03ED, Some(0x0000_02A4), None),
        after_reset(0x6000_0078, 0x0000_1000, Some(0x0000_1000), None),
        after_reset(0x6000_2010, 0x0000_03E0, Some(0), None),
        after_reset(0x6000_2028, 0x0000_0017, Some(0), None),
        after_reset(0x6000_4154, 0x0000_0030, None, Some(0)),
        after_reset(0x6000_4158, 0x0000_0030, None, Some(0)),
        after_reset(0x6000_8024, 0x0001_0000, None, Some(0)),
        after_reset(0x6000_8028, 0x0001_0000, None, Some(0)),
        after_reset(0x6000_802C, 0x0000_0200, None, Some(0)),
        after_reset(0x6000_8030, 0x0001_0000, None, Some(0)),
        after_reset(0x6000_8080, 0x1000_0000, None, Some(0)),
        after_reset(0x6000_8088, 0x0000_0800, None, Some(0)),
        after_reset(0x6000_808C, 0x0040_0000, None, Some(0)),
        after_reset(0x6000_9004, 0x0000_0002, None, Some(0)),
        after_reset(0x6000_9008, 0x0000_0002, None, Some(0)),
        after_reset(0x6000_900C, 0x0000_0002, None, Some(0)),
        after_reset(0x6000_9010, 0x0000_0002, None, Some(0)),
        after_reset(0x6000_9014, 0x0000_0002, None, Some(0)),
        after_reset(0x6000_9018, 0x0000_0002, None, Some(0)),
        after_reset(0x6000_901C, 0x0000_0002, None, Some(0)),
        after_reset(0x6000_9020, 0x0000_0002, None, Some(0)),
        after_reset(0x6000_9024, 0x0000_0002, None, Some(0)),
        after_reset(0x6000_9028, 0x0000_0002, None, Some(0)),
        after_reset(0x6000_902C, 0x0000_0002, None, Some(0)),
        after_reset(0x6000_9030, 0x0000_0002, None, Some(0)),
        after_reset(0x6000_9034, 0x0000_0002, None, Some(0)),
        after_reset(0x6000_9038, 0x0000_0002, None, Some(0)),
        after_reset(0x6000_9040, 0x0000_0002, None, Some(0)),
        after_reset(0x6000_9044, 0x0000_0002, None, Some(0)),
        after_reset(0x6000_9048, 0x0000_0002, None, Some(0)),
        after_reset(0x6000_904C, 0x0000_0002, None, Some(0)),
        after_reset(0x6000_9050, 0x0000_0002, None, Some(0)),
        after_reset(0x6001_F048, 0x383B_0000, Some(0), Some(0)),
        after_reset(0x6001_F04C, 0x4E20_0000, Some(0), None),
        after_reset(0x6001_F068, 0x0401_3000, Some(0x0001_3000), None),
        after_reset(0x6002_004C, 0x4E20_0000, Some(0), None),
        after_reset(0x6004_0018, 0x0063_0000, None, Some(0)),
        after_reset(0x600C_0008, 0x0000_0001, None, Some(0)),
        after_reset(0x600C_0024, 0x0100_0000, None, Some(0)),
        after_reset(0x600C_4078, 0x0000_0023, None, Some(0)),
        after_reset(0x600C_4084, 0x0000_003F, None, Some(0)),
        after_reset(0x600C_E008, 0x0000_0300, None, Some(0)),
        after_reset(0x600C_E048, 0x4004_8B92, None, Some(0)),
    ])],
};

/// Row 28.
pub const ROW_HELLO_SPI1_AFTER_RESET: Difference = Difference {
    number: 28,
    summary: "flash_hello.bin, after its reset: the SPI1 registers the bootloader reads back",
    treatments: &[Treatment::RegistersAfterReset(&[
        after_reset(0x6000_2018, 0x0000_0080, Some(0), None),
        after_reset(0x6000_2020, 0x0000_009F, Some(0), None),
    ])],
};

/// Row 29. Rows 29 to 32 were measured on both lives of `flash_nvs.bin` (SP1b-4d).
pub const ROW_NVS_SPI1: Difference = Difference {
    number: 29,
    summary: "flash_nvs.bin, SPI1: row 7, and QEMU's flash keeps WEL after a program or an erase",
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
            mask: 0xFFFF_FF02,
            qemu: None,
            after_command: Some(0x05),
        },
        copy(0x6000_2098, 0x0000_0014),
        copy(0x6000_209C, 0x0A00_2000),
        copy(0x6000_20DC, 0x0000_0001),
    ])],
};

/// Row 30.
pub const ROW_NVS_EXTMEM: Difference = Difference {
    number: 30,
    summary: "flash_nvs.bin, EXTMEM: the cache sync around flash writes",
    treatments: &[Treatment::Registers(&[copy(0x600C_4030, 0x0000_0800)])],
};

/// Row 31.
pub const ROW_NVS_KEPT_AT_RESET: Difference = Difference {
    number: 31,
    summary: "flash_nvs.bin, at its reset: QEMU keeps registers and CSRs the chip's CPU reset sets",
    treatments: &[Treatment::KeptAtReset(&[
        kept(Field::X(1), 0),
        kept(Field::X(2), 0),
        kept(Field::X(3), 0),
        kept(Field::X(4), 0),
        kept(Field::X(5), 0),
        kept(Field::X(7), 0),
        kept(Field::X(8), 0),
        kept(Field::X(9), 0),
        kept(Field::X(11), 0),
        kept(Field::X(12), 0),
        kept(Field::X(13), 0),
        kept(Field::X(14), 0),
        kept(Field::X(15), 0),
        kept(Field::X(16), 0),
        kept(Field::X(17), 0),
        kept(Field::X(28), 0),
        kept(Field::X(29), 0),
        kept(Field::X(30), 0),
        kept(Field::X(31), 0),
        MSTATUS_MPIE_KEPT,
        kept(Field::Mtvec, MTVEC_AT_RESET),
        kept(Field::Mepc, 0),
    ])],
};

/// Row 32.
pub const ROW_NVS_SPI1_AFTER_RESET: Difference = Difference {
    number: 32,
    summary: "flash_nvs.bin, after its reset: the SPI1 registers the bootloader reads back",
    treatments: &[Treatment::RegistersAfterReset(&[
        after_reset(0x6000_2018, 0x1000_0080, Some(0), None),
        after_reset(0x6000_2020, 0x0000_0005, Some(0), None),
    ])],
};

/// Row 33, measured on both lives of `flash_nvs_ota.bin` (SP1b-4d).
pub const ROW_NVS_OTA_SPI1_STATUS: Difference = Difference {
    number: 33,
    summary: "flash_nvs_ota.bin, SPI1.RD_STATUS: the ROM reads WEL that QEMU's flash keeps",
    treatments: &[Treatment::Registers(&[copy(0x6000_202C, 0x0000_0002)])],
};

/// Row 34. Rows 34 to 41 were measured on `CrossPoint` 1.6.5 in local (SP1b-4d): its window ends
/// before the scheduler starts, and uses fewer registers and bits of rows 7, 9, 11, 14, 15, 20 and
/// 23 than `hello`, a few others more.
pub const ROW_CROSSPOINT_SPI1: Difference = Difference {
    number: 34,
    summary: "CrossPoint, SPI1: row 7, and the WEL QEMU's flash keeps in W0 after RDSR",
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
            mask: 0x0018_6002,
            qemu: None,
            after_command: Some(0x05),
        },
        copy(0x6000_2098, 0x0000_0014),
        copy(0x6000_209C, 0x0A00_2000),
        copy(0x6000_20DC, 0x0000_0001),
    ])],
};

/// Row 35.
pub const ROW_CROSSPOINT_EFUSE: Difference = Difference {
    number: 35,
    summary: "CrossPoint, eFuse: a word of BLOCK_SYS_DATA1 where QEMU's blank eFuses and the X4's differ",
    treatments: &[Treatment::Registers(&[copy(0x6000_8870, 0x5ABA_3A86)])],
};

/// Row 36.
pub const ROW_CROSSPOINT_TIMG0: Difference = Difference {
    number: 36,
    summary: "CrossPoint, TIMG0: row 14's registers it reads, and more bits of the RTC calibration",
    treatments: &[Treatment::Registers(&[
        copy(0x6001_F048, 0x0004_C000),
        copy(0x6001_F068, 0x0000_8000),
        copy(0x6001_F06C, 0x014A_E180),
        copy(0x6001_F080, 0xFFFF_FF99),
    ])],
};

/// Row 37.
pub const ROW_CROSSPOINT_RTC_CNTL: Difference = Difference {
    number: 37,
    summary: "CrossPoint, RTC_CNTL: row 9, fewer bits of the RTC watchdog's configuration",
    treatments: &[
        Treatment::SlowClock {
            register: RTC_TIME_LOW0,
            capture: RTC_TIME_UPDATE,
            trigger: 1 << 31,
            hz: 136_000,
            bound: 1,
            qemu: 0,
        },
        Treatment::Registers(&[
            copy(0x6000_8000, 0x1C00_A000),
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
            copy(0x6000_8090, 0xC007_FA14),
            copy(0x6000_80AC, 0x04B0_0000),
            copy(0x6000_80D8, 0x7BFF_C030),
            copy(0x6000_810C, 0x0000_0007),
            copy(0x6000_811C, 0xC000_0000),
        ]),
    ],
};

/// Row 38.
pub const ROW_CROSSPOINT_IO_MUX: Difference = Difference {
    number: 38,
    summary: "CrossPoint, IO_MUX: the pads of the flash (GPIO12-17) and of UART0 (GPIO20-21)",
    treatments: &[Treatment::Registers(&[
        copy(0x6000_9034, 0x0000_1B00),
        copy(0x6000_9038, 0x0000_1B00),
        copy(0x6000_903C, 0x0000_0B00),
        copy(0x6000_9040, 0x0000_0B00),
        copy(0x6000_9044, 0x0000_0B00),
        copy(0x6000_9048, 0x0000_0B00),
        copy(0x6000_9054, 0x0000_0B00),
        copy(0x6000_9058, 0x0000_0B00),
    ])],
};

/// Row 39.
pub const ROW_CROSSPOINT_SYSTIMER: Difference = Difference {
    number: 39,
    summary: "CrossPoint, SYSTIMER: row 15 for counter 0, the one read before the scheduler starts",
    treatments: &[
        Treatment::Registers(&[copy(0x6002_3004, 0x2000_0000)]),
        Treatment::Counter {
            location: Location::Mmio(SYSTIMER_UNIT0_VALUE_LO),
            drift: Drift::Truncated {
                first: SYSTIMER.0,
                last: SYSTIMER.1,
            },
            origins: &[
                Location::Mmio(SYSTEM_PERIP_RST_EN0),
                Location::Mmio(SYSTIMER_CONF),
            ],
        },
    ],
};

/// Row 40.
pub const ROW_CROSSPOINT_SYSTEM: Difference = Difference {
    number: 40,
    summary: "CrossPoint, SYSTEM: row 20, one bit fewer of PERIP_RST_EN0",
    treatments: &[Treatment::Registers(&[
        copy(0x600C_0000, 0x0000_0040),
        copy(0x600C_0004, 0x0000_00C0),
        copy(0x600C_0008, 0x0000_000C),
        copy(0x600C_000C, 0x0000_0001),
        copy(0x600C_0010, 0xF9C1_E06F),
        copy(0x600C_0014, 0x0000_0204),
        copy(0x600C_0018, 0xA86D_9BEE),
        copy(0x600C_001C, 0x0000_01FE),
        copy(0x600C_0020, 0x0000_00FF),
        copy(0x600C_0024, 0x0200_1001),
        copy(0x600C_0040, 0x0000_0007),
        copy(0x600C_0058, 0x0008_0401),
    ])],
};

/// Row 41.
pub const ROW_CROSSPOINT_ASSIST_DEBUG: Difference = Difference {
    number: 41,
    summary: "CrossPoint, ASSIST_DEBUG: the three registers the bootloader and the start-up touch",
    treatments: &[Treatment::Registers(&[
        admit(0x600C_E044),
        admit(0x600C_E048),
        admit(0x600C_E098),
    ])],
};

/// The rows of the window `hello` ([`crate::window::WINDOWS`]).
pub const HELLO: &[Difference] = &[
    ROW_MIE_AT_POWER_ON,
    ROW_MTVEC_MODE_AT_POWER_ON,
    ROW_MEIP,
    ROW_CYCLE_COUNTER,
    ROW_UART0,
    ROW_SPI1,
    ROW_SPI0,
    ROW_RTC_CNTL,
    ROW_EFUSE,
    ROW_IO_MUX,
    ROW_REGI2C,
    ROW_RADIO,
    ROW_TIMG,
    ROW_SYSTIMER,
    ROW_APB_CTRL,
    ROW_RNG,
    ROW_APB_SARADC,
    ROW_USB_SERIAL_JTAG,
    ROW_SYSTEM,
    ROW_SENSITIVE,
    ROW_EXTMEM,
    ROW_ASSIST_DEBUG,
    ROW_SYSTIMER_STOPPED,
    ROW_HELLO_KEPT_AT_RESET,
    ROW_PMP_KEPT_AT_RESET,
    ROW_AFTER_RESET,
    ROW_HELLO_SPI1_AFTER_RESET,
];

/// The rows of the window `nvs`.
pub const NVS: &[Difference] = &[
    ROW_MIE_AT_POWER_ON,
    ROW_MTVEC_MODE_AT_POWER_ON,
    ROW_MEIP,
    ROW_CYCLE_COUNTER,
    ROW_UART0,
    ROW_SPI0,
    ROW_RTC_CNTL,
    ROW_EFUSE,
    ROW_IO_MUX,
    ROW_REGI2C,
    ROW_RADIO,
    ROW_TIMG,
    ROW_SYSTIMER,
    ROW_APB_CTRL,
    ROW_RNG,
    ROW_APB_SARADC,
    ROW_USB_SERIAL_JTAG,
    ROW_SYSTEM,
    ROW_SENSITIVE,
    ROW_EXTMEM,
    ROW_ASSIST_DEBUG,
    ROW_SYSTIMER_STOPPED,
    ROW_PMP_KEPT_AT_RESET,
    ROW_AFTER_RESET,
    ROW_NVS_SPI1,
    ROW_NVS_EXTMEM,
    ROW_NVS_KEPT_AT_RESET,
    ROW_NVS_SPI1_AFTER_RESET,
];

/// The rows of the window `nvs-ota`: those of `nvs`, and row 33.
pub const NVS_OTA: &[Difference] = &[
    ROW_MIE_AT_POWER_ON,
    ROW_MTVEC_MODE_AT_POWER_ON,
    ROW_MEIP,
    ROW_CYCLE_COUNTER,
    ROW_UART0,
    ROW_SPI0,
    ROW_RTC_CNTL,
    ROW_EFUSE,
    ROW_IO_MUX,
    ROW_REGI2C,
    ROW_RADIO,
    ROW_TIMG,
    ROW_SYSTIMER,
    ROW_APB_CTRL,
    ROW_RNG,
    ROW_APB_SARADC,
    ROW_USB_SERIAL_JTAG,
    ROW_SYSTEM,
    ROW_SENSITIVE,
    ROW_EXTMEM,
    ROW_ASSIST_DEBUG,
    ROW_SYSTIMER_STOPPED,
    ROW_PMP_KEPT_AT_RESET,
    ROW_AFTER_RESET,
    ROW_NVS_SPI1,
    ROW_NVS_EXTMEM,
    ROW_NVS_KEPT_AT_RESET,
    ROW_NVS_SPI1_AFTER_RESET,
    ROW_NVS_OTA_SPI1_STATUS,
];

/// Every row of the table, in the order of `docs/lockstep-differences.md`: those of every window.
pub const DIFFERENCES: &[Difference] = &[
    ROW_MIE_AT_POWER_ON,
    ROW_MTVEC_MODE_AT_POWER_ON,
    ROW_MEIP,
    ROW_CYCLE_COUNTER,
    ROW_UART0,
    ROW_SPI1,
    ROW_SPI0,
    ROW_RTC_CNTL,
    ROW_EFUSE,
    ROW_IO_MUX,
    ROW_REGI2C,
    ROW_RADIO,
    ROW_TIMG,
    ROW_SYSTIMER,
    ROW_APB_CTRL,
    ROW_RNG,
    ROW_APB_SARADC,
    ROW_USB_SERIAL_JTAG,
    ROW_SYSTEM,
    ROW_SENSITIVE,
    ROW_EXTMEM,
    ROW_ASSIST_DEBUG,
    ROW_SYSTIMER_STOPPED,
    ROW_HELLO_KEPT_AT_RESET,
    ROW_PMP_KEPT_AT_RESET,
    ROW_AFTER_RESET,
    ROW_HELLO_SPI1_AFTER_RESET,
    ROW_NVS_SPI1,
    ROW_NVS_EXTMEM,
    ROW_NVS_KEPT_AT_RESET,
    ROW_NVS_SPI1_AFTER_RESET,
    ROW_NVS_OTA_SPI1_STATUS,
    ROW_CROSSPOINT_SPI1,
    ROW_CROSSPOINT_EFUSE,
    ROW_CROSSPOINT_TIMG0,
    ROW_CROSSPOINT_RTC_CNTL,
    ROW_CROSSPOINT_IO_MUX,
    ROW_CROSSPOINT_SYSTIMER,
    ROW_CROSSPOINT_SYSTEM,
    ROW_CROSSPOINT_ASSIST_DEBUG,
];

/// The rows of the window `crosspoint`, compared in local only (`CrossPoint` 1.6.5 cannot be
/// distributed): rows of the first life of `hello` it uses whole, row 33, and its own.
pub const CROSSPOINT: &[Difference] = &[
    ROW_MIE_AT_POWER_ON,
    ROW_MTVEC_MODE_AT_POWER_ON,
    ROW_CYCLE_COUNTER,
    ROW_UART0,
    ROW_SPI0,
    ROW_EFUSE,
    ROW_REGI2C,
    ROW_RADIO,
    ROW_APB_CTRL,
    ROW_RNG,
    ROW_APB_SARADC,
    ROW_USB_SERIAL_JTAG,
    ROW_SENSITIVE,
    ROW_EXTMEM,
    ROW_NVS_OTA_SPI1_STATUS,
    ROW_CROSSPOINT_SPI1,
    ROW_CROSSPOINT_EFUSE,
    ROW_CROSSPOINT_TIMG0,
    ROW_CROSSPOINT_RTC_CNTL,
    ROW_CROSSPOINT_IO_MUX,
    ROW_CROSSPOINT_SYSTIMER,
    ROW_CROSSPOINT_SYSTEM,
    ROW_CROSSPOINT_ASSIST_DEBUG,
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
        assert_eq!(RTC_TIME_UPDATE, addr("RTC_CNTL", "TIME_UPDATE"));
        assert_eq!(RTC_TIME_LOW0, addr("RTC_CNTL", "TIME_LOW0"));
        assert_eq!(SYSTIMER_CONF, addr("SYSTIMER", "CONF"));
        assert_eq!(SYSTIMER_UNIT0_VALUE_LO, addr("SYSTIMER", "UNIT0_VALUE_LO"));
        assert_eq!(SYSTIMER_UNIT1_VALUE_LO, addr("SYSTIMER", "UNIT1_VALUE_LO"));
        assert_eq!(SYSTEM_PERIP_RST_EN0, addr("SYSTEM", "PERIP_RST_EN0"));
        assert_eq!(SYSTEM_PERIP_RST_EN1, addr("SYSTEM", "PERIP_RST_EN1"));
        // The first register of every module a pulse resets.
        let firsts = [
            ("SPI1", "CMD"),
            ("SPI0", "CTRL"),
            ("UART0", "FIFO"),
            ("UART1", "FIFO"),
            ("TIMG0", "T0CONFIG"),
            ("TIMG1", "T0CONFIG"),
            ("USB_DEVICE", "EP1"),
            ("SYSTIMER", "CONF"),
            ("SHA", "MODE"),
        ];
        for (module, (peripheral, register)) in MODULE_RESETS.iter().zip(firsts) {
            assert!(
                (module.first..=module.last).contains(&addr(peripheral, register)),
                "{peripheral}"
            );
        }
        let systimer = QEMU_DEVICES.iter().find(|d| d.name == "SYSTIMER").unwrap();
        assert_eq!(SYSTIMER, (systimer.first, systimer.last));
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

    /// A register is named by one row of a window (`window.rs` holds that); here, every register
    /// is a word, and a row that copies only after a reset names none its own window's rows copy
    /// before one with the same bits.
    #[test]
    fn every_register_is_word_aligned_and_an_after_reset_entry_adds_bits() {
        for window in crate::WINDOWS {
            let mut before = std::collections::BTreeMap::new();
            for row in window.rows {
                for register in row.registers_at_all() {
                    assert_eq!(register.address % 4, 0, "{:#010x}", register.address);
                }
                for treatment in row.treatments {
                    if let Treatment::Registers(registers) = treatment {
                        for register in *registers {
                            *before.entry(register.address).or_insert(0) |= register.mask;
                        }
                    }
                }
            }
            for row in window.rows {
                for treatment in row.treatments {
                    if let Treatment::RegistersAfterReset(entries) = treatment {
                        for register in entries.iter().map(|entry| entry.register) {
                            let earlier = before.get(&register.address).copied().unwrap_or(0);
                            assert_eq!(
                                register.mask & earlier,
                                0,
                                "window {}: {:#010x}",
                                window.name,
                                register.address
                            );
                        }
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
