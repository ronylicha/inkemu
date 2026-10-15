//! Programs the tests of the debugger and of the monitor share: a loop that a periodic
//! interrupt cuts, with `wfi`s.
use crate::memmap::{ROM_BASE, SRAM_DBUS_BASE};
use crate::test_support::{HANG, T0, T1, T2, stop_watchdogs, store_register};
use cpu_rv32::asm::{addi, bne, csrrci, csrrsi, csrrw, jal, lui, lw, mret, sw, wfi};

/// The address of word `index` of a program the flat ROM holds.
pub(super) fn at(index: usize) -> u32 {
    ROM_BASE + 4 * u32::try_from(index).unwrap()
}

/// Where the interrupting program puts its vector table, and how its parts are laid out.
pub(super) const VECTORS: u32 = ROM_BASE + 0x1000;
/// The CPU line the SYSTIMER alarm is routed to: its vector is entry 4 of the table.
pub(super) const TIMER_LINE: u32 = 4;
/// The handler, after the 32 entries of the table.
pub(super) const HANDLER: u32 = VECTORS + 4 * 32;
pub(super) const MSTATUS: u16 = 0x300;
pub(super) const MTVEC: u16 = 0x305;
pub(super) const MEPC: u16 = 0x341;
pub(super) const MSTATUS_MIE: u8 = 1 << 3;
/// The alarm's period, in counts of the 16 MHz SYSTIMER: 25 µs, about 500 CPU cycles.
pub(super) const ALARM_PERIOD: u32 = 400;
/// The passes of the loop between two `wfi`s.
pub(super) const PASSES: i32 = 16;
pub(super) const S1: u8 = 9;
/// The handler's count of its entries.
pub(super) const S2: u8 = 18;
pub(super) const A3: u8 = 13;
pub(super) const A4: u8 = 14;
pub(super) const A5: u8 = 15;

pub(super) fn reg(peripheral: &str, register: &str) -> u32 {
    svd_regs::esp32c3::addr(peripheral, register)
}

/// `sw` of the word in `value` to `addr`, through `base`: `addr` split as `lui` and a signed
/// 12-bit offset, so the handler keeps off the registers the loop uses.
pub(super) fn store_through(base: u8, value: u8, addr: u32) -> [u32; 2] {
    let page = addr.wrapping_add(0x800) & 0xFFFF_F000;
    [
        lui(base, page),
        sw(value, base, addr.wrapping_sub(page).cast_signed()),
    ]
}

/// A periodic SYSTIMER alarm on line 4, taken while a loop counts in the first word of the
/// SRAM; every 16 passes the loop masks interrupts and waits in `wfi`, which the masked line
/// ends, then unmasks them and the interrupt is taken at once. The handler clears the alarm
/// and counts itself in the second word. Returns the program, the addresses of the loop's
/// instructions, and the instruction after the `wfi`'s unmasking.
pub(super) fn interrupting() -> (Vec<u32>, Vec<u32>, u32) {
    let mut body = stop_watchdogs();
    body.extend([lui(T0, VECTORS), csrrw(0, MTVEC, T0)]);
    body.extend(store_register(
        reg("INTERRUPT_CORE0", "SYSTIMER_TARGET0_INT_MAP"),
        TIMER_LINE,
    ));
    body.extend(store_register(
        reg("INTERRUPT_CORE0", "CPU_INT_PRI_0") + 4 * TIMER_LINE,
        1,
    ));
    body.extend(store_register(
        reg("INTERRUPT_CORE0", "CPU_INT_ENABLE"),
        1 << TIMER_LINE,
    ));
    body.extend(store_register(reg("SYSTIMER", "INT_ENA"), 1));
    // `TARGET0_CONF`: bit 30 `PERIOD_MODE`, and the period.
    body.extend(store_register(
        reg("SYSTIMER", "TARGET0_CONF"),
        (1 << 30) | ALARM_PERIOD,
    ));
    body.extend(store_register(reg("SYSTIMER", "COMP0_LOAD"), 1));
    // `CONF`: its reset value with `TARGET0_WORK_EN`, bit 24.
    body.extend(store_register(
        reg("SYSTIMER", "CONF"),
        0x4600_0000 | (1 << 24),
    ));
    body.extend([
        csrrsi(0, MSTATUS, MSTATUS_MIE),
        lui(T0, SRAM_DBUS_BASE),
        addi(T1, 0, 0),
        addi(S1, 0, PASSES),
    ]);
    let first = body.len();
    body.extend([
        addi(T1, T1, 1),
        sw(T1, T0, 0),
        lw(T2, T0, 0),
        addi(S1, S1, -1),
        bne(S1, 0, -16),
        csrrci(0, MSTATUS, MSTATUS_MIE),
        wfi(),
        csrrsi(0, MSTATUS, MSTATUS_MIE),
        addi(S1, 0, PASSES),
        jal(0, -36),
    ]);
    let loop_at: Vec<u32> = (first..body.len()).map(at).collect();
    let after_unmasking = at(first + 8);
    let table = usize::try_from((VECTORS - ROM_BASE) / 4).unwrap();
    assert!(body.len() <= table, "the body overlaps the vector table");
    body.resize(table, HANG);
    let mut vectors = [HANG; 32];
    vectors[TIMER_LINE as usize] = jal(0, (HANDLER - (VECTORS + 4 * TIMER_LINE)).cast_signed());
    body.extend(vectors);
    // The handler counts its entries in `s2` first: a run without points counts them too.
    body.extend([addi(S2, S2, 1), addi(A4, 0, 1)]);
    body.extend(store_through(A5, A4, reg("SYSTIMER", "INT_CLR")));
    body.extend([
        lui(A3, SRAM_DBUS_BASE),
        lw(A4, A3, 4),
        addi(A4, A4, 1),
        sw(A4, A3, 4),
        mret(),
    ]);
    (body, loop_at, after_unmasking)
}
