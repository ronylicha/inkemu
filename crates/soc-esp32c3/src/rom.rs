//! Builds the 384 KiB mask-ROM image from an Espressif ROM ELF.
//! Recipe and byte-for-byte validation against a reference image: `docs/research/esp32c3-rom-elfs.md`, §7 and §9.
use crate::memmap::{DROM_BASE, DROM_ROM_OFFSET, DROM_SIZE, ROM_BASE, ROM_SIZE};
use goblin::elf::Elf;
use goblin::elf::header::EM_RISCV;
use goblin::elf::program_header::{PT_LOAD, ProgramHeader};
use goblin::elf::section_header::{SHT_PROGBITS, SectionHeader};
use std::ops::Range;

/// The ROM size as an index bound. `usize` is at least 32 bits wide on every supported target
/// (native and WASM32), so the cast of this compile-time constant cannot truncate.
#[allow(clippy::cast_possible_truncation)]
pub(crate) const ROM_LEN: usize = ROM_SIZE as usize;
const _: () = assert!(usize::BITS >= u32::BITS);

/// `_rom_chip_id` of an ESP32-C3 ROM (`esp32c3.rom.version.ld`; report §5: value 5 at `0x4000_0010`).
const ESP32C3_CHIP_ID: u32 = 5;
/// Where the ROM stores `_rom_eco_version` (report §5: `0x4000_0014`), as an offset into the image.
#[cfg(any(test, feature = "test-support"))]
const ECO_VERSION_OFFSET: usize = 0x14;
/// One `unpack` entry: destination start, destination end, source, reserved.
const UNPACK_ENTRY_BYTES: u64 = 16;
/// The real tables hold 31 to 33 entries; this bounds the loop on a hostile file.
const MAX_UNPACK_TABLE_BYTES: u64 = 0x1000;
const BTDM_DATA_SECTION: &str = ".data_btdm";

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RomError {
    #[error(
        "not an ELF file ({detail}); pass the `esp32c3_rev<N>_rom.elf` file from the esp-rom-elfs archive"
    )]
    NotElf { detail: String },
    #[error(
        "ELF machine {machine} is not 32-bit little-endian RISC-V; use an ESP32-C3 ROM (`esp32c3_rev<N>_rom.elf`), not the ROM of another chip"
    )]
    WrongMachine { machine: u16 },
    #[error(
        "ELF entry point {entry:#010x} is not the reset vector 0x40000000; use an unmodified ESP32-C3 ROM ELF"
    )]
    WrongEntry { entry: u64 },
    #[error(
        "the ROM identifies itself as chip {found}, not 5 (ESP32-C3); use an `esp32c3_rev<N>_rom.elf` file"
    )]
    WrongChip { found: u32 },
    #[error(
        "the ELF has no `{name}` symbol; use an unstripped ROM ELF from the esp-rom-elfs archive"
    )]
    MissingSymbol { name: &'static str },
    #[error(
        "the ELF is truncated: {what} lies beyond the end of the file; download the archive again"
    )]
    Truncated { what: &'static str },
    #[error(
        "unexpected ROM layout: {detail}; this ROM revision needs a review of the conversion recipe"
    )]
    UnexpectedLayout { detail: String },
    #[error(
        "a flat ROM image must be exactly 393216 bytes, got {len}; pass a complete 384 KiB dump"
    )]
    WrongSize { len: usize },
}

/// The mask ROM of an ESP32-C3: exactly [`ROM_SIZE`] bytes, mapped at [`ROM_BASE`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RomImage {
    bytes: Box<[u8]>,
    eco_version: u32,
}

/// Offsets inside the ROM of `[addr, addr + len)`, when that range lies entirely in the ROM.
fn rom_range(addr: u64, len: u64) -> Option<Range<usize>> {
    let start = addr.checked_sub(u64::from(ROM_BASE))?;
    let end = start.checked_add(len)?;
    if end > u64::from(ROM_SIZE) {
        return None;
    }
    Some(usize::try_from(start).ok()?..usize::try_from(end).ok()?)
}

struct Layout {
    rom: Vec<u8>,
    /// Bytes provided by the ELF; the rest stays zero (alignment padding, validated in §9 of the report).
    covered: Vec<bool>,
}

impl Layout {
    fn put(&mut self, range: Range<usize>, data: &[u8]) {
        // INVARIANT: every caller gets `range` from `rom_range` with `data.len()` as its length,
        // and both vectors are `ROM_LEN` long.
        self.rom[range.clone()].copy_from_slice(data);
        self.covered[range].fill(true);
    }

