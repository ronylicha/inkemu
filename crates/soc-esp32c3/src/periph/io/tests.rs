use super::*;
use crate::periph::pump;
use std::cell::RefCell;
use std::rc::Rc;

/// `IO_MUX_GPIOn` for a pad in the GPIO function (function 1), input enabled, with `extra` bits.
const GPIO_PAD: u32 = (1 << 12) | FUN_IE;
const SOURCE_BIT: u64 = 1 << 16;

fn write(io: &mut IoMatrix, ctx: &mut Ctx, offset: u32, value: u32) {
    assert!(io.write(ctx, offset, value), "{offset:#x}");
}

fn read(io: &mut IoMatrix, ctx: &mut Ctx, offset: u32) -> u32 {
    io.read(ctx, offset)
        .unwrap_or_else(|| panic!("{offset:#x} is not modelled"))
}

fn pin(n: u32) -> u32 {
    PIN0 + 4 * n
}
fn out_sel(n: u32) -> u32 {
    FUNC0_OUT_SEL_CFG + 4 * n
}
fn in_sel(m: u32) -> u32 {
    FUNC0_IN_SEL_CFG + 4 * m
}
fn pad_reg(n: u32) -> u32 {
    PAD0 + 4 * n
}

/// Pad `n` as a plain GPIO output at `level`: its level first, then its output enabled.
fn output(io: &mut IoMatrix, ctx: &mut Ctx, n: u32, high: bool) {
    write(io, ctx, pad_reg(n), GPIO_PAD);
    write(io, ctx, out_sel(n), pad::GPIO_OUT_SIGNAL);
    write(io, ctx, if high { OUT_W1TS } else { OUT_W1TC }, 1 << n);
    write(io, ctx, ENABLE_W1TS, 1 << n);
}

/// A chip that watches one pin and records what it heard.
#[derive(Debug, Default)]
struct Watcher {
    heard: Vec<Option<Level>>,
}

impl GpioOutput for Watcher {
    fn driven(&mut self, level: Option<Level>, _at: Nanos) {
        self.heard.push(level);
    }
}

/// A chip that drives a pin at a level the test changes, until `until` when given.
#[derive(Debug)]
struct Driver {
    level: Option<Level>,
    /// The level from `at` on, and that instant.
    later: Option<(Nanos, Option<Level>)>,
}

impl GpioInput for Driver {
    fn level(&mut self, at: Nanos) -> Option<Level> {
        match self.later {
            Some((from, level)) if at >= from => level,
            _ => self.level,
        }
    }

    fn next_change(&mut self, at: Nanos) -> Option<Nanos> {
        self.later.map(|(from, _)| from).filter(|&from| from > at)
    }
}

fn driver(level: Option<Level>) -> Rc<RefCell<Driver>> {
    Rc::new(RefCell::new(Driver { level, later: None }))
}

#[test]
fn the_registers_start_at_their_reset_values_and_the_six_flash_selectors_store_as_before() {
    // The ROM's flash-boot path and `esp_restart_noos` read back and write these (FB §B.1).
    let ctx = &mut Ctx::default();
    let mut io = IoMatrix::new(0x8);
    assert_eq!(read(&mut io, ctx, STRAP), 0x8);
    assert!(!io.write(ctx, STRAP, 0), "STRAP is read-only");
    assert!(!io.write(ctx, IN, 0), "IN is read-only");
    assert_eq!(read(&mut io, ctx, ENABLE), 0);
    for m in 0..6 {
        assert_eq!(read(&mut io, ctx, in_sel(m)), 0);
        write(&mut io, ctx, in_sel(m), 0x30);
        assert_eq!(read(&mut io, ctx, in_sel(m)), 0x30);
    }
    assert_eq!(read(&mut io, ctx, out_sel(0)), 0x80);
    assert_eq!(read(&mut io, ctx, pad_reg(20)), 0xB00);
    assert_eq!(
        read(&mut io, ctx, io_mux("PIN_CTRL")),
        0x7FF,
        "stored with the SVD's reset value"
    );
    for strobe in [
        OUT_W1TS,
        OUT_W1TC,
        ENABLE_W1TS,
        ENABLE_W1TC,
        STATUS_W1TS,
        STATUS_W1TC,
    ] {
        assert_eq!(read(&mut io, ctx, strobe), 0, "{strobe:#x}");
    }
    assert_eq!(io.read(ctx, gpio("CLOCK_GATE")), None, "not modelled");
    assert_eq!(ctx.take_unsupported(), None);
}

