//! GP-SPI2 as a master in CPU-controlled mode (decision 2b-D7; research P §C).
//!
//! A transaction starts at the write of `CMD.USR` and lasts its bits at the clock `CLOCK` sets. At
//! its end the IO matrix delivers its bytes to the chips selected on the pins its signals reach
//! (ADR 0010), the bytes heard land in `W0..W15`, `CMD.USR` falls and `DMA_INT_RAW.TRANS_DONE`
//! rises. What the model does not carry out is refused by name, and the transaction then ends at
//! once without sending anything, so that no firmware waits for ever.
use crate::clock::XTAL_PERIOD;
use crate::ctx::{Ctx, Deadline, Event, Source};
use crate::diag::{Spi2Refusal, Unsupported};
use crate::periph::io::{IoMatrix, Signal};
use crate::periph::{Accepted, SPI2_BASE, Storage, consecutive};
use emu_core::{Level, Peripheral};

/// Offset of an `SPI2` register; an unknown name fails the build.
const fn reg(name: &str) -> u32 {
    svd_regs::esp32c3::addr("SPI2", name) - SPI2_BASE
}

const CMD: u32 = reg("CMD");
const ADDR: u32 = reg("ADDR");
const CTRL: u32 = reg("CTRL");
const CLOCK: u32 = reg("CLOCK");
const USER: u32 = reg("USER");
const USER1: u32 = reg("USER1");
const USER2: u32 = reg("USER2");
const MS_DLEN: u32 = reg("MS_DLEN");
const MISC: u32 = reg("MISC");
const DMA_CONF: u32 = reg("DMA_CONF");
const DMA_INT_ENA: u32 = reg("DMA_INT_ENA");
const DMA_INT_CLR: u32 = reg("DMA_INT_CLR");
const DMA_INT_RAW: u32 = reg("DMA_INT_RAW");
const DMA_INT_ST: u32 = reg("DMA_INT_ST");
const W0: u32 = reg("W0");
const SLAVE: u32 = reg("SLAVE");
const CLK_GATE: u32 = reg("CLK_GATE");
const _: () = assert!(consecutive(
    "SPI2",
    &[
        "W0", "W1", "W2", "W3", "W4", "W5", "W6", "W7", "W8", "W9", "W10", "W11", "W12", "W13",
        "W14", "W15"
    ]
));
/// `W0..W15`: 64 bytes.
const WORDS: usize = 16;
const BUFFER_BYTES: usize = 4 * WORDS;

