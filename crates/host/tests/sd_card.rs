//! A blank image made by `host::sd_format`, opened by `host::sd_image`, read and written through
//! the card of `chips` by the bytes a driver sends: what `SdFat` checks before it mounts the volume
//! (research `sp2e-sd-card.md` §2 point 9), and a life's write found by the next run (plan `SP2e`,
//! tasks 9 and 10).
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure
use chips::sdcard::{BLOCK_BYTES, CardKind, Overlay, SdCard, SdCardConfig, SdTiming, crc7_byte};
use emu_core::{Cycles, Frequency};
use host::sd_format::{DEFAULT_LABEL, create_blank};
use host::sd_image::{Access, FileBlocks, SdImage};

/// A host on the bus of a card made of an image, one byte a microsecond.
struct Host {
    card: SdCard<Overlay<FileBlocks>>,
    now: u64,
}

impl Host {
    fn new(image: &SdImage) -> Host {
        let config = SdCardConfig {
            write_protected: image.write_protected(),
            timing: SdTiming {
                initialization_ns: 1_000_000,
                ..SdTiming::default()
            },
            ..SdCardConfig::default()
        };
        let mut card = SdCard::new(
            config,
            Overlay::new(image.source().unwrap()),
            Frequency::from_hz(1_000_000_000).unwrap(),
        )
        .unwrap();
        card.select(Cycles(0), true);
        Host { card, now: 0 }
    }

    fn byte(&mut self, out: u8) -> u8 {
        self.now += 1_000;
        self.card.exchange(Cycles(self.now), out).unwrap_or(0xFF)
    }

    fn command(&mut self, index: u8, argument: u32) -> u8 {
        let [a, b, c, d] = argument.to_be_bytes();
        let first = 0x40 | index;
        for byte in [first, a, b, c, d, crc7_byte(&[first, a, b, c, d])] {
            self.byte(byte);
        }
        (0..10)
            .map(|_| self.byte(0xFF))
            .find(|r1| r1 & 0x80 == 0)
            .unwrap_or(0xFF)
    }

    /// CMD0, CMD8, ACMD41 until ready, CMD58: the kind the OCR tells.
    fn initialize(&mut self) -> CardKind {
        assert_eq!(self.command(0, 0), 0x01);
        assert_eq!(self.command(8, 0x1AA), 0x01);
        (0..4).for_each(|_| {
            self.byte(0xFF);
        });
        while {
            self.command(55, 0);
            self.command(41, 1 << 30)
        } != 0
        {}
        assert_eq!(self.command(58, 0), 0);
        let ocr = self.byte(0xFF);
        (0..3).for_each(|_| {
            self.byte(0xFF);
        });
        if ocr & 0x40 == 0 {
            CardKind::Sdsc
        } else {
            CardKind::Sdhc
        }
    }

    fn address(&self, sector: u32) -> u32 {
        match self.card.capacity().kind() {
            CardKind::Sdhc => sector,
            CardKind::Sdsc => sector * 512,
        }
    }

    fn read(&mut self, sector: u32) -> Vec<u8> {
        let address = self.address(sector);
        assert_eq!(self.command(17, address), 0);
        while self.byte(0xFF) != 0xFE {}
        let data: Vec<u8> = (0..BLOCK_BYTES).map(|_| self.byte(0xFF)).collect();
        self.byte(0xFF);
        self.byte(0xFF);
        data
    }

    fn write(&mut self, sector: u32, data: &[u8; BLOCK_BYTES]) {
        let address = self.address(sector);
        assert_eq!(self.command(24, address), 0);
        self.byte(0xFE);
        for &byte in data {
            self.byte(byte);
        }
        self.byte(0xFF);
        self.byte(0xFF);
        assert_eq!(self.byte(0xFF) & 0x1F, 0x05);
        while self.byte(0xFF) != 0xFF {}
    }
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// What `SdFat`'s `FatPartition::init` checks on partition 1, read through the card.
fn mounts_as_sdfat_would(host: &mut Host) -> u32 {
    let mbr = host.read(0);
    assert_eq!(mbr[510..512], [0x55, 0xAA]);
    let entry = &mbr[446..462];
    assert!(entry[0] == 0x00 || entry[0] == 0x80, "boot flag");
    assert_ne!(entry[4], 0, "a partition type");
    let start = u32_at(entry, 8);
    let boot = host.read(start);
    assert_eq!(u16_at(&boot, 11), 512, "bytes per sector");
    assert!(boot[16] == 1 || boot[16] == 2, "FAT count");
    assert!(boot[13].is_power_of_two(), "sectors per cluster");
    let reserved = u32::from(u16_at(&boot, 14));
    let fat = u32_at(&boot, 36);
    let total = u32_at(&boot, 32);
    let data_start = reserved + u32::from(boot[16]) * fat;
    let clusters = (total - data_start) / u32::from(boot[13]);
    assert!(clusters >= 65_525, "FAT32 by its cluster count: {clusters}");
    start + data_start
}

#[test]
fn a_blank_standard_capacity_image_mounts_through_the_card_and_keeps_a_life_s_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("card.img");
    create_blank(&path, 64, DEFAULT_LABEL).unwrap();
    let mut image = SdImage::open(&path, Access::ReadWrite, &[], &mut Vec::new()).unwrap();
    let mut host = Host::new(&image);
    assert_eq!(host.initialize(), CardKind::Sdsc);
    let root = mounts_as_sdfat_would(&mut host);
    assert_eq!(host.read(root)[..11], *b"INKEMU     ");
    host.write(root + 1, &[0x42; BLOCK_BYTES]);
    let card = host.card;
    assert_eq!(card.storage().changes().written_count(), 1);
    image.commit(&card.storage().changes()).unwrap();
    drop(card);
    drop(image);
    let image = SdImage::open(&path, Access::ReadWrite, &[], &mut Vec::new()).unwrap();
    let mut next = Host::new(&image);
    next.initialize();
    assert_eq!(next.read(root + 1), [0x42; BLOCK_BYTES]);
}

#[test]
fn a_blank_high_capacity_image_mounts_through_the_card() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.img");
    create_blank(&path, 4096, "BIG").unwrap();
    let image = SdImage::open(&path, Access::ReadOnly, &[], &mut Vec::new()).unwrap();
    let mut host = Host::new(&image);
    assert_eq!(host.initialize(), CardKind::Sdhc);
    let root = mounts_as_sdfat_would(&mut host);
    assert_eq!(host.read(root)[..11], *b"BIG        ");
    assert_eq!(host.command(9, 0), 0);
    while host.byte(0xFF) != 0xFE {}
    let csd: Vec<u8> = (0..16).map(|_| host.byte(0xFF)).collect();
    assert_eq!(
        csd[14] & 0x10,
        0x10,
        "TMP_WRITE_PROTECT: the image is read-only"
    );
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 4096 * 1024 * 1024);
}
