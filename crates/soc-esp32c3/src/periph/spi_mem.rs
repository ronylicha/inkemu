//! SPI0 and SPI1, the two controllers that share the flash chip (`spi_mem_reg.h`; reports BC §D.1,
//! FB §B and §D). SPI1 runs the commands software asks for; SPI0 refills the cache, which this
//! emulator does not model (the bus reads the flash directly), so SPI0 is configuration storage.
use super::{Accepted, SPI0_BASE, SPI1_BASE, Storage};
use crate::diag::Unsupported;
use crate::flash::{FlashCommand, FlashDevice};
use emu_core::Peripheral;
use svd_regs::esp32c3::addr;

const CMD: u32 = addr("SPI1", "CMD") - SPI1_BASE;
const ADDR: u32 = addr("SPI1", "ADDR") - SPI1_BASE;
const USER: u32 = addr("SPI1", "USER") - SPI1_BASE;
const USER1: u32 = addr("SPI1", "USER1") - SPI1_BASE;
const USER2: u32 = addr("SPI1", "USER2") - SPI1_BASE;
const MOSI_DLEN: u32 = addr("SPI1", "MOSI_DLEN") - SPI1_BASE;
const MISO_DLEN: u32 = addr("SPI1", "MISO_DLEN") - SPI1_BASE;
const RD_STATUS: u32 = addr("SPI1", "RD_STATUS") - SPI1_BASE;
const W0: u32 = addr("SPI1", "W0") - SPI1_BASE;
/// `W0..W15`: the 64-byte data buffer of a transfer.
const DATA_WORDS: u32 = 16;
const DATA_BYTES: usize = 64;
const WORD_BYTES: usize = 4;

/// `CMD` bit 18: run the transfer described by `USER`, `USER1`, `USER2`, `ADDR` and the lengths.
const CMD_USR: u32 = 1 << 18;
/// `CMD` bit 17, `FLASH_PE`: a user-defined program/erase; its opcode is the one in `USER2`.
const CMD_PE: u32 = 1 << 17;
const CMD_RDID: u32 = 1 << 28;
const CMD_RDSR: u32 = 1 << 27;
/// The hard-wired commands: `CMD` bit, the opcode the controller sends (ESP-IDF `spi_flash_defs.h`,
/// report BC §D.2 item 6; `0xB9` deep power-down is the JEDEC opcode, it is not in the report).
const HARD_WIRED: [(u32, u8); 15] = [
    (1 << 31, 0x03), // FLASH_READ
    (1 << 30, 0x06), // WREN
    (1 << 29, 0x04), // WRDI
    (CMD_RDID, 0x9F),
    (CMD_RDSR, 0x05),
    (1 << 26, 0x01), // WRSR
    (1 << 25, 0x02), // PP
    (1 << 24, 0x20), // SE
    (1 << 23, 0xD8), // BE
    (1 << 22, 0xC7), // CE
    (1 << 21, 0xB9), // DP
    (1 << 20, 0xAB), // RES
    (1 << 19, 0xA3), // HPM
    (1 << 1, 0x75),  // PES
    (1 << 0, 0x7A),  // PER
];
const JEDEC_ID_BYTES: usize = 3;
const STATUS_BYTE_MASK: u32 = 0xFF;

const USR_ADDR: u32 = 1 << 30;
const USR_DUMMY: u32 = 1 << 29;
const USR_MISO: u32 = 1 << 28;
const USR_MOSI: u32 = 1 << 27;
/// `USER1` [31:26]: address length in bits, minus one. [5:0]: dummy cycles, minus one.
const ADDR_BITLEN_SHIFT: u32 = 26;
const ADDR_BITLEN_MASK: u32 = 0x3F;
const DUMMY_CYCLES_MASK: u32 = 0x3F;
/// `USER2` [15:0]: the command; its low byte is the opcode of every 8-bit command.
const OPCODE_MASK: u32 = 0xFF;
/// `MOSI_DLEN` / `MISO_DLEN` [9:0]: data length in bits, minus one.
const DLEN_MASK: u32 = 0x3FF;