    fn word(&self, addr: u64, what: &'static str) -> Result<u32, RomError> {
        let range = rom_range(addr, 4).ok_or_else(|| RomError::UnexpectedLayout {
            detail: format!("{what} at {addr:#010x} is outside the ROM"),
        })?;
        let mut le = [0u8; 4];
        // INVARIANT: `rom_range` returned a 4-byte range inside the `ROM_LEN`-byte image.
        le.copy_from_slice(&self.rom[range]);
        Ok(u32::from_le_bytes(le))
    }
}

/// A parsed ROM ELF together with the bytes it was parsed from.
struct Source<'a> {
    file: &'a [u8],
    elf: Elf<'a>,
}

impl<'a> Source<'a> {
    fn parse(file: &'a [u8]) -> Result<Self, RomError> {
        let elf = Elf::parse(file).map_err(|e| RomError::NotElf {
            detail: e.to_string(),
        })?;
        if elf.is_64 || !elf.little_endian || elf.header.e_machine != EM_RISCV {
            return Err(RomError::WrongMachine {
                machine: elf.header.e_machine,
            });
        }
        if elf.entry != u64::from(ROM_BASE) {
            return Err(RomError::WrongEntry { entry: elf.entry });
        }
        Ok(Self { file, elf })
    }

    fn symbol(&self, name: &'static str) -> Result<u64, RomError> {
        self.elf
            .syms
            .iter()
            .find(|sym| self.elf.strtab.get_at(sym.st_name) == Some(name))
            .map(|sym| sym.st_value)
            .ok_or(RomError::MissingSymbol { name })
    }

    fn loads(&self) -> impl Iterator<Item = &ProgramHeader> {
        self.elf
            .program_headers
            .iter()
            .filter(|ph| ph.p_type == PT_LOAD && ph.p_filesz > 0)
    }

    fn progbits(&self) -> impl Iterator<Item = &SectionHeader> {
        self.elf
            .section_headers
            .iter()
            .filter(|sh| sh.sh_type == SHT_PROGBITS && sh.sh_size > 0)
    }