#[test]
fn at_reset_only_the_uart_tx_pad_is_driven_and_every_pad_reads_its_pull_up() {
    // GPIO21 is U0TXD in function 0, idle high; the others are not driven, and the SVD's reset
    // value `0xB00` enables the input and the pull-up of every pad.
    let ctx = &mut Ctx::default();
    let mut io = IoMatrix::new(0x8);
    assert_eq!(read(&mut io, ctx, IN), 0x3F_FFFF);
    assert_eq!(io.driven[21], Some(Level::High));
    assert!(io.driven[..21].iter().all(Option::is_none));
    write(&mut io, ctx, pad_reg(9), GPIO_PAD);
    assert_eq!(
        read(&mut io, ctx, IN) & (1 << 9),
        0,
        "the pull-up off: floating"
    );
}

#[test]
fn a_plain_gpio_output_reaches_the_chip_watching_it_and_reads_back_in_in() {
    let ctx = &mut Ctx::default();
    let mut io = IoMatrix::new(0x8);
    let watcher = Rc::new(RefCell::new(Watcher::default()));
    io.attach_output(ctx, 4, Box::new(Rc::clone(&watcher)))
        .unwrap();
    output(&mut io, ctx, 4, true);
    assert_eq!(read(&mut io, ctx, IN) & (1 << 4), 1 << 4);
    write(&mut io, ctx, OUT_W1TC, 1 << 4);
    assert_eq!(read(&mut io, ctx, IN) & (1 << 4), 0);
    write(&mut io, ctx, ENABLE_W1TC, 1 << 4);
    assert_eq!(read(&mut io, ctx, OUT) & (1 << 4), 0);
    assert_eq!(
        watcher.borrow().heard,
        [None, Some(Level::High), Some(Level::Low), None],
        "told once at the attachment, then at each change"
    );
}

#[test]
fn a_pad_reads_the_chip_driving_it_then_its_pull_and_zero_without_its_input_enabled() {
    let ctx = &mut Ctx::default();
    let mut io = IoMatrix::new(0x8);
    let chip = driver(Some(Level::Low));
    io.attach_input(ctx, 6, Box::new(Rc::clone(&chip))).unwrap();
    write(&mut io, ctx, pad_reg(6), GPIO_PAD | pad::FUN_WPU);
    assert_eq!(read(&mut io, ctx, IN) & (1 << 6), 0, "the chip drives low");
    chip.borrow_mut().level = None;
    assert_eq!(read(&mut io, ctx, IN) & (1 << 6), 1 << 6, "the pull-up");
    write(&mut io, ctx, pad_reg(6), GPIO_PAD | pad::FUN_WPD);
    assert_eq!(read(&mut io, ctx, IN) & (1 << 6), 0, "the pull-down");
    chip.borrow_mut().level = Some(Level::High);
    write(&mut io, ctx, pad_reg(6), 1 << 12);
    assert_eq!(read(&mut io, ctx, IN) & (1 << 6), 0, "FUN_IE clear: 0");
    assert_eq!(
        io.attach_input(ctx, 6, Box::new(driver(None))),
        Err(AttachError::InputTaken { pin: 6 })
    );
}

