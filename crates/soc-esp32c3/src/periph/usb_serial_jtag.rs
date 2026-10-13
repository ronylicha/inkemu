//! USB-Serial-JTAG, the serial endpoint in both directions. Register layout:
//! `usb_serial_jtag_reg.h` and `usb_serial_jtag_struct.h` (report USJ §A).
//!
//! The firmware writes the bytes for the host into the IN FIFO of `EP1` and commits them as a
//! packet with `EP1_CONF.WR_DONE`, or by filling the 64 bytes of a packet. The host's bytes come
//! as OUT packets of at most 64 bytes, one in the FIFO at a time, read from `EP1` (report USJ
//! §R.1, §R.5). The interrupt is source 26 (`USB_INTR_MAP`), level, driven as
//! `INT_RAW & INT_ENA != 0` after every change (report USJ §C).
//!
//! The model behaves like a host attached and reading: a committed packet is taken at once.
use crate::clock::SECOND;
use crate::ctx::{Ctx, Deadline, Event, Source};
use crate::diag::Unsupported;
use crate::periph::Storage;
use emu_core::{Cycles, Peripheral};
use std::collections::VecDeque;

/// The offset of a register of the block.
const fn reg(name: &str) -> u32 {
    svd_regs::esp32c3::addr("USB_DEVICE", name) - svd_regs::esp32c3::addr("USB_DEVICE", "EP1")
}

const EP1: u32 = reg("EP1");
const EP1_CONF: u32 = reg("EP1_CONF");
const INT_RAW: u32 = reg("INT_RAW");
const INT_ST: u32 = reg("INT_ST");
const INT_ENA: u32 = reg("INT_ENA");
const INT_CLR: u32 = reg("INT_CLR");
/// Written by the driver and by Arduino's `HWCDC` (`CONF0`), or by nobody on the paths read
/// (report USJ §A): stored, from their reset values, with no effect.
const STORED: [u32; 5] = [
    reg("CONF0"),
    reg("TEST"),
    reg("MISC_CONF"),
    reg("MEM_CONF"),
    reg("DATE"),
];
/// The state of the JTAG FIFO and of the endpoints, and the frame number: read-only, served at
/// their reset values.
///
/// ASSUMPTION: nobody reads them (report USJ §A: no consumer on the paths read touches them; the
/// LL header has no accessor for `FRAM_NUM`), so the model does not derive them from its FIFOs.
/// A firmware that reads them, or a register dump of a chip with a packet in each FIFO,
/// settles what they hold.
const AT_RESET: [u32; 9] = [
    reg("JFIFO_ST"),
    reg("FRAM_NUM"),
    reg("IN_EP0_ST"),
    reg("IN_EP1_ST"),
    reg("IN_EP2_ST"),
    reg("IN_EP3_ST"),
    reg("OUT_EP0_ST"),
    reg("OUT_EP1_ST"),
    reg("OUT_EP2_ST"),
];
/// `EP1_CONF` bit 0, write 1: commit the bytes written so far as one IN packet, none included (a
/// zero-length packet). It reads 0. A write to `EP1_CONF` acts on this bit only: bits 1 and 2 are
/// read-only (`usb_serial_jtag_struct.h:24-26`).
const WR_DONE: u32 = 1 << 0;
/// `EP1_CONF` bit 1, read-only: the IN endpoint takes a byte (no committed packet waits for the
/// host, and the FIFO holds fewer than 64 bytes).
const SERIAL_IN_EP_DATA_FREE: u32 = 1 << 1;
/// `EP1_CONF` bit 2, read-only: the OUT FIFO holds a byte of the host.
const SERIAL_OUT_EP_DATA_AVAIL: u32 = 1 << 2;
/// The bits of `INT_RAW` the model sets (`usb_serial_jtag_reg.h:554-660`, report USJ §R.1):
/// `SERIAL_OUT_RECV_PKT` (a packet of the host is in the OUT FIFO), `SERIAL_IN_EMPTY` (the host
/// took an IN packet; 1 at reset), `OUT_EP1_ZERO_PAYLOAD` (the host sent a zero-length packet).
const SERIAL_OUT_RECV_PKT: u32 = 1 << 2;
const SERIAL_IN_EMPTY: u32 = 1 << 3;
const OUT_EP1_ZERO_PAYLOAD: u32 = 1 << 10;
/// `INT_ENA` and `INT_CLR` take the 19 bits of `USB_SERIAL_JTAG_LL_INTR_MASK` (`ll.h:19`).
const INTERRUPT_BITS: u32 = 0x7FFFF;
/// Size of a full-speed bulk packet: the hardware sends a full FIFO without waiting for `WR_DONE`.
/// Source: ESP-IDF v5.5.2 `components/hal/esp32c3/include/hal/usb_serial_jtag_ll.h`, note on
/// `usb_serial_jtag_ll_txfifo_flush`: "When fifo is full (with 64 byte), HW will flush the buffer
/// automatically".
pub const PACKET_BYTES: usize = 64;
/// The interrupt source of the block (report USJ §C).
const SOURCE: Source = Source::of("USB_INTR_MAP");
/// The device time the host takes to move a packet: an OUT packet of the host reaches the FIFO
/// this long after it is sent, or after the previous one has been read whole.
///
/// ASSUMPTION: 50 us, about one full-speed bulk transaction of 64 bytes with its token and its
/// handshake (report USJ §D.2, §O.5). No source gives a figure; a board with a host writing to
/// it settles it.
pub const HOST_LATENCY: u64 = 50 * SECOND / 1_000_000;