/// `CMD` fields (`SVD`): `UPDATE` `[23]`, `USR` `[24]`.
const UPDATE: u32 = 1 << 23;
const USR: u32 = 1 << 24;
/// `USER` fields (`SVD`; P §C).
const DOUTDIN: u32 = 1 << 0;
const QPI_MODE: u32 = 1 << 3;
const FWRITE_DUAL: u32 = 1 << 12;
const FWRITE_QUAD: u32 = 1 << 13;
const USR_CONF_NXT: u32 = 1 << 15;
const SIO: u32 = 1 << 17;
const USR_MISO_HIGHPART: u32 = 1 << 24;
const USR_MOSI_HIGHPART: u32 = 1 << 25;
const USR_MOSI: u32 = 1 << 27;
const USR_MISO: u32 = 1 << 28;
const USR_DUMMY: u32 = 1 << 29;
const USR_ADDR: u32 = 1 << 30;
const USR_COMMAND: u32 = 1 << 31;
/// `CTRL`: the dual and quad phases (`FADDR_DUAL/QUAD` bits 5 and 6, `FCMD_DUAL/QUAD` bits 8 and
/// 9, `FREAD_DUAL/QUAD` bits 14 and 15), `RD_BIT_ORDER` `[25]`, `WR_BIT_ORDER` `[26]`.
const CTRL_MULTI_LINE: u32 = (1 << 5) | (1 << 6) | (1 << 8) | (1 << 9) | (1 << 14) | (1 << 15);
const RD_BIT_ORDER: u32 = 1 << 25;
const WR_BIT_ORDER: u32 = 1 << 26;
/// `USER1.USR_ADDR_BITLEN` `[31:27]`, `USER2.USR_COMMAND_BITLEN` `[31:28]`, `USR_COMMAND_VALUE`
/// `[15:0]`.
const ADDR_BITLEN_SHIFT: u32 = 27;
const COMMAND_BITLEN_SHIFT: u32 = 28;
const COMMAND_VALUE_MASK: u32 = 0xFFFF;
/// `MS_DLEN.MS_DATA_BITLEN` `[17:0]`.
const DATA_BITLEN_MASK: u32 = 0x3_FFFF;
/// `MISC`: `CSn_DIS` `[5:0]`, `MASTER_CS_POL` `[12:7]`, `CS_KEEP_ACTIVE` `[30]`.
const CS_DIS_MASK: u32 = 0x3F;
const MASTER_CS_POL_SHIFT: u32 = 7;
const CS_KEEP_ACTIVE: u32 = 1 << 30;
/// `DMA_CONF.DMA_RX_ENA` `[27]`, `DMA_TX_ENA` `[28]`; the afifo resets `[31:29]` are write-only
/// strobes.
const DMA_ENA: u32 = (1 << 27) | (1 << 28);
const DMA_CONF_STROBES: u32 = 0xE000_0000;
/// `SLAVE.MODE` `[26]`, `SLAVE.USR_CONF` `[28]`, `SLAVE.SOFT_RESET` `[27]` (a strobe).
const SLAVE_MODE: u32 = 1 << 26;
const SLAVE_USR_CONF: u32 = 1 << 28;
const SLAVE_SOFT_RESET: u32 = 1 << 27;
/// `CLOCK` fields: `CLKCNT_N` `[17:12]`, `CLKDIV_PRE` `[21:18]`, `CLK_EQU_SYSCLK` `[31]`.
const CLKCNT_N_SHIFT: u32 = 12;
const CLKCNT_N_MASK: u32 = 0x3F;
const CLKDIV_PRE_SHIFT: u32 = 18;
const CLKDIV_PRE_MASK: u32 = 0xF;
const CLK_EQU_SYSCLK: u32 = 1 << 31;
/// `CLK_GATE.MST_CLK_SEL` `[2]`: 1 the default source, the APB clock; 0 the crystal
/// (`spi_ll.h:161-171`).
const MST_CLK_SEL: u32 = 1 << 2;
/// `DMA_INT_*` bit 12: `TRANS_DONE`.
const TRANS_DONE: u32 = 1 << 12;
/// `DMA_INT_RAW`/`ENA`/`ST` hold 21 interrupt bits (`SVD`).
const INT_BITS: u32 = 0x1F_FFFF;

/// Source 19, `GPSPI2_INTR_2` (TRM table 8.3-1).
const SOURCE: Source = Source::of("SPI_INTR_2_MAP");

/// The registers stored without effect: the timing compensation of the input and output lines,
/// the slave's lengths, the version date.
const WHY_TIMING: &str = "line timing compensation: a transfer is delivered whole";
const ACCEPTED: [Accepted; 5] = [
    Accepted::svd("SPI2", "DIN_MODE", WHY_TIMING, "SP8"),
    Accepted::svd("SPI2", "DIN_NUM", WHY_TIMING, "SP8"),
    Accepted::svd("SPI2", "DOUT_MODE", WHY_TIMING, "SP8"),
    Accepted::svd(
        "SPI2",
        "SLAVE1",
        "slave-mode lengths: a transaction in slave mode is refused",
        "SP8",
    ),
    Accepted::svd(
        "SPI2",
        "DATE",
        "none: the version date of the block, stored as written",
        "SP8",
    ),
];

pub(crate) fn accepted() -> Vec<Accepted> {
    ACCEPTED.to_vec()
}

/// The bits of a byte in the other order: a phase sent least significant bit first.
const fn reversed(byte: u8) -> u8 {
    byte.reverse_bits()
}

/// A transaction under way: what it sends and where what it hears goes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Transaction {
    /// The bytes on MOSI, first first, most significant bit first.
    wire: Vec<u8>,
    /// Where the data-in phase is in `wire`, and its length in bytes.
    data_in: Option<(usize, usize)>,
    /// The first word of `W0..W15` the data-in phase lands in.
    data_in_word: usize,
    /// Whether the data-in phase is sent least significant bit first.
    rd_lsb_first: bool,
}

/// GP-SPI2.
#[derive(Debug)]
pub struct Spi2 {
    regs: Storage,
    busy: Option<Transaction>,
    /// The hardware chip selects the last transaction drove active and that are still active
    /// (`CS_KEEP_ACTIVE`), one bit each.
    held: u32,
    wake: Deadline,
}

impl Default for Spi2 {
    fn default() -> Spi2 {
        Spi2::new()
    }
}

