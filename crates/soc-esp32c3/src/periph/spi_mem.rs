//! SPI0 and SPI1, the two controllers that share the flash chip (`spi_mem_reg.h`; reports BC §D.1,
//! FB §B and §D). SPI1 runs the commands software asks for; SPI0 refills the cache, which this
//! emulator does not model (the bus reads the flash directly), so SPI0 is configuration storage.
use super::mmu::MMU_PAGE_BYTES;
use super::{Accepted, SPI0_BASE, SPI1_BASE, Storage};
use crate::ctx::{Ctx, Event};
use crate::diag::Unsupported;
use crate::flash::{FlashAddress, FlashCommand, FlashDevice, FlashJournal, FlashWrite};
use emu_core::Peripheral;
use svd_regs::esp32c3::addr;

const CMD: u32 = addr("SPI1", "CMD") - SPI1_BASE;
const ADDR: u32 = addr("SPI1", "ADDR") - SPI1_BASE;
const CTRL: u32 = addr("SPI1", "CTRL") - SPI1_BASE;
const FLASH_SUS_CTRL: u32 = addr("SPI1", "FLASH_SUS_CTRL") - SPI1_BASE;
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
const CMD_READ: u32 = 1 << 31;
const CMD_RDID: u32 = 1 << 28;
const CMD_RDSR: u32 = 1 << 27;
const CMD_WRSR: u32 = 1 << 26;
const CMD_PP: u32 = 1 << 25;
const CMD_SE: u32 = 1 << 24;
const CMD_BE: u32 = 1 << 23;
/// The hard-wired commands: `CMD` bit, the opcode the controller sends (report FW §A.1; ESP-IDF
/// `spi_flash_defs.h`; `0xB9` deep power-down is the JEDEC opcode). `FLASH_BE` is the 64 KiB
/// erase `D8`, whatever the header's "32KB" says: the ROM and ESP-IDF both use it for 64 KiB.
///
/// `CMD` [7:0] are two read-only state fields (`SPI1_MST_ST`, `MSPI_ST`): no command lives
/// there. Suspend and resume are bits 1 and 0 of `FLASH_SUS_CTRL`.
const HARD_WIRED: [(u32, u8); 13] = [
    (CMD_READ, 0x03),
    (1 << 30, 0x06), // WREN
    (1 << 29, 0x04), // WRDI
    (CMD_RDID, 0x9F),
    (CMD_RDSR, 0x05),
    (CMD_WRSR, 0x01),
    (CMD_PP, 0x02),
    (CMD_SE, 0x20),
    (CMD_BE, 0xD8),
    (1 << 22, 0xC7), // CE
    (1 << 21, 0xB9), // DP
    (1 << 20, 0xAB), // RES
    (1 << 19, 0xA3), // HPM
];
/// Every bit of `CMD` that starts a transfer: the hard-wired commands, bits 31 to 19, and `USR`.
/// Bit 17, `FLASH_PE`, is below them: it is a flag (see [`Spi1::run`]).
const COMMAND_BITS: u32 = !0 << 18;
const JEDEC_ID_BYTES: usize = 3;
const STATUS_BYTE_MASK: u32 = 0xFF;
/// A hard-wired command sends the low 24 bits of `ADDR`: `addr & 0xffffff` for an erase, the
/// same under the byte count for a program (`ROM3:SPI_sector_erase@0x4004d294`,
/// `ROM3:SPI_page_program@0x4004d6b4`; `IDF:hal/esp32c3/include/hal/spimem_flash_ll.h:81-95,
/// 377-389`; measured: report FW §A.2, §A.3).
const HARD_WIRED_ADDRESS_BITS: u32 = 24;
const HARD_WIRED_ADDRESS_MASK: u32 = 0x00FF_FFFF;
/// `ADDR` [31:24] of a hard-wired page program: the number of bytes (`0x20009000` is 32 bytes at
/// `0x9000`; reports FW key result 1, SF §B.2).
const PP_LENGTH_SHIFT: u32 = 24;
/// `CTRL` bit 22, `WRSR_2B`: the status write sends two bytes instead of one
/// (`spi_mem_reg.h:158-161`). `ROM3:Enable_QMode` sets it (`CTRL |= 0x400000` at `0x4004d792` to
/// `0x4004d7a0`) before it writes a 16-bit status with `esp_rom_spiflash_write_status`.
const CTRL_WRSR_2B: u32 = 1 << 22;
/// `FLASH_SUS_CTRL`: bit 0 `FLASH_PER` and bit 1 `FLASH_PES` send a resume and a suspend and clear
/// themselves (`spimem_flash_ll_resume`, `spimem_flash_ll_suspend`); bit 4 `PES_PER_EN` and bit 5
/// `FLASH_PES_EN` enable the automatic ones (`spimem_flash_ll_auto_resume_init`,
/// `spimem_flash_ll_auto_suspend_init`: `spimem_flash_ll.h:102-139`, `spi_mem_reg.h:821-856`).
const SUS_COMMANDS: u32 = 0b11;
const SUS_REQUESTS: u32 = SUS_COMMANDS | (1 << 4) | (1 << 5);

/// `USER` bit 31: the transfer has a command phase.
const USR_COMMAND: u32 = 1 << 31;
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
/// `USER2` [31:28]: command length in bits, minus one.
const COMMAND_BITLEN_SHIFT: u32 = 28;
const COMMAND_BITLEN_MASK: u32 = 0xF;
/// The only command length the model runs: every measured transfer has it (`USER2` [31:28] = 7,
/// FB §B.2).
const MODELED_COMMAND_BITS: u32 = 8;
/// `MOSI_DLEN` / `MISO_DLEN` [9:0]: data length in bits, minus one.
const DLEN_MASK: u32 = 0x3FF;

/// Configuration of SPI1 that nothing acts on here: what shapes the electrical transfer, which
/// is instantaneous, and the automatic wait, suspend and resume around a program or an erase.
/// A flash operation completes at once (report FW §A.5: every measured poll ends on its first
/// read), so there is nothing to wait for and nothing to suspend: these three rows wait for the
/// plan that gives a flash operation its duration.
const SPI1_ACCEPTED: [Accepted; 9] = [
    Accepted::svd(
        "SPI1",
        "CTRL",
        "read mode, dummy output, WP level (the two-byte status write bit acts)",
        "SP8",
    ),
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
        "program/erase suspend state: nothing is ever suspended",
        "SP8",
    ),
    Accepted::svd("SPI1", "CLOCK_GATE", "register clock gate", "SP8"),
    // `spi_flash_hal_device_config` and what it calls (`IDF:hal/spi_flash_hal_common.inc:45`; F1).
    Accepted::svd("SPI1", "MISC", "chip-select idle level and enables", "SP8"),
    Accepted::svd(
        "SPI1",
        "FLASH_WAITI_CTRL",
        "automatic wait for the chip to be idle after a program or an erase: an operation completes at once",
        "SP8",
    ),
    Accepted::svd(
        "SPI1",
        "FLASH_SUS_CTRL",
        "program/erase suspend and resume: an operation completes at once (a suspend or resume request is refused by name)",
        "SP8",
    ),
];

/// SPI0 `+0x24` and `+0x28`: `MOSI_DLEN` and `MISO_DLEN`. The SVD omits them for SPI0; ESP-IDF
/// `spi_mem_reg.h:451,459` defines them for both controllers and the ROM writes them (FB §B.2).
const SPI0_MOSI_DLEN: u32 = SPI0_BASE + 0x24;
const SPI0_MISO_DLEN: u32 = SPI0_BASE + 0x28;
const SPI0_FSM: u32 = addr("SPI0", "FSM") - SPI0_BASE;
const WHY_SPI0: &str =
    "cache-fill transfer (read command, dummy cycles, clock): the bus reads the flash directly";
const SPI0_ACCEPTED: [Accepted; 12] = [
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
    Accepted::svd("SPI0", "CLOCK_GATE", "register clock gate", "SP8"),
    // Read, never written: `spimem_flash_ll_get_source_freq_mhz` (`spimem_flash_ll.h:630`; F1).
    Accepted::svd(
        "SPI0",
        "CORE_CLK_SEL",
        "MSPI core clock selection: a transfer takes no time",
        "SP8",
    ),
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

impl Peripheral<Ctx> for Spi0 {
    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
        if offset == SPI0_FSM {
            return Some(self.fsm_idle);
        }
        self.config.read(offset)
    }

    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        self.config.write(offset, value)
    }
}

