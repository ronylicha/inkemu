//! I2C0 as a master on a bus where no target answers: an Xteink X4 (decision 2b-D9; research P §D).
//!
//! `CTR.TRANS_START` runs the command list `COMD0..COMD7` (TRM §28.4.9). Nothing on the bus ever
//! pulls SDA low, so every byte the controller writes gets a NACK, and every byte it reads is
//! `0xFF` (the lines' pull-ups). A WRITE whose `ack_en` is set stops the sequence at its first byte
//! with `NACK_INT` and a STOP on the bus, as on a real X4 probed for the X3's chips. The effects of
//! a sequence (the `done` bits, the FIFOs, `SR`, the interrupts) appear at the instant its bits end.
use crate::clock::XTAL_PERIOD;
use crate::ctx::{Ctx, Deadline, Event, Source};
use crate::diag::{I2cRefusal, Unsupported};
use crate::periph::{Accepted, I2C0_BASE, Storage, consecutive};
use emu_core::Peripheral;
use std::collections::VecDeque;

/// Offset of an `I2C0` register; an unknown name fails the build.
const fn reg(name: &str) -> u32 {
    svd_regs::esp32c3::addr("I2C0", name) - I2C0_BASE
}

const SCL_LOW_PERIOD: u32 = reg("SCL_LOW_PERIOD");
const CTR: u32 = reg("CTR");
const SR: u32 = reg("SR");
const FIFO_ST: u32 = reg("FIFO_ST");
const FIFO_CONF: u32 = reg("FIFO_CONF");
const DATA: u32 = reg("DATA");
const INT_RAW: u32 = reg("INT_RAW");
const INT_CLR: u32 = reg("INT_CLR");
const INT_ENA: u32 = reg("INT_ENA");
const INT_STATUS: u32 = reg("INT_STATUS");
const SCL_HIGH_PERIOD: u32 = reg("SCL_HIGH_PERIOD");
const SCL_START_HOLD: u32 = reg("SCL_START_HOLD");
const SCL_RSTART_SETUP: u32 = reg("SCL_RSTART_SETUP");
const SCL_STOP_HOLD: u32 = reg("SCL_STOP_HOLD");
const SCL_STOP_SETUP: u32 = reg("SCL_STOP_SETUP");
const CLK_CONF: u32 = reg("CLK_CONF");
const COMD0: u32 = reg("COMD0");
const SCL_SP_CONF: u32 = reg("SCL_SP_CONF");
/// The first of the two RAM windows of non-FIFO mode, which the model does not have.
const TXFIFO_START_ADDR: u32 = reg("TXFIFO_START_ADDR");
const _: () = assert!(consecutive(
    "I2C0",
    &[
        "COMD0", "COMD1", "COMD2", "COMD3", "COMD4", "COMD5", "COMD6", "COMD7"
    ]
));
const COMMANDS: usize = 8;
/// 32-byte FIFOs (`SOC_I2C_FIFO_LEN`).
const FIFO_BYTES: usize = 32;

