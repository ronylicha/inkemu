//! `INTERRUPT_CORE0`: the interrupt controller of the ESP32-C3. It routes the 62 peripheral
//! interrupt sources to the 31 CPU interrupt lines and decides which line the CPU takes.
//! Sources: TRM §1.5 and §8.4, ESP-IDF `interrupt_core0_reg.h`; report IR §1.
//!
//! The rule (IR §1.2, three sources agree):
//!
//! ```text
//! level(n)   = OR of the level of every source s with MAP[s] == n          (n = 1..31)
//! raw(n)     = TYPE[n] ? edge_latch(n) : level(n)
//! pending(n) = ENABLE[n] && raw(n) && PRI[n] != 0 && PRI[n] >= THRESH       (CPU_INT_EIP_STATUS)
//! selected   = the pending line of highest priority, the lowest line number among equals
//! ```
//!
//! `mie` and `mip` play no part: "the ESP32-C3 implements its interrupt mechanism using an
//! interrupt controller instead of RISC-V Privileged ISA specification" (TRM §8.3.2).
//!
//! Sampling contract (ADR 0003; report IR §1.3): a device holds its interrupt source raised
//! until software acknowledges it, and the controller samples the levels at instruction
//! boundaries. The context carries the LEVELS of the sources; the controller reads them when it
//! is evaluated (`resolve`, a register read or a register write), which the machine does between
//! two instructions. A source raised and lowered again between two evaluations is not seen, so
//! an edge-type line does not latch it. That matches the chip, where a source into the interrupt
//! matrix is a level its peripheral holds until software acknowledges it; a device model must do
//! the same (see `Ctx::irq`). The measured application only ever writes 0 to `CPU_INT_TYPE`.
use super::{Accepted, INTMATRIX_BASE, half_of, index_in};
use crate::ctx::{Ctx, MAP_REGISTERS};
use emu_core::Peripheral;
use svd_regs::REGISTER_BYTES;
use svd_regs::esp32c3::addr;

/// `<SRC>_MAP` registers: `MAC_INTR_MAP` (source 0) to `CACHE_CORE0_ACS_INT_MAP` (source 61).
pub const SOURCE_COUNT: usize = SOURCES as usize;
const SOURCES: u32 = MAP_REGISTERS;
/// `CPU_INT_PRI_0..31`; line 0 does not exist (vector slot 0 is the exception entry) but its
/// register is in the SVD and in ESP-IDF's header.
pub const CPU_LINE_COUNT: usize = LINES as usize;
const LINES: u32 = 32;

const fn reg(name: &str) -> u32 {
    addr("INTERRUPT_CORE0", name) - INTMATRIX_BASE
}

/// The first `<SRC>_MAP` register. A map register is written by
/// `ROM3:intr_matrix_set@0x40047a6a`, which ESP-IDF calls as `esp_rom_route_intr_matrix`
/// (`IDF:esp_hw_support/intr_alloc.c:663`).
const MAP_0: u32 = reg("MAC_INTR_MAP");
/// Raw status of sources 0..31 and 32..61, whatever their routing and masking (IR §1.1). No ROM
/// or ESP-IDF code reads them on the measured paths (IR §1.5); the meaning is the TRM's.
const STATUS_0: u32 = reg("INTR_STATUS_REG_0");
const STATUS_1: u32 = reg("INTR_STATUS_REG_1");
const CLOCK_GATE: u32 = reg("CLOCK_GATE");
/// Bit n is set or cleared by `ROM3:esprv_intc_int_enable@0x4005294e` and
/// `ROM3:esprv_intc_int_disable@0x4005295e`.
const CPU_INT_ENABLE: u32 = reg("CPU_INT_ENABLE");
/// Bit n (1: edge) is set by `ROM3:esprv_intc_int_set_type@0x40052972`.
const CPU_INT_TYPE: u32 = reg("CPU_INT_TYPE");
/// Bit n acknowledges an edge latched on line n (TRM §1.5.2). Written by `rv_utils_intr_edge_ack`
/// (`IDF:riscv/include/esp_private/interrupt_intc.h:62-65`:
/// `REG_SET_BIT(INTERRUPT_CORE0_CPU_INT_CLEAR_REG, intr_num)`, an OR into the register with the
/// line number as its mask, nothing written back to 0 there): see [`IntMatrix::acknowledge`].
const CPU_INT_CLEAR: u32 = reg("CPU_INT_CLEAR");
/// Read-only view of `pending(n)` (TRM register 8.60); no ROM or ESP-IDF code reads it on the
/// measured paths (IR §1.5).
const CPU_INT_EIP_STATUS: u32 = reg("CPU_INT_EIP_STATUS");
/// `CPU_INT_PRI_n` is set by `ROM3:esprv_intc_int_set_priority@0x40052992`.
const CPU_INT_PRI_0: u32 = reg("CPU_INT_PRI_0");
/// Set by `ROM3:esprv_intc_int_set_threshold@0x400529a2` (`xPortStartScheduler`), and by the
/// critical sections and `_interrupt_handler` of ESP-IDF (IR §1.5).
const CPU_INT_THRESH: u32 = reg("CPU_INT_THRESH");
// The two blocks are consecutive registers: the index arithmetic below relies on it. The map
// registers are checked one by one, by their place and their name, in a test of `ctx.rs`.
const _: () = assert!(
    reg("CACHE_CORE0_ACS_INT_MAP") == MAP_0 + REGISTER_BYTES * (SOURCES - 1)
        && reg("CPU_INT_PRI_31") == CPU_INT_PRI_0 + REGISTER_BYTES * (LINES - 1)
);

