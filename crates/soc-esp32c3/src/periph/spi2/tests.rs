use super::*;
use crate::clock::CpuClock;
use crate::periph::io::SpiWires;
use crate::periph::{GPIO_BASE, IO_MUX_BASE, pump};
use emu_core::{Cycles, Nanos, SpiTarget};
use std::cell::RefCell;
use std::rc::Rc;

const SOURCE_BIT: u64 = 1 << 19;

/// A chip that records what it hears and answers each byte plus one.
#[derive(Debug, Default)]
struct Target {
    calls: Vec<String>,
}

impl SpiTarget for Target {
    fn select(&mut self, selected: bool, at: Nanos) {
        self.calls
            .push(format!("select {selected} at {}", at.get()));
    }

    fn transfer(&mut self, dc: Option<Level>, mosi: &[u8], miso: &mut [u8], at: Nanos) {
        self.calls
            .push(format!("transfer {dc:?} {mosi:02x?} at {}", at.get()));
        for (out, sent) in miso.iter_mut().zip(mosi) {
            *out = sent.wrapping_add(1);
        }
    }
}

/// A bench: the IO matrix routing GP-SPI2 to the X4's display pins (SCLK 8, MOSI 10, MISO 7, CS 21
/// as a GPIO output held high), a recording chip on them, and GP-SPI2 on an 80 MHz APB clock.
struct Bench {
    ctx: Ctx,
    io: IoMatrix,
    spi: Spi2,
    chip: Rc<RefCell<Target>>,
}

fn io_write(io: &mut IoMatrix, ctx: &mut Ctx, addr: u32, value: u32) {
    assert!(io.write(ctx, addr - GPIO_BASE, value), "{addr:#x}");
}

fn gpio(name: &str) -> u32 {
    svd_regs::esp32c3::addr("GPIO", name)
}

fn bench() -> Bench {
    let mut ctx = Ctx::default();
    ctx.set_clock(CpuClock::new(1, 2));
    let mut io = IoMatrix::new(0x8);
    let pad = |n: u32| IO_MUX_BASE + 4 + 4 * n;
    let gpio_pad = (1 << 12) | (1 << 9);
    for (n, signal) in [(8, 63), (10, 65)] {
        io_write(&mut io, &mut ctx, pad(n), gpio_pad);
        io_write(&mut io, &mut ctx, gpio("FUNC0_OUT_SEL_CFG") + 4 * n, signal);
    }
    io_write(&mut io, &mut ctx, pad(7), gpio_pad | (1 << 8));
    io_write(
        &mut io,
        &mut ctx,
        gpio("FUNC0_IN_SEL_CFG") + 4 * 64,
        (1 << 6) | 7,
    );
    io_write(&mut io, &mut ctx, pad(21), gpio_pad);
    io_write(&mut io, &mut ctx, gpio("FUNC21_OUT_SEL_CFG"), 0x80);
    io_write(&mut io, &mut ctx, gpio("OUT_W1TS"), 1 << 21);
    io_write(&mut io, &mut ctx, gpio("ENABLE_W1TS"), 1 << 21);
    let chip = Rc::new(RefCell::new(Target::default()));
    let wires = SpiWires {
        sclk: 8,
        mosi: 10,
        miso: Some(7),
        cs: 21,
        dc: None,
    };
    io.attach_spi(&mut ctx, wires, Box::new(Rc::clone(&chip)))
        .unwrap();
    let mut spi = Spi2::new();
    // `spi_ll_master_init`: the default clock source; 10 MHz from 80 MHz (`CLKCNT_N` 7).
    assert!(spi.write(&mut ctx, CLK_GATE, 0b111));
    assert!(spi.write(&mut ctx, CLOCK, 7 << CLKCNT_N_SHIFT));
    Bench { ctx, io, spi, chip }
}

impl Bench {
    fn write(&mut self, offset: u32, value: u32) {
        assert!(self.spi.write(&mut self.ctx, offset, value), "{offset:#x}");
    }

    fn read(&mut self, offset: u32) -> u32 {
        self.spi.read(&mut self.ctx, offset).unwrap()
    }

    fn select(&mut self, selected: bool) {
        let strobe = if selected { "OUT_W1TC" } else { "OUT_W1TS" };
        io_write(&mut self.io, &mut self.ctx, gpio(strobe), 1 << 21);
    }

