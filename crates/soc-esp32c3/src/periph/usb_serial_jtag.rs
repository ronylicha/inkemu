//! USB-Serial-JTAG, the serial endpoint in both directions. Register layout:
//! `usb_serial_jtag_reg.h` and `usb_serial_jtag_struct.h` (report USJ §A).
//!
//! The firmware writes the bytes for the host into the IN FIFO of `EP1` and commits them as a
//! packet with `EP1_CONF.WR_DONE`, or by filling the 64 bytes of a packet. The host's bytes come
//! as OUT packets of at most 64 bytes, one in the FIFO at a time, read from `EP1` (report USJ
//! §R.1, §R.5). The interrupt is source 26 (`USB_INTR_MAP`), level, driven as
//! `INT_RAW & INT_ENA != 0` after every change (report USJ §C).
//!
//! The host is part of the run (ADR 0006): absent, attached with its port closed, or open
//! ([`UsbHost`]), and its changes are dated in device time. While attached it starts a frame
//! every millisecond, which sets `SOF`; while open it takes each committed packet
//! [`HOST_LATENCY`] after it is committed, and sends its own (report USJ §D, §R.2 to §R.5).
//! Frames are counted from device time when the block is accessed, never one call per frame.
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
const FRAM_NUM: u32 = reg("FRAM_NUM");
/// Written by the driver and by Arduino's `HWCDC` (`CONF0`), or by nobody on the paths read
/// (report USJ §A): stored, from their reset values, with no effect.
const STORED: [u32; 5] = [
    reg("CONF0"),
    reg("TEST"),
    reg("MISC_CONF"),
    reg("MEM_CONF"),
    reg("DATE"),
];
/// The state of the JTAG FIFO and of the endpoints: read-only, served at their reset values.
///
/// ASSUMPTION: nobody reads them (report USJ §A: no consumer on the paths read touches them), so
/// the model does not derive them from its FIFOs. A firmware that reads them, or a register dump
/// of a chip with a packet in each FIFO, settles what they hold.
const AT_RESET: [u32; 8] = [
    reg("JFIFO_ST"),
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
/// `EP1_CONF` bit 1, read-only: the IN endpoint takes a byte: no committed packet waits for the
/// host, and the FIFO holds fewer than 64 bytes ("After writing `WR_DONE`, this will be 1'b0
/// until the data is sent to the USB Host", `usb_serial_jtag_struct.h:25`).
const SERIAL_IN_EP_DATA_FREE: u32 = 1 << 1;
/// `EP1_CONF` bit 2, read-only: the OUT FIFO holds a byte of the host.
const SERIAL_OUT_EP_DATA_AVAIL: u32 = 1 << 2;
/// The bits of `INT_RAW` the model sets (`usb_serial_jtag_reg.h:554-660`, report USJ §R.1):
/// `SOF` (a frame began), `SERIAL_OUT_RECV_PKT` (a packet of the host is in the OUT FIFO),
/// `SERIAL_IN_EMPTY` (the host took an IN packet; 1 at reset), `USB_BUS_RESET` (the host
/// attached), `OUT_EP1_ZERO_PAYLOAD` (the host sent a zero-length packet).
const SOF: u32 = 1 << 1;
const SERIAL_OUT_RECV_PKT: u32 = 1 << 2;
const SERIAL_IN_EMPTY: u32 = 1 << 3;
const USB_BUS_RESET: u32 = 1 << 9;
const OUT_EP1_ZERO_PAYLOAD: u32 = 1 << 10;
/// `INT_ENA` and `INT_CLR` take the 19 bits of `USB_SERIAL_JTAG_LL_INTR_MASK` (`ll.h:19`).
const INTERRUPT_BITS: u32 = 0x7FFFF;
/// `FRAM_NUM` holds the 11-bit number of the last frame (`SOF_FRAME_INDEX` [10:0]).
const FRAME_NUMBERS: u64 = 2048;
/// Size of a full-speed bulk packet: the hardware sends a full FIFO without waiting for `WR_DONE`.
/// Source: ESP-IDF v5.5.2 `components/hal/esp32c3/include/hal/usb_serial_jtag_ll.h`, note on
/// `usb_serial_jtag_ll_txfifo_flush`: "When fifo is full (with 64 byte), HW will flush the buffer
/// automatically".
pub const PACKET_BYTES: usize = 64;
/// The interrupt source of the block (report USJ §C).
const SOURCE: Source = Source::of("USB_INTR_MAP");
/// One full-speed frame: "SOF packet is sent by the HOST every 1ms on a full speed bus"
/// (`IDF:esp_driver_usb_serial_jtag/src/usb_serial_jtag_connection_monitor.c:39-41`).
pub const FRAME: u64 = SECOND / 1_000;
/// The device time the host takes to move a packet: an open host takes a committed IN packet
/// this long after it is committed, and its own OUT packet reaches the FIFO this long after it
/// is sent, or after the previous one has been read whole.
///
/// ASSUMPTION: 50 us, about one full-speed bulk transaction of 64 bytes with its token and its
/// handshake, independent of the frames (report USJ §D.2, O.5). No source gives a figure; a
/// board with a host reading and writing it settles it.
pub const HOST_LATENCY: u64 = 50 * SECOND / 1_000_000;

/// The USB host on the other side of the cable (report USJ §D.1, §R.2).
///
/// ASSUMPTION: a host whose port is closed starts frames but takes nothing (Linux `cdc-acm`
/// submits its bulk transfers when the tty is opened); an attach from `Absent` raises
/// `USB_BUS_RESET`, a detach raises nothing (report USJ O.3). A capture on a board settles it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UsbHost {
    /// No cable, or a charger: no frame, nothing taken, nothing sent.
    Absent,
    /// Enumerated, no program has the port open: a frame every millisecond, nothing taken,
    /// nothing sent.
    Attached,
    /// A program has the port open: frames, every packet taken, the host's bytes sent.
    #[default]
    Open,
}

