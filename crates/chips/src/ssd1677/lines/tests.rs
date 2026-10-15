//! The SSD1677 driven through its lines gives what the same bytes give at the command level.
use std::cell::RefCell;
use std::rc::Rc;

use emu_core::{Clocked, Cycles, GpioInput, GpioOutput, Level, Nanos, SpiTarget};

use super::{LineFault, ONE_GIGAHERTZ, Ssd1677Lines};
use crate::ssd1677::{DiagnosticKind, Plane, RefreshKind, Ssd1677, Ssd1677Config};

const MS: u64 = 1_000_000;
const ROW_BYTES: usize = 100;
const HEIGHT: usize = 480;

/// What a firmware's driver needs of the panel, whether it reaches the chip through its lines or
/// calls it directly.
trait Panel {
    fn reset(&mut self, at: u64, asserted: bool);
    /// A command and its data. `one_selection`: both in one selection of the chip (`FreeInk`'s
    /// `cmdData`), else one each.
    fn send(&mut self, at: u64, command: u8, data: &[u8], one_selection: bool);
    fn busy(&mut self, at: u64) -> bool;
    fn next_deadline(&mut self, at: u64) -> Option<u64>;
    fn advance(&mut self, at: u64);
    fn chip(&self) -> std::cell::Ref<'_, Ssd1677>;
}

/// The chip called directly, on the same 1 GHz base.
struct Direct(RefCell<Ssd1677>);

impl Panel for Direct {
    fn reset(&mut self, at: u64, asserted: bool) {
        self.0.get_mut().set_reset(Cycles(at), asserted);
    }

    fn send(&mut self, at: u64, command: u8, data: &[u8], _one_selection: bool) {
        let chip = self.0.get_mut();
        chip.write_command(Cycles(at), command);
        for &byte in data {
            chip.write_data(Cycles(at), byte);
        }
    }

    fn busy(&mut self, at: u64) -> bool {
        self.0.get_mut().busy_pin(Cycles(at))
    }

    fn next_deadline(&mut self, _at: u64) -> Option<u64> {
        self.0.get_mut().next_deadline().map(Cycles::get)
    }

    fn advance(&mut self, at: u64) {
        self.0.get_mut().advance(Cycles(at));
    }

    fn chip(&self) -> std::cell::Ref<'_, Ssd1677> {
        self.0.borrow()
    }
}

fn wired_lines() -> Rc<RefCell<Ssd1677Lines>> {
    Rc::new(RefCell::new(
        Ssd1677Lines::new(Ssd1677Config::default()).unwrap(),
    ))
}

/// The chip on its lines, as a `SoC` with a hardware SPI controller reaches it.
struct Bus {
    shared: Rc<RefCell<Ssd1677Lines>>,
    target: Box<dyn SpiTarget>,
    clocked: Box<dyn Clocked>,
    reset: Box<dyn GpioOutput>,
    busy: Box<dyn GpioInput>,
}

impl Bus {
    fn new() -> Bus {
        let shared = wired_lines();
        let outputs = Ssd1677Lines::outputs(&shared);
        let inputs = Ssd1677Lines::inputs(&shared);
        Bus {
            target: Box::new(Rc::clone(&shared)),
            clocked: Box::new(Rc::clone(&shared)),
            reset: Box::new(outputs.reset),
            busy: Box::new(inputs.busy),
            shared,
        }
    }

    /// Chunks of 64 bytes, as an SPI controller with sixteen data words sends them.
    fn transfers(&mut self, at: u64, dc: Level, bytes: &[u8]) {
        for chunk in bytes.chunks(64) {
            let mut miso = vec![0xFF; chunk.len()];
            self.target.transfer(Some(dc), chunk, &mut miso, Nanos(at));
            assert!(miso.iter().all(|&b| b == 0xFF), "the panel drives no MISO");
        }
    }
}

impl Panel for Bus {
    fn reset(&mut self, at: u64, asserted: bool) {
        let level = if asserted { Level::Low } else { Level::High };
        self.reset.driven(Some(level), Nanos(at));
    }

