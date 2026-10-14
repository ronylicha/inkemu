//! Fixtures for tests of this crate and, behind the `test-support` feature, of its dependants.
// test fixtures: a panic is a test failure
#![allow(clippy::unwrap_used, clippy::missing_panics_doc)]

use crate::memmap::ROM_BASE;
use crate::rom::{ROM_LEN, RomImage};
use crate::{ConsoleChannel, Diagnostic, Esp32C3, FlashWrite, StopReason};
use cpu_rv32::asm;
use emu_core::Cycles;

/// A minimal ELF32 little-endian writer: enough structure for `RomImage::from_elf`, nothing more.
#[derive(Clone, Debug, Default)]
pub struct ElfBuilder {
    pub machine: u16,
    pub entry: u32,
    /// `(vaddr, paddr, bytes)` of each `PT_LOAD` segment.
    pub segments: Vec<(u32, u32, Vec<u8>)>,
    /// `(name, addr, bytes)` of each PROGBITS section; none of them is ALLOC.
    pub sections: Vec<(&'static str, u32, Vec<u8>)>,
    /// `(name, value)` of each global absolute symbol.
    pub symbols: Vec<(&'static str, u32)>,
}

fn u16le(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn u32le(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn len32(bytes: &[u8]) -> u32 {
    u32::try_from(bytes.len()).unwrap()
}

fn count32(n: usize) -> u32 {
    u32::try_from(n).unwrap()
}

/// One ELF32 section header, in the field order of `Elf32_Shdr`.
struct SectionHeader {
    name: u32,
    kind: u32,
    addr: u32,
    offset: u32,
    size: u32,
    link: u32,
    info: u32,
    entsize: u32,
}

impl SectionHeader {
    fn write(&self, out: &mut Vec<u8>) {
        // sh_flags is 0 (not ALLOC) and sh_addralign is 1 for every section we write.
        for v in [
            self.name,
            self.kind,
            0,
            self.addr,
            self.offset,
            self.size,
            self.link,
            self.info,
            1,
            self.entsize,
        ] {
            u32le(out, v);
        }
    }
}

const ELF_HEADER_LEN: u32 = 52;
const PROGRAM_HEADER_LEN: u32 = 32;
const SECTION_HEADER_LEN: u32 = 40;
const SYMBOL_LEN: usize = 16;
const SHT_PROGBITS: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_STRTAB: u32 = 3;
/// Null section, `.shstrtab`, `.strtab` and `.symtab` around the user sections.
const SYNTHETIC_SECTIONS: u32 = 4;

/// Appends `text` and a NUL to a string table and returns its offset.
fn push_string(table: &mut Vec<u8>, text: &str) -> u32 {
    let offset = len32(table);
    table.extend_from_slice(text.as_bytes());
    table.push(0);
    offset
}

/// Where the pieces of the file body went, plus the table sizes the section headers need.
struct BodyLayout {
    segment_offsets: Vec<u32>,
    section_offsets: Vec<u32>,
    section_names: Vec<u32>,
    shstrtab_name: u32,
    strtab_name: u32,
    symtab_name: u32,
    shstrtab: (u32, u32),
    strtab: (u32, u32),
    symtab: (u32, u32),
    section_header_offset: u32,
}

impl ElfBuilder {
    /// Serialises the description into an ELF32 file.
    #[must_use]
    pub fn build(&self) -> Vec<u8> {
        let mut body = Vec::new(); // everything after the program headers
        let layout = self.lay_out_body(&mut body);
        let user = count32(self.sections.len());

        let mut out = self.header(layout.section_header_offset, user);
        for (i, (vaddr, paddr, bytes)) in self.segments.iter().enumerate() {
            // PT_LOAD, offset, vaddr, paddr, filesz, memsz, flags R+X, align 4.
            for v in [
                1,
                layout.segment_offsets[i],
                *vaddr,
                *paddr,
                len32(bytes),
                len32(bytes),
                5,
                4,
            ] {
                u32le(&mut out, v);
            }
        }
        out.extend_from_slice(&body);
        self.write_section_headers(&mut out, &layout, user + 2);
        out
    }

    /// Places segments, sections and the string/symbol tables in `body`.
    fn lay_out_body(&self, body: &mut Vec<u8>) -> BodyLayout {
        let body_base = ELF_HEADER_LEN + PROGRAM_HEADER_LEN * count32(self.segments.len());
        let mut place = |bytes: &[u8]| {
            let offset = body_base + len32(body);
            body.extend_from_slice(bytes);
            (offset, len32(bytes))
        };
        let segment_offsets = self.segments.iter().map(|s| place(&s.2).0).collect();
        let section_offsets = self.sections.iter().map(|s| place(&s.2).0).collect();
        // String tables: section names then symbol names.
        let mut shstrtab = vec![0u8];
        let section_names = self
            .sections
            .iter()
            .map(|s| push_string(&mut shstrtab, s.0))
            .collect();
        let shstrtab_name = push_string(&mut shstrtab, ".shstrtab");
        let strtab_name = push_string(&mut shstrtab, ".strtab");
        let symtab_name = push_string(&mut shstrtab, ".symtab");
        let mut strtab = vec![0u8];
        let mut symtab = vec![0u8; SYMBOL_LEN]; // null symbol
        for (name, value) in &self.symbols {
            u32le(&mut symtab, push_string(&mut strtab, name));
            u32le(&mut symtab, *value);
            u32le(&mut symtab, 0);
            symtab.extend_from_slice(&[0x10, 0, 0xf1, 0xff]); // GLOBAL NOTYPE, SHN_ABS
        }
        let (shstrtab, strtab, symtab) = (place(&shstrtab), place(&strtab), place(&symtab));
        BodyLayout {
            segment_offsets,
            section_offsets,
            section_names,
            shstrtab_name,
            strtab_name,
            symtab_name,
            shstrtab,
            strtab,
            symtab,
            section_header_offset: body_base + len32(body),
        }
    }

    fn header(&self, section_header_offset: u32, user_sections: u32) -> Vec<u8> {
        let mut out = vec![0x7f, b'E', b'L', b'F', 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        u16le(&mut out, 2); // ET_EXEC
        u16le(&mut out, self.machine);
        u32le(&mut out, 1); // EV_CURRENT
        u32le(&mut out, self.entry);
        u32le(&mut out, ELF_HEADER_LEN); // e_phoff
        u32le(&mut out, section_header_offset);
        u32le(&mut out, 1); // e_flags: RVC
        let counts = [
            u16::try_from(ELF_HEADER_LEN).unwrap(),
            u16::try_from(PROGRAM_HEADER_LEN).unwrap(),
            u16::try_from(self.segments.len()).unwrap(),
            u16::try_from(SECTION_HEADER_LEN).unwrap(),
            u16::try_from(user_sections + SYNTHETIC_SECTIONS).unwrap(),
            u16::try_from(user_sections + 1).unwrap(), // e_shstrndx: right after the user sections
        ];
        for v in counts {
            u16le(&mut out, v);
        }
        out
    }

    /// `strtab_index` is the section index of `.strtab`, which `.symtab` links to.
    fn write_section_headers(&self, out: &mut Vec<u8>, layout: &BodyLayout, strtab_index: u32) {
        out.extend_from_slice(&[0u8; SECTION_HEADER_LEN as usize]); // null section
        for (i, s) in self.sections.iter().enumerate() {
            SectionHeader {
                name: layout.section_names[i],
                kind: SHT_PROGBITS,
                addr: s.1,
                offset: layout.section_offsets[i],
                size: len32(&s.2),
                link: 0,
                info: 0,
                entsize: 0,
            }
            .write(out);
        }
        let table = |name, (offset, size), kind, link, info, entsize| SectionHeader {
            name,
            kind,
            addr: 0,
            offset,
            size,
            link,
            info,
            entsize,
        };
        table(layout.shstrtab_name, layout.shstrtab, SHT_STRTAB, 0, 0, 0).write(out);
        table(layout.strtab_name, layout.strtab, SHT_STRTAB, 0, 0, 0).write(out);
        let symbol_len = u32::try_from(SYMBOL_LEN).unwrap();
        table(
            layout.symtab_name,
            layout.symtab,
            SHT_SYMTAB,
            strtab_index,
            1,
            symbol_len,
        )
        .write(out);
    }
}

/// `e_machine` of RISC-V.
const RISCV: u16 = 243;
const CODE_LEN: usize = 0x200;
/// Offset of the `unpack` table inside the code segment of [`rom_elf`].
pub(crate) const UNPACK_TABLE: usize = 0x100;

/// A ROM laid out like the real ones, in miniature:
/// code at `0x4000_0000` (0x200 bytes, zero-filled segment up to 0x300), an unpack table with two
/// entries whose sources are 0x300 and 0x308, a 4-byte hole at 0x304 for `.data_btdm`.
#[must_use]
pub fn rom_elf() -> ElfBuilder {
    let mut code = vec![0u8; CODE_LEN];
    code[0x10..0x14].copy_from_slice(&5u32.to_le_bytes()); // _rom_chip_id
    code[0x14..0x18].copy_from_slice(&3u32.to_le_bytes()); // _rom_eco_version
    let table: [u32; 8] = [
        0x3FCD_F000,
        0x3FCD_F004,
        0x4000_0300,
        0,
        0x3FCD_F010,
        0x3FCD_F014,
        0x4000_0308,
        0,
    ];
    for (i, word) in table.iter().enumerate() {
        code[UNPACK_TABLE + 4 * i..UNPACK_TABLE + 4 * i + 4].copy_from_slice(&word.to_le_bytes());
    }
    ElfBuilder {
        machine: RISCV,
        entry: ROM_BASE,
        segments: vec![
            (0x3FCC_9000, 0x3FCC_A000, b"\x7fELF".to_vec()), // linker artefact: must be ignored
            (ROM_BASE, ROM_BASE, code),
            (0x3FCC_AE00, ROM_BASE + 0x200, vec![0; 0x100]), // zeroed init-data placeholder
        ],
        sections: vec![
            (".data_x", 0x3FCD_F000, vec![1, 2, 3, 4]),
            (".data_y", 0x3FCD_F010, vec![5, 6, 7, 8]),
            (".data_btdm", 0x3FCD_F020, vec![9, 9, 9, 9]),
            (
                ".rodata.interface",
                0x3FF1_FFFC,
                vec![0xAA, 0xBB, 0xCC, 0xDD],
            ),
        ],
        symbols: vec![
            ("_rom_chip_id", ROM_BASE + 0x10),
            ("_rom_eco_version", ROM_BASE + 0x14),
            ("_data_start", ROM_BASE + 0x100),
            ("_data_end", ROM_BASE + 0x120),
            ("_bss_end", ROM_BASE + 0x200),
        ],
    }
}

/// A ROM holding `program` at the reset vector and zeros elsewhere.
#[must_use]
pub fn flat_rom(program: &[u32]) -> RomImage {
    let mut rom = vec![0u8; ROM_LEN];
    for (i, word) in program.iter().enumerate() {
        rom[4 * i..4 * i + 4].copy_from_slice(&word.to_le_bytes());
    }
    RomImage::from_flat(rom).unwrap()
}

/// `j .`: the program parks itself.
pub const HANG: u32 = 0x0000_006f;
/// `t0`, the register [`print_program`] and [`load_register`] address through.
pub const T0: u8 = 5;
/// `t1`, the register that carries the byte or receives the value.
pub const T1: u8 = 6;

/// Code that writes each byte of `text` to the register at `base` (a UART FIFO, for instance).
#[must_use]
pub fn print_program(base: u32, text: &[u8]) -> Vec<u32> {
    let mut program = vec![asm::lui(T0, base)];
    for &byte in text {
        program.extend([asm::addi(T1, 0, i32::from(byte)), asm::sw(T1, T0, 0)]);
    }
    program
}

/// The register every test uses as *the* unmodelled one: the first `LEDC` register with a documented,
/// non-zero reset value. No `SP1b` plan models the `LEDC`.
#[must_use]
pub fn unmodeled_register() -> &'static svd_regs::RegisterInfo {
    svd_regs::esp32c3::registers()
        .iter()
        .find(|r| r.peripheral == "LEDC" && r.reset_explicit && r.reset_value != 0)
        .unwrap()
}

/// Two instructions that load the word at `addr` into [`T1`]: `lui` takes the page (rounded so that the
/// remainder fits a signed 12-bit offset), `lw` the remainder.
#[must_use]
pub fn load_register(addr: u32) -> [u32; 2] {
    let page = addr.wrapping_add(0x800) & 0xFFFF_F000;
    let offset = addr.wrapping_sub(page).cast_signed();
    [asm::lui(T0, page), asm::lw(T1, T0, offset)]
}

/// Stores `value` in the register at `addr`, through `T0` and `T1`.
#[must_use]
pub fn store_register(addr: u32, value: u32) -> [u32; 4] {
    // `addi` and `sw` sign-extend their 12 bits: the upper part is rounded so the sum is the word.
    let split = |word: u32| {
        let upper = word.wrapping_add(0x800) & 0xFFFF_F000;
        (upper, word.wrapping_sub(upper).cast_signed())
    };
    let (page, offset) = split(addr);
    let (upper, lower) = split(value);
    [
        asm::lui(T0, page),
        asm::lui(T1, upper),
        asm::addi(T1, T1, lower),
        asm::sw(T1, T0, offset),
    ]
}

/// What a program does first when it must outlive the flash-boot protection: it clears
/// `WDTCONFIG0` of main watchdog 0 and of the RTC watchdog, as the bootloader does
/// (`bootloader_config_wdt`). Both are unlocked at reset: their key registers reset to the key.
#[must_use]
pub fn stop_watchdogs() -> Vec<u32> {
    let mut program = Vec::new();
    for (peripheral, register) in [("TIMG0", "WDTCONFIG0"), ("RTC_CNTL", "WDTCONFIG0")] {
        program.extend(store_register(
            svd_regs::esp32c3::addr(peripheral, register),
            0,
        ));
    }
    program
}

/// `SPI1.CMD` bits of the hard-wired commands the two helpers below use (report FW §A.1).
const SPI1_CMD_WREN: u32 = 1 << 30;
const SPI1_CMD_PP: u32 = 1 << 25;
const SPI1_CMD_SE: u32 = 1 << 24;

/// Code that programs `data` (1 to 64 bytes) into the flash at `address`, the way the ROM and
/// ESP-IDF do: write enable, the bytes in `W0..`, `ADDR = len << 24 | address`, `CMD = FLASH_PP`
/// (reports FW §A.2, §A.3).
#[must_use]
pub fn flash_program(address: u32, data: &[u8]) -> Vec<u32> {
    assert!((1..=64).contains(&data.len()), "1 to 64 bytes");
    assert!(address < 1 << 24, "a 24-bit address");
    let spi1 = |register| svd_regs::esp32c3::addr("SPI1", register);
    let mut program = store_register(spi1("CMD"), SPI1_CMD_WREN).to_vec();
    for (word, chunk) in (0u32..).zip(data.chunks(4)) {
        let mut le = [0u8; 4];
        le[..chunk.len()].copy_from_slice(chunk);
        program.extend(store_register(
            spi1("W0") + 4 * word,
            u32::from_le_bytes(le),
        ));
    }
    let len = u32::try_from(data.len()).unwrap();
    program.extend(store_register(spi1("ADDR"), (len << 24) | address));
    program.extend(store_register(spi1("CMD"), SPI1_CMD_PP));
    program
}

/// Code that erases the 4 KiB flash sector holding `address`: write enable, `ADDR`,
/// `CMD = FLASH_SE`, as ESP-IDF does (report FW §A.2).
#[must_use]
pub fn flash_erase_sector(address: u32) -> Vec<u32> {
    assert!(address < 1 << 24, "a 24-bit address");
    let spi1 = |register| svd_regs::esp32c3::addr("SPI1", register);
    let mut program = store_register(spi1("CMD"), SPI1_CMD_WREN).to_vec();
    program.extend(store_register(spi1("ADDR"), address));
    program.extend(store_register(spi1("CMD"), SPI1_CMD_SE));
    program
}

/// A fixed linear congruential generator (the constants of Knuth's MMIX): numbers for a test that
/// wants many different inputs and the same ones at every run. No host randomness is involved.
#[derive(Clone, Debug)]
pub struct Lcg(u64);

impl Lcg {
    #[must_use]
    pub const fn new(seed: u64) -> Lcg {
        Lcg(seed)
    }

    /// The next number: the high half of the state, whose low bits have short periods.
    pub fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let [.., a, b, c, d] = self.0.to_le_bytes();
        u32::from_le_bytes([a, b, c, d])
    }

    pub fn next_u8(&mut self) -> u8 {
        self.next_u32().to_le_bytes()[3]
    }

    /// A number below `bound`, which is not 0.
    pub fn below(&mut self, bound: u32) -> u32 {
        self.next_u32() % bound
    }
}

/// Runs `work` on its own thread and waits at most `limit` for its result: a test of a loop that
/// must end, which would otherwise hang the whole suite if the loop did not. On a timeout the
/// test fails with `unbounded`; the thread is left to spin until the test process ends. A panic of
/// `work` is the panic of the caller.
///
/// `work` builds what it needs itself (a machine is not `Send`) and returns plain data.
// Test support, compiled only for tests and the `test-support` feature: the crate itself starts
// no thread (`clippy.toml`), a test of a loop that must end needs one to be able to give up.
#[allow(clippy::disallowed_methods)]
pub fn within<T: Send + 'static>(
    limit: std::time::Duration,
    unbounded: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> T {
    let (sender, receiver) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        // A closed channel means the caller gave up: nobody needs the result.
        let _ = sender.send(work());
    });
    match receiver.recv_timeout(limit) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => panic!("{unbounded}"),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => match thread.join() {
            Err(panic) => std::panic::resume_unwind(panic),
            Ok(()) => panic!("the thread ended without a result"),
        },
    }
}

