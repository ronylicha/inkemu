//! The card driven end to end by a host that does what `SdFat` 2.3.1 does on the X4 (research
//! `sp2e-sd-card.md` §2 points 4 to 7; plan `SP2e`, task 8). The driver is written from that
//! description, not copied: the clock of the bus at 400 kHz then 40 MHz, the timeouts of the
//! library counted in device time, CS driven around each access because the card is shared.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure
use chips::sdcard::{
    BLOCK_BYTES, Block, BlockSource, BlockStorage, CardKind, MemoryStorage, Overlay, SdCard,
    SdCardConfig, SdSlot, SdTiming, SparseBlocks,
};
use emu_core::{Cycles, Frequency};

const MS: u64 = 1_000_000;
const MIB: u64 = 1024 * 1024;

/// What failed, as `SdFat` names its errors.
#[derive(Debug, PartialEq, Eq)]
enum Failure {
    Cmd0,
    Cmd8,
    Acmd41,
    Cmd58,
    Command { index: u8, r1: u8 },
    ReadTimeout,
    ReadToken(u8),
    WriteData(u8),
    WriteTimeout,
    Cmd13,
    StopTransmission,
}

/// `SdFat`'s view of the card type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Type {
    Sd1,
    Sd2,
    Sdhc,
}

struct Driver<S: BlockStorage> {
    slot: SdSlot<S>,
    /// Device time, nanoseconds (the card counts at 1 GHz).
    now: u64,
    /// The time a byte takes on the bus.
    byte_ns: u64,
    active: bool,
    kind: Type,
}

impl<S: BlockStorage> Driver<S> {
    fn new(slot: SdSlot<S>) -> Driver<S> {
        Driver {
            slot,
            now: 0,
            byte_ns: 20_000,
            active: false,
            kind: Type::Sd1,
        }
    }

    fn byte(&mut self, out: u8) -> u8 {
        self.now += self.byte_ns;
        // A line no card drives reads FFh: MISO has a pull-up.
        self.slot.exchange(Cycles(self.now), out).unwrap_or(0xFF)
    }

    fn select(&mut self, selected: bool) {
        self.slot.select(Cycles(self.now), selected);
    }

    fn start(&mut self) {
        if !self.active {
            self.active = true;
            self.select(true);
            self.byte(0xFF);
        }
    }

    fn stop(&mut self) {
        if self.active {
            self.active = false;
            self.select(false);
            // Clocked with CS high: no card hears it.
            self.byte(0xFF);
        }
    }

    fn wait_ready(&mut self, ms: u64) -> bool {
        let deadline = self.now + ms * MS;
        loop {
            if self.byte(0xFF) == 0xFF {
                return true;
            }
            if self.now > deadline {
                return false;
            }
        }
    }

    /// A command: wait until ready (but for CMD0 and CMD12), the six bytes with the CRC `SdFat`
    /// sends (95h for CMD0, 87h for anything else), one byte discarded, then up to nine for R1.
    fn command(&mut self, index: u8, argument: u32) -> u8 {
        self.start();
        if index != 0 && index != 12 && !self.wait_ready(300) {
            return 0xFF;
        }
        self.byte(0x40 | index);
        for byte in argument.to_be_bytes() {
            self.byte(byte);
        }
        self.byte(if index == 0 { 0x95 } else { 0x87 });
        self.byte(0xFF);
        let mut r1 = 0xFF;
        for _ in 0..10 {
            r1 = self.byte(0xFF);
            if r1 & 0x80 == 0 {
                break;
            }
        }
        r1
    }

    fn app_command(&mut self, index: u8, argument: u32) -> u8 {
        self.command(55, 0);
        self.command(index, argument)
    }

    fn begin(&mut self) -> Result<Type, Failure> {
        let result = self.initialize();
        self.stop();
        self.byte_ns = 200;
        result
    }