    /// Runs device time to `instant`, delivering what is due.
    fn run_to(&mut self, instant: u64) {
        self.ctx.sleep_until(Cycles(instant));
        let Bench { ctx, io, spi, .. } = self;
        pump(ctx, |ctx, event| match event {
            Event::Spi2 => spi.wake(ctx, io),
            Event::Pins => io.wake(ctx),
            other => panic!("{other:?}"),
        });
    }

    fn calls(&self) -> Vec<String> {
        self.chip.borrow().calls.clone()
    }
}

#[test]
fn update_clears_at_once_and_a_transaction_is_flagged_until_its_end() {
    let mut b = bench();
    b.write(CMD, UPDATE);
    assert_eq!(b.read(CMD) & UPDATE, 0);
    b.select(true);
    b.write(W0, 0x0000_3412);
    b.write(MS_DLEN, 15);
    b.write(USER, DOUTDIN | USR_MOSI | USR_MISO);
    b.write(DMA_INT_ENA, TRANS_DONE);
    b.write(CMD, USR);
    // 16 bits at 10 MHz: 1.6 µs, 256 cycles of the time base.
    assert_eq!(b.read(CMD) & USR, USR);
    assert_eq!(b.ctx.next_wake(), Some(Cycles(256)));
    b.run_to(255);
    assert_eq!(b.read(CMD) & USR, USR, "not yet");
    b.run_to(256);
    assert_eq!(b.read(CMD) & USR, 0);
    assert_eq!(
        b.read(W0),
        0x0000_3513,
        "the chip's answer, in the same words"
    );
    assert_eq!(b.read(DMA_INT_RAW) & TRANS_DONE, TRANS_DONE);
    assert_eq!(b.read(DMA_INT_ST), TRANS_DONE);
    assert_eq!(b.ctx.sources() & SOURCE_BIT, SOURCE_BIT);
    b.write(DMA_INT_CLR, TRANS_DONE);
    assert_eq!(b.read(DMA_INT_RAW) & TRANS_DONE, 0);
    assert_eq!(b.ctx.sources() & SOURCE_BIT, 0);
    assert_eq!(
        b.calls(),
        ["select true at 0", "transfer None [12, 34] at 1600"]
    );
}

#[test]
fn a_half_duplex_read_sends_the_line_high_and_stores_what_comes_back() {
    let mut b = bench();
    b.select(true);
    b.write(MS_DLEN, 7);
    b.write(USER, USR_MISO);
    b.write(W0, 0xAABB_CCDD);
    b.write(CMD, USR);
    b.run_to(128);
    assert_eq!(b.read(W0), 0xAABB_CC00, "0xFF + 1 in the first byte");
    assert_eq!(b.calls()[1], "transfer None [ff] at 800");
}

#[test]
fn the_command_and_address_phases_go_first_most_significant_byte_of_the_address_first() {
    let mut b = bench();
    b.select(true);
    b.write(USER, USR_COMMAND | USR_ADDR | USR_MOSI);
    b.write(USER2, (7 << COMMAND_BITLEN_SHIFT) | 0x2C);
    b.write(USER1, 23 << ADDR_BITLEN_SHIFT);
    b.write(ADDR, 0x1234_5600);
    b.write(MS_DLEN, 7);
    b.write(W0, 0xAA);
    b.write(CMD, USR);
    // 5 bytes, 40 bits: 4 µs.
    b.run_to(640);
    assert_eq!(b.calls()[1], "transfer None [2c, 12, 34, 56, aa] at 4000");
    // A 16-bit command: `USR_COMMAND_VALUE[7:0]` first.
    b.write(USER, USR_COMMAND);
    b.write(USER2, (15 << COMMAND_BITLEN_SHIFT) | 0x0201);
    b.write(CMD, USR);
    b.run_to(640 + 256);
    assert_eq!(b.calls()[2], "transfer None [01, 02] at 5600");
}

#[test]
fn a_phase_sent_least_significant_bit_first_reaches_the_chip_reversed() {
    let mut b = bench();
    b.select(true);
    b.write(CTRL, WR_BIT_ORDER | RD_BIT_ORDER);
    b.write(USER, DOUTDIN | USR_MOSI | USR_MISO);
    b.write(MS_DLEN, 7);
    b.write(W0, 0x01);
    b.write(CMD, USR);
    b.run_to(128);
    // 0x01 sent LSB first is 0x80 on the wire; the chip answers 0x81, read LSB first: 0x81.
    assert_eq!(b.calls()[1], "transfer None [80] at 800");
    assert_eq!(b.read(W0) & 0xFF, 0x81);
}