/// Bits `[4:0]` of a `<SRC>_MAP` register: the CPU line; 0 routes the source nowhere.
const MAP_MASK: u32 = 0x1F;
/// Bits `[3:0]` of a priority or threshold register.
const PRIORITY_MASK: u32 = 0xF;
/// Bits `[31:1]` of the per-line registers: line 0 does not exist.
const LINES_MASK: u32 = !1;
/// `CLOCK_GATE.CLK_EN`, bit 0 (reset value 1 in the SVD).
const CLOCK_GATE_MASK: u32 = 1;
const CLOCK_GATE_RESET: u32 = 1;

/// `CLOCK_GATE` only gates the clock of the register file.
pub(crate) fn accepted() -> Vec<Accepted> {
    vec![Accepted::svd(
        "INTERRUPT_CORE0",
        "CLOCK_GATE",
        "clock gate of the controller's registers: they always answer",
        "SP8",
    )]
}

#[derive(Clone, Debug)]
pub struct IntMatrix {
    map: [u32; SOURCE_COUNT],
    clock_gate: u32,
    enable: u32,
    edge_type: u32,
    /// `CPU_INT_CLEAR` as last written, for the read-back: an edge is acknowledged by the write
    /// that has its bit set, and what is stored here gates nothing.
    clear: u32,
    priority: [u32; CPU_LINE_COUNT],
    threshold: u32,
    /// `level(n)` at the last evaluation: a rising edge is a line that is high now and was low then.
    level: u32,
    /// Edge latches: bit `n` is set when edge-type line `n` rose and was not acknowledged.
    latched: u32,
    /// Lines the CPU has taken since their latch was set.
    claimed: u32,
}

impl IntMatrix {
    #[must_use]
    pub const fn new() -> IntMatrix {
        IntMatrix {
            map: [0; SOURCE_COUNT],
            clock_gate: CLOCK_GATE_RESET,
            enable: 0,
            edge_type: 0,
            clear: 0,
            priority: [0; CPU_LINE_COUNT],
            threshold: 0,
            level: 0,
            latched: 0,
            claimed: 0,
        }
    }

    /// `level(n)` for every line: the OR of the sources routed to it (TRM §8.3.3.2).
    fn line_levels(&self, sources: u64) -> u32 {
        let mut levels = 0;
        for (source, line) in self.map.iter().enumerate() {
            if sources & (1 << source) != 0 {
                levels |= 1 << line;
            }
        }
        levels & LINES_MASK
    }

    /// Brings the lines up to date with the source levels, latching the rising edges. Only the
    /// levels present now are seen: see the sampling contract in the module documentation.
    ///
    /// ASSUMPTION: an edge is latched only while its line is enabled: TRM §1.5.2 property 4 speaks
    /// of "an enabled and unmasked interrupt signal" and says no more (report IR §1.3, open
    /// question 8). No ROM or ESP-IDF code on the measured paths uses an edge line; a probe on a
    /// real chip settles it.
    fn update(&mut self, sources: u64) {
        let levels = self.line_levels(sources);
        let rising = levels & !self.level;
        self.latched |= rising & self.edge_type & self.enable;
        self.level = levels;
    }