/// The first and the last 64 KiB flash page `write` holds bytes of, the unit of the MMU and of
/// what the cache is told; `None` for a status write, which changes no content.
fn cache_pages(write: &FlashWrite) -> Option<(u32, u32)> {
    match *write {
        // A program stays inside its 256-byte page, so inside one 64 KiB page.
        FlashWrite::Program { address, .. } => {
            let page = address / MMU_PAGE_BYTES;
            Some((page, page))
        }
        FlashWrite::Erase { start, len } => {
            let last = start.saturating_add(len.saturating_sub(1));
            Some((start / MMU_PAGE_BYTES, last / MMU_PAGE_BYTES))
        }
        FlashWrite::Status { .. } => None,
    }
}

#[derive(Debug)]
pub struct Spi1 {
    registers: Storage,
    flash: Box<dyn FlashDevice>,
    /// What the commands of the run wrote to the chip. It belongs to the run, not to the
    /// controller: a reset of the controller keeps it.
    journal: FlashJournal,
}

impl Spi1 {
    #[must_use]
    pub fn new(flash: Box<dyn FlashDevice>) -> Spi1 {
        Spi1 {
            registers: Spi1::reset_registers(),
            flash,
            journal: FlashJournal::default(),
        }
    }

    fn reset_registers() -> Storage {
        let mut offsets: Vec<u32> = SPI1_ACCEPTED.iter().map(|a| a.addr - SPI1_BASE).collect();
        offsets.extend([ADDR, USER, USER1, USER2, MOSI_DLEN, MISO_DLEN, RD_STATUS]);
        offsets.extend((0..DATA_WORDS).map(|word| W0 + 4 * word));
        Storage::for_offsets(SPI1_BASE, &offsets)
    }

    /// The controller is reset (`SYSTEM.PERIP_RST_EN0` bit 1, or a core reset): its registers
    /// return to their reset values. The flash chip is another component: it keeps its content.
    pub(crate) fn reset(&mut self) {
        self.registers = Spi1::reset_registers();
    }

    /// The chip, for the cache windows of the bus.
    #[must_use]
    pub fn flash(&self) -> &dyn FlashDevice {
        self.flash.as_ref()
    }

    /// The programs, erases and status writes the chip performed since the last call, oldest
    /// first: see [`FlashJournal`].
    pub fn take_flash_writes(&mut self) -> Vec<FlashWrite> {
        self.journal.take()
    }

    /// The operations the journal did not keep because it was full.
    #[must_use]
    pub const fn flash_writes_dropped(&self) -> u64 {
        self.journal.dropped()
    }

    fn register(&self, offset: u32) -> u32 {
        self.registers.read(offset).unwrap_or(0)
    }

    /// Runs `command`; `Some(bytes)` are the `data_in_len` bytes read, `None` when the chip model
    /// refuses the command (the refusal is reported through the context). What the command
    /// wrote, if anything, goes to the journal.
    fn transfer(
        &mut self,
        ctx: &mut Ctx,
        command: &FlashCommand<'_>,
        data_in_len: usize,
    ) -> Option<Vec<u8>> {
        let mut data_in = vec![0u8; data_in_len.min(DATA_BYTES)];
        let before = self.flash.generation();
        match self.flash.execute(command, &mut data_in) {
            Ok(written) => {
                if let Some(write) = written {
                    // The cache is told of the bytes that changed, not of a command that left
                    // the content as it was: only the former can make a cached line stale.
                    if self.flash.generation() != before
                        && let Some((first_page, last_page)) = cache_pages(&write)
                    {
                        ctx.signal(Event::FlashWritten {
                            first_page,
                            last_page,
                        });
                    }
                    self.journal.record(write);
                }
                Some(data_in)
            }
            Err(refusal) => {
                ctx.unsupported(Unsupported::from(refusal));
                None
            }
        }
    }

    /// The low byte of the command in `USER2`.
    fn user_opcode(&self) -> u8 {
        (self.register(USER2) & OPCODE_MASK).to_le_bytes()[0]
    }

    /// The first `len` bytes of `W0..W15`, the first byte in `W0[7:0]`; at most the 64 there are.
    fn data_out(&self, len: usize) -> Vec<u8> {
        (0..DATA_WORDS)
            .flat_map(|word| self.register(W0 + 4 * word).to_le_bytes())
            .take(len)
            .collect()
    }

    /// Runs the hard-wired command of `command_bit`. The ROM and ESP-IDF write the same registers
    /// for the same command (reports FW §A.2, §A.3; SF §B.1, §D): the address in `ADDR[23:0]`, for a
    /// program its byte count in `ADDR[31:24]` and its bytes in `W0..`.
    fn run_hard_wired(&mut self, ctx: &mut Ctx, command_bit: u32, opcode: u8) {
        let addr = self.register(ADDR);
        let address = Some(FlashAddress {
            value: addr & HARD_WIRED_ADDRESS_MASK,
            bits: HARD_WIRED_ADDRESS_BITS,
        });
        let command = FlashCommand {
            opcode,
            address: None,
            dummy_cycles: 0,
            data_out: &[],
        };
        match command_bit {
            CMD_RDID => {
                if let Some(id) = self.transfer(ctx, &command, JEDEC_ID_BYTES) {
                    // ASSUMPTION: W0[31:24] reads 0 after a read-id; the ROM masks it (FB §L item 7).
                    self.store_data_in(&id);
                }
            }
            CMD_RDSR => {
                if let Some(status) = self.transfer(ctx, &command, 1) {
                    // ASSUMPTION: only RD_STATUS[7:0] is replaced (FB §D.1, §L item 7).
                    let low = status.first().copied().map_or(0, u32::from);
                    let kept = self.register(RD_STATUS) & !STATUS_BYTE_MASK;
                    self.registers.write(RD_STATUS, kept | low);
                }
            }
            CMD_PP => {
                // The byte count is one byte of a register the firmware writes: 0 to 255. The
                // buffer holds 64 bytes, and neither the ROM (32 at most) nor ESP-IDF (64 at
                // most) asks for more: what the controller would send for another count is not
                // established, so such a program is refused rather than guessed (report FW §G).
                let len = usize::from((addr >> PP_LENGTH_SHIFT).to_le_bytes()[0]);
                if len == 0 || len > DATA_BYTES {
                    ctx.unsupported(Unsupported::FlashProgramLength { addr });
                    return;
                }
                let data = self.data_out(len);
                let program = FlashCommand {
                    address,
                    data_out: &data,
                    ..command
                };
                let _ = self.transfer(ctx, &program, 0);
            }
            CMD_SE | CMD_BE => {
                let _ = self.transfer(ctx, &FlashCommand { address, ..command }, 0);
            }
            CMD_WRSR => {
                // The value is `RD_STATUS[15:0]`, where `ROM3:esp_rom_spiflash_write_status`
                // puts it (`0x4004d35e` to `0x4004d370`) before it writes `CMD = 0x04000000`.
                let status = self.register(RD_STATUS).to_le_bytes();
                let bytes = if self.register(CTRL) & CTRL_WRSR_2B == 0 {
                    1
                } else {
                    2
                };
                let write = FlashCommand {
                    data_out: &status[..bytes],
                    ..command
                };
                let _ = self.transfer(ctx, &write, 0);
            }
            CMD_READ => {
                // ASSUMPTION: the hard-wired read sends `ADDR[23:0]` as the erase commands do
                // and clocks in the bytes `MISO_DLEN` asks for, into `W0..`. `spi_mem_reg.h:18`
                // names the bit and nothing more, and no ROM or ESP-IDF v5.5.2 code writes it
                // (report FW §A.1: "nobody"; the ROM reads through `USR`). A firmware that uses
                // it, read against a device, settles the encoding.
                let len = self.data_len(MISO_DLEN);
                if let Some(data) = self.transfer(ctx, &FlashCommand { address, ..command }, len) {
                    self.store_data_in(&data);
                }
            }
            _ => {
                let _ = self.transfer(ctx, &command, 0);
            }
        }
    }

    /// Bytes a `xxx_DLEN` register asks for: its field is the bit length minus one.
    ///
    /// ASSUMPTION: a length that is not a whole number of bytes is rounded up, the bits past it
    /// being transferred like the others. Every measured transfer is a whole number of bytes (FB
    /// §B.2, BA §2.2); what the controller does with a partial byte is not established. The SP1b-4
    /// lockstep settles it.
    fn data_len(&self, register: u32) -> usize {
        let bits = (self.register(register) & DLEN_MASK) + 1;
        usize::try_from(bits.div_ceil(8))
            .unwrap_or(DATA_BYTES)
            .min(DATA_BYTES)
    }