impl UsbHost {
    /// The state as the command line and a front end name it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            UsbHost::Absent => "absent",
            UsbHost::Attached => "attached",
            UsbHost::Open => "open",
        }
    }

    /// Every state, in the order a front end lists them.
    pub const ALL: [UsbHost; 3] = [UsbHost::Absent, UsbHost::Attached, UsbHost::Open];
}

#[derive(Debug)]
pub struct UsbSerialJtag {
    stored: Storage,
    /// The bytes of the IN packet being filled.
    in_fifo: Vec<u8>,
    /// The committed IN packet the host has not taken, with the device time it was committed.
    committed: Option<(u64, Vec<u8>)>,
    /// The bytes the host took, for the console.
    tx: Vec<u8>,
    /// The packets the host sends and that have not reached the FIFO, each with the device
    /// time, in cycles, before which it is not sent; a zero-length packet is empty.
    sent: VecDeque<(u64, Vec<u8>)>,
    /// The bytes of `sent`, a zero-length packet counted as one: kept as the packets come and
    /// go, so that a count costs nothing however many packets wait.
    sent_bytes: usize,
    /// The packet of the host in the OUT FIFO, what the firmware has not read of it.
    out_fifo: VecDeque<u8>,
    /// When the OUT FIFO was last left empty: the host sends its next packet from then on.
    out_free_since: u64,
    /// The state of the host as of `settled_to`.
    host: UsbHost,
    /// The changes of the host still to come, by date.
    host_changes: VecDeque<(u64, UsbHost)>,
    /// The device time the host attached: frames start then and every [`FRAME`] after it.
    attached_at: Option<u64>,
    /// The device time the host opened its port.
    open_since: Option<u64>,
    /// The frames of the current attachment whose `SOF` has been raised.
    frames_raised: u64,
    /// `FRAM_NUM`: the number of the last frame.
    frame_number: u32,
    int_raw: u32,
    int_ena: u32,
    /// The call at the next instant the block has something to do.
    wake: Deadline,
}

impl Default for UsbSerialJtag {
    fn default() -> UsbSerialJtag {
        UsbSerialJtag::new(UsbHost::default())
    }
}

impl UsbSerialJtag {
    /// The block at power-on, with `host` as the host is when the chip starts.
    ///
    /// ASSUMPTION: a host attached at power-on starts its frames at device time 0 and raises no
    /// `USB_BUS_RESET`: frame 0 starts at 0 and sets `SOF`, so the first read of `INT_RAW` gives
    /// `0xA` with such a host and its reset value, `0x8`, with an absent one. The bus reset of an
    /// enumeration after power-on is not modelled; a capture of `INT_RAW` on a board plugged in
    /// before power-on settles it.
    #[must_use]
    pub fn new(host: UsbHost) -> UsbSerialJtag {
        let base = svd_regs::esp32c3::addr("USB_DEVICE", "EP1");
        let attached = host != UsbHost::Absent;
        UsbSerialJtag {
            stored: Storage::for_offsets(base, &STORED),
            in_fifo: Vec::new(),
            committed: None,
            tx: Vec::new(),
            sent: VecDeque::new(),
            sent_bytes: 0,
            out_fifo: VecDeque::new(),
            out_free_since: 0,
            host,
            host_changes: VecDeque::new(),
            attached_at: attached.then_some(0),
            open_since: (host == UsbHost::Open).then_some(0),
            frames_raised: 0,
            frame_number: 0,
            // The SVD's reset value: `SERIAL_IN_EMPTY`.
            int_raw: SERIAL_IN_EMPTY,
            int_ena: 0,
            wake: Deadline::default(),
        }
    }

    /// A core reset, or `SYSTEM.PERIP_RST_EN0` bit 23 (`USB_DEVICE_RST`), returns the block to
    /// its reset values and empties both FIFOs and the committed packet; what the host took
    /// stays for the console. `FRAM_NUM` reads 0 until the
    /// next frame starts. The host is no part of the chip: its state, its frames and the packets
    /// it is sending go on; the next of them comes the host latency after the reset, as after
    /// the OUT FIFO was read whole.
    ///
    /// ASSUMPTION: a core reset resets the block, as it resets "the whole digital system"
    /// (TRM §6.1.3, report IR §4.2), without the host seeing a detach; a CPU reset
    /// (`esp_restart` on the ESP32-C3) keeps the block, as report IR §4.2 lists for the other
    /// peripherals, and does not reach this function. The reset table of the TRM, or a chip
    /// whose USB-Serial-JTAG is read after a watchdog reset, settles it.
    pub(crate) fn reset(&mut self, ctx: &mut Ctx) {
        self.settle(ctx);
        self.stored = UsbSerialJtag::new(self.host).stored;
        self.in_fifo.clear();
        self.committed = None;
        self.out_fifo.clear();
        self.out_free_since = ctx.now().get();
        self.frame_number = 0;
        self.int_raw = SERIAL_IN_EMPTY;
        self.int_ena = 0;
        self.drive(ctx);
        self.arm(ctx);
    }

    /// The bytes the host took since the last call.
    pub fn take_tx(&mut self, ctx: &mut Ctx) -> Vec<u8> {
        self.settle(ctx);
        std::mem::take(&mut self.tx)
    }

    /// Every byte the firmware wrote and no call has taken, the committed packet and the packet
    /// being filled included: for the end of a run, so that the console shows what the firmware
    /// had written when it stopped.
    pub fn take_written(&mut self) -> Vec<u8> {
        let mut written = std::mem::take(&mut self.tx);
        if let Some((_, packet)) = self.committed.take() {
            written.extend(packet);
        }
        written.append(&mut self.in_fifo);
        written
    }

