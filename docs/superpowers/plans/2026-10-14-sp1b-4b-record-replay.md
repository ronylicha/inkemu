# SP1b-4b — Recording and replaying sessions — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** item 9 of §10.2 and criterion 6 of §10.3 of the global spec: a run can be recorded in a `.inkrec` file (`inkemu run --record <file>`) and replayed (`inkemu replay <file>`). The replay gives, to the byte, the same consoles, the same end, the same device time, cycles, instructions and trace hash and the same final flash, whatever the host and however the replay is cut into calls. A replay reads no path of the recording, writes nothing, and says what differs when it diverges, with a new exit code, 6.

**Architecture:**

| Crate | Owns | Why a GUI can call it |
|---|---|---|
| `soc-esp32c3` | `inputs.rs`: `AcceptedInput`, `InputJournal` and their bounds; the machine journals every input it accepts (`Esp32C3::input_journal`) | the GUI's keyboard, a script and the CLI are journaled alike, inside the machine |
| `boards` | `builtin_toml(id)`: the text of a built-in profile | a session carries the profile as text |
| `session` (new) | the format `inkemu-session 1` in memory (`Session::to_bytes`, `Session::from_bytes`, bounded), the outcome of a run (`Outcome`, `End`, `ConsoleDigests`) and its comparison (`Divergence`), the replay driver (`Replay`) | no host I/O (its `clippy.toml` bans it), builds for `wasm32-unknown-unknown`: the web version (SP6) replays sessions |
| `emu-cli` | `--record`, `inkemu replay`, exit code 6, the session file (read bounded, written through a temporary file, `0600`) | reading and writing the file only |

**Tech Stack:** Rust 1.99.0 (edition 2024). No new third-party package: `flate2` and `sha2` are workspace dependencies already (`Cargo.toml`), and the deflate backend they resolve to is pure Rust, which builds for `wasm32-unknown-unknown`.

**Spec:** `docs/superpowers/specs/2026-10-14-sp1b-4b-design.md` (decisions G1–G10, binding). Global spec `docs/superpowers/specs/2026-10-11-inkemu-design.md` (§3.1 determinism, §6.4, §7 errors, §10.2 item 9, §10.3 criterion 6). ADR 0004 (inputs of a run), ADR 0006 (inputs of the consoles). Scoping: `docs/research/sp1b-4-scoping.md` §1(c).

**What was verified, and where.** On the development host, by the rule of the project, only `cargo fmt --all`, `tools/ci/clippy.sh -p <touched crates> --all-targets --all-features -- -D warnings`, `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items -p <touched crates>`, `cargo check --target wasm32-unknown-unknown` of the crates without host I/O, and narrow `cargo test -p <crate> <filters>` are run. **Not run on that host**, and left to the CI: `cargo test --workspace`, the `#[ignore]` suites (the real ROM, the reference firmwares, among them the new test of Task 6), coverage, Docker.

## Values only the CI can produce

None is pinned by this plan. The test of Task 6 compares a run with its own replay; it pins no trace hash.

## Global Constraints

