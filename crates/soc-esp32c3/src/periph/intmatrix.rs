//! `INTERRUPT_CORE0`: routes the 62 peripheral interrupt sources to the 31 CPU interrupt lines.
//! Register layout: `interrupt_core0_reg.h` (report §B.3).
//! This plan keeps the configuration the software writes; no source is modelled yet, so nothing is
//! ever pending. Delivery to the CPU arrives in SP1b-3.
use super::Accepted;
use emu_core::Peripheral;

/// `<SRC>_MAP` registers: `UART_INTR_MAP` ... `CACHE_CORE0_ACS_INT_MAP` @0xF4 (report §B.3).
pub const SOURCE_COUNT: usize = 62;
/// `CPU_INT_PRI_0..31`; line 0 is invalid for software but its register exists in the SVD.
pub const CPU_LINE_COUNT: usize = 32;
// The rows of `accepted()` count these registers: a change of either constant must update them.
const _: () = assert!(SOURCE_COUNT == 62 && CPU_LINE_COUNT == 32);

/// `MAC_INTR_MAP` is the map register of source 0 (`0x600C_2000`).
pub(crate) fn accepted() -> Vec<Accepted> {
    const WHY: &str = "interrupt routing: nothing is delivered to the CPU";
    vec![
        Accepted::svd("INTERRUPT_CORE0", "MAC_INTR_MAP", WHY, "SP1b-3").block(62),
        Accepted::svd("INTERRUPT_CORE0", "CLOCK_GATE", WHY, "SP1b-3"),
        Accepted::svd("INTERRUPT_CORE0", "CPU_INT_ENABLE", WHY, "SP1b-3"),
        Accepted::svd("INTERRUPT_CORE0", "CPU_INT_TYPE", WHY, "SP1b-3"),
        Accepted::svd(
            "INTERRUPT_CORE0",
            "CPU_INT_CLEAR",
            "edge-interrupt acknowledge: nothing is pending",
            "SP1b-3",
        ),
        Accepted::svd("INTERRUPT_CORE0", "CPU_INT_PRI_0", WHY, "SP1b-3").block(32),
        Accepted::svd("INTERRUPT_CORE0", "CPU_INT_THRESH", WHY, "SP1b-3"),
    ]
}

/// `INTR_STATUS_REG_0/1` (the report calls them `INTR_STATUS_0/1`): raw source status.
const STATUS_0: u32 = 0xF8;
const STATUS_1: u32 = 0xFC;
const CLOCK_GATE: u32 = 0x100;
const CPU_INT_ENABLE: u32 = 0x104;
const CPU_INT_TYPE: u32 = 0x108;
const CPU_INT_CLEAR: u32 = 0x10C;
const CPU_INT_EIP_STATUS: u32 = 0x110;
const CPU_INT_PRI_0: u32 = 0x114;
const CPU_INT_THRESH: u32 = 0x194;
/// Bits [4:0] of a `<SRC>_MAP` register: the CPU line.
const MAP_MASK: u32 = 0x1F;
/// Bits [3:0] of a priority or threshold register.
const PRIORITY_MASK: u32 = 0xF;
/// `CLOCK_GATE.CLK_EN`, bit 0 (reset value 1 in the SVD).
const CLOCK_GATE_MASK: u32 = 1;
const CLOCK_GATE_RESET: u32 = 1;

#[derive(Clone, Debug)]
pub struct IntMatrix {
    map: [u32; SOURCE_COUNT],
    clock_gate: u32,
    enable: u32,
    edge_type: u32,
    priority: [u32; CPU_LINE_COUNT],
    threshold: u32,
}

/// Index of `offset` in a block of `count` 32-bit registers starting at `first`.
fn index_in(offset: u32, first: u32, count: usize) -> Option<usize> {
    let index = usize::try_from(offset.checked_sub(first)? / 4).ok()?;
    (index < count).then_some(index)
}