    fn initialize(&mut self) -> Result<Type, Failure> {
        self.byte_ns = 20_000;
        self.select(false);
        for _ in 0..10 {
            self.byte(0xFF);
        }
        self.active = true;
        self.select(true);
        let deadline = self.now + 2_000 * MS;
        while self.command(0, 0) != 0x01 {
            if self.now > deadline {
                return Err(Failure::Cmd0);
            }
        }
        let mut kind;
        loop {
            if self.command(8, 0x1AA) & 0x04 != 0 {
                kind = Type::Sd1;
                break;
            }
            let mut last = 0;
            for _ in 0..4 {
                last = self.byte(0xFF);
            }
            if last == 0xAA {
                kind = Type::Sd2;
                break;
            }
            if self.now > deadline {
                return Err(Failure::Cmd8);
            }
        }
        let hcs = if kind == Type::Sd2 { 1 << 30 } else { 0 };
        while self.app_command(41, hcs) != 0x00 {
            if self.now > deadline {
                return Err(Failure::Acmd41);
            }
        }
        if kind == Type::Sd2 {
            if self.command(58, 0) != 0 {
                return Err(Failure::Cmd58);
            }
            if self.byte(0xFF) & 0xC0 == 0xC0 {
                kind = Type::Sdhc;
            }
            for _ in 0..3 {
                self.byte(0xFF);
            }
        }
        self.kind = kind;
        Ok(kind)
    }

    fn address(&self, sector: u32) -> u32 {
        if self.kind == Type::Sdhc {
            sector
        } else {
            sector << 9
        }
    }

    fn read_data(&mut self) -> Result<Block, Failure> {
        let deadline = self.now + 300 * MS;
        let token = loop {
            let byte = self.byte(0xFF);
            if byte != 0xFF {
                break byte;
            }
            if self.now > deadline {
                return Err(Failure::ReadTimeout);
            }
        };
        if token != 0xFE {
            return Err(Failure::ReadToken(token));
        }
        let mut block = [0u8; BLOCK_BYTES];
        for byte in &mut block {
            *byte = self.byte(0xFF);
        }
        self.byte(0xFF);
        self.byte(0xFF);
        Ok(block)
    }

    /// `readSector` with a shared card: CMD18, one block, CMD12, deselect.
    fn read_sector(&mut self, sector: u32) -> Result<Block, Failure> {
        let address = self.address(sector);
        let r1 = self.command(18, address);
        let result = if r1 == 0 {
            self.read_data()
        } else {
            Err(Failure::Command { index: 18, r1 })
        };
        let stop = self.command(12, 0);
        self.stop();
        let block = result?;
        if stop != 0 {
            return Err(Failure::Command {
                index: 12,
                r1: stop,
            });
        }
        Ok(block)
    }

    fn send_block(&mut self, token: u8, block: &Block) -> Result<(), Failure> {
        self.byte(token);
        for &byte in block {
            self.byte(byte);
        }
        // CRC off: SdFat sends FFFFh.
        self.byte(0xFF);
        self.byte(0xFF);
        let response = self.byte(0xFF);
        if response & 0x1F != 0x05 {
            return Err(Failure::WriteData(response & 0x1F));
        }
        Ok(())
    }

    /// `writeSector` with a shared card: CMD24, the block, the busy wait, CMD13.
    fn write_sector(&mut self, sector: u32, block: &Block) -> Result<(), Failure> {
        let address = self.address(sector);
        let r1 = self.command(24, address);
        if r1 != 0 {
            self.stop();
            return Err(Failure::Command { index: 24, r1 });
        }
        let result = self.send_block(0xFE, block).and_then(|()| {
            if !self.wait_ready(600) {
                return Err(Failure::WriteTimeout);
            }
            if self.command(13, 0) != 0 || self.byte(0xFF) != 0 {
                return Err(Failure::Cmd13);
            }
            Ok(())
        });
        self.stop();
        result
    }

    /// `writeSectors`: CMD25, each block after the card is ready, then the stop token.
    fn write_sectors(&mut self, sector: u32, blocks: &[Block]) -> Result<(), Failure> {
        let address = self.address(sector);
        let r1 = self.command(25, address);
        if r1 != 0 {
            self.stop();
            return Err(Failure::Command { index: 25, r1 });
        }
        for block in blocks {
            if !self.wait_ready(600) {
                self.stop();
                return Err(Failure::WriteTimeout);
            }
            if let Err(failure) = self.send_block(0xFC, block) {
                self.stop();
                return Err(failure);
            }
        }
        if !self.wait_ready(600) {
            self.stop();
            return Err(Failure::StopTransmission);
        }
        self.byte(0xFD);
        self.stop();
        Ok(())
    }
}

fn card<S: BlockStorage>(storage: S, config: SdCardConfig) -> SdSlot<S> {
    let base = Frequency::from_hz(1_000_000_000).unwrap();
    SdSlot::holding(SdCard::new(config, storage, base).unwrap())
}