/// Configuration of SPI1 that only shapes the electrical transfer, which is instantaneous here.
const SPI1_ACCEPTED: [Accepted; 6] = [
    Accepted::svd("SPI1", "CTRL", "read mode, dummy output, WP level", "SP8"),
    Accepted::svd(
        "SPI1",
        "CTRL1",
        "CS hold delay after a release-from-power-down",
        "SP8",
    ),
    Accepted::svd("SPI1", "CTRL2", "CS setup and hold times", "SP8"),
    Accepted::svd(
        "SPI1",
        "CLOCK",
        "SPI clock divider: a transfer takes no time",
        "SP8",
    ),
    Accepted::svd(
        "SPI1",
        "SUS_STATUS",
        "program/erase suspend state",
        "SP1b-3",
    ),
    Accepted::svd("SPI1", "CLOCK_GATE", "register clock gate", "SP1b-3"),
];

/// SPI0 `+0x24` and `+0x28`: `MOSI_DLEN` and `MISO_DLEN`. The SVD omits them for SPI0; ESP-IDF
/// `spi_mem_reg.h:451,459` defines them for both controllers and the ROM writes them (FB §B.2).
const SPI0_MOSI_DLEN: u32 = SPI0_BASE + 0x24;
const SPI0_MISO_DLEN: u32 = SPI0_BASE + 0x28;
const SPI0_FSM: u32 = addr("SPI0", "FSM") - SPI0_BASE;
const WHY_SPI0: &str =
    "cache-fill transfer (read command, dummy cycles, clock): the bus reads the flash directly";
const SPI0_ACCEPTED: [Accepted; 11] = [
    Accepted::svd("SPI0", "CTRL", WHY_SPI0, "SP1b-4"),
    Accepted::svd("SPI0", "CTRL2", WHY_SPI0, "SP1b-4"),
    Accepted::svd("SPI0", "CLOCK", WHY_SPI0, "SP1b-4"),
    Accepted::svd("SPI0", "USER", WHY_SPI0, "SP1b-4"),
    Accepted::svd("SPI0", "USER1", WHY_SPI0, "SP1b-4"),
    Accepted::svd("SPI0", "USER2", WHY_SPI0, "SP1b-4"),
    Accepted::at("SPI0", "MOSI_DLEN", SPI0_MOSI_DLEN, WHY_SPI0, "SP1b-4"),
    Accepted::at("SPI0", "MISO_DLEN", SPI0_MISO_DLEN, WHY_SPI0, "SP1b-4"),
    Accepted::svd("SPI0", "MISC", "chip-select enables", "SP1b-4"),
    Accepted::svd("SPI0", "CACHE_FCTRL", "cache request enable", "SP1b-4"),
    Accepted::svd("SPI0", "CLOCK_GATE", "register clock gate", "SP1b-3"),
];

pub(crate) fn accepted() -> Vec<Accepted> {
    [&SPI1_ACCEPTED[..], &SPI0_ACCEPTED[..]].concat()
}

#[derive(Clone, Debug)]
pub struct Spi0 {
    config: Storage,
    fsm_idle: u32,
}

impl Spi0 {
    #[must_use]
    pub fn new() -> Spi0 {
        Spi0 {
            config: Storage::accepted(SPI0_BASE, &SPI0_ACCEPTED),
            // The reset value has the state bits [6:4] clear: idle, what `Wait_SPI_Idle` waits for.
            fsm_idle: svd_regs::esp32c3::find("SPI0", "FSM").map_or(0, |r| r.reset_value),
        }
    }
}

impl Default for Spi0 {
    fn default() -> Spi0 {
        Spi0::new()
    }
}

impl Peripheral for Spi0 {
    fn read(&mut self, offset: u32) -> Option<u32> {
        if offset == SPI0_FSM {
            return Some(self.fsm_idle);
        }
        self.config.read(offset)
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        self.config.write(offset, value)
    }
}

