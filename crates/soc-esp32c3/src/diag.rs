//! What the emulator has to tell the user about the run: accesses it does not model, CPU
//! exceptions, resets.
use crate::flash::{FlashFrame, FlashRefusal};
use crate::periph::mmu::MMU_PAGE_BYTES;
use crate::reset::ResetCause;
use cpu_rv32::Exception;
use std::collections::BTreeMap;
use std::fmt;

/// Past this many distinct diagnostics, further new ones are counted (see [`Diagnostics::suppressed`])
/// but no longer listed: a runaway program must not grow the log without bound.
pub const MAX_DISTINCT_DIAGNOSTICS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Direction {
    Read,
    Write,
}

/// An operation the emulator cannot perform faithfully: it is reported, and `--strict-mmio` stops on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsupported {
    /// An SPI flash command whose opcode the flash model does not implement.
    FlashCommand { opcode: u8 },
    /// A flash command the model knows, in a frame it does not: see
    /// [`FlashRefusal::Form`](crate::flash::FlashRefusal::Form).
    FlashCommandForm {
        opcode: u8,
        address_bits: u32,
        dummy_cycles: u32,
        data_bytes: u32,
    },
    /// A flash command that addresses a byte past the end of the chip.
    FlashRange { opcode: u8, address: u32, len: u32 },
    /// A program or an erase while a block-protect bit of the flash status register is set.
    FlashProtected { opcode: u8, status: u8 },
    /// An SPI1 user transfer whose command phase is not the 8-bit one the controller model
    /// implements: no command phase at all (`None`), or a command of `command_bits` bits.
    FlashTransfer {
        command_bits: Option<u32>,
        user: u32,
        user2: u32,
    },
    /// An SPI1 transfer whose data phase the controller model cannot carry: more than the 512 bits
    /// `W0..W15` hold, or a number of bits that is not a whole number of bytes. `mosi_bits` and
    /// `miso_bits` are the lengths asked for, 0 for a phase the transfer does not have (a phase
    /// has 1 to 1024 bits).
    FlashTransferLength { mosi_bits: u32, miso_bits: u32 },
    /// A hard-wired page program (`SPI1.CMD` bit `FLASH_PP`) whose byte count, `ADDR[31:24]`, is 0
    /// or more than the 64 bytes `W0..W15` hold; `addr` is the whole register.
    FlashProgramLength { addr: u32 },
    /// One write of `SPI1.CMD` with more than one command bit set; `cmd` is the value written.
    /// Listed once for the run, with the first value: see the deduplication key.
    FlashCommandBits { cmd: u32 },
    /// A write of `SPI1.FLASH_SUS_CTRL` that sends a suspend or a resume, or enables the automatic
    /// ones; `sus_ctrl` is the value written.
    FlashSuspend { sus_ctrl: u32 },
    /// A write to `SHA.DMA_BLOCK_NUM`, `DMA_START` or `DMA_CONTINUE` (D7).
    ShaDma,
    /// A `SHA.MODE` the ESP32-C3 accelerator does not have.
    ShaMode { mode: u32 },
    /// A clock selection without a known CPU frequency (D22).
    ClockSource { sysclk_conf: u32, cpu_per_conf: u32 },
    /// `RTC_CNTL.CLK_CONF` selects a slow clock other than the RC oscillator, the only one modelled.
    SlowClock { clk_conf: u32 },
    /// A byte written to a UART whose transmit FIFO was full.
    /// ASSUMPTION: the byte is dropped. No source says what the silicon does with such a write;
    /// writing 129 bytes or more without waiting on `TXFIFO_CNT` on a real chip and reading what
    /// comes out would settle it.
    UartOverflow { uart: u8 },
    /// A byte sent by a UART whose `CLK_CONF.SCLK_SEL` selects a source clock the model has no
    /// frequency for: the RC fast clock (2), or no documented source (0). The byte is timed with
    /// the crystal.
    UartClock { uart: u8, sclk_sel: u32 },
    /// A read of the receiver of a UART (`FIFO`, `STATUS`, `INT_RAW`, `INT_ST`) after a byte of
    /// the host started, or a receive timeout came, while `CLK_CONF.SCLK_SEL` selected a source
    /// clock the model has no frequency for: they were timed with the crystal. Refused at the read,
    /// an instant the firmware fixes, rather than at the start bit, which any call may reach.
    UartReceiveClock { uart: u8, sclk_sel: u32 },
    /// A UART field set to a mode the model does not have (report UR §10): the register keeps
    /// the value, and the UART goes on as if the field were clear.
    UartFeature { uart: u8, feature: UartFeature },
    /// A byte of the host that found the 128-byte receive FIFO of a UART full: dropped.
    /// ASSUMPTION: the new byte is lost and the 128 stay (report UR A2); 200 bytes sent to a FIFO
    /// nobody reads, on a chip, settle it.
    UartRxOverflow { uart: u8 },
    /// A read of the receive FIFO of a UART while it was empty: 0 was returned (report UR A6).
    UartEmptyRead { uart: u8 },
    /// A read of `USB_DEVICE.EP1` while the OUT FIFO was empty: 0 was returned (report USJ O.6).
    UsbEmptyRead,
    /// A write of `USB_DEVICE.EP1` while the IN endpoint had no room: the byte was ignored
    /// (report USJ O.6).
    UsbInFull,
}

/// A UART mode the model refuses by name when software sets it (report UR §10, A4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UartFeature {
    /// `CONF0.LOOPBACK`: the transmitter drives the receiver.
    Loopback,
    /// `CONF0.RXD_INV`: the receive line is inverted.
    InvertedRx,
    /// `CONF0.AUTOBAUD_EN`: the baud rate is detected from the line.
    Autobaud,
    /// `CONF0.STOP_BIT_NUM` = 0: a frame with no documented stop bit (report UR A4).
    NoStopBit,
    /// `MEM_CONF.RX_SIZE` or `TX_SIZE` other than one 128-byte block.
    FifoSize,
    /// `RS485_CONF` other than 0.
    Rs485,
}

impl UartFeature {
    /// The register field, then what it asks for, then what the model does instead and the
    /// remedy, as the message says them.
    const fn describe(self) -> (&'static str, &'static str, &'static str) {
        match self {
            UartFeature::Loopback => (
                "CONF0.LOOPBACK",
                "to loop its transmitter back to its receiver",
                "its bytes still go to the host and its receiver hears only the host; clear CONF0.LOOPBACK",
            ),
            UartFeature::InvertedRx => (
                "CONF0.RXD_INV",
                "to invert its receive line",
                "the host's bytes are received as they were sent; clear CONF0.RXD_INV",
            ),
            UartFeature::Autobaud => (
                "CONF0.AUTOBAUD_EN",
                "to detect its baud rate from the line",
                "the baud rate stays the one CLKDIV sets; set the baud rate with CLKDIV",
            ),
            UartFeature::NoStopBit => (
                "CONF0.STOP_BIT_NUM = 0",
                "for a frame without a stop bit",
                "its bytes are timed with one stop bit; write STOP_BIT_NUM 1, 2 or 3 (1, 1.5 or 2 stop bits)",
            ),
            UartFeature::FifoSize => (
                "MEM_CONF.RX_SIZE or TX_SIZE other than 1",
                "to give its FIFOs another share of the UART memory",
                "each FIFO keeps 128 bytes; leave RX_SIZE and TX_SIZE at 1",
            ),
            UartFeature::Rs485 => (
                "RS485_CONF other than 0",
                "to RS485 mode",
                "it sends and receives as a plain UART; write RS485_CONF = 0",
            ),
        }
    }
}

impl From<FlashRefusal> for Unsupported {
    fn from(refusal: FlashRefusal) -> Unsupported {
        match refusal {
            FlashRefusal::Unmodeled { opcode } => Unsupported::FlashCommand { opcode },
            FlashRefusal::Form {
                opcode,
                address_bits,
                dummy_cycles,
                data_bytes,
            } => Unsupported::FlashCommandForm {
                opcode,
                address_bits,
                dummy_cycles,
                data_bytes,
            },
            FlashRefusal::OutOfRange {
                opcode,
                address,
                len,
            } => Unsupported::FlashRange {
                opcode,
                address,
                len,
            },
            FlashRefusal::Protected { opcode, status } => {
                Unsupported::FlashProtected { opcode, status }
            }
        }
    }
}

