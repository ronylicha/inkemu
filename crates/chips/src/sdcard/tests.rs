//! The card, driven by the bytes a host sends (plan `SP2e`, 2e-R2): one test per command and per
//! state, and what the card does with bytes no host should send.
use super::*;

const GIGAHERTZ: u64 = 1_000_000_000;
const MS: u64 = 1_000_000;
const US: u64 = 1_000;
/// 4 GiB: a high-capacity card.
const SDHC_BLOCKS: u64 = 8 * 1024 * 1024;
/// 64 KiB: a standard-capacity card small enough to hold in memory.
const SDSC_BLOCKS: usize = 128;

fn time_base() -> Frequency {
    Frequency::from_hz(GIGAHERTZ).unwrap()
}

/// The six bytes of a command with its CRC.
fn frame(index: u8, argument: u32) -> [u8; 6] {
    let [a, b, c, d] = argument.to_be_bytes();
    let first = 0x40 | index;
    [first, a, b, c, d, crc7_byte(&[first, a, b, c, d])]
}

fn numbered(lba: u64) -> Block {
    let mut block = [0u8; BLOCK_BYTES];
    for (index, byte) in block.iter_mut().enumerate() {
        *byte = (lba.to_le_bytes()[0]).wrapping_add(index.to_le_bytes()[0]);
    }
    block
}

/// A host on the bus of one card, the time in nanoseconds (the base is 1 GHz).
struct Bus<S: BlockStorage> {
    card: SdCard<S>,
    now: u64,
}

impl<S: BlockStorage> Bus<S> {
    fn new(storage: S, config: SdCardConfig) -> Bus<S> {
        let mut bus = Bus {
            card: SdCard::new(config, storage, time_base()).unwrap(),
            now: 0,
        };
        bus.select();
        bus
    }

    fn wait(&mut self, nanos: u64) {
        self.now += nanos;
    }

    fn select(&mut self) {
        self.card.select(Cycles(self.now), true);
    }

    fn deselect(&mut self) {
        self.card.select(Cycles(self.now), false);
    }

    fn raw(&mut self, mosi: u8) -> Option<u8> {
        self.card.exchange(Cycles(self.now), mosi)
    }

    /// One byte; a line the card leaves alone reads `FFh`, as with a pull-up.
    fn byte(&mut self, mosi: u8) -> u8 {
        self.raw(mosi).unwrap_or(0xFF)
    }

    fn send(&mut self, bytes: &[u8]) -> Vec<u8> {
        bytes.iter().map(|&byte| self.byte(byte)).collect()
    }

    fn receive(&mut self, count: usize) -> Vec<u8> {
        (0..count).map(|_| self.byte(0xFF)).collect()
    }

    /// Sends a command, checks that the card drives `FFh` during it and on the byte after,
    /// and gives R1, which comes on the second byte after.
    fn cmd(&mut self, index: u8, argument: u32) -> u8 {
        self.cmd_bytes(frame(index, argument))
    }

    fn cmd_bytes(&mut self, bytes: [u8; 6]) -> u8 {
        let during = self.send(&bytes);
        assert!(
            during.iter().all(|&byte| byte == 0xFF),
            "the card drove {during:02x?} during a command"
        );
        assert_eq!(self.byte(0xFF), 0xFF, "the byte after a command is FFh");
        self.byte(0xFF)
    }

    fn acmd(&mut self, index: u8, argument: u32) -> u8 {
        let r1 = self.cmd(55, 0);
        assert_eq!(r1 & !R1_IDLE, 0, "CMD55 refused: {r1:#04x}");
        self.cmd(index, argument)
    }

    /// CMD0, CMD8, ACMD41 until ready, as `SdFat` does.
    fn initialize(&mut self) {
        assert_eq!(self.cmd(0, 0), 0x01);
        assert_eq!(self.cmd(8, 0x1AA), 0x01);
        assert_eq!(self.receive(4), [0x00, 0x00, 0x01, 0xAA]);
        let mut tries = 0;
        while self.acmd(41, 1 << 30) != 0x00 {
            self.wait(MS);
            tries += 1;
            assert!(tries < 100, "never ready");
        }
        self.card.take_diagnostics();
    }

    /// Waits for the start token of a data block, then gives its data and checks its CRC16.
    fn data_block(&mut self, len: usize) -> Vec<u8> {
        let mut polls = 0;
        loop {
            let byte = self.byte(0xFF);
            if byte == START_BLOCK {
                break;
            }
            assert_eq!(byte, 0xFF, "a byte before the token");
            self.wait(US);
            polls += 1;
            assert!(polls < 10_000, "no token");
        }
        let data = self.receive(len);
        let crc = self.receive(2);
        assert_eq!(u16::from_be_bytes([crc[0], crc[1]]), crc16(&data));
        data
    }

    /// Bytes until one that is not `FFh`.
    fn first_not_ff(&mut self) -> u8 {
        for _ in 0..10_000 {
            let byte = self.byte(0xFF);
            if byte != 0xFF {
                return byte;
            }
            self.wait(US);
        }
        panic!("only FFh");
    }

    /// Sends a block to write after its token, and gives the data response.
    fn write_block(&mut self, token: u8, block: &Block) -> u8 {
        assert_eq!(self.byte(token), 0xFF);
        self.send(block);
        let crc = crc16(block).to_be_bytes();
        self.send(&crc);
        self.byte(0xFF)
    }

    /// Waits until the card is no longer busy, and says how long it was.
    fn wait_ready(&mut self) -> u64 {
        let start = self.now;
        while self.byte(0xFF) != 0xFF {
            self.wait(US);
        }
        self.now - start
    }

    fn kinds(&mut self) -> Vec<SdDiagnosticKind> {
        self.card
            .take_diagnostics()
            .into_iter()
            .map(|diagnostic| diagnostic.kind)
            .collect()
    }
}

fn sdhc() -> Bus<Overlay<SparseBlocks>> {
    let mut source = SparseBlocks::new(SDHC_BLOCKS);
    for lba in [0, 1, 2, 3, SDHC_BLOCKS - 1] {
        source.insert(lba, numbered(lba)).unwrap();
    }
    Bus::new(Overlay::new(source), SdCardConfig::default())
}

fn sdsc_storage() -> MemoryStorage {
    let mut storage = MemoryStorage::new(SDSC_BLOCKS);
    for lba in 0..SDSC_BLOCKS as u64 {
        storage.write_block(lba, &numbered(lba)).unwrap();
    }
    storage
}