#[derive(Debug)]
pub struct Spi1 {
    registers: Storage,
    flash: Box<dyn FlashDevice>,
    unsupported: Option<Unsupported>,
}

impl Spi1 {
    #[must_use]
    pub fn new(flash: Box<dyn FlashDevice>) -> Spi1 {
        let mut offsets: Vec<u32> = SPI1_ACCEPTED.iter().map(|a| a.addr - SPI1_BASE).collect();
        offsets.extend([ADDR, USER, USER1, USER2, MOSI_DLEN, MISO_DLEN, RD_STATUS]);
        offsets.extend((0..DATA_WORDS).map(|word| W0 + 4 * word));
        Spi1 {
            registers: Storage::for_offsets(SPI1_BASE, &offsets),
            flash,
            unsupported: None,
        }
    }

    /// The chip, for the cache windows of the bus.
    #[must_use]
    pub fn flash(&self) -> &dyn FlashDevice {
        self.flash.as_ref()
    }

    /// The command the flash model refused since the last call, if any.
    pub(crate) fn take_unsupported(&mut self) -> Option<Unsupported> {
        self.unsupported.take()
    }

    fn register(&self, offset: u32) -> u32 {
        self.registers.read(offset).unwrap_or(0)
    }

    /// Runs `command`; `Some(bytes)` are the `data_in_len` bytes read, `None` when the chip model
    /// does not implement the opcode (the refusal is kept for the bus to report).
    fn transfer(&mut self, command: &FlashCommand<'_>, data_in_len: usize) -> Option<Vec<u8>> {
        let mut data_in = vec![0u8; data_in_len.min(DATA_BYTES)];
        match self.flash.execute(command, &mut data_in) {
            Ok(()) => Some(data_in),
            Err(refused) => {
                self.unsupported = Some(Unsupported::FlashCommand {
                    opcode: refused.opcode,
                });
                None
            }
        }
    }

    /// The low byte of the command in `USER2`.
    fn user_opcode(&self) -> u8 {
        (self.register(USER2) & OPCODE_MASK).to_le_bytes()[0]
    }

    fn run_hard_wired(&mut self, command_bit: u32, opcode: u8) {
        let command = FlashCommand {
            opcode,
            address: None,
            dummy_cycles: 0,
            data_out: &[],
        };
        match command_bit {
            CMD_RDID => {
                if let Some(id) = self.transfer(&command, JEDEC_ID_BYTES) {
                    // ASSUMPTION: W0[31:24] reads 0 after a read-id; the ROM masks it (FB §L item 7).
                    self.store_data_in(&id);
                }
            }
            CMD_RDSR => {
                if let Some(status) = self.transfer(&command, 1) {
                    // ASSUMPTION: only RD_STATUS[7:0] is replaced (FB §D.1, §L item 7).
                    let low = status.first().copied().map_or(0, u32::from);
                    let kept = self.register(RD_STATUS) & !STATUS_BYTE_MASK;
                    self.registers.write(RD_STATUS, kept | low);
                }
            }
            _ => {
                let _ = self.transfer(&command, 0);
            }
        }
    }

    /// Bytes a `xxx_DLEN` register asks for: its field is the bit length minus one.
    fn data_len(&self, register: u32) -> usize {
        let bits = (self.register(register) & DLEN_MASK) + 1;
        usize::try_from(bits.div_ceil(8))
            .unwrap_or(DATA_BYTES)
            .min(DATA_BYTES)
    }