    fn send(&mut self, at: u64, command: u8, data: &[u8], one_selection: bool) {
        self.target.select(true, Nanos(at));
        self.transfers(at, Level::Low, &[command]);
        if !one_selection {
            self.target.select(false, Nanos(at));
            self.target.select(true, Nanos(at));
        }
        if !data.is_empty() {
            self.transfers(at, Level::High, data);
        }
        self.target.select(false, Nanos(at));
    }

    fn busy(&mut self, at: u64) -> bool {
        self.busy.level(Nanos(at)) == Some(Level::High)
    }

    fn next_deadline(&mut self, at: u64) -> Option<u64> {
        self.clocked.next_deadline(Nanos(at)).map(Nanos::get)
    }

    fn advance(&mut self, at: u64) {
        self.clocked.advance(Nanos(at));
    }

    fn chip(&self) -> std::cell::Ref<'_, Ssd1677> {
        std::cell::Ref::map(self.shared.borrow(), Ssd1677Lines::chip)
    }
}

/// `FreeInk`'s `Ssd1677Driver` on the X4 (research `sp2a-external-chips.md` §4), alternating the
/// two framings of a command: `one_selection` for every other command.
struct Driver<P: Panel> {
    panel: P,
    now: u64,
    sent: u32,
}

impl<P: Panel> Driver<P> {
    fn new(panel: P) -> Driver<P> {
        Driver {
            panel,
            now: 0,
            sent: 0,
        }
    }

    fn delay(&mut self, ms: u64) {
        self.now += ms * MS;
        self.panel.advance(self.now);
    }

    fn cmd(&mut self, command: u8, data: &[u8]) {
        self.sent += 1;
        self.panel
            .send(self.now, command, data, self.sent.is_multiple_of(2));
    }

    fn wait_busy(&mut self) {
        while self.panel.busy(self.now) {
            let next = self.panel.next_deadline(self.now).unwrap();
            self.now = next;
            self.panel.advance(next);
        }
    }

    fn begin(&mut self) {
        self.panel.reset(self.now, false);
        self.delay(10);
        self.panel.reset(self.now, true);
        self.delay(10);
        self.panel.reset(self.now, false);
        self.delay(10);
        self.cmd(0x12, &[]);
        self.delay(10);
        self.wait_busy();
        self.cmd(0x18, &[0x80]);
        self.cmd(0x0C, &[0xAE, 0xC7, 0xC3, 0xC0, 0x80]);
        self.cmd(0x01, &[0xDF, 0x01, 0x02]);
        self.cmd(0x3C, &[0x80]);
        self.window();
        self.cmd(0x46, &[0xF7]);
        self.wait_busy();
        self.cmd(0x47, &[0xF7]);
        self.wait_busy();
    }

    fn window(&mut self) {
        self.cmd(0x11, &[0x01]);
        self.cmd(0x44, &[0x00, 0x00, 0x1F, 0x03]);
        self.cmd(0x45, &[0xDF, 0x01, 0x00, 0x00]);
        self.cmd(0x4E, &[0x00, 0x00]);
        self.cmd(0x4F, &[0xDF, 0x01]);
    }

    fn display(&mut self, frame: &[u8], fast: bool) {
        self.window();
        self.cmd(0x24, frame);
        if !fast {
            self.cmd(0x26, frame);
        }
        self.cmd(0x21, &[if fast { 0x00 } else { 0x40 }]);
        self.cmd(0x3C, &[0xC0]);
        self.cmd(0x22, &[if fast { 0xFC } else { 0xF7 }]);
        self.cmd(0x20, &[]);
        self.wait_busy();
    }

    fn sleep(&mut self) {
        self.cmd(0x3C, &[0x80]);
        self.cmd(0x22, &[0x03]);
        self.cmd(0x20, &[]);
        self.delay(200);
        self.wait_busy();
        self.cmd(0x10, &[0x03]);
    }
}

/// A frame of the firmware: a black square in the top-left corner, a black bottom row.
fn frame(shift: usize) -> Vec<u8> {
    let mut bytes = vec![0xFFu8; ROW_BYTES * HEIGHT];
    for y in 0..HEIGHT {
        for x in 0..800 {
            if (x >= shift && x < shift + 16 && y < 8) || y == HEIGHT - 1 {
                bytes[y * ROW_BYTES + x / 8] &= !(0x80 >> (x % 8));
            }
        }
    }
    bytes
}