fn sdsc() -> Bus<MemoryStorage> {
    Bus::new(sdsc_storage(), SdCardConfig::default())
}

fn ready_sdhc() -> Bus<Overlay<SparseBlocks>> {
    let mut bus = sdhc();
    bus.initialize();
    bus
}

fn ready_sdsc() -> Bus<MemoryStorage> {
    let mut bus = sdsc();
    bus.initialize();
    bus
}

// -------------------------------------------------------------------------------------------
// SD mode and CMD0

#[test]
fn before_cmd0_the_card_drives_nothing_and_hears_only_cmd0_with_its_crc() {
    let mut bus = sdhc();
    assert!(!bus.card.is_in_spi_mode());
    for byte in frame(8, 0x1AA) {
        assert_eq!(bus.raw(byte), None);
    }
    let mut bad = frame(0, 0);
    bad[5] = 0x87;
    for byte in bad {
        assert_eq!(bus.raw(byte), None);
    }
    assert_eq!(bus.raw(0xFF), None);
    assert_eq!(
        bus.kinds(),
        [
            SdDiagnosticKind::CommandInSdMode { index: 8 },
            SdDiagnosticKind::Cmd0CrcInSdMode { crc: 0x87 },
        ]
    );
    assert!(!bus.card.is_in_spi_mode());
}

#[test]
fn cmd0_with_cs_low_enters_spi_mode_and_answers_idle_on_the_second_byte() {
    let mut bus = sdhc();
    for byte in frame(0, 0) {
        assert_eq!(
            bus.raw(byte),
            None,
            "still in SD mode while the command comes"
        );
    }
    assert_eq!(bus.raw(0xFF), Some(0xFF));
    assert_eq!(bus.raw(0xFF), Some(0x01));
    assert_eq!(bus.raw(0xFF), Some(0xFF));
    assert!(bus.card.is_in_spi_mode());
    // In SPI mode, CMD0 resets again; its CRC is not checked with CRC off.
    let mut no_crc = frame(0, 0);
    no_crc[5] = 0x01;
    assert_eq!(bus.cmd_bytes(no_crc), 0x01);
}

#[test]
fn a_deselected_card_drives_nothing() {
    let mut bus = ready_sdhc();
    bus.deselect();
    assert!(!bus.card.is_selected());
    assert_eq!(bus.raw(0xFF), None);
    for byte in frame(13, 0) {
        assert_eq!(bus.raw(byte), None, "a deselected card hears nothing");
    }
    bus.select();
    assert_eq!(
        bus.raw(0xFF),
        Some(0xFF),
        "nothing was heard while deselected"
    );
}

// -------------------------------------------------------------------------------------------
// Initialization

#[test]
fn cmd8_echoes_the_voltage_and_the_check_pattern() {
    let mut bus = sdhc();
    assert_eq!(bus.cmd(0, 0), 0x01);
    assert_eq!(bus.cmd(8, 0x1AA), 0x01);
    assert_eq!(bus.receive(4), [0x00, 0x00, 0x01, 0xAA]);
    assert_eq!(bus.cmd(8, 0x15A), 0x01);
    assert_eq!(bus.receive(4), [0x00, 0x00, 0x01, 0x5A]);
}

#[test]
fn cmd8_with_another_voltage_answers_that_it_is_not_accepted() {
    let mut bus = sdhc();
    assert_eq!(bus.cmd(0, 0), 0x01);
    assert_eq!(bus.cmd(8, 0x2AA), 0x01);
    assert_eq!(bus.receive(4), [0x00, 0x00, 0x00, 0xAA], "VCA 0");
    // Without an accepted CMD8, a high-capacity card cannot initialize.
    assert_eq!(bus.acmd(41, 1 << 30), 0x01);
    bus.wait(100 * MS);
    assert_eq!(bus.acmd(41, 1 << 30), 0x01);
    assert_eq!(
        bus.kinds(),
        [SdDiagnosticKind::HighCapacityWithoutHcs { cmd8: false }]
    );
}

#[test]
fn cmd8_with_a_wrong_crc_is_answered_09h_alone_whatever_the_crc_setting() {
    let mut bus = sdhc();
    assert_eq!(bus.cmd(0, 0), 0x01);
    let mut bad = frame(8, 0x1AA);
    bad[5] = 0x95;
    assert_eq!(bus.cmd_bytes(bad), 0x09);
    assert_eq!(bus.receive(4), [0xFF; 4], "R1 alone");
    assert_eq!(
        bus.kinds(),
        [SdDiagnosticKind::CommandCrc {
            index: 8,
            crc: 0x95,
            expected: 0x87
        }]
    );
}

#[test]
fn acmd41_keeps_the_card_idle_until_the_initialization_time_is_over() {
    let mut bus = sdhc();
    assert_eq!(bus.cmd(0, 0), 0x01);
    assert_eq!(bus.cmd(8, 0x1AA), 0x01);
    bus.receive(4);
    assert_eq!(bus.acmd(41, 1 << 30), 0x01, "the first one starts it");
    let started = bus.now;
    bus.wait(49 * MS);
    assert_eq!(bus.acmd(41, 1 << 30), 0x01);
    bus.now = started + 50 * MS;
    assert_eq!(bus.acmd(41, 1 << 30), 0x00);
    assert_eq!(bus.acmd(41, 0), 0x00, "ready stays ready");
}

#[test]
fn cmd58_says_busy_then_the_capacity_once_initialized() {
    let mut bus = sdhc();
    assert_eq!(bus.cmd(0, 0), 0x01);
    assert_eq!(bus.cmd(58, 0), 0x01);
    assert_eq!(bus.receive(4), [0x00, 0xFF, 0x80, 0x00]);
    bus.initialize();
    assert_eq!(bus.cmd(58, 0), 0x00);
    assert_eq!(
        bus.receive(4),
        [0xC0, 0xFF, 0x80, 0x00],
        "CCS: high capacity"
    );
    let mut sdsc = ready_sdsc();
    assert_eq!(sdsc.cmd(58, 0), 0x00);
    assert_eq!(
        sdsc.receive(4),
        [0x80, 0xFF, 0x80, 0x00],
        "CCS 0: standard capacity"
    );
}

