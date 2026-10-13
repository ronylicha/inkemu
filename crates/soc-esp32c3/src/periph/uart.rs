//! UART controller. Register layout: `uart_reg.h` (reports BC §G.2, BA §7, UR §2).
//!
//! A byte written to the FIFO leaves at the baud rate: the transmit FIFO fills, `STATUS` and
//! `FSM_STATUS` say so, and the byte reaches the console when its last bit has gone. Software waits
//! on both: the ROM before each character, ESP-IDF before it changes a clock or restarts (report IR
//! §4.1).
//!
//! A byte the host sends lands in the receive FIFO one frame after its start bit (report UR
//! §5.4): the bytes of one input go out back to back from the device time they are given, at the
//! rate and in the frame the UART is set to. `STATUS` and a read of `FIFO` work the count out from
//! device time when they are accessed (ADR 0003 §4), and the UART asks to be called at each
//! instant a byte lands or an interrupt condition begins.
//!
//! The interrupt (report UR §4): sources 21 (`UART_INTR_MAP`) and 22 (`UART1_INTR_MAP`), level,
//! driven as `INT_RAW & INT_ENA != 0` after every change, as the SYSTIMER drives its own. Five
//! bits of `INT_RAW` have an effect: `RXFIFO_FULL`, `TXFIFO_EMPTY`, `RXFIFO_OVF`, `RXFIFO_TOUT`
//! and `TX_DONE` (report UR §10); the others are never set.
use crate::clock::XTAL_PERIOD;
use crate::ctx::{Ctx, Deadline, Event, Source};
use crate::diag::{UartFeature, Unsupported};
use crate::periph::Storage;
use emu_core::{Cycles, Peripheral};
use std::collections::VecDeque;

/// The interrupt source of each UART (report UR §4).
const SOURCES: [Source; 2] = [Source::of("UART_INTR_MAP"), Source::of("UART1_INTR_MAP")];

/// The model serves UART0 and UART1, whose register offsets are identical: those of UART0 name them.
const fn reg(name: &str) -> u32 {
    svd_regs::esp32c3::addr("UART0", name) - svd_regs::esp32c3::addr("UART0", "FIFO")
}

const FIFO: u32 = reg("FIFO");
const INT_RAW: u32 = reg("INT_RAW");
const INT_ST: u32 = reg("INT_ST");
const INT_ENA: u32 = reg("INT_ENA");
const INT_CLR: u32 = reg("INT_CLR");
const CLKDIV: u32 = reg("CLKDIV");
const STATUS: u32 = reg("STATUS");
const FSM_STATUS: u32 = reg("FSM_STATUS");
const CONF0: u32 = reg("CONF0");
const CONF1: u32 = reg("CONF1");
const CLK_CONF: u32 = reg("CLK_CONF");
const IDLE_CONF: u32 = reg("IDLE_CONF");
const RS485_CONF: u32 = reg("RS485_CONF");
const AT_CMD_CHAR: u32 = reg("AT_CMD_CHAR");
const MEM_CONF: u32 = reg("MEM_CONF");
const MEM_RX_STATUS: u32 = reg("MEM_RX_STATUS");
/// What software writes and reads back. `CLKDIV` and `CLK_CONF` set the baud rate. The ESP-IDF
/// driver writes `IDLE_CONF` and `RS485_CONF` when it configures the port and reads
/// `AT_CMD_CHAR` twice in each pass of its interrupt handler (report UR §3.3): stored, with no
/// effect of their own (`RS485_CONF` other than 0 is refused).
const CONFIG: [u32; 9] = [
    INT_ENA,
    CLKDIV,
    CONF0,
    CONF1,
    CLK_CONF,
    IDLE_CONF,
    RS485_CONF,
    AT_CMD_CHAR,
    MEM_CONF,
];
/// `STATUS` bits [9:0] `RXFIFO_CNT` and [25:16] `TXFIFO_CNT`.
const FIFO_COUNTS: u32 = 0x03FF_03FF;
const TXFIFO_CNT_SHIFT: u32 = 16;
/// `CONF0` bit 17 `RXFIFO_RST`: empties the receive FIFO; a byte that lands while it is set is
/// lost (`ROM3:Uart_Init` and `uart_param_config` pulse it; report UR §3.1, §3.3, A8).
const RXFIFO_RST: u32 = 1 << 17;
/// `CLK_CONF` bit 25 `RX_SCLK_EN`: the receiver's clock; a byte that lands while it is clear is
/// lost (report UR §5.4 point 5, A8).
const RX_SCLK_EN: u32 = 1 << 25;
/// `MEM_CONF` bits [3:1] `RX_SIZE` and [6:4] `TX_SIZE`: the 128-byte blocks of the UART memory
/// each FIFO has. Both are 1 at reset and in every measured configuration (report UR §2.2); the
/// model has one block each and refuses another size.
const MEM_SIZES: u32 = 0b111_1110;
const ONE_BLOCK_EACH: u32 = 0b001_0010;
/// `MEM_CONF` bits [25:16] `RX_TOUT_THRHD`: the receive timeout, in bit times (report UR §5.2).
const RX_TOUT_THRHD_SHIFT: u32 = 16;
const TEN_BITS: u32 = 0x3FF;
/// `CONF1` (report UR §2.2): bits [8:0] `RXFIFO_FULL_THRHD`, [17:9] `TXFIFO_EMPTY_THRHD`, bit 18
/// `DIS_RX_DAT_OVF` ("Disable UART Rx data overflow detect"), bit 21 `RX_TOUT_EN`.
const NINE_BITS: u32 = 0x1FF;
const TXFIFO_EMPTY_THRHD_SHIFT: u32 = 9;
const DIS_RX_DAT_OVF: u32 = 1 << 18;
const RX_TOUT_EN: u32 = 1 << 21;
/// The bits of `INT_RAW` the model sets (`uart_struct.h:20-45`, report UR §2.3).
const RXFIFO_FULL: u32 = 1 << 0;
const TXFIFO_EMPTY: u32 = 1 << 1;
const RXFIFO_OVF: u32 = 1 << 4;
const RXFIFO_TOUT: u32 = 1 << 8;
const TX_DONE: u32 = 1 << 14;
/// `FSM_STATUS` bits [7:4] `ST_UTX_OUT`: 0 when the transmitter is idle, the only value software
/// tests (`ROM3:uart_tx_wait_idle@0x4004c4c6`; BA §7).
/// ASSUMPTION: while a byte is being sent the field reads 1. The other states are not documented
/// in the reports and nothing on the measured paths tells them apart.
const ST_UTX_OUT_BUSY: u32 = 1 << 4;
/// The transmit FIFO holds 128 bytes: the ROM waits while `TXFIFO_CNT` is 128 or more
/// (`ROM3:uart_tx_one_char_uart.part.0@0x4004c0be`; BC §G.2).
const FIFO_BYTES: usize = 128;
/// `CLKDIV`: bits [11:0] the integer part of the divider, bits [23:20] its fraction in sixteenths
/// (`ROM3:uart_div_modify@0x4004c20c` writes both: `0x30015B` is 347 + 3/16; BC §G.2, BA §7).
const CLKDIV_INTEGER_MASK: u32 = 0xFFF;
const CLKDIV_FRAG_SHIFT: u32 = 20;
const CLKDIV_FRAG_MASK: u32 = 0xF;
/// `CLK_CONF`: bits [19:12] `SCLK_DIV_NUM`, the divider of the source clock minus one (the
/// bootloader clears it: `bootloader_console_init`, store at `0x403cc8b6` in trace F1), bits
/// [21:20] `SCLK_SEL`, 1 = `APB_CLK` (`case UART_SCLK_APB: hw->clk_conf.sclk_sel = 1;`,
/// `components/hal/esp32c3/include/hal/uart_ll.h:175-176`, ESP-IDF v5.5.2), 3 = the crystal
/// (`ROM3:Uart_Init` sets 3, store at `0x4004c2e2` in trace F1; BA §7: "the UART core clock is
/// the crystal"; reset value 3 in `uart_reg.h:1189`, ESP-IDF v5.5.2).
const SCLK_DIV_NUM_SHIFT: u32 = 12;
const SCLK_DIV_NUM_MASK: u32 = 0xFF;
const SCLK_SEL_SHIFT: u32 = 20;
const SCLK_SEL_MASK: u32 = 0b11;
const SCLK_SEL_APB: u32 = 1;
const SCLK_SEL_XTAL: u32 = 3;
/// `CONF0` bit 18 `TXFIFO_RST`: empties the transmit FIFO (`ROM3:Uart_Init@0x4004c306` sets it,
/// then clears it; BC §G.2).
const TXFIFO_RST: u32 = 1 << 18;
/// The frame format of `CONF0` (report UR §5.1, from `uart_ll_set_parity`,
/// `uart_ll_set_data_bit_num` and `uart_ll_set_stop_bits`, `IDF:hal/esp32c3/include/hal/uart_ll.h`):
/// bit 1 `PARITY_EN` adds a parity bit; bits [3:2] `BIT_NUM` give 5 + `BIT_NUM` data bits;
/// bits [5:4] `STOP_BIT_NUM` give 1, 1.5 or 2 stop bits for 1, 2 or 3.
const PARITY_EN: u32 = 1 << 1;
const BIT_NUM_SHIFT: u32 = 2;
const STOP_BIT_NUM_SHIFT: u32 = 4;
const TWO_BITS: u32 = 0b11;
/// The modes of `CONF0` the model refuses by name when they are set: loopback, an inverted
/// receive line and autobaud (report UR §10); and what would change the bytes on the line or
/// their timing: CTS flow control, an inverted transmit line, infrared framing, a break after the
/// data. Bit
/// positions from `UART_*_S` in `IDF:components/soc/esp32c3/register/soc/uart_reg.h` (ESP-IDF
/// v5.5.2). ESP-IDF writes the last four as 0 unless asked for them (`uart_ll_set_hw_flow_ctrl`,
/// `uart_ll_inverse_signal`, `uart_ll_set_mode_normal`, `uart_ll_tx_break`,
/// `IDF:components/hal/esp32c3/include/hal/uart_ll.h`), and every console uses no flow control.
const CONF0_REFUSED: [(u32, UartFeature); 7] = [
    (1 << 8, UartFeature::TxBreak),
    (1 << 14, UartFeature::Loopback),
    (1 << 15, UartFeature::TxFlowControl),
    (1 << 16, UartFeature::Irda),
    (1 << 19, UartFeature::InvertedRx),
    (1 << 22, UartFeature::InvertedTx),
    (1 << 27, UartFeature::Autobaud),
];
/// `CONF1` bit 20 `RX_FLOW_EN`: RTS flow control of the receiver, refused by name when set (the
/// host of the model sends at its own pace; `UART_RX_FLOW_EN_S`, `uart_reg.h`).
const RX_FLOW_EN: u32 = 1 << 20;
/// Device time is kept in thirty-seconds of a time-base cycle here: a bit lasts a whole number of
/// sixteenths (the fraction of `CLKDIV`), and a frame may end on half a bit (1.5 stop bits).
const SUBCYCLES: u64 = 32;
/// The last device time a byte of the host can be dated at: in thirty-seconds of a cycle, in a
/// `u64`, it is `u64::MAX / 32` cycles, about 114 years at 160 MHz. A later date would saturate
/// and the byte start before it, so the machine refuses it ([`Uart::receive`]).
pub const LAST_INPUT_DATE: Cycles = Cycles(u64::MAX / SUBCYCLES);

/// The length of a frame in half bit times: a start bit, the data bits, the parity bit, the stop
/// bits (report UR §5.1, §6). `RS485_CONF.DL0_EN`/`DL1_EN` would each add a bit; the model
/// refuses RS485 mode instead.
///
/// ASSUMPTION: `STOP_BIT_NUM` = 0, which no source defines (report UR A4), lasts one stop bit;
/// it is refused by name when written. A frame timed on a chip set that way settles it.
fn frame_half_bits(conf0: u32) -> u64 {
    let data_bits = 5 + u64::from((conf0 >> BIT_NUM_SHIFT) & TWO_BITS);
    let parity_bits = u64::from((conf0 & PARITY_EN) >> 1);
    let stop_half_bits = match (conf0 >> STOP_BIT_NUM_SHIFT) & TWO_BITS {
        2 => 3,
        3 => 4,
        _ => 2,
    };
    2 * (1 + data_bits + parity_bits) + stop_half_bits
}

