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
    i2c
}

fn run(i2c: &mut I2c0, ctx: &mut Ctx) {
    let end = ctx.next_wake().expect("a sequence runs");
    ctx.sleep_until(end);
    pump(ctx, |ctx, event| {
        assert_eq!(event, Event::I2c0);
        i2c.wake(ctx);
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
    let mut i2c = master(ctx);
    assert!(i2c.write(ctx, FIFO_CONF, NONFIFO_EN));
    assert!(i2c.write(ctx, CTR, MS_MODE | TRANS_START));
    assert_eq!(
        ctx.take_unsupported(),
        Some(Unsupported::I2c {
            refusal: I2cRefusal::NonFifo
        })
    );
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