    /// `pending(n)` for every line: what `CPU_INT_EIP_STATUS` reads.
    fn pending(&self) -> u32 {
        let raw = (self.level & !self.edge_type) | (self.latched & self.edge_type);
        let mut pending = 0;
        for (line, priority) in self.priority.iter().enumerate() {
            // "Only if an interrupt has non-zero priority, higher or equal to the value in the
            // threshold register, will it be reflected in CPU_INT_EIP_STATUS" (TRM register 8.60).
            if *priority != 0 && *priority >= self.threshold {
                pending |= 1 << line;
            }
        }
        pending & raw & self.enable & LINES_MASK
    }

    /// The line the CPU must take given the level of every source, if any: the pending line of
    /// highest priority, and among equals the lowest line number ("statically prioritized by
    /// their IDs, lowest ID having highest priority", TRM §1.5.2).
    pub fn resolve(&mut self, sources: u64) -> Option<u32> {
        self.update(sources);
        let pending = self.pending();
        let mut best: Option<(u32, u32)> = None;
        for (line, priority) in (0u32..).zip(self.priority) {
            if pending & (1 << line) != 0 && best.is_none_or(|(_, top)| priority > top) {
                best = Some((line, priority));
            }
        }
        best.map(|(line, _)| line)
    }

    /// The CPU took `line`: an edge latched on it can now be acknowledged (TRM §1.5.2: the
    /// acknowledge clears "the pending state of claimed edge-type interrupts only").
    pub fn claim(&mut self, line: u32) {
        self.claimed |= self.latched & (1 << (line & 31));
    }

    /// A write to `CPU_INT_TYPE`: the lines whose type bit falls (edge to level) lose their latch
    /// and their claim.
    ///
    /// ASSUMPTION: the hardware behaviour is not documented (TRM §1.5.2 is silent). Keeping the
    /// latch would show a stale edge when the line goes back to the edge type, so it is dropped.
    /// A probe on a real chip settles it: latch an edge, switch the line to level and back, read
    /// `CPU_INT_EIP_STATUS`.
    fn retype(&mut self, value: u32) {
        let fallen = self.edge_type & !value;
        self.latched &= !fallen;
        self.claimed &= !fallen;
        self.edge_type = value & LINES_MASK;
    }

    /// A write to `CPU_INT_CLEAR`: a bit written 1 clears the edge latch of its line when the CPU
    /// has taken that line; a latched edge that was never taken can only be flushed once its line
    /// is disabled (TRM §1.5.2, properties 4 and 5). The register stores what is written and what
    /// it holds gates nothing: a new edge latches whatever the stored bit is.
    ///
    /// ASSUMPTION: the latch is cleared by the write that has the bit set, whatever the bit held
    /// before. The TRM describes the acknowledge as "first setting and then clearing the
    /// corresponding bit" (§1.5.2) and says neither which of the two writes acts nor what a second
    /// set without a clear in between does. Clearing at the set serves both ways of writing it:
    /// the first write of the TRM's toggle acknowledges and the second changes nothing, and a
    /// helper that only ever ORs into the register acknowledges every time, which a rule waiting
    /// for the bit to fall never would. ESP-IDF's helper is of that kind:
    /// `rv_utils_intr_edge_ack` is `REG_SET_BIT(INTERRUPT_CORE0_CPU_INT_CLEAR_REG, intr_num)`
    /// (`IDF:riscv/include/esp_private/interrupt_intc.h:62-65`), `REG_SET_BIT(_r, _b)` being
    /// `*_r = *_r | _b` (`IDF:soc/esp32c3/include/soc/soc.h:51-53`), and nothing there writes a bit
    /// back to 0. It does not settle the rule: it passes the line number where the macro takes a
    /// mask, and `esp_intr_alloc` calls it when an edge interrupt is allocated
    /// (`IDF:esp_hw_support/intr_alloc.c:650`), not when one is served.
    ///
    /// The latch is one bit, so the rule decides what becomes of a second edge. One that arrives
    /// after the CPU took the line and before the acknowledge merges with the edge being served
    /// and is cleared with it: one interrupt for two edges. One that arrives after the
    /// acknowledge, between the two writes of the TRM's toggle included, latches anew and is
    /// kept: the write of 0 clears nothing. Report IR §7 question 8 leaves "the exact effect of
    /// the toggle on an edge that arrives between the 1 and the 0" open.
    ///
    /// An edge interrupt on a real chip, acknowledged by one write of 1 and then by the toggle,
    /// with `CPU_INT_EIP_STATUS` read after each write, and a second edge raised before the
    /// acknowledge and then between the two writes, settles all of it (report IR §1.3, §7
    /// question 8).
    fn acknowledge(&mut self, value: u32) {
        let written = value & LINES_MASK;
        let clearable = self.claimed | !self.enable;
        self.latched &= !(written & clearable);
        self.claimed &= self.latched;
        self.clear = written;
    }
}