#[derive(Debug)]
pub struct Uart {
    /// 0 for UART0, 1 for UART1: named in diagnostics.
    index: u8,
    base: u32,
    /// `STATUS` with both FIFO counts at zero: the line-state bits keep their reset value.
    idle_status: u32,
    config: Storage,
    /// `MEM_RX_STATUS`, served at its reset value: nothing reads it (report UR §2.2).
    mem_rx_status: u32,
    /// The bytes on their way out, each with the time, in thirty-seconds of a time-base cycle, at
    /// which its last bit has gone. The first is in the shift register, the others in the FIFO.
    in_flight: VecDeque<(u64, u8)>,
    /// The bytes that have left, for the console.
    tx: Vec<u8>,
    /// The bytes the host sends and that have not started, in order. The caller bounds them
    /// (`MAX_PENDING_HOST_BYTES`, [`Uart::host_pending`]).
    host: VecDeque<u8>,
    /// The dates of `host`, in runs: the device time, in thirty-seconds, before which the next
    /// byte of the run does not start, and how many bytes of `host` the run still holds. An input
    /// dated no later than the run before it joins that run: its bytes follow the run's anyway.
    /// So an input costs a run only when it is dated later, at most one per byte of `host`.
    host_runs: VecDeque<(u64, usize)>,
    /// The byte on the line: the instant its frame ends, when it lands, and the byte.
    receiving: Option<(u64, u8)>,
    /// When the line is free for the next byte: the end of the last frame that started.
    line_free_at: u64,
    /// The receive FIFO.
    rx: VecDeque<u8>,
    /// `INT_RAW`.
    int_raw: u32,
    /// The instant the last byte landed, while the receive timeout of that idle period has not
    /// come: `None` once it came, and from the start bit of the next byte to its landing.
    idle_since: Option<u64>,
    /// The call at the next instant the device has something to do: a byte of its transmitter
    /// leaves (so that a CPU that waits for an interrupt with bytes on their way out sleeps
    /// until they have left, not for ever), a byte of the host lands, the receive timeout comes,
    /// or the transmit FIFO falls below its threshold.
    wake: Deadline,
    /// The machine watches what this UART sends: each byte asks for a call at the instant it has
    /// left, so that the machine sees it go at that instant ([`Uart::watch`]).
    watched: bool,
    /// The `SCLK_SEL` without a frequency in the model that timed a byte of the host or a
    /// receive timeout since the firmware last looked at the receiver: refused at that look
    /// ([`Uart::report_mistimed_reception`]).
    rx_mistimed: Option<u32>,
}

impl Uart {
    #[must_use]
    pub fn new(index: u8, base: u32) -> Uart {
        let reset =
            |offset: u32| svd_regs::esp32c3::lookup(base + offset).map_or(0, |r| r.reset_value);
        Uart {
            index,
            base,
            idle_status: reset(STATUS) & !FIFO_COUNTS,
            config: Storage::for_offsets(base, &CONFIG),
            mem_rx_status: reset(MEM_RX_STATUS),
            in_flight: VecDeque::new(),
            tx: Vec::new(),
            host: VecDeque::new(),
            host_runs: VecDeque::new(),
            receiving: None,
            line_free_at: 0,
            rx: VecDeque::new(),
            // `TXFIFO_EMPTY` at reset: the SVD's `0x00000002`, the condition with an empty FIFO
            // and the reset threshold, 96.
            int_raw: TXFIFO_EMPTY,
            idle_since: None,
            wake: Deadline::default(),
            watched: false,
            rx_mistimed: None,
        }
    }

    /// While `watched`, each byte asks for a call at the instant its last bit has gone, not only
    /// the last one: the machine then sees each byte leave at its own instant, and a front end
    /// that waits for a text knows when it came (ADR 0006). A call changes nothing the firmware
    /// can see.
    pub(crate) fn watch(&mut self, ctx: &mut Ctx, watched: bool) {
        self.watched = watched;
        self.arm(ctx);
    }

    /// Whether bytes have left that no call has taken.
    #[must_use]
    pub(crate) fn has_left(&self) -> bool {
        !self.tx.is_empty()
    }

    /// The module is reset (`SYSTEM.PERIP_RST_EN0`, or a core reset): its registers return to their
    /// reset values, the bytes that have not left are lost, and so are those in the receive FIFO.
    /// Those that left stay for the console. The host is no part of the chip: the bytes it is
    /// sending go on arriving, and land in the FIFO of the reset UART.
    pub fn reset(&mut self, ctx: &mut Ctx) {
        self.settle(ctx);
        self.config = Storage::for_offsets(self.base, &CONFIG);
        self.in_flight.clear();
        self.rx.clear();
        self.int_raw = 0;
        self.idle_since = None;
        self.assert_levels();
        self.drive(ctx);
        self.arm(ctx);
    }

    /// The bytes that have left the transmitter by now, since the last call.
    pub fn take_tx(&mut self, ctx: &mut Ctx) -> Vec<u8> {
        self.settle(ctx);
        std::mem::take(&mut self.tx)
    }

    /// The host sends `bytes`, the first starting at device time `at` (now or later, and no later
    /// than [`LAST_INPUT_DATE`]), the others right after it, and after the bytes it sent before.
    /// The caller bounds how many it hands over ([`Uart::host_pending`]) and their date. Past the
    /// last date, the arithmetic of the line saturates there as device time does at its end
    /// (`Ctx::schedule`): the bytes that the line would end after it land at it.
    ///
    /// ASSUMPTION: the host sends at the baud rate and in the frame the UART is set to, as a
    /// terminal set to match it does; a host at another rate would give framing errors, which the
    /// model does not produce. Every console measured is 8N1 at 115 200 baud (report UR §6).
    pub fn receive(&mut self, ctx: &mut Ctx, at: Cycles, bytes: &[u8]) {
        self.settle(ctx);
        let date = at.max(ctx.now()).get().saturating_mul(SUBCYCLES);
        if bytes.is_empty() {
            return;
        }
        match self.host_runs.back_mut() {
            // The bytes start after those of the run, which start at its date or later.
            Some((run_date, count)) if *run_date >= date => *count += bytes.len(),
            _ => self.host_runs.push_back((date, bytes.len())),
        }
        self.host.extend(bytes);
        self.arm(ctx);
    }

    /// The next byte of the host to start, and the device time, in thirty-seconds, before which
    /// it does not start.
    fn next_host_byte(&self) -> Option<(u64, u8)> {
        let &(date, _) = self.host_runs.front()?;
        self.host.front().map(|&byte| (date, byte))
    }

    /// The next byte of the host has started.
    fn pop_host_byte(&mut self) {
        self.host.pop_front();
        if let Some((_, count)) = self.host_runs.front_mut() {
            *count -= 1;
            if *count == 0 {
                self.host_runs.pop_front();
            }
        }
    }

    /// How many bytes the host has sent that have not landed yet.
    #[must_use]
    pub fn host_pending(&self) -> usize {
        self.host.len() + usize::from(self.receiving.is_some())
    }

    /// Every byte the firmware wrote and no call has taken, those that have not left yet
    /// included: the transmission ends here. For the end of a run, so that the console shows
    /// what the firmware had written when it stopped.
    pub fn take_written(&mut self) -> Vec<u8> {
        let mut written = std::mem::take(&mut self.tx);
        written.extend(self.in_flight.drain(..).map(|(_, byte)| byte));
        written
    }

    /// How many bytes that left wait for [`Uart::take_tx`]: what a caller that never takes them
    /// would let grow.
    #[cfg(test)]
    pub(crate) fn waiting_for_the_console(&self) -> usize {
        self.tx.len()
    }

    /// The device time of a wake-up has come: the bytes that have gone leave, those of the host
    /// that have arrived land, and the device asks to be called again at the next such instant.
    pub(crate) fn wake(&mut self, ctx: &mut Ctx) {
        self.wake.called(ctx.now());
        self.settle(ctx);
        self.arm(ctx);
    }

    /// The clocks are about to change (a write of the `SYSTEM` clock registers): with `SCLK_SEL`
    /// = 1 the UART counts `APB_CLK`, which a register of the UART does not change. It comes to
    /// now first, so that a byte of the host whose start bit came before the change is timed with
    /// the clock of then, and a timeout due by then comes, whether or not something called the
    /// UART since: the device time of an input is fixed by the inputs alone (ADR 0006).
    pub(crate) fn clock_changing(&mut self, ctx: &mut Ctx) {
        self.settle(ctx);
    }

    /// The clocks changed: the instants worked out with the old `APB_CLK` (the landing of the
    /// next byte, the timeout) are worked out again, and a timeout the new rate puts before now
    /// comes now.
    pub(crate) fn clock_changed(&mut self, ctx: &mut Ctx) {
        self.settle(ctx);
        self.arm(ctx);
    }

    /// Asks to be called at the next instant an interrupt condition can begin: the last byte of
    /// the transmitter leaves (`TX_DONE`), the transmit FIFO falls below its threshold
    /// (`TXFIFO_EMPTY`), the next byte of the host lands (`RXFIFO_FULL`, `RXFIFO_OVF`), the
    /// receive timeout comes (`RXFIFO_TOUT`). A landing that changes no bit is a call all the
    /// same: one per byte received, the price of being right whatever the firmware enables.
    fn arm(&mut self, ctx: &mut Ctx) {
        let next_landing = self.next_landing(ctx);
        let last_gone = self.in_flight.back().map(|&(gone_at, _)| gone_at);
        // A timeout that is not enabled raises nothing at its instant: no call.
        let timeout_enabled = self.config.read(CONF1).unwrap_or(0) & RX_TOUT_EN != 0;
        let timeout = self.timeout_at(ctx).filter(|_| timeout_enabled);
        let below_threshold = self.tx_below_threshold_at();
        let first_gone = self
            .in_flight
            .front()
            .map(|&(gone_at, _)| gone_at)
            .filter(|_| self.watched);
        for instant in [
            last_gone,
            next_landing,
            timeout,
            below_threshold,
            first_gone,
        ]
        .into_iter()
        .flatten()
        {
            self.wake.arm(
                ctx,
                Cycles(instant.div_ceil(SUBCYCLES)),
                Event::Uart(self.index),
            );
        }
    }

    /// The instant the next byte of the host lands: the one on the line, or the next to start,
    /// whose frame is timed with the settings of now. They hold until a register write or a
    /// change of the clocks, and each brings the UART to now first (`write`,
    /// [`Uart::clock_changing`]).
    fn next_landing(&self, ctx: &Ctx) -> Option<u64> {
        if let Some((lands_at, _)) = self.receiving {
            return Some(lands_at);
        }
        let (date, _) = self.next_host_byte()?;
        Some(
            date.max(self.line_free_at)
                .saturating_add(self.byte_time(self.source_period(ctx))),
        )
    }

    /// The instant the transmit FIFO count falls below `TXFIFO_EMPTY_THRHD`, when it is not below
    /// it yet: once `n − threshold` of the `n` bytes on their way out have left (the first is in
    /// the shift register, so the count is `n − 1`). A threshold of 0 is never crossed.
    fn tx_below_threshold_at(&self) -> Option<u64> {
        let threshold = usize::try_from(self.tx_empty_threshold()).unwrap_or(usize::MAX);
        let on_the_way = self.in_flight.len();
        if threshold == 0 || on_the_way <= threshold {
            return None;
        }
        self.in_flight
            .get(on_the_way - threshold - 1)
            .map(|&(gone_at, _)| gone_at)
    }

    fn tx_empty_threshold(&self) -> u32 {
        (self.config.read(CONF1).unwrap_or(0) >> TXFIFO_EMPTY_THRHD_SHIFT) & NINE_BITS
    }