#[test]
fn an_open_drain_pad_releases_its_high_level_to_the_pull() {
    let ctx = &mut Ctx::default();
    let mut io = IoMatrix::new(0x8);
    let watcher = Rc::new(RefCell::new(Watcher::default()));
    io.attach_output(ctx, 20, Box::new(Rc::clone(&watcher)))
        .unwrap();
    output(&mut io, ctx, 20, true);
    write(&mut io, ctx, pin(20), pad::PAD_DRIVER);
    assert_eq!(watcher.borrow().heard.last(), Some(&None), "released");
    write(&mut io, ctx, OUT_W1TC, 1 << 20);
    assert_eq!(watcher.borrow().heard.last(), Some(&Some(Level::Low)));
}

#[test]
fn an_edge_on_a_pad_sets_its_status_and_raises_the_gpio_source_while_enabled() {
    // `gpio_set_intr_type(ANYEDGE)` and `gpio_intr_enable`: INT_TYPE 3, INT_ENA 1.
    let ctx = &mut Ctx::default();
    let mut io = IoMatrix::new(0x8);
    let chip = driver(Some(Level::High));
    io.attach_input(ctx, 3, Box::new(Rc::clone(&chip))).unwrap();
    write(&mut io, ctx, pad_reg(3), GPIO_PAD);
    write(
        &mut io,
        ctx,
        pin(3),
        (3 << INT_TYPE_SHIFT) | (1 << INT_ENA_SHIFT),
    );
    assert_eq!(read(&mut io, ctx, STATUS), 0, "no edge yet");
    chip.borrow_mut().level = Some(Level::Low);
    io.settle(ctx);
    assert_eq!(read(&mut io, ctx, STATUS), 1 << 3);
    assert_eq!(read(&mut io, ctx, PCPU_INT), 1 << 3);
    assert_eq!(ctx.sources() & SOURCE_BIT, SOURCE_BIT);
    write(&mut io, ctx, STATUS_W1TC, 1 << 3);
    assert_eq!(read(&mut io, ctx, STATUS), 0, "an edge does not come back");
    assert_eq!(ctx.sources() & SOURCE_BIT, 0);
    // A rising-edge trigger ignores a fall.
    write(
        &mut io,
        ctx,
        pin(3),
        (1 << INT_TYPE_SHIFT) | (1 << INT_ENA_SHIFT),
    );
    chip.borrow_mut().level = Some(Level::High);
    io.settle(ctx);
    chip.borrow_mut().level = Some(Level::Low);
    io.settle(ctx);
    assert_eq!(read(&mut io, ctx, STATUS), 1 << 3, "the rise only");
    // Without INT_ENA the status is set but the source stays low.
    write(&mut io, ctx, STATUS_W1TC, 1 << 3);
    write(&mut io, ctx, pin(3), 2 << INT_TYPE_SHIFT);
    chip.borrow_mut().level = Some(Level::High);
    io.settle(ctx);
    chip.borrow_mut().level = Some(Level::Low);
    io.settle(ctx);
    assert_eq!(read(&mut io, ctx, STATUS), 1 << 3);
    assert_eq!(read(&mut io, ctx, PCPU_INT), 0);
    assert_eq!(ctx.sources() & SOURCE_BIT, 0);
}

#[test]
fn a_level_trigger_sets_its_status_again_while_the_level_holds() {
    let ctx = &mut Ctx::default();
    let mut io = IoMatrix::new(0x8);
    let chip = driver(Some(Level::High));
    io.attach_input(ctx, 2, Box::new(Rc::clone(&chip))).unwrap();
    write(&mut io, ctx, pad_reg(2), GPIO_PAD);
    write(
        &mut io,
        ctx,
        pin(2),
        (5 << INT_TYPE_SHIFT) | (1 << INT_ENA_SHIFT),
    );
    assert_eq!(read(&mut io, ctx, STATUS), 1 << 2);
    assert_eq!(read(&mut io, ctx, STATUS_NEXT), 1 << 2);
    write(&mut io, ctx, STATUS_W1TC, 1 << 2);
    assert_eq!(
        read(&mut io, ctx, STATUS),
        1 << 2,
        "the level is still high"
    );
    chip.borrow_mut().level = Some(Level::Low);
    write(&mut io, ctx, STATUS_W1TC, 1 << 2);
    assert_eq!(read(&mut io, ctx, STATUS), 0);
    assert_eq!(read(&mut io, ctx, STATUS_NEXT), 0);
}

