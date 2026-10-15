//! What an interface shows of a machine (spec §4 of `SP4a`): its CPU, its code around an
//! address, some memory, and the points; read without effect (4a-D5), the flash's content never
//! shown (4a-D6).
use crate::points::{Points, PointsView};
use crate::region::{Region, in_flash_code};
use cpu_rv32::csr::{
    MARCHID, MCAUSE, MCOUNTEREN, MCYCLE, MCYCLEH, MEPC, MHARTID, MIE, MIMPID, MINSTRET, MINSTRETH,
    MIP, MISA, MSCRATCH, MSTATUS, MTVAL, MTVEC, MVENDORID,
};
use cpu_rv32::csr_esp32c3::{
    PCCR_MACHINE, PCCR_USER, PCER_MACHINE, PCER_USER, PCMR_MACHINE, PCMR_USER, USTATUS,
};
use cpu_rv32::disasm::{ABI_NAMES, csr_name, disassemble};
use cpu_rv32::pmp::{PMPADDR0, PMPCFG0};
use cpu_rv32::{Decoded, Privilege, decode};
use soc_esp32c3::{Access, DebugStop, Esp32C3, Peeked};

/// The bytes of memory one view gives at most; a longer request gives the first of them.
pub const MAX_MEMORY_BYTES: u32 = 1024;
/// The lines of one disassembly at most.
pub const MAX_DISASSEMBLY_LINES: u32 = 64;
/// The lines of a disassembly when the request does not say.
pub const DEFAULT_DISASSEMBLY_LINES: u32 = 24;
/// The lines a disassembly around the PC shows before it, at most a third of its lines.
const LINES_BEFORE_PC: u32 = 8;

/// What an interface asks to see.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewRequest {
    /// Where the disassembly starts; `None` for the code around the PC.
    #[serde(default)]
    pub disassembly_at: Option<u32>,
    /// Its lines, 1 to [`MAX_DISASSEMBLY_LINES`]; `None` for [`DEFAULT_DISASSEMBLY_LINES`].
    #[serde(default)]
    pub disassembly_lines: Option<u32>,
    /// Memory to show; `None` for none.
    #[serde(default)]
    pub memory: Option<MemoryRequest>,
}

/// Memory an interface asks to see.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryRequest {
    pub address: u32,
    pub length: u32,
}

/// Everything a debugger panel shows.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugView {
    pub cpu: CpuView,
    pub disassembly: Vec<Line>,
    pub memory: Option<MemoryView>,
    pub points: PointsView,
}

/// The CPU at an instruction boundary.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuView {
    pub pc: u32,
    /// `machine` or `user`.
    pub privilege: &'static str,
    /// The CPU waits in `wfi`: `pc` is the instruction after it.
    pub asleep: bool,
    /// `x0` to `x31`.
    pub registers: Vec<RegisterView>,
    /// The CSRs the core has, in a fixed order.
    pub csrs: Vec<CsrView>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterView {
    pub number: u8,
    /// Its ABI name.
    pub name: &'static str,
    pub value: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CsrView {
    pub address: u16,
    pub name: &'static str,
    pub value: u32,
}

/// One line of a disassembly.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub address: u32,
    /// 2 or 4; 2 for a line that could not be read, which moves the next one by a parcel.
    pub length: u8,
    /// The instruction; `None` where no code can be read: registers, nothing, a flash page no MMU
    /// entry backs, or the data window of the flash.
    pub instruction: Option<Instruction>,
    /// The PC is here.
    pub current: bool,
    pub breakpoint: bool,
}

/// An instruction of a disassembly.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Instruction {
    /// As `objdump` writes it ([`cpu_rv32::disasm`]).
    pub text: String,
    /// A 16-bit parcel, written in its expanded form.
    pub compressed: bool,
}

/// Bytes of memory, read without effect.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryView {
    pub address: u32,
    /// One per address: the byte, or `None` where none is shown ([`MemoryView::regions`] says
    /// why).
    pub bytes: Vec<Option<u8>>,
    /// The bytes by region, in order, consecutive.
    pub regions: Vec<RegionSpan>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionSpan {
    pub start: u32,
    pub length: u32,
    pub region: Region,
}

/// Why the machine stopped for the debugger, for an interface (`Status.stop`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum StopView {
    /// Before the instruction at `pc`.
    Breakpoint { pc: u32 },
    /// After the instruction at `pc`, which made this access to watched bytes.
    Watchpoint {
        address: u32,
        size: u8,
        /// `read` or `write`.
        access: &'static str,
        value: u32,
        pc: u32,
    },
}