impl Spi2 {
    #[must_use]
    pub fn new() -> Spi2 {
        Spi2 {
            regs: Storage::for_peripheral("SPI2", SPI2_BASE),
            busy: None,
            held: 0,
            wake: Deadline::default(),
        }
    }

    fn get(&self, offset: u32) -> u32 {
        self.regs.read(offset).unwrap_or(0)
    }

    fn set(&mut self, offset: u32, value: u32) {
        self.regs.write(offset, value);
    }

    /// `SYSTEM.PERIP_RST_EN0` bit 6, or a core reset: the registers return to their reset values,
    /// a transaction under way is lost and the chip selects it held go inactive.
    pub(crate) fn reset(&mut self, ctx: &mut Ctx, io: &mut IoMatrix) {
        self.wake.cancel(ctx);
        self.release(ctx, io, self.held);
        *self = Spi2::new();
        ctx.irq(SOURCE, false);
    }

    /// The wake-up of the end of a transaction, or the signal of a change of `CS_KEEP_ACTIVE`.
    pub(crate) fn wake(&mut self, ctx: &mut Ctx, io: &mut IoMatrix) {
        self.wake.called(ctx.now());
        if self.wake.due().is_none()
            && let Some(transaction) = self.busy.take()
        {
            self.finish(ctx, io, &transaction);
        }
        if self.get(MISC) & CS_KEEP_ACTIVE == 0 {
            self.release(ctx, io, self.held);
        }
    }

    /// The chip selects `MISC` enables, and the level an active one has.
    fn chip_selects(&self) -> impl Iterator<Item = (Signal, Level)> + use<> {
        let misc = self.get(MISC);
        (0..6u32).filter_map(move |n| {
            let enabled = misc & (1 << n) & CS_DIS_MASK == 0;
            let active_high = (misc >> (MASTER_CS_POL_SHIFT + n)) & 1 != 0;
            enabled.then_some((
                crate::periph::io::FSPICS0 + n,
                Level::from_bool(active_high),
            ))
        })
    }

    /// Drives the inactive level on the held chip selects of `mask`.
    fn release(&mut self, ctx: &mut Ctx, io: &mut IoMatrix, mask: u32) {
        let misc = self.get(MISC);
        for n in 0..6u32 {
            if mask & (1 << n) != 0 {
                let active_high = (misc >> (MASTER_CS_POL_SHIFT + n)) & 1 != 0;
                io.set_signal(
                    ctx,
                    crate::periph::io::FSPICS0 + n,
                    Level::from_bool(!active_high),
                );
            }
        }
        self.held &= !mask;
    }

    /// The end of a transaction: the chip selects go active, the bytes go to the chips, what they
    /// answer lands in `W0..W15`, the chip selects go inactive unless `CS_KEEP_ACTIVE`.
    fn finish(&mut self, ctx: &mut Ctx, io: &mut IoMatrix, transaction: &Transaction) {
        let mut asserted = 0;
        for (signal, active) in self.chip_selects().collect::<Vec<_>>() {
            io.set_signal(ctx, signal, active);
            asserted |= 1 << (signal - crate::periph::io::FSPICS0);
        }
        self.held |= asserted;
        let heard = io.spi_transfer(ctx, &transaction.wire);
        if let Some((start, len)) = transaction.data_in {
            let mut bytes: Vec<u8> = heard[start..start + len].to_vec();
            if transaction.rd_lsb_first {
                for byte in &mut bytes {
                    *byte = reversed(*byte);
                }
            }
            self.store(transaction.data_in_word, &bytes);
        }
        if self.get(MISC) & CS_KEEP_ACTIVE == 0 {
            self.release(ctx, io, asserted);
        }
        self.done(ctx);
    }

    /// `CMD.USR` falls and `TRANS_DONE` rises.
    fn done(&mut self, ctx: &mut Ctx) {
        let cmd = self.get(CMD) & !USR;
        self.set(CMD, cmd);
        let raw = self.get(DMA_INT_RAW) | TRANS_DONE;
        self.set(DMA_INT_RAW, raw);
        self.drive_source(ctx);
    }

    fn drive_source(&self, ctx: &mut Ctx) {
        ctx.irq(SOURCE, self.get(DMA_INT_RAW) & self.get(DMA_INT_ENA) != 0);
    }