- Toolchain pinned: Rust **1.99.0**, edition **2024**. `unsafe_code = "forbid"`; `unwrap_used`/`expect_used` denied outside tests.
- No `TODO`, no placeholder.
- No third-party package added; `cargo deny check` unchanged.
- **No loop or buffer sized by an untrusted value without a constant bound.** A session file is untrusted. Its reader never allocates from a length it read: every length is checked against a constant bound (`MAX_PROFILE_ID_BYTES`, `MAX_PROFILE_TEXT_BYTES`, the flash sizes of `soc_esp32c3::flash`, `MAX_DEFLATED_FLASH_BYTES`, `MAX_CONSOLE_BYTES` per channel, `MAX_HOST_CHANGES`, `MAX_INPUTS`) and against the bytes left in the file before a byte is copied; vectors grow by `push` as fields are read, never by `with_capacity(length read)`. The inflated flash is read through `take(size + 1)`, `size` checked first. The CLI reads at most `MAX_SESSION_BYTES + 1` bytes of a file.
- Determinism (spec §3.1, G7): a replay of a session gives the same outcome whatever the slices (1, 7, 997, 7 919 cycles), through a reset, a deep sleep, console inputs and changes of the USB host.
- `soc-esp32c3`, `firmware`, `boards`, `console-script`, `session`: no host I/O (clippy bans, `crates/boards/tests/host_io_ban.rs` holds the five copies equal), and they build for `wasm32-unknown-unknown`.
- GUI-first: the journal is read from the machine, the format and the replay live in `session`; `emu-cli` reads and writes the file and prints.
- Code, tests, commits, README, ADR and CLI cite the spec's decisions G1–G10 at most; the rulings R1–R13 of this plan are cited by this plan and the hand-over note only.
- Checking a command: redirect its output to a file, then read `$?`; never through a pipe.
- Commits: `type(scope): description`, no trailer, no attribution line. Every commit builds and passes clippy on its own.
- Documents for people (ADR 0007, the hand-over note) are in French; README English then French; code, CLI and this plan in English.

## Review Focus

- **The journal is the machine's.** `send_console` and `set_usb_host` journal what they accept, after every check, and nothing they refuse → `the_machine_journals_the_inputs_it_accepts_and_not_those_it_refuses`. Mutation: journal before the checks → the refused input appears.
- **Replay feeds ahead (R1).** The recording hands inputs mid-run as a script does; the replay hands them all at power-on → `a_session_replays_to_its_outcome_whatever_the_slices` passes only if the machine's contract (an input depends on its date, its bytes and its order, not on the instant it was handed over) holds through a reset, a deep sleep and changes of the USB host.
- **Bounded reading (G4).** Truncation at every byte, every length one past its bound, a version other than 1, a wrong SHA-256 of the profile and of the flash, a flash of the wrong size, bad tags → refused with the field named. Mutation: drop a bound check → its test fails.
- **A cut run replays (G5, R4).** `a_cut_session_replays_to_its_cut` cuts a run at an instant (as Ctrl-C does) and replays it to the same outcome, `End::Cut`.
- **Divergence (G6).** `a_changed_input_byte_diverges_and_names_the_first_quantity`.
- **Exit codes.** `REPLAY_EXIT_CODES_HELP` lists 0, 2, 6, 130; README and `replay --help` agree.
- **Counts of the CI.** `reference-boot`'s `run_cli reference_firmware` goes from 11 to 12; the WebAssembly step builds `session`.

---

## Design decisions taken for this plan

G1–G10 bind it. These were taken while writing it.