#[test]
fn the_pins_are_read_again_at_the_instant_a_chip_announces_while_an_interrupt_waits() {
    // FreeInk waits for BUSY's fall with an interrupt on both edges (P §A): the chip says when.
    let ctx = &mut Ctx::default();
    let mut io = IoMatrix::new(0x8);
    let chip = Rc::new(RefCell::new(Driver {
        level: Some(Level::High),
        later: Some((Nanos(1_000), Some(Level::Low))),
    }));
    io.attach_input(ctx, 6, Box::new(Rc::clone(&chip))).unwrap();
    write(&mut io, ctx, pad_reg(6), GPIO_PAD);
    assert_eq!(ctx.next_wake(), None, "no interrupt configured: no wake-up");
    write(
        &mut io,
        ctx,
        pin(6),
        (3 << INT_TYPE_SHIFT) | (1 << INT_ENA_SHIFT),
    );
    // 1 µs is 160 cycles of the time base.
    assert_eq!(ctx.next_wake(), Some(Cycles(160)));
    ctx.sleep_until(Cycles(160));
    pump(ctx, |ctx, event| {
        assert_eq!(event, Event::Pins);
        io.wake(ctx);
    });
    assert_eq!(read(&mut io, ctx, STATUS), 1 << 6);
    assert_eq!(ctx.sources() & SOURCE_BIT, SOURCE_BIT);
    assert_eq!(ctx.next_wake(), None, "the chip announces nothing more");
}

/// A panel whose reset line starts a 2 µs piece of work, which drives its BUSY line high while it
/// lasts: a clocked chip shared between its lines.
#[derive(Debug, Default)]
struct Busy {
    until: Option<Nanos>,
    advanced: Vec<u64>,
}

impl GpioOutput for Busy {
    fn driven(&mut self, level: Option<Level>, at: Nanos) {
        if level == Some(Level::High) {
            self.until = Some(Nanos(at.get() + 2_000));
        }
    }
}

impl GpioInput for Busy {
    fn level(&mut self, _at: Nanos) -> Option<Level> {
        Some(Level::from_bool(self.until.is_some()))
    }
}

impl Clocked for Busy {
    fn next_deadline(&mut self, _at: Nanos) -> Option<Nanos> {
        self.until
    }

    fn advance(&mut self, at: Nanos) {
        self.advanced.push(at.get());
        if self.until.is_some_and(|until| until <= at) {
            self.until = None;
        }
    }
}

#[test]
fn a_clocked_chip_is_asked_its_deadline_after_a_call_on_its_lines_and_advanced_there() {
    // No interrupt on BUSY: only the clocked attachment brings the chip to the end of its work.
    let ctx = &mut Ctx::default();
    let mut io = IoMatrix::new(0x8);
    let chip = Rc::new(RefCell::new(Busy::default()));
    io.attach_output(ctx, 5, Box::new(Rc::clone(&chip)))
        .unwrap();
    io.attach_input(ctx, 6, Box::new(Rc::clone(&chip))).unwrap();
    io.attach_clocked(ctx, Box::new(Rc::clone(&chip)));
    write(&mut io, ctx, pad_reg(6), GPIO_PAD);
    assert_eq!(ctx.next_wake(), None, "no work: no wake-up");
    output(&mut io, ctx, 5, true);
    assert_eq!(read(&mut io, ctx, IN) & (1 << 6), 1 << 6, "busy");
    // 2 µs is 320 cycles of the time base.
    assert_eq!(ctx.next_wake(), Some(Cycles(320)));
    ctx.sleep_until(Cycles(320));
    pump(ctx, |ctx, event| {
        assert_eq!(event, Event::Pins);
        io.wake(ctx);
    });
    assert_eq!(chip.borrow().advanced, [2_000], "at its deadline, once");
    assert_eq!(ctx.next_wake(), None, "the chip has nothing more to do");
    assert_eq!(read(&mut io, ctx, IN) & (1 << 6), 0, "no longer busy");
}

