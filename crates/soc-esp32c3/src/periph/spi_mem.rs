//! SPI0 and SPI1, the two controllers that share the flash chip (`spi_mem_reg.h`; reports BC §D.1,
//! FB §B and §D). SPI1 runs the commands software asks for; SPI0 refills the cache, which this
//! emulator does not model (the bus reads the flash directly), so SPI0 is configuration storage.
use super::mmu::MMU_PAGE_BYTES;
use super::{Accepted, SPI0_BASE, SPI1_BASE, Storage};
use crate::ctx::{Ctx, Event};
use crate::diag::Unsupported;
use crate::flash::{
    FlashAddress, FlashCommand, FlashDevice, FlashJournal, FlashWrite, erases_whole_chip,
};
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
///
/// ASSUMPTION: `FLASH_CE` (bit 22) sends `C7`. FW §A.1 gives `60` for it, as QEMU's controller
/// sends it, and says that the opcode of a hard-wired bit is fixed by the controller on silicon
/// and not documented; `C7` is ESP-IDF's opcode of a chip erase (`spi_flash_defs.h`, report BC
/// §D.2 item 6). The chip model runs both alike, so no content depends on the choice:
/// only the count of commands per opcode does, and a comparison with QEMU. Settled by a logic
/// analyser on the flash bus of a device while the ROM's `SPI_chip_erase` runs, or by the
/// lockstep against QEMU of SP1b-4 (which would then see `60`).
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
/// What the callers of a hard-wired command put in `ADDR`: the address in its low 24 bits,
/// `addr & 0xffffff` for an erase, the same under the byte count for a program
/// (`ROM3:SPI_sector_erase@0x4004d294`, `ROM3:SPI_page_program@0x4004d6b4`;
/// `IDF:hal/esp32c3/include/hal/spimem_flash_ll.h:81-95, 377-389`; measured: report FW §A.2,
/// §A.3).
///
/// ASSUMPTION: a hard-wired command clocks out 24 address bits and no dummy clock whatever
/// `USER1.USR_ADDR_BITLEN` and `USER.USR_DUMMY` hold: its frame is fixed in the controller, as
/// its opcode is. The sources show what the callers write, not what the controller sends, and
/// every one of them prepares the frame a user transfer would need: `SPI_sector_erase` rewrites
/// `USER` and `USER1` with 24 address bits before `CMD = 0x01020000`,
/// `esp_rom_spiflash_read_status` clears `USR_DUMMY` before the hard-wired status read (report
/// FW §A.3), and ESP-IDF writes `USER1 = 0x5C000003` before `CMD = 0x01000000` (FW §A.2). So no
/// measured boot tells a controller that follows `USER1` from one that does not. A hard-wired
/// erase issued with another `USER1[31:26]` on a device (which sector is erased), or under the
/// SP1b-4 lockstep, settles it.
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
/// SPI1 `+0x54`: `SPI_MEM_FSM_REG`, the same register as SPI0 `FSM`, whose offset it shares. The SVD
/// lists it for SPI0 only; ESP-IDF's `spi_mem_reg.h` (v4.4.7 `include/soc/`, v5.5.2
/// `register/soc/`) defines `SPI_MEM_FSM_REG(i)` at `+0x054` of both controllers: `CSPI_ST` [3:0]
/// and `EM_ST` [6:4] read-only, the state of the SPI0 machines; `CSPI_LOCK_DELAY_TIME` [11:7]
/// read-write, reset 4 (report `docs/research/spi1-fsm-and-usj-reset.md`, §A.1). ESP-IDF v4.4.7
/// reads it through SPI1 to learn whether the controller is idle (`spimem_flash_ll_host_idle`,
/// `fsm.spi0_mst_st == 0`, polled by `spi_flash_chip_generic_wait_idle`, §A.2); v5.5.2 reads
/// `CMD` [3:0] instead.
const SPI1_FSM: u32 = SPI0_FSM;
/// `FSM` [11:7], `CSPI_LOCK_DELAY_TIME`: the one field a write changes.
const FSM_LOCK_DELAY: u32 = 0x1F << 7;
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

/// The 64 KiB pages of the largest chip: those an MMU entry can name, its page number has 8 bits.
const FLASH_PAGES: usize = 256;

/// For each 64 KiB page of the chip, whether it holds something an erase would change, as the
/// chip tells it. 256 questions, whatever the chip and the firmware.
fn pages_holding_data(flash: &dyn FlashDevice) -> [bool; FLASH_PAGES] {
    let mut held = [false; FLASH_PAGES];
    for (page, held) in (0u32..).zip(held.iter_mut()) {
        // Page 255 starts at 0xFF0000: the product cannot overflow.
        *held = flash.holds_data(page * MMU_PAGE_BYTES, MMU_PAGE_BYTES);
    }
    held
}

/// Tells the cache which 64 KiB pages `write` changed bytes of; `write` changed the content of
/// the chip.
///
/// A write inside one page changed that page. An erase of several pages changed those of them
/// that held something to erase and no other: with `held`, what the chip said of each page
/// before the erase, the cache is told of each run of such pages, 128 events at most. Without
/// it the cache is told of the whole range: more than changed, never less.
fn tell_cache(ctx: &mut Ctx, write: &FlashWrite, held: Option<&[bool; FLASH_PAGES]>) {
    let Some((first, last)) = cache_pages(write) else {
        return;
    };
    let (Some(held), true) = (held, first != last) else {
        ctx.signal(Event::FlashWritten {
            first_page: first,
            last_page: last,
        });
        return;
    };
    let mut run: Option<(u32, u32)> = None;
    for (page, &held) in (0u32..).zip(held) {
        if held && (first..=last).contains(&page) {
            run = Some((run.map_or(page, |(first_page, _)| first_page), page));
        } else if let Some((first_page, last_page)) = run.take() {
            ctx.signal(Event::FlashWritten {
                first_page,
                last_page,
            });
        }
    }
    if let Some((first_page, last_page)) = run {
        ctx.signal(Event::FlashWritten {
            first_page,
            last_page,
        });
    }
}