| # | Decision | Where | Cost if wrong |
|---|---|---|---|
| R1 | The replay hands every journaled input to the machine at power-on, in the order of the journal. The machine's contract (ADR 0006, `Esp32C3::send_console`) is that a run depends on the date, the bytes and the order of its inputs, not on the instant a front end hands them over: a UART queues the host's bytes from their date (`Uart::receive`), USB-Serial-JTAG queues its packets in the order given and its host changes by date (`UsbSerialJtag::receive`, `set_host`), and both keep the host's side across a reset of the chip. The test of Task 3 records with inputs handed mid-run (as a `wait-for` does) and replays with them all handed at the start. | Task 3 | Medium: a model that broke the contract would diverge, and the tests say where |
| R2 | The bounds of a session are totals over the run: per console channel, at most `MAX_PENDING_HOST_BYTES` (1 MiB) of input, each chunk counting for `max(len, 1)` bytes; at most `MAX_PENDING_HOST_CHANGES` (16 384) changes of the USB host. They are the bounds of ADR 0006 applied to the whole run, stricter than the machine's bounds on what is pending, so that the reader's bounds are constants and a replay that hands everything at power-on stays within the machine's. The journal of the machine stops at these bounds and says it is incomplete; the machine accepts the inputs all the same. Recording an incomplete journal is refused when the run ends (exit 2, 3 after a strict stop). | Tasks 2, 3, 4 | Low: a run that sends more than a megabyte on one console cannot be recorded, and says so |
| R3 | No checksum over the whole file. The profile and the starting flash carry their SHA-256, checked by the reader; a changed input or a changed outcome is caught by the comparison of the replay, which is what acceptance criterion 2 asks (exit 6, the quantity named, not a refusal of the file). | Task 3 | Low |
| R4 | A run the host cuts (Ctrl-C, a reader that went away, a console that cannot be written, at a pass or at the final flush) is recorded with its cut: the device time at which it stopped, and why. The replay adds the cut to its device-time limit, and a stop on a limit at or after the cut instant is `End::Cut`. The cycle count needs no limit of its own: no instruction executes without moving device time, so every stop at the cut instant has the cycles of the recording. | Tasks 3, 4 | Low |
| R5 | `--record` turns the trace hash on (it changes nothing the machine does), and the summary then prints the `trace-hash` line. | Task 4 | Low |
| R6 | The console a run shows is not an input and is not recorded: `inkemu replay` shows the console `--console` names, else the profile's. | Task 4 | Low |
| R7 | A replay the host cuts before the end of its session (Ctrl-C, a reader that went away, a console that cannot be written) compares nothing and exits 2, with a line that says so: 0 means "identical" only. | Task 4 | Low |
| R8 | The replay loads the ROM of the chip revision of the session's profile, as `run` does (`--rom-dir`, else the cache), and refuses before the run when its image SHA-256 is not the session's, naming both and `inkemu rom fetch --device <id>`. | Task 4 | Low |
| R9 | The session file holds the whole flash, network credentials among it (ADR 0004 point 5): it is written through `.<name>.tmp` in its directory and a rename, mode `0600` on Unix. `--record` naming the same file as `--flash`, `--firmware`, `--bootloader`, `--flash-file` or `--console-script` is refused before the run (the check of `flash_state`). | Task 4 | Low |
| R10 | The format is binary, little-endian, its first bytes the line `inkemu-session 1\n`; the fields come in a fixed order (Task 3, Step 2); the starting flash is deflated (`flate2`, default level) and its SHA-256 is that of the inflated bytes. The comparison reports the first quantity that differs in the order of G3 (end, device time, cycles, instructions, trace hash, final flash, consoles), with both values, then names the others that differ. | Task 3 | Low |
| R11 | The reference test of G9 records `uart_select` driven by the script of the console-script reference test, `--console uart0 --max-time 12 --firmware`, replays it (exit 0, the same console), then replays a copy whose first console input is changed (exit 6). | Task 6 | Medium: first CI run |
| R12 | A session carries the text of its profile (`boards::builtin_toml`) and the replay rebuilds the profile from that text (`DeviceProfile::from_named_toml`): a session outlives a later change of the built-in profile. | Tasks 2, 3 | Low |
| R13 | An empty chunk sent to a UART is accepted by the machine and does nothing; it is journaled all the same (cost 1), so that the journal is what the machine accepted. The deep-sleep helpers of the machine's tests move to `test_support`, which the tests of `session` use. | Tasks 2, 3 | Low |

## File Structure

```
.github/workflows/ci.yml                                  wasm32 step + session; run_cli reference_firmware 11 → 12 (Task 6)
Cargo.toml                                                member and dependency `session` (Task 3)
README.md                                                 --record, inkemu replay, exit code 6 (Task 7)
docs/adr/0007-format-des-sessions.md                      new, French (Task 7)
docs/superpowers/notes/2026-10-14-sp1b-4b-carryover.md    new, French (Task 7)
crates/soc-esp32c3/src/inputs.rs                          new: the journal (Task 2)
crates/soc-esp32c3/src/{machine,lib,test_support}.rs      the journal in the machine; deep-sleep helpers (Task 2)
crates/boards/src/lib.rs                                  builtin_toml (Task 2)
crates/boards/tests/host_io_ban.rs                        five crates (Task 3)
crates/session/{Cargo.toml,clippy.toml,src/{lib,format,outcome,replay}.rs,tests/replay.rs}   new (Task 3)
crates/emu-cli/src/{main,run,exit,lib}.rs, src/{record,session_file}.rs   --record, replay, exit 6 (Task 4)
crates/emu-cli/src/flash_state.rs                         same_file made visible to the crate (Task 4)
crates/emu-cli/tests/run_cli.rs                           replay without a ROM, help, the reference test (Tasks 5, 6)
```