#[derive(Debug)]
pub struct UsbSerialJtag {
    stored: Storage,
    /// The bytes of the IN packet being filled.
    in_fifo: Vec<u8>,
    /// The bytes the host took, for the console.
    tx: Vec<u8>,
    /// The packets the host sends and that have not reached the FIFO, each with the device
    /// time, in cycles, before which it is not sent; a zero-length packet is empty.
    host: VecDeque<(u64, Vec<u8>)>,
    /// The packet of the host in the OUT FIFO, what the firmware has not read of it.
    out_fifo: VecDeque<u8>,
    /// When the OUT FIFO was last left empty: the host sends its next packet from then on.
    out_free_since: u64,
    int_raw: u32,
    int_ena: u32,
    /// The call at the instant the next packet of the host reaches the FIFO.
    wake: Deadline,
}

impl Default for UsbSerialJtag {
    fn default() -> UsbSerialJtag {
        UsbSerialJtag::new()
    }
}

impl UsbSerialJtag {
    #[must_use]
    pub fn new() -> UsbSerialJtag {
        let base = svd_regs::esp32c3::addr("USB_DEVICE", "EP1");
        UsbSerialJtag {
            stored: Storage::for_offsets(base, &STORED),
            in_fifo: Vec::new(),
            tx: Vec::new(),
            host: VecDeque::new(),
            out_fifo: VecDeque::new(),
            out_free_since: 0,
            // The SVD's reset value: `SERIAL_IN_EMPTY`.
            int_raw: SERIAL_IN_EMPTY,
            int_ena: 0,
            wake: Deadline::default(),
        }
    }

    /// A core reset returns the block to its reset values and empties both FIFOs; what the host
    /// took stays for the console. The host is no part of the chip: the packets it is sending go
    /// on arriving.
    ///
    /// ASSUMPTION: a core reset resets the block, as it resets "the whole digital system"
    /// (TRM §6.1.3, report IR §4.2); a CPU reset (`esp_restart` on the ESP32-C3) keeps it, as
    /// report IR §4.2 lists for the other peripherals, and does not reach this function. The
    /// reset table of the TRM, or a chip whose USB-Serial-JTAG is read after a watchdog reset,
    /// settles it.
    pub(crate) fn reset(&mut self, ctx: &mut Ctx) {
        self.settle(ctx);
        let host = std::mem::take(&mut self.host);
        let tx = std::mem::take(&mut self.tx);
        *self = UsbSerialJtag {
            host,
            tx,
            out_free_since: ctx.now().get(),
            ..UsbSerialJtag::new()
        };
        self.drive(ctx);
        self.arm(ctx);
    }

    /// The bytes the host took since the last call.
    pub fn take_tx(&mut self, ctx: &mut Ctx) -> Vec<u8> {
        self.settle(ctx);
        std::mem::take(&mut self.tx)
    }

    /// Every byte the firmware wrote and no call has taken, the packet being filled included:
    /// for the end of a run, so that the console shows what the firmware had written when it
    /// stopped.
    pub fn take_written(&mut self) -> Vec<u8> {
        let mut written = std::mem::take(&mut self.tx);
        written.append(&mut self.in_fifo);
        written
    }