impl Unsupported {
    /// The message of the diagnostic: what the instruction at `pc` asked for and why it was not
    /// done, then what the user can do.
    fn describe(self, pc: u32, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unsupported::FlashCommand { opcode } => write!(
                f,
                "flash command {opcode:#04x} is not modeled (pc {pc:#010x}): the virtual flash knows write enable and disable (0x06, 0x04), the status read and write (0x05, 0x01), the identification (0x9f), the SFDP read (0x5a), release from power-down (0xab), resume (0x7a), the data reads (0x03, 0x0b, 0x3b, 0xbb, 0x6b, 0xeb), the page program (0x02) and the erases (0x20, 0xd8, 0x60, 0xc7), so nothing was read or changed and SPI1.W0..W15 keep the bytes they held; use firmware that sends only these flash commands"
            ),
            Unsupported::FlashCommandForm {
                opcode,
                address_bits,
                dummy_cycles,
                data_bytes,
            } => {
                write!(
                    f,
                    "flash command {opcode:#04x} was sent in a form the virtual flash does not know ({address_bits} address bits, {dummy_cycles} dummy clocks, {data_bytes} bytes of data out, pc {pc:#010x}) and what a chip makes of it is not modeled, so nothing was read or changed and SPI1.W0..W15 keep the bytes they held; "
                )?;
                // The chip refuses the form of a command it knows, so the command has a frame;
                // a value built by hand for another opcode gets the general remedy.
                match FlashFrame::of(opcode) {
                    Some(frame) => write!(f, "send {opcode:#04x} with {frame}"),
                    None => write!(
                        f,
                        "send the command with the address, the dummy clocks and the data its datasheet gives it"
                    ),
                }
            }
            Unsupported::FlashRange {
                opcode,
                address,
                len,
            } => write!(
                f,
                "flash command {opcode:#04x} addresses {len} bytes at {address:#08x}, past the end of the flash of the device (pc {pc:#010x}): a chip wraps there or ignores the address bits it lacks, and inkemu does not guess which, so nothing was read or changed and SPI1.W0..W15 keep the bytes they held; keep flash accesses inside the flash size of the device profile"
            ),
            Unsupported::FlashProtected { opcode, status } => write!(
                f,
                "flash command {opcode:#04x} would program or erase while block-protect bits of the flash status register are set (status {status:#04x}, pc {pc:#010x}): which blocks they protect depends on the flash part and is not modeled, so nothing was changed; clear the block-protect bits (bits 5:2 of the status register) with a status write that keeps its other bits, before the program or the erase"
            ),
            Unsupported::FlashTransfer {
                command_bits,
                user,
                user2,
            } => {
                write!(f, "SPI1 was asked for a flash transfer ")?;
                match command_bits {
                    Some(bits) => write!(f, "with a {bits}-bit command")?,
                    None => write!(f, "without a command phase")?,
                }
                write!(
                    f,
                    " (USER {user:#010x}, USER2 {user2:#010x}, pc {pc:#010x}) but only transfers with an 8-bit command are modeled, so nothing was sent to the flash; use firmware whose flash commands are 8 bits long until other transfers are emulated"
                )
            }
            Unsupported::FlashTransferLength {
                mosi_bits,
                miso_bits,
            } => write!(
                f,
                "SPI1 was asked for a flash transfer with a data phase of {mosi_bits} bits out and {miso_bits} bits in (pc {pc:#010x}) but only whole bytes are modeled, 64 at most, what W0..W15 hold, so nothing was sent to the flash and SPI1.W0..W15 keep the bytes they held; give MOSI_DLEN and MISO_DLEN the value 8 × bytes − 1 for 1 to 64 bytes"
            ),
            Unsupported::FlashProgramLength { addr } => write!(
                f,
                "SPI1 was asked to program {len} bytes with one FLASH_PP command (ADDR {addr:#010x}, pc {pc:#010x}) but one command carries 1 to 64 bytes, what W0..W15 hold, so nothing was sent to the flash; put the byte count, 1 to 64, in ADDR[31:24]",
                len = addr >> 24
            ),
            Unsupported::FlashCommandBits { cmd } => write!(
                f,
                "SPI1.CMD was written with several command bits at once (value {cmd:#010x}, pc {pc:#010x}) and the order in which the controller would run them is not documented, so none was run; start one flash command per write of SPI1.CMD"
            ),
            Unsupported::FlashSuspend { sus_ctrl } => write!(
                f,
                "SPI1.FLASH_SUS_CTRL asks for a program/erase suspend or resume (value {sus_ctrl:#010x}, pc {pc:#010x}) but a flash operation completes at once here and is never suspended, so nothing was sent to the flash; build the firmware without flash suspend: no auto-suspend (CONFIG_SPI_FLASH_AUTO_SUSPEND=n, the ESP-IDF default) and no suspend or resume sent by software (FLASH_PES and FLASH_PER, which ESP-IDF writes only with CONFIG_SPI_FLASH_SOFTWARE_RESUME)"
            ),
            Unsupported::ShaDma => write!(
                f,
                "SHA.DMA_BLOCK_NUM, DMA_START or DMA_CONTINUE was written (pc {pc:#010x}) but only the typical SHA mode is modeled, so DMA-SHA produces no digest; use firmware that hashes through SHA.START/CONTINUE until DMA-SHA is emulated"
            ),
            Unsupported::ShaMode { mode } => write!(
                f,
                "SHA.MODE {mode} selects an algorithm the ESP32-C3 accelerator does not have (pc {pc:#010x}), so the block was not hashed; use SHA-1 (0), SHA-224 (1) or SHA-256 (2)"
            ),
            Unsupported::ClockSource {
                sysclk_conf,
                cpu_per_conf,
            } => write!(
                f,
                "the clock registers select a CPU clock inkemu cannot model (SYSCLK_CONF {sysclk_conf:#010x}, CPU_PER_CONF {cpu_per_conf:#010x}, pc {pc:#010x}), so emulated time keeps the previous frequency; select the crystal or the PLL at 80 or 160 MHz"
            ),
            Unsupported::SlowClock { clk_conf } => write!(
                f,
                "RTC_CNTL.CLK_CONF selects an RTC slow clock inkemu does not model (value {clk_conf:#010x}, pc {pc:#010x}), so the RTC timer and the RTC watchdog keep counting the 136 kHz RC oscillator; select the RC oscillator (ANA_CLK_RTC_SEL = 0) or expect their durations to differ from the device"
            ),
            Unsupported::UartOverflow { .. }
            | Unsupported::UartClock { .. }
            | Unsupported::UartReceiveClock { .. }
            | Unsupported::UartFeature { .. }
            | Unsupported::UartRxOverflow { .. }
            | Unsupported::UartEmptyRead { .. }
            | Unsupported::UsbEmptyRead
            | Unsupported::UsbInFull => self.describe_serial(pc, f),
        }
    }

    /// The messages of the refusals of the serial ports, for [`Unsupported::describe`].
    fn describe_serial(self, pc: u32, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unsupported::UartOverflow { uart } => write!(
                f,
                "a byte was written to UART{uart} while its 128-byte transmit FIFO was full (pc {pc:#010x}), so inkemu dropped the byte; wait for room in the FIFO (UART.STATUS TXFIFO_CNT) before writing"
            ),
            Unsupported::UartClock { uart, sclk_sel } => write!(
                f,
                "UART{uart} sent a byte with a source clock inkemu does not model (CLK_CONF.SCLK_SEL = {sclk_sel}, pc {pc:#010x}), so the byte was timed with the 40 MHz crystal and its duration differs from the device; select the crystal (SCLK_SEL = 3) or APB_CLK (SCLK_SEL = 1)"
            ),
            Unsupported::UartReceiveClock { uart, sclk_sel } => write!(
                f,
                "UART{uart} received from the host, or counted its receive timeout, with a source clock inkemu does not model (CLK_CONF.SCLK_SEL = {sclk_sel}, read at pc {pc:#010x}), so the bit times were those of the 40 MHz crystal and what the read shows differs from the device; select the crystal (SCLK_SEL = 3) or APB_CLK (SCLK_SEL = 1)"
            ),
            Unsupported::UartFeature { uart, feature } => {
                let (field, asked, instead) = feature.describe();
                write!(
                    f,
                    "UART{uart} was set {asked} ({field}, pc {pc:#010x}), which inkemu does not model, so {instead}"
                )
            }
            Unsupported::UartRxOverflow { uart } => write!(
                f,
                "a byte the host sent found the 128-byte receive FIFO of UART{uart} full (pc {pc:#010x}), so it was dropped and the 128 bytes stay; read the FIFO sooner, or send less at once"
            ),
            Unsupported::UartEmptyRead { uart } => write!(
                f,
                "UART{uart}.FIFO was read while the receive FIFO was empty (pc {pc:#010x}), so 0 was returned; read only the number of bytes UART.STATUS RXFIFO_CNT gives"
            ),
            Unsupported::UsbEmptyRead => write!(
                f,
                "USB_DEVICE.EP1 was read while the OUT FIFO was empty (pc {pc:#010x}), so 0 was returned; read EP1 only while EP1_CONF.SERIAL_OUT_EP_DATA_AVAIL is set"
            ),
            Unsupported::UsbInFull => write!(
                f,
                "a byte was written to USB_DEVICE.EP1 while the IN endpoint had no room (pc {pc:#010x}), so it was ignored; write EP1 only while EP1_CONF.SERIAL_IN_EP_DATA_FREE is set"
            ),
            // INVARIANT: `describe` hands only the refusals above to this function.
            _ => Ok(()),
        }
    }
}