    /// The host sends `bytes` from device time `at` (now or later), after what it sent before:
    /// packets of at most 64 bytes; no byte at all is one zero-length packet. A packet waits for
    /// the host to have its port open. The caller bounds how many it hands over
    /// ([`UsbSerialJtag::host_pending`]).
    pub fn receive(&mut self, ctx: &mut Ctx, at: Cycles, bytes: &[u8]) {
        self.settle(ctx);
        let date = at.max(ctx.now()).get();
        if bytes.is_empty() {
            self.sent.push_back((date, Vec::new()));
        }
        for packet in bytes.chunks(PACKET_BYTES) {
            self.sent.push_back((date, packet.to_vec()));
        }
        self.sent_bytes = self.sent_bytes.saturating_add(bytes.len().max(1));
        self.arm(ctx);
    }

    /// From device time `at` (now or later) the host is `state`. Changes given for the same
    /// instant follow each other in the order given. The caller bounds how many it hands over
    /// ([`UsbSerialJtag::host_changes_pending`]).
    pub fn set_host(&mut self, ctx: &mut Ctx, at: Cycles, state: UsbHost) {
        self.settle(ctx);
        let date = at.max(ctx.now()).get();
        let place = self
            .host_changes
            .partition_point(|&(other, _)| other <= date);
        self.host_changes.insert(place, (date, state));
        self.arm(ctx);
    }

    /// The state of the host now, the changes due by now applied.
    pub fn host(&mut self, ctx: &mut Ctx) -> UsbHost {
        self.settle(ctx);
        self.host
    }

    /// How many bytes the host has sent that the firmware has not read, its packets counted as
    /// one byte each at least, so that zero-length ones count too.
    #[must_use]
    pub fn host_pending(&self) -> usize {
        self.out_fifo.len() + self.sent_bytes
    }

    /// Whether the host took bytes that no call has taken. The host takes a packet at an instant
    /// the block asked to be called at: a front end that watches this channel sees it then.
    #[must_use]
    pub(crate) fn has_left(&self) -> bool {
        !self.tx.is_empty()
    }

    /// How many changes of the host are still to come, those due by now applied first.
    pub fn host_changes_pending(&mut self, ctx: &mut Ctx) -> usize {
        self.settle(ctx);
        self.host_changes.len()
    }

    /// The device time of a wake-up has come.
    pub(crate) fn wake(&mut self, ctx: &mut Ctx) {
        self.wake.called(ctx.now());
        self.settle(ctx);
        self.arm(ctx);
    }

    /// The instant an open host takes the committed packet.
    fn drain_at(&self) -> Option<u64> {
        let (committed_at, _) = self.committed.as_ref()?;
        let open_since = self.open_since?;
        Some((*committed_at).max(open_since).saturating_add(HOST_LATENCY))
    }

    /// The instant the next packet of an open host reaches the FIFO: once the FIFO is empty, the
    /// host latency after the packet is sent, the FIFO was left empty or the port opened,
    /// whichever is latest.
    fn next_packet_at(&self) -> Option<u64> {
        if !self.out_fifo.is_empty() {
            return None;
        }
        let open_since = self.open_since?;
        let &(date, _) = self.sent.front()?;
        Some(
            date.max(self.out_free_since)
                .max(open_since)
                .saturating_add(HOST_LATENCY),
        )
    }

    /// The frames started from `attached_at` to `instant`, the one starting at `instant`
    /// included.
    fn frames_by(&self, instant: u64) -> u64 {
        self.attached_at
            .filter(|&attached_at| attached_at <= instant)
            .map_or(0, |attached_at| (instant - attached_at) / FRAME + 1)
    }

    /// Raises `SOF` for the frames started by `instant`.
    fn raise_frames(&mut self, instant: u64) {
        let frames = self.frames_by(instant);
        if frames > self.frames_raised {
            self.frames_raised = frames;
            self.int_raw |= SOF;
            // INVARIANT: the remainder is below 2048.
            self.frame_number = u32::try_from((frames - 1) % FRAME_NUMBERS).unwrap_or(0);
        }
    }

    /// Asks to be called at the next instant something happens that the firmware can see: the
    /// host takes the committed packet, a packet of the host arrives, the host changes, and,
    /// only while `SOF` is enabled and lowered, the next frame starts. A frame that starts while
    /// `SOF` is still raised changes nothing the firmware sees (`FRAM_NUM` is worked out when it
    /// is read); the write of `INT_CLR` that lowers it arms the call again.
    fn arm(&mut self, ctx: &mut Ctx) {
        let next_frame = (self.int_ena & SOF != 0 && self.int_raw & SOF == 0)
            .then(|| {
                self.attached_at.map(|attached_at| {
                    attached_at.saturating_add(self.frames_raised.saturating_mul(FRAME))
                })
            })
            .flatten();
        let next_change = self.host_changes.front().map(|&(at, _)| at);
        for instant in [
            self.drain_at(),
            self.next_packet_at(),
            next_change,
            next_frame,
        ]
        .into_iter()
        .flatten()
        {
            self.wake.arm(ctx, Cycles(instant), Event::UsbSerialJtag);
        }
    }

    /// Brings the block to now: the host changes, takes the committed packet and sends its own,
    /// in the order of their instants (a change of the host first at an instant they share); the
    /// frames started raise `SOF`; the interrupt source follows.
    fn settle(&mut self, ctx: &mut Ctx) {
        let now = ctx.now().get();
        // INVARIANT: each pass applies a change, takes the committed packet or takes a packet of
        // the host, all of them bounded by the caller (`host_pending`, `host_changes_pending`).
        loop {
            let change = self.host_changes.front().map(|&(at, _)| at);
            let next = [change, self.drain_at(), self.next_packet_at()]
                .into_iter()
                .flatten()
                .min()
                .filter(|&at| at <= now);
            let Some(at) = next else {
                break;
            };
            if change == Some(at) {
                self.change_host(at);
            } else if self.drain_at() == Some(at) {
                if let Some((_, packet)) = self.committed.take() {
                    self.tx.extend(packet);
                    self.int_raw |= SERIAL_IN_EMPTY;
                }
            } else {
                self.take_packet(at);
            }
        }
        self.raise_frames(now);
        self.drive(ctx);
    }