    /// The bytes of `W0..W15` from word `first` on, as many as `len`.
    fn buffer(&self, first: usize, len: usize) -> Vec<u8> {
        (first..WORDS)
            .flat_map(|w| {
                self.get(W0 + 4 * u32::try_from(w).unwrap_or(0))
                    .to_le_bytes()
            })
            .take(len)
            .collect()
    }

    /// Writes `bytes` into `W0..W15` from word `first` on; a last partial word keeps its other
    /// bytes.
    fn store(&mut self, first: usize, bytes: &[u8]) {
        for (i, chunk) in bytes.chunks(4).enumerate() {
            let offset = W0 + 4 * u32::try_from(first + i).unwrap_or(0);
            let mut word = self.get(offset).to_le_bytes();
            word[..chunk.len()].copy_from_slice(chunk);
            self.set(offset, u32::from_le_bytes(word));
        }
    }

    /// The transaction the registers describe, or why the model does not carry it out.
    fn transaction(&self) -> Result<(Transaction, u64), Spi2Refusal> {
        let user = self.get(USER);
        let ctrl = self.get(CTRL);
        if self.get(SLAVE) & SLAVE_MODE != 0 {
            return Err(Spi2Refusal::Slave);
        }
        if self.get(SLAVE) & SLAVE_USR_CONF != 0 || user & USR_CONF_NXT != 0 {
            return Err(Spi2Refusal::Segmented);
        }
        if self.get(DMA_CONF) & DMA_ENA != 0 {
            return Err(Spi2Refusal::Dma);
        }
        if ctrl & CTRL_MULTI_LINE != 0 || user & (FWRITE_DUAL | FWRITE_QUAD | QPI_MODE) != 0 {
            return Err(Spi2Refusal::MultiLine);
        }
        if user & SIO != 0 {
            return Err(Spi2Refusal::ThreeWire);
        }
        if user & USR_DUMMY != 0 {
            return Err(Spi2Refusal::Dummy);
        }
        let mut wire = Vec::new();
        if user & USR_COMMAND != 0 {
            let bits = (self.get(USER2) >> COMMAND_BITLEN_SHIFT) + 1;
            let value = self.get(USER2) & COMMAND_VALUE_MASK;
            // `USR_COMMAND_VALUE[7:0]` first, then `[15:8]` (TRM §27.5.8.2).
            match bits {
                8 => wire.push(value.to_le_bytes()[0]),
                16 => wire.extend_from_slice(&value.to_le_bytes()[..2]),
                _ => return Err(Spi2Refusal::CommandLength { bits }),
            }
        }
        if user & USR_ADDR != 0 {
            let bits = (self.get(USER1) >> ADDR_BITLEN_SHIFT) + 1;
            if !bits.is_multiple_of(8) {
                return Err(Spi2Refusal::AddressLength { bits });
            }
            // `USR_ADDR_VALUE[31:24]` first, down (TRM §27.5.8.2).
            let bytes = usize::try_from(bits / 8).unwrap_or(0);
            wire.extend_from_slice(&self.get(ADDR).to_be_bytes()[..bytes]);
        }
        let (mosi, miso) = (user & USR_MOSI != 0, user & USR_MISO != 0);
        let mut data_in = None;
        let mut data_in_word = 0;
        if mosi || miso {
            let bits = (self.get(MS_DLEN) & DATA_BITLEN_MASK) + 1;
            let bytes = usize::try_from(bits / 8).unwrap_or(usize::MAX);
            let room = |highpart: bool| {
                if highpart {
                    BUFFER_BYTES / 2
                } else {
                    BUFFER_BYTES
                }
            };
            if !bits.is_multiple_of(8)
                || (mosi && bytes > room(user & USR_MOSI_HIGHPART != 0))
                || (miso && bytes > room(user & USR_MISO_HIGHPART != 0))
            {
                return Err(Spi2Refusal::DataLength { bits });
            }
            let out_word = if user & USR_MOSI_HIGHPART != 0 {
                WORDS / 2
            } else {
                0
            };
            data_in_word = if user & USR_MISO_HIGHPART != 0 {
                WORDS / 2
            } else {
                0
            };
            let start = wire.len();
            if mosi {
                wire.extend(self.buffer(out_word, bytes));
            }
            if miso {
                if user & DOUTDIN != 0 && mosi {
                    // Full duplex: the data comes back while it goes out.
                    data_in = Some((start, bytes));
                } else {
                    // ASSUMPTION: in a read phase the master leaves MOSI high, as SD cards expect
                    // of a read; the TRM does not say. A half-duplex transaction with both phases
                    // sends then reads, each `MS_DATA_BITLEN + 1` bits long.
                    data_in = Some((wire.len(), bytes));
                    wire.extend(std::iter::repeat_n(0xFF, bytes));
                }
            }
        }
        if ctrl & WR_BIT_ORDER != 0 {
            // A phase sent least significant bit first: what reaches the chip, read most
            // significant bit first, is the byte reversed. The read-only part of a half-duplex
            // read is the line's level and reverses to itself.
            for byte in &mut wire {
                *byte = reversed(*byte);
            }
        }
        let bits = 8 * u64::try_from(wire.len()).unwrap_or(0);
        Ok((
            Transaction {
                wire,
                data_in,
                data_in_word,
                rd_lsb_first: ctrl & RD_BIT_ORDER != 0,
            },
            bits,
        ))
    }

