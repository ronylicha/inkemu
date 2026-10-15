use super::*;
use crate::periph::pump;
use emu_core::Cycles;

const SOURCE_BIT: u64 = 1 << 29;
const COMD1: u32 = COMD0 + 4;
const COMD2: u32 = COMD0 + 8;
const COMD3: u32 = COMD0 + 12;
const MASTER_EVENTS: u32 = NACK | (1 << 8) | TRANS_COMPLETE | (1 << 5) | END_DETECT;

fn command(op: u32, bytes: u32, ack_en: bool) -> u32 {
    (op << OP_SHIFT) | bytes | if ack_en { ACK_EN } else { 0 }
}

/// I2C0 as ESP-IDF's `i2c_master` sets it up for 400 kHz from the crystal (`i2c_ll.h:93-117`):
/// divider 1, half period 50: low 49, high 27, wait high 23 (one period, 100 cycles of 40 MHz);
/// START and STOP setup and hold 49 each.
fn master(ctx: &mut Ctx) -> I2c0 {
    let mut i2c = I2c0::new();
    configure(&mut i2c, ctx);
    i2c
}

/// Writes the registers of [`master`] to `i2c`.
fn configure(i2c: &mut I2c0, ctx: &mut Ctx) {
    for (offset, value) in [
        (CTR, MS_MODE | (1 << 8)),
        (CLK_CONF, 0),
        (SCL_LOW_PERIOD, 49),
        (SCL_HIGH_PERIOD, 0x1B | (23 << WAIT_HIGH_SHIFT)),
        (SCL_RSTART_SETUP, 49),
        (SCL_START_HOLD, 49),
        (SCL_STOP_SETUP, 49),
        (SCL_STOP_HOLD, 49),
        (INT_ENA, MASTER_EVENTS),
    ] {
        assert!(i2c.write(ctx, offset, value), "{offset:#x}");
    }
}

/// Runs the sequence under way on a bus without chips: one wake-up, at its end.
fn run(i2c: &mut I2c0, ctx: &mut Ctx) {
    let end = ctx.next_wake().expect("a sequence runs");
    ctx.sleep_until(end);
    let mut io = IoMatrix::new(0);
    pump(ctx, |ctx, event| {
        assert_eq!(event, Event::I2c0);
        i2c.wake(ctx, &mut io);
    });
}

fn read(i2c: &mut I2c0, ctx: &mut Ctx, offset: u32) -> u32 {
    i2c.read(ctx, offset).unwrap()
}

#[test]
fn the_x3_probe_gets_a_nack_on_its_address_byte_as_on_an_x4() {
    // `readReg8(0x68, 0)`: START, the address byte and the register, with the ACK checked.
    let ctx = &mut Ctx::default();
    let mut i2c = master(ctx);
    for byte in [0x68 << 1, 0x00] {
        assert!(i2c.write(ctx, DATA, byte));
    }
    assert!(i2c.write(ctx, COMD0, command(OP_RSTART, 0, false)));
    assert!(i2c.write(ctx, COMD1, command(OP_WRITE, 2, true)));
    assert!(i2c.write(ctx, COMD2, command(OP_END, 0, false)));
    assert!(i2c.write(ctx, CTR, MS_MODE | TRANS_START));
    assert_eq!(read(&mut i2c, ctx, SR) & BUS_BUSY, BUS_BUSY, "running");
    // START (100 cycles of 40 MHz), one byte (900), the STOP the NACK generates (100): 1 100
    // cycles of the crystal, 4 400 of the time base, 27.5 µs.
    assert_eq!(ctx.next_wake(), Some(Cycles(4_400)));
    run(&mut i2c, ctx);
    let raw = read(&mut i2c, ctx, INT_RAW);
    assert_eq!(raw & (NACK | TRANS_COMPLETE | END_DETECT), NACK);
    assert_eq!(ctx.sources() & SOURCE_BIT, SOURCE_BIT);
    let status = read(&mut i2c, ctx, SR);
    assert_eq!(
        status & (BUS_BUSY | RESP_REC),
        RESP_REC,
        "a STOP on the bus"
    );
    assert_eq!(
        (status >> TXFIFO_CNT_SHIFT) & 0x3F,
        1,
        "the register byte stays"
    );
    assert_eq!(read(&mut i2c, ctx, COMD0) & DONE, DONE);
    assert_eq!(read(&mut i2c, ctx, COMD1) & DONE, 0, "stopped by the NACK");
    // The driver clears the NACK, writes a STOP and starts it (`i2c_master.c:560-568`).
    assert!(i2c.write(ctx, INT_CLR, NACK));
    assert_eq!(ctx.sources() & SOURCE_BIT, 0);
    assert!(i2c.write(ctx, COMD0, command(OP_STOP, 0, false)));
    assert!(i2c.write(ctx, CTR, MS_MODE | TRANS_START));
    run(&mut i2c, ctx);
    assert_eq!(
        read(&mut i2c, ctx, INT_RAW) & TRANS_COMPLETE,
        TRANS_COMPLETE
    );
    assert_eq!(read(&mut i2c, ctx, SR) & BUS_BUSY, 0);
}