#[test]
fn a_read_of_the_pins_past_a_deadline_advances_the_chip_before_it_reads_its_line() {
    // The instruction that reads `GPIO.IN` crosses the end of the chip's work: the read must see
    // the chip after its advance, before the wake-up armed for the deadline is delivered.
    let ctx = &mut Ctx::default();
    let mut io = IoMatrix::new(0x8);
    let chip = Rc::new(RefCell::new(Busy::default()));
    io.attach_output(ctx, 5, Box::new(Rc::clone(&chip)))
        .unwrap();
    io.attach_input(ctx, 6, Box::new(Rc::clone(&chip))).unwrap();
    io.attach_clocked(ctx, Box::new(Rc::clone(&chip)));
    write(&mut io, ctx, pad_reg(6), GPIO_PAD);
    output(&mut io, ctx, 5, true);
    ctx.sleep_until(Cycles(330));
    assert_eq!(read(&mut io, ctx, IN) & (1 << 6), 0, "no longer busy");
    assert_eq!(chip.borrow().advanced, [2_062], "advanced at the read");
}

#[test]
fn attaching_a_watcher_asks_the_deadline_its_first_hearing_gave_a_clocked_chip() {
    let ctx = &mut Ctx::default();
    let mut io = IoMatrix::new(0x8);
    let chip = Rc::new(RefCell::new(Busy::default()));
    io.attach_clocked(ctx, Box::new(Rc::clone(&chip)));
    output(&mut io, ctx, 5, true);
    assert_eq!(ctx.next_wake(), None, "no work yet");
    // The watcher hears GPIO5 high at once, which starts the chip's 2 µs of work.
    io.attach_output(ctx, 5, Box::new(Rc::clone(&chip)))
        .unwrap();
    assert_eq!(ctx.next_wake(), Some(Cycles(320)));
}

#[test]
fn a_signal_of_a_peripheral_the_model_lacks_and_an_nmi_are_refused_by_name() {
    let ctx = &mut Ctx::default();
    let mut io = IoMatrix::new(0x8);
    write(&mut io, ctx, pad_reg(5), GPIO_PAD);
    write(&mut io, ctx, out_sel(5), 45);
    assert_eq!(
        ctx.take_unsupported(),
        Some(Unsupported::PadSignal { pin: 5, signal: 45 })
    );
    write(&mut io, ctx, ENABLE_W1TS, 1 << 5);
    assert_eq!(io.driven[5], None, "not driven");
    write(&mut io, ctx, pin(5), 3 << INT_ENA_SHIFT);
    assert_eq!(
        ctx.take_unsupported(),
        Some(Unsupported::PadInterrupt { pin: 5, int_ena: 3 })
    );
    for modelled in [pad::GPIO_OUT_SIGNAL, pad::U0TXD, pad::I2C_SDA, pad::FSPICLK] {
        write(&mut io, ctx, out_sel(5), modelled);
        assert_eq!(ctx.take_unsupported(), None, "{modelled}");
    }
}

/// A chip on SPI that records what it hears and answers each byte plus one.
#[derive(Debug, Default)]
struct Target {
    calls: Vec<String>,
}

impl SpiTarget for Target {
    fn select(&mut self, selected: bool, _at: Nanos) {
        self.calls.push(format!("select {selected}"));
    }