/// SPI1, the controller that runs the flash commands software asks for.
///
/// It keeps the journal of what the run wrote to the chip and the count of the commands it
/// sent, for the machine: a front end reads both from [`crate::machine::Esp32C3`], which owns
/// the controller, not from here.
///
/// ```compile_fail
/// // The journal is taken from the machine: `Esp32C3::take_flash_writes`.
/// fn journal(spi1: &mut soc_esp32c3::periph::spi_mem::Spi1) -> usize {
///     spi1.take_flash_writes().len()
/// }
/// ```
#[derive(Debug)]
pub struct Spi1 {
    registers: Storage,
    flash: Box<dyn FlashDevice>,
    /// What the commands of the run wrote to the chip. It belongs to the run, not to the
    /// controller: a reset of the controller keeps it.
    journal: FlashJournal,
    /// How many commands of each opcode the controller sent to the chip, refused ones included.
    /// Like the journal it belongs to the run. One counter per opcode a byte can hold: its size
    /// does not depend on what the firmware sends.
    sent: [u64; OPCODES],
    /// `FSM` read through SPI1 ([`SPI1_FSM`]): the lock delay last written in [11:7], every other
    /// bit 0.
    fsm: u32,
}

/// The reset value of `FSM`, the SVD's for SPI0: `0x200`, the lock delay 4 and the two state
/// machines idle.
fn fsm_reset_value() -> u32 {
    svd_regs::esp32c3::find("SPI0", "FSM").map_or(0, |r| r.reset_value)
}

/// The opcodes a command byte can hold.
const OPCODES: usize = 256;