#[test]
fn a_high_capacity_card_asked_with_hcs_0_stays_idle_until_cmd0() {
    let mut bus = sdhc();
    assert_eq!(bus.cmd(0, 0), 0x01);
    assert_eq!(bus.cmd(8, 0x1AA), 0x01);
    bus.receive(4);
    assert_eq!(bus.acmd(41, 0), 0x01);
    bus.wait(200 * MS);
    assert_eq!(
        bus.acmd(41, 1 << 30),
        0x01,
        "HCS is read at the first ACMD41 only"
    );
    assert_eq!(
        bus.kinds(),
        [SdDiagnosticKind::HighCapacityWithoutHcs { cmd8: true }]
    );
    bus.initialize();
}

#[test]
fn a_standard_capacity_card_initializes_without_cmd8_and_ignores_hcs() {
    let mut bus = sdsc();
    assert_eq!(bus.cmd(0, 0), 0x01);
    assert_eq!(bus.acmd(41, 0), 0x01);
    bus.wait(50 * MS);
    assert_eq!(bus.acmd(41, 0), 0x00);
    assert_eq!(bus.kinds(), []);
}

#[test]
fn the_idle_state_takes_only_its_commands() {
    let mut bus = sdhc();
    assert_eq!(bus.cmd(0, 0), 0x01);
    assert_eq!(bus.cmd(17, 0), 0x05);
    assert_eq!(bus.cmd(13, 0), 0x05);
    assert_eq!(bus.acmd(51, 0), 0x05);
    assert_eq!(bus.cmd(59, 0), 0x01);
    assert_eq!(
        bus.kinds(),
        [
            SdDiagnosticKind::IllegalInIdle {
                index: 17,
                app: false
            },
            SdDiagnosticKind::IllegalInIdle {
                index: 13,
                app: false
            },
            SdDiagnosticKind::IllegalInIdle {
                index: 51,
                app: true
            },
        ]
    );
}

#[test]
fn cmd1_is_illegal_before_any_acmd41_then_initializes_like_it() {
    let mut bus = sdsc();
    assert_eq!(bus.cmd(0, 0), 0x01);
    assert_eq!(bus.cmd(1, 0), 0x05);
    assert_eq!(bus.kinds(), [SdDiagnosticKind::Cmd1BeforeAcmd41]);
    bus.initialize();
    assert_eq!(bus.cmd(0, 0), 0x01);
    assert_eq!(
        bus.cmd(1, 0),
        0x01,
        "accepted once an ACMD41 came since power-on"
    );
    bus.wait(50 * MS);
    assert_eq!(bus.cmd(1, 0), 0x00);
}

#[test]
fn an_undefined_application_command_is_the_regular_command_of_its_number() {
    let mut bus = ready_sdhc();
    assert_eq!(bus.acmd(17, 2), 0x00, "ACMD17 does not exist: CMD17");
    assert_eq!(bus.data_block(512), numbered(2));
    assert_eq!(
        bus.acmd(41, 1 << 30),
        0x00,
        "a defined one is the application command"
    );
}

#[test]
fn cmd59_turns_the_crc_check_on_for_commands() {
    let mut bus = ready_sdsc();
    assert_eq!(bus.cmd(59, 1), 0x00);
    let mut bad = frame(16, 16);
    bad[5] ^= 0x02;
    assert_eq!(bus.cmd_bytes(bad), 0x08);
    assert_eq!(
        bus.cmd(17, 0),
        0x00,
        "CMD16 was not executed: a whole block"
    );
    assert_eq!(bus.data_block(512), numbered(0));
    assert_eq!(bus.cmd(59, 0), 0x00);
    assert_eq!(bus.cmd_bytes(bad), 0x00, "CRC off again");
    assert_eq!(
        bus.kinds(),
        [SdDiagnosticKind::CommandCrc {
            index: 16,
            crc: bad[5],
            expected: frame(16, 16)[5]
        }]
    );
}

// -------------------------------------------------------------------------------------------
// Registers and status

#[test]
fn cmd9_and_cmd10_send_the_csd_and_the_cid_as_data_blocks() {
    let mut bus = ready_sdhc();
    assert_eq!(bus.cmd(9, 0), 0x00);
    let csd = bus.data_block(16);
    assert_eq!(csd, bus.card.capacity().csd(false));
    assert_eq!(
        Capacity::told_by(&csd.try_into().unwrap()),
        SDHC_BLOCKS * 512
    );
    assert_eq!(bus.cmd(10, 0), 0x00);
    assert_eq!(bus.data_block(16), Cid::default().register());
}

#[test]
fn cmd13_reports_the_errors_once_and_clears_them() {
    let config = SdCardConfig {
        write_protected: true,
        ..SdCardConfig::default()
    };
    let mut bus = Bus::new(sdsc_storage(), config);
    bus.initialize();
    assert_eq!(bus.cmd(13, 0), 0x00);
    assert_eq!(bus.byte(0xFF), 0x00);
    assert_eq!(bus.cmd(24, 0), 0x00);
    assert_eq!(bus.write_block(START_BLOCK, &[7; 512]), DATA_WRITE_ERROR);
    assert_eq!(bus.cmd(13, 0), 0x00);
    assert_eq!(bus.byte(0xFF), R2_WP_VIOLATION);
    assert_eq!(bus.cmd(13, 0), 0x00);
    assert_eq!(bus.byte(0xFF), 0x00, "cleared by the read");
}

#[test]
fn cmd6_offers_the_default_function_and_refuses_high_speed() {
    let mut bus = ready_sdhc();
    assert_eq!(bus.cmd(6, 0x80FF_FFF1), 0x00);
    let status = bus.data_block(64);
    assert_eq!(register_bits(&status, 379, 376), 0xF);
    assert_eq!(register_bits(&status, 415, 400), 0x8001);
    assert_eq!(bus.cmd(6, 0x00FF_FFF0), 0x00);
    assert_eq!(register_bits(&bus.data_block(64), 379, 376), 0);
}

#[test]
fn acmd13_acmd51_acmd23_and_acmd42_answer_as_the_card_tells() {
    let mut bus = ready_sdhc();
    assert_eq!(bus.acmd(13, 0), 0x00);
    assert_eq!(bus.byte(0xFF), 0x00, "the second byte of R2");
    assert_eq!(bus.data_block(64), [0; 64]);
    assert_eq!(bus.acmd(51, 0), 0x00);
    assert_eq!(bus.data_block(8), SCR);
    assert_eq!(bus.acmd(23, 8), 0x00);
    assert_eq!(bus.acmd(42, 0), 0x00);
    assert_eq!(bus.kinds(), []);
}

