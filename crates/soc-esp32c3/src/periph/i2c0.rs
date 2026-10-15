//! I2C0 as a master (decision 2b-D9; research P §D; ADR 0015).
//!
//! `CTR.TRANS_START` runs the command list `COMD0..COMD7` (TRM §28.4.9). The effects of a sequence
//! (the `done` bits, the FIFOs, `SR`, the interrupts) appear at the instant its bits end.
//!
//! - **No chip on the board** (an Xteink X4): nothing on the bus ever pulls SDA low, so every byte
//!   the controller writes gets a NACK, and every byte it reads is `0xFF` (the lines' pull-ups). A
//!   WRITE whose `ack_en` is set stops the sequence at its first byte with `NACK_INT` and a STOP on
//!   the bus, as on a real X4 probed for the X3's chips. The sequence is worked out whole at its
//!   start.
//! - **Chips attached** (an Xteink X3): the sequence runs condition by condition through the GPIO
//!   matrix (`IoMatrix::i2c_address` and its siblings). Each START, byte and STOP takes its time
//!   on the bus, and the chips on the lines the matrix routes see it at the instant it ends; the
//!   first byte after a START is the address. What the controller hears (each acknowledge, each
//!   byte read) decides where the sequence goes: a NACK where `ack_en` expects an ACK ends it
//!   early.
use crate::clock::XTAL_PERIOD;
use crate::ctx::{Ctx, Deadline, Event, Source};
use crate::diag::{I2cRefusal, Unsupported};
use crate::periph::io::IoMatrix;
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
/// `COMDn` (`i2c_ll.h:33-49`): `byte_num` `[7:0]`, `ack_en` `[8]`, `ack_exp` `[9]`, `ack_val`
/// `[10]` (what the master answers to a byte it reads: 0 ACK, 1 NACK), `op_code` `[13:11]`,
/// `done` `[31]`.
const BYTE_NUM_MASK: u32 = 0xFF;
const ACK_EN: u32 = 1 << 8;
const ACK_EXP: u32 = 1 << 9;
const ACK_VAL: u32 = 1 << 10;
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

/// The registers stored without effect: what a chip could only make matter by stretching SCL or
/// by mastering the bus itself (timeouts, slave addressing, sampling instants, filters), and the
/// date. The chips of a board (ADR 0015) answer each bit in time and never stretch.
const WHY: &str = "the chips never stretch SCL and I2C0 is never a slave: these timings, timeouts and the slave side have nothing to act on";
const ACCEPTED: [Accepted; 9] = [
    Accepted::svd("I2C0", "TO", WHY, "SP4"),
    Accepted::svd("I2C0", "SLAVE_ADDR", WHY, "SP4"),
    Accepted::svd("I2C0", "SDA_HOLD", WHY, "SP4"),
    Accepted::svd("I2C0", "SDA_SAMPLE", WHY, "SP4"),
    Accepted::svd("I2C0", "FILTER_CFG", WHY, "SP4"),
    Accepted::svd("I2C0", "SCL_ST_TIME_OUT", WHY, "SP4"),
    Accepted::svd("I2C0", "SCL_MAIN_ST_TIME_OUT", WHY, "SP4"),
    Accepted::svd("I2C0", "SCL_STRETCH_CONF", WHY, "SP4"),
    Accepted::svd(
        "I2C0",
        "DATE",
        "none: the version date of the block, stored as written",
        "SP4",
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

/// The lengths of the conditions on the bus, in cycles of the source clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Timing {
    /// One SCL period.
    bit: u64,
    /// A START or a repeated START.
    start: u64,
    stop: u64,
}

/// The condition of the bus whose end the wake-up of a stepped sequence marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// The START of an RSTART command.
    Start,
    /// The next byte of the TX FIFO, written.
    Write,
    /// A byte read, which the master answers with an ACK when `acknowledged`.
    Read { acknowledged: bool },
    /// A STOP: that of a STOP command (`listed`), or the one a NACK makes the controller send.
    Stop { listed: bool },
}

/// A sequence run condition by condition on a bus with chips: where it stands in the command list
/// and what it left so far.
#[derive(Debug)]
struct Stepping {
    timing: Timing,
    /// Cycles of the time base in one cycle of the source clock.
    period: u64,
    /// The command under way, and how many of its bytes are done.
    command: usize,
    byte: usize,
    step: Step,
    /// The next byte written is an address: a START came before it.
    address_next: bool,
    outcome: Outcome,
    /// The bytes read so far, in order.
    bytes: Vec<u8>,
}