/// `CTR` (TRM register 28.11): `MS_MODE` `[4]`; the strobes `TRANS_START` `[5]`, `FSM_RST` `[10]`
/// and `CONF_UPGATE` `[11]` read 0.
const MS_MODE: u32 = 1 << 4;
const TRANS_START: u32 = 1 << 5;
const FSM_RST: u32 = 1 << 10;
const CTR_STROBES: u32 = (1 << 5) | (1 << 10) | (1 << 11);
/// `FIFO_CONF` (`SVD`): `RXFIFO_WM_THRHD` `[4:0]`, `TXFIFO_WM_THRHD` `[9:5]`, `NONFIFO_EN`
/// `[10]`, `RX_FIFO_RST` `[12]`, `TX_FIFO_RST` `[13]`.
const WM_MASK: u32 = 0x1F;
const TXFIFO_WM_SHIFT: u32 = 5;
const NONFIFO_EN: u32 = 1 << 10;
const RX_FIFO_RST: u32 = 1 << 12;
const TX_FIFO_RST: u32 = 1 << 13;
/// `SR`: `RESP_REC` `[0]`, `BUS_BUSY` `[4]`, `RXFIFO_CNT` `[13:8]`, `STRETCH_CAUSE` `[15:14]`
/// (3, as at reset: no stretch), `TXFIFO_CNT` `[23:18]`.
const RESP_REC: u32 = 1 << 0;
const BUS_BUSY: u32 = 1 << 4;
const RXFIFO_CNT_SHIFT: u32 = 8;
const NO_STRETCH: u32 = 3 << 14;
const TXFIFO_CNT_SHIFT: u32 = 18;
/// `FIFO_ST` pointers: `RXFIFO_RADDR` `[4:0]`, `RXFIFO_WADDR` `[9:5]`, `TXFIFO_RADDR`
/// `[14:10]`, `TXFIFO_WADDR` `[19:15]`.
const POINTER_MASK: u32 = 0x1F;
/// Interrupt bits (TRM register 28.22).
const RXFIFO_WM: u32 = 1 << 0;
const TXFIFO_WM: u32 = 1 << 1;
const RXFIFO_OVF: u32 = 1 << 2;
const END_DETECT: u32 = 1 << 3;
const BYTE_TRANS_DONE: u32 = 1 << 4;
const MST_TXFIFO_UDF: u32 = 1 << 6;
const TRANS_COMPLETE: u32 = 1 << 7;
const TRANS_START_INT: u32 = 1 << 9;
const NACK: u32 = 1 << 10;
const TXFIFO_OVF: u32 = 1 << 11;
const RXFIFO_UDF: u32 = 1 << 12;
/// `INT_RAW` holds 18 bits.
const INT_BITS: u32 = 0x3_FFFF;
/// `COMDn` (`i2c_ll.h:33-49`): `byte_num` `[7:0]`, `ack_en` `[8]`, `ack_exp` `[9]`, `op_code`
/// `[13:11]`, `done` `[31]`.
const BYTE_NUM_MASK: u32 = 0xFF;
const ACK_EN: u32 = 1 << 8;
const ACK_EXP: u32 = 1 << 9;
const OP_SHIFT: u32 = 11;
const OP_MASK: u32 = 0x7;
const DONE: u32 = 1 << 31;
const OP_WRITE: u32 = 1;
const OP_STOP: u32 = 2;
const OP_READ: u32 = 3;
const OP_END: u32 = 4;
const OP_RSTART: u32 = 6;
/// `CLK_CONF`: `SCLK_DIV_NUM` `[7:0]`, `SCLK_DIV_A` `[13:8]`, `SCLK_DIV_B` `[19:14]`,
/// `SCLK_SEL` `[20]`.
const DIV_NUM_MASK: u32 = 0xFF;
const FRACTION_AND_SEL: u32 = 0x1F_FF00;
/// `SCL_HIGH_PERIOD`: `SCL_HIGH_PERIOD` `[8:0]`, `SCL_WAIT_HIGH_PERIOD` `[15:9]`; the other timing
/// registers hold 9 bits.
const PERIOD_MASK: u32 = 0x1FF;
const WAIT_HIGH_SHIFT: u32 = 9;
const WAIT_HIGH_MASK: u32 = 0x7F;
/// `SCL_SP_CONF.SCL_RST_SLV_EN` `[0]`: the controller sends SCL pulses to free a stuck target, then
/// clears the bit (`i2c_ll.h:799-822`).
const SCL_RST_SLV_EN: u32 = 1 << 0;
/// Clock cycles of SCL in one byte: eight bits and the acknowledge.
const BITS_PER_BYTE: u64 = 9;

/// Source 29, `I2C_EXT0_INTR` (TRM table 8.3-1).
const SOURCE: Source = Source::of("I2C_EXT0_INTR_MAP");

/// The registers stored without effect on a bus where no target answers: what a target could
/// make matter (stretching, timeouts, slave addressing, sampling instants), and the date.
const WHY: &str = "no target: nothing stretches SCL or drives SDA, so these timings and the slave side have nothing to act on";
const ACCEPTED: [Accepted; 9] = [
    Accepted::svd("I2C0", "TO", WHY, "SP3"),
    Accepted::svd("I2C0", "SLAVE_ADDR", WHY, "SP3"),
    Accepted::svd("I2C0", "SDA_HOLD", WHY, "SP3"),
    Accepted::svd("I2C0", "SDA_SAMPLE", WHY, "SP3"),
    Accepted::svd("I2C0", "FILTER_CFG", WHY, "SP3"),
    Accepted::svd("I2C0", "SCL_ST_TIME_OUT", WHY, "SP3"),
    Accepted::svd("I2C0", "SCL_MAIN_ST_TIME_OUT", WHY, "SP3"),
    Accepted::svd("I2C0", "SCL_STRETCH_CONF", WHY, "SP3"),
    Accepted::svd(
        "I2C0",
        "DATE",
        "none: the version date of the block, stored as written",
        "SP3",
    ),
];

pub(crate) fn accepted() -> Vec<Accepted> {
    ACCEPTED.to_vec()
}