impl Default for IntMatrix {
    fn default() -> Self {
        IntMatrix::new()
    }
}

impl Peripheral<Ctx> for IntMatrix {
    fn read(&mut self, ctx: &mut Ctx, offset: u32) -> Option<u32> {
        if let Some(source) = index_in(offset, MAP_0, SOURCE_COUNT) {
            return Some(self.map[source]);
        }
        if let Some(line) = index_in(offset, CPU_INT_PRI_0, CPU_LINE_COUNT) {
            return Some(self.priority[line]);
        }
        let sources = ctx.sources();
        Some(match offset {
            // The low 32 sources, then the 30 others.
            STATUS_0 => half_of(sources, false),
            STATUS_1 => half_of(sources, true),
            CPU_INT_EIP_STATUS => {
                self.update(sources);
                self.pending()
            }
            CLOCK_GATE => self.clock_gate,
            CPU_INT_ENABLE => self.enable,
            CPU_INT_TYPE => self.edge_type,
            CPU_INT_CLEAR => self.clear,
            CPU_INT_THRESH => self.threshold,
            _ => return None,
        })
    }

    fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
        // The lines are brought up to date first: a register written now must not turn a level
        // that was already high into a fresh edge.
        self.update(ctx.sources());
        if let Some(source) = index_in(offset, MAP_0, SOURCE_COUNT) {
            self.map[source] = value & MAP_MASK;
        } else if let Some(line) = index_in(offset, CPU_INT_PRI_0, CPU_LINE_COUNT) {
            self.priority[line] = value & PRIORITY_MASK;
        } else {
            match offset {
                CLOCK_GATE => self.clock_gate = value & CLOCK_GATE_MASK,
                CPU_INT_ENABLE => self.enable = value & LINES_MASK,
                CPU_INT_TYPE => self.retype(value),
                CPU_INT_CLEAR => self.acknowledge(value),
                CPU_INT_THRESH => self.threshold = value & PRIORITY_MASK,
                _ => return false,
            }
        }
        ctx.interrupts_changed();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::Source;
    use emu_core::Peripheral;

    const TICK: Source = Source::of("SYSTIMER_TARGET0_INT_MAP");
    const SOFTWARE: Source = Source::of("CPU_INTR_FROM_CPU_0_MAP");
    const WATCHDOG: Source = Source::of("TG1_WDT_INT_MAP");
    const TICK_MAP: u32 = reg("SYSTIMER_TARGET0_INT_MAP");
    const SOFTWARE_MAP: u32 = reg("CPU_INTR_FROM_CPU_0_MAP");
    const WATCHDOG_MAP: u32 = reg("TG1_WDT_INT_MAP");

    fn priority(line: u32) -> u32 {
        CPU_INT_PRI_0 + 4 * line
    }

    /// The controller as the reference application leaves it (IR §3.2): the software interrupt on
    /// line 3 and the tick on line 4 at priority 1, the interrupt watchdog on line 24 at priority 4,
    /// threshold 1.
    fn application_setup(ctx: &mut Ctx) -> IntMatrix {
        let mut m = IntMatrix::new();
        for (map, line, level) in [
            (SOFTWARE_MAP, 3, 1),
            (TICK_MAP, 4, 1),
            (WATCHDOG_MAP, 24, 4),
        ] {
            assert!(m.write(ctx, map, line));
            assert!(m.write(ctx, CPU_INT_ENABLE, m.enable | (1 << line)));
            assert!(m.write(ctx, priority(line), level));
        }
        assert!(m.write(ctx, CPU_INT_THRESH, 1));
        m
    }

    #[test]
    fn reset_state_gates_the_clock_on_and_routes_nothing() {
        let ctx = &mut Ctx::default();
        let mut m = IntMatrix::new();
        assert_eq!(m.read(ctx, CLOCK_GATE), Some(1));
        for offset in [
            reg("UART_INTR_MAP"),
            CPU_INT_ENABLE,
            CPU_INT_TYPE,
            CPU_INT_CLEAR,
            CPU_INT_PRI_0,
            priority(5),
            CPU_INT_THRESH,
        ] {
            assert_eq!(m.read(ctx, offset), Some(0), "{offset:#x}");
        }
        ctx.irq(TICK, true);
        assert_eq!(
            m.resolve(ctx.sources()),
            None,
            "an unrouted source reaches no line"
        );
    }