#[test]
fn a_hardware_chip_select_is_active_for_the_transaction_and_held_with_keep_active() {
    let mut b = bench();
    io_write(&mut b.io, &mut b.ctx, gpio("FUNC21_OUT_SEL_CFG"), 68);
    b.write(USER, USR_MOSI);
    b.write(MS_DLEN, 7);
    b.write(CMD, USR);
    b.run_to(128);
    assert_eq!(
        b.calls(),
        [
            "select true at 800",
            "transfer None [00] at 800",
            "select false at 800"
        ]
    );
    b.write(MISC, 0x3E | CS_KEEP_ACTIVE);
    b.write(CMD, USR);
    b.run_to(256);
    assert_eq!(b.calls().len(), 5, "selected again, and held");
    assert_eq!(b.calls()[4], "transfer None [00] at 1600");
    b.write(MISC, 0x3E);
    assert_eq!(b.ctx.take_signal(), Some(Event::Spi2));
    let Bench { ctx, io, spi, .. } = &mut b;
    spi.wake(ctx, io);
    assert_eq!(b.calls()[5], "select false at 1600");
}

#[test]
fn what_the_model_does_not_carry_out_is_refused_by_name_and_ends_at_once() {
    for (setup, refusal) in [
        ((DMA_CONF, 1 << 28), Spi2Refusal::Dma),
        ((USER, USR_DUMMY), Spi2Refusal::Dummy),
        ((SLAVE, SLAVE_MODE), Spi2Refusal::Slave),
        ((USER, USR_CONF_NXT), Spi2Refusal::Segmented),
        ((CTRL, 1 << 14), Spi2Refusal::MultiLine),
        ((USER, SIO), Spi2Refusal::ThreeWire),
        (
            (USER, USR_MOSI | USR_MOSI_HIGHPART),
            Spi2Refusal::DataLength { bits: 512 },
        ),
        ((USER, USR_COMMAND), Spi2Refusal::CommandLength { bits: 4 }),
    ] {
        let mut b = bench();
        b.write(MS_DLEN, 511);
        b.write(USER2, 3 << COMMAND_BITLEN_SHIFT);
        b.write(setup.0, setup.1);
        b.select(true);
        b.write(CMD, USR);
        assert_eq!(
            b.ctx.take_unsupported(),
            Some(Unsupported::Spi2 { refusal }),
            "{refusal:?}"
        );
        assert_eq!(b.read(CMD) & USR, 0, "{refusal:?}");
        assert_eq!(b.read(DMA_INT_RAW) & TRANS_DONE, TRANS_DONE);
        assert_eq!(b.ctx.next_wake(), None);
        assert_eq!(b.calls(), ["select true at 0"], "{refusal:?}");
    }
    let mut b = bench();
    b.write(USER, USR_MOSI);
    b.write(MS_DLEN, 9);
    b.write(CMD, USR);
    assert_eq!(
        b.ctx.take_unsupported(),
        Some(Unsupported::Spi2 {
            refusal: Spi2Refusal::DataLength { bits: 10 }
        })
    );
    b.write(USER, USR_ADDR);
    b.write(USER1, 11 << ADDR_BITLEN_SHIFT);
    b.write(CMD, USR);
    assert_eq!(
        b.ctx.take_unsupported(),
        Some(Unsupported::Spi2 {
            refusal: Spi2Refusal::AddressLength { bits: 12 }
        })
    );
}

#[test]
fn a_reset_loses_the_transaction_and_returns_the_registers_to_their_reset_values() {
    let mut b = bench();
    b.write(USER, USR_MOSI);
    b.write(MS_DLEN, 7);
    b.write(CMD, USR);
    let Bench { ctx, io, spi, .. } = &mut b;
    spi.reset(ctx, io);
    assert_eq!(b.read(CMD), 0);
    assert_eq!(b.read(USER), 0x8000_00C0, "the SVD's reset value");
    b.run_to(1_000);
    assert_eq!(b.calls(), Vec::<String>::new(), "nothing was sent");
}

#[test]
fn the_crystal_and_the_undivided_clock_time_a_transaction() {
    let mut b = bench();
    b.select(true);
    b.write(USER, USR_MOSI);
    b.write(MS_DLEN, 7);
    b.write(CLK_GATE, 0b011);
    b.write(CLOCK, CLK_EQU_SYSCLK);
    b.write(CMD, USR);
    // The crystal undivided: 40 MHz, 8 bits in 200 ns, 32 cycles.
    assert_eq!(b.ctx.next_wake(), Some(Cycles(32)));
}