    /// The instant the receive timeout of the current idle period comes: `RX_TOUT_THRHD` bit
    /// times after the last byte landed (report UR §5.2), timed with the settings of now.
    ///
    /// ASSUMPTION: a change of the bit time during an idle period (`CLKDIV`, `CLK_CONF`, or
    /// `APB_CLK` with `SCLK_SEL` = 1) times the whole period again, from the landing, with the
    /// new bit time; a timeout that this puts before the change comes at the change. How the
    /// chip's counter follows such a change is not documented, and no measured path changes the
    /// rate while a driver waits for input. A logic analyser on the receive line of a chip whose
    /// clock is switched during an idle period, with a GPIO toggled by the handler, settles it.
    fn timeout_at(&self, ctx: &Ctx) -> Option<u64> {
        let since = self.idle_since?;
        let threshold = (self.config.read(MEM_CONF).unwrap_or(0) >> RX_TOUT_THRHD_SHIFT) & TEN_BITS;
        Some(since.saturating_add(u64::from(threshold) * self.bit_time(self.source_period(ctx))))
    }

    /// Brings the device to now: the bytes whose last bit has gone move to the console, the bytes
    /// of the host whose frame has ended land, and the receive timeout comes, in the order of
    /// their instants; the interrupt source follows.
    fn settle(&mut self, ctx: &mut Ctx) {
        let now = ctx.now().get().saturating_mul(SUBCYCLES);
        let was_sending = !self.in_flight.is_empty();
        while let Some(&(gone_at, byte)) = self.in_flight.front() {
            if gone_at > now {
                break;
            }
            self.tx.push(byte);
            self.in_flight.pop_front();
        }
        if was_sending && self.in_flight.is_empty() {
            self.int_raw |= TX_DONE;
        }
        // INVARIANT: each pass lands a byte, starts one, ends the timeout or ends the loop, and
        // the bytes are those the caller handed over within its bound (`host_pending`).
        loop {
            let timeout = self.timeout_at(ctx).filter(|&at| at <= now);
            if self.receiving.is_none() {
                let Some((date, byte)) = self.next_host_byte() else {
                    if timeout.is_some() {
                        self.time_out();
                        continue;
                    }
                    break;
                };
                let starts_at = date.max(self.line_free_at);
                // A start bit at the instant the timeout comes finds the line idle until then:
                // the timeout comes first.
                if timeout.is_some_and(|at| at <= starts_at) {
                    self.time_out();
                    continue;
                }
                if starts_at > now {
                    break;
                }
                // The frame of a byte is the one the UART is set to when it starts: a register
                // write and a change of the clocks bring the UART to now before they change it.
                // ASSUMPTION: as a byte sent keeps the duration it was given (`send`), a byte
                // received keeps the bit time of its start bit; a later change of the rate
                // applies to the bytes that start after it. A logic-analyser capture of a byte
                // received while `CLKDIV` is rewritten settles it. A start bit ends the idle line:
                // the timeout it put off is not asked for again (`arm`), and the landing starts
                // the count again.
                let lands_at = starts_at.saturating_add(self.byte_time(self.source_period(ctx)));
                if let Some(sclk_sel) = self.unmodelled_source() {
                    self.rx_mistimed = Some(sclk_sel);
                }
                self.pop_host_byte();
                self.receiving = Some((lands_at, byte));
                self.line_free_at = lands_at;
                self.idle_since = None;
            }
            match self.receiving {
                Some((lands_at, byte)) if lands_at <= now => {
                    self.receiving = None;
                    // A receiver whose clock is off counts no idle line (`land`).
                    if self.land(ctx, byte) {
                        self.idle_since = Some(lands_at);
                    }
                }
                _ => break,
            }
        }
        self.assert_levels();
        self.drive(ctx);
    }

    /// The receive timeout came: it is raised when it is enabled and the FIFO holds data, and
    /// comes once per idle period.
    ///
    /// ASSUMPTION: `RXFIFO_TOUT` needs a byte in the FIFO and an idle line for `RX_TOUT_THRHD`
    /// bit times after the last byte landed; a read of the FIFO does not start the count again;
    /// it comes once per idle period (report UR A3). QEMU raises it with an empty FIFO; ESP-IDF
    /// leaves a byte in the FIFO "in order to trigger `uart_intr_rxfifo_tout`" (`uart.c:1275-1277`).
    /// A logic analyser on the receive line of a chip, with a GPIO toggled by the handler,
    /// settles it. A timeout that comes while the receiver's clock is off is not raised (`land`).
    fn time_out(&mut self) {
        self.idle_since = None;
        let conf1 = self.config.read(CONF1).unwrap_or(0);
        let clk_conf = self.config.read(CLK_CONF).unwrap_or(0);
        if conf1 & RX_TOUT_EN == 0 || clk_conf & RX_SCLK_EN == 0 {
            return;
        }
        // An enabled timeout was timed with the bit time: on a clock the model does not have,
        // its instant is wrong whether or not it raises the bit.
        if let Some(sclk_sel) = self.unmodelled_source() {
            self.rx_mistimed = Some(sclk_sel);
        }
        if !self.rx.is_empty() {
            self.int_raw |= RXFIFO_TOUT;
        }
    }

    /// The firmware looks at the receiver (`FIFO`, `STATUS`, `INT_RAW`, `INT_ST`): what it sees
    /// of a byte of the host, or of a receive timeout, timed with a source clock the model does
    /// not have is refused by name there, at the access. The access is an instant the firmware
    /// fixes, whatever the calls the run is cut into; the instant a byte starts is not one, since
    /// any call may bring the UART to it.
    fn report_mistimed_reception(&mut self, ctx: &mut Ctx) {
        if let Some(sclk_sel) = self.rx_mistimed.take() {
            ctx.unsupported(Unsupported::UartReceiveClock {
                uart: self.index,
                sclk_sel,
            });
        }
    }

    /// Sets the bits whose condition holds now.
    ///
    /// ASSUMPTION: `RXFIFO_FULL` (the FIFO holds more than `RXFIFO_FULL_THRHD` bytes) and
    /// `TXFIFO_EMPTY` (the transmit FIFO holds fewer than `TXFIFO_EMPTY_THRHD`) are levels: a
    /// write of `INT_CLR` while the condition holds sets the bit again at once, and a set bit
    /// stays until it is cleared, after the condition ends too; "more than" is strict (report UR
    /// A1, from "receives more data than"). The ESP-IDF driver works with either reading (it
    /// drains the FIFO before it clears `RXFIFO_FULL`, and disables `TXFIFO_EMPTY` before it
    /// clears it), and so does the ROM's download handler. Clearing the bit on a chip with the
    /// FIFO above the threshold, and sending exactly `RXFIFO_FULL_THRHD` bytes, settles it.
    fn assert_levels(&mut self) {
        let conf1 = self.config.read(CONF1).unwrap_or(0);
        let full_threshold = usize::try_from(conf1 & NINE_BITS).unwrap_or(usize::MAX);
        if self.rx.len() > full_threshold {
            self.int_raw |= RXFIFO_FULL;
        }
        let in_fifo = self.in_flight.len().saturating_sub(1);
        if in_fifo < usize::try_from(self.tx_empty_threshold()).unwrap_or(usize::MAX) {
            self.int_raw |= TXFIFO_EMPTY;
        }
    }

    /// The level of the source: `INT_ST`, `INT_RAW & INT_ENA`, other than 0.
    fn drive(&self, ctx: &mut Ctx) {
        // INVARIANT: the machine builds UART 0 and UART 1 only.
        if let Some(&source) = SOURCES.get(usize::from(self.index)) {
            ctx.irq(source, self.int_status() != 0);
        }
    }

    fn int_status(&self) -> u32 {
        self.int_raw & self.config.read(INT_ENA).unwrap_or(0)
    }

    /// A byte of the host has been received whole: it enters the FIFO. Returns whether the
    /// receiver heard it, its clock on: the idle line that follows is counted from it then.
    ///
    /// ASSUMPTION: a byte that lands while the receiver's clock is off (`CLK_CONF.RX_SCLK_EN`
    /// clear) or while `CONF0.RXFIFO_RST` is set is lost (report UR A8), and a byte that finds
    /// 128 bytes in the FIFO is dropped while the 128 stay (report UR A2). Input sent to a chip
    /// during its boot, and 200 bytes sent to a FIFO nobody reads, settle them.
    ///
    /// ASSUMPTION: the receive timeout is counted with the receiver's clock: a byte lost because
    /// that clock is off starts no idle period, and a timeout that comes while it is off is not
    /// raised (`time_out`); a byte the FIFO reset or a full FIFO drops was heard, and starts one.
    /// No source says; ESP-IDF v5.5.2 clears `RX_SCLK_EN` only when it deletes a driver
    /// (`uart_ll_sclk_disable`, `IDF:components/esp_driver_uart/src/uart.c:295`). Bytes sent to
    /// a chip whose receiver clock is gated with data in its FIFO and the timeout enabled settle
    /// it.
    fn land(&mut self, ctx: &mut Ctx, byte: u8) -> bool {
        let clk_conf = self.config.read(CLK_CONF).unwrap_or(0);
        let conf0 = self.config.read(CONF0).unwrap_or(0);
        if clk_conf & RX_SCLK_EN == 0 {
            return false;
        }
        if conf0 & RXFIFO_RST != 0 {
            return true;
        }
        if self.rx.len() >= FIFO_BYTES {
            ctx.unsupported(Unsupported::UartRxOverflow { uart: self.index });
            if self.config.read(CONF1).unwrap_or(0) & DIS_RX_DAT_OVF == 0 {
                self.int_raw |= RXFIFO_OVF;
            }
            return true;
        }
        self.rx.push_back(byte);
        true
    }

    /// A read of `FIFO`: the oldest byte received.
    ///
    /// ASSUMPTION: a read of an empty receive FIFO returns 0 and changes nothing (report UR A6);
    /// it is refused by name. Only firmware that reads without looking at `RXFIFO_CNT` does it;
    /// a chip settles it.
    fn pop(&mut self, ctx: &mut Ctx) -> u32 {
        if let Some(byte) = self.rx.pop_front() {
            return u32::from(byte);
        }
        ctx.unsupported(Unsupported::UartEmptyRead { uart: self.index });
        0
    }

    /// The period of the source clock the UART times its bits with now, in time-base cycles.
    ///
    /// `SCLK_SEL` 1 is `APB_CLK` and 3 the crystal. 2 selects the RC fast clock
    /// (`case UART_SCLK_RTC: hw->clk_conf.sclk_sel = 2;`,
    /// `components/hal/esp32c3/include/hal/uart_ll.h:178-179`, ESP-IDF v5.5.2), 17.5 MHz nominal,
    /// and 0 has no documented source (`uart_reg.h:1189` documents only the reset value, 3). The
    /// model has a clock for neither ([`Uart::unmodelled_source`]): it times with the crystal so
    /// that a lenient run goes on, and refuses by name what that timing gives, instead of a
    /// duration that is silently wrong (the calibration of the timer groups measures the same
    /// oscillator at its 17.5 MHz): a byte sent ([`Unsupported::UartClock`]), a byte received or
    /// a receive timeout ([`Unsupported::UartReceiveClock`]), each a stop under `--strict-mmio`.
    /// Nothing on the measured paths selects either.
    fn source_period(&self, ctx: &Ctx) -> u64 {
        if self.source_selection() == SCLK_SEL_APB {
            ctx.clock().apb_period()
        } else {
            XTAL_PERIOD
        }
    }

    /// `CLK_CONF.SCLK_SEL`.
    fn source_selection(&self) -> u32 {
        (self.config.read(CLK_CONF).unwrap_or(0) >> SCLK_SEL_SHIFT) & SCLK_SEL_MASK
    }

    /// The `SCLK_SEL` of now when the model has no frequency for the clock it selects.
    fn unmodelled_source(&self) -> Option<u32> {
        match self.source_selection() {
            SCLK_SEL_APB | SCLK_SEL_XTAL => None,
            sclk_sel => Some(sclk_sel),
        }
    }