    /// The host sends `bytes` from device time `at` (now or later), after what it sent before:
    /// packets of at most 64 bytes; no byte at all is one zero-length packet. The caller bounds
    /// how many it hands over ([`UsbSerialJtag::host_pending`]).
    pub fn receive(&mut self, ctx: &mut Ctx, at: Cycles, bytes: &[u8]) {
        self.settle(ctx);
        let date = at.max(ctx.now()).get();
        if bytes.is_empty() {
            self.host.push_back((date, Vec::new()));
        }
        for packet in bytes.chunks(PACKET_BYTES) {
            self.host.push_back((date, packet.to_vec()));
        }
        self.arm(ctx);
    }

    /// How many bytes the host has sent that the firmware has not read, its packets counted as
    /// one byte each so that zero-length ones count too.
    #[must_use]
    pub fn host_pending(&self) -> usize {
        self.out_fifo.len()
            + self
                .host
                .iter()
                .map(|(_, packet)| packet.len().max(1))
                .sum::<usize>()
    }

    /// The device time of a wake-up has come.
    pub(crate) fn wake(&mut self, ctx: &mut Ctx) {
        self.wake.called(ctx.now());
        self.settle(ctx);
        self.arm(ctx);
    }

    /// The instant the next packet of the host reaches the FIFO: once the FIFO is empty, the
    /// host latency after the packet is sent or the FIFO was left empty, whichever is later.
    fn next_packet_at(&self) -> Option<u64> {
        if !self.out_fifo.is_empty() {
            return None;
        }
        let &(date, _) = self.host.front()?;
        Some(date.max(self.out_free_since).saturating_add(HOST_LATENCY))
    }

    fn arm(&mut self, ctx: &mut Ctx) {
        if let Some(at) = self.next_packet_at() {
            self.wake.arm(ctx, Cycles(at), Event::UsbSerialJtag);
        }
    }

    /// Brings the block to now: the packets of the host that have arrived enter the FIFO, one at
    /// a time; the interrupt source follows.
    fn settle(&mut self, ctx: &mut Ctx) {
        let now = ctx.now().get();
        // INVARIANT: each pass takes one packet of those the caller handed over within its bound.
        while let Some(at) = self.next_packet_at().filter(|&at| at <= now) {
            let Some((_, packet)) = self.host.pop_front() else {
                break;
            };
            if packet.is_empty() {
                // A zero-length packet leaves the FIFO empty: the next one can follow.
                self.int_raw |= OUT_EP1_ZERO_PAYLOAD;
                self.out_free_since = at;
            } else {
                self.out_fifo.extend(packet);
                self.int_raw |= SERIAL_OUT_RECV_PKT;
            }
        }
        self.drive(ctx);
    }

    /// The bytes written so far leave as one packet: the host takes it at once, and
    /// `SERIAL_IN_EMPTY` says so.
    fn commit(&mut self) {
        self.tx.append(&mut self.in_fifo);
        self.int_raw |= SERIAL_IN_EMPTY;
    }

    /// A read of `EP1`: the oldest byte of the host.
    ///
    /// ASSUMPTION: a read of an empty OUT FIFO returns 0 and changes nothing (report USJ §R.1,
    /// O.6); it is refused by name. Every consumer read looks at `SERIAL_OUT_EP_DATA_AVAIL`
    /// first; the TRM settles it.
    fn pop(&mut self, ctx: &mut Ctx) -> u32 {
        let Some(byte) = self.out_fifo.pop_front() else {
            ctx.unsupported(Unsupported::UsbEmptyRead);
            return 0;
        };
        if self.out_fifo.is_empty() {
            self.out_free_since = ctx.now().get();
        }
        u32::from(byte)
    }

    /// A write of `EP1`: a byte for the host. The FIFO never holds 64 bytes: the 64th commits.
    fn push(&mut self, byte: u8) {
        self.in_fifo.push(byte);
        if self.in_fifo.len() == PACKET_BYTES {
            self.commit();
        }
    }

    fn ep1_conf(&self) -> u32 {
        let free = if self.in_fifo.len() < PACKET_BYTES {
            SERIAL_IN_EP_DATA_FREE
        } else {
            0
        };
        let avail = if self.out_fifo.is_empty() {
            0
        } else {
            SERIAL_OUT_EP_DATA_AVAIL
        };
        free | avail
    }

