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
//! device time when they are accessed (ADR 0003 §4), and the UART asks to be called at the
//! instant each byte lands. The UART interrupts arrive with the next part of this plan.
use crate::clock::XTAL_PERIOD;
use crate::ctx::{Ctx, Deadline, Event};
use crate::diag::{UartFeature, Unsupported};
use crate::periph::{Accepted, Storage};
use emu_core::{Cycles, Peripheral};
use std::collections::VecDeque;

/// `INT_CLR` is write-only and accepted; the two others are stored.
pub(crate) fn accepted() -> Vec<Accepted> {
    let mut rows = Vec::new();
    for peripheral in ["UART0", "UART1"] {
        for (register, not_modeled) in [
            ("INT_ENA", "interrupt enables: nothing is raised"),
            ("INT_CLR", "interrupt clear: nothing is pending"),
            ("CONF1", "FIFO thresholds"),
        ] {
            rows.push(Accepted::svd(peripheral, register, not_modeled, "SP1b-3d"));
        }
    }
    rows
}

/// The model serves UART0 and UART1, whose register offsets are identical: those of UART0 name them.
const fn reg(name: &str) -> u32 {
    svd_regs::esp32c3::addr("UART0", name) - svd_regs::esp32c3::addr("UART0", "FIFO")
}