    #[test]
    fn rom_setup_sequence_is_remembered() {
        // BC §B.3: the ROM routes UART0 (source 21) to line 5 with priority 1.
        let ctx = &mut Ctx::default();
        let mut m = IntMatrix::new();
        assert!(m.write(ctx, reg("UART_INTR_MAP"), 5));
        assert!(m.write(ctx, priority(5), 1));
        assert!(m.write(ctx, CPU_INT_THRESH, 1));
        assert!(m.write(ctx, CPU_INT_ENABLE, 1 << 5));
        assert_eq!(
            (
                m.read(ctx, reg("UART_INTR_MAP")),
                m.read(ctx, priority(5)),
                m.read(ctx, CPU_INT_THRESH),
                m.read(ctx, CPU_INT_ENABLE)
            ),
            (Some(5), Some(1), Some(1), Some(1 << 5))
        );
        assert!(
            ctx.take_interrupts_changed(),
            "a controller write is a change"
        );
    }

    #[test]
    fn fields_keep_only_their_defined_bits() {
        let ctx = &mut Ctx::default();
        let mut m = IntMatrix::new();
        m.write(ctx, reg("CACHE_CORE0_ACS_INT_MAP"), 0xFFFF_FFFF);
        m.write(ctx, priority(31), 0xFFFF_FFFF);
        m.write(ctx, CPU_INT_THRESH, 0xFFFF_FFFF);
        m.write(ctx, CPU_INT_ENABLE, 0xFFFF_FFFF);
        m.write(ctx, CPU_INT_TYPE, 0xFFFF_FFFF);
        assert_eq!(
            (
                m.read(ctx, reg("CACHE_CORE0_ACS_INT_MAP")),
                m.read(ctx, priority(31)),
                m.read(ctx, CPU_INT_THRESH),
                m.read(ctx, CPU_INT_ENABLE),
                m.read(ctx, CPU_INT_TYPE)
            ),
            (Some(0x1F), Some(0xF), Some(0xF), Some(!1), Some(!1)),
            "line 0 does not exist"
        );
    }

    #[test]
    fn a_level_line_follows_its_source() {
        // IR §3.3: the software interrupt is taken on line 3, then its handler clears the source.
        let ctx = &mut Ctx::default();
        let mut m = application_setup(ctx);
        assert_eq!(m.resolve(ctx.sources()), None);
        ctx.irq(SOFTWARE, true);
        assert_eq!(m.resolve(ctx.sources()), Some(3));
        assert_eq!(m.read(ctx, CPU_INT_EIP_STATUS), Some(1 << 3));
        assert_eq!(
            m.resolve(ctx.sources()),
            Some(3),
            "taking it changes nothing here"
        );
        ctx.irq(SOFTWARE, false);
        assert_eq!(m.resolve(ctx.sources()), None);
        assert_eq!(m.read(ctx, CPU_INT_EIP_STATUS), Some(0));
    }

    #[test]
    fn the_threshold_masks_lower_priorities_and_priority_zero_never_passes() {
        let ctx = &mut Ctx::default();
        let mut m = application_setup(ctx);
        ctx.irq(TICK, true);
        // The handler raises the threshold to the priority of its line plus one (IR §2.3 step 4).
        m.write(ctx, CPU_INT_THRESH, 2);
        assert_eq!(m.resolve(ctx.sources()), None);
        // "Higher or equal": a threshold equal to the priority lets the line through.
        m.write(ctx, CPU_INT_THRESH, 1);
        assert_eq!(m.resolve(ctx.sources()), Some(4));
        // IR §1.5: `esp_intr_alloc` enables a line before it gives it a priority; priority 0 is
        // masked even with the threshold at 0.
        m.write(ctx, priority(4), 0);
        m.write(ctx, CPU_INT_THRESH, 0);
        assert_eq!(m.resolve(ctx.sources()), None);
    }