/// What the consoles of a machine sent since it was driven, and its diagnostics, taken after
/// every call: the one record the tests that drive a machine through its consoles keep.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Consoles {
    sent: [Vec<u8>; 3],
    pub diagnostics: Vec<Diagnostic>,
    pub flash_writes: Vec<FlashWrite>,
}

impl Consoles {
    /// Everything `channel` sent.
    #[must_use]
    pub fn bytes(&self, channel: ConsoleChannel) -> &[u8] {
        &self.sent[channel.index()]
    }

    /// Everything `channel` sent, as text, carriage returns dropped: the ROM and ESP-IDF expand
    /// `\n` to `\r\n` on their consoles.
    #[must_use]
    pub fn text(&self, channel: ConsoleChannel) -> String {
        String::from_utf8_lossy(self.bytes(channel)).replace('\r', "")
    }

    /// Takes what every channel of `machine` sent, its diagnostics and its flash writes, keeps
    /// them, and gives back what the channels sent in this take, in [`ConsoleChannel::ALL`]
    /// order: what a console script reads.
    pub fn take(&mut self, machine: &mut Esp32C3) -> Vec<(ConsoleChannel, Vec<u8>)> {
        self.diagnostics.extend(machine.drain_diagnostics());
        self.flash_writes.extend(machine.take_flash_writes());
        ConsoleChannel::ALL
            .into_iter()
            .map(|channel| {
                let taken = machine.take_console(channel);
                self.sent[channel.index()].extend_from_slice(&taken);
                (channel, taken)
            })
            .collect()
    }
}