    fn int_status(&self) -> u32 {
        self.int_raw & self.int_ena
    }

    fn drive(&self, ctx: &mut Ctx) {
        ctx.irq(SOURCE, self.int_status() != 0);
    }
}

impl Peripheral<Ctx> for UsbSerialJtag {
    fn read(&mut self, ctx: &mut Ctx, offset: u32) -> Option<u32> {
        self.settle(ctx);
        let value = match offset {
            EP1 => self.pop(ctx),
            EP1_CONF => self.ep1_conf(),
            INT_RAW => self.int_raw,
            INT_ST => self.int_status(),
            INT_ENA => self.int_ena,
            _ if AT_RESET.contains(&offset) => {
                svd_regs::esp32c3::lookup(svd_regs::esp32c3::addr("USB_DEVICE", "EP1") + offset)
                    .map_or(0, |register| register.reset_value)
            }
            _ => return self.stored.read(offset),
        };
        self.drive(ctx);
        self.arm(ctx);
        Some(value)
    }

    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        self.settle(ctx);
        let accepted = match offset {
            EP1 => {
                self.push(value.to_le_bytes()[0]);
                true
            }
            EP1_CONF => {
                if value & WR_DONE != 0 {
                    self.commit();
                }
                true
            }
            INT_ENA => {
                self.int_ena = value & INTERRUPT_BITS;
                true
            }
            INT_CLR => {
                self.int_raw &= !value;
                true
            }
            _ => self.stored.write(offset, value),
        };
        self.drive(ctx);
        accepted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    const USB_SOURCE: u64 = 1 << 26;

    fn send(ctx: &mut Ctx, usb: &mut UsbSerialJtag, text: &[u8]) {
        for &byte in text {
            assert!(usb.write(ctx, EP1, u32::from(byte)));
        }
    }

    /// The bytes the firmware reads from `EP1` while `SERIAL_OUT_EP_DATA_AVAIL` says there are
    /// some, as the driver's handler does (report USJ §B.4).
    fn read_packet(ctx: &mut Ctx, usb: &mut UsbSerialJtag) -> Vec<u8> {
        let mut bytes = Vec::new();
        while usb.read(ctx, EP1_CONF).unwrap() & SERIAL_OUT_EP_DATA_AVAIL != 0 {
            bytes.push(u8::try_from(usb.read(ctx, EP1).unwrap()).unwrap());
        }
        bytes
    }

    /// Moves device time to `instant`, delivering each call the block asked for on the way.
    fn run_to(ctx: &mut Ctx, usb: &mut UsbSerialJtag, instant: Cycles) {
        let mut calls = 0;
        while let Some(next) = ctx.next_wake().filter(|&next| next <= instant) {
            ctx.sleep_until(next);
            while ctx.take_due().is_some() {
                calls += 1;
                assert!(
                    calls < 1_000,
                    "the block asks for a call at {next:?} for ever"
                );
                usb.wake(ctx);
            }
        }
        ctx.sleep_until(instant);
    }

    #[test]
    fn the_registers_start_at_their_svd_reset_values() {
        // Report USJ §A.
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new();
        for (name, reset) in [
            ("EP1_CONF", 0x2),
            ("INT_RAW", 0x8),
            ("INT_ST", 0),
            ("INT_ENA", 0),
            ("CONF0", 0x4200),
            ("TEST", 0),
            ("JFIFO_ST", 0x44),
            ("FRAM_NUM", 0),
            ("IN_EP0_ST", 1),
            ("IN_EP3_ST", 1),
            ("OUT_EP1_ST", 0),
            ("MISC_CONF", 0),
            ("MEM_CONF", 0x2),
            ("DATE", 0x0200_7300),
        ] {
            assert_eq!(usb.read(ctx, reg(name)), Some(reset), "{name}");
        }
        assert!(usb.write(ctx, reg("CONF0"), 0x4204));
        assert_eq!(usb.read(ctx, reg("CONF0")), Some(0x4204), "stored");
        assert!(!usb.write(ctx, reg("JFIFO_ST"), 0), "read-only");
        assert!(!usb.write(ctx, reg("IN_EP1_ST"), 0), "read-only");
        assert_eq!(usb.read(ctx, 0x7C), None, "no register there");
        assert_eq!(ctx.sources(), 0);
    }

