# PMP and user mode on the ESP32-C3, and the arch-test group `pmp32` (SP1b-4f)

Date 2026-10-15, branch of SP1b-4f from `main` at `194f286`. Tags: **M** = measured or read (a
file, a line, a page, a command's output), **R** = reasoning. Third-party code ran only in
containers, one at a time: Sail and riscv-arch-test in `inkemu/riscof` (the image
`tools/docker/riscof` builds), ESP-IDF in `espressif/idf:v5.5.2`. Sources downloaded as data and
never executed: the ESP32-C3 TRM, Sail 0.15, the QEMU files of the tag `esp-develop-9.2.2-20250817`,
riscv-tests at `7b68f7b` and its `riscv-test-env` submodule at `6de71ed`. Earlier work this report
builds on: `docs/research/sp1b-4-scoping.md` §1(d), `docs/research/esp32c3-interrupts-and-reset.md`
§2 (report IR).

## 0. Findings

1. **The ESP32-C3 has a user mode** (M, TRM v1.4): `misa` reads `0x40101104`, U = 1 and S = 0
   (register 1.6, page 36); `mcause` 8 is "ECALL from U mode" (register 1.10, page 38); `MPP` holds
   0 (user) or 3 (machine), its high bit tied to its low bit (register 1.5, page 35); `TW` makes `wfi`
   in user mode an illegal instruction (same register). There is no supervisor mode and no `MPRV`.
2. **Its PMP is documented as not conforming** to the privileged specification on two points (M,
   TRM §1.8.2, page 57): no static priority ("access will succeed if it matches at least one of
   enabled PMP entries. An exception will be generated, if access matches none of the enabled PMP
   entries"), and NAPOT ranges of 1 GB at most. 16 entries, a granularity of 4 bytes; machine mode
   is checked only against locked entries, user mode against all, and an access no entry matches is
   allowed in machine mode, refused in user mode (§1.8.3, page 57).
3. **ESP-IDF v5.5.2 programs 16 locked TOR entries that tile the address space without overlap**
   (M, `components/esp_hw_support/port/esp32c3/cpu_region_protect.c` in the pinned image; its first
   comment: "ESP32-C3 CPU doesn't support overlapping PMP regions"). On that configuration the TRM's
   rule and the specification's static priority give the same verdict for every access (R): the
   lockstep against QEMU, which applies the specification, cannot tell them apart.
4. **QEMU applies the specification** (M, `target/riscv/pmp.c` of the tag): lowest-numbered entry
   first, a partial match faults, unlocked entries grant everything to machine mode. Its CPU of the
   ESP32-C3 is the generic `rv32` base CPU with the PMP on; it already refused a store to address 0
   on the reference application (`mcause` 7, `mtval` 0: report IR §2.8). Three behaviours of its own
   are not the TRM's and no measured firmware reaches them (§3).
5. **The arch-test group `pmp32` needs a supervisor mode that the ESP32-C3 does not have** (M): 40
   of its 41 tests declare `rvtest_strap_routine`, whose environment code saves `medeleg` and runs an
   S-mode prolog (`sscratch`, `satp`, `stvec`) before any handler of its own is installed. On Sail
   configured with M and U only, the first such test loops for ever (§5.2). The environment of
   riscv-arch-test 3.10.0 with its S-mode scaffolding compiled out (a 30-line patch, applied the same
   to inkemu and to Sail) runs all 41 tests to the end on Sail M+U, and every check of the tests
   still runs, the "S-mode" parts in user mode (§5.3).
6. **The decoded-instruction cache is not affected** (R, §6): it holds decodings, not permissions;
   the PMP is checked at every fetch, cache hit or not, so a PMP write takes effect on the next
   instruction without invalidating anything.

## 1. The ESP32-C3 core: privilege modes

TRM v1.4, `https://www.espressif.com/sites/default/files/documentation/esp32-c3_technical_reference_manual_en.pdf`,
downloaded 2026-10-15, 903 pages, created 2026-03-26, SHA-256
`90ef825653e7a2657dc1bb294041aace9f9b69da87d775c2f1b5bd11eb317423` (M).

| What | TRM | Page |
|---|---|---|
| Privilege modes | `misa` (register 1.6) resets to `MXL`=1, `U`=1, `M`=1, `I`=1, `C`=1, every other bit 0, among them `S` "Supervisor mode implemented = 0", `N` "User-level interrupts supported = 0": `0x40101104`. Footnote 1 of table 1.4.1: its fields are hard-wired, writes have no effect | 33, 36 |
| `mstatus` (register 1.5) | `MIE`, `MPIE`, `MPP` ("0x0: User mode, 0x3: Machine mode. Only lower bit is writable. Write to the higher bit is ignored as it is directly tied to the lower bit"), `TW` ("If this bit is set, executing WFI instruction in User mode will cause illegal instruction exception"). Nothing else, `MPRV` included | 35 |
| `mcause` (register 1.10) | 0x1 "PMP Instruction access fault", 0x2, 0x3, 0x5 "PMP Load access fault", 0x7 "PMP Store access fault", 0x8 "ECALL from U mode", 0xb "ECALL from M mode"; no 0x0, 0x4, 0x6 | 38 |
| `mtval` (register 1.11) | 0x1: "Faulting virtual address of instruction"; 0x5, 0x7: the faulting data address | 38 |
| CSR list (table 1.4.1) | machine CSRs, the PMP's, the trigger and debug CSRs, `mpcer`/`mpcmr`/`mpccr`, `cpu_gpio_*`; no `mcounteren`, no supervisor CSR. The list is not exhaustive (report IR §2.1: the ROM uses `ustatus` and `0x800`–`0x802`) | 32–33 |

What the TRM does not say (R): what a user-mode access to a machine CSR does (the specification:
an illegal instruction; QEMU and Sail do so); whether interrupts are taken in user mode with
`mstatus.MIE` = 0 (the specification: yes, a machine interrupt is always enabled below machine mode).
inkemu follows the specification on both.

## 2. The ESP32-C3 PMP

TRM §1.8, pages 57–59 (M):

- "It supports 16 regions and a minimum granularity of 4 bytes" (G = 0).
- Non-conformance: "Static priority i.e. overlapping regions are not supported"; "Maximum supported
  NAPOT range is 1 GB". "RISC-V CPU PMP unit in ESP32-C3 does not implement static priority. So,
  software should make sure that all enabled PMP entries are programmed with unique regions ... If
  software still tries to program multiple PMP entries with overlapping region having contradicting
  permissions, then access will succeed if it matches at least one of enabled PMP entries. An
  exception will be generated, if access matches none of the enabled PMP entries."
- "PMP CSR's can only be programmed in machine-mode. Once enabled, write, read and execute
  permission checks are applied to all the accesses in user-mode ... By default, PMP grants
  permission to all accesses in machine-mode and revokes permission of all access in user-mode ...
  In cases where PMP checks are also required in machine-mode, software can set the lock bit of
  required PMP entry to enable permission checks on it. Once lock bit is set, it can only be cleared
  through CPU reset."
- Faults: a fetch without X raises an instruction access fault, a load or store without R or W a
  load or store access fault, with the violating address in `mtval`.
- §1.8.5: "PMP unit implements all pmpcfg0-3 and pmpaddr0-15 CSRs as defined in RISC-V Instruction
  Set Manual Volume II: Privileged Architecture, Version 1.10."

What the TRM leaves open (R), and the reading inkemu takes:

| Question | Reading | Why |
|---|---|---|
| "Matches" an entry | the access lies entirely inside the entry's range and the entry grants the access | the sentence is about "contradicting permissions": an entry that matches but refuses does not make the access fail if another grants it |
| An entry that overlaps the access only in part | it grants nothing; if it is the only entry the access touches, the access faults | the specification faults on a partial match; the TRM is silent. Only a misaligned access (performed on the ESP32-C3) or a 32-bit fetch can straddle the 4-byte grain |
| An unlocked entry, in machine mode | grants everything, as the specification has it | §1.8.3: the checks apply in machine mode only to locked entries |
| A NAPOT range above 1 GB | decoded as the specification does | what the core does with it is not said; ESP-IDF uses TOR and one NA4 |
| `pmpcfg` bits 5–6, the R=0/W=1 combination, the width of `pmpaddr` | as QEMU (§3): bits 5–6 kept, a write of R=0/W=1 ignored, 32 bits kept | the TRM says "as defined in" version 1.10, whose WARL details are a choice; the lockstep's oracle decides between equal readings |

## 3. QEMU (tag `esp-develop-9.2.2-20250817`)

Files read (M): `target/riscv/pmp.c`, `cpu_helper.c`, `esp_cpu.c`, `cpu.c`.

- `esp_cpu.c`: the CPU of the ESP32-C3 is a `TYPE_RISCV_CPU_BASE32` with `misa` set to `RVI | RVM |
  RVC` at init; report IR measured it reading `0x401411AD` (S and U present, among others). The PMP is
  checked: a store to address 0 on the reference application faults with `mcause` 7 (report IR §2.8).
- `pmp_hart_has_privs`: entries in index order; "partially inside" faults at once; the first entry
  whose range holds the whole access decides; in machine mode an unlocked entry grants R, W and X;
  no entry matching: machine mode allowed, other modes refused.
- `pmp_write_cfg`: a locked byte is kept; a byte with W = 1 and R = 0 is **ignored** (the old byte
  stays) when `mseccfg.MML` is clear; any other byte is stored whole, bits 5–6 included.
- `pmpaddr_csr_write`: the 32 bits are stored; a write is ignored when the entry is locked, or when
  the next entry is a locked TOR.
- `pmp_update_rule_addr`: TOR with `pmpaddr[i-1] >= pmpaddr[i]` gives the range `[0, 0]`, which holds
  address 0; the specification says such an entry matches nothing.
- The fetch is checked per 16-bit parcel, through the TLB of the code page.

So QEMU and the TRM differ on overlapping entries (static priority or not) and on the empty TOR
(address 0). With ESP-IDF's 16 contiguous TOR entries neither difference can show (R): a byte lies in
exactly one entry, and the entry ranges below 0x20000000 hold address 0 already (entry 0, `NONE`).
The lockstep table gets no row for them (a row belongs to a window that uses it, R35 of SP1b-4d).

## 4. Sail 0.15, the conformance reference

Sources of the tag `0.15` (M): `model/pmp/pmp_regs.sail`, `pmp_control.sail`,
`model/core/sys_regs.sail`, `model/sys/sys_control.sail`, `model/extensions/I/base_insts.sail`,
`model/postlude/fetch.sail`; default configuration from `sail_riscv_sim --rv32
--print-default-config` in `inkemu/riscof`.

- PMP: 16 entries, `grain` 0, TOR, NA4 and NAPOT supported (the defaults). Static priority, a partial
  match faults, machine mode passes an unlocked entry, no match: machine allowed, others refused.
  `pmpcfg` bits 5–6 read 0. The R=0/W=1 write follows `reserved_behavior.pmpcfg_write_only`
  (default `PMP_ClearPermissions`; `PMP_Ignore` keeps the old byte, as QEMU). `pmpaddr` keeps
  `physaddr_bits − 2` bits: 30 with the bench's `physaddr_bits` 32.
- With U and without S (`legalize_mstatus`): `MIE`, `MPIE`, `TW`, `MPRV` writable; `MPP` keeps a
  nominal mode and turns any other value into the lowest supported one, user. `mret`: illegal below
  machine mode; `MPP` takes user; `MPRV` is cleared when the new mode is not machine. A trap saves the
  current mode in `MPP`. `MPRV` makes explicit loads and stores use `MPP`'s mode.
- `mcounteren` exists when U does. `medeleg` and `mideleg` exist only with S.
- `wfi` in user mode is illegal unless `platform.wfi_available_to_user_mode`; then `TW` is checked
  when the wait ends.
- The fetch is split into 16-bit parcels, each checked, when `Ziccif` is off (it is, in
  `tools/conformance/riscof/sail/inkemu_rv32imc.json`).

## 5. riscv-arch-test 3.10.0, group `pmp32`

### 5.1 The tests

41 files under `riscv-test-suite/rv32i_m/pmp32` (M). Each `RVTEST_CASE` checks only
`ISA:=regex(.*32.*)` and `regex(.*I.*Zicsr.*)`: RISCOF selects all 41 for inkemu's ISA. 40 also say
`def rvtest_strap_routine=True` and switch with `RVTEST_GOTO_LOWER_MODE Smode`, then `Umode`, between
their checks in machine mode; one (`PMP-CFG-reg`) stays in machine mode. The checks lock their
entries, so machine mode is checked too.

### 5.2 Without a supervisor mode the environment loops (measured on Sail)

`arch_test.h` (SHA-256 `cfb0d028baed0fd63e9bf6b8adbbd2a67d2e72b49e58a104659c5b87708f37bc`) runs, with
`rvtest_strap_routine` defined: in the M prolog, `csrrw medeleg` before `mtvec` points at its
handler; then the S prolog (`csrrw sscratch`, `satp`, `stvec`); at the end the S epilog. On Sail with
U and without S (override §5.3), `medeleg` traps to the reset `mtvec`; with a boot hook stepping
over that one, the S prolog traps into the M handler, whose `mepc` lies outside the code and data
segments: the handler "aborts" to the epilogs, whose S epilog traps again, for ever (trace of
`PMP-CFG-reg`, 300 000 instructions, the same loop from instruction 121 on).

### 5.3 The environment with its S scaffolding compiled out

The patch `tools/conformance/riscof/arch-test-env-no-smode.patch` (four hunks): the S prolog is not
instantiated, the S handler and save area are (the M handler reads that save area's static pointers
when it relocates the `mepc` of a trap from user mode, and the save area names the handler's
trampoline); the M prolog and epilog no longer touch `medeleg`; the S epilog is replaced by its end
label, which the S handler names. The S handler never runs: nothing delegates a trap to S.

With it, on Sail configured M+U (`U` supported, `pmpcfg_write_only` `PMP_Ignore`,
`wfi_available_to_user_mode`), all 41 tests end, each with a 100-word signature (M, 2026-10-15). In
`pmp-TOR-R`, for instance, the trap records hold the illegal `csrw satp` of the test body, then the
store and fetch faults of the region under test in machine mode (its entries are locked), then the
same in the "S-mode" part and in the user-mode part. `RVTEST_GOTO_LOWER_MODE Smode` writes `MPP` = 1,
which Sail legalises to user: the "S-mode" part runs in user mode.

What this runs and what it leaves out (R): every PMP check of the group runs, in machine mode and in
user mode; the supervisor-mode copies of the user-mode checks run in user mode, which on the PMP is
the same privilege ("non-machine"). Nothing of S-mode is tested, and the ESP32-C3 has none.

## 6. Fetch and the decoded-instruction cache

`cpu_rv32::hart::Hart::fetch_decoded` serves an instruction from `DecodeCache` when its page's bus
version has not moved since it was decoded (M, `crates/cpu-rv32/src/dcache.rs`). A PMP write changes
no page version. Checking the PMP before the cache lookup, at every fetch, for each 16-bit parcel the
instruction occupies, keeps the cache a pure decoding cache: nothing to invalidate, and a fetch that
the PMP now refuses faults even when its decoding is cached (R). Sail and QEMU check per parcel too
(§3, §4), and inkemu's `fetch` already reads the second parcel separately, with its own address in
`mtval` when it faults.

## 7. riscv-tests on a hart with user mode

Sources at `7b68f7b` (M): `env/p/riscv_test.h` sets a PMP entry granting everything to the lower
modes (`INIT_PMP`, under a temporary `mtvec` that steps over a missing PMP), then `mret`s into the
test with `mstatus` = 0: the `rv32ui` tests run in **user mode** on a hart that has one, the
`rv32mi` tests in machine mode (`RVTEST_ENABLE_MACHINE` sets `MPP` = 3). `isa/rv64si/csr.S` (built as
`rv32mi-p-csr`) checks `misa.U`, then writes `mcounteren` and checks in user mode that a write to
`cycle` and a read of `mstatus` trap. `isa/rv64mi/pmpaddr.S` detects G by writing all ones to
`pmpaddr0`; with G = 0 it passes at once. `isa/rv64mi/illegal.S` writes `MPP` = 1 and passes when it
does not read back.

## 8. Effects outside the CPU

- **Lockstep** (`docs/lockstep-differences.md`, row 26): QEMU keeps the R, W, X bits of each entry
  across its reset, with A and L cleared; the entries are off, so they protect nothing in machine mode
  on either side. The row's sentence "inkemu ne fait pas respecter la PMP (SP1b-4f)" becomes false;
  its treatment does not change (R).
- **Reference firmwares**: none programs overlapping entries or touches a gap of ESP-IDF's table on
  QEMU, where the PMP is applied (the lockstep windows pass). A firmware that faults where it did not
  would change its trace hash; the CI's pinned hashes check it (R).
- **Cost**: the check sits on every fetch, load and store. Measured before the change, on the
  development host (load average about 15 on 16 cores): `coremark_example.bin` at `--max-time 6`,
  748 915 495 instructions, 88.1 s of user CPU time, `trace-hash b9d7c2623875226c`.