#[test]
fn without_the_ack_check_the_bytes_go_and_a_read_gets_the_pulled_up_lines() {
    let ctx = &mut Ctx::default();
    let mut i2c = master(ctx);
    assert!(i2c.write(ctx, DATA, 0x55 << 1 | 1));
    assert!(i2c.write(ctx, COMD0, command(OP_RSTART, 0, false)));
    assert!(i2c.write(ctx, COMD1, command(OP_WRITE, 1, false)));
    assert!(i2c.write(ctx, COMD2, command(OP_READ, 2, false)));
    assert!(i2c.write(ctx, COMD3, command(OP_STOP, 0, false)));
    assert!(i2c.write(ctx, CTR, MS_MODE | TRANS_START));
    run(&mut i2c, ctx);
    assert_eq!(read(&mut i2c, ctx, INT_RAW) & NACK, 0);
    assert_eq!(
        read(&mut i2c, ctx, INT_RAW) & TRANS_COMPLETE,
        TRANS_COMPLETE
    );
    for n in 0..4 {
        assert_eq!(read(&mut i2c, ctx, COMD0 + 4 * n) & DONE, DONE, "COMD{n}");
    }
    assert_eq!((read(&mut i2c, ctx, SR) >> RXFIFO_CNT_SHIFT) & 0x3F, 2);
    assert_eq!(read(&mut i2c, ctx, DATA), 0xFF);
    assert_eq!(read(&mut i2c, ctx, DATA), 0xFF);
    assert_eq!(read(&mut i2c, ctx, DATA), 0, "empty");
    assert_eq!(read(&mut i2c, ctx, INT_RAW) & RXFIFO_UDF, RXFIFO_UDF);
    assert_eq!(read(&mut i2c, ctx, FIFO_ST) & 0x3FF, 2 | (2 << 5));
}

#[test]
fn an_end_suspends_the_sequence_with_the_bus_held() {
    let ctx = &mut Ctx::default();
    let mut i2c = master(ctx);
    assert!(i2c.write(ctx, COMD0, command(OP_RSTART, 0, false)));
    assert!(i2c.write(ctx, COMD1, command(OP_END, 0, false)));
    assert!(i2c.write(ctx, CTR, MS_MODE | TRANS_START));
    run(&mut i2c, ctx);
    assert_eq!(read(&mut i2c, ctx, INT_RAW) & END_DETECT, END_DETECT);
    assert_eq!(read(&mut i2c, ctx, SR) & BUS_BUSY, BUS_BUSY);
}

