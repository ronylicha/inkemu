//! Fixtures for tests of this crate and, behind the `test-support` feature, of its dependants.
// test fixtures: a panic is a test failure
#![allow(clippy::unwrap_used, clippy::missing_panics_doc)]

use crate::memmap::ROM_BASE;
use crate::rom::{ROM_LEN, RomImage};

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
const UNPACK_TABLE: usize = 0x100;

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