    #[test]
    fn a_disabled_line_is_not_pending_and_the_raw_status_ignores_routing_and_masking() {
        let ctx = &mut Ctx::default();
        let mut m = application_setup(ctx);
        ctx.irq(TICK, true);
        ctx.irq(SOFTWARE, true);
        m.write(ctx, CPU_INT_ENABLE, 1 << 3);
        assert_eq!(m.read(ctx, CPU_INT_EIP_STATUS), Some(1 << 3));
        m.write(ctx, CPU_INT_THRESH, 15);
        assert_eq!(m.read(ctx, CPU_INT_EIP_STATUS), Some(0));
        // Sources 37 and 50: bits 5 and 18 of the second status register.
        assert_eq!(m.read(ctx, STATUS_0), Some(0));
        assert_eq!(m.read(ctx, STATUS_1), Some((1 << 5) | (1 << 18)));
        assert!(
            !m.write(ctx, STATUS_1, 0),
            "the status registers are read-only"
        );
        assert!(!m.write(ctx, CPU_INT_EIP_STATUS, 0));
    }

    #[test]
    fn the_highest_priority_wins_and_the_lowest_line_among_equals() {
        let ctx = &mut Ctx::default();
        let mut m = application_setup(ctx);
        ctx.irq(TICK, true);
        ctx.irq(SOFTWARE, true);
        assert_eq!(
            m.resolve(ctx.sources()),
            Some(3),
            "priority 1 on lines 3 and 4"
        );
        ctx.irq(WATCHDOG, true);
        assert_eq!(
            m.resolve(ctx.sources()),
            Some(24),
            "priority 4 beats priority 1"
        );
        // The critical-section threshold of ESP-IDF, 4, leaves the panic lines deliverable (IR §1.4).
        m.write(ctx, CPU_INT_THRESH, 4);
        assert_eq!(m.resolve(ctx.sources()), Some(24));
        assert_eq!(m.read(ctx, CPU_INT_EIP_STATUS), Some(1 << 24));
    }

    #[test]
    fn two_sources_on_one_line_are_ored() {
        // IR §3.2: the application routes both cache-error sources to line 25.
        let ctx = &mut Ctx::default();
        let mut m = IntMatrix::new();
        let (illegal, access) = (
            Source::of("CACHE_IA_INT_MAP"),
            Source::of("CACHE_CORE0_ACS_INT_MAP"),
        );
        m.write(ctx, reg("CACHE_IA_INT_MAP"), 25);
        m.write(ctx, reg("CACHE_CORE0_ACS_INT_MAP"), 25);
        m.write(ctx, CPU_INT_ENABLE, 1 << 25);
        m.write(ctx, priority(25), 4);
        ctx.irq(access, true);
        assert_eq!(m.resolve(ctx.sources()), Some(25));
        ctx.irq(illegal, true);
        ctx.irq(access, false);
        assert_eq!(
            m.resolve(ctx.sources()),
            Some(25),
            "the other source holds the line"
        );
        ctx.irq(illegal, false);
        assert_eq!(m.resolve(ctx.sources()), None);
    }

    #[test]
    fn a_source_mapped_to_line_zero_goes_nowhere() {
        let ctx = &mut Ctx::default();
        let mut m = IntMatrix::new();
        assert!(m.write(ctx, TICK_MAP, 0), "routed to line 0 on purpose");
        m.write(ctx, CPU_INT_ENABLE, 0xFFFF_FFFF);
        m.write(ctx, CPU_INT_PRI_0, 15);
        ctx.irq(TICK, true);
        assert_eq!(m.resolve(ctx.sources()), None);
        assert_eq!(m.read(ctx, CPU_INT_EIP_STATUS), Some(0));
    }

    #[test]
    fn an_edge_line_latches_until_it_is_taken_and_acknowledged_by_a_write_of_its_bit() {
        let ctx = &mut Ctx::default();
        let mut m = application_setup(ctx);
        m.write(ctx, CPU_INT_TYPE, 1 << 4);
        ctx.irq(TICK, true);
        assert_eq!(m.resolve(ctx.sources()), Some(4));
        ctx.irq(TICK, false);
        assert_eq!(
            m.resolve(ctx.sources()),
            Some(4),
            "the latch outlives the source"
        );
        // Not taken yet: the toggle does not clear it (TRM §1.5.2 property 5).
        m.write(ctx, CPU_INT_CLEAR, 1 << 4);
        m.write(ctx, CPU_INT_CLEAR, 0);
        assert_eq!(m.resolve(ctx.sources()), Some(4));
        m.claim(4);
        // A write that sets another line's bit acknowledges nothing on this one.
        m.write(ctx, CPU_INT_CLEAR, 1 << 3);
        assert_eq!(m.resolve(ctx.sources()), Some(4));
        m.write(ctx, CPU_INT_CLEAR, 1 << 4);
        assert_eq!(m.read(ctx, CPU_INT_CLEAR), Some(1 << 4));
        assert_eq!(
            m.resolve(ctx.sources()),
            None,
            "the write that sets the bit acknowledges: the first half of the toggle"
        );
        m.write(ctx, CPU_INT_CLEAR, 0);
        assert_eq!(m.read(ctx, CPU_INT_CLEAR), Some(0));
        assert_eq!(
            m.resolve(ctx.sources()),
            None,
            "the second half changes nothing"
        );
        // A source that stayed high is no new edge.
        ctx.irq(TICK, true);
        assert_eq!(m.resolve(ctx.sources()), Some(4));
        m.claim(4);
        m.write(ctx, CPU_INT_CLEAR, 1 << 4);
        m.write(ctx, CPU_INT_CLEAR, 0);
        assert_eq!(m.resolve(ctx.sources()), None, "still high: no rising edge");
    }