/// What a command sequence leaves at its end.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Outcome {
    /// The commands it ran, one bit each, whose `done` bit rises.
    done: u32,
    /// The bytes it takes from the TX FIFO.
    sent: usize,
    /// The bytes it puts in the RX FIFO.
    received: usize,
    /// The interrupt bits it raises.
    raised: u32,
    /// Whether the bus is busy after it (a START without a STOP).
    busy: bool,
    /// Whether the last byte it sent got a NACK.
    nacked: bool,
}

/// I2C0.
#[derive(Debug)]
pub struct I2c0 {
    regs: Storage,
    tx: VecDeque<u8>,
    rx: VecDeque<u8>,
    /// How many bytes each FIFO gave out since its last reset: the read pointers of `FIFO_ST`.
    tx_taken: u32,
    rx_taken: u32,
    busy: bool,
    nacked: bool,
    /// The sequence under way: its outcome, applied when its bits end.
    running: Option<Outcome>,
    wake: Deadline,
}

impl Default for I2c0 {
    fn default() -> I2c0 {
        I2c0::new()
    }
}

impl I2c0 {
    #[must_use]
    pub fn new() -> I2c0 {
        let offsets: Vec<u32> = svd_regs::esp32c3::registers()
            .iter()
            .filter(|r| r.peripheral == "I2C0")
            .map(|r| r.addr - I2C0_BASE)
            // `DATA` is the FIFO port, the two RAM windows of non-FIFO mode are not modelled.
            .filter(|&o| o != DATA && o < TXFIFO_START_ADDR)
            .collect();
        I2c0 {
            regs: Storage::for_offsets(I2C0_BASE, &offsets),
            tx: VecDeque::new(),
            rx: VecDeque::new(),
            tx_taken: 0,
            rx_taken: 0,
            busy: false,
            nacked: false,
            running: None,
            wake: Deadline::default(),
        }
    }

    fn get(&self, offset: u32) -> u32 {
        self.regs.read(offset).unwrap_or(0)
    }

    fn set(&mut self, offset: u32, value: u32) {
        self.regs.write(offset, value);
    }

    /// `SYSTEM.PERIP_RST_EN0` bit 7, or a core reset.
    pub(crate) fn reset(&mut self, ctx: &mut Ctx) {
        self.wake.cancel(ctx);
        *self = I2c0::new();
        ctx.irq(SOURCE, false);
    }

    /// The wake-up of the end of a sequence: its effects appear.
    pub(crate) fn wake(&mut self, ctx: &mut Ctx) {
        self.wake.called(ctx.now());
        if self.wake.due().is_some() {
            return;
        }
        let Some(outcome) = self.running.take() else {
            return;
        };
        for n in 0..COMMANDS {
            if outcome.done & (1 << n) != 0 {
                let offset = COMD0 + 4 * u32::try_from(n).unwrap_or(0);
                self.set(offset, self.get(offset) | DONE);
            }
        }
        for _ in 0..outcome.sent {
            if self.tx.pop_front().is_some() {
                self.tx_taken = self.tx_taken.wrapping_add(1);
            }
        }
        let mut raised = outcome.raised;
        for _ in 0..outcome.received {
            if self.rx.len() < FIFO_BYTES {
                // Nothing drives SDA: the pull-ups make every bit 1.
                self.rx.push_back(0xFF);
            } else {
                raised |= RXFIFO_OVF;
            }
        }
        self.busy = outcome.busy;
        self.nacked = outcome.nacked;
        self.raise(ctx, raised);
    }

    /// Raises interrupt bits, with the watermarks of the FIFOs as they now stand.
    fn raise(&mut self, ctx: &mut Ctx, bits: u32) {
        let conf = self.get(FIFO_CONF);
        let mut raw = self.get(INT_RAW) | bits;
        // ASSUMPTION: a watermark interrupt rises when a sequence or a FIFO access leaves its
        // FIFO at the threshold (TX at or below it, RX at or above); the driver's master path
        // never enables them (`I2C_LL_MASTER_EVENT_INTR`).
        let count = |len: usize| u32::try_from(len).unwrap_or(u32::MAX);
        if count(self.tx.len()) <= (conf >> TXFIFO_WM_SHIFT) & WM_MASK {
            raw |= TXFIFO_WM;
        }
        if count(self.rx.len()) >= conf & WM_MASK && !self.rx.is_empty() {
            raw |= RXFIFO_WM;
        }
        self.set(INT_RAW, raw & INT_BITS);
        self.drive_source(ctx);
    }

    fn drive_source(&self, ctx: &mut Ctx) {
        ctx.irq(SOURCE, self.get(INT_RAW) & self.get(INT_ENA) != 0);
    }