    /// The host becomes what its first pending change says, at `at`.
    fn change_host(&mut self, at: u64) {
        let Some((_, state)) = self.host_changes.pop_front() else {
            return;
        };
        self.raise_frames(at);
        if self.host == UsbHost::Absent && state != UsbHost::Absent {
            self.attached_at = Some(at);
            self.frames_raised = 0;
            self.int_raw |= USB_BUS_RESET;
            self.raise_frames(at);
        }
        if state == UsbHost::Absent {
            self.attached_at = None;
        }
        self.open_since = match (self.open_since, state) {
            (Some(since), UsbHost::Open) => Some(since),
            (None, UsbHost::Open) => Some(at),
            (_, UsbHost::Absent | UsbHost::Attached) => None,
        };
        self.host = state;
    }

    /// The next packet of the host reaches the OUT FIFO at `at`.
    fn take_packet(&mut self, at: u64) {
        let Some((_, packet)) = self.sent.pop_front() else {
            return;
        };
        self.sent_bytes = self.sent_bytes.saturating_sub(packet.len().max(1));
        if packet.is_empty() {
            // A zero-length packet leaves the FIFO empty: the next one can follow.
            //
            // ASSUMPTION: it sets `OUT_EP1_ZERO_PAYLOAD` but not `SERIAL_OUT_RECV_PKT`: no byte
            // reaches the FIFO, and report USJ §R.1 names only the first. A capture of `INT_RAW`
            // after a host sends a zero-length packet settles it.
            self.int_raw |= OUT_EP1_ZERO_PAYLOAD;
            self.out_free_since = at;
        } else {
            self.out_fifo.extend(packet);
            self.int_raw |= SERIAL_OUT_RECV_PKT;
        }
    }