    /// One period of the SPI clock, in cycles of the time base (`spi_ll_master_cal_clock`): the
    /// source divided by `(CLKDIV_PRE + 1) × (CLKCNT_N + 1)`, or the source itself with
    /// `CLK_EQU_SYSCLK`.
    fn clock_period(&self, ctx: &Ctx) -> u64 {
        let source = if self.get(CLK_GATE) & MST_CLK_SEL != 0 {
            ctx.clock().apb_period()
        } else {
            XTAL_PERIOD
        };
        let clock = self.get(CLOCK);
        if clock & CLK_EQU_SYSCLK != 0 {
            return source;
        }
        let pre = u64::from((clock >> CLKDIV_PRE_SHIFT) & CLKDIV_PRE_MASK) + 1;
        let n = u64::from((clock >> CLKCNT_N_SHIFT) & CLKCNT_N_MASK) + 1;
        source * pre * n
    }

    /// `CMD.USR` was set: the transaction starts, and ends after its bits.
    ///
    /// ASSUMPTION: the CS setup and hold phases (`USER.CS_SETUP`, `CS_HOLD`) add no time; the TRM
    /// counts them in SPI clock cycles of the PREP and DONE states, a few at most (P, open
    /// question 4).
    fn start(&mut self, ctx: &mut Ctx) {
        match self.transaction() {
            Ok((transaction, bits)) => {
                let end = ctx.now() + bits.saturating_mul(self.clock_period(ctx));
                self.busy = Some(transaction);
                self.wake.arm(ctx, end, Event::Spi2);
            }
            Err(refusal) => {
                ctx.unsupported(Unsupported::Spi2 { refusal });
                self.done(ctx);
            }
        }
    }
}

impl Peripheral<Ctx> for Spi2 {
    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
        match offset {
            DMA_INT_ST => Some(self.get(DMA_INT_RAW) & self.get(DMA_INT_ENA)),
            // A strobe reads 0.
            DMA_INT_CLR => Some(0),
            _ => self.regs.read(offset),
        }
    }

    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        match offset {
            CMD => {
                // `UPDATE` applies the configuration and clears at once; ESP-IDF spins on it.
                let starts = value & USR != 0 && self.busy.is_none();
                let usr = if self.busy.is_some() { USR } else { 0 };
                self.set(CMD, (value & !(UPDATE | USR)) | usr);
                if starts {
                    self.set(CMD, self.get(CMD) | USR);
                    self.start(ctx);
                }
            }
            DMA_INT_CLR => {
                let raw = self.get(DMA_INT_RAW) & !value;
                self.set(DMA_INT_RAW, raw);
                self.drive_source(ctx);
            }
            DMA_INT_RAW | DMA_INT_ENA => {
                self.set(offset, value & INT_BITS);
                self.drive_source(ctx);
            }
            DMA_INT_ST => return false,
            DMA_CONF => self.set(DMA_CONF, value & !DMA_CONF_STROBES),
            SLAVE => self.set(SLAVE, value & !SLAVE_SOFT_RESET),
            MISC => {
                self.set(MISC, value);
                if value & CS_KEEP_ACTIVE == 0 && self.held != 0 {
                    // The held chip selects go inactive; the matrix is reached by the machine.
                    ctx.signal(Event::Spi2);
                }
            }
            // ASSUMPTION: a transaction under way sends the bytes `W0..W15` held when it started,
            // whatever is written there before it ends; Arduino and ESP-IDF write the buffer only
            // once `USR` has fallen.
            _ => return self.regs.write(offset, value),
        }
        true
    }
}

#[cfg(test)]
mod tests;
