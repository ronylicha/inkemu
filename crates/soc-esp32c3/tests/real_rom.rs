//! Tests against the real mask ROMs. They need the ELF files of the pinned esp-rom-elfs archive:
//! `INKEMU_ROM_ELF_DIR=<dir> cargo test -p soc-esp32c3 --test real_rom -- --ignored`
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure

use sha2::{Digest, Sha256};
use soc_esp32c3::rom::RomImage;
use std::fmt::Write as _;

fn elf(revision: u16) -> Vec<u8> {
    let dir = std::env::var("INKEMU_ROM_ELF_DIR")
        .expect("set INKEMU_ROM_ELF_DIR to the directory holding the ROM ELF files");
    let path = std::path::Path::new(&dir).join(format!("esp32c3_rev{revision}_rom.elf"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn image_sha256(revision: u16) -> String {
    let rom = RomImage::from_elf(&elf(revision)).unwrap();
    Sha256::digest(rom.bytes())
        .iter()
        .fold(String::new(), |mut hex, byte| {
            write!(hex, "{byte:02x}").unwrap();
            hex
        })
}

/// rev3 is byte-identical to the reference ROM image shipped with Espressif's QEMU; rev0 and rev101
/// have no reference image and are pinned to what the same recipe produces
/// (`docs/research/esp32c3-rom-elfs.md`, §9).
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn converted_images_match_their_pinned_hashes() {
    assert_eq!(
        image_sha256(3),
        "0de1e65020e803bea0d7443dca149d61895e01fca3bb9c82d073234eebd73f99"
    );
    assert_eq!(
        image_sha256(0),
        "832b9a17b4bca5fe307d2d84b3fcd9cdc75f6a8a51956a3e114b4d6713710565"
    );
    assert_eq!(
        image_sha256(101),
        "8b41b9b114e110e373a58188e6e86b044719f3b4e6da0441c6e8b68b72986651"
    );
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn eco_versions_identify_the_three_revisions() {
    let eco = |rev| RomImage::from_elf(&elf(rev)).unwrap().eco_version();
    assert_eq!((eco(0), eco(3), eco(101)), (0, 3, 7));
}