    fn run_user(&mut self) {
        let user = self.register(USER);
        let user1 = self.register(USER1);
        // ASSUMPTION: every transfer has an 8-bit command phase (all measured ones do: `USER2`
        // [31:28] = 7, FB §B.2); the opcode is the low byte of the command value.
        let opcode = self.user_opcode();
        let address = (user & USR_ADDR != 0).then(|| {
            // The field can ask for up to 64 bits; the register holds 32.
            let bits = (((user1 >> ADDR_BITLEN_SHIFT) & ADDR_BITLEN_MASK) + 1).min(u32::BITS);
            // ASSUMPTION: the address is the low `bits` bits of ADDR, unshifted (ESP-IDF
            // `spimem_flash_ll_set_usr_address` stores it as is). Only the SFDP read uses an address
            // on the measured paths and its answer does not depend on it; the lockstep settles it.
            self.register(ADDR) & u32::MAX.checked_shr(u32::BITS - bits).unwrap_or(u32::MAX)
        });
        let dummy_cycles = if user & USR_DUMMY == 0 {
            0
        } else {
            (user1 & DUMMY_CYCLES_MASK) + 1
        };
        let data_out: Vec<u8> = if user & USR_MOSI == 0 {
            Vec::new()
        } else {
            (0..DATA_WORDS)
                .flat_map(|word| self.register(W0 + 4 * word).to_le_bytes())
                .take(self.data_len(MOSI_DLEN))
                .collect()
        };
        let data_in_len = if user & USR_MISO == 0 {
            0
        } else {
            self.data_len(MISO_DLEN)
        };
        let command = FlashCommand {
            opcode,
            address,
            dummy_cycles,
            data_out: &data_out,
        };
        if let Some(data_in) = self.transfer(&command, data_in_len) {
            self.store_data_in(&data_in);
        }
    }

    /// Stores the bytes read into `W0..`, the first byte in `W0[7:0]` (FB §D.1).
    /// ASSUMPTION: the bytes of the last word that were not read are zero (FB §L item 7).
    fn store_data_in(&mut self, bytes: &[u8]) {
        for (word, chunk) in (0u32..).zip(bytes.chunks(WORD_BYTES)) {
            let mut le = [0u8; WORD_BYTES];
            // INVARIANT: `chunks(WORD_BYTES)` yields at most `WORD_BYTES` bytes, so the slice is in bounds.
            le[..chunk.len()].copy_from_slice(chunk);
            self.registers.write(W0 + 4 * word, u32::from_le_bytes(le));
        }
    }

    /// Runs every command whose bit is set. Transfers complete at once, so `CMD` reads 0 afterwards.
    fn run(&mut self, command_bits: u32) {
        for (bit, opcode) in HARD_WIRED {
            if command_bits & bit != 0 {
                self.run_hard_wired(bit, opcode);
            }
        }
        if command_bits & CMD_PE != 0 {
            let opcode = self.user_opcode();
            self.run_hard_wired(CMD_PE, opcode);
        }
        if command_bits & CMD_USR != 0 {
            self.run_user();
        }
    }
}