    #[test]
    fn a_write_that_sets_a_bit_already_set_acknowledges_the_edge_too() {
        // A helper that only ever ORs a mask into the register, as ESP-IDF's does and never
        // writes a bit back to 0: set, an edge the CPU takes, set again, and once more.
        let ctx = &mut Ctx::default();
        let mut m = application_setup(ctx);
        m.write(ctx, CPU_INT_TYPE, 1 << 4);
        m.write(ctx, CPU_INT_CLEAR, 1 << 4);
        for round in 0..2 {
            ctx.irq(TICK, true);
            assert_eq!(
                m.resolve(ctx.sources()),
                Some(4),
                "round {round}: a new edge latches whatever the stored bit is"
            );
            ctx.irq(TICK, false);
            m.claim(4);
            let stored = m.read(ctx, CPU_INT_CLEAR).unwrap();
            assert_eq!(stored, 1 << 4, "the bit stays as it was written");
            m.write(ctx, CPU_INT_CLEAR, stored | (1 << 4));
            assert_eq!(
                m.resolve(ctx.sources()),
                None,
                "round {round}: the write with the bit set acknowledged the edge"
            );
        }
    }

    #[test]
    fn a_second_edge_is_lost_before_the_acknowledge_and_kept_after_it() {
        // Every change of the source is seen by an evaluation, so each rise is an edge: what the
        // controller resolves with the source low, then with the source high again.
        fn pulse(m: &mut IntMatrix, ctx: &mut Ctx) -> (Option<u32>, Option<u32>) {
            ctx.irq(TICK, false);
            let low = m.resolve(ctx.sources());
            ctx.irq(TICK, true);
            (low, m.resolve(ctx.sources()))
        }

        // Pins what the ASSUMPTION of `acknowledge` says of a second edge: the latch is one bit.
        let ctx = &mut Ctx::default();
        let mut m = application_setup(ctx);
        m.write(ctx, CPU_INT_TYPE, 1 << 4);

        // Between the claim and the acknowledge: merged with the edge being served.
        assert_eq!(pulse(&mut m, ctx), (None, Some(4)), "the first edge");
        m.claim(4);
        assert_eq!(
            pulse(&mut m, ctx),
            (Some(4), Some(4)),
            "a second edge while the first is served: the latch is already set"
        );
        m.write(ctx, CPU_INT_CLEAR, 1 << 4);
        assert_eq!(
            m.resolve(ctx.sources()),
            None,
            "one acknowledge for two edges: the second went with the first"
        );
        assert_eq!(m.read(ctx, CPU_INT_EIP_STATUS), Some(0));

        // Between the two writes of the toggle: after the acknowledge, so kept.
        assert_eq!(
            pulse(&mut m, ctx),
            (None, Some(4)),
            "an edge after the write of 1 latches anew"
        );
        m.write(ctx, CPU_INT_CLEAR, 0);
        assert_eq!(
            m.resolve(ctx.sources()),
            Some(4),
            "the write of 0 clears nothing: the edge is kept"
        );
        assert_eq!(m.read(ctx, CPU_INT_EIP_STATUS), Some(1 << 4));

        // After a whole toggle: kept too, and served by its own acknowledge.
        m.claim(4);
        m.write(ctx, CPU_INT_CLEAR, 1 << 4);
        m.write(ctx, CPU_INT_CLEAR, 0);
        assert_eq!(m.resolve(ctx.sources()), None);
        assert_eq!(
            pulse(&mut m, ctx),
            (None, Some(4)),
            "an edge after the toggle"
        );
        m.claim(4);
        m.write(ctx, CPU_INT_CLEAR, 1 << 4);
        assert_eq!(m.resolve(ctx.sources()), None);
    }

