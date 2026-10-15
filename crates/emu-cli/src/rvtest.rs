//! Minimal test machine for riscv-tests and riscv-arch-test: RAM at `0x8000_0000` plus a `tohost` mailbox.
use cpu_rv32::{Core, DatasheetTiming, Hart, HartConfig, MisalignedPolicy, StepOutcome};
use emu_core::{AccessKind, AccessSize, Bus, BusError, Ram, ReadOutcome, TraceHash};
use goblin::elf::{Elf, program_header::PT_LOAD};
use thiserror::Error;

/// Link address used by riscv-tests and riscv-arch-test (`link.ld`).
pub const RAM_BASE: u32 = 0x8000_0000;
pub const RAM_SIZE: usize = 4 * 1024 * 1024;
const TOHOST_PASS: u32 = 1;

#[derive(Debug, Error)]
pub enum RvTestError {
    #[error(
        "not a valid ELF file ({0}); select a bare-metal RV32 ELF built for riscv-tests or riscv-arch-test"
    )]
    Elf(#[from] goblin::error::Error),
    #[error("ELF is not 32-bit RISC-V; build the test with -march=rv32... -mabi=ilp32")]
    WrongArch,
    #[error("ELF has no `{0}` symbol; link the test with the riscv-tests/arch-test environment")]
    MissingSymbol(&'static str),
    #[error(
        "ELF segment at {addr:#010x} does not fit in the {RAM_SIZE}-byte test RAM at {RAM_BASE:#010x}; link the test at {RAM_BASE:#010x} and keep its footprint within the test RAM"
    )]
    SegmentOutOfRange { addr: u32 },
    #[error(
        "signature region is outside test RAM; check that begin_signature and end_signature lie inside a loaded segment"
    )]
    SignatureOutOfRange,
}