impl Peripheral for Spi1 {
    fn read(&mut self, offset: u32) -> Option<u32> {
        if offset == CMD {
            // Every command bit self-clears and the state nibbles read idle (FB §C rows 9 and 11).
            return Some(0);
        }
        self.registers.read(offset)
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        if offset == CMD {
            self.run(value);
            return true;
        }
        self.registers.write(offset, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flash::{FlashConfig, NorFlash};

    /// `USER` bit 31: the transfer has a command phase (every measured one does, so the model never
    /// tests the bit and the constant lives with the tests that build `USER` values).
    const USR_COMMAND: u32 = 1 << 31;

    fn spi1() -> Spi1 {
        Spi1::new(Box::new(NorFlash::blank(FlashConfig::default())))
    }
    /// A `USR` transfer of `opcode` reading `miso_bits` bits, as `bootloader_flash_execute_command_common` sets it up.
    fn user_read(spi: &mut Spi1, opcode: u32, miso_bits: u32) {
        spi.write(USER, USR_COMMAND | USR_MISO);
        spi.write(USER2, 0x7000_0000 | opcode);
        spi.write(MISO_DLEN, miso_bits - 1);
        spi.write(W0, 0);
        spi.write(CMD, CMD_USR);
    }

    #[test]
    fn configuration_registers_start_at_their_svd_reset_values() {
        let mut spi = spi1();
        for (register, reset) in [
            ("CTRL", 0x002C_A000),
            ("CTRL1", 0x0000_0FFC),
            ("CLOCK", 0x0003_0103),
            ("USER", 0x8000_0000),
            ("USER1", 0x5C00_0007),
            ("USER2", 0x7000_0000),
        ] {
            let offset = svd_regs::esp32c3::addr("SPI1", register) - SPI1_BASE;
            assert_eq!(spi.read(offset), Some(reset), "{register}");
        }
        assert_eq!(spi.read(CMD), Some(0));
    }

    #[test]
    fn rom_read_id_puts_the_jedec_bytes_in_w0_and_the_command_self_clears() {
        // FB §B.6: `esp_rom_spi_flash_update_id` writes W0 = 0, CMD = 0x10000000, polls CMD, reads W0.
        let mut spi = spi1();
        spi.write(W0, 0);
        assert!(spi.write(CMD, 0x1000_0000));
        assert_eq!(
            spi.read(CMD),
            Some(0),
            "the ROM waits for the whole register to read 0"
        );
        assert_eq!(spi.read(W0), Some(0x0018_609D));
        assert_eq!(spi.take_unsupported(), None);
    }

    #[test]
    fn rom_read_status_puts_the_status_byte_in_rd_status() {
        // FB §B.6: RD_STATUS[15:0] is zeroed, CMD = 0x08000000, then bit 0 (WIP) must read 0.
        let mut spi = spi1();
        spi.write(RD_STATUS, 0xABCD_0000);
        spi.write(CMD, 0x0800_0000);
        assert_eq!(
            spi.read(RD_STATUS),
            Some(0xABCD_0000),
            "status 0, upper half kept"
        );
        spi.write(CMD, 0x4000_0000); // WREN
        spi.write(CMD, 0x0800_0000);
        assert_eq!(spi.read(RD_STATUS), Some(0xABCD_0002));
        spi.write(CMD, 0x2000_0000); // WRDI
        spi.write(CMD, 0x0800_0000);
        assert_eq!(spi.read(RD_STATUS), Some(0xABCD_0000));
    }

    #[test]
    fn rom_wake_up_runs_without_touching_the_data_registers() {
        // FB §B.4: `SPI_WakeUp` writes CMD = 0x00100000 (FLASH_RES) and polls CMD.
        let mut spi = spi1();
        spi.write(W0, 0x1234_5678);
        spi.write(CMD, 0x0010_0000);
        assert_eq!(spi.read(CMD), Some(0));
        assert_eq!(spi.read(W0), Some(0x1234_5678));
        assert_eq!(spi.take_unsupported(), None);
    }

    #[test]
    fn bootloader_user_commands_run_through_the_generic_path() {
        // BA §2.2, SPI1 row: RDID 0x9F (24 bits in), SFDP 0x5A at 0x10 with 8 dummy bits (8 bits in),
        // RESUME 0x7A, RDSR 0x05 (8 bits in), WRDI 0x04.
        let mut spi = spi1();
        user_read(&mut spi, 0x9F, 24);
        assert_eq!(spi.read(W0), Some(0x0018_609D), "BA §2 row 46");
        spi.write(USER, USR_COMMAND | USR_ADDR | USR_DUMMY | USR_MISO);
        spi.write(USER1, (23 << 26) | 7);
        spi.write(USER2, 0x7000_005A);
        spi.write(ADDR, 0x10);
        spi.write(MISO_DLEN, 7);
        spi.write(CMD, CMD_USR);
        assert_eq!(spi.read(W0), Some(0xFF));
        for opcode in [0x7A, 0x04] {
            spi.write(USER, USR_COMMAND);
            spi.write(USER2, 0x7000_0000 | opcode);
            spi.write(CMD, CMD_USR);
        }
        user_read(&mut spi, 0x05, 8);
        assert_eq!(spi.read(W0), Some(0));
        assert_eq!(spi.read(CMD), Some(0));
        assert_eq!(spi.take_unsupported(), None);
    }

    #[test]
    fn program_and_erase_are_reported_by_opcode() {
        let mut spi = spi1();
        for (command_bit, opcode) in [
            (1u32 << 25, 0x02u8), // PP
            (1 << 24, 0x20),      // SE
            (1 << 23, 0xD8),      // BE
            (1 << 22, 0xC7),      // CE
            (1 << 26, 0x01),      // WRSR
            (1 << 31, 0x03),      // FLASH_READ
        ] {
            assert!(
                spi.write(CMD, command_bit),
                "the write is handled: it is reported, not dropped"
            );
            assert_eq!(
                spi.take_unsupported(),
                Some(Unsupported::FlashCommand { opcode })
            );
            assert_eq!(spi.take_unsupported(), None, "taken once");
            assert_eq!(spi.read(CMD), Some(0), "no hang on an unmodelled command");
        }
    }

    #[test]
    fn unknown_user_opcode_is_reported() {
        let mut spi = spi1();
        spi.write(W0, 0xDEAD_BEEF);
        spi.write(USER, USR_COMMAND | USR_ADDR | USR_MOSI);
        spi.write(USER2, 0x7000_0002); // page program
        spi.write(MOSI_DLEN, 31);
        spi.write(CMD, CMD_USR);
        assert_eq!(
            spi.take_unsupported(),
            Some(Unsupported::FlashCommand { opcode: 0x02 })
        );
        assert_eq!(spi.read(W0), Some(0xDEAD_BEEF), "nothing was clocked in");
    }

    #[test]
    fn transfer_lengths_are_bounded_by_the_sixteen_data_registers() {
        // MISO_DLEN can ask for 1024 bits (128 bytes); the data buffer is W0..W15, 64 bytes.
        let mut spi = spi1();
        user_read(&mut spi, 0x9F, 1024);
        let w15 = svd_regs::esp32c3::addr("SPI1", "W15") - SPI1_BASE;
        // 64 bytes of the repeating id 9D 60 18: byte 60 is 9D (60 % 3 == 0).
        assert_eq!(spi.read(w15), Some(0x9D18_609D));
        assert_eq!(
            spi.read(w15 + 4),
            None,
            "FLASH_WAITI_CTRL is not modelled: nothing was stored past W15"
        );
    }

    #[test]
    fn a_read_shorter_than_a_word_zeroes_the_rest_of_that_word() {
        let mut spi = spi1();
        spi.write(W0, 0xFFFF_FFFF);
        user_read(&mut spi, 0x9F, 16);
        assert_eq!(spi.read(W0), Some(0x0000_609D));
    }

    #[test]
    fn spi0_stores_the_read_mode_the_rom_programs_and_reads_idle() {
        // FB §B.6: SPI0.CTRL ends at 0x00AC2008; the bootloader decodes it to print "SPI Mode : DIO".
        let mut spi0 = Spi0::new();
        let ctrl = svd_regs::esp32c3::addr("SPI0", "CTRL") - SPI0_BASE;
        assert_eq!(spi0.read(ctrl), Some(0x002C_2000));
        assert!(spi0.write(ctrl, 0x00AC_2008));
        assert_eq!(spi0.read(ctrl), Some(0x00AC_2008));
        let fsm = svd_regs::esp32c3::addr("SPI0", "FSM") - SPI0_BASE;
        assert_eq!(
            spi0.read(fsm).map(|v| v & 0x70),
            Some(0),
            "Wait_SPI_Idle: bits 6:4 (FB §C row 10)"
        );
        assert!(!spi0.write(fsm, 0), "FSM is read-only");
        for offset in [SPI0_MOSI_DLEN - SPI0_BASE, SPI0_MISO_DLEN - SPI0_BASE] {
            assert_eq!(spi0.read(offset), Some(0));
            assert!(
                spi0.write(offset, 0xFF),
                "FB §B.2: 256 bits, one cache line"
            );
            assert_eq!(spi0.read(offset), Some(0xFF));
        }
    }
}