impl Spi1 {
    #[must_use]
    pub fn new(flash: Box<dyn FlashDevice>) -> Spi1 {
        Spi1 {
            registers: Spi1::reset_registers(),
            flash,
            journal: FlashJournal::default(),
            sent: [0; OPCODES],
            fsm: fsm_reset_value(),
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
    ///
    /// ASSUMPTION: no reset of the ESP32-C3 reaches the chip, whatever its level (CPU, core,
    /// system, or this controller): the write-enable latch and the status register keep what
    /// they held, so a program sent after the reset runs on a write enable sent before it. The
    /// datasheet clears the latch at power-up, by `04`, and when an operation completes (FW §C);
    /// the chip's own reset, `66` then `99`, is no reset of the ESP32-C3 (FW §G). Nothing
    /// measured says whether a device keeps the chip powered across `esp_restart` or a watchdog
    /// reset, nor whether its flash pins are driven so as to reset the chip. Settled on a device: write
    /// enable, then `esp_restart` or an RTC watchdog reset, then read the status register
    /// before any other write enable. Neither the ROM nor ESP-IDF sends a program or an erase
    /// without a write enable just before it (FW §A.2, §A.3), so no measured path depends on it.
    ///
    /// The generation of the chip is kept with its content. After a core or a system reset the
    /// version of the flash windows is therefore lower than before it: the terms of the MMU and
    /// of the cache restart at 0, that of the chip does not. `SocBus::page_version` says why a
    /// version that goes down is safe: every reset builds a new decode cache.
    pub(crate) fn reset(&mut self) {
        self.registers = Spi1::reset_registers();
        self.fsm = fsm_reset_value();
    }

    /// The chip, for the cache windows of the bus.
    #[must_use]
    pub fn flash(&self) -> &dyn FlashDevice {
        self.flash.as_ref()
    }

    /// The programs, erases and status writes the chip performed since the last call, oldest
    /// first: see [`FlashJournal`]. For the machine, which is where a front end takes them
    /// ([`Esp32C3::take_flash_writes`](crate::machine::Esp32C3::take_flash_writes)).
    pub(crate) fn take_flash_writes(&mut self) -> Vec<FlashWrite> {
        self.journal.take()
    }

    /// The operations the journal did not keep because it was full. For the machine, like the
    /// journal.
    pub(crate) const fn flash_writes_dropped(&self) -> u64 {
        self.journal.dropped()
    }

    /// How many commands of `opcode` the controller sent to the chip since power-on, those the
    /// chip model refused included. For the machine, like the journal.
    pub(crate) fn commands_sent(&self, opcode: u8) -> u64 {
        self.sent.get(usize::from(opcode)).copied().unwrap_or(0)
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
        // The callers ask for 64 bytes at most (`Spi1::phase_bytes`); the `min` is what bounds
        // the allocation whatever a caller does.
        let mut data_in = vec![0u8; data_in_len.min(DATA_BYTES)];
        if let Some(count) = self.sent.get_mut(usize::from(command.opcode)) {
            // A count that reached the largest number stays there: it never wraps to a small
            // one, and never stops the run.
            *count = count.saturating_add(1);
        }
        let before = self.flash.generation();
        // A command that names an address changes bytes of the 64 KiB page of that address and
        // of no other: a program stays in its 256-byte page, a sector or a block erase in its
        // unit of 4 or 64 KiB (FW §C, §G). The erase of the whole chip changes the pages that
        // hold something and leaves the others as they were: the chip is asked which ones hold
        // something before it runs that command, and before no other. The 256 questions would
        // cost many times a write enable or a status read, which accompany every program and
        // every erase and change no content. They are not asked either before a chip erase
        // the chip would ignore (no write enable) or refuse (another frame than its own, a
        // block-protect bit): it changes nothing, and a firmware can send it in a loop.
        let held = (erases_whole_chip(command.opcode) && self.flash.would_erase_the_chip(command))
            .then(|| pages_holding_data(self.flash.as_ref()));
        match self.flash.execute(command, &mut data_in) {
            Ok(written) => {
                if let Some(write) = written {
                    // The cache is told of the bytes that changed, not of a command that left
                    // the content as it was: only the former can make a cached line stale.
                    if self.flash.generation() != before {
                        tell_cache(ctx, &write, held.as_ref());
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
                let miso_bits = self.data_bits(MISO_DLEN);
                let Some(len) = Spi1::phase_bytes(miso_bits) else {
                    ctx.unsupported(Unsupported::FlashTransferLength {
                        mosi_bits: 0,
                        miso_bits,
                    });
                    return;
                };
                if let Some(data) = self.transfer(ctx, &FlashCommand { address, ..command }, len) {
                    self.store_data_in(&data);
                }
            }
            _ => {
                let _ = self.transfer(ctx, &command, 0);
            }
        }
    }

    /// The bits a `xxx_DLEN` register asks for, 1 to 1024: its field is the bit length minus one.
    fn data_bits(&self, register: u32) -> u32 {
        (self.register(register) & DLEN_MASK) + 1
    }

    /// The bytes of a data phase of `bits` bits, 0 for a phase the transfer does not have; `None`
    /// for a phase the model cannot carry, which the caller refuses by name:
    ///
    /// - more than the 512 bits `W0..W15` hold. Where the controller would take or put the bytes
    ///   past `W15` is not established, and a transfer cut to 64 bytes would be judged by the
    ///   chip on a length the firmware did not ask for;
    /// - a number of bits that is not a whole number of bytes. A chip drops a command whose chip
    ///   select rises off a byte boundary (report FW §C, row "Framing"), and where the bits of a
    ///   partial byte read would land in `W0..` is not established.
    ///
    /// Every measured transfer is a whole number of bytes that fits: 8, 24, 32, 256 and 512 bits
    /// in (`MISO_DLEN` 7, 23, `0x1F`, `0xFF`, `0x1FF`: reports FW §A.2, SF §C, §F.1, BA §2.2), 128
    /// for the ROM's `SPI_read_data` (FW §A.3); no measured `USR` transfer has a data-out phase.
    fn phase_bytes(bits: u32) -> Option<usize> {
        let bytes = usize::try_from(bits / u8::BITS).ok()?;
        (bits.is_multiple_of(u8::BITS) && bytes <= DATA_BYTES).then_some(bytes)
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
        // The two data phases are judged before anything is sent: a refused transfer reaches
        // the chip neither whole nor cut short. A length register belongs to its phase: without
        // `USR_MOSI` or `USR_MISO` its value means nothing (ESP-IDF reads the status with
        // `MOSI_DLEN = 0`, one bit, and no data-out phase: FW §A.2).
        let mosi_bits = if user & USR_MOSI == 0 {
            0
        } else {
            self.data_bits(MOSI_DLEN)
        };
        let miso_bits = if user & USR_MISO == 0 {
            0
        } else {
            self.data_bits(MISO_DLEN)
        };
        let (Some(data_out_len), Some(data_in_len)) =
            (Spi1::phase_bytes(mosi_bits), Spi1::phase_bytes(miso_bits))
        else {
            ctx.unsupported(Unsupported::FlashTransferLength {
                mosi_bits,
                miso_bits,
            });
            return;
        };
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
        let data_out = self.data_out(data_out_len);
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
                // `commands` is one of them. The test
                // `each_of_the_thirteen_hard_wired_bits_sends_its_own_opcode_once` holds it: a
                // bit without an entry would start nothing and report nothing.
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
        if offset == SPI1_FSM {
            // A transfer completes inside the write of `CMD` that starts it, and the cache is not
            // modelled: no state machine is ever seen busy, so both state fields read 0, idle
            // (report `docs/research/spi1-fsm-and-usj-reset.md`, §A.3). A busy value would
            // contradict `CMD` reading 0 and could hold `wait_idle` for ever.
            // ASSUMPTION: SPI1 `+0x54` reads the register SPI0 `FSM` holds, as the ESP-IDF headers
            // define one register for both controllers; the value read through SPI1 on a chip is
            // not measured. A read of `0x60002054` on a chip at rest settles it (`0x200` expected).
            return Some(self.fsm);
        }
        self.registers.read(offset)
    }

    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        if offset == CMD {
            self.run(ctx, value);
            return true;
        }
        if offset == SPI1_FSM {
            // The state fields are read-only; the lock delay acts only on a resume after a
            // suspend, which is refused by name (`FlashSuspend`), so storing it is all it does.
            self.fsm = value & FSM_LOCK_DELAY;
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
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

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

    /// `0x60002054`, which ESP-IDF v4.4.7 polls through SPI1 before each status read
    /// (`spimem_flash_ll_host_idle`): `FSM`, idle, without a diagnostic.
    #[test]
    fn spi1_fsm_reads_idle_and_keeps_only_its_lock_delay() {
        let ctx = &mut Ctx::default();
        let mut spi = spi1();
        assert_eq!(SPI1_BASE + SPI1_FSM, 0x6000_2054);
        assert_eq!(
            spi.read(ctx, SPI1_FSM),
            Some(0x200),
            "the reset value of SPI0 FSM"
        );
        // What `spimem_flash_ll_host_idle` tests: `spi0_mst_st`, bits [6:4].
        assert_eq!(spi.read(ctx, SPI1_FSM).map(|v| v & 0x70), Some(0));
        // A transfer completes inside its write: the machines are idle right after it.
        spi.write(ctx, CMD, CMD_RDSR);
        assert_eq!(spi.read(ctx, SPI1_FSM), Some(0x200));
        // Only `CSPI_LOCK_DELAY_TIME` [11:7] is written; the state fields stay idle.
        assert!(spi.write(ctx, SPI1_FSM, 0xFFFF_FFFF));
        assert_eq!(spi.read(ctx, SPI1_FSM), Some(0xF80));
        assert!(spi.write(ctx, SPI1_FSM, 0x7F | (9 << 7)));
        assert_eq!(spi.read(ctx, SPI1_FSM), Some(9 << 7));
        assert_eq!(ctx.take_unsupported(), None);
        // A reset of the controller gives it back its reset value.
        spi.reset();
        assert_eq!(spi.read(ctx, SPI1_FSM), Some(0x200));
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
        // A chip erase: the pages that still hold something (0, 1 and 2: the block erase left
        // page 3 blank), not the pages past the image, which it leaves as they were.
        spi.write(ctx, CMD, CMD_WREN);
        spi.write(ctx, CMD, CMD_CE);
        assert_eq!(ctx.take_signal(), written(0, 2));
        assert_eq!(ctx.take_signal(), None, "one run of pages, one event");
        assert_eq!(ctx.take_unsupported(), None);
    }

    #[test]
    fn a_chip_erase_tells_the_cache_of_each_run_of_pages_it_changed_and_of_no_blank_page() {
        const CMD_CE: u32 = 1 << 22;
        let ctx = &mut Ctx::default();
        let mut spi = spi1();
        // One byte programmed in each of the 64 KiB pages 0, 2, 3 and 255, the last of the chip.
        for address in [0x00_FFFF, 0x02_0000, 0x03_8000, 0xFF_FFFF] {
            spi.write(ctx, CMD, CMD_WREN);
            spi.write(ctx, W0, 0);
            spi.write(ctx, ADDR, (1 << 24) | address);
            spi.write(ctx, CMD, CMD_PP);
            let page = address / MMU_PAGE_BYTES;
            assert_eq!(
                ctx.take_signal(),
                Some(Event::FlashWritten {
                    first_page: page,
                    last_page: page
                })
            );
        }
        spi.write(ctx, CMD, CMD_WREN);
        spi.write(ctx, CMD, CMD_CE);
        let told: Vec<_> = std::iter::from_fn(|| ctx.take_signal()).collect();
        assert_eq!(
            told,
            [(0, 0), (2, 3), (255, 255)].map(|(first_page, last_page)| Event::FlashWritten {
                first_page,
                last_page
            })
        );
        assert_eq!(flash_bytes(&spi, 0xFF_FFFF, 1), [0xFF], "it was erased");
        // The chip is blank: a second erase changes nothing and tells nothing.
        spi.write(ctx, CMD, CMD_WREN);
        spi.write(ctx, CMD, CMD_CE);
        assert_eq!(ctx.take_signal(), None);
        assert_eq!(ctx.take_unsupported(), None);
    }

    /// A chip that answers every command with an erase of the 64 KiB pages 1 to 3 that changed
    /// its content, and says of each page what `holds` says: what the controller tells the
    /// cache when an erase covers several pages.
    #[derive(Debug)]
    struct ErasesThreePages {
        generation: u64,
        holds: Option<fn(u32) -> bool>,
        /// How many times the controller asked what a page holds.
        asked: Rc<Cell<u32>>,
        /// What the chip says when asked whether a chip erase would run now.
        would_erase: Rc<Cell<bool>>,
    }

    impl FlashDevice for ErasesThreePages {
        fn size(&self) -> u32 {
            FlashConfig::default().size()
        }
        fn read(&self, _offset: u32, out: &mut [u8]) {
            out.fill(0xFF);
        }
        fn execute(
            &mut self,
            _command: &FlashCommand<'_>,
            _data_in: &mut [u8],
        ) -> Result<Option<FlashWrite>, FlashRefusal> {
            self.generation += 1;
            Ok(Some(FlashWrite::Erase {
                start: MMU_PAGE_BYTES,
                len: 3 * MMU_PAGE_BYTES,
            }))
        }
        fn generation(&self) -> u64 {
            self.generation
        }
        fn modified_sectors(&self) -> u32 {
            0
        }
        fn holds_data(&self, start: u32, len: u32) -> bool {
            assert_eq!(
                (start % MMU_PAGE_BYTES, len),
                (0, MMU_PAGE_BYTES),
                "asked page by page"
            );
            self.asked.set(self.asked.get() + 1);
            self.holds.is_none_or(|holds| holds(start / MMU_PAGE_BYTES))
        }
        fn would_erase_the_chip(&self, _command: &FlashCommand<'_>) -> bool {
            self.would_erase.get()
        }
    }

    /// The chip model itself, with the questions the controller asks of it counted.
    #[derive(Debug)]
    struct CountsQuestions {
        chip: NorFlash,
        asked: Rc<Cell<u32>>,
    }

    impl FlashDevice for CountsQuestions {
        fn size(&self) -> u32 {
            self.chip.size()
        }
        fn read(&self, offset: u32, out: &mut [u8]) {
            self.chip.read(offset, out);
        }
        fn execute(
            &mut self,
            command: &FlashCommand<'_>,
            data_in: &mut [u8],
        ) -> Result<Option<FlashWrite>, FlashRefusal> {
            self.chip.execute(command, data_in)
        }
        fn generation(&self) -> u64 {
            self.chip.generation()
        }
        fn modified_sectors(&self) -> u32 {
            self.chip.modified_sectors()
        }
        fn holds_data(&self, start: u32, len: u32) -> bool {
            self.asked.set(self.asked.get() + 1);
            self.chip.holds_data(start, len)
        }
        fn would_erase_the_chip(&self, command: &FlashCommand<'_>) -> bool {
            self.chip.would_erase_the_chip(command)
        }
    }

    #[test]
    fn a_chip_erase_the_chip_refuses_for_its_frame_asks_it_nothing() {
        let ctx = &mut Ctx::default();
        let asked = Rc::new(Cell::new(0));
        let mut spi = Spi1::new(Box::new(CountsQuestions {
            chip: NorFlash::with_image(vec![0x00; 16], FlashConfig::default()).unwrap(),
            asked: Rc::clone(&asked),
        }));
        // `60` through `USR` with a 24-bit address, after a write enable: the latch is set, and
        // the chip refuses the command for its frame.
        spi.write(ctx, CMD, CMD_WREN);
        spi.write(ctx, USER, USR_COMMAND | USR_ADDR);
        spi.write(ctx, USER1, 23 << ADDR_BITLEN_SHIFT);
        spi.write(ctx, USER2, 0x7000_0060);
        spi.write(ctx, CMD, CMD_USR);
        assert!(
            matches!(
                ctx.take_unsupported(),
                Some(Unsupported::FlashCommandForm {
                    opcode: 0x60,
                    address_bits: 24,
                    ..
                })
            ),
            "refused for its frame"
        );
        assert_eq!(asked.get(), 0, "and nothing was asked");
        // The same command in its frame, the latch still set: it runs, and the chip is asked.
        spi.write(ctx, USER, USR_COMMAND);
        spi.write(ctx, CMD, CMD_USR);
        assert_eq!(ctx.take_unsupported(), None);
        assert_eq!(asked.get(), 256);
    }

    #[test]
    fn an_erase_of_several_pages_is_told_page_by_page_only_when_the_chip_was_asked_before_it() {
        const CMD_CE: u32 = 1 << 22;
        let told = |holds, command| {
            let ctx = &mut Ctx::default();
            let mut spi = Spi1::new(Box::new(ErasesThreePages {
                generation: 0,
                holds,
                asked: Rc::default(),
                would_erase: Rc::new(Cell::new(true)),
            }));
            spi.write(ctx, ADDR, 0x1_0000);
            spi.write(ctx, CMD, command);
            std::iter::from_fn(|| ctx.take_signal())
                .map(|event| match event {
                    Event::FlashWritten {
                        first_page,
                        last_page,
                    } => (first_page, last_page),
                    other => panic!("{other:?}"),
                })
                .collect::<Vec<_>>()
        };
        let only_page_2: Option<fn(u32) -> bool> = Some(|page| page == 2);
        let pages_0_to_2_and_4: Option<fn(u32) -> bool> = Some(|page| page <= 2 || page == 4);
        // The erase of the whole chip: the chip was asked, and the pages it said held something
        // are the ones told, inside the range of the erase only.
        assert_eq!(told(only_page_2, CMD_CE), [(2, 2)]);
        assert_eq!(told(pages_0_to_2_and_4, CMD_CE), [(1, 2)]);
        assert_eq!(told(Some(|page| page != 2), CMD_CE), [(1, 1), (3, 3)]);
        assert_eq!(told(Some(|_| false), CMD_CE), []);
        // A chip that keeps no account of it says every page may hold something: the range.
        assert_eq!(told(None, CMD_CE), [(1, 3)]);
        assert!(
            Recorder(Rc::default()).holds_data(0, 1),
            "what a chip answers when it does not say otherwise"
        );
        assert!(
            Recorder(Rc::default()).would_erase_the_chip(&FlashCommand {
                opcode: 0xC7,
                address: None,
                dummy_cycles: 0,
                data_out: &[],
            }),
            "a chip that does not say is asked before every chip erase"
        );
        // No other command is expected to reach past one page, whether it names an address (a
        // sector erase) or none (a write enable, a status read), so the chip was not asked: the
        // whole range is told, which is never less than what changed.
        for command in [CMD_SE, CMD_WREN, CMD_RDSR] {
            assert_eq!(told(only_page_2, command), [(1, 3)], "{command:#010x}");
        }
    }

    #[test]
    fn the_chip_is_asked_what_its_pages_hold_before_an_erase_of_the_whole_chip_and_no_other() {
        const CMD_CE: u32 = 1 << 22;
        let ctx = &mut Ctx::default();
        let asked = Rc::new(Cell::new(0));
        let would_erase = Rc::new(Cell::new(true));
        let mut spi = Spi1::new(Box::new(ErasesThreePages {
            generation: 0,
            holds: None,
            asked: Rc::clone(&asked),
            would_erase: Rc::clone(&would_erase),
        }));
        // The commands that accompany every program and every erase, hard-wired and through
        // `USR`: write enable, write disable, the status read and write, the id read, the
        // wake-up. Then the writes that name an address: a program of one byte and the erases
        // of a sector and of a block.
        spi.write(ctx, ADDR, 0x0100_0000);
        for command in [
            CMD_WREN,
            1 << 29,
            CMD_RDID,
            CMD_RDSR,
            CMD_WRSR,
            1 << 20,
            CMD_PP,
            CMD_SE,
            CMD_BE,
        ] {
            spi.write(ctx, CMD, command);
        }
        user_read(ctx, &mut spi, 0x05, 8);
        user_read(ctx, &mut spi, 0x9F, 24);
        for opcode in [0x06, 0x04, 0x7A] {
            spi.write(ctx, USER, USR_COMMAND);
            spi.write(ctx, USER2, 0x7000_0000 | opcode);
            spi.write(ctx, CMD, CMD_USR);
        }
        assert_eq!(spi.commands_sent(0x06), 2, "they were sent");
        assert_eq!(asked.get(), 0, "none of them can change more than one page");
        // Both opcodes of the erase of the whole chip: one question per 64 KiB page, each time.
        spi.write(ctx, CMD, CMD_CE);
        assert_eq!(asked.get(), 256);
        spi.write(ctx, USER, USR_COMMAND);
        spi.write(ctx, USER2, 0x7000_0060);
        spi.write(ctx, CMD, CMD_USR);
        assert_eq!(asked.get(), 512);
        // A chip erase the chip would ignore or refuse: nothing to ask, by either opcode.
        would_erase.set(false);
        spi.write(ctx, CMD, CMD_CE);
        spi.write(ctx, CMD, CMD_USR);
        assert_eq!(spi.commands_sent(0xC7), 2, "it was sent");
        assert_eq!(spi.commands_sent(0x60), 2, "it was sent");
        assert_eq!(asked.get(), 512, "and nothing was asked");
        assert_eq!(ctx.take_unsupported(), None);
    }

    #[test]
    fn a_count_of_commands_that_reached_the_largest_number_stays_there() {
        let ctx = &mut Ctx::default();
        let mut spi = spi1();
        spi.sent[0x06] = u64::MAX - 1;
        for _ in 0..3 {
            spi.write(ctx, CMD, CMD_WREN);
        }
        assert_eq!(spi.commands_sent(0x06), u64::MAX);
        assert_eq!(spi.commands_sent(0x04), 0, "each opcode has its own count");
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
    fn every_command_sent_to_the_chip_is_counted_under_its_opcode_for_the_whole_run() {
        let ctx = &mut Ctx::default();
        let mut spi = spi1();
        let sent =
            |spi: &Spi1| [0x05, 0x06, 0x02, 0x04, 0x9F, 0x35].map(|op| spi.commands_sent(op));
        assert_eq!(sent(&spi), [0; 6]);
        // One program as ESP-IDF's driver sends it to a chip whose latch clears at completion:
        // status, write enable, status, program, status (report SF §B.1). No write disable.
        spi.write(ctx, CMD, CMD_RDSR);
        spi.write(ctx, CMD, CMD_WREN);
        spi.write(ctx, CMD, CMD_RDSR);
        spi.write(ctx, W0, 0xFFFF_FFFE);
        spi.write(ctx, ADDR, 0x0400_9020);
        spi.write(ctx, CMD, CMD_PP);
        spi.write(ctx, CMD, CMD_RDSR);
        assert_eq!(sent(&spi), [3, 1, 1, 0, 0, 0]);
        // A user transfer counts under the opcode of `USER2`; one the chip refuses was sent too.
        user_read(ctx, &mut spi, 0x9F, 24);
        user_read(ctx, &mut spi, 0x35, 8);
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::FlashCommand { opcode: 0x35 })
        );
        assert_eq!(sent(&spi), [3, 1, 1, 0, 1, 1]);
        // A program of no byte is refused by the controller: nothing was sent to the chip.
        spi.write(ctx, ADDR, 0x9020);
        spi.write(ctx, CMD, CMD_PP);
        assert!(ctx.take_unsupported().is_some());
        assert_eq!(sent(&spi), [3, 1, 1, 0, 1, 1]);
        // The counts are those of the run: a reset of the controller keeps them.
        spi.reset();
        spi.write(ctx, CMD, 1 << 29); // FLASH_WRDI
        assert_eq!(sent(&spi), [3, 1, 1, 1, 1, 1]);
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
    fn a_data_phase_of_512_bits_fills_the_sixteen_data_registers_and_nothing_past_them() {
        // `MISO_DLEN = 0x1FF`, the longest measured read (SF §C): 64 bytes, the whole of W0..W15.
        let ctx = &mut Ctx::default();
        let mut spi = spi1();
        user_read(ctx, &mut spi, 0x9F, 512);
        let w15 = svd_regs::esp32c3::addr("SPI1", "W15") - SPI1_BASE;
        // 64 bytes of the repeating id 9D 60 18: byte 60 is 9D (60 % 3 == 0).
        assert_eq!(spi.read(ctx, w15), Some(0x9D18_609D));
        assert_eq!(
            spi.read(ctx, w15 + 4),
            Some(0x14),
            "FLASH_WAITI_CTRL keeps its reset value: nothing was stored past W15"
        );
        assert_eq!(ctx.take_unsupported(), None);
    }

    /// A `USR` page program at `0x9000` whose data phase is `mosi_dlen` (bits minus one).
    fn user_program(ctx: &mut Ctx, spi: &mut Spi1, mosi_dlen: u32) {
        spi.write(ctx, USER, USR_COMMAND | USR_ADDR | USR_MOSI);
        spi.write(ctx, USER1, 23 << 26);
        spi.write(ctx, USER2, 0x7000_0002);
        spi.write(ctx, ADDR, 0x9000);
        spi.write(ctx, MOSI_DLEN, mosi_dlen);
        spi.write(ctx, CMD, CMD_USR);
    }

    #[test]
    fn a_data_phase_longer_than_the_data_registers_is_refused_by_name_and_sends_nothing() {
        // The length fields hold 10 bits: up to 1024 bits, twice what W0..W15 hold. 513 bits is
        // the first length past them, 520 the first whole number of bytes, 1024 the longest.
        for bits in [513u32, 520, 768, 1024] {
            let ctx = &mut Ctx::default();
            let (mut spi, seen) = recording_spi1();
            fill_data_registers(ctx, &mut spi);
            user_program(ctx, &mut spi, bits - 1);
            assert_eq!(
                ctx.take_unsupported(),
                Some(Unsupported::FlashTransferLength {
                    mosi_bits: bits,
                    miso_bits: 0
                }),
                "{bits} bits out"
            );
            assert_eq!(spi.read(ctx, CMD), Some(0), "self-clears: no hang");
            user_read(ctx, &mut spi, 0x9F, bits);
            assert_eq!(
                ctx.take_unsupported(),
                Some(Unsupported::FlashTransferLength {
                    mosi_bits: 0,
                    miso_bits: bits
                }),
                "{bits} bits in"
            );
            assert_eq!(ctx.take_unsupported(), None, "one write, one report");
            assert_eq!(seen.borrow().as_slice(), [], "nothing reached the chip");
            assert_eq!(spi.commands_sent(0x02) + spi.commands_sent(0x9F), 0);
        }
        // The longest phase that fits is sent whole: 64 bytes, W0 first.
        let ctx = &mut Ctx::default();
        let (mut spi, seen) = recording_spi1();
        fill_data_registers(ctx, &mut spi);
        user_program(ctx, &mut spi, 511);
        let sent: Vec<u8> = (1..=64).collect();
        assert_eq!(
            seen.borrow().as_slice(),
            [(0x02, Some(at(0x9000)), 0, sent)]
        );
        assert_eq!(ctx.take_unsupported(), None);
    }

    #[test]
    fn a_read_asked_past_the_end_of_the_chip_through_too_long_a_phase_reads_nothing() {
        // 128 bytes asked from 64 bytes before the end of the chip: 64 of them are past it. The
        // request is refused as it was made, not judged on the 64 bytes the registers could hold.
        let ctx = &mut Ctx::default();
        let mut spi = spi_with_image(vec![0x11; 64]);
        spi.write(ctx, W0, 0xDEAD_BEEF);
        dio_read(ctx, &mut spi, 0xFF_FFC0, 128);
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::FlashTransferLength {
                mosi_bits: 0,
                miso_bits: 1024
            })
        );
        assert_eq!(
            spi.read(ctx, W0),
            Some(0xDEAD_BEEF),
            "nothing was clocked in"
        );
        assert_eq!(spi.commands_sent(0xBB), 0, "nothing was sent");
    }

    #[test]
    fn a_data_phase_that_is_not_whole_bytes_is_refused_by_name_and_changes_nothing() {
        // A chip drops a command whose chip select rises off a byte boundary (FW §C): a program
        // of 9 bits programs nothing, and neither does the model.
        for bits in [1u32, 7, 9, 12, 31, 33, 511] {
            let ctx = &mut Ctx::default();
            let mut spi = spi1();
            spi.write(ctx, CMD, CMD_WREN);
            spi.write(ctx, W0, 0x0000_1200);
            user_program(ctx, &mut spi, bits - 1);
            assert_eq!(
                ctx.take_unsupported(),
                Some(Unsupported::FlashTransferLength {
                    mosi_bits: bits,
                    miso_bits: 0
                }),
                "{bits} bits out"
            );
            assert_eq!(flash_bytes(&spi, 0x9000, 4), [0xFF; 4], "{bits} bits out");
            assert_eq!(spi.take_flash_writes(), []);
            spi.write(ctx, CMD, CMD_RDSR);
            assert_eq!(
                spi.read(ctx, RD_STATUS),
                Some(0x02),
                "nothing reached the chip: its latch is still set"
            );
            // A read of a partial byte: where its bits would land in W0 is not established.
            spi.write(ctx, USER, USR_COMMAND | USR_MISO);
            spi.write(ctx, USER2, 0x7000_009F);
            spi.write(ctx, MISO_DLEN, bits - 1);
            spi.write(ctx, W0, 0xDEAD_BEEF);
            spi.write(ctx, CMD, CMD_USR);
            assert_eq!(
                ctx.take_unsupported(),
                Some(Unsupported::FlashTransferLength {
                    mosi_bits: 0,
                    miso_bits: bits
                }),
                "{bits} bits in"
            );
            assert_eq!(
                spi.read(ctx, W0),
                Some(0xDEAD_BEEF),
                "nothing was clocked in"
            );
            assert_eq!(spi.read(ctx, CMD), Some(0), "self-clears: no hang");
        }
    }

    #[test]
    fn the_length_of_a_phase_the_transfer_does_not_have_is_not_judged() {
        // ESP-IDF reads the status with `USER = 0x90000080` (command and data in), `MOSI_DLEN = 0`
        // (one bit) and `MISO_DLEN = 7` (FW §A.2): the data-out length belongs to no phase.
        let ctx = &mut Ctx::default();
        let mut spi = spi1();
        spi.write(ctx, CMD, CMD_WREN);
        spi.write(ctx, USER, 0x9000_0080);
        spi.write(ctx, USER2, 0x7000_0005);
        spi.write(ctx, MOSI_DLEN, 0);
        spi.write(ctx, MISO_DLEN, 7);
        spi.write(ctx, CMD, CMD_USR);
        assert_eq!(spi.read(ctx, W0), Some(0x02));
        // And the other way round: a write disable while `MISO_DLEN` holds 1024 bits.
        spi.write(ctx, USER, USR_COMMAND);
        spi.write(ctx, USER2, 0x7000_0004);
        spi.write(ctx, MISO_DLEN, 1023);
        spi.write(ctx, MOSI_DLEN, 1023);
        spi.write(ctx, CMD, CMD_USR);
        assert_eq!(spi.commands_sent(0x04), 1);
        assert_eq!(ctx.take_unsupported(), None);
    }

    #[test]
    fn the_hard_wired_read_refuses_a_length_the_data_registers_cannot_hold() {
        // 1024 and 513 bits are more than `W0..W15` hold; 9 bits and 1 bit are no whole number
        // of bytes. One bit is what `MISO_DLEN` asks for at its reset value, 0: a firmware that
        // sets the bit without the length is refused, not answered with one byte.
        let ctx = &mut Ctx::default();
        for (miso_dlen, bits) in [(1023u32, 1024u32), (512, 513), (8, 9), (0, 1)] {
            let (mut spi, seen) = recording_spi1();
            spi.write(ctx, ADDR, 0x101);
            spi.write(ctx, MISO_DLEN, miso_dlen);
            spi.write(ctx, W0, 0xDEAD_BEEF);
            assert!(spi.write(ctx, CMD, CMD_READ));
            assert_eq!(
                ctx.take_unsupported(),
                Some(Unsupported::FlashTransferLength {
                    mosi_bits: 0,
                    miso_bits: bits
                })
            );
            assert_eq!(seen.borrow().as_slice(), [], "nothing reached the chip");
            assert_eq!(spi.read(ctx, W0), Some(0xDEAD_BEEF));
        }
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
    fn each_of_the_thirteen_hard_wired_bits_sends_its_own_opcode_once() {
        // `CMD` bits 31 to 19 and the opcode each one sends (FW §A.1; `B9` is the JEDEC deep
        // power-down). The table is written out here: a bit missing from the controller's, or
        // mapped to the opcode of another, sends nothing or the wrong command in silence.
        let opcodes: [(u32, u8); 13] = [
            (31, 0x03),
            (30, 0x06),
            (29, 0x04),
            (28, 0x9F),
            (27, 0x05),
            (26, 0x01),
            (25, 0x02),
            (24, 0x20),
            (23, 0xD8),
            (22, 0xC7),
            (21, 0xB9),
            (20, 0xAB),
            (19, 0xA3),
        ];
        for (bit, opcode) in opcodes {
            let ctx = &mut Ctx::default();
            let (mut spi, seen) = recording_spi1();
            // One byte for the program, four for the read: lengths both commands carry.
            spi.write(ctx, ADDR, 0x0100_0000);
            spi.write(ctx, MISO_DLEN, 31);
            assert!(spi.write(ctx, CMD, 1 << bit));
            let sent: Vec<u8> = seen.borrow().iter().map(|command| command.0).collect();
            assert_eq!(sent, [opcode], "CMD bit {bit}");
            assert_eq!(spi.commands_sent(opcode), 1, "CMD bit {bit}");
            assert_eq!(ctx.take_unsupported(), None, "CMD bit {bit}");
            assert_eq!(spi.read(ctx, CMD), Some(0), "CMD bit {bit}");
        }
        // Bits 31 to 19 are all of them: with `USR`, bit 18, they are the command bits.
        let all = opcodes
            .iter()
            .fold(CMD_USR, |bits, (bit, _)| bits | 1 << bit);
        assert_eq!(all, COMMAND_BITS);
    }

    #[test]
    fn a_hard_wired_command_sends_its_own_frame_whatever_user_and_user1_describe() {
        // Pins the ASSUMPTION documented on `HARD_WIRED_ADDRESS_BITS`: a 32-bit address and 8
        // dummy clocks in `USER`/`USER1` do not reach a hard-wired erase, program or status read.
        let ctx = &mut Ctx::default();
        let (mut spi, seen) = recording_spi1();
        spi.write(ctx, USER, USR_COMMAND | USR_ADDR | USR_DUMMY | USR_MISO);
        spi.write(ctx, USER1, (31 << 26) | 7);
        spi.write(ctx, W0, 0x0000_00A5);
        spi.write(ctx, ADDR, 0x0100_E000);
        for cmd in [CMD_SE, CMD_BE, CMD_PP, CMD_RDSR] {
            spi.write(ctx, CMD, cmd);
        }
        assert_eq!(
            seen.borrow().as_slice(),
            [
                (0x20, Some(at(0xE000)), 0, Vec::new()),
                (0xD8, Some(at(0xE000)), 0, Vec::new()),
                (0x02, Some(at(0xE000)), 0, vec![0xA5]),
                (0x05, None, 0, Vec::new()),
            ]
        );
        assert_eq!(ctx.take_unsupported(), None);
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
    fn a_command_that_takes_no_address_and_no_data_is_refused_when_a_transfer_gives_it_some() {
        // A write enable, a status read, a read-id: each with a 4-byte address, 8 dummy clocks
        // and 4 bytes sent, a frame no ROM or ESP-IDF code gives them.
        let ctx = &mut Ctx::default();
        let mut spi = spi1();
        for opcode in [0x06u32, 0x04, 0x05, 0x9F, 0xAB, 0x7A] {
            spi.write(ctx, USER, USR_COMMAND | USR_ADDR | USR_DUMMY | USR_MOSI);
            spi.write(ctx, USER1, (31 << 26) | 7);
            spi.write(ctx, USER2, 0x7000_0000 | opcode);
            spi.write(ctx, MOSI_DLEN, 31);
            spi.write(ctx, CMD, CMD_USR);
            assert_eq!(
                ctx.take_unsupported(),
                Some(Unsupported::FlashCommandForm {
                    opcode: opcode.to_le_bytes()[0],
                    address_bits: 32,
                    dummy_cycles: 8,
                    data_bytes: 4
                })
            );
            assert_eq!(spi.read(ctx, CMD), Some(0), "self-clears: no hang");
        }
        spi.write(ctx, CMD, CMD_RDSR);
        assert_eq!(
            spi.read(ctx, RD_STATUS),
            Some(0),
            "the refused write enable did not set the latch"
        );
        assert_eq!(ctx.take_unsupported(), None);
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
    fn a_firmware_that_writes_thousands_of_distinct_values_cannot_fill_the_diagnostics() {
        // 20,000 rounds of registers filled with numbers no firmware would choose, then a
        // command: every refusal the controller and the chip have is reached thousands of times
        // with other values, and the table of distinct diagnostics stays far from full.
        let mut numbers = crate::test_support::Lcg::new(0x1B_F1A5);
        let ctx = &mut Ctx::default();
        // A 1 MiB chip: most 24-bit addresses are past its end.
        let small = FlashConfig::new(1 << 20, crate::flash::JedecId::from_u24(0x9D_6014).unwrap());
        let mut spi = Spi1::new(Box::new(NorFlash::blank(small.unwrap())));
        let mut seen = [0u32; 9];
        for _ in 0..20_000 {
            let n = &mut numbers;
            for register in [ADDR, RD_STATUS, CTRL, W0, USER, USER1, USER2] {
                spi.write(ctx, register, n.next_u32());
            }
            // Half of the rounds describe a transfer the controller can send: an 8-bit command,
            // a 24-bit address, whole bytes.
            if n.below(2) == 0 {
                spi.write(ctx, USER, USR_COMMAND | n.next_u32());
                spi.write(ctx, USER1, (23 << 26) | (n.next_u32() & DUMMY_CYCLES_MASK));
                spi.write(ctx, USER2, 0x7000_0000 | (n.next_u32() & OPCODE_MASK));
                spi.write(ctx, MOSI_DLEN, 8 * (1 + n.below(64)) - 1);
                spi.write(ctx, MISO_DLEN, 8 * (1 + n.below(64)) - 1);
            } else {
                spi.write(ctx, MOSI_DLEN, n.next_u32());
                spi.write(ctx, MISO_DLEN, n.next_u32());
            }
            spi.write(ctx, FLASH_SUS_CTRL, n.next_u32());
            spi.write(ctx, CMD, CMD_WREN);
            let flag_and_state = n.next_u32() & !COMMAND_BITS;
            let cmd = match n.below(4) {
                0 => n.next_u32(),
                1 => CMD_USR | flag_and_state,
                _ => (CMD_USR << (1 + n.below(13))) | flag_and_state,
            };
            spi.write(ctx, CMD, cmd);
            while let Some(refusal) = ctx.take_unsupported() {
                let kind = match refusal {
                    Unsupported::FlashCommand { .. } => 0,
                    Unsupported::FlashCommandForm { .. } => 1,
                    Unsupported::FlashRange { .. } => 2,
                    Unsupported::FlashProtected { .. } => 3,
                    Unsupported::FlashTransfer { .. } => 4,
                    Unsupported::FlashTransferLength { .. } => 5,
                    Unsupported::FlashProgramLength { .. } => 6,
                    Unsupported::FlashCommandBits { .. } => 7,
                    Unsupported::FlashSuspend { .. } => 8,
                    other => panic!("not a refusal of the flash: {other:?}"),
                };
                seen[kind] += 1;
            }
            let _ = spi.take_flash_writes();
        }
        assert!(
            seen.iter().all(|&count| count >= 100),
            "every kind of refusal was reached: {seen:?}"
        );
        // An opcode for each of the four refusals of the chip, a byte count that is not 1 to 64
        // for a program length, a command length for a transfer, and three single lines.
        let most = 4 * 256 + 192 + 17 + 3;
        assert!(
            ctx.diag.distinct() <= most && most < crate::diag::MAX_DISTINCT_DIAGNOSTICS / 2,
            "{} distinct diagnostics",
            ctx.diag.distinct()
        );
        assert_eq!(
            ctx.diag.suppressed(),
            0,
            "nothing was dropped for lack of room"
        );
        // Whatever the firmware does next is still listed.
        let _ = ctx.diag.drain();
        ctx.unsupported(Unsupported::ShaDma);
        assert_eq!(ctx.diag.drain().len(), 1);
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