    /// The time one byte takes, in thirty-seconds of a time-base cycle: the bits of a frame
    /// ([`frame_half_bits`]), each `CLKDIV` (with its fraction) cycles of the source clock divided
    /// by `SCLK_DIV_NUM + 1`. The fractional divider of the source clock (`SCLK_DIV_A/B`) is
    /// ignored: it resets to 0 and no measured code writes it. The 10 bit times of 8N1 give the
    /// durations the model gave every byte before it read the frame format.
    ///
    /// ASSUMPTION: the divider is what the register holds, down to 0. An integer part of 0 with
    /// a fraction divides by that fraction of a cycle, and a `CLKDIV` of 0 gives a byte no
    /// duration: it leaves in the instant it is written, as fast as firmware can write. No source
    /// says what the chip does below a divider of 1 (ESP-IDF never programs one); a byte timed on
    /// a chip with `CLKDIV` = 0 settles it.
    fn byte_time(&self, source_period: u64) -> u64 {
        let conf0 = self.config.read(CONF0).unwrap_or(0);
        frame_half_bits(conf0) * self.half_bit_time(source_period)
    }

    /// The time of one bit, in thirty-seconds of a time-base cycle: what `RX_TOUT_THRHD` counts.
    fn bit_time(&self, source_period: u64) -> u64 {
        2 * self.half_bit_time(source_period)
    }

    /// Half a bit time, in thirty-seconds of a time-base cycle: the divider in sixteenths of a
    /// source-clock cycle.
    fn half_bit_time(&self, source_period: u64) -> u64 {
        let clkdiv = self.config.read(CLKDIV).unwrap_or(0);
        let clk_conf = self.config.read(CLK_CONF).unwrap_or(0);
        let divider = u64::from(clkdiv & CLKDIV_INTEGER_MASK) * 16
            + u64::from((clkdiv >> CLKDIV_FRAG_SHIFT) & CLKDIV_FRAG_MASK);
        let source_divider = u64::from((clk_conf >> SCLK_DIV_NUM_SHIFT) & SCLK_DIV_NUM_MASK) + 1;
        divider * source_divider * source_period
    }

    /// Refuses by name the modes of `conf0` the model does not have (report UR §10, A4).
    fn refuse_modes(&self, ctx: &mut Ctx, conf0: u32) {
        for (bit, feature) in CONF0_REFUSED {
            if conf0 & bit != 0 {
                ctx.unsupported(Unsupported::UartFeature {
                    uart: self.index,
                    feature,
                });
            }
        }
        if (conf0 >> STOP_BIT_NUM_SHIFT) & TWO_BITS == 0 {
            ctx.unsupported(Unsupported::UartFeature {
                uart: self.index,
                feature: UartFeature::NoStopBit,
            });
        }
    }

    fn send(&mut self, ctx: &mut Ctx, byte: u8) {
        // One byte is in the shift register, the FIFO holds the others.
        if self.in_flight.len() > FIFO_BYTES {
            ctx.unsupported(Unsupported::UartOverflow { uart: self.index });
            return;
        }
        if let Some(sclk_sel) = self.unmodelled_source() {
            ctx.unsupported(Unsupported::UartClock {
                uart: self.index,
                sclk_sel,
            });
        }
        let byte_time = self.byte_time(self.source_period(ctx));
        // ASSUMPTION: a byte keeps the duration it was given when it was written: a later change
        // of `CLKDIV`, `CLK_CONF` or the APB clock applies to the bytes written after it. What
        // silicon does with the bits not sent yet is not documented (the divider most likely
        // acts on them from the change on). ESP-IDF waits for the transmitter to be idle before
        // it changes a clock or restarts (report IR §4.1), so no measured path tells. A
        // logic-analyser capture of a byte whose `CLKDIV` is rewritten while it is sent settles
        // it.
        let now = ctx.now().get().saturating_mul(SUBCYCLES);
        let starts_at = self.in_flight.back().map_or(now, |&(gone_at, _)| gone_at);
        let gone_at = starts_at.max(now).saturating_add(byte_time);
        self.in_flight.push_back((gone_at, byte));
        self.wake.arm(
            ctx,
            Cycles(gone_at.div_ceil(SUBCYCLES)),
            Event::Uart(self.index),
        );
    }
}

impl Peripheral<Ctx> for Uart {
    fn read(&mut self, ctx: &mut Ctx, offset: u32) -> Option<u32> {
        self.settle(ctx);
        if matches!(offset, FIFO | STATUS | INT_RAW | INT_ST) {
            self.report_mistimed_reception(ctx);
        }
        match offset {
            FIFO => Some(self.pop(ctx)),
            STATUS => {
                let in_fifo = u32::try_from(self.in_flight.len().saturating_sub(1)).unwrap_or(0);
                // INVARIANT: the receive FIFO holds at most `FIFO_BYTES` bytes (`land`).
                let received = u32::try_from(self.rx.len()).unwrap_or(0);
                Some(self.idle_status | (in_fifo << TXFIFO_CNT_SHIFT) | received)
            }
            // `ST_URX_OUT` (bits [3:0]) stays 0: nothing reads it (report UR §2.2).
            FSM_STATUS => Some(if self.in_flight.is_empty() {
                0
            } else {
                ST_UTX_OUT_BUSY
            }),
            MEM_RX_STATUS => Some(self.mem_rx_status),
            INT_RAW => Some(self.int_raw),
            INT_ST => Some(self.int_status()),
            _ => self.config.read(offset),
        }
    }

    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        self.settle(ctx);
        let accepted = match offset {
            FIFO => {
                self.send(ctx, value.to_le_bytes()[0]);
                true
            }
            // Write 1 to clear (`uart_reg.h:422-530`); a level whose condition holds is set again
            // below (`assert_levels`).
            INT_CLR => {
                self.int_raw &= !value;
                true
            }
            CONF0 => {
                if value & TXFIFO_RST != 0 {
                    // The byte in the shift register is on the wire already: it goes on.
                    self.in_flight.truncate(1);
                }
                if value & RXFIFO_RST != 0 {
                    self.rx.clear();
                }
                self.refuse_modes(ctx, value);
                self.config.write(offset, value)
            }
            CONF1 => {
                if value & RX_FLOW_EN != 0 {
                    ctx.unsupported(Unsupported::UartFeature {
                        uart: self.index,
                        feature: UartFeature::RxFlowControl,
                    });
                }
                self.config.write(offset, value)
            }
            MEM_CONF => {
                if value & MEM_SIZES != ONE_BLOCK_EACH {
                    ctx.unsupported(Unsupported::UartFeature {
                        uart: self.index,
                        feature: UartFeature::FifoSize,
                    });
                }
                self.config.write(offset, value)
            }
            RS485_CONF => {
                if value != 0 {
                    ctx.unsupported(Unsupported::UartFeature {
                        uart: self.index,
                        feature: UartFeature::Rs485,
                    });
                }
                self.config.write(offset, value)
            }
            _ => self.config.write(offset, value),
        };
        // A threshold, an enable or a clear may have changed a bit or the level; the baud rate
        // or the frame may have changed the instant the next byte lands.
        self.assert_levels();
        self.drive(ctx);
        self.arm(ctx);
        accepted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    const UART0: u32 = svd_regs::esp32c3::addr("UART0", "FIFO");
    const UART1: u32 = svd_regs::esp32c3::addr("UART1", "FIFO");
    /// What the ROM and the bootloader leave: the crystal undivided, 347 + 3/16 (BA §7).
    const CLKDIV_115200: u32 = 0x0030_015B;
    const CLK_CONF_XTAL: u32 = 0x0370_0000;
    /// One byte at 115 200 baud, 8N1: 20 half bits x (347 x 16 + 3) x 4 thirty-seconds,
    /// 13 887.5 cycles, 86.8 us.
    const BYTE_SUBCYCLES: u64 = 20 * (347 * 16 + 3) * 4;

    fn console(ctx: &mut Ctx) -> Uart {
        let mut uart = Uart::new(0, UART0);
        assert!(uart.write(ctx, CLK_CONF, CLK_CONF_XTAL));
        assert!(uart.write(ctx, CLKDIV, CLKDIV_115200));
        uart
    }

    fn after_bytes(ctx: &mut Ctx, bytes: u64) {
        ctx.sleep_until(ctx.now() + (bytes * BYTE_SUBCYCLES).div_ceil(SUBCYCLES));
    }

    fn fifo_count(ctx: &mut Ctx, uart: &mut Uart) -> u32 {
        (uart.read(ctx, STATUS).unwrap() >> TXFIFO_CNT_SHIFT) & 0x3FF
    }