---

## Task 1: This plan

- [x] **Step 1:** Commit: `docs(plan): SP1b-4b, recording and replaying sessions`.

## Task 2: The journal of the machine (G1; R2, R12, R13)

**Files:** create `crates/soc-esp32c3/src/inputs.rs`; modify `machine.rs`, `lib.rs`, `test_support.rs`; `crates/boards/src/lib.rs`.

- [x] **Step 1: Write the failing tests** (`inputs.rs`, `machine.rs`): the journal starts with the power-on state of the USB host and no input; an accepted `send_console` and `set_usb_host` are journaled in order with their dates and bytes; a refused one (in the past, too much pending, too many changes) is not; past `MAX_JOURNAL_CONSOLE_BYTES` on a channel or `MAX_JOURNAL_HOST_CHANGES` changes, the journal stops, says it is incomplete, and the machine still accepts the input.
- [x] **Step 2: Code.** `AcceptedInput`, `InputJournal` (`usb_host_at_power_on`, `inputs`, `is_complete`), the bounds; `Esp32C3::input_journal`; journaling after the checks of `send_console` and `set_usb_host`. `boards::builtin_toml`. The deep-sleep helpers `sleep_on_the_rtc_timer` and `TIMER_WAKE_STATE` move from the machine's tests to `test_support` (R13).
- [x] **Step 3: Run** `cargo test -p soc-esp32c3 --lib -- inputs machine::tests::the_machine_journals machine::tests::a_deep`, `cargo test -p boards --lib builtin` → pass.
- [x] **Step 4: Commit:** `feat(soc-esp32c3,boards): the machine journals the inputs it accepts, and a built-in profile gives its text`.

## Task 3: The crate `session` (G2–G4, G7; R1–R4, R10, R12)

**Files:** create `crates/session/`; modify `Cargo.toml`, `crates/boards/tests/host_io_ban.rs`, `.github/workflows/ci.yml` (wasm32 step).

- [x] **Step 1: Write the failing tests.** In `format.rs`: a session goes to bytes and back unchanged; the first bytes are the version line; a file of another version, of no version, cut at every byte, with each length one past its bound, with a wrong SHA-256 of the profile or of the flash, with a flash of another size than its profile, with an unknown tag, with bytes after its end, longer than `MAX_SESSION_BYTES`: refused, the field named. In `outcome.rs`: the first quantity that differs is named with both values, the others after it. In `tests/replay.rs`, with a firmware written in instructions that lives three lives (echo on UART0 of what UART0 and USB-Serial-JTAG receive, a software reset after the first line, a deep sleep on the RTC timer after the second, a deep sleep without a source after the third) and a recording driver that hands inputs as a script does: the session replays to its outcome, twice, and in slices of 1, 7, 997 and 7 919 cycles; through bytes and back; a cut session replays to its cut; a changed input byte diverges; the wrong ROM is refused.
- [x] **Step 2: Code.** The format (R10), field by field: version line; ROM revision (`u16`) and image SHA-256; profile id (`u8` length, at most 64), text (`u32` length, at most 64 KiB) and SHA-256; flash size (`u32`), SHA-256, deflated length (`u32`) and bytes; `--strict-mmio` (`u8`), `--max-cycles` and `--max-time` (a `u8` presence then a `u64`), the cut (a `u8` reason, 0 for none, then the instant in cycles of the time base); the USB host at power-on (`u8`); the count of inputs (`u32`) then each input (a `u8` tag: 0 `uart0`, 1 `usb-serial-jtag`, 2 `uart1`, 3 the USB host; the date, `u64` cycles of the time base; a console's `u32` length and bytes, a host's `u8` state); the outcome: end (`u8`), device time, cycles, instructions, trace hash (`u64` each), the SHA-256 of the final flash and of each console. `Outcome::of`, `ConsoleDigests`, `Outcome::first_difference`. `Replay::start` (ROM check, profile from the text, machine with the starting flash, every input handed over), `Replay::step`, `Replay::finish`; `Limits::run_limits` and `Limits::end_of` (R4), which the CLI uses too. `clippy.toml` is the fifth copy of the ban.
- [x] **Step 3: Run** `cargo test -p session`, `cargo test -p boards --test host_io_ban`, `cargo check --target wasm32-unknown-unknown -p session` → pass.
- [x] **Step 4: Commit:** `feat(session): the format inkemu-session 1, read within constant bounds, and the replay of a session on a machine`.