#[test]
fn commands_the_model_does_not_carry_out_are_illegal_and_named() {
    let mut bus = ready_sdhc();
    for index in [27, 28, 29, 30, 42, 56] {
        assert_eq!(bus.cmd(index, 0), 0x04, "CMD{index}");
    }
    assert_eq!(bus.acmd(18, 0), 0x04);
    for index in [2, 3, 7, 60, 63] {
        assert_eq!(bus.cmd(index, 0), 0x04, "CMD{index}");
    }
    let kinds = bus.kinds();
    assert_eq!(
        kinds[4],
        SdDiagnosticKind::UnsupportedCommand {
            index: 42,
            app: false,
            name: "LOCK_UNLOCK"
        }
    );
    assert_eq!(
        kinds[6],
        SdDiagnosticKind::UnsupportedCommand {
            index: 18,
            app: true,
            name: "security command"
        }
    );
    assert_eq!(
        kinds[7..],
        [2, 3, 7, 60, 63].map(|index| SdDiagnosticKind::UnknownCommand { index, app: false })
    );
}

// -------------------------------------------------------------------------------------------
// Reads

#[test]
fn cmd17_on_a_high_capacity_card_sends_the_block_after_the_access_time() {
    let mut bus = ready_sdhc();
    assert_eq!(bus.cmd(17, 3), 0x00);
    let asked = bus.now;
    bus.now = asked + 249 * US;
    assert_eq!(bus.byte(0xFF), 0xFF, "not before the access time");
    bus.now = asked + 250 * US;
    assert_eq!(bus.byte(0xFF), START_BLOCK);
    let data = bus.receive(512);
    assert_eq!(data, numbered(3));
    assert_eq!(bus.receive(2), crc16(&data).to_be_bytes());
    assert_eq!(bus.byte(0xFF), 0xFF, "a single read ends after its CRC");
    assert_eq!(bus.cmd(13, 0), 0x00, "the card takes commands again");
}

#[test]
fn a_high_capacity_card_reads_its_last_block_and_refuses_the_next() {
    let mut bus = ready_sdhc();
    let last = u32::try_from(SDHC_BLOCKS - 1).unwrap();
    assert_eq!(bus.cmd(17, last), 0x00);
    assert_eq!(bus.data_block(512), numbered(SDHC_BLOCKS - 1));
    assert_eq!(bus.cmd(17, last + 1), R1_PARAMETER);
    assert_eq!(
        bus.kinds(),
        [SdDiagnosticKind::AddressOutOfRange { address: last + 1 }]
    );
}

#[test]
fn a_standard_capacity_card_reads_a_partial_block_at_a_byte_address() {
    let mut bus = ready_sdsc();
    assert_eq!(bus.cmd(16, 16), 0x00);
    assert_eq!(bus.cmd(17, 512 * 5 + 100), 0x00);
    assert_eq!(bus.data_block(16), numbered(5)[100..116]);
    assert_eq!(bus.cmd(17, 512 * 5 + 500), R1_ADDRESS, "across a block");
    assert_eq!(bus.cmd(16, 0), R1_PARAMETER);
    assert_eq!(bus.cmd(16, 513), R1_PARAMETER);
    assert_eq!(bus.cmd(16, 512), 0x00);
    assert_eq!(bus.cmd(17, 512 * 5 + 1), R1_ADDRESS);
    assert_eq!(bus.cmd(17, 128 * 512), R1_PARAMETER, "past the end");
    assert_eq!(
        bus.kinds(),
        [
            SdDiagnosticKind::MisalignedAddress {
                address: 512 * 5 + 500,
                length: 16
            },
            SdDiagnosticKind::BlockLength { length: 0 },
            SdDiagnosticKind::BlockLength { length: 513 },
            SdDiagnosticKind::MisalignedAddress {
                address: 512 * 5 + 1,
                length: 512
            },
            SdDiagnosticKind::AddressOutOfRange { address: 128 * 512 },
        ]
    );
}

#[test]
fn a_high_capacity_card_ignores_the_block_length_for_its_reads() {
    let mut bus = ready_sdhc();
    assert_eq!(bus.cmd(16, 16), 0x00);
    assert_eq!(bus.cmd(17, 1), 0x00);
    assert_eq!(bus.data_block(512), numbered(1));
}

#[test]
fn cmd18_streams_blocks_until_cmd12_whose_answer_comes_on_the_second_byte() {
    let mut bus = ready_sdhc();
    assert_eq!(bus.cmd(18, 1), 0x00);
    assert_eq!(bus.data_block(512), numbered(1));
    assert_eq!(bus.data_block(512), numbered(2));
    let third_asked = bus.now;
    assert_eq!(bus.data_block(512), numbered(3));
    assert!(
        bus.now >= third_asked + 250 * US,
        "each block after the access time"
    );
    // CMD12, sent while the card waits before the next block: the card hears it.
    let during = bus.send(&frame(12, 0));
    assert!(during.iter().all(|&byte| byte == 0xFF));
    assert_eq!(bus.byte(0xFF), 0xFF, "the stuff byte");
    assert_eq!(bus.byte(0xFF), 0x00);
    bus.wait(MS);
    assert_eq!(bus.byte(0xFF), 0xFF, "no block after CMD12");
    assert_eq!(bus.cmd(13, 0), 0x00);
}

#[test]
fn cmd12_is_heard_in_the_middle_of_a_block() {
    let mut bus = ready_sdhc();
    assert_eq!(bus.cmd(18, 0), 0x00);
    assert_eq!(bus.first_not_ff(), START_BLOCK);
    let half = bus.receive(100);
    assert_eq!(half, numbered(0)[..100]);
    let during = bus.send(&frame(12, 0));
    assert_eq!(
        during,
        numbered(0)[100..106],
        "the card goes on sending meanwhile"
    );
    assert_eq!(bus.byte(0xFF), 0xFF);
    assert_eq!(bus.byte(0xFF), 0x00);
    assert_eq!(bus.receive(3), [0xFF; 3]);
    assert_eq!(bus.kinds(), []);
}