    fn transfer(&mut self, dc: Option<Level>, mosi: &[u8], miso: &mut [u8], _at: Nanos) {
        self.calls.push(format!("transfer {dc:?} {mosi:02x?}"));
        for (out, sent) in miso.iter_mut().zip(mosi) {
            *out = sent.wrapping_add(1);
        }
    }
}

/// The X4's display bus as `CrossPoint` routes it (P §A): SCLK 8, MOSI 10, MISO 7, CS 21 and DC 4 as
/// GPIO outputs, both high.
fn x4_bus(io: &mut IoMatrix, ctx: &mut Ctx) {
    for (pad, signal) in [(8, pad::FSPICLK), (10, pad::FSPID)] {
        write(io, ctx, pad_reg(pad), GPIO_PAD);
        write(io, ctx, out_sel(pad), signal);
    }
    write(io, ctx, pad_reg(7), GPIO_PAD | pad::FUN_WPU);
    write(io, ctx, in_sel(pad::FSPIQ), (1 << 6) | 7);
    output(io, ctx, 21, true);
    output(io, ctx, 4, true);
}

const DISPLAY: SpiWires = SpiWires {
    sclk: 8,
    mosi: 10,
    miso: Some(7),
    cs: 21,
    dc: Some(4),
};

#[test]
fn a_transfer_reaches_the_selected_chip_with_its_dc_and_its_answer_comes_back() {
    let ctx = &mut Ctx::default();
    let mut io = IoMatrix::new(0x8);
    x4_bus(&mut io, ctx);
    let target = Rc::new(RefCell::new(Target::default()));
    io.attach_spi(ctx, DISPLAY, Box::new(Rc::clone(&target)))
        .unwrap();
    assert_eq!(
        io.spi_transfer(ctx, &[0x12]),
        [0xFF],
        "not selected: the pull-up of MISO"
    );
    write(&mut io, ctx, OUT_W1TC, 1 << 21);
    write(&mut io, ctx, OUT_W1TC, 1 << 4);
    assert_eq!(io.spi_transfer(ctx, &[0x12]), [0x13]);
    write(&mut io, ctx, OUT_W1TS, 1 << 4);
    assert_eq!(io.spi_transfer(ctx, &[0x20, 0x30]), [0x21, 0x31]);
    write(&mut io, ctx, OUT_W1TS, 1 << 21);
    assert_eq!(
        target.borrow().calls,
        [
            "select true",
            "transfer Some(Low) [12]",
            "transfer Some(High) [20, 30]",
            "select false",
        ]
    );
}

/// A key of the board (input 0) that the chip on the bus answers with: 1 pressed, 0 released.
#[derive(Debug, Default)]
struct KeyOnTheBus {
    pressed: bool,
}

impl BoardInputs for KeyOnTheBus {
    fn check(&self, _input: BoardInput) -> Result<(), emu_core::BoardInputError> {
        Ok(())
    }

    fn apply(&mut self, input: BoardInput, _at: Nanos) {
        self.pressed = input.value == 1;
    }
}

impl SpiTarget for KeyOnTheBus {
    fn select(&mut self, _selected: bool, _at: Nanos) {}

    fn transfer(&mut self, _dc: Option<Level>, _mosi: &[u8], miso: &mut [u8], _at: Nanos) {
        miso.fill(u8::from(self.pressed));
    }
}