## Task 4: The command line (G5, G6; R4–R9)

**Files:** create `crates/emu-cli/src/session_file.rs` and `record.rs` (the recorder of a run and the verdict of a replay, both around `execute_with_script`, testable without a ROM); modify `main.rs`, `run.rs`, `exit.rs`, `lib.rs`, `flash_state.rs`, `Cargo.toml`.

- [x] **Step 1: Write the failing tests** (`run.rs`, `session_file.rs`, `exit.rs`): `execute_with_script` given digests takes every console into them, the flushed bytes included; the end of a run maps to the end of a session (a host-cut run to a cut); a recorded run read back and replayed through `execute` gives exit 0, a changed input gives 6; the session file is written through its temporary file, `0600`, refused at a directory; a file longer than the bound is refused before it is read whole; the replay help lists 0, 2, 6 and 130.
- [x] **Step 2: Code.** `--record <FILE>` on `run`; `inkemu replay <FILE> [--console] [--rom-dir]`; `EXIT_DIVERGED = 6`, `REPLAY_EXIT_CODES_HELP`; the header and the verdict lines of a replay.
- [x] **Step 3: Run** `cargo test -p emu-cli --lib`, `cargo test -p emu-cli --test run_cli -- replay record help readme` → pass.
- [x] **Step 4: Commit:** `feat(emu-cli): inkemu run --record writes the session of a run, and inkemu replay replays it and exits 6 when it diverges`.

## Task 5: The README block of `run --help`

The help of `run` gains `--record`: the README's help block changes in the same commit as Task 4, since `the_readme_shows_run_help_as_the_binary_prints_it` holds them equal.

## Task 6: The proof in the CI (G9; R11)

**Files:** `crates/emu-cli/tests/run_cli.rs`, `.github/workflows/ci.yml`.

- [x] **Step 1:** `x4_records_the_uart_select_reference_firmware_typed_into_and_replays_it_identically` (ignored, `INKEMU_ROM_ELF_DIR`, `INKEMU_UART_SELECT_APP`): run with `--record`, replay (exit 0, the same console), replay a copy with its first console input changed (exit 6, `diverges` named).
- [x] **Step 2:** `reference-boot`: `run_cli reference_firmware` 11 → 12.
- [x] **Step 3: Commit:** `test(emu-cli): the CI records uart_select driven by a console script and replays it, identical, then changed`.

## Task 7: Documents (G8, G10)

**Files:** `README.md`; create `docs/adr/0007-format-des-sessions.md`, `docs/superpowers/notes/2026-10-14-sp1b-4b-carryover.md`.

- [x] **Step 1:** ADR 0007 (French): content, version, bounds, what a session never holds (the ROM, paths, host time), the behaviour of the replay, exit code 6.
- [x] **Step 2:** README (EN, then FR): recording and replaying, exit code 6, status and roadmap.
- [x] **Step 3:** The hand-over note (French): results, rulings R1–R13, what is left to each heir, limits of proof.
- [x] **Step 4: Commit:** `docs: ADR 0007, the format of sessions, the README of recording and replay, and the hand-over of SP1b-4b`.