    #[test]
    fn bytes_reach_the_host_when_the_packet_is_committed() {
        // The ROM: read EP1_CONF bit 1, write the byte, WR_DONE after `\n` (report USJ §B.1).
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new();
        usb.write(ctx, INT_CLR, SERIAL_IN_EMPTY);
        for &byte in b"boot\n" {
            assert_eq!(usb.read(ctx, EP1_CONF).unwrap() & SERIAL_IN_EP_DATA_FREE, 2);
            usb.write(ctx, EP1, u32::from(byte));
        }
        assert_eq!(usb.take_tx(ctx), [], "nothing is sent before WR_DONE");
        assert!(usb.write(ctx, EP1_CONF, WR_DONE));
        assert_eq!(usb.take_tx(ctx), b"boot\n");
        assert_eq!(usb.take_tx(ctx), []);
        assert_eq!(
            usb.read(ctx, INT_RAW),
            Some(SERIAL_IN_EMPTY),
            "the host took the packet"
        );
        assert_eq!(
            usb.read(ctx, EP1_CONF),
            Some(SERIAL_IN_EP_DATA_FREE),
            "WR_DONE reads 0"
        );
    }

    #[test]
    fn a_full_packet_is_sent_without_waiting_for_a_commit() {
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new();
        send(ctx, &mut usb, &[b'x'; 64]);
        assert_eq!(usb.take_tx(ctx).len(), 64);
        send(ctx, &mut usb, b"y");
        assert_eq!(usb.take_tx(ctx), []);
        assert_eq!(usb.take_written(), b"y", "the end of a run takes the rest");
    }

    #[test]
    fn a_commit_of_nothing_is_a_zero_length_packet_the_host_takes() {
        // The driver ends a transfer of 64 bytes with one (report USJ §B.4).
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new();
        usb.write(ctx, INT_CLR, SERIAL_IN_EMPTY);
        assert!(usb.write(ctx, EP1_CONF, WR_DONE));
        assert_eq!(usb.take_tx(ctx), []);
        assert_eq!(usb.read(ctx, INT_RAW), Some(SERIAL_IN_EMPTY));
    }

    #[test]
    fn hwcdc_begin_takes_its_first_interrupt_from_the_reset_value_of_serial_in_empty() {
        // Report USJ §B.5: `begin` clears the 19 enables, sets bits 2, 3 and 9, and never writes
        // INT_CLR: the reset value of SERIAL_IN_EMPTY runs the handler at once.
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new();
        usb.write(ctx, reg("CONF0"), 0x4200);
        let ena = usb.read(ctx, INT_ENA).unwrap();
        usb.write(ctx, INT_ENA, ena & !INTERRUPT_BITS);
        let ena = usb.read(ctx, INT_ENA).unwrap();
        usb.write(
            ctx,
            INT_ENA,
            ena | SERIAL_OUT_RECV_PKT | SERIAL_IN_EMPTY | (1 << 9),
        );
        assert_eq!(usb.read(ctx, INT_ST), Some(SERIAL_IN_EMPTY));
        assert_eq!(ctx.sources(), USB_SOURCE, "source 26, USB_INTR_MAP");
        usb.write(ctx, INT_CLR, SERIAL_IN_EMPTY);
        assert_eq!(ctx.sources(), 0);
        assert_eq!(usb.read(ctx, INT_ENA), Some(0x20C));
        usb.write(ctx, INT_ENA, 0xFFFF_FFFF);
        assert_eq!(usb.read(ctx, INT_ENA), Some(0x7FFFF), "19 bits stored");
    }