#[test]
fn a_multiple_read_past_the_last_block_sends_the_out_of_range_token() {
    let mut bus = ready_sdhc();
    let last = u32::try_from(SDHC_BLOCKS - 1).unwrap();
    assert_eq!(bus.cmd(18, last), 0x00);
    assert_eq!(bus.data_block(512), numbered(SDHC_BLOCKS - 1));
    assert_eq!(bus.first_not_ff(), ERROR_TOKEN_OUT_OF_RANGE);
    bus.wait(MS);
    assert_eq!(bus.receive(4), [0xFF; 4], "nothing more until CMD12");
    assert_eq!(bus.cmd(12, 0), 0x00);
    assert_eq!(bus.cmd(13, 0), 0x00);
    assert_eq!(bus.byte(0xFF), R2_OUT_OF_RANGE);
    assert_eq!(
        bus.kinds(),
        [SdDiagnosticKind::ReadPastEnd { block: SDHC_BLOCKS }]
    );
}

#[test]
fn a_command_other_than_cmd12_during_a_multiple_read_is_refused_and_the_stream_goes_on() {
    let mut bus = ready_sdhc();
    assert_eq!(bus.cmd(18, 0), 0x00);
    assert_eq!(bus.data_block(512), numbered(0));
    bus.send(&frame(17, 9));
    assert_eq!(bus.data_block(512), numbered(1));
    assert_eq!(
        bus.kinds(),
        [SdDiagnosticKind::CommandDuringTransfer { index: 17 }]
    );
    bus.send(&frame(0, 0));
    assert_eq!(bus.byte(0xFF), 0xFF);
    assert_eq!(bus.byte(0xFF), 0x01, "CMD0 resets from any state");
}

/// A storage whose block 2 cannot be read or written.
#[derive(Debug)]
struct Failing(MemoryStorage);

impl BlockSource for Failing {
    fn block_count(&self) -> u64 {
        self.0.block_count()
    }

    fn read_block(&mut self, lba: u64, into: &mut Block) -> Result<(), StorageError> {
        if lba == 2 {
            return Err(StorageError::Failed {
                action: "read",
                lba,
                detail: "Input/output error".into(),
            });
        }
        self.0.read_block(lba, into)
    }
}

impl BlockStorage for Failing {
    fn write_block(&mut self, lba: u64, data: &Block) -> Result<(), StorageError> {
        if lba == 2 {
            return Err(StorageError::Failed {
                action: "write",
                lba,
                detail: "No space left on device".into(),
            });
        }
        self.0.write_block(lba, data)
    }

    fn erase(&mut self, first: u64, count: u64) -> Result<(), StorageError> {
        self.0.erase(first, count)
    }
}

#[test]
fn a_storage_failure_is_a_data_error_never_a_panic() {
    let mut bus = Bus::new(Failing(sdsc_storage()), SdCardConfig::default());
    bus.initialize();
    assert_eq!(bus.cmd(17, 2 * 512), 0x00);
    assert_eq!(bus.first_not_ff(), ERROR_TOKEN_ERROR);
    assert_eq!(bus.cmd(24, 2 * 512), 0x00);
    assert_eq!(bus.write_block(START_BLOCK, &[1; 512]), DATA_WRITE_ERROR);
    assert_eq!(bus.cmd(13, 0), 0x00);
    assert_eq!(bus.byte(0xFF), R2_ERROR);
    assert_eq!(
        bus.kinds(),
        [
            SdDiagnosticKind::StorageFailed {
                block: 2,
                detail: "cannot read block 2: Input/output error".into()
            },
            SdDiagnosticKind::StorageFailed {
                block: 2,
                detail: "cannot write block 2: No space left on device".into()
            },
        ]
    );
}

// -------------------------------------------------------------------------------------------
// Writes and erase

#[test]
fn cmd24_answers_the_data_response_after_the_crc_then_is_busy_while_it_programs() {
    let mut bus = ready_sdhc();
    assert_eq!(bus.cmd(24, 7), 0x00);
    assert_eq!(bus.write_block(START_BLOCK, &[0x5A; 512]), DATA_ACCEPTED);
    assert_eq!(DATA_ACCEPTED & 0x1F, 0x05, "SdFat's mask (`DATA_RES_MASK`)");
    assert_eq!(bus.byte(0xFF), 0x00);
    assert!(bus.card.is_busy(Cycles(bus.now)));
    let busy = bus.wait_ready();
    assert!((999 * US..=1_001 * US).contains(&busy), "busy {busy} ns");
    assert_eq!(bus.cmd(13, 0), 0x00);
    assert_eq!(bus.byte(0xFF), 0x00);
    assert_eq!(bus.cmd(17, 7), 0x00);
    assert_eq!(bus.data_block(512), [0x5A; 512]);
    assert_eq!(bus.card.storage().changes().written_count(), 1);
}

#[test]
fn a_written_block_with_a_wrong_crc_is_rejected_when_the_crc_is_on() {
    let mut bus = ready_sdsc();
    assert_eq!(bus.cmd(59, 1), 0x00);
    assert_eq!(bus.cmd(24, 0), 0x00);
    assert_eq!(bus.byte(START_BLOCK), 0xFF);
    bus.send(&[9; 512]);
    bus.send(&[0xFF, 0xFF]);
    assert_eq!(bus.byte(0xFF), DATA_CRC_ERROR);
    assert_eq!(bus.byte(0xFF), 0xFF, "not busy: nothing programmed");
    assert_eq!(bus.card.storage().bytes()[..512], numbered(0));
    assert_eq!(
        bus.kinds(),
        [SdDiagnosticKind::DataCrc {
            block: 0,
            crc: 0xFFFF,
            expected: crc16(&[9; 512])
        }]
    );
    // With the CRC off, SdFat's FFFFh is fine.
    assert_eq!(bus.cmd(59, 0), 0x00);
    assert_eq!(bus.cmd(24, 0), 0x00);
    assert_eq!(bus.byte(START_BLOCK), 0xFF);
    bus.send(&[9; 512]);
    bus.send(&[0xFF, 0xFF]);
    assert_eq!(bus.byte(0xFF), DATA_ACCEPTED);
}