    fn file_range(&self, offset: u64, len: u64, what: &'static str) -> Result<&'a [u8], RomError> {
        let start = usize::try_from(offset).map_err(|_| RomError::Truncated { what })?;
        let len = usize::try_from(len).map_err(|_| RomError::Truncated { what })?;
        start
            .checked_add(len)
            .and_then(|end| self.file.get(start..end))
            .ok_or(RomError::Truncated { what })
    }

    /// 1. Loadable segments whose LOAD address is in the ROM. This drops the linker artefact that
    ///    carries the ELF header itself (LOAD address in RAM).
    fn place_segments(&self, layout: &mut Layout) -> Result<(), RomError> {
        for ph in self.loads() {
            if let Some(range) = rom_range(ph.p_paddr, ph.p_filesz) {
                layout.put(
                    range,
                    self.file_range(ph.p_offset, ph.p_filesz, "a loadable segment")?,
                );
            }
        }
        Ok(())
    }

    /// 2. Sections in the data-bus window that no segment carries (`.rodata.interface` is not ALLOC).
    fn place_data_bus_sections(&self, layout: &mut Layout) -> Result<(), RomError> {
        let window = u64::from(DROM_BASE)..u64::from(DROM_BASE) + u64::from(DROM_SIZE);
        for sh in self.progbits().filter(|sh| window.contains(&sh.sh_addr)) {
            if self
                .loads()
                .any(|ph| (ph.p_vaddr..ph.p_vaddr + ph.p_filesz).contains(&sh.sh_addr))
            {
                continue;
            }
            let rom_addr = sh.sh_addr - u64::from(DROM_BASE)
                + u64::from(DROM_ROM_OFFSET)
                + u64::from(ROM_BASE);
            let range =
                rom_range(rom_addr, sh.sh_size).ok_or_else(|| RomError::UnexpectedLayout {
                    detail: format!(
                        "section at {:#010x} overruns the data-bus window",
                        sh.sh_addr
                    ),
                })?;
            layout.put(
                range,
                self.file_range(sh.sh_offset, sh.sh_size, "a read-only data section")?,
            );
        }
        Ok(())
    }

    /// 3. Initial values of `.data_*`: `_init` copies them out of the ROM through the `unpack` table,
    ///    but the segments only hold zeros there. The values are in sections at their RAM address.
    fn place_initial_data(&self, layout: &mut Layout) -> Result<(), RomError> {
        let (table, table_end) = (self.symbol("_data_start")?, self.symbol("_data_end")?);
        let table_len = table_end
            .checked_sub(table)
            .filter(|len| *len <= MAX_UNPACK_TABLE_BYTES && len % UNPACK_ENTRY_BYTES == 0);
        let Some(table_len) = table_len else {
            return Err(RomError::UnexpectedLayout {
                detail: format!("unpack table {table:#010x}..{table_end:#010x} is malformed"),
            });
        };
        for entry in (0..table_len / UNPACK_ENTRY_BYTES).map(|i| table + i * UNPACK_ENTRY_BYTES) {
            let dst = u64::from(layout.word(entry, "an unpack entry")?);
            let dst_end = u64::from(layout.word(entry + 4, "an unpack entry")?);
            let src = u64::from(layout.word(entry + 8, "an unpack entry")?);
            let Some(len) = dst_end.checked_sub(dst).filter(|len| *len > 0) else {
                continue;
            };
            let section = self
                .progbits()
                .find(|sh| sh.sh_addr <= dst && dst_end <= sh.sh_addr + sh.sh_size)
                .ok_or_else(|| RomError::UnexpectedLayout {
                    detail: format!(
                        "no section holds the initial data for {dst:#010x}..{dst_end:#010x}"
                    ),
                })?;
            let data = self.file_range(
                section.sh_offset + (dst - section.sh_addr),
                len,
                "an initial-data section",
            )?;
            let range = rom_range(src, len).ok_or_else(|| RomError::UnexpectedLayout {
                detail: format!("unpack source {src:#010x} is outside the ROM"),
            })?;
            layout.put(range, data);
        }
        Ok(())
    }

    /// 4. `.data_btdm` has no unpack entry (the Bluetooth controller copies it itself). In every C3
    ///    revision it is the first range after `_bss_end` that nothing else provides, exactly its size.
    fn place_btdm_data(&self, layout: &mut Layout) -> Result<(), RomError> {
        let Some(btdm) = self
            .progbits()
            .find(|sh| self.elf.shdr_strtab.get_at(sh.sh_name) == Some(BTDM_DATA_SECTION))
        else {
            return Ok(());
        };
        let from = rom_range(self.symbol("_bss_end")?, 0).map_or(ROM_LEN, |r| r.start);
        let start = (from..ROM_LEN)
            .find(|&i| !layout.covered[i])
            .unwrap_or(ROM_LEN);
        let end = (start..ROM_LEN)
            .find(|&i| layout.covered[i])
            .unwrap_or(ROM_LEN);
        if u64::try_from(end - start).ok() != Some(btdm.sh_size) {
            return Err(RomError::UnexpectedLayout {
                detail: format!(
                    "the hole after _bss_end is {} bytes but {BTDM_DATA_SECTION} is {}",
                    end - start,
                    btdm.sh_size
                ),
            });
        }
        layout.put(
            start..end,
            self.file_range(btdm.sh_offset, btdm.sh_size, BTDM_DATA_SECTION)?,
        );
        Ok(())
    }
}

impl RomImage {
    /// Converts an Espressif ROM ELF (`esp32c3_rev<N>_rom.elf`) into the flat mask-ROM image.
    ///
    /// # Errors
    /// [`RomError`] when the file is not an unmodified ESP32-C3 ROM ELF.
    pub fn from_elf(file: &[u8]) -> Result<RomImage, RomError> {
        let source = Source::parse(file)?;
        let mut layout = Layout {
            rom: vec![0; ROM_LEN],
            covered: vec![false; ROM_LEN],
        };
        source.place_segments(&mut layout)?;
        source.place_data_bus_sections(&mut layout)?;
        source.place_initial_data(&mut layout)?;
        source.place_btdm_data(&mut layout)?;
        // 5. Identity.
        let found = layout.word(source.symbol("_rom_chip_id")?, "_rom_chip_id")?;
        if found != ESP32C3_CHIP_ID {
            return Err(RomError::WrongChip { found });
        }
        let eco_version = layout.word(source.symbol("_rom_eco_version")?, "_rom_eco_version")?;
        Ok(RomImage {
            bytes: layout.rom.into_boxed_slice(),
            eco_version,
        })
    }