    #[test]
    fn a_packet_of_the_host_reaches_the_fifo_after_the_host_latency_and_raises_its_interrupt() {
        // The driver at install enables SERIAL_OUT_RECV_PKT only; its handler clears it, then
        // reads while SERIAL_OUT_EP_DATA_AVAIL (report USJ §B.4, §F.1).
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new();
        usb.write(ctx, INT_ENA, SERIAL_OUT_RECV_PKT);
        usb.receive(ctx, Cycles(1_000), b"hi, espressif\n");
        assert_eq!(ctx.next_wake(), Some(Cycles(1_000 + HOST_LATENCY)));
        run_to(ctx, &mut usb, Cycles(1_000 + HOST_LATENCY - 1));
        assert_eq!(
            usb.read(ctx, EP1_CONF).unwrap() & SERIAL_OUT_EP_DATA_AVAIL,
            0
        );
        assert_eq!(ctx.sources(), 0);
        run_to(ctx, &mut usb, Cycles(1_000 + HOST_LATENCY));
        assert_eq!(ctx.sources(), USB_SOURCE, "raised at its instant");
        assert_eq!(usb.read(ctx, INT_ST), Some(SERIAL_OUT_RECV_PKT));
        usb.write(ctx, INT_CLR, SERIAL_OUT_RECV_PKT);
        assert_eq!(read_packet(ctx, &mut usb), b"hi, espressif\n");
        assert_eq!(ctx.sources(), 0);
        assert_eq!(usb.host_pending(), 0);
        assert_eq!(ctx.take_unsupported(), None);
    }

    #[test]
    fn a_long_input_arrives_in_packets_of_64_each_after_the_previous_one_was_read() {
        // Report USJ §F.1: a 90-byte input is read as 64 + 26.
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new();
        let input: Vec<u8> = (0..90).collect();
        usb.receive(ctx, Cycles::ZERO, &input);
        assert_eq!(usb.host_pending(), 90);
        run_to(ctx, &mut usb, Cycles(HOST_LATENCY));
        let first = read_packet(ctx, &mut usb);
        assert_eq!(first, &input[..64]);
        let read_at = ctx.now();
        run_to(ctx, &mut usb, read_at + (HOST_LATENCY - 1));
        assert_eq!(
            read_packet(ctx, &mut usb),
            [],
            "the host waits for the FIFO to be read"
        );
        run_to(ctx, &mut usb, read_at + HOST_LATENCY);
        assert_eq!(read_packet(ctx, &mut usb), &input[64..]);
    }

    #[test]
    fn an_input_of_no_byte_is_a_zero_length_packet() {
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new();
        usb.receive(ctx, Cycles::ZERO, b"");
        usb.receive(ctx, Cycles::ZERO, b"z");
        assert_eq!(usb.host_pending(), 2);
        run_to(ctx, &mut usb, Cycles(HOST_LATENCY));
        assert_eq!(
            usb.read(ctx, INT_RAW).unwrap() & OUT_EP1_ZERO_PAYLOAD,
            OUT_EP1_ZERO_PAYLOAD
        );
        assert_eq!(usb.read(ctx, INT_RAW).unwrap() & SERIAL_OUT_RECV_PKT, 0);
        run_to(ctx, &mut usb, Cycles(2 * HOST_LATENCY));
        assert_eq!(read_packet(ctx, &mut usb), b"z");
    }

    #[test]
    fn a_read_of_an_empty_out_fifo_returns_0_and_is_refused_by_name() {
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new();
        assert_eq!(usb.read(ctx, EP1), Some(0));
        assert_eq!(ctx.take_unsupported(), Some(Unsupported::UsbEmptyRead));
    }

    #[test]
    fn a_core_reset_empties_the_block_and_the_host_goes_on_sending() {
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new();
        send(ctx, &mut usb, b"sent\n");
        usb.write(ctx, EP1_CONF, WR_DONE);
        send(ctx, &mut usb, b"lost");
        usb.write(ctx, INT_ENA, SERIAL_IN_EMPTY);
        usb.receive(ctx, Cycles::ZERO, b"in");
        usb.receive(ctx, Cycles(10 * HOST_LATENCY), b"later");
        run_to(ctx, &mut usb, Cycles(HOST_LATENCY));
        usb.reset(ctx);
        assert_eq!(ctx.sources(), 0, "INT_ENA back to 0");
        assert_eq!(usb.read(ctx, INT_RAW), Some(SERIAL_IN_EMPTY));
        assert_eq!(
            read_packet(ctx, &mut usb),
            [],
            "the packet in the FIFO is lost"
        );
        assert_eq!(usb.take_tx(ctx), b"sent\n", "what the host took stays");
        assert_eq!(usb.take_written(), [], "the packet being filled is lost");
        run_to(ctx, &mut usb, Cycles(11 * HOST_LATENCY));
        assert_eq!(read_packet(ctx, &mut usb), b"later");
    }
}