fn run<P: Panel>(panel: P) -> Driver<P> {
    let mut driver = Driver::new(panel);
    driver.begin();
    driver.display(&frame(0), false);
    driver.display(&frame(100), true);
    driver.sleep();
    driver
}

#[test]
fn freeink_s_bytes_through_the_spi_lines_drive_the_chip_as_its_commands_do() {
    let direct = run(Direct(RefCell::new(
        Ssd1677::new(Ssd1677Config::default(), ONE_GIGAHERTZ).unwrap(),
    )));
    let wired = run(Bus::new());
    assert_eq!(direct.now, wired.now, "the same waits");
    let (a, b) = (direct.panel.chip(), wired.panel.chip());
    assert_eq!(a.ram(Plane::BlackWhite), b.ram(Plane::BlackWhite));
    assert_eq!(a.ram(Plane::Red), b.ram(Plane::Red));
    assert_eq!(a.panel_image(), b.panel_image());
    assert_eq!(b.panel_image().generation, 2);
    assert_eq!(b.panel_image().kind, RefreshKind::Partial);
    drop((a, b));
    let mut shared = wired.panel.shared.borrow_mut();
    assert_eq!(shared.chip_mut().take_diagnostics(), []);
    assert_eq!(shared.take_faults(), []);
}

#[test]
fn a_selection_without_a_transfer_does_nothing() {
    let shared = wired_lines();
    let mut target: Box<dyn SpiTarget> = Box::new(Rc::clone(&shared));
    for at in 0..5 {
        target.select(true, Nanos(at));
        target.select(false, Nanos(at));
    }
    let mut lines = shared.borrow_mut();
    assert_eq!(lines.chip_mut().take_diagnostics(), []);
    assert_eq!(lines.take_faults(), []);
    assert!(lines.chip().ram(Plane::BlackWhite).iter().all(|&b| b == 0));
}

#[test]
fn a_transfer_without_a_data_command_line_is_a_fault_of_the_wiring() {
    let shared = wired_lines();
    let mut target: Box<dyn SpiTarget> = Box::new(Rc::clone(&shared));
    target.select(true, Nanos(3));
    let mut miso = [0x00; 2];
    target.transfer(None, &[0x12, 0x00], &mut miso, Nanos(3));
    assert_eq!(miso, [0x00; 2], "left as it came");
    let mut lines = shared.borrow_mut();
    assert_eq!(
        lines.take_faults(),
        [LineFault::TransferWithoutDc { at: Nanos(3) }]
    );
    assert_eq!(lines.chip_mut().take_diagnostics(), []);
    assert!(!lines.chip().is_busy(Cycles(3)), "no reset was run");
}

#[test]
fn reset_low_stops_an_operation_and_a_released_reset_does_not() {
    let shared = wired_lines();
    let outputs = Ssd1677Lines::outputs(&shared);
    let inputs = Ssd1677Lines::inputs(&shared);
    let (mut reset, mut busy) = (outputs.reset, inputs.busy);
    let mut target: Box<dyn SpiTarget> = Box::new(Rc::clone(&shared));
    reset.driven(None, Nanos(0));
    target.select(true, Nanos(0));
    target.transfer(Some(Level::Low), &[0x12], &mut [0xFF], Nanos(0));
    target.select(false, Nanos(0));
    assert_eq!(
        busy.level(Nanos(1)),
        Some(Level::High),
        "the soft reset runs"
    );
    let falls = busy.next_change(Nanos(1)).unwrap();
    assert!(falls > Nanos(1));
    assert_eq!(
        busy.level(falls),
        Some(Level::Low),
        "BUSY falls when announced"
    );
    assert_eq!(busy.next_change(falls), None);
    target.select(true, Nanos(falls.get()));
    target.transfer(Some(Level::Low), &[0x12], &mut [0xFF], falls);
    reset.driven(Some(Level::Low), Nanos(falls.get() + 1));
    assert_eq!(
        busy.level(Nanos(falls.get() + 1)),
        Some(Level::Low),
        "RES# low cuts the operation; BUSY is low in reset"
    );
    reset.driven(None, Nanos(falls.get() + 2));
    assert_eq!(
        busy.level(Nanos(falls.get() + 2)),
        Some(Level::High),
        "released, RES# leaves reset: the hardware-reset time"
    );
}