    /// The bytes written so far leave the FIFO as one packet, for the host to take.
    ///
    /// ASSUMPTION: `WR_DONE` while a committed packet waits for the host does nothing: the
    /// endpoint holds one packet (report USJ §R.1). `HWCDC` writes it each time it finds itself
    /// not connected (report USJ §B.5); the TRM settles it.
    fn commit(&mut self, now: u64) {
        if self.committed.is_some() {
            return;
        }
        self.committed = Some((now, std::mem::take(&mut self.in_fifo)));
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

    /// A write of `EP1`: a byte for the host. The 64th commits the packet.
    ///
    /// ASSUMPTION: a byte written while the endpoint has no room is ignored (report USJ §R.1,
    /// O.6), and refused by name. Every consumer read looks at `SERIAL_IN_EP_DATA_FREE` first;
    /// the TRM settles it.
    fn push(&mut self, ctx: &mut Ctx, byte: u8) {
        if self.ep1_conf() & SERIAL_IN_EP_DATA_FREE == 0 {
            ctx.unsupported(Unsupported::UsbInFull);
            return;
        }
        self.in_fifo.push(byte);
        if self.in_fifo.len() == PACKET_BYTES {
            self.commit(ctx.now().get());
        }
    }

    fn ep1_conf(&self) -> u32 {
        let free = if self.committed.is_none() && self.in_fifo.len() < PACKET_BYTES {
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
            FRAM_NUM => self.frame_number,
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
                self.push(ctx, value.to_le_bytes()[0]);
                true
            }
            EP1_CONF => {
                if value & WR_DONE != 0 {
                    self.commit(ctx.now().get());
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
            // `INT_RAW` is set by the block and cleared through `INT_CLR`; `INT_ST` and
            // `FRAM_NUM` are read-only. A write to one of them changes nothing and is reported as
            // unmodelled, as UART and TIMG do with theirs: the header gives `INT_RAW` the access
            // R/WTC/SS without saying what a write does, report USJ §R.1 has writes ignored, and
            // no consumer on the paths read writes it (report USJ §A).
            INT_RAW | INT_ST | FRAM_NUM => false,
            _ => self.stored.write(offset, value),
        };
        self.drive(ctx);
        self.arm(ctx);
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

    /// Moves device time to `instant`, delivering each call the block asked for on the way. A
    /// block that asks for a call at the instant it is called again and again fails the test.
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

    fn open() -> UsbSerialJtag {
        UsbSerialJtag::new(UsbHost::Open)
    }

    #[test]
    fn the_registers_start_at_their_svd_reset_values() {
        // Report USJ §A.
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new(UsbHost::Absent);
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
        assert!(!usb.write(ctx, reg("FRAM_NUM"), 0), "read-only");
        assert!(!usb.write(ctx, INT_ST, 0), "read-only");
        assert!(
            !usb.write(ctx, INT_RAW, INTERRUPT_BITS),
            "set by the block, cleared through INT_CLR"
        );
        assert_eq!(
            usb.read(ctx, INT_RAW),
            Some(0x8),
            "the write changed nothing"
        );
        assert_eq!(usb.read(ctx, 0x7C), None, "no register there");
        assert_eq!(ctx.sources(), 0);
        // With the default host, attached and open at power-on, frame 0 starts at 0.
        assert_eq!(
            UsbSerialJtag::default().read(ctx, INT_RAW),
            Some(0xA),
            "SERIAL_IN_EMPTY and SOF"
        );
    }

    #[test]
    fn an_open_host_takes_a_committed_packet_after_the_host_latency() {
        // The ROM: read EP1_CONF bit 1, write the byte, WR_DONE after `\n` (report USJ §B.1).
        let ctx = &mut Ctx::default();
        let mut usb = open();
        usb.write(ctx, INT_CLR, SERIAL_IN_EMPTY);
        for &byte in b"boot\n" {
            assert_eq!(usb.read(ctx, EP1_CONF).unwrap() & SERIAL_IN_EP_DATA_FREE, 2);
            usb.write(ctx, EP1, u32::from(byte));
        }
        assert_eq!(usb.take_tx(ctx), [], "nothing is sent before WR_DONE");
        assert!(usb.write(ctx, EP1_CONF, WR_DONE));
        assert_eq!(
            usb.read(ctx, EP1_CONF),
            Some(0),
            "no room until the host takes it"
        );
        assert_eq!(ctx.next_wake(), Some(Cycles(HOST_LATENCY)));
        run_to(ctx, &mut usb, Cycles(HOST_LATENCY - 1));
        assert_eq!(usb.take_tx(ctx), []);
        run_to(ctx, &mut usb, Cycles(HOST_LATENCY));
        assert_eq!(usb.take_tx(ctx), b"boot\n");
        assert_eq!(
            usb.read(ctx, INT_RAW).unwrap() & SERIAL_IN_EMPTY,
            SERIAL_IN_EMPTY
        );
        assert_eq!(usb.read(ctx, EP1_CONF), Some(SERIAL_IN_EP_DATA_FREE));
    }

    #[test]
    fn a_full_packet_is_committed_without_waiting_for_wr_done() {
        let ctx = &mut Ctx::default();
        let mut usb = open();
        send(ctx, &mut usb, &[b'x'; 64]);
        assert_eq!(usb.read(ctx, EP1_CONF), Some(0));
        run_to(ctx, &mut usb, Cycles(HOST_LATENCY));
        assert_eq!(usb.take_tx(ctx).len(), 64);
        send(ctx, &mut usb, b"y");
        assert_eq!(usb.take_tx(ctx), []);
        assert_eq!(usb.take_written(), b"y", "the end of a run takes the rest");
    }

    #[test]
    fn a_byte_written_while_a_packet_waits_for_the_host_is_refused_and_lost() {
        let ctx = &mut Ctx::default();
        let mut usb = open();
        send(ctx, &mut usb, b"a");
        usb.write(ctx, EP1_CONF, WR_DONE);
        assert!(usb.write(ctx, EP1, u32::from(b'b')));
        assert_eq!(ctx.take_unsupported(), Some(Unsupported::UsbInFull));
        usb.write(ctx, EP1_CONF, WR_DONE);
        run_to(ctx, &mut usb, Cycles(10 * HOST_LATENCY));
        assert_eq!(usb.take_tx(ctx), b"a");
        assert_eq!(
            usb.read(ctx, EP1_CONF),
            Some(SERIAL_IN_EP_DATA_FREE),
            "the second WR_DONE did nothing"
        );
    }

    #[test]
    fn a_commit_of_nothing_is_a_zero_length_packet_the_host_takes() {
        // The driver ends a transfer of 64 bytes with one (report USJ §B.4).
        let ctx = &mut Ctx::default();
        let mut usb = open();
        usb.write(ctx, INT_CLR, SERIAL_IN_EMPTY);
        assert!(usb.write(ctx, EP1_CONF, WR_DONE));
        run_to(ctx, &mut usb, Cycles(HOST_LATENCY));
        assert_eq!(usb.take_tx(ctx), []);
        assert_eq!(
            usb.read(ctx, INT_RAW).unwrap() & SERIAL_IN_EMPTY,
            SERIAL_IN_EMPTY
        );
    }

    #[test]
    fn hwcdc_begin_takes_its_first_interrupt_from_the_reset_value_of_serial_in_empty() {
        // Report USJ §B.5: `begin` clears the 19 enables, sets bits 2, 3 and 9, and never writes
        // INT_CLR: the reset value of SERIAL_IN_EMPTY runs the handler at once.
        let ctx = &mut Ctx::default();
        let mut usb = open();
        usb.write(ctx, reg("CONF0"), 0x4200);
        let ena = usb.read(ctx, INT_ENA).unwrap();
        usb.write(ctx, INT_ENA, ena & !INTERRUPT_BITS);
        let ena = usb.read(ctx, INT_ENA).unwrap();
        usb.write(
            ctx,
            INT_ENA,
            ena | SERIAL_OUT_RECV_PKT | SERIAL_IN_EMPTY | USB_BUS_RESET,
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
        let mut usb = open();
        usb.write(ctx, INT_ENA, SERIAL_OUT_RECV_PKT);
        usb.receive(ctx, Cycles(1_000), b"hi, espressif\n");
        run_to(ctx, &mut usb, Cycles(1_000 + HOST_LATENCY - 1));
        assert_eq!(
            usb.read(ctx, EP1_CONF).unwrap() & SERIAL_OUT_EP_DATA_AVAIL,
            0
        );
        assert_eq!(ctx.sources(), 0);
        assert_eq!(ctx.next_wake(), Some(Cycles(1_000 + HOST_LATENCY)));
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
        let mut usb = open();
        let input: Vec<u8> = (0..90).collect();
        usb.receive(ctx, Cycles::ZERO, &input);
        assert_eq!(usb.host_pending(), 90);
        run_to(ctx, &mut usb, Cycles(HOST_LATENCY));
        assert_eq!(read_packet(ctx, &mut usb), &input[..64]);
        assert_eq!(usb.host_pending(), 26, "the second packet is still to come");
        let read_at = ctx.now();
        run_to(ctx, &mut usb, read_at + (HOST_LATENCY - 1));
        assert_eq!(
            read_packet(ctx, &mut usb),
            [],
            "the host waits for the FIFO to be read"
        );
        run_to(ctx, &mut usb, read_at + HOST_LATENCY);
        assert_eq!(read_packet(ctx, &mut usb), &input[64..]);
        assert_eq!(usb.host_pending(), 0);
    }

    #[test]
    fn an_input_of_no_byte_is_a_zero_length_packet() {
        let ctx = &mut Ctx::default();
        let mut usb = open();
        usb.receive(ctx, Cycles::ZERO, b"");
        usb.receive(ctx, Cycles::ZERO, b"z");
        assert_eq!(usb.host_pending(), 2);
        run_to(ctx, &mut usb, Cycles(HOST_LATENCY));
        assert_eq!(
            usb.read(ctx, INT_RAW).unwrap() & OUT_EP1_ZERO_PAYLOAD,
            OUT_EP1_ZERO_PAYLOAD
        );
        assert_eq!(usb.read(ctx, INT_RAW).unwrap() & SERIAL_OUT_RECV_PKT, 0);
        assert_eq!(usb.host_pending(), 1, "the zero-length packet is gone");
        run_to(ctx, &mut usb, Cycles(2 * HOST_LATENCY));
        assert_eq!(read_packet(ctx, &mut usb), b"z");
        assert_eq!(usb.host_pending(), 0);
    }

    #[test]
    fn a_read_of_an_empty_out_fifo_returns_0_and_is_refused_by_name() {
        let ctx = &mut Ctx::default();
        let mut usb = open();
        assert_eq!(usb.read(ctx, EP1), Some(0));
        assert_eq!(ctx.take_unsupported(), Some(Unsupported::UsbEmptyRead));
    }

    #[test]
    fn a_core_reset_empties_the_block_and_the_host_goes_on() {
        let ctx = &mut Ctx::default();
        let mut usb = open();
        send(ctx, &mut usb, b"sent\n");
        usb.write(ctx, EP1_CONF, WR_DONE);
        usb.write(ctx, INT_ENA, SERIAL_IN_EMPTY);
        usb.receive(ctx, Cycles::ZERO, b"in");
        usb.receive(ctx, Cycles(10 * HOST_LATENCY), b"later");
        run_to(ctx, &mut usb, Cycles(HOST_LATENCY));
        send(ctx, &mut usb, b"lost");
        usb.reset(ctx);
        assert_eq!(ctx.sources(), 0, "INT_ENA back to 0");
        assert_eq!(
            usb.read(ctx, INT_RAW),
            Some(SERIAL_IN_EMPTY),
            "the SOF of frame 0 is gone, frame 1 has not started"
        );
        assert_eq!(
            read_packet(ctx, &mut usb),
            [],
            "the packet in the FIFO is lost"
        );
        assert_eq!(usb.take_tx(ctx), b"sent\n", "what the host took stays");
        assert_eq!(usb.take_written(), [], "the packet being filled is lost");
        run_to(ctx, &mut usb, Cycles(11 * HOST_LATENCY));
        assert_eq!(read_packet(ctx, &mut usb), b"later");
        assert_eq!(usb.host(ctx), UsbHost::Open);
    }

    #[test]
    fn after_a_core_reset_the_next_packet_of_the_host_comes_the_host_latency_later() {
        // The reset empties the OUT FIFO: the host sends again as after the FIFO is read whole.
        let ctx = &mut Ctx::default();
        let mut usb = open();
        usb.receive(ctx, Cycles::ZERO, &[b'a'; 2 * PACKET_BYTES]);
        run_to(ctx, &mut usb, Cycles(2 * HOST_LATENCY));
        usb.reset(ctx);
        assert_eq!(read_packet(ctx, &mut usb), [], "the first packet is lost");
        run_to(ctx, &mut usb, Cycles(3 * HOST_LATENCY - 1));
        assert_eq!(read_packet(ctx, &mut usb), []);
        run_to(ctx, &mut usb, Cycles(3 * HOST_LATENCY));
        assert_eq!(read_packet(ctx, &mut usb), [b'a'; PACKET_BYTES]);
    }

    #[test]
    fn a_core_reset_sets_fram_num_to_0_until_the_next_frame() {
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new(UsbHost::Attached);
        ctx.sleep_until(Cycles(3 * FRAME + 7));
        assert_eq!(usb.read(ctx, FRAM_NUM), Some(3));
        usb.reset(ctx);
        assert_eq!(usb.read(ctx, FRAM_NUM), Some(0), "the SVD's reset value");
        assert_eq!(usb.read(ctx, INT_RAW), Some(SERIAL_IN_EMPTY));
        ctx.sleep_until(Cycles(4 * FRAME));
        assert_eq!(usb.read(ctx, FRAM_NUM), Some(4), "the host's frames go on");
        assert_eq!(usb.read(ctx, INT_RAW), Some(SERIAL_IN_EMPTY | SOF));
    }

    #[test]
    fn an_attached_host_starts_a_frame_every_millisecond_and_the_connection_monitor_sees_each() {
        // Report USJ §B.3: the tick hook reads INT_RAW & SOF and writes INT_CLR = SOF, at 100 Hz
        // in the reference build; it must see a SOF at every tick, or it declares the host gone.
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new(UsbHost::Attached);
        for tick in 1..=50 {
            ctx.sleep_until(Cycles(tick * 10 * FRAME));
            let sof = usb.read(ctx, INT_RAW).unwrap() & SOF;
            assert_eq!(sof, SOF, "tick {tick}");
            usb.write(ctx, INT_CLR, SOF);
            assert_eq!(
                usb.read(ctx, INT_RAW).unwrap() & SOF,
                0,
                "cleared until the next frame"
            );
        }
        assert_eq!(
            ctx.next_wake(),
            None,
            "no call per frame while SOF is not enabled"
        );
        // A 1 kHz tick too, a little after each frame start.
        for tick in 501..=520 {
            ctx.sleep_until(Cycles(tick * FRAME + 7));
            assert_eq!(usb.read(ctx, INT_RAW).unwrap() & SOF, SOF, "tick {tick}");
            usb.write(ctx, INT_CLR, SOF);
        }
        assert_eq!(
            usb.read(ctx, FRAM_NUM),
            Some(520),
            "the frame of the last SOF"
        );
        ctx.sleep_until(Cycles(2048 * FRAME + 3 * FRAME));
        assert_eq!(usb.read(ctx, FRAM_NUM), Some(3), "11 bits");
    }

    #[test]
    fn an_enabled_sof_asks_for_a_call_at_each_frame_start() {
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new(UsbHost::Attached);
        usb.write(ctx, INT_CLR, SOF);
        usb.write(ctx, INT_ENA, SOF);
        assert_eq!(ctx.sources(), 0);
        assert_eq!(ctx.next_wake(), Some(Cycles(FRAME)));
        ctx.sleep_until(Cycles(FRAME));
        assert!(ctx.take_due().is_some());
        usb.wake(ctx);
        assert_eq!(ctx.sources(), USB_SOURCE);
    }

    #[test]
    fn a_raised_sof_asks_for_no_call_until_it_is_cleared() {
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new(UsbHost::Attached);
        usb.write(ctx, INT_ENA, SOF);
        assert_eq!(ctx.sources(), USB_SOURCE, "the SOF of frame 0");
        assert_eq!(ctx.next_wake(), None, "the line is high already");
        ctx.sleep_until(Cycles(3 * FRAME + 5));
        usb.write(ctx, INT_CLR, SOF);
        assert_eq!(ctx.sources(), 0);
        assert_eq!(ctx.next_wake(), Some(Cycles(4 * FRAME)), "the next frame");
        ctx.sleep_until(Cycles(4 * FRAME));
        assert!(ctx.take_due().is_some());
        usb.wake(ctx);
        assert_eq!(ctx.sources(), USB_SOURCE);
        assert_eq!(ctx.next_wake(), None);
        assert_eq!(usb.read(ctx, FRAM_NUM), Some(4));
    }

    #[test]
    fn frames_counted_up_to_the_end_of_device_time_do_not_overflow() {
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new(UsbHost::Attached);
        ctx.sleep_until(Cycles(u64::MAX - 5));
        usb.write(ctx, INT_CLR, SOF);
        usb.write(ctx, INT_ENA, SOF);
        assert_eq!(ctx.next_wake(), None, "the next frame is past the end");
        assert_eq!(ctx.sources(), 0);
    }

    #[test]
    fn an_absent_host_starts_no_frame_and_takes_nothing() {
        // Report USJ §B.1, §F.3: the ROM's first packet stays, and the endpoint has no room after.
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new(UsbHost::Absent);
        send(ctx, &mut usb, b"ESP-ROM\n");
        usb.write(ctx, EP1_CONF, WR_DONE);
        usb.receive(ctx, Cycles::ZERO, b"x");
        run_to(ctx, &mut usb, Cycles(1_000 * FRAME));
        assert_eq!(usb.read(ctx, INT_RAW).unwrap() & SOF, 0);
        assert_eq!(usb.take_tx(ctx), []);
        assert_eq!(
            usb.read(ctx, EP1_CONF),
            Some(0),
            "no room, nothing received"
        );
        assert_eq!(usb.take_written(), b"ESP-ROM\n");
    }

    #[test]
    fn a_host_with_its_port_closed_starts_frames_but_takes_and_sends_nothing() {
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new(UsbHost::Attached);
        send(ctx, &mut usb, b"line\n");
        usb.write(ctx, EP1_CONF, WR_DONE);
        usb.receive(ctx, Cycles::ZERO, b"in");
        run_to(ctx, &mut usb, Cycles(100 * FRAME));
        assert_eq!(usb.read(ctx, INT_RAW).unwrap() & SOF, SOF);
        assert_eq!(usb.take_tx(ctx), []);
        assert_eq!(read_packet(ctx, &mut usb), []);
        // The port opens: the packet waiting since its commit leaves the host latency after,
        // and the input comes as long after.
        let opened = Cycles(200 * FRAME);
        usb.set_host(ctx, opened, UsbHost::Open);
        run_to(ctx, &mut usb, opened + (HOST_LATENCY - 1));
        assert_eq!(usb.take_tx(ctx), []);
        assert_eq!(read_packet(ctx, &mut usb), []);
        run_to(ctx, &mut usb, opened + HOST_LATENCY);
        assert_eq!(usb.take_tx(ctx), b"line\n");
        assert_eq!(read_packet(ctx, &mut usb), b"in");
        assert_eq!(
            usb.read(ctx, INT_RAW).unwrap() & USB_BUS_RESET,
            0,
            "opening the port is no attach"
        );
        assert_eq!(usb.read(ctx, FRAM_NUM), Some(200), "nor a new first frame");
        // The port closes: a packet committed then waits again.
        let closed = ctx.now() + 1;
        usb.set_host(ctx, closed, UsbHost::Attached);
        run_to(ctx, &mut usb, closed);
        send(ctx, &mut usb, b"again\n");
        usb.write(ctx, EP1_CONF, WR_DONE);
        run_to(ctx, &mut usb, closed + 10 * HOST_LATENCY);
        assert_eq!(usb.take_tx(ctx), []);
    }

    #[test]
    fn an_attach_raises_usb_bus_reset_and_starts_the_frames_from_its_instant_and_a_detach_raises_nothing()
     {
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new(UsbHost::Absent);
        let attach = Cycles(5 * FRAME + 123);
        usb.set_host(ctx, attach, UsbHost::Open);
        assert_eq!(ctx.next_wake(), Some(attach));
        run_to(ctx, &mut usb, attach);
        assert_eq!(usb.host(ctx), UsbHost::Open);
        assert_eq!(
            usb.read(ctx, INT_RAW),
            Some(SERIAL_IN_EMPTY | SOF | USB_BUS_RESET)
        );
        assert_eq!(usb.read(ctx, FRAM_NUM), Some(0));
        usb.write(ctx, INT_CLR, INTERRUPT_BITS);
        ctx.sleep_until(attach + (FRAME - 1));
        assert_eq!(
            usb.read(ctx, INT_RAW),
            Some(0),
            "frame 1 starts a millisecond later"
        );
        ctx.sleep_until(attach + FRAME);
        assert_eq!(usb.read(ctx, INT_RAW), Some(SOF));
        let detach = attach + 3 * FRAME;
        usb.set_host(ctx, detach, UsbHost::Absent);
        usb.write(ctx, INT_CLR, INTERRUPT_BITS);
        run_to(ctx, &mut usb, detach + 10 * FRAME);
        assert_eq!(
            usb.read(ctx, INT_RAW),
            Some(SOF),
            "the frame of the detach instant, no more"
        );
        assert_eq!(usb.read(ctx, FRAM_NUM), Some(3));
        usb.write(ctx, INT_CLR, INTERRUPT_BITS);
        ctx.sleep_until(detach + 20 * FRAME);
        assert_eq!(usb.read(ctx, INT_RAW), Some(0));
    }

    #[test]
    fn changes_of_the_host_given_out_of_order_apply_in_the_order_of_their_dates() {
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new(UsbHost::Absent);
        usb.set_host(ctx, Cycles(2 * FRAME), UsbHost::Absent);
        usb.set_host(ctx, Cycles(FRAME), UsbHost::Attached);
        assert_eq!(usb.host_changes_pending(ctx), 2);
        run_to(ctx, &mut usb, Cycles(FRAME));
        assert_eq!(usb.host(ctx), UsbHost::Attached);
        run_to(ctx, &mut usb, Cycles(2 * FRAME));
        assert_eq!(usb.host(ctx), UsbHost::Absent);
        assert_eq!(usb.host_changes_pending(ctx), 0);
    }

    #[test]
    fn a_change_of_the_host_due_by_now_is_not_counted_as_to_come() {
        // The machine bounds the changes still to come: one due but not yet applied (no call has
        // reached the block since its date) takes no room.
        let ctx = &mut Ctx::default();
        let mut usb = open();
        usb.set_host(ctx, Cycles(FRAME), UsbHost::Attached);
        ctx.sleep_until(Cycles(FRAME));
        assert_eq!(usb.host_changes_pending(ctx), 0);
        assert_eq!(usb.host(ctx), UsbHost::Attached);
    }

    #[test]
    fn a_host_that_closes_its_port_at_the_instant_it_would_take_a_packet_does_not_take_it() {
        // A change of the host comes first at an instant it shares with the taking of a packet.
        let ctx = &mut Ctx::default();
        let mut usb = open();
        send(ctx, &mut usb, b"x");
        usb.write(ctx, EP1_CONF, WR_DONE);
        usb.set_host(ctx, Cycles(HOST_LATENCY), UsbHost::Attached);
        run_to(ctx, &mut usb, Cycles(10 * HOST_LATENCY));
        assert_eq!(usb.take_tx(ctx), [], "the port closed first");
        assert_eq!(usb.read(ctx, EP1_CONF), Some(0), "the packet still waits");
        assert_eq!(usb.take_written(), b"x");
    }

    #[test]
    fn two_changes_of_the_host_at_the_same_instant_apply_in_the_order_given() {
        // A cable pulled out and plugged in again at one step of a script.
        let ctx = &mut Ctx::default();
        let mut usb = open();
        let instant = Cycles(5 * FRAME);
        usb.set_host(ctx, instant, UsbHost::Absent);
        usb.set_host(ctx, instant, UsbHost::Open);
        run_to(ctx, &mut usb, instant);
        assert_eq!(usb.host(ctx), UsbHost::Open);
        assert_eq!(
            usb.read(ctx, INT_RAW).unwrap() & USB_BUS_RESET,
            USB_BUS_RESET,
            "attached again"
        );
        assert_eq!(usb.read(ctx, FRAM_NUM), Some(0), "the frames start again");
    }

    #[test]
    fn a_host_attached_again_starts_its_frames_from_the_new_attach() {
        // The connection monitor (report USJ §B.3) must see a SOF at the first tick after the
        // cable is plugged in again, however many frames the first attachment had.
        let ctx = &mut Ctx::default();
        let mut usb = open();
        run_to(ctx, &mut usb, Cycles(5 * FRAME));
        usb.set_host(ctx, Cycles(5 * FRAME), UsbHost::Absent);
        usb.set_host(ctx, Cycles(7 * FRAME), UsbHost::Open);
        run_to(ctx, &mut usb, Cycles(6 * FRAME));
        usb.write(ctx, INT_CLR, INTERRUPT_BITS);
        run_to(ctx, &mut usb, Cycles(7 * FRAME));
        assert_eq!(usb.read(ctx, INT_RAW), Some(SOF | USB_BUS_RESET));
        assert_eq!(usb.read(ctx, FRAM_NUM), Some(0));
        usb.write(ctx, INT_CLR, SOF);
        ctx.sleep_until(Cycles(8 * FRAME));
        assert_eq!(
            usb.read(ctx, INT_RAW).unwrap() & SOF,
            SOF,
            "and one each ms"
        );
        assert_eq!(usb.read(ctx, FRAM_NUM), Some(1));
    }

    #[test]
    fn the_host_names_its_states_as_the_command_line_does() {
        let names: Vec<&str> = UsbHost::ALL.iter().map(|host| host.name()).collect();
        assert_eq!(names, ["absent", "attached", "open"]);
        assert_eq!(UsbHost::default(), UsbHost::Open, "report USJ §R.2");
    }
}