/// The sequence under way.
#[derive(Debug)]
enum Running {
    /// Worked out whole at its start, on a bus without chips: its outcome, applied at its end.
    Whole(Outcome),
    /// Run condition by condition on a bus with chips.
    Stepping(Box<Stepping>),
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
    /// The sequence under way, whose effects appear when its bits end.
    running: Option<Running>,
    wake: Deadline,
    /// The board has attached chips to I2C0 (ADR 0015): a sequence meets them through the GPIO
    /// matrix. The board, not the chip: it survives every reset.
    chips: bool,
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
            chips: false,
        }
    }

    /// The board attached a chip to I2C0: from now on its sequences run on the lines the GPIO
    /// matrix routes.
    pub(crate) fn chips_attached(&mut self) {
        self.chips = true;
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
        let chips = self.chips;
        *self = I2c0::new();
        self.chips = chips;
        ctx.irq(SOURCE, false);
    }

    /// The wake-up of the end of a sequence, or of a condition of a stepped one: its effects
    /// appear, or the next condition is due.
    pub(crate) fn wake(&mut self, ctx: &mut Ctx, io: &mut IoMatrix) {
        self.wake.called(ctx.now());
        if self.wake.due().is_some() {
            return;
        }
        match self.running.take() {
            None => {}
            Some(Running::Whole(outcome)) => {
                // Nothing drives SDA: the pull-ups make every bit 1.
                let bytes = vec![0xFF; outcome.received];
                self.publish(ctx, &outcome, &bytes);
            }
            Some(Running::Stepping(stepping)) => self.step(ctx, io, stepping),
        }
    }

    /// The `COMDn` register of command `n`.
    fn command(&self, n: usize) -> u32 {
        self.get(COMD0 + 4 * u32::try_from(n).unwrap_or(0))
    }

    /// The condition `stepping` was waiting for has ended now: the chips see it, then the next one
    /// is armed, or the sequence ends.
    fn step(&mut self, ctx: &mut Ctx, io: &mut IoMatrix, mut stepping: Box<Stepping>) {
        let cmd = self.command(stepping.command);
        let bit = 1 << stepping.command;
        match stepping.step {
            Step::Start => {
                stepping.outcome.busy = true;
                stepping.outcome.raised |= TRANS_START_INT;
                stepping.address_next = true;
                stepping.outcome.done |= bit;
                stepping.command += 1;
            }
            Step::Write => {
                let byte = self.tx.get(stepping.outcome.sent).copied().unwrap_or(0xFF);
                let acknowledged = if stepping.address_next {
                    stepping.address_next = false;
                    io.i2c_address(ctx, byte)
                } else {
                    io.i2c_write(ctx, byte)
                };
                stepping.outcome.sent += 1;
                stepping.outcome.raised |= BYTE_TRANS_DONE;
                stepping.outcome.nacked = !acknowledged;
                stepping.byte += 1;
                // `ack_exp` names a NACK by 1: the acknowledge differs from it when an ACK came
                // where a NACK was expected, or the reverse.
                let expected_nack = cmd & ACK_EXP != 0;
                if cmd & ACK_EN != 0 && acknowledged == expected_nack {
                    stepping.outcome.raised |= NACK;
                    stepping.step = Step::Stop { listed: false };
                    let cycles = stepping.timing.stop;
                    self.arm(ctx, stepping, cycles);
                    return;
                }
            }
            Step::Read { acknowledged } => {
                let byte = io.i2c_read(ctx, acknowledged);
                stepping.bytes.push(byte);
                stepping.outcome.received += 1;
                stepping.outcome.raised |= BYTE_TRANS_DONE;
                stepping.byte += 1;
            }
            Step::Stop { listed } => {
                io.i2c_stop(ctx);
                stepping.outcome.busy = false;
                if listed {
                    stepping.outcome.raised |= TRANS_COMPLETE;
                    stepping.outcome.done |= bit;
                }
                self.finish(ctx, &stepping);
                return;
            }
        }
        self.plan(ctx, stepping);
    }

    /// Finds the next condition of `stepping` from its command under way, and arms the instant it
    /// ends; the sequence ends when its list does (an END, a command the TRM does not define, the
    /// TX FIFO run dry, past `COMD7`).
    ///
    /// ASSUMPTION: as for a bus without chips (`run`), a list without STOP or END stops after
    /// `COMD7`, and an op code the TRM does not define stops the list where it stands.
    fn plan(&mut self, ctx: &mut Ctx, mut stepping: Box<Stepping>) {
        while stepping.command < COMMANDS {
            let cmd = self.command(stepping.command);
            let bytes = usize::try_from(cmd & BYTE_NUM_MASK).unwrap_or(0);
            let bit = 1 << stepping.command;
            let byte_cycles = BITS_PER_BYTE * stepping.timing.bit;
            let (step, cycles) = match (cmd >> OP_SHIFT) & OP_MASK {
                OP_RSTART => (Step::Start, stepping.timing.start),
                OP_WRITE | OP_READ if stepping.byte >= bytes => {
                    stepping.outcome.done |= bit;
                    stepping.command += 1;
                    stepping.byte = 0;
                    continue;
                }
                OP_WRITE if stepping.outcome.sent >= self.tx.len() => {
                    stepping.outcome.raised |= MST_TXFIFO_UDF;
                    break;
                }
                OP_WRITE => (Step::Write, byte_cycles),
                OP_READ => (
                    Step::Read {
                        acknowledged: cmd & ACK_VAL == 0,
                    },
                    byte_cycles,
                ),
                OP_STOP => (Step::Stop { listed: true }, stepping.timing.stop),
                OP_END => {
                    stepping.outcome.raised |= END_DETECT;
                    stepping.outcome.done |= bit;
                    break;
                }
                _ => break,
            };
            stepping.step = step;
            self.arm(ctx, stepping, cycles);
            return;
        }
        self.finish(ctx, &stepping);
    }

    /// Keeps `stepping` under way and arms the end of its condition, `cycles` of the source clock
    /// from now.
    fn arm(&mut self, ctx: &mut Ctx, stepping: Box<Stepping>, cycles: u64) {
        let at = ctx.now() + cycles.saturating_mul(stepping.period);
        self.running = Some(Running::Stepping(stepping));
        self.wake.arm(ctx, at, Event::I2c0);
    }

    /// The stepped sequence ends now: its effects appear.
    fn finish(&mut self, ctx: &mut Ctx, stepping: &Stepping) {
        self.running = None;
        self.publish(ctx, &stepping.outcome, &stepping.bytes);
    }

    /// The effects of a sequence that ends now: its `done` bits, the bytes it took from the TX
    /// FIFO, `bytes` in the RX FIFO, the state of the bus and its interrupts.
    fn publish(&mut self, ctx: &mut Ctx, outcome: &Outcome, bytes: &[u8]) {
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
        for &byte in bytes {
            if self.rx.len() < FIFO_BYTES {
                self.rx.push_back(byte);
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
        let Timing { bit, start, stop } = self.timing();
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
        if self.chips {
            let stepping = Box::new(Stepping {
                timing: self.timing(),
                period,
                command: 0,
                byte: 0,
                step: Step::Start,
                address_next: false,
                outcome: Outcome {
                    busy: self.busy,
                    ..Outcome::default()
                },
                bytes: Vec::new(),
            });
            self.busy = true;
            self.plan(ctx, stepping);
            return;
        }
        let (outcome, cycles) = self.run();
        self.busy = true;
        self.running = Some(Running::Whole(outcome));
        self.wake
            .arm(ctx, ctx.now() + cycles.saturating_mul(period), Event::I2c0);
    }

    /// The lengths of a START, a STOP and one bit, as the timing registers set them.
    fn timing(&self) -> Timing {
        let field = |offset: u32| self.get(offset) & PERIOD_MASK;
        let high = self.get(SCL_HIGH_PERIOD);
        let bit = u64::from(
            field(SCL_LOW_PERIOD)
                + 1
                + (high & PERIOD_MASK)
                + ((high >> WAIT_HIGH_SHIFT) & WAIT_HIGH_MASK),
        )
        .max(1);
        Timing {
            bit,
            start: u64::from(field(SCL_RSTART_SETUP) + field(SCL_START_HOLD) + 2),
            stop: u64::from(field(SCL_STOP_SETUP) + field(SCL_STOP_HOLD) + 2),
        }
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