#[test]
fn cmd25_writes_blocks_until_the_stop_token_and_acmd22_counts_them() {
    let mut bus = ready_sdhc();
    assert_eq!(bus.cmd(25, 100), 0x00);
    for value in 1..=3u8 {
        assert!(bus.wait_ready() < 2 * MS);
        assert_eq!(
            bus.write_block(START_MULTIPLE_WRITE, &[value; 512]),
            DATA_ACCEPTED
        );
    }
    bus.wait_ready();
    assert_eq!(bus.byte(STOP_TRANSMISSION), 0xFF);
    assert_eq!(bus.cmd(13, 0), 0x00);
    bus.byte(0xFF);
    assert_eq!(bus.acmd(22, 0), 0x00);
    assert_eq!(bus.data_block(4), [0, 0, 0, 3]);
    assert_eq!(bus.cmd(18, 100), 0x00);
    for value in 1..=3u8 {
        assert_eq!(bus.data_block(512), [value; 512]);
    }
    assert_eq!(bus.cmd(12, 0), 0x00);
}

#[test]
fn a_write_to_a_standard_capacity_card_needs_a_whole_aligned_block() {
    let mut bus = ready_sdsc();
    assert_eq!(bus.cmd(24, 100), R1_ADDRESS);
    assert_eq!(bus.cmd(16, 16), 0x00);
    assert_eq!(bus.cmd(24, 512), R1_PARAMETER);
    assert_eq!(bus.cmd(16, 512), 0x00);
    assert_eq!(bus.cmd(24, 127 * 512), 0x00);
    assert_eq!(bus.write_block(START_BLOCK, &[3; 512]), DATA_ACCEPTED);
    assert_eq!(bus.card.storage().bytes()[127 * 512..], [3; 512]);
    assert!(bus.wait_ready() > 0);
    assert_eq!(bus.cmd(24, 128 * 512), R1_PARAMETER);
}

#[test]
fn a_write_protected_card_says_so_and_keeps_its_blocks() {
    let config = SdCardConfig {
        write_protected: true,
        ..SdCardConfig::default()
    };
    let mut bus = Bus::new(sdsc_storage(), config);
    bus.initialize();
    assert_eq!(bus.cmd(9, 0), 0x00);
    let csd = bus.data_block(16);
    assert_eq!(register_bits(&csd, 12, 12), 1);
    assert_eq!(bus.cmd(25, 0), 0x00);
    assert_eq!(
        bus.write_block(START_MULTIPLE_WRITE, &[1; 512]),
        DATA_WRITE_ERROR
    );
    assert_eq!(bus.byte(STOP_TRANSMISSION), 0xFF);
    assert_eq!(bus.card.storage().bytes()[..512], numbered(0));
    assert_eq!(bus.cmd(32, 0), 0x00);
    assert_eq!(bus.cmd(33, 512), 0x00);
    assert_eq!(bus.cmd(38, 0), 0x00);
    assert_eq!(bus.cmd(13, 0), 0x00);
    assert_eq!(bus.byte(0xFF), R2_WP_VIOLATION | R2_WP_ERASE_SKIP);
    assert_eq!(bus.card.storage().bytes()[512..1024], numbered(1));
    assert_eq!(
        bus.kinds(),
        [
            SdDiagnosticKind::WriteProtected { block: 0 },
            SdDiagnosticKind::EraseProtected
        ]
    );
}

#[test]
fn cmd32_cmd33_and_cmd38_erase_the_range_to_zeros_then_the_card_is_busy() {
    let mut bus = ready_sdhc();
    assert_eq!(bus.cmd(32, 1), 0x00);
    assert_eq!(bus.cmd(33, 2), 0x00);
    assert_eq!(bus.cmd(38, 0), 0x00);
    assert_eq!(bus.byte(0xFF), 0x00, "busy erasing");
    let busy = bus.wait_ready();
    assert!((1_999 * US..=2_001 * US).contains(&busy), "busy {busy} ns");
    assert_eq!(bus.cmd(18, 0), 0x00);
    assert_eq!(bus.data_block(512), numbered(0));
    assert_eq!(bus.data_block(512), [0; 512]);
    assert_eq!(bus.data_block(512), [0; 512]);
    assert_eq!(bus.data_block(512), numbered(3));
    assert_eq!(bus.cmd(12, 0), 0x00);
    assert_eq!(
        bus.card.storage().changes().erased().collect::<Vec<_>>(),
        [(1, 2)]
    );
}

#[test]
fn an_erase_out_of_sequence_is_refused_and_another_command_resets_it() {
    let mut bus = ready_sdhc();
    assert_eq!(bus.cmd(38, 0), R1_ERASE_SEQUENCE);
    assert_eq!(bus.cmd(33, 4), R1_ERASE_SEQUENCE, "CMD33 before CMD32");
    assert_eq!(bus.cmd(32, 1), 0x00);
    assert_eq!(
        bus.cmd(16, 512),
        R1_ERASE_RESET,
        "out of the erase sequence"
    );
    assert_eq!(
        bus.cmd(38, 0),
        R1_ERASE_SEQUENCE,
        "the sequence was cleared"
    );
    assert_eq!(
        bus.cmd(32, u32::try_from(SDHC_BLOCKS).unwrap()),
        R1_PARAMETER
    );
    assert_eq!(
        bus.kinds(),
        [
            SdDiagnosticKind::EraseSequence,
            SdDiagnosticKind::EraseSequence,
            SdDiagnosticKind::EraseSequence,
            SdDiagnosticKind::AddressOutOfRange {
                address: u32::try_from(SDHC_BLOCKS).unwrap()
            },
        ]
    );
}

// -------------------------------------------------------------------------------------------
// Busy, deselection, stray bytes

#[test]
fn a_busy_card_ignores_commands_holds_miso_low_when_reselected_and_cmd0_ends_it() {
    let mut bus = ready_sdhc();
    assert_eq!(bus.cmd(24, 0), 0x00);
    assert_eq!(bus.write_block(START_BLOCK, &[1; 512]), DATA_ACCEPTED);
    bus.deselect();
    assert_eq!(bus.raw(0xFF), None, "deselected, the card releases MISO");
    bus.select();
    assert_eq!(bus.byte(0xFF), 0x00, "reselected while programming: low");
    let during = bus.send(&frame(13, 0));
    assert_eq!(during, [0x00; 6]);
    assert_eq!(bus.byte(0xFF), 0x00);
    assert_eq!(
        bus.kinds(),
        [SdDiagnosticKind::CommandWhileBusy { index: 13 }]
    );
    bus.send(&frame(0, 0));
    assert_eq!(bus.byte(0xFF), 0xFF);
    assert_eq!(bus.byte(0xFF), 0x01, "CMD0 ends programming");
    assert!(!bus.card.is_busy(Cycles(bus.now)));
}