#[test]
fn the_fifos_reset_and_a_write_with_too_few_bytes_underflows() {
    let ctx = &mut Ctx::default();
    let mut i2c = master(ctx);
    for byte in 0..40 {
        assert!(i2c.write(ctx, DATA, byte));
    }
    assert_eq!(read(&mut i2c, ctx, INT_RAW) & TXFIFO_OVF, TXFIFO_OVF);
    assert_eq!((read(&mut i2c, ctx, SR) >> TXFIFO_CNT_SHIFT) & 0x3F, 32);
    assert!(i2c.write(ctx, FIFO_CONF, TX_FIFO_RST));
    assert!(i2c.write(ctx, FIFO_CONF, 0));
    assert_eq!((read(&mut i2c, ctx, SR) >> TXFIFO_CNT_SHIFT) & 0x3F, 0);
    assert!(i2c.write(ctx, COMD0, command(OP_WRITE, 1, false)));
    assert!(i2c.write(ctx, CTR, MS_MODE | TRANS_START));
    run(&mut i2c, ctx);
    assert_eq!(
        read(&mut i2c, ctx, INT_RAW) & MST_TXFIFO_UDF,
        MST_TXFIFO_UDF
    );
}

#[test]
fn slave_mode_non_fifo_mode_and_a_foreign_clock_are_refused_by_name() {
    let ctx = &mut Ctx::default();
    let mut i2c = I2c0::new();
    assert!(i2c.write(ctx, CTR, TRANS_START));
    assert_eq!(
        ctx.take_unsupported(),
        Some(Unsupported::I2c {
            refusal: I2cRefusal::Slave
        })
    );
    assert_eq!(ctx.next_wake(), None);
    // Refused, and ended at once: a firmware waiting for the end is not stuck.
    assert_eq!(
        read(&mut i2c, ctx, INT_RAW) & TRANS_COMPLETE,
        TRANS_COMPLETE
    );
    assert_eq!(read(&mut i2c, ctx, SR) & BUS_BUSY, 0);
    let mut i2c = master(ctx);
    assert!(i2c.write(ctx, FIFO_CONF, NONFIFO_EN));
    assert!(i2c.write(ctx, CTR, MS_MODE | TRANS_START));
    assert_eq!(
        ctx.take_unsupported(),
        Some(Unsupported::I2c {
            refusal: I2cRefusal::NonFifo
        })
    );
    assert_eq!(ctx.next_wake(), None);
    assert_eq!(
        read(&mut i2c, ctx, INT_RAW) & TRANS_COMPLETE,
        TRANS_COMPLETE
    );
    assert!(i2c.write(ctx, INT_CLR, TRANS_COMPLETE));
    assert!(i2c.write(ctx, FIFO_CONF, 0));
    assert!(i2c.write(ctx, CLK_CONF, 1 << 20));
    assert!(i2c.write(ctx, COMD0, command(OP_STOP, 0, false)));
    assert!(i2c.write(ctx, CTR, MS_MODE | TRANS_START));
    assert_eq!(
        ctx.take_unsupported(),
        Some(Unsupported::I2c {
            refusal: I2cRefusal::Clock { clk_conf: 1 << 20 }
        })
    );
}

/// A probe of address 0x68 (START, the address byte with its acknowledge checked) started.
fn start_a_probe(i2c: &mut I2c0, ctx: &mut Ctx) {
    assert!(i2c.write(ctx, DATA, 0x68 << 1));
    assert!(i2c.write(ctx, COMD0, command(OP_RSTART, 0, false)));
    assert!(i2c.write(ctx, COMD1, command(OP_WRITE, 1, true)));
    assert!(i2c.write(ctx, COMD2, command(OP_END, 0, false)));
    assert!(i2c.write(ctx, CTR, MS_MODE | TRANS_START));
}

#[test]
fn a_start_during_a_sequence_is_refused_by_name_and_the_sequence_goes_on() {
    let ctx = &mut Ctx::default();
    let mut i2c = master(ctx);
    start_a_probe(&mut i2c, ctx);
    let end = ctx.next_wake();
    assert!(i2c.write(ctx, CTR, MS_MODE | TRANS_START));
    assert_eq!(
        ctx.take_unsupported(),
        Some(Unsupported::I2c {
            refusal: I2cRefusal::StartWhileRunning
        })
    );
    assert_eq!(ctx.next_wake(), end, "the sequence under way keeps its end");
    run(&mut i2c, ctx);
    assert_eq!(read(&mut i2c, ctx, INT_RAW) & NACK, NACK, "and its outcome");
}