/// Why an access through the flash MMU could not be served from the flash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappingFault {
    /// Bit 8 of the MMU entry is set.
    InvalidEntry,
    /// The entry names a page past the end of the flash.
    BeyondFlash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Diagnostic {
    /// Spec §5.2: `value` is what the read returned, or what the ignored write carried.
    UnmodeledMmio {
        addr: u32,
        direction: Direction,
        pc: u32,
        register: Option<(&'static str, &'static str)>,
        value: u32,
        reset_explicit: bool,
    },
    /// An 8- or 16-bit store to a peripheral register (decision D9 of the SP1b-1 plan).
    SubWordWrite { addr: u32, bytes: u32, pc: u32 },
    /// An operation the emulator cannot perform faithfully.
    Unsupported { what: Unsupported, pc: u32 },
    /// An access to a flash window that no valid MMU entry backs.
    FlashMapping {
        vaddr: u32,
        entry: u32,
        content: u32,
        fault: MappingFault,
        pc: u32,
    },
    /// A read through a flash window that a device may answer with other bytes. The emulator
    /// reads the flash at every access; a device reads its cache, which keeps the bytes a 64 KiB
    /// flash `page` held before SPI1 programmed or erased it, until software invalidates it
    /// (report CM §3). The read returned the bytes the flash holds now.
    StaleFlashWindow { vaddr: u32, page: u32, pc: u32 },
    /// A synchronous exception the CPU took. It is no error of the emulator: the firmware's trap
    /// handler runs, as on the device.
    Exception {
        cause: Exception,
        tval: u32,
        pc: u32,
        /// The stack pointer when the instruction started. An exception is listed once per cause
        /// and PC: when the same instruction faults again with another stack pointer, the entry
        /// that is shown keeps the stack pointer of the first occurrence.
        sp: u32,
    },
    /// The chip was reset while it ran, by a watchdog or at the firmware's request: the entry of
    /// the journal spec §7 asks for, with the program counter and the stack. It is no fault of
    /// the emulator and none it can judge of the firmware (`esp_restart` ends with one): it never
    /// stops a strict run, and a front end lists it as an event of the run, not as a warning.
    Reset {
        cause: ResetCause,
        /// The instruction the reset kept from executing: where a watchdog caught the program,
        /// or the instruction after the store that asked for the reset.
        pc: u32,
        /// The stack pointer at that instant. A reset is listed once per cause and PC: when the
        /// same one comes again with another stack pointer (a boot loop), the entry that is shown
        /// keeps the stack pointer of the first occurrence.
        sp: u32,
    },
    /// Emitted once when [`MAX_DISTINCT_DIAGNOSTICS`] is reached.
    Suppressed,
}

/// Deduplication key: what the diagnostic is about, not where it was first seen.
type Key = (u8, u32, u32);

impl Diagnostic {
    fn key(&self) -> Key {
        match *self {
            Diagnostic::UnmodeledMmio {
                addr,
                direction: Direction::Read,
                ..
            } => (0, addr, 0),
            Diagnostic::UnmodeledMmio {
                addr,
                direction: Direction::Write,
                ..
            } => (1, addr, 0),
            Diagnostic::SubWordWrite { addr, bytes, .. } => (2, addr, bytes),
            Diagnostic::Exception { cause, pc, .. } => (3, pc, cause.code()),
            Diagnostic::Suppressed => (4, 0, 0),
            Diagnostic::Unsupported { what, .. } => match what {
                Unsupported::FlashCommand { opcode } => (5, u32::from(opcode), 0),
                Unsupported::ShaDma => (6, 0, 0),
                Unsupported::ShaMode { mode } => (7, mode, 0),
                Unsupported::ClockSource {
                    sysclk_conf,
                    cpu_per_conf,
                } => (8, sysclk_conf, cpu_per_conf),
                Unsupported::SlowClock { .. } => (9, 0, 0),
                Unsupported::UartOverflow { uart } => (12, u32::from(uart), 0),
                Unsupported::UartClock { uart, sclk_sel } => (13, u32::from(uart), sclk_sel),
                Unsupported::UartReceiveClock { uart, sclk_sel } => (28, u32::from(uart), sclk_sel),
                Unsupported::UartFeature { uart, feature } => (23, u32::from(uart), feature as u32),
                Unsupported::UartRxOverflow { uart } => (24, u32::from(uart), 0),
                Unsupported::UartEmptyRead { uart } => (25, u32::from(uart), 0),
                Unsupported::UsbEmptyRead => (26, 0, 0),
                Unsupported::UsbInFull => (27, 0, 0),
                // A command is 1 to 16 bits long, so 0 is free for "no command phase".
                Unsupported::FlashTransfer { command_bits, .. } => {
                    (11, command_bits.unwrap_or(0), 0)
                }
                // One line for every length: each of the two can take 1024 values.
                Unsupported::FlashTransferLength { .. } => (22, 0, 0),
                // One line per opcode for each kind of refusal of the chip.
                Unsupported::FlashCommandForm { opcode, .. } => (18, u32::from(opcode), 0),
                Unsupported::FlashRange { opcode, .. } => (19, u32::from(opcode), 0),
                Unsupported::FlashProtected { opcode, .. } => (20, u32::from(opcode), 0),
                // One line per byte count, and one line each for suspend requests and for writes
                // of several command bits: the 14 command bits alone make four times more sets
                // than the table has entries, and the bits below them are no command at all. A
                // key holds nothing the firmware can vary further than a byte: a firmware that
                // is refused in a loop with other values each time must not fill the table and
                // so hide every later diagnostic of the run.
                Unsupported::FlashProgramLength { addr } => (15, addr >> 24, 0),
                Unsupported::FlashCommandBits { .. } => (16, 0, 0),
                Unsupported::FlashSuspend { .. } => (17, 0, 0),
            },
            Diagnostic::FlashMapping { entry, fault, .. } => (10, entry, fault as u32),
            // One line per flash page: the firmware reads the page again at many addresses.
            Diagnostic::StaleFlashWindow { page, .. } => (21, page, 0),
            Diagnostic::Reset { cause, pc, .. } => (14, pc, cause.code()),
        }
    }

    /// Whether `--strict-mmio` stops the run on this diagnostic. A CPU exception and a reset do
    /// not: both are what the firmware does on the device.
    const fn stops_a_strict_run(&self) -> bool {
        matches!(
            self,
            Diagnostic::UnmodeledMmio { .. }
                | Diagnostic::SubWordWrite { .. }
                | Diagnostic::Unsupported { .. }
                | Diagnostic::FlashMapping { .. }
                | Diagnostic::StaleFlashWindow { .. }
        )
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Diagnostic::UnmodeledMmio {
                addr,
                direction,
                pc,
                register,
                value,
                reset_explicit,
            } => {
                let what = match direction {
                    Direction::Read => "read",
                    Direction::Write => "write",
                };
                match register {
                    Some((peripheral, name)) => write!(
                        f,
                        "unmodeled register {what}: {peripheral}.{name} at {addr:#010x} (pc {pc:#010x}); "
                    )?,
                    None => write!(
                        f,
                        "unmodeled {what} at {addr:#010x} (pc {pc:#010x}): no register is documented there; "
                    )?,
                }
                match (direction, register.is_some() && reset_explicit) {
                    (Direction::Read, true) => {
                        write!(f, "returning its documented reset value {value:#010x}")
                    }
                    (Direction::Read, false) => {
                        write!(f, "returning 0, the SVD documents no reset value")
                    }
                    (Direction::Write, _) => write!(f, "value {value:#010x} ignored"),
                }
            }
            Diagnostic::SubWordWrite { addr, bytes, pc } => write!(
                f,
                "{bytes}-byte write to a 32-bit peripheral register at {addr:#010x} (pc {pc:#010x}); the whole register was written, its other bytes as zero"
            ),
            Diagnostic::Unsupported { what, pc } => what.describe(pc, f),
            Diagnostic::FlashMapping {
                vaddr,
                entry,
                content,
                fault,
                pc,
            } => match fault {
                MappingFault::InvalidEntry => write!(
                    f,
                    "access to the flash window at {vaddr:#010x} through MMU entry {entry}, which is invalid (content {content:#05x}, pc {pc:#010x}): 0 was returned; the firmware used flash it had not mapped"
                ),
                MappingFault::BeyondFlash => write!(
                    f,
                    "access to the flash window at {vaddr:#010x} through MMU entry {entry} (content {content:#05x}, pc {pc:#010x}), which names a page beyond the end of the flash: 0 was returned; check the flash size of the device profile"
                ),
            },
            Diagnostic::StaleFlashWindow { vaddr, page, pc } => {
                // 64 bits: `page` is a number the firmware chose through an MMU entry.
                let start = u64::from(page) * u64::from(MMU_PAGE_BYTES);
                let end = start + u64::from(MMU_PAGE_BYTES) - 1;
                write!(
                    f,
                    "read of the flash window at {vaddr:#010x} (pc {pc:#010x}) may differ from the device: flash {start:#08x} to {end:#08x} was read through a window, then programmed or erased, and the cache was not invalidated for it since; the bytes the flash holds now were returned, where a device may still return those of before; invalidate the cache at the window addresses that map that flash after the write, as ESP-IDF's esp_flash and esp_partition functions do for mapped flash"
                )
            }
            Diagnostic::Exception {
                cause,
                tval,
                pc,
                sp,
            } => write!(
                f,
                "CPU exception: {cause} at pc {pc:#010x} (tval {tval:#010x}, sp {sp:#010x}); the firmware's trap handler takes it, as on the device"
            ),
            // The code as the ROM prints it at the next boot (`rst:0x7`), so that the two lines
            // of one reset can be matched by eye.
            Diagnostic::Reset { cause, pc, sp } => write!(
                f,
                "reset by {cause} (rst:{code:#x}) at pc {pc:#010x}, sp {sp:#010x}",
                code = cause.code()
            ),
            Diagnostic::Suppressed => write!(
                f,
                "more than {MAX_DISTINCT_DIAGNOSTICS} distinct diagnostics; further new ones are counted but not listed"
            ),
        }
    }
}