/// E1, its branch of the bus: an input of the board that acts at the very instant a transfer
/// ends is seen by the chip, though its own event has not run yet (a live run arms the two in
/// another order than its replay).
#[test]
fn a_transfer_at_the_instant_a_board_input_acts_sees_it() {
    let ctx = &mut Ctx::default();
    let mut io = IoMatrix::new(0x8);
    x4_bus(&mut io, ctx);
    let key = Rc::new(RefCell::new(KeyOnTheBus::default()));
    io.attach_board_inputs(Box::new(Rc::clone(&key)));
    io.attach_spi(ctx, DISPLAY, Box::new(Rc::clone(&key)))
        .unwrap();
    write(&mut io, ctx, OUT_W1TC, 1 << 21);
    let pressed = BoardInput {
        target: 0,
        value: 1,
    };
    io.queue_board_input(ctx, ctx.now(), pressed).unwrap();
    // The instant it acts at: its event is armed, not run.
    ctx.advance_time_base(1);
    assert_eq!(io.board_inputs_pending(), 1);
    assert_eq!(io.spi_transfer(ctx, &[0]), [1]);
    assert_eq!(io.board_inputs_pending(), 0);
}

#[test]
fn a_chip_whose_clock_or_mosi_pin_carries_no_spi_signal_hears_nothing() {
    let ctx = &mut Ctx::default();
    let mut io = IoMatrix::new(0x8);
    x4_bus(&mut io, ctx);
    let target = Rc::new(RefCell::new(Target::default()));
    let elsewhere = SpiWires { sclk: 9, ..DISPLAY };
    io.attach_spi(ctx, elsewhere, Box::new(Rc::clone(&target)))
        .unwrap();
    write(&mut io, ctx, OUT_W1TC, 1 << 21);
    assert_eq!(io.spi_transfer(ctx, &[1]), [0xFF]);
    assert_eq!(target.borrow().calls, ["select true"]);
}

#[test]
fn a_hardware_chip_select_routed_to_the_pin_selects_the_chip() {
    let ctx = &mut Ctx::default();
    let mut io = IoMatrix::new(0x8);
    x4_bus(&mut io, ctx);
    write(&mut io, ctx, out_sel(21), pad::FSPICS0);
    let target = Rc::new(RefCell::new(Target::default()));
    io.attach_spi(ctx, DISPLAY, Box::new(Rc::clone(&target)))
        .unwrap();
    io.set_signal(ctx, pad::FSPICS0, Level::Low);
    assert_eq!(io.spi_transfer(ctx, &[7]), [8]);
    io.set_signal(ctx, pad::FSPICS0, Level::High);
    assert_eq!(
        target.borrow().calls,
        ["select true", "transfer Some(High) [07]", "select false"]
    );
}

/// A voltage the test sets.
#[derive(Debug)]
struct Voltage(u32);

impl AnalogSource for Voltage {
    fn millivolts(&mut self, _at: Nanos) -> u32 {
        self.0
    }
}

#[test]
fn a_pin_reads_its_analog_chip_else_its_level_as_a_voltage() {
    let ctx = &mut Ctx::default();
    let mut io = IoMatrix::new(0x8);
    io.attach_analog(1, Box::new(Voltage(1650))).unwrap();
    assert_eq!(io.millivolts(ctx, 1), 1650);
    assert_eq!(
        io.millivolts(ctx, 2),
        PAD_SUPPLY_MV,
        "pulled up, as at reset"
    );
    // As `adc_oneshot_config_channel` leaves an analog pad: no pull, input disabled (TRM §5.7).
    write(&mut io, ctx, pad_reg(2), 1 << 12);
    assert_eq!(io.millivolts(ctx, 2), 0, "floating");
    assert_eq!(
        io.attach_analog(1, Box::new(Voltage(0))),
        Err(AttachError::AnalogTaken { pin: 1 })
    );
    assert_eq!(
        io.attach_analog(6, Box::new(Voltage(0))),
        Err(AttachError::NoAnalogChannel { pin: 6 })
    );
    assert_eq!(
        io.attach_analog(22, Box::new(Voltage(0))),
        Err(AttachError::NoSuchPin { pin: 22 })
    );
    let wires = SpiWires { cs: 30, ..DISPLAY };
    assert_eq!(
        io.attach_spi(ctx, wires, Box::new(Target::default())),
        Err(AttachError::NoSuchPin { pin: 30 })
    );
    assert!(
        AttachError::NoSuchPin { pin: 30 }
            .to_string()
            .contains("GPIO0 to GPIO21")
    );
}