#[test]
fn a_reset_of_the_state_machine_abandons_the_sequence_under_way() {
    let ctx = &mut Ctx::default();
    let mut i2c = master(ctx);
    start_a_probe(&mut i2c, ctx);
    assert_eq!(read(&mut i2c, ctx, SR) & BUS_BUSY, BUS_BUSY);
    assert!(i2c.write(ctx, CTR, MS_MODE | FSM_RST));
    assert_eq!(ctx.next_wake(), None, "no end is coming");
    assert_eq!(read(&mut i2c, ctx, SR) & BUS_BUSY, 0, "the bus is idle");
    assert_eq!(read(&mut i2c, ctx, INT_RAW) & (NACK | END_DETECT), 0);
    assert_eq!(read(&mut i2c, ctx, COMD0) & DONE, 0, "nothing was done");
    assert_eq!(read(&mut i2c, ctx, CTR) & FSM_RST, 0, "a strobe");
    assert_eq!(ctx.take_unsupported(), None);
    // A new sequence starts as from idle.
    assert!(i2c.write(ctx, CTR, MS_MODE | TRANS_START));
    assert_eq!(ctx.take_unsupported(), None);
    run(&mut i2c, ctx);
    assert_eq!(read(&mut i2c, ctx, INT_RAW) & NACK, NACK);
}

#[test]
fn the_strobes_read_zero_and_the_bus_clear_ends_at_once() {
    let ctx = &mut Ctx::default();
    let mut i2c = master(ctx);
    assert_eq!(read(&mut i2c, ctx, CTR) & CTR_STROBES, 0);
    assert!(i2c.write(ctx, SCL_SP_CONF, SCL_RST_SLV_EN | (9 << 1)));
    assert_eq!(read(&mut i2c, ctx, SCL_SP_CONF), 9 << 1);
    assert!(!i2c.write(ctx, SR, 0), "read-only");
    assert_eq!(read(&mut i2c, ctx, INT_STATUS), 0);
    assert_eq!(
        i2c.read(ctx, TXFIFO_START_ADDR),
        None,
        "non-FIFO RAM: not modelled"
    );
    i2c.reset(ctx);
    assert_eq!(read(&mut i2c, ctx, CTR), 0x20B, "the SVD's reset value");
}

/// The X3's bus: SCL on GPIO0, SDA on GPIO20.
const WIRES: crate::periph::I2cWires = crate::periph::I2cWires { scl: 0, sda: 20 };

/// A chip at `address` with a register file read and written through a pointer that moves on
/// after each byte, which records what it sees with the instant, in nanoseconds.
#[derive(Debug)]
struct RegisterChip {
    address: u8,
    registers: [u8; 256],
    pointer: u8,
    /// The next byte written is the pointer.
    pointer_next: bool,
    calls: Vec<String>,
}

impl RegisterChip {
    fn at(address: u8) -> std::rc::Rc<std::cell::RefCell<RegisterChip>> {
        let mut registers = [0; 256];
        registers[0x2C] = 70;
        registers[0x2D] = 0;
        std::rc::Rc::new(std::cell::RefCell::new(RegisterChip {
            address,
            registers,
            pointer: 0,
            pointer_next: false,
            calls: Vec::new(),
        }))
    }
}

impl emu_core::I2cTarget for RegisterChip {
    fn address(&mut self, address: u8, read: bool, at: emu_core::Nanos) -> bool {
        self.calls
            .push(format!("address {address:#04x} {read} {}", at.get()));
        self.pointer_next = !read;
        address == self.address
    }

    fn write(&mut self, byte: u8, at: emu_core::Nanos) -> bool {
        self.calls.push(format!("write {byte:#04x} {}", at.get()));
        if self.pointer_next {
            self.pointer = byte;
            self.pointer_next = false;
        } else {
            self.registers[usize::from(self.pointer)] = byte;
            self.pointer = self.pointer.wrapping_add(1);
        }
        true
    }