#[test]
fn the_chip_counts_in_nanoseconds_and_names_its_deadlines() {
    let shared = wired_lines();
    let mut clocked: Box<dyn Clocked> = Box::new(Rc::clone(&shared));
    let mut target: Box<dyn SpiTarget> = Box::new(Rc::clone(&shared));
    assert_eq!(clocked.next_deadline(Nanos(0)), None);
    target.select(true, Nanos(1_000));
    target.transfer(Some(Level::Low), &[0x12], &mut [0xFF], Nanos(1_000));
    let deadline = clocked.next_deadline(Nanos(1_000)).unwrap();
    let chip_deadline = shared.borrow().chip().next_deadline().unwrap();
    assert_eq!(
        deadline,
        Nanos(chip_deadline.get()),
        "one cycle is one nanosecond"
    );
    // The default software-reset time, 2 ms.
    assert_eq!(deadline, Nanos(1_000 + 2 * MS));
    clocked.advance(deadline);
    assert_eq!(clocked.next_deadline(deadline), None);
}

/// The firmware's side of a bit-banged bus (`XteinkDetect.cpp:50-90` at `111fdcc7`).
struct BitBang {
    shared: Rc<RefCell<Ssd1677Lines>>,
    target: Box<dyn SpiTarget>,
    clock: Box<dyn GpioOutput>,
    data_out: Box<dyn GpioOutput>,
    data_in: Box<dyn GpioInput>,
    dc: Box<dyn GpioOutput>,
    now: u64,
}

impl BitBang {
    fn new() -> BitBang {
        let shared = wired_lines();
        let outputs = Ssd1677Lines::outputs(&shared);
        let inputs = Ssd1677Lines::inputs(&shared);
        let mut bus = BitBang {
            target: Box::new(Rc::clone(&shared)),
            clock: Box::new(outputs.clock),
            data_out: Box::new(outputs.data),
            data_in: Box::new(inputs.data),
            dc: Box::new(outputs.data_command),
            shared,
            now: 0,
        };
        bus.clock.driven(Some(Level::Low), Nanos(0));
        bus
    }

    fn tick(&mut self) -> Nanos {
        self.now += 1_000;
        Nanos(self.now)
    }

    fn write(&mut self, byte: u8) {
        for bit in 0..8 {
            let level = Level::from_bool(byte & (0x80 >> bit) != 0);
            let at = self.tick();
            self.data_out.driven(Some(level), at);
            let at = self.tick();
            self.clock.driven(Some(Level::High), at);
            let at = self.tick();
            self.clock.driven(Some(Level::Low), at);
        }
    }

    /// A read with the line pulled up: a bit the chip leaves alone reads 1.
    fn read(&mut self) -> u8 {
        let mut byte = 0;
        for _ in 0..8 {
            let at = self.tick();
            let bit = self.data_in.level(at).unwrap_or(Level::High);
            byte = (byte << 1) | u8::from(bit.is_high());
            let at = self.tick();
            self.clock.driven(Some(Level::High), at);
            let at = self.tick();
            self.clock.driven(Some(Level::Low), at);
        }
        byte
    }

    /// `epdCmdRead`: the command with D/C# low, then D/C# high and SDA released.
    fn command_read(&mut self, command: u8, count: usize) -> Vec<u8> {
        let at = self.tick();
        self.dc.driven(Some(Level::Low), at);
        self.target.select(true, at);
        self.write(command);
        let at = self.tick();
        self.dc.driven(Some(Level::High), at);
        self.data_out.driven(None, at);
        let bytes = (0..count).map(|_| self.read()).collect();
        let at = self.tick();
        self.target.select(false, at);
        bytes
    }