    /// The length of the command phase `USER` and `USER2` describe: `None` without one.
    fn command_bits(user: u32, user2: u32) -> Option<u32> {
        (user & USR_COMMAND != 0)
            .then_some(((user2 >> COMMAND_BITLEN_SHIFT) & COMMAND_BITLEN_MASK) + 1)
    }

    fn run_user(&mut self, ctx: &mut Ctx) {
        let user = self.register(USER);
        let user1 = self.register(USER1);
        // A transfer without a command phase, or whose command is not 8 bits long, has no opcode the
        // chip model could be given: it is reported and nothing is sent, rather than run as if the
        // low byte of `USER2` were its opcode.
        let command_bits = Spi1::command_bits(user, self.register(USER2));
        if command_bits != Some(MODELED_COMMAND_BITS) {
            ctx.unsupported(Unsupported::FlashTransfer {
                command_bits,
                user,
                user2: self.register(USER2),
            });
            return;
        }
        let opcode = self.user_opcode();
        let address = (user & USR_ADDR != 0).then(|| {
            // The field can ask for up to 64 bits; the register holds 32.
            let bits = (((user1 >> ADDR_BITLEN_SHIFT) & ADDR_BITLEN_MASK) + 1).min(u32::BITS);
            // The address is the low `bits` bits of ADDR, unshifted: ESP-IDF's
            // `spimem_flash_ll_set_usr_address` stores it as it is whatever the bit length
            // (`spimem_flash_ll.h:570`), and a measured 24-bit read of flash offset 0xFF0000
            // has `ADDR = 0x00FF0000` (reports FW §A.1, §P; SF §C). 24 bits is the only length
            // the chip takes for a data command; it refuses the others by name, so nothing
            // rests on this rule for them.
            FlashAddress {
                value: self.register(ADDR)
                    & u32::MAX.checked_shr(u32::BITS - bits).unwrap_or(u32::MAX),
                bits,
            }
        });
        let dummy_cycles = if user & USR_DUMMY == 0 {
            0
        } else {
            (user1 & DUMMY_CYCLES_MASK) + 1
        };
        let data_out: Vec<u8> = if user & USR_MOSI == 0 {
            Vec::new()
        } else {
            self.data_out(self.data_len(MOSI_DLEN))
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
        if let Some(data_in) = self.transfer(ctx, &command, data_in_len) {
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

    /// Runs the command whose bit is set in a write of `CMD`: one write, one transfer. Transfers
    /// complete at once, so `CMD` reads 0 afterwards.
    ///
    /// `CMD` bit 17, `FLASH_PE`, is not a command: "In user mode it is set to indicate that
    /// program/erase operation will be triggered. The bit is combined with `spi_mem_usr` bit" (ESP-IDF
    /// v5.5.2 `components/soc/esp32c3/register/soc/spi_mem_reg.h:113-116`). It accompanies the bit
    /// that starts the transfer, and not always: the rev3 ROM's `SPI_sector_erase` stores
    /// `0x01020000`, `FLASH_SE | FLASH_PE` (`lui a4,0x1020` at `0x4004d296`, store at `0x4004d29a`),
    /// and its page program `0x02000000`, `FLASH_PP` alone; ESP-IDF erases with `FLASH_SE` alone
    /// (`0x01000000`) and programs with `FLASH_PP` alone and `ADDR = len << 24 | addr` (measured:
    /// reports FW key result 1, SF key result 12). ESP-IDF's `spimem_flash_ll_user_start(dev,
    /// pe_ops)` ORs in `0x60000`, `USR | FLASH_PE`, only for the program and erase commands that
    /// take a 4-byte address, which it sends above 16 MiB
    /// (`components/hal/esp32c3/include/hal/spimem_flash_ll.h:391-395`,
    /// `components/spi_flash/spi_flash_chip_generic.c:118-151`; FW §B.1): no measured boot contains
    /// that transfer. This model has no use for the indication (a flash operation is never
    /// suspended), so the flag changes nothing: the command bit it comes with runs once, as
    /// without it.
    ///
    /// Several command bits in one write are refused by name and nothing is sent: no ROM or ESP-IDF
    /// code writes more than one, the order in which the controller would run them is not
    /// documented, and a guess would now write the flash.
    ///
    /// ASSUMPTION: `FLASH_PE` written without any command bit starts no transfer and self-clears. No
    /// ROM or ESP-IDF v5.5.2 code writes it alone; the SP1b-4 lockstep, or a device, settles it.
    fn run(&mut self, ctx: &mut Ctx, value: u32) {
        let commands = value & COMMAND_BITS;
        match commands.count_ones() {
            0 => {}
            1 if commands == CMD_USR => self.run_user(ctx),
            1 => {
                // INVARIANT: `HARD_WIRED` has an entry for each of the bits 31 to 19, and
                // `commands` is one of them.
                if let Some(&(bit, opcode)) = HARD_WIRED.iter().find(|(bit, _)| *bit == commands) {
                    self.run_hard_wired(ctx, bit, opcode);
                }
            }
            _ => ctx.unsupported(Unsupported::FlashCommandBits { cmd: value }),
        }
    }
}

impl Peripheral<Ctx> for Spi1 {
    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
        if offset == CMD {
            // Every command bit self-clears and the state nibbles read idle (FB §C rows 9 and 11).
            return Some(0);
        }
        self.registers.read(offset)
    }

    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        if offset == CMD {
            self.run(ctx, value);
            return true;
        }
        if offset == FLASH_SUS_CTRL {
            // Suspending is not modelled: an operation completes inside the write that starts
            // it (report FW §A.5), so there is never one to suspend. ESP-IDF writes this register
            // at every bus acquisition with every one of these bits clear and only a time-out
            // count (`0x0A000000`: FW §A.2), and refuses to enable auto-suspend on the modelled
            // chip (FW §B.2); a write that asks for a suspend or a resume is reported.
            if value & SUS_REQUESTS != 0 {
                ctx.unsupported(Unsupported::FlashSuspend { sus_ctrl: value });
            }
            // The two command bits clear themselves.
            return self.registers.write(offset, value & !SUS_COMMANDS);
        }
        self.registers.write(offset, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flash::{FlashConfig, FlashRefusal, NorFlash};
    use std::{cell::RefCell, rc::Rc};

    /// `CMD` bit 17, `FLASH_PE`: a flag that accompanies a command bit (see `Spi1::run`).
    const CMD_FLASH_PE: u32 = 1 << 17;

    fn spi1() -> Spi1 {
        Spi1::new(Box::new(NorFlash::blank(FlashConfig::default())))
    }
    /// A `USR` transfer of `opcode` reading `miso_bits` bits, as `bootloader_flash_execute_command_common` sets it up.
    fn user_read(ctx: &mut Ctx, spi: &mut Spi1, opcode: u32, miso_bits: u32) {
        spi.write(ctx, USER, USR_COMMAND | USR_MISO);
        spi.write(ctx, USER2, 0x7000_0000 | opcode);
        spi.write(ctx, MISO_DLEN, miso_bits - 1);
        spi.write(ctx, W0, 0);
        spi.write(ctx, CMD, CMD_USR);
    }

    #[test]
    fn configuration_registers_start_at_their_svd_reset_values() {
        let ctx = &mut Ctx::default();
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
            assert_eq!(spi.read(ctx, offset), Some(reset), "{register}");
        }
        assert_eq!(spi.read(ctx, CMD), Some(0));
    }

    #[test]
    fn rom_read_id_puts_the_jedec_bytes_in_w0_and_the_command_self_clears() {
        // FB §B.6: `esp_rom_spi_flash_update_id` writes W0 = 0, CMD = 0x10000000, polls CMD, reads W0.
        let ctx = &mut Ctx::default();
        let mut spi = spi1();
        spi.write(ctx, W0, 0);
        assert!(spi.write(ctx, CMD, 0x1000_0000));
        assert_eq!(
            spi.read(ctx, CMD),
            Some(0),
            "the ROM waits for the whole register to read 0"
        );
        assert_eq!(spi.read(ctx, W0), Some(0x0018_609D));
        assert_eq!(ctx.take_unsupported(), None);
    }

    #[test]
    fn rom_read_status_puts_the_status_byte_in_rd_status() {
        // FB §B.6: RD_STATUS[15:0] is zeroed, CMD = 0x08000000, then bit 0 (WIP) must read 0.
        let ctx = &mut Ctx::default();
        let mut spi = spi1();
        spi.write(ctx, RD_STATUS, 0xABCD_0000);
        spi.write(ctx, CMD, 0x0800_0000);
        assert_eq!(
            spi.read(ctx, RD_STATUS),
            Some(0xABCD_0000),
            "status 0, upper half kept"
        );
        spi.write(ctx, CMD, 0x4000_0000); // WREN
        spi.write(ctx, CMD, 0x0800_0000);
        assert_eq!(spi.read(ctx, RD_STATUS), Some(0xABCD_0002));
        spi.write(ctx, CMD, 0x2000_0000); // WRDI
        spi.write(ctx, CMD, 0x0800_0000);
        assert_eq!(spi.read(ctx, RD_STATUS), Some(0xABCD_0000));
    }

    #[test]
    fn rom_wake_up_runs_without_touching_the_data_registers() {
        // FB §B.4: `SPI_WakeUp` writes CMD = 0x00100000 (FLASH_RES) and polls CMD.
        let ctx = &mut Ctx::default();
        let mut spi = spi1();
        spi.write(ctx, W0, 0x1234_5678);
        spi.write(ctx, CMD, 0x0010_0000);
        assert_eq!(spi.read(ctx, CMD), Some(0));
        assert_eq!(spi.read(ctx, W0), Some(0x1234_5678));
        assert_eq!(ctx.take_unsupported(), None);
    }

    #[test]
    fn bootloader_user_commands_run_through_the_generic_path() {
        // BA §2.2, SPI1 row: RDID 0x9F (24 bits in), SFDP 0x5A at 0x10 with 8 dummy bits (8 bits in),
        // RESUME 0x7A, RDSR 0x05 (8 bits in), WRDI 0x04.
        let ctx = &mut Ctx::default();
        let mut spi = spi1();
        user_read(ctx, &mut spi, 0x9F, 24);
        assert_eq!(spi.read(ctx, W0), Some(0x0018_609D), "BA §2 row 46");
        spi.write(ctx, USER, USR_COMMAND | USR_ADDR | USR_DUMMY | USR_MISO);
        spi.write(ctx, USER1, (23 << 26) | 7);
        spi.write(ctx, USER2, 0x7000_005A);
        spi.write(ctx, ADDR, 0x10);
        spi.write(ctx, MISO_DLEN, 7);
        spi.write(ctx, CMD, CMD_USR);
        assert_eq!(spi.read(ctx, W0), Some(0xFF));
        for opcode in [0x7A, 0x04] {
            spi.write(ctx, USER, USR_COMMAND);
            spi.write(ctx, USER2, 0x7000_0000 | opcode);
            spi.write(ctx, CMD, CMD_USR);
        }
        user_read(ctx, &mut spi, 0x05, 8);
        assert_eq!(spi.read(ctx, W0), Some(0));
        assert_eq!(spi.read(ctx, CMD), Some(0));
        assert_eq!(ctx.take_unsupported(), None);
    }

    const CMD_WREN: u32 = 1 << 30;
    /// `W0..W15` hold the bytes 1 to 64.
    fn fill_data_registers(ctx: &mut Ctx, spi: &mut Spi1) {
        for word in 0..DATA_WORDS {
            spi.write(ctx, W0 + 4 * word, 0x0403_0201 + 0x0404_0404 * word);
        }
    }
    fn flash_bytes(spi: &Spi1, offset: u32, len: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; len];
        spi.flash().read(offset, &mut bytes);
        bytes
    }

    #[test]
    fn both_encodings_of_an_erase_and_of_a_program_write_the_chip_once_per_write_of_cmd() {
        // The ROM erases with `FLASH_SE | FLASH_PE` and programs 32 bytes at most (FW §A.3);
        // ESP-IDF erases with `FLASH_SE` alone and programs up to 64 (FW §A.2).
        let ctx = &mut Ctx::default();
        let mut spi = spi1();
        fill_data_registers(ctx, &mut spi);
        let pattern: Vec<u8> = (1..=64u8).collect();
        for (erase, len) in [(0x0102_0000u32, 32usize), (0x0100_0000, 64)] {
            spi.write(ctx, CMD, CMD_WREN);
            spi.write(ctx, ADDR, 0xE000);
            spi.write(ctx, CMD, erase);
            assert_eq!(flash_bytes(&spi, 0xE000, 68), vec![0xFF; 68]);
            spi.write(ctx, CMD, CMD_WREN);
            spi.write(ctx, ADDR, (u32::try_from(len).unwrap() << 24) | 0xE000);
            spi.write(ctx, CMD, CMD_PP);
            let content = flash_bytes(&spi, 0xE000, 68);
            assert_eq!(content[..len], pattern[..len]);
            assert!(content[len..].iter().all(|&byte| byte == 0xFF));
            assert_eq!(
                spi.take_flash_writes(),
                [
                    FlashWrite::Erase {
                        start: 0xE000,
                        len: 0x1000
                    },
                    FlashWrite::Program {
                        address: 0xE000,
                        data: pattern[..len].to_vec()
                    }
                ],
                "one chip operation for each write of CMD (erase {erase:#010x})"
            );
            spi.write(ctx, CMD, CMD_RDSR);
            assert_eq!(
                spi.read(ctx, RD_STATUS).map(|status| status & 0xFF),
                Some(0),
                "the latch is clear after the operation"
            );
        }
        assert_eq!(ctx.take_unsupported(), None);
        assert_eq!(spi.flash().modified_sectors(), 1);
    }

    #[test]
    fn a_program_or_an_erase_without_write_enable_changes_nothing_and_is_no_refusal() {
        // What the part does (FW §C): it ignores the command. Nothing is unfaithful here, so
        // nothing is reported.
        let ctx = &mut Ctx::default();
        let image = vec![0x5A; 0x2_0000];
        let mut spi = Spi1::new(Box::new(
            NorFlash::with_image(image.clone(), FlashConfig::default()).unwrap(),
        ));
        fill_data_registers(ctx, &mut spi);
        spi.write(ctx, ADDR, 0x0400_0000);
        for command in [CMD_PP, CMD_SE, CMD_BE, 1 << 22, CMD_WRSR] {
            assert!(spi.write(ctx, CMD, command));
            assert_eq!(spi.read(ctx, CMD), Some(0));
        }
        assert_eq!(flash_bytes(&spi, 0, 0x2_0000), image);
        assert_eq!(spi.take_flash_writes(), []);
        assert_eq!(ctx.take_unsupported(), None);
        assert_eq!(ctx.take_signal(), None, "the cache is told of nothing");
    }

    #[test]
    fn a_command_that_changes_the_flash_tells_the_cache_which_pages_and_no_other_does() {
        const CMD_CE: u32 = 1 << 22;
        let ctx = &mut Ctx::default();
        let mut spi = Spi1::new(Box::new(
            NorFlash::with_image(vec![0x5A; 0x4_0000], FlashConfig::default()).unwrap(),
        ));
        let written = |first_page, last_page| {
            Some(Event::FlashWritten {
                first_page,
                last_page,
            })
        };
        fill_data_registers(ctx, &mut spi);
        // Four bytes from 0x2FFFE: they wrap in their 256-byte page, so stay in 64 KiB page 2.
        spi.write(ctx, ADDR, 0x0402_FFFE);
        spi.write(ctx, CMD, CMD_WREN);
        assert_eq!(ctx.take_signal(), None, "the latch is no content");
        spi.write(ctx, CMD, CMD_PP);
        assert_eq!(ctx.take_signal(), written(2, 2));
        assert_eq!(ctx.take_signal(), None, "one command, one event");
        // The same program again changes no byte: no line of a cache can be stale of it.
        spi.write(ctx, CMD, CMD_WREN);
        spi.write(ctx, CMD, CMD_PP);
        assert_eq!(ctx.take_signal(), None);
        assert_eq!(spi.take_flash_writes().len(), 2, "both are in the journal");
        // The last sector of page 1, then the 64 KiB block that is page 3.
        spi.write(ctx, CMD, CMD_WREN);
        spi.write(ctx, ADDR, 0x1_F000);
        spi.write(ctx, CMD, CMD_SE);
        assert_eq!(ctx.take_signal(), written(1, 1));
        spi.write(ctx, CMD, CMD_WREN);
        spi.write(ctx, ADDR, 0x3_0000);
        spi.write(ctx, CMD, CMD_BE);
        assert_eq!(ctx.take_signal(), written(3, 3));
        // A status write changes no content; neither does a read.
        spi.write(ctx, CMD, CMD_WREN);
        spi.write(ctx, RD_STATUS, 0);
        spi.write(ctx, CMD, CMD_WRSR);
        spi.write(ctx, CMD, CMD_RDSR);
        spi.write(ctx, CMD, CMD_RDID);
        assert_eq!(ctx.take_signal(), None);
        // A chip erase: every page of the chip.
        spi.write(ctx, CMD, CMD_WREN);
        spi.write(ctx, CMD, CMD_CE);
        let last_page = FlashConfig::default().size() / MMU_PAGE_BYTES - 1;
        assert_eq!(ctx.take_signal(), written(0, last_page));
        assert_eq!(ctx.take_unsupported(), None);
    }

    #[test]
    fn the_pages_of_a_write_are_those_of_its_bytes_whatever_its_numbers() {
        let program = |address| FlashWrite::Program {
            address,
            data: vec![0; 64],
        };
        let erase = |start, len| FlashWrite::Erase { start, len };
        assert_eq!(cache_pages(&program(0)), Some((0, 0)));
        assert_eq!(
            cache_pages(&program(0xFFFF)),
            Some((0, 0)),
            "wraps in its page"
        );
        assert_eq!(cache_pages(&program(0xFF_FFC0)), Some((255, 255)));
        assert_eq!(cache_pages(&erase(0xE000, 0x1000)), Some((0, 0)));
        assert_eq!(cache_pages(&erase(0x1_0000, 0x1_0000)), Some((1, 1)));
        assert_eq!(cache_pages(&erase(0xF000, 0x2000)), Some((0, 1)));
        assert_eq!(cache_pages(&erase(0, 0x100_0000)), Some((0, 255)));
        assert_eq!(cache_pages(&FlashWrite::Status { value: 0x40 }), None);
        // Numbers no chip produces must not overflow.
        assert_eq!(
            cache_pages(&erase(u32::MAX, u32::MAX)),
            Some((0xFFFF, 0xFFFF))
        );
        assert_eq!(cache_pages(&erase(5, 0)), Some((0, 0)));
    }

    #[test]
    fn a_refusal_of_the_chip_is_reported_as_what_it_is_and_cmd_reads_idle() {
        let ctx = &mut Ctx::default();
        let mut spi = spi1();
        // High-performance mode, `A3`: an opcode the chip model does not know.
        assert!(spi.write(ctx, CMD, 1 << 19));
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::FlashCommand { opcode: 0xA3 })
        );
        assert_eq!(ctx.take_unsupported(), None, "taken once");
        assert_eq!(spi.read(ctx, CMD), Some(0), "no hang on a refused command");
        // A program while block-protect bits are set: status 0x3C, written through the registers.
        spi.write(ctx, CMD, CMD_WREN);
        spi.write(ctx, RD_STATUS, 0x3C);
        spi.write(ctx, CMD, CMD_WRSR);
        spi.write(ctx, CMD, CMD_WREN);
        spi.write(ctx, ADDR, 0x0100_9000);
        spi.write(ctx, CMD, CMD_PP);
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::FlashProtected {
                opcode: 0x02,
                status: 0x3E
            })
        );
        // A program through `USR` with a 4-byte address: the frame ESP-IDF uses above 16 MiB,
        // with the 3-byte opcode.
        spi.write(ctx, USER, USR_COMMAND | USR_ADDR | USR_MOSI);
        spi.write(ctx, USER1, 31 << 26);
        spi.write(ctx, USER2, 0x7000_0002);
        spi.write(ctx, MOSI_DLEN, 31);
        spi.write(ctx, CMD, CMD_USR);
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::FlashCommandForm {
                opcode: 0x02,
                address_bits: 32,
                dummy_cycles: 0,
                data_bytes: 4
            })
        );
        assert_eq!(
            spi.take_flash_writes(),
            [FlashWrite::Status { value: 0x3C }],
            "only the status write was performed"
        );
        // An erase past the end of a 1 MiB chip.
        let small = FlashConfig::new(1 << 20, crate::flash::JedecId::from_u24(0x9D_6014).unwrap());
        let mut spi = Spi1::new(Box::new(NorFlash::blank(small.unwrap())));
        spi.write(ctx, CMD, CMD_WREN);
        spi.write(ctx, ADDR, 0x10_0000);
        spi.write(ctx, CMD, CMD_SE);
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::FlashRange {
                opcode: 0x20,
                address: 0x10_0000,
                len: 0x1000
            })
        );
        assert_eq!(ctx.take_unsupported(), None);
    }

    #[test]
    fn a_reset_of_the_controller_keeps_the_chip_and_the_journal_of_the_run() {
        let ctx = &mut Ctx::default();
        let mut spi = spi1();
        spi.write(ctx, CMD, CMD_WREN);
        spi.write(ctx, W0, 0x1234_5600);
        spi.write(ctx, ADDR, 0x0100_9000);
        spi.write(ctx, CMD, CMD_PP);
        spi.write(ctx, CMD, CMD_WREN);
        spi.reset();
        assert_eq!(spi.read(ctx, ADDR), Some(0), "the registers are reset");
        assert_eq!(flash_bytes(&spi, 0x9000, 1), [0x00], "the content is kept");
        spi.write(ctx, CMD, CMD_RDSR);
        assert_eq!(
            spi.read(ctx, RD_STATUS),
            Some(0x02),
            "the chip is another component: its latch is as the firmware left it"
        );
        assert_eq!(
            spi.take_flash_writes(),
            [FlashWrite::Program {
                address: 0x9000,
                data: vec![0x00]
            }]
        );
    }

    #[test]
    fn unknown_user_opcode_is_reported() {
        let ctx = &mut Ctx::default();
        let mut spi = spi1();
        spi.write(ctx, W0, 0xDEAD_BEEF);
        spi.write(ctx, USER, USR_COMMAND | USR_MISO);
        spi.write(ctx, USER2, 0x7000_0035); // a second status register this part does not have
        spi.write(ctx, MISO_DLEN, 7);
        spi.write(ctx, CMD, CMD_USR);
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::FlashCommand { opcode: 0x35 })
        );
        assert_eq!(
            spi.read(ctx, W0),
            Some(0xDEAD_BEEF),
            "nothing was clocked in"
        );
    }

    #[test]
    fn transfer_lengths_are_bounded_by_the_sixteen_data_registers() {
        // MISO_DLEN can ask for 1024 bits (128 bytes); the data buffer is W0..W15, 64 bytes.
        let ctx = &mut Ctx::default();
        let mut spi = spi1();
        user_read(ctx, &mut spi, 0x9F, 1024);
        let w15 = svd_regs::esp32c3::addr("SPI1", "W15") - SPI1_BASE;
        // 64 bytes of the repeating id 9D 60 18: byte 60 is 9D (60 % 3 == 0).
        assert_eq!(spi.read(ctx, w15), Some(0x9D18_609D));
        assert_eq!(
            spi.read(ctx, w15 + 4),
            Some(0x14),
            "FLASH_WAITI_CTRL keeps its reset value: nothing was stored past W15"
        );
    }

    #[test]
    fn a_read_shorter_than_a_word_zeroes_the_rest_of_that_word() {
        let ctx = &mut Ctx::default();
        let mut spi = spi1();
        spi.write(ctx, W0, 0xFFFF_FFFF);
        user_read(ctx, &mut spi, 0x9F, 16);
        assert_eq!(spi.read(ctx, W0), Some(0x0000_609D));
    }

    /// What a flash saw: `(opcode, address, dummy cycles, bytes sent)`.
    type Seen = (u8, Option<FlashAddress>, u32, Vec<u8>);

    /// A 24-bit address, the width of every hard-wired command.
    const fn at(value: u32) -> FlashAddress {
        FlashAddress { value, bits: 24 }
    }

    /// A chip that accepts every command and remembers it, to look at what the controller sends.
    /// It answers `A0 A1 A2 ...` to whatever is read.
    #[derive(Debug)]
    struct Recorder(Rc<RefCell<Vec<Seen>>>);

    impl FlashDevice for Recorder {
        fn size(&self) -> u32 {
            FlashConfig::default().size()
        }
        fn read(&self, _offset: u32, out: &mut [u8]) {
            out.fill(0xFF);
        }
        fn execute(
            &mut self,
            command: &FlashCommand<'_>,
            data_in: &mut [u8],
        ) -> Result<Option<FlashWrite>, FlashRefusal> {
            for (byte, answer) in data_in.iter_mut().zip(0xA0u8..) {
                *byte = answer;
            }
            self.0.borrow_mut().push((
                command.opcode,
                command.address,
                command.dummy_cycles,
                command.data_out.to_vec(),
            ));
            Ok(None)
        }
        fn generation(&self) -> u64 {
            0
        }
        fn modified_sectors(&self) -> u32 {
            0
        }
    }

    fn recording_spi1() -> (Spi1, Rc<RefCell<Vec<Seen>>>) {
        let seen = Rc::new(RefCell::new(Vec::new()));
        (Spi1::new(Box::new(Recorder(Rc::clone(&seen)))), seen)
    }

    #[test]
    fn one_write_of_cmd_with_several_command_bits_runs_none_and_says_so() {
        let ctx = &mut Ctx::default();
        let (mut spi, seen) = recording_spi1();
        spi.write(ctx, ADDR, 0x0400_9000);
        spi.write(ctx, USER, USR_COMMAND);
        spi.write(ctx, USER2, 0x7000_0006);
        for cmd in [
            CMD_PP | CMD_SE,
            CMD_BE | CMD_USR,
            COMMAND_BITS,
            (1 << 30) | (1 << 29) | CMD_FLASH_PE,
        ] {
            assert!(spi.write(ctx, CMD, cmd), "handled: reported, not dropped");
            assert_eq!(
                ctx.take_unsupported(),
                Some(Unsupported::FlashCommandBits { cmd })
            );
            assert_eq!(ctx.take_unsupported(), None, "one write, one report");
            assert_eq!(spi.read(ctx, CMD), Some(0), "self-clears: no hang");
        }
        assert_eq!(seen.borrow().as_slice(), [], "nothing reached the chip");
        // The flag and the read-only state bits [7:0] are not commands: beside one command bit
        // they change nothing.
        assert!(spi.write(ctx, CMD, CMD_SE | CMD_FLASH_PE | 0xFF));
        assert_eq!(seen.borrow().len(), 1);
        assert_eq!(ctx.take_unsupported(), None);
    }

    #[test]
    fn a_hard_wired_program_sends_its_address_and_the_bytes_its_count_names() {
        // The ROM's `otadata` program (FW §A.3: `ADDR = 0x2000E000`, `W0..W7`, `CMD = 0x02000000`)
        // and ESP-IDF's (SF §B.2: `0x04009020` is 4 bytes at 0x9020; 64 bytes at most, FW §A.2).
        let ctx = &mut Ctx::default();
        let (mut spi, seen) = recording_spi1();
        for word in 0..DATA_WORDS {
            spi.write(ctx, W0 + 4 * word, 0x0403_0201 + 0x0404_0404 * word);
        }
        for (addr, address, len) in [
            (0x2000_E000u32, 0xE000u32, 32usize),
            (0x0400_9020, 0x9020, 4),
            (0x40FF_FF00, 0xFF_FF00, 64),
            (0x0100_0000, 0, 1),
        ] {
            spi.write(ctx, ADDR, addr);
            assert!(spi.write(ctx, CMD, CMD_PP));
            let last = seen.borrow().last().cloned();
            let bytes: Vec<u8> = (1..=64u8).take(len).collect();
            assert_eq!(
                last,
                Some((0x02, Some(at(address)), 0, bytes)),
                "ADDR {addr:#010x}"
            );
        }
        assert_eq!(seen.borrow().len(), 4, "one write of CMD, one transfer");
        assert_eq!(ctx.take_unsupported(), None);
        assert_eq!(spi.read(ctx, CMD), Some(0));
    }

    #[test]
    fn a_hard_wired_program_of_no_byte_or_of_more_than_64_is_refused_and_sends_nothing() {
        let ctx = &mut Ctx::default();
        let (mut spi, seen) = recording_spi1();
        for addr in [0x0000_9000u32, 0x4100_9000, 0xFF00_9000] {
            spi.write(ctx, ADDR, addr);
            assert!(spi.write(ctx, CMD, CMD_PP));
            assert_eq!(
                ctx.take_unsupported(),
                Some(Unsupported::FlashProgramLength { addr }),
                "ADDR {addr:#010x}"
            );
            assert_eq!(ctx.take_unsupported(), None);
            assert_eq!(spi.read(ctx, CMD), Some(0), "self-clears: no hang");
        }
        assert_eq!(seen.borrow().as_slice(), [], "nothing reached the chip");
    }

    #[test]
    fn sector_and_block_erase_send_the_low_24_bits_of_addr_with_or_without_the_flag() {
        // ROM: `CMD = 0x01020000` and `0x00820000` (FW §A.3); ESP-IDF: `0x01000000` and
        // `0x00800000` (FW §A.2, SF §D).
        let ctx = &mut Ctx::default();
        for (cmd, opcode) in [
            (0x0102_0000u32, 0x20u8),
            (0x0100_0000, 0x20),
            (0x0082_0000, 0xD8),
            (0x0080_0000, 0xD8),
        ] {
            let (mut spi, seen) = recording_spi1();
            // Bits 31:24 belong to the byte count of a program, not to an erase address.
            spi.write(ctx, ADDR, 0xAB00_E000);
            assert!(spi.write(ctx, CMD, cmd));
            assert_eq!(
                seen.borrow().as_slice(),
                [(opcode, Some(at(0xE000)), 0, Vec::new())],
                "CMD {cmd:#010x}"
            );
            assert_eq!(ctx.take_unsupported(), None);
            assert_eq!(spi.read(ctx, CMD), Some(0));
        }
    }

    #[test]
    fn chip_erase_and_the_latch_commands_send_an_opcode_and_nothing_else() {
        let ctx = &mut Ctx::default();
        let (mut spi, seen) = recording_spi1();
        spi.write(ctx, ADDR, 0x2000_E000);
        for (cmd, opcode) in [(1u32 << 22, 0xC7u8), (1 << 30, 0x06), (1 << 29, 0x04)] {
            spi.write(ctx, CMD, cmd);
            let last = seen.borrow().last().cloned();
            assert_eq!(last, Some((opcode, None, 0, Vec::new())), "CMD {cmd:#010x}");
        }
    }

    #[test]
    fn the_status_write_sends_one_byte_of_rd_status_or_two_when_ctrl_says_so() {
        // `esp_rom_spiflash_write_status`: `RD_STATUS[15:0]` = the status, `CMD = 0x04000000`.
        let ctx = &mut Ctx::default();
        let (mut spi, seen) = recording_spi1();
        spi.write(ctx, RD_STATUS, 0xFFFF_3C9C);
        spi.write(ctx, CMD, CMD_WRSR);
        let ctrl = spi.register(CTRL);
        spi.write(ctx, CTRL, ctrl | CTRL_WRSR_2B);
        spi.write(ctx, CMD, CMD_WRSR);
        assert_eq!(
            seen.borrow().as_slice(),
            [
                (0x01, None, 0, vec![0x9C]),
                (0x01, None, 0, vec![0x9C, 0x3C])
            ]
        );
    }

    #[test]
    fn the_hard_wired_read_sends_its_address_and_stores_what_the_chip_answers() {
        // Pins the ASSUMPTION documented in `Spi1::run_hard_wired`.
        let ctx = &mut Ctx::default();
        let (mut spi, seen) = recording_spi1();
        spi.write(ctx, ADDR, 0x7712_3456);
        spi.write(ctx, MISO_DLEN, 8 * 6 - 1);
        spi.write(ctx, W0 + 4, 0xFFFF_FFFF);
        assert!(spi.write(ctx, CMD, CMD_READ));
        assert_eq!(
            seen.borrow().as_slice(),
            [(0x03, Some(at(0x12_3456)), 0, Vec::new())]
        );
        assert_eq!(spi.read(ctx, W0), Some(0xA3A2_A1A0));
        assert_eq!(spi.read(ctx, W0 + 4), Some(0x0000_A5A4));
        assert_eq!(spi.read(ctx, CMD), Some(0));
        // On the chip it is the slow read: the content at that address.
        let mut chip = spi_with_image((0u8..=255).cycle().take(0x200).collect());
        chip.write(ctx, ADDR, 0x101);
        chip.write(ctx, MISO_DLEN, 8 * 4 - 1);
        chip.write(ctx, CMD, CMD_READ);
        assert_eq!(chip.read(ctx, W0), Some(0x0403_0201));
        assert_eq!(ctx.take_unsupported(), None);
    }

    fn spi_with_image(image: Vec<u8>) -> Spi1 {
        Spi1::new(Box::new(
            NorFlash::with_image(image, FlashConfig::default()).unwrap(),
        ))
    }

    /// The registers of ESP-IDF's dual I/O read of `bytes` bytes at `address` (SF §C, measured):
    /// `USER2 = 0x700000BB`, `USER1 = 0x5C000003` (24 address bits, 4 dummy clocks),
    /// `USER = 0xF0000080`, `ADDR` = the flash offset, `MISO_DLEN = 8 × bytes − 1`.
    fn dio_read(ctx: &mut Ctx, spi: &mut Spi1, address: u32, bytes: u32) {
        spi.write(ctx, USER2, 0x7000_00BB);
        spi.write(ctx, USER1, 0x5C00_0003);
        spi.write(ctx, USER, 0xF000_0080);
        spi.write(ctx, ADDR, address);
        spi.write(ctx, MISO_DLEN, 8 * bytes - 1);
        spi.write(ctx, CMD, CMD_USR);
    }

    #[test]
    fn the_measured_dual_io_read_fills_the_data_registers_from_the_flash() {
        let ctx = &mut Ctx::default();
        let image: Vec<u8> = (0u8..=255).cycle().take(0xA000).collect();
        let mut spi = spi_with_image(image.clone());
        let words = |spi: &mut Spi1, ctx: &mut Ctx| -> Vec<u8> {
            (0..DATA_WORDS)
                .flat_map(|word| spi.read(ctx, W0 + 4 * word).unwrap().to_le_bytes())
                .collect()
        };
        // 64, 32 and 4 bytes: `MISO_DLEN` 0x1FF, 0xFF and 0x1F, the three lengths NVS reads.
        dio_read(ctx, &mut spi, 0x9000, 64);
        assert_eq!(words(&mut spi, ctx), image[0x9000..0x9040]);
        dio_read(ctx, &mut spi, 0x9123, 32);
        assert_eq!(words(&mut spi, ctx)[..32], image[0x9123..0x9143]);
        dio_read(ctx, &mut spi, 0x9020, 4);
        assert_eq!(spi.read(ctx, W0), Some(0x2322_2120));
        assert_eq!(ctx.take_unsupported(), None);
        assert_eq!(spi.read(ctx, CMD), Some(0));
        assert_eq!(spi.take_flash_writes(), [], "a read writes nothing");
    }

    #[test]
    fn a_read_the_chip_cannot_place_or_frame_is_refused_by_name_and_clocks_nothing_in() {
        let ctx = &mut Ctx::default();
        let mut spi = spi_with_image(vec![0x11; 64]);
        spi.write(ctx, W0, 0xDEAD_BEEF);
        // 64 bytes from the last byte of the chip: 63 of them are past its end.
        dio_read(ctx, &mut spi, 0xFF_FFFF, 64);
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::FlashRange {
                opcode: 0xBB,
                address: 0xFF_FFFF,
                len: 64
            })
        );
        // The same read with 8 dummy clocks, which `BB` does not take.
        spi.write(ctx, USER2, 0x7000_00BB);
        spi.write(ctx, USER1, 0x5C00_0007);
        spi.write(ctx, USER, 0xF000_0080);
        spi.write(ctx, ADDR, 0);
        spi.write(ctx, MISO_DLEN, 31);
        spi.write(ctx, CMD, CMD_USR);
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::FlashCommandForm {
                opcode: 0xBB,
                address_bits: 24,
                dummy_cycles: 8,
                data_bytes: 0
            })
        );
        assert_eq!(ctx.take_unsupported(), None);
        assert_eq!(
            spi.read(ctx, W0),
            Some(0xDEAD_BEEF),
            "nothing was clocked in"
        );
    }

    #[test]
    fn a_suspend_or_resume_request_is_refused_by_name_and_the_measured_configuration_is_stored() {
        let ctx = &mut Ctx::default();
        let (mut spi, seen) = recording_spi1();
        // What ESP-IDF writes at every bus acquisition (FW §A.2): no enable bit, a time-out count.
        assert!(spi.write(ctx, FLASH_SUS_CTRL, 0x0A00_0000));
        assert_eq!(spi.read(ctx, FLASH_SUS_CTRL), Some(0x0A00_0000));
        // The two wait enables ask for nothing by themselves.
        assert!(spi.write(ctx, FLASH_SUS_CTRL, 0b1100));
        assert_eq!(ctx.take_unsupported(), None);
        for (value, stored) in [
            (1u32 << 5, 1u32 << 5), // FLASH_PES_EN: auto-suspend
            (1 << 4, 1 << 4),       // PES_PER_EN: auto-resume
            (0b10, 0),              // FLASH_PES: send a suspend
            (0b01, 0),              // FLASH_PER: send a resume
            (0x0A00_0023, 0x0A00_0020),
        ] {
            assert!(spi.write(ctx, FLASH_SUS_CTRL, value));
            assert_eq!(
                ctx.take_unsupported(),
                Some(Unsupported::FlashSuspend { sus_ctrl: value }),
                "{value:#x}"
            );
            assert_eq!(
                spi.read(ctx, FLASH_SUS_CTRL),
                Some(stored),
                "the two command bits clear themselves"
            );
        }
        assert_eq!(
            seen.borrow().as_slice(),
            [],
            "no suspend or resume opcode is sent"
        );
    }

    #[test]
    fn a_mosi_length_past_the_buffer_sends_the_sixty_four_bytes_of_w0_to_w15() {
        let ctx = &mut Ctx::default();
        let (mut spi, seen) = recording_spi1();
        for word in 0..DATA_WORDS {
            spi.write(ctx, W0 + 4 * word, 0x0403_0201 + 0x0404_0404 * word);
        }
        spi.write(ctx, USER, USR_COMMAND | USR_MOSI);
        spi.write(ctx, USER2, 0x7000_0002);
        spi.write(ctx, MOSI_DLEN, 1023);
        spi.write(ctx, CMD, CMD_USR);
        let seen = seen.borrow();
        assert_eq!(seen.len(), 1);
        let sent = &seen[0].3;
        assert_eq!(sent.len(), DATA_BYTES, "1024 bits asked, 64 bytes exist");
        assert_eq!(
            sent[..4],
            [1, 2, 3, 4],
            "W0 first, least significant byte first"
        );
        assert_eq!(sent[60..], [0x3D, 0x3E, 0x3F, 0x40], "W15 last");
    }

    #[test]
    fn the_address_is_the_low_bits_of_addr_as_wide_as_user1_says() {
        let ctx = &mut Ctx::default();
        let (mut spi, seen) = recording_spi1();
        spi.write(ctx, USER, USR_COMMAND | USR_ADDR);
        spi.write(ctx, USER2, 0x7000_005A);
        spi.write(ctx, ADDR, 0xDEAD_BEEF);
        // (field value, address the flash must see, on how many bits): the field is the bit
        // length minus one.
        for (field, value, bits) in [
            (0, 0x1, 1),
            (7, 0xEF, 8),
            (23, 0xAD_BEEF, 24),
            (31, 0xDEAD_BEEF, 32),
            (63, 0xDEAD_BEEF, 32), // 64 bits asked, the register holds 32
        ] {
            spi.write(ctx, USER1, field << 26);
            spi.write(ctx, CMD, CMD_USR);
            let last = seen.borrow().last().cloned();
            assert_eq!(
                last.map(|s| s.1),
                Some(Some(FlashAddress { value, bits })),
                "address field {field}"
            );
        }
    }

    #[test]
    fn a_user_transfer_without_an_8_bit_command_phase_is_reported_and_sends_nothing() {
        let ctx = &mut Ctx::default();
        for (user, user2, command_bits) in [
            (USR_MISO, 0x7000_009F, None), // no command phase
            (USR_COMMAND | USR_MISO, 0xF000_9F00, Some(16)),
            (USR_COMMAND | USR_MISO, 0x0000_009F, Some(1)),
            (USR_COMMAND | USR_MISO, 0x6000_009F, Some(7)),
            (USR_COMMAND | USR_MISO, 0x8000_009F, Some(9)),
        ] {
            let (mut spi, seen) = recording_spi1();
            spi.write(ctx, W0, 0xDEAD_BEEF);
            spi.write(ctx, USER, user);
            spi.write(ctx, USER2, user2);
            spi.write(ctx, MISO_DLEN, 23);
            assert!(
                spi.write(ctx, CMD, CMD_USR),
                "handled: reported, not dropped"
            );
            assert_eq!(
                seen.borrow().as_slice(),
                [],
                "USER {user:#x} USER2 {user2:#x}"
            );
            assert_eq!(
                ctx.take_unsupported(),
                Some(Unsupported::FlashTransfer {
                    command_bits,
                    user,
                    user2
                })
            );
            assert_eq!(ctx.take_unsupported(), None);
            assert_eq!(
                spi.read(ctx, W0),
                Some(0xDEAD_BEEF),
                "nothing was clocked in"
            );
            assert_eq!(spi.read(ctx, CMD), Some(0), "self-clears: no hang");
        }
    }

    #[test]
    fn data_lengths_round_up_to_whole_bytes() {
        // Pins the ASSUMPTION documented on `Spi1::data_len`: 9 bits asked, 2 bytes transferred.
        let ctx = &mut Ctx::default();
        let (mut spi, seen) = recording_spi1();
        spi.write(ctx, W0, 0x0403_0201);
        spi.write(ctx, USER, USR_COMMAND | USR_MOSI);
        spi.write(ctx, USER2, 0x7000_0002);
        spi.write(ctx, MOSI_DLEN, 8);
        spi.write(ctx, CMD, CMD_USR);
        assert_eq!(seen.borrow()[0].3, [1, 2]);
        let mut read = spi1();
        read.write(ctx, W0, 0xFFFF_FFFF);
        user_read(ctx, &mut read, 0x9F, 9);
        assert_eq!(read.read(ctx, W0), Some(0x0000_609D), "two bytes of the id");
    }

    #[test]
    fn the_rom_sector_erase_sets_the_program_erase_flag_and_is_one_command() {
        // The rev3 ROM's `SPI_sector_erase` stores CMD = 0x01020000, `FLASH_SE | FLASH_PE`
        // (`lui a4,0x1020` at 0x4004d296, store at 0x4004d29a).
        const ROM_SECTOR_ERASE: u32 = 0x0102_0000;
        assert_eq!(ROM_SECTOR_ERASE, (1 << 24) | CMD_FLASH_PE);
        let ctx = &mut Ctx::default();
        let (mut spi, seen) = recording_spi1();
        spi.write(ctx, ADDR, 0xE000);
        assert!(spi.write(ctx, CMD, ROM_SECTOR_ERASE));
        assert_eq!(
            seen.borrow().as_slice(),
            [(0x20, Some(at(0xE000)), 0, Vec::new())]
        );
        assert_eq!(spi.read(ctx, CMD), Some(0), "self-clears");
        // On the chip it is one erase: `USER2` (0x00 in its low byte at reset) is not a second
        // command, and nothing is reported.
        let mut chip = spi1();
        chip.write(ctx, CMD, CMD_WREN);
        chip.write(ctx, ADDR, 0xE000);
        chip.write(ctx, CMD, ROM_SECTOR_ERASE);
        assert_eq!(
            chip.take_flash_writes(),
            [FlashWrite::Erase {
                start: 0xE000,
                len: 0x1000
            }]
        );
        assert_eq!(ctx.take_unsupported(), None);
    }

    #[test]
    fn a_user_transfer_that_carries_the_program_erase_flag_runs_once() {
        // `spimem_flash_ll_user_start(dev, pe_ops = true)` ORs 0x60000 into CMD: `USR | FLASH_PE`.
        // ESP-IDF v5.5.2 does it for the program and erase commands that take a 4-byte address,
        // above 16 MiB (`spi_flash_chip_generic.c:118-151`); below, its program is the hard-wired
        // `CMD = 0x02000000` (SF key result 12). What is pinned here is the flag beside `USR`,
        // with a transfer the chip of this test understands: a 3-byte-address program.
        const USER_START_PE: u32 = 0x0006_0000;
        assert_eq!(USER_START_PE, CMD_USR | CMD_FLASH_PE);
        let ctx = &mut Ctx::default();
        let page_program = |ctx: &mut Ctx, spi: &mut Spi1| {
            spi.write(ctx, W0, 0x0403_0201);
            spi.write(ctx, USER, USR_COMMAND | USR_ADDR | USR_MOSI);
            spi.write(ctx, USER1, 23 << 26);
            spi.write(ctx, USER2, 0x7000_0002);
            spi.write(ctx, ADDR, 0xE000);
            spi.write(ctx, MOSI_DLEN, 31);
            spi.write(ctx, CMD, USER_START_PE);
        };
        let (mut spi, seen) = recording_spi1();
        page_program(ctx, &mut spi);
        assert_eq!(
            seen.borrow().as_slice(),
            [(0x02, Some(at(0xE000)), 0, vec![1, 2, 3, 4])],
            "one transfer, with its address and its data"
        );
        // On the chip, once its latch is set: one program.
        let mut chip = spi1();
        chip.write(ctx, CMD, CMD_WREN);
        page_program(ctx, &mut chip);
        assert_eq!(
            chip.take_flash_writes(),
            [FlashWrite::Program {
                address: 0xE000,
                data: vec![1, 2, 3, 4]
            }]
        );
        assert_eq!(ctx.take_unsupported(), None);
    }

    #[test]
    fn the_program_erase_flag_alone_starts_no_command() {
        // Pins the ASSUMPTION documented on `Spi1::run`.
        let ctx = &mut Ctx::default();
        let (mut spi, seen) = recording_spi1();
        spi.write(ctx, USER2, 0x7000_0020);
        assert!(spi.write(ctx, CMD, CMD_FLASH_PE));
        assert_eq!(seen.borrow().as_slice(), []);
        assert_eq!(spi.read(ctx, CMD), Some(0), "self-clears");
        assert_eq!(ctx.take_unsupported(), None);
    }

    #[test]
    fn spi0_stores_the_read_mode_the_rom_programs_and_reads_idle() {
        // FB §B.6: SPI0.CTRL ends at 0x00AC2008; the bootloader decodes it to print "SPI Mode : DIO".
        let ctx = &mut Ctx::default();
        let mut spi0 = Spi0::new();
        let ctrl = svd_regs::esp32c3::addr("SPI0", "CTRL") - SPI0_BASE;
        assert_eq!(spi0.read(ctx, ctrl), Some(0x002C_2000));
        assert!(spi0.write(ctx, ctrl, 0x00AC_2008));
        assert_eq!(spi0.read(ctx, ctrl), Some(0x00AC_2008));
        let fsm = svd_regs::esp32c3::addr("SPI0", "FSM") - SPI0_BASE;
        assert_eq!(
            spi0.read(ctx, fsm).map(|v| v & 0x70),
            Some(0),
            "Wait_SPI_Idle: bits 6:4 (FB §C row 10)"
        );
        assert!(!spi0.write(ctx, fsm, 0), "FSM is read-only");
        for offset in [SPI0_MOSI_DLEN - SPI0_BASE, SPI0_MISO_DLEN - SPI0_BASE] {
            assert_eq!(spi0.read(ctx, offset), Some(0));
            assert!(
                spi0.write(ctx, offset, 0xFF),
                "FB §B.2: 256 bits, one cache line"
            );
            assert_eq!(spi0.read(ctx, offset), Some(0xFF));
        }
    }

    #[test]
    fn the_registers_the_flash_driver_configures_are_stored_from_their_svd_reset_values() {
        // What `spi_flash_hal_device_config` writes on SPI1 and `spimem_flash_ll_get_source_freq_mhz`
        // reads on SPI0 (trace F1). A row removed from the accepted lists makes its register
        // unmodelled, and its read here `None`.
        let ctx = &mut Ctx::default();
        let reset = |peripheral: &str, name: &str| {
            svd_regs::esp32c3::lookup(svd_regs::esp32c3::addr(peripheral, name))
                .unwrap()
                .reset_value
        };
        let mut spi1 = spi1();
        for name in ["MISC", "FLASH_SUS_CTRL", "FLASH_WAITI_CTRL"] {
            let offset = svd_regs::esp32c3::addr("SPI1", name) - SPI1_BASE;
            assert_eq!(spi1.read(ctx, offset), Some(reset("SPI1", name)), "{name}");
            assert!(spi1.write(ctx, offset, 0x0A00_0000), "{name}");
            assert_eq!(spi1.read(ctx, offset), Some(0x0A00_0000), "{name}");
        }
        let mut spi0 = Spi0::new();
        for name in ["CORE_CLK_SEL", "MISC"] {
            let offset = svd_regs::esp32c3::addr("SPI0", name) - SPI0_BASE;
            assert_eq!(spi0.read(ctx, offset), Some(reset("SPI0", name)), "{name}");
            assert!(spi0.write(ctx, offset, 0x1), "{name}");
            assert_eq!(spi0.read(ctx, offset), Some(0x1), "{name}");
        }
    }
}