    /// The source clock divided, in cycles of the time base, and the clock the model refuses.
    fn sclk_period(&self, ctx: &mut Ctx) -> u64 {
        let clk_conf = self.get(CLK_CONF);
        if clk_conf & FRACTION_AND_SEL != 0 {
            ctx.unsupported(Unsupported::I2c {
                refusal: I2cRefusal::Clock { clk_conf },
            });
        }
        XTAL_PERIOD * (u64::from(clk_conf & DIV_NUM_MASK) + 1)
    }

    /// The command list as `COMD0..COMD7` hold it, run on a bus nobody answers on: its outcome
    /// and its length in source clock cycles.
    ///
    /// One SCL period is `(SCL_LOW_PERIOD + 1) + SCL_HIGH_PERIOD + SCL_WAIT_HIGH_PERIOD` source
    /// cycles, as ESP-IDF programs it (`i2c_ll.h:172-194`); a byte is nine periods; a START lasts
    /// `SCL_RSTART_SETUP + 1 + SCL_START_HOLD + 1` cycles, a STOP `SCL_STOP_SETUP + 1 +
    /// SCL_STOP_HOLD + 1`.
    ///
    /// ASSUMPTION: a NACK with `ack_en` stops the list at once and the STOP the controller then
    /// generates (TRM §28.4.9) takes a STOP's time and raises no `TRANS_COMPLETE` (P, open question
    /// 5); a list without STOP or END stops after `COMD7`; an op code the TRM does not define stops
    /// the list where it stands.
    fn run(&self) -> (Outcome, u64) {
        let field = |offset: u32| self.get(offset) & PERIOD_MASK;
        let high = self.get(SCL_HIGH_PERIOD);
        let bit = u64::from(
            field(SCL_LOW_PERIOD)
                + 1
                + (high & PERIOD_MASK)
                + ((high >> WAIT_HIGH_SHIFT) & WAIT_HIGH_MASK),
        )
        .max(1);
        let start = u64::from(field(SCL_RSTART_SETUP) + field(SCL_START_HOLD) + 2);
        let stop = u64::from(field(SCL_STOP_SETUP) + field(SCL_STOP_HOLD) + 2);
        let mut outcome = Outcome {
            busy: self.busy,
            ..Outcome::default()
        };
        let mut cycles = 0u64;
        let mut available = self.tx.len();
        'commands: for n in 0..COMMANDS {
            let cmd = self.get(COMD0 + 4 * u32::try_from(n).unwrap_or(0));
            let bytes = usize::try_from(cmd & BYTE_NUM_MASK).unwrap_or(0);
            match (cmd >> OP_SHIFT) & OP_MASK {
                OP_RSTART => {
                    cycles += start;
                    outcome.busy = true;
                    outcome.raised |= TRANS_START_INT;
                }
                OP_WRITE => {
                    for _ in 0..bytes {
                        if available == 0 {
                            outcome.raised |= MST_TXFIFO_UDF;
                            break 'commands;
                        }
                        available -= 1;
                        outcome.sent += 1;
                        cycles += BITS_PER_BYTE * bit;
                        outcome.raised |= BYTE_TRANS_DONE;
                        // No target: the acknowledge bit stays high, a NACK.
                        outcome.nacked = true;
                        if cmd & ACK_EN != 0 && cmd & ACK_EXP == 0 {
                            outcome.raised |= NACK;
                            outcome.busy = false;
                            cycles += stop;
                            break 'commands;
                        }
                    }
                }
                OP_READ => {
                    outcome.received += bytes;
                    cycles += BITS_PER_BYTE * bit * u64::try_from(bytes).unwrap_or(0);
                    outcome.raised |= BYTE_TRANS_DONE;
                }
                OP_STOP => {
                    cycles += stop;
                    outcome.busy = false;
                    outcome.raised |= TRANS_COMPLETE;
                    outcome.done |= 1 << n;
                    break;
                }
                OP_END => {
                    outcome.raised |= END_DETECT;
                    outcome.done |= 1 << n;
                    break;
                }
                _ => break,
            }
            outcome.done |= 1 << n;
        }
        (outcome, cycles)
    }

    /// `CTR.TRANS_START`: the command list runs. A start while a sequence is under way is refused
    /// by name and ignored; a start the model refuses ends at once (`refuse`).
    fn start(&mut self, ctx: &mut Ctx) {
        if self.running.is_some() {
            ctx.unsupported(Unsupported::I2c {
                refusal: I2cRefusal::StartWhileRunning,
            });
            return;
        }
        if self.get(CTR) & MS_MODE == 0 {
            self.refuse(ctx, I2cRefusal::Slave);
            return;
        }
        if self.get(FIFO_CONF) & NONFIFO_EN != 0 {
            self.refuse(ctx, I2cRefusal::NonFifo);
            return;
        }
        let period = self.sclk_period(ctx);
        let (outcome, cycles) = self.run();
        self.busy = true;
        self.running = Some(outcome);
        self.wake
            .arm(ctx, ctx.now() + cycles.saturating_mul(period), Event::I2c0);
    }

    /// A start the model refuses: named, and ended at once as GP-SPI2 ends a refused transaction:
    /// nothing goes on the bus, the bus is idle and `TRANS_COMPLETE` rises, so that no firmware
    /// waits for ever on an end that never comes.
    fn refuse(&mut self, ctx: &mut Ctx, refusal: I2cRefusal) {
        ctx.unsupported(Unsupported::I2c { refusal });
        self.busy = false;
        self.raise(ctx, TRANS_COMPLETE);
    }

    /// `CTR.FSM_RST`: the state machine returns to idle; a sequence under way is abandoned, its
    /// effects never appear, and the bus is idle (the driver resets it so after a timeout,
    /// `s_i2c_hw_fsm_reset`). The FIFOs and the interrupt bits stay.
    fn reset_fsm(&mut self, ctx: &mut Ctx) {
        self.wake.cancel(ctx);
        self.running = None;
        self.busy = false;
    }

    fn status(&self) -> u32 {
        let count = |len: usize| u32::try_from(len).unwrap_or(0);
        let flag = |set: bool, bit: u32| if set { bit } else { 0 };
        flag(self.nacked, RESP_REC)
            | flag(self.busy, BUS_BUSY)
            | (count(self.rx.len()) << RXFIFO_CNT_SHIFT)
            | NO_STRETCH
            | (count(self.tx.len()) << TXFIFO_CNT_SHIFT)
    }

    fn fifo_pointers(&self) -> u32 {
        let pointer = |taken: u32, len: usize| {
            (
                taken & POINTER_MASK,
                taken.wrapping_add(u32::try_from(len).unwrap_or(0)) & POINTER_MASK,
            )
        };
        let (rx_read, rx_write) = pointer(self.rx_taken, self.rx.len());
        let (tx_read, tx_write) = pointer(self.tx_taken, self.tx.len());
        rx_read | rx_write << 5 | tx_read << 10 | tx_write << 15
    }
}