    fn read(&mut self, acknowledged: bool, at: emu_core::Nanos) -> u8 {
        self.calls.push(format!("read {acknowledged} {}", at.get()));
        let byte = self.registers[usize::from(self.pointer)];
        self.pointer = self.pointer.wrapping_add(1);
        byte
    }

    fn stop(&mut self, at: emu_core::Nanos) {
        self.calls.push(format!("stop {}", at.get()));
    }
}

/// I2C0 with `chip` on `wires`, the matrix routing the bus to `routed`.
fn bus_with(
    ctx: &mut Ctx,
    chip: &std::rc::Rc<std::cell::RefCell<RegisterChip>>,
    wires: crate::periph::I2cWires,
    routed: crate::periph::I2cWires,
) -> (I2c0, IoMatrix) {
    let mut io = IoMatrix::new(0);
    io.route_i2c(ctx, routed, true);
    io.attach_i2c(wires, Box::new(std::rc::Rc::clone(chip)))
        .unwrap();
    let mut i2c = master(ctx);
    i2c.chips_attached();
    (i2c, io)
}

/// Runs the sequence under way on a bus with chips: one wake-up for each condition.
fn run_stepped(i2c: &mut I2c0, ctx: &mut Ctx, io: &mut IoMatrix) {
    while i2c.running.is_some() {
        let next = ctx.next_wake().expect("a condition is due");
        ctx.sleep_until(next);
        pump(ctx, |ctx, event| match event {
            Event::I2c0 => i2c.wake(ctx, io),
            Event::Pins => io.wake(ctx),
            other => panic!("{other:?}"),
        });
    }
}

/// `i2c_master_transmit_receive` of one register byte and two data bytes, as `Wire` sends a
/// gauge read (research `sp3-x3-firmware.md` §2.1): the list in `COMD0..COMD6`.
fn register_read(i2c: &mut I2c0, ctx: &mut Ctx, address: u8, register: u8) {
    for byte in [address << 1, register, address << 1 | 1] {
        assert!(i2c.write(ctx, DATA, u32::from(byte)));
    }
    // The first byte read is answered with an ACK, the last with a NACK (`ack_val`).
    for (n, cmd) in [
        command(OP_RSTART, 0, false),
        command(OP_WRITE, 2, true),
        command(OP_RSTART, 0, false),
        command(OP_WRITE, 1, true),
        command(OP_READ, 1, false),
        command(OP_READ, 1, false) | ACK_VAL,
        command(OP_STOP, 0, false),
    ]
    .into_iter()
    .enumerate()
    {
        let offset = COMD0 + 4 * u32::try_from(n).unwrap();
        assert!(i2c.write(ctx, offset, cmd));
    }
    assert!(i2c.write(ctx, CTR, MS_MODE | TRANS_START));
}

/// ADR 0015: each condition reaches the chip at the instant it ends on the bus (START 100 cycles
/// of 40 MHz, a byte 900, 25 ns a cycle), the register pointer and the repeated START included;
/// the bytes it drives fill the RX FIFO and the sequence completes as on the controller.
#[test]
fn a_register_read_reaches_the_chip_condition_by_condition_and_its_bytes_come_back() {
    let ctx = &mut Ctx::default();
    let chip = RegisterChip::at(0x55);
    let (mut i2c, mut io) = bus_with(ctx, &chip, WIRES, WIRES);
    register_read(&mut i2c, ctx, 0x55, 0x2C);
    assert_eq!(read(&mut i2c, ctx, SR) & BUS_BUSY, BUS_BUSY, "running");
    run_stepped(&mut i2c, ctx, &mut io);
    assert_eq!(
        chip.borrow().calls,
        [
            "address 0x55 false 25000",
            "write 0x2c 47500",
            "address 0x55 true 72500",
            "read true 95000",
            "read false 117500",
            "stop 120000",
        ]
    );
    assert_eq!(ctx.now(), Cycles(120_000 * 160 / 1_000));
    let raw = read(&mut i2c, ctx, INT_RAW);
    assert_eq!(
        raw & (NACK | TRANS_COMPLETE | TRANS_START_INT),
        TRANS_COMPLETE | TRANS_START_INT
    );
    for n in 0..7 {
        assert_eq!(read(&mut i2c, ctx, COMD0 + 4 * n) & DONE, DONE, "COMD{n}");
    }
    let status = read(&mut i2c, ctx, SR);
    assert_eq!(
        status & (BUS_BUSY | RESP_REC),
        0,
        "acknowledged, then a STOP"
    );
    assert_eq!((status >> TXFIFO_CNT_SHIFT) & 0x3F, 0, "all sent");
    assert_eq!(read(&mut i2c, ctx, DATA), 70);
    assert_eq!(read(&mut i2c, ctx, DATA), 0);
}