    #[test]
    fn bytes_leave_one_at_a_time_at_the_baud_rate_in_order_and_only_once() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        for byte in *b"ESP" {
            assert!(uart.write(ctx, FIFO, u32::from(byte)));
        }
        assert_eq!(uart.take_tx(ctx), [], "nothing has left yet");
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx), b"E");
        assert_eq!(uart.take_tx(ctx), [], "taken once");
        after_bytes(ctx, 2);
        assert_eq!(uart.take_tx(ctx), b"SP");
    }

    #[test]
    fn only_the_low_byte_of_a_fifo_write_is_transmitted() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        uart.write(ctx, FIFO, 0xFFFF_FF41);
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx), b"A");
    }

    #[test]
    fn the_fifo_count_and_the_transmitter_state_follow_the_bytes_out() {
        // BA §7: `uart_tx_flush` waits for TXFIFO_CNT = 0, `uart_tx_wait_idle` for ST_UTX_OUT = 0.
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        assert_eq!(
            uart.read(ctx, STATUS),
            Some(0xE000_C000),
            "idle: the SVD reset value"
        );
        assert_eq!(uart.read(ctx, FSM_STATUS), Some(0));
        for _ in 0..3 {
            uart.write(ctx, FIFO, 0x55);
        }
        assert_eq!(
            fifo_count(ctx, &mut uart),
            2,
            "one byte is in the shift register"
        );
        assert_eq!(uart.read(ctx, FSM_STATUS), Some(ST_UTX_OUT_BUSY));
        after_bytes(ctx, 2);
        assert_eq!(
            fifo_count(ctx, &mut uart),
            0,
            "empty, the last byte still going out"
        );
        assert_eq!(uart.read(ctx, FSM_STATUS), Some(ST_UTX_OUT_BUSY));
        after_bytes(ctx, 1);
        assert_eq!(uart.read(ctx, FSM_STATUS), Some(0));
        assert_eq!(uart.read(ctx, STATUS), Some(0xE000_C000));
        assert!(!uart.write(ctx, FSM_STATUS, 0xFF), "read-only");
        assert!(!uart.write(ctx, STATUS, 0xFFFF_FFFF), "read-only");
    }

    #[test]
    fn a_byte_written_to_an_idle_transmitter_starts_at_once() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        uart.write(ctx, FIFO, u32::from(b'a'));
        after_bytes(ctx, 10);
        uart.write(ctx, FIFO, u32::from(b'b'));
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx), b"ab");
    }

    #[test]
    fn the_baud_rate_follows_the_divider_and_the_source_clock() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        // The crystal divided by 2: a byte takes twice as long.
        uart.write(ctx, CLK_CONF, CLK_CONF_XTAL | (1 << SCLK_DIV_NUM_SHIFT));
        uart.write(ctx, FIFO, u32::from(b'x'));
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx), []);
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx), b"x");
        // APB_CLK as the source, 20 MHz after power-on: twice as long as with the crystal.
        uart.write(ctx, CLK_CONF, CLK_CONF_XTAL & !(0b10 << SCLK_SEL_SHIFT));
        uart.write(ctx, FIFO, u32::from(b'y'));
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx), []);
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx), b"y");
    }

    #[test]
    fn a_byte_written_to_a_full_fifo_is_dropped_and_reported() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        for byte in 0..=128u32 {
            uart.write(ctx, FIFO, byte);
        }
        assert_eq!(
            fifo_count(ctx, &mut uart),
            128,
            "the ROM waits at this count"
        );
        assert_eq!(ctx.take_unsupported(), None);
        assert!(uart.write(ctx, FIFO, 0xEE));
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::UartOverflow { uart: 0 })
        );
        after_bytes(ctx, 130);
        let sent = uart.take_tx(ctx);
        assert_eq!(sent.len(), 129);
        assert!(!sent.contains(&0xEE));
    }

    #[test]
    fn a_reset_loses_the_bytes_that_have_not_left_and_keeps_those_that_have() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        for byte in *b"gone lost" {
            uart.write(ctx, FIFO, u32::from(byte));
        }
        after_bytes(ctx, 4);
        uart.reset(ctx);
        after_bytes(ctx, 20);
        assert_eq!(uart.take_tx(ctx), b"gone");
        assert_eq!(
            uart.read(ctx, CLKDIV),
            Some(0x2B6),
            "registers back to their reset values"
        );
        assert_eq!(uart.read(ctx, FSM_STATUS), Some(0));
    }

    #[test]
    fn the_transmit_fifo_reset_keeps_only_the_byte_being_sent() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        for byte in *b"abc" {
            uart.write(ctx, FIFO, u32::from(byte));
        }
        // `Uart_Init`: CONF0 with bits 26, 18 and 17 set, then without bits 18 and 17 (BC §G.2).
        uart.write(ctx, CONF0, 0x0406_001C);
        uart.write(ctx, CONF0, 0x0400_001C);
        assert_eq!(fifo_count(ctx, &mut uart), 0);
        after_bytes(ctx, 3);
        assert_eq!(uart.take_tx(ctx), b"a");
    }

    #[test]
    fn line_configuration_is_storage_from_its_reset_value() {
        let ctx = &mut Ctx::default();
        let mut uart = Uart::new(0, UART0);
        for offset in [INT_ENA, CLKDIV, CONF0, CONF1, CLK_CONF] {
            let reset = svd_regs::esp32c3::lookup(UART0 + offset)
                .unwrap()
                .reset_value;
            assert_eq!(uart.read(ctx, offset), Some(reset), "{offset:#x}");
            assert!(uart.write(ctx, offset, 0x15B));
            assert_eq!(uart.read(ctx, offset), Some(0x15B));
        }
    }

    #[test]
    fn the_raw_state_resets_to_txfifo_empty_and_int_st_is_the_enabled_part_of_it() {
        // SVD: INT_RAW resets to 0x00000002; UR §4: INT_ST = INT_RAW & INT_ENA.
        let ctx = &mut Ctx::default();
        let mut uart = Uart::new(0, UART0);
        assert_eq!(uart.read(ctx, INT_RAW), Some(TXFIFO_EMPTY));
        assert_eq!(uart.read(ctx, INT_ST), Some(0));
        assert_eq!(ctx.sources(), 0);
        uart.write(ctx, INT_ENA, TXFIFO_EMPTY);
        assert_eq!(uart.read(ctx, INT_ST), Some(TXFIFO_EMPTY));
        assert_eq!(ctx.sources(), 1 << 21, "source 21, UART_INTR_MAP");
        uart.write(ctx, INT_CLR, 0xFFFF_FFFF);
        assert_eq!(
            uart.read(ctx, INT_RAW),
            Some(TXFIFO_EMPTY),
            "a level set again at once: the FIFO is still below its threshold"
        );
        uart.write(ctx, INT_ENA, 0);
        assert_eq!(ctx.sources(), 0, "disabled: the source falls");
        assert!(
            !uart.write(ctx, INT_RAW, 0),
            "set by the device, cleared through INT_CLR"
        );
        assert!(!uart.write(ctx, INT_ST, 0), "read-only");
    }

    /// What `uart_driver_install` leaves (UR §3.3, run S2, as the chip reads back): the timeout at
    /// 100 bit times and enabled, the full threshold at 120, the transmit threshold at its reset
    /// value 96, `INT_ENA` = `0x00080195`.
    fn driver(ctx: &mut Ctx) -> Uart {
        let mut uart = console(ctx);
        uart.write(ctx, INT_ENA, 0);
        uart.write(ctx, INT_CLR, 0x7FFFF);
        uart.write(ctx, MEM_CONF, 0x0064_0012);
        uart.write(ctx, CONF1, 0x0020_C078);
        uart.write(ctx, INT_ENA, 0x0008_0195);
        assert_eq!(ctx.take_unsupported(), None);
        uart
    }

    /// The instant of the timeout after the `n`th byte sent at 0 lands: 100 bit times later.
    fn timed_out_after(n: u64) -> Cycles {
        Cycles((n * 20 * (347 * 16 + 3) * 4 + 100 * 2 * (347 * 16 + 3) * 4).div_ceil(SUBCYCLES))
    }

    #[test]
    fn a_short_burst_raises_the_timeout_100_bit_times_after_its_last_byte_and_the_handler_reads_it()
    {
        // UR §9.2: `bc\r\n` in one burst; UR §3.3: the handler reads INT_ST, STATUS, each byte,
        // AT_CMD_CHAR twice, writes INT_CLR = 0x101 and reads INT_ST again.
        let ctx = &mut Ctx::default();
        let mut uart = driver(ctx);
        uart.receive(ctx, Cycles::ZERO, b"bc\r\n");
        run_to(ctx, &mut uart, landed(4));
        assert_eq!(
            uart.read(ctx, INT_ST),
            Some(0),
            "four bytes: under the threshold of 120"
        );
        assert_eq!(
            ctx.next_wake(),
            Some(timed_out_after(4)),
            "the call at the timeout"
        );
        ctx.sleep_until(Cycles(timed_out_after(4).get() - 1));
        assert_eq!(uart.read(ctx, INT_ST), Some(0));
        ctx.sleep_until(timed_out_after(4));
        assert_eq!(ctx.take_due(), Some(Event::Uart(0)));
        uart.wake(ctx);
        assert_eq!(
            ctx.sources(),
            1 << 21,
            "raised at its instant, before any access"
        );
        assert_eq!(uart.read(ctx, INT_ST), Some(RXFIFO_TOUT));
        assert_eq!(uart.read(ctx, STATUS).unwrap() & 0x3FF, 4);
        let read: Vec<u32> = (0..4).map(|_| uart.read(ctx, FIFO).unwrap()).collect();
        assert_eq!(read, [98, 99, 13, 10]);
        assert_eq!(uart.read(ctx, AT_CMD_CHAR), Some(0x32B));
        uart.write(ctx, INT_CLR, RXFIFO_TOUT | RXFIFO_FULL);
        assert_eq!(uart.read(ctx, INT_ST), Some(0));
        assert_eq!(ctx.sources(), 0);
        assert_eq!(ctx.take_unsupported(), None);
    }

    #[test]
    fn the_timeout_comes_once_per_idle_period_and_never_with_an_empty_fifo() {
        // UR A3: QEMU raises it as soon as the timeout is configured; the chip needs data.
        let ctx = &mut Ctx::default();
        let mut uart = driver(ctx);
        ctx.sleep_until(timed_out_after(10));
        assert_eq!(uart.read(ctx, INT_RAW).unwrap() & RXFIFO_TOUT, 0, "no data");
        assert_eq!(ctx.next_wake(), None);
        // A byte, read before the timeout: the FIFO is empty when it comes.
        uart.receive(ctx, Cycles::ZERO, b"a");
        let start = ctx.now();
        ctx.sleep_until(start + landed(1).get());
        uart.read(ctx, FIFO);
        ctx.sleep_until(start + timed_out_after(1).get());
        assert_eq!(
            uart.read(ctx, INT_RAW).unwrap() & RXFIFO_TOUT,
            0,
            "empty FIFO"
        );
        // Two bytes, one left in the FIFO: the timeout comes, once.
        let start = ctx.now();
        uart.receive(ctx, start, b"xy");
        ctx.sleep_until(start + landed(2).get());
        uart.read(ctx, FIFO);
        ctx.sleep_until(start + timed_out_after(2).get());
        assert_eq!(
            uart.read(ctx, INT_RAW).unwrap() & RXFIFO_TOUT,
            RXFIFO_TOUT,
            "a read does not start the count again"
        );
        uart.write(ctx, INT_CLR, RXFIFO_TOUT);
        ctx.sleep_until(start + timed_out_after(20).get());
        assert_eq!(
            uart.read(ctx, INT_RAW).unwrap() & RXFIFO_TOUT,
            0,
            "once per idle period"
        );
    }

    #[test]
    fn a_byte_that_starts_before_the_timeout_comes_puts_it_off() {
        // A start bit ends the idle line: a byte that follows within 100 bit times of the last one
        // puts the timeout 100 bit times after itself.
        let ctx = &mut Ctx::default();
        let mut uart = driver(ctx);
        uart.receive(ctx, Cycles::ZERO, b"1");
        let second_at = Cycles(timed_out_after(1).get() - 1_000);
        uart.receive(ctx, second_at, b"2");
        run_to(ctx, &mut uart, timed_out_after(1));
        assert_eq!(uart.read(ctx, INT_RAW).unwrap() & RXFIFO_TOUT, 0);
        ctx.sleep_until(second_at + timed_out_after(1).get());
        assert_eq!(uart.read(ctx, INT_RAW).unwrap() & RXFIFO_TOUT, RXFIFO_TOUT);
    }

    #[test]
    fn a_byte_that_starts_at_the_instant_the_timeout_comes_finds_it_come() {
        // The line was idle until then. A timeout of 2 bit times after one byte ends on a whole
        // cycle: 444 400 + 2 x 44 440 thirty-seconds, 16 665 cycles.
        let ctx = &mut Ctx::default();
        let mut uart = driver(ctx);
        uart.write(ctx, MEM_CONF, 0x0002_0012);
        uart.receive(ctx, Cycles::ZERO, b"1");
        uart.receive(ctx, Cycles(16_665), b"2");
        run_to(ctx, &mut uart, Cycles(16_665));
        assert_eq!(uart.read(ctx, INT_RAW).unwrap() & RXFIFO_TOUT, RXFIFO_TOUT);
    }

    #[test]
    fn more_bytes_than_the_full_threshold_raise_rxfifo_full_and_a_clear_while_they_stay_does_not_lower_it()
     {
        // UR A1: count > threshold, and a level.
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        uart.write(ctx, CONF1, 0x0000_C004); // full threshold 4
        uart.write(ctx, INT_ENA, RXFIFO_FULL);
        uart.receive(ctx, Cycles::ZERO, b"12345");
        run_to(ctx, &mut uart, landed(4));
        assert_eq!(
            uart.read(ctx, INT_ST),
            Some(0),
            "four bytes: not more than 4"
        );
        assert_eq!(ctx.next_wake(), Some(landed(5)));
        ctx.sleep_until(landed(5));
        assert_eq!(ctx.take_due(), Some(Event::Uart(0)));
        uart.wake(ctx);
        assert_eq!(ctx.sources(), 1 << 21);
        uart.write(ctx, INT_CLR, RXFIFO_FULL);
        assert_eq!(
            uart.read(ctx, INT_ST),
            Some(RXFIFO_FULL),
            "five bytes are still there"
        );
        uart.read(ctx, FIFO);
        assert_eq!(
            uart.read(ctx, INT_ST),
            Some(RXFIFO_FULL),
            "set until cleared"
        );
        uart.write(ctx, INT_CLR, RXFIFO_FULL);
        assert_eq!(
            uart.read(ctx, INT_ST),
            Some(0),
            "four left: cleared for good"
        );
        assert_eq!(ctx.sources(), 0);
    }

    #[test]
    fn a_byte_that_finds_the_fifo_full_raises_rxfifo_ovf_unless_detection_is_off() {
        for (conf1, raised) in [(0x0000_C060, RXFIFO_OVF), (0x0004_C060, 0)] {
            let ctx = &mut Ctx::default();
            let mut uart = console(ctx);
            uart.write(ctx, CONF1, conf1);
            let sent: Vec<u8> = (0..=128).collect();
            uart.receive(ctx, Cycles::ZERO, &sent);
            ctx.sleep_until(landed(128));
            assert_eq!(uart.read(ctx, INT_RAW).unwrap() & RXFIFO_OVF, 0);
            ctx.sleep_until(landed(129));
            assert_eq!(
                uart.read(ctx, INT_RAW).unwrap() & RXFIFO_OVF,
                raised,
                "{conf1:#x}"
            );
            assert_eq!(
                ctx.take_unsupported(),
                Some(Unsupported::UartRxOverflow { uart: 0 }),
                "dropped and counted either way"
            );
        }
    }

    #[test]
    fn the_transmit_fifo_falling_below_its_threshold_raises_txfifo_empty_at_that_instant() {
        // UR §3.3: with no transmit buffer, the driver enables TXFIFO_EMPTY at threshold 10 and
        // waits for it; without it the first long burst blocks for ever (UR A10).
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        uart.write(ctx, CONF1, 10 << TXFIFO_EMPTY_THRHD_SHIFT);
        for byte in 0..20u32 {
            uart.write(ctx, FIFO, byte);
        }
        uart.write(ctx, INT_CLR, TXFIFO_EMPTY);
        uart.write(ctx, INT_ENA, TXFIFO_EMPTY);
        assert_eq!(
            uart.read(ctx, INT_ST),
            Some(0),
            "19 in the FIFO, 1 on the wire"
        );
        // Ten bytes have to leave: the FIFO then holds 9.
        let below = Cycles((10 * BYTE_SUBCYCLES).div_ceil(SUBCYCLES));
        run_to(ctx, &mut uart, Cycles(below.get() - 1));
        assert_eq!(uart.read(ctx, INT_ST), Some(0));
        assert_eq!(ctx.next_wake(), Some(below), "the call at the crossing");
        ctx.sleep_until(below);
        assert_eq!(ctx.take_due(), Some(Event::Uart(0)));
        uart.wake(ctx);
        assert_eq!(ctx.sources(), 1 << 21);
        assert_eq!(uart.read(ctx, INT_ST), Some(TXFIFO_EMPTY));
    }

    #[test]
    fn tx_done_is_raised_when_the_last_byte_has_left_and_not_again_while_idle() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        uart.write(ctx, INT_ENA, TX_DONE);
        uart.write(ctx, FIFO, u32::from(b'k'));
        uart.write(ctx, FIFO, u32::from(b'o'));
        after_bytes(ctx, 1);
        assert_eq!(uart.read(ctx, INT_ST), Some(0));
        after_bytes(ctx, 1);
        assert_eq!(uart.read(ctx, INT_ST), Some(TX_DONE));
        uart.write(ctx, INT_CLR, TX_DONE);
        after_bytes(ctx, 5);
        assert_eq!(uart.read(ctx, INT_ST), Some(0), "an event, not a level");
    }

    #[test]
    fn uart1_drives_source_22_and_a_reset_lowers_it() {
        let ctx = &mut Ctx::default();
        let mut uart1 = Uart::new(1, UART1);
        uart1.write(ctx, INT_ENA, TXFIFO_EMPTY);
        assert_eq!(ctx.sources(), 1 << 22, "UART1_INTR_MAP");
        uart1.reset(ctx);
        assert_eq!(ctx.sources(), 0, "INT_ENA back to 0");
        assert_eq!(uart1.read(ctx, INT_RAW), Some(TXFIFO_EMPTY));
    }

    /// The instant, in cycles, at which the `n`th byte (from 1) the host sends at 0 has landed,
    /// at 115 200 baud 8N1 on the crystal.
    fn landed(n: u64) -> Cycles {
        gone_after(20 * n)
    }

    /// Moves device time to `instant` as the machine would: each call the UART asked for is
    /// delivered at its own instant on the way. A UART that asks for a call at the instant it is
    /// called again and again fails the test instead of hanging it.
    fn run_to(ctx: &mut Ctx, uart: &mut Uart, instant: Cycles) {
        let mut calls = 0;
        while let Some(next) = ctx.next_wake().filter(|&next| next <= instant) {
            ctx.sleep_until(next);
            while ctx.take_due().is_some() {
                calls += 1;
                assert!(
                    calls < 1_000,
                    "the UART asks for a call at {next:?} for ever"
                );
                uart.wake(ctx);
            }
        }
        ctx.sleep_until(instant);
    }

    fn rx_count(ctx: &mut Ctx, uart: &mut Uart) -> u32 {
        uart.read(ctx, STATUS).unwrap() & 0x3FF
    }

    /// What a run shows at each cycle up to `end`: the bytes in the receive FIFO and the level
    /// of the sources, looked at without calling the UART.
    type Timeline = Vec<(usize, u64)>;

    /// A UART on `APB_CLK` (power-on: 20 MHz) receives `a` at 0, with a receive timeout of 10 bit
    /// times. The clocks change at each of `changes`, the way the machine changes them
    /// (`Esp32C3::retune`). Device time moves a cycle at a time and each call the UART asked for
    /// is delivered at its instant; with `taken_every` the console is also taken every so many
    /// cycles, as a front end that cuts the run into calls, or watches the UART, does.
    fn apb_timeline(
        changes: &[(u64, crate::clock::CpuClock)],
        end: u64,
        taken_every: Option<u64>,
    ) -> Timeline {
        let ctx = &mut Ctx::default();
        let mut uart = driver(ctx);
        uart.write(ctx, MEM_CONF, 0x000A_0012);
        select_source(ctx, &mut uart, SCLK_SEL_APB);
        uart.receive(ctx, Cycles::ZERO, b"a");
        let mut timeline = Vec::new();
        for cycle in 1..=end {
            ctx.sleep_until(Cycles(cycle));
            if let Some(&(_, clock)) = changes.iter().find(|&&(at, _)| at == cycle) {
                uart.clock_changing(ctx);
                ctx.set_clock(clock);
                uart.clock_changed(ctx);
            }
            while ctx.take_due().is_some() {
                uart.wake(ctx);
            }
            if taken_every.is_some_and(|every| cycle % every == 0) {
                uart.take_tx(ctx);
            }
            timeline.push((uart.rx.len(), ctx.sources()));
        }
        timeline
    }

    #[test]
    fn a_byte_and_a_timeout_on_the_apb_clock_come_at_the_same_instants_whenever_the_uart_is_called()
    {
        // The APB clock goes from 20 to 40 MHz while `a` is on the line (its start bit at 0, its
        // frame at 20 MHz: 27 775 cycles), then to 80 MHz while the timeout counts: 10 bit times
        // at 80 MHz after the landing (34 718.75) is after the second change, at 33 000, and
        // before the instant worked out at 40 MHz (41 662.5).
        let changes = [
            (10_000, crate::clock::CpuClock::new(4, 4)),
            (33_000, crate::clock::CpuClock::new(2, 2)),
        ];
        let end = 45_000;
        let called_only_when_asked = apb_timeline(&changes, end, None);
        let landed = called_only_when_asked
            .iter()
            .position(|&(received, _)| received == 1)
            .map(|index| index + 1);
        assert_eq!(
            landed,
            Some(27_775),
            "timed with the clock of its start bit"
        );
        let raised = called_only_when_asked
            .iter()
            .position(|&(_, sources)| sources != 0)
            .map(|index| index + 1);
        assert_eq!(
            raised,
            Some(34_719),
            "the timeout at the rate of now, at its instant"
        );
        for every in [1, 7, 997] {
            assert!(
                apb_timeline(&changes, end, Some(every)) == called_only_when_asked,
                "the console taken every {every} cycles"
            );
        }
    }

    #[test]
    fn a_byte_of_the_host_lands_one_frame_after_it_starts_and_is_read_once() {
        // UR §5.4: one frame after the start bit (A7); `STATUS.RXFIFO_CNT`, then a read of `FIFO`.
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        uart.receive(ctx, Cycles::ZERO, b"a");
        ctx.sleep_until(Cycles(landed(1).get() - 1));
        assert_eq!(rx_count(ctx, &mut uart), 0, "the stop bit is not over");
        ctx.sleep_until(landed(1));
        assert_eq!(rx_count(ctx, &mut uart), 1);
        assert_eq!(
            uart.read(ctx, STATUS),
            Some(0xE000_C001),
            "the line bits stay"
        );
        assert_eq!(uart.read(ctx, FIFO), Some(u32::from(b'a')));
        assert_eq!(rx_count(ctx, &mut uart), 0);
        assert_eq!(ctx.take_unsupported(), None);
    }

    #[test]
    fn the_bytes_of_an_input_follow_each_other_and_those_sent_before() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        uart.receive(ctx, Cycles::ZERO, b"ab");
        uart.receive(ctx, Cycles::ZERO, b"c");
        ctx.sleep_until(landed(2));
        assert_eq!(rx_count(ctx, &mut uart), 2);
        ctx.sleep_until(landed(3));
        assert_eq!(rx_count(ctx, &mut uart), 3, "c waits for the line");
        // An input dated later starts at its date: the line was idle.
        let later = Cycles(landed(3).get() + 1_000_000);
        uart.receive(ctx, later, b"d");
        ctx.sleep_until(Cycles(later.get() + landed(1).get() - 1));
        assert_eq!(rx_count(ctx, &mut uart), 3);
        ctx.sleep_until(Cycles(later.get() + landed(1).get()));
        assert_eq!(rx_count(ctx, &mut uart), 4);
        let read: Vec<u32> = (0..4).map(|_| uart.read(ctx, FIFO).unwrap()).collect();
        assert_eq!(read, [97, 98, 99, 100]);
    }

    #[test]
    fn an_input_dated_no_later_than_the_one_before_joins_its_run() {
        // A text sent at once costs a byte of memory per byte; a run is kept per later date.
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        uart.receive(ctx, Cycles(1_000), b"ab");
        uart.receive(ctx, Cycles(1_000), b"cd");
        uart.receive(ctx, Cycles(500), b"e");
        uart.receive(ctx, Cycles::ZERO, b"");
        assert_eq!(uart.host_runs.len(), 1);
        uart.receive(ctx, Cycles(1_001), b"f");
        assert_eq!(uart.host_runs.len(), 2);
        assert_eq!(uart.host_pending(), 6);
        // `e`, dated 500, follows `d`; `f`, dated 1 001, waits for the line: back to back from
        // 1 000.
        run_to(ctx, &mut uart, Cycles(1_000 + landed(5).get()));
        assert_eq!(rx_count(ctx, &mut uart), 5);
        run_to(ctx, &mut uart, Cycles(1_000 + landed(6).get() - 1));
        assert_eq!(rx_count(ctx, &mut uart), 5);
        run_to(ctx, &mut uart, Cycles(1_000 + landed(6).get()));
        assert_eq!(rx_count(ctx, &mut uart), 6);
        assert_eq!(uart.host_runs.len(), 0);
        let read: Vec<u32> = (0..6).map(|_| uart.read(ctx, FIFO).unwrap()).collect();
        assert_eq!(read, b"abcdef".map(u32::from));
    }

    #[test]
    fn the_uart_asks_to_be_called_at_the_instant_each_byte_lands() {
        // A CPU that waits for an interrupt must see the byte arrive at its instant.
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        uart.receive(ctx, Cycles::ZERO, b"hi");
        assert_eq!(ctx.next_wake(), Some(landed(1)));
        ctx.sleep_until(landed(1));
        assert_eq!(ctx.take_due(), Some(Event::Uart(0)));
        uart.wake(ctx);
        assert_eq!(ctx.next_wake(), Some(landed(2)));
        ctx.sleep_until(landed(2));
        assert_eq!(ctx.take_due(), Some(Event::Uart(0)));
        uart.wake(ctx);
        assert_eq!(ctx.next_wake(), None, "nothing more to land");
        assert_eq!(uart.host_pending(), 0);
        assert_eq!(rx_count(ctx, &mut uart), 2);
    }

    #[test]
    fn a_byte_that_finds_the_fifo_full_is_dropped_and_reported() {
        // UR A2: the new byte is lost, the 128 stay.
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        let sent: Vec<u8> = (0..=128).collect();
        uart.receive(ctx, Cycles::ZERO, &sent);
        ctx.sleep_until(landed(128));
        assert_eq!(rx_count(ctx, &mut uart), 128);
        assert_eq!(ctx.take_unsupported(), None);
        ctx.sleep_until(landed(129));
        assert_eq!(rx_count(ctx, &mut uart), 128);
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::UartRxOverflow { uart: 0 })
        );
        let read: Vec<u32> = (0..128).map(|_| uart.read(ctx, FIFO).unwrap()).collect();
        assert_eq!(read, (0..128).collect::<Vec<u32>>());
    }

    #[test]
    fn a_byte_that_lands_while_the_receiver_is_off_or_its_fifo_in_reset_is_lost() {
        // UR A8: `CLK_CONF.RX_SCLK_EN` clear, or `CONF0.RXFIFO_RST` held.
        for (register, value) in [
            (CLK_CONF, CLK_CONF_XTAL & !RX_SCLK_EN),
            (CONF0, 0x0400_001C | RXFIFO_RST),
        ] {
            let ctx = &mut Ctx::default();
            let mut uart = console(ctx);
            uart.write(ctx, register, value);
            uart.receive(ctx, Cycles::ZERO, b"x");
            ctx.sleep_until(landed(1));
            assert_eq!(rx_count(ctx, &mut uart), 0, "{register:#x}");
            assert_eq!(uart.host_pending(), 0, "the byte arrived, and was lost");
        }
    }

    #[test]
    fn a_receiver_whose_clock_is_off_counts_no_timeout() {
        let receiver_off = CLK_CONF_XTAL & !RX_SCLK_EN;
        // Three bytes wait in the FIFO; the clock goes off before their timeout comes.
        let ctx = &mut Ctx::default();
        let mut uart = driver(ctx);
        uart.receive(ctx, Cycles::ZERO, b"123");
        run_to(ctx, &mut uart, landed(3));
        uart.write(ctx, CLK_CONF, receiver_off);
        run_to(ctx, &mut uart, timed_out_after(3));
        assert_eq!(ctx.sources(), 0, "a timeout that comes with the clock off");
        // The clock is off when a fourth byte lands, and on again right after: the lost byte
        // starts no idle period, so no timeout comes 100 bit times after it.
        let ctx = &mut Ctx::default();
        let mut uart = driver(ctx);
        uart.receive(ctx, Cycles::ZERO, b"123");
        run_to(ctx, &mut uart, timed_out_after(3));
        assert_eq!(ctx.sources(), 1 << 21, "the timeout of the three bytes");
        uart.write(ctx, INT_CLR, RXFIFO_TOUT);
        uart.write(ctx, CLK_CONF, receiver_off);
        let fourth = ctx.now();
        uart.receive(ctx, fourth, b"4");
        run_to(ctx, &mut uart, fourth + landed(1).get());
        uart.write(ctx, CLK_CONF, CLK_CONF_XTAL);
        run_to(ctx, &mut uart, fourth + timed_out_after(1).get());
        assert_eq!(ctx.sources(), 0, "no idle period after a byte nobody heard");
        assert_eq!(rx_count(ctx, &mut uart), 3);
    }

    #[test]
    fn the_receive_fifo_reset_empties_the_fifo() {
        // `uart_param_config` pulses it (UR §3.3).
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        uart.receive(ctx, Cycles::ZERO, b"xyz");
        ctx.sleep_until(landed(2));
        uart.write(ctx, CONF0, 0x0400_001C | RXFIFO_RST);
        uart.write(ctx, CONF0, 0x0400_001C);
        assert_eq!(rx_count(ctx, &mut uart), 0);
        ctx.sleep_until(landed(3));
        assert_eq!(uart.read(ctx, FIFO), Some(u32::from(b'z')));
    }

    #[test]
    fn a_read_of_an_empty_receive_fifo_returns_0_and_is_refused_by_name() {
        // UR A6.
        let ctx = &mut Ctx::default();
        let mut uart = Uart::new(1, UART1);
        assert_eq!(uart.read(ctx, FIFO), Some(0));
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::UartEmptyRead { uart: 1 })
        );
    }

    #[test]
    fn a_received_byte_keeps_the_frame_set_when_it_started() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        uart.write(ctx, CONF0, 0x0400_003E); // 8E2: 12 bit times
        uart.receive(ctx, Cycles::ZERO, b"pq");
        ctx.sleep_until(Cycles(1));
        uart.write(ctx, CONF0, 0x0400_001C); // 8N1 from now on
        ctx.sleep_until(Cycles(gone_after(24).get() - 1));
        assert_eq!(rx_count(ctx, &mut uart), 0);
        ctx.sleep_until(gone_after(24));
        assert_eq!(rx_count(ctx, &mut uart), 1, "p, 8E2");
        ctx.sleep_until(gone_after(24 + 20));
        assert_eq!(
            rx_count(ctx, &mut uart),
            2,
            "q started after the change: 8N1"
        );
    }

    #[test]
    fn a_reset_empties_the_receive_fifo_and_the_host_goes_on_sending() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        uart.receive(ctx, Cycles::ZERO, b"012");
        ctx.sleep_until(Cycles(landed(2).get() - 1));
        assert_eq!(rx_count(ctx, &mut uart), 1);
        uart.reset(ctx);
        assert_eq!(rx_count(ctx, &mut uart), 0);
        ctx.sleep_until(landed(2));
        assert_eq!(rx_count(ctx, &mut uart), 1, "the byte on the line lands");
        // The next byte starts after the reset, at the reset values: `CLKDIV` 694 on the crystal
        // divided by 2 (`SCLK_DIV_NUM` 1).
        let third = (2 * 20 * (347 * 16 + 3) * 4 + 20 * (694 * 16) * 2 * 4_u64).div_ceil(SUBCYCLES);
        ctx.sleep_until(Cycles(third - 1));
        assert_eq!(rx_count(ctx, &mut uart), 1);
        ctx.sleep_until(Cycles(third));
        assert_eq!(rx_count(ctx, &mut uart), 2);
        assert_eq!(uart.read(ctx, FIFO), Some(u32::from(b'1')));
    }

    #[test]
    fn the_registers_of_the_driver_are_storage_and_rs485_or_another_fifo_size_are_refused() {
        // UR §3.3: the driver writes IDLE_CONF and RS485_CONF, reads AT_CMD_CHAR in its handler.
        let ctx = &mut Ctx::default();
        let mut uart = Uart::new(0, UART0);
        for offset in [IDLE_CONF, RS485_CONF, AT_CMD_CHAR, MEM_CONF] {
            let reset = svd_regs::esp32c3::lookup(UART0 + offset)
                .unwrap()
                .reset_value;
            assert_eq!(uart.read(ctx, offset), Some(reset), "{offset:#x}");
        }
        assert!(uart.write(ctx, IDLE_CONF, 0));
        assert!(
            uart.write(ctx, MEM_CONF, 0x0064_0012),
            "a timeout of 100, one block each"
        );
        assert!(uart.write(ctx, RS485_CONF, 0));
        assert_eq!(ctx.take_unsupported(), None);
        assert_eq!(uart.read(ctx, MEM_CONF), Some(0x0064_0012));
        uart.write(ctx, RS485_CONF, 1);
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::UartFeature {
                uart: 0,
                feature: UartFeature::Rs485
            })
        );
        uart.write(ctx, MEM_CONF, 0x000A_0014);
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::UartFeature {
                uart: 0,
                feature: UartFeature::FifoSize
            })
        );
        assert_eq!(uart.read(ctx, MEM_RX_STATUS), Some(0x0008_0100));
        assert!(!uart.write(ctx, MEM_RX_STATUS, 0), "read-only");
        assert_eq!(uart.read(ctx, FSM_STATUS), Some(0), "ST_URX_OUT stays 0");
    }

    #[test]
    fn the_second_uart_has_its_own_reset_values_and_its_own_name() {
        let ctx = &mut Ctx::default();
        // 0xE000_C000 is the SVD reset value of UART0.STATUS and UART1.STATUS; the FIFO counts are
        // zero in it already. This pins the SVD lookup behind `Uart::new`.
        let mut uart1 = Uart::new(1, UART1);
        assert_eq!(uart1.read(ctx, STATUS), Some(0xE000_C000));
        assert_eq!(uart1.read(ctx, CLKDIV), Some(0x2B6));
        for byte in 0..=129u32 {
            uart1.write(ctx, FIFO, byte);
        }
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::UartOverflow { uart: 1 })
        );
    }

    #[test]
    fn the_transmitter_asks_to_be_called_when_its_last_byte_has_left() {
        // Without this a CPU in `wfi` with bytes on their way out would never see them leave.
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        assert_eq!(ctx.next_wake(), None);
        uart.write(ctx, FIFO, u32::from(b'o'));
        uart.write(ctx, FIFO, u32::from(b'k'));
        let first_gone = Cycles(BYTE_SUBCYCLES.div_ceil(SUBCYCLES));
        let last_gone = Cycles((2 * BYTE_SUBCYCLES).div_ceil(SUBCYCLES));
        assert_eq!(
            ctx.next_wake(),
            Some(first_gone),
            "the earliest call is kept"
        );
        ctx.sleep_until(first_gone);
        assert_eq!(ctx.take_due(), Some(Event::Uart(0)));
        uart.wake(ctx);
        assert_eq!(ctx.next_wake(), Some(last_gone));
        ctx.sleep_until(last_gone);
        assert_eq!(ctx.take_due(), Some(Event::Uart(0)));
        uart.wake(ctx);
        assert_eq!(ctx.next_wake(), None, "idle: nothing more to wait for");
        assert_eq!(uart.take_tx(ctx), b"ok");
    }

    #[test]
    fn the_end_of_a_run_takes_the_bytes_that_have_not_left_too() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        for byte in *b"bye" {
            uart.write(ctx, FIFO, u32::from(byte));
        }
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx), b"b");
        assert_eq!(uart.take_written(), b"ye", "in order, each once");
        assert_eq!(uart.take_written(), b"");
        assert_eq!(
            uart.read(ctx, FSM_STATUS),
            Some(0),
            "the transmission ended"
        );
    }

    #[test]
    fn the_console_is_the_same_whenever_it_is_taken() {
        // Taking after every byte time, or once in the middle and then flushing at the end of the
        // run, gives the same bytes in the same order.
        let text = *b"hello";
        let collect = |takes: &dyn Fn(&mut Ctx, &mut Uart, &mut Vec<u8>)| {
            let ctx = &mut Ctx::default();
            let mut uart = console(ctx);
            for byte in text {
                uart.write(ctx, FIFO, u32::from(byte));
            }
            let mut console_bytes = Vec::new();
            takes(ctx, &mut uart, &mut console_bytes);
            console_bytes.extend(uart.take_written());
            console_bytes
        };
        let every_byte = collect(&|ctx, uart, out| {
            for _ in text {
                after_bytes(ctx, 1);
                out.extend(uart.take_tx(ctx));
            }
        });
        let once_midway = collect(&|ctx, uart, out| {
            after_bytes(ctx, 2);
            out.extend(uart.take_tx(ctx));
        });
        assert_eq!(every_byte, text);
        assert_eq!(once_midway, text);
    }

    #[test]
    fn an_integer_divider_of_zero_with_a_fraction_divides_by_the_fraction() {
        // `CLKDIV` = 0 + 3/16 on the crystal: a bit lasts 3/16 of 4 time-base cycles, a byte ten
        // of them, 7.5 cycles. It has not left after 7 cycles and has after 8.
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        uart.write(ctx, CLKDIV, 3 << CLKDIV_FRAG_SHIFT);
        uart.write(ctx, FIFO, u32::from(b'f'));
        assert_eq!(ctx.next_wake(), Some(Cycles(8)), "7.5 cycles, rounded up");
        ctx.sleep_until(Cycles(7));
        assert_eq!(uart.take_tx(ctx), []);
        assert_eq!(uart.read(ctx, FSM_STATUS), Some(ST_UTX_OUT_BUSY));
        ctx.sleep_until(Cycles(8));
        assert_eq!(uart.take_tx(ctx), b"f");
        assert_eq!(uart.read(ctx, FSM_STATUS), Some(0));
    }

    #[test]
    fn a_divider_of_zero_sends_the_byte_at_once() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        uart.write(ctx, CLKDIV, 0);
        uart.write(ctx, FIFO, u32::from(b'z'));
        assert_eq!(uart.take_tx(ctx), b"z");
        assert_eq!(uart.read(ctx, FSM_STATUS), Some(0));
    }

    fn select_source(ctx: &mut Ctx, uart: &mut Uart, selection: u32) {
        uart.write(
            ctx,
            CLK_CONF,
            (CLK_CONF_XTAL & !(SCLK_SEL_MASK << SCLK_SEL_SHIFT)) | (selection << SCLK_SEL_SHIFT),
        );
    }

    #[test]
    fn a_byte_sent_on_a_source_clock_the_model_does_not_have_is_refused_by_name() {
        // `SCLK_SEL` 2 is the RC fast clock, 17.5 MHz, and 0 selects no documented source: the
        // model has neither. The selection alone is not refused (the register is only stored);
        // each byte sent with it is, and the byte is timed with the crystal.
        for selection in [0, 2] {
            let ctx = &mut Ctx::default();
            let mut uart = console(ctx);
            select_source(ctx, &mut uart, selection);
            assert_eq!(ctx.take_unsupported(), None, "selecting is not sending");
            uart.write(ctx, FIFO, u32::from(b'r'));
            assert_eq!(
                ctx.take_unsupported(),
                Some(Unsupported::UartClock {
                    uart: 0,
                    sclk_sel: selection
                })
            );
            assert_eq!(uart.take_tx(ctx), [], "SCLK_SEL {selection}");
            after_bytes(ctx, 1);
            assert_eq!(uart.take_tx(ctx), b"r", "SCLK_SEL {selection}");
            uart.write(ctx, FIFO, u32::from(b's'));
            assert!(ctx.take_unsupported().is_some(), "every byte is reported");
        }
    }

    #[test]
    fn what_the_receiver_shows_on_a_source_clock_the_model_does_not_have_is_refused_when_the_firmware_reads_it()
     {
        // The byte and the timeout are timed with the crystal. The refusal comes at the read that
        // shows them, an instant the firmware fixes, not when a call reaches the start bit.
        for (selection, byte_read, timeout_read) in [(0, STATUS, INT_ST), (2, FIFO, INT_RAW)] {
            let ctx = &mut Ctx::default();
            let mut uart = driver(ctx);
            select_source(ctx, &mut uart, selection);
            uart.receive(ctx, Cycles::ZERO, b"a");
            run_to(ctx, &mut uart, Cycles(landed(1).get() - 1));
            uart.take_tx(ctx);
            assert_eq!(
                ctx.take_unsupported(),
                None,
                "SCLK_SEL {selection}: not read yet"
            );
            assert_eq!(uart.read(ctx, CONF0), Some(0x1000_001C));
            assert_eq!(ctx.take_unsupported(), None, "not a read of the receiver");
            run_to(ctx, &mut uart, landed(1));
            assert_ne!(
                uart.read(ctx, byte_read),
                Some(0),
                "landed one crystal frame later"
            );
            let refused = Some(Unsupported::UartReceiveClock {
                uart: 0,
                sclk_sel: selection,
            });
            assert_eq!(ctx.take_unsupported(), refused);
            uart.read(ctx, timeout_read);
            assert_eq!(ctx.take_unsupported(), None, "seen already");
            run_to(ctx, &mut uart, timed_out_after(1));
            assert_eq!(
                ctx.take_unsupported(),
                None,
                "the timeout came, not read yet"
            );
            uart.read(ctx, timeout_read);
            assert_eq!(ctx.take_unsupported(), refused, "the timeout read");
        }
    }

    #[test]
    fn a_byte_sent_on_the_crystal_or_on_the_apb_clock_is_not_refused() {
        for selection in [SCLK_SEL_APB, SCLK_SEL_XTAL] {
            let ctx = &mut Ctx::new(true);
            let mut uart = console(ctx);
            select_source(ctx, &mut uart, selection);
            uart.write(ctx, FIFO, u32::from(b'k'));
            assert_eq!(ctx.take_unsupported(), None, "SCLK_SEL {selection}");
            assert_eq!(ctx.diag.take_violation(), None);
        }
    }

    #[test]
    fn a_strict_run_stops_on_a_byte_sent_on_the_rc_fast_clock() {
        let ctx = &mut Ctx::new(true);
        ctx.set_pc(0x4200_0010);
        let mut uart = Uart::new(1, UART1);
        select_source(ctx, &mut uart, 2);
        assert_eq!(ctx.diag.take_violation(), None);
        uart.write(ctx, FIFO, u32::from(b'x'));
        assert_eq!(
            ctx.diag.take_violation(),
            Some(crate::diag::Diagnostic::Unsupported {
                what: Unsupported::UartClock {
                    uart: 1,
                    sclk_sel: 2
                },
                pc: 0x4200_0010
            })
        );
    }

    #[test]
    fn a_byte_keeps_the_duration_it_was_given_when_the_clock_changes_later() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        uart.write(ctx, FIFO, u32::from(b'a'));
        // The crystal divided by 2: only the bytes written from now on are slower.
        uart.write(ctx, CLK_CONF, CLK_CONF_XTAL | (1 << SCLK_DIV_NUM_SHIFT));
        uart.write(ctx, FIFO, u32::from(b'b'));
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx), b"a", "a was timed at 115 200 baud");
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx), [], "b takes two byte times");
        after_bytes(ctx, 2);
        assert_eq!(uart.take_tx(ctx), b"b");
    }

    /// The instant, in cycles, at which the first byte written now to an idle transmitter has
    /// left, for a frame of `half_bits` half bit times at 115 200 baud on the crystal.
    fn gone_after(half_bits: u64) -> Cycles {
        Cycles((half_bits * (347 * 16 + 3) * 4).div_ceil(SUBCYCLES))
    }

    #[test]
    fn a_byte_lasts_its_start_bit_its_data_bits_its_parity_bit_and_its_stop_bits() {
        // UR §5.1, §6: start + (5 + BIT_NUM) + PARITY_EN + stop (1, 1.5 or 2 for STOP_BIT_NUM 1,
        // 2, 3). The ROM's `0x0400001C` and the reset value are 8N1: 10 bit times.
        for (conf0, half_bits, format) in [
            (0x0400_001C, 20, "8N1"),
            (0x1000_001C, 20, "8N1, the reset value"),
            (0x0400_001E, 22, "8E1"),
            (0x0400_001F, 22, "8O1"),
            (0x0400_003C, 22, "8N2"),
            (0x0400_003E, 24, "8E2"),
            (0x0400_0010, 14, "5N1"),
            (0x0400_001A, 20, "7E1"),
            (0x0400_002C, 21, "8N1.5"),
        ] {
            let ctx = &mut Ctx::default();
            let mut uart = console(ctx);
            assert!(uart.write(ctx, CONF0, conf0));
            uart.write(ctx, FIFO, u32::from(b'f'));
            assert_eq!(ctx.next_wake(), Some(gone_after(half_bits)), "{format}");
            assert_eq!(ctx.take_unsupported(), None, "{format}");
        }
    }

    #[test]
    fn half_a_stop_bit_is_kept_over_a_line_of_bytes() {
        // 8N1.5 at 115 200 baud: 10.5 bit times a byte; 128 bytes end at 128 x 10.5 bit times,
        // not at 128 x 10 or 128 x 11.
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        uart.write(ctx, CONF0, 0x0400_002C);
        for byte in 0..128u32 {
            uart.write(ctx, FIFO, byte);
        }
        ctx.sleep_until(gone_after(128 * 21));
        assert_eq!(uart.take_tx(ctx).len(), 128);
        assert_eq!(
            ctx.next_wake(),
            Some(gone_after(21)),
            "the first wake-up is kept"
        );
    }

    #[test]
    fn a_change_of_frame_format_applies_to_the_bytes_written_after_it() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        uart.write(ctx, FIFO, u32::from(b'a'));
        uart.write(ctx, CONF0, 0x0400_003E);
        uart.write(ctx, FIFO, u32::from(b'b'));
        ctx.sleep_until(gone_after(20));
        assert_eq!(uart.take_tx(ctx), b"a", "8N1, as written");
        ctx.sleep_until(gone_after(20 + 23));
        assert_eq!(uart.take_tx(ctx), []);
        ctx.sleep_until(gone_after(20 + 24));
        assert_eq!(uart.take_tx(ctx), b"b", "8E2: 12 bit times");
    }

    #[test]
    fn loopback_an_inverted_receive_line_autobaud_and_no_stop_bit_are_refused_by_name() {
        // UR §10: refused when set; the value is stored all the same.
        for (bits, feature) in [
            (1 << 14, UartFeature::Loopback),
            (1 << 19, UartFeature::InvertedRx),
            (1 << 27, UartFeature::Autobaud),
        ] {
            let ctx = &mut Ctx::default();
            let mut uart = Uart::new(1, UART1);
            assert!(uart.write(ctx, CONF0, 0x0400_001C | bits));
            assert_eq!(
                ctx.take_unsupported(),
                Some(Unsupported::UartFeature { uart: 1, feature })
            );
            assert_eq!(uart.read(ctx, CONF0), Some(0x0400_001C | bits));
            assert!(uart.write(ctx, CONF0, 0x0400_001C));
            assert_eq!(ctx.take_unsupported(), None, "cleared: nothing to refuse");
        }
    }

    #[test]
    fn flow_control_an_inverted_transmit_line_irda_and_a_break_are_refused_by_name() {
        // They would change what the host sees or when: refused when set, stored all the same.
        for (bits, feature) in [
            (1 << 8, UartFeature::TxBreak),
            (1 << 15, UartFeature::TxFlowControl),
            (1 << 16, UartFeature::Irda),
            (1 << 22, UartFeature::InvertedTx),
        ] {
            let ctx = &mut Ctx::default();
            let mut uart = Uart::new(0, UART0);
            assert!(uart.write(ctx, CONF0, 0x0400_001C | bits));
            assert_eq!(
                ctx.take_unsupported(),
                Some(Unsupported::UartFeature { uart: 0, feature })
            );
            assert_eq!(uart.read(ctx, CONF0), Some(0x0400_001C | bits));
            assert!(uart.write(ctx, CONF0, 0x0400_001C));
            assert_eq!(ctx.take_unsupported(), None, "cleared: nothing to refuse");
        }
        let ctx = &mut Ctx::default();
        let mut uart = Uart::new(1, UART1);
        assert!(uart.write(ctx, CONF1, 0x0020_C078 | RX_FLOW_EN));
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::UartFeature {
                uart: 1,
                feature: UartFeature::RxFlowControl
            })
        );
        assert_eq!(uart.read(ctx, CONF1), Some(0x0020_C078 | RX_FLOW_EN));
        assert!(
            uart.write(ctx, CONF1, 0x0020_C078),
            "what the driver writes"
        );
        assert_eq!(ctx.take_unsupported(), None);
    }

    #[test]
    fn a_frame_without_a_stop_bit_is_refused_and_timed_with_one() {
        // UR A4: STOP_BIT_NUM = 0 has no documented frame.
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        uart.write(ctx, CONF0, 0x0400_000C);
        assert_eq!(
            ctx.take_unsupported(),
            Some(Unsupported::UartFeature {
                uart: 0,
                feature: UartFeature::NoStopBit
            })
        );
        uart.write(ctx, FIFO, u32::from(b's'));
        assert_eq!(ctx.next_wake(), Some(gone_after(20)));
    }

    #[test]
    fn the_wake_up_of_bytes_dropped_by_a_fifo_reset_finds_nothing_to_do() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        for byte in *b"abc" {
            uart.write(ctx, FIFO, u32::from(byte));
        }
        // The call is now armed for the last byte, which the reset drops.
        let first_gone = Cycles(BYTE_SUBCYCLES.div_ceil(SUBCYCLES));
        let last_gone = Cycles((3 * BYTE_SUBCYCLES).div_ceil(SUBCYCLES));
        ctx.sleep_until(first_gone);
        assert_eq!(ctx.take_due(), Some(Event::Uart(0)));
        uart.wake(ctx);
        assert_eq!(ctx.next_wake(), Some(last_gone));
        // The pulse of `Uart_Init`, with an 8N1 frame around it.
        uart.write(ctx, CONF0, 0x0400_001C | TXFIFO_RST);
        uart.write(ctx, CONF0, 0x0400_001C);
        ctx.sleep_until(last_gone);
        assert_eq!(ctx.take_due(), Some(Event::Uart(0)));
        uart.wake(ctx);
        assert_eq!(ctx.next_wake(), None, "nothing is left to wait for");
        assert_eq!(uart.take_tx(ctx), b"ab", "c was dropped");
        assert_eq!(uart.read(ctx, FSM_STATUS), Some(0));
        assert_eq!(ctx.take_unsupported(), None);
    }
}