#[test]
fn deselecting_in_the_middle_of_a_command_loses_it() {
    let mut bus = ready_sdhc();
    bus.send(&frame(13, 0)[..3]);
    bus.deselect();
    bus.select();
    assert_eq!(bus.cmd(13, 0), 0x00, "the next command is whole");
    bus.byte(0xFF);
    assert_eq!(bus.kinds(), [SdDiagnosticKind::CommandCut]);
}

#[test]
fn deselecting_in_the_middle_of_a_block_read_abandons_it() {
    let mut bus = ready_sdhc();
    assert_eq!(bus.cmd(18, 0), 0x00);
    assert_eq!(bus.first_not_ff(), START_BLOCK);
    bus.receive(10);
    bus.deselect();
    bus.select();
    bus.wait(MS);
    assert_eq!(bus.receive(4), [0xFF; 4], "the stream ended");
    assert_eq!(bus.cmd(13, 0), 0x00);
    assert_eq!(bus.kinds(), [SdDiagnosticKind::TransferCut]);
}

#[test]
fn deselecting_in_the_middle_of_a_written_block_abandons_it_and_a_multiple_write_waits_on() {
    let mut bus = ready_sdhc();
    assert_eq!(bus.cmd(25, 10), 0x00);
    assert_eq!(bus.byte(START_MULTIPLE_WRITE), 0xFF);
    bus.send(&[1; 100]);
    bus.deselect();
    bus.select();
    assert_eq!(
        bus.write_block(START_MULTIPLE_WRITE, &[2; 512]),
        DATA_ACCEPTED
    );
    bus.wait_ready();
    assert_eq!(bus.byte(STOP_TRANSMISSION), 0xFF);
    assert_eq!(bus.cmd(17, 10), 0x00);
    assert_eq!(
        bus.data_block(512),
        [2; 512],
        "the block abandoned was not written"
    );
    assert_eq!(bus.kinds(), [SdDiagnosticKind::TransferCut]);
}

#[test]
fn a_stray_byte_or_an_unexpected_token_is_reported_and_ignored() {
    let mut bus = ready_sdhc();
    assert_eq!(bus.byte(0x12), 0xFF);
    assert_eq!(bus.cmd(25, 0), 0x00);
    assert_eq!(
        bus.byte(START_BLOCK),
        0xFF,
        "a multiple write starts its blocks with FCh"
    );
    assert_eq!(
        bus.write_block(START_MULTIPLE_WRITE, &[4; 512]),
        DATA_ACCEPTED
    );
    bus.wait_ready();
    assert_eq!(bus.byte(STOP_TRANSMISSION), 0xFF);
    assert_eq!(bus.cmd(24, 1), 0x00);
    assert_eq!(
        bus.byte(STOP_TRANSMISSION),
        0xFF,
        "a single write has no stop token"
    );
    assert_eq!(bus.write_block(START_BLOCK, &[5; 512]), DATA_ACCEPTED);
    assert_eq!(
        bus.kinds(),
        [
            SdDiagnosticKind::StrayByte { byte: 0x12 },
            SdDiagnosticKind::UnexpectedToken { token: START_BLOCK },
            SdDiagnosticKind::UnexpectedToken {
                token: STOP_TRANSMISSION
            },
        ]
    );
}

#[test]
fn a_command_while_a_write_waits_is_refused_but_cmd12_ends_it() {
    let mut bus = ready_sdhc();
    assert_eq!(bus.cmd(25, 0), 0x00);
    bus.send(&frame(13, 0));
    assert_eq!(bus.receive(2), [0xFF, 0xFF], "refused: no answer");
    assert_eq!(bus.cmd(12, 0), 0x00);
    assert_eq!(bus.cmd(13, 0), 0x00);
    assert_eq!(
        bus.kinds(),
        [SdDiagnosticKind::CommandDuringTransfer { index: 13 }]
    );
}

#[test]
fn the_diagnostics_kept_are_bounded_and_the_rest_counted() {
    let mut bus = ready_sdhc();
    for _ in 0..MAX_DIAGNOSTICS + 76 {
        bus.byte(0x12);
    }
    assert_eq!(bus.card.take_diagnostics().len(), MAX_DIAGNOSTICS);
    assert_eq!(bus.card.dropped_diagnostics(), 76);
    bus.byte(0x12);
    assert_eq!(bus.card.take_diagnostics().len(), 1);
}

#[test]
fn time_does_not_go_back_for_the_card() {
    let mut bus = ready_sdhc();
    assert_eq!(bus.cmd(24, 0), 0x00);
    assert_eq!(bus.write_block(START_BLOCK, &[1; 512]), DATA_ACCEPTED);
    bus.wait(2 * MS);
    assert_eq!(bus.byte(0xFF), 0xFF);
    bus.now -= 2 * MS;
    assert_eq!(
        bus.byte(0xFF),
        0xFF,
        "an earlier instant counts as the latest"
    );
}

#[test]
fn a_card_whose_storage_is_no_card_size_is_refused() {
    let error = SdCard::new(
        SdCardConfig::default(),
        MemoryStorage::new(4097),
        time_base(),
    )
    .unwrap_err();
    assert_eq!(error, CapacityError::NotCodableSdsc { bytes: 4097 * 512 });
    assert_eq!(
        SdCard::new(SdCardConfig::default(), MemoryStorage::new(0), time_base()).unwrap_err(),
        CapacityError::Empty
    );
}

#[test]
fn a_slower_time_base_keeps_the_durations_in_nanoseconds() {
    let mut card = SdCard::new(
        SdCardConfig::default(),
        MemoryStorage::new(SDSC_BLOCKS),
        Frequency::from_hz(160_000_000).unwrap(),
    )
    .unwrap();
    card.select(Cycles(0), true);
    let mut send = |at: u64, bytes: &[u8]| -> Vec<u8> {
        bytes
            .iter()
            .map(|&byte| card.exchange(Cycles(at), byte).unwrap_or(0xFF))
            .collect()
    };
    send(0, &frame(0, 0));
    send(0, &frame(55, 0));
    send(0, &frame(41, 0));
    assert_eq!(send(0, &[0xFF, 0xFF])[1], 0x01);
    // 50 ms at 160 MHz: 8 000 000 cycles.
    send(7_999_999, &frame(55, 0));
    send(7_999_999, &frame(41, 0));
    assert_eq!(send(7_999_999, &[0xFF, 0xFF])[1], 0x01);
    send(8_000_000, &frame(55, 0));
    send(8_000_000, &frame(41, 0));
    assert_eq!(send(8_000_000, &[0xFF, 0xFF])[1], 0x00);
}