/// An address nobody acknowledges ends the sequence where `ack_en` expects an ACK, as on a bus
/// without chips, with the same duration; the chip saw the address and the STOP.
#[test]
fn an_address_no_chip_acknowledges_ends_in_a_nack_and_a_stop_at_the_same_instant() {
    let ctx = &mut Ctx::default();
    let chip = RegisterChip::at(0x55);
    let (mut i2c, mut io) = bus_with(ctx, &chip, WIRES, WIRES);
    register_read(&mut i2c, ctx, 0x68, 0x00);
    run_stepped(&mut i2c, ctx, &mut io);
    assert_eq!(
        chip.borrow().calls,
        ["address 0x68 false 25000", "stop 27500"]
    );
    assert_eq!(ctx.now(), Cycles(4_400), "as the probe of an X4");
    let raw = read(&mut i2c, ctx, INT_RAW);
    assert_eq!(raw & (NACK | TRANS_COMPLETE), NACK);
    assert_eq!(read(&mut i2c, ctx, COMD1) & DONE, 0, "stopped by the NACK");
    assert_eq!(read(&mut i2c, ctx, SR) & (BUS_BUSY | RESP_REC), RESP_REC);
}

/// A chip whose pins the matrix does not route I2C0 to hears nothing, and the controller hears
/// the pulled-up line: a NACK.
#[test]
fn a_chip_off_the_routed_pins_hears_nothing() {
    let ctx = &mut Ctx::default();
    let chip = RegisterChip::at(0x55);
    let elsewhere = crate::periph::I2cWires { scl: 2, sda: 3 };
    let (mut i2c, mut io) = bus_with(ctx, &chip, WIRES, elsewhere);
    register_read(&mut i2c, ctx, 0x55, 0x2C);
    run_stepped(&mut i2c, ctx, &mut io);
    assert!(chip.borrow().calls.is_empty(), "{:?}", chip.borrow().calls);
    assert_eq!(read(&mut i2c, ctx, INT_RAW) & NACK, NACK);
}

/// A write of a register reaches the chip byte by byte; a module reset keeps the chips of the
/// board on the bus.
#[test]
fn a_register_write_reaches_the_chip_and_a_reset_keeps_the_board() {
    let ctx = &mut Ctx::default();
    let chip = RegisterChip::at(0x68);
    let (mut i2c, mut io) = bus_with(ctx, &chip, WIRES, WIRES);
    i2c.reset(ctx);
    assert!(i2c.chips, "the board's chips are kept through the reset");
    configure(&mut i2c, ctx);
    for byte in [0x68 << 1, 0x0E, 0x04] {
        assert!(i2c.write(ctx, DATA, byte));
    }
    assert!(i2c.write(ctx, COMD0, command(OP_RSTART, 0, false)));
    assert!(i2c.write(ctx, COMD1, command(OP_WRITE, 3, true)));
    assert!(i2c.write(ctx, COMD2, command(OP_STOP, 0, false)));
    assert!(i2c.write(ctx, CTR, MS_MODE | TRANS_START));
    run_stepped(&mut i2c, ctx, &mut io);
    assert_eq!(chip.borrow().registers[0x0E], 0x04);
    assert_eq!(
        read(&mut i2c, ctx, INT_RAW) & TRANS_COMPLETE,
        TRANS_COMPLETE
    );
}