fn zero_card(bytes: u64) -> SdSlot<Overlay<SparseBlocks>> {
    card(
        Overlay::new(SparseBlocks::new(bytes / 512)),
        SdCardConfig::default(),
    )
}

fn pattern(seed: u8) -> Block {
    let mut block = [0u8; BLOCK_BYTES];
    for (index, byte) in block.iter_mut().enumerate() {
        *byte = seed.wrapping_mul(31).wrapping_add(index.to_le_bytes()[0]);
    }
    block
}

fn reads_back_what_it_writes<S: BlockStorage>(driver: &mut Driver<S>) {
    assert_eq!(driver.read_sector(0).unwrap(), [0; 512]);
    driver.write_sector(5, &pattern(5)).unwrap();
    assert_eq!(driver.read_sector(5).unwrap(), pattern(5));
    let series: Vec<Block> = (10..14).map(pattern).collect();
    driver.write_sectors(10, &series).unwrap();
    for (sector, block) in (10..14).zip(&series) {
        assert_eq!(&driver.read_sector(sector).unwrap(), block);
    }
    assert_eq!(driver.read_sector(14).unwrap(), [0; 512]);
}

#[test]
fn sdfat_initializes_a_standard_capacity_card_and_reads_back_what_it_writes() {
    let mut driver = Driver::new(zero_card(64 * MIB));
    assert_eq!(driver.begin(), Ok(Type::Sd2), "CMD8 answered, CCS 0");
    let initialized = driver.now;
    assert!(
        (50 * MS..60 * MS).contains(&initialized),
        "the 50 ms of the card and the bytes at 400 kHz: {initialized} ns"
    );
    reads_back_what_it_writes(&mut driver);
    let card = driver.slot.card().unwrap();
    assert_eq!(card.capacity().kind(), CardKind::Sdsc);
    assert_eq!(card.storage().changes().written_count(), 5);
}

#[test]
fn sdfat_initializes_a_high_capacity_card_and_reads_back_what_it_writes() {
    let mut driver = Driver::new(zero_card(4 * 1024 * MIB));
    assert_eq!(driver.begin(), Ok(Type::Sdhc));
    reads_back_what_it_writes(&mut driver);
    let mut slot = driver.slot;
    let card = slot.take().unwrap();
    let mut storage = card.into_storage();
    let mut block = [0u8; 512];
    storage.read_block(12, &mut block).unwrap();
    assert_eq!(block, pattern(12));
}

#[test]
fn sdfat_reads_a_card_held_in_memory() {
    let mut storage = MemoryStorage::new(2048);
    storage.write_block(0, &pattern(1)).unwrap();
    let mut driver = Driver::new(card(storage, SdCardConfig::default()));
    driver.begin().unwrap();
    assert_eq!(driver.read_sector(0).unwrap(), pattern(1));
}

#[test]
fn without_a_card_sdfat_gives_up_on_cmd0_after_two_seconds() {
    let mut driver: Driver<MemoryStorage> = Driver::new(SdSlot::empty());
    assert_eq!(driver.begin(), Err(Failure::Cmd0));
    assert!(driver.now >= 2_000 * MS);
    assert!(driver.now < 2_100 * MS);
}

#[test]
fn a_card_slower_to_initialize_than_sdfat_waits_fails_its_acmd41() {
    let config = SdCardConfig {
        timing: SdTiming {
            initialization_ns: 3_000 * MS,
            ..SdTiming::default()
        },
        ..SdCardConfig::default()
    };
    let mut driver = Driver::new(card(Overlay::new(SparseBlocks::new(131_072)), config));
    assert_eq!(driver.begin(), Err(Failure::Acmd41));
}

#[test]
fn a_write_protected_card_fails_sdfat_s_write_and_keeps_the_sector() {
    let config = SdCardConfig {
        write_protected: true,
        ..SdCardConfig::default()
    };
    let mut driver = Driver::new(card(Overlay::new(SparseBlocks::new(131_072)), config));
    driver.begin().unwrap();
    assert_eq!(
        driver.write_sector(3, &pattern(3)),
        Err(Failure::WriteData(0x0D)),
        "data rejected, write error"
    );
    assert_eq!(driver.read_sector(3).unwrap(), [0; 512]);
}