    #[test]
    fn an_edge_that_was_never_taken_is_flushed_once_its_line_is_disabled() {
        let ctx = &mut Ctx::default();
        let mut m = application_setup(ctx);
        m.write(ctx, CPU_INT_TYPE, 1 << 4);
        ctx.irq(TICK, true);
        assert_eq!(m.resolve(ctx.sources()), Some(4));
        ctx.irq(TICK, false);
        assert_eq!(m.resolve(ctx.sources()), Some(4));
        let enabled = m.read(ctx, CPU_INT_ENABLE).unwrap();
        m.write(ctx, CPU_INT_ENABLE, enabled & !(1 << 4));
        m.write(ctx, CPU_INT_CLEAR, 1 << 4);
        m.write(ctx, CPU_INT_CLEAR, 0);
        m.write(ctx, CPU_INT_ENABLE, enabled);
        assert_eq!(m.resolve(ctx.sources()), None);
    }

    #[test]
    fn an_edge_on_a_disabled_line_is_not_latched() {
        // Pins the ASSUMPTION of `update`.
        let ctx = &mut Ctx::default();
        let mut m = application_setup(ctx);
        m.write(ctx, CPU_INT_TYPE, 1 << 4);
        let enabled = m.read(ctx, CPU_INT_ENABLE).unwrap();
        m.write(ctx, CPU_INT_ENABLE, enabled & !(1 << 4));
        ctx.irq(TICK, true);
        assert_eq!(m.resolve(ctx.sources()), None);
        m.write(ctx, CPU_INT_ENABLE, enabled);
        assert_eq!(
            m.resolve(ctx.sources()),
            None,
            "the edge passed while the line was disabled"
        );
    }

    #[test]
    fn a_pulse_shorter_than_one_evaluation_is_lost_to_an_edge_line() {
        // The sampling contract (ADR 0003; report IR §1.3): a device holds its source until
        // software acknowledges it, the context carries levels, and the controller samples them
        // when it is evaluated, at instruction boundaries.
        let ctx = &mut Ctx::default();
        let mut m = application_setup(ctx);
        m.write(ctx, CPU_INT_TYPE, 1 << 4);
        ctx.irq(TICK, true);
        ctx.irq(TICK, false);
        assert_eq!(
            m.resolve(ctx.sources()),
            None,
            "raised and lowered between two evaluations: never seen"
        );
        // A source held up across one evaluation is an edge, and the latch outlives it.
        ctx.irq(TICK, true);
        assert_eq!(m.resolve(ctx.sources()), Some(4));
        ctx.irq(TICK, false);
        assert_eq!(m.resolve(ctx.sources()), Some(4));
        m.claim(4);
        m.write(ctx, CPU_INT_CLEAR, 1 << 4);
        m.write(ctx, CPU_INT_CLEAR, 0);
        assert_eq!(m.resolve(ctx.sources()), None);
    }

    #[test]
    fn a_latch_does_not_survive_the_line_becoming_level_type() {
        // Pins the ASSUMPTION of `retype`.
        let ctx = &mut Ctx::default();
        let mut m = application_setup(ctx);
        m.write(ctx, CPU_INT_TYPE, 1 << 4);
        ctx.irq(TICK, true);
        assert_eq!(m.resolve(ctx.sources()), Some(4));
        ctx.irq(TICK, false);
        m.claim(4);
        m.write(ctx, CPU_INT_TYPE, 0);
        assert_eq!(m.resolve(ctx.sources()), None);
        m.write(ctx, CPU_INT_TYPE, 1 << 4);
        assert_eq!(
            m.resolve(ctx.sources()),
            None,
            "the old latch is not back with the edge type"
        );
        // The claim went with the latch: a new edge is not yet acknowledgeable.
        ctx.irq(TICK, true);
        assert_eq!(m.resolve(ctx.sources()), Some(4));
        ctx.irq(TICK, false);
        m.write(ctx, CPU_INT_CLEAR, 1 << 4);
        m.write(ctx, CPU_INT_CLEAR, 0);
        assert_eq!(
            m.resolve(ctx.sources()),
            Some(4),
            "never taken: the toggle does not clear it"
        );
    }

    #[test]
    fn everything_else_is_unmodeled() {
        let ctx = &mut Ctx::default();
        let mut m = IntMatrix::new();
        assert_eq!(m.read(ctx, reg("INTERRUPT_REG_DATE")), None);
        assert!(!m.write(ctx, reg("INTERRUPT_REG_DATE"), 0));
    }
}