/// The X3's bus, routed by a firmware that leaves the pads' own pull-ups off (`Wire` and
/// `i2c_master` may), with the chip at 0x55 and, when `board`, the resistors of the board on SCL
/// and SDA.
fn unpulled_bus(
    ctx: &mut Ctx,
    chip: &std::rc::Rc<std::cell::RefCell<RegisterChip>>,
    board: bool,
) -> (I2c0, IoMatrix) {
    let mut io = IoMatrix::new(0);
    io.route_i2c(ctx, WIRES, false);
    if board {
        for pin in [WIRES.scl, WIRES.sda] {
            io.pull_up_on_board(ctx, pin).unwrap();
        }
    }
    io.attach_i2c(WIRES, Box::new(std::rc::Rc::clone(chip)))
        .unwrap();
    let mut i2c = master(ctx);
    i2c.chips_attached();
    (i2c, io)
}

/// A read of `count` bytes from `address`, the address byte sent without its ACK checked: the
/// bytes are what the line carries, whoever answers.
fn unchecked_read(i2c: &mut I2c0, ctx: &mut Ctx, address: u8, count: u32) {
    assert!(i2c.write(ctx, DATA, u32::from(address << 1 | 1)));
    for (n, cmd) in [
        command(OP_RSTART, 0, false),
        command(OP_WRITE, 1, false),
        command(OP_READ, count, false) | ACK_VAL,
        command(OP_STOP, 0, false),
    ]
    .into_iter()
    .enumerate()
    {
        let offset = COMD0 + 4 * u32::try_from(n).unwrap();
        assert!(i2c.write(ctx, offset, cmd));
    }
    assert!(i2c.write(ctx, CTR, MS_MODE | TRANS_START));
}

/// ADR 0015: with the pads' own pull-ups off, the resistors of the board hold the released lines
/// high: an address nobody acknowledges gets a NACK, and a byte nobody drives reads `0xFF`, as on
/// a bus without chips; the chip at its own address still answers.
#[test]
fn with_the_pads_unpulled_the_boards_pull_ups_make_an_absent_address_a_nack_and_a_read_ff() {
    // Each sequence on a bus of its own: a NACK leaves unsent bytes in the TX FIFO.
    let ctx = &mut Ctx::default();
    let chip = RegisterChip::at(0x55);
    let (mut i2c, mut io) = unpulled_bus(ctx, &chip, true);
    register_read(&mut i2c, ctx, 0x68, 0x00);
    run_stepped(&mut i2c, ctx, &mut io);
    assert_eq!(read(&mut i2c, ctx, INT_RAW) & (NACK | TRANS_COMPLETE), NACK);

    let ctx = &mut Ctx::default();
    let (mut i2c, mut io) = unpulled_bus(ctx, &chip, true);
    unchecked_read(&mut i2c, ctx, 0x68, 2);
    run_stepped(&mut i2c, ctx, &mut io);
    assert_eq!(
        [read(&mut i2c, ctx, DATA), read(&mut i2c, ctx, DATA)],
        [0xFF, 0xFF]
    );

    let ctx = &mut Ctx::default();
    let (mut i2c, mut io) = unpulled_bus(ctx, &chip, true);
    register_read(&mut i2c, ctx, 0x55, 0x2C);
    run_stepped(&mut i2c, ctx, &mut io);
    assert_eq!(read(&mut i2c, ctx, INT_RAW) & NACK, 0);
    assert_eq!(read(&mut i2c, ctx, DATA), 70);
}

/// What a command list left: the interrupt bits right after `TRANS_START` and once it ended, the
/// instant it ended, its `DONE` bits, the state of the bus and the bytes of the RX FIFO.
#[derive(Debug, PartialEq, Eq)]
struct Left {
    raw_at_start: u32,
    raw: u32,
    ended: Cycles,
    done: u32,
    bus: u32,
    received: Vec<u32>,
}