impl Peripheral<Ctx> for I2c0 {
    fn read(&mut self, ctx: &mut Ctx, offset: u32) -> Option<u32> {
        let value = match offset {
            SR => self.status(),
            FIFO_ST => self.fifo_pointers(),
            INT_STATUS => self.get(INT_RAW) & self.get(INT_ENA),
            INT_CLR => 0,
            DATA => {
                if let Some(byte) = self.rx.pop_front() {
                    self.rx_taken = self.rx_taken.wrapping_add(1);
                    u32::from(byte)
                } else {
                    // ASSUMPTION: an empty RX FIFO reads 0 and raises `RXFIFO_UDF`.
                    self.raise(ctx, RXFIFO_UDF);
                    0
                }
            }
            _ => return self.regs.read(offset),
        };
        Some(value)
    }

    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        match offset {
            CTR => {
                self.set(CTR, value & !CTR_STROBES);
                if value & FSM_RST != 0 {
                    self.reset_fsm(ctx);
                }
                if value & TRANS_START != 0 {
                    self.start(ctx);
                }
            }
            DATA => {
                if self.tx.len() < FIFO_BYTES {
                    self.tx.push_back(value.to_le_bytes()[0]);
                } else {
                    // ASSUMPTION: a byte written to a full TX FIFO is dropped.
                    self.raise(ctx, TXFIFO_OVF);
                }
            }
            FIFO_CONF => {
                if value & TX_FIFO_RST != 0 {
                    self.tx.clear();
                    self.tx_taken = 0;
                }
                if value & RX_FIFO_RST != 0 {
                    self.rx.clear();
                    self.rx_taken = 0;
                }
                self.set(FIFO_CONF, value);
            }
            INT_CLR => {
                let raw = self.get(INT_RAW) & !value;
                self.set(INT_RAW, raw);
                self.drive_source(ctx);
            }
            INT_RAW | INT_ENA => {
                self.set(offset, value & INT_BITS);
                self.drive_source(ctx);
            }
            SR | FIFO_ST | INT_STATUS => return false,
            // ASSUMPTION: the pulses that free a stuck target end at once; nothing is stuck.
            SCL_SP_CONF => self.set(SCL_SP_CONF, value & !SCL_RST_SLV_EN),
            _ => return self.regs.write(offset, value),
        }
        true
    }
}

#[cfg(test)]
mod tests;