#[derive(Debug)]
pub struct Diagnostics {
    strict: bool,
    /// The first diagnostic of each key and its number of occurrences. A `BTreeMap` keeps every
    /// observable order deterministic.
    seen: BTreeMap<Key, (Diagnostic, u64)>,
    pending: Vec<Diagnostic>,
    violation: Option<Diagnostic>,
    capped: bool,
    /// Occurrences dropped because [`MAX_DISTINCT_DIAGNOSTICS`] distinct ones were already known.
    suppressed: u64,
}

impl Diagnostics {
    #[must_use]
    pub fn new(strict: bool) -> Diagnostics {
        Diagnostics {
            strict,
            seen: BTreeMap::new(),
            pending: Vec::new(),
            violation: None,
            capped: false,
            suppressed: 0,
        }
    }

    pub fn record(&mut self, diagnostic: Diagnostic) {
        if self.strict && self.violation.is_none() && diagnostic.stops_a_strict_run() {
            self.violation = Some(diagnostic.clone());
        }
        let key = diagnostic.key();
        if let Some((_, count)) = self.seen.get_mut(&key) {
            *count += 1;
            return;
        }
        if self.seen.len() >= MAX_DISTINCT_DIAGNOSTICS {
            self.suppressed += 1;
            if !std::mem::replace(&mut self.capped, true) {
                self.pending.push(Diagnostic::Suppressed);
            }
            return;
        }
        self.seen.insert(key, (diagnostic.clone(), 1));
        self.pending.push(diagnostic);
    }

