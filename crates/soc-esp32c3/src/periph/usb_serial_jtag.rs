//! USB-Serial-JTAG, device-to-host serial path. Register layout: `usb_serial_jtag_reg.h` (report §G.3).
//!
//! The model behaves like a board with a host attached and reading: the IN endpoint always has
//! room. Reception, interrupts and host attach/detach arrive in SP1b-3d.
use crate::ctx::Ctx;
use emu_core::Peripheral;

const EP1: u32 = 0x00;
const EP1_CONF: u32 = 0x04;
/// `EP1_CONF` bit 0, write 1: commit the bytes written so far as one IN packet. A write to `EP1_CONF`
/// acts on this bit only: bits 1 and 2 are read-only (report §G.3).
const WR_DONE: u32 = 1 << 0;
/// `EP1_CONF` bit 1, read-only: the IN endpoint can take a byte.
const SERIAL_IN_EP_DATA_FREE: u32 = 1 << 1;
/// Size of a full-speed bulk packet: the hardware sends a full FIFO without waiting for `WR_DONE`.
/// Source: ESP-IDF v5.5.2 `components/hal/esp32c3/include/hal/usb_serial_jtag_ll.h`, note on
/// `usb_serial_jtag_ll_txfifo_flush`: "When fifo is full (with 64 byte), HW will flush the buffer
/// automatically".
const PACKET_BYTES: usize = 64;

#[derive(Clone, Debug, Default)]
pub struct UsbSerialJtag {
    packet: Vec<u8>,
    tx: Vec<u8>,
}

impl UsbSerialJtag {
    #[must_use]
    pub const fn new() -> UsbSerialJtag {
        UsbSerialJtag {
            packet: Vec::new(),
            tx: Vec::new(),
        }
    }

    /// A core reset empties the packet being filled; what was sent stays for the console.
    pub(crate) fn reset(&mut self) {
        self.packet.clear();
    }

    /// The bytes sent to the host since the last call.
    pub fn take_tx(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.tx)
    }

    fn commit(&mut self) {
        self.tx.append(&mut self.packet);
    }
}

impl Peripheral<Ctx> for UsbSerialJtag {
    fn read(&mut self, _ctx: &mut Ctx, offset: u32) -> Option<u32> {
        (offset == EP1_CONF).then_some(SERIAL_IN_EP_DATA_FREE)
    }

    fn write(&mut self, _ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        match offset {
            EP1 => {
                self.packet.push(value.to_le_bytes()[0]);
                if self.packet.len() == PACKET_BYTES {
                    self.commit();
                }
                true
            }
            EP1_CONF => {
                if value & WR_DONE != 0 {
                    self.commit();
                }
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    fn send(ctx: &mut Ctx, usb: &mut UsbSerialJtag, text: &[u8]) {
        for &byte in text {
            assert!(usb.write(ctx, 0x00, u32::from(byte)));
        }
    }

    #[test]
    fn endpoint_always_has_room_and_no_received_data() {
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new();
        let conf = usb.read(ctx, 0x04).unwrap();
        assert_eq!(
            conf & 0b010,
            0b010,
            "SERIAL_IN_EP_DATA_FREE: the ROM drops the byte otherwise"
        );
        assert_eq!(
            conf & 0b101,
            0,
            "WR_DONE reads back clear, no data from the host"
        );
    }

    #[test]
    fn bytes_reach_the_host_when_the_packet_is_committed() {
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new();
        send(ctx, &mut usb, b"boot\n");
        assert_eq!(usb.take_tx(), [], "nothing is sent before WR_DONE");
        assert!(usb.write(ctx, 0x04, 1));
        assert_eq!(usb.take_tx(), b"boot\n");
        assert_eq!(usb.take_tx(), []);
    }

    #[test]
    fn full_packet_is_sent_without_waiting_for_a_commit() {
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new();
        send(ctx, &mut usb, &[b'x'; 64]);
        assert_eq!(usb.take_tx().len(), 64);
        send(ctx, &mut usb, b"y");
        assert_eq!(usb.take_tx(), []);
    }

    #[test]
    fn committing_an_empty_packet_sends_nothing_and_other_registers_are_unmodeled() {
        let ctx = &mut Ctx::default();
        let mut usb = UsbSerialJtag::new();
        assert!(usb.write(ctx, 0x04, 1));
        assert_eq!(usb.take_tx(), []);
        assert_eq!(
            usb.read(ctx, 0x08),
            None,
            "INT_RAW arrives with interrupts (SP1b-3d)"
        );
        assert!(!usb.write(ctx, 0x18, 0), "CONF0");
        assert_eq!(usb.read(ctx, 0x00), None, "no host-to-device data yet");
    }
}
