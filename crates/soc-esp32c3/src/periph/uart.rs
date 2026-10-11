//! UART controller, transmit side. Register layout: `uart_reg.h` (reports BC §G.2, BA §7).
//!
//! A byte written to the FIFO leaves at the baud rate: the transmit FIFO fills, `STATUS` and
//! `FSM_STATUS` say so, and the byte reaches the console when its last bit has gone. Software waits
//! on both: the ROM before each character, ESP-IDF before it changes a clock or restarts (report IR
//! §4.1). Reception and the UART interrupts arrive in SP1b-3b.
use crate::clock::XTAL_PERIOD;
use crate::ctx::{Ctx, Deadline, Event};
use crate::diag::Unsupported;
use crate::periph::{Accepted, Storage};
use emu_core::{Cycles, Peripheral};
use std::collections::VecDeque;

/// `INT_CLR` is write-only and accepted; the three others are stored.
pub(crate) fn accepted() -> Vec<Accepted> {
    let mut rows = Vec::new();
    for peripheral in ["UART0", "UART1"] {
        for (register, not_modeled) in [
            ("INT_ENA", "interrupt enables: nothing is raised"),
            ("INT_CLR", "interrupt clear: nothing is pending"),
            (
                "CONF0",
                "frame format: a byte is 10 bit times (the transmit FIFO reset acts)",
            ),
            ("CONF1", "FIFO thresholds"),
        ] {
            rows.push(Accepted::svd(peripheral, register, not_modeled, "SP1b-3b"));
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
/// What software writes and reads back. `CLKDIV` and `CLK_CONF` set the baud rate.
const CONFIG: [u32; 5] = [INT_ENA, CLKDIV, CONF0, CONF1, CLK_CONF];
/// `STATUS` bits [9:0] `RXFIFO_CNT` and [25:16] `TXFIFO_CNT`.
const FIFO_COUNTS: u32 = 0x03FF_03FF;
const TXFIFO_CNT_SHIFT: u32 = 16;
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
/// `CONF0` bit 18 `TXFIFO_RST`: empties the transmit FIFO (`ROM3:Uart_Init@0x4004c306` sets it,
/// then clears it; BC §G.2).
const TXFIFO_RST: u32 = 1 << 18;
/// ASSUMPTION: a byte is 10 bit times on the wire (a start bit, 8 data bits, a stop bit), whatever
/// `CONF0` says. The ROM and ESP-IDF consoles use this format; another one would only change how
/// long a byte takes. SP2 gives the frame format its effect, with the receive side.
const BITS_PER_BYTE: u64 = 10;
/// Device time is kept in sixteenths of a time-base cycle here: a bit lasts a whole number of them.
const SUBCYCLES: u64 = 16;

#[derive(Clone, Debug)]
pub struct Uart {
    /// 0 for UART0, 1 for UART1: named in diagnostics.
    index: u8,
    base: u32,
    /// `STATUS` with both FIFO counts at zero: the line-state bits keep their reset value.
    idle_status: u32,
    config: Storage,
    /// The bytes on their way out, each with the time, in sixteenths of a time-base cycle, at
    /// which its last bit has gone. The first is in the shift register, the others in the FIFO.
    in_flight: VecDeque<(u64, u8)>,
    /// The bytes that have left, for the console.
    tx: Vec<u8>,
    /// The call at the time the transmitter goes idle: a CPU that waits for an interrupt with
    /// bytes still on their way out sleeps until they have left, not for ever.
    wake: Deadline,
}

impl Uart {
    #[must_use]
    pub fn new(index: u8, base: u32) -> Uart {
        let status_reset = svd_regs::esp32c3::lookup(base + STATUS).map_or(0, |r| r.reset_value);
        Uart {
            index,
            base,
            idle_status: status_reset & !FIFO_COUNTS,
            config: Storage::for_offsets(base, &CONFIG),
            in_flight: VecDeque::new(),
            tx: Vec::new(),
            wake: Deadline::default(),
        }
    }

    /// The module is reset (`SYSTEM.PERIP_RST_EN0`, or a core reset): its registers return to their
    /// reset values and the bytes that have not left are lost. Those that left stay for the console.
    pub fn reset(&mut self, now: Cycles) {
        self.settle(now);
        self.config = Storage::for_offsets(self.base, &CONFIG);
        self.in_flight.clear();
    }

    /// The bytes that have left the transmitter by `now`, since the last call.
    pub fn take_tx(&mut self, now: Cycles) -> Vec<u8> {
        self.settle(now);
        std::mem::take(&mut self.tx)
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

    /// The device time of a wake-up has come: the bytes that have gone leave, and the device asks
    /// to be called again when the last one will have.
    pub(crate) fn wake(&mut self, ctx: &mut Ctx) {
        let now = ctx.now();
        self.wake.called(now);
        self.settle(now);
        if let Some(&(gone_at, _)) = self.in_flight.back() {
            self.wake.arm(
                ctx,
                Cycles(gone_at.div_ceil(SUBCYCLES)),
                Event::Uart(self.index),
            );
        }
    }

    /// Moves the bytes whose last bit has gone to the console.
    fn settle(&mut self, now: Cycles) {
        let now = now.get().saturating_mul(SUBCYCLES);
        while let Some(&(gone_at, byte)) = self.in_flight.front() {
            if gone_at > now {
                break;
            }
            self.tx.push(byte);
            self.in_flight.pop_front();
        }
    }

    /// The time one byte takes, in sixteenths of a time-base cycle: 10 bits, each `CLKDIV` (with
    /// its fraction) cycles of the source clock divided by `SCLK_DIV_NUM + 1`.
    ///
    /// Known simplification: a source selection other than `APB_CLK` is clocked by the crystal. 3
    /// is the crystal. 2 selects the RC fast clock (`case UART_SCLK_RTC: hw->clk_conf.sclk_sel =
    /// 2;`, `components/hal/esp32c3/include/hal/uart_ll.h:178-179`, ESP-IDF v5.5.2; `uart_reg.h:1189`
    /// documents only the reset value, 3); the RC fast clock is modelled at the crystal frequency,
    /// and nothing on the measured paths selects it. The fractional divider of the source clock
    /// (`SCLK_DIV_A/B`) is ignored: it resets to 0 and no measured code writes it.
    fn byte_time(&self, ctx: &Ctx) -> u64 {
        let clkdiv = self.config.read(CLKDIV).unwrap_or(0);
        let clk_conf = self.config.read(CLK_CONF).unwrap_or(0);
        let divider = u64::from(clkdiv & CLKDIV_INTEGER_MASK) * SUBCYCLES
            + u64::from((clkdiv >> CLKDIV_FRAG_SHIFT) & CLKDIV_FRAG_MASK);
        let source_period = if (clk_conf >> SCLK_SEL_SHIFT) & SCLK_SEL_MASK == SCLK_SEL_APB {
            ctx.clock().apb_period
        } else {
            XTAL_PERIOD
        };
        let source_divider = u64::from((clk_conf >> SCLK_DIV_NUM_SHIFT) & SCLK_DIV_NUM_MASK) + 1;
        BITS_PER_BYTE * divider * source_divider * source_period
    }

    fn send(&mut self, ctx: &mut Ctx, byte: u8) {
        // One byte is in the shift register, the FIFO holds the others.
        if self.in_flight.len() > FIFO_BYTES {
            ctx.unsupported(Unsupported::UartOverflow { uart: self.index });
            return;
        }
        // A byte keeps the duration it was given when it was written: a later change of `CLKDIV`,
        // `CLK_CONF` or the APB clock applies to the bytes written after it. This is the model's
        // choice; firmware that changes the clock with bytes on their way out is not modelled.
        let now = ctx.now().get().saturating_mul(SUBCYCLES);
        let starts_at = self.in_flight.back().map_or(now, |&(gone_at, _)| gone_at);
        let gone_at = starts_at.max(now).saturating_add(self.byte_time(ctx));
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
        self.settle(ctx.now());
        match offset {
            STATUS => {
                let in_fifo = u32::try_from(self.in_flight.len().saturating_sub(1)).unwrap_or(0);
                Some(self.idle_status | (in_fifo << TXFIFO_CNT_SHIFT))
            }
            FSM_STATUS => Some(if self.in_flight.is_empty() {
                0
            } else {
                ST_UTX_OUT_BUSY
            }),
            _ => self.config.read(offset),
        }
    }

    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        self.settle(ctx.now());
        match offset {
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
                self.config.write(offset, value)
            }
            _ => self.config.write(offset, value),
        }
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
    /// One byte at 115 200 baud: 10 x (347 x 16 + 3) x 4 sixteenths, 13 887.5 cycles, 86.8 us.
    const BYTE_SUBCYCLES: u64 = 10 * (347 * 16 + 3) * 4;

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
        assert_eq!(uart.take_tx(ctx.now()), [], "nothing has left yet");
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx.now()), b"E");
        assert_eq!(uart.take_tx(ctx.now()), [], "taken once");
        after_bytes(ctx, 2);
        assert_eq!(uart.take_tx(ctx.now()), b"SP");
    }

    #[test]
    fn only_the_low_byte_of_a_fifo_write_is_transmitted() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        uart.write(ctx, FIFO, 0xFFFF_FF41);
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx.now()), b"A");
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
        assert_eq!(uart.take_tx(ctx.now()), b"ab");
    }

    #[test]
    fn the_baud_rate_follows_the_divider_and_the_source_clock() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        // The crystal divided by 2: a byte takes twice as long.
        uart.write(ctx, CLK_CONF, CLK_CONF_XTAL | (1 << SCLK_DIV_NUM_SHIFT));
        uart.write(ctx, FIFO, u32::from(b'x'));
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx.now()), []);
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx.now()), b"x");
        // APB_CLK as the source, 20 MHz after power-on: twice as long as with the crystal.
        uart.write(ctx, CLK_CONF, CLK_CONF_XTAL & !(0b10 << SCLK_SEL_SHIFT));
        uart.write(ctx, FIFO, u32::from(b'y'));
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx.now()), []);
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx.now()), b"y");
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
        let sent = uart.take_tx(ctx.now());
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
        uart.reset(ctx.now());
        after_bytes(ctx, 20);
        assert_eq!(uart.take_tx(ctx.now()), b"gone");
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
        assert_eq!(uart.take_tx(ctx.now()), b"a");
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
    fn interrupt_clear_is_accepted_and_the_rest_is_unmodeled() {
        let ctx = &mut Ctx::default();
        let mut uart = Uart::new(0, UART0);
        assert!(uart.write(ctx, INT_CLR, 0xFFFF_FFFF));
        assert_eq!(
            uart.read(ctx, reg("INT_RAW")),
            None,
            "interrupts are SP1b-3b"
        );
        assert_eq!(uart.read(ctx, reg("INT_ST")), None);
        assert_eq!(
            uart.read(ctx, FIFO),
            None,
            "FIFO is write-only: RX is not modelled"
        );
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
        assert_eq!(uart.take_tx(ctx.now()), b"ok");
    }

    #[test]
    fn the_end_of_a_run_takes_the_bytes_that_have_not_left_too() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        for byte in *b"bye" {
            uart.write(ctx, FIFO, u32::from(byte));
        }
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx.now()), b"b");
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
                out.extend(uart.take_tx(ctx.now()));
            }
        });
        let once_midway = collect(&|ctx, uart, out| {
            after_bytes(ctx, 2);
            out.extend(uart.take_tx(ctx.now()));
        });
        assert_eq!(every_byte, text);
        assert_eq!(once_midway, text);
    }

    #[test]
    fn a_divider_of_zero_sends_the_byte_at_once() {
        let ctx = &mut Ctx::default();
        let mut uart = console(ctx);
        uart.write(ctx, CLKDIV, 0);
        uart.write(ctx, FIFO, u32::from(b'z'));
        assert_eq!(uart.take_tx(ctx.now()), b"z");
        assert_eq!(uart.read(ctx, FSM_STATUS), Some(0));
    }

    #[test]
    fn the_rc_fast_selections_are_timed_from_the_crystal() {
        // A known simplification: values 0 and 2 of `SCLK_SEL` run at the crystal frequency.
        for selection in [0, 2] {
            let ctx = &mut Ctx::default();
            let mut uart = console(ctx);
            uart.write(
                ctx,
                CLK_CONF,
                (CLK_CONF_XTAL & !(SCLK_SEL_MASK << SCLK_SEL_SHIFT))
                    | (selection << SCLK_SEL_SHIFT),
            );
            uart.write(ctx, FIFO, u32::from(b'r'));
            assert_eq!(uart.take_tx(ctx.now()), [], "SCLK_SEL {selection}");
            after_bytes(ctx, 1);
            assert_eq!(uart.take_tx(ctx.now()), b"r", "SCLK_SEL {selection}");
        }
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
        assert_eq!(uart.take_tx(ctx.now()), b"a", "a was timed at 115 200 baud");
        after_bytes(ctx, 1);
        assert_eq!(uart.take_tx(ctx.now()), [], "b takes two byte times");
        after_bytes(ctx, 2);
        assert_eq!(uart.take_tx(ctx.now()), b"b");
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
        uart.write(ctx, CONF0, TXFIFO_RST);
        uart.write(ctx, CONF0, 0);
        ctx.sleep_until(last_gone);
        assert_eq!(ctx.take_due(), Some(Event::Uart(0)));
        uart.wake(ctx);
        assert_eq!(ctx.next_wake(), None, "nothing is left to wait for");
        assert_eq!(uart.take_tx(ctx.now()), b"ab", "c was dropped");
        assert_eq!(uart.read(ctx, FSM_STATUS), Some(0));
        assert_eq!(ctx.take_unsupported(), None);
    }
}