impl From<DebugStop> for StopView {
    fn from(stop: DebugStop) -> StopView {
        match stop {
            DebugStop::Breakpoint { pc } => StopView::Breakpoint { pc },
            DebugStop::Watchpoint(hit) => StopView::Watchpoint {
                address: hit.address,
                size: hit.size,
                access: match hit.access {
                    Access::Read => "read",
                    Access::Write => "write",
                },
                value: hit.value,
                pc: hit.pc,
            },
        }
    }
}

impl StopView {
    /// The line of the journal that says the stop.
    #[must_use]
    pub fn describe(&self) -> String {
        match *self {
            StopView::Breakpoint { pc } => {
                format!("inkemu: stopped at the breakpoint {pc:#010x}, before its instruction")
            }
            StopView::Watchpoint {
                address,
                size,
                access,
                value,
                pc,
            } => {
                let verb = if access == "read" { "read" } else { "wrote" };
                format!(
                    "inkemu: stopped by a watchpoint: the instruction at {pc:#010x} {verb} {size} byte{} at {address:#010x} (value {value:#x})",
                    if size == 1 { "" } else { "s" }
                )
            }
        }
    }
}

/// The CSRs a view shows, in this order, those the core has.
const CSR_ORDER: [u16; 25] = [
    MSTATUS,
    MISA,
    MIE,
    MIP,
    MTVEC,
    MEPC,
    MCAUSE,
    MTVAL,
    MSCRATCH,
    MCOUNTEREN,
    MCYCLE,
    MCYCLEH,
    MINSTRET,
    MINSTRETH,
    PCER_MACHINE,
    PCMR_MACHINE,
    PCCR_MACHINE,
    USTATUS,
    PCER_USER,
    PCMR_USER,
    PCCR_USER,
    MVENDORID,
    MARCHID,
    MIMPID,
    MHARTID,
];

/// The 4 `pmpcfg` and the 16 `pmpaddr` follow the CSRs of [`CSR_ORDER`].
const PMP_CSRS: u16 = 20;

fn csrs(machine: &Esp32C3) -> Vec<CsrView> {
    let pmp = (0..4)
        .map(|index| PMPCFG0 + index)
        .chain((0..PMP_CSRS - 4).map(|index| PMPADDR0 + index));
    CSR_ORDER
        .into_iter()
        .chain(pmp)
        .filter_map(|address| {
            Some(CsrView {
                address,
                name: csr_name(address)?,
                value: machine.csr(address)?,
            })
        })
        .collect()
}

/// The CPU of `machine`.
#[must_use]
pub fn cpu(machine: &Esp32C3) -> CpuView {
    let values = machine.x();
    CpuView {
        pc: machine.pc(),
        privilege: match machine.privilege() {
            Privilege::Machine => "machine",
            Privilege::User => "user",
        },
        asleep: machine.is_asleep(),
        registers: ABI_NAMES
            .iter()
            .zip(values)
            .zip(0u8..)
            .map(|((name, value), number)| RegisterView {
                number,
                name,
                value,
            })
            .collect(),
        csrs: csrs(machine),
    }
}

/// A byte of code at `address`: a byte of memory, or of the flash's instruction window, which
/// holds the code the CPU executes (4a-D6); `None` elsewhere.
fn code_byte(machine: &Esp32C3, address: u32) -> Option<u8> {
    match machine.peek(address, 1).first().copied() {
        Some(Peeked::Byte(byte)) => Some(byte),
        Some(Peeked::Flash(byte)) if in_flash_code(address) => Some(byte),
        _ => None,
    }
}

/// The instruction at `address`, when its bytes are code that can be read.
fn instruction_at(machine: &Esp32C3, address: u32) -> Option<Decoded> {
    let low = [
        code_byte(machine, address)?,
        code_byte(machine, address.checked_add(1)?)?,
    ];
    let parcel = u32::from(u16::from_le_bytes(low));
    if parcel & 0b11 != 0b11 {
        return Some(decode(parcel));
    }
    let high = [
        code_byte(machine, address.checked_add(2)?)?,
        code_byte(machine, address.checked_add(3)?)?,
    ];
    Some(decode(parcel | u32::from(u16::from_le_bytes(high)) << 16))
}

