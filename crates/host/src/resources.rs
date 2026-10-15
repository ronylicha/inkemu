//! The host's side of the resource monitor (design `SP4b`, ADR 0017): the reading served before,
//! which gives the next one its recent window, and the panel's refreshes, which the host sees and
//! the machine does not. Reading changes nothing of the machine.
use crate::ports::RefreshKind;
use monitor::{HostSide, MonitorView, PanelUse, Refresh};
use soc_esp32c3::{Esp32C3, Usage};

/// What the thread keeps for the monitor across its readings.
#[derive(Debug, Default)]
pub struct Resources {
    /// The last reading served, and the life it was of.
    last: Option<(u32, Usage)>,
    /// The refreshes of the panel in the life that runs or ended last.
    panel: Option<PanelUse>,
}

impl Resources {
    /// A life powers on: its window and its refreshes start from nothing.
    pub fn new_life(&mut self) {
        self.last = None;
        self.panel = None;
    }

    /// The panel completed refresh `generation` of the life (they count from 1), of `kind`.
    pub fn note_refresh(&mut self, generation: u64, kind: RefreshKind) {
        self.panel = Some(PanelUse {
            refreshes: generation,
            last: match kind {
                RefreshKind::Full => Refresh::Full,
                RefreshKind::Partial => Refresh::Partial,
            },
        });
    }

    /// What the monitor shows of `machine`, of life `life`, with what happened since the last
    /// reading of the same life; the reading becomes the start of the next window.
    pub fn view(&mut self, life: u32, machine: &Esp32C3, speed_ratio: Option<f64>) -> MonitorView {
        let now = machine.usage();
        let before = self
            .last
            .as_ref()
            .filter(|(of, _)| *of == life)
            .map(|(_, usage)| usage);
        let view = monitor::view(
            &now,
            before,
            HostSide {
                speed_ratio,
                panel: self.panel,
            },
        );
        self.last = Some((life, now));
        view
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Cycles;
    use soc_esp32c3::SocConfig;
    use soc_esp32c3::memmap::SRAM_DBUS_BASE;
    use soc_esp32c3::test_support::{T0, T1, flat_rom};

    /// A machine that counts for ever in the first word of the SRAM.
    fn machine() -> Esp32C3 {
        let program = [
            cpu_rv32::asm::lui(T0, SRAM_DBUS_BASE),
            cpu_rv32::asm::addi(T1, T1, 1),
            cpu_rv32::asm::sw(T1, T0, 0),
            cpu_rv32::asm::jal(0, -8),
        ];
        Esp32C3::new(flat_rom(&program), &SocConfig::default())
    }

    #[test]
    fn the_first_reading_of_a_life_has_no_window_and_the_next_one_has() {
        let mut resources = Resources::default();
        let mut m = machine();
        m.run_until(Cycles(1_000));
        assert_eq!(resources.view(1, &m, None).recent, None);
        m.run_until(Cycles(3_000));
        let recent = resources.view(1, &m, None).recent.unwrap();
        assert!(recent.instructions > 0);
        assert!(recent.device_nanos > 0);
    }

    #[test]
    fn a_reading_of_another_life_gives_no_window() {
        let mut resources = Resources::default();
        let mut m = machine();
        m.run_until(Cycles(1_000));
        resources.view(1, &m, None);
        m.run_until(Cycles(3_000));
        assert_eq!(resources.view(2, &m, None).recent, None);
        resources.new_life();
        m.run_until(Cycles(5_000));
        assert_eq!(resources.view(2, &m, None).recent, None);
    }

    #[test]
    fn the_panel_is_its_refreshes_in_the_life_and_the_kind_of_the_last() {
        let mut resources = Resources::default();
        let m = machine();
        assert_eq!(resources.view(1, &m, Some(1.5)).panel, None);
        resources.note_refresh(1, RefreshKind::Full);
        resources.note_refresh(2, RefreshKind::Partial);
        let view = resources.view(1, &m, Some(1.5));
        assert_eq!(
            view.panel,
            Some(PanelUse {
                refreshes: 2,
                last: Refresh::Partial
            })
        );
        assert_eq!(view.speed_ratio, Some(1.5));
        resources.new_life();
        assert_eq!(resources.view(2, &m, None).panel, None);
    }
}