    fn command_write(&mut self, command: u8, data: &[u8]) {
        let at = self.tick();
        self.dc.driven(Some(Level::Low), at);
        self.target.select(true, at);
        self.write(command);
        let at = self.tick();
        self.dc.driven(Some(Level::High), at);
        for &byte in data {
            self.write(byte);
        }
        let at = self.tick();
        self.target.select(false, at);
    }
}

#[test]
fn freeink_s_probe_finds_the_line_floating_and_the_chip_names_two_unknown_commands() {
    let mut bus = BitBang::new();
    let flag = bus.command_read(0x71, 1);
    let version = bus.command_read(0x70, 5);
    assert_eq!(flag, [0xFF]);
    assert_eq!(
        version, [0xFF; 5],
        "a floating line: the SSD-family verdict"
    );
    let kinds: Vec<_> = bus
        .shared
        .borrow_mut()
        .chip_mut()
        .take_diagnostics()
        .into_iter()
        .map(|d| d.kind)
        .collect();
    assert_eq!(
        kinds,
        [
            DiagnosticKind::UnknownCommand { command: 0x71 },
            DiagnosticKind::UnknownCommand { command: 0x70 },
        ]
    );
}

#[test]
fn a_bit_banged_status_read_gives_the_status_byte_bit_by_bit() {
    let mut direct = Ssd1677::new(Ssd1677Config::default(), ONE_GIGAHERTZ).unwrap();
    let mut bus = BitBang::new();
    // Clock and analog on, then a VCI detection: the status byte carries more than the chip id.
    for (command, data) in [(0x22, &[0xC0][..]), (0x20, &[][..])] {
        bus.command_write(command, data);
    }
    direct.write_command(Cycles(0), 0x22);
    direct.write_data(Cycles(0), 0xC0);
    direct.write_command(Cycles(0), 0x20);
    let settled = 1_000 * MS;
    direct.advance(Cycles(settled));
    bus.now = settled;
    direct.write_command(Cycles(settled), 0x2F);
    let expected = direct.read_data(Cycles(settled)).unwrap();
    assert_eq!(bus.command_read(0x2F, 1), [expected]);
    assert_eq!(expected & 0x03, 0x01, "the chip id is in the byte read");
}

#[test]
fn a_bit_banged_write_reaches_the_ram_as_a_transfer_does() {
    let mut direct = Ssd1677::new(Ssd1677Config::default(), ONE_GIGAHERTZ).unwrap();
    let mut bus = BitBang::new();
    let steps: [(u8, &[u8]); 4] = [
        (0x11, &[0x03]),
        (0x4E, &[0x08, 0x00]),
        (0x4F, &[0x02, 0x00]),
        (0x24, &[0x0F, 0xA5, 0x3C]),
    ];
    for (command, data) in steps {
        bus.command_write(command, data);
        direct.write_command(Cycles(0), command);
        for &byte in data {
            direct.write_data(Cycles(0), byte);
        }
    }
    let lines = bus.shared.borrow();
    assert_eq!(
        lines.chip().ram(Plane::BlackWhite),
        direct.ram(Plane::BlackWhite)
    );
    assert_eq!(
        &lines.chip().ram(Plane::BlackWhite)[2 * 120 + 1..2 * 120 + 4],
        [0x0F, 0xA5, 0x3C]
    );
}

#[test]
fn a_read_ends_at_data_command_low_and_rising_edges_during_it_write_nothing() {
    let mut bus = BitBang::new();
    let at = bus.tick();
    bus.dc.driven(Some(Level::Low), at);
    bus.target.select(true, at);
    bus.write(0x2F);
    let at = bus.tick();
    bus.dc.driven(Some(Level::High), at);
    bus.data_out.driven(Some(Level::High), at);
    bus.read();
    let at = bus.tick();
    bus.dc.driven(Some(Level::Low), at);
    assert_eq!(
        bus.data_in.level(at),
        None,
        "D/C# low: the chip lets go of SDA"
    );
    bus.write(0x7F);
    assert_eq!(
        bus.shared.borrow_mut().chip_mut().take_diagnostics(),
        [],
        "the read's clocks wrote nothing; 0x7F is a no-op"
    );
}