/// One line at `address`.
fn line(machine: &Esp32C3, points: &Points, address: u32) -> Line {
    let decoded = instruction_at(machine, address);
    Line {
        address,
        length: decoded.map_or(2, |decoded| decoded.len),
        instruction: decoded.map(|decoded| Instruction {
            text: disassemble(&decoded, address),
            compressed: decoded.len == 2,
        }),
        current: address == machine.pc(),
        breakpoint: points.has_breakpoint(address),
    }
}

/// `count` lines from `start`, the last cut where the address space ends.
fn lines_from(machine: &Esp32C3, points: &Points, start: u32, count: u32) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut address = Some(start);
    while let Some(at) = address
        && u32::try_from(lines.len()).is_ok_and(|len| len < count)
    {
        let one = line(machine, points, at);
        address = at.checked_add(u32::from(one.length));
        lines.push(one);
    }
    lines
}

/// Where a disassembly around `pc` starts: the furthest address, at most `before` lines of 4
/// bytes back, from which decoding forward lands exactly on `pc`; `pc` itself when none does.
/// The lines before the PC are a best guess with 16-bit parcels (research §2).
fn start_before(machine: &Esp32C3, pc: u32, before: u32) -> u32 {
    for back in (1..=2 * before).rev() {
        let Some(start) = pc.checked_sub(2 * back) else {
            continue;
        };
        let mut at = start;
        while at < pc {
            match instruction_at(machine, at) {
                Some(decoded) => at = at.saturating_add(u32::from(decoded.len)),
                None => break,
            }
        }
        if at == pc {
            return start;
        }
    }
    pc
}

/// The code of `machine` from `request.disassembly_at`, or around its PC.
#[must_use]
pub fn disassembly(machine: &Esp32C3, points: &Points, request: &ViewRequest) -> Vec<Line> {
    let count = request
        .disassembly_lines
        .unwrap_or(DEFAULT_DISASSEMBLY_LINES)
        .clamp(1, MAX_DISASSEMBLY_LINES);
    if let Some(at) = request.disassembly_at {
        return lines_from(machine, points, at & !1, count);
    }
    let pc = machine.pc();
    let before = LINES_BEFORE_PC.min(count / 3);
    let start = start_before(machine, pc, before);
    let lines = lines_from(machine, points, start, count + 2 * before);
    let at_pc = lines
        .iter()
        .position(|line| line.address == pc)
        .unwrap_or(0);
    let lines_kept = |n: u32| usize::try_from(n).unwrap_or(usize::MAX);
    let first = at_pc.saturating_sub(lines_kept(before));
    lines
        .into_iter()
        .skip(first)
        .take(lines_kept(count))
        .collect()
}

/// `request` of memory of `machine`: a byte where the memory holds one; none for the flash
/// windows, whose content is never shown (4a-D6), nor for registers, never read (4a-D5).
#[must_use]
pub fn memory(machine: &Esp32C3, request: MemoryRequest) -> MemoryView {
    let length = request.length.min(MAX_MEMORY_BYTES);
    let mut bytes = Vec::new();
    let mut regions: Vec<RegionSpan> = Vec::new();
    for (peeked, offset) in machine
        .peek(request.address, length)
        .into_iter()
        .zip(0u32..)
    {
        let address = request.address.wrapping_add(offset);
        let (byte, region) = match peeked {
            Peeked::Byte(byte) => (Some(byte), Region::of(address)),
            Peeked::Flash(_) => (None, Region::Flash),
            Peeked::Unbacked => (None, Region::Unbacked),
            Peeked::Register => (None, Region::Registers),
            Peeked::Unmapped => (None, Region::Unmapped),
        };
        bytes.push(byte);
        match regions.last_mut() {
            Some(span) if span.region == region => span.length += 1,
            _ => regions.push(RegionSpan {
                start: address,
                length: 1,
                region,
            }),
        }
    }
    MemoryView {
        address: request.address,
        bytes,
        regions,
    }
}