    /// A raw image, for tests that run a hand-written program as the ROM.
    ///
    /// # Errors
    /// [`RomError::WrongSize`] unless `bytes` is exactly the ROM size.
    #[cfg(any(test, feature = "test-support"))]
    pub fn from_flat(bytes: Vec<u8>) -> Result<RomImage, RomError> {
        if bytes.len() != ROM_LEN {
            return Err(RomError::WrongSize { len: bytes.len() });
        }
        let mut eco = [0u8; 4];
        // INVARIANT: the length check above guarantees `ROM_LEN` bytes, far more than this 4-byte window.
        eco.copy_from_slice(&bytes[ECO_VERSION_OFFSET..ECO_VERSION_OFFSET + 4]);
        Ok(RomImage {
            bytes: bytes.into_boxed_slice(),
            eco_version: u32::from_le_bytes(eco),
        })
    }

    /// The image, exactly [`ROM_SIZE`] bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// `_rom_eco_version`: 0, 3 or 7 for the three published ESP32-C3 ROMs.
    #[must_use]
    pub const fn eco_version(&self) -> u32 {
        self.eco_version
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::rom_elf;

    #[test]
    fn converts_segments_sections_and_initial_data() {
        let rom = RomImage::from_elf(&rom_elf().build()).unwrap();
        let b = rom.bytes();
        assert_eq!(b.len(), 0x6_0000);
        assert_eq!(rom.eco_version(), 3);
        assert_eq!(&b[0x300..0x304], [1, 2, 3, 4], "unpack entry 0");
        assert_eq!(&b[0x308..0x30C], [5, 6, 7, 8], "unpack entry 1");
        assert_eq!(
            &b[0x304..0x308],
            [9, 9, 9, 9],
            ".data_btdm fills the hole between them"
        );
        assert_eq!(
            &b[0x5_FFFC..],
            [0xAA, 0xBB, 0xCC, 0xDD],
            "non-ALLOC section in the DROM window"
        );
        assert!(
            b[0x30C..0x4_0000].iter().all(|&x| x == 0),
            "unprovided bytes stay zero"
        );
    }

    #[test]
    fn rejects_non_riscv_elf() {
        let mut elf = rom_elf();
        elf.machine = 94; // Xtensa: an ESP32-S3 ROM
        let err = RomImage::from_elf(&elf.build()).unwrap_err();
        assert!(
            matches!(err, RomError::WrongMachine { machine: 94 }),
            "{err}"
        );
        assert!(err.to_string().contains("esp32c3_rev"), "{err}");
    }

    #[test]
    fn rejects_wrong_chip_id() {
        let mut elf = rom_elf();
        elf.segments[1].2[0x10] = 13;
        assert!(matches!(
            RomImage::from_elf(&elf.build()),
            Err(RomError::WrongChip { found: 13 })
        ));
    }

    #[test]
    fn rejects_an_entry_point_outside_the_reset_vector() {
        let mut elf = rom_elf();
        elf.entry = 0x4000_0400;
        assert!(matches!(
            RomImage::from_elf(&elf.build()),
            Err(RomError::WrongEntry { .. })
        ));
    }

    #[test]
    fn missing_symbol_is_named() {
        let mut elf = rom_elf();
        elf.symbols.retain(|s| s.0 != "_data_end");
        let err = RomImage::from_elf(&elf.build()).unwrap_err();
        assert!(
            matches!(err, RomError::MissingSymbol { name: "_data_end" }),
            "{err}"
        );
    }

    #[test]
    fn btdm_section_that_does_not_fit_its_hole_is_a_layout_error() {
        let mut elf = rom_elf();
        elf.sections[2].2 = vec![9; 8]; // the hole is 4 bytes
        assert!(matches!(
            RomImage::from_elf(&elf.build()),
            Err(RomError::UnexpectedLayout { .. })
        ));
    }

    #[test]
    fn not_an_elf_is_an_error_with_a_remedy() {
        let err = RomImage::from_elf(b"ESP-ROM, honest").unwrap_err();
        assert!(matches!(err, RomError::NotElf { .. }));
        assert!(err.to_string().contains("esp-rom-elfs"), "{err}");
    }

    #[test]
    fn truncated_elf_is_an_error_not_a_panic() {
        let full = rom_elf().build();
        for cut in [0, 10, 52, 60, full.len() / 2, full.len() - 1] {
            assert!(RomImage::from_elf(&full[..cut]).is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn flat_image_must_be_exactly_the_rom_size() {
        assert!(RomImage::from_flat(vec![0; 0x6_0000]).is_ok());
        assert!(matches!(
            RomImage::from_flat(vec![0; 0x5_FFFF]),
            Err(RomError::WrongSize { len: 0x5_FFFF })
        ));
    }
}