const FIFO: u32 = reg("FIFO");
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
/// The modes of `CONF0` the model refuses by name when they are set (report UR §10).
const CONF0_REFUSED: [(u32, UartFeature); 3] = [
    (1 << 14, UartFeature::Loopback),
    (1 << 19, UartFeature::InvertedRx),
    (1 << 27, UartFeature::Autobaud),
];
/// Device time is kept in thirty-seconds of a time-base cycle here: a bit lasts a whole number of
/// sixteenths (the fraction of `CLKDIV`), and a frame may end on half a bit (1.5 stop bits).
const SUBCYCLES: u64 = 32;

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
    /// The bytes the host sends and that have not started: each with the device time, in
    /// thirty-seconds, before which it does not start, the date of the input it came in.
    host: VecDeque<(u64, u8)>,
    /// The byte on the line: the instant its frame ends, when it lands, and the byte.
    receiving: Option<(u64, u8)>,
    /// When the line is free for the next byte: the end of the last frame that started.
    line_free_at: u64,
    /// The receive FIFO.
    rx: VecDeque<u8>,
    /// The call at the next instant the device has something to do: a byte of its transmitter
    /// leaves (so that a CPU that waits for an interrupt with bytes on their way out sleeps
    /// until they have left, not for ever), or a byte of the host lands.
    wake: Deadline,
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
            receiving: None,
            line_free_at: 0,
            rx: VecDeque::new(),
            wake: Deadline::default(),
        }
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
        self.arm(ctx);
    }

    /// The bytes that have left the transmitter by now, since the last call.
    pub fn take_tx(&mut self, ctx: &mut Ctx) -> Vec<u8> {
        self.settle(ctx);
        std::mem::take(&mut self.tx)
    }

    /// The host sends `bytes`, the first starting at device time `at` (now or later), the others
    /// right after it, and after the bytes it sent before. The caller bounds how many it hands
    /// over ([`Uart::host_pending`]).
    ///
    /// ASSUMPTION: the host sends at the baud rate and in the frame the UART is set to, as a
    /// terminal set to match it does; a host at another rate would give framing errors, which the
    /// model does not produce. Every console measured is 8N1 at 115 200 baud (report UR §6).
    pub fn receive(&mut self, ctx: &mut Ctx, at: Cycles, bytes: &[u8]) {
        self.settle(ctx);
        let date = at.max(ctx.now()).get().saturating_mul(SUBCYCLES);
        self.host.extend(bytes.iter().map(|&byte| (date, byte)));
        self.arm(ctx);
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

    /// Asks to be called when the last byte of the transmitter will have left, and when the next
    /// byte of the host lands.
    fn arm(&mut self, ctx: &mut Ctx) {
        let next_landing = self.next_landing(ctx);
        let last_gone = self.in_flight.back().map(|&(gone_at, _)| gone_at);
        for instant in [last_gone, next_landing].into_iter().flatten() {
            self.wake.arm(
                ctx,
                Cycles(instant.div_ceil(SUBCYCLES)),
                Event::Uart(self.index),
            );
        }
    }

    /// The instant the next byte of the host lands: the one on the line, or the next to start,
    /// whose frame is timed with the settings of now (they hold until a register write, which
    /// settles first).
    fn next_landing(&self, ctx: &mut Ctx) -> Option<u64> {
        if let Some((lands_at, _)) = self.receiving {
            return Some(lands_at);
        }
        let &(date, _) = self.host.front()?;
        Some(
            date.max(self.line_free_at)
                .saturating_add(self.byte_time(self.source_period(ctx))),
        )
    }

    /// Brings the device to now: the bytes whose last bit has gone move to the console, the bytes
    /// of the host whose frame has ended land, in the order of their instants.
    fn settle(&mut self, ctx: &mut Ctx) {
        let now = ctx.now().get().saturating_mul(SUBCYCLES);
        while let Some(&(gone_at, byte)) = self.in_flight.front() {
            if gone_at > now {
                break;
            }
            self.tx.push(byte);
            self.in_flight.pop_front();
        }
        // INVARIANT: each pass lands a byte or ends the loop, and the bytes are those the caller
        // handed over within its bound (`host_pending`).
        loop {
            if self.receiving.is_none() {
                let Some(&(date, byte)) = self.host.front() else {
                    break;
                };
                let starts_at = date.max(self.line_free_at);
                if starts_at > now {
                    break;
                }
                // The frame of a byte is the one the UART is set to when it starts: a register
                // write settles before it changes the setting.
                let lands_at = starts_at.saturating_add(self.byte_time(self.source_period(ctx)));
                self.host.pop_front();
                self.receiving = Some((lands_at, byte));
                self.line_free_at = lands_at;
            }
            match self.receiving {
                Some((lands_at, byte)) if lands_at <= now => {
                    self.receiving = None;
                    self.land(ctx, byte);
                }
                _ => break,
            }
        }
    }

    /// A byte of the host has been received whole: it enters the FIFO.
    ///
    /// ASSUMPTION: a byte that lands while the receiver's clock is off (`CLK_CONF.RX_SCLK_EN`
    /// clear) or while `CONF0.RXFIFO_RST` is set is lost (report UR A8), and a byte that finds
    /// 128 bytes in the FIFO is dropped while the 128 stay (report UR A2). Input sent to a chip
    /// during its boot, and 200 bytes sent to a FIFO nobody reads, settle them.
    fn land(&mut self, ctx: &mut Ctx, byte: u8) {
        let clk_conf = self.config.read(CLK_CONF).unwrap_or(0);
        let conf0 = self.config.read(CONF0).unwrap_or(0);
        if clk_conf & RX_SCLK_EN == 0 || conf0 & RXFIFO_RST != 0 {
            return;
        }
        if self.rx.len() >= FIFO_BYTES {
            ctx.unsupported(Unsupported::UartRxOverflow { uart: self.index });
            return;
        }
        self.rx.push_back(byte);
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

    /// The period of the source clock a byte sent now is timed with, in time-base cycles.
    ///
    /// `SCLK_SEL` 1 is `APB_CLK` and 3 the crystal. 2 selects the RC fast clock
    /// (`case UART_SCLK_RTC: hw->clk_conf.sclk_sel = 2;`,
    /// `components/hal/esp32c3/include/hal/uart_ll.h:178-179`, ESP-IDF v5.5.2), 17.5 MHz nominal,
    /// and 0 has no documented source (`uart_reg.h:1189` documents only the reset value, 3). The
    /// model has a clock for neither: a byte sent with one of them is refused by name
    /// ([`Unsupported::UartClock`], a stop under `--strict-mmio`) and timed with the crystal so
    /// that a lenient run goes on, instead of being given a duration that is silently wrong (the
    /// calibration of the timer groups measures the same oscillator at its 17.5 MHz). Nothing on
    /// the measured paths selects either.
    fn source_period(&self, ctx: &mut Ctx) -> u64 {
        let clk_conf = self.config.read(CLK_CONF).unwrap_or(0);
        match (clk_conf >> SCLK_SEL_SHIFT) & SCLK_SEL_MASK {
            SCLK_SEL_APB => ctx.clock().apb_period(),
            SCLK_SEL_XTAL => XTAL_PERIOD,
            sclk_sel => {
                ctx.unsupported(Unsupported::UartClock {
                    uart: self.index,
                    sclk_sel,
                });
                XTAL_PERIOD
            }
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
        let clkdiv = self.config.read(CLKDIV).unwrap_or(0);
        let clk_conf = self.config.read(CLK_CONF).unwrap_or(0);
        let conf0 = self.config.read(CONF0).unwrap_or(0);
        // The divider in sixteenths of a source-clock cycle, a frame in half bits: the product
        // is in thirty-seconds.
        let divider = u64::from(clkdiv & CLKDIV_INTEGER_MASK) * 16
            + u64::from((clkdiv >> CLKDIV_FRAG_SHIFT) & CLKDIV_FRAG_MASK);
        let source_divider = u64::from((clk_conf >> SCLK_DIV_NUM_SHIFT) & SCLK_DIV_NUM_MASK) + 1;
        frame_half_bits(conf0) * divider * source_divider * source_period
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
            // No interrupt condition is modelled yet, so there is nothing to clear.
            INT_CLR => true,
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
        // The baud rate or the frame may have changed: so has the instant the next byte lands.
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
    fn interrupt_clear_is_accepted_and_the_interrupt_state_is_unmodeled() {
        let ctx = &mut Ctx::default();
        let mut uart = Uart::new(0, UART0);
        assert!(uart.write(ctx, INT_CLR, 0xFFFF_FFFF));
        assert_eq!(
            uart.read(ctx, reg("INT_RAW")),
            None,
            "interrupts are not modelled yet"
        );
        assert_eq!(uart.read(ctx, reg("INT_ST")), None);
    }

    /// The instant, in cycles, at which the `n`th byte (from 1) the host sends at 0 has landed,
    /// at 115 200 baud 8N1 on the crystal.
    fn landed(n: u64) -> Cycles {
        gone_after(20 * n)
    }

    fn rx_count(ctx: &mut Ctx, uart: &mut Uart) -> u32 {
        uart.read(ctx, STATUS).unwrap() & 0x3FF
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