/// The hart the conformance suites run on: machine and user modes with a 16-entry PMP
/// ([`Core::MachineUser`], the machine `tools/conformance/riscof/sail` configures Sail as), its
/// misaligned loads and stores trapping unless `misaligned` says otherwise.
#[must_use]
pub fn bench_hart(misaligned: MisalignedPolicy) -> HartConfig {
    HartConfig {
        misaligned,
        core: Core::MachineUser,
        ..HartConfig::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunResult {
    Passed,
    Failed { test_number: u32 },
    Timeout,
}

/// RAM plus the `tohost` mailbox: the first non-zero store to it is the test verdict.
#[derive(Debug)]
struct HostBus {
    ram: Ram,
    tohost: u32,
    written: Option<u32>,
}

impl Bus for HostBus {
    fn read(
        &mut self,
        addr: u32,
        size: AccessSize,
        kind: AccessKind,
    ) -> Result<ReadOutcome, BusError> {
        self.ram.read(addr, size, kind)
    }

    fn write(&mut self, addr: u32, size: AccessSize, value: u32) -> Result<u32, BusError> {
        if addr == self.tohost && value != 0 {
            self.written = Some(value);
        }
        self.ram.write(addr, size, value)
    }

    fn page_version(&self, addr: u32) -> u64 {
        self.ram.page_version(addr)
    }
}

#[derive(Debug)]
pub struct TestMachine {
    hart: Hart,
    bus: HostBus,
    signature: Option<(u32, u32)>,
    trace: TraceHash,
}

impl TestMachine {
    /// Loads a bare-metal RV32 ELF into test RAM and positions the hart at its entry point.
    ///
    /// # Errors
    /// See [`RvTestError`].
    pub fn from_elf(bytes: &[u8], config: HartConfig) -> Result<TestMachine, RvTestError> {
        let elf = Elf::parse(bytes)?;
        if elf.is_64 || elf.header.e_machine != goblin::elf::header::EM_RISCV {
            return Err(RvTestError::WrongArch);
        }
        let mut ram = Ram::new(RAM_BASE, RAM_SIZE);
        for ph in elf
            .program_headers
            .iter()
            .filter(|p| p.p_type == PT_LOAD && p.p_filesz > 0)
        {
            let addr = u32::try_from(ph.p_paddr).map_err(|_| RvTestError::WrongArch)?;
            let out_of_range = || RvTestError::SegmentOutOfRange { addr };
            let start = usize::try_from(ph.p_offset).map_err(|_| out_of_range())?;
            let len = usize::try_from(ph.p_filesz).map_err(|_| out_of_range())?;
            let end = start.checked_add(len).ok_or_else(out_of_range)?;
            let data = bytes.get(start..end).ok_or_else(out_of_range)?;
            ram.load(addr, data).map_err(|_| out_of_range())?;
        }
        let symbol = |name: &str| {
            elf.syms
                .iter()
                .find(|s| elf.strtab.get_at(s.st_name) == Some(name))
                .and_then(|s| u32::try_from(s.st_value).ok())
        };
        let tohost = symbol("tohost").ok_or(RvTestError::MissingSymbol("tohost"))?;
        let signature = symbol("begin_signature").zip(symbol("end_signature"));
        let entry = u32::try_from(elf.entry).map_err(|_| RvTestError::WrongArch)?;
        Ok(TestMachine {
            hart: Hart::new(entry, config),
            bus: HostBus {
                ram,
                tohost,
                written: None,
            },
            signature,
            trace: TraceHash::new(),
        })
    }

    /// Runs until the test writes its verdict to `tohost` or `max_steps` instructions elapse.
    pub fn run(&mut self, max_steps: u64) -> RunResult {
        let timing = DatasheetTiming;
        for _ in 0..max_steps {
            let pc = self.hart.pc;
            if let StepOutcome::Retired { .. } = self.hart.step(&mut self.bus, &timing) {
                self.trace.mix_u32(pc);
                // Compressed instructions in the last two bytes of RAM have no full word to read.
                let word = self
                    .bus
                    .ram
                    .bytes(pc, 4)
                    .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
                self.trace.mix_u32(word);
            }
            if let Some(verdict) = self.bus.written {
                // Both riscv-tests and riscv-arch-test (RVMODEL_HALT) write 1 for a normal end.
                return if verdict == TOHOST_PASS {
                    RunResult::Passed
                } else {
                    RunResult::Failed {
                        test_number: verdict >> 1,
                    }
                };
            }
        }
        RunResult::Timeout
    }

    /// The words between `begin_signature` and `end_signature`.
    ///
    /// # Errors
    /// [`RvTestError::MissingSymbol`] if the ELF has no signature region.
    pub fn signature(&self) -> Result<Vec<u32>, RvTestError> {
        let (begin, end) = self
            .signature
            .ok_or(RvTestError::MissingSymbol("begin_signature"))?;
        let len = usize::try_from(end.saturating_sub(begin))
            .map_err(|_| RvTestError::SignatureOutOfRange)?;
        let bytes = self
            .bus
            .ram
            .bytes(begin, len)
            .map_err(|_| RvTestError::SignatureOutOfRange)?;
        Ok(bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
            .collect())
    }

    #[must_use]
    pub fn trace_hash(&self) -> u64 {
        self.trace.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Imperatives that open the remedy clause of a message (`"; <verb> ..."`).
    const REMEDY_VERBS: [&str; 4] = ["select", "build", "link", "check"];

    /// One instance of every variant. The exhaustive `match` in `variant_index` makes
    /// adding a variant without adding it here a compile error.
    fn one_of_each() -> Vec<RvTestError> {
        let elf_error = Elf::parse(b"not an elf").unwrap_err();
        let all = vec![
            RvTestError::Elf(elf_error),
            RvTestError::WrongArch,
            RvTestError::MissingSymbol("tohost"),
            RvTestError::SegmentOutOfRange { addr: 0x1000 },
            RvTestError::SignatureOutOfRange,
        ];
        let mut seen = [false; 5];
        for e in &all {
            seen[variant_index(e)] = true;
        }
        assert!(
            seen.iter().all(|&s| s),
            "one_of_each must cover every variant"
        );
        all
    }

    fn variant_index(e: &RvTestError) -> usize {
        match e {
            RvTestError::Elf(_) => 0,
            RvTestError::WrongArch => 1,
            RvTestError::MissingSymbol(_) => 2,
            RvTestError::SegmentOutOfRange { .. } => 3,
            RvTestError::SignatureOutOfRange => 4,
        }
    }

    #[test]
    fn every_variant_message_states_a_remedy() {
        for err in one_of_each() {
            let msg = err.to_string();
            let has_remedy = REMEDY_VERBS
                .iter()
                .any(|verb| msg.contains(&format!("; {verb} ")));
            assert!(
                has_remedy,
                "no remedy clause (\"; <{REMEDY_VERBS:?}> ...\") in: {msg}"
            );
        }
    }
}