/// The bounds of [`drive`]: the CPU cycles of each call, and the cycles and device time of the
/// whole run. Both totals are needed: the cycle counter stands still while the CPU sleeps, so a
/// machine that sleeps for ever never exhausts the first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub slice: u64,
    pub max_cycles: u64,
    pub max_nanos: u64,
}

/// How [`drive`] ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Driven {
    /// The caller's check accepted what the machine did.
    Done,
    /// The machine stopped for this reason, which no further call changes: a strict stop, a
    /// CPU that waits for ever, or a chip in a deep sleep nothing can end.
    Stopped(StopReason),
    /// The cycles or the device time of [`Bounds`] ran out first.
    OutOfBounds,
}

/// Drives `machine` in calls of `bounds.slice` cycles, taking its consoles into `consoles`
/// after each call, until `done` accepts them, the machine stops, or the bounds run out. `done`
/// sees the machine (to feed it input dated from what it sent), what this call took
/// ([`Consoles::take`]) and everything taken so far. A call that ends because a watched console
/// spoke, or because the CPU slept a while, is followed by the next.
///
/// # Panics
/// When `bounds.slice` is 0: no call would move the machine on, and the loop would never end.
pub fn drive(
    machine: &mut Esp32C3,
    bounds: Bounds,
    consoles: &mut Consoles,
    mut done: impl FnMut(&mut Esp32C3, &[(ConsoleChannel, Vec<u8>)], &Consoles) -> bool,
) -> Driven {
    assert!(
        bounds.slice > 0,
        "a slice of 0 cycles moves the machine on by nothing; give drive a slice of 1 cycle or more"
    );
    loop {
        let cycles = machine.cycles().get();
        if cycles >= bounds.max_cycles || machine.device_time_nanos() >= bounds.max_nanos {
            return Driven::OutOfBounds;
        }
        let limit = cycles.saturating_add(bounds.slice).min(bounds.max_cycles);
        let stop = machine.run_within(Cycles(limit), Some(bounds.max_nanos));
        let taken = consoles.take(machine);
        if done(machine, &taken, consoles) {
            return Driven::Done;
        }
        match stop {
            StopReason::CycleBudget
            | StopReason::TimeBudget
            | StopReason::Asleep
            | StopReason::ConsoleOutput => {}
            StopReason::StrictMmio(_) | StopReason::Idle | StopReason::DeepSleep => {
                return Driven::Stopped(stop);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SocConfig;
    use asm::wfi;

    const UART0: u32 = 0x6000_0000;

    fn machine(program: &[u32]) -> Esp32C3 {
        Esp32C3::new(flat_rom(program), &SocConfig::default())
    }

    const WIDE: Bounds = Bounds {
        slice: 1_000,
        max_cycles: 10_000_000,
        max_nanos: 1_000_000_000,
    };

    #[test]
    fn drive_keeps_every_channel_and_ends_when_the_check_accepts() {
        let mut m = machine(&print_program(UART0, b"ok"));
        let mut consoles = Consoles::default();
        let mut calls = 0;
        let end = drive(&mut m, WIDE, &mut consoles, |_, _, seen| {
            calls += 1;
            seen.text(ConsoleChannel::Uart0) == "ok"
        });
        assert_eq!(end, Driven::Done);
        assert_eq!(consoles.bytes(ConsoleChannel::Uart0), b"ok");
        assert_eq!(consoles.bytes(ConsoleChannel::UsbSerialJtag), b"");
        assert!(calls > 1, "the check is called after every call");
    }

    #[test]
    fn drive_ends_on_a_stop_and_when_its_bounds_run_out() {
        // Strapped for another boot mode, no watchdog runs at reset: nothing wakes the CPU, and
        // device time stands still, so a driver that called again would never return.
        let end = within(
            std::time::Duration::from_secs(60),
            "drive called an idle machine again and again",
            || {
                let mut idle = Esp32C3::new(
                    flat_rom(&[wfi(), HANG]),
                    &SocConfig {
                        strapping: 0,
                        ..SocConfig::default()
                    },
                );
                drive(&mut idle, WIDE, &mut Consoles::default(), |_, _, _| false)
            },
        );
        assert_eq!(end, Driven::Stopped(StopReason::Idle));
        // A driver that missed a bound would call a machine that has nothing left to do in its
        // budget again and again: each of these calls must return.
        let (end, cycles) = within(
            std::time::Duration::from_secs(60),
            "drive went on past its bound of cycles",
            || {
                let mut spinning = machine(&[HANG]);
                let tight = Bounds {
                    max_cycles: 5_500,
                    ..WIDE
                };
                let end = drive(&mut spinning, tight, &mut Consoles::default(), |_, _, _| {
                    false
                });
                (end, spinning.cycles().get())
            },
        );
        assert_eq!(end, Driven::OutOfBounds);
        // The last call stops at the bound, not a slice later: the instruction that crosses it
        // completes (a jump is 2 cycles).
        assert!((5_500..5_510).contains(&cycles), "{cycles}");
        // A jump is 100 ns at 20 MHz, so instruction boundaries fall every 100 ns: one of these
        // two bounds is one of them, which the run reaches exactly, and the other is not.
        for max_nanos in [10_000, 10_050] {
            let (end, nanos) = within(
                std::time::Duration::from_secs(60),
                "drive went on past its bound of device time",
                move || {
                    let mut spinning = machine(&[HANG]);
                    let short = Bounds { max_nanos, ..WIDE };
                    let end = drive(&mut spinning, short, &mut Consoles::default(), |_, _, _| {
                        false
                    });
                    (end, spinning.device_time_nanos())
                },
            );
            assert_eq!(end, Driven::OutOfBounds);
            // Ends at the first instruction boundary at or past the bound.
            assert!(
                (max_nanos..max_nanos + 100).contains(&nanos),
                "{nanos} for a bound of {max_nanos}"
            );
        }
    }

    #[test]
    #[should_panic(expected = "a slice of 0 cycles moves the machine on by nothing")]
    fn drive_refuses_a_slice_of_0_cycles() {
        // Without the refusal, each call would end where it started and the loop never would.
        within(
            std::time::Duration::from_secs(60),
            "drive called a machine again and again for 0 cycles",
            || {
                let mut m = machine(&[HANG]);
                let empty = Bounds { slice: 0, ..WIDE };
                drive(&mut m, empty, &mut Consoles::default(), |_, _, _| false)
            },
        );
    }

    #[test]
    fn take_keeps_the_diagnostics_and_the_flash_writes_of_every_take() {
        // A read and a write of an unmodelled register (two diagnostics: they differ in
        // direction), an erase and a program of the flash, then `a\r\nb` on UART0: taken in calls
        // of a few cycles, so that each comes in a take of its own.
        let ledc = unmodeled_register().addr;
        let mut program = load_register(ledc).to_vec();
        program.extend(flash_erase_sector(0x9000));
        program.extend(store_register(ledc, 0));
        program.extend(flash_program(0x9020, &[0xFA]));
        program.extend(print_program(UART0, b"a\r\nb"));
        program.push(HANG);
        let mut m = Esp32C3::with_flash(flat_rom(&program), vec![0; 0xA000], &SocConfig::default())
            .unwrap();
        let mut consoles = Consoles::default();
        let mut takes_with_something = 0;
        // 2 ms at 20 MHz: the four bytes take 350 us on the line at 115 200 baud.
        while m.cycles().get() < 40_000 {
            let next = m.cycles() + 7;
            m.run_until(next);
            let before = (consoles.diagnostics.len(), consoles.flash_writes.len());
            consoles.take(&mut m);
            if (consoles.diagnostics.len(), consoles.flash_writes.len()) != before {
                takes_with_something += 1;
            }
        }
        assert!(
            takes_with_something > 1,
            "they came in {takes_with_something} take: {consoles:?}"
        );
        assert!(
            matches!(
                consoles.diagnostics[..],
                [
                    Diagnostic::UnmodeledMmio { addr: read, .. },
                    Diagnostic::UnmodeledMmio { addr: written, .. },
                ] if read == ledc && written == ledc
            ),
            "{:?}",
            consoles.diagnostics
        );
        assert_eq!(
            consoles.flash_writes,
            [
                FlashWrite::Erase {
                    start: 0x9000,
                    len: 0x1000
                },
                FlashWrite::Program {
                    address: 0x9020,
                    data: vec![0xFA]
                },
            ]
        );
        assert_eq!(consoles.bytes(ConsoleChannel::Uart0), b"a\r\nb");
        assert_eq!(
            consoles.text(ConsoleChannel::Uart0),
            "a\nb",
            "the carriage returns are dropped from the text, not from the bytes"
        );
    }

    #[test]
    fn take_gives_back_what_each_channel_sent_in_this_take() {
        let mut m = machine(&print_program(UART0, b"x"));
        let mut consoles = Consoles::default();
        m.run_until(Cycles(1_000_000));
        let taken = consoles.take(&mut m);
        assert_eq!(
            taken,
            [
                (ConsoleChannel::Uart0, b"x".to_vec()),
                (ConsoleChannel::UsbSerialJtag, Vec::new()),
                (ConsoleChannel::Uart1, Vec::new()),
            ]
        );
        assert_eq!(consoles.take(&mut m)[0].1, b"", "taken once");
        assert_eq!(consoles.bytes(ConsoleChannel::Uart0), b"x");
    }
}