#[test]
fn a_reset_returns_the_registers_to_their_values_and_keeps_the_chips_which_hear_it() {
    let ctx = &mut Ctx::default();
    let mut io = IoMatrix::new(0x8);
    let watcher = Rc::new(RefCell::new(Watcher::default()));
    io.attach_output(ctx, 4, Box::new(Rc::clone(&watcher)))
        .unwrap();
    output(&mut io, ctx, 4, true);
    write(&mut io, ctx, pin(4), 5 << INT_TYPE_SHIFT);
    io.reset(ctx);
    assert_eq!(read(&mut io, ctx, ENABLE), 0);
    assert_eq!(read(&mut io, ctx, pin(4)), 0);
    assert_eq!(read(&mut io, ctx, pad_reg(4)), 0xB00);
    assert_eq!(watcher.borrow().heard, [None, Some(Level::High), None]);
    output(&mut io, ctx, 4, false);
    assert_eq!(
        watcher.borrow().heard.last(),
        Some(&Some(Level::Low)),
        "still attached"
    );
}

/// A held pad keeps its output and its pulls through a reset of the digital domain and ignores its
/// registers until released (`gpio_hold_en`, ADR 0013).
#[test]
fn a_held_pad_keeps_its_output_through_a_reset_and_follows_its_registers_once_released() {
    let ctx = &mut Ctx::default();
    let mut io = IoMatrix::new(0x8);
    let watcher = Rc::new(RefCell::new(Watcher::default()));
    io.attach_output(ctx, 13, Box::new(Rc::clone(&watcher)))
        .unwrap();
    output(&mut io, ctx, 13, false);
    assert_eq!(watcher.borrow().heard.last(), Some(&Some(Level::Low)));
    io.set_hold(ctx, 1 << 13);
    write(&mut io, ctx, OUT_W1TS, 1 << 13);
    assert_eq!(
        watcher.borrow().heard.last(),
        Some(&Some(Level::Low)),
        "a held pad ignores its registers"
    );
    io.reset(ctx);
    assert_eq!(
        watcher.borrow().heard.last(),
        Some(&Some(Level::Low)),
        "and keeps its output through a reset"
    );
    io.set_hold(ctx, 0);
    assert_eq!(
        watcher.borrow().heard.last(),
        Some(&None),
        "released, it follows its registers, reset: not an output"
    );
}

/// The levels of the wake pins of a deep sleep: a chip's, else the pad's pull, read without the
/// pads being worked out.
#[test]
fn the_wake_pins_read_their_chip_or_their_pull() {
    let ctx = &mut Ctx::default();
    let mut io = IoMatrix::new(0x8);
    let key = driver(None);
    io.attach_input(ctx, 3, Box::new(Rc::clone(&key))).unwrap();
    write(&mut io, ctx, pad_reg(3), GPIO_PAD | (1 << 8));
    let low_on_3 = [None, None, None, Some(Level::Low), None, None];
    assert_eq!(io.wake_levels(ctx, &low_on_3), 0, "released: the pull-up");
    key.borrow_mut().level = Some(Level::Low);
    assert_eq!(io.wake_levels(ctx, &low_on_3), 1 << 3);
    assert_eq!(io.wake_pins_change(ctx, &low_on_3), None);
    key.borrow_mut().later = Some((Nanos(5_000), None));
    let change = io.wake_pins_change(ctx, &low_on_3).unwrap();
    assert_eq!(change, crate::clock::TIME_BASE.nanos_to_cycles(5_000));
    let high_on_2 = [None, None, Some(Level::High), None, None, None];
    write(&mut io, ctx, pad_reg(2), GPIO_PAD);
    assert_eq!(io.wake_levels(ctx, &high_on_2), 0, "no chip, no pull: low");
}