/// Everything `request` asks of `machine`, with the points of the debugger.
#[must_use]
pub fn inspect(machine: &Esp32C3, points: &Points, request: &ViewRequest) -> DebugView {
    DebugView {
        cpu: cpu(machine),
        disassembly: disassembly(machine, points, request),
        memory: request.memory.map(|wanted| memory(machine, wanted)),
        points: points.view(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpu_rv32::asm::{addi, lui, sw};
    use emu_core::Cycles;
    use soc_esp32c3::memmap::{FLASH_DBUS_BASE, FLASH_IBUS_BASE, ROM_BASE, SRAM_DBUS_BASE};
    use soc_esp32c3::test_support::{HANG, flat_rom, stop_watchdogs};
    use soc_esp32c3::{SocConfig, WatchHit};

    /// A program that stores `0x2a` at the start of the SRAM, then hangs; the index of the hang.
    fn program() -> (Vec<u32>, usize) {
        let mut program = stop_watchdogs();
        program.extend([
            lui(5, SRAM_DBUS_BASE),
            addi(6, 0, 0x2a),
            sw(6, 5, 0),
            // `c.addi a0, 1` then `c.nop`: two compressed parcels in one word.
            0x0001_0505,
        ]);
        let hang = program.len();
        program.push(HANG);
        (program, hang)
    }

    fn at(index: usize) -> u32 {
        ROM_BASE + 4 * u32::try_from(index).unwrap()
    }

    /// The text of a line that holds an instruction.
    fn text(line: &Line) -> &str {
        &line.instruction.as_ref().unwrap().text
    }

    fn run_to_the_hang() -> (Esp32C3, usize) {
        let (program, hang) = program();
        let mut machine = Esp32C3::new(flat_rom(&program), &SocConfig::default());
        let mut points = Points::default();
        points.set_breakpoint(at(hang)).unwrap();
        points.apply(&mut machine);
        assert!(matches!(
            machine.run_stopping(Cycles(100_000), None),
            soc_esp32c3::RunStop::Debug(_)
        ));
        (machine, hang)
    }

    #[test]
    fn the_cpu_is_shown_with_its_registers_by_name_and_the_csrs_of_the_core() {
        let (machine, hang) = run_to_the_hang();
        let view = cpu(&machine);
        assert_eq!(view.pc, at(hang));
        assert_eq!(view.privilege, "machine");
        assert!(!view.asleep);
        assert_eq!(view.registers.len(), 32);
        assert_eq!(
            (
                view.registers[6].number,
                view.registers[6].name,
                view.registers[6].value
            ),
            (6, "t1", 0x2a)
        );
        assert_eq!(view.registers[10].value, 1, "a0 after `c.addi a0, 1`");
        let names: Vec<&str> = view.csrs.iter().map(|csr| csr.name).collect();
        assert_eq!(&names[..3], ["mstatus", "misa", "mie"]);
        assert!(names.contains(&"mpccr") && names.contains(&"pmpaddr15"));
        assert!(
            view.csrs
                .iter()
                .all(|csr| machine.csr(csr.address) == Some(csr.value)),
            "each as an instruction would read it"
        );
    }

    #[test]
    fn the_disassembly_around_the_pc_lands_on_it_and_marks_the_points() {
        let (machine, hang) = run_to_the_hang();
        let mut points = Points::default();
        points.set_breakpoint(at(hang)).unwrap();
        let lines = disassembly(&machine, &points, &ViewRequest::default());
        assert_eq!(lines.len(), DEFAULT_DISASSEMBLY_LINES as usize);
        let current: Vec<&Line> = lines.iter().filter(|line| line.current).collect();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].address, at(hang));
        assert_eq!(text(current[0]), format!("j {:#010x}", at(hang)));
        assert!(current[0].breakpoint);
        let texts: Vec<&str> = lines.iter().map(text).collect();
        let store = texts
            .iter()
            .position(|text| *text == "sw t1, 0(t0)")
            .unwrap();
        assert_eq!(texts[store + 1], "addi a0, a0, 1");
        let compressed = lines[store + 1].instruction.as_ref().unwrap();
        assert!(compressed.compressed && lines[store + 1].length == 2);
        assert_eq!(texts[store + 2], "nop");
        assert!(lines.iter().take_while(|line| !line.current).count() <= 8);
    }

    #[test]
    fn a_disassembly_at_an_address_starts_there_and_says_what_cannot_be_read() {
        let (machine, _) = run_to_the_hang();
        let points = Points::default();
        let request = ViewRequest {
            disassembly_at: Some(0x6000_0001),
            disassembly_lines: Some(3),
            memory: None,
        };
        let lines = disassembly(&machine, &points, &request);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].address, 0x6000_0000, "an even address");
        assert!(lines.iter().all(|line| line.instruction.is_none()));
        assert_eq!(lines[1].address, 0x6000_0002);
        let flash_data = ViewRequest {
            disassembly_at: Some(FLASH_DBUS_BASE),
            disassembly_lines: Some(1),
            memory: None,
        };
        assert!(
            disassembly(&machine, &points, &flash_data)[0]
                .instruction
                .is_none()
        );
        let top = ViewRequest {
            disassembly_at: Some(u32::MAX - 1),
            disassembly_lines: Some(5),
            memory: None,
        };
        assert_eq!(
            disassembly(&machine, &points, &top).len(),
            1,
            "the end of the space"
        );
    }

    #[test]
    fn the_code_of_the_flash_is_read_through_its_instruction_window_only() {
        // An erased flash: no MMU entry is valid at power-on, so the window reads nothing.
        let machine = Esp32C3::new(flat_rom(&[HANG]), &SocConfig::default());
        let request = ViewRequest {
            disassembly_at: Some(FLASH_IBUS_BASE),
            disassembly_lines: Some(1),
            memory: None,
        };
        assert!(
            disassembly(&machine, &Points::default(), &request)[0]
                .instruction
                .is_none()
        );
    }

    #[test]
    fn the_memory_shows_the_ram_and_never_the_flash_nor_the_registers() {
        let (machine, _) = run_to_the_hang();
        let view = memory(
            &machine,
            MemoryRequest {
                address: SRAM_DBUS_BASE,
                length: 4,
            },
        );
        assert_eq!(view.bytes, [Some(0x2a), Some(0), Some(0), Some(0)]);
        assert_eq!(
            view.regions,
            [RegionSpan {
                start: SRAM_DBUS_BASE,
                length: 4,
                region: Region::Sram
            }]
        );
        let flash = memory(
            &machine,
            MemoryRequest {
                address: FLASH_DBUS_BASE - 2,
                length: 4,
            },
        );
        assert_eq!(flash.bytes, [None; 4]);
        assert_eq!(
            flash
                .regions
                .iter()
                .map(|span| span.region)
                .collect::<Vec<_>>(),
            [Region::Unmapped, Region::Unbacked]
        );
        let registers = memory(
            &machine,
            MemoryRequest {
                address: 0x6000_0000,
                length: 2,
            },
        );
        assert_eq!(registers.bytes, [None, None]);
        assert_eq!(registers.regions[0].region, Region::Registers);
        let long = memory(
            &machine,
            MemoryRequest {
                address: SRAM_DBUS_BASE,
                length: 10_000,
            },
        );
        assert_eq!(long.bytes.len(), MAX_MEMORY_BYTES as usize);
    }

    #[test]
    fn a_stop_is_said_in_words_and_named_for_the_interface() {
        let stop = StopView::from(DebugStop::Watchpoint(WatchHit {
            address: 0x3fc8_0010,
            size: 4,
            access: Access::Write,
            value: 0x2a,
            pc: 0x4200_0100,
        }));
        assert_eq!(
            stop.describe(),
            "inkemu: stopped by a watchpoint: the instruction at 0x42000100 wrote 4 bytes at 0x3fc80010 (value 0x2a)"
        );
        assert_eq!(
            serde_json::to_value(stop).unwrap(),
            serde_json::json!({
                "kind": "watchpoint", "address": 0x3fc8_0010_u32, "size": 4, "access": "write",
                "value": 0x2a, "pc": 0x4200_0100_u32
            })
        );
        let breakpoint = StopView::Breakpoint { pc: 0x4000_0000 };
        assert_eq!(
            serde_json::to_value(breakpoint).unwrap(),
            serde_json::json!({ "kind": "breakpoint", "pc": 0x4000_0000_u32 })
        );
        assert!(breakpoint.describe().contains("0x40000000"));
    }

    #[test]
    fn a_view_is_written_in_the_shape_the_interface_reads() {
        let (machine, _) = run_to_the_hang();
        let mut points = Points::default();
        points.set_breakpoint(ROM_BASE).unwrap();
        let request: ViewRequest = serde_json::from_value(serde_json::json!({
            "disassemblyLines": 2,
            "memory": { "address": SRAM_DBUS_BASE, "length": 1 }
        }))
        .unwrap();
        let view = serde_json::to_value(inspect(&machine, &points, &request)).unwrap();
        assert_eq!(view["cpu"]["registers"][2]["name"], "sp");
        assert_eq!(view["disassembly"].as_array().unwrap().len(), 2);
        assert_eq!(view["memory"]["bytes"], serde_json::json!([0x2a]));
        assert_eq!(view["memory"]["regions"][0]["region"], "sram");
        assert_eq!(view["points"]["breakpoints"], serde_json::json!([ROM_BASE]));
        assert!(
            serde_json::from_value::<ViewRequest>(serde_json::json!({ "memry": null })).is_err(),
            "an unknown field is refused"
        );
    }
}
