# SP1b-2 — SPI flash, cache/MMU, SHA, clocks, eFuse content: the 2nd-stage bootloader runs and jumps into the application — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `inkemu run --device x4 --flash <image>` runs the real mask ROM through its flash-boot path, then the unmodified ESP-IDF v5.5.2 2nd-stage bootloader, and jumps into the application, which prints its first log line; in `--strict-mmio` nothing unmodelled is touched and no CPU exception is taken from reset to the application entry.

**Architecture:** `soc-esp32c3` gains a read-only NOR flash model behind a small trait, the SPI1 flash controller (hard-wired commands and the generic `USR` transfer), SPI0 as storage, the cache controller (`EXTMEM`) with its completion bits, the 128-entry MMU table, and two flash windows in the bus that read the flash **live** through that table. A SHA accelerator (SHA-1/224/256, typical mode) wraps the RustCrypto compression functions. Clock registers feed a pure function to the CPU frequency and a virtual clock owned by the machine. `boards` profiles describe the flash; `emu-cli` loads and pre-checks a full flash image. Every register that only stores a value is listed in one table generated from the code.

**Tech Stack:** Rust 1.99.0 (edition 2024), `sha2` 0.11, `sha1` 0.11 and `md-5` 0.11 (the current RustCrypto line; the SHA accelerator uses only the compression functions `sha2::block_api::compress256` and `sha1::block_api::compress`, which need no feature flag), `serde` + `toml` (profiles), `clap`, `thiserror`, `assert_cmd`/`predicates`/`tempfile` (tests), Docker `espressif/idf:v5.5.2` (reference firmware, CI and local only).

**Spec:** `docs/superpowers/specs/2026-10-11-inkemu-design.md` (§3.1–§3.3, §4.1, §4.3, §5.1, §5.2, §7, §8, §10). Hardware facts: `docs/research/esp32c3-rom-flash-boot.md` (**FB**), `docs/research/esp32c3-cache-mmu.md` (**CM**), `docs/research/esp32c3-bootloader-accesses.md` (**BA**), `docs/research/sp1b-test-oracle.md` (**TO**), `docs/research/esp32c3-boot-chain.md` (**BC**), `docs/research/esp32c3-svd.md`. Carried-over points: `docs/superpowers/notes/2026-10-11-sp1b-1-carryover.md`. A value in this plan is followed by the report tag and section it comes from.

## Global Constraints

- Toolchain pinned: Rust **1.99.0**, edition **2024**. Workspace lints unchanged: `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `unsafe_code = "forbid"`; `unwrap_used`/`expect_used` denied outside tests (an invariant is documented with `// INVARIANT:`).
- No `TODO`, no placeholder, no escape hatch that hides a type or a lint.
- Coverage ≥ 80 % for the workspace (`cargo llvm-cov --workspace --fail-under-lines 80`).
- Licence Apache-2.0; `cargo deny check` passes; permissive licences only. A new dependency is added at the version this plan names, in `[workspace.dependencies]`.
- Register addresses and names come from `svd-regs` (`svd_regs::esp32c3::addr("PERIPHERAL", "REGISTER")`), never from a literal. A register the SVD does not contain gets **one named constant with its source in a comment** (decision D17 lists them all).
- Every user-facing message states **the cause, then the remedy**, separated by a semicolon.
- Untrusted input (flash image file, device profile, anything the firmware writes) is bounded and never panics.
- Determinism: same inputs → same trace hash. No wall clock, no host randomness, no `HashMap` iteration on the emulated path; ordered collections are `Vec` or `BTreeMap`.
- `soc-esp32c3` performs **no host I/O** (it must build for WASM in sub-project 6). `emu-core` depends on no workspace crate; `svd-regs` stays `no_std` without dependency.
- Third-party material (ROM, firmware, bootloader, QEMU) stays out of the repository and of the binary, and is **never executed on the host**: the reference firmware is built inside Docker; inkemu is the only thing that executes it.
- QEMU (GPL) is an external test tool only. No QEMU code or comment is copied; only facts (addresses, values) taken from the research reports.
- Each modelled hardware behaviour cites its source in a comment (report tag + section, ROM symbol, ESP-IDF header). An assumed behaviour is marked `ASSUMPTION:` with what would settle it, and is listed in the hand-over to SP1b-4 (Task 17).
- Unmodelled registers (spec §5.2): read = SVD reset value, write ignored, one **deduplicated** warning naming peripheral, register, address and PC; `--strict-mmio` stops the machine (exit code 3).
- Checking a command: redirect its output to a file, then read `$?`; never read an exit code through a pipe.
- Commits: `type(scope): description`, only the paths of the task (`/usr/bin/git commit -- <paths>`), message ending with the line `Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ`.
- Formatting and lints before **every** commit. The code blocks of this plan are not wrapped the way `rustfmt` wraps them (no `rustfmt.toml`: the default 100 columns apply), so each task ends with: `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`, all three `exit=0`, whether or not the task lists them. `rustfmt` may turn a one-line `match` arm into a block: a block whose last expression is an assignment needs its `;` (clippy `semicolon_if_nothing_returned`).

## Review Focus

- **A file given as `--flash` that is not a full flash image** (an application-only `.bin`, an encrypted `.xota`, an image for another chip, a file longer than the flash, an empty file, a directory): refused before the machine starts, with the cause and the remedy; never a panic, never a 16 MiB read of a pipe → tests `xota_is_refused_before_boot`, `image_for_another_chip_is_refused`, `image_longer_than_the_flash_is_refused_with_both_sizes`, `empty_file_and_directory_are_refused` (Task 15; the checks on the content have their own tests in Task 14).
- **Firmware that reads or executes flash it has not mapped** (invalid MMU entry, a page number past the end of the flash, a load straddling two 64 KiB pages or the end of the window): a defined value, a named diagnostic, the fault registers latched, a stop under `--strict-mmio`; never a panic → tests `invalid_entry_reads_zero_latches_the_fault_and_is_reported`, `page_beyond_the_flash_is_reported`, `load_straddling_two_pages_reads_each_byte_through_its_own_entry`, `access_straddling_the_end_of_the_window_is_unmapped` (Task 7).
- **Firmware that sends a flash command the read-only flash cannot honour** (page program, sector erase, write status, an unknown opcode, a `USR` transfer longer than the 64-byte data buffer): the command is named in a diagnostic, nothing is silently dropped, no buffer is overrun → tests `program_and_erase_are_reported_by_opcode`, `unknown_user_opcode_is_reported`, `transfer_lengths_are_bounded_by_the_sixteen_data_registers` (Task 5).
- **The SHA accelerator used outside the measured protocol** (`CONTINUE` without `START`, a mode the C3 does not have, a DMA start, a mode change between blocks): a defined result or a named diagnostic, never a stale digest presented as valid → tests `continue_without_start_hashes_from_the_stored_state`, `unknown_mode_is_reported_and_leaves_the_state`, `dma_start_is_reported` (Task 8).
- **A device profile whose flash description is inconsistent** (size 0, not a power of two, above 16 MiB, a JEDEC capacity byte that contradicts the size, an empty id): refused at load with the offending line → tests `flash_size_must_be_a_power_of_two_up_to_16_mib`, `jedec_capacity_must_match_the_flash_size`, `empty_or_malformed_id_is_refused_with_its_line` (Task 13).

---

## Design decisions taken for this plan

D1–D16 are the framing decisions (binding). D17 onward were taken while writing the plan.

| # | Decision | Reason | Cost if wrong |
|---|---|---|---|
| D1 | Input is a **full flash image** (`--flash <file>`). Composing a flash from an application `.bin` (`--firmware`) is SP1b-3. | CrossPoint releases are app-only; composing needs a bootloader-source decision the milestone does not need. | Low: one more input path later. |
| D2 | Profiles gain the flash size and JEDEC id. Longer image = error; shorter = padded with `0xFF`. JEDEC id = the one the QEMU oracle answers, `0x9D6018` (FB §B.6), an assumption for the real X4/X3. | Real part unknown; the SP1b-4 lockstep needs the same vendor branch as QEMU. | Low: one profile line. |
| D3 | Flash is **read-only**. SPI1 implements RES `0xAB`, RDID `0x9F`, RDSR `0x05`, SFDP `0x5A`, RESUME `0x7A`, WREN `0x06`, WRDI `0x04` and the generic `USR` path. Program/erase/any other opcode = named diagnostic. | Spec §7: no silent wrong state. | Low: writes arrive with NVS (SP1b-3). |
| D4 | Cache windows read the flash **live** through the MMU table. Invalid entry: read 0, latch `CACHE_ILG_INT_ST`/`CACHE_MMU_FAULT_*`, diagnostic, stop under strict. Store to a window: store access fault. Both `ASSUMPTION:`. | CM §6, §10: silicon behaviour undocumented. | Medium: buggy firmware behaves differently; the lockstep shows it. |
| D5 | Bus gating (`ICACHE_CTRL.ENABLE`, `SHUT_IBUS`/`SHUT_DBUS`) is stored, not enforced. | Measured boots never rely on it (CM §6.2). | Low. |
| D6 | Any write to the MMU table, `ICACHE_CTRL` or `ICACHE_CTRL1` changes `Bus::page_version` of both windows, which invalidates decoded flash code. Cache-miss timing is not modelled. | Correctness first; timing is SP8. | Low. |
| D7 | SHA: typical mode, SHA-1/224/256, RustCrypto compression functions. DMA-SHA = named diagnostic. | The C3 has exactly these three; no home-made crypto. | Low. |
| D8 | RNG (`0x600260B0`) is a deterministic PRNG, fixed default seed, advanced on every read. | The bootloader loops on a constant (BA §2.3 item 1); replay is a spec requirement. | Low: seed injection is SP1b-4. |
| D9 | eFuse content = the production v0.3 chip of BA §8 (block version 1.2, MAC, calibration), wafer version still from the profile. | All-zero eFuses make the bootloader refuse the image (BA §2.3 item 3). | Low. |
| D10 | Clock registers are storage plus a pure function to the CPU frequency. | Carried over from SP1b-1. | Low. |
| D11 | Watchdogs and regi2c/analog registers: storage with the read values the bootloader needs, **no expiry**. | Expiry before the SYSTIMER tick would reset a healthy app (BA §6). | Low. |
| D12 | A documented table "registers accepted without effect", pinned against the code by a test. | SP1b-1 final review; spec §7. | Nil. |
| D13 | Reference firmware is **built, not committed** (Docker `espressif/idf:v5.5.2`, pinned SHA-256 of both binaries, `merge_bin`); a CI job builds and runs the ignored tests. | Project rule; the build is reproducible (TO §2). | Low. |
| D14 | `docs/research/esp32c3-boot-chain.md` is corrected with every correction of FB §K, CM §0.1, BA §11. | A research document known to be wrong is debt. | Nil. |
| D15 | Hygiene handled here: profile validation, the SP1a `inspect` points, single-pass extraction in `rom_store`, literal bases in three test modules. | Same files; zero debt. | Nil. |
| D16 | `--strict-mmio` also stops on an invalid flash mapping, an unmodelled flash command, DMA-SHA (and the other "cannot be right silently" events of D22, D27). | One strict switch, one exit code (3). | Nil. |
| D17 | `svd_regs::esp32c3::addr(peripheral, name)` is a `const fn`: a misspelt name is a **compile error**. Registers absent from the SVD are named constants with their source: the MMU table `0x600C_5000` (CM §1), the regi2c window `0x6000_E000/004/040/044/048` (BA §2.2), SPI0 `+0x24`/`+0x28` (FB §B.2), and the radio power registers `0x6001_CCD4`, `0x6000_6090`, `0x6000_50F0` (BA §3). | The table had no lookup by name; the framing allowed one constant (the MMU table) and the boot needs eight more addresses. | Low. |
| D18 | The ESP32-C3 hart gets the PMP CSRs (`pmpcfg0..3`, `pmpaddr0..15`): stored, lock bit honoured, **not enforced**. | The bootloader writes them (BA §2, `esp_cpu_configure_region_protection`); without them it takes an illegal-instruction exception, which the milestone forbids. SP1b-1 had deferred PMP to SP1b-3. | Medium: firmware relying on a PMP fault is not reproduced until SP1b-3 enforces it. |
| D19 | The flash chip is owned by the SPI1 model (`Box<dyn FlashDevice>`); the bus reads the cache windows through it. The image is kept as given (bytes past it read `0xFF`), not expanded to 16 MiB. | One owner for one chip; a blank machine costs no allocation. | Low. |
| D20 | The workspace moves to the current RustCrypto line before anything else (Task 0): `sha2` **0.11**, `md-5` **0.11**, and `sha1` added at **0.11**. The SHA accelerator calls `sha2::block_api::compress256(&mut [u32; 8], &[[u8; 64]])` and `sha1::block_api::compress(&mut [u32; 5], &[[u8; 64]])`, which exist without a feature flag. In 0.11 a digest no longer implements `LowerHex`: each crate that prints one has one small `hex` helper (Task 0). (Overrides the first version of this decision, which kept 0.10 for its `compress` feature.) | Latest stable versions, and one RustCrypto generation (`digest` 0.11) in the tree; D7 only needs the compression functions, whatever their path. | Low: three version lines and four call sites, all in Task 0. |
| D21 | CPU frequency at reset = **20 MHz**: crystal ÷ (`PRE_DIV_CNT` + 1) with `PRE_DIV_CNT` = 1 (ESP-IDF `clk_ll_cpu_get_divider`; BA §5 step 8). The ROM itself believes 40 MHz (BA §10 item 1). `ASSUMPTION:`. Emulated time is kept by a `VirtualClock` in the machine; the per-instruction `TimingModel` is not involved. | Not provable from the sources; the firmware-visible counter counts cycles either way. | Low: one constant; no hang either way (BA §10 item 1). |
| D22 | A clock selection the function cannot turn into a frequency (`SOC_CLK_SEL` 2 or 3, PLL with `CPUPERIOD_SEL` ≥ 2) keeps the previous frequency and raises a named diagnostic. | Zero silent wrong state. | Low. |
| D23 | `EXTMEM.ICACHE_SYNC_CTRL` reads `0x2` (done) from reset; an access through an invalid entry or to a page past the flash returns 0. `ASSUMPTION:` (CM §9 items 1, 5, 8). | The diagnosable option. | Low. |
| D24 | regi2c: each (host, block, register) is one stored byte, 0 before its first write; `ANA_CONFIG`, `ANA_CONFIG2`, `I2C_MST_ANA_CONF0` are storage reset to 0. `ASSUMPTION:` (QEMU answers `0xFFFFFF`; real reset values unknown, BA §2.1 item 7). | Every use on the measured paths is read-modify-write (BA §2.2). | Low: lockstep must mask these reads. |
| D25 | An SFDP read (`0x5A`) is answered with `0xFF` bytes; the `USR` address is the low bits of `SPI1.ADDR`, unshifted. `ASSUMPTION:`. | The bootloader only needs SFDP byte `0x10` ≠ `0x20` (BA §2.2); QEMU's answer is not recorded. | Low. |
| D26 | `--flash` applies the spec §5.1 refusals to the bootloader header at offset 0 (magic, `.xota`, chip id, declared flash size) before the machine starts; an empty file is refused. | The spec is binding; the framing was silent. | Low. |
| D27 | `RTC_CNTL.OPTIONS0` is storage, except that a write with a software-reset bit (31 or 5, BC §E.2) is reported and not stored. | A reset that silently does not happen is a silent wrong state. | Low: SP1b-3 implements the reset. |
| D28 | The flash path of the ROM is tested against the **real ROM with synthetic bootloader images** built by `firmware::builder` (no ESP-IDF needed), in the existing `rom-boot` job; the reference firmware has its own job. | Fast feedback on SPI1 + cache + MMU + SHA without Docker. | Nil. |
| D29 | Every register the application touches between its entry and its first log line (BA §3) is modelled, so the strict run ends on `TIMG0.RTCCALICFG` and nothing earlier. | The milestone asks for a *named* end. | Low. |

## Carried-over points of SP1b-1: where they are handled

Source: `docs/superpowers/notes/2026-10-11-sp1b-1-carryover.md`, section "Reporté au plan SP1b-2".

| Point | Handled |
|---|---|
| `EXTMEM.ICACHE_SYNC_CTRL` and the registers before it: `GPIO.ENABLE`, GPIO matrix, `IO_MUX` flash pads, SPI0/SPI1 | **Tasks 5, 6** |
| MMU table (`0x600C_5000`) absent from the SVD | **Task 6** (table), **Task 7** (windows) |
| eFuse: MAC address and block version | **Task 11** |
| eFuse: programming registers | Re-deferred to **SP1b-3**: no measured path touches `EFUSE.CMD` or `PGM_*` (BA §8); they stay named unmodelled accesses |
| `SYSTEM.CPU_PER_CONF` and the clock tree | **Task 10** |
| Profiles: flash size; validation (empty id, id consistency) | **Task 13** |
| Profiles: partition tables | Re-deferred to **SP1b-3** with `--firmware` (D1): only a composed flash needs a table from the profile |
| `--device <file.toml>` and cleaning the `toml` error text | Re-deferred to **SP2** (first plan with user-supplied profiles); no such option exists |
| SP1a: `fetch_stall` and the flash cache timing | Re-deferred to **SP1b-4** (owns the CoreMark tolerance, spec §4.2; calibration in SP8). `Bus::fetch_stall` keeps returning 0 (D6) |
| SP1a: flash-dump heuristic, `BadFlashSize` message, `Display` of `SpiMode` | **Task 14** (library and `inspect`) |
| `rom_store` decompresses the archive once per missing ELF | **Task 15** |
| Three test modules write base addresses as literals | **Task 1** |

---

## File Structure

```
Cargo.toml                               sha2 0.11, sha1 0.11, md-5 0.11 (Task 0)
crates/emu-cli/src/term.rs               + hex (digests are printed through it)
crates/svd-gen/src/main.rs               + hex (private)
crates/soc-esp32c3/tests/common/mod.rs   hex, shared by the integration tests of the crate
crates/svd-regs/src/lib.rs               + esp32c3::addr (const), esp32c3::find
crates/emu-core/src/time.rs              + Frequency::new (const), VirtualClock
crates/cpu-rv32/src/csr_esp32c3.rs       + PMP CSRs (stored, lock honoured)
crates/soc-esp32c3/src/
  flash.rs                               JedecId, FlashConfig, FlashDevice, NorFlash, FlashImageError
  clock.rs                               cpu_frequency(), XTAL
  config.rs                              + SocConfig::flash, SocConfig::rng_seed
  diag.rs                                + Unsupported, MappingFault, two Diagnostic variants
  memmap.rs                              + flash windows
  bus.rs                                 + flash windows, unsupported relay
  machine.rs                             + with_flash, virtual clock
  periph/mod.rs                          + Accepted, Storage::accepted, AcceptedBlock, accepted_without_effect()
  periph/spi_mem.rs                      Spi0, Spi1
  periph/extmem.rs                       full cache controller
  periph/mmu.rs                          MmuTable
  periph/sha.rs                          Sha
  periph/ana_i2c.rs                      regi2c master
  periph/apb_ctrl.rs                     RNG + two power registers
  periph/efuse.rs  rtc_cntl.rs  system.rs  gpio.rs  uart.rs  assist_debug.rs   extended
crates/soc-esp32c3/tests/
  accepted_without_effect.rs             pins docs/registers-accepted-without-effect.md; rewrites its table with INKEMU_BLESS=1
  real_rom.rs                            + flash path against the real ROM (ignored without INKEMU_ROM_ELF_DIR)
  reference_boot.rs                      milestone tests (ignored without INKEMU_REF_FLASH)
crates/boards/profiles/x4.toml  x3.toml  + [flash]
crates/boards/src/lib.rs                 + flash, id validation, instantiate_with_flash
crates/firmware/src/flash_image.rs       check_flash_image (spec §5.1 refusals for a full image)
crates/firmware/src/builder.rs           + build_flash_image (test support: the one full flash image the tests use)
crates/emu-cli/src/flash_file.rs         bounded read of the --flash file
crates/emu-cli/src/main.rs  run.rs  exit.rs  rom_store.rs
tools/conformance/riscv-tests-skip.txt  README.md  run-arch-test.sh   PMP notes reworded (Task 3)
tools/reference-firmware/build.sh        builds bootloader + hello_world in Docker, checks the pins, merges
tools/reference-firmware/in-container.sh sdkconfig.defaults  SHA256SUMS.inputs  SHA256SUMS.output  README.md
.github/workflows/ci.yml                 + job reference-boot
docs/registers-accepted-without-effect.md
docs/adr/0002-flash-model-and-cache-windows.md
docs/research/esp32c3-boot-chain.md  sp1b-test-oracle.md   corrected (D14)
docs/superpowers/notes/2026-10-11-sp1b-2-carryover.md
README.md
```

---

### Task 0: RustCrypto 0.11 — `sha2`, `md-5` and `sha1` at the current line, digests printed through a `hex` helper

**Files:**
- Modify: `Cargo.toml` (`[workspace.dependencies]`), `Cargo.lock`
- Modify: `crates/emu-cli/src/term.rs`, `crates/emu-cli/src/rom_store.rs`, `crates/emu-cli/tests/riscv_tests.rs`
- Modify: `crates/svd-gen/src/main.rs`
- Create: `crates/soc-esp32c3/tests/common/mod.rs`
- Modify: `crates/soc-esp32c3/tests/real_rom.rs`

**Interfaces:**
- Consumes: `sha2::{Digest, Sha256}`, `md5::{Digest, Md5}` as the code uses them today.
- Produces:
  ```rust
  // Cargo.toml, [workspace.dependencies]
  sha2 = "0.11"
  sha1 = "0.11"      // declared here, first used by Task 8
  md-5 = "0.11"
  // crates/emu-cli/src/term.rs
  /// Lower-case hexadecimal of `bytes`.
  pub fn hex(bytes: &[u8]) -> String;
  // crates/svd-gen/src/main.rs: the same function, private
  // crates/soc-esp32c3/tests/common/mod.rs: the same function, `pub`, for `mod common;` in the integration tests
  ```

Facts about the 0.11 line (decision D20; each checked by compiling and running): `Sha256::digest(..)` and `Md5::digest(..)` return a `hybrid_array::Array<u8, N>` instead of a `GenericArray`. It dereferences to `[u8]`, keeps `as_slice()` and `to_vec()`, and is still an `IntoIterator` of bytes, so `out.extend(digest)` and `Md5::digest(x).as_slice() != &e[16..32]` compile unchanged: **the `firmware` crate needs no source change** (`partition.rs`, `builder.rs`, `image.rs`). The array no longer implements `LowerHex`: `format!("{:x}", digest)` and `format!("{digest:x}")` stop compiling. There are exactly four such call sites: `crates/emu-cli/src/rom_store.rs` (`sha256_hex`, and the helper `sha` of its `tests` module), `crates/emu-cli/tests/riscv_tests.rs`, `crates/svd-gen/src/main.rs` (`generate`), `crates/soc-esp32c3/tests/real_rom.rs` (`image_sha256`).

- [ ] **Step 1: Write the failing test of the helper** (`crates/emu-cli/src/term.rs`, module `tests`)

```rust
    #[test]
    fn hex_writes_two_lower_case_digits_per_byte() {
        assert_eq!(hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
        assert_eq!(hex(&[]), "");
    }
```

- [ ] **Step 2: Run it, see the failure**

Run: `cargo test -p emu-cli term > /tmp/t0.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, "cannot find function `hex`".

- [ ] **Step 3: Implement the helper** (`crates/emu-cli/src/term.rs`, above the `tests` module)

```rust
/// Lower-case hexadecimal of `bytes`: the form in which digests are pinned, compared and shown.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(2 * bytes.len());
    for byte in bytes {
        // Writing to a String cannot fail.
        let _ = write!(text, "{byte:02x}");
    }
    text
}
```

(A `map(|b| format!("{b:02x}")).collect::<String>()` would be shorter and is refused by clippy: `format_collect`.)

Run: `cargo test -p emu-cli term > /tmp/t0.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 4: Move the workspace to the 0.11 line**

Root `Cargo.toml`, `[workspace.dependencies]`: replace `sha2 = "0.10"` and `md-5 = "0.10"` by

```toml
sha2 = "0.11"
sha1 = "0.11"
md-5 = "0.11"
```

No crate manifest changes: every crate takes these with `.workspace = true`, and `sha1` is used from Task 8 on (until then it is declared and absent from `Cargo.lock`; `cargo deny check` accepts that).

Run: `cargo build --workspace --all-targets > /tmp/t0build.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, with errors `the trait bound … LowerHex is not satisfied` at the call sites listed above and no other kind of error (cargo stops building what depends on a crate that failed, so one run may not show all four).

- [ ] **Step 5: Print digests through the helper**

`crates/emu-cli/src/rom_store.rs`: import the helper (`use crate::term::{hex, sanitize};`) and replace the body of `sha256_hex`:

```rust
fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
```

In its `tests` module, the helper `sha` becomes `sha256_hex(bytes)` and the line `use sha2::{Digest, Sha256};` of that module is deleted (nothing else there uses it; left in place it is an unused import, which `-D warnings` refuses).

`crates/emu-cli/tests/riscv_tests.rs`: replace `format!("{digest:x}"),` by `emu_cli::term::hex(&digest),`.

`crates/svd-gen/src/main.rs`: add the private helper above `generate` and use it there (`let found = hex(&Sha256::digest(svd_xml.as_bytes()));`):

```rust
/// Lower-case hexadecimal of `bytes`, the form the pinned digest is written in.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(2 * bytes.len());
    for byte in bytes {
        // Writing to a String cannot fail.
        let _ = write!(text, "{byte:02x}");
    }
    text
}
```

Its tests already exist: `vendored_svd_generates_the_documented_2605_registers` passes only when `hex` of the SVD digest equals the pinned lower-case digest, and `a_modified_svd_is_refused_before_anything_is_generated` only when another digest differs.

Create `crates/soc-esp32c3/tests/common/mod.rs`:

```rust
//! Helpers shared by the integration tests of this crate.

/// Lower-case hexadecimal of `bytes`: the form in which digests are pinned and printed.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(2 * bytes.len());
    for byte in bytes {
        // Writing to a String cannot fail.
        let _ = write!(text, "{byte:02x}");
    }
    text
}
```

`crates/soc-esp32c3/tests/real_rom.rs`: add `mod common;` above the first `use`, and in `image_sha256` replace `format!("{:x}", Sha256::digest(rom.bytes()))` by `common::hex(&Sha256::digest(rom.bytes()))`.

- [ ] **Step 6: Prove that nothing else moved**

Run: `cargo fmt --all > /tmp/t0fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/t0fmtc.log 2>&1; echo "exit=$?"`
Expected: `exit=0` twice.
Run: `cargo test --workspace > /tmp/t0.log 2>&1; echo "exit=$?"`
Expected: `exit=0`: every test that existed before this task passes, among them the MD5 and SHA-256 tests of `firmware` (`detects_corrupted_md5`, the `parse` tests of `image.rs`), `fixture_directory_matches_the_sha256_manifest` (`riscv_tests`) and the `rom_store` tests that compare digests.
Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/t0c.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.
Run: `cargo deny check > /tmp/t0deny.log 2>&1; echo "exit=$?"`
Expected: `exit=0` (`advisories ok, bans ok, licenses ok, sources ok`; `sha1`, `sha2` and `md-5` are MIT OR Apache-2.0, like their new dependencies `hybrid-array`, `block-buffer` 0.12, `crypto-common` 0.2 and `cpufeatures` 0.3).
Run: `cargo run -p svd-gen -- --check > /tmp/t0svd.log 2>&1; echo "exit=$?"`
Expected: `exit=0`: the generator still recognises the vendored SVD by its digest and the generated table is unchanged.
Run: `grep -c 'name = "digest"' Cargo.lock; grep -A1 'name = "digest"' Cargo.lock | grep version`
Expected: `1`, then `version = "0.11.…"`: one RustCrypto generation in the tree.
With the ROM files at hand (`INKEMU_ROM_ELF_DIR`), also run `cargo test --release -p soc-esp32c3 --test real_rom -- --ignored > /tmp/t0rom.log 2>&1; echo "exit=$?"` (expected `exit=0`: `converted_images_match_their_pinned_hashes` compares three digests through `common::hex`).

- [ ] **Step 7: Commit**

Before committing (Global Constraints): `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`: `exit=0` three times. The code blocks of this plan are not wrapped as `rustfmt` wraps them.

```bash
/usr/bin/git add Cargo.toml Cargo.lock crates/emu-cli/src/term.rs crates/emu-cli/src/rom_store.rs crates/emu-cli/tests/riscv_tests.rs crates/svd-gen/src/main.rs crates/soc-esp32c3/tests/common/mod.rs crates/soc-esp32c3/tests/real_rom.rs
/usr/bin/git commit -m "chore(deps): move sha2 and md-5 to RustCrypto 0.11 and declare sha1 0.11

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- Cargo.toml Cargo.lock crates/emu-cli/src/term.rs crates/emu-cli/src/rom_store.rs crates/emu-cli/tests/riscv_tests.rs crates/svd-gen/src/main.rs crates/soc-esp32c3/tests/common/mod.rs crates/soc-esp32c3/tests/real_rom.rs
```

---

### Task 1: `svd-regs` — register addresses by name

**Files:**
- Modify: `crates/svd-regs/src/lib.rs`, `crates/svd-regs/src/tests.rs`
- Modify: `crates/soc-esp32c3/src/machine.rs` (tests), `crates/soc-esp32c3/src/periph/uart.rs` (tests), `crates/emu-cli/src/run.rs` (tests)

**Interfaces:**
- Consumes: `svd_regs::RegisterInfo { addr, peripheral, name, reset_value, reset_explicit }`, the generated `pub(crate) static REGISTERS: [RegisterInfo; 2605]`.
- Produces:
  ```rust
  pub mod esp32c3 {
      /// Absolute address of `peripheral.name`. Used in a `const`, an unknown name fails the build.
      pub const fn addr(peripheral: &str, name: &str) -> u32;
      /// The register `peripheral.name`, if the SVD documents it.
      pub fn find(peripheral: &str, name: &str) -> Option<&'static RegisterInfo>;
  }
  ```

- [ ] **Step 1: Write the failing tests** (append to `crates/svd-regs/src/tests.rs`)

```rust
#[test]
fn addr_resolves_names_to_the_addresses_the_reports_measured() {
    use crate::esp32c3::addr;
    assert_eq!(addr("UART0", "FIFO"), 0x6000_0000);
    assert_eq!(addr("EXTMEM", "ICACHE_SYNC_CTRL"), 0x600C_4028); // CM §2
    assert_eq!(addr("SHA", "M_MEM0"), 0x6003_B080); // FB §B.7
    assert_eq!(addr("SPI1", "W0"), 0x6000_2058); // FB §B.6
    assert_eq!(addr("RNG", "DATA"), 0x6002_60B0); // BA §2 row 88
    assert_eq!(addr("TIMG0", "RTCCALICFG"), 0x6001_F068); // BA §9
}

#[test]
fn addr_is_usable_in_a_constant() {
    const STRAP_OFFSET: u32 =
        crate::esp32c3::addr("GPIO", "STRAP") - crate::esp32c3::addr("GPIO", "BT_SELECT");
    assert_eq!(STRAP_OFFSET, 0x38);
}

#[test]
fn find_returns_the_whole_entry_or_none() {
    let reg = crate::esp32c3::find("SPI1", "CTRL").unwrap();
    assert_eq!((reg.addr, reg.reset_value, reg.reset_explicit), (0x6000_2008, 0x002C_A000, true));
    assert!(crate::esp32c3::find("SPI1", "NOPE").is_none());
    assert!(crate::esp32c3::find("NOPE", "CTRL").is_none());
}

#[test]
#[should_panic(expected = "no such register in the ESP32-C3 SVD")]
fn addr_of_an_unknown_register_panics_when_evaluated_at_run_time() {
    let name = std::hint::black_box("NOPE");
    let _ = crate::esp32c3::addr("SPI1", name);
}
```

`GPIO.BT_SELECT` is the first GPIO register of the SVD (`0x6000_4000`), so the difference is the offset of `STRAP`.

- [ ] **Step 2: Run them, see the compile failure**

Run: `cargo test -p svd-regs > /tmp/t1.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, "cannot find function `addr`".

- [ ] **Step 3: Implement** (in `crates/svd-regs/src/lib.rs`, inside `pub mod esp32c3`)

```rust
    const fn same(a: &str, b: &str) -> bool {
        let (a, b) = (a.as_bytes(), b.as_bytes());
        if a.len() != b.len() {
            return false;
        }
        let mut i = 0;
        while i < a.len() {
            if a[i] != b[i] {
                return false;
            }
            i += 1;
        }
        true
    }

    const fn position(peripheral: &str, name: &str) -> Option<usize> {
        let mut i = 0;
        while i < REGISTERS.len() {
            if same(REGISTERS[i].peripheral, peripheral) && same(REGISTERS[i].name, name) {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    /// Absolute address of the register `peripheral.name`.
    ///
    /// Meant for constants: `const CMD: u32 = addr("SPI1", "CMD") - SPI1_BASE;`. There, a name the
    /// SVD does not contain stops the build.
    ///
    /// # Panics
    /// When the SVD has no such register (at compile time in a `const`).
    #[must_use]
    pub const fn addr(peripheral: &str, name: &str) -> u32 {
        match position(peripheral, name) {
            Some(i) => REGISTERS[i].addr,
            None => panic!("no such register in the ESP32-C3 SVD"),
        }
    }

    /// The register `peripheral.name`, if the SVD documents it.
    #[must_use]
    pub fn find(peripheral: &str, name: &str) -> Option<&'static RegisterInfo> {
        match position(peripheral, name) {
            Some(i) => REGISTERS.get(i),
            None => None,
        }
    }
```

`tests.rs` is compiled with `std` (the crate is `no_std` only outside tests): if `std::hint` is not reachable there, write `core::hint::black_box`.

- [ ] **Step 4: Replace the literal bases of the three test modules**

In `crates/soc-esp32c3/src/periph/uart.rs` (module `tests`): replace `const UART0: u32 = 0x6000_0000;` by `const UART0: u32 = svd_regs::esp32c3::addr("UART0", "FIFO");`.

In `crates/soc-esp32c3/src/machine.rs` (module `tests`): add

```rust
    const UART0: u32 = svd_regs::esp32c3::addr("UART0", "FIFO");
    const USB: u32 = svd_regs::esp32c3::addr("USB_DEVICE", "EP1");
```

and replace the three `print(0x6000_0000, …)` by `print(UART0, …)` and `print(0x6004_3000, …)` by `print(USB, …)`.

In `crates/emu-cli/src/run.rs` (module `tests`): replace the two constants by

```rust
    const UART0: u32 = svd_regs::esp32c3::addr("UART0", "FIFO");
    const USB: u32 = svd_regs::esp32c3::addr("USB_DEVICE", "EP1");
```

(`svd-regs` is already a dev-dependency of `emu-cli`.)

- [ ] **Step 5: Run the workspace tests**

Run: `cargo test --workspace > /tmp/t1.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.
Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/t1c.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 6: Commit**

Before committing (Global Constraints): `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`: `exit=0` three times. The code blocks of this plan are not wrapped as `rustfmt` wraps them.

```bash
/usr/bin/git add crates/svd-regs crates/soc-esp32c3/src/machine.rs crates/soc-esp32c3/src/periph/uart.rs crates/emu-cli/src/run.rs
/usr/bin/git commit -m "feat(svd-regs): look registers up by name at compile time

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/svd-regs crates/soc-esp32c3/src/machine.rs crates/soc-esp32c3/src/periph/uart.rs crates/emu-cli/src/run.rs
```

---

### Task 2: Diagnostics for "cannot be right silently" events, and the table of registers accepted without effect

**Files:**
- Modify: `crates/soc-esp32c3/src/diag.rs`, `crates/soc-esp32c3/src/periph/mod.rs`, `crates/soc-esp32c3/src/machine.rs` (doc comment of `StopReason::StrictMmio`)
- Modify: `crates/soc-esp32c3/src/periph/{io_mux,rtc_cntl,system,uart,intmatrix,assist_debug,extmem}.rs` (one `accepted()` function each)
- Create: `docs/registers-accepted-without-effect.md`, `crates/soc-esp32c3/tests/accepted_without_effect.rs`

**Interfaces:**
- Consumes: `svd_regs::esp32c3::{addr, find, registers}` (Task 1); `Diagnostic`, `Diagnostics`, `Direction`; `Storage`.
- Produces:
  ```rust
  // diag.rs
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum Unsupported {
      FlashCommand { opcode: u8 },
      ShaDma,
      ShaMode { mode: u32 },
      ClockSource { sysclk_conf: u32, cpu_per_conf: u32 },
      SoftwareReset { options0: u32 },
  }
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum MappingFault { InvalidEntry, BeyondFlash }
  // new variants of `Diagnostic`:
  //   Unsupported { what: Unsupported, pc: u32 }
  //   FlashMapping { vaddr: u32, entry: u32, content: u32, fault: MappingFault, pc: u32 }

  // periph/mod.rs
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct Accepted {
      pub peripheral: &'static str,
      pub register: &'static str,   // a register name, or the first register of a block
      pub addr: u32,
      pub count: u32,               // 1, or the number of registers of the block
      pub not_modeled: &'static str,
      pub heir: &'static str,       // the plan that makes it real
  }
  impl Accepted {
      pub const fn svd(peripheral: &'static str, register: &'static str, not_modeled: &'static str, heir: &'static str) -> Accepted;
      pub const fn at(peripheral: &'static str, register: &'static str, addr: u32, not_modeled: &'static str, heir: &'static str) -> Accepted;
      pub const fn block(self, count: u32) -> Accepted;
  }
  impl Storage { pub fn accepted(base: u32, list: &[Accepted]) -> Storage; }
  /// A block made only of registers accepted without effect.
  #[derive(Clone, Debug)]
  pub struct AcceptedBlock { /* private */ }
  impl AcceptedBlock { pub fn new(base: u32, list: &[Accepted]) -> AcceptedBlock; }
  impl Peripheral for AcceptedBlock { /* storage */ }
  pub fn accepted_without_effect() -> Vec<Accepted>;      // every list, sorted by address
  pub fn render_accepted_table(rows: &[Accepted]) -> String;
  ```
  Each peripheral module exposes `pub(crate) fn accepted() -> Vec<Accepted>`.

- [ ] **Step 1: Write the failing tests for the diagnostics** (append to the `tests` module of `diag.rs`)

```rust
    fn unsupported(what: Unsupported) -> Diagnostic {
        Diagnostic::Unsupported { what, pc: 0x4038_1000 }
    }
    fn mapping(entry: u32, fault: MappingFault) -> Diagnostic {
        Diagnostic::FlashMapping { vaddr: 0x3C00_0000 + (entry << 16), entry, content: 0x100, fault, pc: 0x4004_a186 }
    }

    #[test]
    fn strict_mode_stops_on_the_events_that_cannot_be_right_silently() {
        for diagnostic in [
            unsupported(Unsupported::FlashCommand { opcode: 0x02 }),
            unsupported(Unsupported::ShaDma),
            unsupported(Unsupported::ShaMode { mode: 5 }),
            unsupported(Unsupported::ClockSource { sysclk_conf: 0xC00, cpu_per_conf: 0xC }),
            unsupported(Unsupported::SoftwareReset { options0: 0x8000_0000 }),
            mapping(3, MappingFault::InvalidEntry),
            mapping(3, MappingFault::BeyondFlash),
        ] {
            let mut strict = Diagnostics::new(true);
            strict.record(diagnostic.clone());
            assert_eq!(strict.take_violation(), Some(diagnostic.clone()), "{diagnostic}");
            let mut lenient = Diagnostics::new(false);
            lenient.record(diagnostic.clone());
            assert_eq!(lenient.take_violation(), None);
            assert_eq!(lenient.drain(), vec![diagnostic]);
        }
    }

    #[test]
    fn a_cpu_exception_is_still_not_a_strict_violation() {
        let mut d = Diagnostics::new(true);
        d.record(Diagnostic::Exception { cause: cpu_rv32::Exception::IllegalInstr, tval: 0, pc: 4 });
        assert_eq!(d.take_violation(), None);
    }

    #[test]
    fn unsupported_events_are_deduplicated_by_what_they_are_about() {
        let mut d = Diagnostics::new(false);
        d.record(unsupported(Unsupported::FlashCommand { opcode: 0x02 }));
        d.record(Diagnostic::Unsupported { what: Unsupported::FlashCommand { opcode: 0x02 }, pc: 0x4200_0000 });
        d.record(unsupported(Unsupported::FlashCommand { opcode: 0x20 }));
        d.record(mapping(3, MappingFault::InvalidEntry));
        d.record(mapping(3, MappingFault::InvalidEntry));
        d.record(mapping(4, MappingFault::InvalidEntry));
        assert_eq!(d.distinct(), 4);
        assert_eq!(d.occurrences(&unsupported(Unsupported::FlashCommand { opcode: 0x02 })), 2);
    }

    #[test]
    fn new_messages_state_the_cause_then_the_remedy() {
        let texts = [
            (unsupported(Unsupported::FlashCommand { opcode: 0x02 }), "flash command 0x02 is not modeled"),
            (unsupported(Unsupported::ShaDma), "DMA-SHA"),
            (unsupported(Unsupported::ShaMode { mode: 5 }), "SHA.MODE 5"),
            (unsupported(Unsupported::ClockSource { sysclk_conf: 0xC00, cpu_per_conf: 0xC }), "SYSCLK_CONF 0x00000c00"),
            (unsupported(Unsupported::SoftwareReset { options0: 0x8000_0000 }), "software reset"),
            (mapping(3, MappingFault::InvalidEntry), "MMU entry 3"),
            (mapping(3, MappingFault::BeyondFlash), "beyond the end of the flash"),
        ];
        for (diagnostic, needle) in texts {
            let text = diagnostic.to_string();
            assert!(text.contains(needle), "{text}");
            assert!(text.contains("pc 0x40"), "{text}");
            let (cause, remedy) = text.split_once("; ").unwrap_or_else(|| panic!("no remedy in: {text}"));
            assert!(!cause.is_empty() && !remedy.is_empty(), "{text}");
        }
    }
```

- [ ] **Step 2: Run, see the failure**

Run: `cargo test -p soc-esp32c3 diag > /tmp/t2.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, "cannot find type `Unsupported`".

- [ ] **Step 3: Implement the diagnostics** (`diag.rs`)

Add the two enums of the Interfaces block (with these doc comments: `FlashCommand` "an SPI flash command the read-only flash model does not implement (D3)", `ShaDma` "`SHA.DMA_START`/`DMA_CONTINUE` (D7)", `ShaMode` "a `SHA.MODE` the ESP32-C3 accelerator does not have", `ClockSource` "a clock selection without a known CPU frequency (D22)", `SoftwareReset` "a reset requested through `RTC_CNTL.OPTIONS0` (D27)"; `InvalidEntry` "bit 8 of the MMU entry is set", `BeyondFlash` "the entry names a page past the end of the flash"). Add the two variants to `Diagnostic`.

Keys (extend `Diagnostic::key`; `Suppressed` keeps `(4, 0, 0)`):

```rust
            Diagnostic::Unsupported { what, .. } => match what {
                Unsupported::FlashCommand { opcode } => (5, u32::from(opcode), 0),
                Unsupported::ShaDma => (6, 0, 0),
                Unsupported::ShaMode { mode } => (7, mode, 0),
                Unsupported::ClockSource { sysclk_conf, cpu_per_conf } => (8, sysclk_conf, cpu_per_conf),
                Unsupported::SoftwareReset { .. } => (9, 0, 0),
            },
            Diagnostic::FlashMapping { entry, fault, .. } => (10, entry, fault as u32),
```

Rename `is_mmio` to `stops_a_strict_run` and make it true for `UnmodeledMmio`, `SubWordWrite`, `Unsupported` and `FlashMapping` (update its one caller in `record` and the doc comment of `take_violation`: "the first unmodeled access, sub-word register write, unsupported operation or invalid flash mapping").

`Display` arms:

```rust
            Diagnostic::Unsupported { what, pc } => match what {
                Unsupported::FlashCommand { opcode } => write!(
                    f,
                    "flash command {opcode:#04x} is not modeled (pc {pc:#010x}): the virtual flash is read-only and did nothing; run firmware that does not program or erase flash until flash writes are emulated"
                ),
                Unsupported::ShaDma => write!(
                    f,
                    "DMA-SHA was started (pc {pc:#010x}) but only the typical SHA mode is modeled, so no digest was produced; use firmware that hashes through SHA.START/CONTINUE until DMA-SHA is emulated"
                ),
                Unsupported::ShaMode { mode } => write!(
                    f,
                    "SHA.MODE {mode} selects an algorithm the ESP32-C3 accelerator does not have (pc {pc:#010x}), so the block was not hashed; use SHA-1 (0), SHA-224 (1) or SHA-256 (2)"
                ),
                Unsupported::ClockSource { sysclk_conf, cpu_per_conf } => write!(
                    f,
                    "the clock registers select a CPU clock inkemu cannot model (SYSCLK_CONF {sysclk_conf:#010x}, CPU_PER_CONF {cpu_per_conf:#010x}, pc {pc:#010x}), so emulated time keeps the previous frequency; select the crystal or the PLL at 80 or 160 MHz"
                ),
                Unsupported::SoftwareReset { options0 } => write!(
                    f,
                    "a software reset was requested through RTC_CNTL.OPTIONS0 (value {options0:#010x}, pc {pc:#010x}) but resets are not emulated yet, so the write was ignored; restart inkemu to reset the device"
                ),
            },
            Diagnostic::FlashMapping { vaddr, entry, content, fault, pc } => match fault {
                MappingFault::InvalidEntry => write!(
                    f,
                    "access to the flash window at {vaddr:#010x} through MMU entry {entry}, which is invalid (content {content:#05x}, pc {pc:#010x}): 0 was returned; the firmware used flash it had not mapped"
                ),
                MappingFault::BeyondFlash => write!(
                    f,
                    "access to the flash window at {vaddr:#010x} through MMU entry {entry} (content {content:#05x}, pc {pc:#010x}), which names a page beyond the end of the flash: 0 was returned; check the flash size of the device profile"
                ),
            },
```

In `machine.rs`, reword the doc comment of `StopReason::StrictMmio`: "`--strict-mmio`: the program touched a register the emulator does not model, wrote a peripheral register with an 8- or 16-bit store, asked for an operation the emulator cannot perform faithfully, or used flash it had not mapped."

- [ ] **Step 4: Run the diagnostics tests**

Run: `cargo test -p soc-esp32c3 diag > /tmp/t2.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 5: Write the failing tests for the accepted registers** (append to the `tests` module of `periph/mod.rs`, and create the integration test)

```rust
    #[test]
    fn accepted_storage_takes_its_registers_and_reset_values_from_the_list() {
        // `SPI1.CMD` is the first SPI1 register of the SVD: its address is the base of the block.
        const SPI1: u32 = svd_regs::esp32c3::addr("SPI1", "CMD");
        const CTRL: u32 = svd_regs::esp32c3::addr("SPI1", "CTRL") - SPI1;
        const CTRL1: u32 = svd_regs::esp32c3::addr("SPI1", "CTRL1") - SPI1;
        // `DR_REG_SPI0_BASE` (BC §A.2) and SPI0 `MISO_DLEN`, which the SVD omits (FB §B.2).
        const SPI0: u32 = 0x6000_3000;
        const MISO_DLEN: u32 = 0x28;
        const LIST: [Accepted; 2] = [
            Accepted::svd("SPI1", "CTRL", "read mode", "SP8"),
            Accepted::at("SPI0", "MISO_DLEN", SPI0 + MISO_DLEN, "cache line length", "SP8"),
        ];
        let mut s = Storage::accepted(SPI1, &LIST[..1]);
        assert_eq!(s.read(CTRL), Some(0x002C_A000), "SVD reset value of SPI1.CTRL (FB §D.2)");
        assert_eq!(s.read(CTRL1), None, "not in the list");
        let mut other = Storage::accepted(SPI0, &LIST[1..]);
        assert_eq!(other.read(MISO_DLEN), Some(0), "no SVD entry: reset 0");
        assert!(other.write(MISO_DLEN, 0xFF));
        assert!(s.write(CTRL, 1));
    }

    #[test]
    fn an_accepted_block_is_storage_and_nothing_else() {
        // `SENSITIVE.ROM_TABLE_LOCK` is the first SENSITIVE register of the SVD: the base of the block.
        const BASE: u32 = svd_regs::esp32c3::addr("SENSITIVE", "ROM_TABLE_LOCK");
        const USAGE_1: u32 = svd_regs::esp32c3::addr("SENSITIVE", "INTERNAL_SRAM_USAGE_1") - BASE;
        const LIST: [Accepted; 1] = [Accepted::svd("SENSITIVE", "INTERNAL_SRAM_USAGE_1", "SRAM0 given to the cache", "SP8")];
        let mut block = AcceptedBlock::new(BASE, &LIST);
        assert_eq!(block.read(USAGE_1), Some(0xF), "CM §2.1");
        assert!(block.write(USAGE_1, 0xE));
        assert_eq!(block.read(USAGE_1), Some(0xE));
        assert_eq!(block.read(USAGE_1 + 4), None, "the next register is not in the list");
        assert!(!block.write(USAGE_1 + 4, 0));
    }

    #[test]
    fn the_table_lists_every_register_once_in_address_order() {
        let rows = accepted_without_effect();
        assert!(rows.windows(2).all(|w| w[0].addr < w[1].addr), "sorted, no duplicate address");
        assert!(rows.iter().all(|r| r.count >= 1 && !r.not_modeled.is_empty() && r.heir.starts_with("SP")));
        let io_mux = rows.iter().find(|r| r.peripheral == "IO_MUX").unwrap();
        assert_eq!(io_mux.count, 24, "every IO_MUX register of the SVD");
        let table = render_accepted_table(&rows);
        assert!(table.starts_with("| Address | Register | Count | Hardware effect not modelled | Made real by |\n|---|---|---|---|---|\n"));
        assert_eq!(table.lines().count(), rows.len() + 2);
    }
```

`SENSITIVE.ROM_TABLE_LOCK` is the first SENSITIVE register of the SVD (`0x600C_1000`) and `SPI1.CMD` the first SPI1 one (`0x6000_2000`): their addresses are the bases of the two blocks. SPI0 has no register at its base in the SVD, hence the one named literal.

`crates/soc-esp32c3/tests/accepted_without_effect.rs`:

```rust
//! Pins `docs/registers-accepted-without-effect.md` to the lists the peripheral models are built from.
//!
//! When the lists change, `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect`
//! rewrites the table of that file, and of nothing else; review its diff, then commit it.
use soc_esp32c3::periph::{accepted_without_effect, render_accepted_table};
use std::path::Path;

/// The document, relative to the directory of this crate.
const DOCUMENT: &str = "../../docs/registers-accepted-without-effect.md";
/// The line of the document after which everything is the generated table.
const MARKER: &str = "<!-- generated table -->";
const BLESS: &str = "INKEMU_BLESS";

#[test]
fn the_documented_table_is_the_one_the_code_builds() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(DOCUMENT);
    let document =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let (prose, rest) = document
        .split_once(MARKER)
        .unwrap_or_else(|| panic!("{} has no `{MARKER}` line; restore it above the table", path.display()));
    let table = render_accepted_table(&accepted_without_effect());
    if rest.strip_prefix('\n').unwrap_or(rest) == table {
        return;
    }
    assert!(
        std::env::var_os(BLESS).is_some_and(|value| value == "1"),
        "docs/registers-accepted-without-effect.md is stale; run `{BLESS}=1 cargo test -p soc-esp32c3 --test accepted_without_effect` to rewrite its table, then review the diff of that file"
    );
    std::fs::write(&path, format!("{prose}{MARKER}\n{table}"))
        .unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
}
```

What this test guarantees, and must keep guaranteeing: without `INKEMU_BLESS=1` (unset, or any other value) a stale document makes it **fail** and nothing is written; with `INKEMU_BLESS=1` it rewrites what follows the marker line of that one file, whose path is fixed in the test, and passes; a document that is already current is never rewritten; a document without its marker line fails in both cases.

- [ ] **Step 6: Run, see the failure**

Run: `cargo test -p soc-esp32c3 > /tmp/t2.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, "cannot find struct `Accepted`".

- [ ] **Step 7: Implement `Accepted`, `Storage::accepted`, `AcceptedBlock`, the per-module lists and the table**

In `periph/mod.rs`:

```rust
/// A register whose value the emulator stores although the hardware effect is not modelled
/// (decision D12). The lists of these are what the storage of each model is built from, and what
/// `docs/registers-accepted-without-effect.md` is rendered from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accepted {
    pub peripheral: &'static str,
    /// The register, or the first register of a block of `count` consecutive ones.
    pub register: &'static str,
    pub addr: u32,
    pub count: u32,
    pub not_modeled: &'static str,
    /// The plan that gives the register its effect.
    pub heir: &'static str,
}

impl Accepted {
    /// A register of the SVD; an unknown name fails the build when the list is a `const`.
    #[must_use]
    pub const fn svd(peripheral: &'static str, register: &'static str, not_modeled: &'static str, heir: &'static str) -> Accepted {
        Accepted::at(peripheral, register, svd_regs::esp32c3::addr(peripheral, register), not_modeled, heir)
    }

    /// A register the SVD does not contain, at an address documented where the constant is defined.
    #[must_use]
    pub const fn at(peripheral: &'static str, register: &'static str, addr: u32, not_modeled: &'static str, heir: &'static str) -> Accepted {
        Accepted { peripheral, register, addr, count: 1, not_modeled, heir }
    }

    /// The same row standing for `count` consecutive registers.
    #[must_use]
    pub const fn block(self, count: u32) -> Accepted {
        Accepted { count, ..self }
    }
}
```

```rust
impl Storage {
    /// The single registers of `list` (rows with `count == 1`), at their SVD reset value, 0 without one.
    #[must_use]
    pub fn accepted(base: u32, list: &[Accepted]) -> Storage {
        let offsets: Vec<u32> = list.iter().filter(|a| a.count == 1).map(|a| a.addr - base).collect();
        Storage::for_offsets(base, &offsets)
    }
}

#[derive(Clone, Debug)]
pub struct AcceptedBlock {
    registers: Storage,
}

impl AcceptedBlock {
    #[must_use]
    pub fn new(base: u32, list: &[Accepted]) -> AcceptedBlock {
        AcceptedBlock { registers: Storage::accepted(base, list) }
    }
}

impl Peripheral for AcceptedBlock {
    fn read(&mut self, offset: u32) -> Option<u32> {
        self.registers.read(offset)
    }
    fn write(&mut self, offset: u32, value: u32) -> bool {
        self.registers.write(offset, value)
    }
}

/// Every register accepted without effect, in address order.
#[must_use]
pub fn accepted_without_effect() -> Vec<Accepted> {
    let mut rows = [
        io_mux::accepted(),
        rtc_cntl::accepted(),
        system::accepted(),
        uart::accepted(),
        intmatrix::accepted(),
        assist_debug::accepted(),
        extmem::accepted(),
    ]
    .concat();
    rows.sort_by_key(|row| row.addr);
    rows
}

/// The Markdown table of `docs/registers-accepted-without-effect.md`.
#[must_use]
pub fn render_accepted_table(rows: &[Accepted]) -> String {
    use std::fmt::Write as _;
    let mut table = String::from(
        "| Address | Register | Count | Hardware effect not modelled | Made real by |\n|---|---|---|---|---|\n",
    );
    for row in rows {
        // Writing to a String cannot fail.
        let _ = writeln!(
            table,
            "| `{:#010x}` | `{}.{}` | {} | {} | {} |",
            row.addr, row.peripheral, row.register, row.count, row.not_modeled, row.heir
        );
    }
    table
}
```

Per-module lists. Each module keeps its current storage; where it used `Storage::for_offsets(BASE, &CONST_ARRAY)` it now builds the storage with `Storage::accepted(BASE, &ACCEPTED)` so the list is the single source (the offsets must not change: the existing tests of each module stay green). Each of the seven modules imports `Accepted` next to what it already takes from the parent module (`use super::{Accepted, …};` or `use crate::periph::{Accepted, …};`; `intmatrix.rs` and `assist_debug.rs` gain `use super::Accepted;`).

`io_mux.rs`:

```rust
/// One row for the whole block: every pad register is stored (report BC §A.2).
pub(crate) fn accepted() -> Vec<Accepted> {
    let pads: Vec<_> = svd_regs::esp32c3::registers().iter().filter(|r| r.peripheral == "IO_MUX").collect();
    pads.first()
        .map(|first| {
            // The block is far smaller than `u32::MAX` registers.
            let count = u32::try_from(pads.len()).unwrap_or(u32::MAX);
            Accepted::at("IO_MUX", first.name, first.addr, "pad function, pulls, drive strength, input enable", "SP2").block(count)
        })
        .into_iter()
        .collect()
}
```

`rtc_cntl.rs` (replaces `const CONFIG`):

```rust
const ACCEPTED: [Accepted; 5] = [
    Accepted::svd("RTC_CNTL", "PWC", "RTC power-domain control", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "DIG_ISO", "digital pad isolation", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "SWD_CONF", "super watchdog: auto-feed, reset flag, strobes", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "PAD_HOLD", "pad hold", "SP2"),
    Accepted::svd("RTC_CNTL", "DIG_PAD_HOLD", "digital pad hold", "SP2"),
];
pub(crate) fn accepted() -> Vec<Accepted> {
    ACCEPTED.to_vec()
}
```

`system.rs` (replaces `const GATES`):

```rust
const ACCEPTED: [Accepted; 6] = [
    Accepted::svd("SYSTEM", "CPU_PERI_CLK_EN", "clock gates of the CPU peripherals", "SP1b-3"),
    Accepted::svd("SYSTEM", "CPU_PERI_RST_EN", "resets of the CPU peripherals", "SP1b-3"),
    Accepted::svd("SYSTEM", "PERIP_CLK_EN0", "peripheral clock gates", "SP1b-3"),
    Accepted::svd("SYSTEM", "PERIP_CLK_EN1", "peripheral clock gates", "SP1b-3"),
    Accepted::svd("SYSTEM", "PERIP_RST_EN0", "peripheral reset pulses", "SP1b-3"),
    Accepted::svd("SYSTEM", "PERIP_RST_EN1", "peripheral reset pulses", "SP1b-3"),
];
pub(crate) fn accepted() -> Vec<Accepted> {
    ACCEPTED.to_vec()
}
```

`uart.rs`: the model serves two instances, so the list is built per base:

```rust
/// `INT_CLR` is write-only and accepted; the five others are stored.
pub(crate) fn accepted() -> Vec<Accepted> {
    let mut rows = Vec::new();
    for peripheral in ["UART0", "UART1"] {
        for (register, not_modeled) in [
            ("INT_ENA", "interrupt enables: nothing is raised"),
            ("INT_CLR", "interrupt clear: nothing is pending"),
            ("CLKDIV", "baud rate: bytes leave at once"),
            ("CONF0", "frame format"),
            ("CONF1", "FIFO thresholds"),
            ("CLK_CONF", "clock source and divider"),
        ] {
            rows.push(Accepted::svd(peripheral, register, not_modeled, "SP1b-3"));
        }
    }
    rows
}
```

(`Accepted::svd` is evaluated at run time here; the names are those of the constants the model already uses.) The UART storage keeps `Storage::for_offsets(base, &CONFIG)`.

`intmatrix.rs` (rows only; the model is unchanged):

```rust
pub(crate) fn accepted() -> Vec<Accepted> {
    const WHY: &str = "interrupt routing: nothing is delivered to the CPU";
    vec![
        Accepted::svd("INTERRUPT_CORE0", "MAC_INTR_MAP", WHY, "SP1b-3").block(62),
        Accepted::svd("INTERRUPT_CORE0", "CLOCK_GATE", WHY, "SP1b-3"),
        Accepted::svd("INTERRUPT_CORE0", "CPU_INT_ENABLE", WHY, "SP1b-3"),
        Accepted::svd("INTERRUPT_CORE0", "CPU_INT_TYPE", WHY, "SP1b-3"),
        Accepted::svd("INTERRUPT_CORE0", "CPU_INT_CLEAR", "edge-interrupt acknowledge: nothing is pending", "SP1b-3"),
        Accepted::svd("INTERRUPT_CORE0", "CPU_INT_PRI_0", WHY, "SP1b-3").block(32),
        Accepted::svd("INTERRUPT_CORE0", "CPU_INT_THRESH", WHY, "SP1b-3"),
    ]
}
```

`MAC_INTR_MAP` is the map register of source 0 (`0x600C_2000`). Tie the two counts to the model: next to `SOURCE_COUNT` and `CPU_LINE_COUNT` add `const _: () = assert!(SOURCE_COUNT == 62 && CPU_LINE_COUNT == 32);` so a change of either constant breaks the build until the rows follow.

`assist_debug.rs`:

```rust
pub(crate) fn accepted() -> Vec<Accepted> {
    vec![Accepted::svd("ASSIST_DEBUG", "CORE_0_RCD_EN", "PC/SP recording at reset", "SP1b-3")]
}
```

`extmem.rs`:

```rust
pub(crate) fn accepted() -> Vec<Accepted> {
    vec![Accepted::svd("EXTMEM", "ICACHE_CTRL1", "bus gating of the flash windows (SHUT_IBUS, SHUT_DBUS)", "SP1b-4")]
}
```

Create `docs/registers-accepted-without-effect.md`:

```markdown
# Registers accepted without effect

A register listed here **stores what the firmware writes and reads it back**, but the emulator does
not model what the hardware does with the value. Nothing else is silent: any other register is either
modelled or reported by name as unmodelled (spec §5.2), and `--strict-mmio` stops on it.

The table is generated: the lists live next to each peripheral model (`accepted()` in
`crates/soc-esp32c3/src/periph/*.rs`), the storage of each model is built from them, and the test
`crates/soc-esp32c3/tests/accepted_without_effect.rs` fails when this file and the code differ. To
update it, run `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect`: the test
then rewrites the table below, and nothing else; review the diff before committing it.

"Count" is 1 for a single register, or the number of consecutive registers the row stands for.
"Made real by" names the plan that gives the register its effect.

<!-- generated table -->
```

The file ends with the marker line: the table is never typed or pasted by hand (Step 8).

- [ ] **Step 8: Fill the documented table**

Run: `cargo test -p soc-esp32c3 --test accepted_without_effect > /tmp/t2doc.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, "is stale; run `INKEMU_BLESS=1 cargo test …`": without the variable the test fails and writes nothing.
Run: `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect > /tmp/t2doc.log 2>&1; echo "exit=$?"`
Expected: `exit=0`: the test wrote the table under the marker line of `docs/registers-accepted-without-effect.md`.
Check the file (it is new, so read it rather than diff it): the prose above the marker is untouched, and `grep -c '^| .0x' docs/registers-accepted-without-effect.md` prints `33` (12 UART rows, 5 `RTC_CNTL`, 1 `IO_MUX`, 6 `SYSTEM`, 7 `INTERRUPT_CORE0`, 1 `EXTMEM`, 1 `ASSIST_DEBUG`). It is committed in Step 10.

- [ ] **Step 9: Run everything**

Run: `cargo test --workspace > /tmp/t2.log 2>&1; echo "exit=$?"`
Expected: `exit=0` (the pinning test passes without the variable).
Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/t2c.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 10: Commit**

Before committing (Global Constraints): `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`: `exit=0` three times. The code blocks of this plan are not wrapped as `rustfmt` wraps them.

```bash
/usr/bin/git add crates/soc-esp32c3 docs/registers-accepted-without-effect.md
/usr/bin/git commit -m "feat(soc): strict stops for unsupported operations, and the table of registers accepted without effect

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3 docs/registers-accepted-without-effect.md
```

---

### Task 3: `cpu-rv32` — PMP CSRs on the ESP32-C3 hart (stored, not enforced)

**Files:**
- Modify: `crates/cpu-rv32/src/csr_esp32c3.rs`, `crates/cpu-rv32/src/csr.rs` (doc comment of `VendorCsrs::Esp32C3`)
- Modify: `tools/conformance/riscv-tests-skip.txt`, `tools/conformance/README.md`, `tools/conformance/run-arch-test.sh` (the three notes that say the core has no PMP CSRs; wording only)

**Interfaces:**
- Consumes: `Esp32C3Csrs` (`pub(crate)`, fields private), its `read(&self, addr: u16, cycles: Cycles, instret: u64) -> Option<u32>` and `write(&mut self, addr: u16, value: u32, cycles: Cycles, instret: u64) -> bool`; `CsrFile::new(VendorCsrs::Esp32C3)`, `CsrFile::read(addr, cycles, instret)`, `CsrFile::write(addr, value)`.
- Produces: constants `pub const PMPCFG0: u16 = 0x3A0;`, `pub const PMPADDR0: u16 = 0x3B0;`, `pub const PMP_ENTRIES: usize = 16;`. CSRs `0x3A0..=0x3A3` and `0x3B0..=0x3BF` read and write on a hart built with `VendorCsrs::Esp32C3`; they stay illegal with `VendorCsrs::None` (the conformance harness still excludes `rv32mi-p-pmpaddr`).

Source: the bootloader writes all of them in `esp_cpu_configure_region_protection` and never uses the values (BA §2, paragraph "CSRs used by the bootloader's own code"); the application locks 16 TOR entries (BC §B.2). Lock rule: RISC-V Privileged spec, "Physical Memory Protection CSRs" (a locked entry ignores writes to its configuration and its address; a locked TOR entry also locks the address register below it).

- [ ] **Step 1: Write the failing tests** (append to the `tests` module)

```rust
    fn csrs() -> Esp32C3Csrs {
        Esp32C3Csrs::default()
    }
    const L: u32 = 0x80;
    const TOR: u32 = 0x08;

    #[test]
    fn pmp_registers_read_back_what_was_written() {
        let mut c = csrs();
        for i in 0..16u16 {
            assert_eq!(c.read(PMPADDR0 + i, Cycles(0), 0), Some(0));
            assert!(c.write(PMPADDR0 + i, 0x1000_0000 + u32::from(i), Cycles(0), 0));
            assert_eq!(c.read(PMPADDR0 + i, Cycles(0), 0), Some(0x1000_0000 + u32::from(i)));
        }
        for i in 0..4u16 {
            assert!(c.write(PMPCFG0 + i, 0x0F0B_0907, Cycles(0), 0));
            assert_eq!(c.read(PMPCFG0 + i, Cycles(0), 0), Some(0x0F0B_0907));
        }
        assert_eq!(c.read(0x3A4, Cycles(0), 0), None, "pmpcfg4 does not exist on a 16-entry PMP");
        assert_eq!(c.read(0x3C0, Cycles(0), 0), None, "pmpaddr16 does not exist");
    }

    #[test]
    fn a_locked_entry_ignores_later_writes_to_its_configuration_and_address() {
        let mut c = csrs();
        c.write(PMPADDR0 + 1, 0x111, Cycles(0), 0);
        // Entry 1 is byte 1 of pmpcfg0: lock it with R+W.
        c.write(PMPCFG0, (L | 0x03) << 8, Cycles(0), 0);
        // 0x7F in the three other bytes: every bit but the lock, so entries 0, 2 and 3 stay unlocked.
        c.write(PMPCFG0, 0x7F7F_7F7F, Cycles(0), 0);
        assert_eq!(c.read(PMPCFG0, Cycles(0), 0), Some(0x7F7F_007F | ((L | 0x03) << 8)), "byte 1 kept");
        c.write(PMPADDR0 + 1, 0x222, Cycles(0), 0);
        assert_eq!(c.read(PMPADDR0 + 1, Cycles(0), 0), Some(0x111));
        c.write(PMPADDR0, 0x333, Cycles(0), 0);
        assert_eq!(c.read(PMPADDR0, Cycles(0), 0), Some(0x333), "entry 1 is not TOR: pmpaddr0 stays writable");
    }

    #[test]
    fn a_locked_top_of_range_entry_also_locks_the_address_below_it() {
        let mut c = csrs();
        c.write(PMPADDR0 + 4, 0x444, Cycles(0), 0);
        // Entry 5 is byte 1 of pmpcfg1.
        c.write(PMPCFG0 + 1, (L | TOR | 0x01) << 8, Cycles(0), 0);
        c.write(PMPADDR0 + 4, 0x555, Cycles(0), 0);
        assert_eq!(c.read(PMPADDR0 + 4, Cycles(0), 0), Some(0x444));
    }

    #[test]
    fn pmp_csrs_exist_only_on_the_esp32c3_hart() {
        use crate::csr::{CsrFile, VendorCsrs};
        let mut generic = CsrFile::new(VendorCsrs::None);
        assert_eq!(generic.read(PMPADDR0, Cycles(0), 0), None);
        assert!(!generic.write(PMPCFG0, 0));
        let mut c3 = CsrFile::new(VendorCsrs::Esp32C3);
        assert!(c3.write(PMPADDR0 + 15, 0x2000_0000));
        assert_eq!(c3.read(PMPADDR0 + 15, Cycles(0), 0), Some(0x2000_0000));
    }
```

- [ ] **Step 2: Run, see the failure**

Run: `cargo test -p cpu-rv32 csr_esp32c3 > /tmp/t3.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, "cannot find value `PMPADDR0`".

- [ ] **Step 3: Implement**

Constants and state:

```rust
/// Physical memory protection: 16 entries (ESP-IDF `SOC_CPU_PMP_REGION_GRANULARITY`, `cpu_region_protect.c`;
/// report BC §B.2). The registers are stored and the lock bit is honoured; **the protection is not
/// enforced** (decision D18): enforcement arrives with the application's memory protection in SP1b-3.
pub const PMPCFG0: u16 = 0x3A0;
pub const PMPADDR0: u16 = 0x3B0;
pub const PMP_ENTRIES: usize = 16;
const PMPCFG_REGISTERS: u16 = 4;
const ENTRIES_PER_CFG: usize = 4;
/// `pmpcfg` byte: lock bit, and the address-matching field with its top-of-range value.
const PMP_LOCK: u8 = 0x80;
const PMP_MATCH_MASK: u8 = 0x18;
const PMP_MATCH_TOR: u8 = 0x08;
```

Add to `Esp32C3Csrs` the fields `pmpcfg: [u8; PMP_ENTRIES]` and `pmpaddr: [u32; PMP_ENTRIES]` (both zero by `Default`), and:

```rust
    fn pmp_locked(&self, entry: usize) -> bool {
        self.pmpcfg.get(entry).is_some_and(|cfg| cfg & PMP_LOCK != 0)
    }

    /// A locked entry freezes its own address; a locked top-of-range entry also freezes the address
    /// of the entry below, which is its lower bound.
    fn pmpaddr_locked(&self, entry: usize) -> bool {
        self.pmp_locked(entry)
            || self.pmpcfg.get(entry + 1).is_some_and(|next| {
                next & PMP_LOCK != 0 && next & PMP_MATCH_MASK == PMP_MATCH_TOR
            })
    }

    fn read_pmp(&self, addr: u16) -> Option<u32> {
        if let Some(index) = addr.checked_sub(PMPCFG0).filter(|i| *i < PMPCFG_REGISTERS) {
            let first = usize::from(index) * ENTRIES_PER_CFG;
            let bytes = self.pmpcfg.get(first..first + ENTRIES_PER_CFG)?;
            return Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]));
        }
        let entry = usize::from(addr.checked_sub(PMPADDR0)?);
        self.pmpaddr.get(entry).copied()
    }

    fn write_pmp(&mut self, addr: u16, value: u32) -> bool {
        if let Some(index) = addr.checked_sub(PMPCFG0).filter(|i| *i < PMPCFG_REGISTERS) {
            let first = usize::from(index) * ENTRIES_PER_CFG;
            for (i, byte) in value.to_le_bytes().into_iter().enumerate() {
                if !self.pmp_locked(first + i)
                    && let Some(cfg) = self.pmpcfg.get_mut(first + i)
                {
                    *cfg = byte;
                }
            }
            return true;
        }
        let Some(entry) = addr.checked_sub(PMPADDR0).map(usize::from).filter(|e| *e < PMP_ENTRIES) else {
            return false;
        };
        if !self.pmpaddr_locked(entry) {
            self.pmpaddr[entry] = value;
        }
        true
    }
```

`self.pmpaddr[entry]` is in bounds: `entry < PMP_ENTRIES` was just checked (write `// INVARIANT:` above it). In `read`, replace the fall-through `_ => return None` by `_ => return self.read_pmp(addr)`; in `write`, replace `_ => return false` by `_ => return self.write_pmp(addr, value)`. Update the doc comment of `VendorCsrs::Esp32C3` in `csr.rs` to "`ustatus`, the performance counter and the PMP registers (stored, not enforced)".

- [ ] **Step 4: Reword the three conformance notes that say the core has no PMP CSRs**

After Step 3 they are stale: PMP is **stored on the ESP32-C3 hart and enforced only from SP1b-3, when `pmp32` is re-enabled**; the conformance benches run a generic hart (`VendorCsrs::None`), which still has no PMP CSRs. Nothing but wording changes: no test is added to or removed from a list, and `versions.env` is untouched.

`tools/conformance/riscv-tests-skip.txt`: replace the `rv32mi-p-pmpaddr` line (and only that one) by this single line:

```text
rv32mi-p-pmpaddr # the generic hart this test runs on has no PMP CSRs; the ESP32-C3 hart stores pmpcfg0-3/pmpaddr0-15 since SP1b-2 and enforces them only from SP1b-3, when the arch-test group pmp32 is re-enabled (spec §10.2 item 5)
```

`tools/conformance/README.md`: replace the bullet that starts with `- **Excluded: `pmp32`.**` (four lines) by:

```markdown
- **Excluded: `pmp32`.** RISCOF selects it for any DUT with Zicsr (the tests carry no PMP feature
  check) but the generic hart this bench runs has no PMP CSRs and the ISA YAML does not advertise
  them. The ESP32-C3 hart stores the PMP registers (`pmpcfg0..3`, `pmpaddr0..15`) since SP1b-2 and
  does not enforce them; enforcement arrives in SP1b-3. To re-enable `pmp32` then: add
  `pmp32=<count>` to `ARCH_TEST_EXPECTED` and update `ARCH_TEST_TOTAL` in `versions.env`; the script
  copies exactly the groups listed there.
```

`tools/conformance/run-arch-test.sh`, two places, both inside the single-quoted `bash -c '…'` argument, so the new text must not contain an apostrophe. In the comment, replace the two lines

```bash
  # ISA YAML. pmp32/ needs PMP CSRs, which this core does not have and the ISA YAML does not
  # advertise, yet RISCOF selects it anyway, so it is left out of the suite directory itself.
```

by

```bash
  # ISA YAML. pmp32/ needs enforced PMP. The generic hart this bench runs has no PMP CSRs and the
  # ISA YAML does not advertise them (the ESP32-C3 hart stores them, unenforced, since SP1b-2), yet
  # RISCOF selects pmp32/ anyway, so it is left out of the suite directory itself.
```

and in the `echo "arch-test groups:$groups (…)"` line, replace the parenthesis by

```text
(pmp32 excluded: PMP is stored on the ESP32-C3 hart and not enforced; SP1b-3 enforces it and re-enables pmp32 by adding it back to ARCH_TEST_EXPECTED)
```

- [ ] **Step 5: Run the crate, the conformance tests that read the skip list, and the CLI test that depends on `pmpaddr` being absent from the generic hart**

Run: `cargo test -p cpu-rv32 > /tmp/t3.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.
Run: `cargo test -p emu-cli --test rvtest_cli > /tmp/t3b.log 2>&1; echo "exit=$?"`
Expected: `exit=0` (`inkemu rvtest` builds a `VendorCsrs::None` hart, so `rv32mi-p-pmpaddr` still fails as that test expects).
Run: `cargo test -p emu-cli --test riscv_tests > /tmp/t3c.log 2>&1; echo "exit=$?"`
Expected: `exit=0` (`skip_list_entries_exist_and_have_reasons` reads the reworded line; `rv32mi-p-pmpaddr` is still skipped).
Run: `bash -n tools/conformance/run-arch-test.sh; echo "exit=$?"`
Expected: `exit=0` (the script still parses; it is not run here, it needs Docker and changes no behaviour).
Run: `grep -nE "not implemented in SP1a|SP1a core|which this core does not have" tools/conformance/riscv-tests-skip.txt tools/conformance/README.md tools/conformance/run-arch-test.sh | grep -v rv32mi-p-breakpoint | wc -l`
Expected: `0` (it prints `4` before this step: one line in the skip list, one in the README, two in the script; the `rv32mi-p-breakpoint` line keeps its wording, the trigger CSRs are not part of this task).

- [ ] **Step 6: Commit**

Before committing (Global Constraints): `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`: `exit=0` three times. The code blocks of this plan are not wrapped as `rustfmt` wraps them.

```bash
/usr/bin/git add crates/cpu-rv32/src/csr_esp32c3.rs crates/cpu-rv32/src/csr.rs tools/conformance/riscv-tests-skip.txt tools/conformance/README.md tools/conformance/run-arch-test.sh
/usr/bin/git commit -m "feat(cpu-rv32): PMP registers on the ESP32-C3 hart, stored and lockable, not enforced

The conformance notes now say where PMP stands: stored on the ESP32-C3 hart,
enforced from SP1b-3, when pmp32 is re-enabled.

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/cpu-rv32/src/csr_esp32c3.rs crates/cpu-rv32/src/csr.rs tools/conformance/riscv-tests-skip.txt tools/conformance/README.md tools/conformance/run-arch-test.sh
```

---

### Task 4: The flash chip model and its configuration

**Files:**
- Create: `crates/soc-esp32c3/src/flash.rs`
- Modify: `crates/soc-esp32c3/src/lib.rs` (`pub mod flash;` and re-exports), `crates/soc-esp32c3/src/config.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  ```rust
  // flash.rs
  pub const ERASED: u8 = 0xFF;
  pub const MIN_FLASH_BYTES: u32 = 1 << 20;
  pub const MAX_FLASH_BYTES: u32 = 16 << 20;
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct JedecId { pub manufacturer: u8, pub memory_type: u8, pub capacity: u8 }
  impl JedecId {
      pub const QEMU_ORACLE: JedecId;                       // 9D 60 18
      pub const fn from_u24(id: u32) -> Option<JedecId>;    // 0x9D6018 form
      pub const fn to_u24(self) -> u32;
      pub const fn bytes(self) -> [u8; 3];                  // wire order: manufacturer, type, capacity
      pub const fn capacity_bytes(self) -> Option<u32>;     // 1 << capacity
  }
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct FlashConfig { pub size: u32, pub jedec_id: JedecId }      // Default: 16 MiB, QEMU_ORACLE
  impl FlashConfig { pub fn validate(&self) -> Result<(), FlashImageError>; }
  #[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
  pub enum FlashImageError { TooLong { len: usize, size: u32 }, BadSize { size: u32 } }
  #[derive(Clone, Copy, Debug)]
  pub struct FlashCommand<'a> { pub opcode: u8, pub address: Option<u32>, pub dummy_cycles: u32, pub data_out: &'a [u8] }
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct UnmodeledCommand { pub opcode: u8 }
  pub trait FlashDevice: std::fmt::Debug {
      fn size(&self) -> u32;
      /// Content at `offset`; bytes the image does not hold, or past the chip, read erased.
      fn read(&self, offset: u32, out: &mut [u8]);
      /// One SPI transaction; `data_in` receives the bytes the chip clocks out.
      fn execute(&mut self, command: &FlashCommand<'_>, data_in: &mut [u8]) -> Result<(), UnmodeledCommand>;
  }
  #[derive(Clone)]
  pub struct NorFlash { /* private */ }
  impl NorFlash {
      pub fn blank(config: FlashConfig) -> NorFlash;
      pub fn with_image(image: Vec<u8>, config: FlashConfig) -> Result<NorFlash, FlashImageError>;
  }
  impl FlashDevice for NorFlash { /* … */ }
  // config.rs
  pub const DEFAULT_RNG_SEED: u64 = 0x696E_6B65_6D75_0001;
  // SocConfig gains: pub flash: FlashConfig, pub rng_seed: u64
  ```

- [ ] **Step 1: Write the failing tests** (`flash.rs`, module `tests`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const MIB: usize = 1 << 20;

    fn run(flash: &mut NorFlash, opcode: u8, data_in: &mut [u8]) -> Result<(), UnmodeledCommand> {
        flash.execute(&FlashCommand { opcode, address: None, dummy_cycles: 0, data_out: &[] }, data_in)
    }

    #[test]
    fn a_blank_flash_reads_erased_everywhere_even_past_its_end() {
        let flash = NorFlash::blank(FlashConfig::default());
        assert_eq!(flash.size(), 16 << 20);
        let mut bytes = [0u8; 4];
        flash.read(0, &mut bytes);
        assert_eq!(bytes, [0xFF; 4]);
        flash.read(u32::MAX - 1, &mut bytes);
        assert_eq!(bytes, [0xFF; 4], "no wrap, no panic");
    }

    #[test]
    fn an_image_is_read_back_and_the_rest_of_the_chip_is_erased() {
        let flash = NorFlash::with_image(vec![0xE9, 3, 2, 0x4F], FlashConfig::default()).unwrap();
        let mut bytes = [0u8; 6];
        flash.read(2, &mut bytes);
        assert_eq!(bytes, [2, 0x4F, 0xFF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn an_image_longer_than_the_flash_is_refused_and_one_that_fits_exactly_is_not() {
        let small = FlashConfig { size: MIN_FLASH_BYTES, ..FlashConfig::default() };
        assert!(NorFlash::with_image(vec![0; MIB], small).is_ok());
        let err = NorFlash::with_image(vec![0; MIB + 1], small).unwrap_err();
        assert_eq!(err, FlashImageError::TooLong { len: MIB + 1, size: MIN_FLASH_BYTES });
        let text = err.to_string();
        assert!(text.contains("1048577") && text.contains("1048576") && text.contains("; use"), "{text}");
    }

    #[test]
    fn only_power_of_two_sizes_from_1_to_16_mib_are_valid() {
        let with = |size| FlashConfig { size, ..FlashConfig::default() };
        for size in [1u32 << 20, 2 << 20, 4 << 20, 8 << 20, 16 << 20] {
            assert_eq!(with(size).validate(), Ok(()));
        }
        for size in [0, 512 << 10, 3 << 20, 32 << 20] {
            assert_eq!(with(size).validate(), Err(FlashImageError::BadSize { size }), "{size}");
            assert!(NorFlash::with_image(Vec::new(), with(size)).is_err());
        }
        assert!(FlashImageError::BadSize { size: 0 }.to_string().contains("; correct the flash size"));
    }

    #[test]
    fn jedec_id_converts_between_its_number_and_its_wire_bytes() {
        let id = JedecId::from_u24(0x9D_6018).unwrap();
        assert_eq!(id, JedecId::QEMU_ORACLE);
        assert_eq!(id.bytes(), [0x9D, 0x60, 0x18], "FB §D.1: bytes on the wire");
        assert_eq!(id.to_u24(), 0x9D_6018, "FB §B.6: the form the ROM stores");
        assert_eq!(id.capacity_bytes(), Some(16 << 20));
        assert_eq!(JedecId::from_u24(0x0100_0000), None);
        assert_eq!(JedecId { capacity: 40, ..id }.capacity_bytes(), None);
    }

    #[test]
    fn identification_and_status_commands_answer_like_the_measured_chip() {
        let mut flash = NorFlash::blank(FlashConfig::default());
        let mut id = [0u8; 3];
        run(&mut flash, 0x9F, &mut id).unwrap();
        assert_eq!(id, [0x9D, 0x60, 0x18]);
        let mut status = [0xAAu8];
        run(&mut flash, 0x05, &mut status).unwrap();
        assert_eq!(status, [0], "WIP clear: FB §C row 12");
        run(&mut flash, 0xAB, &mut []).unwrap();
        run(&mut flash, 0x7A, &mut []).unwrap();
    }

    #[test]
    fn write_enable_and_disable_only_move_the_latch_bit() {
        let mut flash = NorFlash::blank(FlashConfig::default());
        let mut status = [0u8];
        run(&mut flash, 0x06, &mut []).unwrap();
        run(&mut flash, 0x05, &mut status).unwrap();
        assert_eq!(status, [0x02]);
        run(&mut flash, 0x04, &mut []).unwrap();
        run(&mut flash, 0x05, &mut status).unwrap();
        assert_eq!(status, [0]);
    }

    #[test]
    fn an_sfdp_read_answers_erased_bytes() {
        let mut flash = NorFlash::blank(FlashConfig::default());
        let mut byte = [0u8];
        let command = FlashCommand { opcode: 0x5A, address: Some(0x10), dummy_cycles: 8, data_out: &[] };
        flash.execute(&command, &mut byte).unwrap();
        assert_eq!(byte, [0xFF], "anything but 0x20 (XMC): BA §2.2");
    }

    #[test]
    fn commands_that_would_change_or_read_the_content_are_not_modeled() {
        let mut flash = NorFlash::with_image(vec![1, 2, 3, 4], FlashConfig::default()).unwrap();
        for opcode in [0x02, 0x20, 0xD8, 0xC7, 0x01, 0x31, 0x35, 0x03, 0xBB, 0x00, 0xFF] {
            assert_eq!(run(&mut flash, opcode, &mut [0u8; 4]), Err(UnmodeledCommand { opcode }));
        }
        let mut bytes = [0u8; 4];
        flash.read(0, &mut bytes);
        assert_eq!(bytes, [1, 2, 3, 4], "the content did not change");
    }

    #[test]
    fn debug_output_does_not_dump_the_image() {
        let flash = NorFlash::with_image(vec![0xAB; 4096], FlashConfig::default()).unwrap();
        assert!(format!("{flash:?}").len() < 200);
    }
}
```

In `config.rs`, extend `default_boots_from_spi_flash_on_revision_0_3_without_strictness`:

```rust
        assert_eq!(config.flash, crate::flash::FlashConfig::default());
        assert_eq!(config.rng_seed, DEFAULT_RNG_SEED);
        assert_ne!(DEFAULT_RNG_SEED, 0);
```

- [ ] **Step 2: Run, see the failure**

Run: `cargo test -p soc-esp32c3 flash > /tmp/t4.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, unresolved module `flash`.

- [ ] **Step 3: Implement `flash.rs`**

```rust
//! The SPI NOR flash chip behind the MSPI controllers. Read-only in this plan (decision D3):
//! the commands that would change its content are not modelled and are reported by their opcode.
use std::fmt;

/// An erased NOR flash byte.
pub const ERASED: u8 = 0xFF;
pub const MIN_FLASH_BYTES: u32 = 1 << 20;
/// The MMU names flash pages of 64 KiB with 8 bits (report CM §1): 16 MiB at most.
pub const MAX_FLASH_BYTES: u32 = 16 << 20;

/// Release from power-down: the first command of the ROM (`SPI_WakeUp`, FB §B.4).
const OP_RES: u8 = 0xAB;
/// Read JEDEC id (FB §D.1).
const OP_RDID: u8 = 0x9F;
/// Read status register 1 (FB §D.1).
const OP_RDSR: u8 = 0x05;
/// Read SFDP; the bootloader reads one byte at address 0x10 (BA §2.2, SPI1 row).
const OP_SFDP: u8 = 0x5A;
/// Program/erase resume, sent once by the bootloader (BA §2.2, SPI1 row).
const OP_RESUME: u8 = 0x7A;
/// Write enable and write disable (ESP-IDF `spi_flash_defs.h`; BC §D.2 item 6).
const OP_WREN: u8 = 0x06;
const OP_WRDI: u8 = 0x04;
/// Status register 1: bit 0 write-in-progress (always clear: nothing ever writes), bit 1 write-enable latch.
const STATUS_WEL: u8 = 1 << 1;
/// Largest exponent `1 << capacity` fits a `u32`.
const MAX_CAPACITY_EXPONENT: u8 = 31;

/// The three bytes a flash chip answers to a read-JEDEC-id command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JedecId {
    pub manufacturer: u8,
    pub memory_type: u8,
    /// `log2` of the capacity in bytes.
    pub capacity: u8,
}

impl JedecId {
    /// The id the flash of Espressif's QEMU answers, bytes `9D 60 18` (FB §B.6, §D.1). Decision D2:
    /// the 2nd-stage bootloader then takes the vendor branch the oracle was measured on.
    pub const QEMU_ORACLE: JedecId = JedecId { manufacturer: 0x9D, memory_type: 0x60, capacity: 0x18 };

    /// From the 24-bit form `manufacturer << 16 | type << 8 | capacity` (the one the ROM stores).
    #[must_use]
    pub const fn from_u24(id: u32) -> Option<JedecId> {
        let [top, manufacturer, memory_type, capacity] = id.to_be_bytes();
        if top != 0 {
            return None;
        }
        Some(JedecId { manufacturer, memory_type, capacity })
    }

    #[must_use]
    pub const fn to_u24(self) -> u32 {
        u32::from_be_bytes([0, self.manufacturer, self.memory_type, self.capacity])
    }

    /// The bytes in the order the chip sends them.
    #[must_use]
    pub const fn bytes(self) -> [u8; 3] {
        [self.manufacturer, self.memory_type, self.capacity]
    }

    /// The size the capacity byte stands for.
    #[must_use]
    pub const fn capacity_bytes(self) -> Option<u32> {
        if self.capacity > MAX_CAPACITY_EXPONENT {
            return None;
        }
        Some(1 << self.capacity)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlashConfig {
    /// Size of the chip in bytes.
    pub size: u32,
    pub jedec_id: JedecId,
}

impl Default for FlashConfig {
    /// 16 MiB (spec §2: every Xteink device) with the id of the QEMU oracle.
    fn default() -> FlashConfig {
        FlashConfig { size: MAX_FLASH_BYTES, jedec_id: JedecId::QEMU_ORACLE }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FlashImageError {
    #[error(
        "the flash image is {len} bytes, longer than the {size} byte flash of the device; use an image built for this flash size, or a device whose flash is large enough"
    )]
    TooLong { len: usize, size: u32 },
    #[error(
        "a flash of {size} bytes cannot be modelled: the size must be a power of two from 1 MiB to 16 MiB; correct the flash size of the device profile"
    )]
    BadSize { size: u32 },
}

impl FlashConfig {
    /// # Errors
    /// [`FlashImageError::BadSize`] when the size is not a power of two from 1 MiB to 16 MiB.
    pub fn validate(&self) -> Result<(), FlashImageError> {
        let valid = self.size.is_power_of_two() && (MIN_FLASH_BYTES..=MAX_FLASH_BYTES).contains(&self.size);
        if valid { Ok(()) } else { Err(FlashImageError::BadSize { size: self.size }) }
    }
}

/// One SPI transaction as the chip sees it.
#[derive(Clone, Copy, Debug)]
pub struct FlashCommand<'a> {
    pub opcode: u8,
    pub address: Option<u32>,
    pub dummy_cycles: u32,
    /// Bytes the controller sends after the address and the dummy cycles.
    pub data_out: &'a [u8],
}

/// The chip model does not implement this opcode: nothing happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnmodeledCommand {
    pub opcode: u8,
}

/// The flash chip as its two users see it: the cache (content) and the SPI1 controller (commands).
pub trait FlashDevice: fmt::Debug {
    fn size(&self) -> u32;
    /// Content at `offset`; bytes the image does not hold, or past the chip, read erased.
    fn read(&self, offset: u32, out: &mut [u8]);
    /// Runs one transaction; `data_in` receives the bytes the chip clocks out.
    /// # Errors
    /// [`UnmodeledCommand`] when the opcode is not implemented; `data_in` is then untouched.
    fn execute(&mut self, command: &FlashCommand<'_>, data_in: &mut [u8]) -> Result<(), UnmodeledCommand>;
}

/// A NOR flash holding the image it was given. The image is kept as supplied: what lies past it is
/// erased, so a blank chip costs no memory (decision D19).
#[derive(Clone)]
pub struct NorFlash {
    image: Vec<u8>,
    config: FlashConfig,
    write_enabled: bool,
}

impl fmt::Debug for NorFlash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NorFlash")
            .field("image_bytes", &self.image.len())
            .field("config", &self.config)
            .field("write_enabled", &self.write_enabled)
            .finish()
    }
}

impl NorFlash {
    /// An erased chip.
    #[must_use]
    pub const fn blank(config: FlashConfig) -> NorFlash {
        NorFlash { image: Vec::new(), config, write_enabled: false }
    }

    /// # Errors
    /// [`FlashImageError::BadSize`] for a size the chip cannot have, [`FlashImageError::TooLong`]
    /// when the image does not fit.
    pub fn with_image(image: Vec<u8>, config: FlashConfig) -> Result<NorFlash, FlashImageError> {
        config.validate()?;
        if u64::try_from(image.len()).unwrap_or(u64::MAX) > u64::from(config.size) {
            return Err(FlashImageError::TooLong { len: image.len(), size: config.size });
        }
        Ok(NorFlash { image, config, write_enabled: false })
    }
}

impl FlashDevice for NorFlash {
    fn size(&self) -> u32 {
        self.config.size
    }

    fn read(&self, offset: u32, out: &mut [u8]) {
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = start
                .checked_add(i)
                .and_then(|index| self.image.get(index))
                .copied()
                .unwrap_or(ERASED);
        }
    }

    fn execute(&mut self, command: &FlashCommand<'_>, data_in: &mut [u8]) -> Result<(), UnmodeledCommand> {
        match command.opcode {
            // Nothing to wake up or resume: the chip is never powered down or suspended.
            OP_RES | OP_RESUME => {}
            OP_RDID => {
                // The id repeats for as long as the controller keeps clocking.
                for (byte, id) in data_in.iter_mut().zip(self.config.jedec_id.bytes().into_iter().cycle()) {
                    *byte = id;
                }
            }
            OP_RDSR => data_in.fill(if self.write_enabled { STATUS_WEL } else { 0 }),
            // ASSUMPTION: the modelled part exposes no SFDP table and answers erased bytes. The
            // bootloader only checks that byte 0x10 is not the XMC manufacturer id (BA §2.2). What
            // the QEMU oracle answers is not recorded: the SP1b-4 lockstep settles it.
            OP_SFDP => data_in.fill(ERASED),
            OP_WREN => self.write_enabled = true,
            OP_WRDI => self.write_enabled = false,
            opcode => return Err(UnmodeledCommand { opcode }),
        }
        Ok(())
    }
}
```

In `lib.rs`: `pub mod flash;` and `pub use flash::{FlashConfig, FlashDevice, FlashImageError, JedecId, NorFlash};`. Update the crate doc comment to mention "the flash chip ([`flash`])".

In `config.rs`: add `use crate::flash::FlashConfig;`, the constant

```rust
/// Seed of the hardware random number generator model (decision D8): fixed, so two runs are identical.
/// The bytes spell "inkemu"; any non-zero value would do.
pub const DEFAULT_RNG_SEED: u64 = 0x696E_6B65_6D75_0001;
```

the fields `/// The SPI flash chip.` `pub flash: FlashConfig,` and `/// Seed of the random number generator (`APB_CTRL.RND_DATA`).` `pub rng_seed: u64,`, and their defaults `flash: FlashConfig::default(), rng_seed: DEFAULT_RNG_SEED`.

- [ ] **Step 4: Run**

Run: `cargo test --workspace > /tmp/t4.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 5: Commit**

Before committing (Global Constraints): `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`: `exit=0` three times. The code blocks of this plan are not wrapped as `rustfmt` wraps them.

```bash
/usr/bin/git add crates/soc-esp32c3/src/flash.rs crates/soc-esp32c3/src/lib.rs crates/soc-esp32c3/src/config.rs
/usr/bin/git commit -m "feat(soc): read-only NOR flash model behind a FlashDevice trait

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3/src/flash.rs crates/soc-esp32c3/src/lib.rs crates/soc-esp32c3/src/config.rs
```

---

### Task 5: SPI1 flash controller, SPI0 as storage, and a machine that carries a flash

**Files:**
- Create: `crates/soc-esp32c3/src/periph/spi_mem.rs`
- Modify: `crates/soc-esp32c3/src/periph/mod.rs`, `crates/soc-esp32c3/src/bus.rs`, `crates/soc-esp32c3/src/machine.rs`
- Modify: `docs/registers-accepted-without-effect.md` (regenerated table)

**Interfaces:**
- Consumes: `FlashDevice`, `FlashCommand`, `UnmodeledCommand`, `NorFlash`, `FlashImageError`, `SocConfig::flash` (Task 4); `Accepted`, `Storage::for_offsets`, `Storage::accepted`, `Unsupported`, `Diagnostic::Unsupported` (Task 2); `svd_regs::esp32c3::addr` (Task 1).
- Produces:
  ```rust
  // periph/spi_mem.rs
  pub struct Spi0 { /* private */ }            impl Spi0 { pub fn new() -> Spi0 }
  pub struct Spi1 { /* private */ }
  impl Spi1 {
      pub fn new(flash: Box<dyn FlashDevice>) -> Spi1;
      pub fn flash(&self) -> &dyn FlashDevice;
      pub(crate) fn take_unsupported(&mut self) -> Option<Unsupported>;
  }
  // periph/mod.rs
  pub(crate) const SPI1_BASE: u32 = 0x6000_2000;   pub(crate) const SPI0_BASE: u32 = 0x6000_3000;
  impl Peripherals {
      pub fn new(config: &SocConfig, flash: Box<dyn FlashDevice>) -> Peripherals;   // was new(config)
      pub(crate) fn take_unsupported(&mut self) -> Option<Unsupported>;
  }
  // new fields: pub spi0: Spi0, pub spi1: Spi1
  // bus.rs
  impl SocBus { pub fn new(rom: RomImage, flash: Box<dyn FlashDevice>, config: &SocConfig) -> SocBus; }  // was new(rom, config)
  // machine.rs
  impl Esp32C3 {
      pub fn new(rom: RomImage, config: &SocConfig) -> Esp32C3;                       // unchanged: blank flash
      pub fn with_flash(rom: RomImage, image: Vec<u8>, config: &SocConfig) -> Result<Esp32C3, FlashImageError>;
  }
  ```

Register facts used below: `CMD` bits (BC §D.1): b31 `FLASH_READ`, b30 `WREN`, b29 `WRDI`, b28 `RDID`, b27 `RDSR`, b26 `WRSR`, b25 `PP`, b24 `SE`, b23 `BE`, b22 `CE`, b21 `DP`, b20 `RES`, b19 `HPM`, b18 `USR`, b17 `PE`, b1 `PES`, b0 `PER`; every command bit self-clears and the ROM waits for the whole register to read 0 (FB §C row 9, §D.1). `USER`: b31 `USR_COMMAND`, b30 `USR_ADDR`, b29 `USR_DUMMY`, b28 `USR_MISO`, b27 `USR_MOSI` (FB §B.2). `USER1`: [31:26] address bit length − 1, [5:0] dummy cycles − 1 (FB §B.2, §B.6). `USER2`: [31:28] command bit length − 1, [15:0] command value (FB §B.2). `MOSI_DLEN`/`MISO_DLEN`: [9:0] bit length − 1 (FB §B.2). Reset values (SVD; FB §D.2): `CTRL 0x002CA000`, `CTRL1 0xFFC`, `CLOCK 0x30103`, `USER 0x80000000`, `USER1 0x5C000007`, `USER2 0x70000000`.

- [ ] **Step 1: Write the failing tests** (`spi_mem.rs`, module `tests`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::flash::{FlashConfig, NorFlash};

    /// `USER` bit 31: the transfer has a command phase (every measured one does, so the model never
    /// tests the bit and the constant lives with the tests that build `USER` values).
    const USR_COMMAND: u32 = 1 << 31;

    fn spi1() -> Spi1 {
        Spi1::new(Box::new(NorFlash::blank(FlashConfig::default())))
    }
    /// A `USR` transfer of `opcode` reading `miso_bits` bits, as `bootloader_flash_execute_command_common` sets it up.
    fn user_read(spi: &mut Spi1, opcode: u32, miso_bits: u32) {
        spi.write(USER, USR_COMMAND | USR_MISO);
        spi.write(USER2, 0x7000_0000 | opcode);
        spi.write(MISO_DLEN, miso_bits - 1);
        spi.write(W0, 0);
        spi.write(CMD, CMD_USR);
    }

    #[test]
    fn configuration_registers_start_at_their_svd_reset_values() {
        let mut spi = spi1();
        for (register, reset) in [
            ("CTRL", 0x002C_A000),
            ("CTRL1", 0x0000_0FFC),
            ("CLOCK", 0x0003_0103),
            ("USER", 0x8000_0000),
            ("USER1", 0x5C00_0007),
            ("USER2", 0x7000_0000),
        ] {
            let offset = svd_regs::esp32c3::addr("SPI1", register) - SPI1_BASE;
            assert_eq!(spi.read(offset), Some(reset), "{register}");
        }
        assert_eq!(spi.read(CMD), Some(0));
    }

    #[test]
    fn rom_read_id_puts_the_jedec_bytes_in_w0_and_the_command_self_clears() {
        // FB §B.6: `esp_rom_spi_flash_update_id` writes W0 = 0, CMD = 0x10000000, polls CMD, reads W0.
        let mut spi = spi1();
        spi.write(W0, 0);
        assert!(spi.write(CMD, 0x1000_0000));
        assert_eq!(spi.read(CMD), Some(0), "the ROM waits for the whole register to read 0");
        assert_eq!(spi.read(W0), Some(0x0018_609D));
        assert_eq!(spi.take_unsupported(), None);
    }

    #[test]
    fn rom_read_status_puts_the_status_byte_in_rd_status() {
        // FB §B.6: RD_STATUS[15:0] is zeroed, CMD = 0x08000000, then bit 0 (WIP) must read 0.
        let mut spi = spi1();
        spi.write(RD_STATUS, 0xABCD_0000);
        spi.write(CMD, 0x0800_0000);
        assert_eq!(spi.read(RD_STATUS), Some(0xABCD_0000), "status 0, upper half kept");
        spi.write(CMD, 0x4000_0000); // WREN
        spi.write(CMD, 0x0800_0000);
        assert_eq!(spi.read(RD_STATUS), Some(0xABCD_0002));
        spi.write(CMD, 0x2000_0000); // WRDI
        spi.write(CMD, 0x0800_0000);
        assert_eq!(spi.read(RD_STATUS), Some(0xABCD_0000));
    }

    #[test]
    fn rom_wake_up_runs_without_touching_the_data_registers() {
        // FB §B.4: `SPI_WakeUp` writes CMD = 0x00100000 (FLASH_RES) and polls CMD.
        let mut spi = spi1();
        spi.write(W0, 0x1234_5678);
        spi.write(CMD, 0x0010_0000);
        assert_eq!(spi.read(CMD), Some(0));
        assert_eq!(spi.read(W0), Some(0x1234_5678));
        assert_eq!(spi.take_unsupported(), None);
    }

    #[test]
    fn bootloader_user_commands_run_through_the_generic_path() {
        // BA §2.2, SPI1 row: RDID 0x9F (24 bits in), SFDP 0x5A at 0x10 with 8 dummy bits (8 bits in),
        // RESUME 0x7A, RDSR 0x05 (8 bits in), WRDI 0x04.
        let mut spi = spi1();
        user_read(&mut spi, 0x9F, 24);
        assert_eq!(spi.read(W0), Some(0x0018_609D), "BA §2 row 46");
        spi.write(USER, USR_COMMAND | USR_ADDR | USR_DUMMY | USR_MISO);
        spi.write(USER1, (23 << 26) | 7);
        spi.write(USER2, 0x7000_005A);
        spi.write(ADDR, 0x10);
        spi.write(MISO_DLEN, 7);
        spi.write(CMD, CMD_USR);
        assert_eq!(spi.read(W0), Some(0xFF));
        for opcode in [0x7A, 0x04] {
            spi.write(USER, USR_COMMAND);
            spi.write(USER2, 0x7000_0000 | opcode);
            spi.write(CMD, CMD_USR);
        }
        user_read(&mut spi, 0x05, 8);
        assert_eq!(spi.read(W0), Some(0));
        assert_eq!(spi.read(CMD), Some(0));
        assert_eq!(spi.take_unsupported(), None);
    }

    #[test]
    fn program_and_erase_are_reported_by_opcode() {
        let mut spi = spi1();
        for (command_bit, opcode) in [
            (1u32 << 25, 0x02u8), // PP
            (1 << 24, 0x20),      // SE
            (1 << 23, 0xD8),      // BE
            (1 << 22, 0xC7),      // CE
            (1 << 26, 0x01),      // WRSR
            (1 << 31, 0x03),      // FLASH_READ
        ] {
            assert!(spi.write(CMD, command_bit), "the write is handled: it is reported, not dropped");
            assert_eq!(spi.take_unsupported(), Some(Unsupported::FlashCommand { opcode }));
            assert_eq!(spi.take_unsupported(), None, "taken once");
            assert_eq!(spi.read(CMD), Some(0), "no hang on an unmodelled command");
        }
    }

    #[test]
    fn unknown_user_opcode_is_reported() {
        let mut spi = spi1();
        spi.write(W0, 0xDEAD_BEEF);
        spi.write(USER, USR_COMMAND | USR_ADDR | USR_MOSI);
        spi.write(USER2, 0x7000_0002); // page program
        spi.write(MOSI_DLEN, 31);
        spi.write(CMD, CMD_USR);
        assert_eq!(spi.take_unsupported(), Some(Unsupported::FlashCommand { opcode: 0x02 }));
        assert_eq!(spi.read(W0), Some(0xDEAD_BEEF), "nothing was clocked in");
    }

    #[test]
    fn transfer_lengths_are_bounded_by_the_sixteen_data_registers() {
        // MISO_DLEN can ask for 1024 bits (128 bytes); the data buffer is W0..W15, 64 bytes.
        let mut spi = spi1();
        user_read(&mut spi, 0x9F, 1024);
        let w15 = svd_regs::esp32c3::addr("SPI1", "W15") - SPI1_BASE;
        // 64 bytes of the repeating id 9D 60 18: byte 60 is 9D (60 % 3 == 0).
        assert_eq!(spi.read(w15), Some(0x9D18_609D));
        assert_eq!(spi.read(w15 + 4), None, "FLASH_WAITI_CTRL is not modelled: nothing was stored past W15");
    }

    #[test]
    fn a_read_shorter_than_a_word_zeroes_the_rest_of_that_word() {
        let mut spi = spi1();
        spi.write(W0, 0xFFFF_FFFF);
        user_read(&mut spi, 0x9F, 16);
        assert_eq!(spi.read(W0), Some(0x0000_609D));
    }

    #[test]
    fn spi0_stores_the_read_mode_the_rom_programs_and_reads_idle() {
        // FB §B.6: SPI0.CTRL ends at 0x00AC2008; the bootloader decodes it to print "SPI Mode : DIO".
        let mut spi0 = Spi0::new();
        let ctrl = svd_regs::esp32c3::addr("SPI0", "CTRL") - SPI0_BASE;
        assert_eq!(spi0.read(ctrl), Some(0x002C_2000));
        assert!(spi0.write(ctrl, 0x00AC_2008));
        assert_eq!(spi0.read(ctrl), Some(0x00AC_2008));
        let fsm = svd_regs::esp32c3::addr("SPI0", "FSM") - SPI0_BASE;
        assert_eq!(spi0.read(fsm).map(|v| v & 0x70), Some(0), "Wait_SPI_Idle: bits 6:4 (FB §C row 10)");
        assert!(!spi0.write(fsm, 0), "FSM is read-only");
        for offset in [SPI0_MOSI_DLEN - SPI0_BASE, SPI0_MISO_DLEN - SPI0_BASE] {
            assert_eq!(spi0.read(offset), Some(0));
            assert!(spi0.write(offset, 0xFF), "FB §B.2: 256 bits, one cache line");
            assert_eq!(spi0.read(offset), Some(0xFF));
        }
    }
}
```

Bus and machine tests. In `bus.rs` (module `tests`), add `use crate::flash::NorFlash;`, change `bus_with` to pass a flash (`SocBus::new(RomImage::from_flat(rom).unwrap(), Box::new(NorFlash::blank(config.flash)), &config)`, same for the one other `SocBus::new` of that module, in `the_last_word_of_the_rom_reads_and_one_straddling_its_end_is_unmapped`, with `SocConfig::default().flash`) and add:

```rust
    #[test]
    fn an_unmodelled_flash_command_becomes_a_named_diagnostic_and_a_strict_violation() {
        let cmd = svd_regs::esp32c3::addr("SPI1", "CMD");
        let mut b = bus_with(SocConfig { strict_mmio: true, ..SocConfig::default() });
        b.set_pc(0x4038_0010);
        b.write(cmd, Word, 1 << 24).unwrap(); // sector erase
        let expected = Diagnostic::Unsupported {
            what: crate::diag::Unsupported::FlashCommand { opcode: 0x20 },
            pc: 0x4038_0010,
        };
        assert_eq!(b.diag.drain(), vec![expected.clone()]);
        assert_eq!(b.diag.take_violation(), Some(expected));
        assert_eq!(load(&mut b, cmd), 0);
    }
```

In `machine.rs` (module `tests`):

```rust
    #[test]
    fn a_machine_can_be_built_around_a_flash_image_and_refuses_one_that_is_too_long() {
        let config = SocConfig::default();
        assert!(Esp32C3::with_flash(flat_rom(&[HANG]), vec![0xE9; 64], &config).is_ok());
        let too_long = vec![0u8; config.flash.size as usize + 1];
        let err = Esp32C3::with_flash(flat_rom(&[HANG]), too_long, &config).unwrap_err();
        assert!(matches!(err, crate::flash::FlashImageError::TooLong { .. }), "{err}");
    }
```

- [ ] **Step 2: Run, see the failure**

Run: `cargo test -p soc-esp32c3 > /tmp/t5.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, unresolved `spi_mem`.

- [ ] **Step 3: Implement `periph/spi_mem.rs`**

```rust
//! SPI0 and SPI1, the two controllers that share the flash chip (`spi_mem_reg.h`; reports BC §D.1,
//! FB §B and §D). SPI1 runs the commands software asks for; SPI0 refills the cache, which this
//! emulator does not model (the bus reads the flash directly), so SPI0 is configuration storage.
use super::{Accepted, SPI0_BASE, SPI1_BASE, Storage};
use crate::diag::Unsupported;
use crate::flash::{FlashCommand, FlashDevice};
use emu_core::Peripheral;
use svd_regs::esp32c3::addr;

const CMD: u32 = addr("SPI1", "CMD") - SPI1_BASE;
const ADDR: u32 = addr("SPI1", "ADDR") - SPI1_BASE;
const USER: u32 = addr("SPI1", "USER") - SPI1_BASE;
const USER1: u32 = addr("SPI1", "USER1") - SPI1_BASE;
const USER2: u32 = addr("SPI1", "USER2") - SPI1_BASE;
const MOSI_DLEN: u32 = addr("SPI1", "MOSI_DLEN") - SPI1_BASE;
const MISO_DLEN: u32 = addr("SPI1", "MISO_DLEN") - SPI1_BASE;
const RD_STATUS: u32 = addr("SPI1", "RD_STATUS") - SPI1_BASE;
const W0: u32 = addr("SPI1", "W0") - SPI1_BASE;
/// `W0..W15`: the 64-byte data buffer of a transfer.
const DATA_WORDS: u32 = 16;
const DATA_BYTES: usize = 64;
const WORD_BYTES: usize = 4;

/// `CMD` bit 18: run the transfer described by `USER`, `USER1`, `USER2`, `ADDR` and the lengths.
const CMD_USR: u32 = 1 << 18;
/// `CMD` bit 17, `FLASH_PE`: a user-defined program/erase; its opcode is the one in `USER2`.
const CMD_PE: u32 = 1 << 17;
const CMD_RDID: u32 = 1 << 28;
const CMD_RDSR: u32 = 1 << 27;
/// The hard-wired commands: `CMD` bit, the opcode the controller sends (ESP-IDF `spi_flash_defs.h`,
/// report BC §D.2 item 6; `0xB9` deep power-down is the JEDEC opcode, it is not in the report).
const HARD_WIRED: [(u32, u8); 15] = [
    (1 << 31, 0x03), // FLASH_READ
    (1 << 30, 0x06), // WREN
    (1 << 29, 0x04), // WRDI
    (CMD_RDID, 0x9F),
    (CMD_RDSR, 0x05),
    (1 << 26, 0x01), // WRSR
    (1 << 25, 0x02), // PP
    (1 << 24, 0x20), // SE
    (1 << 23, 0xD8), // BE
    (1 << 22, 0xC7), // CE
    (1 << 21, 0xB9), // DP
    (1 << 20, 0xAB), // RES
    (1 << 19, 0xA3), // HPM
    (1 << 1, 0x75),  // PES
    (1 << 0, 0x7A),  // PER
];
const JEDEC_ID_BYTES: usize = 3;
const STATUS_BYTE_MASK: u32 = 0xFF;

const USR_ADDR: u32 = 1 << 30;
const USR_DUMMY: u32 = 1 << 29;
const USR_MISO: u32 = 1 << 28;
const USR_MOSI: u32 = 1 << 27;
/// `USER1` [31:26]: address length in bits, minus one. [5:0]: dummy cycles, minus one.
const ADDR_BITLEN_SHIFT: u32 = 26;
const ADDR_BITLEN_MASK: u32 = 0x3F;
const DUMMY_CYCLES_MASK: u32 = 0x3F;
/// `USER2` [15:0]: the command; its low byte is the opcode of every 8-bit command.
const OPCODE_MASK: u32 = 0xFF;
/// `MOSI_DLEN` / `MISO_DLEN` [9:0]: data length in bits, minus one.
const DLEN_MASK: u32 = 0x3FF;

/// Configuration of SPI1 that only shapes the electrical transfer, which is instantaneous here.
const SPI1_ACCEPTED: [Accepted; 6] = [
    Accepted::svd("SPI1", "CTRL", "read mode, dummy output, WP level", "SP8"),
    Accepted::svd("SPI1", "CTRL1", "CS hold delay after a release-from-power-down", "SP8"),
    Accepted::svd("SPI1", "CTRL2", "CS setup and hold times", "SP8"),
    Accepted::svd("SPI1", "CLOCK", "SPI clock divider: a transfer takes no time", "SP8"),
    Accepted::svd("SPI1", "SUS_STATUS", "program/erase suspend state", "SP1b-3"),
    Accepted::svd("SPI1", "CLOCK_GATE", "register clock gate", "SP1b-3"),
];

/// SPI0 `+0x24` and `+0x28`: `MOSI_DLEN` and `MISO_DLEN`. The SVD omits them for SPI0; ESP-IDF
/// `spi_mem_reg.h:451,459` defines them for both controllers and the ROM writes them (FB §B.2).
const SPI0_MOSI_DLEN: u32 = SPI0_BASE + 0x24;
const SPI0_MISO_DLEN: u32 = SPI0_BASE + 0x28;
const SPI0_FSM: u32 = addr("SPI0", "FSM") - SPI0_BASE;
const WHY_SPI0: &str = "cache-fill transfer (read command, dummy cycles, clock): the bus reads the flash directly";
const SPI0_ACCEPTED: [Accepted; 11] = [
    Accepted::svd("SPI0", "CTRL", WHY_SPI0, "SP1b-4"),
    Accepted::svd("SPI0", "CTRL2", WHY_SPI0, "SP1b-4"),
    Accepted::svd("SPI0", "CLOCK", WHY_SPI0, "SP1b-4"),
    Accepted::svd("SPI0", "USER", WHY_SPI0, "SP1b-4"),
    Accepted::svd("SPI0", "USER1", WHY_SPI0, "SP1b-4"),
    Accepted::svd("SPI0", "USER2", WHY_SPI0, "SP1b-4"),
    Accepted::at("SPI0", "MOSI_DLEN", SPI0_MOSI_DLEN, WHY_SPI0, "SP1b-4"),
    Accepted::at("SPI0", "MISO_DLEN", SPI0_MISO_DLEN, WHY_SPI0, "SP1b-4"),
    Accepted::svd("SPI0", "MISC", "chip-select enables", "SP1b-4"),
    Accepted::svd("SPI0", "CACHE_FCTRL", "cache request enable", "SP1b-4"),
    Accepted::svd("SPI0", "CLOCK_GATE", "register clock gate", "SP1b-3"),
];

pub(crate) fn accepted() -> Vec<Accepted> {
    [&SPI1_ACCEPTED[..], &SPI0_ACCEPTED[..]].concat()
}

#[derive(Clone, Debug)]
pub struct Spi0 {
    config: Storage,
    fsm_idle: u32,
}

impl Spi0 {
    #[must_use]
    pub fn new() -> Spi0 {
        Spi0 {
            config: Storage::accepted(SPI0_BASE, &SPI0_ACCEPTED),
            // The reset value has the state bits [6:4] clear: idle, what `Wait_SPI_Idle` waits for.
            fsm_idle: svd_regs::esp32c3::find("SPI0", "FSM").map_or(0, |r| r.reset_value),
        }
    }
}

impl Default for Spi0 {
    fn default() -> Spi0 {
        Spi0::new()
    }
}

impl Peripheral for Spi0 {
    fn read(&mut self, offset: u32) -> Option<u32> {
        if offset == SPI0_FSM {
            return Some(self.fsm_idle);
        }
        self.config.read(offset)
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        self.config.write(offset, value)
    }
}

#[derive(Debug)]
pub struct Spi1 {
    registers: Storage,
    flash: Box<dyn FlashDevice>,
    unsupported: Option<Unsupported>,
}

impl Spi1 {
    #[must_use]
    pub fn new(flash: Box<dyn FlashDevice>) -> Spi1 {
        let mut offsets: Vec<u32> = SPI1_ACCEPTED.iter().map(|a| a.addr - SPI1_BASE).collect();
        offsets.extend([ADDR, USER, USER1, USER2, MOSI_DLEN, MISO_DLEN, RD_STATUS]);
        offsets.extend((0..DATA_WORDS).map(|word| W0 + 4 * word));
        Spi1 { registers: Storage::for_offsets(SPI1_BASE, &offsets), flash, unsupported: None }
    }

    /// The chip, for the cache windows of the bus.
    #[must_use]
    pub fn flash(&self) -> &dyn FlashDevice {
        self.flash.as_ref()
    }

    /// The command the flash model refused since the last call, if any.
    pub(crate) fn take_unsupported(&mut self) -> Option<Unsupported> {
        self.unsupported.take()
    }

    fn register(&self, offset: u32) -> u32 {
        self.registers.read(offset).unwrap_or(0)
    }

    /// Runs `command`; `Some(bytes)` are the `data_in_len` bytes read, `None` when the chip model
    /// does not implement the opcode (the refusal is kept for the bus to report).
    fn transfer(&mut self, command: &FlashCommand<'_>, data_in_len: usize) -> Option<Vec<u8>> {
        let mut data_in = vec![0u8; data_in_len.min(DATA_BYTES)];
        match self.flash.execute(command, &mut data_in) {
            Ok(()) => Some(data_in),
            Err(refused) => {
                self.unsupported = Some(Unsupported::FlashCommand { opcode: refused.opcode });
                None
            }
        }
    }

    /// The low byte of the command in `USER2`.
    fn user_opcode(&self) -> u8 {
        (self.register(USER2) & OPCODE_MASK).to_le_bytes()[0]
    }

    fn run_hard_wired(&mut self, command_bit: u32, opcode: u8) {
        let command = FlashCommand { opcode, address: None, dummy_cycles: 0, data_out: &[] };
        match command_bit {
            CMD_RDID => {
                if let Some(id) = self.transfer(&command, JEDEC_ID_BYTES) {
                    // ASSUMPTION: W0[31:24] reads 0 after a read-id; the ROM masks it (FB §L item 7).
                    self.store_data_in(&id);
                }
            }
            CMD_RDSR => {
                if let Some(status) = self.transfer(&command, 1) {
                    // ASSUMPTION: only RD_STATUS[7:0] is replaced (FB §D.1, §L item 7).
                    let low = status.first().copied().map_or(0, u32::from);
                    let kept = self.register(RD_STATUS) & !STATUS_BYTE_MASK;
                    self.registers.write(RD_STATUS, kept | low);
                }
            }
            _ => {
                let _ = self.transfer(&command, 0);
            }
        }
    }

    /// Bytes a `xxx_DLEN` register asks for: its field is the bit length minus one.
    fn data_len(&self, register: u32) -> usize {
        let bits = (self.register(register) & DLEN_MASK) + 1;
        usize::try_from(bits.div_ceil(8)).unwrap_or(DATA_BYTES).min(DATA_BYTES)
    }

    fn run_user(&mut self) {
        let user = self.register(USER);
        let user1 = self.register(USER1);
        // ASSUMPTION: every transfer has an 8-bit command phase (all measured ones do: `USER2`
        // [31:28] = 7, FB §B.2); the opcode is the low byte of the command value.
        let opcode = self.user_opcode();
        let address = (user & USR_ADDR != 0).then(|| {
            // The field can ask for up to 64 bits; the register holds 32.
            let bits = (((user1 >> ADDR_BITLEN_SHIFT) & ADDR_BITLEN_MASK) + 1).min(u32::BITS);
            // ASSUMPTION: the address is the low `bits` bits of ADDR, unshifted (ESP-IDF
            // `spimem_flash_ll_set_usr_address` stores it as is). Only the SFDP read uses an address
            // on the measured paths and its answer does not depend on it; the lockstep settles it.
            self.register(ADDR) & u32::MAX.checked_shr(u32::BITS - bits).unwrap_or(u32::MAX)
        });
        let dummy_cycles = if user & USR_DUMMY == 0 { 0 } else { (user1 & DUMMY_CYCLES_MASK) + 1 };
        let data_out: Vec<u8> = if user & USR_MOSI == 0 {
            Vec::new()
        } else {
            (0..DATA_WORDS)
                .flat_map(|word| self.register(W0 + 4 * word).to_le_bytes())
                .take(self.data_len(MOSI_DLEN))
                .collect()
        };
        let data_in_len = if user & USR_MISO == 0 { 0 } else { self.data_len(MISO_DLEN) };
        let command = FlashCommand { opcode, address, dummy_cycles, data_out: &data_out };
        if let Some(data_in) = self.transfer(&command, data_in_len) {
            self.store_data_in(&data_in);
        }
    }

    /// Stores the bytes read into `W0..`, the first byte in `W0[7:0]` (FB §D.1).
    /// ASSUMPTION: the bytes of the last word that were not read are zero (FB §L item 7).
    fn store_data_in(&mut self, bytes: &[u8]) {
        for (word, chunk) in (0u32..).zip(bytes.chunks(WORD_BYTES)) {
            let mut le = [0u8; WORD_BYTES];
            le[..chunk.len()].copy_from_slice(chunk);
            self.registers.write(W0 + 4 * word, u32::from_le_bytes(le));
        }
    }

    /// Runs every command whose bit is set. Transfers complete at once, so `CMD` reads 0 afterwards.
    fn run(&mut self, command_bits: u32) {
        for (bit, opcode) in HARD_WIRED {
            if command_bits & bit != 0 {
                self.run_hard_wired(bit, opcode);
            }
        }
        if command_bits & CMD_PE != 0 {
            let opcode = self.user_opcode();
            self.run_hard_wired(CMD_PE, opcode);
        }
        if command_bits & CMD_USR != 0 {
            self.run_user();
        }
    }
}

impl Peripheral for Spi1 {
    fn read(&mut self, offset: u32) -> Option<u32> {
        if offset == CMD {
            // Every command bit self-clears and the state nibbles read idle (FB §C rows 9 and 11).
            return Some(0);
        }
        self.registers.read(offset)
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        if offset == CMD {
            self.run(value);
            return true;
        }
        self.registers.write(offset, value)
    }
}
```

`chunk.len()` is at most `WORD_BYTES`, so `le[..chunk.len()]` is in bounds (write `// INVARIANT:` above it). `USER` bit 31 (`USR_COMMAND`) has no constant in the model: nothing there reads it (a constant used by the tests only is dead code for the library build), so it is declared in the `tests` module (Step 1).

- [ ] **Step 4: Wire the two controllers and the flash through `Peripherals`, the bus and the machine**

`periph/mod.rs`: `pub mod spi_mem;`, `pub use spi_mem::{Spi0, Spi1};`, the bases

```rust
// The two MSPI controllers (`reg_base.h` `DR_REG_SPI1_BASE`, `DR_REG_SPI0_BASE`; report BC §A.2).
pub(crate) const SPI1_BASE: u32 = 0x6000_2000;
const SPI1: (u32, u32) = (SPI1_BASE, 0x6000_3000);
pub(crate) const SPI0_BASE: u32 = 0x6000_3000;
const SPI0: (u32, u32) = (SPI0_BASE, 0x6000_4000);
```

fields `pub spi0: Spi0, pub spi1: Spi1`, the constructor `pub fn new(config: &SocConfig, flash: Box<dyn FlashDevice>) -> Peripherals` (`spi0: Spi0::new(), spi1: Spi1::new(flash)`), two arms in `device` (`within(SPI1)` → `(&mut self.spi1, SPI1_BASE)`, `within(SPI0)` → `(&mut self.spi0, SPI0_BASE)`), `spi_mem::accepted()` added to the array of `accepted_without_effect`, and

```rust
    /// What a device was asked to do and cannot do faithfully, since the last call.
    pub(crate) fn take_unsupported(&mut self) -> Option<Unsupported> {
        self.spi1.take_unsupported()
    }
```

In the `tests` module of `mod.rs`, `Peripherals::new(&SocConfig::default())` becomes `Peripherals::new(&SocConfig::default(), Box::new(crate::flash::NorFlash::blank(crate::flash::FlashConfig::default())))`.

`bus.rs`: `pub fn new(rom: RomImage, flash: Box<dyn FlashDevice>, config: &SocConfig) -> SocBus` passing `flash` to `Peripherals::new`; add

```rust
    /// Turns what a device could not do into a diagnostic that names the instruction.
    fn relay_unsupported(&mut self) {
        if let Some(what) = self.periph.take_unsupported() {
            self.diag.record(Diagnostic::Unsupported { what, pc: self.pc });
        }
    }
```

and call it in `mmio_read` (after `modelled` is computed, before the value is returned) and in `mmio_write` (after the `handled` block).

`machine.rs`: factor the construction.

```rust
    /// A machine in its power-on reset state with an erased flash.
    #[must_use]
    pub fn new(rom: RomImage, config: &SocConfig) -> Esp32C3 {
        Esp32C3::build(rom, NorFlash::blank(config.flash), config)
    }

    /// A machine whose flash holds `image` from offset 0; what the image does not cover is erased.
    ///
    /// # Errors
    /// [`FlashImageError`] when the image is longer than the flash or the flash size is not valid.
    pub fn with_flash(rom: RomImage, image: Vec<u8>, config: &SocConfig) -> Result<Esp32C3, FlashImageError> {
        Ok(Esp32C3::build(rom, NorFlash::with_image(image, config.flash)?, config))
    }

    fn build(rom: RomImage, flash: NorFlash, config: &SocConfig) -> Esp32C3 { /* the former body of `new`, with `SocBus::new(rom, Box::new(flash), config)` */ }
```

Imports this step needs: `periph/mod.rs`: `use crate::diag::Unsupported;` and `use crate::flash::FlashDevice;`; `bus.rs`: `use crate::flash::FlashDevice;`; `machine.rs`: `use crate::flash::{FlashImageError, NorFlash};`.

- [ ] **Step 5: Regenerate the documented table and run**

Run: `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect > /tmp/t5doc.log 2>&1; echo "exit=$?"`
Expected: `exit=0`: the lists changed in this task, so the test rewrote the table of `docs/registers-accepted-without-effect.md` (run without the variable it fails and names this command; it never writes then).
Check `/usr/bin/git diff -- docs/registers-accepted-without-effect.md`: the diff touches table rows only, nothing above the `<!-- generated table -->` line, and `grep -c '^| .0x' docs/registers-accepted-without-effect.md` prints `50`. The file is part of this task's commit. Then:
Run: `cargo test --workspace > /tmp/t5.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.
Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/t5c.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 6: Commit**

Before committing (Global Constraints): `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`: `exit=0` three times. The code blocks of this plan are not wrapped as `rustfmt` wraps them.

```bash
/usr/bin/git add crates/soc-esp32c3 docs/registers-accepted-without-effect.md
/usr/bin/git commit -m "feat(soc): SPI1 flash controller with hard-wired and user commands, SPI0 as storage

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3 docs/registers-accepted-without-effect.md
```

---

### Task 6: Cache controller (`EXTMEM`), MMU table, and the storage the ROM flash path needs

**Files:**
- Rewrite: `crates/soc-esp32c3/src/periph/extmem.rs`
- Create: `crates/soc-esp32c3/src/periph/mmu.rs`
- Modify: `crates/soc-esp32c3/src/periph/{mod,gpio,system}.rs`, `crates/soc-esp32c3/src/bus.rs` (one existing test)
- Modify: `docs/registers-accepted-without-effect.md`

**Interfaces:**
- Consumes: `Accepted`, `AcceptedBlock`, `Storage::accepted`, `accepted_without_effect` (Task 2); `svd_regs::esp32c3::addr` (Task 1).
- Produces:
  ```rust
  // periph/mmu.rs
  /// `DR_REG_MMU_TABLE` (ESP-IDF `reg_base.h`; CM §1). Not in the SVD.
  pub const MMU_TABLE_BASE: u32 = 0x600C_5000;
  pub const MMU_ENTRIES: usize = 128;
  pub const MMU_PAGE_BYTES: u32 = 0x1_0000;
  pub const MMU_ENTRY_INVALID: u32 = 1 << 8;
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct MmuMiss { pub entry: u32, pub content: u32 }
  pub struct MmuTable { /* private */ }
  impl MmuTable {
      pub const fn new() -> MmuTable;                                         // every entry invalid
      pub fn entry(&self, index: usize) -> Option<u32>;
      /// Flash offset of an offset inside a flash window (`vaddr & 0x7F_FFFF`).
      pub fn translate(&self, window_offset: u32) -> Result<u32, MmuMiss>;
      pub const fn generation(&self) -> u64;                                  // bumps on every write
  }
  // periph/extmem.rs
  impl Extmem {
      pub fn new() -> Extmem;
      pub const fn generation(&self) -> u64;              // bumps on ICACHE_CTRL / ICACHE_CTRL1 writes
      pub fn latch_mmu_fault(&mut self, vaddr: u32, content: u32);
      pub fn latch_store(&mut self, instruction_bus: bool);
  }
  // `pub`, like `generation` and `MmuTable::translate`: their only caller, the bus, arrives in Task 7,
  // and a `pub(crate)` method nothing calls yet is dead code under `-D warnings`.
  // Peripherals: new fields `pub mmu: MmuTable`, `pub sensitive: AcceptedBlock`
  ```

Behaviour and values (CM §2, §10; FB §B.3, §B.5, §C rows 5–8):

| Register | Model |
|---|---|
| `ICACHE_CTRL` | storage, reset 0; a write bumps the generation |
| `ICACHE_CTRL1` | storage, reset 3; a write bumps the generation |
| `ICACHE_SYNC_CTRL` | reads `0x2` always (`SYNC_DONE`); writes accepted (the invalidation has nothing to invalidate) |
| `ICACHE_PRELOAD_CTRL` | reads `0x2` or-ed with the stored `ORDER` bit 2 |
| `ICACHE_AUTOLOAD_CTRL` | reads the stored bits or-ed with `0x8` (`DONE`); `DONE` is not writable; `ENA` is **not** cleared by a read |
| `ICACHE_FREEZE` | stores bits 1:0; `DONE` (bit 2) reads as `ENA` (bit 0) |
| `CACHE_STATE` | reads 1 |
| `CACHE_ILG_INT_ST`, `CORE0_ACS_CACHE_INT_ST` | latched status, cleared by a write of the same bit to the `_CLR` register |
| `CACHE_MMU_FAULT_CONTENT`, `CACHE_MMU_FAULT_VADDR` | record of the last invalid mapping |
| the storage list below | read back what was written |
| any other EXTMEM register | unmodelled (reported by name) |

- [ ] **Step 1: Write the failing tests**

`extmem.rs`, module `tests` (replaces the two existing tests):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn reg(name: &str) -> u32 {
        svd_regs::esp32c3::addr("EXTMEM", name) - EXTMEM_BASE
    }

    #[test]
    fn reset_state_lets_every_rom_poll_terminate() {
        let mut x = Extmem::new();
        assert_eq!(x.read(reg("ICACHE_CTRL")), Some(0));
        assert_eq!(x.read(reg("ICACHE_CTRL1")), Some(3), "CM §2: both buses shut");
        assert_eq!(x.read(reg("ICACHE_SYNC_CTRL")), Some(2), "decision D23");
        assert_eq!(x.read(reg("ICACHE_PRELOAD_CTRL")), Some(2), "Cache_Suspend_ICache spins on bit 1");
        assert_eq!(x.read(reg("ICACHE_AUTOLOAD_CTRL")), Some(8), "Cache_Suspend_ICache_Autoload spins on bit 3");
        assert_eq!(x.read(reg("CACHE_STATE")), Some(1), "Cache_Disable_ICache waits for exactly 1");
        assert_eq!(x.read(reg("ICACHE_FREEZE")), Some(0));
    }

    #[test]
    fn invalidate_completes_at_once() {
        // CM §1.2: `R SYNC_CTRL=2 ; W SYNC_CTRL=3 ; R =2`, with SYNC_ADDR and SYNC_SIZE stored.
        let mut x = Extmem::new();
        assert!(x.write(reg("ICACHE_SYNC_ADDR"), 0x3C55_0000));
        assert!(x.write(reg("ICACHE_SYNC_SIZE"), 0x800));
        assert!(x.write(reg("ICACHE_SYNC_CTRL"), 3));
        assert_eq!(x.read(reg("ICACHE_SYNC_CTRL")), Some(2));
        assert_eq!(x.read(reg("ICACHE_SYNC_ADDR")), Some(0x3C55_0000));
        assert_eq!(x.read(reg("ICACHE_SYNC_SIZE")), Some(0x800));
    }

    #[test]
    fn freeze_done_follows_enable() {
        // FB §B.5: enable, poll DONE set; then the ROM writes back the value without bit 0, poll DONE clear.
        let mut x = Extmem::new();
        x.write(reg("ICACHE_FREEZE"), 1);
        let frozen = x.read(reg("ICACHE_FREEZE")).unwrap();
        assert_ne!(frozen & 4, 0);
        x.write(reg("ICACHE_FREEZE"), frozen & !1);
        assert_eq!(x.read(reg("ICACHE_FREEZE")), Some(0), "software cannot hold DONE");
        x.write(reg("ICACHE_FREEZE"), 3);
        assert_eq!(x.read(reg("ICACHE_FREEZE")), Some(7), "MODE is stored");
    }

    #[test]
    fn autoload_enable_is_a_stored_bit_and_done_cannot_be_cleared() {
        let mut x = Extmem::new();
        x.write(reg("ICACHE_AUTOLOAD_CTRL"), 4);
        assert_eq!(x.read(reg("ICACHE_AUTOLOAD_CTRL")), Some(0xC));
        assert_eq!(x.read(reg("ICACHE_AUTOLOAD_CTRL")), Some(0xC), "a read does not clear ENA (CM §2)");
        x.write(reg("ICACHE_AUTOLOAD_CTRL"), 0);
        assert_eq!(x.read(reg("ICACHE_AUTOLOAD_CTRL")), Some(8));
        x.write(reg("ICACHE_PRELOAD_CTRL"), 5);
        assert_eq!(x.read(reg("ICACHE_PRELOAD_CTRL")), Some(6), "ENA self-clears, ORDER kept, DONE set");
    }

    #[test]
    fn configuration_is_stored_from_its_reset_value() {
        let mut x = Extmem::new();
        for (name, reset) in [
            ("CACHE_MMU_OWNER", 0),
            ("CACHE_WRAP_AROUND_CTRL", 0),
            ("ICACHE_TAG_POWER_CTRL", 5),
            ("CACHE_MMU_POWER_CTRL", 5),
            ("CACHE_ILG_INT_ENA", 0),
            ("CORE0_ACS_CACHE_INT_ENA", 0),
        ] {
            assert_eq!(x.read(reg(name)), Some(reset), "{name}");
            assert!(x.write(reg(name), 0xF));
            assert_eq!(x.read(reg(name)), Some(0xF), "{name}");
        }
    }

    #[test]
    fn bus_enable_writes_change_the_generation() {
        let mut x = Extmem::new();
        let before = x.generation();
        x.write(reg("ICACHE_CTRL"), 1);
        assert_ne!(x.generation(), before);
        let before = x.generation();
        x.write(reg("ICACHE_CTRL1"), 0);
        assert_ne!(x.generation(), before);
        let before = x.generation();
        x.write(reg("ICACHE_SYNC_CTRL"), 1);
        assert_eq!(x.generation(), before, "an invalidation maps nothing new");
    }

    #[test]
    fn an_invalid_mapping_is_latched_until_software_clears_it() {
        let mut x = Extmem::new();
        x.latch_mmu_fault(0x3C03_0010, 0x100);
        assert_eq!(x.read(reg("CACHE_ILG_INT_ST")), Some(1 << 5), "MMU_ENTRY_FAULT_ST (CM §6.1)");
        assert_eq!(x.read(reg("CACHE_MMU_FAULT_VADDR")), Some(0x3C03_0010));
        assert_eq!(x.read(reg("CACHE_MMU_FAULT_CONTENT")), Some(0x100 | (1 << 10)), "content, code 1 = CPU miss");
        assert!(x.write(reg("CACHE_ILG_INT_CLR"), 0xFFFF_FFFF), "Cache_Set_Default_Mode (FB §B.3)");
        assert_eq!(x.read(reg("CACHE_ILG_INT_ST")), Some(0));
        assert!(!x.write(reg("CACHE_ILG_INT_ST"), 1), "status is read-only");
    }

    #[test]
    fn a_store_to_a_window_is_latched_per_bus() {
        let mut x = Extmem::new();
        x.latch_store(false);
        assert_eq!(x.read(reg("CORE0_ACS_CACHE_INT_ST")), Some(1 << 5), "DBUS_WR_ICACHE_ST");
        x.latch_store(true);
        assert_eq!(x.read(reg("CORE0_ACS_CACHE_INT_ST")), Some((1 << 5) | (1 << 1)), "IBUS_WR_ICACHE_ST");
        x.write(reg("CORE0_ACS_CACHE_INT_CLR"), 0x3F);
        assert_eq!(x.read(reg("CORE0_ACS_CACHE_INT_ST")), Some(0));
    }

    #[test]
    fn registers_no_measured_path_touches_stay_unmodelled() {
        let mut x = Extmem::new();
        for name in ["ICACHE_LOCK_CTRL", "ICACHE_PRELOAD_ADDR", "IBUS_PMS_TBL_LOCK", "CACHE_CONF_MISC", "REG_DATE"] {
            assert_eq!(x.read(reg(name)), None, "{name}");
            assert!(!x.write(reg(name), 1), "{name}");
        }
    }
}
```

`mmu.rs`, module `tests`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    #[test]
    fn every_entry_starts_invalid() {
        let mut mmu = MmuTable::new();
        for offset in (0..0x200).step_by(4) {
            assert_eq!(mmu.read(offset), Some(0x100), "{offset:#x}");
        }
        assert_eq!(mmu.read(0x200), None, "128 entries, 0x200 bytes (CM §1)");
        assert!(!mmu.write(0x200, 0));
        assert_eq!(mmu.translate(0), Err(MmuMiss { entry: 0, content: 0x100 }));
    }

    #[test]
    fn an_entry_keeps_nine_bits_and_reads_back() {
        let mut mmu = MmuTable::new();
        assert!(mmu.write(4 * 85, 0xFFFF_FFFF));
        assert_eq!(mmu.read(4 * 85), Some(0x1FF), "CM §1: 0x1FF kept after writing 0xFFFFFFFF");
        assert_eq!(mmu.entry(85), Some(0x1FF));
        assert_eq!(mmu.entry(128), None);
    }

    #[test]
    fn translation_uses_the_page_number_and_keeps_the_offset_in_the_page() {
        // CM §1: entry_id = (vaddr & 0x7FFFFF) >> 16; paddr = (entry & 0xFF) << 16 | (vaddr & 0xFFFF).
        let mut mmu = MmuTable::new();
        mmu.write(4 * 3, 1);
        assert_eq!(mmu.translate(0x03_0000), Ok(0x1_0000));
        assert_eq!(mmu.translate(0x03_FFFF), Ok(0x1_FFFF));
        assert_eq!(mmu.translate(0x04_0000), Err(MmuMiss { entry: 4, content: 0x100 }));
        mmu.write(4 * 127, 0x35);
        assert_eq!(mmu.translate(0x7F_0020), Ok(0x35_0020), "entry 127 is the bootloader's one-page window (BA §2.2)");
    }

    #[test]
    fn every_write_changes_the_generation() {
        let mut mmu = MmuTable::new();
        let first = mmu.generation();
        mmu.write(0, 0);
        let second = mmu.generation();
        mmu.write(0, 0);
        assert!(first != second && second != mmu.generation(), "even an identical value: the cost is one re-decode");
    }
}
```

`gpio.rs`, module `tests` (create it) and `system.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    #[test]
    fn output_enable_and_the_two_input_selectors_are_stored() {
        // FB §B.1: gpio_output_disable clears bits of ENABLE; gpio_bypass_matrix_in clears bit 6 of FUNCn_IN_SEL_CFG.
        let mut gpio = Gpio::new(0x8);
        for name in ["ENABLE", "FUNC0_IN_SEL_CFG", "FUNC1_IN_SEL_CFG"] {
            let offset = svd_regs::esp32c3::addr("GPIO", name) - crate::periph::GPIO_BASE;
            assert_eq!(gpio.read(offset), Some(0), "{name}");
            assert!(gpio.write(offset, 0x40));
            assert_eq!(gpio.read(offset), Some(0x40));
        }
        assert_eq!(gpio.read(STRAP), Some(0x8), "STRAP is unchanged");
        assert!(!gpio.write(STRAP, 0), "STRAP is read-only");
        let out = svd_regs::esp32c3::addr("GPIO", "OUT") - crate::periph::GPIO_BASE;
        assert_eq!(gpio.read(out), None, "OUT is not modelled");
    }
}
```

```rust
    #[test]
    fn cache_control_is_storage_from_its_reset_value() {
        // CM §2.1: reset 5; ROM_Boot_Cache_Init does |=1, |=2, &=~2.
        let mut sys = System::new();
        let offset = svd_regs::esp32c3::addr("SYSTEM", "CACHE_CONTROL") - SYSTEM_BASE;
        assert_eq!(sys.read(offset), Some(5));
        assert!(sys.write(offset, 7));
        assert_eq!(sys.read(offset), Some(7));
    }
```

In `mod.rs` tests:

```rust
    #[test]
    fn the_mmu_table_sits_right_after_the_cache_registers() {
        let mut p = Peripherals::new(&SocConfig::default(), Box::new(crate::flash::NorFlash::blank(crate::flash::FlashConfig::default())));
        let base_of = |p: &mut Peripherals, addr: u32| p.device(addr).map(|(_, base)| base);
        assert_eq!(base_of(&mut p, 0x600C_4FFC), Some(EXTMEM_BASE));
        assert_eq!(base_of(&mut p, 0x600C_5000), Some(mmu::MMU_TABLE_BASE));
        assert_eq!(base_of(&mut p, 0x600C_51FC), Some(mmu::MMU_TABLE_BASE));
        assert_eq!(base_of(&mut p, 0x600C_5200), None, "nothing is documented past the 128 entries");
        assert_eq!(base_of(&mut p, svd_regs::esp32c3::addr("SENSITIVE", "INTERNAL_SRAM_USAGE_1")), Some(SENSITIVE_BASE));
    }
```

In `bus.rs` (module `tests`), the existing test `gap_in_peripheral_window_reads_zero_and_warns` reads `0x600C_5000` as an address where nothing is documented; this task puts the MMU table there. Move it to the first word after the table: `assert_eq!(load(&mut b, 0x600C_5200), 0); // right after the MMU table: nothing is documented there` (the rest of the test is unchanged: the SVD has no register at that address either).

- [ ] **Step 2: Run, see the failure**

Run: `cargo test -p soc-esp32c3 > /tmp/t6.log 2>&1; echo "exit=$?"`
Expected: `exit=101`.

- [ ] **Step 3: Implement `periph/mmu.rs`**

```rust
//! The flash MMU table: 128 entries that map 64 KiB pages of the two flash windows to flash pages.
//! Source: report CM §1 (ESP-IDF `ext_mem_defs.h`, `mmu_ll.h`; ROM `Cache_MMU_Init`).
use emu_core::Peripheral;

/// `DR_REG_MMU_TABLE` (ESP-IDF `reg_base.h`). The SVD does not describe this block (decision D17).
pub const MMU_TABLE_BASE: u32 = 0x600C_5000;
/// `SOC_MMU_ENTRY_NUM`.
pub const MMU_ENTRIES: usize = 128;
/// Fixed page size: `mmu_ll_get_page_size` returns 64 KiB unconditionally.
pub const MMU_PAGE_BYTES: u32 = 0x1_0000;
/// `SOC_MMU_INVALID`: bit 8 set means the entry maps nothing.
pub const MMU_ENTRY_INVALID: u32 = 1 << 8;
/// `SOC_MMU_VALID_VAL_MASK`: the flash page number.
const PAGE_NUMBER_MASK: u32 = 0xFF;
/// ASSUMPTION: an entry is 9 bits wide, as the QEMU oracle stores it (0xFFFFFFFF reads back 0x1FF).
/// `CACHE_MMU_FAULT_CONTENT` is 10 bits wide, so silicon may keep bit 9; software only uses [8:0].
const ENTRY_BITS: u32 = 0x1FF;
const ENTRY_BYTES: u32 = 4;
const PAGE_SHIFT: u32 = 16;
const PAGE_OFFSET_MASK: u32 = MMU_PAGE_BYTES - 1;

/// An access through an entry that maps nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MmuMiss {
    pub entry: u32,
    pub content: u32,
}

#[derive(Clone, Debug)]
pub struct MmuTable {
    entries: [u32; MMU_ENTRIES],
    generation: u64,
}

impl MmuTable {
    /// ASSUMPTION: every entry is invalid at power-on. The content before the ROM runs is not
    /// documented; the ROM writes all 128 entries before it uses one (`Cache_MMU_Init`, FB §B.3).
    #[must_use]
    pub const fn new() -> MmuTable {
        MmuTable { entries: [MMU_ENTRY_INVALID; MMU_ENTRIES], generation: 0 }
    }

    #[must_use]
    pub fn entry(&self, index: usize) -> Option<u32> {
        self.entries.get(index).copied()
    }

    /// The flash offset `window_offset` (an address minus the base of its window) maps to.
    ///
    /// # Errors
    /// [`MmuMiss`] when the entry is invalid or `window_offset` lies past the 128 pages.
    pub fn translate(&self, window_offset: u32) -> Result<u32, MmuMiss> {
        let entry = window_offset >> PAGE_SHIFT;
        let content = usize::try_from(entry)
            .ok()
            .and_then(|index| self.entry(index))
            .unwrap_or(MMU_ENTRY_INVALID);
        if content & MMU_ENTRY_INVALID != 0 {
            return Err(MmuMiss { entry, content });
        }
        Ok(((content & PAGE_NUMBER_MASK) << PAGE_SHIFT) | (window_offset & PAGE_OFFSET_MASK))
    }

    /// Changes whenever the mapping may have changed: the bus uses it as the version of the code
    /// decoded through the flash windows (decision D6).
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    fn index(offset: u32) -> Option<usize> {
        usize::try_from(offset / ENTRY_BYTES).ok().filter(|index| *index < MMU_ENTRIES)
    }
}

impl Default for MmuTable {
    fn default() -> MmuTable {
        MmuTable::new()
    }
}

impl Peripheral for MmuTable {
    fn read(&mut self, offset: u32) -> Option<u32> {
        Self::index(offset).and_then(|index| self.entry(index))
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        let Some(slot) = Self::index(offset).and_then(|index| self.entries.get_mut(index)) else {
            return false;
        };
        *slot = value & ENTRY_BITS;
        self.generation += 1;
        true
    }
}
```

- [ ] **Step 4: Implement `periph/extmem.rs`** (replace the file)

```rust
//! `EXTMEM`: the flash-cache controller. Source: report CM §2 and §10 (ESP-IDF `extmem_reg.h`, the
//! ROM cache functions), FB §B.3 and §B.5.
//!
//! No cache line is modelled: the bus reads the flash through the MMU table at every access
//! (decision D4). The operations software waits for (invalidate, preload, autoload, freeze)
//! therefore complete at once, and the registers below only keep what software reads back.
use super::{Accepted, EXTMEM_BASE, Storage};
use emu_core::Peripheral;
use svd_regs::esp32c3::addr;

const fn reg(name: &str) -> u32 {
    addr("EXTMEM", name) - EXTMEM_BASE
}

const ICACHE_CTRL: u32 = reg("ICACHE_CTRL");
const ICACHE_CTRL1: u32 = reg("ICACHE_CTRL1");
const ICACHE_SYNC_CTRL: u32 = reg("ICACHE_SYNC_CTRL");
const ICACHE_PRELOAD_CTRL: u32 = reg("ICACHE_PRELOAD_CTRL");
const ICACHE_AUTOLOAD_CTRL: u32 = reg("ICACHE_AUTOLOAD_CTRL");
const ICACHE_FREEZE: u32 = reg("ICACHE_FREEZE");
const CACHE_STATE: u32 = reg("CACHE_STATE");
const CACHE_ILG_INT_CLR: u32 = reg("CACHE_ILG_INT_CLR");
const CACHE_ILG_INT_ST: u32 = reg("CACHE_ILG_INT_ST");
const CORE0_ACS_CACHE_INT_CLR: u32 = reg("CORE0_ACS_CACHE_INT_CLR");
const CORE0_ACS_CACHE_INT_ST: u32 = reg("CORE0_ACS_CACHE_INT_ST");
const CACHE_MMU_FAULT_CONTENT: u32 = reg("CACHE_MMU_FAULT_CONTENT");
const CACHE_MMU_FAULT_VADDR: u32 = reg("CACHE_MMU_FAULT_VADDR");

/// `ICACHE_SYNC_CTRL` bit 1, `SYNC_DONE`. ASSUMPTION: (decision D23) it reads set from reset. The
/// SVD gives reset 1 (`INVALIDATE_ENA` set, `DONE` clear), which reads as a tag initialisation that
/// ends before the ROM runs; the ROM sets bit 0 and waits for bit 1 either way (CM §9 item 5).
const SYNC_DONE: u32 = 1 << 1;
/// `ICACHE_PRELOAD_CTRL`: bit 1 `DONE` (always set: nothing is ever preloading), bit 2 `ORDER` (stored).
const PRELOAD_DONE: u32 = 1 << 1;
const PRELOAD_ORDER: u32 = 1 << 2;
/// `ICACHE_AUTOLOAD_CTRL`: bit 3 `DONE` (read-only, set); bits 0, 1, 2, 4, 5, 6 are configuration.
const AUTOLOAD_DONE: u32 = 1 << 3;
const AUTOLOAD_CONFIG: u32 = 0x77;
/// `ICACHE_FREEZE`: bit 0 `ENA`, bit 1 `MODE`, bit 2 `DONE` which mirrors `ENA`.
const FREEZE_ENA: u32 = 1 << 0;
const FREEZE_MODE: u32 = 1 << 1;
const FREEZE_DONE: u32 = 1 << 2;
/// `CACHE_STATE` [11:0]: 1 is idle, the only value `Cache_Disable_ICache` accepts.
const CACHE_IDLE: u32 = 1;
/// `CACHE_ILG_INT_ST` bit 5: an access went through an invalid MMU entry.
const MMU_ENTRY_FAULT: u32 = 1 << 5;
/// `CACHE_MMU_FAULT_CONTENT`: [9:0] the entry, [13:10] the operation; 1 is a CPU access.
const FAULT_CONTENT_MASK: u32 = 0x3FF;
const FAULT_CODE_CPU: u32 = 1 << 10;
/// `CORE0_ACS_CACHE_INT_ST`: bit 1 a store through the instruction bus, bit 5 through the data bus.
const IBUS_WRITE: u32 = 1 << 1;
const DBUS_WRITE: u32 = 1 << 5;

const WHY_OPERATION: &str = "cache operation: completes at once, there are no cache lines";
const WHY_FAULT_IRQ: &str = "cache error interrupt: the status is latched, no interrupt is raised";
const ACCEPTED: [Accepted; 14] = [
    Accepted::svd("EXTMEM", "ICACHE_CTRL", "cache enable: the flash windows are always readable (D5)", "SP1b-4"),
    Accepted::svd("EXTMEM", "ICACHE_CTRL1", "bus gating of the flash windows (SHUT_IBUS, SHUT_DBUS; D5)", "SP1b-4"),
    Accepted::svd("EXTMEM", "ICACHE_TAG_POWER_CTRL", "tag memory power", "SP8"),
    Accepted::svd("EXTMEM", "ICACHE_SYNC_CTRL", WHY_OPERATION, "SP1b-4"),
    Accepted::svd("EXTMEM", "ICACHE_SYNC_ADDR", WHY_OPERATION, "SP1b-4"),
    Accepted::svd("EXTMEM", "ICACHE_SYNC_SIZE", WHY_OPERATION, "SP1b-4"),
    Accepted::svd("EXTMEM", "ICACHE_PRELOAD_CTRL", WHY_OPERATION, "SP1b-4"),
    Accepted::svd("EXTMEM", "ICACHE_AUTOLOAD_CTRL", WHY_OPERATION, "SP1b-4"),
    Accepted::svd("EXTMEM", "CACHE_ILG_INT_ENA", WHY_FAULT_IRQ, "SP1b-3"),
    Accepted::svd("EXTMEM", "CORE0_ACS_CACHE_INT_ENA", WHY_FAULT_IRQ, "SP1b-3"),
    Accepted::svd("EXTMEM", "CACHE_WRAP_AROUND_CTRL", "wrap-around read mode", "SP1b-4"),
    Accepted::svd("EXTMEM", "CACHE_MMU_POWER_CTRL", "MMU memory power", "SP8"),
    Accepted::svd("EXTMEM", "CACHE_MMU_OWNER", "MMU ownership bits", "SP1b-4"),
    Accepted::svd("EXTMEM", "ICACHE_FREEZE", WHY_OPERATION, "SP1b-4"),
];

pub(crate) fn accepted() -> Vec<Accepted> {
    ACCEPTED.to_vec()
}

#[derive(Clone, Debug)]
pub struct Extmem {
    registers: Storage,
    preload_order: u32,
    autoload: u32,
    freeze: u32,
    illegal_status: u32,
    access_status: u32,
    fault_content: u32,
    fault_vaddr: u32,
    generation: u64,
}

impl Extmem {
    #[must_use]
    pub fn new() -> Extmem {
        Extmem {
            registers: Storage::accepted(EXTMEM_BASE, &ACCEPTED),
            preload_order: 0,
            autoload: 0,
            freeze: 0,
            illegal_status: 0,
            access_status: 0,
            fault_content: 0,
            fault_vaddr: 0,
            generation: 0,
        }
    }

    /// Changes when the cache or one of its buses is switched on or off (decision D6).
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Records an access through an invalid MMU entry (`CACHE_ILG_INT_ST`, `CACHE_MMU_FAULT_*`; CM §6.1).
    pub fn latch_mmu_fault(&mut self, vaddr: u32, content: u32) {
        self.illegal_status |= MMU_ENTRY_FAULT;
        self.fault_content = (content & FAULT_CONTENT_MASK) | FAULT_CODE_CPU;
        self.fault_vaddr = vaddr;
    }

    /// Records a store to a flash window (`CORE0_ACS_CACHE_INT_ST`; CM §6.1).
    pub fn latch_store(&mut self, instruction_bus: bool) {
        self.access_status |= if instruction_bus { IBUS_WRITE } else { DBUS_WRITE };
    }
}

impl Default for Extmem {
    fn default() -> Extmem {
        Extmem::new()
    }
}

impl Peripheral for Extmem {
    fn read(&mut self, offset: u32) -> Option<u32> {
        Some(match offset {
            ICACHE_SYNC_CTRL => SYNC_DONE,
            ICACHE_PRELOAD_CTRL => PRELOAD_DONE | self.preload_order,
            ICACHE_AUTOLOAD_CTRL => AUTOLOAD_DONE | self.autoload,
            ICACHE_FREEZE => {
                let done = if self.freeze & FREEZE_ENA == 0 { 0 } else { FREEZE_DONE };
                self.freeze | done
            }
            CACHE_STATE => CACHE_IDLE,
            CACHE_ILG_INT_ST => self.illegal_status,
            CORE0_ACS_CACHE_INT_ST => self.access_status,
            CACHE_MMU_FAULT_CONTENT => self.fault_content,
            CACHE_MMU_FAULT_VADDR => self.fault_vaddr,
            // Write-only registers read 0.
            CACHE_ILG_INT_CLR | CORE0_ACS_CACHE_INT_CLR => 0,
            _ => return self.registers.read(offset),
        })
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        match offset {
            // The invalidation has nothing to invalidate; `SYNC_DONE` already reads set.
            ICACHE_SYNC_CTRL => {}
            ICACHE_PRELOAD_CTRL => self.preload_order = value & PRELOAD_ORDER,
            ICACHE_AUTOLOAD_CTRL => self.autoload = value & AUTOLOAD_CONFIG,
            ICACHE_FREEZE => self.freeze = value & (FREEZE_ENA | FREEZE_MODE),
            CACHE_ILG_INT_CLR => self.illegal_status &= !value,
            CORE0_ACS_CACHE_INT_CLR => self.access_status &= !value,
            ICACHE_CTRL | ICACHE_CTRL1 => {
                self.generation += 1;
                return self.registers.write(offset, value);
            }
            // Read-only: the write is reported as unmodelled.
            CACHE_STATE | CACHE_ILG_INT_ST | CORE0_ACS_CACHE_INT_ST | CACHE_MMU_FAULT_CONTENT
            | CACHE_MMU_FAULT_VADDR => return false,
            _ => return self.registers.write(offset, value),
        }
        true
    }
}
```

- [ ] **Step 5: GPIO, `SYSTEM.CACHE_CONTROL`, `SENSITIVE`, and the registry**

`gpio.rs`: add the storage.

```rust
/// What the ROM's flash-boot path reads back and modifies (FB §B.1): the output enables and the two
/// input selectors of the flash pads. Pins themselves arrive with the board wiring (SP2).
const ACCEPTED: [Accepted; 3] = [
    Accepted::svd("GPIO", "ENABLE", "pin output enable", "SP2"),
    Accepted::svd("GPIO", "FUNC0_IN_SEL_CFG", "input signal routing (GPIO matrix)", "SP2"),
    Accepted::svd("GPIO", "FUNC1_IN_SEL_CFG", "input signal routing (GPIO matrix)", "SP2"),
];
pub(crate) fn accepted() -> Vec<Accepted> {
    ACCEPTED.to_vec()
}
```

`gpio.rs` imports `use super::{Accepted, GPIO_BASE, Storage};`. `Gpio` gains `config: Storage` (`Storage::accepted(GPIO_BASE, &ACCEPTED)`; `Gpio::new` is no longer `const`), `read` becomes `if offset == STRAP { Some(self.strapping) } else { self.config.read(offset) }`, `write` becomes `self.config.write(offset, value)` (`STRAP` is not in the list, so it stays read-only). Replace the literal `const STRAP: u32 = 0x38;` by `const STRAP: u32 = svd_regs::esp32c3::addr("GPIO", "STRAP") - GPIO_BASE;`.

`system.rs`: append to its `ACCEPTED` list (array length 7) `Accepted::svd("SYSTEM", "CACHE_CONTROL", "cache clock enable and reset", "SP1b-4")`.

`periph/mod.rs`: `pub mod mmu;`, `pub use mmu::MmuTable;`,

```rust
pub(crate) const SENSITIVE_BASE: u32 = 0x600C_1000;
const SENSITIVE: (u32, u32) = (SENSITIVE_BASE, 0x600C_2000);
const MMU_TABLE: (u32, u32) = (mmu::MMU_TABLE_BASE, mmu::MMU_TABLE_BASE + 0x200);
/// `Cache_Occupy_ICache_MEMORY` gives SRAM0 to the cache by clearing bit 0 (CM §2.1). SRAM0 stays
/// plain RAM here: nothing uses it once the cache is on (CM §9 item 9).
const SENSITIVE_ACCEPTED: [Accepted; 1] = [Accepted::svd(
    "SENSITIVE",
    "INTERNAL_SRAM_USAGE_1",
    "SRAM0 handed to the cache as line storage",
    "SP1b-4",
)];
```

fields `pub mmu: MmuTable` (`MmuTable::new()`) and `pub sensitive: AcceptedBlock` (`AcceptedBlock::new(SENSITIVE_BASE, &SENSITIVE_ACCEPTED)`), arms in `device` for `SENSITIVE` and `MMU_TABLE` (base `mmu::MMU_TABLE_BASE`), and `gpio::accepted()` plus `SENSITIVE_ACCEPTED.to_vec()` in `accepted_without_effect`.

- [ ] **Step 6: Regenerate the documented table and run**

Run: `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect > /tmp/t6doc.log 2>&1; echo "exit=$?"`
Expected: `exit=0`: the lists changed in this task, so the test rewrote the table of `docs/registers-accepted-without-effect.md` (run without the variable it fails and names this command; it never writes then).
Check `/usr/bin/git diff -- docs/registers-accepted-without-effect.md`: the diff touches table rows only, nothing above the `<!-- generated table -->` line, and `grep -c '^| .0x' docs/registers-accepted-without-effect.md` prints `68`. The file is part of this task's commit. Then:
Run: `cargo test --workspace > /tmp/t6.log 2>&1; echo "exit=$?"`
Expected: `exit=0`. No existing test uses an EXTMEM register as an unmodelled one (they use `crate::test_support::unmodeled_register()`, a `LEDC` register); the only existing test this task invalidates is `gap_in_peripheral_window_reads_zero_and_warns`, changed in Step 1.
Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/t6c.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.
Do not run the ignored `real_rom` tests at the end of this task: with the cache controller modelled and the flash windows not yet mapped, the ROM gets past its cache polls and takes a load access fault at `0x3C00_0000`, so the three `…_banner_is_reached_without_any_unmodeled_access_or_exception` tests are red at this commit, which is accepted: Task 7 adds the windows and its gate runs the `real_rom` suite as the proof that they are green again.

- [ ] **Step 7: Commit**

Before committing (Global Constraints): `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`: `exit=0` three times. The code blocks of this plan are not wrapped as `rustfmt` wraps them.

```bash
/usr/bin/git add crates/soc-esp32c3 docs/registers-accepted-without-effect.md
/usr/bin/git commit -m "feat(soc): cache controller completion bits, MMU table, GPIO and SENSITIVE storage of the flash-boot path

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3 docs/registers-accepted-without-effect.md
```

---

### Task 7: The two flash windows of the bus

**Files:**
- Modify: `crates/soc-esp32c3/src/memmap.rs`, `crates/soc-esp32c3/src/bus.rs`, `crates/soc-esp32c3/src/machine.rs` (tests)

**Interfaces:**
- Consumes: `MmuTable::{translate, generation}`, `MmuMiss`, `MMU_PAGE_BYTES`, `Extmem::{generation, latch_mmu_fault, latch_store}` (Task 6); `Spi1::flash()` → `&dyn FlashDevice` with `read(offset, out)` and `size()` (Tasks 4, 5); `Diagnostic::FlashMapping`, `MappingFault` (Task 2); `Esp32C3::with_flash` (Task 5).
- Produces:
  ```rust
  // memmap.rs
  pub const FLASH_DBUS_BASE: u32 = 0x3C00_0000;
  pub const FLASH_IBUS_BASE: u32 = 0x4200_0000;
  pub const FLASH_WINDOW_SIZE: u32 = 0x80_0000;
  ```
  Bus behaviour: loads and fetches in `[FLASH_DBUS_BASE, +8 MiB)` and `[FLASH_IBUS_BASE, +8 MiB)` return flash bytes through the MMU table; stores fault; `page_version` of both windows changes on every MMU-table or cache-enable write.

- [ ] **Step 1: Write the failing tests** (`bus.rs`, module `tests`)

```rust
    /// A bus whose flash holds 0x11 at offset 0, then `page << 4 | n` patterns at the start of pages 1 and 2.
    fn bus_with_flash(config: SocConfig) -> SocBus {
        let mut image = vec![0u8; 0x3_0000];
        image[..4].copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
        image[0x1_0000..0x1_0004].copy_from_slice(&[0xE9, 0x06, 0x02, 0x4F]);
        image[0x1_FFFC..0x2_0004].copy_from_slice(&[0xA0, 0xA1, 0xA2, 0xA3, 0xB0, 0xB1, 0xB2, 0xB3]);
        let flash = NorFlash::with_image(image, config.flash).unwrap();
        SocBus::new(RomImage::from_flat(vec![0u8; ROM_SIZE as usize]).unwrap(), Box::new(flash), &config)
    }
    fn map(b: &mut SocBus, entry: u32, page: u32) {
        b.write(crate::periph::mmu::MMU_TABLE_BASE + 4 * entry, Word, page).unwrap();
    }

    #[test]
    fn one_entry_maps_the_same_flash_page_into_both_windows() {
        // CM §7 T2: write entry 3 = 1; 0x3C030000 and 0x42030000 both read flash[0x10000..].
        let mut b = bus_with_flash(SocConfig::default());
        map(&mut b, 3, 1);
        assert_eq!(load(&mut b, FLASH_DBUS_BASE + 0x3_0000), 0x4F02_06E9);
        assert_eq!(load(&mut b, FLASH_IBUS_BASE + 0x3_0000), 0x4F02_06E9);
        assert_eq!(b.read(FLASH_IBUS_BASE + 0x3_0000, Half, Fetch).unwrap().value, 0x06E9);
        assert_eq!(b.read(FLASH_DBUS_BASE + 0x3_0003, Byte, Load).unwrap().value, 0x4F);
        assert_eq!(b.diag.drain(), Vec::new());
    }

    #[test]
    fn a_remap_is_visible_at_once() {
        // CM §7 T3: the window is a live view (no cache contents, decision D4).
        let mut b = bus_with_flash(SocConfig::default());
        map(&mut b, 0, 1);
        assert_eq!(load(&mut b, FLASH_DBUS_BASE), 0x4F02_06E9);
        map(&mut b, 0, 0);
        assert_eq!(load(&mut b, FLASH_DBUS_BASE), 0x4433_2211);
    }

    #[test]
    fn what_the_image_does_not_hold_reads_erased() {
        let mut b = bus_with_flash(SocConfig::default());
        map(&mut b, 0, 0x80);
        assert_eq!(load(&mut b, FLASH_DBUS_BASE), 0xFFFF_FFFF, "page 0x80 = offset 8 MiB, inside the 16 MiB chip");
    }

    #[test]
    fn invalid_entry_reads_zero_latches_the_fault_and_is_reported() {
        let mut b = bus_with_flash(SocConfig { strict_mmio: true, ..SocConfig::default() });
        b.set_pc(0x4004_a186);
        let vaddr = FLASH_DBUS_BASE + 0x5_0010;
        assert_eq!(load(&mut b, vaddr), 0);
        let expected = Diagnostic::FlashMapping {
            vaddr,
            entry: 5,
            content: 0x100,
            fault: crate::diag::MappingFault::InvalidEntry,
            pc: 0x4004_a186,
        };
        assert_eq!(b.diag.drain(), vec![expected.clone()]);
        assert_eq!(b.diag.take_violation(), Some(expected));
        let st = svd_regs::esp32c3::addr("EXTMEM", "CACHE_ILG_INT_ST");
        assert_eq!(load(&mut b, st), 1 << 5);
        assert_eq!(load(&mut b, svd_regs::esp32c3::addr("EXTMEM", "CACHE_MMU_FAULT_VADDR")), vaddr);
        assert_eq!(b.read(FLASH_IBUS_BASE + 0x5_0000, Half, Fetch).unwrap().value, 0, "a fetch decodes 0: illegal instruction");
    }

    #[test]
    fn page_beyond_the_flash_is_reported() {
        let small = SocConfig {
            flash: crate::flash::FlashConfig { size: 1 << 20, ..crate::flash::FlashConfig::default() },
            ..SocConfig::default()
        };
        let flash = NorFlash::with_image(vec![0xAB; 16], small.flash).unwrap();
        let mut b = SocBus::new(RomImage::from_flat(vec![0u8; ROM_SIZE as usize]).unwrap(), Box::new(flash), &small);
        map(&mut b, 0, 0x10); // 1 MiB: the first page past a 1 MiB chip
        assert_eq!(load(&mut b, FLASH_DBUS_BASE), 0);
        assert!(matches!(
            b.diag.drain().as_slice(),
            [Diagnostic::FlashMapping { entry: 0, content: 0x10, fault: crate::diag::MappingFault::BeyondFlash, .. }]
        ));
        map(&mut b, 0, 0x0F);
        assert_eq!(load(&mut b, FLASH_DBUS_BASE), 0xFFFF_FFFF, "the last page of the chip is fine");
    }

    #[test]
    fn load_straddling_two_pages_reads_each_byte_through_its_own_entry() {
        // The machine performs misaligned loads; one that crosses a 64 KiB page uses two entries.
        let mut b = bus_with_flash(SocConfig::default());
        map(&mut b, 0, 1); // window page 0 -> flash page 1, ends with A0 A1 A2 A3
        map(&mut b, 1, 0); // window page 1 -> flash page 0, starts with 11 22 33 44
        assert_eq!(load(&mut b, FLASH_DBUS_BASE + 0xFFFE), 0x2211_A3A2);
        map(&mut b, 1, 2); // -> flash page 2, starts with B0 B1 B2 B3
        assert_eq!(load(&mut b, FLASH_DBUS_BASE + 0xFFFE), 0xB1B0_A3A2);
    }

    #[test]
    fn access_straddling_the_end_of_the_window_is_unmapped() {
        let mut b = bus_with_flash(SocConfig::default());
        map(&mut b, 127, 0);
        let last = FLASH_DBUS_BASE + FLASH_WINDOW_SIZE - 4;
        assert!(b.read(last, Word, Load).is_ok());
        for addr in [last + 2, FLASH_DBUS_BASE + FLASH_WINDOW_SIZE, FLASH_IBUS_BASE + FLASH_WINDOW_SIZE] {
            assert_eq!(b.read(addr, Word, Load), Err(BusError::Unmapped { addr, kind: Load }), "CM §6.2: load at 0x3C800000 faults");
        }
    }

    #[test]
    fn a_store_to_a_window_faults_and_is_latched() {
        let mut b = bus_with_flash(SocConfig::default());
        map(&mut b, 0, 0);
        for (addr, bit) in [(FLASH_DBUS_BASE, 1u32 << 5), (FLASH_IBUS_BASE, 1 << 1)] {
            assert_eq!(b.write(addr, Word, 0), Err(BusError::Unmapped { addr, kind: Store }));
            let st = svd_regs::esp32c3::addr("EXTMEM", "CORE0_ACS_CACHE_INT_ST");
            assert_ne!(load(&mut b, st) & bit, 0);
        }
        assert_eq!(load(&mut b, FLASH_DBUS_BASE), 0x4433_2211, "the flash did not change");
    }

    #[test]
    fn mapping_and_cache_enable_writes_change_the_version_of_both_windows() {
        let mut b = bus_with_flash(SocConfig::default());
        let versions = |b: &SocBus| (b.page_version(FLASH_IBUS_BASE + 0x2000), b.page_version(FLASH_DBUS_BASE + 0x7F_0000));
        let start = versions(&b);
        assert_eq!(start.0, start.1);
        map(&mut b, 0, 0);
        let mapped = versions(&b);
        assert_ne!(mapped, start);
        b.write(svd_regs::esp32c3::addr("EXTMEM", "ICACHE_CTRL"), Word, 1).unwrap();
        let enabled = versions(&b);
        assert_ne!(enabled, mapped);
        b.write(svd_regs::esp32c3::addr("EXTMEM", "ICACHE_CTRL1"), Word, 0).unwrap();
        assert_ne!(versions(&b), enabled);
        let sram = b.page_version(SRAM_IBUS_BASE);
        map(&mut b, 1, 1);
        assert_eq!(b.page_version(SRAM_IBUS_BASE), sram, "RAM versions are untouched");
    }
```

No import to add to the test module: `NorFlash` is imported there since Task 5, and the three `FLASH_*` constants reach it through `use super::*;` once Step 3 imports them in `bus.rs`.

The existing test `addresses_outside_every_region_are_unmapped` lists `0x3C00_0000` and `0x4200_0000` among the unmapped addresses; they are now the first bytes of the two windows. Replace these two entries by `FLASH_DBUS_BASE - 4` and `FLASH_IBUS_BASE - 4` (the word just below each window is still unmapped; the end of each window is covered by `access_straddling_the_end_of_the_window_is_unmapped`).

`machine.rs`, module `tests` — code remapped under the CPU must be re-decoded:

```rust
    #[test]
    fn code_executed_from_the_flash_window_follows_a_remap() {
        use cpu_rv32::asm::jalr;
        const A0: u8 = 10;
        const RA: u8 = 1;
        const T2: u8 = 7;
        const S0: u8 = 8;
        // Flash page 0: `li a0, 1; ret`. Flash page 1: `li a0, 2; ret`.
        let mut image = vec![0u8; 0x1_0008];
        for (page, value) in [(0usize, 1), (1, 2)] {
            for (i, word) in [addi(A0, 0, value), jalr(0, RA, 0)].into_iter().enumerate() {
                let at = page * 0x1_0000 + 4 * i;
                image[at..at + 4].copy_from_slice(&word.to_le_bytes());
            }
        }
        let program = [
            lui(T0, crate::periph::mmu::MMU_TABLE_BASE),
            lui(T2, crate::memmap::FLASH_IBUS_BASE),
            lui(S0, SRAM_DBUS_BASE),
            sw(0, T0, 0),          // entry 0 = flash page 0
            jalr(RA, T2, 0),       // a0 = 1
            sw(A0, S0, 0),
            addi(T1, 0, 1),
            sw(T1, T0, 0),         // entry 0 = flash page 1: same address, new code
            jalr(RA, T2, 0),       // a0 = 2 only if the decoded code was dropped
            sw(A0, S0, 4),
            HANG,
        ];
        let mut m = Esp32C3::with_flash(flat_rom(&program), image, &SocConfig::default()).unwrap();
        m.run_until(Cycles(400));
        assert_eq!(m.drain_diagnostics(), Vec::new());
        assert_eq!((bus_word(&mut m, SRAM_DBUS_BASE), bus_word(&mut m, SRAM_DBUS_BASE + 4)), (1, 2));
    }
```

- [ ] **Step 2: Run, see the failure**

Run: `cargo test -p soc-esp32c3 > /tmp/t7.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, `FLASH_DBUS_BASE` not found.

- [ ] **Step 3: Implement**

`memmap.rs`:

```rust
/// Flash cache window, data bus (`SOC_DRAM0_CACHE_ADDRESS_LOW`; report CM §1).
pub const FLASH_DBUS_BASE: u32 = 0x3C00_0000;
/// Flash cache window, instruction bus (`SOC_IRAM0_CACHE_ADDRESS_LOW`). The two windows share the
/// 128 MMU entries: entry `i` maps `base + i * 64 KiB` of both.
pub const FLASH_IBUS_BASE: u32 = 0x4200_0000;
/// 8 MiB each (128 pages of 64 KiB).
pub const FLASH_WINDOW_SIZE: u32 = 0x80_0000;
```

`bus.rs`. Add to `Region`:

```rust
    /// A flash window: the offset inside it, and whether it is the instruction-bus one.
    Flash { offset: u32, instruction_bus: bool },
```

and to `locate`, before the peripheral arm:

```rust
    } else if let Some(offset) = at(FLASH_DBUS_BASE, FLASH_WINDOW_SIZE) {
        Some(Region::Flash { offset, instruction_bus: false })
    } else if let Some(offset) = at(FLASH_IBUS_BASE, FLASH_WINDOW_SIZE) {
        Some(Region::Flash { offset, instruction_bus: true })
```

The window read:

```rust
    /// The flash offset one window byte maps to; a miss or a page past the chip is latched, reported, and yields `None`.
    /// `vaddr` is the address of the access the byte belongs to: the fault registers and the diagnostic name it.
    fn flash_offset(&mut self, vaddr: u32, window_offset: u32) -> Option<u32> {
        let (entry, content, fault) = match self.periph.mmu.translate(window_offset) {
            Ok(offset) if offset < self.periph.spi1.flash().size() => return Some(offset),
            Ok(offset) => (
                window_offset / MMU_PAGE_BYTES,
                offset / MMU_PAGE_BYTES,
                MappingFault::BeyondFlash,
            ),
            Err(MmuMiss { entry, content }) => (entry, content, MappingFault::InvalidEntry),
        };
        self.periph.extmem.latch_mmu_fault(vaddr, content);
        self.diag.record(Diagnostic::FlashMapping { vaddr, entry, content, fault, pc: self.pc });
        None
    }

    /// Reads the flash **live** through the MMU table (decision D4): no cache line is modelled.
    ///
    /// ASSUMPTION: (decisions D4, D23) an access through an invalid entry, or to a page past the end
    /// of the flash, returns 0. What silicon returns is not documented (report CM §6.1: "wrong
    /// data"); 0 makes a fetch an illegal instruction, as ESP-IDF's panic handler expects. The fault
    /// registers are latched and the access is reported; the SP1b-4 lockstep may refine the value.
    fn flash_read(&mut self, vaddr: u32, window_offset: u32, size: AccessSize) -> ReadOutcome {
        let len = size.bytes();
        let mut bytes = [0u8; 4];
        // Byte by byte: a misaligned access may straddle two 64 KiB pages, each with its own entry.
        for (index, byte) in (0u32..len).zip(bytes.iter_mut()) {
            if let Some(offset) = self.flash_offset(vaddr, window_offset + index) {
                self.periph.spi1.flash().read(offset, std::slice::from_mut(byte));
            }
        }
        ReadOutcome { value: u32::from_le_bytes(bytes) & size_mask(size), stall: 0 }
    }
```

`window_offset + index` cannot overflow: `locate` checked that the whole access lies inside the window (write `// INVARIANT:`). Every byte of one access reports the address of the access (`vaddr`), not its own: `CACHE_MMU_FAULT_VADDR` then holds the address the CPU used, which is what `invalid_entry_reads_zero_latches_the_fault_and_is_reported` checks after a 4-byte load. In `Bus::read`, add the arm `Some(Region::Flash { offset, .. }) => Ok(self.flash_read(addr, offset, size)),` (the existing `ASSUMPTION` comment about a fetch through a data-bus address also covers the data window). In `Bus::write`:

```rust
            // ASSUMPTION: (decision D4) a store to a flash window raises a store access fault, as the
            // QEMU oracle does (CM §6.2); silicon latches the status bit, the fault is not established.
            Some(Region::Flash { instruction_bus, .. }) => {
                self.periph.extmem.latch_store(instruction_bus);
                Err(BusError::Unmapped { addr, kind: AccessKind::Store })
            }
```

In `page_version`:

```rust
            // Flash content cannot change (the flash is read-only, D3); what an address shows changes
            // with the MMU table and with the cache enables. Both counters only grow, so their sum
            // changes whenever either does (decision D6).
            Some(Region::Flash { .. }) => self.periph.mmu.generation() + self.periph.extmem.generation(),
```

Imports: `crate::diag::MappingFault`, `crate::memmap::{FLASH_DBUS_BASE, FLASH_IBUS_BASE, FLASH_WINDOW_SIZE}`, `crate::periph::mmu::{MMU_PAGE_BYTES, MmuMiss}`. `page_version` now has an arm of its own for the windows: keep its last arm exhaustive (`Some(Region::Rom(_) | Region::Periph) | None => 0`). Update the module doc comment of `memmap.rs` ("Every value comes from … §A.1" → add "and `esp32c3-cache-mmu.md` §1 for the flash windows").

- [ ] **Step 4: Run**

Run: `cargo test --workspace > /tmp/t7.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.
Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/t7c.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 5: Prove the real-ROM tests are green again**

The commit of Task 6 left three ignored tests of `crates/soc-esp32c3/tests/real_rom.rs` red (`rev3_`, `rev0_` and `rev101_banner_is_reached_without_any_unmodeled_access_or_exception`): the ROM passed its cache polls and faulted on the unmapped window at `0x3C00_0000`. The windows exist now; this step is the proof, and the task is not finished without it.

Fetch the ROMs once: `cargo run --release -p emu-cli -- rom fetch --all --rom-dir /tmp/inkemu-rom > /tmp/t7fetch.log 2>&1; echo "exit=$?"` (expected `exit=0`; the files are downloaded and verified, never committed).
Run: `INKEMU_ROM_ELF_DIR=/tmp/inkemu-rom cargo test --release -p soc-esp32c3 --test real_rom -- --ignored > /tmp/t7rom.log 2>&1; echo "exit=$?"`
Expected: `exit=0`, and `grep -c "^test .* ok$" /tmp/t7rom.log` prints `9`: every test of the suite passes, the three strict-boot tests among them.

- [ ] **Step 6: Commit**

Before committing (Global Constraints): `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`: `exit=0` three times. The code blocks of this plan are not wrapped as `rustfmt` wraps them.

```bash
/usr/bin/git add crates/soc-esp32c3/src/memmap.rs crates/soc-esp32c3/src/bus.rs crates/soc-esp32c3/src/machine.rs
/usr/bin/git commit -m "feat(soc): flash windows that read the flash live through the MMU table

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3/src/memmap.rs crates/soc-esp32c3/src/bus.rs crates/soc-esp32c3/src/machine.rs
```

---

### Task 8: SHA accelerator (SHA-1, SHA-224, SHA-256, typical mode)

**Files:**
- Create: `crates/soc-esp32c3/src/periph/sha.rs`
- Modify: `crates/soc-esp32c3/Cargo.toml`, `Cargo.lock` (`sha1` enters it), `crates/soc-esp32c3/src/periph/mod.rs`

**Interfaces:**
- Consumes: `Unsupported::{ShaDma, ShaMode}` (Task 2); `Peripherals::take_unsupported` (Task 5); `svd_regs::esp32c3::addr`; the workspace dependencies `sha1 = "0.11"` and `sha2 = "0.11"` (Task 0), of which `sha2::block_api::compress256(state: &mut [u32; 8], blocks: &[[u8; 64]])` and `sha1::block_api::compress(state: &mut [u32; 5], blocks: &[[u8; 64]])` (no feature flag; both take the state as native-endian words and the blocks as byte arrays).
- Produces: `pub struct Sha` with `Sha::new()`, `impl Peripheral for Sha`, `pub(crate) fn take_unsupported(&mut self) -> Option<Unsupported>`; `Peripherals::sha: Sha` at `SHA_BASE = 0x6003_B000`.

Protocol (FB §B.7 and §F "Hashing"; BA §7): per 64-byte block, `MODE` ← algorithm, 16 word writes to `M_MEM0..15`, poll `BUSY` until it is not 1, then `START` ← 1 (first block: hash from the initial value) or `CONTINUE` ← 1 (hash from the state in `H_MEM`). The message bytes are the bytes of the `M_MEM` words in memory order, so SHA word `W[i]` is the byte-swapped register; the digest bytes are the bytes of the `H_MEM` words in memory order. `MODE`: 0 SHA-1, 1 SHA-224, 2 SHA-256 (2 is measured, FB §B.7; 0 and 1 are the order of the `SHA_TYPE` enum of ESP-IDF `components/esp_rom/esp32c3/include/esp32c3/rom/sha.h`, not exercised by the measured boots — `ASSUMPTION:` in the code). Initial values: FIPS 180-4 §5.3.

- [ ] **Step 1: Add the dependencies**

The root `Cargo.toml` does not change: Task 0 declared `sha1 = "0.11"` and `sha2 = "0.11"` in `[workspace.dependencies]` (decision D20), and the 0.11 compression functions need no feature. `crates/soc-esp32c3/Cargo.toml`: in `[dependencies]` add `sha1.workspace = true` and `sha2.workspace = true`; remove `sha2.workspace = true` from `[dev-dependencies]` (the dependency now covers the tests).

Run: `cargo deny check > /tmp/t8deny.log 2>&1; echo "exit=$?"`
Expected: `exit=0` (`sha1` is MIT OR Apache-2.0 and brings no dependency `sha2` does not already have).

- [ ] **Step 2: Write the failing tests** (`sha.rs`, module `tests`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;

    const MODE_SHA1: u32 = 0;
    const MODE_SHA224: u32 = 1;
    const MODE_SHA256: u32 = 2;

    /// Writes one 64-byte block the way the ROM's `memcpy` does: 16 little-endian words.
    fn write_block(sha: &mut Sha, block: &[u8]) {
        for (word, chunk) in (0u32..).zip(block.chunks(4)) {
            assert!(sha.write(M_MEM0 + 4 * word, u32::from_le_bytes(chunk.try_into().unwrap())));
        }
    }
    /// The padded message: 0x80, zeros, 64-bit big-endian bit length (what `ets_sha_finish` builds).
    fn padded(message: &[u8]) -> Vec<u8> {
        let mut bytes = message.to_vec();
        bytes.push(0x80);
        while bytes.len() % 64 != 56 {
            bytes.push(0);
        }
        bytes.extend((message.len() as u64 * 8).to_be_bytes());
        bytes
    }
    /// Hashes `message` through the registers and returns the first `len` digest bytes.
    fn hash(mode: u32, message: &[u8], len: usize) -> Vec<u8> {
        let mut sha = Sha::new();
        for (index, block) in padded(message).chunks(64).enumerate() {
            assert!(sha.write(MODE, mode));
            write_block(&mut sha, block);
            assert_eq!(sha.read(BUSY), Some(0), "the ROM loops while BUSY reads 1");
            assert!(sha.write(if index == 0 { START } else { CONTINUE }, 1));
        }
        assert_eq!(sha.take_unsupported(), None);
        (0..8u32).flat_map(|i| sha.read(H_MEM0 + 4 * i).unwrap().to_le_bytes()).take(len).collect()
    }

    #[test]
    fn sha256_of_abc_matches_the_fips_180_4_example() {
        // FIPS 180-4 example: SHA-256("abc") = ba7816bf 8f01cfea 414140de 5dae2223 b00361a3 96177a9c b410ff61 f20015ad.
        let mut sha = Sha::new();
        sha.write(MODE, MODE_SHA256);
        sha.write(M_MEM0, 0x8063_6261); // bytes 61 62 63 80
        sha.write(M_MEM0 + 4 * 15, 0x1800_0000); // bit length 24, big-endian, in the last word
        sha.write(START, 1);
        assert_eq!(sha.read(H_MEM0), Some(0xBF16_78BA), "digest bytes ba 78 16 bf in memory order");
        assert_eq!(sha.read(H_MEM0 + 4 * 7), Some(0xAD15_00F2));
    }

    #[test]
    fn digest_words_are_the_digest_bytes_in_memory_order() {
        // FB §F: H_MEM read as words gives the digest bytes in order (`0x1c59639a` for `9a 63 59 1c…`).
        let message = b"abc";
        assert_eq!(hash(MODE_SHA256, message, 32), sha2::Sha256::digest(message).to_vec());
    }

    #[test]
    fn the_three_algorithms_of_the_c3_agree_with_their_reference_over_several_blocks() {
        // 200 bytes: four blocks with the padding, so CONTINUE runs three times.
        let message: Vec<u8> = (0..200u8).collect();
        assert_eq!(hash(MODE_SHA256, &message, 32), sha2::Sha256::digest(&message).to_vec());
        assert_eq!(hash(MODE_SHA224, &message, 28), sha2::Sha224::digest(&message).to_vec());
        assert_eq!(hash(MODE_SHA1, &message, 20), sha1::Sha1::digest(&message).to_vec());
    }

    #[test]
    fn sha1_and_sha224_of_abc_match_their_published_digests() {
        // FIPS 180-4 examples: SHA-1 a9993e36 4706816a ba3e2571 7850c26c 9cd0d89d;
        // SHA-224 23097d22 3405d822 8642a477 bda255b3 2aadbce4 bda0b3f7 e36c9da7.
        assert_eq!(hash(MODE_SHA1, b"abc", 4), [0xa9, 0x99, 0x3e, 0x36]);
        assert_eq!(hash(MODE_SHA224, b"abc", 4), [0x23, 0x09, 0x7d, 0x22]);
    }

    #[test]
    fn continue_without_start_hashes_from_the_stored_state() {
        // The ROM restores a saved state by writing H_MEM, then CONTINUE (FB §B.7, note under the table).
        let message: Vec<u8> = (0..100u8).collect();
        let blocks = padded(&message);
        let mut first = Sha::new();
        first.write(MODE, MODE_SHA256);
        write_block(&mut first, &blocks[..64]);
        first.write(START, 1);
        let saved: Vec<u32> = (0..8u32).map(|i| first.read(H_MEM0 + 4 * i).unwrap()).collect();

        let mut resumed = Sha::new();
        resumed.write(MODE, MODE_SHA256);
        for (i, word) in (0u32..).zip(&saved) {
            assert!(resumed.write(H_MEM0 + 4 * i, *word));
        }
        write_block(&mut resumed, &blocks[64..128]);
        resumed.write(CONTINUE, 1);
        let digest: Vec<u8> = (0..8u32).flat_map(|i| resumed.read(H_MEM0 + 4 * i).unwrap().to_le_bytes()).collect();
        assert_eq!(digest, sha2::Sha256::digest(&message).to_vec());
    }

    #[test]
    fn a_start_or_continue_written_as_zero_does_nothing() {
        let mut sha = Sha::new();
        sha.write(MODE, MODE_SHA256);
        sha.write(START, 0);
        assert_eq!(sha.read(H_MEM0), Some(0));
    }

    #[test]
    fn unknown_mode_is_reported_and_leaves_the_state() {
        let mut sha = Sha::new();
        sha.write(H_MEM0, 0x1234_5678);
        sha.write(MODE, 5);
        assert!(sha.write(START, 1));
        assert_eq!(sha.take_unsupported(), Some(Unsupported::ShaMode { mode: 5 }));
        assert_eq!(sha.read(H_MEM0), Some(0x1234_5678), "no digest is presented as valid");
        assert_eq!(sha.read(MODE), Some(5));
    }

    #[test]
    fn dma_start_is_reported() {
        let mut sha = Sha::new();
        for name in ["DMA_START", "DMA_CONTINUE", "DMA_BLOCK_NUM"] {
            let offset = svd_regs::esp32c3::addr("SHA", name) - SHA_BASE;
            assert!(sha.write(offset, 1), "{name}");
            assert_eq!(sha.take_unsupported(), Some(Unsupported::ShaDma), "{name}");
        }
    }

    #[test]
    fn registers_outside_the_typical_mode_stay_unmodelled() {
        let mut sha = Sha::new();
        for name in ["T_STRING", "T_LENGTH", "CLEAR_IRQ", "IRQ_ENA", "DATE"] {
            let offset = svd_regs::esp32c3::addr("SHA", name) - SHA_BASE;
            assert_eq!(sha.read(offset), None, "{name}");
        }
        assert_eq!(sha.read(START), None, "START is write-only");
    }
}
```

- [ ] **Step 3: Run, see the failure**

Run: `cargo test -p soc-esp32c3 sha > /tmp/t8.log 2>&1; echo "exit=$?"`
Expected: `exit=101`.

- [ ] **Step 4: Implement `periph/sha.rs`**

```rust
//! SHA accelerator in typical (non-DMA) mode: SHA-1, SHA-224, SHA-256 — the three the ESP32-C3 has.
//! Register protocol: reports FB §B.7 and §F, BA §7 (ROM `ets_sha_process`, ESP-IDF `hwcrypto_reg.h`).
//! The compression functions come from the `sha1` and `sha2` crates (decisions D7 and D20); only the
//! register interface is ours.
use super::SHA_BASE;
use crate::diag::Unsupported;
use emu_core::Peripheral;
use svd_regs::esp32c3::addr;

const MODE: u32 = addr("SHA", "MODE") - SHA_BASE;
const START: u32 = addr("SHA", "START") - SHA_BASE;
const CONTINUE: u32 = addr("SHA", "CONTINUE") - SHA_BASE;
const BUSY: u32 = addr("SHA", "BUSY") - SHA_BASE;
const DMA_BLOCK_NUM: u32 = addr("SHA", "DMA_BLOCK_NUM") - SHA_BASE;
const DMA_START: u32 = addr("SHA", "DMA_START") - SHA_BASE;
const DMA_CONTINUE: u32 = addr("SHA", "DMA_CONTINUE") - SHA_BASE;
const H_MEM0: u32 = addr("SHA", "H_MEM0") - SHA_BASE;
const M_MEM0: u32 = addr("SHA", "M_MEM0") - SHA_BASE;
const STATE_WORDS: usize = 8;
const BLOCK_WORDS: usize = 16;
const BLOCK_BYTES: usize = 64;
const SHA1_WORDS: usize = 5;
// INVARIANT: the SHA-1 state is the first five words of the eight-word register state.
const _: () = assert!(SHA1_WORDS <= STATE_WORDS);

/// Initial hash values, FIPS 180-4 §5.3.1 (SHA-1), §5.3.2 (SHA-224), §5.3.3 (SHA-256).
const SHA1_INITIAL: [u32; SHA1_WORDS] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
const SHA224_INITIAL: [u32; STATE_WORDS] = [
    0xC105_9ED8, 0x367C_D507, 0x3070_DD17, 0xF70E_5939, 0xFFC0_0B31, 0x6858_1511, 0x64F9_8FA7, 0xBEFA_4FA4,
];
const SHA256_INITIAL: [u32; STATE_WORDS] = [
    0x6A09_E667, 0xBB67_AE85, 0x3C6E_F372, 0xA54F_F53A, 0x510E_527F, 0x9B05_688C, 0x1F83_D9AB, 0x5BE0_CD19,
];

/// `SHA.MODE`. 2 is measured (FB §B.7). ASSUMPTION: 0 and 1 follow the `SHA_TYPE` enum of ESP-IDF
/// `esp32c3/rom/sha.h` (`SHA1`, `SHA2_224`, `SHA2_256`); no measured boot uses them.
#[derive(Clone, Copy)]
enum Algorithm {
    Sha1,
    Sha224,
    Sha256,
}

impl Algorithm {
    const fn from_mode(mode: u32) -> Option<Algorithm> {
        match mode {
            0 => Some(Algorithm::Sha1),
            1 => Some(Algorithm::Sha224),
            2 => Some(Algorithm::Sha256),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Sha {
    mode: u32,
    /// `H_MEM0..7` as the registers read: each is a state word with its bytes swapped.
    state: [u32; STATE_WORDS],
    /// `M_MEM0..15` as written.
    block: [u32; BLOCK_WORDS],
    unsupported: Option<Unsupported>,
}

/// Index of `offset` in a block of `count` words starting at `first`.
fn word_index(offset: u32, first: u32, count: usize) -> Option<usize> {
    let index = usize::try_from(offset.checked_sub(first)? / 4).ok()?;
    (index < count).then_some(index)
}

impl Sha {
    #[must_use]
    pub fn new() -> Sha {
        Sha::default()
    }

    pub(crate) fn take_unsupported(&mut self) -> Option<Unsupported> {
        self.unsupported.take()
    }

    /// Compresses the block in `M_MEM`; `fresh` starts from the algorithm's initial value.
    fn compress(&mut self, fresh: bool) {
        let Some(algorithm) = Algorithm::from_mode(self.mode) else {
            self.unsupported = Some(Unsupported::ShaMode { mode: self.mode });
            return;
        };
        // The message bytes are the bytes of the M_MEM words in memory order (FB §F).
        let mut block = [0u8; BLOCK_BYTES];
        for (chunk, word) in block.as_chunks_mut::<4>().0.iter_mut().zip(self.block) {
            *chunk = word.to_le_bytes();
        }
        // The state words are the H_MEM registers with their bytes swapped (FB §F).
        let mut h = self.state.map(u32::swap_bytes);
        match algorithm {
            Algorithm::Sha1 => {
                let mut h1 = if fresh { SHA1_INITIAL } else { [h[0], h[1], h[2], h[3], h[4]] };
                sha1::block_api::compress(&mut h1, &[block]);
                h[..SHA1_WORDS].copy_from_slice(&h1);
            }
            Algorithm::Sha224 | Algorithm::Sha256 => {
                if fresh {
                    h = if matches!(algorithm, Algorithm::Sha224) { SHA224_INITIAL } else { SHA256_INITIAL };
                }
                sha2::block_api::compress256(&mut h, &[block]);
            }
        }
        self.state = h.map(u32::swap_bytes);
    }
}

impl Peripheral for Sha {
    fn read(&mut self, offset: u32) -> Option<u32> {
        if let Some(index) = word_index(offset, H_MEM0, STATE_WORDS) {
            return self.state.get(index).copied();
        }
        if let Some(index) = word_index(offset, M_MEM0, BLOCK_WORDS) {
            return self.block.get(index).copied();
        }
        match offset {
            MODE => Some(self.mode),
            // A block is hashed inside the write that triggers it: never busy (FB §C row 14).
            BUSY => Some(0),
            _ => None,
        }
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        if let Some(slot) = word_index(offset, H_MEM0, STATE_WORDS).and_then(|i| self.state.get_mut(i)) {
            *slot = value;
            return true;
        }
        if let Some(slot) = word_index(offset, M_MEM0, BLOCK_WORDS).and_then(|i| self.block.get_mut(i)) {
            *slot = value;
            return true;
        }
        match offset {
            MODE => self.mode = value,
            START | CONTINUE => {
                if value & 1 != 0 {
                    self.compress(offset == START);
                }
            }
            // Decision D7: DMA-SHA is not modelled; the write is reported, never silently dropped.
            DMA_BLOCK_NUM | DMA_START | DMA_CONTINUE => {
                self.unsupported = Some(Unsupported::ShaDma);
            }
            _ => return false,
        }
        true
    }
}
```

`h[..SHA1_WORDS]` and the five indices are in bounds because `SHA1_WORDS <= STATE_WORDS`, which the `const _` assertion next to the constants checks at build time. The block is a plain `[u8; 64]`: the 0.11 compression functions take `&[[u8; 64]]`, so no `GenericArray` is involved, and `as_chunks_mut::<4>()` is what clippy asks for in place of `chunks_exact_mut(4)` (`chunks_exact_to_as_chunks`).

`periph/mod.rs`: `pub mod sha;`, `pub use sha::Sha;`, `pub(crate) const SHA_BASE: u32 = 0x6003_B000;` and `const SHA: (u32, u32) = (SHA_BASE, 0x6003_C000);` (comment: "`DR_REG_SHA_BASE`; report BC §A.2"), field `pub sha: Sha`, an arm in `device`, and `take_unsupported` becomes `self.spi1.take_unsupported().or_else(|| self.sha.take_unsupported())`.

- [ ] **Step 5: Run**

Run: `cargo test --workspace > /tmp/t8.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.
Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/t8c.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 6: Commit**

Before committing (Global Constraints): `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`: `exit=0` three times. The code blocks of this plan are not wrapped as `rustfmt` wraps them.

```bash
/usr/bin/git add Cargo.lock crates/soc-esp32c3
/usr/bin/git commit -m "feat(soc): SHA accelerator in typical mode on the RustCrypto compression functions

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- Cargo.lock crates/soc-esp32c3
```

---

### Task 9: The real ROM boots from flash — synthetic bootloader images and failure paths

**Files:**
- Modify: `crates/soc-esp32c3/Cargo.toml` (dev-dependency `firmware` with `test-support`), `crates/soc-esp32c3/tests/real_rom.rs`

**Interfaces:**
- Consumes: `Esp32C3::{new, with_flash, run_until, take_console, drain_diagnostics, pc, cycles, trace_hash}`, `SocConfig`, `StopReason`, `Diagnostic` (Tasks 2–8); `firmware::builder::{ImageSpec, build_image}` (existing: `ImageSpec { chip: u16, entry: u32, flash_size_nibble: u8, spi_mode: u8, segments: Vec<(u32, Vec<u8>)>, hash_appended: bool }`, `build_image(&ImageSpec) -> Vec<u8>`); the helpers `elf(revision: u16)`, `banner(rom: &str, build: &str) -> String`, the constants `BOOT_LINE` and `SLICE` (100,000 cycles) and the import `sha2::{Digest, Sha256}` already in `real_rom.rs`; `common::hex` (`mod common;`, Task 0).
- Produces: tests only. They are `#[ignore]`d without `INKEMU_ROM_ELF_DIR` and run in the existing `rom-boot` CI job (it already runs `cargo test --release -p soc-esp32c3 --test real_rom -- --ignored`).

This task writes no model code: it is the first end-to-end proof that SPI1, the cache controller, the MMU, the flash window and the SHA accelerator satisfy the **unmodified** ROM (decision D28). Expected outputs: FB §G (lines), §F (checks), §H (failure paths). The synthetic bootloader is one segment holding `j .` at `0x4038_0000`: the address passes the ROM's range check `load_addr ≥ 0x3FCDF664` (FB §F step 5) exactly as the real bootloader's `0x403cbf10` does.

- [ ] **Step 1: Add the dev-dependency**

`crates/soc-esp32c3/Cargo.toml`, `[dev-dependencies]`: `firmware = { workspace = true, features = ["test-support"] }`.

- [ ] **Step 2: Write the tests** (append to `crates/soc-esp32c3/tests/real_rom.rs`)

```rust
use firmware::builder::{ImageSpec, build_image};

/// Where the synthetic bootloader is loaded and entered: the start of the SRAM seen from the instruction bus.
const STUB_ENTRY: u32 = 0x4038_0000;
/// `j .`
const STUB_CODE: [u8; 4] = [0x6f, 0, 0, 0];
/// Where the ROM parks after `ets_run_flash_bootloader` failed: the `j .` of `main` (FB §H, "hang").
const ROM_HANG_PC: u32 = 0x4004_7ed2;
/// The ROM path from the banner to the bootloader entry is about 121,000 instructions (FB §1 item 1).
const FLASH_BOOT_BUDGET: u64 = 40_000_000;
const INVALID_HEADER_BLANK: &str = "invalid header: 0xffffffff\n";

fn stub_spec() -> ImageSpec {
    ImageSpec {
        chip: 5,
        entry: STUB_ENTRY,
        flash_size_nibble: 4,
        spi_mode: 2,
        segments: vec![(STUB_ENTRY, STUB_CODE.to_vec())],
        hash_appended: true,
    }
}

struct FlashBoot {
    /// UART0 output after the three banner lines, carriage returns dropped.
    after_banner: String,
    diagnostics: Vec<Diagnostic>,
    stop: StopReason,
    pc: u32,
    trace_hash: Option<u64>,
}

/// Boots the rev3 ROM on `flash` (`None`: an erased chip) until `done` accepts the text printed after
/// the banner, then one more slice so the ROM reaches the place it stops at.
fn flash_boot(flash: Option<Vec<u8>>, config: SocConfig, done: impl Fn(&str) -> bool) -> FlashBoot {
    let rom = RomImage::from_elf(&elf(3)).unwrap();
    let mut machine = match flash {
        Some(image) => Esp32C3::with_flash(rom, image, &config).unwrap(),
        None => Esp32C3::new(rom, &config),
    };
    let banner_text = banner("esp32c3-api1-20210207", "Feb  7 2021");
    let (mut uart0, mut diagnostics) = (Vec::new(), Vec::new());
    let mut stop = StopReason::CycleBudget;
    let mut extra_slices = 1;
    let after = |bytes: &[u8]| {
        let text = String::from_utf8_lossy(bytes).replace('\r', "");
        text.strip_prefix(&banner_text).map(str::to_owned).unwrap_or_default()
    };
    while stop == StopReason::CycleBudget && machine.cycles().get() < FLASH_BOOT_BUDGET {
        if done(&after(&uart0)) {
            if extra_slices == 0 {
                break;
            }
            extra_slices -= 1;
        }
        stop = machine.run_until(Cycles(machine.cycles().get() + SLICE));
        uart0.extend(machine.take_console(ConsoleChannel::Uart0));
        let _ = machine.take_console(ConsoleChannel::UsbSerialJtag);
        diagnostics.extend(machine.drain_diagnostics());
    }
    FlashBoot {
        after_banner: after(&uart0),
        diagnostics,
        stop,
        pc: machine.pc(),
        trace_hash: machine.trace_hash(),
    }
}

fn strict() -> SocConfig {
    SocConfig { strict_mmio: true, ..SocConfig::default() }
}

/// The hexadecimal number that follows `label` in `text`.
fn hex_after(text: &str, label: &str) -> u8 {
    let rest = &text[text.find(label).unwrap_or_else(|| panic!("no `{label}` in {text:?}")) + label.len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
    u8::from_str_radix(&digits, 16).unwrap()
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn rom_loads_a_synthetic_bootloader_and_jumps_to_its_entry() {
    let boot = flash_boot(Some(build_image(&stub_spec())), strict(), |text| text.contains("entry 0x"));
    assert_eq!(
        boot.after_banner,
        "SPIWP:0xee\nmode:DIO, clock div:1\nload:0x40380000,len:0x4\nentry 0x40380000\n",
        "FB §G; no `SHA-256 comparison failed` line: the hardware digest equals the appended one"
    );
    assert_eq!(boot.diagnostics, Vec::new(), "nothing unmodelled, no exception, on the whole flash path");
    assert_eq!(boot.stop, StopReason::CycleBudget);
    assert_eq!(boot.pc, STUB_ENTRY, "the ROM jumped into the image");
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn blank_flash_prints_invalid_header_forever() {
    // FB §H row 1: a tight loop, no SPIWP line, never exits.
    let boot = flash_boot(None, strict(), |text| text.matches(INVALID_HEADER_BLANK).count() >= 3);
    assert!(boot.after_banner.matches(INVALID_HEADER_BLANK).count() >= 3, "{:?}", boot.after_banner);
    assert!(
        boot.after_banner.lines().all(|line| line == INVALID_HEADER_BLANK.trim_end() || INVALID_HEADER_BLANK.starts_with(line)),
        "only that line, the last one possibly cut by the slice: {:?}",
        boot.after_banner
    );
    assert_eq!(boot.diagnostics, Vec::new());
    assert_eq!(boot.stop, StopReason::CycleBudget, "a loop, not a stop");
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_header_that_is_not_an_image_is_printed_byte_by_byte() {
    // FB §H rows 2 and 3: the four header bytes are printed as hdr[3] hdr[2] hdr[1] hdr[0].
    let zeros = flash_boot(Some(vec![0u8; 16]), strict(), |text| text.contains('\n'));
    assert!(zeros.after_banner.starts_with("invalid header: 0x00000000\n"), "{:?}", zeros.after_banner);
    let mut image = build_image(&stub_spec());
    image[0] = 0x00;
    let no_magic = flash_boot(Some(image), strict(), |text| text.contains('\n'));
    assert!(no_magic.after_banner.starts_with("invalid header: 0x4f020100\n"), "{:?}", no_magic.after_banner);
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn an_image_for_another_chip_stops_the_rom_before_it_configures_the_flash() {
    // FB §H: chip id 9 -> one message, `ets_main.c 333 `, then `j .`; the check comes before SPIWP.
    let image = build_image(&ImageSpec { chip: 9, ..stub_spec() });
    let boot = flash_boot(Some(image), strict(), |text| text.contains("ets_main.c 333 \n"));
    assert_eq!(
        boot.after_banner,
        "Invalid chip id. Expected 5 read 9. Bootloader for wrong chip?\nets_main.c 333 \n"
    );
    assert_eq!(boot.pc, ROM_HANG_PC);
    assert_eq!(boot.diagnostics, Vec::new());
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_flipped_data_bit_is_a_checksum_failure() {
    // FB §H: detected after the `load:` line; the ROM prints both checksums, then hangs.
    const FIRST_DATA_BYTE: usize = 24 + 8; // image header, segment header
    let mut image = build_image(&stub_spec());
    image[FIRST_DATA_BYTE] ^= 0x01;
    let stored = image[image.len() - 32 - 1];
    let boot = flash_boot(Some(image), strict(), |text| text.contains("ets_main.c 333 \n"));
    assert!(boot.after_banner.contains("load:0x40380000,len:0x4\nChecksum failure. Calculated 0x"), "{:?}", boot.after_banner);
    assert_eq!(hex_after(&boot.after_banner, "stored 0x"), stored);
    assert_eq!(hex_after(&boot.after_banner, "Calculated 0x"), stored ^ 0x01);
    assert!(!boot.after_banner.contains("entry"));
    assert_eq!(boot.pc, ROM_HANG_PC);
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn a_wrong_digest_is_reported_and_the_rom_boots_anyway() {
    // FB §F step 7 and §H: a SHA mismatch is not fatal without secure boot. The digest the ROM prints
    // as "Calculated" comes out of the SHA accelerator model.
    let mut image = build_image(&stub_spec());
    let hashed = image.len() - 32;
    let calculated = common::hex(&Sha256::digest(&image[..hashed]));
    let last = image.len() - 1;
    image[last] ^= 0xFF;
    let expected = common::hex(&image[hashed..]);
    let boot = flash_boot(Some(image), strict(), |text| text.contains("entry 0x"));
    let text = boot.after_banner.to_lowercase();
    assert!(
        text.contains(&format!(
            "sha-256 comparison failed:\ncalculated: {calculated}\nexpected: {expected}\nattempting to boot anyway...\nentry 0x40380000\n"
        )),
        "{:?}",
        boot.after_banner
    );
    assert_eq!(boot.pc, STUB_ENTRY);
    assert_eq!(boot.diagnostics, Vec::new());
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn two_flash_boots_are_identical_down_to_the_trace_hash() {
    let config = SocConfig { trace_hash: true, ..SocConfig::default() };
    let run = || flash_boot(Some(build_image(&stub_spec())), config, |text| text.contains("entry 0x"));
    let (first, second) = (run(), run());
    assert!(first.trace_hash.is_some());
    assert_eq!(first.trace_hash, second.trace_hash);
    assert_eq!(first.after_banner, second.after_banner);
}
```

`0x4f020100` in the third test: `build_image` writes segment count 1, SPI mode 2 and `0x4F` (16 MB, 80 MHz) in header bytes 1–3, and byte 0 was zeroed.

- [ ] **Step 3: Run them against the real ROM**

Fetch the ROMs once: `cargo run --release -p emu-cli -- rom fetch --all --rom-dir /tmp/inkemu-rom > /tmp/t9fetch.log 2>&1; echo "exit=$?"` (expected `exit=0`).
Run: `INKEMU_ROM_ELF_DIR=/tmp/inkemu-rom cargo test --release -p soc-esp32c3 --test real_rom -- --ignored > /tmp/t9.log 2>&1; echo "exit=$?"`
Expected: `exit=0`, the seven new tests and the existing ones pass.

If a test fails with a diagnostic in `boot.diagnostics`, the line names what the ROM touched. Compare it with FB §B (every access of this path, in order) and §J (model per block):
- the register is in FB §B and missing from the model that Tasks 5–8 built → that task has a bug: fix the model **and add the missing unit test to that model's test module**, in this commit;
- the register is not in FB §B → the emulated ROM left the measured path. Do not add a register to make the test pass: report the diagnostic lines and the last 20 lines of `boot.after_banner` to the plan owner.

If the expected text differs only in a `Checksum failure`/`SHA-256` wording, print `boot.after_banner` and compare with FB §H: the strings there are measured, the test must match them.

- [ ] **Step 4: Run the whole workspace**

Run: `cargo test --workspace > /tmp/t9w.log 2>&1; echo "exit=$?"`
Expected: `exit=0` (the new tests are ignored there).

- [ ] **Step 5: Commit**

Before committing (Global Constraints): `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`: `exit=0` three times. The code blocks of this plan are not wrapped as `rustfmt` wraps them.

```bash
/usr/bin/git add crates/soc-esp32c3/Cargo.toml crates/soc-esp32c3/tests/real_rom.rs Cargo.lock
/usr/bin/git commit -m "test(soc): boot the real ROM from flash with synthetic bootloader images

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3/Cargo.toml crates/soc-esp32c3/tests/real_rom.rs Cargo.lock
```

---

### Task 10: Clock tree — CPU frequency from the clock registers, and device time

**Files:**
- Modify: `crates/emu-core/src/time.rs`, `crates/emu-core/src/lib.rs` (re-export `VirtualClock`)
- Create: `crates/soc-esp32c3/src/clock.rs`
- Modify: `crates/soc-esp32c3/src/lib.rs`, `crates/soc-esp32c3/src/periph/system.rs`, `crates/soc-esp32c3/src/machine.rs`, `crates/emu-cli/src/run.rs`
- Modify: `docs/registers-accepted-without-effect.md`

**Interfaces:**
- Consumes: `Frequency`, `Cycles` (`emu-core`); `System` (`periph/system.rs`: field `sysclk_writable`, its `ACCEPTED` list and storage from Tasks 2 and 6); `Unsupported::ClockSource`, `Diagnostic::Unsupported` (Task 2); `Hart::cycles`.
- Produces:
  ```rust
  // emu-core/src/time.rs
  impl Frequency { pub const fn new(hz: NonZeroU64) -> Frequency; }
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct VirtualClock { /* private */ }
  impl VirtualClock {
      pub const fn new(frequency: Frequency) -> VirtualClock;
      pub const fn frequency(&self) -> Frequency;
      /// From cycle `now` on, cycles last `1 / frequency`.
      pub fn set_frequency(&mut self, now: Cycles, frequency: Frequency);
      /// Nanoseconds elapsed when the cycle counter reads `now`.
      pub fn nanos_at(&self, now: Cycles) -> u64;
  }
  // soc-esp32c3/src/clock.rs
  pub const XTAL: Frequency;                 // 40 MHz
  pub fn cpu_frequency(sysclk_conf: u32, cpu_per_conf: u32) -> Option<Frequency>;
  // periph/system.rs
  impl System {
      pub fn clock_registers(&self) -> (u32, u32);              // (SYSCLK_CONF, CPU_PER_CONF) as software reads them
      pub(crate) fn take_clock_change(&mut self) -> bool;
  }
  // machine.rs
  impl Esp32C3 { pub const fn cpu_frequency(&self) -> Frequency; pub fn device_time_nanos(&self) -> u64; }
  ```

Facts (BA §5, BC §E.1): `SYSCLK_CONF` [9:0] `PRE_DIV_CNT` (reset 1), [11:10] `SOC_CLK_SEL` (0 crystal, 1 PLL), [18:12] `CLK_XTAL_FREQ` = 40; `CPU_PER_CONF` [1:0] `CPUPERIOD_SEL` (0 → 80 MHz, 1 → 160 MHz), reset `0xC`. Crystal source: CPU = 40 MHz ÷ (`PRE_DIV_CNT` + 1) (ESP-IDF `clk_ll_cpu_get_divider`). Bootloader: `PRE_DIV_CNT` ← 0, then `SOC_CLK_SEL` ← 1 with `CPUPERIOD_SEL` = 0 → 80 MHz (BA §5 step 11). Application: `CPUPERIOD_SEL` ← 1 → 160 MHz (BA §5, last paragraph). `SOC_CLK_SEL` = 3, or PLL with `CPUPERIOD_SEL` ≥ 2, makes ESP-IDF abort (BA §2.2, SYSTEM clock row).

- [ ] **Step 1: Write the failing tests**

`emu-core/src/time.rs`, module `tests`:

```rust
    fn mhz(value: u64) -> Frequency {
        Frequency::from_hz(value * 1_000_000).unwrap()
    }

    #[test]
    fn a_virtual_clock_adds_up_time_across_frequency_switches() {
        let mut clock = VirtualClock::new(mhz(20));
        assert_eq!(clock.nanos_at(Cycles(20_000_000)), 1_000_000_000, "20 M cycles at 20 MHz");
        clock.set_frequency(Cycles(20_000_000), mhz(80));
        assert_eq!(clock.frequency(), mhz(80));
        assert_eq!(clock.nanos_at(Cycles(100_000_000)), 2_000_000_000, "+80 M cycles at 80 MHz");
        clock.set_frequency(Cycles(100_000_000), mhz(160));
        assert_eq!(clock.nanos_at(Cycles(260_000_000)), 3_000_000_000, "+160 M cycles at 160 MHz");
    }

    #[test]
    fn a_cycle_that_is_not_a_whole_nanosecond_does_not_drift() {
        // 6.25 ns per cycle at 160 MHz: switching on an odd cycle must not lose the fraction.
        let mut clock = VirtualClock::new(mhz(160));
        clock.set_frequency(Cycles(1), mhz(160));
        clock.set_frequency(Cycles(3), mhz(160));
        assert_eq!(clock.nanos_at(Cycles(4)), 25);
        // Asked about a cycle before the last switch (cycle 3, 18.75 ns), the clock answers the time
        // of that switch: it keeps no history, and it never goes back past it.
        assert_eq!(clock.nanos_at(Cycles(1)), 18, "time does not run backwards before the last switch");
    }

    #[test]
    fn a_virtual_clock_saturates_instead_of_overflowing() {
        let clock = VirtualClock::new(Frequency::from_hz(1).unwrap());
        assert_eq!(clock.nanos_at(Cycles(u64::MAX)), u64::MAX);
    }

    #[test]
    fn a_frequency_can_be_a_constant() {
        const ONE: Frequency = Frequency::new(std::num::NonZeroU64::MIN);
        assert_eq!(ONE.hz(), 1);
    }
```

`soc-esp32c3/src/clock.rs`, module `tests`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const CPU_PER_RESET: u32 = 0xC;
    fn hz(sysclk_conf: u32, cpu_per_conf: u32) -> Option<u64> {
        cpu_frequency(sysclk_conf, cpu_per_conf).map(|f| f.hz())
    }

    #[test]
    fn the_reset_state_runs_the_cpu_from_the_crystal_divided_by_two() {
        // Decision D21: PRE_DIV_CNT = 1 at reset, divider = PRE_DIV_CNT + 1.
        assert_eq!(hz(0x2_8001, CPU_PER_RESET), Some(20_000_000));
    }

    #[test]
    fn the_bootloader_sequence_ends_at_80_mhz_and_the_application_at_160() {
        assert_eq!(hz(0x2_8000, CPU_PER_RESET), Some(40_000_000), "PRE_DIV_CNT <- 0");
        assert_eq!(hz(0x2_8400, CPU_PER_RESET), Some(80_000_000), "SOC_CLK_SEL <- 1 (BA §5 step 11)");
        assert_eq!(hz(0x2_8400, CPU_PER_RESET | 1), Some(160_000_000), "CPUPERIOD_SEL <- 1");
        assert_eq!(hz(0x2_8400, 0x1), Some(160_000_000), "the PLL at 320 MHz gives the same CPU clocks");
    }

    #[test]
    fn the_crystal_divider_covers_its_whole_field() {
        assert_eq!(hz(0x2_83FF, CPU_PER_RESET), Some(40_000_000 / 1024));
    }

    #[test]
    fn selections_without_a_known_frequency_are_refused() {
        assert_eq!(hz(0x2_8800, CPU_PER_RESET), None, "SOC_CLK_SEL = 2");
        assert_eq!(hz(0x2_8C00, CPU_PER_RESET), None, "SOC_CLK_SEL = 3: ESP-IDF aborts");
        assert_eq!(hz(0x2_8400, CPU_PER_RESET | 2), None, "PLL with CPUPERIOD_SEL >= 2");
    }
}
```

`periph/system.rs`, module `tests` (also change the last assertion of `clock_gates_and_resets_are_storage_from_their_reset_values`, which expected `sys.read(0x08) == None`, into `Some(0xC)`):

```rust
    #[test]
    fn clock_writes_are_flagged_once_and_the_registers_read_as_software_sees_them() {
        let mut sys = System::new();
        assert_eq!(sys.clock_registers(), (0x2_8001, 0xC), "reset: BA §5");
        assert!(!sys.take_clock_change());
        let cpu_per_conf = svd_regs::esp32c3::addr("SYSTEM", "CPU_PER_CONF") - SYSTEM_BASE;
        assert!(sys.write(cpu_per_conf, 0xD));
        assert!(sys.take_clock_change());
        assert!(!sys.take_clock_change(), "taken once");
        sys.write(SYSCLK_CONF, 0x400);
        assert!(sys.take_clock_change());
        assert_eq!(sys.clock_registers(), (0x2_8400, 0xD));
        sys.write(svd_regs::esp32c3::addr("SYSTEM", "PERIP_CLK_EN0") - SYSTEM_BASE, 0);
        assert!(!sys.take_clock_change(), "a clock gate is not the CPU clock");
    }
```

`machine.rs`, module `tests`:

```rust
    /// Three instructions that store `value` (a 12-bit signed immediate) at `addr`.
    fn store(addr: u32, value: i32) -> [u32; 3] {
        let page = addr.wrapping_add(0x800) & 0xFFFF_F000;
        [lui(T0, page), addi(T1, 0, value), sw(T1, T0, addr.wrapping_sub(page).cast_signed())]
    }
    const SYSCLK_CONF: u32 = svd_regs::esp32c3::addr("SYSTEM", "SYSCLK_CONF");
    const CPU_PER_CONF: u32 = svd_regs::esp32c3::addr("SYSTEM", "CPU_PER_CONF");

    #[test]
    fn device_time_follows_the_cpu_clock_the_firmware_selects() {
        let mut program = Vec::new();
        program.extend(store(SYSCLK_CONF, 0x400)); // PLL, CPUPERIOD_SEL = 0: 80 MHz
        program.extend(store(CPU_PER_CONF, 0xD)); // CPUPERIOD_SEL = 1: 160 MHz
        program.push(HANG);
        let mut m = machine(&program);
        assert_eq!(m.cpu_frequency().hz(), 20_000_000, "decision D21");
        assert_eq!(m.device_time_nanos(), 0);
        m.run_until(Cycles(1_000));
        assert_eq!(m.cpu_frequency().hz(), 160_000_000);
        assert_eq!(m.drain_diagnostics(), Vec::new());
        // Fewer than 20 cycles ran before the switch to 160 MHz, each of at most 50 ns.
        let nanos = m.device_time_nanos();
        let all_fast = m.cycles().get() * 1_000 / 160;
        assert!(nanos >= all_fast && nanos <= all_fast + 20 * 50, "{nanos} vs {all_fast}");
    }

    #[test]
    fn device_time_at_the_reset_clock_is_50_ns_per_cycle() {
        let mut m = machine(&[HANG]);
        m.run_until(Cycles(2_000));
        assert_eq!(m.device_time_nanos(), m.cycles().get() * 50);
    }

    #[test]
    fn a_clock_selection_without_a_known_frequency_is_reported_and_keeps_the_previous_one() {
        // -1024 = 0x...C00: SOC_CLK_SEL = 3 once masked to the 12 writable bits.
        let mut program = store(SYSCLK_CONF, -1024).to_vec();
        program.push(HANG);
        let mut m = machine_with(&program, SocConfig { strict_mmio: true, ..SocConfig::default() });
        let stop = m.run_until(Cycles(1_000));
        let StopReason::StrictMmio(Diagnostic::Unsupported { what, .. }) = &stop else {
            panic!("expected a strict stop, got {stop:?}");
        };
        assert_eq!(*what, crate::diag::Unsupported::ClockSource { sysclk_conf: 0x2_8C00, cpu_per_conf: 0xC });
        assert_eq!(m.cpu_frequency().hz(), 20_000_000);
    }
```

`emu-cli/src/run.rs`, module `tests` — add to `max_cycles_ends_the_run_cleanly`, after the existing assertion on `log` (at that point `log` is already the `String` the test made with `let log = text(&log);`):

```rust
        assert!(
            log.contains("us of device time") && log.contains("CPU clock at 20 MHz"),
            "{log}"
        );
```

- [ ] **Step 2: Run, see the failure**

Run: `cargo test --workspace > /tmp/t10.log 2>&1; echo "exit=$?"`
Expected: `exit=101`.

- [ ] **Step 3: Implement `VirtualClock`** (`emu-core/src/time.rs`)

```rust
const ATTOS_PER_SECOND: u128 = 1_000_000_000_000_000_000;
const ATTOS_PER_NANO: u128 = 1_000_000_000;

impl Frequency {
    /// A frequency known at compile time.
    #[must_use]
    pub const fn new(hz: NonZeroU64) -> Frequency {
        Frequency(hz)
    }
}

/// Converts the cycle counter of a CPU whose clock frequency changes into elapsed time.
///
/// Time is kept in attoseconds, so a cycle that is not a whole number of nanoseconds (6.25 ns at
/// 160 MHz) accumulates without drift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualClock {
    frequency: Frequency,
    /// Cycle counter at the last frequency change.
    since: Cycles,
    /// Time elapsed at `since`.
    elapsed_attos: u128,
}

impl VirtualClock {
    #[must_use]
    pub const fn new(frequency: Frequency) -> VirtualClock {
        VirtualClock { frequency, since: Cycles::ZERO, elapsed_attos: 0 }
    }

    #[must_use]
    pub const fn frequency(&self) -> Frequency {
        self.frequency
    }

    fn attos_at(&self, now: Cycles) -> u128 {
        // `u64::MAX` cycles times 1e18 stays below `u128::MAX` (1.8e37 < 3.4e38).
        let cycles = u128::from(now.0.saturating_sub(self.since.0));
        self.elapsed_attos.saturating_add(cycles * ATTOS_PER_SECOND / u128::from(self.frequency.hz()))
    }

    /// From cycle `now` on, a cycle lasts `1 / frequency`.
    pub fn set_frequency(&mut self, now: Cycles, frequency: Frequency) {
        self.elapsed_attos = self.attos_at(now);
        self.since = now;
        self.frequency = frequency;
    }

    /// Nanoseconds elapsed when the cycle counter reads `now`, saturating at `u64::MAX`.
    #[must_use]
    pub fn nanos_at(&self, now: Cycles) -> u64 {
        u64::try_from(self.attos_at(now) / ATTOS_PER_NANO).unwrap_or(u64::MAX)
    }
}
```

`emu-core/src/lib.rs`: `pub use time::{Cycles, Frequency, VirtualClock};`.

- [ ] **Step 4: Implement `clock.rs`** and declare it (`pub mod clock;` in `lib.rs`)

```rust
//! The CPU clock tree, reduced to what software can select. Sources: reports BA §5 and BC §E.1
//! (ESP-IDF `system_reg.h`, `clk_tree_ll.h`).
use emu_core::Frequency;
use std::num::NonZeroU64;

const fn megahertz(mhz: u64) -> Frequency {
    match NonZeroU64::new(mhz * 1_000_000) {
        Some(hz) => Frequency::new(hz),
        // INVARIANT: only called below with non-zero literals, at compile time.
        None => panic!("a clock frequency is never zero"),
    }
}

/// The Xteink boards carry a 40 MHz crystal (`SYSCLK_CONF.CLK_XTAL_FREQ` reads 40).
pub const XTAL: Frequency = megahertz(40);
const PLL_CPU_80: Frequency = megahertz(80);
const PLL_CPU_160: Frequency = megahertz(160);

/// `SYSCLK_CONF` [9:0] `PRE_DIV_CNT`, [11:10] `SOC_CLK_SEL`.
const PRE_DIV_MASK: u32 = 0x3FF;
const SOC_CLK_SEL_SHIFT: u32 = 10;
const SOC_CLK_SEL_MASK: u32 = 0x3;
const SOC_CLK_XTAL: u32 = 0;
const SOC_CLK_PLL: u32 = 1;
/// `CPU_PER_CONF` [1:0] `CPUPERIOD_SEL`: 0 is 80 MHz, 1 is 160 MHz, whichever PLL frequency is selected.
const CPUPERIOD_SEL_MASK: u32 = 0x3;

/// The CPU clock frequency the two clock registers select, or `None` for a selection this model
/// has no frequency for (the RC oscillator, or a value ESP-IDF itself aborts on: BA §2.2).
///
/// ASSUMPTION: (decision D21) with the crystal as source the CPU runs at 40 MHz / (`PRE_DIV_CNT` + 1),
/// the formula of ESP-IDF's `clk_ll_cpu_get_divider`, hence **20 MHz at reset** (`PRE_DIV_CNT` = 1).
/// The mask ROM assumes 40 MHz (`ets_get_xtal_div` returns 1 unless `CLK_DIV_EN` is set): which one
/// silicon does is not established (BA §10 item 1). The firmware-visible cycle counter is the same
/// either way; only this conversion to time depends on it. A timestamped log of a real unit, or the
/// SP8 calibration bench, settles it.
#[must_use]
pub fn cpu_frequency(sysclk_conf: u32, cpu_per_conf: u32) -> Option<Frequency> {
    match (sysclk_conf >> SOC_CLK_SEL_SHIFT) & SOC_CLK_SEL_MASK {
        SOC_CLK_XTAL => {
            let divider = u64::from(sysclk_conf & PRE_DIV_MASK) + 1;
            Frequency::from_hz(XTAL.hz() / divider)
        }
        SOC_CLK_PLL => match cpu_per_conf & CPUPERIOD_SEL_MASK {
            0 => Some(PLL_CPU_80),
            1 => Some(PLL_CPU_160),
            _ => None,
        },
        _ => None,
    }
}
```

- [ ] **Step 5: `System`: the clock registers and their change flag**

In `periph/system.rs`: rename the field `gates` to `registers`; append to `ACCEPTED` (length 9) the two rows

```rust
    Accepted::svd("SYSTEM", "CPU_PER_CONF", "PLL_FREQ_SEL and the CPU wait-mode bits (CPUPERIOD_SEL is used by the clock model)", "SP8"),
    Accepted::svd("SYSTEM", "MEM_PD_MASK", "memory power-down mask in light sleep", "SP1b-3"),
```

add `const CPU_PER_CONF: u32 = svd_regs::esp32c3::addr("SYSTEM", "CPU_PER_CONF") - SYSTEM_BASE;`, replace `const SYSCLK_CONF: u32 = 0x58;` by its `addr(..) - SYSTEM_BASE` form, add the field `clock_changed: bool` (false), set it in `write` when `offset == SYSCLK_CONF` or `offset == CPU_PER_CONF` (for `CPU_PER_CONF`, set the flag then fall through to the storage write), and add:

```rust
    /// `SYSCLK_CONF` and `CPU_PER_CONF` as software reads them: the inputs of [`crate::clock::cpu_frequency`].
    #[must_use]
    pub fn clock_registers(&self) -> (u32, u32) {
        (
            self.sysclk_writable | (XTAL_FREQ_MHZ << XTAL_FREQ_SHIFT),
            self.registers.read(CPU_PER_CONF).unwrap_or(0),
        )
    }

    /// Whether a clock register was written since the last call.
    pub(crate) fn take_clock_change(&mut self) -> bool {
        std::mem::take(&mut self.clock_changed)
    }
```

Update the module doc comment: "The CPU clock selection is read by `crate::clock`; gates and resets are stored."

- [ ] **Step 6: The machine keeps device time**

`machine.rs`: add the field `clock: VirtualClock`. `build` creates the bus inside the struct literal today: bind it to a local first (`let bus = SocBus::new(rom, Box::new(flash), config);`, then `bus,` in the literal), and right after that binding:

```rust
        let (sysclk_conf, cpu_per_conf) = bus.periph.system.clock_registers();
        // The reset values select the crystal, for which a frequency always exists.
        let frequency = clock::cpu_frequency(sysclk_conf, cpu_per_conf).unwrap_or(clock::XTAL);
```

and `clock: VirtualClock::new(frequency)`. In `run_until`, after the `match` on the step outcome and before the trace hash:

```rust
            if self.bus.periph.system.take_clock_change() {
                self.retune(pc);
            }
```

```rust
    /// Applies a write to the clock registers: from the end of the instruction that wrote them,
    /// cycles last what the new selection says.
    fn retune(&mut self, pc: u32) {
        let (sysclk_conf, cpu_per_conf) = self.bus.periph.system.clock_registers();
        match clock::cpu_frequency(sysclk_conf, cpu_per_conf) {
            Some(frequency) => self.clock.set_frequency(self.hart.cycles, frequency),
            // Decision D22: the previous frequency stays, and the selection is reported.
            None => self.bus.diag.record(Diagnostic::Unsupported {
                what: Unsupported::ClockSource { sysclk_conf, cpu_per_conf },
                pc,
            }),
        }
    }

    /// The CPU clock frequency the firmware selected.
    #[must_use]
    pub const fn cpu_frequency(&self) -> Frequency {
        self.clock.frequency()
    }

    /// Time elapsed on the device since reset, following every CPU clock switch.
    #[must_use]
    pub fn device_time_nanos(&self) -> u64 {
        self.clock.nanos_at(self.hart.cycles)
    }
```

Imports: `crate::clock`, `crate::diag::Unsupported`, `emu_core::{Frequency, VirtualClock}`.

- [ ] **Step 7: The CLI says how much device time elapsed** (`emu-cli/src/run.rs`, in `execute`, right after the `stopped at pc` line)

```rust
    let _ = writeln!(
        log,
        "inkemu: {} us of device time; CPU clock at {} MHz",
        machine.device_time_nanos() / 1_000,
        machine.cpu_frequency().hz() / 1_000_000
    );
```

- [ ] **Step 8: Regenerate the documented table and run**

Run: `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect > /tmp/t10doc.log 2>&1; echo "exit=$?"`
Expected: `exit=0`: the lists changed in this task, so the test rewrote the table of `docs/registers-accepted-without-effect.md` (run without the variable it fails and names this command; it never writes then).
Check `/usr/bin/git diff -- docs/registers-accepted-without-effect.md`: the diff touches table rows only, nothing above the `<!-- generated table -->` line, and `grep -c '^| .0x' docs/registers-accepted-without-effect.md` prints `70`. The file is part of this task's commit. Then:
Run: `cargo test --workspace > /tmp/t10.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.
Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/t10c.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 9: Commit**

Before committing (Global Constraints): `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`: `exit=0` three times. The code blocks of this plan are not wrapped as `rustfmt` wraps them.

```bash
/usr/bin/git add crates/emu-core crates/soc-esp32c3 crates/emu-cli/src/run.rs docs/registers-accepted-without-effect.md
/usr/bin/git commit -m "feat(soc): CPU frequency from the clock registers and device time across clock switches

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/emu-core crates/soc-esp32c3 crates/emu-cli/src/run.rs docs/registers-accepted-without-effect.md
```

---

### Task 11: eFuse content of a production chip, and the random number generator

**Files:**
- Modify: `crates/soc-esp32c3/src/periph/efuse.rs`
- Create: `crates/soc-esp32c3/src/periph/apb_ctrl.rs`
- Modify: `crates/soc-esp32c3/src/periph/mod.rs`, `crates/soc-esp32c3/src/bus.rs` (one existing test), `docs/registers-accepted-without-effect.md`

**Interfaces:**
- Consumes: `SocConfig::{chip_revision, rng_seed}`, `DEFAULT_RNG_SEED` (Task 4); `Accepted`, `Storage::accepted` (Task 2); `svd_regs::esp32c3::addr`.
- Produces: `Efuse::new(revision)` unchanged in signature, new content; `pub struct ApbCtrl` with `ApbCtrl::new(seed: u64)`, `impl Peripheral`; `Peripherals::apb_ctrl: ApbCtrl` at `APB_CTRL_BASE = 0x6002_6000`.

eFuse values: the raw words of a production ESP32-C3 v0.3 quoted in BA §8 (Espressif's own `espefuse` dump): BLOCK0 `RD_REPEAT_DATA3 = 0x80000000` (the rest 0); BLOCK1 `RD_MAC_SPI_SYS_0..5 = 790f968c 000058cf 00000000 020c0000 715424e0 0047d2f2`; BLOCK2 `RD_SYS_PART1_DATA0..7 = 96046025 6f41fdc3 512cedbe 217ee31d d864ea41 5aba3a86 1e260363 00000009`. Decoded there: MAC `58:cf:79:0f:96:8c`, block version 1.2, chip v0.3. Why it matters: both image headers require chip revision ≥ v0.3 (BA §2.2 eFuse row), and the application's `rtc_init` needs `BLK_VERSION_MAJOR` = 1 (BA §3 notes).

RNG: `bootloader_fill_random` XORs many reads and loops while a word is 0 (BA §2.3 item 1): the values must vary. `RNG.DATA` is the SVD name of `0x600260B0` (the alias of `APB_CTRL.RND_DATA` kept by `svd-gen`).

- [ ] **Step 1: Write the failing tests**

`bus.rs`, module `tests`: the existing test `rtc_cntl_and_efuse_share_a_page_but_not_their_registers` reads `EFUSE.RD_MAC_SPI_SYS_3` (`0x6000_8850`) and expects `3 << 18`, the blank chip of SP1b-1. With the production content the same word is `0x020C_0000` (the wafer version v0.3 in bits [20:18], and bit 25 of the dump): change the expected value to `0x020C_0000`.

`efuse.rs`, module `tests`: delete `blank_chip_reads_zero_everywhere_except_the_wafer_version` and `minor_revision_is_split_across_two_words`, keep the two others (the 91-register count and the unmodelled programming registers do not change) and the helpers `revision` and `svd_read_offsets`, which the new tests use, and add:

```rust
    fn word(efuse: &mut Efuse, name: &str) -> u32 {
        efuse.read(svd_regs::esp32c3::addr("EFUSE", name) - EFUSE_BASE).unwrap()
    }

    #[test]
    fn the_mirror_holds_the_dump_of_a_production_v0_3_chip() {
        // BA §8, raw words of the three blocks.
        let mut efuse = Efuse::new(ChipRevision::V0_3);
        let expected = [
            ("RD_WR_DIS", 0),
            ("RD_REPEAT_DATA0", 0),
            ("RD_REPEAT_DATA1", 0),
            ("RD_REPEAT_DATA2", 0),
            ("RD_REPEAT_DATA3", 0x8000_0000),
            ("RD_REPEAT_DATA4", 0),
            ("RD_MAC_SPI_SYS_0", 0x790F_968C),
            ("RD_MAC_SPI_SYS_1", 0x0000_58CF),
            ("RD_MAC_SPI_SYS_2", 0),
            ("RD_MAC_SPI_SYS_3", 0x020C_0000),
            ("RD_MAC_SPI_SYS_4", 0x7154_24E0),
            ("RD_MAC_SPI_SYS_5", 0x0047_D2F2),
            ("RD_SYS_PART1_DATA0", 0x9604_6025),
            ("RD_SYS_PART1_DATA1", 0x6F41_FDC3),
            ("RD_SYS_PART1_DATA2", 0x512C_EDBE),
            ("RD_SYS_PART1_DATA3", 0x217E_E31D),
            ("RD_SYS_PART1_DATA4", 0xD864_EA41),
            ("RD_SYS_PART1_DATA5", 0x5ABA_3A86),
            ("RD_SYS_PART1_DATA6", 0x1E26_0363),
            ("RD_SYS_PART1_DATA7", 0x0000_0009),
        ];
        for (name, value) in expected {
            assert_eq!(word(&mut efuse, name), value, "{name}");
        }
        let listed: Vec<u32> = expected.iter().map(|(name, _)| svd_regs::esp32c3::addr("EFUSE", name) - EFUSE_BASE).collect();
        for offset in svd_read_offsets().into_iter().filter(|o| !listed.contains(o)) {
            assert_eq!(efuse.read(offset), Some(0), "{offset:#x}: keys, user data and error flags are blank");
        }
    }

    #[test]
    fn the_fields_the_boot_chain_decodes_have_the_values_it_needs() {
        let mut efuse = Efuse::new(ChipRevision::V0_3);
        assert_eq!(revision(&mut efuse), (0, 3), "chip revision v0.3: the image headers require at least v0.3");
        let sys3 = word(&mut efuse, "RD_MAC_SPI_SYS_3");
        let part1_4 = word(&mut efuse, "RD_SYS_PART1_DATA4");
        assert_eq!((part1_4 & 3, (sys3 >> 24) & 7), (1, 2), "eFuse block revision v1.2");
        let mac_low = word(&mut efuse, "RD_MAC_SPI_SYS_0").to_be_bytes();
        let mac_high = word(&mut efuse, "RD_MAC_SPI_SYS_1").to_be_bytes();
        assert_eq!([mac_high[2], mac_high[3], mac_low[0], mac_low[1], mac_low[2], mac_low[3]], [0x58, 0xCF, 0x79, 0x0F, 0x96, 0x8C]);
        // What the ROM decides on (FB §C rows 2 to 4): default SPI pads, no secure boot, no flash
        // power-up delay, no forced resume, direct boot not disabled.
        assert_eq!(word(&mut efuse, "RD_MAC_SPI_SYS_1") >> 16, 0);
        assert_eq!(word(&mut efuse, "RD_MAC_SPI_SYS_2"), 0);
        let data2 = word(&mut efuse, "RD_REPEAT_DATA2");
        assert_eq!((data2 >> 20) & 1, 0, "SECURE_BOOT_EN");
        assert_eq!(data2 >> 28, 0, "FLASH_TPUW");
        let data3 = word(&mut efuse, "RD_REPEAT_DATA3");
        assert_eq!(data3 & ((1 << 13) | (1 << 1)), 0, "FORCE_SEND_RESUME, DIS_DIRECT_BOOT");
        assert_eq!(data3 >> 31, 1, "ERR_RST_ENABLE (BA §11)");
        assert_eq!((word(&mut efuse, "RD_REPEAT_DATA1") >> 16) & 3, 0, "WDT_DELAY_SEL");
    }

    #[test]
    fn the_profile_revision_replaces_the_wafer_version_and_nothing_else() {
        let mut efuse = Efuse::new(ChipRevision::new(1, 9).unwrap());
        assert_eq!(revision(&mut efuse), (1, 9));
        let untouched_3 = !(0x7 << 18);
        let untouched_5 = !((0x3 << 24) | (1 << 23));
        assert_eq!(word(&mut efuse, "RD_MAC_SPI_SYS_3") & untouched_3, 0x020C_0000 & untouched_3);
        assert_eq!(word(&mut efuse, "RD_MAC_SPI_SYS_5") & untouched_5, 0x0047_D2F2 & untouched_5);
        let mut v0_0 = Efuse::new(ChipRevision::new(0, 0).unwrap());
        assert_eq!(revision(&mut v0_0), (0, 0), "the rev0 ROM tests boot a v0.0 chip");
    }
```

`apb_ctrl.rs`, module `tests`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_RNG_SEED;

    #[test]
    fn the_generator_is_deterministic_and_never_repeats_a_value_on_the_boot_path() {
        let mut first = ApbCtrl::new(DEFAULT_RNG_SEED);
        // Pinned: a recorded session replays only if the sequence never changes.
        assert_eq!(first.read(RND_DATA), Some(0x2B0E_D4C7));
        assert_eq!(first.read(RND_DATA), Some(0xC0B1_3AB1));
        assert_eq!(first.read(RND_DATA), Some(0x73D5_9E4D));
        let mut second = ApbCtrl::new(DEFAULT_RNG_SEED);
        let mut third = ApbCtrl::new(DEFAULT_RNG_SEED ^ 1);
        // 1,604 reads in the measured bootloader (BA §2 row 88).
        let a: Vec<u32> = (0..1604).map(|_| second.read(RND_DATA).unwrap()).collect();
        let b: Vec<u32> = (0..1604).map(|_| third.read(RND_DATA).unwrap()).collect();
        assert_eq!(a[..3], [0x2B0E_D4C7, 0xC0B1_3AB1, 0x73D5_9E4D]);
        assert_ne!(a, b, "the seed matters");
        let mut sorted = a.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), a.len(), "no constant, no short cycle");
    }

    #[test]
    fn an_even_number_of_reads_does_not_cancel_out() {
        // BA §2.3 item 1: each word is the XOR of many reads and the bootloader loops while it is 0.
        let mut apb = ApbCtrl::new(DEFAULT_RNG_SEED);
        for reads in [2usize, 802, 1604] {
            let word = (0..reads).fold(0, |acc, _| acc ^ apb.read(RND_DATA).unwrap());
            assert_ne!(word, 0, "{reads} reads");
        }
    }

    #[test]
    fn a_zero_seed_does_not_lock_the_generator() {
        let mut apb = ApbCtrl::new(0);
        assert_ne!(apb.read(RND_DATA), apb.read(RND_DATA));
    }

    #[test]
    fn the_generator_is_read_only_and_the_two_power_registers_are_stored() {
        let mut apb = ApbCtrl::new(DEFAULT_RNG_SEED);
        assert!(!apb.write(RND_DATA, 0));
        for (name, reset) in [("FRONT_END_MEM_PD", 0x15), ("MEM_POWER_UP", 0x3F)] {
            let offset = svd_regs::esp32c3::addr("APB_CTRL", name) - APB_CTRL_BASE;
            assert_eq!(apb.read(offset), Some(reset), "{name}");
            assert!(apb.write(offset, 1));
            assert_eq!(apb.read(offset), Some(1));
        }
        let wifi_clk_en = svd_regs::esp32c3::addr("APB_CTRL", "WIFI_CLK_EN") - APB_CTRL_BASE;
        assert_eq!(apb.read(wifi_clk_en), None, "WIFI_CLK_EN belongs to a later plan");
    }
}
```

- [ ] **Step 2: Run, see the failure**

Run: `cargo test -p soc-esp32c3 > /tmp/t11.log 2>&1; echo "exit=$?"`
Expected: `exit=101`.

- [ ] **Step 3: Implement the eFuse content**

In `efuse.rs`, replace the literal offsets by SVD lookups and add the dump:

```rust
const fn reg(name: &str) -> u32 {
    svd_regs::esp32c3::addr("EFUSE", name) - EFUSE_BASE
}

/// First and last register of the read mirror.
const RD_FIRST: u32 = reg("RD_WR_DIS");
const RD_LAST: u32 = reg("RD_RS_ERR1");
const RD_MAC_SPI_SYS_3: u32 = reg("RD_MAC_SPI_SYS_3");
const RD_MAC_SPI_SYS_5: u32 = reg("RD_MAC_SPI_SYS_5");
/// The wafer-version fields the profile's chip revision overrides.
const WAFER_FIELDS_SYS_3: u32 = WAFER_MINOR_LO_MASK << WAFER_MINOR_LO_SHIFT;
const WAFER_FIELDS_SYS_5: u32 = (0x3 << WAFER_MAJOR_SHIFT) | (1 << WAFER_MINOR_HI_SHIFT);

/// The eFuse content of a production ESP32-C3 v0.3: Espressif's own `espefuse` dump, quoted in
/// report BA §8 (decision D9). MAC `58:cf:79:0f:96:8c`, eFuse block version 1.2, calibration words,
/// `ERR_RST_ENABLE` set; no key, no secure boot, no flash encryption. Registers not listed read 0.
/// ASSUMPTION: the eFuse content of a real Xteink X4 is unknown; this is a real chip of the same
/// revision. An `espefuse.py summary` of a unit settles it.
const PRODUCTION_V0_3: [(u32, u32); 14] = [
    (reg("RD_REPEAT_DATA3"), 0x8000_0000),
    (reg("RD_MAC_SPI_SYS_0"), 0x790F_968C),
    (reg("RD_MAC_SPI_SYS_1"), 0x0000_58CF),
    (RD_MAC_SPI_SYS_3, 0x020C_0000),
    (reg("RD_MAC_SPI_SYS_4"), 0x7154_24E0),
    (RD_MAC_SPI_SYS_5, 0x0047_D2F2),
    (reg("RD_SYS_PART1_DATA0"), 0x9604_6025),
    (reg("RD_SYS_PART1_DATA1"), 0x6F41_FDC3),
    (reg("RD_SYS_PART1_DATA2"), 0x512C_EDBE),
    (reg("RD_SYS_PART1_DATA3"), 0x217E_E31D),
    (reg("RD_SYS_PART1_DATA4"), 0xD864_EA41),
    (reg("RD_SYS_PART1_DATA5"), 0x5ABA_3A86),
    (reg("RD_SYS_PART1_DATA6"), 0x1E26_0363),
    (reg("RD_SYS_PART1_DATA7"), 0x0000_0009),
];
```

`RD_RS_ERR1` is the register at `+0x1C4`; `Efuse::new` becomes:

```rust
    /// The production chip of [`PRODUCTION_V0_3`], with its wafer version replaced by `revision`.
    #[must_use]
    pub fn new(revision: ChipRevision) -> Efuse {
        let (major, minor) = (u32::from(revision.major()), u32::from(revision.minor()));
        let wafer_lo = (minor & WAFER_MINOR_LO_MASK) << WAFER_MINOR_LO_SHIFT;
        let wafer_hi_major =
            ((minor >> WAFER_MINOR_LO_BITS) << WAFER_MINOR_HI_SHIFT) | (major << WAFER_MAJOR_SHIFT);
        let read_mirror = documented_offsets()
            .into_iter()
            .map(|offset| {
                let dumped = PRODUCTION_V0_3.iter().find(|(o, _)| *o == offset).map_or(0, |&(_, value)| value);
                let value = match offset {
                    RD_MAC_SPI_SYS_3 => (dumped & !WAFER_FIELDS_SYS_3) | wafer_lo,
                    RD_MAC_SPI_SYS_5 => (dumped & !WAFER_FIELDS_SYS_5) | wafer_hi_major,
                    _ => dumped,
                };
                (offset, value)
            })
            .collect();
        Efuse { read_mirror }
    }
```

Update the module doc comment: the last sentence becomes "The content is that of a production chip (decision D9); programming registers stay unmodelled until a firmware needs them (SP1b-3)."

- [ ] **Step 4: Implement `periph/apb_ctrl.rs`**

```rust
//! `APB_CTRL` (`SYSCON`): the hardware random number generator and two radio memory power registers.
//! Sources: report BA §2 row 88, §2.3 item 1, §3 rows 29 and 34.
use super::{APB_CTRL_BASE, Accepted, Storage};
use crate::config::DEFAULT_RNG_SEED;
use emu_core::Peripheral;
use svd_regs::esp32c3::addr;

/// `RNG_DATA_REG` = `SYSCON_RND_DATA_REG` = `WDEV_RND_REG`, `0x600260B0`; the SVD lists it as `RNG.DATA`.
const RND_DATA: u32 = addr("RNG", "DATA") - APB_CTRL_BASE;
/// Multiplier of the xorshift64* generator (Vigna, "An experimental exploration of Marsaglia's
/// xorshift generators, scrambled", 2016).
const XORSHIFT_STAR: u64 = 0x2545_F491_4F6C_DD1D;

const ACCEPTED: [Accepted; 2] = [
    Accepted::svd("APB_CTRL", "FRONT_END_MEM_PD", "radio front-end memory power", "SP5a"),
    Accepted::svd("APB_CTRL", "MEM_POWER_UP", "radio memory power", "SP5a"),
];

pub(crate) fn accepted() -> Vec<Accepted> {
    ACCEPTED.to_vec()
}

#[derive(Clone, Debug)]
pub struct ApbCtrl {
    /// State of the generator; never zero.
    rng: u64,
    power: Storage,
}

impl ApbCtrl {
    /// Decision D8: the random number generator is a deterministic sequence set by `seed`, so two
    /// runs are identical and a session can be replayed. It advances on every read.
    #[must_use]
    pub fn new(seed: u64) -> ApbCtrl {
        ApbCtrl {
            // A xorshift state of 0 stays 0.
            rng: if seed == 0 { DEFAULT_RNG_SEED } else { seed },
            power: Storage::accepted(APB_CTRL_BASE, &ACCEPTED),
        }
    }

    fn next_random(&mut self) -> u32 {
        let mut state = self.rng;
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        self.rng = state;
        let [a, b, c, d, ..] = state.wrapping_mul(XORSHIFT_STAR).to_be_bytes();
        u32::from_be_bytes([a, b, c, d])
    }
}

impl Peripheral for ApbCtrl {
    fn read(&mut self, offset: u32) -> Option<u32> {
        if offset == RND_DATA {
            return Some(self.next_random());
        }
        self.power.read(offset)
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        self.power.write(offset, value)
    }
}
```

`periph/mod.rs`: `pub mod apb_ctrl;`, `pub use apb_ctrl::ApbCtrl;`, `pub(crate) const APB_CTRL_BASE: u32 = 0x6002_6000;` and `const APB_CTRL: (u32, u32) = (APB_CTRL_BASE, 0x6002_7000);` (comment: "`DR_REG_SYSCON_BASE`; report BC §A.2"), field `pub apb_ctrl: ApbCtrl` (`ApbCtrl::new(config.rng_seed)`), an arm in `device`, `apb_ctrl::accepted()` in `accepted_without_effect`.

- [ ] **Step 5: Regenerate the documented table and run**

Run: `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect > /tmp/t11doc.log 2>&1; echo "exit=$?"`
Expected: `exit=0`: the lists changed in this task, so the test rewrote the table of `docs/registers-accepted-without-effect.md` (run without the variable it fails and names this command; it never writes then).
Check `/usr/bin/git diff -- docs/registers-accepted-without-effect.md`: the diff touches table rows only, nothing above the `<!-- generated table -->` line, and `grep -c '^| .0x' docs/registers-accepted-without-effect.md` prints `72`. The file is part of this task's commit. Then:
Run: `cargo test --workspace > /tmp/t11.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.
Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/t11c.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.
Run: `INKEMU_ROM_ELF_DIR=/tmp/inkemu-rom cargo test --release -p soc-esp32c3 --test real_rom -- --ignored > /tmp/t11rom.log 2>&1; echo "exit=$?"`
Expected: `exit=0`: the three ROM revisions still reach their banner in strict mode with the new eFuse content, and the flash-path tests of Task 9 still pass.

- [ ] **Step 6: Commit**

Before committing (Global Constraints): `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`: `exit=0` three times. The code blocks of this plan are not wrapped as `rustfmt` wraps them.

```bash
/usr/bin/git add crates/soc-esp32c3 docs/registers-accepted-without-effect.md
/usr/bin/git commit -m "feat(soc): eFuse content of a production v0.3 chip and a deterministic random number generator

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3 docs/registers-accepted-without-effect.md
```

---

### Task 12: Analog bus, watchdog, RTC and power registers the bootloader and the early application configure

**Files:**
- Create: `crates/soc-esp32c3/src/periph/ana_i2c.rs`
- Modify: `crates/soc-esp32c3/src/periph/{mod,rtc_cntl,uart,assist_debug}.rs`
- Modify: `docs/registers-accepted-without-effect.md`

**Interfaces:**
- Consumes: `Accepted`, `AcceptedBlock`, `Storage::accepted` (Task 2); `Unsupported::SoftwareReset` (Task 2); `Peripherals::take_unsupported` (Tasks 5, 8).
- Produces: `pub struct AnaI2c` (`AnaI2c::new()`, `impl Peripheral`) at `ANA_I2C_BASE = 0x6000_E000`; `RtcCntl::take_unsupported(&mut self) -> Option<Unsupported>`; new `Peripherals` fields `ana_i2c: AnaI2c`, `timg0: AcceptedBlock`, `apb_saradc: AcceptedBlock`, `radio_power: AcceptedBlock`.

What each block must provide (BA §2.2, §3, §5, §6):

| Block | Registers | Model |
|---|---|---|
| regi2c master, `0x6000E000` | host-0 command `+0x00`, host-1 command `+0x04`, `I2C_MST_ANA_CONF0 +0x40`, `ANA_CONFIG +0x44`, `ANA_CONFIG2 +0x48` | command: bit 25 (busy) reads 0, bit 24 = write, [23:16] data, [15:8] register, [7:0] block; one stored byte per (host, block, register) (D24) |
| `RTC_CNTL` | `OPTIONS0`, `TIMER1`, `TIMER3..6`, `ANA_CONF`, `INT_ENA_RTC`, `INT_CLR_RTC`, `CLK_CONF`, `SLOW_CLK_CONF`, `RTC_CNTL` (`+0x80`), `DIG_PWC`, `WDTCONFIG0`, `WDTCONFIG1`, `WDTFEED`, `WDTWPROTECT`, `SWD_WPROTECT`, `BROWN_OUT`, `FIB_SEL`, `SENSOR_CTRL` | storage (D11: no watchdog expiry); `OPTIONS0` refuses a software reset (D27) |
| `TIMG0` | `WDTCONFIG0`, `WDTWPROTECT` | storage (the bootloader clears `FLASHBOOT_MOD_EN`, BA §6); every other TIMG0 register stays unmodelled: `RTCCALICFG` is where SP1b-3 starts |
| `APB_SARADC` | `CLKM_CONF`, `CTRL`, `FSM_WAIT`, `SAR_PATT_TAB1`, `SAR_PATT_TAB2`, `CTRL2`, `DMA_CONF` | storage (RNG entropy source set-up, BA §2 rows 79–85) |
| radio power | `BB.BBPD_CTRL 0x6001D054`, `NRX 0x6001CCD4`, `FE 0x60006090`, `FE2 0x600050F0` | storage (`rtc_sleep_pu`, BA §3 rows 30–33) |
| `UART0`/`UART1` | `FSM_STATUS` | reads its reset value 0: `uart_tx_wait_idle` needs bits [7:4] = 0 (BA §2.2 UART0 row) |
| `ASSIST_DEBUG` | `C0RE_0_DEBUG_MODE` | reads 0: 1 would mean "debugger attached" and skip the image verification (BA §2.2) |

- [ ] **Step 1: Write the failing tests**

`ana_i2c.rs`, module `tests`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const BUSY: u32 = 1 << 25;
    fn write_cmd(block: u32, register: u32, data: u32) -> u32 {
        0x0500_0000 | (data << 16) | (register << 8) | block
    }
    fn read_cmd(block: u32, register: u32) -> u32 {
        0x0400_0000 | (register << 8) | block
    }

    #[test]
    fn a_command_is_never_busy_and_a_read_returns_the_byte_written_before() {
        // BA §2.2: the ROM writes the command, spins while bit 25 is set, reads bits [23:16].
        let mut i2c = AnaI2c::new();
        assert!(i2c.write(HOST0_CMD, write_cmd(0x69, 0x07, 0x5A)));
        assert_eq!(i2c.read(HOST0_CMD).map(|v| v & BUSY), Some(0));
        assert!(i2c.write(HOST0_CMD, read_cmd(0x69, 0x07))); // BA §2 row 76: 0x4000769
        assert_eq!(i2c.read(HOST0_CMD), Some(0x0400_0769 | (0x5A << 16)));
    }

    #[test]
    fn a_register_never_written_reads_zero_and_the_two_hosts_are_separate() {
        let mut i2c = AnaI2c::new();
        i2c.write(HOST0_CMD, read_cmd(0x66, 0x02));
        assert_eq!(i2c.read(HOST0_CMD).map(|v| (v >> 16) & 0xFF), Some(0), "decision D24");
        i2c.write(HOST0_CMD, write_cmd(0x66, 0x02, 0x6B)); // BBPLL MODE_HF = 0x6B (BA §5 step 10)
        i2c.write(HOST1_CMD, read_cmd(0x66, 0x02));
        assert_eq!(i2c.read(HOST1_CMD).map(|v| (v >> 16) & 0xFF), Some(0));
        i2c.write(HOST0_CMD, read_cmd(0x66, 0x02));
        assert_eq!(i2c.read(HOST0_CMD).map(|v| (v >> 16) & 0xFF), Some(0x6B));
    }

    #[test]
    fn a_busy_bit_written_by_software_is_not_kept() {
        let mut i2c = AnaI2c::new();
        i2c.write(HOST0_CMD, read_cmd(0x69, 0x07) | BUSY);
        assert_eq!(i2c.read(HOST0_CMD).map(|v| v & BUSY), Some(0));
    }

    #[test]
    fn the_three_configuration_registers_are_stored_from_zero() {
        let mut i2c = AnaI2c::new();
        for offset in [I2C_MST_ANA_CONF0, ANA_CONFIG, ANA_CONFIG2] {
            assert_eq!(i2c.read(offset), Some(0), "decision D24");
            assert!(i2c.write(offset, 0xFFFF_BFFF)); // BA §3 row 10
            assert_eq!(i2c.read(offset), Some(0xFFFF_BFFF));
        }
        assert_eq!(i2c.read(0x08), None);
        assert!(!i2c.write(0x4C, 0));
    }
}
```

`rtc_cntl.rs`, module `tests` — replace `everything_else_is_left_to_the_unmodeled_path` by:

```rust
    fn reg(name: &str) -> u32 {
        svd_regs::esp32c3::addr("RTC_CNTL", name) - RTC_CNTL_BASE
    }

    #[test]
    fn clock_analog_and_watchdog_configuration_is_stored_from_its_reset_value() {
        let mut rtc = RtcCntl::new();
        for (name, reset) in [
            ("OPTIONS0", 0x1C00_A000),
            ("TIMER1", 0x2814_0403),
            ("ANA_CONF", 0x00C4_0000),
            ("CLK_CONF", 0x1158_3218),
            ("RTC_CNTL", 0xA000_0000),
            ("DIG_PWC", 0x0055_5010),
            ("WDTCONFIG0", 0x0001_3214),
            ("WDTCONFIG1", 0x0003_0D40),
            ("BROWN_OUT", 0x43FF_0010),
            ("FIB_SEL", 0x0000_0007),
        ] {
            assert_eq!(rtc.read(reg(name)), Some(reset), "{name}");
            assert!(rtc.write(reg(name), 0x0000_0140));
            assert_eq!(rtc.read(reg(name)), Some(0x0000_0140), "{name}");
        }
        for name in ["TIMER3", "TIMER4", "TIMER5", "TIMER6", "SLOW_CLK_CONF", "INT_ENA_RTC", "INT_CLR_RTC", "WDTFEED", "WDTWPROTECT", "SWD_WPROTECT", "SENSOR_CTRL"] {
            assert!(rtc.write(reg(name), 0x8F1D_312A), "{name}");
            assert_eq!(rtc.read(reg(name)), Some(0x8F1D_312A), "{name}");
        }
    }

    #[test]
    fn the_bootloader_watchdog_sequence_leaves_the_registers_as_written_and_nothing_expires() {
        // BA §6: unlock, clear FLASHBOOT_MOD_EN, stage 0 = 0x956A0, feed, enable, lock.
        let mut rtc = RtcCntl::new();
        rtc.write(reg("WDTWPROTECT"), 0x50D8_3AA1);
        rtc.write(reg("WDTCONFIG1"), 0x0009_56A0);
        rtc.write(reg("WDTFEED"), 1 << 31);
        rtc.write(reg("WDTCONFIG0"), 0xC007_EA00);
        rtc.write(reg("WDTWPROTECT"), 0);
        assert_eq!(rtc.read(reg("WDTCONFIG0")), Some(0xC007_EA00));
        assert_eq!(rtc.read(reg("WDTCONFIG1")), Some(0x0009_56A0));
        assert_eq!(rtc.read(RESET_STATE).map(|v| v & 0x3F), Some(1), "still a power-on reset: decision D11");
        assert_eq!(rtc.take_unsupported(), None);
    }

    #[test]
    fn a_software_reset_request_is_reported_and_not_stored() {
        // BC §E.2: OPTIONS0 bit 31 SW_SYS_RST, bit 5 SW_PROCPU_RST.
        for value in [1u32 << 31, 1 << 5, 0x1C00_A000 | (1 << 5)] {
            let mut rtc = RtcCntl::new();
            assert!(rtc.write(reg("OPTIONS0"), value));
            assert_eq!(rtc.take_unsupported(), Some(Unsupported::SoftwareReset { options0: value }));
            assert_eq!(rtc.take_unsupported(), None);
            assert_eq!(rtc.read(reg("OPTIONS0")), Some(0x1C00_A000), "not stored: decision D27");
        }
        let mut rtc = RtcCntl::new();
        rtc.write(reg("OPTIONS0"), 0x1C00_A000 & !(1 << 13));
        assert_eq!(rtc.take_unsupported(), None, "BBPLL power bits are plain configuration (BA §5 step 9)");
    }

    #[test]
    fn everything_else_is_left_to_the_unmodeled_path() {
        let mut rtc = RtcCntl::new();
        assert!(!rtc.write(RESET_STATE, 0), "RESET_STATE is not writable here");
        for name in ["TIME_UPDATE", "TIME_LOW0", "SLP_TIMER0", "STATE0", "WDTCONFIG2"] {
            assert_eq!(rtc.read(reg(name)), None, "{name}: the RTC timer, sleep and watchdog stages are SP1b-3");
        }
    }
```

`uart.rs` and `assist_debug.rs`, modules `tests`:

```rust
    #[test]
    fn the_transmitter_state_machine_reads_idle() {
        // BA §2.2: `uart_tx_wait_idle` loops until FSM_STATUS[7:4] is 0.
        let mut uart = Uart::new(UART0);
        let fsm = svd_regs::esp32c3::addr("UART0", "FSM_STATUS") - UART0;
        assert_eq!(uart.read(fsm), Some(0));
        assert!(!uart.write(fsm, 0xFF), "read-only");
    }
```

```rust
    #[test]
    fn no_debugger_is_attached() {
        // BA §2.2: bit 0 set would make the bootloader skip the checksum and hash verification.
        let mut dbg = AssistDebug::new();
        let mode = svd_regs::esp32c3::addr("ASSIST_DEBUG", "C0RE_0_DEBUG_MODE") - crate::periph::ASSIST_DEBUG_BASE;
        assert_eq!(dbg.read(mode), Some(0));
        assert!(!dbg.write(mode, 1), "read-only");
    }
```

`periph/mod.rs`, module `tests`:

```rust
    #[test]
    fn storage_only_blocks_answer_exactly_their_listed_registers() {
        let mut p = Peripherals::new(&SocConfig::default(), Box::new(crate::flash::NorFlash::blank(crate::flash::FlashConfig::default())));
        let mut read = |addr: u32| p.device(addr).and_then(|(device, base)| device.read(addr - base));
        let svd = svd_regs::esp32c3::addr;
        assert_eq!(read(svd("TIMG0", "WDTCONFIG0")), Some(0x0004_C000), "FLASHBOOT_MOD_EN set at reset (BC §E.3)");
        assert_eq!(read(svd("TIMG0", "WDTWPROTECT")), Some(0x50D8_3AA1));
        assert_eq!(read(svd("TIMG0", "RTCCALICFG")), None, "the RTC calibration is where SP1b-3 starts");
        assert_eq!(read(svd("TIMG0", "T0CONFIG")), None);
        assert_eq!(read(svd("APB_SARADC", "CTRL")), Some(0x4003_8240));
        assert_eq!(read(svd("APB_SARADC", "CLKM_CONF")), Some(4));
        assert_eq!(read(svd("APB_SARADC", "ONETIME_SAMPLE")), None, "conversions are SP2");
        for addr in [svd("BB", "BBPD_CTRL"), NRX_NRXPD_CTRL, FE_GEN_CTRL, FE2_TX_INTERP_CTRL] {
            assert_eq!(read(addr), Some(0), "{addr:#010x}");
        }
        assert_eq!(read(svd("BB", "BBPD_CTRL") - 4), None, "the register before BBPD_CTRL is not listed");
        assert_eq!(read(ana_i2c::ANA_I2C_BASE + 0x44), Some(0), "ANA_CONFIG");
    }
```

- [ ] **Step 2: Run, see the failure**

Run: `cargo test -p soc-esp32c3 > /tmp/t12.log 2>&1; echo "exit=$?"`
Expected: `exit=101`.

- [ ] **Step 3: Implement `periph/ana_i2c.rs`**

```rust
//! The analog I2C master ("regi2c") through which the ROM configures analog blocks: PLL, SAR ADC,
//! bias. Source: report BA §2.2 (ROM `rom_chip_i2c_readReg_org@0x40038e58`,
//! `rom_chip_i2c_writeReg@0x40039170`, `rom_get_i2c_hostid@0x40038e30`; ESP-IDF `regi2c_defs.h`).
//!
//! The SVD does not describe this window (decision D17). No analog behaviour is modelled: a byte
//! written to an analog register reads back, nothing more (decision D24).
use super::Accepted;
use emu_core::Peripheral;
use std::collections::BTreeMap;

/// `DR_REG_RTC_I2C_BASE` (ESP-IDF `reg_base.h`).
pub(crate) const ANA_I2C_BASE: u32 = 0x6000_E000;
/// Command register of I2C host 0, and of host 1 (used for blocks `0x62..=0x64` only).
const HOST0_CMD: u32 = 0x00;
const HOST1_CMD: u32 = 0x04;
/// `I2C_MST_ANA_CONF0_REG`, `ANA_CONFIG_REG`, `ANA_CONFIG2_REG` (`regi2c_defs.h`).
const I2C_MST_ANA_CONF0: u32 = 0x40;
const ANA_CONFIG: u32 = 0x44;
const ANA_CONFIG2: u32 = 0x48;
const HOSTS: usize = 2;
const CONFIG_REGISTERS: usize = 3;

/// Command word: [7:0] block, [15:8] register, [23:16] data, bit 24 write, bit 25 busy.
const CMD_DATA_SHIFT: u32 = 16;
const CMD_DATA_MASK: u32 = 0xFF << CMD_DATA_SHIFT;
const CMD_WRITE: u32 = 1 << 24;
const CMD_BUSY: u32 = 1 << 25;

const WHY: &str = "analog register bus: bytes are stored, the analog blocks (PLL, SAR ADC, bias) do nothing";

pub(crate) fn accepted() -> Vec<Accepted> {
    vec![
        Accepted::at("ANA_I2C", "HOST0_CMD", ANA_I2C_BASE + HOST0_CMD, WHY, "SP2"),
        Accepted::at("ANA_I2C", "HOST1_CMD", ANA_I2C_BASE + HOST1_CMD, WHY, "SP2"),
        Accepted::at("ANA_I2C", "I2C_MST_ANA_CONF0", ANA_I2C_BASE + I2C_MST_ANA_CONF0, WHY, "SP2"),
        Accepted::at("ANA_I2C", "ANA_CONFIG", ANA_I2C_BASE + ANA_CONFIG, WHY, "SP2"),
        Accepted::at("ANA_I2C", "ANA_CONFIG2", ANA_I2C_BASE + ANA_CONFIG2, WHY, "SP2"),
    ]
}

#[derive(Clone, Debug, Default)]
pub struct AnaI2c {
    command: [u32; HOSTS],
    /// The analog registers written so far: (host, block, register) to byte. Ordered: deterministic.
    bytes: BTreeMap<(usize, u8, u8), u8>,
    /// ASSUMPTION: (decision D24) these three reset to 0; their reset values are not documented and
    /// the QEMU oracle answers `0xFFFFFF`. Every use on the measured paths is read-modify-write.
    config: [u32; CONFIG_REGISTERS],
}

impl AnaI2c {
    #[must_use]
    pub fn new() -> AnaI2c {
        AnaI2c::default()
    }

    /// Runs the transfer `word` asks for and returns what the command register then reads: never busy.
    fn run(&mut self, host: usize, word: u32) -> u32 {
        let [block, register, data, _] = word.to_le_bytes();
        if word & CMD_WRITE != 0 {
            self.bytes.insert((host, block, register), data);
            return word & !CMD_BUSY;
        }
        // ASSUMPTION: (decision D24) an analog register that was never written reads 0.
        let read = self.bytes.get(&(host, block, register)).copied().unwrap_or(0);
        (word & !(CMD_BUSY | CMD_DATA_MASK)) | (u32::from(read) << CMD_DATA_SHIFT)
    }

    fn config_index(offset: u32) -> Option<usize> {
        [I2C_MST_ANA_CONF0, ANA_CONFIG, ANA_CONFIG2].iter().position(|&o| o == offset)
    }
}

impl Peripheral for AnaI2c {
    fn read(&mut self, offset: u32) -> Option<u32> {
        match offset {
            HOST0_CMD => self.command.first().copied(),
            HOST1_CMD => self.command.get(1).copied(),
            _ => Self::config_index(offset).and_then(|i| self.config.get(i).copied()),
        }
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        let host = match offset {
            HOST0_CMD => 0,
            HOST1_CMD => 1,
            _ => {
                let Some(slot) = Self::config_index(offset).and_then(|i| self.config.get_mut(i)) else {
                    return false;
                };
                *slot = value;
                return true;
            }
        };
        let answer = self.run(host, value);
        if let Some(slot) = self.command.get_mut(host) {
            *slot = answer;
        }
        true
    }
}
```

- [ ] **Step 4: `RTC_CNTL`, UART, ASSIST_DEBUG**

`rtc_cntl.rs`: the list grows to 26 rows (the five of Task 2, then):

```rust
    Accepted::svd("RTC_CNTL", "OPTIONS0", "BBPLL, crystal and I2C power forcing (software reset bits are refused)", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "TIMER1", "clock and PLL wake-up delays", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "TIMER3", "Wi-Fi/BT power-up timing", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "TIMER4", "digital power-up timing", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "TIMER5", "minimum sleep time", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "TIMER6", "peripheral power-up timing", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "ANA_CONF", "analog configuration, glitch reset", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "INT_ENA_RTC", "RTC interrupt enables: nothing is raised", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "INT_CLR_RTC", "RTC interrupt clear: nothing is pending", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "CLK_CONF", "RTC fast and slow clock sources and dividers", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "SLOW_CLK_CONF", "RTC slow clock divider", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "RTC_CNTL", "RTC regulator and digital bias", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "DIG_PWC", "digital power-domain control", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "WDTCONFIG0", "RTC watchdog enable and stages: it never expires (D11)", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "WDTCONFIG1", "RTC watchdog stage 0 timeout", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "WDTFEED", "RTC watchdog feed", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "WDTWPROTECT", "RTC watchdog write protection: writes are never locked out", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "SWD_WPROTECT", "super watchdog write protection", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "BROWN_OUT", "brown-out detector", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "FIB_SEL", "brown-out and glitch reset selection", "SP1b-3"),
    Accepted::svd("RTC_CNTL", "SENSOR_CTRL", "SAR ADC power forcing", "SP2"),
```

Replace the literal `RESET_STATE` and `STORE` offsets by their `addr("RTC_CNTL", …) - RTC_CNTL_BASE` forms (`STORE0..STORE7`), with the imports `use crate::diag::Unsupported;` and `use svd_regs::esp32c3::addr;`. Add the field `unsupported: Option<Unsupported>` (`None` in `new`), `pub(crate) fn take_unsupported(&mut self) -> Option<Unsupported> { self.unsupported.take() }`, and at the top of `write`:

```rust
        // Decision D27: a software reset is not emulated yet (SP1b-3). Storing the bit would be a
        // reset that silently never happens: the request is reported and dropped instead.
        if offset == OPTIONS0 && value & (SW_SYS_RST | SW_PROCPU_RST) != 0 {
            self.unsupported = Some(Unsupported::SoftwareReset { options0: value });
            return true;
        }
```

with `const OPTIONS0: u32 = addr(…)`, `/// `OPTIONS0` bit 31 and bit 5 (report BC §E.2).` `const SW_SYS_RST: u32 = 1 << 31; const SW_PROCPU_RST: u32 = 1 << 5;`. Update the module doc comment: "watchdogs are stored and never expire (decision D11); the RTC timer and resets arrive in SP1b-3".

`uart.rs`: add `const FSM_STATUS: u32 = svd_regs::esp32c3::addr("UART0", "FSM_STATUS") - svd_regs::esp32c3::addr("UART0", "FIFO");` (the model serves UART0 and UART1, whose register offsets are identical), and in `read` the arm `FSM_STATUS => Some(0),` with the comment "The transmitter is idle: bytes leave at once (decision D7 of SP1b-1). `uart_tx_wait_idle` waits for bits [7:4] = 0 (BA §2.2)."

`assist_debug.rs`: `const DEBUG_MODE: u32 = svd_regs::esp32c3::addr("ASSIST_DEBUG", "C0RE_0_DEBUG_MODE") - crate::periph::ASSIST_DEBUG_BASE;` and `DEBUG_MODE` joins the existing arm that reads 0 (a separate arm with the same body is refused by clippy, `match_same_arms`):

```rust
            // Nothing was recorded (this is a power-on reset) and no debugger is attached (BA §2.2).
            CORE_0_RCD_PDEBUGPC | CORE_0_RCD_PDEBUGSP | DEBUG_MODE => Some(0),
```

- [ ] **Step 5: The storage-only blocks and the registry** (`periph/mod.rs`)

```rust
pub mod ana_i2c;
pub use ana_i2c::AnaI2c;

const ANA_I2C: (u32, u32) = (ana_i2c::ANA_I2C_BASE, ana_i2c::ANA_I2C_BASE + 0x100);
pub(crate) const TIMG0_BASE: u32 = 0x6001_F000;
const TIMG0: (u32, u32) = (TIMG0_BASE, 0x6002_0000);
pub(crate) const APB_SARADC_BASE: u32 = 0x6004_0000;
const APB_SARADC: (u32, u32) = (APB_SARADC_BASE, 0x6004_1000);

/// Decision D11: the main watchdog of timer group 0 is stored and never expires. The timers and the
/// RTC calibration of this block are not modelled here: they are the first thing SP1b-3 adds.
const TIMG0_ACCEPTED: [Accepted; 2] = [
    Accepted::svd("TIMG0", "WDTCONFIG0", "main watchdog 0 enable and stages: it never expires (D11)", "SP1b-3"),
    Accepted::svd("TIMG0", "WDTWPROTECT", "main watchdog 0 write protection", "SP1b-3"),
];
/// The bootloader configures the SAR ADC as an entropy source for the RNG (BA §2 rows 79 to 85).
const WHY_SARADC: &str = "SAR ADC configuration: no conversion is performed";
const APB_SARADC_ACCEPTED: [Accepted; 7] = [
    Accepted::svd("APB_SARADC", "CTRL", WHY_SARADC, "SP2"),
    Accepted::svd("APB_SARADC", "CTRL2", WHY_SARADC, "SP2"),
    Accepted::svd("APB_SARADC", "FSM_WAIT", WHY_SARADC, "SP2"),
    Accepted::svd("APB_SARADC", "SAR_PATT_TAB1", WHY_SARADC, "SP2"),
    Accepted::svd("APB_SARADC", "SAR_PATT_TAB2", WHY_SARADC, "SP2"),
    Accepted::svd("APB_SARADC", "DMA_CONF", WHY_SARADC, "SP2"),
    Accepted::svd("APB_SARADC", "CLKM_CONF", WHY_SARADC, "SP2"),
];
/// Radio memory power bits `rtc_sleep_pu` sets in the application's `rtc_init` (BA §3 rows 30 to 33).
/// Only `BB.BBPD_CTRL` is in the SVD; the three others are named in ESP-IDF `nrx_reg.h`
/// (`NRXPD_CTRL`), `fe_reg.h` (`FE_GEN_CTRL`, `FE2_TX_INTERP_CTRL`) (decision D17).
const NRX_NRXPD_CTRL: u32 = 0x6001_CCD4;
const FE_GEN_CTRL: u32 = 0x6000_6090;
const FE2_TX_INTERP_CTRL: u32 = 0x6000_50F0;
const WHY_RADIO: &str = "radio memory power: there is no radio";
const RADIO_POWER_ACCEPTED: [Accepted; 4] = [
    Accepted::at("FE2", "FE2_TX_INTERP_CTRL", FE2_TX_INTERP_CTRL, WHY_RADIO, "SP5a"),
    Accepted::at("FE", "FE_GEN_CTRL", FE_GEN_CTRL, WHY_RADIO, "SP5a"),
    Accepted::at("NRX", "NRXPD_CTRL", NRX_NRXPD_CTRL, WHY_RADIO, "SP5a"),
    Accepted::svd("BB", "BBPD_CTRL", WHY_RADIO, "SP5a"),
];
```

Fields: `pub ana_i2c: AnaI2c` (`AnaI2c::new()`), `pub timg0: AcceptedBlock` (`AcceptedBlock::new(TIMG0_BASE, &TIMG0_ACCEPTED)`), `pub apb_saradc: AcceptedBlock` (`AcceptedBlock::new(APB_SARADC_BASE, &APB_SARADC_ACCEPTED)`), `pub radio_power: AcceptedBlock` (`AcceptedBlock::new(0, &RADIO_POWER_ACCEPTED)`: base 0, so its offsets are absolute addresses). In `device`, the radio registers are four isolated words:

```rust
        } else if RADIO_POWER_ACCEPTED.iter().any(|a| a.addr == addr) {
            Some((&mut self.radio_power, 0))
```

placed before the final `else`, plus the arms for `ANA_I2C` (base `ana_i2c::ANA_I2C_BASE`), `TIMG0` and `APB_SARADC`. `take_unsupported` becomes:

```rust
        self.spi1
            .take_unsupported()
            .or_else(|| self.sha.take_unsupported())
            .or_else(|| self.rtc_cntl.take_unsupported())
```

and `accepted_without_effect` gains `ana_i2c::accepted()`, `TIMG0_ACCEPTED.to_vec()`, `APB_SARADC_ACCEPTED.to_vec()`, `RADIO_POWER_ACCEPTED.to_vec()`.

- [ ] **Step 6: Regenerate the documented table and run**

Run: `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect > /tmp/t12doc.log 2>&1; echo "exit=$?"`
Expected: `exit=0`: the lists changed in this task, so the test rewrote the table of `docs/registers-accepted-without-effect.md` (run without the variable it fails and names this command; it never writes then).
Check `/usr/bin/git diff -- docs/registers-accepted-without-effect.md`: the diff touches table rows only, nothing above the `<!-- generated table -->` line, and `grep -c '^| .0x' docs/registers-accepted-without-effect.md` prints `111`. The file is part of this task's commit. Then:
Run: `cargo test --workspace > /tmp/t12.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.
Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/t12c.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 7: Commit**

Before committing (Global Constraints): `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`: `exit=0` three times. The code blocks of this plan are not wrapped as `rustfmt` wraps them.

```bash
/usr/bin/git add crates/soc-esp32c3 docs/registers-accepted-without-effect.md
/usr/bin/git commit -m "feat(soc): analog bus, watchdog, RTC and power configuration the bootloader writes

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3 docs/registers-accepted-without-effect.md
```

---

### Task 13: `boards` — the flash in device profiles, and profile validation

**Files:**
- Modify: `crates/boards/profiles/x4.toml`, `crates/boards/profiles/x3.toml`, `crates/boards/src/lib.rs`

**Interfaces:**
- Consumes: `soc_esp32c3::flash::{FlashConfig, FlashImageError, JedecId}`, `SocConfig::flash`, `Esp32C3::with_flash` (Tasks 4, 5).
- Produces:
  ```rust
  pub struct DeviceProfile { pub id: String, pub name: String, pub soc: SocFamily, pub chip_revision: ChipRevision, pub flash: FlashConfig }
  pub enum ProfileError { Parse { origin: String, detail: String }, UnknownDevice { id: String, known: String }, IdMismatch { origin: String, expected: String, found: String } }
  impl DeviceProfile {
      pub fn from_toml(origin: &str, text: &str) -> Result<DeviceProfile, ProfileError>;                       // unchanged signature
      pub fn from_named_toml(expected_id: &str, origin: &str, text: &str) -> Result<DeviceProfile, ProfileError>;
      pub fn instantiate(&self, rom: RomImage, options: MachineOptions) -> Esp32C3;                             // unchanged: erased flash
      pub fn instantiate_with_flash(&self, rom: RomImage, image: Vec<u8>, options: MachineOptions) -> Result<Esp32C3, FlashImageError>;
  }
  ```
  Profile format: a required table `[flash]` with `size_mib` (1, 2, 4, 8 or 16) and `jedec_id` (`0xMMTTCC`).

- [ ] **Step 1: Write the failing tests** (`crates/boards/src/lib.rs`, module `tests`)

Replace the constant `VALID` (line numbers matter to the tests below):

```rust
    const VALID: &str = "id = \"demo\"\nname = \"Demo board\"\n\n[soc]\nfamily = \"esp32c3\"\nrevision = \"v1.1\"\n\n[flash]\nsize_mib = 4\njedec_id = 0xEF4016\n";
```

In `unknown_key_is_rejected_rather_than_ignored`, the appended line `flash_size = "16MB"` now lands in `[flash]` on line 11: change the expected `"line 7"` to `"line 11"`. No other test of this module needs a `[flash]` table: the only one that writes a profile text of its own, `missing_key_and_broken_toml_are_reported_with_the_file_name`, expects the error about the missing `name`, which is still the first missing key reported. In `builtin_profiles_load_and_carry_their_own_id`, add:

```rust
            assert_eq!(profile.flash.size, 16 << 20, "spec §2: 16 MB of flash");
            assert_eq!(profile.flash.jedec_id, soc_esp32c3::JedecId::QEMU_ORACLE, "decision D2");
```

In `options_and_profile_combine_into_the_soc_configuration` (its variables are `profile` and `config`), add `assert_eq!(config.flash, profile.flash);`, and add `flash: profile.flash,` to the `SocConfig { chip_revision: config.chip_revision, ..SocConfig::default() }` literal its last assertion compares with: the demo profile has a 4 MiB flash, not the default one. New tests:

```rust
    /// `VALID` with one line replaced.
    fn with_line(number: usize, replacement: &str) -> String {
        VALID
            .lines()
            .enumerate()
            .map(|(i, line)| if i + 1 == number { replacement } else { line })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn valid_profile_carries_its_flash() {
        let profile = DeviceProfile::from_toml("demo.toml", VALID).unwrap();
        assert_eq!(profile.flash.size, 4 << 20);
        assert_eq!(profile.flash.jedec_id.bytes(), [0xEF, 0x40, 0x16]);
    }

    #[test]
    fn flash_size_must_be_a_power_of_two_up_to_16_mib() {
        for bad in ["0", "3", "32", "-4", "\"16MB\""] {
            let err = error_for(&with_line(9, &format!("size_mib = {bad}")));
            assert!(err.contains("line 9"), "{bad}: {err}");
        }
        let err = error_for(&with_line(9, "size_mib = 3"));
        assert!(err.contains("1, 2, 4, 8 or 16"), "{err}");
    }

    #[test]
    fn jedec_id_must_be_three_bytes() {
        let err = error_for(&with_line(10, "jedec_id = 0x1EF4016"));
        assert!(err.contains("line 10") && err.contains("0xMMTTCC"), "{err}");
    }

    #[test]
    fn jedec_capacity_must_match_the_flash_size() {
        // 0x16 is 4 MiB; the profile says 8.
        let err = error_for(&with_line(9, "size_mib = 8"));
        assert!(err.contains("line ") && err.contains("capacity byte 0x16") && err.contains("8 MiB"), "{err}");
    }

    #[test]
    fn the_flash_table_is_required() {
        let without = VALID.split("\n[flash]").next().unwrap();
        assert!(error_for(without).contains("flash"));
    }

    #[test]
    fn empty_or_malformed_id_is_refused_with_its_line() {
        // TOML source lines: the fourth id holds an escape character (written with a TOML escape).
        for line in [r#"id = """#, r#"id = "X4""#, r#"id = "x 4""#, r#"id = "x4\u001b[31m""#, r#"id = "../x4""#] {
            let err = error_for(&with_line(1, line));
            assert!(err.contains("line 1") && err.contains("a-z, 0-9"), "{line}: {err}");
            assert!(!err.contains('\u{1b}'), "control characters are not echoed");
        }
    }

    #[test]
    fn a_profile_registered_under_another_id_is_refused() {
        let err = DeviceProfile::from_named_toml("x9", "built-in profile x9", VALID).unwrap_err();
        assert_eq!(
            err,
            ProfileError::IdMismatch { origin: "built-in profile x9".into(), expected: "x9".into(), found: "demo".into() }
        );
        let text = err.to_string();
        assert!(text.contains("`demo`") && text.contains("`x9`") && text.contains("; make"), "{text}");
        assert!(DeviceProfile::from_named_toml("demo", "demo.toml", VALID).is_ok());
    }

    #[test]
    fn a_machine_can_be_instantiated_around_a_flash_image() {
        let profile = builtin("x4").unwrap();
        let ok = profile.instantiate_with_flash(flat_rom(&[HANG]), vec![0xE9; 32], MachineOptions::default());
        assert!(ok.is_ok());
        let too_long = vec![0u8; (16 << 20) + 1];
        let err = profile.instantiate_with_flash(flat_rom(&[HANG]), too_long, MachineOptions::default()).unwrap_err();
        assert!(err.to_string().contains("16777217"), "{err}");
    }
```

`error_for` is the existing helper of that module (it returns the message of `DeviceProfile::from_toml("demo.toml", text)`).

- [ ] **Step 2: Run, see the failure**

Run: `cargo test -p boards > /tmp/t13.log 2>&1; echo "exit=$?"`
Expected: `exit=101`.

- [ ] **Step 3: Implement**

Both profiles gain (after the `[soc]` table):

```toml

[flash]
# 16 MB: docs/research/xteink-hardware.md.
size_mib = 16
# ASSUMPTION: the flash part of retail units is not established. 0x9D6018 (manufacturer 0x9D, type
# 0x60, capacity 2^24 bytes) is the id the flash of Espressif's QEMU answers, so the 2nd-stage
# bootloader takes the vendor branch the reference traces were measured on
# (docs/research/esp32c3-bootloader-accesses.md, section 2.1 item 5). `esptool.py flash_id` on a
# unit settles it.
jedec_id = 0x9D6018
```

`lib.rs`:

```rust
use soc_esp32c3::flash::{FlashConfig, FlashImageError, JedecId};

const MIB: u32 = 1 << 20;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    #[serde(deserialize_with = "profile_id")]
    id: String,
    name: String,
    soc: RawSoc,
    flash: RawFlash,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedFlash {
    #[serde(deserialize_with = "flash_size")]
    size_mib: u32,
    #[serde(deserialize_with = "jedec_id")]
    jedec_id: JedecId,
}

/// The `[flash]` table once its two keys agree with each other.
#[derive(Deserialize)]
#[serde(try_from = "UncheckedFlash")]
struct RawFlash(FlashConfig);

impl TryFrom<UncheckedFlash> for RawFlash {
    type Error = String;

    fn try_from(raw: UncheckedFlash) -> Result<RawFlash, String> {
        let config = FlashConfig { size: raw.size_mib * MIB, jedec_id: raw.jedec_id };
        // The third JEDEC byte is log2 of the capacity: a profile that contradicts itself would make
        // the bootloader and the flash driver disagree about the size.
        if raw.jedec_id.capacity_bytes() != Some(config.size) {
            return Err(format!(
                "the flash is {} MiB but the JEDEC id {:#08x} ends with capacity byte {:#04x}, which stands for another size; make size_mib and the last byte of jedec_id agree (0x14 = 1 MiB … 0x18 = 16 MiB)",
                raw.size_mib,
                raw.jedec_id.to_u24(),
                raw.jedec_id.capacity
            ));
        }
        Ok(RawFlash(config))
    }
}

/// A device id names files and appears in messages: lower-case letters, digits and `-` only.
fn profile_id<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let id = String::deserialize(deserializer)?;
    let valid = !id.is_empty() && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if valid {
        Ok(id)
    } else {
        Err(serde::de::Error::custom(format!(
            "the device id `{}` is empty or has characters other than a-z, 0-9 and `-`; write an id such as x4",
            id.escape_debug()
        )))
    }
}

/// `size_mib`, validated where the TOML error can still carry its line. The result is in MiB.
fn flash_size<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let mib = u32::deserialize(deserializer)?;
    let valid = mib
        .checked_mul(MIB)
        .is_some_and(|size| FlashConfig { size, jedec_id: JedecId::QEMU_ORACLE }.validate().is_ok());
    if valid {
        Ok(mib)
    } else {
        Err(serde::de::Error::custom(format!(
            "a flash of {mib} MiB cannot be modelled; write size_mib as 1, 2, 4, 8 or 16"
        )))
    }
}

fn jedec_id<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<JedecId, D::Error> {
    let raw = u32::deserialize(deserializer)?;
    JedecId::from_u24(raw).ok_or_else(|| {
        serde::de::Error::custom(format!(
            "the JEDEC id {raw:#x} does not fit three bytes; write it as 0xMMTTCC (manufacturer, type, capacity)"
        ))
    })
}
```

`raw.size_mib * MIB` cannot overflow: `flash_size` accepted at most 16 (write `// INVARIANT:` above it). `DeviceProfile` gains `pub flash: FlashConfig` (`flash: raw.flash.0` in `from_toml`), `soc_config` gains `flash: self.flash,`. `ProfileError` gains:

```rust
    #[error("{origin}: the profile declares the id `{found}` but is registered as `{expected}`; make the id of the profile match the name it is loaded under")]
    IdMismatch { origin: String, expected: String, found: String },
```

```rust
    /// Like [`DeviceProfile::from_toml`], and checks that the profile carries the id it is loaded under.
    ///
    /// # Errors
    /// [`ProfileError::Parse`], or [`ProfileError::IdMismatch`] when the ids differ.
    pub fn from_named_toml(expected_id: &str, origin: &str, text: &str) -> Result<DeviceProfile, ProfileError> {
        let profile = DeviceProfile::from_toml(origin, text)?;
        if profile.id == expected_id {
            Ok(profile)
        } else {
            Err(ProfileError::IdMismatch { origin: origin.to_owned(), expected: expected_id.to_owned(), found: profile.id })
        }
    }

    /// Builds the device in its power-on reset state with `image` in its flash from offset 0.
    ///
    /// # Errors
    /// [`FlashImageError`] when the image is longer than the flash of the device.
    pub fn instantiate_with_flash(&self, rom: RomImage, image: Vec<u8>, options: MachineOptions) -> Result<Esp32C3, FlashImageError> {
        match self.soc {
            SocFamily::Esp32C3 => Esp32C3::with_flash(rom, image, &self.soc_config(options)),
        }
    }
```

`builtin` calls `DeviceProfile::from_named_toml(id, &format!("built-in profile {id}"), text)`. In `emu-cli/src/main.rs`, `profile_for` matches the errors of `boards::builtin` exhaustively: widen its last arm to `other @ (boards::ProfileError::Parse { .. } | boards::ProfileError::IdMismatch { .. }) => other,` (one arm: a second arm with the same body is refused by clippy, `match_same_arms`).

- [ ] **Step 4: Run**

Run: `cargo test --workspace > /tmp/t13.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.
Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/t13c.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 5: Commit**

Before committing (Global Constraints): `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`: `exit=0` three times. The code blocks of this plan are not wrapped as `rustfmt` wraps them.

```bash
/usr/bin/git add crates/boards crates/emu-cli/src/main.rs
/usr/bin/git commit -m "feat(boards): flash size and JEDEC id in device profiles, validated ids

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/boards crates/emu-cli/src/main.rs
```

---

### Task 14: `firmware` — refusals for a full flash image, and the three SP1a points

**Files:**
- Create: `crates/firmware/src/flash_image.rs`
- Modify: `crates/firmware/src/{lib,chip,error,image,builder}.rs`, `crates/emu-cli/src/inspect.rs`

**Interfaces:**
- Consumes: `ChipId::from_raw`, `FlashSize::{from_nibble, megabytes, bytes}`, `SpiMode::from_raw`, `FirmwareError`, `PartitionTable::{parse, to_bytes}`, `xteink_x4_stock`, `PARTITION_TABLE_OFFSET`, `PARTITION_TABLE_LEN`, `HEADER_LEN`, `IMAGE_MAGIC`, `builder::{ImageSpec, build_image}` (existing).
- Produces:
  ```rust
  // flash_image.rs
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct FlashImageInfo { pub chip: ChipId, pub spi_mode: SpiMode, pub flash_size: FlashSize, pub entry: u32 }
  /// Spec §5.1: the refusals that precede the start of the machine, for a full flash image.
  pub fn check_flash_image(bytes: &[u8], device_chip: ChipId, device_flash_bytes: u64) -> Result<FlashImageInfo, FirmwareError>;
  /// Whether `bytes` holds a partition table at offset 0x8000 (entry magics, and its MD5 when present).
  pub fn has_partition_table(bytes: &[u8]) -> bool;
  // error.rs: new variants
  //   FirmwareError::NotAFlashImage { found: u8 }, FirmwareError::FlashImageTooShort { len: usize }, FirmwareError::NoPartitionTable
  // chip.rs
  impl std::fmt::Display for SpiMode  // "QIO", "QOUT", "DIO", "DOUT", "FAST_RD", "SLOW_RD"
  // builder.rs (module compiled for tests and with the feature `test-support`, like `build_image`)
  /// A full flash image: the image of `spec` at offset 0, erased bytes up to 0x8000, then the stock
  /// Xteink X4 partition table for a 16 MB flash (0x8C00 bytes). Panics when the image reaches 0x8000.
  pub fn build_flash_image(spec: &ImageSpec) -> Vec<u8>;
  ```
  `build_flash_image` is the one place a test gets a full flash image from: the tests of this task use it, and so do the `flash_file` tests and `tests/run_cli.rs` of Task 15 (`firmware::builder::build_flash_image`; `emu-cli` already has `firmware` with `test-support` in its dev-dependencies).

- [ ] **Step 1: Write the failing tests**

`builder.rs`, a new module `tests` at the end of the file (the file has none yet):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::partition::{PARTITION_TABLE_LEN, PartitionTable};

    #[test]
    fn a_flash_image_is_the_image_then_erased_bytes_then_the_stock_table() {
        let spec = ImageSpec::default();
        let image = build_image(&spec);
        let flash = build_flash_image(&spec);
        assert_eq!(flash.len(), 0x8000 + PARTITION_TABLE_LEN);
        assert_eq!(flash[..image.len()], image[..], "the image is at offset 0");
        assert!(flash[image.len()..0x8000].iter().all(|&byte| byte == 0xFF), "erased up to the table");
        assert_eq!(PartitionTable::parse(&flash[0x8000..]), Ok(xteink_x4_stock()));
    }

    #[test]
    #[should_panic(expected = "would overlap the partition table")]
    fn an_image_that_reaches_the_partition_table_is_refused() {
        let spec = ImageSpec { segments: vec![(0x4038_0000, vec![0; 0x8000])], ..ImageSpec::default() };
        let _ = build_flash_image(&spec);
    }
}
```

`flash_image.rs`, module `tests`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{ImageSpec, build_flash_image, build_image};

    const MIB16: u64 = 16 << 20;

    fn refusal(bytes: &[u8]) -> FirmwareError {
        check_flash_image(bytes, ChipId::Esp32C3, MIB16).unwrap_err()
    }

    #[test]
    fn a_merged_image_is_accepted_and_described() {
        let info = check_flash_image(&build_flash_image(&ImageSpec::default()), ChipId::Esp32C3, MIB16).unwrap();
        assert_eq!(info.chip, ChipId::Esp32C3);
        assert_eq!(info.spi_mode, SpiMode::Dio);
        assert_eq!(info.flash_size, FlashSize::Mb16);
        assert_eq!(info.entry, ImageSpec::default().entry);
    }

    #[test]
    fn an_encrypted_ota_file_is_refused() {
        let mut bytes = b"XOTA".to_vec();
        bytes.resize(0x9000, 0);
        assert_eq!(refusal(&bytes), FirmwareError::EncryptedOta);
    }

    #[test]
    fn a_file_that_does_not_start_with_an_image_is_refused_with_the_byte_found() {
        let mut bytes = build_flash_image(&ImageSpec::default());
        bytes[0] = 0x7F;
        let err = refusal(&bytes);
        assert_eq!(err, FirmwareError::NotAFlashImage { found: 0x7F });
        let text = err.to_string();
        assert!(text.contains("0x7f") && text.contains("0xe9") && text.contains("; use"), "{text}");
        assert_eq!(refusal(&[0xE9; 10]), FirmwareError::FlashImageTooShort { len: 10 });
        assert_eq!(refusal(&[]), FirmwareError::FlashImageTooShort { len: 0 });
    }

    #[test]
    fn an_image_for_another_chip_is_refused() {
        let bytes = build_flash_image(&ImageSpec { chip: 9, ..ImageSpec::default() });
        assert_eq!(refusal(&bytes), FirmwareError::ChipMismatch { expected: ChipId::Esp32C3, found: ChipId::Esp32S3 });
    }

    #[test]
    fn an_image_that_declares_more_flash_than_the_device_has_is_refused() {
        let bytes = build_flash_image(&ImageSpec { flash_size_nibble: 5, ..ImageSpec::default() });
        assert_eq!(refusal(&bytes), FirmwareError::FlashTooSmall { needed: 32, available: 16 });
        let fits = build_flash_image(&ImageSpec { flash_size_nibble: 2, ..ImageSpec::default() });
        assert!(check_flash_image(&fits, ChipId::Esp32C3, MIB16).is_ok(), "4 MB on a 16 MB device");
    }

    #[test]
    fn an_application_or_bootloader_alone_is_not_a_flash_image() {
        let app = build_image(&ImageSpec::default());
        let err = refusal(&app);
        assert_eq!(err, FirmwareError::NoPartitionTable);
        let text = err.to_string();
        assert!(text.contains("0x8000") && text.contains("merge_bin"), "{text}");
    }

    #[test]
    fn a_table_is_recognised_by_its_content_not_by_two_bytes() {
        let full = build_flash_image(&ImageSpec::default());
        assert!(has_partition_table(&full));
        // The two magic bytes alone, followed by something that is not an entry: SP1a's false positive.
        let mut coincidence = build_image(&ImageSpec::default());
        coincidence.resize(0x9000, 0x00);
        coincidence[0x8000] = 0xAA;
        coincidence[0x8001] = 0x50;
        assert!(!has_partition_table(&coincidence));
        // `full` is 0x8C00 bytes (0x8000 + the 0xC00 bytes `to_bytes` returns): it already ends inside
        // the table sector, which SP1a's length rule refused below 0x9000. A dump cut right after the
        // six entries and their MD5 entry (0x80E0) is recognised too.
        assert_eq!(full.len(), 0x8C00);
        let mut cut = full.clone();
        cut.truncate(0x8100);
        assert!(has_partition_table(&cut));
        assert!(!has_partition_table(&full[..0x8000]));
        // A table whose MD5 does not match its entries is not a table.
        let mut corrupted = full.clone();
        corrupted[0x8004] ^= 0xFF;
        assert!(!has_partition_table(&corrupted));
    }
}
```

`chip.rs`, module `tests`:

```rust
    #[test]
    fn spi_modes_print_the_names_the_rom_prints() {
        // The ROM's own table (report FB §G): QIO, QOUT, DIO, DOUT, FAST_RD, SLOW_RD.
        let names: Vec<String> = (0..6).map(|raw| SpiMode::from_raw(raw).unwrap().to_string()).collect();
        assert_eq!(names, ["QIO", "QOUT", "DIO", "DOUT", "FAST_RD", "SLOW_RD"]);
    }
```

`image.rs`, module `tests`: the helper `flash_image(len)` builds a fake table from two magic bytes; make it build a real one with `build_flash_image` (the import of the module becomes `use crate::builder::{ImageSpec, build_flash_image, build_image};`). Of the three tests that use it, `refuses_a_full_flash_image_instead_of_describing_its_bootloader` and `a_large_application_image_without_a_partition_table_magic_is_unaffected` pass unchanged with the new helper (overwriting the first two table bytes with `FF FF` makes the table empty, hence not a table), and the third is replaced:

```rust
    fn flash_image(len: usize) -> Vec<u8> {
        let mut b = build_flash_image(&ImageSpec::default());
        b.resize(len, 0xFF);
        b
    }

    #[test]
    fn flash_image_detection_needs_a_table_not_a_file_length() {
        assert_eq!(FirmwareImage::parse(&flash_image(0x8C00)), Err(FirmwareError::FlashImage), "a dump cut in the table sector");
        let mut coincidence = build_image(&ImageSpec::default());
        let len = coincidence.len();
        coincidence.resize(0x9000, 0);
        coincidence[0x8000] = 0xAA;
        coincidence[0x8001] = 0x50;
        // Not a table: the file is parsed as the application it starts with (its trailing bytes are ignored).
        assert!(FirmwareImage::parse(&coincidence).is_ok(), "image of {len} bytes with AA 50 at 0x8000");
    }
```

(delete `flash_image_detection_starts_at_the_end_of_the_partition_table_sector`, which pinned the old length rule). `FirmwareImage::parse` does not look past the checksum and the hash (it reads the segments, then `verify_trailer`, and never checks the file length), so the trailing bytes of `coincidence` are accepted and the last assertion holds as written.

`error.rs`: add the three new variants to `one_of_each` and `variant_index` (indices 15, 16 and 17; the exhaustive `match` forces it), change `let mut seen = [false; 15];` to `[false; 18]` in `one_of_each` (it is indexed by `variant_index`: left at 15 the test panics on index 15), and:

```rust
    #[test]
    fn an_unknown_flash_size_code_says_what_is_known_and_what_to_do() {
        let text = FirmwareError::BadFlashSize { raw: 9 }.to_string();
        assert!(text.contains("code 9") && text.contains("0 to 7") && text.contains("128 MB"), "{text}");
        assert!(text.contains("; re-download the file, or use an image built for a flash of at most 128 MB"), "{text}");
    }
```

- [ ] **Step 2: Run, see the failure**

Run: `cargo test -p firmware > /tmp/t14.log 2>&1; echo "exit=$?"`
Expected: `exit=101`.

- [ ] **Step 3: Implement**

`chip.rs`:

```rust
impl fmt::Display for SpiMode {
    /// The names the mask ROM prints in its `mode:` line (`docs/research/esp32c3-rom-flash-boot.md`, §G).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SpiMode::Qio => "QIO",
            SpiMode::Qout => "QOUT",
            SpiMode::Dio => "DIO",
            SpiMode::Dout => "DOUT",
            SpiMode::FastRead => "FAST_RD",
            SpiMode::SlowRead => "SLOW_RD",
        })
    }
}
```

`error.rs`: replace the message of `BadFlashSize` and add the variants (each keeps the remedy convention: `"; "` then a verb of `REMEDY_VERBS`):

```rust
    #[error(
        "unknown flash size code {raw} (inkemu knows 0 to 7, that is 1 MB to 128 MB): the header is damaged, or the image targets a larger flash; re-download the file, or use an image built for a flash of at most 128 MB"
    )]
    BadFlashSize { raw: u8 },
    #[error(
        "the file does not start with a bootloader image: its first byte is {found:#04x}, expected 0xe9; use a full flash dump, or an image merged with `esptool.py merge_bin` (bootloader at offset 0)"
    )]
    NotAFlashImage { found: u8 },
    #[error(
        "the file is empty or shorter than an image header ({len} bytes); use a full flash dump, or an image merged with `esptool.py merge_bin`"
    )]
    FlashImageTooShort { len: usize },
    #[error(
        "the file starts with an image but holds no partition table at offset 0x8000, so it is an application or a bootloader alone, not a full flash image; use a flash dump or the output of `esptool.py merge_bin` (bootloader, partition table and application)"
    )]
    NoPartitionTable,
```

`image.rs`: make `XOTA_MAGIC`, `le_u16` and `le_u32` `pub(crate)`, and replace `is_flash_image` by a call to the shared rule:

```rust
        if crate::flash_image::has_partition_table(bytes) {
            return Err(FirmwareError::FlashImage);
        }
```

(delete the function `is_flash_image` and the imports only it used).

`flash_image.rs`:

```rust
//! Checks of a full flash image (bootloader, partition table, application) before a machine is
//! started on it: the refusals of spec §5.1.
use crate::chip::{ChipId, FlashSize, SpiMode};
use crate::error::FirmwareError;
use crate::image::{HEADER_LEN, IMAGE_MAGIC, XOTA_MAGIC, le_u16, le_u32};
use crate::partition::{PARTITION_TABLE_LEN, PARTITION_TABLE_OFFSET, PartitionTable};

/// Offsets in the image header (`esp_image_header_t`): SPI mode, flash size and speed, entry point,
/// chip id in the extended header.
const SPI_MODE_OFFSET: usize = 2;
const SIZE_FREQ_OFFSET: usize = 3;
const ENTRY_OFFSET: usize = 4;
const CHIP_ID_OFFSET: usize = 12;
const BYTES_PER_MB: u64 = 1024 * 1024;

/// What the header of the bootloader at offset 0 declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlashImageInfo {
    pub chip: ChipId,
    pub spi_mode: SpiMode,
    pub flash_size: FlashSize,
    pub entry: u32,
}

/// Whether `bytes` holds a partition table at offset 0x8000: entries with the entry magic, ended by
/// an MD5 entry that matches them or by erased bytes. A dump cut inside the table sector counts.
#[must_use]
pub fn has_partition_table(bytes: &[u8]) -> bool {
    let start = PARTITION_TABLE_OFFSET as usize;
    let end = bytes.len().min(start + PARTITION_TABLE_LEN);
    bytes.get(start..end).is_some_and(|table| PartitionTable::parse(table).is_ok())
}

/// Checks that `bytes` can be the flash content of a device with the chip `device_chip` and
/// `device_flash_bytes` of flash. Only the header of the bootloader is read: the ROM and the
/// bootloader validate the rest when they run, as on the device.
///
/// # Errors
/// [`FirmwareError::EncryptedOta`], [`FirmwareError::FlashImageTooShort`],
/// [`FirmwareError::NotAFlashImage`], [`FirmwareError::BadSpiMode`], [`FirmwareError::BadFlashSize`],
/// [`FirmwareError::ChipMismatch`], [`FirmwareError::FlashTooSmall`] or [`FirmwareError::NoPartitionTable`].
pub fn check_flash_image(bytes: &[u8], device_chip: ChipId, device_flash_bytes: u64) -> Result<FlashImageInfo, FirmwareError> {
    if bytes.starts_with(XOTA_MAGIC) {
        return Err(FirmwareError::EncryptedOta);
    }
    if bytes.len() < HEADER_LEN {
        return Err(FirmwareError::FlashImageTooShort { len: bytes.len() });
    }
    // INVARIANT: every index below is under HEADER_LEN (24), which the length check just covered.
    if bytes[0] != IMAGE_MAGIC {
        return Err(FirmwareError::NotAFlashImage { found: bytes[0] });
    }
    let spi_mode = SpiMode::from_raw(bytes[SPI_MODE_OFFSET]).ok_or(FirmwareError::BadSpiMode { raw: bytes[SPI_MODE_OFFSET] })?;
    let size_code = bytes[SIZE_FREQ_OFFSET] >> 4;
    let flash_size = FlashSize::from_nibble(size_code).ok_or(FirmwareError::BadFlashSize { raw: size_code })?;
    let chip = ChipId::from_raw(le_u16(bytes, CHIP_ID_OFFSET));
    if chip != device_chip {
        return Err(FirmwareError::ChipMismatch { expected: device_chip, found: chip });
    }
    if flash_size.bytes() > device_flash_bytes {
        return Err(FirmwareError::FlashTooSmall {
            needed: flash_size.megabytes(),
            available: u32::try_from(device_flash_bytes / BYTES_PER_MB).unwrap_or(u32::MAX),
        });
    }
    if !has_partition_table(bytes) {
        return Err(FirmwareError::NoPartitionTable);
    }
    Ok(FlashImageInfo { chip, spi_mode, flash_size, entry: le_u32(bytes, ENTRY_OFFSET) })
}
```

`lib.rs`: `pub mod flash_image;` and `pub use flash_image::{FlashImageInfo, check_flash_image, has_partition_table};`.

`builder.rs`: add `use crate::chip::FlashSize;` and `use crate::partition::{PARTITION_TABLE_OFFSET, xteink_x4_stock};` to the imports, and after `build_image`:

```rust
/// An erased flash byte, which fills the gap between the image and the partition table.
const ERASED: u8 = 0xFF;

/// A full flash image: the image of `spec` at offset 0, where the bootloader goes, erased bytes up
/// to the partition table at `0x8000`, then the stock Xteink X4 table for a 16 MB flash. The result
/// is `0x8C00` bytes long; what a real flash holds after the table reads erased.
///
/// # Panics
/// When the image of `spec` does not fit below the partition table.
#[must_use]
pub fn build_flash_image(spec: &ImageSpec) -> Vec<u8> {
    let table_offset = PARTITION_TABLE_OFFSET as usize;
    let mut flash = build_image(spec);
    assert!(
        flash.len() <= table_offset,
        "the image is {} bytes and would overlap the partition table at {table_offset:#x}; use smaller segments",
        flash.len()
    );
    flash.resize(table_offset, ERASED);
    let Ok(table) = xteink_x4_stock().to_bytes(FlashSize::Mb16) else {
        // INVARIANT: the stock table is a fixed layout that is valid for a 16 MB flash.
        unreachable!("the stock partition table is valid for a 16 MB flash")
    };
    flash.extend(table);
    flash
}
```

The module is not test code for the lints (it is also compiled with the feature `test-support`), hence no `unwrap`: the `let … else` with its `// INVARIANT:` comment is the form the workspace lints accept. `PARTITION_TABLE_OFFSET as usize` is a widening cast (`u32` to `usize`).

`emu-cli/src/inspect.rs`: in `render`, print the SPI mode with `{}` instead of `{:?}` (the line becomes `Flash:       16 MB, DIO, frequency code 0xf`). No existing test pins that line (`grep -rn 'Dio' crates/emu-cli` finds nothing but the format string itself; `tests/inspect_cli.rs` does not check the flash line), so write the test first, in the `tests` module of `inspect.rs`: it fails with `Dio` before the change and passes after it.

```rust
    #[test]
    fn the_flash_line_names_the_spi_mode_as_the_rom_does() {
        use firmware::builder::{ImageSpec, build_image};
        let bytes = build_image(&ImageSpec::default());
        let text = render(&FirmwareImage::parse(&bytes).unwrap(), "app.bin", bytes.len());
        assert!(text.contains("Flash:       16 MB, DIO, frequency code 0xf\n"), "{text}");
    }
```

(`firmware` is a dev-dependency of `emu-cli` with its `test-support` feature, which exposes `builder`.)

- [ ] **Step 4: Run**

Run: `cargo test --workspace > /tmp/t14.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.
Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/t14c.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 5: Commit**

Before committing (Global Constraints): `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`: `exit=0` three times. The code blocks of this plan are not wrapped as `rustfmt` wraps them.

```bash
/usr/bin/git add crates/firmware crates/emu-cli/src/inspect.rs
/usr/bin/git commit -m "feat(firmware): pre-start checks of a full flash image; partition-table detection by content

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/firmware crates/emu-cli/src/inspect.rs
```

---

### Task 15: `inkemu run --flash`, and a single pass over the ROM archive

**Files:**
- Create: `crates/emu-cli/src/flash_file.rs`
- Modify: `crates/emu-cli/src/{lib,main,exit,rom_store}.rs`, `crates/emu-cli/tests/run_cli.rs`

**Interfaces:**
- Consumes: `firmware::{check_flash_image, ChipId, FirmwareError}` (Task 14); `firmware::builder::build_flash_image(spec: &ImageSpec) -> Vec<u8>` (Task 14; tests only: it is the one helper that makes a full flash image, do not write another); `DeviceProfile::{flash, instantiate, instantiate_with_flash}` (Task 13); `FlashImageError` (Task 4); `rom_store` internals (`download`, `check_elf`, `store`, `read_present`, `Provenance::Archive`, `RomStoreError::{Archive, MissingInArchive}`, `MAX_ELF_BYTES`, `sanitize`).
- Produces:
  ```rust
  // flash_file.rs
  #[derive(Debug, thiserror::Error)]
  pub enum FlashFileError { Read { path: String, detail: String }, NotAFile { path: String }, Empty { path: String }, TooLong { path: String, len: u64, size: u32 }, Refused { path: String, source: firmware::FirmwareError } }
  /// Reads `path` (at most `flash_size` bytes) and applies the spec §5.1 refusals for an ESP32-C3 device.
  pub fn load(path: &Path, flash_size: u32) -> Result<Vec<u8>, FlashFileError>;
  ```
  CLI: `inkemu run --device <id> [--flash <file>] …`. Without `--flash` the flash is erased. Exit code 2 for a refused flash file.

- [ ] **Step 1: Write the failing tests**

`flash_file.rs`, module `tests`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use firmware::builder::{ImageSpec, build_flash_image};

    const MIB16: u32 = 16 << 20;

    fn file_with(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flash.bin");
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }

    #[test]
    fn a_merged_image_is_loaded_as_it_is() {
        let image = build_flash_image(&ImageSpec::default());
        let (_dir, path) = file_with(&image);
        assert_eq!(load(&path, MIB16).unwrap(), image);
    }

    #[test]
    fn a_missing_file_says_which_and_what_to_check() {
        let err = load(Path::new("/nonexistent/flash.bin"), MIB16).unwrap_err().to_string();
        assert!(err.contains("/nonexistent/flash.bin") && err.contains("; check the path"), "{err}");
    }

    #[test]
    fn empty_file_and_directory_are_refused() {
        let (dir, path) = file_with(&[]);
        let err = load(&path, MIB16).unwrap_err();
        assert!(matches!(err, FlashFileError::Empty { .. }), "{err}");
        assert!(err.to_string().contains("; use a full flash dump"), "{err}");
        let err = load(dir.path(), MIB16).unwrap_err();
        assert!(matches!(err, FlashFileError::NotAFile { .. }), "{err}");
    }

    #[test]
    fn image_longer_than_the_flash_is_refused_with_both_sizes() {
        // A sparse file one byte longer than the flash: nothing that large is read into memory.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.bin");
        std::fs::File::create(&path).unwrap().set_len(u64::from(MIB16) + 1).unwrap();
        let err = load(&path, MIB16).unwrap_err();
        assert!(matches!(err, FlashFileError::TooLong { len: 16_777_217, size: MIB16, .. }), "{err}");
        let text = err.to_string();
        assert!(text.contains("16777217") && text.contains("16777216") && text.contains("; use"), "{text}");
    }

    #[test]
    fn a_refusal_of_the_content_names_the_file_and_keeps_the_reason() {
        let (_dir, path) = file_with(&build_flash_image(&ImageSpec { chip: 9, ..ImageSpec::default() }));
        let err = load(&path, MIB16).unwrap_err();
        assert!(matches!(err, FlashFileError::Refused { source: firmware::FirmwareError::ChipMismatch { .. }, .. }), "{err}");
        let text = err.to_string();
        assert!(text.contains("flash.bin") && text.contains("ESP32-S3") && text.contains("ESP32-C3"), "{text}");
    }

    #[test]
    fn a_path_with_control_characters_is_shown_escaped() {
        let err = load(Path::new("/nonexistent/\u{1b}[31mred.bin"), MIB16).unwrap_err().to_string();
        assert!(!err.contains('\u{1b}'), "{err:?}");
    }
}
```

`rom_store.rs`, module `tests` (the fixture `trio()` already returns the archive body, the `RomArchive`, three `RomEntry` and their three ELF contents):

```rust
    #[test]
    fn every_wanted_elf_is_extracted_in_one_pass_whatever_their_order() {
        let (body, archive, entries, elfs) = trio();
        let wanted = [&entries[2], &entries[0], &entries[1]];
        let extracted = extract_entries(&body, &archive, &wanted).unwrap();
        assert_eq!(extracted, [elfs[2].clone(), elfs[0].clone(), elfs[1].clone()]);
        assert_eq!(extract_entries(&body, &archive, &[]).unwrap(), Vec::<Vec<u8>>::new());
    }

    #[test]
    fn a_wanted_elf_the_archive_lacks_is_named() {
        let (body, archive, entries, _) = trio();
        let absent = RomEntry { elf_name: "esp32c3_rev999_rom.elf", ..entries[0] };
        let err = extract_entries(&body, &archive, &[&entries[0], &absent]).unwrap_err();
        assert!(
            matches!(err, RomStoreError::MissingInArchive { elf_name: "esp32c3_rev999_rom.elf", .. }),
            "{err}"
        );
    }
```

`RomEntry` is `Copy` (`#[derive(Clone, Copy, Debug)]`), so `..entries[0]` compiles as written. No existing test calls `extract_entry` or `extract_and_store` directly (they all go through `load_elf`, `ensure_elfs` or `load_rom`): none has to change.

`crates/emu-cli/tests/run_cli.rs`:

```rust
use firmware::builder::{ImageSpec, build_flash_image, build_image};

/// Runs `inkemu run --device x4 --flash <file with bytes>` with a ROM directory that holds nothing:
/// a refusal of the flash file must come before any attempt to load a ROM.
fn run_with_flash(bytes: &[u8]) -> assert_cmd::assert::Assert {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("flash.bin");
    std::fs::write(&path, bytes).unwrap();
    inkemu()
        .args(["run", "--device", "x4", "--max-cycles", "1000", "--flash"])
        .arg(&path)
        .arg("--rom-dir")
        .arg(dir.path())
        .assert()
}

#[test]
fn xota_is_refused_before_boot() {
    let mut bytes = b"XOTA".to_vec();
    bytes.resize(0x9000, 0);
    run_with_flash(&bytes)
        .code(2)
        .stderr(contains("flash.bin").and(contains("encrypted OTA")).and(contains("; use")));
}

#[test]
fn a_file_without_the_image_magic_is_refused_before_boot() {
    run_with_flash(&vec![0u8; 0x9000])
        .code(2)
        .stderr(contains("first byte is 0x00").and(contains("expected 0xe9")).and(contains("merge_bin")));
}

#[test]
fn image_for_another_chip_is_refused() {
    run_with_flash(&build_flash_image(&ImageSpec { chip: 9, ..ImageSpec::default() }))
        .code(2)
        .stderr(contains("ESP32-S3").and(contains("ESP32-C3")).and(contains("; choose")));
}

#[test]
fn image_declaring_more_flash_than_the_device_is_refused() {
    run_with_flash(&build_flash_image(&ImageSpec { flash_size_nibble: 5, ..ImageSpec::default() }))
        .code(2)
        .stderr(contains("32 MB").and(contains("16 MB")));
}

#[test]
fn an_application_image_given_as_flash_is_refused() {
    run_with_flash(&build_image(&ImageSpec::default()))
        .code(2)
        .stderr(contains("no partition table at offset 0x8000").and(contains("merge_bin")));
}

#[test]
fn a_flash_file_that_does_not_exist_exits_2() {
    inkemu()
        .args(["run", "--device", "x4", "--flash", "/nonexistent/flash.bin"])
        .assert()
        .code(2)
        .stderr(contains("/nonexistent/flash.bin").and(contains("; check the path")));
}

#[test]
fn run_help_documents_the_flash_option_and_what_strict_stops_on() {
    inkemu()
        .args(["run", "--help"])
        .assert()
        .code(0)
        .stdout(contains("--flash").and(contains("erased")).and(contains("flash that is not mapped")));
}

/// The real ROM on a full flash image, through the real command.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn x4_boots_a_flash_image_through_the_real_rom() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let stub = ImageSpec {
        entry: 0x4038_0000,
        segments: vec![(0x4038_0000, vec![0x6f, 0, 0, 0])],
        ..ImageSpec::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("flash.bin");
    std::fs::write(&path, build_flash_image(&stub)).unwrap();
    let output = inkemu()
        .args(["run", "--device", "x4", "--strict-mmio", "--max-cycles", "5000000", "--rom-dir", &rom_dir, "--flash"])
        .arg(&path)
        .assert()
        .code(0)
        .get_output()
        .clone();
    let console = String::from_utf8_lossy(&output.stdout).replace('\r', "");
    assert!(
        console.ends_with("SPIWP:0xee\nmode:DIO, clock div:1\nload:0x40380000,len:0x4\nentry 0x40380000\n"),
        "{console}"
    );
    let log = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(log.contains("flash: flash.bin") && log.contains("16 MiB") && !log.contains("warning:"), "{log}");
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn x4_without_a_flash_image_loops_on_the_invalid_header_of_an_erased_chip() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let output = inkemu()
        .args(["run", "--device", "x4", "--strict-mmio", "--max-cycles", "3000000", "--rom-dir", &rom_dir])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let console = String::from_utf8_lossy(&output.stdout).replace('\r', "");
    assert!(console.matches("invalid header: 0xffffffff\n").count() >= 3, "{console}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("flash: erased"), "the header line says so");
}
```

- [ ] **Step 2: Run, see the failure**

Run: `cargo test -p emu-cli > /tmp/t15.log 2>&1; echo "exit=$?"`
Expected: `exit=101`.

- [ ] **Step 3: Implement `flash_file.rs`**

```rust
//! Reads the file given to `inkemu run --flash` and refuses, before any machine exists, what cannot
//! be the flash content of the device (spec §5.1).
use crate::term::sanitize;
use firmware::{ChipId, FirmwareError};
use std::io::Read;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum FlashFileError {
    #[error("cannot read the flash image {path}: {detail}; check the path and its read permissions")]
    Read { path: String, detail: String },
    #[error("the flash image {path} is not a regular file; use the path of a flash dump or of a merged image")]
    NotAFile { path: String },
    #[error("the flash image {path} is empty; use a full flash dump, or an image merged with `esptool.py merge_bin`")]
    Empty { path: String },
    #[error(
        "the flash image {path} is {len} bytes, longer than the {size} byte flash of the device; use an image built for this flash size, or a device whose flash is large enough"
    )]
    TooLong { path: String, len: u64, size: u32 },
    #[error("{path}: {source}")]
    Refused {
        path: String,
        #[source]
        source: FirmwareError,
    },
}

/// The content of the regular file `path`, which must hold at most `flash_size` bytes and pass the
/// checks of a full ESP32-C3 flash image. A path that is not a regular file is refused without being
/// opened (a pipe would block), and no more than `flash_size + 1` bytes are ever read.
///
/// # Errors
/// [`FlashFileError`], whose message names the file, the cause and the remedy.
pub fn load(path: &Path, flash_size: u32) -> Result<Vec<u8>, FlashFileError> {
    let shown = sanitize(&path.display().to_string());
    let read_error = |e: &std::io::Error| FlashFileError::Read { path: shown.clone(), detail: sanitize(&e.to_string()) };
    let metadata = std::fs::metadata(path).map_err(|e| read_error(&e))?;
    if !metadata.is_file() {
        return Err(FlashFileError::NotAFile { path: shown });
    }
    if metadata.len() > u64::from(flash_size) {
        return Err(FlashFileError::TooLong { path: shown, len: metadata.len(), size: flash_size });
    }
    let file = std::fs::File::open(path).map_err(|e| read_error(&e))?;
    let mut bytes = Vec::new();
    // One byte past the flash: a file that grew since the metadata was read is still caught.
    file.take(u64::from(flash_size) + 1).read_to_end(&mut bytes).map_err(|e| read_error(&e))?;
    let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if len > u64::from(flash_size) {
        return Err(FlashFileError::TooLong { path: shown, len, size: flash_size });
    }
    if bytes.is_empty() {
        return Err(FlashFileError::Empty { path: shown });
    }
    firmware::check_flash_image(&bytes, ChipId::Esp32C3, u64::from(flash_size))
        .map_err(|source| FlashFileError::Refused { path: shown, source })?;
    Ok(bytes)
}
```

`lib.rs`: `pub mod flash_file;`.

- [ ] **Step 4: The `--flash` option** (`main.rs`, `exit.rs`)

Add to `Command::Run`, after `device`:

```rust
        /// Full flash image to boot (a flash dump, or the output of `esptool.py merge_bin`): its
        /// bootloader, partition table and application run as on the device. Without it the flash
        /// is erased and the ROM reports an invalid header, as a blank chip does.
        #[arg(long)]
        flash: Option<PathBuf>,
```

Reword the doc comment of `strict_mmio`: "Stop at the first access to a register the emulator does not model, 8/16-bit write to a peripheral register, operation the emulator cannot perform faithfully, or access to flash that is not mapped (exit code 3)." Pass `flash` to `run_device(&device, flash.as_deref(), rom_dir, …)`. In `run_device`, **before** the ROM is located:

```rust
    // The flash file is checked first: refusing it needs no ROM, and no download should start for a
    // run that cannot happen.
    let flash_image = match flash {
        Some(path) => match flash_file::load(path, profile.flash.size) {
            Ok(bytes) => Some((path, bytes)),
            Err(e) => return fail(&e),
        },
        None => None,
    };
```

and, once the ROM is loaded:

```rust
    let flash_note = match &flash_image {
        Some((path, bytes)) => format!(
            "flash: {} ({} bytes in {} MiB)",
            sanitize(&path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())),
            bytes.len(),
            profile.flash.size >> 20
        ),
        None => format!("flash: erased ({} MiB)", profile.flash.size >> 20),
    };
    let mut machine = match flash_image {
        Some((_, bytes)) => match profile.instantiate_with_flash(rom, bytes, machine_options) {
            Ok(machine) => machine,
            Err(e) => return fail(&e),
        },
        None => profile.instantiate(rom, machine_options),
    };
    eprintln!("{}; {flash_note}", header(&profile, entry, &machine));
```

(`use emu_cli::flash_file;`). In `exit.rs`, the help becomes:

```rust
pub const RUN_EXIT_CODES_HELP: &str = "\
Exit codes:
  0  the run ended normally: the cycle limit was reached, or the reader of standard output went away
  2  error: unknown device, missing or invalid ROM, refused flash image, or standard output cannot be written
  3  stopped by --strict-mmio (an unmodelled register, an 8/16-bit register write, an operation the
     emulator cannot perform faithfully, or flash that is not mapped)
  4  the CPU is waiting for an interrupt and nothing can raise one (interrupts are not emulated yet)";
```

- [ ] **Step 5: One pass over the ROM archive** (`rom_store.rs`)

Replace `extract_entry` and `extract_and_store` by:

```rust
/// Reads the entries named exactly like `wanted` out of the gzip-compressed tar `body`, in one pass
/// over the archive, in memory. The result has the order of `wanted`.
///
/// No other entry is looked at and no entry name is used as a path.
fn extract_entries(body: &[u8], archive: &RomArchive, wanted: &[&RomEntry]) -> Result<Vec<Vec<u8>>, RomStoreError> {
    let read_error = |e: &std::io::Error| RomStoreError::Archive {
        url: sanitize(archive.url),
        detail: sanitize(&e.to_string()),
    };
    let mut found: Vec<Option<Vec<u8>>> = vec![None; wanted.len()];
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(body));
    for member in tar.entries().map_err(|e| read_error(&e))? {
        if found.iter().all(Option::is_some) {
            break;
        }
        let member = member.map_err(|e| read_error(&e))?;
        if !member.header().entry_type().is_file() {
            continue;
        }
        let name = member.path_bytes();
        // The first member with a wanted name wins, as before.
        let Some(index) = wanted
            .iter()
            .zip(&found)
            .position(|(entry, slot)| slot.is_none() && name.as_ref() == entry.elf_name.as_bytes())
        else {
            continue;
        };
        if member.size() > MAX_ELF_BYTES {
            return Err(RomStoreError::Archive {
                url: sanitize(archive.url),
                detail: format!(
                    "the entry `{}` is {} bytes, above the {MAX_ELF_BYTES} byte (4 MiB) limit of a ROM ELF",
                    wanted[index].elf_name,
                    member.size()
                ),
            });
        }
        drop(name);
        let mut bytes = Vec::new();
        member.take(MAX_ELF_BYTES).read_to_end(&mut bytes).map_err(|e| read_error(&e))?;
        found[index] = Some(bytes);
    }
    wanted
        .iter()
        .zip(found)
        .map(|(entry, bytes)| {
            bytes.ok_or_else(|| RomStoreError::MissingInArchive { url: archive.url.to_owned(), elf_name: entry.elf_name })
        })
        .collect()
}
```

`wanted[index]` and `found[index]` are in bounds: `index` comes from `position` over the same two slices (write `// INVARIANT:`). In `ensure_elfs`, replace the loop that called `extract_and_store` once per missing ELF by:

```rust
    let missing: Vec<&RomEntry> = entries.iter().zip(&elfs).filter(|(_, elf)| elf.is_none()).map(|(entry, _)| *entry).collect();
    // One decompression of the archive, however many ELF files are missing.
    let mut extracted = extract_entries(&body, archive, &missing)?.into_iter();
    for (slot, entry) in elfs.iter_mut().zip(entries) {
        if slot.is_none()
            && let Some(elf) = extracted.next()
        {
            check_elf(&elf, entry, &source.dir().join(entry.elf_name), &Provenance::Archive)?;
            store(source.dir(), entry.elf_name, &elf)?;
            *slot = Some(elf);
        }
    }
```

(`missing` and the loop walk `entries` in the same order, so the n-th extracted ELF belongs to the n-th empty slot.) `load_elf` does not change: it calls `ensure_elfs` with one entry and never touched `extract_and_store`, whose only caller was the loop replaced above.

- [ ] **Step 6: Run**

Run: `cargo test --workspace > /tmp/t15.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.
Run: `INKEMU_ROM_ELF_DIR=/tmp/inkemu-rom cargo test --release -p emu-cli --test run_cli -- --ignored > /tmp/t15rom.log 2>&1; echo "exit=$?"`
Expected: `exit=0` (the two new real-ROM tests and the existing ones).
Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/t15c.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 7: Commit**

Before committing (Global Constraints): `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`: `exit=0` three times. The code blocks of this plan are not wrapped as `rustfmt` wraps them.

```bash
/usr/bin/git add crates/emu-cli
/usr/bin/git commit -m "feat(cli): inkemu run --flash with pre-start refusals; one pass over the ROM archive

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/emu-cli
```

---

### Task 16: Reference firmware (built, never committed), its CI job, and the milestone tests

**Files:**
- Create: `tools/reference-firmware/build.sh`, `tools/reference-firmware/in-container.sh`, `tools/reference-firmware/sdkconfig.defaults`, `tools/reference-firmware/SHA256SUMS.inputs`, `tools/reference-firmware/SHA256SUMS.output`, `tools/reference-firmware/README.md`
- Create: `crates/soc-esp32c3/tests/reference_boot.rs`
- Modify: `crates/emu-cli/tests/run_cli.rs`, `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: everything above; `common::hex` of `crates/soc-esp32c3/tests/common/mod.rs` (Task 0). `tools/conformance/versions.env` (`ESP_IDF_VERSION=v5.5.2`), `tools/docker/esp-idf/Dockerfile` (`FROM espressif/idf:${ESP_IDF_VERSION}`).
- Produces: `tools/reference-firmware/build.sh [output-dir]` → `<output-dir>/flash_hello.bin` (16,777,216 bytes, SHA-256 `fc05ef5dbe85372e1da52abdec3bfcbbb110738a60163734666095fcd8c39bf2`) plus `bootloader.nm` and `hello_world.nm` (symbol tables, text). Environment variable `INKEMU_REF_FLASH` = path of `flash_hello.bin` for the ignored tests. CI job `reference-boot`.

Pins (TO §2): `bootloader.bin` 21,040 bytes `601b48456b0a9d5c9f1beac166135e06686d3c5ed0309dd16840ab39887afaa3`; `partition-table.bin` 3,072 bytes `7f00b6c042a89b15b0cac534f82ed988caf29278ff5700b0c511eb1b5bb7c820`; `hello_world.bin` 136,160 bytes `4b82d73e37d10969d660eba95984cbe7f053212d39306cc54bfce6c366d0f04b`.

- [ ] **Step 1: The build files**

`tools/reference-firmware/sdkconfig.defaults` (TO §2, verbatim):

```
CONFIG_IDF_TARGET="esp32c3"
CONFIG_ESPTOOLPY_FLASHSIZE_16MB=y
CONFIG_ESPTOOLPY_FLASHMODE_DIO=y
CONFIG_ESPTOOLPY_FLASHFREQ_80M=y
CONFIG_ESP_DEFAULT_CPU_FREQ_MHZ_160=y
CONFIG_APP_REPRODUCIBLE_BUILD=y
```

`tools/reference-firmware/SHA256SUMS.inputs` (paths relative to the ESP-IDF `build` directory):

```
601b48456b0a9d5c9f1beac166135e06686d3c5ed0309dd16840ab39887afaa3  bootloader/bootloader.bin
7f00b6c042a89b15b0cac534f82ed988caf29278ff5700b0c511eb1b5bb7c820  partition_table/partition-table.bin
4b82d73e37d10969d660eba95984cbe7f053212d39306cc54bfce6c366d0f04b  hello_world.bin
```

`tools/reference-firmware/SHA256SUMS.output`:

```
fc05ef5dbe85372e1da52abdec3bfcbbb110738a60163734666095fcd8c39bf2  flash_hello.bin
```

`tools/reference-firmware/build.sh` (mode 755):

```bash
#!/usr/bin/env bash
# Builds the reference flash image of the SP1b tests: the stock ESP-IDF 2nd-stage bootloader and the
# hello_world example, merged into a 16 MB image. Everything third-party runs inside Docker; nothing
# built here is committed. The three binaries and the merged image are checked against pinned
# SHA-256 values (docs/research/sp1b-test-oracle.md, section 2): a mismatch stops the script.
#
# Usage: tools/reference-firmware/build.sh [output-dir]      (default: target/reference-firmware)
set -euo pipefail
cd "$(dirname "$0")/../.."
source tools/conformance/versions.env
out="${1:-target/reference-firmware}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
docker build -q -t inkemu/esp-idf --build-arg ESP_IDF_VERSION="$ESP_IDF_VERSION" tools/docker/esp-idf > /dev/null
# No network: the example has no managed component, and the build must not depend on anything that
# is not in the pinned image. The container builds as its own user in its own file system (the
# conditions the pinned hashes were measured in) and hands over the results at the end.
docker run --rm --network none \
  -e IDF_COMPONENT_MANAGER=0 -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
  -v "$PWD/tools/reference-firmware:/ref:ro" -v "$out:/out" \
  inkemu/esp-idf bash /ref/in-container.sh
echo "$out/flash_hello.bin"
```

`tools/reference-firmware/in-container.sh`:

```bash
#!/usr/bin/env bash
# Runs inside the espressif/idf container (see build.sh). /ref is this directory, read-only; /out
# receives the results.
set -euo pipefail
project=/tmp/hello_world
rm -rf "$project"
cp -r "$IDF_PATH/examples/get-started/hello_world" "$project"
cp /ref/sdkconfig.defaults "$project/sdkconfig.defaults"
cd "$project"
if ! idf.py set-target esp32c3 > /tmp/set-target.log 2>&1; then
  tail -n 40 /tmp/set-target.log >&2
  echo "error: idf.py set-target failed; see the log above" >&2
  exit 1
fi
if ! idf.py build > /tmp/build.log 2>&1; then
  tail -n 60 /tmp/build.log >&2
  echo "error: idf.py build failed; see the log above" >&2
  exit 1
fi
cd build
if ! sha256sum -c /ref/SHA256SUMS.inputs; then
  sha256sum bootloader/bootloader.bin partition_table/partition-table.bin hello_world.bin >&2
  echo "error: the build is not the pinned one (hashes above); do not edit SHA256SUMS.inputs: check the image tag, the ESP-IDF version and sdkconfig.defaults" >&2
  exit 1
fi
esptool.py --chip esp32c3 merge_bin --fill-flash-size 16MB -o /out/flash_hello.bin \
  --flash_mode dio --flash_freq 80m --flash_size 16MB \
  0x0 bootloader/bootloader.bin 0x8000 partition_table/partition-table.bin 0x10000 hello_world.bin > /tmp/merge.log 2>&1
riscv32-esp-elf-nm -n bootloader/bootloader.elf > /out/bootloader.nm
riscv32-esp-elf-nm -n hello_world.elf > /out/hello_world.nm
(cd /out && sha256sum -c /ref/SHA256SUMS.output)
chown "$HOST_UID:$HOST_GID" /out/flash_hello.bin /out/bootloader.nm /out/hello_world.nm
```

`tools/reference-firmware/README.md`:

```markdown
# Reference firmware of the SP1b tests

`build.sh [output-dir]` builds, inside the pinned `espressif/idf` Docker image, the stock ESP-IDF
2nd-stage bootloader and the `hello_world` example for the ESP32-C3, checks the three binaries
against `SHA256SUMS.inputs`, merges them into a 16 MB flash image and checks it against
`SHA256SUMS.output`. The ESP-IDF version is `ESP_IDF_VERSION` in `tools/conformance/versions.env`.

Nothing built here is committed and nothing is executed on the host: the image is data for inkemu.
`bootloader.nm` and `hello_world.nm` are symbol tables (text) that turn a program counter printed by
inkemu into a function name: the last symbol whose address is not above the PC.

Run the tests that need the image:

    tools/reference-firmware/build.sh
    cargo run --release -p emu-cli -- rom fetch --all --rom-dir /tmp/inkemu-rom
    INKEMU_ROM_ELF_DIR=/tmp/inkemu-rom INKEMU_REF_FLASH=target/reference-firmware/flash_hello.bin \
      cargo test --release -p soc-esp32c3 --test reference_boot -- --ignored

The build is reproducible (same hashes from two clean builds and from another path): see
`docs/research/sp1b-test-oracle.md`, section 2. If the hashes differ, the image, the ESP-IDF version
or `sdkconfig.defaults` changed: fix that, never the pinned values.
```

- [ ] **Step 2: Build it**

Run: `tools/reference-firmware/build.sh > /tmp/t16build.log 2>&1; echo "exit=$?"`
Expected: `exit=0`; the last line of the log is the path of `flash_hello.bin`; `sha256sum target/reference-firmware/flash_hello.bin` prints `fc05ef5d…c39bf2`.
The pre-flight check of this plan (2026-10-11) ran the two scripts exactly as written, with `--network none` and `IDF_COMPONENT_MANAGER=0`, against the local `inkemu/esp-idf` image: the three inputs and the merged image matched their pins. Should it nevertheless stop on the hashes, run it once without `--network none` and without `IDF_COMPONENT_MANAGER=0` to learn which of the two changes the build, keep the stricter setting that reproduces the pins, and record the finding in the task report. The pinned values do not change.

- [ ] **Step 3: Write the milestone tests** (`crates/soc-esp32c3/tests/reference_boot.rs`)

```rust
//! The milestone of plan SP1b-2 on the reference firmware (ESP-IDF v5.5.2 bootloader + `hello_world`):
//! `INKEMU_ROM_ELF_DIR=<dir> INKEMU_REF_FLASH=<flash_hello.bin> cargo test -p soc-esp32c3 --test reference_boot -- --ignored`
//! Build the image with `tools/reference-firmware/build.sh`.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure

mod common;

use emu_core::Cycles;
use sha2::{Digest, Sha256};
use soc_esp32c3::{ConsoleChannel, Diagnostic, Esp32C3, RomImage, SocConfig, StopReason};

/// SHA-256 of the merged reference image (`docs/research/sp1b-test-oracle.md`, §2).
const REFERENCE_SHA256: &str = "fc05ef5dbe85372e1da52abdec3bfcbbb110738a60163734666095fcd8c39bf2";
/// QEMU reaches the first application line after 1.69 M instructions (BA §1); far more than needed.
const BUDGET: u64 = 400_000_000;
const SLICE: u64 = 1_000_000;

/// The ROM lines of the reference image (FB §G, measured).
const ROM_LINES: &str = "ESP-ROM:esp32c3-api1-20210207\nBuild:Feb  7 2021\nrst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)\nSPIWP:0xee\nmode:DIO, clock div:1\nload:0x3fcd5820,len:0x15a4\nload:0x403cbf10,len:0xc64\nload:0x403ce710,len:0x2fd0\nentry 0x403cbf1a\n";

/// The 2nd-stage bootloader lines of the golden log (TO §2), timestamps masked, with the two
/// documented differences from the QEMU capture:
/// - `efuse block revision: v1.2`, the production chip of decision D9 (QEMU's virtual chip is v1.3);
/// - `SPI Mode : DIO`, because SPI0 stores the mode the ROM programmed (QEMU does not model SPI0 and
///   prints `SLOW READ`; BA §2.1 item 3).
const BOOTLOADER_LINES: [&str; 21] = [
    "I (#) boot: ESP-IDF v5.5.2 2nd stage bootloader",
    "I (#) boot: chip revision: v0.3",
    "I (#) boot: efuse block revision: v1.2",
    "I (#) boot.esp32c3: SPI Speed      : 80MHz",
    "I (#) boot.esp32c3: SPI Mode       : DIO",
    "I (#) boot.esp32c3: SPI Flash Size : 16MB",
    "I (#) boot: Enabling RNG early entropy source...",
    "I (#) boot: Partition Table:",
    "I (#) boot: ## Label            Usage          Type ST Offset   Length",
    "I (#) boot:  0 nvs              WiFi data        01 02 00009000 00006000",
    "I (#) boot:  1 phy_init         RF data          01 01 0000f000 00001000",
    "I (#) boot:  2 factory          factory app      00 00 00010000 00100000",
    "I (#) boot: End of partition table",
    "I (#) esp_image: segment 0: paddr=00010020 vaddr=3c010020 size=0656ch ( 25964) map",
    "I (#) esp_image: segment 1: paddr=00016594 vaddr=3fc8b600 size=011d8h (  4568) load",
    "I (#) esp_image: segment 2: paddr=00017774 vaddr=40380000 size=088a4h ( 34980) load",
    "I (#) esp_image: segment 3: paddr=00020020 vaddr=42000020 size=0e6fch ( 59132) map",
    "I (#) esp_image: segment 4: paddr=0002e724 vaddr=403888a4 size=02c70h ( 11376) load",
    "I (#) esp_image: segment 5: paddr=0003139c vaddr=50000000 size=00020h (    32) load",
    "I (#) boot: Loaded app from partition at offset 0x10000",
    "I (#) boot: Disabling RNG early entropy source...",
];
const FIRST_APPLICATION_LINE: &str = "I (#) cpu_start: Unicore app";
/// `call_start_cpu0` of the application (BA §1).
const APPLICATION_ENTRY: u32 = 0x4038_0330;

fn rom() -> RomImage {
    let dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR to the directory holding the ROM ELF files");
    let path = std::path::Path::new(&dir).join("esp32c3_rev3_rom.elf");
    RomImage::from_elf(&std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap()
}

fn reference_flash() -> Vec<u8> {
    let path = std::env::var("INKEMU_REF_FLASH").expect("set INKEMU_REF_FLASH to flash_hello.bin (tools/reference-firmware/build.sh)");
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// `I (123) tag: text` becomes `I (#) tag: text`: timestamps depend on the timing model.
fn mask_timestamp(line: &str) -> String {
    match (line.strip_prefix("I ("), line.find(") ")) {
        (Some(_), Some(close)) if line[3..close].bytes().all(|b| b.is_ascii_digit()) => format!("I (#{}", &line[close..]),
        _ => line.to_owned(),
    }
}

struct Run {
    console: String,
    diagnostics: Vec<Diagnostic>,
    stop: StopReason,
    trace_hash: Option<u64>,
}

/// Runs the reference image until `done` accepts the console, the machine stops, or the budget ends.
fn run(config: SocConfig, done: impl Fn(&str) -> bool) -> Run {
    let mut machine = Esp32C3::with_flash(rom(), reference_flash(), &config).unwrap();
    let (mut uart0, mut diagnostics) = (Vec::new(), Vec::new());
    let mut stop = StopReason::CycleBudget;
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace('\r', "");
    while stop == StopReason::CycleBudget && machine.cycles().get() < BUDGET && !done(&text(&uart0)) {
        stop = machine.run_until(Cycles(machine.cycles().get() + SLICE));
        uart0.extend(machine.take_console(ConsoleChannel::Uart0));
        let _ = machine.take_console(ConsoleChannel::UsbSerialJtag);
        diagnostics.extend(machine.drain_diagnostics());
    }
    Run { console: text(&uart0), diagnostics, stop, trace_hash: machine.trace_hash() }
}

#[test]
#[ignore = "needs INKEMU_REF_FLASH"]
fn the_image_under_test_is_the_pinned_reference() {
    assert_eq!(common::hex(&Sha256::digest(reference_flash())), REFERENCE_SHA256);
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn rom_bootloader_and_application_print_the_golden_log() {
    let boot = run(SocConfig::default(), |console| console.contains("cpu_start: Unicore app\n"));
    assert!(boot.console.starts_with(ROM_LINES), "ROM part:\n{}", boot.console);
    let after_rom: Vec<String> = boot.console[ROM_LINES.len()..].lines().map(mask_timestamp).collect();
    let mut expected: Vec<&str> = BOOTLOADER_LINES.to_vec();
    expected.push(FIRST_APPLICATION_LINE);
    assert_eq!(
        after_rom.iter().map(String::as_str).take(expected.len()).collect::<Vec<_>>(),
        expected,
        "bootloader and first application line\ndiagnostics: {:#?}",
        boot.diagnostics
    );
    assert!(
        !boot.diagnostics.iter().any(|d| matches!(d, Diagnostic::Exception { .. })),
        "no CPU exception: {:#?}",
        boot.diagnostics
    );
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn strict_run_reaches_the_application_and_ends_on_the_rtc_calibration() {
    let strict = SocConfig { strict_mmio: true, ..SocConfig::default() };
    let boot = run(strict, |_| false);
    // Milestone point 4: nothing unmodelled and no exception from reset to the application entry;
    // then the run ends on a *named* register, the first one plan SP1b-3 has to model (BA §9).
    let StopReason::StrictMmio(violation) = &boot.stop else {
        panic!("expected a strict stop, got {:?}\nconsole:\n{}", boot.stop, boot.console);
    };
    assert!(
        matches!(violation, Diagnostic::UnmodeledMmio { register: Some(("TIMG0", "RTCCALICFG")), .. }),
        "the run must end on TIMG0.RTCCALICFG, it ended on: {violation}\nconsole:\n{}",
        boot.console
    );
    assert_eq!(boot.diagnostics, vec![violation.clone()], "nothing else was reported before it");
    // The first application line is printed by the application's own `system_early_init` (BA §3):
    // seeing it means the bootloader jumped and the application ran from the flash window.
    assert!(boot.console.lines().map(mask_timestamp).any(|line| line == FIRST_APPLICATION_LINE), "{}", boot.console);
    if let Diagnostic::UnmodeledMmio { pc, .. } = violation {
        assert!(
            (0x4038_0000..0x403E_0000).contains(pc) || (0x4200_0000..0x4280_0000).contains(pc) || (0x4000_0000..0x4006_0000).contains(pc),
            "the access comes from application or ROM code: {pc:#010x} (application entry {APPLICATION_ENTRY:#010x})"
        );
    }
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn without_strict_mode_the_wait_on_the_rtc_calibration_is_a_named_warning_not_a_silent_hang() {
    // Far past the first application line: the application then polls the calibration register.
    let mut machine = Esp32C3::with_flash(rom(), reference_flash(), &SocConfig::default()).unwrap();
    let stop = machine.run_until(Cycles(60_000_000));
    let diagnostics = machine.drain_diagnostics();
    assert_eq!(stop, StopReason::CycleBudget);
    assert!(
        diagnostics.iter().any(|d| matches!(d, Diagnostic::UnmodeledMmio { register: Some(("TIMG0", "RTCCALICFG")), .. })),
        "{diagnostics:#?}"
    );
    assert!(
        machine.most_repeated_diagnostics(3).iter().any(|(d, count)| *count > 1
            && matches!(d, Diagnostic::UnmodeledMmio { register: Some(("TIMG0", _)), .. })),
        "the end-of-run summary shows what the firmware keeps polling"
    );
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn two_boots_of_the_reference_image_are_identical_down_to_the_trace_hash() {
    let config = SocConfig { trace_hash: true, ..SocConfig::default() };
    let boot = || run(config, |console| console.contains("cpu_start: Unicore app\n"));
    let (first, second) = (boot(), boot());
    assert!(first.trace_hash.is_some());
    assert_eq!(first.trace_hash, second.trace_hash, "spec §8: same session, same trace hash");
    assert_eq!(first.console, second.console);
}
```

`crates/emu-cli/tests/run_cli.rs`:

```rust
/// The milestone through the real command: ROM, bootloader, first application line, strict stop.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
fn x4_boots_the_reference_firmware_to_its_first_application_line() {
    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let flash = std::env::var("INKEMU_REF_FLASH").expect("set INKEMU_REF_FLASH");
    let output = inkemu()
        .args(["run", "--device", "x4", "--strict-mmio", "--rom-dir", &rom_dir, "--flash", &flash])
        .assert()
        .code(3)
        .get_output()
        .clone();
    let console = String::from_utf8_lossy(&output.stdout).replace('\r', "");
    assert!(console.contains("entry 0x403cbf1a\n"), "{console}");
    assert!(console.contains(") boot: ESP-IDF v5.5.2 2nd stage bootloader\n"), "{console}");
    assert!(console.contains(") boot.esp32c3: SPI Mode       : DIO\n"), "{console}");
    assert!(console.contains(") boot: Disabling RNG early entropy source...\n"), "{console}");
    assert!(console.contains(") cpu_start: Unicore app\n"), "{console}");
    let log = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(log.contains("stopped by --strict-mmio") && log.contains("TIMG0.RTCCALICFG"), "{log}");
    assert!(!log.contains("warning:"), "nothing was reported before the stop: {log}");
}
```

- [ ] **Step 4: Run them, and handle what the hardware-like path reveals**

Run: `INKEMU_ROM_ELF_DIR=/tmp/inkemu-rom INKEMU_REF_FLASH=target/reference-firmware/flash_hello.bin cargo test --release -p soc-esp32c3 --test reference_boot -- --ignored > /tmp/t16.log 2>&1; echo "exit=$?"`
Expected: `exit=0`, five tests pass.

This is the first run of the whole chain, and inkemu takes branches the QEMU traces could not show: the hardware-only PLL bring-up (BA §5 steps 9–10, "read, not run": BA §10 item 2) and the calibration branch of `rtc_init` with real eFuse values (BA §3 notes). Reference point from the pre-flight check of this plan (2026-10-11, Tasks 0 to 15 applied as written on a scratch copy): the five tests pass with no register added; the strict run ends on a **read** of `TIMG0.RTCCALICFG` at pc `0x403829fa`, after about 1.94 M cycles, with the CPU clock at 80 MHz. A stop anywhere else therefore points first at a transcription difference in Tasks 5 to 12; compare with the plan before applying the rules below. If the strict test stops before `TIMG0.RTCCALICFG`, the message names the register and the PC. Find the function: the last symbol of `target/reference-firmware/bootloader.nm` (PC in `0x403C_xxxx`) or `hello_world.nm` (PC in `0x4038_xxxx`/`0x4200_xxxx`) whose address is not above the PC; for a PC in `0x4000_xxxx`–`0x4005_xxxx` it is ROM code (FB and BA name the ROM functions). Then apply exactly one of these rules:

1. **The register is in a table of BA §2, §3 or §5** → a task above missed it: add it to the model that task built (same pattern: an `Accepted` row with its source, or the read value BA §2.2 requires), with a unit test next to that model's tests, regenerate the table of `docs/registers-accepted-without-effect.md` (`INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect`, then check `/usr/bin/git diff -- docs/registers-accepted-without-effect.md`; never by hand), and commit it in this task.
2. **The register is not in BA, the function only read-modify-writes it, and no branch of that function depends on the value read** (check the ESP-IDF source of the function in the Docker image: `docker run --rm inkemu/esp-idf grep -rn "<REGISTER_NAME>" /opt/esp/idf/components/<component>`) → add it as an `Accepted` row whose comment cites the ESP-IDF file and line, with a unit test, and list it in the task report under "registers the hardware-like path added".
3. **Anything else** (the value read is tested, a bit is polled, the register is not in the SVD) → stop. Report the diagnostic line, the function, and the 20 console lines before the stop. Do not model behaviour that no report documents.

If the golden-log test differs on a line other than the two documented ones, do not edit the expected text: report the differing line with the same information.

Run: `INKEMU_ROM_ELF_DIR=/tmp/inkemu-rom INKEMU_REF_FLASH=target/reference-firmware/flash_hello.bin cargo test --release -p emu-cli --test run_cli -- --ignored > /tmp/t16cli.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 5: The CI job** (append to `.github/workflows/ci.yml`)

```yaml
  reference-boot:
    # Boots the reference firmware (stock ESP-IDF bootloader + hello_world) from the real mask ROM to
    # the first line of the application. The firmware is built here, inside the pinned ESP-IDF image,
    # and checked against pinned SHA-256 values; it is never stored in this repository. The cache
    # saves the build and replaces no check: the image is re-hashed on every run.
    runs-on: ubuntu-24.04
    timeout-minutes: 45
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@master
        with: { toolchain: "1.99.0" }
      - uses: Swatinem/rust-cache@v2
      - uses: actions/cache@v6
        with:
          path: ${{ runner.temp }}/rom
          key: esp-rom-elfs-20260528
      - id: firmware
        uses: actions/cache@v6
        with:
          path: ${{ runner.temp }}/reference-firmware
          key: reference-firmware-${{ hashFiles('tools/reference-firmware/**', 'tools/docker/esp-idf/Dockerfile', 'tools/conformance/versions.env') }}
      - name: Build the reference firmware
        if: steps.firmware.outputs.cache-hit != 'true'
        run: tools/reference-firmware/build.sh "$RUNNER_TEMP/reference-firmware"
      - name: Verify the reference image
        working-directory: ${{ runner.temp }}/reference-firmware
        run: sha256sum -c "$GITHUB_WORKSPACE/tools/reference-firmware/SHA256SUMS.output"
      - name: Fetch and verify the mask ROMs
        run: cargo run --release -p emu-cli -- rom fetch --all --rom-dir "$RUNNER_TEMP/rom"
      - name: Boot the reference firmware
        env:
          INKEMU_ROM_ELF_DIR: ${{ runner.temp }}/rom
          INKEMU_REF_FLASH: ${{ runner.temp }}/reference-firmware/flash_hello.bin
        run: |
          cargo test --release -p soc-esp32c3 --test reference_boot -- --ignored
          cargo test --release -p emu-cli --test run_cli x4_boots_the_reference_firmware -- --ignored
```

The existing `rom-boot` job is unchanged: it already runs every ignored test of `real_rom` and `run_cli`; add `--skip x4_boots_the_reference_firmware` to its `run_cli` line (`cargo test --release -p emu-cli --test run_cli -- --ignored --skip x4_boots_the_reference_firmware`), because that job has no reference image.

- [ ] **Step 6: Run the whole workspace**

Run: `cargo test --workspace > /tmp/t16w.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.
Run: `cargo llvm-cov --workspace --fail-under-lines 80 > /tmp/t16cov.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 7: Commit**

Before committing (Global Constraints): `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`: `exit=0` three times. The code blocks of this plan are not wrapped as `rustfmt` wraps them.

```bash
/usr/bin/git add tools/reference-firmware crates/soc-esp32c3/tests/reference_boot.rs crates/emu-cli/tests/run_cli.rs .github/workflows/ci.yml
/usr/bin/git commit -m "test: build the reference firmware in Docker and boot it to its first application line

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- tools/reference-firmware crates/soc-esp32c3/tests/reference_boot.rs crates/emu-cli/tests/run_cli.rs .github/workflows/ci.yml
```

(If Step 4 added registers, the commit also carries `crates/soc-esp32c3` and `docs/registers-accepted-without-effect.md`.)

---

### Task 17: Documentation — README, ADR, corrected research reports, carry-over note

**Files:**
- Modify: `README.md`, `docs/research/esp32c3-boot-chain.md`, `docs/research/sp1b-test-oracle.md`, `docs/superpowers/notes/2026-10-11-sp1b-1-carryover.md`, `docs/registers-accepted-without-effect.md` (final check only)
- Create: `docs/adr/0002-flash-model-and-cache-windows.md`, `docs/superpowers/notes/2026-10-11-sp1b-2-carryover.md`

**Interfaces:**
- Consumes: the behaviour delivered by Tasks 0–16 and the task reports (deviations, registers added in Task 16 Step 4).
- Produces: documents only. The test `crates/soc-esp32c3/tests/accepted_without_effect.rs` keeps pinning the table. The ADR and the carry-over note are in French (like `docs/adr/0001-*.md` and the SP1b-1 note); the README, the research reports and the registers table stay in English.

- [ ] **Step 1: README**

Replace the block of `## Build & try` from the first code fence to the end of the paragraph that starts with "The device console goes to standard output" by:

````markdown
```bash
cargo build --release

# Boot a virtual Xteink X4 from a full flash image: mask ROM, 2nd-stage bootloader, application.
target/release/inkemu run --device x4 --flash flash.bin --max-cycles 50000000
```

```text
ESP-ROM:esp32c3-api1-20210207
Build:Feb  7 2021
rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)
SPIWP:0xee
mode:DIO, clock div:1
load:0x3fcd5820,len:0x15a4
load:0x403cbf10,len:0xc64
load:0x403ce710,len:0x2fd0
entry 0x403cbf1a
I (…) boot: ESP-IDF v5.5.2 2nd stage bootloader
…
I (…) boot: Loaded app from partition at offset 0x10000
I (…) boot: Disabling RNG early entropy source...
I (…) cpu_start: Unicore app
```

`--flash` takes a **full flash image**: a flash dump, or the output of `esptool.py merge_bin` (bootloader at offset 0, partition table at `0x8000`, application). The file may be shorter than the flash of the device; the rest reads as erased. inkemu refuses, before anything runs, a file that cannot be the flash of the device: an encrypted `.xota`, an application image alone, an image for another chip, an image that declares more flash than the device has, a file longer than the flash. Without `--flash` the flash is erased and the ROM prints `invalid header: 0xffffffff` in a loop, as a blank chip does. Booting an application `.bin` on its own (`--firmware`) is not available yet.

Today the run goes as far as the first log line of the application: the mask ROM validates and loads the bootloader through the emulated flash controller, cache and SHA accelerator, the unmodified ESP-IDF bootloader runs and jumps into the application, which then waits for a timer inkemu does not emulate yet (`TIMG0.RTCCALICFG`); that wait shows as a named warning.

The device console goes to standard output. Everything inkemu has to say goes to standard error: the device, the timing model and the flash in use, then one `warning:` line for each thing it cannot do faithfully — an unmodelled register (with the peripheral, the register, its address and the program counter), an 8- or 16-bit write to a peripheral register, a flash command the read-only flash does not implement, an access to flash that is not mapped, a CPU exception — and, at the end, the most repeated ones, the device time elapsed and the CPU clock. `--strict-mmio` turns the first of these (CPU exceptions excepted) into a stop with exit code 3, for CI. Registers that inkemu stores without modelling their effect are listed in [docs/registers-accepted-without-effect.md](docs/registers-accepted-without-effect.md).
````

In `## Documentation`, add: `- [Registers accepted without effect](docs/registers-accepted-without-effect.md): what is stored but not modelled, and the plan that makes each real` and `- [ADR 0002](docs/adr/0002-flash-model-and-cache-windows.md): the flash model and the cache windows`. In `## Third-party material`, add: `- **Reference firmware.** The tests boot the stock ESP-IDF bootloader and `hello_world` example (Apache-2.0). They are built in Docker by `tools/reference-firmware/build.sh` and checked against pinned hashes; no firmware binary is stored in this repository.` In `## Français`, replace the sentence that begins "La flash, le bootloader de second niveau et l'application sont la prochaine étape" by "À partir d'une image flash complète (`--flash`), la ROM charge le bootloader de second niveau, qui s'exécute et lance l'application jusqu'à sa première ligne de journal ; les interruptions, les timers et les watchdogs sont la prochaine étape (sous-projets 1b-3 et 1b-4)." In `## Roadmap`, replace the italic note of line 1 by: "*1a done; 1b-1, the SoC skeleton and the ROM boot, done; 1b-2, flash and the bootloader, done; 1b-3, interrupts, timers, watchdogs and reset, is next.*"

- [ ] **Step 2: ADR** (`docs/adr/0002-flash-model-and-cache-windows.md`)

The ADR is written in French, like `docs/adr/0001-espressif-rom-and-svd-licensing.md` (same header lines, same section names). Write the file below as it is.

```markdown
# ADR 0002 — Modèle de la flash et fenêtres de cache

- Date : 2026-10-11
- Statut : accepté (plan SP1b-2)

## Contexte

L'ESP32-C3 exécute et lit sa flash à travers deux fenêtres d'adresses de 8 Mio (`0x3C000000` pour les données, `0x42000000` pour les instructions), servies par un cache de 16 Kio qu'une table MMU de 128 entrées fait correspondre à des pages de flash de 64 Kio. La puce de flash elle-même est pilotée par deux contrôleurs : SPI0 remplit le cache, SPI1 exécute les commandes que le logiciel demande.
Espressif ne documente ni le contrôleur de cache (`EXTMEM`) ni la table MMU dans le manuel de référence technique. Ce que le silicium renvoie pour un accès à travers une entrée invalide, ou quand le cache est désactivé, n'est pas documenté (`docs/research/esp32c3-cache-mmu.md`, sections 6 et 9).
Il faut décider ce que l'émulateur modélise de cet ensemble pour que la ROM et le bootloader s'exécutent sans modification, et ce qu'il fait des cas que rien ne documente.

## Décision

1. **Les fenêtres lisent la flash en direct à travers la table MMU.** Aucune ligne de cache, aucune étiquette, aucun défaut de cache n'est modélisé. Un accès traduit son adresse avec l'entrée de sa page de 64 Kio et lit le contenu de la flash à cet instant.
2. **Le contrôleur de cache se réduit à une logique d'achèvement et à du stockage.** Les opérations que le logiciel attend (invalidation, préchargement, chargement automatique, gel) se terminent aussitôt ; les bits d'activation et de fermeture des bus sont mémorisés et ne sont pas appliqués.
3. **Ce qui ne peut pas être juste en silence est signalé.** Un accès à travers une entrée invalide, ou vers une page située au-delà de la fin de la flash, renvoie 0, verrouille `CACHE_ILG_INT_ST` et `CACHE_MMU_FAULT_*`, et produit un diagnostic nommé ; `--strict-mmio` s'y arrête. Une écriture dans une fenêtre lève une faute d'accès en écriture.
4. **La flash est pour l'instant en lecture seule et appartient au modèle de SPI1**, derrière le trait `FlashDevice`. SPI1 implémente ses commandes câblées et le transfert utilisateur générique ; le modèle de la puce répond aux commandes que la chaîne de démarrage envoie, d'après les mesures, et refuse tout autre opcode en le nommant.
5. **Le code décodé suit la table.** `Bus::page_version` des deux fenêtres change à chaque écriture dans la table MMU ou dans les bits d'activation du cache : le cache de décodage du CPU abandonne ainsi le code de flash qui a pu être déplacé.

## Conséquences

- **ROM et bootloader non modifiés** : la ROM masquée et le bootloader ESP-IDF s'exécutent tels quels. Chacune de leurs attentes se termine, et les quelque 500 changements de table que fait le bootloader à chaque démarrage ne demandent aucun suivi d'invalidation.
- **Temps** : un défaut du cache de flash ne coûte aucun temps. Le modèle de temps reste « non calibré » ; le coût d'un défaut (spec, section 4.2) revient au plan SP1b-4, par `Bus::fetch_stall`, et sa calibration au sous-projet 8.
- **Écart connu avec le silicium** : un firmware qui réécrit la flash puis relit une zone projetée **sans** invalider le cache verra ici les nouvelles données, et les anciennes sur le silicium. Cela ne compte qu'une fois les écritures en flash disponibles (SP1b-3) ; ce plan devra choisir entre cette vue en direct et des lignes qui restent périmées jusqu'à leur invalidation.
- **Hypothèse** : le comportement d'une entrée invalide est une hypothèse (`ASSUMPTION:` dans `bus.rs`). Le lockstep de SP1b-4, puis un appareil réel, la trancheront. Un firmware sain ne l'atteint jamais : le test du jalon tourne en mode strict.
- **Révision de l'ADR** : à rouvrir quand les écritures en flash arriveront, ou si une mesure sur le matériel contredit le point 3.
```

- [ ] **Step 3: Correct `docs/research/esp32c3-boot-chain.md`** (decision D14)

Apply each correction below in place, and append the line `(corrected 2026-10-11, plan SP1b-2: see <report> §<section>)` to the corrected cell or sentence. Then add, under the title, the paragraph: "Corrected on 2026-10-11 from `esp32c3-rom-flash-boot.md` §K, `esp32c3-cache-mmu.md` §0.1 and `esp32c3-bootloader-accesses.md` §11. Where this report and those three disagree, they prevail."

| # | Place in `esp32c3-boot-chain.md` | Replace | By | Source |
|---|---|---|---|---|
| 1 | §C.1, row `0x040` `ICACHE_AUTOLOAD_CTRL` | "`ROM_Boot_Cache_Init` sets b0, b1 then clears b1; reset value DONE=1 (QEMU)" | "Only read on the boot path: `Cache_Suspend_ICache_Autoload` polls b3 (`DONE`), which must read 1; b2 (`ENA`) is a stored configuration bit. `ROM_Boot_Cache_Init` does **not** write this register (it writes `SYSTEM.CACHE_CONTROL`)" | CM §0.1, FB §K |
| 2 | §C.1, row `0x0CC` `ICACHE_FREEZE` | (keep the text) | append "This is required: `Cache_Suspend_ICache` spins on it" | CM §0.1 |
| 3 | §D.2, item 1 | "…`esp_rom_spiflash_config_readmode`, `SPI_WakeUp@0x4004d1c8` (write `CMD`, poll)." | "…`esp_rom_spiflash_config_readmode`. With `a2 == 0` the attach does **not** wake the flash up: `SPI_WakeUp@0x4004d1c8` is called later, from `ets_run_flash_bootloader`." | FB §K |
| 4 | §D.2, item 2 | "then reads status (RDSR via `esp_rom_spiflash_read_user_cmd@0x4004d0cc`: …, result in `W0 & 0xFF`)" | "then reads the status with the hard-wired command: `CMD = 0x08000000` (`FLASH_RDSR`), result in `RD_STATUS`. This happens twice, inside `ets_config_flash_by_image_hdr`, not in the attach. The `USR` variant `esp_rom_spiflash_read_user_cmd` is not reached on this path." | FB §K, §D.1 |
| 5 | §D.2, item 4 | "`ROM_Boot_Cache_Init@0x40050ee6` (EXTMEM `AUTOLOAD_CTRL`, `Cache_MMU_Init`, …)" | "`ROM_Boot_Cache_Init@0x40050ee6` (`SYSTEM.CACHE_CONTROL` ×3, `CACHE_MMU_OWNER |= 0xF`, `Cache_MMU_Init`, `CACHE_ILG_INT_CLR`, `CACHE_WRAP_AROUND_CTRL`, `SENSITIVE.INTERNAL_SRAM_USAGE_1 &= ~1`, invalidate-all, `Cache_Enable_ICache`, `Cache_UnMask_Dram0`)" | CM §0.1, §5.1 |
| 6 | §E.3, row MWDT0/1 | "optionally starts RWDT (`CONFIG_BOOTLOADER_WDT_ENABLE`)" | "starts RWDT in the default configuration: one stage, whole-chip reset after 9 s, left running into the application" | BA §11, §6 |
| 7 | §H, row `0x34` `RD_REPEAT_DATA1` | "b1:0 WDT_DELAY_SEL, **b4:2 SPI_BOOT_CRYPT_CNT**" | "b17:16 WDT_DELAY_SEL, **b20:18 SPI_BOOT_CRYPT_CNT** (b15:0 reserved)" | BA §11 |
| 8 | §H, row `0x38` `RD_REPEAT_DATA2` | "**b16 SECURE_BOOT_EN**" and "**b21:18 FLASH_TPUW**" | "**b20 SECURE_BOOT_EN**" and "**b31:28 FLASH_TPUW**" | FB §K |
| 9 | §H, row `0x3C` `RD_REPEAT_DATA3` | "b30 ERR_RST_ENABLE" | "b31 ERR_RST_ENABLE" | BA §11 |
| 10 | §I.1, header validation | "segment count ≤ 16" and "optional appended SHA-256" | "segment count ≤ 32 in the header gate, then ≤ 16" and "the SHA-256 that follows the checksum is always computed and compared; a mismatch is not fatal without secure boot, and `hash_appended` is never consulted" | FB §K, §F |
| 11 | §J, row 14 | "`SelectSpiFunction`, `SPI_init`, `esp_rom_spiflash_config_readmode`, `SPI_WakeUp`" | "`SelectSpiFunction`, `SPI_init`, `esp_rom_spiflash_config_readmode` (no `SPI_WakeUp` and no status read here)" | FB §K |
| 12 | §J, row 15 | "**RMW `EXTMEM.AUTOLOAD_CTRL 0x600C4040`** (b0, b1, clear b1)" and the order of the following operations | "**RMW `SYSTEM.CACHE_CONTROL 0x600C0040`** (set b0, set b1, clear b1)", then the order of correction 5 | CM §0.1 |
| 13 | §J, row 16 | "(b21:18)" after `RD_REPEAT_DATA2` | "(b31:28)" | FB §K |
| 14 | §J, rows 17 and 23 | "`Cache_Suspend_ICache` (RMW `EXTMEM.CTRL1 |=3`, `CTRL &=~1`, poll `EXTMEM+0x34 & 2`)" | add: "then `Cache_Suspend_ICache_Autoload` (poll `AUTOLOAD_CTRL` b3) before the preload poll, and after it a freeze enable + freeze disable pair on `ICACHE_FREEZE`, each with its `DONE` poll" | CM §0.1 |
| 15 | §J, rows 17 and 22 | "MMU entries rewritten, cache suspend/invalidate/resume each time" | "the mapping changes only when the wanted range leaves the current window: once in total for an image below 64 KiB" | FB §K |
| 16 | §K, item 7 | "the ROM busy-wait condition was inferred from QEMU's `0xffffff` stub, not disassembled" | "settled: the ROM spins while bit 25 of the command register `0x6000E000` is set and reads the data byte in bits [23:16]" | BA §11, §2.2 |

In `docs/research/sp1b-test-oracle.md` §1c, add under the options table: "Addendum 2026-10-11 (from `esp32c3-bootloader-accesses.md` §11): MMIO accesses can be traced without plugins with `-trace 'memory_region_ops_*'`, and QEMU's random number generator makes a register-level lockstep diverge inside the bootloader (first read at instruction ≈ 635,927): mask the affected registers and memory, or inject the values."

Check: `grep -c "corrected 2026-10-11" docs/research/esp32c3-boot-chain.md` prints at least 16 (one per corrected line).

- [ ] **Step 4: Carry-over note** (`docs/superpowers/notes/2026-10-11-sp1b-2-carryover.md`)

The note is written in French, with the tone and the sections of `docs/superpowers/notes/2026-10-11-sp1b-1-carryover.md`. Write the file below. Its section "Décisions prises pendant l'exécution" is filled from the task reports: one table row per deviation from this plan (columns `#`, `Décision`, `Raison`, `Coût si elle est fausse`, rows numbered R1, R2, …), including every register Task 16 Step 4 added; write "Aucune." in place of the table if there was none.

```markdown
# SP1b-2 — décisions prises et points reportés

Bilan de l'exécution du plan `docs/superpowers/plans/2026-10-11-sp1b-2-flash-bootloader.md`. Ce document garde deux choses que l'historique git ne dit pas : les **décisions** prises en cours de route, et ce qui est **volontairement reporté**, avec le plan qui en hérite.

## Résultat

Avec l'image flash de référence, `inkemu run --device x4 --flash <image>` affiche les lignes de la ROM sur son chemin de démarrage depuis la flash, toutes les lignes du bootloader de second niveau, puis la première ligne de l'application. En mode strict, aucun registre non modélisé n'est touché et aucune exception CPU n'est levée, du reset jusqu'à l'entrée de l'application ; l'exécution s'arrête sur `TIMG0.RTCCALICFG`, la calibration de l'horloge lente du RTC. C'est le point de départ du plan SP1b-3.

## Décisions prises pendant l'exécution

(à remplir à partir des comptes rendus de tâche)

Les décisions de conception D1 à D29 du plan restent valables. Celles que les plans suivants doivent connaître : la flash est en lecture seule (D3) ; les fenêtres de cache sont une vue en direct de la flash (D4, ADR 0002) ; les watchdogs n'expirent jamais (D11) ; les registres PMP sont mémorisés mais pas appliqués (D18) ; le CPU tourne à 20 MHz au reset (D21, une hypothèse).

## Reporté au plan SP1b-3 (interruptions, timers, watchdogs, reset)

- **Premiers registres à modéliser** : `TIMG0.RTCCALICFG`, `RTCCALICFG1`, `RTCCALICFG2` (la calibration doit se dire prête, avec un résultat plausible : `esp32c3-bootloader-accesses.md`, §9) ; puis `SYSTIMER.UNIT0_OP` (bit de valeur valide), le timer RTC (`TIME_UPDATE`, `TIME_LOW0/HIGH0`), puis l'interruption logicielle `SYSTEM.CPU_INTR_FROM_CPU_0` qui lance l'ordonnanceur (même rapport, §9).
- **Écritures en flash** : programmation de page, effacement de secteur et de bloc, écriture du registre d'état par SPI1 (`FLASH_PP`, `FLASH_SE`, le chemin `USR`), lectures de données par SPI1 (`0x03`, `0x0B`, `0xBB` : `esp_flash` lit ainsi), fichier de flash par appareil qui survit aux redémarrages (spec §5.1). Décider alors si les fenêtres de cache restent une vue en direct ou gardent des lignes périmées jusqu'à leur invalidation (ADR 0002).
- **`--firmware`** : composer une flash à partir d'un `.bin` d'application, d'un bootloader et d'une table de partitions du profil (D1) ; tables de partitions dans les profils.
- **Reset logiciel** : les bits 31 et 5 de `RTC_CNTL.OPTIONS0` sont refusés aujourd'hui (D27) ; causes de reset.
- **Watchdogs** : RWDT, MWDT0/1 et super watchdog sont du stockage (D11). Leur expiration demande d'abord le tick du SYSTIMER. Le critère 4 de la spec (un firmware bloqué est réinitialisé par son watchdog) et la boucle de reset d'une flash vierge ou corrompue (`rst:0x7`, `esp32c3-rom-flash-boot.md`, §H) en dépendent.
- **Application de la PMP** (D18) et déclencheurs de débogage ; le test de la CLI qui compte sur l'absence de `pmpaddr` dans le cœur générique tient toujours.
- **Interruptions d'erreur du cache** : `CACHE_ILG_INT_ST` et `CORE0_ACS_CACHE_INT_ST` sont verrouillés, mais aucune interruption n'est levée (sources 36 et 61).
- **Registres de programmation des eFuse** (`EFUSE.CMD`, `PGM_*`) : non modélisés ; aucun chemin mesuré n'y touche.
- Tout ce que SP1b-1 avait déjà attribué à SP1b-3 (temps des périphériques, temps d'émission et réception de l'UART, livraison des interruptions, journal d'exception avec PC et pile) est inchangé.
- Chaque ligne de `docs/registers-accepted-without-effect.md` dont la dernière colonne est SP1b-3.

## Reporté au plan SP1b-4 (lockstep QEMU, CoreMark, rejeu, hygiène CI)

Hypothèses de ce plan (les lignes 1 à 11 sont marquées `ASSUMPTION:` dans le code) et écarts connus avec l'oracle QEMU (lignes 12 à 14), à trancher ou à masquer dans le lockstep :

| # | Hypothèse | Où | Écart avec l'oracle QEMU |
|---|---|---|---|
| 1 | Un accès à travers une entrée MMU invalide, ou vers une page au-delà de la flash, renvoie 0 | `bus.rs`, `flash_read` | QEMU renvoie 0 pour une entrée jamais projetée et `0xDEADBEEF` après un passage de valide à invalide |
| 2 | Une écriture dans une fenêtre de flash est une faute d'accès en écriture | `bus.rs` | identique à QEMU ; silicium inconnu |
| 3 | `EXTMEM.ICACHE_SYNC_CTRL` se lit « terminé » (`0x2`) dès le reset | `periph/extmem.rs` | QEMU lit 0 au reset |
| 4 | Une entrée MMU fait 9 bits ; toutes les entrées sont invalides à la mise sous tension | `periph/mmu.rs` | identique à QEMU |
| 5 | Le CPU tourne à 20 MHz au reset (quartz ÷ 2) | `clock.rs` | QEMU initialise `SYSCLK_CONF` sur la PLL : le bootloader y saute la mise en route de la PLL |
| 6 | Les registres regi2c se lisent 0 avant d'être écrits ; `ANA_CONFIG*` valent 0 au reset | `periph/ana_i2c.rs` | QEMU répond `0xFFFFFF` à toute lecture de la fenêtre |
| 7 | Une lecture SFDP répond `0xFF` ; l'adresse `USR` n'est pas décalée ; tout transfert `USR` a une commande de 8 bits | `flash.rs`, `periph/spi_mem.rs` | la flash de QEMU rejette `0x5A` |
| 8 | `W0[31:24]` et les octets non lus du dernier mot de données se lisent 0 ; RDSR ne remplace que `RD_STATUS[7:0]` | `periph/spi_mem.rs` | non comparé |
| 9 | `SHA.MODE` 0 est SHA-1 et 1 est SHA-224 | `periph/sha.rs` | non exercé par le démarrage |
| 10 | Le contenu des eFuse est celui d'une puce v0.3 de production (bloc v1.2), pas celui du X4 | `periph/efuse.rs` | QEMU : bloc v1.3, ni MAC ni calibration ; les lectures d'eFuse et la branche de calibration de `rtc_init` diffèrent |
| 11 | Identifiant JEDEC de la flash `0x9D6018` | profils | identique à QEMU, par choix (D2) |
| 12 | Le générateur aléatoire est une suite à graine fixe | `periph/apb_ctrl.rs` | QEMU utilise l'aléa de l'hôte : injecter les valeurs ou masquer (D8) |
| 13 | `SPI0.CTRL` est mémorisé, donc le bootloader affiche `SPI Mode : DIO` | `periph/spi_mem.rs` | QEMU affiche `SLOW READ` |
| 14 | La console de la ROM est dupliquée sur USB-Serial-JTAG | (SP1b-1) | QEMU ne l'émet pas |

Autres points pour SP1b-4 :

- **Temps du cache de flash** : `Bus::fetch_stall` renvoie 0 et un défaut de cache ne coûte rien (D6, ADR 0002) ; la simulation du cache (spec §4.2) et la tolérance CoreMark se décident ici, la calibration au sous-projet 8.
- **La mise en route de la PLL, propre au matériel** (`esp32c3-bootloader-accesses.md`, §5, étapes 9 et 10) et la branche de calibration de `rtc_init` s'exécutent dans inkemu et ne peuvent pas être comparées à QEMU.
- **Le chemin de démarrage depuis la flash des ROM rev0 et rev101** n'a pas été exécuté (`esp32c3-rom-flash-boot.md`, §L, point 9) : seule rev3 est testée depuis la flash.
- **Fermeture des bus** des fenêtres de flash : mémorisée, pas appliquée (D5).
- **CI** : le job `reference-boot` récupère `espressif/idf` par étiquette, pas par empreinte ; les empreintes épinglées des fichiers produits le font échouer fermé. Épingler l'empreinte de l'image avec les autres épinglages.
- Tout ce que SP1b-1 avait déjà attribué à SP1b-4 est inchangé.

## À prévoir plus tard

- **SP2** : `--device <fichier.toml>` et le nettoyage du message d'erreur de `toml` ; broches GPIO, matrice GPIO et `IO_MUX` ; le SAR ADC ; chaque ligne de la table des registres acceptés dont la dernière colonne est SP2.
- **SP5a** : registres d'alimentation des mémoires de la radio.
- **SP8** : durées des transferts SPI et du SHA (`esp32c3-rom-flash-boot.md`, §L, point 1) ; fréquence du CPU au reset (hypothèse 5).
- La puce de flash, le contenu des eFuse et la révision de silicium d'un Xteink X4 réel restent inconnus : `esptool.py flash_id`, `espefuse.py summary` et le journal de démarrage d'un appareil trancheraient les trois.
```

Then, in `docs/superpowers/notes/2026-10-11-sp1b-1-carryover.md` (a French document: keep its language), section "Reporté au plan SP1b-2", close each of the eight bullets the way the SP1b-1 plan closed the SP1a note. Strike the handled text (`~~…~~`) and append the outcome:

| Bullet | Append |
|---|---|
| `EXTMEM.ICACHE_SYNC_CTRL`, GPIO, `IO_MUX`, SPI0/SPI1 | `**Levé par SP1b-2**` |
| MMU table | `**Levé par SP1b-2**` |
| eFuse | strike "adresse MAC et version de bloc (le bootloader les lit)" → `**Levé par SP1b-2**`; leave "registres de programmation" and append `**Reporté à SP1b-3**` |
| `SYSTEM.CPU_PER_CONF` and the clock tree | `**Levé par SP1b-2**` |
| Device profiles | strike "taille de flash" and "validation … (identifiant vide, cohérence de l'identifiant)" → `**Levé par SP1b-2**`; "tables de partitions" → `**Reporté à SP1b-3** (avec --firmware)`; the `--device <fichier.toml>` sentence → `**Reporté à SP2**` |
| SP1a points | strike "l'heuristique de dump flash, le message de `BadFlashSize`, le `Display` de `SpiMode`" → `**Levé par SP1b-2**`; "`fetch_stall` et le cache flash" → `**Reporté à SP1b-4**` |
| `rom_store` | `**Levé par SP1b-2**` |
| literal base addresses in three test modules | `**Levé par SP1b-2**` |

- [ ] **Step 5: Final checks**

Run: `cargo test -p soc-esp32c3 --test accepted_without_effect > /tmp/t17doc.log 2>&1; echo "exit=$?"`
Expected: `exit=0` (the documented table is the one the code builds).
Run: `cargo fmt --all --check > /tmp/t17fmt.log 2>&1; echo "exit=$?"`, `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/t17c.log 2>&1; echo "exit=$?"`, `cargo test --workspace > /tmp/t17.log 2>&1; echo "exit=$?"`, `cargo deny check > /tmp/t17deny.log 2>&1; echo "exit=$?"`, `cargo run -p svd-gen -- --check > /tmp/t17svd.log 2>&1; echo "exit=$?"`
Expected: `exit=0` five times.
Run: `grep -rn "ASSUMPTION:" crates/soc-esp32c3/src crates/cpu-rv32/src crates/boards/profiles | wc -l` and check that every assumption this plan introduced appears in the table of the carry-over note (add the missing rows).
Run: `/usr/bin/git ls-files > /tmp/t17files.log; /usr/bin/git status --porcelain >> /tmp/t17files.log; grep -E '\.(bin|elf|img)$' /tmp/t17files.log`
Expected: the only line is `tests/fixtures/partitions/x4-stock.bin`, the partition-table fixture tracked since SP1a (no firmware, ROM or flash image is tracked or pending).

- [ ] **Step 6: Commit**

Before committing (Global Constraints): `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`: `exit=0` three times. The code blocks of this plan are not wrapped as `rustfmt` wraps them.

```bash
/usr/bin/git add README.md docs/adr/0002-flash-model-and-cache-windows.md docs/research/esp32c3-boot-chain.md docs/research/sp1b-test-oracle.md docs/superpowers/notes/2026-10-11-sp1b-1-carryover.md docs/superpowers/notes/2026-10-11-sp1b-2-carryover.md docs/registers-accepted-without-effect.md
/usr/bin/git commit -m "docs: flash boot in the README, ADR 0002, corrected boot-chain report, SP1b-2 carry-over

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- README.md docs/adr/0002-flash-model-and-cache-windows.md docs/research/esp32c3-boot-chain.md docs/research/sp1b-test-oracle.md docs/superpowers/notes/2026-10-11-sp1b-1-carryover.md docs/superpowers/notes/2026-10-11-sp1b-2-carryover.md docs/registers-accepted-without-effect.md
```

---

## Build & try

```bash
cargo build --release

# The ROM alone, on an erased flash: banner, then `invalid header: 0xffffffff` in a loop.
target/release/inkemu run --device x4 --max-cycles 3000000

# The reference firmware (needs Docker): ROM, bootloader, first application line.
tools/reference-firmware/build.sh
target/release/inkemu run --device x4 --flash target/reference-firmware/flash_hello.bin --max-cycles 50000000

# The same in strict mode: exit code 3 on TIMG0.RTCCALICFG, nothing reported before it.
target/release/inkemu run --device x4 --flash target/reference-firmware/flash_hello.bin --strict-mmio; echo "exit=$?"

# Every test, including those on the real ROM and on the reference firmware.
target/release/inkemu rom fetch --all --rom-dir /tmp/inkemu-rom
cargo test --workspace
INKEMU_ROM_ELF_DIR=/tmp/inkemu-rom cargo test --release -p soc-esp32c3 --test real_rom -- --ignored
INKEMU_ROM_ELF_DIR=/tmp/inkemu-rom INKEMU_REF_FLASH=target/reference-firmware/flash_hello.bin \
  cargo test --release -p soc-esp32c3 --test reference_boot -- --ignored
INKEMU_ROM_ELF_DIR=/tmp/inkemu-rom INKEMU_REF_FLASH=target/reference-firmware/flash_hello.bin \
  cargo test --release -p emu-cli --test run_cli -- --ignored
```

## Third-party material

| Material | Origin and licence | Where it lives |
|---|---|---|
| ESP32-C3 mask ROM | Espressif, `esp-rom-elfs` release `20260528`, used under Apache-2.0 (ADR 0001) | downloaded and verified by `inkemu rom fetch`; never in the repository |
| ESP-IDF v5.5.2 bootloader and `hello_world` | Espressif, Apache-2.0 | built by `tools/reference-firmware/build.sh` inside `espressif/idf:v5.5.2`, verified against pinned SHA-256, kept under `target/` or the CI runner's temporary directory; never in the repository, never executed on the host |
| eFuse content of a production chip | the `espefuse` dump in ESP-IDF's documentation (Apache-2.0), quoted in `esp32c3-bootloader-accesses.md` §8 | 14 words in `crates/soc-esp32c3/src/periph/efuse.rs` |
| Golden boot log lines | output of the Apache-2.0 bootloader and example | expected strings in `crates/soc-esp32c3/tests/reference_boot.rs` |
| `sha1`, `sha2` (RustCrypto) | MIT OR Apache-2.0 | Cargo dependencies |
| QEMU (Espressif fork) | GPL | not used by this plan; facts taken from the research reports only |

---

## Acceptance criteria of this plan

1. `inkemu run --device x4 --flash flash_hello.bin` prints the ROM lines of the flash path, the 21 bootloader lines through `boot: Disabling RNG early entropy source...` (timestamps masked; `efuse block revision: v1.2` and `SPI Mode : DIO` being the two documented differences from the QEMU capture), and `cpu_start: Unicore app` → `rom_bootloader_and_application_print_the_golden_log`, `x4_boots_the_reference_firmware_to_its_first_application_line` (Task 16).
2. In strict mode, the same run touches nothing unmodelled and takes no CPU exception from reset to the application entry, and ends on `TIMG0.RTCCALICFG` by name → `strict_run_reaches_the_application_and_ends_on_the_rtc_calibration` (Task 16).
3. Without `--flash`, the ROM prints `invalid header: 0xffffffff` in a loop → `blank_flash_prints_invalid_header_forever`, `x4_without_a_flash_image_loops_on_the_invalid_header_of_an_erased_chip` (Tasks 9, 15).
4. The real ROM validates, loads and enters a synthetic bootloader, and its failure paths print what FB §H measured → the seven tests of Task 9.
5. Every refusal of spec §5.1 that applies to a full flash image has a test of its message (spec criterion 7) → Tasks 14 and 15.
6. Two runs of the same image give the same trace hash (spec criterion 6) → `two_flash_boots_are_identical_down_to_the_trace_hash`, `two_boots_of_the_reference_image_are_identical_down_to_the_trace_hash`.
7. `docs/registers-accepted-without-effect.md` is the table the code builds → `the_documented_table_is_the_one_the_code_builds`.
8. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace`, `cargo deny check`, `cargo run -p svd-gen -- --check` and `cargo llvm-cov --workspace --fail-under-lines 80` pass; the CI jobs `check`, `coverage`, `arch-test`, `rom-boot` and `reference-boot` are green.
9. No ROM, firmware or flash image is in the repository.

Out of this plan, by decision: `--firmware` and flash writes (SP1b-3), interrupt delivery, timers and watchdog expiry (SP1b-3), the QEMU lockstep and CoreMark (SP1b-4). Spec criteria 2 to 5 of §10.3 are therefore not met yet; criterion 2 is met up to the first application line of the reference firmware.