// -------------------------------------------------------------------------------------------
// Transfers and the slot

#[test]
fn a_transfer_is_its_bytes_one_by_one_and_leaves_alone_what_the_card_does_not_drive() {
    let mut bus = ready_sdhc();
    let mut miso = [0xA5; 10];
    let mut mosi = [0xFF; 10];
    mosi[..6].copy_from_slice(&frame(17, 2));
    bus.card.transfer(Cycles(bus.now), &mosi, &mut miso);
    assert_eq!(miso[..7], [0xFF; 7]);
    assert_eq!(miso[7], 0x00, "R1 on the second byte after the command");
    let mut short = [0xA5; 2];
    bus.wait(MS);
    bus.card.transfer(Cycles(bus.now), &[0xFF; 4], &mut short);
    assert_eq!(
        short,
        [START_BLOCK, numbered(2)[0]],
        "miso shorter than mosi"
    );
    bus.deselect();
    let mut untouched = [0x5A; 3];
    bus.card
        .transfer(Cycles(bus.now), &[0xFF; 3], &mut untouched);
    assert_eq!(untouched, [0x5A; 3]);
}

#[test]
fn an_empty_slot_never_drives_miso_and_a_held_card_answers() {
    let mut slot: SdSlot<MemoryStorage> = SdSlot::empty();
    slot.select(Cycles(0), true);
    for byte in frame(0, 0).iter().chain(&[0xFF; 20]) {
        assert_eq!(slot.exchange(Cycles(0), *byte), None);
    }
    let mut miso = [0xFF; 4];
    slot.transfer(Cycles(0), &[0xFF; 4], &mut miso);
    assert_eq!(miso, [0xFF; 4], "the pad's pull-up reads FFh");
    assert!(slot.card().is_none());
    let card = SdCard::new(SdCardConfig::default(), sdsc_storage(), time_base()).unwrap();
    assert!(slot.insert(card).is_none());
    slot.select(Cycles(0), true);
    for byte in frame(0, 0) {
        slot.exchange(Cycles(0), byte);
    }
    assert_eq!(slot.exchange(Cycles(0), 0xFF), Some(0xFF));
    assert_eq!(slot.exchange(Cycles(0), 0xFF), Some(0x01));
    assert!(slot.card_mut().is_some());
    let card = slot.take().unwrap();
    assert_eq!(card.into_storage(), sdsc_storage());
    assert_eq!(slot.exchange(Cycles(0), 0xFF), None);
    let mut held =
        SdSlot::holding(SdCard::new(SdCardConfig::default(), sdsc_storage(), time_base()).unwrap());
    held.select(Cycles(0), true);
    assert_eq!(
        held.exchange(Cycles(0), 0xFF),
        None,
        "in SD mode until CMD0"
    );
}

// -------------------------------------------------------------------------------------------
// Robustness (2e-R3)

/// A deterministic stream of numbers (the generator of Numerical Recipes).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        u32::try_from(self.0 >> 33).unwrap()
    }
}

/// Bytes a host could send: mostly fill, commands with their CRC, tokens and noise.
fn hostile_byte(rng: &mut Lcg, pending: &mut Vec<u8>) -> u8 {
    if let Some(byte) = pending.pop() {
        return byte;
    }
    match rng.next() % 16 {
        0..=5 => 0xFF,
        6..=8 => {
            let index = [
                0, 8, 55, 41, 17, 18, 12, 24, 25, 13, 9, 10, 16, 32, 33, 38, 59, 6, 58, 1,
            ][usize::try_from(rng.next() % 20).unwrap()];
            let argument = match rng.next() % 4 {
                0 => 0,
                1 => rng.next() % 300,
                2 => 1 << 30 | 0x1AA,
                _ => rng.next(),
            };
            let mut bytes = frame(index, argument).to_vec();
            if rng.next().is_multiple_of(8) {
                bytes[5] ^= 0x10;
            }
            bytes.reverse();
            pending.extend(bytes);
            0xFF
        }
        9 => [0xFE, 0xFC, 0xFD][usize::try_from(rng.next() % 3).unwrap()],
        _ => rng.next().to_le_bytes()[0],
    }
}

/// Drives a card with `steps` random actions, then checks that CMD0 brings it back to the idle
/// state whatever it was doing.
fn shake<S: BlockStorage>(card: &mut SdCard<S>, seed: u64, steps: usize) {
    let mut rng = Lcg(seed);
    let mut pending = Vec::new();
    let mut now = 0u64;
    for _ in 0..steps {
        match rng.next() % 64 {
            0 => card.select(Cycles(now), rng.next().is_multiple_of(2)),
            1 => now += u64::from(rng.next() % 5_000_000),
            2 => now = now.saturating_sub(1000),
            _ => {
                let byte = hostile_byte(&mut rng, &mut pending);
                card.exchange(Cycles(now), byte);
            }
        }
        now += 50;
        if card.diagnostics.len() >= MAX_DIAGNOSTICS {
            card.take_diagnostics();
        }
    }
    card.select(Cycles(now), false);
    card.select(Cycles(now), true);
    let answer: Vec<Option<u8>> = frame(0, 0)
        .iter()
        .chain(&[0xFF, 0xFF])
        .map(|&byte| card.exchange(Cycles(now), byte))
        .collect();
    assert_eq!(answer[6..], [Some(0xFF), Some(0x01)], "seed {seed}");
}

#[test]
fn no_stream_of_bytes_selections_and_instants_makes_the_card_panic() {
    let mut standard = SdCard::new(SdCardConfig::default(), sdsc_storage(), time_base()).unwrap();
    shake(&mut standard, 1, 400_000);
    let config = SdCardConfig {
        write_protected: true,
        timing: SdTiming {
            initialization_ns: 0,
            read_access_ns: 0,
            program_ns: 0,
            erase_ns: 0,
        },
        ..SdCardConfig::default()
    };
    let mut protected = SdCard::new(config, sdsc_storage(), time_base()).unwrap();
    shake(&mut protected, 2, 400_000);
    let mut high = SdCard::new(
        SdCardConfig::default(),
        Overlay::new(SparseBlocks::new(SDHC_BLOCKS)),
        time_base(),
    )
    .unwrap();
    shake(&mut high, 3, 400_000);
}