/// Runs the list `list` writes, with `tx` in the TX FIFO, on a bus without chips or, when
/// `chips`, on the X3's bus with a chip at 0x55 that the list never addresses.
fn left_by(chips: bool, tx: &[u32], list: &[u32]) -> Left {
    let ctx = &mut Ctx::default();
    let chip = RegisterChip::at(0x55);
    let (mut i2c, mut io) = if chips {
        bus_with(ctx, &chip, WIRES, WIRES)
    } else {
        (master(ctx), IoMatrix::new(0))
    };
    for &byte in tx {
        assert!(i2c.write(ctx, DATA, byte));
    }
    for (n, &cmd) in list.iter().enumerate() {
        let offset = COMD0 + 4 * u32::try_from(n).unwrap();
        assert!(i2c.write(ctx, offset, cmd));
    }
    assert!(i2c.write(ctx, CTR, MS_MODE | TRANS_START));
    let raw_at_start = read(&mut i2c, ctx, INT_RAW);
    if chips {
        run_stepped(&mut i2c, ctx, &mut io);
    } else {
        run(&mut i2c, ctx);
    }
    let rx = (read(&mut i2c, ctx, SR) >> RXFIFO_CNT_SHIFT) & 0x3F;
    Left {
        raw_at_start,
        raw: read(&mut i2c, ctx, INT_RAW),
        ended: ctx.now(),
        done: (0..8).fold(0, |done, n| {
            done | u32::from(read(&mut i2c, ctx, COMD0 + 4 * n) & DONE != 0) << n
        }),
        bus: read(&mut i2c, ctx, SR) & (BUS_BUSY | RESP_REC),
        received: (0..rx).map(|_| read(&mut i2c, ctx, DATA)).collect(),
    }
}

/// The two paths of I2C0 agree wherever nobody answers (ADR 0015): the same list gives the same
/// interrupts, at the same instant, with the same `DONE` bits, bus state and bytes, whether chips
/// are on the bus or not. A list that lasts no time publishes at the next wake-up on both, a READ
/// of no byte raises no `BYTE_TRANS_DONE` on either.
#[test]
fn the_same_list_leaves_the_same_on_a_bus_with_and_without_chips() {
    let address = 0x68 << 1;
    let lists: [(&[u32], &[u32]); 6] = [
        (&[], &[command(OP_END, 0, false)]),
        (
            &[],
            &[command(OP_READ, 0, false), command(OP_STOP, 0, false)],
        ),
        (
            &[address],
            &[
                command(OP_RSTART, 0, false),
                command(OP_WRITE, 1, true),
                command(OP_STOP, 0, false),
            ],
        ),
        (
            &[address | 1],
            &[
                command(OP_RSTART, 0, false),
                command(OP_WRITE, 1, false),
                command(OP_READ, 0, false),
                command(OP_READ, 2, false) | ACK_VAL,
                command(OP_STOP, 0, false),
            ],
        ),
        (
            &[address],
            &[
                command(OP_RSTART, 0, false),
                command(OP_WRITE, 2, false),
                command(OP_STOP, 0, false),
            ],
        ),
        (&[], &[7 << OP_SHIFT, command(OP_STOP, 0, false)]),
    ];
    for (tx, list) in lists {
        let without = left_by(false, tx, list);
        let with = left_by(true, tx, list);
        assert_eq!(with, without, "{list:x?}");
        assert_eq!(
            without.raw_at_start & !TXFIFO_WM,
            0,
            "{list:x?}: nothing yet"
        );
    }
}

/// Without a pull-up anywhere, a released line reads low: an absent address seems acknowledged
/// and a byte nobody drives reads `0x00`. The board's resistors are what the test above relies on.
#[test]
fn without_any_pull_up_a_released_bus_reads_low() {
    let ctx = &mut Ctx::default();
    let chip = RegisterChip::at(0x55);
    let (mut i2c, mut io) = unpulled_bus(ctx, &chip, false);
    unchecked_read(&mut i2c, ctx, 0x68, 1);
    run_stepped(&mut i2c, ctx, &mut io);
    assert_eq!(read(&mut i2c, ctx, DATA), 0x00);
}
