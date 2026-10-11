//! UART controller, transmit side. Register layout: `uart_reg.h` (report §G.2).
//!
//! Decision D7 of the SP1b-1 plan: a byte written to the FIFO leaves at once, so the transmit FIFO
//! always reads empty and the baud rate has no effect. Real timing, reception and interrupts arrive
//! in SP1b-3; until then the emulated boot is shorter than the real one by the time the UART would
//! take to send its log.
use crate::periph::Storage;
use emu_core::Peripheral;

const FIFO: u32 = 0x00;
const INT_ENA: u32 = 0x0C;
const INT_CLR: u32 = 0x10;
const CLKDIV: u32 = 0x14;
const STATUS: u32 = 0x1C;
const CONF0: u32 = 0x20;
const CONF1: u32 = 0x24;
const CLK_CONF: u32 = 0x78;
/// Configuration the ROM and ESP-IDF write and read back; nothing acts on it yet.
const CONFIG: [u32; 5] = [INT_ENA, CLKDIV, CONF0, CONF1, CLK_CONF];
/// `STATUS` bits [9:0] `RXFIFO_CNT` and [25:16] `TXFIFO_CNT`.
const FIFO_COUNTS: u32 = 0x03FF_03FF;

#[derive(Clone, Debug)]
pub struct Uart {
    /// `STATUS` with both FIFO counts at zero: the line-state bits keep their reset value.
    idle_status: u32,
    config: Storage,
    tx: Vec<u8>,
}

impl Uart {
    #[must_use]
    pub fn new(base: u32) -> Uart {
        let status_reset = svd_regs::esp32c3::lookup(base + STATUS).map_or(0, |r| r.reset_value);
        Uart {
            idle_status: status_reset & !FIFO_COUNTS,
            config: Storage::for_offsets(base, &CONFIG),
            tx: Vec::new(),
        }
    }

    /// The bytes transmitted since the last call.
    pub fn take_tx(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.tx)
    }
}

impl Peripheral for Uart {
    fn read(&mut self, offset: u32) -> Option<u32> {
        match offset {
            STATUS => Some(self.idle_status),
            _ => self.config.read(offset),
        }
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        match offset {
            FIFO => {
                self.tx.push(value.to_le_bytes()[0]);
                true
            }
            // No interrupt condition is modelled yet, so there is nothing to clear.
            INT_CLR => true,
            _ => self.config.write(offset, value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    const UART0: u32 = 0x6000_0000;

    #[test]
    fn bytes_written_to_the_fifo_come_out_in_order_and_only_once() {
        let mut uart = Uart::new(UART0);
        for byte in *b"ESP" {
            assert!(uart.write(0x00, u32::from(byte)));
        }
        assert_eq!(uart.take_tx(), b"ESP");
        assert_eq!(uart.take_tx(), []);
    }

    #[test]
    fn only_the_low_byte_of_a_fifo_write_is_transmitted() {
        let mut uart = Uart::new(UART0);
        uart.write(0x00, 0xFFFF_FF41);
        assert_eq!(uart.take_tx(), b"A");
    }

    #[test]
    fn transmit_fifo_always_reads_empty_so_the_rom_never_waits() {
        let mut uart = Uart::new(UART0);
        for _ in 0..200 {
            uart.write(0x00, 0x55);
        }
        let status = uart.read(0x1C).unwrap();
        assert_eq!((status >> 16) & 0x3FF, 0, "TXFIFO_CNT");
        assert_eq!(status & 0x3FF, 0, "RXFIFO_CNT");
    }

    #[test]
    fn line_configuration_is_storage_from_its_reset_value() {
        let mut uart = Uart::new(UART0);
        for offset in [0x0C, 0x14, 0x20, 0x24, 0x78] {
            let reset = svd_regs::esp32c3::lookup(UART0 + offset)
                .unwrap()
                .reset_value;
            assert_eq!(uart.read(offset), Some(reset), "{offset:#x}");
            assert!(uart.write(offset, 0x15B));
            assert_eq!(uart.read(offset), Some(0x15B));
        }
    }

    #[test]
    fn interrupt_clear_is_accepted_and_interrupt_status_is_not_modelled_yet() {
        let mut uart = Uart::new(UART0);
        assert!(uart.write(0x10, 0xFFFF_FFFF));
        assert_eq!(
            uart.read(0x04),
            None,
            "INT_RAW arrives with interrupts (SP1b-3)"
        );
        assert_eq!(
            uart.read(0x08),
            None,
            "INT_ST arrives with interrupts (SP1b-3)"
        );
    }

    #[test]
    fn second_uart_uses_its_own_reset_values() {
        let mut uart1 = Uart::new(0x6001_0000);
        let reset = svd_regs::esp32c3::lookup(0x6001_0014).unwrap().reset_value;
        assert_eq!(uart1.read(0x14), Some(reset));
    }
}