impl IntMatrix {
    #[must_use]
    pub const fn new() -> IntMatrix {
        IntMatrix {
            map: [0; SOURCE_COUNT],
            clock_gate: CLOCK_GATE_RESET,
            enable: 0,
            edge_type: 0,
            priority: [0; CPU_LINE_COUNT],
            threshold: 0,
        }
    }
}

impl Default for IntMatrix {
    fn default() -> Self {
        IntMatrix::new()
    }
}

impl Peripheral for IntMatrix {
    fn read(&mut self, offset: u32) -> Option<u32> {
        if let Some(source) = index_in(offset, 0, SOURCE_COUNT) {
            return Some(self.map[source]);
        }
        if let Some(line) = index_in(offset, CPU_INT_PRI_0, CPU_LINE_COUNT) {
            return Some(self.priority[line]);
        }
        Some(match offset {
            // No interrupt source is modelled before SP1b-3: nothing is asserted, nothing is pending.
            STATUS_0 | STATUS_1 | CPU_INT_EIP_STATUS => 0,
            CLOCK_GATE => self.clock_gate,
            CPU_INT_ENABLE => self.enable,
            CPU_INT_TYPE => self.edge_type,
            CPU_INT_THRESH => self.threshold,
            _ => return None,
        })
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        if let Some(source) = index_in(offset, 0, SOURCE_COUNT) {
            self.map[source] = value & MAP_MASK;
            return true;
        }
        if let Some(line) = index_in(offset, CPU_INT_PRI_0, CPU_LINE_COUNT) {
            self.priority[line] = value & PRIORITY_MASK;
            return true;
        }
        match offset {
            CLOCK_GATE => self.clock_gate = value & CLOCK_GATE_MASK,
            CPU_INT_ENABLE => self.enable = value,
            CPU_INT_TYPE => self.edge_type = value,
            // Write-1-to-clear of edge-latched lines; none can be latched yet.
            CPU_INT_CLEAR => {}
            CPU_INT_THRESH => self.threshold = value & PRIORITY_MASK,
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    #[test]
    fn reset_state_gates_the_clock_on_and_routes_nothing() {
        let mut m = IntMatrix::new();
        assert_eq!(m.read(0x100), Some(1), "CLOCK_GATE");
        for offset in [0x54, 0x104, 0x108, 0x114, 0x128, 0x194] {
            assert_eq!(m.read(offset), Some(0), "{offset:#x}");
        }
    }

    #[test]
    fn rom_setup_sequence_is_remembered() {
        let mut m = IntMatrix::new();
        assert!(m.write(0x54, 5)); // UART_INTR_MAP = CPU line 5
        assert!(m.write(0x128, 1)); // CPU_INT_PRI_5
        assert!(m.write(0x194, 1)); // CPU_INT_THRESH
        assert!(m.write(0x104, 1 << 5)); // CPU_INT_ENABLE
        assert_eq!(
            (m.read(0x54), m.read(0x128), m.read(0x194), m.read(0x104)),
            (Some(5), Some(1), Some(1), Some(1 << 5))
        );
    }

    #[test]
    fn fields_keep_only_their_defined_bits() {
        let mut m = IntMatrix::new();
        m.write(0xF4, 0xFFFF_FFFF); // last source map
        m.write(0x114 + 4 * 31, 0xFFFF_FFFF); // CPU_INT_PRI_31
        m.write(0x194, 0xFFFF_FFFF);
        assert_eq!(
            (m.read(0xF4), m.read(0x114 + 4 * 31), m.read(0x194)),
            (Some(0x1F), Some(0xF), Some(0xF))
        );
    }

    #[test]
    fn status_registers_read_zero_while_no_source_exists_and_the_rest_is_unmodeled() {
        let mut m = IntMatrix::new();
        assert_eq!(
            (m.read(0xF8), m.read(0xFC), m.read(0x110)),
            (Some(0), Some(0), Some(0))
        );
        assert!(m.write(0x10C, 0xFFFF_FFFF), "CPU_INT_CLEAR accepts a write");
        assert_eq!(m.read(0x7FC), None, "INTERRUPT_DATE is not modelled");
        assert!(!m.write(0xF8, 1), "status registers are read-only");
    }
}