    /// Diagnostics seen for the first time since the last call, in order of first occurrence.
    pub fn drain(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.pending)
    }

    /// In strict mode, the first violation; `None` otherwise or once taken. A violation is the first
    /// unmodeled access, sub-word register write, unsupported operation, invalid flash mapping or
    /// window read that may differ from the device.
    pub fn take_violation(&mut self) -> Option<Diagnostic> {
        self.violation.take()
    }

    #[must_use]
    pub fn occurrences(&self, diagnostic: &Diagnostic) -> u64 {
        self.seen
            .get(&diagnostic.key())
            .map_or(0, |&(_, count)| count)
    }

    /// How many occurrences were dropped because [`MAX_DISTINCT_DIAGNOSTICS`] distinct diagnostics were
    /// already known: none of them is listed, and each is counted here.
    #[must_use]
    pub const fn suppressed(&self) -> u64 {
        self.suppressed
    }

    /// At most `limit` diagnostics that occurred more than once, the most frequent first (ties in key
    /// order, so the result is deterministic), each with its number of occurrences.
    #[must_use]
    pub fn most_repeated(&self, limit: usize) -> Vec<(Diagnostic, u64)> {
        let mut repeated: Vec<(Diagnostic, u64)> = self
            .seen
            .values()
            .filter(|(_, count)| *count > 1)
            .cloned()
            .collect();
        // The map iterates in key order and the sort is stable: equal counts keep key order.
        repeated.sort_by_key(|&(_, count)| std::cmp::Reverse(count));
        repeated.truncate(limit);
        repeated
    }

    #[must_use]
    pub fn distinct(&self) -> usize {
        self.seen.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_at(addr: u32, pc: u32) -> Diagnostic {
        Diagnostic::UnmodeledMmio {
            addr,
            direction: Direction::Read,
            pc,
            register: Some(("LEDC", "CONF")),
            value: 7,
            reset_explicit: true,
        }
    }

    #[test]
    fn repeated_diagnostic_is_listed_once_and_counted() {
        let mut d = Diagnostics::new(false);
        for pc in 0..1000 {
            d.record(read_at(0x6001_9000, pc)); // the PC differs, the register does not
        }
        let listed = d.drain();
        assert_eq!(listed.len(), 1);
        assert_eq!(d.occurrences(&listed[0]), 1000);
        assert!(d.drain().is_empty(), "draining twice lists nothing new");
    }

    #[test]
    fn read_and_write_of_the_same_register_are_two_diagnostics() {
        let mut d = Diagnostics::new(false);
        d.record(read_at(0x6001_9000, 0));
        d.record(Diagnostic::UnmodeledMmio {
            addr: 0x6001_9000,
            direction: Direction::Write,
            pc: 0,
            register: None,
            value: 1,
            reset_explicit: false,
        });
        assert_eq!(d.drain().len(), 2);
    }

    #[test]
    fn distinct_diagnostics_are_capped() {
        let mut d = Diagnostics::new(false);
        for i in 0..u32::try_from(MAX_DISTINCT_DIAGNOSTICS).unwrap() + 10 {
            d.record(Diagnostic::SubWordWrite {
                addr: 0x6000_0000 + 4 * i,
                bytes: 1,
                pc: 0,
            });
        }
        let listed = d.drain();
        assert_eq!(d.distinct(), MAX_DISTINCT_DIAGNOSTICS);
        assert_eq!(listed.len(), MAX_DISTINCT_DIAGNOSTICS + 1);
        assert_eq!(listed.last(), Some(&Diagnostic::Suppressed));
    }

    fn sub_word_write_at(index: u32) -> Diagnostic {
        Diagnostic::SubWordWrite {
            addr: 0x6000_0000 + 4 * index,
            bytes: 1,
            pc: 0,
        }
    }

    #[test]
    fn occurrences_past_the_cap_are_counted_as_suppressed() {
        let mut d = Diagnostics::new(false);
        let cap = u32::try_from(MAX_DISTINCT_DIAGNOSTICS).unwrap();
        for i in 0..cap + 10 {
            d.record(sub_word_write_at(i));
        }
        assert_eq!(d.suppressed(), 10);
        d.record(sub_word_write_at(cap + 3)); // one more occurrence of a dropped diagnostic
        assert_eq!(d.suppressed(), 11);
        d.record(sub_word_write_at(0)); // a listed diagnostic is counted as such, not as suppressed
        assert_eq!(d.suppressed(), 11);
        assert_eq!(d.occurrences(&sub_word_write_at(0)), 2);
    }

    #[test]
    fn most_repeated_lists_the_frequent_ones_first_and_skips_the_single_ones() {
        let mut d = Diagnostics::new(false);
        let times = |d: &mut Diagnostics, index: u32, n: u32| {
            for _ in 0..n {
                d.record(sub_word_write_at(index));
            }
        };
        times(&mut d, 7, 1); // seen once: never listed
        times(&mut d, 3, 5);
        times(&mut d, 1, 9);
        times(&mut d, 2, 5); // ties with index 3: key order puts the lower address first
        let counts: Vec<(u32, u64)> = d
            .most_repeated(10)
            .into_iter()
            .map(|(diagnostic, count)| match diagnostic {
                Diagnostic::SubWordWrite { addr, .. } => ((addr - 0x6000_0000) / 4, count),
                other => panic!("unexpected {other}"),
            })
            .collect();
        assert_eq!(counts, [(1, 9), (2, 5), (3, 5)]);
        assert_eq!(d.most_repeated(2).len(), 2, "the limit applies");
        assert_eq!(Diagnostics::new(false).most_repeated(3).len(), 0);
    }

    #[test]
    fn writes_of_different_widths_to_one_address_are_two_diagnostics() {
        let mut d = Diagnostics::new(false);
        for bytes in [1, 2] {
            d.record(Diagnostic::SubWordWrite {
                addr: 0x6000_9054,
                bytes,
                pc: 0,
            });
        }
        assert_eq!(d.drain().len(), 2);
    }

    #[test]
    fn strict_mode_reports_the_first_mmio_diagnostic_as_a_violation() {
        let mut strict = Diagnostics::new(true);
        strict.record(Diagnostic::Exception {
            cause: cpu_rv32::Exception::IllegalInstr,
            tval: 0,
            pc: 4,
            sp: 0,
        });
        assert_eq!(
            strict.take_violation(),
            None,
            "a CPU exception is not an MMIO violation"
        );
        strict.record(read_at(0x6001_9000, 8));
        strict.record(read_at(0x6001_9004, 12));
        assert_eq!(strict.take_violation(), Some(read_at(0x6001_9000, 8)));
        assert_eq!(strict.take_violation(), None);
        let mut lenient = Diagnostics::new(false);
        lenient.record(read_at(0x6001_9000, 8));
        assert_eq!(lenient.take_violation(), None);
    }

    fn unsupported(what: Unsupported) -> Diagnostic {
        Diagnostic::Unsupported {
            what,
            pc: 0x4038_1000,
        }
    }

    fn mapping(entry: u32, fault: MappingFault) -> Diagnostic {
        Diagnostic::FlashMapping {
            vaddr: 0x3C00_0000 + (entry << 16),
            entry,
            content: 0x100,
            fault,
            pc: 0x4004_a186,
        }
    }

    /// The three refusals of the flash chip that are not an unknown opcode.
    const FORM: Unsupported = Unsupported::FlashCommandForm {
        opcode: 0x02,
        address_bits: 32,
        dummy_cycles: 0,
        data_bytes: 4,
    };
    const RANGE: Unsupported = Unsupported::FlashRange {
        opcode: 0x20,
        address: 0x10_0000,
        len: 0x1000,
    };
    const PROTECTED: Unsupported = Unsupported::FlashProtected {
        opcode: 0xD8,
        status: 0x3E,
    };

    #[test]
    fn a_refusal_of_the_flash_chip_becomes_the_unsupported_operation_of_the_same_kind() {
        use crate::flash::FlashRefusal;
        assert_eq!(
            Unsupported::from(FlashRefusal::Unmodeled { opcode: 0x35 }),
            Unsupported::FlashCommand { opcode: 0x35 }
        );
        assert_eq!(
            Unsupported::from(FlashRefusal::Form {
                opcode: 0x02,
                address_bits: 32,
                dummy_cycles: 0,
                data_bytes: 4
            }),
            FORM
        );
        assert_eq!(
            Unsupported::from(FlashRefusal::OutOfRange {
                opcode: 0x20,
                address: 0x10_0000,
                len: 0x1000
            }),
            RANGE
        );
        assert_eq!(
            Unsupported::from(FlashRefusal::Protected {
                opcode: 0xD8,
                status: 0x3E
            }),
            PROTECTED
        );
    }

    #[test]
    fn strict_mode_stops_on_the_events_that_cannot_be_right_silently() {
        for diagnostic in [
            unsupported(Unsupported::FlashCommand { opcode: 0x02 }),
            unsupported(Unsupported::ShaDma),
            unsupported(Unsupported::ShaMode { mode: 5 }),
            unsupported(Unsupported::ClockSource {
                sysclk_conf: 0xC00,
                cpu_per_conf: 0xC,
            }),
            unsupported(Unsupported::SlowClock {
                clk_conf: 0x4000_0000,
            }),
            unsupported(Unsupported::UartOverflow { uart: 0 }),
            unsupported(Unsupported::UartClock {
                uart: 1,
                sclk_sel: 2,
            }),
            unsupported(Unsupported::UartReceiveClock {
                uart: 0,
                sclk_sel: 0,
            }),
            unsupported(Unsupported::UartFeature {
                uart: 0,
                feature: UartFeature::Loopback,
            }),
            unsupported(Unsupported::FlashProgramLength { addr: 0x4100_9000 }),
            unsupported(Unsupported::FlashCommandBits { cmd: 0x0300_0000 }),
            unsupported(Unsupported::FlashSuspend { sus_ctrl: 0x20 }),
            unsupported(Unsupported::FlashTransferLength {
                mosi_bits: 9,
                miso_bits: 0,
            }),
            unsupported(FORM),
            unsupported(RANGE),
            unsupported(PROTECTED),
            mapping(3, MappingFault::InvalidEntry),
            mapping(3, MappingFault::BeyondFlash),
            Diagnostic::StaleFlashWindow {
                vaddr: 0x3C00_E000,
                page: 0,
                pc: 0x4038_0000,
            },
        ] {
            let mut strict = Diagnostics::new(true);
            strict.record(diagnostic.clone());
            assert_eq!(
                strict.take_violation(),
                Some(diagnostic.clone()),
                "{diagnostic}"
            );
            let mut lenient = Diagnostics::new(false);
            lenient.record(diagnostic.clone());
            assert_eq!(lenient.take_violation(), None);
            assert_eq!(lenient.drain(), vec![diagnostic]);
        }
    }

    #[test]
    fn a_cpu_exception_is_still_not_a_strict_violation() {
        let mut d = Diagnostics::new(true);
        d.record(Diagnostic::Exception {
            cause: cpu_rv32::Exception::IllegalInstr,
            tval: 0,
            pc: 4,
            sp: 0,
        });
        assert_eq!(d.take_violation(), None);
    }

    fn reset_at(cause: ResetCause, pc: u32, sp: u32) -> Diagnostic {
        Diagnostic::Reset { cause, pc, sp }
    }

    #[test]
    fn a_reset_is_an_entry_of_the_journal_and_never_a_strict_violation() {
        let reset = reset_at(ResetCause::Mwdt0Core, 0x4004_c0d4, 0x3FCD_E710);
        let mut strict = Diagnostics::new(true);
        strict.record(reset.clone());
        assert_eq!(
            strict.take_violation(),
            None,
            "a strict run goes on across a reset"
        );
        assert_eq!(strict.drain(), vec![reset]);
    }

    #[test]
    fn a_reset_names_its_cause_in_words_with_the_code_the_rom_prints_its_pc_and_its_stack() {
        for (cause, pc, sp, text) in [
            (
                ResetCause::Mwdt0Core,
                0x4004_c0d4,
                0x3FCD_E710,
                "reset by main watchdog 0, a core reset (rst:0x7) at pc 0x4004c0d4, sp 0x3fcde710",
            ),
            (
                ResetCause::SoftwareCpu,
                0x4004_8b92,
                0x3FC9_0000,
                "reset by the firmware, a CPU reset (rst:0xc) at pc 0x40048b92, sp 0x3fc90000",
            ),
            (
                ResetCause::RtcWatchdogSystem,
                0x10,
                0,
                "reset by the RTC watchdog, a system reset (rst:0x10) at pc 0x00000010, sp 0x00000000",
            ),
        ] {
            assert_eq!(reset_at(cause, pc, sp).to_string(), text);
        }
    }

    #[test]
    fn resets_are_listed_once_per_cause_and_pc_and_counted() {
        let mut d = Diagnostics::new(false);
        let first = reset_at(ResetCause::Mwdt0Core, 0x4004_c0d4, 0x3FCD_E710);
        d.record(first.clone());
        // The loop of a blank chip: the same watchdog at the same instruction, another stack.
        d.record(reset_at(ResetCause::Mwdt0Core, 0x4004_c0d4, 0x3FCD_E700));
        // Another cause at the same instruction, and the same cause at another one.
        d.record(reset_at(
            ResetCause::RtcWatchdogSystem,
            0x4004_c0d4,
            0x3FCD_E710,
        ));
        d.record(reset_at(ResetCause::Mwdt0Core, 0x4004_c0d8, 0x3FCD_E710));
        let listed = d.drain();
        assert_eq!(listed.len(), 3, "{listed:?}");
        assert_eq!(
            listed[0], first,
            "the entry shown is the first occurrence, with its stack pointer"
        );
        assert_eq!(d.occurrences(&first), 2, "the second one is counted");
        assert_eq!(d.most_repeated(4), vec![(first, 2)]);
    }

    #[test]
    fn unsupported_events_are_deduplicated_by_what_they_are_about() {
        let mut d = Diagnostics::new(false);
        d.record(unsupported(Unsupported::FlashCommand { opcode: 0x02 }));
        d.record(Diagnostic::Unsupported {
            what: Unsupported::FlashCommand { opcode: 0x02 },
            pc: 0x4200_0000,
        });
        d.record(unsupported(Unsupported::FlashCommand { opcode: 0x20 }));
        d.record(mapping(3, MappingFault::InvalidEntry));
        d.record(mapping(3, MappingFault::InvalidEntry));
        d.record(mapping(4, MappingFault::InvalidEntry));
        assert_eq!(d.distinct(), 4);
        assert_eq!(
            d.occurrences(&unsupported(Unsupported::FlashCommand { opcode: 0x02 })),
            2
        );

        // A byte sent on a clock that is not modelled is about its UART and the selection, not
        // about the instruction that wrote it: a firmware prints from many places.
        let uart_clock = |uart: u8, sclk_sel: u32, pc: u32| Diagnostic::Unsupported {
            what: Unsupported::UartClock { uart, sclk_sel },
            pc,
        };
        d.record(uart_clock(0, 2, 0x4038_1000));
        d.record(uart_clock(0, 2, 0x4200_0000));
        assert_eq!(d.distinct(), 5, "one entry for two call sites");
        assert_eq!(d.occurrences(&uart_clock(0, 2, 0x4038_2000)), 2);
        d.record(uart_clock(0, 0, 0x4038_1000));
        assert_eq!(d.distinct(), 6, "another selection is another entry");
        d.record(uart_clock(1, 2, 0x4038_1000));
        assert_eq!(d.distinct(), 7, "another UART is another entry");
        for (uart, sclk_sel, count) in [(0, 2, 2), (0, 0, 1), (1, 2, 1), (1, 0, 0)] {
            assert_eq!(
                d.occurrences(&uart_clock(uart, sclk_sel, 0x4038_1000)),
                count,
                "UART{uart}, SCLK_SEL {sclk_sel}"
            );
        }
        // What the receiver showed on such a clock is another line, once per UART and selection.
        let received = |uart: u8, sclk_sel: u32, pc: u32| Diagnostic::Unsupported {
            what: Unsupported::UartReceiveClock { uart, sclk_sel },
            pc,
        };
        d.record(received(0, 2, 0x4038_1000));
        d.record(received(0, 2, 0x4200_0000));
        assert_eq!(d.distinct(), 8, "not a byte sent");
        assert_eq!(d.occurrences(&received(0, 2, 0x4038_2000)), 2);
        assert_eq!(d.occurrences(&uart_clock(0, 2, 0x4038_1000)), 2);
        d.record(received(1, 2, 0x4038_1000));
        d.record(received(0, 0, 0x4038_1000));
        assert_eq!(d.distinct(), 10);
    }

    #[test]
    fn a_refused_uart_feature_is_listed_once_per_uart_and_feature() {
        let feature = |uart, feature, pc| Diagnostic::Unsupported {
            what: Unsupported::UartFeature { uart, feature },
            pc,
        };
        let mut d = Diagnostics::new(false);
        d.record(feature(0, UartFeature::Loopback, 0x4200_0000));
        d.record(feature(0, UartFeature::Loopback, 0x4200_0010));
        d.record(feature(1, UartFeature::Loopback, 0x4200_0000));
        d.record(feature(0, UartFeature::Autobaud, 0x4200_0000));
        assert_eq!(d.distinct(), 3, "per UART and feature, whatever the pc");
        assert_eq!(
            d.occurrences(&feature(0, UartFeature::Loopback, 0x4200_0020)),
            2
        );
    }

    #[test]
    fn a_refusal_of_the_flash_is_listed_once_per_what_it_is_about_whatever_its_numbers() {
        // Each row: a refusal, the same one with every other number changed (one entry with the
        // first), and one that is about something else (another entry), or `None` when the kind
        // has a single line.
        let form = |opcode, address_bits, dummy_cycles, data_bytes| Unsupported::FlashCommandForm {
            opcode,
            address_bits,
            dummy_cycles,
            data_bytes,
        };
        let range = |opcode, address, len| Unsupported::FlashRange {
            opcode,
            address,
            len,
        };
        let protected = |opcode, status| Unsupported::FlashProtected { opcode, status };
        let length = |mosi_bits, miso_bits| Unsupported::FlashTransferLength {
            mosi_bits,
            miso_bits,
        };
        let rows: [(Unsupported, Unsupported, Option<Unsupported>); 7] = [
            // A firmware that retries a read at another address, with another length.
            (
                range(0xBB, 0x10_0000, 64),
                range(0xBB, 0xFF_FFC0, 4),
                Some(range(0x20, 0x10_0000, 64)),
            ),
            (
                form(0xBB, 24, 8, 0),
                form(0xBB, 32, 6, 3),
                Some(form(0x02, 24, 8, 0)),
            ),
            (
                protected(0x02, 0x3E),
                protected(0x02, 0x06),
                Some(protected(0xD8, 0x3E)),
            ),
            // 65 bytes at two addresses, then 66 bytes.
            (
                Unsupported::FlashProgramLength { addr: 0x4100_9000 },
                Unsupported::FlashProgramLength { addr: 0x41FF_F000 },
                Some(Unsupported::FlashProgramLength { addr: 0x4200_9000 }),
            ),
            (
                Unsupported::FlashSuspend { sus_ctrl: 0x20 },
                Unsupported::FlashSuspend {
                    sus_ctrl: 0x0A00_0013,
                },
                None,
            ),
            // Two other command bits, and other bits below them.
            (
                Unsupported::FlashCommandBits { cmd: 0x0300_0000 },
                Unsupported::FlashCommandBits { cmd: 0xC004_1234 },
                None,
            ),
            (length(9, 0), length(0, 1024), None),
        ];
        for (first, same, other) in rows {
            let mut d = Diagnostics::new(false);
            d.record(unsupported(first));
            d.record(Diagnostic::Unsupported {
                what: same,
                pc: 0x4200_0000,
            });
            assert_eq!(
                d.drain(),
                vec![unsupported(first)],
                "{first:?} and {same:?}"
            );
            assert_eq!(d.occurrences(&unsupported(same)), 2, "{same:?}");
            if let Some(other) = other {
                d.record(unsupported(other));
                assert_eq!(d.drain(), vec![unsupported(other)], "{other:?}");
                assert_eq!(d.occurrences(&unsupported(first)), 2, "{first:?}");
            }
        }
    }

    /// The most entries the refusals of the flash can make, all kinds together: an opcode for
    /// each of the four refusals of the chip, a byte count for a program length, a command
    /// length (none, or 1 to 16 bits) for a transfer, and one line each for a data length, a
    /// suspend request and several command bits.
    const FLASH_REFUSAL_ENTRIES: usize = 4 * 256 + 256 + 17 + 3;

    #[test]
    fn no_refusal_of_the_flash_can_fill_the_table_whatever_values_the_firmware_writes() {
        const {
            assert!(FLASH_REFUSAL_ENTRIES < MAX_DISTINCT_DIAGNOSTICS / 2);
        }
        let mut d = Diagnostics::new(false);
        let mut numbers = crate::test_support::Lcg::new(0x5EED);
        for _ in 0..40_000 {
            let n = &mut numbers;
            let refusals = [
                Unsupported::FlashCommand {
                    opcode: n.next_u8(),
                },
                Unsupported::FlashCommandForm {
                    opcode: n.next_u8(),
                    address_bits: n.next_u32(),
                    dummy_cycles: n.next_u32(),
                    data_bytes: n.next_u32(),
                },
                Unsupported::FlashRange {
                    opcode: n.next_u8(),
                    address: n.next_u32(),
                    len: n.next_u32(),
                },
                Unsupported::FlashProtected {
                    opcode: n.next_u8(),
                    status: n.next_u8(),
                },
                // `USER2[31:28]` plus one: a command phase has 1 to 16 bits, or does not exist.
                Unsupported::FlashTransfer {
                    command_bits: Some(n.below(17)).filter(|&bits| bits != 0),
                    user: n.next_u32(),
                    user2: n.next_u32(),
                },
                Unsupported::FlashTransferLength {
                    mosi_bits: n.below(1025),
                    miso_bits: n.below(1025),
                },
                Unsupported::FlashProgramLength { addr: n.next_u32() },
                Unsupported::FlashCommandBits { cmd: n.next_u32() },
                Unsupported::FlashSuspend {
                    sus_ctrl: n.next_u32(),
                },
            ];
            for what in refusals {
                let pc = n.next_u32();
                d.record(Diagnostic::Unsupported { what, pc });
            }
        }
        assert_eq!(
            d.distinct(),
            FLASH_REFUSAL_ENTRIES,
            "360,000 refusals with distinct numbers: every entry there can be, and no more"
        );
        assert_eq!(d.suppressed(), 0);
        assert!(!d.drain().contains(&Diagnostic::Suppressed));
        // Whatever comes next is still listed.
        d.record(read_at(0x6001_9000, 0));
        assert_eq!(d.drain(), vec![read_at(0x6001_9000, 0)]);
    }

    #[test]
    fn new_messages_state_the_cause_then_the_remedy() {
        let texts = [
            (
                unsupported(Unsupported::FlashCommand { opcode: 0x02 }),
                "flash command 0x02 is not modeled",
            ),
            (unsupported(Unsupported::ShaDma), "DMA-SHA"),
            (unsupported(Unsupported::ShaMode { mode: 5 }), "SHA.MODE 5"),
            (
                unsupported(Unsupported::ClockSource {
                    sysclk_conf: 0xC00,
                    cpu_per_conf: 0xC,
                }),
                "SYSCLK_CONF 0x00000c00",
            ),
            (
                unsupported(Unsupported::SlowClock {
                    clk_conf: 0x4000_0000,
                }),
                "RTC slow clock",
            ),
            (
                unsupported(Unsupported::UartClock {
                    uart: 1,
                    sclk_sel: 2,
                }),
                "UART1 sent a byte with a source clock inkemu does not model (CLK_CONF.SCLK_SEL = 2",
            ),
            (
                unsupported(Unsupported::FlashProgramLength { addr: 0x4100_9000 }),
                "program 65 bytes with one FLASH_PP command (ADDR 0x41009000",
            ),
            (
                unsupported(Unsupported::FlashCommandBits { cmd: 0x0300_0000 }),
                "several command bits at once (value 0x03000000",
            ),
            (
                unsupported(Unsupported::FlashSuspend { sus_ctrl: 0x20 }),
                "program/erase suspend or resume (value 0x00000020",
            ),
            (
                unsupported(Unsupported::FlashTransferLength {
                    mosi_bits: 9,
                    miso_bits: 1024,
                }),
                "a data phase of 9 bits out and 1024 bits in (pc 0x40381000) but only whole bytes are modeled, 64 at most, what W0..W15 hold, so nothing was sent to the flash and SPI1.W0..W15 keep the bytes they held; give MOSI_DLEN and MISO_DLEN the value 8 × bytes − 1",
            ),
            (
                unsupported(FORM),
                "flash command 0x02 was sent in a form the virtual flash does not know (32 address bits, 0 dummy clocks, 4 bytes of data out",
            ),
            (
                unsupported(RANGE),
                "flash command 0x20 addresses 4096 bytes at 0x100000, past the end of the flash",
            ),
            (
                unsupported(PROTECTED),
                "flash command 0xd8 would program or erase while block-protect bits of the flash status register are set (status 0x3e",
            ),
            (mapping(3, MappingFault::InvalidEntry), "MMU entry 3"),
            (
                mapping(3, MappingFault::BeyondFlash),
                "beyond the end of the flash",
            ),
        ];
        for (diagnostic, needle) in texts {
            assert_cause_then_remedy(&diagnostic, needle);
        }
    }

    fn assert_cause_then_remedy(diagnostic: &Diagnostic, needle: &str) {
        let text = diagnostic.to_string();
        assert!(text.contains(needle), "{text}");
        assert!(text.contains("pc 0x40"), "{text}");
        let (cause, remedy) = text
            .split_once("; ")
            .unwrap_or_else(|| panic!("no remedy in: {text}"));
        assert!(!cause.is_empty() && !remedy.is_empty(), "{text}");
    }

    #[test]
    fn the_refusals_of_the_serial_ports_state_the_cause_then_the_remedy() {
        let texts = [
            (
                Unsupported::UartFeature {
                    uart: 1,
                    feature: UartFeature::Loopback,
                },
                "UART1 was set to loop its transmitter back to its receiver (CONF0.LOOPBACK, pc 0x40381000), which inkemu does not model",
            ),
            (
                Unsupported::UartFeature {
                    uart: 0,
                    feature: UartFeature::InvertedRx,
                },
                "(CONF0.RXD_INV, pc 0x40381000)",
            ),
            (
                Unsupported::UartFeature {
                    uart: 0,
                    feature: UartFeature::Autobaud,
                },
                "(CONF0.AUTOBAUD_EN, pc 0x40381000)",
            ),
            (
                Unsupported::UartFeature {
                    uart: 0,
                    feature: UartFeature::NoStopBit,
                },
                "(CONF0.STOP_BIT_NUM = 0, pc 0x40381000)",
            ),
            (
                Unsupported::UartFeature {
                    uart: 0,
                    feature: UartFeature::FifoSize,
                },
                "(MEM_CONF.RX_SIZE or TX_SIZE other than 1, pc 0x40381000)",
            ),
            (
                Unsupported::UartFeature {
                    uart: 0,
                    feature: UartFeature::Rs485,
                },
                "UART0 was set to RS485 mode (RS485_CONF other than 0, pc 0x40381000)",
            ),
            (
                Unsupported::UartReceiveClock {
                    uart: 1,
                    sclk_sel: 2,
                },
                "UART1 received from the host, or counted its receive timeout, with a source clock inkemu does not model (CLK_CONF.SCLK_SEL = 2, read at pc 0x40381000)",
            ),
            (
                Unsupported::UartRxOverflow { uart: 1 },
                "a byte the host sent found the 128-byte receive FIFO of UART1 full (pc 0x40381000)",
            ),
            (
                Unsupported::UartEmptyRead { uart: 0 },
                "UART0.FIFO was read while the receive FIFO was empty (pc 0x40381000), so 0 was returned",
            ),
            (
                Unsupported::UsbEmptyRead,
                "USB_DEVICE.EP1 was read while the OUT FIFO was empty (pc 0x40381000)",
            ),
            (
                Unsupported::UsbInFull,
                "a byte was written to USB_DEVICE.EP1 while the IN endpoint had no room (pc 0x40381000)",
            ),
        ];
        for (what, needle) in texts {
            assert_cause_then_remedy(&unsupported(what), needle);
        }
    }

    #[test]
    fn a_refused_flash_command_is_described_truthfully_whatever_the_command_does() {
        // The same text serves a 4-byte-address read (0x0C) or program (0x12), deep power-down
        // (0xB9) and an opcode no chip has (0x00).
        for opcode in [0x0C, 0x12, 0xB9, 0x00] {
            let text = unsupported(Unsupported::FlashCommand { opcode }).to_string();
            assert!(
                text.contains(&format!("flash command {opcode:#04x} is not modeled")),
                "{text}"
            );
            assert!(
                text.contains(
                    "the virtual flash knows write enable and disable (0x06, 0x04), the status read and write (0x05, 0x01), the identification (0x9f), the SFDP read (0x5a), release from power-down (0xab), resume (0x7a), the data reads (0x03, 0x0b, 0x3b, 0xbb, 0x6b, 0xeb), the page program (0x02) and the erases (0x20, 0xd8, 0x60, 0xc7), so nothing was read or changed and SPI1.W0..W15 keep the bytes they held; use firmware that sends only these flash commands"
                ),
                "{text}"
            );
            assert!(
                !text.contains("read-only"),
                "the flash is no longer read-only: {text}"
            );
        }
    }

    #[test]
    fn the_commands_a_refusal_lists_are_the_ones_the_chip_runs_all_of_them_and_no_other() {
        // The opcode refused is one the list cannot hold: `0xb9` is in neither.
        let text = unsupported(Unsupported::FlashCommand { opcode: 0xB9 }).to_string();
        let (_, list) = text.split_once("the virtual flash knows").unwrap();
        let (list, _) = list.split_once(", so nothing").unwrap();
        let mut listed: Vec<u8> = list
            .split("0x")
            .skip(1)
            .map(|rest| u8::from_str_radix(&rest[..2], 16).unwrap())
            .collect();
        listed.sort_unstable();
        let run: Vec<u8> = (0..=u8::MAX)
            .filter(|&opcode| FlashFrame::of(opcode).is_some())
            .collect();
        assert_eq!(listed, run, "{list}");
    }

    #[test]
    fn the_remedies_of_the_flash_refusals_cover_every_cause_they_are_raised_for() {
        // A suspend request is raised for the automatic suspend and for a suspend or a resume
        // the firmware sends itself (`FLASH_PES`, `FLASH_PER`).
        let suspend = unsupported(Unsupported::FlashSuspend { sus_ctrl: 0b10 }).to_string();
        assert!(
            suspend.ends_with(
                "so nothing was sent to the flash; build the firmware without flash suspend: no auto-suspend (CONFIG_SPI_FLASH_AUTO_SUSPEND=n, the ESP-IDF default) and no suspend or resume sent by software (FLASH_PES and FLASH_PER, which ESP-IDF writes only with CONFIG_SPI_FLASH_SOFTWARE_RESUME)"
            ),
            "{suspend}"
        );
        // The remedy of a protected write clears the bits that protect and no other: a status
        // write of 0x00 would clear quad enable too.
        let protected = unsupported(PROTECTED).to_string();
        assert!(
            protected.ends_with(
                "so nothing was changed; clear the block-protect bits (bits 5:2 of the status register) with a status write that keeps its other bits, before the program or the erase"
            ),
            "{protected}"
        );
        assert!(!protected.contains("0x00"), "{protected}");
        // A refused read left the data registers as they were: the firmware goes on with the
        // bytes of an earlier transfer.
        let range = unsupported(Unsupported::FlashRange {
            opcode: 0xBB,
            address: 0x10_0000,
            len: 64,
        })
        .to_string();
        assert!(
            range.contains(
                "so nothing was read or changed and SPI1.W0..W15 keep the bytes they held; keep flash accesses inside the flash size of the device profile"
            ),
            "{range}"
        );
    }

    #[test]
    fn a_command_in_a_form_the_flash_does_not_know_is_told_the_frame_it_takes() {
        let form = |opcode, address_bits, dummy_cycles, data_bytes| {
            unsupported(Unsupported::FlashCommandForm {
                opcode,
                address_bits,
                dummy_cycles,
                data_bytes,
            })
            .to_string()
        };
        // (the refusal, what was sent, the frame to send instead)
        let cases = [
            (
                form(0xBB, 24, 8, 0),
                "(24 address bits, 8 dummy clocks, 0 bytes of data out, pc 0x40381000)",
                "; send 0xbb with a 24-bit address, 4 dummy clocks and no data out",
            ),
            (
                form(0x06, 24, 0, 3),
                "(24 address bits, 0 dummy clocks, 3 bytes of data out, pc 0x40381000)",
                "; send 0x06 with no address, no dummy clock and no data out",
            ),
            (
                form(0x01, 0, 0, 2),
                "(0 address bits, 0 dummy clocks, 2 bytes of data out, pc 0x40381000)",
                "; send 0x01 with no address, no dummy clock and 1 byte of data out",
            ),
            (
                form(0x02, 32, 0, 4),
                "(32 address bits, 0 dummy clocks, 4 bytes of data out, pc 0x40381000)",
                "; send 0x02 with a 24-bit address, no dummy clock and 1 byte of data out or more",
            ),
            // The chip never refuses the form of an opcode it does not know; a value built by
            // hand still gets a message with a remedy.
            (
                form(0x35, 0, 0, 1),
                "flash command 0x35 was sent in a form",
                "; send the command with the address, the dummy clocks and the data its datasheet gives it",
            ),
        ];
        for (text, sent, remedy) in cases {
            assert!(text.contains(sent), "{text}");
            assert!(text.ends_with(remedy), "{text}");
            assert!(
                text.contains(
                    "so nothing was read or changed and SPI1.W0..W15 keep the bytes they held; "
                ),
                "{text}"
            );
        }
    }

    #[test]
    fn a_dma_sha_write_is_not_described_as_a_started_hash() {
        // The same diagnostic reports a write to `DMA_BLOCK_NUM`, which configures and starts nothing.
        let text = unsupported(Unsupported::ShaDma).to_string();
        assert!(
            text.contains("SHA.DMA_BLOCK_NUM, DMA_START or DMA_CONTINUE was written"),
            "{text}"
        );
        assert!(!text.contains("was started"), "{text}");
    }

    #[test]
    fn a_flash_transfer_of_an_unmodeled_shape_says_which_shape() {
        let without = unsupported(Unsupported::FlashTransfer {
            command_bits: None,
            user: 0x1000_0000,
            user2: 0x7000_009F,
        });
        let sixteen_bits = unsupported(Unsupported::FlashTransfer {
            command_bits: Some(16),
            user: 0x9000_0000,
            user2: 0xF000_9F00,
        });
        assert!(
            without.to_string().contains("without a command phase"),
            "{without}"
        );
        assert!(
            sixteen_bits.to_string().contains("with a 16-bit command"),
            "{sixteen_bits}"
        );
        for diagnostic in [&without, &sixteen_bits] {
            let text = diagnostic.to_string();
            assert!(
                text.contains("USER 0x") && text.contains("USER2 0x") && text.contains("pc 0x40"),
                "{text}"
            );
            assert!(
                text.contains("nothing was sent to the flash; use firmware"),
                "{text}"
            );
        }
        // Two shapes are two diagnostics; the same shape with another opcode or data phase is one.
        let mut d = Diagnostics::new(true);
        d.record(without.clone());
        d.record(sixteen_bits);
        d.record(unsupported(Unsupported::FlashTransfer {
            command_bits: None,
            user: 0x0800_0000,
            user2: 0x7000_0002,
        }));
        assert_eq!(d.distinct(), 2);
        assert_eq!(d.occurrences(&without), 2);
        assert_eq!(
            d.take_violation(),
            Some(without),
            "a strict run stops on it"
        );
    }

    #[test]
    fn messages_name_peripheral_register_address_and_pc() {
        let text = read_at(0x6001_9000, 0x4000_1234).to_string();
        for part in ["LEDC.CONF", "0x60019000", "0x40001234", "0x00000007"] {
            assert!(text.contains(part), "{part} missing from: {text}");
        }
        let undocumented = Diagnostic::UnmodeledMmio {
            addr: 0x6000_4038,
            direction: Direction::Read,
            pc: 0,
            register: Some(("GPIO", "STRAP")),
            value: 0,
            reset_explicit: false,
        };
        assert!(
            undocumented
                .to_string()
                .contains("documents no reset value"),
            "{undocumented}"
        );
        let nowhere = Diagnostic::UnmodeledMmio {
            addr: 0x600C_5000,
            direction: Direction::Write,
            pc: 0,
            register: None,
            value: 9,
            reset_explicit: false,
        };
        assert!(
            nowhere.to_string().contains("no register is documented"),
            "{nowhere}"
        );
    }

    #[test]
    fn an_exception_names_its_pc_its_tval_and_the_stack_pointer() {
        let text = Diagnostic::Exception {
            cause: cpu_rv32::Exception::LoadAccessFault,
            tval: 0x0000_0010,
            pc: 0x4200_1234,
            sp: 0x3FC9_0000,
        }
        .to_string();
        assert_eq!(
            text,
            "CPU exception: load access fault at pc 0x42001234 (tval 0x00000010, sp 0x3fc90000); the firmware's trap handler takes it, as on the device"
        );
        assert!(
            !text.contains("LoadAccessFault"),
            "words for the user, not the identifier of the code"
        );
    }

    #[test]
    fn the_same_exception_at_the_same_pc_is_listed_once_whatever_the_stack_pointer() {
        let mut d = Diagnostics::new(false);
        for sp in [0x3FC9_0000, 0x3FC9_0010] {
            d.record(Diagnostic::Exception {
                cause: cpu_rv32::Exception::IllegalInstr,
                tval: 0,
                pc: 4,
                sp,
            });
        }
        let listed = d.drain();
        assert_eq!(listed.len(), 1, "one line per cause and PC (decision E12)");
        assert!(
            matches!(
                listed[0],
                Diagnostic::Exception {
                    sp: 0x3FC9_0000,
                    ..
                }
            ),
            "the entry shown is the first occurrence, with its stack pointer: {:?}",
            listed[0]
        );
        assert_eq!(d.occurrences(&listed[0]), 2, "the second one is counted");
    }

    #[test]
    fn a_window_read_that_may_differ_is_listed_once_per_flash_page_and_says_what_to_do() {
        let stale = |vaddr, page| Diagnostic::StaleFlashWindow {
            vaddr,
            page,
            pc: 0x4038_0000,
        };
        let mut d = Diagnostics::new(false);
        d.record(stale(0x3C00_E000, 0));
        d.record(stale(0x3C00_E004, 0));
        d.record(stale(0x3C01_0000, 1));
        assert_eq!(
            d.drain(),
            vec![stale(0x3C00_E000, 0), stale(0x3C01_0000, 1)],
            "one line per flash page, with the first address read"
        );
        assert_eq!(d.occurrences(&stale(0, 0)), 2);
        let message = stale(0x3C01_E000, 1).to_string();
        for part in [
            "0x3c01e000",
            "pc 0x40380000",
            "may differ from the device",
            "flash 0x010000 to 0x01ffff",
            // An invalidation takes addresses of a window, not a flash range: the remedy says
            // which addresses.
            "; invalidate the cache at the window addresses that map that flash",
        ] {
            assert!(message.contains(part), "{part:?} in {message:?}");
        }
        // The page is a number the firmware chose: the message of any of them can be formed.
        assert!(stale(0, u32::MAX).to_string().contains("0xffffffffffff"));
    }
}
